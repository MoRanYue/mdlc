# mdlc — Source 引擎模型编译器（studiomdl 重写的 MVP）

把 **TOML 描述文件 + SMD 网格**编译成 Source 引擎能加载的 `.mdl` + `.vvd`。

本工程是 `studiomdl.exe` 重写的**第一阶段**：先打通「描述 + 网格 → 二进制」
这条主动脉，用真实编译器做参照物逐字段对齐，再逐步扩展覆盖面。

## 职责划分（与 QC 一致）

| 内容 | 由谁承载 | 对应 QC |
|---|---|---|
| 模型名 / 材质 / 骨骼 / body part 树 | TOML 描述文件 | `$modelname` / `$cdmaterials` / `$definebone` / `$bodygroup` |
| **网格（顶点、法线、UV、蒙皮）** | **SMD 文件** | `studio "x.smd"` |
| **参考姿态** | **SMD 的 `skeleton` 第 0 帧** | 参考 SMD |

网格**不写在 TOML 里** —— 真实模型有几万到几十万顶点（官方
`v_autoshotgun` 有 388,765 个），内联会让描述文件膨胀到几百 MB
且无法用文本工具处理。描述文件只**引用** SMD。

## 当前状态

| 项 | 状态 |
|---|---|
| VVD 读写（逐字节往返） | ✅ 官方模型 24,881,024 字节**完全一致** |
| **VVD fixup 表 + 多 LOD** | ✅ **语料 3302/3302 逐字节往返**（含 53 个 fixup、230 个多 LOD） |
| **真实切线计算** | ✅ **与官方 studiomdl 同几何对照：w 8/8、方向 8/8**（见下） |
| SMD 解析 | ✅ 真实文件验证（89 骨骼 / 22,911 三角形） |
| MDL 写出（头部/骨骼/材质/body part/model/mesh/字符串池） | ✅ 与 studiomdl 产物**逐字段对齐** |
| **VTX 写出** | ✅ **与 studiomdl 产物逐字段完全一致（0 差异）** |
| **VTX 多 LOD** | ✅ 结构树 / `switchPoint` / `origMeshVertID` 按实测规则写出 |
| **hitbox set** | ✅ **全部字段一致**（`$hboxset` / `$hbox`） |
| **`$attachment`** | ✅ **全部字段一致**（含 `local` 矩阵） |
| **`$bonemerge` + 骨骼 flags 按用途计算** | ✅ **与官方逐位相同**（如 `0x40700`） |
| **动画 / 序列（`$sequence`）** | ✅ **动画链语义与官方完全一致**（见下） |
| 骨骼 `poseToBone` | ✅ 官方模型 89 根骨骼**逐骨骼吻合**（误差 < 1e-3） |
| 骨骼 `quat` | ✅ 与官方**逐位相同**（`[0.7071066,0,0,0.7071069]`） |
| VVD 写出 | ✅ 布局自检通过 |
| 真实规模编译 | ✅ 89 骨骼 / 22,911 三角形 / 5 万顶点，0.21 秒 |
| QC 解析 | ❌ 未实现（MVP 刻意用 TOML，见下） |
| flex / PHY | ❌ 未实现 |
| **LOD 的自动生成（简化网格）** | ❌ 未实现 —— 只支持**输入**多 LOD，不做 decimate |

> **三件套齐全**：`.mdl` + `.vvd` + `.dx90.vtx` 都已产出并通过布局自检。
> 特性差距的完整清单（137 条 QC 命令逐组对照、8 层分解、拦路虎）
> 见 [`docs/feature-gap.md`](docs/feature-gap.md)。

### 切线、多 LOD、fixup（实测报告）

这三块都是「写错**不会报错**、只会在游戏里表现异常」的类型，所以全部结论
都来自对真实产物的实测，探针脚本在 `docs/_probe/`。

#### 1. 真实切线：算法与吻合率

算法逐字对应 studiomdl 的 `CalcTriangleTangentSpace` /
`CalcModelTangentSpaces`（`hl2sdk-episode1\utils\studiomdl\simplify.cpp`
4920 / 5037 行），实现在 [`src/tangent.rs`](src/tangent.rs)。

**语料实测**（3072 个单 LOD 模型、631 万个顶点，用真实 VVD 的顶点 +
配套 VTX 的三角形重算，与官方写下的切线逐条比对）：

| 指标 | 实测 |
|---|---|
| 方向一致（cos > 0.999） | **96.69%** |
| 手性 `w` 一致 | **99.96%** |
| 逐 float 位完全相同 | 2.23% |
| ≤ 4 ULP | 56.98% |

逐位吻合率低是**浮点运算顺序**导致的（x87/SSE 混合路径 vs Rust f32），
方向（cos）才是语义判据。

**剩下 3.31% 是什么**：逐模型统计是**双峰**的 —— 3013/3067 个模型 > 99%，
54 个接近 0%。对后者做独立判据：

```text
|dot(normalize(官方切线), VVD 里存的法线)|
  好的模型（3013 个）：平均 0.000000，最大 0.000012
  差的模型（  54 个）：平均 0.468690，最大 1.000000
```

也就是说这些模型的**官方切线根本不垂直于它自己 VVD 里的法线** ——
矛盾出在数据本身（法线是另一套来源），不是重算算法的问题。
代表模型：`v_autoshotgun` / `v_rifle` / `v_chainsaw` / `v_medkit`。
另验证过 8 种候选约定（S/T 互换、UV 翻转、各种叉积组合），
没有任何替代约定能同时解释那 54 个（`probe_tangent_hypotheses.js`）。

