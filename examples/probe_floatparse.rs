//! 量出「浮点/整数解析」在 `parse_smd` 里占多大份额。
//!
//! 这是判断 SIMD 能不能帮上忙的**决定性数字**：SIMD 只能加速算术，
//! 所以只有「花在 `str::parse::<f32>()` 上的时间」才是它的可达范围。
//!
//! 做法：把 SMD 按 `parse_smd` 的同一套规则切成 token（`split_whitespace`
//! 加去 `//` 注释），然后**只**跑数字解析，单独计时。这是对
//! `parse_smd` 内部解析开销的忠实估计（同一批 token、同一个 `parse`）。
//!
//! 用法：
//! ```text
//! cargo run --release --example probe_floatparse -- <case_dir>
//! ```

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("用法：probe_floatparse <含 .smd 的目录>");
        std::process::exit(2);
    };

    let mut smds: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("读目录")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("smd")))
        .collect();
    smds.sort();

    let mut texts = Vec::new();
    let mut total_bytes = 0u64;
    for p in &smds {
        let t = std::fs::read_to_string(p).expect("读 SMD");
        total_bytes += t.len() as u64;
        texts.push(t);
    }

    // ---- 基线：完整 parse_smd ----
    for t in &texts {
        std::hint::black_box(mdlc::smd::parse_smd(t).expect("解析"));
    }
    let mut full_ms = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        for text in &texts {
            std::hint::black_box(mdlc::smd::parse_smd(text).expect("解析"));
        }
        full_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    full_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let full = full_ms[full_ms.len() / 2];

    // ---- 只切词（不解析数字）：量出 tokenize 的成本 ----
    let mut tok_ms = Vec::new();
    let mut n_tokens = 0usize;
    for _ in 0..3 {
        let t = Instant::now();
        let mut n = 0usize;
        for text in &texts {
            for raw in text.lines() {
                let body = match raw.find("//") {
                    Some(p) => &raw[..p],
                    None => raw,
                };
                n += body.split_whitespace().count();
            }
        }
        tok_ms.push(t.elapsed().as_secs_f64() * 1e3);
        n_tokens = n;
    }
    tok_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let tokenize = tok_ms[tok_ms.len() / 2];

    // ---- 切词 + 逐 token 数字解析 ----
    // 与 parse_smd 的关键差异：这里**不分配**（不建 SmdVertex），
    // 所以它量的是「解析算术」本身，而不是分配。
    let mut num_ms = Vec::new();
    let mut n_floats = 0usize;
    let mut n_ints = 0usize;
    for _ in 0..3 {
        let t = Instant::now();
        let mut sink = 0f64;
        let mut nf = 0usize;
        let mut ni = 0usize;
        for text in &texts {
            for raw in text.lines() {
                let body = match raw.find("//") {
                    Some(p) => &raw[..p],
                    None => raw,
                };
                for tok in body.split_whitespace() {
                    // SMD 的数字 token 以数字或 '-' / '.' 开头。
                    let b = tok.as_bytes();
                    if b.is_empty() {
                        continue;
                    }
                    let c = b[0];
                    if c.is_ascii_digit() || c == b'-' || c == b'.' {
                        // parse_smd 对整数 token 先试 i32 再退 f32；
                        // 这里模拟同一条路径。
                        if let Ok(v) = tok.parse::<i32>() {
                            sink += v as f64;
                            ni += 1;
                        } else if let Ok(v) = tok.parse::<f32>() {
                            sink += v as f64;
                            nf += 1;
                        }
                    }
                }
        std::hint::black_box(sink);
            }
        }
        num_ms.push(t.elapsed().as_secs_f64() * 1e3);
        n_floats = nf;
        n_ints = ni;
    }
    num_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let numeric = num_ms[num_ms.len() / 2];

    println!("目录          {}", dir);
    println!("SMD           {} 个，{:.1} MB", smds.len(), total_bytes as f64 / 1e6);
    println!("token 总数    {}", n_tokens);
    println!("  整数 token  {}", n_ints);
    println!("  浮点 token  {}", n_floats);
    println!();
    println!("{:<30} {:>12} {:>10}", "阶段", "中位数 ms", "占 full");
    println!("{}", "-".repeat(54));
    println!("{:<30} {:>12.1} {:>9.1}%", "① 完整 parse_smd", full, 100.0);
    println!(
        "{:<30} {:>12.1} {:>9.1}%",
        "② 只切词（split_whitespace）",
        tokenize,
        100.0 * tokenize / full
    );
    println!(
        "{:<30} {:>12.1} {:>9.1}%",
        "③ 切词 + 数字解析（无分配）",
        numeric,
        100.0 * numeric / full
    );
    println!("{}", "-".repeat(54));
    println!(
        "{:<30} {:>12.1} {:>9.1}%",
        "④ = ③ − ② 纯数字解析",
        numeric - tokenize,
        100.0 * (numeric - tokenize) / full
    );
    println!(
        "{:<30} {:>12.1} {:>9.1}%",
        "⑤ = ① − ③ 分配/建结构/拷贝",
        full - numeric,
        100.0 * (full - numeric) / full
    );
    println!();
    println!(
        "⇒ **SIMD 的理论可达上界 ≈ ④**（而且只在能把浮点解析向量化时才拿得到）。\n\
         ⑤ 是 malloc 与结构体构建，SIMD 完全碰不到。"
    );
}
