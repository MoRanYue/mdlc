//! SMD（Studiomodel 源文件）解析。
//!
//! # 为什么网格必须放在 SMD 而不是描述文件里
//!
//! 真实模型的顶点数是几万到几十万（官方 `v_autoshotgun` 有 388,765 个顶点）。
//! 把网格写进 TOML 会让描述文件膨胀到几百 MB 且无法用文本工具处理 ——
//! 所以网格由 SMD/DMX 承载，描述文件只**引用**它们，这与 QC 的
//! `$bodygroup { studio "x.smd" }` 设计一致。
//!
//! # 格式（实测自真实文件）
//!
//! ```text
//! version 1
//! nodes
//!   <index> "<name>" <parent>          ← parent = -1 为根
//!   ...
//! end
//! skeleton
//!   time <frame>
//!   <boneIndex> <px> <py> <pz> <rx> <ry> <rz>
//!   ...
//! end
//! triangles
//!   <materialName>
//!   <parentBone> <px py pz> <nx ny nz> <u v> <links> <bone> <weight> [...]
//!   ... （每个三角形 3 行）
//! end
//! ```
//!
//! # 四条容易踩的规则
//!
//! 1. **三角形顶点行有 12 个 token**，第一个是 `parentBone`。漏掉它会让
//!    解析器把坐标当成骨骼下标 —— 实测 studiomdl 会报 `bogus bone index`。
//! 2. **三角形没有索引行**。每个三角形是「材质名行 + 3 个顶点行」，
//!    写成像 OBJ 那样的 `0 1 2` 索引行会让 studiomdl 报
//!    `error on g_szLine N` 然后崩溃。
//! 3. **`skeleton` 段的旋转是弧度**，不是角度。实测真实文件里
//!    `1.570796`（= π/2），且 MDL 的 `mstudiobone_t.rotation` 也存弧度，
//!    两边口径一致、直接搬运即可。
//! 4. **`nodes` 的 index 不保证等于行序**，要用它自己的字段做下标；
//!    而 `skeleton` 段的行序与 `nodes` 的顺序一一对应。

use std::collections::HashMap;
use std::fmt;

/// SMD 解析错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmdError {
    /// 出错行号（1 起）。
    pub line: usize,
    pub message: String,
}

impl fmt::Display for SmdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "第 {} 行：{}", self.line, self.message)
    }
}

impl std::error::Error for SmdError {}

fn err(line: usize, message: impl Into<String>) -> SmdError {
    SmdError {
        line,
        message: message.into(),
    }
}

/// `nodes` 段的一个节点（骨骼）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmdNode {
    /// 文件里声明的下标（不保证等于行序）。
    pub index: i32,
    pub name: String,
    /// 父节点下标，-1 为根。
    pub parent: i32,
}

/// `skeleton` 段里某一帧的一根骨骼。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmdPose {
    /// 骨骼下标（指向 [`SmdNode::index`]）。
    pub bone: i32,
    pub position: [f32; 3],
    /// **弧度**（与 MDL 一致，不是角度）。
    pub rotation: [f32; 3],
}

/// `skeleton` 段的一帧。
#[derive(Debug, Clone, PartialEq)]
pub struct SmdFrame {
    pub time: i32,
    /// 按 `nodes` 顺序排列的骨骼姿态。
    pub poses: Vec<SmdPose>,
}

/// 一个骨骼绑定（骨骼下标 + 权重）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmdBoneLink {
    pub bone: i32,
    pub weight: f32,
}

/// 三角形顶点行（12 个 token）。
#[derive(Debug, Clone, PartialEq)]
pub struct SmdVertex {
    /// 行首的 `parentBone` —— **不是**蒙皮骨骼，只是顶点所属的父骨骼。
    pub parent_bone: i32,
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// 蒙皮绑定，1..=3 组（SMD 允许更多，本实现只保留前 3 组）。
    pub links: Vec<SmdBoneLink>,
}

/// 一个三角形：材质名 + 3 个顶点。
#[derive(Debug, Clone, PartialEq)]
pub struct SmdTriangle {
    pub material: String,
    pub vertices: [SmdVertex; 3],
}

/// 解析后的 SMD。
#[derive(Debug, Clone, PartialEq)]
pub struct Smd {
    pub version: i32,
    pub nodes: Vec<SmdNode>,
    pub frames: Vec<SmdFrame>,
    pub triangles: Vec<SmdTriangle>,
}

