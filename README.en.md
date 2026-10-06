<div align="center">

<img src="assets/mdlc-icon.svg" width="128" height="128" alt="mdlc">

# mdlc

**Source engine model compiler** — an independent Rust rewrite of Valve's `studiomdl.exe`.

**English** · [简体中文](README.md)

</div>

It compiles a **TOML descriptor** or a **QC script** plus **SMD / FBX / glTF meshes** into the
`.mdl` + `.vvd` + `.dx90.vtx` that the Source engine loads (plus `.phy` when there is collision
data, and `.ani` when `$animblocksize` is used).

- **Language**: Rust **1.89+** (`edition 2024`)
- **Platforms**: no platform-specific APIs; prebuilt binaries for Windows / Linux / macOS (arm64 + x86_64)
- **License**: [GPL-3.0-only](LICENSE)

```powershell
git clone https://github.com/MoRanYue/mdlc.git
cd mdlc
cargo build --release
```

> ⚠️ **Legal notice**: this project is a **clean-room reimplementation**, based on the Source SDK
> headers, public format documentation, and **measurements** of official artifacts. It contains
> **no** Valve binaries or art assets. If you distribute a model compiled with it, make sure you
> have the rights to the source assets. Source and `studiomdl` are trademarks/works of Valve
> Corporation; this project is not affiliated with them.

---

## Table of contents

