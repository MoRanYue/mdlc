//! 把焊接顶点号 → (控制点, 位置, 法线, UV) 全表打印，用于反推官方 vertanim 排列规则。
//!
//! 用法：`cargo run --release --example probe_fbx_welds -- <file.fbx>`

use std::path::PathBuf;

#[derive(PartialEq, Clone, Copy, Debug)]
struct Key([f32; 3], [f32; 3], [f32; 2]);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx_welds <file.fbx> [...]");
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
            let mut welds: Vec<Key> = Vec::new();
            let mut cp_of: Vec<u32> = Vec::new();
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
                            cp_of.push(mesh.vertex_indices[corner]);
                            welds.len() - 1
                        }
                    };
                    let _ = w;
                }
            }
            println!("节点[{ni}] {:?} 焊接顶点 {} 个", n.element.name, welds.len());
            for (w, k) in welds.iter().enumerate() {
                println!(
                    "  w[{w:2}] cp={:2} pos=({:7.3},{:7.3},{:7.3}) nrm=({:5.2},{:5.2},{:5.2}) uv=({:.3},{:.3})",
                    cp_of[w], k.0[0], k.0[1], k.0[2], k.1[0], k.1[1], k.1[2], k.2[0], k.2[1]
                );
            }
            println!("  控制点序列 = {:?}", cp_of);
        }
    }
}
