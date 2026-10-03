//! FBX 源文件读取（`ufbx`）—— 转成中立的 [`crate::smd::Smd`]。
//!
//! # 为什么是「转成 Smd」而不是「实现一个 trait」
//!
//! `compile.rs` 有 13583 行，它只认 [`crate::smd::Smd`]。让泛型或 `dyn` 穿过
//! 整条管线会把编译期与运行期成本都抬起来，而格式来自**运行期**的路径字符串
//! （`resolve_smd_path` 的返回值），没有任何可以「延迟到单态化」的东西。
//!
//! 读一个源文件在整次编译里只发生 4 次左右，所以这里是**单点分派**：
//!
//! ```text
//! path ──► kind_of(path) ──► read_source(path) ──► Smd ──► 原有管线（零改动）
//! ```
//!
//! # 与官方的对齐（全部实测，见 `docs/fbx-support.md` §1）
//!
//! | 行为 | 官方 | 本模块 |
//! |---|---|---|
//! | 骨骼集合 | 被蒙皮权重引用的骨骼 ∪ 其全部祖先；无蒙皮的网格节点自身算一根 | 同 |
//! | 网格节点 | 丢（除非它自己就是网格宿主） | 同 |
//! | ufbx 合成根 | 不存在于 FBX 里 | 丢（`is_root`） |
//! | 材质名 | 原样进 texture 表 | 同 |
//! | 无材质的网格 | 合成 `debug/debugempty` | 同 |
//! | 多块网格 | 合并进同一个 model | 同 |
//! | 单位 / 轴向 | **不干预**（原样搬运局部变换） | 同 |
//! | 动画栈 | **恒取第一条** | 同（`srcstack` 可选别的） |
//! | 重采样率 | 固定 30 fps | 同（`srcfps` 可改） |
//! | shape key | 自动注册成 flex（帧号 = 文件顺序 + 1） | 同（见 [`FbxShapeKey`]） |
//!
//! # 两条容易踩的坑（都已在实测里确认）
//!
//! 1. **UV 的 V 轴要翻一次**（`1.0 - v`）。SMD 解析器里有这一步，
//!    而 FBX 路径**不经过** SMD 解析 —— 必须在这里补上，否则贴图上下颠倒。
//! 2. **顶点位置/法线/UV 按「角」取，蒙皮权重按「顶点」取**。
//!    `mesh.vertex_position[corner]` 与 `mesh.vertex_indices[corner]` 是
//!    两套下标，混用会静默错位（原型里踩过一次）。

use std::collections::HashMap;
use std::path::Path;

use crate::smd::{Smd, SmdBoneLink, SmdFrame, SmdNode, SmdPose, SmdTriangle, SmdVertex};

/// FBX 读取错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbxError {
    pub message: String,
}

impl std::fmt::Display for FbxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FbxError {}

fn ferr(message: impl Into<String>) -> FbxError {
    FbxError {
        message: message.into(),
    }
}

/// 官方在没有材质的网格上合成的材质名（实测，`docs/fbx-support.md` §1.9）。
pub const FALLBACK_MATERIAL: &str = "debug/debugempty";

/// 官方的固定重采样率（`docs/fbx-support.md` §1.5：24 / 30 / 60 fps 的源
/// 全部被归一化成 30 fps）。
pub const DEFAULT_FPS: f32 = 30.0;

/// `srcaxis` 指定的目标上轴。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForcedAxis {
    /// 文件按 **Y-up** 写（FBX 的常见默认），转换到 Source 的 Z-up。
    Y,
    /// 文件按 **Z-up** 写（= Source 的约定），不做转换。
    Z,
}

impl ForcedAxis {
    /// 解析 `srcaxis` 的参数。
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "y" | "yup" | "y-up" => Some(ForcedAxis::Y),
            "z" | "zup" | "z-up" => Some(ForcedAxis::Z),
            _ => None,
        }
    }

    /// 该上轴 → Z-up 的旋转矩阵（列主序命名，`m03/m13/m23` 是平移）。
    ///
    /// Y-up → Z-up 用 `(x, y, z) → (x, −z, y)`（绕 X 轴 +90°）；Z-up 是恒等。
    fn rotation(self) -> ufbx::Matrix {
        match self {
            ForcedAxis::Z => ufbx::Matrix {
                m00: 1.0,
                m10: 0.0,
                m20: 0.0,
                m01: 0.0,
                m11: 1.0,
                m21: 0.0,
                m02: 0.0,
                m12: 0.0,
                m22: 1.0,
                m03: 0.0,
                m13: 0.0,
                m23: 0.0,
            },
            ForcedAxis::Y => ufbx::Matrix {
                m00: 1.0,
                m10: 0.0,
                m20: 0.0,
                m01: 0.0,
                m11: 0.0,
                m21: 1.0,
                m02: 0.0,
                m12: -1.0,
                m22: 0.0,
                m03: 0.0,
                m13: 0.0,
                m23: 0.0,
            },
        }
    }
}

/// 读一个 FBX 源时的 mdlc 扩展选项（对应 `srcpart` / `srcmaterial` /
/// `srcscale` / `srcaxis`）。
///
/// **全部默认值与官方逐条一致** —— 不写这些语法时行为与官方相同。
#[derive(Debug, Clone, PartialEq)]
pub struct FbxOpts {
    /// `srcpart`：只取这些名字的网格；空 = 取全部并合并（官方）。
    pub parts: Vec<String>,
    /// `srcmaterial`：网格**没有**材质时用它；`None` = 官方的
    /// [`FALLBACK_MATERIAL`]。
    pub material: Option<String>,
    /// `srcscale`：顶点与骨骼位移的缩放；`1.0` = 官方。
    pub scale: f32,
    /// `srcaxis`：强制上轴；`None` = 官方（不干预）。
    pub axis: Option<ForcedAxis>,
}

impl Default for FbxOpts {
    fn default() -> Self {
        Self {
            parts: Vec::new(),
            material: None,
            scale: 1.0,
            axis: None,
        }
    }
}

impl FbxOpts {
    /// 是否有任何「会改变几何」的选项被设置（用于决定要不要做矩阵变换）。
    fn is_identity(&self) -> bool {
        self.axis.is_none() && (self.scale - 1.0).abs() < f32::EPSILON
    }

    /// 归一化到 Z-up 的完整变换 `M = S(s) · R(axis)`。
    fn matrix(&self) -> ufbx::Matrix {
        let r = self.axis.unwrap_or(ForcedAxis::Z).rotation();
        let s = f64::from(self.scale);
        ufbx::Matrix {
            m00: r.m00 * s,
            m10: r.m10 * s,
            m20: r.m20 * s,
            m01: r.m01 * s,
            m11: r.m11 * s,
            m21: r.m21 * s,
            m02: r.m02 * s,
            m12: r.m12 * s,
            m22: r.m22 * s,
            m03: r.m03 * s,
            m13: r.m13 * s,
            m23: r.m23 * s,
        }
    }

    /// 顶点位置用：`M · p`。
    fn point(&self, p: ufbx::Vec3) -> ufbx::Vec3 {
        if self.is_identity() {
            return p;
        }
        ufbx::transform_position(&self.matrix(), p)
    }

    /// 法线用：只有旋转，没有缩放与平移。
    fn direction(&self, v: ufbx::Vec3) -> ufbx::Vec3 {
        let Some(a) = self.axis else { return v };
        ufbx::transform_direction(&a.rotation(), v)
    }

    /// 骨骼的**局部**变换用：`M · L · M⁻¹`。
    ///
    /// 推导：世界变换 `W_i = L_parent · L_i`。要求 `W'_i = M · W_i`，于是
    /// `L'_i = M · L_i · M⁻¹`（`M` 是「均匀缩放 + 旋转」时这个式子成立，
    /// 非均匀缩放才会引入剪切）。旋转部分是 `R·Q·R⁻¹`，平移是 `M·T`。
    fn local(&self, t: ufbx::Transform) -> (ufbx::Vec3, ufbx::Quat) {
        if self.is_identity() {
            return (t.translation, t.rotation);
        }
        let m = self.matrix();
        let mi = ufbx::matrix_invert(&m);
        let l = ufbx::transform_to_matrix(&t);
        let out = ufbx::matrix_mul(&ufbx::matrix_mul(&m, &l), &mi);
        let r = ufbx::matrix_to_transform(&out);
        (r.translation, r.rotation)
    }
}

