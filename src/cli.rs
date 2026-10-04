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
//!
//! # mdlc 自有形态：`clap` derive + i18n
//!
//! 自有子命令走 `clap` 的 derive（[`Cli`] + [`Commands`]），帮助文本由
//! doc comment 生成 —— 不再有手写的 `USAGE` 常量，也不再有
//! 「注册命令 + `match` 分支按键取值」。官方兼容形态**仍是 builder**，
//! 理由见 [`Cli`] 的文档。
//!
//! 多语言由 `clap-i18n-richformatter` 提供：段落标题（用法/参数/选项/命令）、
//! 帮助模板与**错误排版**按系统显示语言自动切换。⚠️ **命令与参数自身的
//! 描述**是 mdlc 写死的中文，不在该 crate 的覆盖范围内。

use clap::{Arg, ArgAction, ArgMatches, Args, Command, FromArgMatches, Parser, Subcommand};
use clap_i18n_richformatter::{ClapI18nRichFormatter, clap_i18n};
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
    /// 交给 clap 的 argv（含 `argv[0]`）。
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

// ---------------------------------------------------------------------------
// mdlc 自有形态：derive
// ---------------------------------------------------------------------------

/// Source 引擎模型编译器（studiomdl 重写）
///
/// 把 TOML 描述或 QC 脚本编译成 Source 引擎的 `.mdl` / `.vvd` / `.vtx` /
/// `.phy` 四件套，另带若干只读的检查与转换子命令。
///
/// 也能当官方 `studiomdl` 用：`mdlc -game <gamedir> <model.qc>` 会把产物写到
/// `<gamedir>\models\<$modelname>`，可直接替换 Crowbar 的编译器路径。
// 实现注记（不是给用户的帮助文本，所以放在 `//` 里而不是 doc comment）：
//
// - 子命令与参数的帮助正文由这些类型上的 doc comment 自动生成 —— 没有手写
//   的 usage 常量，也没有「注册命令 + `match` 分支按键取值」。
// - 官方兼容形态（`mdlc -game <gamedir> <x.qc>`）由 `main` 在解析前分流，
//   不走这条路径；那份定义是 builder 版的 [`build_official_cli`]。
// - 段落标题（用法 / 参数 / 选项 / 命令）与错误排版由
//   `clap-i18n-richformatter` 按**系统显示语言**翻译；命令与参数**自身的
//   描述**是下面的 doc comment，始终是中文。
#[derive(Debug, Parser)]
#[clap_i18n]
#[command(name = "mdlc", version, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// mdlc 自有子命令。
///
/// ⚠️ 每个变体都显式写了 `#[command(name = ...)]`，不依赖 clap 从变体名
/// 推断的 kebab-case —— 这些名字是脚本与探针的调用契约
/// （`build` / `build-qc` / `vvd-info` / `vvd-roundtrip` / `qc2toml`）。
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// TOML 描述 → .mdl/.vvd/.vtx（MVP 主线）
    #[command(name = "build")]
    Build(BuildArgs),
    /// 只校验描述文件，不写文件
    #[command(name = "check")]
    Check(CheckArgs),
    /// SMD 三角形 → 凸包 → .phy 碰撞文件
    #[command(name = "phy")]
    Phy(PhyArgs),
    /// QC 脚本 → TOML 描述（只写文本，不编译）
    #[command(name = "qc2toml")]
    Qc2toml(Qc2tomlArgs),
    /// 直接从 QC 编译
    #[command(name = "build-qc")]
    BuildQc(BuildQcArgs),
    /// 解析并打印 VVD 头部与统计
    #[command(name = "vvd-info")]
    VvdInfo(VvdInfoArgs),
    /// VVD 读入再写出，逐字节比对
    #[command(name = "vvd-roundtrip")]
    VvdRoundtrip(VvdRoundtripArgs),
    /// 打印一份带注释的最小 TOML 模板
    #[command(name = "template")]
    Template,
    /// （内部）跑一次更新检测并写缓存
    ///
    /// **隐藏**子命令：更新检测的子进程入口（`mdlc __update-check`）。
    /// 它刻意不出现在 `--help` 里（`hide`）—— 这是给 [`crate::update`]
    /// 派生出去的子进程用的，不是给用户的功能。手动跑它只是为了排错：
    /// 它会**同步**执行一次检测，把结果写进缓存。
    ///
    /// 之所以做成「真子命令」而不是内部环境变量开关：`spawn` 出去的是
    /// **同一个可执行文件**，走同一条参数解析路径，所以「手动跑」与
    /// 「后台跑」不可能分叉出两种行为。
    #[command(name = crate::update::HIDDEN_SUBCOMMAND, hide = true)]
    UpdateCheck,
}

