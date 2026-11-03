//! DMX（Valve datamodel）导入。
//!
//! # 为什么要有这一层
//!
//! BlenderSourceTools（下称 BST）导出的模型就是 DMX：Blender 里 `export_format` 选
//! `DMX` 时，插件写的是二进制 datamodel 而不是 SMD。mdlc 原先只认 TOML / QC / SMD，
//! 于是这条路走不通。
//!
//! # 这一层只做「读」
//!
//! 本模块把 DMX 读成一组中间结构（[`DmxModel`] / [`DmxBone`] / [`DmxMesh`]），
//! **不碰** [`crate::model::ModelDesc`]。理由：DMX 的 schema 比 mdlc 的 IR 宽得多
//! （`DmeLog`、`DmeCombinationOperator`、`DmeVertexDeltaData` …… 全是动画与表情的东西），
//! 而「哪些字段映射到 IR 的哪个字段」是要靠真实产物逐项验的活。先把「读得对」与
//! 「映射得对」拆开，出错时能立刻分清是哪一边。
//!
//! # BST 实际写出来的 schema
//!
//! 下面是实测（L4D2 / GMod / SFM 三种预设各导一份）抠出来的属性表，不是照抄文档：
//!
//! ```text
//! DmeModel      name, children[], baseStates[], transform, jointList[], jointTransforms[]
//! DmeDag        name, shape→DmeMesh|DmeJoint, transform, children[]
//! DmeJoint      name, transform
//! DmeTransform  name, position(Vector3), orientation(Quaternion)
//! DmeMesh       name, visible, bindState→DmeVertexData, currentState, baseStates[], faceSets[]
//! DmeVertexData name, vertexFormat[], flipVCoordinates, jointCount,
//!               positions/positionsIndices, normals/normalsIndices,
//!               textureCoordinates/textureCoordinatesIndices,
//!               jointWeights(FloatArray) / jointIndices(IntegerArray)
//! DmeMaterial   name, mtlName
//! DmeFaceSet    name, material→DmeMaterial, faces(IntegerArray)
//! ```
//!
//! 五个容易踩的细节（全部是实测踩出来的，不是读文档读来的）：
//!
//! 1. 属性名是 **`jointWeights` / `jointIndices`**（大写 `W` / `I`），不是社区文档里
//!    常见的全小写写法。属性名在 datamodel 里是大小写敏感的。
//! 2. **BST 不写 `jointnames`**。`jointIndices` 的索引空间就是 `DmeModel.jointList`
//!    的顺序 —— 这也是官方 `dmxconvert` 认的口径。
//! 3. `jointTransforms[]` 只在 format ≥ 15 出现，且与 `jointList` 一一对应；两个都读
//!    会重复计数，所以只认 `jointList`。
//! 4. **`DmeFaceSet.faces` 不是三角形三元组**，而是「若干顶点下标 + `-1` 结束符」的
//!    多边形表：立方体是 `[0,1,2,3,-1, 4,5,6,7,-1, …]`（6 个四边形）。按三元组硬切
//!    会得到 `(0,1,2) (3,-1,4) …` 这种垃圾 —— 必须按 `-1` 切完再扇形三角化。
//! 5. **骨骼层级在 `children` 上**，而且挂在**父骨骼元素自己**（`DmeJoint` 在 Source
//!    的 schema 里本该没有这个属性）。按「`DmeDag.shape` 指向 joint」那套走会得到
//!    「全部都是根骨骼」。另外 `jointList` 里可能混着**网格自己的 `DmeDag`**（BST 给
//!    它留一个槽位，未被蒙皮的顶点就指向它），所以不能假设「jointList 里全是骨骼」。
//!
//! 权重布局：`jointCount` 是「每顶点最多几条权重链」（BST 取所有顶点的最大值，至少 1），
//! 每个顶点写满 `jointCount` 条，于是 `jointWeights.len() == positions.len() * jointCount`。
//!
//! # 依赖
//!
//! 解析器是 `datamodel` crate（`=0.10.0`，钉精确版本：它 0.x 会破坏性发版，
//! 且上游零单元测试）。验收走
//! `cargo run --example probe_dmx -- <file.dmx>`。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use datamodel::attribute::AttributeValue;
use datamodel::element::Element;

/// 一个 DMX 模型读出来的全部内容。
#[derive(Debug, Clone, Default)]
pub struct DmxModel {
    /// `Header.format_version`，例如 L4D2 预设是 15、GMod 是 1、SFM 是 18。
    pub format_version: u32,
    /// 骨骼，顺序与 `DmeModel.jointList` 一致（`jointIndices` 就是按这个顺序索引的）。
    pub bones: Vec<DmxBone>,
    /// 材质，按首次出现的顺序。
    pub materials: Vec<DmxMaterial>,
    /// 网格，一个 `DmeMesh` 一项。
    pub meshes: Vec<DmxMesh>,
    /// 文件里有没有动画数据（任意 `Dme*Log` 元素）。
    ///
    /// BST 把动画写成**单独一个文件**（`anims/<骨架名>.dmx`），模型文件里没有
    /// 通道，所以这个标志决定 [`read_frames`] 走哪条路：有动画 ⟹ 逐帧读
    /// 通道日志；没有 ⟹ 给一帧绑定姿态（静态道具的 DMX 就长这样）。
    pub has_animation: bool,
    /// 形态键控制器的名字，顺序即 `DmeCombinationOperator.controls`。
    ///
    /// `deltaStates[i]` 的 flex 名要取这里的第 `i` 项（见 [`DmxDelta`]）。
    pub controls: Vec<String>,
    /// 读得下去但值得提醒的事（例如一个网格挂了多个材质）。
    pub warnings: Vec<String>,
}

/// 一根骨骼。
#[derive(Debug, Clone, Default)]
pub struct DmxBone {
    pub name: String,
    /// 父骨骼在 [`DmxModel::bones`] 里的下标；根骨骼是 `None`。
    pub parent: Option<usize>,
    /// 相对父骨骼的平移。
    ///
    /// ⚠️ 实测 BST 写出来的值**已经等于 SMD skeleton 行**（`spine` 是 `[0, 10, 0]`、
    /// `head` 是 `[0, 11.180, 0]`，与 `Collection.smd` 逐位相同），不需要再做
    /// 「父链矩阵求相对」那一套 —— 那是我原本预想的做法，读真文件才发现多余。
    pub position: [f32; 3],
    /// 相对父骨骼的旋转，`[x, y, z, w]`。
    ///
    /// 同一份夹具里 `root` 是 `[0.6830, 0.1830, 0.1830, 0.6830]`，正好是 SMD 欧拉角
    /// `(90°, 0°, 30°)` 的四元数（`qz(30°) ⊗ qx(90°)`）⟹ 直接
    /// [`crate::bone_math::quaternion_angles`] 就能换成 SMD 的欧拉角。
    pub orientation: [f32; 4],
    /// 这个槽位是不是**真骨骼**（`DmeJoint`）。
    ///
    /// `jointList` 里会混进网格自己的 `DmeDag`（BST 给未蒙皮的顶点留的落点），
    /// 它不是骨骼，不能进 SMD 的 `nodes` 表。
    pub is_joint: bool,
}

/// 一个材质。
#[derive(Debug, Clone, Default)]
pub struct DmxMaterial {
    /// 元素名（`DmeMaterial.name`）。
    pub name: String,
    /// 真正写进 MDL 的材质名（`DmeMaterial.mtlName`），形如 `materials/models/x/y.vmt`。
    pub material: String,
}

/// 一个网格。
///
/// ⚠️ **法线与 UV 不能摊平到 position 空间**。DMX 把它们存在 loop（角）空间：
/// 立方体有 8 个 position 却有 24 个不同的「位置 + 法线 + UV」组合，每个面一套。
/// 摊平到 position 只会留下「第一个用到该顶点的角」，于是六个面共用一套法线 ——
/// 编译出来的模型每个面都朝同一个方向。所以这里按**逐角**存（[`DmxTriangle`]）。
#[derive(Debug, Clone, Default)]
pub struct DmxMesh {
    pub name: String,
    /// 顶点位置（position 空间，去重后的）。
    pub positions: Vec<[f32; 3]>,
    /// 每顶点的权重链：`(jointList 下标, 权重)`，权重为 0 的已剔除。
    pub weights: Vec<Vec<(u32, f32)>>,
    /// 三角形，逐角带着自己的法线与 UV。
    pub triangles: Vec<DmxTriangle>,
    /// 形态键（`DmeMesh.deltaStates`），顺序即文件里的 `deltaStates` 顺序。
    pub deltas: Vec<DmxDelta>,
}

