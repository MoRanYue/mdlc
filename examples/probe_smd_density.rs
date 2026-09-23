//! 只读探针：统计一个 SMD 的顶点位置/UV 分布，用于给 LOD 顶点字典
//! 选空间索引的网格尺寸。
//!
//! 用法：cargo run --release --example probe_smd_density -- <x.smd>

use std::collections::{HashMap, HashSet};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("用法：probe_smd_density <x.smd>");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(path).expect("读 SMD");
    let smd = mdlc::smd::parse_smd(&text).expect("解析 SMD");

    // 逐三角形顶点去重（按位置 bits）—— 近似 IR 顶点池。
    let mut uniq_pos: HashSet<[u32; 3]> = HashSet::new();
    let mut uniq_v: HashSet<([u32; 3], [u32; 2])> = HashSet::new();
    let mut n_corners = 0usize;
    let mut bbox = [f32::MAX; 3];
    let mut bbox_min = [f32::MIN; 3];
    for t in &smd.triangles {
        for v in &t.vertices {
            n_corners += 1;
            let p = v.position;
            uniq_pos.insert([p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]);
            uniq_v.insert(([p[0].to_bits(), p[1].to_bits(), p[2].to_bits()], [v.uv[0].to_bits(), v.uv[1].to_bits()]));
            for k in 0..3 {
                if p[k] < bbox[k] { bbox[k] = p[k]; }
                if p[k] > bbox_min[k] { bbox_min[k] = p[k]; }
            }
        }
    }

    // 0.05 网格占用分布
    let cell = 0.05f32;
    let mut grid: HashMap<(i64, i64, i64), usize> = HashMap::new();
    for b in &uniq_pos {
        let p = [f32::from_bits(b[0]), f32::from_bits(b[1]), f32::from_bits(b[2])];
        let key = (
            (p[0] / cell).floor() as i64,
            (p[1] / cell).floor() as i64,
            (p[2] / cell).floor() as i64,
        );
        *grid.entry(key).or_insert(0) += 1;
    }
    let mut occ: Vec<usize> = grid.values().copied().collect();
    occ.sort_unstable();
    let pct = |q: f64| -> usize {
        if occ.is_empty() { return 0; }
        let i = ((occ.len() as f64 - 1.0) * q).round() as usize;
        occ[i]
    };

    // 27 邻域平均候选数（即每次查询要扫的顶点数）
    let mut probe: Vec<usize> = Vec::new();
    for b in uniq_pos.iter().take(2000) {
        let p = [f32::from_bits(b[0]), f32::from_bits(b[1]), f32::from_bits(b[2])];
        let cx = (p[0] / cell).floor() as i64;
        let cy = (p[1] / cell).floor() as i64;
        let cz = (p[2] / cell).floor() as i64;
        let mut n = 0usize;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    n += grid.get(&(cx + dx, cy + dy, cz + dz)).copied().unwrap_or(0);
                }
            }
        }
        probe.push(n);
    }
    probe.sort_unstable();
    let avg: f64 = if probe.is_empty() { 0.0 } else { probe.iter().sum::<usize>() as f64 / probe.len() as f64 };

    println!("文件          : {path}");
    println!("三角形        : {}", smd.triangles.len());
    println!("角点数        : {n_corners}");
    println!("唯一位置      : {}", uniq_pos.len());
    println!("唯一(位置,UV) : {}", uniq_v.len());
    println!("包围盒        : {:?} .. {:?}", bbox, bbox_min);
    println!("0.05 网格格数 : {}", grid.len());
    println!("格内占用 p50/p90/p99/max : {}/{}/{}/{}", pct(0.5), pct(0.9), pct(0.99), occ.last().copied().unwrap_or(0));
    println!("27 邻域候选数 平均 {avg:.1}  p50 {}  p90 {}  max {}", 
        probe.get(probe.len()/2).copied().unwrap_or(0),
        probe.get(probe.len()*9/10).copied().unwrap_or(0),
        probe.last().copied().unwrap_or(0));
}