impl Smd {
    /// 骨骼名 → 下标。
    pub fn node_index(&self) -> HashMap<&str, usize> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.as_str(), i))
            .collect()
    }

    /// 按**首次出现顺序**收集去重后的材质名。
    ///
    /// 顺序很重要：它决定材质表的下标，而 mesh 通过下标引用材质。
    pub fn materials_in_order(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for t in &self.triangles {
            if !seen.contains(&t.material) {
                seen.push(t.material.clone());
            }
        }
        seen
    }

    /// 第 0 帧（参考姿态）。无帧时返回 `None`。
    pub fn reference_frame(&self) -> Option<&SmdFrame> {
        self.frames.first()
    }
}

// 早先这里有一个 `tokens(line) -> Vec<&str>` 辅助函数（`split_whitespace`
// 后 `collect`）。它**每行分配一次堆内存**，是 `parse_smd` 最大的分配来源，
// 已删除 —— 现在 `parse_smd` 复用一个 token 缓冲，见那里的说明。
//
// 切词规则未变：按空白切分，**不**处理引号内的空格（SMD 的引号只用于名字，
// 而名字里不含空格 —— 这是格式约定，不是本实现的简化）。

fn parse_f32(tok: &str, line: usize, what: &str) -> Result<f32, SmdError> {
    tok.parse::<f32>()
        .map_err(|_| err(line, format!("{what} 不是合法浮点数：{tok:?}")))
}

fn parse_i32(tok: &str, line: usize, what: &str) -> Result<i32, SmdError> {
    // SMD 里偶见 `0.000000` 形式的整数（导出器不严谨），先试 i32 再退 f32。
    if let Ok(v) = tok.parse::<i32>() {
        return Ok(v);
    }
    tok.parse::<f32>()
        .map(|v| v as i32)
        .map_err(|_| err(line, format!("{what} 不是合法整数：{tok:?}")))
}

/// 去掉名字两侧的引号。
fn unquote(s: &str) -> String {
    s.trim_matches('"').to_string()
}

