//! 量出 IR 结构体的**内存足迹**，用来定位「峰值内存」的构成。
//!
//! 用法：`cargo run --release --example probe_structsize`

use std::mem::size_of;

fn main() {
    println!("{:<46} {:>8} {:>8}", "类型", "B", "含堆?");
    println!("{}", "-".repeat(66));
    println!(
        "{:<46} {:>8}",
        "model::Vertex（现状：bones 是 Vec）",
        size_of::<mdlc::model::Vertex>()
    );
    println!(
        "{:<46} {:>8}",
        "  其中 Vec 头本身",
        size_of::<Vec<[f32; 2]>>()
    );
    println!(
        "{:<46} {:>8}",
        "smd::SmdVertex",
        size_of::<mdlc::smd::SmdVertex>()
    );
    println!(
        "{:<46} {:>8}",
        "smd::SmdTriangle",
        size_of::<mdlc::smd::SmdTriangle>()
    );
    println!(
        "{:<46} {:>8}",
        "smd::SmdPose",
        size_of::<mdlc::smd::SmdPose>()
    );
    println!(
        "{:<46} {:>8}",
        "vvd::VvdVertex",
        size_of::<mdlc::vvd::VvdVertex>()
    );
    println!(
        "{:<46} {:>8}",
        "vvd::VvdTangent",
        size_of::<mdlc::vvd::VvdTangent>()
    );
    println!();

    let v = size_of::<mdlc::model::Vertex>();
    let sv = size_of::<mdlc::smd::SmdVertex>();
    let st = size_of::<mdlc::smd::SmdTriangle>();
    // 内联 3 组后的 Vertex：12 + 12 + 8 + 3*(4+4) + 1(pad→4) ≈ 56 → 56 B
    let inline_vertex = 12 + 12 + 8 + 3 * 8 + 4;
    println!("=== 每单位成本（现状 vs 内联后）===");
    println!("{:<34} {:>12} {:>12}", "项", "现状 B", "内联后 B");
    println!("{}", "-".repeat(60));
    println!("{:<34} {:>12} {:>12}", "IR Vertex（含 1 次堆分配）", v + 24, inline_vertex);
    println!("{:<34} {:>12} {:>12}", "SmdVertex（含 1 次堆分配）", sv + 16, inline_vertex);
    println!("{:<34} {:>12} {:>12}", "SmdTriangle", st, 8 + 3 * inline_vertex);
    println!();
    println!("注：SmdVertex 的 links 堆块最小 16 B（1 组 [i32,f32]）；");
    println!("    Vertex 的 bones 堆块最小 24 B（3 组 [f32;2] 按 8 B 对齐）。");
    println!();
    println!("=== 规模换算 ===");
    for (name, verts, tris) in [
        ("Linnea（真实）", 53_295usize, 60_060usize),
        ("t500000", 253_472, 501_264),
        ("t1000000", 1_008_192, 1_002_528),
    ] {
        let ir_now = verts * (v + 24);
        let ir_opt = verts * inline_vertex;
        let smd_now = tris * (st + 16 * 3); // 3 个顶点各一个 links 堆块
        let smd_opt = tris * (8 + 3 * inline_vertex);
        println!(
            "{:<16} IR 顶点 {:.1} → {:.1} MB   SMD 中间体 {:.1} → {:.1} MB",
            name,
            ir_now as f64 / 1e6,
            ir_opt as f64 / 1e6,
            smd_now as f64 / 1e6,
            smd_opt as f64 / 1e6
        );
    }
}
