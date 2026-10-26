"""Skyfold Studio -> Unreal Engine 5.4+ importer.

Run it inside the Unreal Editor with the "Python Editor Script Plugin" enabled:
    Tools > Execute Python Script... > import_world.py
or in the Output Log (Cmd mode "Python"):  py "<this folder>/import_world.py"

It imports the GLB models (Interchange), places every tree, plant, rock and prop as hierarchical
instanced static meshes, builds the sky (SkyAtmosphere, sun at the world's time of day, SkyLight,
height fog, clouds, post process), the ocean / lake / river surfaces with a water material, the
roads and the gameplay markers, creates the landscape material, and writes the heightmap and
weightmaps oriented for Landscape import. Unreal's Python API can't create a landscape from a
heightmap, so that step is manual: the script prints the exact values (also in README.md).

Every step is defensive: it logs what it did and carries on when an API is missing.
"""

import json
import math
import os
import struct
import zlib
from array import array

import unreal

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = json.load(open(os.path.join(HERE, "Data", "worldbuilder_unreal.json")))
DEST = "/Game/WorldBuilder/" + DATA["safe_name"]


def log(msg):
    unreal.log("WorldBuilder: " + msg)


def warn(msg):
    unreal.log_warning("WorldBuilder: " + msg)


def actors():
    try:
        return unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
    except Exception:
        return None


def editor_world():
    try:
        return unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world()
    except Exception:
        return unreal.EditorLevelLibrary.get_editor_world()


def spawn(cls, location=None, rotation=None, label=None):
    location = location or unreal.Vector(0.0, 0.0, 0.0)
    rotation = rotation or unreal.Rotator(0.0, 0.0, 0.0)
    sub = actors()
    a = sub.spawn_actor_from_class(cls, location, rotation) if sub else unreal.EditorLevelLibrary.spawn_actor_from_class(cls, location, rotation)
    if a is not None and label:
        a.set_actor_label(label)
    return a


# --------------------------------------------------------------------------- importing


def import_files(files, dest, options=None):
    tasks = []
    for f in files:
        t = unreal.AssetImportTask()
        t.set_editor_property("filename", f)
        t.set_editor_property("destination_path", dest)
        t.set_editor_property("automated", True)
        t.set_editor_property("save", True)
        t.set_editor_property("replace_existing", True)
        if options is not None:
            t.set_editor_property("options", options)
        tasks.append(t)
    if tasks:
        unreal.AssetToolsHelpers.get_asset_tools().import_asset_tasks(tasks)
    return tasks


def assets_in(path, cls):
    out = {}
    if not unreal.EditorAssetLibrary.does_directory_exist(path):
        return out
    for p in unreal.EditorAssetLibrary.list_assets(path, recursive=True, include_folder=False):
        a = unreal.EditorAssetLibrary.load_asset(p)
        if isinstance(a, cls):
            out[a.get_name()] = a
    return out


def find_mesh(meshes, node):
    """The static mesh imported from glTF node / mesh `node` (Interchange may add an SM_ prefix)."""
    for key in (node, "SM_" + node):
        if key in meshes:
            return meshes[key]
    for name, m in meshes.items():
        if name.endswith(node):
            return m
    return None


# --------------------------------------------------------------------------- axes

# Skyfold Studio is metres, Y up, +X east, +Z south, right-handed (glTF). How the glTF importer
# maps that into Unreal is measured with a probe mesh whose corners sit at (1,0,0), (0,2,0) and
# (0,0,3) metres: M[a] = (world axis, sign) for Unreal axis a, in centimetres per metre.
AXES = [(0, 1.0), (2, 1.0), (1, 1.0)]


def measure_axes():
    global AXES
    dest = DEST + "/Probe"
    import_files([os.path.join(HERE, "Data", "axis_probe.glb")], dest)
    meshes = assets_in(dest, unreal.StaticMesh)
    if not meshes:
        warn("axis probe didn't import; assuming Unreal X = east, Y = south, Z = up")
        return
    box = list(meshes.values())[0].get_bounding_box()
    lo, hi = box.min, box.max
    found = []
    for a in range(3):
        mn = [lo.x, lo.y, lo.z][a]
        mx = [hi.x, hi.y, hi.z][a]
        ext = mx - mn
        axis = min(range(3), key=lambda g: abs(ext - 100.0 * (g + 1)))
        found.append((axis, 1.0 if abs(mx) >= abs(mn) else -1.0))
    if sorted(a for a, _ in found) == [0, 1, 2]:
        AXES = found
        log("glTF import axes: Unreal X/Y/Z = %s" % ", ".join(("+" if s > 0 else "-") + "xyz"[g] for g, s in AXES))
    else:
        warn("couldn't read the probe axes (%s); using the default" % found)


def to_ue(p):
    """World metres -> Unreal centimetres."""
    return unreal.Vector(*[AXES[a][1] * p[AXES[a][0]] * 100.0 for a in range(3)])


def quat_matrix(q):
    x, y, z, w = q
    return [
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ]


def to_ue_rotator(q):
    """World rotation -> Unreal rotator: R_ue = M R M^T with M the signed axis permutation."""
    r = quat_matrix(q)
    m = [[0.0] * 3 for _ in range(3)]
    for a in range(3):
        m[a][AXES[a][0]] = AXES[a][1]
    mr = [[sum(m[i][k] * r[k][j] for k in range(3)) for j in range(3)] for i in range(3)]
    rue = [[sum(mr[i][k] * m[j][k] for k in range(3)) for j in range(3)] for i in range(3)]
    x_axis = unreal.Vector(rue[0][0], rue[1][0], rue[2][0])
    z_axis = unreal.Vector(rue[0][2], rue[1][2], rue[2][2])
    return unreal.MathLibrary.make_rot_from_xz(x_axis, z_axis)


# --------------------------------------------------------------------------- landscape files


def write_png(path, width, height, rows, bit_depth):
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + r for r in rows)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, bit_depth, 0, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def landscape_pixel_map(n):
    """For Unreal landscape pixel (column c, row r): the world grid index (i, j)."""
    gx, sx = AXES[0]
    gy, sy = AXES[1]

    def index(c, r):
        u = c if sx > 0 else n - 1 - c
        v = r if sy > 0 else n - 1 - r
        return (u, v) if gx == 0 else (v, u)

    return index


def write_landscape_files():
    t = DATA["landscape"]
    n = t["resolution"]
    folder = os.path.join(HERE, "Landscape")
    index = landscape_pixel_map(n)
    if AXES[2][0] != 1 or AXES[2][1] < 0 or AXES[0][0] == 1 or AXES[1][0] == 1:
        warn("unexpected up axis from the glTF probe; the landscape files assume Z up")
    h = array("H")
    with open(os.path.join(folder, "heightmap.r16"), "rb") as f:
        h.frombytes(f.read())
    rows = []
    for r in range(n):
        row = array("H", (h[j * n + i] for i, j in (index(c, r) for c in range(n))))
        row.byteswap()  # PNG is big-endian
        rows.append(row.tobytes())
    write_png(os.path.join(folder, "heightmap_ue.png"), n, n, rows, 16)
    # the same, raw little-endian, for the Skyfold Studio plugin's CreateLandscape
    with open(os.path.join(folder, "heightmap_ue.r16"), "wb") as f:
        for row in rows:
            le = array("H")
            le.frombytes(row)
            le.byteswap()
            f.write(le.tobytes())
    os.makedirs(os.path.join(folder, "weights_ue"), exist_ok=True)
    for layer in t["layers"]:
        with open(os.path.join(folder, layer["weights_raw"]), "rb") as f:
            w = f.read()
        rows = [bytes(w[j * n + i] for i, j in (index(c, r) for c in range(n))) for r in range(n)]
        write_png(os.path.join(folder, "weights_ue", layer["key"] + ".png"), n, n, rows, 8)
        with open(os.path.join(folder, "weights_ue", layer["key"] + ".r8"), "wb") as f:
            f.write(b"".join(rows))
    write_hole_file(n, index)
    log("wrote Landscape/heightmap_ue.png and %d weightmaps oriented for this editor" % len(t["layers"]))
    tiles = t.get("tiles_per_side", 1)
    if tiles > 1 and AXES[:2] != [(0, 1.0), (2, 1.0)]:
        # the exported World Partition tiles assume X east, Y south: rewrite them for these axes
        q = t["tile_resolution"] - 1
        out = os.path.join(folder, "Tiles")
        os.makedirs(out, exist_ok=True)
        full = [array("H", (h[j * n + i] for i, j in (index(c, r) for c in range(n)))) for r in range(n)]
        for ty in range(tiles):
            for tx in range(tiles):
                rows = []
                for r in range(ty * q, ty * q + q + 1):
                    row = array("H", full[r][tx * q : tx * q + q + 1])
                    row.byteswap()
                    rows.append(row.tobytes())
                write_png(os.path.join(out, "heightmap_x%d_y%d.png" % (tx, ty)), q + 1, q + 1, rows, 16)
        warn("rewrote the heightmap tiles for the measured axes (weightmap tiles: paint layers from weights_ue/*.png)")


def write_hole_file(n, index):
    """The cave mouths for the landscape's visibility layer (`Landscape/holes_ue.r8`, one byte per
    vertex, 255 = hole): a landscape quad is a hole when its centre lies in a cave cell of the
    editor's terrain grid; Unreal hides a quad by its first (lowest X / Y) vertex."""
    path = os.path.join(HERE, "Data", "caves.json")
    out = os.path.join(HERE, "Landscape", "holes_ue.r8")
    if os.path.exists(out):
        os.remove(out)
    if not os.path.exists(path):
        return False
    caves = json.load(open(path))
    cells = set(tuple(h) for h in caves.get("holes", []))
    grid = caves.get("holes_grid", 0)
    if not cells or not grid:
        return False
    # landscape quad (c, r) -> world grid quad (i, j): corners map through the axis permutation
    vis = bytearray(n * n)
    k = grid / float(n - 1)
    for r in range(n - 1):
        for c in range(n - 1):
            a = index(c, r)
            b = index(c + 1, r + 1)
            ci = int((a[0] + b[0]) * 0.5 * k)
            cj = int((a[1] + b[1]) * 0.5 * k)
            if (ci, cj) in cells:
                vis[r * n + c] = 255
    with open(out, "wb") as f:
        f.write(bytes(vis))
    return True


def create_landscape(material):
    """The Landscape itself, with the Skyfold Studio plugin (Plugins/SkyfoldStudio, compiled
    with the project). Without it the landscape stays a manual import (the table printed at
    the end). Returns the landscape or None."""
    lib = getattr(unreal, "SkyfoldStudioLibrary", None)
    if lib is None:
        warn("the Skyfold Studio plugin isn't compiled in this project: import the landscape by hand (below)")
        return None
    if DATA["landscape"].get("tiles_per_side", 1) > 1:
        warn("World Partition tiles: import the landscape by hand (below)")
        return None
    label = "WB Landscape"
    sub = actors()
    for a in (sub.get_all_level_actors() if sub else []):
        if isinstance(a, unreal.LandscapeProxy) and a.get_actor_label() == label:
            sub.destroy_actor(a)
    t = DATA["landscape"]
    v = landscape_values()
    folder = os.path.join(HERE, "Landscape")
    names = [l["name"] for l in t["layers"]]
    files = [os.path.join(folder, "weights_ue", l["key"] + ".r8") for l in t["layers"]]
    holes = os.path.join(folder, "holes_ue.r8")
    try:
        land = lib.create_landscape(
            os.path.join(folder, "heightmap_ue.r16"), v["resolution"],
            unreal.Vector(*v["location"]), unreal.Vector(*v["scale"]), material,
            names, files, DEST + "/Landscape", holes if os.path.exists(holes) else "", label)
    except Exception as e:
        warn("landscape: %s" % e)
        return None
    if land is None:
        warn("the landscape couldn't be created (see LogSkyfoldStudio)")
        return None
    log("landscape %d x %d with %d layers%s" % (v["resolution"], v["resolution"], len(names), ", cave mouths open" if os.path.exists(holes) else ""))
    return land


def landscape_values():
    t = DATA["landscape"]
    n = t["resolution"]
    half = t["size_m"] * 0.5
    cell_cm = t["size_m"] / (n - 1) * 100.0
    z_scale = 12800.0 * (t["height_max_m"] - t["height_min_m"]) / 65535.0
    z_loc = 100.0 * t["height_min_m"] + 32768.0 * 100.0 * (t["height_max_m"] - t["height_min_m"]) / 65535.0
    return {
        "location": (-half * 100.0, -half * 100.0, z_loc),
        "scale": (cell_cm, cell_cm, z_scale),
        "resolution": n,
        "components": (n - 1) // t["quads_per_component"],
        "tiles": t.get("tiles_per_side", 1),
    }


# --------------------------------------------------------------------------- materials

MEL = unreal.MaterialEditingLibrary


def new_asset(name, path, cls, factory):
    full = path + "/" + name
    if unreal.EditorAssetLibrary.does_asset_exist(full):
        unreal.EditorAssetLibrary.delete_asset(full)
    return unreal.AssetToolsHelpers.get_asset_tools().create_asset(name, path, cls, factory)


