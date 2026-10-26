//! 目标引擎预设（`--target` / `[model] target` / `$target`）。
//!
//! **预设不是约束**：它只给「这个引擎用什么 studio 版本、最多几根骨骼」一组
//! **默认值**，显式参数（CLI > QC/TOML）永远压过它。缺省 `generic` 不应用
//! 任何值，每个参数沿用自身默认。
//!
//! # 为什么需要它
//!
//! 骨骼上限**不能由版本号推出来**：
//!
//! | 引擎 | studio 版本 | 骨骼上限 | 依据 |
//! |---|---|---|---|
//! | L4D2 | 49 | **127** | 反汇编 `FUN_00434620`：`if (0x7f < numbones)` 报错 |
//! | Garry's Mod | 48 | **255** | 反汇编 `FUN_00435630`：`if (0xff < numbones)` 报错 |
//! | Source SDK 2013 / TF2 / CS:S | 48 | 127（推定） | 源码里 `MAXSTUDIOBONES == 128`，未逐二进制核对 |
//! | CS:GO | 49 | 255（推定） | 本机没有该编译器，只有 SDK 头文件 |
//!
//! 也就是说 **v49 里 L4D2 是 127 而 CS:GO 是 255、v48 里 SDK2013 是 127 而
//! GMod 是 255** —— 同一个版本号对应两个不同的上限。所以「版本」与「骨骼上限」
//! 是两个独立参数，`target` 只是把常见的组合打包起来。
//!
//! # 官方那两个上限长什么样
//!
//! L4D2（`studiomdl.exe`，骨骼表建好之后的总检查）：
//!
//! ```c
//! if (0x7f < iVar5) {
//!   FUN_0044d940("Too many bones used in model, used %d, max %d\n", iVar5, 0x80);
//! }
//! ```
//!
//! 消息里印的 `max 128` 是官方自己的笔误 —— 判据是 `> 127`，**写 128 根就被拒**。
//!
//! GMod 同一个函数里常量是 `0x80 → 0x100`（`0xff` 判据），这是 Facepunch 改的，
//! 与「DMX 格式」无关：**纯 SMD 输入同样能到 255**。

use crate::format::StudioVersion;

/// 骨骼表条数的**默认**上限。
///
/// 取 127 而不是 128：官方判据是 `numbones > 0x7f`，128 根会被真编译器拒绝。
pub const DEFAULT_MAX_BONES: u32 = 127;

/// 骨骼表条数的**格式**上限。
///
/// 三个写骨骼下标的地方都是 1 字节 —— 顶点 `mstudioboneweight_t.bone[]`
/// （`hl2sdk-doi/public/studio.h:1236`）、VTX `Vertex_t.boneID[]`
/// （`public/optimize.h:48`）、动画 `mstudioanim_t.bone`（`studio.h:684`）
/// —— 所以下标只能到 255，**超过 255 的骨骼无法被引用**，256 条是有效上限。
///
/// `studiohdr_t.numbones` 本身是 `int`（理论上 2³¹−1），但多出来的骨骼既
/// 不能被顶点引用、也不能被动画引用，写进去只是把文件撑大。
pub const MAX_BONES_FORMAT: u32 = 256;

/// 一个引擎预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// 规范化名字（全小写；`--target` 的输入大小写不敏感）。
    pub name: &'static str,
    /// 别名（同样大小写不敏感）。
    pub aliases: &'static [&'static str],
    /// 该引擎读的 studio 版本；`None` = 不指定，沿用内置默认（49）。
    pub version: Option<StudioVersion>,
    /// 该引擎允许的骨骼表条数；`None` = 不指定，沿用 [`DEFAULT_MAX_BONES`]。
    pub max_bones: Option<u32>,
}

/// 不应用任何预设 —— 每个参数各用各的默认值。
pub static GENERIC: Target = Target {
    name: "generic",
    aliases: &[],
    version: None,
    max_bones: None,
};

/// Left 4 Dead 2。
pub static L4D2: Target = Target {
    name: "l4d2",
    aliases: &["left4dead2", "left-4-dead-2"],
    version: Some(StudioVersion::V49),
    max_bones: Some(127),
};

