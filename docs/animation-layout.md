# MDL 动画数据布局 —— 实测反推规格

> ## ⚠️ 三条结论已被后续实验推翻（2026-09 更新）
>
> 本文的受控实验（`exp50`/`exp53`/`exp64`/`exp65` 等）有一个**共同的盲点**：
> 它们的 SMD **第 0 帧旋转恰好都是 0**。凡是在这个前提下「绝对」与
> 「增量」等价的假设，实验都**无法区分**。在 89 骨骼 × 30 帧的真实规模
> 上重测后，以下三条结论被推翻：
>
> | 本文结论 | 实测真相 | 证据 |
> |---|---|---|
> | **C10 / §3.4** `LOOPING` 末帧强制 0 | 末帧照抄**第 0 帧的存储值** | `gen_loop` 实验：末帧 = `0.499991`（= 第 0 帧），不是 0 |
> | **§3.5** 逐帧值 = 相对**第 0 帧**的增量 | 相对**参考姿态**（骨骼表里的 `rot`/`pos`） | `biganim`：13.14% vs **100.00%** |
> | **C8 / §4.2** 除数恒为 32767 | 按极值符号：负 → **32768**，正 → **32767** | 反解 `div = maxAbs/scale`：负的 31 轴平均 32767.995，正的 22 轴平均 32766.998 |
>
> 另外两条本文未提及但同样关键的规则：
>
> - **存储值是「规范化欧拉角 − 参考姿态」**：`canonical_euler` 复刻
>   studiomdl 的 `MatrixAngles`（含万向锁分支），且必须**全程 f64**。
>   `gimbal` 实验实测官方把 `[0.35, π/2, −0.25]` 规范化成 `[0, π/2, −0.6]`。
> - **增量要 `wrapToPi` 包裹**：`canonical_euler` 值域是 `(−π, π]`，
>   参考姿态可能也在 π 附近，直接相减会在割线处产生 ±2π 跳变。
>
> **权威来源**：实现与逐值验证见 `src/anim_writer.rs`、`src/bone_math.rs`；
> 结论汇总见 `README.md` 的「动画实现的关键规则」。
> 本文其余部分（结构布局、payload 顺序、union 语义、偏移基准等）
> **仍然有效** —— 被推翻的只是「值怎么算」，不是「字节怎么排」。
>
> 复现脚本：`docs/_probe/gen_loop.js`、`gen_rootbias.js`、`gen_gimbal.js`、
> `gen_bigref.js`、`gen_big_anim.js`、`diag_divisor_rule.js`、
> `diag_solve_div.js`、`cmp_anim_semantics.js`。

> 目标：把 Valve `studiomdl.exe`（L4D2，MDL version 49）写出的动画段布局，逐字节反推成一份可直接据以实现的规格。
>
> **本文所有结论都来自实测。** 证据来源三类：
> 1. **最小模型** `mymod/myprop.mdl`（1720 字节，1 骨骼、1 序列、1 帧）—— 原始文件在调研期间被并行任务删除，但调研开始时已完整 dump 成 hex；用相同 QC/SMD 重新编译得到**动画区逐字节一致**的复现件（§2.2）。
> 2. **受控实验**：用 `studiomdl.exe -nop4 -game <l4d2>` 编译自建 SMD/QC，共 **128 个成功样本**，覆盖旋转范围、帧数、循环、section、`$sectionframes`、`$bonesaveframe`、SMD 文本精度等变量。
> 3. **真实中等复杂度模型** `v_autoshotgun.mdl`（619136 字节，89 骨骼、37 动画、17 个带动画 section）。
>
> 复现脚本全部在 `docs/_probe/` 下（只读分析，**未改动 `src/`**）。

---

## 1. 结论摘要

### 1.1 已确认（有逐字节实测证据）

| # | 结论 | 证据位置 |
|---|---|---|
| C1 | `mstudioanim_t` = **4 字节头**：`bone`@+0, `flags`@+1, `nextoffset`@+2(int16)。`pData()` 从**结构起点 +4** 开始 | §2.3 |
| C2 | `nextoffset` **相对本条 `mstudioanim_t` 自身** | §2.3 |
| C3 | 链结束 = `nextoffset == 0`，其后紧跟 **1 条 4 字节全零记录**（实测 120/125 个链）；**空链**（该动画无任何骨骼动画）= `numbones` 条 `ff 00 00 00` | §2.4 |
| C4 | **链上只出现「实际有动画」的骨骼**；常量骨骼被省略（姿态写进 `mstudiobone_t`） | §3.6 |
| C5 | `animdesc.baseptr = -animdesc_self_offset`（相对自身，指向 studiohdr） | §6 |
| C6 | `seqdesc.animindexindex` → `int16[numblends]`，元素是 **animdesc 下标**（不是偏移） | §7 |
| C7 | RLE 头 2 字节是 **union**：`int16 value` ≡ `{byte valid; byte total}`（低字节=valid，高字节=total）。`valid` 个 int16 是采样，run 覆盖 `total` 帧，多余帧**重复最后一个采样** | §4.1 |
| C8 | **缩放公式（精确）**：`rotscale[k] = f32(max(π/8, maxAbs) / 32767)`，`posscale[k] = f32(max(128.0, maxAbs) / 32767)`。根骨骼 Z 的 rotscale 下限是 **π/2** | §4.2 |
| C9 | **根骨骼 Z 轴 +90° 偏置**（无条件加） | §5 |
| C10 | `LOOPING` 序列**末帧强制 0**（= 第 0 帧）；非 loop 不强制 | §3.4 |
| C11 | `sectionCount = numframes / sectionframes + 2`；表项 = `{int32 animblock, int32 animindex}`；section 0 的 `animindex` == `animdesc.animindex` | §8.1 |
| C12 | `sectionframes` 默认 **30**，但仅当 `numframes >= 4 * sectionframes` 时才真正启用；否则 `sectionindex = sectionframes = 0` | §8.2 |
| C13 | section 表最后两项是**重复/垃圾数据**（`framesInSection` 为 0），必须跳过 | §8.1 |
| C14 | `seqdesc.bbmin/bbmax` = 用**该序列自己的动画数据**蒙皮后的**逐帧顶点 AABB 并集** | §9 |
| C15 | L4D2 的 `studiomdl.exe` **从不生成** `STUDIO_FRAMEANIM`、zeroframe、saveframe、animblock | §10 |
| C16 | 逐帧值 = `wrapToPi(angle[f] - angle[0] + rootZOffset)`；位移 = `pos[f] - pos[0]` | §3.5 |

### 1.2 未确认（明确标注「未验证」）

| # | 项 | 原因 |
|---|---|---|
| U1 | `STUDIO_FRAMEANIM` 实际字节布局与 `framelength` 自洽性 | L4D2 studiomdl 不产出；全盘 152 个 .mdl 无样本；10 个 QC 触发实验全部失败（§10） |
| U2 | `zeroframespan`/`zeroframecount`/`zeroframeindex` 数据布局 | 全盘 0 样本；`$bonesaveframe` 也不置位 `BONE_HAS_SAVEFRAME_*` |
| U3 | `animblock`（外置 `.ani`）路径 | 全盘 `numanimblocks == 0` |
| U4 | `bbmin/bbmax` 在**常量姿态**模型上的精确取整规则 | 多帧模型精确匹配；`exp2`/`exp31`/`exp41`/`exp42` 不匹配（§9.3） |
| U5 | RLE run 边界的**判定阈值** | 只观测到结果形态（§4.3），未反推 studiomdl 的最优性逻辑 |
| U6 | `$sectionframes <count> <minFrameCount>` 第二参数的确切语义 | 实测 `30 100` 会禁用（`40 < 100`），但精确比较式未逐点测定（§8.2） |
| U7 | `STUDIO_FRAME_CONST_POS2(0x20)` / `CONST_ROT2(0x40)` / `ANIM_ROT2(0x80)` | 仅见于 Crowbar 反推文档，L4D2/CSGO 公开 SDK 头文件中无定义（§10.3） |
| U8 | `animdesc` `+0x1C` 字段（v49 `ikrulezeroframeindex`） | 实测所有样本恒为 0，未追踪其指向 |
| U9 | 多 blend / pose parameter 的端到端写出 | 只验证了 `animindexindex` 的 int16 数组语义（§7） |