- [Quick start](#quick-start)
- [Outputs](#outputs)
- [Two input formats, one IR](#two-input-formats-one-ir)
- [TOML descriptor](#toml-descriptor)
- [QC support & drop-in Crowbar replacement](#qc-support--drop-in-crowbar-replacement)
- [Command-line reference](#command-line-reference)
- [Update check](#update-check)
- [Using as a library](#using-as-a-library)
- [Format limits: only what the format can express](#format-limits-only-what-the-format-can-express)
- [Automatic splitting of oversized meshes](#automatic-splitting-of-oversized-meshes)
- [Multiple LODs](#multiple-lods)
- [Known unimplemented](#known-unimplemented)
- [Further documentation](#further-documentation)

---

## Quick start

```powershell
cargo build --release

# Print a fully commented TOML template (every available table and field)
.\target\release\mdlc.exe template > myprop.toml

# Validate only, write nothing (exit code 0 = valid)
.\target\release\mdlc.exe check myprop.toml

# Compile → myprop.mdl / myprop.vvd / myprop.dx90.vtx
.\target\release\mdlc.exe build myprop.toml --out .\out

# Compile straight from QC (equivalent to qc2toml then build, descriptor never hits disk)
.\target\release\mdlc.exe build-qc myprop.qc --out .\out
```

The output of `mdlc template` is the **authoritative field reference** — it cannot drift from
what is actually supported.

> ⚠️ **Three easy traps (the template spells them out; repeated here):**
>
> 1. **`mass` is not in `[model]`, it is in `[physics]`.** Official studiomdl has **no**
>    top-level `$mass` — it only appears inside `$collisionmodel {}` / `$collisionjoints {}`,
>    and the same value is written to both `editparams.totalmass` in the `.phy` and `mass`
>    in the `.mdl` header. **Writing `mass = 1.0` under `[model]` is a parse failure**
>    (measured).
> 2. **`contents` defaults to `1` (`CONTENTS_SOLID`), not `0`.** Compiling a model without
>    `contents` and reading the header gives **1** at `+0x14C` (measured).
> 3. **`flags` on `[[bones]]` does not default to an unconditional `0x500`** — it is computed
>    per bone from how the bone is used and propagated up the parent chain (see the
>    [`[[bones]]`](#bones) section).
>
> The template also **does not list** `[physics]`, `[[animations]]`, `[[flex_descriptors]]`,
> `[[flex_controllers]]`, `[[flex_rules]]`, `[[flex_controller_ui]]`, `[[mouths]]`,
> `[[jiggle_bones]]`, `[[quat_interp_bones]]`, `[[bonecontrollers]]`,
> `include_models`, `skin_families`, `key_values`, `pose_parameters`,
> `flip_triangles`, `eyeballs`, `flexes`, `no_facial`, `section_frames` and others —
> they are all **supported**; their semantics are in the rest of this document.

> ⚠️ **Unknown key names are always hard errors**, including unknown top-level tables. A
> misspelled field is never silently ignored; parsing fails and lists every legal field name.

---

## Outputs

| Extension | Contents | When |
|---|---|---|
| `.mdl` | Header, bones, materials, bodypart/model/mesh, animation chains, sequences, flex, IK, jigglebone… | always |
| `.vvd` | Vertex pool (position/normal/UV/weights) + tangents; includes a fixup table with multiple LODs | always |
| `.dx90.vtx` | Index buffers (strip group / strip / vertex palette) | always |
| `.phy` | IVP collision (convex hulls / `$concave` / `$collisionjoints`) | with `[physics]` or `$collisionmodel` |
| `.ani` | External animation block | `[model].anim_block_size` or `$animblocksize` written, **and that animation has ≥ 2 frames** |

> ⚠️ **`.ani` is aligned to exactly one `studiomdl` build.** mdlc replicates
> `Left 4 Dead 2\bin\studiomdl.exe` (2024-06-04 build). `.ani` files floating around elsewhere
> sometimes come from **earlier builds with a different payload format** (header `+0` is always
> 28; those are 56/84/88/92) — using them as a baseline only produces an illusion of "all wrong".
> The container layer (416-byte header / `IDAG` / version 49) is consistent across builds.

> ⚠️ **Single-frame animations never go into `.ani`** — the predicate is
> `anim_block_size > 0 && numframes >= 2`; single-frame animations stay inline in the `.mdl`.
> The `.ani` filename is **forced** to `models/<modelname>.ani`, because the `.mdl` header
> stores exactly that path at `+0x15C`; a wrong name means the engine cannot find it.

The output root is `--out` (default: the current directory), with the relative path from the
model name layered on top. So `name = "models/mymod/myprop.mdl"` is written to
`<out>\models\mymod\myprop.mdl`.

All four files share the same **`checksum`** pairing token. It is **not a content hash** — the
engine and Crowbar only ever compare it, never compute it. mdlc derives it from the model name
with FNV-1a by default (stable, consistent across processes); `[model].checksum` overrides it.

On success stdout gets a summary (bone count, material count, vertex count, triangle count and
the byte size of each file).

---

## Two input formats, one IR

```text
TOML descriptor ──┐
                  ├──► IR ──► compile ──► writers ──► .mdl/.vvd/.vtx/.phy/.ani
QC script       ──┘
```

Both inputs **share a single IR**, so the writers do not care which side the input came from.
QC support was added later and **changed no writer code**.

Meshes are **not written in the descriptor** — real models have tens of thousands to hundreds of
thousands of vertices (the official `v_autoshotgun` has 388,765), and inlining them would bloat
the descriptor to hundreds of MB and make it unusable with text tools. The descriptor only
**references** mesh sources.

A mesh source can be **SMD**, **FBX** (`.fbx`, via `ufbx`, see
[`docs/fbx-support.md`](docs/fbx-support.md)) or **glTF / GLB** (`.gltf` / `.glb`, via the
`gltf` crate, see [`docs/gltf-support.md`](docs/gltf-support.md)).
**The format is determined by the extension, so it must be written in full**
— which is one reason extensions are not auto-appended (see `## Known unimplemented`).

> **FBX / glTF can be written directly into QC**, with no intermediate conversion. The default
> behaviour **matches the official tool line by line** (merge every mesh, always take the first
> NLA stack, auto-register shape keys as flex), while the official tool's **silent failures**
> become explicit messages in mdlc, and every implicit decision has an explicit override
> (nine `src*` options, see [`### FBX / glTF source options`](#fbx--gltf-source-options-src-9-of-them)).
>
> ⚠️ **glTF has no official baseline** (official `studiomdl.exe` does not recognise the format
> at all); its semantics were pinned down by **transitive comparison** — export the same Blender
> scene twice as `.fbx` + `.glb`, use the official tool on the `.fbx` as the baseline, then show
> that the glTF numbers imply the FBX numbers. The provenance of every rule is recorded in
> `docs/gltf-support.md` §5.
>
> ```qc
> $modelname "models/mymod/linnea.mdl"
> $cdmaterials "models/mymod/"
> $body body "linnea.fbx" srcpart "body"   // take only the body mesh
> // No expressions needed: shape keys are auto-registered
> ```

Division of responsibility:

| Content | Carried by | Corresponding QC |
|---|---|---|
| Model name / materials / bones / bodypart tree | TOML descriptor | `$modelname` / `$cdmaterials` / `$definebone` / `$bodygroup` |
| **Mesh (vertices, normals, UVs, skinning)** | **SMD / FBX / glTF file** | `studio "x.smd"` / `$body body "x.fbx"` / `$body body "x.glb"` |
| **Reference pose** | **frame 0 of the SMD `skeleton`** (FBX / glTF use the node's local TRS) | the reference SMD |
| **Expressions (flex)** | **frames of a `.vta`**, or **FBX shape keys / glTF morph targets** (auto-registered) | `flexfile` + `flex` |

---

## TOML descriptor

> Only **structure and key points** are listed below; the full semantics and defaults of every
> field are whatever `mdlc template` prints.

### `[model]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `name` | string | **required** | Output path, relative to the game directory; backslashes are normalised to forward slashes |
| `version` | int | `49` | Only **44 / 48 / 49** are supported |
| `checksum` | int | derived from the model name | Pairing token for the four files; not a content hash |
| `static_prop` | bool | `false` | `$staticprop`, sets header bit `0x10` automatically |
| `surface_prop` | string | — | `$surfaceprop`; also the default `surface_prop` of every bone |
| `eye_position` / `illum_position` | `[f32; 3]` | — | `$eyeposition` / `$illumposition` |
| `max_eye_deflection` | f32 | `0` | `$maxeyedeflection`, **written as an angle** (converted to `cos` on disk). Unset = 0, and the engine falls back to `cos(30°)` |
| `hull_min` / `hull_max` | `[f32; 3]` | computed from SMD vertices | Bounding box |
| `extra_flags` | int | `0` | Extra `STUDIOHDR_FLAGS_*` bits |
| `contents` | int | **`1`** (`CONTENTS_SOLID`) | `$contents`. The official `s_nDefaultContents = CONTENTS_SOLID`, **not 0** |
| `skip_bone_in_bbox` | bool | `false` | `$skipboneinbbox` |
| `optimize_vtx` | bool | **`false`** | Vertex cache optimisation (`meshopt`); reorders indices only, does not change geometry. QC: `$optimizevtx` (**mdlc extension**) |
| `split_oversized_meshes` | bool | **`true`** | Automatic splitting of oversized meshes, see [below](#automatic-splitting-of-oversized-meshes). QC: `$nosplitoversizedmeshes` / `$splitoversizedmeshes` (**mdlc extensions**) |
| `key_values` | string | — | `$keyvalues` contents (without the outer `mdlkeyvalue` wrapper) |
| `pose_parameters` | table array | `[]` | `$poseparameter` |
| `realign_bones` | bool | `false` | `$realignbones` |
| `anim_block_size` | int | — | `$animblocksize`, triggers an external `.ani` animation block |
| `section_frames` | `[int, int]` | — | The **global** value of a top-level `$sectionframes <frames per section> <threshold>` (not a per-sequence field) |

### `[materials]`

```toml
[materials]
search_paths = ["models/mymod"]              # $cdmaterials
skin_families = [[0, 1, 2], [3, 4]]          # optional, $texturegroup
textures = [ { name = "models/mymod/myprop" } ]
```

Material names in the SMD are mapped to `textures` in **order of first appearance**; a name
carrying a `search_paths` prefix has it stripped on disk (matching studiomdl).

### `[[bones]]`

```toml
[[bones]]
name = "root"                # the root bone must come first; parent may only point at an earlier bone
# position = [0.0, 0.0, 0.0] # empty = taken from frame 0 of the SMD skeleton
# rotation = [0.0, 0.0, 0.0] # degrees; empty = same as above
# flags = 1280               # empty = computed per bone from usage (see below)
# surface_prop = "metal"     # defaults to inheriting [model].surface_prop
bonemerge = true             # $bonemerge: allow this bone to be merged
```

When `flags` is empty it is computed per bone from **usage** (used by vertices / by hitboxes / by
attachments / by ikchains / bonemerge) and propagated up the parent chain, bit-for-bit identical
to the official tool. `0x500` (`DEFAULT_BONE_FLAGS`) is merely the result when a bone really is
used by both vertices and hitboxes; it is **not** an unconditional default — always writing
`0x500` would mark vertex-unused bones as used.

`[[bones]]` also accepts three fields relating to `$definebone`'s `realign`:

- `pre_aligned` (bool, optional) — whether `$realignbones` skips this bone. When empty it is
  **inferred** from whether an explicit `position`/`rotation` was written (the only official
  command that can write a reference pose, `$definebone`, always sets this flag);
- `realign_position` / `realign_rotation` — `srcRealign`, the **last 6 numbers** of
  `$definebone` (only present when all 12 numbers are written; in the 6-number form it is the
  identity). `realign_rotation` is in **degrees**, in the same order as `rotation`. ⚠️ QC's
  `$definebone` argument order is `<x> <y> <z> <pitch> <yaw> <roll>`, so
  `pitch = rot[1]`, `yaw = rot[2]`, `roll = rot[0]`.

### `[[bodyparts]]` / `[[bodyparts.models]]` / `.lods`

```toml
[[bodyparts]]
name = "body"
base = 1                     # bodygroup preset weight base (optional, default 1)
models = []                  # required key; at least one

[[bodyparts.models]]
smd = "myprop-ref.smd"       # LOD 0 (relative to the descriptor's directory); required
flip_triangles = true        # default true (Source front faces are CW)

[[bodyparts.models.lods]]
smd = "myprop-lod1.smd"
switch_point = 20.0          # optional, default 20·2^k
```

`[[bodyparts.models]]` also accepts `eyeballs` (`$eyeball`/`$eyelid`) and `flexes`
(`$flex`/`$flexpair`, pointing at a `.vta`), plus `name` (defaults to the SMD filename).

### `[hitboxes]` / `[[attachments]]`

```toml
[hitboxes]
set_name = "default"         # optional
[[hitboxes.boxes]]
bone = "root"
bbmin = [-8.0, -8.0, -8.0]
bbmax = [ 8.0,  8.0,  8.0]
# group = 0                  # intersection group
# name  = "body"

[[attachments]]
name = "muzzle"              # $attachment
bone = "tip"
position = [0.0, 0.0, 8.0]
rotation = [0.0, 0.0, 0.0]   # degrees
```

`[[attachments]]` also accepts four fields:

- `absolute` (bool, default `false`) — the official `IS_ABSOLUTE`. The written `local` describes
  an absolute pose in **world coordinates**, independent of the bone's pose; ⚠️ this bit is
  **not written to disk** (the official tool writes only `flags`);
- `absolute_rotation` (bool, optional) — whether `absolute` **overrides** the rotation of
  `local`. The official option loop is last-writer-wins: `absolute` writes
  `AngleIMatrix(g_defaultrotation)`, `rotate` writes `AngleMatrix(angles)`, and `IS_ABSOLUTE`
  is **set in both cases** — only the rotation source differs. Empty ⟹ **follows `absolute`**
  (also the most common official form);
- `rigid` (bool, default `false`) — the official `IS_RIGID`. Also **not written to disk**; it
  only affects bone retention (walking up the parent chain to the first bone referenced by
  vertices);
- `flags` (int, optional) — bits written straight into `mstudioattachment_t.flags`. Officially
  only `world_align` (`0x10000`) can land here; `absolute`/`rigid` go into a different field and
  are not written to the artifact.

**When no explicit hitbox is written, one `default` set is generated automatically** (the
official `SetupHitBoxes` semantics) with the `autogenerated` flag set — real L4D2 models all have
hitboxes; without them bullets cannot hit anything.

### `[[sequences]]` and `[[animations]]`

```toml
# Single-animation sequence
[[sequences]]
name = "idle"
smd = "idle.smd"
fps = 30.0
looping = true
# delta = true                 # $sequence ... delta (sets DELTA|POST together)
# activity = "ACT_VM_IDLE"
# activity_weight = 1
# weight_list = "upper"        # references [[weight_lists]] by name
# no_auto_ik = true            # suppress the auto-added IK_RELEASE rule

# Blend grid sequence
[[sequences]]
name = "walk"
blends = ["a_run", "a_idle", "a_run", "a_idle"]   # must be a perfect square
blend_width = 2
[[sequences.blend_params]]
parameter = "move_x"
start = -1.0
end = 1.0
```

Sequence-level fields (mirroring the `$sequence` block options in QC):
`fps` / `looping` / `delta` / `activity` / `activity_weight` / `events` /
`fade_in` / `fade_out` / `forward_declared` / `no_auto_ik` / `ik_rules` /
`iklocks` / `blends` / `blend_width` / `blend_params` / `blend_ref` /
`blend_comp` / `blend_center` / `auto_layers` / `movements` / `section_frames` /
`section_threshold` / `extra_flags` / `weight_list` / `subtract` /
`subtract_frame` / `num_frames`.

`[[animations]]` (QC's `$animation`) declares **animations that several sequences can reuse**.
The official tool keeps animdescs in a global pool and `$sequence` merely references them — so an
`$animation` has exactly **one** copy of its data, shared when several sequences reference the
same slot. That leads to an easy mistake: `weightlist` / `numframes` / `subtract` / `ikrule`
modify the **shared animation object**, not just the sequence you wrote them on.

`forward_declared = true` corresponds to `$declaresequence` — a **forward-declared empty-shell
sequence** (the core mechanism of survivor mods): the main model declares a pile of empty names
and the real animations live in the `anim_<survivor>.mdl` pulled in by `$includemodel`, swapped
in by name at load time. An empty shell **must not write `smd`** (the official tool does not even
allocate a `panim`); writing one is rejected.

> ⚠️ **Known gap: `weight_list` / `num_frames` / `subtract` are silently dropped on blend
> sequences.** These three options are currently **implemented only on single-animation
> sequences**: adding them to a blend sequence does not error, but has **no effect whatsoever**
> (the artifact is **byte-identical** to one without them, measured). So this is **silent data
> loss**, not a parse error — write them on a single-animation sequence if you need them to work.

### `[[weight_lists]]`

```toml
[[weight_lists]]
name = "upper"
[[weight_lists.bones]]
bone = "mid"
weight = 0.5
pos_weight = 0.25            # optional, only used for compile-time IK error, not written to disk
```

Weight lists drive the **reconstruction scaling of delta animations**: a bone with `s = 0` does
not participate in that sequence's delta blending at all (it keeps its reference pose). Mod
authors use it for "only the upper body moves".

> ⚠️ The semantics are **not** "bones not listed are 1". The official algorithm is: ① the
> **root bone of a named list defaults to 0**; ② explicit entries override; ③ values are
> **inherited** down the parent chain. For the chain `root → mid → leaf → tip`, writing only
> `mid 0.5` yields `[0, 0.5, 0.5, 0.5]`.

### `[physics]`

```toml
[physics]
smd = "physics.smd"          # the SMD of $collisionmodel
concave = true               # $concave: split into several convex pieces by connected component
joints = false               # true = $collisionjoints (a ragdoll with one solid per bone)
# mass / damping / rot_damping / inertia / drag / root_bone
# mass_center / auto_mass / no_self_collisions
# joint_overrides / constraints / animated_friction / collision_pairs / merge
```

`concave` and `joints` are **mutually exclusive**: a ragdoll already has one convex hull per
bone, and `$concave` only applies to the single-solid prop path.

> ⚠️ **`concave` is not VHACD-style "volume decomposition".** The official `$concave` is
> **connected-component decomposition**: weld vertices (same position **and** normal angle
> < 2°) → union-find over shared welded vertices → one convex hull per connected component. So a
> **connected** concave body (a U shape, a torus) yields exactly **one** convex hull that
> **fills in** the concavity. mdlc implements it this way.
>
> To get a genuinely concave collision body that **preserves the concavity**, use the standalone
> `mdlc phy --vhacd` (parry3d's VHACD) — that is **non-official semantics** and **cannot** be
> triggered from the TOML/QC compile path.

The world-space transform of collision geometry uses **frame 0 of the first sequence**, not the
pose of the collision SMD itself. Under `[physics]`, `joint_overrides` / `constraints` /
`collision_pairs` **error out** when `joints = false` (they only make sense for a ragdoll), and
so does `auto_mass = true` — it needs a surface-material density table that mdlc does not have.

### Other tables

| Table | QC equivalent | Notes |
|---|---|---|
| `[[ikchains]]` | `$ikchain` | `name` / `bone` (the **end** bone) / `knee_dir` |
| `[[ik_autoplay_locks]]` | `$ikautoplaylock` | `chain` / `pos_weight` / `local_q_weight` |
| `[[flex_descriptors]]` | `flex` / `eyelid` / `mouth` | `name` |
| `[[flex_controllers]]` | `flexcontroller` | `name` / **`type`** / `min` / `max` |
| `[[flex_rules]]` | `%<flex> = <expr>` | `flex` + `ops` (each with `op` + `value`/`controller`/`flexdesc`) |
| `[[flex_controller_ui]]` | — | `name` / `stereo` / `left` / `right` |
| `[[mouths]]` | `mouth` | `index` (explicit, drives `g_nummouths`) / `flexdesc` / `bone` / `forward` |
| `[[jiggle_bones]]` | `$jigglebone` | `bone` + `is_flexible` / `is_rigid` / `has_base_spring` + **`writes`** |
| `[[quat_interp_bones]]` | `$proceduralbones` (proctype 2) | `bone` / `control` / `base_pos` / `triggers` |
| `[[bonecontrollers]]` | `$controller` | L4D2 has deprecated the feature, but the section still holds a slot. ⚠️ the key is the literal **`type_`** (with an underscore) |
| `include_models` | `$includemodel` | Top-level string array; **you must write the `models/` prefix yourself**, mdlc will not add it |

> ⚠️ **Only two key names are special**: the type key of `[[sequences.ik_rules]]` and
> `[[flex_controllers]]` is written **`type`** in TOML (not `kind`). Every other key name is
> exactly what the tables above say. Three further keys have **fixed choices** as values and fail
> to parse if misspelled: `op` in `[[flex_rules]]` (21 values, below), `type` in `ik_rules`, and
> `wrap` in `pose_parameters`.

> ⚠️ **`[[flex_rules]]`'s `op` has 21 values**: `const` `fetch1` `fetch2` `add`
> `sub` `mul` `div` `neg` `exp` `open` `close` `comma` `max` `min` `2way_0`
> `2way_1` `nway` `combo` `dominate` `dme_lower_eyelid` `dme_upper_eyelid`.

> ⚠️ **Pointing `include_models` at a nonexistent file still compiles** — the official tool
> **only writes the name** and never reads the included `.mdl` (merging bones/sequences is done
> by the **engine** at runtime).

> **`[[ikchains]]` adds IK rules automatically**: whenever a model has an IK chain, the official
> tool appends a `type = 4` (`IK_RELEASE`) rule to every chain that has no explicit ikrule. So in
> the vast majority of cases you do **not** need to write `[[sequences.ik_rules]]` by hand. To
> suppress it, write `no_auto_ik = true` on that sequence.

### A TOML gotcha

**Top-level keys must be written before all `[[tables]]`.** For example, putting
`bonemerge = [...]` after `[[attachments]]` makes TOML parse it as a field of `attachments` and
fail. This implementation therefore puts `bonemerge` inside `[[bones]]` (`bonemerge = true`)
rather than defining a top-level array.

---

## QC support & drop-in Crowbar replacement

### Official-compatible form

```powershell
.\target\release\mdlc.exe -game "<gamedir>" [-nop4] [-verbose] myprop.qc
```

Artifacts are written to `<gamedir>\models\<$modelname>` — the same rule as official
`studiomdl`.

**Why it is needed**: [Crowbar](https://github.com/ZeqMacaw/Crowbar) treats the compiler path as
an **opaque configuration item**, passing only `-game "<gamedir>" <options> "<qc filename>"` and
setting the CWD to the QC's directory. It judges success by exactly two things — ① the compiler
produced output; ② `<gamedir>\models\<$modelname>.mdl` exists. **It never looks at the exit code
and never parses error text.** So pointing Crowbar's "compiler path" at `mdlc.exe` is a drop-in
replacement.

The compatibility layer handles the official tool's **single-dash long options** (clap only
accepts `--long`, so normalisation is required). Known **unimplemented** official options
(`-minlod`, `-striplods`, `-definebones`, `-printbones`, `-t`, `-a`) are **warned about and
ignored**, never silently changing the artifact (of these, `-minlod` / `-t` / `-a` take a value
officially, and omitting the value is still a usage error).

> ⚠️ **Which stream diagnostics go to is decided by the invocation form**: official `studiomdl`
> writes `ERROR:` to **stdout**, and Crowbar's "is the compiler alive" flag is only set in its
> stdout handler. So in the compatible form mdlc's diagnostics also go to **stdout** (matching
> the official tool); mdlc-native subcommands still use stderr. The predicate is the
> **invocation form**, not the parent process name — zero dependencies, cross-platform.

### QC command coverage

The QC front end implements full lexing/parsing, `$include`, `$definevariable`,
`$pushd`/`$popd`, a `$cd` directory stack, and the following commands:

`$modelname` `$cd` `$pushd` `$popd` `$cdmaterials` `$surfaceprop`
`$contents` `$eyeposition` `$illumposition` `$maxeyedeflection` `$bbox` `$cbox`
`$staticprop` `$realignbones` `$skipboneinbbox` `$animblocksize` `$keyvalues`
`$sectionframes` `$poseparameter` `$texturegroup` `$body` `$bodygroup` `$model`
`$sequence` `$animation` `$definebone` `$bonemerge` `$attachment` `$hboxset`
`$hbox` `$ikchain` `$ikautoplaylock` `$includemodel` `$lod` `$shadowlod`
`$jigglebone` `$proceduralbones` `$collisionmodel` `$collisionjoints`
`$jointsurfaceprop` `$weightlist` `$declaresequence`
`$jointconstrain` `$animatedfriction` `$noselfcollisions` `$jointcollide`
`$jointmerge` `$unlockdefinebones`, plus a batch of header flags (`$opaque` `$mostlyopaque`
`$noforcedfade` `$casttextureshadows` `$ambientboost` `$donotcastshadows`
`$forcephonemecrossfade` `$constantdirectionallight`).

**Known but deliberately unsupported**:

| Command | Behaviour |
|---|---|
| `$nekomodel` | **Explicit error** — it points at a DMX source, and mdlc does not implement DMX (the official tool also delegates to `dmxconvert.exe`) |
| `$defaultweightlist` | **Explicit error** — it would override **every** sequence without an explicit weightlist, and silently ignoring it produces models that "look right but are semantically wrong". For an equivalent effect use `$weightlist` and reference it explicitly per sequence |
| `$fakevta` | The whole block is skipped (no trace in the artifact) |
| `$scale` | **Accepted but has no effect** (the scale is only recorded, never used in computation). Use `srcscale` instead |
| `$cbox` / `$maxconvexpieces` / `$phyname`, `height`/`pad`/`floor`/`center` of `$ikchain`, `x_and_z_axes` of `$attachment`, `usesequence` of ikrule | Arguments are consumed but **not written to disk** (or are expressed equivalently by other fields) |
| A batch of rare commands (`$minlod` `$maxverts` `$renamebone` `$hierarchy` `$collapsebones` `$screenalign` `$upaxis` `$origin` `$maxbones` `$controller` …) | **The line is ignored** (no error, no trace) |
| Any other unknown command | Error (matching the official `bad command`) |

> ⚠️ **An unknown keyword inside a `$sequence` block is treated as an "animation name".** For
> example, the `nodefaults` in `$sequence "x" "a.smd" nodefaults` goes into `blends` and then
> fails with `找不到动画 "nodefaults"` when the animation pool is queried. **Not a silent
> ignore, but the error message is misleading.**

> ⚠️ **`$maxverts` is ignored** (not implemented, and no error either). It is a **non-official
> extension** of third-party NekoMDL that splits an oversized model into several **bodyparts** by
> triangle — which changes the index-based selection semantics of `$bodygroup`. mdlc solves the
> same problem the equivalent and safer way, with
> [multiple meshes inside one model](#automatic-splitting-of-oversized-meshes).

> ⚠️ **"Animation options" can be written inside a `$sequence` block.** `subtract` / `numframes`
> / `weightlist` / `ikrule` / `addlayer` / `blendlayer` / `calcblend` are
> **equally legal inside `$sequence`**, and they modify the **shared animation object being
> referenced** — that is, writing one inside a sequence affects everywhere that animation is
> referenced.

### mdlc-extended QC commands

The following 3 are **not official commands**; they are mdlc's own extensions. The naming rule
maps one-to-one onto TOML fields (drop the underscores from the field name and prefix `$`):

| Command | Equivalent TOML | Notes |
|---|---|---|
| `$optimizevtx` | `optimize_vtx = true` | Turns on vertex cache optimisation; equivalent to the `--optimize-vtx` command-line flag |
| `$nosplitoversizedmeshes` | `split_oversized_meshes = false` | Turns off [automatic splitting of oversized meshes](#automatic-splitting-of-oversized-meshes) and errors out on an oversized mesh |
| `$splitoversizedmeshes` | `split_oversized_meshes = true` | Turns the previous one **back on** (the default is already `true`; it exists so a main QC can undo what an included `.qci` turned off) |

All three are **bare flags** (no argument, consuming only their own token) and their **command
names are case-insensitive** (matching the official tokenizer). QC is interpreted top to bottom,
**last write wins**.

```qc
$modelname "models/mymod/myprop.mdl"
$body body "myprop.smd"
$optimizevtx              ; turn on vertex cache optimisation
$nosplitoversizedmeshes   ; turn off automatic splitting (error out on an oversized mesh)
```

> ⚠️ **Why these are mdlc extensions rather than official command names.** The official
> `studiomdl.exe` dispatch table has **no** keyword for either feature (`optimize` / `vcache` /
> `nvtristrip` / `split` / `oversized` all score 0 hits) — the official tool exposes vertex cache
> optimisation as the **command-line** switch `-nvtristrip`, and **rejects** oversized meshes
> outright (`ERROR: too many indices in source`). Third-party NekoMDL has neither QC command
> either (its `$maxverts` does something else: it splits an oversized model into **new
> bodyparts**, which mdlc deliberately does not copy, see
> [automatic splitting of oversized meshes](#automatic-splitting-of-oversized-meshes)).
>
> So a real `studiomdl.exe` reports `bad command` for all three. **A QC containing them cannot be
> fed to the official tool as-is**; for cross-tool compatibility use the corresponding TOML
> fields instead.

> ⚠️ **`$optimizevtx` has no inverse command.** `optimize_vtx` already defaults to `false`, so a
> "turn it off" command would have no use (the official bare flags such as `$staticprop` have no
> inverse either). Likewise the command-line `--optimize-vtx` can only turn it on, never off.

### FBX / glTF source options (`src*`, 9 of them)

When the mesh source is `.fbx` or `.gltf` / `.glb`, the importer makes a series of **implicit
decisions**, all of them **silent**: merge every mesh, always take the first NLA stack, use the
material names straight from the file, carry the axes over verbatim. mdlc's default behaviour
**matches the official tool line by line**, but turns those decisions into **explicit syntax** —
not writing them means the default, writing them changes it.

⭐ **All nine work on glTF too**, because they are **named after concepts** (the format comes from
the file extension): that is why it is `srcpart` and not `fbxpart`. Adding glTF support **added
zero new syntax**.

> ⚠️ **The one exception is unit scaling**: on the FBX path the official tool only honours `LclS`
> on nodes and completely ignores `UnitScaleFactor`, so a model exported with Blender's defaults
> (`Apply Scalings` = "All Local", ×100 baked into the nodes) ends up with **bones 100× larger
> than the mesh**, and **mesh positions left in centimetres while mesh sizes are in metres**.
> mdlc **deliberately fixes this** (neither vertices nor bones are scaled): both export styles
> compile to **value-identical** artifacts, so you need not care about that Blender option.
> Details in [`docs/fbx-support.md`](docs/fbx-support.md).
>
> ⚠️ **The problem does not exist on the glTF side** — Blender's glTF exporter does not write
> `LclS`, so a `.glb` is self-consistent from the start. Both inputs compile to models of the
> same magnitude.

| Command | Written where | Effect | When omitted (= official) |
|---|---|---|---|
| `srcpart "name"` | inline in or inside a `$body` / `$model` block | take only meshes with these names; **repeatable** | all meshes merged into one part |
| `srcmaterial "name"` | same | fallback name when the file has no material | `debug/debugempty` |
| `srcscale 1.0` | same | uniform scale (vertices and bones multiplied together, **ratios unchanged**) | 1.0 |
| `srcaxis "z"` | same | force the up axis (`y` / `z`) | no intervention (root transform carried over verbatim) |
| `srcstack "name"` | inside a `$sequence` / `$animation` block | which animation stack to use | **the first one** (glTF selects by `animation.name()`) |
| `srcfps 30` | same | animation resample rate | 30 |
| `srcshapekey "name"` | inside a `$model` block | take only these shape keys / morph targets, **and order them**; repeatable | all of them, in file order |
| `srcshapekeyorder "name"` | same | order only, no filtering | file order |
| `srcshapekeyignore` | same | ignore all of them (register no flex) | register all |

```qc
$modelname "models/mymod/linnea.mdl"
$cdmaterials "models/mymod/"

// One FBX with body / hair / eyes meshes; take only body
$body body "linnea.fbx" srcpart "body" srcmaterial "face"

// glTF is written the same way (format comes from the extension, syntax does not change)
$body body "linnea.glb" srcpart "body"

// Multi-stack FBX: the official tool always takes the first; here we ask for run by name
$sequence run "anim.fbx" srcstack "run" srcfps 30

// Expressions: nothing to write, shape keys / morph targets auto-register; to control them:
$model "face" "linnea_face.fbx" {
    srcshapekey "smile"
    srcshapekey "blink"
}
```

⭐ **The default path for expressions (flex) needs no new syntax at all.** FBX shape keys and
glTF morph targets are both **auto-registered** as flexdesc + flexcontroller + flexrule +
payload, field-for-field identical to the official tool.
The three `srcshapekey*` commands are only needed when you want to **filter or reorder**.

> ⚠️ **`srcpart` / `srcshapekey` / `srcshapekeyorder` read one token each**
> (which is why they are repeatable). Do **not** write `srcpart "body" "hair"` — the second name
> is treated as a mesh name rather than "another option", and the options after it get swallowed.
> Quote mesh names containing spaces: `srcpart "my mesh"`.

> ⚠️ **One default behaviour is an "error" rather than new syntax** (so **a QC without new syntax
> still cannot run on the official tool**): writing `flexfile "<some.fbx>"` + `flex` on an FBX
> source — the official tool **always crashes**, mdlc errors out. It only triggers when it would
> **actually crash**. The full list is in the deviation table in
> [`docs/fbx-support.md`](docs/fbx-support.md) §4.6.

> ⭐ **The same `.fbx` as both mesh source and animation source: mdlc compiles it normally plus
> one notice.** The official tool **silently emits only 1 frame** in this combination (measured
> exit=0, no warning) — but FBX is by design a container holding both mesh and animation, so mdlc
> **does not copy this degenerate behaviour**; it samples all frames and merely notes that "a
> frame-count difference against the official tool is expected here; split the animation into its
> own FBX if you want the two to agree". The notice is only emitted when mdlc really sampled
> **> 1** frame (a static FBX is 1 frame on both sides, no notice).

> ⚠️ **A QC containing `src*` cannot be fed to the official tool** (it reports `bad command`),
> for the same reason as the three `$optimizevtx` commands. For cross-tool compatibility use the
> TOML-side fields (`src_parts` / `src_material` / `src_scale` / `src_axis` / `src_stack` /
> `src_fps` / `src_shape_keys` / `src_shape_key_order` / `src_shape_key_ignore`).

### FBX / glTF diagnostics

The official tool **passes silently** (`exit=0`) in these situations, but the result is usually
not what you wanted. mdlc prints a `提示：` line:

| Situation | Message |
|---|---|
| Several meshes merged into one part | lists the mesh names + "use `srcpart` or split into several `$body` to keep them apart" |
| A mesh has no material | "synthesised `debug/debugempty`; write `srcmaterial` to use something else" |
| Several animation stacks and no `srcstack` | lists every stack name + "only the first is used by default" |
| Shape keys / morph targets present (already auto-registered as flex) | lists names and frame numbers + "use `srcshapekey*` to control them" |

All four are **notices, not errors**, and have **zero effect on `.smd` projects** (not even one
extra line of output). One more notice of the same kind (the same FBX as both mesh and animation
source) is described in the block above.

glTF has three notices of its own (see `docs/gltf-support.md` §6.3):

| Situation | Type | Notes |
|---|---|---|
| An accessor has no `bufferView` | notice | Treated as **all zeros** per spec (which is exactly what Blender writes for a morph target with all-zero normal offsets) |
| A channel with `CubicSpline` interpolation | notice | Resampled as **linear** (tangent values dropped) |
| A channel driving `MorphTargetWeights` | notice | **Ignored** — expressions are controlled only by QC's `flex` statements |
| Draco / meshopt compression extensions used | **error** | mdlc does not decompress (`KHR_draco_mesh_compression` / `EXT_meshopt_compression`) |
| `data:` URI decoding fails | **error** | It must not silently degrade to zero geometry |

---

## Command-line reference

### mdlc-native form

```text
mdlc build <model.toml> [--out <dir>] [--optimize-vtx]
mdlc check <model.toml>
mdlc build-qc <model.qc> [--out <dir>] [--optimize-vtx]
mdlc qc2toml <model.qc> [--out <path.toml>]
mdlc phy <in.smd> <out.phy> [--checksum N] [--mass F] [--surfaceprop S]
                          [--concave] [--vhacd] [--decompose] [--ragdoll]
mdlc vvd-info <file.vvd>
mdlc vvd-roundtrip <file.vvd>
mdlc template
mdlc --version | -V
```

**Both the block above and the table below are only a quick reference** — the authoritative usage
is `mdlc --help` / `mdlc <subcommand> --help`.

| Subcommand | Purpose |
|---|---|
| `build` | TOML descriptor → `.mdl`/`.vvd`/`.vtx` (the main line) |
| `check` | Validate only (reads mesh sources), writes nothing. Exit code 0=valid, 1=descriptor or compile error, 2=file unreadable or TOML syntax error |
| `build-qc` | Compile straight from QC; the intermediate descriptor never hits disk |
| `qc2toml` | QC → TOML (**writes text only, does not compile**). For migration or manually checking the parse result |
| `phy` | Compute a convex hull from SMD triangles and write a `.phy` |
| `vvd-info` | Parse and print the VVD header, statistics and self-consistency check results |
| `vvd-roundtrip` | Read a VVD, write it back and compare byte for byte (to confirm the writer matches the format exactly) |
| `template` | Print the fully commented TOML template |

`--version` / `-V` prints `mdlc <version>`. ⚠️ **The official-compatible form does not recognise
it** (official `studiomdl` has no such option), and the two spellings behave differently:

- `-V` is treated as an **unknown single-dash option and dropped**, with a warning line; the
  compile proceeds normally (exit code 0);
- `--version` is a **hard error** (`unexpected argument`, exit code 2) — the official form
  deliberately disables the version flag, so clap does not recognise it.

Crowbar passes neither, so this does not affect drop-in replacement.

### Help and error localization

Help and error messages **switch automatically with the system display language** (falling back
to English when there is no match). So on a Chinese system `mdlc -h` prints:

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

(`mdlc --help` is the **long help**: `-h` / `-V` expand to two lines with full descriptions; the
`参数:` / `选项:` section headings inside subcommand help are localized too.)

> Colour **only appears on a direct terminal** and degrades to plain text when redirected or
> captured through a pipe — so no ANSI escape codes are ever pushed into host logs such as
> Crowbar's.

> ⚠️ **The official-compatible form is unaffected**: its help/errors stay English, matching the
> output shape of official `studiomdl`.

`--optimize-vtx` uses `meshopt` to reorder the indices of each strip group to improve the GPU's
post-transform vertex cache hit rate. It **only changes index order**; the vertex pool and the
triangle set are unchanged, so rendering is identical. It defaults to **off**, keeping artifacts
byte-identical to before.

It is an `||` relationship with `[model].optimize_vtx` (either being true turns it on), so it
**can only be turned on, never off**; the equivalent QC command is `$optimizevtx` (see
[mdlc-extended QC commands](#mdlc-extended-qc-commands)).

---

## Update check

Every run of `mdlc` checks for a new version in the background — **without blocking the
compile**: the network I/O happens in a spawned child process, and the main process only reads a
~100-byte cache file.

| Item | Value |
|---|---|
| Interval | 24 hours; **no process is spawned** within that window |
| When the notice appears | **The next run** (run 1 spawns the child to check, run 2 reads the cache and prints) |
| Turning it off | Environment variable `MDLC_NO_UPDATE_CHECK=1` |
| CI | Skipped automatically when the `CI` environment variable is detected |
| Debugging | `MDLC_UPDATE_DEBUG=1` prints the failure reason to stderr |
| Cache | Windows `%LOCALAPPDATA%\mdlc\update-check.json`; elsewhere `$XDG_CACHE_HOME/mdlc/` or `~/.cache/mdlc/` |

The notice follows the existing rules: **the official-compatible form goes to stdout** (matching
official `studiomdl`, and Crowbar only reads stdout), while mdlc-native subcommands use stderr.

---

## Using as a library

`mdlc` is also a library. To embed the compiler in a third-party GUI or build system, **do not
hand-wire the pipeline yourself** — use `mdlc::pipeline`:

```rust
use std::path::Path;
use mdlc::model::ModelDesc;
use mdlc::pipeline::{self, PipelineOptions};

let text = std::fs::read_to_string("model.toml")?;
let desc = ModelDesc::from_toml(&text)?;

// Compile only, touching no filesystem: get the four files' bytes + the compile-time IR.
let out = pipeline::build(&desc, Path::new("."), PipelineOptions::default())?;

// Or write straight to disk (creating directories from `$modelname`).
let paths = pipeline::write_files(&out, Path::new("out"))?;
println!("{}", paths.mdl.display());
```

`pipeline::build` only reads files; `write_files` only writes them. **The two steps exist so a
GUI can show a summary or ask for confirmation before writing.**

### Why you must use it

`compile()` only does "descriptor → compile-time IR"; it **does not handle the collision SMD at
all** and writes no files. The step most easily missed when hand-wiring the pipeline is
"collision SMD → `physicsbone`":

```rust
// Miss this and `.mdl`/`.vvd`/`.dx90.vtx` are all correct, but physicsbone is silently all 0.
if let Some(cs) = &collision_smd {
    let parents = mdlc::compile::bone_parents(&compiled.desc);
    compiled.physics_bone = mdlc::phy::physics_bone_table(cs, compiled.desc.bones.len(), &parents);
}
```

### Errors and exit codes

`PipelineError` is an **enum** — one variant per failure point — and `kind()` maps onto the same
exit codes as `mdlc.exe`:

| Variant | `kind()` | Exit code | Trigger |
|---|---|---|---|
| `Compile(Vec<CompileError>)` | `Build` | 1 | Descriptor/QC validation failure, source SMD unreadable |
| `ParseCollisionSmd { path, source }` | `Build` | 1 | Collision SMD parse failure |
| `WriteMdl(WriteError)` | `Build` | 1 | `.mdl` write failure |
| `BuildVvd` / `EncodeVvd` / `CheckVvd` | `Build` | 1 | VVD build / encode / self-check failure |
| `WriteVtx` / `CheckVtx` | `Build` | 1 | VTX write / self-check failure |
| `BuildPhy` / `CheckPhy` | `Build` | 1 | PHY build / self-check failure |
| `ResolveCollisionSmd(SrcError)` | `Io` | 2 | Collision SMD path resolution failure |
| `ReadCollisionSmd { path, source }` | `Io` | 2 | Collision SMD unreadable |
| `CreateDir { path, source }` | `Io` | 2 | Directory creation failure |
| `WriteFile { path, source }` | `Io` | 2 | File write failure |

`kind()` is **exhaustive** — there is no `_ => Build` fallback, so adding a variant forces the
compiler to make you classify it.

Underlying errors are **preserved** (`#[source]`) instead of being flattened into a string by
`format!`: `ReadCollisionSmd`'s `source` is a `std::io::Error`, so you can test
`.downcast_ref::<std::io::Error>().kind() == ErrorKind::NotFound` rather than matching on
localized text.

`lines()` yields exactly the same text as the CLI line for line (a compile failure is multiple
lines, everything else is one); `Display` is them joined together. A GUI can render `lines()`
straight into its log without assembling strings itself.

### Summary

`PipelineOutput::summary_lines(&paths)` reproduces the table `mdlc.exe` prints on success
(model / version / checksum / statistics / each artifact's byte count and path / `**编译成功**`).
The CLI itself just `println!`s it line by line — so what a GUI shows and what the command line
shows are **word for word identical**.

---

## Format limits: only what the format can express

`studiomdl` has a batch of **artificial** limits (65536 vertices per model, 32 materials, …) that
are not constraints of the file format. This implementation **does not replicate** them, keeping
only the hard limits that follow from **bit widths / offset arithmetic**:

| Dimension | Limit | Basis |
|---|---|---|
| **Index** of a **referenced** bone | **127** | Both VVD `mstudioboneweight_t.bone[]` and VTX `Vertex_t.boneID[]` are **signed** `char` |
| **Total** bones | no limit | The `mstudiobone_t` array length is `int32` (`MAXSTUDIOBONES = 128` is only an engine limit, not a rejection criterion) |
| Bones an animation chain can **address** | **256** | `mstudioanim_t.bone` is a `byte` (predicate is `≤255`) |
| Bone palette per **strip** | **127** | VTX `Vertex_t.boneID[]` is a `char` (official `maxBonesPerStrip = 53`) |
| Vertices per **mesh** | **65536** | VTX `Vertex_t.origMeshVertID` is a `uint16` |
| Vertices per **model** | **44,739,242** | `vertexindex` is an int32 byte offset ÷ 48 |
| **Material** table entries | **32768** | `pSkinref[]` is a **signed** `short` |
| **Triangle** count | no limit | VTX `numIndices` is an int32 |
| **LOD** levels | **8** | The engine's `MAX_NUM_LODS` |

Each writer also runs a **self-check** afterwards (VVD / VTX consistency checks, PHY's 13 hard
constraints); a failure aborts as "a bug in this implementation" rather than leaving a broken
file behind.

> ⚠️ **What is constrained is the "index of a referenced bone", not the "total bone count".**
> **Unreferenced bones do not consume VVD index space** — they appear only in the bone table.
> So a model with 134 bones that only references up to index 118 is **legal**.

> ⚠️ **The granularity of "≤ 65536 vertices per mesh" is the mesh (= one material)**, not the
> bodypart: adding a material adds a mesh, and each is counted independently. The predicate is
> `n > 65536` (**not `>=`**) — at exactly 65536 vertices the indices are `0..65535`, which all
> fit.

---

## Automatic splitting of oversized meshes

**Nothing to do by default** — mdlc splits automatically. VTX's `origMeshVertID` is a `uint16`,
so **one mesh (= one material) holds at most 65536 vertices**. Official `studiomdl` rejects it
outright (`ERROR: too many indices in source`); mdlc splits an oversized mesh **into several
meshes in triangle order** at compile time:

| | |
|---|---|
| Switch | `[model].split_oversized_meshes`, **default `true`** |
| Granularity | each piece ≤ 65536 vertices |
| Placed where | **inside the same model** (**no new bodypart**) |
| Material | all pieces **share the original material index** (`mesh.material` is just an index into `pSkinref[]`) |
| Rendering | **pixel-identical** to before splitting (just a few more draw calls) |
| Turning it off | `= false` ⟹ back to "error out and suggest an alternative" |

**Why not split into new bodyparts** (which is what NekoMDL's `$maxverts` does): once the
bodypart count changes, the engine's `$bodygroup` selection (**by index**) shifts.

> **A real example**: one material of a mod project had 305,703 vertices — compiling **failed**
> without splitting; after splitting, meshes went from 20 to 24, **the largest single mesh was
> exactly 65,536**, the triangle total was conserved at 232,099, and the bodypart count **stayed
> at 2** (unchanged).

For a model that is not oversized this path **is not touched at all** (early return), so it has
zero effect on existing artifacts.

---

## Multiple LODs

```toml
[[bodyparts.models]]
smd = "lod0.smd"             # LOD 0 (most detailed)

[[bodyparts.models.lods]]
smd = "lod1.smd"
switch_point = 20.0          # optional, default 20·2^k (LOD 1→20, LOD 2→40, LOD 3→80)

[[bodyparts.models.lods]]
smd = "lod2.smd"
switch_point = 40.0
```

Each LOD is a **complete, independent SMD** (not "some triangles deleted"). This implementation
merges them into a single vertex pool with exact cross-LOD deduplication, sorts by LOD
membership, and generates the fixup table — matching studiomdl's `UnifyLODs`.

Three constraints:

1. The **material set must be identical** across LODs — violating this is an **explicit error**
   (both missing and extra materials error; silently aligning them would apply the wrong
   material);
2. At most **8** levels including LOD 0 — violating this errors;
3. The vertex count of each LOD **should** be monotonically non-increasing (higher LOD = coarser)
   — this is a **recommendation**, not a hard check; `numLODVertexes[n]` is computed from each
   piece's length on a cumulative basis, so the written values are always self-consistent.

> **Unimplemented**: **automatic generation** of LODs (mesh simplification / decimation). This
> implementation only accepts explicitly provided multiple-LOD input and **will not simplify the
> mesh for you**. This is the largest gap versus official `studiomdl`.

`[[bodyparts.models.lods]]` also accepts `bone_tree_collapse` / `replace_bone` / `no_facial`
(corresponding to those `$lod` options). `smd` may be omitted — when it is, the LOD 0 mesh is
reused and only the bone options are applied.

---

## Known unimplemented

| Item | Notes |
|---|---|
| **Automatic LOD generation** | Mesh simplification / decimation. Multiple-LOD **input and output** are supported, but it will not simplify for you. **This is the largest gap versus the official tool** |
| **DMX input** | Deliberately unimplemented. `$nekomodel` gives an **explicit error**; `studio "x.dmx"` is not specially recognised and fails while reading the file / parsing it as SMD (the official tool also delegates to `dmxconvert.exe`) |
| `$maxverts` | A non-official NekoMDL extension; ignored. Use automatic splitting instead |
| `ikrule footstep` (type 3) | Needs the `center` of `$ikchain`, which mdlc does not model yet. Writing it is an **explicit error** rather than silently producing a wrong payload |
| A number of rare QC commands | `$renamebone` / `$hierarchy` / `$insertbone` / `$collapsebones` / `$screenalign` / `$upaxis` / `$origin` / `$maxbones` … are ignored |
| Official CLI's `-minlod` / `-striplods` / `-definebones` / `-printbones` / `-t` / `-a` | Arguments are accepted but **warned about and ignored** |
| `$bodygroup { … blank }` | The `blank` member (an empty model) is an **error** — `BodyModel.smd` is a required field and mdlc has no way to express "a model with no mesh" |
| **`weightlist` / `numframes` / `subtract` on blend sequences** | **Silently dropped** (see above). They work normally on single-animation sequences |
| `[[sequences.movements]]` can only be written from TOML | The QC front end has no corresponding keyword (the writer is fully implemented; the QC side simply cannot express it) |
| DX8 / DX7 fallback variants | The official tool additionally produces `.dx80.vtx` / `.sw.vtx`; L4D2 is a DX9 engine, so only `.dx90.vtx` is produced for now |

---

## Further documentation

The `docs/` directory holds eight specification reports whose conclusions all come from
byte-level analysis of real artifacts:

| Document | Contents |
|---|---|
| [`docs/animation-layout.md`](docs/animation-layout.md) | Specification of the MDL animation data layout (including errata for retracted conclusions) |
| [`docs/blend-sequences.md`](docs/blend-sequences.md) | The complete specification of blend sequences (QC side + binary side) |
| [`docs/coordinate-systems.md`](docs/coordinate-systems.md) | Coordinate-system conventions and the `$staticprop` geometry rotation |
| [`docs/qc-coverage-gap.md`](docs/qc-coverage-gap.md) | QC coverage against the **137-entry dispatch table** of L4D2's `studiomdl.exe` |
| [`docs/feature-gap.md`](docs/feature-gap.md) | Feature gap list and priorities versus official `studiomdl` |
| [`docs/fbx-support.md`](docs/fbx-support.md) | FBX support semantics and UX plan (design rationale for the nine `src*` commands + deviation table) |
| [`docs/gltf-support.md`](docs/gltf-support.md) | Where glTF / GLB semantics come from, and the trade-offs (zero new syntax) |
| [`docs/update-check.md`](docs/update-check.md) | The update check's design and trade-offs (why it is on by default, when it goes online, how to turn it off) |

> ⚠️ **`docs/feature-gap.md` and `docs/qc-coverage-gap.md` are research reports** carrying a
> snapshot date and **describing the state at the time of writing**; "what is implemented today"
> is whatever this file says.
