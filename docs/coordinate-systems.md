# studiomdl 的坐标系与 `$staticprop` 几何旋转

本文记录 L4D2 `studiomdl.exe` 的**坐标系约定**，以及由此引出的
`$staticprop` 几何旋转规则。全部结论都有**三重证据**：Source 源码、
受控实验（studiomdl 实跑）、以及 3333 个真实模型的语料统计。

---

## 1. 全局默认旋转 `g_defaultrotation`

`studiomdl.cpp:6883` 在初始化时设下：

```cpp
g_currentscale = g_defaultscale = 1.0;
g_defaultrotation = RadianEuler( 0, 0, M_PI / 2 );
```

即**绕 Z 轴 +90°**。`RadianEuler` 在 Source 里是 `[roll, pitch, yaw]`，
所以这里 `yaw = π/2`、`roll = pitch = 0`。

`AngleMatrix(RadianEuler(0,0,π/2))` 展开后是：

```
        | 0  -1   0 |
Rz(90°) =| 1   0   0 |
        | 0   0   1 |
```

作用在点上就是：

```
(x, y, z) -> (-y, x, z)          Z 分量不变
```

> 源码注释解释这个约定为「rotate points into frame of reference so
> g_model points down the positive x axis」，但同一处也坦承
> 「FIXME: these coords are bogus」。

---

## 2. `$eyeposition` / `$illumposition` 的轴变换

`studiomdl.cpp:845-882` 对这两个命令做**完全相同**的轴变换：

```cpp
void Cmd_Eyeposition (void)
{
// rotate points into frame of reference so g_model points down the positive x
// axis
	//	FIXME: these coords are bogus
	GetToken (false);
	eyeposition[1] = verify_atof (token);

	GetToken (false);
	eyeposition[0] = -verify_atof (token);

	GetToken (false);
	eyeposition[2] = verify_atof (token);
}
```

注意它**不是**按顺序读进 `[0] [1] [2]`，而是把第 1 个 token 写进
`[1]`、第 2 个 token 取负写进 `[0]`、第 3 个 token 写进 `[2]` —— 结果
正是 `(x,y,z) -> (-y,x,z)`，与上面的 `Rz(90°)` 一致。

`Cmd_Illumposition`（`studiomdl.cpp:867-882`）逐字相同，唯一区别是最后
多置一个 `illumpositionset = true`。

### 受控实验（第一轮）

`docs/_probe/gen_illumpos.js` 故意给三个轴**各不相同**的值：

| 输入 | 产物 |
|---|---|
| `$eyeposition 4 5 6` | `eyeposition = [-5, 4, 6]` |
| `$illumposition 1 2 3` | `illumposition = [-2, 1, 3]` |

### 受控实验（第二轮，基向量）

两点拟合无法区分「`Rz(90°)`」与其它恰好在两点上取值相同的映射，
所以补了基向量实验 `docs/_probe/gen_illumpos_basis.js`。
四个探针**全部命中**：

| 输入 | 预期 | 实测 |
|---|---|---|
| `illum (1,0,0)` | `(0,1,0)` | `(0,1,0)` ✅ |
| `eye (0,1,0)` | `(-1,0,0)` | `(-1,0,0)` ✅ |
| `illum (0,0,1)` | `(0,0,1)` | `(0,0,1)` ✅ |
| `eye (2,7,-3)` | `(-7,2,-3)` | `(-7,2,-3)` ✅ |

第三、四项同时确认了 **Z 不变**与**线性性**。

实现见 `src/mdl_writer.rs` 的 `qc_axis_to_model()`。

---

## 3. `$illumposition` 缺省时的回退规则

`simplify.cpp:7204` 的 `SetIlluminationPosition()`：

```cpp
void SetIlluminationPosition()
{
	// find center of domain
	if (!illumpositionset)
	{
		// Only use the 0th sequence; that should be the idle sequence
		VectorFill( illumposition, 0 );
		if (g_sequence.Count() != 0)
		{
			VectorAdd( g_sequence[0].bmin, g_sequence[0].bmax, illumposition );
			illumposition *= 0.5f;
		}
		illumpositionset = true;
	}
}
```

**不写 `$illumposition` 时，取第 0 个 sequence 的包围盒中心。**

