//! 顶点元组裁决：位置 / 法线 / UV 三个分量**分别**与官方 VVD 对比，判定
//! 官方对每个分量各做了什么变换。
//!
//! 位置候选（`probe_fbx_cmp` 已定 R3）：
//!   P1 raw                       —— 原样
//!   P3 rot_norm(g)·p + t(g)      —— 逐列归一化旋转 + 平移
//!   P4 g·p                       —— geometry_to_world 全量
//!
//! 法线候选：
//!   N1 raw
//!   N3 rot_norm(g)·n
//!   N4 matrix_for_normals(g)·n
//!
//! UV 候选：
//!   U1 [u, v]
//!   U2 [u, 1-v]
//!
//! 用法：`probe_fbx_vert <file.fbx> <official.vvd>`

use std::collections::BTreeSet;

fn key3(p: ufbx::Vec3) -> (i64, i64, i64) {
    let r = |x: f64| (x * 1000.0).round() as i64;
    (r(p.x), r(p.y), r(p.z))
}

fn key2(p: ufbx::Vec2) -> (i64, i64) {
    let r = |x: f64| (x * 1000.0).round() as i64;
    (r(p.x), r(p.y))
}

/// 取矩阵的旋转部分（每列归一化）。
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

type K3 = (i64, i64, i64);
type K2 = (i64, i64);
type Full = (K3, K3, K2);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("用法：probe_fbx_vert <file.fbx> <official.vvd>");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[0]).expect("读不到 fbx");
    let scene = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("fbx 解析失败");

    // ---- 官方 VVD ----
    let vvd = std::fs::read(&args[1]).expect("读不到 vvd");
    let rd32 = |o: usize| u32::from_le_bytes([vvd[o], vvd[o + 1], vvd[o + 2], vvd[o + 3]]);
    let n_official = rd32(0x10) as usize;
    let start = rd32(0x38) as usize;
    let mut o_pos: BTreeSet<K3> = BTreeSet::new();
    let mut o_nrm: BTreeSet<K3> = BTreeSet::new();
    let mut o_uv: BTreeSet<K2> = BTreeSet::new();
    let mut o_full: BTreeSet<Full> = BTreeSet::new();
    for i in 0..n_official {
        let o = start + i * 48;
        let f = |k: usize| {
            f32::from_le_bytes([
                vvd[o + k * 4],
                vvd[o + k * 4 + 1],
                vvd[o + k * 4 + 2],
                vvd[o + k * 4 + 3],
            ]) as f64
        };
        let r = |x: f64| (x * 1000.0).round() as i64;
        // 位置 @16，法线 @28，UV @40
        let p = (r(f(4)), r(f(5)), r(f(6)));
        let nn = (r(f(7)), r(f(8)), r(f(9)));
        let uv = (r(f(10)), r(f(11)));
        o_pos.insert(p);
        o_nrm.insert(nn);
        o_uv.insert(uv);
        o_full.insert((p, nn, uv));
    }
    println!(
        "官方 VVD: {} 顶点 | 去重 位置={} 法线={} UV={} 全元组={}",
        n_official,
        o_pos.len(),
        o_nrm.len(),
        o_uv.len(),
        o_full.len()
    );

    // ---- FBX 侧 ----
    let mut p1: BTreeSet<K3> = BTreeSet::new();
    let mut p3: BTreeSet<K3> = BTreeSet::new();
    let mut p4: BTreeSet<K3> = BTreeSet::new();
    let mut n1: BTreeSet<K3> = BTreeSet::new();
    let mut n3: BTreeSet<K3> = BTreeSet::new();
    let mut n4: BTreeSet<K3> = BTreeSet::new();
    let mut u1: BTreeSet<K2> = BTreeSet::new();
    let mut u2: BTreeSet<K2> = BTreeSet::new();
    let mut full_p3n3u2: BTreeSet<Full> = BTreeSet::new();
    let mut full_p3n3u1: BTreeSet<Full> = BTreeSet::new();

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
        let nm = ufbx::matrix_for_normals(&g);
        for f in 0..mesh.num_faces {
            let face = mesh.faces[f];
            let mut buf = vec![0u32; mesh.max_face_triangles * 3];
            let nt = mesh.triangulate_face(&mut buf, face);
            for &corner_raw in buf.iter().take(nt as usize * 3) {
                let corner = corner_raw as usize;
                let p = mesh.vertex_position[corner];
                let nn = if mesh.vertex_normal.exists {
                    mesh.vertex_normal[corner]
                } else {
                    ufbx::Vec3 {
                        x: 0.0,
                        y: 0.0,
                        z: 1.0,
                    }
                };
                let uv = if mesh.vertex_uv.exists {
                    mesh.vertex_uv[corner]
                } else {
                    ufbx::Vec2 { x: 0.0, y: 0.0 }
                };
                let a3 = add(mul(&rot, p), t);
                let a3n = mul(&rot, nn);
                let a4n = mul(&nm, nn);
                p1.insert(key3(p));
                p3.insert(key3(a3));
                p4.insert(key3(ufbx::transform_position(&g, p)));
                n1.insert(key3(nn));
                n3.insert(key3(a3n));
                n4.insert(key3(a4n));
                u1.insert(key2(uv));
                u2.insert(key2(ufbx::Vec2 {
                    x: uv.x,
                    y: 1.0 - uv.y,
                }));
                full_p3n3u2.insert((key3(a3), key3(a3n), key2(ufbx::Vec2 {
                    x: uv.x,
                    y: 1.0 - uv.y,
                })));
                full_p3n3u1.insert((key3(a3), key3(a3n), key2(uv)));
            }
        }
    }

    let rep3 = |name: &str, s: &BTreeSet<K3>, o: &BTreeSet<K3>| {
        let miss: Vec<_> = o.difference(s).take(3).collect();
        let extra: Vec<_> = s.difference(o).take(3).collect();
        println!(
            "  {name:<26} 去重={:<4} 缺={:<4} 多={:<4} {}",
            s.len(),
            o.difference(s).count(),
            s.difference(o).count(),
            if miss.is_empty() && extra.is_empty() {
                "✅".to_string()
            } else {
                format!("✗ 缺{miss:?} 多{extra:?}")
            }
        );
    };
    let rep2 = |name: &str, s: &BTreeSet<K2>, o: &BTreeSet<K2>| {
        let miss: Vec<_> = o.difference(s).take(3).collect();
        let extra: Vec<_> = s.difference(o).take(3).collect();
        println!(
            "  {name:<26} 去重={:<4} 缺={:<4} 多={:<4} {}",
            s.len(),
            o.difference(s).count(),
            s.difference(o).count(),
            if miss.is_empty() && extra.is_empty() {
                "✅".to_string()
            } else {
                format!("✗ 缺{miss:?} 多{extra:?}")
            }
        );
    };

    println!("\n位置:");
    rep3("P1 raw", &p1, &o_pos);
    rep3("P3 rot_norm+t", &p3, &o_pos);
    rep3("P4 full", &p4, &o_pos);
    println!("法线:");
    rep3("N1 raw", &n1, &o_nrm);
    rep3("N3 rot_norm", &n3, &o_nrm);
    rep3("N4 for_normals", &n4, &o_nrm);
    println!("UV:");
    rep2("U1 [u,v]", &u1, &o_uv);
    rep2("U2 [u,1-v]", &u2, &o_uv);
    println!("全元组:");
    {
        let s = &full_p3n3u2;
        let miss: Vec<_> = o_full.difference(s).take(3).collect();
        let extra: Vec<_> = s.difference(&o_full).take(3).collect();
        println!(
            "  P3+N3+U2                  去重={:<4} 缺={:<4} 多={:<4} {}",
            s.len(),
            o_full.difference(s).count(),
            s.difference(&o_full).count(),
            if miss.is_empty() && extra.is_empty() {
                "✅".to_string()
            } else {
                format!("✗ 缺{miss:?} 多{extra:?}")
            }
        );
    }
    {
        let s = &full_p3n3u1;
        let miss: Vec<_> = o_full.difference(s).take(3).collect();
        let extra: Vec<_> = s.difference(&o_full).take(3).collect();
        println!(
            "  P3+N3+U1                  去重={:<4} 缺={:<4} 多={:<4} {}",
            s.len(),
            o_full.difference(s).count(),
            s.difference(&o_full).count(),
            if miss.is_empty() && extra.is_empty() {
                "✅".to_string()
            } else {
                format!("✗ 缺{miss:?} 多{extra:?}")
            }
        );
    }
}