/// 一个形态键（`DmeVertexDeltaData`）。
///
/// 两条实测结论：
///
/// * **数据是增量不是绝对值** —— `positions` 每项要加到基准网格对应的顶点上
///   （BST 导出的立方体里 `wide` 写的是 `(0, 0, ±3)` 这种纯偏移）。
/// * **不要拿 `DmeVertexDeltaData.name` 去配控制器**。夹具里名为 `wide` 的
///   delta 带着另一条形态键的位移（±3 是 `tall` 的、±2.5 是 `wide` 的），
///   名字与数据对不上。可靠的配对是**下标**：`deltaStates[i]` 对应
///   `DmeCombinationOperator.controls[i]`，所以名字要取控制器的。
#[derive(Debug, Clone, Default)]
pub struct DmxDelta {
    /// `DmeVertexDeltaData.name`（**只用来做诊断**，不作配对依据）。
    pub name: String,
    /// 位置增量，与 [`DmxDelta::position_indices`] 平行。
    pub positions: Vec<[f32; 3]>,
    /// 每个增量作用在 [`DmxMesh::positions`] 的哪个下标上。
    pub position_indices: Vec<u32>,
}

/// 一个三角形。材质是**逐面集**给的，同一网格里可以混（BST 按材质切 face set）。
#[derive(Debug, Clone, Default)]
pub struct DmxTriangle {
    /// 材质在 [`DmxModel::materials`] 里的下标；该 face set 没有材质时是 `None`。
    pub material: Option<usize>,
    /// 三个角，顺序即原文件顺序（Source 的正面是 CW）。
    pub corners: [DmxCorner; 3],
}

/// 一个角：它自己的位置、法线、UV。
#[derive(Debug, Clone, Copy, Default)]
pub struct DmxCorner {
    /// [`DmxMesh::positions`] 的下标。
    pub position: u32,
    /// 该角的法线（loop 空间取出来的）。
    pub normal: [f32; 3],
    /// 该角的 UV（已按 `flipVCoordinates` 归一化成 Valve 口径）。
    pub uv: [f32; 2],
}

/// 读 DMX 时可能出的错。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DmxError {
    /// 打不开文件。
    #[error("{}", crate::tr_fmt!("cannot open %{path}: %{message}", path = path, message = message))]
    Io { path: String, message: String },
    /// 文件不是合法的 datamodel。
    #[error("{}", crate::tr_fmt!("%{path} is not a Valve datamodel: %{message}", path = path, message = message))]
    Parse { path: String, message: String },
    /// 是 datamodel，但不是 `model`（例如 `animation` 或 `sheet`）。
    #[error("{}", crate::tr_fmt!("%{path} is a %{format} datamodel, not a model one", path = path, format = format))]
    NotAModel { path: String, format: String },
    /// 结构上缺东西（没有 `DmeModel`、网格没有 `bindState` 等）。
    #[error("{}", crate::tr_fmt!("%{path}: %{message}", path = path, message = message))]
    Malformed { path: String, message: String },
}

impl DmxError {
    fn malformed(path: &str, message: impl Into<String>) -> Self {
        Self::Malformed {
            path: path.to_owned(),
            message: message.into(),
        }
    }
}

/// 读一个 DMX 模型。
pub fn load(path: &Path) -> Result<DmxModel, DmxError> {
    let label = path.display().to_string();
    let file = File::open(path).map_err(|e| DmxError::Io {
        path: label.clone(),
        message: e.to_string(),
    })?;
    read_from(&mut BufReader::new(file), &label)
}

/// 从任意 reader 读。抽出来是为了让单元测试能直接吃内嵌的字节。
fn read_from<R: BufRead>(reader: &mut R, path: &str) -> Result<DmxModel, DmxError> {
    let (header, root) = datamodel::deserialize(reader).map_err(|e| DmxError::Parse {
        path: path.to_owned(),
        message: e.to_string(),
    })?;

    if header.format != "model" {
        return Err(DmxError::NotAModel {
            path: path.to_owned(),
            format: header.format,
        });
    }

    // 一次性把所有元素摊平（按 id 去重）。datamodel 保留对象身份，同一个元素从多条
    // 路径都能走到 —— 不按 id 去重会把共享子树重复计数，权重和材质也会跟着翻倍。
    let mut flat = Vec::new();
    let mut seen = HashSet::new();
    walk(&root, &mut flat, &mut seen);

    let model = flat
        .iter()
        .find(|e| class_is(e, "DmeModel"))
        .ok_or_else(|| DmxError::malformed(path, "the file has no DmeModel element"))?;

    let mut out = DmxModel {
        format_version: u32::try_from(header.format_version).unwrap_or_default(),
        // BST 把动画写成 `DmeChannelsClip` + 每条通道一个 `DmeQuaternionLog` /
        // `DmeVector3Log`。类名不是 `DmeLog`，是 `Dme*Log` —— 按前缀认。
        has_animation: flat.iter().any(|e| {
            let class = e.get_class();
            class.starts_with("Dme") && class.ends_with("Log")
        }),
        ..Default::default()
    };

    out.materials = collect_materials(&flat);
    out.bones = collect_bones(model);
    out.controls = collect_control_names(&flat);
    let mesh_transforms = collect_mesh_transforms(model);
    out.meshes = collect_meshes(&flat, &out.materials, &mesh_transforms, &mut out.warnings);
    Ok(out)
}

/// 形态键控制器的名字，顺序即 `DmeCombinationOperator.controls` 的顺序。
///
/// 这就是 `deltaStates[i]` 该叫什么 —— **不能**用 `DmeVertexDeltaData.name`，
/// 实测两者会对不上（见 [`DmxDelta`]）。
fn collect_control_names(flat: &[Element]) -> Vec<String> {
    let Some(operator) = flat
        .iter()
        .find(|e| class_is(e, "DmeCombinationOperator"))
    else {
        return Vec::new();
    };
    element_array_attr(operator, "controls")
        .iter()
        .map(|control| {
            string_array_attr(control, "rawControlNames")
                .into_iter()
                .next()
                .or_else(|| string_attr(control, "name"))
                .unwrap_or_default()
        })
        .collect()
}