### 关键细节：回退路径不做轴变换

回退发生在 `SetIlluminationPosition()` 里，**不经过** `Cmd_Illumposition`，
所以中心值**不再**被 `(x,y,z)->(-y,x,z)` 变换。

而 `write.cpp:2071-2087` 又把**同一个盒子**写进 `hull_min/hull_max`：

```cpp
	if ( !g_wrotebbox && g_sequence.Count() > 0)
	{
		VectorCopy( g_sequence[0].bmin, bbox[0] );
		VectorCopy( g_sequence[0].bmax, bbox[1] );
		CollisionModel_ExpandBBox( bbox[0], bbox[1] );
		VectorCopy( bbox[0], g_sequence[0].bmin );
		VectorCopy( bbox[1], g_sequence[0].bmax );
	}
	...
	phdr->hull_min = bbox[0];
	phdr->hull_max = bbox[1];
```

于是得到一个**可验证的推论**：凡是没写 `$illumposition` 的模型，

```
illumposition == (hull_min + hull_max) / 2
```

### 语料验证

`docs/_probe/verify_illum_fallback.js`（排除有 `.phy` 的模型，
因为 `CollisionModel_ExpandBBox` 会把碰撞盒 AABB 并进 hull、破坏等式）：

| 偏差 | 数量 |
|---|---|
| ≤ 0.1 | **768** |
| ≤ 0.5 | 2 |
| ≤ 2.0 | 2 |
| > 2.0 | 6 |

中位数偏差 **0.0000**，90 分位 **0.0000**。剩下 6 个偏差大的
（如 `cane_field_*.mdl`，`illum = [0,0,250]`）都是**写死了**
`$illumposition` 的，符合预期。

实现上用 `hull_min/hull_max` 求中心（而不是另算一遍顶点包围盒），
这样与 hull **天然自洽** —— 将来 hull 换成姿态包围盒时 illum 自动跟随。

---

## 4. `$staticprop`：几何本身被旋转

`$staticprop` 会让 studiomdl 调用 `MakeStaticProp()`
（`simplify.cpp:3268`），其中对**每个顶点**做：

```cpp
	AngleMatrix( g_defaultrotation, rotated );
	...
			// **shift everything into identity space**
			// position
			Vector tmp;
			VectorTransform( psource->vertex[j].position, rotated, tmp );
			VectorCopy( tmp, psource->vertex[j].position );

			// normal
			VectorRotate( psource->vertex[j].normal, rotated, tmp );
			VectorCopy( tmp, psource->vertex[j].normal );

			// tangentS
			VectorRotate( psource->vertex[j].tangentS.AsVector3D(), rotated, tmp );
			VectorCopy( tmp, psource->vertex[j].tangentS.AsVector3D() );
```

即**位置、法线、切线**都被 `Rz(90°)` 旋转。注释「shift everything into
identity space」说明意图：把几何转到单位空间，好让静态道具只有一根骨骼。

### 同时塌缩骨骼

```cpp
	strcpy( psource->localBone[0].name, "static_prop" );
	psource->localBone[0].parent = -1;

	for (k = 1; k < psource->numbones; k++)
	{
		psource->localBone[k].parent = -1;
	}
	...
			// attach everything to root
			psource->localBoneweight[j].bone[k] = 0;
```

所有骨骼 parent 置 -1、所有顶点权重归到骨骼 0，最终产出**单根**
名为 `static_prop` 的骨骼。

### 不做居中

`g_centerstaticprop` 在 `studiomdl.cpp:6900` 默认被设为 **false**：

```cpp
	g_centerstaticprop = false;
```

所以 `MakeStaticProp` 里 `if (g_centerstaticprop)` 那段
（`simplify.cpp:3325-3348`，计算 `g_PropCenterOffset` 并平移顶点）
**不会执行**。

> 注意 `studiomdl.cpp:908` 另有一处 `g_centerstaticprop = true`，
> 那是别的命令路径，不影响 `$staticprop` 的默认行为 —— 以实测产物为准。

### 受控实验：铁证

同一份 SMD 几何，只差 `$staticprop`：

