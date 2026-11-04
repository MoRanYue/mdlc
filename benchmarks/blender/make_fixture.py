"""Generate one benchmark fixture in Blender (mesh + skeleton + optional morphs + optional animation).

**This file is invoked by `bench.py` through `blender -b --python`**; do not run it by hand.
All arguments are positional and follow `--`:

```
blender -b --python blender/make_fixture.py -- \
        <FORMAT> <SUBDIVISIONS> <SIZE> <MORPHS> <MATERIALS> <SEQ_FRAMES> <OUT_DIR>
```

`FORMAT` is one of `DMX` / `SMD` / `FBX`. The outputs are renamed to fixed names:

| File | Content |
|---|---|
| `<OUT_DIR>/mesh.<ext>` | mesh + skeleton (`ext` is `dmx` / `smd` / `fbx`) |
| `<OUT_DIR>/anims/idle.<ext>` | a short animation (only when `SEQ_FRAMES > 0`) |

The last line prints `RESULT <format> <mesh path> <animation path or ->`, which
`bench.py` parses.

WARNING: three traps that were actually hit; read them before editing this file:

1. **`shape_key_add()` defaults to `value = 1.0`, not 0.** Without resetting it
   the exported base mesh is the "all morph targets at full strength" shape, so
   every face normal differs and vertices can no longer be welded (10,201
   vertices became 58,336 in testing).
2. **A mesh with shape keys can no longer be `modifier_apply`-ed**, so
   triangulation must happen before the shape keys are created.
3. **BlenderSourceTools' `export_path` is a directory, not a file name**; the
   real file name comes from the collection, and the animation is written into
   an `anims/` subdirectory. That is why everything is renamed afterwards.
"""

from __future__ import annotations

import math
import os
import shutil
import sys

import bpy

# -- arguments -------------------------------------------------------------

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
if len(argv) < 7:
    print("RESULT ERROR usage: -- <FORMAT> <SUBDIV> <SIZE> <MORPHS> <MATERIALS> <FRAMES> <OUT>")
    raise SystemExit(2)

FORMAT = argv[0].upper()
SUBDIV = int(argv[1])
SIZE = float(argv[2])
MORPHS = int(argv[3])
MATERIALS = int(argv[4])
SEQ_FRAMES = int(argv[5])
OUT_DIR = os.path.abspath(argv[6])

os.makedirs(OUT_DIR, exist_ok=True)
os.makedirs(os.path.join(OUT_DIR, "anims"), exist_ok=True)


def log(*args: object) -> None:
    print("[fixture]", *args)


# -- clear the scene -------------------------------------------------------
#
# WARNING: do not use `bpy.ops.wm.read_factory_settings()` or `--factory-startup`:
# that unloads the BlenderSourceTools add-on as well, and `bpy.context.scene.vs`
# disappears with it.

for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj, do_unlink=True)
for mesh in list(bpy.data.meshes):
    bpy.data.meshes.remove(mesh)
for arm in list(bpy.data.armatures):
    bpy.data.armatures.remove(arm)
for mat in list(bpy.data.materials):
    bpy.data.materials.remove(mat)

# -- skeleton --------------------------------------------------------------


bpy.ops.object.armature_add(enter_editmode=True, location=(0, 0, 0))
armature = bpy.context.active_object
armature.name = "rig"
armature.data.name = "rig"
edit_bones = armature.data.edit_bones
root = edit_bones[0]
root.name = "root"
root.head = (0.0, 0.0, 0.0)
root.tail = (0.0, 0.0, 10.0)
root.roll = math.radians(15.0)
bpy.ops.object.mode_set(mode='OBJECT')

# -- mesh ------------------------------------------------------------------


bpy.ops.mesh.primitive_grid_add(
    x_subdivisions=SUBDIV,
    y_subdivisions=SUBDIV,
    size=SIZE,
    location=(0.0, 0.0, 0.0),
)
body = bpy.context.active_object
body.name = "body"
mesh = body.data
mesh.name = "body_mesh"
me = mesh

log(f"mesh {len(me.vertices):,} vertices / {len(me.polygons):,} faces")

# UV: **reuse layer 0**. `uv_layers.new()` adds a layer, while the exporter uses
# the active render layer.
uv_layer = me.uv_layers[0] if me.uv_layers else me.uv_layers.new(name="UVMap")
uv_layer.name = "UVMap"
uv_layer.active_render = True
me.uv_layers.active = uv_layer
for poly in me.polygons:
    for loop_index in poly.loop_indices:
        vert = me.loops[loop_index].vertex_index
        co = me.vertices[vert].co
        uv_layer.data[loop_index].uv = ((co.x + SIZE / 2) / SIZE, (co.y + SIZE / 2) / SIZE)

# Materials: hand the faces to mat_0..mat_N in round-robin order, so the
# multi-material path gets exercised too.
for i in range(max(1, MATERIALS)):
    mat = bpy.data.materials.new(name=f"mat_{i}")
    mat.use_nodes = True
    me.materials.append(mat)
if MATERIALS > 1:
    for poly in me.polygons:
        poly.material_index = poly.index % MATERIALS

# Triangulation: **must happen before the shape keys** (a mesh with shape keys
# cannot apply modifiers).
tris = body.modifiers.new(name="Tri", type='TRIANGULATE')
bpy.context.view_layer.objects.active = body
bpy.ops.object.modifier_apply(modifier=tris.name)
log(f"{len(me.polygons):,} triangles after triangulation")

