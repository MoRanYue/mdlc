//! glTF / GLB 源读取（`gltf` crate → 中立 SMD）。
//!
//! # 与 [`crate::fbx`] 的关系
//!
//! 两者是**同一层**的两个实现：都读一种外部资产格式，都产出中立
//! [`Smd`]，都带一份「形变目标 → flex」的元数据（[`crate::fbx::FbxShapeKey`]）。
//! 上层（`compile.rs` 的 `read_source*` 三个分派函数）只按扩展名选一个，
//! 之后**完全不知道**源是什么格式。
//!
//! 复用 [`crate::fbx`] 的：`FbxOpts`（四条 `src*` 选项与格式无关）、
//! `FbxShapeKey` / `FbxGeometry`、`dfs_preorder`、以及 `bone_math` 的
//! `quaternion_angles`。
//!
//! # ⚠️ 这个格式**没有官方 oracle**
//!
//! 官方 `studiomdl.exe` **完全不认 glTF**（`docs/gltf-support.md` §1 的
//! 字符串扫描：`gltf`/`glb`/`KHR_` 全 0 命中，扩展名试探链里也没有）。
//! 所以这里每一条口径都只能来自**传递式 oracle**：同一个 Blender 场景
//! 双导出 `.fbx` + `.glb`，官方编 FBX 给出官方口径的产物，而 mdlc 的 FBX
//! 路径已被那批产物逐字段校准过 ⟹ 只要证明「glTF 的数能推出 FBX 的数」，
//! glTF 就获得了 oracle。
//!
//! 已逐值验证的四条（`docs/gltf-support.md` §2）：
//!
//! 1. **顶点 / 法线 / UV**：`cmp_gltf_vvd.js` 对官方 `.vvd` 的**去重集合**全等
//!    （`export_yup=True` 时；`zup.glb` 的 POSITION 差一个 Z/Y 互换）；
//! 2. **flex 位移**：`flex_vs_vvd.js` 逐值相同（`wide` = ±3 x、`tall` = (0,0,3)）；
//! 3. **骨骼集合**：`probe13.rs` 的名字 / 顺序 / 父子关系与官方逐项一致；
//! 4. **骨骼位移**：与官方差一个 100×（那是 FBX 导出器写进骨架根的
//!    `LclS`，glTF 导出器不写）⟹ **glTF 侧本来就是自洽的**，不需要
//!    `docs/fbx-support.md` §4.6 偏离 12 那条修正。
//!
//! 反过来，**没有**对照物的口径（文档里逐条标了「mdlc 自定」）：
//!
//! * 动画重采样（glTF 有显式秒轴 + 插值模式，FBX 是烘焙关键帧）；
//! * `CubicSpline` 插值（**尚未处理**：降级成线性重采样，切线值被丢掉，
//!   并打一条提示 —— 见 `Channel::apply`）；
//! * `KHR_draco_mesh_compression` / `EXT_meshopt_compression`（**明确报错**）。
//!
//! # 数据口径（与 FBX 的**结构性差别**）
//!
//! | 项 | FBX | glTF |
//! |---|---|---|
//! | 骨骼集合 | 靠蒙皮簇 + 祖先上溯**推断** | **显式** `skin.joints` |
//! | UV | 要翻 V（`1.0 - v`） | **不翻**（已是最终值） |
//! | 顶点位置 | `rot_norm(g)·p + t` | **直接取 `POSITION`** |
//! | 单位缩放 | 骨架根常带 `LclS=100` | 导出器已烘进数据 |
//! | 形变目标名 | `shape.element.name` | `mesh.extras()["targetNames"]` |
//! | 动画 | 关键帧 + 固定 30 fps | 秒轴 + 三种插值 |

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gltf::animation::Interpolation;
use gltf::buffer::Source;
use gltf::mesh::Mode;

use crate::fbx::{FALLBACK_MATERIAL, FbxError, FbxGeometry, FbxOpts, FbxShapeKey, ferr};
use crate::smd::{SmdBoneLink, Smd, SmdFrame, SmdNode, SmdPose, SmdTriangle, SmdVertex};

/// glTF 动画的**默认重采样率**。
///
/// 与 FBX 同值（[`crate::fbx::DEFAULT_FPS`]）—— 官方对 FBX 固定归一化到
/// 30 fps（`docs/fbx-support.md` §1.5），glTF 没有官方对照物，沿用同一值
/// 可以让两套输入在同一份 QC 下产出一致的帧数。
pub const DEFAULT_FPS: f32 = crate::fbx::DEFAULT_FPS;

/// 一个 glTF 源读出来的几何 + 元数据。
///
/// 与 [`crate::fbx::FbxGeometry`] 是**同一个形状** —— 上层 `compile.rs`
/// 直接把它拆成 `SourceGeometry` 的五个字段，两条路径共用一套下游。
pub type GltfGeometry = FbxGeometry;

/// 一个 glTF 源的读取选项。
///
/// **就是** [`FbxOpts`]：四条 `src*` 选项（`srcpart` / `srcmaterial` /
/// `srcscale` / `srcaxis`）按概念命名，与格式无关
/// （`docs/gltf-support.md` §6.2「新增语法 0 条」）。
pub type GltfOpts = FbxOpts;

/// 读取时的一个错误（与 [`FbxError`] 同构，共用同一个类型）。
pub type GltfError = FbxError;

// ---------------------------------------------------------------------------
// 诊断（不是错误）
// ---------------------------------------------------------------------------

/// 读一个 glTF 文件时收集到的**提示**（全部是「官方也会静默通过」的情形）。
///
/// 与 `compile.rs` 的 `fbx_diagnostics` 一样是**纯函数**（返回文案而不是直接
/// 打印），这样这条路径才测得了 —— `diagln!` 走 stdout/stderr，测试里抓不到。
///
/// ⚠️ **报错的那几条不走这里**（data URI 解码失败 / Draco / meshopt 都在
/// 读文件那一步直接返回 `Err`）—— 静默零几何是绝对不能接受的。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GltfNotes {
    /// 有 accessor 没有 `bufferView`（规范语义 = 全零）。crate 的校验层会
    /// 误判为错误，我们放行并按全零处理。
    pub zero_accessors: usize,
    /// 用了 `CubicSpline` 插值的通道数（按线性重采样）。
    pub cubic_spline_channels: usize,
    /// 驱动 `MorphTargetWeights` 的通道数（mdlc 不做逐帧表情权重）。
    ///
    /// ⚠️ 只有 [`read_frames`] 会填这一项（读几何时表情权重动画无所谓）。
    pub morph_weight_channels: usize,
}

impl GltfNotes {
    /// 是否有任何值得报告的提示。
    pub fn is_empty(&self) -> bool {
        self.zero_accessors == 0
            && self.cubic_spline_channels == 0
            && self.morph_weight_channels == 0
    }

