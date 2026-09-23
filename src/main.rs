//! `mdlc` 命令行入口。
//!
//! 子命令：
//! - `build <model.toml> [--out <目录>]`  TOML 描述 → `.mdl` + `.vvd`（**MVP 主线**）
//! - `check <model.toml>`                 只做校验，不写文件
//! - `vvd-info <file>`                    解析并打印 VVD 头部与统计
//! - `vvd-roundtrip <file>`               读入再写出，逐字节比对（布局判据）
//! - `phy <in.smd> <out.phy>`             SMD 三角形 → 凸包 → `.phy` 碰撞文件
//!
//! 另有**官方 `studiomdl` 兼容形态**（首参为 `-` 或以 `.qc` 结尾时启用）：
//! `mdlc -game <gamedir> [-nop4] [-verbose] <model.qc>`，产物写到
//! `<gamedir>\models\<$modelname>` —— 用于直接替换 Crowbar 的编译器路径。
//! 解析细节见 [`mdlc::cli`] 模块文档。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mdlc::compile::compile;
use mdlc::mdl_writer::{flatten_vertices, write_mdl};
use mdlc::model::ModelDesc;
use mdlc::phy::{self, PhyHull, PhyParams, PhySolid};
use mdlc::vtx_writer::{self, write_vtx_with};
use mdlc::vvd::{Vvd, check_invariants};

const USAGE: &str = "\
mdlc —— Source 引擎模型编译器（MVP：TOML 描述 → MDL/VVD）

用法：
  mdlc build <model.toml> [--out <目录>]
      把 TOML 描述编译成 <name>.mdl 与 <name>.vvd。
      --out 指定输出根目录（默认取当前目录），模型名里的目录结构会被创建。

  mdlc check <model.toml>
      只校验描述文件，不写任何文件。退出码 0=合法，1=有错误。

  mdlc vvd-info <file.vvd>
      解析并打印 VVD 头部、统计与自洽性检查结果。

  mdlc vvd-roundtrip <file.vvd>
      读入再写出，逐字节比对。用于验证布局理解是否正确。

  mdlc phy <in.smd> <out.phy> [--checksum N] [--mass F] [--surfaceprop S]
      从 SMD 读三角形 → 算凸包 → 写出 Source 引擎的 .phy 碰撞文件。
      SMD 的三角形顶点已在模型局部坐标，不做任何变换（与 studiomdl 一致）。
      --checksum  配对 .mdl 的 checksum（十进制或 0x 十六进制），默认 0
      --mass      $mass 等效总质量，默认 1（官方缺省；见 --automass 说明）
      --surfaceprop  表面材质，默认 default
      --concave   $concave：按**连通分量**拆成多个凸块（官方语义，
                  不是 VHACD 体分解 —— 见 `phy::decompose_connected_components`）
      --vhacd     VHACD 近似凸分解。**非官方语义**：保留凹口，
                  而官方 $concave 是填平。仅用于需要真凹形碰撞体的场合。
      --ragdoll   $collisionjoints：按**蒙皮权重骨骼**分组，每个骨骼一个 solid，
                  并把 boneIndex+1 写进 ledge 的 client_data。
                  `parent` 沿骨骼链**上溯**到第一个在碰撞列表里的祖先
                  （官方 FixParent）。

  mdlc qc2toml <model.qc> [--out <path.toml>]
      把 QC 脚本解析成 mdlc 的 TOML 描述文件（**只写文本，不编译**）。
      QC 里的 $include / $definevariable / $pushd 都会被展开，
      骨骼表与材质表会**读 SMD 补全**（与官方 BuildGlobalBonetable 一致）。
      用途：把 QC 项目迁移到 TOML，或人工核对解析结果。

  mdlc build-qc <model.qc> [--out <目录>] [--optimize-vtx]
      直接从 QC 编译 —— 等价于 `qc2toml` 之后再 `build`，
      但中间描述不落盘（与 studiomdl 的行为一致）。

  mdlc template
      打印一份带注释的最小 TOML 模板。

官方 studiomdl 兼容形态（用于直接替换 Crowbar 等宿主的编译器路径）：
  mdlc -game <gamedir> [-nop4] [-verbose] <model.qc>
      等价于 `build-qc <model.qc> --out <gamedir>\\models`
      —— 即官方的产物规则 `<gamedir> + models/ + $modelname`
      （见 write.cpp:1321-1331）。也可省略 -game：`mdlc <model.qc>`。
      官方单横线选项会被归一化接受；未实现的（如 -minlod）会警告后忽略。
