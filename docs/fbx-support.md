# FBX 支持：可行性调研与 UX 方案

> **状态**：调研 + UX 设计（**未实现任何代码**）。
> 本文的每条结论都标注了取证方式：`[实测]` = 真跑官方 `studiomdl.exe` 或真跑 Rust 探针；
> `[读码]` = 读 `hl2sdk-episode1/utils/studiomdl` 源码或 crate 源码。
>
> 取证素材：`D:\DSH\L4D2ReverseEngineering\_fbxresearch\`（Blender 生成脚本 + 三个探针工程）、
> `D:\GITHUB\mdlc\docs\_probe\` 下 **11 个 oracle 探针**：
> `oracle_fbx.js`（6）、`oracle_fbx_ux.js`（12）、`oracle_fbx_stack.js`（9）、
> `oracle_fbx_order.js`（9）、`oracle_fbx_footguns.js`（8）、`oracle_fbx_morph_full.js`（8）、
> `oracle_fbx_flexmix.js`（6）、`oracle_fbx_flexfile_rule.js`（4）、`oracle_fbx_isolate.js`（6）、
> `oracle_fbx_eyelid.js`（5）、`oracle_fbx_samefile.js`（6）、`oracle_fbx_axis.js`（3）。
> **合计 82 个官方用例，全部真跑 `studiomdl.exe`。**

---

## 0. 结论速览

| 问题 | 结论 |
|---|---|
| 官方 `studiomdl` 支持 FBX 吗？ | **支持，且完全可用**。`.fbx` 在 `Load_Source` 试探链里**排第一** `[实测]` |
| 能对 FBX 路径做 oracle 差分吗？ | **能**。**82 个用例**全部产出四件套 `[实测]` |
| `animsmith-fbx` 能用吗？ | **不建议**。它**主动丢弃 blend shape / morph**，而 mdlc 的表情（flex）系统正建立在 morph 上 `[实测]` |
| 那用什么？ | **直接用 `ufbx`**（`animsmith-fbx` 的底层依赖）。morph 与采样率都可控 `[实测]`；许可 `MIT OR Unlicense`，与 mdlc 的 GPL-3.0-only 兼容 |
| 构建成本？ | `ufbx` 是 C 库，但 **`cargo build` 49.8 s 通过，无需额外工具链** `[实测]` |
| **推荐的 UX？** | **FBX 作为一等源直接写进 QC**（`$body` / `$model` / `$sequence` / `$animation` / `$collisionmodel`），默认行为**逐条对齐官方**，但把官方所有**静默失败**改成**显式诊断**，并给每个隐式决策一个**显式覆盖语法**（§4）。⭐ **新语法按「概念」命名（`src*`）、不按「格式」命名** —— 加 glTF/GLB 时新增语法 **0 条**（§4.3） |
| 最大的 UX 障碍？ | ⭐⭐⭐ **官方恒取第一条 NLA 栈、完全忽略 `$sequence` 名字**（§1.8）。多栈 FBX 在官方路径下**无解**，必须由 mdlc 提供显式选择 |
| 表情（flex）怎么走？ | ⭐⭐⭐⭐⭐ **什么都不用写** —— FBX 的 shape key **自动注册**成 flexdesc + controller + rule + 载荷（§1.6b），与官方逐字段一致。⚠️ 但**不能在 FBX 源上写 `flexfile`/`flex`**（官方必崩） |

---

## 1. 官方侧的实测裁决

### 1.1 `Load_Source` 试探链（`[读码]` 官方 exe 串）

`studiomdl.exe` 里的字符串：

```
..could not load file '%s%s'..fbx.xml.obj.vta.phys....sma.smd.mpp.dmx.vrm.Load_S
```

⟹ 试探链是 **`.fbx .xml .obj .vta .phys .sma .smd .mpp .dmx .vrm`**，`.fbx` **排第一**。

其它相关串：`Error! FBX system not initialized` / `Error! Couldn't create FbxScene` /
`Error! Password protected FBX files unsupported` / `FBX SDK/FBX Plugins. version ...2013.2`。

`bin\` 目录里**没有任何 FBX SDK 的 DLL** ⟹ SDK 是静态链进 `studiomdl.exe` 的。

### 1.2 六个用例的裁决矩阵（`[实测]`）

夹具：`_fbxresearch\samples\`（Blender 5.2.2 无头生成，骨架用 `ValveBiped.Bip01_` 前缀）。
探针：`docs\_probe\oracle_fbx.js`（`spawnSync(studiomdl.exe, ['-nop4','-game',GAMEDIR,qc])`）。

| 用例 | QC 形态 | exit | MDL | VVD | VTX | bones |
|---|---|---|---|---|---|---|
| `A_box_body` | `$body body "box.fbx"` + `$sequence idle "box.fbx"` | 0 | 1732 | 1600 | 429 | 1 |
| `B_rig_body` | `$body body "rig.fbx"`（骨架 + 蒙皮） | 0 | 3148 | 1600 | 437 | 4 |
| `C_anim_seq` | body = `rig.fbx`，`$sequence idle "anim.fbx"` | 0 | 3172 | 1600 | 437 | 4 |
| `D_combined` | `rig_anim.fbx`（一体） | 0 | 3148 | 1600 | 437 | 4 |
| `E_noext` | **不写扩展名**（`"rig"`） | **−1** | — | — | — | — |
| `F_mixamo` | `mixamo_style.fbx`（Maya 式 scale 补偿） | 0 | 3148 | 1600 | 437 | 4 |

⭐ **`E_noext` 官方直接崩**：`ERROR: 'EXCEPTION_ACCESS_VIOLATION' (assert: 1)` +
`ERROR: Aborted Processing on 'mymod/E_noext.mdl'`，**无产物**。
⟹ **mdlc R34「资产引用必须写完整扩展名」的决定是对的，而且比官方更安全。**

### 1.3 ⭐ 官方走的是 **DMX 导入器**

所有 FBX 用例的日志里都有一行：

```
DMX Model d:\github\mdlc\docs\_probe\_oracle_fbx\morph.fbx
```

⟹ **L4D2 的 studiomdl 把 `.fbx` 喂给它的 DMX 导入器**（FBX SDK 只是 DMX 的前端），
不是一条独立的 FBX 路径。这对 mdlc 是**好消息**：官方 FBX 路径的可观测行为，
等价于「FBX → DMX 中间表示 → 既有管线」。

### 1.4 骨骼集合：官方**丢节点**

`b_rig_body.mdl` 的骨骼表（`docs\_probe\bone_list.js`）：

```
numbones=4
  0  parent=-1  Skeleton
  1  parent= 0  ValveBiped.Bip01_Pelvis
  2  parent= 1  ValveBiped.Bip01_Spine
  3  parent= 2  ValveBiped.Bip01_Head1
```

而 `ufbx` 读同一个文件得到 **7 个节点**：`<fbx-root>` / `Skeleton` / `body` / 3 根 ValveBiped / `helper_forward`。

⟹ 官方的规则是：**丢掉网格节点（`body`）与无子节点的叶骨骼（`helper_forward`），保留 `Skeleton` 根**。

⚠️ **但这条规则有例外**：`P_two_fbx`（两个 `$body` 各引一个 FBX）里，`box.fbx` 的网格节点 `box`
**成了第 5 根骨骼** —— 它在单 FBX 用例里是被丢掉的。⟹ **过滤规则依赖于该节点在当前模型里
是否承载网格**，不是单纯看类型。

### 1.5 ⭐⭐ 官方把 FBX 动画**固定归一化到 30 fps**

同一段 1 秒动画分别以 24 / 30 / 60 fps 场景导出（`_fbxresearch\gen_fbx_fps.py`）：
`fps24.fbx` 28924 B / `fps30.fbx` 28972 B / `fps60.fbx` 29196 B。

`$body` 用 `rig.fbx`、`$sequence` 用 `fpsN.fbx`，三者日志**完全相同**：

```
animations     204 bytes (1 anims) (31 frames) [0:01]
```

⟹ ⭐ **源 fps 完全不影响输出帧数，官方固定按 30 fps 采样**（1 秒 ⟹ 31 帧，含首尾）。

> ⚠️ **夹具设计教训**：首次实验把 `$body` 与 `$sequence` 都指向同一个 `fpsN.fbx`，
> 三者都只得 `(1 frames)` —— **必须用独立的动画文件**，`$sequence` 引用的文件才提供帧数据。

### 1.6 ⭐⭐⭐ 官方从 FBX shape key 产出 flex

夹具 `morph.fbx` 13820 B：立方体 + 2 个 shape key `wide`（X×1.5）/ `tall`（Z×1.5），value 都设 0。

官方日志逐字：

```
DMX Model ...\morph.fbx
bones          964 bytes (1)
flexes         888 bytes (2 flexes)
Max flex verts 24
Completed "G_morph.qc"
```

产物 `G_morph.mdl` 2752 B 的头部：`numbones=1 numflexdesc=2 numlocalanim=1`，
flexdesc 名字直读（`docs\_probe\oracle_fbx_morph_names.js`）：

```
flexdesc[0] name="wide"
flexdesc[1] name="tall"
```

⟹ **官方把 shape key 名原样当作 flexdesc 名**，逐顶点偏移进了 flex 载荷。

### 1.6b ⭐⭐⭐⭐⭐ FBX shape key **自动注册** flex（无需任何 QC flex 语法）

这是**第二重要的发现**，且**推翻了我最初的假设**。

最初我只读了 `G_morph.mdl` 的 flexdesc 名（`docs\_probe\oracle_fbx_morph_names.js`），
看到 `flexdesc[0]="wide"` / `flexdesc[1]="tall"`，就以为「用户需要在 QC 里写 `flex` 语句」。
**错。** 那个 QC 里**一个 flex 关键字都没有**：

```
$modelname "mymod/G_morph.mdl"
$body body "morph.fbx"
$surfaceprop "default"
$cdmaterials "models/mymod"
$sequence idle "morph.fbx" fps 30
```

**完整结构**（`oracle_fbx_morph_full.js`，8 用例）：

| 用例 | QC 形态 | exit | flexdesc | flexctrl | flexrules | 载荷 |
|---|---|---|---|---|---|---|
| `X1_auto_body` | `$body body "morph.fbx"`（**零 flex 语法**） | 0 | 2 | **2** | **2** | `flexes 888 bytes (2 flexes)` |
| `X2_auto_model` | `$model body "morph.fbx" { }`（空块） | 0 | 2 | 2 | 2 | 同上 |
| `X3_shapekey_as_seq` | `$body body "rig.fbx"` + `$sequence idle "morph.fbx"` | **−1** | — | — | — | **崩** |
| `X4_explicit_flexfile` | `flexfile "morph.fbx"` + `flex "wide" frame 1` + `flex "tall" frame 2` | **−1** | — | — | — | **崩** |
| `X5_no_shapekey` | `rig.fbx`（无 shape key） | 0 | 0 | 0 | 0 | `flexes 0 bytes` |
| `X6_flexfile_only` | `flexfile "morph.fbx"`（**无 flex 语句**） | 0 | 2 | 2 | 2 | 888 B |
| `X7_flex_nofile` | `flex "wide" frame 1`（**无 flexfile**） | **−1** | — | — | — | **崩**（`could not load file '.vta'`） |
| `X8_auto_rig_anim` | `rig_anim.fbx`（有动画无 shape key） | 0 | 0 | 0 | 0 | `flexes 0 bytes` |

⭐ **官方从 FBX 自动产出完整的三件套**（`X1`/`X2`/`X6` 逐字段）：

```
flexdesc      : ["wide", "tall"]              ← shape key 名原样
flexcontroller: type="wide" name="wide" min=0 max=1   ← 每个 shape key 一个，范围 [0,1]
                type="tall" name="tall" min=0 max=1
