//! 探明官方 FBX vertanim 的排列规则：把「焊接后的顶点号」映射回 FBX 的**控制点号**
//! （shape key 的 `offset_vertices` 用的就是控制点号）。
//!
//! 用法：`cargo run --release --example probe_fbx_weldmap -- <file.fbx>`
//!
//! 焊接口径与 `src/compile.rs::build_meshes` 一致：按三角形顺序遍历每个角，
//! 以 (position, normal, uv) 三元组做「首次出现」去重。

use std::path::PathBuf;

#[derive(PartialEq)]
struct Key([f32; 3], [f32; 3], [f32; 2]);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx_weldmap <file.fbx> [...]");
        std::process::exit(2);
    }
    for a in args {
        let path = PathBuf::from(&a);
        println!("==== {} ====", path.display());
        let bytes = std::fs::read(&path).expect("读文件");
        let scene = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("解析");

        for ni in 0..scene.nodes.len() {
            let n = &scene.nodes[ni];
            if n.is_root {
                continue;
            }
            let Some(mesh) = n.mesh.as_ref() else { continue };
            println!("节点[{ni}] {:?} verts={}", n.element.name, mesh.num_vertices);
            let mut welds: Vec<Key> = Vec::new();
            // 焊接顶点号 → 控制点号（可能多个角共享同一控制点）
            let mut cp_of: Vec<Vec<u32>> = Vec::new();
            let mut corner_cp: Vec<u32> = Vec::new();
            let mut buf = vec![0u32; mesh.max_face_triangles * 3];
            for fi in 0..mesh.num_faces {
                let face = mesh.faces[fi];
                let nt = mesh.triangulate_face(&mut buf, face);
                for &corner_raw in buf.iter().take(nt as usize * 3) {
                    let corner = corner_raw as usize;
                    let p = mesh.vertex_position[corner];
                    let nrm = if mesh.vertex_normal.exists {
                        mesh.vertex_normal[corner]
                    } else {
                        ufbx::Vec3 {
                            x: 0.0,
                            y: 0.0,
                            z: 1.0,
                        }
                    };
                    let uv = if mesh.vertex_uv.exists {
                        let u = mesh.vertex_uv[corner];
                        ufbx::Vec2 { x: u.x, y: 1.0 - u.y }
                    } else {
                        ufbx::Vec2 { x: 0.0, y: 0.0 }
                    };
                    let key = Key(
                        [p.x as f32, p.y as f32, p.z as f32],
                        [nrm.x as f32, nrm.y as f32, nrm.z as f32],
                        [uv.x as f32, uv.y as f32],
                    );
                    let w = match welds.iter().position(|k| *k == key) {
                        Some(i) => i,
                        None => {
                            welds.push(key);
                            cp_of.push(Vec::new());
                            welds.len() - 1
                        }
                    };
                    let cp = mesh.vertex_indices[corner];
                    if !cp_of[w].contains(&cp) {
                        cp_of[w].push(cp);
                    }
                    corner_cp.push(cp);
                }
            }
            println!("焊接顶点数 = {}", welds.len());
            for (w, cps) in cp_of.iter().enumerate() {
                println!("  w[{w:2}] → 控制点 {cps:?}");
            }
            // 反过来：控制点 → 焊接顶点列表（升序）
            let mut by_cp: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
            for (w, cps) in cp_of.iter().enumerate() {
                for &cp in cps {
                    by_cp.entry(cp).or_default().push(w);
                }
            }
            println!("── 控制点 → 焊接顶点（升序）");
            for (cp, ws) in &by_cp {
                println!("  cp[{cp:2}] → {ws:?}");
            }
        }
    }
}
