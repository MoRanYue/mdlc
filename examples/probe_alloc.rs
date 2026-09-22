//! 用**计数分配器**量出 `parse_smd` 的分配次数与字节数。
//!
//! 动机：`profile_smd` 实测解析吞吐只有 ~115 MB/s（134 MB 的 SMD 花 1.16 s），
//! 远低于一个纯文本解析器应有的水平。本探针回答「时间是不是花在 malloc 上」。
//!
//! 全局分配器只装在**本 example 二进制**里，不碰 `src/`。
//!
//! 用法：
//! ```text
//! cargo run --release --example probe_alloc -- <case_dir>
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

/// 计数分配器：只统计次数与字节，行为完全转发给 `System`。
///
/// 另有 `BUMP` 模式（见下），用运行期开关切换 —— 这样**一个二进制**就能
/// 跑「真 malloc」与「无 malloc 成本」两组对照。
struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static REALLOCS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCS: AtomicUsize = AtomicUsize::new(0);

/// 是否处于 bump 模式。
static BUMP_MODE: AtomicBool = AtomicBool::new(false);
/// bump 区基址 / 已用偏移。
static BUMP_BASE: AtomicUsize = AtomicUsize::new(0);
static BUMP_OFF: AtomicUsize = AtomicUsize::new(0);
const BUMP_SIZE: usize = 6 << 30;