def import_textures(layers):
    """Ground layer textures: albedo sRGB, normal (OpenGL -> flip green), mask linear (R metallic,
    G ambient occlusion, B height, A smoothness)."""
    dest = DEST + "/Textures"
    files = []
    for l in layers:
        files += [os.path.join(HERE, l["albedo"]), os.path.join(HERE, l["normal"])]
        if l.get("mask"):
            files.append(os.path.join(HERE, l["mask"]))
    if DATA["water"].get("normal_map"):
        files.append(os.path.join(HERE, DATA["water"]["normal_map"]))
    import_files(files, dest)
    tex = assets_in(dest, unreal.Texture2D)
    for name, t in tex.items():
        if "normal" in name.lower():
            t.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_NORMALMAP)
            t.set_editor_property("srgb", False)
            t.set_editor_property("flip_green_channel", True)
            unreal.EditorAssetLibrary.save_loaded_asset(t)
        elif name.lower().endswith("_mask"):
            t.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_MASKS)
            t.set_editor_property("srgb", False)
            unreal.EditorAssetLibrary.save_loaded_asset(t)
    return tex


GRASS_UV_HLSL = r"""
// a world position -> the grass map's texture coordinates (map vertex (0, 0) at Origin, one
// vertex step along I and J, Steps steps across)
float3 d = P - Origin;
return float2(dot(d, I) / dot(I, I), dot(d, J) / dot(J, J)) / Steps;
"""
# landscape grass tufts per 10 m x 10 m at full density (the editor's blade field, measured)
GRASS_DENSITY = 900.0


def build_grass():
    """Skyfold Studio's meadow grass as Unreal landscape grass: one tuft mesh (the editor's blade
    recipe), a two-sided material from the world's blade colours with the wind, and two grass
    types (lush and dry) the landscape material scatters by the editor's grass map."""
    g = DATA.get("grass")
    if not g:
        return None
    try:
        import_files([os.path.join(HERE, g["clump"])], DEST + "/Grass")
        mesh = next(iter(assets_in(DEST + "/Grass", unreal.StaticMesh).values()), None)
        if mesh is None:
            warn("grass tuft didn't import")
            return None
        try:
            ns = mesh.get_editor_property("nanite_settings")
            ns.set_editor_property("enabled", False)
            mesh.set_editor_property("nanite_settings", ns)
        except Exception:
            pass
        unreal.EditorAssetLibrary.save_loaded_asset(mesh)
        mat = new_asset("M_WB_Grass", DEST + "/Materials", unreal.Material, unreal.MaterialFactoryNew())
        mat.set_editor_property("two_sided", True)
        mat.set_editor_property("used_with_instanced_static_meshes", True)
        base = MEL.create_material_expression(mat, unreal.MaterialExpressionVectorParameter, -700, 0)
        base.set_editor_property("parameter_name", "Base")
        base.set_editor_property("default_value", unreal.LinearColor(*g["base"], 1.0))
        tip = MEL.create_material_expression(mat, unreal.MaterialExpressionVectorParameter, -700, 120)
        tip.set_editor_property("parameter_name", "Tip")
        tip.set_editor_property("default_value", unreal.LinearColor(*g["tip"], 1.0))
        uv = MEL.create_material_expression(mat, unreal.MaterialExpressionTextureCoordinate, -900, 240)
        v = MEL.create_material_expression(mat, unreal.MaterialExpressionComponentMask, -760, 240)
        v.set_editor_property("g", True)
        MEL.connect_material_expressions(uv, "", v, "")
        # (dark at the root, the tip's colour from half way up)
        pw = MEL.create_material_expression(mat, unreal.MaterialExpressionPower, -640, 240)
        pw.set_editor_property("const_exponent", 0.7)
        MEL.connect_material_expressions(v, "", pw, "Base")
        col = MEL.create_material_expression(mat, unreal.MaterialExpressionLinearInterpolate, -480, 60)
        MEL.connect_material_expressions(tip, "", col, "A")
        MEL.connect_material_expressions(base, "", col, "B")
        MEL.connect_material_expressions(pw, "", col, "Alpha")
        MEL.connect_material_property(col, "", unreal.MaterialProperty.MP_BASE_COLOR)
        for name, value, prop in (("Roughness", 0.85, unreal.MaterialProperty.MP_ROUGHNESS), ("Specular", 0.25, unreal.MaterialProperty.MP_SPECULAR)):
            c = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant, -480, 300 if name == "Roughness" else 360)
            c.set_editor_property("r", value)
            MEL.connect_material_property(c, "", prop)
        # the wind, as the plants' (the blades flutter)
        custom = MEL.create_material_expression(mat, unreal.MaterialExpressionCustom, -480, 500)
        custom.set_editor_property("code", WIND_HLSL)
        custom.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT3)
        ins = []
        for n in ("Local", "Bounds", "Obj", "Time", "Wind", "Flutter"):
            ci = unreal.CustomInput()
            ci.set_editor_property("input_name", n)
            ins.append(ci)
        custom.set_editor_property("inputs", ins)
        MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionLocalPosition, -800, 420), "", custom, "Local")
        bounds = MEL.create_material_expression(mat, unreal.MaterialExpressionObjectLocalBounds, -800, 480)
        MEL.connect_material_expressions(bounds, "Max", custom, "Bounds")
        MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionObjectPositionWS, -800, 540), "", custom, "Obj")
        MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionTime, -800, 600), "", custom, "Time")
        wind = MEL.create_material_expression(mat, unreal.MaterialExpressionCollectionParameter, -950, 660)
        wind.set_editor_property("collection", world_parameters())
        wind.set_editor_property("parameter_name", "Wind")
        # (a collection vector reads as one float4)
        MEL.connect_material_expressions(wind, "", custom, "Wind")
        one = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant, -800, 720)
        one.set_editor_property("r", 1.0)
        MEL.connect_material_expressions(one, "", custom, "Flutter")
        MEL.connect_material_property(custom, "", unreal.MaterialProperty.MP_WORLD_POSITION_OFFSET)
        MEL.recompile_material(mat)
        unreal.EditorAssetLibrary.save_loaded_asset(mat)
        types = {}
        reach = g.get("reach_m", 75.0) * 100.0
        for key, tip_c in (("Lush", g["tip"]), ("Dry", g["dry"])):
            mi = new_asset("MI_WB_Grass_" + key, DEST + "/Materials", unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew())
            mi.set_editor_property("parent", mat)
            MEL.set_material_instance_vector_parameter_value(mi, "Tip", unreal.LinearColor(*tip_c, 1.0))
            unreal.EditorAssetLibrary.save_loaded_asset(mi)
            gt = new_asset("GT_WB_" + key, DEST + "/Grass", unreal.LandscapeGrassType, unreal.LandscapeGrassTypeFactory())
            var = unreal.GrassVariety()
            var.set_editor_property("grass_mesh", mesh)
            var.set_editor_property("override_materials", [mi])
            var.set_editor_property("grass_density", unreal.PerPlatformFloat(default=GRASS_DENSITY))
            var.set_editor_property("start_cull_distance", unreal.PerPlatformInt(default=int(reach * 0.7)))
            var.set_editor_property("end_cull_distance", unreal.PerPlatformInt(default=int(reach)))
            var.set_editor_property("random_rotation", True)
            var.set_editor_property("align_to_surface", True)
            var.set_editor_property("scale_x", unreal.FloatInterval(min=0.8, max=1.25))
            var.set_editor_property("cast_dynamic_shadow", False)
            gt.set_editor_property("grass_varieties", [var])
            unreal.EditorAssetLibrary.save_loaded_asset(gt)
            types[key] = gt
        map_files = [os.path.join(HERE, g["map"])]
        import_files(map_files, DEST + "/Grass")
        gmap = assets_in(DEST + "/Grass", unreal.Texture2D).get("grass")
        if gmap is not None:
            gmap.set_editor_property("srgb", False)
            gmap.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_VECTOR_DISPLACEMENTMAP)
            unreal.EditorAssetLibrary.save_loaded_asset(gmap)
        log("meadow grass: lush and dry landscape grass from the grass map")
        return {"types": types, "map": gmap, "vertices": g["map_vertices"]}
    except Exception as e:
        warn("grass: %s" % e)
        return None


def grass_output(mat, grass):
    """The landscape material's grass: lush where the grass map is dense and green, dry where
    it is dense and dry (R density, G dryness)."""
    if not grass or grass.get("map") is None:
        return
    n = grass["vertices"]
    half = DATA["landscape"]["size_m"] * 0.5
    step = DATA["landscape"]["size_m"] / (n - 1)
    uvc = MEL.create_material_expression(mat, unreal.MaterialExpressionCustom, -700, 1800)
    uvc.set_editor_property("code", GRASS_UV_HLSL)
    uvc.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT2)
    ins = []
    for nm in ("P", "Origin", "I", "J", "Steps"):
        ci = unreal.CustomInput()
        ci.set_editor_property("input_name", nm)
        ins.append(ci)
    uvc.set_editor_property("inputs", ins)
    wp = MEL.create_material_expression(mat, unreal.MaterialExpressionWorldPosition, -1000, 1700)
    MEL.connect_material_expressions(wp, "", uvc, "P")
    for k, (nm, v) in enumerate((("Origin", to_ue([-half, 0.0, -half])), ("I", to_ue([step, 0.0, 0.0])), ("J", to_ue([0.0, 0.0, step])))):
        c = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant3Vector, -1000, 1760 + k * 60)
        c.set_editor_property("constant", unreal.LinearColor(v.x, v.y, v.z, 1.0))
        MEL.connect_material_expressions(c, "", uvc, nm)
    steps = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant, -1000, 1940)
    steps.set_editor_property("r", float(n))
    MEL.connect_material_expressions(steps, "", uvc, "Steps")
    smp = MEL.create_material_expression(mat, unreal.MaterialExpressionTextureSample, -520, 1800)
    smp.set_editor_property("texture", grass["map"])
    smp.set_editor_property("sampler_type", unreal.MaterialSamplerType.SAMPLERTYPE_LINEAR_COLOR)
    MEL.connect_material_expressions(uvc, "", smp, "UVs")
    inv = MEL.create_material_expression(mat, unreal.MaterialExpressionOneMinus, -380, 1880)
    MEL.connect_material_expressions(smp, "G", inv, "")
    lush = MEL.create_material_expression(mat, unreal.MaterialExpressionMultiply, -260, 1800)
    MEL.connect_material_expressions(smp, "R", lush, "A")
    MEL.connect_material_expressions(inv, "", lush, "B")
    dry = MEL.create_material_expression(mat, unreal.MaterialExpressionMultiply, -260, 1900)
    MEL.connect_material_expressions(smp, "R", dry, "A")
    MEL.connect_material_expressions(smp, "G", dry, "B")
    out = MEL.create_material_expression(mat, unreal.MaterialExpressionLandscapeGrassOutput, -100, 1850)
    gi = []
    for key in ("Lush", "Dry"):
        x = unreal.GrassInput()
        x.set_editor_property("name", key)
        x.set_editor_property("grass_type", grass["types"][key])
        gi.append(x)
    out.set_editor_property("grass_types", gi)
    MEL.connect_material_expressions(lush, "", out, "Lush")
    MEL.connect_material_expressions(dry, "", out, "Dry")


def landscape_material(layers, tex, grass=None):
    try:
        mat = new_asset("M_WB_Landscape", DEST + "/Materials", unreal.Material, unreal.MaterialFactoryNew())
        cell = DATA["landscape"]["size_m"] / (DATA["landscape"]["resolution"] - 1)
        color_blend = MEL.create_material_expression(mat, unreal.MaterialExpressionLandscapeLayerBlend, -300, 0)
        normal_blend = MEL.create_material_expression(mat, unreal.MaterialExpressionLandscapeLayerBlend, -300, 400)
        inputs = []
        for l in layers:
            li = unreal.LayerBlendInput()
            li.set_editor_property("layer_name", l["name"])
            li.set_editor_property("blend_type", unreal.LandscapeLayerBlendType.LB_WEIGHT_BLEND)
            inputs.append(li)
        rough_blend = MEL.create_material_expression(mat, unreal.MaterialExpressionLandscapeLayerBlend, -300, 800)
        ao_blend = MEL.create_material_expression(mat, unreal.MaterialExpressionLandscapeLayerBlend, -300, 1200)
        color_blend.set_editor_property("layers", inputs)
        normal_blend.set_editor_property("layers", inputs)
        rough_blend.set_editor_property("layers", inputs)
        ao_blend.set_editor_property("layers", inputs)
        for k, l in enumerate(layers):
            coords = MEL.create_material_expression(mat, unreal.MaterialExpressionLandscapeLayerCoords, -1100, k * 260)
            coords.set_editor_property("mapping_scale", l["tile_m"] / cell)
            for blend, key, y, kind in ((color_blend, l["albedo_name"], 0, unreal.MaterialSamplerType.SAMPLERTYPE_COLOR), (normal_blend, l["normal_name"], 120, unreal.MaterialSamplerType.SAMPLERTYPE_NORMAL)):
                t = tex.get(key)
                if t is None:
                    continue
                s = MEL.create_material_expression(mat, unreal.MaterialExpressionTextureSample, -800, k * 260 + y)
                s.set_editor_property("texture", t)
                s.set_editor_property("sampler_type", kind)
                MEL.connect_material_expressions(coords, "", s, "UVs")
                MEL.connect_material_expressions(s, "RGB", blend, "Layer " + l["name"])
            # the mask map: roughness = 1 - smoothness (A), ambient occlusion (G)
            m = tex.get(l.get("mask_name", ""))
            if m is not None:
                s = MEL.create_material_expression(mat, unreal.MaterialExpressionTextureSample, -800, k * 260 + 240)
                s.set_editor_property("texture", m)
                s.set_editor_property("sampler_type", unreal.MaterialSamplerType.SAMPLERTYPE_MASKS)
                MEL.connect_material_expressions(coords, "", s, "UVs")
                inv = MEL.create_material_expression(mat, unreal.MaterialExpressionOneMinus, -550, k * 260 + 240)
                MEL.connect_material_expressions(s, "A", inv, "")
                MEL.connect_material_expressions(inv, "", rough_blend, "Layer " + l["name"])
                MEL.connect_material_expressions(s, "G", ao_blend, "Layer " + l["name"])
            else:
                c = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant, -550, k * 260 + 240)
                c.set_editor_property("r", 0.85)
                MEL.connect_material_expressions(c, "", rough_blend, "Layer " + l["name"])
                one = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant, -550, k * 260 + 300)
                one.set_editor_property("r", 1.0)
                MEL.connect_material_expressions(one, "", ao_blend, "Layer " + l["name"])
        MEL.connect_material_property(color_blend, "", unreal.MaterialProperty.MP_BASE_COLOR)
        MEL.connect_material_property(normal_blend, "", unreal.MaterialProperty.MP_NORMAL)
        MEL.connect_material_property(rough_blend, "", unreal.MaterialProperty.MP_ROUGHNESS)
        MEL.connect_material_property(ao_blend, "", unreal.MaterialProperty.MP_AMBIENT_OCCLUSION)
        grass_output(mat, grass)
        if os.path.exists(os.path.join(HERE, "Landscape", "holes_ue.r8")):
            # cave mouths: the visibility layer cuts the ground open
            vis = MEL.create_material_expression(mat, unreal.MaterialExpressionLandscapeVisibilityMask, -300, 1600)
            MEL.connect_material_property(vis, "", unreal.MaterialProperty.MP_OPACITY_MASK)
            mat.set_editor_property("blend_mode", unreal.BlendMode.BLEND_MASKED)
        MEL.recompile_material(mat)
        unreal.EditorAssetLibrary.save_loaded_asset(mat)
        log("created landscape material " + mat.get_path_name())
        return mat
    except Exception as e:
        warn("landscape material: %s" % e)
        return None