flexrule      : rule[0] flexdesc=0("wide") numops=1 op[0]=STUDIO_FETCH1 index=0
                rule[1] flexdesc=1("tall") numops=1 op[0]=STUDIO_FETCH1 index=1
mesh flex 记录: flexdesc=0 targets=[0,1,10,11] numverts=24 pair=0 vtype=0
                flexdesc=1 targets=[0,1,10,11] numverts=24 pair=0 vtype=0
```

> ⚠️ **勘误**：本节早期版本写的是 `op[0] = FLEXOP_MUL`，**错了**。
> 直接在官方产物 `k4_morph.mdl` 上读出来的 `flexrule.op` 是 **`2`**，
> 即 `hl2sdk-l4d2\public\studio.h:2795` 的 **`#define STUDIO_FETCH1 2`**
> （`// get Flexcontroller value`），不是 `STUDIO_MUL`（`6`）。
> 验证：`node docs\_probe\fbx_shape_payload.js <k4_morph.mdl>`
> ⟹ `rule[0] {op=2 index=0}` / `rule[1] {op=2 index=1}`。

⭐⭐⭐ **载荷字段全表**（在 `k4_morph.mdl` 上逐条读出，48 条 vertanim 无例外）：

| 字段 | 官方取值 | 说明 |
|---|---|---|
| `flexdesc` | `0` / `1` | 指向自己的 flexdesc |
| `targets` | **恒 `[0, 1, 10, 11]`** | 与 `.vta` 路径默认值同形 |
| `flexpair` | **恒 `0`** | shape key 不做左右配对 |
| `vertanimtype` | **恒 `0`** | 没有 wrinkle（步长 16） |
| `speed` | **恒 `255`** | = 1.0（等价于 `decay = 0.0`） |
| `side` | **恒 `255`** | = 1.0（`.vta` 非 pair 路径写 0，**这里不同**） |
| `ndelta` | **恒 `(0,0,0)`** | 官方 FBX 路径**丢弃** `nrm_off` |
| `delta` | 见下 | 已变换到 Source 空间 |

⭐⭐⭐⭐⭐ **`delta` 的轴向已单顶点定案**（`gen_fbx_shapeaxis.py` → `axisprobe.fbx`
→ `oracle_fbx_shapeaxis.js`）：Blender 里把 `(5,5,5)` 那个顶点移动 **`(+1,+2,+3)`**，
官方产物 `delta = (1.0000, 2.9980, -2.0000)` ⟹ **变换是 `(x,y,z) → (x, z, −y)`**
（绕 X 轴 **−90°**，即 FBX 导入的 Z-up → Y-up 约定）。
`3.0 → 2.9980` 是**半精度向零截断**（`f32_to_half_bits`，`probe_half_rounding.js` 7/7）。

⭐⭐⭐⭐⭐ **vertanim 的排列规则已解出**（`docs\_probe\k4_group_rule.js`）：
**按控制点号分组，组内按 mesh 顶点号降序；控制点组的出现顺序 = 该 mesh 里
各控制点的首次出现顺序**（= 官方 VVD 的顶点顺序）。

`morph.fbx` 的 `wide`：`cp6→20,10,5` → `cp4→18,8,7` → `cp7→23,14,9` → `cp5→17,12,11`
→ `cp2→22,6,1` → `cp0→16,4,3` → `cp3→21,13,2` → `cp1→19,15,0`，
与官方产物逐项吻合。`tall` 同序。

⚠️ **但 mdlc 有意不复刻这个顺序**（§4.6 偏离 9）：它取决于官方的顶点焊接顺序，
而那个顺序与 mdlc **本就不同**（同一批顶点、不同编号）。载荷的**内容**
（顶点集合 + delta 多重集）完全一致，已由 `k4_flex_align.js` 按 VVD 位置
对齐验证：`位置对齐相同 16，不同 0，仅 A 0，仅 B 0`。

⭐⭐⭐ **一个控制点展开到它的全部焊接顶点**：`axisprobe.fbx` 的 `(5,5,5)`
在官方 VVD 里有 3 个焊接顶点（v9/v14/v23），官方就写了 **3 条** vertanim，
内容完全相同。

⟹ ⭐ **每个 shape key 自动产生：1 个 flexdesc + 1 个 flexcontroller（`[0,1]`）+ 1 条
`MUL <自己的 controller 下标>` 的 flexrule + 逐顶点载荷。**

对照 mdlc 侧的等价物：`FlexDescriptor`（`src\model.rs:1129`）、`FlexController`（`:1141`）、
`FlexRule { flex, ops }`（`:1295`）、`FlexOpKind`（`src\compile.rs:5817` 的 `Fetch1`）。
**官方自动产出的这三样，正好是 mdlc 的 IR 里已有的概念** ⟹ 实现时不需要新 IR，
只需要在「读 FBX」时**合成**这三样。

⭐⭐⭐ **而 `flexfile` 指向 FBX 时官方会崩 —— 但只在与 `flex` 语句组合时**：

`oracle_fbx_flexfile_rule.js`（2×2 单变量矩阵，模型源固定 `morph.fbx`）：

| 用例 | `flexfile` | `flex` 名 | exit | flexdesc |
|---|---|---|---|---|
| `V1_vta_nameV1` | **`eo1.vta`** | `v1` | **0** | `["wide","tall","v1"]`（3，**共存**） |
| `V2_vta_nameWide` | **`eo1.vta`** | `wide` | **0** | `["wide","tall"]`（2，**去重**） |
| `V3_fbx_nameV1` | `morph.fbx` | `v1` | **−1** | 崩 |
| `V4_fbx_nameWide` | `morph.fbx` | `wide` | **−1** | 崩 |

⟹ ⭐ **崩因是 `flexfile` 指向 `.fbx`，与 `flex` 的名字无关**（V3/V4 同崩）。

**但 `flexfile "<fbx>"` 单独写（无 `flex` 语句）不崩**：
`X6_flexfile_only` / `W5_morphfbx_selffile` 都是 `exit=0`，
且产物与「什么都不写」**逐字段相同**（`desc=["wide","tall"] ctrl=2 rules=2`）
⟹ **它是个静默的空操作**。

`oracle_fbx_isolate.js`（6 用例）复核了这条边界：

| 用例 | 模型源 | `flexfile` | exit | 结果 |
|---|---|---|---|---|
| `W1_rigfbx_vta_flex` | `rig.fbx`（无 shape key） | `eo1.vta` | 0 | `desc=["v1"]`，`flexes 0 bytes`（顶点对不上） |
| `W2_morphfbx_vta_flex` | `morph.fbx`（有 shape key） | `eo1.vta` | 0 | `desc=["wide","tall","v1"]`，`888 bytes (3 flexes)` |
| `W3_smd_vta_flex` | `fxr.smd`（对照组） | `eo1.vta` | 0 | `desc=["v1"]`，`480 bytes` |
| `W4_morphfbx_auto` | `morph.fbx` | — | 0 | `desc=["wide","tall"]`（自动） |
| `W5_morphfbx_selffile` | `morph.fbx` | `morph.fbx` | 0 | 与 W4 逐字段相同（**空操作**） |
| `W6_rigfbx_selffile` | `rig.fbx` | `rig.fbx` | 0 | `desc=[]`（无 shape key） |

⭐⭐ **结论：FBX 的 shape key 自动注册与 `.vta` 显式 flex 可以共存**（V1/W2）
⟹ 用户既能拿到 FBX 的表情，也能继续用 `.vta` 补充额外的 flex。
**唯一禁止的组合是 `flexfile "<某.fbx>"` + `flex` 语句。**

⭐⭐ **另一个陷阱**：`Y4_twobody`（`$body body "rig.fbx"` + `$body hat "morph.fbx"`）**成功**，
但日志里是**两行** `flexes`：

```
flexes           0 bytes (2 flexes)     ← 第一个 bodypart（rig.fbx，无 shape key）
flexes         888 bytes (2 flexes)     ← 第二个 bodypart（morph.fbx，有 shape key）
```

⟹ **`flexes` 统计行是逐 bodypart 打的**，不是全模型合计。
（这也解释了 §1.11 里为什么有的用例打两行。）

⭐ **`eyelid` / `mouth` / `flexcontroller` 在 FBX 模型上都能用**（`oracle_fbx_eyelid.js`，5 用例）：

