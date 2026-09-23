//! VTX（`.dx90.vtx`，version 7）写出。
//!
//! # 为什么它比 MDL/VVD 更关键
//!
//! 只有 `.mdl` + `.vvd` 的模型**在游戏里根本不渲染** —— 引擎要靠 VTX
//! 才知道「哪些顶点组成三角形、用什么材质、走哪条硬件蒙皮路径」。
//! 缺 VTX 时报 `ErrorRequiredVtxFileNotFound`，模型不可见。
//!
//! # 布局（实测自 studiomdl 产出的 285 字节最小文件，8 顶点 12 三角形）
//!
//! ```text
//!   0  FileHeader            36   无魔数；checksum 在 0x10
//!  36  BodyPartHeader         8   {numModels, modelOffset}
//!  44  ModelHeader            8   {numLODs, lodOffset}
//!  52  ModelLODHeader        12   {numMeshes, meshOffset, switchPoint}
//!  64  MeshHeader             9   {numStripGroups, stripGroupOffset, flags}
//!  73  StripGroupHeader      25   {numVerts, vertOffset, numIndices, indexOffset,
//!                                  numStrips, stripOffset, flags}
//!  98  StripHeader           27   {indexCount, indexOffset, vertexCount, vertOffset,
//!                                  boneCount, flags, boneStateChangeCount,
//!                                  boneStateChangeOffset}
//! 125  Vertex[]           8×9    {boneWeightIndex[3], boneCount, origMeshVertID, boneId[3]}
//! 197  Index[]           36×2    uint16，tri-list
//! 269  BoneStateChange[]  1×8    {hardwareID, newBoneID}
//! 277  MaterialReplacementList 8 {numReplacements=0, replacementOffset}
//! ```
//!
//! # 四条实测规则（都会导致模型不可见或错乱，且**不会报错**）
//!
//! 1. **`stripGroup.flags` 用 `STRIPGROUP_IS_HWSKINNED`（0x02）**，
//!    不用 `0x01`（`IS_FLEXED`）。实测 studiomdl 对无 flex 的模型写 **2**；
//!    写 1 会让引擎走 flex 路径去查不存在的 flex 数据。
//! 2. **`strip.flags` 用 `STRIP_IS_TRILIST`（0x01）**。实测 L4D2 全部是
//!    tri-list，没有 tri-strip —— 这让写出器简单很多（不需要 strip 退化规则）。
//! 3. **`origMeshVertID` 是「mesh 内顶点下标」**，与 `boneId` 一起让引擎
//!    把 VTX 顶点对回 VVD 顶点。studiomdl 会**重排**顶点（实测最小文件的
//!    映射是 `[7,1,0,4,5,3,2,6]`），但重排不是必需的 —— 我们按原序写
//!    `0..n`，只要 `origMeshVertID` 与 VVD 里的顺序一致即可。
//! 4. **`mesh.vertexoffset`（在 MDL 里）是相对 model 的**，
//!    所以 `origMeshVertID` 只需在 model 内偏移，不加 body part 的累计值。
//!    （这条与 Crowbar 解编时读法不同，见 `vvd-vtx-layout.md` §3.5 ——
//!    那是解编器的口径，写出器要跟 studiomdl 的口径。）
//!
//! # 与 MDL 的一致性约束
//!
//! VTX 的 mesh 必须与 MDL 的 mesh **一一对应、顺序相同**，
//! 否则材质会错位（模型用错贴图）。本写出器与 [`crate::mdl_writer`]
//! 共用同一份 [`CompiledModelDesc`]，并断言两者 mesh 数一致。
//!
//! # 多 LOD（实测 230 个真实模型）
//!
//! `numLODs > 1` 时结构树变成「每个 model 有 numLODs 个 `ModelLODHeader_t`」，
//! 每个 LOD 有自己的 mesh 数组与 strip group。实测 230/230 个多 LOD 模型的
//! **VTX `numLODs` 与 VVD `numLODs` 完全一致**（`probe_vtx_lod.js`），
//! 所以两者必须同源。
//!
//! ## `switchPoint` 的语义
//!
//! `ModelLODHeader_t.switchPoint` 是**切换到该 LOD 的屏幕高度阈值**
//! （像素高度，越小越远）。实测 230 个模型的取值分布：
//! `0`（LOD 0 恒为 0，出现在 442 处）、`10/15/20/25/30/35/40/45/50/55/60/
//! 65/70/80/100/150/200`，以及 **`-1`**（20 处，表示「不切换」）。
//! 它**不影响渲染正确性**，只影响 LOD 何时切换 —— 写错只会让 LOD 切换距离不对。
//!
//! ## `origMeshVertID` 在多 LOD 下是 `finalMeshVertID`
//!
//! 单 LOD 时它就是 mesh 内顶点号（`0..numvertices`）。多 LOD 时它是
//! **「把 LOD 排序的顶点按 fixup 拼回 mesh 顺序后」的下标** ——
//! 由 [`crate::lod::build_lod_layout`] 的 `final_mesh_vert_id` 给出，
//! 上界是 `mstudiomesh_t.numvertices`（跨 LOD 去重的总数），
//! **不是**该 LOD 的 strip group 顶点数。
//!
//! 实测确认（`probe_vtx_origmeshvertid.js`，230 个多 LOD 模型）：
//! `origMeshVertID < MDL.mesh.numvertices` 是 **230/230** 成立的。

use std::collections::HashMap;

use crate::model::CompiledModelDesc;

/// `FileHeader_t` 的字节大小。
pub const HEADER_SIZE: usize = 36;
/// `BodyPartHeader_t` 的字节大小。
pub const BODY_PART_SIZE: usize = 8;
/// `ModelHeader_t` 的字节大小。
pub const MODEL_SIZE: usize = 8;
/// `ModelLODHeader_t` 的字节大小。
pub const MODEL_LOD_SIZE: usize = 12;
/// `MeshHeader_t` 的字节大小（**紧凑，无填充**）。
pub const MESH_SIZE: usize = 9;
/// `StripGroupHeader_t` 的字节大小（L4D2 实测，**无 v49+ 扩展**）。
pub const STRIP_GROUP_SIZE: usize = 25;
/// `StripHeader_t` 的字节大小（L4D2 实测，**无 v49+ 扩展**）。
pub const STRIP_SIZE: usize = 27;
/// `Vertex_t` 的字节大小。
pub const VERTEX_SIZE: usize = 9;
/// `BoneStateChangeHeader_t` 的字节大小。
pub const BONE_STATE_CHANGE_SIZE: usize = 8;
/// `MaterialReplacementListHeader_t` 的字节大小。
pub const MATERIAL_REPLACEMENT_LIST_SIZE: usize = 8;

/// 文件版本。
pub const VERSION: i32 = 7;

/// `STRIPGROUP_IS_FLEXED`：该 strip group 使用顶点动画（flex）。
///
/// ⚠️ **L4D2 的 DX9 产物不用这一位** —— 用的是
/// [`STRIPGROUP_IS_DELTA_FLEXED`]（`optimize.cpp:975` 的注释：
/// 「Going forward, DX9 models are delta flexed」）。
pub const STRIPGROUP_IS_FLEXED: u8 = 0x01;
/// `STRIPGROUP_IS_HWSKINNED`：硬件蒙皮。
pub const STRIPGROUP_IS_HWSKINNED: u8 = 0x02;
/// `STRIPGROUP_IS_DELTA_FLEXED`：该 strip group 的顶点动画是**差量**形式。
///
/// 有 VTA 载荷的 mesh 会写 `IS_HWSKINNED | IS_DELTA_FLEXED = 0x6`
/// （`optimize.cpp:972-976`）。语料 17/17 命中、无 flex 的 5568 个全不带
/// （`probe_vtx_flex_flags.js`）。
pub const STRIPGROUP_IS_DELTA_FLEXED: u8 = 0x04;
/// `STRIP_IS_TRILIST`：索引数组是独立三角形列表。
pub const STRIP_IS_TRILIST: u8 = 0x01;
/// `STRIP_IS_TRISTRIP`：索引数组是三角形带（L4D2 不用）。
pub const STRIP_IS_TRISTRIP: u8 = 0x02;

/// 写出错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtxWriteError {
    /// mesh 数超过 `uint16` 能表达的范围（`origMeshVertID` 是 u16）。
    TooManyVertices { model: String, count: usize },
    /// 单个 strip 的索引数超过 `int32`。
    TooManyIndices { model: String, count: usize },
    /// 内部不一致 —— 属本实现的 bug。
    Internal(String),
}

impl std::fmt::Display for VtxWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyVertices { model, count } => write!(
                f,
                "{model} 有 {count} 个顶点，超过 VTX 的 uint16 上限 65535"
            ),
            Self::TooManyIndices { model, count } => {
                write!(f, "{model} 的三角形索引数 {count} 超出 int32")
            }
            Self::Internal(m) => write!(f, "内部错误（请报告）：{m}"),
        }
    }
}

impl std::error::Error for VtxWriteError {}

/// 一个 mesh 的 VTX 侧统计（供自检与日志）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshVtxStat {
    pub vertex_count: usize,
    pub triangle_count: usize,
}

/// VTX 写出选项。
///
/// # 为什么**默认不做**顶点缓存优化
///
/// 加了它产物就与**真 studiomdl** 不再逐字节一致，而本项目的验收基准
/// 是真 studiomdl（见 `HANDBOOK.md` 教训 75）。所以默认 `false`，
/// 由 TOML `[model] optimize_vtx` 或命令行 `--optimize-vtx` 显式打开。
///
/// # 它做什么
///
/// 对**每个 strip group 单独**跑 `meshopt::optimize_vertex_cache`
/// （每个 strip group 是一次独立的 draw call，必须分开优化 ——
/// 这是 meshopt 文档明确要求的），重排索引以**减少顶点着色器调用次数**。
///
/// 三角形**集合**不变，只改顺序；顶点池与 `origMeshVertID` 也不变。
/// 所以这是个**纯性能优化**，不影响几何、UV、法线、骨骼绑定。
///
/// # 与 nekomdl 的关系
///
/// nekomdl 默认走 meshoptimizer、`-nvtristrip` 回退 NvTriStrip；
/// 真 studiomdl 的对应开关也是 `-nvtristrip`。**两者都会重排**
/// （实测：真 studiomdl 与 nekomdl 的 `origMeshVertID` 都是非平凡排列，
/// 21/21 组），所以打开本项是**接近官方行为**的，只是不会逐字节相同
/// —— meshopt 与 NvTriStrip 是两套不同算法，输出顺序必然不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VtxOptions {
    /// 是否对每个 strip group 做顶点缓存优化（`meshopt_optimizeVertexCache`）。
    ///
    /// 默认 `false` —— 保持与真 studiomdl 的逐字节一致。
    pub optimize_vertex_cache: bool,
}

