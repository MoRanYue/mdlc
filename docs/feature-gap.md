# `mdlc` 相对 Valve 原版 `studiomdl.exe` 的特性差距清单

> **调研性质**：纯只读分析 + 实测验证。**未修改 `mdlc` 任何代码文件**，只新增本报告。
> **调研日期**：本机实测，2026-09-18。
> **报告目的**：给 `mdlc`（Rust 重写的 studiomdl MVP）排后续开发优先级，逐项列出它**缺少什么**。

---

## 0. 阅读前必读：四条会改变结论的前提

### 0.1 本报告针对的是一个**移动靶**，已固定快照

调研期间 `D:\GITHUB\mdlc\src\` **被另一个 agent 并发改写**：文件从 4 个变成 8 个
（新增 `smd.rs` / `compile.rs` / `bone_math.rs`），`model.rs` 被重写两次，
输入格式从「网格内联在 TOML」改成「网格来自 SMD」，且中途一段时间**编译不过**。

本报告的「第一部分：当前能力」以**下列快照**为准（SHA256 前 16 位）：

| 文件 | 字节 | SHA256[0..16] |
|---|---|---|
| `src/bone_math.rs` | 11458 | `2ECB78D065FC0C1D` |
| `src/compile.rs` | 25338 | `C67EC66E2B82C810` |
| `src/lib.rs` | 8509 | `27494A1988858FE8` |
| `src/main.rs` | 13127 | `74847E2A77FB0C99` |
| `src/mdl_writer.rs` | 52814 | `600059F5E9ABCE00` |
| `src/model.rs` | 26240 | `17DFC8CF132567C7` |
| `src/smd.rs` | 21964 | `A92AD5DF3BFF7225` |
| `src/vvd.rs` | 17688 | `EBBE41633F76AE72` |

该快照 `cargo build --release` **通过**，`#[test]` 共 **77** 个。
若后续代码继续变化，第一部分的逐条结论需重新核对。

### 0.2 原版编译器的**写方向**规格从未公开 —— 这是本项目的根本约束

- `hl2sdk-l4d2\public\studio.h`（105,757 字节，`STUDIO_VERSION 48`，第 71 行）
  是**读方向**的权威结构定义。它**不包含**编译器如何产生这些结构的任何信息。
- `hl2sdk-l4d2\public\bone_setup.cpp`（5,800 行）是**运行时**骨骼求解，
  **不含** `poseToBone` 的计算，**不含** `BONE_USED_BY_*` 的计算（已穷举 grep 确认）。
- 编译器侧源码**不在** L4D2 SDK 里（`hl2sdk-l4d2\utils\` 下**没有** `studiomdl` 目录；
  只有 `smdlexp` 导出器、`nvtristriplib` 的**头文件**、`qc_eyes` 等）。
  可参照的是 `hl2sdk-episode1\utils\studiomdl\`（v44 时代），**是强佐证但不是 L4D2 规格**。
- 因此：**凡本报告标注「实测」的结论，都是用真实 L4D2 产物反推出来的**，
  而不是从规格文档读出来的。这是本项目唯一可靠的取证方式。

### 0.3 三份「高价值研究文档」的实际性质与预期不符（重要）
任务描述把它们列为「QC 各命令的语义」「动画」等权威参考。**实际读完后必须纠正**：

| 文档 | 实际是什么 | 对「写方向」的价值 |
|---|---|---|
| `qc-generation.md`（105 KB） | **Crowbar 反编译器**（MDL→QC 文本）的逆向规格，源自 `SourceQcFile49.vb` | **只有间接价值**。它描述 Crowbar *输出*什么文本，不描述 studiomdl *接受*什么、参数默认值是什么、命令如何影响二进制 |
| `eyeball-eyelid-mouth.md`（46 KB） | 同上，Crowbar 的 eyeball/eyelid/mouth **写出器**规格 | 同上。**完全不含** flex 结构体布局、flex opcode、VTA 二进制格式、以及任何「游戏内会怎样」的分析 |
| `animation-command.md`（66 KB） | 同上，Crowbar 的 `$animation` **写出器**规格 | **几乎为零**。不含 `mstudioanimdesc_t`/`mstudioseqdesc_t` 字段表、不含位流/RLE/量化、不含 `.ani` 格式、不含 events/activity/transition |
| `vrd-generation.md`（63 KB） | **是** VRD 文本格式 + 4 个程序化骨骼结构体的字节级规格 | **有价值**。但**不含** `mstudiojigglebone_t` 布局，也不含引擎运行时后果 |
| `mdl-layout.md`（889 行） | MDL 布局，**这才是**二进制结构的主要来源 | **高价值** |
| `vvd-vtx-layout.md`（691 行） | VVD/VTX 布局，**这才是**主要来源 | **高价值** |

**后果**：任务要求里点名的一批 QC 命令（`$pushd`/`$popd`/`$definevariable`/`$scale`/
`$renamebone`/`$collapsebone`/`$loddistance`/`$ikground` 等约 30 条）在 `qc-generation.md`
中**零覆盖**——因为它们从不被 Crowbar 输出。这些命令的存在性由我从 exe 里独立提取确认，
但**它们的语义我没有权威来源**，本报告对此**明确标注为未验证**，不做推测。

### 0.4 ⚠️ 一个必须先澄清的矛盾：`hl2sdk-l4d2` 声明 v48，但真实 L4D2 模型是 v49

任务背景称「L4D2 是 **MDL v49**」，同时把
`D:\DSH\L4D2ReverseEngineering\hl2sdk-l4d2\public\studio.h` 列为「读方向的权威结构定义」。
**这两件事直接冲突**：

- `hl2sdk-l4d2\public\studio.h:71` → `#define STUDIO_VERSION 48`，
  且 `:2875` 有 `COMPILE_TIME_ASSERT( STUDIO_VERSION == 48 );`
- 但子调研解析 `left4dead2\pak01_dir.vpk`（3,333 个 `.mdl` 条目）后报告：
  `models/survivors/anim_biker.mdl` 等**全部是 version = 49**，
  40 个随机抽样 **40/40 都是 v49**。

**子调研给出的 v49 证据（较强）**：真实 L4D2 模型的 `mstudiobone_t.flags`
出现 `0x00800000`（= v49 新增的 `BONE_HAS_SAVEFRAME_ROT32`，
**在 v48 头文件里未定义**）；且 `mstudioanimdesc_t.flags` 的
`STUDIO_FRAMEANIM (0x40)` 在 **2239/2239** 个动画上置位（也是 v49 语义）。

**v48 → v49 的关键差异**（子调研通过对比 `hl2sdk-doi\public\studio.h` 得出）：

| 位置 | v48（l4d2 头文件） | v49 |
|---|---|---|
| `studiohdr_t` @0x188/0x18C | `unused3[2]` | `flVertAnimFixedPointScale` / `surfacepropLookup` |
| `studiohdr2_t` | `reserved[59]` | `+ sznameindex, m_nBoneFlexDriverCount, m_nBoneFlexDriverIndex`（仍 256 字节） |
| `mstudioseqdesc_t` @+0xB8/+0xBC | `unused[0]`, `unused[1]` | `activitymodifierindex`, `numactivitymodifiers` |
| `mstudioanimdesc_t` @+0x1C | `unused1[0]` | `ikrulezeroframeindex` |
| 动画结构 | `mstudioanim_t` | 更名 `mstudio_rle_anim_t` + 新增 `mstudio_frame_anim_t` |
| 新增标志 | — | `STUDIO_FRAMEANIM 0x40`、`BONE_HAS_SAVEFRAME_ROT32 0x00800000` 等 |

**结构大小在两个版本里一致**（`mstudiobone_t` 216、`mstudioseqdesc_t` 212、
`mstudioanimdesc_t` 100、`studiohdr_t` 408、`studiohdr2_t` 256），
**但若干槽位的语义不同，且动画编码真正不同**。

**这对 `mdlc` 的直接影响**：
- `mdlc` 默认写 **version 49**（`model.rs:294` `unwrap_or(49)`）。
  **这是正确的选择**（与真实素材一致）。
- **但 `vvd.rs:59` 与 `model.rs:294` 的注释把 49 当作「L4D2 版本」，
  而唯一的本地头文件是 v48** —— 两者不能同时为真。
- **风险**：一旦实现动画，若照 `hl2sdk-l4d2\public\studio.h` 的
  `mstudioanim_t` 写，就会写出 v49 引擎读不懂的位流。
- **建议**：把 `hl2sdk-doi\public\studio.h`（或 `hl2sdk-csgo`，据子调研称字节相同）
  作为 v49 的结构参考；`hl2sdk-l4d2` 仅用于 v48 及读方向交叉验证。

> **诚实标注**：**我本人未验证 VPK 中模型的版本号**，此条为子调研结论。
> 但它是本报告中**最值得优先独立复核的一条**，因为它会影响动画层的所有后续工作。


---

# 第一部分：`mdlc` 当前能力清单（逐条核对代码）

## 1.1 一句话总结

> `mdlc` 目前是一个**「静态几何 + 参考姿态骨骼」的 MDL/VVD 写出器**：
> 它能把「TOML 描述 + 若干 SMD 网格文件」编译成一对 `.mdl` + `.vvd`，
> 写出 MDL 头部的前两段、骨骼表、材质表、bodypart→model→mesh 树和字符串池；
> **除这些以外的所有段一律写 0**，并且**完全不产出 `.vtx` / `.phy` / `.ani` / `.vta`**。

## 1.2 输入格式（实际支持）

| 输入 | 状态 | 证据 |
|---|---|---|
| **TOML 描述文件** | ✅ 支持 | `model.rs:27-192` 定义 `ModelDesc`/`ModelMeta`/`Materials`/`Texture`/`Bone`/`BodyPart`/`BodyModel`/`Mesh`/`Vertex`，全部 `#[serde(deny_unknown_fields)]` |
| **SMD 网格**（`nodes`/`skeleton`/`triangles`） | ✅ 支持 | `smd.rs`（612 行）完整解析器：`SmdNode`/`SmdPose`/`SmdFrame`/`SmdBoneLink`/`SmdVertex`/`SmdTriangle`；`parse_smd()` 在 `smd.rs:188` |
| SMD 的 **vertex 动画**（多帧 skeleton） | ⚠️ **解析了但没用** | `Smd.frames: Vec<SmdFrame>` 会收集全部帧（`smd.rs:91-95`），但 `compile.rs` 只取第 0 帧作参考姿态；**没有任何代码消费 `frames[1..]`** |
| SMD 的 `vertex` 段（顶点动画） | ❌ 不支持 | `smd.rs` 只识别 `version`/`nodes`/`skeleton`/`triangles` 四段；`vertex` 段会走到 `Section::None` 分支报错（`smd.rs:263-265`） |
| **DMX** | ❌ 不支持 | 代码中无任何 `dmx` 字样。原版通过子进程委托 `dmxconvert.exe`（实测该 exe 存在，690,880 字节） |
| **QC 语法** | ❌ 不支持 | `model.rs:1-21` 模块文档明确说明 MVP 刻意用 TOML；无任何 QC 解析器 |
| **QCI** | ❌ 不支持 | 同上 |
| **VTA** | ❌ 不支持 | 无任何 `vta` 字样 |
| **VRD** | ❌ 不支持 | 无任何 `vrd` 字样 |

## 1.3 编译管线

`main.rs:55` `build` 子命令 → `load_desc()`（`main.rs:96`）→ `ModelDesc::from_toml` +
`validate()` → `compile::compile(desc, base_dir)`（`compile.rs:302`）→ `write_mdl()` +
`flatten_vertices()` → `build_vvd()` → 写 `.mdl` + `.vvd`。

CLI 子命令只有 4 个（`main.rs:47-71`）：`template` / `build` / `check` / `vvd-info` / `vvd-roundtrip`。
**没有** `-game` / `-nop4` / `-verbose` / `-define` 等原版命令行开关，也没有 `$include` 式的多文件输入。

## 1.4 MDL 写出：实际写了什么

`mdl_writer.rs` 的段布局（`mdl_writer.rs:355-372`）：

```
[studiohdr_t 第0+第1部分  0x198 = 408 字节]
[骨骼表       n × 216]
[材质表       n × 64]
[body part 表 n × 16]
  → [model 表  n × 148]
      → [mesh 表 n × 116]
[字符串池（骨骼名 / 骨骼 surfaceprop / 材质名 / bodypart 名 / 头部 surfaceprop / $cdmaterials）]
[$cdmaterials 绝对偏移数组]
```