| 用例 | 形态 | exit | 结果 |
|---|---|---|---|
| `Z1_eyelid_vta_on_fbx` | `$model body "morph.fbx" { eyelid upper_right "eo1.vta" ... }` | **0** | `desc=["wide","tall","upper_right","upper_right_lowerer","upper_right_neutral","upper_right_raiser"]`（6）—— **shape key 的 2 个与 eyelid 的 4 个共存** |
| `Z2_mouth_on_fbx` | `mouth 0 "mouth" "head" ...` | −1 | `unknown mouth link 'head'` ⚠️ **夹具骨骼名错**（`morph.fbx` 只有 1 根骨骼），**不是 FBX 问题** |
| `Z3_flexctrl_on_fbx` | `flexcontroller eyes range -30 30 eyes_updown` | **0** | `desc=["wide","tall"] ctrl=3`（2 自动 + 1 手工） |
| `Z4_flexrule_on_fbx` | `flexrule "wide" = 1.0` | −1 | `unknown model option "flexrule"` ⚠️ **夹具语法错**：官方 `$model` 块里的 flexrule 写法是 `%<名> = <表达式>`（见 `survivors_facerules.qci`），没有 `flexrule` 关键字 |
| `Z5_fbx_body_smd_face` | `$body body "rig.fbx"` + `$model face "fxr.smd" { flexfile "eo1.vta" flex "v1" frame 1 }` | **0** | `bones=7 desc=["v1"]` —— **FBX 与 SMD 部件各带各的 flex** |

⟹ ⭐ **只有 `flex` 语句（配合 FBX 作 `flexfile`）是雷区，`eyelid` / `flexcontroller` 都安全。**

### 1.7 单位与缩放

`b_rig_body.mdl`（源 `rig_anim.fbx`）：`Skeleton` 的 `S=(100,100,100)`、`Spine` 的 `T=(0,10,0)`
⟹ MDL 里 `Spine.pos = [0, 1000, 0]`、`Head1.pos = [0, 2000, 0]`（**×100**）。
但 VVD 顶点 `[-3,0,-3]..[3,45,3]`（**×1**，与源网格一致）。

`f_mixamo.mdl`（源 `mixamo_style.fbx`，其 `Skeleton` 的 `S=(1,1,1)`）：
`Spine.pos = [0, 10, 0]`、`Head1.pos = [0, 20, 0]`（**×1**）。

⟹ 结论：**没有隐藏的单位换算 —— 官方把 FBX 的局部变换原样传递**（含父节点的 scale）。
那个 ×100 是 **Blender 默认导出时压在骨架根节点上的 m→cm 缩放**，不是官方引入的。

⚠️ **但骨骼被 ×100 而网格顶点没有** ⟹ 该夹具的骨架比网格高 100 倍。
这究竟是「官方本来就不给网格顶点乘父节点 scale」还是「夹具本身不自洽」，
**本次调研未定案**，实现前需要用一份真实 Source 绑定 FBX 复验。

### 1.7b ⭐⭐⭐⭐ 轴向：官方**不做任何变换**，原样透传

`gen_fbx_axis.py` 造出三份**同一骨架、只有 Blender 导出轴向不同**的 FBX
（`axis_forward` / `axis_up` 是导出器参数，决定 FBX 内部怎么写坐标）：

| 样本 | 导出参数 | VVD 包围盒 | 根骨骼 `Skeleton` 的 quat |
|---|---|---|---|
| `axis_yup` | forward `-Z`, up `Y`（Blender 默认） | `[-3, 0, -3]..[3, 45, 3]`（**Z 是长轴**） | `[-0.7071, 0, 0, 0.7071]`（−90° 绕 X） |
| `axis_zup` | forward `Y`, up `Z`（Max / Source 风格） | `[-3, -3, 0]..[3, 3, 45]`（**Z 是长轴**） | `[0, 0, 0, 1]`（单位） |
| `axis_yup_max` | forward `X`, up `Y` | `[-3, 0, -3]..[3, 45, 3]` | `[-0.5, -0.5, -0.5, 0.5]` |

⭐⭐ **判读**：
- `axis_yup`（Blender 默认）导出时会把几何**转成 Y-up**（长轴落在 Y），
  于是官方读到的网格长轴是 Y；但产物 VVD 的包围盒**长轴在 Z（45）**
  ⟹ 官方**做了一次 Y-up → Z-up 的还原**（由根节点那个 −90° 绕 X 的 quat 承担）。
- `axis_zup` 导出时**不做转换**（长轴本来就在 Z），官方读到的就是 Z-up，
  根骨骼 quat 是**单位阵**，产物包围盒长轴也在 Z ⟹ 一致。
- ⟹ ⭐⭐ **官方既不做「强制 Y-up」也不做「强制 Z-up」**，它只是把 FBX 的
  **根节点变换原样搬进骨骼表**（`Skeleton` 的 quat 就是 FBX 场景根的朝向），
  于是**只要 FBX 的根变换与几何自洽，产物就自洽**。
- ⚠️ **反面含义**：如果用户的 FBX 导出轴向选错（例如本该 Z-up 却导成 Y-up 且根变换没补偿），
  官方**不会救**，产物就是躺倒的。⟹ mdlc 的 `srcaxis` 语法（§4.3）应当
  **默认不干预**（与官方一致），只在用户显式写时才施加一次旋转。

> ⚠️ `axis_yup_max` 的 `Skeleton` quat `[-0.5,-0.5,-0.5,0.5]` 与 `axis_yup` 不同，
> 但两者产物包围盒相同 ⟹ **根变换的不同写法可以表达同一个朝向**，
> 这进一步说明官方是**原样搬运**而非「归一化到某个规范形式」。

> ⚠️ 探针教训：`mstudiobone_t` 的 `parent` 在 **`@4`**，不是 `@0x80`
> （`sznameindex@0` / `parent@4` / `bonecontroller[6]@8` / `pos@0x20` / `quat@0x2c` /
> `rot@0x3c` / `posscale@0x48` / `rotscale@0x54` / `poseToBone@0x60` / `qAlignment@0x90` /
> `flags@0xa0` / `proctype@0xa4` / `procindex@0xa8` / `physicsbone@0xac` /
> `surfacepropidx@0xb0` / `contents@0xb4` / `unused[8]@0xb8`，步长 216）。
> 首版读 `@0x80` 得到 `parent=-2147483648` / `859553070` / `-1082130432` 这种垃圾值。

### 1.8 ⭐⭐⭐⭐⭐ 动画栈：官方**恒取第一条**，`$sequence` 的名字完全不参与

这是本次调研最重要的发现，也是 UX 必须解决的问题。

**第一轮（`oracle_fbx_ux.js` 的 K/L/M）**：`$body` 与 `$sequence` 同指 `twostack.fbx`
⟹ 三个用例全部只得 `frames=1`，**零信息量**（踩到 §1.11 的陷阱 1）。

**第二轮（`oracle_fbx_stack.js`）**：`$body body "rig.fbx"` + `$sequence <名> "twostack.fbx"`：

| 用例 | `$sequence` 名 | anim 名 | frames |
|---|---|---|---|
| `T1_seq_walk` | walk | `@walk` | 6 |
| `T2_seq_run` | run | `@run` | 6 |
| `T3_seq_idle` | idle（**不匹配任何栈名**） | `@idle` | 6 |
| `T4_anim_twostack` | （`$animation a1`） | `a1` | 6 |

⟹ 三个名字的帧数**完全相同** ⟹ 官方**不看名字**。

**第三轮（决定性，`gen_fbx_order.py` + `oracle_fbx_order.js`）**：
用 Blender 造出 `order_wr.fbx`（walk 在前）与 `order_rw.fbx`（run 在前），
两者结构相同、**只有 NLA 栈顺序不同**（各 100780 B）：

| 样本 | walk | run | idle |
|---|---|---|---|
| `twostack`（walk 在前） | 6 | 6 | 6 |
| `order_wr`（walk 在前） | 6 | 6 | 6 |
| **`order_rw`（run 在前）** | **12** | **12** | **12** |

⭐ **判读：官方恒取「第一条 NLA 栈」**；顺序翻转后三个名字全部 6→12 帧。
`ufbx` 侧证实这些栈确实存在且有不同时长：

```
twostack.fbx / order_wr.fbx:
  stack[0] name="walk"           dur=0.1667s (30fps 下 5.0 帧)
  stack[1] name="run"            dur=0.3750s (30fps 下 11.2 帧)
  stack[2] name="Skeleton|run"   dur=0.3750s
  stack[3] name="Skeleton|walk"  dur=0.1667s
order_rw.fbx:
  stack[0] name="run"  ← 顺序翻转
  stack[1] name="walk"
```

⟹ ⭐⭐⭐ **用户若想用第 2 条栈，官方路径下无解（只能把栈拆成独立文件）。**
`studiomdl.exe` 的串扫描也印证了这点：有 `FbxAnimStack` / `AnimStack` / `AnimationLayer`
（共 11 处），但 **`GetAnimationStack` / `CurrentAnimationStack` / `FbxCriteria` / `GetMember`
全部 0 命中** ⟹ 官方**没有任何「按名字取栈」的代码**。

**顺带定案的命名规则**：
- FBX 作 `$sequence` 源 ⟹ 动画名 = **`@` + `$sequence` 的名字**（`@walk` / `@run` / `@idle`）
- FBX 作 `$animation` 源 ⟹ 动画名 = **`$animation` 给的名字**（`a1`，**不带 `@`**）

### 1.9 材质：FBX 材质名**原样**进 texture 表

`oracle_fbx_ux.js` 的 H/I/J 三例（`mat.fbx` 的材质名分别是 `face` 与 `models/mymod/face`）：

| 用例 | QC | `textures` 表 | `cdtextures` 表 |
|---|---|---|---|
| `H_mat_face` | `$body body "mat.fbx"` + `$cdmaterials "models/mymod"` | `[face]` | `[models\mymod\, ]`（**反斜杠 + 尾部空串**） |
| `I_mat_slash` | 同上（材质名 `models/mymod/face`） | `[models/mymod/face]` | 同上 |
| `J_mat_nocd` | 同上但**省略 `$cdmaterials`** | `[face]` | `[]`（空表，`textures` 少 4 B） |