    /// 逐条文案（`at` 是调用方给的资产引用串，用于定位）。
    pub fn lines(&self, at: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if self.zero_accessors > 0 {
            out.push(format!(
                "提示：{at} 有 {} 个 accessor 没有 `bufferView`；按 glTF 规范它们表示**全零**，\
                 已照此处理（`gltf` 1.4.1 的校验层会把它误判为错误，mdlc 走的是绕校验的读取路径）。",
                self.zero_accessors
            ));
        }
        if self.cubic_spline_channels > 0 {
            out.push(format!(
                "提示：{at} 有 {} 个动画通道用 `CubicSpline` 插值；mdlc 按**线性**重采样\
                 （三次样条的切线信息被忽略）。要避免这条提示请让导出器改用 `LINEAR`。",
                self.cubic_spline_channels
            ));
        }
        if self.morph_weight_channels > 0 {
            out.push(format!(
                "提示：{at} 有 {} 个动画通道驱动 `MorphTargetWeights`（逐帧表情权重）；\
                 mdlc **不做**逐帧表情权重 —— 表情只由 QC 的 `flex` 语句控制，这些通道被忽略。",
                self.morph_weight_channels
            ));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// 加载与校验
// ---------------------------------------------------------------------------

/// 把 `data:` URI 解成字节。
///
/// ⚠️ **关掉 `import` feature 之后 `gltf` crate 不再自带这一步**
/// （`gltf-1.4.1\src\import.rs` 里的 `Scheme::Data(_, base64) => base64::decode(..)`
/// 整段被 feature 门禁），而 `Source::Uri` 给的是**原样**的 uri 字符串
/// （`buffer.rs:86-88`）。不自己解的话内嵌 `.gltf` 会**静默读出零顶点**
/// （`docs/gltf-support.md` §3.4 实测）。
///
/// 支持两种形态：`data:<mime>;base64,<载荷>` 与 `data:;base64,<载荷>`
/// （mime 可省）。**非 base64 的百分号编码形态不支持** —— 那是给文本用的，
/// 二进制 buffer 不会那么写；真遇到就报错，不猜。
fn decode_data_uri(uri: &str, at: &str) -> Result<Vec<u8>, GltfError> {
    let rest = uri
        .strip_prefix("data:")
        .ok_or_else(|| ferr(format!("{at}：内部错误 —— {uri:?} 不是 data URI")))?;
    let (meta, payload) = rest.split_once(',').ok_or_else(|| {
        ferr(format!(
            "{at}：data URI 里没有逗号，无法定位载荷（前 64 字节：{:?}）",
            &uri[..uri.len().min(64)]
        ))
    })?;
    if !meta.ends_with(";base64") {
        return Err(ferr(format!(
            "{at}：data URI 不是 base64 形态（元信息 {meta:?}）。\
             glTF 的二进制 buffer 应当用 `data:application/octet-stream;base64,...`。"
        )));
    }
    // ⚠️ `base64` 0.13 的 API 是**自由函数** `base64::decode`；
    // `Engine` trait + `engine::general_purpose::STANDARD` 是 0.21 起才有的。
    // 这里钉 `0.13` 是为了与 `gltf` 的 `import` 分支同版本（不新增包）。
    base64::decode(payload.trim()).map_err(|err| {
        ferr(format!(
            "{at}：data URI 的 base64 解码失败（{err}）。\
             这条**不能静默跳过** —— 跳过会让模型读出零顶点却不报错。"
        ))
    })
}

/// 把文件里声明的每个 buffer 都读成字节。
///
/// 三种来源（`gltf::buffer::Source`）：
///
/// * [`Source::Bin`] —— GLB 的 BIN chunk（`Gltf::blob`）；
/// * [`Source::Uri`] 且以 `data:` 开头 —— 内嵌，走 [`decode_data_uri`]；
/// * 其它 `Uri` —— 相对当前文件所在目录的**外部文件**（`.gltf` + `.bin` 分离式）。
fn load_buffers(
    doc: &gltf::Document,
    blob: Option<Vec<u8>>,
    base_dir: &Path,
    at: &str,
) -> Result<Vec<Vec<u8>>, GltfError> {
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(doc.buffers().len());
    for b in doc.buffers() {
        let bytes = match b.source() {
            Source::Bin => blob.clone().ok_or_else(|| {
                ferr(format!(
                    "{at}：buffer[{}] 声明自己是 GLB 的 BIN chunk，但文件里没有 BIN chunk。",
                    b.index()
                ))
            })?,
            Source::Uri(uri) if uri.starts_with("data:") => decode_data_uri(uri, at)?,
            Source::Uri(uri) => {
                // 外部文件：相对当前 glTF 文件所在目录解析。
                let path = base_dir.join(uri);
                std::fs::read(&path).map_err(|err| {
                    ferr(format!(
                        "{at}：读不到外部 buffer {uri:?}（解析为 {}）：{err}",
                        path.display()
                    ))
                })?
            }
        };
        // ⚠️ 自己补的第一项校验（`from_slice_without_validation` 把 crate 的
        // 校验全绕过了）：声明的 `byteLength` 不能超过实际拿到的字节数。
        // 少了这一条，越界的 bufferView 会在 reader 层静默变成 `None`，
        // 表现为「顶点少了一截」而不是报错。
        if bytes.len() < b.length() {
            return Err(ferr(format!(
                "{at}：buffer[{}] 声明 {} 字节，实际只有 {} 字节。",
                b.index(),
                b.length(),
                bytes.len()
            )));
        }
        out.push(bytes);
    }
    Ok(out)
}

/// 自己补的第二、三项校验：每个 bufferView 与每个 accessor 都要落在 buffer 内。
///
/// `from_slice_without_validation` 把 crate 的校验整段跳过了（那是为了绕开
/// 「无 `bufferView` 的 accessor」误判，见 `docs/gltf-support.md` §3.2），
/// 所以长度检查得自己做。三项（§8.1）：
///
/// 1. buffer 声明长度 ≤ 实际字节数 —— 在 [`load_buffers`] 里做了；
/// 2. 每个 bufferView 的 `[byteOffset, byteOffset+byteLength)` 落在对应 buffer 内；
/// 3. accessor 的 `stride × count` 落在其 bufferView 内
///    （⚠️ **没有 `bufferView` 时跳过** —— 规范语义是全零，那正是坑一）。
fn validate_ranges(doc: &gltf::Document, buffers: &[Vec<u8>], at: &str) -> Result<usize, GltfError> {
    for v in doc.views() {
        let bi = v.buffer().index();
        let len = buffers.get(bi).map_or(0, Vec::len);
        let end = v.offset().saturating_add(v.length());
        if end > len {
            return Err(ferr(format!(
                "{at}：bufferView[{}] 覆盖 [{}, {})，超出 buffer[{bi}] 的 {len} 字节。",
                v.index(),
                v.offset(),
                end
            )));
        }
    }

    // 第 3 项：accessor 的字节跨度。`size()` = 单元素字节数（分量大小 × 分量数），
    // `stride` 缺省时等于 `size()`。**没有 bufferView 的 accessor 直接跳过**
    // —— 它的语义是「全零」，没有可校验的范围。
    let mut zero_accessors = 0usize;
    for a in doc.accessors() {
        let Some(v) = a.view() else {
            zero_accessors += 1;
            continue;
        };
        let stride = v.stride().unwrap_or_else(|| a.size());
        if a.count() == 0 {
            continue;
        }
        let end = a
            .offset()
            .saturating_add(stride.saturating_mul(a.count() - 1))
            .saturating_add(a.size());
        if end > v.length() {
            return Err(ferr(format!(
                "{at}：accessor[{}] 的 {} 个元素（步长 {stride}、单元素 {} 字节）\
                 需要 {end} 字节，超出 bufferView[{}] 的 {} 字节。",
                a.index(),
                a.count(),
                a.size(),
                v.index(),
                v.length()
            )));
        }
    }
    Ok(zero_accessors)
}

/// 用了这些扩展就**直接报错** —— crate 不支持它们，静默读出来的几何是错的。
///
/// `gltf` 1.4.1 的 `gltf-json` 里 `draco|meshopt` **零命中**
/// （`docs/gltf-support.md` §3.3），所以压缩过的网格读出来只有未压缩的
/// 那部分属性，顶点数是错的。上游 master 也仍未支持。
const UNSUPPORTED_EXTENSIONS: [&str; 2] = [
    "KHR_draco_mesh_compression",
    "EXT_meshopt_compression",
];

/// 检查 `extensionsRequired` / `extensionsUsed` 里有没有我们不支持的。
///
/// `extensionsUsed` 也查 —— 压缩扩展**必须**同时出现在 `extensionsRequired`
/// 里才是强制的，但只写在 `extensionsUsed` 里也说明作者动过压缩，
/// 值得拦下来问一句（报错文案里会说明是哪一种）。
fn check_extensions(doc: &gltf::Document, at: &str) -> Result<(), GltfError> {
    let required: HashSet<&str> = doc.extensions_required().collect();
    let used: HashSet<&str> = doc.extensions_used().collect();
    for ext in UNSUPPORTED_EXTENSIONS {
        if required.contains(ext) {
            return Err(ferr(format!(
                "{at}：用了 {ext}（列在 `extensionsRequired` 里），mdlc 不支持解压它。\
                 请在导出时关掉网格压缩（Blender 的 glTF 导出器里叫 `Compression`）。"
            )));
        }
        if used.contains(ext) {
            return Err(ferr(format!(
                "{at}：`extensionsUsed` 里有 {ext}；mdlc 不支持它，读出来的几何可能不完整。\
                 请关掉网格压缩后重导。"
            )));
        }
    }
    Ok(())
}

/// 解析文件 + 装 buffer + 补校验，返回 `(文档, buffer 字节, 提示)`。
fn load_document(
    path: &Path,
    at: &str,
) -> Result<(gltf::Document, Vec<Vec<u8>>, GltfNotes), GltfError> {
    let bytes =
        std::fs::read(path).map_err(|err| ferr(format!("{at}：读不到 {}：{err}", path.display())))?;

    // ⭐ **永远走绕校验的这条路径**（`docs/gltf-support.md` §8.1）：
    // 1.4.1 的校验层会把「无 bufferView 的 accessor」误判为错误，而那是
    // 规范允许的（= 全零），且是 Blender 导出 morph target 的常态。
    //
    // 不写成「先试 from_slice，失败再退」—— 那会让行为依赖 crate 版本
    // （同一个文件在两个版本下走不同分支），而且失败重试本身有成本。
    let (doc, blob) = gltf::Gltf::from_slice_without_validation(&bytes)
        .map_err(|err| ferr(format!("{at}：{} 解析失败：{err}", path.display())))
        .map(|g| (g.document, g.blob))?;

    check_extensions(&doc, at)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let buffers = load_buffers(&doc, blob, base_dir, at)?;
    let zero_accessors = validate_ranges(&doc, &buffers, at)?;

    let mut notes = GltfNotes {
        zero_accessors,
        ..Default::default()
    };
    notes.cubic_spline_channels = count_cubic_spline(&doc);
    Ok((doc, buffers, notes))
}

/// 数一下有多少个通道用了 `CubicSpline`（只为发一条提示）。
fn count_cubic_spline(doc: &gltf::Document) -> usize {
    doc.animations()
        .flat_map(|a| a.channels())
        .filter(|c| c.sampler().interpolation() == gltf::animation::Interpolation::CubicSpline)
        .count()
}

/// 数一下有多少个通道驱动 `MorphTargetWeights`（只为发一条提示）。
///
/// ⚠️ 这里用**通道的 target 属性**判断，不读 output 数据 —— 提示只需要条数，
/// 而 `read_outputs()` 会真去解一遍 accessor（代价白花）。
fn count_morph_weight_channels(anim: &gltf::Animation<'_>) -> usize {
    anim.channels()
        .filter(|c| c.target().property() == gltf::animation::Property::MorphTargetWeights)
        .count()
}

// ---------------------------------------------------------------------------
// 场景图
// ---------------------------------------------------------------------------

/// 一个节点在**世界空间**的变换（4×4，列主序，与 [`crate::fbx`] 的
/// `ufbx::Matrix` 同布局，便于复用那边的 `bone_offset`）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct World {
    /// 3×3 部分，按列存：`[列0, 列1, 列2]`。
    m: [[f64; 3]; 3],
    /// 平移列。
    t: [f64; 3],
}

impl World {
    /// 单位阵。
    fn identity() -> Self {
        Self {
            m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            t: [0.0, 0.0, 0.0],
        }
    }

    /// 从 glTF 的局部 TRS 建一个（`T · R · S`，与规范一致）。
    fn from_trs(t: [f32; 3], r: [f32; 4], s: [f32; 3]) -> Self {
        let (x, y, z, w) = (
            f64::from(r[0]),
            f64::from(r[1]),
            f64::from(r[2]),
            f64::from(r[3]),
        );
        // 四元数 → 3×3（列主序，与 glTF 规范的矩阵约定一致）。
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        let (sx, sy, sz) = (
            f64::from(s[0]),
            f64::from(s[1]),
            f64::from(s[2]),
        );
        Self {
            m: [
                [
                    (1.0 - 2.0 * (yy + zz)) * sx,
                    (2.0 * (xy + wz)) * sx,
                    (2.0 * (xz - wy)) * sx,
                ],
                [
                    (2.0 * (xy - wz)) * sy,
                    (1.0 - 2.0 * (xx + zz)) * sy,
                    (2.0 * (yz + wx)) * sy,
                ],
                [
                    (2.0 * (xz + wy)) * sz,
                    (2.0 * (yz - wx)) * sz,
                    (1.0 - 2.0 * (xx + yy)) * sz,
                ],
            ],
            t: [f64::from(t[0]), f64::from(t[1]), f64::from(t[2])],
        }
    }

    /// 从 glTF 的 4×4 矩阵建一个（列主序）。
    fn from_matrix(m: [[f32; 4]; 4]) -> Self {
        Self {
            m: [
                [f64::from(m[0][0]), f64::from(m[0][1]), f64::from(m[0][2])],
                [f64::from(m[1][0]), f64::from(m[1][1]), f64::from(m[1][2])],
                [f64::from(m[2][0]), f64::from(m[2][1]), f64::from(m[2][2])],
            ],
            t: [f64::from(m[3][0]), f64::from(m[3][1]), f64::from(m[3][2])],
        }
    }

    /// `self · rhs`（`self` 是父，`rhs` 是子）。
    fn mul(&self, rhs: &Self) -> Self {
        let mut m = [[0.0f64; 3]; 3];
        for (c, col) in m.iter_mut().enumerate() {
            for (r, cell) in col.iter_mut().enumerate() {
                *cell = self.m[0][r] * rhs.m[c][0]
                    + self.m[1][r] * rhs.m[c][1]
                    + self.m[2][r] * rhs.m[c][2];
            }
        }
        let mut t = [0.0f64; 3];
        for (r, cell) in t.iter_mut().enumerate() {
            *cell = self.m[0][r] * rhs.t[0]
                + self.m[1][r] * rhs.t[1]
                + self.m[2][r] * rhs.t[2]
                + self.t[r];
        }
        Self { m, t }
    }

    /// 把本节点转成 [`crate::fbx`] 的矩阵类型（那边的一整套口径都吃它）。
    ///
    /// ⚠️ `ufbx::Matrix` 是 **12 个字段、无第 4 行**，且**列主序**
    /// （`m00 m10 m20` 是第 0 列）。`Matrix::default()` 是**全 0**，
    /// 所以每个字段都得写。
    fn to_ufbx(self) -> ufbx::Matrix {
        ufbx::Matrix {
            m00: self.m[0][0],
            m10: self.m[0][1],
            m20: self.m[0][2],
            m01: self.m[1][0],
            m11: self.m[1][1],
            m21: self.m[1][2],
            m02: self.m[2][0],
            m12: self.m[2][1],
            m22: self.m[2][2],
            m03: self.t[0],
            m13: self.t[1],
            m23: self.t[2],
        }
    }
}

/// 一个场景节点的局部变换（从 `Node::transform()` 归一化出来）。
fn local_of(n: &gltf::Node) -> World {
    match n.transform() {
        gltf::scene::Transform::Matrix { matrix } => World::from_matrix(matrix),
        // ⭐ `decomposed()` 连 `Matrix` 变体都会自动分解，所以这里其实永远
        // 走这一支；保留 `Matrix` 分支只是为了不用 `unreachable!()`。
        gltf::scene::Transform::Decomposed {
            translation,
            rotation,
            scale,
        } => World::from_trs(translation, rotation, scale),
    }
}

/// 逐节点的父下标表（`parent[i]` = 节点 `i` 的父）。
///
/// glTF 的 `Node::children()` 是**单向**的（只有父知道子），要反查父必须
/// 扫全表。`world_transforms` / `kept_joints` / `reference_poses` / `smd_nodes`
/// 四处都要它，所以建一次传下去 —— 每处都自己扫一遍是 O(节点数²)。
fn parent_table(doc: &gltf::Document) -> Vec<Option<usize>> {
    let mut p: Vec<Option<usize>> = vec![None; doc.nodes().len()];
    for node in doc.nodes() {
        let pi = node.index();
        for c in node.children() {
            p[c.index()] = Some(pi);
        }
    }
    p
}

/// 逐节点算世界矩阵。
///
/// ⚠️ **不能像 [`crate::fbx`] 的 `accumulate_worlds` 那样顺数组累乘** ——
/// ufbx 保证 `scene.nodes` 是 parents-first，而 glTF 的 `scene.nodes` 是
/// **层序**（实测 `rig.glb` 给 `Tip, Mid, Root, Body, Rig`，子先于父）。
/// 顺迭代顺序累乘会读到还没算出来的父矩阵。
///
/// 这里用「迭代到不动点」：每轮把**父已知**的节点算出来，直到全部算完。
/// 比递归简单（不用怕深链爆栈），复杂度是 O(深度 × 节点数)，而真实资产
/// 的深度是个位数。
fn world_transforms(doc: &gltf::Document, parent: &[Option<usize>]) -> Vec<World> {
    let locals: Vec<World> = doc.nodes().map(|n| local_of(&n)).collect();
    accumulate(&locals, parent)
}

/// 给定逐节点的局部矩阵，累乘出逐节点的世界矩阵。
///
/// 算法与 [`world_transforms`] 相同（迭代到不动点，见那里的注释），
/// 只是局部矩阵由调用方给 —— 动画路径要逐帧换一套 locals。
fn accumulate(locals: &[World], parent: &[Option<usize>]) -> Vec<World> {
    let n = locals.len();

    let mut world: Vec<Option<World>> = vec![None; n];
    let mut done = 0usize;
    while done < n {
        let mut progressed = false;
        for i in 0..n {
            if world[i].is_some() {
                continue;
            }
            match parent[i] {
                // 父还没算 ⟹ 下一轮再说。
                Some(p) if world[p].is_none() => continue,
                Some(p) => {
                    world[i] = Some(world[p].expect("父已算过").mul(&locals[i]));
                }
                None => world[i] = Some(locals[i]),
            }
            done += 1;
            progressed = true;
        }
        // 防环：glTF 规范不允许环，真遇到就停手（剩下的留 `None`）。
        if !progressed {
            break;
        }
    }
    world
        .into_iter()
        .map(|w| w.unwrap_or_else(World::identity))
        .collect()
}

// ---------------------------------------------------------------------------
// 收骨
// ---------------------------------------------------------------------------

/// 收骨：**显式** `skin.joints` + 祖先上溯 + DFS 先序。
///
/// # 与 FBX 的差别
///
/// FBX 没有「骨骼列表」这个概念，官方（与 mdlc）靠「被蒙皮权重引用的骨骼
/// ∪ 其全部祖先」**推断**（`docs/fbx-support.md` §1.4）。glTF 有
/// **显式的 `skin.joints`**，所以推断那一步不需要 —— 但**祖先上溯仍要**
/// （`joints` 只列被引用的骨骼，它们的父节点可能不在列表里，而 SMD 的
/// 骨骼表必须是连通的）。
///
/// ⚠️ 顺序仍是 **DFS 先序**（`scene.nodes` 是层序）—— 直接复用
/// [`crate::fbx::dfs_preorder`]，它只吃 `(id, parent)` 与 id 集合，与格式无关。
///
/// ⚠️ **没有蒙皮的网格节点也算一根骨骼**（对齐 FBX 的
/// 「无蒙皮的网格节点自身算一个种子」）—— 静态道具（`box.fbx` / `morph.fbx`）
/// 只有一根骨骼就是这个道理。否则 `SmdInfo::vert_refs` 为空 ⟹ 报
/// 「至少需要一根骨骼」。
fn kept_joints(
    doc: &gltf::Document,
    parent: &[Option<usize>],
    opts: &GltfOpts,
) -> Vec<usize> {
    let mut used: HashSet<u32> = HashSet::new();

    for node in doc.nodes() {
        if !node_selected(&node, opts) {
            continue;
        }
        match node.skin() {
            Some(skin) => {
                for j in skin.joints() {
                    used.insert(j.index() as u32);
                }
            }
            None => {
                // 没有蒙皮的**网格节点**自身算一根；非网格节点（纯 Empty /
                // 骨架根）不主动加 —— 它们只作为祖先被带进来。
                if node.mesh().is_some() {
                    used.insert(node.index() as u32);
                }
            }
        }
    }

    // 祖先上溯：`skin.joints` 只列被引用的，父节点可能不在里面。
    let mut extra: Vec<u32> = Vec::new();
    let mut ids: Vec<u32> = used.iter().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let mut cur = parent[id as usize];
        while let Some(p) = cur {
            let pu = p as u32;
            if used.contains(&pu) {
                break;
            }
            extra.push(pu);
            cur = parent[p];
        }
    }
    used.extend(extra);

    let nodes: Vec<(u32, Option<u32>)> = (0..doc.nodes().len())
        .map(|i| (i as u32, parent[i].map(|p| p as u32)))
        .collect();
    crate::fbx::dfs_preorder(&nodes, &used)
        .into_iter()
        .map(|id| id as usize)
        .collect()
}

/// 该节点是否被 `srcpart` 选中（没写 `srcpart` 时全选）。
///
/// 与 FBX 侧的判据一致（比节点名），但 glTF 上多一个来源：**网格名**。
/// 用户看到的「部件」多半是网格名（Blender 里的 mesh 名），而节点名可能
/// 是 `Body` / `Body.001` 这类。两者任一命中就算选中。
fn node_selected(n: &gltf::Node, opts: &GltfOpts) -> bool {
    if opts.parts.is_empty() {
        return true;
    }
    let node_name = n.name().unwrap_or("");
    let mesh_name = n.mesh().and_then(|m| m.name().map(str::to_string));
    opts.parts.iter().any(|p| {
        p.eq_ignore_ascii_case(node_name)
            || mesh_name
                .as_deref()
                .is_some_and(|m| p.eq_ignore_ascii_case(m))
    })
}

fn smd_nodes(doc: &gltf::Document, parent: &[Option<usize>], kept: &[usize]) -> Vec<SmdNode> {
    let idx_of: HashMap<usize, i32> = kept
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i as i32))
        .collect();
    kept.iter()
        .enumerate()
        .map(|(i, &id)| {
            let n = doc.nodes().nth(id).expect("下标来自 nodes()");
            SmdNode {
                index: i as i32,
                name: n.name().unwrap_or("").to_string(),
                parent: parent[id]
                    .and_then(|p| idx_of.get(&p).copied())
                    .unwrap_or(-1),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 几何
// ---------------------------------------------------------------------------

/// 一个 primitive 的材质名。
///
/// glTF 的材质是**可选**的（`Primitive::material()` 在没写时返回
/// `Material::default()`，其 `name()` 是 `None`），此时按官方对 FBX 的同一条
/// 规则合成 [`FALLBACK_MATERIAL`]（`docs/gltf-support.md` §5.7）。
///
/// `srcmaterial` 写在 QC 里时优先用它 —— 与 FBX 侧的 `material_name` 一致
/// （那边是「网格没有材质时用 `srcmaterial`」）。
fn material_name(node: &gltf::Node, prim: &gltf::Primitive, opts: &GltfOpts) -> String {
    // glTF 的材质可以挂在网格上（`mesh.materials()`）也可以挂在 primitive 上。
    // `Primitive::material()` 已经把两者合并好了，但**没写材质时**它给的是
    // `Material::default()`（`index()` 为 `None`）—— 靠这一点区分「没写」
    // 与「写了但没名字」。
    if prim.material().index().is_some()
        && let Some(name) = prim.material().name()
        && !name.is_empty()
    {
        return name.to_string();
    }
    let _ = node;
    opts.material
        .clone()
        .unwrap_or_else(|| FALLBACK_MATERIAL.to_string())
}

/// 把一个 primitive 的三角形追加到 `out`。
///
/// 与 FBX 侧 `from_scene` 的三角形循环同构，差别只在数据来源：
///
/// | | FBX | glTF |
/// |---|---|---|
/// | 位置 / 法线 / UV | 按**角**下标（`corner`） | 按**顶点**下标（`vi`） |
/// | 权重 | 按顶点下标 | 按顶点下标 |
/// | UV | 要翻 V | **不翻** |
/// | 无权重时 | 绑到网格节点 | 绑到网格节点（同） |
///
/// ⚠️ glTF 的索引就是顶点下标（`POSITION` 数组的下标），没有 SMD 那种
/// 「角」的概念 —— `TEXCOORD_0` 等属性数组与 `POSITION` **等长同序**。
/// 所以这里只用一个 `vi`。
#[allow(clippy::too_many_arguments)]
fn push_primitive<'a, 's, F>(
    prim: &gltf::Primitive<'a>,
    node: &gltf::Node,
    material: &str,
    cp_off: u32,
    bone_ix: Option<i32>,
    idx_of: &HashMap<usize, i32>,
    parent: &[Option<usize>],
    get: F,
    out: &mut Vec<SmdTriangle>,
) where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    let pos = prim.get(&gltf::Semantic::Positions);
    let Some(pos) = pos else {
        // 没有 POSITION 的 primitive 按规范是无效的；静默跳过会让用户看到
        // 「没有任何三角形」，比 panic 好。
        return;
    };
    let positions = read_vec3(&pos, get.clone());
    let normals = prim
        .get(&gltf::Semantic::Normals)
        .map(|a| read_vec3(&a, get.clone()));
    let uvs = prim
        .get(&gltf::Semantic::TexCoords(0))
        .map(|a| read_uv(&a, get.clone()));
    let joints = prim
        .get(&gltf::Semantic::Joints(0))
        .map(|a| read_joints(&a, get.clone()));
    let weights = prim
        .get(&gltf::Semantic::Weights(0))
        .map(|a| read_weights(&a, get.clone()));

    // ⚠️ `JOINTS_0` 是 `skin.joints` 数组的下标（见 [`read_joints`]），
    // 要先经 skin 换成**节点**下标，再经 `idx_of` 换成 SMD 骨骼下标。
    // 节点自己没有 skin 时（静态网格）没有这一层，权重也必然为空。
    let joint_to_node: Vec<usize> = match node.skin() {
        Some(skin) => skin.joints().map(|j| j.index()).collect(),
        None => Vec::new(),
    };
    let _ = parent;

    // 索引：没写 `indices` 时按规范是「0,1,2,…」（非索引化）。
    let indices = match prim.indices() {
        Some(a) => read_indices(&a, get.clone()),
        None => (0..positions.len() as u32).collect(),
    };
    let tris = triangles_of(prim.mode(), &indices);

    for t in tris {
        let mut vs: Vec<SmdVertex> = Vec::with_capacity(3);
        for &vi in &t {
            let vi = vi as usize;
            let p = positions.get(vi).copied().unwrap_or([0.0; 3]);
            let nrm = normals
                .as_ref()
                .and_then(|n| n.get(vi).copied())
                .unwrap_or([0.0, 0.0, 1.0]);
            // ⚠️ **glTF 路径不翻 V**（`docs/gltf-support.md` §5.1）——
            // Blender 的 glTF 导出器写的就是最终值，而 SMD 解析器那次翻转
            // 发生在**解析 SMD** 时，glTF 不经过它。
            let uv = uvs.as_ref().and_then(|u| u.get(vi).copied()).unwrap_or([0.0, 0.0]);

            let mut links = weights_of(
                joints.as_ref().and_then(|j| j.get(vi)),
                weights.as_ref().and_then(|w| w.get(vi)),
                &joint_to_node,
                idx_of,
            );
            // ⚠️ 与 FBX 侧同一条兜底：静态网格（没有 `skin`）的顶点没有权重，
            // 绑到**它所在的那个网格节点**上。不补的话 `SmdInfo::vert_refs`
            // 为空 ⟹ 收骨判据看不到顶点引用 ⟹ 报「至少需要一根骨骼」。
            let parent_bone = links
                .iter()
                .max_by(|a, b| {
                    a.weight
                        .partial_cmp(&b.weight)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|l| l.bone)
                .unwrap_or_else(|| bone_ix.unwrap_or(0));
            if links.is_empty() {
                links.push(SmdBoneLink {
                    bone: parent_bone,
                    weight: 1.0,
                });
            }

            vs.push(SmdVertex {
                parent_bone,
                position: p,
                normal: nrm,
                uv,
                links,
                // 控制点号 = 全局号段 + 本网格内的顶点号（与
                // [`shape_keys_of`] 里 `cp_off + vi` 用的是同一套）。
                src_index: cp_off + vi as u32,
            });
        }
        if vs.len() == 3 {
            out.push(SmdTriangle {
                material: material.to_string(),
                vertices: [vs[0].clone(), vs[1].clone(), vs[2].clone()],
            });
        }
    }
}

/// 读一个 `[f32; 3]` 的 accessor。
///
/// ⚠️ **两种 `None` 必须分开看**（`docs/gltf-support.md` §3.2 的坑一）：
///
/// * accessor **本身不存在** —— 那是调用方用 `Primitive::get` 判断的，
///   轮不到这里；
/// * accessor **没有 `bufferView`** —— 规范语义是**全零**，而 crate 的
///   `Iter::new` 对这种情况返回 `None`（`gltf-1.4.1\src\accessor\util.rs`
///   里 `accessor.view().and_then(..)`，`view()` 为 `None` 就直接短路）。
///
/// 这里统一按**全零**处理（长度取 `count()`）—— 那正是 Blender 对「只改位置、
/// 不改法线」的 morph target 写出来的东西（实测 `morph_zeronrm.glb`）。
/// 把它当读失败会让**最常见的 shape key 形态**直接编不出来。
///
/// ⚠️ `Iter::new` 内部算 `stride × (count − 1)`，`count == 0` 时在 debug 下
/// 会**下溢 panic**，所以零元素要提前返回（release 下则是无意义的越界读）。
///
/// 这个函数之所以能替代 `Primitive::reader`：`mesh::Reader` 的字段是
/// `pub(crate)`（`mesh\mod.rs:126-134`），外部造不出来；但 `accessor::Iter`
/// 是**公开**的（`accessor\mod.rs:76 pub use self::util::{Item, Iter}`），
/// 而且 `Primitive::reader` 内部走的正是同一个 `Iter::new`
/// （`mesh\mod.rs:336-347`）。
fn read_vec3<'a, 's, F>(a: &gltf::Accessor<'a>, get: F) -> Vec<[f32; 3]>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    if a.count() == 0 {
        return Vec::new();
    }
    match gltf::accessor::Iter::new(a.clone(), get) {
        Some(it) => it.collect(),
        None => vec![[0.0; 3]; a.count()],
    }
}

/// 读一个 `[f32; 2]` 的 UV accessor。
///
/// ⚠️ `TEXCOORD_n` 允许 `U8` / `U16`（**归一化**）与 `F32` 三种分量类型，
/// 而 `accessor::Iter` 是按 Rust 类型硬解字节的（`debug_assert_eq!` 会检查
/// `size_of::<T>() == accessor.size()`）⟹ 用错类型在 debug 下直接 panic。
/// 所以这里必须先按 `data_type()` 分派，再按规范做归一化
/// （`u8 / 255`、`u16 / 65535`，与 crate 内部 `Normalize` 的口径一致）。
fn read_uv<'a, 's, F>(a: &gltf::Accessor<'a>, get: F) -> Vec<[f32; 2]>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    use gltf::accessor::DataType;
    if a.count() == 0 {
        return Vec::new();
    }
    match a.data_type() {
        DataType::U8 => match gltf::accessor::Iter::new(a.clone(), get) {
            Some(it) => it
                .map(|v: [u8; 2]| [f32::from(v[0]) / 255.0, f32::from(v[1]) / 255.0])
                .collect(),
            None => vec![[0.0; 2]; a.count()],
        },
        DataType::U16 => match gltf::accessor::Iter::new(a.clone(), get) {
            Some(it) => it
                .map(|v: [u16; 2]| [f32::from(v[0]) / 65535.0, f32::from(v[1]) / 65535.0])
                .collect(),
            None => vec![[0.0; 2]; a.count()],
        },
        _ => match gltf::accessor::Iter::new(a.clone(), get) {
            Some(it) => it.collect(),
            None => vec![[0.0; 2]; a.count()],
        },
    }
}

/// 读一个 `[f32; 4]` 的蒙皮权重 accessor（`WEIGHTS_0`）。
///
/// 与 [`read_uv`] 同样的理由要先按 `data_type()` 分派：`WEIGHTS_n` 也允许
/// `U8` / `U16`（归一化）与 `F32`，且 Blender 默认写 **`F32`**，而别的导出器
/// 可能写 `U8`。
fn read_weights<'a, 's, F>(a: &gltf::Accessor<'a>, get: F) -> Vec<[f32; 4]>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    use gltf::accessor::DataType;
    if a.count() == 0 {
        return Vec::new();
    }
    match a.data_type() {
        DataType::U8 => match gltf::accessor::Iter::new(a.clone(), get) {
            Some(it) => it
                .map(|v: [u8; 4]| v.map(|x| f32::from(x) / 255.0))
                .collect(),
            None => vec![[0.0; 4]; a.count()],
        },
        DataType::U16 => match gltf::accessor::Iter::new(a.clone(), get) {
            Some(it) => it
                .map(|v: [u16; 4]| v.map(|x| f32::from(x) / 65535.0))
                .collect(),
            None => vec![[0.0; 4]; a.count()],
        },
        _ => match gltf::accessor::Iter::new(a.clone(), get) {
            Some(it) => it.collect(),
            None => vec![[0.0; 4]; a.count()],
        },
    }
}

/// 读一个 `[u16; 4]` 的 accessor（`JOINTS_0`）。
///
/// ⚠️ **`JOINTS_0` 是「`skin.joints` 数组的下标」，不是节点下标**
/// （`docs/_probe/gltf_joints.js` 实测 `rig.glb`：底部顶点 `joints[0]` ⟹
/// `skin.joints[0] = 2` = 节点 2 = `Pelvis`）⟹ 拿到之后还要再查一次
/// `skin.joints()`。这个坑很隐蔽：直接当节点下标用会**静默**绑错骨骼，
/// 而骨骼数恰好够多时不会越界，于是产物只是「动起来不对劲」。
///
/// 走 `u16` 是为了兼容 `Uint8` / `Uint16` 两种分量类型（Blender 默认写
/// `Uint8`）；`Uint32` 在规范里不允许用于 `JOINTS_n`。
fn read_joints<'a, 's, F>(a: &gltf::Accessor<'a>, get: F) -> Vec<[u16; 4]>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    use gltf::accessor::DataType;
    if a.count() == 0 {
        return Vec::new();
    }
    match a.data_type() {
        DataType::U8 => match gltf::accessor::Iter::new(a.clone(), get) {
            Some(it) => it
                .map(|v: [u8; 4]| v.map(u16::from))
                .collect(),
            None => vec![[0; 4]; a.count()],
        },
        DataType::U16 => match gltf::accessor::Iter::new(a.clone(), get) {
            Some(it) => it.collect(),
            None => vec![[0; 4]; a.count()],
        },
        // 规范里 `JOINTS_n` 只能是 U8 / U16；别的取值是坏文件，按全零处理
        // 会让顶点绑到第 0 根骨骼上，所以这里报不了错也至少不 panic。
        _ => vec![[0; 4]; a.count()],
    }
}

/// 读索引 accessor（`primitive.indices()`），统一成 `u32`。
fn read_indices<'a, 's, F>(a: &gltf::Accessor<'a>, get: F) -> Vec<u32>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    use gltf::accessor::DataType;
    if a.count() == 0 {
        return Vec::new();
    }
    match a.data_type() {
        // ⚠️ 分量类型必须**显式**写在 `Iter::<T>` 上：`it.map(u32::from)` 里的
        // `u32::from` 对多种整型都有实现，编译器推不出 `T`（E0283）。
        DataType::U8 => match gltf::accessor::Iter::<u8>::new(a.clone(), get) {
            Some(it) => it.map(u32::from).collect(),
            None => vec![0; a.count()],
        },
        DataType::U16 => match gltf::accessor::Iter::<u16>::new(a.clone(), get) {
            Some(it) => it.map(u32::from).collect(),
            None => vec![0; a.count()],
        },
        DataType::U32 => match gltf::accessor::Iter::<u32>::new(a.clone(), get) {
            Some(it) => it.collect(),
            None => vec![0; a.count()],
        },
        _ => Vec::new(),
    }
}

/// 把 `primitive.indices()` 的结果按 `mode` 展开成三角形列表（每 3 个一组）。
///
/// `mode` 缺省是 `TRIANGLES`（规范）。`TRIANGLE_STRIP` / `TRIANGLE_FAN` 也一并
/// 支持 —— 它们只是索引空间的重排，顺手做了比报错更省事；`POINTS` / `LINES`
/// 之类没有面，返回空（调用方会看到「没有任何三角形」那条错误）。
fn triangles_of(mode: Mode, idx: &[u32]) -> Vec<[u32; 3]> {
    match mode {
        Mode::Triangles => idx
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect(),
        Mode::TriangleStrip => (0..idx.len().saturating_sub(2))
            .map(|i| {
                // 奇数号三角形要翻转绕序（规范要求）。
                if i % 2 == 0 {
                    [idx[i], idx[i + 1], idx[i + 2]]
                } else {
                    [idx[i + 1], idx[i], idx[i + 2]]
                }
            })
            .collect(),
        Mode::TriangleFan => (1..idx.len().saturating_sub(1))
            .map(|i| [idx[0], idx[i], idx[i + 1]])
            .collect(),
        _ => Vec::new(),
    }
}

/// 从一个网格的 `extras.targetNames` 取形变目标名。
///
/// glTF 2.0 **没有**给 morph target 起名字的字段，`targetNames` 是
/// **事实标准**（Blender 导出器、three.js、model-viewer 都用它），
/// 放在 `mesh.extras` 里（`docs/gltf-support.md` §5.5）。
///
/// 拿不到就返回空表，调用方回退到 [`crate::fbx::shape_key_frame`] 的编号名。
/// ⚠️ `extras()` 返回的是 `&Option<Box<RawValue>>`（`gltf-json` 的
/// `pub type Extras = Option<Box<RawValue>>`），所以要 `.as_ref()` 再
/// `RawValue::get()` 拿原始 JSON 串自己解析 —— 那正是 `extras` feature 的用法。
fn morph_target_names(mesh: &gltf::Mesh) -> Vec<String> {
    let Some(raw) = mesh.extras().as_ref() else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.get()) else {
        return Vec::new();
    };
    v.get("targetNames")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 收集**一个 primitive** 的形变目标，变换成 [`FbxShapeKey`]。
///
/// 与 FBX 侧的 `shape_keys_of` 同构（下游 `resolve_shape_key_flexes` 因此
/// 一行都不用改），差别只在数据来源：
///
/// | | FBX | glTF |
/// |---|---|---|
/// | 名字 | `shape.element.name` | `mesh.extras()["targetNames"]`（回退编号） |
/// | 位移 | `rot_norm(geometry_to_world) · pos_off` | **直接取** `POSITION` 的差值 |
/// | 法线 | 有（官方丢弃） | 有（官方无对照，mdlc 也丢弃） |
///
/// ⚠️ glTF 的 morph target `POSITION` **本身就是位移量**（不是绝对位置），
/// 所以不需要减去基准位置（`docs/gltf-support.md` §5.5）。
///
/// ⚠️ 顶点映射**按顶点下标**（`vi`），不是角下标 —— glTF 的 morph target
/// 与 `POSITION` 共用同一套顶点编号。
fn shape_keys_of<'a, 's, F>(
    prim: &gltf::Primitive<'a>,
    names: &[String],
    cp_off: u32,
    get: F,
) -> Vec<FbxShapeKey>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    /// `0.001²`，与 `simplify.cpp:2537` 的 `DotProduct(delta,delta) > 0.001f*0.001f`
    /// 以及 [`crate::flex`] 的 `MIN_DELTA_SQR` 同口径（FBX 侧同值）。
    const MIN_DELTA_SQR: f32 = 0.001 * 0.001;

    let mut out: Vec<FbxShapeKey> = Vec::new();
    for (ti, target) in prim.morph_targets().enumerate() {
        let Some(a) = target.positions() else {
            continue;
        };
        let offsets = read_vec3(&a, get.clone());
        let normals = target
            .normals()
            .map(|na| read_vec3(&na, get.clone()))
            .unwrap_or_default();

        let mut position_offsets = Vec::with_capacity(offsets.len());
        let mut normal_offsets = Vec::with_capacity(offsets.len());
        let mut vertex_index = Vec::with_capacity(offsets.len());
        for (vi, d) in offsets.iter().enumerate() {
            if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= MIN_DELTA_SQR {
                continue;
            }
            position_offsets.push(*d);
            normal_offsets.push(normals.get(vi).copied().unwrap_or([0.0; 3]));
            vertex_index.push(cp_off + vi as u32);
        }
        if vertex_index.is_empty() {
            continue;
        }
        let name = names
            .get(ti)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| FbxGeometry::shape_key_frame(ti).to_string());
        out.push(FbxShapeKey {
            name,
            position_offsets,
            normal_offsets,
            vertex_index,
        });
    }
    out
}