---

## 2. `mstudioanim_t` 链布局

### 2.1 结构定义（SDK 原文，`hl2sdk-l4d2/public/studio.h:572`）

```c
struct mstudioanim_t {
    byte  bone;
    byte  flags;              // weighing options
    inline byte *pData() const { return ((byte*)this) + sizeof(struct mstudioanim_t); };
    inline mstudioanimvalueptr_t *pRotV() const { return (mstudioanimvalueptr_t*)(pData()); };
    inline mstudioanimvalueptr_t *pPosV() const { return (mstudioanimvalueptr_t*)(pData()) + ((flags & STUDIO_ANIM_ANIMROT) != 0); };
    inline Quaternion48 *pQuat48() const { return (Quaternion48*)(pData()); };
    inline Quaternion64 *pQuat64() const { return (Quaternion64*)(pData()); };
    inline Vector48 *pPos() const { return (Vector48*)(pData()
        + ((flags & STUDIO_ANIM_RAWROT)!=0)*sizeof(*pQuat48())
        + ((flags & STUDIO_ANIM_RAWROT2)!=0)*sizeof(*pQuat64())); };
    short nextoffset;
    inline mstudioanim_t *pNext() const { if (nextoffset != 0) return (mstudioanim_t*)(((byte*)this) + nextoffset); else return NULL; }
};
```

**关键点**：`nextoffset` 声明在 `pData()` 之后，但 4 字节对齐下 `sizeof(mstudioanim_t) = 1+1+(2 padding)+2 = **4**`。所以 `pData() = this + 4`，`nextoffset` 在 **+2**。**数据不是紧跟 bone+flags（+2），而是从 +4 开始** —— 这一点由实测确认（§2.3）。

### 2.2 最小模型动画区（实测 hex）

`myprop.mdl`，`animdesc[0]` @ `964`，`animindex = 108` → 动画数据 @ `1072` = `0x430`：

```
0430  00 20 00 00 | 00 00 10 00 00 3e 41 6d | 00 00 00 00
```

| 偏移 | 字节 | 字段 | 值 |
|---|---|---|---|
| 0x430 | `00` | `bone` | **0** |
| 0x431 | `20` | `flags` | **0x20 = STUDIO_ANIM_RAWROT2** |
| 0x432–0x433 | `00 00` | `nextoffset` | **0 → 链结束** |
| 0x434–0x43B | `00 00 10 00 00 3e 41 6d` | `Quaternion64` | 见 §5.1 |
| 0x43C–0x43F | `00 00 00 00` | 链尾全零记录 | §2.4 |

**这条链只有 1 个 `mstudioanim_t`（bone=0），共 16 字节。**

> **复现件一致性**：用相同 QC/SMD 重新编译，动画区 `[964, 1308)` 与原始 hex 转储**逐字节相同**。整文件仅 6 个字节不同（`0x599/0x59a/0x59d/0x59e/0x5f2/0x646`），全部属于 VTX/纹理哈希等无关区。见 `docs/_probe/artifacts/exp1.mdl`。

### 2.3 多骨骼链（受控实验 `exp5`，40 帧）

```
0660  00 20 0c 00 | 00 00 10 00 00 3e 41 6d | 01 08 5c 00 | 00 00 00 00 06 00 | 28 28 ...
      ↑bone0      ↑RAWROT2 8B                ↑bone1      ↑rotV offsets     ↑RLE
```

| 偏移 | 字节 | 解析 |
|---|---|---|
| 0x660 | `00 20 0c 00` | `bone=0 flags=0x20 nextoffset=+12` → 下一条在 `0x660+12 = 0x66C` |
| 0x664–0x66B | `00 00 10 00 00 3e 41 6d` | Quaternion64（根骨骼常量姿态） |
| 0x66C | `01 08 5c 00` | `bone=1 flags=0x08(ANIMROT) nextoffset=+92` → `0x66C+92 = 0x6C8` |
| 0x670 | `00 00 00 00 06 00` | `mstudioanim_valueptr_t`：`offset[3] = {0, 0, 6}` |
| 0x676–… | RLE | z 轴流在 `0x670 + 6 = 0x676` |
| 0x6C8 | `02 08 00 00` | `bone=2 flags=0x08 nextoffset=0` → **链结束** |
| 0x6CC | `06 00 00 00 00 00` | `offset[3] = {6, 0, 0}`，x 轴流在 `0x6CC + 6 = 0x6D2` |
| 0x6D2–0x6D5 | `28 28 00 00` | RLE 头（`valid=40 total=40`）+ 首采样 `0` |
| 链末 +4 | `00 00 00 00` | 链尾全零记录 |

**验证 `nextoffset` 相对自身**：`0x660 + 12 = 0x66C` ✓；`0x66C + 92 = 0x6C8` ✓。

### 2.4 链的结束方式（实测统计）

对 `v_autoshotgun.mdl` 的全部 **125 个 section 链**逐一走到链尾：

| 结束形态 | 数量 | 含义 |
|---|---|---|
| `nextoffset == 0`，其后紧跟 1 条 4 字节**全零记录** `00 00 00 00` | **120** | 正常链 |
| 首条记录就是 `ff 00 00 00`（`bone=255`），**连续 `numbones` 条** | **5** | **空链**：该动画没有任何骨骼动画 |

**正常链的实测例**（`anim[0] 'a_idle'` sec0）：

```
last record @30380, payloadEnd = 30556
30556: 00 00 00 00        ← 链尾全零记录（bone=0, flags=0x00, nextoffset=0）
```

**空链的实测例**（`anim[3] 'a_look_mid'`，`numframes=90`，89 骨骼）：

```
294b0  ff 00 00 00 ff 00 00 00 ff 00 00 00 ff 00 00 00   ← 4 条 bone=255
294c0  ff 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ← 第 5 条 bone=255，然后全零
```

实测前导 `ff 00 00 00` 记录数 = **5**，而 `numbones = 89`。

> **结论**：`bone == 255` 是「**该骨骼在本动画中无数据**」的占位记录，不是终止符。空链由 `numbones` 条这样的记录组成（也可能更少 —— 实测只见到 5 条，说明它是 studiomdl 内部「已处理骨骼数」的残留，不可依赖具体条数）。
>
> **实现要点（写出器）**：
> - 有动画的骨骼 → 正常记录，用 `nextoffset` 串起来；
> - 链尾写 `nextoffset = 0` + **1 条 4 字节全零记录**；
> - 某动画完全无骨骼动画 → 写 ≥1 条 `ff 00 00 00`（建议写 `numbones` 条以贴近 studiomdl）。
>
> **读取器**必须把 `bone == 255` 当作「跳过该记录、不产生姿态数据」处理，而不是当成终止符提前收工。

