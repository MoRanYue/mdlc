//! 命令行解析（`clap` builder API）。
//!
//! # 为什么要兼容官方 `studiomdl` 的**单横线**选项
//!
//! 官方的调用形态是
//!
//! ```text
//! studiomdl.exe -game "<gamedir>" [-nop4] [-verbose] "<model.qc>"
//! ```
//!
//! 即 **单横线长选项**。clap 只认 `--long` / `-s`，`-game` 会被拆成
//! `-g -a -m -e`（实测 `UnknownArgument :: unexpected argument '-g' found`）。
//! 所以官方形态必须**先归一化再交给 clap**，见 [`normalize_official_args`]。
//!
//! # 为什么需要这个别名（Crowbar 工作流）
//!
//! Crowbar（`Crowbar\Core\Compiler\Compiler.vb:556-579`）把编译器路径当
//! **不透明配置项**，调用契约只有：
//!
//! ```text
//! <CompilerPathFileName> -game "<gamedir>" <CompileOptionsText> "<qc 文件名>"
//! ```
//!
//! 且把 **CWD 设为 QC 所在目录**。它的成败判定只有两条：
//!
//! 1. `theProcessHasOutputData` —— 编译器 stdout/stderr 有**任意一行**输出
//!    （否则报 "compiler is not the correct one for the selected game"）；
//! 2. `File.Exists(<gamedir>\models\<$modelname>.mdl)` —— 产物落在
//!    官方路径上。
//!
//! **它完全不看退出码，也不解析错误文本**（全源码搜 `ERROR` 抓取，命中的
//! 全是 Crowbar 自己的消息）。所以只要参数能被接受、产物落在正确路径，
//! 把 Crowbar 的编译器路径指向 `mdlc.exe` 就能直接替换官方编译器。

use clap::{Arg, ArgAction, ArgMatches, Command};
use std::path::PathBuf;

/// 官方 `studiomdl` 的**无值** flag（L4D2 `studiomdl.exe` 实测 usage）。
///
/// 归一化成 `--<name>` 后交给 clap；不在表里的未知项由
/// [`normalize_official_args`] 丢弃并记入警告。
const OFFICIAL_FLAGS_NO_VALUE: &[&str] = &[
    // ---- 已实现 / 无副作用，容忍 ----
    "nop4",
    "verbose",
    "quiet",
    "nowarnings",
    // ---- 官方语义，mdlc 尚未实现（警告后继续）----
    "definebones",
    "printbones",
    "printgraph",
    "fullcollide",
    "checklengths",
    "perf",
    "ihvtest",
    "allowdebug",
    "verify",
    "makefile",
    "xbox",
    "notxbox",
    "x360",
    "nox360",
    "striplods",
    "dumpmaterials",
    "mdlreport",
    "mdlreportspreadsheet",
    "stripmodel",
    "stripvhv",
    "vsi",
    "overridedefinebones",
    "fastbuild",
    "preview",
    "basedir",
    "tempcontent",
    "dontremoveduplicates",
    "maxwarnings",
    // ---- 单字符 flag ----
    "h", // 官方 = dump hboxes；⚠️ 与 mdlc 自己的 -h=help 冲突，见下
    "i",
    "f",
    "r",
    "n",
    "d",
];

/// 官方 `studiomdl` 的**吃一个值**的 flag。
const OFFICIAL_FLAGS_WITH_VALUE: &[&str] = &["game", "minlod", "t", "a"];

/// 归一化结果。
#[derive(Debug, PartialEq, Eq)]
pub struct Normalized {
    /// 交给 clap 的 argv（含 argv[0]）。
    pub argv: Vec<String>,
    /// 被丢弃的未知选项，用于警告。
    pub unknown: Vec<String>,
}

