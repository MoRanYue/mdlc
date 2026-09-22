# QC 命令覆盖缺口报告

> **问题**：「还有没有 studiomdl 支持但 mdlc 没有支持的 QC 参数？」
>
> **答：有，但真正值得做的只有 3 项；3 项现已全部实现。**
> 以 **L4D2 `studiomdl.exe` 自己的分发表**为权威基准（**137** 条命令），
> 逐条对照 mdlc 的 TOML 能力后：
>
> | 分类 | 数量 | 处置 |
> |---|---|---|
> | **真实缺口** | **3** | 序列级 `iklock`、`physicsbone`、`$maxeyedeflection` —— ✅ **均已实现并验收**（见 §7） |
> | 无具名键但**可用 `extra_flags` 表达** | 7 | 标志位类，非功能缺口 |
> | 判据受限 / 语料 0 次 ⟹ **不必做** | 8 | `$autocenter`、`$scale`/`$origin`/`$upaxis`、`$screenalign` … |
> | **不适用**（QC 文本组织 / QC 预处理） | 13 | 属既定范围外 |
>
> ⚠️ **注意占比的分母**：序列级 `iklock` 是 **14.27% 的序列**、
> 但只有 **0.48% 的模型**（差 30 倍，见 §2.1）。
>
> 数据来源与判据脚本全部可复现，见 §6。
>
> ---
>
> ## ⚠️ 后续追加：§8 是**更严重**的一批发现
>
> 用户随后拿来一份**真实第三方项目的 QC** 问「mdlc 能编吗」。
> 端到端对照真 `studiomdl.exe` 后，查出 **9 个既有 bug**，
> 其中 **4 个影响全部 3333 个语料模型**，还有一个让
> **所有 `.phy` 碰撞体的几何大了 39.37 倍**（§8.7）。
>
> **这批 bug 不是「QC 覆盖缺口」** —— 它们是已声称支持的路径上的实现错误，
> 与「支持哪些 QC 命令」无关。但它们是本轮最有价值的产出，
> 所以一并记在本报告里。
>
> 一句话结论：**86 个自造 parity 用例全绿，却漏掉了 9 个 bug** ——
> 自造用例验证「规则成立」，验证不了「规则叠加」（教训 8）。

---

## 0. 为什么不能用 VDC 文档当基准

| 来源 | 数量 | 性质 |
|---|---|---|
| VDC `Category:Source_base_QC_commands` | 101 | **文档**，且自我声明「far from complete」 |
| **L4D2 `studiomdl.exe` 分发表** | **137** | **二进制事实**，本报告的权威基准 |
| mdlc TOML 字段 | 235 | 实现现状 |

**三方差集**（`probe_qc_three_way.js`）：

* **VDC 有、L4D2 没有（7 条）**：`$boneflexdriver` `$checkuv`
  `$defaultfadein` `$defaultfadeout` `$internalname` `$lcaseallsequences`
  `$maxverts` ⟹ **不适用，mdlc 不做是对的**。
* **L4D2 有、VDC 没列（43 条）**：全部是 `$collisionjoints` 子关键字与
  物理/材质命令（`$concave` `$weldnormal` `$polysoup` …）。
  ⟹ **只看文档会漏掉 43 条**，其中就有 `$concave`（mdlc 已实现）。

> **教训**：查「studiomdl 支持什么」必须看**二进制分发表**，不能看 VDC ——
> 后者漏了 43 条、多了 7 条。

---

## 1. 真实缺口总表（按影响面排序）

「影响面」= 语料 3333 个真实 `.mdl` 里**能观测到该字段非零**的比例。
判据一律是**语料实测**，不是推测。

| # | 缺口 | 影响面 | 性质 | 优先级 |
|---|---|---|---|---|
| **1** | **序列级 `iklock`**（`seqdesc.numiklocks` @0xA4） | **14.27% 的序列**（1594/11170）／ **0.48% 的模型**（16/3333） | **结构性**（子表布局） | ✅ **已实现**（§7.1） |
| **2** | **`physicsbone`**（`mstudiobone_t` +0xAC） | **1.14%**（38/3333 ragdoll；表面值 15.33%） | 字段值 | ✅ **已实现**（§7.2） |
| 3 | `$maxeyedeflection`（`studiohdr2` +0x0C） | 0.12%（4 个 survivor） | 单字段 | ✅ **已实现**（§7.3） |
| 4 | `$screenalign`（`BONE_SCREEN_ALIGN_SPHERE`） | **0%**（0/15757 骨骼） | — | 低（可不做） |
| 5 | `$autocenter`（`g_centerstaticprop`） | **判据不确定**（见 §3.3） | 几何居中 | 低 |
| 6 | `$scale` / `$origin` / `$upaxis` | **无法从 .mdl 判定**（见 §3.4） | 全局几何变换 | 低 |
| 7 | `$opaque` / `$mostlyopaque` / `$noforcedfade` / `$casttextureshadows` / `$ambientboost` / `$donotcastshadows` / `$forcephonemecrossfade` | 1.65% / 3.90% / **5.70%** / 4.62% / 0.39% / 0.27% / 0.12% | **可由 `extra_flags` 表达** | 低（已有绕过） |
| 8 | `$shadowlod` / `$minlod` / `$allowrootlods` / `$skinnedLODs` | 0.15% / 0% / — / — | LOD 控制 | 低 |
| 9 | `$collapsebones` / `$alwayscollapse` / `$forcerealign` | 判据未建 | 骨骼折叠 | 低 |
| 10 | `$renamematerial` / `$externaltextures` / `$cliptotextures` / `$gamma` / `$hgroup` / `$decal` / `$ignorez` / `$vertexcolor` | 判据未建 | 材质管线 | 低 |
| 11 | `$append` / `$prepend` / `$continue` / `$declaresequence` / `$declareanimation` / `$calctransitions` / `$skiptransition` | — | QC 文本组织 | **不适用**（TOML 无「上一个序列」概念） |
| 12 | `$definemacro` / `$definevariable` / `$include` / `$pushd` / `$popd` / `$cmdlist` | — | QC 预处理 | **不适用**（属 QC 解析，既定范围外） |
| 13 | `$insertbone` / `$limitrotation` / `$lockbonelengths` / `$unlockdefinebones` / `$bonesaveframe` / `$renamebone` / `$hierarchy` | 判据未建 | 骨骼编辑 | 低 |
| 14 | `$jointmerge` / `$jointskip` / `$noselfcollisions` / `$automass` / `$masscenter` 等 | 见 PROGRESS §26 | PHY 细节 | 中（已知） |

> **「判据未建」的含义**：这些命令的产物痕迹**尚未设计出可靠判据**
> （例如 `$collapsebones` 会**减少骨骼数**，但骨骼少也可能是 SMD 本来如此，
> 无法从 `.mdl` 单独归因）。**未测 ≠ 影响面为 0** —— 本报告不替它们下结论。


---

## 2. 两个高影响缺口（详细）

### 2.1 序列级 `iklock` —— **14.27%，且是结构性缺口**

**mdlc 现状**（`src/anim_writer.rs:3099`）：

