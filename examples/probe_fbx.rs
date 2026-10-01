//! 冒烟：把 `src/fbx.rs` 的输出与原型 `ufbxraw` 的实测值逐条对照。
//!
//! 用法：`cargo run --release --example probe_fbx -- <file.fbx> [...]`

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx <file.fbx> [...]");
        std::process::exit(2);
    }
    for a in args {
        let path = PathBuf::from(&a);
        println!("==== {} ====", path.display());
        let opts = mdlc::fbx::FbxOpts::default();
        match mdlc::fbx::read(&path, "probe", &opts) {
            Ok(g) => {
                println!(
                    "[nodes] 共 {}  anim_stacks={:?}",
                    g.smd.nodes.len(),
                    g.anim_stacks
                );
                for n in &g.smd.nodes {
                    println!("  [{}] parent={} {:?}", n.index, n.parent, n.name);
                }
                println!("[triangles] 共 {}", g.smd.triangles.len());
                let mats = g.smd.materials_in_order();
                println!("[materials] {mats:?}");
                if let Some(f) = g.smd.reference_frame() {
                    println!("[refpose]");
                    for p in &f.poses {
                        let name = g
                            .smd
                            .nodes
                            .get(p.bone.max(0) as usize)
                            .map_or("<越界>", |n| n.name.as_str());
                        println!(
                            "  [{:3}] {:<40} pos=[{:.4},{:.4},{:.4}] rot=[{:.4},{:.4},{:.4}]",
                            p.bone,
                            name,
                            p.position[0],
                            p.position[1],
                            p.position[2],
                            p.rotation[0],
                            p.rotation[1],
                            p.rotation[2]
                        );
                    }
                }
                for (i, t) in g.smd.triangles.iter().take(2).enumerate() {
                    println!("[tri {i}] material={:?}", t.material);
                    for v in &t.vertices {
                        println!(
                            "  parent_bone={} pos=[{:.4},{:.4},{:.4}] nrm=[{:.4},{:.4},{:.4}] uv=[{:.4},{:.4}] links={:?}",
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
                println!("[shape_keys] 共 {}", g.shape_keys.len());
                for (i, k) in g.shape_keys.iter().enumerate() {
                    println!(
                        "  [{}] name={:?} offsets={} frame={}",
                        i,
                        k.name,
                        k.vertex_index.len(),
                        mdlc::fbx::FbxGeometry::shape_key_frame(i)
                    );
                }
                println!(
                    "[untextured] {:?}  [merged] {:?}",
                    g.untextured_meshes, g.merged_meshes
                );
                match mdlc::fbx::read_frames(&path, "probe", None, mdlc::fbx::DEFAULT_FPS, &opts) {
                    Ok(s) => {
                        println!("[frames] 共 {}", s.frames.len());
                        if let Some(f) = s.frames.first() {
                            for p in f.poses.iter().take(4) {
                                println!(
                                    "  [{:3}] pos=[{:.4},{:.4},{:.4}] rot=[{:.4},{:.4},{:.4}]",
                                    p.bone, p.position[0], p.position[1], p.position[2],
                                    p.rotation[0], p.rotation[1], p.rotation[2]
                                );
                            }
                        }
                    }
                    Err(e) => println!("[frames] 错误：{e}"),
                }
            }
            Err(e) => println!("错误：{e}"),
        }
    }
}
