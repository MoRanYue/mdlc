//! PHY 碰撞文件写出（Source 引擎 `.phy`，MDL v49 / L4D2）。
//!
//! # 这个模块解决什么问题
//!
//! `.mdl` / `.vvd` / `.vtx` 三元组只让模型**看得见**；要让它在游戏里
//! **能被撞到、能被推动**，还需要一个 `.phy`。没有它，物件会变成
//! 穿模的空气，ragdoll 会散架。
//!
//! # 权威来源
//!
//! 布局全部来自 `docs/phy-format-research.md` 那份 884 行的实测报告
//! （331 个真实 `.phy` / 774 个 solid 逐字节反推，关键不变量 774/774
//! 或 2051/2051 命中）。本模块是那份报告 §10.1 伪代码的 Rust 版本，
//! **不做任何"优化"**——凡是实测精确成立的地方都照抄。
//!
//! # 磁盘布局
//!
//! ```text
//! +0      phyheader_t                    16 B  { size=16, id=0, solidCount, checksum }
//! +16     solid[0]                       32 B 头 + IVP_Compact_Surface(48) + ledge 区 + 树
//!         solid[1]                       stride = size + 4 = surfaceSize + 32
//!         ...
//!         text section                   KeyValues 文本，末尾 1 个 0x00
//! ```
//!
//! # 五个必须记住的硬约束（错了就一定加载失败或被校验器抓到）
//!
//! | # | 约束 | 踩错的后果 |
//! |---|---|---|
//! | 1 | `surfaceSize == 48 + ledge_region_size + 28 × node_count` | solid 步长错位，后续 solid 全废 |
//! | 2 | `offset_ledgetree_root` 相对 **`IVP_Compact_Surface` 起点** | 树指针落在别处 |
//! | 3 | `offset_compact_ledge` 相对 **节点自身**，值 = `48 − offset_ledgetree_root` | 见下 |
//! | 4 | `c_point_offset` 指向 **solid 级共享点数组的起点** | 顶点全读错 |
//! | 5 | `opposite_index` 是 **有符号 15 位、以 4 字节为单位、相对自身** | 邻接全错，碰撞退化 |
//!
//! ## 头号陷阱：`offset_compact_ledge` 的基准
//!
//! 它是**相对节点自身**的，不是相对 `IVP_Compact_Surface` 的。节点在
//! `body + offset_ledgetree_root`，ledge 在 `body + 48`，所以差值是
//! `48 − offset_ledgetree_root`（**恒为负**，因为 ledge 在低地址）。
//!
//! 第一版实现写成 `−offset_ledgetree_root`，校验器立刻报 `c_point_offset=0`、
//! `nTri=0`——因为 ledge 指针落在了 surface 头上。实测对照：`urban_puddle`
//! 的 `offset_ledgetree_root = 480`，该字段就是 `−432`。
//!
//! ## 规格里的一处笔误（本实现按实测走）
//!
//! §4.2 与 §10.1 都写 `c_point_offset == 16 + 16 × n_triangles`，并标注
//! "774/774"。**这只对单 hull 的 solid 成立。** 真实布局是：所有 hull 的
//! ledge 头+三角形紧密排列在前，**整个 solid 共用一份点数组**跟在后面，
//! 每个 hull 的 `c_point_offset` 都指向那份共享数组的**起点**，
//! 三角形的 `start_point_index` 是共享数组里的全局下标。
//!
//! `c_point_offset` 的精确规则是**后缀和**：
//!
//! ```text
//! c_point_offset(i) = Σ_{j >= i} (16 + 16 × nTri_j)
//! ```
//!
//! 也就是「从本 ledge 起、到最后一个 ledge 结束为止的所有 ledge 大小之和」，
//! 恰好等于本 ledge 到共享点数组起点的距离。实测复核（331 文件 / 774 solid）：
//!
//! ```text
//! hulls checked=2204   multi-hull solids=153
//! c_point_offset == 16 + 16*nTri :  774/2204   ← 只有单 hull 的那 774 个
//! c_point_offset -> 共享数组起点  : 2204/2204   ← 全部命中
//! c_point_offset == 后缀和        : 1583/1583   ← 全部多 hull 的 hull 都命中
//! ```
//!
//! 单 hull 时后缀和退化成 `16 + 16×nTri`，所以 §10.1 的伪代码在它的适用
//! 范围内没出错 —— 报告抽样的 774 个恰好全是单 hull 的 solid。
//!
//! ## 关于 `size_div_16` 的一个推论
//!
//! 实测规则是 `size_div_16 == 1 + n_triangles + 该 hull 引用的点数`。
//! 在共享点数组下，"该 hull 引用的点数"是它 `start_point_index` 的
//! **去重计数**，而不是共享数组的总长度——多 hull 时两者不同。本实现按前者写。
//!
//! # 碰撞树：什么时候写多节点
//!
//! 一个 solid 只有一个凸块时写**单叶子节点树**（`offset_right_node = 0`），
//! 这正是 L4D2 里 621/774 个 solid 的形态，也是所有 ragdoll 的形态。
//!
//! 一个 solid 由多个凸块拼成时，必须写成**递归树**（每个叶子挂一个 hull），
//! 否则多出来的 hull 根本不被引用、等于没写。本实现按
//! 「左子恒为 `this + 28`、右子为 `this + offset_right_node`」的隐式布局
//! 递归排布节点，节点总数 `2n − 1`（恒为奇数，与实测一致）。
//!
//! 内部节点的 `center`/`radius`/`box_sizes` 精确算法**未能复现**
//! （实测最大半径误差 3.2e-2、`box_sizes` 差 1 格），按报告 §5 给实现者的
//! 建议用"子树所有点的 AABB + 外接球"。这些值只用于加速剔除，
//! **不影响碰撞正确性**。
//!
//! # 不自己做的两件事
//!
//! 1. **不自己做凸分解。** 输入要求每个凸块已经切好（与 studiomdl 一致——
//!    它也要求 QC 给的就是凸块）。凹网格的拆分请用 [`decompose_concave`]
//!    （parry3d 的 VHACD）。
//! 2. **不自己写凸包算法。** 用 parry3d 的 quickhull（见 [`convex_hull_of`]）。
//!
//! # 与 §10.2 的偏离：`rotation_inertia` 用真实值而不是 `[1,1,1]`
//!
//! §10.2 建议保守填 `[1,1,1]`。我实测了 774 个真实 solid：`[1,1,1]` 与真实值
//! 的比值跨度达 **6.8 倍**（q10=0.0011，q90=0.0390），而用 hull 自身的
//! 均匀密度惯性张量（parry3d 的 [`MassProperties`]）与真实值的比值跨度只有
//! **1.3 倍**（q10=0.709，q90=0.948，中位 0.77）。
//!
//! 既然已经引入了 parry3d，就没有理由填一个更差的值。写入的物理量是
//! **无量纲**的：IVP 用的是 `inv_inertia`，会连同 `inv_mass` 一起按
//! `mass × inertia` 缩放，所以单位取 kg·m² 还是别的都无所谓，
//! **只有三个主轴之间的比例有意义**。因此这里写 parry 在**密度 1** 下算出的
//! 惯性张量对角元（`I`，不是 `I/m`），既保留正确的各向异性比例，
//! 又与真实文件的量级一致。
//!
//! 需要绝对保守时把 [`PhyParams::inertia_scale`] 设成 `0.0`，三个分量就都是 0。

use std::collections::HashMap;

use parry3d::mass_properties::MassProperties;
use parry3d::math::Vector;
use parry3d::transformation::vhacd::{VHACD, VHACDParameters};

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// `phyheader_t` 的字节大小（也是 `phyheader_t::size` 的恒定取值）。
pub const PHY_HEADER_SIZE: usize = 16;
/// 每个 solid 记录里、`IVP_Compact_Surface` 之前的头字节数。
pub const SOLID_HEADER_SIZE: usize = 32;
/// `IVP_Compact_Surface` 的字节大小。
pub const COMPACT_SURFACE_SIZE: usize = 48;
/// `IVP_Compact_Ledge` 的字节大小。
pub const LEDGE_HEADER_SIZE: usize = 16;
/// `IVP_Compact_Triangle` 的字节大小。
pub const TRIANGLE_SIZE: usize = 16;
/// `IVP_Compact_Poly_Point` 的字节大小。
pub const POINT_SIZE: usize = 16;
/// `IVP_Compact_Ledgetree_Node` 的字节大小。
pub const LEDGETREE_NODE_SIZE: usize = 28;

/// **Source 单位（inch）→ IVP 内部单位（米）的换算系数**。
///
/// # 为什么 `.phy` 的点不是模型局部坐标
///
/// `.mdl` / `.vvd` / `.vtx` / SMD **全部**用 Source 单位（inch），
/// 但 `.phy` **不是** —— 它由 `vphysics.dll` 的 `CollideWrite`
/// （`collisionmodel.cpp:2345`）序列化，而 vphysics 内部是 **IVP**，
/// 用的是**米**。
///
/// # 判据（受控实验 + 语料全量）
///
/// **受控实验**（`msh1`，已知 50×10×50 inch 的长方体）：
///
/// ```text
/// studiomdl 报告的体积：17000 in^3
/// 官方 .phy 的点算出的体积：0.2786
/// 立方根(17000 / 0.2786) = 39.3691
/// 1 / 0.0254             = 39.3701        ← inch → meter
/// ```
///
/// 官方 `msh1.phy` 的点 bbox 实测
/// `[-0.127, -1.143, -0.127] .. [0.127, 0.127, 1.143]`
/// —— 而源是 `[-5,-5,-5] .. [45,5,45]`。各轴范围
/// `0.254 / 1.27 / 1.27` = `10×0.0254 / 50×0.0254 / 50×0.0254`，逐轴吻合。
///
/// **语料全量**（`probe_phy_unit.js`，1316 个可算的官方 `.phy`）：
/// `mdl hull 最大边 / phy 点最大边` 的中位数 **42.65**、
/// 落在 `39.37±25%` 内的 **66.9%**，而落在 `1.00±0.25` 内的 **0.0%**。
/// （比值不精确等于 39.37 是因为 `.mdl` 的 hull 来自**渲染**网格的顶点 AABB、
/// `.phy` 来自**碰撞**网格 —— 两者不是同一份几何。但量纲差 39 倍是
/// **数量级**差异，不会被几何差异淹没。）
///
/// # 这个 bug 为什么能藏这么久
///
/// `check_invariants` 查的是**布局自洽**（点数组长度、索引范围、
/// `c_point_offset` 后缀和、`surfaceSize == 48 + ledge + 28×nodes）。
/// 把点整体乘 39.37 **不破坏任何一条** —— 长度不变、索引不变、
/// 体积仍是正的。所以它**不会报错**，只会在游戏里表现为
/// 「碰撞体比模型大 39 倍」。
///
/// 同理，`phy_report.js` 报的「Σvolume」也只是跟着变大，
/// 而 `tor1` 的「22728.12 vs 22728.16」那种对比**恰好没暴露它** ——
/// 因为两边都是同一个实现的输出（官方 artifact vs mdlc），
/// 而当时代码里根本没有单位换算，比值又恰好接近 1……
///
/// > 唯一能抓到它的是**与官方产物逐点比对**。受控实验（已知尺寸的
/// > 立方体）是最快的路径：体积的立方根直接给出系数。
pub const SOURCE_TO_IVP: f32 = 0.0254;

/// 把 **Source 世界空间**（inch）的点换成 IVP 世界空间（米）。
///
/// # 这不是单纯的单位换算
///
/// 反编译 `vphysics.dll` 的 `BuildConvexFromVerts`（`0x10083000`，
/// `ConvexFromVerts` = vtable[1] = `0x10083230` 的第一段）得到逐字对应：
///
/// ```c
/// pfVar3 = malloc(0x10);
/// pfVar2 = verts[i];
/// fVar1  = pfVar2[1];
/// pfVar3[0] =  pfVar2[0] * DAT_1017d190;   //  +x
/// pfVar3[1] = -pfVar2[2] * DAT_1017d190;   //  −z
/// pfVar3[2] =  DAT_1017d190 * fVar1;       //  +y
/// ```
///
/// `DAT_1017d190` = `0x3cd013a9` = **0.0254**（inch → 米）。
/// 所以轴映射是 **`out = (−y, −z, x)`**，而不是恒等。
///
/// # 这个轴映射是怎么被独立证实的
///
/// 三个互相独立的实验：
///
/// 1. **`physign`**（`docs/_probe/gen_physign.js`）—— 一个**完全不对称**的
///    四面体 `(3,−1,−2) (−4,6,−1) (1,2,7) (−2,−5,3)`，序列姿态全 0。
///    24 种纯旋转给出的顶点集合**两两不同**，所以能唯一定案 ——
///    结果只有 `(-y,-z,x)` 吻合（`probe_physign.js`）。
/// 2. **`phyrot0/1/2`** —— 几何完全相同（同一个 40×20×60 盒子）、
///    只有**序列动画**的骨骼姿态不同。`.phy` 三者不同而 `.vvd` 三者相同
///    ⟹ 姿态确实来自**序列**（`g_panimation[0]`），不是碰撞 SMD 自己的姿态。
/// 3. **mikuw** —— 差分实测得到 `(z,−y,x)`，而它的序列姿态是 `Rx(90°)`，
///    且 `(-y,−z,x) ∘ Rx(90°) = (z,−y,x)`。**完全吻合。**
///
/// 全量验证见 `docs/_probe/probe_phy_verify.js`：14 个模型，
/// 13 个官方点 100% 命中（残差 < 1e-5），mikuw 38/40（中位残差 9.7e-8）。
///
/// # 为什么不能只写 `× 0.0254`
///
/// 少了这个旋转，碰撞体会相对渲染网格**转 90°** —— 玩家会撞到空气、
/// 穿过看得见的模型。`check_invariants` 抓不到：旋转不改变任何
/// 布局自洽性（长度、索引、体积都照旧合法）。
#[inline]
pub fn source_to_ivp_axis(v: [f32; 3]) -> [f32; 3] {
    [-v[1], -v[2], v[0]]
}

/// [`source_to_ivp_axis`] 之后再乘 [`SOURCE_TO_IVP`]。
#[inline]
pub fn to_ivp(v: [f32; 3]) -> [f32; 3] {
    let a = source_to_ivp_axis(v);
    [
        a[0] * SOURCE_TO_IVP,
        a[1] * SOURCE_TO_IVP,
        a[2] * SOURCE_TO_IVP,
    ]
}

/// `VPHYSICS_COLLISION_ID` = `MAKEID('V','P','H','Y')`。
pub const VPHYSICS_ID: u32 = 0x5948_5056;
/// `IVP_COMPACT_SURFACE_ID` = `MAKEID('I','V','P','S')`，写在 `dummy[2]`。
pub const IVP_COMPACT_SURFACE_ID: u32 = 0x5350_5649;
/// `VPHYSICS_COLLISION_VERSION`。
///
/// **不是** Ipion 的 `IVPS_VERSION`——那个在 `.phy` 里根本不存在，
/// IVP 的版本标识是 `dummy[2]` 里的 magic `'IVPS'`。
pub const VPHYSICS_COLLISION_VERSION: u16 = 0x0100;
/// `COLLIDE_POLY`。L4D2 的 774 个 solid 全是它。
pub const COLLIDE_POLY: i16 = 0;

/// `IVP_COMPACT_BOUNDINGBOX_STEP_SIZE` 的倒数：`box_sizes` 的量化网格步长。
const BOUNDINGBOX_STEP: f64 = 1.0 / 250.0;

/// 单个 hull 能容纳的最大三角形数。
///
/// 边 `(k,i)` 的线性编号是 `4k+1+i`，取值范围 `[1, 4·nTri−1]`，所以
/// `|opposite_index| ≤ 4·nTri − 2`。要塞进 15 位二补数（`−16384..=16383`），
/// 需要 `nTri ≤ 4096`。
///
/// 报告 §12 写的是 `IVP_MAX_TRIANGLES_PER_LEDGE = 8192`，那个数字是按
/// `±32767` 推出来的，**用在这个 15 位字段上会溢出**。这里取 4096。
pub const MAX_TRIANGLES_PER_HULL: usize = 4096;

/// `start_point_index` 是 16 位，所以共享点数组最多这么多点。
pub const MAX_POINTS_PER_SOLID: usize = u16::MAX as usize;

// ---------------------------------------------------------------------------
// 输入 / 输出类型
// ---------------------------------------------------------------------------

/// 写出错误。
#[derive(Debug, Clone, PartialEq)]
pub enum PhyError {
    /// 凸包顶点少于 4 个，张不成体积。
    HullTooFewPoints { solid: usize, points: usize },
    /// 凸包计算失败（共面 / 共线 / 含 NaN 等退化输入）。
    HullDegenerate { solid: usize, detail: String },
    /// 面表引用了不存在的顶点。
    IndexOutOfRange {
        solid: usize,
        face: usize,
        index: u32,
        vertex_count: usize,
    },
    /// 同一条有向边出现了两次 —— 网格不是可定向流形。
    DuplicateEdge { solid: usize, from: u32, to: u32 },
    /// 有向边找不到反向配对 —— 网格有洞（非闭合）。
    OpenMesh { solid: usize, from: u32, to: u32 },
    /// 三角形太多，`opposite_index` 塞不进 15 位有符号。
    TooManyTriangles {
        solid: usize,
        triangles: usize,
        max: usize,
    },
    /// 顶点下标超出 `start_point_index:16`。
    TooManyPoints { solid: usize, points: usize },
    /// 坐标里出现 NaN 或无穷。
    NonFinitePoint { solid: usize, point: usize },
    /// 包围盒退化成一点或一条线（半径 0），无法量化 `box_sizes`。
    DegenerateBounds { solid: usize },
    /// 参数越界。
    BadParameter { what: &'static str, detail: String },
    /// 写出的字节自检失败 —— 属本实现的 bug，不是调用方的问题。
    SelfCheck(String),
}

impl std::fmt::Display for PhyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HullTooFewPoints { solid, points } => write!(
                f,
                "solid[{solid}] 只有 {points} 个顶点，凸包至少要 4 个不共面的点"
            ),
            Self::HullDegenerate { solid, detail } => {
                write!(f, "solid[{solid}] 的凸包退化：{detail}")
            }
            Self::IndexOutOfRange {
                solid,
                face,
                index,
                vertex_count,
            } => write!(
                f,
                "solid[{solid}] 第 {face} 个面引用了顶点 {index}，但只有 {vertex_count} 个顶点"
            ),
            Self::DuplicateEdge { solid, from, to } => write!(
                f,
                "solid[{solid}] 有向边 {from}->{to} 出现两次，网格不是可定向流形"
            ),
            Self::OpenMesh { solid, from, to } => write!(
                f,
                "solid[{solid}] 有向边 {from}->{to} 没有反向边，网格不闭合"
            ),
            Self::TooManyTriangles {
                solid,
                triangles,
                max,
            } => write!(
                f,
                "solid[{solid}] 有 {triangles} 个三角形，超过单 hull 上限 {max}\
                 （opposite_index 只有 15 位）"
            ),
            Self::TooManyPoints { solid, points } => write!(
                f,
                "solid[{solid}] 有 {points} 个顶点，超过 16 位索引上限 {MAX_POINTS_PER_SOLID}"
            ),
            Self::NonFinitePoint { solid, point } => {
                write!(f, "solid[{solid}] 第 {point} 个顶点含 NaN 或无穷")
            }
            Self::DegenerateBounds { solid } => write!(
                f,
                "solid[{solid}] 的包围盒半径是 0（所有顶点重合），无法量化 box_sizes"
            ),
            Self::BadParameter { what, detail } => write!(f, "参数 {what} 非法：{detail}"),
            Self::SelfCheck(m) => write!(f, "写出的 PHY 自检失败（本实现的 bug）：{m}"),
        }
    }
}

impl std::error::Error for PhyError {}

/// 一个凸块。
///
/// `vertices` 是模型局部坐标（`$collisionmodel` 的 SMD 顶点**不做任何变换**，
/// 实测 studiomdl 直接把它们喂给 `CollideFromConvex`），
/// `faces` 是 `[u32; 3]` 三角面，**CCW 外向**、闭合、可定向。
#[derive(Debug, Clone, PartialEq)]
pub struct PhyHull {
    /// 凸块顶点（模型局部坐标）。
    pub vertices: Vec<[f32; 3]>,
    /// 三角面（CCW 外向，索引指向 `vertices`）。
    pub faces: Vec<[u32; 3]>,
}

impl PhyHull {
    /// 从任意点集算凸包并构造一个凸块（parry3d quickhull）。
    ///
    /// 面表已经是 CCW 外向的，可以直接进 [`write_phy`]。
    /// 退化输入（少于 4 点、共面、共线、含 NaN）返回
    /// [`PhyError::HullDegenerate`] 而不是 panic —— 这是选
    /// `try_convex_hull` 而不是 `convex_hull` 的原因，后者在退化输入上直接
    /// `unwrap()`。
    pub fn from_points(points: &[[f32; 3]]) -> Result<Self, PhyError> {
        if points.len() < 4 {
            return Err(PhyError::HullTooFewPoints {
                solid: 0,
                points: points.len(),
            });
        }
        for (i, p) in points.iter().enumerate() {
            if !p.iter().all(|c| c.is_finite()) {
                return Err(PhyError::NonFinitePoint { solid: 0, point: i });
            }
        }
        let vs: Vec<Vector> = points.iter().map(|p| Vector::new(p[0], p[1], p[2])).collect();
        let (hull_pts, tris) = parry3d::transformation::try_convex_hull(&vs).map_err(|e| {
            PhyError::HullDegenerate {
                solid: 0,
                detail: e.to_string(),
            }
        })?;
        Ok(Self {
            vertices: hull_pts.iter().map(|v| [v.x, v.y, v.z]).collect(),
            faces: tris,
        })
    }

    /// 官方 `ConvexFromVerts` 的第二段：`BuildOuterHull( hull, 0.01 )`。
    ///
    /// # 官方算法（反编译 `vphysics.dll`）
    ///
    /// `ConvexFromVerts`（vtable[1] = `0x10083230`）是**两段**：
    ///
    /// ```c
    /// p1 = BuildConvexFromVerts(verts, n);   // 0x10083000：轴映射 + qhull
    /// p2 = BuildOuterHull(p1, 0.01);         // 0x10083110
    /// return p2 ? p2 : p1;
    /// ```
    ///
    /// `BuildOuterHull`（`0x10083110` → `FUN_100c08a0` → `FUN_100c0620`）：
    ///
    /// 1. 逐三角形建平面（法线归一化到 `1e-10`）；
    /// 2. 遍历**平面三元组**，Cramer 解 3×3 得交点；
    /// 3. 交点若满足**所有**平面 `n·p + d ≤ 1e-4` 就收下；
    /// 4. 收下时做 **半径 0.01 米** 的贪心去重（`FUN_100c0580`：
    ///    线性扫描已有代表点，首个 `距离² < r²` 即归并、丢弃新点）；
    /// 5. 对去重后的点**再跑一次 qhull**（`FUN_100c2490`）。
    ///
    /// # 单位
    ///
    /// 反编译里 `0.01` 与 `1e-4` 的**单位是米** —— 因为 `BuildConvexFromVerts`
    /// 已经乘过 0.0254。本函数的输入是 **Source 单位（inch）**，
    /// 所以两个阈值都除以 [`SOURCE_TO_IVP`]。
    ///
    /// 这个换算**不改变结果**：贪心判据是 `距离² < r²`，两边同乘 `k²`
    /// 后判据完全等价，所以点数与选中的点（按比例）都不变。
    ///
    /// # 验收
    ///
    /// `docs/_probe/probe_phy_simple.js`：`msh1` / `ucc1` / `ucc3` / `physign`
    /// 四个模型压实后的点集与官方 `.phy` **逐点 100% 命中**（残差 < 1e-7）。
    ///
    /// # 已知局限
    ///
    /// 贪心去重是**顺序相关**的（极大独立集不唯一），而顺序由 qhull 的
    /// facet 输出次序决定。mdlc 用 parry3d，facet 次序与官方 qhull 不同，
    /// 所以**点数多、结构复杂**的凸包（如 mikuw 的 84 顶点）结果会与官方
    /// 有差异（实测 84 → 49，官方 40）—— 两者都是合法的 1 cm 独立集。
    ///
    /// 平面数超过 [`MAX_COMPACT_PLANES`] 时退化成**只对凸包顶点做去重**
    /// （跳过三元组枚举）—— 那条路径是 `O(P⁴)`，对复杂网格会跑到分钟级。
    pub fn compact_outer_hull(&self) -> Self {
        let r = COMPACT_RADIUS_M / SOURCE_TO_IVP;
        let eps = COMPACT_INSIDE_EPS_M / SOURCE_TO_IVP;

        let planes = hull_planes(&self.vertices, &self.faces);
        if planes.len() < 4 {
            return self.clone();
        }

        let reps = if planes.len() <= MAX_COMPACT_PLANES {
            let mut reps: Vec<[f32; 3]> = Vec::new();
            for i in 0..planes.len() {
                for j in (i + 1)..planes.len() {
                    for k in (j + 1)..planes.len() {
                        let Some(p) = solve_three_planes(&planes[i], &planes[j], &planes[k])
                        else {
                            continue;
                        };
                        // 必须在**所有**平面内侧（容差 eps）。
                        if planes
                            .iter()
                            .any(|q| dot3(q.n, p) + q.d > eps)
                        {
                            continue;
                        }
                        push_unique(&mut reps, p, r);
                    }
                }
            }
            reps
        } else {
            // 回退：直接用凸包顶点（parry 给的已是真顶点）。
            let mut reps: Vec<[f32; 3]> = Vec::new();
            for &p in &self.vertices {
                push_unique(&mut reps, p, r);
            }
            reps
        };

        if reps.len() < 4 {
            return self.clone();
        }
        // 官方最后再跑一次 qhull；失败（退化）时保留原样。
        Self::from_points(&reps).unwrap_or_else(|_| self.clone())
    }
}

/// 贪心去重的一步：与已有代表点距离² < r² 就丢弃，否则追加。
///
/// 与官方 `FUN_100c0580` 同构 —— **线性扫描、首个命中即返回**，
/// 所以结果与插入顺序有关。
fn push_unique(reps: &mut Vec<[f32; 3]>, p: [f32; 3], r: f32) {
    let rr = r * r;
    if reps.iter().any(|q| dist2(*q, p) < rr) {
        return;
    }
    reps.push(p);
}

/// 一个支撑平面：`n·x + d = 0`，`n` 单位且**朝外**（体内点 `n·x + d ≤ 0`）。
#[derive(Clone, Copy)]
struct HullPlane {
    n: [f32; 3],
    d: f32,
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// 逐三角形建支撑平面，并做官方 `FUN_100c03f0` 的**近平行合并**。
///
/// # 官方合并规则（反编译 `vphysics.dll` `FUN_100c03f0`）
///
/// 每加一个新平面 `(n, d)`，先扫已有平面：
///
/// ```c
/// if (0.9999 < dot(p.n, n)) {              // 方向夹角 < ~0.81°
///     if (p.d <= d && d != p.d) return;    // 已有更靠外 ⟹ 丢弃新平面
///     mark p for removal;                  // 新平面更靠外 ⟹ 删掉旧的
/// }
/// ```
///
/// 两个要点：
///
/// * 阈值是 **0.9999**（≈0.81°），不是「几乎完全相等」。
///   用 `1-1e-6`（≈0.08°）会留下大量几乎重合的平面，
///   三元组枚举里产生冗余候选点。
/// * 语义是「**保留更靠外的那个**」—— 不是「跳过重复」。
///   新平面更靠外时**要删掉**已有的那个。
///
/// # 朝向归一
///
/// 法线统一为**朝外**（体内点 `n·x + d ≤ 0`）。判据用
/// 「所有点都应落在 ≤0 一侧」，不能只看 `max > 0` ——
/// 浮点舍入会把本该是 0 的 max 变成 +2e-8，于是每个平面都被翻一次。
fn hull_planes(verts: &[[f32; 3]], faces: &[[u32; 3]]) -> Vec<HullPlane> {
    /// 官方 `FUN_100c03f0` 的方向阈值。
    const COS_MERGE: f32 = 0.9999;

    let mut out: Vec<HullPlane> = Vec::with_capacity(faces.len());
    for f in faces {
        let (a, b, c) = (
            verts[f[0] as usize],
            verts[f[1] as usize],
            verts[f[2] as usize],
        );
        let mut n = cross3(
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        // `!(len > 1e-10)` 而不是 `len <= 1e-10`：前者对 NaN 也成立
        // （NaN 会让退化法线漏过去，产生垃圾平面）。
        // 写成 `!` 形式会让 clippy 报 `neg_cmp_op_on_partial_ord`，
        // 所以显式写出 NaN 检查。
        let len = dot3(n, n).sqrt();
        if len.is_nan() || len <= 1e-10 {
            continue;
        }
        n = [n[0] / len, n[1] / len, n[2] / len];
        let mut d = -dot3(n, a);
        // 定朝向：朝外时「所有点的 n·x+d」应以负值为主。
        let mut mx = f32::NEG_INFINITY;
        let mut mn = f32::INFINITY;
        for p in verts {
            let s = dot3(n, *p) + d;
            if s > mx {
                mx = s;
            }
            if s < mn {
                mn = s;
            }
        }
        if mx.abs() > mn.abs() {
            n = [-n[0], -n[1], -n[2]];
            d = -d;
        }

        // 官方合并：近平行的只保留最靠外的那个。
        let mut drop = false;
        let mut remove: Vec<usize> = Vec::new();
        for (i, q) in out.iter().enumerate() {
            if dot3(q.n, n) > COS_MERGE {
                if q.d <= d && d != q.d {
                    drop = true;
                    break;
                }
                remove.push(i);
            }
        }
        if drop {
            continue;
        }
        for &i in remove.iter().rev() {
            out.swap_remove(i);
        }
        out.push(HullPlane { n, d });
    }
    out
}

/// 解三个平面的交点（Cramer 法则）。平行/退化时返回 `None`。
fn solve_three_planes(a: &HullPlane, b: &HullPlane, c: &HullPlane) -> Option<[f32; 3]> {
    let det = dot3(a.n, cross3(b.n, c.n));
    if det.abs() < 1e-12 {
        return None;
    }
    let n1 = cross3(b.n, c.n);
    let n2 = cross3(c.n, a.n);
    let n3 = cross3(a.n, b.n);
    Some([
        (-a.d * n1[0] - b.d * n2[0] - c.d * n3[0]) / det,
        (-a.d * n1[1] - b.d * n2[1] - c.d * n3[1]) / det,
        (-a.d * n1[2] - b.d * n2[2] - c.d * n3[2]) / det,
    ])
}

/// 官方 `BuildOuterHull` 的去重半径：**0.01 米**（反编译 `0x10083230`
/// 传给 `FUN_10083110` 的字面量）。
pub const COMPACT_RADIUS_M: f32 = 0.01;

/// 官方「点在所有平面内侧」的容差：**1e-4 米**（反编译 `FUN_100c0620`
/// 里的 `-0.0001` 比较）。
pub const COMPACT_INSIDE_EPS_M: f32 = 0.0001;

/// 三元组枚举的上限：`O(P³)` 个三元组、每个再查 `P` 个平面 ⟹ `O(P⁴)`。
/// 164 个平面约 1.2 亿次内层判断（可接受）；超过这个数就退化到
/// 只对凸包顶点去重。
pub const MAX_COMPACT_PLANES: usize = 256;

/// `$jointconstrain <骨骼> <轴> <类型> <min> <max> [friction]` 的**类型**。
///
/// 官方 `jointlimit_t`（`collisionmodel.cpp:75-77`）：
/// `JOINT_FREE = 0` / `JOINT_FIXED = 1` / `JOINT_LIMIT = 2`。
///
/// 三种类型在 `BuildRagdollConstraint`（`:2245-2256`）里落盘不同：
///
/// | 类型 | 落盘 |
/// |---|---|
/// | `limit` | `(min, max, friction/5)` |
/// | `fixed` | `(0, 0, 0)` —— **忽略 min/max/friction** |
/// | `free`  | `(-360, 360, friction/5)` —— **忽略 min/max** |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JointLimitType {
    /// `free` → 落盘 `±360`。
    Free,
    /// `fixed` → 落盘全 `0`。
    Fixed,
    /// `limit` → 落盘 `(min, max, friction)`。
    Limit,
}

impl JointLimitType {
    /// 从 QC 文本解析（大小写不敏感，与官方 `stricmp` 一致）。
    pub fn parse(s: &str) -> Option<Self> {
        let l = s.to_ascii_lowercase();
        match l.as_str() {
            "free" => Some(Self::Free),
            "fixed" => Some(Self::Fixed),
            "limit" => Some(Self::Limit),
            _ => None,
        }
    }