";

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let rest: Vec<String> = argv.iter().skip(1).cloned().collect();

    // ---- 分流：官方兼容形态 vs mdlc 自有形态 ----
    //
    // 官方形态有两种（`studiomdl.cpp:6823` 的 `studiomdl [options] <file.qc>`）：
    // - 首参以 `-` 开头：`mdlc -game <gamedir> <x.qc>`（Crowbar 用）；
    // - 首参就是 `.qc`：`mdlc <x.qc>`（选项全默认）。
    //
    // 两者与 mdlc 自有形态无歧义：mdlc 的子命令都是**裸词**，且没有以
    // `.qc` 结尾的。`-h` / `--help` 例外 —— 仍归 mdlc（官方 `-h` 是
    // dump hboxes，归一化后是 `--h`，不会撞上 `--help`）。
    let first = rest.first().map(String::as_str);
    let official_form = match first {
        Some(f) if f.starts_with('-') => f != "-h" && f != "--help",
        Some(f) => f.to_ascii_lowercase().ends_with(".qc"),
        None => false,
    };

    if official_form {
        return run_official(&argv);
    }

    let matches = match mdlc::cli::build_cli().try_get_matches_from(&argv) {
        Ok(m) => m,
        Err(e) => return cli_exit(e),
    };

    match matches.subcommand() {
        None => {
            // 保持迁移前的契约：**无参数 = 用法错误**（usage 走 stderr、
            // 退出码 2），而不是 clap 默认的「打印帮助、退出 0」。
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        Some(("template", _)) => {
            print!("{}", mdlc::model::TEMPLATE_TOML);
            ExitCode::SUCCESS
        }
        Some(("build", m)) => {
            let (toml_path, out_root, cli_optimize_vtx) = mdlc::cli::build_args(m);
            let desc = match load_desc(&toml_path) {
                Ok(d) => d,
                Err(c) => return c,
            };
            let base = toml_path.parent().unwrap_or(Path::new("."));
            compile_and_write(&desc, base, &out_root, cli_optimize_vtx)
        }
        Some(("check", m)) => check(Path::new(m.get_one::<String>("toml").expect("required"))),
        Some(("phy", m)) => phy_cmd(m),
        Some(("qc2toml", m)) => qc2toml(m),
        Some(("build-qc", m)) => {
            let (qc, out, optimize_vtx) = mdlc::cli::build_qc_args(m);
            build_qc_from(&qc, &out, optimize_vtx)
        }
        Some(("vvd-info", m)) => vvd_info(m.get_one::<String>("file").expect("required")),
        Some(("vvd-roundtrip", m)) => {
            vvd_roundtrip(m.get_one::<String>("file").expect("required"))
        }
        Some((other, _)) => {
            eprintln!("错误：未知子命令 {other:?}\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// 把 clap 的错误映射成退出码。
///
/// - `--help` / `--version` 之类走 **stdout、退出码 0**（clap 的
///   `DisplayHelp` / `DisplayVersion`）；
/// - 其余用法错误走 **stderr、退出码 2** —— 与迁移前的**手写解析契约一致**，
///   既有脚本（`probe_*.js` / `cmp_*.js`）都按这个码判定。
fn cli_exit(e: clap::Error) -> ExitCode {
    let _ = e.print();
    if e.use_stderr() {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

/// 官方兼容模式：`mdlc -game <gamedir> [-nop4] [-verbose] <model.qc>`。
///
/// 语义与官方 `studiomdl` 对齐（见 [`mdlc::cli`] 模块文档）：
/// 产物写到 `<gamedir>\models\<$modelname>`。
fn run_official(argv: &[String]) -> ExitCode {
    let norm = mdlc::cli::normalize_official_args(argv);

    // 用户要求：未知选项**一律警告后继续**（不中断编译）。
    if !norm.unknown.is_empty() {
        eprintln!(
            "警告：忽略无法识别的选项 {}（mdlc 未实现或非官方选项）",
            norm.unknown.join(" ")
        );
    }

    let m = match mdlc::cli::build_official_cli().try_get_matches_from(&norm.argv) {
        Ok(m) => m,
        Err(e) => return cli_exit(e),
    };

    // 未实现的官方 flag 逐一告警 —— **静默忽略会产出与官方不同的模型**，
    // 那比拒绝更危险（例如 `-minlod` 会截断 LOD）。
    //
    // ⚠️ 布尔 flag 必须用 `get_flag` 判断，**不能用 `value_source`**：
    // clap 对未出现的 `SetTrue` 参数也返回 `Some(DefaultValue)`，
    // 用它会把「没传的 flag」也报成「已忽略」。
    for name in ["striplods", "definebones", "printbones"] {
        if m.get_flag(name) {
            eprintln!("警告：官方选项 -{name} 尚未实现，已忽略");
        }
    }
    for name in ["minlod", "t", "a"] {
        if m.value_source(name).is_some() {
            eprintln!("警告：官方选项 -{name} 尚未实现，已忽略");
        }
    }

    let Some(qc) = m.get_one::<String>("qc") else {
        eprintln!("错误：缺少 .qc 文件参数");
        eprintln!("用法：mdlc -game <gamedir> [选项] <model.qc>");
        return ExitCode::from(2);
    };

    let out_root = mdlc::cli::official_out_root(&m);
    build_qc_from(Path::new(qc), &out_root, m.get_flag("optimize_vtx"))
}

/// 读入并解析描述文件，打印全部校验错误。
fn load_desc(path: &Path) -> Result<ModelDesc, ExitCode> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("错误：读不到 {}：{e}", path.display());
            return Err(ExitCode::from(2));
        }
    };
    let desc = match ModelDesc::from_toml(&text) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("错误：{e}");
            return Err(ExitCode::from(2));
        }
    };
    if let Err(errs) = desc.validate() {
        eprintln!("描述文件有 {} 处错误：", errs.len());
        for e in &errs {
            eprintln!("  - {e}");
        }
        return Err(ExitCode::from(1));
    }
    Ok(desc)
}

fn check(p: &Path) -> ExitCode {
    let desc = match load_desc(p) {
        Ok(d) => d,
        Err(c) => return c,
    };
    // check 也读 SMD —— 「描述合法」但 SMD 缺失/材质对不上，同样是错误。
    let base = p.parent().unwrap_or(Path::new("."));
    match compile(&desc, base) {
        Ok(c) => {
            println!("{} 合法", p.display());
            println!(
                "  骨骼 {}，材质 {}，body part {}，顶点 {}，三角形 {}",
                desc.bones.len(),
                desc.materials.textures.len(),
                c.bodyparts.len(),
                c.total_vertices(),
                c.total_triangles()
            );
            for (bi, bp) in c.bodyparts.iter().enumerate() {
                for (mi, m) in bp.models.iter().enumerate() {
                    println!(
                        "    bodyparts[{bi}].models[{mi}] {} ← {}（{} mesh，{} 顶点）",
                        m.name,
                        m.smd_path.display(),
                        m.meshes.len(),
                        m.meshes.iter().map(|k| k.vertices.len()).sum::<usize>()
                    );
                }
            }
            if let Some((min, max)) = c.bounds() {
                println!("  包围盒 {min:?} .. {max:?}");
            }
            ExitCode::SUCCESS
        }
        Err(errs) => {
            eprintln!("{} 有 {} 处错误：", p.display(), errs.len());
            for e in &errs {
                eprintln!("  - {e}");
            }
            ExitCode::from(1)
        }
    }
}

/// 把 QC 解析成 `ModelDesc`，报错格式与 TOML 路径一致。
fn load_qc(path: &Path) -> Result<ModelDesc, ExitCode> {
    match mdlc::qc::parse_qc_file(path) {
        Ok(d) => Ok(d),
        Err(errs) => {
            eprintln!("{} 解析失败，{} 处错误：", path.display(), errs.len());
            for e in &errs {
                eprintln!("  - {e}");
            }
            Err(ExitCode::from(1))
        }
    }
}

/// `mdlc qc2toml <model.qc> [--out <path.toml>]`。
fn qc2toml(m: &clap::ArgMatches) -> ExitCode {
    let qc = PathBuf::from(m.get_one::<String>("qc").expect("required"));
    let out: Option<PathBuf> = m.get_one::<String>("out").map(PathBuf::from);
    let desc = match load_qc(&qc) {
        Ok(d) => d,
        Err(c) => return c,
    };
    // 与 `build` 一样先校验 —— 「解析成功但描述非法」也应当报出来。
    if let Err(errs) = desc.validate() {
        eprintln!("解析出的描述有 {} 处错误：", errs.len());
        for e in &errs {
            eprintln!("  - {e}");
        }
        return ExitCode::from(1);
    }
    let text = match desc.to_toml() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("错误：{e}");
            return ExitCode::from(1);
        }
    };
    let out = out.unwrap_or_else(|| qc.with_extension("toml"));
    if let Err(e) = std::fs::write(&out, &text) {
        eprintln!("错误：写不到 {}：{e}", out.display());
        return ExitCode::from(2);
    }
    println!("{} → {}", qc.display(), out.display());
    println!(
        "  骨骼 {}，材质 {}，body part {}，序列 {}，动画 {}",
        desc.bones.len(),
        desc.materials.textures.len(),
        desc.bodyparts.len(),
        desc.sequences.len(),
        desc.animations.len()
    );
    ExitCode::SUCCESS
}

