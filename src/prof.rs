//! 分段计时（**默认零开销**）。
//!
//! # 用法
//!
//! ```text
//! cargo build --release --features profiling
//! $env:MDLC_PROF=1; .\target\release\mdlc.exe build x.toml --out y
//! ```
//!
//! 不打开 `profiling` feature 时，本模块里所有函数都被 `#[cfg]` 消掉、
//! 变成**空实现**（`Span::new` 返回零大小类型、`dump`/`count` 是空函数），
//! 编译器会把它们全部内联掉 ⟹ **生产构建零开销**。
//!
//! # 为什么要做成 feature（实测数据）
//!
//! 最初是「运行时查一次 `MDLC_PROF` 环境变量」的实现。实测发现它给
//! **单 LOD 常见路径**（基准语料 3302/3302 都是单 LOD）带来约
//! **2.5% 的额外耗时**（`base 116 ms` vs `带探针 120 ms`，各 7 次中位数），
//! 而这条路径本来一行代码都没改。性能改动**不能让未改动的路径变慢**，
//! 所以改成编译期开关。
//!
//! # 存在的理由（§49 的实测结论）
//!
//! `PROGRESS.md` §48 曾记着「多 LOD 的 `unify_lods_remapped` 只占
//! `102 ms / 8721 ms`，真正的热点不在那里」。**§49 实测推翻了这条**：
//!
//! ```text
//! LOD: unify_lods_remapped(全部 mesh)   14414.5 ms  ×2
//! bodyparts: 读 SMD + build_meshes + LOD 14763.1 ms  ×2
//! ```
//!
//! 它占了 `compile()` 的 **97.7%**，而且内部两处线性扫描各占一半。
//! 教训：**交接文档里的性能结论也会过期** —— 换一台机器、换一个夹具
//! 就可能完全不同。要量，不要抄。

// ---------------------------------------------------------------------------
// 关闭时：全部退化成空实现
// ---------------------------------------------------------------------------

#[cfg(not(feature = "profiling"))]
mod imp {
    /// 空 `Span`：零大小，`new`/`Drop` 都是 no-op。
    pub struct Span;

    impl Span {
        #[inline(always)]
        pub fn new(_name: &'static str) -> Self {
            Span
        }
    }

    // 显式给一个**空的** `Drop`：调用点写了 `drop(_t_x)` 来提前结束计时区间，
    // 若这里没有 `Drop`，clippy 会报 `drop_non_drop`（「drop 一个不实现 Drop
    // 的类型只延长其借用」）。空实现让那些调用点保持原样且零成本。
    impl Drop for Span {
        #[inline(always)]
        fn drop(&mut self) {}
    }

    #[inline(always)]
    pub fn count(_name: &'static str, _n: u64) {}

    #[inline(always)]
    pub fn dump() {}
}

// ---------------------------------------------------------------------------
// 打开时：真实计时
// ---------------------------------------------------------------------------

#[cfg(feature = "profiling")]
mod imp {
    use std::sync::OnceLock;
    use std::time::Instant;

    fn enabled() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("MDLC_PROF").is_some_and(|v| v != "0"))
    }

    struct Entry {
        name: &'static str,
        total: std::time::Duration,
        count: u64,
    }

    fn entries() -> &'static std::sync::Mutex<Vec<Entry>> {
        static E: OnceLock<std::sync::Mutex<Vec<Entry>>> = OnceLock::new();
        E.get_or_init(|| std::sync::Mutex::new(Vec::new()))
    }

    /// 计数器（用于「某条分支被走了多少次」这类问题）。
    fn counters() -> &'static std::sync::Mutex<Vec<(&'static str, u64)>> {
        static C: OnceLock<std::sync::Mutex<Vec<(&'static str, u64)>>> = OnceLock::new();
        C.get_or_init(|| std::sync::Mutex::new(Vec::new()))
    }

    pub fn count(name: &'static str, n: u64) {
        if !enabled() {
            return;
        }
        let mut g = counters().lock().unwrap();
        if let Some(e) = g.iter_mut().find(|e| e.0 == name) {
            e.1 += n;
        } else {
            g.push((name, n));
        }
    }

    fn record(name: &'static str, d: std::time::Duration) {
        let mut g = entries().lock().unwrap();
        if let Some(e) = g.iter_mut().find(|e| e.name == name) {
            e.total += d;
            e.count += 1;
        } else {
            g.push(Entry {
                name,
                total: d,
                count: 1,
            });
        }
    }

    /// 在作用域结束时把耗时累加到同名条目。
    pub struct Span {
        name: &'static str,
        start: Option<Instant>,
    }

    impl Span {
        pub fn new(name: &'static str) -> Self {
            Span {
                name,
                start: enabled().then(Instant::now),
            }
        }
    }

    impl Drop for Span {
        fn drop(&mut self) {
            if let Some(t) = self.start {
                record(self.name, t.elapsed());
            }
        }
    }

    /// 打印并清空累计结果（`main` 退出前调用）。
    pub fn dump() {
        if !enabled() {
            return;
        }
        let mut g = entries().lock().unwrap();
        if g.is_empty() {
            return;
        }
        let mut rows: Vec<(String, f64, u64)> = g
            .iter()
            .map(|e| (e.name.to_string(), e.total.as_secs_f64() * 1e3, e.count))
            .collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let total: f64 = rows
            .iter()
            .filter(|r| !r.0.starts_with("  "))
            .map(|r| r.1)
            .sum();
        eprintln!("--- MDLC_PROF 分段计时（ms） ---");
        for (name, ms, count) in &rows {
            eprintln!("{ms:>12.1}  ×{count:<6} {name}");
        }
        eprintln!("{total:>12.1}  顶层合计");
        g.clear();
        drop(g);
        let c = counters().lock().unwrap().clone();
        if !c.is_empty() {
            eprintln!("--- 计数器 ---");
            for (n, v) in c {
                eprintln!("{v:>12}  {n}");
            }
        }
    }
}

pub use imp::{Span, count, dump};