### 2.5 payload 顺序（实测确认）

```
+0  bone        u8
+1  flags       u8
+2  nextoffset  i16
+4  [flags&0x20] Quaternion64   8B   RAWROT2
+…  [flags&0x02] Quaternion48   6B   RAWROT
+…  [flags&0x01] Vector48       6B   RAWPOS
+…  [flags&0x08] valueptr       6B   ANIMROT  (int16 offset[3]，相对 valueptr 自身)
+…  [flags&0x04] valueptr       6B   ANIMPOS
```

实测出现的 flags 组合：`0x20`（12B）、`0x21`（RAWROT2+RAWPOS = 18B）、`0x08`（10B）、`0x0C`（ANIMROT+ANIMPOS = 16B）。**RAWROT2 与 RAWROT 在实测中从未同时出现**（互斥）。

---

## 3. 每骨骼记录的出现规则

### 3.1 最小模型只有 1 根骨骼（重要更正）

任务描述说「2 根骨骼」，实测 **`numbones == 1`**。SMD 也确实只有 1 个 node：

```
nodes
0 "root" -1
end
skeleton
time 0
0 0.000000 0.000000 0.000000 0.000000 0.000000 0.000000
```

（`numbones = 1`，`boneindex = 664`；`mstudiobone_t` 216 字节 → 664..880。）

### 3.2 最小模型的 `flags = 0x20` 是**规则**，不是特例

**规则：某骨骼的旋转在整个序列中恒定 → 写 1 条 `RAWROT2`（8 字节 Quaternion64），不写逐帧数据。**

| 实验 | 内容 | 结果 |
|---|---|---|
| `exp2` | 3 骨骼 1 帧 | chain = `[bone 0]`，`flags=0x20` |
| `exp26` | 8 帧，全部 0° | chain = `[bone 0]`，`flags=0x20` |
| `exp41` | 6 帧，b1 每帧恒定 30° | chain = `[bone 0]` |
| `exp42` | 6 帧，b1 每帧恒定位移 (10,20,30) | chain = `[bone 0]` |
| `exp9` | 5 帧，b1 静止、b2 旋转 | chain = `[bone 0, bone 2]` |
| `exp43` | 6 帧，b1 静止、b2 旋转 | chain = `[bone 0, bone 2]` |

最小模型里 bone 0 在唯一一帧的姿态 = 单位四元数 + 根骨骼 Z 偏置 90° → 恒定 → 写 `RAWROT2`。

**所以「1 帧」不是原因，「常量」才是。** 若换成 2 根骨骼且其中一根有逐帧动画，链上就会有 2 条记录（见 `exp9`/`exp43`）。

### 3.3 为什么最小模型只有 1 条记录

`numbones == 1` 且该骨骼旋转恒定 → 恰好 1 条 `RAWROT2`。

### 3.4 loop 语义：末帧强制为 0

| 实验 | SMD 输入 | `flags` | 写入的 int 序列 |
|---|---|---|---|
| `exp64` | Z 0/10/20/30/40°，5 帧 | `0x1`（loop） | `0, 10922, 21844, 32767, **0**` |
| `exp65` | 同上 | `0x0`（非 loop） | `0, 8191, 16383, 24575, 32767` |
| `exp66` | Z 5/15/25/35/45°，5 帧 | `0x1`（loop） | `0, 10922, 21844, 32767, **0**` |

**结论**：`STUDIO_LOOPING` 时，第 `numframes-1` 帧的**所有**通道（旋转 + 位移）强制写成 0（即与第 0 帧同姿态）。非 loop 不强制。

### 3.5 逐帧值公式

```
旋转：d[f][k] = wrapToPi( smd_rot[b][f][k] - smd_rot[b][0][k] + (k==2 && bone.parent<0 ? π/2 : 0) )
位移：e[f][k] = smd_pos[b][f][k] - smd_pos[b][0][k]
若 (flags & STUDIO_LOOPING) 且 f == numframes-1：d = e = 0
```

**实测对照 `exp65`**（非 loop，Z 0/10/20/30/40°，5 帧，`rotscale = 0.000021305946575012058`）：

| 帧 | SMD 角度 | 量化 int | `int × rotscale` | 误差 |
|---|---|---|---|---|
| 0 | 0° | 0 | 0.0000° | — |
| 1 | 10° | 8191 | 9.9991° | 0.0009° |
| 2 | 20° | 16383 | 19.9994° | 0.0006° |
| 3 | 30° | 24575 | 29.9997° | 0.0003° |
| 4 | 40° | 32767 | 40.0000° | 0.0000° |

**实测对照 `exp5`**（40 帧，bone2 X 轴 0→78° 步进 2°）：`reconstruct.js` 逐帧重建，误差 < 0.004°（受 SMD 6 位小数限制）。

### 3.6 常量骨骼不进链 —— 但姿态必须写进 bone 表

`exp41`：6 帧，b1 每帧恒定 30°。
- 链上**没有** bone 1；
- 但 `bone[1].quat = [0, 0, 0.258819, 0.965926]`、`bone[1].rot = [0, 0, 30°]` —— **常量姿态被写进了骨骼参考姿态**。

**「省略」= 回落到骨骼参考姿态。** 实现时：常量骨骼不写链记录，但必须把姿态写进 `mstudiobone_t.quat/rot`（mdlc 已有此逻辑）。

---

## 4. RLE 编码规则

### 4.1 语义（实测）

```c
union mstudioanimvalue_t {
    struct { byte valid; byte total; } num;   // valid = 低字节, total = 高字节
    short value;                              // ← 是 union！不是独立字段
};
```

**实测证明这是 union**：

| 流位置 | 原始字节 | `valid` | `total` | `union int16` | `valid \| (total<<8)` |
|---|---|---|---|---|---|
| `a_deploy` b12 rot.x @169778 | `1e 1e` | 30 | 30 | 7710 | 7710 ✓ |
| `a_deploy` b12 pos.x @169964 | `01 1e` | 1 | 30 | 7681 | 7681 ✓ |
| `a_deploy` b28 rot.x run1 @172602 | `01 04` | 1 | 4 | 1025 | 1025 ✓ |
| `a_deploy` b68 rot.y run1 @177120 | `05 08` | 5 | 8 | 2053 | 2053 ✓ |

解码循环：

```
frame = 0
while frame < frameCount:
    valid = byte[p]          // 低字节
    total = byte[p+1]        // 高字节
    if total == 0: break
    p += 2
    for i in 0..valid-1:
        samples[frame++] = int16[p];  p += 2
    last = samples[frame-1]
    for i in valid..total-1:
        samples[frame++] = last          // 重复最后一个采样
```

> **注意**：`value` 字段**不是**「run 的代表值」，而是 `valid|(total<<8)` 的 reinterpret。studiomdl 写的 `valid` 恒 ≥ 1，所以 `value` 恒 > 0（实测最小 769 = `01 03`）。

### 4.2 缩放公式（精确，已严格验证）

```
rotscale[b][k] = float32( max( floorRot(b,k), maxAbsRot ) / 32767 )
posscale[b][k] = float32( max( 128.0,          maxAbsPos ) / 32767 )

floorRot(b,k) = (k == 2 && bone[b].parent < 0) ? π/2 : π/8      // 90° 或 22.5°
```

**严格相等验证**（`prec*` 实验，SMD 文本 6/9/12/15 位小数）：