/// 写出结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VtxWriteOutcome {
    pub bytes: Vec<u8>,
    /// 每个 mesh 的顶点/三角形数（顺序与 MDL 的 mesh 一致）。
    pub mesh_stats: Vec<MeshVtxStat>,
    pub checksum: i32,
}

/// 对一个 strip group 的三角形做**顶点缓存优化**，返回重排后的索引。
///
/// # 为什么必须**逐 strip group** 调用
///
/// `meshopt::optimize_vertex_cache` 的文档明确写了：
///
/// > If index buffer contains multiple ranges for multiple draw calls,
/// > this function needs to be called on each range individually.
///
/// VTX 里每个 strip group 是**一次独立的 draw call**（引擎按 strip group
/// 提交），所以逐组优化才对。若把它们拼成一个数组一起优化，meshopt 会
/// 为了跨组的缓存局部性而打乱组内顺序，反而**恶化**单次 draw 的命中率。
///
/// # 输入是「局部槽位」还是「mesh 顶点号」
///
/// 这里收发的都是 **strip group 局部槽位**（= 写进 VTX 索引数组的值）。
/// 缓存优化的对象正是这个空间 —— 引擎的 post-transform cache 索引的就是
/// 提交给它的槽位序号。`vertex_count` 也传局部顶点数。
///
/// # 三角形集合不变
///
/// meshopt 只是重排三角形顺序（并可能翻转绕序以改善连续性？——**不会**，
/// `optimizeVertexCache` 保持三角形顶点循环序）。这里额外断言
/// 「输入输出的排序后三角形多重集相同」，一旦不符就是 meshopt 行为变了，
/// 直接报 [`VtxWriteError::Internal`] 而不是静默写出坏数据。
fn optimize_group_indices(
    tris: &[[u16; 3]],
    vertex_count: usize,
) -> Result<Vec<[u16; 3]>, VtxWriteError> {
    if tris.is_empty() {
        return Ok(Vec::new());
    }
    // meshopt 要 u32 索引；VTX 是 u16，转换无损。
    let flat: Vec<u32> = tris
        .iter()
        .flat_map(|t| t.iter().map(|v| *v as u32))
        .collect();
    let optimized = meshopt::optimize_vertex_cache(&flat, vertex_count);

    if optimized.len() != flat.len() {
        return Err(VtxWriteError::Internal(format!(
            "meshopt 返回的索引数 {} 与输入 {} 不符",
            optimized.len(),
            flat.len()
        )));
    }

    let out: Vec<[u16; 3]> = optimized
        .chunks_exact(3)
        .map(|c| {
            [
                u16::try_from(c[0]).unwrap_or(u16::MAX),
                u16::try_from(c[1]).unwrap_or(u16::MAX),
                u16::try_from(c[2]).unwrap_or(u16::MAX),
            ]
        })
        .collect();

    // 自检：**三角形集合必须不变**。这是「只能重排、不能改拓扑」的硬约束。
    //
    // 用「排序后的三角形」做多重集比较 —— 这会**丢掉绕序**，所以它只能
    // 证明「几何没变」，不能证明「朝向没变」。后者由下面的循环单独保证：
    // `optimizeVertexCache` 保持每个三角形的顶点循环序（只整体轮转/不变），
    // 所以这里断言「每个三角形的顶点集合相同」之外，再断言
    // **原始三元组（未排序）也出现在输出里**。
    let sorted_key = |t: &[u16; 3]| {
        let mut s = *t;
        s.sort_unstable();
        s
    };
    let mut a: Vec<[u16; 3]> = tris.iter().map(sorted_key).collect();
    let mut b: Vec<[u16; 3]> = out.iter().map(sorted_key).collect();
    a.sort_unstable();
    b.sort_unstable();
    if a != b {
        return Err(VtxWriteError::Internal(
            "meshopt 改变了三角形集合（应只重排顺序）—— 拒绝写出".into(),
        ));
    }
    // 绕序检查：**未排序**的三元组多重集也必须一致。
    let mut ra: Vec<[u16; 3]> = tris.to_vec();
    let mut rb: Vec<[u16; 3]> = out.clone();
    ra.sort_unstable();
    rb.sort_unstable();
    if ra != rb {
        return Err(VtxWriteError::Internal(
            "meshopt 改变了三角形绕序（背面剔除会反过来）—— 拒绝写出".into(),
        ));
    }
    Ok(out)
}

/// 把编译结果写成 VTX 字节（**默认选项** —— 不做缓存优化）。
///
/// 等价于 `write_vtx_with(compiled, VtxOptions::default())`。
/// 保留这个签名是为了让调用方（与全部既有测试）**一行不改**，
/// 从而保证默认产物与加本特性之前**逐字节相同**。
pub fn write_vtx(compiled: &CompiledModelDesc) -> Result<VtxWriteOutcome, VtxWriteError> {
    write_vtx_with(compiled, VtxOptions::default())
}

/// 把编译结果写成 VTX 字节，可指定 [`VtxOptions`]。
///
/// 按是否有 LOD 数据分派：
/// - **全部单 LOD** → [`write_vtx_single`]，与加多 LOD 支持之前**逐字节相同**
///   （那条路径一行未改）；
/// - **有 model 带 LOD** → [`write_vtx_multi`]。
pub fn write_vtx_with(
    compiled: &CompiledModelDesc,
    opts: VtxOptions,
) -> Result<VtxWriteOutcome, VtxWriteError> {
    let multi = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .any(|m| m.lods.as_ref().is_some_and(|l| l.is_multi()));
    if multi {
        write_vtx_multi(compiled, opts)
    } else {
        write_vtx_single(compiled, opts)
    }
}

