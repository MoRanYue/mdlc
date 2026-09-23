//! 只读探针：量「多 LOD 顶点统一」的**复杂度指数**。
//!
//! 背景：`lod.rs::find_best_in_range` 对**每个** LOD-N 顶点线性扫描根 LOD
//! 顶点池（O(N)），而 LOD-N 顶点数本身也是 O(N) ⟹ 整体 O(N²)。
//!
//! 本探针不生成夹具（夹具由调用方准备好），只对一个已有的 TOML
//! 重复测 `compile()`，配合不同规模的 TOML 就能算出经验指数。
//!
//! 用法：
//! ```text
//! cargo run --release --example probe_lod_time -- <x.toml> [--reps N]
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut reps = 3usize;
    let mut toml: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--reps" => {
                i += 1;
                reps = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(3);
            }
            other => toml = Some(PathBuf::from(other)),
        }
        i += 1;
    }
    let Some(toml) = toml else {
        eprintln!("用法：probe_lod_time <x.toml> [--reps N]");
        std::process::exit(2);
    };

    let text = std::fs::read_to_string(&toml).expect("读 TOML");
    let desc: mdlc::model::ModelDesc = toml::from_str(&text).expect("解析 TOML");
    let base: PathBuf = toml.parent().unwrap_or(Path::new(".")).to_path_buf();

    // 规模：LOD0 的三角形数（其余档同规模）。
    let mut n_tris = 0usize;
    let mut n_lods = 1usize;
    if let Some(bp) = desc.bodyparts.first()
        && let Some(m) = bp.models.first()
    {
        n_lods = m.lods.len() + 1;
        let p = mdlc::compile::resolve_smd_path(&base, &m.smd);
        if let Ok(t) = std::fs::read_to_string(&p)
            && let Ok(s) = mdlc::smd::parse_smd(&t)
        {
            n_tris = s.triangles.len();
        }
    }

    // 预热
    std::hint::black_box(mdlc::compile::compile(&desc, &base).expect("warm"));

    let mut times = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        std::hint::black_box(mdlc::compile::compile(&desc, &base).expect("compile"));
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let ms = times[times.len() / 2];

    println!(
        "{:<34} LOD {}  三角形 {:>8}  compile {:>12.1} ms  ({:.4} ms/三角形)",
        toml.file_name().unwrap_or_default().to_string_lossy(),
        n_lods,
        n_tris,
        ms,
        ms / n_tris.max(1) as f64
    );
    mdlc::prof::dump();
}