FOAM_HLSL = r"""
// Skyfold Studio's shore foam: a lace line where the water meets the land and bands rolling in
// over the shallows, torn up by the ripples (Noise). Behind / Here are scene depths without and
// at the water (cm); the vertical depth follows from the view's slant.
// Lakes and rivers foam only at their very edge; the sea also breaks in bands over the shallows
// (Surf 1) as the Godot export's surf does.
float d = max(Behind - Here, 0.0) * max(abs(CamZ), 0.05);
float n = Noise * 0.5 + 0.5;
float lace = (1.0 - smoothstep(2.0, 18.0, d)) * smoothstep(0.35, 0.65, n) * 0.6;
float band = Surf * (1.0 - smoothstep(20.0, 90.0, d)) * smoothstep(0.82, 1.0, sin(Time * 1.3 - d * 0.06 + n * 3.0)) * smoothstep(0.3, 0.7, n) * 0.7;
return saturate((lace + band) * Amount);
"""


def water_material(tex):
    """M_WB_Water: Unreal's Single Layer Water shading with the editor's water optics (its
    extinction per metre and in-scatter albedo per style, `water::water_optics`) as scattering
    and absorption coefficients, two drifting ripple normal maps faded with distance."""
    try:
        mat = new_asset("M_WB_Water", DEST + "/Materials", unreal.Material, unreal.MaterialFactoryNew())
        mat.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_SINGLE_LAYER_WATER)
        mat.set_editor_property("blend_mode", unreal.BlendMode.BLEND_OPAQUE)
        # the base colour and opacity are the foam lying on the water (unconnected, Opacity counts
        # as 1 under Substrate: an opaque top layer)
        white = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant3Vector, -400, -200)
        white.set_editor_property("constant", unreal.LinearColor(0.82, 0.85, 0.87, 1.0))
        MEL.connect_material_property(white, "", unreal.MaterialProperty.MP_BASE_COLOR)
        foam = MEL.create_material_expression(mat, unreal.MaterialExpressionCustom, -400, 120)
        foam.set_editor_property("code", FOAM_HLSL)
        foam.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT1)
        fins = []
        for n in ("Behind", "Here", "CamZ", "Time", "Noise", "Amount", "Surf"):
            ci = unreal.CustomInput()
            ci.set_editor_property("input_name", n)
            fins.append(ci)
        foam.set_editor_property("inputs", fins)
        MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionSceneDepthWithoutWater, -800, 60), "", foam, "Behind")
        MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionPixelDepth, -800, 120), "", foam, "Here")
        cam = MEL.create_material_expression(mat, unreal.MaterialExpressionCameraVectorWS, -950, 180)
        camz = MEL.create_material_expression(mat, unreal.MaterialExpressionComponentMask, -800, 180)
        camz.set_editor_property("b", True)
        MEL.connect_material_expressions(cam, "", camz, "")
        MEL.connect_material_expressions(camz, "", foam, "CamZ")
        MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionTime, -800, 240), "", foam, "Time")
        amount = MEL.create_material_expression(mat, unreal.MaterialExpressionScalarParameter, -800, 360)
        amount.set_editor_property("parameter_name", "ShoreFoam")
        amount.set_editor_property("default_value", 1.0)
        MEL.connect_material_expressions(amount, "", foam, "Amount")
        surf = MEL.create_material_expression(mat, unreal.MaterialExpressionScalarParameter, -800, 420)
        surf.set_editor_property("parameter_name", "Surf")
        surf.set_editor_property("default_value", 0.0)
        MEL.connect_material_expressions(surf, "", foam, "Surf")
        MEL.connect_material_property(foam, "", unreal.MaterialProperty.MP_OPACITY)
        for name, value, prop, y in (("Roughness", 0.04, unreal.MaterialProperty.MP_ROUGHNESS, -100), ("Specular", 0.5, unreal.MaterialProperty.MP_SPECULAR, 0)):
            p = MEL.create_material_expression(mat, unreal.MaterialExpressionScalarParameter, -400, y)
            p.set_editor_property("parameter_name", name)
            p.set_editor_property("default_value", value)
            MEL.connect_material_property(p, "", prop)
        out = MEL.create_material_expression(mat, unreal.MaterialExpressionSingleLayerWaterMaterialOutput, 0, 400)
        for name, value, pin, y in (("Scattering", (0.0, 0.0022, 0.0083), "ScatteringCoefficients", 300), ("Absorption", (0.0045, 0.0007, 0.0002), "AbsorptionCoefficients", 450)):
            p = MEL.create_material_expression(mat, unreal.MaterialExpressionVectorParameter, -400, y)
            p.set_editor_property("parameter_name", name)
            p.set_editor_property("default_value", unreal.LinearColor(value[0], value[1], value[2], 1.0))
            MEL.connect_material_expressions(p, "", out, pin)
        # the bed seen through the water at its own colour; light scattered mostly forwards
        for value, pin, y in ((1.0, "ColorScaleBehindWater", 600), (0.3, "PhaseG", 680)):
            c = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant, -400, y)
            c.set_editor_property("r", value)
            MEL.connect_material_expressions(c, "", out, pin)
        normal = next((t for n, t in tex.items() if n.lower().startswith("water_normal")), None)
        if normal is not None:
            # two ripple layers drifting across each other (world-aligned, 9 m and 23 m), flattened
            # with distance so the far sea doesn't shimmer
            wp = MEL.create_material_expression(mat, unreal.MaterialExpressionWorldPosition, -1400, 700)
            mask = MEL.create_material_expression(mat, unreal.MaterialExpressionComponentMask, -1250, 700)
            mask.set_editor_property("r", True)
            mask.set_editor_property("g", True)
            mask.set_editor_property("b", False)
            MEL.connect_material_expressions(wp, "", mask, "")
            samples = []
            for k, (size, sx, sy) in enumerate(((900.0, 0.004, 0.0025), (2300.0, -0.0015, 0.003))):
                div = MEL.create_material_expression(mat, unreal.MaterialExpressionDivide, -1100, 650 + k * 160)
                div.set_editor_property("const_b", size)
                MEL.connect_material_expressions(mask, "", div, "A")
                pan = MEL.create_material_expression(mat, unreal.MaterialExpressionPanner, -950, 650 + k * 160)
                pan.set_editor_property("speed_x", sx)
                pan.set_editor_property("speed_y", sy)
                MEL.connect_material_expressions(div, "", pan, "Coordinate")
                smp = MEL.create_material_expression(mat, unreal.MaterialExpressionTextureSample, -780, 650 + k * 160)
                smp.set_editor_property("texture", normal)
                smp.set_editor_property("sampler_type", unreal.MaterialSamplerType.SAMPLERTYPE_NORMAL)
                MEL.connect_material_expressions(pan, "", smp, "UVs")
                samples.append(smp)
            add = MEL.create_material_expression(mat, unreal.MaterialExpressionAdd, -640, 700)
            MEL.connect_material_expressions(samples[0], "RGB", add, "A")
            MEL.connect_material_expressions(samples[1], "RGB", add, "B")
            blend = MEL.create_material_expression(mat, unreal.MaterialExpressionNormalize, -540, 700)
            MEL.connect_material_expressions(add, "", blend, "")
            dist = MEL.create_material_expression(mat, unreal.MaterialExpressionPixelDepth, -720, 900)
            far = MEL.create_material_expression(mat, unreal.MaterialExpressionDivide, -560, 900)
            far.set_editor_property("const_b", 60000.0)
            MEL.connect_material_expressions(dist, "", far, "A")
            sat = MEL.create_material_expression(mat, unreal.MaterialExpressionSaturate, -440, 900)
            MEL.connect_material_expressions(far, "", sat, "")
            flat = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant3Vector, -440, 820)
            flat.set_editor_property("constant", unreal.LinearColor(0.0, 0.0, 1.0, 1.0))
            lerp = MEL.create_material_expression(mat, unreal.MaterialExpressionLinearInterpolate, -300, 760)
            MEL.connect_material_expressions(blend, "", lerp, "A")
            MEL.connect_material_expressions(flat, "", lerp, "B")
            MEL.connect_material_expressions(sat, "", lerp, "Alpha")
            MEL.connect_material_property(lerp, "", unreal.MaterialProperty.MP_NORMAL)
            # the foam's breakup: the first ripple layer's slope
            MEL.connect_material_expressions(samples[0], "R", foam, "Noise")
        if normal is None:
            zero = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant, -800, 300)
            MEL.connect_material_expressions(zero, "", foam, "Noise")
        MEL.recompile_material(mat)
        unreal.EditorAssetLibrary.save_loaded_asset(mat)
        return mat
    except Exception as e:
        warn("water material: %s" % e)
        return None


# Unreal's water light model scatters less than the editor's for the same albedo (its in-scatter
# goes through an isotropic phase): the editor's in-scatter albedo is scaled up by this
WATER_ALBEDO_GAIN = 5.0


def water_instance(parent, name, style):
    try:
        mi = new_asset("MI_WB_" + name, DEST + "/Materials", unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew())
        mi.set_editor_property("parent", parent)
        ext = style.get("extinct")
        if ext:
            # per metre -> per centimetre (Unreal's units)
            alb = [min(0.98, a * WATER_ALBEDO_GAIN) for a in style["inscat"]]
            sc = [alb[k] * ext[k] * 0.01 for k in range(3)]
            ab = [max(0.0, ext[k] * 0.01 - sc[k]) for k in range(3)]
            # opaque styles (paint-like: gold, blood, void) see nothing behind them
            t = max(0.02, style.get("transparency", 1.0))
            ab = [a / t for a in ab]
            MEL.set_material_instance_vector_parameter_value(mi, "Scattering", unreal.LinearColor(sc[0], sc[1], sc[2], 1.0))
            MEL.set_material_instance_vector_parameter_value(mi, "Absorption", unreal.LinearColor(ab[0], ab[1], ab[2], 1.0))
        MEL.set_material_instance_scalar_parameter_value(mi, "Roughness", max(0.02, style["roughness"] * 0.5))
        MEL.set_material_instance_scalar_parameter_value(mi, "ShoreFoam", float(style.get("shore_foam", 1.0)))
        MEL.set_material_instance_scalar_parameter_value(mi, "Surf", 1.0 if name == "Ocean" else 0.0)
        unreal.EditorAssetLibrary.save_loaded_asset(mi)
        return mi
    except Exception as e:
        warn("water material instance %s: %s" % (name, e))
        return parent


# --------------------------------------------------------------------------- instances


def ism_actor(label, mesh, transforms, cull_cm, shadows, collide=True, folder=None):
    """An actor with one HierarchicalInstancedStaticMeshComponent holding all `transforms`."""
    actor = spawn(unreal.Actor, label=label)
    if folder:
        try:
            actor.set_folder_path(folder)
        except Exception:
            pass
    try:
        sds = unreal.get_engine_subsystem(unreal.SubobjectDataSubsystem)
        root = sds.k2_gather_subobject_data_for_instance(actor)[0]
        params = unreal.AddNewSubobjectParams(parent_handle=root, new_class=unreal.HierarchicalInstancedStaticMeshComponent, blueprint_context=None)
        handle, reason = sds.add_new_subobject(params)
        lib = unreal.SubobjectDataBlueprintFunctionLibrary
        # (5.8 renamed GetObject to GetAssociatedObject)
        get = getattr(lib, "get_associated_object", None) or lib.get_object
        comp = get(lib.get_data(handle))
        comp.set_static_mesh(mesh)
        # the actor's root too: a static mesh can't hang under a movable root (Unreal detaches it
        # when the level starts)
        if actor.root_component is not None:
            actor.root_component.set_mobility(unreal.ComponentMobility.STATIC)
        comp.set_mobility(unreal.ComponentMobility.STATIC)
        comp.set_cast_shadow(shadows)
        if not collide:
            comp.set_collision_enabled(unreal.CollisionEnabled.NO_COLLISION)
        if cull_cm > 0:
            comp.set_cull_distances(int(cull_cm * 0.9), int(cull_cm))
        comp.add_instances(transforms, False, True)
        return True
    except Exception as e:
        warn("instanced meshes for %s failed (%s); placing up to 2000 single actors" % (label, e))
        actor.destroy_actor()
        for t in transforms[:2000]:
            a = spawn(unreal.StaticMeshActor, t.translation, t.rotation.rotator(), label)
            a.static_mesh_component.set_static_mesh(mesh)
            a.set_actor_scale3d(t.scale3d)
        return False


