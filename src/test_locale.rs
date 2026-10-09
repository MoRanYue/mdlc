//! 测试里切换语言（**仅 `#[cfg(test)]`**）。
//!
//! `rust_i18n` 的当前语言是**进程级全局状态**（一个 `static CURRENT_LOCALE`），
//! 而 `cargo test` 默认多线程 —— 任何断言文案的测试都必须先把它钉到某一门
//! 语言，且这些测试彼此之间必须**串行**。
//!
//! ⚠️ 锁与辅助函数只放这一份，不要各模块各写一份：两个独立的 `Mutex`
//! 等于没有锁 —— `cli` 的测试把语言改成 `zh-CN` 时，`pipeline` 的测试
//! 正拿着另一个锁断言英文，照样会偶发失败。

use std::sync::Mutex;

/// 串行化所有「改语言 → 断言 → 还原」的测试。
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

/// 在指定语言下跑一段代码，跑完还原原语言。
///
/// 还原不只是卫生问题：同一个进程里还跑着几百条别的测试。
///
/// 中毒的锁取回内层 guard —— 一条测试失败不该让其余测试连环失败。
pub fn with_locale<R>(locale: &str, f: impl FnOnce() -> R) -> R {
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved = (*rust_i18n::locale()).to_string();
    rust_i18n::set_locale(locale);
    let out = f();
    rust_i18n::set_locale(&saved);
    out
}

/// 在**英文源文**下跑一段代码。
///
/// 英文是源语言（键就是英文原文），所以 `t!` 未命中 ⟹ 拿到的是键本身。
/// 断英文文案的测试都走这里，而不是假定「没别人改过语言」。
pub fn with_english<R>(f: impl FnOnce() -> R) -> R {
    with_locale("en", f)
}