⟹ ⭐ **FBX 材质名被直接当作 Source 材质名写进 texture 表**；
`$cdmaterials` **只**决定 `cdtexture` 搜索路径，**不改写**材质名（`models/mymod/face` 原样保留）。

⭐ **FBX 没有材质时，官方合成 `debug/debugempty`**（K/L/M/N/O/P/R/S 全部如此），
与 `fbx2dmx` 产出的 DMX 里 `"name" "debugempty"` / `"mtlName" "debug/debugempty"` 一致。

### 1.10 多网格：官方**合并**，bodypart 数由 `$body` 语句数决定

| 用例 | QC | bodyparts | 结果 |
|---|---|---|---|
| `N_twomesh` | `$body body "twomesh.fbx"`（FBX 内 `body` + `hat` 两块网格） | **1** | 两块**合并**进同一个 `mstudiomodel_t`（48 顶点 = 24+24），只报 `mesh=1` |
| `O_twomesh_bg` | `$bodygroup "studio" { studio "twomesh.fbx" }` | 1 | 与 N 相同，只是 bodypart 名变成 `studio` |
| `P_two_fbx` | `$body body "rig.fbx"` + `$body hat "box.fbx"` | **2** | 各成一个 bodypart；骨骼表**合并**（5 根） |

⟹ ⭐ **FBX 内部的网格边界在官方路径下会丢失**；要保留部件必须拆成多个 FBX 或用多个 `$body`。

⭐ **SMD + FBX 混用完全可行**（`Q_mix_smd_fbx`，`exit=0`）：日志里**同时**出现
`SMD MODEL fxr.smd` 与 `DMX Model ...box.fbx`，材质表合并为
`[body, eye_right, eye_left, jaw_mesh, debug/debugempty]`，骨骼表合并（`root, head, jaw, box`），
索引按 bodypart 顺序递增。

### 1.11 ⭐ 陷阱清单（`oracle_fbx_footguns.js`，8 用例）

| 用例 | 形态 | exit | 结果 | 性质 |
|---|---|---|---|---|
| `U1_same_body_seq` | `$body body "twostack.fbx"` + `$sequence walk "twostack.fbx"` | 0 | `frames=1` | ⚠️ **静默** |
| `U2_same_body_anim` | 同上，但走 `$animation a1 "twostack.fbx"` | 0 | `frames=1` | ⚠️ **静默** |
| `U3_noanim` | `$sequence idle "rig.fbx"`（该文件无动画） | 0 | `frames=1` | ✅ 合理 |
| `U4_anim_as_body` | `rig_anim.fbx` 同时作 body 与 sequence | 0 | `frames=1` | ⚠️ **静默** |
| `U5_fps_override` | `$sequence idle "twostack.fbx" fps 10` | 0 | `fps=10 **frames=6**` | ⚠️ **字段与重采样脱钩** |
| `U6_scale_option` | `... scale 2.0` | 0 | `frames=6` | 行内 `scale` 被接受 |
| `U7_startloop` | `... loop startloop 2 4` | **−1** | `could not load file '4'` + 崩溃 | ⚠️ **官方解析错** |
| `U8_two_seq_same_fbx` | 两条 `$sequence` 引同一 FBX | 0 | 两条都 `frames=6` | 都取第一条栈 |

⭐ **陷阱 1（最危险）**：**同一个 FBX 既作网格源又作动画源 ⟹ 静默只得 1 帧。**
`U1`/`U2`/`U4` 三例都命中，`exit=0`、无任何警告、产物"成功"。

⭐⭐ **这是 FBX 专属行为，SMD 没有这个陷阱**（`oracle_samefile_smd.js`，5 用例）：

| 用例 | `$body` 源 | `$sequence` 源 | 帧数 |
|---|---|---|---|
| `M1_smd_same` | `ab_z8.smd`（8 帧） | `ab_z8.smd`（**同**） | **8** ✅ 正常 |
| `M2_smd_diff` | `ab_z8.smd` | `abi7.smd`（异） | 5 |
| `M3_fbx_same` | `rig_anim.fbx` | `rig_anim.fbx`（**同**） | **1** ❌ 陷阱 |
| `M4_fbx_diff` | `rig.fbx` | `rig_anim.fbx`（异） | 6 |
| `M5_smd_same_5f` | `abi7.smd`（5 帧） | `abi7.smd`（**同**） | **5** ✅ 正常 |

⟹ **SMD 同文件完全正常（8 帧源出 8 帧、5 帧源出 5 帧），只有 FBX 退化成 1 帧。**
因此 mdlc 的拒绝判据**必须按源格式分流** —— 对 SMD 同文件放行，
否则会误伤大量既有工程（parity 语料里 `$body x.smd` + `$sequence y x.smd` 是常见写法）。

⭐ **精确边界已单变量定案**（`oracle_fbx_samefile.js`，6 用例）：
触发条件是**「同一个文件」**，**不是**「body 源本身带动画」：

| 用例 | `$body` 源 | `$sequence` 源 | 帧数 |
|---|---|---|---|
| `S1_same_twostack` | `twostack.fbx` | `twostack.fbx`（**同**） | **1** |
| `S2_body_twoseq_riganim` | `twostack.fbx`（有动画） | `rig_anim.fbx`（**异**） | 6 |
| `S3_body_riganim_seq_two` | `rig_anim.fbx`（有动画） | `twostack.fbx`（**异**） | 6 |
| `S4_body_riganim_same` | `rig_anim.fbx` | `rig_anim.fbx`（**同**） | **1** |
| `S5_body_rig_seq_anim` | `rig.fbx` | `anim.fbx`（**异**） | 6 |
| `S6_body_rig_seq_rig` | `rig.fbx` | `rig.fbx`（**同**） | **1** |

⟹ S2/S3 证明**body 源带不带动画都无所谓**（只要文件不同就正常）；
S1/S4 证明**只要文件相同就退化成 1 帧**（哪怕该文件有 6 帧动画）。
（S6 是「同文件 + 都无动画」，1 帧属预期，不构成反例。）

⭐ **陷阱 2**：**`fps` 参数只写 `animdesc.fps` 字段，不参与重采样。**
`U5` 写 `fps 10` 得到 `fps=10 frames=6` —— 但帧数仍是按 30 fps 采出来的 6 帧，
播放时长会变成 6/10 = 0.6 s（源是 0.1667 s）⟹ **动画变慢 3.6×**。

⭐ **陷阱 3**：`loop startloop 2 4` 让官方把 `4` 当成文件名去加载（`could not load file '4'`）
并触发 `EXCEPTION_ACCESS_VIOLATION` ⟹ **官方在 FBX 路径上对 `startloop` 的解析是坏的**。

---

## 2. `animsmith-fbx` 的硬伤（`[实测]`）

### 2.1 ⭐⭐⭐⭐ 它**主动丢弃** morph / blend shape

`animsmith-core` 的 `src/model.rs` 里 `blend_shape|morph|shape_key` **零命中** ⟹
**`Document` 模型里根本没有 morph 的位置**。

`animsmith-fbx::load(morph.fbx)` 的实测输出：

```
[骨骼] 共 2 根（<fbx-root> / body）
[动画] 共 0 个 clip
[资产] meshes=1 instances=1 materials=0 scenes=1
mesh "Cube" primitives=1 verts=24 tris=12
prim[0] mat=None uv=24 影响数直方图[0..4]=[0,0,0,0,0] extra_influence_sets=0
```

⟹ **shape key 被完全静默丢弃**。

它只是**计数并标为 Unsupported**（`capability.rs`）：`morphs_present: true`、
`morph_weights_present: true`、**`whole_document_morphs_preservable: false`**。

⭐ **决定性对照**：官方从**同一个文件**产出了 `flexes 888 bytes (2 flexes)` + `numflexdesc=2`。

### 2.2 ⭐⭐⭐⭐ 但 `ufbx` 本体读得到

直接调 `ufbx`（`_fbxresearch\ufbxprobe\`，`cargo add ufbx@0.11`）读同一个 `morph.fbx`：

```
[blend] blend_deformers=1 blend_shapes=2
  deformer[0] name="Cube" channels=2
    channel[0] name="wide" weight=0.0000 keyframes=1 目标 shape "wide": offsets=8
    channel[1] name="tall" weight=0.0000 keyframes=1 目标 shape "tall": offsets=8
  shape[0] name="wide" num_offsets=8 offset_weights=8
      offset[0] vertex=0 dpos=(-2.5000,0.0000,0.0000)
  shape[1] name="tall" num_offsets=8 offset_weights=8
      offset[0] vertex=0 dpos=(0.0000,0.0000,-2.5000)