```
输入 SMD 顶点: (0,0,0) (10,0,3) (0,20,7)     法线全为 (1,0,0)
Rz(90°) 之后:  (0,0,0) (0,10,3) (-20,0,7)     法线应为 (0,1,0)
```

| 产物 | VVD 顶点 | VVD 法线 |
|---|---|---|
| `ipa1.mdl`（**无** `$staticprop`） | `(0,0,0) (10,0,3) (0,20,7)` | `(1,0,0)` |
| `ipa2.mdl`（**有** `$staticprop`） | `(0,0,0) (0,10,3) (-20,0,7)` | `(0,1,0)` |

`ipa2` 与 `Rz(90°)` 的预测**逐分量吻合**，`ipa1` 则是原样。
生成脚本 `docs/_probe/gen_illumpos_ab.js`。

### 语料验证：骨骼塌缩

`docs/_probe/verify_staticprop_bones.js`，3333 个真实模型：

| 检查项 | 静态道具（2681 个） |
|---|---|
| `numbones == 1` | **2681 / 2681** |
| `bone[0].name == "static_prop"` | **2681 / 2681** |
| `bone[0].parent == -1` | **2681 / 2681** |

普通模型 652 个里，首骨骼叫 `static_prop` 的有 **0** 个 —— 无假阳性。

### 语料验证：几何旋转

`docs/_probe/verify_hull_mechanism.js` 给出干净的 2×2 列联表
（只取「无 `.phy` + 单帧动画 + X/Y 非对称」的样本）：

| 分组 | `hull == rot(VVD bbox)` | `hull == VVD 原样` |
|---|---|---|
| 静态道具（479） | **0** | **318** |
| 普通模型（217） | **15** | **0** |

**零交叉污染**：静态道具的 VVD 顶点**已经**被旋转过，所以 hull
相对它「原样」；普通模型几何不转，所以 hull 相对它「多转 90°」。
这正是第 5 节要解释的现象。

---

## 5. 为什么 `hull` 与 VVD 顶点「不在同一坐标系」

这是本项目早期的一个困惑：受控实验里 hull 看起来比 VVD 顶点多转了
90°。原因不是存在什么隐藏变换，而是**两条不同的路径**：

- **普通模型**：几何**不**旋转（VVD 是 SMD 原样），但动画的参考旋转是
  `g_defaultrotation`。`CalcSequenceBoundingBoxes()`（`simplify.cpp:7049`）
  用 `AngleMatrix(sanim[j][k].rot, ...)` 逐级 `ConcatTransforms` 算姿态
  变换，于是姿势包围盒天然比静止姿势多转 90°。
  → `hull == rot(VVD bbox)`。

- **静态道具**：`MakeStaticProp()` 把**几何本身**转了 90°，骨骼又塌缩
  成一根，所以姿态变换退化为单位矩阵。
  → `hull == VVD bbox 原样`。

另外，凡是有 `.phy` 的模型，`CollisionModel_ExpandBBox()` 会把碰撞盒的
AABB（在引擎坐标系里算的）**并进** hull，使 hull 既不等于「原样」也不
等于「旋转后」—— 这解释了早期语料统计里大量的「两者都不是」。做这类
判定时必须排除有 `.phy` 的模型。

---

## 6. 对 mdlc 的影响

| 项 | 状态 |
|---|---|
| `$eyeposition` / `$illumposition` 轴变换 | ✅ 已实现（`qc_axis_to_model`） |
| `$illumposition` 缺省回退到 seq0 包围盒中心 | ✅ 已实现 |
| `$staticprop` 几何旋转 `Rz(90°)` | ✅ **已实现**（`compile::static_prop_rotate`） |
| `$staticprop` 骨骼塌缩成单根 `static_prop` | ✅ **已实现**（`compile::collapse_static_prop`） |
| `$staticprop` 动画塌陷（1 anim / 1 帧） | ✅ **已实现**（`anim_writer::anim_specs`） |
| `$staticprop` 附着点 `local` 左乘旋转 | ✅ **已实现** |
| 自动 hitbox（`SetupHitBoxes`） | ✅ **已实现**（`compile::auto_hitboxes`） |
| `$skipboneinbbox`（bbox 起点 ±9999） | ✅ **已实现**（`[model].skip_bone_in_bbox`） |
| hull / seqdesc 改用**姿态**包围盒 | ✅ **已实现**（`compile::sequence_pose_bounds`） |