/// 把官方 `studiomdl` 的**单横线长选项**归一化成 clap 认识的 `--long`。
///
/// 只动 `-x` 形态（单横线且第二个字符不是 `-`）；`--x` 与裸词原样保留，
/// 所以 mdlc 自己的 `--out` / `build-qc` 等不受影响。
///
/// 未知选项**丢弃并记录**（用户要求：一律警告后继续）—— 但**吃值**的
/// 未知选项会把它的值一起吃掉，否则那个值会变成多余的位置参数而报错。
/// 已知的吃值 flag（`-minlod 2`）必须整对丢弃，这是这里最容易错的地方。
pub fn normalize_official_args(argv: &[String]) -> Normalized {
    let mut out: Vec<String> = Vec::with_capacity(argv.len());
    let mut unknown: Vec<String> = Vec::new();
    if let Some(first) = argv.first() {
        out.push(first.clone());
    }
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        // 只处理 `-x`（单横线）；`--x` 与裸词原样放行。
        let Some(name) = a.strip_prefix('-').filter(|s| !s.is_empty() && !s.starts_with('-'))
        else {
            out.push(a.clone());
            i += 1;
            continue;
        };
        // `-key=value` 形态
        let (key, inline_value) = match name.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (name, None),
        };
        let key_lc = key.to_ascii_lowercase();

        // ⚠️ 用**小写规范名**喂 clap，而不是用户原始大小写 ——
        // 官方全部用 `stricmp` 比较（`studiomdl.cpp:6916` 起），
        // 所以 `-GAME` / `-NoP4` 都合法；若原样透传，clap 会因
        // `--GAME` 未注册而报 UnknownArgument。
        if OFFICIAL_FLAGS_WITH_VALUE.contains(&key_lc.as_str()) {
            out.push(format!("--{key_lc}"));
            match inline_value {
                Some(v) => out.push(v),
                None => {
                    if let Some(v) = argv.get(i + 1) {
                        out.push(v.clone());
                        i += 1;
                    }
                }
            }
        } else if OFFICIAL_FLAGS_NO_VALUE.contains(&key_lc.as_str()) {
            out.push(format!("--{key_lc}"));
        } else {
            // 未知：丢弃。若它看起来吃值（下一个不是选项），一并丢弃。
            unknown.push(a.clone());
            if inline_value.is_none()
                && let Some(next) = argv.get(i + 1)
                && !next.starts_with('-')
            {
                // ⚠️ 只有当下一个参数**不是**唯一的 `.qc` 位置参数时才吞掉。
                // 否则 `-bogus x.qc` 会把 QC 也吃掉，导致「缺少 QC」的
                // 误导性报错。判据：后面还有没有别的裸参数。
                let has_later_positional = argv[i + 2..].iter().any(|s| !s.starts_with('-'));
                if has_later_positional {
                    unknown.push(next.clone());
                    i += 1;
                }
            }
        }
        i += 1;
    }
    Normalized { argv: out, unknown }
}

/// 构建完整 CLI 定义。
///
/// 顶层同时接受两种形态：
/// - **mdlc 自有**：`mdlc <子命令> ...`（裸词）；
/// - **官方兼容**：`mdlc -game <gamedir> [-nop4] <model.qc>`（首参以 `-` 开头）。
///
/// 分流在 [`crate::main`] 里做（首参是否以 `-` 开头），因为两种形态的
/// 位置参数语义不同：官方形态的裸参数是 `.qc`，mdlc 形态的是子命令。
pub fn build_cli() -> Command {
    Command::new("mdlc")
        .about("Source 引擎模型编译器（studiomdl 重写）")
        .disable_version_flag(true)
        .subcommand_required(false)
        .arg_required_else_help(false)
        .subcommand(build_cmd())
        .subcommand(check_cmd())
        .subcommand(phy_cmd())
        .subcommand(qc2toml_cmd())
        .subcommand(build_qc_cmd())
        .subcommand(vvd_info_cmd())
        .subcommand(vvd_roundtrip_cmd())
        .subcommand(template_cmd())
}