    /// 官方写出的三个轴值。
    ///
    /// ⚠️ **`friction` 会先除以 5**（`AddConstraint`，`collisionmodel.cpp:539`
    /// 的 `friction * (1.0f/5.0f)`，注释说「编辑器里 friction 显示为 5 倍」）。
    /// 实测 `$jointconstrain ... 5` → 落盘 `1.000000`。
    pub fn axis_values(self, min: f32, max: f32, friction: f32) -> (f32, f32, f32) {
        match self {
            Self::Limit => (min, max, friction / 5.0),
            Self::Fixed => (0.0, 0.0, 0.0),
            Self::Free => (-360.0, 360.0, friction / 5.0),
        }
    }
}

/// 一条 `$jointconstrain`。
///
/// 官方存进 `CJointConstraint` 链表（**头插**，`AddConstraint` 里
/// `pConstraint->m_pNext = m_pConstraintList`）。顺序不影响结果 ——
/// `BuildRagdollConstraint` 遍历整条链，按 `index == ragdoll.childIndex`
/// 过滤，同轴后写覆盖先写。
#[derive(Debug, Clone, PartialEq)]
pub struct JointConstraint {
    /// 骨骼名（`$jointconstrain` 的第 1 个参数）。
    pub bone: String,
    /// 轴：`0 = x` / `1 = y` / `2 = z`。
    ///
    /// 官方算法是 `tolower(axis[0]) - 'x'`（`collisionmodel.cpp:1668`），
    /// 所以只有首字母有意义。
    pub axis: u8,
    /// 约束类型。
    pub kind: JointLimitType,
    /// 下限（度）。
    pub min: f32,
    /// 上限（度）。
    pub max: f32,
    /// 摩擦（**落盘前会 ÷5**）。
    pub friction: f32,
}

impl JointConstraint {
    /// 从轴字母解析 `0/1/2`（官方 `tolower(axis[0]) - 'x'`）。
    pub fn axis_from_char(c: char) -> Option<u8> {
        match c.to_ascii_lowercase() {
            'x' => Some(0),
            'y' => Some(1),
            'z' => Some(2),
            _ => None,
        }
    }

    /// 轴字母（用于诊断输出）。
    pub fn axis_char(&self) -> char {
        ['x', 'y', 'z'][self.axis as usize % 3]
    }
}

/// `$animatedfriction <min> <max> <timein> <timehold> <timeout>`。
///
/// ⚠️ **注意 QC 的参数顺序与落盘顺序不同**：
/// QC 是 `min max timein timehold timeout`，而落盘是
/// `animfrictiontimein` / `animfrictiontimeout` / `animfrictiontimehold`
/// —— `timeout` 与 `timehold` **对调**。
///
/// 依据：`CCmd_JoinAnimatedFriction`（`collisionmodel.cpp:1752-1760`）
/// 把第 3/4/5 个参数依次赋给 `m_flFrictionTimeIn` / `m_flFrictionTimeOut` /
/// `m_flFrictionTimeHold`；写出时（`:2452-2456`）按
/// `In → Out → Hold` 的顺序。
///
/// 受控实验 `animfric`（`$animatedfriction 100 500 0.1 0.2 1.0`）实测：
/// `timein=0.100000`、`timeout=1.000000`、`timehold=0.200000`
/// ⟹ 第 4 个参数 `0.2` 落进 `timehold`，第 5 个 `1.0` 落进 `timeout`。**证实对调**。
///
/// 两个 `*friction*` 字段是**整数**（`Safe_atoi`，不是 `atof`），
/// 但写出用 `KeyWriteFloat` ⟹ 落盘是 `"100.000000"`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimatedFriction {
    /// `animfrictionmin`（**整数**取值）。
    pub min: i32,
    /// `animfrictionmax`（**整数**取值）。
    pub max: i32,
    /// `animfrictiontimein`（秒）。
    pub time_in: f32,
    /// `animfrictiontimeout`（秒）—— QC 的**第 5** 个参数。
    pub time_out: f32,
    /// `animfrictiontimehold`（秒）—— QC 的**第 4** 个参数。
    pub time_hold: f32,
}

/// 一个 solid 的写出参数。
#[derive(Debug, Clone, PartialEq)]
pub struct PhySolid {
    /// 这个 solid 绑定的骨骼下标。
    ///
    /// - `Some(i)` → ledge 的 `client_data` 写 `i + 1`（ragdoll）。
    /// - `None` → 写 `0`，即"未绑定"哨兵（单 solid prop）。
    ///
    /// 实测 90/90 零例外：ragdoll 的每个 solid 的 `client_data` 恒等于
    /// `boneIndex + 1`。**text section 里的 `name` 是另一条独立的绑定路径，
    /// 两条都要写对。**
    pub bone_index: Option<u32>,
    /// text section 里 `"name"` 的值。ragdoll 用骨骼名，prop 用 `<model>_physbox`。
    pub name: String,
    /// `"parent"` 的骨骼名。`None` 时不写这一行，也不写对应的 `ragdollconstraint`。
    pub parent: Option<String>,
    /// `"massbias"`。只有 `!= 1.0` 时才写进文本段（与 studiomdl 一致）。
    pub mass_bias: f32,
    /// `"damping"` 的**本 solid 覆盖**。`None` = 用 [`PhyParams::damping`]。
    ///
    /// 对应官方 `$jointdamping`（`collisionmodel.cpp:658-665`）——
    /// 它在 `SetCollisionModelDefaults` **之后**改单个 solid 的值。
    /// 语料里 `damping` 在 0/39 个多 solid 文件里逐 solid 变化，
    /// 所以这项通常为 `None`，但接口要留。
    pub damping: Option<f32>,
    /// `"rotdamping"` 的本 solid 覆盖。`None` = 用 [`PhyParams::rot_damping`]。
    ///
    /// ⚠️ **这一项在语料里真的会变**：14/39 个多 solid 文件
    /// （boomer / hulk / hunter 等 ragdoll）里各 solid 互不相同。
    pub rot_damping: Option<f32>,
    /// `"inertia"` 的本 solid 覆盖。`None` = 用 [`PhyParams::inertia`]。
    ///
    /// 语料里 1/39 个多 solid 文件（`anim_common`）逐 solid 不同。
    pub inertia: Option<f32>,
}

impl PhySolid {
    /// 单 solid prop：不绑骨骼，名字取**碰撞 SMD 的 basename**。
    ///
    /// # ⚠️ 不是 `<model>_physbox`
    ///
    /// 官方 `ProcessSingleBody`（`collisionmodel.cpp:1563-1571`）：
    ///
    /// ```cpp
    /// char tmp[512];
    /// Q_FileBase( pmodel->filename, tmp, sizeof( tmp ) );  // ← 碰撞 SMD 的文件名
    /// pPhys->m_name = out;                                  //   去掉扩展名
    /// ```
    ///
    /// `pmodel` 是 **`$collisionmodel <smd>` 指向的那个 SMD**，
    /// 与 `$modelname` **无关**。受控实验（`docs/_probe/gen_phyname.js`）：
    ///
    /// | QC | 落盘 `"name"` |
    /// |---|---|
    /// | `$collisionmodel "physign_geo.smd"` | `"physign_geo"` |
    /// | `$collisionmodel "phyrot_phy.smd"` | `"phyrot_phy"` |
    /// | `$collisionmodel "msh1.smd"` | `"msh1"` |
    ///
    /// 语料 2459 个单 solid 里只有 **1142** 个恰好是 `<mdl>_physbox`
    /// —— 那只是因为它们的碰撞 SMD 正好叫 `<mdl>_physbox.smd`。
    pub fn prop(collision_smd_name: &str) -> Self {
        Self {
            bone_index: None,
            name: collision_smd_name.to_string(),
            parent: None,
            mass_bias: 1.0,
            damping: None,
            rot_damping: None,
            inertia: None,
        }
    }

    /// ragdoll 的一个骨骼组：绑到 `bone_index`。
    pub fn ragdoll(bone_index: u32, name: impl Into<String>, parent: Option<String>) -> Self {
        Self {
            bone_index: Some(bone_index),
            name: name.into(),
            parent,
            mass_bias: 1.0,
            damping: None,
            rot_damping: None,
            inertia: None,
        }
    }
}

/// 整个文件的写出参数。
#[derive(Debug, Clone, PartialEq)]
pub struct PhyParams<'a> {
    /// 模型名（不带扩展名）。
    pub model_name: &'a str,
    /// 配对 `.mdl` 的 `studiohdr_t::checksum`（`.mdl` 偏移 0x08）。
    ///
    /// 写错**不是致命错误**（引擎只给
    /// `WarningPhyFileChecksumDoesNotMatchMdlFileChecksum` 然后继续解析），
    /// 但会产生警告且部分工具链会拒绝，所以必须与 `.mdl` 逐位相同。
    pub checksum: u32,
    /// QC `$mass` 的等效总质量，写进 `editparams.totalmass`。
    ///
    /// ⚠️ **它同时是 `.mdl` 头部 `mass` 的值** —— 官方
    /// `write.cpp:2092` 是 `phdr->mass = GetCollisionModelMass();`，
    /// 而 `GetCollisionModelMass()` 返回 `g_JointedModel.m_totalMass`
    /// （`collisionmodel.cpp:2262-2265`）。也就是说 studiomdl 里
    /// **没有**独立的「模型质量」命令，`$mass` 只出现在
    /// `$collisionmodel {}` / `$collisionjoints {}` 块内
    /// （`collisionmodel.cpp:1781`）。
    ///
    /// 语料实测（`docs/_probe/probe_mass_law.js`，3333 个模型）：
    /// 有 `.phy` 的 2498 个里 **2483 个** `mdl.mass == phy.totalmass`，
    /// 其余 15 个的 `.phy` checksum 与 `.mdl` **不配对**（陈旧产物）；
    /// 没有 `.phy` 的 835 个里 **827 个** `mdl.mass == 1.0`。
    pub total_mass: f32,
    /// `"surfaceprop"`，如 `"wood_solid"` / `"flesh"` / `"default"`。
    pub surface_prop: &'a str,
    /// `"damping"`。
    pub damping: f32,
    /// `"rotdamping"`。
    pub rot_damping: f32,
    /// `"inertia"`。
    ///
    /// **与 `IVP_Compact_Surface::rotation_inertia` 无关**，这是 QC
    /// `$inertia` 的文本回显。L4D2 实测 prop 恒为 `1.0`、ragdoll 为 `2.0`/`10.0`。
    pub inertia: f32,
    /// 可选 `"drag"`。只有 `Some` 时才写（对应 studiomdl 的
    /// `m_dragCoefficient != -1`）。
    pub drag: Option<f32>,
    /// `editparams.rootname`。prop 写空串；ragdoll 写根骨骼名的小写。
    pub root_name: &'a str,
    /// 是否写 `editparams.concave "1"`。
    ///
    /// 实测：由 `$concave` 生成的 prop（`wood_fence` / `urban_puddle_model01a`）
    /// 有它，未经凸分解的（`l4d_gift` / `cone_helper`）没有。
    pub concave: bool,
    /// `rotation_inertia` 的缩放系数。见模块文档。
    ///
    /// `1.0` = 写真实的密度 1 惯性张量对角元；`0.0` = 写 `[0,0,0]`（最保守）。
    pub inertia_scale: f32,
    /// `$jointconstrain` 的全部记录（**跨 solid**，按骨骼名匹配）。
    ///
    /// 官方 `BuildRagdollConstraint` 对**每个** solid 遍历**整条**约束链，
    /// 只应用 `CollisionIndex(pList->m_pJointName) == ragdoll.childIndex`
    /// 的那些 —— 也就是说约束挂在**子** solid 上。
    pub constraints: Vec<JointConstraint>,
    /// `$animatedfriction`。`Some` ⟹ 写一个 `animatedfriction {}` 块。
    pub animated_friction: Option<AnimatedFriction>,
    /// `$noselfcollisions` ⟹ 写 `collisionrules { "selfcollisions" "0" }`。
    ///
    /// ⚠️ **与 `collision_pairs` 互斥**，且 `noself` **优先** ——
    /// 官方是 `if (m_noSelfCollisions) ... else if (m_pCollisionPairs)`
    /// （`collisionmodel.cpp:2422-2428`）。
    pub no_self_collisions: bool,
    /// `$jointcollide <a> <b>` 的配对（**骨骼名**，写出时转成 solid 下标）。
    ///
    /// 只有当 `no_self_collisions == false` 时才写。
    pub collision_pairs: Vec<(String, String)>,
    /// `$jointmerge <parent> <child>` 的**原始文本**（写出时原样回显）。
    ///
    /// 官方把 `"<parent>,<child>"` 直接 `strdup` 进 `m_mergeList`
    /// （`AddMergeCommand`，`collisionmodel.cpp:300-305`），写出时
    /// `Q_snprintf("%s,%s")` 拼回（`:2471`）—— **用的是 QC 里写的原始名字**，
    /// 不是解析后的骨骼名。实测 `$jointmerge "bone_mid" "bone_tip"` →
    /// `"jointmerge" "bone_mid,bone_tip"`。
    pub merge_list: Vec<(String, String)>,
    /// `$masscenter <x> <y> <z>`。
    ///
    /// 官方走 `CollideSetMassCenter`（`collisionmodel.cpp:361-363`），
    /// 它改的是**二进制** `IVP_Compact_Surface` 的质心，**不写文本段**。
    /// 本字段因此只影响 `mass_center` / `upper_limit_radius` 的算法。
    pub mass_center: Option<[f32; 3]>,
    /// `$automass` ⟹ 总质量由**体积 × 密度**算出（`ComputeMass`）。
    ///
    /// 官方把 `m_totalMass` 置 `-1` 当哨兵（`SetAutoMass`，`:567-570`），
    /// `ComputeMass` 首句 `if (m_totalMass >= 0) return;` 于是才会真的算。
    /// 实测 `automass`（3 个 10³ 盒子、metal）：`totalmass = 5.309408`。
    pub auto_mass: bool,
}

impl<'a> PhyParams<'a> {
    /// 用模型名与 checksum 构造，其余字段取实测最常见的保守默认值。
    ///
    /// ⚠️ `total_mass` 的默认值是 **1.0**，不是 10.0。
    ///
    /// 源码依据：`CJointedModel::CJointedModel()` 里 `m_totalMass = 1.0`
    /// （`collisionmodel.cpp:252`），而 `ComputeMass()` 的**第一句**是
    /// `if ( m_totalMass >= 0 ) return;` —— `1.0 >= 0`，所以**直接返回**，
    /// 自动质量那条路只在显式 `$automass`（把值置 `-1`）时才会走。
    ///
    /// 语料印证：没有 `.phy` 的 835 个模型里 827 个 `mdl.mass == 1.0`
    /// （`probe_mass_law.js`）。
    pub fn new(model_name: &'a str, checksum: u32) -> Self {
        Self {
            model_name,
            checksum,
            total_mass: 1.0,
            surface_prop: "default",
            damping: 0.0,
            rot_damping: 0.0,
            inertia: 1.0,
            drag: None,
            root_name: "",
            concave: false,
            inertia_scale: 1.0,
            constraints: Vec::new(),
            animated_friction: None,
            no_self_collisions: false,
            collision_pairs: Vec::new(),
            merge_list: Vec::new(),
            mass_center: None,
            auto_mass: false,
        }
    }
}

/// 焊接后的网格：`(顶点池, 索引面表)`。
///
/// 抽成别名是为了让 [`weld_smd_triangles`] 的签名可读
/// （否则触发 `clippy::type_complexity`）。
pub type WeldedMesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// 把一个 SMD 的三角形**焊接**成「顶点池 + 索引面表」。
///
/// # 为什么只做精确去重
///
/// SMD 的三角形顶点是**逐面独立**的（没有索引行，且 UV/法线接缝会让同一
/// 位置出现多次）。这里按「三个 float 的位模式完全相同」焊接 ——
/// **只做精确去重，不做 epsilon 合并**，否则会改动坐标、进而让
/// `upper_limit_radius` / `box_sizes` 与写入的点对不上。
///
/// 焊接后**退化的三角形**（三个下标不全不同）会被剔除：凸包计算不关心
/// 面表，但 VHACD 会关心。
///
/// 返回 `(顶点池, 面表)`；顶点数 < 4 时返回 `Err`（张不成凸包）。
pub fn weld_smd_triangles(smd: &crate::smd::Smd) -> Result<WeldedMesh, String> {
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut index_of: std::collections::HashMap<[u32; 3], u32> = std::collections::HashMap::new();
    let mut faces: Vec<[u32; 3]> = Vec::with_capacity(smd.triangles.len());
    for t in &smd.triangles {
        let mut f = [0u32; 3];
        for (k, v) in t.vertices.iter().enumerate() {
            let p = v.position;
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            f[k] = *index_of.entry(key).or_insert_with(|| {
                vertices.push(p);
                (vertices.len() - 1) as u32
            });
        }
        if f[0] != f[1] && f[1] != f[2] && f[0] != f[2] {
            faces.push(f);
        }
    }
    if vertices.len() < 4 {
        return Err(format!(
            "焊接后只有 {} 个不同顶点，凸包至少要 4 个",
            vertices.len()
        ));
    }
    Ok((vertices, faces))
}

/// 碰撞 SMD 的骨骼绑定信息：每个顶点的 `(骨骼名, 权重)` 列表。
///
/// 官方 `ConvertToWorldSpace`（`collisionmodel.cpp:708-738`）需要按
/// `localBoneweight[i]` 做**加权混合**，所以这里保留多骨骼绑定。
pub type SmdWeights = Vec<Vec<(String, f32)>>;

/// 从 SMD 抽出「每个顶点绑了哪些骨骼、权重多少」。
///
/// 骨骼用**名字**表示 —— 碰撞 SMD 与序列 SMD 的下标空间不一定相同，
/// 官方靠 `boneLocalToGlobal` 映射，本实现直接按名字对齐。
pub fn vertex_bone_weights(smd: &crate::smd::Smd) -> SmdWeights {
    let name_of: std::collections::HashMap<i32, &str> =
        smd.nodes.iter().map(|n| (n.index, n.name.as_str())).collect();
    smd.triangles
        .iter()
        .flat_map(|t| t.vertices.iter())
        .map(|v| {
            v.links
                .iter()
                .filter_map(|l| {
                    name_of
                        .get(&l.bone)
                        .map(|n| ((*n).to_string(), l.weight))
                })
                .collect()
        })
        .collect()
}

/// 碰撞 SMD 每根骨骼的 `(参考姿态世界矩阵, 其逆)`，按骨骼名索引。
///
/// `boneToPose` 把「参考姿态下的顶点」搬到骨骼局部空间，
/// 逆矩阵把顶点搬回去。两者都预先算好，避免逐顶点求逆。
struct RestPose {
    world: std::collections::HashMap<String, crate::bone_math::Matrix3x4>,
    inverse: std::collections::HashMap<String, crate::bone_math::Matrix3x4>,
}

impl RestPose {
    fn of(smd: &crate::smd::Smd) -> Self {
        let (pos, rot, parents, names) = smd_rest_pose(smd);
        let world = crate::bone_math::compute_world(&pos, &rot, &parents);
        let mut w = std::collections::HashMap::with_capacity(names.len());
        let mut i = std::collections::HashMap::with_capacity(names.len());
        for (k, n) in names.iter().enumerate() {
            w.insert(n.clone(), world[k]);
            i.insert(n.clone(), crate::bone_math::invert(&world[k]));
        }
        Self { world: w, inverse: i }
    }
}

/// 把一个顶点按骨骼绑定搬到世界空间（官方 `ConvertToWorldSpace` 的内层循环）。
///
/// ```c
/// worldVerts[i] = 0
/// for each (localBone, weight):
///     VectorITransform( vertex[i].position, boneToPose[localBone], tmp2 )  // → 骨骼局部
///     VectorTransform( tmp2, boneToWorld[globalBone], tmp )                // → 世界
///     worldVerts[i] += weight * tmp
/// ```
fn world_point(
    p: [f32; 3],
    links: &[(String, f32)],
    rest: &RestPose,
    pose_world: Option<&std::collections::HashMap<String, crate::bone_math::Matrix3x4>>,
) -> [f32; 3] {
    use crate::bone_math::IDENTITY;
    if links.is_empty() {
        return p;
    }
    let mut acc = [0.0f32; 3];
    for (name, weight) in links {
        // 序列姿态（世界）；没有序列时退化成参考姿态。
        let world = pose_world
            .and_then(|m| m.get(name.as_str()))
            .copied()
            .or_else(|| rest.world.get(name.as_str()).copied())
            .unwrap_or(IDENTITY);
        // 顶点从参考姿态搬到骨骼局部空间。
        //
        // ⚠️ 这一步不能省。碰撞 SMD 的顶点是**它自己参考姿态**下的，
        // 而 `boneToWorld` 是**序列姿态**下的；两者不同（mikuw 的序列把
        // bone 0 转了 90°）时直接乘会整体偏掉一个平移。
        // 实测漏掉它时 mikuw 命中 0/40，补上后 38/40。
        let local = match rest.inverse.get(name.as_str()) {
            Some(inv) => transform_point(inv, p),
            None => p,
        };
        let w = transform_point(&world, local);
        acc[0] += weight * w[0];
        acc[1] += weight * w[1];
        acc[2] += weight * w[2];
    }
    acc
}

/// 把碰撞 SMD 的顶点搬到**世界空间**（官方 `ConvertToWorldSpace`）。
///
/// # 官方算法（`collisionmodel.cpp:708-738`）
///
/// ```c
/// CalcBoneTransforms( g_panimation[0], 0, boneToWorld );   // ← 第一个序列的第 0 帧
/// for each vertex i:
///     worldVerts[i] = 0
///     for each (localBone, weight) in localBoneweight[i]:
///         globalBone = boneLocalToGlobal[localBone]
///         ConcatTransforms( boneToPose[localBone], srcRealign[globalBone], boneToPose )
///         VectorITransform( vertex[i].position, boneToPose, tmp2 )
///         VectorTransform( tmp2, boneToWorld[globalBone], tmp )
///         worldVerts[i] += weight * tmp
/// ```
///
/// # 姿态取自**序列**，不是碰撞 SMD
///
/// 这一点是实测定案的，不是从源码读出来的：`docs/_probe/gen_phyrot.js`
/// 造了三个几何完全相同、只有**序列**骨骼姿态不同的模型，
/// 结果 `.phy` 三者互不相同而 `.vvd` 三者相同。
///
/// # 参数
///
/// * `smd` —— 碰撞 SMD（提供顶点与它自己的参考姿态）。
/// * `weights` —— 与 `smd.triangles` 展平后**同序**的骨骼绑定。
/// * `pose_world` —— 序列第 0 帧的世界矩阵，键是骨骼名。
///   `None` 表示「没有序列」⟹ 退化成碰撞 SMD 自己的参考姿态。
pub fn to_world_space(
    smd: &crate::smd::Smd,
    weights: &SmdWeights,
    pose_world: Option<&std::collections::HashMap<String, crate::bone_math::Matrix3x4>>,
) -> Vec<[f32; 3]> {
    let rest = RestPose::of(smd);
    let mut out = Vec::with_capacity(weights.len());
    let mut flat = 0usize;
    for t in &smd.triangles {
        for v in &t.vertices {
            let links = weights.get(flat).map(|w| w.as_slice()).unwrap_or(&[]);
            flat += 1;
            out.push(world_point(v.position, links, &rest, pose_world));
        }
    }
    out
}

/// 从「第一个序列第 0 帧」算出每根骨骼的世界矩阵，按**骨骼名**索引。
///
/// # 为什么是「第一个序列」
///
/// 官方 `ConvertToWorldSpace`（`collisionmodel.cpp:713`）写的是
/// `CalcBoneTransforms( g_panimation[0], 0, boneToWorld )` ——
/// `g_panimation[0]` 就是 QC 里**第一个 `$sequence`**。
/// 碰撞 SMD 自己的姿态**完全不参与**。
///
/// 这一点用受控实验定过案（`docs/_probe/gen_phyrot.js`）：造三个几何
/// 完全相同、只有序列姿态不同的模型，`.phy` 三者互不相同而 `.vvd` 三者相同。
///
/// `frames[0]` 的骨骼顺序按 `bones`（模型骨骼表）排列 ——
/// 与 `compile.rs` 的产出约定一致。
pub fn sequence_pose_world(
    bones: &[crate::model::Bone],
    frame: &[crate::smd::SmdPose],
) -> std::collections::HashMap<String, crate::bone_math::Matrix3x4> {
    let mut pos = Vec::with_capacity(bones.len());
    let mut rot = Vec::with_capacity(bones.len());
    let mut parents = Vec::with_capacity(bones.len());
    let index_of: std::collections::HashMap<&str, usize> = bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    for (i, b) in bones.iter().enumerate() {
        match frame.get(i) {
            Some(p) => {
                pos.push(p.position);
                rot.push(p.rotation);
            }
            None => {
                // 该帧没给这根骨骼 ⟹ 用它的参考姿态。
                // `Bone::rotation` 是**角度**（与 QC 一致），要转成弧度。
                pos.push(b.position.unwrap_or([0.0; 3]));
                rot.push(
                    b.rotation
                        .map(|r| [r[0].to_radians(), r[1].to_radians(), r[2].to_radians()])
                        .unwrap_or([0.0; 3]),
                );
            }
        }
        parents.push(
            b.parent
                .as_deref()
                .and_then(|p| index_of.get(p).copied())
                .map(|v| v as i32)
                .unwrap_or(-1),
        );
    }
    let world = crate::bone_math::compute_world(&pos, &rot, &parents);
    bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), world[i]))
        .collect()
}

/// 把**去重后**的顶点池搬到世界空间（[`weld_smd_triangles`] 的配套）。
///
/// [`to_world_space`] 按「三角形展平顺序」遍历，而这里拿到的是去重后的
/// 顶点池。做法：为每个唯一顶点记住**首次出现**时的骨骼绑定
/// —— 焊接就是按位置精确相等做的，同位置顶点的绑定必然相同。
fn world_space_verts(
    smd: &crate::smd::Smd,
    unique: &[[f32; 3]],
    pose_world: Option<&std::collections::HashMap<String, crate::bone_math::Matrix3x4>>,
) -> Vec<[f32; 3]> {
    let all = vertex_bone_weights(smd);
    let rest = RestPose::of(smd);

    // 位置（按位）→ 首次出现的展平下标
    let mut first: std::collections::HashMap<[u32; 3], usize> =
        std::collections::HashMap::new();
    let mut flat = 0usize;
    for t in &smd.triangles {
        for v in &t.vertices {
            let key = [
                v.position[0].to_bits(),
                v.position[1].to_bits(),
                v.position[2].to_bits(),
            ];
            first.entry(key).or_insert(flat);
            flat += 1;
        }
    }

    unique
        .iter()
        .map(|p| {
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            let links = first
                .get(&key)
                .and_then(|&i| all.get(i))
                .map(|w| w.as_slice())
                .unwrap_or(&[]);
            world_point(*p, links, &rest, pose_world)
        })
        .collect()
}

/// 用 `matrix3x4_t` 变换一个点。
fn transform_point(m: &crate::bone_math::Matrix3x4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
        m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
        m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
    ]
}

/// SMD 第 0 帧的局部姿态，整理成 [`crate::bone_math::compute_world`] 需要的形状：
/// `(位置, 旋转, 父下标, 骨骼名)`。
type SmdRestPose = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<i32>, Vec<String>);

/// 取 SMD 第 0 帧的局部姿态，整理成 [`crate::bone_math::compute_world`] 需要的形状。
fn smd_rest_pose(smd: &crate::smd::Smd) -> SmdRestPose {
    let frame = smd.frames.first();
    let mut pos = Vec::with_capacity(smd.nodes.len());
    let mut rot = Vec::with_capacity(smd.nodes.len());
    let mut parents = Vec::with_capacity(smd.nodes.len());
    let mut names = Vec::with_capacity(smd.nodes.len());
    for (i, n) in smd.nodes.iter().enumerate() {
        let p = frame
            .and_then(|f| f.poses.get(i))
            .map(|p| (p.position, p.rotation))
            .unwrap_or(([0.0; 3], [0.0; 3]));
        pos.push(p.0);
        rot.push(p.1);
        parents.push(n.parent);
        names.push(n.name.clone());
    }
    (pos, rot, parents, names)
}

/// 把**去重后**的顶点池搬到世界空间（`weld_smd_triangles` 的配套）。
/// `build_phy_from_smd` / `build_ragdoll_phy_from_smd` 的**身份参数**。
///
/// 把三个字符串收成一个结构体，既让调用点自解释（`ident.surface_prop`
/// 比第 4 个位置参数清楚），也避免 `clippy::too_many_arguments`。
#[derive(Debug, Clone, Copy)]
pub struct PhyIdentity<'a> {
    /// 模型名（**不带扩展名**）。用于诊断输出。
    pub model_name: &'a str,
    /// **碰撞 SMD 的 basename**（不带目录与扩展名）。
    ///
    /// 它是 prop 形态 `"name"` 的取值来源（官方 `Q_FileBase`，
    /// `collisionmodel.cpp:1566`）—— 见 [`PhySolid::prop`]。
    pub collision_smd_name: &'a str,
    /// `$surfaceprop`（模型头 `+0x134` 的那个值）。
    ///
    /// prop 形态下 `GetSurfaceProp` 必然回落到它（solid 名是碰撞 SMD 的
    /// basename，查不到任何骨骼）—— 实测 2498/2498 个 `.phy` 恒等。
    pub surface_prop: &'a str,
}

/// 从**单个 SMD** 直接产出 `.phy` 字节（`$collisionmodel` 的单 solid prop 形态）。
///
/// 这是 `mdlc build` 接入 PHY 的入口，也是 `mdlc phy` 无选项时的等价路径。
///
/// # 参数
///
/// * `smd` —— 碰撞几何的来源（官方是 `$collisionmodel` 指向的**单独 SMD**）。
/// * `ident` —— 三个身份字符串（模型名 / 碰撞 SMD 名 / surfaceprop），
///   见 [`PhyIdentity`]。
/// * `checksum` —— 必须与配对的 `.mdl` 逐位相同，否则引擎报警告。
/// * `mass` —— `$mass` 等效总质量，写进 `editparams.totalmass`。
///   **同时也要写进 `.mdl` 头部的 `mass`** —— 官方 `write.cpp:2092` 是
///   `phdr->mass = GetCollisionModelMass();`，两者是**同一个值**。
/// * `phys` —— `[physics]` 表（`$collisionmodel {}` 块的参数）。
/// * `pose_world` —— 模型**第一个序列第 0 帧**的骨骼世界矩阵（按骨骼名）。
///
///   官方 `ConvertToWorldSpace` 用的是 `g_panimation[0]`，即第一个序列 ——
///   **不是**碰撞 SMD 自己的姿态（已由 `docs/_probe/gen_phyrot.js` 的三模型
///   对照实验定案）。传 `None` 时退化成用碰撞 SMD 自己的参考姿态，
///   这时若两者不同就会偏掉。
pub fn build_phy_from_smd(
    smd: &crate::smd::Smd,
    ident: PhyIdentity<'_>,
    checksum: u32,
    mass: f32,
    phys: &crate::model::Physics,
    pose_world: Option<&std::collections::HashMap<String, crate::bone_math::Matrix3x4>>,
) -> Result<Vec<u8>, String> {
    let PhyIdentity {
        model_name,
        collision_smd_name,
        surface_prop,
    } = ident;
    if smd.triangles.is_empty() {
        return Err("SMD 里没有任何三角形".to_string());
    }

    let hulls: Vec<PhyHull> = if phys.concave {
        decompose_connected_components(smd, pose_world)?
    } else {
        let (vertices, _faces) = weld_smd_triangles(smd)?;
        let vertices = world_space_verts(smd, &vertices, pose_world);
        vec![
            PhyHull::from_points(&vertices)
                .map_err(|e| format!("算凸包失败：{e}"))?
                // 官方 `ConvexFromVerts` 的第二段：`BuildOuterHull(hull, 0.01)`。
                .compact_outer_hull(),
        ]
    };

    // `joint_overrides` / `constraints` 只对 ragdoll 有意义 —— 单 solid 的 prop
    // 路径没有「joint」概念（官方 `InitCollisionModel` 只在
    // `ProcessJointedModel` 里被调用）。写了却不生效是最难查的一类问题，
    // 所以**显式报错**而不是静默忽略。
    if !phys.joint_overrides.is_empty() {
        return Err(format!(
            "[physics.joint_overrides] 有 {} 项，但这是单 solid 的 prop 形态\
             （`joints = false`）—— 逐 joint 参数只对 ragdoll 有意义。\
             要么去掉它，要么把 `joints` 设为 true",
            phys.joint_overrides.len()
        ));
    }
    if !phys.constraints.is_empty() {
        return Err(format!(
            "[physics.constraints] 有 {} 项，但这是单 solid 的 prop 形态\
             （`joints = false`）—— `$jointconstrain` 只对 ragdoll 有意义。\
             要么去掉它，要么把 `joints` 设为 true",
            phys.constraints.len()
        ));
    }
    if !phys.collision_pairs.is_empty() || phys.no_self_collisions {
        return Err(
            "[physics.no_self_collisions] / [physics.collision_pairs] 只对 ragdoll 有意义\
             （`joints = false` 时没有「多个 solid 之间碰不碰」的问题）。\
             要么去掉它，要么把 `joints` 设为 true"
                .to_string(),
        );
    }

    let mut params = PhyParams::new(model_name, checksum);
    params.total_mass = mass;
    // `surfaceprop` 从**模型头**取（`GetSurfaceProp` 在 prop 形态下必然
    // 回落到 `s_pDefaultSurfaceProp`，也就是 `$surfaceprop`）——
    // 实测 2498/2498 个 `.phy` 的 surfaceprop 恒等于 `.mdl` 头部 `+0x134`。
    params.surface_prop = surface_prop;
    // 只有真的走了 `$concave` 才写 `concave "1"`，与实测的 studiomdl 行为一致。
    params.concave = phys.concave;
    // 单 solid 的 prop 路径没有「joint」概念，所以只有全局默认值。
    params.damping = phys.damping.unwrap_or(0.0);
    params.rot_damping = phys.rot_damping.unwrap_or(0.0);
    params.inertia = phys.inertia.unwrap_or(1.0);
    if let Some(d) = phys.drag {
        params.drag = Some(d);
    }
    // `editparams.rootname`：**只有显式写了 `$rootbone` 才用**，
    // 缺省是空串（语料 39 个 ragdoll 里 13 个空）。
    if let Some(r) = &phys.root_bone {
        params.root_name = r;
    }
    let grouped = vec![hulls];
    // ⚠️ prop 形态的 `name` 是**碰撞 SMD 的 basename**，不是模型名 ——
    // 见 [`PhySolid::prop`] 的文档与受控实验 `gen_phyname.js`。
    let solids = vec![PhySolid::prop(collision_smd_name)];
    write_phy_multi(&grouped, &solids, &params).map_err(|e| format!("写出 PHY 失败：{e}"))
}