/// 一个顶点的蒙皮绑定。
///
/// `joints` 是 `JOINTS_0` 的四个值，它们是 **`skin.joints` 数组的下标**
/// （见 [`read_joints`]），所以这里要先经 `joint_to_node` 换成节点下标，
/// 再经 `idx_of` 换成 SMD 骨骼下标。
fn weights_of(
    joints: Option<&[u16; 4]>,
    weights: Option<&[f32; 4]>,
    joint_to_node: &[usize],
    idx_of: &HashMap<usize, i32>,
) -> Vec<SmdBoneLink> {
    let (Some(j), Some(w)) = (joints, weights) else {
        return Vec::new();
    };
    let mut out: Vec<SmdBoneLink> = Vec::new();
    for k in 0..4 {
        if w[k] <= 0.0 {
            continue;
        }
        let Some(&node) = joint_to_node.get(j[k] as usize) else {
            continue;
        };
        let Some(&bone) = idx_of.get(&node) else {
            continue;
        };
        out.push(SmdBoneLink {
            bone,
            weight: w[k],
        });
    }
    // 权重降序（与 FBX 侧 `weights_of` 一致；下游按这个顺序取前 3 组）。
    out.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// 参考姿态（= 源的第 0 帧）。
///
/// 与 FBX 侧的 `reference_poses` 同构：位移取 [`crate::fbx::bone_offset`]
/// （根骨骼取 [`crate::fbx::normalized_translation`]），旋转取节点自己的
/// **局部**四元数。
///
/// ⚠️ glTF 的节点本来就不带 FBX 那种 `localS = 100`，所以
/// `normalized_translation` 在这里是恒等 —— 但**照样要走它**：万一某个
/// 导出器真的写了缩放，两边用同一个判据才不会又分道扬镳。
fn reference_poses(
    doc: &gltf::Document,
    parent: &[Option<usize>],
    world: &[World],
    kept: &[usize],
    opts: &GltfOpts,
) -> Vec<SmdPose> {
    kept.iter()
        .enumerate()
        .map(|(i, &id)| {
            let n = doc.nodes().nth(id).expect("下标来自 kept_joints");
            let (_, rot, _) = n.transform().decomposed();
            let pos = match parent[id] {
                Some(p) => crate::fbx::bone_offset(&world[p].to_ufbx(), &world[id].to_ufbx()),
                None => crate::fbx::normalized_translation(&world[id].to_ufbx()),
            };
            let (pos, rot) = opts.local(ufbx::Transform {
                translation: pos,
                rotation: ufbx::Quat {
                    x: f64::from(rot[0]),
                    y: f64::from(rot[1]),
                    z: f64::from(rot[2]),
                    w: f64::from(rot[3]),
                },
                scale: ufbx::Vec3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            });
            SmdPose {
                bone: i as i32,
                position: [pos.x as f32, pos.y as f32, pos.z as f32],
                rotation: crate::fbx::source_euler_from_quat(rot.x, rot.y, rot.z, rot.w),
            }
        })
        .collect()
}

/// 一个文件里所有动画栈的名字（没名字的按 `animationN` 合成）。
///
/// [`read`] 拿它填 `FbxGeometry::anim_stacks`，[`read_frames`] 拿它做
/// `srcstack` 的匹配与报错文案 —— **两处必须用同一个函数**，否则诊断里
/// 印出来的名字会跟用户能写的名字对不上。
fn anim_names(doc: &gltf::Document) -> Vec<String> {
    doc.animations()
        .enumerate()
        .map(|(i, a)| match a.name() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => format!("animation{i}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 主入口
// ---------------------------------------------------------------------------

/// 一个节点上挂的**网格实例**：`(节点下标, 网格下标)`。
///
/// glTF 的 `Node::mesh()` 给的是网格，而一个网格可以被多个节点引用
/// （instancing）。每个引用都是一个独立的几何来源 —— 但它们的顶点
/// **共用同一套编号**，所以控制点号段要按「网格」而不是按「节点」分配。
fn mesh_instances(doc: &gltf::Document, opts: &GltfOpts) -> Vec<(usize, usize)> {
    doc.nodes()
        .filter(|n| node_selected(n, opts))
        .filter_map(|n| n.mesh().map(|m| (n.index(), m.index())))
        .collect()
}

/// accessor → 字节 的取值器（`gltf::accessor::Iter::new` 的第二个参数）。
///
/// ⚠️ **为什么是一个具名函数而不是就地写的闭包**：就地写 `move |b: gltf::Buffer|
/// buffers.get(b.index()).map(Vec::as_slice)` 会把生命周期推成
/// `for<'x> Fn(gltf::Buffer<'x>) -> Option<&'x [u8]>`（输出借自**输入**），
/// 而 `Iter::new` 要的是 `Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>`（输出借自
/// **buffers**）。两者只在「文档与 buffers 共享同一个借用」时才等价 —— 这里
/// 不是（`doc` 与 `buffers` 是 `load_document` 返回的两个独立值），于是就地
/// 写会报 `returning this value requires that '1 must outlive '2`。
fn buffer_getter<'a, 's>(
    buffers: &'s [Vec<u8>],
) -> impl Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]> + 's {
    move |b: gltf::Buffer<'a>| buffers.get(b.index()).map(Vec::as_slice)
}

/// 读一个 glTF / GLB 文件的几何，转成中立 SMD。
///
/// 返回的 [`GltfGeometry`] 与 [`crate::fbx::read`] 同构 ⟹ 上层 `compile.rs`
/// 两条路径共用一套下游（材质表、shape key → flex、LOD、诊断）。
///
/// # 诊断
///
/// [`GltfNotes`] 里的每一条都在这里 `diagln!` 出去（与 `compile.rs` 的
/// `emit_fbx_diagnostics` 一个路数）。**报错的那几条不在这里** —— data URI
/// 解码失败 / Draco / meshopt 都在读文件那一步就返回 `Err` 了。
///
/// ⚠️ 与 FBX 不同，glTF **没有**「骨骼被过滤」这条诊断：收骨用的是显式的
/// `skin.joints`，没有 FBX 那套「蒙皮簇 + 祖先上溯」的推断，也就没有
/// 「某个节点被丢掉了」这种需要解释的情形。
pub fn read(path: &Path, at: &str, opts: &GltfOpts) -> Result<GltfGeometry, GltfError> {
    let (doc, buffers, notes) = load_document(path, at)?;
    for line in notes.lines(at) {
        crate::diagln!("{line}");
    }

    let parent = parent_table(&doc);
    let kept = kept_joints(&doc, &parent, opts);
    let nodes = smd_nodes(&doc, &parent, &kept);
    let idx_of: HashMap<usize, i32> = kept
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i as i32))
        .collect();
    let world = world_transforms(&doc, &parent);
    let poses = reference_poses(&doc, &parent, &world, &kept, opts);

    // accessor → 字节 的闭包（`Iter::new` 要吃它）。按 `Buffer::index()` 取。
    let get = buffer_getter(&buffers);

    let mut triangles: Vec<SmdTriangle> = Vec::new();
    let mut shape_keys: Vec<FbxShapeKey> = Vec::new();
    let mut untextured_meshes: Vec<String> = Vec::new();
    let mut merged_meshes: Vec<String> = Vec::new();

    // 控制点号的全局起点。**按网格**（不是按节点）分配 —— 同一个网格被多个
    // 节点引用时，两处的顶点编号是同一套，各分一段会让 shape key 的
    // `vertex_index` 只对上其中一处。
    let mut cp_base: u32 = 0;
    let mut cp_done: HashMap<usize, u32> = HashMap::new();

    for (node_ix, mesh_ix) in mesh_instances(&doc, opts) {
        let node = doc.nodes().nth(node_ix).expect("下标来自 nodes()");
        let mesh = doc.meshes().nth(mesh_ix).expect("下标来自 meshes()");
        let mesh_name = mesh
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("mesh{mesh_ix}"));
        if !merged_meshes.contains(&mesh_name) {
            merged_meshes.push(mesh_name.clone());
        }
        let bone_ix = idx_of.get(&node_ix).copied();
        let cp_off = *cp_done.entry(mesh_ix).or_insert_with(|| {
            let off = cp_base;
            cp_base += mesh
                .primitives()
                .map(|p| p.get(&gltf::Semantic::Positions).map_or(0, |a| a.count()))
                .max()
                .unwrap_or(0) as u32;
            off
        });

        let names = morph_target_names(&mesh);
        for prim in mesh.primitives() {
            let mat = material_name(&node, &prim, opts);
            if (mat == FALLBACK_MATERIAL || Some(&mat) == opts.material.as_ref())
                && !untextured_meshes.contains(&mesh_name)
            {
                untextured_meshes.push(mesh_name.clone());
            }
            // ⚠️ 形变目标的名字是**网格级**的（`mesh.extras.targetNames`），
            // 而 morph target 挂在 primitive 上。多 primitive 的网格里每个
            // primitive 各有一份 target 列表，但名字表是共享的。
            shape_keys.extend(shape_keys_of(&prim, &names, cp_off, get.clone()));
            push_primitive(
                &prim, &node, &mat, cp_off, bone_ix, &idx_of, &parent, get.clone(),
                &mut triangles,
            );
        }
    }

    if triangles.is_empty() {
        return Err(ferr(format!(
            "{at}：{} 里没有任何三角形（网格）。\
             若写了 `srcpart`，请核对网格名字是否与文件里的一致。",
            path.display()
        )));
    }

    Ok(GltfGeometry {
        smd: Smd {
            version: 1,
            nodes,
            frames: vec![SmdFrame { time: 0, poses }],
            triangles,
        },
        shape_keys,
        anim_stacks: anim_names(&doc),
        untextured_meshes,
        merged_meshes,
    })
}

