//! 冒烟：把 `src/dmx.rs` 读出的东西打出来，人工对一遍。
//!
//! 用法：`cargo run --example probe_dmx -- <file.dmx> [...] [--tree] [--smd]`
//!
//! 判据是「与我用 Blender 导出的内容对得上」，不是「跑通不 panic」：
//! 骨骼数 / 层级、材质名、每网格的顶点数、三角形数、权重链数、被引用的最大骨骼下标。
//! 三种 BST 预设（L4D2 binary 4/15、GMod 2/1、SFM 5/18）都要过。
//!
//! `--smd` 打印转成 SMD 口径之后的结果（节点表、骨骼姿态、三角形与材质），
//! 用来和同一份场景导出的 `.smd` 对照。

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tree = args.iter().any(|a| a == "--tree");
    let smd = args.iter().any(|a| a == "--smd");
    let files: Vec<String> = args.into_iter().filter(|a| !a.starts_with("--")).collect();
    if files.is_empty() {
        eprintln!("用法：probe_dmx <file.dmx> [...] [--tree] [--smd]");
        std::process::exit(2);
    }

    let mut failed = 0;
    for arg in files {
        let path = PathBuf::from(&arg);
        println!("==== {} ====", path.display());
        match mdlc::dmx::load(&path) {
            Ok(model) => {
                report(&model, tree);
                if smd {
                    match mdlc::dmx::read(&path, &arg) {
                        Ok(geometry) => report_smd(&geometry),
                        Err(e) => {
                            println!("[转 SMD 失败] {e}");
                            failed += 1;
                        }
                    }
                }
            }
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
                "  {:width$}{i} {:?} joint={} parent={:?} pos=[{:.3}, {:.3}, {:.3}] quat=[{:.4}, {:.4}, {:.4}, {:.4}]",
                "",
                bone.name,
                bone.is_joint,
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
        let materials: Vec<Option<usize>> = mesh.triangles.iter().map(|t| t.material).collect();
        let distinct_materials: std::collections::BTreeSet<Option<usize>> =
            materials.iter().copied().collect();
        let corners = mesh.triangles.len() * 3;
        let uv_ok = mesh
            .triangles
            .iter()
            .flat_map(|t| t.corners.iter())
            .all(|c| c.uv[0].is_finite() && c.uv[1].is_finite());
        println!(
            "  {:?} 顶点={} 三角形={} 角={} 材质={:?} 有权重的顶点={}/{} UV 有效={uv_ok}",
            mesh.name,
            mesh.positions.len(),
            mesh.triangles.len(),
            corners,
            distinct_materials,
            weighted,
            mesh.weights.len()
        );
        if let Some(t) = mesh.triangles.first() {
            let inside = mesh
                .triangles
                .iter()
                .flat_map(|t| t.corners.iter())
                .all(|c| (c.position as usize) < mesh.positions.len());
            let p: Vec<u32> = t.corners.iter().map(|c| c.position).collect();
            println!("    首三角形={p:?} 下标全在范围内={inside}");
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

fn report_smd(geometry: &mdlc::dmx::DmxGeometry) {
    let smd = &geometry.smd;
    println!("[SMD] nodes={} frames={} triangles={}", smd.nodes.len(), smd.frames.len(), smd.triangles.len());
    for node in &smd.nodes {
        println!("  node {} {:?} parent={}", node.index, node.name, node.parent);
    }
    for frame in &smd.frames {
        for pose in &frame.poses {
            println!(
                "  pose bone={} pos=[{:.6}, {:.6}, {:.6}] rot=[{:.6}, {:.6}, {:.6}]",
                pose.bone,
                pose.position[0],
                pose.position[1],
                pose.position[2],
                pose.rotation[0],
                pose.rotation[1],
                pose.rotation[2]
            );
        }
    }
    let materials: std::collections::BTreeSet<&str> =
        smd.triangles.iter().map(|t| t.material.as_str()).collect();
    println!("  materials={materials:?}");
    for (i, t) in smd.triangles.iter().take(2).enumerate() {
        println!("  tri {i} material={:?}", t.material);
        for v in &t.vertices {
            println!(
                "    bone={} pos=[{:.6}, {:.6}, {:.6}] nrm=[{:.6}, {:.6}, {:.6}] uv=[{:.6}, {:.6}] links={:?}",
                v.parent_bone,
                v.position[0],
                v.position[1],
                v.position[2],
                v.normal[0],
                v.normal[1],
                v.normal[2],
                v.uv[0],
                v.uv[1],
                v.links
                    .iter()
                    .map(|l| (l.bone, l.weight))
                    .collect::<Vec<_>>()
            );
        }
    }
    if !geometry.warnings.is_empty() {
        println!("[SMD warnings]");
        for w in &geometry.warnings {
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