**头部实际非零的字段**（`mdl_writer.rs:486-593`）：

| 偏移 | 字段 | 来源 |
|---|---|---|
| 0x00 | `id` = `IDST` | 常量 |
| 0x04 | `version` | `desc.version()`，默认 49 |
| 0x08 | `checksum` | `desc.checksum()`（FNV-1a 或显式值） |
| 0x0C | `name[64]` | `desc.output_name()` |
| 0x4C | `length` | 实际长度 |
| 0x50 | `eyeposition` | `model.eye_position`，缺省 0 |
| 0x5C | `illumposition` | `model.illum_position`，缺省 0（**无轴交换**，见 4.4） |
| 0x68/0x74 | `hull_min/max` | 显式值或顶点包围盒 |
| 0x80/0x8C | `view_bbmin/max` | **恒写 0** |
| 0x98 | `flags` | `extra_flags \| STATIC_PROP(1<<4)` |
| 0x9C/0xA0 | `numbones`/`boneindex` | ✅ |
| 0xCC/0xD0 | `numtextures`/`textureindex` | ✅ |
| 0xD4/0xD8 | `numcdtextures`/`cdtextureindex` | ✅ |
| 0xE8/0xEC | `numbodyparts`/`bodypartindex` | ✅ |
| 0x134 | `surfacepropindex` | ✅ |
| 0x148 | `mass` | 缺省 **1.0** |
| 0x14C | `contents` | 缺省 0 |

**其余全部头部字段显式写 0**（`mdl_writer.rs:524-577` 的循环，共 47 个偏移），
包括 `bonecontroller` / `hitboxset` / `localanim` / `localseq` / `skin` /
`attachment` / `localnode` / `flexdesc` / `flexcontroller` / `flexrule` /
`ikchain` / `mouth` / `poseparam` / `keyvalue` / `ikautoplaylock` /
`includemodel` / `animblock` / `bonetablebyname` / `studiohdr2`。

## 1.5 骨骼表：实际写了什么

`mdl_writer.rs:595-673`，每根骨骼写：

| 字段 | 状态 |
|---|---|
| `sznameindex` | ✅ 相对骨骼自身的偏移（注释 `mdl_writer.rs:146-158` 记录了实测依据） |
| `parent` | ✅ |
| `bonecontroller[6]` | ❌ 写 0 |
| `pos` | ✅ 来自 TOML 或 SMD 第 0 帧 |
| `quat` | ❌ **写 0**（`mdl_writer.rs:613` 注释写「引擎读的是 rotation」——**这条注释是错的**，见 4.3） |
| `rot` | ✅ |
| `posscale`/`rotscale` | ✅ 恒 1.0 |
| `poseToBone` | ✅ 由 `bone_math::compute_pose_to_bone` 计算（`mdl_writer.rs:646-673`） |
| `qAlignment` | ❌ 写 0 |
| `flags` | ⚠️ 缺省硬编码 `0x500`（`DEFAULT_BONE_FLAGS`），**不做 `BONE_USED_BY_*` 计算** |
| `proctype`/`procindex` | ❌ 写 0 |
| `physicsbone` | ❌ 写 0 |
| `surfacepropidx` | ✅ 缺省继承头部 |
| `contents` | ✅ 继承头部 |

**`poseToBone` 的实现是正确的**（这是本轮调研最重要的正向确认）：
`bone_math.rs:57-76` 的 `angle_matrix` 用 Source 约定 `R = Rz·Ry·Rx`，
`compute_pose_to_bone`（`bone_math.rs:113`）正向累积 `world = parent_world ∘ local`，
再取 `invert`（`bone_math.rs:74`，即 `[Rᵀ | −Rᵀ·t]`）。
我用官方 `v_autoshotgun.mdl` 的 89 根骨骼逐根比对，**0 根不符，最大偏差 1.18e-05**（纯 f32 舍入）。

## 1.6 材质表 / bodypart 树：实际写了什么

- 材质表（`mdl_writer.rs:665-676`）：只写 `sznameindex`（相对自身）与 `flags`。`used` 写 0。
- bodypart（`mdl_writer.rs:681-697`）：`sznameindex`（相对自身）、`nummodels`、`base`、`modelindex`（相对自身）。
- model（`mdl_writer.rs:699-730`）：`name[64]`、`nummeshes`、`meshindex`（相对 model）、`numvertices`、`vertexindex`（**相对 VVD 顶点块的字节偏移** = `start × 48`）。`type`/`boundingradius`/`tangentsindex`/`attachment`/`eyeball` 全写 0。
- mesh（`mdl_writer.rs:733-765`）：`material`、`modelindex`（写 `-148`）、`numvertices`、`vertexoffset`、`numflexes`/`flexindex`/`materialtype`/`materialparam` 写 0、`numbones` + `boneids[8]`（从顶点绑定去重推导）。

## 1.7 VVD 写出：实际写了什么

`vvd.rs`（462 行）是**唯一**经过逐字节往返验证的模块：

- 布局（`vvd.rs:1-46` 模块文档）：头部 64 字节、顶点 stride 48、切线 16 字节。
- **官方 `v_autoshotgun.vvd`（24,881,024 字节）读入再写出逐字节相同**
  （测试 `lib.rs:205` `real_l4d2_vvd_round_trips_byte_for_byte`，硬编码锚点 `lib.rs:96-100`）。
- 写出时只支持 `numFixups == 0`（`vvd.rs:303-310` 显式拒绝）。
- `from_vertices`（`vvd.rs:352`）在 `tangents = None` 时**由法线派生占位切线**（`vvd.rs:365-388`），
  并在注释里承认「官方产物里切线是真实计算的……留给后续阶段做对」。
- `num_lod_vertexes` **8 个槽位全部填同一个值**（`vvd.rs:394`）——这条是实测纠错，注释记录了依据。

## 1.8 校验强度（值得肯定的部分）

`model.rs:374-571` 的 `validate()` 相当严格：骨骼名重复、父骨骼前向引用、
骨骼下标越界、权重和不为 1、每顶点骨骼数 > 3、三角形退化、材质下标越界、
非有限数、版本白名单（44/48/49）。`vvd.rs:415` 的 `check_invariants` 校验头部等式。
`mdl_writer.rs` 有大量硬编码偏移断言测试（如 `mdl_writer.rs:940-957`）。

**这套「宁可报错也不写坏文件」的设计是本项目最有价值的资产** ——
因为原版 studiomdl 的失败模式恰恰是大量**静默**产出坏模型（见 0.2）。

## 1.9 当前能力的边界（一句话）

`mdlc` 能产出的最好结果是：**一个只有静态几何、单一 LOD、无 VTX、无 PHY、
无动画、无表情、无 hitbox、骨骼标志位不计算的 `.mdl` + `.vvd` 对**。
按 Source 引擎的加载要求，这个产物**缺少必需文件 `.vtx`**，因此**无法在游戏中渲染**（见 3.4）。

---

# 第二部分：缺失特性清单

> **难度评级口径**（主观，但给出依据）：
> - **低**：结构已知、逻辑直白、有逐字段参考（如 `studio.h`）
> - **中**：结构已知但语义需要实测反推，或涉及一定算法
> - **高**：算法本身是黑盒，需要从真实产物大量反推
> - **极高**：既黑盒又缺乏足够样本，或需要复现 Valve 未公开的启发式

---

## 第 1 层：输入格式

| # | 名称 | 作用 | 缺失后果（用户可感知） | 难度 | 公开参考 |
|---|---|---|---|---|---|
| 1.1 | **QC 语法解析** | 真实工作流的入口格式 | 用户**无法编译任何现成的 QC 工程**；必须手工把 QC 翻译成 TOML，几万顶点的模型不可行 | 中 | 无权威语法规格；`qc-generation.md` 只给输出形状；命令名可从 exe 提取 |
| 1.2 | QC 块语法 `{ }` | `$bodygroup`/`$sequence`/`$collisionmodel` 等 | 同上 | 低 | 同上 |
| 1.3 | `$include` / QCI | 多文件组织 | 无法编译分文件的工程 | 低 | 命令存在性已确认；**搜索路径/递归规则未验证** |
| 1.4 | `$pushd` / `$popd` / `$cd` | 目录栈 | 相对路径解析错误 | 低 | 命令存在性已确认；**语义未验证** |
| 1.5 | `$definevariable` / `$definemacro` | 变量与宏展开 | 参数化的 QC 完全无法编译 | 中 | 命令存在性已确认；**语法未验证** |
| 1.6 | **DMX 输入** | 现代导出器（Blender/Maya）的格式 | 只能用 SMD；DMX 用户必须先转格式 | 高 | 原版**委托子进程** `dmxconvert.exe`（实测存在），重写可同样委托 |
| 1.7 | SMD `vertex` 段 | 顶点动画 | 顶点动画模型无法编译 | 中 | `smdlexp.cpp` 有导出侧 |
| 1.8 | SMD 多帧 skeleton | 序列动画的来源 | **解析了却丢弃**：动画数据在 IR 里但没进任何输出 | 中 | `smd.rs:91-95` 已解析；`animation-command.md` 只讲 Crowbar 方向 |
| 1.9 | **VTA** | flex 顶点动画 | 表情系统完全不可用 | 高 | **无任何文档覆盖 VTA 二进制格式**（已确认 `.tmp/research/` 下无此文档） |
| 1.10 | **VRD** | 程序化骨骼定义 | 抖动骨骼/程序化骨骼不可用 | 中 | `vrd-generation.md` 有文本格式 + QUATINTERP/AIMAT/AXISINTERP 结构体 |
| 1.11 | `$scale` / `$origin` | 全局缩放与偏移 | 无法复用不同尺度的源文件 | 低 | 命令存在性已确认；**语义未验证** |
| 1.12 | `$upaxis` | Y 轴向上声明 | 坐标轴约定错误 | 低 | `qc-generation.md` §2.3 有；注意它会**同时**驱动 SMD 的 Y/Z 交换 |

**本层小结**：`mdlc` 支持 **2 项**（TOML、SMD 网格），
**部分支持 1 项**（SMD skeleton 仅第 0 帧），**完全缺失 9 项**。

---

## 第 2 层：QC 命令覆盖

### 2.0 命令总数：**137 条，已由二进制调度表确认**

原版 exe 的 QC 命令表没有公开清单。本次调研用**两种独立方法**提取，
其中第二种达到了「二进制事实」的强度：

**方法 A（我做的字符串扫描）**：提取全部 NUL 结尾可打印 ASCII 串（长度 ≥ 3，共 121,410 个），
按**大小写敏感**正则 `^\$[a-z][a-z0-9_]{1,30}$` 过滤去重 → **157 个候选**。
再用「偏移聚集分析」定位命令表区域（主表 `5738400..5739700`）→ 聚集区内 **73 个**。

**方法 B（子调研做的调度表定位，我独立复核通过）**：找到 studiomdl 的
**命令分派表（dispatch table）**：

| 项 | 值 |
|---|---|
| 表位置 | `.data` 文件偏移 `0x6E6988`，共 **104 条 × 12 字节 = 1248 字节**，连续无空隙 |
| 记录布局 | `{DWORD 名字VA; DWORD 处理函数VA; DWORD 0}` |
| 名字 | **104 个，全部唯一，全部以 `$` 开头** |
| 处理函数 | **103 个唯一**（`$hierarchy` 与 `$heirarchy` 共享 `0x0044C1A0`），全部落在 `.text` 内 |
| 第三字段 | **全部为 0** |

**我独立复核的结果（全部通过）**：
- 104 条、104 个唯一名字、103 个唯一 handler ✓
- 所有 handler 落在 `.text`（`0x401000..0x56F67B`）✓
- 第三个 DWORD 全为 0 ✓
- `$hierarchy`(索引 63) 与 `$heirarchy`(索引 64) 确实共享 `0x0044C1A0` ✓
- **只有 `$skinnedLODs` 一个名字含大写字母** ✓
- 表内 104 个名字**全部**出现在子调研的 137 清单中，无遗漏、无多余 ✓

> **对子调研报告的一处更正**：其报告的 VA 列（如 `0x00AE7D88`、`0x0097A7E4`）
> **一律比真实 VA 大 `0x400000`**。正确映射是
> `.rdata`/`.data` 均为 `file = VA − 0x1400`，故表基址真实 VA 是 **`0x6E7D88`**。
> **其「文件偏移 `0x6E6988`」是对的**，且该笔误不影响任何结论 ——
> 我用其文件偏移 + 正确的 VA 映射重算，104 条全部解析成功。

**最终口径（三档相加，无重叠）**：