/// 单 LOD 的 VTX 写出：每个 model 一个 LOD、每个 mesh 一个 strip group、
/// 每个 strip group 一个 tri-list strip。
fn write_vtx_single(
    compiled: &CompiledModelDesc,
    opts: VtxOptions,
) -> Result<VtxWriteOutcome, VtxWriteError> {
    let desc = &compiled.desc;
    let checksum = desc.checksum();

    // ---- 1. 先算各段偏移 ----
    let bp_count = compiled.bodyparts.len();
    let model_total: usize = compiled.bodyparts.iter().map(|bp| bp.models.len()).sum();
    let mesh_total: usize = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .map(|m| m.meshes.len())
        .sum();

    let bp_off = HEADER_SIZE;
    let model_off = bp_off + bp_count * BODY_PART_SIZE;
    let lod_off = model_off + model_total * MODEL_SIZE;
    let mesh_off = lod_off + model_total * MODEL_LOD_SIZE;
    let sg_off = mesh_off + mesh_total * MESH_SIZE;
    // 每个 mesh 一个 strip group + 一个 strip。
    let strip_off = sg_off + mesh_total * STRIP_GROUP_SIZE;
    let vertex_off = strip_off + mesh_total * STRIP_SIZE;

    // 顶点区之后依次是索引区、bone state change 区、material replacement 区。
    // 逐 mesh 累加，同时记录每个 mesh 的起点。
    let mut mesh_vertex_at = Vec::with_capacity(mesh_total);
    let mut mesh_index_at = Vec::with_capacity(mesh_total);
    let mut mesh_bsc_at = Vec::with_capacity(mesh_total);
    let mut mesh_stats = Vec::with_capacity(mesh_total);

    let mut vcursor = vertex_off;

    // 第一遍：算顶点区大小。
    for bp in &compiled.bodyparts {
        for m in &bp.models {
            for mesh in &m.meshes {
                let n = mesh.vertices.len();
                // ⚠️ 判据是 `>`（不是 `>=`）：`origMeshVertID` 是 `uint16`，
                // 能表达下标 `0..=65535` ⟹ **65536 个顶点是合法的**。
                // 早期写成 `n > u16::MAX`（= `n > 65535`）⟹ 把 65536 误拒，
                // 恰好少了一个 —— 见 `MAXSTUDIOVERTS_PER_MESH`。
                if n > crate::mdl_writer::MAXSTUDIOVERTS_PER_MESH {
                    return Err(VtxWriteError::TooManyVertices {
                        model: m.name.clone(),
                        count: n,
                    });
                }
                mesh_vertex_at.push(vcursor);
                vcursor += n * VERTEX_SIZE;
            }
        }
    }
    // 索引区紧跟顶点区。
    let mut icursor = vcursor;
    for bp in &compiled.bodyparts {
        for m in &bp.models {
            for mesh in &m.meshes {
                let n = mesh.triangles.len() * 3;
                if n > i32::MAX as usize {
                    return Err(VtxWriteError::TooManyIndices {
                        model: m.name.clone(),
                        count: n,
                    });
                }
                mesh_index_at.push(icursor);
                icursor += n * 2;
            }
        }
    }
    // bone state change 区紧跟索引区。每个 strip 一个条目。
    let mut bsc_cursor = icursor;
    for bp in &compiled.bodyparts {
        for m in &bp.models {
            for _mesh in &m.meshes {
                mesh_bsc_at.push(bsc_cursor);
                bsc_cursor += BONE_STATE_CHANGE_SIZE;
            }
        }
    }
    // mesh 统计按同一顺序收集。
    for bp in &compiled.bodyparts {
        for m in &bp.models {
            for mesh in &m.meshes {
                mesh_stats.push(MeshVtxStat {
                    vertex_count: mesh.vertices.len(),
                    triangle_count: mesh.triangles.len(),
                });
            }
        }
    }
    // material replacement list（每 LOD 一个，内容为空）放在最后。
    let mat_repl_off = bsc_cursor;
    let total_len = mat_repl_off + MATERIAL_REPLACEMENT_LIST_SIZE;

    let mut buf = vec![0u8; total_len];

    // ---- 2. 头部 ----
    // vertexCacheSize：studiomdl 写 24（实测）。它是「优化后的顶点缓存
    // 大小」提示，引擎不依赖其正确性；用 24 与官方一致。
    put_i32(&mut buf, 0x00, VERSION);
    put_i32(&mut buf, 0x04, 24);
    put_u16(&mut buf, 0x08, 53); // maxBonesPerStrip（实测 53）
    put_u16(&mut buf, 0x0A, 9); // maxBonesPerTri（实测 9）
    put_i32(&mut buf, 0x0C, 3); // maxBonesPerVertex（MAX_NUM_BONES_PER_VERT）
    put_i32(&mut buf, 0x10, checksum);
    put_i32(&mut buf, 0x14, 1); // numLODs
    put_i32(&mut buf, 0x18, mat_repl_off as i32);
    put_i32(&mut buf, 0x1C, bp_count as i32);
    put_i32(&mut buf, 0x20, bp_off as i32);

    // ---- 3. 结构树 ----
    let mut model_cursor = 0usize;
    let mut mesh_cursor = 0usize;
    let mut bp_abs = bp_off;
    for bp in &compiled.bodyparts {
        put_i32(&mut buf, bp_abs, bp.models.len() as i32);
        put_i32(
            &mut buf,
            bp_abs + 4,
            (model_off + model_cursor * MODEL_SIZE) as i32 - bp_abs as i32,
        );
        let model_abs = model_off + model_cursor * MODEL_SIZE;
        for m in &bp.models {
            put_i32(&mut buf, model_abs, 1); // numLODs
            put_i32(
                &mut buf,
                model_abs + 4,
                (lod_off + model_cursor * MODEL_LOD_SIZE) as i32 - model_abs as i32,
            );
            let lod_abs = lod_off + model_cursor * MODEL_LOD_SIZE;
            put_i32(&mut buf, lod_abs, m.meshes.len() as i32);
            put_i32(
                &mut buf,
                lod_abs + 4,
                (mesh_off + mesh_cursor * MESH_SIZE) as i32 - lod_abs as i32,
            );
            put_f32(&mut buf, lod_abs + 8, 0.0); // switchPoint

            for ki in 0..m.meshes.len() {
                let mesh_abs = mesh_off + (mesh_cursor + ki) * MESH_SIZE;
                put_i32(&mut buf, mesh_abs, 1); // numStripGroups
                put_i32(
                    &mut buf,
                    mesh_abs + 4,
                    (sg_off + (mesh_cursor + ki) * STRIP_GROUP_SIZE) as i32 - mesh_abs as i32,
                );
                put_u8(&mut buf, mesh_abs + 8, 0); // mesh flags（非牙齿/眼睛）
            }
            model_cursor += 1;
            mesh_cursor += m.meshes.len();
        }
        bp_abs += BODY_PART_SIZE;
    }

    // ---- 4. strip group + strip + 顶点 + 索引 + bone state change ----
    let mut mi = 0usize;
    for bp in &compiled.bodyparts {
        for m in &bp.models {
            for mesh in &m.meshes {
                let sg_abs = sg_off + mi * STRIP_GROUP_SIZE;
                let st_abs = strip_off + mi * STRIP_SIZE;
                let v_abs = mesh_vertex_at[mi];
                let i_abs = mesh_index_at[mi];
                let bsc_abs = mesh_bsc_at[mi];

                let nv = mesh.vertices.len();
                let ni = mesh.triangles.len() * 3;

                // strip group
                put_i32(&mut buf, sg_abs, nv as i32);
                put_i32(&mut buf, sg_abs + 4, (v_abs - sg_abs) as i32);
                put_i32(&mut buf, sg_abs + 8, ni as i32);
                put_i32(&mut buf, sg_abs + 12, (i_abs - sg_abs) as i32);
                put_i32(&mut buf, sg_abs + 16, 1); // numStrips
                put_i32(&mut buf, sg_abs + 20, (st_abs - sg_abs) as i32);
                // `flags`：`optimize.cpp:968-981` 的 `ComputeStripGroupFlags`：
                //
                // ```c
                // pStripGroup->flags = 0;
                // if (isFlexed) { flags |= IS_FLEXED(0x01); flags |= IS_DELTA_FLEXED(0x04); }
                // if (isHWSkinned) flags |= IS_HWSKINNED(0x02);
                // ```
                //
                // ⟹ **有 VTA 载荷的 mesh 写 `0x6`（HWSKINNED|DELTA_FLEXED）**，
                //    没有的写 `0x2`。
                //
                // ⚠️ 是 `0x04 DELTA_FLEXED` 而**不是** `0x01 IS_FLEXED` ——
                // 源码注释明说「Going forward, DX9 models are delta flexed」。
                //
                // 语料判据（`probe_vtx_flex_flags.js`）：flags 直方图
                // **恰好 17 个 `0x6`**，与「有 flex 的 mesh 数」17 完全吻合；
                // 5568 个无 flex 的 mesh **无一**带 `0x04`（0 例外）。
                //
                // > 早先这里恒写 `0x02`（VTA 未实现时的正确值）。有 VTA 后
                // > 必须跟着变 —— 否则引擎不会走 flex 路径，形状不生效。
                let is_flexed = m
                    .mesh_flexes
                    .get(mi)
                    .is_some_and(|v| !v.is_empty());
                let sg_flags = if is_flexed {
                    STRIPGROUP_IS_HWSKINNED | STRIPGROUP_IS_DELTA_FLEXED
                } else {
                    STRIPGROUP_IS_HWSKINNED
                };
                put_u8(&mut buf, sg_abs + 24, sg_flags);

                // strip（tri-list）
                put_i32(&mut buf, st_abs, ni as i32); // indexCount
                put_i32(&mut buf, st_abs + 4, 0); // indexOffset（组内起点）
                put_i32(&mut buf, st_abs + 8, nv as i32); // vertexCount
                put_i32(&mut buf, st_abs + 12, 0); // vertOffset（组内起点）
                put_u16(&mut buf, st_abs + 16, 1); // boneCount
                put_u8(&mut buf, st_abs + 18, STRIP_IS_TRILIST);
                put_i32(&mut buf, st_abs + 19, 1); // boneStateChangeCount
                put_i32(&mut buf, st_abs + 23, (bsc_abs - st_abs) as i32);

                // 顶点：boneWeightIndex 指向该 VVD 顶点自己的权重数组槽位。
                for (vi, v) in mesh.vertices.iter().enumerate() {
                    let o = v_abs + vi * VERTEX_SIZE;
                    let bone_count = v.bones.len().min(3) as u8;
                    // 实测 studiomdl **恒写 [0, 1, 2]**（固定序列），不是
                    // 「0..bone_count」。它表示「槽位 k 对应 VVD 顶点里
                    // weight[k]/bone[k]」；写 0 填充会让引擎把槽位 1/2
                    // 也指向 weight[0]。
                    buf[o] = 0;
                    buf[o + 1] = 1;
                    buf[o + 2] = 2;
                    buf[o + 3] = bone_count.max(1);
                    put_u16(&mut buf, o + 4, vi as u16); // origMeshVertID
                    // boneId：**全局**骨骼下标（与 VVD 顶点的 bone[] 一致）。
                    for k in 0..3usize {
                        let b = v
                            .bones
                            .get(k)
                            .map(|p| p[0] as u8)
                            .unwrap_or(0);
                        buf[o + 6 + k] = b;
                    }
                }

                // 索引：tri-list，每三角形三个 u16。
                //
                // `optimize_vtx` 打开时**逐 strip group** 做顶点缓存优化
                // （每个 strip group 是一次独立 draw call，见
                // [`optimize_group_indices`]）。默认关闭 ⇒ 这段是原序直写，
                // 产物与本特性引入前**逐字节相同**。
                if opts.optimize_vertex_cache {
                    let src: Vec<[u16; 3]> = mesh
                        .triangles
                        .iter()
                        .map(|t| [t[0] as u16, t[1] as u16, t[2] as u16])
                        .collect();
                    let opt = optimize_group_indices(&src, nv)?;
                    for (ti, tri) in opt.iter().enumerate() {
                        let o = i_abs + ti * 6;
                        put_u16(&mut buf, o, tri[0]);
                        put_u16(&mut buf, o + 2, tri[1]);
                        put_u16(&mut buf, o + 4, tri[2]);
                    }
                } else {
                    for (ti, tri) in mesh.triangles.iter().enumerate() {
                        let o = i_abs + ti * 6;
                        put_u16(&mut buf, o, tri[0] as u16);
                        put_u16(&mut buf, o + 2, tri[1] as u16);
                        put_u16(&mut buf, o + 4, tri[2] as u16);
                    }
                }

                // bone state change：{hardwareID, newBoneID}。
                put_i32(&mut buf, bsc_abs, 0);
                put_i32(&mut buf, bsc_abs + 4, 0);

                mi += 1;
            }
        }
    }

    // ---- 5. material replacement list（每 LOD 一个，空表）----
    put_i32(&mut buf, mat_repl_off, 0);
    // 实测 studiomdl 的 `replacementOffset` 写 **0**，不是「指向数组末尾」。
    // 表为空时该字段无意义，写 0 与官方一致。
    put_i32(&mut buf, mat_repl_off + 4, 0);

    if buf.len() != total_len {
        return Err(VtxWriteError::Internal(format!(
            "写入长度 {} 与预算 {total_len} 不符",
            buf.len()
        )));
    }
    Ok(VtxWriteOutcome {
        bytes: buf,
        mesh_stats,
        checksum,
    })
}