```

⟹ **`animsmith` 是主动丢弃，不是 `ufbx` 读不到。** 绑定结构：

```rust
pub struct BlendShape {
    pub element: Element, pub num_offsets: usize,
    pub offset_vertices: List<u32>,
    pub position_offsets: List<Vec3>, pub normal_offsets: List<Vec3>,
    pub offset_weights: List<Real>,
}
pub struct BlendChannel {
    pub element: Element, pub weight: Real,
    pub keyframes: List<BlendKeyframe>,
    pub target_shape: Option<Ref<BlendShape>>,
}
```

### 2.3 ⭐⭐⭐ 采样率：`animsmith` 锁死默认，`ufbx` 可控

`animsmith-fbx\src\lib.rs:454-465` 用 `ufbx::BakeOpts { trim_start_time: true, ..Default::default() }`
⟹ 落在源网格上。实测 `rig_anim.fbx`：`Spine` 的旋转关键帧
`times=[0.00000, 0.04167, 0.08333, 0.12500, 0.16667]` = **1/24 s 间隔 = 24 fps**，
而官方要求 **30 fps**。

`ufbx::BakeOpts` 本身有三个字段（`generated.rs` + `ufbx.h` 逐字注释）：

| 字段 | 默认 | `ufbx.h` 注释 |
|---|---|---|
| `resample_rate: f64` | `30` | *"Samples per second to use for resampling non-linear animation."* |
| `minimum_sample_rate: f64` | `19.5` | *"Minimum sample rate to not resample. … To avoid double-resampling keyframe rates higher or equal to this will not be resampled."* |
| `maximum_sample_rate: f64` | unlimited | *"Maximum sample rate to use, this will remove keys if they are too close together."* |

⚠️ **实测**：把 `resample_rate` 设成 24 / 30 / 60 烘焙 `rig_anim.fbx`，
**三者关键帧数完全相同**（`rot_keys=5`）—— 因为 `minimum_sample_rate = 19.5` 让既有的
24 Hz 键**不被重采样**。⟹ **要复刻官方的「固定 30 fps」，必须显式设
`minimum_sample_rate`（或先把关键帧线性化）。**

### 2.4 其它落差（实现时需自行处理）

| 落差 | 说明 |
|---|---|
| **骨骼集合** | `animsmith` 每 node 一根骨骼（7 根）；官方 4 根（丢网格节点与空叶节点，但见 §1.4 的例外） |
| **UV 翻转** | `animsmith-fbx\src\lib.rs:1352` 已做一次 `1.0 - uv.y`；mdlc 在 `src\smd.rs:527` 解析 SMD 时也翻一次 ⟹ **直连会翻两次** |
| **影响数** | `animsmith` 给 4（`joints: [u16;4]` / `weights: [f32;4]`）；mdlc 上限 **3**（`src\model.rs:5265 MAX_BONES_PER_VERT = 3`）⟹ 需重排 + 截断 + 重归一化 |
| **材质** | `MaterialAsset` 是 **glTF PBR 语义**（base_color / metallic / roughness / occlusion），与 Source 的 `$cdmaterials` + VMT 模型完全不同 |

---

## 3. 构建成本（`[实测]`）

`animsmith-fbx 0.14`（含 `ufbx` 的 C 源码）在本机：

```
cargo build --release   ⟹   Finished in 49.8s   BUILD_EXIT=0
```

**无需额外工具链**：`cc` crate 经 vswhere 找到 VS 2026 Community；
`clang` / `clang-cl` 也已在 `C:\Program Files\LLVM\bin\`。

⟹ **「引入 ufbx 会破坏纯 Rust 构建」的担忧被实测否定。**
（`ufbx` 单独用更轻：`ufbxraw` 探针 3.32 s。许可 `MIT OR Unlicense`。）

---

## 4. UX 设计：QC 里直接引用 FBX

> **前提**：用户明确要求 **不做中间转换**，FBX 直接写进 QC 引用；
> 且明确授权「**哪怕增加一些专有 QC 语法 / TOML 字段，以此换来可控性与人体工程学**」，
> 理由是「**不希望用户在使用 FBX 时需要猜测编译器的内部机制和意图**」。

### 4.0 现状：mdlc 今天遇到 FBX 会怎样（`[实测]`）

这是本方案要消灭的**起点状态**。实测（`target\release\mdlc.exe`，2026-10-01）：

```
$ mdlc build-qc a_body.qc --out outA        # $body body "rig.fbx"
  - D:\...\rig.fbx:0: 读不到 SMD（QC 里引用了它）：stream did not contain valid UTF-8

$ mdlc build-qc b_seq.qc --out outB         # $sequence idle "rig.fbx"
  - sequences[0].smd: 读不到 D:\...\rig.fbx：stream did not contain valid UTF-8
```

⭐⭐ **两条信息都不可用**：

1. **措辞错**：报的是「读不到 SMD」，但用户写的是 FBX。用户第一反应会是
   「我的 SMD 路径写错了？」—— 而真正的原因是这个文件**根本不是 SMD**。
2. **根因藏在错误里**：`stream did not contain valid UTF-8` 是
   `src\compile.rs:370` 的 `read_smd` 用 `read_to_string` 读**二进制文件**的副产物。
   用户要猜出「mdlc 把所有网格源都当文本 SMD 读」才能理解这句话。

**本方案的目标**：把上面两条变成

```
$ mdlc build-qc a_body.qc --out outA
  警告：`$body body "rig.fbx"` 引用的是 FBX 文件（按扩展名识别）。
        已自动注册 2 个 shape key 为 flex：`wide`(帧 1)、`tall`(帧 2)。
        该 FBX 有 4 条动画栈，但本条 `$body` 只取网格 —— 动画栈在 `$sequence` 处选。
  警告：`$sequence idle "rig.fbx"` 与 `$body body "rig.fbx"` 引用了同一个文件。
        官方在这个组合下**静默地**只产出 1 帧（实测 exit=0、无警告），mdlc 不产出这种结果。
        请任选其一：
          · 把动画拆到独立的 FBX 文件（推荐）
          · 用 `$animation idle "rig.fbx"` + `$sequence idle { idle }` 显式声明
