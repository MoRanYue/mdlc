//! `mdlc` —— Source 引擎模型编译器（studiomdl 重写的 MVP 探针）。
//!
//! # 当前阶段的目标
//!
//! **用 TOML 描述文件编译出一个能被引擎加载的模型。** 输入刻意不是 QC
//! （见 [`model`] 模块文档的理由），输出是 `.mdl` + `.vvd` 三元组。
//!
//! # 两条判据
//!
//! 1. **布局判据（已达成）**：官方 L4D2 模型的 VVD 读入再写出必须
//!    **逐字节相同**。这比「能解析」强得多 —— 它能一次性抓住字段顺序、
//!    结构大小、偏移基准、对齐填充的任何错误。见 [`vvd_round_trip`]。
//! 2. **编译判据（进行中）**：TOML → MDL/VVD，产物要能被
//!    `hlmv.exe` 加载且不报错。
//!
//! # 为什么从 VVD 开始（而不是 MDL 或 VTX）
//!
//! - VVD 是三者中最机械的：定长头 + 两个等长的定长数组，没有 strip、
//!   没有 LOD 合并、没有变长编码；
//! - 它的头部字段之间存在**可算术验证的等式**（见 [`vvd`] 模块文档），
//!   错了立刻能发现；
//! - 它承载了模型最核心的数据（顶点位置/法线/UV/骨骼权重），
//!   打通它就等于打通了写方向的主动脉。
//!
//! # 模块一览
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`model`] | TOML 描述文件的数据结构与校验 |
//! | [`smd`] | SMD 网格/骨架解析 |
//! | [`compile`] | TOML + SMD → 编译期 IR |
//! | [`mdl_writer`] | `.mdl` 写出 |
//! | [`vvd`] | `.vvd` 解析 / 写出 / 往返比对 |
//! | [`vtx_writer`] | `.dx90.vtx` 写出 |
//! | [`anim_writer`] | 动画数据链写出 |
//! | [`bone_math`] | 骨骼矩阵与四元数工具 |
//! | [`layout`] | 布局偏移常量 |
//! | [`phy`] | **`.phy` 碰撞文件写出**（凸包 → IVP 紧凑表面） |
//! | [`tangent`] | **切线空间计算**（法线贴图用，对应 studiomdl 的 `CalcModelTangentSpaces`） |
//! | [`lod`] | **多 LOD 与 fixup 表**（顶点池排序、分段、重映射） |
//! | [`vta`] | **`.vta` 顶点动画解析**（SMD 语法、7 字段行、相对帧号） |
//! | [`flex`] | **VTA 形状解析**（就近匹配 → 差量 → smoothstep → 载荷） |
//! | [`qc`] | **QC 前端**（`.qc` 脚本 → [`model::ModelDesc`]，即 Phase 2 的输入适配） |

pub mod anim_writer;
pub mod ani_writer;
pub mod bone_math;
pub mod compile;
pub mod flex;
pub mod layout;
pub mod lod;
pub mod mdl_writer;
pub mod model;
pub mod phy;
pub mod prof;
pub mod qc;
pub mod smd;
pub mod tangent;
pub mod vta;
pub mod vtx_writer;
pub mod vvd;

/// 往返比对的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundTrip {
    /// 原文件长度。
    pub original_len: usize,
    /// 重新序列化后的长度。
    pub rewritten_len: usize,
    /// 首个不同字节的位置（完全相同则为 `None`）。
    pub first_diff: Option<usize>,
    /// 不同的字节总数。
    pub diff_count: usize,
}

impl RoundTrip {
    /// 是否逐字节完全相同。
    pub fn is_identical(&self) -> bool {
        self.first_diff.is_none()
    }
}

