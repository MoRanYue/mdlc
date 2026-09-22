//! 峰值内存 / 实时堆高水位探针（**只读**）。
//!
//! 回答「内存占用有多大、峰值出现在哪个阶段」。
//!
//! 计数分配器只装在**本 example 二进制**里，不碰 `src/`。
//!
//! 用法：
//! ```text
//! cargo run --release --example probe_peakmem -- <x.toml>
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct Track;

/// 当前存活堆字节。
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// 存活堆的**高水位**（峰值）。
static HIGH: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Track {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size(), Ordering::Relaxed);
            let live = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            HIGH.fetch_max(live, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let np = unsafe { System.realloc(p, l, new) };
        if !np.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new, Ordering::Relaxed);
            if new >= l.size() {
                let d = new - l.size();
                let live = LIVE.fetch_add(d, Ordering::Relaxed) + d;
                HIGH.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(l.size() - new, Ordering::Relaxed);
            }
        }
        np
    }
}

#[global_allocator]
static A: Track = Track;

fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}
fn high() -> usize {
    HIGH.load(Ordering::Relaxed)
}
/// 把高水位重置到**当前存活量** —— 否则预热阶段的峰值会永久压住后续测量。
fn reset_high() {
    HIGH.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}
fn allocs() -> usize {
    ALLOCS.load(Ordering::Relaxed)
}
fn bytes() -> usize {
    BYTES.load(Ordering::Relaxed)
}
fn mb(v: usize) -> f64 {
    v as f64 / (1024.0 * 1024.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(p) = args.first() else {
        eprintln!("用法：probe_peakmem <x.toml>");
        std::process::exit(2);
    };
    let toml_path = PathBuf::from(p);
    let text = std::fs::read_to_string(&toml_path).expect("读 TOML");
    let desc: mdlc::model::ModelDesc = toml::from_str(&text).expect("解析 TOML");
    let base: PathBuf = toml_path.parent().unwrap_or(Path::new(".")).to_path_buf();

    println!("TOML  {}", toml_path.display());
    println!();
    println!("{:<30} {:>14} {:>14} {:>16}", "阶段", "堆峰值 MB", "结束时 MB", "累计 alloc");
    println!("{}", "-".repeat(78));

    // ---- 预热（不进统计）----
    std::hint::black_box(mdlc::compile::compile(&desc, &base).expect("compile"));

    // ---- ① compile() ----
    let base_live = live();
    reset_high();
    let base_allocs = allocs();
    let base_bytes = bytes();
    let t = Instant::now();
    let compiled = mdlc::compile::compile(&desc, &base).expect("compile");
    let compile_ms = t.elapsed().as_secs_f64() * 1e3;
    let compile_high = high().saturating_sub(base_live);
    let compile_end = live().saturating_sub(base_live);
    let compile_allocs = allocs() - base_allocs;
    let _ = bytes() - base_bytes;
    println!(
        "{:<30} {:>14.1} {:>14.1} {:>16}",
        format!("① compile()  [{compile_ms:.0} ms]"),
        mb(compile_high),
        mb(compile_end),
        compile_allocs
    );

    // ---- ② write_mdl() ----
    let a0 = allocs();
    let base_live = live();
    reset_high();
    let t = Instant::now();
    let out = mdlc::mdl_writer::write_mdl(&compiled).expect("write_mdl");
    let ms = t.elapsed().as_secs_f64() * 1e3;
    println!(
        "{:<30} {:>14.1} {:>14.1} {:>16}",
        format!("② write_mdl()  [{ms:.0} ms]"),
        mb(high().saturating_sub(base_live)),
        mb(live().saturating_sub(base_live)),
        allocs() - a0
    );

    // ---- ③ flatten + VVD + VTX ----
    let a0 = allocs();
    let base_live = live();
    reset_high();
    let t = Instant::now();
    let flat = mdlc::mdl_writer::flatten_vertices(&compiled, &out.spans);
    let multi = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .any(|m| m.lods.as_ref().is_some_and(|l| l.is_multi()));
    let vvd = if multi {
        mdlc::lod::build_multi_lod_vvd(&compiled, out.checksum).expect("vvd").0
    } else {
        mdlc::lod::build_single_lod_vvd(&compiled, out.checksum).expect("vvd")
    };
    let vvd_bytes = vvd.to_bytes().expect("vvd bytes");
    let vtx = mdlc::vtx_writer::write_vtx_with(
        &compiled,
        mdlc::vtx_writer::VtxOptions {
            optimize_vertex_cache: desc.model.optimize_vtx,
        },
    )
    .expect("vtx");
    let ms = t.elapsed().as_secs_f64() * 1e3;
    println!(
        "{:<30} {:>14.1} {:>14.1} {:>16}",
        format!("③ VVD+VTX  [{ms:.0} ms]"),
        mb(high().saturating_sub(base_live)),
        mb(live().saturating_sub(base_live)),
        allocs() - a0
    );

    // ---- 汇总 ----
    let total_alloc_bytes = bytes() - base_bytes;
    println!("{}", "-".repeat(78));
    println!(
        "{:<30} {:>14.1} {:>14.1} {:>16}",
        "全程（未释放峰值 = 进程峰值）",
        mb(high().saturating_sub(base_live)),
        mb(live().saturating_sub(base_live)),
        allocs() - base_allocs
    );
    println!();
    println!("累计分配字节   {:.1} MB", mb(total_alloc_bytes));
    println!("分配/释放比    {:.1}×（累计分配 ÷ 峰值存活）", total_alloc_bytes as f64 / high().max(1) as f64);
    println!();
    println!(
        "产物           MDL {:.1} MB / VVD {:.1} MB / VTX {:.1} MB",
        mb(out.bytes.len()),
        mb(vvd_bytes.len()),
        mb(vtx.bytes.len())
    );
    println!("顶点 {}，三角形 {}", flat.len(), compiled.total_triangles());
}