/// 一个 FBX shape key（= glTF 的 morph target）。
///
/// 官方把 FBX 的 shape key **全自动**注册成 flex：desc 名 = shape key 名、
/// 每个 shape key 一个 `flexcontroller`（`min = 0` / `max = 1`）、
/// 一条 `flexrule`（`op[0] = STUDIO_FETCH1`，指向自己的 controller），
/// 帧号 = 文件顺序 + 1（`docs/fbx-support.md` §1.6b）。
///
/// ⚠️ 这里存的 `position_offsets` **已经是 Source 空间的**（官方口径
/// `rot_norm(geometry_to_world) · pos_off`），
/// 而 `vertex_index` 也已经过**跨网格偏移**（[`FbxShapeKey::vertex_index`] 是
/// 全局唯一控制点号，不是「本 mesh 内的控制点号」）。
#[derive(Debug, Clone, PartialEq)]
pub struct FbxShapeKey {
    /// shape key 的名字（原样，官方直接拿它当 flexdesc 名）。
    pub name: String,
    /// 逐顶点的位置偏移（下标与 [`FbxShapeKey::vertex_index`] 平行）。
    pub position_offsets: Vec<[f32; 3]>,
    /// 逐顶点的法线偏移（与 `position_offsets` 平行）。
    ///
    /// ⚠️ 官方在 FBX 路径下**丢弃**它（`shapemix.fbx` 的 8 条 `nrm_off` 全非零，
    /// 官方产物仍写 `ndelta = (0,0,0)`）。保留在结构里只为诊断。
    pub normal_offsets: Vec<[f32; 3]>,
    /// 偏移作用在哪个顶点上。
    ///
    /// ⚠️ 这是**全局唯一**的控制点号（= 该网格在文件里的控制点起始号 + 网格内
    /// 控制点号），与 `SmdVertex::src_index` 同一套编号。FBX 的
    /// `offset_vertices` 原本是**每个 mesh 内**的号，多网格文件（如
    /// `twomesh.fbx`）里 mesh A 的 cp0 与 mesh B 的 cp0 会撞号。
    pub vertex_index: Vec<u32>,
}

/// 一个 FBX 文件读出来的全部内容。
///
/// 几何与参考姿态在 [`FbxGeometry::smd`] 里；动画帧要另外调用
/// [`read_frames`]（它需要 `srcstack` / `srcfps` 两个参数，而几何不需要）。
#[derive(Debug, Clone, PartialEq)]
pub struct FbxGeometry {
    /// 中立的 SMD：`nodes` + 一帧参考姿态 + `triangles`。
    pub smd: Smd,
    /// 全部 shape key，**按 ufbx 给出的文件顺序**。
    pub shape_keys: Vec<FbxShapeKey>,
    /// 全部动画栈的名字（**第一条就是官方会用的那条**）。
    pub anim_stacks: Vec<String>,
    /// 没有材质、被兜底成 [`FALLBACK_MATERIAL`] 的网格名字（用于告警）。
    pub untextured_meshes: Vec<String>,
    /// 被合并进同一个 model 的网格名字（用于告警，官方静默）。
    pub merged_meshes: Vec<String>,
}

impl FbxGeometry {
    /// shape key 的帧号（**1 起**，与官方一致）。
    ///
    /// 帧 0 是「基准」—— 它在 flex 里恒为空载荷（`simplify.cpp:2453-2457`），
    /// 所以第 `i` 个 shape key（0 起）落在帧 `i + 1`。
    pub fn shape_key_frame(index: usize) -> i32 {
        index as i32 + 1
    }
}

/// 读一个 FBX 文件，转成中立 SMD。
pub fn read(path: &Path, at: &str, opts: &FbxOpts) -> Result<FbxGeometry, FbxError> {
    let scene = load_scene(path, at)?;
    from_scene(&scene, opts, path, at)
}

