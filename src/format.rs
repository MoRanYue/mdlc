//! studio 二进制格式的版本与规格表。
//!
//! mdlc 现在能写出两种 studio 版本：
//!
//! | 版本 | 阵营 | 代表 |
//! |---|---|---|
//! | **49** | 默认 | L4D2 / CS:GO / DOI / Insurgency / Blade / MCV / Swarm |
//! | **48** | Source 2013 系 | Garry's Mod / TF2 / CS:S / DoD:S / HL2:DM / Portal / BMS / Contagion |
//!
//! # 为什么需要这张表
//!
//! 早先 `ModelDesc::version()` 是个**纯透传字段** —— 它的值只被写进
//! `.mdl` 头部 `+0x04` 那一个 int（`mdl_writer.rs` 里全文件唯一的使用点），
//! 而所有段的布局**恒按 v49 算**。于是 `version = 48` 会产出一个
//! 「标称 48、结构却是 49」的文件：对不带 jigglebone 的模型恰好能读
//! （v48/v49 的其余结构尺寸相同），**带 jigglebone 的模型则静默错位 20 字节**。
//!
//! 本模块把「版本」变成真正驱动写出的规格，`layout` / `mdl_writer` 都从这里取值。
//!
//! # v48 与 v49 的差异只有四处
//!
//! 判据是逐字段比对两份官方头文件（`docs/_probe` 之外的离线工具
//! `studiohdr_diff.js`，输入 `hl2sdk-sdk2013/public/studio.h`（v48）与
//! `hl2sdk-doi/public/studio.h`（v49））：
//!
//! | 结构 | v48 | v49 | 尺寸 |
//! |---|---|---|---|
//! | `mstudiojigglebone_t` | 多 5 个 `boing*` float | — | **140 vs 120（+20）** |
//! | `mstudiobone_t` | `int unused[8]` | `int surfacepropLookup; int unused[7]` | 216 = 216 |
//! | `mstudioanimdesc_t` | `int unused1[6]` | `int ikrulezeroframeindex; int unused1[5]` | 100 = 100 |
//! | `studiohdr_t` | `int checksum; int unused3[1]` | `long checksum; int surfacepropLookup` | 408 = 408 |
//!
//! 也就是说**只有 jigglebone 会改变字节布局**；其余三处都是
//! 「同一个位置换个字段名」，尺寸不变。v49 新增的结构
//! （`mstudiotwistbone_t` / `mstudiotwistbonetarget_t` /
//! `mstudioikrulezeroframe_t` / `mstudio_frame_anim_t` /
//! `vertexStreamFileHeader_t`）mdlc **本来就不写**（`grep -i twist src/` 零命中）。
//!
//! ⚠️ `surfacepropLookup` 在 v48 里是 `unused3` / `unused[0]`，
//! **必须写 0** —— 写文件偏移进去会让 v48 的读取端拿到垃圾值
//! （虽然官方读取端不看它，但「写垃圾进未定义字段」不是可接受的默认行为）。
//!
//! # CS:GO 不需要新版本
//!
//! `hl2sdk-csgo/public/studio.h` 与 `hl2sdk-doi/public/studio.h`
//! **逐字节相同**（都是 116692 B，SHA256 `2ECB05236A4C9403`，
//! `STUDIO_VERSION 49`）—— CS:GO 用的就是 mdlc 现在写的这套结构。
//!
//! # 不支持 v44
//!
//! v44（`darkm` / `episode1`）与 v48 的差异**远大于** v48 与 v49 的差异
//! （新增 13 个结构，`seqdesc` / `animdesc` / `eyeball` / `flex` 全改），
//! 且官方只有 v44 的 `utils/studiomdl` 源码可参考。mdlc **不写** v44：
//! 校验层会直接报 [`crate::model::DescErrorKind::UnsupportedVersion`]。

