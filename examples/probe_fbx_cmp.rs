//! 顶点空间裁决：把 FBX 侧若干候选变换的**顶点集合**与官方 VVD 的顶点集合对比。
//!
//! 用法：`probe_fbx_cmp <file.fbx> <official.vvd>`
//!
//! 比对用「去重后的点集」而不是逐下标 —— 官方 VVD 的顶点顺序由优化器决定，
//! 不保证与 FBX 的面角顺序一致。

use std::collections::BTreeSet;

fn key(p: ufbx::Vec3) -> (i64, i64, i64) {
    let r = |x: f64| (x * 1000.0).round() as i64;
    (r(p.x), r(p.y), r(p.z))
}

/// 取矩阵的旋转部分（每列归一化）。假定列两两正交且等长。
fn rotation_of(m: &ufbx::Matrix) -> ufbx::Matrix {
    let len = |x: f64, y: f64, z: f64| (x * x + y * y + z * z).sqrt();
    let l0 = len(m.m00, m.m10, m.m20);
    let l1 = len(m.m01, m.m11, m.m21);
    let l2 = len(m.m02, m.m12, m.m22);
    ufbx::Matrix {
        m00: m.m00 / l0,
        m10: m.m10 / l0,
        m20: m.m20 / l0,
        m01: m.m01 / l1,
        m11: m.m11 / l1,
        m21: m.m21 / l1,
        m02: m.m02 / l2,
        m12: m.m12 / l2,
        m22: m.m22 / l2,
        m03: 0.0,
        m13: 0.0,
        m23: 0.0,
    }
}

fn mul(m: &ufbx::Matrix, p: ufbx::Vec3) -> ufbx::Vec3 {
    ufbx::Vec3 {
        x: m.m00 * p.x + m.m01 * p.y + m.m02 * p.z,
        y: m.m10 * p.x + m.m11 * p.y + m.m12 * p.z,
        z: m.m20 * p.x + m.m21 * p.y + m.m22 * p.z,
    }
}

fn add(a: ufbx::Vec3, b: ufbx::Vec3) -> ufbx::Vec3 {
    ufbx::Vec3 {
        x: a.x + b.x,
        y: a.y + b.y,
        z: a.z + b.z,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("用法：probe_fbx_cmp <file.fbx> <official.vvd>");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[0]).expect("读不到 fbx");
    let scene = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("fbx 解析失败");

    // ---- 官方 VVD 的点集 ----
    let vvd = std::fs::read(&args[1]).expect("读不到 vvd");
    let rd32 = |o: usize| u32::from_le_bytes([vvd[o], vvd[o + 1], vvd[o + 2], vvd[o + 3]]);
    let n_official = rd32(0x10) as usize;
    let start = rd32(0x38) as usize;
    let mut official: BTreeSet<(i64, i64, i64)> = BTreeSet::new();
    for i in 0..n_official {
        let o = start + i * 48 + 16;
        let f = |k: usize| {
            f32::from_le_bytes([vvd[o + k * 4], vvd[o + k * 4 + 1], vvd[o + k * 4 + 2], vvd[o + k * 4 + 3]])
                as f64
        };
        official.insert((
            (f(0) * 1000.0).round() as i64,
            (f(1) * 1000.0).round() as i64,
            (f(2) * 1000.0).round() as i64,
        ));
    }
    println!(
        "官方 VVD: {} 顶点，去重后 {}",
        n_official,
        official.len()
    );

    // ---- FBX 侧：对每个候选规则建点集 ----
    let mut raw: BTreeSet<(i64, i64, i64)> = BTreeSet::new();
    let mut r_rot: BTreeSet<(i64, i64, i64)> = BTreeSet::new();
    let mut r_rot_t: BTreeSet<(i64, i64, i64)> = BTreeSet::new();
    let mut r_full: BTreeSet<(i64, i64, i64)> = BTreeSet::new();

    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        if n.is_root {
            continue;
        }
        let Some(mesh) = n.mesh.as_ref() else { continue };
        let g = n.geometry_to_world;
        let rot = rotation_of(&g);
        let t = ufbx::Vec3 {
            x: g.m03,
            y: g.m13,
            z: g.m23,
        };
        println!(
            "\n网格节点 {:?}  worldT=({:.3},{:.3},{:.3})  geometry_to_world 平移=({:.3},{:.3},{:.3})  有蒙皮={}  skinned_is_local={}",
            n.element.name,
            n.node_to_world.m03,
            n.node_to_world.m13,
            n.node_to_world.m23,
            t.x,
            t.y,
            t.z,
            !mesh.skin_deformers.is_empty(),
            mesh.skinned_is_local
        );
        for f in 0..mesh.num_faces {
            let face = mesh.faces[f];
            let mut buf = vec![0u32; mesh.max_face_triangles * 3];
            let nt = mesh.triangulate_face(&mut buf, face);
            for &corner_raw in buf.iter().take(nt as usize * 3) {
                let corner = corner_raw as usize;
                let p = mesh.vertex_position[corner];
                raw.insert(key(p));
                r_rot.insert(key(mul(&rot, p)));
                r_rot_t.insert(key(add(mul(&rot, p), t)));
                r_full.insert(key(ufbx::transform_position(&g, p)));
            }
        }
    }

    let report = |name: &str, s: &BTreeSet<(i64, i64, i64)>| {
        let missing: Vec<_> = official.difference(s).take(4).collect();
        let extra: Vec<_> = s.difference(&official).take(4).collect();
        println!(
            "  {name:<28} 去重={:<4} 官方缺={:<4} 多出={:<4} {}",
            s.len(),
            official.difference(s).count(),
            s.difference(&official).count(),
            if missing.is_empty() && extra.is_empty() {
                "✅ 完全一致".to_string()
            } else {
                format!("✗ 缺{missing:?} 多{extra:?}")
            }
        );
    };
    println!("\n候选规则对比（官方 {} 个去重点）:", official.len());
    report("R1 raw（mdlc 现状）", &raw);
    report("R2 旋转", &r_rot);
    report("R3 旋转+平移", &r_rot_t);
    report("R4 geometry_to_world 全量", &r_full);
    println!("\n官方点集: {:?}", official);
}
