//! 打印 FBX 的**控制点位置**与**shape key 偏移表**，用于解出官方 flex vertanim 的分组顺序。
//!
//! 用法：`cargo run --release --example probe_fbx_shapefull -- <file.fbx>`
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = PathBuf::from(args.get(1).expect("用法：probe_fbx_shapefull <file.fbx>"));
    let bytes = std::fs::read(&path).expect("读不到文件");
    let scene = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("解析失败");

    println!("== 场景 ==");
    println!("unit_meters={} fps={}", scene.settings.unit_meters, scene.settings.frames_per_second);
    println!("nodes={} meshes={} blend_deformers={}", scene.nodes.len(), scene.meshes.len(), scene.blend_deformers.len());

    for ni in 0..scene.nodes.len() {
        let n = &scene.nodes[ni];
        let name = n.element.name.to_string();
        let Some(mesh) = n.mesh.as_ref() else {
            println!("[node {ni}] {name:?} (无网格) is_root={}", n.is_root);
            continue;
        };
        println!(
            "[node {ni}] {name:?} cps={} indices={} tris={} skin_deformers={} blend_deformers={}",
            mesh.vertices.len(),
            mesh.vertex_indices.len(),
            mesh.num_triangles,
            mesh.skin_deformers.len(),
            mesh.blend_deformers.len(),
        );
        let g = &n.geometry_to_world;
        println!("  geometry_to_world = [{:.5} {:.5} {:.5} | {:.5}]", g.m00, g.m01, g.m02, g.m03);
        println!("                      [{:.5} {:.5} {:.5} | {:.5}]", g.m10, g.m11, g.m12, g.m13);
        println!("                      [{:.5} {:.5} {:.5} | {:.5}]", g.m20, g.m21, g.m22, g.m23);
        for i in 0..mesh.vertices.len() {
            let p = mesh.vertices[i];
            println!("  cp{i:>2} pos=({:.4}, {:.4}, {:.4})", p.x, p.y, p.z);
        }
        // 多边形索引流（前 36 项足够看出面序）
        let n_idx = mesh.vertex_indices.len().min(36);
        let stream: Vec<u32> = (0..n_idx).map(|i| mesh.vertex_indices[i]).collect();
        println!("  index_stream[0..{n_idx}] = {stream:?}");
        for di in 0..mesh.blend_deformers.len() {
            let d = &mesh.blend_deformers[di];
            println!("  [deformer {di}] channels={}", d.channels.len());
        }
    }

    println!("== shape keys（按 blend_deformers → channels → target_shape 遍历）==");
    for di in 0..scene.blend_deformers.len() {
        let d = &scene.blend_deformers[di];
        for ci in 0..d.channels.len() {
            let ch = &d.channels[ci];
            let Some(shape) = ch.target_shape.as_ref() else { continue };
            println!(
                "[deformer {di} channel {ci}] name={:?} offsets={} weight={}",
                shape.element.name.to_string(),
                shape.num_offsets,
                ch.weight,
            );
            for k in 0..shape.num_offsets {
                let vi = shape.offset_vertices[k];
                let po = shape.position_offsets[k];
                let no = shape.normal_offsets[k];
                println!(
                    "  off{k:>2} v={vi:>2} pos_off=({:.4}, {:.4}, {:.4}) nrm_off=({:.4}, {:.4}, {:.4})",
                    po.x, po.y, po.z, no.x, no.y, no.z,
                );
            }
        }
    }
}
