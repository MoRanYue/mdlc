//! 探针：FBX 蒙皮网格的顶点与骨骼到底该落在哪个空间。
//!
//! 背景：`src/fbx.rs` 的 `reference_poses` 用**节点 rest 姿态**（`accumulate_worlds`
//! 累加 `local_transform`）算骨骼世界矩阵，顶点却用 `geometry_point(geometry_to_world, raw)`。
//! 但蒙皮网格的顶点是写在**绑定姿态**空间里的（ufbx 的 `cluster.bind_to_world` /
//! `geometry_to_bone`），两者混用就会让武器骨骼与网格错位。
//!
//! 本探针把一个网格里**每个有权重的簇**单独拎出来，用三种口径算它名下顶点的包围盒，
//! 再看骨骼的绑定位置是否落在盒内 —— 这正是 `bone_vs_verts.js` 的判据，只是搬到了 FBX 侧：
//!
//!   a = geometry_point(node.geometry_to_world, raw)       —— mdlc 现在用的
//!   b = rot_norm(geometry_to_world)⁻¹ · a                 —— 「网格节点局部」口径
//!   c = bind_to_world · geometry_to_bone · raw            —— ufbx 文档推荐口径
//!
//! 骨骼位置也同时给出两个口径：`bindW` 是 `bind_to_world` 的平移（除以列长，即米），
//! `bindWl` 是它再乘上 `rot_norm(geometry_to_world)⁻¹` 后的结果。
//!
//! 用法：cargo run --release --locked --example probe_fbx_bind -- <file.fbx> [名字过滤...]

type M = ufbx::Matrix;
type V = ufbx::Vec3;

fn column_lengths(m: &M) -> (f64, f64, f64) {
    fn len(x: f64, y: f64, z: f64) -> f64 {
        let l = (x * x + y * y + z * z).sqrt();
        if l > 0.0 { l } else { 1.0 }
    }
    (
        len(m.m00, m.m10, m.m20),
        len(m.m01, m.m11, m.m21),
        len(m.m02, m.m12, m.m22),
    )
}

fn rotation_of(m: &M) -> M {
    fn unit(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        let l = (x * x + y * y + z * z).sqrt();
        if l > 0.0 { (x / l, y / l, z / l) } else { (x, y, z) }
    }
    let (a0, a1, a2) = unit(m.m00, m.m10, m.m20);
    let (b0, b1, b2) = unit(m.m01, m.m11, m.m21);
    let (c0, c1, c2) = unit(m.m02, m.m12, m.m22);
    M {
        m00: a0,
        m10: a1,
        m20: a2,
        m01: b0,
        m11: b1,
        m21: b2,
        m02: c0,
        m12: c1,
        m22: c2,
        m03: 0.0,
        m13: 0.0,
        m23: 0.0,
    }
}

fn normalized_translation(m: &M) -> V {
    let (lx, ly, lz) = column_lengths(m);
    V { x: m.m03 / lx, y: m.m13 / ly, z: m.m23 / lz }
}

fn geometry_point(g: &M, p: V) -> V {
    let r = rotation_of(g);
    let t = normalized_translation(g);
    V {
        x: r.m00 * p.x + r.m01 * p.y + r.m02 * p.z + t.x,
        y: r.m10 * p.x + r.m11 * p.y + r.m12 * p.z + t.y,
        z: r.m20 * p.x + r.m21 * p.y + r.m22 * p.z + t.z,
    }
}

/// 转置 3×3（旋转矩阵的逆）。
fn transpose3(m: &M) -> M {
    M {
        m00: m.m00,
        m10: m.m01,
        m20: m.m02,
        m01: m.m10,
        m11: m.m11,
        m21: m.m12,
        m02: m.m20,
        m12: m.m21,
        m22: m.m22,
        m03: 0.0,
        m13: 0.0,
        m23: 0.0,
    }
}

#[derive(Clone, Copy)]
struct Bounds {
    min: V,
    max: V,
}