/// 每个网格在**模型空间**里的变换（位置 + 四元数），按网格元素 id 索引。
///
/// ⚠️ 不能直接用网格顶点：DMX 的 `positions` 是**网格自身空间**的，而 SMD 的顶点是
/// **骨架空间**的（BST 导出 SMD 时把物体变换烘进去了）。实测同一份场景：
/// DMX 顶点 `z ∈ [-4, 4]`，SMD 顶点 `z ∈ [0, 8]` —— 差的正是 `DmeDag` 的
/// `position = [0, 0, 4]`。不烘这一层，模型会整体偏移（这里是 4 个单位）。
fn collect_mesh_transforms(
    model: &Element,
) -> HashMap<String, ([f32; 3], [f32; 4])> {
    let mut out: HashMap<String, ([f32; 3], [f32; 4])> = HashMap::new();
    let mut stack: Vec<(Element, [f32; 3], [f32; 4])> = element_array_attr(model, "children")
        .into_iter()
        .map(|child| (child, [0.0; 3], [0.0, 0.0, 0.0, 1.0]))
        .collect();
    let mut seen = HashSet::new();
    while let Some((element, parent_position, parent_orientation)) = stack.pop() {
        if !seen.insert(id_of(&element)) {
            continue;
        }
        let (local_position, local_orientation) = transform_of(&element);
        let world_position = add3(
            parent_position,
            rotate(parent_orientation, local_position),
        );
        let world_orientation = multiply(parent_orientation, local_orientation);
        if let Some(shape) = element_attr(&element, "shape")
            && class_is(&shape, "DmeMesh")
        {
            out.insert(id_of(&shape), (world_position, world_orientation));
        }
        for child in element_array_attr(&element, "children") {
            stack.push((child, world_position, world_orientation));
        }
    }
    out
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// 用四元数转一个向量：`v' = v + 2w(q×v) + 2(q×(q×v))`。
fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = q;
    let t = [
        2.0 * (y * v[2] - z * v[1]),
        2.0 * (z * v[0] - x * v[2]),
        2.0 * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

/// 四元数乘法 `a ⊗ b`（先 `b` 后 `a` 的复合顺序，与 SMD 的口径一致）。
fn multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

// ---------------------------------------------------------------- 转成 SMD

/// DMX 转成 SMD 口径之后的结果。
///
/// 把 `.dmx` 当成 `.smd` 用（`$body` / `$model` / LOD）：`smd` 里是节点、参考
/// 姿态与三角形，`shape_keys` 是 `DmeMesh.deltaStates`（形态键）。动画不走这里，
/// 见 [`read_frames`]。
#[derive(Debug, Clone)]
pub struct DmxGeometry {
    pub smd: crate::smd::Smd,
    /// 形态键（`DmeMesh.deltaStates`），已经换算成 Source 空间与全局控制点号。
    ///
    /// 与 FBX 路径口径一致：官方把形态键全自动注册成 flex，所以这份数据不能丢。
    pub shape_keys: Vec<crate::fbx::FbxShapeKey>,
    pub warnings: Vec<String>,
}

/// 读一个 DMX 模型并转成 SMD 口径的几何。
pub fn read(path: &Path, at: &str) -> Result<DmxGeometry, DmxError> {
    let label = path.display().to_string();
    let model = load(path)?;
    to_smd(&model, at, &label)
}

/// 读一个 DMX 的**几何**（必须真有三角形）。
///
/// 与 [`read`] 的差别只有一条：网格源里一个三角形都没有是错误。
/// BST 的动画文件（`anims/<骨架>.dmx`）只有骨架，那种文件会被 QC 预解析
/// 用 [`read`] 打开一次（只取节点名），所以拦在这里而不是拦在读取层。
pub fn read_geometry(path: &Path, at: &str) -> Result<DmxGeometry, DmxError> {
    let geometry = read(path, at)?;
    if geometry.smd.triangles.is_empty() {
        return Err(DmxError::malformed(
            &path.display().to_string(),
            "the model contains no triangles",
        ));
    }
    Ok(geometry)
}

/// 把一个 DMX 当**动画源**读（`$sequence` / `$animation` 直接指向它）。
///
/// * 文件里**有**动画数据（`Dme*Log`）⟹ 逐帧读出来（见 `read_animation`）；
/// * 文件里**没有**动画数据 ⟹ 给一帧绑定姿态，与 `.smd` 只有 `skeleton` 段时一致
///   （BST 给静态道具导出的 DMX 就长这样）。
///
/// ⚠️ 返回的 [`crate::smd::Smd`] 的 `triangles` 是空的 —— 与
/// [`crate::fbx::read_frames`] 同一口径，调用方只取 `frames`。
pub fn read_frames(path: &Path, at: &str) -> Result<crate::smd::Smd, DmxError> {
    let label = path.display().to_string();
    let model = load(path)?;
    if model.has_animation {
        let file = File::open(path).map_err(|err| DmxError::Io {
            path: label.clone(),
            message: err.to_string(),
        })?;
        return read_animation(&mut BufReader::new(file), &label, &model);
    }
    let mut smd = to_smd(&model, at, &label)?.smd;
    smd.triangles.clear();
    Ok(smd)
}

/// 从动画 DMX 里读出逐帧姿态。
///
/// BST 把动画写在**单独的文件**里（`anims/<骨架名>.dmx`），结构与模型文件不同：
/// 根上是 `animationList` → `DmeAnimationList` → `DmeChannelsClip`，每个通道
/// （`DmeChannel`）指向一个 `DmeTransform` 和它的 `position` / `orientation` 日志。
///
/// 三个实测结论决定了这里的做法：
///
/// * **时间单位是 1/10000 秒的 tick**：`frameRate = 20` 时相邻两帧差 500。
/// * **值已经是骨骼空间的绝对局部变换**（与 `DmeTransform` 逐位相同），所以
///   逐帧只需要 [`crate::bone_math::quaternion_angles`]，不需要父链矩阵。
/// * **常量通道会被压成 1 个键**（`times = [0]`），所以取样必须支持「只有一个键
///   ⟹ 恒定」，不能假设键数等于帧数。
fn read_animation<R: BufRead>(
    reader: &mut R,
    label: &str,
    model: &DmxModel,
) -> Result<crate::smd::Smd, DmxError> {
    use crate::smd::{SmdFrame, SmdPose};

    let (_, root) = datamodel::deserialize(reader).map_err(|err| DmxError::Parse {
        path: label.to_owned(),
        message: err.to_string(),
    })?;
    let mut flat = Vec::new();
    let mut seen = HashSet::new();
    walk(&root, &mut flat, &mut seen);

    let clip = element_attr(&root, "animationList")
        .and_then(|list| element_array_attr(&list, "animations").into_iter().next())
        .or_else(|| {
            flat.iter()
                .find(|el| class_is(el, "DmeChannelsClip"))
                .cloned()
        })
        .ok_or_else(|| {
            DmxError::malformed(
                label,
                "the file signals animation data but has no DmeChannelsClip",
            )
        })?;

    // 帧率决定 tick ↔ 帧的换算。官方写法是 `Integer`，SFM 以后可能是浮点。
    let fps = int_attr(&clip, "frameRate").unwrap_or(30).max(1) as f32;
    let ticks_per_frame = 10_000.0 / fps;

    // `DmeTransform` 元素 id → 骨骼槽位。通道的 `toElement` 指向变换本身，
    // 名字（`root_p` / `root_o`）只是给人看的，**不能**拿来配对。
    let skeleton = element_attr(&root, "skeleton")
        .or_else(|| flat.iter().find(|el| class_is(el, "DmeModel")).cloned());
    let mut transform_slot: HashMap<String, usize> = HashMap::new();
    if let Some(skeleton) = &skeleton {
        for (slot, entry) in joint_entries(skeleton).into_iter().enumerate() {
            let target = element_attr(&entry, "transform")
                .map(|transform| id_of(&transform))
                .unwrap_or_else(|| id_of(&entry));
            transform_slot.insert(target, slot);
        }
    }

    let mut positions: HashMap<usize, Track<[f32; 3]>> = HashMap::new();
    let mut rotations: HashMap<usize, Track<[f32; 4]>> = HashMap::new();
    let mut last_tick = 0i32;
    for channel in element_array_attr(&clip, "channels") {
        let Some(target) = element_attr(&channel, "toElement") else {
            continue;
        };
        let Some(&slot) = transform_slot.get(&id_of(&target)) else {
            continue;
        };
        let Some(log) = element_attr(&channel, "log") else {
            continue;
        };
        let Some(layer) = element_array_attr(&log, "layers").into_iter().next() else {
            continue;
        };
        let times = time_array_attr(&layer, "times");
        if let Some(tick) = times.last() {
            last_tick = last_tick.max(*tick);
        }
        match string_attr(&channel, "toAttribute").unwrap_or_default().as_str() {
            "position" => {
                let values = vector3_array_attr(&layer, "values");
                if !values.is_empty() {
                    positions.insert(slot, Track { times, values });
                }
            }
            "orientation" => {
                let values = quaternion_array_attr(&layer, "values");
                if !values.is_empty() {
                    rotations.insert(slot, Track { times, values });
                }
            }
            // 缩放等通道官方也不读，静默跳过。
            _ => {}
        }
    }

    let (nodes, node_of) = joint_node_table(model);
    if nodes.is_empty() {
        return Err(DmxError::malformed(
            label,
            "the animation file has no joints, so there is no skeleton to animate",
        ));
    }
    let frames = (last_tick as f32 / ticks_per_frame).round() as i32 + 1;
    let mut out_frames = Vec::with_capacity(frames.max(1) as usize);
    for frame in 0..frames.max(1) {
        let tick = (frame as f32 * ticks_per_frame).round() as i32;
        let mut poses = Vec::new();
        for (slot, bone) in model.bones.iter().enumerate() {
            // 没有通道的骨骼用文件里的绑定姿态（常量骨就是这样表达的）。
            let Some(&node) = node_of.get(slot).and_then(|node| node.as_ref()) else {
                continue;
            };
            let position = positions
                .get(&slot)
                .map(|track| track.sample(tick))
                .unwrap_or(bone.position);
            let orientation = rotations
                .get(&slot)
                .map(|track| track.sample(tick))
                .unwrap_or(bone.orientation);
            poses.push(SmdPose {
                bone: node,
                position,
                rotation: crate::bone_math::quaternion_angles(orientation),
            });
        }
        out_frames.push(SmdFrame {
            time: frame,
            poses,
        });
    }

    Ok(crate::smd::Smd {
        version: 1,
        nodes,
        frames: out_frames,
        triangles: Vec::new(),
    })
}

/// 一条采样曲线：键时刻（tick）+ 值。
struct Track<V> {
    times: Vec<i32>,
    values: Vec<V>,
}

impl Track<[f32; 3]> {
    /// 线性取样；只有一个键时恒定；超界时夹到首/末键。
    fn sample(&self, tick: i32) -> [f32; 3] {
        let Some((a, b, t)) = bracket(&self.times, tick) else {
            return self.values.first().copied().unwrap_or_default();
        };
        let from = self.values[a];
        let to = self.values[b];
        [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
            from[2] + (to[2] - from[2]) * t,
        ]
    }
}

impl Track<[f32; 4]> {
    /// 球面插值；只有一个键时恒定。四元数要处理同向/反向两种等价表示。
    fn sample(&self, tick: i32) -> [f32; 4] {
        let Some((a, b, t)) = bracket(&self.times, tick) else {
            return self
                .values
                .first()
                .copied()
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        };
        if a == b {
            return self.values[a];
        }
        slerp(self.values[a], self.values[b], t)
    }
}

/// 找出 `tick` 落在哪两个键之间，返回 `(左, 右, 比例)`。
fn bracket(times: &[i32], tick: i32) -> Option<(usize, usize, f32)> {
    let first = *times.first()?;
    let last_index = times.len() - 1;
    let last = times[last_index];
    if times.len() < 2 || tick <= first {
        return Some((0, 0, 0.0));
    }
    if tick >= last {
        return Some((last_index, last_index, 0.0));
    }
    for index in 1..times.len() {
        if tick <= times[index] {
            let span = (times[index] - times[index - 1]) as f32;
            let t = if span > 0.0 {
                (tick - times[index - 1]) as f32 / span
            } else {
                0.0
            };
            return Some((index - 1, index, t));
        }
    }
    Some((last_index, last_index, 0.0))
}

fn slerp(from: [f32; 4], to: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = from[0] * to[0] + from[1] * to[1] + from[2] * to[2] + from[3] * to[3];
    let mut to = to;
    if dot < 0.0 {
        to = [-to[0], -to[1], -to[2], -to[3]];
        dot = -dot;
    }
    if dot > 0.9995 {
        // 夹角太小，球面插值与线性插值数值上没区别，直接归一化更稳。
        let mixed = [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
            from[2] + (to[2] - from[2]) * t,
            from[3] + (to[3] - from[3]) * t,
        ];
        let len = (mixed[0] * mixed[0]
            + mixed[1] * mixed[1]
            + mixed[2] * mixed[2]
            + mixed[3] * mixed[3])
            .sqrt();
        if len > f32::EPSILON {
            return [
                mixed[0] / len,
                mixed[1] / len,
                mixed[2] / len,
                mixed[3] / len,
            ];
        }
        return mixed;
    }
    let theta0 = dot.clamp(-1.0, 1.0).acos();
    let theta = theta0 * t;
    let sin = theta0.sin();
    let s0 = (theta0 - theta).sin() / sin;
    let s1 = theta.sin() / sin;
    [
        from[0] * s0 + to[0] * s1,
        from[1] * s0 + to[1] * s1,
        from[2] * s0 + to[2] * s1,
        from[3] * s0 + to[3] * s1,
    ]
}

/// 骨骼槽位 → SMD 节点下标，并顺带生成 `nodes` 表。
///
/// 只留 `DmeJoint`：`jointList` 里混着网格自己的 `DmeDag`（未蒙皮顶点的落点），
/// 它不是骨骼。父骨骼可能落在被跳过的槽位上，所以顺着 `parent` 链上找最近的真骨骼。
fn joint_node_table(model: &DmxModel) -> (Vec<crate::smd::SmdNode>, Vec<Option<i32>>) {
    let mut node_of: Vec<Option<i32>> = vec![None; model.bones.len()];
    let mut nodes: Vec<crate::smd::SmdNode> = Vec::new();
    for (slot, bone) in model.bones.iter().enumerate() {
        if !bone.is_joint {
            continue;
        }
        node_of[slot] = Some(nodes.len() as i32);
        nodes.push(crate::smd::SmdNode {
            index: nodes.len() as i32,
            name: bone.name.clone(),
            parent: -1,
        });
    }
    for (slot, bone) in model.bones.iter().enumerate() {
        let Some(node) = node_of[slot] else {
            continue;
        };
        let mut cursor = bone.parent;
        let mut parent = -1;
        let mut hops = 0usize;
        while let Some(p) = cursor {
            if let Some(ix) = node_of.get(p).copied().flatten() {
                parent = ix;
                break;
            }
            cursor = model.bones[p].parent;
            hops += 1;
            if hops > model.bones.len() {
                break;
            }
        }
        nodes[node as usize].parent = parent;
    }
    (nodes, node_of)
}

/// 把 [`DmxModel`] 翻译成 [`crate::smd::Smd`]。
///
/// 三处换算都是实测定的（见 [`DmxBone::position`] / [`DmxBone::orientation`]）：
/// 平移**原样**、旋转走 [`crate::bone_math::quaternion_angles`]、节点表只留真骨骼。
fn to_smd(model: &DmxModel, at: &str, path: &str) -> Result<DmxGeometry, DmxError> {
    use crate::smd::{SmdBoneLink, SmdFrame, SmdPose, SmdTriangle, SmdVertex};

    let mut warnings = model.warnings.clone();

    let (nodes, node_of) = joint_node_table(model);
    if nodes.is_empty() {
        return Err(DmxError::malformed(
            path,
            "the model has no joints (only DAGs), so there is no skeleton to compile",
        ));
    }

    let poses: Vec<SmdPose> = model
        .bones
        .iter()
        .enumerate()
        .filter_map(|(slot, bone)| {
            node_of[slot].map(|node| SmdPose {
                bone: node,
                position: bone.position,
                rotation: crate::bone_math::quaternion_angles(bone.orientation),
            })
        })
        .collect();

    // 顶点权重里的 joint 下标也要换到节点空间；指向 DAG 的（未蒙皮顶点）落到父骨骼。
    let remap_joint = |slot: u32| -> Option<i32> {
        let slot = usize::try_from(slot).ok()?;
        if let Some(node) = node_of.get(slot).copied().flatten() {
            return Some(node);
        }
        // 指向 DAG 槽位：顺着 `parent` 链找最近的真骨骼（BST 用它表示「没有蒙皮」）。
        let mut cursor = model.bones.get(slot)?.parent;
        let mut hops = 0usize;
        while let Some(p) = cursor {
            if let Some(node) = node_of.get(p).copied().flatten() {
                return Some(node);
            }
            cursor = model.bones.get(p)?.parent;
            hops += 1;
            if hops > model.bones.len() {
                break;
            }
        }
        None
    };

    let mut triangles: Vec<SmdTriangle> = Vec::new();
    let mut shape_keys: Vec<crate::fbx::FbxShapeKey> = Vec::new();
    let mut src_index: u32 = 0;
    let mut merged: Vec<String> = Vec::new();
    for mesh in &model.meshes {
        if mesh.triangles.is_empty() {
            continue;
        }
        merged.push(mesh.name.clone());
        let base = src_index;
        src_index += mesh.positions.len() as u32;
        // 形态键：**名字取控制器、数据取 delta**（下标配对），理由见 [`DmxDelta`]。
        for (index, delta) in mesh.deltas.iter().enumerate() {
            let name = model
                .controls
                .get(index)
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| delta.name.clone());
            if name.is_empty() {
                warnings.push(format!(
                    "the mesh {mesh:?} has a delta state with no name, skipped",
                    mesh = mesh.name
                ));
                continue;
            }
            if !delta.name.is_empty() && delta.name != name {
                // 真实夹具里会撞上（BST 的 `name` 与控制器的顺序对不上）—— 说出来，
                // 免得用户对着一个方向不对的形态键查半天。
                warnings.push(format!(
                    "delta state {index} is named {delta:?} but its controller is {name:?}; mdlc pairs them by index",
                    delta = delta.name
                ));
            }
            shape_keys.push(crate::fbx::FbxShapeKey {
                name,
                position_offsets: delta.positions.clone(),
                // 官方在 FBX 路径上同样丢弃法线增量（见 [`collect_deltas`]）。
                normal_offsets: vec![[0.0; 3]; delta.positions.len()],
                vertex_index: delta.position_indices.iter().map(|&i| base + i).collect(),
            });
        }
        for tri in &mesh.triangles {
            let material = tri
                .material
                .and_then(|i| model.materials.get(i))
                .map(|m| m.material.clone())
                .unwrap_or_default();
            let mut vs: Vec<SmdVertex> = Vec::with_capacity(3);
            for corner in &tri.corners {
                let position = mesh
                    .positions
                    .get(corner.position as usize)
                    .copied()
                    .unwrap_or([0.0; 3]);
                let mut links: Vec<SmdBoneLink> = mesh
                    .weights
                    .get(corner.position as usize)
                    .map(|chain| {
                        chain
                            .iter()
                            .filter_map(|&(joint, weight)| {
                                remap_joint(joint).map(|bone| SmdBoneLink { bone, weight })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // ⚠️ 空 `links` 不是「无绑定」：官方把这种顶点绑到它所在的网格节点上
                // （SMD 解析器对 `links == 0` 也是这么兜底的，见
                // `docs/_probe/oracle_links_zero.js`）。DMX 路径不经过那个解析器，
                // 所以这里必须补，否则收骨判据看不到任何顶点引用。
                let parent_bone = links
                    .iter()
                    .max_by(|a, b| {
                        a.weight
                            .partial_cmp(&b.weight)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|l| l.bone)
                    .unwrap_or(0);
                if links.is_empty() {
                    links.push(SmdBoneLink {
                        bone: parent_bone,
                        weight: 1.0,
                    });
                }
                vs.push(SmdVertex {
                    parent_bone,
                    position,
                    normal: corner.normal,
                    uv: corner.uv,
                    links,
                    src_index: base + corner.position,
                });
            }
            triangles.push(SmdTriangle {
                material,
                vertices: [vs[0].clone(), vs[1].clone(), vs[2].clone()],
            });
        }
    }

    // ⚠️ 这里**不**因为「一个三角形都没有」报错：BST 的动画文件
    // （`anims/<骨架>.dmx`）只有骨架与通道日志，没有网格，而 QC 预解析
    // 会把它当普通源打开一次（只取节点名）。真正需要几何的入口是
    // [`read_geometry`]，那里才拦。
    if triangles.is_empty() && !model.meshes.is_empty() {
        warnings.push("the model has meshes but none of them produced a triangle".to_owned());
    }
    if model.meshes.len() > 1 {
        // 多个网格合并成一个 SMD：与 FBX 路径同一套口径（按顺序拼起来）。
        warnings.push(format!(
            "{} meshes were merged into one SMD: {}",
            merged.len(),
            merged.join(", ")
        ));
    }
    let _ = at;

    Ok(DmxGeometry {
        smd: crate::smd::Smd {
            version: 1,
            nodes,
            frames: vec![SmdFrame { time: 0, poses }],
            triangles,
        },
        shape_keys,
        warnings,
    })
}

// ---------------------------------------------------------------- 遍历

fn class_is(el: &Element, class: &str) -> bool {
    *el.get_class() == class
}

fn id_of(el: &Element) -> String {
    el.get_id().to_string()
}

/// 深度优先摊平，按元素 id 去重。
fn walk(el: &Element, out: &mut Vec<Element>, seen: &mut HashSet<String>) {
    if !seen.insert(id_of(el)) {
        return;
    }
    out.push(el.clone());
    for child in child_elements(el) {
        walk(&child, out, seen);
    }
}

/// 一个元素直接挂着的子元素（`element` 与 `element_array` 两种属性都算）。
fn child_elements(el: &Element) -> Vec<Element> {
    let mut out = Vec::new();
    let attrs = el.get_attributes();
    for attr in attrs.values() {
        match attr.get_inner_value() {
            AttributeValue::Element(Some(child)) => out.push(child.clone()),
            AttributeValue::ElementArray(items) => {
                out.extend(items.iter().flatten().cloned());
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------- 材质

fn collect_materials(flat: &[Element]) -> Vec<DmxMaterial> {
    let mut out: Vec<DmxMaterial> = Vec::new();
    let mut seen = HashSet::new();
    for el in flat {
        if !class_is(el, "DmeMaterial") {
            continue;
        }
        let name = string_attr(el, "name").unwrap_or_default();
        let material = string_attr(el, "mtlName").unwrap_or_else(|| name.clone());
        if !seen.insert(material.clone()) {
            continue;
        }
        out.push(DmxMaterial { name, material });
    }
    out
}

// ---------------------------------------------------------------- 骨骼

fn collect_bones(model: &Element) -> Vec<DmxBone> {
    let joints = joint_entries(model);

    // `jointList` 只给顺序（`jointIndices` 就是按它索引的），层级要靠 `children`。
    let mut bones: Vec<DmxBone> = joints
        .iter()
        .map(|joint| {
            let (position, orientation) = transform_of(joint);
            DmxBone {
                name: string_attr(joint, "name").unwrap_or_default(),
                parent: None,
                position,
                orientation,
                // format ≥ 2 的槽位元素本身就是 `DmeJoint` / `DmeDag`；
                // format 1 的槽位是 `DmeTransform`（真身要看 `children` 里挂的谁，
                // 由下面的遍历覆盖），先按「是关节」处理。
                is_joint: !class_is(joint, "DmeDag"),
            }
        })
        .collect();

    // 索引空间按「成员自己的 id」与「它指向的 `DmeTransform` 的 id」双键登记：
    // format ≥ 2 的成员是 `DmeJoint`（走前者），format 1 的成员是 `DmeTransform`
    // （走后者，因为 DAG 里挂的是 `DmeJoint`，它通过 `transform` 属性指向同一个元素）。
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, joint) in joints.iter().enumerate() {
        index.insert(id_of(joint), i);
        if let Some(transform) = element_attr(joint, "transform") {
            index.entry(id_of(&transform)).or_insert(i);
        }
    }

    // ⚠️ BST 把子骨骼挂在**父骨骼元素自己的 `children` 上**（`DmeJoint` 在 Source
    // 的 schema 里本该没有这个属性），而不是 `DmeDag.shape` 那一套。按 DAG 走会得到
    // 「全部都是根骨骼」—— 实测 `bone_a` / `bone_b` 都挂在 `root` 的 `children` 里。
    // 另外 `jointList` 里可能混着**网格自己的 `DmeDag`**（BST 会给它留一个槽位，
    // 未被蒙皮的顶点就指向它），所以这里不假设「jointList 里全是 DmeJoint」。
    let mut stack: Vec<(Element, Option<usize>)> = element_array_attr(model, "children")
        .into_iter()
        .map(|child| (child, None))
        .collect();
    let mut seen = HashSet::new();
    while let Some((element, parent)) = stack.pop() {
        if !seen.insert(id_of(&element)) {
            continue;
        }
        let slot = index.get(&id_of(&element)).copied().or_else(|| {
            element_attr(&element, "transform")
                .and_then(|transform| index.get(&id_of(&transform)).copied())
        });
        let here = match slot {
            Some(i) => {
                bones[i].parent = parent;
                // 槽位的真身（format 1 里 `jointList` 存的是 `DmeTransform`，
                // 真身是挂在 `children` 上的 `DmeJoint` / `DmeDag`）。
                if class_is(&element, "DmeDag") {
                    bones[i].is_joint = false;
                } else if class_is(&element, "DmeJoint") {
                    bones[i].is_joint = true;
                }
                Some(i)
            }
            // 不在索引空间里的中间层（例如 `DmeTransformList`）不打断层级。
            None => parent,
        };
        for child in element_array_attr(&element, "children") {
            stack.push((child, here));
        }
    }
    bones
}

/// 取元素的 `transform` 子元素里的 position / orientation。
///
/// 两种形态都要吃：`DmeJoint` / `DmeDag` 是通过 `transform` 属性指向一个
/// `DmeTransform`；而 `jointTransforms` 里的元素**本身就是** `DmeTransform`，
/// position / orientation 是它自己的属性。
fn transform_of(el: &Element) -> ([f32; 3], [f32; 4]) {
    let owner = match element_attr(el, "transform") {
        Some(transform) => transform,
        None => el.clone(),
    };
    let position = vector3_attr(&owner, "position").unwrap_or([0.0; 3]);
    let orientation = quaternion_attr(&owner, "orientation").unwrap_or([0.0, 0.0, 0.0, 1.0]);
    (position, orientation)
}

/// 骨骼索引空间的成员。
///
/// format ≥ 2 有 `jointList`（`DmeJoint` 或网格自己的 `DmeDag`）；format 1 及更早
/// **没有** `jointList`，索引空间是 `jointTransforms` 的顺序（元素是 `DmeTransform`）。
fn joint_entries(model: &Element) -> Vec<Element> {
    let list = element_array_attr(model, "jointList");
    if !list.is_empty() {
        return list;
    }
    element_array_attr(model, "jointTransforms")
}

// ---------------------------------------------------------------- 网格

fn collect_meshes(
    flat: &[Element],
    materials: &[DmxMaterial],
    mesh_transforms: &HashMap<String, ([f32; 3], [f32; 4])>,
    warnings: &mut Vec<String>,
) -> Vec<DmxMesh> {
    let material_index: HashMap<String, usize> = materials
        .iter()
        .enumerate()
        .map(|(i, m)| (m.material.clone(), i))
        .collect();

    let mut out = Vec::new();
    for mesh in flat.iter().filter(|e| class_is(e, "DmeMesh")) {
        let name = string_attr(mesh, "name").unwrap_or_default();
        let Some(vd) = element_attr(mesh, "bindState").or_else(|| element_attr(mesh, "currentState"))
        else {
            warnings.push(format!("mesh {name:?} has no bindState, skipped"));
            continue;
        };

        let positions = vector3_array_attr(&vd, "positions");
        if positions.is_empty() {
            warnings.push(format!("mesh {name:?} has no positions, skipped"));
            continue;
        }
        let position_indices = i32_array_attr(&vd, "positionsIndices");

        // 网格顶点的空间是「网格自己」的，烘上它的世界变换才是骨架空间（见
        // [`collect_mesh_transforms`]）。
        let (offset, orientation) = mesh_transforms
            .get(&id_of(mesh))
            .copied()
            .unwrap_or(([0.0; 3], [0.0, 0.0, 0.0, 1.0]));
        let positions: Vec<[f32; 3]> = positions
            .iter()
            .map(|p| add3(offset, rotate(orientation, *p)))
            .collect();

        // 法线 / UV 是 loop 空间的（见 [`DmxMesh`] 的文档），交给逐角解析。
        let loops = LoopAttrs {
            position_indices: &position_indices,
            normals: &vector3_array_attr(&vd, "normals"),
            normal_indices: &i32_array_attr(&vd, "normalsIndices"),
            uvs: &vector2_array_attr(&vd, "textureCoordinates"),
            uv_indices: &i32_array_attr(&vd, "textureCoordinatesIndices"),
            // ⚠️ `flipVCoordinates` 的方向是实测定的，不是查文档定的：BST 在 L4D2
            // 预设下写 `true`，而它导出的 SMD 里 UV 是 (0.375, 0.000) —— 同一份
            // 场景的 DMX 里 V 存的是 `1 - v`，所以这里要翻回来。
            // `docs/_probe/cmp_dmx_vs_smd.js` 与
            // `read_matches_the_smd_export_of_the_same_scene` 会盯住它。
            flip_v: bool_attr(&vd, "flipVCoordinates").unwrap_or(false),
            position_count: positions.len(),
            orientation,
        };

        let weights = expand_weights(&vd, positions.len(), &name, warnings);

        let triangles = collect_faces(mesh, &loops, &material_index, &name, warnings);

        out.push(DmxMesh {
            name,
            positions,
            weights,
            triangles,
            deltas: collect_deltas(mesh, orientation),
        });
    }
    out
}

/// 读一个网格的形态键（`DmeMesh.deltaStates`）。
///
/// 只取位置增量：`normals` 是 **loop 空间**的（`normalsIndices` 指向角，不是控制点），
/// 而下游的 [`crate::fbx::FbxShapeKey`] 按控制点存，两者对不上；官方在 FBX 路径上
/// 也把这些法线增量整个丢掉。
///
/// 增量要跟着网格的**旋转**走（不能加平移：增量是相对量），否则多网格合并之后
/// 形态会朝着错误的方向变形。
fn collect_deltas(mesh: &Element, orientation: [f32; 4]) -> Vec<DmxDelta> {
    element_array_attr(mesh, "deltaStates")
        .iter()
        .map(|delta| {
            let mut positions = Vec::new();
            let mut position_indices = Vec::new();
            let values = vector3_array_attr(delta, "positions");
            for (index, position) in i32_array_attr(delta, "positionsIndices").iter().zip(values) {
                if let Ok(index) = u32::try_from(*index) {
                    position_indices.push(index);
                    positions.push(rotate(orientation, position));
                }
            }
            DmxDelta {
                name: string_attr(delta, "name").unwrap_or_default(),
                positions,
                position_indices,
            }
        })
        .collect()
}

/// loop 空间的属性表：给一个 loop 下标，取出这个角自己的位置 / 法线 / UV。
struct LoopAttrs<'a> {
    position_indices: &'a [i32],
    normals: &'a [[f32; 3]],
    normal_indices: &'a [i32],
    uvs: &'a [[f32; 2]],
    uv_indices: &'a [i32],
    flip_v: bool,
    position_count: usize,
    /// 网格的世界朝向；法线跟顶点位置一样要转过去。
    orientation: [f32; 4],
}

impl LoopAttrs<'_> {
    fn corner(&self, loop_index: usize) -> Option<DmxCorner> {
        let position = *self.position_indices.get(loop_index)?;
        let position = u32::try_from(position).ok()?;
        if position as usize >= self.position_count {
            return None;
        }
        let normal = self
            .normal_indices
            .get(loop_index)
            .and_then(|&i| usize::try_from(i).ok())
            .and_then(|i| self.normals.get(i).copied())
            .map(|n| rotate(self.orientation, n))
            .unwrap_or([0.0, 0.0, 0.0]);
        let uv = self
            .uv_indices
            .get(loop_index)
            .and_then(|&i| usize::try_from(i).ok())
            .and_then(|i| self.uvs.get(i).copied())
            .map(|uv| {
                if self.flip_v {
                    [uv[0], 1.0 - uv[1]]
                } else {
                    uv
                }
            })
            .unwrap_or([0.0, 0.0]);
        Some(DmxCorner {
            position,
            normal,
            uv,
        })
    }
}

/// 权重是 position 空间的：每顶点 `jointCount` 条，`jointWeights` 与 `jointIndices` 平行。
fn expand_weights(
    vd: &Element,
    position_count: usize,
    mesh_name: &str,
    warnings: &mut Vec<String>,
) -> Vec<Vec<(u32, f32)>> {
    let weights = f32_array_attr(vd, "jointWeights");
    let indices = i32_array_attr(vd, "jointIndices");
    let mut out = vec![Vec::new(); position_count];
    if weights.is_empty() || indices.is_empty() {
        return out;
    }

    // `jointCount` 是权威口径；缺了就按数组长度反推，再对不上就当整块没有权重。
    let declared = int_attr(vd, "jointCount").and_then(|n| usize::try_from(n).ok());
    let per_vertex = declared.unwrap_or_else(|| weights.len().checked_div(position_count).unwrap_or(0));
    if per_vertex == 0 || weights.len() != position_count * per_vertex || indices.len() != weights.len()
    {
        warnings.push(format!(
            "mesh {mesh_name:?} has an inconsistent joint weight block ({} weights, {} indices, {} vertices, jointCount {:?}), weights dropped",
            weights.len(),
            indices.len(),
            position_count,
            declared
        ));
        return out;
    }

    for (vertex, slot) in out.iter_mut().enumerate() {
        let base = vertex * per_vertex;
        for k in 0..per_vertex {
            let weight = weights[base + k];
            if weight <= 0.0 {
                continue;
            }
            let Ok(joint) = u32::try_from(indices[base + k]) else {
                continue;
            };
            slot.push((joint, weight));
        }
    }
    out
}

/// 读所有 face set 的三角形。材质是**逐 face set** 给的，所以逐三角形记住。
fn collect_faces(
    mesh: &Element,
    loops: &LoopAttrs<'_>,
    material_index: &HashMap<String, usize>,
    mesh_name: &str,
    warnings: &mut Vec<String>,
) -> Vec<DmxTriangle> {
    let mut triangles: Vec<DmxTriangle> = Vec::new();

    for face_set in element_array_attr(mesh, "faceSets") {
        let material = element_attr(&face_set, "material")
            .and_then(|m| string_attr(&m, "mtlName"))
            .and_then(|name| material_index.get(&name).copied());
        let faces = i32_array_attr(&face_set, "faces");
        // ⚠️ `faces` 不是三角形三元组，而是「若干个 loop 下标 + `-1` 结束符」的多边形表。
        // BST 导出的立方体是 6 个四边形（`[0,1,2,3,-1, 4,5,6,7,-1, …]`），
        // 按三元组硬切会得到 `(0,1,2) (3,-1,4) …` 这种垃圾。官方 `dmxconvert`
        // 也是先按 `-1` 切多边形再三角化。
        let mut polygon: Vec<usize> = Vec::new();
        let mut dropped = 0usize;
        for &raw in &faces {
            if raw < 0 {
                triangulate(&mut polygon, &mut triangles, loops, material);
                continue;
            }
            let corner = usize::try_from(raw).ok().and_then(|i| loops.corner(i));
            match corner {
                Some(_) => polygon.push(usize::try_from(raw).unwrap_or(0)),
                None => {
                    polygon.clear();
                    dropped += 1;
                }
            }
        }
        triangulate(&mut polygon, &mut triangles, loops, material);
        if dropped > 0 {
            warnings.push(format!(
                "mesh {mesh_name:?}: {dropped} face indices point outside the index space, those polygons were dropped"
            ));
        }
    }

    triangles
}

/// 扇形三角化一个多边形（顶点顺序保持原样，绕序不变），然后清空它。
fn triangulate(
    polygon: &mut Vec<usize>,
    triangles: &mut Vec<DmxTriangle>,
    loops: &LoopAttrs<'_>,
    material: Option<usize>,
) {
    if polygon.len() >= 3 {
        let first = loops.corner(polygon[0]);
        if let Some(first) = first {
            for k in 1..polygon.len() - 1 {
                let b = loops.corner(polygon[k]);
                let c = loops.corner(polygon[k + 1]);
                if let (Some(b), Some(c)) = (b, c) {
                    triangles.push(DmxTriangle {
                        material,
                        corners: [first, b, c],
                    });
                }
            }
        }
    }
    polygon.clear();
}

// ---------------------------------------------------------------- 属性取值

fn element_attr(el: &Element, name: &str) -> Option<Element> {
    let attr = el.get_attribute(name)?;
    match attr.get_inner_value() {
        AttributeValue::Element(child) => child.clone(),
        _ => None,
    }
}

fn element_array_attr(el: &Element, name: &str) -> Vec<Element> {
    let Some(attr) = el.get_attribute(name) else {
        return Vec::new();
    };
    match attr.get_inner_value() {
        AttributeValue::ElementArray(items) => items.iter().flatten().cloned().collect(),
        AttributeValue::Element(child) => child.iter().cloned().collect(),
        _ => Vec::new(),
    }
}

fn string_attr(el: &Element, name: &str) -> Option<String> {
    let attr = el.get_attribute(name)?;
    match attr.get_inner_value() {
        AttributeValue::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn string_array_attr(el: &Element, name: &str) -> Vec<String> {
    let Some(attr) = el.get_attribute(name) else {
        return Vec::new();
    };
    match attr.get_inner_value() {
        AttributeValue::StringArray(values) => values.clone(),
        _ => Vec::new(),
    }
}

fn int_attr(el: &Element, name: &str) -> Option<i32> {
    let attr = el.get_attribute(name)?;
    match attr.get_inner_value() {
        AttributeValue::Integer(value) => Some(*value),
        _ => None,
    }
}

fn bool_attr(el: &Element, name: &str) -> Option<bool> {
    let attr = el.get_attribute(name)?;
    match attr.get_inner_value() {
        AttributeValue::Boolean(value) => Some(*value),
        _ => None,
    }
}

fn f32_array_attr(el: &Element, name: &str) -> Vec<f32> {
    let Some(attr) = el.get_attribute(name) else {
        return Vec::new();
    };
    match attr.get_inner_value() {
        AttributeValue::FloatArray(values) => values.clone(),
        _ => Vec::new(),
    }
}

fn i32_array_attr(el: &Element, name: &str) -> Vec<i32> {
    let Some(attr) = el.get_attribute(name) else {
        return Vec::new();
    };
    match attr.get_inner_value() {
        AttributeValue::IntegerArray(values) => values.clone(),
        _ => Vec::new(),
    }
}

fn vector3_array_attr(el: &Element, name: &str) -> Vec<[f32; 3]> {
    let Some(attr) = el.get_attribute(name) else {
        return Vec::new();
    };
    match attr.get_inner_value() {
        AttributeValue::Vector3Array(values) => values.iter().map(|v| [v.x, v.y, v.z]).collect(),
        _ => Vec::new(),
    }
}

fn vector2_array_attr(el: &Element, name: &str) -> Vec<[f32; 2]> {
    let Some(attr) = el.get_attribute(name) else {
        return Vec::new();
    };
    match attr.get_inner_value() {
        AttributeValue::Vector2Array(values) => values.iter().map(|v| [v.x, v.y]).collect(),
        _ => Vec::new(),
    }
}

fn vector3_attr(el: &Element, name: &str) -> Option<[f32; 3]> {
    let attr = el.get_attribute(name)?;
    match attr.get_inner_value() {
        AttributeValue::Vector3(v) => Some([v.x, v.y, v.z]),
        _ => None,
    }
}

fn quaternion_attr(el: &Element, name: &str) -> Option<[f32; 4]> {
    let attr = el.get_attribute(name)?;
    match attr.get_inner_value() {
        AttributeValue::Quaternion(q) => Some([q.x, q.y, q.z, q.w]),
        _ => None,
    }
}

fn quaternion_array_attr(el: &Element, name: &str) -> Vec<[f32; 4]> {
    let Some(attr) = el.get_attribute(name) else {
        return Vec::new();
    };
    match attr.get_inner_value() {
        AttributeValue::QuaternionArray(values) => {
            values.iter().map(|q| [q.x, q.y, q.z, q.w]).collect()
        }
        _ => Vec::new(),
    }
}

/// `TimeArray`（时间戳，单位是 1/10000 秒的 tick）。
fn time_array_attr(el: &Element, name: &str) -> Vec<i32> {
    let Some(attr) = el.get_attribute(name) else {
        return Vec::new();
    };
    match attr.get_inner_value() {
        AttributeValue::TimeArray(values) => values.iter().map(|t| t.0).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// BlenderSourceTools 用 L4D2 预设导出的夹具：一个立方体 + 3 根骨骼。
    /// binary 4 / format 15；`faces` 是「顶点下标 + `-1` 结束符」的多边形表，
    /// `jointList` 里还混着网格自己的 `DmeDag`。
    const BST_L4D2: &[u8] = include_bytes!("dmx_fixtures/bst_l4d2.dmx");

    #[test]
    fn reads_a_blender_source_tools_export() {
        let model = read_from(&mut Cursor::new(BST_L4D2), "bst_l4d2.dmx").expect("应当能解析");
        assert_eq!(model.format_version, 15);

        // jointList 有 5 个槽位，但只有 3 个是真骨骼（另外两个是 blender_implicit
        // 与网格自己的 DmeDag）—— 全部保留，因为权重里的下标就是这个顺序。
        let names: Vec<&str> = model.bones.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            ["blender_implicit", "root", "bone_a", "bone_b", "probe_mesh"]
        );

        // 层级：root 是根，bone_a 挂在 root 下，bone_b 挂在 bone_a 下。
        assert_eq!(model.bones[1].parent, None);
        assert_eq!(model.bones[2].parent, Some(1));
        assert_eq!(model.bones[3].parent, Some(2));

        assert_eq!(model.materials.len(), 1);
        assert_eq!(model.materials[0].material, "no_material");

        assert_eq!(model.meshes.len(), 1);
        let mesh = &model.meshes[0];
        assert_eq!(mesh.positions.len(), 8);
        // 立方体是 6 个四边形 —— 只有按 -1 切开再扇形三角化才得到 12 个三角形。
        assert_eq!(mesh.triangles.len(), 12);
        // 法线 / UV 是逐角存的：8 个 position 配 24 个角，每个面的角各取一套。
        assert!(
            mesh.triangles.iter().all(|t| t.material == Some(0)),
            "每个三角形都该带上材质"
        );
        assert!(
            mesh.triangles
                .iter()
                .flat_map(|t| t.corners.iter())
                .all(|c| (c.position as usize) < mesh.positions.len()),
            "角的位置下标必须落在 position 表里"
        );
        assert!(
            mesh.weights.iter().all(|chain| chain.len() == 1),
            "每个顶点应当只有一条权重链"
        );
    }

    #[test]
    fn rejects_something_that_is_not_a_datamodel() {
        let err =
            read_from(&mut Cursor::new(b"not a dmx".as_slice()), "x.dmx").expect_err("应当报错");
        assert!(matches!(err, DmxError::Parse { .. }), "实际得到 {err:?}");
    }

    /// 同一份 Blender 场景的 SMD 与 DMX 双导出（3 根骨骼、两材质立方体、
    /// 单/双权重混合）。
    const BLENDER_CUBE_DMX: &[u8] = include_bytes!("dmx_fixtures/blender_cube.dmx");
    const BLENDER_CUBE_SMD: &str = include_str!("dmx_fixtures/blender_cube.smd");

    /// **验收主判据**：DMX 读出来的东西，必须与同一份场景导出的 SMD 一模一样。
    ///
    /// 这一条比「跑通不 panic」硬得多：节点表、骨骼姿态、每个角的
    /// 位置/法线/UV/权重链、逐三角形材质，全部逐项比对。它同时盯住三处换算
    /// （位置原样、旋转走 `quaternion_angles`、节点表只留真骨骼）与 V 翻转。
    #[test]
    fn read_matches_the_smd_export_of_the_same_scene() {
        let model =
            read_from(&mut Cursor::new(BLENDER_CUBE_DMX), "blender_cube.dmx").expect("应当能解析");
        let ours = to_smd(&model, "blender_cube.dmx", "blender_cube.dmx")
            .expect("应当能转成 SMD")
            .smd;
        let theirs = crate::smd::parse_smd(BLENDER_CUBE_SMD).expect("SMD 夹具应当能解析");

        // ---- 节点表 ----
        assert_eq!(ours.nodes.len(), theirs.nodes.len(), "node 数");
        for (a, b) in ours.nodes.iter().zip(&theirs.nodes) {
            assert_eq!(a.name, b.name, "node {} 名字", a.index);
            assert_eq!(a.parent, b.parent, "node {:?} 的父节点", a.name);
        }

        // ---- 参考姿态 ----
        let (a, b) = (&ours.frames[0].poses, &theirs.frames[0].poses);
        assert_eq!(a.len(), b.len(), "pose 数");
        for (x, y) in a.iter().zip(b) {
            for k in 0..3 {
                assert!(
                    (x.position[k] - y.position[k]).abs() < 1e-4,
                    "骨骼 {} 位置分量 {k}：DMX {:?} vs SMD {:?}",
                    x.bone,
                    x.position,
                    y.position
                );
            }
            // 旋转按四元数比会绕开欧拉角的分支问题，但两边都是欧拉角，直接逐分量
            // 比即可（SMD 夹具是同一台机器导出的，不会跨分支）。
            for k in 0..3 {
                assert!(
                    (x.rotation[k] - y.rotation[k]).abs() < 1e-4,
                    "骨骼 {} 旋转分量 {k}：DMX {:?} vs SMD {:?}",
                    x.bone,
                    x.rotation,
                    y.rotation
                );
            }
        }

        // ---- 三角形：材质 + 每个角 ----
        //
        // ⚠️ **按集合比，不按下标比**。两个导出器给三角形的**顺序**不一样（SMD 按
        // 多边形写、材质的「分块」也不同：实测 SMD 是 aaa/bbb/aaa/bbb，DMX 是
        // aaaaaa/bbbbbb），而且四边形三角化的对角线也可以不同。三角形顺序对产物
        // 没有影响（`compile.rs` 会重新焊接顶点、排序），所以这里比「同一批三角形」。
        //
        // 每个三角形做一次**循环旋转**（从字典序最小的角开始），这样既能忽略起点，
        // 又能保留绕序 —— 绕序反了会被抓出来。
        fn key(smd: &crate::smd::Smd) -> Vec<String> {
            let mut out: Vec<String> = smd
                .triangles
                .iter()
                .map(|t| {
                    let corners: Vec<String> = t
                        .vertices
                        .iter()
                        .map(|v| {
                            let links: Vec<String> = v
                                .links
                                .iter()
                                .map(|l| format!("{}:{:.5}", l.bone, l.weight))
                                .collect();
                            format!(
                                "({:.4},{:.4},{:.4})|({:.4},{:.4},{:.4})|({:.4},{:.4})|[{}]",
                                v.position[0],
                                v.position[1],
                                v.position[2],
                                v.normal[0],
                                v.normal[1],
                                v.normal[2],
                                v.uv[0],
                                v.uv[1],
                                links.join(",")
                            )
                        })
                        .collect();
                    // 旋转到字典序最小的角开头（保留循环顺序 ⟹ 保留绕序）。
                    let start = (0..3).min_by_key(|&i| corners[i].clone()).unwrap_or(0);
                    let rotated: Vec<String> = (0..3).map(|i| corners[(start + i) % 3].clone()).collect();
                    format!("{}#{}", t.material, rotated.join(" "))
                })
                .collect();
            out.sort();
            out
        }

        let ours_key = key(&ours);
        let theirs_key = key(&theirs);
        assert_eq!(ours_key.len(), theirs_key.len(), "三角形数");
        for (i, (a, b)) in ours_key.iter().zip(&theirs_key).enumerate() {
            assert_eq!(a, b, "第 {i} 个三角形（按排序后比）不一致");
        }
    }

    /// 同一场景的**动画** DMX（BST 写在 `anims/<骨架>.dmx` 里）。
    const BST_ANIM: &[u8] = include_bytes!("dmx_fixtures/blender_anim.dmx");
    /// 同一场景的动画 SMD（BST 的 SMD 导出），用来当对照真值。
    const BST_ANIM_SMD: &str = include_str!("dmx_fixtures/blender_anim.smd");
    /// 带形态键（`deltaStates` + `DmeCombinationOperator`）的模型。
    const BST_SHAPES: &[u8] = include_bytes!("dmx_fixtures/blender_shapes.dmx");

    #[test]
    fn reads_blender_source_tools_animation() {
        let mut reader = Cursor::new(BST_ANIM);
        let model = read_from(&mut reader, "blender_anim.dmx").expect("应当能解析");
        assert!(model.has_animation, "这份夹具带 DmeLog");

        let smd = read_animation(
            &mut Cursor::new(BST_ANIM),
            "blender_anim.dmx",
            &model,
        )
        .expect("应当能读出动画");

        // 同一场景的 SMD 导出：20 帧（`time 0 .. 19`），4 个节点。
        let truth = crate::smd::parse_smd(BST_ANIM_SMD).expect("对照夹具应当能解析");
        assert_eq!(smd.nodes.len(), truth.nodes.len(), "节点数");
        for (ours, theirs) in smd.nodes.iter().zip(&truth.nodes) {
            assert_eq!(ours.name, theirs.name, "节点名");
            assert_eq!(ours.parent, theirs.parent, "节点 {}", ours.name);
        }
        assert_eq!(smd.frames.len(), truth.frames.len(), "帧数");
        assert!(smd.frames.len() > 1, "这份夹具是 20 帧的动画");

        // 逐帧逐骨骼比姿态：DMX 的四元数走 `quaternion_angles`，SMD 里存的也是
        // 欧拉角，所以两边可以直接比（实测差在 1e-5 量级，SMD 只写 6 位小数）。
        for (frame, (ours, theirs)) in smd.frames.iter().zip(&truth.frames).enumerate() {
            for (a, b) in ours.poses.iter().zip(&theirs.poses) {
                assert_eq!(a.bone, b.bone, "第 {frame} 帧的骨骼下标");
                for axis in 0..3 {
                    let (x, y) = (a.position[axis], b.position[axis]);
                    assert!(
                        (x - y).abs() < 1e-3,
                        "第 {frame} 帧骨骼 {} 的位移第 {axis} 轴：DMX {x} vs SMD {y}",
                        a.bone
                    );
                    let (x, y) = (a.rotation[axis], b.rotation[axis]);
                    assert!(
                        (x - y).abs() < 1e-3,
                        "第 {frame} 帧骨骼 {} 的旋转第 {axis} 轴：DMX {x} vs SMD {y}",
                        a.bone
                    );
                }
            }
        }
    }

    #[test]
    fn reads_delta_states_as_shape_keys() {
        let model =
            read_from(&mut Cursor::new(BST_SHAPES), "blender_shapes.dmx").expect("应当能解析");
        assert_eq!(model.controls, ["wide", "tall"], "控制器顺序");
        let mesh = &model.meshes[0];
        assert_eq!(mesh.deltas.len(), 2, "两个 delta state");

        let geometry = to_smd(&model, "test", "blender_shapes.dmx").expect("应当能转成 SMD");
        assert_eq!(geometry.shape_keys.len(), 2, "两个形态键");
        // 名字取控制器，数据取下标的 delta state —— 两者在这份夹具里对不上
        // （`deltaStates[0].name` 是 "wide" 但带着另一条的位移），所以这里钉住
        // 「按控制器命名」这条口径。
        let names: Vec<&str> = geometry
            .shape_keys
            .iter()
            .map(|key| key.name.as_str())
            .collect();
        assert_eq!(names, ["wide", "tall"]);

        for key in &geometry.shape_keys {
            assert_eq!(
                key.position_offsets.len(),
                key.vertex_index.len(),
                "{} 的增量与下标必须平行",
                key.name
            );
            assert_eq!(key.position_offsets.len(), 8, "立方体 8 个控制点");
            assert!(key.vertex_index.iter().all(|&i| i < 8), "下标落在控制点范围内");
            assert_eq!(key.normal_offsets.len(), key.position_offsets.len());
        }
        // 两条形态键的位移方向必须不同（一条竖直、一条横向），否则说明配对错了。
        assert_ne!(
            geometry.shape_keys[0].position_offsets, geometry.shape_keys[1].position_offsets
        );
        assert!(
            geometry.warnings.is_empty(),
            "这份夹具的名字与控制器一致，不该有警告：{:?}",
            geometry.warnings
        );
    }
}
