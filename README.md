# mdlc

**Source 引擎模型编译器** —— Valve `studiomdl.exe` 的 Rust 独立重写。

把 **TOML 描述文件**或 **QC 脚本** + **SMD 网格**编译成 Source 引擎能加载的
`.mdl` + `.vvd` + `.dx90.vtx`（带碰撞时另有 `.phy`，用 `$animblocksize` 时另有 `.ani`）。

- **语言**：Rust **1.89+**（源码用了 `let`-chains，1.88 才稳定；`edition 2024`）
- **平台**：无平台专有 API；CI 在 Windows 与 Linux 上同时构建与测试
- **许可证**：[GPL-3.0-only](LICENSE)

```powershell
git clone https://github.com/MoRanYue/mdlc.git
cd mdlc
cargo build --release
cargo test --release        # 618 passed / 0 failed / 6 ignored（不需要任何外部素材）
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
- [两套输入格式，一个 IR](#两套输入格式一个-ir)
- [TOML 描述文件](#toml-描述文件)
- [QC 支持与 Crowbar 直接替换](#qc-支持与-crowbar-直接替换)
- [命令行参考](#命令行参考)
- [格式上限：只保留格式能表示的那些](#格式上限只保留格式能表示的那些)
- [顶点超限自动拆分](#顶点超限自动拆分)
- [多 LOD](#多-lod)
- [验证方法：差分对照，不是自证](#验证方法差分对照不是自证)
- [测试](#测试)
- [代码结构](#代码结构)
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

`mdlc template` 的输出是**权威的字段参考** —— 它由 `src/model.rs` 的
`TEMPLATE_TOML` 常量维护，并且有一个测试断言它能被 `ModelDesc::from_toml` 解析，
所以它不会和代码脱节。

> ⚠️ **但模板里的注释有两处是过期的，别照着取消注释：**
>
> 1. **`mass` 不在 `[model]` 里，它在 `[physics]` 里。** 模板顶部仍列着
>    `# [model].mass <- $mass` 与 `# mass = 1.0`。官方 studiomdl **没有**顶层
>    `$mass` —— 它只出现在 `$collisionmodel {}` / `$collisionjoints {}` 块里，
>    并且同一个值同时写进 `.phy` 的 `editparams.totalmass` 与 `.mdl` 头部的 `mass`。
>    由于所有 TOML 结构体都是 `deny_unknown_fields`，**在 `[model]` 下写
>    `mass = 1.0` 会直接解析失败**（实测）。
> 2. **`contents` 的缺省是 `1`（`CONTENTS_SOLID`），不是 `0`。** 模板注释写的是
>    `默认 0`。实测编译一个不写 `contents` 的模型，产物头部 `+0x14C` 是 **1**。
>
> 另外模板**没有列出** `[physics]`、`[[animations]]`、`[[flex_descriptors]]`、
> `[[flex_controllers]]`、`[[flex_rules]]`、`[[flex_controller_ui]]`、`[[mouths]]`、
> `[[jiggle_bones]]`、`[[quat_interp_bones]]`、`[[bonecontrollers]]`、
> `include_models`、`skin_families`、`key_values`、`pose_parameters`、
> `flip_triangles`、`eyeballs`、`flexes`、`no_facial` 等表与字段 ——
> 它们都在代码里实现了，语义见本文档其余小节与 `src/model.rs` 的文档注释。

> ⚠️ **所有 TOML 表都是 `deny_unknown_fields`：未知键一律是硬错误**，包括未知的
> 顶层表。所以字段名写错不会被静默忽略，而是解析失败并列出全部合法字段名。

---

## 产物

| 扩展名 | 内容 | 何时产出 |
|---|---|---|
| `.mdl` | 头部、骨骼、材质、bodypart/model/mesh、动画链、序列、flex、IK、jigglebone… | 总是 |
| `.vvd` | 顶点池（位置/法线/UV/权重）+ 切线，多 LOD 时含 fixup 表 | 总是 |
| `.dx90.vtx` | 索引缓冲（strip group / strip / 顶点调色板） | 总是 |
| `.phy` | IVP 碰撞体（凸包 / `$concave` / `$collisionjoints`） | 有 `[physics]` 或 `$collisionmodel` |
| `.ani` | 外置动画块 | 写了 `[model].anim_block_size` 或 `$animblocksize`，**且该动画 ≥ 2 帧** |

