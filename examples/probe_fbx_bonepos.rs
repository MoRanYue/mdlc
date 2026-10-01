//! 骨骼位移口径取证：对每根保留骨骼打印各候选口径，并可对照官方 MDL 的
//! `mstudiobone_t.pos` 直接裁决哪一条是对的。
//!
//! 候选（`l` = 自己的 `local_transform.translation`，`P` = 父的 `node_to_world`）：
//!   A `l`                     —— 原样局部平移（mdlc 现状）
//!   B `P.basis · l`           —— 用父的世界基（含父的旋转与缩放）变换
//!   C `worldT − parentWorldT` —— 世界位移差
//!   D `worldT`                —— 世界位置
//!   E `P.scale · l`           —— 只用父的**缩放**，不施加父的旋转
//!   F `P.rot⁻¹ · C / P.scale` —— 把世界位移差转回父的局部系（「标准」逆变换）
//!
//! ⭐ 只在「父的世界旋转不是恒等」且「自己的 `l` 非零」的骨骼上，
//! B / E / F 才会分道扬镳。`rig.fbx` 恰好不是这种样本：`Skeleton` 绕 X −90°、
//! 其子 `Pelvis` 绕 X +90°，两者抵消 ⟹ 父基旋转为恒等，B ≡ E ≡ F。
//! 因此本探针支持传官方 `.mdl`，用产物本身来裁决，而不是靠人眼挑样本。
//!
//! 用法：`cargo run --release --example probe_fbx_bonepos -- <file.fbx> [official.mdl]`

use std::path::PathBuf;

/// 官方 MDL 骨骼表里的一根骨骼（只需名字与 `pos`）。
struct OfficialBone {
    name: String,
    pos: [f64; 3],
}

/// 读 MDL 骨骼表。偏移依据 `studio.h`：`numbones@0x9C` / `boneindex@0xA0`，
/// 记录步长 216，`sznameindex@0`（**相对该记录自身**）、`pos@0x20`。
fn read_official_bones(path: &PathBuf) -> Option<Vec<OfficialBone>> {
    let b = std::fs::read(path).ok()?;
    if b.len() < 0xB0 {
        return None;
    }
    let i32_at = |o: usize| -> i32 { i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) };
    let f32_at = |o: usize| -> f64 { f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as f64 };
    let n = i32_at(0x9C).max(0) as usize;
    let base = i32_at(0xA0).max(0) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = base + i * 216;
        if o + 216 > b.len() {
            break;
        }
        let no = o + i32_at(o).max(0) as usize;
        let mut e = no;
        while e < b.len() && b[e] != 0 {
            e += 1;
        }
        if no >= b.len() {
            continue;
        }
        out.push(OfficialBone {
            name: String::from_utf8_lossy(&b[no..e]).to_string(),
            pos: [f32_at(o + 0x20), f32_at(o + 0x24), f32_at(o + 0x28)],
        });
    }
    Some(out)
}

/// 名字匹配：FBX 节点名与官方骨骼名可能只差前缀（如 `ValveBiped.Bip01_`），
/// 故按「一方是另一方后缀」匹配，大小写不敏感。
fn name_matches(node: &str, bone: &str) -> bool {
    let (a, b) = (node.to_ascii_lowercase(), bone.to_ascii_lowercase());
    a == b || a.ends_with(&b) || b.ends_with(&a)
}

fn len3(v: ufbx::Vec3) -> f64 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

fn col(m: &ufbx::Matrix, i: usize) -> ufbx::Vec3 {
    match i {
        0 => ufbx::Vec3 { x: m.m00, y: m.m10, z: m.m20 },
        1 => ufbx::Vec3 { x: m.m01, y: m.m11, z: m.m21 },
        _ => ufbx::Vec3 { x: m.m02, y: m.m12, z: m.m22 },
    }
}

/// `M.basis · v`（列主序：`m00/m10/m20` 是第 0 列）。
fn basis_mul(m: &ufbx::Matrix, v: ufbx::Vec3) -> ufbx::Vec3 {
    ufbx::Vec3 {
        x: m.m00 * v.x + m.m01 * v.y + m.m02 * v.z,
        y: m.m10 * v.x + m.m11 * v.y + m.m12 * v.z,
        z: m.m20 * v.x + m.m21 * v.y + m.m22 * v.z,
    }
}

fn fmt(v: ufbx::Vec3) -> String {
    format!("({:.4},{:.4},{:.4})", v.x, v.y, v.z)
}

