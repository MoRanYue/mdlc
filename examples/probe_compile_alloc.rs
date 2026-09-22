//! 把 `compile()` 的分配量测出来，用来解释「parse 之外那 71%」。
//!
//! `profile_stages` 显示 `compile()` 占 90%，`profile_smd` 显示其中
//! 读+解析 SMD 只占 ~28%。本探针回答：剩下的时间里有多少是 malloc。
//!
//! 用法：
//! ```text
//! cargo run --release --example probe_compile_alloc -- <x.toml>
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static REALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(l.size(), Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        REALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn snap() -> (usize, usize, usize) {
    (
        ALLOCS.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed),
        REALLOCS.load(Ordering::Relaxed),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(p) = args.first() else {
        eprintln!("用法：probe_compile_alloc <x.toml>");
        std::process::exit(2);
    };
    let toml_path = PathBuf::from(p);
    let text = std::fs::read_to_string(&toml_path).expect("读 TOML");
    let desc: mdlc::model::ModelDesc = toml::from_str(&text).expect("解析 TOML");
    let base: PathBuf = toml_path.parent().unwrap_or(Path::new(".")).to_path_buf();

    // 预热
    std::hint::black_box(mdlc::compile::compile(&desc, &base).expect("compile"));

    // ---- 完整 compile() ----
    let (a0, b0, r0) = snap();
    let t = Instant::now();
    let compiled = mdlc::compile::compile(&desc, &base).expect("compile");
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let (a1, b1, r1) = snap();
    let compile_allocs = a1 - a0;
    let compile_bytes = b1 - b0;
    let compile_reallocs = r1 - r0;

    // 规模统计
    let n_models = compiled.bodyparts.iter().map(|b| b.models.len()).sum::<usize>();
    let n_meshes: usize = compiled
        .bodyparts
        .iter()
        .flat_map(|b| &b.models)
        .map(|m| m.meshes.len())
        .sum();
    let n_verts: usize = compiled
        .bodyparts
        .iter()
        .flat_map(|b| &b.models)
        .flat_map(|m| &m.meshes)
        .map(|k| k.vertices.len())
        .sum();
    let n_tris: usize = compiled
        .bodyparts
        .iter()
        .flat_map(|b| &b.models)
        .flat_map(|m| &m.meshes)
        .map(|k| k.triangles.len())
        .sum();

    println!("TOML                {}", toml_path.display());
    println!("compile()           {ms:.1} ms");
    println!();
    println!("model               {n_models}");
    println!("mesh                {n_meshes}");
    println!("去重后顶点          {n_verts}");
    println!("三角形              {n_tris}");
    println!();
    println!("{:<28} {:>16}", "指标", "值");
    println!("{}", "-".repeat(46));
    println!("{:<28} {:>16}", "alloc 次数", compile_allocs);
    println!("{:<28} {:>16}", "realloc 次数", compile_reallocs);
    println!("{:<28} {:>16}", "alloc 字节", compile_bytes);
    println!("{}", "-".repeat(46));
    println!(
        "{:<28} {:>16.1}",
        "每三角形 alloc 次数",
        compile_allocs as f64 / n_tris.max(1) as f64
    );
    println!(
        "{:<28} {:>16.1}",
        "每次 alloc 平均 ns",
        ms * 1e6 / compile_allocs.max(1) as f64
    );
    println!(
        "{:<28} {:>16.1}",
        "若 40 ns/alloc，总耗时 ms",
        compile_allocs as f64 * 40.0 / 1e6
    );
    println!(
        "{:<28} {:>16.1} %",
        "该估算占 compile() 比例",
        100.0 * (compile_allocs as f64 * 40.0 / 1e6) / ms
    );
}