> ⚠️ **`.ani` 只对一个 `studiomdl` 构建负责，不能拿语料 `.ani` 当 oracle。**
> mdlc 复刻的是本机 `Left 4 Dead 2\bin\studiomdl.exe`（2024-06-04 构建），
> 而 `mdl-corpus` 里那 121 个 `.ani` 来自**更早的构建、载荷格式不同**
> （头部 `+0` 恒为 28 vs 语料的 56/84/88/92；`+0 == 28` 在语料里只有 **10/2969**
> 命中）。容器层（416 字节头 / `IDAG` / version 49）两个构建一致（121/121）。
> 所以这条路径的验收只能用**受控实验**（真实 `studiomdl.exe` 编译 `parity/ab_*.smd`
> 的产物），拿语料做差分只会得到「全错」的假象。

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

## 两套输入格式，一个 IR

```text
TOML 描述 ──┐
            ├──► ModelDesc（IR）──► compile() ──► 写出器 ──► .mdl/.vvd/.vtx/.phy/.ani
QC 脚本  ──┘
```

两套输入**共用同一个 IR**（`src/model.rs` 的 `ModelDesc`），所以写出器完全不关心
输入来自哪一边。QC 支持是后加的，**没有改动任何写出代码**。

网格**不写在描述里** —— 真实模型有几万到几十万个顶点（官方 `v_autoshotgun` 有
388,765 个），内联会让描述文件膨胀到几百 MB 且无法用文本工具处理。描述文件只
**引用** SMD。

职责划分：

| 内容 | 由谁承载 | 对应 QC |
|---|---|---|
| 模型名 / 材质 / 骨骼 / bodypart 树 | TOML 描述文件 | `$modelname` / `$cdmaterials` / `$definebone` / `$bodygroup` |
| **网格（顶点、法线、UV、蒙皮）** | **SMD 文件** | `studio "x.smd"` |
| **参考姿态** | **SMD 的 `skeleton` 第 0 帧** | 参考 SMD |

---

## TOML 描述文件

