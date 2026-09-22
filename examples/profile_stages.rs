//! 分阶段性能剖析探针（**只读**，不修改任何既有模块）。
//!
//! 用途：回答「mdlc 的时间花在哪个阶段」，为 SIMD 化选点提供实测依据。
//! 与 `src/main.rs` 的 `compile_and_write` 走**同一串公开 API**，
//! 只是把每个阶段单独计时。
//!
//! 用法：
//! ```text
//! cargo run --release --example profile_stages -- <x.toml> [--reps N]
//! ```
//!
//! 输出每阶段的中位数毫秒数与占比。

use std::path::{Path, PathBuf};
use std::time::Instant;

use mdlc::compile::compile;
use mdlc::mdl_writer::{flatten_vertices, write_mdl};
use mdlc::model::ModelDesc;
use mdlc::vtx_writer::{self, write_vtx_with};
use mdlc::vvd::check_invariants;

/// 一个阶段的累计耗时样本。
struct Stage {
    name: &'static str,
    samples: Vec<f64>,
}

impl Stage {
    fn new(name: &'static str) -> Self {
        Stage {
            name,
            samples: Vec::new(),
        }
    }
    fn push(&mut self, ms: f64) {
        self.samples.push(ms);
    }
    fn median(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut s = self.samples.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s[s.len() / 2]
    }
}

/// 一次完整流程，返回各阶段耗时（毫秒）。
fn one_pass(toml_path: &Path, stages: &mut [Stage]) {
    // ---- 阶段 0：读 TOML + 解析 ----
    let t = Instant::now();
    let text = std::fs::read_to_string(toml_path).expect("读 TOML");
    let desc: ModelDesc = toml::from_str(&text).expect("解析 TOML");
    stages[0].push(t.elapsed().as_secs_f64() * 1e3);

    let base: PathBuf = toml_path.parent().unwrap_or(Path::new(".")).to_path_buf();

    // ---- 阶段 1：compile()（读 SMD + 划分 mesh + 姿态 + flex/LOD/权重）----
    let t = Instant::now();
    let compiled = compile(&desc, &base).expect("compile");
    stages[1].push(t.elapsed().as_secs_f64() * 1e3);

    // ---- 阶段 2：write_mdl（含骨骼/序列/动画链）----
    let t = Instant::now();
    let out = write_mdl(&compiled).expect("write_mdl");
    stages[2].push(t.elapsed().as_secs_f64() * 1e3);

    // ---- 阶段 3：flatten_vertices ----
    let t = Instant::now();
    let flat = flatten_vertices(&compiled, &out.spans);
    stages[3].push(t.elapsed().as_secs_f64() * 1e3);

    // ---- 阶段 4：build_vvd（切线 + 多 LOD fixup）----
    let t = Instant::now();
    let multi = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .any(|m| m.lods.as_ref().is_some_and(|l| l.is_multi()));
    let vvd = if multi {
        mdlc::lod::build_multi_lod_vvd(&compiled, out.checksum).expect("multi vvd").0
    } else {
        mdlc::lod::build_single_lod_vvd(&compiled, out.checksum).expect("single vvd")
    };
    stages[4].push(t.elapsed().as_secs_f64() * 1e3);

    // ---- 阶段 5：VVD 序列化 ----
    let t = Instant::now();
    let vvd_bytes = vvd.to_bytes().expect("vvd bytes");
    stages[5].push(t.elapsed().as_secs_f64() * 1e3);

    // ---- 阶段 6：VTX 写出 ----
    let t = Instant::now();
    let vtx = write_vtx_with(
        &compiled,
        vtx_writer::VtxOptions {
            optimize_vertex_cache: desc.model.optimize_vtx,
        },
    )
    .expect("vtx");
    stages[6].push(t.elapsed().as_secs_f64() * 1e3);

    // ---- 阶段 7：自检 ----
    let t = Instant::now();
    check_invariants(&vvd, vvd_bytes.len()).expect("vvd 自洽");
    vtx_writer::check_invariants(&vtx, &compiled).expect("vtx 自洽");
    stages[7].push(t.elapsed().as_secs_f64() * 1e3);

    // 消费掉，防止优化器把整段消掉。
    std::hint::black_box((flat.len(), out.bytes.len(), vvd_bytes.len(), vtx.bytes.len()));
}

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
        eprintln!("用法：profile_stages <x.toml> [--reps N]");
        std::process::exit(2);
    };

    let names = [
        "读+解析 TOML",
        "compile()",
        "write_mdl()",
        "flatten_vertices()",
        "build_vvd() 切线",
        "VVD to_bytes()",
        "write_vtx()",
        "自检",
    ];
    let mut stages: Vec<Stage> = names.iter().map(|n| Stage::new(n)).collect();

    // 预热一次（页缓存 / 分配器），不计入。
    let mut warm: Vec<Stage> = names.iter().map(|n| Stage::new(n)).collect();
    one_pass(&toml, &mut warm);

    for _ in 0..reps {
        one_pass(&toml, &mut stages);
    }

    let total: f64 = stages.iter().map(|s| s.median()).sum();
    println!("文件  {}", toml.display());
    println!("重复  {} 次，取中位数\n", reps);
    println!("{:<22} {:>10} {:>8}", "阶段", "中位数 ms", "占比");
    println!("{}", "-".repeat(44));
    for s in &stages {
        let m = s.median();
        println!("{:<22} {:>10.2} {:>7.1}%", s.name, m, 100.0 * m / total);
    }
    println!("{}", "-".repeat(44));
    println!("{:<22} {:>10.2}", "合计", total);
}
