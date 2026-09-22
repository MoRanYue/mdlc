//! SMD 解析阶段的独立计时探针（**只读**）。
//!
//! `profile_stages` 显示 `compile()` 占 90%，而 `compile()` 的第一件事就是
//! 读 + 解析 SMD。本探针把这两步单独拆出来，量出它们在总量里的份额。
//!
//! 用法：
//! ```text
//! cargo run --release --example profile_smd -- <case_dir>
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("用法：profile_smd <含 .smd 的目录>");
        std::process::exit(2);
    };
    let dir = PathBuf::from(dir);

    let mut smds: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("读目录")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("smd")))
        .collect();
    smds.sort();

    let total_bytes: u64 = smds.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum();
    println!("目录  {}", dir.display());
    println!("SMD   {} 个，共 {:.1} MB\n", smds.len(), total_bytes as f64 / 1e6);

    // ---- 读盘 ----
    let t = Instant::now();
    let mut texts = Vec::with_capacity(smds.len());
    for p in &smds {
        texts.push(std::fs::read_to_string(p).expect("读 SMD"));
    }
    let read_ms = t.elapsed().as_secs_f64() * 1e3;

    // ---- 解析（重复 3 次取中位数）----
    let mut parse_ms: Vec<f64> = Vec::new();
    let mut stats = (0usize, 0usize);
    for _ in 0..3 {
        let t = Instant::now();
        let mut tris = 0usize;
        let mut nodes = 0usize;
        for text in &texts {
            let s = mdlc::smd::parse_smd(text).expect("解析 SMD");
            tris += s.triangles.len();
            nodes += s.nodes.len();
        }
        parse_ms.push(t.elapsed().as_secs_f64() * 1e3);
        stats = (tris, nodes);
        std::hint::black_box(stats);
    }
    parse_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let parse = parse_ms[parse_ms.len() / 2];

    let mb = total_bytes as f64 / 1e6;
    println!("{:<16} {:>12} {:>14}", "阶段", "中位数 ms", "吞吐 MB/s");
    println!("{}", "-".repeat(44));
    println!("{:<16} {:>12.2} {:>14.1}", "read_to_string", read_ms, mb / (read_ms / 1e3));
    println!("{:<16} {:>12.2} {:>14.1}", "parse_smd", parse, mb / (parse / 1e3));
    println!("{}", "-".repeat(44));
    println!("{:<16} {:>12.2}", "读+解析 合计", read_ms + parse);
    println!("\n三角形 {}，骨骼 {}", stats.0, stats.1);

    let _ = Path::new("");
}