```rust
// ---- ⑥ iklocks（`mstudioiklock_t` = 32 字节）----
let iklock_off_in_sub = seq_subtables.len();
iklock_offsets.push(iklock_off_in_sub);
// （mdlc 不产出 iklock 记录，所以这里长度恒 0。）
```

`src/anim_writer.rs:3262-3263` 也硬写 `numiklocks = 0`。

**官方依据**：

* QC 侧关键字（`studiomdl.cpp:2873-2885`）：
  `iklock <链名> <flPosWeight> <flLocalQWeight>` —— **`$sequence` 块内**，
  与 `$ikautoplaylock`（模型级）是**两个不同的东西**。
* 写出（`write.cpp:596-611`）：
  `pseqdesc->numiklocks = IsChar(g_sequence[i].numiklocks);`
  `pseqdesc->iklockindex = IsInt24(pData - pSequenceStart);` → 之后 `ALIGN4`。
  ⚠️ **`pSequenceStart` 是「本记录自身」** —— `write.cpp:429` 在循环**内**
  赋值 `byte *pSequenceStart = (byte *)pseqdesc;`，
  而 `pData` 是**跨全部序列的全局游标**。
  所以 `iklockindex` **相对本记录**（与 `eventindex` 的推导同型，
  见 §25 bug 2：`eventindex = (seq_count − si) * 212 + 表内偏移`）。

**语料实测**（`probe_iklock_seq_corpus.js` + `probe_iklock_base.js` + `probe_iklock_values.js`）：

```text
序列总数            : 11170
numiklocks != 0     : 1594  (14.27%)   ← 分布：2 条 ×1575、4 条 ×15、3 条 ×4
iklock 记录总数     : 3222
涉及模型            : 16 / 3333 (0.48%)
chain == -1 / 越界  : 0 / 0            ← 全部合法
flPosWeight         : 1 ×3201、0 ×17、0.8 ×2、0.75、0.5
flLocalQWeight      : 0 ×3205、1 ×17
```

> **两个分母都要看**：**14.27% 是「序列占比」**（1594/11170），
> 而**模型占比只有 0.48%**（16/3333）—— 因为带 `iklock` 的模型
> （`anim_boomer`/`anim_hunter`/survivor 等）**序列特别多**。
> 谈「影响面」时两个数字都要给，否则会误导优先级判断。

**基准已钉死 = 相对本记录自身**（与源码 `write.cpp:429/601` 一致）——
两种假设对比（`probe_iklock_base.js`）：

| 假设 | 合法记录 | 非法 | NaN |
|---|---|---|---|
| 相对 **seqdesc 数组起点** | 1787 | 1435 | 384 |
| **相对本记录自身** ← 源码确认 | **3222** | **0** | **0** |

判据 = 「chain 必须是合法链下标 + 权重必须是有限浮点」。
**语料反解与源码阅读在此互相印证**（源码是先读出来的，语料是独立验证）。

> ⚠️ **为什么这是「结构性」而非「一个字段」**：
> `iklock` 记录**占子表区空间**。mdlc 不写它们 ⟹ 其后的
> **blend 数组 / keyvalue 块的偏移整体前移**，且 `numiklocks` 为 0
> 会让引擎不去读这些锁。影响的是**子表布局**，不只是单个字段。

**建议 TOML 形态**（与既有 `auto_layers` 对称）：

```toml
[[sequences]]
name = "Boomer_AimMatrix_Idle_Standing"
# …

[[sequences.iklocks]]
chain = "rhand"          # 链名，落盘解析成下标（与 ik_autoplay_locks 同惯例）
pos_weight = 1.0         # 缺省 1.0（语料 3201/3222）
local_q_weight = 0.0     # 缺省 0.0
```

### 2.2 `physicsbone` —— **15.33%**

**mdlc 现状**：`src/mdl_writer.rs:447` 定义了
`pub const PHYSICS_BONE: usize = 0xAC;`，但**全代码库没有任何地方写它** ——
该字段恒为 `0`。

**官方依据**（`collisionmodel.cpp:2141-2184`，`write.cpp:195`）：

```c
// ① 先全置 -1
for (i = 0; i < g_numbones; i++) g_bonetable[i].physicsBoneIndex = -1;
// ② 碰撞列表里每个 solid → 该骨骼的 physicsBoneIndex = **solid 下标**
while (pPhys) { ... g_bonetable[boneIndex].physicsBoneIndex = index; ... index++; }
// ③ 未置位的骨骼沿父链上溯找第一个已置位的祖先
// ④ 都找不到 → 0
```

⟹ **语义 = 「该骨骼受哪个 solid 的物理模拟驱动」**，只在**有碰撞模型**时有意义。

**语料实测**（`probe_physicsbone_precise.js`）——必须排除两种平凡情形才看得清：

| 形态 | 全部 | 有 `.phy` | 多骨骼 & 有 `.phy` | 多骨骼 & 无 `.phy` |
|---|---|---|---|---|
| **全 0**（= mdlc 当前输出） | 3295 | 2460 | 180 | 299 |
| **恒等** `pb[i]==i` | 6 | 6 | 6 | 0 |
| **其它**（真正需要推导） | **32** | **32** | **32** | 0 |

**关键结论**：

* `identity` / `other` 形态 **38 个模型全部有 `.phy`**（`无 .phy` 列全 0）
  ⟹ 该字段**确实由碰撞模型驱动**，与源码一致。
* 但 **`allZero` 里有 2460 个有 `.phy`** —— 说明**大多数有 `.phy` 的模型
  该字段仍是全 0**。这不是矛盾：`physicsbone` 只在
  `$collisionjoints`（ragdoll，多 solid）时才被真正赋值；
  单 solid 的 `$collisionmodel` 走的是另一条路径
  （`ProcessSingleBody`，不填 `physicsBoneIndex`）。
* 代表样本：`boomer.mdl`（28 骨骼）`[0,14,15,16,16,11,12,13,13,0,1,2,…]`、
  `destruction_tanker_front.mdl` `[0,4,5,6,9,8,7,10,11,12,12,12,…]`。

⟹ **正确的影响面是 38/3333 = 1.14%**（ragdoll 模型），
而非表面上的 15.33%。mdlc 的 PHY ragdoll 已实现（PROGRESS §24.3），
所以**只需把已有的 solid 分组结果写进 `+0xAC`**，成本很低。

> **判据教训**：`physicsbone != self` 这个「直觉判据」把
> 「多骨骼 + 全 0」误算成非平凡（`bone[i] != i`），
> 得到 511 的虚高数字。**必须按「全 0 / 恒等 / 其它」三分**才看得准。

---

## 3. 头部标志位类（3 项真实使用，但有绕过方案）

### 3.1 `extra_flags` 已能表达全部标志位命令

mdlc 有 `[model].extra_flags`（i32 位掩码），而下列命令**只置一个 flags 位**，
所以**功能上已可表达**，只是没有具名键：

