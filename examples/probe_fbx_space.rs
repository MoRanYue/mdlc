//! 空间取证：把 FBX 节点的局部/世界变换、网格的 geometry 变换与蒙皮位置
//! 全部打出来，用来判定「官方把网格顶点放在哪个空间」。
//!
//! 用法：`cargo run --example probe_fbx_space -- <file.fbx> [...]`

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx_space <file.fbx> [...]");
        std::process::exit(2);
    }
    for a in args {
        let path = PathBuf::from(&a);
        println!("==== {} ====", path.display());
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                println!("读不到：{e}");
                continue;
            }
        };
        let scene = match ufbx::load_memory(&bytes, ufbx::LoadOpts::default()) {
            Ok(s) => s,
            Err(e) => {
                println!("解析失败：{e:?}");
                continue;
            }
        };
        let ax = scene.settings.axes;
        println!(
            "[axes] up={:?} right={:?} front={:?}",
            ax.up, ax.right, ax.front
        );
        println!(
            "[settings] unit_meters={} fps={}",
            scene.settings.unit_meters, scene.settings.frames_per_second
        );
        for i in 0..scene.nodes.len() {
            let n = &scene.nodes[i];
            let t = n.local_transform;
            println!(
                "  node[{i}] {:?} is_root={} parent={:?} mesh={} localT=({:.3},{:.3},{:.3}) localS=({:.3},{:.3},{:.3}) localQ=({:.4},{:.4},{:.4},{:.4})",
                n.element.name,
                n.is_root,
                n.parent.as_ref().map(|p| p.element.name.to_string()),
                n.mesh.is_some(),
                t.translation.x,
                t.translation.y,
                t.translation.z,
                t.scale.x,
                t.scale.y,
                t.scale.z,
                t.rotation.x,
                t.rotation.y,
                t.rotation.z,
                t.rotation.w
            );
            let w = n.node_to_world;
            println!(
                "        node_to_world T=({:.4},{:.4},{:.4}) basis=({:.4},{:.4},{:.4} | {:.4},{:.4},{:.4} | {:.4},{:.4},{:.4})",
                w.m03, w.m13, w.m23, w.m00, w.m10, w.m20, w.m01, w.m11, w.m21, w.m02, w.m12, w.m22
            );
            if let Some(m) = n.mesh.as_ref() {
                let gw = n.geometry_to_world;
                println!(
                    "        mesh: geometry_to_world T=({:.4},{:.4},{:.4}) basis=({:.4},{:.4},{:.4} | {:.4},{:.4},{:.4} | {:.4},{:.4},{:.4})",
                    gw.m03, gw.m13, gw.m23, gw.m00, gw.m10, gw.m20, gw.m01, gw.m11, gw.m21, gw.m02,
                    gw.m12, gw.m22
                );
                let gt = n.geometry_transform;
                println!(
                    "        mesh: geometry_transform T=({:.4},{:.4},{:.4}) S=({:.4},{:.4},{:.4})",
                    gt.translation.x, gt.translation.y, gt.translation.z, gt.scale.x, gt.scale.y,
                    gt.scale.z
                );
                println!(
                    "        mesh: verts={} skinned_is_local={} skinned_values={}",
                    m.num_vertices,
                    m.skinned_is_local,
                    m.skinned_position.values.len()
                );
                if m.vertex_position.exists && m.num_vertices > 0 {
                    let v = m.vertex_position[0];
                    println!(
                        "        mesh: raw vertex_position[0]=({:.4},{:.4},{:.4})",
                        v.x, v.y, v.z
                    );
                }
                if !m.skinned_position.values.is_empty() {
                    let v = m.skinned_position[0];
                    println!(
                        "        mesh: skinned_position[0]=({:.4},{:.4},{:.4})",
                        v.x, v.y, v.z
                    );
                }
                for d in 0..m.skin_deformers.len() {
                    let sd = &m.skin_deformers[d];
                    println!(
                        "        skin[{d}] clusters={} max_weights_per_vertex={}",
                        sd.clusters.len(),
                        sd.max_weights_per_vertex
                    );
                    for c in 0..sd.clusters.len() {
                        let cl = &sd.clusters[c];
                        let bw = cl.bind_to_world;
                        println!(
                            "          cluster[{c}] bone={:?} num_weights={} bind_to_world T=({:.4},{:.4},{:.4})",
                            cl.bone_node.as_ref().map(|b| format!("{:?}", b.element.name)),
                            cl.num_weights,
                            bw.m03,
                            bw.m13,
                            bw.m23
                        );
                    }
                }
            }
        }
    }
}