/// 官方兼容模式的命令行（首参为 `-` 时走这里）。
///
/// 它**没有子命令**，位置参数就是 `.qc`。
///
/// flag 表由 [`OFFICIAL_FLAGS_NO_VALUE`] / [`OFFICIAL_FLAGS_WITH_VALUE`]
/// 直接生成 —— 这样「归一化认识的」与「clap 接受的」永远是同一份清单，
/// 不会出现归一化放行、clap 却报 `UnknownArgument` 的裂缝。
pub fn build_official_cli() -> Command {
    let mut cmd = Command::new("mdlc")
        .about("Source 引擎模型编译器（studiomdl 兼容模式）")
        .disable_version_flag(true)
        .arg_required_else_help(false)
        .arg(
            Arg::new("qc")
                .value_name("file.qc")
                .num_args(1)
                .help("要编译的 QC 脚本"),
        )
        .arg(
            Arg::new("optimize_vtx")
                .long("optimize-vtx")
                .action(ArgAction::SetTrue)
                .help("mdlc 扩展：打开 VTX 缓存优化"),
        )
        .arg(
            Arg::new("out")
                .long("out")
                .value_name("目录")
                .help("mdlc 扩展：显式输出根目录（优先于 -game 推出的路径）"),
        );

    // ⚠️ `-h` 在官方是「dump hboxes」，在 mdlc 是 help。归一化后是 `--h`
    // （**不是** `--help`），所以两者不冲突：mdlc 的 help 仍走 `--help`。
    for name in OFFICIAL_FLAGS_NO_VALUE {
        cmd = cmd.arg(flag(name));
    }
    // `-game` / `-minlod` 吃值；`-t` / `-a` 也吃值，但语义未实现。
    cmd = cmd.arg(
        Arg::new("game")
            .long("game")
            .value_name("gamedir")
            .help("游戏目录（官方 -game）；产物写到 <gamedir>\\models\\"),
    );
    for name in OFFICIAL_FLAGS_WITH_VALUE {
        if *name == "game" {
            continue;
        }
        cmd = cmd.arg(Arg::new(name).long(name).value_name("值"));
    }
    cmd
}

fn flag(name: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue)
}

fn build_cmd() -> Command {
    Command::new("build")
        .about("TOML 描述 → .mdl/.vvd/.vtx（MVP 主线）")
        .arg(Arg::new("toml").required(true).value_name("model.toml"))
        .arg(
            Arg::new("out")
                .long("out")
                .value_name("目录")
                .help("输出根目录（默认当前目录）"),
        )
        .arg(
            Arg::new("optimize_vtx")
                .long("optimize-vtx")
                .action(ArgAction::SetTrue),
        )
}

fn check_cmd() -> Command {
    Command::new("check")
        .about("只校验描述文件，不写文件")
        .arg(Arg::new("toml").required(true).value_name("model.toml"))
}

fn qc2toml_cmd() -> Command {
    Command::new("qc2toml")
        .about("QC 脚本 → TOML 描述（只写文本，不编译）")
        .arg(Arg::new("qc").required(true).value_name("model.qc"))
        .arg(Arg::new("out").long("out").value_name("path.toml"))
}

fn build_qc_cmd() -> Command {
    Command::new("build-qc")
        .about("直接从 QC 编译")
        .arg(Arg::new("qc").required(true).value_name("model.qc"))
        .arg(Arg::new("out").long("out").value_name("目录"))
        .arg(
            Arg::new("optimize_vtx")
                .long("optimize-vtx")
                .action(ArgAction::SetTrue),
        )
}

fn vvd_info_cmd() -> Command {
    Command::new("vvd-info")
        .about("解析并打印 VVD 头部与统计")
        .arg(Arg::new("file").required(true).value_name("file.vvd"))
}