/// `mdlc build <model.toml> [--out <目录>] [--optimize-vtx]`
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// 要编译的 TOML 描述文件
    #[arg(value_name = "model.toml")]
    pub toml: PathBuf,
    /// 输出根目录（默认当前目录）
    #[arg(long, value_name = "目录")]
    pub out: Option<PathBuf>,
    /// mdlc 扩展：打开 VTX 缓存优化
    #[arg(long)]
    pub optimize_vtx: bool,
}

impl BuildArgs {
    /// 输出根目录：`--out` 缺省为当前目录。
    pub fn out_root(&self) -> PathBuf {
        self.out.clone().unwrap_or_else(|| PathBuf::from("."))
    }
}

/// `mdlc check <model.toml>`
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// 要校验的 TOML 描述文件
    #[arg(value_name = "model.toml")]
    pub toml: PathBuf,
}

/// `mdlc qc2toml <model.qc> [--out <path.toml>]`
#[derive(Debug, Args)]
pub struct Qc2tomlArgs {
    /// 要转换的 QC 脚本
    #[arg(value_name = "model.qc")]
    pub qc: PathBuf,
    /// 输出的 TOML 路径（缺省与 QC 同目录同名，扩展名换成 .toml）
    #[arg(long, value_name = "path.toml")]
    pub out: Option<PathBuf>,
}

/// `mdlc build-qc <model.qc> [--out <目录>] [--optimize-vtx]`
#[derive(Debug, Args)]
pub struct BuildQcArgs {
    /// 要编译的 QC 脚本
    #[arg(value_name = "model.qc")]
    pub qc: PathBuf,
    /// 输出根目录（默认当前目录）
    #[arg(long, value_name = "目录")]
    pub out: Option<PathBuf>,
    /// mdlc 扩展：打开 VTX 缓存优化
    #[arg(long)]
    pub optimize_vtx: bool,
}

impl BuildQcArgs {
    /// 输出根目录：`--out` 缺省为当前目录。
    pub fn out_root(&self) -> PathBuf {
        self.out.clone().unwrap_or_else(|| PathBuf::from("."))
    }
}

/// `mdlc vvd-info <file.vvd>`
#[derive(Debug, Args)]
pub struct VvdInfoArgs {
    /// 要解析并打印头部与统计的 VVD 文件
    #[arg(value_name = "file.vvd")]
    pub file: PathBuf,
}

/// `mdlc vvd-roundtrip <file.vvd>`
#[derive(Debug, Args)]
pub struct VvdRoundtripArgs {
    /// 要往返比对的 VVD 文件
    #[arg(value_name = "file.vvd")]
    pub file: PathBuf,
}