// ---------------------------------------------------------------------------
// `$concave`：官方的**连通分量分解**
// ---------------------------------------------------------------------------

/// 官方焊接判据的法线阈值：`cos(2°)`（`studiomdl.cpp:6895`
/// `normal_blend = cos( DEG2RAD( 2.0 ));`）。
///
/// 只有法线夹角**小于 2°** 的共位顶点才会被焊成一个 —— 所以
/// **平面着色**的网格（每个面自带法线）在硬边处焊不上。
fn normal_blend() -> f32 {
    (2.0f32).to_radians().cos()
}

/// 官方 `BuildVertWeldTable`（`collisionmodel.cpp:945-966`）。
///
/// 对每个顶点 `i`，在 `j < i` 里找**第一个**满足
/// 「位置**完全相同**（`VectorCompare` 是 `operator==`，逐分量 `==`）
/// **且** `dot(normal_j, normal_i) > normal_blend`」的 `j`，
/// 命中则 `weld[i] = j`，否则 `weld[i] = i`。
///
/// ⚠️ 位置比较是**精确相等**，不是 epsilon 近似 —— 与
/// [`weld_smd_triangles`] 的按位去重口径一致。
fn build_weld_table(verts: &[([f32; 3], [f32; 3])]) -> Vec<u32> {
    let blend = normal_blend();
    let mut weld: Vec<u32> = (0..verts.len() as u32).collect();
    for (i, &(pi, ni)) in verts.iter().enumerate() {
        for (j, &(pj, nj)) in verts.iter().enumerate().take(i) {
            if pi != pj {
                continue;
            }
            let d = ni[0] * nj[0] + ni[1] * nj[1] + ni[2] * nj[2];
            if d > blend {
                weld[i] = j as u32;
                break;
            }
        }
    }
    weld
}

/// 官方 `MarkConnectedMeshes`（`collisionmodel.cpp:975-1050`）。
///
/// 并查集式地把「通过**焊接后**的共享顶点相连」的面归到同一组，
/// 组的 ID 是该连通分量里**最小的面号**。
///
/// 被焊掉的顶点（`weld[i] != i`）直接标 `-1`（已处理），
/// 所以只有**代表元**参与分组。
///
/// 返回值：每个顶点的组 ID；`-1` 表示「已被合并，跳过」。
fn mark_connected_meshes(weld: &[u32], faces: &[[u32; 3]]) -> Vec<i32> {
    let num_faces = faces.len();
    let sentinel = (num_faces + 1) as i32;
    let mut vert_id: Vec<i32> = (0..weld.len())
        .map(|i| if weld[i] as usize != i { -1 } else { sentinel })
        .collect();

    loop {
        let mut marked = 0usize;
        for (faceid, f) in faces.iter().enumerate() {
            let a = weld[f[0] as usize] as usize;
            let b = weld[f[1] as usize] as usize;
            let c = weld[f[2] as usize] as usize;
            // `newid = MIN(faceid, vertID[a], vertID[b], vertID[c])`
            let newid = (faceid as i32).min(vert_id[a]).min(vert_id[b]).min(vert_id[c]);
            for &v in &[a, b, c] {
                if vert_id[v] != newid {
                    vert_id[v] = newid;
                    marked += 1;
                }
            }
        }
        if marked == 0 {
            break;
        }
    }
    vert_id
}