/// 把 VVD 字节读入再写出，并逐字节比对。
///
/// 这是本阶段的核心判据。注意它**不是**「解析成功」——解析成功只说明
/// 头部字段读得对，往返相同才说明**整个文件布局**（含两个数据块的
/// 位置与内容）都写对了。
pub fn vvd_round_trip(buf: &[u8]) -> Result<RoundTrip, vvd::VvdError> {
    let parsed = vvd::Vvd::parse(buf)?;
    vvd::check_invariants(&parsed, buf.len())?;
    let rewritten = parsed.to_bytes()?;

    let mut first_diff = None;
    let mut diff_count = 0usize;
    for (i, (a, b)) in buf.iter().zip(rewritten.iter()).enumerate() {
        if a != b {
            diff_count += 1;
            if first_diff.is_none() {
                first_diff = Some(i);
            }
        }
    }
    // 长度不同时，多出来的字节也算差异。
    let common = buf.len().min(rewritten.len());
    diff_count += buf.len().abs_diff(rewritten.len());

    Ok(RoundTrip {
        original_len: buf.len(),
        rewritten_len: rewritten.len(),
        first_diff: if buf.len() == rewritten.len() {
            first_diff
        } else {
            first_diff.or(Some(common))
        },
        diff_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vvd::{
        HEADER_SIZE, MAX_NUM_LODS, TANGENT_SIZE, VERTEX_SIZE, Vvd, VvdHeader, VvdTangent, VvdVertex,
        check_invariants,
    };

    /// 官方 L4D2 模型：`v_autoshotgun.vvd`，实测的头部字段。
    /// 这些常量是**独立于实现**记录下来的实测值，用来防止实现与测试一起跑偏。
    const REAL_VVD: &str = r"D:\GITHUB\plank\examples\v_autoshotgun.vvd";
    const REAL_MDL_CHECKSUM: i32 = -1709441603;
    const REAL_VERTEX_COUNT: usize = 388765;
    const REAL_FILE_LEN: usize = 24_881_024;
    const REAL_VERTEX_DATA_START: i32 = 64;
    const REAL_TANGENT_DATA_START: i32 = 18_660_784;

    fn real_vvd() -> Option<Vec<u8>> {
        std::fs::read(REAL_VVD).ok()
    }

    /// 构造一个最小可用的合成 VVD（不依赖真实素材，CI 也能跑）。
    fn synth(checksum: i32, count: usize) -> Vvd {
        let vertices = (0..count)
            .map(|i| VvdVertex {
                weight: [1.0, 0.0, 0.0],
                bone: [(i % 7) as u8, 0, 0],
                bone_count: 1,
                position: [i as f32, 1.5, -2.25],
                normal: [0.0, 0.0, 1.0],
                tex_coord: [0.125, 0.875],
            })
            .collect();
        let tangents = (0..count)
            .map(|_| VvdTangent {
                xyz: [1.0, 0.0, 0.0],
                w: 1.0,
            })
            .collect();
        Vvd {
            header: VvdHeader {
                checksum,
                num_lods: 1,
                num_lod_vertexes: [count as i32; MAX_NUM_LODS],
                num_fixups: 0,
                fixup_table_start: HEADER_SIZE as i32,
                vertex_data_start: HEADER_SIZE as i32,
                tangent_data_start: (HEADER_SIZE + count * VERTEX_SIZE) as i32,
            },
            vertices,
            tangents,
            fixups: Vec::new(),
        }
    }

    #[test]
    fn synthetic_round_trip_is_identical() {
        let v = synth(-1709441603, 37);
        let bytes = v.to_bytes().unwrap();
        assert_eq!(
            bytes.len(),
            HEADER_SIZE + 37 * VERTEX_SIZE + 37 * TANGENT_SIZE
        );
        let rt = vvd_round_trip(&bytes).unwrap();
        assert!(rt.is_identical(), "合成数据往返应完全相同：{rt:?}");
        assert_eq!(rt.diff_count, 0);
    }

    /// 旧实现**显式拒绝** `numFixups != 0`。现在支持了，所以那条拒绝已删除。
    ///
    /// 但「`numFixups` 声明与实际表长不符」仍然必须报错 —— 否则会写出
    /// 一个头部与数据对不上的文件（引擎按头部读，读到 fixup 表里当顶点）。
    #[test]
    fn mismatched_fixup_count_is_rejected() {
        let mut v = synth(1, 4);
        v.header.num_fixups = 2; // 声明 2 条，但 `fixups` 是空的
        let err = v.to_bytes().unwrap_err();
        assert!(
            matches!(err, crate::vvd::VvdError::Inconsistent { .. }),
            "numFixups 与实际表长不符时必须报错，实际：{err:?}"
        );
    }

    /// `numLODVertexes[0]` 与实际顶点数不符必须报错 —— 否则写出的文件
    /// 长度与头部声明对不上，引擎会读到别的块里去。
    #[test]
    fn mismatched_vertex_count_is_rejected() {
        let mut v = synth(1, 4);
        v.header.num_lod_vertexes[0] = 9;
        let err = v.to_bytes().unwrap_err();
        assert!(
            matches!(err, crate::vvd::VvdError::Inconsistent { .. }),
            "numLODVertexes[0] 与顶点数不符时必须报错，实际：{err:?}"
        );
    }

    /// 构造一份**带 fixup 表**的合成 VVD，验证偏移按 ALIGN16 递推且往返一致。
    ///
    /// 这是新增能力：旧实现遇到 `numFixups != 0` 直接报错。
    /// 偏移公式来自 `write.cpp` 2799-2815，已在 53 个真实模型上验证
    /// （`docs/_probe/probe_vvd_fixup_semantics2.js`）。
    #[test]
    fn synthetic_fixup_round_trip_is_identical() {
        let mut v = synth(7, 6);
        // 3 条 fixup：12*3 = 36 字节，不是 16 的倍数 → 顶点块必须对齐到 112。
        v.fixups = vec![
            crate::lod::Fixup { lod: 1, source_vertex_id: 0, num_vertexes: 2 },
            crate::lod::Fixup { lod: 0, source_vertex_id: 2, num_vertexes: 2 },
            crate::lod::Fixup { lod: 0, source_vertex_id: 4, num_vertexes: 2 },
        ];
        v.header.num_lods = 2;
        v.header.num_lod_vertexes = [6, 2, 2, 2, 2, 2, 2, 2];
        v.recompute_offsets();

        assert_eq!(v.header.fixup_table_start, 64, "fixupTableStart = ALIGN4(64)");
        assert_eq!(
            v.header.vertex_data_start, 112,
            "vertexDataStart = ALIGN16(64 + 3*12) = ALIGN16(100) = 112"
        );
        assert_eq!(
            v.header.tangent_data_start,
            (112 + 6 * VERTEX_SIZE) as i32,
            "tangentDataStart = ALIGN16(vertexDataStart + n0*48)"
        );

        let bytes = v.to_bytes().unwrap();
        assert_eq!(bytes.len(), v.declared_len());
        check_invariants(&v, bytes.len()).expect("带 fixup 的合成 VVD 必须自洽");

        let rt = vvd_round_trip(&bytes).unwrap();
        assert!(rt.is_identical(), "带 fixup 的往返必须逐字节相同：{rt:?}");
        // 解析回来必须还原出同样的 fixup 表。
        let parsed = Vvd::parse(&bytes).unwrap();
        assert_eq!(parsed.fixups, v.fixups, "fixup 表必须能读回");
        assert_eq!(parsed.header.num_lods, 2);
        assert_eq!(parsed.header.num_lod_vertexes, v.header.num_lod_vertexes);
    }

    /// fixup 表必须**精确铺满** `[0, numLODVertexes[0])` —— 有空洞或重叠
    /// 都说明布局理解错了（实测 53/53 个真实模型满足）。
    #[test]
    fn fixup_table_must_tile_the_pool() {
        let mut v = synth(7, 6);
        // 只覆盖 [0,4)，漏掉 [4,6) → 应报错。
        v.fixups = vec![
            crate::lod::Fixup { lod: 1, source_vertex_id: 0, num_vertexes: 2 },
            crate::lod::Fixup { lod: 0, source_vertex_id: 2, num_vertexes: 2 },
        ];
        v.header.num_lods = 2;
        v.header.num_lod_vertexes = [6, 2, 2, 2, 2, 2, 2, 2];
        v.recompute_offsets();
        let bytes = v.to_bytes().unwrap();
        let err = check_invariants(&v, bytes.len()).unwrap_err();
        assert!(
            matches!(err, crate::vvd::VvdError::Inconsistent { .. }),
            "有空洞时必须报错，实际：{err:?}"
        );
    }

    /// `numLODVertexes` 必须单调不增 —— 它是**累计值**（渲染该 LOD 所需
    /// 的顶点数），随 LOD 变粗而减少。递增说明语义理解反了。
    #[test]
    fn non_monotone_lod_counts_are_rejected() {
        let mut v = synth(7, 6);
        v.header.num_lods = 3;
        // [0] 必须是顶点总数（6），但 [2] > [1] 破坏单调性。
        v.header.num_lod_vertexes = [6, 2, 4, 4, 4, 4, 4, 4];
        v.recompute_offsets();
        let bytes = v.to_bytes().unwrap();
        let err = check_invariants(&v, bytes.len()).unwrap_err();
        assert!(
            matches!(err, crate::vvd::VvdError::Inconsistent { .. }),
            "单调性被破坏时必须报错，实际：{err:?}"
        );
        // 报错信息应指向单调性，而不是别的检查。
        let crate::vvd::VvdError::Inconsistent { detail } = err else {
            unreachable!()
        };
        assert!(detail.contains("单调不增"), "应报单调性：{detail}");
    }

    /// `numLODVertexes` 的尾部槽位必须 ripple 成最后一个有效值 ——
    /// 留 0 会让引擎的 LOD 切换读到「该 LOD 有 0 个顶点」。
    #[test]
    fn non_rippled_tail_is_rejected() {
        let mut v = synth(7, 6);
        v.header.num_lods = 2;
        v.header.num_lod_vertexes = [6, 2, 0, 0, 0, 0, 0, 0]; // 尾部应为 2
        v.recompute_offsets();
        let bytes = v.to_bytes().unwrap();
        let err = check_invariants(&v, bytes.len()).unwrap_err();
        let crate::vvd::VvdError::Inconsistent { detail } = err else {
            panic!("应报 Inconsistent")
        };
        assert!(detail.contains("ripple"), "应报 ripple：{detail}");
    }

    #[test]
    fn parse_rejects_bad_magic_and_version() {
        let mut bytes = synth(7, 2).to_bytes().unwrap();
        bytes[0] = b'X';
        assert!(matches!(
            Vvd::parse(&bytes),
            Err(crate::vvd::VvdError::BadId { .. })
        ));

        let mut bytes = synth(7, 2).to_bytes().unwrap();
        bytes[4..8].copy_from_slice(&99i32.to_le_bytes());
        assert!(matches!(
            Vvd::parse(&bytes),
            Err(crate::vvd::VvdError::BadVersion { found: 99 })
        ));
    }

    #[test]
    fn parse_rejects_truncated_vertex_block() {
        let bytes = synth(7, 10).to_bytes().unwrap();
        let cut = &bytes[..bytes.len() - VERTEX_SIZE];
        let err = Vvd::parse(cut).unwrap_err();
        assert!(
            matches!(err, crate::vvd::VvdError::Truncated { .. }),
            "截断必须报 Truncated，实际：{err:?}"
        );
    }

    #[test]
    fn invariants_reject_wrong_tangent_start() {
        let mut v = synth(7, 5);
        let bytes = v.to_bytes().unwrap();
        v.header.tangent_data_start += 1;
        let err = check_invariants(&v, bytes.len()).unwrap_err();
        assert!(matches!(err, crate::vvd::VvdError::Inconsistent { .. }));
    }

    /// **核心判据**：官方 L4D2 模型的往返必须逐字节相同。
    ///
    /// 素材不存在时跳过（而不是伪造通过）—— 缺失会打印出来，
    /// 不会静默变成绿色。
    #[test]
    fn real_l4d2_vvd_round_trips_byte_for_byte() {
        let Some(buf) = real_vvd() else {
            eprintln!("跳过：找不到真实素材 {REAL_VVD}");
            return;
        };

        // 先用独立记录的实测常量锚定解析结果。
        let parsed = Vvd::parse(&buf).expect("官方 VVD 必须能解析");
        assert_eq!(parsed.header.checksum, REAL_MDL_CHECKSUM);
        assert_eq!(parsed.header.vertex_count(), REAL_VERTEX_COUNT);
        assert_eq!(parsed.header.num_lods, 1);
        assert_eq!(parsed.header.num_fixups, 0);
        assert_eq!(parsed.header.vertex_data_start, REAL_VERTEX_DATA_START);
        assert_eq!(parsed.header.tangent_data_start, REAL_TANGENT_DATA_START);
        assert_eq!(buf.len(), REAL_FILE_LEN);

        // 再验证布局等式（错一个字段这里就会炸）。
        check_invariants(&parsed, buf.len()).expect("头部字段之间必须自洽");

        // 最后是真正的判据。
        let rt = vvd_round_trip(&buf).expect("往返必须成功");
        assert!(
            rt.is_identical(),
            "官方 VVD 往返必须逐字节相同，实际首个差异 @{}，共 {} 字节不同",
            rt.first_diff.unwrap_or(0),
            rt.diff_count
        );
        assert_eq!(rt.rewritten_len, REAL_FILE_LEN);
    }

    /// **多 LOD + fixup 的核心判据**：真实语料里全部带 fixup 的 VVD
    /// 必须逐字节往返。
    ///
    /// 语料在 `D:\DSH\L4D2ReverseEngineering\mdl-corpus\`（3333 个真实模型，
    /// 其中 53 个带 fixup、230 个多 LOD）。素材不存在时**跳过并打印**，
    /// 不伪造通过。
    ///
    /// 实测结果（本机）：**53/53 逐字节相同**，另有 230 个多 LOD 模型
    /// （含 177 个 `numFixups == 0` 的单 mesh 形态）也全部往返一致。
    #[test]
    fn real_fixup_vvds_round_trip_byte_for_byte() {
        const CORPUS: &str = r"D:\DSH\L4D2ReverseEngineering\mdl-corpus";
        if !std::path::Path::new(CORPUS).is_dir() {
            eprintln!("跳过：找不到语料 {CORPUS}");
            return;
        }
        // 递归收集 .vvd。
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("vvd"))
                {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(CORPUS), &mut files);
        if files.is_empty() {
            eprintln!("跳过：{CORPUS} 里没有 .vvd");
            return;
        }

        let mut total = 0usize;
        let mut with_fixup = 0usize;
        let mut multi_lod = 0usize;
        let mut failures: Vec<String> = Vec::new();
        for f in &files {
            let Ok(buf) = std::fs::read(f) else { continue };
            // 解析失败的文件不属本测试范围（另有语料统计脚本负责）。
            let Ok(parsed) = Vvd::parse(&buf) else {
                continue;
            };
            total += 1;
            if parsed.header.num_fixups > 0 {
                with_fixup += 1;
            }
            if parsed.header.num_lods > 1 {
                multi_lod += 1;
            }
            // 自检 + 往返。
            if let Err(e) = check_invariants(&parsed, buf.len()) {
                failures.push(format!("{}: 自检失败 {e}", f.display()));
                continue;
            }
            match vvd_round_trip(&buf) {
                Ok(rt) if rt.is_identical() => {}
                Ok(rt) => failures.push(format!(
                    "{}: 往返有 {} 字节不同（首个 @{}）",
                    f.display(),
                    rt.diff_count,
                    rt.first_diff.unwrap_or(0)
                )),
                Err(e) => failures.push(format!("{}: 往返失败 {e}", f.display())),
            }
        }

        eprintln!(
            "语料实测：{total} 个 VVD（多 LOD {multi_lod}，带 fixup {with_fixup}），\
             失败 {}",
            failures.len()
        );
        assert!(
            failures.is_empty(),
            "{} 个文件往返失败：\n{}",
            failures.len(),
            failures
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
        // 语料确实覆盖了 fixup 与多 LOD —— 否则这个测试等于没测。
        assert!(
            with_fixup > 0,
            "语料里应至少有 1 个带 fixup 的模型（否则该测试没覆盖到新能力）"
        );
        assert!(multi_lod > 0, "语料里应至少有多 LOD 模型");
    }
}