| 档 | 内容 | 条数 |
|---|---|---|
| (a) | 调度表内的顶层命令 | **104** |
| (b) | `$collisionjoints` 块内关键字（不在调度表，仅在该块内合法） | **27** |
| (c) | 独立关键字（不在调度表，有各自的比较点） | **6** |
| | `$include` `$definevariable` `$definemacro` `$decal` `$vertexcolor` `$ignorez` | |
| **合计** | | **137** |

我复核了 (b)+(c) = 33 条与「104 表之外」的差集**完全吻合**：
33 条恰好是那 27 个碰撞关键字 + 6 个独立关键字，**零遗漏零多余**。

**这与项目既有文档的「约 140 条」一致，且现在有了二进制层面的确证。**
本报告后续一律采用 **137**。

**同时排除的噪声**（子调研逐一定位并检查了周围字节）：
- **28 个非 NUL 分隔的碎片**。其中 19 个是 `.text` 里的 x86 代码 ——
  `0x24` 是 ModRM/SIB 位移字节，紧随其后的 opcode 恰好是小写字母，
  例如 `83 EC 08 DD 1C 24 68 30 D9 99 00` 会伪造出 `$h0`。
  这解释了我在方法 A 里看到的 `$h0 $h4 $h8 $hd $hh $hl $hp $ht $hx $km $lm $qm $sm` 等
  「命令」的真实来源。另有 6 个是 `.rdata` 任意二进制、2 个在 `.reloc` 里。
- printf/错误格式串（`"$BoneSaveFrame \"%s\""` 等）、AutoCAD DXF 变量
  （`$ACADVER $UCSORG $UCSXDIR $UCSYDIR $TILEMODE`）、PE 导入名装饰
  （`$WriteConsoleW` 等）、`$$$DUMMY` 占位符。

> **这条对我方法 A 的启示**：我的 157 个候选里那 24 个「疑似噪声」
> （`$ak $h0 $h4 $h8 $hh $hl $hp $ht $hx $km $lm $qm $sm` 等）**全部是这 28 个碎片**，
> 不是命令。方法 A 只能给上界，方法 B 才能给准确值。

### 2.1 逐组统计

下表分母为**子调研的机器校验分组**（分组是判断，非二进制事实；
脚本对「未分配/重复分配/不存在的 token」会抛错，已确认 137 条各恰好分配一次）。

| 功能组 | 原版（137 口径） | `mdlc` 支持 | 部分支持 | 缺失 | 备注 |
|---|---|---|---|---|---|
| **模型定义** | 38 | 6 | 2 | 30 | 支持：`$modelname` `$staticprop`（**含几何 `Rz(90°)` 旋转 + 骨骼塌缩 + 动画塌陷**，见 `coordinate-systems.md` §7）`$surfaceprop` `$mass` `$contents` `$bbox`；部分：`$illumposition` `$eyeposition`（**轴交换已实现**） |
| **材质** | 10 | 1 | 0 | 9 | 支持：`$cdmaterials` |
| **骨骼** | 24 | 1 | 1 | 22 | 部分：`$definebone`（仅 name/parent/pos/rot，**无 6 个 fixup 值**） |
| **动画序列** | 10 | 0 | 0 | 10 | 全缺 |
| **flex 与面部** | **3** | 0 | 0 | 3 | **注意：只有 3 条** —— `$eyeposition` `$maxeyedeflection` `$forcephonemecrossfade`。**根本不存在 `$flex*` 命令**，见 2.2 |
| **碰撞与物理** | 42 | 0 | 0 | 42 | **本组命令数最多**，全缺 |
| **IK** | 2 | 0 | 0 | 2 | `$ikchain` `$ikautoplaylock`（`ikrule` 是块内子关键字，不单独计数） |
| **附着点与 hitbox** | 3 | 0 | 0 | 3 | `$attachment` `$hboxset` `$hbox` |
| **LOD** | 5 | 0 | 0 | 5 | `$lod` `$shadowlod` `$minlod` `$allowrootlods` `$alwayscollapse` |
| **杂项** | 0 | — | — | — | 该分组为空 —— 所有命令都归入了具名组 |
| **合计** | **137** | **约 8** | **约 3** | **约 126** | |

> **统计口径声明**：
> - 分母 **137** 有二进制依据（104 调度表 + 27 碰撞关键字 + 6 独立关键字），已独立复核。
> - **分组归属是子调研的判断，不是二进制事实**（二进制里没有分组元数据）。
>   边界项已在 `qc_commands_grouped.md` 里逐条给出理由，例如
>   `$rootbone` 归入碰撞组（它是 `$collisionjoints` 的子项，不是骨骼命令）、
>   `$controller`/`$poseparameter`/`$weightlist` 归入骨骼组
>   （`$poseparameter` 也可争论应归 flex 组，因为它驱动 flex controller，但它声明在骨骼层）。
> - 「支持」定义为：TOML 有对应字段**且**该字段确实写进了 MDL 的对应结构。
> - 「部分支持」定义为：有对应字段但语义不完整。
> - **覆盖率约 6%–8%**（8/137 ≈ 5.8%，含部分支持 11/137 ≈ 8.0%）。

### 2.2 两个由调度表得出的事实（修正了先前的假设）

**(a) 根本不存在 `$flex*` 命令。**
137 条里**没有** `$flex`、`$flexfile`、`$flexcontroller`、`$flexrule`、`$flexpair` 中的任何一条。
面部相关命令只有 3 条：`$eyeposition`、`$maxeyedeflection`、`$forcephonemecrossfade`。

这**印证了** `qc-generation.md` 的说法（「`$flex`/`$flexcontroller`/`$flexrule` 作为顶层命令
在 v49 中不被输出」），但给出了更强的原因：**它们在这个编译器里根本不存在**。
flex 数据来自 `.vta` / DMX，不是来自 QC 文本。
**对 `mdlc` 的影响**：实现 flex 不需要新增 QC 命令，需要的是**读取 VTA 与 DMX**。

**(b) IK 只有 2 条顶层命令，且 `ikrule` 等子关键字不是独立字符串。**
子调研对 `.rdata` 做了穷举扫描，确认 `ikrule`/`iklock`/`footstep`/`release`/`unlatch` 等
**不以独立的 NUL 结尾字符串存在** —— 它们只在 `$ikchain`/`$sequence` 的解析上下文里被识别。
**对 `mdlc` 的影响**：IK 命令面比预想小，但**语义面**（块内子关键字的合法性规则）更难，
因为没有可枚举的清单可依。

**(c) 27 个碰撞关键字不在调度表里。**
它们位于 `.rdata` 的一个连续块（子调研报告 VA `0x00972608..0x0097278C`），
每个都与当前 token 做直接 `push imm32` 比较 —— 即**只在 `$collisionjoints` 块体内合法**。
这解释了为什么 `$mass`/`$inertia`/`$damping` 等既出现在顶层也可能出现在块内：
**它们有两个独立的识别点**。

### 2.3 最致命的三个命令缺口

1. **`$collisionmodel` / `$collisionjoints` 全组（约 30 条）**
   —— 没有任何碰撞体，模型**无法作为物理实体存在**（可穿过、无弹道、无布娃娃）。
2. **`$sequence`（1 条，但牵动 12 条动画组）**
   —— 没有序列 = **纯静态模型**。任何需要 `ACT_*` 活动的逻辑（开门、射击、换弹）全部失效。
3. **`$attachment`**
   —— 无法挂载任何东西（枪口火焰、手电、武器挂点）。L4D2 的武器模型**全部**依赖它。

### 2.4 `$hboxset` 的实测必要性

`mdl-layout.md` 未给出「必需性」判定。我对真实素材做了频率统计（该统计由子调研完成，
样本 `D:\SOURCE\SOURCEMDLS` 的 800 个 MDL）：

| 头部段 | 非零样本数 / 800 | 推断必要性 |
|---|---|---|
| `numhitboxsets` | **800 / 800** | **必需**（命中判定） |
| `mass` / `contents` | 800 / 800 | 必需 |
| `illumposition` | 743 / 800 | 软必需 |
| `studiohdr2index` | 748 / 800（值 = 408） | 软必需 |
| `flags & STATIC_PROP` | 587 / 800 | 软必需 |
| `numanimblocks` | 80 / 800 | 有则为硬必需（`.ani` 随之必需） |
| `numlocalattachments` | 140 / 800 | 常用 |
| `numkeyvalues` | 77 / 800 | 常用 |
| `numskinfamilies > 1` | 41 / 800 | 少用 |
| `numbonecontrollers` | **0 / 800** | 可永久推迟 |

**结论**：`mdlc` 完全不写 `hitboxset` 是一个**高优先级缺陷** —— 真实模型 100% 都有。

---

## 第 3 层：输出文件

### 3.1 MDL —— 未写出的段（完整清单）

| 段 | 结构 | 大小 | 缺失后果 | 难度 | 参考 |
|---|---|---|---|---|---|
| bonecontroller | `mstudiobonecontroller_t` | 56 | `$controller` 不可用。实测 800/800 为 0 → **可推迟** | 低 | `studio.h:343` |
| **hitboxset** | `mstudiohitboxset_t` + `mstudiobbox_t` | 12 + 68 | **命中判定失效**，子弹穿透 | 低 | `studio.h:1550/356` |
| **localanim** | `mstudioanimdesc_t` | 100 | **无动画** | 极高（位流编码） | `studio.h:626` |
| **localseq** | `mstudioseqdesc_t` | 212 | **无活动** | 高 | `studio.h:695` |
| skin | `short[numskinref]` | — | **皮肤族不可切换**（实测 41/800 有多族） | 低 | `studio.h:220-228`；**实测元素是 uint16 不是 int32**（见 3.5） |
| attachment | `mstudioattachment_t` | 92 | **无法挂载** | 低 | `studio.h:414` |
| localnode | 原始字节数组 | — | 转移图失效 | 中 | `studio.h:2095` |
| flexdesc | `mstudioflexdesc_t` | 4 | 表情失效 | 高 | `studio.h:810` |
| flexcontroller | `mstudioflexcontroller_t` | 20 | 表情失效 | 高 | `studio.h:819` |
| flexrule | `mstudioflexrule_t` + `mstudioflexop_t` | 12 + 8 | 表情失效 | 高 | `studio.h:1074/1063` |
| flexcontrollerui | `mstudioflexcontrollerui_t` | 20 | SFM 表情 UI 失效 | 中 | `studio.h:842` |
| ikchain | `mstudioikchain_t` + `mstudioiklink_t` | 16 + 28 | 脚部 IK 失效（实测 36/800） | 中 | `studio.h:1178/1165` |
| mouth | `mstudiomouth_t` | 20 | 口型失效（实测 10/800） | 中 | `studio.h:1537` |
| poseparam | `mstudioposeparamdesc_t` | 20 | 姿势参数失效（实测 37/800） | 低 | `studio.h:799` |
| keyvalues | 文本 | — | 实体 KV 失效（实测 77/800） | 低 | `studio.h:312-316` |
| ikautoplaylock | `mstudioiklock_t` | 32 | — | 低 | `studio.h:512` |
| includemodel | `mstudiomodelgroup_t` | 8 | 模型组合失效 | 中 | `studio.h:382` |
| animblock | `mstudioanimblock_t` | 8 | `.ani` 外置动画 | 高 | `studio.h:612` |
| **studiohdr2** | `studiohdr2_t` | 256 | 见 3.2 | 低 | `studio.h:1941` |
| bonetablebyname | `byte[numbones]` | — | 按名查骨骼失效 | 低 | `studio.h:364` |
| linearbone | `mstudiolinearbone_t` + 9 数组 | 68+ | 加速结构，实测 748/800 存在 | 中 | `studio.h:266` |

### 3.2 `studiohdr2` —— 实测确认它在真实模型里**确实存在且非空**

我逐字段 dump 了官方 `v_autoshotgun.mdl` 的 `studiohdr2`（`studiohdr2index = 408`，紧跟头部之后）：

```
+0x00 numsrcbonetransform      = 0
+0x04 srcbonetransformindex    = 602596   （计数为 0，此值无意义）
+0x08 illumpositionattachmentindex = 0
+0x0C flMaxEyeDeflection       = 0
+0x10 linearboneindex          = 602188   → 绝对偏移，存在完整线性骨骼表
+0x14 sznameindex              = 614357   → 指向 "v_models\v_autoshotgun.mdl"，覆盖 header.name
+0x18..+0x3C                    = 0
```