`$staticprop` 影响 **80.4%** 的真实模型（2681/3333）。

---

## 7. `$staticprop` 的实现细节（本轮补齐）

### 7.1 顶点级：旋转 + 权重归零

`compile.rs` 的 [`smd_vertex_to_ir`] 在构造顶点时直接产出旋转后的值：

```rust
if desc.model.static_prop {
    return Ok(Vertex {
        pos: static_prop_rotate(sv.position),
        normal: static_prop_rotate(sv.normal),
        uv: sv.uv,
        bones: vec![[0.0, 1.0]],
    });
}
```

**为什么放在这里而不是编译末尾做后处理**：`unify_lods` 的去重键
**包含骨骼绑定**。若先统一、再改权重，那些「原本只差绑定」的顶点
已经各自占了池里一个位置，改完权重后会出现**重复顶点**，
`numLODVertexes` 与实际存储量对不上。studiomdl 的顺序也是如此 ——
`MakeStaticProp()` 在 `RemapBones()` 里，早于 `UnifyLODs()`。

权重直接写「骨骼 0、权重 1.0」：原权重和恒为 1，全部归到骨骼 0
之后总权重仍是 1，语义与逐项置 0 等价。

### 7.2 骨骼表：塌缩成单根

`simplify.cpp:3362-3366` 把骨骼 0 的姿态强制成单位：

```cpp
psource->rawanim[0][0].pos = Vector( 0, 0, 0 );
psource->rawanim[0][0].rot = RadianEuler( 0, 0, 0 );
AngleMatrix( QAngle( 0, 0, 0 ), psource->boneToPose[0] );
```

**实测确认**（`ipg` 实验）：即便 SMD 的 root 带非零姿态
（pos `(5,6,7)`、rot `(0.1,0.2,0.3)`），产物 `static_prop` 仍是
`pos=[0,0,0] rot=[0,0,0] poseToBone=单位矩阵`。

### 7.3 动画：`numlocalanim=1` 但 `numlocalseq` 保留

**这是最容易漏的一条。** `simplify.cpp:3374-3380`：

```cpp
// throw away all animations
g_numani = 1;
g_panimation[0]->numframes = 1;
g_panimation[0]->startframe = 0;
g_panimation[0]->endframe = 1;
g_panimation[0]->rotation = RadianEuler( 0, 0, 0 );
g_panimation[0]->adjust = Vector( 0, 0, 0 );
```

它压的是 `g_numani`（→ `numlocalanim`），**不动** `g_sequence`
（→ `numlocalseq`）。受控实验 `ipe2`（2 条序列 + `$staticprop`）：

| 字段 | 无 `$staticprop`（ipe1） | 有（ipe2） |
|---|---|---|
| `numlocalanim` | 2 | **1** |
| `numlocalseq` | 2 | **2** |
| `anim[0].numframes` | 2 | **1** |
| `seq[0]` blend → anim | 0 | 0 |
| `seq[1]` blend → anim | 1 | **1（悬垂，官方原样保留）** |
| `seq[1].bbmin/bbmax` | 有值 | **全 0** |

语料里唯一的「静态道具 + 多序列」样本
`c5m3_overpass_explosion\smalldebris_part_baked_setsexp.mdl` 更极端：
**1 个 animdesc 对 5 个 seqdesc**，`seq[0]` 有真实包围盒、
`seq[1..4]` 全是 `[0,0,0]`。

> `seq[1]` 的 blend 值写 `1` 而 `numlocalanim==1`，是**越界**的 ——
> 官方产物确实如此，照抄即可（引擎不会去解引用它）。

**实现上的三处改动**（早先的代码假设 `animdesc[i] ↔ seqdesc[i]` 一一对应）：

1. `layout.rs` 的 `SectionCounts` 拆出 `seqs` 字段
   （`localanim + anims*100` → `localseq` 用 `seqs*212`）；
2. `anim_writer.rs` 新增 `AnimSpec`，动画链与 animdesc 按 `specs`
   写、seqdesc 按 `sequences` 写；
