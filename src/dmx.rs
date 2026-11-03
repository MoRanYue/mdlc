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
    pub position: [f32; 3],
    /// 相对父骨骼的旋转，`[x, y, z, w]`。
    pub orientation: [f32; 4],
}

/// 一个材质。
#[derive(Debug, Clone, Default)]
pub struct DmxMaterial {
    /// 元素名（`DmeMaterial.name`）。
    pub name: String,
    /// 真正写进 MDL 的材质名（`DmeMaterial.mtlName`），形如 `materials/models/x/y.vmt`。
    pub material: String,
}

/// 一个网格。顶点属性按 **position 空间** 存放（DMX 本身是按 loop 空间存的，
/// 这里已经按下标表摊平）。
#[derive(Debug, Clone, Default)]
pub struct DmxMesh {
    pub name: String,
    /// 材质在 [`DmxModel::materials`] 里的下标。多材质网格不猜，留 `None`。
    pub material: Option<usize>,
    /// 顶点位置。
    pub positions: Vec<[f32; 3]>,
    /// 每顶点的法线；DMX 里缺失时为空。
    pub normals: Vec<[f32; 3]>,
    /// 每顶点的 UV；DMX 里缺失时为空。
    pub uvs: Vec<[f32; 2]>,
    /// 每顶点的权重链：`(jointList 下标, 权重)`，权重为 0 的已剔除。
    pub weights: Vec<Vec<(u32, f32)>>,
    /// 三角形，元素是 [`DmxMesh::positions`] 的下标。
    pub triangles: Vec<[u32; 3]>,
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
        ..Default::default()
    };

    out.materials = collect_materials(&flat);
    out.bones = collect_bones(model);
    out.meshes = collect_meshes(&flat, &out.materials, &mut out.warnings);
    Ok(out)
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

        // 法线 / UV 在 DMX 里是 loop 空间的，按「第一个用到该顶点的 loop」摊平。
        let normals = expand_loops(
            &vector3_array_attr(&vd, "normals"),
            &i32_array_attr(&vd, "normalsIndices"),
            &position_indices,
            positions.len(),
        );
        let uvs = expand_loops(
            &vector2_array_attr(&vd, "textureCoordinates"),
            &i32_array_attr(&vd, "textureCoordinatesIndices"),
            &position_indices,
            positions.len(),
        );

        let weights = expand_weights(&vd, positions.len(), &name, warnings);

        let (triangles, material) = collect_faces(mesh, &position_indices, &material_index, &name, warnings);

        out.push(DmxMesh {
            name,
            material,
            positions,
            normals,
            uvs,
            weights,
            triangles,
        });
    }
    out
}

/// 把 loop 空间的属性摊平到 position 空间。
fn expand_loops<T: Copy + Default>(
    values: &[T],
    value_indices: &[i32],
    position_indices: &[i32],
    position_count: usize,
) -> Vec<T> {
    let mut out = vec![T::default(); position_count];
    let mut filled = vec![false; position_count];
    for (loop_index, &position_index) in position_indices.iter().enumerate() {
        let Ok(position) = usize::try_from(position_index) else {
            continue;
        };
        if position >= position_count || filled[position] {
            continue;
        }
        let Some(&value_index) = value_indices.get(loop_index) else {
            continue;
        };
        let Ok(value_index) = usize::try_from(value_index) else {
            continue;
        };
        if let Some(value) = values.get(value_index) {
            out[position] = *value;
            filled[position] = true;
        }
    }
    out
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

/// 读所有 face set 的三角形；材质只有一个且一致时才返回下标。
fn collect_faces(
    mesh: &Element,
    position_indices: &[i32],
    material_index: &HashMap<String, usize>,
    mesh_name: &str,
    warnings: &mut Vec<String>,
) -> (Vec<[u32; 3]>, Option<usize>) {
    let mut triangles = Vec::new();
    let mut material: Option<usize> = None;
    let mut mixed = false;

    for face_set in element_array_attr(mesh, "faceSets") {
        let faces = i32_array_attr(&face_set, "faces");
        // ⚠️ `faces` 不是三角形三元组，而是「若干个顶点下标 + `-1` 结束符」的多边形表。
        // BST 导出的立方体是 6 个四边形（`[0,1,2,3,-1, 4,5,6,7,-1, …]`），
        // 按三元组硬切会得到 `(0,1,2) (3,-1,4) …` 这种垃圾。官方 `dmxconvert`
        // 也是先按 `-1` 切多边形再三角化。
        let mut polygon: Vec<u32> = Vec::new();
        let mut dropped = 0usize;
        for &raw in &faces {
            if raw < 0 {
                triangulate(&mut polygon, &mut triangles);
                continue;
            }
            let Ok(loop_index) = usize::try_from(raw) else {
                polygon.clear();
                dropped += 1;
                continue;
            };
            let Some(&position_index) = position_indices.get(loop_index) else {
                polygon.clear();
                dropped += 1;
                continue;
            };
            let Ok(position_index) = u32::try_from(position_index) else {
                polygon.clear();
                dropped += 1;
                continue;
            };
            polygon.push(position_index);
        }
        triangulate(&mut polygon, &mut triangles);
        if dropped > 0 {
            warnings.push(format!(
                "mesh {mesh_name:?}: {dropped} face indices point outside the index space, those polygons were dropped"
            ));
        }

        let this_material = element_attr(&face_set, "material")
            .and_then(|m| string_attr(&m, "mtlName"))
            .and_then(|name| material_index.get(&name).copied());
        match (material, this_material) {
            (None, m) => material = m,
            (Some(existing), Some(m)) if existing != m => mixed = true,
            _ => {}
        }
    }

    if mixed {
        warnings.push(format!(
            "mesh {mesh_name:?} mixes several materials, the material is left unset"
        ));
        material = None;
    }
    (triangles, material)
}

/// 扇形三角化一个多边形（顶点顺序保持原样，绕序不变），然后清空它。
fn triangulate(polygon: &mut Vec<u32>, triangles: &mut Vec<[u32; 3]>) {
    if polygon.len() >= 3 {
        for k in 1..polygon.len() - 1 {
            triangles.push([polygon[0], polygon[k], polygon[k + 1]]);
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

fn int_attr(el: &Element, name: &str) -> Option<i32> {
    let attr = el.get_attribute(name)?;
    match attr.get_inner_value() {
        AttributeValue::Integer(value) => Some(*value),
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
        assert_eq!(mesh.normals.len(), 8);
        assert_eq!(mesh.uvs.len(), 8);
        assert!(
            mesh.weights.iter().all(|chain| chain.len() == 1),
            "每个顶点应当只有一条权重链"
        );
        assert_eq!(mesh.material, Some(0));
    }

    #[test]
    fn rejects_something_that_is_not_a_datamodel() {
        let err =
            read_from(&mut Cursor::new(b"not a dmx".as_slice()), "x.dmx").expect_err("应当报错");
        assert!(matches!(err, DmxError::Parse { .. }), "实际得到 {err:?}");
    }
}