/// 读一个 glTF / GLB 文件的动画，转成 SMD 帧。
///
/// # 与 FBX 的差别
///
/// FBX 路径让 ufbx 把关键帧**烘焙**成定长采样（`bake_anim` + `resample_rate`），
/// glTF 没有这个设施，要自己按秒轴求值：
///
/// * `input` accessor = **秒**（不是帧号）；
/// * `output` accessor = 关键帧值，个数与 `input` 相同（`CubicSpline` 是 3 倍）；
/// * 采样点 = `time_begin + i / fps`，`i` 从 0 到 `⌊(time_end − time_begin) × fps⌉`；
/// * 通道缺省 = 保持参考姿态（Blender 对「没动的通道」写 `Step` 常量，
///   所以「缺省」与「常量」是两件事，都要处理）。
///
/// ⚠️ 插值只处理 `Step` 与 `Linear`；`CubicSpline` 按线性重采样并**发一条提示**
/// （`docs/gltf-support.md` §5.6）。
///
/// ⚠️ 骨骼集合与 [`read`] **完全一致**（同一套收骨），所以帧里的
/// `SmdPose.bone` 可以直接与几何侧的 `nodes` 对齐。
pub fn read_frames(
    path: &Path,
    at: &str,
    stack: Option<&str>,
    fps: f32,
    opts: &GltfOpts,
) -> Result<Smd, GltfError> {
    let (doc, buffers, notes) = load_document(path, at)?;
    for line in notes.lines(at) {
        crate::diagln!("{line}");
    }

    let parent = parent_table(&doc);
    let kept = kept_joints(&doc, &parent, opts);
    let nodes = smd_nodes(&doc, &parent, &kept);
    let get = buffer_getter(&buffers);

    // 参考姿态（= 没有动画时的姿态，也是每条通道缺省时的取值）。
    let reference_world = world_transforms(&doc, &parent);
    let reference: Vec<SmdPose> = reference_poses(&doc, &parent, &reference_world, &kept, opts);

    let names = anim_names(&doc);
    let anim = match stack {
        None => {
            // ⭐ 多栈而用户没点名时发提示。**与 FBX 不同**：glTF 的栈有名字，
            // 而 `srcstack` 就是按名字选的，所以这条提示只是告知默认值。
            if names.len() > 1 {
                crate::diagln!(
                    "提示：{} 有 {} 条动画 {:?}；默认只用**第一条**。要用别的写 `srcstack \"名\"`（写在 `$sequence` / `$animation` 里）。",
                    path.display(),
                    names.len(),
                    names
                );
            }
            doc.animations().next()
        }
        Some(want) => doc
            .animations()
            .find(|a| a.name().is_some_and(|n| n.eq_ignore_ascii_case(want)))
            .or_else(|| {
                // 也允许按序号点名（`animation2`），与 [`anim_names`] 的合成名一致。
                doc.animations().enumerate().find_map(|(i, a)| {
                    (format!("animation{i}").eq_ignore_ascii_case(want)).then_some(a)
                })
            }),
    };

    // 没有动画（或点名了不存在的）⟹ 1 帧的参考姿态。
    // ⚠️ 与 FBX 同一条口径：**「文件里没有动画」不是错误**（静态文件本来就是
    // 1 帧），只有**显式点名**了不存在的栈才是错误。
    let Some(anim) = anim else {
        if let Some(want) = stack {
            return Err(ferr(format!(
                "{} 里没有名为 {want:?} 的动画；现有的是 {names:?}。",
                path.display()
            )));
        }
        return Ok(Smd {
            version: 1,
            nodes,
            frames: vec![SmdFrame {
                time: 0,
                poses: reference,
            }],
            triangles: Vec::new(),
        });
    };

    let fps = if fps > 0.0 { fps } else { DEFAULT_FPS };
    let channels = sample_channels(&anim, &get);
    // ⭐ 逐帧表情权重 mdlc 不做（表情只由 QC 的 `flex` 控制）。这里只统计条数
    // 用来发一条提示 —— 静默忽略会让用户以为「glTF 里的表情动画生效了」。
    let morph_notes = GltfNotes {
        morph_weight_channels: count_morph_weight_channels(&anim),
        ..GltfNotes::default()
    };
    for line in morph_notes.lines(at) {
        crate::diagln!("{line}");
    }
    let (begin, end) = time_range(&channels);
    let n_frames = ((end - begin) * f64::from(fps)).round().max(0.0) as usize + 1;

    // 逐帧：从**参考姿态**（节点自己的 TRS）出发，把有通道的节点覆盖掉，
    // 再累乘世界矩阵。没有通道的节点因此天然保持参考姿态 —— 那正是规范
    // 对「某节点没有动画」的要求。
    let base: Vec<LocalTrs> = doc.nodes().map(|n| local_trs(&n)).collect();
    let mut frames: Vec<SmdFrame> = Vec::with_capacity(n_frames);
    for fi in 0..n_frames {
        let t = begin + (fi as f64) / f64::from(fps);
        let mut trs = base.clone();
        for ch in &channels {
            if let Some(node) = ch.node
                && let Some(slot) = trs.get_mut(node)
            {
                ch.apply(t, slot);
            }
        }
        let locals: Vec<World> = trs.iter().map(|t| t.to_world()).collect();
        let world = accumulate(&locals, &parent);

        let mut poses: Vec<SmdPose> = Vec::with_capacity(kept.len());
        for (i, &id) in kept.iter().enumerate() {
            let pos = match parent[id] {
                Some(p) => crate::fbx::bone_offset(&world[p].to_ufbx(), &world[id].to_ufbx()),
                None => crate::fbx::normalized_translation(&world[id].to_ufbx()),
            };
            let (pos, rot) = opts.local(ufbx::Transform {
                translation: pos,
                rotation: trs[id].rotation,
                scale: ufbx::Vec3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            });
            poses.push(SmdPose {
                bone: i as i32,
                position: [pos.x as f32, pos.y as f32, pos.z as f32],
                rotation: crate::fbx::source_euler_from_quat(rot.x, rot.y, rot.z, rot.w),
            });
        }
        frames.push(SmdFrame {
            time: fi as i32,
            poses,
        });
    }

    Ok(Smd {
        version: 1,
        nodes,
        frames,
        triangles: Vec::new(),
    })
}