| 命令 | 位 | 语料命中 | 等价的 `extra_flags` |
|---|---|---|---|
| `$opaque` | `FORCE_OPAQUE` 0x4 | **55（1.65%）** | `extra_flags = 4` |
| `$mostlyopaque` | `TRANSLUCENT_TWOPASS` 0x8 | **130（3.90%）** | `extra_flags = 8` |
| `$noforcedfade` | `NO_FORCED_FADE` 0x800 | **190（5.70%）** | `extra_flags = 2048` |
| `$forcephonemecrossfade` | `FORCE_PHONEME_CROSSFADE` 0x1000 | **4（0.12%）** | `extra_flags = 4096` |
| `$ambientboost` | `AMBIENT_BOOST` 0x10000 | **13（0.39%）** | `extra_flags = 65536` |
| `$donotcastshadows` | `DO_NOT_CAST_SHADOWS` 0x20000 | **9（0.27%）** | `extra_flags = 131072` |
| `$casttextureshadows` | `CAST_TEXTURE_SHADOWS` 0x40000 | **154（4.62%）** | `extra_flags = 262144` |
| `$shadowlod` | `HASSHADOWLOD` 0x40 + 0x100 | **5（0.15%）** | 需两个位 |
| `$subd` | `SUBDIVISION_SURFACE` 0x80000 | **0** | — |
| `$obsolete` | `OBSOLETE` 0x200 | **0** | — |
| `$constantdirectionallight` | `CONSTANT_DIRECTIONAL_LIGHT_DOT` 0x2000 | **0** | — |

> **建议**：若要提升易用性，可为高频项（`$noforcedfade` 5.7%、
> `$casttextureshadows` 4.6%、`$mostlyopaque` 3.9%）加**具名布尔键**，
> 但**不是功能缺口**。

### 3.2 `$maxeyedeflection` —— 0.12%，mdlc 硬编码 0

`src/mdl_writer.rs:2050-2054` 注释「`flMaxEyeDeflection` 写 `0`
（引擎在读到 0 时回退到 `cos(30°)`）」，代码写 `0.0`。

**语料**：4 个 survivor（`coach`/`gambler`/`mechanic`/`producer`）
写的是 `0.8660253882408142` = **`cos(30°)`** —— 与引擎缺省值**数值相同**，
所以**当前行为在渲染上等价**，只是字节不同。

⟹ **优先级低**（不影响画面），但若要逐字节对齐需补。

### 3.3 `$autocenter` —— 判据被「几何对称」污染，**未能定论**

`Cmd_Autocenter`（`studiomdl.cpp:906-909`）只做一件事：
`g_centerstaticprop = true`。默认 **false**（`studiomdl.cpp:6900`），
mdlc **不实现居中**（HANDBOOK 第 3 节明确「不要实现居中」）。

**试图用语料验证**（`probe_autocenter_corpus.js`）：对 2681 个静态道具
检查包围盒中心是否≈原点 —— 结果 **48 居中 / 2633 偏离**。

**但这条判据不成立**：48 个「居中」样本全部是
`metal_gib*` / `wood_gib*` / `wood_pallet_debris_*` ——
**本身左右对称**的碎块（`hull_min[i] == -hull_max[i]` 逐位对称）。
对称几何的包围盒中心**天然就是原点**，无论有没有 `$autocenter`。

⟹ **无法从包围盒中心区分「居中」与「几何本身对称」**。
要定论需要一个**不对称**的静态道具样本，且知道它是否写了 `$autocenter`
（语料里没有 QC 源码，做不到）。

**结论：保持现状（不居中）**。理由：
① 官方默认就是 false；② 没有反例证据表明 L4D2 用了 `$autocenter`；
③ 贸然实现居中会**破坏** 2681 个静态道具中绝大多数（它们明显未居中）。

### 3.4 `$scale` / `$origin` / `$upaxis` —— **无法从 `.mdl` 判定**

这三个命令作用于**源几何的全局变换**，且变换结果已被**烘焙**进顶点与骨骼
坐标 —— 产物里**没有任何残留字段**，因此**不能用语料反查**。

* `$scale <f>` → `g_defaultscale`：整体缩放，无残留。
* `$upaxis <轴>` → `g_defaultrotation`：源 up 轴旋转，无残留。
  （注意：默认 `g_defaultrotation = Rz(90°)` **是**有痕迹的 ——
  见 HANDBOOK 第 6 节「根骨骼要套一层 Rz(90°)」；但**额外**的 `$upaxis`
  会**覆盖**它，无法区分。）
* `$origin x y z` → `g_defaultadjust`：整体平移。**唯一有间接痕迹**的一个。

**间接判据**（`probe_scale_origin_corpus.js`）：根骨骼 `pos != 0`
是 `$origin` 的**必要不充分**条件。

实测 **210/3333（6.30%）** 根骨骼 `pos != 0`，但逐样本看**全部**是
SMD 本身骨骼就不在原点的情形：

```text
baked_left_bot_glass_5_Primary544  [-187.26, 19.77, 142.51]   ← 烘焙道具，骨骼在几何上
baked_plywoodbot_surf_obj114       [-395.81, 32.62,  21.49]
chTURN                             [6345.80,-398.41,3664.25]   ← 直升机，世界坐标
Bip01_Pelvis                       [0.00, 38.03, -0.26]       ← 角色骨盆，SMD 原生
```

⟹ **没有证据表明 L4D2 语料用了 `$origin`**；`$scale`/`$upaxis` 更无从判定。

**结论**：三项都**保持不实现**。若将来遇到明确的 QC 项目需要，
再按 `studiomdl.cpp:1392-1466` 的语义补（`$origin` 平移所有骨骼与顶点、
`$scale` 缩放、`$upaxis` 换基）—— 三者的实现点都在**解析 SMD 之后、
烘焙之前**，与 mdlc 的 `smd_vertex_to_ir` 位置对应。

---

## 4. 明确「不适用」的（不要做）

| 类别 | 命令 | 理由 |
|---|---|---|
| **QC 文本组织** | `$append` `$prepend` `$continue` `$declaresequence` `$declareanimation` `$calctransitions` `$skiptransition` | 依赖「上一个序列」的**文本顺序**概念，TOML 是声明式描述，无对应语义 |
| **QC 预处理** | `$definemacro` `$definevariable` `$include` `$pushd` `$popd` `$cmdlist` | 属 **QC 解析**，项目既定范围外（输入是 TOML） |
| **L4D2 不存在** | `$boneflexdriver` `$checkuv` `$defaultfadein` `$defaultfadeout` `$internalname` `$lcaseallsequences` `$maxverts` | VDC 有、**L4D2 二进制没有** |
| **语料 0 次** | `$screenalign`（`BONE_SCREEN_ALIGN_SPHERE` 0/15757）、`$subd`、`$obsolete`、`$constantdirectionallight` | 实测 0 命中，做了也无法验收 |

---

## 5. 建议的落地顺序

1. **序列级 `iklock`**（14.27%，结构性）—— 影响子表布局，优先级最高。
   需要：`Sequence.iklocks` 字段 + `anim_writer` 子表区写记录 +
   `numiklocks`/`iklockindex` 回填 + parity 用例（`anim_boomer` 有 4 条锁的序列）。