def mesh_subsystem():
    """The static mesh editing functions: the editor subsystem, or (batch runs, where the
    subsystem isn't there) the older library with the same functions."""
    try:
        sub = unreal.get_editor_subsystem(unreal.StaticMeshEditorSubsystem)
    except Exception:
        sub = None
    return sub if sub is not None else getattr(unreal, "EditorStaticMeshLibrary", None)


PHYSICAL = {}


def physical_material(p):
    """One PhysicalMaterial per footstep surface (friction and restitution from the export)."""
    if not p:
        return None
    key = p["surface"]
    if key in PHYSICAL:
        return PHYSICAL[key]
    pm = None
    try:
        pm = new_asset("PM_WB_" + key, DEST + "/Physics", unreal.PhysicalMaterial, unreal.PhysicalMaterialFactoryNew())
        pm.set_editor_property("friction", p["friction"])
        pm.set_editor_property("restitution", p["bounciness"])
        unreal.EditorAssetLibrary.save_loaded_asset(pm)
    except Exception as e:
        warn("physical material %s: %s" % (key, e))
    PHYSICAL[key] = pm
    return pm


def setup_collision(mesh, m, variant, meshes):
    """Collision from the export's proxies: small props and rocks a 26-sided hull, walk-in models
    their simplified shape (`<variant>_COL`) as complex collision used as simple, trees their trunk
    prism the same way; plants none. Returns what it set."""
    kind = m.get("collider", "none")
    sub = mesh_subsystem()
    try:
        body = mesh.get_editor_property("body_setup")
        if sub is not None:
            sub.remove_collisions(mesh)
        if kind == "none":
            if body is not None:
                body.set_editor_property("collision_trace_flag", unreal.CollisionTraceFlag.CTF_USE_SIMPLE_AS_COMPLEX)
        elif kind == "convex":
            if sub is not None:
                # (5.8 renamed ScriptingCollisionShapeType to ScriptCollisionShapeType)
                shapes = getattr(unreal, "ScriptCollisionShapeType", None) or unreal.ScriptingCollisionShapeType
                sub.add_simple_collisions(mesh, shapes.NDOP26)
        else:
            proxy = find_mesh(meshes, variant["collision"]) if variant.get("collision") else None
            if proxy is not None:
                mesh.set_editor_property("complex_collision_mesh", proxy)
            if body is not None:
                body.set_editor_property("collision_trace_flag", unreal.CollisionTraceFlag.CTF_USE_COMPLEX_AS_SIMPLE)
        phys = physical_material(m.get("physics"))
        if body is not None and phys is not None:
            body.set_editor_property("phys_material", phys)
        unreal.EditorAssetLibrary.save_loaded_asset(mesh)
        return kind
    except Exception as e:
        warn("collision for %s: %s" % (m["safe_name"], e))
        return "default"


def lightmap_uvs(mesh):
    """Buildings and other big static pieces bake lightmaps: make sure the import generated a
    lightmap UV channel (Unreal's default for static meshes, set here in case a project turned
    it off)."""
    sub = mesh_subsystem()
    try:
        if sub is not None:
            sub.set_generate_lightmap_uv(mesh, True)
            unreal.EditorAssetLibrary.save_loaded_asset(mesh)
    except Exception as e:
        warn("lightmap UVs for %s: %s" % (mesh.get_name(), e))


def merge_lods(meshes, variant):
    """Folds the variant's LOD meshes into its LOD0 static mesh (when the API allows)."""
    lod0 = find_mesh(meshes, variant["lods"][0]) if variant["lods"] else None
    if lod0 is None:
        return None
    try:
        sub = mesh_subsystem()
        for k, node in enumerate(variant["lods"][1:], start=1):
            src = find_mesh(meshes, node)
            if src is not None and lod0.get_num_lods() <= k:
                sub.set_lod_from_static_mesh(lod0, k, src, 0, True)
        unreal.EditorAssetLibrary.save_loaded_asset(lod0)
    except Exception as e:
        warn("LODs for %s: %s" % (variant["node"], e))
    return lod0


WIND_HLSL = r"""
// Skyfold Studio's wind on trees and plants: the mesh bends more the higher up it is (its own
// height from its local bounds), a slow sway and gusts push it along the wind, leaves and blades
// flutter; every instance has its own phase. Wind: direction x, y (Unreal), speed m/s, gustiness.
float top = max(Bounds.z, 30.0);
float h = saturate(Local.z / top);
float bend = h * h;
float speed = Wind.z;
float phase = dot(Obj.xy, float2(0.0013, 0.0017));
float sway = sin(Time * (0.8 + speed * 0.06) + phase) * 0.6 + sin(Time * 2.1 + phase * 1.7) * 0.25;
float gust = 0.5 + 0.5 * sin(Time * 0.35 + dot(Obj.xy, float2(0.0004, 0.0003)));
float push = speed * (0.35 + 0.4 * sway + 0.5 * Wind.w * gust);
float3 dir = float3(Wind.x, Wind.y, 0.0);
float3 o = dir * push * bend * top * 0.01;
o += float3(sin(Time * 7.0 + Local.x * 0.05 + phase * 3.0), cos(Time * 6.1 + Local.y * 0.05 + phase), 0.0) * Flutter * speed * 0.2 * h;
return o;
"""

WIND_CATEGORIES = ("Trees", "Plants")
WIND = {}


def world_parameters():
    """MPC_WB_World: what the atmosphere runtime shares with the materials each frame (the wind:
    direction x, y, speed m/s, gustiness; how wet the ground is). Its defaults are the world's."""
    if "mpc" in WIND:
        return WIND["mpc"]
    mpc = None
    try:
        path = DEST + "/Materials/MPC_WB_World"
        mpc = unreal.load_asset(path) if unreal.EditorAssetLibrary.does_asset_exist(path) else unreal.AssetToolsHelpers.get_asset_tools().create_asset("MPC_WB_World", DEST + "/Materials", unreal.MaterialParameterCollection, unreal.MaterialParameterCollectionFactoryNew())
        atm = json.load(open(os.path.join(HERE, "Data", "atmosphere.json"))) if os.path.exists(os.path.join(HERE, "Data", "atmosphere.json")) else {}
        wind = atm.get("world", {}).get("wind", {"dir": [1.0, 0.0], "speed": 0.0, "gustiness": 0.0})
        d = to_ue([wind["dir"][0], 0.0, wind["dir"][1]])
        n = max(math.hypot(d.x, d.y), 1e-6)
        v = unreal.CollectionVectorParameter()
        v.set_editor_property("parameter_name", "Wind")
        v.set_editor_property("default_value", unreal.LinearColor(d.x / n, d.y / n, wind["speed"], wind.get("gustiness", 0.0)))
        w = unreal.CollectionScalarParameter()
        w.set_editor_property("parameter_name", "Wetness")
        w.set_editor_property("default_value", 0.0)
        mpc.set_editor_property("vector_parameters", [v])
        mpc.set_editor_property("scalar_parameters", [w])
        unreal.EditorAssetLibrary.save_loaded_asset(mpc)
    except Exception as e:
        warn("world parameters: %s" % e)
    WIND["mpc"] = mpc
    return mpc


def wind_master():
    """M_WB_Foliage: Interchange's glTF master material with Skyfold Studio's wind as its world
    position offset (the plants' and trees' material instances move under it)."""
    if "master" in WIND:
        return WIND["master"]
    master = None
    try:
        src = "/InterchangeAssets/gltf/M_Default"
        path = DEST + "/Materials/M_WB_Foliage"
        if unreal.EditorAssetLibrary.does_asset_exist(path):
            unreal.EditorAssetLibrary.delete_asset(path)
        master = unreal.EditorAssetLibrary.duplicate_asset(src, path)
        mpc = world_parameters()
        custom = MEL.create_material_expression(master, unreal.MaterialExpressionCustom, -600, 1400)
        custom.set_editor_property("code", WIND_HLSL)
        custom.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT3)
        ins = []
        for n in ("Local", "Bounds", "Obj", "Time", "Wind", "Flutter"):
            ci = unreal.CustomInput()
            ci.set_editor_property("input_name", n)
            ins.append(ci)
        custom.set_editor_property("inputs", ins)
        MEL.connect_material_expressions(MEL.create_material_expression(master, unreal.MaterialExpressionLocalPosition, -1000, 1300), "", custom, "Local")
        bounds = MEL.create_material_expression(master, unreal.MaterialExpressionObjectLocalBounds, -1000, 1360)
        MEL.connect_material_expressions(bounds, "Max", custom, "Bounds")
        MEL.connect_material_expressions(MEL.create_material_expression(master, unreal.MaterialExpressionObjectPositionWS, -1000, 1420), "", custom, "Obj")
        MEL.connect_material_expressions(MEL.create_material_expression(master, unreal.MaterialExpressionTime, -1000, 1480), "", custom, "Time")
        wind = MEL.create_material_expression(master, unreal.MaterialExpressionCollectionParameter, -1150, 1540)
        wind.set_editor_property("collection", mpc)
        wind.set_editor_property("parameter_name", "Wind")
        # (a collection vector reads as one float4)
        MEL.connect_material_expressions(wind, "", custom, "Wind")
        flutter = MEL.create_material_expression(master, unreal.MaterialExpressionScalarParameter, -1000, 1600)
        flutter.set_editor_property("parameter_name", "WindFlutter")
        flutter.set_editor_property("default_value", 0.0)
        MEL.connect_material_expressions(flutter, "", custom, "Flutter")
        MEL.connect_material_property(custom, "", unreal.MaterialProperty.MP_WORLD_POSITION_OFFSET)
        MEL.recompile_material(master)
        unreal.EditorAssetLibrary.save_loaded_asset(master)
    except Exception as e:
        warn("wind material: %s" % e)
        master = None
    WIND["master"] = master
    return master


def add_wind(meshes):
    """Moves the meshes' material instances under the wind master: each Interchange base
    instance (opaque, masked, two-sided...) gets a twin under it, leaves and blades fluttering."""
    master = wind_master()
    if master is None:
        return 0
    n = 0
    for mesh in meshes:
        try:
            for slot in mesh.get_editor_property("static_materials"):
                mi = slot.get_editor_property("material_interface")
                if not isinstance(mi, unreal.MaterialInstanceConstant):
                    continue
                base = mi.get_editor_property("parent")
                if base is None or not base.get_path_name().startswith("/InterchangeAssets/"):
                    continue
                twin = WIND.get(base.get_path_name())
                if twin is None:
                    path = DEST + "/Materials/MI_WB_Wind_" + base.get_name()
                    if unreal.EditorAssetLibrary.does_asset_exist(path):
                        unreal.EditorAssetLibrary.delete_asset(path)
                    twin = unreal.EditorAssetLibrary.duplicate_asset(base.get_path_name(), path)
                    twin.set_editor_property("parent", master)
                    unreal.EditorAssetLibrary.save_loaded_asset(twin)
                    WIND[base.get_path_name()] = twin
                mi.set_editor_property("parent", twin)
                # (masked materials are leaves, petals and blades)
                if "Mask" in base.get_name():
                    MEL.set_material_instance_scalar_parameter_value(mi, "WindFlutter", 1.0)
                unreal.EditorAssetLibrary.save_loaded_asset(mi)
                n += 1
        except Exception as e:
            warn("wind for %s: %s" % (mesh.get_name(), e))
    return n


def weapon_pipeline():
    """Interchange pipeline asset for weapon GLBs (made once from the default assets pipeline):
    every mesh as one skeletal mesh (the glTF's nodes become bones, so the moving parts and the
    sockets — grip_r, grip_l, muzzle, aim, nock, trail_base / trail_tip — are bones to attach to)
    and every glTF clip as an AnimSequence. Returns its path, or None."""
    path = DEST + "/Weapons/WB_WeaponPipeline"
    try:
        if not unreal.EditorAssetLibrary.does_asset_exist(path):
            unreal.EditorAssetLibrary.duplicate_asset("/Interchange/Pipelines/DefaultAssetsPipeline", path)
        p = unreal.EditorAssetLibrary.load_asset(path)
        cm = p.get_editor_property("common_meshes_properties")
        cm.set_editor_property("force_all_mesh_as_type", unreal.InterchangeForceMeshType.IFMT_SKELETAL_MESH)
        mp = p.get_editor_property("mesh_pipeline")
        mp.set_editor_property("import_skeletal_meshes", True)
        mp.set_editor_property("combine_skeletal_meshes_behavior", unreal.InterchangeCombineSkeletalMeshesBehavior.BY_SKELETON)
        ap = p.get_editor_property("animation_pipeline")
        ap.set_editor_property("import_animations", True)
        unreal.EditorAssetLibrary.save_asset(path)
        return path
    except Exception as e:
        warn("weapon import pipeline unavailable (%s): importing with the defaults" % e)
        return None