**两个可操作的结论**：
1. `studiohdr2.sznameindex` 会**覆盖** `studiohdr_t.name`。`mdlc` 只写后者，引擎读到的名字来源不同。
2. `studiohdr2.linearboneindex` 在真实模型里**非空**。这是引擎的加速路径。
   （注意：`linearboneindex` 是**相对 `studiohdr2` 自身**的偏移，不是绝对偏移。）

**但有一个重要的兼容性陷阱**：`mdl-layout.md:184` 指出 Crowbar **从不 seek 到 `studiohdr2index`**，
而是假定它就在 `0x198` 顺序读。实测 L4D2 恰好 `== 408`，所以 Crowbar 侥幸正确。
**若 `mdlc` 把 `studiohdr2` 放在别处，会破坏 Crowbar 兼容性** —— 建议固定写 408。

### 3.3 VVD

| 缺失项 | 后果 | 难度 | 参考 |
|---|---|---|---|
| **fixup 表**（`numFixups > 0`） | `mdlc` 显式拒绝（`vvd.rs:303-310`）。实测 792 个真实 VVD 中 **23 个（约 2.9%）** 有 fixup，**全部是多 LOD 模型**。有 fixup 时顶点块按 LOD 分段排序，直接顺序索引会**得到完全错乱的面** | 中 | `vvd-vtx-layout.md:56-64, 124-130` |
| **多 LOD**（`numLODs > 1`） | 无法做 LOD 切换；实测约 **10%** 的真实模型是多 LOD | 高 | 同上 `:121-144` |
| **真实切线** | `mdlc` 用「法线叉积」占位（`vvd.rs:365-388`）。后果：**法线贴图（bump mapping）错误/发黑** | 中 | `vvd-vtx-layout.md:88-97` |
| 多 UV 集 | **实测 792 个 VVD 中 0 个含额外 UV 数据** → L4D2 不产出，**可永久跳过** | — | `vvd-vtx-layout.md:99-117` |
| 顶点去重/索引化 | `mdlc` 不做去重：SMD 每个三角形顶点直接成为一个 VVD 顶点 → **顶点数膨胀**、文件变大、与 studiomdl 产物无法逐字段对齐 | 中 | 无公开规格；需实测反推 studiomdl 的哈希顺序 |

**关于 UV 的一个实测提醒**：VVD 里的 UV **已归一化**，且**会超出 [0,1]**
（实测 `v_autoshotgun` 的 u 最大 **1.99414**）。Source 用平铺表达重复贴图，
**写作者不得 clamp**。`mdlc` 目前不做 clamp，这点是对的。

### 3.4 VTX —— 完全缺失（重点）

**VTX 是什么**：优化后的**索引/拓扑**文件。三个文件的分工是：

- `.mdl` —— 几何**归属**（哪个 mesh 用哪个材质、骨骼表、LOD 描述）
- `.vvd` —— 顶点**属性**（位置/法线/UV/骨骼权重）
- `.vtx` —— **索引**：哪个 mesh 用哪些顶点、按什么顺序组成三角形、每个顶点怎么绑骨骼

**为什么必需**：`mdl-layout.md:764` 记录引擎错误 `ErrorRequiredVtxFileNotFound`。
没有 VTX，引擎**无从知道如何索引 VVD**，模型**完全不渲染**。
这直接决定了 `mdlc` 当前产物**无法在游戏中显示** —— 这是它最严重的单一缺陷。

**L4D2 的 VTX 布局（v7）**：

| 结构 | 大小 | 备注 |
|---|---|---|
| `FileHeader_t` | 36 | `0x08`/`0x0A` 是 **uint16**（`maxBonesPerStrip`/`maxBonesPerTri`），误当 int32 读会让整个头部错位 |
| `BodyPartHeader_t` | 8 | |
| `ModelHeader_t` | 8 | |
| `ModelLODHeader_t` | 12 | 含 `switchPoint` float |
| `MeshHeader_t` | **9** | **packed，无填充** |
| `StripGroupHeader_t` | **25** | L4D2 是 25，**不是** 33 |
| `Vertex_t` | **9** | **packed，无填充** |
| `StripHeader_t` | **27** | L4D2 是 27，**不是** 35 |
| `BoneStateChangeHeader_t` | 8 | |

**我独立实测确认了头部**（解析 `v_autoshotgun.dx90.vtx`，4,894,107 字节）：

```
version            7
vertexCacheSize    24
maxBonesPerStrip   53      （uint16）
maxBonesPerTri     9       （uint16）
maxBonesPerVertex  3
checksum           -1709441603   ← 与 .mdl 的 checksum 完全一致
numLODs            1
matReplListOffset  4894099
bodyPartCount      9
bodyPartOffset     36
```

**一个与预期不符的重要事实**：`mdl-oracle\src\vtx.rs:21-24` 与
`vvd-vtx-layout.md:228/259/317` 都声称 **MDL v49+ 的 VTX 用 33/35 字节**。
子调研对 786 个真实 VTX 做了统计：**25/27 有 775 个零违规，33/35 只有 735 个且 40 个明确失败**。
并且 `hl2sdk-l4d2\public\optimize.h` **没有** `numTopologyIndices`/`topologyIndexOffset` 字段
（该扩展只存在于 CSGO/Alien Swarm 分支）。
**即：L4D2 用的是 25/27，`plank/src/mdl/vtx.rs` 的版本判定对 L4D2 是错的。**
（本条为子调研结论 + 我独立复核了头字段；25/27 的结论我未亲自逐字节复算。）

**难度评估：中（而非「极高」）** —— 这是本轮调研最有价值的修正之一：

1. 布局固定 25/27，**无版本分支**。
2. 实测 786 个 VTX 的 strip flags：`0x01`(TRILIST) **3720 个**、`0x00` 21 个、
   **`0x02`(TRISTRIP) 0 个** → **L4D2 全是 tri-list，不需要实现 strip 生成算法**。
3. `StripHeader.indexOffset` 与 `vertOffset` 在 3684/3692 个样本中为 0
   → 每个 strip group 实际只有一个 strip 且从数组头开始 → **扁平 index 数组直接可用**。
4. 只有 **56/3631** 个 mesh 有 >1 个 strip group。

**真正需要小心的三件事**：(a) 偏移基准逐层下降（头部两个字段相对文件起始，
其余相对各自父结构）；(b) `MeshHeader_t` 9 字节、`Vertex_t` 9 字节**无填充**；
(c) 块间 4 字节对齐。

### 3.5 PHY —— 完全缺失

- **`.phy` 从来不是加载必需**（`mdl-layout.md:771`「required: NO (never checked)」）。
  实测 800 个模型里 **420 有 / 380 无**。
- 但**没有它就没有物理**：模型可被穿过、无弹道、无布娃娃、无车辆物理。
- 需要：凸包分解（`$concave`/`$maxconvexpieces`）、关节约束（`$jointconstrain`）、
  质量分布（`$jointmassbias`）、自碰撞（`$noselfcollisions`/`$jointcollide`）。
- **难度：极高**。原版链接了 `vphysics.dll`（实测导入表含 `vphysics`），
  凸包分解是 Valve 未公开的实现。
- **可行退路**：委托外部工具或 `vphysics` 的 `IVPhysics2Collide`（与 `dmxconvert` 同样的策略）。

### 3.6 ANI —— 完全缺失

- 外部动画块。`numanimblocks > 0` 时 **`.ani` 变成必需文件**
  （`mdl-layout.md:780-786`，错误 `ErrorRequiredAniFileNotFound`）。实测 **80/800** 非零。
- `mdlc` 写 `numanimblocks = 0`，所以**不会**触发这个需求 —— 这是**当前唯一「正确」的地方**。
- 一旦实现动画，就必须同时决定「内联还是外置」。

---

## 第 4 层：编译期计算

### 4.1 骨骼逆变换（`poseToBone`）—— **已正确**，但要记录曾经的错误

**原版语义**（`hl2sdk-episode1\utils\studiomdl\write.cpp:200`）：
```cpp
200: 		MatrixInvert( g_bonetable[i].boneToPose, pbone[i].poseToBone );
```
即 `poseToBone = MatrixInvert(boneToPose)`，而 `boneToPose` 由
`BuildGlobalBoneToPose()`（`simplify.cpp:3723-3739`）累积：
```cpp
3730: 		if (g_bonetable[k].parent == -1) {
3732: 			MatrixCopy( g_bonetable[k].rawLocal, g_bonetable[k].boneToPose );
3734: 		else {
3736: 			ConcatTransforms (g_bonetable[g_bonetable[k].parent].boneToPose, g_bonetable[k].rawLocal, g_bonetable[k].boneToPose);
```
其中 `rawLocal = AngleMatrix(rot, pos)`（`simplify.cpp:3682`），是**完整的旋转+平移矩阵**。

`MatrixInvert` **不是通用求逆，而是转置型刚体逆**
（`mathlib_base.cpp:377` 注释原文：「NOTE: This is just the transpose not a general inverse」）：
```
409: 	out[0][3] = -DotProduct( tmp, out[0] );
410: 	out[1][3] = -DotProduct( tmp, out[1] );
411: 	out[2][3] = -DotProduct( tmp, out[2] );
```
即 `[Rᵀ | −Rᵀ·t]`。**平移是 `−Rᵀ·t`，不是 `−t`** —— 这是最容易写错的一点。

`matrix3x4_t` 的存储是 **12 个连续 float、行主序**（`mathlib.h:160` `float m_flMatVal[3][4]`），
平移在平坦下标 **3/7/11**。

**实测验证**：我用官方 `v_autoshotgun.mdl`（89 骨骼）独立复算，
`poseToBone == invert(累积世界变换)` **0/89 不符，最大偏差 1.18e-05**（纯 f32 舍入）。

**曾经的缺陷（已由并发 agent 修复，记录备查）**：
早先版本用「单位旋转 + 平移取负」，对 `rotation = 0` 的合成模型看不出问题。
更早的一版 `bone_math.rs` 的 `angle_matrix` 写成了 Source 矩阵的**转置**，
导致 89/89 骨骼全错、最大偏差 111.22。该 bug 在本次调研期间被修复并验证。

**一个重要的范围澄清**：`poseToBone` **不是**正常渲染/动画路径使用的量。
它在整个 SDK 里**只有一个消费者** —— `bone_setup.cpp:1731-1738`
`Studio_CalcBoneToBoneTransform`，被布娃娃约束（`ragdoll_shared.cpp:237`）
和 strider IK（`npc_strider.cpp:4299`）调用。
正常路径用的是 `mstudiobone_t.pos` / `.quat`（`bone_setup.cpp:1757-1758` 的 `InitPose`，
经 `CalcBonePosition`/`CalcBoneQuaternion` → `BuildBoneChain` → `Studio_BuildMatrices`）。
**所以 `poseToBone` 错 → 布娃娃和 IK 坏；`quat` 错 → 一切都坏。**

### 4.2 `$definebone` 语义

原版 `$definebone` 有 **14 个参数**（`bone parent px py pz ry rz rx fx fy fz frx fry frz`），
最后 6 个是**源骨骼变换 fixup**，用于把动画从一套骨骼命名重映射到另一套。
`mdlc` 的 `Bone` 只有 `name`/`parent`/`position`/`rotation`/`flags`/`surface_prop`
（`model.rs:111-129`）—— **缺 6 个 fixup 值**。
后果：`$definebone` 的重映射能力不可用；且**无 `mstudiosrcbonetransform_t` 表**
（`studio.h:1565`，100 字节/项）。
注意 `qc-generation.md` 证实 Crowbar 写出的 fixup **恒为 0**，所以真实场景里这个缺口影响有限，
但**无法处理真正需要 fixup 的模型**。

### 4.3 `quat` 字段 —— **当前写 0，这是明确的 bug**

`mdl_writer.rs:613` 的注释写「quat 留 0（引擎读的是 rotation）」。
**这条注释是错的。** 原版（`write.cpp:203`）：
```cpp
AngleQuaternion( RadianEuler( rot[0], rot[1], rot[2] ), pbone[i].quat );
```
即 `quat = AngleQuaternion(rot)`。

**实测验证**：官方 `v_autoshotgun.mdl` 的 89 根骨骼，
`quat == AngleQuaternion(rot)` 的最大偏差 **8.21e-08**（纯 f32 舍入，允许整体符号翻转）。
例如 bone[0]：`rot = (1.570796, 0, 0)` → `quat = (0.7071066, 0, 0, 0.7071069)`。

**后果**：运行时 `InitPose` 直接读 `pbone->quat`（`bone_setup.cpp:1757-1758`）。
`quat = 0` 不是合法四元数（模长为 0），骨骼姿态求解会退化。
**这是当前 `mdlc` 第二严重的正确性缺陷**（仅次于没有 VTX）。