/// 读一个 FBX 文件的动画，转成 SMD 帧。
///
/// * `stack` —— 用哪条动画栈（`None` = 第一条，官方行为）；
/// * `fps` —— 重采样率（[`DEFAULT_FPS`] 是官方值）。
///
/// 骨骼集合与 [`read`] **完全一致**（同一套过滤），所以帧里的
/// `SmdPose.bone` 可以直接和几何侧的 `nodes` 对齐。
pub fn read_frames(
    path: &Path,
    at: &str,
    stack: Option<&str>,
    fps: f32,
    opts: &FbxOpts,
) -> Result<Smd, FbxError> {
    let scene = load_scene(path, at)?;
    let kept = kept_nodes(&scene, opts);
    let nodes = smd_nodes(&kept);

    // ⭐ **没有动画栈不是错误**：官方对「文件里根本没有 `AnimStack`」照样产出
    // **1 帧**（`docs/_probe/oracle_fbx_footguns.js` 的 `U3_noanim` 实测
    // `exit=0` / `frames=1`；`docs/fbx-support.md` §1.11 把它标为「✅ 合理」）。
    // 这不是静默失败，而是「静态文件本来就是 1 帧」，所以 mdlc 照做。
    //
    // 只有用户**显式**点名某条栈时（`stack` 是 `Some`，来自 `srcstack`）才报错
    // —— 那时落到下面的 `position()` 分支，错误里会列出「现有的是 []」。
    if stack.is_none() && scene.anim_stacks.is_empty() {
        return Ok(Smd {
            version: 1,
            nodes,
            frames: vec![SmdFrame {
                time: 0,
                poses: reference_poses(&scene, &kept, opts),
            }],
            triangles: Vec::new(),
        });
    }
    let stack_index = match stack {
        None => {
            // ⭐ 多栈而用户没点名时发一条提示（`docs/fbx-support.md` §1.8 的
            // 最大障碍）：官方恒取第一条，且 `$sequence` 的名字**完全不参与**
            // 栈选择（`oracle_fbx_order.js` 实测 `order_rw` 让 walk/run/idle
            // 三个名字全部拿到 12 帧）。不提示的话用户会以为「名字对得上就该
            // 生效」，然后对着一个不动的动画查半天。
            if scene.anim_stacks.len() > 1 {
                let names: Vec<String> = scene
                    .anim_stacks
                    .iter()
                    .map(|s| s.element.name.to_string())
                    .collect();
                crate::diagln!(
                    "提示：{} 有 {} 条动画栈 {:?}；默认只用**第一条**。要用别的写 `srcstack \"名\"`（写在 `$sequence` / `$animation` 里）。",
                    path.display(),
                    names.len(),
                    names
                );
            }
            0
        }
        Some(want) => scene
            .anim_stacks
            .iter()
            .position(|s| s.element.name.as_ref().eq_ignore_ascii_case(want))
            .ok_or_else(|| {
                let names: Vec<String> = scene
                    .anim_stacks
                    .iter()
                    .map(|s| s.element.name.to_string())
                    .collect();
                ferr(format!(
                    "{} 里没有名为 {want:?} 的动画栈；现有的是 {names:?}。",
                    path.display()
                ))
            })?,
    };

    let fps = if fps > 0.0 { fps } else { DEFAULT_FPS };
    let bake = ufbx::bake_anim(
        &scene,
        &scene.anim_stacks[stack_index].anim,
        ufbx::BakeOpts {
            resample_rate: f64::from(fps),
            ..Default::default()
        },
    )
    .map_err(|err| ferr(format!("{} 的动画烘焙失败：{err:?}", path.display())))?;

    // 帧数 = 重采样后的时长 × fps + 1（含首帧）。
    let duration = bake.playback_duration;
    let n_frames = ((duration * f64::from(fps)).round() as i64).max(0) as usize + 1;

    // `ufbx::find_baked_node_by_typed_id` 是线性搜索；逐帧逐骨骼调它会变成
    // O(帧 × 骨骼²)。先建一次索引表。
    let baked_of: HashMap<u32, usize> = (0..bake.nodes.len())
        .map(|i| (bake.nodes[i].typed_id, i))
        .collect();

    let mut frames: Vec<SmdFrame> = Vec::with_capacity(n_frames);
    // 世界矩阵要按帧重算（父链的缩放/旋转会随动画变化），所以先备好
    // 「每个场景节点在本帧的局部矩阵」的复用缓冲。
    let mut locals: Vec<ufbx::Matrix> = vec![ufbx::Matrix::default(); scene.nodes.len()];
    let mut rots: Vec<ufbx::Quat> = vec![ufbx::Quat::default(); scene.nodes.len()];
    let node_ix: HashMap<u32, usize> = (0..scene.nodes.len())
        .map(|i| (scene.nodes[i].element.element_id, i))
        .collect();
    for fi in 0..n_frames {
        let t = bake.playback_time_begin + (fi as f64) / f64::from(fps);
        for i in 0..scene.nodes.len() {
            let n = &scene.nodes[i];
            let bn = baked_of.get(&n.element.typed_id).map(|&bi| &bake.nodes[bi]);
            let (pos, rot, scale) = match bn {
                None => (
                    n.local_transform.translation,
                    n.local_transform.rotation,
                    n.local_transform.scale,
                ),
                Some(b) => {
                    let rot = ufbx::evaluate_baked_quat(&b.rotation_keys, t);
                    let scale = if b.scale_keys.is_empty() {
                        n.local_transform.scale
                    } else {
                        ufbx::evaluate_baked_vec3(&b.scale_keys, t)
                    };
                    (
                        ufbx::evaluate_baked_vec3(&b.translation_keys, t),
                        rot,
                        scale,
                    )
                }
            };
            rots[i] = rot;
            locals[i] = ufbx::transform_to_matrix(&ufbx::Transform {
                translation: pos,
                rotation: rot,
                scale,
            });
        }
        let world = accumulate_worlds(&scene, &locals);
        let mut poses: Vec<SmdPose> = Vec::with_capacity(kept.len());
        for (i, n) in kept.iter().enumerate() {
            let mi = node_ix.get(&n.element.element_id).copied();
            // 位移口径与参考姿态完全一致（见 [`bone_offset`]）：父链的缩放
            // 会随动画变化，所以这里也必须逐帧重算，不能沿用局部平移。
            let pos = match mi {
                Some(mi) => match n.parent.as_ref().and_then(|p| node_ix.get(&p.element.element_id))
                {
                    Some(&pi) => bone_offset(&world[pi], &world[mi]),
                    // 根骨骼没有父链可除，但自己那份累积缩放仍要除掉（见
                    // [`normalized_translation`]）。
                    None => normalized_translation(&world[mi]),
                },
                None => n.local_transform.translation,
            };
            let rot = mi.map_or(n.local_transform.rotation, |mi| rots[mi]);
            let (pos, rot) = opts.local(ufbx::Transform {
                translation: pos,
                rotation: rot,
                scale: ufbx::Vec3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            });
            poses.push(SmdPose {
                bone: i as i32,
                position: [pos.x as f32, pos.y as f32, pos.z as f32],
                rotation: quat_to_source_euler(rot),
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

/// 把 ufbx 的四元数转成 Source 的 `RadianEuler`（`[roll, pitch, yaw]`，弧度）。
///
/// ⚠️ **必须走 [`crate::bone_math::quaternion_angles`]**（= 官方
/// `QuaternionAngles` 的逐字复刻），不能手搓 —— 那条路径带**万向锁分支**
/// （`matrix_angles` 的 `xy_dist > 0.001`），在锁死附近会丢掉一个自由度。
fn quat_to_source_euler(q: ufbx::Quat) -> [f32; 3] {
    crate::bone_math::quaternion_angles([q.x as f32, q.y as f32, q.z as f32, q.w as f32])
}

// ---------------------------------------------------------------------------
// 官方的两条几何口径 + 一条**有意偏离**
//
// 顶点 / 法线 / UV 三条由 `examples/probe_fbx_cmp.rs` 对**官方产物**逐样本裁决
// 得出（细节见 `docs/fbx-support.md` §1.7）；骨骼位移由
// `examples/probe_fbx_bonepos.rs` 裁决出官方的 `E = P*scale`，但**本实现不照抄**
// —— 官方那个口径会让骨骼比网格大 100 倍，理由见 [`bone_offset`]。
// ---------------------------------------------------------------------------

/// 把一个矩阵的 3×3 部分**逐列归一化**，得到纯旋转（丢掉缩放）。
///
/// FBX 里 `Skeleton` 这类节点常带 `localS = (100,100,100)`（Blender 导出的
/// m→cm 缩放），于是 `geometry_to_world` / `node_to_world` 的基里混着它。
/// 官方在**顶点**与**法线**上把它丢掉（`probe_fbx_cmp` 的 R3/N3，10/10 命中），
/// 却在**骨骼位移**上保留 —— mdlc 两边都丢掉，见 [`bone_offset`]。
fn rotation_of(m: &ufbx::Matrix) -> ufbx::Matrix {
    fn unit(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        let len = (x * x + y * y + z * z).sqrt();
        if len > 0.0 {
            (x / len, y / len, z / len)
        } else {
            (x, y, z)
        }
    }
    let (a0, a1, a2) = unit(m.m00, m.m10, m.m20);
    let (b0, b1, b2) = unit(m.m01, m.m11, m.m21);
    let (c0, c1, c2) = unit(m.m02, m.m12, m.m22);
    ufbx::Matrix {
        m00: a0,
        m10: a1,
        m20: a2,
        m01: b0,
        m11: b1,
        m21: b2,
        m02: c0,
        m12: c1,
        m22: c2,
        m03: 0.0,
        m13: 0.0,
        m23: 0.0,
    }
}

/// 矩阵的平移列。
fn translation_of(m: &ufbx::Matrix) -> ufbx::Vec3 {
    ufbx::Vec3 {
        x: m.m03,
        y: m.m13,
        z: m.m23,
    }
}

/// 顶点位置：`rot_norm(geometry_to_world) · p + normalized_translation(g)`。
///
/// 旋转部分就是 `probe_fbx_cmp` 的 **R3**（9 个样本 10/10 全绿；R1 原样、R2 只旋转、
/// R4 含缩放各有失手）。判别样本是 `rig.fbx` / `twostack.fbx` / `axis_yup.fbx`
/// 这类带「绕 X −90°」旋转的 —— `box.fbx` / `axis_zup.fbx` 上 R1 也会「绿」，
/// 那是旋转恰好为恒等或轴置换造成的**假绿**。
///
/// ⚠️ **平移列与官方的 R3 不同**：官方直接加原始平移，mdlc 加
/// [`normalized_translation`]（除掉累积缩放）。理由是**同一份缩放对两者的作用不同**：
/// 平移列在矩阵链里**会**被父链缩放乘到（`world = parent · T · R · S`），而顶点
/// **尺寸**不会（`rot_norm` 已经把它丢掉）⟹ 不除就会让网格**位置**与网格**尺寸**
/// 差 100 倍（`be1_two_roots.fbx` 的 `body_a` 平移 `(0,50,0)` 其实是 Blender 里的
/// `(0,0.5,0)`，而它 1 米见方的网格顶点仍是 `±0.5`）。
fn geometry_point(g: &ufbx::Matrix, p: ufbx::Vec3) -> ufbx::Vec3 {
    let r = rotation_of(g);
    let t = normalized_translation(g);
    ufbx::Vec3 {
        x: r.m00 * p.x + r.m01 * p.y + r.m02 * p.z + t.x,
        y: r.m10 * p.x + r.m11 * p.y + r.m12 * p.z + t.y,
        z: r.m20 * p.x + r.m21 * p.y + r.m22 * p.z + t.z,
    }
}

/// 法线：只有 `rot_norm(geometry_to_world)`，没有平移（`probe_fbx_cmp` 的 N3）。
///
/// 排除的两条：N1 原样（在轴对齐样本上与 N3 不可分，但一般情形不对）、
/// N4 `ufbx::matrix_for_normals`（会把 100× 缩放带进来，量级 1e7）。
fn geometry_normal(g: &ufbx::Matrix, v: ufbx::Vec3) -> ufbx::Vec3 {
    let r = rotation_of(g);
    ufbx::Vec3 {
        x: r.m00 * v.x + r.m01 * v.y + r.m02 * v.z,
        y: r.m10 * v.x + r.m11 * v.y + r.m12 * v.z,
        z: r.m20 * v.x + r.m21 * v.y + r.m22 * v.z,
    }
}

/// 矩阵 3×3 部分的**逐列长度**（= 该矩阵施加的缩放）。
///
/// `parent_world` 的列长就是父链累积的缩放；`Skeleton` 带 `localS = 100` 时它是
/// `(100,100,100)`。列长为 0 时退回 `1.0`（退化矩阵不该把位移变成 `NaN`）。
fn column_lengths(m: &ufbx::Matrix) -> (f64, f64, f64) {
    fn len(x: f64, y: f64, z: f64) -> f64 {
        let l = (x * x + y * y + z * z).sqrt();
        if l > 0.0 { l } else { 1.0 }
    }
    (
        len(m.m00, m.m10, m.m20),
        len(m.m01, m.m11, m.m21),
        len(m.m02, m.m12, m.m22),
    )
}

/// 平移列**除掉累积缩放**：`translation_of(m) / column_lengths(m)`。
///
/// 与 [`bone_offset`] 用的是同一个判据（见那里的推导）：Blender 的
/// `FBX_SCALE_NONE`（默认 "All Local"）把 m→cm 的 ×100 **左乘**进每个顶层节点的
/// 局部矩阵，于是平移列被 ×100，而它 3×3 部分的列长也正是 100 ⟹ 除掉列长就回到
/// 米。网格节点与骨架节点各带一份同样的 `localS`（两者是兄弟），所以两边都除才
/// 自洽；只除一边就会差 100 倍。
///
/// 列长为 0（退化矩阵）时 [`column_lengths`] 退回 1.0 ⟹ 这里是恒等。
fn normalized_translation(m: &ufbx::Matrix) -> ufbx::Vec3 {
    let t = translation_of(m);
    let (sx, sy, sz) = column_lengths(m);
    ufbx::Vec3 {
        x: t.x / sx,
        y: t.y / sy,
        z: t.z / sz,
    }
}

/// 骨骼位移：`pos = (R_norm(parent_world) · S(parent_world))⁻¹ · (child_world.t − parent_world.t)`。
///
/// ⭐ **这是 mdlc 有意偏离官方的一处**（见 `docs/fbx-support.md` §4.6）。
///
/// 官方（`probe_fbx_bonepos` 的 **E ≡ G**）算的是 `R_norm(parent_world)ᵀ · d`，
/// 父链的**缩放**因此进了位移：`rig.fbx` 的 `Skeleton` 带 `localS = 100`，
/// 官方把 `Spine` 的位移写成 `(0,1000,0)`，而 FBX 里它的局部平移是 `(0,10,0)`。
/// 与此同时官方在**顶点**上用 `rot_norm(geometry_to_world)` 把同一个 100 丢掉了
/// （[`geometry_point`] 的 R3）⟹ 同一个文件编出来**骨骼比网格大 100 倍**，
/// 模型不自洽（`docs/_probe/bone_mesh_space.js` 实测比值 0.0153，人形应落 0.3~3）。
///
/// 这里改成正牌的局部平移：再除以父世界的列长。父链没有缩放时列长 = 1，
/// 除法是恒等 ⟹ 与官方逐值相同（`parity` 的 101 个用例全走 SMD，不受影响；
/// 不带缩放的 FBX 也不受影响）。
///
/// 验收：`aso_none.fbx`（Blender 默认 "All Local"，节点带 `localS = 100`）与
/// `aso_units.fbx`（"FBX Units Scale"，节点无 `localS`、`UnitScaleFactor = 100`）
/// 编出**逐值相同**的产物，且等于官方在 "FBX Units Scale" 下编出的那个
/// （`u_aso_units.mdl`：骨骼 `(0,10,0)` / `(0,30,0)` + 网格 `[-3,-3,0]..[3,3,45]`，
/// 比值 1.5264 ✅）。用户从此不必关心 Blender 的 `Apply Scalings` 选项。
fn bone_offset(parent_world: &ufbx::Matrix, child_world: &ufbx::Matrix) -> ufbx::Vec3 {
    // 两端都取「除掉累积缩放」的世界位置（见 [`normalized_translation`]）——
    // 与顶点口径用的是同一个判据，这样骨骼与网格才落在同一个尺度里。
    let pc = normalized_translation(parent_world);
    let cc = normalized_translation(child_world);
    let d = ufbx::Vec3 {
        x: cc.x - pc.x,
        y: cc.y - pc.y,
        z: cc.z - pc.z,
    };
    let r = rotation_of(parent_world);
    // `Rᵀ · d`：`R` 的第 i 列与 `d` 点积。
    ufbx::Vec3 {
        x: r.m00 * d.x + r.m10 * d.y + r.m20 * d.z,
        y: r.m01 * d.x + r.m11 * d.y + r.m21 * d.z,
        z: r.m02 * d.x + r.m12 * d.y + r.m22 * d.z,
    }
}

/// 按 `scene.nodes` 的 parents-first 顺序把局部矩阵累加成世界矩阵。
fn accumulate_worlds(scene: &ufbx::Scene, locals: &[ufbx::Matrix]) -> Vec<ufbx::Matrix> {
    let ix: HashMap<u32, usize> = (0..scene.nodes.len())
        .map(|i| (scene.nodes[i].element.element_id, i))
        .collect();
    let mut world: Vec<ufbx::Matrix> = Vec::with_capacity(scene.nodes.len());
    for (i, &local) in locals.iter().enumerate() {
        let w = match scene.nodes[i]
            .parent
            .as_ref()
            .and_then(|p| ix.get(&p.element.element_id))
        {
            // 父节点一定排在前面（ufbx 保证 parents-first）。
            Some(&j) if j < i => ufbx::matrix_mul(&world[j], &local),
            _ => local,
        };
        world.push(w);
    }
    world
}

/// 参考姿态（= 源的第 0 帧）：旋转取节点自己的局部旋转，位移取 [`bone_offset`]。
///
/// 官方 `Build_Reference()`（`studiomdl.cpp:728-762`）用源的第 0 帧；FBX 源的
/// 「第 0 帧」就是节点自己的局部 TRS。
fn reference_poses(scene: &ufbx::Scene, kept: &[&ufbx::Node], opts: &FbxOpts) -> Vec<SmdPose> {
    let ix: HashMap<u32, usize> = (0..scene.nodes.len())
        .map(|i| (scene.nodes[i].element.element_id, i))
        .collect();
    let locals: Vec<ufbx::Matrix> = (0..scene.nodes.len())
        .map(|i| ufbx::transform_to_matrix(&scene.nodes[i].local_transform))
        .collect();
    let world = accumulate_worlds(scene, &locals);
    kept.iter()
        .enumerate()
        .map(|(i, n)| {
            let mi = ix.get(&n.element.element_id).copied();
            let pos = match mi {
                Some(mi) => match n.parent.as_ref().and_then(|p| ix.get(&p.element.element_id)) {
                    Some(&pi) => bone_offset(&world[pi], &world[mi]),
                    // 根骨骼没有父链可除，但自己那份累积缩放仍要除掉（见
                    // [`normalized_translation`]）。
                    None => normalized_translation(&world[mi]),
                },
                None => n.local_transform.translation,
            };
            let (pos, rot) = opts.local(ufbx::Transform {
                translation: pos,
                rotation: n.local_transform.rotation,
                scale: ufbx::Vec3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            });
            SmdPose {
                bone: i as i32,
                position: [pos.x as f32, pos.y as f32, pos.z as f32],
                rotation: quat_to_source_euler(rot),
            }
        })
        .collect()
}

fn load_scene(path: &Path, at: &str) -> Result<ufbx::SceneRoot, FbxError> {
    let bytes =
        std::fs::read(path).map_err(|err| ferr(format!("{at}：读不到 {}：{err}", path.display())))?;
    ufbx::load_memory(&bytes, ufbx::LoadOpts::default())
        .map_err(|err| ferr(format!("{at}：{} 解析失败：{err:?}", path.display())))
}

/// 该网格节点是否被 `srcpart` 选中（没写 `srcpart` 时全选）。
fn node_selected(n: &ufbx::Node, opts: &FbxOpts) -> bool {
    if opts.parts.is_empty() {
        return true;
    }
    let name = n.element.name.as_ref();
    opts.parts.iter().any(|p| p.eq_ignore_ascii_case(name))
}

/// 官方 FBX 路径的骨骼过滤（`docs/fbx-support.md` §1.4，7 个用例实测）。
///
/// 规则：
///
/// 1. **种子**：有网格的节点 —— 无蒙皮时插节点自身（`box.fbx` / `morph.fbx`
///    只有一根骨骼就是这个道理）；有蒙皮时插**权重非空**的簇的骨骼节点。
/// 2. **上溯**：种子的全部祖先也保留（`Spine` 自身没被加权，但它是
///    `Head1` 的祖先 ⟹ 保留）；遇到 `is_root` 就停。
/// 3. **丢弃**：ufbx 的合成根（`is_root`）与没进集合的节点。
///
/// ⚠️ **叶节点且零权重 ⟹ 丢弃**（`helper_forward` 在 `rig.fbx` 里被丢），
/// 这与 `$bonemerge` 无关 —— 它不创造骨骼。
fn kept_nodes<'a>(scene: &'a ufbx::Scene, opts: &FbxOpts) -> Vec<&'a ufbx::Node> {
    let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();

    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        if n.is_root || !node_selected(n, opts) {
            continue;
        }
        let Some(mesh) = n.mesh.as_ref() else { continue };
        if mesh.skin_deformers.is_empty() {
            used.insert(n.element.element_id);
        } else {
            for di in 0..mesh.skin_deformers.len() {
                let d = &mesh.skin_deformers[di];
                for ci in 0..d.clusters.len() {
                    let c = &d.clusters[ci];
                    if c.num_weights > 0
                        && let Some(b) = c.bone_node.as_ref()
                    {
                        used.insert(b.element.element_id);
                    }
                }
            }
        }
    }

    // 沿父链上溯（`scene.nodes` 是 parents-first，但上溯本身与顺序无关）。
    let by_id: HashMap<u32, &ufbx::Node> = (0..scene.nodes.len())
        .map(|i| {
            let n = &scene.nodes[i];
            (n.element.element_id, n)
        })
        .collect();
    let mut extra: Vec<u32> = Vec::new();
    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        if !used.contains(&n.element.element_id) {
            continue;
        }
        let mut cur = n.parent.as_ref().map(|p| p.element.element_id);
        while let Some(id) = cur {
            let Some(p) = by_id.get(&id) else { break };
            if p.is_root || used.contains(&id) {
                break;
            }
            extra.push(id);
            cur = p.parent.as_ref().map(|q| q.element.element_id);
        }
    }
    used.extend(extra);

    // 排列顺序：官方是**深度优先先序**，而 `scene.nodes` 是逐层顺序（BFS）。
    let ids: Vec<(u32, Option<u32>)> = (0..scene.nodes.len())
        .map(|i| {
            let n = &scene.nodes[i];
            (
                n.element.element_id,
                n.parent.as_ref().map(|p| p.element.element_id),
            )
        })
        .collect();
    let order = dfs_preorder(&ids, &used);
    order.into_iter().filter_map(|id| by_id.get(&id).copied()).collect()
}

/// 把**逐层顺序**（BFS，即 `ufbx::Scene::nodes` 的顺序）重排成官方骨骼表的
/// **深度优先先序**（DFS pre-order）。
///
/// 输入 `nodes` 按源顺序给出 `(element_id, parent_element_id)`；`keep` 是已经过
/// 过滤（见 [`kept_nodes`]）的 id 集合。输出只含 `keep` 里的 id。
///
/// ⚠️ **为什么必须重排**：`scene.nodes` 是 parents-first 的**层序**，官方骨骼表
/// 却是 DFS 先序。两个判别样本（`docs/_probe/oracle_fbx_boneedge.js`）：
///
/// * `be1_two_roots.fbx`（两根平行骨架）—— ufbx 给 `RigA, RigB, A_root, B_root,
///   A_tip, B_tip`，官方是 `RigA, A_root, A_tip, RigB, B_root, B_tip`；
/// * `bo1_fork.fbx`（`R → A → A1` 与 `R → B → B1`，只 `A1` / `B1` 被加权）——
///   ufbx 给 `Skeleton, R, A, B, A1, B1`，官方是 `Skeleton, R, A, A1, B, B1`。
///
/// ⚠️ **单链样本上两种顺序恰好重合**，所以只有「分叉」样本能把它分开 ——
/// 早先按 `scene.nodes` 原样过滤，在 `rig.fbx` 等 7 个用例上全部通过，
/// 直到 `be1` / `bo1` 才暴露。兄弟姐妹之间沿用输入里的相对顺序。
fn dfs_preorder(nodes: &[(u32, Option<u32>)], keep: &std::collections::HashSet<u32>) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut roots: Vec<u32> = Vec::new();
    for &(id, parent) in nodes {
        if !keep.contains(&id) {
            continue;
        }
        match parent {
            Some(pid) if keep.contains(&pid) => children.entry(pid).or_default().push(id),
            // 父节点不是保留节点（`is_root`、或（防御性地）不在集合里）⟹ 它是本棵树的根。
            _ => roots.push(id),
        }
    }

    let mut out: Vec<u32> = Vec::with_capacity(keep.len());
    let mut stack: Vec<u32> = roots.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        out.push(id);
        if let Some(cs) = children.get(&id) {
            // 反向压栈 ⟹ 出栈顺序 = 输入里的兄弟姐妹顺序。
            stack.extend(cs.iter().rev().copied());
        }
    }
    out
}