# -- skinning --------------------------------------------------------------

group = body.vertex_groups.new(name="root")
group.add([v.index for v in me.vertices], 1.0, 'REPLACE')

mod = body.modifiers.new(name="Armature", type='ARMATURE')
mod.object = armature
body.parent = armature

# -- morph targets (optional) ----------------------------------------------


if MORPHS:
    body.shape_key_add(name="Basis", from_mix=False)
    for k in range(MORPHS):
        key = body.shape_key_add(name=f"morph_{k:02d}", from_mix=False)
        fx = (k + 1) * 0.55
        fy = (k + 1) * 0.31
        amp = 1.0 + 0.05 * k
        for i, pt in enumerate(key.data):
            co = me.vertices[i].co
            pt.co.z += amp * math.sin(fx * co.x) * math.cos(fy * co.y)
    # WARNING: trap 1 in the module docstring -- the default value is 1.0.
    for kb in body.data.shape_keys.key_blocks:
        kb.value = 0.0
    log(f"{MORPHS} morph targets (values reset to zero)")

# -- animation (optional) --------------------------------------------------


if SEQ_FRAMES > 0:
    bpy.context.view_layer.objects.active = armature
    bpy.ops.object.mode_set(mode='POSE')
    pose_bone = armature.pose.bones["root"]
    scene = bpy.context.scene
    scene.frame_start = 1
    scene.frame_end = SEQ_FRAMES
    scene.render.fps = 30
    for frame, degrees in ((1, 0.0), (SEQ_FRAMES, 25.0)):
        scene.frame_set(frame)
        pose_bone.rotation_mode = 'XYZ'
        pose_bone.rotation_euler = (math.radians(degrees), 0.0, 0.0)
        pose_bone.keyframe_insert(data_path="rotation_euler", frame=frame)
    bpy.ops.object.mode_set(mode='OBJECT')
    scene.frame_set(1)
    log(f"{SEQ_FRAMES}-frame animation")

# -- export ----------------------------------------------------------------


def restamp(produced: str, target: str) -> str:
    """BlenderSourceTools names files after collections / objects; normalize them."""
    if not os.path.isfile(produced):
        return "-"
    os.makedirs(os.path.dirname(target) or ".", exist_ok=True)
    shutil.copyfile(produced, target)
    return target


mesh_target = os.path.join(OUT_DIR, f"mesh.{FORMAT.lower()}")
anim_target = os.path.join(OUT_DIR, "anims", f"idle.{FORMAT.lower()}")

bpy.ops.object.select_all(action='DESELECT')
body.select_set(True)
if SEQ_FRAMES > 0:
    armature.select_set(True)
bpy.context.view_layer.objects.active = body

if FORMAT == "FBX":
    bpy.ops.export_scene.fbx(
        filepath=os.path.join(OUT_DIR, "fixture.fbx"),
        use_selection=True,
        object_types={'ARMATURE', 'MESH'},
        use_mesh_modifiers=False,
        mesh_smooth_type='FACE',
        bake_anim=SEQ_FRAMES > 0,
        add_leaf_bones=False,
        axis_forward='-Z',
        axis_up='Y',
    )
    # WARNING: Blender 5.2 takes `filepath` literally -- passing `fixture.fbx`
    # produces `fixture.fbx`, passing `fixture` produces a file with no extension.
    # Accept both.
    produced_mesh = os.path.join(OUT_DIR, "fixture.fbx")
    if not os.path.isfile(produced_mesh):
        produced_mesh = os.path.join(OUT_DIR, "fixture")
    mesh_path = restamp(produced_mesh, mesh_target)
    # FBX keeps the animation inside the same file, so there is no separate anims/.
    anim_path = mesh_path if SEQ_FRAMES > 0 else "-"
else:
    vs = bpy.context.scene.vs
    vs.export_path = OUT_DIR
    vs.export_format = FORMAT
    vs.use_kv2 = False
    vs.qc_compile = False
    vs.dmx_encoding = '4'
    vs.dmx_format = '15'
    vs.export_list.clear()
    for obj in ([body, armature] if SEQ_FRAMES > 0 else [body]):
        item = vs.export_list.add()
        item.obj = obj
    from io_scene_valvesource import export_smd
    export_smd.State.update_scene()
    bpy.ops.export_scene.smd(export_scene=True)

    ext = FORMAT.lower()
    mesh_path = restamp(os.path.join(OUT_DIR, f"Collection.{ext}"), mesh_target)
    anim_path = restamp(
        os.path.join(OUT_DIR, "anims", f"rig.{ext}"), anim_target
    )
    if anim_path == "-" and SEQ_FRAMES > 0:
        # The object is not always named "rig"; scan anims/ as a fallback.
        anims_dir = os.path.join(OUT_DIR, "anims")
        if os.path.isdir(anims_dir):
            for entry in sorted(os.listdir(anims_dir)):
                if entry.lower().endswith("." + ext):
                    anim_path = restamp(
                        os.path.join(anims_dir, entry), anim_target
                    )
                    break

if mesh_path == "-":
    print(f"RESULT ERROR export failed ({FORMAT})")
    raise SystemExit(1)

print(f"RESULT {FORMAT} {mesh_path} {anim_path}")