> 下面只列**结构与要点**；每个字段的完整语义、默认值、实测依据都在
> `mdlc template` 的输出与 `src/model.rs` 的文档注释里。

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
| `optimize_vtx` | bool | **`false`** | 顶点缓存优化（`meshopt`）；只重排索引，不改几何 |
| `split_oversized_meshes` | bool | **`true`** | 顶点超限自动拆分，见[下文](#顶点超限自动拆分) |
| `key_values` | string | — | `$keyvalues` 内容（不含外层 `mdlkeyvalue` 包装） |
| `pose_parameters` | table array | `[]` | `$poseparameter` |
| `realign_bones` | bool | `false` | `$realignbones` |
| `anim_block_size` | int | — | `$animblocksize`，触发 `.ani` 外置动画块 |

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

**没写显式 hitbox 时会自动生成**一个 `default` set（官方 `SetupHitBoxes` 语义），
并置 `autogenerated` 标志 —— 实测真实 L4D2 模型 100% 都有 hitbox，没有它子弹打不中。

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
空壳**不要写 `smd`**（官方连 `panim` 都不分配），写了会被 `validate()` 拒。

> ⚠️ **已知缺口：`weight_list` / `num_frames` / `subtract` 在 blend 序列上被静默丢弃。**
>
> `compile.rs` 对 `$sequence` 有**两条互斥路径**：`if !s.blends.is_empty()`
> （blend 网格）与后面的单动画路径。这三个选项目前**只实现在单动画路径上**。
>
> **实测**：给 `parity/blend1.toml` 的 blend 序列加一张全 0 权重表，
> 产物与不加**逐字节相同**（3880 字节，0 处差异）；同样一张表加在单动画序列上
> 则产物从 3880 变成 3872 字节（生效）。所以这是**静默的数据丢失**，
> 不是解析错误。
>
> 同一族的历史 bug（`ikrule`、`addlayer`）已经**两条路径都修好**了，
> 这三个是遗留项。**加新选项时必须同时问「两条路径都处理了吗」。**

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

碰撞几何的世界空间转换用的是**第一条序列的第 0 帧**，不是碰撞 SMD 自己的姿态
（受控实验结论）。`[physics]` 的 `joint_overrides` / `constraints` /
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
| `[[jiggle_bones]]` | `$jigglebone` | `bone` + `is_flexible` / `is_rigid` / `has_base_spring` |
| `[[quat_interp_bones]]` | `$proceduralbones`（proctype 2） | `bone` / `control` / `base_pos` / `triggers` |
| `[[bonecontrollers]]` | `$controller` | L4D2 已废弃该特性（语料 0 次），但段仍占位。⚠️ 键名是字面的 **`type_`**（带下划线） |
| `include_models` | `$includemodel` | 顶层字符串数组；**必须自己写 `models/` 前缀**，mdlc 不替你补 |

> ⚠️ **两个 TOML 键名与 Rust 字段名不同**：`[[sequences.ik_rules]]` 与
> `[[flex_controllers]]` 的类型键在 TOML 里写作 **`type`**（Rust 侧字段叫 `kind`）。
> 只有这两处做了 serde rename，其余字段名与 Rust 一致（snake_case）。

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

兼容层细节（官方是**单横线长选项**，而 clap 只认 `--long`，故需归一化）见
`src/cli.rs` 的模块文档。已知**未实现**的官方选项（`-minlod`、`-striplods`、
`-definebones`、`-t`、`-a`）会**警告并忽略**，不会静默改变产物。

> ⚠️ **诊断流的走向是按调用形态决定的**：官方 `studiomdl` 把 `ERROR:` 写在
> **stdout**，而 Crowbar 的「编译器是否活着」标志只在 stdout 处理器里置位。
> 所以兼容形态下 mdlc 的诊断也走 **stdout**（与官方一致）；mdlc 自有子命令仍走
> stderr。判据是**调用形态**而非父进程名 —— 零依赖、跨平台。

### QC 命令覆盖

QC 前端（`src/qc/`）已实现完整的词法/语法分析、`$include`、`$definevariable`、
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
| `$defaultweightlist` | **显式报错** —— 它会覆盖**所有**未显式指定 weightlist 的序列，静默忽略会产出「看起来对但语义错」的模型。mdlc 的隐式表 0 是常量，没有可覆盖的存储。语料出现 **0 次**；要等价效果请用 `$weightlist` 并在序列上显式引用 |
| `$fakevta` | 跳过整个块（无产物痕迹） |
| `$scale` | **接受但无效果**（`default_scale` 只被写入、从未被读取）。语料 0 次 |
| `$cbox` / `$maxconvexpieces` / `$phyname`、`$ikchain` 的 `height`/`pad`/`floor`/`center`、`$attachment … x_and_z_axes`、ikrule 的 `usesequence` | 参数被消费但**不落盘**（语料 0 次，或由其它字段等价表达） |
| 一批语料 0 次的命令（`$minlod` `$maxverts` `$renamebone` `$hierarchy` `$collapsebones` `$screenalign` `$upaxis` `$origin` `$maxbones` `$controller` …） | **忽略本行**（不报错、不留痕迹） |
| 其它未知命令 | 报错（对应官方的 `bad command`） |

> ⚠️ **`$sequence` 块里出现未知关键字时，它会被当成「动画名」。** 例如
> `$sequence "x" "a.smd" nodefaults` 里的 `nodefaults` 会进 `blends`，
> 然后在查动画池时报 `找不到动画 "nodefaults"`。**不是静默忽略，但报错信息会误导。**

> ⚠️ **`$maxverts` 被忽略**（不是实现，也不报错）。它是第三方 NekoMDL 的**非官方
> 扩展**，会把超限模型按三角形切成多个 **bodypart** —— 那会改变 `$bodygroup` 的
> 按下标选择语义。mdlc 用[同 model 内多 mesh](#顶点超限自动拆分)的等价且更安全的
> 方式解决同一个问题。

> ⚠️ **`$sequence` 块里可以写「动画选项」。** 官方 `ParseSequence` 在
> `numblends || isAppend` 时把 token 交给 `ParseAnimationToken`，后者会走到
> `ParseCmdlistToken`。所以 `subtract` / `numframes` / `weightlist` / `ikrule` /
> `addlayer` / `blendlayer` / `calcblend` 这些**在 `$sequence` 里同样合法**，
> 而且它们改的是**被引用的那个共享动画对象**。
> 判据是「它由官方哪个函数处理」，不是「它写在哪个块里」。

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
```

| 子命令 | 作用 |
|---|---|
| `build` | TOML 描述 → `.mdl`/`.vvd`/`.vtx`（主线） |
| `check` | 只校验描述文件，不写文件。退出码 0=合法，1=有错误 |
| `build-qc` | 直接从 QC 编译，中间描述不落盘 |
| `qc2toml` | QC → TOML（**只写文本，不编译**）。用于迁移或人工核对解析结果 |
| `phy` | 从 SMD 三角形算凸包并写出 `.phy` |
| `vvd-info` | 解析并打印 VVD 头部、统计与自洽性检查结果 |
| `vvd-roundtrip` | VVD 读入再写出并逐字节比对（布局判据） |
| `template` | 打印带注释的完整 TOML 模板 |

`--optimize-vtx` 用 `meshopt` 对每个 strip group 重排索引以提升 GPU 后变换顶点
缓存命中率。它**只改索引顺序**，顶点池与三角形集合都不变（写出前有守门断言校验
这两条），所以渲染结果相同。默认**关闭**，以保持与既有产物逐字节相同。

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

写出后还会各自跑一次**自检**（VVD 的 `check_invariants`、VTX 的 `check_invariants`、
PHY 的 13 条硬约束），失败会以「本实现的 bug」中止而不是留下坏文件。

> ⚠️ **约束的是「被引用的骨骼下标」，不是「骨骼总数」。**
> 早期版本误把「总数 ≤ 128」当格式约束，结果**打死真实模型**
> （`linnea-export` 有 134 根骨骼，但只引用到下标 118）。
> **没被引用的骨骼不占 VVD 下标空间** —— 它们只出现在骨骼表里。

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

> **实测**（某改模工程，单个材质 305,703 顶点）：拆分前 mesh 20 个、最大单 mesh
> 305,703 顶点 ⟹ 编译**失败**；拆分后 mesh 24 个、**最大单 mesh 恰好 65,536**、
> 0 个超限，**三角形总数守恒 232,099**，bodypart 数**仍是 2**（未变）。

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

## 验证方法：差分对照，不是自证

**每项特性的验收都是与真实 `studiomdl.exe` 的产物逐字段对照**，而不是「单元测试
全绿」。这不是洁癖 —— 本项目的绝大多数 bug 都属于「**写错不会报错，只会在游戏里
表现异常**」的类型：

- 字段偏移错位 → 能解析，但引擎读到别的块；
- 旋转矩阵转置 → 顶点被反向旋转（对 `rotation = 0` 的合成模型完全看不出来）；
- UV 不翻转 V → 法线贴图凹凸方向整体反了；
- 三角形绕序反了 → 面法向整体朝内；
- 动画常量载荷存成差值 → 骨骼整体少转一个参考旋转；
- VTX 调色板写错 → 顶点蒙皮到错误骨骼，模型撕裂。

这些**全部**只能靠与真实产物对照才能发现。

### 判据分三层

| 层 | 手段 | 例子 |
|---|---|---|
| **布局判据** | 官方文件读入再写出必须**逐字节相同** | 语料 **3302/3302** 个 VVD 往返一致（含 53 个 fixup、230 个多 LOD） |
| **编译判据** | 同一几何分别用两个编译器编译，产物**逐字段对照** | `verify_parity.ps1` + oracle 工具 |
| **回归判据** | 改动前后对 101 个夹具出 SHA256 快照，逐文件比对 | `parity_snapshot.js` —— **101/101** |

配套的 oracle 工具在 `D:\GITHUB\mdlc-oracle`（独立仓库），用**独立解析器**读两边的
产物再逐字段比：

```powershell
cd D:\GITHUB\mdlc-oracle
cargo build --release
.\target\release\oracle.exe diff <studiomdl产物>.mdl <mdlc产物>.mdl
```

### 回归三连

改任何东西之后都跑：

```powershell
cd D:\GITHUB\mdlc
cargo test --release                  # 应为 618 passed / 0 failed / 6 ignored
cargo clippy --release --all-targets  # 应为 0 warning
node docs\_probe\parity_snapshot.js   # 应为 101/101
```

> ⚠️ **`cargo clippy -- -D warnings` 会被缓存骗过**：cargo 的 clippy 结果按
> **源码内容**缓存，**不区分 `-D warnings`**。先跑过一次不带它的 clippy，之后带它
> 的那次会**复用缓存直接返回成功**，于是 allow-by-default 的 lint 全部漏掉 ——
> 症状是「本地 clippy 干净，CI 却红」。CI 里已改成显式
> `-W clippy::all -D warnings`，且 CI 总是全新构建。

> ⚠️ **纯性能改动必须做「基线二进制 A/B」**：只比快照不够 —— 快照证明「产物没变」，
> 但不证明「你没把别的路径改慢」。

### 变异测试

新增或修改差分测试后，会把被测代码**故意改坏**，确认测试变红。本项目已被
「测试全绿但改坏了也全绿」坑过多次，所以这是**必做收尾**。

> ⚠️ **一个恒真的测试比没有测试更危险。** 边界测试的夹具必须能**精确落在边界值
> 上**（用「每三角形 3 个独立顶点」的夹具永远够不到 65536，把 `>` 变异成 `>=`
> 照样全绿）；判据里「差异计数等于 0」的形式天然会被**空夹具**满足，所以必须在
> 判定前加「夹具非空」硬门。

---

## 测试

```powershell
cargo test --release          # 618 passed / 0 failed / 6 ignored
```

**618 个测试默认全跑，不需要任何外部素材。** 按模块分布：

| 模块 | 数量 | 覆盖 |
|---|---|---|
| `compile` | 105 | 描述 + SMD → IR、跨文件一致性校验、拆分、LOD 统一 |
| `qc` | 78 | QC 词法 / 语法 / 命令语义 |
| `anim_writer` | 78 | 动画链编码、量化、RLE、IK 误差、段表、外置块 |
| `phy` | 77 | 凸包、`$concave`、ragdoll、IVP 布局不变量 |
| `mdl_writer` | 77 | 各结构体偏移与大小的硬编码断言、字符串池、段顺序 |
| `lod` | 26 | 顶点池排序、分段铺满、fixup 分组 |
| `vtx_writer` | 24 | strip group 链、骨骼调色板、缓存优化守门 |
| `flex` | 24 | 就近匹配、差量、smoothstep、载荷 |
| `model` | 23 | TOML 解析与校验（含各类非法输入） |
| `smd` | 18 | SMD 解析（含 9/10 token 顶点行） |
| `bone_math` | 17 | 欧拉/四元数、矩阵约定 |
| `ani_writer` | 16 | `.ani` 容器与块对齐 |
| `cli` | 16 | 官方选项归一化、Crowbar 调用形态 |
| `vta` | 14 | `.vta` 解析 |
| `lib.rs` 的 `tests` | 10 | VVD 往返判据、fixup 铺满、`numLODVertexes` 单调性与 ripple |
| `tangent` | 9 | 切线算法（轴对齐 / 手性 / 退化 UV） |
| `layout` | 6 | 段偏移计算与单调性 |

### 6 个需要真实素材的测试（默认 `#[ignore]`）

另有 6 个测试拿**真实 `studiomdl.exe` 产物**做判据。这些素材**不能进仓库**
（体量大、含 Valve 版权内容），所以它们标了 `#[ignore]`：

```powershell
cargo test --release -- --ignored
```

素材用**环境变量**指定（不设则回退到开发机的历史路径）：

| 环境变量 | 素材 |
|---|---|
| `MDLC_TEST_VVD` | 官方 `v_autoshotgun.vvd`（388,765 顶点 / 24.8 MB） |
| `MDLC_TEST_MDL` | 官方 `v_autoshotgun.mdl`（89 骨骼） |
| `MDLC_TEST_SMD` | 真实反编译 SMD（22,911 三角形） |
| `MDLC_TEST_VTX` | 官方 `myprop.dx90.vtx`（须由 `studiomdl` 编译 `parity/myprop.qc` 得到） |
| `MDLC_TEST_CORPUS` | 真实语料根目录（3333 个 `.mdl` / 3302 个 `.vvd`） |

```powershell
$env:MDLC_TEST_CORPUS = 'D:\somewhere\mdl-corpus'
cargo test --release -- --ignored
```

> ⚠️ **为什么用 `#[ignore]` 而不是「读不到就 `return`」。**
> 后者在 `cargo test` 的默认输出里**显示成 `ok`**，看起来像「验过了」，实际什么
> 也没验。改成 `#[ignore]` 后，「跳过」与「通过」在结果行里**不再混淆**：显式跑
> 它们时素材缺失会**失败**并打印该设哪个环境变量，而不是静默变绿。

### 仓库里没有的东西

以下内容**存在于开发机但不在仓库里**（`.gitignore` 逐条写明了理由：体量大、
含 L4D2 解包素材）：

| 路径 | 内容 |
|---|---|
| `docs/_probe/` | 判据脚本（1100+ 个 `.js`）、受控实验夹具、官方与 mdlc 的对照产物 |
| `parity/` | 101 个 `.toml` + 64 个 `.smd` + 11 个 `.qc` 输入，以及 `_snap/` 快照 |
| `out/`、`mymod/` | mdlc 自己的编译输出 |
| `verify_parity.ps1`、`cmp_features.ps1` | 两个回归脚本（依赖上面两项） |

因此本 README 与源码注释里对它们的引用，在**只克隆本仓库时是悬空的** ——
它们描述的是**验证过程**，不是运行本程序的依赖。重建方式见 `.gitignore` 各条目。

> ⚠️ **`git clean -xdf` 会把它们真的删掉** —— 探针脚本与夹具目前只有这一份，
> 而 `parity/` 与 `docs/_probe/` **不在版本控制里**，删了不可回滚。
> 源码（`src/`、`docs/*.md`、`Cargo.toml`）则是受 git 保护的。

---

## 代码结构

```text
src/
  main.rs         命令行入口与编排（编译 → 写出 → 摘要）
  cli.rs          clap 定义 + 官方 studiomdl 单横线选项的归一化兼容层
  diag.rs         诊断输出的流路由（兼容形态走 stdout）
  lib.rs          库根：模块声明与 VVD 往返判据
  model.rs        IR（ModelDesc）与全部校验规则；含 TEMPLATE_TOML
  smd.rs          SMD 网格 / 骨架解析
  compile.rs      TOML|QC + SMD → 编译期 IR（跨文件校验、拆分、LOD 统一）
  qc/             QC 前端
    lexer.rs        词法（含官方 TokenAvailable 的行内语义）
    parse.rs        命令分发表 → ModelDesc
    flexrule.rs     %<flex> = <expr> 表达式编译
  mdl_writer.rs   .mdl 写出（头部、骨骼、材质、bodypart、mesh、flex、IK…）
  vvd.rs          .vvd 解析 / 写出 / 往返比对 / 自洽性检查
  vtx_writer.rs   .dx90.vtx 写出（strip group、骨骼调色板、缓存优化）
  anim_writer.rs  动画链写出（量化、RLE、常量载荷、IK 误差、段表、外置块）
  ani_writer.rs   .ani 容器写出
  lod.rs          多 LOD 与 fixup 表（顶点池排序、分段、重映射）
  flex.rs         VTA 形状解析（就近匹配 → 差量 → smoothstep → 载荷）
  vta.rs          .vta 顶点动画解析
  phy.rs          .phy 碰撞体写出（凸包 / $concave / ragdoll / 阻尼 / 惯量）
  tangent.rs      切线空间计算（法线贴图用）
  bone_math.rs    骨骼矩阵与四元数工具
  layout.rs       MDL 段偏移的声明式计算
  prof.rs         分段计时探针（profiling feature，默认关闭）
  test_assets.rs  真实素材测试的路径解析（仅测试）
```

**段布局是一个框架，不是一个长函数。** MDL 的段偏移集中在 `src/layout.rs` 里
声明式地计算（`SectionOffsets::compute` + `SectionCounts`），`write_mdl` 只负责
「填计数」和「按算出的偏移写字节」。顺序在 `compute` 里硬编码为权威顺序，写错会被
`check_monotonic()` 抓到。加一个新段只需三步：加计数字段、在 `compute` 里把 `+ 0`
换成 `+ n * SIZE`、在 `write_mdl` 里按偏移写字节。

**数学用现成 crate**：`parry3d`（quickhull 凸包、VHACD、质量属性）、`meshopt`
（顶点缓存优化）、`clap`（CLI）、`toml` + `serde`（描述文件）。

---

## 已知未实现

| 项 | 说明 |
|---|---|
| **LOD 自动生成** | 网格简化 / decimate。多 LOD 的**输入与写出**已支持，但不会替你简化。**这是与官方差距最大的一块** |
| **DMX 输入** | 刻意不实现。`$nekomodel` 会**显式报错**；`studio "x.dmx"` 不会被专门识别，而是在读文件/解析 SMD 时失败（官方也是委托 `dmxconvert.exe`） |
| `$maxverts` | NekoMDL 的非官方扩展，被忽略。用自动拆分替代 |
| `ikrule footstep`（type 3） | 需要 `$ikchain` 的 `center`，mdlc 尚未建模该量。**语料出现 0 次**；写出时会**显式报错**而不是静默产出错误载荷 |
| 若干语料 0 次的 QC 命令 | `$renamebone` / `$hierarchy` / `$insertbone` / `$collapsebones` / `$screenalign` / `$upaxis` / `$origin` / `$maxbones` … 被忽略 |
| 官方 CLI 的 `-minlod` / `-striplods` / `-definebones` / `-t` / `-a` | 参数被接受但**警告并忽略** |
| `$bodygroup { … blank }` | `blank` 成员（一个空 model）**报错** —— `BodyModel.smd` 是必填字段，mdlc 没有表达「无网格 model」的方式 |
| **blend 序列上的 `weightlist` / `numframes` / `subtract`** | **被静默丢弃**（见下）。单动画序列上它们正常生效 |
| `[[sequences.movements]]` 只能从 TOML 写 | QC 前端没有对应关键字（`mstudiomovement_t` 的**写出**是完整实现的，只是 QC 侧无法表达） |
| DX8 / DX7 回退变体 | 官方额外产出 `.dx80.vtx` / `.sw.vtx`；L4D2 是 DX9 引擎，目前只产 `.dx90.vtx` |

### 已知的语义等价差异（不必追）

- **VVD 顶点排列顺序**不同：`studiomdl` 按它内部的哈希/去重顺序重排，mdlc 按 SMD
  出现顺序。顶点索引表会跟着一起变，两边自洽，**不影响渲染**。
- **法线有约 `1e-7` 的量化误差**（`-0.9999998807907104` vs `-1.0`）。
- **逐 float 位完全相同的切线比例较低**（浮点运算顺序不同：x87/SSE 混合路径 vs
  Rust f32）。方向（cos）才是语义判据。

---

## 延伸文档

仓库内的 `docs/` 收录了五份**实测反推**的规格报告（结论全部来自对真实产物的
逐字节分析，不是从 `studio.h` 推断的）：

| 文档 | 内容 |
|---|---|
| [`docs/animation-layout.md`](docs/animation-layout.md) | MDL 动画数据布局的实测规格（含已推翻结论的勘误） |
| [`docs/blend-sequences.md`](docs/blend-sequences.md) | blend 序列的完整规格（QC 侧 + 二进制侧） |
| [`docs/coordinate-systems.md`](docs/coordinate-systems.md) | 坐标系约定与 `$staticprop` 几何旋转 |
| [`docs/qc-coverage-gap.md`](docs/qc-coverage-gap.md) | 以 L4D2 `studiomdl.exe` 的 **137 条分发表**为基准的 QC 覆盖对照 |
| [`docs/feature-gap.md`](docs/feature-gap.md) | 相对官方 `studiomdl` 的特性差距清单与优先级 |

> ⚠️ **`docs/feature-gap.md` 与 `docs/qc-coverage-gap.md` 是调研报告**，
> 带有明确的快照日期（当时的文件 SHA256 与测试数）。**它们描述的是历史状态**，
> 「当前实现了什么」应以**源码与 `README.md` 为准**。
