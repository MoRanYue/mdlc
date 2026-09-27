//! 分段计时（**默认零开销**），底层是 `hotpath`。
//!
//! 注：这里不写 intra-doc 链接 —— `hotpath` 是 optional 依赖，默认构建下
//! 不在 extern prelude 里，`[`hotpath`]` 会让 `cargo doc -D warnings` 报
//! `no item named hotpath in scope`。
//!
//! # 用法
//!
//! ```text
//! cargo build --release --features hotpath
//! $env:MDLC_PROF=1; .\target\release\mdlc.exe build x.toml --out y
//! ```
//!
//! 报告**写到文件**（默认工作目录下的 `mdlc-prof.txt`），不写 stdout ——
//! 理由见 `src/diag.rs`：官方 `studiomdl.exe` 把包括 `ERROR:` 在内的所有
//! 输出都写 stdout，Crowbar 靠 stdout 判断「编译器是否活着」，往那里塞一张
//! 性能表格会污染宿主日志。用 `HOTPATH_OUTPUT_PATH` 可以改路径（该环境变量
//! 的优先级高于这里写死的默认值）。
//!
//! # 为什么要保留一个 feature（实测数据）
//!
//! 最初的实现是「运行时查一次 `MDLC_PROF` 环境变量」。实测发现它给
//! **单 LOD 常见路径**（基准语料 3302/3302 都是单 LOD）带来约
//! **2.5% 的额外耗时**（`base 116 ms` vs `带探针 120 ms`，各 7 次中位数），
//! 而这条路径本来一行代码都没改。性能改动**不能让未改动的路径变慢**，
//! 所以必须能在编译期整体消掉。
//!
//! 换成 `hotpath` 之后这一点仍然成立：`hotpath` 自己的 `hotpath` feature
//! 关闭时，`Span` 是零大小类型、`start()` 直接返回 `None`，整个 crate
//! 只有 3 个依赖、不参与任何计时。
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
//!
//! # 两个容易踩的坑
//!
//! 1. **必须用 `let _guard = prof::start();` 绑定名字**。写成 `let _ = ...`
//!    会让 guard **立刻**析构，报告随之写出且计时区间为空。
//! 2. `start()` 建 guard 时会起一个 worker 线程和一个**本地回环 HTTP
//!    metrics server**（默认端口 6770，可用 `HOTPATH_METRICS_PORT` 改）。
//!    端口被占只会往 stderr 打一行提示，不影响报告；不想要它就把
//!    `HOTPATH_METRICS_SERVER_OFF=1` 设上。**只有 `MDLC_PROF` 被设置时
//!    才会走到这里**，默认构建连 guard 都不建。

// ---------------------------------------------------------------------------
// 关闭时：全部退化成空实现
// ---------------------------------------------------------------------------

#[cfg(not(feature = "hotpath"))]
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

    /// 关闭时占位，保证 `start()` 的签名在两种模式下一致。
    pub struct Guard;

    #[inline(always)]
    pub fn start() -> Option<Guard> {
        None
    }
}

// ---------------------------------------------------------------------------
// 打开时：交给 hotpath
// ---------------------------------------------------------------------------

#[cfg(feature = "hotpath")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 报告默认写到工作目录下的这个名字；`HOTPATH_OUTPUT_PATH` 可覆盖。
    const DEFAULT_REPORT: &str = "mdlc-prof.txt";

    /// guard 是否真的建起来了。没有它的时候**不能**去构造 `Span`：那样会把
    /// 测量值送进一个没人消费的通道，纯粹是白付开销。
    static ACTIVE: AtomicBool = AtomicBool::new(false);

    fn enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("MDLC_PROF").is_some_and(|v| v != "0"))
    }

    /// 持有 `hotpath` 的顶层 guard；析构时写出报告。
    ///
    /// 必须**绑定到一个具名变量**并让它活到 `main` 结束，否则报告会在绑定
    /// 语句处就写出来（而且那一瞬间什么都还没测到）。
    pub struct Guard {
        _inner: hotpath::HotpathGuard,
    }

    /// 在 `MDLC_PROF` 打开时启动计时；否则返回 `None`（零副作用）。
    ///
    /// `functions_limit(0)` 是必须的：hotpath 默认只列前 15 个函数，而本仓库
    /// 有 16 个分段标签，加上顶层 wrapper 一共 17 行，默认值会把它们截掉。
    pub fn start() -> Option<Guard> {
        if !enabled() {
            return None;
        }
        let inner = hotpath::HotpathGuardBuilder::new("mdlc")
            .functions_limit(0)
            .percentiles(&[50.0, 95.0, 99.0])
            .output_path(DEFAULT_REPORT)
            .build();
        ACTIVE.store(true, Ordering::Relaxed);
        Some(Guard { _inner: inner })
    }

    /// 在作用域结束时把耗时记到同名标签下（标签之间靠名字聚合）。
    pub struct Span {
        // 用 `Option` 而不是 `#[cfg]`：这样「未启动」只是一个空 `Option`，
        // 调用点不必区分自己跑在哪种模式下。
        _g: Option<hotpath::functions::MeasurementGuardSync>,
    }

    impl Span {
        pub fn new(name: &'static str) -> Self {
            if !ACTIVE.load(Ordering::Relaxed) {
                return Span { _g: None };
            }
            Span {
                _g: Some(hotpath::functions::build_measurement_guard_block(name, false)),
            }
        }
    }
}

pub use imp::{Guard, Span, start};
