// 参考姿态 vs 动画第 0 帧：同一条 FBX 路径内部是否自洽。
//
// 背景：`src/fbx.rs` 里两条流各有一套骨骼位移口径 ——
//   `reference_poses`（`read()` 用）已改成以 `cluster.bind_to_world` 为父链基；
//   `read_frames`（动画流）仍以节点 rest 姿态（`accumulate_worlds`）为父链基。
// 若两者对同一根骨骼给出不同的局部平移，动画第 0 帧就会与参考姿态对不上 ——
// 这正是「参考姿态对了但动画里武器位置还是不对」的形态。
//
// usage: cargo run --release --locked --example probe_fbx_animref -- <file.fbx> [stackName] [名字过滤...]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe_fbx_animref <file.fbx> [stackName] [名字过滤...]");
        std::process::exit(2);
    }
    let path = std::path::PathBuf::from(&args[0]);
    let stack = args.get(1).cloned();
    let filters: Vec<String> = args.iter().skip(2).cloned().collect();
    let hit = |n: &str| filters.is_empty() || filters.iter().any(|f| n.contains(f.as_str()));

    let opts = mdlc::fbx::FbxOpts::default();

    // 参考姿态（= 骨骼表里写进去的那一份）
    let geom = mdlc::fbx::read(&path, "probe", &opts).expect("read 失败");
    let ref_poses = &geom.smd.frames[0].poses;

    // 动画第 0 帧
    let anim = mdlc::fbx::read_frames(&path, "probe", stack.as_deref(), 30.0, &opts)
        .expect("read_frames 失败");
    let f0 = &anim.frames[0].poses;

    let name_of = |i: usize| -> String {
        geom.smd
            .nodes
            .iter()
            .find(|n| n.index == i as i32)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| format!("<{i}>"))
    };

    println!(
        "节点 {} / 参考姿态 {} 条 / 动画 {} 帧（第 0 帧 {} 条）",
        geom.smd.nodes.len(),
        ref_poses.len(),
        anim.frames.len(),
        f0.len()
    );

    let mut same = 0usize;
    let mut diff = 0usize;
    for rp in ref_poses {
        let name = name_of(rp.bone as usize);
        if !hit(&name) {
            continue;
        }
        let Some(ap) = f0.iter().find(|p| p.bone == rp.bone) else {
            println!("  {name} [{:>3}]：动画第 0 帧里没有这根骨骼", rp.bone);
            diff += 1;
            continue;
        };
        let dp = [
            ap.position[0] - rp.position[0],
            ap.position[1] - rp.position[1],
            ap.position[2] - rp.position[2],
        ];
        let dr = [
            ap.rotation[0] - rp.rotation[0],
            ap.rotation[1] - rp.rotation[1],
            ap.rotation[2] - rp.rotation[2],
        ];
        let maxp = dp.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let maxr = dr.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        if maxp == 0.0 && maxr == 0.0 {
            same += 1;
            continue;
        }
        diff += 1;
        println!(
            "  ✗ {name} [{:>3}]  dpos=({:.4},{:.4},{:.4}) max={:.4}   drot=({:.4},{:.4},{:.4}) max={:.4}",
            rp.bone, dp[0], dp[1], dp[2], maxp, dr[0], dr[1], dr[2], maxr
        );
        println!(
            "        参考 pos=({:.4},{:.4},{:.4}) rot=({:.4},{:.4},{:.4})",
            rp.position[0], rp.position[1], rp.position[2], rp.rotation[0], rp.rotation[1], rp.rotation[2]
        );
        println!(
            "        动画 pos=({:.4},{:.4},{:.4}) rot=({:.4},{:.4},{:.4})",
            ap.position[0], ap.position[1], ap.position[2], ap.rotation[0], ap.rotation[1], ap.rotation[2]
        );
    }
    println!("  => 逐值相同 {same} / 不同 {diff}");
}