fn vvd_roundtrip_cmd() -> Command {
    Command::new("vvd-roundtrip")
        .about("VVD 读入再写出，逐字节比对")
        .arg(Arg::new("file").required(true).value_name("file.vvd"))
}

fn template_cmd() -> Command {
    Command::new("template").about("打印一份带注释的最小 TOML 模板")
}

/// `phy` 子命令（参数多，单独一个函数）。
pub fn phy_cmd() -> Command {
    Command::new("phy")
        .about("SMD 三角形 → 凸包 → .phy 碰撞文件")
        .arg(Arg::new("input").required(true).value_name("in.smd"))
        .arg(Arg::new("output").required(true).value_name("out.phy"))
        .arg(
            Arg::new("checksum")
                .long("checksum")
                .value_name("N")
                .help("配对 .mdl 的 checksum（十进制或 0x 十六进制），默认 0"),
        )
        .arg(
            Arg::new("mass")
                .long("mass")
                .value_name("F")
                .help("$mass 等效总质量，默认 1"),
        )
        .arg(
            Arg::new("surfaceprop")
                .long("surfaceprop")
                .value_name("S")
                .default_value("default"),
        )
        .arg(
            Arg::new("concave")
                .long("concave")
                .action(ArgAction::SetTrue)
                .help("$concave：按连通分量拆成多个凸块"),
        )
        .arg(
            Arg::new("vhacd")
                .long("vhacd")
                .action(ArgAction::SetTrue)
                .help("VHACD 近似凸分解（非官方语义）"),
        )
        .arg(
            Arg::new("decompose")
                .long("decompose")
                .action(ArgAction::SetTrue)
                .help("--vhacd 的旧别名"),
        )
        .arg(
            Arg::new("ragdoll")
                .long("ragdoll")
                .action(ArgAction::SetTrue)
                .help("$collisionjoints：按蒙皮权重骨骼分组"),
        )
}

/// 从 `build` 的匹配结果取参数。
pub fn build_args(m: &ArgMatches) -> (PathBuf, PathBuf, bool) {
    (
        PathBuf::from(m.get_one::<String>("toml").expect("required")),
        m.get_one::<String>("out")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        m.get_flag("optimize_vtx"),
    )
}

/// 从 `build-qc` 的匹配结果取参数。
pub fn build_qc_args(m: &ArgMatches) -> (PathBuf, PathBuf, bool) {
    (
        PathBuf::from(m.get_one::<String>("qc").expect("required")),
        m.get_one::<String>("out")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        m.get_flag("optimize_vtx"),
    )
}

