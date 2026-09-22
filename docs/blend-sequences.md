// blend 序列的完整规格（本文件是**实现依据**，结论全部来自实测/源码）
//
// ═══════════════════════════════════════════════════════════════════════
// 一、QC 侧
// ═══════════════════════════════════════════════════════════════════════
//
// $sequence "idle" {
//     "a_run"                 <- 多个动画名 = blend 网格
//     "a_idle"
//     "a_run"
//     activity "ACT_VM_IDLE" 1
//     blend "move_x" -1 1     <- 参数名 + start + end
//     blendwidth 3            <- groupsize[0] = 3
//     addlayer "look_poses"   <- 自动层
// }
//
// $animation "look_down" "look_poses" frames 0 0 subtract "a_idle" 0
//   ^ 从 look_poses.smd 抽第 0 帧，减去 a_idle 的第 0 帧 ⟹ 1 帧的 delta 动画
//
// ═══════════════════════════════════════════════════════════════════════
// 二、官方产物实测（v_autoshotgun.mdl，27 seqdesc / 29 animdesc）
// ═══════════════════════════════════════════════════════════════════════
//
// [0] look_poses  numblends=3  groupsize=3x1  paramindex=[0,-1]
//     paramstart=[-1,0]  paramend=[1,0]
//     flags=0x14 (DELTA|POST)
//     posekeyindex=5724 -> [-1, 0, 1, 0]   (前 3 个 = param0，后 1 个 = param1)
//     animindexindex=6212 -> [2, 3, 4]     (animdesc: look_down, look_mid, look_up)
//     numautolayers=0
//
// [2] idle        numblends=3  groupsize=3x1  paramindex=[1,-1]
//     paramstart=[-1,0]  paramend=[1,0]
//     flags=0x01 (LOOPING)   actweight=1
//     posekeyindex=5800 -> [-1, 0, 1, 0]
//     animindexindex=5840 -> [1, 0, 1]     (animdesc: a_run, a_idle, a_run)
//     numautolayers=1  autolayerindex=5816
//       -> iSequence=0(look_poses) iPose=0 flags=0x0
//          start=peak=tail=end=0
//
// ⚠️ **blend 的 animdesc 用「源动画名」，不是 `@序列名`**：
//    单动画序列的 animdesc 叫 `@reload`，而 blend 的格子叫 `a_run` / `look_down`。
//
// ⚠️ 每个格子是**独立动画**，帧数可以不同：
//    `idle` 的格子 = a_run(33 帧) / a_idle(121 帧) / a_run(33 帧)
//
// ═══════════════════════════════════════════════════════════════════════
// 三、布局（write.cpp）
// ═══════════════════════════════════════════════════════════════════════
//
// 段序（都在 seq 子表区内，**相对 seqdesc 数组起点**）：
//
//   events → ikrules(挂在 animdesc 上) → autolayers → iklocks →
//   **blend 表** → ALIGN4 → keyvalues → …
//
// `write.cpp:449-466`：`groupsize[0]>1 || groupsize[1]>1` 时写 posekey：
//     `posekeyindex = pData - pSequenceStart`
//     `groupsize[0]` 个 param0[]，紧跟 `groupsize[1]` 个 param1[]
//     （**没有 ALIGN4**，但 float 本就 4 对齐）
//
// `write.cpp:613-638`：blend 表
//     `animindexindex = pData - pSequenceStart`
//     `groupsize[0]*groupsize[1]` 个 **int16**，随后 `ALIGN4`
//     填充顺序：`for j in 0..groupsize[0] { for k in 0..groupsize[1] {
//         offset = k*groupsize[0] + j;  blends[offset] = panim[j][k]->index; } }`
//     ⟹ **列主序存储**（offset 以 groupsize[0] 为行宽）
//
// `write.cpp:529-551`：autolayer（`mstudioautolayer_t` = **24 字节**）
//     short iSequence / short iPose / int flags /
//     float start / peak / tail / end
//     非 POSE 标志时，四个量**除以 (numframes-1)**（转成 cycle）；
//     带 `STUDIO_AL_POSE` 时原样写。
//
// ═══════════════════════════════════════════════════════════════════════
// 四、param0/param1 的计算（simplify.cpp:5448-5594）
// ═══════════════════════════════════════════════════════════════════════
//
// `CalcPoseParameters()`：对每个 groupsize[iPose] > 1 的轴：
//
//   * **有 `paramattachment`**（QC 的 `blend ... <attachment>` 形态）：
//     逐格算 `CalcPoseParameterValue(paramcontrol, angles, pos)` —— 复杂。
//   * **否则**（本项目的形态）：**线性插值**
//     ```
//     for m in 0..groupsize[iPose]:
//         f = m / (groupsize[iPose] - 1)
//         param0[m] = paramstart * (1-f) + paramend * f
//     ```
//     实测 `look_poses`：paramstart=-1, paramend=1, groupsize[0]=3
//     ⟹ param0 = [-1, 0, 1] ✓
//     第二个轴 groupsize[1]=1 ⟹ param1[0] = 0（不参与）
//
// ═══════════════════════════════════════════════════════════════════════
// 五、TOML 设计
// ═══════════════════════════════════════════════════════════════════════
//
// 保持向后兼容（单动画序列仍写 `smd = "x.smd"`），新增：
//
//   [[sequences]]
//   name = "idle"
//   activity = "ACT_VM_IDLE"
//   activity_weight = 1
//   looping = true
//
//   # blend 网格：按**行主序**列出每一格的 SMD（groupsize[1] 行 × groupsize[0] 列）
//   blends = ["a_run.smd", "a_idle.smd", "a_run.smd"]
//   blend_width = 3                 # groupsize[0]
//
//   # 参数轴（最多 2 个）
//   [[sequences.blend_params]]
//   parameter = "move_x"            # 引用 [[model.pose_parameters]] 的名字
//   start = -1.0
//   end = 1.0
//
//   # 自动层
//   [[sequences.auto_layers]]
//   sequence = "look_poses"
//   start = 0.0
//   peak = 0.0
//   tail = 0.0
//   end = 0.0