def import_weapon(file, dest):
    """Imports a weapon GLB through Interchange with the weapon pipeline."""
    path = weapon_pipeline()
    if path is not None:
        try:
            params = unreal.ImportAssetParameters()
            params.set_editor_property("is_automated", True)
            params.set_editor_property("replace_existing", True)
            params.set_editor_property("override_pipelines", [unreal.SoftObjectPath(path)])
            src = unreal.InterchangeManager.create_source_data(file)
            unreal.InterchangeManager.get_interchange_manager_scripted().import_asset(dest, src, params)
            return
        except Exception as e:
            warn("weapon import with its pipeline failed (%s): importing with the defaults" % e)
    import_files([file], dest)


def place_weapons(m, folder):
    """A weapon: a skeletal mesh with its clips, one SkeletalMeshActor per placed copy."""
    dest = DEST + "/Weapons/" + m["safe_name"]
    import_weapon(os.path.join(HERE, m["file"]), dest)
    skels = assets_in(dest, unreal.SkeletalMesh)
    anims = assets_in(dest, unreal.AnimSequence)
    if not skels:
        warn("no skeletal mesh imported from " + m["file"] + "; the weapon is not placed")
        return 0
    mesh = list(skels.values())[0]
    log("%s: skeletal mesh %s, clips %s" % (m["name"], mesh.get_name(), sorted(anims.keys())))
    data = m["instances"]
    n = 0
    for k in range(0, len(data) - 8, 9):
        t = unreal.Transform(to_ue(data[k + 1 : k + 4]), to_ue_rotator(data[k + 4 : k + 8]), unreal.Vector(data[k + 8], data[k + 8], data[k + 8]))
        a = spawn(unreal.SkeletalMeshActor, t.translation, t.rotation.rotator(), "WB %s" % m["safe_name"])
        a.set_actor_scale3d(t.scale3d)
        comp = a.skeletal_mesh_component
        comp.set_skeletal_mesh_asset(mesh) if hasattr(comp, "set_skeletal_mesh_asset") else comp.set_skeletal_mesh(mesh)
        try:
            a.set_folder_path(folder + "/Weapons")
        except Exception:
            pass
        a.tags = [unreal.Name("wb_weapon")]
        n += 1
    return n