/// `mdlc build-qc <model.qc> [--out <目录>] [--optimize-vtx]`，
/// 以及官方兼容模式（`mdlc -game <gamedir> <model.qc>`）共用的入口。
///
/// `out_root` 由调用方决定：
/// - mdlc 自有形态 → `--out`（默认当前目录）；
/// - 官方兼容形态 → `<gamedir>\models`（见 [`mdlc::cli::official_out_root`]）。
fn build_qc_from(qc: &Path, out_root: &Path, optimize_vtx: bool) -> ExitCode {
    let desc = match load_qc(qc) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let base = qc.parent().unwrap_or(Path::new("."));
    compile_and_write(&desc, base, out_root, optimize_vtx)
}

/// 把已解析的描述编译成四件套并落盘。
///
/// `build`（TOML）与 `build-qc`（QC）共用这条路径 —— 两者只在
/// **怎么得到 `ModelDesc`** 上不同，之后完全一致。这正是
/// `model.rs` 模块文档承诺的「写出器一行都不用改」。
fn compile_and_write(
    desc: &ModelDesc,
    base: &Path,
    out_root: &Path,
    cli_optimize_vtx: bool,
) -> ExitCode {
    let _t_compile = mdlc::prof::Span::new("main: compile()");
    let mut compiled = match compile(desc, base) {
        Ok(c) => c,
        Err(errs) => {
            eprintln!("编译失败，{} 处错误：", errs.len());
            for e in &errs {
                eprintln!("  - {e}");
            }
            return ExitCode::from(1);
        }
    };
    drop(_t_compile);

    // ---- 碰撞 SMD：**必须在 `write_mdl` 之前**解析 ----
    //
    // 因为 `physicsbone`（`mstudiobone_t` `+0xAC`）要写进**骨骼表**，
    // 而骨骼表是 `write_mdl` 产出的。碰撞几何来自**单独的 SMD**
    // （官方 `$collisionmodel` / `$collisionjoints`），所以要提前读。
    //
    // 解析结果留到下面构造 `.phy` 时**复用**，避免读两次。
    let collision_smd: Option<mdlc::smd::Smd> = match desc.physics.smd.as_deref() {
        None => None,
        Some(rel_smd) => {
            let p = mdlc::compile::resolve_smd_path(base, rel_smd);
            let text = match std::fs::read_to_string(&p) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("错误：读不到碰撞 SMD {}：{e}", p.display());
                    return ExitCode::from(2);
                }
            };
            match mdlc::smd::parse_smd(&text) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("错误：解析碰撞 SMD {} 失败：{e}", p.display());
                    return ExitCode::from(1);
                }
            }
        }
    };

    // `physicsbone`：只有**碰撞几何**能提供 solid→骨骼的映射。
    //
    // 官方（`collisionmodel.cpp:2141-2184`）在**跑了碰撞模型**之后填它，
    // 没跑就保持 0 —— 实测单 solid 2459/2459 全 0、多 solid 38/38 非平凡。
    //
    // ⚠️ 用**骨骼表**的父链（`compiled.desc.bones`）而不是碰撞 SMD 的
    // `nodes` —— 两者在静态道具 / 骨骼塌缩后可能不同，而落盘的
    // `physicsbone` 下标必须对**最终骨骼表**成立。
    if let Some(cs) = &collision_smd {
        let bi = compiled.desc.bone_index();
        let parents: Vec<i32> = compiled
            .desc
            .bones
            .iter()
            .map(|b| match b.parent.as_deref() {
                Some(p) => bi.get(p).map(|v| *v as i32).unwrap_or(-1),
                None => -1,
            })
            .collect();
        compiled.physics_bone =
            mdlc::phy::physics_bone_table(cs, compiled.desc.bones.len(), &parents);
    }

    let _t_mdl = mdlc::prof::Span::new("main: write_mdl + flatten_vertices");
    let out = match write_mdl(&compiled) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("错误：{e}");
            return ExitCode::from(1);
        }
    };

    // 顶点摊平 —— 必须与 write_mdl 用同一套 spans 编号。
    let flat = flatten_vertices(&compiled, &out.spans);
    drop(_t_mdl);
    let _t_vvd = mdlc::prof::Span::new("main: build_vvd + to_bytes");
    let vvd = match build_vvd(&compiled, out.checksum) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("错误：构造 VVD 失败：{e}");
            return ExitCode::from(1);
        }
    };
    let vvd_bytes = match vvd.to_bytes() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("错误：写出 VVD 失败：{e}");
            return ExitCode::from(1);
        }
    };
    // 写出后立刻自检 —— 偏移/长度对不上说明我们的写出器有 bug。
    if let Err(e) = check_invariants(&vvd, vvd_bytes.len()) {
        eprintln!("错误：写出的 VVD 不自洽（本实现的 bug）：{e}");
        return ExitCode::from(1);
    }
    drop(_t_vvd);

    // VTX：没有它模型在游戏里根本不渲染。
    //
    // 缓存优化默认关闭（保持与真 studiomdl 逐字节一致）；
    // TOML 的 `[model] optimize_vtx` 或命令行 `--optimize-vtx` 可打开，
    // 后者**覆盖**前者（命令行优先）。
    let vtx_opts = vtx_writer::VtxOptions {
        optimize_vertex_cache: desc.model.optimize_vtx || cli_optimize_vtx,
    };
    let _t_vtx = mdlc::prof::Span::new("main: write_vtx");
    let vtx = match write_vtx_with(&compiled, vtx_opts) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("错误：写出 VTX 失败：{e}");
            return ExitCode::from(1);
        }
    };
    drop(_t_vtx);
    if let Err(e) = vtx_writer::check_invariants(&vtx, &compiled) {
        eprintln!("错误：写出的 VTX 不自洽（本实现的 bug）：{e}");
        return ExitCode::from(1);
    }

    // 输出路径：out_root + 模型名（去掉 .mdl 换扩展名）。
    let rel = desc.output_name();
    let stem = rel.strip_suffix(".mdl").unwrap_or(&rel);
    let mdl_path = out_root.join(format!("{stem}.mdl"));
    let vvd_path = out_root.join(format!("{stem}.vvd"));
    let vtx_path = out_root.join(format!("{stem}.dx90.vtx"));

    // ---- PHY 碰撞模型（可选，由 `[model].collision_smd` 触发）----
    //
    // 官方 QC 的 `$collisionmodel <.smd>` **指向另一个 SMD**（通常
    // `*_phys.smd`），碰撞几何与渲染网格是两份数据 —— 碰撞网格通常刻意
    // 做得更简单、更接近凸体。所以本实现同样接受一个**单独的 SMD 路径**。
    //
    // `checksum` 用 `.mdl` 的那一个（配对令牌，引擎会校验）。
    //
    // ⚠️ 碰撞 SMD **已在上面（`write_mdl` 之前）解析过** —— 这里复用，
    // 因为 `physicsbone` 需要它，而那个字段要写进骨骼表。
    let phy_bytes: Option<Vec<u8>> = match &collision_smd {
        None => None,
        Some(smd) => {
            // `editparams.modelname` 用**不带扩展名**的模型名 ——
            // 与 `mdlc phy` 的 `file_stem` 行为一致。
            let phys_name = Path::new(stem)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unnamed".to_string());
            // `[physics].mass` 与 `.mdl` 头部 `+0x148` 是**同一个值**
            // （`write.cpp:2092` `phdr->mass = GetCollisionModelMass();`），
            // 所以两处都取 `effective_mass()`。
            //
            // `joints` 走 ragdoll（每骨骼一个 solid），
            // 否则走 prop 形态（单 solid，可选 `concave`）。
            let mass = desc.physics.effective_mass();
            // ---- prop 形态的 solid `name` = **碰撞 SMD 的 basename** ----
            //
            // 官方 `ProcessSingleBody`（`collisionmodel.cpp:1563-1571`）：
            // `Q_FileBase( pmodel->filename, tmp, ... )` —— `pmodel` 是
            // `$collisionmodel <smd>` 指向的那个 SMD，**与 `$modelname` 无关**。
            // 受控实验（`gen_phyname.js`）：`physign_geo.smd` → `"physign_geo"`。
            let collision_smd_name = desc
                .physics
                .smd
                .as_deref()
                .map(|p| {
                    std::path::Path::new(p)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string())
                })
                .unwrap_or_else(|| phys_name.clone());
            // ---- `surfaceprop` 从**模型头**取 ----
            //
            // `GetSurfaceProp`（`studiomdl.cpp:4974-5001`）先查
            // `$jointsurfaceprop`、再沿父链上溯，最后回落到
            // `s_pDefaultSurfaceProp`（= `$surfaceprop`）。
            // prop 形态的 solid 名是碰撞 SMD 的 basename ⟹ 必然查不到 ⟹
            // 恒等于 `$surfaceprop`。实测 **2498/2498 = 100%** 的 `.phy`
            // 的 surfaceprop 等于 `.mdl` 头部 `+0x134`。
            let surface_prop = desc.model.surface_prop.as_deref().unwrap_or("default");
            // ---- 官方 `ConvertToWorldSpace` 用的是**第一个序列第 0 帧** ----
            //
            // `collisionmodel.cpp:713` `CalcBoneTransforms( g_panimation[0], 0, ... )`。
            // 碰撞 SMD 自己的姿态**完全不参与** —— 这一点用受控实验定过案
            // （`docs/_probe/gen_phyrot.js`：三个几何相同、只有序列姿态不同的
            // 模型，`.phy` 互不相同而 `.vvd` 相同）。
            //
            // 没有序列时传 `None`，退化成用碰撞 SMD 自己的参考姿态。
            let pose_world = compiled
                .sequences
                .first()
                .and_then(|s| s.frames.first())
                .map(|f| mdlc::phy::sequence_pose_world(&compiled.desc.bones, f));
            let ident = mdlc::phy::PhyIdentity {
                model_name: &phys_name,
                collision_smd_name: &collision_smd_name,
                surface_prop,
            };
            let built = if desc.physics.joints {
                phy::build_ragdoll_phy_from_smd(
                    smd,
                    ident,
                    out.checksum as u32,
                    mass,
                    &desc.physics,
                )
            } else {
                phy::build_phy_from_smd(
                    smd,
                    ident,
                    out.checksum as u32,
                    mass,
                    &desc.physics,
                    pose_world.as_ref(),
                )
            };
            match built {
                Ok(b) => Some(b),
                Err(e) => {
                    eprintln!("错误：构造 PHY 失败：{e}");
                    return ExitCode::from(1);
                }
            }
        }
    };
    // 写出前自检 —— 13 条硬约束，失败说明是本实现的 bug。
    if let Some(b) = &phy_bytes
        && let Err(e) = phy::check_invariants(b)
    {
        eprintln!("错误：写出的 PHY 自检失败（本实现的 bug）：{e}");
        return ExitCode::from(1);
    }
    let phy_path = phy_bytes.as_ref().map(|_| out_root.join(format!("{stem}.phy")));

    // `$animblocksize` 的外置动画块。**文件名必须是 `models/<模型名>.ani`**
    // —— `.mdl` 头部 `+0x15C` 存的就是这个路径，写错名字引擎就找不到。
    //
    // 注意 `output_name()` 已经是 `models/` 前缀形式（如 `mymod/x.mdl`
    // 的 `models/mymod/x.mdl`），所以这里直接用 `{stem}.ani`。
    let ani_path = out.ani.as_ref().map(|_| out_root.join(format!("{stem}.ani")));
    for p in [&mdl_path, &vvd_path, &vtx_path]
        .into_iter()
        .chain(ani_path.iter())
        .chain(phy_path.iter())
    {
        if let Some(dir) = p.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("错误：建目录 {} 失败：{e}", dir.display());
            return ExitCode::from(2);
        }
    }
    for (path, bytes) in [
        (&mdl_path, &out.bytes),
        (&vvd_path, &vvd_bytes),
        (&vtx_path, &vtx.bytes),
    ] {
        if let Err(e) = std::fs::write(path, bytes) {
            eprintln!("错误：写 {} 失败：{e}", path.display());
            return ExitCode::from(2);
        }
    }
    if let (Some(path), Some(bytes)) = (&ani_path, &out.ani)
        && let Err(e) = std::fs::write(path, bytes)
    {
        eprintln!("错误：写 {} 失败：{e}", path.display());
        return ExitCode::from(2);
    }
    if let (Some(path), Some(bytes)) = (&phy_path, &phy_bytes)
        && let Err(e) = std::fs::write(path, bytes)
    {
        eprintln!("错误：写 {} 失败：{e}", path.display());
        return ExitCode::from(2);
    }

    println!("模型        {}", desc.model.name);
    println!("版本        {}", desc.version());
    println!("checksum    {}  （已写入各文件，配对一致）", out.checksum);
    println!();
    println!("骨骼        {}", desc.bones.len());
    println!("材质        {}", desc.materials.textures.len());
    println!("body part   {}", desc.bodyparts.len());
    println!("顶点        {}", flat.len());
    println!("三角形      {}", compiled.total_triangles());
    println!();
    println!("MDL         {:>10} 字节  {}", out.bytes.len(), mdl_path.display());
    println!("VVD         {:>10} 字节  {}", vvd_bytes.len(), vvd_path.display());
    println!("VTX         {:>10} 字节  {}", vtx.bytes.len(), vtx_path.display());
    if let (Some(p), Some(b)) = (&phy_path, &phy_bytes) {
        println!("PHY         {:>10} 字节  {}", b.len(), p.display());
    }
    println!();
    println!("**编译成功**（各文件布局自检均通过）");
    mdlc::prof::dump();
    ExitCode::SUCCESS
}

