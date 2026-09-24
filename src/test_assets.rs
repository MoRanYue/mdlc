//! 真实素材测试的路径解析（**仅 `#[cfg(test)]`**，不进产物）。
//!
//! # 为什么需要这个模块
//!
//! 有 6 个测试拿**真实素材**做判据 —— 官方 `studiomdl.exe` 产物、真实
//! SMD、以及 3333 个 `.mdl` 的语料库。这些素材**不能进仓库**
//! （体量大、含 Valve 版权内容），所以它们只在开发机上存在。
//!
//! 原先这些路径是**硬编码的绝对路径**（`D:\GITHUB\plank\...`、
//! `D:\DSH\...`、`E:\SteamLibrary\...`）。这有两个问题：
//!
//! 1. **别人克隆后无法跑**，而且更糟 —— 测试会用「文件不存在就 `return`」
//!    的方式**静默跳过**，在 `cargo test` 的默认输出里**显示成 `ok`**，
//!    看起来像「验过了」，实际什么也没验。本项目已被这类空洞测试坑过两次
//!    （见 `AGENTS.md`「一个恒真的测试比没有测试更危险」）。
//! 2. 路径写死意味着换个素材就要改源码。
//!
//! # 现在的约定
//!
//! - 这 6 个测试**全部标 `#[ignore]`** ⟹ 默认 `cargo test` **不跑**，
//!   结果行里显示 `6 ignored` 而不是混在 `passed` 里（**跳过是显式的**）。
//! - 路径优先读**环境变量**，读不到再退回本机历史路径，
//!   两者都没有就 panic 并提示该设哪个变量。
//! - 手动跑：`cargo test --release -- --ignored`
//!
//! | 环境变量 | 素材 | 本机默认路径 | 怎么获得 |
//! |---|---|---|---|
//! | `MDLC_TEST_VVD` | 官方 `v_autoshotgun.vvd`（388,765 顶点 / 24.8 MB） | `D:\GITHUB\plank\examples\v_autoshotgun.vvd` | 从 L4D2 的 `pak01_dir.vpk` 解出（`docs/_probe/vpk_extract.js`） |
//! | `MDLC_TEST_MDL` | 官方 `v_autoshotgun.mdl`（89 骨骼） | `D:\GITHUB\plank\examples\v_autoshotgun.mdl` | 同上 |
//! | `MDLC_TEST_SMD` | 真实反编译 SMD（22,911 三角形） | `D:\GITHUB\plank\target\decompile-sample\body2_model0.smd` | 用 plank 反编译官方模型 |
//! | `MDLC_TEST_VTX` | 官方 `myprop.dx90.vtx`（**该夹具须由 studiomdl 编译 `parity/myprop.qc` 得到**） | `E:\SteamLibrary\...\mymod\myprop.dx90.vtx` | `verify_parity.ps1` 或 `studiomdl.exe -game <gamedir> parity/myprop.qc` |
//! | `MDLC_TEST_CORPUS` | 真实语料根目录（3333 个 `.mdl` / 3302 个 `.vvd`） | `D:\DSH\L4D2ReverseEngineering\mdl-corpus` | `docs/_probe/extract_corpus.ps1` |
//!
//! > ⚠️ **注意**：语料里也有一个叫 `v_autoshotgun` 的模型
//! > （`models\v_models\`），但那是**另一个资产** —— 3,632 顶点 / 63 骨骼，
//! > 而这里要的是 388,765 顶点 / 89 骨骼的那个。**两者不可互换**
//! > （测试里硬编码了后者的实测值，这正是「防止实现与测试一起跑偏」的手段）。
//! > 同名不同物是这套素材最容易踩的坑。

use std::path::PathBuf;

/// 取真实素材路径：**环境变量优先**，其次本机历史默认路径。
///
/// 两者都不存在时返回 `None`，并打印**可操作**的提示（该设哪个变量、
/// 从哪来），而不是一句「跳过」。
pub fn asset(env_var: &str, local_default: &str) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(env_var) {
        let p = PathBuf::from(v);
        if p.exists() {
            return Some(p);
        }
        panic!(
            "{env_var} 指向的路径不存在：{}\n\
             （环境变量已设置但路径无效 —— 这是配置错误，不是「素材缺失」，故直接失败）",
            p.display()
        );
    }
    let p = PathBuf::from(local_default);
    if p.exists() {
        return Some(p);
    }
    None
}

/// 取真实素材路径，取不到就 panic（带可操作提示）。
///
/// 用于「标了 `#[ignore]`、被显式要求跑」的测试 —— 既然用户明确要跑，
/// 素材缺失就是**失败**，不是跳过。这正是 `#[ignore]` 相比「静默 return」
/// 的价值：**跳过与失败不再混淆**。
pub fn require(env_var: &str, local_default: &str, what: &str) -> PathBuf {
    asset(env_var, local_default).unwrap_or_else(|| {
        panic!(
            "找不到{what}。\n\
             请设置环境变量 {env_var} 指向它，例如：\n\
             \x20   PowerShell:  $env:{env_var} = 'D:\\path\\to\\asset'\n\
             \x20   bash:        export {env_var}=/path/to/asset\n\
             本机默认路径（当前不存在）：{local_default}"
        )
    })
}
