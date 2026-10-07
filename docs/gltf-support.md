# mdlc 的 glTF / GLB 支持：可行性调研与 UX 设计

> **状态**：调研完成，**实现已落地并验收通过**（`src\gltf.rs`，2026-10）。
> **结论**：**可以做，但必须先接受一个前提 —— glTF 没有官方 oracle。**
>
> 本文的每一条结论都标了来源：
> - `[实测]` = 跑真程序得出的（真 `studiomdl.exe`、真 `gltf` crate、真夹具）
> - `[读码]` = 读 crate / mdlc 源码得出
> - `[推断]` = 由上两者推出，**未经独立验证**
>
> 取证素材：
> - 夹具生成：`D:\DSH\L4D2ReverseEngineering\_gltfresearch\gen_gltf.py` / `gen_gltf2.py` / `gen_dual.py` / `gen_embed.py`
> - 探针工程：`...\_gltfresearch\gltfprobe\`（`src\main.rs` + `src\bin\probe2..13.rs`）、`...\gltflite\`、`...\gltflite-import\`
> - 官方对照：`D:\GITHUB\mdlc\docs\_probe\oracle_dual.js` → `docs\_probe\_oracle_dual\official.json`
> - 几何/载荷对照：`...\_gltfresearch\cmp_gltf_vvd.js`、`D:\GITHUB\mdlc\docs\_probe\flex_vs_vvd.js`
>
> **实现验收记录（与调研结论逐条对应）**：
> - 几何：`g_rig.qc`（`rig.glb`）⟹ MDL **3148 B / VVD 1600 / VTX 437**，与官方 FBX 路径
>   `dual_rig.mdl` **尺寸完全相同**；`bone_mesh_space.js` 比值 **1.5264 ✅**。
> - flex：`k4_flex_align.js` 按 VVD 顶点位置对齐后，官方 `dual_full.mdl` vs mdlc `g_full.mdl`
>   **wide 24 vs 24、tall 24 vs 24，不同 0**（载荷逐值一致；只有排列顺序是有意偏离）。
> - 动画：`probe_anim_frames.js` 官方 `@idle` / `ValveBiped.Bip01_Spine` 31 帧 vs mdlc
>   **逐帧吻合**（差 ≤0.002°，ANIMROT 量化噪声）。
> - 单元测试 28 个（`src\gltf.rs` 的 `mod tests`）；回归三连全绿。

---

## 0. 结论速览

| 问题 | 结论 |
|---|---|
| 官方 `studiomdl.exe` 支持 glTF 吗？ | **完全不支持**。扩展名试探链里没有 `gltf`/`glb`，全二进制零命中。[实测] |
| 那怎么验收？ | **传递式 oracle**：同一 Blender 场景双导出 `.fbx` + `.glb`，官方编 `.fbx` 给出「官方口径」，再证明「glTF 的数能推出 FBX 的数」。 |
| 传递式 oracle 成立吗？ | **成立，且已逐值验证**：顶点 / 法线 / UV / flex 位移**四项全部逐值相同**；骨骼表的**名字、顺序、父子关系**也完全一致（位移的 100× 因子已在 FBX 侧修掉，见下一行）。[实测] |
| `gltf` crate 能用吗？ | **能用，但有三个坑**（见 §3），其中一个是**crate 的 bug**，会拒掉 Blender 导出的**最常见**文件形态。 |
| 那个 bug 修了吗？ | **上游 master 已修**（`ca97641`，2025-03-19，PR #449），但**尚未发版**（crates.io 仍是 1.4.1，`git tag --contains` 为空）。见 §3.5。 |
| 那用 GitHub nightly？ | **不建议**。坑一有确定的绕法；而 git 依赖有 5 条**无法用代码消除**的代价（最硬的一条：**`cargo publish` 会被 crates.io 拒绝**）。见 §4.4。 |
| 依赖代价多大？ | **+5 个包**（master，关 `import`）/ **+6 个**（1.4.1，关 `import`）；开 `import` 是 **+21/+22 个包**且对 mdlc 毫无用处。[实测] |
| 要新增 QC 语法吗？ | **0 条**。九条 `src*` 语法按概念命名，一条都不用改。[读码] |
| 两套输入会产出同一个模型吗？ | **现在会了**。曾经差 100×（Blender 的 **FBX 导出器**把 m→cm 单位换算烘进节点、**glTF 导出器不做**，而官方 studiomdl 只对顶点丢掉那个缩放、对骨骼保留）—— 已在 FBX 侧修掉（`docs/fbx-support.md` §4.6 偏离 12），两套输入现在编出**同一量级**的模型。见 §5.2。 |
| ⭐ **参考姿态取哪个姿态？** | ⚠️ **取错了**（§5.1b，已证实、尚未修）。glTF 的绑定姿态载体是 `skin.inverseBindMatrices`（IBM），而 `src\gltf.rs` **一个字节都没读它**，参考姿态全取节点 TRS。Blender 的 `Use Rest Position Armature`（**默认开**）关掉后，节点 TRS = 当前帧姿态而 IBM 仍是 rest ⟹ 骨骼与蒙皮网格分属两个空间，**与 FBX 侧 R59 是同一个缺陷**。自造夹具实测 `outside` 达 **9.823**、包围盒从 `[2,2,2]` 被拉成 `[2, 11.726, 8.633]`。⚠️ 现有三个夹具是**退化情形**（`jointWorld · IBM = I`），**测不出**这个问题。 |
| 建议 | **做**。按 §7 的落点实现，把「无 oracle」这件事在文档里说清楚。 |

---

## 1. 官方侧：零支持（决定性负结果）

### 1.1 扩展名试探链 [实测]

`E:\SteamLibrary\steamapps\common\Left 4 Dead 2\bin\studiomdl.exe`（7,800,512 B）里，
`Load_Source` 的试探链字符串逐字是：

```
fbx\0xml\0obj\0vta\0phys\0\0\0\0sma\0smd\0mpp\0dmx\0vrm\0
```

**没有 `gltf`，也没有 `glb`。**

### 1.2 全二进制串扫描 [实测]

| 模式 | `studiomdl.exe` | `bin\` 90 个模块 |
|---|---|---|
| `gltf` / `GLTF` / `.gltf` | 0 | 0 |
| `glb` / `GLB` / `.glb` | 0 | 只命中 `icudt.dll` / `icudt42.dll`（ICU 数据里的子串，与 glTF 无关） |
| `KHR_` / `Draco` / `draco` / `meshopt` | 0 | — |
| `assimp` / `tinygltf` / `cgltf` / `ufbx` / `OpenFBX` / `nlohmann` | 0 | 0 |
| `libfbxsdk` / `FBX SDK` | ✓ | 命中 `fbx2dmx.exe` + `studiomdl.exe` |

### 1.3 这意味着什么

FBX 之所以能做得那么扎实，是因为有 **82 个官方 oracle 用例**把每一条口径都裁决过
（`docs/fbx-support.md` §1）。glTF **一个都没有**。

所以本项目的硬约束 ——「验收 = oracle 差分对照真 `studiomdl.exe` 产物」——
在 glTF 上**无法直接满足**。这不是可以绕过去的小事，它决定了：

1. 不能照抄 FBX 的任何一条口径（照抄等于把「已验证」偷偷降级成「未验证」）；
2. 必须换一种验收方法，且这个方法本身要写进文档（见 §2）；
3. 文档里每条口径都必须标 `[推断]` 而不是 `[实测]`。

---

## 2. 传递式 oracle

### 2.1 思路

```
        Blender 场景
        ├── 导出 rig.fbx ──► 官方 studiomdl ──► rig.mdl / rig.vvd   ← 官方口径（真 oracle）
        └── 导出 rig.glb ──► ???              ← 我们要用 glTF 的数推出上面那一份