/// 进入 bump 模式。**只能在「此前所有分配都已释放」之后调用**。
///
/// # 安全约定（本 example 自己遵守）
///
/// bump 模式下的 `dealloc` 是空转。若一块 **bump** 内存在 bump 模式下
/// 被释放，空转即可；但绝不能让它落到 `System.dealloc` 上（那是 UB）。
/// 因此调用方必须保证：进入 bump 模式后**不再切回**、且进程随即结束。
fn enter_bump_mode() {
    let layout = std::alloc::Layout::from_size_align(BUMP_SIZE, 64).unwrap();
    let base = unsafe { System.alloc(layout) } as usize;
    assert!(base != 0, "bump 区分配失败");
    BUMP_BASE.store(base, Ordering::Relaxed);
    BUMP_OFF.store(0, Ordering::Relaxed);
    BUMP_MODE.store(true, Ordering::SeqCst);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if BUMP_MODE.load(Ordering::Relaxed) {
            // 按 16 字节对齐切一刀 —— 一次指针加法，无锁无系统调用。
            let size = (l.size() + 15) & !15;
            let off = BUMP_OFF.fetch_add(size, Ordering::Relaxed);
            return BUMP_BASE.load(Ordering::Relaxed).wrapping_add(off) as *mut u8;
        }
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(l.size(), Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if BUMP_MODE.load(Ordering::Relaxed) {
            return; // bump 模式：空转
        }
        DEALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if BUMP_MODE.load(Ordering::Relaxed) {
            // 搬运式 realloc：语义正确，代价与真分配器同阶。
            let np = unsafe { self.alloc(Layout::from_size_align_unchecked(new, l.align())) };
            unsafe { std::ptr::copy_nonoverlapping(p, np, l.size().min(new)) };
            return np;
        }
        REALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn snapshot() -> (usize, usize, usize, usize) {
    (
        ALLOCS.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
        REALLOCS.load(Ordering::Relaxed),
        DEALLOCS.load(Ordering::Relaxed),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("用法：probe_alloc <含 .smd 的目录>");
        std::process::exit(2);
    };

    let mut smds: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("读目录")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("smd")))
        .collect();
    smds.sort();

    let mut total_bytes = 0u64;
    let mut texts = Vec::new();
    for p in &smds {
        let t = std::fs::read_to_string(p).expect("读 SMD");
        total_bytes += t.len() as u64;
        texts.push(t);
    }
    println!("目录      {}", dir);
    println!("SMD       {} 个，{:.1} MB\n", smds.len(), total_bytes as f64 / 1e6);

    // 预热一次，让 Vec 增长路径进缓存。
    for t in &texts {
        std::hint::black_box(mdlc::smd::parse_smd(t).expect("解析"));
    }

    let (a0, b0, r0, d0) = snapshot();
    let t = Instant::now();
    let mut tris = 0usize;
    let mut verts = 0usize;
    let mut links = 0usize;
    for text in &texts {
        let s = mdlc::smd::parse_smd(text).expect("解析");
        tris += s.triangles.len();
        for tri in &s.triangles {
            for v in &tri.vertices {
                verts += 1;
                links += v.links.len();
            }
        }
        std::hint::black_box(&s);
    }
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let (a1, b1, r1, d1) = snapshot();

    let allocs = a1 - a0;
    let bytes = b1 - b0;
    let reallocs = r1 - r0;
    let deallocs = d1 - d0;

    println!("{:<22} {:>16}", "指标", "值");
    println!("{}", "-".repeat(40));
    println!("{:<22} {:>16}", "耗时 ms", format!("{ms:.1}"));
    println!("{:<22} {:>16}", "三角形", tris);
    println!("{:<22} {:>16}", "三角形顶点", verts);
    println!("{:<22} {:>16}", "骨骼绑定总数", links);
    println!("{}", "-".repeat(40));
    println!("{:<22} {:>16}", "alloc 次数", allocs);
    println!("{:<22} {:>16}", "realloc 次数", reallocs);
    println!("{:<22} {:>16}", "dealloc 次数", deallocs);
    println!("{:<22} {:>16}", "alloc 字节", bytes);
    println!("{}", "-".repeat(40));
    println!("{:<22} {:>16.1}", "每三角形 alloc 次数", allocs as f64 / tris as f64);
    println!("{:<22} {:>16.1}", "每次 alloc 平均字节", bytes as f64 / allocs.max(1) as f64);
    println!("{:<22} {:>16.1}", "每次 alloc 平均 ns", ms * 1e6 / allocs.max(1) as f64);
    println!();
    println!(
        "判读：若「每次 alloc 平均 ns」接近一次 malloc 的典型成本（~20-50 ns），\n\
         则解析时间是**分配主导**的 —— SIMD 对 malloc 无效。"
    );

    // ---- 对照实验：把分配器成本压到接近零，看解析还剩多少 ----
    //
    // 注意顺序：先释放所有真分配（`texts` / `s` 都已 drop），
    // 再切到 bump 模式；之后不再切回（否则 bump 指针会落到 System.dealloc）。
    drop(texts);
    enter_bump_mode();

    let mut bump_ms: Vec<f64> = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        let mut n = 0usize;
        for p in &smds {
            let text = std::fs::read_to_string(p).expect("读 SMD");
            let s = mdlc::smd::parse_smd(&text).expect("解析");
            n += s.triangles.len();
            std::hint::black_box(&s);
        }
        bump_ms.push(t.elapsed().as_secs_f64() * 1e3);
        std::hint::black_box(n);
    }
    bump_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let bump = bump_ms[bump_ms.len() / 2];

    println!();
    println!("{}", "=".repeat(52));
    println!("对照实验：同一个 parse_smd，换分配器");
    println!("{}", "=".repeat(52));
    println!("{:<26} {:>10.1} ms", "真 malloc（Counting）", ms);
    println!("{:<26} {:>10.1} ms", "bump（无 malloc 成本）", bump);
    println!("{:<26} {:>10.1} ms", "差值 = 分配/释放开销", ms - bump);
    println!(
        "{:<26} {:>10.1} %",
        "分配开销占比",
        100.0 * (ms - bump) / ms
    );
    println!();
    println!(
        "⇒ 这部分时间**任何 SIMD 都碰不到**：它发生在 malloc/free 里，\n\
         不在算术里。要拿回来只能改数据结构（少分配、批量分配）。"
    );
}