### 4.4 `$illumposition` 轴交换 —— **缺失**

原版 `studiomdl.cpp:867-882`：
```cpp
illumposition[1] = verify_atof (token);   // 读第 1 个数 → 写到 y
illumposition[0] = -verify_atof (token);  // 读第 2 个数 → 取负写到 x
illumposition[2] = verify_atof (token);   // 读第 3 个数 → 写到 z
```
即 QC 的 `$illumposition a b c` 落到文件里是 `(x,y,z) = (−b, a, c)` —— 一个 **90° Z 旋转**。
`mdlc` 的 `put_vec3(&mut buf, off::ILLUM_POSITION, desc.model.illum_position...)`
（`mdl_writer.rs:497-501`）**原样写入，没有轴交换**。
`$eyeposition` 同理（`write.cpp:2068`）。

**实测旁证**：官方 `v_autoshotgun.mdl` 的 `illumposition = (20.977, -0.765, -31.64)` 非零；
800 个样本中 **743 个非零** → 这个字段几乎总是有值。

**缺失后果**：静态光照中心错误 → 光照不均匀/穿帮。用户不易察觉但可见。
**另一个缺口**：`$illumposition` 缺省时原版会用**序列 0 包围盒的中心**
（`simplify.cpp:7204-7218`）；`mdlc` 直接写 0。

### 4.5 包围盒自动生成

`mdlc` 有 `bounds()`（`model.rs:247` 附近的 `CompiledModelDesc::bounds`），
由顶点算 min/max，并用于 `hull_min`/`hull_max`（`mdl_writer.rs:493-495`）。
**部分正确，但缺三点**：
1. **`view_bbmin`/`view_bbmax` 恒写 0**（`mdl_writer.rs:507-508`）。
   实测 800 个样本中只有 2 个非零 → **优先级低**。
2. 原版的 hull 会考虑**骨骼运动范围**（`$sequence` 的 bbox 并集），不只是静态顶点。
   没有动画时看不出差别。
3. `$hbox` 的 `$skipboneinbbox` 会**排除**某些骨骼对 bbox 的贡献 —— 未实现。

### 4.6 hitbox 自动生成 —— 缺失

原版在 QC 未提供 hitbox 时**自动生成**，并置 `STUDIOHDR_FLAGS_AUTOGENERATED_HITBOX (1<<0)`。
实测 800 个样本中 **160 个**带这个标志 → 约 20% 的模型依赖自动生成。
`mdlc` 完全不写 hitboxset（`numhitboxsets = 0`）→ **命中判定失效**。

### 4.7 LOD 生成与合并 —— 缺失

需要：按 `switchPoint` 分组、逐 LOD 生成/合并网格、`BONE_USED_BY_VERTEX_LODn` 标记、
`$shadowlod`（`switchPoint = -1`，必须排最后）、VVD fixup 表、VTX 每 LOD 完整 mesh 列表。
**难度：高**。实测约 10% 的真实模型是多 LOD。

### 4.8 strip 生成 —— **实测：L4D2 不需要**

如 3.4 所述，786 个真实 VTX 中 **`0x02`(TRISTRIP) 出现 0 次**，
全是 `0x01`(TRILIST)。**这一项可以从「拦路虎」名单里划掉** ——
这是把 VTX 难度从「极高」降到「中」的关键事实。

### 4.9 法线/切线计算

- 法线：`mdlc` 从 SMD 直接读取，不重算。合理。
- 切线：**占位实现**（`vvd.rs:365-388` 由法线叉积派生）。
  后果：**法线贴图错误**。正确做法需要按 UV 导数在三角形上累加再正交化。

### 4.10 顶点去重与索引化 —— 缺失

`mdlc` 把 SMD 的每个三角形顶点直接映射成一个 VVD 顶点，**不去重**。
studiomdl 会按（位置/法线/UV/骨骼权重）哈希去重并重排。
后果：文件膨胀（SMD 的三角形顶点数通常远大于唯一顶点数）、
**与 studiomdl 产物无法逐字段对齐**（`README.md:138-142` 把顶点顺序差异列为「已知语义等价差异」，
这在本阶段可接受，但去重本身是编译器的核心职责之一）。

### 4.10b 骨骼权重归一化（`SortAndBalanceBones`）—— **缺失，且与实测数据矛盾**

`mdlc` 的 `build_vvd`（`main.rs`）只做「按权重降序排序 + 取前 3 组」。
原版多做了三件事，代码在 `hl2sdk-episode1\utils\studiomdl\mrmsupport.cpp:41-95`：

```cpp
45: 	// collapse duplicate bone weights        ← ① 合并重复骨骼（同一骨骼的权重相加）
46: 	for (i = 0; i < iCount-1; i++)
...
59: 	// do sleazy bubble sort                  ← ② 降序冒泡排序
...
74: 	// throw away all weights less than 1/20th
75: 	while (iCount > 1 && weights[iCount-1] < 0.05)   ← ③ 丢弃权重 < 0.05 的骨骼
76: 	{
77: 		iCount--;
78: 	}
81: 	if (iCount > iMaxCount) { iCount = iMaxCount; }  ← 裁剪到 3
...
86: 	float t = 0;
87: 	for (i = 0; i < iCount; i++) { t += weights[i]; }  ← ④ 重新归一化到和 = 1
```

**① 合并重复骨骼**和 **③ 丢弃 < 0.05** 是 `mdlc` **完全没有**的步骤，
**④ 重新归一化**也没有（`mdlc` 只是校验输入权重和是否为 1，然后原样写出）。

**实测旁证**：官方 `v_autoshotgun.vvd` 的 388,765 个顶点中
`boneCount` 分布为 `1`: 284,724 / `2`: 1,306 / `3`: 1,808，
且**权重和不为 1 的顶点为 0 个** —— 说明归一化确实被执行。
`boneCount == 1` 占 73%，与「丢弃 < 0.05 后只剩一根」的行为一致。

**后果**：`mdlc` 产出的 VVD 里可能出现权重和为 1 但**含微小权重**的顶点，
与 studiomdl 产物不可比；且引擎的硬件蒙皮路径对权重分布有假设。
**难度：低**（算法已完全公开，20 行以内）。

### 4.11 骨骼使用标志（`BONE_USED_BY_*`）计算 —— 缺失

`mdl_writer.rs:69` 定义 `DEFAULT_BONE_FLAGS = 0x500`
（`BONE_USED_BY_VERTEX_LOD0 | BONE_USED_BY_HITBOX`），**对所有骨骼一律写这个值**。

原版的完整语义（`studio.h:309-340`）：

| 标志 | 值 | 计算位置 |
|---|---|---|
| `BONE_USED_BY_HITBOX` | 0x00000100 | `simplify.cpp:6997-7011`，沿父链上溯 |
| `BONE_USED_BY_ATTACHMENT` | 0x00000200 | `simplify.cpp:3472-3487`（`$attachment`）、3491-3515（`$ikchain`/`$mouth`）、3544（eyeball） |
| `BONE_USED_BY_VERTEX_LOD0..7` | 0x00000400 << n | `simplify.cpp:3446-3455`（LOD0）、`UnifyLODs.cpp:1090-1098`（LODn） |
| `BONE_USED_BY_BONE_MERGE` | 0x00040000 | `simplify.cpp:3526` |
| `BONE_ALWAYS_PROCEDURAL` | 0x00000004 | `simplify.cpp:3936/3962/4012` |
| `BONE_FIXED_ALIGNMENT` | 0x00100000 | `simplify.cpp:2817` |
| `BONE_HAS_SAVEFRAME_POS/ROT` | 0x00200000/0x00400000 | — |

**关键的传播规则**：`UpdateBonerefRecursive()`（`simplify.cpp:3404-3418`）
会把每个骨骼的标志**沿父链上溯传播**（这就是 `studio.h:318` 注释里
「bone (or child) is used by a hit box」的含义），
且注释明确要求 **「必须在所有标志设置完之后最后执行」**。

**缺失后果**：引擎的骨骼掩码优化会误判。写死的 `0x500` 对「被顶点使用」是**偶然正确**的
（因为 LOD0 顶点确实用了它们），但对 `BONE_USED_BY_ATTACHMENT`、
`BONE_USED_BY_BONE_MERGE`、以及 LOD1+ 的顶点使用**全部错误**。
一旦实现 attachment/bonemerge/LOD，这个写死值就会开始出错。

---

## 第 5 层：运行时相关（动画）

**`mdlc` 在这整层是零。**

| 缺失项 | 说明 | 难度 | 参考 |
|---|---|---|---|
| `mstudioanimdesc_t` | 100 字节/项，`studio.h:626` | 高 | `studio.h` |
| `mstudioseqdesc_t` | 212 字节/项，`studio.h:695` | 高 | `studio.h` |
| **动画位流编码** | `mstudioanim_t`（4 字节头 + 变长）+ `mstudioanimvalue_t`（RLE，2 字节） | **极高** | `mdl-layout.md:355-390`（结构）；**编码算法无公开规格** |
| 每骨骼每轴通道偏移 | `mstudioanim_valueptr_t`，`short offset[3]` | 高 | `studio.h:556` |
| 量化（scale/offset） | `STUDIO_ANIM_*`；无公开规格 | **极高** | 无 |
| 骨骼选择启发式 | studiomdl 决定哪些骨骼进动画 | **极高** | 无 |
| **activity 映射** | `mstudioseqdesc_t.activity`/`actweight`；`activitylistversion` 是**运行时缓存，写 0 正确** | 中 | `studio.h:695`；实测 800/800 `activitylistversion = 0` |
| **events** | `mstudioevent_t`，80 字节：`{float cycle; int event; int type; char options[64]; int szeventindex}`。**`options` 是内联 64 字节数组，不是偏移** | 中 | `mdl-layout.md:467` |
| v49 indexed events | `eventsindexed`；`type & 0x400` = `NEW_EVENT_STYLE` | 中 | `mdl-layout.md:467` |
| **autolayer** | `mstudioautolayer_t`，24 字节 | 高 | `studio.h:680` |
| **transition** | `mstudiolocalhierarchy_t`，48 字节 + localnode 表 | 高 | `studio.h:524` |
| 混合（blend/blendwidth） | `groupsize[2]`/`paramindex[2]`/`paramstart[2]`/`paramend[2]` + `posekeyindex` 数组 | 高 | `studio.h:695` |
| `$sequence` 各参数 | 约 30 个参数（`activity`/`blend`/`addlayer`/`event`/`ikrule`/`fps`/`loop`/`delta`/…） | 中 | `qc-generation.md` §2.36 给出**输出形状**（Crowbar 方向） |

**一个诚实的重要提醒**：`animation-command.md`（66 KB，本被列为权威参考）
**完全不含**位流/RLE/量化/`.ani` 格式/`mstudioactivity_t`/`mstudioevent_t` 的任何内容
（已逐项 grep 确认零匹配）。**动画位流编码目前没有找到任何权威参考** ——
这是本项目真正的「无参考区」，只能靠对真实产物做大量统计反推。

---

## 第 6 层：面部系统

**`mdlc` 在这整层是零。**

| 缺失项 | 结构 | 大小 | 难度 | 参考 |
|---|---|---|---|---|
| flexdesc | `mstudioflexdesc_t` | 4 | 高 | `studio.h:810` |
| flexcontroller | `mstudioflexcontroller_t` | 20 | 高 | `studio.h:819` |
| flexcontrollerui | `mstudioflexcontrollerui_t` | 20 | 中 | `studio.h:842` |
| flexrule | `mstudioflexrule_t` | 12 | 高 | `studio.h:1074` |
| flexop | `mstudioflexop_t` | 8 | 高 | `studio.h:1063` |
| flex 帧 | `mstudioflex_t` | 60 | 高 | `studio.h:1037` |
| flex 顶点增量 | `mstudiovertanim_t` | 16（v49 有 18 字节的 wrinkle 变体） | 高 | `studio.h:926/1007` |
| mesh→flex 链 | `mstudiomesh_t.numflexes`/`flexindex` | — | 中 | `studio.h:1238` |
| **VTA 文件** | 顶点动画，`$flexfile` 指定 | — | 高 | **无任何文档覆盖其二进制格式** |
| **eyeball** | `mstudioeyeball_t` | 172 | 中 | `studio.h:1128`；`eyeball-eyelid-mouth.md` 给出完整字段表与读取顺序 |
| **eyelid** | 由上表的 `upperlidflexdesc`/`lowerlidflexdesc` + flex 帧驱动 | — | 高 | 同上；**但该文档只讲反编译方向** |
| **mouth** | `mstudiomouth_t` | 20 | 中 | `studio.h:1537` |