/// `mdlc phy <in.smd> <out.phy> [选项]`（参数多，单独一个结构体）。
#[derive(Debug, Args)]
pub struct PhyArgs {
    /// 输入 SMD（三角形顶点已在模型局部坐标，不做任何变换，与 studiomdl 一致）
    #[arg(value_name = "in.smd")]
    pub input: PathBuf,
    /// 输出 .phy 路径
    #[arg(value_name = "out.phy")]
    pub output: PathBuf,
    /// 配对 .mdl 的 checksum（十进制或 0x 十六进制），默认 0
    #[arg(long, value_name = "N", default_value_t = 0, value_parser = parse_u32)]
    pub checksum: u32,
    /// $mass 等效总质量，默认 1（官方缺省；见 --automass 说明）
    #[arg(long, value_name = "F", default_value_t = 1.0)]
    pub mass: f32,
    /// 表面材质，默认 default
    #[arg(long = "surfaceprop", value_name = "S", default_value = "default")]
    pub surface_prop: String,
    /// $concave：按**连通分量**拆成多个凸块（官方语义，不是 VHACD 体分解）
    #[arg(long)]
    pub concave: bool,
    /// VHACD 近似凸分解。**非官方语义**：保留凹口，而官方 $concave 是填平
    #[arg(long)]
    pub vhacd: bool,
    /// `--vhacd` 的旧别名（保留以免破坏既有脚本；语义已明确为 VHACD）
    #[arg(long)]
    pub decompose: bool,
    /// $collisionjoints：按**蒙皮权重骨骼**分组，每个骨骼一个 solid
    #[arg(long)]
    pub ragdoll: bool,
}

impl PhyArgs {
    /// 是否走 VHACD 体分解：`--vhacd` 或它的旧别名 `--decompose`。
    pub fn vhacd_enabled(&self) -> bool {
        self.vhacd || self.decompose
    }
}

/// clap 自动生成的 `help` 子命令描述是**硬编码英文常量**
/// （`clap_builder` 的 `command.rs:4842`），而 `clap-i18n-richformatter` 的词条
/// 表里没有对应的 key（只有 `clap-subcommand-context` 这类上下文词），
/// 所以这里直接写中文 —— 与其余所有子命令描述保持一致。
const HELP_SUBCOMMAND_ABOUT: &str = "打印本信息或给定子命令的帮助";

/// 构建 mdlc 自有形态的 CLI 定义（已做 i18n 处理）。
///
/// ⚠️ 会先初始化语言环境（`clap-i18n-richformatter` 按**系统显示语言**探测），
/// 所以 [`Cli::command_i18n`] 与下面补的段落标题才是目标语言的。
///
/// 官方兼容形态是另一份定义，见 [`build_official_cli`]。
pub fn build_cli() -> Command {
    clap_i18n_richformatter::init_clap_rich_formatter_localizer();

    // ⚠️ 顺序要紧：真实子命令必须在 `build()` **之前**本地化。
    // `mut_arg` 是「摘掉再放回」，而 `build()` 之后参数已经被 `_build()` 处理过、
    // 且 `Built` 标记会让 `_build_self` 变成空操作 —— 摘放会打乱那份已建好的状态，
    // 结果是 `mdlc phy --help` 这类正常调用报「需要为 '--mass <F>' 赋值」。
    let mut cmd = localize_subcommands(Cli::command_i18n());

    // clap 的自动 `help` 子命令是在 `_build_self` 里才 push 进 `subcommands` 的
    // （`command.rs:4840-4879`），而 `_build_self` 要等到解析或渲染时才跑 ——
    // 所以想改它必须先显式 `build()` 一次。`build()` 会置上 `Built` 标记，
    // 之后的 `_build_self` 就是空操作，不会重复 push。
    cmd.build();

    // `build()` 顺带用 `_copy_subtree_for_help` 把整棵子树克隆一份挂到 `help`
    // 下面（`command.rs:4844-4858`），`mdlc help help` 渲染的就是那份克隆。
    // 克隆体只被渲染、不被解析，所以在这里改是安全的。
    cmd.mut_subcommand("help", |sc| {
        localize_subcommands(localize_one_subcommand(sc))
    })
}

