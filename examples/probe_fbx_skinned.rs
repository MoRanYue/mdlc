//! 临时取证：把 FBX 里**蒙皮网格**的原始顶点范围、`geometry_to_world`、以及每个
//! 簇的绑定矩阵（`bind_to_world`）打出来，用来判定「蒙皮网格的顶点到底在哪个空间」。
//!
//! 用法：`cargo run --release --example probe_fbx_skinned -- <file.fbx> [名字过滤...]`

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx_skinned <file.fbx> [名字过滤...]");
        std::process::exit(2);
    }
    let path = PathBuf::from(&args[0]);
    let filters = &args[1..];
    let hit = |n: &str| filters.is_empty() || filters.iter().any(|f| n.contains(f.as_str()));

    let bytes = std::fs::read(&path).expect("读不到");
    let scene = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("解析失败");

    println!("==== {} ====", path.display());
    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        let name = n.element.name.to_string();
        let Some(m) = n.mesh.as_ref() else { continue };
        if !hit(&name) {
            continue;
        }
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for v in m.vertex_position.values.iter() {
            for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                if c < min[k] {
                    min[k] = c;
                }
                if c > max[k] {
                    max[k] = c;
                }
            }
        }
        let g = &n.geometry_to_world;
        let w = &n.node_to_world;
        println!("  mesh {name:?}  verts={} skinned_is_local={}", m.num_vertices, m.skinned_is_local);
        println!(
            "     raw   min=({:.3},{:.3},{:.3}) max=({:.3},{:.3},{:.3})",
            min[0], min[1], min[2], max[0], max[1], max[2]
        );
        println!(
            "     node_to_world      T=({:.4},{:.4},{:.4}) basis=({:.4},{:.4},{:.4} | {:.4},{:.4},{:.4} | {:.4},{:.4},{:.4})",
            w.m03, w.m13, w.m23, w.m00, w.m10, w.m20, w.m01, w.m11, w.m21, w.m02, w.m12, w.m22
        );
        println!(
            "     geometry_to_world  T=({:.4},{:.4},{:.4}) basis=({:.4},{:.4},{:.4} | {:.4},{:.4},{:.4} | {:.4},{:.4},{:.4})",
            g.m03, g.m13, g.m23, g.m00, g.m10, g.m20, g.m01, g.m11, g.m21, g.m02, g.m12, g.m22
        );
        let gt = n.geometry_transform;
        println!(
            "     geometry_transform T=({:.4},{:.4},{:.4}) S=({:.4},{:.4},{:.4})",
            gt.translation.x, gt.translation.y, gt.translation.z, gt.scale.x, gt.scale.y, gt.scale.z
        );
        if !m.skinned_position.values.is_empty() {
            let mut smin = [f64::INFINITY; 3];
            let mut smax = [f64::NEG_INFINITY; 3];
            for v in m.skinned_position.values.iter() {
                for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                    if c < smin[k] {
                        smin[k] = c;
                    }
                    if c > smax[k] {
                        smax[k] = c;
                    }
                }
            }
            println!(
                "     skinned min=({:.3},{:.3},{:.3}) max=({:.3},{:.3},{:.3})",
                smin[0], smin[1], smin[2], smax[0], smax[1], smax[2]
            );
        }
        for d in 0..m.skin_deformers.len() {
            let sd = &m.skin_deformers[d];
            for c in 0..sd.clusters.len() {
                let cl = &sd.clusters[c];
                let b = &cl.bind_to_world;
                println!(
                    "     cluster bone={:?} num_weights={} bind_to_world T=({:.4},{:.4},{:.4}) basis=({:.3},{:.3},{:.3} | {:.3},{:.3},{:.3} | {:.3},{:.3},{:.3})",
                    cl.bone_node.as_ref().map(|b| b.element.name.to_string()),
                    cl.num_weights,
                    b.m03,
                    b.m13,
                    b.m23,
                    b.m00,
                    b.m10,
                    b.m20,
                    b.m01,
                    b.m11,
                    b.m21,
                    b.m02,
                    b.m12,
                    b.m22
                );
            }
        }
    }

    println!("---- 骨骼节点（无网格）----");
    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        let name = n.element.name.to_string();
        if n.mesh.is_some() || !hit(&name) {
            continue;
        }
        let w = &n.node_to_world;
        println!(
            "  bone {name:?} localT=({:.4},{:.4},{:.4}) worldT=({:.4},{:.4},{:.4})",
            n.local_transform.translation.x,
            n.local_transform.translation.y,
            n.local_transform.translation.z,
            w.m03,
            w.m13,
            w.m23
        );
    }
}
