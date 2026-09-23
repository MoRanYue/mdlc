//! 编译前端：把「描述文件 + SMD」编译成写出器要的 IR。
//!
//! # 职责边界
//!
//! - 描述文件只声明**结构与元数据**（模型名、材质、骨骼、body part 树）；
//! - SMD 承载**网格与参考姿态**；
//! - 本模块把两者合并，并做**跨文件的一致性校验**（这类错误无法在
//!   单看描述或单看 SMD 时发现）。
//!
//! # 两条与 studiomdl 对齐的行为
//!
//! 1. **mesh 按材质名划分**：SMD 里同一材质名的三角形归入同一个 mesh。
//!    材质名到材质表下标的映射按**首次出现顺序**建立。
//! 2. **骨骼参考姿态默认取自 SMD 的 `skeleton` 第 0 帧**。描述里显式写了
//!    `position` / `rotation` 才覆盖（QC 的 `$definebone` 也是覆盖语义）。
//!
//! # 顶点去重
//!
//! SMD 是「每个三角形三个顶点」的展开格式，同一个顶点会被重复列出很多次。
//! VVD 需要的是**去重后的顶点池 + 索引**，所以这里按
//! `(位置, 法线, UV, 骨骼绑定)` 精确去重 —— 与 studiomdl 的语义一致：
//! 只要有一项不同就是不同顶点（因为它们在 VVD 里是不同的记录）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::{
    Bone, BodyModel, CompiledBodyPart, CompiledModel, CompiledModelDesc, MAX_BONES_PER_VERT, Mesh,
    ModelDesc, ModelLods, Vertex,
};
use crate::smd::{Smd, SmdPose, parse_smd};

/// 编译前端错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    /// 出错位置（描述文件里的路径，或 SMD 文件路径）。
    pub at: String,
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}

impl std::error::Error for CompileError {}

/// 未配 `eyelid` 的 eyeball 用的占位 flexdesc 名（L4D2 行为，见
/// [`resolve_flex_eyeball_mouth`] 的「0.」小节）。
const DUMMY_EYELID: &str = "dummy_eyelid";

/// `sectionframes` 的缺省段长（studiomdl `.data` 静态初值 = **30**）。
const DEFAULT_SECTION_FRAMES: i32 = 30;
/// `sectionframes` 的缺省触发阈值（studiomdl `.data` 静态初值 = **120**）。
const DEFAULT_SECTION_THRESHOLD: i32 = 120;

/// 未配 `eyelid` 时 lid 的缺省 target —— 受控实验实测**恒为 `[-1, 0, 1]`**
/// （`fxr1`/`fx2`/`fxl1` 三个用例一致，且与 eyeball 的 `radius` 无关）。
const DUMMY_LID_TARGETS: [f32; 3] = [-1.0, 0.0, 1.0];

fn e(at: impl Into<String>, message: impl Into<String>) -> CompileError {
    CompileError {
        at: at.into(),
        message: message.into(),
    }
}

/// 浮点去重键：按位比较。
///
/// 用 `to_bits` 而不是 `==` —— `-0.0 == 0.0` 但二者在文件里是不同的字节，
/// 而且 `NaN` 用 `==` 永远不相等会让去重失效。
///
/// # 为什么 `bones` 是内联数组而不是 `Vec`
///
/// `Vertex.bones` 最多 [`MAX_BONES_PER_VERT`] 组（`smd_vertex_to_ir` 里已截断），
/// 所以键里的骨骼部分天然定长。早期版本用 `Vec<(i32, u32)>`，
/// **每个顶点**都要分配一次堆内存（100 万三角形约 300 万次）。
///
/// 用「定长数组 + 计数」后零分配。为了保持 `Hash`/`Eq` 的语义与 `Vec` 版本
/// **完全一致**，比较与哈希只看前 `n` 项（见下面的手写实现）——
/// 尾部未使用的槽位不参与，否则「3 组」与「1 组 + 2 个填充」会被判成不同。
#[derive(Debug, Clone, Copy)]
struct VertexKey {
    pos: [u32; 3],
    normal: [u32; 3],
    uv: [u32; 2],
    /// 排序后的 (骨骼下标, 权重位模式)，前 `n_bones` 项有效。
    bones: [(i32, u32); MAX_BONES_PER_VERT],
    n_bones: u8,
}

impl VertexKey {
    #[inline]
    fn bones_slice(&self) -> &[(i32, u32)] {
        &self.bones[..self.n_bones as usize]
    }
}

impl PartialEq for VertexKey {
    fn eq(&self, other: &Self) -> bool {
        self.pos == other.pos
            && self.normal == other.normal
            && self.uv == other.uv
            && self.bones_slice() == other.bones_slice()
    }
}
impl Eq for VertexKey {}

impl std::hash::Hash for VertexKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.pos.hash(state);
        self.normal.hash(state);
        self.uv.hash(state);
        // 与 `Vec` 的 Hash 对齐：先写长度，再写元素。
        // `Vec<T>` 的 `Hash` 是 `len.hash()` + 每项 `hash()`（含长度前缀），
        // 这里复刻同样的口径，保证哈希分布不退化。
        self.bones_slice().hash(state);
    }
}

fn fbits(v: f32) -> u32 {
    // 归一化 -0.0 → 0.0，否则同一位置会被当成两个顶点。
    if v == 0.0 { 0.0f32.to_bits() } else { v.to_bits() }
}

fn vertex_key(v: &Vertex) -> VertexKey {
    let n = v.bones.len().min(MAX_BONES_PER_VERT);
    let mut bones = [(0i32, 0u32); MAX_BONES_PER_VERT];
    for (i, p) in v.bones.iter().take(n).enumerate() {
        bones[i] = (p[0] as i32, fbits(p[1]));
    }
    // 排序让「绑定顺序不同但语义相同」的顶点也能合并。
    bones[..n].sort_unstable();
    VertexKey {
        pos: [fbits(v.pos[0]), fbits(v.pos[1]), fbits(v.pos[2])],
        normal: [
            fbits(v.normal[0]),
            fbits(v.normal[1]),
            fbits(v.normal[2]),
        ],
        uv: [fbits(v.uv[0]), fbits(v.uv[1])],
        bones,
        n_bones: n as u8,
    }
}

/// `$staticprop` 的几何旋转：绕 Z 轴 +90°。
///
/// # 来源
///
/// `studiomdl.cpp:6883` 设下全局默认旋转：
///
/// ```cpp
/// g_defaultrotation = RadianEuler( 0, 0, M_PI / 2 );
/// ```
///
/// `MakeStaticProp()`（`simplify.cpp:3273`）用
/// `AngleMatrix(g_defaultrotation, rotated)` 建矩阵，然后对**每个顶点**把
/// 位置、法线、切线各旋转一次（`simplify.cpp:3310-3319`）。
///
/// `AngleMatrix(RadianEuler(0,0,π/2))` 展开后是
/// `[[0,-1,0],[1,0,0],[0,0,1]]`，作用在点上即：
///
/// ```text
/// (x, y, z) -> (-y, x, z)          Z 分量不变
/// ```
///
/// 这与 `$eyeposition` / `$illumposition` 的轴变换（[`crate::mdl_writer`]
/// 的 `qc_axis_to_model`）**是同一个映射** —— 见
/// `docs/coordinate-systems.md`。
///
/// # 实测佐证
///
/// 同一份 SMD 只差 `$staticprop`（`docs/_probe/smdl/ipa1.smd`）：
///
/// | 产物 | VVD 顶点 | 法线 |
/// |---|---|---|
/// | `ipa1`（无） | `(0,0,0) (10,0,3) (0,20,7)` | `(1,0,0)` |
/// | `ipa2`（有） | `(0,0,0) (0,10,3) (-20,0,7)` | `(0,1,0)` |
///
/// 逐分量吻合。
pub fn static_prop_rotate(v: [f32; 3]) -> [f32; 3] {
    [-v[1], v[0], v[2]]
}