fn smd_nodes(kept: &[&ufbx::Node]) -> Vec<SmdNode> {
    let idx_of: HashMap<u32, i32> = kept
        .iter()
        .enumerate()
        .map(|(i, n)| (n.element.element_id, i as i32))
        .collect();
    kept.iter()
        .enumerate()
        .map(|(i, n)| SmdNode {
            index: i as i32,
            name: n.element.name.to_string(),
            parent: n
                .parent
                .as_ref()
                .and_then(|p| idx_of.get(&p.element.element_id).copied())
                .unwrap_or(-1),
        })
        .collect()
}

fn from_scene(
    scene: &ufbx::Scene,
    opts: &FbxOpts,
    path: &Path,
    at: &str,
) -> Result<FbxGeometry, FbxError> {
    let kept = kept_nodes(scene, opts);
    let nodes = smd_nodes(&kept);
    let idx_of: HashMap<u32, i32> = kept
        .iter()
        .enumerate()
        .map(|(i, n)| (n.element.element_id, i as i32))
        .collect();

    // ---- 参考姿态：每根保留骨骼的**局部**变换 ----
    //
    // 官方 `Build_Reference()`（`studiomdl.cpp:728-762`）用的是源的第 0 帧；
    // FBX 源的「第 0 帧」就是节点自己的局部 TRS。
    //
    // 旋转 = 节点自己的局部旋转；位移 = **真正的局部平移**（[`bone_offset`]）。
    // ⚠️ 官方把父链的缩放累积进了位移（`rig.fbx` 的 `Skeleton` 带
    // `localS = (100,100,100)`，官方把 `Spine` 写成 `(0,1000,0)` 而不是
    // FBX 里的 `(0,10,0)`）—— mdlc 有意不照抄，理由见 [`bone_offset`]。
    let poses = reference_poses(scene, &kept, opts);

    // ---- 几何 ----
    let mut triangles: Vec<SmdTriangle> = Vec::new();
    let mut untextured_meshes: Vec<String> = Vec::new();
    let mut merged_meshes: Vec<String> = Vec::new();
    let mut shape_keys: Vec<FbxShapeKey> = Vec::new();
    // 控制点号的**全局**起点（见 `SmdVertex::src_index` 的注释）。
    let mut cp_base: u32 = 0;
    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        if n.is_root || !node_selected(n, opts) {
            continue;
        }
        let Some(mesh) = n.mesh.as_ref() else { continue };
        let mesh_name = n.element.name.to_string();
        merged_meshes.push(mesh_name.clone());
        let node_ix = idx_of.get(&n.element.element_id).copied();

        // ⚠️ FBX 的 `offset_vertices` 是**每个 mesh 内**的控制点号，而 mdlc 把
        // 全部节点的三角形拼进**同一个** `Smd` ⟹ 多网格文件（`twomesh.fbx`）
        // 里 mesh A 的 cp0 与 mesh B 的 cp0 会撞号。给每个网格一段全局唯一的
        // 号段，`SmdVertex::src_index` 与 shape key 的 `vertex_index` 共用它。
        let cp_off = cp_base;
        cp_base += mesh.num_vertices as u32;
        shape_keys.extend(shape_keys_of(mesh, &n.geometry_to_world, cp_off, opts));

        for f in 0..mesh.num_faces {
            let face = mesh.faces[f];
            let mut buf = vec![0u32; mesh.max_face_triangles * 3];
            let n_tris = mesh.triangulate_face(&mut buf, face);
            let mat = material_name(n, mesh, f, opts);
            if (mat == FALLBACK_MATERIAL || Some(&mat) == opts.material.as_ref())
                && !untextured_meshes.contains(&mesh_name)
            {
                untextured_meshes.push(mesh_name.clone());
            }
            for t in 0..n_tris as usize {
                let mut vs: Vec<SmdVertex> = Vec::with_capacity(3);
                for k in 0..3 {
                    // ⚠️ 位置 / 法线 / UV 用**角**下标，权重用**顶点**下标。
                    let corner = buf[t * 3 + k] as usize;
                    let vi = mesh.vertex_indices[corner] as usize;
                    // 网格自己的 `geometry_to_world`（含那个 m→cm 缩放与节点
                    // 旋转）。顶点口径见 [`geometry_point`] / [`geometry_normal`]。
                    let g = &n.geometry_to_world;
                    let p = opts.point(geometry_point(g, mesh.vertex_position[corner]));
                    let local_nrm = if mesh.vertex_normal.exists {
                        mesh.vertex_normal[corner]
                    } else {
                        ufbx::Vec3 {
                            x: 0.0,
                            y: 0.0,
                            z: 1.0,
                        }
                    };
                    let nrm = opts.direction(geometry_normal(g, local_nrm));
                    let uv = if mesh.vertex_uv.exists {
                        let u = mesh.vertex_uv[corner];
                        // ⚠️ V 轴翻转（SMD 解析器里有这一步，FBX 路径不经过它）。
                        [u.x as f32, 1.0 - (u.y as f32)]
                    } else {
                        [0.0, 0.0]
                    };
                    let mut links = weights_of(mesh, vi, &idx_of);
                    // ⚠️ **空 `links` 不是「无绑定」** —— 静态道具（`box.fbx`、
                    // `morph.fbx` 这类没有蒙皮的网格）的每个顶点都没有权重，
                    // 官方把这种顶点绑到**它所在的那个网格节点**上。
                    //
                    // SMD 侧同一条语义由解析器兜底（`smd.rs` 的 `links == 0`
                    // 分支塞进 `parentBone` 一组权重 1.0，见
                    // `docs/_probe/oracle_links_zero.js` 的实测裁决）；FBX 路径
                    // **不经过那个解析器**，所以必须在这里补上，否则：
                    //
                    //   * `SmdInfo::vert_refs` 为空 ⟹ 收骨判据看不到任何顶点引用
                    //     ⟹ 骨骼表空 ⟹ 报「至少需要一根骨骼」；
                    //   * 即使骨骼表不空，`compile::smd_vertex_to_ir` 也会报
                    //     「里有顶点的蒙皮权重全为 0 或没有绑定」。
                    let parent_bone = links
                        .iter()
                        .max_by(|a, b| {
                            a.weight
                                .partial_cmp(&b.weight)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|l| l.bone)
                        .unwrap_or_else(|| node_ix.unwrap_or(0));
                    if links.is_empty() {
                        links.push(SmdBoneLink {
                            bone: parent_bone,
                            weight: 1.0,
                        });
                    }
                    vs.push(SmdVertex {
                        parent_bone,
                        position: [p.x as f32, p.y as f32, p.z as f32],
                        normal: [nrm.x as f32, nrm.y as f32, nrm.z as f32],
                        uv,
                        links,
                        // FBX 的 shape key 是**按控制点**给位移的，这里记住
                        // 这个角来自哪个控制点，供 shape key → flex 展开用
                        // （见 `docs/fbx-support.md` §1.6b）。
                        // ⚠️ 加 `cp_off` 让它**跨网格唯一** —— shape key 的
                        // `vertex_index` 用的是同一套号。
                        src_index: cp_off + vi as u32,
                    });
                }
                triangles.push(SmdTriangle {
                    material: mat.clone(),
                    vertices: [vs[0].clone(), vs[1].clone(), vs[2].clone()],
                });
            }
        }
    }

    if triangles.is_empty() {
        return Err(ferr(format!(
            "{at}：{} 里没有任何三角形（网格）。\
             若写了 `srcpart`，请核对网格名字是否与文件里的一致。",
            path.display()
        )));
    }

    Ok(FbxGeometry {
        smd: Smd {
            version: 1,
            nodes,
            frames: vec![SmdFrame { time: 0, poses }],
            triangles,
        },
        shape_keys,
        anim_stacks: scene
            .anim_stacks
            .iter()
            .map(|s| s.element.name.to_string())
            .collect(),
        untextured_meshes,
        merged_meshes,
    })
}