```
目标：40 帧，bone1 Z 从 0° 到 117°（步进 3°）
target maxAbs = 117° = 2.0420352248333655 rad
stored rotscale            = 0.000062319872085936368
f32(maxAbs/32767)          = 0.000062319872085936368
stored == f32(maxAbs/32767)?  TRUE
```

`verify_model.js` 对 `prec6/9/12/15` 四个样本用**严格 `!==` 比较** rotscale/posscale，全部 PASS。

**除数确实是 32767，不是 32768**：

```
maxAbs/32767 = 0.000062319871359397123
maxAbs/32768 = 0.000062317969507854173
stored       = 0.000062319872085936368   ← 与 /32767 同量级
```

（stored 与 `maxAbs/32767` 相差 1 个 float32 ULP = 1.17e-8 相对量，来自 SMD 文本 → float32 的舍入，**公式本身精确**。）

**`maxAbs` 的定义与比值验证**（6 位小数 SMD，噪声 ≤ 2e-3 相对量）：

| 实验 | bone/轴 | maxAbs | rotscale | maxAbs/rotscale |
|---|---|---|---|---|
| `exp5` | b1 z | 3.11503838 | 0.0000950634349 | 32767.997 |
| `exp5` | b2 x | 3.13274123 | 0.0000956036820 | 32767.998 |
| `exp28` | b1 z | 0.523599000 | 0.0000159789743 | 32767.998 |
| `exp35` | b1 x | 2.79252631 | 0.0000852211379 | 32768.001 |
| `exp39` | b1 z | 3.14159231 | 0.0000958737874 | 32768.000 |
| `exp36` | b1 x (pos) | 300.000000 | 0.00915555283 | 32767.000 |

**下限（floor）实测**：

| 实验 | 骨骼/轴 | maxAbs | 写入的 scale | `scale × 32767` | 结论 |
|---|---|---|---|---|---|
| `exp23` | b1 z | 0.01745 rad (1°) | 0.0000119845909 | **22.5°** | π/8 下限生效 |
| `exp20` | b1 z | 0.13963 rad (8°) | 0.0000119845909 | **22.5°** | π/8 下限生效 |
| `exp22` | b1 z | 1.22173 rad (70°) | 0.0000372853792 | 70° | 未触发 |
| `exp26` | b0 z | 0 | 0.0000479383634 | **90°** | 根 Z 下限 π/2 |
| `exp34` | b0 z | 2.35619 rad (135°) | 0.0000719075470 | 135° | 未触发 |
| `exp37` | b1 x (pos) | 50.0 | 0.00390636921 | **128.0** | 位移下限 128 |

**`rotscale` / `posscale` 是每骨骼、每轴的，写在 `mstudiobone_t`（+0x48 posscale / +0x54 rotscale），不属于 animdesc。** 全部 89 骨骼共用同一份（每个动画不单独存）。

### 4.3 完整逐字节解码示例（真实模型）

#### 示例 A：6 个 run 的压缩流 —— `a_deploy` bone 28 rot.x

`bone[28].rotscale = [0.0000382511935, 0.0000351516937, 0.0000517716326]`，流指针 = `172602` (`0x2A23A`)，30 帧。

```
2a23a  01 04 8e 3a | 01 05 8d 3a | 01 03 8e 3a | 01 03 8f 3a
2a24a  02 09 90 3a 8f 3a | 05 06 90 3a 91 3a 8e 3a 8f 3a 90 3a
2a25a  01 0a 50 08 14 14 4f 08 4f 08 4e 08 4f 08 4f 08 4d 08
2a26a  4f 08 4f 08 4e 08 4e 08 4f 08 4f 08 4e 08 4f 08 4e 08
2a27a  4d 08 4f 08 4f 08 4d 08 4e 08
2a28a  01 04 74 c5 1a 1a 76 c5 78 c5
```

| run | 偏移 | 头 2 字节 | `valid` | `total` | 采样（int16） | 覆盖帧 |
|---|---|---|---|---|---|---|
| 1 | 172602 | `01 04` | 1 | 4 | `0x3a8e` = 14990 | [0, 4) |
| 2 | 172606 | `01 05` | 1 | 5 | `0x3a8d` = 14989 | [4, 9) |
| 3 | 172610 | `01 03` | 1 | 3 | `0x3a8e` = 14990 | [9, 12) |
| 4 | 172614 | `01 03` | 1 | 3 | `0x3a8f` = 14991 | [12, 15) |
| 5 | 172618 | `02 09` | 2 | 9 | `14992, 14991` | [15, 24) |
| 6 | 172624 | `05 06` | 5 | 6 | `14992, 14993, 14990, 14991, 14992` | [24, 30) |

`valid < total` 的 run 用「重复最后一个采样」补足（run 1 的第 2–4 帧 = 14990，run 2 的第 2–5 帧 = 14989，…）。

展开 30 帧：

```
ints        = [14990,14990,14990,14990, 14989,14989,14989,14989,14989,
               14990,14990,14990, 14991,14991,14991,
               14992,14991,14991,14991,14991,14991,14991,14991,14991,
               14992,14993,14990,14991,14992,14992]
× rotscale[0] = [0.57339,0.57339,0.57339,0.57339, 0.57335×5,
                 0.57339×3, 0.57342×3, 0.57346,0.57342×8,
                 0.57346,0.57350,0.57339,0.57342,0.57346,0.57346]
```

即 **bone 28 的 X 欧拉角在 30 帧里只围绕 0.5734 rad ≈ 32.85° 抖动 ±0.0001 rad**（0.006°）—— 典型的高频噪声被 RLE 精确捕获。

#### 示例 B：常量通道压成 1 个采样 —— `a_deploy` bone 12 pos.x

```
297ec  01 1e 57 fd ...
       ↑valid=1 ↑total=30
```
→ `valid=1, total=30`：采样 `0xfd57` = −681，重复 30 帧。
`× posscale (0.00390636921) = −2.66024`。

#### 示例 C：无压缩 —— `a_deploy` bone 12 rot.x

```
29732  1e 1e | 6b f6 91 f6 fc f6 aa f7 ...
       ↑valid=30 ↑total=30
```
→ 30 个采样全部逐帧存放：

```
ints   = [-2453,-2415,-2308,-2134,-1897,-1600,-1248,-844,-395,95,
           622,1182,1769,2381,3016,3674,4354,5054,5776,6516,
           7273,8038,8807,9570,10311,11006,11619,12108,12430,12543]
× rotscale[0] (0.0000878825449)
       = [-0.21558,-0.21224,-0.20283,-0.18754,-0.16671,-0.14061,-0.10968,-0.07417,
          -0.03471,0.00835,0.05466,0.10388,0.15546,0.20925,0.26505,0.32288,
          0.38264,0.44416,0.50761,0.57264,0.63917,0.70640,0.77398,0.84104,
          0.90616,0.96724,1.02111,1.06408,1.09238,1.10231]
```

#### RLE 形态统计（`a_deploy` 全部 ~180 条流）

| 形态（`valid/total` per run） | 出现次数 |
|---|---|
| `30/30`（无压缩） | 53 |
| `1/4` + `26/26` | 19 |
| `29/30` | 4 |
| `1/30`（纯常量） | 3 |
| `1/4` + `4/6` + `20/20` | 3 |
| 其余（3 run 以上） | ~30 |

**未验证**：run 边界的判定阈值（studiomdl 的最优性逻辑）。

---

## 5. 四元数、坐标系与根骨骼 90° 偏置

### 5.1 最小模型的 Quaternion64 解码