/// 把 IR 顶点转成 VVD —— **切线按真实三角形计算**，多 LOD 时生成 fixup 表。
///
/// 走哪条路径由 `compiled` 里有没有 LOD 数据决定：
/// - 全部 model 都是单 LOD → [`mdlc::lod::build_single_lod_vvd`]，
///   顶点顺序与旧的「摊平」实现完全一致（除切线块外逐字节相同）；
/// - 有 model 带 LOD → [`mdlc::lod::build_multi_lod_vvd`]，
///   顶点池跨 LOD 去重并按 LOD 归属排序，附带 fixup 表。
fn build_vvd(
    compiled: &mdlc::model::CompiledModelDesc,
    checksum: i32,
) -> Result<Vvd, mdlc::vvd::VvdError> {
    let multi = compiled
        .bodyparts
        .iter()
        .flat_map(|bp| &bp.models)
        .any(|m| m.lods.as_ref().is_some_and(|l| l.is_multi()));
    if multi {
        let (vvd, _layout) = mdlc::lod::build_multi_lod_vvd(compiled, checksum)?;
        Ok(vvd)
    } else {
        mdlc::lod::build_single_lod_vvd(compiled, checksum)
    }
}

fn vvd_info(path: &str) -> ExitCode {
    let buf = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("错误：读不到 {path}：{e}");
            return ExitCode::from(2);
        }
    };
    let v = match Vvd::parse(&buf) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("错误：解析失败：{e}");
            return ExitCode::from(2);
        }
    };
    let h = &v.header;
    println!("文件        {path}");
    println!("长度        {} 字节", buf.len());
    println!("checksum    {}  （与 .mdl/.vtx/.phy 配对，非内容哈希）", h.checksum);
    println!("numLODs     {}", h.num_lods);
    println!(
        "顶点数      {}   （numLODVertexes[0]，也是切线数）",
        h.vertex_count()
    );
    println!(
        "numLODVertexes  {:?}",
        &h.num_lod_vertexes[..(h.num_lods.max(0) as usize).clamp(1, 8)]
    );
    println!("numFixups   {}", h.num_fixups);
    println!("顶点块      @ {}  （{} × 48 = {} 字节）", h.vertex_data_start, h.vertex_count(), h.vertex_count() * 48);
    println!("切线块      @ {}  （{} × 16 = {} 字节）", h.tangent_data_start, h.vertex_count(), h.vertex_count() * 16);
    println!(
        "预期总长    {} 字节",
        h.tangent_data_start.max(0) as usize + h.vertex_count() * 16
    );

    match check_invariants(&v, buf.len()) {
        Ok(()) => println!("自洽性      通过（偏移、长度、块大小全部一致）"),
        Err(e) => {
            println!("自洽性      **失败**：{e}");
            return ExitCode::from(1);
        }
    }

    if let Some(v0) = v.vertices.first() {
        println!("首个顶点    pos={:?} nrm={:?} uv={:?}", v0.position, v0.normal, v0.tex_coord);
        println!(
            "            weight={:?} bone={:?} boneCount={}",
            v0.weight, v0.bone, v0.bone_count
        );
    }
    ExitCode::SUCCESS
}