```

mdlc 的 **FBX 路径**已经被官方产物逐字段校准过（`docs/fbx-support.md` §1）。
于是只要证明：

> **「同一场景的 `.glb` 里读出来的数」能推出「同一场景的 `.fbx` 里读出来的数」**

glTF 就获得了**传递式** oracle —— 它不是「官方支持 glTF」，而是「官方支持 FBX，
而 glTF 与 FBX 在这个场景上等价」。

### 2.2 夹具 [实测]

`gen_dual.py`（188 行）用 Blender 5.2.2 无头建同一个场景，同时导出两种格式：

- 骨架 `Skeleton`，根节点带 `rotation_euler = (90°, 0, 0)`（判别用的非平凡根旋转）；
- 3 骨骼链 `ValveBiped.Bip01_Pelvis`（head 0 → tail 10）/ `Spine`（10 → 30）/ `Head1`（30 → 45）；
- 立方体 `body`（scale `(6,6,45)`、location `(0,0,22.5)`、已 `transform_apply`）；
- UV `((co.x+3)/6, co.z/45)` —— **刻意不对称**，能判别 V 翻转；
- 材质 `face`；
- shape key `wide`（x×2）/ `tall`（z+3）；
- 动画 Pelvis 60° / Spine 30° 绕 Z，25 帧，`scene.render.fps = 24`。

产出：`rig.fbx` 19308 B / `rig.glb` 3180 B / `full.fbx` 49772 B / `full.glb` 7984 B / `zup.glb` 3232 B。

`docs\_probe\oracle_dual.js` 把 `dual\` 拷进 `docs\_probe\_oracle_dual\`，生成 QC 并跑真 `studiomdl.exe`，
把 MDL / VVD 解析成 `official.json`。实测 `rig` 与 `full` 都是 `exit=0`。

### 2.3 四项逐值验证 [实测]

#### ① 顶点 / 法线 / UV —— `cmp_gltf_vvd.js`

按**去重集合**比对（glTF 按角拆点、VVD 已焊接，重数天然不等，只能比集合）：

| 夹具 | POSITION | NORMAL | TEXCOORD | 三元组 |
|---|---|---|---|---|
| `dual\rig.glb` vs 官方 `dual_rig.vvd` | ✅ | ✅ | ✅ | ✅ |
| `dual\full.glb` vs 官方 `dual_full.vvd` | ✅ | ✅ | ✅ | ✅ |
| `dual\zup.glb`（`export_yup=False`） | ❌ **Z 与 Y 互换** | ✅ | ✅ | ❌ |

⭐ **`export_yup=True`（Blender 默认）时几何完全同源。**

#### ② flex 位移 —— `flex_vs_vvd.js`

⚠️ 这个探针第一版把 vertanim 的 `delta` 当 `int16 / 4096` 解码，得出「官方对 morph 位移做了仿射变换」的
**错误结论**。真相是 **half-float（f16）**：`0x4200` = `+3.0`、`0xC200` = `-3.0`，
而 `4.125 = 16896/4096`、`-3.875 = -15872/4096` 纯属巧合。

修正后：

| | 官方 `dual_full.mdl` | mdlc 的 FBX 路径产物 |
|---|---|---|
| `flex[0] "wide"` | `delta = [±3.0000, 0, 0]` | 同 |
| `flex[1] "tall"` | `delta = [0, -0.0000, 3.0000]` | 同 |
| `ndelta` | 全零 | 全零 |
| `speed` / `side` | `-1` / `-1` | 同 |
| `vtype` | 0 | 0 |
| `targets` | `[0,1,10,11]` | 同 |
| vertanim 顺序 | `20,10,5,18,…`（= VVD 顶点序，**非升序**） | `0..23` 升序（**mdlc 的有意偏离**） |

⭐ **`delta` ≡ 源侧原始 shape key 偏移，逐值相同。**

#### ③ 骨骼表 —— `probe13.rs`（新建）

按 **G1 口径**（= 照抄 mdlc 的 FBX 收骨规则）从 `.glb` 推：

1. **种子** = `skin.joints`（glTF 是**显式**的，FBX 要靠蒙皮簇 + 祖先上溯推断）；
2. 沿父链上溯收集**全部祖先**；
3. 排列成 **DFS 先序**；
4. 位移 = `R_norm(parent_world)ᵀ·(child_world.t − parent_world.t)`（= `bone_offset`）；
   旋转 = 节点自己的局部四元数 → `QuaternionAngles`。

| # | 名字 | parent | glTF 推出 | 官方（FBX 路径） |
|---|---|---|---|---|
| 0 | `Skeleton` | −1 | pos `(0,0,0)` | pos `(0,0,0)` |
| 1 | `ValveBiped.Bip01_Pelvis` | 0 | pos `(0,0,0)` | pos `(0,0,0)` |
| 2 | `ValveBiped.Bip01_Spine` | 1 | pos **`(0,10,0)`** | pos **`(0,1000,0)`** |
| 3 | `ValveBiped.Bip01_Head1` | 2 | pos **`(0,20,0)`** | pos **`(0,2000,0)`** |

⭐⭐ **名字、顺序、父子关系三项完全一致**（连 DFS 先序都对上了）；
位移那一栏的 100× 差异已在 FBX 侧修掉（§5.2），现在两套输入给出**同一个** `(0,10,0)` / `(0,20,0)`。

#### ④ UV 不需要翻 [实测]

`dual\rig.glb` 原始 UV 底顶点 `[0.0000, 1.0000]`、顶顶点 `[0.0000, 0.0000]`，
与 FBX 侧（`probe_fbx.exe` 跑 `dual\rig.fbx`）**完全相同**。

⚠️ 对比 SMD 路径：`src\smd.rs:542` 逐字 `1.0 - parse_f32(t[8], line, "v")?`
（`studiomdl` 在**解析 SMD 时**就翻，`hl2sdk-episode1\utils\studiomdl\v1support.cpp:161` 的
`// invert v` + `t[1] = 1.0 - t[1];`）。
FBX 路径**要**自己翻（`src\fbx.rs` 的 `uv = [u.x as f32, 1.0 - u.y as f32]`），
**glTF 路径不能翻** —— 导出器已经给的是最终值。

### 2.4 传递式 oracle 的边界

它**只能**验证「glTF 与 FBX 在同一场景上等价」，**不能**验证：

- 官方的 FBX 口径本身对不对（那是 `docs/fbx-support.md` §1 的 82 个用例在管）；
- glTF 独有的东西（`skin.joints` 显式列表、`inverseBindMatrices`、`CubicSpline` 插值、
  `KHR_*` 扩展）—— 这些 FBX 里根本没有对应物，**永远不会有 oracle**。

⟹ 文档里必须把这两类分开写：**「有传递式 oracle」** 与 **「mdlc 自定，无 oracle」**。

---

## 3. `gltf` crate：能力与三个坑

**crates.io 上最新仍是 1.4.1**（2024-05-10 发布）/ `MIT OR Apache-2.0` / `edition = "2021"` /
**`rust-version = "1.61"`**（对 mdlc 的 MSRV 1.89 无约束）。`gltf-json = "=1.4.1"`（精确版本）。

⚠️ **上游 master 已经领先 1.4.1 一年多**，且**坑一已被修掉**（见 §3.5）。
项目**从不发 GitHub Release**（唯一的 Release 是 2022 年的 `1.0.0`，早已过时），
所以「用 nightly」只能用 **git 依赖**（`rev = "50d6522…"`）。

### 3.1 读得出来的（全部实测成功）

| 需要什么 | 怎么拿 |
|---|---|
| 节点层级 | `n.children()` / 自建父链 |
| 局部变换 | `n.transform()` → `Transform::Matrix{matrix}` 或 `Decomposed{translation,rotation,scale}`；`Transform::decomposed()` 连 `Matrix` 变体都会自动分解 |
| 网格数据 | `mesh.reader(get_buffer)` → `read_positions` / `read_normals` / `read_tex_coords(0)` / `read_joints(0)` / `read_weights(0)` / `read_indices` / `read_morph_targets` |
| 蒙皮 | `skin.joints()` / `skin.skeleton()` / `skin.reader(..).read_inverse_bind_matrices()` |
| 动画 | `animation.channels()` → `target().node()` / `target().property()` / `sampler().interpolation()` / `sampler().read_inputs()` / `sampler().read_outputs()` |
| 材质名 | `material.name()`（`names` feature） |
| morph 目标名 | `mesh.extras()` → `{"targetNames":[…]}`（`extras` feature） |

⚠️ **`gltf::Accessor` 没有 `reader()`** —— 这是最容易踩的 API 误判。
正确路径是 `Mesh::reader` / `Skin::reader` / `Animation::reader`
（`gltf-1.4.1\src\mesh\mod.rs:319`、`src\skin\util.rs:24`、`src\animation\mod.rs:168`）。

⚠️ 其它与 docs.rs 不符的地方：

| 写法 | 实际 |
|---|---|
| `doc.asset()` | **不存在** ⟹ `doc.as_json().asset` |
| `n.translation()` / `.rotation()` / `.scale()` | **不存在** ⟹ `match n.transform() { … }` |
| `mesh.index()` / `skin.index()` | 返回 **`usize`** |
| `prim.index()` / `accessor.index()` | 返回 **`Option<usize>`** |
| `read_joints()` / `read_weights()` | 返回 enum `ReadJoints`/`ReadWeights`，**没有 `.next()`** ⟹ 先 `.into_u16()` / `.into_f32()` |
| `sampler.reader().read_input` | **不存在** ⟹ 是 **`read_inputs`**（复数） |
| `read_morph_targets()` | 返回 **`ReadMorphTargets`（Iterator）**，`Item = (Option<ReadPositionDisplacements>, Option<ReadNormalDisplacements>, Option<ReadTangentDisplacements>)` ⟹ 必须 `.collect()` 后再解构 |
| `Mesh::extras()` | `Option<Box<RawValue>>`，`Option::map` 会 move ⟹ `.as_ref().map(\|v\| v.get().to_string())` |

### 3.2 ⚠️ 坑一（**crate 的 bug**）：无 `bufferView` 的 accessor 被拒

**症状** [实测]：

```
invalid glTF: accessors[6].bufferView: Missing data; accessors[8].bufferView: Missing data;
```

`gltf::import` 与 `Gltf::from_slice` **都**失败。

**根因** [读码]：`gltf-json-1.4.1\src\accessor.rs:235-245` 的 `accessor_validate_hook`：

```rust
if accessor.sparse.is_none() && accessor.buffer_view.is_none() {
    // If sparse is missing, then bufferView must be present. Report that bufferView is
    // missing since it is the more common one to require.
    report(&|| path().field("bufferView"), Error::Missing);
}
```

`gltf-json-1.4.1\src\validation.rs:220` → `Error::Missing => "Missing data"`。

**这是 crate 的 bug，不是文件的错。** glTF 2.0 规范原文：

> When undefined, the accessor **MUST be initialized with zeros**.

⟹ **没有 `bufferView` 是合法的**（表示全零）。

**为什么这条特别致命**：Blender 的导出器正是这么写「法线偏移全零」的 morph target 的。
也就是说 —— **任何用 Blender 导出的、只改位置不改法线的 shape key 文件都会被拒**。
这是**最常见**的情形，不是边角情况。

佐证 [实测]：`morph.glb` 的 JSON 里 `targets[0] = {POSITION: 5, NORMAL: 6}`，
而 `accessors[6]` 只有 `{componentType:5126, count:16, type:"VEC3"}`（**无 `bufferView`**）；
`bufferViews` 只有 8 条（索引 0..7）。

