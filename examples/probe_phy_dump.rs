// 用 mdlc 自己的 `phy::check_invariants` 解析 `.phy`，输出碰撞几何摘要。
//
// 用途：用户报「世界模型物理不对劲，一堆枪散落在地上会被推挤地很开，
// 好像有 1 个隐藏的巨大碰撞盒」——需要比较 mdlc 与官方（Valve 原版）
// 的**碰撞体尺寸**与**体积**。
//
// 用法：cargo run --release --example probe_phy_dump -- <a.phy> [b.phy]
use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: probe_phy_dump <a.phy> [b.phy]");
        std::process::exit(2);
    }
    for p in &args {
        let bytes = match fs::read(p) {
            Ok(b) => b,
            Err(e) => {
                println!("=== {p} ===\n  读取失败：{e}");
                continue;
            }
        };
        println!("=== {p} ===");
        println!("  文件 {} 字节", bytes.len());
        match mdlc::phy::check_invariants(&bytes) {
            Ok(layout) => {
                println!("  ✅ 不变量自检通过");
                println!("     solid_count = {}", layout.solid_count);
                println!("     solids_end  = {}", layout.solids_end);
                println!("     text_size   = {}", layout.text_size);
                println!("     surface_sizes = {:?}", layout.surface_sizes);
                println!("     node_counts   = {:?}", layout.node_counts);
                println!("     ledge_region_sizes = {:?}", layout.ledge_region_sizes);
                let text = &bytes[layout.solids_end..];
                let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
                let s = String::from_utf8_lossy(&text[..end]);
                for line in s.lines() {
                    if !line.trim().is_empty() {
                        println!("      {line}");
                    }
                }
            }
            Err(e) => println!("  ❌ 不变量自检失败：{e}"),
        }
        println!();
    }
}