2. **`physicsbone`**（1.14% 真实 / ragdoll）—— 复用已有 solid 分组结果写 `+0xAC`。
3. **`$maxeyedeflection`**（0.12%）—— 单字段，成本极低。
4. 高频标志位加具名键（可选，纯易用性）。
5. 其余按需。

---

## 6. 复现方式

```powershell
cd D:\GITHUB\mdlc

# ① 三方命令清单比对（VDC vs 二进制 vs mdlc）
node docs\_probe\probe_qc_three_way.js

# ② 头部 flags 位普查（哪些位真的被用到）
node docs\_probe\probe_header_flags_corpus.js

# ③ 硬编码字段普查（哪些字段真的非零）
node docs\_probe\probe_hardcoded_fields_corpus.js

# ④ physicsbone 三分普查（区分全0/恒等/其它）
node docs\_probe\probe_physicsbone_precise.js

# ⑤ 序列级 iklock（基准 + 取值）
node docs\_probe\probe_iklock_base.js      # 判定 iklockindex 基准
node docs\_probe\probe_iklock_values.js    # 解码真值

# ⑥ seqdesc 未建模字段全扫
node docs\_probe\probe_seqdesc_fields_corpus.js
node docs\_probe\probe_seqdesc_rest.js

# ⑦ 骨骼 flags 高位普查
node docs\_probe\probe_screenalign_autocenter.js

# ⑧ 三个"无残留字段"的命令（判据受限，结论见 §3.3/§3.4）
node docs\_probe\probe_autocenter_corpus.js
node docs\_probe\probe_scale_origin_corpus.js
```

**权威命令表**（L4D2 二进制提取，本次复用）：
`D:\DSH\L4D2ReverseEngineering\tmp-qcscan\dispatch_table.tsv`（104 条）
+ `tier2_collisionjoints.txt`（27 条）+ `tier3_extra.txt`（6 条）= **137 条**。

**语料**：`D:\DSH\L4D2ReverseEngineering\mdl-corpus`（3333 `.mdl` / 15757 骨骼 /
11170 序列）。

---

## 7. 两项缺口的实现与验收（✅ 已完成）

### 7.1 序列级 `iklock` —— 已实现

**改动**（4 个文件）：

| 文件 | 内容 |
|---|---|
| `src/model.rs` | `Sequence.iklocks: Vec<IkAutoplayLock>`（复用既有类型，与 `ik_autoplay_locks` 同形）；`CompiledSequence.iklocks` 透传 |
| `src/compile.rs` | 两处构造点透传 `s.iklocks.clone()` |
| `src/anim_writer.rs` | 子表区 ⑥ 写 `mstudioiklock_t`（32 B/条，只写前 3 个字段）；`numiklocks` @0xA4 写**条数**；链名→下标解析（找不到链名**报错**）；新增 `IK_LOCK_SIZE` 常量 |
| `src/mdl_writer.rs` | 无改动（`numiklocks`/`iklockindex` 由 `anim_writer` 写） |

**TOML**：

```toml
[[sequences]]
name = "idle"
smd = "idle.smd"

[[sequences.iklocks]]
chain = "leg"            # 链名（落盘解析成下标，与 ik_autoplay_locks 同惯例）
pos_weight = 1.0         # 缺省 1.0（语料 3201/3222）
local_q_weight = 0.0     # 缺省 0.0
```

**受控实验**（新建 `docs/_probe/smdl/ikl{1,2}.qc`，真实 `studiomdl.exe`）：

| 用例 | QC | 官方产物 | 结果 |
|---|---|---|---|
| `parity/iklock-basic.toml` | `ikl1`（1 链 1 锁） | `numiklocks=1`, `iklockindex=228`, `{0, 1.0, 0.0}` | **16/16，0 差异** |
| `parity/iklock-two.toml` | `ikl2`（2 链 2 锁，非平凡权重） | `numiklocks=2`, `{0,1.0,0.1}` + `{1,0.5,0.25}` | **24/24，0 差异** |

**判据脚本**：`docs/_probe/cmp_iklock.js`（4 层：字段 / 逐记录 8 个 dword /
结构自洽 / 与 keyvalue 的先后）。

**更强的证据 —— iklock 区逐字节相同**：

```text
ikl1 iklock 区 32 字节: ✅ 逐字节相同
   mdlc    : 000000000000803f0000000000000000...
   official: 000000000000803f0000000000000000...
ikl2 iklock 区 64 字节: ✅ 逐字节相同
```

**回归**：`ikrule-touch`（无 iklock）的 `iklockindex` 仍为 **228**、与官方相同
—— 证明空路径未受影响。

**单元测试 +2**：`sequence_iklocks_are_written_as_32_byte_records`、
`sequence_iklock_with_unknown_chain_errors`。**两条都已反向证伪**
（把 `numiklocks` 改回常量 0 后立刻报 `left: 0, right: 2`）。

### 7.2 `physicsbone` —— 已实现

**改动**（4 个文件）：

| 文件 | 内容 |
|---|---|
| `src/phy.rs` | 新增 `physics_bone_table(smd, bone_count, bone_parents)`：**复刻 ragdoll 的同一套过滤**（顶点 < 4 或张不成凸包则跳过，不占 solid 下标），再做「② solid 下标 → 骨骼、③ 沿父链上溯、④ 兜底 0」 |
| `src/model.rs` | `CompiledModelDesc.physics_bone: Option<Vec<i32>>` |
| `src/mdl_writer.rs` | 骨骼循环里写 `+0xAC`（`None` 时**不写**，保持 0） |
| `src/main.rs` | **把碰撞 SMD 的解析提前到 `write_mdl` 之前**（`physicsbone` 要进骨骼表），并复用给 `.phy` 构造 —— 避免读两次 |

**⚠️ 关键设计点：分组口径必须与 ragdoll 完全一致。**
官方对「顶点数不足」或「张不成凸包」的骨骼**直接跳过**，被跳过的骨骼
**不占 solid 下标**。所以 `physics_bone_table` 复用了
`build_ragdoll_phy_from_smd` 的同一套过滤，否则下标会错位。

**受控实验**：

| 用例 | 官方产物 | 结果 |
|---|---|---|
| `parity/phy-ragdoll.toml`（`rjd1`，3 骨骼各一盒子） | `physicsbone = [0,1,2]` | **7/7，0 差异** |
| `parity/phy-ragdoll-uplevel.toml`（`rjd2`，**中间骨骼无几何**） | `physicsbone = [0,0,1]` | **7/7，0 差异** |

> `rjd2` 是关键用例：它同时覆盖「沿父链上溯」（`bone_mid` → `bone_root`）
> 与「solid 下标不跳号」（`bone_tip` 拿到 **1** 而不是 2）。

**判据脚本**：`docs/_probe/cmp_physicsbone.js`（3 层：逐骨骼值 /
值域 `< solidCount` / 非平凡性一致）。

**回归范围精确**：84 个 parity 产物里只有 **2 个**（`rjd1`/`rjd2`）
的 `physicsbone` 非 0，其余 **74 个全 0**（未被触碰）。