fn near(a: ufbx::Vec3, b: [f64; 3]) -> bool {
    (a.x - b[0]).abs() < 0.01 && (a.y - b[1]).abs() < 0.01 && (a.z - b[2]).abs() < 0.01
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx_bonepos <file.fbx> [official.mdl]");
        std::process::exit(2);
    }
    let path = PathBuf::from(&args[0]);
    let official = args.get(1).map(PathBuf::from).and_then(|p| read_official_bones(&p));

    println!("==== {} ====", path.display());
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            println!("读不到：{e}");
            return;
        }
    };
    let scene = match ufbx::load_memory(&bytes, ufbx::LoadOpts::default()) {
        Ok(s) => s,
        Err(e) => {
            println!("解析失败：{e:?}");
            return;
        }
    };
    if let Some(bones) = &official {
        println!("[官方骨骼] {} 根", bones.len());
    }

    // 各候选累计命中数，用来在末尾直接给结论。
    let mut hits = [0usize; 6];
    let mut total = 0usize;

    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        if n.is_root {
            continue;
        }
        let lt = n.local_transform.translation;
        let w = n.node_to_world;
        let parent_world = n.parent.as_ref().map(|p| p.node_to_world);
        let pb = parent_world.unwrap_or_default();

        let a = lt;
        let b = basis_mul(&pb, lt);
        let c = match parent_world {
            Some(p) => ufbx::Vec3 {
                x: w.m03 - p.m03,
                y: w.m13 - p.m13,
                z: w.m23 - p.m23,
            },
            None => ufbx::Vec3 { x: w.m03, y: w.m13, z: w.m23 },
        };
        let d = ufbx::Vec3 { x: w.m03, y: w.m13, z: w.m23 };
        let (sx, sy, sz) = (len3(col(&pb, 0)), len3(col(&pb, 1)), len3(col(&pb, 2)));
        let e = ufbx::Vec3 {
            x: lt.x * sx,
            y: lt.y * sy,
            z: lt.z * sz,
        };
        // F：把世界位移差 C 用父基的**逆**转回父的局部系。
        // 父基 = 旋转 × 缩放（无剪切时），逆 = 缩放⁻¹ × 旋转ᵀ。
        let inv = ufbx::matrix_invert(&pb);
        let f = basis_mul(&inv, c);

        let mesh = if n.mesh.is_some() { " MESH" } else { "" };
        println!("  [{i}] {:?}{mesh}", n.element.name);
        println!("      A(local)  = {}", fmt(a));
        println!("      B(P*basis)= {}", fmt(b));
        println!("      C(dWorld) = {}", fmt(c));
        println!("      D(world)  = {}", fmt(d));
        println!("      E(P*scale)= {}", fmt(e));
        println!("      F(P⁻¹*dW) = {}", fmt(f));
        // 父基归一化后就是父的世界旋转 —— 直接打出来，肉眼即可判断
        // 它是不是单位阵（只有不是单位阵，B/E/F 才会分家）。
        let u0 = col(&pb, 0);
        let u1 = col(&pb, 1);
        let u2 = col(&pb, 2);
        println!(
            "      父基列长 = ({sx:.4},{sy:.4},{sz:.4})"
        );
        if sx > 1e-12 && sy > 1e-12 && sz > 1e-12 {
            let n0 = (u0.x / sx, u0.y / sx, u0.z / sx);
            let n1 = (u1.x / sy, u1.y / sy, u1.z / sy);
            let n2 = (u2.x / sz, u2.y / sz, u2.z / sz);
            println!(
                "      父世界旋转 = [{:.3},{:.3},{:.3} | {:.3},{:.3},{:.3} | {:.3},{:.3},{:.3}]",
                n0.0, n0.1, n0.2, n1.0, n1.1, n1.2, n2.0, n2.1, n2.2
            );
        }

        // 有官方产物就逐候选判定。
        if let Some(bones) = &official
            && let Some(ob) = bones.iter().find(|ob| name_matches(&n.element.name, &ob.name))
        {
            let cands = [a, b, c, d, e, f];
            let labels = ["A", "B", "C", "D", "E", "F"];
            let marks: Vec<String> = cands
                .iter()
                .enumerate()
                .map(|(k, v)| {
                    if near(*v, ob.pos) {
                        hits[k] += 1;
                        format!("{}✅", labels[k])
                    } else {
                        format!("{}✗", labels[k])
                    }
                })
                .collect();
            total += 1;
            println!("      官方 {:?} pos = {:?}", ob.name, ob.pos);
            println!("      判定 {}", marks.join(" "));
        }
    }

    if total > 0 {
        let labels = ["A", "B", "C", "D", "E", "F"];
        let line: Vec<String> = hits
            .iter()
            .enumerate()
            .map(|(k, h)| format!("{}({h}/{total})", labels[k]))
            .collect();
        println!("[汇总] {}", line.join("  "));
    }
}