/// 一个节点的局部 TRS（动画逐帧改的就是它）。
///
/// ⚠️ **不 derive `PartialEq`**：`ufbx::Vec3` / `ufbx::Quat` 没实现 `PartialEq`
/// （它们来自 C 绑定，只有 `Debug` / `Clone` / `Copy`）。
#[derive(Debug, Clone, Copy)]
struct LocalTrs {
    translation: ufbx::Vec3,
    rotation: ufbx::Quat,
    scale: ufbx::Vec3,
}

impl LocalTrs {
    fn to_world(self) -> World {
        World::from_trs(
            [
                self.translation.x as f32,
                self.translation.y as f32,
                self.translation.z as f32,
            ],
            [
                self.rotation.x as f32,
                self.rotation.y as f32,
                self.rotation.z as f32,
                self.rotation.w as f32,
            ],
            [self.scale.x as f32, self.scale.y as f32, self.scale.z as f32],
        )
    }
}

/// 一个节点的局部 TRS（从 `Node::transform()` 取）。
fn local_trs(n: &gltf::Node) -> LocalTrs {
    let (t, r, s) = n.transform().decomposed();
    LocalTrs {
        translation: ufbx::Vec3 {
            x: f64::from(t[0]),
            y: f64::from(t[1]),
            z: f64::from(t[2]),
        },
        rotation: ufbx::Quat {
            x: f64::from(r[0]),
            y: f64::from(r[1]),
            z: f64::from(r[2]),
            w: f64::from(r[3]),
        },
        scale: ufbx::Vec3 {
            x: f64::from(s[0]),
            y: f64::from(s[1]),
            z: f64::from(s[2]),
        },
    }
}

/// 一条动画通道（已经解码成 `f32` 的关键帧序列）。
#[derive(Debug, Clone)]
struct Channel {
    /// 目标节点（`None` = 通道指向不存在的节点，规范不允许，静默跳过）。
    node: Option<usize>,
    /// 关键帧时刻（**秒**），升序。
    times: Vec<f32>,
    /// 插值模式（`CubicSpline` 已按线性处理，这里只会是 `Step` / `Linear`）。
    interp: Interpolation,
    /// 目标属性。
    ///
    /// ⚠️ **必须存下来**：`ChannelValues::Vec3` 同时承载 `translation` 与
    /// `scale` 两种属性（两者的分量形状一样），靠 `read_outputs()` 的 enum
    /// 臂区分只能知道「是 3 分量」，写回 `LocalTrs` 时还要知道写哪个字段。
    property: gltf::animation::Property,
    /// 关键帧值。
    values: ChannelValues,
}

/// 一条通道的值（按 `Property` 分派）。
#[derive(Debug, Clone)]
enum ChannelValues {
    /// `translation` / `scale`：每个关键帧一个 `[x,y,z]`。
    Vec3(Vec<[f32; 3]>),
    /// `rotation`：每个关键帧一个四元数 `[x,y,z,w]`。
    Quat(Vec<[f32; 4]>),
}

impl Channel {
    /// 把这条通道在时刻 `t` 的取值写进 `slot`。
    fn apply(&self, t: f64, slot: &mut LocalTrs) {
        let t = t as f32;
        let i = match self.interp {
            // `Step`：取最后一个 `times[k] <= t` 的关键帧（不做插值）。
            Interpolation::Step => match self.times.partition_point(|&x| x <= t) {
                0 => 0,
                k => k - 1,
            },
            // `Linear`（含被降级处理的 `CubicSpline`）：夹在相邻两帧之间。
            _ => {
                let k = self.times.partition_point(|&x| x <= t);
                if k == 0 {
                    0
                } else if k >= self.times.len() {
                    self.times.len() - 1
                } else {
                    let (t0, t1) = (self.times[k - 1], self.times[k]);
                    let span = t1 - t0;
                    let u = if span > 0.0 { (t - t0) / span } else { 0.0 };
                    self.apply_lerp(k, u, slot);
                    return;
                }
            }
        };
        self.apply_key(i, slot);
    }