```

⟹ **一句话**：从「一个和用户意图无关的 UTF-8 报错」变成「说清发生了什么、默认怎么处理、
想改该怎么写」。

### 4.1 三条设计原则

1. **默认逐条对齐官方。** 同一份 QC 喂官方与喂 mdlc，产物应当一致。
   这是 mdlc 的验收方式（AGENTS.md），也是用户迁移的前提。
2. **官方的静默失败一律改成显式诊断。** 官方在 FBX 路径上有 3 类静默陷阱（§1.11），
   全部表现为 `exit=0` + "成功"的产物。**mdlc 不能复制这种沉默** ——
   这正是用户说的「猜测编译器的内部机制和意图」。
3. **每个隐式决策都有一个显式覆盖语法。** 默认值是什么，写在文档里；
   用户想改，有明确的写法。**新增的语法只做「显式化」，不引入新的隐式规则。**

### 4.2 默认行为（逐条对照官方）

| 决策点 | 官方行为 | mdlc 默认 | 说明 |
|---|---|---|---|
| 材质名 | FBX 材质名原样进 texture 表 | **同官方** | §1.9 |
| 无材质的网格 | 合成 `debug/debugempty` | **同官方** + ⚠️ 告警 | 告警列出可选替代 |
| 一个 FBX 的多块网格 | 合并成一个 model | **同官方** + ⚠️ 告警 | 告警列出各网格名 |
| bodypart 划分 | 由 `$body` 语句数决定 | **同官方** | §1.10 |
| SMD + FBX 混用 | 允许，表按顺序合并 | **同官方** | §1.10 |
| 动画栈 | **恒取第一条** | **同官方** + ⚠️ 告警 | ⚠️ 有多条时告警并列出名字 |
| 重采样率 | 固定 30 fps | **同官方**（30） | §1.5 |
| `animdesc.fps` 字段 | 取 `$sequence ... fps N` | **同官方** | ⚠️ 与重采样率不等时告警 |
| 单位/缩放 | 局部变换原样传递 | **同官方** | §1.7 |
| 轴向 | **不干预**（原样搬运根变换） | **同官方** | §1.7b |
| 骨骼过滤 | 丢网格节点与空叶节点 | **同官方** | §1.4，但需复验例外 |
| **shape key → flex** | **自动注册**（desc + controller + rule + 载荷） | **同官方** | §1.6b；**零 QC 语法** |
| **FBX 上写 `flexfile "<x.fbx>"` + `flex`** | **崩溃** | **报错**（见 §4.4 陷阱 4） | ⚠️ 默认偏离 |
| **FBX 上只写 `flexfile "<x.fbx>"`** | 不崩，但**静默空操作** | **报错** | ⚠️ 默认偏离（用户以为生效了，其实没有） |
| 同一文件既是网格源又是动画源（**仅 `.fbx`**） | 静默 1 帧 | **拒绝**（见 §4.4） | ⚠️ **默认偏离**；SMD 同文件正常，**放行** |

### 4.3 命名原则：按概念命名，不按格式命名

⭐ **语法按「概念」命名，格式由文件扩展名决定。**

这条原则是用户提出的（「如果以后要加 gltf/glb，是否每个格式都要加新语法？」），
它推翻了本节初稿的 `fbx*` 前缀方案。理由是：把下面的九条语法逐条对照
glTF/GLB 的等价概念后，**九条全部是通用概念**（详见下表「glTF 对应物」列）：

| 概念 | FBX 里的叫法 | glTF 里的叫法 |
|---|---|---|
| 取哪个子网格 | mesh / geometry | mesh / primitive |
| 无材质时的兜底名 | material | material |
| 单位缩放 | unit scale | unit scale（米制） |
| 上轴 | axis up | Y-up（规范固定） |
| 用哪条动画 | anim stack | `animations[]` |
| 重采样率 | fps | fps |
| 形变目标 | shape key / blend shape | **morph target** |

⟹ 若按格式命名，加 glTF 时这九条要**原样重抄**成 `gltfpart` / `gltfmaterial` / …，
加 OBJ 再抄一遍。那不是「每个格式加新语法」，而是「**每个格式把同一套语法重抄一遍**」，
比原始担忧更糟。

**格式信号从哪来？** 从文件扩展名 —— 这正好复用 R34 已有的决定
（「资产引用必须写完整扩展名」，`src\compile.rs:353 resolve_smd_path`）。
当时那条改动是为了消除「同一 token 在不同上下文解析到不同文件」的歧义，
顺带让**扩展名成为可靠的格式信号**：`resolve_smd_path` 已经拿到了路径，
按 `.smd` / `.fbx` / `.glb` 分派 reader 是免费的。

于是：

- **加 glTF 支持 = 加一个 source reader，新增语法 0 条。** 用户把 `.fbx` 换成 `.glb`，同一套选项照用。
- 选项挂在 `$body` / `$model` / `$sequence` 块内，**每个块各管各的文件**
  ⟹ 同一个模型里 `body` 用 FBX、`hat` 用 GLB 天然可行，不需要全局格式开关。
- 用户只需要学**一套**词汇表，而不是每个格式一套。

**例外判据：只有概念本身在某格式里独有时，才带格式前缀。**
例如 FBX 的 inherit mode（Mixamo 的 `scale 0.01` 那种 Maya 式继承）、glTF 的 extension 开关。
这类旋钮写成 `fbxinheritmode` 反而更清楚 —— glTF 根本没有对应物，强行通用化才是误导。

> **规则：通用概念用 `src*`；格式独有的旋钮用 `<格式><旋钮>`。前者是常态，后者是例外。**

沿用 mdlc 既有扩展命令的约定（`README.md:493-527`）：
**TOML 字段名去掉下划线、前面加 `$`**；命令名大小写不敏感；QC 自上而下解释、后写的赢。
（`src*` 与 `src\compile.rs` 的 `resolve_src` / `mesh_sources` 词汇一致。）

**A. 网格侧（`$body` / `$model` / `$bodygroup { studio }` 的块内选项）**

| QC | TOML | 作用 | 省略时 | glTF 对应物 |
|---|---|---|---|---|
| `srcpart "body"` | `src_parts = ["body"]` | 只取指定名字的网格；**可重复写**（`srcpart "a" srcpart "b"`） | 取全部并合并（官方） | mesh / primitive |
| `srcmaterial "face"` | `src_material = "face"` | 网格**没有**材质时用它 | `debug/debugempty`（官方） | material |
| `srcscale 1.0` | `src_scale = 1.0` | 顶点与骨骼位移的缩放 | `1.0`（官方） | 同 |
| `srcaxis "z"` | `src_axis = "z"` | **强制**上轴（`y` / `z`）；见下 | **不干预**（官方，§1.7b） | Y-up 固定 |

> ⭐ **`srcaxis` 只在源文件自身轴向坏掉时才需要。** §1.7b 实测表明：只要 FBX 的
> **根节点变换与几何自洽**（正常导出器都保证这点），官方与 mdlc 的默认行为就已经正确 ——
> 无论是 Y-up 还是 Z-up 导出。`srcaxis` 是给「根变换丢了 / 被清掉」的坏文件准备的逃生门，
> **不是常规选项**。默认必须与官方一致（不干预）。

> ⚠️ **多值选项的写法：一个选项一个值，靠重复来累加。**
> `srcpart` / `srcshapekey` / `srcshapekeyorder` **都只吃紧随其后的一个 token**：
>
> ```qc
> srcpart "body" srcpart "hair"      // ✅ 两个网格
> srcpart "body" "hair"              // ❌ "hair" 不是选项名，会报未知选项
> srcpart "hat" srcaxis "y"          // ✅ srcaxis 正常生效
> ```
>
> 这与 `$attachment` / `$sequence` 那类「后面跟定长参数」的命令一致，
> 也是**唯一**能保证选项之间不互相吞并的写法。名字里带空格用引号解决
> （`srcpart "my mesh"`），不需要靠「读到行尾」来支持。
>
> 📌 首版实现曾把这三个选项写成「读到行尾」，结果是
> `srcpart "hat" srcaxis "y"` 里的 `srcaxis` 被当成第二个网格名 ——
> **不报错**（不存在的网格名会被静默过滤），只是 `srcaxis` 静默失效。
> 回归测试：`srcpart_does_not_swallow_the_following_option` /
> `srcshapekey_does_not_swallow_the_following_option` / `srcpart_is_repeatable`。

**B. 动画侧（`$sequence` / `$animation` 的块内或行内选项）**

| QC | TOML | 作用 | 省略时 | glTF 对应物 |
|---|---|---|---|---|
| `srcstack "walk"` | `src_stack = "walk"` | **按名字选动画栈** | 第一条（官方）+ 多条时告警 | `animations[]` 的 name |
| `srcfps 30` | `src_fps = 30` | 重采样率 | `30`（官方） | 同 |

**C. flex 侧（`$model` 块内）**

⭐ **首先注意：FBX 的 shape key 默认就会自动注册（§1.6b），用户什么都不用写。**
下面这些语法**只在需要偏离自动行为时才用**。

| QC | TOML | 作用 | 省略时 | glTF 对应物 |
|---|---|---|---|---|
| `srcshapekey "wide"` | `src_shape_keys = ["wide"]` | **只取指定名字的形变目标**；可重复、按写出顺序定帧号 | 取全部（官方） | morph target |
| `srcshapekeyorder "tall" "wide"` | `src_shape_key_order = [...]` | **显式指定帧号顺序** | 文件顺序（官方） | 同 |
| `srcshapekeyignore` | `src_shape_key_ignore = true` | **完全忽略形变目标**（不产出任何 flex） | 自动注册（官方） | 同 |

⚠️ **不要在 FBX 源上写 `flexfile "x.fbx"` + `flex "name" frame N`** ——
**官方在这种组合下必崩**（§1.6b 的 2×2 矩阵）。
只写 `flexfile "x.fbx"`（无 `flex`）虽不崩，但它是**静默空操作**（什么都不做）。
mdlc 对这两种写法都会**报错**并指向 `srcshapekey*` 系列（§4.4 陷阱 4）。

⭐ **`.vta` 路径完全不变**：`flexfile "x.vta"` + `flex "name" frame N` 仍然照旧
（帧号 = 序号 + 1）。**新增的 `srcshapekey*` 只服务「带形变目标的几何源」。**

> 设计理由：官方把「shape key → flex」做成了**全自动、无语法、不可控**，
> 而它同时**禁止**任何显式 flex 语法（一写就崩）。
> 对用户来说这是「**只能猜**」的典型场景 —— 想看某个 shape key 对应第几帧、
> 想跳过某个 shape key、想改帧号顺序，**官方路径下一个都做不到**。
> `srcshapekey*` 三个语法正好补上这三件事，且**默认值与官方逐条一致**。

### 4.4 诊断设计：把官方的静默失败变成显式提示

这是整个 UX 的核心。三类陷阱各有对应的处置：

**陷阱 1 —— 同一文件既是网格源又是动画源 ⟹ 官方静默 1 帧**

⭐ 触发条件是**文件路径相同**（已单变量定案，见 §1.11 的 S1–S6 表），
与「body 源带不带动画」无关。

⭐⭐ **这是 FBX 专属**：SMD 同文件完全正常（`oracle_samefile_smd.js` 的
`M1_smd_same` 8 帧源出 8 帧、`M5_smd_same_5f` 5 帧源出 5 帧）。
mdlc 的判据因此**必须按源格式分流**，只对 `.fbx` 报错。

⭐⭐ **出路只有一条**（`oracle_samefile_fix.js`，6 用例实测）：

| 用例 | 写法 | 官方帧数 |
|---|---|---|
| `N1_direct_same` | `$body "rig_anim.fbx"` + `$sequence idle "rig_anim.fbx" fps 30` | **1** |
| `N2_anim_block_same` | 同上 + `$animation anim "rig_anim.fbx"` + `$sequence idle { anim }` | **1** ❌ |
| `N3_anim_block_diff` | `$body "rig.fbx"` + `$animation anim "rig_anim.fbx"` + `$sequence idle { anim }` | 6 ✅ |
| `N4_seq_by_name` | `$body "rig_anim.fbx"` + `$animation anim "rig_anim.fbx"` + `$sequence idle "anim"` | **1** ❌ |
| `N5_anim_then_seq_same_file` | 同上 + `$sequence idle "rig_anim.fbx" fps 30` | **1** ❌（还多出一条 `@idle`） |
| `N6_twostack_anim_block` | `$body "twostack.fbx"` + `$animation anim "twostack.fbx"` + `$sequence idle { anim }` | **1** ❌ |

⚠️ **`$animation` 走不通** —— N2/N4/N5/N6 全部仍是 1 帧。
「同一文件」这个条件一旦成立，**无论怎么改写 QC 都拿不回动画**
（N5 甚至会同时产出 1 帧的 `anim` 和 1 帧的 `@idle`）。
唯一有效的出路是 **N3：把动画放到另一个 FBX 文件**。

mdlc **拒绝**，并给出可操作的出路：

```
错误：`$sequence idle` 引用的 twostack.fbx 已经被 `$body body "twostack.fbx"` 当作网格源加载。
      官方在这个组合下**静默地**只产出 1 帧（实测 exit=0、无警告），mdlc 不产出这种结果。
      改 QC 没有用 —— `$animation` 块、按名引用、重写 `$sequence` 都被实测证明仍是 1 帧
      （`oracle_samefile_fix.js` 的 N2/N4/N5/N6）。唯一有效的做法是：
        · 把动画拆到独立的 FBX 文件（唯一可行）
        · 若确实要 1 帧静态姿态，写 `numframes 1`
```

> ⚠️ **这是两处默认偏离之一**（另一处见陷阱 4）。理由是官方结果**没有任何可用性**（1 帧动画），
> 而它**没有任何诊断**。若用户确实需要「1 帧静态姿态」，写 `numframes 1` 即可显式表达。

**陷阱 2 —— `fps` 字段与重采样率脱钩**

```
警告：`$sequence walk` 写了 `fps 10`，但 FBX 的重采样率是 30（官方固定值）。
      产物会有 6 帧、`animdesc.fps = 10` ⟹ 播放时长 0.6 s（源 0.1667 s，慢 3.6 倍）。
      要让两者一致，写 `srcfps 10`；要保留官方行为，忽略本警告。
```

**陷阱 3 —— 多条 NLA 栈（官方静默只取第一条）**

```
警告：`$sequence walk` 引用的 twostack.fbx 有 4 条动画栈：
        [0] "walk"          0.1667 s（30 fps 下 5 帧）
        [1] "run"           0.3750 s（30 fps 下 11 帧）
        [2] "Skeleton|run"  0.3750 s
        [3] "Skeleton|walk" 0.1667 s
      官方恒取第一条（"walk"），`$sequence` 的名字**不参与选择**。
      要用别的栈，写 `srcstack "run"`。
