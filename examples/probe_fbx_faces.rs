//! 探明官方 FBX vertanim 的排列规则（续）：逐面打印每个角的控制点号 / 焊接号 / 位置 / UV。
//!
//! 用法：`cargo run --release --example probe_fbx_faces -- <file.fbx>`

use std::path::PathBuf;

#[derive(PartialEq, Clone, Copy, Debug)]
struct Key([f32; 3], [f32; 3], [f32; 2]);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx_faces <file.fbx> [...]");
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
            println!(
                "节点[{ni}] {:?} verts={} faces={} tris={}",
                n.element.name, mesh.num_vertices, mesh.num_faces, mesh.num_triangles
            );
            let mut welds: Vec<Key> = Vec::new();
            let mut buf = vec![0u32; mesh.max_face_triangles * 3];
            // 按「面」而不是「三角形」打印
            for fi in 0..mesh.num_faces {
                let face = mesh.faces[fi];
                let nt = mesh.triangulate_face(&mut buf, face);
                print!("  face[{fi:2}] corners={} :", face.num_indices);
                for &corner_raw in buf.iter().take(nt as usize * 3) {
                    let corner = corner_raw as usize;
                    let p = mesh.vertex_position[corner];
                    let nrm = if mesh.vertex_normal.exists {
                        mesh.vertex_normal[corner]
                    } else {
                        ufbx::Vec3 { x: 0.0, y: 0.0, z: 1.0 }
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
                            welds.len() - 1
                        }
                    };
                    let cp = mesh.vertex_indices[corner];
                    print!(" w{w}(cp{cp})");
                }
                println!();
            }
            println!("  焊接顶点数 = {}", welds.len());
        }
    }
}