    /// 直接取第 `i` 个关键帧。
    fn apply_key(&self, i: usize, slot: &mut LocalTrs) {
        match &self.values {
            ChannelValues::Vec3(v) => {
                let Some(&x) = v.get(i) else { return };
                let val = ufbx::Vec3 {
                    x: f64::from(x[0]),
                    y: f64::from(x[1]),
                    z: f64::from(x[2]),
                };
                // 用「哪个分量与参考姿态不同」来区分 translation / scale 是不
                // 可靠的；改由构造时写进 `ChannelValues` 的变体 + 这里的
                // `slot` 字段名对齐 —— 见 `sample_channels` 里的分派。
                match self.property {
                    gltf::animation::Property::Translation => slot.translation = val,
                    gltf::animation::Property::Scale => slot.scale = val,
                    _ => {}
                }
            }
            ChannelValues::Quat(v) => {
                let Some(&q) = v.get(i) else { return };
                slot.rotation = ufbx::Quat {
                    x: f64::from(q[0]),
                    y: f64::from(q[1]),
                    z: f64::from(q[2]),
                    w: f64::from(q[3]),
                };
            }
        }
    }

    /// 在关键帧 `k-1` 与 `k` 之间按 `u ∈ [0,1]` 线性插值。
    fn apply_lerp(&self, k: usize, u: f32, slot: &mut LocalTrs) {
        match &self.values {
            ChannelValues::Vec3(v) => {
                let (Some(&a), Some(&b)) = (v.get(k - 1), v.get(k)) else {
                    return;
                };
                let val = ufbx::Vec3 {
                    x: f64::from(a[0] + (b[0] - a[0]) * u),
                    y: f64::from(a[1] + (b[1] - a[1]) * u),
                    z: f64::from(a[2] + (b[2] - a[2]) * u),
                };
                match self.property {
                    gltf::animation::Property::Translation => slot.translation = val,
                    gltf::animation::Property::Scale => slot.scale = val,
                    _ => {}
                }
            }
            ChannelValues::Quat(v) => {
                let (Some(&a), Some(&b)) = (v.get(k - 1), v.get(k)) else {
                    return;
                };
                slot.rotation = nlerp(
                    ufbx::Quat {
                        x: f64::from(a[0]),
                        y: f64::from(a[1]),
                        z: f64::from(a[2]),
                        w: f64::from(a[3]),
                    },
                    ufbx::Quat {
                        x: f64::from(b[0]),
                        y: f64::from(b[1]),
                        z: f64::from(b[2]),
                        w: f64::from(b[3]),
                    },
                    f64::from(u),
                );
            }
        }
    }
}

/// 四元数的归一化线性插值（nlerp）。
///
/// glTF 规范对 `LINEAR` 旋转的定义就是球面线性插值（slerp），但 nlerp 与
/// slerp 在**单位四元数之间**给出同一条旋转路径（只是角速度不均），而
/// Source 的骨骼姿态最终都要转成欧拉角再逐帧写进 `.ani`，角度差在
/// 1e-3 弧度量级 —— 而 ufbx 的 FBX 路径用的也是 nlerp 口径。取 nlerp 是
/// 为了与 FBX 侧一致（`ufbx::evaluate_baked_quat` 内部就是 nlerp + 归一化）。
///
/// ⚠️ 必须处理**符号**：`q` 与 `−q` 表示同一个旋转，直接插值会绕远路。
fn nlerp(a: ufbx::Quat, b: ufbx::Quat, u: f64) -> ufbx::Quat {
    let dot = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w;
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let (x, y, z, w) = (
        a.x + (b.x * sign - a.x) * u,
        a.y + (b.y * sign - a.y) * u,
        a.z + (b.z * sign - a.z) * u,
        a.w + (b.w * sign - a.w) * u,
    );
    let len = (x * x + y * y + z * z + w * w).sqrt();
    if len > 0.0 {
        ufbx::Quat {
            x: x / len,
            y: y / len,
            z: z / len,
            w: w / len,
        }
    } else {
        a
    }
}

/// 把一条动画的全部通道解码成 `f32` 关键帧。
///
/// 逐条按 `data_type()` 分派是必须的：`translation` / `scale` 允许 `F32`
/// 与归一化的整型，`rotation` 允许 `F32` / `I8` / `U8` / `I16` / `U16`
/// （后四种是**归一化**的，范围 `[-1, 1]` 或 `[0, 1]`，要按规范换算）。
/// 走 `read_outputs()` 拿到的是按分量类型分派的 enum，逐臂 `into_f32()`。
fn sample_channels<'a, 's, F>(anim: &gltf::Animation<'a>, get: &F) -> Vec<Channel>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    let mut out: Vec<Channel> = Vec::new();
    for ch in anim.channels() {
        let sampler = ch.sampler();
        let interp = sampler.interpolation();
        let Some(times) = gltf::accessor::Iter::<f32>::new(sampler.input(), get.clone()) else {
            continue;
        };
        let times: Vec<f32> = times.collect();
        if times.is_empty() {
            continue;
        }
        // `CubicSpline` 的 output 是「每关键帧 3 个值」（in-tangent / value /
        // out-tangent），这里只取中间那个 value（切线信息丢弃，按线性重采样）。
        let stride = if interp == Interpolation::CubicSpline {
            3
        } else {
            1
        };
        let pick = |i: usize| i * stride + (stride - 1) / 2;

        let values = match ch.reader(get.clone()).read_outputs() {
            Some(gltf::animation::util::ReadOutputs::Translations(it)) => {
                ChannelValues::Vec3(it.collect())
            }
            Some(gltf::animation::util::ReadOutputs::Scales(it)) => ChannelValues::Vec3(it.collect()),
            Some(gltf::animation::util::ReadOutputs::Rotations(r)) => {
                ChannelValues::Quat(r.into_f32().collect())
            }
            // 逐帧表情权重：mdlc 不做（见 [`GltfNotes::morph_weight_channels`]）。
            Some(gltf::animation::util::ReadOutputs::MorphTargetWeights(_)) | None => continue,
        };

        // `CubicSpline` 的三倍长度在这里收回来。
        let values = if stride == 3 {
            match values {
                ChannelValues::Vec3(v) => {
                    ChannelValues::Vec3((0..times.len()).map(|i| v[pick(i)]).collect())
                }
                ChannelValues::Quat(v) => {
                    ChannelValues::Quat((0..times.len()).map(|i| v[pick(i)]).collect())
                }
            }
        } else {
            values
        };

        out.push(Channel {
            node: Some(ch.target().node().index()),
            times,
            // `CubicSpline` 按线性处理（上面已把切线丢掉）。
            interp: if interp == Interpolation::CubicSpline {
                Interpolation::Linear
            } else {
                interp
            },
            property: ch.target().property(),
            values,
        });
    }
    out
}