```

**陷阱 4 —— 在 FBX 源上写 `flexfile "<某.fbx>"` + `flex` ⟹ 官方崩溃**

⚠️ 精确边界（`oracle_fbx_flexfile_rule.js` 的 2×2 矩阵，§1.6b）：

| 写法 | 官方 |
|---|---|
| `flexfile "x.fbx"` **单独** | ✅ 不崩，但是**静默空操作**（与什么都不写逐字段相同） |
| `flexfile "x.fbx"` **+ `flex "name" frame N`** | ❌ `EXCEPTION_ACCESS_VIOLATION` |
| `flexfile "x.vta"` + `flex "name" frame N` | ✅ 正常（`.vta` 路径不变） |

```
错误：`$model` 块里对 FBX 源 `morph.fbx` 写了 `flexfile` + `flex`。
      官方在这个组合下**必然崩溃**（实测 `EXCEPTION_ACCESS_VIOLATION`）；
      只写 `flexfile "morph.fbx"` 不崩，但它是**静默空操作**（什么都不做）。
      而**不写**任何 flex 语法时，官方会自动把 shape key 注册成 flex。
      FBX 的 shape key 默认就会自动变成 flex，不需要显式声明。
      要控制取哪些 / 顺序 / 忽略，用：
        · `srcshapekey "wide"`              只取指定的 shape key
        · `srcshapekeyorder "tall" "wide"`  显式指定帧号顺序
        · `srcshapekeyignore`               完全忽略 shape key
      （`.vta` 源不受影响，仍用 `flexfile` + `flex ... frame N`。）
```

> 这条与陷阱 1 是同一类：官方在这个组合上**要么静默给坏结果、要么直接崩**，
> 两者都**没有可用诊断**。mdlc 报错并给出出路。

**其它诊断**（都是 `exit=0` 但结果可疑的情况）：

| 触发条件 | 提示 |
|---|---|
| FBX 的多块网格被合并 | 列出网格名 + 「要分开请用 `srcpart` 或拆成多个 `$body`」 |
| 网格没有材质 | 「已合成 `debug/debugempty`；要改用别的写 `srcmaterial`」 |
| `$collisionmodel` 吃 FBX 且凸体分解退化 | 官方会打 `building single convex`；mdlc 应转成明确警告 |
| FBX 里的骨骼被过滤掉 | 列出被丢的节点名（官方静默丢弃） |
| FBX 有 shape key | 「已自动注册 N 个 flex：`wide`(帧1) `tall`(帧2)；要控制请用 `srcshapekey*`」 |

### 4.5 逐问题方案

| 问题 | 方案 |
|---|---|
| **多栈 FBX**（§1.8，最大障碍） | 默认第一条 + 告警列出全部；`srcstack "名"` 显式选择；找不到该名字时**硬报错**并列出可用名 |
| **材质命名**（§1.9） | 默认原样透传（官方）；`srcmaterial` 兜底无材质的情况 |
| **多网格 → 部件**（§1.10） | 默认合并 + 告警；`srcpart` 选网格；要多个 bodypart 就写多条 `$body` |
| **单位/缩放**（§1.7） | 默认原样透传（官方）；`srcscale` 显式缩放 |
| **轴向**（§1.7b） | **默认不干预**（官方；根变换原样搬运）；只在坏文件上用 `srcaxis` 强制 |
| **重采样率**（§1.5） | 默认 30（官方）；`srcfps` 覆盖 |
| **shape key → flex**（§1.6b） | **默认自动注册**（与官方逐字段一致：desc + controller `[0,1]` + `MUL` rule + 载荷）；`srcshapekey*` 控制取哪些/顺序/忽略。⚠️ **FBX 上禁止显式 flex 语法**（官方必崩） |
| **骨骼过滤**（§1.4） | 默认同官方；被丢的节点**列出来**（官方静默） |

### 4.6 与官方的有意偏离（全部列出，便于审计）

| # | 偏离 | 理由 |
|---|---|---|
| 1 | **同一文件既是网格源又是动画源 ⟹ 报错**（官方静默 1 帧）**——仅当该文件是 `.fbx`** | 官方结果不可用且无诊断（§4.4 陷阱 1）；**SMD 同文件正常**（8 帧源出 8 帧），故不报错 |
| 2 | **FBX 源上写 `flexfile "<某.fbx>"` + `flex` ⟹ 报错**（官方必崩） | 官方 4 个变体全部 `EXCEPTION_ACCESS_VIOLATION`；只写 `flexfile "x.fbx"` 虽不崩但是**静默空操作**；而 FBX 的 shape key 本就自动注册（§1.6b） |
| 3 | 新增 `srcpart` / `srcmaterial` / `srcscale` / `srcaxis` / `srcstack` / `srcfps` | 用户授权（§4 前提）；官方对这些决策**没有**任何语法 |
| 4 | 新增 `srcshapekey` / `srcshapekeyorder` / `srcshapekeyignore` | 官方的自动注册**不可控**（取哪些 / 顺序 / 忽略都做不到），且禁止显式语法 |
| 5 | 对 §1.11 的全部陷阱发诊断 | 官方静默 |
| 6 | `$collisionmodel` 的 `building single convex` 升级为明确警告 | 官方只打一行 WARNING 就继续 |
| 7 | **shape key 名含 `_` 时保留原名**（官方会做 token 重排，`t_2e3` → `2e3_t`） | 官方行为让 QC 作者必须猜内部机制（`%t_2e3` 还是 `%2e3_t`？）；用户明确反对「猜编译器内部机制」（§4.0 前提） |
| 8 | **shape key 名含 `_` 时照常写 `flexrule`**（官方 `numflexrules` 变 0） | 官方是**静默**丢掉规则 ⟹ 表情不动且不报错，属有害行为 |
| 9 | **vertanim 顺序 = 焊接顶点升序**（官方 FBX 是「控制点首次出现序 × 组内降序」） | 官方的顺序取决于它自己的顶点焊接顺序，**该顺序 mdlc 与官方本就不同**（见偏离 11），复刻它毫无意义；载荷内容（顶点集合 + delta 多重集）完全一致，已由 `k4_flex_align.js` 按 VVD 位置对齐验证（16/16） |
| 10 | **丢弃 `nrm_off`，`ndelta` 恒 0** | 与官方 FBX 路径一致（`axisprobe.fbx` 的 `nrm_off` 非零而官方产物 `ndelta=(0,0,0)`）；`flex` 的 `.vta` 路径仍照常算 `ndelta` |
| 11 | **`mesh.vertexdata` / `model.vertexdata` 写 0**（官方写运行期堆指针） | 官方把**进程地址**写进产物（语料 87/87 个 mesh 非 0，如 `0x7de5088c`）—— 那是 ASLR 后的堆地址，**逐字节对齐既不可能也无意义**；这两个字段由引擎在加载时填 |

> ⚠️ **偏离 11 的连带后果**：`k4_morph.mdl` 与官方产物**不可能逐字节一致**
> （还有 `checksum`、1 ULP 级的 `hull_min/hull_max`、`seqdesc.bbmin/bbmax`）。
> 验收口径是**逐字段**（`docs/_probe/mdl_field_dump.js`）而非逐字节。

> ⚠️ **跨工具兼容性**：偏离 1–4 意味着**用了新语法的 QC 不能直接跑官方工具**。
> 这与既有扩展命令（`$optimizevtx` 等）的处理方式一致 ——
> README 应明确写出这条，并指出「要跨工具通用请改用 TOML 侧字段」。
> ⚠️ 注意偏离 1、2 是**报错**而非新语法：**没写新语法的 QC 也跑不了官方**
> （官方在那两种组合下给的是坏结果或崩溃），这一点必须写清楚。

### 4.7 完整示例

```qc
$modelname "models/survivors/linnea.mdl"
$cdmaterials "models/survivors/linnea_replaces_zoey/"

// ── 网格：直接吃 FBX，只取 body 网格，没有材质时用 face ──
$body body "linnea.fbx" {
    srcpart "body"          // 该 FBX 里还有 hair / eyes，不取
    srcmaterial "face"      // FBX 里没给材质时的兜底
    srcscale 1.0            // 原样（官方行为）
}

// ── 第二个部件：另一个 FBX ──
$body hat "hat.fbx"

// ── 表情：什么都不用写，shape key 自动注册（官方行为） ──
// 要控制，用 srcshapekey*：
$body face "linnea_face.fbx" {
    srcshapekeyorder "blink" "smile" "wide"   // 显式指定帧号 1..3
    // srcshapekey "blink"                     // 或者只取某几个
}
// 想完全忽略 shape key（不要表情）：
// $body face "linnea_face.fbx" { srcshapekeyignore }

// ⚠️ 不要在 FBX 源上写 flexfile / flex —— 官方会崩，mdlc 会报错。
//    `.vta` 源照旧：flexfile "x.vta" + flex "name" frame 1

// ── 动画：显式选第二条栈 ──
$sequence walk "anims.fbx" fps 30 {
    srcstack "walk"
}

$sequence run "anims.fbx" fps 30 {
    srcstack "run"          // 官方会静默给你 walk；这里显式指定
}
```

对应的 TOML 形态（`[[bodyparts.models]]` 与 `[[sequences]]`）：

```toml
[[bodyparts]]
name = "body"
[[bodyparts.models]]
smd = "linnea.fbx"
src_parts = ["body"]
src_material = "face"
src_scale = 1.0