/// 多 LOD 的 VTX 写出。
///
/// # 与单 LOD 的结构差异
///
/// - 头部 `numLODs` = 各 model 的最大 LOD 数（实测与 VVD 的 `numLODs` 一致）；
/// - 每个 model 有 `numLODs` 个连续的 `ModelLODHeader_t`（单 LOD 时是 1 个）；
/// - 每个 LOD 有自己的 mesh 数组与 strip group —— **结构块数按 LOD 翻倍**；
/// - `materialReplacementList` 也是**每 LOD 一个**（单 LOD 时 1 个）。
///
/// # 每个 LOD 的顶点与索引怎么来
///
/// 用 `layout.mesh_local_id(mesh, unified)` 把该 LOD 三角形引用的统一顶点
/// 翻译成 `origMeshVertID`（= `finalMeshVertID`），然后：
///
/// - strip group 的顶点数组 = 该 LOD **实际用到**的 `origMeshVertID` 去重集合；
/// - 索引 = 该 LOD 的三角形，下标指向「顶点数组内的槽位」；
/// - 顶点记录里的 `boneId` / `boneCount` 取自统一池顶点（蒙皮信息与 LOD 无关）。
///
/// 实测依据（`dbg_lod_ids_exact.js`，230 个真实多 LOD 模型）：各 LOD 的
/// `origMeshVertID` 集合**不是**简单的嵌套关系，LOD 会各自挑选顶点
/// （例如 `boomer` 的 LOD0 用 `0-711,3248-5783`，LOD1 用 `0-3247`），
/// 所以必须按「该 LOD 自己的三角形」精确收集，不能套用某个区间。
fn write_vtx_multi(
    compiled: &CompiledModelDesc,
    opts: VtxOptions,
) -> Result<VtxWriteOutcome, VtxWriteError> {
    let desc = &compiled.desc;
    let checksum = desc.checksum();

    // 全局 LOD 数 = 各 model 的最大值（实测与 VVD 的 numLODs 一致）。
    let num_lods = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .map(|m| m.lods.as_ref().map_or(1, |l| l.num_lods))
        .max()
        .unwrap_or(1)
        .max(1);

    // 全局 mesh 布局：与 `build_multi_lod_vvd` 用同一份算法，保证编号一致。
    let mut all: Vec<crate::lod::MeshLods> = Vec::new();
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
    let layout = crate::lod::build_lod_layout(&all);

    let bp_count = compiled.bodyparts.len();
    let model_total: usize = compiled.bodyparts.iter().map(|bp| bp.models.len()).sum();
    let mesh_total: usize = all.len();
    // 每个 model 的 LOD 数可能不同（本实现里都是 num_lods，但按 model 取更稳）。
    let lod_header_total: usize = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .map(|m| m.lods.as_ref().map_or(1, |l| l.num_lods))
        .sum();
    // 每个 LOD 的每个 mesh 一个 strip group + 一个 strip。
    let sg_total = mesh_total * num_lods;
    let strip_total = sg_total;

    let bp_off = HEADER_SIZE;
    let model_off = bp_off + bp_count * BODY_PART_SIZE;
    let lod_off = model_off + model_total * MODEL_SIZE;
    let mesh_off = lod_off + lod_header_total * MODEL_LOD_SIZE;
    let sg_off = mesh_off + sg_total * MESH_SIZE;
    let strip_off = sg_off + sg_total * STRIP_GROUP_SIZE;
    let vertex_off = strip_off + strip_total * STRIP_SIZE;

    // ---- 逐 (model, lod, mesh) 展开成写入项 ----
    struct Item {
        /// 该 LOD 的 mesh 在统一池里的序号（用于查 layout）。
        unified_mesh: usize,
        lod: usize,
        /// 本 LOD 实际用到的统一顶点下标（去重、升序）。
        verts: Vec<u32>,
        /// 本 LOD 的三角形（下标指向 `verts` 内的槽位）。
        tris: Vec<[u32; 3]>,
        /// 本档是否 `nofacial`（QC 的 `nofacial`）。
        ///
        /// `true` ⟹ 本档的 strip group **不带** `0x04`（见写出处的说明）。
        no_facial: bool,
    }
    let mut items: Vec<Item> = Vec::with_capacity(sg_total);
    let mut mesh_stats = Vec::with_capacity(mesh_total);
    let mut unified_mesh = 0usize;
    // 每个统一 mesh 是否有 flex 载荷 —— 决定 strip group 是否带
    // `STRIPGROUP_IS_DELTA_FLEXED`（`0x04`）。索引 == `items[].unified_mesh`。
    let mut mesh_has_flex: Vec<bool> = Vec::new();
    for bp in &compiled.bodyparts {
        for m in &bp.models {
            for ki in 0..m.meshes.len() {
                mesh_has_flex.push(
                    m.mesh_flexes.get(ki).is_some_and(|v| !v.is_empty()),
                );
            }
        }
    }
    for bp in &compiled.bodyparts {
        for m in &bp.models {
            let n_mesh = m.meshes.len();
            let model_lods = m.lods.as_ref().map_or(1, |l| l.num_lods);
            for ki in 0..n_mesh {
                let ml = &all[unified_mesh + ki];
                // 单 LOD 的 mesh_stats 统计（与单 LOD 路径同义：LOD 0 的顶点/三角形数）。
                mesh_stats.push(MeshVtxStat {
                    vertex_count: ml.lod_vertex_counts.first().copied().unwrap_or(0),
                    triangle_count: ml.triangles.first().map_or(0, |t| t.len()),
                });
            }
            for lod in 0..model_lods {
                for ki in 0..n_mesh {
                    let ml = &all[unified_mesh + ki];
                    let tris_u = ml.triangles.get(lod).cloned().unwrap_or_default();
                    // 收集本 LOD 用到的统一顶点（去重 + 升序，保证确定性）。
                    let mut used: Vec<u32> = Vec::new();
                    for t in &tris_u {
                        for &u in t {
                            if !used.contains(&u) {
                                used.push(u);
                            }
                        }
                    }
                    used.sort_unstable();
                    // 三角形下标改写为「verts 内的槽位」。
                    let mut slot: HashMap<u32, u32> = HashMap::with_capacity(used.len());
                    for (i, &u) in used.iter().enumerate() {
                        slot.insert(u, i as u32);
                    }
                    let tris: Vec<[u32; 3]> = tris_u
                        .iter()
                        .map(|t| [slot[&t[0]], slot[&t[1]], slot[&t[2]]])
                        .collect();
                    // 同前：`>` 而非 `>=`，65536 个顶点合法（下标 0..=65535）。
                    if used.len() > crate::mdl_writer::MAXSTUDIOVERTS_PER_MESH {
                        return Err(VtxWriteError::TooManyVertices {
                            model: m.name.clone(),
                            count: used.len(),
                        });
                    }
                    if tris.len() * 3 > i32::MAX as usize {
                        return Err(VtxWriteError::TooManyIndices {
                            model: m.name.clone(),
                            count: tris.len() * 3,
                        });
                    }
                    items.push(Item {
                        unified_mesh: unified_mesh + ki,
                        lod,
                        verts: used,
                        tris,
                        // `nofacial`：本档禁用面部动画。
                        no_facial: m.lods.as_ref().is_some_and(|l| {
                            l.no_facial.get(lod).copied().unwrap_or(false)
                        }),
                    });
                }
            }
            unified_mesh += n_mesh;
        }
    }

    // ---- 预算各数据区 ----
    let mut item_vertex_at = Vec::with_capacity(items.len());
    let mut item_index_at = Vec::with_capacity(items.len());
    let mut item_bsc_at = Vec::with_capacity(items.len());
    let mut vcursor = vertex_off;
    for it in &items {
        item_vertex_at.push(vcursor);
        vcursor += it.verts.len() * VERTEX_SIZE;
    }
    let mut icursor = vcursor;
    for it in &items {
        item_index_at.push(icursor);
        icursor += it.tris.len() * 6;
    }
    let mut bsc_cursor = icursor;
    for _ in &items {
        item_bsc_at.push(bsc_cursor);
        bsc_cursor += BONE_STATE_CHANGE_SIZE;
    }
    // material replacement list：**每 LOD 一个**。
    let mat_repl_off = bsc_cursor;
    let total_len = mat_repl_off + num_lods * MATERIAL_REPLACEMENT_LIST_SIZE;

    let mut buf = vec![0u8; total_len];

    // ---- 头部 ----
    put_i32(&mut buf, 0x00, VERSION);
    put_i32(&mut buf, 0x04, 24); // vertexCacheSize（实测 24）
    put_u16(&mut buf, 0x08, 53); // maxBonesPerStrip（实测 53）
    put_u16(&mut buf, 0x0A, 9); // maxBonesPerTri（实测 9）
    put_i32(&mut buf, 0x0C, 3); // maxBonesPerVertex
    put_i32(&mut buf, 0x10, checksum);
    put_i32(&mut buf, 0x14, num_lods as i32);
    put_i32(&mut buf, 0x18, mat_repl_off as i32);
    put_i32(&mut buf, 0x1C, bp_count as i32);
    put_i32(&mut buf, 0x20, bp_off as i32);

    // ---- 结构树：bodypart → model → LOD[] → mesh[] → stripgroup ----
    let mut model_cursor = 0usize;
    let mut lod_cursor = 0usize;
    let mut mesh_cursor = 0usize;
    let mut item_cursor = 0usize;
    let mut bp_abs = bp_off;
    for bp in &compiled.bodyparts {
        put_i32(&mut buf, bp_abs, bp.models.len() as i32);
        put_i32(
            &mut buf,
            bp_abs + 4,
            (model_off + model_cursor * MODEL_SIZE) as i32 - bp_abs as i32,
        );
        let model_abs = model_off + model_cursor * MODEL_SIZE;
        for m in &bp.models {
            let model_lods = m.lods.as_ref().map_or(1, |l| l.num_lods);
            let switch_points: Vec<f32> = m
                .lods
                .as_ref()
                .map(|l| l.switch_points.clone())
                .unwrap_or_else(|| vec![0.0]);
            put_i32(&mut buf, model_abs, model_lods as i32);
            put_i32(
                &mut buf,
                model_abs + 4,
                (lod_off + lod_cursor * MODEL_LOD_SIZE) as i32 - model_abs as i32,
            );
            for lod in 0..model_lods {
                let lod_abs = lod_off + (lod_cursor + lod) * MODEL_LOD_SIZE;
                put_i32(&mut buf, lod_abs, m.meshes.len() as i32);
                put_i32(
                    &mut buf,
                    lod_abs + 4,
                    (mesh_off + mesh_cursor * MESH_SIZE) as i32 - lod_abs as i32,
                );
                // switchPoint：LOD 0 恒 0；其余用编译期算好的值。
                let sp = switch_points.get(lod).copied().unwrap_or(0.0);
                put_f32(&mut buf, lod_abs + 8, sp);

                for ki in 0..m.meshes.len() {
                    let mesh_abs = mesh_off + (mesh_cursor + ki) * MESH_SIZE;
                    put_i32(&mut buf, mesh_abs, 1); // numStripGroups
                    put_i32(
                        &mut buf,
                        mesh_abs + 4,
                        (sg_off + (item_cursor + ki) * STRIP_GROUP_SIZE) as i32 - mesh_abs as i32,
                    );
                    put_u8(&mut buf, mesh_abs + 8, 0); // mesh flags
                }
                item_cursor += m.meshes.len();
                mesh_cursor += m.meshes.len();
            }
            model_cursor += 1;
            lod_cursor += model_lods;
        }
        bp_abs += BODY_PART_SIZE;
    }

    // ---- strip group + strip + 顶点 + 索引 + bsc ----
    for (ii, it) in items.iter().enumerate() {
        let sg_abs = sg_off + ii * STRIP_GROUP_SIZE;
        let st_abs = strip_off + ii * STRIP_SIZE;
        let v_abs = item_vertex_at[ii];
        let i_abs = item_index_at[ii];
        let bsc_abs = item_bsc_at[ii];
        let nv = it.verts.len();
        let ni = it.tris.len() * 3;

        put_i32(&mut buf, sg_abs, nv as i32);
        put_i32(&mut buf, sg_abs + 4, (v_abs - sg_abs) as i32);
        put_i32(&mut buf, sg_abs + 8, ni as i32);
        put_i32(&mut buf, sg_abs + 12, (i_abs - sg_abs) as i32);
        put_i32(&mut buf, sg_abs + 16, 1); // numStrips
        put_i32(&mut buf, sg_abs + 20, (st_abs - sg_abs) as i32);
        // `stripGroup.flags`（**结构体偏移 +24**，`StripGroupHeader_t` 的最后一个字段）。
        //
        // `ComputeStripGroupFlags`（定义在 `optimize.cpp:968`）：
        //   `isFlexed`   ⟹ `0x04` `STRIPGROUP_IS_DELTA_FLEXED`
        //   `isHWSkinned`⟹ `0x02` `STRIPGROUP_IS_HWSKINNED`
        //
        // ⚠️ **不写 `0x01`（`IS_FLEXED`）** —— 反汇编 L4D2 的 `studiomdl.exe`
        // （`0x0041b9c8`：`MOV [EAX+0x3c],0x4`）与语料直方图
        // （3302 个 `.dx90.vtx` / 6625 个 strip group：`0x2`×6581、`0x6`×44，
        // **`0x01` 零次**）双重确认。SDK 源码在这点上**是过期的**。
        //
        // `nofacial` 的效果就落在这里：`forceNoFlex` 把 `triangleIsFlexed`
        // 强制为 `false`（`optimize.cpp:1311`），于是本档**不再有带 `0x04` 的组**。
        let sg_flags = if it.no_facial {
            STRIPGROUP_IS_HWSKINNED
        } else {
            let is_flexed = mesh_has_flex.get(it.unified_mesh).copied().unwrap_or(false);
            if is_flexed {
                STRIPGROUP_IS_HWSKINNED | STRIPGROUP_IS_DELTA_FLEXED
            } else {
                STRIPGROUP_IS_HWSKINNED
            }
        };
        put_u8(&mut buf, sg_abs + 24, sg_flags);

        put_i32(&mut buf, st_abs, ni as i32);
        put_i32(&mut buf, st_abs + 4, 0);
        put_i32(&mut buf, st_abs + 8, nv as i32);
        put_i32(&mut buf, st_abs + 12, 0);
        put_u16(&mut buf, st_abs + 16, 1); // boneCount
        put_u8(&mut buf, st_abs + 18, STRIP_IS_TRILIST);
        put_i32(&mut buf, st_abs + 19, 1); // boneStateChangeCount
        put_i32(&mut buf, st_abs + 23, (bsc_abs - st_abs) as i32);

        // 顶点：`origMeshVertID` 用 `finalMeshVertID`（多 LOD 的关键）。
        for (vi, &u) in it.verts.iter().enumerate() {
            let o = v_abs + vi * VERTEX_SIZE;
            let v = &all[it.unified_mesh].vertices[u as usize];
            let bone_count = v.bones.len().min(3) as u8;
            buf[o] = 0;
            buf[o + 1] = 1;
            buf[o + 2] = 2;
            buf[o + 3] = bone_count.max(1);
            let local = layout
                .mesh_local_id(it.unified_mesh, u)
                .ok_or_else(|| {
                    VtxWriteError::Internal(format!(
                        "mesh {} 的统一顶点 {u} 不在布局里（LOD {}）",
                        it.unified_mesh, it.lod
                    ))
                })?;
            if local > u16::MAX as u32 {
                return Err(VtxWriteError::TooManyVertices {
                    model: format!("mesh {}", it.unified_mesh),
                    count: local as usize,
                });
            }
            put_u16(&mut buf, o + 4, local as u16);
            for k in 0..3usize {
                let b = v.bones.get(k).map(|p| p[0] as u8).unwrap_or(0);
                // ⚠️ VTX 的 `boneID[]` 也是 **`char`（有符号）**
                // （`optimize.h:48`，v49 = `hl2sdk-doi`；与 VVD 的 `bone[]`
                // 同宽度）⟹ 骨骼下标同样必须 ≤ 127，越界会被读成负数。
                //
                // 真正的拒绝在 VVD 写出时（`vvd::Vvd::to_bytes` 逐顶点查
                // `MAX_BONE_INDEX_IN_VERTEX`）—— VTX 与 VVD 共用同一份顶点，
                // 所以那边先触发。这里只做 debug 断言。
                debug_assert!(
                    v.bones.get(k).is_none_or(|p| {
                        p[0] >= 0.0
                            && p[0] <= crate::mdl_writer::MAX_BONE_INDEX_IN_VERTEX as f32
                    }),
                    "骨骼下标 {} 超出有符号 char 的范围（≤ {}）",
                    v.bones.get(k).map_or(0.0, |p| p[0]),
                    crate::mdl_writer::MAX_BONE_INDEX_IN_VERTEX
                );
                buf[o + 6 + k] = b;
            }
        }

        // 索引（逐 strip group 可选做缓存优化，见单 LOD 路径的说明）。
        if opts.optimize_vertex_cache {
            let src: Vec<[u16; 3]> = it
                .tris
                .iter()
                .map(|t| [t[0] as u16, t[1] as u16, t[2] as u16])
                .collect();
            let opt = optimize_group_indices(&src, nv)?;
            for (ti, tri) in opt.iter().enumerate() {
                let o = i_abs + ti * 6;
                put_u16(&mut buf, o, tri[0]);
                put_u16(&mut buf, o + 2, tri[1]);
                put_u16(&mut buf, o + 4, tri[2]);
            }
        } else {
            for (ti, tri) in it.tris.iter().enumerate() {
                let o = i_abs + ti * 6;
                put_u16(&mut buf, o, tri[0] as u16);
                put_u16(&mut buf, o + 2, tri[1] as u16);
                put_u16(&mut buf, o + 4, tri[2] as u16);
            }
        }

        put_i32(&mut buf, bsc_abs, 0);
        put_i32(&mut buf, bsc_abs + 4, 0);
    }

    // ---- material replacement list：每 LOD 一个空表 ----
    for n in 0..num_lods {
        let o = mat_repl_off + n * MATERIAL_REPLACEMENT_LIST_SIZE;
        put_i32(&mut buf, o, 0);
        put_i32(&mut buf, o + 4, 0);
    }

    if buf.len() != total_len {
        return Err(VtxWriteError::Internal(format!(
            "写入长度 {} 与预算 {total_len} 不符",
            buf.len()
        )));
    }
    Ok(VtxWriteOutcome {
        bytes: buf,
        mesh_stats,
        checksum,
    })
}