```
字节：00 00 10 00 00 3e 41 6d
bit-packed 21/21/21/1 小端：
  x    = bits[0..20]  = 0x100000 = 1048576
  y    = bits[21..41] = 0x100000 = 1048576
  z    = bits[42..62] = 0x1B7D00 = 1801472
  wneg = bit63 = 0
qx = (1048576 - 1048576) / 1048576.5 = 0
qy = 0
qz = (1801472 - 1048576) / 1048576.5 = 0.70710625
qw = +sqrt(1 - qz²)                  = 0.70710731
→ q = [0, 0, 0.7071063, 0.7071073]  ≈ 绕 Z 轴 +90°
```

### 5.2 与骨骼参考姿态对比 —— **差一个 +90° Z**

`bone[0]` @ `664` = `0x298`：

| 字段 | 偏移 | 值 |
|---|---|---|
| `pos` | 0x2B8 | `[0, 0, 0]` |
| `quat` | 0x2C4 | `[0, 0, 0, 1]`（单位四元数） |
| `rot` | 0x2D4 | `[0, 0, 0]` |

原始字节（`0298` 起）：

```
02b8  00 00 00 00 00 00 00 00 00 00 00 00   ← pos
02c4  00 00 00 00 00 00 00 00 00 00 80 3f   ← quat = (0,0,0,1)
02d0  00 00 00 00 00 00 00 00 00 00 00 00   ← rot = (0,0,0)
```

**动画四元数 `[0,0,0.7071,0.7071]` = 绕 Z +90°，骨骼参考姿态是单位四元数。二者不等。**

### 5.3 偏置的验证

`eulerToQuat(rot + [0,0,π/2]) = eulerToQuat([0,0,π/2]) = [0, 0, 0.70710678, 0.70710678]`
—— 与解码值 `[0, 0, 0.70710625, 0.70710731]` 一致（差 6e-7，即 Quaternion64 量化精度）。

**受控实验 `exp50`**（root 绕 X 转 0/10/20/30°，4 帧）：

```
main start=1632 frames=4 chain: b0(0x8)@1632 rotV=[6,0,16]@1636
   b0 rot.x @1642: vals=[0,10922,21844,32767]      → 0°, 10°, 20°, 30°   ✓ 与 SMD 一致
   b0 rot.z @1652: vals=[32767,32767,32767,32767]  → 恒定 90°           ← SMD 里 Z 恒为 0！
```

**`exp51`**（root 绕 Y）：`rot.y` 正常，`rot.z` 恒 90°。
**`exp53`**（root Z 0/−10/−20/−30°）：`vals=[32767,29126,25485,21844]` → 90°, 80°, 70°, 60° —— **确实是 SMD 值 + 90°**。
**`exp54`**（root Z 90/100/110/120°）：`vals=[24575,27305,30036,32767]` → 90°, 100°, 110°, 120° ✓。

**结论**：studiomdl 在写根骨骼（`parent < 0`）时，**给 Z 轴无条件加 +90°**。因为通常它是常量，rotscale 取 π/2 下限，整条 Z 流被压成「`valid=1, total=N`」。

> **实现必须复刻**，否则所有根骨骼朝向差 90°。

### 5.4 `exp2` 的旁证

`exp2`（3 骨骼 1 帧，姿态非平凡）实测链仍只有 `bone 0` 的 `RAWROT2` —— 因为**该动画只有 1 帧，所有骨骼的逐帧变化都是 0**，所以每根骨骼都退化成常量，被折叠进骨骼表。这再次印证 §3.2：**「常量」而非「帧数」决定编码形态**。

---

## 6. `animdesc.baseptr` 为什么是负数

最小模型：`animdesc[0]` @ `964`，`baseptr = -964`。

`studio.h` 的语义：

```c
inline studiohdr_t *pStudiohdr( void ) const { return (studiohdr_t *)(((byte*)this) + baseptr); }
```

**相对 animdesc 自身**，`-964` → 文件偏移 **0** = `studiohdr_t` 起点。

**规则：`baseptr = -animdesc_self_offset`。**

| 模型 | `animdesc[i]` 偏移 | `baseptr` | 解析到 |
|---|---|---|---|
| `myprop.mdl` | 964 | −964 | 0 ✓ |
| `exp5.mdl` | 1532 | −1532 | 0 ✓ |
| `exp67.mdl` animdesc[0] | 1472 | −1472 | 0 ✓ |
| `exp67.mdl` animdesc[1] | 1572 | −1572 | 0 ✓ |

同理 `mstudioseqdesc_t.baseptr` 也相对自身，= `-seqdesc_self_offset`（最小模型 `seqdesc[0]` @ 1088，`baseptr = -1088` ✓）。

> 这是「运行时把 studiohdr 指针贴回来」的序列化残留。**写出器只需写 `-self_offset`。**

---

## 7. `seqdesc.animindexindex`

最小模型：`seqdesc[0]` @ `1088`，`animindexindex = 216` → 绝对 `1304`。

该处 8 字节实测：`00 00 00 00 7f 01 00 00`

- 前 2 字节 `00 00` = **int16 值 0** = `animdesc[0]` 的下标；
- 后 2 字节是第二个元素（`groupsize = [1,1]`，只用 1 个）；
- 再往后 `7f 01` 属于下一个结构。

**结论：`animindexindex` 指向 `int16[numblends]`，元素是 animdesc 下标（index），不是字节偏移。**

**`exp67`**（2 个序列，各 1 个 blend）：

| seqdesc | `animindexindex` | 绝对偏移 | 该处 int16 | 含义 |
|---|---|---|---|---|
| `SEQ[0] 'seqA'` | 436 | 2264 | `[0]` | → `animdesc[0]` |
| `SEQ[1] 'seqB'` | 228 | 2268 | `[1]` | → `animdesc[1]` |

**`v_autoshotgun.mdl`** 的 blend 序列：

| seqdesc | `numblends` / `groupsize` | 值 |
|---|---|---|
| `SEQ[0] 'look_poses'` | 3 / `[3,1]` | `[2, 3, 4]` |
| `SEQ[2] 'idle'` | 3 / `[3,1]` | `[1, 0, 1]` |

**注意**：`numblends == groupsize[0] * groupsize[1]`，数组长度 = `numblends` 个 int16。

---

## 8. section 表

### 8.1 计数公式与走法

```
sectionCount = numframes / sectionframes + 2        // 整数除法
```

**当 `sectionindex == 0` 或 `sectionframes == 0` 时没有 section 表**，整条链在 `animdesc.off + animindex`，覆盖 `numframes` 帧。

否则 section 表在 `animdesc.off + sectionindex`，是 `mstudioanimsections_t[sectionCount]`，每项 8 字节：

```c
struct mstudioanimsections_t { int animblock; int animindex; };
```

链的实际位置：

```
base     = animdesc.off + animdesc.animindex
sec0Anim = sections[0].animindex
链偏移    = base + sections[s].animindex - sec0Anim
```

**实测：`v_autoshotgun.mdl` 的 `animdesc[5] 'a_deploy'`**

```
numframes = 30, sectionframes = 30, sectionindex = 145748, animindex = 145780
sectionCount = 30/30 + 2 = 3
section 表 @169664 (0x296c0)，24 字节：
296c0  00 00 00 00 | 74 39 02 00    → animblock=0, animindex=145780
296c8  00 00 00 00 | cc 56 02 00    → animblock=0, animindex=153292
296d0  00 00 00 00 | 24 59 02 00    → animblock=0, animindex=153892
```