def place_models(models=None, folder="WorldBuilder/Instances"):
    total = 0
    for m in DATA["models"] if models is None else models:
        if not m["instances"]:
            continue
        if m.get("weapon"):
            total += place_weapons(m, folder)
            continue
        dest = DEST + "/Models/" + m["safe_name"]
        import_files([os.path.join(HERE, m["file"])], dest)
        meshes = assets_in(dest, unreal.StaticMesh)
        if not meshes:
            warn("no meshes imported from " + m["file"])
            continue
        if m.get("category") in WIND_CATEGORIES:
            add_wind(meshes.values())
        per_variant = {}
        data = m["instances"]
        st = m.get("stretch", [])
        # per-axis factors of non-uniformly scaled instances (model axes), by instance row
        stretch = {int(st[k]): st[k + 1 : k + 4] for k in range(0, len(st) - 3, 4)}
        # one HISM per variant and world cell (`cell_m`): culling, HLODs and World Partition
        # streaming work per cell instead of across the whole map
        cell = DATA.get("cell_m", 0) or 1e9
        half = DATA["landscape"]["size_m"] * 0.5
        for k in range(0, len(data) - 8, 9):
            v = int(data[k])
            f = stretch.get(k // 9, [1.0, 1.0, 1.0])
            s = [data[k + 8] * f[AXES[a][0]] for a in range(3)]
            t = unreal.Transform(to_ue(data[k + 1 : k + 4]), to_ue_rotator(data[k + 4 : k + 8]), unreal.Vector(*s))
            key = (v, int((data[k + 1] + half) // cell), int((data[k + 3] + half) // cell))
            per_variant.setdefault(key, []).append(t)
        meshes_of = {}
        for (v, cx, cz), transforms in sorted(per_variant.items(), key=lambda kv: kv[0]):
            variant = m["variants"][min(v, len(m["variants"]) - 1)]
            if v not in meshes_of:
                if variant["lods"]:
                    # generated trees and scans, and packed models with their levels of detail
                    mesh = merge_lods(meshes, variant)
                else:
                    visible = {k: x for k, x in meshes.items() if not k.endswith("_COL")}
                    mesh = list(visible.values())[0] if visible else None
                    if len(visible) > 1:
                        warn("%s has %d meshes; instancing the first (place the others from %s)" % (m["name"], len(visible), dest))
                if mesh is not None:
                    setup_collision(mesh, m, variant, meshes)
                    if m.get("lightmap"):
                        lightmap_uvs(mesh)
                meshes_of[v] = mesh
            mesh = meshes_of[v]
            if mesh is None:
                continue
            label = variant["node"] or m["safe_name"]
            ism_actor("WB %s %d_%d" % (label, cx, cz), mesh, transforms, m["cull_m"] * 100.0, m["shadows"], m.get("collider", "none") != "none", "%s/%d_%d" % (folder, cx, cz))
            total += len(transforms)
        log("%s: %d instances" % (m["name"], len(data) // 9))
    return total


def place_static(glb, folder, label, materials=None, collide=False, no_collide=("_water_webs",)):
    """Every mesh of a world-space GLB (water, roads, caves, dungeons) as a static mesh actor at
    the origin. `collide`: its triangles block (cave rock, dungeon walls and floors; never meshes
    whose names end in `no_collide`: still pools, cobwebs, liquids, portal sheets).
    Returns {mesh name: actor}."""
    dest = DEST + "/" + folder
    import_files([os.path.join(HERE, glb)], dest)
    out = {}
    for name, mesh in assets_in(dest, unreal.StaticMesh).items():
        a = spawn(unreal.StaticMeshActor, label=label + " " + name)
        a.static_mesh_component.set_static_mesh(mesh)
        out[name] = a
        try:
            if collide and not name.endswith(tuple(no_collide)):
                mesh.get_editor_property("body_setup").set_editor_property("collision_trace_flag", unreal.CollisionTraceFlag.CTF_USE_COMPLEX_AS_SIMPLE)
                unreal.EditorAssetLibrary.save_loaded_asset(mesh)
            else:
                a.static_mesh_component.set_collision_enabled(unreal.CollisionEnabled.NO_COLLISION)
        except Exception as e:
            warn("collision for %s: %s" % (name, e))
        if materials:
            for key, mat in materials.items():
                if name.endswith(key) and mat is not None:
                    a.static_mesh_component.set_material(0, mat)
    return out


def build_nav_bounds():
    """A NavMeshBoundsVolume over the whole world: Unreal builds its navmesh from the collision
    (for big worlds turn on World Partitioned navmesh in the project's navigation settings)."""
    b = DATA.get("navmesh_bounds")
    if not b:
        return False
    a = spawn(unreal.NavMeshBoundsVolume, to_ue(b["center"]), None, "WB NavMesh Bounds")
    if a is None:
        return False
    try:
        origin, extent = a.get_actor_bounds(False)
        size = [b["size"][AXES[k][0]] * 100.0 for k in range(3)]
        if extent.x > 0.1 and extent.y > 0.1 and extent.z > 0.1:
            a.set_actor_scale3d(unreal.Vector(size[0] / (2 * extent.x), size[1] / (2 * extent.y), size[2] / (2 * extent.z)))
        else:
            warn("the NavMeshBoundsVolume has no brush: give it a box of %.0f x %.0f x %.0f cm" % tuple(size))
        a.set_folder_path("WorldBuilder")
        try:
            unreal.NavigationSystemV1.get_navigation_system(editor_world()).on_navigation_bounds_updated(a)
        except Exception:
            pass
        return True
    except Exception as e:
        warn("navmesh bounds: %s" % e)
        return False


def build_cave_lights():
    """The caves' glowing crystals as point lights (lumens for an eye adapted to the dark)."""
    n = 0
    for l in DATA.get("cave_lights", []):
        a = spawn(unreal.PointLight, to_ue(l["position"]), None, "WB Crystal Light")
        if a is None:
            continue
        try:
            c = a.light_component
            c.set_editor_property("intensity_units", unreal.LightUnits.LUMENS)
            c.set_intensity(l["lumens"] * LIGHT_SCALE)
            c.set_light_color(unreal.LinearColor(l["color"][0], l["color"][1], l["color"][2], 1.0))
            c.set_attenuation_radius(l["range"] * 100.0)
            c.set_cast_shadows(False)
            a.set_folder_path("WorldBuilder/Caves")
            n += 1
        except Exception as e:
            warn("crystal light: %s" % e)
    if n:
        log("%d crystal lights" % n)
    return n


def yaw_rotator(yaw):
    """A rotation of `yaw` radians about the world's up axis as an Unreal rotator."""
    return to_ue_rotator([0.0, math.sin(yaw * 0.5), 0.0, math.cos(yaw * 0.5)])


def build_water_volumes():
    """Lakes, river stretches and the ocean as PhysicsVolumes with Water Volume on (characters
    swim in them), each box from the bed up to the surface."""
    n = 0
    for v in DATA["water"].get("volumes", []):
        lava = v["kind"] == "lava"
        a = spawn(unreal.PainCausingVolume if lava else unreal.PhysicsVolume, to_ue(v["center"]), yaw_rotator(v["yaw"]), "WB Water Volume %s: %s" % (v["kind"], v["name"]))
        if a is None:
            continue
        try:
            # (lava hurts instead: a pain-causing volume nobody swims in)
            a.set_editor_property("water_volume", not lava)
            a.set_editor_property("fluid_friction", 0.3)
            # the volume's brush: scale whatever cube it was spawned with to the box
            origin, extent = a.get_actor_bounds(False)
            size = [v["size"][AXES[k][0]] * 100.0 for k in range(3)]
            if extent.x > 0.1 and extent.y > 0.1 and extent.z > 0.1:
                a.set_actor_scale3d(unreal.Vector(size[0] / (2 * extent.x), size[1] / (2 * extent.y), size[2] / (2 * extent.z)))
            else:
                warn("water volume %s has no brush: give it a box (Brush Settings) of %.0f x %.0f x %.0f cm" % (v["name"], size[0], size[1], size[2]))
            a.set_folder_path("WorldBuilder/Water")
            n += 1
        except Exception as e:
            warn("water volume %s: %s" % (v["name"], e))
    return n


# --------------------------------------------------------------------------- sky, markers


def build_sky():
    """The sky, sun, sky light, haze, clouds and look; returns them for the atmosphere runtime."""
    l = DATA["light"]
    spawn(unreal.SkyAtmosphere, label="WB Sky Atmosphere")
    to_sun = to_ue(l["sun_direction"])
    sun = spawn(unreal.DirectionalLight, unreal.Vector(0, 0, 50000), unreal.MathLibrary.make_rot_from_x(unreal.Vector(-to_sun.x, -to_sun.y, -to_sun.z)), "WB Sun")
    try:
        c = sun.light_component
        c.set_intensity(l["intensity_lux"])
        c.set_light_color(unreal.LinearColor(l["sun_color"][0], l["sun_color"][1], l["sun_color"][2], 1.0))
        c.set_editor_property("atmosphere_sun_light", True)
        c.set_editor_property("mobility", unreal.ComponentMobility.MOVABLE)
    except Exception as e:
        warn("sun: %s" % e)
    sky = spawn(unreal.SkyLight, label="WB Sky Light")
    try:
        sky.light_component.set_editor_property("real_time_capture", True)
        sky.light_component.set_editor_property("mobility", unreal.ComponentMobility.MOVABLE)
    except Exception as e:
        warn("sky light: %s" % e)
    fog = build_fog(l)
    return {"sun": sun, "sky_light": sky, "fog": fog, "look": build_look(l), "clouds": build_clouds(l)}


# Unreal's stock volumetric clouds (`m_SimpleVolumetricCloud`): their coverage for the editor's
# cloud cover 0 and 1 (its coverage is an offset; measured side by side), and the layer's depth
CLOUD_COVERAGE = (-0.6, 0.35)
CLOUD_LAYER_KM = 4.0


def build_clouds(l):
    """VolumetricCloud at the editor's cloud base with the world's cover (the atmosphere runtime
    moves the cover and storm cloud with the weather)."""
    try:
        a = spawn(unreal.VolumetricCloud, label="WB Clouds")
        c = a.get_component_by_class(unreal.VolumetricCloudComponent)
        c.set_editor_property("layer_bottom_altitude", l.get("cloud_base_m", 1500.0) / 1000.0)
        c.set_editor_property("layer_height", CLOUD_LAYER_KM)
        stock = c.get_editor_property("material")
        if stock is not None:
            mi = new_asset("MI_WB_Clouds", DEST + "/Materials", unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew())
            mi.set_editor_property("parent", stock)
            cover = CLOUD_COVERAGE[0] + (CLOUD_COVERAGE[1] - CLOUD_COVERAGE[0]) * max(0.0, min(1.0, l["clouds"]))
            MEL.set_material_instance_scalar_parameter_value(mi, "Cloud_GlobalCoverage", cover)
            unreal.EditorAssetLibrary.save_loaded_asset(mi)
            c.set_editor_property("material", mi)
        return a
    except Exception as e:
        warn("clouds: %s" % e)
        return None


# the share of the calibrated haze's clear-air term (`Haze::AIR`) that Unreal's sky atmosphere
# already adds as aerial perspective: little at these distances (measured side by side)
AIR_PER_M = 0.0
# The overworld's light units: its sun is 10 lux for Skyfold Studio's 110 000 (the manual
# exposure is set for that), so the world's lights (fires, crystals; physical lumens) scale alike
# (the dungeons keep physical units under their own exposure)
LIGHT_SCALE = 10.0 / 110000.0
# Unreal's manual exposure (EV bias) that lands its sunlit ground at the editor's brightness
EXPOSURE_CALIBRATION_EV = -1.3
# Unreal's filmic tone curve saturates more than the editor's: this global saturation before it
# lands the editor's colours (the editor's own saturation and contrast are in the LUT)
TONEMAP_SATURATION = 0.82
# Godot adds volumetric fog on top of this density; Unreal's height fog carries the editor's
# whole haze alone (measured side by side)
FOG_DENSITY_GAIN = 2.5
# the haze colour (sRGB, under the exposure) as fog in-scattering luminance
FOG_LUMINANCE = 1.0


def build_fog(l):
    """The editor's distance haze as exponential height fog (calibrated like the Godot and Unity
    exports: the same density and colour)."""
    density = l.get("haze_density")
    if density is None:
        if not l["fog"]:
            return
        density, color = l["fog_density"] / 10.0, l["fog_color"]
    else:
        color = [srgb_to_linear(c) for c in l["haze_color"]]
    extra = max(density - AIR_PER_M, 0.0)
    fog = spawn(unreal.ExponentialHeightFog, label="WB Height Fog")
    try:
        fc = fog.component
        # Unreal's fog density is per 1000 units (cm): per metre x 10
        fc.set_editor_property("fog_density", extra * 10.0 * FOG_DENSITY_GAIN)
        # the editor's haze grows with distance, not with depth below the camera: nearly flat
        fc.set_editor_property("fog_height_falloff", 0.001)
        fc.set_editor_property("fog_inscattering_luminance", unreal.LinearColor(color[0] * FOG_LUMINANCE, color[1] * FOG_LUMINANCE, color[2] * FOG_LUMINANCE, 1.0))
        fog.set_actor_location(to_ue([0.0, DATA["water"].get("sea_level", 0.0) if DATA["water"]["enabled"] else 0.0, 0.0]), False, False)
    except Exception as e:
        warn("fog: %s" % e)
    return fog


def srgb_to_linear(v):
    return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4


def build_look(l):
    """An unbound PostProcessVolume with the world's look: manual exposure at the editor's,
    bloom, vignette, and its colour grade after tonemapping (saturation, contrast, split toning,
    lift, foliage hue, vibrance) as a colour grading LUT (`Look/grade_lut.png`)."""
    ppv = spawn(unreal.PostProcessVolume, label="WB Look")
    try:
        ppv.set_editor_property("unbound", True)
        st = ppv.get_editor_property("settings")
        values = [
            ("auto_exposure_method", unreal.AutoExposureMethod.AEM_MANUAL),
            ("auto_exposure_apply_physical_camera_exposure", False),
            ("auto_exposure_bias", l.get("ev", l["exposure"]) + EXPOSURE_CALIBRATION_EV),
            ("bloom_intensity", l["bloom"]),
            ("vignette_intensity", l["vignette"]),
        ]
        lut = load_lut(l.get("lut"))
        if lut is not None:
            k = TONEMAP_SATURATION
            values += [("color_grading_lut", lut), ("color_grading_intensity", 1.0), ("color_saturation", unreal.Vector4(k, k, k, 1.0))]
        else:
            for key, v in (("color_saturation", l["saturation"]), ("color_contrast", l["contrast"])):
                values.append((key, unreal.Vector4(v, v, v, 1.0)))
        for key, value in values:
            try:
                st.set_editor_property("override_" + key, True)
            except Exception:
                pass
            st.set_editor_property(key, value)
        ppv.set_editor_property("settings", st)
    except Exception as e:
        warn("post process: %s" % e)
    return ppv


def load_lut(rel):
    if not rel:
        return None
    try:
        import_files([os.path.join(HERE, rel)], DEST + "/Look")
        t = assets_in(DEST + "/Look", unreal.Texture2D)
        lut = next(iter(t.values()), None)
        if lut is not None:
            lut.set_editor_property("lod_group", unreal.TextureGroup.TEXTUREGROUP_COLOR_LOOKUP_TABLE)
            lut.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_VECTOR_DISPLACEMENTMAP)
            lut.set_editor_property("mip_gen_settings", unreal.TextureMipGenSettings.TMGS_NO_MIPMAPS)
            lut.set_editor_property("srgb", False)
            unreal.EditorAssetLibrary.save_loaded_asset(lut)
        return lut
    except Exception as e:
        warn("look LUT: %s" % e)
        return None


def particle_material(name, snow):
    """Rain streaks (a faint vertical gradient) or snow flakes (soft discs): unlit, translucent,
    fading out right in front of the lens."""
    try:
        mat = new_asset(name, DEST + "/Materials", unreal.Material, unreal.MaterialFactoryNew())
        mat.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT)
        mat.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_UNLIT)
        mat.set_editor_property("two_sided", True)
        # (drawn as instances round the camera: compiled for it up front, so they never show as
        # the default material while it compiles in game)
        mat.set_editor_property("used_with_instanced_static_meshes", True)
        col = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant3Vector, -500, 0)
        col.set_editor_property("constant", unreal.LinearColor(1.0, 1.0, 1.0, 1.0) if snow else unreal.LinearColor(0.75, 0.8, 0.88, 1.0))
        MEL.connect_material_property(col, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
        uv = MEL.create_material_expression(mat, unreal.MaterialExpressionTextureCoordinate, -900, 200)
        half = MEL.create_material_expression(mat, unreal.MaterialExpressionConstant2Vector, -900, 320)
        half.set_editor_property("r", 0.5)
        half.set_editor_property("g", 0.5)
        sub = MEL.create_material_expression(mat, unreal.MaterialExpressionSubtract, -750, 250)
        MEL.connect_material_expressions(uv, "", sub, "A")
        MEL.connect_material_expressions(half, "", sub, "B")
        if snow:
            shape_in = sub
        else:
            # a streak: fade along its length only
            mask = MEL.create_material_expression(mat, unreal.MaterialExpressionComponentMask, -620, 250)
            mask.set_editor_property("r", True)
            MEL.connect_material_expressions(sub, "", mask, "")
            shape_in = mask
        ln = MEL.create_material_expression(mat, unreal.MaterialExpressionLength, -500, 250)
        MEL.connect_material_expressions(shape_in, "", ln, "")
        k = MEL.create_material_expression(mat, unreal.MaterialExpressionMultiply, -380, 250)
        k.set_editor_property("const_b", 2.0)
        MEL.connect_material_expressions(ln, "", k, "A")
        inv = MEL.create_material_expression(mat, unreal.MaterialExpressionOneMinus, -260, 250)
        MEL.connect_material_expressions(k, "", inv, "")
        alpha = MEL.create_material_expression(mat, unreal.MaterialExpressionMultiply, -140, 250)
        alpha.set_editor_property("const_b", 0.95 if snow else 0.3)
        MEL.connect_material_expressions(inv, "", alpha, "A")
        # 0 within 60 cm of the lens, 1 from 3 m
        depth = MEL.create_material_expression(mat, unreal.MaterialExpressionPixelDepth, -500, 400)
        off = MEL.create_material_expression(mat, unreal.MaterialExpressionSubtract, -380, 400)
        off.set_editor_property("const_b", 60.0)
        MEL.connect_material_expressions(depth, "", off, "A")
        span = MEL.create_material_expression(mat, unreal.MaterialExpressionDivide, -260, 400)
        span.set_editor_property("const_b", 240.0)
        MEL.connect_material_expressions(off, "", span, "A")
        fade = MEL.create_material_expression(mat, unreal.MaterialExpressionSaturate, -140, 400)
        MEL.connect_material_expressions(span, "", fade, "")
        both = MEL.create_material_expression(mat, unreal.MaterialExpressionMultiply, -20, 300)
        MEL.connect_material_expressions(alpha, "", both, "A")
        MEL.connect_material_expressions(fade, "", both, "B")
        MEL.connect_material_property(both, "", unreal.MaterialProperty.MP_OPACITY)
        MEL.recompile_material(mat)
        unreal.EditorAssetLibrary.save_loaded_asset(mat)
        return mat
    except Exception as e:
        warn("%s: %s" % (name, e))
        return None


def build_atmosphere(scene):
    """The weather and play runtime (the plugin's SkyfoldAtmosphere): the zone and biome maps,
    weathers, zones and biomes, and what it drives (sun, sky light, fog, look, rain and snow)."""
    cls = getattr(unreal, "SkyfoldAtmosphere", None)
    lib = getattr(unreal, "SkyfoldStudioLibrary", None)
    path = os.path.join(HERE, "Data", "atmosphere.json")
    if cls is None or lib is None or not os.path.exists(path):
        if cls is None:
            warn("the Skyfold Studio plugin isn't compiled: no zone weather, rain, snow or explorer in game")
        return None
    a = spawn(cls, label="WB Atmosphere")
    if a is None:
        return None
    try:
        data = json.load(open(path))
        g = data["grid"]
        n, cell = g["vertices"], g["cell"]
        half = (n - 1) * cell * 0.5
        if not lib.load_atmosphere(a, path, os.path.join(HERE, g["zones"]), os.path.join(HERE, g["biomes"]), n):
            warn("atmosphere data didn't load")
        a.set_editor_property("map_origin", to_ue([-half, 0.0, -half]))
        a.set_editor_property("map_step_i", to_ue([cell, 0.0, 0.0]))
        a.set_editor_property("map_step_j", to_ue([0.0, 0.0, cell]))
        a.set_editor_property("parameters", world_parameters())
        a.set_editor_property("cloud_coverage_clear", CLOUD_COVERAGE[0])
        a.set_editor_property("cloud_coverage_overcast", CLOUD_COVERAGE[1])
        for key in ("sun", "sky_light", "fog", "look", "clouds"):
            if scene.get(key) is not None:
                a.set_editor_property(key, scene[key])
        a.set_editor_property("fog_units_per_metre", 10.0)
        a.set_editor_property("fog_luminance", FOG_LUMINANCE)
        a.set_editor_property("particle_mesh", unreal.EditorAssetLibrary.load_asset("/Engine/BasicShapes/Plane.Plane"))
        a.set_editor_property("rain_material", particle_material("M_WB_Rain", False))
        a.set_editor_property("snow_material", particle_material("M_WB_Snow", True))
        a.set_folder_path("WorldBuilder")
        log("atmosphere: %d zones, %d biomes, weather %s" % (len(data.get("zones", {})), len(data.get("biomes", {})), data["world"].get("override") or "by zone"))
    except Exception as e:
        warn("atmosphere: %s" % e)
    return a


def set_game_mode():
    """Play explores the level as Skyfold Studio's walk mode (the plugin's SkyfoldGameMode)."""
    gm = getattr(unreal, "SkyfoldGameMode", None)
    if gm is None:
        return
    try:
        world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world()
        world.get_world_settings().set_editor_property("default_game_mode", gm)
    except Exception as e:
        warn("game mode: %s" % e)


FLAME_HLSL = r"""
// Skyfold Studio's flame (Godot's wb_flame.gdshader): two neighbouring frames blended, each fire
// at its own phase and pace (Seed), half mirrored, a wide fire (Spread) one flame as wide as its
// bed narrowing as it rises; magic fires (Tint.w) take their light's colour. Premultiplied.
float frames = Grid.z;
float pace = 1.0 + (frac(Seed * 13.71) - 0.5) * 0.16;
float f = Time * Grid.w * pace + Seed * frames;
float i0 = fmod(f, frames);
float a = floor(i0);
float b = fmod(a + 1.0, frames);
float k = i0 - a;
float2 st = UV;
float s = max(Spread, 1.0);
float wide = lerp(s, 1.0, smoothstep(0.06, 0.78, 1.0 - UV.y));
st.x = 0.5 + (UV.x - 0.5) * s / wide;
float inside = smoothstep(0.0, 0.04, st.x) * smoothstep(1.0, 0.96, st.x);
st.x = saturate(st.x);
if (frac(Seed * 7.31) > 0.5) { st.x = 1.0 - st.x; }
st = clamp(st, 0.003, 0.997);
float2 ca = (float2(fmod(a, Grid.x), floor(a / Grid.x)) + st) / Grid.xy;
float2 cb = (float2(fmod(b, Grid.x), floor(b / Grid.x)) + st) / Grid.xy;
float4 c = lerp(Texture2DSample(Sheet, SheetSampler, ca), Texture2DSample(Sheet, SheetSampler, cb), k);
float heat = max(c.r, max(c.g, c.b));
float3 magic = Tint.rgb * heat * 1.2 + pow(heat, 3.0) * 0.5;
float3 col = lerp(c.rgb, magic, Tint.w);
float edge = smoothstep(0.0, 0.06, UV.x) * smoothstep(1.0, 0.94, UV.x) * smoothstep(0.0, 0.1, UV.y);
float m = edge * inside;
return float4(col * m, c.a * m);
"""


FLAME_WPO_HLSL = r"""
// stand the plane (vertices +-50 cm in X and Y) upright facing the camera round the vertical,
// Size wide and tall (cm), its rows (+Y) running down
float3 to = Cam - Obj;
to.z = 0.0;
to = length(to) > 1e-3 ? normalize(to) : float3(1.0, 0.0, 0.0);
float3 right = normalize(cross(float3(0.0, 0.0, 1.0), to));
float3 p = Obj + right * (Local.x / 100.0 * Size.x) - float3(0.0, 0.0, 1.0) * (Local.y / 100.0 * Size.y);
return p - Abs;
"""


def flame_material():
    """M_WB_Flame: Skyfold Studio's flame flipbooks (premultiplied sheets of `Columns` x `Rows`,
    `Frames` at `Fps`), unlit and alpha-composited, its glow scaled by `Strength` and its cover by
    `Occlusion`; each fire's seed and spread come from its custom primitive data (0, 1)."""
    mat = new_asset("M_WB_Flame", DEST + "/Materials", unreal.Material, unreal.MaterialFactoryNew())
    mat.set_editor_property("blend_mode", unreal.BlendMode.BLEND_ALPHA_COMPOSITE)
    mat.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_UNLIT)
    mat.set_editor_property("two_sided", True)

    def scalar(name, value, x, y):
        p = MEL.create_material_expression(mat, unreal.MaterialExpressionScalarParameter, x, y)
        p.set_editor_property("parameter_name", name)
        p.set_editor_property("default_value", value)
        return p

    def rgba(p, x, y):
        """A vector parameter as a float4 (its default output is RGB)."""
        a = MEL.create_material_expression(mat, unreal.MaterialExpressionAppendVector, x, y)
        MEL.connect_material_expressions(p, "", a, "A")
        MEL.connect_material_expressions(p, "A", a, "B")
        return a

    custom = MEL.create_material_expression(mat, unreal.MaterialExpressionCustom, -400, 200)
    custom.set_editor_property("code", FLAME_HLSL)
    custom.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT4)
    names = ["UV", "Time", "Grid", "Seed", "Spread", "Tint", "Sheet"]
    inputs = []
    for n in names:
        ci = unreal.CustomInput()
        ci.set_editor_property("input_name", n)
        inputs.append(ci)
    custom.set_editor_property("inputs", inputs)
    MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionTextureCoordinate, -900, 0), "", custom, "UV")
    MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionTime, -900, 80), "", custom, "Time")
    grid = MEL.create_material_expression(mat, unreal.MaterialExpressionVectorParameter, -900, 160)
    grid.set_editor_property("parameter_name", "Grid")
    grid.set_editor_property("default_value", unreal.LinearColor(8.0, 6.0, 48.0, 24.0))
    MEL.connect_material_expressions(rgba(grid, -760, 160), "", custom, "Grid")
    for k, (n, v) in enumerate((("Seed", 0.0), ("Spread", 1.0))):
        d = scalar(n, v, -900, 260 + k * 80)
        d.set_editor_property("use_custom_primitive_data", True)
        d.set_editor_property("primitive_data_index", k)
        MEL.connect_material_expressions(d, "", custom, n)
    tint = MEL.create_material_expression(mat, unreal.MaterialExpressionVectorParameter, -900, 440)
    tint.set_editor_property("parameter_name", "Tint")
    tint.set_editor_property("default_value", unreal.LinearColor(1.0, 0.62, 0.3, 0.0))
    MEL.connect_material_expressions(rgba(tint, -760, 440), "", custom, "Tint")
    sheet = MEL.create_material_expression(mat, unreal.MaterialExpressionTextureObjectParameter, -900, 540)
    sheet.set_editor_property("parameter_name", "Sheet")
    MEL.connect_material_expressions(sheet, "", custom, "Sheet")
    rgb = MEL.create_material_expression(mat, unreal.MaterialExpressionComponentMask, -200, 160)
    for c in ("r", "g", "b"):
        rgb.set_editor_property(c, True)
    MEL.connect_material_expressions(custom, "", rgb, "")
    glow = MEL.create_material_expression(mat, unreal.MaterialExpressionMultiply, -60, 160)
    MEL.connect_material_expressions(rgb, "", glow, "A")
    MEL.connect_material_expressions(scalar("Strength", 2.2, -200, 60), "", glow, "B")
    MEL.connect_material_property(glow, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
    alpha = MEL.create_material_expression(mat, unreal.MaterialExpressionComponentMask, -200, 300)
    alpha.set_editor_property("a", True)
    MEL.connect_material_expressions(custom, "", alpha, "")
    cover = MEL.create_material_expression(mat, unreal.MaterialExpressionMultiply, -60, 300)
    MEL.connect_material_expressions(alpha, "", cover, "A")
    MEL.connect_material_expressions(scalar("Occlusion", 0.35, -200, 400), "", cover, "B")
    MEL.connect_material_property(cover, "", unreal.MaterialProperty.MP_OPACITY)
    # the billboard: a world position offset (each fire's size in its custom data 2, 3)
    wpo = MEL.create_material_expression(mat, unreal.MaterialExpressionCustom, -400, 800)
    wpo.set_editor_property("code", FLAME_WPO_HLSL)
    wpo.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT3)
    ins = []
    for n in ("Local", "Obj", "Cam", "Abs", "Size"):
        ci = unreal.CustomInput()
        ci.set_editor_property("input_name", n)
        ins.append(ci)
    wpo.set_editor_property("inputs", ins)
    MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionLocalPosition, -900, 700), "", wpo, "Local")
    MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionObjectPositionWS, -900, 760), "", wpo, "Obj")
    MEL.connect_material_expressions(MEL.create_material_expression(mat, unreal.MaterialExpressionCameraPositionWS, -900, 820), "", wpo, "Cam")
    absp = MEL.create_material_expression(mat, unreal.MaterialExpressionWorldPosition, -900, 880)
    absp.set_editor_property("world_position_shader_offset", unreal.WorldPositionIncludedOffsets.WPT_EXCLUDE_ALL_SHADER_OFFSETS)
    MEL.connect_material_expressions(absp, "", wpo, "Abs")
    sw = scalar("Width", 50.0, -1060, 940)
    sw.set_editor_property("use_custom_primitive_data", True)
    sw.set_editor_property("primitive_data_index", 2)
    sh = scalar("Height", 80.0, -1060, 1000)
    sh.set_editor_property("use_custom_primitive_data", True)
    sh.set_editor_property("primitive_data_index", 3)
    size = MEL.create_material_expression(mat, unreal.MaterialExpressionAppendVector, -900, 960)
    MEL.connect_material_expressions(sw, "", size, "A")
    MEL.connect_material_expressions(sh, "", size, "B")
    MEL.connect_material_expressions(size, "", wpo, "Size")
    MEL.connect_material_property(wpo, "", unreal.MaterialProperty.MP_WORLD_POSITION_OFFSET)
    MEL.recompile_material(mat)
    unreal.EditorAssetLibrary.save_loaded_asset(mat)
    return mat