#### 2. SMD 的 V 必须翻转（一条曾漏掉的关键规则）

studiomdl 在**解析 SMD 时**就把 UV 的 V 翻转
（`hl2sdk-episode1\utils\studiomdl\v1support.cpp:161`）：

```c
// invert v
t[1] = 1.0 - t[1];
```

翻转后的值才进 `s_source_t.vertex[].texcoord`，因此它**同时**决定
VVD 里的 UV 与切线的计算。两条独立实测判据：

| 判据 | 结果 |
|---|---|
| SMD ↔ 官方 VVD 逐顶点 UV 对照（`myprop` / `rb`） | **11/11 顶点满足 `vvd.v == 1 - smd.v`** |
| 切线手性 `w`（同几何，翻转 vs 不翻转） | 翻转 **8/8**；不翻转 **0/8** |

修正后与官方 studiomdl 的同几何产物对照：**`w` 8/8、方向 8/8、
4/8 逐 float 位相同**。

> 注意：VVD 里存的**已经是翻转后**的值，所以「读真实 VVD 反推切线」时
> **不能**再翻一次 —— 那会把 w 吻合率从 99.96% 打到 2.29%
> （`probe_vflip_corpus.js` 实测）。翻转只发生**一次**，在 SMD 解析这一步。

#### 3. 多 LOD 与 fixup 表

布局与排序规则来自 `write.cpp` 的 `BuildSortedVertexList`（2407 行）、
`FindVertexOffsets`（2326 行）、`FixupVvdFile`（2700 行）与
`_CompareUsedVertexes`（2370 行）；实现在 [`src/lod.rs`](src/lod.rs)。

关键语义（**都不是从 `studio.h` 猜的，是实测反推的**）：

- 每个顶点带一个 `lodFlags` 位掩码（bit n = 被 LOD n 使用）。
- 排序键：**最高置位 bit 降序** → mesh 序升序 → mesh 内顶点号升序。
- `numLODVertexes[n]` 是**累计值**（渲染 LOD n 所需的顶点数 =
  所有「细节不低于 n」的块之和），所以**单调不增**；尾部槽位 ripple 成
  最后一个有效值。
- fixup 表：每个 mesh 内按 LOD **从粗到细**，每段
  `{lod, sourceVertexID, numVertexes}`。
- `mstudiomesh_t.numvertices` = 该 mesh **跨全部 LOD 去重后**的顶点总数
  （不是 LOD 0 的顶点数）。
- `origMeshVertID` = `finalMeshVertID`（把 LOD 排序的块拼回 mesh 顺序后的编号）。

**实测验证**（53 个真实 fixup 模型，7 条不变式 **53/53 全通过**，
`probe_vvd_lod_model.js`）：

| 不变式 | 含义 |
|---|---|
| R1 | 各 block 精确铺满 `[0, numLODVertexes[0])`，不重叠无空洞 |
| R2 | `numLODVertexes[n] == Σ_{k>=n} 独占数` |
| R3 | 按 `sourceVertexID` 扫描时最高位非递增 |
| R4 | 同一 mesh 内 block 按 LOD 降序 |
| R5 | fixup 的 mesh 分组数 == MDL 的 mesh 总数 |
| R6 | `MDL.mesh.numvertices == Σ block 长度` |
| R7 | VTX 的 `origMeshVertID` 落在该 LOD 的 block 覆盖范围内 |

**头部偏移**（53/53 实测，`probe_vvd_fixup_semantics2.js`）：

```text
fixupTableStart  = ALIGN4(64)                     = 64
vertexDataStart  = ALIGN16(fixupTableStart + numFixups * 12)
tangentDataStart = ALIGN16(vertexDataStart + numLODVertexes[0] * 48)
文件长度          = tangentDataStart + numLODVertexes[0] * 16   （尾部无填充）
```

`ALIGN16` **不能省略** —— 例如 3 条 fixup = 36 字节时顶点块从 100 对齐到 112。

**往返判据**：语料 **3302/3302 个 VVD 逐字节往返相同**（含全部 53 个
带 fixup、230 个多 LOD 的模型），由
`real_fixup_vvds_round_trip_byte_for_byte` 测试钉住。

**单 LOD 不受影响**：`numLODs == 1` 时排序是恒等变换、`numFixups == 0`，
产物与加这个特性之前**逐字节相同**。

**输入格式**（TOML）：

```toml
[[bodyparts.models]]
smd = "lod0.smd"          # LOD 0（最精细）

[[bodyparts.models.lods]]
smd = "lod1.smd"
switch_point = 20.0       # 可省略，缺省按 20/40/80… 推算

[[bodyparts.models.lods]]
smd = "lod2.smd"
switch_point = 40.0
```

各 LOD 的**材质集合必须一致**（否则报错，不静默对齐）；
LOD 0 之外的网格由 `lods` 给出，最多 8 层。

> **未实现**：LOD 的**自动生成**（网格简化 / decimate）。
> 本实现只接受显式给出的多 LOD 输入，不会替你简化网格。

### 动画：与官方产物的实测对照

**小模型（2 骨骼 × 5 帧，`anim.toml`）——逐值完全一致：**