/// Garry's Mod（Source 2013 分支，但骨骼上限被改到 255）。
pub static GMOD: Target = Target {
    name: "gmod",
    aliases: &["garrysmod", "garry", "garrys-mod"],
    version: Some(StudioVersion::V48),
    max_bones: Some(255),
};

/// Source SDK Base 2013（= TF2 / CS:S / DoD:S 那一批的基础）。
pub static SDK2013: Target = Target {
    name: "sdk2013",
    aliases: &["source2013", "sdk-2013", "source-sdk-2013"],
    version: Some(StudioVersion::V48),
    max_bones: Some(127),
};

/// Team Fortress 2。
pub static TF2: Target = Target {
    name: "tf2",
    aliases: &["teamfortress2", "team-fortress-2"],
    version: Some(StudioVersion::V48),
    max_bones: Some(127),
};

/// Counter-Strike: Source。
pub static CSS: Target = Target {
    name: "css",
    aliases: &["cstrike", "counter-strike-source"],
    version: Some(StudioVersion::V48),
    max_bones: Some(127),
};

/// CS:GO（上限是推定的：本机没有它的编译器）。
pub static CSGO: Target = Target {
    name: "csgo",
    aliases: &["cs2", "counter-strike-global-offensive"],
    version: Some(StudioVersion::V49),
    max_bones: Some(255),
};

/// 全部预设（顺序 = 帮助与错误提示里的列举顺序）。
pub const ALL: [&Target; 7] = [&GENERIC, &L4D2, &GMOD, &SDK2013, &TF2, &CSS, &CSGO];

impl Target {
    /// 按名字（含别名）查预设，大小写不敏感。
    pub fn from_name(name: &str) -> Option<&'static Target> {
        let n = name.trim().to_ascii_lowercase();
        ALL.iter()
            .copied()
            .find(|t| t.name == n || t.aliases.contains(&n.as_str()))
    }

    /// 可用预设名，逗号分隔 —— 报错时列给用户看。
    pub fn available() -> String {
        ALL.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_insensitive_and_honours_aliases() {
        assert_eq!(Target::from_name("l4d2").map(|t| t.name), Some("l4d2"));
        assert_eq!(Target::from_name("  L4D2 ").map(|t| t.name), Some("l4d2"));
        assert_eq!(
            Target::from_name("Left4Dead2").map(|t| t.name),
            Some("l4d2")
        );
        assert_eq!(
            Target::from_name("GarrysMod").map(|t| t.name),
            Some("gmod")
        );
        assert!(Target::from_name("nope").is_none());
        assert!(Target::from_name("").is_none());
    }

    #[test]
    fn generic_applies_nothing() {
        assert!(GENERIC.version.is_none());
        assert!(GENERIC.max_bones.is_none());
    }

    /// 判据：同一版本号下确实存在两个不同的骨骼上限 —— 这正是
    /// 「不能由 version 推导 max_bones」的直接证据。
    #[test]
    fn same_version_has_different_bone_limits() {
        assert_eq!(L4D2.version, CSGO.version);
        assert_ne!(L4D2.max_bones, CSGO.max_bones);
        assert_eq!(GMOD.version, SDK2013.version);
        assert_ne!(GMOD.max_bones, SDK2013.max_bones);
    }

    /// 反汇编得到的两个常量：L4D2 `> 0x7f` 拒、GMod `> 0xff` 拒。
    #[test]
    fn decompiled_limits() {
        assert_eq!(L4D2.max_bones, Some(0x7f));
        assert_eq!(GMOD.max_bones, Some(0xff));
        assert_eq!(DEFAULT_MAX_BONES, L4D2.max_bones.unwrap());
    }

    #[test]
    fn format_ceiling_is_one_byte() {
        assert_eq!(MAX_BONES_FORMAT, 256);
        assert_eq!(MAX_BONES_FORMAT - 1, u8::MAX as u32);
        for t in ALL {
            if let Some(n) = t.max_bones {
                assert!(
                    n <= MAX_BONES_FORMAT,
                    "预设 {} 的上限 {n} 超过格式上限",
                    t.name
                );
            }
        }
    }

    #[test]
    fn available_lists_every_preset() {
        let s = Target::available();
        for t in ALL {
            assert!(s.contains(t.name), "{s} 里缺少 {}", t.name);
        }
    }
}