fn vvd_roundtrip(path: &str) -> ExitCode {
    let buf = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("错误：读不到 {path}：{e}");
            return ExitCode::from(2);
        }
    };
    match mdlc::vvd_round_trip(&buf) {
        Ok(rt) => {
            println!("原文件      {} 字节", rt.original_len);
            println!("重写后      {} 字节", rt.rewritten_len);
            if rt.is_identical() {
                println!("结果        **逐字节完全相同**（{} 字节全部一致）", rt.original_len);
                ExitCode::SUCCESS
            } else {
                println!(
                    "结果        **不一致**：首个差异 @{}，共 {} 字节不同",
                    rt.first_diff.unwrap_or(0),
                    rt.diff_count
                );
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("错误：{e}");
            ExitCode::from(2)
        }
    }
}

/// `phy` 子命令的参数。
struct PhyArgs {
    input: PathBuf,
    output: PathBuf,
    checksum: u32,
    mass: f32,
    surface_prop: String,
    /// `$concave`（**官方语义**：连通分量分解）。
    concave: bool,
    /// VHACD 体分解（**非官方语义**，保留凹口）。
    vhacd: bool,
    ragdoll: bool,
}

/// 从 `phy` 的 clap 匹配结果取参数。
///
/// `--checksum` 同时接受十进制与 `0x` 十六进制（实测工作流里 checksum
/// 常常是从 `.mdl` 头里以十六进制抄出来的），所以不走 clap 的数值解析，
/// 而是取字符串后交给 [`parse_u32`]。
fn phy_args_from(m: &clap::ArgMatches) -> Result<PhyArgs, String> {
    let checksum = match m.get_one::<String>("checksum") {
        Some(v) => parse_u32(v).map_err(|e| format!("--checksum {v:?}：{e}"))?,
        None => 0,
    };
    // 官方缺省是 **1.0**（`CJointedModel` 构造函数 `m_totalMass = 1.0`，
    // 而 `ComputeMass()` 首句 `if (m_totalMass >= 0) return;` 直接返回）。
    let mass = match m.get_one::<String>("mass") {
        Some(v) => v
            .parse::<f32>()
            .map_err(|_| format!("--mass 不是合法浮点数：{v:?}"))?,
        None => 1.0,
    };
    // `--decompose` 是 `--vhacd` 的旧别名：保留以免破坏既有脚本，
    // 但**语义已明确**为 VHACD（非官方），不是 `$concave`。
    let vhacd = m.get_flag("vhacd") || m.get_flag("decompose");
    Ok(PhyArgs {
        input: PathBuf::from(m.get_one::<String>("input").expect("required")),
        output: PathBuf::from(m.get_one::<String>("output").expect("required")),
        checksum,
        mass,
        surface_prop: m
            .get_one::<String>("surfaceprop")
            .cloned()
            .unwrap_or_else(|| "default".to_string()),
        concave: m.get_flag("concave"),
        vhacd,
        ragdoll: m.get_flag("ragdoll"),
    })
}