| 项 | 官方 studiomdl | mdlc |
|---|---|---|
| `bone[0]`（root）编码 | `flags=0x20` `RAWROT2` | **相同** |
| `bone[0]` 四元数 | `[0, 0, 0.707106, 0.707107]` | **逐位相同** |
| `bone[1]`（tip）编码 | `flags=0x08` `ANIMROT` | **相同** |
| `bone[1]` valueptr 偏移 | `[0, 0, 6]` | **相同** |
| `bone[1]` Z 轴采样 | `[0, 10922, 21844, 32767, 0]` | **逐值相同** |
| `fps` / `numframes` / `numblends` | 30 / 5 / 1 | **相同** |
| 解码后采样 | 5 个 | **差异 0 个** |

**真实规模（89 骨骼 × 30 帧，`biganim.toml`）：**

| 项 | 结果 |
|---|---|
| `rotscale` / `posscale`（267 个轴） | **100% 逐位相同** |
| 解码后采样（5370 个） | 18 个差 **1 LSB**（0.33%） |

剩下 0.33% 的 1-LSB 差异来自 studiomdl 内部中间量的浮点精度
（约 6 ulp），**不影响任何语义** —— 差 1 个量化步长，
在 30 fps 的插值动画里完全不可见。

复现命令：

```powershell
.\verify_parity.ps1                        # MDL/VVD/VTX 逐字段
node docs\_probe\decode_chain.js <mdl>      # 解码动画链（形态对照）
node docs\_probe\cmp_anim_semantics.js <官方> <mdlc>   # 逐值语义对照
```

### 动画实现的关键规则（都曾写错，且都不会报错）

1. **存储值是「规范化欧拉角 − 参考姿态」，不是「相对第 0 帧的增量」。**
   这是最重要的一条。早先的结论（见 `docs/animation-layout.md` §3.5）
   来自 `exp50`/`exp53`/`exp64`/`exp65` —— 那些 SMD 的**第 0 帧旋转
   恰好都是 0**，于是两种模型给出**完全相同**的结果，实验无法区分。
   在 89 骨骼 × 30 帧上实测命中率：**13.14% vs 100.00%**。

   ```text
   stored[f][k] = wrapToPi( canonical_euler(pose[f])[k] − ref_rot[k] )
                  + (k == 2 && 根骨骼 ? π/2 : 0)
   stored_pos[f][k] = pose[f].pos[k] − ref_pos[k]
   ```

   其中 `ref_rot`/`ref_pos` 是**骨骼表里的参考姿态**（SMD 第 0 帧或
   `$definebone` 覆盖后的值）—— 所以它必须由调用方显式传入，
   不能自己从 `seq.frames[0]` 取。

2. **`canonical_euler` 必须全程 `f64`。** 降到 f32 会让规范化结果差
   约 4e-6（`bone 66 "bolt"` 的 `rot[0]`：f32 给 `3.135712`，
   f64 给 `3.135716`，官方是 `3.135716`），进而使动画存储值差 1 LSB。

3. **量化是「向零截断」**，不是四舍五入、也不是向下取整。两条独立判据：
   `exp65`（正数 10° → 8191.75 → 官方 **8191**）、
   `negq`（负数 −0.1 → −8344.05 → 官方 **−8344**；floor 会给 −8345）。
   89 骨骼模型上的逐值命中率：`trunc` **99.59%** vs `round` 54.15%
   vs `floor` 26.89%。

4. **量化除数按极值符号选择**：极值为负 → `/32768`，为正 → `/32767`。
   `i16` 的范围 `[-32768, +32767]` **不对称**，studiomdl 让极值正好
   压在边界上。实测反解 `div = maxAbs / 官方scale`：极值为负的 31 个轴
   平均 **32767.995**，为正的 22 个轴平均 **32766.998**，零反例。
   下限主导时（`floor` 是正数常量）恒用 32767。

5. **`rotscale`/`posscale` 要两趟扫描。** 它们在 bone 表里是**全局一份**
   （所有序列共用）。边扫边量化的话，后面的序列抬高 scale 会让前面
   已量化的整数作废，解码后表现为「动画幅度被压缩」。

6. **根骨骼 Z 的 +90° 偏置在「减参考姿态」之后加。** `gen_rootbias`
   实验（根 Z 参考姿态非零）实测 `2.234459` 对上「减完再加」的
   `2.234464`；「先给参考加偏置」会给出 `−0.907129`。

7. **`LOOPING` 末帧照抄第 0 帧的存储值**，不是「归零」。
   `gen_loop` 实验（第 0 帧偏离参考姿态）实测末帧 = `0.499991`
   （即第 0 帧的值），而不是 0。文档 §3.4 的「末帧强制 0」只在
   「第 0 帧 == 参考姿态」时成立。

8. **增量要 `wrapToPi` 包裹。** `canonical_euler` 的值域是 `(−π, π]`
   （`atan2` 割线），而参考姿态可能也在 π 附近。直接相减会在割线处
   产生 ±2π 跳变（实测 `bone 66 "bolt"` 的 roll ≈ ±π，2 帧跳到 −32768）。

9. **两个 valueptr 必须紧挨着，中间不能插流。** `mstudio_rle_anim_t`
   的 `pPosV()` 只按 `ANIMROT` 是否置位偏移 6 字节，所以顺序必须是
   「常量载荷 → rotV → posV → 各轴的流」。写成「rotV → rot 流 → posV」
   会让 `pPosV()` 落进 rot 的流里，解码器顺着垃圾偏移读到文件尾。