def build_fires(fires, folder="WorldBuilder/Fires", overworld=True):
    """Braziers, torches, campfires and candles: Skyfold Studio's flame flipbooks (the plugin's
    SkyfoldFire: a camera-facing sheet and its flickering light, in lumens)."""
    cls = getattr(unreal, "SkyfoldFire", None)
    if not fires or cls is None:
        return 0
    sheets = DATA.get("flames", {})
    try:
        base = flame_material()
        files = [os.path.join(HERE, "Flames", v["file"]) for v in sheets.values() if os.path.exists(os.path.join(HERE, "Flames", v["file"]))]
        import_files(files, DEST + "/Flames")
        tex = assets_in(DEST + "/Flames", unreal.Texture2D)
    except Exception as e:
        warn("fires: %s" % e)
        return 0
    mats = {}
    n = 0
    plane = unreal.EditorAssetLibrary.load_asset("/Engine/BasicShapes/Plane.Plane")
    for f in fires:
        kind = f["fire"]
        sh = sheets.get(kind)
        if sh is None:
            continue
        key = (kind, tuple(round(c, 3) for c in f["color_linear"]), f["strength"], round(f.get("magic", 0.0), 2))
        mi = mats.get(key)
        if mi is None:
            try:
                mi = new_asset("MI_WB_Flame_%s_%d" % (kind, len(mats)), DEST + "/Materials", unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew())
                mi.set_editor_property("parent", base)
                t = tex.get(os.path.splitext(sh["file"])[0])
                if t is not None:
                    MEL.set_material_instance_texture_parameter_value(mi, "Sheet", t)
                MEL.set_material_instance_vector_parameter_value(mi, "Grid", unreal.LinearColor(float(sh["columns"]), float(sh["rows"]), float(sh["frames"]), float(sh["fps"])))
                MEL.set_material_instance_scalar_parameter_value(mi, "Strength", float(f["strength"]))
                c = f["color_linear"]
                MEL.set_material_instance_vector_parameter_value(mi, "Tint", unreal.LinearColor(c[0], c[1], c[2], float(f.get("magic", 0.0))))
                unreal.EditorAssetLibrary.save_loaded_asset(mi)
            except Exception as e:
                warn("flame material %s: %s" % (kind, e))
                continue
            mats[key] = mi
        w, h = f["frame"]
        bottom = f["frame_bottom"]
        centre = [f["foot"][0], bottom + h * 0.5, f["foot"][2]]
        a = spawn(cls, to_ue(centre), None, "WB Fire %s %d" % (kind, n))
        if a is None:
            continue
        try:
            a.flame.set_static_mesh(plane)
            a.flame.set_material(0, mi)
            a.set_editor_property("size", unreal.Vector2D(w * 100.0, h * 100.0))
            a.set_editor_property("seed", (n * 0.618034) % 1.0)
            a.set_editor_property("spread", float(f.get("spread", 1.0)))
            if f.get("light", True):
                lc = a.light
                lc.set_editor_property("intensity_units", unreal.LightUnits.LUMENS)
                lc.set_intensity(f["light_lumens"] * (LIGHT_SCALE if overworld else 1.0))
                lc.set_attenuation_radius(f["light_range"] * 100.0)
                c = f["color_linear"]
                lc.set_light_color(unreal.LinearColor(c[0], c[1], c[2], 1.0))
                lp = to_ue(f["light_position"])
                lc.set_world_location(lp, False, False)
            else:
                a.light.set_visibility(False)
            a.set_folder_path(folder)
            n += 1
        except Exception as e:
            warn("fire %d: %s" % (n, e))
    return n


def build_fixture_lights(lights, folder="WorldBuilder/Lights"):
    """The placed models' light fixtures (downlights, lanterns, street lights): a point or spot
    light each, in the fire lights' units (`engine_lumens` x LIGHT_SCALE); street and path lights
    (`night_only`) at the exported hour's share (`on`)."""
    n = 0
    for l in lights:
        share = float(l.get("on", 1.0))
        if share <= 0.0:
            continue
        spot = bool(l.get("spot"))
        a = spawn(unreal.SpotLight if spot else unreal.PointLight, to_ue(l["position"]), None, "WB Fixture Light %d" % n)
        if a is None:
            continue
        try:
            c = a.spot_light_component if spot else a.point_light_component
            c.set_editor_property("intensity_units", unreal.LightUnits.LUMENS)
            c.set_intensity(l["engine_lumens"] * LIGHT_SCALE * share)
            c.set_attenuation_radius(l["range"] * 100.0)
            cl = l["color_linear"]
            c.set_light_color(unreal.LinearColor(cl[0], cl[1], cl[2], 1.0))
            c.set_cast_shadows(bool(l.get("shadows", False)))
            if spot:
                d = to_ue(l["direction"])
                a.set_actor_rotation(unreal.MathLibrary.make_rot_from_x(d), False)
                half = float(l.get("angle_deg", 70.0)) * 0.5
                c.set_outer_cone_angle(half)
                c.set_inner_cone_angle(half * 0.7)
            a.set_folder_path(folder)
            n += 1
        except Exception as e:
            warn("fixture light %d: %s" % (n, e))
    return n


def build_markers():
    n = 0
    for m in DATA["markers"]:
        cls = unreal.PlayerStart if m["kind"] == "player_start" else unreal.TargetPoint
        # Skyfold Studio heading: 0 faces +X, counter-clockwise seen from above
        facing = to_ue([math.cos(m["yaw"]), 0.0, -math.sin(m["yaw"])])
        rot = unreal.MathLibrary.make_rot_from_x(facing)
        a = spawn(cls, to_ue(m["position"]), rot, "WB %s: %s" % (m["kind"], m["name"]))
        if a is None:
            continue
        try:
            tags = ["wb_kind:" + m["kind"], "wb_id:%d" % m["id"], "wb_type:" + m["type"], "wb_radius:%g" % m["radius"]]
            a.set_editor_property("tags", [unreal.Name(t) for t in tags])
            a.set_folder_path("WorldBuilder/Gameplay")
        except Exception:
            pass
        n += 1
    return n


# --------------------------------------------------------------------------- dungeons


def tagged_trigger(t, label, tags, level=None, arrive=None):
    """A portal over a Skyfold trigger (center, rotation, half size): with the Skyfold Studio
    plugin a SkyfoldPortal that opens `level` on overlap and puts the explorer at `arrive`; else a
    TriggerBox. Either way its link is in the tags: `wb_portal`, `wb_level=<level>`,
    `wb_via=<way in>`, `wb_arrive=x,y,z,lx,ly,lz` (Unreal cm and axes)."""
    portal = getattr(unreal, "SkyfoldPortal", None)
    a = spawn(portal or unreal.TriggerBox, to_ue(t["center"]), to_ue_rotator(t["rotation"]), label)
    if a is None:
        return None
    try:
        h = t["half_size"]
        extent = unreal.Vector(*[abs(h[AXES[k][0]]) * 100.0 for k in range(3)])
        if portal is not None:
            a.box.set_box_extent(extent)
            if level:
                a.set_editor_property("level", level)
            if arrive:
                p = to_ue(arrive["position"])
                lv = to_ue(arrive["look"])
                a.set_editor_property("arrive_location", p)
                a.set_editor_property("arrive_yaw", math.degrees(math.atan2(lv.y, lv.x)))
            a.set_editor_property("label", label)
        else:
            a.collision_component.set_box_extent(extent)
    except Exception as e:
        warn("trigger for %s: %s" % (label, e))
    a.tags = [unreal.Name(x) for x in ["wb_portal"] + tags]
    a.set_folder_path("WorldBuilder/Dungeons")
    return a