**单元测试 +2**：`physics_bone_maps_solids_and_walks_up_parents`、
`physics_bone_is_all_zero_for_single_solid`。
**已反向证伪**（去掉写入后 `cmp_physicsbone.js` 报 3 处差异）。

### 7.3 全量回归（本轮实测）

| 项 | 结果 |
|---|---|
| `cargo test --release` | **325 passed / 0 failed**（+4） |
| `cargo clippy --all-targets` | **零警告** |
| `cargo build --release` | 干净 |
| `parity_snapshot.js` | **84/84 编译成功**（+2 新用例） |
| `verify_linearbone.js` | **76 个产物 0 违反** |
| `cmp_ikrule.js`（9 用例） | **合计 0 差异**（无回归） |
| `cmp_ragdoll.js` | **2/2 通过** |
| VVD 往返语料 | **3302/3302 逐字节相同** |

### 7.3 `$maxeyedeflection` —— 已实现

**语义由反汇编确证**（handler `0x00450270`，全部有效指令只有 4 条）：

```asm
0x004502a3  call 0x5b6796          ; atof(token)
0x004502a8  fmul qword [0x9764a0]  ; × π
0x004502b1  fmul qword [0x97c988]  ; × (1/180)
0x004502b7  fcos                   ; cos(...)
0x004502b9  fstp dword [0x14a7708] ; → g_flMaxEyeDeflection（**f32**）
```

两个常量实测为 **π** 与 **1/180** ⟹

```text
落盘值 = cos(deg2rad(输入的度数))
```

**判据（逐位）**：`$maxeyedeflection 30` → f32 `0.8660253882408142`，
与语料 4 个 survivor 的 `flMaxEyeDeflection` **逐位相同**。

**改动**（2 个文件）：

| 文件 | 内容 |
|---|---|
| `src/model.rs` | `ModelMeta.max_eye_deflection: Option<f32>`（TOML 收**度**） |
| `src/mdl_writer.rs` | `studiohdr2 +0x0C` 写 `deg.to_radians().cos()`；`None` → `0.0` |

**TOML**：

```toml
[model]
max_eye_deflection = 30.0    # **度**；落盘 cos(deg2rad(30))
```

**为什么 TOML 收「度」**：与 `jiggle_bones` / `quat_interp_bones` 同惯例 ——
那两处也是「TOML 写 QC 的原始角度、落盘时转换」。
让用户写 `30` 比写 `0.8660254` 更贴近 QC 且不易写错。

**受控实验**（新建 `docs/_probe/smdl/med{1,2}.qc`）：

| 用例 | QC | 官方产物 | 结果 |
|---|---|---|---|
| `parity/maxeyedeflection.toml` | `med1`（`30`） | `0.8660253882408142` | **3/3，逐位相同** |
| `parity/maxeyedeflection-45.toml` | `med2`（`45`） | `0.7071067690849304` | **3/3，逐位相同** |

> **`med2`（45°）是关键用例**：它能一刀切开三种假设 ——
> 「原样落盘」得 `45.0`、「恒 `cos(30°)`」得 `0.866…`，
> 只有真的做 `cos(deg2rad(x))` 才得 `0.707…`。
> **30° 单独用不行** —— 它恰好是引擎缺省值，区分不了「转换」与「硬编码」。

**判据脚本**：`docs/_probe/cmp_maxeyedeflection.js`
（3 层：**逐位**相同 / 公式自证 / 未写必须为 0）。

> ⚠️ 判据用**逐位**而不是容差 —— 官方是 f64 算完截成 f32，
> 容差比较会放过「公式写错但数值接近」的实现。

**缺省 = 不写（保持 0）**：`g_flMaxEyeDeflection` 在 `.bss`，
官方初值 **0**，引擎读到 0 才回退 `cos(30°)`（`studio.h:2173`）。
所以「不写」与「写 30」**渲染等价、仅字节不同** ——
语料 3333 个模型里只有 **4 个**（survivor）显式写了它。

**单元测试 +1**：`max_eye_deflection_is_cosine_of_degrees`
（含「必须真的做了 cos 转换」的反向断言）。**已反向证伪**
（改成原样落盘后立刻报 `left: 30.0, right: 0.8660254`）。

### 7.4 全量回归（本轮实测）

| 项 | 结果 |
|---|---|
| `cargo test --release` | **326 passed / 0 failed**（+1） |
| `cargo clippy --all-targets` | **零警告** |
| `cargo build --release` | 干净 |
| `parity_snapshot.js` | **86/86 编译成功**（+2 新用例） |
| `verify_linearbone.js` | **78 个产物 0 违反** |
| `cmp_ikrule.js`（9 用例） | **合计 0 差异**（无回归） |
| `cmp_ragdoll.js` | **2/2 通过** |
| VVD 往返语料 | **3302/3302 逐字节相同** |
| `cmp_iklock.js` | `ikl1` **16/0**、`ikl2` **24/0**（iklock 区逐字节相同） |
| `cmp_physicsbone.js` | `rjd1` **7/0**、`rjd2` **7/0** |
| `cmp_maxeyedeflection.js` | `med1` **3/0**、`med2` **3/0**（逐位） |

**回归范围精确**：86 个 parity 产物里 `max_eye_deflection` 非 0 的
只有 **2 个**（`med1`/`med2`），其余 **76 个全 0**（未被触碰）。

### 7.5 本节新增的方法论教训

1. **`pSequenceStart` 的基准要读赋值点，不能读名字。**
   我按名字直觉假设它是「seqdesc 数组起点」，解出 **384 个 NaN** 才发现错；
   回看 `write.cpp:429` —— 它在**循环内**赋值
   `byte *pSequenceStart = (byte *)pseqdesc;`，是**本记录自身**。
   语料反解（**3222/3222 合法**）与源码阅读**互相印证**。
   > 而同一个变量名在 `eventindex` 上基准恰好**相反**
   > （`write.cpp:490` 的语义是「相对数组起点」，故有
   > `(seq_count − si) * 212` 那一项）。**同名变量在相邻字段上可以是两种基准。**

2. **「复刻一个已有算法」时要复刻它的过滤，不只是主循环。**
   `physicsbone` 的下标必须与 `.phy` 的 solid 序号一致，而官方会**跳过**
   顶点不足/张不成凸包的骨骼。只抄「solid → 骨骼」的映射而漏掉过滤，
   下标就会错位 —— 且**不会报错**，只是值全偏。

3. **反证要真跑。** 两条新测试都**故意改回旧实现**验证过：
   `numiklocks` 改常量 0 → 报 `left: 0, right: 2`；
   去掉 `physicsbone` 写入 → `cmp_physicsbone.js` 报 3 处差异。
   **一个永远不会失败的测试等于没有测试。**

4. **重构 `main.rs` 的求值顺序时，要先问「谁依赖谁」。**
   `physicsbone` 要进骨骼表（`write_mdl` 产出），而它来自碰撞 SMD ——
   于是碰撞 SMD 的解析必须**提前**到 `write_mdl` 之前。
   顺手把解析结果复用给 `.phy` 构造，避免读两次。

