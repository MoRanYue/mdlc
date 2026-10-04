# mdlc

**Source 引擎模型编译器** —— Valve `studiomdl.exe` 的 Rust 独立重写。

把 **TOML 描述文件**或 **QC 脚本** + **SMD / FBX / glTF 网格**编译成 Source 引擎能加载的
`.mdl` + `.vvd` + `.dx90.vtx`（带碰撞时另有 `.phy`，用 `$animblocksize` 时另有 `.ani`）。

- **语言**：Rust **1.89+**（`edition 2024`）
- **平台**：无平台专有 API；提供 Windows / Linux / macOS（arm64 + x86_64）预编译产物
- **许可证**：[GPL-3.0-only](LICENSE)

```powershell
git clone https://github.com/MoRanYue/mdlc.git
cd mdlc
cargo build --release
```

> ⚠️ **法律提示**：本项目是**独立重写**（clean-room reimplementation），依据的是
> Source SDK 头文件、公开格式文档，以及对官方产物的**实测**。它**不包含**任何
> Valve 的二进制或美术资源。使用它编译的模型若要分发，请自行确认你拥有相应素材的
> 权利。Source 引擎与 `studiomdl` 是 Valve Corporation 的商标/作品，本项目与之无
> 隶属关系。

---

## 目录

- [快速开始](#快速开始)
- [产物](#产物)
- [两套输入格式，一个中间表示](#两套输入格式一个中间表示)
- [TOML 描述文件](#toml-描述文件)
- [QC 支持与 Crowbar 直接替换](#qc-支持与-crowbar-直接替换)
- [命令行参考](#命令行参考)
- [更新检测](#更新检测)
- [作为依赖使用](#作为依赖使用)
- [格式上限：只保留格式能表示的那些](#格式上限只保留格式能表示的那些)
- [顶点超限自动拆分](#顶点超限自动拆分)
- [多 LOD](#多-lod)
- [已知未实现](#已知未实现)
- [延伸文档](#延伸文档)

---

## 快速开始

```powershell
cargo build --release

# 打印一份带注释的完整 TOML 模板（含全部可用表与字段说明）
.\target\release\mdlc.exe template > myprop.toml

# 只校验，不写文件（退出码 0 = 合法）
.\target\release\mdlc.exe check myprop.toml

# 编译 → myprop.mdl / myprop.vvd / myprop.dx90.vtx
.\target\release\mdlc.exe build myprop.toml --out .\out

# 直接用 QC 编译（等价于 qc2toml 后再 build，中间描述不落盘）
.\target\release\mdlc.exe build-qc myprop.qc --out .\out
```

`mdlc template` 的输出是**权威的字段参考**，不会和实际支持的字段脱节。

> ⚠️ **三个容易踩的坑（模板里已写明，这里再强调一次）：**
>
> 1. **`mass` 不在 `[model]` 里，它在 `[physics]` 里。** 官方 studiomdl **没有**顶层
>    `$mass` —— 它只出现在 `$collisionmodel {}` / `$collisionjoints {}` 块里，
>    并且同一个值同时写进 `.phy` 的 `editparams.totalmass` 与 `.mdl` 头部的 `mass`。
>    **在 `[model]` 下写 `mass = 1.0` 会直接解析失败**（实测）。
> 2. **`contents` 的缺省是 `1`（`CONTENTS_SOLID`），不是 `0`。** 实测编译一个
>    不写 `contents` 的模型，产物头部 `+0x14C` 是 **1**。
> 3. **`[[bones]]` 的 `flags` 缺省不是无条件的 `0x500`**，而是按用途逐根计算后
>    沿父链传播的值（详见 [`[[bones]]`](#bones) 一节）。
>
> 另外模板**没有列出** `[physics]`、`[[animations]]`、`[[flex_descriptors]]`、
> `[[flex_controllers]]`、`[[flex_rules]]`、`[[flex_controller_ui]]`、`[[mouths]]`、
> `[[jiggle_bones]]`、`[[quat_interp_bones]]`、`[[bonecontrollers]]`、
> `include_models`、`skin_families`、`key_values`、`pose_parameters`、
> `flip_triangles`、`eyeballs`、`flexes`、`no_facial`、`section_frames` 等表与字段 ——
> 它们都**受支持**，语义见本文档其余小节。

> ⚠️ **未知的键名一律是硬错误**，包括未知的顶层表。所以字段名写错不会被静默忽略，
> 而是解析失败并列出全部合法字段名。

---

## 产物

| 扩展名 | 内容 | 何时产出 |
|---|---|---|
| `.mdl` | 头部、骨骼、材质、bodypart/model/mesh、动画链、序列、flex、IK、jigglebone… | 总是 |
| `.vvd` | 顶点池（位置/法线/UV/权重）+ 切线，多 LOD 时含 fixup 表 | 总是 |
| `.dx90.vtx` | 索引缓冲（strip group / strip / 顶点调色板） | 总是 |
| `.phy` | IVP 碰撞体（凸包 / `$concave` / `$collisionjoints`） | 有 `[physics]` 或 `$collisionmodel` |
| `.ani` | 外置动画块 | 写了 `[model].anim_block_size` 或 `$animblocksize`，**且该动画 ≥ 2 帧** |

> ⚠️ **`.ani` 只对齐一个 `studiomdl` 构建。** mdlc 复刻的是 `Left 4 Dead 2\bin\studiomdl.exe`
> （2024-06-04 构建）。别处流传的 `.ani` 有些来自**更早的构建、载荷格式不同**
> （头部 `+0` 恒为 28，那些是 56/84/88/92），拿它们当基准只会得到「全错」的假象。
> 容器层（416 字节头 / `IDAG` / version 49）各构建一致。

> ⚠️ **单帧动画不进 `.ani`** —— 判据是 `anim_block_size > 0 && numframes >= 2`，
> 单帧动画留在 `.mdl` 内联。`.ani` 的文件名**强制**是 `models/<模型名>.ani`，
> 因为 `.mdl` 头部 `+0x15C` 存的就是这个路径，写错名字引擎就找不到。

产物根目录 = `--out`（默认当前目录），再叠加模型名里的相对路径。所以
`name = "models/mymod/myprop.mdl"` 会写到 `<out>\models\mymod\myprop.mdl`。

四个文件共享同一个 **`checksum`** 配对令牌。它**不是内容哈希** —— 引擎与 Crowbar
都只比较、从不计算它。mdlc 缺省用模型名的 FNV-1a 生成（稳定、跨进程一致），
也可以用 `[model].checksum` 显式指定。

编译成功后 stdout 会打印一份摘要（骨骼数、材质数、顶点数、三角形数与各文件字节数）。

---

## 两套输入格式，一个中间表示

```text
TOML 描述 ──┐
            ├──► 中间表示 ──► 编译 ──► 写出器 ──► .mdl/.vvd/.vtx/.phy/.ani
QC 脚本  ──┘
```

两套输入**共用同一个中间表示**，所以写出器完全不关心
输入来自哪一边。QC 支持是后加的，**没有改动任何写出代码**。

网格**不写在描述里** —— 真实模型有几万到几十万个顶点（官方 `v_autoshotgun` 有
388,765 个），内联会让描述文件膨胀到几百 MB 且无法用文本工具处理。描述文件只
**引用**网格源。

网格源可以是 **SMD**、**FBX**（`.fbx`，走 `ufbx`，见
[`docs/fbx-support.md`](docs/fbx-support.md)）或 **glTF / GLB**（`.gltf` / `.glb`，走
`gltf` crate，见 [`docs/gltf-support.md`](docs/gltf-support.md)）。
**格式由扩展名决定，所以必须写全**
—— 这也是不自动补扩展名的原因之一（见 `## 已知未实现`）。

> **FBX / glTF 都可以直接写进 QC**，不需要中间转换。默认行为**逐条对齐官方**
> （合并所有网格、恒取第一条 NLA 栈、shape key 自动注册成 flex），
> 而官方那些**静默失败**会被 mdlc 变成显式提示，每个隐式决定都有一个
> 显式覆盖语法（九条 `src*`，见 [`### FBX / glTF 源选项`](#fbx--gltf-源选项src9-条)）。
>
> ⚠️ **glTF 没有官方基准**（官方 `studiomdl.exe` 完全不认这个格式），
> 它的口径是靠**传递式对照**定的 —— 同一个 Blender 场景双导出 `.fbx` + `.glb`，
> 官方编 `.fbx` 给基准，再证明「glTF 的数能推出 FBX 的数」。
> 每一条口径的来源都标在 `docs/gltf-support.md` §5。
>
> ```qc
> $modelname "models/mymod/linnea.mdl"
> $cdmaterials "models/mymod/"
> $body body "linnea.fbx" srcpart "body"   // 只取 body 网格
> // 表情不用写：shape key 自动注册
> ```

职责划分：

| 内容 | 由谁承载 | 对应 QC |
|---|---|---|
| 模型名 / 材质 / 骨骼 / bodypart 树 | TOML 描述文件 | `$modelname` / `$cdmaterials` / `$definebone` / `$bodygroup` |
| **网格（顶点、法线、UV、蒙皮）** | **SMD / FBX / glTF 文件** | `studio "x.smd"` / `$body body "x.fbx"` / `$body body "x.glb"` |
| **参考姿态** | **SMD 的 `skeleton` 第 0 帧**（FBX / glTF 用节点的局部 TRS） | 参考 SMD |
| **表情（flex）** | **`.vta` 的帧**，或 **FBX shape key / glTF morph target**（自动注册） | `flexfile` + `flex` |

---

## TOML 描述文件

> 下面只列**结构与要点**；每个字段的完整语义与默认值以
> `mdlc template` 的输出为准。

### `[model]`

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `name` | string | **必填** | 输出路径，相对游戏目录；反斜杠自动规范化为正斜杠 |
| `version` | int | `49` | 只支持 **44 / 48 / 49** |
| `checksum` | int | 按模型名生成 | 四件套配对令牌，不是内容哈希 |
| `static_prop` | bool | `false` | `$staticprop`，自动置头部 `0x10` |
| `surface_prop` | string | — | `$surfaceprop`；也是每根骨骼的缺省 `surface_prop` |
| `eye_position` / `illum_position` | `[f32; 3]` | — | `$eyeposition` / `$illumposition` |
| `max_eye_deflection` | f32 | `0` | `$maxeyedeflection`，**写角度**（落盘时转成 `cos`）。不写 = 0，引擎回退到 `cos(30°)` |
| `hull_min` / `hull_max` | `[f32; 3]` | 由 SMD 顶点算 | 包围盒 |
| `extra_flags` | int | `0` | 额外的 `STUDIOHDR_FLAGS_*` 位 |
| `contents` | int | **`1`**（`CONTENTS_SOLID`） | `$contents`。官方 `s_nDefaultContents = CONTENTS_SOLID`，**不是 0** |
| `skip_bone_in_bbox` | bool | `false` | `$skipboneinbbox` |
| `optimize_vtx` | bool | **`false`** | 顶点缓存优化（`meshopt`）；只重排索引，不改几何。QC：`$optimizevtx`（**mdlc 扩展**） |
| `split_oversized_meshes` | bool | **`true`** | 顶点超限自动拆分，见[下文](#顶点超限自动拆分)。QC：`$nosplitoversizedmeshes` / `$splitoversizedmeshes`（**mdlc 扩展**） |
| `key_values` | string | — | `$keyvalues` 内容（不含外层 `mdlkeyvalue` 包装） |
| `pose_parameters` | table array | `[]` | `$poseparameter` |
| `realign_bones` | bool | `false` | `$realignbones` |
| `anim_block_size` | int | — | `$animblocksize`，触发 `.ani` 外置动画块 |
| `section_frames` | `[int, int]` | — | 顶层 `$sectionframes <每段帧数> <阈值>` 的**全局**值（不是逐序列字段） |

### `[materials]`

```toml
[materials]
search_paths = ["models/mymod"]              # $cdmaterials
skin_families = [[0, 1, 2], [3, 4]]          # 可选，$texturegroup
textures = [ { name = "models/mymod/myprop" } ]
```

SMD 里的材质名按**首次出现顺序**映射到 `textures`；名字带 `search_paths`
前缀时落盘会自动剥掉（与 studiomdl 一致）。

### `[[bones]]`

```toml
[[bones]]
name = "root"                # 根骨骼必须排在最前；parent 只能指向更靠前的骨骼
# position = [0.0, 0.0, 0.0] # 留空则取自 SMD skeleton 第 0 帧
# rotation = [0.0, 0.0, 0.0] # 角度；留空同上
# flags = 1280               # 留空则按用途逐根计算（见下）
# surface_prop = "metal"     # 缺省继承 [model].surface_prop
bonemerge = true             # $bonemerge：允许该骨骼被合并
```

`flags` 留空时会按**用途**逐根计算（被顶点使用 / 被 hitbox 使用 / 被 attachment
使用 / 被 ikchain 使用 / bonemerge）并沿父链向上传播，与官方逐位一致。
`0x500`（`DEFAULT_BONE_FLAGS`）只是「该骨骼确实被顶点与 hitbox 使用」时的结果，
**不是**一个无条件缺省值 —— 一律写 `0x500` 会把未被顶点使用的骨骼误标为已使用。

`[[bones]]` 还接受三个与 `$definebone` 的 `realign` 有关的字段：

- `pre_aligned`（bool，可省略）—— 该骨骼是否被 `$realignbones` 跳过。留空时按
  「是否写了显式 `position`/`rotation`」**推断**（官方唯一能写参考姿态的命令
  `$definebone` 总是置该标志）；
- `realign_position` / `realign_rotation` —— `$definebone` **后 6 个数字**的
  `srcRealign`（只有写满 12 个数字时才有；6 数字形式下它是单位阵）。
  `realign_rotation` 是**角度**，顺序与 `rotation` 一致。⚠️ QC 的
  `$definebone` 参数顺序是 `<x> <y> <z> <pitch> <yaw> <roll>`，所以
  `pitch = rot[1]`、`yaw = rot[2]`、`roll = rot[0]`。

### `[[bodyparts]]` / `[[bodyparts.models]]` / `.lods`

```toml
[[bodyparts]]
name = "body"
base = 1                     # bodygroup 预设权重基数（可省略，缺省 1）
models = []                  # 必填键；至少一个

[[bodyparts.models]]
smd = "myprop-ref.smd"       # LOD 0（相对描述文件所在目录）；必填
flip_triangles = true        # 缺省 true（Source 正面是 CW）

[[bodyparts.models.lods]]
smd = "myprop-lod1.smd"
switch_point = 20.0          # 可省略，缺省 20·2^k
```

`[[bodyparts.models]]` 还接受 `eyeballs`（`$eyeball`/`$eyelid`）与 `flexes`
（`$flex`/`$flexpair`，指向 `.vta`），以及 `name`（缺省取 SMD 文件名）。

### `[hitboxes]` / `[[attachments]]`

```toml
[hitboxes]
set_name = "default"         # 可省略
[[hitboxes.boxes]]
bone = "root"
bbmin = [-8.0, -8.0, -8.0]
bbmax = [ 8.0,  8.0,  8.0]
# group = 0                  # 相交分组
# name  = "body"

[[attachments]]
name = "muzzle"              # $attachment
bone = "tip"
position = [0.0, 0.0, 8.0]
rotation = [0.0, 0.0, 0.0]   # 角度
```

`[[attachments]]` 还接受四个字段：

- `absolute`（bool，缺省 `false`）—— 官方 `IS_ABSOLUTE`。写出的 `local` 描述的是
  **世界坐标**下的绝对位姿，与骨骼姿态无关；⚠️ 该位**不落盘**（官方只写 `flags`）；
- `absolute_rotation`（bool，可省略）—— `absolute` 是否**覆盖**了 `local` 的旋转。
  官方选项循环是「最后写入者胜」：`absolute` 写 `AngleIMatrix(g_defaultrotation)`、
  `rotate` 写 `AngleMatrix(angles)`，两种情形的 `IS_ABSOLUTE` **都置位**，
  只有旋转来源不同。留空 ⟹ **跟随 `absolute`**（也是官方最常见的写法）；
- `rigid`（bool，缺省 `false`）—— 官方 `IS_RIGID`。同样**不落盘**，只影响骨骼保活
  （沿父链上溯到第一根被顶点引用的骨骼）；
- `flags`（int，可省略）—— 直接写进 `mstudioattachment_t.flags` 的位。官方这里
  **只**可能落 `world_align`（`0x10000`）；`absolute`/`rigid` 走的是另一个字段，
  不写进产物。

**没写显式 hitbox 时会自动生成**一个 `default` set（官方 `SetupHitBoxes` 语义），
并置 `autogenerated` 标志 —— 真实 L4D2 模型全部都有 hitbox，没有它子弹打不中。

### `[[sequences]]` 与 `[[animations]]`

```toml
# 单动画序列
[[sequences]]
name = "idle"
smd = "idle.smd"
fps = 30.0
looping = true
# delta = true                 # $sequence ... delta（同时置 DELTA|POST）
# activity = "ACT_VM_IDLE"
# activity_weight = 1
# weight_list = "upper"        # 按名字引用 [[weight_lists]]
# no_auto_ik = true            # 抑制自动补的 IK_RELEASE 规则

# blend 网格序列
[[sequences]]
name = "walk"
blends = ["a_run", "a_idle", "a_run", "a_idle"]   # 必须是完全平方数
blend_width = 2
[[sequences.blend_params]]
parameter = "move_x"
start = -1.0
end = 1.0
```

序列级可用字段（对应 QC 的 `$sequence` 块选项）：
`fps` / `looping` / `delta` / `activity` / `activity_weight` / `events` /
`fade_in` / `fade_out` / `forward_declared` / `no_auto_ik` / `ik_rules` /
`iklocks` / `blends` / `blend_width` / `blend_params` / `blend_ref` /
`blend_comp` / `blend_center` / `auto_layers` / `movements` / `section_frames` /
`section_threshold` / `extra_flags` / `weight_list` / `subtract` /
`subtract_frame` / `num_frames`。

`[[animations]]`（QC 的 `$animation`）声明**可被多条序列复用的动画**。
官方把 animdesc 放在全局池里，`$sequence` 只是引用它 —— 所以一个 `$animation`
只有**一份**数据，多条序列引用同一格时共享它。这带来一个容易写错的地方：
`weightlist` / `numframes` / `subtract` / `ikrule` 改的是**共享的动画对象**，
而不只是当前这条序列。

`forward_declared = true` 对应 `$declaresequence` —— **前向声明的空壳序列**
（survivor 模组的核心机制）：主模型里声明一堆空名字，真正的动画在
`$includemodel` 进来的 `anim_<survivor>.mdl` 里，引擎加载时按名字替换。
空壳**不要写 `smd`**（官方连 `panim` 都不分配），写了会被拒绝。

> ⚠️ **已知缺口：`weight_list` / `num_frames` / `subtract` 在 blend 序列上被静默丢弃。**
> 这三个选项目前**只实现在单动画序列上**：加在 blend 序列上不会报错，但**完全不生效**
> （实测产物与不加**逐字节相同**）。所以这是**静默的数据丢失**，不是解析错误 ——
> 要这三个选项生效，请把它们写在单动画序列上。

### `[[weight_lists]]`

```toml
[[weight_lists]]
name = "upper"
[[weight_lists.bones]]
bone = "mid"
weight = 0.5
pos_weight = 0.25            # 可选，只参与编译期 IK 误差计算，不落盘
```

权重表用于**增量动画（delta）的重建缩放**：`s = 0` 的骨骼完全不参与该序列的增量
叠加（保持基准姿态）。模组作者用它做「只让上半身动」。

> ⚠️ 语义**不是**「没列出的骨骼就是 1」。官方算法是：① 具名表的**根骨骼默认 0**；
> ② 显式条目覆盖；③ 沿父链**继承**。骨骼链 `root → mid → leaf → tip` 只写
> `mid 0.5` 会得到 `[0, 0.5, 0.5, 0.5]`。

### `[physics]`

```toml
[physics]
smd = "physics.smd"          # $collisionmodel 的 SMD
concave = true               # $concave：按连通分量拆成多个凸块
joints = false               # true = $collisionjoints（每骨骼一个 solid 的 ragdoll）
# mass / damping / rot_damping / inertia / drag / root_bone
# mass_center / auto_mass / no_self_collisions
# joint_overrides / constraints / animated_friction / collision_pairs / merge
```

`concave` 与 `joints` **互斥**：ragdoll 每根骨骼本来就是一个凸包，
`$concave` 只作用于单 solid 的 prop 路径。

> ⚠️ **`concave` 不是 VHACD 式的「体分解」。** 官方 `$concave` 是
> **连通分量分解**：顶点焊接（位置相同**且**法线夹角 < 2°）→ 按共享焊接顶点做
> 并查集 → 每个连通分量各算一个凸包。所以一个**连通的**凹体（U 形、圆环）
> 只会得到**一个**把凹口**填平**的凸包。mdlc 照此实现。
>
> 需要**保留凹口**的真凹形碰撞体时，用独立的 `mdlc phy --vhacd`（parry3d 的
> VHACD）—— 那是**非官方语义**，且**不能**从 TOML/QC 编译路径触发。

碰撞几何的世界空间转换用的是**第一条序列的第 0 帧**，不是碰撞 SMD 自己的姿态。
`[physics]` 的 `joint_overrides` / `constraints` /
`collision_pairs` 在 `joints = false` 时会**报错**（它们只对 ragdoll 有意义）；
`auto_mass = true` 也会报错 —— 它需要一张 mdlc 没有的表面材质密度表。

### 其余表

| 表 | 对应 QC | 说明 |
|---|---|---|
| `[[ikchains]]` | `$ikchain` | `name` / `bone`（**末端**骨骼）/ `knee_dir` |
| `[[ik_autoplay_locks]]` | `$ikautoplaylock` | `chain` / `pos_weight` / `local_q_weight` |
| `[[flex_descriptors]]` | `flex` / `eyelid` / `mouth` | `name` |
| `[[flex_controllers]]` | `flexcontroller` | `name` / **`type`** / `min` / `max` |
| `[[flex_rules]]` | `%<flex> = <expr>` | `flex` + `ops`（每项 `op` + `value`/`controller`/`flexdesc`） |
| `[[flex_controller_ui]]` | — | `name` / `stereo` / `left` / `right` |
| `[[mouths]]` | `mouth` | `index`（显式，决定 `g_nummouths`）/ `flexdesc` / `bone` / `forward` |
| `[[jiggle_bones]]` | `$jigglebone` | `bone` + `is_flexible` / `is_rigid` / `has_base_spring` + **`writes`** |
| `[[quat_interp_bones]]` | `$proceduralbones`（proctype 2） | `bone` / `control` / `base_pos` / `triggers` |
| `[[bonecontrollers]]` | `$controller` | L4D2 已废弃该特性，但段仍占位。⚠️ 键名是字面的 **`type_`**（带下划线） |
| `include_models` | `$includemodel` | 顶层字符串数组；**必须自己写 `models/` 前缀**，mdlc 不替你补 |

> ⚠️ **只有两个键名是特例**：`[[sequences.ik_rules]]` 与 `[[flex_controllers]]`
> 的类型键在 TOML 里写作 **`type`**（不是 `kind`）。其余键名一律是表里写的那些。
> 另有三个键的**取值**是固定选项，写错会解析失败：`[[flex_rules]]` 的 `op`
> （21 个取值，见下）、`ik_rules` 的 `type`、以及 `pose_parameters` 的 `wrap`。

> ⚠️ **`[[flex_rules]]` 的 `op` 取值**共 21 个：`const` `fetch1` `fetch2` `add`
> `sub` `mul` `div` `neg` `exp` `open` `close` `comma` `max` `min` `2way_0`
> `2way_1` `nway` `combo` `dominate` `dme_lower_eyelid` `dme_upper_eyelid`。

> ⚠️ **`include_models` 里指不存在的文件也能编译** —— 官方**只写名字**，
> 从不读被包含的 `.mdl`（合并骨骼/序列是**引擎**运行时做的）。

> **`[[ikchains]]` 会自动补 IK 规则**：只要模型有 IK 链，官方会给每个「没有任何
> 显式 ikrule 的链」追加一条 `type = 4`（`IK_RELEASE`）的规则。所以绝大多数情况下
> **不需要**手写 `[[sequences.ik_rules]]`。要抑制它就在该序列上写 `no_auto_ik = true`。

### TOML 的一个陷阱

**顶层键必须写在所有 `[[表]]` 之前。** 例如把 `bonemerge = [...]` 写在
`[[attachments]]` 之后，TOML 会把它解析成 `attachments` 的字段而报错。
本实现因此把 `bonemerge` 放在 `[[bones]]` 里（`bonemerge = true`），
而不是设一个顶层数组。

---

## QC 支持与 Crowbar 直接替换

### 官方兼容形态

```powershell
.\target\release\mdlc.exe -game "<gamedir>" [-nop4] [-verbose] myprop.qc
```

产物写到 `<gamedir>\models\<$modelname>` —— 与官方 `studiomdl` 的规则一致。

**为什么需要它**：[Crowbar](https://github.com/ZeqMacaw/Crowbar) 把编译器路径当
**不透明配置项**，只传 `-game "<gamedir>" <选项> "<qc 文件名>"` 并把 CWD 设为 QC
所在目录。它的成败判定只有两条 —— ① 编译器有输出；②
`<gamedir>\models\<$modelname>.mdl` 存在。**它不看退出码，也不解析错误文本。**
所以把 Crowbar 的「编译器路径」指向 `mdlc.exe` 即可直接替换。

兼容层处理官方那些**单横线长选项**（clap 只认 `--long`，故需归一化）。
已知**未实现**的官方选项（`-minlod`、`-striplods`、
`-definebones`、`-printbones`、`-t`、`-a`）会**警告并忽略**，不会静默改变产物
（其中 `-minlod` / `-t` / `-a` 在官方那边要带值，省略值仍是用法错误）。

> ⚠️ **诊断流的走向是按调用形态决定的**：官方 `studiomdl` 把 `ERROR:` 写在
> **stdout**，而 Crowbar 的「编译器是否活着」标志只在 stdout 处理器里置位。
> 所以兼容形态下 mdlc 的诊断也走 **stdout**（与官方一致）；mdlc 自有子命令仍走
> stderr。判据是**调用形态**而非父进程名 —— 零依赖、跨平台。

### QC 命令覆盖

QC 前端已实现完整的词法/语法分析、`$include`、`$definevariable`、
`$pushd`/`$popd`、`$cd` 目录栈，以及下面这些命令：

`$modelname` `$cd` `$pushd` `$popd` `$cdmaterials` `$surfaceprop`
`$contents` `$eyeposition` `$illumposition` `$maxeyedeflection` `$bbox` `$cbox`
`$staticprop` `$realignbones` `$skipboneinbbox` `$animblocksize` `$keyvalues`
`$sectionframes` `$poseparameter` `$texturegroup` `$body` `$bodygroup` `$model`
`$sequence` `$animation` `$definebone` `$bonemerge` `$attachment` `$hboxset`
`$hbox` `$ikchain` `$ikautoplaylock` `$includemodel` `$lod` `$shadowlod`
`$jigglebone` `$proceduralbones` `$collisionmodel` `$collisionjoints`
`$jointsurfaceprop` `$weightlist` `$declaresequence`
`$jointconstrain` `$animatedfriction` `$noselfcollisions` `$jointcollide`
`$jointmerge` `$unlockdefinebones`，以及一批头部 flag（`$opaque` `$mostlyopaque`
`$noforcedfade` `$casttextureshadows` `$ambientboost` `$donotcastshadows`
`$forcephonemecrossfade` `$constantdirectionallight`）。

**已知但故意不支持的**：

| 命令 | 行为 |
|---|---|
| `$nekomodel` | **显式报错** —— 它指向 DMX 源，mdlc 不实现 DMX（官方也委托 `dmxconvert.exe`） |
| `$defaultweightlist` | **显式报错** —— 它会覆盖**所有**未显式指定 weightlist 的序列，静默忽略会产出「看起来对但语义错」的模型。要等价效果请用 `$weightlist` 并在序列上显式引用 |
| `$fakevta` | 跳过整个块（无产物痕迹） |
| `$scale` | **接受但无效果**（缩放只被记录、从不参与计算）。请改用 `srcscale` |
| `$cbox` / `$maxconvexpieces` / `$phyname`、`$ikchain` 的 `height`/`pad`/`floor`/`center`、`$attachment … x_and_z_axes`、ikrule 的 `usesequence` | 参数被消费但**不落盘**（或由其它字段等价表达） |
| 一批罕见的命令（`$minlod` `$maxverts` `$renamebone` `$hierarchy` `$collapsebones` `$screenalign` `$upaxis` `$origin` `$maxbones` `$controller` …） | **忽略本行**（不报错、不留痕迹） |
| 其它未知命令 | 报错（对应官方的 `bad command`） |

> ⚠️ **`$sequence` 块里出现未知关键字时，它会被当成「动画名」。** 例如
> `$sequence "x" "a.smd" nodefaults` 里的 `nodefaults` 会进 `blends`，
> 然后在查动画池时报 `找不到动画 "nodefaults"`。**不是静默忽略，但报错信息会误导。**

> ⚠️ **`$maxverts` 被忽略**（不是实现，也不报错）。它是第三方 NekoMDL 的**非官方
> 扩展**，会把超限模型按三角形切成多个 **bodypart** —— 那会改变 `$bodygroup` 的
> 按下标选择语义。mdlc 用[同 model 内多 mesh](#顶点超限自动拆分)的等价且更安全的
> 方式解决同一个问题。

> ⚠️ **`$sequence` 块里可以写「动画选项」。** `subtract` / `numframes` /
> `weightlist` / `ikrule` / `addlayer` / `blendlayer` / `calcblend` 这些
> **在 `$sequence` 里同样合法**，而且它们改的是**被引用的那个共享动画对象**
> —— 也就是说，在序列里写一次会影响所有引用同一动画的地方。

### mdlc 扩展的 QC 命令

下面 3 条**不是官方命令**，是 mdlc 自己的扩展。命名规则与 TOML 字段一一对应
（字段名去掉下划线、前面加 `$`）：

| 命令 | 等价 TOML | 说明 |
|---|---|---|
| `$optimizevtx` | `optimize_vtx = true` | 打开顶点缓存优化，等价于命令行 `--optimize-vtx` |
| `$nosplitoversizedmeshes` | `split_oversized_meshes = false` | 关掉[顶点超限自动拆分](#顶点超限自动拆分)，遇到超限 mesh 直接报错 |
| `$splitoversizedmeshes` | `split_oversized_meshes = true` | 把上面那条**开回来**（缺省本来就是 `true`；它存在的意义是 `$include` 的 `.qci` 关掉后主 QC 还能改回来） |

三条都是**裸标志位**（不带参数，只消费自己那一个 token），且**命令名大小写不敏感**
（与官方取词器一致）。QC 自上而下解释，**后写的赢**。

```qc
$modelname "models/mymod/myprop.mdl"
$body body "myprop.smd"
$optimizevtx              ; 打开顶点缓存优化
$nosplitoversizedmeshes   ; 关掉超限自动拆分（遇到超限 mesh 就报错）
```

> ⚠️ **为什么是 mdlc 扩展而不是官方命令名。** 官方 `studiomdl.exe` 的 QC
> 分发表里**没有**这两个功能的任何关键字（`optimize` / `vcache` / `nvtristrip` /
> `split` / `oversized` 全部 0 命中）——官方把顶点缓存优化做成**命令行**开关
> `-nvtristrip`，超限网格则**直接拒绝**（`ERROR: too many indices in source`）。
> 第三方 NekoMDL 也没有这两个 QC 命令（它的 `$maxverts` 做的是另一件事：
> 把超限模型拆成**新 bodypart**，mdlc 故意不学，理由见
> [顶点超限自动拆分](#顶点超限自动拆分)）。
>
> 所以真 `studiomdl.exe` 对这三条会报 `bad command`。**写了它们的 QC 不能
> 直接拿去跑官方工具**；要跨工具通用请改用 TOML 侧的对应字段。

> ⚠️ **`$optimizevtx` 没有反向命令。** `optimize_vtx` 缺省就是 `false`，
> 一条「关掉」的命令没有实际用途（官方的裸标志位如 `$staticprop` 也都没有
> 反向命令）。同理命令行 `--optimize-vtx` 也只能开、不能关。

### FBX / glTF 源选项（`src*`，9 条）

网格源是 `.fbx` 或 `.gltf` / `.glb` 时，导入器会替你做一串**隐式决定**，而且
**全部静默**：合并所有网格、恒取第一条 NLA 栈、材质名直接用文件里的、
轴向原样搬运。mdlc 的默认行为**逐条对齐官方**，
但把这些决定变成**显式语法**——不写就等于默认行为，写了就能改。

⭐ **九条语法在 glTF 上逐条都适用**，因为它们是**按概念命名**的
（格式由文件扩展名决定）：所以叫 `srcpart` 而不是 `fbxpart`。
加 glTF 支持时**新增语法 0 条**。

> ⚠️ **唯一的例外是单位缩放**：官方在 FBX 路径上只认节点上的 `LclS`、完全忽略
> `UnitScaleFactor`，于是 Blender 默认导出（`Apply Scalings` = "All Local"，
> ×100 烘在节点上）的模型会**骨骼比网格大 100 倍**，而且**网格位置留在厘米、
> 网格尺寸在米**。mdlc **有意修掉它**（顶点与骨骼都不进缩放）：两种导出方式
> 编出**逐值相同**的产物，你不必关心 Blender 的那个选项。细节见
> [`docs/fbx-support.md`](docs/fbx-support.md)。
>
> ⚠️ **glTF 侧不存在这个问题** —— Blender 的 glTF 导出器不写 `LclS`，
> 所以 `.glb` 从一开始就是自洽的。两套输入编出同一量级的模型。

| 命令 | 写在哪 | 作用 | 不写时（= 官方） |
|---|---|---|---|
| `srcpart "名"` | `$body` / `$model` 行内或块内 | 只取这些名字的网格；**可重复写** | 全部网格合并进同一个部件 |
| `srcmaterial "名"` | 同上 | 文件里没有材质时的兜底名 | `debug/debugempty` |
| `srcscale 1.0` | 同上 | 统一缩放（顶点与骨骼同乘，**比值不变**） | 1.0 |
| `srcaxis "z"` | 同上 | 强制上轴（`y` / `z`） | 不干预（原样搬运根变换） |
| `srcstack "名"` | `$sequence` / `$animation` 块内 | 用哪条动画栈 | **第一条**（glTF 按 `animation.name()` 选） |
| `srcfps 30` | 同上 | 动画重采样率 | 30 |
| `srcshapekey "名"` | `$model` 块内 | 只取这些 shape key / morph target，**并定序**；可重复写 | 全部，按文件顺序 |
| `srcshapekeyorder "名"` | 同上 | 只定序，不筛 | 文件顺序 |
| `srcshapekeyignore` | 同上 | 全部忽略（不注册 flex） | 全部注册 |

```qc
$modelname "models/mymod/linnea.mdl"
$cdmaterials "models/mymod/"

// 一个 FBX 里有 body / hair / eyes 三块网格，只要 body
$body body "linnea.fbx" srcpart "body" srcmaterial "face"

// glTF 一样写（格式由扩展名决定，语法不用换）
$body body "linnea.glb" srcpart "body"

// 多栈 FBX：官方恒取第一条，这里点名要 run
$sequence run "anim.fbx" srcstack "run" srcfps 30

// 表情：什么都不用写，shape key / morph target 自动注册；要控制就这样写
$model "face" "linnea_face.fbx" {
    srcshapekey "smile"
    srcshapekey "blink"
}
```

⭐ **表情（flex）的默认路径不需要任何新语法。** FBX 的 shape key 与
glTF 的 morph target 都会被**自动注册**成 flexdesc + flexcontroller + flexrule + 载荷，
与官方逐字段一致。
`srcshapekey*` 三条只在你想**筛掉或重排**时才需要。

> ⚠️ **`srcpart` / `srcshapekey` / `srcshapekeyorder` 每次只读一个 token**
> （所以可以重复写）。**不要**写成 `srcpart "body" "hair"`——第二个名字会被
> 当成网格名而不是「另一个选项」，后面的选项会被吞掉。网格名带空格用引号解决：
> `srcpart "my mesh"`。

> ⚠️ **一处默认行为是「报错」而不是新语法**（所以**没写新语法的 QC 也跑不了官方**）：
> 在 FBX 源上写 `flexfile "<某.fbx>"` + `flex`——官方**必崩**，mdlc 报错。
> 只在**确实会崩**时才触发。
> 完整清单见 [`docs/fbx-support.md`](docs/fbx-support.md) §4.6 的偏离表。

> ⭐ **同一个 `.fbx` 既作网格源又作动画源：mdlc 正常编译 + 一条提示。**
> 官方在这个组合下**静默只出 1 帧**（实测 exit=0、无警告）——但 FBX 本来就是
> 网格与动画合一的容器，所以 mdlc **不复制这个退化行为**，照常采出全部帧，
> 只提示「与官方对照时帧数不同是预期的；想让两边一致就把动画拆到独立 FBX」。
> 提示只在 mdlc 真采出 > 1 帧时才发（静态 FBX 两边都是 1 帧，不提示）。

> ⚠️ **写了 `src*` 的 QC 不能直接跑官方工具**（官方会报 `bad command`），
> 与 `$optimizevtx` 那三条同理。要跨工具通用请改用 TOML 侧字段
> （`src_parts` / `src_material` / `src_scale` / `src_axis` / `src_stack` /
> `src_fps` / `src_shape_keys` / `src_shape_key_order` / `src_shape_key_ignore`）。

### FBX / glTF 的诊断

官方在这些情形下**静默通过**（`exit=0`），但结果通常不是你要的。
mdlc 会打一行 `提示：`：

| 情形 | 提示内容 |
|---|---|
| 多块网格被合并进同一个部件 | 列出网格名 + 「要分开请用 `srcpart` 或拆成多个 `$body`」 |
| 网格没有材质 | 「已合成 `debug/debugempty`；要改用别的写 `srcmaterial`」 |
| 有多条动画栈而没写 `srcstack` | 列出全部栈名 + 「默认只用第一条」 |
| 有 shape key / morph target（已自动注册成 flex） | 列出名字与帧号 + 「要控制请用 `srcshapekey*`」 |

四条全部是**提示**而非错误，且**对 `.smd` 工程零影响**（连一行输出都不多）。
另有一条同类提示（同一个 FBX 既作网格源又作动画源）见上面的说明块。

glTF 侧另有三条自己的提示（见 `docs/gltf-support.md` §6.3）：

| 情形 | 类型 | 说明 |
|---|---|---|
| accessor 没有 `bufferView` | 提示 | 按规范当**全零**处理（Blender 对「法线偏移全零」的 morph target 正是这么写的） |
| 有 `CubicSpline` 插值的通道 | 提示 | 按**线性**降级重采样（切线值被丢掉） |
| 有通道驱动 `MorphTargetWeights` | 提示 | **忽略** —— 表情只由 QC 的 `flex` 语句控制 |
| 用了 Draco / meshopt 压缩扩展 | **错误** | mdlc 不解压（`KHR_draco_mesh_compression` / `EXT_meshopt_compression`） |
| `data:` URI 解码失败 | **错误** | 不能静默退化成零几何 |

---

## 命令行参考

### mdlc 自有形态

```text
mdlc build <model.toml> [--out <目录>] [--optimize-vtx]
mdlc check <model.toml>
mdlc build-qc <model.qc> [--out <目录>] [--optimize-vtx]
mdlc qc2toml <model.qc> [--out <path.toml>]
mdlc phy <in.smd> <out.phy> [--checksum N] [--mass F] [--surfaceprop S]
                          [--concave] [--vhacd] [--decompose] [--ragdoll]
mdlc vvd-info <file.vvd>
mdlc vvd-roundtrip <file.vvd>
mdlc template
mdlc --version | -V
```

**上面这段与下面那张表都只是速查** —— 权威用法是 `mdlc --help` /
`mdlc <子命令> --help`。

| 子命令 | 作用 |
|---|---|
| `build` | TOML 描述 → `.mdl`/`.vvd`/`.vtx`（主线） |
| `check` | 只校验（会读网格源），不写文件。退出码 0=合法，1=描述或编译有错，2=读不到文件或 TOML 语法错 |
| `build-qc` | 直接从 QC 编译，中间描述不落盘 |
| `qc2toml` | QC → TOML（**只写文本，不编译**）。用于迁移或人工核对解析结果 |
| `phy` | 从 SMD 三角形算凸包并写出 `.phy` |
| `vvd-info` | 解析并打印 VVD 头部、统计与自洽性检查结果 |
| `vvd-roundtrip` | VVD 读入再写出并逐字节比对（用于确认写出器与格式完全一致） |
| `template` | 打印带注释的完整 TOML 模板 |

`--version` / `-V` 打印 `mdlc <版本>`。⚠️ **官方兼容形态不认它**（官方
`studiomdl` 没有这个选项），而且两种写法后果不同：

- `-V` 会被当成**未知的单横线选项丢弃**并打一行警告，编译照常进行（退出码 0）；
- `--version` 是**硬错误**（`unexpected argument`，退出码 2）—— 官方形态
  刻意关掉了版本标志，clap 于是不认它。

Crowbar 两个都不会传，所以不影响直接替换。

### 帮助与错误的多语言

帮助与错误信息**按系统显示语言自动切换**（未命中时回落到英文）。
所以中文系统上 `mdlc -h` 打的是：

```text
用法: mdlc.exe <命令>

命令:
  build          TOML 描述 → .mdl/.vvd/.vtx（MVP 主线）
  check          只校验描述文件，不写文件
  phy            SMD 三角形 → 凸包 → .phy 碰撞文件
  ...
  help           打印本信息或给定子命令的帮助

选项:
  -h, --help     打印帮助信息（使用 '-h' 查看摘要）
  -V, --version  打印版本信息
```

（`mdlc --help` 是**长帮助**：`-h` / `-V` 会展开成两行并带上完整说明；
子命令的帮助里 `参数:` / `选项:` 段落标题同样是中文。）

> 颜色**只在直连终端时**才出现，被重定向或管道抓取时自动降级成纯文本 ——
> 所以不会往 Crowbar 之类的宿主日志里塞 ANSI 转义码。

> ⚠️ **官方兼容形态不受影响**：它的帮助/错误仍是英文，
> 与官方 `studiomdl` 的输出形态一致。

`--optimize-vtx` 用 `meshopt` 对每个 strip group 重排索引以提升 GPU 后变换顶点
缓存命中率。它**只改索引顺序**，顶点池与三角形集合都不变，所以渲染结果相同。
默认**关闭**，以保持与既有产物逐字节相同。

它与 `[model].optimize_vtx` 是 `||` 关系（任一为真即生效），所以**只能开、不能关**；
QC 侧等价命令是 `$optimizevtx`（见 [mdlc 扩展的 QC 命令](#mdlc-扩展的-qc-命令)）。

---

## 更新检测

`mdlc` 每次运行都会在后台检查有没有新版本 —— **不阻塞编译**：网络 I/O 发生在
派生出去的子进程里，主进程只读一个约 100 字节的缓存文件。

| 项 | 值 |
|---|---|
| 间隔 | 24 小时；期内**不派生任何进程** |
| 提示时机 | **下一次运行**（第 1 次派生子进程去查，第 2 次读缓存打印） |
| 关闭 | 环境变量 `MDLC_NO_UPDATE_CHECK=1` |
| CI | 检测到 `CI` 环境变量时自动跳过 |
| 排错 | `MDLC_UPDATE_DEBUG=1` 把失败原因打到 stderr |
| 缓存 | Windows `%LOCALAPPDATA%\mdlc\update-check.json`；其余 `$XDG_CACHE_HOME/mdlc/` 或 `~/.cache/mdlc/` |

提示跟着既有规则走：**官方兼容形态落 stdout**（与官方 `studiomdl` 一致，
Crowbar 只认 stdout），mdlc 自有形态落 stderr。

---

## 作为依赖使用

`mdlc` 同时是一个库。要给第三方 GUI / 构建系统内置编译器时，**不要自己串一遍
管线** —— 用 `mdlc::pipeline`：

```rust
use std::path::Path;
use mdlc::model::ModelDesc;
use mdlc::pipeline::{self, PipelineOptions};

let text = std::fs::read_to_string("model.toml")?;
let desc = ModelDesc::from_toml(&text)?;

// 只编译，不碰文件系统：拿到四件套字节 + 编译期中间结果。
let out = pipeline::build(&desc, Path::new("."), PipelineOptions::default())?;

// 或者直接落盘（按 `$modelname` 建目录）。
let paths = pipeline::write_files(&out, Path::new("out"))?;
println!("{}", paths.mdl.display());
```

`pipeline::build` 只读文件、不写文件；`write_files` 只写文件。**分两步是为了让
GUI 能在写盘前显示摘要或让用户确认。**

### 为什么必须用它

`compile()` 只做「描述 → 编译期中间结果」，它**完全不管碰撞 SMD**，也不写任何文件。
自己串管线最容易漏掉的是「碰撞 SMD → `physicsbone`」这一步：

```rust
// 这一步漏了，`.mdl`/`.vvd`/`.dx90.vtx` 全对，只有 physicsbone 悄悄全 0。
if let Some(cs) = &collision_smd {
    let parents = mdlc::compile::bone_parents(&compiled.desc);
    compiled.physics_bone = mdlc::phy::physics_bone_table(cs, compiled.desc.bones.len(), &parents);
}
```

### 错误与退出码

`PipelineError` 分四类，`kind()` 映射到与 `mdlc.exe` 相同的退出码：

| 变体 | `kind()` | 退出码 | 触发 |
|---|---|---|---|
| `Compile(Vec<CompileError>)` | `Build` | 1 | 描述/QC 校验失败、读不到源 SMD |
| `Collision(String)` | `Build` | 1 | 碰撞 SMD 解析失败 |
| `Write(String)` | `Build` | 1 | 写出或自检失败（本实现的 bug） |
| `Io(String)` | `Io` | 2 | 路径解析、读文件、建目录、写文件失败 |

`lines()` 给出与 CLI 逐行相同的文案（编译失败是多行，其余是单行）；`Display`
就是它们拼起来的。GUI 直接用 `lines()` 渲染日志即可，不必自己拼字符串。

### 摘要

`PipelineOutput::summary_lines(&paths)` 复刻 `mdlc.exe` 成功时打印的那张表
（模型 / 版本 / checksum / 统计 / 每个产物的字节数与路径 / `**编译成功**`）。
CLI 自己就是逐行 `println!` 它 —— 所以 GUI 显示的与命令行看到的**逐字相同**。

---

## 格式上限：只保留格式能表示的那些

`studiomdl` 里有一批**人为**上限（例如单 model 65536 顶点、材质 32 个），它们不是
文件格式的约束。本实现**不复刻**这些限制，只保留**位宽 / 偏移算术**推出的硬上限：

| 维度 | 上限 | 依据 |
|---|---|---|
| 被**引用**的骨骼**下标** | **127** | VVD `mstudioboneweight_t.bone[]` 与 VTX `Vertex_t.boneID[]` 都是**有符号** `char` |
| 骨骼**总数** | 无上限 | `mstudiobone_t` 数组长度是 `int32`（`MAXSTUDIOBONES = 128` 只是引擎上限，不作拒绝条件） |
| 动画链能**寻址**的骨骼数 | **256** | `mstudioanim_t.bone` 是 `byte`（判据是 `≤255`） |
| 每 **strip** 的骨骼调色板 | **127** | VTX `Vertex_t.boneID[]` 是 `char`（官方 `maxBonesPerStrip = 53`） |
| 每 **mesh** 顶点 | **65536** | VTX `Vertex_t.origMeshVertID` 是 `uint16` |
| 每 **model** 顶点 | **44,739,242** | `vertexindex` 是 int32 字节偏移 ÷ 48 |
| **材质**表条数 | **32768** | `pSkinref[]` 是**有符号** `short` |
| **三角形**数 | 无上限 | VTX `numIndices` 是 int32 |
| **LOD** 档数 | **8** | 引擎 `MAX_NUM_LODS` |

写出后还会各自跑一次**自检**（VVD / VTX 的自洽性检查、PHY 的 13 条硬约束），
失败会以「本实现的 bug」中止而不是留下坏文件。

> ⚠️ **约束的是「被引用的骨骼下标」，不是「骨骼总数」。**
> **没被引用的骨骼不占 VVD 下标空间** —— 它们只出现在骨骼表里。
> 所以一个 134 根骨骼、只引用到下标 118 的模型是**合法**的。

> ⚠️ **「每 mesh 顶点 ≤ 65536」的粒度是 mesh（= 一个材质）**，不是 bodypart：
> 加一个材质就多一个 mesh，各自独立计数。判据是 `n > 65536`（**不是 `>=`**）——
> 恰好 65536 个顶点时下标是 `0..65535`，全部装得下。

---

## 顶点超限自动拆分

**默认不用管** —— mdlc 会自动拆。VTX 的 `origMeshVertID` 是 `uint16`，
所以**一个 mesh（= 一个材质）最多 65536 个顶点**。官方 `studiomdl` 会直接拒绝
（`ERROR: too many indices in source`）；mdlc 在编译期把超限的 mesh
**按三角形顺序切成多个 mesh**：

| | |
|---|---|
| 开关 | `[model].split_oversized_meshes`，**默认 `true`** |
| 拆到什么粒度 | 每块 ≤ 65536 顶点 |
| 放在哪里 | **同一个 model** 里（**不新增 bodypart**） |
| 材质 | 所有块**共用原材质下标**（`mesh.material` 只是 `pSkinref[]` 的下标） |
| 渲染结果 | 与拆分前**逐像素相同**（只是多几个 draw call） |
| 关掉它 | `= false` ⟹ 回到「报错并给出替代路径」 |

**为什么不拆成新 bodypart**（NekoMDL 的 `$maxverts` 是那样做的）：bodypart 数量
一变，引擎的 `$bodygroup` 选择（**按下标**）就会错位。

> **一个真实例子**：某改模工程的单个材质有 305,703 顶点 —— 不拆分时编译**失败**；
> 拆分后 mesh 从 20 个变成 24 个、**最大单 mesh 恰好 65,536**、三角形总数守恒
> 232,099，bodypart 数**仍是 2**（未变）。

对不超限的模型这条路径**完全不碰**（提前返回），所以对既有产物零影响。

---

## 多 LOD

```toml
[[bodyparts.models]]
smd = "lod0.smd"             # LOD 0（最精细）

[[bodyparts.models.lods]]
smd = "lod1.smd"
switch_point = 20.0          # 可省略，缺省 20·2^k（LOD 1→20、LOD 2→40、LOD 3→80）

[[bodyparts.models.lods]]
smd = "lod2.smd"
switch_point = 40.0
```

每个 LOD 是一个**完整独立的 SMD**（不是「删掉一些三角形」）。本实现把它们跨 LOD
精确去重合并成一个顶点池，按 LOD 归属排序，并生成 fixup 表 —— 与 studiomdl 的
`UnifyLODs` 一致。

三条约束：

1. 各 LOD 的**材质集合必须一致** —— 违反会**显式报错**（缺材质与多材质都报，
   静默对齐会贴错材质）；
2. 最多 **8** 层（含 LOD 0）—— 违反会报错；
3. 每个 LOD 的顶点数**应当**单调不增（LOD 越高越粗）—— 这是**建议**而不是硬检查；
   `numLODVertexes[n]` 由各块长度按累计口径算出，所以写出的值始终自洽。

> **未实现**：LOD 的**自动生成**（网格简化 / decimate）。本实现只接受显式给出的
> 多 LOD 输入，**不会替你简化网格**。这是与官方 `studiomdl` 差距最大的一块。

`[[bodyparts.models.lods]]` 还接受 `bone_tree_collapse` / `replace_bone` /
`no_facial`（对应 `$lod` 的这些选项）。`smd` 可省略 —— 省略时复用 LOD 0 的网格，
只应用骨骼选项。

---

## 已知未实现

| 项 | 说明 |
|---|---|
| **LOD 自动生成** | 网格简化 / decimate。多 LOD 的**输入与写出**已支持，但不会替你简化。**这是与官方差距最大的一块** |
| **DMX 输入** | 刻意不实现。`$nekomodel` 会**显式报错**；`studio "x.dmx"` 不会被专门识别，而是在读文件/解析 SMD 时失败（官方也是委托 `dmxconvert.exe`） |
| `$maxverts` | NekoMDL 的非官方扩展，被忽略。用自动拆分替代 |
| `ikrule footstep`（type 3） | 需要 `$ikchain` 的 `center`，mdlc 尚未建模该量。写出时会**显式报错**而不是静默产出错误载荷 |
| 若干罕见的 QC 命令 | `$renamebone` / `$hierarchy` / `$insertbone` / `$collapsebones` / `$screenalign` / `$upaxis` / `$origin` / `$maxbones` … 被忽略 |
| 官方 CLI 的 `-minlod` / `-striplods` / `-definebones` / `-printbones` / `-t` / `-a` | 参数被接受但**警告并忽略** |
| `$bodygroup { … blank }` | `blank` 成员（一个空 model）**报错** —— `BodyModel.smd` 是必填字段，mdlc 没有表达「无网格 model」的方式 |
| **blend 序列上的 `weightlist` / `numframes` / `subtract`** | **被静默丢弃**（见上）。单动画序列上它们正常生效 |
| `[[sequences.movements]]` 只能从 TOML 写 | QC 前端没有对应关键字（写出是完整实现的，只是 QC 侧无法表达） |
| DX8 / DX7 回退变体 | 官方额外产出 `.dx80.vtx` / `.sw.vtx`；L4D2 是 DX9 引擎，目前只产 `.dx90.vtx` |

---

## 延伸文档

仓库内的 `docs/` 收录了八份规格报告，结论全部来自对真实产物的逐字节分析：

| 文档 | 内容 |
|---|---|
| [`docs/animation-layout.md`](docs/animation-layout.md) | MDL 动画数据布局的规格（含已推翻结论的勘误） |
| [`docs/blend-sequences.md`](docs/blend-sequences.md) | blend 序列的完整规格（QC 侧 + 二进制侧） |
| [`docs/coordinate-systems.md`](docs/coordinate-systems.md) | 坐标系约定与 `$staticprop` 几何旋转 |
| [`docs/qc-coverage-gap.md`](docs/qc-coverage-gap.md) | 以 L4D2 `studiomdl.exe` 的 **137 条分发表**为基准的 QC 覆盖对照 |
| [`docs/feature-gap.md`](docs/feature-gap.md) | 相对官方 `studiomdl` 的特性差距清单与优先级 |
| [`docs/fbx-support.md`](docs/fbx-support.md) | FBX 支持的口径与 UX 方案（九条 `src*` 语法的设计理由 + 偏离表） |
| [`docs/gltf-support.md`](docs/gltf-support.md) | glTF / GLB 支持的口径来源与取舍（新增语法 0 条） |
| [`docs/update-check.md`](docs/update-check.md) | 更新检测的设计与取舍（为什么默认开启、什么时候联网、怎么关掉） |

> ⚠️ **`docs/feature-gap.md` 与 `docs/qc-coverage-gap.md` 是调研报告**，
> 带有快照日期，**描述的是撰写当时的状态**；「当前实现了什么」以本文件为准。