/// 递归把子命令的帮助本地化。
///
/// [`Cli::command_i18n`] 只作用在**顶层**命令上：
///
/// - 它给顶层参数设了 `help_heading`（遍历的是顶层 `get_positionals()` /
///   `get_opts()`），子命令的参数会落回 clap 硬编码的英文
///   `Arguments` / `Options`；
/// - 它的 `help_template`（里面才有本地化的「用法:」标题）也只在顶层生效，
///   子命令仍用 clap 的默认模板，于是打出英文 `Usage:`；
/// - 子命令列表的 `Commands:` 标题同理。
///
/// 递归是必要的：`Command::build()` 会把整棵子命令树克隆一份挂到自动生成的
/// `help` 子命令下面（`command.rs:4844-4858`），而 `mdlc help help` 渲染的
/// 正是那份克隆。
fn localize_subcommands(cmd: Command) -> Command {
    cmd.mut_subcommands(|sc| localize_subcommands(localize_one_subcommand(sc)))
}

/// 给单个子命令补上本地化的帮助模板、段落标题与子命令列表标题。
///
/// 自动生成的 `help` 子命令也走这里 —— 它的描述是 clap 里的英文常量，
/// 见 [`HELP_SUBCOMMAND_ABOUT`]。
fn localize_one_subcommand(mut sc: Command) -> Command {
    use clap_i18n_richformatter::__private::get_translation;

    // `clap::builder::Str` 没有 `From<String>`，只能从 `&'static str` 之类构造，
    // 所以这里与 `clap-i18n-derive` 自己的做法一致：把翻译串 leak 成 `'static`。
    // 每次进程启动只 leak 几条短串，代价可忽略。
    let arguments: &'static str =
        Box::leak(get_translation("clap-arguments-heading").into_boxed_str());
    let options: &'static str = Box::leak(get_translation("clap-options-heading").into_boxed_str());
    let commands: &'static str =
        Box::leak(get_translation("clap-commands-heading").into_boxed_str());
    let usage_heading: &'static str =
        Box::leak(get_translation("clap-usage-heading").into_boxed_str());

    // clap 自动生成的 `help` 子命令：换个本地化描述。
    if sc.get_name() == "help" {
        sc = sc.about(HELP_SUBCOMMAND_ABOUT);
    }

    // `help_template` 与 `command_i18n` 给顶层用的那份同构。
    sc = sc
        .help_template(format!(
            "{{before-help}}{{about-with-newline}}\n{usage_heading} {{usage}}\n\n{{all-args}}{{after-help}}"
        ))
        .subcommand_help_heading(commands)
        .subcommand_value_name(commands);

    let positional_ids: Vec<_> = sc.get_positionals().map(|a| a.get_id().clone()).collect();
    for id in positional_ids {
        sc = sc.mut_arg(id, |a| a.help_heading(arguments));
    }
    let option_ids: Vec<_> = sc.get_opts().map(|a| a.get_id().clone()).collect();
    for id in option_ids {
        sc = sc.mut_arg(id, |a| a.help_heading(options));
    }
    sc
}

/// 解析 mdlc 自有形态的命令行。
///
/// 与 `Cli::parse_i18n()` 的区别有两点，都是为了嵌进 `main` 的现有骨架：
///
/// 1. 接受**显式 argv** —— `main` 要先按首参分流官方兼容形态，不能直接读
///    `std::env::args_os()`；
/// 2. 出错时**不退出**，把错误交回调用方 —— 退出码契约由 `main.rs` 的
///    `cli_exit` 统一决定（`--help`/`--version` → stdout + 0，其余 → stderr + 2）。
pub fn parse_i18n(argv: &[String]) -> Result<Cli, clap::error::Error<ClapI18nRichFormatter>> {
    build_cli()
        .try_get_matches_from(argv)
        .and_then(|m| Cli::from_arg_matches(&m))
        .map_err(|e| e.apply::<ClapI18nRichFormatter>())
}

/// 解析 `u32`，接受十进制与 `0x` 前缀的十六进制。
///
/// 直接当 `--checksum` 的 `value_parser` 用（实测工作流里 checksum 常常是
/// 从 `.mdl` 头里以十六进制抄出来的），所以不走 clap 的默认数值解析。
fn parse_u32(s: &str) -> Result<u32, String> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| e.to_string())
    } else if let Some(hex) = t.strip_prefix("-0x") {
        // 允许 `-0x1234`：`.mdl` 的 checksum 是有符号 int32，
        // 十六进制抄出来常常带负号，但 `.phy` 里按 u32 存。
        let v = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
        Ok(v.wrapping_neg())
    } else {
        t.parse::<u32>()
            .or_else(|_| t.parse::<i32>().map(|v| v as u32))
            .map_err(|e| e.to_string())
    }
}