/// 解析 SMD 文本。
///
/// # 分配策略（性能关键）
///
/// 本函数**按行**处理，每行都要切词。早期版本用 `line.split_whitespace()
/// .collect::<Vec<_>>()` —— 那是**每行一次堆分配**（100 万三角形约
/// 1200 万次）。现在改成**复用一个 token 缓冲**（`clear()` + `extend()`），
/// 全程只有一次分配。
///
/// 语义完全不变：`t` 在每个分支里看到的仍是同一行、同一顺序的 token。
pub fn parse_smd(text: &str) -> Result<Smd, SmdError> {
    let mut version: Option<i32> = None;
    let mut nodes: Vec<SmdNode> = Vec::new();
    let mut frames: Vec<SmdFrame> = Vec::new();
    let mut triangles: Vec<SmdTriangle> = Vec::new();

    /// 当前所在的段。
    #[derive(PartialEq)]
    enum Section {
        None,
        Nodes,
        Skeleton,
        Triangles,
    }
    let mut section = Section::None;
    // 当前帧（skeleton 段）。
    let mut frame: Option<SmdFrame> = None;
    // 当前三角形正在累积的顶点（triangles 段）。
    //
    // 用**定长数组 + 计数**而不是 `Vec<SmdVertex>`：后者在收满 3 个顶点时
    // 要 `pending[0..2].clone()`，而 `SmdVertex` 里的 `links` 是 `Vec`，
    // clone 会连带再分配一次。改用数组后是**移动**，零分配、零克隆。
    let mut current_material: Option<String> = None;
    let mut pending: [Option<SmdVertex>; 3] = [None, None, None];
    let mut pending_len = 0usize;
    // 复用的 token 缓冲（见函数文档）。
    let mut t: Vec<&str> = Vec::with_capacity(32);
    // 三角形数粗估：顶点行约占 6 行、每行约 90 字节。只是省几次 realloc，
    // 估错也不影响正确性。
    triangles.reserve(text.len() / 600);

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        // 去掉行内注释（`//` 起），再切词。
        let body = match raw.find("//") {
            Some(p) => &raw[..p],
            None => raw,
        };
        t.clear();
        t.extend(body.split_whitespace());
        if t.is_empty() {
            continue;
        }

        match t[0] {
            "version" => {
                let v = t
                    .get(1)
                    .ok_or_else(|| err(line, "version 后面缺少数字"))?;
                version = Some(parse_i32(v, line, "version")?);
                continue;
            }
            "nodes" => {
                section = Section::Nodes;
                continue;
            }
            "skeleton" => {
                section = Section::Skeleton;
                continue;
            }
            "triangles" => {
                section = Section::Triangles;
                continue;
            }
            "end" => {
                // 段结束：skeleton 段要把当前帧收尾。
                if section == Section::Skeleton
                    && let Some(f) = frame.take()
                {
                    frames.push(f);
                }
                // triangles 段收尾：未满 3 个顶点的残留**静默丢弃**。
                //
                // # 为什么是丢弃而不是报错（oracle 实测）
                //
                // 官方 studiomdl 对「三角形数不是 3 的倍数」是**静默截断**的。
                // 判据（`docs/_probe/smdl/iph1.qc` + `iph.smd`）：
                //
                // | 项 | 值 |
                // |---|---|
                // | `iph.smd` 的 triangles 段顶点行 | **7**（= 2 组 + 1 行残留） |
                // | 官方 `iph1.vvd` 的 `numLODVertexes[0]` | **6** |
                // | 官方 stdout/stderr | **无任何相关警告** |
                //
                // 且重编官方产物与既有 `artifacts/iph1.mdl` 逐字节相同
                // （sha `C73F16BF…`）—— 所以这不是「旧产物残留」。
                //
                // 语料侧同类证据：`tb1.smd` 也是 4 行（1 组 + 1 行残留），
                // 官方同样产出 `.mdl`。
                //
                // ⟹ 早先这里 `return Err(...)` 比官方**更严**，会让
                // 官方能编的 QC 在 mdlc 侧失败。改为丢弃（与官方一致）。
                //
                // ⚠️ 丢弃的是**残留的不足一组**的顶点行，不是「多出来的
                // 完整三角形」—— 后者仍会全部保留。
                if section == Section::Triangles {
                    pending = [None, None, None];
                    pending_len = 0;
                }
                section = Section::None;
                current_material = None;
                continue;
            }
            _ => {}
        }

        match section {
            Section::None => {
                return Err(err(line, format!("段外出现无法识别的内容：{:?}", t[0])));
            }
            Section::Nodes => {
                if t.len() < 3 {
                    return Err(err(line, "nodes 行应为 `<index> \"<name>\" <parent>`"));
                }
                nodes.push(SmdNode {
                    index: parse_i32(t[0], line, "node index")?,
                    name: unquote(t[1]),
                    parent: parse_i32(t[2], line, "node parent")?,
                });
            }
            Section::Skeleton => {
                // `time <n>` 开启新帧。
                if t[0] == "time" {
                    if let Some(f) = frame.take() {
                        frames.push(f);
                    }
                    let v = t.get(1).ok_or_else(|| err(line, "time 后面缺少帧号"))?;
                    frame = Some(SmdFrame {
                        time: parse_i32(v, line, "frame time")?,
                        poses: Vec::new(),
                    });
                    continue;
                }
                if t.len() < 7 {
                    return Err(err(
                        line,
                        "skeleton 行应为 `<bone> <px py pz> <rx ry rz>`（7 个 token）",
                    ));
                }
                let f = frame
                    .as_mut()
                    .ok_or_else(|| err(line, "skeleton 段在 time 之前出现了姿态行"))?;
                f.poses.push(SmdPose {
                    bone: parse_i32(t[0], line, "pose bone")?,
                    position: [
                        parse_f32(t[1], line, "pos.x")?,
                        parse_f32(t[2], line, "pos.y")?,
                        parse_f32(t[3], line, "pos.z")?,
                    ],
                    rotation: [
                        parse_f32(t[4], line, "rot.x")?,
                        parse_f32(t[5], line, "rot.y")?,
                        parse_f32(t[6], line, "rot.z")?,
                    ],
                });
            }
            Section::Triangles => {
                // 材质名行：非数字开头。
                let is_vertex_line = t[0].parse::<f32>().is_ok();
                if !is_vertex_line {
                    // 新的材质名 —— 只有在上一个三角形已收满时才合法。
                    if pending_len != 0 {
                        return Err(err(
                            line,
                            format!(
                                "材质名 {:?} 出现在未完成的三角形中间（已有 {} 个顶点行）",
                                t[0], pending_len
                            ),
                        ));
                    }
                    current_material = Some(unquote(t[0]));
                    continue;
                }

                // 顶点行：12 个 token。
                if t.len() < 12 {
                    return Err(err(
                        line,
                        format!(
                            "三角形顶点行需要至少 12 个 token（parentBone + pos3 + nrm3 + uv2 + links + bone + weight），实际 {}",
                            t.len()
                        ),
                    ));
                }
                let links_count = parse_i32(t[9], line, "links 数")?;
                // `links == 0` 是**合法**的：顶点没有任何骨骼权重。
                //
                // # 判据（oracle 实测）
                //
                // `docs/_probe/smdl/tb1.smd` 的三个顶点都写 `links = 0`，
                // 官方 studiomdl 正常产出 `.mdl`（1732 字节，重编可复现），
                // 且 `.vvd` 里第 3 个顶点是 **`boneCount = 0`**：
                //
                // ```text
                // v2 weights=[0.0000,0.0000,0.0000] bones=[0,0,0] boneCount=0
                // ```
                //
                // ⟹ 官方允许「无权重顶点」，`boneCount = 0`。
                // 早先这里 `links_count < 1` 直接报错，比官方**更严**，
                // 会让官方能编的 QC 在 mdlc 侧失败（853 个真实 QC 的普查
                // 暴露了 `tb1`）。
                if links_count < 0 {
                    return Err(err(line, format!("links 数不能为负，实际 {links_count}")));
                }
                let mut links = Vec::with_capacity(links_count as usize);
                let mut k = 10usize;
                for n in 0..links_count as usize {
                    if k + 1 >= t.len() {
                        return Err(err(
                            line,
                            format!("声明了 {links_count} 组绑定，但第 {} 组不完整", n + 1),
                        ));
                    }
                    links.push(SmdBoneLink {
                        bone: parse_i32(t[k], line, "绑定骨骼")?,
                        weight: parse_f32(t[k + 1], line, "绑定权重")?,
                    });
                    k += 2;
                }

                // 材质名只在**收满一个三角形时**克隆一次，而不是每个顶点
                // 克隆一次。原先每顶点 `current_material.clone()` 会分配
                // 一个 `String`（100 万三角形约 300 万次）；这里改成先借用，
                // 到 `triangles.push` 时才 `to_string()`。
                let material = current_material.as_deref().ok_or_else(|| {
                    err(line, "顶点行出现在任何材质名之前")
                })?;

                pending[pending_len] = Some(SmdVertex {
                    parent_bone: parse_i32(t[0], line, "parentBone")?,
                    position: [
                        parse_f32(t[1], line, "pos.x")?,
                        parse_f32(t[2], line, "pos.y")?,
                        parse_f32(t[3], line, "pos.z")?,
                    ],
                    normal: [
                        parse_f32(t[4], line, "nrm.x")?,
                        parse_f32(t[5], line, "nrm.y")?,
                        parse_f32(t[6], line, "nrm.z")?,
                    ],
                    uv: [
                        parse_f32(t[7], line, "u")?,
                        // **V 翻转**（`v → 1 - v`）。
                        //
                        // # 为什么
                        //
                        // studiomdl 在**解析 SMD 时**就翻转 V ——
                        // `hl2sdk-episode1\utils\studiomdl\v1support.cpp:161`：
                        //
                        // ```c
                        // // invert v
                        // t[1] = 1.0 - t[1];
                        // ```
                        //
                        // 翻转后的值才写进 `s_source_t.vertex[].texcoord`，
                        // 因此它同时影响 **VVD 的 UV** 与**切线的计算**
                        // （`CalcTriangleTangentSpace` 读的就是这个 texcoord）。
                        //
                        // # 实测证据（两条独立判据）
                        //
                        // 1. **SMD ↔ 官方 VVD 逐顶点对照**：`myprop` 立方体
                        //    8/8 个顶点的 `vvd.uv[1] == 1 - smd.v`；
                        //    `rb` 3/3 同样（`docs/_probe/probe_smd_vs_vvd_uv.js`）。
                        // 2. **切线手性**：不翻转时 w 吻合率 **99.96%**（看似很高，
                        //    但剩下的 0.04% 是系统性错误）；翻转后 w 是
                        //    **100%**（`dbg_vflip.js`：不翻转 0/8，
                        //    翻转 8/8）。
                        //
                        // 注意：VVD 里存的**已经是翻转后**的值，所以
                        // 「读真实 VVD 反推切线」时不该再翻一次 ——
                        // 那正是 `probe_vflip_corpus.js` 里
                        // 「再翻转」把吻合率从 99.96% 打到 2.29% 的原因。
                        // 翻转只发生**一次**，在 SMD 解析这一步。
                        1.0 - parse_f32(t[8], line, "v")?,
                    ],
                    links,
                });
                pending_len += 1;

                if pending_len == 3 {
                    // 三个顶点**移动**进三角形（不是 clone）—— 原先的
                    // `pending[i].clone()` 会连带把每个顶点的 `links` Vec
                    // 再分配一次，即每三角形 3 次多余分配。
                    let v: [SmdVertex; 3] = [
                        pending[0].take().expect("已收满 3 个顶点"),
                        pending[1].take().expect("已收满 3 个顶点"),
                        pending[2].take().expect("已收满 3 个顶点"),
                    ];
                    pending_len = 0;
                    triangles.push(SmdTriangle {
                        material: material.to_string(),
                        vertices: v,
                    });
                }
            }
        }
    }

    // 文件在段内结束时也要收尾（不报错，SMD 允许省略末尾的 end）。
    if let Some(f) = frame.take() {
        frames.push(f);
    }
    // 同 `end` 分支：不足一组的顶点行**静默丢弃**（官方实测行为）。
    //
    // 这里不需要显式清空 `pending` —— 局部数组在函数返回时自动消失，
    // 且 `pending_len` 此后不再被读。写 `pending = [None; 3]` 只会触发
    // `unused_assignments` 警告。

    let version = version.ok_or(SmdError {
        line: 1,
        message: "缺少 `version` 行".into(),
    })?;
    if version != 1 {
        return Err(SmdError {
            line: 1,
            message: format!("只支持 version 1，实际为 {version}"),
        });
    }
    if nodes.is_empty() {
        return Err(SmdError {
            line: 1,
            message: "`nodes` 段为空 —— 至少需要一根骨骼".into(),
        });
    }

    Ok(Smd {
        version,
        nodes,
        frames,
        triangles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与真实文件同构的最小 SMD：两根骨骼、一个三角形。
    const MINIMAL: &str = r#"version 1
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

    #[test]
    fn parses_minimal_smd() {
        let s = parse_smd(MINIMAL).expect("最小 SMD 必须能解析");
        assert_eq!(s.version, 1);
        assert_eq!(s.nodes.len(), 2);
        assert_eq!(s.nodes[0].name, "root");
        assert_eq!(s.nodes[0].parent, -1);
        assert_eq!(s.nodes[1].parent, 0);
        assert_eq!(s.frames.len(), 1);
        assert_eq!(s.frames[0].poses.len(), 2);
        assert_eq!(s.frames[0].poses[1].position, [0.0, 0.0, 8.0]);
        assert_eq!(s.triangles.len(), 1);
        assert_eq!(s.triangles[0].material, "myprop");
        assert_eq!(s.triangles[0].vertices[0].position, [-8.0, -8.0, 0.0]);
        assert_eq!(s.triangles[0].vertices[0].normal, [0.0, 0.0, 1.0]);
        // SMD 里写的是 `u=0 v=0`，但 **V 会被翻转为 1 - v**（见解析处的说明）。
        // 这条断言钉住翻转确实发生 —— 去掉翻转会让 UV 与官方 VVD 不一致，
        // 并让切线手性 w 系统性错一个符号。
        assert_eq!(s.triangles[0].vertices[0].uv, [0.0, 1.0]);
        assert_eq!(s.triangles[0].vertices[0].parent_bone, 1);
        assert_eq!(s.triangles[0].vertices[0].links.len(), 1);
        assert_eq!(s.triangles[0].vertices[0].links[0].bone, 1);
        assert_eq!(s.triangles[0].vertices[0].links[0].weight, 1.0);
    }

    #[test]
    fn rotation_is_radians_not_degrees() {
        // 真实文件里出现 1.570796（= π/2），必须是弧度原样保留。
        let text = MINIMAL.replace(
            "0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000",
            "0 0.000000 0.000000 0.000000 1.570796 0.000000 0.000000",
        );
        let s = parse_smd(&text).unwrap();
        assert!((s.frames[0].poses[0].rotation[0] - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn multiple_materials_are_collected_in_first_appearance_order() {
        let text = MINIMAL.replace(
            "myprop\n  1 -8.000000",
            "b_mat\n  1 -8.000000",
        ) + "triangles\nz_mat\n  1 0 0 0 0 0 1 0 0 1 1 1.0\n  1 1 0 0 0 0 1 1 0 1 1 1.0\n  1 0 1 0 0 0 1 0 1 1 1 1.0\na_mat\n  1 0 0 0 0 0 1 0 0 1 1 1.0\n  1 1 0 0 0 0 1 1 0 1 1 1.0\n  1 0 1 0 0 0 1 0 1 1 1 1.0\nend\n";
        let s = parse_smd(&text).unwrap();
        assert_eq!(s.materials_in_order(), vec!["b_mat", "z_mat", "a_mat"]);
    }

    #[test]
    fn rejects_index_lines_instead_of_vertices() {
        // 写成 OBJ 风格的索引行 → token 数不足，必须报错而不是静默读错。
        let bad = MINIMAL.replace(
            "  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000\n  1 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000\n  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000",
            "0 1 2",
        );
        let e = parse_smd(&bad).unwrap_err();
        assert!(e.message.contains("12 个 token"), "{e}");
    }

    #[test]
    fn rejects_vertex_line_with_11_tokens() {
        // 漏掉行首 parentBone 的经典写法。
        let bad = MINIMAL.replace(
            "  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000",
            "  -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000",
        );
        let e = parse_smd(&bad).unwrap_err();
        assert!(e.message.contains("12 个 token"), "{e}");
    }

    /// **不足一组的顶点行必须被静默丢弃**（官方实测行为）。
    ///
    /// # 判据（oracle，不是推测）
    ///
    /// `docs/_probe/smdl/iph.smd` 的 triangles 段有 **7** 行顶点
    /// （2 组 + 1 行残留），官方 studiomdl 编译 `iph1.qc` 后：
    ///
    /// * 产物 `iph1.vvd` 的 `numLODVertexes[0] == 6`（只算了 2 组）；
    /// * stdout/stderr **无任何相关警告**；
    /// * 重编结果与既有 `artifacts/iph1.mdl` 逐字节相同（sha `C73F16BF…`）。
    ///
    /// 同类：`tb1.smd` 也是 4 行（1 组 + 1 行残留），官方同样产出 `.mdl`。
    ///
    /// ⚠️ 本测试**曾经断言相反的结论**（`rejects_incomplete_triangle`，
    /// 要求报错）。那是「比官方更严」，会让官方能编的 QC 在 mdlc 侧失败 ——
    /// 853 个真实 QC 的普查把它暴露了出来（`iph1`/`mvz`/`tb1` 三个用例
    /// 有官方产物却解析失败）。**旧断言是错的，已按 oracle 改正。**
    #[test]
    fn incomplete_trailing_triangle_group_is_silently_dropped() {
        // 删掉 3 个顶点行中的 1 个 ⟹ 剩 2 行，不足一组。
        let bad = MINIMAL.replace(
            "  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000\n",
            "",
        );
        let smd = parse_smd(&bad).expect("不足一组的残留必须被丢弃，而不是报错");
        // 残留的 2 行被丢掉 ⟹ 三角形数应为 0（本夹具只有 1 个三角形）。
        assert_eq!(
            smd.triangles.len(),
            0,
            "残留顶点行不得构成三角形，也不得让解析失败"
        );
        // 但 nodes / skeleton 必须完好 —— 丢弃只作用于 triangles 段的残留。
        assert_eq!(smd.nodes.len(), 2, "nodes 段不受影响");
        assert!(!smd.frames.is_empty(), "skeleton 段不受影响");
    }

    #[test]
    fn rejects_missing_version() {
        let bad = MINIMAL.replace("version 1\n", "");
        let e = parse_smd(&bad).unwrap_err();
        assert!(e.message.contains("version"), "{e}");
    }

    #[test]
    fn rejects_unsupported_version() {
        let bad = MINIMAL.replace("version 1", "version 2");
        let e = parse_smd(&bad).unwrap_err();
        assert!(e.message.contains("只支持 version 1"), "{e}");
    }

    #[test]
    fn rejects_empty_nodes() {
        let bad = MINIMAL.replace("  0 \"root\" -1\n  1 \"tip\" 0\n", "");
        let e = parse_smd(&bad).unwrap_err();
        assert!(e.message.contains("nodes"), "{e}");
    }

    #[test]
    fn tolerates_crlf_and_comments_and_missing_final_end() {
        let text = MINIMAL
            .replace('\n', "\r\n")
            .replace("version 1", "// 由某导出器生成\r\nversion 1")
            .trim_end()
            .trim_end_matches("end")
            .to_string();
        let s = parse_smd(&text).expect("CRLF + 注释 + 省略末尾 end 都应容忍");
        assert_eq!(s.triangles.len(), 1);
        assert_eq!(s.frames.len(), 1);
    }

    #[test]
    fn parses_multi_link_weights() {
        let text = MINIMAL.replace(
            "1 1 1.000000\n  1 8.000000",
            "2 1 0.500000 0 0.500000\n  1 8.000000",
        );
        let s = parse_smd(&text).unwrap();
        let l = &s.triangles[0].vertices[0].links;
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].bone, 1);
        assert_eq!(l[0].weight, 0.5);
        assert_eq!(l[1].bone, 0);
        assert_eq!(l[1].weight, 0.5);
    }

    #[test]
    fn counts_links_correctly_when_extra_tokens_present() {
        // 有些导出器在权重之后还有多余列 —— 只要 links 数对，不应读错。
        let text = MINIMAL.replace(
            "1 1 1.000000\n  1 8.000000",
            "1 1 1.000000 99 99\n  1 8.000000",
        );
        let s = parse_smd(&text).unwrap();
        assert_eq!(s.triangles[0].vertices[0].links.len(), 1);
        assert_eq!(s.triangles[0].vertices[0].links[0].weight, 1.0);
    }

    /// 真实文件验证（存在则跑，不存在则跳过并打印）。
    #[test]
    fn parses_real_decompiled_smd() {
        let p = r"D:\GITHUB\plank\target\decompile-sample\body2_model0.smd";
        let Ok(text) = std::fs::read_to_string(p) else {
            eprintln!("跳过：找不到真实素材 {p}");
            return;
        };
        let s = parse_smd(&text).expect("真实 SMD 必须能解析");
        assert_eq!(s.version, 1);
        assert_eq!(s.nodes.len(), 89);
        assert_eq!(s.nodes[0].name, "ValveBiped.ValveBiped");
        assert_eq!(s.nodes[0].parent, -1);
        // 参考姿态：bone0 的 rot.x 是 π/2（弧度，不是 90）。
        assert!((s.frames[0].poses[0].rotation[0] - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(s.triangles.len(), 22911);
        assert_eq!(s.materials_in_order(), vec!["shield", "chain"]);
    }

    // ---------------------------------------------------------------------
    // 分配优化的回归测试（见 `parse_smd` 的「分配策略」文档）
    // ---------------------------------------------------------------------
    //
    // 这一组测试钉住**优化后的可观察行为**。它们不直接量分配次数
    // （那要全局分配器，不适合放单元测试），而是钉住「一旦为了省分配
    // 而改变了语义，就会立刻失败」的那些点。

    /// **顶点必须是「移动」进三角形的，不是「共享」的。**
    ///
    /// 优化把 `pending: Vec<SmdVertex>` 换成了 `[Option<SmdVertex>; 3]`，
    /// 收满时用 `take()` 取出。若有人误改成复用同一组槽位（只 `clone` 或不
    /// 清空），第 2、3 个三角形就会拿到前一个三角形的顶点 —— 这里用
    /// **两个材质不同、坐标不同**的三角形把它钉死。
    #[test]
    fn consecutive_triangles_do_not_share_pending_vertices() {
        let text = String::from(MINIMAL)
            + "triangles\nsecond_mat\n  1 1 1 0 0 0 1 0 0 1 1 1.0\n  1 2 2 0 0 0 1 1 0 1 1 1.0\n  1 3 3 0 0 0 1 0 1 1 1 1.0\nend\n";
        let s = parse_smd(&text).unwrap();
        assert_eq!(s.triangles.len(), 2, "两个三角形都应保留");
        assert_eq!(s.triangles[0].material, "myprop");
        assert_eq!(s.triangles[1].material, "second_mat");
        // 第 2 个三角形的顶点必须来自它自己那 3 行，而不是第 1 个的残留。
        assert_eq!(s.triangles[1].vertices[0].position, [1.0, 1.0, 0.0]);
        assert_eq!(s.triangles[1].vertices[1].position, [2.0, 2.0, 0.0]);
        assert_eq!(s.triangles[1].vertices[2].position, [3.0, 3.0, 0.0]);
        // 第 1 个三角形也不能被后续覆盖。
        assert_eq!(s.triangles[0].vertices[0].position, [-8.0, -8.0, 0.0]);
    }

    /// **材质名是「每个三角形克隆一次」，不是「每个顶点克隆一次」。**
    ///
    /// 优化把 `current_material.clone()`（每顶点）挪到了 push 时（每三角形）。
    /// 若有人把它改成借用/复用，多个三角形的材质名就会串味 ——
    /// 尤其是「同一材质出现多个三角形」与「材质在中间切换」两种情形。
    #[test]
    fn material_is_per_triangle_and_switching_is_exact() {
        // 材质 A 两个三角形 → 材质 B 一个三角形 → 材质 A 再现。
        let mut text = String::from(MINIMAL);
        text.push_str("triangles\nA\n");
        for k in 0..3 {
            text.push_str(&format!(
                "  1 {k} 0 0 0 0 1 0 0 1 1 1.0\n"
            ));
        }
        text.push_str("B\n");
        for k in 0..3 {
            text.push_str(&format!("  1 0 {k} 0 0 0 1 0 0 1 1 1.0\n"));
        }
        text.push_str("A\n");
        for k in 0..3 {
            text.push_str(&format!("  1 0 0 {k} 0 0 1 0 0 1 1 1.0\n"));
        }
        text.push_str("end\n");
        let s = parse_smd(&text).unwrap();
        assert_eq!(s.triangles.len(), 4, "1（MINIMAL）+ 3");
        assert_eq!(s.triangles[0].material, "myprop");
        assert_eq!(s.triangles[1].material, "A");
        assert_eq!(s.triangles[2].material, "B");
        assert_eq!(s.triangles[3].material, "A", "材质名必须逐三角形独立");
        // 材质表按首次出现顺序，且 A 只出现一次。
        assert_eq!(s.materials_in_order(), vec!["myprop", "A", "B"]);
    }

    /// **`links` 的全部内容必须原样保留**（含超过 3 组的）。
    ///
    /// 解析层**不允许**截断到 3 组 —— 截断是 `compile.rs` 里「按权重降序
    /// 排序后取前 3」的职责。真实语料里有 **3040 个顶点**的权重不是降序的，
    /// 解析期截断会选错骨骼（实测会造成 9178 处字段不一致）。
    ///
    /// 本测试用 5 组绑定钉住「解析层不截断」，并用**权重乱序**模拟真实
    /// 导出器（这样「先截断再排序」与「先排序再截断」结果必然不同）。
    #[test]
    fn parser_keeps_all_links_beyond_three_including_unsorted_weights() {
        // 5 组绑定，权重故意**升序**写（真实导出器不保证降序）。
        // 按权重降序取前 3 应该是 bone 4,3,2；若解析期截断则只剩 0,1,2。
        let text = MINIMAL.replace(
            "1 1 1.000000\n  1 8.000000",
            "5 4 0.10 3 0.20 2 0.30 1 0.35 0 0.05\n  1 8.000000",
        );
        let s = parse_smd(&text).unwrap();
        let l = &s.triangles[0].vertices[0].links;
        assert_eq!(l.len(), 5, "解析层必须保留全部 5 组绑定，不得截断");
        assert_eq!(
            l.iter().map(|x| x.bone).collect::<Vec<_>>(),
            vec![4, 3, 2, 1, 0],
            "顺序必须与文件一致"
        );

        // 再验证「排序后取前 3」得到的是 1,2,3（权重 0.35/0.30/0.20），
        // 而不是「截断后取前 3」的 4,3,2 —— 两者不同，正是本测试的意义。
        let mut sorted = l.clone();
        sorted.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap());
        assert_eq!(
            sorted.iter().take(3).map(|x| x.bone).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "按权重降序的前 3 组应是 1,2,3"
        );
        assert_ne!(
            sorted.iter().take(3).map(|x| x.bone).collect::<Vec<_>>(),
            l.iter().take(3).map(|x| x.bone).collect::<Vec<_>>(),
            "本夹具必须让「先排序」与「先截断」结果不同，否则测不出问题"
        );
    }

    /// **`links == 0` 与「残留不足一组」两种边界在优化后仍然成立。**
    ///
    /// 优化改动了 `pending` 的清空逻辑（`end` 分支与文件结束两处），
    /// 这两条边界正是最容易被动坏的地方。
    #[test]
    fn zero_links_and_trailing_partial_group_survive_optimization() {
        // 一个三角形：全部 3 个顶点 links=0（官方允许，boneCount=0）。
        //
        // ⚠️ 顶点行**仍然需要 12 个 token**（`t.len() < 12` 的检查先于
        // `links` 解析）—— 所以 `links=0` 时后面还要凑够占位 token。
        // 这正是 `docs/_probe/smdl/tb1.smd` 的写法。
        let text = MINIMAL.replace(
            "  1 -8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 0.000000 0.000000 1 1 1.000000\n  1 8.000000 -8.000000 0.000000 0.000000 0.000000 1.000000 1.000000 0.000000 1 1 1.000000\n  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000",
            "  1 -8 -8 0 0 0 1 0 0 0 0 0\n  1 8 -8 0 0 0 1 1 0 0 0 0\n  1 0 8 0 0 0 1 0.5 1 0 0 0",
        );
        let s = parse_smd(&text).unwrap();
        assert_eq!(s.triangles.len(), 1);
        for v in &s.triangles[0].vertices {
            assert!(v.links.is_empty(), "links=0 必须解析成空绑定");
        }

        // 残留不足一组：删掉 3 行中的 1 行 ⟹ 必须静默丢弃，不报错。
        let bad = MINIMAL.replace(
            "  1 0.000000 8.000000 0.000000 0.000000 0.000000 1.000000 0.500000 1.000000 1 1 1.000000\n",
            "",
        );
        let s2 = parse_smd(&bad).expect("残留必须被丢弃而不是报错");
        assert_eq!(s2.triangles.len(), 0);

        // 两个完整三角形后跟 1 行残留：前两个必须完好，残留丢弃。
        let mixed = String::from(MINIMAL)
            + "triangles\nm2\n  1 1 1 0 0 0 1 0 0 1 1 1.0\n  1 2 2 0 0 0 1 1 0 1 1 1.0\n  1 3 3 0 0 0 1 0 1 1 1 1.0\n  1 9 9 0 0 0 1 0 0 1 1 1.0\nend\n";
        let s3 = parse_smd(&mixed).unwrap();
        assert_eq!(s3.triangles.len(), 2, "完整的两组保留，残留丢弃");
        assert_eq!(s3.triangles[1].vertices[0].position, [1.0, 1.0, 0.0]);
    }
}
