//! 探针：某条动画栈里，**哪些节点真的有动画通道**。
//!
//! 用途：判 `read_frames` 对「没被动画驱动的骨骼」退回节点 rest 姿态是否合理。
//!
//! `src/fbx.rs` 的 `read_frames` 里，节点在 `bake.nodes` 里查不到时用
//! `n.local_transform`（节点自己的 rest 姿态）。若某个骨骼**根本不在动画里**，
//! 那它每帧都会停在 rest 姿态 —— 而模型的**参考姿态**取的是绑定姿态
//! （`reference_poses` → `bind_worlds`）。两者不一致时，模型一播动画就会跳。
//!
//! 所以要先分清两种情形：
//! * **有通道**：动画确实驱动了这根骨骼 ⟹ 用节点局部变换是对的；
//! * **无通道**：动画没碰它 ⟹ 应当保持参考姿态（绑定姿态），而不是 rest。
//!
//! 用法：`cargo run --release --locked --example probe_fbx_animchan -- <file.fbx> [stackName] [名字过滤...]`

use std::collections::HashMap;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: probe_fbx_animchan <file.fbx> [stackName] [名字过滤...]");
        std::process::exit(2);
    }
    let path = PathBuf::from(&args[0]);
    let stack = args.get(1).cloned().filter(|s| !s.is_empty());
    let filters: Vec<String> = args.iter().skip(2).cloned().collect();
    let hit = |n: &str| filters.is_empty() || filters.iter().any(|f| n.contains(f.as_str()));

    let bytes = std::fs::read(&path).expect("读不到");
    let scene = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("解析失败");

    let stacks: Vec<String> = scene
        .anim_stacks
        .iter()
        .map(|s| s.element.name.to_string())
        .collect();
    let si = match &stack {
        Some(w) => stacks
            .iter()
            .position(|s| s.eq_ignore_ascii_case(w))
            .unwrap_or_else(|| {
                eprintln!("没有栈 {w:?}；有的是 {stacks:?}");
                std::process::exit(1);
            }),
        None => 0,
    };

    let bake = ufbx::bake_anim(
        &scene,
        &scene.anim_stacks[si].anim,
        ufbx::BakeOpts {
            resample_rate: 30.0,
            ..Default::default()
        },
    )
    .expect("烘焙失败");

    let baked_of: HashMap<u32, usize> = (0..bake.nodes.len())
        .map(|i| (bake.nodes[i].typed_id, i))
        .collect();

    println!(
        "栈 {:?}  烘焙节点 {}  时长 {:.6}  t_begin {:.6}",
        scene.anim_stacks[si].element.name,
        bake.nodes.len(),
        bake.playback_duration,
        bake.playback_time_begin
    );

    let t = bake.playback_time_begin;
    let mut with_ch = 0usize;
    let mut without_ch = 0usize;
    for n in &scene.nodes {
        let name = n.element.name.to_string();
        if !hit(&name) {
            continue;
        }
        let rest = n.local_transform.translation;
        match baked_of.get(&n.element.typed_id) {
            None => {
                without_ch += 1;
                println!(
                    "  [无通道] {name}  restT=({:.4},{:.4},{:.4})",
                    rest.x, rest.y, rest.z
                );
            }
            Some(&bi) => {
                with_ch += 1;
                let b = &bake.nodes[bi];
                let tv = if b.translation_keys.is_empty() {
                    None
                } else {
                    Some(ufbx::evaluate_baked_vec3(&b.translation_keys, t))
                };
                let rv = if b.rotation_keys.is_empty() {
                    None
                } else {
                    Some(ufbx::evaluate_baked_quat(&b.rotation_keys, t))
                };
                println!(
                    "  [有通道] {name}  tkeys={} rkeys={} skeys={} constT={} constR={} constS={}",
                    b.translation_keys.len(),
                    b.rotation_keys.len(),
                    b.scale_keys.len(),
                    b.constant_translation,
                    b.constant_rotation,
                    b.constant_scale
                );
                match tv {
                    Some(v) => println!(
                        "           t0T=({:.4},{:.4},{:.4})   restT=({:.4},{:.4},{:.4})   同值={}",
                        v.x,
                        v.y,
                        v.z,
                        rest.x,
                        rest.y,
                        rest.z,
                        (v.x - rest.x).abs() < 1e-9
                            && (v.y - rest.y).abs() < 1e-9
                            && (v.z - rest.z).abs() < 1e-9
                    ),
                    None => println!("           translation_keys 为空"),
                }
                if let Some(q) = rv {
                    let r = n.local_transform.rotation;
                    println!(
                        "           t0R=({:.6},{:.6},{:.6},{:.6})  restR=({:.6},{:.6},{:.6},{:.6})",
                        q.x, q.y, q.z, q.w, r.x, r.y, r.z, r.w
                    );
                }
            }
        }
    }
    println!("---- 有通道 {with_ch} / 无通道 {without_ch} ----");
}
