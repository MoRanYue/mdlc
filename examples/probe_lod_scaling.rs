//! 只读探针：量出多 LOD 路径（`unify_lods_remapped`）的**复杂度**。
//! 
//! 背景：`find_best_in_range` 对**每个** LOD-N 顶点线性扫描根 LOD 的顶点池
//! （O(N)），而 LOD-N 顶点数也是 O(N) ⟹ 整体 O(N²)。
//! 
//! 本探针生成一串规模递增、且每档 LOD 与 LOD0 顶点数相同的用例，
//! 量 `compile()` 的耗时，看它是不是平方增长。
//! 
//! 用法：
//!   cargo run --release --example probe_lod_scaling -- <源 SMD 目录> <输出目录>

use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(src_dir), Some(out_dir)) = (args.first(), args.get(1)) else {
        eprintln!("用法：probe_lod_scaling <源 SMD 目录> <输出目录>");
        std::process::exit(2);
    };
    let src_dir = PathBuf::from(src_dir);
    let out_dir = PathBuf::from(out_dir);
    std::fs::create_dir_all(&out_dir).expect("建输出目录");

    // 收集候选 SMD，按大小排序，挑出「单 mesh、顶点数递增」的几档。
    let mut smds: Vec<PathBuf> = std::fs::read_dir(&src_dir)
        .expect("读源目录")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("smd")))
        .collect();
    smds.sort();

    // 对每个候选，量出三角形数（跳过 idle 那种没有三角形的）。
    let mut cands: Vec<(PathBuf, usize, usize)> = Vec::new();
    for p in &smds {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        let Ok(s) = mdlc::smd::parse_smd(&text) else {
            continue;
        };
        if s.triangles.is_empty() {
            continue;
        }
        let verts: usize = s.triangles.len() * 3;
        cands.push((p.clone(), s.triangles.len(), verts));
    }
    cands.sort_by_key(|c| c.1);
    if cands.len() < 2 {
        eprintln!("需要至少 2 个带三角形的 SMD 才能做规模对比");
        std::process::exit(1);
    }

    println!(
        "{:<28} {:>10} {:>12} {:>14}",
        "用例（LOD0 + 3 档同规模）", "三角形", "compile ms", "ms/三角形"
    );
    println!("{}", "-".repeat(68));

    let mut prev: Option<(usize, f64)> = None;
    for (src, tris, _) in &cands {
        let base = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "b".into());
        let tag = format!("lodscale_{base}");
        // 复制 4 份作为 LOD0 + LOD1..3。
        for k in 0..4 {
            std::fs::copy(src, out_dir.join(format!("{tag}_l{k}.smd"))).expect("复制 SMD");
        }
        // 造一个「没有三角形」的序列 SMD（复用该文件即可，序列段会被忽略）。
        let toml = format!(
            r#"[model]
name = "bench/{tag}.mdl"
surface_prop = "metal"

[materials]
search_paths = ["models/bench"]
textures = [{{ name = "mat" }}]

[[bones]]
name = "root"

[[bones]]
name = "spine"
parent = "root"

[[bones]]
name = "arm"
parent = "spine"

[[bones]]
name = "hand"
parent = "arm"

[[bodyparts]]
name = "body0"

[[bodyparts.models]]
smd = "{tag}_l0.smd"

[[bodyparts.models.lods]]
smd = "{tag}_l1.smd"
switch_point = 20.0

[[bodyparts.models.lods]]
smd = "{tag}_l2.smd"
switch_point = 40.0

[[bodyparts.models.lods]]
smd = "{tag}_l3.smd"
switch_point = 60.0

[[sequences]]
name = "idle"
smd = "{tag}_l0.smd"
fps = 30.0
"#
        );
        let toml_path = out_dir.join(format!("{tag}.toml"));
        std::fs::write(&toml_path, toml).expect("写 TOML");

        let text = std::fs::read_to_string(&toml_path).unwrap();
        let desc: mdlc::model::ModelDesc = toml::from_str(&text).unwrap();
        let base_dir = out_dir.clone();

        // 预热一次（页缓存 / 分配器），再取 3 次中位数。
        std::hint::black_box(mdlc::compile::compile(&desc, &base_dir).expect("warm compile"));
        let mut times = Vec::new();
        for _ in 0..3 {
            let t = Instant::now();
            std::hint::black_box(mdlc::compile::compile(&desc, &base_dir).expect("compile"));
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let ms = times[1];
        println!(
            "{:<28} {:>10} {:>12.1} {:>14.4}",
            format!("{base}（4 档）"),
            tris,
            ms,
            ms / *tris as f64
        );

        if let Some((pt, pm)) = prev {
            let ratio_t = *tris as f64 / pt as f64;
            let ratio_m = ms / pm;
            println!(
                "    ↑ 相对上一档：三角形 ×{ratio_t:.2}，耗时 ×{ratio_m:.2}  ⟹ 指数 ≈ {:.2}",
                ratio_m.ln() / ratio_t.ln()
            );
        }
        prev = Some((*tris, ms));
    }
    println!();
    println!("指数 ≈ 2 表示 O(N²)（每顶点线性扫描），≈ 1 表示 O(N)。");
    let _ = Path::new("");
}