10. **骨骼表的 `rot` 也要规范化。** `gimbal` 实验（pitch = π/2）实测
    官方把 SMD 的 `[0.35, π/2, −0.25]` 写成 `[0, π/2, −0.6]`。
    不规范化会让骨骼表与动画的参考姿态基准不一致，整个动画偏移一个常量。

### 与官方产物的实测差距（同一份几何）

| 项 | 结果 |
|---|---|
| MDL 语义差异 | **0**（40 处差异中 32 处是我方未实现的段，8 处是结构性偏移） |
| 骨骼参考姿态 | **仅 f32 舍入级**（最大绝对误差 2e-4，出现在 89 根骨骼链末端） |
| VVD 顶点 | 排列顺序不同 + 法线约 1e-7 量化误差（语义等价） |

**尚未对齐的骨骼字段**（已知，属未实现功能）：
`contents`（官方 1 vs 我方 0）、`procedural_rule_*`（jigglebone 等程序化骨骼）、
`surface_prop_offset`（值不同但指向同一字符串，属布局差异）。
`flags` 已按用途逐根计算并与官方逐位一致。

## 为什么 MVP 用 TOML 而不是 QC

QC 有约 140 条命令、多套块语法、宏展开、`$include` 与 `$pushd/$popd` 目录栈，
还有只在特定上下文合法的命令（`flexfile` 写在顶层会报 `bad command`）。
把 QC 解析和二进制写出**同时**做，等于一次面对两个未验证的子系统，
出错时无法判断是哪一边的问题。

所以 MVP 先把输入定义成 TOML。QC 适配放到后续阶段 ——
届时只需把 QC 解析成同一个 IR，写出器一行都不用改。

## 用法

```powershell
cargo build --release

# 打印带注释的模板（含 hitbox / attachment / bonemerge 的示例）
.\target\release\mdlc.exe template > myprop.toml

# 校验描述文件 + SMD（材质是否声明、骨骼是否对得上都会查）
.\target\release\mdlc.exe check myprop.toml

# 编译（产出 .mdl + .vvd + .dx90.vtx）
.\target\release\mdlc.exe build myprop.toml --out .\out

# 布局判据：官方 VVD 读入再写出必须逐字节相同
.\target\release\mdlc.exe vvd-roundtrip <官方.vvd>
```

### 三个一键回归脚本

```powershell
.\verify_parity.ps1    # 同一几何：mdlc vs 真实 studiomdl，逐字段对照
.\cmp_features.ps1     # hitbox / attachment / bonemerge 三项的逐字段对照
```

> `cmp_features.ps1` 需要先用 studiomdl 编译 `parity\myprop.qc`
> （`verify_parity.ps1` 会做这件事），再用
> `mdlc build parity\hb.toml --out parity\hb` 生成对照产物。

### TOML 的一个陷阱

**顶层键必须写在所有 `[[表]]` 之前**。例如把 `bonemerge = [...]` 写在
`[[attachments]]` 之后，TOML 会把它解析成 `attachments` 的字段而报错。
本实现因此把 `bonemerge` 放在 `[[bones]]` 里（`bonemerge = true`），
而不是设一个顶层数组。

## 与 studiomdl 的逐字段对照（核心验证手段）

一键回归：

```powershell
.\verify_parity.ps1     # 编译 → 用真实 studiomdl 编译同一几何 → 逐字段对照
```


`D:\GITHUB\mdlc-oracle` 是配套的 oracle 工具，用**独立解析器**读两边的产物再逐字段比：

```powershell
cd D:\GITHUB\mdlc-oracle
cargo build --release

# 同一份几何分别用 studiomdl 与 mdlc 编译后：
.\target\release\oracle.exe diff <studiomdl产物>.mdl <mdlc产物>.mdl
```

### 实测结论（`mymod/myprop.mdl`，8 顶点立方体）

- **42 处差异中 32 处是「MVP 未实现的段」**（studiohdr2、hitbox set、
  bone controller、序列、skin、flex、ik 等，我方写 0）—— 预期差异。
- 其余 8 处**全部是结构性偏移**：因为 studiomdl 多写了那些段，
  后面所有块的起始偏移与文件长度随之推后。
- **语义差异为 0。**

## 靠差分揪出的真实缺陷（全部已修）

这些**都不会报错**，只会产出「能解析但进游戏就错」的文件 ——
正是 Phase 0 先建 oracle 的价值所在：

1. **`mstudiobone_t` 字段偏移整体错位**：`pos` 在 `0x20` 不是 `0x0C`、
   `rot` 在 `0x3C` 不是 `0x24`（`0x08..0x1F` 是 6 个 bonecontroller 下标）。
   实测依据：官方 `v_autoshotgun.mdl` 的 `bone[0].rot @0x3C = [1.5708,0,0]`。
2. **`sznameindex` 是相对自身而非绝对**：骨骼、材质、body part 三处都是。
   实测依据：官方 `bone[0].sznameindex = 614136`，骨骼表在 664，
   文件偏移 614136 处是空串，**664+614136** 处才是 `ValveBiped.ValveBiped`。
3. **`mstudiobodyparts_t.modelindex` / `mstudiomodel_t.meshindex` 是相对父结构**，
   不是绝对偏移。
4. **`mstudiotexture_t.flags` 在 `0x04`**，不是 `0x40`（0x40 是下一项的名字偏移）。
5. **头部 `0x140` 之后整段偏移错位**：`mass` 在 `0x148`（且缺省值是 **1.0** 不是 0）、
   `contents` 在 `0x14C`。
