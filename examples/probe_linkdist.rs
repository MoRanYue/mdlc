//! 统计 SMD 顶点**骨骼绑定数**的分布 —— 决定内联数组该开多大。
//!
//! 背景：`optparse` 原型把 `links` 内联成 3 组，结果在真实 Linnea 模型上
//! 出现 5076 处不一致 —— 那个模型有 **4 组绑定**的顶点。而 `compile.rs`
//! 的截断（`MAX_BONES_PER_VERT = 3`）发生在**按权重降序排序之后**，
//! 所以解析期不能截断。本探针量出真实分布，据此选内联容量。
//!
//! 用法：
//! ```text
//! cargo run --release --example probe_linkdist -- <目录...>
//! ```

use std::collections::BTreeMap;

fn main() {
    let dirs: Vec<String> = std::env::args().skip(1).collect();
    if dirs.is_empty() {
        eprintln!("用法：probe_linkdist <含 .smd 的目录> [...]");
        std::process::exit(2);
    }

    let mut dist: BTreeMap<usize, usize> = BTreeMap::new();
    let mut max_links = 0usize;
    let mut total_verts = 0usize;
    let mut total_tris = 0usize;
    let mut files = 0usize;
    let mut unsorted = 0usize;
    let mut truncated_diff = 0usize;

    for d in &dirs {
        let mut smds: Vec<std::path::PathBuf> = match std::fs::read_dir(d) {
            Ok(rd) => rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("smd")))
                .collect(),
            Err(_) => continue,
        };
        smds.sort();

        for p in smds {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(s) = mdlc::smd::parse_smd(&text) else {
                continue;
            };
            files += 1;
            total_tris += s.triangles.len();
            for t in &s.triangles {
                for v in &t.vertices {
                    total_verts += 1;
                    let n = v.links.len();
                    *dist.entry(n).or_default() += 1;
                    max_links = max_links.max(n);

                    // 检查权重是否已按降序排列（SMD 导出器不保证）。
                    if n > 1 && v.links.windows(2).any(|w| w[0].weight < w[1].weight) {
                        unsorted += 1;
                    }

                    // 「解析期截断到 3」会不会与「排序后取前 3」不同？
                    let parse_trunc: Vec<i32> = v.links.iter().take(3).map(|l| l.bone).collect();
                    let mut sorted = v.links.clone();
                    sorted.sort_by(|a, b| {
                        b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    let compile_trunc: Vec<i32> = sorted.iter().take(3).map(|l| l.bone).collect();
                    if parse_trunc != compile_trunc {
                        truncated_diff += 1;
                    }
                }
            }
        }
    }

    println!("文件        {files}");
    println!("三角形      {total_tris}");
    println!("顶点        {total_verts}");
    println!("最大绑定数  {max_links}");
    println!();
    println!("=== 绑定数分布 ===");
    println!("{:<10} {:>14} {:>10}", "绑定数", "顶点数", "占比");
    println!("{}", "-".repeat(36));
    for (n, c) in &dist {
        println!(
            "{:<10} {:>14} {:>9.4}%",
            n,
            c,
            100.0 * *c as f64 / total_verts.max(1) as f64
        );
    }
    println!();
    println!("=== 对「内联容量」的含义 ===");
    for cap in [3usize, 4, 5, 6, 8] {
        let covered: usize = dist.range(..=cap).map(|(_, c)| *c).sum();
        println!(
            "  内联 {cap} 组  →  覆盖 {:>7.4}% 的顶点（{:.0} 个需要溢出）",
            100.0 * covered as f64 / total_verts.max(1) as f64,
            (total_verts - covered) as f64
        );
    }
    println!();
    println!("=== 截断时机的陷阱 ===");
    println!("权重未按降序排列的顶点   {unsorted} / {total_verts}");
    println!(
        "「解析期截断到 3」与「排序后取前 3」结果不同的顶点   {truncated_diff}"
    );
    println!(
        "⇒ 若这个数 > 0，解析期就截断是**错的** —— 必须保留全部绑定，\n\
         把截断留给 compile.rs 排序之后做。"
    );
}