| section | animblock | animindex | 绝对链偏移 | framesInSection |
|---|---|---|---|---|
| 0 | 0 | 145780 | **169696** | 30 |
| 1 | 0 | 153292 | 177208 | `30 − (3−2)×30` = **0** |
| 2 | 0 | 153892 | 177808 | **0** |

**section 0 的 `animindex` 恰好等于 `animdesc.animindex`（145780）** —— 所以 `base + animindex − sec0Anim = base`。这是个可依赖的简化。

**section 1/2 是垃圾**：`framesInSection = 0`，且实测 `sec1` 与 `sec0` 的头 48 字节**逐字节相同**（见 `out_report_sections.txt`）。**读取时必须跳过最后两个 section。**

### 8.2 `sectionframes` 何时启用（受控实验，共 32 个样本）

`sectionframes` 默认 **30**。但**只有 `numframes >= 4 × sectionframes` 时才真正启用**：

| 实验 | `numframes` | `sectionframes` | `sectionindex` | 说明 |
|---|---|---|---|---|
| `s016` … `s100` | 16…100 | **0** | **0** | 100 < 4×30 = 120 → 不启用 |
| `s120` | 120 | 30 | 100 | 120 ≥ 120 → **启用** |
| `s150` / `s200` | 150 / 200 | 30 | 100 | 启用 |
| `exp5` / `exp39` / `exp60` | 40 / 100 / 40 | **0** | **0** | 不启用 |
| `exp40` | 260 | 30 | 100 | 启用 |
| `t01` | 40 | 30 | 100 | **显式 `$sectionframes 30 0` 强制启用** |
| `t03` | 40 | **0** | **0** | `$sectionframes 30 100` → `40 < 100`，**禁用** |
| `t07` | 40 | 10 | 100 | `$sectionframes 10 0` → `40 ≥ 40`，启用 |
| `t08` | 39 | 10 | 100 | `$sectionframes 10 0` → `39 < 40`，**仍启用** |

**`v_autoshotgun.mdl` 的对照**（其 QC `example_1.qc` / `example_11.qc` **没有** `$sectionframes` 行，即用默认 30）：

| 动画 | `numframes` | `fps` | `sectionframes` | `sc` |
|---|---|---|---|---|
| `a_idle` | 402 | 60 | 30 | 15 |
| `a_run` | 401 | 60 | 30 | 15 |
| `a_look_down` | 1 | 30 | **0** | 0 |
| `a_look_mid` | 90 | 1 | 30 | 5 |
| `a_look_up` | 1 | 30 | **0** | 0 |
| `a_deploy` | 30 | 30 | 30 | 3 |
| `a_reload` | 20 | 30 | **0** | 0 |
| `a_helping_hand_loop` | 121 | 60 | 30 | 6 |
| `a_fidget` | 121 | 60 | 30 | 6 |
| `a_fidget2` | 181 | 60 | 30 | 8 |

**注意 `a_deploy`（30 帧）有 section 表，而 `s030`（30 帧，我的实验）没有。** 说明判定不只是帧数 —— 极可能是**动画数据的字节体积**（`a_deploy` 89 骨骼、550 字节链 vs `s030` 3 骨骼、几十字节）。**精确判据未验证（U6）**。

**对写出器的建议**：直接写 `sectionindex = 0, sectionframes = 0`（不分段）。这是合法且被 60+ 个实测样本验证的形态。

### 8.3 `$sectionframes` 语法

`$sectionframes <count> <minFrameCount>`（**两个参数**，缺第二个报 `Line 5 is incomplete`）。实测 `$sectionframes 10 0` / `$sectionframes 30 0` 生效，`$sectionframes 30 100` 会禁用分段。

---

## 9. `seqdesc.bbmin` / `bbmax`

### 9.1 结论

`bbmin/bbmax` = **用该序列自己的动画数据（逐帧骨骼姿态）蒙皮 VVD 顶点，取所有帧的 AABB 并集**。

### 9.2 精确匹配的证据

`exp5`（3 骨骼，40 帧，bone1 Z 0→117°，bone2 X 0→78°）：

| 来源 | bbmin | bbmax |
|---|---|---|
| studiomdl `seqdesc[0]` | `[-13.6640, -13.6879, -11.3015]` | `[13.6640, 13.6879, 11.3015]` |
| 用 MDL 自己的动画数据蒙皮（`bbox2.js`） | `[-13.6642, -13.6878, -11.3015]` | `[13.6642, 13.6878, 11.3015]` |

**误差 ≤ 2e-4。**

其余精确匹配的样本：

| 实验 | studiomdl `bbmin .. bbmax` | 蒙皮计算 |
|---|---|---|
| `exp35` | `[-11.2707, -8, -11.2707] .. [11.2707, 8, 11.2707]` | 完全相同 |
| `exp9` | `[-11.2871, -11.2871, -8] .. [11.2871, 11.2871, 8]` | 完全相同 |
| `exp39` | `[-11.3137, -11.3137, -8] .. [11.3137, 11.3137, 8]` | 完全相同 |
| `exp40` | `[-11.3137, -11.3137, -8] .. [11.3137, 11.3137, 8]` | 完全相同 |

### 9.3 不匹配的情形（未验证）

| 实验 | studiomdl | 蒙皮计算 | 差异 |
|---|---|---|---|
| `exp2`（1 帧） | `[-19.5308,-18.7128,-20.4949] .. [19.5308,18.7128,20.4949]` | `[-19.1786,-13.0515,-16.8842] .. [10.8421,12.1035,14.8189]` | studiomdl **关于原点镜像对称** |
| `exp42`（恒定位移） | `[-20,-8,-8] .. [8,10,30]` | `[-28,-8,-8] .. [8,18,38]` | 差 8 = 立方体半边长 |
| `exp31` / `exp41` | 略大 | 略小 | 系统性差异 |

**结论**：`bbmin/bbmax` 的主规则已确认（蒙皮并集），但**1 帧模型与部分常量姿态模型走了不同路径**（疑似「骨骼包围盒」或含取整/收缩步骤）。**精确规则未验证（U4）**。

**实现建议**：用蒙皮并集（对多帧模型精确）；1 帧模型可接受小偏差。

### 9.4 最小模型对照

`seqdesc[0].bbmin = [-8,-8,-8]`、`bbmax = [8,8,8]`（实测原始字节 `0460`：`00 00 00 c1`×3 = −8.0，`00 00 00 41`×3 = +8.0）。

立方体在原点、边长 16、骨骼无位移 → 蒙皮并集 = `[-8,-8,-8] .. [8,8,8]` ✓。

> 任务描述提到的 `-0.5` 与 `4.0` **未在实测中出现**；实测是 ±8。

### 9.5 最小模型 `seqdesc` 其余字段

| 字段 | 实测值 | 含义 |
|---|---|---|
| `flags` | `0x1` | `STUDIO_LOOPING`（QC 里有 `loop`） |
| `activity` | `-1` | QC 未指定 `activity` → 无活动关联 |
| `actweight` | `0` | 未指定 |
| `paramindex[2]` | `[-1, -1]` | 无 pose parameter |
| `groupsize[2]` | `[1, 1]` | 单 blend |
| `numblends` | `1` | |
| `fadeintime` / `fadeouttime` | `0.2` / `0.2` | **默认值**（实测所有未指定 fade 的序列都是 0.2） |
| `numevents` / `eventindex` | `0` / `212` | 无事件（`eventindex` 仍写了结构末尾偏移，是 studiomdl 的固定布局） |
| `lastframe` | `0` | |