5. **「与缺省值同值」的参数要用别的角度验证。**
   `$maxeyedeflection 30` 的落盘值恰好**等于**引擎缺省 `cos(30°)` ——
   单用它**区分不了**「真做了 `cos(deg2rad(x))`」与「硬编码 `cos30°`」。
   换成 **45°**（`0.7071…`）才一刀切开。
   **造用例时要避开「与某个常量恰好重合」的输入** —— 这与
   §29.3 的「参考姿态为 0 会让符号错误隐身」是同一类陷阱。

6. **「逐位比较」在浮点字段上是必要的判据强度。**
   官方是 `f64` 算完 `fstp dword`（截成 f32），而容差比较会放过
   「公式写错但数值接近」的实现。`cmp_maxeyedeflection.js` 因此
   比**十六进制位模式**，而不是比数值差。

7. **改完源码后要确认真的重编译了。**
   我用 `Copy-Item` 还原备份时**保留了原 mtime**，
   cargo 认为文件没变、**跳过重建**，于是测试跑的是旧的二进制，
   报出一个已经修好的失败。**用 `(Get-Item f).LastWriteTime = Get-Date`
   强制刷新 mtime** 才暴露真相。
   > 症状与「代码没改对」完全一样 —— **先确认构建新鲜度，再怀疑代码**。

---

## 8. 用真实第三方 QC 做端到端对照 —— 查出 8 个既有 bug

> **起因**：用户拿来一份**真实第三方项目的 QC**
> （`w_shotgun_spas.qc`，794 字节）问「mdlc 现在能编吗」。
>
> **答**：能编，但**逐字段对照真 `studiomdl.exe` 时有 11 处差异** ——
> 查下去发现**全是 mdlc 的 bug**，其中一个影响了**全部 3333 个语料模型**。
>
> 修完后：**MDL 逐字段完全一致（0 处差异），文件长度 3708 字节与官方相同**。

### 8.1 这份 QC 为什么特别有价值

它小（794 字节）、用了 mdlc 当时**已声称支持**的全部特性
（`$attachment` / `$bbox` / `$keyvalues` / `$collisionmodel` / `$bodygroup` /
`$sequence` / `$cdmaterials`），而且是**别人写的** —— 不是为本项目量身定制的
受控实验。既有 parity 用例（`parity/*.toml`，86 个）**全部通过**，
却漏掉了这 8 个 bug。

**原因**：那些用例是「按已知规律造出来验证已知规律」的，
每个用例只覆盖 1~2 个特性；这份 QC **一次用了 7 个特性**，
于是把「各自单独正确、组合起来错位」的问题暴露出来。

> **教训**：受控实验能验证**规则的成立**，验证不了**规则之间的相互作用**。
> 必须有一份「不是我写的、我也不知道它会踩到什么」的真实输入当**冒烟测试**。

### 8.2 八个 bug（按发现顺序）

| # | 位置 | 症状 | 语料判据 | 影响面 |
|---|---|---|---|---|
| 1 | `mdl_writer` keyvalues 前缀 | 多写一个前导 `"`（54 vs 官方 53 字节） | **735/735** 个有 keyvalues 的模型首字节**都不是** `"` | 22.1% 的模型 |
| 2 | `mdl_writer` `szanimblocknameindex` | 无动画块时写 **0**，官方写**指向空串的偏移** | **3212/3212** 指向空串，**0 个**写 0 | 96.4% 的模型 |
| 3 | `layout` `localnode`/`localnodename` | 排在 `bodypart` **之后**，官方在**之前** | 28/28 个非空模型 `localnodeindex < bodypartindex` | **全部** 3333 个 |
| 4 | `layout` `srcbonetransformindex` | 空段时写**未对齐**的 `kv_end` | **3262/3262** 等于 `ALIGN4(kv_end)`，0 个等于 `kv_end` | 97.9% 的模型 |
| 5 | `mdl_writer` 字符串池首字节 | 池首**没写** NUL（空串占位） | **3333/3333** 池首字节为 0 | **全部** |
| 6 | `mdl_writer` `seqdesc.label` | 复用了 animdesc 的 `@name` 串（`+1` 跳过 `@`） | 11170 条序列的 label **无一**以 `@` 开头 | **全部**序列 |
| 7 | `layout` 文件末尾 | 字符串池后**漏了** `ALIGN4` | **3333/3333** 文件长度是 4 的倍数 | **全部** |
| 8 | `compile` `$definebone` 骨骼 | SMD 里没有的骨骼**不建**（直接报错） | 官方 `BuildGlobalBonetable` 先插 `$definebone` | 见 §8.4 |

**#3 + #5 + #6 + #7 影响全部 3333 个模型** —— 也就是说
**在修之前，mdlc 的每一个产物都与官方有结构差异**，只是
`verify_parity.ps1` 的判据把「段偏移不同」当成「结构性偏移」**放过**了。

> **教训（最重要的一条）**：
> 「结构性偏移」这个豁免类别**太宽**。它本意是「官方多写了段所以后面推后」，
> 但它同时**吞掉**了「我方段顺序写错」「我方少写一次 ALIGN4」这类**真 bug**。
> 判据应当区分「**已知的、量化的**段差」与「未知偏移」——
> 而不是把所有偏移差异一律归入豁免。

### 8.3 每个 bug 的判据强度

全部是**语料全量普查**（3333 个模型逐个），不是抽样：

| bug | 判据脚本 | 命中 |
|---|---|---|
| #1 | `probe_keyvalue_quote.js` | 735/735 无前导引号，**0 例外** |
| #2 | `probe_empty_section_offsets.js` | 3212/3212 指向空串 |
| #3 | `probe_localnode_order.js` | 28/28 `localnode < bodypart` |
| #4 | `probe_srcbone_align.js` | 3262/3262 `== ALIGN4(kv_end)` |
| #5 | `probe_pool_start2.js` | 池起点 == 公式预测 **3331/3333** |
| #6 | `probe_seq_label_prefix.js` | 8965 个 animdesc 名全带 `@`；11170 个 label 全不带 |
| #7 | `probe_file_align.js` | 3333/3333 文件长度 % 4 == 0 |
| #8 | `probe_definebone_only.js` | 2973 根骨骼无顶点权重（106 个模型） |

**#5 的 2 个例外**（`dest_fire_ceilingfallbig` / `dest_fire_wallcollapse`）
差 3 与 5 字节，是**尚未查清**的 `linearbone` 形态差异 ——
诚实标注，不假装 3333/3333。

### 8.4 顺带修掉的一个真缺口：`$definebone` 会**创造**骨骼

`parity/myprop.qc` 写 `$definebone "tip" "root" 0 0 8`，而
`myprop-ref.smd` 的 `nodes` **只有 root**。官方产物仍是 **2 根骨骼**：

```text
官方 myprop.mdl：numbones = 2
  BONE[0] root  pos = [0,0,0]  flags = 0x40700
  BONE[1] tip   pos = [0,0,8]  flags = 0x200   ← 来自 $definebone
```

源码 `BuildGlobalBonetable`（`simplify.cpp:3616-3654`）：**先**把
`g_importbone`（`$definebone` 收集的）逐条插进骨骼表，**再**并入各 SMD
用到的骨骼（同名靠 `findGlobalBone` 去重）。