3. `mdl_writer.rs` 把回填循环拆成两个（原先用同一个
   `compiled.sequences` 同时驱动两者，静态道具上会越界）。

### 7.4 动画链是 **4 字节**，不是 16

静态道具的动画数据块**恰好 4 字节**：一条
`bone=255 / flags=0 / nextoffset=0` 的记录。

> **踩过的坑**：最初我在 `ad.off + ad.animindex` 处 dump 了 16 字节，
> 看到 `ff 00 00 00 | cc fb ff ff | 5d 02 00 00 | 3c 02 00 00`，
> 误以为后 12 字节是「垃圾 valueptr」，还据此推断出一个「246 种取值」
> 的伪规律。实际上那 12 字节**是 `seqdesc[0]` 自己的字段** ——
> `cc fb ff ff` = `-1076` = `localseqindex` 的相反数 = `seqdesc[0].baseptr`。
>
> 语料验证（`verify_staticprop_animlen.js`）：**2681/2681** 个静态道具
> 满足「链头 4 字节 == `ff 00 00 00`」且
> `localseqindex - anim_data_off == 4`。普通模型里只有 2 个巧合命中。

### 7.5 根骨骼 Z 的 π/2 下限消失

`g_panimation[0]->rotation` 被置 0 后，根骨骼 Z 轴的 +90° 偏置
与 `π/2` 的 scale 下限都不再适用。

实测语料 **2681/2681** 个静态道具的 `rotscale` 三轴都是 `π/8/32767`，
**没有一个**用 `π/2`（普通模型则是 `[π/8, π/8, π/2]/32767`）。
见 `verify_staticprop_bonescale.js`。

### 7.6 附着点

`simplify.cpp:3392-3396` 对每个附着点：

```cpp
ConcatTransforms( rotated, g_attachment[i].local, g_attachment[i].local );
Q_strncpy( g_attachment[i].bonename, "static_prop", ... );
g_attachment[i].bone = 0;
```

实测 `ipf4`（`$staticprop` + `$attachment "muzzle" "root" 7 8 9`）：

```
local = [[-0, -1, 0, -8],
         [ 1, -0, 0,  7],
         [ 0,  0, 1,  9]]
```

即 `Rz(90°) × 平移(7,8,9)` —— 旋转部分变成 `Rz(90°)`，平移变成 `(-8,7,9)`。

> **实现坑**：必须**先组装完整的 `local`（含平移）再左乘**。分两步写
> （先写旋转矩阵、再用 `pos` 覆盖平移列）会把旋转后的平移又换回原值。

### 7.7 显式 `$hbox` + `$staticprop` 是**硬错误**

`MakeStaticProp()` 把骨骼 0 改名成 `static_prop`，之后
`simplify.cpp:6984` 按**旧名字**查 hitbox 的骨骼：

```cpp
k = findGlobalBone( set->hitbox[j].name );
if (k != -1) set->hitbox[j].bone = k;
else MdlError( "cannot find bone %s for bbox\n", set->hitbox[j].name );
```

**实测**：`ipf2`（`$hbox 0 "tip"`）、`ipf3`（`$hbox 0 "root"`）、
`ipf5`（只有 `$hbox` 无 `$attachment`）三个 QC **全部编译失败**，
报 `cannot find bone ... for bbox`。

**语料印证**：2681 个静态道具**全部**带 `AUTOGENERATED` 标志
（`flags & 0x1`），**0 个**用显式 `$hbox`
（`verify_staticprop_hitbox_auto.js`）。

mdlc 的处理：**不报错，改为把 hitbox 的骨骼重定向到 `static_prop`**。
理由是这样产出仍然合法（hitbox 落在唯一那根骨骼上），比直接失败更有用；
若要复刻官方的硬错误语义，应在 `validate()` 里拒绝这种组合。

### 7.8 验收结果

用官方 studiomdl 的受控实验产物逐字段对照
（`docs/_probe/cmp_staticprop.js`，含 VVD 逐顶点）：