---

## 10. FRAMEANIM / zeroframe / animblock —— **L4D2 不产出**

### 10.1 全盘扫描结果

`scan_all.js` 扫描 `E:\SteamLibrary\steamapps\common` 与 `D:\GITHUB` 下全部 **152 个 `.mdl`**，其中 **119 个有动画**（共 191 个 animdesc）：

```
### frameAnim: 0
### zero: 0
### saveframe: 0
### animblock: 0
### sections: 6
```

**L4D2 的 studiomdl 完全不写 `STUDIO_FRAMEANIM`、zeroframe、saveframe、animblock。**

### 10.2 针对性触发实验（全部失败）

| 实验 | QC | 结果 |
|---|---|---|
| `f01` | `$sequence idle ... frameanim` | 报错/忽略，`flags = 0x0` |
| `f02` | `... frame_anim` | 同上 |
| `f03` | `... frameanim 1` | 同上 |
| `f04` | `... realtime` | `flags = 0x0` |
| `f05` | `... delta` | `flags = 0x0` |
| `f07` | `$sectionframes 4 0` | 有 section 表，但仍是 RLE |
| `f08` | `$bonesaveframe "b2" position rotation` | **骨骼 flags 仍为 0x500**，无 saveframe |
| `f09` | `$bonesaveframe "b2" rotation` | 同上 |
| `f10` | `$bonesaveframe "b1" position rotation` | 同上 |

`$bonesaveframe` 语法确认为 `$bonesaveframe "<boneName>" position rotation`（`position`/`rotation` 是 flag 名，**不是数字** —— `"b1" 2` 报 `unknown option "2"`）。但即便语法正确，**L4D2 的 studiomdl 也没有置位 `BONE_HAS_SAVEFRAME_POS/ROT`**。

### 10.3 FRAMEANIM 的字段（仅来自 SDK 头文件，**未实测**）

`hl2sdk-csgo/public/studio.h:710` 有结构定义：

```c
struct mstudio_frame_anim_t {
    inline byte *pBoneFlags() const { return ((byte*)this) + sizeof(struct mstudio_frame_anim_t); };
    int  constantsoffset;   // 相对自身
    int  frameoffset;       // 相对自身
    int  framelength;       // 每帧字节数
    int  unused[3];
};
// sizeof = 24 字节 (0x18)
```

标志位（`hl2sdk-csgo/public/studio.h:703`）：

```c
#define STUDIO_FRAME_RAWPOS       0x01  // Vector48 in constants
#define STUDIO_FRAME_RAWROT       0x02  // Quaternion48 in constants
#define STUDIO_FRAME_ANIMPOS      0x04  // Vector48 in framedata
#define STUDIO_FRAME_ANIMROT      0x08  // Quaternion48 in framedata
#define STUDIO_FRAME_FULLANIMPOS  0x10  // Vector in framedata
```

> **`0x20` / `0x40` / `0x80`（`CONST_POS2` / `CONST_ROT2` / `ANIM_ROT2`）只出现在 Crowbar 的反推文档里，L4D2/CSGO 的公开 SDK 头文件中没有定义。** 任务描述中「`STUDIO_FRAMEANIM = 0x40` 帧主序编码，布局见 `mdl-layout.md` §3.3」属于 Crowbar 的读法，**本文无法实测验证**。
>
> **本文对 FRAMEANIM 的立场：布局按 SDK 头文件 + Crowbar 读法记录，标注「未验证」；L4D2 写出器无需实现。**

### 10.4 zeroframe 字段（**未实测**）

`animdesc` 的 `zeroframespan`(u16@0x58) / `zeroframecount`(u16@0x5A) / `zeroframeindex`(@0x5C) / `zeroframestalltime`(float@0x60) 在**全部 119 个有动画的 .mdl 里都是 0**，最小模型也是 0。

> 任务描述问「真实模型里 `zeroframespan` / `zeroframecount` 是多少」—— **实测答案是：`v_autoshotgun.mdl` 的 37 个动画也全是 0。**

---

## 11. 实现建议：从 SMD 多帧 skeleton 生成动画数据

### 11.1 最简可行路径（MVP）

**核心洞察：可以不实现 RLE 压缩。** `valid == total` 的「RLE」在语义上就是「逐帧完整数据」，任何合规解码器都必须正确处理。

```
1. 读 SMD 的 N 帧 skeleton。
2. 对每根骨骼 b、每轴 k：
     a. 旋转增量：
          d[f] = wrapToPi(smd_rot[b][f][k] - smd_rot[b][0][k] + (k==2 && b.parent<0 ? π/2 : 0))
          若 (seq.flags & LOOPING): d[N-1] = 0
     b. 位移增量：
          e[f] = smd_pos[b][f][k] - smd_pos[b][0][k]
          若 (seq.flags & LOOPING): e[N-1] = 0
     c. 缩放：
          rotscale[b][k] = f32( max(k==2 && b.parent<0 ? π/2 : π/8, max|d|) / 32767 )
          posscale[b][k] = f32( max(128.0, max|e|) / 32767 )
     d. 量化：q[f] = clamp(round(d[f] / rotscale[b][k]), -32768, 32767)
3. 写 mstudiobone_t：quat/rot 用常量姿态；posscale/rotscale 如上。
4. 对每个序列：
     若某骨骼所有轴 d[] 全 0 且 e[] 全 0 → 该骨骼不进链
     否则写一条 mstudioanim_t：
       flags = 0x08 | 0x04            (ANIMROT | ANIMPOS)
       rotV offset[k] = 有数据的轴 → 指向该轴流；无数据 → 0
       posV 同理
       每轴流 = { u8 valid=N; u8 total=N; int16 sample[N] }
5. 链尾：最后一条记录 nextoffset = 0，紧跟 1 条 4 字节全零记录。
   若该动画完全无骨骼动画 → 写 numbbones 条 ff 00 00 00。
6. sectionindex = 0, sectionframes = 0（不分段）。
7. zeroframespan / zeroframecount / zeroframeindex / zeroframestalltime = 0。
8. seqdesc.bbmin/bbmax = 蒙皮所有帧的顶点 AABB 并集。
```

**为什么可以不分段**：`sectionindex == 0 && sectionframes == 0` 是合法且被 60+ 个实测样本验证的形态，Source 引擎读取路径简单。

### 11.2 代价：文件变大多少

以 `v_autoshotgun.mdl` 的 `a_deploy`（30 帧，89 骨骼）为基准：

| 项目 | studiomdl 实际 | MVP（`valid=total=N`） |
|---|---|---|
| RLE 流条数 | ~180 条 | ~180 条 |
| 每条流字节数 | 平均约 30 B（含多 run 压缩） | `2 + 2×30 = 62` B |
| 该动画动画数据总量 | **550 B**（实测 chain bytes） | 约 **11.2 KB** |
| 膨胀比 | — | **约 20×** |

**换算到整个模型**：`v_autoshotgun.mdl` 的动画区约 `619136 − 27248 ≈ 592 KB`。按 20× 膨胀 → 约 **12 MB**。

**更温和的中间方案（推荐）：只做「常量折叠」，不做 run 合并。**

| 通道类型 | studiomdl | 中间方案 | 字节数 |
|---|---|---|---|
| 全常量（旋转） | `RAWROT2` 8B | `RAWROT2` 8B（或写进骨骼表 + 不进链） | 8 / 0 |
| 全常量（位移） | `valid=1, total=N` | `valid=1, total=N` | 4 |
| 逐帧变化 | 多 run 压缩 | `valid=N, total=N` | `2 + 2N` |