mdlc 之前要求「SMD 每帧都必须有全部骨骼的姿态」，于是这种 QC 直接报
`缺少部分骨骼的姿态（1 / 2 根有数据）`—— 而官方正常编过。

**这解释了 `verify_parity.ps1` 为什么长期红灯**：`cube.toml` 与
`myprop.qc` 漂移了，而漂移的原因是 mdlc 缺这个特性。
（我一度以为只是 fixture 写错 —— 那会把真缺口当成测试维护问题放过。）

修法（`compile.rs::load_smd_frames` 的 `fallback`）：
SMD 里没有、但 `[[bones]]` **显式给了** `position`/`rotation` 的骨骼保留，
姿态取该显式值；**没给就报错**（静默用 `[0,0,0]` 会让骨骼塌到原点，
表现为顶点被拉向世界原点，且不报错）。

### 8.5 验收

| 项 | 修前 | 修后 |
|---|---|---|
| `mikuw.mdl` vs 官方 | **11 处差异**（3708 vs 3710 字节） | **0 处差异**（3708 == 3708） |
| `verify_parity.ps1` | **红灯**（33 处非预期差异） | **绿灯**（MDL 0 差异，2372 == 2372） |
| `mikuw.phy` 点体积 | 257.64（**大了 61023 倍**） | **0.004222**（官方 0.004126） |
| `cargo test --release` | 326 | **330**（+4 新回归测试） |
| `cargo clippy --all-targets` | 零警告 | 零警告 |
| `parity_snapshot.js` | 86/86 | **86/86** |
| `verify_linearbone.js` | 78 产物 0 违反 | **80 产物 0 违反** |
| `cmp_ikrule.js` | 0 差异 | **0 差异** |
| `cmp_miku_real.js` | 2379 一致 / 42 不同 | **2379 / 42**（不变，全是 rotscale ULP） |
| VVD 往返 | 3302/3302 | **3302/3302** |

**新回归测试**：
* `compile.rs::bone_declared_but_absent_from_smd_is_kept` —— 正向：必须保留。
* `compile.rs::bone_absent_from_smd_without_explicit_pose_errors` —— 反向：必须报错。
* `phy.rs::phy_points_are_converted_from_inches_to_metres` —— 点必须是米。
* `phy.rs::text_volume_is_inches_cubed_while_points_are_metres` —— 两种单位并存。

### 8.6 仍未解决（诚实清单）

> ⚠️ **本表已部分过期，见下方各条的状态更新。** 最新状态以
> `PROGRESS.md` §1.1 与 §32 为准。

| 项 | 状态 |
|---|---|
| `mstudiotexture_t.material`（+0x10） | **不是缺口** —— 运行时指针（ASLR），引擎不读。见下 |
| `.phy` 凸包**点集** | ✅ **已解决** —— 见 §8.7 与 `PROGRESS.md` §32（坐标系 + 1cm 压实） |
| 字符串池起点 2/3333 例外 | 差 3/5 字节，疑与 `linearbone` 形态有关 |
| `$inertia` / `$damping` / `$rotdamping` / `$rootbone` | TOML 未建模（本例恰好全等于缺省，**未暴露**） |
| 非零 `$cbox` | TOML 未建模（本例是 `0 0 0 0 0 0`，恰好等于缺省） |

**`mstudiotexture_t.material` 不可复现**（实测）：

```text
同一 QC 连编两次 → material 值不同
  run1 = 181746972, 181746972, 181746972, 181746972, 181746972
  run2 = 178535708, 178535708, 178535708, 178535708, 178535708
```

`studio.h` 的注释已说明：`mutable IMaterial *material; // fixme: this
needs to go away . . isn't used by the engine, but is used by studiomdl`。
它是**进程内指针**（ASLR 每次不同），语料里 5987 条材质记录有 **1056 个
不同取值**、761 个模型内部就不一致 —— **引擎不读，mdlc 写 0 是对的**。

**`.phy` 凸包点集差异**（实测三方对照，同一份 `phy.smd`，
源网格 168 三角形 / 86 唯一位置；单位换算修复**后**的数据）：

| 编译器 | `surfSize` | ledge 区 | **三角形** | **点** |
|---|---|---|---|---|
| nekomdl 2.1.3（原作者用的） | 1996 | 1968 | 78 | 41 |
| **真 studiomdl.exe** | 1948 | 1920 | **76** | **40** |
| **mdlc** | 4108 | 4080 | **166** | **85** |

（判据：`IVP_Compact_Ledge` 的 `+0x08` 是 `size_div_16 = 1 + nTri + nPts`，
`+0x0C` 是 `i16 nTri` ⟹ `nPts = size_div_16 − 1 − nTri`。
脚本 `phy_ledge_counts.js`。）

**已排除的两个假设**（都实测否掉，不是推测）：

1. **「一方没三角化」** —— 否。
   官方 76 = 2×40−4、mdlc 166 = 2×85−4，**两边都满足单纯形凸包的欧拉关系**
   （`F = 2V − 4`）。所以都是完整三角化的凸包，只是**点集不同**。
2. **「mdlc 多留了内部点」** —— 否。
   `probe_phy_inside_official.js` 用官方凸包的**外法线**逐点判内外
   （自检：官方自己 40 个顶点被判「严格内部」的 **0/40**、mdlc 自己 **0/85**，
   说明符号标定正确）：

   ```text
   修单位之前：mdlc 85 个顶点落在官方凸包内的 —— 0 / 85（坐标系都不同！）
   修单位之后：容差 1e-4 → 11 / 85；容差 0.01 → 19 / 85；容差 0.1 → 42 / 85
   反向：官方 40 个顶点落在 mdlc 凸包内的 —— 40 / 40
   ```

   ⟹ 修单位后两者**进入了同一个坐标系**（11 个点已精确落在官方凸包内），
   但 mdlc 的凸包仍**严格包含**官方的。官方那个**更小**。

**结论**：官方在求凸包**之前**把点集从 86 简化到了 40（体积随之小 2.3%），
mdlc 直接用全部 86 个位置点。这不是「漏剔除内部点」，而是
**官方先做了一趟点云简化 / 焊接**。

**对照组 `tor1`**（光滑圆环）两者 `surfSize` **完全相同**（13852 == 13852）、
`nTri=572 / nPts=288` 逐项相同，体积差 0.0002% —— 因为光滑曲面上
**没有可合并的近共面点**，简化不生效。

> 属独立的几何算法工作（要复刻 IVP 的点云简化），本轮**未修**，
> 但已从「体积差 2.3%」这种间接指标**定位到具体的点集差异**（86 → 40），
> 并**排除**了两个看似合理的错误解释。

---

## 8.7 **`.phy` 的量纲 bug —— 所有碰撞体大了 39.37 倍**

> 这一条是查 §8.6 的「凸包点集差异」时**顺带撞出来的**，
> 严重程度**远超**它原本要解释的现象。

### 症状