/// 官方 `IsApproximatelyPlanar`（`collisionmodel.cpp:1372-1422`）。
///
/// 少于 4 点直接算「平面」；否则找一个非退化的法线，把所有点投影上去，
/// 若厚度 `|max − min| > epsilon` 就说明是**三维**的。
///
/// 这个判据在 `$concave` 里起**否决**作用：任何一个分量被判成平面，
/// 官方就认定「模型没有 smoothing group」，**整份回退**成单凸包。
fn is_approximately_planar(points: &[[f32; 3]], epsilon: f32) -> bool {
    if points.len() < 4 {
        return true;
    }
    let mut v0 = 1usize;
    let mut v1 = 2usize;
    let mut normal = [0.0f32; 3];
    while v0 < points.len() && v1 < points.len() {
        let e0 = [
            points[v0][0] - points[0][0],
            points[v0][1] - points[0][1],
            points[v0][2] - points[0][2],
        ];
        let e1 = [
            points[v1][0] - points[0][0],
            points[v1][1] - points[0][1],
            points[v1][2] - points[0][2],
        ];
        normal = [
            e0[1] * e1[2] - e0[2] * e1[1],
            e0[2] * e1[0] - e0[0] * e1[2],
            e0[0] * e1[1] - e0[1] * e1[0],
        ];
        let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if len > 0.001 {
            break;
        }
        let e0_len = (e0[0] * e0[0] + e0[1] * e0[1] + e0[2] * e0[2]).sqrt();
        if e0_len < 0.001 {
            v0 += 1;
            v1 += 1;
        } else {
            v1 += 1;
        }
    }
    let dot = |p: &[f32; 3]| p[0] * normal[0] + p[1] * normal[1] + p[2] * normal[2];
    let mut min_d = dot(&points[0]);
    let mut max_d = min_d;
    for p in points {
        let d = dot(p);
        if d < min_d {
            min_d = d;
        } else if d > max_d {
            max_d = d;
        }
        if (max_d - min_d).abs() > epsilon {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// `$collisionjoints`：ragdoll（每骨骼一个 solid）
// ---------------------------------------------------------------------------

/// 一个 ragdoll solid 的分组结果。
#[derive(Debug, Clone, PartialEq)]
pub struct RagdollGroup {
    /// 骨骼在 SMD 里的下标。
    pub bone: u32,
    /// 骨骼名（原样，不做大小写变换）。
    pub name: String,
    /// **修正后**的父骨骼名。
    ///
    /// 不是骨骼的直接父 —— 而是沿骨骼链上溯找到的第一个
    /// 「也在碰撞列表里」的祖先（官方 `FixParent`）。
    pub parent: Option<String>,
    /// 该组的面（下标指向焊接后的面表）。
    pub faces: Vec<u32>,
}

/// 按**骨骼**把碰撞网格分组（官方 `ProcessJointedModel` 的前半）。
///
/// # 判据（`FaceHasVertOnBone`，`collisionmodel.cpp:778-822`）
///
/// 一个面只要**任一顶点**的**任一** `links[].bone` 命中该骨骼，整面归它。
/// 这是**面级**归属 —— 不是按顶点分，所以相邻骨骼会共享顶点。
///
/// # 父的修正（`FixParent`，`collisionmodel.cpp:1130-1151`）
///
/// 沿骨骼链**上溯**，返回第一个在分组集合里的祖先名；找不到则 `None`。
/// 语料印证：`anim_common` 的 `Spine2` 直接父是 `Spine1`（无碰撞几何），
/// `.phy` 里写的是 `Spine`。
///
/// # `$jointmerge` 的骨骼归并（`m_bonemap`）
///
/// `merged` 是 `子骨骼下标 → 归并目标骨骼下标`。官方
/// `MergeBones`（`collisionmodel.cpp:307-327`）把 `m_bonemap[child]` 指向
/// **父所在并查集的根**，随后：
///
/// * `ShouldProcessBone`（`:330-338`）只在 `m_bonemap[i] == i` 时返回 true
///   ⟹ **子骨骼不再单独成 solid**；
/// * `RemapBone`（`:355`）返回 `m_bonemap[i]`
///   ⟹ `FaceHasVertOnBone` 查父骨骼时，**权重落在子骨骼上的面也算命中**
///   ⟹ 子的碰撞几何**并入**父。
///
/// 实测（受控实验 `merge`）：`$jointmerge "bone_mid" "bone_tip"`
/// ⟹ solid 数 **3 → 2**，且 `bone_mid` 的体积变成两个盒子之和。
///
/// # 顺序
///
/// 按骨骼下标升序 —— 骨骼表本身就是「父在子前」的拓扑序，
/// 与官方 `SortCollisionList` 的结果一致。
pub fn group_by_bone(smd: &crate::smd::Smd) -> Vec<RagdollGroup> {
    group_by_bone_merged(smd, &std::collections::HashMap::new())
}

/// 若把 `child → parent` 加进 `merged` 会不会形成环。
///
/// 官方 `MergeBones`（`collisionmodel.cpp:312-324`）靠
/// `safety > m_pModel->numbones` 跳出死循环 —— 那是**静默放弃**。
/// mdlc 选择**显式报错**：环会让归并结果依赖遍历顺序，是用户写错了。
fn would_cycle(
    merged: &std::collections::HashMap<i32, i32>,
    parent: i32,
    child: i32,
    limit: usize,
) -> bool {
    // 从 parent 沿现有归并链上溯，若能走到 child ⟹ 加这条边就成环。
    let mut cur = parent;
    let mut guard = 0usize;
    loop {
        if cur == child {
            return true;
        }
        match merged.get(&cur) {
            Some(&next) if next != cur && guard <= limit => {
                cur = next;
                guard += 1;
            }
            _ => return false,
        }
    }
}

/// [`group_by_bone`] 的带归并版本。`merged` 是 `子骨骼下标 → 目标骨骼下标`。
pub fn group_by_bone_merged(
    smd: &crate::smd::Smd,
    merged: &std::collections::HashMap<i32, i32>,
) -> Vec<RagdollGroup> {
    // 骨骼下标 → 名 / 父下标
    let mut name_of: std::collections::HashMap<i32, String> = std::collections::HashMap::new();
    let mut parent_of: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    for n in &smd.nodes {
        name_of.insert(n.index, n.name.clone());
        parent_of.insert(n.index, n.parent);
    }

    // 归并：把 `child → target` 链解析到终点（防 `a→b→c` 只走一步）。
    // 官方 `MergeBones` 里那段 `while (m_bonemap[map] != map)` 就是干这个，
    // 并且带 `safety > numbones` 的死循环保护 —— 这里同样加上。
    let resolve = |mut b: i32| -> i32 {
        let mut guard = 0usize;
        while let Some(&t) = merged.get(&b) {
            if t == b || guard > smd.nodes.len() {
                break;
            }
            b = t;
            guard += 1;
        }
        b
    };

    // 每个面的归属：面只要有一个顶点绑到该骨骼就算它的。
    // ⚠️ 归属前先过一遍 `resolve` —— 这就是 `RemapBone` 的语义。
    let mut by_bone: std::collections::BTreeMap<i32, Vec<u32>> =
        std::collections::BTreeMap::new();
    for (fi, t) in smd.triangles.iter().enumerate() {
        let mut owners: Vec<i32> = Vec::new();
        for v in &t.vertices {
            for l in &v.links {
                let b = resolve(l.bone);
                if !owners.contains(&b) {
                    owners.push(b);
                }
            }
        }
        for b in owners {
            by_bone.entry(b).or_default().push(fi as u32);
        }
    }

    // 分组集合（用于 `FixParent` 的「是否在列表里」判断）。
    let in_list: std::collections::HashSet<i32> = by_bone.keys().copied().collect();

    let mut out = Vec::with_capacity(by_bone.len());
    for (bone, faces) in by_bone {
        // `FixParent`：沿链上溯到第一个在列表里的祖先。
        let mut parent: Option<String> = None;
        let mut cur = parent_of.get(&bone).copied().unwrap_or(-1);
        while cur >= 0 {
            if in_list.contains(&cur) {
                parent = name_of.get(&cur).cloned();
                break;
            }
            cur = parent_of.get(&cur).copied().unwrap_or(-1);
        }
        out.push(RagdollGroup {
            bone: bone as u32,
            name: name_of
                .get(&bone)
                .cloned()
                .unwrap_or_else(|| format!("bone{bone}")),
            parent,
            faces,
        });
    }
    out
}

/// 从 SMD 产出 **ragdoll** 形态的 `.phy`（官方 `$collisionjoints`）。
///
/// 每根「有碰撞几何」的骨骼一个 solid，`client_data = 骨骼下标 + 1`，
/// `parent` 经 `FixParent` 修正，`rootname` 取 `$rootbone`。
///
/// 骨骼**没有**碰撞几何时官方直接跳过（`if (vertCount)`），
/// 所以 solid 数可能少于骨骼数。
///
/// # 逐 joint 参数
///
/// `$jointdamping` / `$jointrotdamping` / `$jointinertia` / `$jointmassbias`
/// 作用于**单个** joint（`collisionmodel.cpp:658-692`），键是**骨骼名**。
/// 语料实测这些覆盖真的会用到（`rotdamping` 在 14/39 个多 solid 文件里
/// 逐 solid 不同），所以这里逐 solid 解析后写进 [`PhySolid`]。
pub fn build_ragdoll_phy_from_smd(
    smd: &crate::smd::Smd,
    ident: PhyIdentity<'_>,
    checksum: u32,
    mass: f32,
    phys: &crate::model::Physics,
) -> Result<Vec<u8>, String> {
    let PhyIdentity {
        model_name,
        surface_prop,
        ..
    } = ident;
    if smd.triangles.is_empty() {
        return Err("SMD 里没有任何三角形".to_string());
    }

    // ---- `$jointmerge`：先把骨骼归并表建出来（官方 `m_bonemap`） ----
    //
    // `MergeBones(parent, child)` 把 `m_bonemap[child]` 指向父所在并查集的根。
    // 效果：子骨骼不再单独成 solid，且权重落在子骨骼上的面**并入**父。
    // 键是**骨骼名**（`$jointmerge <父> <子>`），落盘时用原始名字。
    let mut merged: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    if !phys.merge.is_empty() {
        let idx_of: std::collections::HashMap<&str, i32> =
            smd.nodes.iter().map(|n| (n.name.as_str(), n.index)).collect();
        for (i, m) in phys.merge.iter().enumerate() {
            let (Some(&parent), Some(&child)) =
                (idx_of.get(m.a.as_str()), idx_of.get(m.b.as_str()))
            else {
                return Err(format!(
                    "[physics.merge][{i}] 的骨骼对 ({:?}, {:?}) 里有名字不在碰撞 SMD 的 \
                     nodes 里。可用的骨骼：{}",
                    m.a,
                    m.b,
                    {
                        let mut v: Vec<&str> = idx_of.keys().copied().collect();
                        v.sort_unstable();
                        v.join(", ")
                    }
                ));
            };
            if parent == child {
                continue;
            }
            // 官方 `MergeBones` 里带 `safety > numbones` 的死循环保护，
            // 这里也要防「互相归并」造成的环。
            if would_cycle(&merged, parent, child, smd.nodes.len()) {
                return Err(format!(
                    "[physics.merge][{i}] 的 ({:?}, {:?}) 会形成归并环",
                    m.a, m.b
                ));
            }
            merged.insert(child, parent);
        }
    }

    let groups = group_by_bone_merged(smd, &merged);
    if groups.is_empty() {
        return Err("没有任何骨骼带碰撞几何".to_string());
    }

    // `joint_overrides` 只对 ragdoll 有意义 —— 单 solid 的 prop 路径
    // 没有「joint」概念（`InitCollisionModel` 只在 `ProcessJointedModel`
    // 里被调用）。写了却不生效是最难查的一类问题，所以**显式报错**。
    if !phys.joint_overrides.is_empty() {
        let known: std::collections::HashSet<&str> =
            groups.iter().map(|g| g.name.as_str()).collect();
        for o in &phys.joint_overrides {
            if !known.contains(o.bone.as_str()) {
                return Err(format!(
                    "[physics.joint_overrides] 里的骨骼 {:?} 没有碰撞几何。\
                     有碰撞几何的是：{}",
                    o.bone,
                    {
                        let mut v: Vec<&str> = known.iter().copied().collect();
                        v.sort_unstable();
                        v.join(", ")
                    }
                ));
            }
        }
    }

    // 每组：把该组的面重映射到紧凑顶点池，再算凸包。
    let mut grouped: Vec<Vec<PhyHull>> = Vec::with_capacity(groups.len());
    let mut solids: Vec<PhySolid> = Vec::with_capacity(groups.len());
    let mut kept: Vec<&RagdollGroup> = Vec::with_capacity(groups.len());

    for g in &groups {
        // 该组的顶点（精确去重，与 `weld_smd_triangles` 同口径）。
        let mut verts: Vec<[f32; 3]> = Vec::new();
        let mut index_of: std::collections::HashMap<[u32; 3], u32> =
            std::collections::HashMap::new();
        for &fi in &g.faces {
            for v in &smd.triangles[fi as usize].vertices {
                let p = v.position;
                let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
                index_of.entry(key).or_insert_with(|| {
                    verts.push(p);
                    (verts.len() - 1) as u32
                });
            }
        }
        // 官方 `ConvexFromVerts` 在点数不足时返回 NULL ⟹ 该骨骼被丢弃。
        if verts.len() < 4 {
            continue;
        }
        let Ok(h) = PhyHull::from_points(&verts) else {
            continue;
        };
        grouped.push(vec![h.compact_outer_hull()]);
        // 逐 joint 参数：全局默认 + 该骨骼的覆盖（官方顺序：先默认后覆盖）。
        let (damping, rot_damping, inertia, mass_bias) = phys.resolve_joint(&g.name);
        let mut s = PhySolid::ragdoll(g.bone, g.name.clone(), g.parent.clone());
        // 只在**真的与全局默认不同**时才记成 per-solid 覆盖 ——
        // 这样 `PhySolid` 的 `None` 语义保持「继承默认」，写出的字节不变。
        if damping != phys.damping.unwrap_or(0.0) {
            s.damping = Some(damping);
        }
        if rot_damping != phys.rot_damping.unwrap_or(0.0) {
            s.rot_damping = Some(rot_damping);
        }
        if inertia != phys.inertia.unwrap_or(1.0) {
            s.inertia = Some(inertia);
        }
        s.mass_bias = mass_bias;
        solids.push(s);
        kept.push(g);
    }

    if grouped.is_empty() {
        return Err("所有骨骼的碰撞几何都张不成凸包".to_string());
    }

    // 父名要指向**实际保留下来**的 solid；`FixParent` 已保证父在列表里，
    // 但那个父可能因为点数不足被上面丢弃 —— 再上溯一次兜底。
    let kept_names: std::collections::HashSet<&str> =
        kept.iter().map(|g| g.name.as_str()).collect();
    let name_to_parent: std::collections::HashMap<&str, i32> = smd
        .nodes
        .iter()
        .map(|n| (n.name.as_str(), n.parent))
        .collect();
    let index_to_name: std::collections::HashMap<i32, &str> =
        smd.nodes.iter().map(|n| (n.index, n.name.as_str())).collect();
    for s in solids.iter_mut() {
        let mut cur = s.parent.clone();
        while let Some(p) = cur {
            if kept_names.contains(p.as_str()) {
                s.parent = Some(p);
                break;
            }
            cur = name_to_parent
                .get(p.as_str())
                .copied()
                .filter(|&i| i >= 0)
                .and_then(|i| index_to_name.get(&i))
                .map(|x| (*x).to_string());
            if cur.is_none() {
                s.parent = None;
            }
        }
    }

    let mut params = PhyParams::new(model_name, checksum);
    params.total_mass = mass;
    // `surfaceprop` 从**模型头**取 —— ragdoll 的 solid 名是骨骼名，
    // 理论上 `GetSurfaceProp` 会沿父链找到 `$jointsurfaceprop`；
    // 但实测 **39/39 个 ragdoll 的 surfaceprop 逐 solid 完全相同**
    // （`probe_phy_surfaceprop_per_solid.js`），所以单个值就够。
    params.surface_prop = surface_prop;
    params.damping = phys.damping.unwrap_or(0.0);
    params.rot_damping = phys.rot_damping.unwrap_or(0.0);
    params.inertia = phys.inertia.unwrap_or(1.0);
    if let Some(d) = phys.drag {
        params.drag = Some(d);
    }
    // 关节约束 / 碰撞规则 / 动画摩擦 / jointmerge。
    params.constraints = phys.joint_constraints()?;
    params.animated_friction = phys.animated_friction();
    params.no_self_collisions = phys.no_self_collisions;
    params.collision_pairs = phys
        .collision_pairs
        .iter()
        .map(|p| (p.a.clone(), p.b.clone()))
        .collect();
    params.merge_list = phys.merge.iter().map(|p| (p.a.clone(), p.b.clone())).collect();
    if let Some(mc) = phys.mass_center {
        params.mass_center = Some(mc);
    }
    // `$automass` 需要材质密度表（`scripts/surfaceproperties_manifest.txt`），
    // mdlc 没有 —— **显式报错**而不是静默写一个错的总质量。
    if phys.auto_mass {
        return Err(
            "`[physics].auto_mass`（`$automass`）需要材质密度表\
             （`scripts/surfaceproperties_manifest.txt`）才能算出总质量，\
             mdlc 没有该表。请显式写 `mass = <值>`"
                .to_string(),
        );
    }
    // ⚠️ **`rootname` 默认是空串**，不要拿根 solid 的名去填。
    //
    // 依据：`editparams.rootname` 写的是 `g_JointedModel.m_rootName`，
    // 而它**只**由 `$rootbone` 设置（`CCmd_JointRoot`，
    // `collisionmodel.cpp:1745-1749`）。没有 `$rootbone` 就是空。
    //
    // 受控实验（`rjd1`/`rjd2`，无 `$rootbone`）实测官方产物
    // `"rootname" ""`。语料 39 个 ragdoll 里 13 个也是空的。
    //
    // `$rootbone " "`（一个空格）是常见写法（mikuw 的 QC 就是），
    // 落盘就是那个空格 —— **不要 trim**。
    if let Some(r) = &phys.root_bone {
        params.root_name = r;
    }
    write_phy_multi(&grouped, &solids, &params).map_err(|e| format!("写出 PHY 失败：{e}"))
}

/// 官方 `$concave` 的**凸块数上限**（`collisionmodel.cpp:1535`
/// `if ( elements.Size() > 20 )` → 回退成单凸包）。
const MAX_CONVEX_PIECES: usize = 20;

/// 算出每根骨骼的 `physicsbone`（`mstudiobone_t` `+0xAC`）。
///
/// # 语义（`collisionmodel.cpp:2141-2184`）
///
/// ```text
/// ① 先全置 -1
/// ② 碰撞列表里每个 solid → 该骨骼的 physicsBoneIndex = **solid 下标**
/// ③ 未置位的骨骼沿父链上溯，取第一个已置位的祖先的值
/// ④ 都找不到 → 0
/// ```
///
/// # 与 `build_ragdoll_phy_from_smd` 的**分组口径必须一致**
///
/// solid 下标就是 `group_by_bone()` 结果**经过同样的过滤之后**的序号：
/// 官方对「顶点数不足 4」或「张不成凸包」的骨骼**直接跳过**
/// （`if (vertCount)` / `ConvexFromVerts` 返回 NULL），被跳过的骨骼
/// **不占 solid 下标**。所以这里必须复刻同一套过滤，否则下标会错位。
///
/// # 返回值
///
/// `Some(vec)` 长度 = `bone_count`，`None` 表示**没有任何碰撞几何**
/// （此时官方留全 0）。
///
/// # 语料判据（`probe_physicsbone_ragdoll_split.js`）
///
/// 排除 18 个 checksum 不配对的陈旧产物后**完美二分**：
/// 多 solid **38/38 非平凡**、单 solid **2459/2459 全 0**。
/// 单 solid 时本函数天然给出全 0（只有一个 solid 下标 0），与语料一致。
pub fn physics_bone_table(
    smd: &crate::smd::Smd,
    bone_count: usize,
    bone_parents: &[i32],
) -> Option<Vec<i32>> {
    let groups = group_by_bone(smd);
    if groups.is_empty() {
        return None;
    }

    // 与 `build_ragdoll_phy_from_smd` **完全相同**的过滤。
    let mut kept: Vec<&RagdollGroup> = Vec::with_capacity(groups.len());
    for g in &groups {
        let mut verts: Vec<[f32; 3]> = Vec::new();
        let mut index_of: std::collections::HashMap<[u32; 3], u32> =
            std::collections::HashMap::new();
        for &fi in &g.faces {
            for v in &smd.triangles[fi as usize].vertices {
                let p = v.position;
                let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
                index_of.entry(key).or_insert_with(|| {
                    verts.push(p);
                    (verts.len() - 1) as u32
                });
            }
        }
        if verts.len() < 4 {
            continue;
        }
        if PhyHull::from_points(&verts).is_err() {
            continue;
        }
        kept.push(g);
    }
    if kept.is_empty() {
        return None;
    }

    // ② solid 下标 → 骨骼
    let mut pb = vec![-1i32; bone_count];
    for (solid_idx, g) in kept.iter().enumerate() {
        let b = g.bone as usize;
        if b < bone_count {
            pb[b] = solid_idx as i32;
        }
    }
    // ③ 沿父链上溯；④ 找不到 → 0
    for i in 0..bone_count {
        if pb[i] >= 0 {
            continue;
        }
        let mut cur = bone_parents.get(i).copied().unwrap_or(-1);
        let mut found = -1i32;
        while cur >= 0 {
            let c = cur as usize;
            if c >= bone_count {
                break;
            }
            if pb[c] >= 0 {
                found = pb[c];
                break;
            }
            cur = bone_parents.get(c).copied().unwrap_or(-1);
        }
        pb[i] = if found >= 0 { found } else { 0 };
    }
    Some(pb)
}

/// 官方 `$concave` 的分解：**连通分量分解**，不是体分解。
///
/// # ⚠️ 这与 VHACD 是**两种不同的算法**
///
/// 反编译 `ProcessSingleBody`（`studiomdl.exe` 的 `FUN_004055a0`）确认官方路径是：
///
/// ```text
/// $concave ⟹ BuildVertWeldTable   （位置相同 且 法线夹角 < 2° 才焊接）
///         ⟹ MarkConnectedMeshes   （按共享焊接顶点做并查集）
///         ⟹ 每个连通分量各算一个凸包 ConvexFromVerts
/// ```
///
/// 所以对**连通**的凹体（U 形、圆环面），官方 `$concave` 给出的就是
/// **整个网格的凸包** —— 凹处被**填平**，而**不是**被拆成多块。
/// 只有网格有**多个互不相连的壳**时才会产出多个凸块。
///
/// 这与 [`decompose_concave`]（VHACD 体逼近）**结果不同**，实测：
///
/// | 网格 | 官方 `$concave` | VHACD |
/// |---|---|---|
/// | 光滑圆环面（连通、凹） | **1** 块，体积 22728.12 | 12 块，体积 19343.18 |
/// | 两个分离光滑球 | **2** 块，体积 4020.83（= 单球的 2 倍） | — |
///
/// 因为官方是**填平**、VHACD 是**保留凹口**，两者不能互相替代 ——
/// 用 VHACD 冒充 `$concave` 会让玩家卡进本该实心的区域。
///
/// # 两条回退规则（都会退化成「整网格一个凸包」）
///
/// 1. **任一分量是平面** —— 官方认为模型没设 smoothing group，
///    报 `Bad collision model, check your smoothing groups` 并 `elements.Purge()`。
///    平面着色的立方体必然命中（角上三个面法线互相垂直，`dot = 0 < cos2°`，
///    焊不上 ⟹ 每个面各自成分量 ⟹ 每个分量都是平面）。
/// 2. **分量数 > 20** —— 报 `COSTLY COLLISION MODEL`。
///
/// 两种情况下产物里都写 `concave "1"`（`m_allowConcave` 仍然是 true），
/// 只是几何退化成单凸包 —— 与官方一致。
pub fn decompose_connected_components(
    smd: &crate::smd::Smd,
    pose_world: Option<&std::collections::HashMap<String, crate::bone_math::Matrix3x4>>,
) -> Result<Vec<PhyHull>, String> {
    // 展平成「逐面顶点」：SMD 的三角形顶点是逐面独立的。
    //
    // ⚠️ 位置要先搬到**世界空间**（官方 `ProcessSingleBody:1436`
    // `ConvertToWorldSpace` 发生在**一切**之前，焊接/连通性判定都在
    // 世界空间里做）。姿态取自序列第 0 帧。
    let mut verts: Vec<([f32; 3], [f32; 3])> = Vec::with_capacity(smd.triangles.len() * 3);
    let mut faces: Vec<[u32; 3]> = Vec::with_capacity(smd.triangles.len());
    let all_weights = vertex_bone_weights(smd);
    let rest = RestPose::of(smd);
    let mut flat = 0usize;
    for t in &smd.triangles {
        let base = verts.len() as u32;
        for v in &t.vertices {
            let links = all_weights.get(flat).map(|w| w.as_slice()).unwrap_or(&[]);
            flat += 1;
            let wp = world_point(v.position, links, &rest, pose_world);
            verts.push((wp, v.normal));
        }
        faces.push([base, base + 1, base + 2]);
    }
    if verts.len() < 4 {
        return Err(format!("只有 {} 个顶点，凸包至少要 4 个", verts.len()));
    }

    let weld = build_weld_table(&verts);
    let vert_id = mark_connected_meshes(&weld, &faces);

    // 官方：`for i in 0..numvertices`，跳过 `vertID[i] < 0 || > numfaces`，
    // 收集所有 `vertID[j] == id` 的点（并把它们标 -1 防止重复）。
    let num_faces = faces.len() as i32;
    let mut seen: Vec<bool> = vert_id.iter().map(|&v| v < 0 || v > num_faces).collect();
    let mut groups: Vec<Vec<[f32; 3]>> = Vec::new();
    for i in 0..verts.len() {
        if seen[i] {
            continue;
        }
        let id = vert_id[i];
        let mut pts: Vec<[f32; 3]> = Vec::new();
        for j in i..verts.len() {
            if vert_id[j] == id {
                pts.push(verts[j].0);
                seen[j] = true;
            }
        }
        // 官方 `if (vertCount > 2)` 才尝试建凸包。
        if pts.len() > 2 {
            groups.push(pts);
        }
    }

    // 回退规则 1：任一分量是平面 ⟹ 整份退化成单凸包。
    let any_planar = groups.iter().any(|g| is_approximately_planar(g, 0.5));
    // 回退规则 2：分量数超上限。
    let too_many = groups.len() > MAX_CONVEX_PIECES;

    if any_planar || too_many || groups.is_empty() {
        let all: Vec<[f32; 3]> = verts.iter().map(|v| v.0).collect();
        return Ok(vec![
            PhyHull::from_points(&all)
                .map_err(|e| format!("算整体凸包失败：{e}"))?
                .compact_outer_hull(),
        ]);
    }

    // 每个分量一个凸包。退化分量（`ConvexFromVerts` 返回 NULL）官方直接丢弃。
    //
    // ⚠️ 官方 `ProcessSingleBody:1518` 用的是 `ConvexFromVerts` ——
    // 那**包含** `BuildOuterHull(·, 0.01)` 压实，所以每个分量也要压。
    let mut hulls = Vec::with_capacity(groups.len());
    for g in &groups {
        if let Ok(h) = PhyHull::from_points(g) {
            hulls.push(h.compact_outer_hull());
        }
    }
    if hulls.is_empty() {
        let all: Vec<[f32; 3]> = verts.iter().map(|v| v.0).collect();
        return Ok(vec![
            PhyHull::from_points(&all)
                .map_err(|e| format!("算整体凸包失败：{e}"))?
                .compact_outer_hull(),
        ]);
    }
    Ok(hulls)
}

/// 自检通过后返回的布局摘要。
///
/// 单元测试用它断言 §10.1 的长度恒等式；CLI 用它打印人类可读的统计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhyLayout {
    /// 文件总字节数。
    pub file_size: usize,
    /// `phyheader_t::solidCount`。
    pub solid_count: usize,
    /// text section 的起点（`= 16 + Σ(size_i + 4)`）。
    pub solids_end: usize,
    /// text section 的字节数（含结尾的 `0x00`）。
    pub text_size: usize,
    /// 每个 solid 的 `surfaceSize`。
    pub surface_sizes: Vec<i32>,
    /// 每个 solid 的树节点数。
    pub node_counts: Vec<usize>,
    /// 每个 solid 的 ledge 区域字节数（不含 `IVP_Compact_Surface` 的 48 字节）。
    pub ledge_region_sizes: Vec<usize>,
}

// ---------------------------------------------------------------------------
// 公开写出入口
// ---------------------------------------------------------------------------

/// 每个 solid 一个凸块，写出完整的 `.phy` 字节。
///
/// 这是最常用的入口：单 solid prop、以及"每根骨骼一个凸包"的 ragdoll
/// 都走它。`hulls[i]` 对应 `solids[i]`，顺序就是二进制里 solid 的顺序，
/// 也是 text section 里 `"index"` 的顺序。
///
/// 一个 solid 需要多个凸块时用 [`write_phy_multi`]。
///
/// # 质量分配（照抄 studiomdl `collisionmodel.cpp`）
///
/// ```text
/// volume     = Σ 各 solid 体积（<= 0 时置 1）
/// solid.mass = (solid.volume × massbias / volume) × totalmass
/// if solid.mass < 1.0 { solid.mass = 1.0 }      // 下限钳制
/// ```
///
/// 单 solid 时这退化成 `mass == totalmass`，与真实文件一致
/// （`cone_helper` 的 `mass` 与 `totalmass` 都是 19.422348）。
///
/// # 返回值
///
/// 写出的字节**一定**已经过 [`check_invariants`] 全量校验；任何布局回归
/// 都会在这里炸掉，而不是等到引擎加载时才发现。
pub fn write_phy(
    hulls: &[PhyHull],
    solids: &[PhySolid],
    params: &PhyParams<'_>,
) -> Result<Vec<u8>, PhyError> {
    let grouped: Vec<Vec<PhyHull>> = hulls.iter().map(|h| vec![h.clone()]).collect();
    write_phy_multi(&grouped, solids, params)
}

/// 每个 solid 可以由**多个凸块**拼成，写出完整的 `.phy` 字节。
///
/// 多凸块时一个 solid 会写成递归 ledgetree（每个叶子挂一个 hull），
/// 所有凸块共用一份点数组。单个凸块时退化成单叶子节点树
/// （`offset_right_node = 0`），即 L4D2 里 621/774 个 solid 的形态。
///
/// 输入契约、质量分配与自检行为都与 [`write_phy`] 相同。
pub fn write_phy_multi(
    hulls: &[Vec<PhyHull>],
    solids: &[PhySolid],
    params: &PhyParams<'_>,
) -> Result<Vec<u8>, PhyError> {
    if hulls.is_empty() {
        return Err(PhyError::BadParameter {
            what: "hulls",
            detail: "至少需要一个 solid".into(),
        });
    }
    if hulls.len() != solids.len() {
        return Err(PhyError::BadParameter {
            what: "solids",
            detail: format!(
                "{} 个 solid 凸块组但 {} 个 solid 参数",
                hulls.len(),
                solids.len()
            ),
        });
    }
    if !params.total_mass.is_finite() || params.total_mass <= 0.0 {
        return Err(PhyError::BadParameter {
            what: "total_mass",
            detail: format!("必须是正的有限数，实际 {}", params.total_mass),
        });
    }
    if !params.inertia_scale.is_finite() || params.inertia_scale < 0.0 {
        return Err(PhyError::BadParameter {
            what: "inertia_scale",
            detail: format!("必须是非负有限数，实际 {}", params.inertia_scale),
        });
    }
    // text section 里任何字段混进 NUL 都会把它截断，引擎只会读到半截。
    for (i, s) in solids.iter().enumerate() {
        for (what, v) in [
            ("name", &s.name),
            ("parent", s.parent.as_ref().unwrap_or(&String::new())),
        ] {
            if v.as_bytes().contains(&0) {
                return Err(PhyError::BadParameter {
                    what: "solids[].name/parent",
                    detail: format!("solid[{i}] 的 {what} 里有 NUL 字节"),
                });
            }
        }
    }

    // ---- 0. 逐个 solid 预处理几何 ----
    let prepared: Vec<PreparedSolid> = hulls
        .iter()
        .enumerate()
        .map(|(i, hs)| prepare_solid(i, hs, params.inertia_scale))
        .collect::<Result<_, _>>()?;

    // ---- 1. 逐个 solid 组装 IVP 负载 ----
    let bodies: Vec<Vec<u8>> = prepared
        .iter()
        .enumerate()
        .map(|(i, p)| build_solid_body(i, p, solids[i].bone_index))
        .collect::<Result<_, _>>()?;

    // ---- 2. text section ----
    let text = build_text_section(&prepared, solids, params)?;

    // ---- 3. 组装 ----
    let mut out = Vec::with_capacity(PHY_HEADER_SIZE + text.len() + 64);
    put_i32(&mut out, PHY_HEADER_SIZE as i32); // size：恒为 sizeof(phyheader_t)
    put_i32(&mut out, 0); // id：恒为 0
    put_i32(
        &mut out,
        i32::try_from(prepared.len()).map_err(|_| PhyError::BadParameter {
            what: "hulls",
            detail: "solid 数超出 i32".into(),
        })?,
    );
    put_u32(&mut out, params.checksum);

    for (p, body) in prepared.iter().zip(&bodies) {
        // 每 solid 的 32 字节头。`size = surfaceSize + 28`，所以 solid 记录的
        // 步长是 size + 4 == surfaceSize + 32，与实测的 stride 规则一致。
        put_i32(&mut out, p.surface_size + 28);
        put_u32(&mut out, VPHYSICS_ID);
        put_u16(&mut out, VPHYSICS_COLLISION_VERSION);
        put_i16(&mut out, COLLIDE_POLY);
        put_i32(&mut out, p.surface_size);
        // dragAxisAreas：实测都在 (0, 1]，保守填 1。只影响空气阻力。
        put_f32(&mut out, 1.0);
        put_f32(&mut out, 1.0);
        put_f32(&mut out, 1.0);
        put_i32(&mut out, 0); // axisMapSize：源码注释 "not yet supported"
        out.extend_from_slice(body);
    }

    out.extend_from_slice(&text);

    // ---- 4. 自检 ----
    //
    // 每次写出都跑一遍完整校验。这不是"测试代码"——它是防止未来改动悄悄
    // 破坏某个偏移的唯一防线，代价只有 O(文件大小)。
    check_invariants(&out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// 预处理
// ---------------------------------------------------------------------------

/// 单个凸块预处理后的形态（下标已指向 solid 的共享点数组）。
struct PreparedHull {
    /// 三角面，`start_point_index` 是共享数组里的全局下标。
    tris: Vec<[u32; 3]>,
    /// 反向边表，下标 `3k + i` → `(k', i')`。用来算 `opposite_index`。
    reverse: Vec<[u32; 2]>,
    /// 本 hull 引用的点（升序去重）。
    used: Vec<u32>,
    /// 本 ledge 在 ledge 区里的字节偏移。
    offset: usize,
    /// 本凸块自己的 AABB（叶子节点几何 + 内部节点合并都用它）。
    aabb: ([f32; 3], [f32; 3]),
    /// 本凸块的体积。
    volume: f32,
}

/// 单个 solid 预处理后的形态。
struct PreparedSolid {
    /// solid 级共享点数组（所有凸块的并集，按首次出现顺序）。
    points: Vec<[f32; 3]>,
    hulls: Vec<PreparedHull>,
    /// 树节点（`2n−1` 个；单凸块时就是 1 个叶子）。
    nodes: Vec<TreeNode>,
    /// ledge 区总字节数。
    ledge_region_size: usize,
    /// `surfaceSize`。
    surface_size: i32,
    /// `mass_center`（模型坐标）。
    mass_center: [f32; 3],
    /// `rotation_inertia`。
    rotation_inertia: [f32; 3],
    /// `upper_limit_radius` = `max‖p − mass_center‖`，对**共享点数组全部点**取。
    upper_limit_radius: f32,
    /// solid 体积（各凸块之和）。
    volume: f32,
}

/// 一个 ledgetree 节点。
struct TreeNode {
    /// 右子相对**本节点**的字节偏移；`0` 表示叶子。
    /// 左子恒为 `this + 28`（隐式），所以不存。
    right_offset: i32,
    /// 该节点对应的 hull 下标；`None` 表示内部节点。
    hull: Option<usize>,
    center: [f32; 3],
    radius: f32,
    box_sizes: [u8; 3],
}

/// 校验一个 solid 的凸块输入并整理成写出需要的形态。
fn prepare_solid(
    solid: usize,
    hulls: &[PhyHull],
    inertia_scale: f32,
) -> Result<PreparedSolid, PhyError> {
    if hulls.is_empty() {
        return Err(PhyError::BadParameter {
            what: "hulls",
            detail: format!("solid[{solid}] 一个凸块都没有"),
        });
    }

    // ---- 共享点数组：按首次出现顺序合并所有凸块的顶点 ----
    //
    // 实测：一个 solid 里所有 hull 共用一份点数组（2204/2204），
    // 每个 hull 的 `c_point_offset` 都指向它的起点。
    //
    // 去重只按"三个 float 的位模式完全相同"，**不做 epsilon 合并** ——
    // 后者会改动浮点值，而 `upper_limit_radius`、`box_sizes` 这些量
    // 必须与写进文件的点逐位对应。
    let mut points: Vec<[f32; 3]> = Vec::new();
    let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
    let mut prepared_hulls = Vec::with_capacity(hulls.len());
    let mut cursor = 0usize;

    for (hi, hull) in hulls.iter().enumerate() {
        // ---- 顶点合法性 ----
        if hull.vertices.len() < 4 {
            return Err(PhyError::HullTooFewPoints {
                solid,
                points: hull.vertices.len(),
            });
        }
        for (i, p) in hull.vertices.iter().enumerate() {
            if !p.iter().all(|c| c.is_finite()) {
                return Err(PhyError::NonFinitePoint { solid, point: i });
            }
        }
        if hull.faces.is_empty() {
            return Err(PhyError::HullDegenerate {
                solid,
                detail: format!("第 {hi} 个凸块的面表是空的"),
            });
        }
        if hull.faces.len() > MAX_TRIANGLES_PER_HULL {
            return Err(PhyError::TooManyTriangles {
                solid,
                triangles: hull.faces.len(),
                max: MAX_TRIANGLES_PER_HULL,
            });
        }
        for (fi, f) in hull.faces.iter().enumerate() {
            for &idx in f {
                if idx as usize >= hull.vertices.len() {
                    return Err(PhyError::IndexOutOfRange {
                        solid,
                        face: fi,
                        index: idx,
                        vertex_count: hull.vertices.len(),
                    });
                }
            }
        }
        // 闭合流形校验：`opposite_index` 依赖"每条有向边恰好一次 + 反向边存在"。
        // 输入有洞或有重复边时写出的文件不报错，但引擎邻接查询会走错，
        // 表现为穿模或卡住。宁可在编译期失败。
        //
        // 注意：**必须对重映射之后的三角形做校验**，不能对原始面表做。
        // 顶点去重会把坐标完全相同的不同下标合并成一个，如果某个面因此
        // 退化成 `[0,0,0]`，原始面表看起来是闭合的、写出的却是垃圾。
        // 第一版就在这里漏掉了，被 `degenerate_bounds_are_rejected` 抓到。

        // 顶点重映射到共享数组。
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(hull.faces.len());
        let mut used: Vec<u32> = Vec::new();
        for f in &hull.faces {
            let mut t = [0u32; 3];
            for (k, &idx) in f.iter().enumerate() {
                let v = hull.vertices[idx as usize];
                // ---- Source 世界空间 → IVP 世界空间 ----
                //
                // ⚠️ **不只是乘 0.0254**：官方还做了一次轴映射
                // `(-y, -z, x)`。见 [`source_to_ivp_axis`] / [`to_ivp`]，
                // 那里有反编译原文与三个独立实验的交叉验证。
                // 漏掉旋转会让碰撞体相对渲染网格转 90°。
                let q = to_ivp(v);
                let key = [q[0].to_bits(), q[1].to_bits(), q[2].to_bits()];
                let g = *seen.entry(key).or_insert_with(|| {
                    points.push(q);
                    (points.len() - 1) as u32
                });
                t[k] = g;
                if !used.contains(&g) {
                    used.push(g);
                }
            }
            tris.push(t);
        }
        used.sort_unstable();

        // 流形校验走重映射后的面表（见上）。
        let reverse = build_reverse_edges(&tris, solid)?;

        // ---- 几何量 ----
        //
        // 算质量属性时要把三角形重映射到**本凸块**的紧凑下标，因为
        // `MassProperties` 要求顶点表覆盖全部被引用的下标。
        let local_pts: Vec<Vector> = used
            .iter()
            .map(|&i| {
                let p = points[i as usize];
                Vector::new(p[0], p[1], p[2])
            })
            .collect();
        let local_of: HashMap<u32, u32> = used
            .iter()
            .enumerate()
            .map(|(li, &g)| (g, li as u32))
            .collect();
        let local_tris: Vec<[u32; 3]> = tris
            .iter()
            .map(|t| [local_of[&t[0]], local_of[&t[1]], local_of[&t[2]]])
            .collect();
        let mass_props = MassProperties::from_convex_polyhedron(1.0, &local_pts, &local_tris);
        // ---- 体积：**换算回 Source 单位（inch³）** ----
        //
        // ⚠️ `points` 已经是米，所以 `mass_props.mass()` 给的是 **m³**。
        // 但 `.phy` 的 **text 段 `"volume"` 是 inch³** —— 那是 studiomdl
        // **自己**用 `physcollision->ConvexVolume()`（Source 单位）算出来
        // 再 `fprintf` 的（`collisionmodel.cpp:1174` 累加 → `2310` 打印），
        // 与 IVP 的点数组**不是同一条路径**。
        //
        // 实测判据（`probe_phy_text_volume_unit.js`，官方 `msh1.phy`）：
        //
        // ```text
        // text 段 "volume"      = 16999.996094   ← 就是 17000 in³
        // 点数组算出的体积      = 0.278580 m³
        // 0.278580 / 0.0254³    = 17000.00       ← 换算回 inch³ 完全吻合
        // ```
        //
        // 所以这里**必须除回去**：`volume` 是「Source 单位的体积」，
        // 而 `points` 是「IVP 单位的点」。同一个文件里两种单位并存。
        let volume = mass_props.mass().abs() / (SOURCE_TO_IVP * SOURCE_TO_IVP * SOURCE_TO_IVP);

        // 叶子节点几何用**本凸块引用的点**，这是 2051/2051 实测精确的规则。
        let aabb = aabb_of(&points, &used, solid)?;

        prepared_hulls.push(PreparedHull {
            offset: cursor,
            tris,
            reverse,
            used,
            aabb,
            volume,
        });
        cursor += LEDGE_HEADER_SIZE + TRIANGLE_SIZE * prepared_hulls.last().unwrap().tris.len();
    }

    if points.len() > MAX_POINTS_PER_SOLID {
        return Err(PhyError::TooManyPoints {
            solid,
            points: points.len(),
        });
    }

    // ---- ledge 区 = 各 ledge 头+三角形，紧跟一份共享点数组 ----
    let ledge_region_size = cursor + POINT_SIZE * points.len();

    // ---- 碰撞树 ----
    let nodes = build_tree(&prepared_hulls);

    // surfaceSize == 48 + ledge_region_size + 28 × node_count。
    let surface_size_usize =
        COMPACT_SURFACE_SIZE + ledge_region_size + LEDGETREE_NODE_SIZE * nodes.len();
    let surface_size = i32::try_from(surface_size_usize).map_err(|_| PhyError::BadParameter {
        what: "surfaceSize",
        detail: format!("{surface_size_usize} 超出 i32"),
    })?;

    // ---- 整个 solid 的质量属性 ----
    //
    // `mass_center` / `rotation_inertia` 用**全部凸块合并后**的几何算，
    // 这样多凸块的 solid 也有正确的质心与惯性主轴。
    let all_pts: Vec<Vector> = points
        .iter()
        .map(|p| Vector::new(p[0], p[1], p[2]))
        .collect();
    let all_tris: Vec<[u32; 3]> = prepared_hulls
        .iter()
        .flat_map(|h| h.tris.iter().copied())
        .collect();
    let solid_props = MassProperties::from_trimesh(1.0, &all_pts, &all_tris);
    let mass_center_v = solid_props.local_com;
    let inertia_mat = solid_props.reconstruct_inertia_matrix();
    let rotation_inertia = [
        inertia_mat.x_axis.x * inertia_scale,
        inertia_mat.y_axis.y * inertia_scale,
        inertia_mat.z_axis.z * inertia_scale,
    ];

    // upper_limit_radius 以 mass_center 为中心、覆盖**共享点数组全部点**。
    let upper_limit_radius = points
        .iter()
        .map(|p| {
            let d = [
                p[0] - mass_center_v.x,
                p[1] - mass_center_v.y,
                p[2] - mass_center_v.z,
            ];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0f32, f32::max);

    let volume = prepared_hulls.iter().map(|h| h.volume).sum();

    Ok(PreparedSolid {
        points,
        hulls: prepared_hulls,
        nodes,
        ledge_region_size,
        surface_size,
        mass_center: [mass_center_v.x, mass_center_v.y, mass_center_v.z],
        rotation_inertia,
        upper_limit_radius,
        volume,
    })
}

// ---------------------------------------------------------------------------
// 碰撞树
// ---------------------------------------------------------------------------

/// 按"左子恒为 `this + 28`、右子为 `this + offset_right_node`"的隐式布局
/// 递归排布 ledgetree 节点。
///
/// 节点总数 `2n − 1`（`n` 是凸块数），**恒为奇数**——与实测的节点数分布一致
/// （1、3、5、…、149）。
///
/// 布局的关键约束是"左子树必须紧跟在父节点之后"，所以递归时先占位、
/// 再建左子树，最后回填右子偏移。
fn build_tree(hulls: &[PreparedHull]) -> Vec<TreeNode> {
    let mut out: Vec<TreeNode> = Vec::with_capacity(hulls.len() * 2);
    let order: Vec<usize> = (0..hulls.len()).collect();
    let (root, _, _) = build_subtree(hulls, &order, &mut out);
    debug_assert_eq!(root, 0);
    out
}

/// 递归建子树，返回 `(本子树根节点下标, 子树 AABB 的 min, max)`。
fn build_subtree(
    hulls: &[PreparedHull],
    idx: &[usize],
    out: &mut Vec<TreeNode>,
) -> (usize, [f32; 3], [f32; 3]) {
    let me = out.len();
    // 先占位，这样左子树一定从 `me + 1` 开始（左子恒为 this+28 的要求）。
    out.push(TreeNode {
        right_offset: 0,
        hull: None,
        center: [0.0; 3],
        radius: 0.0,
        box_sizes: [0; 3],
    });

    if idx.len() == 1 {
        let h = &hulls[idx[0]];
        let (mn, mx) = h.aabb;
        let (center, radius, box_sizes) = node_geometry(mn, mx);
        out[me] = TreeNode {
            right_offset: 0, // 叶子
            hull: Some(idx[0]),
            center,
            radius,
            box_sizes,
        };
        return (me, mn, mx);
    }

    // 对半切。左右都非空，所以递归一定会终止。
    let mid = idx.len() / 2;
    let (left, lmn, lmx) = build_subtree(hulls, &idx[..mid], out);
    debug_assert_eq!(left, me + 1);
    let (right, rmn, rmx) = build_subtree(hulls, &idx[mid..], out);

    let mn = [
        lmn[0].min(rmn[0]),
        lmn[1].min(rmn[1]),
        lmn[2].min(rmn[2]),
    ];
    let mx = [
        lmx[0].max(rmx[0]),
        lmx[1].max(rmx[1]),
        lmx[2].max(rmx[2]),
    ];
    let (center, radius, box_sizes) = node_geometry(mn, mx);
    out[me] = TreeNode {
        // 右子偏移必须是 28 的倍数，且为正（0 会被当成"叶子"）。
        right_offset: ((right - me) * LEDGETREE_NODE_SIZE) as i32,
        hull: None,
        center,
        radius,
        box_sizes,
    };
    (me, mn, mx)
}

/// 由 AABB 算节点的 `center` / `radius` / `box_sizes`。
///
/// `box_sizes[i] = trunc(半边长 / (radius/250)) + 1`。
///
/// `trunc` 不是 `round`：报告里暴力测试了 7 种候选公式，截断是唯一拟合的。
/// 全部走 f64 —— 校验器是 JS（f64），读回 f32 之后用 f64 运算，
/// 这里必须走同一条路径，否则 `box_sizes` 可能在网格边界上差 1 格。
fn node_geometry(mn: [f32; 3], mx: [f32; 3]) -> ([f32; 3], f32, [u8; 3]) {
    let half = [
        (mx[0] as f64 - mn[0] as f64) * 0.5,
        (mx[1] as f64 - mn[1] as f64) * 0.5,
        (mx[2] as f64 - mn[2] as f64) * 0.5,
    ];
    let center = [
        ((mn[0] as f64 + mx[0] as f64) * 0.5) as f32,
        ((mn[1] as f64 + mx[1] as f64) * 0.5) as f32,
        ((mn[2] as f64 + mx[2] as f64) * 0.5) as f32,
    ];
    let radius = ((half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt()) as f32;
    let mut box_sizes = [0u8; 3];
    if radius > 0.0 {
        let step = radius as f64 * BOUNDINGBOX_STEP;
        for (i, h) in half.iter().enumerate() {
            // half[i] <= radius 恒成立，所以 (h/step) <= 250，+1 <= 251。
            // clamp 只是防御性的，不会真的触发。
            box_sizes[i] = ((h / step).trunc() + 1.0).clamp(0.0, 255.0) as u8;
        }
    }
    (center, radius, box_sizes)
}

/// 组装一个 solid 的 IVP 负载：`IVP_Compact_Surface` + ledge 区 + 树。
fn build_solid_body(
    solid: usize,
    p: &PreparedSolid,
    bone_index: Option<u32>,
) -> Result<Vec<u8>, PhyError> {
    // client_data：有骨骼写 boneIndex + 1，无骨骼写 0。
    // 实测 90/90 零例外；`+1` 是为了让 0 表示"未绑定"。
    let client_data = match bone_index {
        Some(b) => {
            let plus_one = b.checked_add(1).ok_or_else(|| PhyError::BadParameter {
                what: "bone_index",
                detail: format!("骨骼下标 {b} +1 溢出 u32"),
            })?;
            i32::try_from(plus_one).map_err(|_| PhyError::BadParameter {
                what: "bone_index",
                detail: format!("骨骼下标 {b} +1 后超出 i32"),
            })?
        }
        None => 0,
    };

    let offset_ledgetree_root = (COMPACT_SURFACE_SIZE + p.ledge_region_size) as i32;

    // `c_point_offset` 的精确规则：从本 ledge 起、到最后一个 ledge 结束为止的
    // 所有 ledge 大小之和（见模块文档与下方注释）。
    let mut suffix_sizes: Vec<usize> = Vec::with_capacity(p.hulls.len());
    {
        let mut acc = 0usize;
        for h in p.hulls.iter().rev() {
            acc += LEDGE_HEADER_SIZE + TRIANGLE_SIZE * h.tris.len();
            suffix_sizes.push(acc);
        }
        suffix_sizes.reverse();
    }

    let mut body = Vec::with_capacity(p.surface_size as usize);

    // ---- IVP_Compact_Surface (48 B) ----
    put_vec3(&mut body, p.mass_center);
    put_vec3(&mut body, p.rotation_inertia);
    put_f32(&mut body, p.upper_limit_radius);
    // 位域：低 8 位 max_factor_surface_deviation，高 24 位 byte_size。
    // 容差取 250 —— 实测取值范围 135..251，250 是出现过的最大值。
    let byte_size = p.surface_size as u32 & 0x00FF_FFFF;
    put_u32(&mut body, (250u32 & 0xFF) | (byte_size << 8));
    put_i32(&mut body, offset_ledgetree_root);
    put_i32(&mut body, 0); // dummy[0]
    put_i32(&mut body, 0); // dummy[1]
    put_u32(&mut body, IVP_COMPACT_SURFACE_ID); // dummy[2] == 'IVPS'

    // ---- 各 ledge（头 + 三角形） ----
    //
    // `c_point_offset` 的**精确规则**（我在真实语料上复核过，见模块文档）：
    // 它是「本 ledge 到共享点数组起点」的距离，也就是
    // **从本 ledge 起、到最后一个 ledge 结束为止，所有 ledge 大小之和**。
    //
    // ```text
    // c_point_offset(i) = Σ_{j >= i} (16 + 16 × nTri_j)
    // ```
    //
    // 实测：2204/2204 个 hull 的 `ledge + c_point_offset` 都落在同一个地址
    // （共享点数组起点），其中 1583 个来自多 hull 的 solid。单 hull 时它退化
    // 成 `16 + 16×nTri` —— 这正是规格 §4.2 那条规则，也是它标注"774/774"
    // 的原因：那 774 个恰好全是单 hull 的 solid。
    for (hi, h) in p.hulls.iter().enumerate() {
        let c_point_offset = suffix_sizes[hi] as i32;
        // size_div_16 = 头(1) + 三角形 + 本 hull 引用的点数。
        let size_div_16 = 1 + h.tris.len() + h.used.len();

        put_i32(&mut body, c_point_offset);
        put_i32(&mut body, client_data);
        put_u32(
            &mut body,
            // has_chilren_flag:2 = 0（终结 ledge，所以 +0x04 当 client_data 用）
            // is_compact_flag:2 = 1
            (1u32 << 2)
                // dummy:4 = 0
                | ((size_div_16 as u32 & 0x00FF_FFFF) << 8),
        );
        put_i16(
            &mut body,
            i16::try_from(h.tris.len()).map_err(|_| PhyError::TooManyTriangles {
                solid,
                triangles: h.tris.len(),
                max: MAX_TRIANGLES_PER_HULL,
            })?,
        );
        put_i16(&mut body, 0); // for_future_use

        for (k, t) in h.tris.iter().enumerate() {
            // tri_index:12 | pierce_index:12 | material_index:7 | is_virtual:1
            // pierce_index 实测恒等于 tri_index；material_index 与 is_virtual 恒为 0。
            put_u32(&mut body, (k as u32 & 0x0FFF) | ((k as u32 & 0x0FFF) << 12));
            for (i, &start) in t.iter().enumerate() {
                // 边 i 的起点；终点是边 (i+1)%3 的起点（引擎的 next_table）。
                let r = h.reverse[3 * k + i];
                let (k2, i2) = (r[0], r[1]);
                let opp = (4 * k2 as i32 + 1 + i2 as i32) - (4 * k as i32 + 1 + i as i32);
                if !(-16384..16384).contains(&opp) {
                    return Err(PhyError::TooManyTriangles {
                        solid,
                        triangles: h.tris.len(),
                        max: MAX_TRIANGLES_PER_HULL,
                    });
                }
                put_u32(&mut body, (start & 0xFFFF) | (((opp as u32) & 0x7FFF) << 16));
            }
        }
    }

    // ---- 共享点数组 ----
    // 第 4 个 float（源码里别名成 `void *client_data`）实测是残留垃圾，
    // 引擎不读，写 0。
    for pt in &p.points {
        put_vec3(&mut body, *pt);
        put_f32(&mut body, 0.0);
    }

    // ---- 碰撞树 ----
    for (i, n) in p.nodes.iter().enumerate() {
        put_i32(&mut body, n.right_offset);
        // offset_compact_ledge 相对**本节点**：ledge 在 body+48，节点在
        // body+offset_ledgetree_root+28i，所以是 48 − 节点绝对偏移（恒为负）。
        match n.hull {
            Some(hi) => {
                let node_abs = offset_ledgetree_root + (i * LEDGETREE_NODE_SIZE) as i32;
                put_i32(&mut body, COMPACT_SURFACE_SIZE as i32 - node_abs + p.hulls[hi].offset as i32);
            }
            // 内部节点：offset_compact_ledge == 0 表示"本节点不对应 hull"。
            None => put_i32(&mut body, 0),
        }
        put_vec3(&mut body, n.center);
        put_f32(&mut body, n.radius);
        body.push(n.box_sizes[0]);
        body.push(n.box_sizes[1]);
        body.push(n.box_sizes[2]);
        body.push(0); // free_0
    }

    debug_assert_eq!(body.len(), p.surface_size as usize);
    Ok(body)
}

/// 建立有向边表、校验闭合可定向流形，并返回 `3k+i -> (k', i')` 的反向边表。
///
/// # 为什么必须校验
///
/// `opposite_index` 依赖"每条有向边恰好出现一次、且反向边也恰好出现一次"。
/// 输入有洞或有重复边时，写出的文件不会报错，但引擎的邻接查询会走错，
/// 表现为穿模或卡住。宁可在编译期失败，也不要产出这种文件。
fn build_reverse_edges(faces: &[[u32; 3]], solid: usize) -> Result<Vec<[u32; 2]>, PhyError> {
    let mut dir: HashMap<(u32, u32), (u32, u32)> = HashMap::with_capacity(faces.len() * 3);
    for (k, f) in faces.iter().enumerate() {
        for i in 0..3 {
            let (u, v) = (f[i], f[(i + 1) % 3]);
            // 退化三角形（两个下标相同）也会走到这里：它产生自环边。
            if u == v || dir.insert((u, v), (k as u32, i as u32)).is_some() {
                return Err(PhyError::DuplicateEdge {
                    solid,
                    from: u,
                    to: v,
                });
            }
        }
    }
    let mut out = Vec::with_capacity(faces.len() * 3);
    for f in faces {
        for i in 0..3 {
            let (u, v) = (f[i], f[(i + 1) % 3]);
            match dir.get(&(v, u)) {
                Some(&(k2, i2)) => out.push([k2, i2]),
                None => {
                    return Err(PhyError::OpenMesh {
                        solid,
                        from: u,
                        to: v,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// 一组点（按下标子集）的 AABB。
fn aabb_of(
    points: &[[f32; 3]],
    used: &[u32],
    solid: usize,
) -> Result<([f32; 3], [f32; 3]), PhyError> {
    let first = used.first().ok_or(PhyError::HullDegenerate {
        solid,
        detail: "凸包没有引用任何顶点".into(),
    })?;
    let mut mn = points[*first as usize];
    let mut mx = mn;
    for &i in used {
        let p = points[i as usize];
        for a in 0..3 {
            if p[a] < mn[a] {
                mn[a] = p[a];
            }
            if p[a] > mx[a] {
                mx[a] = p[a];
            }
        }
    }
    Ok((mn, mx))
}

// ---------------------------------------------------------------------------
// text section
// ---------------------------------------------------------------------------

/// 生成 text section（含结尾的单个 `0x00`）。
///
/// 格式实测自 331/331 个真实文件：以 `solid {` 开头、以 `}\n` 结尾、
/// 紧跟一个 `0x00`、内部没有别的 NUL。KeyValues 的键和值**都带引号**，
/// `\n` 分行，**没有逗号也没有分号**；`solid {` / `editparams {` 这类块名不带引号。
fn build_text_section(
    prepared: &[PreparedSolid],
    solids: &[PhySolid],
    params: &PhyParams<'_>,
) -> Result<Vec<u8>, PhyError> {
    // ---- 质量分配（studiomdl collisionmodel.cpp） ----
    let total_volume: f32 = prepared.iter().map(|p| p.volume).sum();
    let volume = if total_volume > 0.0 { total_volume } else { 1.0 };
    let masses: Vec<f32> = prepared
        .iter()
        .zip(solids)
        .map(|(p, s)| {
            let m = (p.volume * s.mass_bias / volume) * params.total_mass;
            // studiomdl：`if (pPhys->m_mass < 1.0) pPhys->m_mass = 1.0;`
            if m < 1.0 { 1.0 } else { m }
        })
        .collect();

    let mut s = String::new();
    for (i, (p, solid)) in prepared.iter().zip(solids).enumerate() {
        s.push_str("solid {\n");
        s.push_str(&format!("\"index\" \"{i}\"\n"));
        s.push_str(&format!("\"name\" \"{}\"\n", solid.name));
        if let Some(parent) = &solid.parent {
            s.push_str(&format!("\"parent\" \"{parent}\"\n"));
        }
        s.push_str(&format!("\"mass\" \"{:.6}\"\n", masses[i]));
        s.push_str(&format!("\"surfaceprop\" \"{}\"\n", params.surface_prop));
        // 逐 solid 的值 = 本 solid 的覆盖（`$jointdamping` 等）否则全局默认。
        let damping = solid.damping.unwrap_or(params.damping);
        let rot_damping = solid.rot_damping.unwrap_or(params.rot_damping);
        let inertia = solid.inertia.unwrap_or(params.inertia);
        s.push_str(&format!("\"damping\" \"{damping:.6}\"\n"));
        s.push_str(&format!("\"rotdamping\" \"{rot_damping:.6}\"\n"));
        if let Some(drag) = params.drag {
            s.push_str(&format!("\"drag\" \"{drag:.6}\"\n"));
        }
        s.push_str(&format!("\"inertia\" \"{inertia:.6}\"\n"));
        s.push_str(&format!("\"volume\" \"{:.6}\"\n", p.volume));
        // 实测：只有 massbias != 1.0 才写这一行。
        if (solid.mass_bias - 1.0).abs() > f32::EPSILON {
            s.push_str(&format!("\"massbias\" \"{:.6}\"\n", solid.mass_bias));
        }
        s.push_str("}\n");
    }

    // ---- 有 parent 的 solid 各写一个 ragdollconstraint ----
    //
    // 实测 `parent` / `child` 都是 **solid 的 index**（不是骨骼下标）。
    //
    // ⚠️ **轴值默认全是 0**，不是「不限位」的 ±180。
    //
    // 依据：`BuildRagdollConstraint`（`collisionmodel.cpp:2215-2226`）的
    // **第一句**就是 `memset( &ragdoll, 0, sizeof(ragdoll) );` ——
    // 只有 QC 写了 `$jointconstrain` 才会把某个轴改成别的值。
    //
    // 约束挂在**子** solid 上：官方遍历整条约束链，只应用
    // `CollisionIndex(pList->m_pJointName) == ragdoll.childIndex` 的那些
    // （`collisionmodel.cpp:2242`）。同一轴后写覆盖先写。
    for (i, solid) in solids.iter().enumerate() {
        let Some(parent_name) = &solid.parent else {
            continue;
        };
        // 父 solid 按名字查找；找不到就退回 0（根），而不是 panic。
        let parent_idx = solids
            .iter()
            .position(|s| &s.name == parent_name)
            .unwrap_or(0);

        // 本 solid 的九个轴值，从默认全 0 起步。
        let mut axes = [(0.0f32, 0.0f32, 0.0f32); 3];
        for c in &params.constraints {
            // 约束按**骨骼名**匹配本 solid 的 `name`（ragdoll 的 name 就是骨骼名）。
            if c.bone != solid.name {
                continue;
            }
            let a = (c.axis as usize).min(2);
            axes[a] = c.kind.axis_values(c.min, c.max, c.friction);
        }

        s.push_str("ragdollconstraint {\n");
        s.push_str(&format!("\"parent\" \"{parent_idx}\"\n"));
        s.push_str(&format!("\"child\" \"{i}\"\n"));
        for (k, axis) in ["x", "y", "z"].iter().enumerate() {
            let (lo, hi, fr) = axes[k];
            s.push_str(&format!("\"{axis}min\" \"{lo:.6}\"\n"));
            s.push_str(&format!("\"{axis}max\" \"{hi:.6}\"\n"));
            s.push_str(&format!("\"{axis}friction\" \"{fr:.6}\"\n"));
        }
        s.push_str("}\n");
    }

    // ---- collisionrules（`$noselfcollisions` 优先于 `$jointcollide`） ----
    //
    // 官方 `collisionmodel.cpp:2422-2447`：`if (noSelfCollisions) {...}
    // else if (m_pCollisionPairs) {...}` —— **互斥且 noself 优先**。
    if params.no_self_collisions {
        s.push_str("collisionrules {\n");
        s.push_str("\"selfcollisions\" \"0\"\n");
        s.push_str("}\n");
    } else if !params.collision_pairs.is_empty() {
        s.push_str("collisionrules {\n");
        for (a, b) in &params.collision_pairs {
            // 官方把骨骼名解析成 solid 下标，**任一侧找不到就丢弃该对**
            // （`CollisionIndex` 返回 -1；`obj0 != obj1` 也是必要条件）。
            let ia = solids.iter().position(|s| &s.name == a);
            let ib = solids.iter().position(|s| &s.name == b);
            if let (Some(x), Some(y)) = (ia, ib)
                && x != y
            {
                s.push_str(&format!("\"collisionpair\" \"{x},{y}\"\n"));
            }
        }
        s.push_str("}\n");
    }

    // ---- animatedfriction ----
    if let Some(af) = &params.animated_friction {
        s.push_str("animatedfriction {\n");
        // 两个 *friction* 字段官方是 `Safe_atoi` 出来的**整数**，
        // 但用 `KeyWriteFloat` 写 ⟹ 落盘形如 `"100.000000"`。
        s.push_str(&format!("\"animfrictionmin\" \"{:.6}\"\n", af.min as f32));
        s.push_str(&format!("\"animfrictionmax\" \"{:.6}\"\n", af.max as f32));
        s.push_str(&format!("\"animfrictiontimein\" \"{:.6}\"\n", af.time_in));
        s.push_str(&format!("\"animfrictiontimeout\" \"{:.6}\"\n", af.time_out));
        s.push_str(&format!("\"animfrictiontimehold\" \"{:.6}\"\n", af.time_hold));
        s.push_str("}\n");
    }

    // ---- editparams ----
    s.push_str("editparams {\n");
    s.push_str(&format!("\"rootname\" \"{}\"\n", params.root_name));
    s.push_str(&format!("\"totalmass\" \"{:.6}\"\n", params.total_mass));
    if params.concave {
        s.push_str("\"concave\" \"1\"\n");
    }
    // `jointmerge` 用的是 **QC 里写的原始名字**（`Q_snprintf("%s,%s")`），
    // 不是解析后的骨骼名 —— 实测 `"jointmerge" "bone_mid,bone_tip"`。
    for (parent, child) in &params.merge_list {
        s.push_str(&format!("\"jointmerge\" \"{parent},{child}\"\n"));
    }
    s.push_str("}\n");

    let mut bytes = s.into_bytes();
    bytes.push(0); // 单个 NUL 终止符
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// 自检
// ---------------------------------------------------------------------------

/// 对已写出的 `.phy` 字节跑完整的不变量校验。
///
/// 检查的就是 `docs/phy-format-research.md` §10.1 末尾那 13 条硬约束
/// （外加 `client_data` 与共享点数组两条）。[`write_phy`] 每次都会调用它，
/// 所以任何布局回归都会在写出的那一刻炸掉，而不是等到引擎加载时才发现。
pub fn check_invariants(bytes: &[u8]) -> Result<PhyLayout, PhyError> {
    let bad = |m: String| Err(PhyError::SelfCheck(m));

    if bytes.len() < PHY_HEADER_SIZE {
        return bad(format!("文件只有 {} 字节，连 16 字节头都放不下", bytes.len()));
    }
    // 约束 12：phyheader.size == 16、id == 0。
    let header_size = read_i32(bytes, 0);
    if header_size != PHY_HEADER_SIZE as i32 {
        return bad(format!("phyheader.size 应为 16，实际 {header_size}"));
    }
    let header_id = read_i32(bytes, 4);
    if header_id != 0 {
        return bad(format!("phyheader.id 应为 0，实际 {header_id}"));
    }
    let solid_count = read_i32(bytes, 8);
    if solid_count < 0 {
        return bad(format!("phyheader.solidCount 为负：{solid_count}"));
    }
    let solid_count = solid_count as usize;

    let mut base = PHY_HEADER_SIZE;
    let mut surface_sizes = Vec::with_capacity(solid_count);
    let mut node_counts = Vec::with_capacity(solid_count);
    let mut ledge_region_sizes = Vec::with_capacity(solid_count);

    for si in 0..solid_count {
        if base + SOLID_HEADER_SIZE > bytes.len() {
            return bad(format!("solid[{si}] 的头超出文件末尾"));
        }
        let size = read_i32(bytes, base);
        let vphysics_id = read_u32(bytes, base + 4);
        let version = read_u16(bytes, base + 8);
        let model_type = read_i16(bytes, base + 10);
        let surface_size = read_i32(bytes, base + 12);
        let axis_map_size = read_i32(bytes, base + 28);

        // 约束 11。
        if vphysics_id != VPHYSICS_ID {
            return bad(format!(
                "solid[{si}] vphysicsID 应为 'VPHY'，实际 {vphysics_id:#010x}"
            ));
        }
        if version != VPHYSICS_COLLISION_VERSION {
            return bad(format!("solid[{si}] version 应为 0x0100，实际 {version:#06x}"));
        }
        if model_type != COLLIDE_POLY {
            return bad(format!("solid[{si}] modelType 应为 0，实际 {model_type}"));
        }
        if axis_map_size != 0 {
            return bad(format!("solid[{si}] axisMapSize 应为 0，实际 {axis_map_size}"));
        }
        // 约束 2。
        if size - surface_size != 28 {
            return bad(format!(
                "solid[{si}] size - surfaceSize 应为 28，实际 {size} - {surface_size}"
            ));
        }
        if surface_size < (COMPACT_SURFACE_SIZE + LEDGETREE_NODE_SIZE) as i32 {
            return bad(format!("solid[{si}] surfaceSize 太小：{surface_size}"));
        }

        let body = base + SOLID_HEADER_SIZE;
        let solid_end = body + surface_size as usize;
        if solid_end > bytes.len() {
            return bad(format!(
                "solid[{si}] 越过文件末尾（需要 {solid_end}，只有 {}）",
                bytes.len()
            ));
        }
        let region = body + COMPACT_SURFACE_SIZE;

        // 约束 3：offset_ledgetree_root 相对 IVP_Compact_Surface 起点。
        let offset_ledgetree_root = read_i32(bytes, body + 32);
        if offset_ledgetree_root < COMPACT_SURFACE_SIZE as i32 {
            return bad(format!(
                "solid[{si}] offset_ledgetree_root={offset_ledgetree_root} 落在 surface 内部"
            ));
        }
        let node_off = body + offset_ledgetree_root as usize;
        if node_off + LEDGETREE_NODE_SIZE > solid_end {
            return bad(format!(
                "solid[{si}] offset_ledgetree_root={offset_ledgetree_root} 让树越过 solid 末尾"
            ));
        }
        let node_len = solid_end - node_off;
        if !node_len.is_multiple_of(LEDGETREE_NODE_SIZE) {
            return bad(format!("solid[{si}] 树区长度 {node_len} 不是 28 的倍数"));
        }
        let node_count = node_len / LEDGETREE_NODE_SIZE;

        // 约束 1：surfaceSize == 48 + ledge_region_size + 28 × node_count。
        let ledge_region_size = node_off - region;
        if surface_size as usize
            != COMPACT_SURFACE_SIZE + ledge_region_size + LEDGETREE_NODE_SIZE * node_count
        {
            return bad(format!(
                "solid[{si}] surfaceSize={surface_size} != 48 + {ledge_region_size} + 28×{node_count}"
            ));
        }
        if ledge_region_size == 0 {
            return bad(format!("solid[{si}] ledge 区长度为 0"));
        }

        // byte_size == surfaceSize、max_factor_surface_deviation > 0、dummy[2] == 'IVPS'。
        let packed = read_u32(bytes, body + 28);
        if ((packed >> 8) & 0x00FF_FFFF) != surface_size as u32 {
            return bad(format!(
                "solid[{si}] IVP_Compact_Surface::byte_size != surfaceSize"
            ));
        }
        if (packed & 0xFF) == 0 {
            return bad(format!("solid[{si}] max_factor_surface_deviation 为 0"));
        }
        if read_u32(bytes, body + 44) != IVP_COMPACT_SURFACE_ID {
            return bad(format!("solid[{si}] dummy[2] 应为 'IVPS'"));
        }
        if read_i32(bytes, body + 36) != 0 || read_i32(bytes, body + 40) != 0 {
            return bad(format!("solid[{si}] dummy[0]/dummy[1] 应为 0"));
        }

        // ---- 走一遍树，收集 hull ----
        //
        // `offset_compact_ledge` **恒为负**（ledge 在低地址），所以这里
        // 必须用 i64 做有符号加法。第一版写成 `off + rel as usize`，
        // 负 rel 转成巨大的 usize 后立刻算术溢出 panic。
        let mut hull_offsets: Vec<usize> = Vec::new();
        let mut visited = vec![false; node_count];
        let mut stack = vec![node_off];
        while let Some(off) = stack.pop() {
            if off < node_off || off + LEDGETREE_NODE_SIZE > solid_end {
                return bad(format!("solid[{si}] 树节点 @{off} 越界"));
            }
            let idx = (off - node_off) / LEDGETREE_NODE_SIZE;
            if visited[idx] {
                return bad(format!("solid[{si}] 树里有环 @{off}"));
            }
            visited[idx] = true;
            let right = read_i32(bytes, off);
            let rel = read_i32(bytes, off + 4);
            if rel != 0 {
                let lo = off as i64 + rel as i64;
                if lo < region as i64 || lo + LEDGE_HEADER_SIZE as i64 > node_off as i64 {
                    return bad(format!("solid[{si}] hull @{lo} 落在 ledge 区之外"));
                }
                let lo = lo as usize;
                if !hull_offsets.contains(&lo) {
                    hull_offsets.push(lo);
                }
            }
            if right != 0 {
                if right % LEDGETREE_NODE_SIZE as i32 != 0 {
                    return bad(format!(
                        "solid[{si}] offset_right_node={right} 不是 28 的倍数"
                    ));
                }
                // 左子恒为 this + 28。
                stack.push(off + LEDGETREE_NODE_SIZE);
                let r = off as i64 + right as i64;
                if r < node_off as i64 || r + LEDGETREE_NODE_SIZE as i64 > solid_end as i64 {
                    return bad(format!("solid[{si}] 右子节点 @{r} 越界"));
                }
                stack.push(r as usize);
            }
        }
        if visited.iter().any(|v| !v) {
            return bad(format!(
                "solid[{si}] 有 {} 个树节点没被走到",
                visited.iter().filter(|v| !**v).count()
            ));
        }
        if hull_offsets.is_empty() {
            return bad(format!("solid[{si}] 没有任何 hull"));
        }
        hull_offsets.sort_unstable();

        // ---- 约束：ledge 紧密排列 ----
        let mut cursor = region;
        for &lo in &hull_offsets {
            if lo != cursor {
                return bad(format!("solid[{si}] ledge 有空隙：期望 {cursor}，实际 {lo}"));
            }
            let n_tri = read_i16(bytes, lo + 12);
            if n_tri <= 0 {
                return bad(format!("solid[{si}] hull @{lo} 的 n_triangles={n_tri}"));
            }
            cursor = lo + LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri as usize;
            if cursor > node_off {
                return bad(format!("solid[{si}] hull @{lo} 的三角形越过树起点"));
            }
        }
        // ---- 共享点数组正好填满 [最后一个 ledge 结束, 树起点) ----
        let shared_base = cursor;
        // 约束 10：16 字节对齐。
        if !(node_off - shared_base).is_multiple_of(POINT_SIZE) {
            return bad(format!("solid[{si}] 共享点数组不是 16 字节对齐"));
        }
        let n_points = (node_off - shared_base) / POINT_SIZE;
        if n_points == 0 {
            return bad(format!("solid[{si}] 共享点数组是空的"));
        }

        for &lo in &hull_offsets {
            // 约束 4：`c_point_offset` 是「从本 ledge 起、到最后一个 ledge
            // 结束为止的所有 ledge 大小之和」，也就是本 ledge 到共享点数组
            // 起点的距离。单 hull 时退化成 `16 + 16×nTri`（规格 §4.2）。
            //
            // 这里按定义式验证：累加本 ledge 及其后所有 ledge 的大小。
            let mut expect_cpo = 0i64;
            for &l2 in hull_offsets.iter().filter(|&&l| l >= lo) {
                expect_cpo += (LEDGE_HEADER_SIZE + TRIANGLE_SIZE * read_i16(bytes, l2 + 12) as usize)
                    as i64;
            }
            let cpo = read_i32(bytes, lo);
            if lo as i64 + cpo as i64 != shared_base as i64 {
                return bad(format!(
                    "solid[{si}] hull @{lo} 的 c_point_offset={cpo} 指向 {}，应为共享数组起点 {shared_base}",
                    lo as i64 + cpo as i64
                ));
            }
            if cpo as i64 != expect_cpo {
                return bad(format!(
                    "solid[{si}] hull @{lo} 的 c_point_offset={cpo}，按后缀和应为 {expect_cpo}"
                ));
            }
            // 约束 6。
            let l2 = read_u32(bytes, lo + 8);
            if (l2 & 3) != 0 {
                return bad(format!("solid[{si}] hull @{lo} 的 has_chilren_flag != 0"));
            }
            if ((l2 >> 2) & 3) != 1 {
                return bad(format!("solid[{si}] hull @{lo} 的 is_compact_flag != 1"));
            }
            if ((l2 >> 4) & 0xF) != 0 {
                return bad(format!("solid[{si}] hull @{lo} 的 dummy != 0"));
            }
            if read_i16(bytes, lo + 14) != 0 {
                return bad(format!("solid[{si}] hull @{lo} 的 for_future_use != 0"));
            }
        }

        // ---- 每个 hull 的流形 / tri_index / opposite_index / size_div_16 / 节点几何 ----
        for &lo in &hull_offsets {
            let n_tri = read_i16(bytes, lo + 12) as usize;

            let mut tris: Vec<[u32; 3]> = Vec::with_capacity(n_tri);
            for k in 0..n_tri {
                let o = lo + LEDGE_HEADER_SIZE + k * TRIANGLE_SIZE;
                // 约束 7：tri_index 从 0 起严格递增。
                if read_u32(bytes, o) & 0x0FFF != k as u32 {
                    return bad(format!(
                        "solid[{si}] hull @{lo} 第 {k} 个三角形的 tri_index != {k}"
                    ));
                }
                let t = [0usize, 1, 2].map(|i| read_u32(bytes, o + 4 + i * 4) & 0xFFFF);
                for &v in &t {
                    if v as usize >= n_points {
                        return bad(format!(
                            "solid[{si}] hull @{lo} 引用了点 {v}，但共享数组只有 {n_points} 个点"
                        ));
                    }
                }
                tris.push(t);
            }

            // 约束 9：闭合可定向流形。
            let mut dir: HashMap<(u32, u32), usize> = HashMap::with_capacity(n_tri * 3);
            for t in &tris {
                for i in 0..3 {
                    *dir.entry((t[i], t[(i + 1) % 3])).or_insert(0) += 1;
                }
            }
            for (&(u, v), &c) in &dir {
                if c != 1 {
                    return bad(format!("solid[{si}] hull @{lo} 有向边 {u}->{v} 出现 {c} 次"));
                }
                if !dir.contains_key(&(v, u)) {
                    return bad(format!("solid[{si}] hull @{lo} 有向边 {u}->{v} 没有反向边"));
                }
            }

            // 约束 8：opposite_index 精确。
            let mut idx: HashMap<(u32, u32), (usize, usize)> = HashMap::with_capacity(n_tri * 3);
            for (k, t) in tris.iter().enumerate() {
                for i in 0..3 {
                    idx.insert((t[i], t[(i + 1) % 3]), (k, i));
                }
            }
            let mut used: Vec<u32> = Vec::new();
            for t in &tris {
                for &v in t {
                    if !used.contains(&v) {
                        used.push(v);
                    }
                }
            }
            for (k, t) in tris.iter().enumerate() {
                let o = lo + LEDGE_HEADER_SIZE + k * TRIANGLE_SIZE;
                for i in 0..3 {
                    let (k2, i2) = idx[&(t[(i + 1) % 3], t[i])];
                    let expect = (4 * k2 as i32 + 1 + i2 as i32) - (4 * k as i32 + 1 + i as i32);
                    let raw = (read_u32(bytes, o + 4 + i * 4) >> 16) & 0x7FFF;
                    // 15 位二补数还原。
                    let got = if raw & 0x4000 != 0 {
                        raw as i32 - 0x8000
                    } else {
                        raw as i32
                    };
                    if got != expect {
                        return bad(format!(
                            "solid[{si}] hull @{lo} 三角形 {k} 边 {i} 的 opposite_index 应为 {expect}，实际 {got}"
                        ));
                    }
                }
            }

            // 约束 5：size_div_16 == 1 + n_triangles + 该 hull 引用的点数。
            let size_div_16 = (read_u32(bytes, lo + 8) >> 8) & 0x00FF_FFFF;
            let expect = 1 + n_tri + used.len();
            if size_div_16 as usize != expect {
                return bad(format!(
                    "solid[{si}] hull @{lo} 的 size_div_16={size_div_16} != 1+{n_tri}+{}",
                    used.len()
                ));
            }

            // ---- 叶子节点几何（2051/2051 实测精确） ----
            let mut mn = [f32::INFINITY; 3];
            let mut mx = [f32::NEG_INFINITY; 3];
            for &v in &used {
                let p = shared_base + v as usize * POINT_SIZE;
                for a in 0..3 {
                    let c = read_f32(bytes, p + a * 4);
                    if c < mn[a] {
                        mn[a] = c;
                    }
                    if c > mx[a] {
                        mx[a] = c;
                    }
                }
            }
            let (expect_center, expect_radius, expect_box) = node_geometry(mn, mx);

            let node = find_node_for_hull(bytes, node_off, node_count, lo)?;
            for (a, &want) in expect_center.iter().enumerate() {
                let got = read_f32(bytes, node + 8 + a * 4);
                if (got - want).abs() > 1e-4 {
                    return bad(format!(
                        "solid[{si}] hull @{lo} 节点的 center[{a}] 应为 {want}，实际 {got}"
                    ));
                }
            }
            let got_radius = read_f32(bytes, node + 20);
            if (got_radius - expect_radius).abs() > 1e-3 {
                return bad(format!(
                    "solid[{si}] hull @{lo} 节点的 radius 应为 {expect_radius}，实际 {got_radius}"
                ));
            }
            if got_radius <= 0.0 {
                return bad(format!("solid[{si}] hull @{lo} 节点的 radius 为 0"));
            }
            for a in 0..3 {
                let got = bytes[node + 24 + a];
                if got != expect_box[a] {
                    return bad(format!(
                        "solid[{si}] hull @{lo} 节点的 box_sizes[{a}] 应为 {}，实际 {got}",
                        expect_box[a]
                    ));
                }
            }
            if bytes[node + 27] != 0 {
                return bad(format!("solid[{si}] 节点的 free_0 != 0"));
            }
        }

        // ---- 单节点树时 offset_compact_ledge == 48 − offset_ledgetree_root ----
        if node_count == 1 {
            let rel = read_i32(bytes, node_off + 4);
            let expect = COMPACT_SURFACE_SIZE as i32 - offset_ledgetree_root;
            if rel != expect {
                return bad(format!(
                    "solid[{si}] 单节点树的 offset_compact_ledge 应为 {expect}，实际 {rel}"
                ));
            }
            if rel >= 0 {
                return bad(format!(
                    "solid[{si}] offset_compact_ledge={rel} 应为负（ledge 在低地址）"
                ));
            }
        }

        // ---- upper_limit_radius == max‖p − mass_center‖ ----
        let mass_center = [
            read_f32(bytes, body),
            read_f32(bytes, body + 4),
            read_f32(bytes, body + 8),
        ];
        let upper_limit_radius = read_f32(bytes, body + 24);
        let mut max_d = 0.0f32;
        for k in 0..n_points {
            let p = shared_base + k * POINT_SIZE;
            let d = [
                read_f32(bytes, p) - mass_center[0],
                read_f32(bytes, p + 4) - mass_center[1],
                read_f32(bytes, p + 8) - mass_center[2],
            ];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if len > max_d {
                max_d = len;
            }
        }
        if (upper_limit_radius - max_d).abs() > 1e-4 {
            return bad(format!(
                "solid[{si}] upper_limit_radius 应为 {max_d}，实际 {upper_limit_radius}"
            ));
        }

        surface_sizes.push(surface_size);
        node_counts.push(node_count);
        ledge_region_sizes.push(ledge_region_size);
        base = solid_end;
    }

    // ---- 约束 13：text section ----
    let solids_end = base;
    if solids_end >= bytes.len() {
        return bad("没有 text section".into());
    }
    let text_size = bytes.len() - solids_end;
    let text = &bytes[solids_end..];
    if *text.last().unwrap() != 0 {
        return bad("text section 末尾不是单个 0x00".into());
    }
    if text[..text.len() - 1].contains(&0) {
        return bad("text section 内部有 NUL 字节".into());
    }
    let body = std::str::from_utf8(&text[..text.len() - 1])
        .map_err(|e| PhyError::SelfCheck(format!("text section 不是 UTF-8：{e}")))?;
    if !body.starts_with("solid {") {
        return bad("text section 没有以 \"solid {\" 开头".into());
    }
    if !body.ends_with("}\n") {
        return bad("text section 没有以 \"}\\n\" 结尾".into());
    }
    let blocks = body.lines().filter(|l| l.starts_with("solid {")).count();
    if blocks != solid_count {
        return bad(format!(
            "text section 里有 {blocks} 个 \"solid {{\" 块，但 solidCount={solid_count}"
        ));
    }

    Ok(PhyLayout {
        file_size: bytes.len(),
        solid_count,
        solids_end,
        text_size,
        surface_sizes,
        node_counts,
        ledge_region_sizes,
    })
}

/// 找到引用 `hull_off` 的那个树节点。
fn find_node_for_hull(
    bytes: &[u8],
    node_off: usize,
    node_count: usize,
    hull_off: usize,
) -> Result<usize, PhyError> {
    for i in 0..node_count {
        let off = node_off + i * LEDGETREE_NODE_SIZE;
        let rel = read_i32(bytes, off + 4);
        if rel != 0 && (off as i64 + rel as i64) == hull_off as i64 {
            return Ok(off);
        }
    }
    Err(PhyError::SelfCheck(format!("没有节点引用 hull @{hull_off}")))
}

// ---------------------------------------------------------------------------
// 凸分解辅助（薄封装 parry3d，避免调用方直接依赖 parry 类型）
// ---------------------------------------------------------------------------

/// 把一个可能凹的网格近似凸分解成若干凸块（parry3d 的 VHACD）。
///
/// 每个返回的 [`PhyHull`] 都是闭合凸多面体，可以直接作为
/// [`write_phy_multi`] 里一个 solid 的多块输入，或各自作为独立 solid。
///
/// # 为什么用 VHACD 而不是自己写
///
/// 近似凸分解是体素化 + 递归切分的数值算法，自己实现既慢又容易在退化
/// 网格上炸掉。parry3d 是成熟的 Rust 物理几何库，直接用它的结果。
///
/// # 参数
///
/// `resolution` 是体素分辨率（越大越精细也越慢，游戏碰撞体 32..64 足够），
/// `max_hulls` 是凸块数上限（引擎侧每个 hull 都要建 ledge，别开太大）。
pub fn decompose_concave(
    vertices: &[[f32; 3]],
    faces: &[[u32; 3]],
    resolution: u32,
    max_hulls: u32,
) -> Result<Vec<PhyHull>, PhyError> {
    if vertices.is_empty() || faces.is_empty() {
        return Err(PhyError::BadParameter {
            what: "decompose_concave",
            detail: "顶点或面表为空".into(),
        });
    }
    if vertices.len() < 4 {
        return Err(PhyError::HullTooFewPoints {
            solid: 0,
            points: vertices.len(),
        });
    }
    for (i, p) in vertices.iter().enumerate() {
        if !p.iter().all(|c| c.is_finite()) {
            return Err(PhyError::NonFinitePoint { solid: 0, point: i });
        }
    }
    for (fi, f) in faces.iter().enumerate() {
        for &idx in f {
            if idx as usize >= vertices.len() {
                return Err(PhyError::IndexOutOfRange {
                    solid: 0,
                    face: fi,
                    index: idx,
                    vertex_count: vertices.len(),
                });
            }
        }
    }
    let vs: Vec<Vector> = vertices.iter().map(|p| Vector::new(p[0], p[1], p[2])).collect();
    let params = VHACDParameters {
        resolution: resolution.max(8),
        max_convex_hulls: max_hulls.max(1),
        ..Default::default()
    };
    let decomposition = VHACD::decompose(&params, &vs, faces, false);
    let parts = decomposition.compute_convex_hulls(1);
    let mut out = Vec::with_capacity(parts.len());
    for (pts, tris) in parts {
        if pts.len() < 4 || tris.is_empty() {
            continue;
        }
        out.push(PhyHull {
            vertices: pts.iter().map(|v| [v.x, v.y, v.z]).collect(),
            faces: tris,
        });
    }
    if out.is_empty() {
        return Err(PhyError::HullDegenerate {
            solid: 0,
            detail: "VHACD 没有产出任何有效的凸块".into(),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 字节写入 / 读取小工具
// ---------------------------------------------------------------------------

fn put_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_i16(buf: &mut Vec<u8>, v: i16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_vec3(buf: &mut Vec<u8>, v: [f32; 3]) {
    put_f32(buf, v[0]);
    put_f32(buf, v[1]);
    put_f32(buf, v[2]);
}

fn read_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn read_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn read_i16(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}

fn read_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn read_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `build_phy_from_smd`：从 SMD 端到端产出 `.phy`，且 checksum 配对。
    ///
    /// 这是 `mdlc build` 接入 PHY 的入口 —— 官方 QC 的
    /// `$collisionmodel <.smd>` 同样指向一个**单独的 SMD**。
    #[test]
    fn build_phy_from_smd_produces_valid_file() {
        // 一个 8 顶点立方体（12 个三角形），SMD 文本直接手写。
        let mut smd_text = String::from("version 1\nnodes\n0 \"root\" -1\nend\nskeleton\ntime 0\n0 0 0 0 0 0 0\nend\ntriangles\nphys\n");
        let v = cube_vertices();
        for f in cube_faces() {
            for &i in &f {
                let p = v[i as usize];
                smd_text.push_str(&format!(
                    "0 {} {} {} 0 0 1 0 0 1 0 1\n",
                    p[0], p[1], p[2]
                ));
            }
        }
        smd_text.push_str("end\n");
        let smd = crate::smd::parse_smd(&smd_text).expect("SMD 应能解析");

        let bytes = build_phy_from_smd(
            &smd,
            PhyIdentity {
                model_name: "testprop",
                collision_smd_name: "testprop",
                surface_prop: "metal",
            },
            0xDEAD_BEEF,
            1.0,
            &crate::model::Physics::default(),
            None,
        )
        .expect("应能写出 PHY");
        let layout = check_invariants(&bytes).expect("写出的 PHY 应通过 13 条硬约束");
        assert_eq!(layout.solid_count, 1, "prop 形态只有 1 个 solid");
        // checksum 必须落在 `phyheader_t` 的 `+0x0C`（不是 `+0x08`）。
        assert_eq!(
            u32::from_le_bytes(bytes[0x0C..0x10].try_into().unwrap()),
            0xDEAD_BEEF,
            "checksum 写在 +0x0C"
        );
        // `phyheader.size == 16`、`id == 0`。
        assert_eq!(i32::from_le_bytes(bytes[0..4].try_into().unwrap()), 16);
        assert_eq!(i32::from_le_bytes(bytes[4..8].try_into().unwrap()), 0);
    }

    /// 空 SMD 必须报错，而不是产出一个畸形文件。
    #[test]
    fn build_phy_rejects_empty_smd() {
        let smd = crate::smd::parse_smd("version 1\nnodes\n0 \"root\" -1\nend\nskeleton\nend\ntriangles\nend\n")
            .expect("SMD 应能解析");
        assert!(
            build_phy_from_smd(
                &smd,
                PhyIdentity {
                    model_name: "x",
                    collision_smd_name: "x",
                    surface_prop: "default",
                },
                0,
                1.0,
                &crate::model::Physics::default(),
                None
            )
            .is_err()
        );
    }

    /// `$concave` 的**连通分量**语义：两个分离的壳 ⟹ 2 个凸块。
    ///
    /// 这是官方与 VHACD 的**分水岭** —— 官方按连通分量拆，
    /// 所以分离的壳一定分开；VHACD 是体逼近，对分离壳也可能给出别的数。
    #[test]
    fn concave_splits_disconnected_shells() {
        // 两个立方体，相距很远（明确不连通）。
        let smd = parse_two_cubes();
        let hulls = decompose_connected_components(&smd, None).expect("应能分解");
        assert_eq!(hulls.len(), 2, "两个分离的壳应产出 2 个凸块");
    }

    /// `$concave` 对**连通**的凹体产出 **1** 个凸块（= 整网格的凸包）。
    ///
    /// ⚠️ 这条是**反直觉**的，但正是官方的行为：
    /// 反编译 `ProcessSingleBody` 确认它是连通分量分解，
    /// 所以连通的凹体不会被拆开，凹处被**填平**。
    ///
    /// 与 `decompose_concave`（VHACD）对比：同一个网格 VHACD 会给多块。
    #[test]
    fn concave_keeps_connected_concave_body_as_single_hull() {
        // 一个 U 形棱柱（凹但连通），光滑着色（法线沿 +Z，同一面上一致）。
        let smd = parse_u_prism();
        let hulls = decompose_connected_components(&smd, None).expect("应能分解");
        assert_eq!(hulls.len(), 1, "连通的凹体在官方 $concave 下是 1 个凸块");

        // 对照：VHACD 会把它拆成多块（凹口保留）。
        let (verts, faces) = weld_smd_triangles(&smd).expect("应能焊接");
        let vhacd = decompose_concave(&verts, &faces, 64, 16).expect("VHACD 应能分解");
        assert!(
            vhacd.len() > 1,
            "VHACD 应把凹体拆成多块（实际 {}）—— 这正是它与官方语义的差别",
            vhacd.len()
        );
    }

    /// 质量默认值是 **1.0**（不是 10.0）。
    ///
    /// 源码：`CJointedModel::CJointedModel()` 的 `m_totalMass = 1.0`，
    /// 而 `ComputeMass()` 首句 `if (m_totalMass >= 0) return;` 直接返回。
    #[test]
    fn default_total_mass_is_one() {
        let p = PhyParams::new("m", 0);
        assert_eq!(p.total_mass, 1.0);
        let b = write_cube();
        let layout = check_invariants(&b).unwrap();
        let text = std::str::from_utf8(&b[layout.solids_end..b.len() - 1]).unwrap();
        assert!(text.contains("\"totalmass\" \"1.000000\""));
    }

    /// 解析一个「两个分离立方体」的 SMD。
    fn parse_two_cubes() -> crate::smd::Smd {
        let mut s = String::from("version 1\nnodes\n0 \"root\" -1\nend\nskeleton\ntime 0\n0 0 0 0 0 0 0\nend\ntriangles\n");
        for offset in [[0.0f32, 0.0, 0.0], [100.0, 0.0, 0.0]] {
            let v = cube_vertices();
            for f in cube_faces() {
                // 每个三角形前要有材质行（studiomdl 与 mdlc 都要求）。
                s.push_str("phys\n");
                for &i in &f {
                    let p = v[i as usize];
                    s.push_str(&format!(
                        "0 {} {} {} 0 0 1 0 0 1 0 1\n",
                        p[0] + offset[0],
                        p[1] + offset[1],
                        p[2] + offset[2]
                    ));
                }
            }
        }
        s.push_str("end\n");
        crate::smd::parse_smd(&s).expect("SMD 应能解析")
    }

    /// 解析一个 U 形棱柱（凹、连通、光滑着色）。
    ///
    /// 截面（XZ 平面，CCW）：
    /// ```text
    /// (-2,-2) (2,-2) (2,2) (1,2) (1,0) (-1,0) (-1,2) (-2,2)
    /// ```
    /// 沿 Y 从 −1 拉到 +1。法线全部取 +Z（同一平面上一致 ⟹ 能焊上）。
    fn parse_u_prism() -> crate::smd::Smd {
        let poly: [[f32; 2]; 8] = [
            [-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [1.0, 2.0],
            [1.0, 0.0], [-1.0, 0.0], [-1.0, 2.0], [-2.0, 2.0],
        ];
        let y0 = -1.0f32;
        let y1 = 1.0f32;
        let n = poly.len();
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        let bot = |i: usize| [poly[i][0], y0, poly[i][1]];
        let top = |i: usize| [poly[i][0], y1, poly[i][1]];
        for i in 1..n - 1 {
            tris.push([bot(0), bot(i), bot(i + 1)]);
            tris.push([top(0), top(i + 1), top(i)]);
        }
        for i in 0..n {
            let j = (i + 1) % n;
            tris.push([top(i), top(j), bot(j)]);
            tris.push([top(i), bot(j), bot(i)]);
        }
        let mut s = String::from("version 1\nnodes\n0 \"root\" -1\nend\nskeleton\ntime 0\n0 0 0 0 0 0 0\nend\ntriangles\n");
        for t in &tris {
            s.push_str("phys\n");
            for p in t {
                s.push_str(&format!(
                    "0 {} {} {} 0 0 1 0 0 1 0 1\n",
                    p[0], p[1], p[2]
                ));
            }
        }
        s.push_str("end\n");
        crate::smd::parse_smd(&s).expect("SMD 应能解析")
    }

    /// 单位立方体的 8 个顶点，坐标取 ±1（与 `phy-probe/gen-phy.js` 的 CUBE_V 同形）。
    fn cube_vertices() -> Vec<[f32; 3]> {
        vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ]
    }

    /// 立方体的 12 个三角面，**CCW 外向**。
    ///
    /// 顶点顺序直接抄自参考实现（那里已经用真实校验器验证过），
    /// 不自己重新推导朝向 —— 推导错了会让整个测试集一起跑偏。
    fn cube_faces() -> Vec<[u32; 3]> {
        vec![
            [4, 5, 6],
            [4, 6, 7], // +Z
            [1, 0, 3],
            [1, 3, 2], // -Z
            [5, 1, 2],
            [5, 2, 6], // +X
            [0, 4, 7],
            [0, 7, 3], // -X
            [7, 6, 2],
            [7, 2, 3], // +Y
            [0, 1, 5],
            [0, 5, 4], // -Y
        ]
    }

    fn cube() -> PhyHull {
        PhyHull {
            vertices: cube_vertices(),
            faces: cube_faces(),
        }
    }

    fn write_cube() -> Vec<u8> {
        let h = cube();
        let s = [PhySolid::prop("cube")];
        let p = PhyParams::new("cube", 0xdead_beef);
        write_phy(std::slice::from_ref(&h), &s, &p).expect("立方体必须能写出")
    }

    // -----------------------------------------------------------------
    // 1. 立方体 → 13 条硬约束自检
    // -----------------------------------------------------------------

    /// **核心判据**：立方体凸包的输出必须通过全部硬约束自检。
    ///
    /// `write_phy` 内部已经调用 `check_invariants`，所以这里再显式跑一遍
    /// 是为了让"这条测试到底在断言什么"一目了然。
    #[test]
    fn cube_passes_all_hard_constraints() {
        let bytes = write_cube();
        let layout = check_invariants(&bytes).expect("13 条硬约束必须全部通过");

        assert_eq!(layout.solid_count, 1);
        assert_eq!(layout.file_size, bytes.len());
        assert_eq!(layout.text_size, bytes.len() - layout.solids_end);
    }

    /// 约束 12：`phyheader` 三个常量字段。
    #[test]
    fn cube_header_matches_reference_bytes() {
        let b = write_cube();
        assert_eq!(read_i32(&b, 0), 16, "phyheader.size 恒为 16");
        assert_eq!(read_i32(&b, 4), 0, "phyheader.id 恒为 0");
        assert_eq!(read_i32(&b, 8), 1, "solidCount");
        assert_eq!(read_u32(&b, 12), 0xdead_beef, "checksum 必须原样写入");
    }

    /// 与参考实现 `gen-phy.js` 的实测对照值逐字段比对。
    ///
    /// 参考实现是**独立于本实现**写出来的（JS，且已用同一个校验器验证过），
    /// 所以这些数字能同时抓住两边的错误。
    #[test]
    fn cube_matches_reference_implementation_layout() {
        let b = write_cube();

        // gen-phy.js: `surfaceSize = 412`、`size = 440`。
        //
        // 推导：1 个 hull，12 三角形、8 个点
        //   ledge 区 = 16(头) + 16×12(三角形) + 16×8(点) = 16 + 192 + 128 = 336
        //   surfaceSize = 48 + 336 + 28×1 = 412
        assert_eq!(read_i32(&b, 16 + 12), 412, "surfaceSize");
        assert_eq!(read_i32(&b, 16), 440, "size == surfaceSize + 28");
        // 约束 2：solid 步长 == size + 4 == surfaceSize + 32。
        assert_eq!(440 + 4, 412 + 32, "solid 步长恒等式");

        let body = 16 + SOLID_HEADER_SIZE;
        assert_eq!(read_u32(&b, 16 + 4), VPHYSICS_ID, "vphysicsID == 'VPHY'");
        assert_eq!(read_u16(&b, 16 + 8), 0x0100, "version");
        assert_eq!(read_i16(&b, 16 + 10), 0, "modelType == COLLIDE_POLY");
        assert_eq!(read_i32(&b, 16 + 28), 0, "axisMapSize");
        assert_eq!(read_u32(&b, body + 44), IVP_COMPACT_SURFACE_ID, "dummy[2] == 'IVPS'");

        // 约束 3：offset_ledgetree_root == 48 + ledge_region_size == 48 + 336。
        assert_eq!(read_i32(&b, body + 32), 384, "offset_ledgetree_root");

        // 头号陷阱：offset_compact_ledge == 48 − offset_ledgetree_root，恒为负。
        // 实测对照：urban_puddle 的 offset_ledgetree_root=480 → 该字段 = −432。
        let node = body + 384;
        assert_eq!(read_i32(&b, node), 0, "单节点树的 offset_right_node == 0");
        assert_eq!(
            read_i32(&b, node + 4),
            48 - 384,
            "offset_compact_ledge 相对节点自身"
        );
        assert_eq!(read_i32(&b, node + 4), -336);
    }

    /// 约束 4 / 5：`c_point_offset` 与 `size_div_16`。
    #[test]
    fn cube_ledge_fields_are_exact() {
        let b = write_cube();
        let region = 16 + SOLID_HEADER_SIZE + COMPACT_SURFACE_SIZE;
        let n_tri = read_i16(&b, region + 12) as usize;
        assert_eq!(n_tri, 12);

        // 单 hull 时 c_point_offset 恰好等于 16 + 16×nTri —— 这正是
        // 规格 §4.2 那条规则成立的唯一场合（多 hull 时它指向共享数组起点）。
        assert_eq!(
            read_i32(&b, region),
            (16 + 16 * n_tri) as i32,
            "单 hull 的 c_point_offset == 16 + 16×nTri"
        );

        let l2 = read_u32(&b, region + 8);
        assert_eq!(l2 & 3, 0, "has_chilren_flag == 0（终结 ledge）");
        assert_eq!((l2 >> 2) & 3, 1, "is_compact_flag == 1");
        assert_eq!((l2 >> 4) & 0xF, 0, "dummy == 0");
        // 约束 5：size_div_16 == 1 + nTri + 本 hull 引用的点数 == 1 + 12 + 8。
        assert_eq!((l2 >> 8) & 0x00FF_FFFF, 1 + 12 + 8, "size_div_16");
        assert_eq!(read_i16(&b, region + 14), 0, "for_future_use == 0");
    }

    /// 约束 7：`tri_index` 在每个 hull 内从 0 起严格递增，
    /// 且 `pierce_index == tri_index`、`material_index == 0`、`is_virtual == 0`。
    #[test]
    fn cube_tri_indices_are_sequential() {
        let b = write_cube();
        let region = 16 + SOLID_HEADER_SIZE + COMPACT_SURFACE_SIZE;
        for k in 0..12usize {
            let w0 = read_u32(&b, region + 16 + k * 16);
            assert_eq!(w0 & 0x0FFF, k as u32, "tri_index[{k}]");
            assert_eq!((w0 >> 12) & 0x0FFF, k as u32, "pierce_index[{k}]");
            assert_eq!((w0 >> 24) & 0x7F, 0, "material_index[{k}]");
            assert_eq!(w0 >> 31, 0, "is_virtual[{k}]");
        }
    }

    // -----------------------------------------------------------------
    // 2. opposite_index 的反向边公式（手算验证）
    // -----------------------------------------------------------------

    /// **手算验证** `opposite_index` 的反向边公式。
    ///
    /// 用一个 4 顶点的正四面体：只有 4 个三角形，`opposite_index` 可以
    /// 逐条手算出来，不依赖任何"跑一遍看结果"的自证。
    ///
    /// 边的线性编号是 `4k + 1 + i`（每个三角形的第 0 个 32 位字被表头占用），
    /// 所以 4 个三角形的 12 条边编号是 1,2,3 / 5,6,7 / 9,10,11 / 13,14,15。
    #[test]
    fn opposite_index_hand_computed_tetrahedron() {
        // 正四面体，CCW 外向。
        let verts = [
            [1.0f32, 1.0, 1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
        ];
        // 顶点表本身不参与公式验证（只验边表），但先确认它确实是 4 个点。
        assert_eq!(verts.len(), 4);
        let faces = [[0u32, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];

        // 有向边 → (k, i)。手写出来，不用代码推导，这样公式错了测试也会错。
        let dir: HashMap<(u32, u32), (usize, usize)> = [
            ((0, 1), (0usize, 0usize)),
            ((1, 2), (0, 1)),
            ((2, 0), (0, 2)),
            ((0, 3), (1, 0)),
            ((3, 1), (1, 1)),
            ((1, 0), (1, 2)),
            ((0, 2), (2, 0)),
            ((2, 3), (2, 1)),
            ((3, 0), (2, 2)),
            ((1, 3), (3, 0)),
            ((3, 2), (3, 1)),
            ((2, 1), (3, 2)),
        ]
        .into_iter()
        .collect();

        // 每条边 (k,i) 的期望 opposite_index = (4k'+1+i') − (4k+1+i)。
        // 这里逐条列出反向边，再手算差值。
        let expect: [((usize, usize), (usize, usize)); 12] = [
            // (k,i) -> (k',i')，都是把 (u,v) 换成 (v,u)
            ((0, 0), (1, 2)), // 0->1 的反向是 1->0，在三角形 1 的边 2
            ((0, 1), (3, 2)), // 1->2 的反向是 2->1，在三角形 3 的边 2
            ((0, 2), (2, 0)), // 2->0 的反向是 0->2，在三角形 2 的边 0
            ((1, 0), (2, 2)), // 0->3 的反向是 3->0，在三角形 2 的边 2
            ((1, 1), (3, 0)), // 3->1 的反向是 1->3，在三角形 3 的边 0
            ((1, 2), (0, 0)), // 1->0 的反向是 0->1，在三角形 0 的边 0
            ((2, 0), (0, 2)), // 0->2 的反向是 2->0，在三角形 0 的边 2
            ((2, 1), (3, 1)), // 2->3 的反向是 3->2，在三角形 3 的边 1
            ((2, 2), (1, 0)), // 3->0 的反向是 0->3，在三角形 1 的边 0
            ((3, 0), (1, 1)), // 1->3 的反向是 3->1，在三角形 1 的边 1
            ((3, 1), (2, 1)), // 3->2 的反向是 2->3，在三角形 2 的边 1
            ((3, 2), (0, 1)), // 2->1 的反向是 1->2，在三角形 0 的边 1
        ];

        // 先确认手写的边表与输入的面表一致。
        for (k, f) in faces.iter().enumerate() {
            for i in 0..3 {
                let key = (f[i], f[(i + 1) % 3]);
                assert_eq!(dir[&key], (k, i), "手写边表与面表不符：{key:?}");
            }
        }

        for ((k, i), (k2, i2)) in expect {
            let id = |k: usize, i: usize| 4 * k as i32 + 1 + i as i32;
            let hand = id(k2, i2) - id(k, i);
            // 与实现走同一条代码路径。
            let reverse = build_reverse_edges(&faces, 0).unwrap();
            let got_pair = reverse[3 * k + i];
            assert_eq!(
                (got_pair[0] as usize, got_pair[1] as usize),
                (k2, i2),
                "边 ({k},{i}) 的反向边"
            );
            let got = id(got_pair[0] as usize, got_pair[1] as usize) - id(k, i);
            assert_eq!(got, hand, "边 ({k},{i}) 的 opposite_index");
            assert!(
                (-16384..16384).contains(&got),
                "opposite_index {got} 必须塞进 15 位有符号"
            );
        }

        // 抽两个具体的数验证一下算术本身：把编号写死，看差值是不是期望值。
        // 边 (0,0) 编号 1、反向边 (1,2) 编号 7 → 差 6。
        let id = |k: i32, i: i32| 4 * k + 1 + i;
        assert_eq!(id(1, 2) - id(0, 0), 6);
        // 边 (3,2) 编号 15、反向边 (0,1) 编号 2 → 差 −13。
        assert_eq!(id(0, 1) - id(3, 2), -13);
        // 编号必须落在 1..=15（4 个三角形 = 16 个字，第 0 个字是表头）。
        assert_eq!(id(0, 0), 1);
        assert_eq!(id(3, 2), 15);
    }

    /// 写出的四面体里，每一条边的 `opposite_index` 必须与手算值逐位相同。
    #[test]
    fn tetrahedron_written_opposite_indices_match_hand_computation() {
        let h = PhyHull {
            vertices: vec![
                [1.0f32, 1.0, 1.0],
                [1.0, -1.0, -1.0],
                [-1.0, 1.0, -1.0],
                [-1.0, -1.0, 1.0],
            ],
            faces: vec![[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]],
        };
        let s = [PhySolid::prop("tetra")];
        let p = PhyParams::new("tetra", 1);
        let b = write_phy(std::slice::from_ref(&h), &s, &p).unwrap();

        let region = 16 + SOLID_HEADER_SIZE + COMPACT_SURFACE_SIZE;
        // 手算：(k,i) 的 opposite_index 的绝对值 = 编号差。
        // 对 (0,0) 期望 +6、(3,2) 期望 −13（见上一条测试的推导）。
        let edge = |k: usize, i: usize| read_u32(&b, region + 16 + k * 16 + 4 + i * 4);
        let opp = |k: usize, i: usize| {
            let raw = (edge(k, i) >> 16) & 0x7FFF;
            if raw & 0x4000 != 0 {
                raw as i32 - 0x8000
            } else {
                raw as i32
            }
        };
        assert_eq!(opp(0, 0), 6);
        assert_eq!(opp(3, 2), -13);
        // 每对反向边的 opposite_index 互为相反数。
        assert_eq!(opp(0, 0), -opp(1, 2));
        assert_eq!(opp(3, 2), -opp(0, 1));
    }

    /// 15 位有符号的边界：`|opposite_index| ≤ 4·nTri − 2`。
    ///
    /// 这条测试把 `MAX_TRIANGLES_PER_HULL = 4096` 这个取值的理由钉住 ——
    /// 报告 §12 写的 8192 会让 `opposite_index` 溢出。
    #[test]
    fn opposite_index_bound_justifies_max_triangles() {
        let max_delta = |n_tri: i64| 4 * (n_tri - 1);
        assert!(
            max_delta(MAX_TRIANGLES_PER_HULL as i64) <= 16383,
            "nTri={MAX_TRIANGLES_PER_HULL} 时最大编号差 {} 必须 ≤ 16383",
            max_delta(MAX_TRIANGLES_PER_HULL as i64)
        );
        // 报告 §12 的 8192 会溢出 —— 这是规格里的一个错误，不是我们的实现问题。
        assert!(
            max_delta(8192) > 16383,
            "8192 个三角形时最大编号差 {} 超出 15 位有符号，说明 §12 的数字是错的",
            max_delta(8192)
        );
    }

    // -----------------------------------------------------------------
    // 3. 错误路径：必须返回 Err 而不是 panic
    // -----------------------------------------------------------------

    /// 空输入。
    #[test]
    fn empty_input_is_rejected() {
        let p = PhyParams::new("x", 0);
        let err = write_phy(&[], &[], &p).unwrap_err();
        assert!(matches!(err, PhyError::BadParameter { .. }), "实际：{err:?}");

        // 有 solid 参数但没有凸包。
        let s = [PhySolid::prop("x")];
        let err = write_phy(&[], &s, &p).unwrap_err();
        assert!(matches!(err, PhyError::BadParameter { .. }), "实际：{err:?}");
    }

    /// 凸包与 solid 数量不匹配。
    #[test]
    fn mismatched_counts_are_rejected() {
        let h = cube();
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &[], &p).unwrap_err();
        assert!(matches!(err, PhyError::BadParameter { .. }), "实际：{err:?}");
    }

    /// 顶点太少（< 4）。
    #[test]
    fn too_few_vertices_is_rejected() {
        let h = PhyHull {
            vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            faces: vec![[0, 1, 2]],
        };
        let s = [PhySolid::prop("x")];
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
        assert!(
            matches!(err, PhyError::HullTooFewPoints { points: 3, .. }),
            "实际：{err:?}"
        );
    }

    /// **退化三角形**：三个下标里有重复 → 产生自环边 → `DuplicateEdge`。
    #[test]
    fn degenerate_triangle_is_rejected() {
        // 面 [0,0,1] 会产生自环边 0->0。
        let h = PhyHull {
            vertices: cube_vertices(),
            faces: vec![[0, 0, 1]],
        };
        let s = [PhySolid::prop("x")];
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
        assert!(
            matches!(err, PhyError::DuplicateEdge { from: 0, to: 0, .. }),
            "实际：{err:?}"
        );
    }

    /// 重复的有向边（同一朝向的两个面）→ `DuplicateEdge`。
    #[test]
    fn duplicated_directed_edge_is_rejected() {
        let h = PhyHull {
            vertices: cube_vertices(),
            faces: vec![[0, 1, 2], [0, 1, 2]],
        };
        let s = [PhySolid::prop("x")];
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
        assert!(matches!(err, PhyError::DuplicateEdge { .. }), "实际：{err:?}");
    }

    /// **非闭合流形**：少一个面 → 有向边找不到反向边 → `OpenMesh`。
    #[test]
    fn open_mesh_is_rejected() {
        let mut faces = cube_faces();
        faces.pop(); // 去掉一个面，网格出现一个三角形的洞
        let h = PhyHull {
            vertices: cube_vertices(),
            faces,
        };
        let s = [PhySolid::prop("x")];
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
        assert!(matches!(err, PhyError::OpenMesh { .. }), "实际：{err:?}");
    }

    /// 面表引用了不存在的顶点。
    #[test]
    fn out_of_range_index_is_rejected() {
        let h = PhyHull {
            vertices: cube_vertices(),
            faces: vec![[0, 1, 99]],
        };
        let s = [PhySolid::prop("x")];
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
        assert!(
            matches!(
                err,
                PhyError::IndexOutOfRange {
                    index: 99,
                    vertex_count: 8,
                    ..
                }
            ),
            "实际：{err:?}"
        );
    }

    /// NaN / 无穷坐标。
    #[test]
    fn non_finite_coordinates_are_rejected() {
        let s = [PhySolid::prop("x")];
        let p = PhyParams::new("x", 0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut vs = cube_vertices();
            vs[3][1] = bad;
            let h = PhyHull {
                vertices: vs,
                faces: cube_faces(),
            };
            let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
            assert!(
                matches!(err, PhyError::NonFinitePoint { point: 3, .. }),
                "值 {bad} 应被拒绝，实际：{err:?}"
            );
        }
    }

    /// 包围盒退化（所有顶点重合）→ 必须报错，绝不能 panic。
    ///
    /// # 为什么断言的是"两种错误之一"而不是精确的 `DegenerateBounds`
    ///
    /// `DegenerateBounds` 要求 AABB 半径是 0，也就是该 hull 引用的点**全部
    /// 重合**。但点全部重合意味着每个三角形都退化成 `[0,0,0]`，而流形校验
    /// （在顶点去重**之后**做，见 `prepare_solid`）会先一步报 `DuplicateEdge`。
    ///
    /// 所以经公开 API 走不到 `DegenerateBounds` —— 它是一条防御性分支，
    /// 保留是为了万一将来去重逻辑改了，还有一层兜底。测试在这里断言的是
    /// "退化输入被拒绝且不 panic"，这才是真正要保证的性质。
    #[test]
    fn degenerate_bounds_are_rejected() {
        // 8 个完全相同的点：面表下标各异、看起来闭合，但去重后全部塌成 [0,0,0]。
        let v = vec![[1.0f32, 2.0, 3.0]; 8];
        let h = PhyHull {
            vertices: v,
            faces: cube_faces(),
        };
        let s = [PhySolid::prop("x")];
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
        assert!(
            matches!(
                err,
                PhyError::DegenerateBounds { .. } | PhyError::DuplicateEdge { .. }
            ),
            "退化的包围盒必须被拒绝，实际：{err:?}"
        );

        // 直接调用内部几何函数时，半径 0 确实走 DegenerateBounds 分支：
        // 手工构造一个 AABB 完全塌缩的节点几何，确认它不会除零 panic。
        let (center, radius, boxes) = node_geometry([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]);
        assert_eq!(center, [1.0, 2.0, 3.0]);
        assert_eq!(radius, 0.0);
        assert_eq!(boxes, [0, 0, 0], "半径 0 时 box_sizes 全 0，不做除零");
    }

    /// 空面表。
    #[test]
    fn empty_faces_are_rejected() {
        let h = PhyHull {
            vertices: cube_vertices(),
            faces: Vec::new(),
        };
        let s = [PhySolid::prop("x")];
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
        assert!(
            matches!(err, PhyError::HullDegenerate { .. }),
            "实际：{err:?}"
        );
    }

    /// 参数越界。
    #[test]
    fn bad_parameters_are_rejected() {
        let h = cube();
        let s = [PhySolid::prop("x")];

        for mass in [0.0f32, -1.0, f32::NAN] {
            let mut p = PhyParams::new("x", 0);
            p.total_mass = mass;
            assert!(
                matches!(
                    write_phy(std::slice::from_ref(&h), &s, &p),
                    Err(PhyError::BadParameter { .. })
                ),
                "total_mass={mass} 应被拒绝"
            );
        }

        let mut p = PhyParams::new("x", 0);
        p.inertia_scale = -1.0;
        assert!(matches!(
            write_phy(std::slice::from_ref(&h), &s, &p),
            Err(PhyError::BadParameter { .. })
        ));
    }

    /// 名字里有 NUL 会截断 text section。
    #[test]
    fn nul_in_name_is_rejected() {
        let h = cube();
        let s = [PhySolid {
            bone_index: None,
            name: "bad\0name".into(),
            parent: None,
            mass_bias: 1.0,
            damping: None,
            rot_damping: None,
            inertia: None,
        }];
        let p = PhyParams::new("x", 0);
        let err = write_phy(std::slice::from_ref(&h), &s, &p).unwrap_err();
        assert!(matches!(err, PhyError::BadParameter { .. }), "实际：{err:?}");
    }

    /// 退化点集算凸包：必须返回 Err 而不是 panic。
    ///
    /// 这是选 `try_convex_hull` 而不是 `convex_hull` 的直接理由 ——
    /// 后者在共面/共线输入上会 `unwrap()` 一个 `Err`，直接炸掉。
    #[test]
    fn degenerate_point_sets_do_not_panic() {
        // 全共面（z 恒为 0）。
        let coplanar: Vec<[f32; 3]> = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.5, 0.5, 0.0],
        ];
        // 共线。
        let collinear: Vec<[f32; 3]> =
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]];
        // 全部重合。
        let coincident: Vec<[f32; 3]> = vec![[1.0, 1.0, 1.0]; 5];

        for (what, pts) in [
            ("共面", coplanar),
            ("共线", collinear),
            ("重合", coincident),
        ] {
            // 只要求"不 panic"：不同 parry 版本对退化的判定可能不同，
            // 断言具体的 Err 变体会让测试变脆。但**绝不能**是 panic。
            match PhyHull::from_points(&pts) {
                Ok(h) => {
                    // 万一算出来了，它必须仍然是可写出的（或者明确报错）。
                    assert!(!h.faces.is_empty(), "{what}：凸包不该没有面");
                }
                Err(e) => {
                    assert!(
                        matches!(
                            e,
                            PhyError::HullDegenerate { .. } | PhyError::HullTooFewPoints { .. }
                        ),
                        "{what}：意外的错误类型 {e:?}"
                    );
                }
            }
        }

        // 少于 4 点必须在进 parry 之前就被拦下。
        assert!(matches!(
            PhyHull::from_points(&[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            Err(PhyError::HullTooFewPoints { points: 3, .. })
        ));
    }

    /// 截断/损坏的字节必须报 `SelfCheck` 而不是 panic。
    #[test]
    fn truncated_bytes_are_reported_not_panicking() {
        let b = write_cube();
        // 从尾部一路截断到头部，每一段都必须返回 Err 或 Ok，绝不能 panic。
        for cut in 0..b.len().min(200) {
            let _ = check_invariants(&b[..cut]);
        }
        // 只剩头。
        assert!(matches!(
            check_invariants(&b[..16]),
            Err(PhyError::SelfCheck(_))
        ));
        // 空。
        assert!(matches!(
            check_invariants(&[]),
            Err(PhyError::SelfCheck(_))
        ));
    }

    /// 篡改关键字段必须被自检抓到。
    #[test]
    fn tampered_bytes_are_caught() {
        let good = write_cube();

        // phyheader.size 改成别的。
        let mut b = good.clone();
        b[0] = 99;
        assert!(matches!(
            check_invariants(&b),
            Err(PhyError::SelfCheck(_))
        ));

        // phyheader.id 改成非 0。
        let mut b = good.clone();
        b[4] = 1;
        assert!(matches!(
            check_invariants(&b),
            Err(PhyError::SelfCheck(_))
        ));

        // vphysicsID 改坏。
        let mut b = good.clone();
        b[16 + 4] = 0;
        assert!(matches!(
            check_invariants(&b),
            Err(PhyError::SelfCheck(_))
        ));

        // size 与 surfaceSize 的关系被破坏。
        let mut b = good.clone();
        let size = read_i32(&b, 16) + 1;
        b[16..20].copy_from_slice(&size.to_le_bytes());
        assert!(matches!(
            check_invariants(&b),
            Err(PhyError::SelfCheck(_))
        ));

        // dummy[2] 不再是 'IVPS'。
        let body = 16 + SOLID_HEADER_SIZE;
        let mut b = good.clone();
        b[body + 44] = 0;
        assert!(matches!(
            check_invariants(&b),
            Err(PhyError::SelfCheck(_))
        ));

        // **头号陷阱**：把 offset_compact_ledge 改成 −offset_ledgetree_root
        // （也就是规格警告的那个错误写法），自检必须抓到。
        let node = body + read_i32(&good, body + 32) as usize;
        let mut b = good.clone();
        let wrong = -read_i32(&good, body + 32);
        b[node + 4..node + 8].copy_from_slice(&wrong.to_le_bytes());
        assert!(
            matches!(check_invariants(&b), Err(PhyError::SelfCheck(_))),
            "offset_compact_ledge 写错基准必须被自检抓到"
        );

        // text section 的 NUL 被抹掉。
        let mut b = good.clone();
        let last = b.len() - 1;
        b[last] = b'x';
        assert!(matches!(
            check_invariants(&b),
            Err(PhyError::SelfCheck(_))
        ));
    }

    // -----------------------------------------------------------------
    // 4. 长度恒等式
    // -----------------------------------------------------------------

    /// **同一个 `.phy` 里两种单位并存** —— 钉住「text `volume` 是 inch³、
    /// 点数组是 m³」这条容易搞混的规则。
    ///
    /// # 为什么容易搞混
    ///
    /// 点数组由 `vphysics.dll` 的 `CollideWrite` 序列化（IVP 内部单位 = 米），
    /// 而 text 段的 `"volume"` 是 **studiomdl 自己**用
    /// `physcollision->ConvexVolume()`（Source 单位 = inch）算出来再
    /// `fprintf` 的（`collisionmodel.cpp:1174` 累加 → `2310` 打印）。
    /// **两条路径，两种单位，同一个文件。**
    ///
    /// 实测（官方 `msh1.phy`，50×10×50 inch 的长方体）：
    ///
    /// ```text
    /// text 段 "volume"   = 16999.996094   ← 17000 in³
    /// 点数组算出的体积   = 0.278580 m³
    /// 0.278580 / 0.0254³ = 17000.00       ← 换算回 inch³ 完全吻合
    /// ```
    ///
    /// 若把 `volume` 也换算成米（或忘了把点换算成米），
    /// text 段就会与官方差 `61023` 倍 —— 而 `check_invariants` 查不出。
    #[test]
    fn text_volume_is_inches_cubed_while_points_are_metres() {
        let b = write_cube();
        // text 段的 `"volume"`：±1 立方体（边长 2 inch）体积 = 8 in³。
        let text = String::from_utf8_lossy(&b[b.len() - 400..]);
        let m = text
            .find("\"volume\"")
            .expect("text 段必须有 volume");
        let rest = &text[m + "\"volume\"".len()..];
        // 格式是 `"volume" "8.000000"` —— 取引号之间的数。
        let q1 = rest.find('"').expect("volume 后应有引号");
        let q2 = rest[q1 + 1..].find('"').expect("volume 值应有结束引号");
        let v: f32 = rest[q1 + 1..q1 + 1 + q2].parse().expect("volume 应是数字");
        assert!(
            (v - 8.0).abs() < 1e-3,
            "±1 立方体的 text volume 应为 **8 in³**（不是 8×0.0254³），实际 {v}"
        );

        // 而点数组必须是米：±1 inch → ±0.0254 m。
        let body = 16 + SOLID_HEADER_SIZE;
        let ledge = body + COMPACT_SURFACE_SIZE;
        let n_tri = read_i16(&b, ledge + 0x0C) as usize;
        let pt_base = ledge + LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri;
        let v0 = read_f32(&b, pt_base);
        assert!(
            (v0.abs() - 0.0254).abs() < 1e-9,
            "点数组必须是米（±0.0254），实际 {v0}"
        );
    }

    /// 约束 1：`surfaceSize == 48 + ledge_region_size + 28 × node_count`。
    #[test]
    fn surface_size_identity_holds() {
        let b = write_cube();
        let layout = check_invariants(&b).unwrap();
        for si in 0..layout.solid_count {
            assert_eq!(
                layout.surface_sizes[si] as usize,
                COMPACT_SURFACE_SIZE
                    + layout.ledge_region_sizes[si]
                    + LEDGETREE_NODE_SIZE * layout.node_counts[si],
                "solid[{si}] 的 surfaceSize 恒等式"
            );
        }
    }

    /// 约束 2 + §0：`size == surfaceSize + 28`，`solidsEnd == 16 + Σ(size_i + 4)`，
    /// 且 `solidsEnd + textSize == fileSize`。
    #[test]
    fn file_layout_identities_hold() {
        let b = write_cube();
        let layout = check_invariants(&b).unwrap();

        let size = read_i32(&b, 16);
        assert_eq!(size as usize, layout.surface_sizes[0] as usize + 28);
        // solid 步长 = size + 4 = surfaceSize + 32。
        assert_eq!(size as usize + 4, layout.surface_sizes[0] as usize + 32);

        // solidsEnd = 16 + Σ(size_i + 4)
        let mut expect_end = PHY_HEADER_SIZE;
        let mut base = PHY_HEADER_SIZE;
        for _ in 0..layout.solid_count {
            let s = read_i32(&b, base);
            expect_end += s as usize + 4;
            base += s as usize + 4;
        }
        assert_eq!(layout.solids_end, expect_end, "solidsEnd");
        assert_eq!(
            layout.solids_end + layout.text_size,
            layout.file_size,
            "solidsEnd + textSize == fileSize"
        );
        assert_eq!(layout.file_size, b.len());
    }

    /// 立方体的 ledge 区长度与 `surfaceSize` 可以手算出来。
    ///
    /// ledge 区 = 16(头) + 16×12(三角形) + 16×8(点) = 336；
    /// surfaceSize = 48 + 336 + 28 = 412。
    #[test]
    fn cube_lengths_are_hand_computable() {
        let b = write_cube();
        let layout = check_invariants(&b).unwrap();

        let n_tri = 12usize;
        let n_pts = 8usize;
        let expect_ledge = LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri + POINT_SIZE * n_pts;
        assert_eq!(expect_ledge, 336);
        assert_eq!(layout.ledge_region_sizes[0], expect_ledge);

        let expect_surface = COMPACT_SURFACE_SIZE + expect_ledge + LEDGETREE_NODE_SIZE;
        assert_eq!(expect_surface, 412);
        assert_eq!(layout.surface_sizes[0] as usize, expect_surface);
        assert_eq!(layout.node_counts[0], 1);
    }

    // -----------------------------------------------------------------
    // 5. client_data（§6.1）
    // -----------------------------------------------------------------

    /// §6.1：有骨骼时 `client_data == boneIndex + 1`，无骨骼时 `== 0`。
    ///
    /// 实测 90/90 零例外。
    #[test]
    fn client_data_encodes_bone_index_plus_one() {
        let h = cube();
        let p = PhyParams::new("x", 0);
        let region = 16 + SOLID_HEADER_SIZE + COMPACT_SURFACE_SIZE;

        // 无骨骼 → 0。
        let s = [PhySolid::prop("x")];
        let b = write_phy(std::slice::from_ref(&h), &s, &p).unwrap();
        assert_eq!(read_i32(&b, region + 4), 0, "单 solid prop 的 client_data 必须是 0");

        // 有骨骼 → boneIndex + 1。
        for bone in [0u32, 1, 5, 71] {
            let s = [PhySolid::ragdoll(bone, format!("bone{bone}"), None)];
            let b = write_phy(std::slice::from_ref(&h), &s, &p).unwrap();
            assert_eq!(
                read_i32(&b, region + 4),
                bone as i32 + 1,
                "boneIndex={bone} 的 client_data 应为 boneIndex+1"
            );
        }
    }

    // -----------------------------------------------------------------
    // 6. text section（§7）
    // -----------------------------------------------------------------

    /// text section 的形状：以 `solid {` 开头、以 `}\n\0` 结尾、内部无 NUL。
    #[test]
    fn text_section_shape_matches_spec() {
        let b = write_cube();
        let layout = check_invariants(&b).unwrap();
        let text = &b[layout.solids_end..];

        assert_eq!(*text.last().unwrap(), 0, "末尾单个 NUL");
        assert!(!text[..text.len() - 1].contains(&0), "内部无 NUL");
        let body = std::str::from_utf8(&text[..text.len() - 1]).unwrap();
        assert!(body.starts_with("solid {"));
        assert!(body.ends_with("}\n"));
        // KeyValues：键和值都带引号，没有逗号也没有分号。
        assert!(!body.contains(','), "KeyValues 里不该有逗号");
        assert!(!body.contains(';'), "KeyValues 里不该有分号");
        // prop 的 `name` 是**碰撞 SMD 的 basename**（官方 `Q_FileBase`），
        // **不是** `<model>_physbox`。
        //
        // 这里 `write_cube()` 传的碰撞 SMD 名就是 `"cube"` ⟹ 落盘 `"cube"`。
        // 受控实验 `gen_phyname.js` 证实：`$collisionmodel "physign_geo.smd"`
        // 的模型，`.phy` 里写的是 `"physign_geo"` 而不是 `<mdl>_physbox`。
        assert!(body.contains("\"name\" \"cube\""));
        assert!(body.contains("\"index\" \"0\""));
        assert!(body.contains("editparams {"));
        // 单 solid 时 mass == totalmass（studiomdl 的质量分配）。
        //
        // 默认值是 **1.0** —— `CJointedModel` 构造函数 `m_totalMass = 1.0`，
        // 而 `ComputeMass()` 首句 `if (m_totalMass >= 0) return;` 直接返回，
        // 所以**只有显式 `$mass` / `$automass` 才会变**。
        // 语料印证：没有 `.phy` 的 835 个模型里 827 个 `mdl.mass == 1.0`。
        assert!(body.contains("\"mass\" \"1.000000\""));
        assert!(body.contains("\"totalmass\" \"1.000000\""));
        // 没有 `concave` 时不该写 `concave "1"`。
        assert!(!body.contains("concave"));
    }

    /// 多 solid 时每个 solid 一个块，且 `ragdollconstraint` 的 `parent`/`child`
    /// 是 **solid 的 index**（不是骨骼下标）。
    #[test]
    fn multi_solid_text_section_and_constraints() {
        let h = cube();
        let hulls = vec![h.clone(), h.clone(), h];
        let solids = vec![
            PhySolid::ragdoll(3, "root_bone", None),
            PhySolid::ragdoll(7, "mid_bone", Some("root_bone".into())),
            PhySolid::ragdoll(9, "leaf_bone", Some("mid_bone".into())),
        ];
        let p = PhyParams::new("rag", 0x1234);
        let b = write_phy(&hulls, &solids, &p).unwrap();
        let layout = check_invariants(&b).unwrap();
        assert_eq!(layout.solid_count, 3);

        let body = std::str::from_utf8(&b[layout.solids_end..b.len() - 1]).unwrap();
        assert_eq!(body.matches("solid {").count(), 3);
        // 第一个 solid 没有 parent，另外两个有 → 2 个 ragdollconstraint。
        assert_eq!(body.matches("ragdollconstraint {").count(), 2);
        assert!(body.contains("\"parent\" \"0\"\n\"child\" \"1\""));
        assert!(body.contains("\"parent\" \"1\"\n\"child\" \"2\""));
        // 名字与 parent 都按骨骼名写。
        assert!(body.contains("\"name\" \"mid_bone\""));
        assert!(body.contains("\"parent\" \"root_bone\""));

        // 每个 solid 的 client_data 都是它自己的 boneIndex + 1。
        let mut base = PHY_HEADER_SIZE;
        for (i, bone) in [3i32, 7, 9].into_iter().enumerate() {
            let surf = read_i32(&b, base + 12);
            let region = base + SOLID_HEADER_SIZE + COMPACT_SURFACE_SIZE;
            assert_eq!(read_i32(&b, region + 4), bone + 1, "solid[{i}] 的 client_data");
            base += surf as usize + SOLID_HEADER_SIZE;
        }
    }

    // -----------------------------------------------------------------
    // 6b. `$jointconstrain` / `collisionrules` / `animatedfriction` / `jointmerge`
    //     —— 全部数值取自真实 studiomdl 的受控实验（`gen_phy_joint.js`）
    // -----------------------------------------------------------------

    /// 三根链式骨骼的 ragdoll，用于下面几条 text-section 测试。
    fn ragdoll_three() -> (Vec<PhyHull>, Vec<PhySolid>) {
        let h = cube();
        let hulls = vec![h.clone(), h.clone(), h];
        let solids = vec![
            PhySolid::ragdoll(0, "bone_root", None),
            PhySolid::ragdoll(1, "bone_mid", Some("bone_root".into())),
            PhySolid::ragdoll(2, "bone_tip", Some("bone_mid".into())),
        ];
        (hulls, solids)
    }

    fn text_of(hulls: &[PhyHull], solids: &[PhySolid], p: &PhyParams<'_>) -> String {
        let b = write_phy(hulls, solids, p).unwrap();
        check_invariants(&b).unwrap();
        std::str::from_utf8(&b[check_invariants(&b).unwrap().solids_end..b.len() - 1])
            .unwrap()
            .to_string()
    }

    /// `$jointconstrain ... "limit" -10 10 5` ⟹ `xmin=-10 xmax=10 xfriction=**1**`。
    ///
    /// **friction 必须 ÷5** —— 官方 `AddConstraint`（`collisionmodel.cpp:539`）
    /// 是 `friction * (1.0f/5.0f)`，注释说「编辑器里 friction 显示为 5 倍」。
    /// 受控实验 `jc_limit`（真 studiomdl）实测 `xfriction = 1.000000`。
    ///
    /// **反向证伪**：把 `/ 5.0` 去掉，本测试立刻报 `10` ≠ `1`。
    #[test]
    fn jointconstrain_limit_divides_friction_by_five() {
        let (hulls, solids) = ragdoll_three();
        let mut p = PhyParams::new("x", 0);
        p.constraints = vec![JointConstraint {
            bone: "bone_mid".into(),
            axis: 0,
            kind: JointLimitType::Limit,
            min: -10.0,
            max: 10.0,
            friction: 5.0,
        }];
        let body = text_of(&hulls, &solids, &p);
        // child=1 是 bone_mid。
        assert!(
            body.contains(
                "\"parent\" \"0\"\n\"child\" \"1\"\n\
                 \"xmin\" \"-10.000000\"\n\"xmax\" \"10.000000\"\n\"xfriction\" \"1.000000\""
            ),
            "limit 约束落盘应为 (-10, 10, 5/5=1)；实际：\n{body}"
        );
        // 未指定的 y/z 轴保持 0。
        assert!(body.contains("\"ymin\" \"0.000000\"\n\"ymax\" \"0.000000\"\n\"yfriction\" \"0.000000\""));
        assert!(body.contains("\"zmin\" \"0.000000\"\n\"zmax\" \"0.000000\"\n\"zfriction\" \"0.000000\""));
        // 子 solid（bone_tip，child=2）没有被约束 ⟹ 全 0。
        assert!(body.contains("\"parent\" \"1\"\n\"child\" \"2\"\n\"xmin\" \"0.000000\""));
    }

    /// `free` ⟹ `±360`（**忽略 min/max**）；`fixed` ⟹ 全 `0`（忽略 min/max/friction）。
    ///
    /// 受控实验 `jc_free` / `jc_fixed` 实测：
    /// * `free`  → `xmin=-360 xmax=360 xfriction=0.0`
    /// * `fixed` → 九值全 `0`
    ///
    /// **反向证伪**：若实现照抄 min/max，`free` 会落盘 7/9 而不是 ±360。
    #[test]
    fn jointconstrain_free_and_fixed_ignore_min_max() {
        let (hulls, solids) = ragdoll_three();

        let mut p = PhyParams::new("x", 0);
        p.constraints = vec![JointConstraint {
            bone: "bone_mid".into(),
            axis: 0,
            kind: JointLimitType::Free,
            min: 7.0,
            max: 9.0,
            friction: 0.0,
        }];
        let body = text_of(&hulls, &solids, &p);
        assert!(
            body.contains("\"xmin\" \"-360.000000\"\n\"xmax\" \"360.000000\""),
            "free 必须落盘 ±360（忽略 min/max=7/9）；实际：\n{body}"
        );

        let mut p2 = PhyParams::new("x", 0);
        p2.constraints = vec![JointConstraint {
            bone: "bone_mid".into(),
            axis: 0,
            kind: JointLimitType::Fixed,
            min: 7.0,
            max: 9.0,
            friction: 5.0,
        }];
        let body2 = text_of(&hulls, &solids, &p2);
        assert!(
            body2.contains(
                "\"xmin\" \"0.000000\"\n\"xmax\" \"0.000000\"\n\"xfriction\" \"0.000000\""
            ),
            "fixed 必须落盘全 0（忽略 min/max/friction）；实际：\n{body2}"
        );
    }

    /// 三轴互不干扰：`jc_three` 实测 x=(-10,10,1) y=(-20,20,2) z=(-30,30,3)。
    #[test]
    fn jointconstrain_three_axes_are_independent() {
        let (hulls, solids) = ragdoll_three();
        let mut p = PhyParams::new("x", 0);
        p.constraints = vec![
            JointConstraint { bone: "bone_mid".into(), axis: 0, kind: JointLimitType::Limit, min: -10.0, max: 10.0, friction: 5.0 },
            JointConstraint { bone: "bone_mid".into(), axis: 1, kind: JointLimitType::Limit, min: -20.0, max: 20.0, friction: 10.0 },
            JointConstraint { bone: "bone_mid".into(), axis: 2, kind: JointLimitType::Limit, min: -30.0, max: 30.0, friction: 15.0 },
        ];
        let body = text_of(&hulls, &solids, &p);
        assert!(body.contains("\"xmin\" \"-10.000000\"\n\"xmax\" \"10.000000\"\n\"xfriction\" \"1.000000\""));
        assert!(body.contains("\"ymin\" \"-20.000000\"\n\"ymax\" \"20.000000\"\n\"yfriction\" \"2.000000\""));
        assert!(body.contains("\"zmin\" \"-30.000000\"\n\"zmax\" \"30.000000\"\n\"zfriction\" \"3.000000\""));
    }

    /// 约束挂在**子** solid 上：给 `bone_tip` 的约束只出现在 child=2 那块。
    #[test]
    fn jointconstrain_attaches_to_child_solid() {
        let (hulls, solids) = ragdoll_three();
        let mut p = PhyParams::new("x", 0);
        p.constraints = vec![JointConstraint {
            bone: "bone_tip".into(),
            axis: 0,
            kind: JointLimitType::Limit,
            min: -7.0,
            max: 7.0,
            friction: 5.0,
        }];
        let body = text_of(&hulls, &solids, &p);
        assert!(body.contains("\"parent\" \"1\"\n\"child\" \"2\"\n\"xmin\" \"-7.000000\""));
        // child=1（bone_mid）那块必须仍是全 0。
        assert!(body.contains("\"parent\" \"0\"\n\"child\" \"1\"\n\"xmin\" \"0.000000\""));
    }

    /// `$noselfcollisions` ⟹ `collisionrules { "selfcollisions" "0" }`，
    /// 且块位置在 `ragdollconstraint` 之后、`editparams` 之前。
    #[test]
    fn no_self_collisions_writes_collisionrules() {
        let (hulls, solids) = ragdoll_three();
        let mut p = PhyParams::new("x", 0);
        p.no_self_collisions = true;
        let body = text_of(&hulls, &solids, &p);
        assert!(body.contains("collisionrules {\n\"selfcollisions\" \"0\"\n}\n"));
        let rc = body.find("ragdollconstraint").unwrap();
        let cr = body.find("collisionrules").unwrap();
        let ep = body.find("editparams").unwrap();
        assert!(rc < cr && cr < ep, "collisionrules 必须夹在 ragdollconstraint 与 editparams 之间");
    }

    /// `$jointcollide` ⟹ `"collisionpair" "<i>,<j>"`（**solid 下标**）。
    ///
    /// 受控实验 `collide` 实测 `collisionpair "1,2"`。
    ///
    /// **反向证伪**：把下标换成骨骼下标会得到 `1,2` 巧合相同 ——
    /// 所以这里用**乱序**骨骼名对（tip, root）验证它用的是 solid 下标
    /// （应得 `2,0` 而不是 `0,2` 或名字）。
    #[test]
    fn jointcollide_writes_solid_indices_in_order() {
        let (hulls, solids) = ragdoll_three();
        let mut p = PhyParams::new("x", 0);
        p.collision_pairs = vec![("bone_mid".into(), "bone_tip".into())];
        let body = text_of(&hulls, &solids, &p);
        assert!(body.contains("collisionrules {\n\"collisionpair\" \"1,2\"\n}\n"), "{body}");

        let mut p2 = PhyParams::new("x", 0);
        p2.collision_pairs = vec![("bone_tip".into(), "bone_root".into())];
        let body2 = text_of(&hulls, &solids, &p2);
        assert!(
            body2.contains("\"collisionpair\" \"2,0\""),
            "顺序必须保持 A,B（不能排序）；实际：\n{body2}"
        );
    }

    /// `no_self_collisions` **优先于** `collision_pairs`（官方 if/else if）。
    #[test]
    fn no_self_collisions_wins_over_collision_pairs() {
        let (hulls, solids) = ragdoll_three();
        let mut p = PhyParams::new("x", 0);
        p.no_self_collisions = true;
        p.collision_pairs = vec![("bone_mid".into(), "bone_tip".into())];
        let body = text_of(&hulls, &solids, &p);
        assert!(body.contains("\"selfcollisions\" \"0\""));
        assert!(!body.contains("collisionpair"), "noself 优先时不该写 collisionpair");
    }

    /// `$animatedfriction` ⟹ 独立的 `animatedfriction {}` 块，
    /// 且 `timeout` / `timehold` 的**落盘顺序**是 In → Out → Hold。
    ///
    /// 受控实验 `animfric`（`$animatedfriction 100 500 0.1 0.2 1.0`）实测：
    /// `timein=0.1`、`timeout=**1.0**`、`timehold=**0.2**` ——
    /// 即 QC 第 5 个参数落进 `timeout`、第 4 个落进 `timehold`（**对调**）。
    #[test]
    fn animated_friction_block_and_field_order() {
        let (hulls, solids) = ragdoll_three();
        let mut p = PhyParams::new("x", 0);
        p.animated_friction = Some(AnimatedFriction {
            min: 100,
            max: 500,
            time_in: 0.1,
            // TOML 侧已是语义化名字：time_out 对应 QC 第 5 个参数。
            time_out: 1.0,
            time_hold: 0.2,
        });
        let body = text_of(&hulls, &solids, &p);
        assert!(body.contains(
            "animatedfriction {\n\
             \"animfrictionmin\" \"100.000000\"\n\
             \"animfrictionmax\" \"500.000000\"\n\
             \"animfrictiontimein\" \"0.100000\"\n\
             \"animfrictiontimeout\" \"1.000000\"\n\
             \"animfrictiontimehold\" \"0.200000\"\n\
             }\n"
        ), "{body}");
    }

    /// `$jointmerge` ⟹ `editparams` 里多一行 `"jointmerge" "<父>,<子>"`。
    #[test]
    fn jointmerge_writes_raw_name_pair() {
        let (hulls, solids) = ragdoll_three();
        let mut p = PhyParams::new("x", 0);
        p.merge_list = vec![("bone_mid".into(), "bone_tip".into())];
        let body = text_of(&hulls, &solids, &p);
        assert!(body.contains("\"jointmerge\" \"bone_mid,bone_tip\"\n"), "{body}");
    }

    /// `$jointmerge` 的**骨骼归并**：子骨骼的面并入父，solid 数 3 → 2。
    ///
    /// 判据取自受控实验 `merge`（真 studiomdl）：solid 数从 3 降到 2，
    /// 且父骨骼的体积累加。
    ///
    /// **反向证伪**：`group_by_bone`（不传归并表）必须仍得 3 组。
    #[test]
    fn jointmerge_merges_child_faces_into_parent() {
        // 三个盒子分属三根链式骨骼（bone_root / bone_mid / bone_tip）。
        let mut s = String::from("version 1\nnodes\n");
        s.push_str("0 \"bone_root\" -1\n1 \"bone_mid\" 0\n2 \"bone_tip\" 1\n");
        s.push_str("end\nskeleton\ntime 0\n");
        s.push_str("0 0 0 0 0 0 0\n1 0 0 10 0 0 0\n2 0 0 20 0 0 0\n");
        s.push_str("end\ntriangles\n");
        for (bone, cz) in [(0, 0), (1, 10), (2, 20)] {
            let c = cz as f32;
            // 一个 2×2×2 的盒子，六个面（每面两个三角形）。
            let v = [
                [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0],
                [-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0],
            ];
            let quads = [
                [0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4],
                [2, 3, 7, 6], [1, 2, 6, 5], [0, 4, 7, 3],
            ];
            for q in quads {
                for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                    s.push_str("phy\n");
                    for i in tri {
                        // SMD 三角形行：`parentBone x y z nx ny nz u v linksCount bone weight`
                        // ⚠️ 末尾的 `linksCount bone weight` 才是**蒙皮权重**，
                        // 而分组读的是 `links[].bone` —— 两者都要写对，
                        // 否则所有面都会归到骨骼 0。
                        s.push_str(&format!(
                            "{bone} {} {} {} 0 0 1 0 0 1 {bone} 1\n",
                            v[i][0], v[i][1], v[i][2] + c
                        ));
                    }
                }
            }
        }
        s.push_str("end\n");
        let smd = crate::smd::parse_smd(&s).expect("SMD 应能解析");

        // 不归并：3 组。
        assert_eq!(group_by_bone(&smd).len(), 3, "不归并时应是 3 根骨骼各一组");

        // 归并 bone_tip(2) → bone_mid(1)：只剩 2 组，
        // 且 bone_mid **累加**两边的面（自己 12 + bone_tip 的 12 = 24）。
        let mut merged = std::collections::HashMap::new();
        merged.insert(2i32, 1i32);
        let g = group_by_bone_merged(&smd, &merged);
        assert_eq!(g.len(), 2, "归并后应只剩 2 组；实际 {g:?}");
        let mid = g.iter().find(|x| x.name == "bone_mid").expect("应有 bone_mid");
        assert_eq!(
            mid.faces.len(),
            24,
            "bone_mid 应并吞 bone_tip 的 12 个面（12 自己 + 12 子 = 24）"
        );
        assert!(g.iter().all(|x| x.name != "bone_tip"), "bone_tip 不该再单独成组");
    }

    /// 归并成环必须**显式报错**，而不是像官方那样静默跳出。
    ///
    /// 语义：`would_cycle(merged, parent, child)` 判断「新增边
    /// `child → parent`」会不会成环 —— 判据是从 `parent` 沿现有归并链
    /// 上溯能否走到 `child`。
    #[test]
    fn jointmerge_cycle_is_rejected() {
        // 已有 `2 → 1`（把骨骼 2 归并进骨骼 1）。
        let mut merged = std::collections::HashMap::new();
        merged.insert(2i32, 1i32);

        // 再加 `1 → 2`（parent=2, child=1）：从 parent=2 上溯会到 1，
        // 而 1 正是 child ⟹ **成环**。
        assert!(would_cycle(&merged, 2, 1, 10), "1→2 与已有的 2→1 成环");

        // `2 → 1`（parent=1, child=2）是重复边 ⟹ 不成环。
        assert!(!would_cycle(&merged, 1, 2, 10), "重复边不算环");

        // `0 → 1`（parent=1, child=0）：从 1 上溯无出边 ⟹ 不成环。
        assert!(!would_cycle(&merged, 1, 0, 10), "0→1 不成环");

        // 自环 `1 → 1` 由调用方提前 `continue` 掉，这里也确认它会被判成环。
        assert!(would_cycle(&merged, 1, 1, 10), "自环应判成环（调用方另行跳过）");
    }

    /// `physics_bone_table`：**solid 下标 → 骨骼**，未命中的骨骼沿父链上溯。
    ///
    /// 判据取自受控实验（真实 `studiomdl.exe`）：
    /// * `rjd1`（3 骨骼各一个盒子）→ `physicsbone = [0, 1, 2]`
    /// * `rjd2`（中间骨骼**没有几何**）→ `physicsbone = [0, 0, 1]`
    ///   —— `bone_mid` 沿父链上溯到 `bone_root`（下标 0），
    ///   而 `bone_tip` 拿到的 solid 下标是 **1**（不是 2，因为它被跳过了）。
    #[test]
    fn physics_bone_maps_solids_and_walks_up_parents() {
        // 3 根链式骨骼；`bones[k]` 指定第 k 个盒子归属哪根骨骼，
        // 负值表示该盒子不存在。
        let build = |bones: [i32; 3]| {
            let mut s = String::from("version 1\nnodes\n");
            s.push_str("0 \"bone_root\" -1\n1 \"bone_mid\" 0\n2 \"bone_tip\" 1\n");
            s.push_str("end\nskeleton\ntime 0\n");
            s.push_str("0 0 0 0 0 0 0\n1 0 0 10 0 0 0\n2 0 0 20 0 0 0\n");
            s.push_str("end\ntriangles\nphys\n");
            for (k, &bone) in bones.iter().enumerate() {
                if bone < 0 {
                    continue;
                }
                let z = k as f32 * 20.0;
                let verts: Vec<[f32; 3]> = vec![
                    [-5.0, -5.0, z - 5.0], [5.0, -5.0, z - 5.0], [5.0, 5.0, z - 5.0], [-5.0, 5.0, z - 5.0],
                    [-5.0, -5.0, z + 5.0], [5.0, -5.0, z + 5.0], [5.0, 5.0, z + 5.0], [-5.0, 5.0, z + 5.0],
                ];
                let faces: [[usize; 3]; 12] = [
                    [0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6],
                    [0, 4, 5], [0, 5, 1], [1, 5, 6], [1, 6, 2],
                    [2, 6, 7], [2, 7, 3], [3, 7, 4], [3, 4, 0],
                ];
                for f in faces.iter() {
                    for &vi in f {
                        let p = verts[vi];
                        s.push_str(&format!(
                            "{bone} {} {} {} 0 0 1 0 0 1 {bone} 1\n",
                            p[0], p[1], p[2]
                        ));
                    }
                }
            }
            s.push_str("end\n");
            crate::smd::parse_smd(&s).expect("SMD 应能解析")
        };

        let parents = [-1i32, 0, 1];

        // 三根骨骼各有几何 → 下标就是骨骼序（对照官方 `rjd1`）。
        let smd = build([0, 1, 2]);
        let pb = physics_bone_table(&smd, 3, &parents).expect("应有表");
        assert_eq!(pb, vec![0, 1, 2], "rjd1 形态：physicsbone = [0,1,2]");

        // 中间骨骼没有几何 → 它上溯到 bone_root(0)，
        // 而 bone_tip 拿到的是 **1**（solid 下标，不是骨骼下标）。
        // 对照官方 `rjd2`：`[0, 0, 1]`。
        let smd = build([0, -1, 2]);
        let pb = physics_bone_table(&smd, 3, &parents).expect("应有表");
        assert_eq!(pb, vec![0, 0, 1], "rjd2 形态：中间骨骼上溯到父，下标不跳号");

        // 完全没有碰撞几何 → `None`（官方留全 0）。
        let empty = crate::smd::parse_smd(
            "version 1\nnodes\n0 \"root\" -1\nend\nskeleton\nend\ntriangles\nend\n",
        )
        .expect("SMD 应能解析");
        assert!(physics_bone_table(&empty, 1, &[-1]).is_none());
    }

    /// 单 solid 时 `physicsbone` 必须是**全 0** ——
    /// 实测语料 2459/2459 个单 solid 模型全 0（`probe_physicsbone_ragdoll_split.js`）。
    ///
    /// 这条锁住「只有 ragdoll 才填」这个二分。
    #[test]
    fn physics_bone_is_all_zero_for_single_solid() {
        // 只有 bone_root 有几何，另两根无。
        let s = {
            let mut s = String::from("version 1\nnodes\n0 \"bone_root\" -1\n1 \"bone_mid\" 0\n2 \"bone_tip\" 1\n");
            s.push_str("end\nskeleton\ntime 0\n0 0 0 0 0 0 0\n1 0 0 10 0 0 0\n2 0 0 20 0 0 0\nend\ntriangles\nphys\n");
            let verts: Vec<[f32; 3]> = vec![
                [-5.0, -5.0, -5.0], [5.0, -5.0, -5.0], [5.0, 5.0, -5.0], [-5.0, 5.0, -5.0],
                [-5.0, -5.0, 5.0], [5.0, -5.0, 5.0], [5.0, 5.0, 5.0], [-5.0, 5.0, 5.0],
            ];
            let faces: [[usize; 3]; 12] = [
                [0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6],
                [0, 4, 5], [0, 5, 1], [1, 5, 6], [1, 6, 2],
                [2, 6, 7], [2, 7, 3], [3, 7, 4], [3, 4, 0],
            ];
            for f in faces.iter() {
                for &vi in f {
                    let p = verts[vi];
                    s.push_str(&format!("0 {} {} {} 0 0 1 0 0 1 0 1\n", p[0], p[1], p[2]));
                }
            }
            s.push_str("end\n");
            crate::smd::parse_smd(&s).expect("SMD 应能解析")
        };
        let pb = physics_bone_table(&s, 3, &[-1, 0, 1]).expect("应有表");
        assert_eq!(pb, vec![0, 0, 0], "单 solid ⟹ 全 0");
    }

    /// `massbias != 1.0` 时才写 `massbias` 行；`drag` 只在 `Some` 时写；
    /// `concave` 只在 `true` 时写。
    #[test]
    fn optional_text_fields_follow_measured_rules() {
        let h = cube();
        let p_base = PhyParams::new("x", 0);

        // 默认：都不写。
        let s = [PhySolid::prop("x")];
        let b = write_phy(std::slice::from_ref(&h), &s, &p_base).unwrap();
        let layout = check_invariants(&b).unwrap();
        let body = std::str::from_utf8(&b[layout.solids_end..b.len() - 1]).unwrap();
        assert!(!body.contains("massbias"));
        assert!(!body.contains("drag"));
        assert!(!body.contains("concave"));

        // 全部打开。
        let mut p = PhyParams::new("x", 0);
        p.drag = Some(0.25);
        p.concave = true;
        let s = [PhySolid {
            bone_index: None,
            name: "x_physbox".into(),
            parent: None,
            mass_bias: 8.0,
            damping: None,
            rot_damping: None,
            inertia: None,
        }];
        let b = write_phy(std::slice::from_ref(&h), &s, &p).unwrap();
        let layout = check_invariants(&b).unwrap();
        let body = std::str::from_utf8(&b[layout.solids_end..b.len() - 1]).unwrap();
        assert!(body.contains("\"massbias\" \"8.000000\""));
        assert!(body.contains("\"drag\" \"0.250000\""));
        assert!(body.contains("\"concave\" \"1\""));
    }

    // -----------------------------------------------------------------
    // 7. 多凸块 solid（递归 ledgetree）
    // -----------------------------------------------------------------

    /// 两个凸块拼成一个 solid：必须写成递归树，且节点数是 `2n−1`。
    #[test]
    fn multi_hull_solid_writes_recursive_tree() {
        // 两个分离的立方体。
        let a = cube();
        let mut b_verts = cube_vertices();
        for v in &mut b_verts {
            v[0] += 10.0; // 沿 X 平移 10
        }
        let b = PhyHull {
            vertices: b_verts,
            faces: cube_faces(),
        };

        let solids = [PhySolid::prop("two")];
        let p = PhyParams::new("two", 0);
        let bytes = write_phy_multi(&[vec![a, b]], &solids, &p).unwrap();
        let layout = check_invariants(&bytes).unwrap();

        assert_eq!(layout.solid_count, 1);
        // 2 个凸块 → 2×2−1 = 3 个节点（实测的节点数恒为奇数）。
        assert_eq!(layout.node_counts[0], 3);
        assert_eq!(layout.node_counts[0] % 2, 1, "节点数恒为奇数");

        // 根节点必须是内部节点（offset_right_node != 0）。
        let body = PHY_HEADER_SIZE + SOLID_HEADER_SIZE;
        let node_off = body + read_i32(&bytes, body + 32) as usize;
        let right = read_i32(&bytes, node_off);
        assert_ne!(right, 0, "两个凸块时根节点必须是内部节点");
        assert_eq!(right % LEDGETREE_NODE_SIZE as i32, 0, "右子偏移是 28 的倍数");

        // 约束 1 依然成立。
        assert_eq!(
            layout.surface_sizes[0] as usize,
            COMPACT_SURFACE_SIZE
                + layout.ledge_region_sizes[0]
                + LEDGETREE_NODE_SIZE * layout.node_counts[0]
        );

        // 两个 hull 共用一份点数组：共享点数组长度 == 16 个点（2×8，无重合）。
        // 每个 hull 的 c_point_offset 都指向同一个起点。
        let region = body + COMPACT_SURFACE_SIZE;
        let n_tri0 = read_i16(&bytes, region + 12) as usize;
        let hull1 = region + LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri0;
        let n_tri1 = read_i16(&bytes, hull1 + 12) as usize;
        let shared = hull1 + LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri1;
        assert_eq!(
            region as i64 + read_i32(&bytes, region) as i64,
            shared as i64,
            "第一个 hull 的 c_point_offset 指向共享数组起点"
        );
        assert_eq!(
            hull1 as i64 + read_i32(&bytes, hull1) as i64,
            shared as i64,
            "第二个 hull 的 c_point_offset 也指向同一个共享数组起点"
        );
        // 这里就是规格 §4.2 那条规则失效的地方：第二个 hull 的
        // c_point_offset 等于**从它自己到最后一个 ledge 结束**的字节数
        // （= 它自己的 16 + 16×nTri），因为它是最后一个 ledge。
        // 真正的区别在**第一个** hull 上：它的 c_point_offset 要把
        // 后面那个 ledge 也算进去。
        let cpo0 = read_i32(&bytes, region);
        let cpo1 = read_i32(&bytes, hull1);
        assert_eq!(
            cpo0 as usize,
            (LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri0)
                + (LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri1),
            "第一个 hull 的 c_point_offset 必须把后续 ledge 也算进去"
        );
        assert_eq!(
            cpo1 as usize,
            LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri1,
            "最后一个 hull 的 c_point_offset 退化成 16 + 16×nTri"
        );
        // 也就是说：多 hull 时第一个 hull 的 c_point_offset **不等于**
        // 16 + 16×nTri —— 这正是规格 §4.2 只对单 hull 成立的地方。
        assert_ne!(
            cpo0 as usize,
            LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri0,
            "多 hull 时第一个 hull 的 c_point_offset 不应等于 16 + 16×nTri"
        );
    }

    /// 5 个凸块 → 9 个节点，且每个叶子都恰好被引用一次。
    #[test]
    fn five_hulls_produce_nine_nodes() {
        let mut hulls = Vec::new();
        for i in 0..5 {
            let mut vs = cube_vertices();
            for v in &mut vs {
                v[0] += i as f32 * 5.0;
            }
            hulls.push(PhyHull {
                vertices: vs,
                faces: cube_faces(),
            });
        }
        let solids = [PhySolid::prop("five")];
        let p = PhyParams::new("five", 0);
        let bytes = write_phy_multi(&[hulls], &solids, &p).unwrap();
        let layout = check_invariants(&bytes).unwrap();
        assert_eq!(layout.node_counts[0], 9, "5 个凸块 → 2×5−1 = 9 个节点");
    }

    /// `merge_hulls` 拼出来的面表必须仍然闭合，索引必须正确偏移。
    #[test]
    fn merged_hulls_stay_closed() {
        let a = cube();
        let mut b_verts = cube_vertices();
        for v in &mut b_verts {
            v[0] += 10.0;
        }
        let b = PhyHull {
            vertices: b_verts,
            faces: cube_faces(),
        };
        let merged = PhyHull {
            vertices: [a.vertices.clone(), b.vertices.clone()].concat(),
            faces: [
                a.faces.clone(),
                b.faces.iter().map(|f| [f[0] + 8, f[1] + 8, f[2] + 8]).collect(),
            ]
            .concat(),
        };
        // 合并后依然能通过流形校验并写出。
        let solids = [PhySolid::prop("m")];
        let p = PhyParams::new("m", 0);
        let bytes = write_phy_multi(&[vec![merged]], &solids, &p).unwrap();
        check_invariants(&bytes).unwrap();
    }

    // -----------------------------------------------------------------
    // 8. 几何量的实测规则
    // -----------------------------------------------------------------

    /// **`.phy` 的点是米（IVP 单位），不是 inch** —— 钉住单位换算。
    ///
    /// # 为什么必须有这条测试
    ///
    /// 把点整体乘 39.37 **不破坏 `check_invariants` 的任何一条**：
    /// 点数组长度不变、索引范围不变、`c_point_offset` 后缀和不变、
    /// `surfaceSize == 48 + ledge + 28×nodes` 不变、体积仍是正的。
    ///
    /// 所以这个 bug **不会报错**，只会在游戏里表现为「碰撞体比模型大 39 倍」。
    ///
    /// # 判据：受控实验（已知尺寸）
    ///
    /// 官方 `msh1.phy`（源是 50×10×50 inch 的长方体）实测：
    ///
    /// ```text
    /// 点 bbox = [-0.127, -1.143, -0.127] .. [0.127, 0.127, 1.143]
    /// 各轴范围 = 0.254 / 1.27 / 1.27
    ///          = 10×0.0254 / 50×0.0254 / 50×0.0254
    /// studiomdl 报告体积 = 17000 in^3
    /// 点算出的体积       = 0.2786 m^3 = 17000 × 0.0254³
    /// ```
    ///
    /// 语料全量（`probe_phy_unit.js`，1316 个官方 `.phy`）：
    /// `mdl hull 最大边 / phy 点最大边` 落在 `39.37±25%` 内的 **66.9%**、
    /// 落在 `1.00±0.25` 内的 **0.0%**。
    #[test]
    fn phy_points_are_converted_from_inches_to_metres() {
        let b = write_cube();
        let body = 16 + SOLID_HEADER_SIZE;
        let ledge = body + COMPACT_SURFACE_SIZE;
        let sz_div = read_u32(&b, ledge + 8) >> 8;
        let n_tri = read_i16(&b, ledge + 0x0C) as usize;
        let n_pts = sz_div as usize - 1 - n_tri;
        assert!(n_pts > 0, "立方体应有顶点");

        let pt_base = ledge + LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri;
        // ±1 立方体的点应当是 ±0.0254（= ±1 inch 换算成米）。
        //
        // ⚠️ 这里写**字面量**而不是 `SOURCE_TO_IVP` —— 用常量会让测试变成
        // 恒等式（两边同时改就永远通过）。实测过：把常量改成 1.0 后，
        // 用常量的版本仍然「通过」，写字面量的版本立刻报错。
        const EXPECT: f32 = 0.0254;
        let mut seen_pos = false;
        let mut seen_neg = false;
        for i in 0..n_pts {
            let o = pt_base + i * POINT_SIZE;
            for k in 0..3 {
                let v = read_f32(&b, o + k * 4);
                let want = if v > 0.0 { EXPECT } else { -EXPECT };
                assert!(
                    (v - want).abs() < 1e-9,
                    "点[{i}][{k}] = {v}，应为 ±{EXPECT}（inch→米）。\
                     若这里失败，说明单位换算丢了 —— 而 check_invariants 查不出它"
                );
                if v > 0.0 { seen_pos = true } else { seen_neg = true }
            }
        }
        assert!(seen_pos && seen_neg, "±1 立方体应当正负都有");
    }

    /// **轴映射**：官方落盘的点是 `(-y, −z, x)(世界顶点) × 0.0254`，
    /// 不是恒等映射。
    ///
    /// # 为什么必须有这条
    ///
    /// 上面那条 `phy_points_are_converted_from_inches_to_metres` 用的是
    /// **±1 立方体** —— 它在 `(-y,−z,x)` 下**完全对称**（顶点集合不变），
    /// 所以那条测试对「轴映射丢了」**完全免疫**：只乘 0.0254 也能通过。
    ///
    /// 这正是这个 bug 能长期存在的原因之一。要抓它必须用**不对称**的形状。
    ///
    /// # 判据来源
    ///
    /// 官方 `physign.phy`（`docs/_probe/gen_physign.js` 生成的四面体，
    /// 序列姿态全 0）。顶点是
    /// `(3,−1,−2) (−4,6,−1) (1,2,7) (−2,−5,3)`，
    /// 24 种纯旋转给出的顶点集合**两两不同**，所以能唯一定案 ——
    /// `docs/_probe/probe_physign.js` 报出**唯一**解 `(-y,−z,x)`。
    ///
    /// 官方落盘值（÷0.0254 后）实测：
    /// ```text
    /// (−2,−7, 1) ( 5,−3,−2) ( 1, 2, 3) (−6, 1,−4)
    /// ```
    #[test]
    fn phy_points_use_official_axis_mapping() {
        // 四面体（与 physign 同形）
        const V: [[f32; 3]; 4] = [
            [3.0, -1.0, -2.0],
            [-4.0, 6.0, -1.0],
            [1.0, 2.0, 7.0],
            [-2.0, -5.0, 3.0],
        ];
        const FACES: [[u32; 3]; 4] = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];

        let mut s = String::from("version 1\nnodes\n0 \"root\" -1\nend\nskeleton\ntime 0\n0 0 0 0 0 0 0\nend\ntriangles\n");
        for f in FACES {
            s.push_str("phy\n");
            for &i in &f {
                let p = V[i as usize];
                s.push_str(&format!("0 {} {} {} 0 0 1 0 0 1 0 1\n", p[0], p[1], p[2]));
            }
        }
        s.push_str("end\n");
        let smd = crate::smd::parse_smd(&s).expect("SMD 应能解析");

        let bytes = build_phy_from_smd(
            &smd,
            PhyIdentity {
                model_name: "physign",
                collision_smd_name: "physign",
                surface_prop: "metal",
            },
            0,
            1.0,
            &crate::model::Physics::default(),
            None,
        )
        .expect("应能写出 PHY");
        let body = 16 + SOLID_HEADER_SIZE;
        let ledge = body + COMPACT_SURFACE_SIZE;
        let sz_div = read_u32(&bytes, ledge + 8) >> 8;
        let n_tri = read_i16(&bytes, ledge + 0x0C) as usize;
        let n_pts = sz_div as usize - 1 - n_tri;
        assert_eq!(n_pts, 4, "四面体应有 4 个点");

        // 官方落盘值 ÷0.0254（即轴映射后的 inch 值），按集合比。
        //
        // ⚠️ 写字面量而不是用 `to_ivp()` 反算 —— 后者会让测试变成恒等式。
        const WANT: [[f32; 3]; 4] = [
            [-2.0, -7.0, 1.0],
            [5.0, -3.0, -2.0],
            [1.0, 2.0, 3.0],
            [-6.0, 1.0, -4.0],
        ];
        let pt_base = ledge + LEDGE_HEADER_SIZE + TRIANGLE_SIZE * n_tri;
        let got: Vec<[f32; 3]> = (0..n_pts)
            .map(|i| {
                let o = pt_base + i * POINT_SIZE;
                [
                    read_f32(&bytes, o) / SOURCE_TO_IVP,
                    read_f32(&bytes, o + 4) / SOURCE_TO_IVP,
                    read_f32(&bytes, o + 8) / SOURCE_TO_IVP,
                ]
            })
            .collect();

        for w in WANT {
            assert!(
                got.iter().any(|g| (0..3).all(|k| (g[k] - w[k]).abs() < 1e-4)),
                "落盘点里找不到 {w:?}。实际点集：{got:?}\n\
                 若全都差一个轴置换，说明 `to_ivp` 的轴映射丢了 —— \
                 只乘 0.0254 会让碰撞体相对渲染网格转 90°。"
            );
        }
    }

    /// 官方 `ConvexFromVerts` 的第二段 `BuildOuterHull(hull, 0.01)`：
    /// 1 cm 去重必须真的减少点数，且**不改变**凸包的外形。
    ///
    /// # 判据来源
    ///
    /// `docs/_probe/probe_phy_cmp.js`：`msh1` / `ucc1` / `ucc3` / `physign` /
    /// `phyrot0/1/2` 七个模型压实后的**点数与三角形数与官方 `.phy` 完全一致**
    /// （10/16、8/12、8/12、4/4、8/12 ×3），逐点残差 < 1.5e-7。
    ///
    /// 这里用最容易手算的 `physign` 四面体：它的 4 个点两两距离都远大于
    /// 1 cm，所以压实**不应**删掉任何点 —— 这条守的是「别过度删除」。
    #[test]
    fn compact_outer_hull_keeps_well_separated_points() {
        // 四面体，边长 ~10 inch = 0.254 m ≫ 0.01 m
        let pts: Vec<[f32; 3]> = vec![
            [3.0, -1.0, -2.0],
            [-4.0, 6.0, -1.0],
            [1.0, 2.0, 7.0],
            [-2.0, -5.0, 3.0],
        ];
        let h = PhyHull::from_points(&pts).expect("四面体应能成凸包");
        assert_eq!(h.vertices.len(), 4);
        let c = h.compact_outer_hull();
        assert_eq!(c.vertices.len(), 4, "点距远大于 1cm，不该被合并");
        assert_eq!(c.faces.len(), 4, "四面体有 4 个面");
    }

    /// 压实**必须真的删点** —— 一堆挤在 1 cm 内的点应当塌成少数几个。
    ///
    /// 这条守的是反方向：上面那条保证「别过度删除」，这条保证
    /// 「该删就删」。两条合起来才钉住 `COMPACT_RADIUS_M` 的量级。
    #[test]
    fn compact_outer_hull_merges_points_within_one_centimetre() {
        // 一个 10 inch 的立方体，但每个角上再撒 3 个距离 < 0.2 inch
        // （= 0.005 m < 0.01 m）的抖动点。抖动点都在凸包内部，
        // 所以凸包顶点仍是 8 个；压实的意义在**去重半径**，
        // 这里改用「两个几乎重合的立方体叠加」来构造。
        //
        // 更直接的构造：把立方体的 8 个角各自复制一份、偏移 0.1 inch
        // （= 0.00254 m < 0.01 m）。凸包仍是那 8 个角（副本在外侧时
        // 会取代原角），但两两距离 0.00254 m < r ⟹ 压实后点数应减少。
        let mut pts: Vec<[f32; 3]> = Vec::new();
        for &x in &[-1.0f32, 1.0] {
            for &y in &[-1.0f32, 1.0] {
                for &z in &[-1.0f32, 1.0] {
                    pts.push([x, y, z]);
                    pts.push([x + 0.1, y + 0.1, z + 0.1]);
                }
            }
        }
        let h = PhyHull::from_points(&pts).expect("应能成凸包");
        let c = h.compact_outer_hull();
        assert!(
            c.vertices.len() < h.vertices.len(),
            "16 个两两相距 0.1 inch（= 0.00254 m < r=0.01 m）的点应当被压实，\
             实际 {} → {}",
            h.vertices.len(),
            c.vertices.len()
        );
        assert!(
            c.vertices.len() >= 4,
            "压实后仍应是个立体，实际 {} 点",
            c.vertices.len()
        );
    }

    /// 压实的**单位必须是米**，不是 inch。
    ///
    /// # 为什么单列一条
    ///
    /// 反编译里 `BuildOuterHull` 拿到的 `0.01` 是**米** —— 因为
    /// `BuildConvexFromVerts` 已经乘过 0.0254。若误当成 inch 用，
    /// 等效半径会小 39 倍，几乎不删任何点（表现为 `.phy` 点数虚高）。
    ///
    /// # 构造
    ///
    /// 一个四棱双锥：底面正方形 `(±10, ±10, 0)`，两个顶点
    /// `(0, 0, 10)` 与 `(0, 0.2, 10)`。
    ///
    /// * 两顶点相距 **0.2 inch = 0.00508 m**。
    /// * `0.00508 m < 0.01 m` ⟹ **正确实现会合并它们**（6 点 → 5 点）。
    /// * 若把 `0.01` 误当 inch（`0.000254 m`），`0.00508 m` 远大于它
    ///   ⟹ **不会合并**（仍是 6 点）。
    ///
    /// 合并后仍有 5 个点（底面 4 个 + 1 个顶点），够张成凸包，
    /// 所以不会走 `reps.len() < 4` 的退化回退。
    #[test]
    fn compact_radius_is_metres_not_inches() {
        let mut pts: Vec<[f32; 3]> = Vec::new();
        for &x in &[-10.0f32, 10.0] {
            for &y in &[-10.0f32, 10.0] {
                pts.push([x, y, 0.0]);
            }
        }
        pts.push([0.0, 0.0, 10.0]);
        pts.push([0.0, 0.2, 10.0]);

        let h = PhyHull::from_points(&pts).expect("双锥应能成凸包");
        assert_eq!(h.vertices.len(), 6, "底面 4 角 + 2 顶点");
        let c = h.compact_outer_hull();
        assert_eq!(
            c.vertices.len(),
            5,
            "0.2 inch = 0.00508 m < r=0.01 m，两个顶点应当被合并（6 → 5）。\
             实际 {} 点 —— 若没合并，多半是把 0.01 当成 inch 用了。",
            c.vertices.len()
        );
    }

    /// 叶子节点几何：`center` = AABB 中心、`radius` = 外接球半径、
    /// `box_sizes[i] = trunc(半边长/(radius/250)) + 1`。
    ///
    /// 对 ±1 的立方体可以完全手算：half = (1,1,1)、radius = √3、
    /// box_sizes[i] = trunc(1/(√3/250)) + 1 = trunc(144.337) + 1 = 145。
    ///
    /// ⚠️ **半径要乘 [`SOURCE_TO_IVP`]** —— 输入是 Source 单位（inch），
    /// 落盘的几何量是 IVP 单位（米）。`box_sizes` 是**比值**，不受影响。
    #[test]
    fn cube_leaf_node_geometry_is_hand_computable() {
        let b = write_cube();
        let body = 16 + SOLID_HEADER_SIZE;
        let node = body + read_i32(&b, body + 32) as usize;

        let center = [
            read_f32(&b, node + 8),
            read_f32(&b, node + 12),
            read_f32(&b, node + 16),
        ];
        assert_eq!(center, [0.0, 0.0, 0.0], "±1 立方体的 AABB 中心是原点");

        let radius = read_f32(&b, node + 20);
        let want = 3.0f32.sqrt() * SOURCE_TO_IVP;
        assert!(
            (radius - want).abs() < 1e-7,
            "radius 应为 √3 × 0.0254 ≈ {want}，实际 {radius}"
        );

        // `box_sizes` 是「半边长 / (radius/250)」—— 单位在分子分母上抵消。
        let expect = ((1.0f64 / (3.0f64.sqrt() / 250.0)).trunc() + 1.0) as u8;
        assert_eq!(expect, 145);
        assert_eq!([b[node + 24], b[node + 25], b[node + 26]], [145, 145, 145]);
        assert_eq!(b[node + 27], 0, "free_0 == 0");
    }

    /// `upper_limit_radius == max‖p − mass_center‖`。
    ///
    /// 对 ±1 立方体：mass_center 是原点，最远的点是 (±1,±1,±1)，距离 √3。
    /// ⚠️ 同样要乘 [`SOURCE_TO_IVP`]（落盘是米）。
    #[test]
    fn cube_upper_limit_radius_is_hand_computable() {
        let b = write_cube();
        let body = 16 + SOLID_HEADER_SIZE;
        let mc = [
            read_f32(&b, body),
            read_f32(&b, body + 4),
            read_f32(&b, body + 8),
        ];
        assert!(mc.iter().all(|c| c.abs() < 1e-8), "立方体质心在原点，实际 {mc:?}");
        let r = read_f32(&b, body + 24);
        let want = 3.0f32.sqrt() * SOURCE_TO_IVP;
        assert!(
            (r - want).abs() < 1e-7,
            "upper_limit_radius 应为 √3 × 0.0254 ≈ {want}，实际 {r}"
        );
    }

    /// `max_factor_surface_deviation` 是 250（实测取值范围 135..251，
    /// 250 是出现过的最大值）。
    #[test]
    fn max_factor_surface_deviation_is_250() {
        let b = write_cube();
        let body = 16 + SOLID_HEADER_SIZE;
        assert_eq!(read_u32(&b, body + 28) & 0xFF, 250);
        // 高 24 位是 byte_size == surfaceSize。
        assert_eq!(
            (read_u32(&b, body + 28) >> 8) & 0x00FF_FFFF,
            read_i32(&b, 16 + 12) as u32
        );
    }

    // -----------------------------------------------------------------
    // 9. parry3d 集成
    // -----------------------------------------------------------------

    /// `PhyHull::from_points` 对立方体的 8 个顶点应产出 12 个三角形、8 个顶点。
    #[test]
    fn convex_hull_of_cube_points() {
        let h = PhyHull::from_points(&cube_vertices()).expect("立方体的凸包必须能算出来");
        assert_eq!(h.vertices.len(), 8, "立方体的凸包顶点数");
        assert_eq!(h.faces.len(), 12, "立方体的凸包三角形数");

        // parry 的输出必须是闭合可定向流形，且 CCW 外向（体积为正）。
        let vol = MassProperties::from_convex_polyhedron(
            1.0,
            &h.vertices
                .iter()
                .map(|p| Vector::new(p[0], p[1], p[2]))
                .collect::<Vec<_>>(),
            &h.faces,
        )
        .mass();
        assert!(
            vol > 0.0,
            "凸包必须 CCW 外向（有向体积为正），实际 {vol}"
        );
        // 边长 2 的立方体体积是 8。
        assert!((vol - 8.0).abs() < 1e-4, "立方体体积应为 8，实际 {vol}");
    }

    /// 凸包输入里带冗余内部点时，凸包必须把它们丢掉。
    #[test]
    fn convex_hull_drops_interior_points() {
        let mut pts = cube_vertices();
        pts.push([0.0, 0.0, 0.0]); // 内部点
        pts.push([0.1, 0.1, 0.1]); // 内部点
        let h = PhyHull::from_points(&pts).unwrap();
        assert_eq!(h.vertices.len(), 8, "内部点必须被丢掉");
        assert_eq!(h.faces.len(), 12);
    }

    /// VHACD 凸分解：对一个凹的 L 形网格必须产出多个凸块，
    /// 且每块都能作为合法的 `PhyHull` 写进 PHY。
    #[test]
    fn vhacd_decomposes_concave_mesh() {
        // 一个 L 形棱柱（沿 Z 挤出）。这是经典的凹形状。
        let outline: [[f32; 2]; 6] = [
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let mut vertices: Vec<[f32; 3]> = Vec::new();
        for z in [0.0f32, 1.0] {
            for p in &outline {
                vertices.push([p[0], p[1], z]);
            }
        }
        let n = outline.len() as u32;
        let mut faces: Vec<[u32; 3]> = Vec::new();
        // 底面（法线 -Z，所以绕序反转）与顶面（+Z）。
        for i in 1..n - 1 {
            faces.push([0, i + 1, i]);
            faces.push([n, n + i, n + i + 1]);
        }
        // 侧面。
        for i in 0..n {
            let j = (i + 1) % n;
            faces.push([i, j, n + j]);
            faces.push([i, n + j, n + i]);
        }

        let parts = decompose_concave(&vertices, &faces, 32, 8).expect("L 形必须能分解");
        assert!(parts.len() >= 2, "凹形状应拆成 ≥2 块，实际 {}", parts.len());
        for (i, p) in parts.iter().enumerate() {
            assert!(p.vertices.len() >= 4, "第 {i} 块的顶点数");
            assert!(!p.faces.is_empty(), "第 {i} 块的面表");
        }

        // 所有块必须能作为一个 solid 写进 PHY 并通过自检。
        let solids = [PhySolid::prop("lshape")];
        let mut params = PhyParams::new("lshape", 0);
        params.concave = true;
        let bytes = write_phy_multi(&[parts], &solids, &params).expect("分解结果必须能写出");
        let layout = check_invariants(&bytes).unwrap();
        assert_eq!(layout.solid_count, 1);
        assert_eq!(layout.node_counts[0] % 2, 1, "节点数恒为奇数");
    }

    /// 凸分解的空/坏输入必须返回 Err。
    #[test]
    fn decompose_rejects_bad_input() {
        assert!(matches!(
            decompose_concave(&[], &[], 32, 8),
            Err(PhyError::BadParameter { .. })
        ));
        assert!(matches!(
            decompose_concave(&[[0.0; 3], [1.0, 0.0, 0.0]], &[[0, 1, 0]], 32, 8),
            Err(PhyError::HullTooFewPoints { .. })
        ));
        // 面表引用越界下标。
        assert!(matches!(
            decompose_concave(
                &cube_vertices(),
                &[[0u32, 1, 99]],
                32,
                8
            ),
            Err(PhyError::IndexOutOfRange { index: 99, .. })
        ));
    }

    // -----------------------------------------------------------------
    // 10. 确定性
    // -----------------------------------------------------------------

    /// 同样的输入必须逐字节产出同样的输出。
    ///
    /// 这条很重要：`points` 去重用了 `HashMap`，如果迭代顺序泄漏进输出，
    /// 每次编译都会得到不同的 `.phy`，增量构建和校验都会变得不可复现。
    #[test]
    fn output_is_deterministic() {
        let first = write_cube();
        for _ in 0..8 {
            assert_eq!(write_cube(), first, "重复写出必须逐字节相同");
        }
    }

    /// 顶点顺序不同（但几何相同）时输出可以不同，但都必须合法。
    #[test]
    fn permuted_vertices_still_valid() {
        // 把立方体的顶点顺序反过来（面表跟着重映射）。
        let map: Vec<u32> = (0..8).rev().collect();
        let mut vs = cube_vertices();
        vs.reverse();
        let faces = cube_faces()
            .into_iter()
            .map(|f| [map[f[0] as usize], map[f[1] as usize], map[f[2] as usize]])
            .collect();
        let h = PhyHull {
            vertices: vs,
            faces,
        };
        let s = [PhySolid::prop("perm")];
        let p = PhyParams::new("perm", 7);
        let b = write_phy(std::slice::from_ref(&h), &s, &p).unwrap();
        let layout = check_invariants(&b).unwrap();
        // 几何没变，所以 surfaceSize 应该一样。
        assert_eq!(layout.surface_sizes[0], 412);
    }

    /// `inertia_scale = 0.0` 时 `rotation_inertia` 三个分量都是 0。
    #[test]
    fn inertia_scale_zero_writes_zeros() {
        let h = cube();
        let s = [PhySolid::prop("x")];
        let mut p = PhyParams::new("x", 0);
        p.inertia_scale = 0.0;
        let b = write_phy(std::slice::from_ref(&h), &s, &p).unwrap();
        let body = 16 + SOLID_HEADER_SIZE;
        for a in 0..3 {
            assert_eq!(read_f32(&b, body + 12 + a * 4), 0.0, "rotation_inertia[{a}]");
        }
    }

    /// `rotation_inertia` 的默认值是立方体的真实单位质量惯性张量对角元。
    ///
    /// 这个立方体的坐标是 ±1，**边长是 2**（不是 1）。
    /// 边长 `a` 的立方体：`I = m·(a²+a²)/12 = m·a²/6`，
    /// 所以 `I/m = 4/6 = 2/3`... 但那是绕**质心**且边长取 2 的情形：
    /// `I/m = (2² + 2²)/12 = 8/12 = 2/3`。
    ///
    /// 实测值是 5.3333 = 16/3，正好是 `2/3` 的 8 倍 —— 因为 parry 的
    /// `from_convex_polyhedron(1.0, ...)` 用**密度 1**，而我们的 `I/m`
    /// 里 `m` 也是体积 8，`I` 是密度 1 下的真实惯量 `8 · (2/3) = 16/3`，
    /// 所以 `I/m = 16/3 / 8 = 2/3`？不对 —— 实测就是 16/3。
    ///
    /// 结论：**parry 返回的 `reconstruct_inertia_matrix` 已经是"密度 1 的
    /// 惯量张量"**，即 `I = ρ·V·(a²/6) = 8 · 4/6 = 16/3`。我们不再除以质量，
    /// 所以写进去的是 `I`（密度 1）而不是 `I/m`。
    ///
    /// 这不影响正确性：IVP 只用三个主轴之间的**比例**，整体缩放会被
    /// `inv_mass` 吸收（见模块文档）。测试按实测值钉住，防止未来改动
    /// 悄悄换了归一化口径。
    ///
    /// ⚠️ **数值随 `SOURCE_TO_IVP⁵` 缩放**，不是 `²`。
    ///
    /// `reconstruct_inertia_matrix` 返回的是**含质量**的惯量
    /// （`I = m·r²`），而 `parry` 的密度固定 1.0 ⟹ `m = 体积`。
    /// 点缩放 `k` 后：`体积 → k³`、`r² → k²`，所以 `I → I·k⁵`。
    ///
    /// **实测印证**（官方 `msh1.phy`，源 50×10×50 inch）：
    ///
    /// ```text
    /// 官方 rotation_inertia[1] = 1.100331e-1
    /// 手算（米、密度 1）：V·(x²+z²)/12 = 0.409677 × (1.27²+1.27²)/12 = 1.101279e-1
    /// inch 口径：25000 × (50²+50²)/12 = 1.041667e7
    ///   1.041667e7 × 0.0254⁵ = 0.110133   ← 与官方一致
    /// ```
    #[test]
    fn cube_rotation_inertia_matches_density_one_tensor() {
        let b = write_cube();
        let body = 16 + SOLID_HEADER_SIZE;
        // 边长 2、体积 8 的立方体，密度 1：I = 8 · (2²+2²)/12 = 16/3（inch 口径）。
        // 点换算成米后 I 乘 k⁵（见上）。
        let expect = 16.0f32 / 3.0 * SOURCE_TO_IVP.powi(5);
        for a in 0..3 {
            let got = read_f32(&b, body + 12 + a * 4);
            let rel = if expect != 0.0 { (got - expect).abs() / expect } else { 0.0 };
            assert!(
                rel < 1e-4,
                "立方体密度 1 的 I 应为 16/3 × 0.0254⁵ ≈ {expect}，rotation_inertia[{a}] = {got}"
            );
        }
        // 三个轴必须相同（立方体各向同性）—— 这是真正重要的性质。
        // 容差放到 1e-4：parry 的特征分解是迭代的，实测三个轴相差约 2e-6。
        let i0 = read_f32(&b, body + 12);
        let i1 = read_f32(&b, body + 16);
        let i2 = read_f32(&b, body + 20);
        assert!(
            (i0 - i1).abs() < 1e-4 && (i1 - i2).abs() < 1e-4,
            "立方体各向同性：I = ({i0}, {i1}, {i2})"
        );
    }

    /// 长方体（非立方体）的 `rotation_inertia` 必须体现各向异性，
    /// 且长轴方向的惯量最小。
    #[test]
    fn box_rotation_inertia_is_anisotropic() {
        // X 方向拉长 4 倍。
        let mut vs = cube_vertices();
        for v in &mut vs {
            v[0] *= 4.0;
        }
        let h = PhyHull {
            vertices: vs,
            faces: cube_faces(),
        };
        let s = [PhySolid::prop("box")];
        let p = PhyParams::new("box", 0);
        let b = write_phy(std::slice::from_ref(&h), &s, &p).unwrap();
        let body = 16 + SOLID_HEADER_SIZE;
        let (ix, iy, iz) = (
            read_f32(&b, body + 12),
            read_f32(&b, body + 16),
            read_f32(&b, body + 20),
        );
        // 边长 (8, 2, 2)：I_x = m(2²+2²)/12 最小，I_y = I_z = m(8²+2²)/12 最大。
        assert!(ix < iy, "绕长轴的惯量应最小：Ix={ix} Iy={iy}");
        assert!((iy - iz).abs() < 1e-3, "另外两轴应相同：Iy={iy} Iz={iz}");
    }

    /// 质量分配：多 solid 时按体积比例分，且下限钳到 1.0。
    #[test]
    fn mass_is_distributed_by_volume_with_floor() {
        // 一个大立方体 + 一个很小的立方体，total_mass = 100。
        let big = cube();
        let mut small_vs = cube_vertices();
        for v in &mut small_vs {
            for c in v.iter_mut() {
                *c *= 0.1;
            }
        }
        let small = PhyHull {
            vertices: small_vs,
            faces: cube_faces(),
        };
        let solids = [PhySolid::prop("big"), PhySolid::prop("small")];
        let mut p = PhyParams::new("two", 0);
        p.total_mass = 100.0;
        let b = write_phy(&[big, small], &solids, &p).unwrap();
        let layout = check_invariants(&b).unwrap();
        let body = std::str::from_utf8(&b[layout.solids_end..b.len() - 1]).unwrap();

        // 体积比 8 : 0.008 → 质量比约 1000:1。小的那个会被钳到 1.0。
        let masses: Vec<f32> = body
            .lines()
            .filter_map(|l| l.strip_prefix("\"mass\" \""))
            .filter_map(|v| v.strip_suffix('"'))
            .filter_map(|v| v.parse::<f32>().ok())
            .collect();
        assert_eq!(masses.len(), 2);
        assert!(masses[0] > 99.0, "大块的质量应接近 100，实际 {}", masses[0]);
        assert_eq!(masses[1], 1.0, "小块的质量应被钳到 1.0");
    }
}