实测 `a_deploy` 的 RLE 形态统计显示 **53/180 条流本来就是 `30/30`（无法再压）**，**约 26 条是「常量折叠」形态**。所以：

- **只做常量折叠**：省掉约 14% 的流；膨胀比约 **4–6×**（整模型约 2.5–3.5 MB）。
- **完全不做压缩**：膨胀约 **20×**（整模型约 12 MB）。

> **建议 MVP 先做「常量折叠 + `valid=total=N`」**：实现量极小，覆盖 studiomdl 的绝大多数实际收益，产物 100% 可被引擎正确读取。

### 11.3 必须实现的细节清单（否则产物错误）

| # | 细节 | 漏掉的后果 |
|---|---|---|
| 1 | 根骨骼 Z **+90° 偏置** | 所有根骨骼朝向差 90° |
| 2 | 逐帧值 = **相对第 0 帧的增量**（不是绝对值） | 姿态整体偏移 |
| 3 | `wrapToPi` 包裹 | 超过 ±180° 时跳变 |
| 4 | `rotscale` 下限 **π/8**、根 Z 下限 **π/2**、`posscale` 下限 **128** | 静止骨骼量化噪声放大 |
| 5 | 除数 **32767**（不是 32768） | 系统性 3e-5 相对误差 |
| 6 | `LOOPING` 时末帧强制 0 | 循环接缝跳变 |
| 7 | 常量骨骼不进链，但姿态要进 `mstudiobone_t` | 骨骼回落到错误姿态 |
| 8 | 链尾 1 条 4 字节全零记录 | 读取器可能多读/少读 4 字节 |
| 9 | `nextoffset` 相对**自身** | 链走错位 |
| 10 | `baseptr = -self_offset` | 运行时 `pStudiohdr()` 解引用崩溃 |
| 11 | `animindexindex` 写 animdesc **下标** | 序列指向错误动画 |
| 12 | 每骨骼的 `posscale`/`rotscale` 是**全局一份**（在 bone 表里） | 每个动画重复写 / 解码时找不到 |

---

## 12. 未验证项清单

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| U1 | `STUDIO_FRAMEANIM` 实际字节布局、`framelength` 自洽性 | **未验证** | L4D2 studiomdl 不产出；全盘 152 个 .mdl 无样本；10 个 QC 触发实验全部失败。仅 SDK 头文件有结构定义 |
| U2 | `zeroframespan`/`zeroframecount`/`zeroframeindex` 数据布局 | **未验证** | 全盘 119 个有动画的 .mdl 中该四字段恒为 0；`$bonesaveframe` 不置位 `BONE_HAS_SAVEFRAME_*` |
| U3 | `animblock`（外置 `.ani`）路径 | **未验证** | 全盘 `numanimblocks == 0` |
| U4 | `bbmin/bbmax` 在 1 帧 / 常量姿态模型上的精确规则 | **部分验证** | 多帧模型精确匹配蒙皮并集；`exp2`/`exp31`/`exp41`/`exp42` 不匹配，疑似有取整/收缩或改用骨骼包围盒 |
| U5 | RLE run 边界的判定阈值 | **未验证** | 只观测到结果形态（§4.3） |
| U6 | `sectionframes` 自动启用的精确判据 | **部分验证** | 确认「帧数 ≥ 4×sectionframes」是必要条件（`s100` 无、`s120` 有；`t01` 强制、`t03` 禁用）；但 `a_deploy`（30 帧、89 骨骼）有 section 而 `s030`（30 帧、3 骨骼）没有，说明还有体积/骨骼数因子。`$sectionframes` 第二参数的确切语义未逐点测定 |
| U7 | `STUDIO_FRAME_CONST_POS2(0x20)` / `CONST_ROT2(0x40)` / `ANIM_ROT2(0x80)` | **未验证** | 仅见于 Crowbar 反推文档，L4D2/CSGO 公开 SDK 头文件中无定义 |
| U8 | `animdesc` `+0x1C`（v49 `ikrulezeroframeindex`） | **未验证** | 实测所有样本恒为 0 |
| U9 | 多 blend / pose parameter 的端到端写出 | **未验证** | 只验证了 `animindexindex` 的 int16 数组语义（§7） |
| U10 | 空链中 `ff 00 00 00` 记录的**确切条数规则** | **未验证** | `a_look_mid` 实测 5 条（`numbones = 89`）。建议写出 `numbones` 条，读取时容忍任意条数 |

---

## 附录 A：复现脚本

全部位于 `D:\GITHUB\mdlc\docs\_probe\`（**未修改 `src/` 下任何文件**）：

| 脚本 | 作用 |
|---|---|
| `mdl.js` | MDL 解析库 + 压缩原语（Quaternion64/48/48S/32、Vector48、float16）+ 四元数/欧拉数学 |
| `gen.js` … `gen12.js` | 生成实验用 SMD + QC |
| `build_all.ps1` | 批量调用 `studiomdl.exe` 编译，产物存 `artifacts/` |
| `analyze.js` | section 走法 + anim_t 链 + RLE 解码（带 run trace） |
| `summarize.js` | 单模型动画结构总览 |
| `deep.js` | 单个 animdesc 的完整逐字节 + RLE dump |
| `verify_model.js` | **端到端验证**：从 SMD 预测 rotscale/posscale/逐帧值，与 MDL 实测严格比对 |
| `diag_scale.js` | 打印 `maxAbs / scale` 以确定缩放常数 |
| `chainend2.js` | 链结束形态统计（125 个链） |
| `empty_chain.js` | 空链 `ff 00 00 00` 记录计数 |
| `bbox2.js` | 用 MDL 自己的动画数据蒙皮 VVD 顶点，与 `seqdesc.bbmin/bbmax` 比对 |
| `survey.js` / `seqs.js` / `scan_all.js` | 模型特征统计与全盘扫描 |
| `report_data.js` | 生成本文引用的字节注解 |
| `out_*.txt` | 各步骤的原始输出留档 |

## 附录 B：实验样本清单（128 个成功编译）

| 前缀 | 数量 | 覆盖内容 |
|---|---|---|
| `exp2`–`exp9` | 8 | 基础：多骨骼、多帧、零帧、单轴动画 |
| `exp20`–`exp28` | 9 | 单轴角度扫描（0–180°） |
| `exp31`–`exp43` | 13 | loop 语义、根骨骼范围、位移缩放、section、常量骨骼 |
| `exp50`–`exp55` | 6 | 根骨骼 X/Y/Z 偏置验证 |
| `exp60`–`exp67` | 8 | loop vs 非 loop 对照、多序列 / `animindexindex` |
| `r01`–`r15` | 15 | 斜坡扫描（帧数 × 步长 × 起始角 × loop） |
| `q01`–`q21` | 21 | 精确范围扫描（确定 rotscale 常数与下限） |
| `s016`–`s200` | 32 | section 启用阈值扫描（16–200 帧） |
| `t01`–`t08` | 8 | `$sectionframes` 两参数语义 |
| `b*f*` | 11 | 骨骼数 × 帧数 对照 |
| `prec6`–`prec15` | 4 | SMD 文本精度对 rotscale 的影响（证明公式精确） |
| `f01`–`f10` | 6 | FRAMEANIM / saveframe 触发尝试（全部失败） |
| `exp1` | 1 | 最小模型复现（动画区逐字节一致） |