/// 官方兼容模式的命令行（首参为 `-` 时走这里）。
///
/// 它**没有子命令**，位置参数就是 `.qc`。
///
/// flag 表由 `OFFICIAL_FLAGS_NO_VALUE` / `OFFICIAL_FLAGS_WITH_VALUE`
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

/// 官方兼容模式的输出根目录。
///
/// 官方规则（`write.cpp:1321-1331`、`optimize.cpp:3923`、`collisionmodel.cpp:2307`）：
///
/// ```text
/// <gamedir> + "models/" + $modelname
/// ```
///
/// 所以 `-game <gamedir>` ⟹ `out_root = <gamedir>\models`，
/// 再叠加 `$modelname` 里的相对路径（由 [`crate::pipeline::write_files`] 拼）。
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
        // 回归：clap derive 迁移不能弄坏既有子命令。
        let cli = parse_i18n(&[
            "mdlc".into(),
            "build".into(),
            "x.toml".into(),
            "--out".into(),
            "d".into(),
        ])
        .expect("build 必须可解析");
        match cli.command {
            Commands::Build(a) => {
                assert_eq!(a.toml, PathBuf::from("x.toml"));
                assert_eq!(a.out_root(), PathBuf::from("d"));
                assert!(!a.optimize_vtx);
            }
            other => panic!("期望 Build，得到 {other:?}"),
        }

        let cli = parse_i18n(&["mdlc".into(), "build-qc".into(), "x.qc".into()])
            .expect("build-qc 必须可解析");
        match cli.command {
            Commands::BuildQc(a) => {
                assert_eq!(a.qc, PathBuf::from("x.qc"));
                assert_eq!(
                    a.out_root(),
                    PathBuf::from("."),
                    "默认输出根目录必须是当前目录"
                );
            }
            other => panic!("期望 BuildQc，得到 {other:?}"),
        }

        // 无参数子命令（Unit 变体）必须仍可解析 —— 它没有
        // `arg_required_else_help`，不能把 `mdlc template` 误判成用法错误。
        let cli = parse_i18n(&["mdlc".into(), "template".into()]).expect("template 必须可解析");
        assert!(matches!(cli.command, Commands::Template));

        // 隐藏子命令必须仍是真子命令。
        let cli = parse_i18n(&["mdlc".into(), crate::update::HIDDEN_SUBCOMMAND.into()])
            .expect("隐藏子命令必须可解析");
        assert!(matches!(cli.command, Commands::UpdateCheck));
    }

    #[test]
    fn phy_flags_and_values_parse() {
        let cli = parse_i18n(&[
            "mdlc".into(),
            "phy".into(),
            "in.smd".into(),
            "out.phy".into(),
            "--checksum".into(),
            "0x1a2b".into(),
            "--mass".into(),
            "2.5".into(),
            "--surfaceprop".into(),
            "metal".into(),
            "--decompose".into(),
            "--ragdoll".into(),
        ])
        .expect("phy 必须可解析");
        match cli.command {
            Commands::Phy(a) => {
                assert_eq!(a.input, PathBuf::from("in.smd"));
                assert_eq!(a.output, PathBuf::from("out.phy"));
                assert_eq!(a.checksum, 0x1a2b);
                assert_eq!(a.mass, 2.5);
                assert_eq!(a.surface_prop, "metal");
                assert!(!a.concave);
                assert!(!a.vhacd);
                assert!(a.decompose);
                assert!(a.vhacd_enabled(), "--decompose 是 --vhacd 的旧别名");
                assert!(a.ragdoll);
            }
            other => panic!("期望 Phy，得到 {other:?}"),
        }

        // 缺省值：checksum=0、mass=1.0、surfaceprop=default。
        let cli = parse_i18n(&["mdlc".into(), "phy".into(), "a.smd".into(), "b.phy".into()])
            .expect("phy 缺省参数必须可解析");
        match cli.command {
            Commands::Phy(a) => {
                assert_eq!(a.checksum, 0);
                assert_eq!(a.mass, 1.0);
                assert_eq!(a.surface_prop, "default");
                assert!(!a.vhacd_enabled());
            }
            other => panic!("期望 Phy，得到 {other:?}"),
        }
    }

    #[test]
    fn phy_checksum_accepts_decimal_and_signed_hex() {
        // checksum 常常是从 .mdl 头里以十六进制抄出来的（可能带负号）。
        assert_eq!(parse_u32("123").unwrap(), 123);
        assert_eq!(parse_u32("0x1a2b").unwrap(), 0x1a2b);
        assert_eq!(parse_u32("0X1A2B").unwrap(), 0x1a2b);
        assert_eq!(parse_u32("-0x1").unwrap(), u32::MAX);
        assert!(parse_u32("zzz").is_err());

        // 解析失败必须是 clap 的值校验错误（退出码 2），不是 panic。
        let e = parse_i18n(&[
            "mdlc".into(),
            "phy".into(),
            "a.smd".into(),
            "b.phy".into(),
            "--checksum".into(),
            "zzz".into(),
        ])
        .unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.use_stderr());
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
    fn no_subcommand_is_usage_error_not_ok() {
        // 无参数 = 用法错误：help 走 **stderr**、退出码 2。
        //
        // 这是迁移前 `main.rs` 里 `None => diagprint!("{USAGE}"); 2` 那条
        // 分支的等价物 —— derive 用 `#[command(subcommand)]` 非 `Option`
        // 生成的 `arg_required_else_help(true)` 表达的正是这个契约。
        let e = build_cli().try_get_matches_from(["mdlc"]).unwrap_err();
        assert_eq!(e.exit_code(), 2, "无参数必须是用法错误（退出 2）");
        assert!(e.use_stderr(), "无参数的用法提示必须走 stderr");
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
        // 子命令帮助同理（`update_e2e_routing.js` 断言它走 stderr 的那条
        // 说的是**更新提示**，不是 clap 的 help）。
        let e3 = build_cli()
            .try_get_matches_from(["mdlc", "build-qc", "--help"])
            .unwrap_err();
        assert_eq!(e3.exit_code(), 0);
        assert!(!e3.use_stderr());
    }

    #[test]
    fn version_flag_is_display_version_not_error() {
        let e = build_cli()
            .try_get_matches_from(["mdlc", "--version"])
            .unwrap_err();
        assert_eq!(
            e.exit_code(),
            0,
            "--version 必须是 DisplayVersion（退出 0）"
        );
        assert!(!e.use_stderr(), "--version 必须走 stdout");
    }

    #[test]
    fn version_propagates_to_subcommands() {
        // `propagate_version = true` 不只是为了好看：i18n 宏往顶层塞了
        // `.global(true)` 的 `--version` 参数，global 参数会被推进每个
        // 子命令 —— 而 clap 的 debug 断言要求「有 `ArgAction::Version`
        // 参数的命令自己必须有 version」。少了这个设置，debug 构建
        // （`cargo test`）会在解析子命令时 panic。
        let e = build_cli()
            .try_get_matches_from(["mdlc", "build", "--version"])
            .unwrap_err();
        assert_eq!(e.exit_code(), 0);
        assert!(!e.use_stderr());
    }

    #[test]
    fn unknown_subcommand_is_usage_error() {
        // 迁移前 main.rs 兜底分支打印「未知子命令」+ usage、退出 2；
        // derive 下由 clap 的 `InvalidSubcommand` 表达，流与退出码一致。
        let e = build_cli()
            .try_get_matches_from(["mdlc", "bogus"])
            .unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.use_stderr());
    }
}