/// 一个顶点上的全部蒙皮绑定（**按权重降序**，与 SMD 的惯例一致）。
fn weights_of(mesh: &ufbx::Mesh, vi: usize, idx_of: &HashMap<u32, i32>) -> Vec<SmdBoneLink> {
    let mut out = Vec::new();
    for di in 0..mesh.skin_deformers.len() {
        let d = &mesh.skin_deformers[di];
        if vi >= d.vertices.len() {
            continue;
        }
        let sv = d.vertices[vi];
        let b = sv.weight_begin as usize;
        let e = b + sv.num_weights as usize;
        for k in b..e.min(d.weights.len()) {
            let w = d.weights[k];
            if w.weight <= 0.0 {
                continue;
            }
            let Some(c) = d.clusters.get(w.cluster_index as usize) else {
                continue;
            };
            let Some(bn) = c.bone_node.as_ref() else { continue };
            let Some(&my) = idx_of.get(&bn.element.element_id) else {
                continue;
            };
            out.push(SmdBoneLink {
                bone: my,
                weight: w.weight as f32,
            });
        }
    }
    out.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// 一个面的材质名（官方口径，`docs/fbx-support.md` §1.9）。
///
/// 先查 mesh 自己的材质表；查不到再按 `fm - mesh.materials.len()` 查
/// **节点**上的材质表（FBX 允许材质挂在节点上）；再查不到就用兜底名
/// （`srcmaterial` 写了就用它）。
fn material_name(node: &ufbx::Node, mesh: &ufbx::Mesh, face: usize, opts: &FbxOpts) -> String {
    let fm = mesh.face_material.get(face).copied().unwrap_or(0) as usize;
    if let Some(m) = mesh.materials.get(fm) {
        return m.element.name.to_string();
    }
    if let Some(m) = node.materials.get(fm.saturating_sub(mesh.materials.len())) {
        return m.element.name.to_string();
    }
    opts.material
        .clone()
        .unwrap_or_else(|| FALLBACK_MATERIAL.to_string())
}

/// 收集**一个网格**的 shape key，并把偏移变换到 Source 空间。
///
/// 官方对同一个 `BlendDeformer` 下的多个 channel 是逐个注册的
/// （`docs/fbx-support.md` §1.6b 的 8 用例矩阵），顺序就是文件顺序。
///
/// # 官方口径（实测，9/9 数据点全中）
///
/// **`delta = rot_norm(geometry_to_world) · pos_off`** —— 与顶点法线
/// （[`geometry_normal`]）**是同一个原语**。判别性证据：
///
/// * `axisprobe.fbx`：Blender 位移 `(+1,+2,+3)` ⟹ 官方 `(1.0000, 2.9980, -2.0000)`；
/// * `shapemix.fbx`：8 个控制点位移各不相同 ⟹ 8 组 delta 逐条命中
///   `(x, z, -y)`（`3.0 → 2.9980` 是 half 的**向零截断**，见
///   [`crate::mdl_writer::f32_to_half_bits`] 的口径）。
///
/// ⚠️ `(x, z, -y)` 是 `rot_norm` 在**这个文件**上的具体取值，不是通用公式 ——
/// 换个轴向的文件 `rot_norm` 就不同，所以这里必须走矩阵而不是硬编码置换。
///
/// `normal_offsets` 官方**丢弃**（`shapemix` 的 8 条 `nrm_off` 全非零，官方
/// 产物仍写 `ndelta = (0,0,0)`），这里原样保留只为诊断。
///
/// # 过滤
///
/// 官方按「源 vanim」逐个判阈值（`simplify.cpp:2524-2545`，FBX 路径下
/// `ndelta` 恒 0，所以实际就是 `|delta|² > 0.001²`）。`axisprobe.fbx` 的
/// 8 个控制点里有 7 个位移为 0，官方只写了 3 条 vertanim（那一个控制点展开到
/// 它的 3 个焊接顶点）⟹ 零偏移**在展开之前**就被滤掉。
fn shape_keys_of(
    mesh: &ufbx::Mesh,
    geometry_to_world: &ufbx::Matrix,
    cp_off: u32,
    opts: &FbxOpts,
) -> Vec<FbxShapeKey> {
    /// `0.001²`，与 `simplify.cpp:2537` 的 `DotProduct(delta,delta) > 0.001f*0.001f`
    /// 以及 [`crate::flex`] 的 `MIN_DELTA_SQR` 同口径。
    const MIN_DELTA_SQR: f32 = 0.001 * 0.001;

    let mut out: Vec<FbxShapeKey> = Vec::new();
    for di in 0..mesh.blend_deformers.len() {
        let d = &mesh.blend_deformers[di];
        for ci in 0..d.channels.len() {
            let ch = &d.channels[ci];
            let Some(shape) = ch.target_shape.as_ref() else {
                continue;
            };
            let n = shape.num_offsets;
            let mut position_offsets = Vec::with_capacity(n);
            let mut normal_offsets = Vec::with_capacity(n);
            let mut vertex_index = Vec::with_capacity(n);
            for k in 0..n {
                let p = shape.position_offsets[k];
                // `geometry_normal` 就是 `rot_norm(geometry_to_world) · v`；
                // `opts.point` 再叠上 `srcaxis` / `srcscale`（`M` 无平移分量，
                // 所以它作用在**增量**上是正确的）。
                let raw = ufbx::Vec3 {
                    x: p.x,
                    y: p.y,
                    z: p.z,
                };
                let delta = opts.point(geometry_normal(geometry_to_world, raw));
                let d32 = [delta.x as f32, delta.y as f32, delta.z as f32];
                if d32[0] * d32[0] + d32[1] * d32[1] + d32[2] * d32[2] <= MIN_DELTA_SQR {
                    continue;
                }
                let nn = shape.normal_offsets[k];
                position_offsets.push(d32);
                normal_offsets.push([nn.x as f32, nn.y as f32, nn.z as f32]);
                // ⚠️ 加 `cp_off` 让它跨网格唯一（与 `SmdVertex::src_index` 同号段）。
                vertex_index.push(cp_off + shape.offset_vertices[k]);
            }
            if vertex_index.is_empty() {
                continue;
            }
            out.push(FbxShapeKey {
                name: shape.element.name.to_string(),
                position_offsets,
                normal_offsets,
                vertex_index,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单位矩阵（`ufbx::Matrix` 没有 `Default`，只能手搓一次）。
    fn ident() -> ufbx::Matrix {
        ufbx::Matrix {
            m00: 1.0,
            m10: 0.0,
            m20: 0.0,
            m01: 0.0,
            m11: 1.0,
            m21: 0.0,
            m02: 0.0,
            m12: 0.0,
            m22: 1.0,
            m03: 0.0,
            m13: 0.0,
            m23: 0.0,
        }
    }

    /// 均匀缩放矩阵（`scale100()` 与带平移的夹具都用它起手）。
    fn scaled(s: f64) -> ufbx::Matrix {
        ufbx::Matrix {
            m00: s,
            m11: s,
            m22: s,
            ..ident()
        }
    }

    fn v(x: f64, y: f64, z: f64) -> ufbx::Vec3 {
        ufbx::Vec3 { x, y, z }
    }

    fn near(a: ufbx::Vec3, b: (f64, f64, f64)) -> bool {
        (a.x - b.0).abs() < 1e-9 && (a.y - b.1).abs() < 1e-9 && (a.z - b.2).abs() < 1e-9
    }

    /// `Skeleton` 的 `localS = (100,100,100)` —— 全部样本里那个 100× 的来源。
    fn scale100() -> ufbx::Matrix {
        scaled(100.0)
    }

    /// **Blender FBX 导出器的** Z-up → Y-up 约定：`(x,y,z) → (x, z, −y)`。
    ///
    /// ⚠️ 这是**导出器**写的，不是 studiomdl 做的 —— 官方对 FBX 原样透传
    /// （`docs/fbx-support.md` §1.7b）。所以它在这里只用来**造夹具**，
    /// 不是 [`ForcedAxis::Y`]（那个是反向的：把 Y-up 文件转回 Z-up）。
    fn blender_export_rotation() -> ufbx::Matrix {
        ufbx::Matrix {
            m11: 0.0,
            m21: -1.0,
            m12: 1.0,
            m22: 0.0,
            ..ident()
        }
    }

    #[test]
    fn forced_axis_parses_common_spellings() {
        for s in ["y", "Y", "yup", "YUP", "y-up", "Y-Up"] {
            assert_eq!(ForcedAxis::parse(s), Some(ForcedAxis::Y), "{s}");
        }
        for s in ["z", "Z", "zup", "ZUP", "z-up", "Z-Up"] {
            assert_eq!(ForcedAxis::parse(s), Some(ForcedAxis::Z), "{s}");
        }
        assert_eq!(ForcedAxis::parse("x"), None);
        assert_eq!(ForcedAxis::parse(""), None);
    }

    /// `srcaxis y` 把 `(x,y,z)` 映射成 `(x,−z,y)`；`srcaxis z` 是恒等。
    #[test]
    fn forced_axis_rotation_matches_the_documented_mapping() {
        let y = ForcedAxis::Y.rotation();
        assert!(near(ufbx::transform_direction(&y, v(0.0, 1.0, 0.0)), (0.0, 0.0, 1.0)));
        assert!(near(
            ufbx::transform_direction(&y, v(0.0, 0.0, 1.0)),
            (0.0, -1.0, 0.0)
        ));
        assert!(near(ufbx::transform_direction(&y, v(1.0, 0.0, 0.0)), (1.0, 0.0, 0.0)));

        let z = ForcedAxis::Z.rotation();
        assert!(near(
            ufbx::transform_direction(&z, v(1.0, 2.0, 3.0)),
            (1.0, 2.0, 3.0)
        ));
    }

    /// 默认选项必须**逐条等于官方**（不写 `src*` 时不做任何额外的单位/轴干预）。
    ///
    /// ⚠️ 唯一的例外是 [`bone_offset`] 的父链缩放处理 —— 那处是**有意偏离**，
    /// 与这里的选项无关（它修的是官方自身的不自洽，不是用户旋钮）。
    #[test]
    fn default_opts_are_the_official_behaviour() {
        let o = FbxOpts::default();
        assert!(o.parts.is_empty(), "srcpart 缺省 = 取全部网格并合并（官方）");
        assert_eq!(o.material, None, "srcmaterial 缺省 = debug/debugempty");
        assert_eq!(o.scale, 1.0, "srcscale 缺省 = 1.0（官方不做单位换算）");
        assert_eq!(o.axis, None, "srcaxis 缺省 = 不干预（官方原样透传）");
        assert!(o.is_identity());
    }

    /// `point` 叠缩放与旋转，`direction` **只**叠旋转（法线不能被缩放）。
    #[test]
    fn point_scales_but_direction_does_not() {
        let o = FbxOpts {
            scale: 2.0,
            ..Default::default()
        };
        assert!(near(o.point(v(1.0, 2.0, 3.0)), (2.0, 4.0, 6.0)));
        assert!(near(o.direction(v(1.0, 2.0, 3.0)), (1.0, 2.0, 3.0)));

        let o = FbxOpts {
            axis: Some(ForcedAxis::Y),
            ..Default::default()
        };
        assert!(near(o.point(v(0.0, 1.0, 0.0)), (0.0, 0.0, 1.0)));
        assert!(near(o.direction(v(0.0, 1.0, 0.0)), (0.0, 0.0, 1.0)));
    }

    /// `local` 是 `M·L·M⁻¹`：缩放进了平移，旋转被共轭。
    #[test]
    fn local_transform_is_conjugated_by_the_option_matrix() {
        let o = FbxOpts {
            scale: 3.0,
            ..Default::default()
        };
        let t = ufbx::Transform {
            translation: v(1.0, 0.0, 0.0),
            rotation: ufbx::Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            scale: v(1.0, 1.0, 1.0),
        };
        let (p, _) = o.local(t);
        assert!(near(p, (3.0, 0.0, 0.0)), "缩放应进平移：{p:?}");
    }

    /// 恒等选项下 `local` 原样返回（零成本路径）。
    #[test]
    fn local_is_passthrough_when_identity() {
        let o = FbxOpts::default();
        let t = ufbx::Transform {
            translation: v(7.0, 8.0, 9.0),
            rotation: ufbx::Quat {
                x: 0.1,
                y: 0.2,
                z: 0.3,
                w: 0.9,
            },
            scale: v(1.0, 1.0, 1.0),
        };
        let (p, q) = o.local(t);
        assert_eq!(p.x, 7.0);
        assert_eq!(q.x, 0.1, "恒等路径不该走四元数往返（会有精度损失）");
    }

    /// `rotation_of` 逐列归一化 —— 100× 缩放被丢掉，平移被清零。
    #[test]
    fn rotation_of_drops_scale_and_translation() {
        let g = ufbx::Matrix {
            m03: 5.0,
            m13: 6.0,
            m23: 7.0,
            ..scale100()
        };
        let r = rotation_of(&g);
        assert!((r.m00 - 1.0).abs() < 1e-12);
        assert!((r.m11 - 1.0).abs() < 1e-12);
        assert!((r.m22 - 1.0).abs() < 1e-12);
        assert_eq!((r.m03, r.m13, r.m23), (0.0, 0.0, 0.0), "平移必须清掉");
        let t = translation_of(&g);
        assert_eq!((t.x, t.y, t.z), (5.0, 6.0, 7.0));
    }

    /// ⭐ **R3 + 归一化平移**：顶点位置用 `rot_norm·p + 归一化平移`，**缩放既不进顶点尺寸、也不进顶点位置**。
    ///
    /// 判别性：`rig.fbx` 的 `Skeleton` 带 100× 缩放，官方 VVD 顶点仍是
    /// `[-3,-0,-3]..[3,45,3]`（源网格 6×6×45）而不是 ×100 倍。
    ///
    /// ⚠️ 平移那一项与官方的 R3 **不同**（这是有意偏离，见 [`normalized_translation`]）：
    /// 官方加原始平移 `(5,6,7)`，本实现加 `(0.05,0.06,0.07)`。理由是平移列**会**被
    /// 父链缩放乘到（`world = parent·T·R·S`），而顶点尺寸不会 ⟹ 不除就会让网格的
    /// **位置**与**尺寸**差 100 倍（`be1_two_roots.fbx` 的实测症状）。
    #[test]
    fn geometry_point_uses_normalized_rotation_plus_translation() {
        let g = ufbx::matrix_mul(&scale100(), &blender_export_rotation());
        let g = ufbx::Matrix {
            m03: 5.0,
            m13: 6.0,
            m23: 7.0,
            ..g
        };
        // 旋转：(x,y,z) → (x, z, −y)；平移：原始 (5,6,7) ÷ 列长 100。
        assert!(near(
            geometry_point(&g, v(1.0, 2.0, 3.0)),
            (1.05, 3.06, -1.93)
        ));
    }

    /// `normalized_translation` = 平移列 ÷ 列长；无缩放时与 [`translation_of`] 相同。
    #[test]
    fn normalized_translation_divides_out_the_column_lengths() {
        let scaled = ufbx::Matrix {
            m03: 500.0,
            m13: 50.0,
            m23: 0.0,
            ..scale100()
        };
        let t = normalized_translation(&scaled);
        assert!(
            near(t, (5.0, 0.5, 0.0)),
            "应除掉 100× 列长；实际 {t:?}"
        );

        // 无缩放时是恒等（官方口径不受影响）。
        let plain = ufbx::Matrix {
            m03: 5.0,
            m13: 6.0,
            m23: 7.0,
            ..ufbx::Matrix::default()
        };
        let t = normalized_translation(&plain);
        assert!(near(t, (5.0, 6.0, 7.0)), "无缩放时不该改动：{t:?}");
    }

    /// ⭐ **网格位置与网格尺寸必须同尺度**（`be1_two_roots.fbx` 的回归判据）。
    ///
    /// 该夹具的 Blender 场景：1 米见方的立方体，网格节点 `body_a` 带 `localS = 100`
    /// 且局部平移 `(0,50,0)`（= Blender 里的 `(0,0.5,0)`）；骨架祖先 `RigA` 也带
    /// `localS = 100`，骨骼 `A_root → A_tip` 的局部平移是 `(0,1,0)`（1 米）。
    ///
    /// 自洽的判据：**立方体的边长（1 米）应等于骨骼长度（1 米）**，且立方体正好
    /// 骑在这根骨骼上。修复前 mdlc 让骨骼留在 100×（跨度 5.1），修复后又让骨骼掉到
    /// 1× 而网格**位置**留在 100× —— 两次都错；现在两边都在米里。
    #[test]
    fn mesh_position_and_mesh_size_share_one_scale() {
        // R(+90°)，即 `blender_export_rotation()` 的**转置**（= 逆）—— 夹具里
        // `A_root` 的旋转，它把 `RigA` 的 −90° 抵掉（所以 `A_root` 的世界旋转是单位阵）。
        // 逐列写：col0=(1,0,0)、col1=(0,0,1)、col2=(0,−1,0)。
        let rot_plus90 = ufbx::Matrix {
            m11: 0.0,
            m21: 1.0,
            m12: -1.0,
            m22: 0.0,
            ..ident()
        };

        // 骨架：RigA（100× ∘ R(−90°)）→ A_root（R(+90°)）→ A_tip（局部平移 (0,1,0)）。
        let rig_a = ufbx::matrix_mul(&scale100(), &blender_export_rotation());
        let a_root = ufbx::matrix_mul(&rig_a, &rot_plus90);
        let a_tip = ufbx::matrix_mul(&a_root, &{
            let mut t = ident();
            t.m13 = 1.0;
            t
        });

        // 网格：body_a 带 100× 与局部平移 (0,50,0)（cm）；立方体自身是 ±0.5 米。
        let body_a = ufbx::Matrix {
            m03: 0.0,
            m13: 50.0,
            m23: 0.0,
            ..scale100()
        };
        let lo = geometry_point(&body_a, v(0.0, -0.5, 0.0));
        let hi = geometry_point(&body_a, v(0.0, 0.5, 0.0));
        let mesh_span = hi.y - lo.y;
        let mesh_center = (hi.y + lo.y) / 2.0;

        // 骨骼长度（局部口径）与骨骼末端的世界位置。
        let tip_local = bone_offset(&a_root, &a_tip);
        let bone_len = tip_local.y;
        let tip_world = normalized_translation(&a_tip);

        assert!(
            (mesh_span - 1.0).abs() < 1e-9 && (bone_len - 1.0).abs() < 1e-9,
            "网格边长与骨骼长度应同为 1 米；实际 网格={mesh_span} 骨骼={bone_len}"
        );
        assert!(
            (tip_world.y - 1.0).abs() < 1e-9,
            "A_tip 应落在世界 y=1 米处；实际 {}",
            tip_world.y
        );
        assert!(
            (mesh_center - tip_world.y / 2.0).abs() < 1e-9,
            "立方体应骑在骨骼上（中心 = 骨骼中点）；实际 网格中心={mesh_center} 骨骼中点={}",
            tip_world.y / 2.0
        );
    }

    /// ⭐ **N3**：法线只有 `rot_norm`，没有平移；缩放必须丢掉。
    #[test]
    fn geometry_normal_has_no_translation_and_no_scale() {
        let g = ufbx::Matrix {
            m03: 5.0,
            m13: 6.0,
            m23: 7.0,
            ..scale100()
        };
        let n = geometry_normal(&g, v(1.0, 0.0, 0.0));
        assert!(near(n, (1.0, 0.0, 0.0)), "100× 缩放不能进法线：{n:?}");

        let r = blender_export_rotation();
        assert!(near(geometry_normal(&r, v(0.0, 1.0, 0.0)), (0.0, 0.0, -1.0)));
    }

    /// ⭐ **有意的偏离**：骨骼位移是**真正的局部平移**，不继承父链缩放。
    ///
    /// 判别样本是 `axis_zup.fbx` 的形状：父世界旋转**不是**单位阵
    /// （`Skeleton` 恒等、`Pelvis` 自带旋转）。官方的 `E ≡ P*scale` 在这里给出
    /// `(0,1000,0)`（`probe_fbx_bonepos` 10/10 吻合），本实现给出 `(0,10,0)` ——
    /// 后者才是 FBX 里 `Spine` 的局部平移，也才与网格同尺度。
    #[test]
    fn bone_offset_drops_the_parent_chain_scale() {
        // `axis_zup.fbx`：Skeleton 恒等、Pelvis 自带旋转，Spine 的局部平移 (0,10,0)。
        let skeleton = scale100();
        let pelvis = ufbx::matrix_mul(&skeleton, &blender_export_rotation());
        let spine = ufbx::matrix_mul(&pelvis, &{
            let mut t = blender_export_rotation();
            t.m13 = 10.0;
            t
        });
        // 官方 "ValveBiped.Bip01_Spine" pos = [0, 999.9999389648438, ~0]（= 100×）
        let p = bone_offset(&pelvis, &spine);
        assert!(
            near(p, (0.0, 10.0, 0.0)),
            "应为 (0,10,0)（父链 100× 缩放必须除掉）；实际 {p:?}"
        );
    }

    /// ⭐ **验收判据**：Blender 的两种单位导出方式编出**逐值相同**的骨骼表。
    ///
    /// `aso_none.fbx`（默认 "All Local"）把 ×100 写进 `Skeleton` / `body` 的
    /// `localS`；`aso_units.fbx`（"FBX Units Scale"）不写 `localS`，改用
    /// `UnitScaleFactor = 100`。两者几何完全相同，只有单位信息放在哪的差别。
    /// 官方编出两个**骨骼跨度差 100 倍**的模型（比值 0.0153 vs 1.5264）；
    /// 本实现两边都归到 `(0,10,0)` / `(0,20,0)`，用户不必关心那个选项。
    #[test]
    fn bone_offset_is_identical_for_both_blender_unit_export_forms() {
        let rot = blender_export_rotation();
        let local = |dy: f64| {
            let mut t = blender_export_rotation();
            t.m13 = dy;
            t
        };

        // "All Local"：×100 烘在 Skeleton 的 localS 里。
        let all_local = {
            let skeleton = scale100();
            let pelvis = ufbx::matrix_mul(&skeleton, &rot);
            let spine = ufbx::matrix_mul(&pelvis, &local(10.0));
            let head = ufbx::matrix_mul(&spine, &local(10.0));
            [
                bone_offset(&skeleton, &pelvis),
                bone_offset(&pelvis, &spine),
                bone_offset(&spine, &head),
            ]
        };

        // "FBX Units Scale"：没有 localS，单位记在 UnitScaleFactor（本函数看不到）。
        let units_scale = {
            let skeleton = ident();
            let pelvis = ufbx::matrix_mul(&skeleton, &rot);
            let spine = ufbx::matrix_mul(&pelvis, &local(10.0));
            let head = ufbx::matrix_mul(&spine, &local(10.0));
            [
                bone_offset(&skeleton, &pelvis),
                bone_offset(&pelvis, &spine),
                bone_offset(&spine, &head),
            ]
        };

        for i in 0..3 {
            assert!(
                near(all_local[i], (units_scale[i].x, units_scale[i].y, units_scale[i].z)),
                "骨骼 {i} 两种导出方式必须一致：All Local {:?} vs Units Scale {:?}",
                all_local[i],
                units_scale[i]
            );
        }
        assert!(near(all_local[1], (0.0, 10.0, 0.0)), "{:?}", all_local[1]);
        assert!(near(all_local[2], (0.0, 10.0, 0.0)), "{:?}", all_local[2]);
    }

    /// 父链没有缩放时，本实现与官方口径**逐值相同**（除法是恒等）。
    ///
    /// 这是「改动不影响既有 FBX 用例」的守门测试：`parity` 的 101 个用例全走
    /// SMD，而不带节点缩放的 FBX（绝大多数）走的就是这条路径。
    #[test]
    fn bone_offset_equals_official_when_parent_has_no_scale() {
        let parent = ident();
        let mut child = ident();
        child.m13 = 1000.0;
        let p = bone_offset(&parent, &child);
        assert!(near(p, (0.0, 1000.0, 0.0)), "无缩放时应与官方相同：{p:?}");
    }

    /// `shape_key_frame` = 文件顺序 + 1（帧 0 是基准，恒空载荷）。
    #[test]
    fn shape_key_frame_is_one_based() {
        assert_eq!(FbxGeometry::shape_key_frame(0), 1);
        assert_eq!(FbxGeometry::shape_key_frame(1), 2);
        assert_eq!(FbxGeometry::shape_key_frame(41), 42);
    }

    /// ⭐ **DFS 先序**（官方骨骼表顺序）：`be1_two_roots.fbx` 的判别用例。
    ///
    /// ufbx 的 `scene.nodes` 是层序 `RigA, RigB, A_root, B_root, A_tip, B_tip`，
    /// 官方骨骼表是 `RigA, A_root, A_tip, RigB, B_root, B_tip`。
    #[test]
    fn dfs_preorder_matches_two_parallel_roots() {
        // (id, parent)：0=RigA 1=RigB 2=A_root 3=B_root 4=A_tip 5=B_tip
        let nodes = [
            (0u32, None),
            (1, None),
            (2, Some(0)),
            (3, Some(1)),
            (4, Some(2)),
            (5, Some(3)),
        ];
        let keep: std::collections::HashSet<u32> = (0..6).collect();
        assert_eq!(dfs_preorder(&nodes, &keep), vec![0, 2, 4, 1, 3, 5]);
    }

    /// ⭐ **DFS 先序**：`bo1_fork.fbx` 的判别用例（兄弟姐妹顺序要保留）。
    ///
    /// ufbx 给 `Skeleton, R, A, B, A1, B1`；官方是 `Skeleton, R, A, A1, B, B1`。
    /// 注意 `A2` 在源里排在 `B` 前面（层序），但**不在** `keep` 里。
    #[test]
    fn dfs_preorder_matches_forked_hierarchy() {
        // 0=Skeleton 1=R 2=A 3=B 4=A1 5=A2 6=B1
        let nodes = [
            (0u32, None),
            (1, Some(0)),
            (2, Some(1)),
            (3, Some(1)),
            (4, Some(2)),
            (5, Some(2)),
            (6, Some(3)),
        ];
        let keep: std::collections::HashSet<u32> = [0, 1, 2, 3, 4, 6].into_iter().collect();
        assert_eq!(dfs_preorder(&nodes, &keep), vec![0, 1, 2, 4, 3, 6]);
    }

    /// 单链上 DFS 与层序**恰好重合** —— 这正是这个 bug 能躲过 `rig.fbx` 等
    /// 7 个用例的原因，也是本条测试存在的理由（守住「别以为单链绿了就没事」）。
    #[test]
    fn dfs_preorder_is_identity_on_a_single_chain() {
        let nodes = [
            (0u32, None),
            (1, Some(0)),
            (2, Some(1)),
            (3, Some(2)),
        ];
        let keep: std::collections::HashSet<u32> = (0..4).collect();
        assert_eq!(dfs_preorder(&nodes, &keep), vec![0, 1, 2, 3]);
    }

    /// 父节点**不在** `keep` 里时，子节点成为新树的根（`is_root` 被过滤后
    /// 的常见形态）；多棵树按源顺序依次展开。
    #[test]
    fn dfs_preorder_starts_a_new_tree_when_the_parent_is_filtered() {
        // 0=root(被丢) 1=A 2=A_child 3=B
        let nodes = [
            (0u32, None),
            (1, Some(0)),
            (2, Some(1)),
            (3, Some(0)),
        ];
        let keep: std::collections::HashSet<u32> = [1, 2, 3].into_iter().collect();
        assert_eq!(dfs_preorder(&nodes, &keep), vec![1, 2, 3]);
    }

    /// 空集合与「全部被过滤」都必须给出空结果（不 panic）。
    #[test]
    fn dfs_preorder_handles_empty_input() {
        let empty: std::collections::HashSet<u32> = std::collections::HashSet::new();
        assert!(dfs_preorder(&[], &empty).is_empty());
        assert!(dfs_preorder(&[(0, None)], &empty).is_empty());
    }
}