/// 官方兼容模式的输出根目录。
///
/// 官方规则（`write.cpp:1321-1331`、`optimize.cpp:3923`、`collisionmodel.cpp:2307`）：
///
/// ```text
/// <gamedir> + "models/" + $modelname
/// ```
///
/// 所以 `-game <gamedir>` ⟹ `out_root = <gamedir>\models`，
/// 再叠加 `$modelname` 里的相对路径（由 `compile_and_write` 拼）。
///
/// `--out` 显式给了就优先用它（mdlc 扩展，方便测试与脚本）。
pub fn official_out_root(m: &ArgMatches) -> PathBuf {
    if let Some(o) = m.get_one::<String>("out") {
        return PathBuf::from(o);
    }
    match m.get_one::<String>("game") {
        Some(g) => PathBuf::from(g).join("models"),
        None => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(v: &[&str]) -> Normalized {
        let argv: Vec<String> = v.iter().map(|s| s.to_string()).collect();
        normalize_official_args(&argv)
    }

    #[test]
    fn single_dash_long_option_becomes_double_dash() {
        // 这是整个兼容层的存在理由：clap 会把 -game 拆成 -g -a -m -e。
        let n = norm(&["mdlc", "-game", "C:/g", "x.qc"]);
        assert_eq!(n.argv, vec!["mdlc", "--game", "C:/g", "x.qc"]);
        assert!(n.unknown.is_empty());
    }

    #[test]
    fn crowbar_default_invocation_parses() {
        // Crowbar 原样传参（CompileOptionsText 默认空）。
        let n = norm(&["mdlc", "-game", "C:/g", "t10000.qc"]);
        let m = build_official_cli()
            .try_get_matches_from(&n.argv)
            .expect("Crowbar 默认形态必须能解析");
        assert_eq!(m.get_one::<String>("game").map(String::as_str), Some("C:/g"));
        assert_eq!(m.get_one::<String>("qc").map(String::as_str), Some("t10000.qc"));
    }

    #[test]
    fn crowbar_with_all_three_ui_flags_parses() {
        // Crowbar UI 只能追加这三个 flag（CompileUserControl.vb:725-727）。
        let n = norm(&["mdlc", "-game", "C:/g", "-nop4", "-verbose", "x.qc"]);
        assert!(n.unknown.is_empty(), "这三个是已知 flag，不该进 unknown");
        let m = build_official_cli()
            .try_get_matches_from(&n.argv)
            .expect("必须能解析");
        assert!(m.get_flag("nop4"));
        assert!(m.get_flag("verbose"));
    }

    #[test]
    fn unknown_flag_is_dropped_and_reported() {
        let n = norm(&["mdlc", "-game", "C:/g", "-bogus", "x.qc"]);
        assert_eq!(n.unknown, vec!["-bogus"]);
        // 关键：x.qc 不能被吞掉
        let m = build_official_cli()
            .try_get_matches_from(&n.argv)
            .expect("未知 flag 不应导致解析失败");
        assert_eq!(m.get_one::<String>("qc").map(String::as_str), Some("x.qc"));
    }

    #[test]
    fn unknown_flag_with_value_drops_both() {
        // -bogusval 2 x.qc：`2` 是它的值，必须一起丢弃，否则变成多余位置参数。
        let n = norm(&["mdlc", "-bogusval", "2", "x.qc"]);
        assert_eq!(n.unknown, vec!["-bogusval", "2"]);
        let m = build_official_cli()
            .try_get_matches_from(&n.argv)
            .expect("必须能解析");
        assert_eq!(m.get_one::<String>("qc").map(String::as_str), Some("x.qc"));
    }

    #[test]
    fn known_value_flag_minlod_is_accepted_not_dropped() {
        // -minlod 吃值且已知 ⟹ 归一化后由 clap 接住（语义层面再警告）。
        let n = norm(&["mdlc", "-game", "C:/g", "-minlod", "2", "x.qc"]);
        assert!(n.unknown.is_empty(), "-minlod 是已知吃值 flag");
        assert_eq!(n.argv, vec!["mdlc", "--game", "C:/g", "--minlod", "2", "x.qc"]);
    }

    #[test]
    fn double_dash_and_subcommands_are_untouched() {
        // mdlc 自己的形态必须原样通过（parity/benchmark 全靠它）。
        let n = norm(&["mdlc", "build", "x.toml", "--out", "dir"]);
        assert_eq!(n.argv, vec!["mdlc", "build", "x.toml", "--out", "dir"]);
        assert!(n.unknown.is_empty());
    }

    #[test]
    fn equals_form_is_supported() {
        let n = norm(&["mdlc", "-game=C:/g", "x.qc"]);
        assert_eq!(n.argv, vec!["mdlc", "--game", "C:/g", "x.qc"]);
    }

    #[test]
    fn official_out_root_appends_models() {
        // 官方规则：<gamedir> + "models/"（write.cpp:1321-1331）。
        let n = norm(&["mdlc", "-game", "C:/g", "x.qc"]);
        let m = build_official_cli().try_get_matches_from(&n.argv).unwrap();
        assert_eq!(official_out_root(&m), PathBuf::from("C:/g").join("models"));
    }

    #[test]
    fn explicit_out_overrides_game_derived_root() {
        let n = norm(&["mdlc", "-game", "C:/g", "--out", "D:/o", "x.qc"]);
        let m = build_official_cli().try_get_matches_from(&n.argv).unwrap();
        assert_eq!(official_out_root(&m), PathBuf::from("D:/o"));
    }

    #[test]
    fn official_flags_are_case_insensitive() {
        // 官方用 `stricmp`（studiomdl.cpp:6916 起），`-GAME` / `-NoP4` 合法。
        let n = norm(&["mdlc", "-GAME", "C:/g", "-NoP4", "x.qc"]);
        assert!(n.unknown.is_empty(), "-GAME/-NoP4 必须被识别");
        assert_eq!(n.argv, vec!["mdlc", "--game", "C:/g", "--nop4", "x.qc"]);
        let m = build_official_cli()
            .try_get_matches_from(&n.argv)
            .expect("大小写变体必须能解析");
        assert_eq!(m.get_one::<String>("game").map(String::as_str), Some("C:/g"));
        assert!(m.get_flag("nop4"));
    }

    #[test]
    fn bare_qc_parses_without_game() {
        // 官方 `studiomdl <file.qc>`（选项全默认）也必须能走兼容模式。
        let m = build_official_cli()
            .try_get_matches_from(["mdlc", "x.qc"])
            .expect("裸 .qc 必须能解析");
        assert_eq!(m.get_one::<String>("qc").map(String::as_str), Some("x.qc"));
        assert!(m.get_one::<String>("game").is_none());
        // 没有 -game 也没有 --out ⟹ 输出根目录是当前目录
        assert_eq!(official_out_root(&m), PathBuf::from("."));
    }

    #[test]
    fn mdlc_native_commands_still_parse() {
        // 回归：clap 迁移不能弄坏既有子命令。
        let cli = build_cli();
        let m = cli
            .clone()
            .try_get_matches_from(["mdlc", "build", "x.toml", "--out", "d"])
            .expect("build 必须可解析");
        let (t, o, v) = build_args(m.subcommand().unwrap().1);
        assert_eq!(t, PathBuf::from("x.toml"));
        assert_eq!(o, PathBuf::from("d"));
        assert!(!v);

        let m = cli
            .clone()
            .try_get_matches_from(["mdlc", "build-qc", "x.qc"])
            .expect("build-qc 必须可解析");
        let (q, o, _) = build_qc_args(m.subcommand().unwrap().1);
        assert_eq!(q, PathBuf::from("x.qc"));
        assert_eq!(o, PathBuf::from("."), "默认输出根目录必须是当前目录");
    }

    #[test]
    fn missing_required_arg_is_usage_error() {
        // 退出码契约：用法错误 = 2（clap 默认行为，与旧手写解析一致）。
        let e = build_cli()
            .try_get_matches_from(["mdlc", "build"])
            .unwrap_err();
        assert_eq!(e.exit_code(), 2);
    }

    #[test]
    fn no_subcommand_yields_none_not_error() {
        // 无参数时 clap 不应报错（`subcommand_required(false)`），
        // 由 main 决定「打印 usage + 退出 2」—— 这是迁移前的契约。
        let m = build_cli()
            .try_get_matches_from(["mdlc"])
            .expect("无参数不应是 clap 错误");
        assert!(m.subcommand().is_none());
    }

    #[test]
    fn help_flag_is_display_help_not_error() {
        // `-h` / `--help` 走 stdout、退出 0（与旧手写解析一致）。
        let e = build_cli()
            .try_get_matches_from(["mdlc", "--help"])
            .unwrap_err();
        assert_eq!(e.exit_code(), 0, "--help 必须是 DisplayHelp（退出 0）");
        assert!(!e.use_stderr(), "--help 必须走 stdout");
        let e2 = build_cli().try_get_matches_from(["mdlc", "-h"]).unwrap_err();
        assert_eq!(e2.exit_code(), 0, "-h 必须仍是 mdlc 的 help");
    }
}