外部佐证：[KhronosGroup/glTF#2310](https://github.com/KhronosGroup/glTF/issues/2310) ——
「an undefined accessor.bufferView means only that the data must come from some source other than a buffer view」。

**绕法** [实测，已验证有效]：`Gltf::from_slice_without_validation(&bytes)`
（`gltf-1.4.1\src\lib.rs:322`；`:337-341` 的 `from_slice` 就是它 + `document.validate()`）。

绕过后手工喂 buffer，`read_morph_targets()` 读出的数据**完全正确**：

```
mesh#0 prim#0 目标数=2 (pos,nrm,tan)=[(16, 0, 0), (16, 0, 0)]
target[0] pos 前两个 = Some([-1.0, 0.0, -0.0])   ← wide 沿 X ×2
target[1] pos 前两个 = Some([0.0, 3.0, -0.0])    ← tall 沿 Z +3
nrm 首个 = None                                   ← 无 bufferView ⟹ 规范语义「全零」
```

⚠️ **代价**：`from_slice_without_validation` **绕过全部校验**，不只是这一条。
采用它必须自己补回必要检查（buffer 长度、索引范围、accessor 与 bufferView 的边界）。

⭐ **好消息**：crate 的 reader 层**本来就有边界保护** [读码]：
`gltf-1.4.1\src\accessor\util.rs:7-13 fn buffer_view_slice` =
`get_buffer_data(view.buffer()).and_then(|slice| slice.get(start..end))`
⟹ **越界返回 `None` 而不是 panic**。所以我们只需要补「buffer 声明的长度是否够」这一类检查。

### 3.3 ⚠️ 坑二：data URI 与 `import` feature 绑定

**实测**：

| feature | 文件 | 结果 |
|---|---|---|
| 关 `import` | `embed\embed.gltf`（内嵌 data URI） | 解析 OK，但 **`pos=0 nrm=0 uv=0 idx=0 首顶点=None`** —— **零几何、零报错** |
| 关 `import` | `embed\sep.gltf`（外部 `.bin`） | `pos=24 nrm=24 uv=24 idx=36 首顶点=Some([-1,-1,1])` |
| 开 `import` | `embed\embed.gltf` | **`[import] OK buffers=1 各长度=[1032]`**、`pos=24 idx=36` |

**机制** [读码]：`gltf-1.4.1\src\buffer.rs:43-45 pub enum Source<'a> { Bin, Uri(&'a str) }`
就是**原样**的 uri 字符串，**没有任何 data URI 解析** —— 解码在 `import.rs` 里，被 `import` feature 门禁。
关掉 `import` 后，`data:application/octet-stream;base64,AAAA…` 被当成普通文件路径
`base.join("data:…")` 去读，读不到就 `unwrap_or_default()` **静默变空 buffer**。

⚠️ **这是最危险的一类失败**：产物不报任何错，只是模型没有几何。
与 User said (m15848) 反对的「让用户猜编译器内部机制」正相反。

**两条路**：

- **A**：开 `import` ⟹ 开箱可用，但 **+22 个包**（含整条图片链 `image`/`png`/`zune-jpeg`/`moxcms`/…）
  且**会解码全部图片** —— 对 mdlc 完全无用（只要材质**名**，不要纹理像素）。
- **B**：关 `import` + 自己处理 `data:` 前缀（`base64` 是**独立 feature**，可单独开，不拉图片链）。

⭐ 选 **B**，理由还有一条更硬的：**我们无论如何都要用 `from_slice_without_validation`**（§3.2），
而 `import` 路径内部走的是带校验的 `from_slice` ⟹ **开着 `import` 也救不了那个 bug**。
既然 buffer 必须自己喂，`import` 就只剩「解 data URI」这一个作用，不值得 22 个包。

### 3.4 ⚠️ 坑三：不支持压缩扩展

`gltf-json-1.4.1\src\*.rs` 里 `draco|meshopt|KHR_draco` **零命中** [读码]。
所以 `KHR_draco_mesh_compression` / `EXT_meshopt_compression` 的文件读不了 ——
必须在文档层明确报「不支持的扩展」，**不能静默零几何**。

（`sparse` accessor **支持完整**：`gltf-1.4.1\src\accessor\util.rs:22-23 Iter::Sparse(SparseIter)`、
`src\accessor\sparse.rs`。）

### 3.5 ⭐ 坑一在上游 master 上**已被修掉**（2025-03-19）

[实测 + 读码] 上游 issue [#346](https://github.com/gltf-rs/gltf/issues/346)
「`Accessor` implementation not conformant with specification」（2022-05-31 开，**已 closed**）
就是这条，正文与评论都引用了规范原文。修复提交：

| 项 | 值 |
|---|---|
| 提交 | **`ca97641`「allow empty accessors」**（2025-03-19，PR [#449](https://github.com/gltf-rs/gltf/pull/449) from `robtfm`） |
| 合并 | `12fc1b7`（2025-05-01） |
| 改动 | `gltf-json/src/accessor.rs` **-15 行**（删掉 `accessor_validate_hook` 整段）+ `src/accessor/util.rs` +28 行（新增 `SparseIter::empty`） |
| 进过 tag 吗？ | **没有**（`git tag --contains ca97641` 为空）⟹ 只在 master 上 |

**实测对照**（同一组夹具，`from_slice` **带校验**路径）：

| 文件 | crates.io 1.4.1 | GitHub master `50d6522` |
|---|---|---|
| `morph.glb` | ❌ `accessors[6].bufferView: Missing data` | ✅ **OK**，`pos=16 idx=54 目标数=2` |
| `morph_zeronrm.glb` | ❌ `accessors[7]` / `[9]` 同错 | ✅ **OK**，`pos=16 idx=54 目标数=2` |

⭐⭐ **而且 master 做得比「不报错」更多** —— 它按规范把缺失的数据**物化成全零**：

```
target[0] pos=16 nrm=16 tan=0
  nrm[0]=[0.0000,0.0000,0.0000]  ← Some，说明按全零给了数据（1.4.1 给的是 None）
```

⟹ 这才是规范要求的语义（*"MUST be initialized with zeros"*），不是简单地跳过检查。

⚠️ 上游还把 `Accessor::byte_offset` 从 `USize64` 改成了 **`Option<USize64>`**（`gltf-json` 层），
属**破坏性变更**（`gltf-json` 历史上不受 semver 约束，见其 CHANGELOG）。对 mdlc 无影响（我们不读该字段）。

### 3.6 坑二 / 坑三在 master 上**仍未变**

| 坑 | master 状态 | 证据 |
|---|---|---|
| 坑二 data URI 与 `import` 绑定 | **未变** | 关 `import` 跑 `embed.gltf` ⟹ `各长度=[0]` / `pos=0 idx=0 首顶点=None`（**静默零几何照旧**）；开 `import` ⟹ `[import] OK 各长度=[1032] pos=24 idx=36` |
| 坑三 不支持 Draco / meshopt | **未变** | 全仓（`*.rs` / `*.toml` / `*.md`）搜 `draco\|meshopt` **零命中** |

**master 新增的能力**（`CHANGELOG.md` 的 `## Unreleased` 段）：`KHR_animation_pointer` 扩展 +
`allow_empty_animation_target_node` feature + `EXT_texture_webp`。
另有一批 **panic 修复**（PR [#471](https://github.com/gltf-rs/gltf/pull/471)「Fix panics」，2026-05-07）：
`fix crash if accessor is not available` / `fix crash with invalid UTF-8` / `fix crash if header is too small` /
`avoid underflows with zero counts` / `fix crashes if uri or mimeType is missing` —— 对**读不可信输入**的编译器来说，这一批本身就有价值。

---

## 4. 依赖与构建成本 [实测]

### 4.1 增量（`Cargo.lock` 的 `[[package]]` 逐条 diff）

mdlc 现在 **102** 个包。下表左半 = crates.io **1.4.1**，右半 = GitHub **master `50d6522`**：

| 方案 | 1.4.1 | master | 具体 |
|---|---|---|---|
| 关 `import`，不处理 data URI | **+5** | **+4** | `gltf` / `gltf-derive` / `gltf-json` / `inflections`（master 少了 `lazy_static`） |
| **关 `import` + 开 `base64`（推荐）** | **+6** | **+5** | 上面 + `base64 0.13.1` |
| 开 `import` | **+22** | **+21** | 上面 + `adler2` / `byteorder-lite` / `crc32fast` / `fdeflate` / `flate2` / `image` / `miniz_oxide`×2 / `moxcms` / `png` / `pxfm` / `simd-adler32` / `urlencoding` / `zlib-rs` / `zune-core` / `zune-jpeg` |

⭐ **master 少一个包**：`lazy_static` 被彻底移除了（`gltf-json/Cargo.toml` 的 `[dependencies]` 现在只有
`gltf-derive` / `serde` / `serde_derive` / `serde_json`，源码里也搜不到 `lazy_static`）。
`[[package]]` 计数：master 关 import **19**（1.4.1 是 20）、开 import **40**（1.4.1 是 41）。

⭐ **`serde_json` 已经在 mdlc 的 `Cargo.lock:604`**（传递依赖）
⟹ 若要用它读 `extras.targetNames`，**加为直接依赖不新增任何包**。

### 4.2 其它成本

| 项 | 值 |
|---|---|
| 构建耗时（关 `import`） | `Finished \`release\` profile in 7.78s`（只编 `base64` + `gltf` + 探针） |
| 构建耗时（开 `import`） | `Finished in 20.72s`（编 22 个包） |
| 构建耗时（master，关 `import`） | `Finished \`release\` profile in 56.44s`（**含从 GitHub 克隆 + 编译三个 git 源包**） |
| 二进制大小（关 `import`） | `gltflite.exe` 712704 B |
| 二进制大小（开 `import`） | `gltfi.exe` 723456 B（**只差 +10752 B ≈ +1.5%**） |
| 需要 C 工具链吗？ | **不需要**（纯 Rust，与 `ufbx` 不同） |

⚠️ `cargo tree --depth 1` 只显示直接依赖 ⟹ **必须 `cargo tree`（不带 `--depth`）才看得到整棵树**。

### 4.3 建议的 `Cargo.toml` 条目

**方案 A（推荐）：用 crates.io 的 1.4.1**

```toml
# glTF / GLB 源（走 gltf crate）。
#
# `default-features = false` 关掉 `import`：它只做三件事 —— 解 data URI、
# 读外部 .bin、解码全部图片。前两件我们自己做得更可控（见 §3.3），
# 第三件对 mdlc 完全无用（只要材质**名**，不要纹理像素）。
# 关掉它省下 16 个包（含整条 image/png/zune-jpeg 链）：+22 ⟹ +6。
#
# `base64` 是独立 feature，用来解内嵌 .gltf 的 data URI。
# `names` 拿节点/材质名，`extras` 拿 morph target 名（`extras.targetNames`）。
#
# ⚠️ 1.4.1 必须用 `Gltf::from_slice_without_validation`：
# crate 会把「无 bufferView 的 accessor」（规范允许，表示全零）判为错误，
# 而 Blender 对「法线偏移全零」的 morph target 正是这么写的（见 §3.2）。
# 上游 master 已修（§3.5），但尚未发版。
gltf = { version = "1.4", default-features = false, features = [
    "utils", "names", "extras", "base64",
] }
```

**方案 B：钉到上游 master（拿到坑一的修复）**

```toml
# ⚠️ 未发版的 git 依赖。好处是坑一（§3.2）已被修掉，可以走带校验的 `from_slice`，
# 不必再用 `from_slice_without_validation`；还白拿一批 panic 修复（§3.6）。
# 代价见 §4.4。
gltf = { git = "https://github.com/gltf-rs/gltf", rev = "50d65229477fe5f785c2c90df21eb59c93ea2261", default-features = false, features = [
    "utils", "names", "extras", "base64",
] }
```

### 4.4 ⚠️ 用 git 依赖的代价

| # | 代价 | 说明 |
|---|---|---|
| 1 | **`cargo publish` 会被 crates.io 拒绝** | crates.io **不允许**发布带 git 依赖的包（[Rust 论坛](https://users.rust-lang.org/t/help-cargo-package-with-github-dependencies/96275)、[crates.io#652](https://github.com/rust-lang/crates.io/issues/652)）。mdlc 的 `Cargo.toml` 目前**没有** `publish = false`，也从未发布过 ⟹ 一旦上 git 依赖，**发布这条路就断了**。 |
| 2 | 需要网络（或 vendor） | 首次 `cargo build` 要克隆 `gltf-rs/gltf`。CI 上没问题（GitHub 通），但**离线构建**要配 `[source]` 替换或 `cargo vendor`（mdlc 现在没有 `vendor\`，也没有 `.cargo\config`）。 |
| 3 | **构建耗时** | 56.44 s vs 7.78 s（首次；有缓存后差异小）。 |
| 4 | **没有版本号锚** | `rev` 是死钉的，但上游不会为它做兼容性承诺 —— `gltf-json` 历史上不受 semver 约束。 |
| 5 | 依赖升级靠手动 | `cargo update` 不会自动跟进；要自己改 `rev`。 |

⭐ **权衡**：坑一**已经有确定的绕法**（`from_slice_without_validation` + reader 层自带的越界保护），
而 git 依赖引入的 5 条代价**没有一条能靠代码消除**。
⟹ **建议先用方案 A**；把方案 B 记为「等上游发版后切换」——
届时两条好处（免绕校验 + panic 修复）都会自动到手，且**没有任何一条代价**。

---

## 5. 数据口径

### 5.1 几何：直接取 accessor [实测]

| 项 | 口径 | 与 FBX 的差别 |
|---|---|---|
| 顶点位置 | **直接用 `POSITION` accessor 的值** | FBX 要 `rot_norm(geometry_to_world)·p + t` |
| 法线 | **直接用 `NORMAL` accessor 的值** | FBX 要 `rot_norm(geometry_to_world)·v` |
| UV | **直接用 `TEXCOORD_0` 的值，不翻 V** | FBX **要**翻一次（`1 − v`） |
| 蒙皮 | `jointWorld · IBM` —— **在现有夹具上恒为单位阵**，但**不能据此跳过 IBM**（§5.1b） | FBX 无 IBM，靠簇推断 |

⭐ **为什么不需要 FBX 那两条公式**：glTF 的 `POSITION` 是**网格自己的局部空间**，
而 FBX 的 `vertex_position` 要经过 `geometry_to_world` 才是同一空间。
`probe7` 实测 `dual\rig.glb` 的原始 accessor 包围盒 = `[-3,-3,0]..[3,3,45]`
**与官方 VVD 完全相同**，且三个关节的 `jointWorld · IBM` 全是单位阵 ⟹ **在这三个夹具上**
蒙皮不改变顶点。

> ⚠️ **但「恒为单位阵」是夹具的退化性质，不是 glTF 的普遍性质** —— 见 §5.1b。

### 5.1b ⭐⭐⭐⭐⭐ 参考姿态：`inverseBindMatrices` 被完全忽略（**已证实有缺陷，尚未修**）

**一句话：glTF 的 IBM 就是 FBX `bind_to_world` 的对应物，而 `src\gltf.rs` 一个字节都没读它
—— 参考姿态全部取节点 TRS。当导出器把「当前帧姿态」当作关节 rest pose 时，
这条路径会复现 FBX 那次一模一样的错位。**

#### 5.1b.1 两个姿态、两个来源（与 FBX 同构）

| 姿态 | glTF 里的载体 | FBX 里的对应物 |
|---|---|---|
| **绑定姿态**（bind pose） | `skin.inverseBindMatrices`（IBM）的**逆** | `cluster.bind_to_world` |
| **节点 rest 姿态** | `node.translation/rotation/scale`（TRS）沿父链累乘 | `node.local_transform` 沿父链累乘 |

glTF 规范把 IBM 定义为「关节**初始配置**下全局变换的逆」
（`jointMatrix(j) = globalTransformOfJointNode(j) · inverseBindMatrixForJoint(j)`）；
蒙皮顶点写在绑定姿态空间，所以**参考姿态必须取 IBM⁻¹**，不能取节点 TRS。

#### 5.1b.2 为什么会分叉：Blender 的一个开关

Blender glTF 导出器（本机 **5.2.2 LTS**，`addons_core\io_scene_gltf2\`）的
Export → Data - Armature 有 **`Use Rest Position Armature`**，**默认 `True`**
（`__init__.py:902-909`）。

两条码路径**来自两个不同的数据源**：

- **IBM 永远取 rest**（`exp\skins.py:70-125`）：
  `inverse_bind_matrix = (axis_basis_change @ (armature.matrix_world @ bone.bone.matrix_local)).inverted_safe()`
  —— `bone.bone.matrix_local` 是 **edit bone = rest 姿态**，**与开关无关**。
- **节点 TRS 受开关控制**（`exp\tree.py:314-326`）：
  `gltf_rest_position_armature is False` ⟹ 用 `blender_bone.matrix`（**pose bone = 当前帧姿态**）；
  `True` ⟹ 用 `blender_bone.bone.matrix_local`（rest）。

⟹ 关掉这个开关，IBM 仍是 rest、节点 TRS 变成 pose ⟹ `jointWorld · IBM ≠ I`
—— **这正是 FBX 侧 `bind_to_world` 与 `local_transform` 分叉的 glTF 版本**。

#### 5.1b.3 决定性实证（自造夹具，`fbxbug\gltfbind\`）

现有三个夹具（`_oracle_dual\rig.glb` / `zup.glb` / `full.glb`）**测不出这个问题**
—— 它们的 `jointWorld · IBM` 最大偏离只有 `1.3e-7`（= 浮点噪声）：

```
rig.glb   最大偏离 = 1.344e-7 (Pelvis)   >1e-4 的有 0/3
zup.glb   最大偏离 = 1.001e-7 (Pelvis)   >1e-4 的有 0/3
full.glb  最大偏离 = 1.344e-7 (Pelvis)   >1e-4 的有 0/3
```

`make_fixture.py`（Blender `--background`）造出一对**只有导出开关不同**的 GLB
（3 根骨骼 `Pelvis`/`Spine`/`Head`，3 个立方体各以骨骼 head 为中心、权重 1.0，
姿态绕骨骼**局部 X 轴**转 20°/25°/30°）：

```
rest.glb  （Use Rest Position Armature = 开）  最大偏离 = 3.423e-8 (Pelvis)   >1e-4 的有 0/3
posed.glb （关）                              最大偏离 = 1.129e+1 (Head)     >1e-4 的有 3/3
              ⚠ Pelvis: dev=0.34202 / Spine: dev=3.65087 / Head: dev=11.29162
```

两个文件的 **IBM 逐值完全相同**（`n=48`，最大逐值差 `0`）⟹ 证实「开关只改节点 TRS」。
`IBM⁻¹` 的平移列 = 绑定姿态的骨骼原点（两文件相同）：
`Pelvis [0,0,0]` / `Spine [0,0,-10]` / `Head [0,0,-20]`（glTF Y-up 后）。

**mdlc A/B 实测**（`fbxbug\gltfbind\case\`，同一二进制、只换源文件）：

| | `Head` `outside` | `Spine` `outside` | `Pelvis` `outside` | 模型空间顶点包围盒尺寸 |
|---|---|---|---|---|
| `rest.mdl` | 0.000 | 0.000 | 0.000 | `[2.000, 2.000, 2.000]` ✅ 三个立方体各自叠在骨骼上 |
| `posed.mdl` | **9.823** | **2.420** | 0.000 | `[2.000, 11.726, 8.633]` ❌ 网格被拉成一条 |

骨骼表也印证：`posed.mdl` 的 `Pelvis quat=-0.5736,0,0,0.8192`（= −20° 绕 X，
**这就是摆的姿势**），`ptb` 平移列变成 `Spine [.., -9.0631, 4.2262]` /
`Head [.., -14.3960, 13.1915]` ⟹ **mdlc 把 pose 当成了 rest**。

> ⚠️ **夹具设计两条教训**（第一版两个都踩了）：
> ① 姿态必须绕骨骼**局部 X 轴**转 —— Y 是骨骼轴向，绕 Y 转只是自转、骨骼原点不动，测不出东西；
> ② 测 `outside` 的网格必须与骨骼 head **同心** —— 加了偏移会让「正确」情形也有非零基线，把信号淹掉。

#### 5.1b.4 修复落点（**尚未实施**）

- `src\gltf.rs` 的 `reference_poses`（`:1179`）：把世界矩阵基从节点 TRS 累乘换成
  **IBM⁻¹ 累乘**（无 IBM 时退回节点 TRS，与 FBX 侧「无蒙皮簇退回 rest」同构）。
- `src\gltf.rs` 目前 grep `inverseBindMatrices|inverse_bind|IBM` **0 命中**；
  `gltf` crate（`Cargo.lock` 里是 **1.4.1**）通过
  `skin.reader(..).read_inverse_bind_matrices()` 提供（§3 已记）。
- 下游**不需要再改**：`CompiledModel.source_world`（`src\model.rs:3913`）与
  `remap_vertices_to_reference_pose`（`src\compile.rs:4870-4978`）已经通了 ——
  glTF 侧也是经 `read()` 产出 `FbxGeometry`（含 `reference_frame`）再走同一条 `compile()`。
  换句话说，`source_world` 只需在 glTF 路径上填成 `IBM⁻¹` 而不是节点 TRS 累乘。
- `read_frames`（动画流）与 FBX 侧同状态：**未同步改**（见 `docs/fbx-support.md` §1.7c.4）。

> ✅ **零回归风险**：仓库里 **parity 夹具没有任何 `.glb`/`.gltf`**
> （`parity\*.qc` grep `.glb|.gltf` 0 命中，`parity\` 下无 glTF 文件）；
> 仅有的三个 glb 都在 `docs\_probe\_oracle_dual\` 且**恰好是退化情形**（IBM = 节点 TRS），
> 所以改与不改在这三个文件上产物**逐字节相同**。

### 5.2 ⭐⭐⭐⭐⭐ 骨骼位移：两套输入曾经差 100×（已在 FBX 侧修掉）

同一个 Blender 场景：

| | 网格 bbox | `Spine` 位移 | `Head1` 位移 | 自洽？ |
|---|---|---|---|---|
| `.glb`（`export_yup=True`） | `[-3,-3,0]..[3,3,45]` | `(0,10,0)` | `(0,20,0)` | ✅ 骨骼在网格内 |
| `.fbx` → **官方** `studiomdl` | `[-3,-3,0]..[3,3,45]` | **`(0,1000,0)`** | **`(0,2000,0)`** | ❌ 骨骼在网格外 100 倍 |
| `.fbx` → **mdlc**（本次修复后） | `[-3,-3,0]..[3,3,45]` | `(0,10,0)` | `(0,20,0)` | ✅ **与 `.glb` 一致** |

**机制** [实测 + 读码]：

- **FBX 导出器**把 m→cm 的单位换算烘进**骨架节点与网格节点**（两者各带
  `LclS = (100,100,100)`，是兄弟 ⟹ 文件本身自洽）；**glTF 导出器不做这个换算**
  （`Skeleton` 的 scale = `(1,1,1)`）；
- 官方 `studiomdl` 在**三处**用了不一致的判据：顶点**尺寸**走 `rot_norm(geometry_to_world)`
  （把 ×100 归一化掉）、顶点**位置**加原始平移（`t` 里含 ×100）、骨骼位移用
  `R_norm(parent)ᵀ · d`（`d` 里含父链缩放）⟹ 同一个 ×100 在一处被丢掉、在另两处被保留，
  于是编出**骨骼比网格大 100 倍**、且**网格位置在厘米而尺寸在米**的不自洽模型；
- ⭐ **mdlc 的 FBX 路径有意不照抄**（`src\fbx.rs` 的 `normalized_translation`：顶点位置与
  骨骼位移都先除掉累积缩放）⟹ 现在**两套输入编出的模型一致**。父链无缩放时该除法是恒等，
  所以不带节点缩放的 FBX 逐值不变（65 夹具 A/B 只有 16 个变）。
- 完整取证与验收见 `docs/fbx-support.md` §1.7.3 / §1.7.4 / §4.6 偏离 12。

> ⚠️ **对照时的注意事项**：修完之后，带节点缩放的 FBX（Blender 默认导出的**全部**
> 文件）其骨骼表**与网格位置**都与**官方产物数值不同** —— 官方是 ×100 的那个，
> mdlc 是 ÷100 的那个。要与 mdlc 对照，请用官方在 `Apply Scalings = "FBX Units Scale"`
> 下编出的产物（`u_aso_units.mdl`），那才是 mdlc 的对应物；或者用
> `docs\_probe\bone_mesh_space.js` 看比值（尺度不变量）。

**对 glTF 路径的含义**：

1. **不要**为了让 glTF 对齐官方 FBX 而去补一个隐式的 ×100 —— 那正好是
   User said (m15848) 反对的「让用户猜内部机制」；FBX 侧现在是向 glTF 看齐，不是反过来；
2. glTF 路径**原样输出自洽的模型**（顶点 ±3、骨骼 10/20）；
3. ⚠️ **不要试图用 `srcscale` 去「对齐」两套输入 —— 它做不到** [实测]。
   `srcscale` 是**整体缩放**：顶点乘 `s`（`opts.point` = `M·p`）、骨骼位移也乘 `s`
   （`opts.local` 的平移列 = `s·R·t`）⟹ **两者的比值不变**，那个 100× 的不一致
   被原样保留。实测（同一 `dual\rig.fbx`，探针 `docs\_probe\cmp_scale_pair.js`）：

   | QC | 顶点 bbox | `Spine` | `Head1` |
   |---|---|---|---|
   | `$body body "rig.fbx"` | `[-3,-3,0]..[3,3,45]` | `(0,10,0)` | `(0,20,0)` |
   | 同上 + `srcscale 0.01` | **`[-0.03,-0.03,0]..[0.03,0.03,0.45]`** | **`(0,0.1,0)`** | **`(0,0.2,0)`** |

   （上表是修复**后**的数值；修复前两行分别是 `(0,1000,0)/(0,2000,0)` 与
   `(0,10,0)/(0,20,0)`，看起来「对上了」但顶点同时缩到 `±0.03`。）
   第二行**模型整体小了 100 倍**，并没有变「自洽」。`srcscale` 的正当用途是
   「源文件单位不是 Source 单位时整体换算」（比如导出器写的是米），
   **不是**修正两套导入器之间的口径差。

### 5.3 骨骼集合与顺序 [实测]

**G1 规则**（§2.3 ③）在 glTF 上**逐项成立**：名字、顺序、父子关系与官方 FBX 产物**完全一致**。

与 FBX 的差别：

| 步 | FBX | glTF |
|---|---|---|
| 种子 | 有网格的节点 → 蒙皮簇里 `num_weights > 0` 的 `bone_node` | **`skin.joints` 直接给** |
| 祖先 | 沿父链上溯，遇 `is_root` 停 | 沿父链上溯到根（glTF 无合成根） |
| 顺序 | `dfs_preorder`（**必须自己排**，ufbx 给层序） | `dfs_preorder`（**同样必须自己排**，`scene.nodes` 也是层序） |
| 位移 | `bone_offset`（**已改成不继承父链缩放**，见 §5.2） | **同一条公式**（glTF 的节点本来就不带单位缩放，两者一致） |

⚠️ **`scene.nodes` 是层序**（`rig.glb` 给 `Tip, Mid, Root, Body, Rig` —— 子先于父），
与 ufbx 同病 ⟹ **不能照抄 `src\fbx.rs:572 accumulate_worlds` 的「父一定排在前面」假设**，
必须**自顶向下递归 + memo**。
（`probe12` 首版直接 `world[pi]` 就 panic 了 `no entry found for key`。）

### 5.4 旋转 [实测 + 读码]

glTF 的 `rotation` 是**四元数**，FBX 的 `local_transform.rotation` 也是四元数
⟹ **同一条路径**：`crate::bone_math::quaternion_angles([x,y,z,w])`。

⚠️ **必须走 `bone_math::quaternion_angles`，不能手搓**（`src\bone_math.rs:393`）。
它带**万向锁分支**（`matrix_angles_f64` 的 `xy_dist > 0.001`，`src\bone_math.rs:261-286`）。
我在 `probe13.rs` 里手搓了一个「数学上等价」的版本，结果 `Skeleton` 的 roll 给了 `−1.5708`
而官方是 `+1.5708` —— **符号全反**。

### 5.5 flex（morph target）[实测]

| 项 | 来源 |
|---|---|
| flexdesc 名 | `mesh.extras()["targetNames"]`（glTF 2.0 无「目标名」字段，这是事实标准） |
| 缺失时 | 按 FBX 的既定规则回退（`shape_key_frame(i) = i + 1` 的编号名） |
| 位移量 | morph target 的 `POSITION` accessor（**是位移量，不是绝对位置**，与 FBX shape key 语义相同） |
| 法线偏移 | 规范语义「无 `bufferView` = 全零」；FBX 路径**丢弃** `nrm_off` ⟹ glTF 同样丢弃 |
| 顶点映射 | `target` 的 accessor 按**顶点下标**（不是角下标）对齐到网格顶点 |
| 未命中的顶点 | 静默跳过（对齐官方 `simplify.cpp:2524` 的 `if (scale > 0 && vanim_mapcount[vertex])`） |

⭐ **官方 vs mdlc 的 VVD 顶点编号本身就不同** ⟹ flex 段不可能逐字节对齐，
只能按 mdlc 自己的编号生成并记入偏离表（这条 FBX 已经记过，glTF 沿用）。

### 5.6 动画 [实测]

与 FBX 的**结构性差别**：glTF 有**显式时间轴（秒）+ 插值模式**。

实测 `dual\full.glb`：`anim "SkeletonAction"` 9 通道，时间区间 `[0,1]` 秒 ⟹ **31 帧 @30fps**；
插值 `{Step: 7, Linear: 2}`；`target=2 Rotation Linear 25 关键帧`
⟹ `q0=[0,0,0,1] q15=[0,0,0.2588,0.9659] q30=[0,0,0.5,0.866]`（Pelvis 60°）。

⚠️ **Blender 对未动的通道写 `Step` 常量**（`Translation Step` + `Rotation Linear` + `Scale Step`）
⟹ 重采样必须自己处理：

1. **三种插值**：`Step`（阶跃）/ `Linear`（线性）/ `CubicSpline`（三次样条，**尚未处理**）；
2. **补零通道**：没有通道的节点 = 保持参考姿态；
3. **时间轴**：`read_inputs()` 给秒，`srcfps`（默认 30）决定采样点。

⭐ 好处：glTF 的 `Step`/`Linear` 语义**比 FBX 的关键帧更明确**，重采样逻辑反而更好写。

### 5.7 材质 [实测]

`material.name()` 直接拿得到（`uv_mat.glb` → `Some("face")`）。
无材质时沿用 `crate::fbx::FALLBACK_MATERIAL`（`"debug/debugempty"`）——
**这是官方 FBX 路径的既定行为**，glTF 沿用以保持两套输入一致。

### 5.8 轴向 [实测]

| 导出选项 | 原始 accessor bbox | 与官方 VVD |
|---|---|---|
| `export_yup=True`（Blender 默认） | `[-3,-3,0]..[3,3,45]` | ✅ 相同 |
| `export_yup=False` | `[-3,-45,-3]..[3,0,3]` | ❌ Z 与 Y 互换 |

⭐ **`export_yup=True` 时，直接取原始 `POSITION` 就得到 Source 要的 Z-up 坐标** ——
不需要任何变换。`export_yup=False` 会让 glTF 路径与官方 FBX 口径**分道扬镳**。

⚠️ 骨骼侧同理：`zup.glb` 的 `Pelvis` 额外带一份 `Rx(90°)`
（`rot=[1.5708,0,0]`，而 `rig.glb` 的 `Pelvis` 是单位）⟹ **轴向选错时骨骼也会错**。

`srcaxis` 仍然是那个旋钮（`ForcedAxis`），默认不施加任何变换。

---

## 6. UX 设计

### 6.1 三条原则（沿用 `docs/fbx-support.md` §4.1）

1. **格式由扩展名决定** —— 写 `.glb` 就是 glTF，写 `.fbx` 就是 FBX，**不猜**；
2. **不静默** —— 任何「读到了但没用上」的东西都要发诊断；
3. **不猜内部机制** —— 需要用户决策的地方给语法，不给「魔法默认值」。

### 6.2 新增语法：**0 条**

九条 `src*` 语法（`srcpart` / `srcmaterial` / `srcscale` / `srcaxis` / `srcstack` / `srcfps` /
`srcshapekey` / `srcshapekeyorder` / `srcshapekeyignore`）**按概念命名**，
在 glTF 上**逐条都适用**：

| 语法 | 在 glTF 上的对应物 |
|---|---|
| `srcpart` | `mesh.name()` / 节点名 |
| `srcmaterial` | 无材质时的兜底名 |
| `srcscale` | 统一缩放（顶点 + 骨骼） |
| `srcaxis` | 强制轴向（默认不施加） |
| `srcstack` | `animation.name()`（glTF 用名字选动画，**不需要 FBX 的「恒取第一条」限制**） |
| `srcfps` | 重采样率（默认 30） |
| `srcshapekey` / `srcshapekeyorder` / `srcshapekeyignore` | morph target 选择与排序 |

⭐ **这正是 R34「必须写完整扩展名」的红利**：扩展名成为可靠的格式信号，
所以「按概念命名」是可行的 —— 加 glTF 是**加一个 source reader**，不是加一套语法。

### 6.3 新增诊断（5 条）

| 触发 | 文案方向 | 实现状态 |
|---|---|---|
| 有 `data:` URI 且解码失败 | **报错**（不能静默零几何） | ✅ `decode_data_uri` 直接 `Err`（非 base64 形态也不猜） |
| 文件用了 `KHR_draco_mesh_compression` / `EXT_meshopt_compression` | **报错**（crate 不支持） | ✅ `check_extensions` 同时查 `extensionsRequired` 与 `extensionsUsed` |
| 无 `bufferView` 的 accessor | **提示**「按规范当全零处理」（crate 判错，我们放行） | ✅ `GltfNotes::zero_accessors` |
| `CubicSpline` 插值 | **提示**「按线性重采样」 | ✅ `GltfNotes::cubic_spline_channels`（取中间那个 value，切线丢弃） |
| 多网格被合并进同一个 `mstudiomodel_t` | 提示（沿用 FBX 的那条） | ✅ 沿用 `fbx_diagnostics` 那条 |
| 动画里有 `MorphTargetWeights` 通道 | **提示**「mdlc 不做逐帧表情权重」 | ✅ `GltfNotes::morph_weight_channels`（实现时补的第 6 条） |

⭐ **两条设计约束**（实现时定死的）：
1. **提示文案是纯函数**（`GltfNotes::lines(at) -> Vec<String>`），`diagln!` 只是逐条转发 ——
   `diagln!` 走 stdout，测试里抓不到，文案放进纯函数才测得了。
2. **报错的那两条不走 `GltfNotes`**（它们在 `load_document` 里就 `Err` 了）——
   静默零几何是绝对不能接受的。

### 6.4 明确不做

- ❌ **不实现 Draco / meshopt 解压**（要拉 `draco` / `meshopt` 解码器，且 mdlc 已经依赖 `meshopt` 但那是编码器）。
- ❌ **不解码图片**（只要材质名）。
- ❌ **不读 `KHR_materials_*` 扩展**（Source 材质系统没有对应概念）。
- ❌ **不做 glTF 导出**（mdlc 只编译，不转换）。

### 6.5 ⭐ cdtexture 哨兵：glTF **不追加**（用户裁决）

背景：官方 FBX 产物比 mdlc 的 glTF 产物**多一条空 cdtexture**（`numcdtextures` 2 vs 1）。

**受控实验**（`target\dmxprobe\`，只换几何源格式、其余 QC 逐字相同，真 `studiomdl.exe`）：

| 几何源 | 官方日志逐字 | `numcdtextures` | 最后一条 |
|---|---|---|---|
| `.smd` | `grabbing box.smd` | **1** | — |
| `.obj` | `grabbing box.obj` | **1** | — |
| `.dmx` | `DMX Model …box.dmx` | **2** | `""` |
| `.fbx` | `DMX Model …box.fbx` | **2** | `""` |

⟹ 那条哨兵是**「DMX 导入器家族」的行为，不是 FBX 专属**（`.fbx` 只是 DMX 导入器的一个前端）。
且**无条件**：把 `box.dmx` 里唯一的材质名改成与 cd 路径匹配后，哨兵**依然追加**。

**裁决（用户，2026-10）**：**不追加**。理由 —— 官方对 glTF 零支持、没有 oracle 可问；
与其按「同族 DMX 会追加」外推，不如守「**只写能从 QC 直接读出来的条目**」这条规则，
后者不需要用户猜 mdlc 的内部推断（对齐 `docs/fbx-support.md` §4.1 原则 3）。

**代价**（记入偏离表）：同一个 Blender 场景的 `.fbx` 与 `.glb` 产物，
cdtexture 表**差一条空串**。第三方实现（NekoMDL 的 `neko_rig.mdl` / `neko_rig_glb.mdl` /
`nk_morphflex.mdl`）**也都只有 1 条** ⟹ 不追加与第三方一致。

⚠️ 这条规则**故意不写进 `mdl_writer.rs` 的判据**（那里只认 `SourceKind::Fbx`）——
`src\mdl_writer.rs:1678` 起有一整段注释记录了这次实验，防止将来有人「顺手」把 glTF 加进去。

---

## 7. 实现落点

架构**完全复用 FBX 那一套单点分派 + 中立 IR**（`docs/fbx-support.md` §6.2），
一行都不用改结构：

| 位置 | 改动 | 状态 |
|---|---|---|
| `Cargo.toml` | 加 `gltf`（见 §4.3） | ✅ `gltf = { version = "1.4", default-features = false, features = ["utils","names","extras"] }` + 独立的 `base64 = "0.13"` / `serde_json = "1"` |
| `src\compile.rs` `enum SourceKind` | 加 `Gltf`；`of()` 认 `.gltf` / `.glb` | ✅ 三处 `SourceKind::Fbx =>` 各配一个 `Gltf` 臂 |
| `src\gltf.rs`（新建） | `read()` / `read_frames()`，与 `src\fbx.rs` 同构 | ✅ 约 2600 行（含 28 个测试） |
| `src\compile.rs` `read_source*` | 加一个 `match` 臂 | ✅ `read_source` / `read_source_geometry` / `read_source_frames` |
| `src\lib.rs` | 模块表加一行 + `pub mod gltf;` | ✅ 插在 `pub mod flex;` 与 `pub mod layout;` 之间 |
| `docs\` | 本文 | ✅ |

**`src\gltf.rs` 内部要点**：

1. `Gltf::from_slice_without_validation` + 手工装 buffer（`Bin` ⟹ `g.blob`；`Uri` ⟹ 文件或 `data:`）；
2. **自顶向下递归**算世界矩阵（`scene.nodes` 是层序）；
3. 收骨用 `skin.joints` + 祖先上溯 + `dfs_preorder`（**直接复用 `src\fbx.rs` 那个纯函数**，
   它只吃 `&[(u32, Option<u32>)]` + `&HashSet<u32>`，与格式无关）；
4. 顶点 / 法线 / UV 直接取 accessor（**不翻 V**）；
5. `bone_offset` / `normalized_translation` / `source_euler_from_quat` **复用**（`src\fbx.rs` + `src\bone_math.rs`）；
6. morph target → `FbxShapeKey` 同构结构，下游 `resolve_shape_key_flexes` 不用改；
7. 动画：按秒轴重采样到 `srcfps`，`Step` / `Linear` 各自求值，`CubicSpline` 取中间值降级为线性。

**实现时新踩到的坑（写代码时才知道的）**：

| # | 坑 | 解法 |
|---|---|---|
| 1 | ⚠️ **`src\lib.rs` 没加 `pub mod gltf;` 时 `cargo build` 报 EXIT=0** —— `src\gltf.rs` 根本没被编译（**假绿**） | 接线后再编译，一次暴露 18 个错误 |
| 2 | `base64` 0.13 的 API 是**自由函数** `base64::decode(..)`，**没有** `Engine` trait / `engine::general_purpose::STANDARD`（那是 0.21+） | 直接用 `base64::decode` |
| 3 | 闭包生命周期：`let get = move \|b: gltf::Buffer\| buffers.get(b.index()).map(Vec::as_slice);` 会被推成 `for<'x> Fn(Buffer<'x>) -> Option<&'x [u8]>`，而 `Iter::new` 要的是 `Fn(Buffer<'a>) -> Option<&'s [u8]>`（借自 **buffers**） | 抽成具名函数 `fn buffer_getter<'a,'s>(buffers: &'s [Vec<u8>]) -> impl Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]> + 's` |
| 4 | `gltf::accessor::Iter::new` 对 `read_indices` 报 `E0283: type annotations needed`（`u32::from` 对多种整型都有实现） | 显式写 `Iter::<u8>::new(..)` / `Iter::<u16>::new(..)` / `Iter::<u32>::new(..)` |
| 5 | `ufbx::Vec3` / `ufbx::Quat` **没实现 `PartialEq`**（C 绑定）⟹ `#[derive(PartialEq)]` 的 `LocalTrs` 报 `E0369` | 去掉 `PartialEq` |
| 6 | `shape_key_frame` 是 **`FbxGeometry` 的关联函数**，不是 `crate::fbx` 的自由函数 | 写 `FbxGeometry::shape_key_frame(ti)` |
| 7 | ⚠️ 夹具里把 accessor **插到数组开头**会把整张表挪位（下标就是引用编号）—— **而且不报错**，只是静默读出垃圾 | 测试夹具一律**追加**到末尾（`add_accessor` / `add_view` 的锚点取「数组收尾 + 下一节开头」，这样能连续追加） |
| 8 | ⚠️ 公开条目的文档注释里写了指向**私有条目**的 intra-doc 链接（`GltfNotes` / `read` 里的 `[`load_document`]`），以及一个**根本不存在**的名字 `[`resample`]` ⟹ CI 的 `cargo doc` 报 3 个 error（`RUSTDOCFLAGS=-D warnings`） | 改成普通文字（私有条目不加链接）。⚠️ **本地跑 `cargo doc` 必须换新 `--target-dir`**，否则 cargo 认为 doc 已最新直接跳过 ⟹ 假绿 |

---

## 8. 待办与风险

| # | 项 | 风险 |
|---|---|---|
| 1 | **无 oracle** | **最高**。所有口径都只能靠「与 FBX 传递等价」或「mdlc 自定」。文档必须逐条标注。 |
| 2 | `from_slice_without_validation` 绕过全部校验 | 中。需要自己补 buffer 长度 / 索引范围检查（reader 层已有越界保护，见 §3.2）。 |
| 3 | crate 的 `bufferView` bug | 中。**上游 master 已修**（`ca97641`，§3.5），但**尚未发版**（crates.io 仍是 1.4.1）。绕过代码要能**自动失效**（不能依赖 bug 存在）—— 见下方「兼容两种上游的写法」。 |
| 4 | `CubicSpline` 插值 | 低-中。Blender 默认不写，但第三方工具会。 |
| 5 | 两套输入差 100× | ✅ **已在 FBX 侧修掉**（`docs/fbx-support.md` §4.6 偏离 12），两套输入现在一致。⚠️ 遗留影响：带节点缩放的 FBX 其骨骼表与**官方产物数值不同** ⟹ 与 mdlc 对照时要用官方 `Apply Scalings = "FBX Units Scale"` 的产物。⚠️ 仍然**没有旋钮能补偿**（§5.2 实测：`srcscale` 是整体缩放，比值不变）。 |
| 6 | `base64 0.13.1` 版本较老（2021） | 低。只在解 data URI 时用，且输入是本地文件。 |
| 7 | glTF 几何源**不追加**空 cdtexture | ✅ **用户裁决，见 §6.5**。代价：同一场景 `.fbx` vs `.glb` 的 cdtexture 表差一条。 |
| 8 | ⭐⭐⭐⭐⭐ **`inverseBindMatrices` 被完全忽略** | **高**（已证实、尚未修，§5.1b）。`src\gltf.rs` 参考姿态全取节点 TRS；Blender 的 `Use Rest Position Armature` 关掉后节点 TRS = 当前帧姿态、IBM 仍 = rest ⟹ 骨骼与蒙皮网格分属两个空间，与 FBX 侧 R59 是**同一个缺陷**。自造夹具实测 `posed.glb` 的 `outside` 达 **9.823**、模型空间包围盒从 `[2,2,2]` 被拉成 `[2, 11.726, 8.633]`。修复只需把世界矩阵基换成 IBM⁻¹ 累乘（下游 `source_world` 已通）。✅ **零回归风险**：parity 夹具无任何 `.glb`/`.gltf`，仅有的三个 glb 恰好是退化情形。 |
| 9 | **动画流是否同步 bind 口径** | 中（未定案，§5.1b.4 末）。与 FBX 侧同状态：`read_frames` 取节点 TRS。 |

### 8.1 兼容两种上游的写法（风险 2 + 3）

风险 2 与风险 3 的解法是同一个：**永远走 `from_slice_without_validation`，自己补齐校验**。
这样上游修与不修**都不影响我们** —— 修了只是我们少报一条错误，绕过的代码本身仍然正确。

```rust
// 不写成「先试 from_slice，失败了再退到 without_validation」——
// 那会让行为依赖 crate 的版本（同一个文件在两个版本下走不同分支），
// 而且失败重试本身有成本。直接固定走绕校验的那条路。
let gltf = Gltf::from_slice_without_validation(&bytes)?;
// 自己补的三件事（顺序即成本从低到高）：
// 1. 每个 buffer 声明的 byteLength ≤ 实际读到的字节数（GLB 的 BIN chunk / 外部 .bin / data URI）
// 2. 每个 bufferView 的 [byteOffset, byteOffset+byteLength) 落在对应 buffer 内
// 3. accessor 的 stride/count/type 推出的字节数落在其 bufferView 内
//    （⚠️ accessor 没有 bufferView 时**跳过**，按规范当全零 —— 这正是坑一）
```

⚠️ **不要**依赖 `accessor.buffer_view.is_some()` 来分支处理「全零」：
master 上它仍然是 `None`（修复是在 reader 层返回全零迭代器，没有伪造一个 bufferView）。
判据是**读出来的迭代器**：1.4.1 给 `None`、master 给「`count` 个全零」，
两者都表示「这个 target 的法线偏移是全零」⟹ **按全零处理即可，不需要区分版本**。

---

## 附：本次调研新建的文件

**夹具生成（`D:\DSH\L4D2ReverseEngineering\_gltfresearch\`）**
- `gen_gltf.py`（6188 B）⟹ `fixtures\` 7 个：`rig.glb` / `rig.gltf`（分离式）/ `anim.glb` / `morph.glb` / `full.glb` / `twomesh.glb` / `axis.glb`
- `gen_gltf2.py`（4794 B）⟹ `fixtures2\` 3 个（带 UV + 材质）：`uv_mat.glb` / `morph_zeronrm.glb` / `anim_uv.glb`
- `gen_dual.py`（6505 B）⟹ `dual\` 5 个：`rig.fbx` / `rig.glb` / `full.fbx` / `full.glb` / `zup.glb`
- `gen_embed.py`（1301 B）+ `make_embed.js`（1102 B）⟹ `embed\`：`sep.gltf` + `sep.bin` + `embed.gltf`（手工合成 data URI）

**探针（`...\_gltfresearch\gltfprobe\src\`）**
- `main.rs`（7700 B）：总览 —— asset / counts / 节点 / 网格逐 prim / 蒙皮 / 动画 / 材质 / 原始容器
- `bin\probe2.rs`（4599 B）：`gltf::import` vs `from_slice + import_buffers` vs 手工 GLB 解析
- `bin\probe3.rs`（4773 B）：`from_slice_without_validation` 绕校验
- `bin\probe4.rs`（3806 B）：`extras.targetNames` + 动画通道输出类型
- `bin\probe5.rs`（6203 B）：节点局部 TRS + 世界平移 + 子节点 + `skin.joints` + 祖先链
- `bin\probe6.rs`（12346 B）：综合
- `bin\probe7.rs`（8336 B）：顶点空间（原始 accessor / `jointWorld·IBM` / 蒙皮结果）
- `bin\probe8.rs`（3850 B）：UV 口径
- `bin\probe9.rs`（7994 B）：morph 位移 + 动画插值
- `bin\probe11.rs`：**data URI**（`gltf::import` vs 手工）
- `bin\probe12.rs`（172 行）：**世界矩阵逐关节**（发现 `scene.nodes` 是层序）
- `bin\probe13.rs`：**G1 收骨**（`skin.joints` + 祖先 + DFS 先序 + `bone_offset`），与官方 MDL 逐根对照

**探针（`...\_gltfresearch\gltfnightly\` 与 `gltfnightly-import\`）—— 验证上游 master**
- 两个工程都钉 `gltf = { git = "https://github.com/gltf-rs/gltf", rev = "50d65229477fe5f785c2c90df21eb59c93ea2261" }`；
  前者 `default-features = false, features = ["utils","names","extras","base64"]`（关 `import`），
  后者把 `base64` 换成 **`import`**。
- `gltfnightly\src\main.rs`：对每个夹具跑 **[A] `Gltf::from_slice`（带校验）** 与 **[B] `from_slice_without_validation`**，
  打印 buffers 长度 / `pos` / `idx` / 目标数 / 首顶点 / 每个 buffer 的 `uri` 前缀。
- `gltfnightly\src\bin\probe_morph.rs`：**把 morph target 摊开**，逐个 target 打印 `pos`/`nrm`/`tan` 计数与前两个位移
  —— 这是看出「master 把缺失数据物化成全零」的那一支。
- `gltfnightly-import\src\main.rs`：显式调 **`gltf::import(path)`** 与手工路径对照。
  ⚠️ **教训**：上一轮的 `gltflite-import` 复制了关 `import` 版的 `main.rs`，
  用的是 `from_slice_without_validation` + 手工 `std::fs::read`，**根本没调 `gltf::import`**
  ⟹ 那次 A/B 对「`import` 能否解 data URI」**零信息量**。**对照组必须真的走那条路径，不能只改 `Cargo.toml`。**

**对照脚本**
- `cmp_gltf_vvd.js`（4826 B）：`.glb` 几何 vs 官方 `.vvd`（去重集合比对）
- `dump_glb.js`（1046 B）：手解 GLB 容器
- `dump_vvd_full.js` / `dump_nrm.js`

**mdlc 侧**
- `docs\_probe\oracle_dual.js`（132 行）：跑真 `studiomdl.exe` 编 `dual\*.fbx`，解析 MDL/VVD 写 `official.json`
- `docs\_probe\flex_vs_vvd.js`（84 行）：MDL flex vertanim vs VVD 顶点并排（**发现 half-float 解码错误**）
- `docs\_probe\cmp_scale_pair.js`：两份 mdlc 产物的骨骼表 + VVD 包围盒并排（**推翻 `srcscale` 补偿说**）
- `target\gltfscale\gs_plain.qc` / `gs_s001.qc`：§5.2 那组 A/B 的 QC 夹具（同一 `.fbx`，后者加 `srcscale 0.01`）

**教训（写给未来的自己）**

1. **`delta`/`ndelta` 是 half-float，不是 `int16/4096`。** 后者能凑出「看起来像仿射变换」的
   数字（`4.125` / `-3.875`），非常像真的。判据：仓库里 40+ 处既有探针**全都**用 half-float。
2. **`Transform::Decomposed` 给的四元数要经 `QuaternionAngles`，不能手搓。**
   手搓版在 `Rx(90°)` 上给出 `-1.5708` 而官方是 `+1.5708` —— 差一个符号，且**看起来完全合理**。
3. **`scene.nodes` 是层序**，世界矩阵必须递归算；照抄 ufbx 那条「父一定在前面」的假设会 panic。
4. **`Rᵀ·d` 的写法**：存储 `m[col][row]` 时，`(Rᵀ·d)[i] = Σ_j m[i][j]·d[j]`（**按行**遍历）。
   写成 `Σ_j m[j][i]·d[j]` 就是 `R·d`，**符号全反**（`Spine` 给 `(0,-10,0)` 而官方 `(0,+1000,0)`）。
5. **探针的自检要覆盖「两种口径在现有样本上是否可分」** —— `cmp_gltf_vvd.js` 首版按多重集比对，
   而 glTF 按角拆点、VVD 已焊接，重数天然不等 ⟹ 必须比去重集合。
6. **Blender 无头必须加 `--factory-startup`**（用户装了 `io_scene_valvesource` / `comfyui_blender`，
   否则 handler 抛 `AttributeError: 'Scene' object has no attribute 'vs'` / `PermissionError`）。
7. **Blender 5.2 已移除 `GLTF_EMBEDDED`**（只剩 `'GLB'` / `'GLTF_SEPARATE'`）⟹ 内嵌版只能手工合成。
8. **「某个旋钮能补偿差异」是必须实测的断言，不能靠读代码推断。**
   §5.2 初稿凭 `opts.point` / `opts.local` 的公式推断「`srcscale 100` 能对齐两套输入」，
   实测后**被推翻** —— `srcscale` 是整体缩放，顶点与骨骼同乘，比值恒定。
   教训：**当两条口径只差一个比例因子时，任何「也乘同一个因子」的旋钮都补偿不了它**；
   要证明补偿有效，必须看到**比值变化**，而不只是某一侧的数字变好看。
9. **「上游修了吗」要查三件事，缺一不可**：① 修复提交在不在 master（`git log`）；
   ② 它在不在**已发布的 tag** 里（`git tag --contains <sha>`）；
   ③ 有没有**新的 Release**（`gltf-rs/gltf` 从不发 Release，唯一的 `1.0.0` 是 2022 年的，
   `get_latest_release` 会把人骗到那里去）。只看 ① 就会得出「已经修了，可以用 nightly」，
   而实际上要用只能上 git 依赖。
10. **对照组必须真的走那条路径，不能只改配置。** `gltflite-import` 复制了关 `import` 版的 `main.rs`
   （`from_slice_without_validation` + 手工 `std::fs::read`），**根本没调 `gltf::import`**
   ⟹ 那次「开/关 `import`」的 A/B 对 data URI 问题**零信息量**，
   而且两次日志**逐字节相同**这个可疑现象当时没被追下去。
   **判据：如果两组实验的输出完全一样，先怀疑实验没做对，再下结论。**
11. **⭐ `cargo build` 报 EXIT=0 不等于新文件被编译过。** `src\gltf.rs` 写完之后第一次
   `cargo build --release --locked` 是绿的 —— 因为 `src\lib.rs` 里还没有 `pub mod gltf;`，
   那个文件**根本不在编译单元里**。接上模块声明后一次冒出 18 个错误。
   **判据：新增一个 `.rs` 文件后，第一件事是把它接进 `lib.rs` 再编译**，
   否则「0 error」证明的是「这段代码没被看过」。
12. **⚠️ 夹具里 accessor / bufferView 的下标就是引用编号，插到数组开头会把整张表挪位。**
   第一次写 morph 测试时把新 accessor 插在最前面，于是 `POSITION` 指向法线、`TEXCOORD_0`
   指向索引表 —— **而且不报错**，只在读取层炸出 `size_of` 不符，看起来像代码 bug。
   解法：夹具辅助函数一律**追加到末尾**，锚点取「数组收尾 + 下一节开头」（可连续追加）。
13. **⚠️ 公开条目的文档注释里不能写指向私有条目的 intra-doc 链接。**
   CI 的 `cargo doc`（`RUSTDOCFLAGS=-D warnings`）抓到三处：`pub struct GltfNotes` 与
   `pub fn read` 的文档里写了 `[`load_document`]`（私有函数），以及一句
   `[`resample`]`（**根本没有这个名字**）。
   **判据：`cargo doc` 要在 CI 之外也跑一次**（本地默认不开 `-D warnings`，
   而且**必须换一个新的 `--target-dir`**，否则 cargo 认为 doc 已最新直接跳过 ⟹ 假绿）。

**实现新增的探针 / 夹具**
- `docs\_probe\dump_cd_raw.js`：把 cdtexture 槽值的三种解释并排打出来
  （A 相对槽自身 / B 相对数组基址 / **C 绝对文件偏移** —— 官方写的是 **C**）
- `docs\_probe\oracle_dual_anim.js`：干净的动画 oracle（`$body` 与 `$sequence` 来自**不同文件**，
  绕开官方「同文件 ⟹ 静默 1 帧」的陷阱）
- `docs\_probe\_oracle_dual\dual_full_glb.qc` / `dual_anim.qc` / `dual_anim_glb.qc` / `dual_nocd.qc`
- `target\gltffirst\g_rig.qc` / `g_full.qc`：glTF 几何 + flex 的端到端夹具
- `target\dmxprobe\`：§6.5 那组「只换几何源格式」的受控实验（`.smd` / `.obj` / `.dmx` / `.fbx`）