**关于 flex opcode 的一个纠正**：任务描述提到
「`mstudioflexops` 枚举：IMAD, COMP, NEG, EXTRAPOLATE, INTERPOLATE」。
**这些名字在任何本地文档中都不存在。**
`mdl-layout.md:531` 给出的实际枚举是：
`CONST 1, FETCH1 2, FETCH2 3, ADD 4, SUB 5, MUL 6, DIV 7, NEG 8, EXP 9, OPEN 10,`
`CLOSE 11, COMMA 12, MAX 13, MIN 14, 2WAY_0 15, 2WAY_1 16, NWAY 17, COMBO 18,`
`DOMINATE 19, DME_LOWER_EYELID 20, DME_UPPER_EYELID 21`。
**我未在 `hl2sdk-l4d2\public\studio.h` 中找到 `mstudioflexops` 枚举定义**
（该头文件里 `mstudioflexop_t` 只有 `int op`，没有枚举常量）。
**建议以真实文件实测为准，不要采用任务描述里的名字。**

**关于 `.vta` 是否必需**：`eyeball-eyelid-mouth.md` **从未说明** `.vta` 是否必须随模型分发。
这是未验证项，不做推测。

---

## 第 7 层：物理

**`mdlc` 在这整层是零，且这一层的 QC 命令数最多（约 30 条）。**

| 缺失项 | 说明 | 难度 |
|---|---|---|
| `$collisionmodel` / `$collisionjoints` | 入口命令 | 中 |
| **凸包分解** | `$concave` / `$maxconvexpieces` / `$concaveperjoint` | **极高**（Valve 未公开；原版链接 `vphysics.dll`） |
| `$mass` / `$automass` / `$masscenter` | 质量与质心 | 低 |
| `$inertia` / `$damping` / `$rotdamping` | 惯性/阻尼 | 低 |
| `$jointconstrain` | 关节约束（每关节 x/y/z 的 limit + friction） | 中 |
| `$jointmassbias` / `$jointdamping` / `$jointinertia` / `$jointrotdamping` | 逐关节参数 | 低 |
| `$noselfcollisions` / `$jointcollide` | 自碰撞 | 中 |
| `$rootbone` / `$jointskip` / `$jointmerge` | 根骨骼与关节合并 | 低 |
| `$remove2d` / `$weldnormal` / `$weldposition` / `$polysoup` / `$drag` / `$rollingDrag` | 网格预处理 | 中 |
| `$physmaxvelocity` / `$physmaxpenetration` / `$physreduce` / `$forcecapsules` / `$physskin` / `$phyname` / `$snapcollisionjoints` / `$collisiongroup` / `$collisiontext` | 调优参数 | 低 |
| **ragdoll** | 由上述全部共同决定 | 高 |

**实测必要性**：`.phy` **从来不是加载必需**（`mdl-layout.md:771`），
实测 800 个模型 **420 有 / 380 无**。所以物理缺失**不会阻止模型显示**，
但会让模型**无法作为物理实体**。

**难度重估**：命令数量最多（约 30 条），但**绝大多数是低难度的参数透传**。
真正难的是**凸包分解**一项。若允许委托外部工具，整层可大幅降级。

---

## 第 8 层：其他

| 缺失项 | 说明 | 难度 | 参考 |
|---|---|---|---|
| **jigglebone** | `mstudiojigglebone_t`，120 字节基础 + `IS_BOING` 时额外 5 个 float = **140 字节** | 中 | `studio.h:162`；`mdl-layout.md:272-277` |
| **IK chain** | `mstudioikchain_t`(16) + `mstudioiklink_t`(28) | 中 | `studio.h:1178/1165` |
| **IK rule** | `mstudioikrule_t`(152) + `mstudioikerror_t`(28) + `mstudiocompressedikerror_t`(36) | 高 | `studio.h:460/432/447` |
| **IK lock** | `mstudioiklock_t`(32) | 低 | `studio.h:512` |
| **attachment** | `mstudioattachment_t`(92)；实测 140/800 非零 | 低 | `studio.h:414` |
| **hitbox set** | `mstudiohitboxset_t`(12) + `mstudiobbox_t`(68)；实测 **800/800** | 低 | `studio.h:1550/356` |
| **bonemerge** | `BONE_USED_BY_BONE_MERGE` + 合并逻辑 | 中 | `studio.h:329` |
| **bodygroup preset** | `mstudiobodygrouppreset_t`(12)，v49 新增 | 低 | `mdl-layout.md:627-629`；**实测 L4D2 无此数据** |
| **`studiohdr2`** | 见 3.2 | 低 | `studio.h:1941` |
| **include model** | `mstudiomodelgroup_t`(8)；实测 9/800 | 中 | `studio.h:382` |
| **纹理/材质处理** | `mdlc` 只写材质名与 flags，**不接触 `.vmt`/`.vtf`**。原版也不编译纹理（那是 `vtex.exe` 的职责），但会做材质名解析与 `$cdmaterials` 规范化 —— 这部分 `mdlc` **已实现且正确**（`mdl_writer.rs:305-342`） | — | 已实现 |
| **`$keyvalues`** | 头部 `keyvalueindex`/`keyvaluesize`；实测 77/800 | 低 | `studio.h:312-316` |
| **`$includemodel`** | 模型组合 | 中 | — |
| **LOD / `$shadowlod`** | 见 4.7 | 高 | — |
| **`$bonemerge`** | 骨骼合并（L4D2 的 survivor 模型大量使用 —— 实测 `example_1.qc` 有 **55 次** `$bonemerge`） | 中 | — |
| **`$poseparameter`** | `mstudioposeparamdesc_t`(20)；实测 37/800 | 低 | `studio.h:799` |

---

# 第三部分：优先级建议

## 3.1 真正的「拦路虎」（没有它们就无法编译任何真实模型）

**判定口径**：我用「L4D2 真实 QC 工程的命令使用频率」作为依据。
实测三个真实 QC（`D:\GITHUB\plank\examples\example_{1,2,11}.qc`）的命令分布：

| 命令 | example_1 | example_2 | example_11 | 合计 |
|---|---|---|---|---|
| `$definebone` | 63 | 91 | 63 | **217** |
| `$bonemerge` | 55 | 57 | 55 | **167** |
| `$sequence` | 27 | 23 | 27 | **77** |
| `$animation` | 10 | 12 | 10 | **32** |
| `$attachment` | 8 | 6 | 8 | **22** |
| `$cdmaterials` | 3 | 1 | 3 | 7 |
| `$poseparameter` | 2 | 2 | 2 | 6 |
| `$ikchain` | 2 | 0 | 2 | 4 |
| `$jigglebone` | 0 | 2 | 0 | 2 |
| `$modelname`/`$bodygroup`/`$bbox`/`$cbox`/`$contents`/`$surfaceprop`/`$illumposition` | 各 1 | 各 1 | 各 1 | 各 3 |
| `$opaque` | 0 | 1 | 0 | 1 |

**六个拦路虎 —— 现已全部解决（2026-09 更新）**：

| # | 拦路虎 | 一句话理由 | 状态 |
|---|---|---|---|
| **1** | **VTX 写出** | 没有它模型**完全不渲染**（引擎报 `ErrorRequiredVtxFileNotFound`）。实测 L4D2 全是 tri-list（0 个 tri-strip），难度比预期低得多 | ✅ **已完成**（25/27 字节布局；与官方逐字段 0 差异） |
| **2** | **`mstudiobone_t.quat` 必须写 `AngleQuaternion(rot)`** | 运行时 `InitPose` 直接读 `quat`。写 0 是非法四元数，**骨骼姿态全坏** | ✅ **已完成**（与官方逐位相同） |
| **3** | **`$sequence` + 动画位流** | 没有序列 = **纯静态模型**。真实 QC 里 `$sequence` 出现 **77 次**。这一项同时是**难度最高**的（位流编码无任何权威参考） | ✅ **已完成**（89 骨骼 × 30 帧：scale 267 轴逐位相同，5370 采样仅 18 个差 1 LSB） |
| **4** | **hitbox set** | 实测 **800/800** 的真实模型都有。没有它**命中判定失效** | ✅ **已完成**（全部字段一致） |
| **5** | **`$bonemerge`** | 真实 QC 里出现 **167 次**，仅次于 `$definebone`。L4D2 的 survivor 模型**全部**依赖它 | ✅ **已完成**（骨骼 flags 与官方逐位相同） |
| **6** | **`$attachment`** | 真实 QC 里 22 次。L4D2 的**全部武器模型**依赖挂点 | ✅ **已完成**（含 `local` 矩阵全部字段） |

> **动画层的三条旧结论已被推翻。** 本文早先依据 `docs/animation-layout.md`
> 引用「相对第 0 帧的增量」「除数恒为 32767」「LOOPING 末帧归零」——
> 那些结论的受控实验里 SMD 第 0 帧旋转恰好都是 0，无法区分「绝对」与
> 「增量」。正确模型与实测证据见 `README.md` 的「动画实现的关键规则」
> 与 `src/anim_writer.rs` 的模块文档。
>
> **仍然未实现的动画子项**：RLE run 合并（当前只做常量折叠，产物大
> 4–6×）、`sectionframes` 分段、IK rule、movement、
> `seqdesc.bbmin/bbmax` 用逐帧蒙皮并集（当前用顶点 AABB）。

**关于「拦路虎」的一条重要澄清**：**PHY 不是拦路虎**。
`.phy` 从来不是加载必需（`mdl-layout.md:771`），实测 420/800 有、380/800 无。
缺 PHY 的模型**能正常显示与动画**，只是没有物理。

## 3.2 分阶段补齐顺序

排序依据：**用户可感知价值 ÷ 实现难度 × 有公开参考的程度**。

### Phase 1 —— 「让产物真的能用」（全部低难度，收益最大）

| 项 | 难度 | 依据 |
|---|---|---|
| 1. 修 `quat = AngleQuaternion(rot)` | 低 | 约 10 行；实测公式已验证（偏差 8.2e-08） |
| 2. **VTX 写出**（25/27 布局、tri-list、扁平 index） | 中 | 布局已知；实测无 tri-strip、单 strip group 占绝大多数 |
| 3. hitbox set 写出 | 低 | `studio.h:1550/356` |
| 4. `$illumposition` / `$eyeposition` 轴交换 | 低 | `studiomdl.cpp:867-882` 有确切代码 |
| 5. skin 表（uint16） | 低 | **实测元素是 uint16 不是 int32**；`v_autoshotgun` 是恒等映射 0..20 |
| 6. `$attachment` | 低 | `studio.h:414` |
| 7. `studiohdr2`（固定放 408，写 `sznameindex`/`linearboneindex`） | 低 | 实测布局已 dump |
| 8. `$keyvalues` / `$poseparameter` | 低 | 结构简单 |
| 9. **骨骼权重归一化**（`SortAndBalanceBones` 的 4 个步骤） | 低 | `mrmsupport.cpp:41-95` 已逐行确认；20 行以内 |

**Phase 1 完成后的产物**：可以在游戏里**正确渲染**的静态模型。

### Phase 2 —— 「让它像真的模型」

| 项 | 难度 | 依据 |
|---|---|---|
| 9. **`$sequence` + 动画位流** | 极高 | **无权威参考**，需大量实测反推。**建议先做只读的位流解析器**，用真实模型验证理解后再写编码器 |
| 10. events | 中 | `mdl-layout.md:467` 有 80 字节布局 |
| 11. `$bonemerge` | 中 | 真实 QC 频率最高之一 |
| 12. `BONE_USED_BY_*` 完整计算（含 `UpdateBonerefRecursive` 父链传播） | 中 | `simplify.cpp:3404-3418` 有确切代码 |
| 13. 顶点去重与索引化 | 中 | 无规格，需实测反推哈希顺序 |
| 14. 真实切线计算 | 中 | 标准算法 |
| 15. VVD fixup 表 + 多 LOD | 高 | 占 2.9%，但全是多 LOD 模型 |

### Phase 3 —— 「功能完整」

| 项 | 难度 |
|---|---|
| 16. QC 解析器（→ 现有 IR） | 中 |
| 17. flex / eyeball / mouth / eyelid / VTA | 高 |
| 18. IK chain/rule/lock | 中–高 |
| 19. `$lod` / `$shadowlod` | 高 |
| 20. PHY（**建议委托外部工具**） | 极高 |
| 21. VRD / jigglebone | 中 |
| 22. DMX（**建议委托 `dmxconvert.exe`**） | 高→低（若委托） |
| 23. `$include`/`$pushd`/`$popd`/`$definevariable` | 低–中 |