def build_dungeon_pieces(d, meshes):
    """Doors, gates, bars, secret doors, levers, plates and traps: their parts (movable static
    mesh actors, colliding as they stand) attached under a TargetPoint at the piece's foot, which
    a Blueprint can work (tags: `wb_piece`, `wb_kind=<kind>`, `wb_id=<id>`, `wb_open=0|1`,
    `wb_links=<ids>`, `wb_label=<text>`). The game's markers (arenas, checkpoints, loot and spawn
    spots) are TargetPoints tagged likewise. Each part's shut and open poses (relative to the
    piece, glTF space) are in the dungeon's dungeon.json."""
    n = 0
    for p in d.get("pieces", []):
        tags = ["wb_piece", "wb_kind=" + p["kind"], "wb_id=%d" % p["id"], "wb_open=%d" % int(p.get("open", False)),
                "wb_links=" + ",".join(str(x) for x in p.get("links", [])), "wb_width=%.2f" % p["width"]]
        if p.get("label"):
            tags.append("wb_label=" + p["label"])
        label = "WB %s %d" % (p["name"], p["id"])
        a = spawn(unreal.TargetPoint, to_ue(p["position"]), to_ue_rotator(p["rotation"]), label)
        if a is None:
            continue
        a.tags = [unreal.Name(x) for x in tags]
        a.set_folder_path("WorldBuilder/Pieces")
        for part in p.get("parts", []):
            for name, m in meshes.items():
                if name.endswith(part["node"]):
                    try:
                        m.static_mesh_component.set_mobility(unreal.ComponentMobility.MOVABLE)
                        m.attach_to_actor(a, unreal.Name(""), unreal.AttachmentRule.KEEP_WORLD, unreal.AttachmentRule.KEEP_WORLD, unreal.AttachmentRule.KEEP_WORLD, False)
                    except Exception as e:
                        warn("piece %s: %s" % (label, e))
        n += 1
    if n:
        log("%d pieces (tagged wb_piece: work them from a Blueprint; poses in dungeon.json)" % n)


def arrive_tag(look):
    p = to_ue(look["position"])
    l = to_ue(look["look"])
    return "wb_arrive=%.1f,%.1f,%.1f,%.3f,%.3f,%.3f" % (p.x, p.y, p.z, l.x / 100.0, l.y / 100.0, l.z / 100.0)


def build_dungeon_lights(d):
    lights = d.get("lights", [])
    shadowed = set(sorted(range(len(lights)), key=lambda k: -lights[k]["lumens"])[:12])
    for k, l in enumerate(lights):
        cls = unreal.SpotLight if l["spot"] else unreal.PointLight
        rot = None
        if l["spot"]:
            dv = to_ue(l["direction"])
            rot = unreal.MathLibrary.make_rot_from_x(unreal.Vector(dv.x, dv.y, dv.z))
        a = spawn(cls, to_ue(l["position"]), rot, "WB %s %d" % (l["kind"], l["id"]))
        if a is None:
            continue
        try:
            c = a.light_component
            c.set_editor_property("intensity_units", unreal.LightUnits.LUMENS)
            c.set_intensity(l["lumens"])
            c.set_light_color(unreal.LinearColor(l["color_linear"][0], l["color_linear"][1], l["color_linear"][2], 1.0))
            c.set_attenuation_radius(l["range"] * 100.0)
            c.set_cast_shadows(l["shadows"] and k in shadowed)
            c.set_editor_property("volumetric_scattering_intensity", 2.0 if l.get("haze") else 0.0)
            if l["spot"]:
                c.set_outer_cone_angle(max(1.0, l["cone_deg"] * 0.5))
                c.set_inner_cone_angle(max(0.5, l["cone_deg"] * 0.35))
            a.set_folder_path("WorldBuilder/Lights")
        except Exception as e:
            warn("light %s: %s" % (l["id"], e))


def build_dungeon_sky(d):
    """Under a sky (open-ceilinged rooms): its sun or moon as a directional light, a sky
    atmosphere and a sky light."""
    s = d.get("sky") or {}
    if s.get("mode", "none") == "none":
        return
    dv = to_ue(s["light_direction"])
    sun = spawn(unreal.DirectionalLight, None, unreal.MathLibrary.make_rot_from_x(unreal.Vector(dv.x, dv.y, dv.z)), "WB Dungeon Sun")
    if sun is not None:
        try:
            c = sun.light_component
            c.set_intensity({"day": 10.0, "dusk": 3.0, "night": 0.3}.get(s["mode"], 5.0))
            c.set_light_color(unreal.LinearColor(s["color"][0], s["color"][1], s["color"][2], 1.0))
            c.set_editor_property("atmosphere_sun_light", True)
        except Exception as e:
            warn("dungeon sun: %s" % e)
    spawn(unreal.SkyAtmosphere, None, None, "WB Dungeon Sky")


def build_dungeon_mood(d):
    m = d.get("mood") or {}
    if not m:
        return
    fog = spawn(unreal.ExponentialHeightFog, None, None, "WB Dungeon Fog")
    if fog is not None:
        try:
            c = fog.component
            c.set_fog_density(min(0.05, m["fog_density"] * 10.0))
            c.set_fog_height_falloff(0.001)
            fc = m["fog_linear"]
            c.set_fog_inscattering_color(unreal.LinearColor(fc[0], fc[1], fc[2], 1.0))
            c.set_volumetric_fog(m.get("haze", 0.0) > 0.05)
        except Exception as e:
            warn("dungeon fog: %s" % e)
    sky = spawn(unreal.SkyLight, None, None, "WB Dungeon Fill")
    if sky is not None:
        try:
            c = sky.light_component
            a = m["ambient_linear"]
            c.set_editor_property("source_type", unreal.SkyLightSourceType.SLS_SPECIFIED_CUBEMAP)
            c.set_light_color(unreal.LinearColor(a[0], a[1], a[2], 1.0))
            c.set_intensity(max(0.02, m["ambient_strength"] * 0.5))
        except Exception as e:
            warn("dungeon fill: %s" % e)


def build_ways_in(levels):
    """The world's ways into its dungeons: their glowing sheets and a tagged trigger each."""
    if DATA.get("portal_sheets"):
        place_static(DATA["portal_sheets"], "Dungeons/Portals", "WB Portal")
    n = 0
    for w in DATA.get("ways_in", []):
        level = levels.get(w["dungeon"], w["level"])
        if tagged_trigger(w, "WB Way In %s" % w["label"], ["wb_level=" + level, "wb_via=%d" % w["via"], arrive_tag(w["arrive"])], level, w["arrive"]) is not None:
            n += 1
    return n


def build_dungeons():
    """A level per dungeon (`<dest>/Dungeons/L_<name>`): its architecture (complex collision but
    liquids and the way out's sheet), lights, objects, fog and fill, a PlayerStart and a tagged
    trigger back to the world's level; then the world's ways in. Returns the levels made."""
    ds = DATA.get("dungeons", [])
    if not ds and not DATA.get("ways_in"):
        return []
    les = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
    world = editor_world()
    main = world.get_path_name().split(".")[0] if world else ""
    levels = {d["safe_name"]: DEST + "/Dungeons/" + d["level"] for d in ds}
    if not main or main.startswith("/Temp"):
        warn("save this level first, then run the import again: the dungeons are levels of their own and their ways out lead back here")
        build_ways_in(levels)
        return []
    les.save_current_level()
    made = []
    by_safe = {m["safe_name"]: m for m in DATA["models"]}
    for d in ds:
        path = levels[d["safe_name"]]
        log("dungeon '%s' -> %s" % (d["name"], path))
        ok = les.load_level(path) if unreal.EditorAssetLibrary.does_asset_exist(path) else les.new_level(path)
        if not ok:
            warn("couldn't make the level " + path)
            continue
        meshes = place_static(d["glb"], "Dungeons/" + d["safe_name"], "WB " + d["name"], collide=True, no_collide=("_liquids", "_exit"))
        build_dungeon_pieces(d, meshes)
        build_dungeon_lights(d)
        models = []
        for o in d.get("objects", []):
            m = by_safe.get(o["safe_name"])
            if m is not None:
                models.append(dict(m, instances=o["instances"], stretch=o.get("stretch", [])))
        place_models(models, "WorldBuilder/Objects")
        build_dungeon_mood(d)
        build_dungeon_sky(d)
        st = d["start"]
        lv = to_ue(st["look"])
        ps = spawn(unreal.PlayerStart, to_ue(st["position"]), unreal.MathLibrary.make_rot_from_x(unreal.Vector(lv.x, lv.y, 0.0)), "WB Player Start")
        if ps is not None:
            ps.set_folder_path("WorldBuilder")
        if d.get("exit"):
            tags = ["wb_level=" + main] + [arrive_tag(a) + ",via=%d" % a["via"] for a in d["exit"].get("arrivals", [])]
            arrivals = d["exit"].get("arrivals", [])
            tagged_trigger(d["exit"], "WB Way Out", tags, main, arrivals[0] if arrivals else None)
        set_game_mode()
        les.save_current_level()
        made.append(path)
    les.load_level(main)
    n = build_ways_in(levels)
    les.save_current_level()
    log("%d dungeon levels, %d ways in" % (len(made), n))
    return made


# --------------------------------------------------------------------------- main


def main():
    log("importing '%s' into %s" % (DATA["name"], DEST))
    with unreal.ScopedSlowTask(7, "Skyfold Studio import") as task:
        task.make_dialog(True)
        task.enter_progress_frame(1, "Measuring glTF axes")
        measure_axes()
        task.enter_progress_frame(1, "Landscape files and material")
        write_landscape_files()
        tex = import_textures(DATA["landscape"]["layers"])
        land = create_landscape(landscape_material(DATA["landscape"]["layers"], tex, build_grass()))
        task.enter_progress_frame(1, "Models and instances")
        placed = place_models()
        task.enter_progress_frame(1, "Water")
        water = 0
        if DATA["water"]["enabled"]:
            mat = water_material(tex)
            if DATA["water"]["ocean"]:
                plane = unreal.EditorAssetLibrary.load_asset("/Engine/BasicShapes/Plane.Plane")
                size = DATA["landscape"]["size_m"] * 4.0
                o = spawn(unreal.StaticMeshActor, to_ue([0.0, DATA["water"]["sea_level"], 0.0]), None, "WB Ocean")
                o.static_mesh_component.set_static_mesh(plane)
                o.set_actor_scale3d(unreal.Vector(size, size, 1.0))
                if mat is not None:
                    o.static_mesh_component.set_material(0, water_instance(mat, "Ocean", DATA["water"]["ocean_style"]))
                water += 1
            if DATA["water"]["mesh"]:
                mats = {b["node"]: water_instance(mat, b["node"], b["style"]) for b in DATA["water"]["bodies"]} if mat else None
                water += len(place_static(DATA["water"]["mesh"], "Water", "WB Water", mats))
            volumes = build_water_volumes()
        task.enter_progress_frame(1, "Roads")
        roads = len(place_static(DATA["roads"], "Roads", "WB Road")) if DATA["roads"] else 0
        if DATA.get("caves"):
            caves = len(place_static(DATA["caves"], "Caves", "WB Cave", collide=True))
            build_cave_lights()
            log("%d cave meshes (open the landscape's cave mouths with the Visibility tool over Caves/holes.png)" % caves)
        task.enter_progress_frame(1, "Sky, weather and fires")
        build_atmosphere(build_sky())
        fires = build_fires(DATA.get("fires", []))
        if fires:
            log("%d fires" % fires)
        fixtures = build_fixture_lights(DATA.get("fixture_lights", []))
        if fixtures:
            log("%d fixture lights" % fixtures)
        set_game_mode()
        task.enter_progress_frame(1, "Markers")
        markers = build_markers()
        if build_nav_bounds():
            log("navmesh bounds placed: Unreal builds the navmesh from the collision (Build > Build Paths)")
        dungeon_levels = build_dungeons()
    v = landscape_values()
    tiled = v["tiles"] > 1
    if land is not None:
        log("done: landscape, %d instances, %d water surfaces, %d road meshes, %d markers." % (placed, water, roads, markers))
        unreal.EditorDialog.show_message("Skyfold Studio", "Imported the landscape, %d instances, %d water surfaces, %d roads, %d markers." % (placed, water, roads, markers), unreal.AppMsgType.OK)
        return
    steps = (
        "Landscape (manual, Landscape mode > Manage > New > Import from File%s):\n"
        "  Heightmap: %s\n"
        "  Material: %s/Materials/M_WB_Landscape\n"
        "  Layers (Weight-Blended): one per row, files in %s\n"
        "  Section Size 63x63 quads, Sections Per Component 2x2, Number of Components %d x %d\n"
        "  Location X %.1f  Y %.1f  Z %.1f   Rotation 0 0 0\n"
        "  Scale    X %.4f  Y %.4f  Z %.4f\n"
        % (" with World Partition: pick the first tile, the others are found by their _x/_y names" if tiled else "",
           os.path.join(HERE, "Landscape", "Tiles", "heightmap_x0_y0.png") if tiled else os.path.join(HERE, "Landscape", "heightmap_ue.png"), DEST, os.path.join(HERE, "Landscape", "weights_ue"), v["components"], v["components"], v["location"][0], v["location"][1], v["location"][2], v["scale"][0], v["scale"][1], v["scale"][2])
    )
    log("done: %d instances, %d water surfaces, %d road meshes, %d markers.\n%s" % (placed, water, roads, markers, steps))
    unreal.EditorDialog.show_message("Skyfold Studio", "Imported %d instances, %d water surfaces, %d roads, %d markers.\n\n%s" % (placed, water, roads, markers, steps), unreal.AppMsgType.OK)


# (run as a script: Tools > Execute Python Script, py "...", or runpy with run_name "__main__")
if __name__ == "__main__":
    main()