6. **`numLODVertexes` 八个槽位都要填**，只填 `[0]` 会让引擎的 LOD 切换读到 0 顶点。
7. **`mesh.modelindex` 写的是 `-148`**（`-MODEL_SIZE`），不是 model 下标。
8. **`$cdmaterials` 落盘时规范化为反斜杠 + 结尾分隔符**（`models\mymod\`），
   且数组项是**绝对**偏移、数组与字符串是**分开的两块**。
9. **材质名落盘时剥掉 `$cdmaterials` 前缀**（`models/mymod/myprop` → `myprop`）。
10. **骨骼缺省 `flags` 是 `0x500`**（`BONE_USED_BY_VERTEX_LOD0 | BONE_USED_BY_HITBOX`），
    写 0 会被判定为「未被使用」。
11. **骨骼 `surfaceprop` 缺省继承头部的 `$surfaceprop`**。
12. **`view_bb` 在没写 `$cbox` 时留 0**，不复用 hull。
13. **`angle_matrix` 的欧拉约定与 Source 的 `AngleMatrix` 相反**（写成了转置）。
    这条最凶险：它让 `poseToBone` 变成 `R` 而不是 `R⁻¹`，**顶点被反向旋转**。
    对 `rotation = 0` 的合成模型完全看不出来，而真实模型的第一根骨骼
    几乎都带 π/2 旋转。当时的代码注释还写着「实测依据：官方 bone[0].rot…」
    —— 那条实测**根本没跑过**，注释在撒谎。
    现在由 `real_model_pose_to_bone_matches_official` 钉住：89 根骨骼逐字段比对，
    任何约定错误都会让它全线失败。
14. **SMD 三角形顶点行是 12 个 token**（行首是 `parentBone`），
    且**没有索引行** —— 每个三角形是「材质名 + 3 个顶点行」。
    写成 OBJ 风格会让 studiomdl 报 `bogus bone index` 或直接崩溃。
15. **`mstudiobone_t.quat` 不是可选字段**，必须写 `AngleQuaternion(rot)`。
    留 0 是**非法四元数**（模长 0，无法归一化），引擎的 `InitPose`
    直接读它，会让参考姿态失效。实测官方 `bone[0]`：
    `rot=[1.570796,0,0]` ↔ `quat=[0.7071066,0,0,0.7071069]`。
16. **L4D2 的 VTX 没有 v49+ 扩展** —— `StripGroupHeader_t` 是 **25** 字节、
    `StripHeader_t` 是 **27** 字节（不是 33/35）。
    这条纠正了本工程与 `plank` 早先按「MDL ≥ 49 即有扩展」的判断。
    判据见 `mdlc-oracle/src/vtx.rs` 的模块文档与
    `real_model_strip_group_chain_closes_at_25` 测试。
17. **`stripGroup.flags` 用 `HWSKINNED`(0x02)**，不是 `FLEXED`(0x01)。
    无 flex 的模型写 1 会让引擎去查不存在的 flex 数据。
18. **`boneWeightIndex` 恒写 `[0,1,2]`**（固定序列），不是「0..boneCount」。
    写 0 填充会让槽位 1/2 都指向 `weight[0]`。
19. **`materialReplacementList.replacementOffset` 写 0**，不是「指向数组末尾」。
20. **`mstudiomesh_t.vertexoffset` 是相对 model 的**，不是全局顶点下标。
    实测官方 `bp[2].model[0]` 从 VVD 顶点 99996 开始，但它的
    `mesh[0].vertexoffset` 是 **0**。
21. **`mstudiomesh_t.modelindex` = model 绝对位置 − mesh 绝对位置**
    （负值）。实测官方三个 body part 分别是 −2572 / −4512 / −4480 ——
    **各不相同**，所以不能写死 `-148`（那只是「单 mesh 且 mesh 紧跟
    model」时的巧合，我一开始正是这么错的）。
22. **`mstudiobbox_t.hitboxindex` 是相对 hitbox set 自身**的偏移
    （实测官方 = 12），不是绝对偏移。
23. **`mstudioattachment_t.local` 的平移列在矩阵下标 3/7/11，
    即字节偏移 `0x0C + 3*4 / 7*4 / 11*4`** —— 我漏乘 4 写成
    `+3/+7/+11`，结果附着点位置全丢（写成极小浮点数的位模式）。
    这类错误不会报错，只会让枪口火焰出现在错误的位置。
24. **骨骼 `flags` 必须按用途计算，且沿父链向上传播**。
    实测官方：`$hbox 0 "root"` + `$attachment "muzzle" "tip"`
    → `bone[0] = 0x40700`、`bone[1] = 0x200`。
    早先一律写 `0x500` 会把未被顶点使用的骨骼误标为已使用。
    `DEFAULT_BONE_FLAGS` 只在**完全无信息**时兜底。
25. **动画量化是「向零截断」**（`trunc`），不是四舍五入，也不是向下取整。
    两条独立实测判据：`exp65` 正数 10° → 8191.75 → 官方 **8191**；
    `negq` 负数 −0.1 → −8344.05 → 官方 **−8344**（floor 会给 −8345）。
26. **量化除数按极值符号选**：极值为负 → `/32768`，为正 → `/32767`。
    `i16` 范围 `[-32768, +32767]` 不对称，studiomdl 让极值正好压在边界上。
    实测反解 `div = maxAbs / 官方scale`：负的 31 个轴平均 **32767.995**，
    正的 22 个轴平均 **32766.998**，零反例。恒用 32767 会让所有负向
    通道幅度偏大约 1/32767。
27. **动画存储值是「规范化欧拉角 − 参考姿态」**，不是「相对第 0 帧的增量」。
    这条推翻了 `docs/animation-layout.md` §3.5 —— 那个结论的依据
    （`exp50`/`exp53`/`exp64`/`exp65`）第 0 帧旋转恰好都是 0，
    两种模型给出相同结果，实验**无法区分**。89 骨骼 × 30 帧实测：
    13.14% vs **100.00%**。
28. **`canonical_euler` 必须全程 `f64`。** 降到 f32 会让规范化结果差
    约 4e-6（`bone 66` 的 `rot[0]`：f32 `3.135712` vs 官方 `3.135716`），
    进而使动画存储值差 1 LSB。
29. **`LOOPING` 末帧照抄第 0 帧的存储值**，不是「归零」。
    `gen_loop` 实验（第 0 帧偏离参考姿态）实测末帧 = `0.499991`。
30. **根骨骼 Z 的 +90° 偏置在「减参考姿态」之后加。**
    `gen_rootbias` 实测 `2.234459` 对上「减完再加」的 `2.234464`。
31. **增量要 `wrapToPi` 包裹。** `canonical_euler` 值域是 `(−π, π]`，
    参考姿态可能也在 π 附近，直接相减会在割线处产生 ±2π 跳变。
32. **两个 `valueptr` 必须紧挨着**，中间不能插流 —— `pPosV()` 只按
    `ANIMROT` 是否置位偏移 6 字节。写成「rotV → rot 流 → posV」会让
    解码器顺着垃圾偏移读到文件尾。
33. **骨骼表的 `rot` 也要规范化。** `gimbal` 实验实测官方把
    `[0.35, π/2, −0.25]` 写成 `[0, π/2, −0.6]`。
34. **`RAWROT2` 的判定是「旋转逐轴都不随时间变化」**，不是「三轴同一个角度」。
    `Absent` 轴记 0、`Constant` 轴取值，三轴合起来是常量就用 `RAWROT2`。
    最小模型里恰好是 `[0,0,π/2]`，容易被误读成「整体一个角度」。
35. **`mstudioanim_valueptr_t.offset[i] == 0` 表示该轴无数据**
    （`pAnimvalue` 返回 NULL），不是「偏移 0」。
    实测 `exp50` 的 `rotV = [6, 0, 16]`：中间的 0 就是恒 0 的 Y 轴。
36. **`mstudioanimvalue_t` 是 union**（`{valid,total}` ∪ `short value`）。
    解码循环是「读 `valid` 个采样，再把最后一个重复 `total - valid` 次」，
    `total == 0` 才终止。把 `value` 当成「run 的代表值」是错的。
37. **链尾有一条 4 字节全零记录**，且末条记录的 `nextoffset == 0`。
38. **空链写 `numbones` 条 `ff 00 00 00`** —— `bone == 255` 是
    「该骨骼在本动画无数据」的**占位记录，不是终止符**。
39. **SMD 的 UV 必须翻转 V**（`v → 1-v`），且翻转发生在**解析 SMD 时**
    （`v1support.cpp:162` 的 `t[1] = 1.0 - t[1];`），不是写 VVD 时。
    漏掉它会让 VVD 的 UV 与官方不一致，**并让切线手性 `w` 系统性错一个符号**
    —— 表现为法线贴图的凹凸方向整体反了，且不会报任何错。
    实测：修正后与官方同几何产物的 `w` 一致率从 0/8 变成 **8/8**。
40. **切线要用真实三角形算，不能用「法线叉积」占位。**
    占位切线在引擎的静态光照路径下看不出问题，但法线贴图会发黑/方向错。
    算法见 `src/tangent.rs`；语料 631 万顶点的方向吻合率 **96.69%**，
    其余 3.31% 集中在 54 个「官方切线不垂直于自己法线」的模型上
    （数据本身矛盾，不是算法差异）。
41. **`numLODVertexes[n]` 是累计值**（渲染 LOD n 所需的顶点数），
    不是「第 n 个 LOD 各自的顶点数」。它**单调不增**，且尾部槽位
    ripple 成最后一个有效值。当成「各自顶点数」会让 LOD 切换读到错误范围。
42. **`mstudiomesh_t.numvertices` 在多 LOD 下是「跨全部 LOD 去重后的总数」**，
    不是 LOD 0 的顶点数；`vertexoffset` 按这个总数累加。
    用 LOD 0 的值会让 `origMeshVertID` 越界（实测 53/53 个真实模型
    满足 `numvertices == Σ block 长度`）。
43. **有 fixup 时 `vertexDataStart != 64`**：顶点块被推到
    `ALIGN16(64 + numFixups*12)`。沿用单 LOD 的「三偏移都是 64」会让
    引擎把 fixup 表当顶点读。`ALIGN16` 在 `numFixups*12` 不是 16 的倍数时
    才真的移动偏移（3 条 fixup：100 → 112），所以**不能靠巧合通过**。
44. **单 mesh 的多 LOD 模型 `numFixups == 0`**（`write.cpp` 2777 行显式跳过：
    数据本来就连续，不需要重定位表）。实测 177 个「多 LOD 且单 mesh」的
    真实模型全部为 0，53 个「多 LOD 且多 mesh」的全部非 0。
    无条件写 fixup 表会与官方形态不同（虽然仍可读）。
45. **`mstudiomesh_t` 偏移 `0x20` 是 `meshid`（全局序号），不是 `numBones`。**
    实测 5585/5585 个真实 mesh 的 `0x20` 恰好等于它在该 model 内的序号
    （0,1,2…），骨骼数完全对不上。早先把它当 `numBones`、把 `0x24` 当
    `boneIds[8]` 是错的 —— `0x24` 是 `center`（`Vector`，实测恒为 0）。
    写错会让引擎读到一个非法的 `meshid`。
    判据：`docs/_probe/probe_mesh_offsets.js`。
46. **`mstudiomesh_t` 偏移 `0x34` 是 `numLODVertexes[8]`**，语义与 VVD 的
    同名数组同构但**作用域是单个 mesh**：`[n]` = 该 mesh 中「最高 LOD 位
    >= n」的顶点数（累计值，单调不增）。
    实测 **448/448** 个多 LOD 模型的 mesh 满足，且
    **`Σ_mesh mesh.numLODVertexes[n] == VVD.numLODVertexes[n]`**。
    单 LOD 时八个槽位都填 `numvertices`（实测官方是 `[8,8,8,8,8,8,8,8]`）；
    留 0 会让引擎认为该 mesh 在该 LOD 没有顶点。
    判据：`docs/_probe/probe_mesh_numlodvertexes.js`。

## 一条重要的格式事实：checksum 不是内容哈希

`.mdl` / `.vvd` / `.vtx` / `.phy` 四件套里的 `checksum` **只是配对令牌**：

- Crowbar 与 plank 都只**比较**、从不计算它；
- 引擎只做配对校验，报错形如
  `Error Vertex File for '...' checksum 1875668995 should be -1530146530`。

所以编译器只需生成一个值并原样写进四个文件，**不需要逆向任何哈希算法**。
本实现用模型名的 FNV-1a（稳定、跨进程一致；`DefaultHasher` 不保证跨进程稳定）。

## 已知未实现（后续阶段）

优先级与完整清单见 [`docs/feature-gap.md`](docs/feature-gap.md)。要点：

- QC 解析（→ IR）；**137 条命令目前只支持约 8 条**
- 动画的 **RLE run 合并**（当前只做常量折叠，见下）
- 动画的 `sectionframes` 分段、IK rule、movement、`STUDIO_FRAMEANIM`
- flex / eyeball / mouth / ik
- **LOD 的自动生成**（网格简化 / decimate）—— 多 LOD 的**输入与写出**已支持，
  但不会替你简化网格
- PHY
- 程序化骨骼（jigglebone 的 `procedural_rule_type` / `procindex`）
- DMX 输入（`dmxconvert` 子进程）、VTA、VRD
- `$definebone` 语义、`$illumposition` 轴交换等 QC 级变换

> **注**：`studiohdr2`、`bonetablename`、动画 events、`$keyvalues`、
> `bonecontroller` 段已实现（见下「已实现的新增段」）。

### 段布局是一个**框架**，不是一个长函数

MDL 的段偏移集中在 [`src/layout.rs`](src/layout.rs) 里声明式地计算
（`SectionOffsets::compute` + `SectionCounts`）。`write_mdl` 只负责
「填计数」和「按算出的偏移写字节」。

**加一个新段只需三步**（以 `flexdesc` 为例）：

1. `SectionCounts` 加字段（已预留，填真实数量即可）；
2. `compute` 里把 `+ 0` 换成 `+ n * SIZE`（已写成可累加形式）；
3. `write_mdl` 里按 `layout.flexdesc` 写字节。

顺序**不需要**判断 —— 它在 `compute` 里硬编码为权威顺序，写错会被
`check_monotonic()` 抓到。这个设计的目的是让「一点点实现剩余特性」
不会因为布局耦合而互相干扰。

### 已实现的新增段（2026-09）

| 段 | 实测频率 | 说明 |
|---|---|---|
| `studiohdr2` | **3333/3333 (100%)** | 固定放 408（Crowbar 硬编码该位置） |
| `bonetablename` | **3333/3333 (100%)** | `byte[numbones]` 名字索引表 |
| 动画 events | **1802/3333 (54.1%)** | `mstudioevent_t` = 80 字节，名字是**相对偏移**（不是内联） |
| `$keyvalues` | **735/3333 (22.1%)** | `"mdlkeyvalue\n{…}\n\0"`，与官方逐字节一致 |
| `bonecontroller` | 0/3333 (0%) | 段占位（L4D2 已废弃该特性） |

**空段的偏移语义**：实测 studiomdl 对**计数为 0** 的段仍写「该段应处的
位置」，而不是 0（3333/3333 模型的 `bonecontroller` 如此）。早先的实现
一律写 0，与官方逐字段对照时会产生十几处假差异。

### 段顺序的权威结论（3333 个真实模型实测）

```text
studiohdr2 → bone → bonecontroller → attachment → hitboxset
  → bonetablename → localanim → 动画链 → localseq → seq 子表
  → bodypart → localnodename → model 数组
  → flexdesc → flexcontroller → flexrule → ikchain → mouth
  → poseparam → ikautoplaylock → mesh 数组
  → texture → includemodel → animblock → cdtexture → skin
  → 字符串池 → keyvalues → $cdmaterials 数组