### 3.3 两条战略性建议

**建议 A：把「委托子进程」作为一等策略。**
原版 studiomdl **自己就这么做** —— 实测它把 DMX 转换委托给 `bin\dmxconvert.exe`
（导入表与字符串中确认）。同理，PHY 可以委托 `vphysics`。
**不必为了「纯 Rust」而重复实现凸包分解这类极高难度、且与核心价值无关的组件。**

**建议 B：Phase 2 的第 9 项（动画位流）应该先做一个只读解析器。**
理由：本项目已被验证的最有效方法是「oracle 差分」
（`README.md:47-72` 记录的 12 个静默缺陷全部由差分揪出）。
动画位流是**唯一完全没有参考**的部分，
在没有解析器的情况下直接写编码器，会重蹈「一次面对两个未验证子系统」的覆辙 ——
这正是 `README.md:19-27` 当初选择 TOML 而非 QC 的理由。

---

# 第四部分：诚实性声明

## 4.1 我**直接读代码/文档/实测确认**的结论

以下每一条都有可追溯的证据，**可以直接采信**：

| 结论 | 证据 |
|---|---|
| `mdlc` 只写 MDL 头部的 0/1 段 + 骨骼表 + 材质表 + bodypart 树 + 字符串池，其余 47 个头部字段显式写 0 | 逐行读 `mdl_writer.rs:486-593`，零字段列表在 `mdl_writer.rs:524-577` |
| `mdlc` 完全不产出 `.vtx`/`.phy`/`.ani`/`.vta` | `main.rs:189-209` 只写两个文件；全源码无 `vtx`/`phy`/`ani` 字样 |
| `mdlc` 的 `quat` 写 0 | `mdl_writer.rs:613` 及其注释 |
| `quat` 应该是 `AngleQuaternion(rot)` | `write.cpp:203`；**且我用官方 `v_autoshotgun.mdl` 89 根骨骼验证，最大偏差 8.21e-08** |
| `mdlc` 的 `poseToBone` 实现（当前版本）是正确的 | `bone_math.rs:57-76, 113`；**89 根骨骼逐根比对，0 根不符，最大偏差 1.18e-05** |
| `$illumposition` 有 90° Z 轴交换 `(a,b,c) → (−b,a,c)` | `studiomdl.cpp:867-882` 的确切代码 |
| 真实 MDL 的 `studiohdr2` 非空，`linearboneindex`/`sznameindex` 有值 | 我逐字段 dump 官方 `v_autoshotgun.mdl`（`studiohdr2index = 408`） |
| `mstudiotexture_t.skinref` 元素是 **uint16** 不是 int32 | 我实测：按 uint16 读得 `0..20` 恒等映射，表末 602594 与下一段起点吻合 |
| L4D2 VTX 头部字段值（`maxBonesPerStrip=53` 等，uint16） | 我独立解析 `v_autoshotgun.dx90.vtx` |
| `skinref` 是 uint16、`mstudioattachment_t` 是 92 字节等结构大小 | `studio.h` 行号 + 实测 |
| `poseToBone` 在 SDK 里只有 1 个消费者（布娃娃/strider IK），**不是**正常渲染路径 | `bone_setup.cpp:1731-1738`；`InitPose` 读 `pos`/`quat`（`bone_setup.cpp:1757-1758`） |
| `BONE_USED_BY_*` 的计算**不在** `bone_setup.cpp` 里 | 穷举 grep 确认；计算在 `simplify.cpp`/`UnifyLODs.cpp`（episode1 SDK） |
| `hl2sdk-l4d2` 的 `STUDIO_VERSION` 是 **48**，且**没有** `#if STUDIO_VERSION >= N` 版本分支 | `studio.h:71`；grep 确认只有 5 处出现、只有 2 处有行为 |
| 三个真实 QC 的命令使用频率 | 我直接对 `example_{1,2,11}.qc` 做正则统计 |
| 官方 `v_autoshotgun.mdl` 的头部全部字段值 | 我逐字段 dump（`numbones=89`, `numlocalanim=37`, `numlocalseq=37`, `numtextures=21`, `numbodyparts=9`, `numattachments=8`, `numikchains=12`, `mass=1.0`, `contents=1` 等） |
| exe 中 `$[a-z]` 命令候选 157 个，聚集区 73 个 | 我独立用 PowerShell 提取 + 偏移聚集分析 |
| **QC 命令共 137 条，其中 104 条在二进制调度表内** | **我独立复核通过**：解析 `.data` 文件偏移 `0x6E6988` 起的 104×12 字节表，确认 104 个唯一名字全部以 `$` 开头、103 个唯一 handler 全在 `.text`、第三字段全为 0、`$hierarchy`/`$heirarchy` 共享 `0x0044C1A0`、只有 `$skinnedLODs` 含大写、104 个名字全部落在 137 清单内 |
| 104 表之外的 33 条与 137 总数互补 | 我复核差集**恰好**是 27 个碰撞关键字 + 6 个独立关键字，零遗漏零多余 |
| 28 个非 NUL 分隔碎片是噪声 | 子调研逐一定位并检查周围字节（19 个是 `.text` 的 ModRM/SIB 字节伪造、6 个 `.rdata` 二进制、2 个 `.reloc`）；我复核了 `0x24` 伪造机制与我自己方法 A 里 24 个「噪声」的对应关系 |
| QC 命令在真实工程里的分组使用 | `example_*.qc` 统计 |
| `SortAndBalanceBones` 的四个步骤（合并重复骨骼 / 降序排序 / 丢弃权重 < 0.05 / 裁剪到 3 / 重新归一化） | 我逐行读 `mrmsupport.cpp:41-95`，确认第 75 行的 `weights[iCount-1] < 0.05` 阈值与第 81 行的裁剪 |
| 官方 VVD 的 `boneCount` 分布与权重和 | 子调研实测 388,765 顶点：`1`:284,724 / `2`:1,306 / `3`:1,808，权重和不为 1 的 0 个 |

## 4.2 我**推断**的结论（有依据但非直接证实）

| 结论 | 推断依据 | 不确定性 |
|---|---|---|
| 「引擎加载必需 vs 可选」的分类 | 从 `mdl-layout.md` 的加载错误码 + 800 样本频率统计推出 | **文档没有「必需性」章节**，这是推断 |
| **QC 命令的功能分组（38/10/24/10/3/42/2/3/5/0）** | 子调研的手工归类 + 机器校验（脚本对未分配/重复分配/不存在的 token 抛错） | **二进制里没有分组元数据**，这是判断。边界项已逐条记录理由（如 `$rootbone` → 碰撞组、`$poseparameter` → 骨骼组） |
| 各命令的「难度评级」 | 我的主观判断，依据是结构是否已知、算法是否黑盒 | **主观**。同一项不同人可能给不同级别 |
| 137 条命令的「一句话作用」 | 领域知识（Valve QC 语义）+ `qc-generation.md` | **子调研明确声明未反编译那 104 个 handler**，故语义描述不是从该二进制提取的 |
| 「L4D2 不需要 strip 生成」 | 子调研对 786 个 VTX 的统计（0 个 `0x02`）；我复核了头部字段但**未亲自逐字节复算 strip flags 分布** | 样本是 `D:\SOURCE\SOURCEMDLS`，**不完全是 L4D2 官方素材** |
| 「VTX 25/27 而非 33/35」 | 子调研的 786 样本统计 + `optimize.h` 字段对比 | **我未亲自逐字节复算**。这条与既有代码注释直接冲突，建议独立复核 |
| 「约 10% 的模型是多 LOD」「2.9% 有 fixup」「420/800 有 .phy」等频率 | 子调研对 800/792/786 样本的统计 | 样本来源是 `D:\SOURCE\SOURCEMDLS`，**未验证其与 L4D2 官方素材的分布一致性** |
| 「`$sequence` 缺失 → 纯静态模型」 | 领域常识 + 头部字段语义 | 高置信但非实测 |

## 4.3 我**无法验证**的部分（明确列出，不做推测）