/// 解析 `u32`，接受十进制与 `0x` 前缀的十六进制。
fn parse_u32(s: &str) -> Result<u32, String> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| e.to_string())
    } else if let Some(hex) = t.strip_prefix("-0x") {
        // 允许 `-0x1234`：`.mdl` 的 checksum 是有符号 int32，
        // 十六进制抄出来常常带负号，但 `.phy` 里按 u32 存。
        let v = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
        Ok(v.wrapping_neg())
    } else {
        t.parse::<u32>()
            .or_else(|_| t.parse::<i32>().map(|v| v as u32))
            .map_err(|e| e.to_string())
    }
}

/// `mdlc phy`：SMD 三角形 → 凸包 → `.phy`。
///
/// 存在的意义是**让 PHY 写出能脱离完整模型编译独立测试**：只要有任意一个
/// SMD 就能产出一个可被 `validate-final.js` 校验的碰撞文件。
fn phy_cmd(m: &clap::ArgMatches) -> ExitCode {
    let args = match phy_args_from(m) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("错误：{e}");
            return ExitCode::from(2);
        }
    };

    let text = match std::fs::read_to_string(&args.input) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("错误：读不到 {}：{e}", args.input.display());
            return ExitCode::from(2);
        }
    };
    let smd = match mdlc::smd::parse_smd(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("错误：解析 {} 失败：{e}", args.input.display());
            return ExitCode::from(1);
        }
    };
    if smd.triangles.is_empty() {
        eprintln!("错误：{} 里没有任何三角形", args.input.display());
        return ExitCode::from(1);
    }

    // SMD 的三角形顶点是**逐面独立**的（没有索引行，且 UV/法线接缝会让同一
    // 位置出现多次）。这里按"三个 float 的位模式完全相同"焊接成索引表 ——
    // 只做精确去重，不做 epsilon 合并，否则会改动坐标、进而让
    // `upper_limit_radius` / `box_sizes` 与写入的点对不上。
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut index_of: std::collections::HashMap<[u32; 3], u32> =
        std::collections::HashMap::new();
    // 焊接后的面表。
    //
    // ⚠️ 早先这里还顺手按**三角形行的 `parentBone`** 建了一张
    // `by_bone` 表给 `--ragdoll` 用 —— 那是**错的**（见
    // `phy::group_by_bone` 的说明：官方按 `links[].bone` 做**面级**归属）。
    // 现在 ragdoll 走 `phy::build_ragdoll_phy_from_smd`，这里不再建表。
    let mut faces: Vec<[u32; 3]> = Vec::with_capacity(smd.triangles.len());
    for t in &smd.triangles {
        let mut f = [0u32; 3];
        for (k, v) in t.vertices.iter().enumerate() {
            let p = v.position;
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            f[k] = *index_of.entry(key).or_insert_with(|| {
                vertices.push(p);
                (vertices.len() - 1) as u32
            });
        }
        // 焊接后可能出现退化三角形（三个下标不全不同）。凸包计算不关心面表，
        // 但 VHACD 会关心，所以这里直接剔除。
        if f[0] != f[1] && f[1] != f[2] && f[0] != f[2] {
            faces.push(f);
        }
    }
    if vertices.len() < 4 {
        eprintln!(
            "错误：焊接后只有 {} 个不同顶点，凸包至少要 4 个",
            vertices.len()
        );
        return ExitCode::from(1);
    }

    let model_name = args
        .input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());

    // 凸包计算：默认把整个网格当一个凸包；`--vhacd` 走 VHACD 体分解。
    //
    // 渲染网格通常既不凸也不闭合（有 T 型接缝、有重复边），所以**不能**
    // 直接把面表当凸包喂进去 —— 必须先算凸包。
    let hull_of = |verts: &[[f32; 3]], face_idx: &[usize]| -> Result<Vec<PhyHull>, String> {
        if args.vhacd {
            let sub: Vec<[u32; 3]> = face_idx.iter().map(|&i| faces[i]).collect();
            phy::decompose_concave(verts, &sub, 64, 16).map_err(|e| format!("凸分解失败：{e}"))
        } else {
            PhyHull::from_points(verts)
                .map(|h| vec![h])
                .map_err(|e| format!("算凸包失败：{e}"))
        }
    };

    // 每组凸块 + 对应的 solid 参数。
    let mut grouped: Vec<Vec<PhyHull>> = Vec::new();
    let mut solids: Vec<PhySolid> = Vec::new();

    if args.ragdoll {
        // `--ragdoll`：官方 `$collisionjoints` 语义 —— 按**骨骼**分组，
        // 每根骨骼一个 solid，`client_data = 骨骼下标 + 1`，
        // `parent` 经 `FixParent` 沿骨骼链上溯修正。
        //
        // 整个流程复用 `phy::build_ragdoll_phy_from_smd`（与 `build` 同一条路径），
        // 这里只是为了沿用 CLI 的 `--vhacd` / `--surfaceprop` 等选项，
        // 所以拿到字节后再自己拼参数重写一次。
        //
        // ⚠️ 早先这里按**三角形行的 `parentBone`** 分组，并把 solid 串成
        // 一条链当 `parent` —— 那是**错的**：官方按 `links[].bone`
        // （蒙皮权重）分组，`parent` 是骨骼名且要上溯。
        // `mdlc phy` 是独立子命令，没有 TOML 上下文 —— 用 CLI 选项
        // 拼一个 `Physics`（`$collisionjoints` 的默认形态）。
        let phys = mdlc::model::Physics {
            joints: true,
            mass: Some(args.mass),
            ..Default::default()
        };
        match phy::build_ragdoll_phy_from_smd(
            &smd,
            phy::PhyIdentity {
                model_name: &model_name,
                collision_smd_name: &model_name,
                surface_prop: &args.surface_prop,
            },
            args.checksum,
            args.mass,
            &phys,
        ) {
            Ok(b) => {
                // 自检后直接落盘，跳过下面通用的写出路径。
                if let Err(e) = phy::check_invariants(&b) {
                    eprintln!("错误：写出的 PHY 自检失败（本实现的 bug）：{e}");
                    return ExitCode::from(1);
                }
                if let Some(dir) = args.output.parent()
                    && !dir.as_os_str().is_empty()
                    && let Err(e) = std::fs::create_dir_all(dir)
                {
                    eprintln!("错误：建目录 {} 失败：{e}", dir.display());
                    return ExitCode::from(2);
                }
                if let Err(e) = std::fs::write(&args.output, &b) {
                    eprintln!("错误：写 {} 失败：{e}", args.output.display());
                    return ExitCode::from(2);
                }
                let layout = phy::check_invariants(&b).expect("刚查过");
                println!("输入        {}", args.input.display());
                println!("输出        {}", args.output.display());
                println!("checksum    0x{:08x}", args.checksum);
                println!("三角形      {}（焊接后 {} 个不同顶点）", smd.triangles.len(), vertices.len());
                println!("solid       {}（ragdoll，每骨骼一个）", layout.solid_count);
                println!();
                println!("文件        {:>8} 字节", layout.file_size);
                println!("text 段     {:>8} 字节  @ {}", layout.text_size, layout.solids_end);
                println!();
                println!("**写出成功**（13 条硬约束自检全部通过）");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("错误：ragdoll 构造失败：{e}");
                return ExitCode::from(1);
            }
        }
    } else if args.concave {
        // `--concave`：**官方 `$concave` 语义** —— 按连通分量拆成多个凸块。
        //
        // ⚠️ 与 `--vhacd` 是**两种不同算法**：官方对连通的凹体给出的是
        // **整个网格的凸包**（凹处被填平），VHACD 保留凹口。
        // 见 `phy::decompose_connected_components`。
        //
        // 它需要**法线**（焊接判据是「位置相同 **且** 法线夹角 < 2°」），
        // 所以直接吃 SMD，而不是上面那份按位置焊接过的 `vertices`/`faces`。
        //
        // ⚠️ 第二个参数是**序列姿态**（官方 `ConvertToWorldSpace` 用
        // `g_panimation[0]`）。`mdlc phy` 是独立子命令、手里只有碰撞 SMD，
        // 没有序列上下文，所以传 `None` —— 此时退化成用碰撞 SMD 自己的
        // 参考姿态。走 `mdlc build` 的路径会正确传入序列姿态。
        let hs = match phy::decompose_connected_components(&smd, None) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("错误：$concave 分解失败：{e}");
                return ExitCode::from(1);
            }
        };
        grouped.push(hs);
        // `name` = **碰撞 SMD 的 basename**（官方 `Q_FileBase`）。
        // `mdlc phy <in.smd>` 里那个 `<in.smd>` 就是碰撞 SMD。
        solids.push(PhySolid::prop(&model_name));
    } else {
        let all_idx: Vec<usize> = (0..faces.len()).collect();
        let hs = match hull_of(&vertices, &all_idx) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("错误：{e}");
                return ExitCode::from(1);
            }
        };
        grouped.push(hs);
        // 同上：`name` 取碰撞 SMD 的 basename。
        solids.push(PhySolid::prop(&model_name));
    }

    let mut params = PhyParams::new(&model_name, args.checksum);
    params.total_mass = args.mass;
    params.surface_prop = &args.surface_prop;
    // 只有真的走了 `$concave` 才写 `concave "1"`，与实测的 studiomdl 行为一致
    // （VHACD 也写 —— 它同样产出了多个凸块）。
    params.concave = args.concave || args.vhacd;

    // 凸分解出来的多个凸块属于**同一个 solid**（共用一份点数组与一棵 ledgetree），
    // 所以走 `write_phy_multi` 而不是 `write_phy` —— 后者的契约是
    // 「每个 solid 一个凸块」。
    let bytes = match phy::write_phy_multi(&grouped, &solids, &params) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("错误：写出 PHY 失败：{e}");
            return ExitCode::from(1);
        }
    };
    let hull_count: usize = grouped.iter().map(|g| g.len()).sum();

    if let Some(dir) = args.output.parent()
        && !dir.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("错误：建目录 {} 失败：{e}", dir.display());
        return ExitCode::from(2);
    }
    if let Err(e) = std::fs::write(&args.output, &bytes) {
        eprintln!("错误：写 {} 失败：{e}", args.output.display());
        return ExitCode::from(2);
    }

    // 写出后立刻重读一遍做自检 —— `write_phy` 内部已经查过，这里再查一次
    // 是为了覆盖"落盘"这一段（截断、权限、路径写错都能被它抓到）。
    match phy::check_invariants(&bytes) {
        Ok(layout) => {
            println!("输入        {}", args.input.display());
            println!("输出        {}", args.output.display());
            println!("checksum    0x{:08x}", args.checksum);
            println!(
                "三角形      {}（焊接后 {} 个不同顶点）",
                smd.triangles.len(),
                vertices.len()
            );
            println!("solid       {}（共 {} 个凸块）", solids.len(), hull_count);
            println!();
            println!("文件        {:>8} 字节", layout.file_size);
            println!("solid       {}", layout.solid_count);
            println!("text 段     {:>8} 字节  @ {}", layout.text_size, layout.solids_end);
            for (i, ((surf, nodes), ledge)) in layout
                .surface_sizes
                .iter()
                .zip(&layout.node_counts)
                .zip(&layout.ledge_region_sizes)
                .enumerate()
            {
                println!(
                    "  solid[{i}]  surfaceSize={surf}  ledge 区={ledge}  树节点={nodes}"
                );
            }
            println!();
            println!("**写出成功**（13 条硬约束自检全部通过）");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("错误：写出的 PHY 自检失败（本实现的 bug）：{e}");
            ExitCode::from(1)
        }
    }
}
