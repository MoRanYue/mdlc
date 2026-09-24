//! 诊断输出的**流路由**。
//!
//! # 为什么需要它
//!
//! 官方 `studiomdl.exe` 把**所有**输出（含 `ERROR:`）写到 **stdout** ——
//! 实测：
//!
//! ```text
//! studiomdl 的 ERROR 在 stdout 里 → True
//! studiomdl 的 ERROR 在 stderr 里 → False
//! ```
//!
//! 而 Crowbar 判定「编译器是否活着」只认 **stdout**
//! （`Crowbar\Core\Compiler\Compiler.vb`）：
//!
//! | 行 | 处理器 | 行为 |
//! |---|---|---|
//! | `:751` | `OutputDataReceived`（stdout） | `theProcessHasOutputData = True` |
//! | `:785-803` | `ErrorDataReceived`（stderr） | 只显示，**不置位** |
//!
//! 所以当 mdlc 把错误写到 stderr 时，Crowbar 会一边**显示出**那些错误、
//! 一边补一句误导的
//! `ERROR: The compiler did not return any status messages.`
//! `CAUSE: The compiler is not the correct one for the selected game.`
//!
//! # 判据：调用形态，不是父进程名
//!
//! 用 **`-game <gamedir> <qc>` 这个兼容形态**当判据，而不是去检测父进程
//! 是不是 `Crowbar.exe`。理由：
//!
//! - **零依赖、跨平台** —— 本 crate 目前没有任何平台专有代码
//!   （CI 有一条 ubuntu job 专门证明这点）；检测父进程要 Windows 专有 API。
//! - **覆盖更广** —— 除了 Crowbar，任何照官方行为写的 GUI 包装/脚本都受益。
//! - **行为可预测** —— 父进程名会被改名、被包装、被 CI 调用，
//!   而调用形态是用户显式选择的，能写进文档。
//!
//! mdlc 自有子命令（`build` / `check` / …）**不受影响**，仍走 stderr ——
//! 既有脚本按退出码判定，且 stderr 是这类工具的惯例。

use std::sync::atomic::{AtomicBool, Ordering};

/// 诊断是否改走 stdout。默认 `false`（stderr）。
static TO_STDOUT: AtomicBool = AtomicBool::new(false);

/// 设置诊断流。**必须在任何诊断输出之前调用一次**（`run_official` 入口）。
pub fn set_to_stdout(v: bool) {
    TO_STDOUT.store(v, Ordering::Relaxed);
}

/// 诊断当前是否走 stdout。
#[inline]
pub fn to_stdout() -> bool {
    TO_STDOUT.load(Ordering::Relaxed)
}

/// `eprintln!` 的路由版：官方兼容模式下走 **stdout**（与官方一致），
/// 否则走 stderr。
#[macro_export]
macro_rules! diagln {
    () => {
        if $crate::diag::to_stdout() {
            println!()
        } else {
            eprintln!()
        }
    };
    ($($arg:tt)*) => {
        if $crate::diag::to_stdout() {
            println!($($arg)*)
        } else {
            eprintln!($($arg)*)
        }
    };
}

/// `eprint!` 的路由版（**不换行**）。见 [`diagln!`]。
///
/// ⚠️ 名字**不能**叫 `diag!` —— 那会与本模块名 `diag` 撞（虽然 Rust 的
/// 宏与模块分属不同命名空间，但 `use mdlc::diag` 会同时引入两者，
/// 读代码的人极容易看错）。
#[macro_export]
macro_rules! diagprint {
    ($($arg:tt)*) => {
        if $crate::diag::to_stdout() {
            print!($($arg)*)
        } else {
            eprint!($($arg)*)
        }
    };
}
