//! 冒烟：把 `src/dmx.rs` 读出的东西打出来，人工对一遍。
//!
//! 用法：`cargo run --example probe_dmx -- <file.dmx> [--tree]`
//!
//! 判据是「与我用 Blender 导出的内容对得上」，不是「跑通不 panic」：
//! 骨骼数 / 层级、材质名、每网格的顶点数、三角形数、权重链数、被引用的最大骨骼下标。
//! 三种 BST 预设（L4D2 binary 4/15、GMod 2/1、SFM 5/18）都要过。

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tree = args.iter().any(|a| a == "--tree");
    let files: Vec<String> = args.into_iter().filter(|a| !a.starts_with("--")).collect();
    if files.is_empty() {
        eprintln!("用法：probe_dmx <file.dmx> [...] [--tree]");
        std::process::exit(2);
    }

    let mut failed = 0;
    for arg in files {
        let path = PathBuf::from(&arg);
        println!("==== {} ====", path.display());
        match mdlc::dmx::load(&path) {
            Ok(model) => report(&model, tree),
            Err(e) => {
                println!("[错误] {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
}

fn report(model: &mdlc::dmx::DmxModel, tree: bool) {
    println!("[header] format_version={}", model.format_version);
    println!("[bones] {}", model.bones.len());
    if tree {
        for (i, bone) in model.bones.iter().enumerate() {
            let depth = depth_of(model, i);
            println!(
                "  {:width$}{i} {:?} parent={:?} pos=[{:.3}, {:.3}, {:.3}] quat=[{:.4}, {:.4}, {:.4}, {:.4}]",
                "",
                bone.name,
                bone.parent,
                bone.position[0],
                bone.position[1],
                bone.position[2],
                bone.orientation[0],
                bone.orientation[1],
                bone.orientation[2],
                bone.orientation[3],
                width = depth * 2
            );
        }
    } else {
        let roots = model.bones.iter().filter(|b| b.parent.is_none()).count();
        let names: Vec<&str> = model
            .bones
            .iter()
            .take(6)
            .map(|b| b.name.as_str())
            .collect();
        println!("  根骨骼 {roots} 根，前几个：{names:?}");
    }

    println!("[materials] {}", model.materials.len());
    for (i, m) in model.materials.iter().enumerate() {
        println!("  {i} {:?} → {:?}", m.name, m.material);
    }

    println!("[meshes] {}", model.meshes.len());
    let mut total_positions = 0usize;
    let mut total_triangles = 0usize;
    let mut max_joint = None::<u32>;
    let mut max_chains = 0usize;
    for mesh in &model.meshes {
        total_positions += mesh.positions.len();
        total_triangles += mesh.triangles.len();
        for chain in &mesh.weights {
            max_chains = max_chains.max(chain.len());
            for (joint, _) in chain {
                max_joint = Some(max_joint.map_or(*joint, |m: u32| m.max(*joint)));
            }
        }
        let weighted = mesh.weights.iter().filter(|c| !c.is_empty()).count();
        println!(
            "  {:?} 材质={:?} 顶点={} 三角形={} 法线={} UV={} 有权重的顶点={}/{}",
            mesh.name,
            mesh.material,
            mesh.positions.len(),
            mesh.triangles.len(),
            mesh.normals.len(),
            mesh.uvs.len(),
            weighted,
            mesh.weights.len()
        );
        if let Some(&[a, b, c]) = mesh.triangles.first() {
            let inside = mesh
                .triangles
                .iter()
                .flatten()
                .all(|&i| (i as usize) < mesh.positions.len());
            println!("    首三角形=[{a}, {b}, {c}] 下标全在范围内={inside}");
        }
    }
    println!(
        "[合计] 顶点 {total_positions}  三角形 {total_triangles}  每顶点最多权重链 {max_chains}  被引用的最大骨骼下标 {max_joint:?}"
    );

    if !model.warnings.is_empty() {
        println!("[warnings] {}", model.warnings.len());
        for w in &model.warnings {
            println!("  - {w}");
        }
    }
}

fn depth_of(model: &mdlc::dmx::DmxModel, mut index: usize) -> usize {
    let mut depth = 0;
    while let Some(parent) = model.bones[index].parent {
        depth += 1;
        index = parent;
        if depth > model.bones.len() {
            break;
        }
    }
    depth
}
