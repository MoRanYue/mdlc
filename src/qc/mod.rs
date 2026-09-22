//! **QC 前端** —— 把 `studiomdl` 的 `.qc` 脚本解析成 [`crate::model::ModelDesc`]。
//!
//! # 为什么现在才做
//!
//! `model.rs` 的模块文档从第一天起就写着：
//!
//! > QC 适配推迟到 Phase 2，届时只需把 QC 解析成同一个 [`ModelDesc`]，
//! > **写出器一行都不用改**。
//!
//! 本模块就是那个 Phase 2。它**只做输入侧** —— 产出 `ModelDesc` 之后，
//! 走的是与手写 TOML **完全相同**的 `compile()` → 写出器链路。
//! 所以「QC 能不能编」这件事被彻底隔离成「QC 能不能正确变成 IR」。
//!
//! # 权威依据（**不是** VDC 文档）
//!
//! | 层 | 依据 |
//! |---|---|
//! | 词法 | `hl2sdk-l4d2\utils\common\scriplib.cpp`（L4D2 版，比 episode1 多 `$definevariable`） |
//! | 命令分发表 | `studiomdl.exe` 的 137 条（`tmp-qcscan\dispatch_table.tsv`） |
//! | 各命令语义 | `hl2sdk-episode1\utils\studiomdl\studiomdl.cpp` + 本项目 `docs\qc-coverage-gap.md` |
//!
//! # 词法层的六条**反直觉**行为（全部读源码确认，不是推测）
//!
//! 这六条都曾让人写出「看起来对、实际吃数据」的解析器，逐条列出：
//!
//! 1. **`\r` 只是空白，不是换行。** `GetToken` 的 `skipspace` 只对 `'\n'`
//!    递增行号；`TokenAvailable()` 也是「跳过一切 `<= 32` 的字节，
//!    遇到 `'\n'` 才返回 false」。所以 `... 58.8\r$definebone ...`
//!    里 `\r` **不结束行** —— `$definebone` 会被当成 `$bbox` 的后续参数吃掉。
//!    （这正是 `qc2toml_miku.js` 记录的「118 条 definebone 只转出 117 条」。）
//! 2. **`;` 既结束 token 也起注释。** 常规 token 的扫描条件是
//!    `> 32 && != ';'`；而注释起始符是 `;` / `#` / `//` 三者。
//!    注意 `#` 和 `/` **不**结束 token，只有 `;` 会。
//! 3. **`$include` 相对 QC 所在目录，与 `$pushd`/`$cd` 无关。**
//!    `AddScriptToStack` 走 `ExpandPath()` = `qdir + token`，而 `qdir` 是
//!    **主 QC 文件的目录**（`filesystem_tools.cpp:105`）。
//!    `$pushd` 只影响 `cddir[]`，而 `cddir` 仅用于**网格/动画**文件名
//!    （`Load_Source`，`studiomdl.cpp:1522`）。实测 `anim_fix.qc`：
//!    `$pushd anims` 之后 `$include includes/anims_fix.qci` 解析到
//!    `<qc 目录>/includes/anims_fix.qci`（**不是** `anims/includes/...`）。
//! 4. **`$definevariable` 的名字匹配只比 `len-2` 个字符。**
//!    `ExpandVariableToken` 用的是 `Q_strnicmp(param, tp, len - 2)`，
//!    其中 `len` 是 `$...$` 之间名字的长度。所以 `$Bone$` 实际只比
//!    `"Bo"`。这是上游的 bug，但它**决定**了实际行为，所以照抄。
//! 5. **`\\`（两个反斜杠）不是通用续行符。** 它只被
//!    `Option_Flexrule`（`studiomdl.cpp:3936`）识别，用于 flexrule 表达式。
//!    实测语料：`survivors_facerules.qci` 的 `\\` 是真续行，
//!    而 `anims_fix.qci` 里的 59 处 `\\` **全在注释里**（画表格的装饰）。
//!    把它做成通用预处理会**破坏注释**。
//! 6. **`$include` 找不到文件时官方静默跳过。** `LoadFile` 失败返回
//!    `buffer = NULL, size = 0`，于是 `script_p >= end_p` 立刻成立，
//!    `EndOfScript` 弹栈继续。mdlc **故意不照抄这条** —— 改为报错，
//!    因为静默跳过正是本项目反复踩到的「静默吃数据」类缺陷。
//!
//! # 与 `$pushd` 的关系（mdlc 怎么表达）
//!
//! 官方用 `cddir[numdirs]` 给**网格文件名**加前缀。mdlc 的
//! [`crate::model::BodyModel::smd`] 是「相对描述文件所在目录」的路径，
//! 所以解析器在产出 IR 时把 `cddir` 前缀**拼进路径字符串**
//! （见 [`parse`] 的 `resolve_src`）。这样写出器不需要知道 `$pushd` 存在。

pub mod flexrule;
pub mod lexer;
pub mod parse;

use std::path::{Path, PathBuf};

/// QC 解析错误。
///
/// 带**文件名与行号** —— 官方 `TokenError` 会打印 `GetTokenizerStatus`
/// 给的文件/行，这里保持一致，否则用户面对一个 41 KB 的 `.qci` 无从下手。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QcError {
    /// 出错位置所在文件（`$include` 进来的文件会显示它自己的名字）。
    pub file: String,
    /// 行号（从 1 起）。
    pub line: usize,
    /// 说明。
    pub message: String,
}

impl std::fmt::Display for QcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.message)
    }
}

impl std::error::Error for QcError {}

impl QcError {
    /// 构造一条错误。
    pub fn new(file: impl Into<String>, line: usize, message: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line,
            message: message.into(),
        }
    }
}

/// 解析一份 `.qc`，得到 [`crate::model::ModelDesc`]。
///
/// `path` 是主 QC 文件；它的**父目录**就是官方的 `qdir`
/// （`$include` 与网格相对路径都以它为基准）。
///
/// 返回的 `ModelDesc` 里所有路径都是**相对 QC 所在目录**的字符串
/// （`$pushd` 前缀已经拼进去），所以调用方可以直接拿
/// `path.parent()` 当 `base_dir` 去 `compile()`。
pub fn parse_qc_file(path: &Path) -> Result<crate::model::ModelDesc, Vec<QcError>> {
    let qdir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut lex = lexer::Lexer::new(qdir);
    let mut errors = Vec::new();

    if let Err(e) = lex.load(path) {
        errors.push(e);
        return Err(errors);
    }

    let mut parser = parse::Parser::new(lex, path);
    match parser.run() {
        Ok(desc) => Ok(desc),
        Err(mut errs) => {
            errors.append(&mut errs);
            Err(errors)
        }
    }
}

/// 解析**内存里**的 QC 文本（单元测试用）。
///
/// `qdir` 是 `$include` 与网格路径的基准目录。
pub fn parse_qc_str(text: &str, qdir: &Path) -> Result<crate::model::ModelDesc, Vec<QcError>> {
    let mut lex = lexer::Lexer::new(qdir.to_path_buf());
    lex.load_from_memory(text, "<memory>");
    let mem_path = PathBuf::from("<memory>");
    let mut parser = parse::Parser::new(lex, &mem_path);
    parser.run()
}