| 用例 | 对应 QC | 相同 | 不同 |
|---|---|---|---|
| `parity/sp-basic.toml` | `ipa2`（单骨骼 + 序列） | **47** | **0** |
| `parity/sp-multiseq.toml` | `ipe2`（2 条序列） | **52** | **0** |
| `parity/sp-attach.toml` | `ipf4`（+ `$attachment`） | **50** | **0** |
| `parity/ip.toml` | `ip`（自动 hitbox + `contents`） | **28** | **0** |

**全部 0 差异。** 早先唯一剩下的 `bone[0].flags`（缺
`BONE_USED_BY_HITBOX 0x100`）已随 `AUTOGENERATED_HITBOX` 的实现而消失。
比较覆盖 VVD 的每个顶点（位置/法线/UV/骨骼/权重）、骨骼表全部字段
（含 `poseToBone` / `posscale` / `rotscale`）、动画链头字节、
seqdesc 的 blend 数组、附着点矩阵。

---

## 7b. 自动 hitbox 与包围盒（同属坐标系问题）

`$staticprop` 之外，本轮还查清了三条与「坐标系 / 空间」直接相关的机制。

### 7b.1 自动 hitbox：bbox 起点有两种约定

`SetupHitBoxes()`（`simplify.cpp:6884-6973`）在 QC 没写
`$hboxset`/`$hbox` 时自动生成 hitbox。每根骨骼的 bbox 起点由
`g_bUseBoneInBBox` 决定（`simplify.cpp:6899-6909`）：

| `g_bUseBoneInBBox` | 起点 | 来源 |
|---|---|---|
| `true`（**默认**，`studiomdl.cpp:57`） | **全 0** | —— |
| `false` | **±9999** | QC 写了 `$skipboneinbbox`（`studiomdl.cpp:5706`） |

起点为 0 意味着**原点恒在 bbox 内** —— 这就是产物里 `bbmin` 经常出现
`0.00` 的原因。

**语料二分**（`probe_skipboneinbbox.js`，判据是「原点是否在 box 内」）：

| 形态 | 数量 |
|---|---|
| 所有 box 都含原点 | **3082** |
| 所有 box 都不含原点 | **21** |
| 混合 | 1 |

那 21 个**全部**是 `static_prop` 碎片（`boat001a_chunk*` /
`concrete_spawnchunk*` / `tree_trunk_chunk*`）。

### 7b.2 自动 hitbox 的其余要点

- **矩阵方向**：源码的 `VectorITransform(pos, g_bonetable[k].boneToPose)` 里，
  `boneToPose` 是**内部** `s_bone_t` 的，与**文件里**的
  `mstudiobone_t.poseToBone`（偏移 `0x60`）**互为逆**。所以对文件矩阵应当做
  **正向** `VectorTransform`。受控实验 `iph1` 对照 `dump_hboxes` 真值
  **4/4 逐字段命中**。
- **权重口径**：`MAXSTUDIOBONEWEIGHTS` **就是 3**（`studiomdl.h:36`），
  `v1support.cpp:174` 在解析 SMD 时就已裁到 3 组。**VVD 的 3 组就是全部权重**
  —— 早先「12% 不符是权重口径问题」的猜测是错的。
- **空 set 是合法形态**：官方**先建 set、置标志，之后才过滤**，所以
  「有 set、0 box、带 `0x1` 标志」成立。语料 **166** 个如此，且 **166/166
  全部带标志**（`probe_hbox_empty_set.js`）。
- **`contents` 与 hitbox 无关**：默认值是 `CONTENTS_SOLID` = **1**
  （`studiomdl.cpp:5031` 的 `s_nDefaultContents`），无条件默认。
  语料 3281/3333 是 1，其余 52 个是 `CONTENTS_GRATE`（8，来自显式 `$contents`）。

### 7b.3 姿态包围盒：根骨骼的 `Rz(90°)`

`hull_min/hull_max` 与 `seqdesc[].bbmin/bbmax` 来自
`CalcSequenceBoundingBoxes()`（`simplify.cpp:7049`）—— **逐帧摆姿势**后的
顶点包围盒，再并上每根骨骼的渲染包围盒。`hull` 取自 `seq[0]`
（`write.cpp:2071-2087`）。

**坑**：`BuildRawTransforms`（`simplify.cpp:247-285`）对根骨骼**左乘**
`rootxform = AngleMatrix(panim->rotation)`，而 `panim->rotation` 在
`studiomdl.cpp:2427` 被设为 `g_defaultrotation` = `Rz(90°)`。
漏掉它的症状是 **x/y 互换**：