/// 一个 studio 版本。
///
/// 默认 [`Self::V49`]（L4D2 / CS:GO 那一系），与 mdlc 的历史行为一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StudioVersion {
    /// **48** —— Source 2013 系（Garry's Mod / TF2 / CS:S / DoD:S / HL2:DM /
    /// Portal / BMS / Contagion …）。
    ///
    /// ⚠️ mdlc 只写 `.dx90.vtx`，**不写 `.dx80.vtx`** —— 现代玩家都跑 DX9+，
    /// 引擎按 `mat_dxlevel` 选文件，DX8 才会去读 dx80。代价是 GMod 自带的
    /// `-mdlreport` / HLMV 这类工具会抱怨缺文件（工具侧，不是引擎侧）。
    V48,
    /// **49** —— L4D2 / CS:GO / DOI / Insurgency / Blade / MCV / Swarm。
    #[default]
    V49,
}

impl StudioVersion {
    /// 全部受支持的版本，按版本号升序。
    pub const ALL: [Self; 2] = [Self::V48, Self::V49];

    /// 从头部里的版本号解析。不认识的版本返回 `None`。
    pub fn from_i32(v: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_i32() == v)
    }

    /// 写进 `.mdl` 头部 `+0x04` 的版本号。
    pub fn as_i32(self) -> i32 {
        self.spec().version
    }

    /// 该版本的规格。
    pub fn spec(self) -> &'static FormatSpec {
        match self {
            Self::V48 => &V48,
            Self::V49 => &V49,
        }
    }
}

/// 一个版本的全部格式参数。
///
/// 加新版本时**只加一个 `static`** —— `layout` 与 `mdl_writer` 都从这里取值，
/// 不写 `match version { … }`。
#[derive(Debug)]
pub struct FormatSpec {
    /// 写进 `.mdl` 头部 `+0x04` 的版本号。
    pub version: i32,
    /// `mstudiojigglebone_t` 的字节大小（v48 **140**、v49 **120**）。
    pub jiggle_bone_size: usize,
    /// `studiohdr_t` / `mstudiobone_t` 里有没有 `surfacepropLookup` 字段。
    ///
    /// v49 有（写字符串池偏移），v48 那里是 `unused`（**必须写 0**）。
    pub writes_surfaceprop: bool,
}

/// v49 的规格（L4D2 / CS:GO 一系）。
static V49: FormatSpec = FormatSpec {
    version: 49,
    jiggle_bone_size: 120,
    writes_surfaceprop: true,
};

/// v48 的规格（Source 2013 一系）。
static V48: FormatSpec = FormatSpec {
    version: 48,
    jiggle_bone_size: 140,
    writes_surfaceprop: false,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_numbers_round_trip() {
        for s in StudioVersion::ALL {
            assert_eq!(StudioVersion::from_i32(s.as_i32()), Some(s));
        }
        assert_eq!(StudioVersion::from_i32(44), None);
        assert_eq!(StudioVersion::from_i32(7), None);
        assert_eq!(StudioVersion::from_i32(0), None);
    }

    #[test]
    fn default_is_v49() {
        assert_eq!(StudioVersion::default(), StudioVersion::V49);
        assert_eq!(StudioVersion::default().as_i32(), 49);
    }

    /// 判据：v48 的 jigglebone 恰好比 v49 大 20 字节
    /// （5 个 `boing*` float），其余规格一致。
    #[test]
    fn v48_jigglebone_is_20_bytes_larger() {
        let (a, b) = (StudioVersion::V48.spec(), StudioVersion::V49.spec());
        assert_eq!(b.jiggle_bone_size, 120);
        assert_eq!(a.jiggle_bone_size, 140);
        assert_eq!(a.jiggle_bone_size - b.jiggle_bone_size, 5 * 4);
    }

    #[test]
    fn only_v49_writes_surfaceprop() {
        assert!(StudioVersion::V49.spec().writes_surfaceprop);
        assert!(!StudioVersion::V48.spec().writes_surfaceprop);
    }
}