/// 全部通道的时间范围（秒）。空表给 `(0, 0)`。
fn time_range(channels: &[Channel]) -> (f64, f64) {
    let mut begin = f64::INFINITY;
    let mut end = f64::NEG_INFINITY;
    for ch in channels {
        if let (Some(&t0), Some(&t1)) = (ch.times.first(), ch.times.last()) {
            begin = begin.min(f64::from(t0));
            end = end.max(f64::from(t1));
        }
    }
    if begin.is_finite() && end.is_finite() {
        (begin, end)
    } else {
        (0.0, 0.0)
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -----------------------------------------------------------------
    // 夹具
    // -----------------------------------------------------------------

    /// 每个用例用**唯一**的临时目录（测试并行跑，共用目录会互相删文件）。
    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mdlc-gltf-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// 写一个**内嵌 base64 data URI** 的最小 `.gltf`。
    ///
    /// `body` 是 JSON 里 `"buffers"` 之后的内容（`bufferViews` / `accessors` /
    /// `meshes` / `nodes` / 可选 `animations` …），由调用方按需拼。
    ///
    /// ⚠️ 走 data URI 而不是 GLB：那样能顺带把 [`decode_data_uri`] 这条
    /// 「关掉 `import` feature 之后必须自己解」的路径也覆盖掉
    /// （`docs/gltf-support.md` §3.4 —— 不自己解会**静默读出零顶点**）。
    fn write_gltf(dir: &Path, name: &str, bin: &[u8], body: &str) -> PathBuf {
        let json = format!(
            "{{\n  \"asset\": {{\"version\": \"2.0\"}},\n  \
             \"buffers\": [{{\"byteLength\": {}, \
             \"uri\": \"data:application/octet-stream;base64,{}\"}}],\n{}\n}}\n",
            bin.len(),
            base64::encode(bin),
            body
        );
        let p = dir.join(name);
        std::fs::write(&p, json).unwrap();
        p
    }

    /// 一个三角形的几何字节（102 B）：POSITION / NORMAL / TEXCOORD_0 / indices。
    ///
    /// 偏移刻意排成 `0 / 36 / 72 / 96`（全是 4 的倍数 —— glTF 要求 accessor
    /// 的 `byteOffset` 对齐到分量大小）。
    ///
    /// ⚠️ UV 取成**不对称**的（三个角的 V 各不相同），这样「V 有没有被翻过」
    /// 一眼可辨：FBX 路径**要**翻、glTF 路径**不能**翻（见 [`read_uv`] 与
    /// `docs/gltf-support.md` §5.1）。
    fn tri_bytes() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0])); // POSITION
        b.extend(f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0])); // NORMAL
        b.extend(f32s(&[0.0, 1.0, 1.0, 1.0, 0.0, 0.0])); // TEXCOORD_0
        b.extend(u16s(&[0, 1, 2])); // indices
        b
    }

    /// `tri_bytes()` 对应的 `bufferViews` + `accessors` + 材质 + 网格 + 节点。
    ///
    /// 节点名与网格名**故意不同**（`Body.001` vs `hat`），这样 `srcpart`
    /// 的两条匹配路径都能单独验证。
    const TRI_BODY: &str = r#"  "bufferViews": [
    {"buffer": 0, "byteOffset": 0, "byteLength": 36},
    {"buffer": 0, "byteOffset": 36, "byteLength": 36},
    {"buffer": 0, "byteOffset": 72, "byteLength": 24},
    {"buffer": 0, "byteOffset": 96, "byteLength": 6}
  ],
  "accessors": [
    {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
    {"bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR"},
    {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2"},
    {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}
  ],
  "materials": [{"name": "face"}],
  "meshes": [{"name": "hat", "primitives": [
    {"attributes": {"POSITION": 0, "NORMAL": 3, "TEXCOORD_0": 2}, "indices": 1, "material": 0}
  ]}],
  "nodes": [{"name": "Body.001", "mesh": 0}]"#;

    fn near3(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    fn near2(a: [f32; 2], b: [f32; 2]) -> bool {
        (0..2).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    /// 往 [`TRI_BODY`] 的 `accessors` 数组**末尾**追加声明（`decl` 可含多行）。
    ///
    /// ⚠️ 必须追加，不能插到开头 —— accessor 的下标就是 `attributes` 里引用的
    /// 编号，插到前面会把整张表挪位，于是 `POSITION` 指向别的东西、`TEXCOORD_0`
    /// 指向索引表……**而且不会报错**，只会静默读出垃圾（第一次写这个夹具时
    /// 就是这么栽的：报的是 `size_of` 不符，看起来像代码 bug）。
    ///
    /// 锚点取「`accessors` 数组的收尾 + 下一节的开头」而不是最后一条记录，
    /// 这样**可以连续追加多次**（锚点在每次追加后依然唯一）。
    fn add_accessor(body: &str, decl: &str) -> String {
        const ANCHOR: &str = "}\n  ],\n  \"materials\": [";
        let with = format!("}},\n    {decl}\n  ],\n  \"materials\": [");
        assert!(body.contains(ANCHOR), "夹具结构变了：找不到 accessors 数组的结尾");
        body.replacen(ANCHOR, &with, 1)
    }

    /// 往 [`TRI_BODY`] 的 `bufferViews` 数组**末尾**追加一条（可连续调用）。
    fn add_view(body: &str, offset: usize, len: usize) -> String {
        const ANCHOR: &str = "}\n  ],\n  \"accessors\": [";
        let with = format!(
            "}},\n    {{\"buffer\": 0, \"byteOffset\": {offset}, \"byteLength\": {len}}}\n  ],\n  \"accessors\": ["
        );
        assert!(body.contains(ANCHOR), "夹具结构变了：找不到 bufferViews 数组的结尾");
        body.replacen(ANCHOR, &with, 1)
    }

    /// 把形变目标挂到唯一的那个图元上。
    fn add_target(body: &str, accessor: usize) -> String {
        const OLD: &str = "\"indices\": 1, \"material\": 0}";
        assert!(body.contains(OLD), "夹具结构变了：找不到图元声明");
        body.replace(
            OLD,
            &format!("\"indices\": 1, \"material\": 0, \"targets\": [{{\"POSITION\": {accessor}}}]}}"),
        )
    }

    // -----------------------------------------------------------------
    // 纯函数
    // -----------------------------------------------------------------

    /// `data:` URI 要自己解（关掉 `import` feature 之后 crate 不管了）。
    #[test]
    fn decode_data_uri_decodes_base64() {
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::encode([1u8, 2, 3, 4])
        );
        assert_eq!(decode_data_uri(&uri, "t").unwrap(), vec![1, 2, 3, 4]);
    }

    /// 非 base64 的 data URI **报错不猜** —— 猜错会让模型读出零顶点却不报错。
    #[test]
    fn decode_data_uri_rejects_percent_encoding() {
        let err = decode_data_uri("data:application/octet-stream,%01%02", "t").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("base64"), "报错应说明只支持 base64 形态：{msg}");
    }

    /// `TriangleStrip` 的**奇数号**三角形要翻转绕序（否则法线朝里）。
    #[test]
    fn triangles_of_flips_odd_strip_winding() {
        let tris = triangles_of(Mode::TriangleStrip, &[0, 1, 2, 3]);
        assert_eq!(tris, vec![[0, 1, 2], [2, 1, 3]]);
    }

    /// `TriangleFan` 以第 0 个顶点为扇心。
    #[test]
    fn triangles_of_expands_a_fan() {
        let tris = triangles_of(Mode::TriangleFan, &[0, 1, 2, 3]);
        assert_eq!(tris, vec![[0, 1, 2], [0, 2, 3]]);
    }

    /// 非三角形图元（点 / 线）静默丢弃 —— 它们进不了 SMD。
    #[test]
    fn triangles_of_drops_non_triangle_modes() {
        assert!(triangles_of(Mode::Points, &[0, 1, 2]).is_empty());
        assert!(triangles_of(Mode::Lines, &[0, 1]).is_empty());
    }

    /// `nlerp` 必须走**短弧**：`q` 与 `−q` 是同一个旋转，直接插值会绕远路。
    #[test]
    fn nlerp_takes_the_short_path_across_the_sign() {
        // 绕 Z 轴 0° → 180°。`-b` 与 `b` 表示同一个旋转。
        let a = ufbx::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        let b = ufbx::Quat {
            x: 0.0,
            y: 0.0,
            z: 1.0,
            w: 0.0,
        };
        let neg_b = ufbx::Quat {
            x: 0.0,
            y: 0.0,
            z: -1.0,
            w: 0.0,
        };
        let p = nlerp(a, b, 0.5);
        let q = nlerp(a, neg_b, 0.5);
        // 两者应给出**同一个**旋转（同一个 z 分量的绝对值），而不是相反方向。
        assert!(
            (p.z.abs() - q.z.abs()).abs() < 1e-9,
            "同一旋转的两种四元数表示应给出同一条路径：{p:?} vs {q:?}"
        );
        assert!(p.z > 0.5, "90° 的四元数 z 分量应显著非零：{p:?}");
    }

    /// `World::from_trs` 用 glTF 的**列主序**约定，且 `T·R·S`。
    #[test]
    fn world_from_trs_uses_the_gltf_column_convention() {
        // 绕 Z 轴 +90°：x 轴应转到 y 轴。
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let w = World::from_trs([1.0, 2.0, 3.0], [0.0, 0.0, s, s], [1.0, 1.0, 1.0]);
        // 第 0 列 = 变换后的 x 轴 ≈ (0, 1, 0)。
        assert!(
            (w.m[0][0]).abs() < 1e-6 && (w.m[0][1] - 1.0).abs() < 1e-6,
            "第 0 列应是 x 轴转 90° 后的像：{:?}",
            w.m[0]
        );
        assert_eq!(w.t, [1.0, 2.0, 3.0]);
    }

    /// ⭐ **层序陷阱**：glTF 的 `scene.nodes` 是**层序**（子可以排在父前面），
    /// 所以 `accumulate` 不能像 FBX 那样顺数组累乘，必须迭代到不动点。
    ///
    /// 夹具刻意把**子放在父前面**（`[tip, mid, root]`），正是 `rig.glb` 的
    /// 形状（`Tip, Mid, Root, Body, Rig`）。
    #[test]
    fn accumulate_computes_children_before_parents() {
        let local = |y: f64| World {
            m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            t: [0.0, y, 0.0],
        };
        // 下标 0 = tip（父是 1），1 = mid（父是 2），2 = root（无父）。
        let locals = vec![local(5.0), local(10.0), local(0.0)];
        let parent = vec![Some(1), Some(2), None];
        let world = accumulate(&locals, &parent);
        assert!((world[2].t[1] - 0.0).abs() < 1e-9);
        assert!(
            (world[1].t[1] - 10.0).abs() < 1e-9,
            "mid 的世界 y 应是 10（父 root 在原点）：{:?}",
            world[1].t
        );
        assert!(
            (world[0].t[1] - 15.0).abs() < 1e-9,
            "tip 的世界 y 应是 10+5=15（层序下必须等父先算出来）：{:?}",
            world[0].t
        );
    }

    /// 环（规范不允许）不能让 `accumulate` 死循环。
    #[test]
    fn accumulate_breaks_a_cycle_instead_of_hanging() {
        let locals = vec![World::identity(), World::identity()];
        let parent = vec![Some(1), Some(0)];
        let world = accumulate(&locals, &parent);
        assert_eq!(world.len(), 2, "环上的节点退回单位阵，但不能卡住");
    }

    /// ⭐ `JOINTS_0` 是 **`skin.joints` 数组的下标**，不是节点下标。
    ///
    /// 夹具用 Blender 的真实形态（`skin.joints = [2, 1, 0]`，实测
    /// `docs/_probe/gltf_joints.js`）：`JOINTS_0 = 0` 指的是
    /// **节点 2**（`skin.joints[0]`），不是节点 0。
    #[test]
    fn weights_map_skin_joint_indices_through_the_node_table() {
        let joint_to_node = [2usize, 1, 0];
        let idx_of: HashMap<usize, i32> = [(0usize, 0i32), (1, 1), (2, 2)].into_iter().collect();

        let j = [0u16, 2, 3, 3];
        let w = [1.0f32, 0.0, 0.0, 0.0];
        let links = weights_of(Some(&j), Some(&w), &joint_to_node, &idx_of);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].bone, 2,
            "JOINTS_0 = 0 应映射到节点 2（skin.joints[0]），而不是节点 0"
        );

        // 权重为 0 的分量、以及越界的 joint 下标都要静默跳过。
        let j = [0u16, 1, 9, 9];
        let w = [0.25f32, 0.75, 0.0, 0.0];
        let links = weights_of(Some(&j), Some(&w), &joint_to_node, &idx_of);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].bone, 1, "权重降序：0.75 在前");
        assert_eq!(links[1].bone, 2);

        // 没有 JOINTS_0 / WEIGHTS_0（静态网格）时给空表，由调用方补兜底。
        assert!(weights_of(None, Some(&w), &joint_to_node, &idx_of).is_empty());
    }

    /// `Step` 保持上一个关键帧的值（不插值）。
    #[test]
    fn channel_step_holds_the_previous_key() {
        let ch = Channel {
            node: Some(0),
            times: vec![0.0, 1.0, 2.0],
            interp: Interpolation::Step,
            property: gltf::animation::Property::Translation,
            values: ChannelValues::Vec3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]),
        };
        let mut slot = LocalTrs {
            translation: ufbx::Vec3 {
                x: 9.0,
                y: 9.0,
                z: 9.0,
            },
            rotation: ufbx::Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            scale: ufbx::Vec3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        };
        ch.apply(1.5, &mut slot);
        assert!(
            (slot.translation.x - 1.0).abs() < 1e-6,
            "Step 应停在 t=1 那一帧：{:?}",
            slot.translation
        );
    }

    /// `Linear` 在相邻关键帧之间插值。
    #[test]
    fn channel_linear_interpolates_between_keys() {
        let ch = Channel {
            node: Some(0),
            times: vec![0.0, 2.0],
            interp: Interpolation::Linear,
            property: gltf::animation::Property::Translation,
            values: ChannelValues::Vec3(vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0]]),
        };
        let mut slot = LocalTrs {
            translation: ufbx::Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            rotation: ufbx::Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            scale: ufbx::Vec3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        };
        ch.apply(1.0, &mut slot);
        assert!(
            (slot.translation.x - 2.0).abs() < 1e-6,
            "t=1 是 0→4 的中点：{:?}",
            slot.translation
        );
        // 超出末端要夹住，不能越界读。
        ch.apply(99.0, &mut slot);
        assert!((slot.translation.x - 4.0).abs() < 1e-6);
        ch.apply(-99.0, &mut slot);
        assert!((slot.translation.x - 0.0).abs() < 1e-6);
    }

    /// `time_range` 取全部通道的并集。
    #[test]
    fn time_range_spans_all_channels() {
        let mk = |t0: f32, t1: f32| Channel {
            node: Some(0),
            times: vec![t0, t1],
            interp: Interpolation::Linear,
            property: gltf::animation::Property::Translation,
            values: ChannelValues::Vec3(vec![[0.0; 3], [0.0; 3]]),
        };
        let (b, e) = time_range(&[mk(0.0, 1.0), mk(0.5, 2.0)]);
        assert!((b - 0.0).abs() < 1e-9 && (e - 2.0).abs() < 1e-9);
        assert_eq!(time_range(&[]), (0.0, 0.0), "空表不能给出 ±inf");
    }

    // -----------------------------------------------------------------
    // 端到端（`read`）
    // -----------------------------------------------------------------

    /// 最小三角形：几何 / 节点表 / 材质名都要对。
    #[test]
    fn read_builds_geometry_from_an_inline_gltf() {
        let d = tmpdir("tri");
        let p = write_gltf(&d, "tri.gltf", &tri_bytes(), TRI_BODY);
        let g = read(&p, "tri.gltf", &GltfOpts::default()).expect("应能读出三角形");

        assert_eq!(g.smd.nodes.len(), 1);
        assert_eq!(g.smd.nodes[0].name, "Body.001");
        assert_eq!(g.smd.nodes[0].parent, -1, "根节点的 parent 是 -1");
        assert_eq!(g.smd.frames.len(), 1, "几何路径只产一帧参考姿态");

        assert_eq!(g.smd.triangles.len(), 1);
        let t = &g.smd.triangles[0];
        assert_eq!(t.material, "face", "材质名要原样进表");
        assert!(near3(t.vertices[0].position, [0.0, 0.0, 0.0]));
        assert!(near3(t.vertices[1].position, [1.0, 0.0, 0.0]));
        assert!(near3(t.vertices[2].position, [0.0, 1.0, 0.0]));
        assert!(near3(t.vertices[0].normal, [0.0, 0.0, 1.0]));

        // 没有 `skin` 的静态网格：顶点绑到它所在的那个网格节点。
        assert_eq!(t.vertices[0].links.len(), 1);
        assert_eq!(t.vertices[0].links[0].bone, 0);
        assert_eq!(t.vertices[0].parent_bone, 0);

        assert!(g.merged_meshes.contains(&"hat".to_string()));
        assert!(
            g.untextured_meshes.is_empty(),
            "写了材质就不该进「无材质」清单"
        );
    }

    /// ⭐ **glTF 路径不翻 V** —— 与 FBX 相反。
    ///
    /// 这是最容易被「照抄 FBX 那边」带错的一处：FBX 的 `geometry_normal` /
    /// UV 路径要 `1.0 - v`，而 SMD 解析器**自己**翻过一次；glTF 不经过
    /// SMD 解析器，所以**必须不翻**（`docs/gltf-support.md` §5.1）。
    #[test]
    fn read_uv_is_not_flipped() {
        let d = tmpdir("uv");
        let p = write_gltf(&d, "tri.gltf", &tri_bytes(), TRI_BODY);
        let g = read(&p, "tri.gltf", &GltfOpts::default()).unwrap();
        let v = &g.smd.triangles[0].vertices;
        assert!(
            near2(v[0].uv, [0.0, 1.0]),
            "顶点 0 的 UV 应原样是 (0,1)；翻过就成 (0,0) 了：{:?}",
            v[0].uv
        );
        assert!(near2(v[1].uv, [1.0, 1.0]));
        assert!(near2(v[2].uv, [0.0, 0.0]));
    }

    /// `srcpart` 两条匹配路径都要通：**节点名**与**网格名**。
    ///
    /// 夹具的节点叫 `Body.001`、网格叫 `hat` —— 用户看到的「部件」多半是
    /// 网格名，而节点名可能是 Blender 自动加的后缀。
    #[test]
    fn srcpart_matches_either_the_node_name_or_the_mesh_name() {
        let d = tmpdir("part");
        let p = write_gltf(&d, "tri.gltf", &tri_bytes(), TRI_BODY);

        for part in ["hat", "Body.001"] {
            let opts = GltfOpts {
                parts: vec![part.to_string()],
                ..Default::default()
            };
            let g = read(&p, "tri.gltf", &opts)
                .unwrap_or_else(|e| panic!("srcpart {part:?} 应能选中：{e}"));
            assert_eq!(g.smd.triangles.len(), 1, "srcpart {part:?}");
        }

        let opts = GltfOpts {
            parts: vec!["nope".to_string()],
            ..Default::default()
        };
        let err = read(&p, "tri.gltf", &opts).unwrap_err();
        assert!(
            err.to_string().contains("没有任何三角形"),
            "选不中时要报错而不是静默出空模型：{err}"
        );
    }

    /// 没写材质的网格要落到 `debug/debugempty` 并进「无材质」清单。
    #[test]
    fn read_falls_back_to_the_empty_material() {
        let d = tmpdir("nomat");
        let body = TRI_BODY.replace("\"material\": 0", "\"material\": -1");
        // `material: -1` 不是合法的 glTF 索引，所以改成整个删掉 `material`。
        let body = body.replace(", \"material\": -1", "");
        let p = write_gltf(&d, "tri.gltf", &tri_bytes(), &body);
        let g = read(&p, "tri.gltf", &GltfOpts::default()).unwrap();
        assert_eq!(g.smd.triangles[0].material, FALLBACK_MATERIAL);
        assert_eq!(g.untextured_meshes, vec!["hat".to_string()]);
    }

    /// `srcmaterial` 显式兜底名要盖过 `debug/debugempty`。
    #[test]
    fn srcmaterial_overrides_the_fallback_name() {
        let d = tmpdir("mat");
        let body = TRI_BODY.replace(", \"material\": 0", "");
        let p = write_gltf(&d, "tri.gltf", &tri_bytes(), &body);
        let opts = GltfOpts {
            material: Some("my/fallback".to_string()),
            ..Default::default()
        };
        let g = read(&p, "tri.gltf", &opts).unwrap();
        assert_eq!(g.smd.triangles[0].material, "my/fallback");
    }

    /// ⭐ morph target 要变成 shape key，名字走 `extras.targetNames`
    /// （glTF 2.0 没有这个字段，是**事实标准**）。
    #[test]
    fn read_registers_morph_targets_as_shape_keys() {
        let d = tmpdir("morph");
        let mut bin = tri_bytes();
        // 形变偏移：三个顶点各 +1 在 z 上。
        bin.extend(f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
        let body = add_target(
            &add_view(
                &add_accessor(
                    TRI_BODY,
                    "{\"bufferView\": 4, \"componentType\": 5126, \"count\": 3, \"type\": \"VEC3\"}",
                ),
                102,
                36,
            ),
            4,
        );
        // `extras` 挂在网格上（不是图元上）。
        let body = body.replace(
            "\"meshes\": [{\"name\": \"hat\"",
            "\"meshes\": [{\"name\": \"hat\", \"extras\": {\"targetNames\": [\"wide\"]}",
        );
        let p = write_gltf(&d, "morph.gltf", &bin, &body);
        let g = read(&p, "morph.gltf", &GltfOpts::default()).unwrap();

        assert_eq!(g.shape_keys.len(), 1, "应有一个 shape key：{:?}", g.shape_keys);
        let k = &g.shape_keys[0];
        assert_eq!(k.name, "wide", "名字应来自 extras.targetNames");
        assert_eq!(k.vertex_index, vec![0, 1, 2]);
        for (i, off) in k.position_offsets.iter().enumerate() {
            assert!(near3(*off, [0.0, 0.0, 1.0]), "第 {i} 个偏移：{off:?}");
        }
        // 法线偏移官方在 FBX 路径就丢弃；glTF 这边照读但下游不用。
        // 夹具没给形变法线 ⟹ 逐顶点补零（长度与 `vertex_index` 对齐）。
        assert_eq!(k.normal_offsets.len(), k.vertex_index.len());
        for n in &k.normal_offsets {
            assert!(near3(*n, [0.0, 0.0, 0.0]), "没给形变法线时应是零：{n:?}");
        }
    }

    /// 形变偏移**全零**的目标要跳过（`0.001²` 阈值，与 `simplify.cpp:2537` 同口径）。
    #[test]
    fn shape_keys_skip_all_zero_offsets() {
        let d = tmpdir("morphzero");
        let mut bin = tri_bytes();
        bin.extend(f32s(&[0.0; 9]));
        let body = add_target(
            &add_view(
                &add_accessor(
                    TRI_BODY,
                    "{\"bufferView\": 4, \"componentType\": 5126, \"count\": 3, \"type\": \"VEC3\"}",
                ),
                102,
                36,
            ),
            4,
        );
        let p = write_gltf(&d, "morphzero.gltf", &bin, &body);
        let g = read(&p, "morphzero.gltf", &GltfOpts::default()).unwrap();
        assert!(
            g.shape_keys.is_empty(),
            "全零偏移的 target 不该产出 flex：{:?}",
            g.shape_keys
        );
    }

    /// ⭐ **无 `bufferView` 的 accessor**（规范 = 全零）要走得通 ——
    /// 这是 `from_slice_without_validation` 存在的**唯一理由**
    /// （`docs/gltf-support.md` §3.2：Blender 对「只改位置不改法线」的 morph
    /// target 正是这么写的，用带校验的路径会直接编不出来）。
    ///
    /// 夹具照 Blender 的真实形态做：**位置偏移有数据、法线偏移无 `bufferView`**。
    #[test]
    fn accessors_without_a_buffer_view_are_treated_as_zero() {
        let d = tmpdir("noview");
        let mut bin = tri_bytes();
        bin.extend(f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
        // accessor 4 = 位置偏移（有 bufferView）；5 = 法线偏移（**没有**）。
        let body = add_target(
            &add_view(
                &add_accessor(
                    TRI_BODY,
                    "{\"bufferView\": 4, \"componentType\": 5126, \"count\": 3, \"type\": \"VEC3\"},\n    \
                     {\"componentType\": 5126, \"count\": 3, \"type\": \"VEC3\"}",
                ),
                102,
                36,
            ),
            4,
        );
        let body = body.replace(
            "\"targets\": [{\"POSITION\": 4}]",
            "\"targets\": [{\"POSITION\": 4, \"NORMAL\": 5}]",
        );
        let p = write_gltf(&d, "noview.gltf", &bin, &body);
        let g = read(&p, "noview.gltf", &GltfOpts::default())
            .expect("无 bufferView 的 accessor 必须放行（规范 = 全零）");

        // 位置偏移照常读出来。
        assert_eq!(g.shape_keys.len(), 1);
        assert_eq!(g.shape_keys[0].vertex_index, vec![0, 1, 2]);
        assert!(near3(g.shape_keys[0].position_offsets[0], [0.0, 0.0, 1.0]));
        // 法线偏移按全零读出来（**不是**报错、也不是跳过整条 target）。
        assert_eq!(g.shape_keys[0].normal_offsets.len(), 3);
        for n in &g.shape_keys[0].normal_offsets {
            assert!(near3(*n, [0.0, 0.0, 0.0]), "应按全零处理：{n:?}");
        }
    }

    /// 压缩扩展要**直接报错** —— crate 解不了，静默读出来的顶点数是错的。
    #[test]
    fn compressed_mesh_extensions_are_rejected() {
        for ext in UNSUPPORTED_EXTENSIONS {
            let d = tmpdir("ext");
            let body = format!("{TRI_BODY},\n  \"extensionsRequired\": [\"{ext}\"]");
            let p = write_gltf(&d, "ext.gltf", &tri_bytes(), &body);
            let err = read(&p, "ext.gltf", &GltfOpts::default()).unwrap_err();
            assert!(
                err.to_string().contains(ext),
                "报错文案要点名是哪个扩展：{err}"
            );
        }
    }

    /// 声明长度超过实际字节数要报错（`from_slice_without_validation` 绕过了
    /// crate 的全部校验，这三项得自己补 —— `docs/gltf-support.md` §8.1）。
    #[test]
    fn declared_buffer_length_is_checked() {
        let d = tmpdir("shortbuf");
        let json = format!(
            "{{\n  \"asset\": {{\"version\": \"2.0\"}},\n  \
             \"buffers\": [{{\"byteLength\": 9999, \
             \"uri\": \"data:application/octet-stream;base64,{}\"}}],\n{}\n}}\n",
            base64::encode([0u8; 8]),
            TRI_BODY
        );
        let p = d.join("short.gltf");
        std::fs::write(&p, json).unwrap();
        let err = read(&p, "short.gltf", &GltfOpts::default()).unwrap_err();
        assert!(
            err.to_string().contains("9999"),
            "报错应说明声明了多少字节：{err}"
        );
    }

    // -----------------------------------------------------------------
    // 端到端（`read_frames`）
    // -----------------------------------------------------------------

    /// 一个带旋转动画的夹具：绕 Z 轴 0° → 90°，`LINEAR`，时间轴 `[0, 1]` 秒。
    ///
    /// 几何字节尾部接上动画的两个 accessor（时间轴 8 B + 四元数 32 B）。
    fn anim_fixture(dir: &Path) -> PathBuf {
        let mut bin = tri_bytes();
        bin.extend(f32s(&[0.0, 1.0]));
        let s = std::f32::consts::FRAC_1_SQRT_2;
        bin.extend(f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s, s]));
        // 几何 102 B ⟹ 时间轴在 102，四元数在 110。
        let body = add_accessor(
            TRI_BODY,
            "{\"bufferView\": 4, \"componentType\": 5126, \"count\": 2, \"type\": \"SCALAR\"},\n    \
             {\"bufferView\": 5, \"componentType\": 5126, \"count\": 2, \"type\": \"VEC4\"}",
        );
        let body = add_view(&add_view(&body, 102, 8), 110, 32);
        let body = body.replace(
            "\"nodes\": [{\"name\": \"Body.001\", \"mesh\": 0}]",
            "\"nodes\": [{\"name\": \"Body.001\", \"mesh\": 0}],\n  \
             \"animations\": [{\"name\": \"SkeletonAction\", \"channels\": [\
             {\"sampler\": 0, \"target\": {\"node\": 0, \"path\": \"rotation\"}}], \
             \"samplers\": [{\"input\": 4, \"output\": 5, \"interpolation\": \"LINEAR\"}]}]",
        );
        write_gltf(dir, "anim.gltf", &bin, &body)
    }

    /// 动画按**秒轴**重采样到 `fps`，帧数 = `round(时长 × fps) + 1`。
    #[test]
    fn read_frames_samples_the_animation_axis() {
        let d = tmpdir("anim");
        let p = anim_fixture(&d);
        let smd = read_frames(&p, "anim.gltf", None, 30.0, &GltfOpts::default()).unwrap();

        assert_eq!(smd.nodes.len(), 1);
        assert_eq!(
            smd.frames.len(),
            31,
            "时间轴 0..1 秒 @30fps ⟹ round(1.0 × 30) + 1 = 31 帧"
        );
        assert_eq!(smd.frames[0].time, 0);
        assert_eq!(smd.frames[30].time, 30);

        // 欧拉角的第三位是 yaw（绕 Z）。这里只断言**大小** ——
        // 符号约定由 `quaternion_angles` 决定，不是本测试要守的东西。
        let yaw = |i: usize| smd.frames[i].poses[0].rotation[2].abs();
        assert!(yaw(0) < 1e-4, "第 0 帧是单位四元数：{}", yaw(0));
        assert!(
            (yaw(15) - std::f32::consts::FRAC_PI_4).abs() < 1e-3,
            "第 15 帧应是 45°：{}",
            yaw(15)
        );
        assert!(
            (yaw(30) - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
            "第 30 帧应是 90°：{}",
            yaw(30)
        );
    }

    /// 文件里没有动画**不是错误** —— 返回一帧参考姿态（与 FBX 同一条口径）。
    #[test]
    fn read_frames_returns_the_reference_frame_without_animation() {
        let d = tmpdir("noanim");
        let p = write_gltf(&d, "tri.gltf", &tri_bytes(), TRI_BODY);
        let smd = read_frames(&p, "tri.gltf", None, 30.0, &GltfOpts::default()).unwrap();
        assert_eq!(smd.frames.len(), 1);
        assert_eq!(smd.frames[0].poses.len(), 1);
    }

    /// `srcstack` 按名选动画；名字不存在要**报错并列出可选项**。
    #[test]
    fn srcstack_selects_by_name_and_lists_the_alternatives() {
        let d = tmpdir("stack");
        let p = anim_fixture(&d);

        let smd = read_frames(
            &p,
            "anim.gltf",
            Some("SkeletonAction"),
            30.0,
            &GltfOpts::default(),
        )
        .unwrap();
        assert_eq!(smd.frames.len(), 31);

        let err = read_frames(&p, "anim.gltf", Some("nope"), 30.0, &GltfOpts::default())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("SkeletonAction"),
            "报错要列出文件里真正有的动画名：{err}"
        );
    }

    /// 动画与几何的**骨骼表必须一致**（同一套收骨），否则帧对不上几何。
    #[test]
    fn read_frames_and_read_agree_on_the_bone_table() {
        let d = tmpdir("agree");
        let p = anim_fixture(&d);
        let geo = read(&p, "anim.gltf", &GltfOpts::default()).unwrap();
        let smd = read_frames(&p, "anim.gltf", None, 30.0, &GltfOpts::default()).unwrap();
        let a: Vec<_> = geo.smd.nodes.iter().map(|n| n.name.clone()).collect();
        let b: Vec<_> = smd.nodes.iter().map(|n| n.name.clone()).collect();
        assert_eq!(a, b, "两条路径的骨骼表必须逐项相同");
        for f in &smd.frames {
            assert_eq!(f.poses.len(), smd.nodes.len());
        }
    }

    /// 提示文案是**纯函数**（`diagln!` 走 stdout，测试里抓不到）。
    #[test]
    fn notes_render_every_kind_of_hint() {
        let n = GltfNotes {
            zero_accessors: 2,
            cubic_spline_channels: 3,
            morph_weight_channels: 4,
        };
        assert!(!n.is_empty());
        let lines = n.lines("a.gltf");
        assert_eq!(lines.len(), 3, "三种提示各一条：{lines:?}");
        assert!(lines[0].contains("2 个 accessor"), "{}", lines[0]);
        assert!(lines[1].contains("CubicSpline"), "{}", lines[1]);
        assert!(lines[2].contains("MorphTargetWeights"), "{}", lines[2]);

        // ⭐ 每条提示都要能**单独**出现（漏一条就等于静默忽略一类数据）。
        for one in [
            GltfNotes {
                zero_accessors: 1,
                ..Default::default()
            },
            GltfNotes {
                cubic_spline_channels: 1,
                ..Default::default()
            },
            GltfNotes {
                morph_weight_channels: 1,
                ..Default::default()
            },
        ] {
            assert!(!one.is_empty());
            assert_eq!(one.lines("a.gltf").len(), 1);
        }

        assert!(GltfNotes::default().is_empty());
        assert!(GltfNotes::default().lines("a.gltf").is_empty());
    }
}