```

两条最容易写错的（早先的实现**两条都错了**）：

1. **`attachment` 恒在 `hitboxset` 之前**（3333/3333 实测）；
2. **`mesh` 数组在 flex/ik/poseparam 之后**，不是紧跟 model（3300/3300 实测）。

归纳脚本：`docs/_probe/canonical_order2.js`。

### 重建验证语料

```powershell
# 从 pak01 解出全部 3333 个模型（含 2498 个 .phy）
node docs\_probe\vpk_extract.js `
  "E:\SteamLibrary\steamapps\common\Left 4 Dead 2\left4dead2\pak01_dir.vpk" `
  "D:\DSH\L4D2ReverseEngineering\mdl-corpus"
```

> **不要用 L4D2 自带的 `bin\vpk.exe`** —— 它在本机写出**全零文件**
> （打印 `extracting` 后跟 `FS: Tried to Write NULL file handle!`），
> 且一次传 200 个文件名时会静默失败。所以自实现了 VPK v1 解包器。

特性频率统计脚本：`docs/_probe/survey_features.js`。

### 动画的已知取舍（不影响正确性）

- **不做 RLE run 合并**，只做常量折叠。`mstudioanimvalue_t` 的
  `{valid, total}` 语义允许 `valid == total == N`（逐帧完整数据），
  任何合规解码器都能读。代价是动画区比官方大 **约 4–6×**
  （整模型量级 2.5–3.5 MB）。这是**体积**问题，不是正确性问题。
- 超过 255 帧的通道会拆成多条 run（`total` 是 `u8`）。
- `seqdesc.bbmin/bbmax` 用顶点 AABB，官方用的是**逐帧蒙皮后的并集**
  （多帧模型精确匹配，1 帧/常量姿态模型有差异，见
  `docs/animation-layout.md` §12 的 U4）。
- 不做 `sectionframes` 分段（实测 `sectionindex = sectionframes = 0`
  在 60+ 样本上均合法）。

### VTX 的已知取舍（不影响正确性）

- 每个 mesh 固定 **1 个 strip group + 1 个 tri-list strip**，不做 strip 优化。
  实测 L4D2 全部是 tri-list（0 个 tri-strip），所以这不会导致错误；
  代价是渲染时顶点缓存命中率略低于官方产物。
- 顶点**不重排**（studiomdl 会按内部哈希重排，实测最小文件的映射是
  `[7,1,0,4,5,3,2,6]`）。重排不影响语义 —— 已用「各自配套的 VVD 解析出
  三角形集合」证明两者几何完全等价。
- **多 LOD 已支持**（结构树 / `switchPoint` / `origMeshVertID` 按实测规则写出），
  但**不做 LOD 的自动生成** —— 需要显式给出每个 LOD 的 SMD。
  单 LOD 的产物与加这个特性之前逐字节相同。
- **`switchPoint` 的缺省值是推算的**（`20 * 2^(n-1)`），不是 studiomdl 的算法 ——
  真实模型的 `switchPoint` 来自 QC 里显式写的距离。写错只影响 LOD 切换距离，
  不影响渲染正确性。实测真实取值分布：`0`（LOD 0 恒为 0）、
  `10/15/20/25/30/40/50/60/65/80/100/150/200`，以及 `-1`（不切换）。

### 一条会影响后续工作的版本事实

本地 `hl2sdk-l4d2\public\studio.h` 声明 **`STUDIO_VERSION 48`**，
但真实 L4D2 模型是 **v49**（官方 `v_autoshotgun.mdl` 实测 `version = 49`）。
两者的结构大小相同，但**若干槽位语义不同，动画编码真正不同**。

**所以做动画层时不要照 `hl2sdk-l4d2` 的头文件写位流** ——
应改用 `hl2sdk-doi`（实测其 `studio.h` 是 `STUDIO_VERSION 49`）。

## 测试

```powershell
cargo test
```

212 个测试，覆盖：TOML 解析与校验（含各类非法输入）、
各结构体偏移与大小的**硬编码断言**（防止实现与测试一起跑偏）、
相对/绝对偏移语义、字符串池规范化、官方 VVD 的逐字节往返、
**切线算法**（轴对齐四边形 / 手性翻转 / 共享顶点累加 / 退化 UV / 孤立顶点）、
以及**多 LOD 与 fixup**（排序不变式、分段铺满、fixup 分组、
单 mesh 不做 fixup、VTX 的 `origMeshVertID` 范围）。

其中一个测试会**扫描真实语料**（`D:\DSH\L4D2ReverseEngineering\mdl-corpus\`，
3302 个 VVD）并逐个做逐字节往返；语料不存在时跳过并打印，不伪造通过。

## 后续阶段（按可验证性排序）

1. **QC 解析 → 同一 IR**（写出器不用改）
2. `studiohdr2` 与 hitbox set —— 差分里最容易补上的两块
3. 序列与动画（`mstudioseqdesc_t` / `mstudioanimdesc_t` / 位流编码）
4. flex / eyeball / mouth / ik / attachment
5. **VTX**（strip 与 LOD 生成是最黑盒的一块）
6. PHY（可先委托 `vphysics` 或外部工具）

### 已知的语义等价差异（不必追）

同一几何经两个编译器，VVD 里顶点的**排列顺序**不同（studiomdl 按它内部的
哈希/去重顺序重排），法线还有约 `1e-7` 的量化误差（`-0.9999998807907104`
vs `-1.0`）。这些不影响渲染 —— 顶点索引表会跟着一起变，两边自洽。