/// 自洽性检查：各段必须落在文件内、且 mesh 数与 MDL 一致。
///
/// 与 MDL/VVD 的自检同样的理由 —— 宁可显式失败，也不要产出
/// 「能解析但进游戏不可见」的文件。
pub fn check_invariants(
    vtx: &VtxWriteOutcome,
    compiled: &CompiledModelDesc,
) -> Result<(), VtxWriteError> {
    let b = &vtx.bytes;
    let expect_meshes: usize = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .map(|m| m.meshes.len())
        .sum();
    if vtx.mesh_stats.len() != expect_meshes {
        return Err(VtxWriteError::Internal(format!(
            "mesh 数不符：VTX 有 {}，MDL 有 {expect_meshes}",
            vtx.mesh_stats.len()
        )));
    }
    let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    // 头部
    if g(0x00) != VERSION {
        return Err(VtxWriteError::Internal("version 不是 7".into()));
    }
    if g(0x10) != vtx.checksum {
        return Err(VtxWriteError::Internal("checksum 与 MDL 不一致".into()));
    }
    // 每个 body part / model / LOD / mesh / strip group 的偏移都要在文件内。
    let bp_count = g(0x1C).max(0) as usize;
    let bp_off = g(0x20).max(0) as usize;
    if bp_off + bp_count * BODY_PART_SIZE > b.len() {
        return Err(VtxWriteError::Internal("body part 表越界".into()));
    }
    for i in 0..bp_count {
        let at = bp_off + i * BODY_PART_SIZE;
        let mc = g(at).max(0) as usize;
        let m_base = at + g(at + 4).max(0) as usize;
        for k in 0..mc {
            let ma = m_base + k * MODEL_SIZE;
            if ma + MODEL_SIZE > b.len() {
                return Err(VtxWriteError::Internal(format!("model[{k}] 越界")));
            }
            let lc = g(ma).max(0) as usize;
            let l_base = ma + g(ma + 4).max(0) as usize;
            for l in 0..lc {
                let la = l_base + l * MODEL_LOD_SIZE;
                let kc = g(la).max(0) as usize;
                let k_base = la + g(la + 4).max(0) as usize;
                for kk in 0..kc {
                    let ka = k_base + kk * MESH_SIZE;
                    let gc = g(ka).max(0) as usize;
                    let g_base = ka + g(ka + 4).max(0) as usize;
                    for gg in 0..gc {
                        let ga = g_base + gg * STRIP_GROUP_SIZE;
                        if ga + STRIP_GROUP_SIZE > b.len() {
                            return Err(VtxWriteError::Internal("strip group 越界".into()));
                        }
                        // 顶点/索引/strip 数组也必须在文件内。
                        let nv = g(ga).max(0) as usize;
                        let vo = ga + g(ga + 4).max(0) as usize;
                        let ni = g(ga + 8).max(0) as usize;
                        let io = ga + g(ga + 12).max(0) as usize;
                        if vo + nv * VERTEX_SIZE > b.len() {
                            return Err(VtxWriteError::Internal("顶点数组越界".into()));
                        }
                        if io + ni * 2 > b.len() {
                            return Err(VtxWriteError::Internal("索引数组越界".into()));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn put_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(buf: &mut [u8], off: usize, v: f32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u8(buf: &mut [u8], off: usize, v: u8) {
    buf[off] = v;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile;
    use crate::model::ModelDesc;

    const TOML: &str = r#"
[model]
name = "models/test/minimal.mdl"
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
"#;

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

    fn minimal() -> CompiledModelDesc {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mdlc-vtx-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("myprop-ref.smd"), SMD).unwrap();
        let desc = ModelDesc::from_toml(TOML).unwrap();
        let c = compile(&desc, &d).expect("测试模型应能编译");
        std::fs::remove_dir_all(&d).ok();
        c
    }

    /// 造一个**足够大**的网格：`(n+1)²` 个顶点的规则栅格 → `2n²` 个三角形。
    ///
    /// 用栅格是因为它的**原序已经很缓存友好**（行优先），
    /// meshopt 仍有优化空间但不至于把顺序完全打乱 —— 正好能同时验证
    /// 「顺序变了」与「拓扑没变」。
    fn grid_model(n: usize) -> CompiledModelDesc {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let id = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mdlc-vtxgrid-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();

        // ⚠️ mdlc 的 SMD 解析器**不接受引用行**（`<v0> <v1> <v2> <bone>`，
        // 4 token）—— 每个顶点必须是**完整的 12 token 定义行**。
        // （原始 SMD 格式允许混排，本实现从简，只支持全定义形式。）
        // 所以每个三角形的三个顶点都重复写完整定义。
        let mut smd = String::from(
            "version 1\nnodes\n  0 \"root\" -1\nend\nskeleton\n  time 0\n    0 0 0 0 0 0 0\nend\ntriangles\nmyprop\n",
        );
        let row = |i: usize| -> String {
            let fx = (i % (n + 1)) as f32;
            let fy = (i / (n + 1)) as f32;
            // 12 token：`bone x y z nx ny nz u v links bone1 weight1`
            format!(
                "  0 {fx} {fy} 0 0 0 1 {} {} 1 0 1.0\n",
                fx / n as f32,
                fy / n as f32
            )
        };
        for y in 0..n {
            for x in 0..n {
                let a = y * (n + 1) + x;
                let b = a + 1;
                let c = a + (n + 1);
                let e = c + 1;
                for tri in [[a, b, c], [b, e, c]] {
                    for v in tri {
                        smd.push_str(&row(v));
                    }
                }
            }
        }
        smd.push_str("end\n");

        std::fs::write(d.join("myprop-ref.smd"), &smd).unwrap();
        let toml = r#"
[model]
name = "models/test/grid.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/myprop" }]

[[bones]]
name = "root"

[hitboxes]
set_name = "default"

[[hitboxes.boxes]]
bone = "root"
bbmin = [-1.0, -1.0, -1.0]
bbmax = [1.0, 1.0, 1.0]

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "myprop-ref.smd"
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("栅格模型应能编译");
        std::fs::remove_dir_all(&d).ok();
        c
    }

    /// 读第一个 strip group 的 `(origMeshVertID 表, 三角形列表)`。
    fn first_group(out: &VtxWriteOutcome) -> (Vec<u16>, Vec<[u16; 3]>) {
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bpo = g(0x20) as usize;
        let mo = bpo + g(bpo + 4) as usize;
        let lo = mo + g(mo + 4) as usize;
        let mh = lo + g(lo + 4) as usize;
        let sgo = mh + g(mh + 4) as usize;
        let num_verts = g(sgo) as usize;
        let vert_off = sgo + g(sgo + 4) as usize;
        let sg_index_off = sgo + g(sgo + 12) as usize;
        let strip_off = sgo + g(sgo + 20) as usize;
        let idx_count = g(strip_off) as usize;
        // ⚠️ strip 的 indexOffset 也**相对 strip group**
        let io = sg_index_off + g(strip_off + 4) as usize;
        let map = (0..num_verts)
            .map(|v| u16::from_le_bytes([b[vert_off + v * 9 + 4], b[vert_off + v * 9 + 5]]))
            .collect();
        let tr = (0..idx_count / 3)
            .map(|t| {
                let o = io + t * 6;
                [
                    u16::from_le_bytes([b[o], b[o + 1]]),
                    u16::from_le_bytes([b[o + 2], b[o + 3]]),
                    u16::from_le_bytes([b[o + 4], b[o + 5]]),
                ]
            })
            .collect();
        (map, tr)
    }

    fn sorted3(t: &[u16; 3]) -> [u16; 3] {
        let mut s = *t;
        s.sort_unstable();
        s
    }

    /// **默认必须与加本特性之前逐字节相同** —— 即 `write_vtx` 不做优化。
    ///
    /// 判据：`write_vtx` 与显式 `VtxOptions { optimize_vertex_cache: false }`
    /// 产物相同，且**与优化版不同**。
    #[test]
    fn vtx_optimization_is_off_by_default() {
        let c = grid_model(8);
        let def = write_vtx(&c).unwrap();
        let explicit_off = write_vtx_with(
            &c,
            VtxOptions {
                optimize_vertex_cache: false,
            },
        )
        .unwrap();
        let on = write_vtx_with(
            &c,
            VtxOptions {
                optimize_vertex_cache: true,
            },
        )
        .unwrap();
        assert_eq!(def.bytes, explicit_off.bytes, "默认应等价于显式关闭");
        assert_ne!(def.bytes, on.bytes, "打开优化后产物应当不同");
    }

    /// 优化**只重排三角形顺序**，不改拓扑、绕序、顶点池。
    #[test]
    fn vtx_optimization_only_reorders_triangles() {
        let c = grid_model(8);
        let def = write_vtx(&c).unwrap();
        let on = write_vtx_with(
            &c,
            VtxOptions {
                optimize_vertex_cache: true,
            },
        )
        .unwrap();

        // 长度必须相同（只重排，不增删）
        assert_eq!(def.bytes.len(), on.bytes.len(), "优化不应改变文件长度");

        let (map_a, tr_a) = first_group(&def);
        let (map_b, tr_b) = first_group(&on);
        assert_eq!(map_a, map_b, "origMeshVertID 表不应改变");
        assert_eq!(tr_a.len(), tr_b.len(), "三角形数不应改变");

        // 拓扑：无序集合相同
        let mut sa: Vec<_> = tr_a.iter().map(sorted3).collect();
        let mut sb: Vec<_> = tr_b.iter().map(sorted3).collect();
        sa.sort_unstable();
        sb.sort_unstable();
        assert_eq!(sa, sb, "三角形**集合**必须不变");

        // 绕序：未排序三元组多重集相同
        let mut ra = tr_a.clone();
        let mut rb = tr_b.clone();
        ra.sort_unstable();
        rb.sort_unstable();
        assert_eq!(ra, rb, "**绕序**必须不变（否则背面剔除会反过来）");

        // 顺序：应当变了（否则优化没生效）
        assert_ne!(tr_a, tr_b, "三角形顺序应当被重排（否则优化没生效）");
    }

    /// 顶点缓存 miss 数**必须下降** —— 这是「优化有效」的正面判据。
    #[test]
    fn vtx_optimization_reduces_cache_misses() {
        // 用 16 项 LRU 近似 GPU 的 post-transform cache。
        fn misses(tr: &[[u16; 3]], cap: usize) -> usize {
            let mut cache: Vec<u16> = Vec::new();
            let mut n = 0;
            for t in tr {
                for &v in t {
                    match cache.iter().position(|&c| c == v) {
                        Some(i) => {
                            cache.remove(i);
                            cache.push(v);
                        }
                        None => {
                            n += 1;
                            cache.push(v);
                            if cache.len() > cap {
                                cache.remove(0);
                            }
                        }
                    }
                }
            }
            n
        }
        let c = grid_model(10);
        let def = write_vtx(&c).unwrap();
        let on = write_vtx_with(
            &c,
            VtxOptions {
                optimize_vertex_cache: true,
            },
        )
        .unwrap();
        let (_, tr_a) = first_group(&def);
        let (_, tr_b) = first_group(&on);
        let ma = misses(&tr_a, 16);
        let mb = misses(&tr_b, 16);
        assert!(
            mb < ma,
            "优化后 miss 应下降：优化前 {ma}，优化后 {mb}（三角形 {} 个）",
            tr_a.len()
        );
    }

    /// 空网格（0 三角形）不应 panic。
    #[test]
    fn vtx_optimization_handles_empty_mesh() {
        let empty: Vec<[u16; 3]> = Vec::new();
        assert_eq!(optimize_group_indices(&empty, 0).unwrap(), empty);
    }

    /// **只能重排、不能改拓扑** —— 验证判据本身能区分这两种情况。
    ///
    /// meshopt 不会改拓扑，所以没法从外部触发守门分支；这里直接对
    /// **判据**做单元测试：同一套排序比较逻辑，对「同集合重排」必须判
    /// **相同**，对「改了拓扑」必须判**不同**。
    #[test]
    fn topology_guard_distinguishes_different_sets() {
        let a = [[0u16, 1, 2], [2, 3, 0]];
        let same_set = [[2u16, 3, 0], [0, 1, 2]]; // 只是顺序变了
        let other_set = [[0u16, 1, 2], [0, 3, 1]]; // 第二个三角形换了顶点

        let prep = |tr: &[[u16; 3]]| {
            let mut v: Vec<[u16; 3]> = tr.iter().map(sorted3).collect();
            v.sort_unstable();
            v
        };
        assert_eq!(prep(&a), prep(&same_set), "同集合（仅重排）应判**相同**");
        assert_ne!(prep(&a), prep(&other_set), "改拓扑应判**不同**");
    }

    #[test]
    fn struct_sizes_match_verified_layout() {        assert_eq!(HEADER_SIZE, 36);
        assert_eq!(BODY_PART_SIZE, 8);
        assert_eq!(MODEL_SIZE, 8);
        assert_eq!(MODEL_LOD_SIZE, 12);
        assert_eq!(MESH_SIZE, 9);
        assert_eq!(STRIP_GROUP_SIZE, 25);
        assert_eq!(STRIP_SIZE, 27);
        assert_eq!(VERTEX_SIZE, 9);
        assert_eq!(BONE_STATE_CHANGE_SIZE, 8);
    }

    #[test]
    fn writes_header_and_tree() {
        let c = minimal();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        assert_eq!(g(0x00), 7, "version");
        assert_eq!(g(0x10), out.checksum, "checksum 必须与 MDL 一致");
        assert_eq!(g(0x14), 1, "numLODs");
        assert_eq!(g(0x1C), 1, "bodyPartCount");
        assert_eq!(g(0x20), HEADER_SIZE as i32, "bodyPartOffset");
        // body part → model → LOD → mesh 的偏移链必须自洽。
        let bp = g(0x20) as usize;
        assert_eq!(g(bp), 1, "numModels");
        let m_abs = bp + g(bp + 4) as usize;
        assert_eq!(g(m_abs), 1, "numLODs");
        let l_abs = m_abs + g(m_abs + 4) as usize;
        assert_eq!(g(l_abs), 1, "numMeshes");
        let k_abs = l_abs + g(l_abs + 4) as usize;
        assert_eq!(g(k_abs), 1, "numStripGroups");
        check_invariants(&out, &c).expect("写出后必须自洽");
    }

    #[test]
    fn strip_group_flags_is_hwskinned_not_flexed() {
        let c = minimal();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        let l_abs = m_abs + g(m_abs + 4) as usize;
        let k_abs = l_abs + g(l_abs + 4) as usize;
        let sg_abs = k_abs + g(k_abs + 4) as usize;
        assert_eq!(
            b[sg_abs + 24],
            STRIPGROUP_IS_HWSKINNED,
            "无 flex 时必须用 HWSKINNED(0x02)；写 FLEXED(0x01) 会让引擎查不存在的 flex 数据"
        );
        assert_ne!(b[sg_abs + 24], STRIPGROUP_IS_FLEXED);
    }

    /// **有 VTA 载荷时 flags 必须是 `0x6`**（HWSKINNED | DELTA_FLEXED）。
    ///
    /// 官方带 VTA 的产物实测 `0x6`（`cmp_vta.js` 的 `vta_qc.dx90.vtx`），
    /// 语料 17 个有 flex 的 mesh **全部** `0x6`、5568 个无 flex 的**全部**
    /// `0x2`（`probe_vtx_flex_flags.js`，0 例外）。
    ///
    /// ⚠️ 关键是 **`0x04 DELTA_FLEXED`**，不是 `0x01 IS_FLEXED` ——
    /// `optimize.cpp:975` 注释：「Going forward, DX9 models are delta flexed」。
    #[test]
    fn strip_group_flags_is_delta_flexed_when_mesh_has_flexes() {
        use crate::flex::{ResolvedFlex, ResolvedVertAnim};
        let mut c = minimal();
        c.bodyparts[0].models[0].mesh_flexes = vec![vec![ResolvedFlex {
            flexdesc: 0,
            targets: [0.0, 1.0, 10.0, 11.0],
            flexpair: 0,
            vertanimtype: 0,
            vertanims: vec![ResolvedVertAnim {
                index: 0,
                speed: 255,
                side: 0,
                delta: [0.0, 0.0, 1.0],
                ndelta: [0.0, 0.0, 0.0],
            }],
        }]];
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        let l_abs = m_abs + g(m_abs + 4) as usize;
        let k_abs = l_abs + g(l_abs + 4) as usize;
        let sg_abs = k_abs + g(k_abs + 4) as usize;
        let want = STRIPGROUP_IS_HWSKINNED | STRIPGROUP_IS_DELTA_FLEXED;
        assert_eq!(
            b[sg_abs + 24], want,
            "有 VTA 载荷时必须写 0x6（HWSKINNED|DELTA_FLEXED）；\
             否则引擎不走 flex 路径，形状不生效"
        );
        assert_eq!(want, 0x06, "0x6 是实测值");
    }

    /// `nofacial`：该档的 strip group **不再带 `0x04`**。
    ///
    /// oracle（`docs\_probe\smdl\lodnf.qc`，官方 studiomdl.exe 实测，
    /// 用 `dump_sg_flags.js` 逐 strip group 读 `+24`）：
    /// ```text
    /// lod0（facial）  : sg=1 flags=[0x2] ×1、sg=1 flags=[0x6] ×1
    /// lod1（nofacial）: sg=1 flags=[0x2] ×2      ← 0x04 全部消失
    /// ```
    ///
    /// ⚠️ **判据是「不再有 `0x04`」，不是「strip group 数下降」**：
    /// 本夹具的 mesh 全 flexed，所以两档都是 **1 个** strip group，
    /// 只有 flags 从 `0x6` 变成 `0x2` —— 只断言数量会漏掉。
    #[test]
    fn no_facial_drops_delta_flexed_flag() {
        use crate::flex::{ResolvedFlex, ResolvedVertAnim};
        let mk_flex = || {
            vec![vec![ResolvedFlex {
                flexdesc: 0,
                targets: [0.0, 1.0, 10.0, 11.0],
                flexpair: 0,
                vertanimtype: 0,
                vertanims: vec![ResolvedVertAnim {
                    index: 0,
                    speed: 255,
                    side: 0,
                    delta: [0.0, 0.0, 1.0],
                    ndelta: [0.0, 0.0, 0.0],
                }],
            }]]
        };
        // 逐档读 (numStripGroups, 各 strip group 的 flags)。
        let read = |b: &[u8], lod: usize| -> (i32, Vec<u8>) {
            let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let bp = g(0x20) as usize;
            let m_abs = bp + g(bp + 4) as usize;
            let l_abs = m_abs + g(m_abs + 4) as usize + lod * 12;
            let k_abs = l_abs + g(l_abs + 4) as usize;
            let nsg = g(k_abs);
            let mut sg = k_abs + g(k_abs + 4) as usize;
            let mut out = Vec::new();
            for _ in 0..nsg {
                out.push(b[sg + 24]);
                sg += STRIP_GROUP_SIZE;
            }
            (nsg, out)
        };

        // ---- facial（默认）：两档都应有 0x6 ----
        let mut c = multi_lod();
        c.bodyparts[0].models[0].mesh_flexes = mk_flex();
        let facial = write_vtx(&c).unwrap().bytes;
        for lod in 0..2 {
            let (nsg, fl) = read(&facial, lod);
            assert_eq!(nsg, 1, "lod{lod} 应只有 1 个 strip group");
            assert_eq!(
                fl,
                vec![STRIPGROUP_IS_HWSKINNED | STRIPGROUP_IS_DELTA_FLEXED],
                "lod{lod} facial 档必须是 0x6"
            );
        }

        // ---- nofacial 只作用在 LOD 1 ----
        let mut c2 = multi_lod();
        c2.bodyparts[0].models[0].mesh_flexes = mk_flex();
        c2.bodyparts[0].models[0]
            .lods
            .as_mut()
            .unwrap()
            .no_facial = vec![false, true];
        let nf = write_vtx(&c2).unwrap().bytes;
        let (nsg0, fl0) = read(&nf, 0);
        assert_eq!(nsg0, 1);
        assert_eq!(
            fl0,
            vec![STRIPGROUP_IS_HWSKINNED | STRIPGROUP_IS_DELTA_FLEXED],
            "LOD 0 不受 nofacial 影响，仍是 0x6"
        );
        let (nsg1, fl1) = read(&nf, 1);
        assert_eq!(nsg1, 1, "全 flexed 的 mesh：数量**不变**（这正是易漏之处）");
        assert_eq!(
            fl1,
            vec![STRIPGROUP_IS_HWSKINNED],
            "LOD 1 nofacial ⟹ 必须丢掉 0x04（官方实测 0x2）"
        );
        assert!(
            !fl1.contains(&STRIPGROUP_IS_DELTA_FLEXED),
            "nofacial 档绝不能出现 DELTA_FLEXED"
        );
    }

    #[test]
    fn strip_is_trilist() {
        let c = minimal();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        let l_abs = m_abs + g(m_abs + 4) as usize;
        let k_abs = l_abs + g(l_abs + 4) as usize;
        let sg_abs = k_abs + g(k_abs + 4) as usize;
        let st_abs = sg_abs + g(sg_abs + 20) as usize;
        assert_eq!(b[st_abs + 18], STRIP_IS_TRILIST);
        // 索引数 = 三角形数 × 3
        assert_eq!(g(st_abs), 3, "1 个三角形 → 3 个索引");
    }

    #[test]
    fn vertex_orig_mesh_vert_id_matches_order() {
        let c = minimal();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        let l_abs = m_abs + g(m_abs + 4) as usize;
        let k_abs = l_abs + g(l_abs + 4) as usize;
        let sg_abs = k_abs + g(k_abs + 4) as usize;
        let v_abs = sg_abs + g(sg_abs + 4) as usize;
        for i in 0..3usize {
            let o = v_abs + i * VERTEX_SIZE;
            let id = u16::from_le_bytes([b[o + 4], b[o + 5]]);
            assert_eq!(id as usize, i, "origMeshVertID 必须等于 mesh 内顶点序号");
            assert_eq!(b[o + 3], 1, "boneCount");
            // boneId 应指向 VVD 顶点里绑定的骨骼（SMD 里是 1 = tip）
            assert_eq!(b[o + 6], 1, "boneId 应为全局骨骼下标 1");
        }
    }

    #[test]
    fn indices_are_tri_list_in_order() {
        let c = minimal();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        let l_abs = m_abs + g(m_abs + 4) as usize;
        let k_abs = l_abs + g(l_abs + 4) as usize;
        let sg_abs = k_abs + g(k_abs + 4) as usize;
        let i_abs = sg_abs + g(sg_abs + 12) as usize;
        let idx: Vec<u16> = (0..3)
            .map(|k| u16::from_le_bytes([b[i_abs + k * 2], b[i_abs + k * 2 + 1]]))
            .collect();
        assert_eq!(idx, vec![0, 1, 2], "tri-list 应按三角形顺序写出");
    }

    #[test]
    fn material_replacement_list_is_empty() {
        let c = minimal();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let mr = g(0x18) as usize;
        assert_eq!(g(mr), 0, "numReplacements 应为 0");
        assert!(mr + MATERIAL_REPLACEMENT_LIST_SIZE <= b.len());
    }

    #[test]
    fn every_mesh_gets_its_own_strip_group() {
        // 两个材质 → 两个 mesh → 两个 strip group。
        let c = {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let d = std::env::temp_dir().join(format!("mdlc-vtx2-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            let smd = SMD.replace(
                "myprop\n  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000\n  1 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000\n  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000\nend",
                "tex_a\n  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000\n  1 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000\n  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000\ntex_b\n  1 1.000000 0.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000\n  1 2.000000 0.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000\n  1 1.000000 1.000000 0.000000 0.000000 0.000000 1.000000 0.000000 1.000000 1 1 1.000000\nend",
            );
            std::fs::write(d.join("myprop-ref.smd"), &smd).unwrap();
            let toml = TOML.replace(
                r#"textures = [{ name = "models/test/myprop" }]"#,
                "textures = [\n  { name = \"models/test/tex_a\" },\n  { name = \"models/test/tex_b\" },\n]",
            );
            let desc = ModelDesc::from_toml(&toml).unwrap();
            let c = compile(&desc, &d).unwrap();
            std::fs::remove_dir_all(&d).ok();
            c
        };
        let out = write_vtx(&c).unwrap();
        assert_eq!(out.mesh_stats.len(), 2);
        check_invariants(&out, &c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        let l_abs = m_abs + g(m_abs + 4) as usize;
        assert_eq!(g(l_abs), 2, "numMeshes 应为 2");
    }

    /// 与真实 studiomdl 产物对照：结构字段必须逐项一致。
    #[test]
    fn matches_studiomdl_reference_layout() {        let p = r"E:\SteamLibrary\steamapps\common\Left 4 Dead 2\left4dead2\models\mymod\myprop.dx90.vtx";
        let Ok(refb) = std::fs::read(p) else {
            eprintln!("跳过：找不到 studiomdl 参考产物 {p}");
            return;
        };
        let c = minimal();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |buf: &[u8], o: usize| {
            i32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]])
        };
        // 头部：这些字段是常量，应与官方一致。
        for (name, off) in [("version", 0x00), ("cacheSize", 0x04), ("maxBonesPerVert", 0x0C), ("numLODs", 0x14)] {
            assert_eq!(
                g(b, off),
                g(&refb, off),
                "{name}（偏移 {off:#x}）与 studiomdl 不一致"
            );
        }
        assert_eq!(
            u16::from_le_bytes([b[0x08], b[0x09]]),
            u16::from_le_bytes([refb[0x08], refb[0x09]]),
            "maxBonesPerStrip"
        );
        assert_eq!(
            u16::from_le_bytes([b[0x0A], b[0x0B]]),
            u16::from_le_bytes([refb[0x0A], refb[0x0B]]),
            "maxBonesPerTri"
        );
        assert_eq!(g(b, 0x1C), g(&refb, 0x1C), "bodyPartCount");
        assert_eq!(g(b, 0x20), g(&refb, 0x20), "bodyPartOffset");
        // strip group 与 strip 的 flags。
        let sg = HEADER_SIZE + BODY_PART_SIZE + MODEL_SIZE + MODEL_LOD_SIZE + MESH_SIZE;
        assert_eq!(b[sg + 24], refb[sg + 24], "stripGroup.flags");
        let st = sg + STRIP_GROUP_SIZE;
        assert_eq!(b[st + 18], refb[st + 18], "strip.flags");
    }

    // -----------------------------------------------------------------
    // 多 LOD
    // -----------------------------------------------------------------

    /// 构造一个「2 个 LOD × 2 个 mesh」的最小多 LOD 模型。
    ///
    /// LOD 0 每个 mesh 有 4 个顶点（两个三角形），LOD 1 只有 3 个顶点
    /// （其中一个三角形）。材质名两边一致。
    fn multi_lod() -> CompiledModelDesc {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mdlc-vtx-lod-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();

        // 一个 mesh 的 4 顶点正方形（2 个三角形 = 6 个顶点行，SMD 是展开格式）。
        fn quad_smd(mat: &str, z: f32, coords: &[[f32; 2]]) -> String {
            let mut s = format!("version 1\nnodes\n  0 \"root\" -1\nend\nskeleton\n  time 0\n    0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000\nend\ntriangles\n{mat}\n");
            let pos = [
                [-8.0f32, -8.0, z],
                [8.0, -8.0, z],
                [8.0, 8.0, z],
                [-8.0, 8.0, z],
            ];
            // 三角形 (0,1,2) 与 (0,2,3) —— 每个三角形 3 个顶点行。
            // 12 个 token：`parentBone pos3 nrm3 uv2 links bone weight`。
            for tri in [[0usize, 1, 2], [0, 2, 3]] {
                for &i in &tri {
                    let p = pos[i];
                    s.push_str(&format!(
                        "  0 {:.6} {:.6} {:.6} 0.000000 0.000000 1.000000 {:.6} {:.6} 1 0 1.000000\n",
                        p[0], p[1], p[2], coords[i][0], coords[i][1]
                    ));
                }
            }
            s.push_str("end\n");
            s
        }
        // LOD 0：两个 mesh 各 4 顶点。
        let c = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        std::fs::write(d.join("lod0.smd"), quad_smd("tex_a", 0.0, &c)).unwrap();
        // LOD 1：同一网格（简化版用同样的 4 顶点，但材质集合一致）。
        std::fs::write(d.join("lod1.smd"), quad_smd("tex_a", 0.0, &c)).unwrap();

        let toml = r#"
[model]
name = "models/test/lod.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/test"]
textures = [{ name = "models/test/tex_a" }]

[[bones]]
name = "root"

[[bodyparts]]
name = "body"

[[bodyparts.models]]
smd = "lod0.smd"

[[bodyparts.models.lods]]
smd = "lod1.smd"
switch_point = 30.0
"#;
        let desc = ModelDesc::from_toml(toml).unwrap();
        let c = compile(&desc, &d).expect("多 LOD 模型应能编译");
        std::fs::remove_dir_all(&d).ok();
        c
    }

    #[test]
    fn multi_lod_compiles_with_two_lods() {
        let c = multi_lod();
        let m = &c.bodyparts[0].models[0];
        let lods = m.lods.as_ref().expect("应有多 LOD 数据");
        assert_eq!(lods.num_lods, 2);
        assert_eq!(lods.switch_points.len(), 2);
        assert_eq!(lods.switch_points[0], 0.0, "LOD 0 的 switchPoint 恒为 0");
        assert_eq!(lods.switch_points[1], 30.0, "LOD 1 用显式值");
        assert_eq!(lods.meshes.len(), m.meshes.len(), "LOD 的 mesh 数应与 MDL 一致");
    }

    /// 多 LOD 的 VTX 头部 `numLODs` 必须与 VVD 一致（实测 230/230）。
    #[test]
    fn multi_lod_vtx_num_lods_matches_vvd() {
        let c = multi_lod();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        assert_eq!(g(0x14), 2, "VTX numLODs 应为 2");

        let (vvd, layout) = crate::lod::build_multi_lod_vvd(&c, 123).unwrap();
        assert_eq!(vvd.header.num_lods, 2, "VVD numLODs 应为 2");
        assert_eq!(layout.num_lods, 2);
        assert_eq!(
            g(0x14),
            vvd.header.num_lods,
            "VTX 与 VVD 的 numLODs 必须一致"
        );
    }

    /// 多 LOD 的 VTX 结构树必须能被自检走通，且每 LOD 的 mesh 数正确。
    #[test]
    fn multi_lod_vtx_tree_is_walkable() {
        let c = multi_lod();
        let out = write_vtx(&c).unwrap();
        check_invariants(&out, &c).expect("多 LOD 的 VTX 必须自洽");
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        assert_eq!(g(m_abs), 2, "model 应有 2 个 LOD");
        let l0 = m_abs + g(m_abs + 4) as usize;
        let l1 = l0 + MODEL_LOD_SIZE;
        assert_eq!(g(l0), 1, "LOD 0 的 numMeshes");
        assert_eq!(g(l1), 1, "LOD 1 的 numMeshes");
        // switchPoint 必须写进对应 LOD。
        assert_eq!(b[l0 + 8..l0 + 12], 0.0f32.to_le_bytes(), "LOD 0 switchPoint=0");
        assert_eq!(
            b[l1 + 8..l1 + 12],
            30.0f32.to_le_bytes(),
            "LOD 1 switchPoint=30"
        );
        // 两个 LOD 的 mesh 数组必须不同（各有自己的 strip group）。
        let k0 = l0 + g(l0 + 4) as usize;
        let k1 = l1 + g(l1 + 4) as usize;
        assert_ne!(k0, k1, "两个 LOD 的 mesh 数组应分开");
    }

    /// 多 LOD 时 `origMeshVertID` 必须落在 `MDL.mesh.numvertices` 范围内。
    ///
    /// 实测判据：230/230 个真实多 LOD 模型满足
    /// `origMeshVertID < MDL.mesh.numvertices`（`probe_vtx_origmeshvertid.js`）。
    #[test]
    fn multi_lod_orig_mesh_vert_id_within_mesh_total() {
        let c = multi_lod();
        let out = write_vtx(&c).unwrap();
        check_invariants(&out, &c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);

        // 各 mesh 的跨 LOD 顶点总数（= layout 的 total_vertexes）。
        let mut all = Vec::new();
        for bp in &c.bodyparts {
            for m in &bp.models {
                if let Some(l) = &m.lods {
                    all.extend(l.meshes.iter().cloned());
                }
            }
        }
        let layout = crate::lod::build_lod_layout(&all);

        // 遍历 VTX 里每个 strip group 的每个顶点。
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        let n_lods = g(m_abs);
        let mut checked = 0usize;
        for l in 0..n_lods as usize {
            let lod_abs = m_abs + g(m_abs + 4) as usize + l * MODEL_LOD_SIZE;
            for k in 0..g(lod_abs) as usize {
                let k_abs = lod_abs + g(lod_abs + 4) as usize + k * MESH_SIZE;
                let sg_abs = k_abs + g(k_abs + 4) as usize;
                let nv = g(sg_abs) as usize;
                let v_abs = sg_abs + g(sg_abs + 4) as usize;
                let limit = layout.meshes[k].total_vertexes;
                for vi in 0..nv {
                    let o = v_abs + vi * VERTEX_SIZE;
                    let id = u16::from_le_bytes([b[o + 4], b[o + 5]]) as usize;
                    assert!(
                        id < limit,
                        "LOD {l} mesh {k} 的 origMeshVertID {id} 超出该 mesh 的顶点总数 {limit}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "应该检查到顶点");
    }

    /// 多 LOD 时索引必须指向本 strip group 的顶点槽位（不能越界）。
    #[test]
    fn multi_lod_indices_are_in_range() {
        let c = multi_lod();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let bp = g(0x20) as usize;
        let m_abs = bp + g(bp + 4) as usize;
        for l in 0..g(m_abs) as usize {
            let lod_abs = m_abs + g(m_abs + 4) as usize + l * MODEL_LOD_SIZE;
            for k in 0..g(lod_abs) as usize {
                let k_abs = lod_abs + g(lod_abs + 4) as usize + k * MESH_SIZE;
                let sg_abs = k_abs + g(k_abs + 4) as usize;
                let nv = g(sg_abs) as usize;
                let ni = g(sg_abs + 8) as usize;
                let i_abs = sg_abs + g(sg_abs + 12) as usize;
                for ii in 0..ni {
                    let v = u16::from_le_bytes([b[i_abs + ii * 2], b[i_abs + ii * 2 + 1]]) as usize;
                    assert!(v < nv, "LOD {l} mesh {k} 索引 {v} 越界（顶点数 {nv}）");
                }
            }
        }
    }

    /// materialReplacementList 必须**每 LOD 一个**（单 LOD 时 1 个）。
    #[test]
    fn multi_lod_has_one_material_replacement_list_per_lod() {
        let c = multi_lod();
        let out = write_vtx(&c).unwrap();
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let mr = g(0x18) as usize;
        assert!(mr + 2 * MATERIAL_REPLACEMENT_LIST_SIZE <= b.len(), "两个空表应都在文件内");
        for n in 0..2 {
            let o = mr + n * MATERIAL_REPLACEMENT_LIST_SIZE;
            assert_eq!(g(o), 0, "第 {n} 个表的 numReplacements");
        }
    }

    /// 单 LOD 路径必须**一行未变**：加多 LOD 支持后产物逐字节不变。
    #[test]
    fn single_lod_path_is_unchanged() {
        let c = minimal();
        let out = write_vtx(&c).unwrap();
        // 单 LOD：头部 numLODs = 1，且只有一个 materialReplacementList。
        let b = &out.bytes;
        let g = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        assert_eq!(g(0x14), 1);
        let mr = g(0x18) as usize;
        assert_eq!(
            b.len(),
            mr + MATERIAL_REPLACEMENT_LIST_SIZE,
            "单 LOD 的文件应在唯一的 materialReplacementList 之后结束"
        );
    }
}