用同一份 `msh1.smd`（已知 50×10×50 inch 的长方体）分别交给真
`studiomdl.exe` 与 mdlc：

```text
studiomdl 报告的体积：17000 in^3
官方 .phy 的点算出的体积：0.278580
mdlc  .phy 的点算出的体积：17000.000000

立方根(17000 / 0.278580) = 39.3691
1 / 0.0254               = 39.3701        ← inch → meter
```

官方 `msh1.phy` 的点 bbox 实测
`[-0.127, -1.143, -0.127] .. [0.127, 0.127, 1.143]`，
源是 `[-5,-5,-5] .. [45,5,45]` —— 各轴范围
`0.254 / 1.27 / 1.27` = `10×0.0254 / 50×0.0254 / 50×0.0254`，逐轴吻合。

### 根因

`.mdl` / `.vvd` / `.vtx` / SMD **全部**用 Source 单位（inch），
但 `.phy` **不是** —— 它由 `vphysics.dll` 的 `CollideWrite`
（`collisionmodel.cpp:2345`）序列化，而 vphysics 内部是 **IVP**，
用**米**。

### 为什么藏了这么久

`check_invariants` 查的是**布局自洽**：点数组长度、索引范围、
`c_point_offset` 后缀和、`surfaceSize == 48 + ledge + 28×nodes`。
把点整体乘 39.37 **不破坏任何一条** —— 长度不变、索引不变、体积仍是正的。

所以它**不会报错**，只会在游戏里表现为「碰撞体比模型大 39 倍」。
而既有的 `tor1` 体积对比（`22728.12 vs 22728.16`）**恰好没暴露它**：
两边都是同一个实现的输出，且当时根本没有单位换算。

### 语料全量判据（`probe_phy_unit.js`）

1316 个可算的官方 `.phy`，比较 `mdl hull 最大边 / phy 点最大边`：

| 区间 | 占比 |
|---|---|
| `39.37 ± 25%` | **66.9%** |
| `1.00 ± 0.25` | **0.0%** |

（比值不精确等于 39.37，是因为 `.mdl` 的 hull 来自**渲染**网格的顶点 AABB、
`.phy` 来自**碰撞**网格 —— 两者不是同一份几何。但量纲差 39 倍是
**数量级**差异，不会被几何差异淹没。）

### 修复后

| 量 | 官方 | mdlc 修前 | mdlc 修后 |
|---|---|---|---|
| 点体积（`mikuw`） | 0.004126 | 257.64 | **0.004222** |
| 点体积（`msh1`） | 0.278580 | 17000.00 | **0.278580** |
| text `volume`（`msh1`） | 16999.996 | — | **17000.002** |

`mikuw` 剩下的 2.3% 差异就是 §8.7 末尾说的**点集未简化**（85 vs 40 点），
与量纲无关。

### ⚠️ 同一文件里**两种单位并存**

修完点数组后，`text` 段的 `"volume"` **不能**跟着换算 ——
它是 studiomdl **自己**用 `physcollision->ConvexVolume()`（Source 单位）
算出来再 `fprintf` 的（`collisionmodel.cpp:1174` 累加 → `2310` 打印），
与 IVP 的点数组**不是同一条路径**。

实测（官方 `msh1.phy`）：

```text
text 段 "volume"    = 16999.996094   ← 就是 17000 in³
点数组算出的体积    = 0.278580 m³
0.278580 / 0.0254³  = 17000.00       ← 换算回 inch³ 完全吻合
```

我在第一版修复里**顺手把 `volume` 也换算成米了**，结果 text 段与官方
差 `61023` 倍 —— 被 `probe_phy_text_volume_unit.js` 抓到。
**两条路径、两种单位、同一个文件。**

### 新增的回归测试（都能反向证伪）

| 测试 | 钉住什么 |
|---|---|
| `phy_points_are_converted_from_inches_to_metres` | 点必须是 ±0.0254（写字面量，**不用常量** —— 见下） |
| `text_volume_is_inches_cubed_while_points_are_metres` | text 段是 inch³ **而**点是米 |
| `cube_leaf_node_geometry_is_hand_computable` | `radius` 跟着换算、`box_sizes`（比值）不换算 |
| `cube_rotation_inertia_matches_density_one_tensor` | 惯量按 `k⁵` 缩放（含质量 ⟹ 体积 `k³` × 长度² `k²`） |

**反向证伪实测**：把 `SOURCE_TO_IVP` 改成 `1.0` 后，
`phy_points_are_converted_from_inches_to_metres` 立刻报
`点[0][0] = -1，应为 ±0.0254`。

> ⚠️ **测试里不能引用被测常量。** 第一版写成
> `let want = SOURCE_TO_IVP;` —— 两边同时改就永远通过，
> 把常量改成 1.0 后测试**仍然「通过」**。改成字面量 `0.0254` 才真的会红。
> **恒等式不是测试。**

### 8.8 本轮新增的方法论教训（续）

12. **「查不到原因」可能因为问错了问题。**
    我本来在查「凸包点数为什么是 85 vs 40」，量到一半发现
    **坐标系本身就不同** —— 那是个严重得多的问题。
    **差异的「形状」比差异的「数值」信息量大**：
    85 与 40 差 2 倍（像是算法差异），
    而 bbox 差 39.37 倍（是量纲错误）。

13. **`check_invariants` 这种「自洽性」检查抓不到量纲错误。**
    整体缩放不破坏任何一条结构不变量。
    **只有与外部参照物（官方产物）比对才能发现。**
    这条与教训 8（真实第三方输入不可替代）是同一个道理的另一面。

14. **同一个文件里可以有两种单位。**
    `.phy` 的点是米、text `volume` 是 inch³ —— 因为它们来自
    **两条不同的代码路径**（vphysics 序列化 vs studiomdl 自己算）。
    **不要假设「一个文件一种单位」。**



### 8.7 本轮新增的方法论教训

8. **真实第三方输入是不可替代的冒烟测试。**
   86 个自造 parity 用例全绿，却漏掉 8 个 bug（其中 4 个影响全部语料）。
   自造用例验证「规则成立」，验证不了「规则叠加」。

9. **「豁免类别」会吞掉真 bug。**
   `verify_parity.ps1` 把「段偏移不同」一律归入「结构性偏移」，
   于是**段顺序写错**、**少写一次 ALIGN4** 都被放过。
   豁免必须**量化**（「偏移差 == 官方多写段的字节数」），不能是**类别**。

10. **fixture 漂移可能是真缺口，不要当成测试维护问题。**
    `cube.toml` 与 `myprop.qc` 不一致，第一反应是「fixture 写错了」。
    查下去才发现是「mdlc 不支持 `$definebone` 创造骨骼」这个真特性缺失。
    **先问「官方会怎么做」，再决定改 fixture 还是改代码。**

11. **同一个字段在不同上下文可以是两种基准 —— 也可是两种「空」语义。**
    `eventindex` 相对数组起点、`iklockindex` 相对记录自身（§7.5 教训 1）；
    而「空段」在 `szanimblocknameindex` 上是**指向空串**、
    在 `linearboneindex` 上是**写 0**。**逐字段实测，不要外推。**