/// 解析 SMD 路径：相对路径以 `base_dir` 为基准。
pub fn resolve_smd_path(base_dir: &Path, smd: &str) -> PathBuf {
    let p = Path::new(smd);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

/// 读取并解析一个 SMD 文件。
fn read_smd(path: &Path, at: &str) -> Result<Smd, CompileError> {
    let text = std::fs::read_to_string(path).map_err(|err| {
        e(
            at,
            format!("读不到 {}：{err}", path.display()),
        )
    })?;
    parse_smd(&text).map_err(|err| e(at, format!("{} 解析失败：{err}", path.display())))
}

/// 一个 LOD 的原始网格：顶点池 + 三角形。
type LodMesh = (Vec<Vertex>, Vec<[u32; 3]>);

/// 构建一个 model 的多 LOD 数据。
///
/// # 算法（对应 studiomdl 的 `$lod` + `UnifyLODs`）
///
/// 1. 读每个 LOD 的 SMD，各自按材质名划分 mesh（与 LOD 0 同一套规则）；
/// 2. **按材质名对齐**各 LOD 的 mesh —— 同一个材质名在各 LOD 里必须是
///    同一个 mesh，否则 `mstudiomesh_t` 的对应关系会错位（材质贴错面）；
/// 3. 对每个 mesh，用 [`crate::lod::unify_lods`] 把各 LOD 的顶点池
///    精确去重合并成一个统一池，并算出每个顶点的 LOD 归属位掩码。
///
/// # 为什么按材质名而不是按 mesh 序号对齐
///
/// SMD 里 mesh 的划分是「材质名首次出现顺序」。不同 LOD 的导出器很可能
/// 以不同顺序写材质块（甚至漏掉某个材质），按下标对齐会静默贴错材质。
/// 按名字对齐 + 显式报错缺失的材质，是唯一能保证正确的方式。
fn build_model_lods(
    m: &BodyModel,
    lod0_meshes: &[Mesh],
    lod0_smd: &Smd,
    desc: &ModelDesc,
    base_dir: &Path,
    at: &str,
) -> Result<ModelLods, Vec<CompileError>> {
    let mut errs: Vec<CompileError> = Vec::new();
    let num_lods = m.lods.len() + 1;

    // LOD 0 的材质名 → mesh 下标（`lod0_meshes[i].material`）。
    let lod0_material_of: Vec<usize> = lod0_meshes.iter().map(|k| k.material).collect();
    let _ = lod0_smd;

    // 每个 mesh 收集各 LOD 的 (顶点池, 三角形)。先放 LOD 0。
    let mut per_mesh: Vec<Vec<LodMesh>> = vec![Vec::with_capacity(num_lods); lod0_meshes.len()];
    for (ki, mesh) in lod0_meshes.iter().enumerate() {
        per_mesh[ki].push((mesh.vertices.clone(), mesh.triangles.clone()));
    }

    // ---- 每档的骨骼映射（与 mesh 无关，先算一次）----
    //
    // 顺序严格照 `SimplifyModel()`（`simplify.cpp:7246-7256`）：
    // `ConvertBoneTreeCollapsesToReplaceBones()` → `FixupReplacedBones()`。
    let bone_index = desc.bone_index();
    let n_bones = desc.bones.len();
    let parents: Vec<i32> = desc
        .bones
        .iter()
        .map(|b| match b.parent.as_deref() {
            Some(p) => bone_index.get(p).map(|v| *v as i32).unwrap_or(-1),
            None => -1,
        })
        .collect();
    let mut lod_bone_maps: Vec<Vec<usize>> = Vec::with_capacity(num_lods);
    lod_bone_maps.push(Vec::new()); // LOD 0 恒等（官方 `BuildBoneLODMapping(map, 0)`）
    for lod in &m.lods {
        let roots: Vec<usize> = lod
            .bone_tree_collapse
            .iter()
            .filter_map(|n| bone_index.get(n.as_str()).copied())
            .collect();
        let mut reps = crate::lod::expand_bone_tree_collapses(&roots, &parents);
        for pair in &lod.replace_bone {
            if let (Some(&s), Some(&d)) = (
                bone_index.get(pair[0].as_str()),
                bone_index.get(pair[1].as_str()),
            ) {
                reps.push((s, d));
            }
        }
        crate::lod::fixup_replaced_bones(&mut reps);
        lod_bone_maps.push(crate::lod::build_bone_lod_mapping(n_bones, &reps));
    }

    // 逐 LOD 读 SMD 并对齐。
    let _t_read = crate::prof::Span::new("  LOD: 读 SMD + build_meshes");
    for (li, lod) in m.lods.iter().enumerate() {
        let lod_no = li + 1; // LOD 0 是 `m.smd`
        let lpath = format!("{at}.lods[{li}]");

        // 没写 `smd` ⟹ 复用 LOD 0 的网格，只应用骨骼选项。
        // 官方 `GetLODSources`：`if (!pSource && !found) pSource = pSrcModel->source;`
        let Some(lod_smd) = lod.smd.as_deref() else {
            for (ki, mesh) in lod0_meshes.iter().enumerate() {
                per_mesh[ki].push((mesh.vertices.clone(), mesh.triangles.clone()));
            }
            continue;
        };

        let smd_path = resolve_smd_path(base_dir, lod_smd);
        let smd = match read_smd(&smd_path, &lpath) {
            Ok(s) => s,
            Err(err) => {
                errs.push(err);
                continue;
            }
        };
        // 骨骼必须与 LOD 0 一致（同一个 model 的各 LOD 共用骨架）。
        let desc_index = desc.bone_index();
        let missing: Vec<&str> = smd
            .nodes
            .iter()
            .map(|n| n.name.as_str())
            .filter(|n| !desc_index.contains_key(n))
            .collect();
        if !missing.is_empty() {
            errs.push(e(
                &lpath,
                format!(
                    "{} 里有 {} 根骨骼不在 [[bones]] 中：{}",
                    smd_path.display(),
                    missing.len(),
                    missing
                        .iter()
                        .take(5)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
            continue;
        }
        let meshes = match build_meshes(&smd, desc, &smd_path, &lpath) {
            Ok(v) => v,
            Err(err) => {
                errs.push(err);
                continue;
            }
        };
        // 按材质下标对齐到 LOD 0 的 mesh。
        // 缺材质 / 多材质都要显式报错 —— 静默对齐会贴错材质。
        let mut by_material: HashMap<usize, &Mesh> =
            meshes.iter().map(|k| (k.material, k)).collect();
        for (ki, mat) in lod0_material_of.iter().enumerate() {
            let Some(lod_mesh) = by_material.remove(mat) else {
                errs.push(e(
                    &lpath,
                    format!(
                        "LOD {lod_no} 缺少材质下标 {mat} 的 mesh（LOD 0 的 mesh[{ki}] 用了它）—— \
                         各 LOD 的材质集合必须一致"
                    ),
                ));
                continue;
            };
            per_mesh[ki].push((lod_mesh.vertices.clone(), lod_mesh.triangles.clone()));
        }
        if !by_material.is_empty() {
            let extra: Vec<usize> = by_material.keys().copied().collect();
            errs.push(e(
                &lpath,
                format!("LOD {lod_no} 多出 LOD 0 没有的材质下标 {extra:?}"),
            ));
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    drop(_t_read);

    // 每个 mesh 统一去重（**走骨骼重映射感知的字典**）。
    //
    // `per_mesh[ki]` 是「LOD 0、LOD 1、…」的顶点池，与 `lod_bone_maps` 一一对应。
    let mut bone_lod_usage: Vec<u32> = vec![0; n_bones];
    let _t_unify = crate::prof::Span::new("  LOD: unify_lods_remapped(全部 mesh)");
    let mesh_lods: Vec<crate::lod::MeshLods> = per_mesh
        .iter()
        .map(|lods| {
            let srcs: Vec<crate::lod::LodSource> = lods
                .iter()
                .enumerate()
                .map(|(n, (verts, tris))| crate::lod::LodSource {
                    vertices: verts.clone(),
                    triangles: tris.clone(),
                    bone_map: lod_bone_maps.get(n).cloned().unwrap_or_default(),
                })
                .collect();
            crate::lod::unify_lods_remapped(&srcs, &mut bone_lod_usage)
        })
        .collect();
    drop(_t_unify);

    // switchPoint：LOD 0 恒 0；其余用显式值，否则按 20/40/80… 推算。
    let mut switch_points = Vec::with_capacity(num_lods);
    switch_points.push(0.0f32);
    for (li, lod) in m.lods.iter().enumerate() {
        let auto = 20.0f32 * (1u32 << li) as f32;
        switch_points.push(lod.switch_point.unwrap_or(auto));
    }

    // `nofacial`：**逐档**（官方 `scriptLOD.GetFacialAnimationEnabled()`）。
    // LOD 0 恒 `false` —— 官方那个隐式插入的空档用的是 `LodScriptData_t` 的
    // 缺省 `m_bFacialAnimation = true`（`studiomdl.h:1313`）。
    let no_facial: Vec<bool> = std::iter::once(false)
        .chain(m.lods.iter().map(|l| l.no_facial))
        .collect();

    Ok(ModelLods {
        meshes: mesh_lods,
        num_lods,
        switch_points,
        bone_lod_usage,
        no_facial,
    })
}

/// 把 SMD 的三角形按材质名划分成 mesh。
///
/// 材质名 → 材质表下标的映射：先在描述文件的 `[materials].textures` 里
/// 按名字找（忽略分隔符与 `$cdmaterials` 前缀差异）；找不到则**报错** ——
/// 静默新建一个材质会让用户在游戏里看到「材质丢失」而不知道原因。
/// 「SMD 骨骼下标 → 描述文件骨骼下标」的查找表，**每个 SMD 只建一次**。
///
/// # 为什么要把它提出顶点循环
///
/// [`smd_vertex_to_ir`] 原先在**每个顶点**上重建这两张表：
/// `node_names`（`Vec<&str>`）与 `desc.bone_index()`（`HashMap<&str, usize>`）。
/// 后者实测 **71 ns/次**（68 根骨骼），而且**每次都新分配一个 HashMap**。
///
/// 100 万三角形（300 万个顶点）时：
///
/// | 项 | 成本 |
/// |---|---|
/// | 时间 | ≈ 214 ms（占 `compile()` 的 5%） |
/// | 分配 | ≈ 300 万次 |
///
/// 表的内容在一个 SMD 内是**恒定**的（骨骼表来自描述文件，`nodes` 段来自
/// SMD 本身），所以没有任何理由逐顶点重建。
struct VertexBoneMap<'s, 'd> {
    /// SMD `nodes` 段的骨骼名（按 SMD 自己的下标）。
    node_names: Vec<&'s str>,
    /// 描述文件骨骼名 → 下标。与 [`ModelDesc::bone_index`] 一致（**大小写敏感**）。
    desc_index: HashMap<&'d str, usize>,
    /// `nodes` 段的条目数（只用于报错文案）。
    node_count: usize,
    /// 描述文件的骨骼总数。
    bone_count: usize,
}

impl<'s, 'd> VertexBoneMap<'s, 'd> {
    fn new(smd: &'s Smd, desc: &'d ModelDesc) -> Self {
        VertexBoneMap {
            node_names: smd.nodes.iter().map(|n| n.name.as_str()).collect(),
            desc_index: desc.bone_index(),
            node_count: smd.nodes.len(),
            bone_count: desc.bones.len(),
        }
    }
}

fn build_meshes(
    smd: &Smd,
    desc: &ModelDesc,
    smd_path: &Path,
    at: &str,
) -> Result<Vec<Mesh>, CompileError> {
    let names = smd.materials_in_order();
    if names.is_empty() {
        return Err(e(at, format!("{} 里没有任何三角形", smd_path.display())));
    }

    let cd: Vec<String> = desc.materials.search_paths.clone();
    let mut lookup: HashMap<String, usize> = HashMap::new();
    for (i, t) in desc.materials.textures.iter().enumerate() {
        // 匹配键：统一分隔符 + basename 兜底。
        //
        // 为什么需要 basename：SMD/DMX 源里的材质名可能是**裸名**，
        // 而 TOML 里写的是**带路径**的全名（官方产物就是这么写的）。
        // 官方的 `$cdmaterials` 是 `models\moranyue\...\`，而材质名是
        // `moranyue/.../frown` —— **前缀对不上 cd 路径**，
        // 所以 `normalize_texture_name` 剥不掉它。这正是引擎需要
        // 「根目录搜索哨兵」（空 cdtexture）的原因。
        let key = crate::mdl_writer::texture_match_key(&t.name, &cd);
        // 先插入者优先（`entry` 保留首个），保证确定性。
        lookup.entry(key).or_insert(i);
    }

    // 材质名 → mesh 下标。
    let mut material_of: HashMap<&str, usize> = HashMap::new();
    for n in &names {
        let key = crate::mdl_writer::texture_match_key(n, &cd);
        let Some(&mi) = lookup.get(&key) else {
            return Err(e(
                at,
                format!(
                    "SMD 里的材质名 {n:?} 在 [materials].textures 里找不到（匹配键为 {key:?}）"
                ),
            ));
        };
        material_of.insert(n.as_str(), mi);
    }

    // 逐 mesh 累积（保持材质首次出现顺序）。
    let mut order: Vec<usize> = Vec::new();
    let mut per_mesh: HashMap<usize, Vec<Vertex>> = HashMap::new();
    let mut tris_per_mesh: HashMap<usize, Vec<[u32; 3]>> = HashMap::new();
    // 每个 mesh 自己的顶点去重表。
    let mut dedup: HashMap<usize, HashMap<VertexKey, u32>> = HashMap::new();

    // 骨骼查找表**只建一次**（原先在 `smd_vertex_to_ir` 里逐顶点重建）。
    let bone_map = VertexBoneMap::new(smd, desc);

    for t in &smd.triangles {
        let mi = material_of[t.material.as_str()];
        if let std::collections::hash_map::Entry::Vacant(e) = per_mesh.entry(mi) {
            order.push(mi);
            e.insert(Vec::new());
            tris_per_mesh.insert(mi, Vec::new());
            dedup.insert(mi, HashMap::new());
        }
        let pool = per_mesh.get_mut(&mi).unwrap();
        let table = dedup.get_mut(&mi).unwrap();
        let mut corner = [0u32; 3];
        for (c, sv) in t.vertices.iter().enumerate() {
            let v = smd_vertex_to_ir(sv, desc, &bone_map, smd_path, at)?;
            let key = vertex_key(&v);
            let idx = match table.get(&key) {
                Some(&i) => i,
                None => {
                    let i = pool.len() as u32;
                    pool.push(v);
                    table.insert(key, i);
                    i
                }
            };
            corner[c] = idx;
        }
        // 退化三角形（去重后有两个角相同）直接跳过 —— 它们在 VVD/VTX 里
        // 是零面积面，会让 strip 生成器产出无效数据。
        if corner[0] == corner[1] || corner[1] == corner[2] || corner[0] == corner[2] {
            continue;
        }
        tris_per_mesh.get_mut(&mi).unwrap().push(corner);
    }

    let mut meshes = Vec::with_capacity(order.len());
    let mut dropped = 0usize;
    for mi in order {
        let vertices = per_mesh.remove(&mi).unwrap();
        let triangles = tris_per_mesh.remove(&mi).unwrap();
        if vertices.is_empty() || triangles.is_empty() {
            dropped += 1;
            continue;
        }
        meshes.push(Mesh {
            material: mi,
            vertices,
            triangles,
            eyeball_tag: None,
        });
    }
    // 全部三角形都退化掉时必须报错 —— 否则会产出一个「有材质但没有几何」
    // 的模型，在游戏里表现为看不见任何东西，且没有任何提示。
    if meshes.is_empty() {
        return Err(e(
            at,
            format!(
                "{} 里没有可用的三角形（{dropped} 个材质的三角形全部退化）",
                smd_path.display()
            ),
        ));
    }
    Ok(meshes)
}

/// 把一个 SMD 三角形顶点转成 IR 顶点，并做边界校验。
///
/// `bone_map` 由调用方**每个 SMD 建一次**并复用 —— 见 [`VertexBoneMap`]。
/// 早期版本在这里逐顶点重建骨骼表，是 `compile()` 分配量的主要来源之一。
fn smd_vertex_to_ir(
    sv: &crate::smd::SmdVertex,
    desc: &ModelDesc,
    bone_map: &VertexBoneMap<'_, '_>,
    smd_path: &Path,
    at: &str,
) -> Result<Vertex, CompileError> {
    let bone_count = bone_map.bone_count;

    // SMD 的绑定用的是**SMD 自己的**骨骼下标，需要映射到描述文件的骨骼表。
    // 两边都用名字对齐 —— 这样 SMD 里多出的骨骼（例如仅用于动画的辅助骨）
    // 会被明确报错而不是静默错位。
    let node_names = &bone_map.node_names;
    let desc_index = &bone_map.desc_index;

    // 权重按从大到小排序，并只保留前 MAX_BONES_PER_VERT 组（引擎上限）。
    //
    // # 为什么不克隆 `sv.links`
    //
    // `SmdVertex.links` 是 `Vec`，`clone()` 会在**每个顶点**上分配一次堆内存
    // （100 万三角形约 300 万次）。这里改用栈上的定长数组。
    //
    // ⚠️ **必须排全部元素**：若先截断到内联容量再排序，超过容量的顶点
    // 就可能漏掉本该进前 3 的绑定 —— 那是**语义改变**，不是优化。
    // 因此超出内联容量时回退到原来的 `Vec` 路径，保证任何输入都逐位等价。
    const INLINE: usize = 8;
    let mut inline_buf: [crate::smd::SmdBoneLink; INLINE] =
        [crate::smd::SmdBoneLink { bone: 0, weight: 0.0 }; INLINE];
    let mut fallback: Vec<crate::smd::SmdBoneLink>;
    let links: &mut [crate::smd::SmdBoneLink] = if sv.links.len() <= INLINE {
        let n = sv.links.len();
        inline_buf[..n].copy_from_slice(&sv.links[..n]);
        &mut inline_buf[..n]
    } else {
        fallback = sv.links.clone();
        &mut fallback
    };
    links.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));

    let mut bones: Vec<[f32; 2]> = Vec::with_capacity(links.len().min(MAX_BONES_PER_VERT));
    for l in links.iter().take(MAX_BONES_PER_VERT) {
        if l.weight <= 0.0 {
            continue;
        }
        let node_name = node_names.get(l.bone.max(0) as usize).ok_or_else(|| {
            e(
                at,
                format!(
                    "{} 里顶点引用了骨骼下标 {}，但 nodes 段只有 {} 项",
                    smd_path.display(),
                    l.bone,
                    bone_map.node_count
                ),
            )
        })?;
        let Some(&di) = desc_index.get(node_name) else {
            return Err(e(
                at,
                format!(
                    "{} 里的骨骼 {node_name:?} 不在描述的 [[bones]] 里",
                    smd_path.display()
                ),
            ));
        };
        if di >= bone_count {
            return Err(e(at, format!("骨骼下标 {di} 越界（共 {bone_count} 根）")));
        }
        bones.push([di as f32, l.weight]);
    }
    if bones.is_empty() {
        return Err(e(
            at,
            format!(
                "{} 里有顶点的蒙皮权重全为 0 或没有绑定",
                smd_path.display()
            ),
        ));
    }
    // 归一化权重：SMD 里的权重常有浮点残差（0.999999），
    // 不归一化会让引擎的蒙皮出现微小但可见的偏差。
    let sum: f32 = bones.iter().map(|b| b[1]).sum();
    if sum > 0.0 && (sum - 1.0).abs() > 1e-6 {
        for b in &mut bones {
            b[1] /= sum;
        }
    }

    // `$staticprop`：几何整体旋转 `Rz(90°)`，且**所有**权重归到骨骼 0。
    //
    // 这一步必须发生在**顶点构造时**（而不是编译末尾的后处理），因为
    // `unify_lods` 的去重键**包含骨骼绑定**：先把权重塌缩再统一，
    // 才能让「原本只差绑定」的顶点正确合并。studiomdl 的顺序也是如此 ——
    // `MakeStaticProp()` 在 `RemapBones()` 里，早于 `UnifyLODs()`。
    //
    // 权重直接写成「骨骼 0、权重 1.0」：原权重之和恒为 1，全部归到骨骼 0
    // 之后总权重仍是 1，语义与「逐项置 0」等价，且省掉 studiomdl 那步
    // `MergeLikeBoneIndicesWithinVert` 合并。实测官方产物正是
    // `bone=[0,0,0] bone_count=1 weight=[1.0, 0, 0]`。
    if desc.model.static_prop {
        return Ok(Vertex {
            pos: static_prop_rotate(sv.position),
            normal: static_prop_rotate(sv.normal),
            uv: sv.uv,
            bones: vec![[0.0, 1.0]],
        });
    }

    Ok(Vertex {
        pos: sv.position,
        normal: sv.normal,
        uv: sv.uv,
        bones,
    })
}

/// blend 网格的尺寸 `(groupsize[0], groupsize[1])`。
///
/// # 推断规则（`simplify.cpp:2994-3024`）
///
/// * `blend_width` 给了 ⟹ `groupsize[0] = blend_width`、
///   `groupsize[1] = 格数 / blend_width`；除不尽直接报错。
/// * 没给 ⟹ 格数 < 4 时 `groupsize = (格数, 1)`；
///   否则要求**完全平方数**，开方成方阵，否则报
///   `non-square (%d) number of blends without "blendwidth" set`。
fn blend_grid_size(
    s: &crate::model::Sequence,
    at: &str,
    errs: &mut Vec<CompileError>,
) -> Option<(i32, i32)> {
    let n = s.blends.len() as i32;
    let e = |msg: String| CompileError {
        at: format!("{at}.blends"),
        message: msg,
    };
    match s.blend_width {
        Some(w) if w > 0 => {
            if n % w != 0 {
                errs.push(e(format!(
                    "blend 格数 {n} 不能被 blend_width {w} 整除（groupsize[0]*groupsize[1] 必须等于格数）"
                )));
                return None;
            }
            Some((w, n / w))
        }
        Some(w) => {
            errs.push(e(format!("blend_width 必须是正数，实际 {w}")));
            None
        }
        None => {
            if n < 4 {
                Some((n, 1))
            } else {
                let r = (n as f64).sqrt() as i32;
                if r * r == n {
                    Some((r, r))
                } else {
                    errs.push(e(format!(
                        "blend 格数 {n} 不是完全平方数，且没有给 blend_width —— \
                         官方会报 non-square number of blends（simplify.cpp:3011）"
                    )));
                    None
                }
            }
        }
    }
}

/// 读一个 SMD 并把它转成「按描述骨骼顺序排列」的逐帧姿态。
///
/// 抽出来是为了让**单动画序列**与 **blend 的每一格**共用同一条路径 ——
/// 两处各写一份迟早会分叉。
///
/// 失败时把错误推进 `errs` 并返回 `None`。
/// 读一个 SMD 的**全部帧**，按描述文件的骨骼表对齐成稠密行。
///
/// # 骨骼表 = `$definebone` ∪ SMD 的 `nodes`
///
/// 官方 `BuildGlobalBonetable`（`simplify.cpp:3616-3654`）**先**把
/// `g_importbone`（`$definebone` 收集来的）逐条插进骨骼表，**再**并入
/// 各 SMD 用到的骨骼（同名的靠 `findGlobalBone` 去重）。
///
/// 所以 `$definebone` 声明了、而 SMD 里**没有**的骨骼**依然存在**，
/// 参考姿态取 `$definebone` 给的 `rawLocal`。
///
/// 实测（`parity/myprop.qc` → 官方 `myprop.mdl`）：
///
/// ```text
/// $definebone "root" ""    0 0 0 0 0 0     SMD nodes 有 root
/// $definebone "tip" "root" 0 0 8 0 0 0     SMD nodes **没有** tip
///
/// 官方产物：numbones = 2
///   BONE[0] root  pos = [0, 0, 0]  flags = 0x40700
///   BONE[1] tip   pos = [0, 0, 8]  flags = 0x200   ← 来自 $definebone
/// ```
///
/// > 早先 mdlc 要求「每帧都必须有全部骨骼的姿态」，于是这种 QC
/// > 直接报「缺少部分骨骼的姿态（1 / 2 根有数据）」——
/// > 而官方是正常编过的。`verify_parity.ps1` 因此长期红灯。
///
/// # 缺失骨骼的姿态从哪来
///
/// 只能取 `$definebone` 声明的 `position`/`rotation`（TOML 的
/// `[[bones]]`）。**没声明就是错误** —— 无法凭空发明一个参考姿态，
/// 静默用 0 会让骨骼塌到原点，表现为顶点被拉向世界原点。
fn load_smd_frames(
    smd_path: &std::path::Path,
    desc: &ModelDesc,
    desc_index: &std::collections::HashMap<&str, usize>,
    at: &str,
    errs: &mut Vec<CompileError>,
) -> Option<(Vec<Vec<crate::smd::SmdPose>>, Smd)> {
    let e = |msg: String| CompileError {
        at: at.to_string(),
        message: msg,
    };
    let bone_count = desc.bones.len();
    let text = match std::fs::read_to_string(smd_path) {
        Ok(t) => t,
        Err(err) => {
            errs.push(e(format!("读不到 {}：{err}", smd_path.display())));
            return None;
        }
    };
    let smd = match parse_smd(&text) {
        Ok(s) => s,
        Err(err) => {
            errs.push(e(format!("{} 解析失败：{err}", smd_path.display())));
            return None;
        }
    };
    if smd.frames.is_empty() {
        errs.push(e(format!("{} 的 skeleton 段没有任何帧", smd_path.display())));
        return None;
    }
    let missing: Vec<&str> = smd
        .nodes
        .iter()
        .map(|n| n.name.as_str())
        .filter(|n| !desc_index.contains_key(n))
        .collect();
    if !missing.is_empty() {
        errs.push(e(format!(
            "{} 里有 {} 根骨骼不在 [[bones]] 中：{}",
            smd_path.display(),
            missing.len(),
            missing.iter().take(5).copied().collect::<Vec<_>>().join(", ")
        )));
        return None;
    }
    // 描述里有、SMD 里没有的骨骼 —— 官方会保留它们，姿态取 `$definebone`。
    //
    // 只有在**显式声明了姿态**时才允许（`position` 或 `rotation` 任一）：
    // 那是 `$definebone` 的语义。完全没声明的骨骼若又不在 SMD 里，
    // 就没有任何姿态来源，必须报错。
    let smd_names: std::collections::HashSet<&str> =
        smd.nodes.iter().map(|n| n.name.as_str()).collect();
    let mut fallback: Vec<Option<crate::smd::SmdPose>> = vec![None; bone_count];
    for (di, b) in desc.bones.iter().enumerate() {
        if smd_names.contains(b.name.as_str()) {
            continue;
        }
        if b.position.is_none() && b.rotation.is_none() {
            errs.push(e(format!(
                "{} 里没有骨骼 {:?}，而 [[bones]] 也没给它 position/rotation —— \
                 无法确定参考姿态（官方会取 $definebone 的 rawLocal）",
                smd_path.display(),
                b.name
            )));
            return None;
        }
        fallback[di] = Some(crate::smd::SmdPose {
            bone: di as i32,
            position: b.position.unwrap_or([0.0; 3]),
            // TOML 的 `rotation` 是**角度**，SMD 的 `SmdPose.rotation` 是**弧度**。
            rotation: b
                .rotation
                .unwrap_or([0.0; 3])
                .map(f32::to_radians),
        });
    }
    let mut frames: Vec<Vec<crate::smd::SmdPose>> = Vec::with_capacity(smd.frames.len());
    for f in &smd.frames {
        let mut row = vec![
            crate::smd::SmdPose {
                bone: 0,
                position: [0.0; 3],
                rotation: [0.0; 3],
            };
            bone_count
        ];
        let mut seen = vec![false; bone_count];
        // 先铺 `$definebone` 的兜底姿态，SMD 里有数据的会覆盖它。
        for (di, fb) in fallback.iter().enumerate() {
            if let Some(p) = fb {
                row[di] = *p;
                seen[di] = true;
            }
        }
        for p in &f.poses {
            let Some(node) = smd.nodes.get(p.bone.max(0) as usize) else {
                continue;
            };
            let Some(&di) = desc_index.get(node.name.as_str()) else {
                continue;
            };
            row[di] = crate::smd::SmdPose {
                bone: di as i32,
                position: p.position,
                rotation: p.rotation,
            };
            seen[di] = true;
        }
        if seen.iter().any(|s| !s) {
            errs.push(e(format!(
                "{} 的帧 {} 缺少部分骨骼的姿态（{} / {bone_count} 根有数据）",
                smd_path.display(),
                f.time,
                seen.iter().filter(|s| **s).count()
            )));
            return None;
        }
        frames.push(row);
    }
    Some((frames, smd))
}

/// 解析 blend 参数名 → `[[model.pose_parameters]]` 的下标。
///
/// 接受**名字**或**十进制下标**（与 `[[ikchains]]` 的「名字或下标」惯例一致）。
fn resolve_pose_param_index(desc: &ModelDesc, name: &str) -> Option<i32> {
    if let Ok(i) = name.parse::<i32>()
        && i >= 0
        && (i as usize) < desc.model.pose_parameters.len()
    {
        return Some(i);
    }
    desc.model
        .pose_parameters
        .iter()
        .position(|p| p.name == name)
        .map(|i| i as i32)
}

/// 对每一帧减掉 `base` 的某一帧 —— QC 的 `subtract`。
///
/// # 语义（`simplify.cpp:1066-1122`，`STUDIO_POST` 分支）
///
/// ```c
/// QuaternionSMAngles( -1, src[k].rot, pdest->sanim[j][k].rot, ... );  // 旋转
/// VectorSubtract( pdest->sanim[j][k].pos, src[k].pos, ... );          // 位置
/// ```
///
/// `QuaternionSM(s, p, q) = normalize((s·p) · q)`（`bone_setup.cpp:1131-1142`），
/// 所以旋转结果是 **`(−src) · dest`** —— 注意顺序：**参考在左**。
/// 写成 `dest · (−src)` 会得到共轭的结果（旋转方向相反），
/// 在纯单轴旋转上不易察觉，但复合旋转会明显错。
///
/// # 为什么 `subtract` 必然带 `STUDIO_POST`
///
/// QC 的 `subtract` 关键字自己就 `|= STUDIO_POST`（`studiomdl.cpp:1750`），
/// 所以永远走上面这一支；`else` 分支（`QuaternionMAAngles`）对应的是
/// **没有** `STUDIO_POST` 的内部调用路径，QC 表达不出来。
fn subtract_base_frames(
    frames: &mut [Vec<crate::smd::SmdPose>],
    base: &[Vec<crate::smd::SmdPose>],
    base_frame: usize,
) {
    let Some(src) = base.get(base_frame) else {
        return;
    };
    for row in frames.iter_mut() {
        for (k, pose) in row.iter_mut().enumerate() {
            let Some(s) = src.get(k) else { continue };
            // 旋转：`conj(src) · dest`，再转回欧拉角。
            //
            // ⚠️ **`QuaternionScale(p, -1)` 是共轭，不是「四个分量全取负」。**
            //
            // 官方 `QuaternionScale`（`mathlib_base.cpp`）：
            //
            // ```c
            // float sinom = MIN( sqrt(DotProduct(&p.x, &p.x)), 1.f );
            // float sinsom = sin( asin(sinom) * t );      // t = -1 → -sinom
            // t = sinsom / (sinom + FLT_EPSILON);         // ≈ -1
            // VectorScale( &p.x, t, &q.x );               // 向量部分 × (-1)
            // r = sqrt( 1 - sinsom*sinsom );              // = |w|
            // if (p.w < 0) q.w = -r; else q.w = r;        // **w 保持 p.w 的符号**
            // ```
            //
            // 所以 `q = (-x, -y, -z, w)` = **共轭**。对单位四元数，
            // 共轭是**逆旋转**，而「全取负」`-q` 是**同一个**旋转 ——
            // 两者语义完全不同：
            //
            // | 写法 | 结果 | Rz(20°) 与 Rz(35°) 的差 |
            // |---|---|---|
            // | 共轭 `conj(src)·dest` | **相对旋转** | **15°** ✓ |
            // | 取负 `(-src)·dest` | 复合旋转 | 55° ✗ |
            //
            // 实测判据：受控实验 `blend1`/`blend2` 的参考动画 `a_base`
            // 第 0 帧旋转**全为 0**（单位四元数），此时共轭与取负**同结果**
            // —— 所以那两个实验照不到这个 bug。miku 的 `a_idle` 参考姿态
            // **非零**，实测 mdlc 给出 **54.999996°**（正是「取负」的预测），
            // 修正后为 15°。
            let qs = crate::bone_math::angle_quaternion(s.rotation);
            let qd = crate::bone_math::angle_quaternion(pose.rotation);
            let p1 = [-qs[0], -qs[1], -qs[2], qs[3]];
            let q = quaternion_mult(p1, qd);
            pose.rotation = crate::bone_math::quaternion_angles(q);
            // 位置：直接相减。
            for i in 0..3 {
                pose.position[i] -= s.position[i];
            }
        }
    }
}

/// 四元数乘法 `a · b`（与 `QuaternionMult` 同序）。
///
/// 实现在 [`crate::bone_math::quaternion_mult`] —— 那里有完整的
/// `QuaternionAlign` 说明。这里留一个薄封装是因为本模块有若干调用点
/// 是按「欧拉角 → 四元数」直接传的，名字短一点读起来更顺。
fn quaternion_mult(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    crate::bone_math::quaternion_mult(a, b)
}

/// 一个 blend 轴上**逐格**的参数取值（`simplify.cpp:5579-5590`）。
///
/// ```text
/// for m in 0..groupsize:
///     f = m / (groupsize - 1)
///     keys[m] = start * (1 - f) + end * f
/// ```
///
/// `groupsize == 1` 时官方**不写这个轴**（`write.cpp:449` 只在
/// `groupsize[0] > 1 || groupsize[1] > 1` 时写 posekey），
/// 但 `param1[0]` 在官方是 0 —— 所以这里返回长度 1 的 `[0.0]`，
/// 由写出器决定要不要用它。
fn blend_param_keys(start: f32, end: f32, groupsize: usize) -> Vec<f32> {
    if groupsize <= 1 {
        return vec![0.0];
    }
    (0..groupsize)
        .map(|m| {
            let f = m as f32 / (groupsize - 1) as f32;
            start * (1.0 - f) + end * f
        })
        .collect()
}

/// 自动层的一个时间量（`write.cpp:539-551`）。
///
/// 不带 `STUDIO_AL_POSE`（**0x4000**）时**除以 `numframes − 1`** 转成 cycle。
fn layer_time(v: f32, flags: i32, num_frames: f32) -> f32 {
    const STUDIO_AL_POSE: i32 = 0x4000;
    if flags & STUDIO_AL_POSE != 0 || num_frames <= 1.0 {
        v
    } else {
        v / (num_frames - 1.0)
    }
}

/// 取 SMD 参考姿态里某根骨骼的姿态。
fn pose_for(smd: &Smd, node_index: usize) -> Option<&SmdPose> {
    let f = smd.reference_frame()?;
    // skeleton 段的行序与 nodes 顺序一一对应，所以按下标取；
    // 但也允许文件里显式给出 bone 下标（用 bone 字段匹配）。
    f.poses
        .iter()
        .find(|p| p.bone == node_index as i32)
        .or_else(|| f.poses.get(node_index))
}

/// 编译：描述 + SMD → IR。
///
/// `base_dir` 是描述文件所在目录，用于解析 SMD 的相对路径。
pub fn compile(desc: &ModelDesc, base_dir: &Path) -> Result<CompiledModelDesc, Vec<CompileError>> {
    let _t_total = crate::prof::Span::new("compile() 顶层");
    // 先做描述层校验（能一次报出全部问题，比逐个文件报错友好）。
    if let Err(errs) = desc.validate() {
        return Err(errs
            .iter()
            .map(|d| CompileError {
                at: d.path.clone(),
                message: d.message.clone(),
            })
            .collect());
    }

    let mut errors: Vec<CompileError> = Vec::new();
    let mut bodyparts = Vec::with_capacity(desc.bodyparts.len());

    let _t_body = crate::prof::Span::new("bodyparts: 读 SMD + build_meshes + LOD");
    for (bi, bp) in desc.bodyparts.iter().enumerate() {
        let mut models = Vec::with_capacity(bp.models.len());
        for (mi, m) in bp.models.iter().enumerate() {
            let at = format!("bodyparts[{bi}].models[{mi}]");
            let smd_path = resolve_smd_path(base_dir, &m.smd);
            let text = match std::fs::read_to_string(&smd_path) {
                Ok(t) => t,
                Err(err) => {
                    errors.push(e(
                        format!("{at}.smd"),
                        format!("读不到 {}：{err}", smd_path.display()),
                    ));
                    continue;
                }
            };
            let smd = match parse_smd(&text) {
                Ok(s) => s,
                Err(err) => {
                    errors.push(e(
                        format!("{at}.smd"),
                        format!("{} 解析失败：{err}", smd_path.display()),
                    ));
                    continue;
                }
            };

            // SMD 里的骨骼必须在描述的骨骼表里能找到，且数量一致 ——
            // 否则顶点绑定会指向不存在的骨骼。
            let desc_index = desc.bone_index();
            let mut missing: Vec<&str> = Vec::new();
            for n in &smd.nodes {
                if !desc_index.contains_key(n.name.as_str()) {
                    missing.push(n.name.as_str());
                }
            }
            if !missing.is_empty() {
                errors.push(e(
                    format!("{at}.smd"),
                    format!(
                        "{} 里有 {} 根骨骼不在 [[bones]] 中：{}",
                        smd_path.display(),
                        missing.len(),
                        missing
                            .iter()
                            .take(5)
                            .copied()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
                continue;
            }

            let meshes = match build_meshes(&smd, desc, &smd_path, &at) {
                Ok(v) => v,
                Err(err) => {
                    errors.push(err);
                    continue;
                }
            };

            // ---- 多 LOD：读每个 LOD 的 SMD，按材质名对齐 mesh ----
            // 只有描述里写了 `lods` 才走这条路；否则 `lods` 保持 `None`，
            // 写出器走单 LOD 路径，产物与加这个特性之前完全一致。
            let lods = if m.lods.is_empty() {
                None
            } else {
                match build_model_lods(m, &meshes, &smd, desc, base_dir, &at) {
                    Ok(v) => Some(v),
                    Err(errs) => {
                        errors.extend(errs);
                        continue;
                    }
                }
            };

            let name = model_name(m, &smd_path);
            // 参考姿态：SMD 第 0 帧（用于自动补全描述里没写的骨骼姿态）。
            let poses: Vec<SmdPose> = smd
                .reference_frame()
                .map(|f| f.poses.clone())
                .unwrap_or_default();
            let _ = pose_for; // 保留该辅助函数供后续「按名取姿态」使用

            models.push(CompiledModel {
                smd_path,
                name,
                poses,
                meshes,
                lods,
                eyeballs: Vec::new(),
                mesh_flexes: Vec::new(),
            });
        }
        bodyparts.push(CompiledBodyPart {
            name: bp.name.clone(),
            base: bp.base.unwrap_or(1),
            models,
        });
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    drop(_t_body);

    // ---- 动画池：`$animation` 一条，顺序 = 声明顺序 ----
    let mut seq_errors: Vec<CompileError> = Vec::new();
    // ⚠️ 这个池**先于序列**建，因为序列的 blend 格子只是**引用**它。
    // 官方 `g_panimation[]` 就是这么一个全局池
    // （`studiomdl.cpp:2400-2413` 的 `g_panimation[g_numani++]->index`）。
    //
    // `subtract` 也在这一步做掉 —— 它引用的是**另一个动画**，
    // 所以必须等池里全部读完之后再减（QC 的声明顺序与引用顺序无关）。
    let bone_index = desc.bone_index();
    // ---- 权重表（`$weightlist`）----
    //
    // 先解析成「逐骨骼权重数组」。`resolved` 与 `desc.weight_lists` 等长；
    // 索引 0 是**隐式默认表**（全 1）。
    //
    // `weights_of` 把「动画声明的表名」映射成实际数组，缺省 = 表 0。
    let resolved_weights = resolve_weight_lists(desc);
    let n_bones = desc.bones.len();
    let default_weights = default_weight_list(n_bones);
    // 每个动画的权重（下标与 `anims` 对齐，最后用它算序列权重）。
    let mut anim_weights: Vec<Vec<f32>> = Vec::with_capacity(desc.animations.len());
    let weights_of = |name: Option<&str>| -> Vec<f32> {
        match name {
            None => default_weights.clone(),
            Some(n) => match weight_list_index(desc, n) {
                // 下标从 1 起 ⟹ 数组下标减 1。
                Some(i) => resolved_weights[i - 1].clone(),
                // 未知名在 `validate()` 里已报错；这里回落到默认。
                None => default_weights.clone(),
            },
        }
    };
    let mut anims: Vec<crate::model::CompiledAnimation> = Vec::with_capacity(desc.animations.len());
    let mut anim_index: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(desc.animations.len());
    // 先只读原始帧（不减除），并记下减除参数。
    let mut pending_subtract: Vec<Option<(String, i32)>> = Vec::with_capacity(desc.animations.len());
    for (ai, a) in desc.animations.iter().enumerate() {
        let at = format!("animations[{ai}]");
        if anim_index.contains_key(a.name.as_str()) {
            seq_errors.push(CompileError {
                at: format!("{at}.name"),
                message: format!("动画名 {:?} 重复", a.name),
            });
            continue;
        }
        let p = resolve_smd_path(base_dir, &a.smd);
        let Some((mut frames, _smd)) = load_smd_frames(
            &p,
            desc,
            &bone_index,
            &format!("{at}.smd"),
            &mut seq_errors,
        ) else {
            continue;
        };
        // ---- 取帧区间（QC 的 `frames a b`，闭区间）----
        if let Some([lo, hi]) = a.frames {
            let n = frames.len() as i32;
            // 官方把越界值**夹到**源范围（`studiomdl.cpp:2250-2254`），
            // 只有 `end < start` 才报错（2256-2257）。
            let lo = lo.clamp(0, n - 1);
            let hi = hi.clamp(0, n - 1);
            if hi < lo {
                seq_errors.push(CompileError {
                    at: format!("{at}.frames"),
                    message: format!("结束帧 {hi} 早于起始帧 {lo}（源只有 {n} 帧）"),
                });
                continue;
            }
            frames = frames[lo as usize..=hi as usize].to_vec();
        }
        pending_subtract.push(a.subtract.as_deref().map(|s| (s.to_owned(), a.subtract_frame.unwrap_or(0))));
        anim_index.insert(a.name.as_str(), anims.len());
        anim_weights.push(weights_of(a.weight_list.as_deref()));
        anims.push(crate::model::CompiledAnimation {
            name: a.name.clone(),
            smd_path: p,
            fps: a.fps.unwrap_or(30.0),
            looping: a.looping,
            frames,
            // `subtract` 会让 `ProcessIKRules` 置 `STUDIO_DELTA`
            // （`simplify.cpp:163-166`），那又**抑制自动补的 IK 规则**。
            delta: a.subtract.is_some(),
            ik_rules: a.ik_rules.clone(),
            no_auto_ik: a.no_auto_ik,
            // 由下面的 `subtract` 循环填。
            pre_subtract_frames: None,
        });
    }
    // ---- 减除参考（QC 的 `subtract "x" f`）----
    //
    // 旋转走 `QuaternionSM(-1, src, dest)`，即 **`(−1·src) · dest`**
    // （`simplify.cpp:1102-1107`，`subtract` 自带 `STUDIO_POST`）。
    //
    // 同时把**减除前**的帧留一份（`pre_subtract`）—— 官方包围盒用的是
    // 未减除的姿态，见 `CompiledSequence::pre_subtract_frames`。
    let mut pre_subtract: Vec<Option<Vec<Vec<crate::smd::SmdPose>>>> =
        vec![None; anims.len()];
    for (i, sub) in pending_subtract.iter().enumerate() {
        let Some((ref_name, ref_frame)) = sub else {
            continue;
        };
        let Some(&j) = anim_index.get(ref_name.as_str()) else {
            seq_errors.push(CompileError {
                at: format!("animations[{i}].subtract"),
                message: format!(
                    "找不到参考动画 {ref_name:?}（subtract 引用的是 [[animations]] 里的**动画名**）"
                ),
            });
            continue;
        };
        let src = anims[j].frames.clone();
        let bf = (*ref_frame).max(0) as usize;
        if bf >= src.len() {
            seq_errors.push(CompileError {
                at: format!("animations[{i}].subtract_frame"),
                message: format!("参考动画 {ref_name:?} 只有 {} 帧，取不到第 {bf} 帧", src.len()),
            });
            continue;
        }
        // ⚠️ **先把减除前的帧留一份**给包围盒用（见
        // `CompiledAnimation::pre_subtract_frames` 的说明）。
        let before = anims[i].frames.clone();
        anims[i].pre_subtract_frames = Some(before.clone());
        pre_subtract[i] = Some(before);
        subtract_base_frames(&mut anims[i].frames, &src, bf);
    }
    if !seq_errors.is_empty() {
        return Err(seq_errors);
    }

    // ---- 序列：读每个序列的 SMD，取全部帧 ----
    let _t_seq = crate::prof::Span::new("sequences: 读 SMD 帧");
    let mut sequences = Vec::with_capacity(desc.sequences.len());
    for (si, s) in desc.sequences.iter().enumerate() {
        let at = format!("sequences[{si}]");

        // ---- `$declaresequence`：前向声明的**空壳** ----
        //
        // 官方 `Cmd_DeclareSequence` 只 `memset` 一条 `s_sequence_t` 再置
        // `STUDIO_OVERRIDE`，**不分配 `panim`、不读 SMD**。
        // 所以这里**必须**在所有「读 SMD / 解析动画」之前短路 ——
        // 否则会去读一个空路径。
        //
        // 落盘值与普通序列**处处不同**，但那不是特例代码，而是
        // 「`memset` 之后一个字段都没被赋值」的自然结果。实测表见
        // [`crate::model::Sequence::forward_declared`]。
        if s.forward_declared {
            sequences.push(crate::model::CompiledSequence {
                name: s.name.clone(),
                smd_path: std::path::PathBuf::new(),
                // 官方 `memset` 后 `fps = 0`（普通序列在 `Cmd_Sequence`
                // 里才被设成 30）。它不落盘，但保持一致以免误导。
                fps: 0.0,
                looping: false,
                // ⚠️ 这两个是**空壳与普通序列差别最大的地方**：
                // 普通序列 `activity = -1`、`fade = 0.2`，
                // 空壳全是 `memset` 的 0。
                activity: 0,
                activity_name: String::new(),
                activity_weight: 0,
                delta: false,
                frames: Vec::new(),
                // `groupsize = [0, 0]` ⟹ `cells` 空 ⟹ 写出器走空壳分支。
                cells: Vec::new(),
                blend_width: 0,
                blend_params: [None, None],
                auto_layers: Vec::new(),
                events: Vec::new(),
                fade_in: 0.0,
                fade_out: 0.0,
                forward_declared: true,
                no_auto_ik: false,
                ik_rules: Vec::new(),
                iklocks: Vec::new(),
                movements: Vec::new(),
                section_frames: 0,
                num_sections: 0,
                // ⚠️ **全 0，不是全 1** —— 见 `merge_weights` 的说明：
                // `groupsize = [0,0]` 让官方的 MAX 循环一次都不跑。
                weights: vec![0.0; n_bones],
                pre_subtract_frames: None,
                extra_flags: None,
            });
            continue;
        }

        // ---- blend 网格（`$sequence` 块里写了多个动画名）----
        //
        // ⚠️ **格子只是引用**，不是「每格一个 animdesc」。
        // 官方把 animdesc 放在全局池 `g_panimation[]` 里，`$sequence`
        // 里的裸名字先按名字查池（`studiomdl.cpp:2952-2959`），查到就
        // **复用同一个 animdesc**。实测 `v_autoshotgun.mdl`：27 个
        // seqdesc / 29 个 animdesc —— `idle` 的两格 `a_run` 共用一份，
        // `idle` 与 `idle_raw` 的 `a_idle` 共用一份。
        //
        // 所以这里只记录**动画下标**，帧数据在下面的动画池里统一建。
        // 单动画序列走下面的老路径（`s.smd`）。
        if !s.blends.is_empty() {
            let (width, height) = match blend_grid_size(s, &at, &mut seq_errors) {
                Some(v) => v,
                None => continue,
            };
            // 名字 → 动画池下标。查不到就报错（官方的「隐含动画」由
            // 单动画序列那条路径覆盖，blend 的格子必须已声明）。
            let mut cell_idx = Vec::with_capacity(s.blends.len());
            let mut ok = true;
            for (ci, nm) in s.blends.iter().enumerate() {
                match anim_index.get(nm.as_str()) {
                    Some(&i) => cell_idx.push(i),
                    None => {
                        seq_errors.push(e(
                            format!("{at}.blends[{ci}]"),
                            format!(
                                "找不到动画 {nm:?}（blend 的每一格都必须是 [[animations]] 里声明过的名字；\
                                 现有：{}）",
                                desc.animations
                                    .iter()
                                    .map(|a| a.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        ));
                        ok = false;
                    }
                }
            }
            if !ok {
                continue;
            }
            let _ = (width, height);

            // 参数轴：名字 → 下标，并算逐格取值。
            let mut params: [Option<crate::model::CompiledBlendParam>; 2] = [None, None];
            let grid = [width as usize, height as usize];
            let mut bad = false;
            for (pi, bp) in s.blend_params.iter().take(2).enumerate() {
                let idx = match resolve_pose_param_index(desc, &bp.parameter) {
                    Some(i) => i,
                    None => {
                        seq_errors.push(e(
                            format!("{at}.blend_params[{pi}].parameter"),
                            format!(
                                "找不到姿势参数 {:?}（[[model.pose_parameters]] 里有 {} 个：{}）",
                                bp.parameter,
                                desc.model.pose_parameters.len(),
                                desc.model
                                    .pose_parameters
                                    .iter()
                                    .map(|p| p.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        ));
                        bad = true;
                        continue;
                    }
                };
                params[pi] = Some(crate::model::CompiledBlendParam {
                    parameter_index: idx,
                    start: bp.start,
                    end: bp.end,
                    keys: blend_param_keys(bp.start, bp.end, grid[pi]),
                });
            }
            if bad {
                continue;
            }

            // 自动层：序列名 → 下标。时间量在这里就转成落盘值。
            //
            // 帧数取**本序列第一格**的（官方用 `panim[0][0]->numframes`，
            // `write.cpp:541-544`）。
            let nf = anims[cell_idx[0]].frames.len().max(1) as f32;
            let mut auto_layers = Vec::with_capacity(s.auto_layers.len());
            for (li, al) in s.auto_layers.iter().enumerate() {
                let Some(seq_idx) = desc
                    .sequences
                    .iter()
                    .position(|x| x.name == al.sequence)
                else {
                    seq_errors.push(e(
                        format!("{at}.auto_layers[{li}].sequence"),
                        format!("找不到序列 {:?}", al.sequence),
                    ));
                    bad = true;
                    continue;
                };
                auto_layers.push(crate::model::CompiledAutoLayer {
                    sequence: seq_idx as i16,
                    pose: al.pose,
                    flags: al.flags,
                    // `write.cpp:539-551`：不带 `STUDIO_AL_POSE` 时
                    // 四个量**除以 `numframes − 1`**（转 cycle）。
                    start: layer_time(al.start, al.flags, nf),
                    peak: layer_time(al.peak, al.flags, nf),
                    tail: layer_time(al.tail, al.flags, nf),
                    end: layer_time(al.end, al.flags, nf),
                });
            }
            if bad {
                continue;
            }

            let first = anims[cell_idx[0]].frames.clone();
            let nf_i = first.len() as i32;
            let sec_len = s.section_frames.unwrap_or(DEFAULT_SECTION_FRAMES);
            let sec_thr = s.section_threshold.unwrap_or(DEFAULT_SECTION_THRESHOLD);
            sequences.push(crate::model::CompiledSequence {
                name: s.name.clone(),
                smd_path: anims[cell_idx[0]].smd_path.clone(),
                fps: s.fps.unwrap_or(30.0),
                looping: s.looping,
                activity: -1,
                activity_name: s.activity.clone().unwrap_or_default(),
                activity_weight: s.activity_weight,
                delta: s.delta,
                frames: first,
                cells: cell_idx.clone(),
                blend_width: width,
                blend_params: params,
                auto_layers,
                events: s.events.clone(),
                fade_in: s.fade_in,
                fade_out: s.fade_out,
                no_auto_ik: s.no_auto_ik,
                ik_rules: s.ik_rules.clone(),
                iklocks: s.iklocks.clone(),
                movements: s.movements.clone(),
                section_frames: if sec_len > 0 && nf_i >= sec_thr { sec_len } else { 0 },
                num_sections: 0,
                // 本序列的权重 = 各格动画**逐骨骼取 MAX**（`simplify.cpp:302-318`）。
                weights: merge_weights(&cell_idx, &anim_weights, desc.bones.len()),
                // 取**第一格**减除前的帧（官方按格取，但 blend 的每一格
                // 都是独立的 animdesc，这里 `first` 也是第一格的）。
                pre_subtract_frames: pre_subtract
                    .get(cell_idx[0])
                    .and_then(|p| p.clone()),
                extra_flags: s.extra_flags,
                forward_declared: false,
            });
            continue;
        }

        // ---- 单动画序列：隐含动画 ----
        //
        // QC 里 `$sequence "reload" "reload.smd"` 与 `$sequence "x" "a_idle"`
        // 是**同一个语法**：块里给的是一个**名字**（token）。官方先按
        // 这个名字查 `$animation` 池（`studiomdl.cpp:2952-2959`）——
        //
        // * 查到 ⟹ **复用**那个 animdesc（不新建）；
        // * 查不到 ⟹ `Cmd_ImpliedAnimation` 新建一个，名字加 `@` 前缀。
        //
        // ⚠️ **隐含动画登记在 `@序列名` 下，不是文件名下。**
        // 所以 `$sequence "reload" "reload.smd"` 与
        // `$sequence "reload_layer" "reload.smd"` 是**两条独立动画**
        // （`@reload` / `@reload_layer`）—— 后者的 token 再去查池时
        // 找不到 `reload.smd`（池里没有这个名字），于是又建一个。
        // 早先本实现按**文件名**登记，于是这两条被错误地并成一条
        // （实测 `numlocalanim` 17 vs 官方 29）。
        let smd_path = resolve_smd_path(base_dir, &s.smd);
        let by_name = anim_index.get(s.smd.as_str()).copied();
        let (anim_ix, frames) = match by_name {
            Some(i) => (i, anims[i].frames.clone()),
            None => {
                let Some((frames, _smd)) = load_smd_frames(
                    &smd_path,
                    desc,
                    &bone_index,
                    &format!("{at}.smd"),
                    &mut seq_errors,
                ) else {
                    continue;
                };
                let i = anims.len();
                let name = format!("@{}", s.name);
                // 登记在**动画名**下，与官方一致。
                anim_index.insert(Box::leak(name.clone().into_boxed_str()), i);
                // 隐含动画的权重取**序列**的 `weightlist`
                // （官方 `Cmd_ImpliedAnimation` 建完动画后，序列的
                // `cmds[]` 里的 `CMD_WEIGHTS` 会作用到它）。
                anim_weights.push(weights_of(s.weight_list.as_deref()));
                anims.push(crate::model::CompiledAnimation {
                    name,
                    smd_path: smd_path.clone(),
                    fps: s.fps.unwrap_or(30.0),
                    looping: s.looping,
                    frames: frames.clone(),
                    delta: false,
                    ik_rules: s.ik_rules.clone(),
                    no_auto_ik: s.no_auto_ik,
                    pre_subtract_frames: None,
                });
                (i, frames)
            }
        };
        let nf = frames.len() as i32;
        let sec_len = s.section_frames.unwrap_or(DEFAULT_SECTION_FRAMES);
        let sec_thr = s.section_threshold.unwrap_or(DEFAULT_SECTION_THRESHOLD);
        sequences.push(crate::model::CompiledSequence {
            name: s.name.clone(),
            smd_path,
            fps: s.fps.unwrap_or(30.0),
            looping: s.looping,
            activity: -1,
            activity_name: s.activity.clone().unwrap_or_default(),
            activity_weight: s.activity_weight,
            delta: s.delta,
            frames,
            cells: vec![anim_ix],
            blend_width: 1,
            blend_params: [None, None],
            auto_layers: Vec::new(),
            events: s.events.clone(),
            fade_in: s.fade_in,
            fade_out: s.fade_out,
            no_auto_ik: s.no_auto_ik,
            ik_rules: s.ik_rules.clone(),
            iklocks: s.iklocks.clone(),
            movements: s.movements.clone(),
            section_frames: if sec_len > 0 && nf >= sec_thr { sec_len } else { 0 },
            num_sections: 0, // 下面按 section_frames 算（依赖 frames 数）
            pre_subtract_frames: pre_subtract.get(anim_ix).and_then(|p| p.clone()),
            extra_flags: s.extra_flags,
            forward_declared: false,
            weights: merge_weights(&[anim_ix], &anim_weights, n_bones),
        });
    }
    if !seq_errors.is_empty() {
        return Err(seq_errors);
    }
    drop(_t_seq);

    // 段表条目数 = `floor(numframes / sectionframes) + 2`（**不是 ceil**）。
    // 引擎索引的最大下标是 `numframes/sectionframes + 1`（`studio.cpp:345`）。
    for s in sequences.iter_mut() {
        s.num_sections = if s.section_frames > 0 {
            (s.frames.len() as i32 / s.section_frames) as usize + 2
        } else {
            0
        };
    }

    // ---- `$staticprop`：骨骼塌缩 + 动画塌陷 ----
    //
    // 顶点级的旋转与权重归零已在 [`smd_vertex_to_ir`] 里做掉（必须在
    // `unify_lods` 之前）。这里补上**骨骼表**与**动画**两处。
    //
    // 顺序与 studiomdl 一致：`MakeStaticProp()` 在 `RemapBones()`
    // （`simplify.cpp:4461`）里跑，而 `RemapBones()` 是 `SimplifyModel()`
    // 的第一步，早于 `UnifyLODs()` 与 `CalcSequenceBoundingBoxes()`。
    let desc = if desc.model.static_prop {
        collapse_static_prop(desc)
    } else {
        desc.clone()
    };

    // ---- 自动生成 hitbox（`SetupHitBoxes`，`simplify.cpp:6849-6973`）----
    //
    // 时序：`SetupHitBoxes()` 在 `simplify.cpp:7319` 被调用，**晚于**
    // `RemapBones()`（7231），所以静态道具的骨骼此时**已经**塌缩成
    // 单根 `static_prop` —— 自动 hitbox 自然就落在那一根上。
    //
    // 只在用户**没有**写显式 hitbox 时生成（`g_hitboxsets.Size() == 0`）。
    // 先构造出 `CompiledModelDesc`，因为参考姿态要用 [`resolve_bone_pose`]
    // 解析（它会回退到 SMD 第 0 帧并做 `canonical_euler` 规范化 ——
    // 必须与写进 `mstudiobone_t` 的值**完全一致**）。
    let mut compiled = CompiledModelDesc {
        desc,
        bodyparts,
        sequences,
        animations: anims,
        realigned: None,
        resolved_flex_rules: Vec::new(),
        resolved_flex_controller_ui: Vec::new(),
        resolved_mouths: Vec::new(),
        resolved_jiggle_bones: Vec::new(),
        resolved_quat_interp_bones: Vec::new(),
        // `physicsbone` 由 `main.rs` 在解析**碰撞 SMD** 之后填 ——
        // 编译前端看不到碰撞几何（那是另一份 SMD）。
        physics_bone: None,
    };

    // ---- flex 系列 / eyeball / mouth：把名字解析成下标、算骨骼空间量 ----
    //
    // 时序：**在 `RealignBones` 之后做**（eyeball 的 `up`/`forward`/`org`
    // 要用重排后的 `boneToPose` 逆变换，与自动 hitbox 同一份世界矩阵口径）。
    // 所以这里先占位，真正的解析在 `compiled.realigned` 定稿之后（见下）。

    // ---- 骨骼轴重对齐（`RealignBones`，`simplify.cpp:4224-4425`）----
    //
    // 时序：`RealignBones()` 在 `simplify.cpp:7237` 调用，**晚于**
    // `LinkIKChains()`（7233）—— 所以 `$ikchain` 推出来的 `childbone[]`
    // 此时已经就绪。它**早于** `SetupHitBoxes()`（7319），所以自动 hitbox
    // 用的是重排后的 `boneToPose`。
    let _t_realign = crate::prof::Span::new("realign + flex/jiggle 收尾");
    compiled.realigned = compute_realigned_poses(&compiled);

    // 动画帧也要搬进重排后的空间（`simplify.cpp:1527`）：
    //
    // ```cpp
    // ConcatTransforms( srcBoneToWorld[q], g_bonetable[k].srcRealign, destBoneToWorld[k] );
    // ```
    //
    // 即「源骨架的第 f 帧世界变换」右乘 `srcRealign` 就得到重排后的世界变换，
    // 再由它反解出新的**局部**姿态。漏掉这一步的症状：骨骼表是对的，
    // 但动画的骨骼位置整体错位 —— `hull`/`seqdesc` 包围盒、`rotscale`、
    // 动画链头全都跟着错，而**不会报任何错**。
    if compiled.realigned.is_some() {
        realign_sequence_frames(&mut compiled);
    }

    // ---- flex / eyeball / mouth 解析（在重排定稿后）----
    //
    // eyeball 的 `up`/`forward`/`org` 用 [`internal_bone_world`] 的世界矩阵
    // 逆变换 —— 与自动 hitbox / 姿态包围盒**同一份口径**（官方
    // `g_bonetable[k].boneToPose`，不是骨骼表最终矩阵）。
    //
    // ⚠️ VTA 的 flexdesc 注册必须在这里做（**早于** flexrule/eyeball 的
    // 名字解析），因为 `flex` 会**追加** flexdesc，而后续所有按名查表
    // 都依赖最终的下标。所以先注册 VTA 的 desc，再统一解析。
    resolve_vta_flexes(&mut compiled, base_dir)?;

    resolve_flex_eyeball_mouth(&mut compiled)?;

    // ---- jigglebone（`mstudiojigglebone_t`，L4D2 独有的 `$jigglebone`）----
    //
    // 程序化骨骼块夹在骨骼数组与 `bonecontroller` 之间（`write.cpp:214-285`），
    // 所以 `layout.rs` 的公式依赖这里的条数；解析必须在写出之前完成。
    resolve_jiggle_bones(&mut compiled)?;
    resolve_quat_interp_bones(&mut compiled)?;

    if compiled.desc.hitboxes.boxes.is_empty() {
        let auto = auto_hitboxes(&compiled);
        // **即使 `auto` 为空也要建 set** —— 官方在过滤前就建好了 set 并置了
        // 标志（`simplify.cpp:6884-6892` 在建 set 与置标志之后才过滤）。
        // 实测语料 166 个模型是「set 存在但 0 box」且**全部**带 `0x1` 标志。
        compiled.desc.hitboxes.set_name = Some("default".to_string());
        compiled.desc.hitboxes.boxes = auto;
        compiled.desc.hitboxes.autogenerated = true;
        // `simplify.cpp:6892`：自动生成路径置
        // `gflags |= STUDIOHDR_FLAGS_AUTOGENERATED_HITBOX`（**0x1**）。
        //
        // 注意它是 flags 的**最低位**，不是 0x2000。
        // 实测 `ip_official.mdl`（`ip.qc` 没写 `$hbox`）的 `flags == 0x1`
        // 正是它。
        let f = compiled.desc.model.extra_flags.unwrap_or(0);
        compiled.desc.model.extra_flags =
            Some(f | crate::mdl_writer::FLAG_AUTOGENERATED_HITBOX);
    }
    drop(_t_realign);
    Ok(compiled)
}

/// 把动画的每一帧搬到**重排后**的骨骼空间（`simplify.cpp:1527`）。
///
/// # 原理
///
/// `srcRealign[k] = srcWorld[k]⁻¹ ∘ newWorld[k]`，所以
/// `srcWorld[k] ∘ srcRealign[k] == newWorld[k]`。
/// 对动画的第 `f` 帧，把**源**骨架的世界变换右乘 `srcRealign[k]`，
/// 就得到「同一姿态在重排后骨架上的世界变换」；再由它和父骨骼的
/// 世界变换反解出新的局部姿态。
///
/// # 为什么不能只改骨骼表
///
/// 骨骼表存的是**参考姿态**，动画存的是**每帧相对参考姿态的增量**。
/// 重排换了局部基，增量也跟着换基 —— 只改一边会让两者不一致。
fn realign_sequence_frames(compiled: &mut CompiledModelDesc) {
    let Some(r) = compiled.realigned.clone() else {
        return;
    };
    let desc = &compiled.desc;
    let n = desc.bones.len();
    let bone_index = desc.bone_index();
    let parents: Vec<i32> = desc
        .bones
        .iter()
        .map(|b| match b.parent.as_deref() {
            Some(p) => bone_index.get(p).map(|v| *v as i32).unwrap_or(-1),
            None => -1,
        })
        .collect();

    for seq in &mut compiled.sequences {
        for frame in &mut seq.frames {
            if frame.len() < n {
                continue;
            }
            // 源骨架在这一帧的世界变换。
            let src_pos: Vec<[f32; 3]> = (0..n).map(|i| frame[i].position).collect();
            let src_rot: Vec<[f32; 3]> = (0..n).map(|i| frame[i].rotation).collect();
            let src_world = crate::bone_math::compute_world(&src_pos, &src_rot, &parents);
            // 搬到重排后的空间。
            let new_world: Vec<crate::bone_math::Matrix3x4> = (0..n)
                .map(|i| crate::bone_math::concat(&src_world[i], &r.src_realign[i]))
                .collect();
            // 反解局部姿态。
            for i in 0..n {
                let local = match parents[i] {
                    p if p >= 0 => crate::bone_math::concat(
                        &crate::bone_math::invert(&new_world[p as usize]),
                        &new_world[i],
                    ),
                    _ => new_world[i],
                };
                frame[i].position = [local[3], local[7], local[11]];
                frame[i].rotation = crate::bone_math::matrix_angles(&local);
            }
        }
    }
}

/// 计算骨骼轴重对齐后的局部姿态；不需要重对齐时返回 `None`。
///
/// # `childbone[]` 的两条来源（`simplify.cpp:4236-4286`）
///
/// 1. **`$ikchain`** —— 对每条链填相邻两段：
///    `childbone[link[0]] = link[1]`、`childbone[link[1]] = link[2]`。
///    链的三段由 [`crate::mdl_writer`] 同一套规则推出（末端 / 父 / 祖父）。
/// 2. **`$realignbones`** —— 额外把所有「父骨骼只有唯一子骨骼」的
///    `parent → child` 也填进去。
///
/// 两条路径**都**可能被触发，官方是依次执行（先 ikchain 后 realignbones），
/// 后面的赋值会覆盖前面的 —— 这里照做。
///
/// # 返回 `None` 的两种情形
///
/// - 两条路径都没触发（`childbone[]` 全 -1）；
/// - 触发了但**没有一根**骨骼满足判据（都已沿 +X）—— 此时重排是恒等变换，
///   返回 `Some(原值)` 与 `None` 等价，但省掉 `None` 分支的判断开销不重要，
///   真正的好处是**保持语义清晰**：`Some` 表示「官方确实跑了 RealignBones」。
fn compute_realigned_poses(compiled: &CompiledModelDesc) -> Option<crate::model::RealignedBones> {
    let desc = &compiled.desc;
    let n = desc.bones.len();
    if n == 0 {
        return None;
    }
    let bone_index = desc.bone_index();
    let parents: Vec<i32> = desc
        .bones
        .iter()
        .map(|b| match b.parent.as_deref() {
            Some(p) => bone_index.get(p).map(|v| *v as i32).unwrap_or(-1),
            None => -1,
        })
        .collect();

    // `bPreAligned`（`simplify.cpp:4299`）：写了 `$definebone` 的骨骼被
    // `RealignBones` **整个跳过**。判定规则见
    // [`crate::model::Bone::is_pre_aligned`]。
    let pre_aligned: Vec<bool> = desc.bones.iter().map(|b| b.is_pre_aligned()).collect();
    // 显式 `srcRealign`（`$definebone` 的**后 6 个数字**，`simplify.cpp:3652`）。
    let explicit: Vec<Option<crate::bone_math::Matrix3x4>> =
        desc.bones.iter().map(|b| b.explicit_src_realign()).collect();

    let mut childbone = vec![-1i32; n];
    let mut any = false;

    // 1) `$ikchain`（`LinkIKChains`，`simplify.cpp:5604-5638`）。
    for ch in &desc.ikchains {
        let Some(&tip) = bone_index.get(ch.bone.as_str()) else {
            continue;
        };
        let Ok(mid) = usize::try_from(parents[tip]) else {
            continue;
        };
        let Ok(root) = usize::try_from(parents[mid]) else {
            continue;
        };
        // 与写出器同序：link[0]=祖父、link[1]=父、link[2]=末端。
        childbone[root] = mid as i32;
        childbone[mid] = tip as i32;
        any = true;
    }

    // 2) `$realignbones`（`simplify.cpp:4261-4286`）。
    if desc.model.realign_bones {
        let mut children = vec![0i32; n];
        for &p in &parents {
            if p >= 0 {
                children[p as usize] += 1;
            }
        }
        for (k, &p) in parents.iter().enumerate() {
            if p >= 0 && children[p as usize] == 1 {
                childbone[p as usize] = k as i32;
                any = true;
            }
        }
    }

    // 显式 `srcRealign`（`$definebone` 的 12 数字形式）**本身**也要搬运动画帧，
    // 哪怕一根骨骼都没被重排 —— 所以它单独就能让本函数返回 `Some`。
    if explicit.iter().any(|e| e.is_some()) {
        any = true;
    }

    if !any {
        return None;
    }

    // 原始局部姿态：**不能**走 `resolve_bone_pose`（它会读 `realigned_poses`，
    // 而此时正是 `None`，所以其实是安全的；但显式取原始值更能表明意图）。
    let orig: Vec<([f32; 3], [f32; 3])> = (0..n).map(|i| resolve_bone_pose(desc, compiled, i)).collect();
    let positions: Vec<[f32; 3]> = orig.iter().map(|p| p.0).collect();
    let rotations: Vec<[f32; 3]> = orig.iter().map(|p| p.1).collect();

    let (poses, mut src_realign) = crate::bone_math::realign_bones(
        &positions,
        &rotations,
        &parents,
        &childbone,
        &pre_aligned,
    );

    // 显式给出的 `srcRealign` **覆盖**推导值。
    //
    // 官方两条路径合起来正是这个效果：`simplify.cpp:3652` 把 importbone 的
    // `srcRealign` 拷进骨骼表，而 4390-4401 的推导循环对 pre-aligned 骨骼
    // **整个跳过** —— 于是文件里留下的就是 `$definebone` 给的那个矩阵。
    //
    // 反过来说：**pre-aligned 骨骼的 `srcRealign` 不是** `srcWorld⁻¹ ∘ newWorld`，
    // 因为它的世界矩阵压根没被重排过（推导出来会恒等于单位阵）。
    for (k, m) in explicit.iter().enumerate() {
        if let Some(m) = m {
            src_realign[k] = *m;
        }
    }

    // 一根骨骼都**没真正动过**（例如全部骨骼都是 pre-aligned，`ipq2`）时
    // 返回 `None`：
    //
    // - `src_realign` 全是单位阵，`poses` 与原值逐位相同 —— 语义上与
    //   「官方跑了 `RealignBones` 但什么都没改」等价；
    // - 但保留 `Some` 会让 `realign_sequence_frames` 白跑一遍，把动画帧
    //   从 `world` 反解回 `local`，**凭空引入一次浮点往返误差**。
    //
    // 所以这里宁可返回 `None`。（触发过重排、或给了显式 `srcRealign` 时
    // 仍然返回 `Some`。）
    let untouched = src_realign
        .iter()
        .all(|m| m.iter().zip(crate::bone_math::IDENTITY.iter()).all(|(a, b)| a == b))
        && poses.iter().zip(orig.iter()).all(|(a, b)| a == b);
    if untouched {
        return None;
    }

    Some(crate::model::RealignedBones {
        poses,
        src_realign,
    })
}

/// 自动生成 hitbox（`SetupHitBoxes`，`simplify.cpp:6884-6973`）。
///
/// # 算法（逐步对应源码）
///
/// 1. **bbox 起点**（`simplify.cpp:6899-6909`）：`g_bUseBoneInBBox` 在
///    `studiomdl.cpp:57` 默认 **true**，所以每根骨骼的 bbox 从 `(0,0,0)` 起步。
///    只有 QC 写了 `$skipboneinbbox`（`Cmd_SkipBoneInBBox`，
///    `studiomdl.cpp:5706`）才改成 `±9999`。
///    > 起点为 0 意味着「原点恒在 bbox 内」—— 实测产物里 `bbmin` 经常
///    > 出现 `0.00`，就是这个原因。
///    >
///    > 语料里 3104 个自动生成 hitbox 的模型有 **21 个**是 `±9999` 形态，
///    > 用「原点是否在 box 内」可干净二分（3082 全含 / 21 全不含 / 1 混合），
///    > 且那 21 个**全部**是 `static_prop` 碎片模型
///    > （`probe_skipboneinbbox.js`）。
/// 2. **遍历所有顶点，对每一组权重骨骼都算**（`simplify.cpp:6920-6935`）：
///    ```cpp
///    for (n = 0; n < globalBoneweight.numbones; n++) {
///        k = globalBoneweight.bone[n];
///        VectorITransform( vertex[j].position, g_bonetable[k].boneToPose, p );
///        // 并入骨骼 k 的 bbox
///    }
///    ```
///    注意是**每一组**权重都算，不是只算主导骨骼。
/// 3. **并入子骨骼位置**（`simplify.cpp:6937-6948`）：对每根有 parent 的
///    骨骼 k，把 `g_bonetable[k].pos`（子骨骼在**父空间**的局部位移）
///    并入**父骨骼** j 的 bbox。
/// 4. **过滤**（`simplify.cpp:6950-6955`）：只保留三轴厚度**都 > 1** 的骨骼
///    （`bmin[a] < bmax[a] - 1`），按**骨骼下标顺序**生成 hitbox。
///    一个都没通过时 set 仍存在但 `numhitboxes == 0` —— 语料里有 166 个
///    这样的模型，是**合法状态**。
///
/// # 矩阵方向
///
/// 源码写的是 `VectorITransform(pos, g_bonetable[k].boneToPose)`，但
/// `g_bonetable` 是**内部**结构 `s_bone_t`，其 `boneToPose` 与**文件里**的
/// `mstudiobone_t.poseToBone`（偏移 `0x60`）**互为逆矩阵**。所以
/// `VectorITransform(pos, boneToPose) ≡ pos × poseToBone` —— 对**文件矩阵**
/// 做普通正向变换。
///
/// 这一条已用 `dump_hboxes` 的**内部真值**验证：`docs/_probe/verify_hbox_algo.js`
/// 在受控实验 `iph1` 上 **4/4 逐字段命中**（含 3 组权重混合与链式父子骨骼）。
///
/// # 权重口径（澄清 PROGRESS.md 的一处误判）
///
/// 早先怀疑「VVD 只有 3 组权重、而内部 `globalBoneweight` 上限 4」是
/// 12% 不符的原因。**不是**：
///
/// - `studiomdl.h:36` 的 `MAXSTUDIOBONEWEIGHTS` 就是 **3**；
/// - `v1support.cpp:174` 在解析 SMD 时已经调
///   `SortAndBalanceBones(iCount, MAXSTUDIOBONEWEIGHTS, ...)` 裁到 3 组
///   （并且丢掉权重 < 0.05 的轴）；
/// - `UnifyLODs.cpp:1238` 的 `Assert(... <= 4)` 只在「骨骼折叠后不同局部
///   骨骼映射到同一全局骨骼并累加」（`simplify.cpp:5210-5215`）时才可能触及。
///
/// 所以 **VVD 的 3 组就是全部权重**。用 VVD 顶点重算的实测命中率是
/// **391/392**（`verify_autohitbox_recheck.js`），唯一那个「失败」样本
/// 经查是**脚本自己多转了一次** `Rz(90°)`（它是 `static_prop`，几何已被
/// 旋转过）—— 即算法本身 **392/392** 正确。
fn auto_hitboxes(compiled: &CompiledModelDesc) -> Vec<crate::model::Hitbox> {
    let desc = &compiled.desc;
    let n = desc.bones.len();
    if n == 0 {
        return Vec::new();
    }

    // 参考姿态：必须与写进 `mstudiobone_t` 的值**完全一致**，所以走同一个
    // [`resolve_bone_pose`]（它会回退到 SMD 第 0 帧并做 `canonical_euler`）。
    let poses: Vec<([f32; 3], [f32; 3])> = (0..n).map(|i| resolve_bone_pose(desc, compiled, i)).collect();
    let parents: Vec<i32> = desc
        .bones
        .iter()
        .map(|b| match b.parent.as_deref() {
            Some(p) => desc
                .bone_index()
                .get(p)
                .map(|v| *v as i32)
                .unwrap_or(-1),
            None => -1,
        })
        .collect();

    // bbox 起点：`g_bUseBoneInBBox` 默认 **true** → 全 0 起步。
    // 置了 `$skipboneinbbox` 则从 ±9999 起步（`simplify.cpp:6899-6909`）。
    let start_at_zero = !desc.model.skip_bone_in_bbox;
    let init = if start_at_zero { 0.0f32 } else { 9999.0 };
    let init_max = if start_at_zero { 0.0f32 } else { -9999.0 };
    let mut bmin = vec![[init; 3]; n];
    let mut bmax = vec![[init_max; 3]; n];

    // 世界矩阵走 **[源骨架] ∘ `srcRealign`**，**不是**骨骼表最终的参考姿态
    // —— 见 [`source_bone_pose`] 的说明。
    let hit_ptb: Vec<crate::bone_math::Matrix3x4> = internal_bone_world(compiled, &parents)
        .iter()
        .map(crate::bone_math::invert)
        .collect();

    for bp in &compiled.bodyparts {
        for m in &bp.models {
            for mesh in &m.meshes {
                for v in &mesh.vertices {
                    for pair in &v.bones {
                        let k = pair[0] as usize;
                        if k >= n {
                            continue;
                        }
                        // `VectorITransform(pos, boneToPose)` 对**文件矩阵**
                        // 等价于正向 `VectorTransform`。
                        let p = transform_point(&hit_ptb[k], v.pos);
                        for a in 0..3 {
                            if p[a] < bmin[k][a] {
                                bmin[k][a] = p[a];
                            }
                            if p[a] > bmax[k][a] {
                                bmax[k][a] = p[a];
                            }
                        }
                    }
                }
            }
        }
    }

    // 并入子骨骼位置（子骨骼在父空间的局部位移 → 父骨骼的 bbox）。
    for k in 0..n {
        let j = parents[k];
        if j < 0 {
            continue;
        }
        let j = j as usize;
        let pos = poses[k].0;
        for a in 0..3 {
            if pos[a] < bmin[j][a] {
                bmin[j][a] = pos[a];
            }
            if pos[a] > bmax[j][a] {
                bmax[j][a] = pos[a];
            }
        }
    }

    // 过滤：三轴厚度都 > 1。
    let mut out = Vec::new();
    for k in 0..n {
        if bmin[k][0] < bmax[k][0] - 1.0
            && bmin[k][1] < bmax[k][1] - 1.0
            && bmin[k][2] < bmax[k][2] - 1.0
        {
            out.push(crate::model::Hitbox {
                bone: desc.bones[k].name.clone(),
                group: Some(0),
                bbmin: bmin[k],
                bbmax: bmax[k],
                name: None,
            });
        }
    }
    out
}

/// 每根骨骼的**渲染包围盒**（`SetupFullBoneRenderBounds`，`simplify.cpp:7014`）。
///
/// # 用途
///
/// `CalcSequenceBoundingBoxes()`（`simplify.cpp:7049`）在算每个序列的包围盒时，
/// 除了蒙皮顶点，还会把每根骨骼的渲染包围盒用 `bonetransform` 变换后**并入**
/// （`simplify.cpp:7120-7130`）。这个「渲染包围盒」由两部分构成：
///
/// 1. **自动 hitbox 的 bbox** —— 即 `g_bonetable[i].bmin/bmax`
///    （`SetupHitBoxes` 填的，见 [`auto_hitboxes`]）；
/// 2. **显式 hitbox** —— `simplify.cpp:7030-7045` 把每个 hitbox 的
///    `bmin/bmax` 按骨骼并进去。
///
/// # 实测印证
///
/// `ip`（无 `$hbox`）：几何 z ∈ [-8,-8]，自动 hitbox 因**全 0 起步**而得到
/// `bbmax[2] == 0`。于是序列包围盒的 `bbmax[2]` 是 **0** 而不是 **-8** ——
/// 官方 `ip_official.mdl` 的 `hull_max = [8,8,0]` 正是这么来的。
pub fn bone_render_bounds(
    desc: &ModelDesc,
    compiled: &CompiledModelDesc,
) -> Vec<([f32; 3], [f32; 3])> {
    let n = desc.bones.len();
    let mut out = vec![([0.0f32; 3], [0.0f32; 3]); n];
    let index = desc.bone_index();

    // 1) 自动 hitbox 的 bbox（= `g_bonetable[].bmin/bmax`）。
    if desc.hitboxes.autogenerated {
        let auto = auto_hitboxes(compiled);
        for hb in &auto {
            if let Some(&k) = index.get(hb.bone.as_str()) {
                for a in 0..3 {
                    out[k].0[a] = out[k].0[a].min(hb.bbmin[a]);
                    out[k].1[a] = out[k].1[a].max(hb.bbmax[a]);
                }
            }
        }
    }

    // 2) 显式 hitbox。
    for hb in &desc.hitboxes.boxes {
        if let Some(&k) = index.get(hb.bone.as_str()) {
            for a in 0..3 {
                out[k].0[a] = out[k].0[a].min(hb.bbmin[a]);
                out[k].1[a] = out[k].1[a].max(hb.bbmax[a]);
            }
        }
    }
    out
}

/// 用 `matrix3x4`（行主序 12 个 f32）变换一个点。
fn transform_point(m: &[f32; 12], v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2] + m[3],
        m[4] * v[0] + m[5] * v[1] + m[6] * v[2] + m[7],
        m[8] * v[0] + m[9] * v[1] + m[10] * v[2] + m[11],
    ]
}

/// 一个序列的**摆好姿势**包围盒（`CalcSequenceBoundingBoxes`，
/// `simplify.cpp:7049-7163`）。
///
/// # 为什么要算它
///
/// `write.cpp:2071-2087` 把 `g_sequence[0].bmin/bmax` 直接写进头部的
/// `hull_min/hull_max`。所以 hull **不是**静止姿势的顶点 AABB ——
/// 它来自**逐帧摆姿势后**的顶点包围盒，再加上每根骨骼的渲染包围盒。
///
/// 用静止姿势 AABB 会在有动画时明显偏小，而且**静态道具也会偏**：
/// 自动 hitbox 的 bbox 从全 0 起步，会撑大这个盒子。
///
/// # 算法（逐帧）
///
/// 对序列的**每一帧**：
///
/// 1. 逐骨骼算 `bonetransform[k]`：用该帧的局部姿态
///    （`AngleMatrix(sanim[j][k].rot, sanim[j][k].pos)`）沿父链
///    `ConcatTransforms` 累乘；
/// 2. `posetransform[k] = bonetransform[k] ∘ inverse(boneToPose[k])`
///    —— 即「该帧世界矩阵」与「参考姿态世界矩阵」的相对变换；
/// 3. 把每根骨骼的渲染包围盒用 `bonetransform[k]` 变换后并入
///    （`simplify.cpp:7120-7130` 的 `TransformAABB`）；
/// 4. 把每个顶点用**它绑定的每一根**骨骼的 `posetransform` 变换后
///    加权求和，并入（`simplify.cpp:7141-7158`）。
///    注意源码这里是「先逐骨骼变换再按权重累加」，**不是**「先蒙皮再变换」。
///
/// # `boneToPose`
///
/// `g_bonetable[k].boneToPose` 是**内部**结构里的世界矩阵，与文件里的
/// `mstudiobone_t.poseToBone`（偏移 `0x60`）**互为逆**。
/// 这里用 [`crate::bone_math::compute_pose_to_bone`] 拿到与写出器**完全一致**
/// 的那一份，再取逆得到世界矩阵。
/// 骨骼父链（`-1` = 根），下标与 [`ModelDesc::bones`] 一致。
///
/// 父名查不到时按根处理 —— 与写出器的容错一致（`validate()` 会先报错）。
pub fn bone_parents(desc: &ModelDesc) -> Vec<i32> {
    let index = desc.bone_index();
    desc.bones
        .iter()
        .map(|b| match b.parent.as_deref() {
            Some(p) => index.get(p).map(|v| *v as i32).unwrap_or(-1),
            None => -1,
        })
        .collect()
}

/// 一帧的**世界**变换 —— 官方 `CalcBoneTransforms`（`simplify.cpp:4533-4589`）。
///
/// # 根骨骼要先套一层 `panim->rotation`（= `g_defaultrotation`）
///
/// `BuildRawTransforms`（`simplify.cpp:1432-1486`）对根骨骼
/// （`localBone[k].parent == -1`）做：
///
/// ```cpp
/// AngleMatrix( rotate, rootxform );          // rotate = panim->rotation
/// VectorSubtract( pos, shift, tmp );
/// VectorRotate( tmp, rootxform, pos );       // 平移也转
/// AngleMatrix( rot, m );
/// ConcatTransforms( rootxform, m, bonematrix );
/// MatrixAngles( bonematrix, rot );
/// ```
///
/// 而 `panim->rotation` 在 `studiomdl.cpp:2427` 被设为
/// `g_defaultrotation` = `RadianEuler(0, 0, π/2)`。
///
/// **实测印证**（`ipe1`，2 条序列、含真实动画）：不加这一层时
/// 算出的 `seq[1]` 盒子是 `[-19.95,-19.8,0]..[10,20,7]`，
/// 而官方是 `[-20,-19.95,0]..[19.8,10,7]` —— 恰好差一个 `Rz(90°)`
/// （`(x,y,z) → (-y,x,z)`：`-19.95 ↔ -19.8`、`20 → 19.8`…）。
///
/// `$staticprop` 时 `panim->rotation` 被置 0（`simplify.cpp:3379`），
/// 所以那一层**不套** —— 与几何已被旋转的事实一致。
///
/// # 为什么 `frame` 里的是「局部」姿态
///
/// [`crate::compile::realign_sequence_frames`] 已经把 `srcRealign` 折进
/// 每一帧的局部姿态里，所以这里对根骨骼**再**左乘一次 `Rz(90°)` 就等价于
/// 官方的 `sanim`（它本身也是「父相对」的：`ConvertAnimation` 用
/// `inverse(destBoneToWorld[parent]) ∘ destBoneToWorld[k]` 反解出来）。
/// 全局左乘一个旋转不改变任何非根骨骼的局部姿态，所以两种约定只在根上分叉。
pub fn frame_worlds(
    desc: &ModelDesc,
    parents: &[i32],
    frame: &[crate::smd::SmdPose],
) -> Vec<crate::bone_math::Matrix3x4> {
    let locals: Vec<crate::bone_math::Matrix3x4> = (0..parents.len())
        .map(|k| {
            let p = frame.get(k).copied().unwrap_or(crate::smd::SmdPose {
                bone: k as i32,
                position: [0.0; 3],
                rotation: [0.0; 3],
            });
            // `sanim[j][k].rot` 是**弧度**（`s_bone_t` 与骨骼表同构）。
            crate::bone_math::local_transform(p.position, p.rotation)
        })
        .collect();
    frame_worlds_from_locals(desc, parents, &locals)
}

/// 由**已算好的局部矩阵**求世界矩阵（官方 `CalcBoneTransforms` 的
/// `ConcatTransforms` 那半段）。
///
/// 抽出来是因为 [`delta_local_matrices`] 的局部矩阵**不是**由欧拉角直接建的，
/// 而是「四元数重建」的产物 —— 两条路径必须共用同一段层级合成代码，
/// 否则根骨骼的 `Rz(90°)` 偏置很容易在一边漏掉。
pub fn frame_worlds_from_locals(
    desc: &ModelDesc,
    parents: &[i32],
    locals: &[crate::bone_math::Matrix3x4],
) -> Vec<crate::bone_math::Matrix3x4> {
    let root_rot = if desc.model.static_prop {
        None
    } else {
        Some(crate::bone_math::angle_matrix([
            0.0,
            0.0,
            std::f32::consts::FRAC_PI_2,
        ]))
    };
    let mut world: Vec<crate::bone_math::Matrix3x4> = Vec::with_capacity(parents.len());
    for (k, &parent) in parents.iter().enumerate() {
        let mut local = locals.get(k).copied().unwrap_or_else(|| {
            crate::bone_math::local_transform([0.0; 3], [0.0; 3])
        });
        if parent == -1 && let Some(rr) = &root_rot {
            // `ConcatTransforms(rootxform, m, bonematrix)` —— 左乘。
            local = crate::bone_math::concat(rr, &local);
        }
        let m = match parent {
            -1 => local,
            par => crate::bone_math::concat(&world[par as usize], &local),
        };
        world.push(m);
    }
    world
}

/// **`STUDIO_DELTA` 动画的逐骨骼局部矩阵重建** —— 官方
/// `CalcBoneTransforms` 的 `else` 分支（`simplify.cpp:4562-4578`）。
///
/// # 官方代码
///
/// ```c
/// if (!(panimation->flags & STUDIO_DELTA)) {
///     AngleMatrix( panimation->sanim[frame][k].rot, panimation->sanim[frame][k].pos, bonematrix );
/// } else {
///     AngleQuaternion( pbaseanimation->sanim[0][k].rot, q1 );   // ← 基准动画**第 0 帧**
///     AngleQuaternion( panimation->sanim[frame][k].rot, q2 );  // ← 本帧存的**增量**
///     float s = panimation->weight[k];                          // ← $weightlist，缺省 1
///     QuaternionMA( q1, s, q2, q3 );
///     p3 = pbaseanimation->sanim[0][k].pos + s * panimation->sanim[frame][k].pos;
///     AngleMatrix( q3, p3, bonematrix );
/// }
/// ```
///
/// # 为什么这是个真 bug（不是「等价实现」）
///
/// `subtract`（`subtractBaseAnimations`）把动画变成**增量**：
/// `delta.rot = conj(src.rot) · raw.rot`、`delta.pos = raw.pos − src.pos`。
/// 重建在 `s = 1` 时**精确抵消**它：
/// `q1·q2 = src·conj(src)·raw = raw`、`p3 = src.pos + (raw.pos − src.pos) = raw.pos`。
///
/// 所以官方产物对「有无 `subtract`」**不变**。mdlc 之前把增量直接当完整
/// 姿态喂给 [`frame_worlds`]，增量近零 ⟹ 所有骨骼塌到原点 ⟹ IK 误差 ≈ 0。
///
/// **实测判据**（`docs/_probe/ab_iksub_mdlc.js`，同一份几何只改有无
/// `subtract`，对照官方 `@idle` 的 `mstudiocompressedikerror_t`）：
///
/// | 通道 | 官方（两种情况**相同**） | mdlc 修复前 | mdlc 修复后 |
/// |---|---|---|---|
/// | `pos.x` 采样 | `[2674, 2274, 1826]` | `[0, 0, 0]` | `[2674, 2274, 1826]` |
/// | 通道字节数 | `[8, 8, 4, 4, 4, 4]` | `[4, 4, 4, 4, 4, 4]` | `[8, 8, 4, 4, 4, 4]` |
///
/// # `s` 恒为 1
///
/// `s = panimation->weight[k]` 来自 `$weightlist`（`setAnimationWeight`，
/// `simplify.cpp:1721-1729`）。mdlc 不实现 `$weightlist`，缺省权重全 1
/// （`buildAnimationWeights` 把根设为 1、子骨骼沿父链继承）。
///
/// ⚠️ 即便 `s == 1`，**也不能**直接返回源姿态 —— `QuaternionMA` 内部的
/// `QuaternionScale` / `QuaternionNormalize` 会引入约 `1e-7` 的浮点差，
/// 在压缩误差的量化边界上足以翻转一个采样值。必须走完整条路径。
///
/// # 基准动画是 `g_panimation[0]`
///
/// 官方调用的是单参数版 `CalcBoneTransforms(panimation, frame, …)`
/// （`simplify.cpp:4533-4536`），它固定传 `g_panimation[0]` —— 即 QC 里
/// **第一条** `$animation`，**不是** `subtract` 引用的那条。
/// 所以基准取 [`CompiledModelDesc::animations`]`[0]` 的第 0 帧。
pub fn delta_local_matrices(
    base_frame0: &[crate::smd::SmdPose],
    frame: &[crate::smd::SmdPose],
) -> Vec<crate::bone_math::Matrix3x4> {
    /// `panimation->weight[k]` —— mdlc 不实现 `$weightlist`，恒 1。
    const WEIGHT: f32 = 1.0;

    let n = base_frame0.len().max(frame.len());
    (0..n)
        .map(|k| {
            let empty = crate::smd::SmdPose {
                bone: k as i32,
                position: [0.0; 3],
                rotation: [0.0; 3],
            };
            let b = base_frame0.get(k).copied().unwrap_or(empty);
            let f = frame.get(k).copied().unwrap_or(empty);
            let q1 = crate::bone_math::angle_quaternion(b.rotation);
            let q2 = crate::bone_math::angle_quaternion(f.rotation);
            let q3 = crate::bone_math::quaternion_ma(q1, WEIGHT, q2);
            // `p3 = pbaseanimation->sanim[0][k].pos + s * panimation->sanim[frame][k].pos`
            let mut p3 = b.position;
            for (dst, src) in p3.iter_mut().zip(f.position.iter()) {
                *dst += WEIGHT * src;
            }
            crate::bone_math::quaternion_local_transform(q3, p3)
        })
        .collect()
}

/// [`frame_worlds`] 的 `STUDIO_DELTA` 版本 —— 先重建局部矩阵再合成世界矩阵。
pub fn delta_frame_worlds(
    desc: &ModelDesc,
    parents: &[i32],
    base_frame0: &[crate::smd::SmdPose],
    frame: &[crate::smd::SmdPose],
) -> Vec<crate::bone_math::Matrix3x4> {
    let locals = delta_local_matrices(base_frame0, frame);
    frame_worlds_from_locals(desc, parents, &locals)
}

pub fn sequence_pose_bounds(
    desc: &ModelDesc,
    compiled: &CompiledModelDesc,
    seq_index: usize,
    render_bounds: &[([f32; 3], [f32; 3])],
) -> Option<([f32; 3], [f32; 3])> {
    let seq = compiled.sequences.get(seq_index)?;
    let n = desc.bones.len();
    if n == 0 || seq.frames.is_empty() {
        return None;
    }

    // 父链（与写出器同源）。
    //
    // 注意这里**不**再取 `resolve_bone_pose` —— 参考世界矩阵改走
    // [`source_world`]（源骨架 + `srcRealign`），见下面的 `ref_world`。
    let parents: Vec<i32> = bone_parents(desc);

    // 参考世界矩阵走 **[源骨架] ∘ `srcRealign`**（= 官方的
    // `g_bonetable[k].boneToPose`），**不是**骨骼表最终的参考姿态 ——
    // 见 [`source_bone_pose`] / [`source_world`]。
    //
    // 必须与上面 `world[]`（帧世界矩阵）**同源**：`posetransform` 是二者的
    // 相对变换，不同源会凭空多出一次平移（实测 `ipq2` 顶点被推远 2）。
    let ref_world: Vec<crate::bone_math::Matrix3x4> = internal_bone_world(compiled, &parents);

    let mut bmin = [f32::INFINITY; 3];
    let mut bmax = [f32::NEG_INFINITY; 3];

    // `$staticprop`：动画已被 `MakeStaticProp()` 压成 **1 条 1 帧的身份动画**
    // （`simplify.cpp:3362-3380`）——
    //   `rawanim[0][0].pos = 0`、`rawanim[0][0].rot = 0`、
    //   `g_panimation[0]->rotation = 0`、`numframes = 1`
    // 所以包围盒必须用**那一帧**算，而不是原始序列的逐帧姿态。
    //
    // 实测（`ipe2`）：用原始 2 帧算 `seq[0]` 会得到 `bbmax=[0,10,12]`，
    // 而官方是 `[0,10,7]` —— 后者正是「只有身份帧」的结果
    // （`12` 来自 `ipeanim.smd` 第 1 帧把 root 抬到 z=5）。
    let identity_frames: Vec<Vec<crate::smd::SmdPose>> = if desc.model.static_prop {
        let mut row = vec![
            crate::smd::SmdPose {
                bone: 0,
                position: [0.0; 3],
                rotation: [0.0; 3],
            };
            n
        ];
        for (k, p) in row.iter_mut().enumerate() {
            p.bone = k as i32;
        }
        vec![row]
    } else {
        Vec::new()
    };

    // ⚠️ **blend 序列的包围盒是「每一格的并集」。**
    //
    // 官方分两步（`simplify.cpp:7049-7202` 的 `CalcSequenceBoundingBoxes`）：
    //
    // ```c
    // // ① 逐动画算自己的 bmin/bmax（遍历该动画的所有帧）
    // for (i = 0; i < g_numani; i++) { ... g_panimation[i]->bmin = bmin; }
    //
    // // ② 逐**序列**把所有格的盒子求并
    // for (i = 0; i < g_sequence.Count(); i++)
    //   for (j = 0; j < g_sequence[i].groupsize[0]; j++)
    //     for (k = 0; k < g_sequence[i].groupsize[1]; k++) {
    //       s_animation_t *panim = g_sequence[i].panim[j][k];
    //       if (panim->bmin[0] < bmin[0]) bmin[0] = panim->bmin[0];
    //       ...
    //     }
    // ```
    //
    // 早先本实现只用了**第一格**的帧（`seq.frames` 就是第一格的帧），
    // 于是 miku `look_poses`（3 格 → `look_down`/`look_mid`/`look_up`）
    // 的包围盒只有 `look_down` 一格的：
    //
    // | | bbmin | bbmax |
    // |---|---|---|
    // | 官方 | `[-9.681, -15.120, -13.719]` | `[37.516, 5.152, 56.902]` |
    // | mdlc（修前） | `[-5.265, -27.594, -1.5]` | `[3.230, 4.423, 1.5]` |
    //
    // 注意三格的帧数可能不同（官方按**每格自己的** `numframes` 遍历）。
    //
    // ⚠️ **仍未闭合**：miku `look_poses`（seq[0]，3 个 `subtract` 格）的
    // 包围盒与官方差很多（span `[32.05, 9.78, 3.05]` vs `[47.20, 20.27, 70.62]`）。
    //
    // 已**证否**的假设（都实测过，记录在此避免重走）：
    //
    // 1. 「只转根位置、不转根旋转」—— 推理是 `subtract` 在已带 `Rz90` 的
    //    值上做差，旋转里相消、位置里留下一次。改完 bbmin 从
    //    `[-6.551, -27.594, -1.550]` 变成 `[-27.594, -3.230, -1.550]`，
    //    只是把错误换了个轴，离官方更远。
    // 2. 「`write.cpp:2071-2078` 的 `CollisionModel_ExpandBBox` 撑开了 seq[0]」
    //    —— 但官方 `look_poses.bbmin[1] = -15.120` **小于**官方
    //    `hull_min[1] = -12.029`，而 hull 才是被撑开的那个，所以不是。
    // 3. 「每格自己的盒子求并」—— **这条是对的**（`simplify.cpp:7184-7196`），
    //    已修（`idle`/`idle_raw` 因此与官方一致到 1e-6），但不足以解释
    //    `look_poses`：3 个 `subtract` 格的增量都很小，并起来仍然很小。
    //
    // 线索：官方 `CalcSequenceBoundingBoxes` 用 **`AngleMatrix(sanim.rot,
    // sanim.pos)`**（`simplify.cpp:7087`）直接建局部矩阵，**不**走
    // `CalcBoneTransforms` 的 DELTA 合成分支（`simplify.cpp:4568-4575`）。
    // 对 `subtract` 动画，`sanim` 存的是**增量**，所以官方那边等于把
    // 「增量」当成「完整姿态」用 —— 增量近零 ⟹ `bonetransform` 近单位阵
    // ✅ **正解（已实测）**：官方包围盒用**减除之前**的姿态算。
    //
    // 官方 `CalcSequenceBoundingBoxes` 用 `AngleMatrix(sanim.rot, sanim.pos)`
    // （`simplify.cpp:7087`）直接建局部矩阵，**不**走 `CalcBoneTransforms`
    // 的 DELTA 合成分支（`simplify.cpp:4558-4577`）。对 `subtract` 动画
    // `sanim` 是**增量**，于是官方那边 `bonetransform` 近单位阵 ⟹
    // `posetransform = inverse(boneToPose)` ⟹ 顶点被映射到**参考姿态**附近。
    //
    // **决定性判据**（`probe_nosub_bbox.js`）：把 `subtract` 从 miku 的
    // TOML 里去掉再编译，`look_poses` 的包围盒从 **44276588 ULP → 51 ULP**：
    //
    // | 序列 | 带 `subtract` | 去掉 `subtract` |
    // |---|---|---|
    // | `look_poses` | 44276588 ULP | **51 ULP** |
    //
    // 所以这里改用 `pre_subtract_frames`（减除前的原始 SMD 姿态）。
    // 注意**只影响包围盒** —— 写进动画链的仍是 `frames`（减除后）。
    let cell_frames: Vec<&[Vec<crate::smd::SmdPose>]> = if desc.model.static_prop {
        vec![&identity_frames]
    } else if seq.cells.is_empty() {
        vec![seq.pre_subtract_frames.as_ref().unwrap_or(&seq.frames)]
    } else {
        seq.cells
            .iter()
            .filter_map(|c| compiled.animations.get(*c))
            .map(|a| a.pre_subtract_frames.as_ref().unwrap_or(&a.frames).as_slice())
            .collect()
    };

    for frames in cell_frames {
        for frame in frames {
            // 1) 该帧的局部世界矩阵（官方 `CalcBoneTransforms`）。
            let world = frame_worlds(desc, &parents, frame);

            // 2) `posetransform[k] = world[k] ∘ inverse(ref_world[k])`。
            let posetransform: Vec<crate::bone_math::Matrix3x4> = world
                .iter()
                .zip(ref_world.iter())
                .map(|(w, r)| crate::bone_math::concat(w, &crate::bone_math::invert(r)))
                .collect();

            // 3) 并入每根骨骼的渲染包围盒（用 `bonetransform`，即 `world`）。
            for (k, w) in world.iter().enumerate() {
                let (mn, mx) = render_bounds.get(k).copied().unwrap_or(([0.0; 3], [0.0; 3]));
                let (tmn, tmx) = transform_aabb(w, mn, mx);
                for a in 0..3 {
                    if tmn[a] < bmin[a] {
                        bmin[a] = tmn[a];
                    }
                    if tmx[a] > bmax[a] {
                        bmax[a] = tmx[a];
                    }
                }
            }

            // 4) 并入蒙皮后的顶点。
            for bp in &compiled.bodyparts {
                for m in &bp.models {
                    for mesh in &m.meshes {
                        for v in &mesh.vertices {
                            let mut pos = [0.0f32; 3];
                            for pair in &v.bones {
                                let k = pair[0] as usize;
                                if k >= n {
                                    continue;
                                }
                                let t = transform_point(&posetransform[k], v.pos);
                                for a in 0..3 {
                                    pos[a] += pair[1] * t[a];
                                }
                            }
                            for a in 0..3 {
                                if pos[a] < bmin[a] {
                                    bmin[a] = pos[a];
                                }
                                if pos[a] > bmax[a] {
                                    bmax[a] = pos[a];
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if bmin[0].is_finite() && bmax[0].is_finite() {
        Some((bmin, bmax))
    } else {
        None
    }
}

/// 把一个 AABB 用 `matrix3x4` 变换后求新的 AABB（`TransformAABB`）。
///
/// 做法是变换 8 个角再取 min/max —— 对旋转矩阵这是**精确**的
/// （不像只变换两个角那样会低估）。
fn transform_aabb(
    m: &crate::bone_math::Matrix3x4,
    mn: [f32; 3],
    mx: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let mut out_min = [f32::INFINITY; 3];
    let mut out_max = [f32::NEG_INFINITY; 3];
    for i in 0..8 {
        let c = [
            if i & 1 == 0 { mn[0] } else { mx[0] },
            if i & 2 == 0 { mn[1] } else { mx[1] },
            if i & 4 == 0 { mn[2] } else { mx[2] },
        ];
        let t = transform_point(m, c);
        for a in 0..3 {
            if t[a] < out_min[a] {
                out_min[a] = t[a];
            }
            if t[a] > out_max[a] {
                out_max[a] = t[a];
            }
        }
    }
    (out_min, out_max)
}

/// `$staticprop` 的骨骼塌缩（`simplify.cpp:3278-3305`）。
///
/// # 做什么
///
/// 1. 骨骼 0 改名为 [`STATIC_PROP_BONE`]、`parent = -1`；
/// 2. 位置与旋转**归零** —— `simplify.cpp:3362-3366`：
///    ```cpp
///    psource->rawanim[0][0].pos = Vector( 0, 0, 0 );
///    psource->rawanim[0][0].rot = RadianEuler( 0, 0, 0 );
///    AngleMatrix( QAngle( 0, 0, 0 ), psource->boneToPose[0] );
///    ```
///    实测：即便 SMD 的 root 带非零姿态（`ipg.smd` 的 pos `(5,6,7)`、
///    rot `(0.1,0.2,0.3)`），产物 `static_prop` 仍是
///    `pos=[0,0,0] rot=[0,0,0] poseToBone=单位矩阵`。
/// 3. **其余骨骼全部丢弃** —— 实测 `numbones == 1`（2681/2681）。
///
/// # 为什么保留第 0 根骨骼的 `surface_prop` / `bonemerge`
///
/// 只有第 0 根会被写出，其余字段对产物没有影响；但保留它们可以让
/// 「用户显式写的 `flags`」等覆盖继续生效，语义上更贴近
/// 「第 0 根骨骼被改名」而不是「新建一根骨骼」。
fn collapse_static_prop(desc: &ModelDesc) -> ModelDesc {
    let mut out = desc.clone();
    let Some(first) = out.bones.first().cloned() else {
        return out;
    };
    out.bones = vec![Bone {
        name: crate::model::STATIC_PROP_BONE.to_string(),
        parent: None,
        position: Some([0.0; 3]),
        rotation: Some([0.0; 3]),
        flags: first.flags,
        surface_prop: first.surface_prop,
        // `$bonemerge` 对静态道具没有意义（只有一根骨骼，无可合并的层级）。
        bonemerge: false,
        // 塌缩后的姿态是**手工写死**的 `[0,0,0]`，等价于 `$definebone`，
        // 所以显式标成 pre-aligned（`MakeStaticProp`，`simplify.cpp:3362-3366`）。
        // 单根骨骼没有子骨骼，`childbone == -1`，重排本来也不会碰它 ——
        // 这里只是把意图写明，不依赖推断。
        pre_aligned: Some(true),
        realign_position: None,
        realign_rotation: None,
    }];

    // hitbox / attachment 的骨骼引用全部重定向到那唯一一根骨骼。
    //
    // 对 hitbox：studiomdl 在 `$staticprop` + 显式 `$hbox` 时**直接报错**
    // （`simplify.cpp:6991` 的 `cannot find bone %s for bbox`，因为骨骼已改名
    // 而 `$hbox` 里写的还是旧名 —— 实测 `ipf2`/`ipf3`/`ipf5` 三个 QC 全部
    // 编译失败）。而语料里 2681/2681 个静态道具的 hitbox 都带
    // `AUTOGENERATED` 标志、0 个用显式 `$hbox` —— 与「显式路径必失败」
    // 完全一致。
    //
    // 这里**不报错**而是重定向：报错会让 `static_prop = true` 与显式
    // `[[hitboxes]]` 无法共存，而重定向产出的 hitbox 与官方自动生成路径
    // 落在同一根骨骼上，是更有用的行为。要复刻官方「硬错误」语义的话，
    // 应该在 `validate()` 里拒绝这种组合（见 [`ModelDesc::validate`]）。
    for hb in &mut out.hitboxes.boxes {
        hb.bone = crate::model::STATIC_PROP_BONE.to_string();
    }

    // 附着点：`MakeStaticProp()` 会把 `bonename` 改成 `static_prop`
    // （`simplify.cpp:3394`），并把 `local` 矩阵左乘旋转矩阵
    // （`simplify.cpp:3392` 的 `ConcatTransforms( rotated, local, local )`）。
    //
    // 矩阵的合成放在**写出器**里做（见 `mdl_writer` 的 attachment 段），
    // 因为那里才有「角度 → 矩阵」的原始精度；在这里拆成欧拉角再让写出器
    // 重建会多走一趟 `to_degrees()`/`to_radians()`，破坏逐位一致性。
    // 这里只负责把骨骼引用重定向。
    //
    // 实测 `ipf4`（`$staticprop` + `$attachment "muzzle" "root" 7 8 9`）：
    // ```text
    //   bone = 0
    //   local = [[-0, -1, 0, -8],
    //            [ 1, -0, 0,  7],
    //            [ 0,  0, 1,  9]]
    // ```
    // 正是 `Rz(90°) × 单位矩阵(平移 7,8,9)`：旋转部分变成 `Rz(90°)`，
    // 平移部分变成 `(-8, 7, 9)`。
    for at in &mut out.attachments {
        at.bone = crate::model::STATIC_PROP_BONE.to_string();
    }

    out
}

/// `$staticprop` 的几何旋转矩阵（`Rz(90°)`）。
///
/// 与 [`static_prop_rotate`] 是同一个映射，这里给需要矩阵形式的调用方
/// （附着点的 `ConcatTransforms`）用。
pub fn static_prop_matrix() -> crate::bone_math::Matrix3x4 {
    crate::bone_math::angle_matrix([0.0, 0.0, std::f32::consts::FRAC_PI_2])
}

/// `mstudiomodel_t.name`：显式给了就用，否则取 SMD 的文件名
/// （studiomdl 实测行为）。
fn model_name(m: &BodyModel, smd_path: &Path) -> String {
    if let Some(n) = &m.name {
        return n.clone();
    }
    smd_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 解析骨骼的最终参考姿态：描述显式值优先，否则取 SMD。
///
/// 返回值是 **(位置, 旋转弧度)**。描述里的 `rotation` 是**角度**，
/// 这里转成弧度以统一口径。
///
/// # 旋转会被**规范化**
///
/// 欧拉角不唯一，同一个旋转有多组等价三元组。实测官方 studiomdl 写进
/// `mstudiobone_t.rot` 的是它自己那套 `MatrixAngles` 分解出来的值 ——
/// `gimbal` 实验（pitch = π/2 万向锁）里：
///
/// | 来源 | 值 |
/// |---|---|
/// | SMD 第 0 帧 | `[0.35, 1.570796, -0.25]` |
/// | 规范化后 | `[0, 1.570796, -0.6]` |
/// | **官方骨骼表** | **`[0, 1.570796, -0.6]`** ← 规范化 |
///
/// 不规范化会让 `mstudiobone_t.rot` 与动画的参考姿态基准不一致，
/// 解码后整个动画偏移一个常量（**不报错**，只是动作错位）。
/// 旋转本身不变，所以 `quat` / `poseToBone` 不受影响。
pub fn resolve_bone_pose(
    desc: &ModelDesc,
    compiled: &CompiledModelDesc,
    bone_index: usize,
) -> ([f32; 3], [f32; 3]) {
    // **骨骼轴重对齐优先** —— 一旦触发，骨骼表的 pos/rot 就来自
    // `RealignBones` 重建的结果，不再是 SMD 的原始姿态。
    //
    // 这一步必须在这里做（而不是在写出器里）：`poseToBone`、自动 hitbox、
    // 动画的参考姿态**全部**经由本函数取姿态，任何一处漏掉都会让它们
    // 与骨骼表互相矛盾。
    if let Some(r) = &compiled.realigned
        && let Some(v) = r.poses.get(bone_index)
    {
        return *v;
    }
    let b = &desc.bones[bone_index];
    // 从任意一个 model 的参考姿态里找（同一次编译里它们应当一致）。
    let from_smd = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .find_map(|m| m.poses.iter().find(|p| p.bone == bone_index as i32).copied());

    let pos = b
        .position
        .or_else(|| from_smd.map(|p| p.position))
        .unwrap_or([0.0; 3]);
    let rot = match b.rotation {
        // 描述里写的是角度 → 转弧度。
        Some(deg) => [
            deg[0].to_radians(),
            deg[1].to_radians(),
            deg[2].to_radians(),
        ],
        // SMD 里本来就是弧度，原样搬运。
        None => from_smd.map(|p| p.rotation).unwrap_or([0.0; 3]),
    };
    (pos, crate::bone_math::canonical_euler(rot))
}

/// 取自 **SMD / skeleton** 的参考姿态 —— **忽略** TOML 的
/// `position`/`rotation`。
///
/// # 为什么单要一份「源」姿态
///
/// 官方 `SetupHitBoxes`（`simplify.cpp:6925`）用的世界矩阵是
/// **`srcWorld ∘ srcRealign`** —— 也就是 `simplify.cpp:1527` 那条**动画**
/// 路径的产物，**不是**骨骼表最终的参考姿态。两者只在写了 `$definebone`
/// （TOML 的 `position`/`rotation`）时才会分叉。
///
/// 实测（受控实验，对照官方产物）：
///
/// | 用例 | SMD 里 b 的世界 z | 文件里 b 的世界 z | 官方 hitbox(bone 2) 的 z | ⟹ 用的世界 z |
/// |---|---|---|---|---|
/// | `ipq2`（`$definebone` **6** 数字） | 30 | **20** | **`-8 .. 0`** | **30** = SMD |
/// | `ipq3`（**12** 数字，`srcRealign` 平移 `[0,0,10]`） | 30 | **20** | **`-18 .. 0`** | **40** = 30+10 |
/// | `ipr2`（`$ikchain` 触发重排，无 `$definebone`） | 30 | 重排后 x=20 | 落在**重排后**的基里 | = `srcWorld ∘ srcRealign` |
///
/// 顶点 z 是 22，于是 `22 − 30 = −8`、`22 − 40 = −18` —— 三个用例同一公式。
///
/// > 所以官方产物**内部并不自洽**：`poseToBone` 说 b 在世界 z=20，
/// > 而 hitbox 是按 30 算的。照搬这个实际行为即可 —— 修好它之后
/// > `ipq2`/`ipq3` 的 `hull_max` 与 `illumposition` 会跟着归位
/// > （姿态包围盒把每根骨骼的渲染 bbox 并了进去）。
///
/// 骨骼**没有** SMD 姿态时（纯 TOML 骨骼）回退到 [`resolve_bone_pose`]。
pub fn source_bone_pose(compiled: &CompiledModelDesc, bone_index: usize) -> ([f32; 3], [f32; 3]) {
    let from_smd = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .find_map(|m| m.poses.iter().find(|p| p.bone == bone_index as i32).copied());
    match from_smd {
        // SMD 里本来就是弧度，**不**做 `canonical_euler` —— 动画帧
        // （`realign_sequence_frames`）也是原样用的，两边必须一致。
        Some(p) => (p.position, p.rotation),
        None => resolve_bone_pose(&compiled.desc, compiled, bone_index),
    }
}

/// 官方**内部**的 `g_bonetable[k].boneToPose` —— `SetupHitBoxes`
/// （`simplify.cpp:6925`）与 `CalcSequenceBoundingBoxes` 用的世界矩阵。
///
/// **不是**骨骼表最终写进文件的参考姿态。两者的差别与证据见
/// [`source_bone_pose`]。
///
/// # 取值规则（逐骨骼）
///
/// | 骨骼 | 内部世界矩阵 |
/// |---|---|
/// | 写了 `$definebone`（TOML 有 `position`/`rotation`）或有显式 `srcRealign` | **`srcWorld ∘ srcRealign`** |
/// | 其余 | **骨骼表的重排后世界矩阵**（= `resolve_bone_pose` 那套） |
///
/// # 为什么不能一律用 `srcWorld ∘ srcRealign`
///
/// 对**非** pre-aligned 骨骼，`srcRealign = srcWorld⁻¹ ∘ newWorld`，
/// 于是 `srcWorld ∘ srcRealign` 在代数上就是 `newWorld` —— 但**浮点上不是**：
/// 多做一次「求逆再相乘」会引入约 1 ulp 的误差。
///
/// 实测（回归对照 `ipr2`/`ipr3`，全部骨骼都走了重排）：
///
/// | | `hull_min[2]` | `hull_max[0]` |
/// |---|---|---|
/// | 一律往返 | `-3.4969110629390343e-7` | `8` |
/// | 直接用 `newWorld` | **`-3.4969110629390343e-7`** ✓ | **`7.999999523162842`** ✓ = 官方 |
///
/// 所以只在**真正需要**（pre-aligned / 显式 `srcRealign`）时才走
/// `srcWorld ∘ srcRealign`，其余走骨骼表矩阵，逐位与官方一致。
///
/// # 为什么两处调用必须共用同一份
///
/// `CalcSequenceBoundingBoxes` 里
/// `posetransform[k] = bonetransform[k] ∘ inverse(boneToPose[k])` ——
/// 帧世界矩阵与参考世界矩阵**必须同源**，否则这个「相对变换」会凭空多出
/// 一次平移。实测 `ipq2`：`bonetransform` 用 30、参考用骨骼表的 20 时，
/// 顶点 z=22 被推到 **32**，而官方是 **30**。
fn internal_bone_world(
    compiled: &CompiledModelDesc,
    parents: &[i32],
) -> Vec<crate::bone_math::Matrix3x4> {
    let desc = &compiled.desc;
    let n = desc.bones.len();

    let src: Vec<([f32; 3], [f32; 3])> = (0..n).map(|i| source_bone_pose(compiled, i)).collect();
    let src_world = crate::bone_math::compute_world(
        &src.iter().map(|p| p.0).collect::<Vec<_>>(),
        &src.iter().map(|p| p.1).collect::<Vec<_>>(),
        parents,
    );

    let table: Vec<([f32; 3], [f32; 3])> =
        (0..n).map(|i| resolve_bone_pose(desc, compiled, i)).collect();
    let table_world = crate::bone_math::compute_world(
        &table.iter().map(|p| p.0).collect::<Vec<_>>(),
        &table.iter().map(|p| p.1).collect::<Vec<_>>(),
        parents,
    );

    (0..n)
        .map(|k| {
            let explicit = desc.bones[k].explicit_src_realign();
            if desc.bones[k].is_pre_aligned() || explicit.is_some() {
                let sr = explicit.unwrap_or(crate::bone_math::IDENTITY);
                crate::bone_math::concat(&src_world[k], &sr)
            } else {
                table_world[k]
            }
        })
        .collect()
}

/// 把矩阵的旋转部分作用到一个方向向量上（`VectorIRotate` 语义）。
///
/// 对**正交**矩阵，旋转一个向量就是 `R · v`（不含平移）。
/// 用于把 `(0,0,1)`/`(0,1,0)` 逆旋转成 eyeball 的骨骼空间 `up`/`forward`。
fn rotate_vector(m: &crate::bone_math::Matrix3x4, v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[4] * v[0] + m[5] * v[1] + m[6] * v[2],
        m[8] * v[0] + m[9] * v[1] + m[10] * v[2],
    ]
}

/// 归一化成单位向量（零向量原样返回，由调用方保证输入非退化）。
fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 { v } else { [v[0] / len, v[1] / len, v[2] / len] }
}

/// **VTA 形状解析**：`[[bodyparts.models.flexes]]` → 每个 mesh 的载荷。
///
/// # 做两件事
///
/// 1. **注册 flexdesc**（按名去重，与 `Add_Flexdesc` 一致）——
///    `pair = true` 时注册 **`<名>R` 与 `<名>L` 两个**（先 R 后 L）。
///    这一步会**改变 `numflexdesc` 与段布局**，所以必须在
///    [`resolve_flex_eyeball_mouth`] 建查找表**之前**完成。
/// 2. **算载荷**：读 `.vta` → 就近匹配 → 差量 → smoothstep → `speed`，
///    结果按 mesh 分组存进 [`CompiledModel::mesh_flexes`]。
///
/// # 顶点池口径（关键）
///
/// `mstudioflex_t.vertindex` 指向的 vertanim 数组里，`index` 是
/// **该 mesh 内的局部顶点下标**（`write.cpp:1789` 的
/// `n = vanim[k].vertex - pmesh[m].vertexoffset`）。
/// 所以匹配必须在「该 model 的顶点池」里做，再减去该 mesh 的
/// `vertexoffset` 折算成局部下标 —— 这个 `vertexoffset` 必须与
/// `mdl_writer` 写出的**完全一致**（多 LOD 时它来自 `LodLayout`）。
fn resolve_vta_flexes(
    compiled: &mut CompiledModelDesc,
    base_dir: &Path,
) -> Result<(), Vec<CompileError>> {
    let mut errs: Vec<CompileError> = Vec::new();

    // 有没有活要干？没有就直接返回（保证零开销、产物逐字节不变）。
    let any = compiled
        .desc
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .any(|m| !m.flexes.is_empty());
    if !any {
        return Ok(());
    }

    // ---- 1. 注册全部 flexdesc 名（先 R 后 L）----
    //
    // `Add_Flexdesc` 按名去重（`stricmp`），所以同名只注册一次。
    // 这里先把「每条 flex 规格 → (desc 下标, pair 下标)」算出来，
    // 再在下面算载荷时用。
    //
    // ⚠️ 先把 flex 规格**克隆**出来：注册会改 `compiled.desc.flex_descriptors`，
    // 而同时借用 `compiled.desc.bodyparts` 会被借用检查拒绝。
    let specs: Vec<Vec<Vec<crate::model::Flex>>> = compiled
        .desc
        .bodyparts
        .iter()
        .map(|bp| bp.models.iter().map(|m| m.flexes.clone()).collect())
        .collect();

    let mut desc_index: HashMap<String, usize> = compiled
        .desc
        .flex_descriptors
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    let mut register = |name: &str, list: &mut Vec<crate::model::FlexDescriptor>| -> usize {
        if let Some(&i) = desc_index.get(name) {
            return i;
        }
        list.push(crate::model::FlexDescriptor {
            name: name.to_string(),
        });
        let i = list.len() - 1;
        desc_index.insert(name.to_string(), i);
        i
    };

    // 每条 flex 规格的 (主 desc 下标, 配对 desc 下标)。0 表示无配对。
    let mut desc_of: Vec<Vec<Vec<(usize, usize)>>> = Vec::new();
    for (bi, bp) in specs.iter().enumerate() {
        let mut per_model = Vec::new();
        for (mi, m) in bp.iter().enumerate() {
            let mut row = Vec::with_capacity(m.len());
            for (fi, f) in m.iter().enumerate() {
                let at = format!("bodyparts[{bi}].models[{mi}].flexes[{fi}]");
                if f.name.is_empty() {
                    errs.push(e(&at, "name 不能为空".to_string()));
                    row.push((0, 0));
                    continue;
                }
                if f.pair {
                    let (rn, ln) = crate::flex::pair_names(&f.name);
                    let r = register(&rn, &mut compiled.desc.flex_descriptors);
                    let l = register(&ln, &mut compiled.desc.flex_descriptors);
                    row.push((r, l));
                } else {
                    let d = register(&f.name, &mut compiled.desc.flex_descriptors);
                    row.push((d, 0));
                }
            }
            per_model.push(row);
        }
        desc_of.push(per_model);
    }

    // ---- 2. 算载荷 ----
    //
    // ⚠️ 顶点池必须与 `mdl_writer` 的口径一致。多 LOD 时
    // `mstudiomesh_t.vertexoffset` 来自 `model_vertex_layout`，所以这里
    // 也走同一条路径；单 LOD 时是「按 mesh 顺序累加」。
    let multi = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .any(|m| m.lods.as_ref().is_some_and(|l| l.is_multi()));
    let lod_layout = if multi {
        let mut all = Vec::new();
        for bp in &compiled.bodyparts {
            for m in &bp.models {
                match &m.lods {
                    Some(l) => all.extend(l.meshes.iter().cloned()),
                    None => {
                        for mesh in &m.meshes {
                            all.push(crate::lod::MeshLods::single(
                                mesh.vertices.clone(),
                                mesh.triangles.clone(),
                            ));
                        }
                    }
                }
            }
        }
        Some(crate::lod::build_lod_layout(&all))
    } else {
        None
    };

    // `.vta` 按路径缓存（同一文件常被多条 flex 共用）。
    let mut vta_cache: HashMap<std::path::PathBuf, crate::vta::Vta> = HashMap::new();

    let mut model_global = 0usize;
    for (bi, bp) in specs.iter().enumerate() {
        for (mi, m) in bp.iter().enumerate() {
            let cur = model_global;
            model_global += 1;
            if m.is_empty() {
                continue;
            }
            let at = format!("bodyparts[{bi}].models[{mi}]");

            // 该 model 的顶点池 + 「池下标 → (mesh, mesh 内局部下标)」。
            let comp = &compiled.bodyparts[bi].models[mi];
            let mut pool: Vec<Vertex> = Vec::new();
            let mut mesh_of_vertex: Vec<(usize, u32)> = Vec::new();
            for (ki, mesh) in comp.meshes.iter().enumerate() {
                for (vi, v) in mesh.vertices.iter().enumerate() {
                    pool.push(v.clone());
                    mesh_of_vertex.push((ki, vi as u32));
                }
            }
            // 多 LOD：`vertexoffset` 是「该 mesh 在 model 内的累计起点」，
            // 与单 LOD 的累加语义相同，但**总数**是跨 LOD 去重后的。
            // flex 只关心 LOD 0 的顶点，所以用 `model_vertex_layout`
            // 给出的 mesh 起点来定位。
            let mesh_offsets: Vec<usize> = match &lod_layout {
                Some(l) => {
                    let counts: Vec<usize> = compiled
                        .bodyparts
                        .iter()
                        .flat_map(|bp| &bp.models)
                        .map(|m| m.meshes.len())
                        .collect();
                    let mv = crate::lod::model_vertex_layout(l, &counts);
                    mv[cur].meshes.iter().map(|(_, off)| *off).collect()
                }
                None => {
                    let mut offs = Vec::with_capacity(comp.meshes.len());
                    let mut c = 0usize;
                    for mesh in &comp.meshes {
                        offs.push(c);
                        c += mesh.vertices.len();
                    }
                    offs
                }
            };
            let _ = &mesh_offsets;

            let mut per_mesh: Vec<Vec<crate::flex::ResolvedFlex>> =
                vec![Vec::new(); comp.meshes.len()];

            // `vanim_map` 只取决于 `(vta 第 0 帧, model 顶点池)`，与 flex 无关，
            // 所以同一 `(model, vta)` 下复用一份。真实模型上
            // `build_vanim_map` 是十万×十万量级 —— 逐条 flex 重算会让
            // 编译时间从秒级涨到分钟级（`main.vta` 42 条 flex 共用一份）。
            let mut vanim_map_cache: HashMap<
                std::path::PathBuf,
                crate::flex::VanimMap,
            > = HashMap::new();

            for (fi, f) in m.iter().enumerate() {
                let fat = format!("{at}.flexes[{fi}]");
                let vta_path = resolve_smd_path(base_dir, &f.vta);
                let vta = match vta_cache.entry(vta_path.clone()) {
                    std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        let text = match std::fs::read_to_string(&vta_path) {
                            Ok(t) => t,
                            Err(err) => {
                                errs.push(e(
                                    &fat,
                                    format!("读不到 {}：{err}", vta_path.display()),
                                ));
                                continue;
                            }
                        };
                        match crate::vta::parse_vta(&text) {
                            Ok(v) => {
                                for w in &v.warnings {
                                    errs.push(e(&fat, format!("{}：{w}", vta_path.display())));
                                }
                                slot.insert(v)
                            }
                            Err(err) => {
                                errs.push(e(
                                    &fat,
                                    format!("{} 解析失败：{err}", vta_path.display()),
                                ));
                                continue;
                            }
                        }
                    }
                };

                let fd = desc_of[bi][mi][fi];
                let vanim_map = vanim_map_cache
                    .entry(vta_path.clone())
                    .or_insert_with(|| crate::flex::build_vanim_map(vta, &pool));
                match crate::flex::resolve_flex_mapped(
                    f,
                    vta,
                    vanim_map,
                    &mesh_of_vertex,
                    (fd.0 as i32, fd.1 as i32),
                    fi,
                ) {
                    Ok(meshes) => {
                        for (k, mf) in meshes.into_iter().enumerate() {
                            if k < per_mesh.len() {
                                per_mesh[k].extend(mf.flexes);
                            }
                        }
                    }
                    Err(err) => errs.push(e(&fat, err.message)),
                }
            }

            // 一致性检查：`index` 是 mesh 内局部下标，必须落在该 mesh 的
            // 顶点数内。错位时静默产出「引擎读到错误顶点」的产物，
            // 所以这里显式拦一道。
            for (k, fx) in per_mesh.iter().enumerate() {
                let n = comp.meshes[k].vertices.len();
                for f in fx {
                    for a in &f.vertanims {
                        if a.index as usize >= n {
                            errs.push(e(
                                &at,
                                format!(
                                    "内部错误：mesh {k} 的 vertanim 下标 {} 超出顶点数 {n}\
                                     （顶点池口径与写出器不一致）",
                                    a.index
                                ),
                            ));
                        }
                    }
                }
            }

            compiled.bodyparts[bi].models[mi].mesh_flexes = per_mesh;
        }
    }

    if errs.is_empty() { Ok(()) } else { Err(errs) }
}

/// flex 系列 / eyeball / mouth 的名字解析与骨骼空间量计算。
///
/// # 做什么
///
/// 1. **flexrule**：把 `flex`（flexdesc 名）与每个 op 的 `controller`/`flexdesc`
///    名解析成下标（`fetch1` → flexcontroller、`fetch2` → flexdesc）。
/// 2. **flexcontrollerui**：每条 `[[flex_controllers]]` **自动**产生一条 ui
///    （`szindex0` 指向该 fc 自身，单声道）；用户显式写的追加在后
///    （stereo 对里 `szindex0` 指向**下标更大**那条）。
/// 3. **mouth**：按显式 `index` 摆进数组（长度 = `max(index)+1`），
///    `bone`/`flexdesc` 解析成下标，空洞写全 0。
/// 4. **eyeball**：`org` 经 [`internal_bone_world`] 的逆变换成骨骼空间，
///    `up`/`forward` 由逆旋转 `(0,0,1)`/`(0,1,0)` 得到；lid 名解析成下标；
///    并把对应 mesh 打标 `eyeball_tag`。
fn resolve_flex_eyeball_mouth(compiled: &mut CompiledModelDesc) -> Result<(), Vec<CompileError>> {
    let mut errs: Vec<CompileError> = Vec::new();

    // ---- 0. `dummy_eyelid`：未配 `eyelid` 的 eyeball 会得到一个占位 flexdesc ----
    //
    // ⚠️ 这段在 episode1 源码里**不存在**（`grep dummy_eyelid` 零命中）——
    // 是 L4D2 的行为，只用受控实验钉死：
    //
    // | QC | flexdesc 产物 |
    // |---|---|
    // | `fxr1`（只写 2 条 `eyeball`，**无**任何 flexdesc/eyelid） | `["dummy_eyelid"]` |
    // | `fxl1`（`localvar alpha beta` + eyeball） | `[alpha, beta, dummy_eyelid]` |
    // | `fx2`（localvar/mouth + eyeball） | `[…, dummy_eyelid]` |
    // | 4 个 survivor（**写了** `eyelid`） | **不含** dummy_eyelid |
    //
    // 即：**只要某个 eyeball 缺 lid 数据就追加一条**，且**永远追加在末尾**
    // （`fxl1` 的 alpha/beta 在它之前）。
    //
    // 必须在这里（构造查找表与写字节**之前**）做 —— 它改变
    // `numflexdesc`、flexdesc 数组下标、字符串池内容与段布局。
    let dummy_index: Option<usize> = {
        let needs = compiled
            .desc
            .bodyparts
            .iter()
            .flat_map(|bp| &bp.models)
            .flat_map(|m| &m.eyeballs)
            .any(|eb| eb.upper_lid.is_none() || eb.lower_lid.is_none());
        if !needs {
            None
        } else {
            // 用户显式写了同名就复用，否则追加到末尾（`Add_Flexdesc` 按名去重）。
            match compiled
                .desc
                .flex_descriptors
                .iter()
                .position(|f| f.name == DUMMY_EYELID)
            {
                Some(i) => Some(i),
                None => {
                    compiled.desc.flex_descriptors.push(crate::model::FlexDescriptor {
                        name: DUMMY_EYELID.to_string(),
                    });
                    Some(compiled.desc.flex_descriptors.len() - 1)
                }
            }
        }
    };

    let desc = &compiled.desc;

    // 名字 → 下标的查找表。
    let bone_index = desc.bone_index();
    let flexdesc_index: HashMap<&str, usize> = desc
        .flex_descriptors
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), i))
        .collect();
    let fc_index: HashMap<&str, usize> = desc
        .flex_controllers
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), i))
        .collect();

    // ---- 1. flexrule ----
    let mut rules = Vec::with_capacity(desc.flex_rules.len());
    for (ri, r) in desc.flex_rules.iter().enumerate() {
        let at = format!("flex_rules[{ri}]");
        let Some(&flex) = flexdesc_index.get(r.flex.as_str()) else {
            errs.push(e(&at, format!("flex {:?} 在 [[flex_descriptors]] 里找不到", r.flex)));
            continue;
        };
        let mut ops = Vec::with_capacity(r.ops.len());
        for (oi, op) in r.ops.iter().enumerate() {
            let oat = format!("{at}.ops[{oi}]");
            let kind = op.op;
            let d = match kind.operand() {
                crate::model::FlexOperand::Value => {
                    let Some(v) = op.value else {
                        errs.push(e(&oat, format!("op {:?} 需要 value", kind.code())));
                        continue;
                    };
                    crate::model::FlexOpData::Value(v)
                }
                crate::model::FlexOperand::Index => match kind {
                    crate::model::FlexOpKind::Fetch1 => {
                        let Some(name) = op.controller.as_deref() else {
                            errs.push(e(&oat, "fetch1 需要 controller".to_string()));
                            continue;
                        };
                        let Some(&idx) = fc_index.get(name) else {
                            errs.push(e(
                                &oat,
                                format!("fetch1 的 controller {name:?} 在 [[flex_controllers]] 里找不到"),
                            ));
                            continue;
                        };
                        crate::model::FlexOpData::Index(idx as i32)
                    }
                    crate::model::FlexOpKind::Fetch2 => {
                        let Some(name) = op.flexdesc.as_deref() else {
                            errs.push(e(&oat, "fetch2 需要 flexdesc".to_string()));
                            continue;
                        };
                        let Some(&idx) = flexdesc_index.get(name) else {
                            errs.push(e(
                                &oat,
                                format!("fetch2 的 flexdesc {name:?} 在 [[flex_descriptors]] 里找不到"),
                            ));
                            continue;
                        };
                        crate::model::FlexOpData::Index(idx as i32)
                    }
                    _ => unreachable!("operand()==Index 只有 fetch1/fetch2"),
                },
                crate::model::FlexOperand::None => crate::model::FlexOpData::None,
            };
            ops.push(crate::model::ResolvedFlexOp { op: kind.code(), d });
        }
        rules.push(crate::model::ResolvedFlexRule { flex: flex as i32, ops });
    }
    compiled.resolved_flex_rules = rules;

    // ---- 2. flexcontrollerui（自动 + 显式）----
    let mut uis: Vec<crate::model::ResolvedFlexControllerUi> = Vec::new();
    // 每条 flexcontroller 自动产一条（官方 `flexcontroller` 命令顺带生成，
    // 实测 fx2：3 个 fc → 3 条 ui，szindex0 指向 fc 自身，stereo=0）。
    for (i, fc) in desc.flex_controllers.iter().enumerate() {
        uis.push(crate::model::ResolvedFlexControllerUi {
            name: fc.name.clone(),
            fc0: i as i32,
            fc1: None,
            stereo: false,
        });
    }
    // 用户显式写的（DMX 形态 / stereo 对）。
    for (ui_i, u) in desc.flex_controller_ui.iter().enumerate() {
        let at = format!("flex_controller_ui[{ui_i}]");
        // 解析 left/right 到 fc 下标；缺省回退：单条时指向同一个。
        let li = u.left.as_deref().and_then(|n| fc_index.get(n).copied());
        let ri = u.right.as_deref().and_then(|n| fc_index.get(n).copied());
        // 校验引用存在。
        for (which, name) in [("left", &u.left), ("right", &u.right)] {
            if let Some(n) = name
                && !fc_index.contains_key(n.as_str())
            {
                errs.push(e(
                    &at,
                    format!("{which} 的 flexcontroller {n:?} 在 [[flex_controllers]] 里找不到"),
                ));
            }
        }
        let (fc0, fc1) = match (li, ri) {
            (Some(a), Some(b)) => {
                // `szindex0` 指向**下标更大**那条（语料 108/108）。
                if a >= b { (a as i32, Some(b as i32)) } else { (b as i32, Some(a as i32)) }
            }
            (Some(a), None) => (a as i32, None),
            (None, Some(b)) => (b as i32, None),
            (None, None) => {
                errs.push(e(&at, "stereo ui 需要 left/right 至少一个".to_string()));
                continue;
            }
        };
        uis.push(crate::model::ResolvedFlexControllerUi {
            name: u.name.clone(),
            fc0,
            fc1,
            stereo: u.stereo,
        });
    }
    compiled.resolved_flex_controller_ui = uis;

    // ---- 3. mouth（按显式 index 摆放）----
    if !desc.mouths.is_empty() {
        let max_index = desc.mouths.iter().map(|m| m.index).max().unwrap_or(0);
        if max_index < 0 {
            errs.push(e("mouths", "index 不能为负".to_string()));
        } else {
            let n = (max_index + 1) as usize;
            // 空洞写全 0（与官方 g_mouth[index] 未初始化一致）。
            let mut slots = vec![
                crate::model::ResolvedMouth {
                    bone: 0,
                    forward: [0.0; 3],
                    flexdesc: 0,
                };
                n
            ];
            let mut filled = vec![false; n];
            for (mi, m) in desc.mouths.iter().enumerate() {
                let at = format!("mouths[{mi}]");
                if m.index < 0 {
                    errs.push(e(&at, format!("index {} 不能为负", m.index)));
                    continue;
                }
                let slot = m.index as usize;
                if filled[slot] {
                    errs.push(e(&at, format!("index {} 被写了两次", m.index)));
                    continue;
                }
                let Some(&bone) = bone_index.get(m.bone.as_str()) else {
                    errs.push(e(&at, format!("骨骼 {:?} 找不到", m.bone)));
                    continue;
                };
                let Some(&fd) = flexdesc_index.get(m.flexdesc.as_str()) else {
                    errs.push(e(
                        &at,
                        format!("flexdesc {:?} 在 [[flex_descriptors]] 里找不到", m.flexdesc),
                    ));
                    continue;
                };
                slots[slot] = crate::model::ResolvedMouth {
                    bone: bone as i32,
                    forward: m.forward,
                    flexdesc: fd as i32,
                };
                filled[slot] = true;
            }
            compiled.resolved_mouths = slots;
        }
    }

    // ---- 4. eyeball（骨骼空间变换 + mesh 打标）----
    // 世界矩阵口径与自动 hitbox / 姿态包围盒一致（`internal_bone_world`）。
    let any_eyeball = desc
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .any(|m| !m.eyeballs.is_empty());
    if any_eyeball {
        let parents: Vec<i32> = bone_parents(desc);
        let world = internal_bone_world(compiled, &parents);
        // 逐 model 处理；把描述里的 material 名映射到该 model 的 mesh 下标。
        for (bi, bp) in compiled.bodyparts.iter_mut().enumerate() {
            for (mi, cm) in bp.models.iter_mut().enumerate() {
                let dmodel = &desc.bodyparts[bi].models[mi];
                if dmodel.eyeballs.is_empty() {
                    continue;
                }
                let at = format!("bodyparts[{bi}].models[{mi}].eyeballs");
                // 该 model 的「材质名 → mesh 下标」。
                let mat_to_mesh: HashMap<usize, usize> = cm
                    .meshes
                    .iter()
                    .enumerate()
                    .map(|(k, mesh)| (mesh.material, k))
                    .collect();
                let mut out_eb = Vec::with_capacity(dmodel.eyeballs.len());
                for (ej, eb) in dmodel.eyeballs.iter().enumerate() {
                    let eat = format!("{at}[{ej}]");
                    let Some(&bone) = bone_index.get(eb.bone.as_str()) else {
                        errs.push(e(&eat, format!("骨骼 {:?} 找不到", eb.bone)));
                        continue;
                    };
                    // org：世界空间 → 骨骼空间（`VectorITransform` 对内部
                    // `boneToPose` 是正向变换；等价于 `org × poseToBone`，
                    // 即「世界矩阵取逆后变换」）。
                    let inv = crate::bone_math::invert(&world[bone]);
                    let org = transform_point(&inv, eb.org);
                    // up / forward：`VectorIRotate`（逆旋转）—— 对内部矩阵
                    // 是正向旋转 `(0,0,1)`/`(0,-1,0)`；等价于用**逆矩阵**旋转。
                    //
                    // ⚠️ **`forward` 是 `(0,-1,0)` 而不是 `(0,1,0)`。**
                    // episode1 源码 `studiomdl.cpp:3451` 写的是 `(0,1,0)`，
                    // 还带一行注释 `// FIXME: this is backwards` —— L4D2
                    // **把这个 FIXME 修掉了**（又一次「episode1 源码不是
                    // L4D2 的 studiomdl」）。
                    //
                    // 证据（真实语料，`opt_eyeball_forward` 判据）：
                    // 4 个 survivor 模型共 8 条 eyeball，
                    // `forward == VectorIRotate((0,-1,0), poseToBone)`
                    // **8/8 命中**，而 `(0,+1,0)` **0/8**。
                    // `up == VectorIRotate((0,0,1), …)` 同样 **8/8**。
                    //
                    // `rotate_vector` 走的正是 `poseToBone`（= `inv(world)`），
                    // 所以这里直接给世界空间的 `(0,-1,0)`。
                    let up = normalize3(rotate_vector(&inv, [0.0, 0.0, 1.0]));
                    let forward = normalize3(rotate_vector(&inv, [0.0, -1.0, 0.0]));
                    // lid 三元组 → 下标。
                    //
                    // 缺省（未写 `eyelid` / 未给 lid）：三个槽与 `*lidflexdesc`
                    // 一律指向 `dummy_eyelid`，target 写死 `[-1, 0, 1]`
                    // （受控实验 `fxr1`/`fx2`/`fxl1` 一致）。
                    let d = dummy_index.unwrap_or(0) as i32;
                    let (mut ufd, mut utgt, mut ulid) = ([d; 3], DUMMY_LID_TARGETS, d);
                    let (mut lfd, mut ltgt, mut llid) = ([d; 3], DUMMY_LID_TARGETS, d);
                    if let Some(lid) = &eb.upper_lid
                        && let Err(msg) = resolve_lid(lid, &flexdesc_index, &mut ufd, &mut utgt, &mut ulid)
                    {
                        errs.push(e(format!("{eat}.upper_lid"), msg));
                    }
                    if let Some(lid) = &eb.lower_lid
                        && let Err(msg) = resolve_lid(lid, &flexdesc_index, &mut lfd, &mut ltgt, &mut llid)
                    {
                        errs.push(e(format!("{eat}.lower_lid"), msg));
                    }
                    // mesh 打标：按材质名定位 mesh。
                    //
                    // ⚠️ 与 `build_meshes` 同样用**匹配键**（统一分隔符 +
                    // basename 兜底）—— QC 的 `eyeball` 用裸名（`pupil_r`），
                    // 而 TOML 的材质名可能带路径前缀，两者都要能匹配。
                    let want = crate::mdl_writer::texture_match_key(
                        &eb.material,
                        &desc.materials.search_paths,
                    );
                    let mat_idx = desc.materials.textures.iter().position(|t| {
                        crate::mdl_writer::texture_match_key(
                            &t.name,
                            &desc.materials.search_paths,
                        ) == want
                    });
                    match mat_idx.and_then(|mi2| mat_to_mesh.get(&mi2).copied()) {
                        Some(mesh_k) => cm.meshes[mesh_k].eyeball_tag = Some(ej),
                        None => errs.push(e(
                            &eat,
                            format!("材质 {:?} 在该 model 的 mesh 里找不到", eb.material),
                        )),
                    }
                    out_eb.push(crate::model::CompiledEyeball {
                        bone: bone as i32,
                        org,
                        zoffset: eb.zoffset,
                        radius: eb.radius,
                        up,
                        forward,
                        iris_scale: eb.iris_scale,
                        upperflexdesc: ufd,
                        lowerflexdesc: lfd,
                        uppertarget: utgt,
                        lowertarget: ltgt,
                        upperlidflexdesc: ulid,
                        lowerlidflexdesc: llid,
                    });
                }
                cm.eyeballs = out_eb;
            }
        }
    }

    if errs.is_empty() { Ok(()) } else { Err(errs) }
}

/// 把 `$jigglebone` 解析成「可直接写字节」的形态：算 `flags`、角度转弧度、
/// 填缺省、骨骼名查下标。
///
/// # `flags`（受控实验 `jig{1,2,4,6,7,8}` 钉死）
///
/// | 位 | 何时置位 |
/// |---|---|
/// | `0x01 IS_FLEXIBLE` | `is_flexible` 块出现 |
/// | `0x02 IS_RIGID` | `is_rigid` 块出现 |
/// | `0x04 YAW_CONSTRAINT` | `is_flexible` 内出现 `yaw_constraint` |
/// | `0x08 PITCH_CONSTRAINT` | `is_flexible` 内出现 `pitch_constraint` |
/// | `0x10 ANGLE_CONSTRAINT` | `angle_constraint` 出现（两种块都算） |
/// | **`0x20 LENGTH_CONSTRAINT`** | **`is_flexible` 或 `is_rigid` 出现就置位** |
/// | `0x40 BASE_SPRING` | `has_base_spring` 块出现 |
///
/// ⚠️ `0x20` 是**无条件**的（只要块在）—— 受控实验 `jig4`（只有
/// `yaw_stiffness`）得 `0x01` 而非 `0x21`，但 `jig6` 的 `is_flexible`/`is_rigid`
/// 分别得 `0x21`/`0x22`。区别在**块**在不在，不在里头的字段。
///
/// # 角度单位
///
/// `angle_constraint`/`yaw_constraint`/`pitch_constraint` 的输入是**度**，
/// 写盘转**弧度**（实测 `angle_constraint 60` → `1.0471976` = π/3）。
fn resolve_jiggle_bones(compiled: &mut CompiledModelDesc) -> Result<(), Vec<CompileError>> {
    use crate::model::jiggle_defaults as def;
    use crate::model::jiggle_flags as fl;

    let mut errs: Vec<CompileError> = Vec::new();
    let desc = &compiled.desc;
    let deg2rad = |d: f32| d * std::f32::consts::PI / 180.0;

    let mut out = Vec::with_capacity(desc.jiggle_bones.len());
    for (i, j) in desc.jiggle_bones.iter().enumerate() {
        let at = format!("jiggle_bones[{i}]");
        let Some(&bone) = desc.bone_index().get(j.bone.as_str()) else {
            errs.push(e(&at, format!("骨骼 {:?} 找不到", j.bone)));
            continue;
        };

        let mut flags = 0i32;
        // 缺省：length 10、六个 stiffness 100、其余 0、base 三轴 ±100。
        let mut length = def::LENGTH;
        let mut tip_mass = 0.0;
        let mut yaw_stiffness = def::STIFFNESS;
        let mut yaw_damping = 0.0;
        let mut pitch_stiffness = def::STIFFNESS;
        let mut pitch_damping = 0.0;
        let mut along_stiffness = def::STIFFNESS;
        let mut along_damping = 0.0;
        let mut angle_limit = 0.0;
        let (mut min_yaw, mut max_yaw, mut yaw_friction, mut yaw_bounce) = (0.0, 0.0, 0.0, 0.0);
        let (mut min_pitch, mut max_pitch, mut pitch_friction, mut pitch_bounce) =
            (0.0, 0.0, 0.0, 0.0);
        let mut base_mass = 0.0;
        let mut base_stiffness = def::STIFFNESS;
        let mut base_damping = 0.0;
        let (mut base_min_left, mut base_max_left, mut base_left_friction) =
            (def::BASE_MIN, def::BASE_MAX, 0.0);
        let (mut base_min_up, mut base_max_up, mut base_up_friction) =
            (def::BASE_MIN, def::BASE_MAX, 0.0);
        let (mut base_min_fwd, mut base_max_fwd, mut base_fwd_friction) =
            (def::BASE_MIN, def::BASE_MAX, 0.0);

        if let Some(fx) = &j.is_flexible {
            flags |= fl::IS_FLEXIBLE | fl::HAS_LENGTH_CONSTRAINT;
            // `allow_length_flex` 清除 `0x20`（见 `JiggleFlexible` 的说明）——
            // 实测 `jig4`（只写那个键 + `yaw_stiffness`）得 `0x01`，
            // 而 `jig6`（不写它）得 `0x21`。
            if fx.allow_length_flex {
                flags &= !fl::HAS_LENGTH_CONSTRAINT;
            }
            if let Some(v) = fx.length {
                length = v;
            }
            if let Some(v) = fx.tip_mass {
                tip_mass = v;
            }
            if let Some(v) = fx.yaw_stiffness {
                yaw_stiffness = v;
            }
            if let Some(v) = fx.yaw_damping {
                yaw_damping = v;
            }
            if let Some(v) = fx.pitch_stiffness {
                pitch_stiffness = v;
            }
            if let Some(v) = fx.pitch_damping {
                pitch_damping = v;
            }
            if let Some(v) = fx.along_stiffness {
                along_stiffness = v;
            }
            if let Some(v) = fx.along_damping {
                along_damping = v;
            }
            if let Some(v) = fx.angle_constraint {
                angle_limit = deg2rad(v);
                flags |= fl::HAS_ANGLE_CONSTRAINT;
            }
            if let Some([a, b]) = fx.yaw_constraint {
                min_yaw = deg2rad(a);
                max_yaw = deg2rad(b);
                flags |= fl::HAS_YAW_CONSTRAINT;
            }
            if let Some(v) = fx.yaw_friction {
                yaw_friction = v;
            }
            if let Some(v) = fx.yaw_bounce {
                yaw_bounce = v;
            }
            if let Some([a, b]) = fx.pitch_constraint {
                min_pitch = deg2rad(a);
                max_pitch = deg2rad(b);
                flags |= fl::HAS_PITCH_CONSTRAINT;
            }
            if let Some(v) = fx.pitch_friction {
                pitch_friction = v;
            }
            if let Some(v) = fx.pitch_bounce {
                pitch_bounce = v;
            }
        }
        if let Some(rg) = &j.is_rigid {
            flags |= fl::IS_RIGID | fl::HAS_LENGTH_CONSTRAINT;
            if let Some(v) = rg.length {
                length = v;
            }
            if let Some(v) = rg.tip_mass {
                tip_mass = v;
            }
            if let Some(v) = rg.angle_constraint {
                angle_limit = deg2rad(v);
                flags |= fl::HAS_ANGLE_CONSTRAINT;
            }
        }
        if let Some(b) = &j.has_base_spring {
            flags |= fl::HAS_BASE_SPRING;
            if let Some(v) = b.base_mass {
                base_mass = v;
            }
            if let Some(v) = b.base_stiffness {
                base_stiffness = v;
            }
            if let Some(v) = b.base_damping {
                base_damping = v;
            }
            if let Some([a, c]) = b.base_left {
                base_min_left = a;
                base_max_left = c;
            }
            if let Some(v) = b.base_left_friction {
                base_left_friction = v;
            }
            if let Some([a, c]) = b.base_up {
                base_min_up = a;
                base_max_up = c;
            }
            if let Some(v) = b.base_up_friction {
                base_up_friction = v;
            }
            if let Some([a, c]) = b.base_forward {
                base_min_fwd = a;
                base_max_fwd = c;
            }
            if let Some(v) = b.base_forward_friction {
                base_fwd_friction = v;
            }
        }

        out.push(crate::model::ResolvedJiggleBone {
            bone: bone as i32,
            flags,
            length,
            tip_mass,
            yaw_stiffness,
            yaw_damping,
            pitch_stiffness,
            pitch_damping,
            along_stiffness,
            along_damping,
            angle_limit,
            min_yaw,
            max_yaw,
            yaw_friction,
            yaw_bounce,
            min_pitch,
            max_pitch,
            pitch_friction,
            pitch_bounce,
            base_mass,
            base_stiffness,
            base_damping,
            base_min_left,
            base_max_left,
            base_left_friction,
            base_min_up,
            base_max_up,
            base_up_friction,
            base_min_forward: base_min_fwd,
            base_max_forward: base_max_fwd,
            base_forward_friction: base_fwd_friction,
        });
    }
    compiled.resolved_jiggle_bones = out;

    if errs.is_empty() { Ok(()) } else { Err(errs) }
}

/// 解析 `[[quat_interp_bones]]`：名字 → 下标、角度（度）→ 四元数、
/// `inv_tolerance = 1/tolerance`。
///
/// 复刻 `Grab_QuatInterpBones`（`studiomdl.cpp:6371-6400`）与写出
/// （`write.cpp:257-263`）：
///
/// ```text
/// tolerance  = DEG2RAD(<trigger> 的第 1 个数字)
/// trigger[k] = AngleQuaternion(DEG2RAD(<trigger> 的 tx ty tz))
/// quat[k]    = AngleQuaternion(DEG2RAD(<trigger> 的 ax ay az))
/// pos[k]     = <basepos> + (px py pz)          // VectorAdd
/// 落盘 inv_tolerance = 1.0 / tolerance         // write.cpp:259
/// ```
///
/// ⚠️ **`inv_tolerance` 是倒数**，不是 `tolerance` 本身 —— 实测语料
/// `survivor_producer` 的第一个触发器 `inv_tolerance = 0.6366197466850281`
/// = `1 / 1.5707963`（即 `tol = 90°`）。
fn resolve_quat_interp_bones(
    compiled: &mut CompiledModelDesc,
) -> Result<(), Vec<CompileError>> {
    let bone_index = compiled.desc.bone_index();
    let mut out: Vec<crate::model::ResolvedQuatInterpBone> = Vec::new();
    let mut errs: Vec<CompileError> = Vec::new();

    for (qi, q) in compiled.desc.quat_interp_bones.iter().enumerate() {
        let at = format!("quat_interp_bones[{qi}]");
        let Some(&bone) = bone_index.get(q.bone.as_str()) else {
            errs.push(e(&at, format!("骨骼 {:?} 找不到", q.bone)));
            continue;
        };
        let bone = bone as i32;
        // `Missing control bone "…" for procedural bone "…"` ——
        // `simplify.cpp:3957-3959` 是**硬错误**，这里同样拒绝。
        let Some(&control) = bone_index.get(q.control.as_str()) else {
            errs.push(e(
                &at,
                format!("control 骨骼 {:?} 找不到（目标骨骼 {:?}）", q.control, q.bone),
            ));
            continue;
        };
        let control = control as i32;
        let base = q.base_pos.unwrap_or([0.0; 3]);
        let mut triggers = Vec::with_capacity(q.triggers.len());
        for t in &q.triggers {
            let tolerance = t.tolerance.to_radians();
            // 触发四元数 / 目标四元数：`AngleQuaternion` 的输入是弧度。
            let trigger = crate::bone_math::angle_quaternion([
                t.trigger[0].to_radians(),
                t.trigger[1].to_radians(),
                t.trigger[2].to_radians(),
            ]);
            let quat = crate::bone_math::angle_quaternion([
                t.angles[0].to_radians(),
                t.angles[1].to_radians(),
                t.angles[2].to_radians(),
            ]);
            let p = t.pos.unwrap_or([0.0; 3]);
            triggers.push(crate::model::ResolvedQuatInterpTrigger {
                // `1.0 / tolerance` —— 官方写盘时取倒数（`write.cpp:259`）。
                inv_tolerance: 1.0 / tolerance,
                trigger,
                pos: [base[0] + p[0], base[1] + p[1], base[2] + p[2]],
                quat,
            });
        }
        out.push(crate::model::ResolvedQuatInterpBone {
            bone,
            control,
            triggers,
        });
    }
    compiled.resolved_quat_interp_bones = out;

    if errs.is_empty() { Ok(()) } else { Err(errs) }
}

/// 把一个 lid 三元组的名字解析成 flexdesc 下标。
///
/// 填 `fd[3]`（lowerer/neutral/raiser 下标）、`tgt[3]`（target）、
/// `lid_base`（基准 `<type>` 的下标，落 `upperlidflexdesc`）。
fn resolve_lid(
    lid: &crate::model::EyeballLid,
    flexdesc_index: &HashMap<&str, usize>,
    fd: &mut [i32; 3],
    tgt: &mut [f32; 3],
    lid_base: &mut i32,
) -> Result<(), String> {
    let get = |name: &str| -> Result<i32, String> {
        flexdesc_index
            .get(name)
            .map(|&i| i as i32)
            .ok_or_else(|| format!("flexdesc {name:?} 在 [[flex_descriptors]] 里找不到"))
    };
    *lid_base = get(&lid.lid_flexdesc)?;
    fd[0] = get(&lid.lowerer.flexdesc)?;
    fd[1] = get(&lid.neutral.flexdesc)?;
    fd[2] = get(&lid.raiser.flexdesc)?;
    tgt[0] = lid.lowerer.target;
    tgt[1] = lid.neutral.target;
    tgt[2] = lid.raiser.target;
    Ok(())
}

/// 把一条序列的各格动画权重合并成序列权重：**逐骨骼取 MAX**。
///
/// # 官方依据（`simplify.cpp:302-318`，逐字）
///
/// ```c
/// for (i = 0; i < g_sequence.Count(); i++) {
///     for (n = 0; n < g_numbones; n++) {
///         g_sequence[i].weight[n] = 0.0;
///         for (j = 0; j < g_sequence[i].groupsize[0]; j++)
///             for (k = 0; k < g_sequence[i].groupsize[1]; k++)
///                 g_sequence[i].weight[n] =
///                     MAX( g_sequence[i].weight[n], g_sequence[i].panim[j][k]->weight[n] );
///     }
/// }
/// ```
///
/// ⚠️ 是 **MAX 而不是「取第一格」** —— blend 序列的每一格可能是不同动画，
/// 各自带不同权重表。
///
/// 空的 `cells`（理论上不会出现）回落到全 1。
fn merge_weights(cells: &[usize], anim_weights: &[Vec<f32>], n_bones: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; n_bones];
    for &c in cells {
        let Some(w) = anim_weights.get(c) else {
            continue;
        };
        for (n, slot) in out.iter_mut().enumerate() {
            if let Some(v) = w.get(n) {
                *slot = slot.max(*v);
            }
        }
    }
    // 没有任何格（或格为空）⟹ 全 1，与「无 `$weightlist`」一致。
    if cells.is_empty() {
        return default_weight_list(n_bones);
    }
    out
}

/// 把权重表解析成**逐骨骼权重数组**（`float[numbones]`）。
///
/// # 算法（`buildAnimationWeights`，`simplify.cpp:1646-1719` 逐字复刻）
///
/// ```c
/// for (i = 0; i < g_numweightlist; i++) {
///     if (i == 0) {                              // 隐式默认表
///         for (j) if (parent[j] != -1) weight[j] = -1;   // 子骨骼：未初始化
///                 else                 weight[j] = 1;    // 根骨骼：1
///     } else {
///         for (j) if (parent[j] != -1) weight[j] = g_weightlist[0].weight[j];
///                 else                 weight[j] = 0;    // 根骨骼：**0**
///     }
///     for (j = 0; j < numbones; j++) {           // 显式条目
///         k = findGlobalBone(bonename[j]);
///         if (k == -1) MdlError(...);            // 未知骨骼 = 硬错误
///         weight[k] = boneweight[j];
///     }
/// }
/// for (i) for (j) {                              // 沿父链补齐
///     if (weight[j] < 0.0 && parent[j] != -1)
///         weight[j] = weight[parent[j]];
/// }
/// ```
///
/// # ⚠️ 三个容易读错的点（都有实测判据）
///
/// 1. **`i != 0` 的表把根骨骼置 0，不是 1。**
///    实测 `$weightlist WL mid 0.5` → `[0, 0.5, 0.5, 0.5]`（root = **0**）。
/// 2. **`i != 0` 抄表 0 时抄到的是 `-1`（哨兵），不是 1。**
///    因为第 ③ 步（沿父链补齐）在**全部**表初始化完之后才跑 ——
///    抄的那一刻表 0 的子骨骼还是 `-1`。所以子骨骼最终由第 ③ 步
///    按**父链**决定，而不是「默认 1」。
/// 3. **沿父链补齐是「子取父」，方向向下。**
///    实测 `mid 0.5` 让 `leaf`/`tip` 也变 0.5。
///
/// # 返回
///
/// 与 `desc.weight_lists` **等长**的数组（不含隐式表 0）。
/// 调用方用 [`weight_list_index`] 拿到「官方下标」（从 1 起）后减 1 索引。
pub fn resolve_weight_lists(desc: &ModelDesc) -> Vec<Vec<f32>> {
    let n = desc.bones.len();
    let index = desc.bone_index();
    // ⚠️ 官方 `findGlobalBone` 用 **`stricmp`**（`simplify.cpp:2628`），
    // 所以 `$weightlist WL MID 0.5` 能命中 `mid`。
    // `bone_index()` 是**大小写敏感**的，直接用它会让这条静默失效 ——
    // 而 `validate()` 现在用的是大小写不敏感的表，两边必须一致。
    let ci: std::collections::HashMap<String, usize> = desc
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.to_ascii_lowercase(), i))
        .collect();
    let parents: Vec<i32> = desc
        .bones
        .iter()
        .map(|b| match b.parent.as_deref() {
            Some(p) => index.get(p).map(|v| *v as i32).unwrap_or(-1),
            None => -1,
        })
        .collect();

    // 表 0 = 隐式默认：根 1、子骨骼 -1（哨兵）。
    let mut default: Vec<f32> = (0..n)
        .map(|j| if parents[j] != -1 { -1.0 } else { 1.0 })
        .collect();

    let mut out: Vec<Vec<f32>> = Vec::with_capacity(desc.weight_lists.len());
    for wl in &desc.weight_lists {
        // `i != 0` 分支：根 0、子骨骼抄表 0（此刻表 0 的子骨骼仍是 -1）。
        let mut w: Vec<f32> = (0..n)
            .map(|j| if parents[j] != -1 { default[j] } else { 0.0 })
            .collect();
        for e in &wl.bones {
            if let Some(&k) = ci.get(&e.bone.to_ascii_lowercase()) {
                w[k] = e.weight;
            }
        }
        out.push(w);
    }

    // 沿父链补齐 —— **表 0 与所有具名表都要跑**。
    // 表 0 的子骨骼是 `-1`，必须由父链补成 1。
    for w in std::iter::once(&mut default).chain(out.iter_mut()) {
        // `j` 升序：父下标一定小于子下标（`validate()` 保证）。
        for j in 0..n {
            if w[j] < 0.0 && parents[j] != -1 {
                w[j] = w[parents[j] as usize];
            }
        }
    }

    out
}

/// 隐式默认权重表（`g_weightlist[0]`，全 1）。
///
/// 实测：`probe_weightlist_semantics.js` 的 `none` 行 → `[1,1,1,1]`。
pub fn default_weight_list(n_bones: usize) -> Vec<f32> {
    vec![1.0; n_bones]
}

/// 权重表名 → **官方下标**（从 **1** 起，0 是隐式默认表）。
///
/// 官方查找循环是 `for (i = 1; i < g_numweightlist; i++)` ——
/// **跳过 0**（`studiomdl.cpp:1719`）。找不到报
/// `unknown weightlist '<名>'`。
pub fn weight_list_index(desc: &ModelDesc, name: &str) -> Option<usize> {
    desc.weight_lists
        .iter()
        .position(|w| w.name.eq_ignore_ascii_case(name))
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelDesc;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    // ---------------------------------------------------------------------
    // 分配优化的回归测试
    // ---------------------------------------------------------------------
    //
    // `VertexKey` 的 `bones` 从 `Vec<(i32,u32)>` 换成了
    // 「定长数组 + 计数」（为了消掉每顶点一次堆分配）。这带来一个**真实的
    // 风险**：手写 `PartialEq`/`Hash` 时若把尾部未使用的槽位也算进去，
    // 「3 组绑定」与「1 组绑定 + 2 个零填充」就会被判成不同 ——
    // 去重失效 ⟹ 顶点数变多 ⟹ 产物字节变化。

    fn vkey(bones: &[[f32; 2]], pos: [f32; 3]) -> VertexKey {
        vertex_key(&Vertex {
            pos,
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            bones: bones.to_vec(),
        })
    }

    fn hash_of(k: &VertexKey) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        k.hash(&mut h);
        h.finish()
    }

    /// 绑定组数不同 ⟹ 键必须不同（否则会把不同蒙皮的顶点合并）。
    #[test]
    fn vertex_key_distinguishes_bone_counts() {
        let a = vkey(&[[0.0, 1.0]], [1.0, 2.0, 3.0]);
        let b = vkey(&[[0.0, 1.0], [1.0, 0.0]], [1.0, 2.0, 3.0]);
        let c = vkey(&[[0.0, 1.0], [1.0, 0.0], [0.0, 0.0]], [1.0, 2.0, 3.0]);
        assert_ne!(a, b, "1 组 vs 2 组必须不同");
        assert_ne!(b, c, "2 组 vs 3 组必须不同");
        assert_ne!(a, c);
    }

    /// 同内容同组数 ⟹ 必须相等**且哈希相同**。
    #[test]
    fn vertex_key_equal_for_identical_bones() {
        let a = vkey(&[[0.0, 0.5], [1.0, 0.5]], [1.0, 2.0, 3.0]);
        let b = vkey(&[[0.0, 0.5], [1.0, 0.5]], [1.0, 2.0, 3.0]);
        assert_eq!(a, b);
        assert_eq!(hash_of(&a), hash_of(&b), "相等对象必须同哈希");
    }

    /// **绑定顺序不同但语义相同**必须合并（键里会先排序）。
    #[test]
    fn vertex_key_is_order_insensitive_within_bones() {
        let a = vkey(&[[0.0, 0.5], [1.0, 0.5]], [1.0, 2.0, 3.0]);
        let b = vkey(&[[1.0, 0.5], [0.0, 0.5]], [1.0, 2.0, 3.0]);
        assert_eq!(a, b, "绑定顺序不应影响去重");
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    /// **尾部填充槽位绝不能参与比较**（这是手写 `PartialEq` 最容易犯的错）。
    ///
    /// `bones` 是定长数组 + `n_bones` 计数。若把整个数组拿来比，
    /// 「1 组绑定」与「1 组绑定 + 未使用槽位里的垃圾」就会被判成不同 ⟹
    /// 去重失效 ⟹ 顶点数变多 ⟹ 产物字节变化。
    ///
    /// 本测试往**未使用**的槽位里塞垃圾，断言相等性与哈希都不受影响。
    #[test]
    fn vertex_key_ignores_unused_padding_slots() {
        let mut a = vkey(&[[0.0, 1.0]], [1.0, 2.0, 3.0]);
        let b = vkey(&[[0.0, 1.0]], [1.0, 2.0, 3.0]);
        assert_eq!(a, b);
        assert_eq!(hash_of(&a), hash_of(&b));

        assert_eq!(a.n_bones, 1, "本夹具只有 1 组有效绑定");
        a.bones[1] = (999, 0xDEAD_BEEF);
        a.bones[2] = (123, 0x1234_5678);
        assert_eq!(
            a, b,
            "未使用的填充槽位不得参与相等性比较（否则去重会失效）"
        );
        assert_eq!(
            hash_of(&a),
            hash_of(&b),
            "未使用的填充槽位不得参与哈希（否则 HashMap 行为不一致）"
        );
    }

    /// 位置/法线/UV 任一不同 ⟹ 键不同。
    #[test]
    fn vertex_key_distinguishes_geometry() {
        let base = vkey(&[[0.0, 1.0]], [1.0, 2.0, 3.0]);
        let other_pos = vkey(&[[0.0, 1.0]], [1.0, 2.0, 3.5]);
        assert_ne!(base, other_pos, "位置不同必须区分");

        // 法线不同（构造完整 Vertex，走真实的 vertex_key 路径）。
        let other_normal = vertex_key(&Vertex {
            pos: [1.0, 2.0, 3.0],
            normal: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            bones: vec![[0.0, 1.0]],
        });
        assert_ne!(base, other_normal, "法线不同必须区分");

        // UV 不同。
        let other_uv = vertex_key(&Vertex {
            pos: [1.0, 2.0, 3.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.5, 0.0],
            bones: vec![[0.0, 1.0]],
        });
        assert_ne!(base, other_uv, "UV 不同必须区分");
    }

    /// **`-0.0` 必须归一化到 `0.0`**（否则同一位置会被拆成两个顶点）。
    #[test]
    fn vertex_key_normalizes_negative_zero() {
        let a = vkey(&[[0.0, 1.0]], [0.0, 1.0, 2.0]);
        let b = vkey(&[[0.0, 1.0]], [-0.0, 1.0, 2.0]);
        assert_eq!(a, b, "-0.0 与 0.0 必须视为同一位置");
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    /// `NaN` 用位模式比较 ⟹ 相同位模式的 NaN 必须相等（`==` 语义会失效）。
    #[test]
    fn vertex_key_compares_nan_by_bits() {
        let a = vkey(&[[0.0, 1.0]], [f32::NAN, 0.0, 0.0]);
        let b = vkey(&[[0.0, 1.0]], [f32::NAN, 0.0, 0.0]);
        assert_eq!(a, b, "同一位模式的 NaN 必须相等（按位比较）");
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    /// 键的相等性必须与「直接用 HashMap 去重」的结果一致 ——
    /// 这是 `build_meshes` 真正依赖的性质。
    #[test]
    fn vertex_key_dedups_consistently_in_hashmap() {
        let keys = [
            vkey(&[[0.0, 1.0]], [1.0, 2.0, 3.0]),
            vkey(&[[0.0, 1.0]], [1.0, 2.0, 3.0]), // 重复
            vkey(&[[1.0, 1.0]], [1.0, 2.0, 3.0]), // 不同骨骼
            vkey(&[[0.0, 1.0]], [1.0, 2.0, 4.0]), // 不同位置
            vkey(&[[1.0, 0.5], [0.0, 0.5]], [1.0, 2.0, 3.0]),
            vkey(&[[0.0, 0.5], [1.0, 0.5]], [1.0, 2.0, 3.0]), // 与上一条等价
        ];
        let mut m: HashMap<VertexKey, u32> = HashMap::new();
        for k in keys {
            let n = m.len() as u32;
            m.entry(k).or_insert(n);
        }
        assert_eq!(m.len(), 4, "6 个键应去重成 4 个");
    }

    /// **骨骼查找表提出顶点循环后，报错信息必须仍然准确。**
    ///
    /// `VertexBoneMap` 把 `node_names` / `desc_index` / 两个计数缓存起来，
    /// 报错文案里用的 `node_count` 必须等于 `smd.nodes.len()`，
    /// 否则「nodes 段只有 N 项」会给出错误数字。
    #[test]
    fn vertex_bone_map_reports_correct_node_count() {
        let dir = std::env::temp_dir().join("mdlc_opt_bonemap_test");
        let _ = std::fs::create_dir_all(&dir);
        // SMD 引用了一个不存在的骨骼下标 7（nodes 只有 2 项）。
        let smd = SMD.replace("1 1 1.000000", "1 7 1.000000");
        write(&dir, "bad.smd", &smd);
        let toml = desc_toml("bad.smd");
        let desc: ModelDesc = toml::from_str(&toml).unwrap();
        let errs = compile(&desc, &dir).unwrap_err();
        let joined = errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("nodes 段只有 2 项"),
            "报错必须给出正确的 nodes 项数，实际：{joined}"
        );
    }


    /// 与 SMD 配套的最小描述。
    fn desc_toml(smd: &str) -> String {
        format!(
            r#"
[model]
name = "models/test/minimal.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{{ name = "models/test/myprop" }}]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "{smd}"
"#
        )
    }

    const SMD: &str = r#"version 1
nodes
  0 "root" -1
  1 "tip" 0
end
skeleton
  time 0
    0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
    1 0.000000 0.000000 8.000000 0.000000 0.000000 0.000000
end
triangles
myprop
  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000
  1 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000
  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000
end
"#;

    // ---- `$animation` 的 `subtract` ----
    //
    // 真实项目（miku SPAS-12）里 `look_poses.smd` 与 `a_idle.smd` 的
    // **骨骼顺序不同**（118 个位置里 77 个不同），而 `subtract` 必须按
    // **名字**对齐后相减。这两条测试锁住「按名字映射」与「减除生效」。

    /// 两个 SMD 的骨骼顺序**不同**时，`subtract` 仍按**名字**正确相减。
    ///
    /// 构造：`base.smd` 顺序是 `root, mid, tip`，`pose.smd` 顺序是
    /// `root, tip, mid`（**打乱**）。`tip` 在两边的第 0 帧都是绕 Z 20°，
    /// 而 `pose.smd` 第 0 帧的 `tip` 是 35° —— 减完应当是 15°。
    ///
    /// 若实现按**下标**相减，`tip`（`pose` 的第 1 项）会减到 `base` 的
    /// 第 1 项（`mid`，0°），得到 35° —— 与 15° 差得很明显。
    #[test]
    fn subtract_aligns_bones_by_name_not_index() {
        let d = tmpdir("subtract-byname");
        // base.smd：root(0), mid(1), tip(2)；tip 绕 Z 20°（0.349066 rad）
        let base = r#"version 1
nodes
  0 "root" -1
  1 "mid" 0
  2 "tip" 1
end
skeleton
  time 0
    0 0 0 0 0 0 0
    1 0 0 4 0 0 0
    2 0 0 8 0 0 0.349066
end
triangles
myprop
  2 -8 -8 8 0 0 1 0 0 1 2 1
  2 8 -8 8 0 0 1 1 0 1 2 1
  2 0 8 8 0 0 1 0.5 1 1 2 1
end
"#;
        // pose.smd：**顺序打乱** —— root(0), tip(1), mid(2)
        // tip 第 0 帧绕 Z 35°（弧度 0.610865）
        let pose = r#"version 1
nodes
  0 "root" -1
  1 "tip" 0
  2 "mid" 1
end
skeleton
  time 0
    0 0 0 0 0 0 0
    1 0 0 8 0 0 0.610865
    2 0 0 4 0 0 0
end
triangles
myprop
  1 -8 -8 8 0 0 1 0 0 1 1 1
  1 8 -8 8 0 0 1 1 0 1 1 1
  1 0 8 8 0 0 1 0.5 1 1 1 1
end
"#;
        write(&d, "base.smd", base);
        write(&d, "pose.smd", pose);
        let toml = r#"
[model]
name = "models/test/sub.mdl"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "mid"
parent = "root"

[[bones]]
name = "tip"
parent = "mid"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "base.smd"

[[animations]]
name = "base"
smd = "base.smd"

[[animations]]
name = "posed"
smd = "pose.smd"
frames = [0, 0]
subtract = "base"
subtract_frame = 0
"#;
        let desc: ModelDesc = toml::from_str(toml).expect("TOML 解析");
        let c = compile(&desc, &d).expect("编译");
        assert_eq!(c.animations.len(), 2);
        // 先确认**源数据**是对的：base 的 tip 是 20°。
        assert!(
            (c.animations[0].frames[0][2].rotation[2].to_degrees() - 20.0).abs() < 0.01,
            "base 的 tip 应为 20°，实际 {}",
            c.animations[0].frames[0][2].rotation[2].to_degrees()
        );
        let posed = &c.animations[1];
        assert!(posed.delta, "subtract ⇒ delta");
        // `tip` 是**描述骨骼**的第 2 项。
        let tip = posed.frames[0][2].rotation;
        let deg = tip[2].to_degrees();
        assert!(
            (deg - 15.0).abs() < 0.01,
            "tip 的 Z 应为 35° − 20° = 15°，实际 {deg}°（按名字对齐失败？）"
        );
        // `mid` 两边都是 0° ⇒ 减完仍是 0。
        let mid = posed.frames[0][1].rotation;
        assert!(
            mid.iter().all(|v| v.abs() < 1e-4),
            "mid 两边同为 0° ⇒ 差值应为 0，实际 {mid:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `subtract` 出来的动画在**写出阶段**只保留真正变化的骨骼。
    ///
    /// 这是 miku `look_*` 的核心判据：官方 `look_down` 只有 **1** 条轨道
    /// （只有 `ankle`/`body` 变了），而不是全部 118 根。
    #[test]
    fn subtract_marks_only_changed_bones() {
        let d = tmpdir("subtract-tracks");
        write(&d, "base.smd", SMD);
        // pose.smd 与 base.smd **骨骼顺序相同**，只有 tip 的 Z 不同。
        let pose = r#"version 1
nodes
  0 "root" -1
  1 "tip" 0
end
skeleton
  time 0
    0 0 0 0 0 0 0
    1 0 0 8 0 0 0.5
end
triangles
myprop
  1 -8 -8 8 0 0 1 0 0 1 1 1
  1 8 -8 8 0 0 1 1 0 1 1 1
  1 0 8 8 0 0 1 0.5 1 1 1 1
end
"#;
        write(&d, "pose.smd", pose);
        let toml = format!(
            "{}[[animations]]\nname = \"base\"\nsmd = \"base.smd\"\n\n\
             [[animations]]\nname = \"posed\"\nsmd = \"pose.smd\"\nframes = [0, 0]\n\
             subtract = \"base\"\nsubtract_frame = 0\n\n\
             [[sequences]]\nname = \"posed\"\nsmd = \"posed\"\n",
            desc_toml("base.smd")
        );
        let desc: ModelDesc = toml::from_str(&toml).expect("TOML 解析");
        let c = compile(&desc, &d).expect("编译");
        assert_eq!(c.animations.len(), 2);
        let posed = &c.animations[1];
        // root 两边都是 0 ⇒ 差值 0；tip 差 0.5 rad ⇒ 非 0。
        assert!(
            posed.frames[0][0].rotation.iter().all(|v| v.abs() < 1e-4),
            "root 应无差异"
        );
        assert!(
            (posed.frames[0][1].rotation[2] - 0.5).abs() < 1e-4,
            "tip 的 Z 应为 0.5 rad，实际 {}",
            posed.frames[0][1].rotation[2]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- `$definebone` 声明的骨骼即使 SMD 里没有也要保留 ----
    //
    // 官方 `BuildGlobalBonetable`（`simplify.cpp:3616-3654`）**先**把
    // `g_importbone`（`$definebone` 收集的）插进骨骼表，**再**并入各 SMD
    // 用到的骨骼（同名靠 `findGlobalBone` 去重）。
    //
    // 判据是 `parity/myprop.qc` → 官方 `myprop.mdl`：
    // QC 写 `$definebone "tip" "root" 0 0 8`，而 `myprop-ref.smd` 的
    // `nodes` **只有 root** —— 官方产物仍是 2 根骨骼，
    // `BONE[1].pos = [0, 0, 8]`、`flags = 0x200`。

    /// SMD 里没有、但 `[[bones]]` 显式给了 `position` 的骨骼**必须保留**，
    /// 参考姿态取那个显式值。
    ///
    /// 这条曾经缺失，症状是 `verify_parity.ps1` 长期红灯：
    /// 报「骨骼数 2 vs 1」以及十几处由它引起的段偏移差异 ——
    /// **看起来像代码坏了，实际是一个真实缺口**。
    #[test]
    fn bone_declared_but_absent_from_smd_is_kept() {
        let d = tmpdir("definebone-only");
        // SMD 只有 root（与 `parity/myprop-ref.smd` 同形态）。
        // 顶点行 12 个 token：parentBone + pos3 + nrm3 + uv2 + **links 数** + bone + weight。
        let smd = r#"version 1
nodes
  0 "root" -1
end
skeleton
  time 0
    0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
end
triangles
myprop
  0 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 0 1.000000
  0 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 0 1.000000
  0 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 0 1.000000
end
"#;
        write(&d, "myprop-ref.smd", smd);
        let toml = r#"
[model]
name = "models/test/dbonly.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"
position = [0.0, 0.0, 8.0]

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "myprop-ref.smd"

[[sequences]]
name = "idle"
smd = "myprop-ref.smd"
fps = 30.0
"#;
        let desc2: ModelDesc = toml::from_str(toml).expect("TOML 解析");
        let c = compile(&desc2, &d).expect("编译");
        assert_eq!(c.desc.bones.len(), 2, "tip 必须保留（官方 numbones = 2）");
        // 动画帧里 tip 的姿态取自 `[[bones]].position`（= `$definebone` 的
        // rawLocal）—— `poses` 是 SMD 参考帧的**原始记录**，只含 root，
        // 所以要看**编译后的动画帧**。
        let frames = &c.animations[0].frames;
        assert_eq!(
            frames[0][1].position,
            [0.0, 0.0, 8.0],
            "tip 的参考位置应来自显式声明，实际 {:?}",
            frames[0][1].position
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反向：SMD 里没有、`[[bones]]` 也**没给**姿态的骨骼必须**报错**。
    ///
    /// 静默用 `[0,0,0]` 会让骨骼塌到原点，表现为顶点被拉向世界原点 ——
    /// 不会报错，只会让模型在游戏里扭曲。
    #[test]
    fn bone_absent_from_smd_without_explicit_pose_errors() {
        let d = tmpdir("definebone-missing");
        let smd = r#"version 1
nodes
  0 "root" -1
end
skeleton
  time 0
    0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
end
triangles
myprop
  0 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 0 1.000000
  0 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 0 1.000000
  0 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 0 1.000000
end
"#;
        write(&d, "myprop-ref.smd", smd);
        let toml = r#"
[model]
name = "models/test/dbmiss.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "myprop-ref.smd"

[[sequences]]
name = "idle"
smd = "myprop-ref.smd"
fps = 30.0
"#;
        let desc: ModelDesc = toml::from_str(toml).expect("TOML 解析");
        let errs = compile(&desc, &d).expect_err("缺姿态的骨骼必须报错");
        assert!(
            errs.iter().any(|e| e.message.contains("tip")),
            "错误里应点名 tip，实际：{errs:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 每个用例用**唯一**的临时目录：测试并行跑，共用目录会互相删文件。
    fn tmpdir(tag: &str) -> PathBuf {        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mdlc-test-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // ---- flex / eyeball / mouth ----
    //
    // 这三条规则都**不在 episode1 源码里**（`dummy_eyelid` grep 零命中、
    // `forward` 的符号被 L4D2 改过），只能用受控实验钉死。对应的官方产物：
    // `docs/_probe/smdl/{fx2,fxr1,fxl1}.qc` → `docs/_probe/artifacts/*.mdl`，
    // 逐字段对照脚本 `docs/_probe/cmp_flex.js`。

    /// **`dummy_eyelid` 自动追加**：只要某个 eyeball 缺 lid 数据，
    /// 就在 flexdesc 数组**末尾**追加一条（若用户没显式写过同名）。
    ///
    /// 受控实验 `fxr1.qc` —— **一个 flexdesc 都没写**，只写了两条 `eyeball`，
    /// 官方产物仍是 `numflexdesc=1`、`fd[0]="dummy_eyelid"`；
    /// `fxl1.qc`（`localvar alpha beta` + eyeball）得到
    /// `[alpha, beta, dummy_eyelid]` 证明它**永远排在最后**。
    /// 对照：4 个 survivor 模型写了 `eyelid`，产物**不含** dummy_eyelid。
    #[test]
    fn dummy_eyelid_appended_for_eyeball_without_eyelid() {
        let d = tmpdir("dummyeyelid");
        write(&d, "myprop-ref.smd", SMD);
        let toml = format!(
            r#"
{}

[[flex_descriptors]]
name = "alpha"

[[bodyparts.models.eyeballs]]
bone = "tip"
org = [0.0, 0.0, 8.0]
material = "models/test/myprop"
radius = 0.5
"#,
            desc_toml("myprop-ref.smd")
        );
        let desc = ModelDesc::from_toml(&toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let names: Vec<&str> = c
            .desc
            .flex_descriptors
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["alpha", DUMMY_EYELID],
            "dummy_eyelid 应追加在末尾（受控实验 fxl1）"
        );
        // 三个 lid 槽与两个 `*lidflexdesc` 全指向它，target 恒 [-1,0,1]。
        let eb = &c.bodyparts[0].models[0].eyeballs[0];
        assert_eq!(eb.upperlidflexdesc, 1, "upperlidflexdesc → dummy_eyelid");
        assert_eq!(eb.lowerlidflexdesc, 1, "lowerlidflexdesc → dummy_eyelid");
        assert_eq!(eb.upperflexdesc, [1, 1, 1]);
        assert_eq!(eb.lowerflexdesc, [1, 1, 1]);
        assert_eq!(eb.uppertarget, DUMMY_LID_TARGETS);
        assert_eq!(eb.lowertarget, DUMMY_LID_TARGETS);
        std::fs::remove_dir_all(&d).ok();
    }

    /// **两个 lid 都写齐了才不补** `dummy_eyelid`
    /// （对照 survivor 语料：4/4 上下眼睑都有，产物 0/4 不含 dummy）。
    ///
    /// 只写一个（本例只给 `upper_lid`）仍算「缺 lid」→ 追加 dummy 到末尾，
    /// **缺的那个** 指向 dummy，**给了的那个** 用真实值。
    #[test]
    fn no_dummy_eyelid_only_when_both_lids_given() {
        let d = tmpdir("nodummy");
        write(&d, "myprop-ref.smd", SMD);
        let toml = format!(
            r#"
{}

[[flex_descriptors]]
name = "upper"

[[flex_descriptors]]
name = "upper_lowerer"

[[flex_descriptors]]
name = "upper_neutral"

[[flex_descriptors]]
name = "upper_raiser"

[[bodyparts.models.eyeballs]]
bone = "tip"
org = [0.0, 0.0, 8.0]
material = "models/test/myprop"
radius = 0.5

[bodyparts.models.eyeballs.upper_lid]
lid_flexdesc = "upper"
lowerer = {{ flexdesc = "upper_lowerer", target = -0.1 }}
neutral = {{ flexdesc = "upper_neutral", target = 0.2 }}
raiser = {{ flexdesc = "upper_raiser", target = 0.3 }}
"#,
            desc_toml("myprop-ref.smd")
        );
        let desc = ModelDesc::from_toml(&toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let names: Vec<&str> = c
            .desc
            .flex_descriptors
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        // 只给了 upper_lid → 仍缺 lower_lid → 追加 dummy（下标 4）。
        assert_eq!(names, vec!["upper", "upper_lowerer", "upper_neutral", "upper_raiser", DUMMY_EYELID]);
        let eb = &c.bodyparts[0].models[0].eyeballs[0];
        assert_eq!(eb.upperlidflexdesc, 0, "基准 → upper");
        assert_eq!(eb.upperflexdesc, [1, 2, 3]);
        assert_eq!(eb.uppertarget, [-0.1, 0.2, 0.3]);
        // 下眼睑缺省走 dummy。
        assert_eq!(eb.lowerlidflexdesc, 4, "lowerlidflexdesc → dummy");
        assert_eq!(eb.lowerflexdesc, [4, 4, 4]);
        assert_eq!(eb.lowertarget, DUMMY_LID_TARGETS);
        std::fs::remove_dir_all(&d).ok();
    }

    /// **`up`/`forward` 是骨骼空间的逆旋转**，且 `forward` 的符号是
    /// **`(0,-1,0)`** —— episode1 源码那个 `(0,1,0)` 带
    /// `// FIXME: this is backwards`，L4D2 修掉了。
    ///
    /// 真实语料判据（4 个 survivor / 8 条 eyeball）：
    /// `forward == VectorIRotate((0,-1,0), poseToBone)` **8/8**，
    /// `(0,+1,0)` **0/8**；`up == VectorIRotate((0,0,1))` **8/8**。
    ///
    /// 这里用**恒等参考姿态**（`tip` 无旋转）：`poseToBone` 是单位阵，
    /// 于是 `up=(0,0,1)`、`forward=(0,-1,0)` 可直接读出符号。
    #[test]
    fn eyeball_up_forward_use_bone_space_with_negated_forward() {
        let d = tmpdir("ebfw");
        write(&d, "myprop-ref.smd", SMD);
        let toml = format!(
            r#"
{}

[[bodyparts.models.eyeballs]]
bone = "tip"
org = [0.0, 0.0, 8.0]
material = "models/test/myprop"
radius = 0.5
"#,
            desc_toml("myprop-ref.smd")
        );
        let desc = ModelDesc::from_toml(&toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let eb = &c.bodyparts[0].models[0].eyeballs[0];
        let near = |a: [f32; 3], b: [f32; 3]| {
            a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-6)
        };
        assert!(near(eb.up, [0.0, 0.0, 1.0]), "up 应为 (0,0,1)，got {:?}", eb.up);
        assert!(
            near(eb.forward, [0.0, -1.0, 0.0]),
            "forward 应为 (0,-1,0)（L4D2 修掉了源码的 FIXME），got {:?}",
            eb.forward
        );
        // `org` 也要落到骨骼空间：世界 (0,0,8) 在 tip 局部（tip 原点在世界
        // (0,0,8)）就是 (0,0,0)。
        assert!(
            near(eb.org, [0.0, 0.0, 0.0]),
            "org 应逆变换到骨骼空间（got {:?}）",
            eb.org
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// **官方 `flexcontroller` 命令会顺带生成一条 flexcontrollerui**。
    ///
    /// 受控实验 `fx2.qc`（3 个 `flexcontroller`、**没有任何 ui 命令**）的
    /// 官方产物是 `numflexcontrollerui=3`、`flexcontrolleruiindex=2240`
    /// （= flexop 区末尾），每条的 `szindex0` 指向对应 fc 记录（相对自身、
    /// 恒负）、`szindex1=0`、`stereo=0`、`remaptype=0`。
    ///
    /// 报告原先标注「ui 是 DMX 专有」—— 实测 QC 也会生成，所以必须自动产。
    #[test]
    fn each_flexcontroller_gets_a_flexcontrollerui() {
        let d = tmpdir("fcui");
        write(&d, "myprop-ref.smd", SMD);
        let toml = format!(
            r#"
{}

[[flex_controllers]]
name = "right_lid_raiser"
type = "lid"

[[flex_controllers]]
name = "jaw_drop"
type = "mouth"
"#,
            desc_toml("myprop-ref.smd")
        );
        let desc = ModelDesc::from_toml(&toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let uis = &c.resolved_flex_controller_ui;
        assert_eq!(uis.len(), 2, "每条 fc 一条 ui");
        assert_eq!(uis[0].name, "right_lid_raiser", "ui 名 = fc 名（fx2 实测）");
        assert_eq!(uis[0].fc0, 0, "szindex0 指向对应 fc 自身");
        assert_eq!(uis[1].fc0, 1);
        assert!(uis.iter().all(|u| !u.stereo && u.fc1.is_none()), "自动产的都是单声道");
        std::fs::remove_dir_all(&d).ok();
    }

    // ---- jigglebone（`$jigglebone` → `mstudiojigglebone_t`）----
    //
    // 规则全部来自受控实验（`docs/_probe/smdl/jig{1,2,4,6,7,8}.qc` →
    // `artifacts/jig*.mdl`），验收脚本 `docs/_probe/cmp_jiggle.js`。
    // **调研报告 §6.2 的 `flags` 结论有反例**（见 `allow_length_flex`）。

    /// `flags` 的置位规则：块出现即置 `LENGTH(0x20)`，
    /// `allow_length_flex` 把它**清掉**。
    ///
    /// 受控实验对照：`jig4`（`is_flexible` + `allow_length_flex`）= `0x01`；
    /// `jig6` 的 `is_flexible`（无该键）= `0x21`、`is_rigid` = `0x22`；
    /// `jig2`（`is_rigid` + `angle_constraint` + `has_base_spring`）= `0x72`。
    #[test]
    fn jiggle_flags_follow_controlled_experiments() {
        let d = tmpdir("jigflags");
        write(&d, "myprop-ref.smd", SMD);
        let base = desc_toml("myprop-ref.smd");

        // jig6 的 is_flexible：0x21
        let t = format!(
            "{base}\n[[jiggle_bones]]\nbone = \"tip\"\n[jiggle_bones.is_flexible]\nyaw_stiffness = 100.0\n"
        );
        let c = compile(&ModelDesc::from_toml(&t).unwrap(), &d).unwrap();
        assert_eq!(c.resolved_jiggle_bones[0].flags, 0x21, "is_flexible → 0x21");

        // jig4：allow_length_flex 清掉 0x20
        let t = format!(
            "{base}\n[[jiggle_bones]]\nbone = \"tip\"\n[jiggle_bones.is_flexible]\nyaw_stiffness = 100.0\nallow_length_flex = true\n"
        );
        let c = compile(&ModelDesc::from_toml(&t).unwrap(), &d).unwrap();
        assert_eq!(
            c.resolved_jiggle_bones[0].flags, 0x01,
            "allow_length_flex 应清掉 LENGTH（受控实验 jig4 = 0x01）"
        );

        // jig6 的 is_rigid：0x22
        let t = format!(
            "{base}\n[[jiggle_bones]]\nbone = \"tip\"\n[jiggle_bones.is_rigid]\ntip_mass = 100.0\n"
        );
        let c = compile(&ModelDesc::from_toml(&t).unwrap(), &d).unwrap();
        assert_eq!(c.resolved_jiggle_bones[0].flags, 0x22, "is_rigid → 0x22");

        // jig2：is_rigid + angle_constraint + base_spring → 0x72
        let t = format!(
            "{base}\n[[jiggle_bones]]\nbone = \"tip\"\n\
             [jiggle_bones.is_rigid]\nangle_constraint = 60.0\n\
             [jiggle_bones.has_base_spring]\nbase_stiffness = 800.0\n"
        );
        let c = compile(&ModelDesc::from_toml(&t).unwrap(), &d).unwrap();
        assert_eq!(
            c.resolved_jiggle_bones[0].flags, 0x72,
            "RIGID|ANGLE|LENGTH|BASE_SPRING"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// 角度输入是**度**、写盘转**弧度**；缺省值按报告 §6.3（这节无错）。
    ///
    /// 判据来自 `jig1`：`angle_constraint 60` → `π/3`、
    /// `yaw_constraint -30 40` → `−0.5235988/0.6981317`、
    /// `pitch_constraint -20 50` → `−0.3490659/0.8726646`。
    #[test]
    fn jiggle_angles_are_degrees_in_radians_out() {
        let d = tmpdir("jigdeg");
        write(&d, "myprop-ref.smd", SMD);
        let t = format!(
            "{}\n[[jiggle_bones]]\nbone = \"tip\"\n[jiggle_bones.is_flexible]\n\
             angle_constraint = 60.0\nyaw_constraint = [-30.0, 40.0]\n\
             pitch_constraint = [-20.0, 50.0]\n",
            desc_toml("myprop-ref.smd")
        );
        let c = compile(&ModelDesc::from_toml(&t).unwrap(), &d).unwrap();
        let j = &c.resolved_jiggle_bones[0];
        let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(near(j.angle_limit, std::f32::consts::PI / 3.0), "60° → π/3");
        assert!(near(j.min_yaw, -30.0f32.to_radians()), "-30°");
        assert!(near(j.max_yaw, 40.0f32.to_radians()), "40°");
        assert!(near(j.min_pitch, -20.0f32.to_radians()), "-20°");
        assert!(near(j.max_pitch, 50.0f32.to_radians()), "50°");
        assert_eq!(j.flags, 0x3d, "FLEXIBLE|YAW|PITCH|ANGLE|LENGTH");
        std::fs::remove_dir_all(&d).ok();
    }

    /// 缺省值：`length` 10、`*Stiffness` 100、`base*` 三轴 ±100、其余 0。
    #[test]
    fn jiggle_defaults_match_corpus() {
        let d = tmpdir("jigdef");
        write(&d, "myprop-ref.smd", SMD);
        // 只给块、不给任何字段。
        let t = format!(
            "{}\n[[jiggle_bones]]\nbone = \"tip\"\n[jiggle_bones.is_flexible]\n",
            desc_toml("myprop-ref.smd")
        );
        let c = compile(&ModelDesc::from_toml(&t).unwrap(), &d).unwrap();
        let j = &c.resolved_jiggle_bones[0];
        assert_eq!(j.length, 10.0, "length 缺省 10");
        assert_eq!(j.yaw_stiffness, 100.0);
        assert_eq!(j.pitch_stiffness, 100.0);
        assert_eq!(j.along_stiffness, 100.0);
        assert_eq!(j.base_stiffness, 100.0, "base_stiffness 未写 base_spring 时也是 100");
        assert_eq!(j.base_min_left, -100.0);
        assert_eq!(j.base_max_left, 100.0);
        assert_eq!(j.base_min_up, -100.0);
        assert_eq!(j.base_max_up, 100.0);
        assert_eq!(j.base_min_forward, -100.0);
        assert_eq!(j.base_max_forward, 100.0);
        assert_eq!(j.tip_mass, 0.0);
        assert_eq!(j.yaw_damping, 0.0);
        std::fs::remove_dir_all(&d).ok();
    }

    /// `procindex` 相对**骨骼记录自身**，且记录按**书写顺序**排列。
    ///
    /// 判据来自 `jig8`：QC 写 knee(2)→ankle(3)→hip(1)，产物里
    /// `@1528 knee` / `@1648 ankle` / **`@1768 hip`** ——
    /// 调研报告 §6.4 的「按骨骼下标升序」是**错的**。
    ///
    /// 这里验证「解析保序」这半步（写字节的顺序由 `mdl_writer` 保证）。
    #[test]
    fn jiggle_records_keep_written_order() {
        let d = tmpdir("jigord");
        write(&d, "myprop-ref.smd", SMD);
        let t = format!(
            "{}\n[[jiggle_bones]]\nbone = \"tip\"\n[jiggle_bones.is_flexible]\nyaw_stiffness = 111.0\n\
             [[jiggle_bones]]\nbone = \"root\"\n[jiggle_bones.is_flexible]\nyaw_stiffness = 222.0\n",
            desc_toml("myprop-ref.smd")
        );
        let c = compile(&ModelDesc::from_toml(&t).unwrap(), &d).unwrap();
        let j = &c.resolved_jiggle_bones;
        assert_eq!(j.len(), 2);
        assert_eq!(j[0].bone, 1, "第一条仍是 tip(1)（书写顺序，不是下标序）");
        assert_eq!(j[0].yaw_stiffness, 111.0);
        assert_eq!(j[1].bone, 0, "第二条是 root(0)");
        assert_eq!(j[1].yaw_stiffness, 222.0);
        std::fs::remove_dir_all(&d).ok();
    }

    // ---- `$includemodel` ----
    //
    // 官方**只写名字**、从不读被包含的 `.mdl`（`g_numincludemodels` 在
    // 整个 studiomdl 里只出现 2 次），所以这里只需确认描述层能收下路径，
    // 且**不做存在性检查**、**不补 `models/` 前缀**。

    /// `$includemodel` 只存字符串：指向不存在的文件也**不该**报错。
    ///
    /// 官方行为：`Cmd_IncludeModel`（`studiomdl.cpp:5961-5967`）无去重、
    /// 无存在性检查，写不存在的文件也照样进表。
    #[test]
    fn include_models_accept_nonexistent_files() {
        let d = tmpdir("incmodel");
        write(&d, "myprop-ref.smd", SMD);
        // ⚠️ `include_models` 是**顶层**键，必须写在 `[model]`/`[[bones]]`
        // 等表**之前** —— 追加在末尾会落进最后一张表里（TOML 语义）。
        let t = format!(
            "include_models = [\"models/infected/anim_boomer.mdl\", \"models/nope/missing.mdl\"]\n{}",
            desc_toml("myprop-ref.smd")
        );
        let desc = ModelDesc::from_toml(&t).expect("应能解析");
        let c = compile(&desc, &d).expect("指向不存在的 .mdl 不应报错");
        assert_eq!(
            c.desc.include_models,
            vec![
                "models/infected/anim_boomer.mdl".to_string(),
                "models/nope/missing.mdl".to_string()
            ],
            "原样保留、不补前缀、不校验存在性"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    // ---- 自动生成 hitbox（`SetupHitBoxes`，`simplify.cpp:6884-6973`）----
    /// 没写显式 hitbox 时，`compile()` 应自动生成一个 `default` set，
    /// 并置 `STUDIOHDR_FLAGS_AUTOGENERATED_HITBOX`（**0x1**）。
    ///
    /// 实测语料：**3333/3333** 个模型都有 hitbox set，其中 3104 个
    /// 自动生成（带 `0x1`）、63 个显式。
    #[test]
    fn autogenerates_hitbox_when_none_given() {
        let d = tmpdir("autohb");
        write(&d, "myprop-ref.smd", SMD);
        let desc = ModelDesc::from_toml(&desc_toml("myprop-ref.smd")).unwrap();
        assert!(desc.hitboxes.boxes.is_empty(), "前提：描述里没有 hitbox");
        let c = compile(&desc, &d).expect("应能编译");

        assert!(c.desc.hitboxes.autogenerated, "应标记为自动生成");
        assert_eq!(c.desc.hitboxes.set_name.as_deref(), Some("default"));
        assert_eq!(
            c.desc.model.extra_flags.unwrap_or(0) & crate::mdl_writer::FLAG_AUTOGENERATED_HITBOX,
            crate::mdl_writer::FLAG_AUTOGENERATED_HITBOX,
            "应置 0x1 标志"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// **核心判据**：自动 hitbox 的 bbox 必须与官方 `dump_hboxes` 的真值一致。
    ///
    /// 用受控实验 `docs/_probe/smdl/iph1.qc` 的真值（`studiomdl -h` 输出）：
    ///
    /// ```text
    /// $hbox 0 "b0" -8.00 -8.00 0.00  10.00 8.00 20.00
    /// $hbox 0 "b1" -18.00 -8.00 0.00  20.00 8.00 12.00
    /// $hbox 0 "b2" -38.00 -8.00 0.00  30.00 8.00 20.00
    /// $hbox 0 "b3" -64.00 -6.00 0.00  0.00 6.00 30.00
    /// ```
    ///
    /// 注意 `bbmin` 里出现 `0.00`（甚至 `b0` 的 `bbmax[2] == 20` 而
    /// `bmin[2] == 0`）—— 那是 `g_bUseBoneInBBox == true` 让 bbox 从
    /// **全 0** 起步的直接后果。
    #[test]
    fn autohitbox_matches_official_dump_hboxes() {
        let d = tmpdir("autohb-truth");
        // 与 `iph.smd` 等价的 4 骨骼链 + 混合权重。
        let smd = r#"version 1
nodes
0 "b0" -1
1 "b1" 0
2 "b2" 1
3 "b3" 2
end
skeleton
time 0
0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
1 10.000000 0.000000 0.000000 0.000000 0.000000 0.000000
2 20.000000 0.000000 0.000000 0.000000 0.000000 0.000000
3 30.000000 0.000000 0.000000 0.000000 0.000000 0.000000
end
triangles
myprop
0 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 3 0 0.333333 1 0.333333 2 0.333333
0 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 3 0 0.333333 1 0.333333 2 0.333333
0 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 3 0 0.333333 1 0.333333 2 0.333333
0 -6.000000 -6.000000 12.000000 0.000000 0.000000 1.000000 0.100000 0.100000 3 0 0.100000 1 0.200000 2 0.700000
0 6.000000 -6.000000 12.000000 0.000000 0.000000 1.000000 0.900000 0.100000 3 1 0.500000 2 0.300000 3 0.200000
0 0.000000 6.000000 20.000000 0.000000 0.000000 1.000000 0.500000 0.900000 3 2 0.600000 3 0.300000 0 0.100000
0 -4.000000 4.000000 30.000000 0.000000 0.000000 1.000000 0.200000 0.500000 1 3 1.000000
0 4.000000 4.000000 30.000000 0.000000 0.000000 1.000000 0.800000 0.500000 1 3 1.000000
0 0.000000 6.000000 30.000000 0.000000 0.000000 1.000000 0.500000 0.900000 1 3 1.000000
end
"#;
        write(&d, "iph.smd", smd);
        let toml = r#"
[model]
name = "models/test/iph.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "b0"

[[bones]]
name = "b1"
parent = "b0"

[[bones]]
name = "b2"
parent = "b1"

[[bones]]
name = "b3"
parent = "b2"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "iph.smd"
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");

        let got: Vec<(&str, [f32; 3], [f32; 3])> = c
            .desc
            .hitboxes
            .boxes
            .iter()
            .map(|h| (h.bone.as_str(), h.bbmin, h.bbmax))
            .collect();
        let want: [(&str, [f32; 3], [f32; 3]); 4] = [
            ("b0", [-8.0, -8.0, 0.0], [10.0, 8.0, 20.0]),
            ("b1", [-18.0, -8.0, 0.0], [20.0, 8.0, 12.0]),
            ("b2", [-38.0, -8.0, 0.0], [30.0, 8.0, 20.0]),
            ("b3", [-64.0, -6.0, 0.0], [0.0, 6.0, 30.0]),
        ];
        assert_eq!(got.len(), want.len(), "应有 4 个 hitbox，实际 {got:?}");
        for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
            assert_eq!(g.0, w.0, "box[{i}] 骨骼名");
            for a in 0..3 {
                assert!(
                    (g.1[a] - w.1[a]).abs() <= 1e-2,
                    "box[{i}] bbmin[{a}]：得 {}，官方真值 {}",
                    g.1[a],
                    w.1[a]
                );
                assert!(
                    (g.2[a] - w.2[a]).abs() <= 1e-2,
                    "box[{i}] bbmax[{a}]：得 {}，官方真值 {}",
                    g.2[a],
                    w.2[a]
                );
            }
        }
        std::fs::remove_dir_all(&d).ok();
    }

    /// 自动 hitbox 的 bbox 从**全 0** 起步（`g_bUseBoneInBBox` 默认 true）。
    ///
    /// 判据：几何全在 `z > 0` 时，`bbmin[2]` 仍应为 **0**（而不是几何的最小 z）。
    /// 这正是官方产物里 `bbmin` 常见 `0.00` 的原因。
    #[test]
    fn autohitbox_starts_from_zero() {
        let d = tmpdir("autohb-zero");
        // 三角形全在 z ∈ [5, 9]，x/y 也远离原点。
        let smd = r#"version 1
nodes
  0 "root" -1
end
skeleton
  time 0
    0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
end
triangles
myprop
  0 100.000000 100.000000 5.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 0 1.000000
  0 110.000000 100.000000 5.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 0 1.000000
  0 100.000000 110.000000 9.000000 0.000000 0.000000 1.000000 0.000000 1.000000 1 0 1.000000
end
"#;
        write(&d, "zero.smd", smd);
        let toml = r#"
[model]
name = "models/test/zero.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "zero.smd"
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        assert_eq!(c.desc.hitboxes.boxes.len(), 1, "应生成 1 个 box");
        let h = &c.desc.hitboxes.boxes[0];
        assert_eq!(
            h.bbmin,
            [0.0, 0.0, 0.0],
            "bbox 必须从全 0 起步（g_bUseBoneInBBox = true）"
        );
        assert_eq!(h.bbmax, [110.0, 110.0, 9.0], "max 应是几何的真实上界");
        std::fs::remove_dir_all(&d).ok();
    }

    /// 显式 hitbox 存在时**不得**自动生成，也不得置 `0x1` 标志。
    ///
    /// 实测语料：63 个模型用显式 `$hbox`，它们都**不带** `0x1`。
    #[test]
    fn explicit_hitbox_suppresses_autogeneration() {
        let d = tmpdir("explicit-hb");
        write(&d, "myprop-ref.smd", SMD);
        let toml = format!(
            "{}\n[hitboxes]\nset_name = \"default\"\n\n[[hitboxes.boxes]]\nbone = \"root\"\nbbmin = [-1.0, -2.0, -3.0]\nbbmax = [4.0, 5.0, 6.0]\n",
            desc_toml("myprop-ref.smd")
        );
        let desc = ModelDesc::from_toml(&toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        assert!(!c.desc.hitboxes.autogenerated, "不应标记为自动生成");
        assert_eq!(c.desc.hitboxes.boxes.len(), 1, "应保留显式的 1 个 box");
        assert_eq!(c.desc.hitboxes.boxes[0].bbmin, [-1.0, -2.0, -3.0]);
        assert_eq!(
            c.desc.model.extra_flags.unwrap_or(0) & crate::mdl_writer::FLAG_AUTOGENERATED_HITBOX,
            0,
            "显式 hitbox 不应置 0x1 标志"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    // ---- 姿态包围盒（`CalcSequenceBoundingBoxes`，`simplify.cpp:7049`）----

    /// **核心判据**：姿态包围盒必须与官方 studiomdl 产物一致。
    ///
    /// 用官方受控实验 `ipe1`（2 条序列、**无** `$staticprop`、含真实动画）：
    ///
    /// ```text
    /// hull   = [-20, 0, 0]      .. [0, 10, 12]
    /// seq[0] = [-20, 0, 0]      .. [0, 10, 12]      （hull 取自 seq[0]）
    /// seq[1] = [-20, -19.95, 0] .. [19.8, 10, 7]    （动画摆姿势后）
    /// ```
    ///
    /// `seq[1]` 的 y 到 **-19.95**、x 到 **19.8**，而静止姿势顶点 AABB 只有
    /// `[-20,0,0]..[0,10,7]` —— 这正是「姿态包围盒」与「顶点 AABB」的差别。
    ///
    /// 同时钉住**根骨骼的 `Rz(90°)`**（`panim->rotation` =
    /// `g_defaultrotation`，见 `BuildRawTransforms`，`simplify.cpp:264`）：
    /// 少了它，seq[1] 会算成 `[-19.95,-19.8,0]..[10,20,7]`（x/y 互换）。
    #[test]
    fn pose_bounds_match_official_ipe1() {
        let d = tmpdir("pose-bbox");
        write(&d, "ipe.smd", IPE_SMD);
        write(&d, "ipeanim.smd", IPE_ANIM1);
        write(&d, "ipeanim2.smd", IPE_ANIM2);
        let toml = r#"
[model]
name = "models/test/ipe1.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"
position = [10.0, 0.0, 3.0]

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "ipe.smd"

[[sequences]]
name = "idle"
smd = "ipeanim.smd"
fps = 30.0

[[sequences]]
name = "wave"
smd = "ipeanim2.smd"
fps = 30.0
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");

        let rb = bone_render_bounds(&c.desc, &c);
        let s0 = sequence_pose_bounds(&c.desc, &c, 0, &rb).expect("seq0 应有包围盒");
        let s1 = sequence_pose_bounds(&c.desc, &c, 1, &rb).expect("seq1 应有包围盒");

        // 官方 seq[0]
        let want0 = ([-20.0, 0.0, 0.0], [0.0, 10.0, 12.0]);
        // 官方 seq[1]
        let want1 = ([-20.0, -19.95, 0.0], [19.8, 10.0, 7.0]);

        for (tag, got, want) in [("seq0", s0, want0), ("seq1", s1, want1)] {
            for a in 0..3 {
                assert!(
                    (got.0[a] - want.0[a]).abs() <= 0.02,
                    "{tag} bbmin[{a}]：得 {}，官方 {}",
                    got.0[a],
                    want.0[a]
                );
                assert!(
                    (got.1[a] - want.1[a]).abs() <= 0.02,
                    "{tag} bbmax[{a}]：得 {}，官方 {}",
                    got.1[a],
                    want.1[a]
                );
            }
        }
        std::fs::remove_dir_all(&d).ok();
    }

    /// `$staticprop` **不套**根骨骼的 `Rz(90°)`。
    ///
    /// `MakeStaticProp()` 把 `g_panimation[0]->rotation` 置 0
    /// （`simplify.cpp:3379`），而几何本身已被旋转 —— 两者不能叠加，
    /// 否则会多转 90°。
    #[test]
    fn static_prop_pose_bounds_have_no_root_rotation() {
        let d = tmpdir("pose-sp");
        write(&d, "ipe.smd", IPE_SMD);
        let toml = r#"
[model]
name = "models/test/sp-pose.mdl"
surface_prop = "metal"
static_prop = true

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"
position = [10.0, 0.0, 3.0]

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "ipe.smd"

[[sequences]]
name = "idle"
smd = "ipe.smd"
fps = 30.0
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let rb = bone_render_bounds(&c.desc, &c);
        let b = sequence_pose_bounds(&c.desc, &c, 0, &rb).expect("应有包围盒");

        // 几何经 `Rz(90°)` 后是 x∈[-20,0] y∈[0,10] z∈[0,7]。
        // 若错误地再套一次根旋转，会变成 x∈[-10,0] y∈[-20,0]。
        assert!(
            (b.0[0] - (-20.0)).abs() <= 0.02,
            "静态道具的 x 下界应是 -20（几何已旋转过），实际 {}",
            b.0[0]
        );
        assert!(
            (b.0[1] - 0.0).abs() <= 0.02,
            "静态道具的 y 下界应是 0（**不**再套根旋转），实际 {}",
            b.0[1]
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// `hull` 取自 `seq[0]`（`write.cpp:2071-2087`）。
    #[test]
    fn hull_equals_seq0_pose_bounds() {
        let d = tmpdir("hull-seq0");
        write(&d, "ipe.smd", IPE_SMD);
        write(&d, "ipeanim.smd", IPE_ANIM1);
        let toml = r#"
[model]
name = "models/test/hull.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"
position = [10.0, 0.0, 3.0]

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "ipe.smd"

[[sequences]]
name = "idle"
smd = "ipeanim.smd"
fps = 30.0
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let rb = bone_render_bounds(&c.desc, &c);
        let s0 = sequence_pose_bounds(&c.desc, &c, 0, &rb).expect("应有包围盒");

        // 写出的 hull 必须等于 seq0 的姿态包围盒。
        let out = crate::mdl_writer::write_mdl(&c).unwrap();
        let g = |o: usize| f32::from_le_bytes(out.bytes[o..o + 4].try_into().unwrap());
        for a in 0..3 {
            assert!(
                (g(crate::mdl_writer::off::HULL_MIN + a * 4) - s0.0[a]).abs() <= 1e-3,
                "hull_min[{a}] 应等于 seq0 的 {}",
                s0.0[a]
            );
            assert!(
                (g(crate::mdl_writer::off::HULL_MAX + a * 4) - s0.1[a]).abs() <= 1e-3,
                "hull_max[{a}] 应等于 seq0 的 {}",
                s0.1[a]
            );
        }
        std::fs::remove_dir_all(&d).ok();
    }

    /// `$staticprop` 的包围盒必须用**塌缩后的身份动画**算，而不是原始序列。
    ///
    /// `MakeStaticProp()` 把动画压成 1 条 1 帧的身份动画
    /// （`simplify.cpp:3362-3380`），所以原始序列的逐帧姿态**不再参与**。
    ///
    /// 实测（`ipe2`）：`ipeanim.smd` 第 1 帧把 root 抬到 z=5，
    /// 若误用原始帧会得到 `bbmax=[0,10,12]`，而官方是 **`[0,10,7]`**。
    #[test]
    fn static_prop_pose_bounds_use_identity_frame() {
        let d = tmpdir("pose-sp-identity");
        write(&d, "ipe.smd", IPE_SMD);
        write(&d, "ipeanim.smd", IPE_ANIM1);
        let toml = r#"
[model]
name = "models/test/sp-id.mdl"
surface_prop = "metal"
static_prop = true

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"
position = [10.0, 0.0, 3.0]

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "ipe.smd"

[[sequences]]
name = "idle"
smd = "ipeanim.smd"
fps = 30.0
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let rb = bone_render_bounds(&c.desc, &c);
        let b = sequence_pose_bounds(&c.desc, &c, 0, &rb).expect("应有包围盒");

        // 官方 `ipe2`：hull = [0,0,0] .. [0,10,7]
        // （z 上界 7 而不是 12 —— 身份帧里 root 在 z=0）。
        assert!(
            (b.1[2] - 7.0).abs() <= 0.02,
            "bbmax[2] 应是 7（身份帧），实际 {} —— 若为 12 说明误用了原始帧",
            b.1[2]
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// **blend 序列的包围盒是「每一格的并集」**（`simplify.cpp:7184-7196`）。
    ///
    /// 官方分两步：
    ///
    /// ```c
    /// // ① 逐动画算自己的 bmin/bmax
    /// for (i = 0; i < g_numani; i++) { ... g_panimation[i]->bmin = bmin; }
    ///
    /// // ② 逐**序列**把所有格的盒子求并
    /// for (i = 0; i < g_sequence.Count(); i++)
    ///   for (j = 0; j < g_sequence[i].groupsize[0]; j++)
    ///     for (k = 0; k < g_sequence[i].groupsize[1]; k++) {
    ///       s_animation_t *panim = g_sequence[i].panim[j][k];
    ///       if (panim->bmin[0] < bmin[0]) bmin[0] = panim->bmin[0];
    ///       ...
    ///     }
    /// ```
    ///
    /// 早先本实现只用了**第一格**的帧 —— 实测 miku `look_poses`
    /// （3 格）的包围盒因此只有第一格那么大。
    ///
    /// 判据：造两条动画，一条让 `tip` 在 `+x` 伸展、另一条在 `+y`，
    /// 则序列的包围盒必须**同时**覆盖两者 —— 只看第一格会漏掉第二格。
    #[test]
    fn blend_sequence_bounds_union_all_cells() {
        let d = tmpdir("blend-bounds");
        write(&d, "ipe.smd", IPE_SMD);
        write(&d, "x.smd", IPE_ANIM_X);
        write(&d, "y.smd", IPE_ANIM_Y);
        let toml = r#"
[model]
name = "models/test/blendbounds.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"

[[model.pose_parameters]]
name = "p"
start = 0.0
end = 1.0

[[animations]]
name = "a_x"
smd = "x.smd"

[[animations]]
name = "a_y"
smd = "y.smd"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "ipe.smd"

[[sequences]]
name = "poses"
smd = "x.smd"
blend_width = 2
blends = ["a_x", "a_y"]

[[sequences.blend_params]]
parameter = "p"
start = 0.0
end = 1.0
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let rb = bone_render_bounds(&c.desc, &c);
        let b = sequence_pose_bounds(&c.desc, &c, 0, &rb).expect("应有包围盒");

        // `a_x` 把 tip 抬到 x=+30，`a_y` 抬到 y=+30（模型空间是 `(x,y,z)`，
        // 根骨骼的 Rz90 会把两者换轴 —— 所以只断言「两个方向都被覆盖」，
        // 不锁死具体轴）。
        let span = [b.1[0] - b.0[0], b.1[1] - b.0[1], b.1[2] - b.0[2]];
        let covered = span.iter().filter(|v| **v >= 25.0).count();
        assert!(
            covered >= 2,
            "两条动画分别在不同方向伸展，包围盒应覆盖**两个**方向；实际 span={span:?} \
             —— 只有 1 个方向说明只用了第一格"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// **`subtract` 动画的包围盒用「减除之前」的姿态算**（官方行为）。
    ///
    /// 官方 `CalcSequenceBoundingBoxes`（`simplify.cpp:7087`）用
    /// `AngleMatrix(sanim.rot, sanim.pos)` 直接建局部矩阵，**不**走
    /// `CalcBoneTransforms` 的 DELTA 合成分支（`simplify.cpp:4558-4577`）。
    /// 对 `subtract` 动画 `sanim` 是**增量**，于是官方那边
    /// `bonetransform` 近单位阵 ⟹ `posetransform = inverse(boneToPose)`
    /// ⟹ 顶点被映射到**参考姿态**附近 ⟹ 得到一个**全身大小**的盒子。
    ///
    /// **决定性判据**（miku，`probe_nosub_bbox.js`）：把 `subtract` 去掉
    /// 再编译，`look_poses` 从 **44276588 ULP → 51 ULP**。
    ///
    /// 本测试造一个「减除后几乎不动、减除前动很多」的样本：
    /// 若误用减除后的帧，包围盒会缩成一点；用减除前的帧才是大的。
    #[test]
    fn subtract_animation_bounds_use_pre_subtract_frames() {
        let d = tmpdir("subtract-bounds");
        write(&d, "ipe.smd", IPE_SMD);
        // 参考动画与目标动画**同一姿态**（tip 在 `[30, 0, 3]`）：
        // 减除后增量恒为 **0**，而减除前是 `[30, 0, 3]`。
        // 这正是 miku `look_*` 的处境 —— 官方包围盒用后者。
        write(&d, "base.smd", IPE_ANIM_X);
        write(&d, "far.smd", IPE_ANIM_X);
        let toml = r#"
[model]
name = "models/test/subbbox.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"

[[animations]]
name = "a_base"
smd = "base.smd"

[[animations]]
name = "a_same"
smd = "far.smd"
subtract = "a_base"
subtract_frame = 0

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "ipe.smd"

[[sequences]]
name = "idle"
smd = "a_same"
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        // 减除前后帧都留了一份。
        assert!(
            c.animations[1].pre_subtract_frames.is_some(),
            "`subtract` 动画必须留一份减除前的帧（包围盒要用）"
        );
        // 减除后增量应为 0（目标与参考同帧）。
        let after = &c.animations[1].frames[0][1].position;
        assert!(
            after.iter().all(|v| v.abs() < 1e-6),
            "减除后增量应为 0，实际 {after:?}"
        );
        // 而减除前不是 0。
        let before = &c.animations[1].pre_subtract_frames.as_ref().unwrap()[0][1].position;
        assert!(
            before.iter().any(|v| v.abs() > 1.0),
            "减除前的姿态不应全 0，实际 {before:?}"
        );

        // ⚠️ **真正的判据**：序列的包围盒必须用**减除前**的姿态算。
        //
        // 用减除后的帧（全 0 增量）会让 `posetransform` 恒为
        // `inverse(boneToPose)`，盒子会**明显偏小**；
        // 用减除前的帧才会覆盖 `tip` 所在的 `x=30`。
        let rb = bone_render_bounds(&c.desc, &c);
        let b = sequence_pose_bounds(&c.desc, &c, 0, &rb).expect("应有包围盒");
        let span_x = b.1[0] - b.0[0];
        let span_y = b.1[1] - b.0[1];
        assert!(
            span_x.max(span_y) >= 25.0,
            "包围盒应覆盖 `tip` 的 30 单位伸展（说明用了**减除前**的姿态）；\
             实际 span=[{span_x}, {span_y}] —— 偏小说明误用了减除后的帧"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// `ipe` 的网格 SMD（官方 `docs/_probe/smdl/ipe.smd`）。
    const IPE_SMD: &str = r#"version 1
nodes
0 "root" -1
1 "tip" 0
end
skeleton
time 0
0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
1 10.000000 0.000000 3.000000 0.000000 0.000000 0.000000
end
triangles
myprop
0 0 0 0 1 0 0 0 0 1 0 1.000000
0 10 0 3 1 0 0 1 0 1 0 1.000000
1 0 20 7 1 0 0 0 1 1 0 1.000000
end
"#;

    // ---- `STUDIO_DELTA` 重建（`CalcBoneTransforms` 的 `else` 分支）----
    //
    // `subtract` 把动画变成**增量**，官方在 `CalcBoneTransforms`
    // （`simplify.cpp:4562-4578`）里用**基准动画第 0 帧**把它重建回完整姿态。
    // 不重建 ⟹ 增量近零 ⟹ 所有骨骼塌到原点 ⟹ IK 误差恒 0。
    //
    // 实测判据见 `docs/_probe/ab_iksub_mdlc.js` / `cmp_ikpayload_bytes.js`：
    // 官方与 mdlc 的 `@idle` 压缩载荷从「6 通道全不同」变成
    // **逐字节完全相同的 72 字节**。

    /// **核心判据**：`subtract` 造出的增量经重建后必须**还原出原始姿态**。
    ///
    /// 这是「有无 `subtract` 官方产物不变」的数学根据。两条恒等式：
    ///
    /// ```text
    /// p3 = base.pos + 1 · delta.pos = base.pos + (raw.pos − base.pos) = raw.pos
    /// q3 = base.q · (1 · conj(base.q)·raw.q) = raw.q
    /// ```
    ///
    /// 所以判据是：`delta_frame_worlds(base, subtract(raw, base))` 必须等于
    /// `frame_worlds(raw)`。
    ///
    /// 若 `delta_local_matrices` 被删掉（退回「直接用增量」），
    /// 左侧会给出增量本身 ⟹ 骨骼塌到原点 ⟹ 本测试变红。
    #[test]
    fn delta_reconstruction_undoes_subtract() {
        use crate::smd::SmdPose;

        let base = vec![
            SmdPose {
                bone: 0,
                position: [0.0; 3],
                rotation: [0.0; 3],
            },
            SmdPose {
                bone: 1,
                position: [30.0, 0.0, 3.0],
                rotation: [0.0, 0.0, 20.0f32.to_radians()],
            },
        ];
        // 原始（减除前）姿态：相对基准再转 15°、沿 x 再走 7。
        let raw = vec![
            SmdPose {
                bone: 0,
                position: [0.0; 3],
                rotation: [0.0; 3],
            },
            SmdPose {
                bone: 1,
                position: [37.0, 0.0, 3.0],
                rotation: [0.0, 0.0, 35.0f32.to_radians()],
            },
        ];
        // 用**生产代码**造增量（`subtract_base_frames`），避免测试自己
        // 手写一份可能与实现分叉的减法。
        let mut delta = [raw.clone()];
        // `std::slice::from_ref` 而不是 `&[base.clone()]` —— `base` 下面还要用。
        subtract_base_frames(&mut delta, std::slice::from_ref(&base), 0);
        let delta = &delta[0];

        // 增量确实是「小」的 —— 塌陷 bug 下误差会趋近 0。
        assert!(
            delta[1].position[0].abs() < 10.0,
            "增量应在 7 附近，实际 {:?}",
            delta[1].position
        );

        // 单骨骼的局部矩阵判据（不依赖父链，最直接）。
        let locals = delta_local_matrices(&base, delta);
        let p = [locals[1][3], locals[1][7], locals[1][11]];
        assert!(
            (p[0] - 37.0).abs() < 1e-4 && p[1].abs() < 1e-4 && (p[2] - 3.0).abs() < 1e-4,
            "重建后 tip 应还原到 [37, 0, 3]；实际 {p:?} —— \
             全 0 说明增量被当成完整姿态直接用，漏了基准帧"
        );
        let (s, c) = 35.0f32.to_radians().sin_cos();
        assert!(
            (locals[1][0] - c).abs() < 1e-4 && (locals[1][1] + s).abs() < 1e-4,
            "重建后绕 Z 应还原为 35°，实际 (0,0)={} (0,1)={}",
            locals[1][0],
            locals[1][1]
        );
    }

    /// `s == 1` 时重建**不等于**直接返回源姿态 —— `QuaternionMA` 内部的
    /// `QuaternionScale` / `QuaternionNormalize` 会引入浮点差。
    ///
    /// 这个差很小（约 `1e-7`），但足以在压缩误差的**量化边界**上翻转一个采样值，
    /// 所以实现里必须走完整条路径、不能加「`s == 1` 就直接拷贝」的短路。
    #[test]
    fn delta_reconstruction_keeps_float_noise_of_quaternion_ma() {
        use crate::smd::SmdPose;
        let base = vec![SmdPose {
            bone: 0,
            position: [0.0; 3],
            rotation: [0.0; 3],
        }];
        // 增量姿态刻意取一个**非平凡**旋转，让 `QuaternionScale` 的误差可见。
        let delta = vec![SmdPose {
            bone: 0,
            position: [0.0; 3],
            rotation: [0.3, 0.7, -0.4],
        }];
        let got = delta_local_matrices(&base, &delta);
        let want = crate::bone_math::local_transform([0.0; 3], [0.3, 0.7, -0.4]);
        let max = (0..12)
            .map(|i| (got[0][i] - want[i]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max < 1e-5,
            "重建应非常接近源姿态（差 {max}）；差得离谱说明基准帧用错了"
        );
    }

    /// **核心判据**：增量为**零**时重建必须还原出基准姿态本身。
    ///
    /// 由 `p3 = base.pos + s · delta.pos`、`q3 = base.q · (s · delta.q)`，
    /// `delta = 0`（零平移 + 单位四元数）时 `p3 = base.pos`、`q3 = base.q`。
    ///
    /// ⚠️ **不能**写成 `delta_frame_worlds(base, base) == frame_worlds(base)` ——
    /// 那是错的：`p3 = base.pos + base.pos = 2·base.pos`。
    /// 这条式子恰好是「基准帧语义」的判据：若实现误把 `delta` 当完整姿态
    /// （即漏了 `+ base.pos`），零增量会给出 `[0,0,0]` 而不是 `base.pos`。
    #[test]
    fn delta_frame_worlds_with_zero_delta_reproduces_base() {
        use crate::smd::SmdPose;
        let d = tmpdir("delta-worlds");
        write(&d, "ipe.smd", IPE_SMD);
        write(&d, "base.smd", IPE_ANIM_X);
        let toml = r#"
[model]
name = "models/test/dw.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"

[[animations]]
name = "a_base"
smd = "base.smd"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "ipe.smd"

[[sequences]]
name = "idle"
smd = "a_base"
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        let parents = bone_parents(&c.desc);
        let base = &c.animations[0].frames[0];

        // 零增量：平移全 0、旋转全 0（单位四元数）。
        let zero: Vec<SmdPose> = base
            .iter()
            .map(|p| SmdPose {
                bone: p.bone,
                position: [0.0; 3],
                rotation: [0.0; 3],
            })
            .collect();

        let plain = frame_worlds(&c.desc, &parents, base);
        let delta = delta_frame_worlds(&c.desc, &parents, base, &zero);
        for k in 0..plain.len() {
            let max = (0..12)
                .map(|i| (plain[k][i] - delta[k][i]).abs())
                .fold(0.0f32, f32::max);
            assert!(
                max < 1e-5,
                "零增量时重建应还原基准姿态，骨骼 {k} 差 {max} —— \
                 差一个 base.pos 说明漏了「加上基准帧」那一步"
            );
        }
        std::fs::remove_dir_all(&d).ok();
    }

    /// 单帧动画：把 `tip` 沿 **+x** 推远。
    const IPE_ANIM_X: &str = r#"version 1
nodes
0 "root" -1
1 "tip" 0
end
skeleton
time 0
0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
1 30.000000 0.000000 3.000000 0.000000 0.000000 0.000000
end
triangles
end
"#;

    /// 单帧动画：把 `tip` 沿 **+y** 推远（与 [`IPE_ANIM_X`] 不同方向）。
    const IPE_ANIM_Y: &str = r#"version 1
nodes
0 "root" -1
1 "tip" 0
end
skeleton
time 0
0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
1 0.000000 30.000000 3.000000 0.000000 0.000000 0.000000
end
triangles
end
"#;

    /// `ipeanim.smd`：2 帧，root 的 z 从 0 到 5。
    const IPE_ANIM1: &str = r#"version 1
nodes
0 "root" -1
1 "tip" 0
end
skeleton
time 0
0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
1 10.000000 0.000000 3.000000 0.000000 0.000000 0.000000
time 1
0 0.000000 0.000000 5.000000 0.000000 0.000000 0.000000
1 10.000000 0.000000 3.000000 0.000000 0.000000 0.000000
end
triangles
myprop
0 0 0 0 1 0 0 0 0 1 0 1.000000
0 10 0 3 1 0 0 1 0 1 0 1.000000
1 0 20 7 1 0 0 0 1 1 0 1.000000
end
"#;

    /// `ipeanim2.smd`：3 帧，root 绕 z 转到 3.0 弧度。
    const IPE_ANIM2: &str = r#"version 1
nodes
0 "root" -1
1 "tip" 0
end
skeleton
time 0
0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
1 10.000000 0.000000 3.000000 0.000000 0.000000 0.000000
time 1
0 0.000000 0.000000 0.000000 0.000000 0.000000 1.500000
1 10.000000 0.000000 3.000000 0.000000 0.000000 0.000000
time 2
0 0.000000 0.000000 0.000000 0.000000 0.000000 3.000000
1 10.000000 0.000000 3.000000 0.000000 0.000000 0.000000
end
triangles
myprop
0 0 0 0 1 0 0 0 0 1 0 1.000000
0 10 0 3 1 0 0 1 0 1 0 1.000000
1 0 20 7 1 0 0 0 1 1 0 1.000000
end
"#;

    #[test]
    fn compiles_smd_into_mesh() {
        let d = tmpdir("basic");
        write(&d, "myprop-ref.smd", SMD);
        let desc = ModelDesc::from_toml(&desc_toml("myprop-ref.smd")).unwrap();
        let c = compile(&desc, &d).expect("应能编译");
        assert_eq!(c.bodyparts.len(), 1);
        assert_eq!(c.bodyparts[0].models.len(), 1);
        let m = &c.bodyparts[0].models[0];
        assert_eq!(m.name, "myprop-ref.smd", "名字应自动取 SMD 文件名");
        assert_eq!(m.meshes.len(), 1);
        assert_eq!(m.meshes[0].material, 0);
        assert_eq!(m.meshes[0].vertices.len(), 3);
        assert_eq!(m.meshes[0].triangles.len(), 1);
        assert_eq!(c.total_vertices(), 3);
        assert_eq!(c.total_triangles(), 1);
        // SMD 的骨骼下标 1 是 "tip"，应映射到描述里的下标 1。
        assert_eq!(m.meshes[0].vertices[0].bones, vec![[1.0, 1.0]]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn deduplicates_shared_vertices_across_triangles() {
        let d = tmpdir("dedup");
        // 两个三角形共享一条边（4 个不同顶点，6 个顶点行）。
        let smd = SMD.replace(
            "  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000\nend",
            "  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000\nmyprop\n  1 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000\n  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000\n  1 8.000000 8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 1.000000 1 1 1.000000\nend",
        );
        write(&d, "myprop-ref.smd", &smd);
        let desc = ModelDesc::from_toml(&desc_toml("myprop-ref.smd")).unwrap();
        let c = compile(&desc, &d).unwrap();
        let m = &c.bodyparts[0].models[0].meshes[0];
        assert_eq!(m.triangles.len(), 2);
        // 6 个顶点行 → 去重成 4 个顶点。
        assert_eq!(m.vertices.len(), 4, "共享顶点必须去重");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn splits_meshes_by_material_name() {
        let d = tmpdir("mats");
        // 注意：`str::replace` 会替换**所有**匹配，而 SMD 里有三个 `end`
        // （nodes / skeleton / triangles 各一个）。所以这里显式拼出完整文件，
        // 而不是用 replace 去插一段。
        let smd = r#"version 1
nodes
  0 "root" -1
  1 "tip" 0
end
skeleton
  time 0
    0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
    1 0.000000 0.000000 8.000000 0.000000 0.000000 0.000000
end
triangles
tex_a
  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000
  1 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000
  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000
tex_b
  1 1.000000 0.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000
  1 2.000000 0.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000
  1 1.000000 1.000000 0.000000 0.000000 0.000000 1.000000 0.000000 1.000000 1 1 1.000000
end
"#;
        write(&d, "myprop-ref.smd", smd);
        // 描述里要同时声明两个材质。
        let toml = desc_toml("myprop-ref.smd").replace(
            r#"textures = [{ name = "models/test/myprop" }]"#,
            "textures = [\n  { name = \"models/test/tex_a\" },\n  { name = \"models/test/tex_b\" },\n]",
        );
        let desc = ModelDesc::from_toml(&toml).unwrap();
        let c = compile(&desc, &d).unwrap();
        let m = &c.bodyparts[0].models[0];
        assert_eq!(m.meshes.len(), 2, "两个材质名应产生两个 mesh");
        assert_eq!(m.meshes[0].material, 0);
        assert_eq!(m.meshes[1].material, 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reports_unknown_material_with_smd_path() {
        let d = tmpdir("badmat");
        let smd = SMD.replace("myprop\n", "not_declared\n");
        write(&d, "myprop-ref.smd", &smd);
        let desc = ModelDesc::from_toml(&desc_toml("myprop-ref.smd")).unwrap();
        let errs = compile(&desc, &d).unwrap_err();
        assert!(
            errs.iter().any(|x| x.message.contains("找不到")),
            "应报材质未声明：{errs:?}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reports_missing_smd_file() {
        let d = tmpdir("missing");
        let desc = ModelDesc::from_toml(&desc_toml("nope.smd")).unwrap();
        let errs = compile(&desc, &d).unwrap_err();
        assert!(errs.iter().any(|x| x.message.contains("读不到")), "{errs:?}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reports_bone_not_declared_in_desc() {
        let d = tmpdir("badbone");
        let smd = SMD.replace("1 \"tip\" 0", "1 \"ghost\" 0");
        write(&d, "myprop-ref.smd", &smd);
        let desc = ModelDesc::from_toml(&desc_toml("myprop-ref.smd")).unwrap();
        let errs = compile(&desc, &d).unwrap_err();
        assert!(
            errs.iter().any(|x| x.message.contains("不在 [[bones]]")),
            "{errs:?}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn bone_pose_falls_back_to_smd_then_desc_overrides() {
        let d = tmpdir("pose");
        write(&d, "myprop-ref.smd", SMD);
        let desc = ModelDesc::from_toml(&desc_toml("myprop-ref.smd")).unwrap();
        let c = compile(&desc, &d).unwrap();
        // 描述没写 position → 取 SMD 的 (0,0,8)。
        let (pos, rot) = resolve_bone_pose(&desc, &c, 1);
        assert_eq!(pos, [0.0, 0.0, 8.0]);
        assert_eq!(rot, [0.0, 0.0, 0.0]);

        // 描述显式写 position/rotation（角度）→ 覆盖，且旋转转成弧度。
        let mut d2 = desc.clone();
        d2.bones[1].position = Some([1.0, 2.0, 3.0]);
        d2.bones[1].rotation = Some([90.0, 0.0, 0.0]);
        let c2 = compile(&d2, &d).unwrap();
        let (pos2, rot2) = resolve_bone_pose(&d2, &c2, 1);
        assert_eq!(pos2, [1.0, 2.0, 3.0]);
        assert!((rot2[0] - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "{rot2:?}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn normalizes_weights_that_do_not_sum_to_one() {
        let d = tmpdir("weights");
        // 两个绑定，权重和 0.9（SMD 导出器常见）。
        let smd = SMD.replace("1 1 1.000000\n  1 8.000000", "2 1 0.600000 0 0.300000\n  1 8.000000");
        write(&d, "myprop-ref.smd", &smd);
        let desc = ModelDesc::from_toml(&desc_toml("myprop-ref.smd")).unwrap();
        let c = compile(&desc, &d).unwrap();
        let b = &c.bodyparts[0].models[0].meshes[0].vertices[0].bones;
        let sum: f32 = b.iter().map(|x| x[1]).sum();
        assert!((sum - 1.0).abs() < 1e-5, "权重应被归一化，实际 {b:?}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn drops_degenerate_triangles_after_dedup() {
        let d = tmpdir("degen");
        // 三个顶点行里有两行完全相同 → 去重后退化，应被丢弃。
        let smd = SMD.replace(
            "  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000",
            "  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000",
        );
        write(&d, "myprop-ref.smd", &smd);
        let desc = ModelDesc::from_toml(&desc_toml("myprop-ref.smd")).unwrap();
        let errs = compile(&desc, &d).unwrap_err();
        // 唯一的三角形被丢弃 → 必须报错而不是产出空模型。
        assert!(
            errs.iter().any(|x| x.message.contains("没有可用的三角形")),
            "{errs:?}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    // ================= $weightlist =================
    //
    // 判据来自 `docs/_probe/cmp_weightlist_oracle.js`（11/11 与官方逐位一致）
    // 与 `docs/_probe/cmp_weightlist_errors.js`（11/11 判定一致）。
    // 这里的表就是那两张表的 Rust 版 —— 用的是**官方产物实测**的值。

    /// 骨骼链 `root → mid → leaf → tip` 的权重表描述。
    fn wl_desc(lists: &str) -> ModelDesc {
        let toml = format!(
            r#"
{lists}
[[bones]]
name = "root"

[[bones]]
name = "mid"
parent = "root"

[[bones]]
name = "leaf"
parent = "mid"

[[bones]]
name = "tip"
parent = "leaf"

[model]
name = "models/test/wl.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{{ name = "models/test/mat" }}]

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "wl.smd"
"#
        );
        ModelDesc::from_toml(&toml).expect("描述必须能解析")
    }

    #[test]
    fn weightlist_semantics_match_official() {
        // 官方实测（`cmp_weightlist_oracle.js`，逐位比较）：
        //   none             -> [1, 1, 1, 1]
        //   root 1           -> [1, 1, 1, 1]
        //   root 0           -> [0, 0, 0, 0]
        //   mid 0.5          -> [0, 0.5, 0.5, 0.5]   ← 根是 0，子沿父链继承
        //   mid 0            -> [0, 0, 0, 0]
        //   leaf 0           -> [0, 0, 0, 0]
        //   tip 0.25         -> [0, 0, 0, 0.25]
        //   mid 0.5 + tip 0  -> [0, 0.5, 0.5, 0]     ← 显式条目覆盖继承
        //   all four         -> [1, 0.75, 0.5, 0.25]
        let cases: &[(&str, &str, [f32; 4])] = &[
            (
                "root1",
                r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "root"
weight = 1.0
"#,
                [1.0, 1.0, 1.0, 1.0],
            ),
            (
                "root0",
                r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "root"
weight = 0.0
"#,
                [0.0, 0.0, 0.0, 0.0],
            ),
            (
                // 最能说明问题的一行：根是 **0**（不是 1），
                // 而 leaf/tip 沿父链继承 mid 的 0.5。
                "mid_half",
                r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "mid"
weight = 0.5
"#,
                [0.0, 0.5, 0.5, 0.5],
            ),
            (
                "mid_zero",
                r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "mid"
weight = 0.0
"#,
                [0.0, 0.0, 0.0, 0.0],
            ),
            (
                "tip_quarter",
                r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "tip"
weight = 0.25
"#,
                [0.0, 0.0, 0.0, 0.25],
            ),
            (
                // 显式 `tip 0` **覆盖**从 mid 继承来的 0.5。
                "block_form",
                r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "mid"
weight = 0.5

[[weight_lists.bones]]
bone = "tip"
weight = 0.0
"#,
                [0.0, 0.5, 0.5, 0.0],
            ),
            (
                "all_four",
                r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "root"
weight = 1.0

[[weight_lists.bones]]
bone = "mid"
weight = 0.75

[[weight_lists.bones]]
bone = "leaf"
weight = 0.5

[[weight_lists.bones]]
bone = "tip"
weight = 0.25
"#,
                [1.0, 0.75, 0.5, 0.25],
            ),
        ];

        for (tag, lists, want) in cases {
            let d = wl_desc(lists);
            let r = resolve_weight_lists(&d);
            assert_eq!(r.len(), 1, "{tag}: 应当解析出 1 张表");
            assert_eq!(r[0], want, "{tag}: 与官方实测不一致");
        }
    }

    #[test]
    fn weightlist_absent_means_all_ones() {
        // 官方实测 `none` → [1,1,1,1]（隐式表 0：根 1、子骨骼沿父链继承 1）。
        let d = wl_desc("");
        assert!(resolve_weight_lists(&d).is_empty());
        assert_eq!(default_weight_list(4), vec![1.0; 4]);
    }

    #[test]
    fn weightlist_bone_name_is_case_insensitive() {
        // 官方 `findGlobalBone` 用 `stricmp`（`simplify.cpp:2628`），
        // 所以 `MID` 必须命中 `mid` —— 用大小写敏感的表会静默失效。
        let d = wl_desc(
            r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "MID"
weight = 0.5
"#,
        );
        assert_eq!(resolve_weight_lists(&d)[0], [0.0, 0.5, 0.5, 0.5]);
    }

    #[test]
    fn weightlist_index_starts_at_one() {
        // 官方查找循环 `for (i = 1; i < g_numweightlist; i++)`
        // （`studiomdl.cpp:1719`）—— **跳过 0**（0 是隐式默认表）。
        let d = wl_desc(
            r#"[[weight_lists]]
name = "first"

[[weight_lists.bones]]
bone = "mid"
weight = 0.5

[[weight_lists]]
name = "second"

[[weight_lists.bones]]
bone = "tip"
weight = 0.25
"#,
        );
        assert_eq!(weight_list_index(&d, "first"), Some(1));
        assert_eq!(weight_list_index(&d, "second"), Some(2));
        // 表名也大小写不敏感（官方 `stricmp`）。
        assert_eq!(weight_list_index(&d, "FIRST"), Some(1));
        assert_eq!(weight_list_index(&d, "nope"), None);
    }

    #[test]
    fn merge_weights_takes_per_bone_max_across_cells() {
        // 官方 `simplify.cpp:302-318` 是 **MAX 而不是「取第一格」**。
        let a = vec![1.0, 0.0, 0.5, 0.0];
        let b = vec![0.0, 1.0, 0.25, 0.0];
        let m = merge_weights(&[0, 1], &[a.clone(), b.clone()], 4);
        assert_eq!(m, vec![1.0, 1.0, 0.5, 0.0]);
        // 单格 = 它自己。
        assert_eq!(merge_weights(&[1], &[vec![0.0; 4], b.clone()], 4), b);
        // 没有任何格 ⟹ 全 1（与「无 `$weightlist`」一致）。
        assert_eq!(merge_weights(&[], &[], 4), vec![1.0; 4]);
    }

    #[test]
    fn declared_but_unused_weightlist_leaves_weights_all_ones() {
        // 官方实测 `declared_unused` → [1,1,1,1]：表被声明但序列没引用它，
        // 于是序列用的是隐式表 0。**表仍然会被 resolve**（官方在
        // `buildAnimationWeights` 里遍历全部表，所以未知骨骼照样报错）。
        let d = wl_desc(
            r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "mid"
weight = 0.5
"#,
        );
        // 表本身被解析出来了……
        assert_eq!(resolve_weight_lists(&d)[0], [0.0, 0.5, 0.5, 0.5]);
        // ……但序列没有 `weight_list` ⟹ 用隐式表 0。
        assert!(d.sequences.is_empty());
        assert_eq!(default_weight_list(d.bones.len()), vec![1.0; 4]);
    }

    // ---- 校验：官方是硬错误，不是 warning ----

    #[test]
    fn validate_rejects_unknown_bone_in_weightlist() {
        // 官方 `unknown bone reference '%s' in weightlist '%s'`
        // （`simplify.cpp:1697`）—— 即使该表**没被引用**也报错。
        let d = wl_desc(
            r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "nosuchbone"
weight = 0.5
"#,
        );
        let errs = d.validate().unwrap_err();
        assert!(
            errs.iter().any(|e| e.path.contains("weight_lists[0].bones[0].bone")
                && e.message.contains("nosuchbone")),
            "{errs:?}"
        );
    }

    #[test]
    fn validate_rejects_unknown_weightlist_reference() {
        // 官方 `unknown weightlist '%s'`（`studiomdl.cpp:1728`）。
        let mut d = wl_desc("");
        d.sequences.push(crate::model::Sequence {
            name: "idle".into(),
            smd: "wl.smd".into(),
            fps: Some(30.0),
            looping: false,
            delta: false,
            activity: None,
            activity_weight: 0,
            events: Vec::new(),
            fade_in: 0.2,
            fade_out: 0.2,
            forward_declared: false,
            no_auto_ik: false,
            ik_rules: Vec::new(),
            iklocks: Vec::new(),
            blends: Vec::new(),
            blend_width: None,
            blend_params: Vec::new(),
            auto_layers: Vec::new(),
            movements: Vec::new(),
            section_frames: None,
            section_threshold: None,
            extra_flags: None,
            weight_list: Some("NOPE".into()),
        });
        let errs = d.validate().unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.path == "sequences[0].weight_list" && e.message.contains("NOPE")),
            "{errs:?}"
        );
    }

    #[test]
    fn validate_rejects_duplicate_weightlist_name() {
        // 官方 `Duplicate weightlist '%s'`（`studiomdl.cpp:3344`），大小写不敏感。
        let d = wl_desc(
            r#"[[weight_lists]]
name = "WL"

[[weight_lists.bones]]
bone = "mid"
weight = 0.5

[[weight_lists]]
name = "wl"

[[weight_lists.bones]]
bone = "tip"
weight = 0.25
"#,
        );
        let errs = d.validate().unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("权重表名重复")),
            "{errs:?}"
        );
    }

    #[test]
    fn validate_weightlist_limits_match_real_binary() {
        // ⚠️ 官方 L4D2 的实测上限是 **128 条/表、128 张表**，
        // 不是 episode1 头文件写的 16/32（见 `MAX_WEIGHT_ENTRIES` 的说明）。
        // 这里把边界钉死：128 条合法、129 条非法。
        let ok = wl_desc(&format!(
            "[[weight_lists]]\nname = \"WL\"\n\n{}",
            (0..128)
                .map(|_| "[[weight_lists.bones]]\nbone = \"mid\"\nweight = 0.5\n")
                .collect::<String>()
        ));
        assert!(
            ok.validate().is_ok(),
            "128 条应当合法：{:?}",
            ok.validate().unwrap_err()
        );

        let bad = wl_desc(&format!(
            "[[weight_lists]]\nname = \"WL\"\n\n{}",
            (0..129)
                .map(|_| "[[weight_lists.bones]]\nbone = \"mid\"\nweight = 0.5\n")
                .collect::<String>()
        ));
        let errs = bad.validate().unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("条目过多")),
            "{errs:?}"
        );
    }

    // ================= $declaresequence =================
    //
    // 判据来自 `docs/_probe/cmp_declaresequence_oracle.js`
    // （6/6 逐字段一致）与 `cmp_survivor_declaresequence.js`
    // （真实 41 KB survivor QCI，936 条序列含 933 条空壳全部一致）。

    /// 造一个带 `$declaresequence` 空壳的描述（TOML 侧）。
    ///
    /// `forward_declared = true` 是**顶层字段**，直接写在 `[[sequences]]` 里。
    ///
    /// ⚠️ 材质名必须与 `SMD` 夹具里的一致（`myprop`）—— 用别的名字
    /// `compile()` 会报「SMD 里的材质名找不到」。
    fn dsq_desc(extra: &str) -> String {
        format!(
            r#"
[model]
name = "models/test/dsq.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{{ name = "models/test/myprop" }}]

[[bones]]
name = "root"

[[bones]]
name = "tip"
parent = "root"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "dsq.smd"

{extra}
"#
        )
    }

    #[test]
    fn forward_declared_sequence_has_no_animation_data() {
        // 官方 `Cmd_DeclareSequence` 只 `memset` + 置 `STUDIO_OVERRIDE`：
        // **不分配 `panim`、不读 SMD**。所以编译出来的
        // `CompiledSequence` 必须是「空」的，且各字段停在 `memset` 值。
        let d = tmpdir("dsq");
        write(&d, "dsq.smd", SMD);
        let desc = ModelDesc::from_toml(&dsq_desc(
            r#"[[sequences]]
name = "shell"
forward_declared = true
"#,
        ))
        .unwrap();
        assert!(desc.validate().is_ok(), "{:?}", desc.validate().unwrap_err());

        let c = compile(&desc, &d).unwrap();
        assert_eq!(c.sequences.len(), 1);
        let s = &c.sequences[0];
        assert!(s.forward_declared);
        // 官方 `memset` 之后这些字段**保持 0**，不是普通序列的默认值。
        assert_eq!(s.activity, 0, "空壳的 activity 是 memset 的 0，不是 -1");
        assert_eq!(s.fade_in, 0.0, "空壳的 fadeintime 是 0，不是 0.2");
        assert_eq!(s.fade_out, 0.0, "空壳的 fadeouttime 是 0，不是 0.2");
        assert!(s.cells.is_empty(), "空壳没有 blend 格");
        assert!(s.frames.is_empty(), "空壳没有帧");
        // ⚠️ **全 0 不是全 1** —— `groupsize=[0,0]` 让官方的 MAX 循环不跑。
        assert_eq!(
            s.weights,
            vec![0.0; desc.bones.len()],
            "空壳的 weightlist 是全 0（simplify.cpp:302-318 的初值）"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn forward_declared_sequence_is_not_required_to_have_smd() {
        // 官方连 `panim` 都不分配 ⟹ 空壳**没有 SMD**。
        // `validate()` 对普通序列要求 `smd` 非空，对空壳必须放行。
        let d = tmpdir("dsq2");
        write(&d, "dsq.smd", SMD);
        let desc = ModelDesc::from_toml(&dsq_desc(
            r#"[[sequences]]
name = "shell"
forward_declared = true
"#,
        ))
        .unwrap();
        assert!(
            desc.validate().is_ok(),
            "空壳不该因为没有 smd 而报错：{:?}",
            desc.validate().unwrap_err()
        );
        // 反例：普通序列没有 smd 仍然要报错。
        let bad = ModelDesc::from_toml(&dsq_desc(
            r#"[[sequences]]
name = "normal"
"#,
        ))
        .unwrap();
        let errs = bad.validate().unwrap_err();
        assert!(
            errs.iter().any(|e| e.path == "sequences[0].smd"),
            "{errs:?}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn forward_declared_sequence_may_share_a_name_with_a_real_one() {
        // 官方 `Cmd_Sequence` 走 `LookupAnimation`（查**动画池**），
        // 空壳不在池里 ⟹ 「先 `$sequence x` 再 `$declaresequence x`」
        // 官方**不报错**，产出两条同名序列（实测 `fill_then_declare` → 2 条）。
        let d = tmpdir("dsq3");
        write(&d, "dsq.smd", SMD);
        let desc = ModelDesc::from_toml(&dsq_desc(
            r#"[[sequences]]
name = "same"
smd = "dsq.smd"
fps = 30.0

[[sequences]]
name = "same"
forward_declared = true
"#,
        ))
        .unwrap();
        assert!(
            desc.validate().is_ok(),
            "同名空壳不该报重复：{:?}",
            desc.validate().unwrap_err()
        );
        let c = compile(&desc, &d).unwrap();
        assert_eq!(c.sequences.len(), 2, "两条同名序列都要保留");
        assert!(!c.sequences[0].forward_declared);
        assert!(c.sequences[1].forward_declared);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn forward_declared_sequence_rejects_a_real_smd() {
        // 空壳带 SMD 说明两种东西被混在了一起 —— 产物会「看起来对但语义错」。
        let d = tmpdir("dsq4");
        write(&d, "dsq.smd", SMD);
        let desc = ModelDesc::from_toml(&dsq_desc(
            r#"[[sequences]]
name = "shell"
forward_declared = true
smd = "dsq.smd"
"#,
        ))
        .unwrap();
        let errs = desc.validate().unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.path == "sequences[0].smd" && e.message.contains("不该有 smd")),
            "{errs:?}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn forward_declared_survives_a_model_with_no_animations_at_all() {
        // ⚠️ **本轮修掉的一个真 bug**：只有空壳的模型
        // `numlocalanim == 0` 而 `numlocalseq > 0`。
        // `mdl_writer` 早先把 seqdesc 数组整块挂在 `anim_count > 0` 上，
        // 于是空壳**一个字节都没写**。
        //
        // 官方实测（`dump_mdl_header.js`）：`numlocalanim=0` 时
        // `localanimindex` / `localseqindex` **都不是 0**（都是 1532），
        // seqdesc 数组照写不误。
        let d = tmpdir("dsq5");
        write(&d, "dsq.smd", SMD);
        let desc = ModelDesc::from_toml(&dsq_desc(
            r#"[[sequences]]
name = "shell_a"
forward_declared = true

[[sequences]]
name = "shell_b"
forward_declared = true
"#,
        ))
        .unwrap();
        let c = compile(&desc, &d).unwrap();
        assert_eq!(c.sequences.len(), 2);
        assert!(
            c.sequences.iter().all(|s| s.forward_declared),
            "两条都应是空壳"
        );
        // 写出后自检：`localseqindex` 必须指向 seqdesc 数组而不是 0。
        let out = crate::mdl_writer::write_mdl(&c).unwrap();
        let rd = |o: usize| i32::from_le_bytes(out.bytes[o..o + 4].try_into().unwrap());
        let seq_off = rd(0xC0);
        assert_ne!(seq_off, 0, "有序列时 localseqindex 不该是 0");
        assert_eq!(rd(0xBC), 2, "numlocalseq");
        // 该偏移处的第一条 seqdesc 必须真的是空壳：`flags = 0x800`。
        let flags = i32::from_le_bytes(
            out.bytes[seq_off as usize + 0x0C..seq_off as usize + 0x10]
                .try_into()
                .unwrap(),
        );
        assert_eq!(
            flags & 0x800,
            0x800,
            "seqdesc[0].flags 应当带 STUDIO_OVERRIDE（说明数组真的写出来了）"
        );
        std::fs::remove_dir_all(&d).ok();
    }
}