impl Bounds {
    fn new() -> Self {
        Bounds {
            min: V { x: f64::INFINITY, y: f64::INFINITY, z: f64::INFINITY },
            max: V { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY, z: f64::NEG_INFINITY },
        }
    }
    fn add(&mut self, p: V) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.min.z = self.min.z.min(p.z);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
        self.max.z = self.max.z.max(p.z);
    }
    fn outside(&self, p: V) -> f64 {
        let dx = (self.min.x - p.x).max(p.x - self.max.x).max(0.0);
        let dy = (self.min.y - p.y).max(p.y - self.max.y).max(0.0);
        let dz = (self.min.z - p.z).max(p.z - self.max.z).max(0.0);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
    fn show(&self) -> String {
        format!(
            "min=({:9.3},{:9.3},{:9.3}) max=({:9.3},{:9.3},{:9.3})",
            self.min.x, self.min.y, self.min.z, self.max.x, self.max.y, self.max.z
        )
    }
}

fn show_m(m: &M) -> String {
    format!(
        "T=({:9.4},{:9.4},{:9.4}) basis=({:8.3},{:8.3},{:8.3} | {:8.3},{:8.3},{:8.3} | {:8.3},{:8.3},{:8.3})",
        m.m03, m.m13, m.m23,
        m.m00, m.m10, m.m20,
        m.m01, m.m11, m.m21,
        m.m02, m.m12, m.m22
    )
}

fn show_v(p: V) -> String {
    format!("({:9.4},{:9.4},{:9.4})", p.x, p.y, p.z)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("用法：probe_fbx_bind <file.fbx> [名字过滤...]");
    let filters: Vec<String> = args.collect();
    let hit = |n: &str| filters.is_empty() || filters.iter().any(|f| n.contains(f.as_str()));

    let bytes = std::fs::read(&path).expect("读不到");
    let scene = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("解析失败");

    for node in &scene.nodes {
        let Some(mesh) = node.mesh.as_ref() else { continue };
        let name = node.element.name.to_string();
        if !hit(&name) {
            continue;
        }
        let g = &node.geometry_to_world;
        let g_inv_rot = transpose3(&rotation_of(g));

        println!("\n=== mesh {name:?}  verts={}  deformers={}", mesh.num_vertices, mesh.skin_deformers.len());
        println!("    node_to_world     {}", show_m(&node.node_to_world));
        println!("    geometry_to_world {}", show_m(g));

        let raw = &mesh.vertex_position.values;
        let mut raw_b = Bounds::new();
        for p in raw {
            raw_b.add(*p);
        }
        println!("    raw               {}", raw_b.show());

        for d in &mesh.skin_deformers {
            for c in &d.clusters {
                if c.num_weights == 0 {
                    continue;
                }
                let bone = c
                    .bone_node
                    .as_ref()
                    .map(|b| b.element.name.to_string())
                    .unwrap_or_else(|| "<无>".to_string());
                if !hit(&bone) {
                    continue;
                }
                let mut a_b = Bounds::new();
                let mut b_b = Bounds::new();
                let mut c_b = Bounds::new();
                let m_c = ufbx::matrix_mul(&c.bind_to_world, &c.geometry_to_bone);
                for vi in c.vertices.iter() {
                    let p = raw[*vi as usize];
                    let a = geometry_point(g, p);
                    b_b.add(ufbx::transform_direction(&g_inv_rot, a));
                    a_b.add(a);
                    c_b.add(ufbx::transform_position(&m_c, p));
                }
                let bind_w = normalized_translation(&c.bind_to_world);
                let bind_wl = ufbx::transform_direction(&g_inv_rot, bind_w);
                println!(
                    "  cluster {bone:?} n={}  bindW={}  bindWl={}",
                    c.num_weights,
                    show_v(bind_w),
                    show_v(bind_wl)
                );
                println!("    geometry_to_bone  {}", show_m(&c.geometry_to_bone));
                println!("    bind_to_world     {}", show_m(&c.bind_to_world));
                println!("    bind*gtb          {}", show_m(&m_c));
                for (tag, b) in [("a", &a_b), ("b", &b_b), ("c", &c_b)] {
                    println!(
                        "    [{tag}] {}  outside={:8.3}",
                        b.show(),
                        b.outside(bind_w)
                    );
                }
                println!(
                    "    [a-l] {}  outside={:8.3}   <- 用 bindWl 量 a 盒",
                    a_b.show(),
                    a_b.outside(bind_wl)
                );
            }
        }
    }
}