1. **L4D2 版 `studiomdl.exe` 自身的编译逻辑**。
   PE32 x86、`.text` 5,695,099 字节、**无 PDB**（残留路径
   `c:\buildslave\l4d2_rel_win32\...\studiomdl.pdb`）。
   本报告引用的编译器侧代码来自 **`hl2sdk-episode1`（v44 时代）**，
   是**强佐证但不是 L4D2 规格**。L4D2 SDK 的 `utils\` 下**没有** studiomdl 源码。
2. **约 33 条 QC 命令的语义**：那 27 个 `$collisionjoints` 块内关键字
   （`$mass` `$inertia` `$damping` `$rotdamping` `$concave` `$maxconvexpieces`
   `$jointconstrain` `$jointcollide` `$jointmassbias` `$noselfcollisions` …）
   与 6 个独立关键字（`$include` `$definevariable` `$definemacro` `$decal`
   `$vertexcolor` `$ignorez`），以及 `$pushd` `$popd` `$scale` `$origin`
   `$renamebone` `$collapsebones` `$insertbone` `$realignbones` `$unlockdefinebones`
   `$loddistance` `$lodvertexcount` `$jointlod` `$ikground` `$noseparate`
   `$definecollision` `$external` `$animblockname` `$sequencegroup` `$pose`
   `$vbox` `$hboxbone` `$cdtexture` `$material` `$flexscale` `$stereosplit` 等。
   **它们的存在性已由调度表确认**（137 条清单是权威的），**但语义无权威来源** ——
   `qc-generation.md` 对它们**零覆盖**（因为 Crowbar 从不输出它们）。
   **子调研也明确声明没有反编译那 104 个 handler。**
3. **动画位流编码算法**：`STUDIO_ANIM_*` 量化常量、RLE 阈值、
   「哪些骨骼进动画」的选择启发式。**任何本地文档都没有覆盖**。
4. **VTA 二进制格式**：已确认 `.tmp/research/` 下**没有任何文档**描述它。
5. **凸包分解算法**：Valve 未公开；原版链接 `vphysics.dll`。
6. **`.vta` 是否必须随模型分发**：`eyeball-eyelid-mouth.md` **从未说明**。
7. **`hardwareID → newBoneID` 的查找规则**：`vvd-vtx-layout.md:508, 684` 明确标注为未确定。
8. **多 UV 集 `ExtraVertexAttributeIndex_t.m_offset` 的基准**：
   `vvd-vtx-layout.md:115, 682` 明确标注为未确定（实测 L4D2 无此数据，可回避）。
9. **`mstudioseqdesc_t.posekeyindex` 数组长度**：
   `mdl-layout.md:470`（加法）与 `:873`（乘积）**自相矛盾**，本次**未核实**。
10. **`mstudioflexops` 枚举的准确成员名**：任务描述给的
    `IMAD/COMP/NEG/EXTRAPOLATE/INTERPOLATE` 在任何本地文档中**都不存在**；
    `hl2sdk-l4d2\public\studio.h` 中**没有该枚举的定义**（只有 `mstudioflexop_t.op` 这个 int）。
    `mdl-layout.md:531` 给了另一套名字（`CONST/FETCH/ADD/.../DME_UPPER_EYELID`）。
    **两者我都没有独立验证**。
11. **`v_autoshotgun.mdl` 的 `attachments` 表**：我尝试 dump 时读出的 `flags` 值不合理
    （594794 等），说明 `mstudioattachment_t` 的 stride 92 或起始偏移 22168 有误，
    或该模型此项为垃圾值。**这一项我没有查清**，故 3.2 节对 attachment 的判断
    依据的是 `studio.h:414` 的结构定义与 140/800 的频率统计，**不是**该模型的实测。
12. **`hl2sdk-l4d2` 的 `STUDIO_VERSION 48` 与真实 L4D2 模型是 v49 的矛盾**：
    子调研声称真实 L4D2 模型是 v49（并给出 `0x00800000` 标志等实测证据），
    并从 `hl2sdk-doi` 取 v49 结构定义。
    **我未亲自验证 VPK 中的模型版本号**。若属实，则
    **用 `hl2sdk-l4d2\public\studio.h` 作为 v49 的规格是危险的** ——
    但 `mdlc` 的 `vvd.rs:59` 与 `model.rs:294` 都默认写 49。
    这是一个**需要优先澄清的矛盾**（详见 0.4 节）。
13. **`mstudioattachment_t` 的 stride 92 与 `v_autoshotgun.mdl` 的实测不符**：
    我尝试从 `attachmentindex = 22168` 按 stride 92 dump 时，读出的 `flags` 值不合理
    （594794 等），说明该模型的此项要么是垃圾值、要么起始偏移/stride 有误。
    **这一项我没有查清**。3.2 节对 attachment 的判断依据的是
    `studio.h:414` 的结构定义与 140/800 的频率统计，**不是**该模型的实测。
14. **`mstudiobone_t` 在 v48 与 v49 的字节布局是否真的完全一致**：
    子调研称「byte-identical」，我实测的 89 骨骼数据来自 v49 素材且与
    `hl2sdk-l4d2`（v48）的字段偏移完全吻合 —— 这与「一致」的说法相符，
    但**我没有做 v48 与 v49 素材的并排对比**。
15. **`hardwareID → newBoneID` 的实际查找算法**：子调研报告
    `vvd-vtx-layout.md:508, 684` 明确标注为未确定（Crowbar 从未实现该查找）。
    实测只能确认「`boneStateChangeOffset` 的基准是 strip 自身起始」，
    但**「如何应用这张表」仍未确定**。好消息是：L4D2 由 Valve studiomdl 产出时，
    VVD 的 `mstudioboneweight_t` **已经是最终全局骨骼索引**，可直接使用、无需查表。

## 4.4 关于「数字准确性」的说明

本报告中的所有数字分为三类，已分别标注：

- **精确值**（我亲自 dump/复算）：如「89 根骨骼」「最大偏差 1.18e-05」
  「`maxBonesPerStrip = 53`」「`skinref` 按 uint16 读得 `0..20`」。
- **带口径的统计值**：如「157 个候选（字符串扫描上界）/ 104 条（调度表，权威）/
  137 条（三档合计）/ 73 个在偏移聚集区」—— **口径已写明**，读者可自行复算。
- **子调研的样本统计**：如「800/800 有 hitboxset」「420/800 有 .phy」
  「786 个 VTX 中 0 个 tri-strip」—— 这些来自对
  `D:\SOURCE\SOURCEMDLS` 的批量统计，**我未独立复算**，
  且**该样本集与 L4D2 官方素材的分布一致性未验证**。采信时请保留这一层不确定性。

**凡我没有查证的，本报告都直接写「未验证」，没有用「可能」「大概」来掩盖。**

---

# 附录 A：QC 命令全集（137 条，已由二进制调度表确认）

## A.1 提取方法与验证链

**方法 B（权威）**：定位 studiomdl 的**命令分派表**。
- 表位置：`.data` 文件偏移 **`0x6E6988`**（真实 VA `0x6E7D88`），
  104 条 × 12 字节 = 1248 字节，连续无空隙。
- 记录布局：`{DWORD 名字VA; DWORD 处理函数VA; DWORD 0}`。
- 分派器在 `.text` 内，其循环边界是字面量 `cmp esi,0x4E0`（= 1248 = 104 × 12），
  **独立佐证了 104 这个条数**。
- 名字比较函数先按原始字节比，失配时才做**大小写折叠的第二次尝试** ——
  这就是为什么混合大小写的 `$skinnedLODs` 也能被识别。

**我的独立复核（全部通过）**：104 条、104 个唯一名字、103 个唯一 handler、
handler 全在 `.text`、第三字段全为 0、`$hierarchy`/`$heirarchy` 共享 `0x0044C1A0`、
只有 `$skinnedLODs` 含大写、104 个名字全部落在 137 清单内。

> **一处更正**：子调研报告里的 VA 列（`0x00AE7D88`、`0x0097A7E4` 等）
> **一律比真实 VA 大 `0x400000`**。正确映射是 `.rdata`/`.data` 均为
> `file = VA − 0x1400`，故表基址真实 VA 为 **`0x6E7D88`**。
> **其文件偏移是对的**，此笔误不影响任何结论。

## A.2 三档构成（无重叠）

| 档 | 内容 | 条数 |
|---|---|---|
| (a) | 调度表内的顶层命令 | 104 |
| (b) | `$collisionjoints` 块内关键字（`.rdata` 连续块，仅块内合法） | 27 |
| (c) | 独立关键字（有各自比较点，不在调度表） | 6 |
| **合计** | | **137** |

**(c) 的 6 条**：`$include` `$definevariable` `$definemacro` `$decal` `$vertexcolor` `$ignorez`

**(b)+(c) 共 33 条**（即 137 − 104），我已复核该差集与 104 表**恰好互补**，零遗漏零多余：
```
$animatedfriction $assumeworldspace $automass $concave $concaveperjoint $damping
$decal $definemacro $definevariable $drag $ignorez $include $inertia $jointcollide
$jointconstrain $jointdamping $jointinertia $jointmassbias $jointmerge $jointrotdamping
$jointskip $mass $masscenter $maxconvexpieces $noselfcollisions $polysoup $remove2d
$rollingDrag $rootbone $rotdamping $vertexcolor $weldnormal $weldposition
```

## A.3 已排除的噪声（28 个非 NUL 分隔碎片）

子调研逐一定位并检查了周围字节，确认**全部不是命令**：

```
$0f0x0 $0r0 $4b $8m $8p $9u $ak $h0 $h0y $h4 $h8 $h81 $h8z $hd $hh $hht $hl
$hln $hp $hpp $ht $hx $hxj $hxk $km $lm $qm $sm
```

来源分解：
- **19 个是 `.text` 里的 x86 代码**：`0x24` 是 ModRM/SIB 位移字节，紧随其后的 opcode 恰好是小写字母。
  例：`83 EC 08 DD 1C 24 68 30 D9 99 00` 中 `24` 属于 `DD 1C 24`（`fstp qword [esp]`），
  下一字节 `68` 是 `push imm32` 的 opcode —— 于是伪造出 `$h0`。
- **6 个是 `.rdata` 任意二进制**（浮点/整数表、压缩块）：`$4b $8m $8p $9u $ak` 等。
- **2 个在 `.reloc`**：`24 30 66 30 78 30` 是重定位块头 + 小端 16 位偏移 → `$0f0x0`、`$0r0`。

**另排除**：printf/错误格式串（`"$BoneSaveFrame \"%s\""`、`"$jigglebone: parse error"` 等）、
AutoCAD DXF 变量（`$ACADVER $UCSORG $UCSXDIR $UCSYDIR $TILEMODE`）、
PE 导入名装饰（`$WriteConsoleW`、`$LoggingSystem_RegisterLoggingListener` 等）、
`$$$DUMMY` 占位符。

> **对我方法 A 的启示**：我的 157 个候选里那 24 个「疑似 C++ 修饰名噪声」
> （`$ak $h0 $h4 $h8 $hh $hl $hp $ht $hx $km $lm $qm $sm` 等）
> **全部是这 28 个碎片**，一个都不是命令。**字符串扫描只能给上界，调度表才能给准确值。**

## A.4 全量清单（137 条，按字母序）

```
$addsearchdir          $allowrootlods        $alwayscollapse        $ambientboost
$animatedfriction      $animation            $animblocksize         $append
$assumeworldspace      $attachment           $autocenter            $automass
$bbox                  $body                 $bodygroup             $bonemerge
$bonesaveframe         $calctransitions      $casttextureshadows    $cbox
$cd                    $cdmaterials          $centerbonesonverts    $clampworldspace
$cliptotextures        $cmdlist              $collapsebones         $collapsebonesaggressive
$collisiongroup        $collisionjoints      $collisionmodel        $collisiontext
$concave               $concaveperjoint      $constantdirectionallight  $contents
$continue              $controller           $damping               $decal
$declareanimation      $declaresequence      $defaultweightlist     $definebone
$definemacro           $definevariable       $donotcastshadows      $drag
$externaltextures      $eyeposition          $fakevta               $forcecapsules
$forcephonemecrossfade $forcerealign         $gamma                 $hbox
$hboxset               $heirarchy            $hgroup                $hierarchy
$ignorez               $ikautoplaylock       $ikchain               $illumposition
$include               $includemodel         $inertia               $insertbone
$jigglebone            $jointcollide         $jointconstrain        $jointcontents
$jointdamping          $jointinertia         $jointmassbias         $jointmerge
$jointrotdamping       $jointskip            $jointsurfaceprop      $keyvalues
$limitrotation         $lockbonelengths      $lod                   $mass
$masscenter            $maxconvexpieces      $maxeyedeflection      $minlod
$model                 $modelname            $mostlyopaque          $motionrollback
$noforcedfade          $noselfcollisions     $obsolete              $opaque
$origin                $phyname              $physmaxpenetration    $physmaxvelocity
$physreduce            $physskin             $polysoup              $popd
$poseparameter         $prepend              $preservetriangleorder $proceduralbones
$pushd                 $realignbones         $remove2d              $renamebone
$renamematerial        $rollingDrag          $root                  $rootbone
$rotdamping            $scale                $screenalign           $sectionframes
$sequence              $shadowlod            $skinnedLODs           $skipboneinbbox
$skiptransition        $snapcollisionjoints  $staticprop            $subd
$surfaceprop           $texturegroup         $unlockdefinebones     $upaxis
$vertexcolor           $weightlist           $weldnormal            $weldposition
$zbrush
```

**三条书写注意**（供 `mdlc` 的 QC 解析器使用）：
1. **只有 `$skinnedLODs` 含大写**。比较函数会做大小写折叠的第二次尝试，
   所以写 `$skinnedlods` 也能解析，但**建议按原样书写**。
2. **`$hierarchy` 与 `$heirarchy` 是两条独立记录、共用一个 handler** ——
   `$heirarchy` 是历史拼写错误的别名。**两者都是合法输入，都应接受。**
3. **没有任何命令字符串在二进制里出现两次**（去重后 104 个名字互不相同）。

> **这些命令的「一句话作用」是领域知识，不是从该二进制提取的。**
> 子调研明确声明没有反编译那 104 个 handler。
> 本报告对语义的引用一律来自 `qc-generation.md`（Crowbar 方向）或标注为未验证。

---

# 附录 B：本报告引用的主要证据来源

| 类别 | 路径 |
|---|---|
| `mdlc` 源码（快照） | `D:\GITHUB\mdlc\src\{model,mdl_writer,vvd,lib,main,smd,compile,bone_math}.rs` |
| oracle 工具 | `D:\GITHUB\mdlc-oracle\src\{mdl,vvd,vtx,diff,compile,reader}.rs` |
| 原版编译器 | `E:\SteamLibrary\steamapps\common\Left 4 Dead 2\bin\studiomdl.exe`（7,800,512 字节）<br>命令调度表：`.data` 文件偏移 `0x6E6988`，104 条 × 12 字节 |
| QC 命令扫描产物 | `D:\DSH\L4D2ReverseEngineering\tmp-qcscan\`（`qc_commands.txt`、`qc_commands_grouped.md`、`master_tokens.txt`、`dispatch_table.tsv` 等；子调研产出，**不在 `mdlc` 树内**） |
| 真实素材（我实测） | `D:\GITHUB\plank\examples\v_autoshotgun.{mdl,vvd,dx90.vtx}` |
| 真实 QC | `D:\GITHUB\plank\examples\example_{1,2,11}.qc` |
| 格式文档 | `D:\GITHUB\plank\.tmp\research\{mdl-layout,vvd-vtx-layout,animation-command,qc-generation,eyeball-eyelid-mouth,vrd-generation}.md` |
| plank 解编器 | `D:\GITHUB\plank\src\mdl\`（`qc_writer.rs` 206 KB、`real_assets_tests.rs` 154 KB 等） |
| SDK 权威头（读方向） | `D:\DSH\L4D2ReverseEngineering\hl2sdk-l4d2\public\studio.h`（105,757 字节，`STUDIO_VERSION 48`） |
| SDK 运行时骨骼 | `D:\DSH\L4D2ReverseEngineering\hl2sdk-l4d2\public\bone_setup.cpp`（5,800 行） |
| SDK 优化模型 | `D:\DSH\L4D2ReverseEngineering\hl2sdk-l4d2\public\optimize.h` |
| 编译器侧源码（**v44 时代，非 L4D2**） | `hl2sdk-episode1\utils\studiomdl\{write,simplify,studiomdl,mrmsupport,UnifyLODs}.cpp` |
| SMD 导出器 | `D:\DSH\L4D2ReverseEngineering\hl2sdk-l4d2\utils\smdlexp\` |

---

*本报告为只读调研产物。除本文件外未创建或修改 `mdlc` 的任何文件。*