[[sequences]]
name = "run"
smd = "anims.fbx"
src_stack = "run"
```

### 4.8 明确不做

- **不做 DMX**（AGENTS.md 硬约束）。
- **不做 `fbx2smd` 中间转换子命令** —— 用户已明确否决，改为 QC 直引。
- **不用 `animsmith-fbx`**（§2.1：丢 morph）。若将来只需要网格 + 动画、不需要表情，
  它可以作为轻量选择；但 mdlc 的核心场景是角色，表情必需。
- **不自动按 `$sequence` 名字匹配栈名**。看着诱人，但它是**新的隐式规则**，
  且会静默偏离官方 ⟹ 与「不让用户猜」的目标相反。默认第一条 + 告警 + 显式覆盖。

---

## 5. 待办与风险

| # | 事项 | 说明 |
|---|---|---|
| 1 | **单位/缩放定案** | §1.7 的「骨骼 ×100、网格 ×1」需要一份**真实 Source 绑定 FBX** 复验；现有夹具是 Blender 默认导出（根节点带 m→cm 缩放）。⚠️ §1.7b 已证明**轴向**侧官方不做变换，单位侧大概率同理（都是「原样搬运根变换」），但仍需一份真实资产确认 |
| 2 | **骨骼过滤规则** | §1.4 的例外（`box.fbx` 的网格节点成了骨骼）需要更多样本确认边界（多根骨骼、多网格、嵌套骨架） |
| 3 | **flex 的 oracle 差分** | 用 `morph.fbx` 做「官方 FBX→flex」vs「mdlc FBX→flex」的逐字段对照；再复验「shape key 顺序 = 帧 1..N」这条映射 |
| 4 | **动画采样率** | 官方固定 30 fps（§1.5）；`ufbx` 需显式设 `minimum_sample_rate` 才能复刻（§2.3） |
| 5 | **`ufbx` 的依赖体积** | 纯 Rust 之外的 C 源码；需评估对 MSRV job（`rust-version = "1.89"`）与 CI 的影响 |
| 6 | **UV 翻转口径** | FBX 路径**不经过** SMD 解析 ⟹ 需在 FBX 读取侧补一次 `1.0 - v`（与 `src\smd.rs:527` 对齐，但**不能翻两次**） |
| 7 | **影响数 4 → 3** | `ufbx` 给最多 4 组权重，mdlc 上限 3（`src\model.rs:5265`）⟹ 重排 + 截断 + 重归一化 |
| 8 | **`startloop` 官方 bug** | §1.11 陷阱 3：官方在 FBX 路径上把 `startloop` 的 token 当成文件名。mdlc 应正常解析（并记入「有意偏离」） |

---

## 6. 实现落点（改动清单，供后续施工）

> 本节只为「将来实现时不用重新找一遍」而写。**当前未改任何代码。**

### 6.1 依赖

- `Cargo.toml` 加 `ufbx = "0.11"`（**不是** `animsmith-fbx`，理由见 §2.1）。
  许可 `MIT OR Unlicense`，与 `GPL-3.0-only` 兼容。
- ⚠️ 构建需要 C 编译器：实测本机 **VS 2026 Community** 可用、`cargo build` 49.8 s 通过。
  需在 CI 的 MSRV job（`rust-version = "1.89"`）上确认 `ufbx` 的 MSRV 不高于 1.89。

### 6.2 源加载层（`src\compile.rs`）

| 位置 | 现状 | 改动 |
|---|---|---|
| `:353 resolve_smd_path` | 只校验「有没有扩展名」 | **增加按扩展名分派**（`.smd` / `.fbx`），返回 `SourceKind` |
| `:370 fn read_smd` | `read_to_string` + `parse_smd` | **保留**；新增 `fn read_fbx` 走 `ufbx` |
| `:476`（`$lod` 调用点） | `read_smd(&smd_path, &lpath)` | 按 `SourceKind` 分派 |
| `:1052 fn load_smd_frames` | SMD 专用 | 新增 FBX 版本（`ufbx::bake_anim` + 30 fps 重采样） |
| `:398 fn build_model_lods` | 按材质名对齐各 LOD 的 mesh | FBX 侧要给出**同名材质**才能对齐 |

### 6.3 QC 前端（`src\qc\parse.rs`）

| 位置 | 现状 | 改动 |
|---|---|---|
| `:895 cmd_body` / `:911 cmd_bodygroup` | 调 `option_studio` | 不变 |
| `:969 option_studio` | 行内选项 `reverse` / `scale` / `faces` / `bias` | **新增块内选项** `srcpart` / `srcmaterial` / `srcscale` / `srcaxis`（`{ }` 分支现在只是 `UnGetToken` 后 `break`，需要真正进块） |
| `:1705 cmd_sequence` / `:1784 parse_sequence_body` | — | **新增** `srcstack` / `srcfps` |
| `:2180`（`$sequence` 块内「文件路径 vs blend 名」判据） | `t.text.ends_with(".smd") \|\| t.text.ends_with(".SMD")` | ⭐ **必须加 `.fbx`**，否则 FBX 会被当成 blend 名 |
| `cmd_animation`（`$animation` 分支） | — | 同样要认 `.fbx` |
| `cmd_model`（`$model` 块） | `flexfile` / `flex` / `eyelid` / `mouth` | **新增** `srcshapekey` / `srcshapekeyorder` / `srcshapekeyignore`；**对 FBX 源上的 `flexfile`/`flex` 报错** |

### 6.4 IR（`src\model.rs`）

| 结构 | 现状 | 改动 |
|---|---|---|
| `:3261 BodyModel.smd: String` | 源路径 | 字段名**保持不变**（它是 TOML 公开字段，改名是破坏性变更）；改为在 `resolve_smd_path` 侧按扩展名分派 |
| `:3246 BodyPart` | `name` / `base` / `models` | 新增 `src_*` 选项字段（全部 `Option`，`#[serde(skip_serializing_if = "Option::is_none")]`） |
| `Sequence`（`:1449`） | 已有很多字段 | 新增 `src_stack: Option<String>` / `src_fps: Option<f32>` |
| `FlexDescriptor`（`:1129`）/ `FlexController`（`:1141`）/ `FlexRule`（`:1295`） | **已够用** | ⭐ **不需要新 IR** —— FBX 的 shape key 正好合成这三样（§1.6b） |

### 6.5 需要复用的既有设施

- `src\flex.rs`：`resolve_flex_indexed`（VTA → vertanim 的解析与阈值）——
  FBX 的 shape key 偏移量要走**同一条**「匹配 + 阈值 + 归一化」路径，
  才能与官方产物逐字段一致（`MATCH_DIST_SQR = 0.15` / `MIN_DELTA_SQR` / `MIN_NDELTA_SQR`）。
- `src\bone_math.rs`：`compute_world` / `realign_bones` / `compute_pose_to_bone` —— FBX 的
  骨骼姿态要落到与 SMD 相同的世界空间口径。
- `src\compile.rs:821-846`：权重排序 + 截断到 `MAX_BONES_PER_VERT`（FBX 给 4 组）。
- `src\smd.rs:496-527` 的 UV 翻转 —— ⚠️ FBX 路径**不经过** SMD 解析，
  要在 FBX 读取侧补一次 `1.0 - v`（见待办 6）。

### 6.6 验收方式

沿用 AGENTS.md：**每个特性都要用 oracle 差分对照真实 `studiomdl.exe` 产物**。
FBX 侧已有 **82 个官方用例**的现成夹具与期望值（§1 各表），
新增的 `fbx*` 语法属于 mdlc 扩展，**无法对官方差分**，只能：
① 默认路径逐字段对官方差分；② 扩展语法用自己的单测 + 变异验证。

---

## 附：本次调研新建的文件

| 路径 | 作用 |
|---|---|
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\gen_fbx.py` | Blender 无头生成 5 个 FBX 样本（骨架 / 蒙皮 / 动画 / 一体 / Mixamo scale） |
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\gen_fbx_fps.py` | 生成 24 / 30 / 60 fps 三份动画 FBX |
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\gen_fbx_morph.py` | 生成带 2 个 shape key 的 `morph.fbx` |
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\gen_fbx_ux.py` | 生成材质 / 双栈 / 双网格样本（`mat.fbx` / `mat_slash.fbx` / `twostack.fbx` / `twomesh.fbx`） |
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\gen_fbx_order.py` | 生成**只有 NLA 栈顺序不同**的 `order_wr.fbx` / `order_rw.fbx`（§1.8 的决定性夹具） |
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\probe\` | `animsmith-fbx` 结构探针（证明它丢 morph） |
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\ufbxprobe\` | `ufbx` 直连探针（证明 morph 可读、采样率可控） |
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\ufbxraw\` | `ufbx` 原始变换读取（定位 ×100 来源）+ 动画栈/材质/网格枚举 |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx.js` | 官方 studiomdl 对 FBX 的 6 用例裁决矩阵 |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_morph_names.js` | 官方 shape key → flexdesc 名直读 |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_ux.js` | 12 用例 UX 矩阵（材质 / 多网格 / 混用 / `$animation` / `$collisionmodel`） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_stack.js` | 9 用例动画栈矩阵（证明不看名字） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_order.js` | 9 用例顺序对照（**证明恒取第一条**） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_footguns.js` | 8 用例陷阱矩阵（§1.11） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_morph_full.js` | 8 用例：**shape key 自动注册**（§1.6b 主表） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_flexmix.js` | 6 用例：显式 flex 语法的崩溃面 |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_flexfile_rule.js` | **2×2 单变量矩阵**：崩因是 `flexfile` 指向 `.fbx`（§1.6b 决定性） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_isolate.js` | 6 用例：FBX 自动注册与 `.vta` 显式 flex **可共存** |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_eyelid.js` | 5 用例：`eyelid` / `flexcontroller` 在 FBX 上安全 |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_samefile.js` | **6 用例单变量**：证明「静默 1 帧」的触发条件是**同一文件**（§1.11 陷阱 1） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_samefile_smd.js` | **5 用例**：证明该陷阱是 **FBX 专属**（SMD 同文件 8 帧源出 8 帧、5 帧源出 5 帧） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_samefile_fix.js` | **6 用例**：证明 `$animation` 走不通（N2/N4/N5/N6 全 1 帧），唯一出路是换文件（N3=6 帧） |
| `D:\GITHUB\mdlc\docs\_probe\oracle_fbx_axis.js` | 3 用例：证明官方**不做轴向变换**，原样搬运根变换（§1.7b） |
| `D:\DSH\L4D2ReverseEngineering\_fbxresearch\gen_fbx_axis.py` | 造出只有导出轴向不同的三份 FBX（§1.7b 的决定性夹具） |
| `D:\GITHUB\mdlc\docs\_probe\_tmp_dump_flexrules.js` | 只读 dump：`mstudioflexrule_t` 逐字段（§1.6b 的三件套） |

> ⚠️ Blender 无头启动**必须加 `--factory-startup`**：用户装的 `io_scene_valvesource` 与
> `comfyui_blender` 会在无头启动时抛 `AttributeError: 'Scene' object has no attribute 'vs'`
> 与 `PermissionError: [WinError 5]`，导致脚本中途中断。
