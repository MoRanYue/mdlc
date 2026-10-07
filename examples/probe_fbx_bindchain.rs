//! 探针：把「bind 姿态」折算成 SMD skeleton 的局部 pos/rot，与 SMD 真值逐值对照。
//!
//! 用法：cargo run --release --locked --example probe_fbx_bindchain -- <file.fbx>
//!
//! 真值取自 `cmp\bowsmd\bow.smd` 第 0 帧（`node dump_smd_skeleton.js`），单位米 / 弧度。

use std::collections::HashMap;

type M = ufbx::Matrix;

// 真值是 SMD 文件里抄下来的十进制字面量，1.5708 只是「恰好接近 π/2」，
// 不能换成 `FRAC_PI_2` —— 换了就不再是逐值对照。
#[allow(clippy::approx_constant)]
const TRUTH: &[(&str, [f64; 3], [f64; 3])] = &[
    ("bow", [11.9570, 50.2094, -1.0593], [0.0000, 0.0000, 0.0000]),
    ("ring_1", [2.6414, 13.5096, 0.4425], [-0.1118, 1.0370, 0.1929]),
    ("ring_2", [3.7181, 11.2516, 0.2548], [-1.2749, -0.1555, -0.6864]),
    ("string_top", [-3.5934, 17.1527, -0.3392], [3.0335, 1.5708, 0.0000]),
    ("string_bottom", [-2.1638, -14.9031, 0.3756], [-0.1477, 1.5708, 0.0000]),
    ("arrow", [-7.3429, 4.0642, 0.4246], [-1.5709, -1.5607, 0.0001]),
];

fn column_lengths(m: &M) -> (f64, f64, f64) {
    fn len(x: f64, y: f64, z: f64) -> f64 {
        let l = (x * x + y * y + z * z).sqrt();
        if l > 0.0 { l } else { 1.0 }
    }
    (len(m.m00, m.m10, m.m20), len(m.m01, m.m11, m.m21), len(m.m02, m.m12, m.m22))
}

fn normalized_translation(m: &M) -> [f64; 3] {
    let (lx, ly, lz) = column_lengths(m);
    [m.m03 / lx, m.m13 / ly, m.m23 / lz]
}

fn euler_of(q: ufbx::Quat) -> [f32; 3] {
    mdlc::bone_math::quaternion_angles([q.x as f32, q.y as f32, q.z as f32, q.w as f32])
}

fn diff(a: [f64; 3], b: [f64; 3]) -> f64 {
    let mut m = 0.0f64;
    for i in 0..3 {
        m = m.max((a[i] - b[i]).abs());
    }
    m
}

fn main() {
    let path = std::env::args().nth(1).expect("用法：probe_fbx_bindchain <file.fbx>");
    let bytes = std::fs::read(&path).expect("读不到");
    let scene = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("解析失败");

    let mut ix: HashMap<u32, usize> = HashMap::new();
    for (i, n) in scene.nodes.iter().enumerate() {
        ix.insert(n.element.element_id, i);
    }

    // 每根骨骼的绑定世界矩阵：任取一个挂着它的簇（同一骨骼处处逐位相同）。
    let mut bind: HashMap<u32, M> = HashMap::new();
    for n in &scene.nodes {
        let Some(mesh) = n.mesh.as_ref() else { continue };
        for d in &mesh.skin_deformers {
            for c in &d.clusters {
                if let Some(b) = c.bone_node.as_ref() {
                    bind.entry(b.element.element_id).or_insert(c.bind_to_world);
                }
            }
        }
    }

    // ⚠️ 同名歧义：`bow` / `ring_1` / `string_*` / `arrow` 既是顶层网格节点又是骨骼节点，
    // 只有后者挂着绑定簇。优先取有簇的那个（没有簇时才退回第一个同名节点）。
    let by_name = |want: &str| {
        let mut fallback = None;
        for n in &scene.nodes {
            if n.element.name.as_ref() != want {
                continue;
            }
            if bind.contains_key(&n.element.element_id) {
                return n;
            }
            fallback.get_or_insert(n);
        }
        fallback.unwrap_or_else(|| panic!("找不到节点 {want:?}"))
    };
    let bind_of = |n: &ufbx::Node| -> M {
        bind.get(&n.element.element_id).copied().unwrap_or(n.node_to_world)
    };

    println!("节点总数 {}，有绑定簇的骨骼 {}", scene.nodes.len(), bind.len());
    println!(
        "{:<15} {:>9} {:>9} {:>9}   {:>9} {:>9} {:>9}   {:>8} {:>8}",
        "bone", "pos.x", "pos.y", "pos.z", "rot.x", "rot.y", "rot.z", "dpos", "drot"
    );

    for (name, tpos, trot) in TRUTH {
        let n = by_name(name);
        let child = bind_of(n);
        let p = n.parent.as_ref().expect("没有父节点");
        let parent = bind_of(p);

        // bind 相对变换：inv(bind_parent) · bind_child
        let rel = ufbx::matrix_mul(&ufbx::matrix_invert(&parent), &child);
        let tr = ufbx::matrix_to_transform(&rel);
        let pos = normalized_translation(&rel);
        let rot = euler_of(tr.rotation);

        let dpos = diff(pos, *tpos);
        let drot = diff([rot[0] as f64, rot[1] as f64, rot[2] as f64], *trot);
        println!(
            "{:<15} {:9.4} {:9.4} {:9.4}   {:9.4} {:9.4} {:9.4}   {:8.4} {:8.4}   {}",
            name,
            pos[0],
            pos[1],
            pos[2],
            rot[0],
            rot[1],
            rot[2],
            dpos,
            drot,
            if dpos < 1e-3 && drot < 1e-3 { "✅" } else { "❌" }
        );

        // 对照：现行代码用的口径（节点 rest 局部姿态）
        let cur_pos = normalized_translation(&child);
        let cur_rot = euler_of(n.local_transform.rotation);
        println!(
            "{:<15} {:9.4} {:9.4} {:9.4}   {:9.4} {:9.4} {:9.4}   {:8.4} {:8.4}   （现行：节点 rest）",
            "  ↳",
            cur_pos[0],
            cur_pos[1],
            cur_pos[2],
            cur_rot[0],
            cur_rot[1],
            cur_rot[2],
            diff(cur_pos, *tpos),
            diff([cur_rot[0] as f64, cur_rot[1] as f64, cur_rot[2] as f64], *trot)
        );

        // 顺带把 parent 的绑定矩阵打印出来，便于人工核验
        let pi = ix[&p.element.element_id];
        println!("      parent[{pi}] {:?}  bind_col_len={:?}", p.element.name.as_ref(), column_lengths(&parent));
    }

    // 无簇的回退骨骼
    println!("\n---- 没有绑定簇的保留骨骼（须回退 node_to_world）----");
    for n in &scene.nodes {
        if n.is_root || n.mesh.is_some() {
            continue;
        }
        if bind.contains_key(&n.element.element_id) {
            continue;
        }
        println!(
            "  {:?}  node_to_world T={:?}  local_rot={:?}",
            n.element.name.as_ref(),
            [n.node_to_world.m03, n.node_to_world.m13, n.node_to_world.m23],
            [n.local_transform.rotation.x, n.local_transform.rotation.y, n.local_transform.rotation.z, n.local_transform.rotation.w]
        );
    }
}
