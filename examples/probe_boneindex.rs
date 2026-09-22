//! 量出 `desc.bone_index()`（骨骼名 → 下标 HashMap）的单次成本，
//! 再乘以「每顶点一次」的调用次数，判断它是不是 `compile()` 的主开销。
//!
//! 背景：`compile.rs::smd_vertex_to_ir` 里有一行
//! `let desc_index = desc.bone_index();` —— 它在**每个顶点**上重建一次
//! 整张骨骼表。本探针用来把这个猜测变成数字。
//!
//! 用法：
//! ```text
//! cargo run --release --example probe_boneindex -- <x.toml>
//! ```

use std::path::PathBuf;
use std::time::Instant;

use mdlc::model::ModelDesc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(toml_path) = args.first() else {
        eprintln!("用法：probe_boneindex <x.toml>");
        std::process::exit(2);
    };
    let toml_path = PathBuf::from(toml_path);

    let text = std::fs::read_to_string(&toml_path).expect("读 TOML");
    let desc: ModelDesc = toml::from_str(&text).expect("解析 TOML");
    let n_bones = desc.bones.len();

    // 单次成本：跑 200_000 次取总时间。
    const N: usize = 200_000;
    // 预热
    for _ in 0..1000 {
        std::hint::black_box(desc.bone_index());
    }
    let t = Instant::now();
    let mut sink = 0usize;
    for _ in 0..N {
        sink += std::hint::black_box(desc.bone_index()).len();
    }
    let per_call_ns = t.elapsed().as_secs_f64() * 1e9 / N as f64;
    std::hint::black_box(sink);

    println!("TOML        {}", toml_path.display());
    println!("骨骼数      {}", n_bones);
    println!();
    println!("bone_index() 单次   {:.0} ns  （{} 次实测）", per_call_ns, N);
    println!();

    // 每个 SMD 三角形顶点都会调用一次 ⟹ 调用次数 = 3 × 三角形数。
    // 从 TOML 里读不到三角形数（网格在 SMD 里），所以按调用方给的规模算。
    let counts: [(u64, &str); 4] = [
        (50_244, "t50000"),
        (250_632, "t250000"),
        (501_264, "t500000"),
        (1_002_528, "t1000000"),
    ];
    println!("{:<12} {:>10} {:>16} {:>14}", "case", "三角形", "调用次数(=3T)", "预估总耗时 ms");
    println!("{}", "-".repeat(56));
    for (tris, name) in counts {
        let calls = tris * 3;
        let ms = per_call_ns * calls as f64 / 1e6;
        println!("{:<12} {:>10} {:>16} {:>14.0}", name, tris, calls, ms);
    }
}