| | mdlc（漏） | 官方 |
|---|---|---|
| `ipe1` 的 `seq[1]` | `[-19.95,-19.8,0]..[10,20,7]` | `[-20,-19.95,0]..[19.8,10,7]` |

**`$staticprop` 不套这一层** —— `MakeStaticProp()` 把
`g_panimation[0]->rotation` 置 0（`simplify.cpp:3379`），且几何已被旋转，
叠加会多转 90°。

**验收**：官方 `ipe1` 的 `hull` + `seq[0]` + `seq[1]` 逐位一致；
176 个官方产物 `hull == seq[0]` **176/176**；
语料（排除有 `.phy` 的）**801/804**（3 个例外是显式 `$bbox`）。

> 比较 `hull` 时必须**排除有 `.phy` 的模型** —— `CollisionModel_ExpandBBox`
> （`write.cpp:2075`）会把碰撞盒 AABB 并进 hull。

---

## 8. 复现方法

```powershell
# 1. 生成受控实验的 QC/SMD
node D:\GITHUB\mdlc\docs\_probe\gen_illumpos.js
node D:\GITHUB\mdlc\docs\_probe\gen_illumpos_basis.js
node D:\GITHUB\mdlc\docs\_probe\gen_illumpos_default.js
node D:\GITHUB\mdlc\docs\_probe\gen_illumpos_ab.js

# 2. 用官方 studiomdl 编译（产物拷到 docs\_probe\artifacts\）
& D:\GITHUB\mdlc\docs\_probe\run_one.ps1 -Names ip,ipb1,ipb2,ipd1,ipa1,ipa2
& D:\GITHUB\mdlc\docs\_probe\run_one.ps1 -Names ipc1,ipc2,ipe1,ipe2,ipf1,ipf4,ipg1,ipg2

# 3. 看产物
node D:\GITHUB\mdlc\docs\_probe\dump_header.js docs\_probe\artifacts\ip_official.mdl
node D:\GITHUB\mdlc\docs\_probe\dump_vvd.js <game>\left4dead2\models\mymod\ipa2.vvd 5

# 4. `$staticprop` 逐字段验收（mdlc 产物 vs 官方）
cd D:\GITHUB\mdlc
& target\release\mdlc.exe build parity\sp-basic.toml    --out out\sp-basic
& target\release\mdlc.exe build parity\sp-multiseq.toml --out out\sp-multiseq
& target\release\mdlc.exe build parity\sp-attach.toml   --out out\sp-attach
cd docs\_probe
node cmp_staticprop.js ..\..\out\sp-basic\mymod\ipa2.mdl artifacts\ipa2.mdl `
                       ..\..\out\sp-basic\mymod\ipa2.vvd artifacts\ipa2.vvd

# 5. 语料统计
node D:\GITHUB\mdlc\docs\_probe\verify_illum_fallback.js        D:\DSH\L4D2ReverseEngineering\mdl-corpus
node D:\GITHUB\mdlc\docs\_probe\verify_staticprop_bones.js      D:\DSH\L4D2ReverseEngineering\mdl-corpus
node D:\GITHUB\mdlc\docs\_probe\verify_hull_mechanism.js        D:\DSH\L4D2ReverseEngineering\mdl-corpus
node D:\GITHUB\mdlc\docs\_probe\verify_staticprop_animlen.js    D:\DSH\L4D2ReverseEngineering\mdl-corpus
node D:\GITHUB\mdlc\docs\_probe\verify_staticprop_bonescale.js  D:\DSH\L4D2ReverseEngineering\mdl-corpus
node D:\GITHUB\mdlc\docs\_probe\verify_staticprop_hitbox_auto.js D:\DSH\L4D2ReverseEngineering\mdl-corpus
node D:\GITHUB\mdlc\docs\_probe\probe_staticprop_seqbbox.js     D:\DSH\L4D2ReverseEngineering\mdl-corpus
```

> `studiomdl.exe` 的 **exit code 不可靠**，必须扫 stdout/stderr 里的
> `ERROR` 标记来判断成功与否。
