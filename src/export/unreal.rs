//! Unreal Engine 5 target: a folder with the landscape heightmap and weightmaps (Unreal landscape
//! sizes, the exact Location / Scale for this world in the README), GLB models, water and road
//! meshes, and `import_world.py` (Python Editor Script Plugin, UE 5.4+) that imports the meshes,
//! places hierarchical instanced static meshes, builds sky / sun / fog / post process, water,
//! roads and markers, creates the landscape material and orients the landscape files. Unreal's
//! Python API has no documented way to create a landscape from heightmap files (only to
//! overwrite an existing landscape's heights from a render target), so that one step stays
//! manual: one import, or a World Partition tiled import for big worlds (`tiles`).
//!
//! The script is written against the documented UE Python API but can't be run here: it is
//! defensive (every step logs and carries on). It measures how the glTF importer maps axes with a
//! probe mesh (`Data/axis_probe.glb`) instead of assuming it.

use bevy::math::Vec3;
use serde_json::{Value, json};

use super::core::{self, Ctx, Snapshot, TerrainOut};
use super::generic::clean;
use super::glb::{Glb, Material, Prim};

const SCRIPT: &str = include_str!("unreal/import_world.py");

/// A tetrahedron with corners at 1 m on X, 2 m on Y and 3 m on Z: its imported bounding box shows
/// where each glTF axis lands in Unreal.
fn axis_probe() -> Vec<u8> {
    let mut g = Glb::new();
    let pos = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 2.0, 0.0],
        [0.0, 0.0, 3.0],
    ];
    let nrm = [
        [-0.577, -0.577, -0.577],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ];
    let idx = [0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
    let mat = g.material(&Material::new("probe"));
    let mesh = g.mesh(
        "axis_probe",
        &[Prim {
            positions: &pos,
            normals: Some(&nrm),
            indices: &idx,
            material: Some(mat),
            ..Default::default()
        }],
    );
    let node = g.node("axis_probe", mesh, vec![]);
    g.finish(&[node])
}

/// Landscape Location (cm), Scale and components per side for the default axis mapping
/// (Unreal X = east, Y = south, Z = up; the script re-orients the files if the import differs).
fn landscape_values(t: &TerrainOut) -> ([f32; 3], [f32; 3], usize) {
    let z_scale = 12800.0 * (t.hi - t.lo) / 65535.0;
    let z_loc = 100.0 * t.lo + 32768.0 * 100.0 * (t.hi - t.lo) / 65535.0;
    (
        [-t.half * 100.0, -t.half * 100.0, z_loc],
        [t.cell * 100.0, t.cell * 100.0, z_scale],
        (t.n - 1) / 126,
    )
}

/// The dungeons for the script (Skyfold space: the script converts like everything else): each
/// one's GLB, objects (instances of the world's models), lights, mood, start and way out (a level
/// each), the ways in (trigger boxes in the world's level) and the world's portal sheets.
fn dungeons(
    s: &Snapshot,
    ctx: &Ctx,
    models: &[core::ModelOut],
) -> Result<(Vec<Value>, Vec<Value>, Option<String>), String> {
    if super::dungeons::list(s)?.is_empty() {
        return Ok((vec![], vec![], None));
    }
    let (written, sheets) =
        super::dungeons::write_all(s, ctx, models, "Dungeons", "", &|rel, b| {
            ctx.write_glb(rel, b)
        })?;
    let eye = Vec3::Y * 1.7;
    let look =
        |(p, l): (Vec3, Vec3)| json!({ "position": (p + eye).to_array(), "look": l.to_array() });
    let trigger = |t: &crate::dungeons::export::Trigger| json!({ "center": t.center.to_array(), "rotation": t.rot.to_array(), "half_size": t.half.to_array() });
    let mut dungeons = vec![];
    let mut ways = vec![];
    for w in &written {
        let d = w.d;
        let Some(glb) = &w.glb else { continue };
        ctx.write_json(&format!("Dungeons/{}/dungeon.json", d.slug), &w.json)?;
        let level = format!("L_{}", w.safe);
        let objects: Vec<Value> = models
            .iter()
            .zip(&w.inst)
            .filter(|(_, l)| !l.is_empty())
            .map(|(m, l)| json!({ "safe_name": m.safe, "instances": l.iter().flat_map(|i| core::inst_row(i, i.pos, i.rot)).collect::<Vec<f64>>(), "stretch": core::stretch_rows(l) }))
            .collect();
        let lights: Vec<Value> = d
            .map
            .as_ref()
            .map(crate::dungeons::export::lights_json)
            .unwrap_or_default();
        let start = d.start.unwrap_or((Vec3::ZERO, Vec3::NEG_Z));
        let exit = d.exit.as_ref().map(|t| {
            let mut v = trigger(t);
            v["arrivals"] = json!(
                d.ways_in
                    .iter()
                    .map(|wi| {
                        let mut a = look(wi.arrive);
                        a["via"] = json!(wi.entrance);
                        a
                    })
                    .collect::<Vec<_>>()
            );
            v
        });
        for wi in &d.ways_in {
            let mut v = trigger(&wi.trigger);
            v["dungeon"] = json!(w.safe);
            v["level"] = json!(level);
            v["via"] = json!(wi.entrance);
            v["label"] = json!(format!("{} ({})", d.name, wi.label));
            v["arrive"] = look(start);
            ways.push(v);
        }
        dungeons.push(json!({
            "name": d.name, "safe_name": w.safe, "level": level, "glb": glb,
            "objects": objects, "lights": lights, "mood": d.map.as_ref().map(crate::dungeons::export::mood_json), "sky": d.map.as_ref().map(crate::dungeons::export::sky_json),
            "start": look(start), "exit": exit,
            "pieces": d.map.as_ref().map(|m| super::dungeons::pieces_json(m, &w.safe, &|p| p, &|q| q)).unwrap_or_default(),
        }));
    }
    Ok((dungeons, ways, sheets))
}

pub fn export(s: &Snapshot, ctx: &Ctx) -> Result<Value, String> {
    clean(
        ctx,
        &[
            "Landscape",
            "Models",
            "Textures",
            "Water",
            "Roads",
            "Caves",
            "Data",
            "import_world.py",
            "README.md",
            "CREDITS.md",
            "manifest.json",
        ],
    );
    let o = &ctx.opts;
    if o.shared_textures {
        ctx.shared.enable("Textures");
    }
    let n = o.target.terrain_vertices(o.terrain);
    ctx.stage("Terrain heights and weights", 0.02)?;
    let t = core::terrain(s, n);
    super::generic::write_heightmaps(ctx, "Landscape", &t, false)?;
    super::generic::write_weightmaps(ctx, "Landscape", &t)?;
    for (k, &l) in t.used.iter().enumerate() {
        ctx.write(
            &format!(
                "Landscape/weights/{}.r8",
                crate::terrain::textures::LAYER_FILES[l]
            ),
            &t.weights[k],
        )?;
    }
    // big worlds: World Partition tiles (`_x<i>_y<j>`, i east, j south, sharing their edges) for
    // Landscape mode's tiled "Import from File", oriented for Unreal's usual glTF axes (X east,
    // Y south; the script rewrites them if it measures others)
    let (tiles, tn) = super::tiles::split(o.target, n);
    if tiles > 1 {
        let q = tn - 1;
        let h16 = t.heights16();
        for j in 0..tiles {
            for i in 0..tiles {
                ctx.write(
                    &format!("Landscape/Tiles/heightmap_x{i}_y{j}.png"),
                    &core::png_gray16(
                        tn as u32,
                        tn as u32,
                        super::tiles::block(&h16, n, i * q, j * q, tn, tn),
                    ),
                )?;
                for (k, &l) in t.used.iter().enumerate() {
                    let key = crate::terrain::textures::LAYER_FILES[l];
                    ctx.write(
                        &format!("Landscape/Tiles/{key}_x{i}_y{j}.png"),
                        &core::png_gray8(
                            tn as u32,
                            tn as u32,
                            super::tiles::block(&t.weights[k], n, i * q, j * q, tn, tn),
                        ),
                    )?;
                }
            }
        }
    }
    ctx.stage("Terrain layer textures", 0.08)?;
    let layers = core::layer_textures(s, &t.used, o.texture_size)?;
    for l in &layers {
        ctx.write(
            &format!("Landscape/Layers/{}_albedo.png", l.key),
            &core::png_rgb(&l.albedo),
        )?;
        ctx.write(
            &format!("Landscape/Layers/{}_normal.png", l.key),
            &core::png_rgb(&l.normal),
        )?;
        ctx.write(
            &format!("Landscape/Layers/{}_mask.png", l.key),
            &core::png_rgba(&l.mask),
        )?;
    }
    // (Unreal imports one static mesh per glTF mesh: packed models come with each level of
    // detail baked into one mesh, `lods::packed`)
    let models = core::models(s, ctx, "Models", 0.15, 0.85)?;
    let inst = core::instances(s, &models);
    ctx.stage("Water, roads, gameplay", 0.88)?;
    let mut water = json!({ "enabled": false, "normal_map": "" });
    if o.water {
        let (glb, bodies) = core::water(s);
        if let Some(b) = &glb {
            ctx.write_glb("Water/water.glb", b.clone())?;
        }
        ctx.write("Water/water_normal.png", &core::water_normal_png(512))?;
        let style = |st: &crate::water::WaterStyle| {
            let lin = |c: [f32; 3]| bevy::color::Color::srgb(c[0], c[1], c[2]).to_linear();
            let c = lin(st.shallow);
            json!({ "shallow_linear": [c.red, c.green, c.blue], "transparency": st.transparency, "roughness": st.roughness, "clarity": st.clarity })
        };
        water = json!({
            "enabled": true,
            "ocean": s.terrain.heights.iter().any(|h| *h < s.terrain.sea_level),
            "sea_level": s.terrain.sea_level,
            "ocean_style": style(&s.ocean),
            "mesh": if glb.is_some() { "Water/water.glb" } else { "" },
            "normal_map": "Water/water_normal.png",
            "bodies": bodies.iter().map(|b| json!({ "node": b.node, "kind": b.kind, "name": b.name, "style": style(&b.style) })).collect::<Vec<_>>(),
            "volumes": super::collision::water_volumes(s).iter().map(|v| v.json()).collect::<Vec<_>>(),
        });
    }
    let mut roads = "";
    if o.roads {
        let (glb, list) = core::roads(s, o.texture_size);
        if let Some(b) = glb {
            ctx.write_glb("Roads/roads.glb", b)?;
            ctx.write_json("Data/roads.json", &core::roads_json(&list))?;
            roads = "Roads/roads.glb";
        }
    }
    // caves: their rock, and a mask of the landscape cells they open (Unreal needs those painted
    // with the Visibility tool: see the README)
    let caves = core::caves(s, o.texture_size, "", o.optimize_caves);
    let mut caves_glb = "";
    if let Some(b) = &caves.glb {
        ctx.write_glb("Caves/caves.glb", b.clone())?;
        ctx.write_json(
            "Data/caves.json",
            &json!({ "caves": caves.json, "holes": caves.holes }),
        )?;
        caves_glb = "Caves/caves.glb";
    }
    if let Some(b) = &caves.holes_png {
        ctx.write("Caves/holes.png", b)?;
    }
    let markers = if o.gameplay { core::markers(s) } else { vec![] };
    if o.gameplay {
        ctx.write_json("Data/gameplay.json", &core::gameplay_json(s))?;
    }
    let navmesh = super::nav::write(ctx, &s.navmesh, "Data/Navmesh")?;
    let light = core::lighting(s);
    ctx.write_json("Data/lighting.json", &light.json(s))?;
    ctx.write("Data/axis_probe.glb", &axis_probe())?;
    // dungeons: a level each (built by the script), triggers at their ways in
    let (dungeons_json, ways_json, sheets) = dungeons(s, ctx, &models)?;
    super::dungeons::write_flame_sheets(s, "Flames", &|rel, b| ctx.write(rel, b))?;
    ctx.shared.write_index(ctx)?;
    let doc = json!({
        "name": s.name,
        "safe_name": core::safe_name(&s.name),
        "landscape": {
            "resolution": n,
            "size_m": t.size,
            "height_min_m": t.lo,
            "height_max_m": t.hi,
            "quads_per_component": 126,
            "tiles_per_side": tiles,
            "tile_resolution": tn,
            "layers": layers.iter().map(|l| json!({
                "name": l.name.replace(' ', ""), "key": l.key, "tile_m": l.tile_m,
                "albedo": format!("Landscape/Layers/{}_albedo.png", l.key), "normal": format!("Landscape/Layers/{}_normal.png", l.key),
                "albedo_name": format!("{}_albedo", l.key), "normal_name": format!("{}_normal", l.key),
                "mask": format!("Landscape/Layers/{}_mask.png", l.key), "mask_name": format!("{}_mask", l.key),
                "weights_raw": format!("weights/{}.r8", l.key),
            })).collect::<Vec<_>>(),
        },
        "models": models.iter().zip(&inst).map(|(m, list)| json!({
            "name": s.assets[m.asset].name,
            "safe_name": m.safe,
            "file": format!("Models/{}", m.file),
            "generated": m.generated,
            "variants": m.variants.iter().map(|v| json!({ "node": v.node, "lods": v.lods, "collision": v.collision })).collect::<Vec<_>>(),
            "lod_end_m": m.lod_end,
            "cull_m": m.lod_end.last().copied().unwrap_or(0.0),
            "shadows": m.shadows,
            "collider": m.collider.name(),
            "lightmap": m.collider == core::Collider::Mesh && s.assets[m.asset].category != crate::objects::Category::Trees,
            "physics": super::collision::physics_json(s.assets[m.asset].surface),
            "instances": list.iter().flat_map(|i| core::inst_row(i, i.pos, i.rot)).collect::<Vec<f64>>(),
            "stretch": core::stretch_rows(list),
        })).collect::<Vec<_>>(),
        "water": water,
        "roads": roads,
        "caves": caves_glb,
        "dungeons": dungeons_json,
        "ways_in": ways_json,
        "fires": crate::flames::fires_json(&s.fires),
        "flames": crate::dungeons::export::flame_sheets_json(),
        "portal_sheets": sheets,
        // a NavMeshBoundsVolume over the whole world: Unreal builds its navmesh from the collision
        "navmesh_bounds": if o.navmesh { json!({ "center": [0.0, (t.lo + t.hi) * 0.5, 0.0], "size": [t.size, (t.hi - t.lo) + 200.0, t.size] }) } else { Value::Null },
        "cave_lights": super::lights::cave_lights(&caves.json).iter().map(|l| json!({ "position": l.pos, "color": l.color, "lumens": l.physical_lumens(), "range": l.range })).collect::<Vec<_>>(),
        "light": {
            "sun_direction": light.sun_dir.to_array(),
            "sun_color": light.sun_color,
            "intensity_lux": (10.0 * light.sun_strength).max(0.05),
            "fog": light.fog,
            "fog_density": (light.fog_density_exp(3000.0) * 10.0).clamp(0.000_05, 0.05),
            "fog_color": light.fog_color,
            "clouds": light.clouds,
            "exposure": light.exposure,
            "contrast": light.contrast,
            "saturation": light.saturation,
            "bloom": light.bloom * 4.0,
            "vignette": light.vignette,
        },
        // instances are placed as one HISM actor per model variant and world cell of this size
        "cell_m": super::tiles::CELL_M,
        "markers": markers.iter().map(|m| json!({ "kind": m.kind, "id": m.id, "name": m.name, "type": m.sub, "position": m.pos, "yaw": m.yaw, "radius": m.radius })).collect::<Vec<_>>(),
    });
    ctx.write_json("Data/worldbuilder_unreal.json", &doc)?;
    ctx.write("import_world.py", SCRIPT.as_bytes())?;
    ctx.write("CREDITS.md", core::credits(s, &layers).as_bytes())?;
    let (loc, scale, comps) = landscape_values(&t);
    let counts = core::counts(s, &models, &inst);
    ctx.write_json(
        "manifest.json",
        &json!({
            "format": "world-builder-engine-export/1",
            "target": "unreal5",
            "world": s.name,
            "terrain": t.meta(),
            "navmesh": navmesh,
            "landscape": { "resolution": n, "location_cm": loc, "scale": scale, "section_quads": 63, "sections_per_component": 2, "components_per_side": comps, "tiles_per_side": tiles, "tile_resolution": tn },
            "lighting": light.json(s),
            "counts": counts,
        }),
    )?;
    let layer_rows: String = layers.iter().map(|l| format!("| {} | `Landscape/weights_ue/{}.png` (generic orientation: `weights/{}.png`) | {:.2} m |\n", l.name.replace(' ', ""), l.key, l.key, l.tile_m)).collect();
    let readme = README
        .replace("{name}", &s.name)
        .replace("{n}", &n.to_string())
        .replace("{comps}", &comps.to_string())
        .replace("{lx}", &format!("{:.1}", loc[0]))
        .replace("{ly}", &format!("{:.1}", loc[1]))
        .replace("{lz}", &format!("{:.1}", loc[2]))
        .replace("{sx}", &format!("{:.4}", scale[0]))
        .replace("{sy}", &format!("{:.4}", scale[1]))
        .replace("{sz}", &format!("{:.4}", scale[2]))
        .replace("{lo}", &format!("{:.2}", t.lo))
        .replace("{hi}", &format!("{:.2}", t.hi))
        .replace("{size}", &format!("{:.0}", t.size))
        .replace("{layers}", &layer_rows)
        .replace("{world}", &core::safe_name(&s.name));
    ctx.write("README.md", readme.as_bytes())?;
    Ok(
        json!({ "landscape_resolution": n, "landscape_location_cm": loc, "landscape_scale": scale, "components_per_side": comps, "layers": layers.len(), "markers": markers.len(), "counts": counts }),
    )
}

const README: &str = r#"# {name} — Unreal Engine 5 export

Exported from Skyfold Studio for **Unreal Engine 5.4 or newer**. The import script is written
against Unreal's documented Python API but was not run in Unreal when this export was made: it
works defensively and logs every step (Output Log, filter `WorldBuilder`).

## 1. Run the import script

1. *Edit ▸ Plugins*: enable **Python Editor Script Plugin** (and keep **Interchange** / glTF
   import on, the default). Restart the editor.
2. Open (or create) the level to fill, then *Tools ▸ Execute Python Script…* ▸ `import_world.py`
   (or in the Output Log, Python mode: `py "<this folder>/import_world.py"`).

It imports everything into `/Game/WorldBuilder/{world}/` and:

- measures how the glTF importer maps axes (`Data/axis_probe.glb`) and converts every position
  and rotation accordingly;
- imports every model GLB and places all trees, plants, rocks and props as **hierarchical
  instanced static meshes** (one actor per model variant and 512 m world cell, with cull
  distances; the variant's LOD meshes — packed Poly Haven / Skyborn / hand-painted / imported
  models included — are folded into LOD0's LOD chain when the API allows);
- sets up **collision** from the export's proxies: a 26-sided hull for props, rocks and logs,
  each building's simplified collision mesh (`<variant>_COL`: floors and stairs within a few
  centimetres) as complex collision used as simple, trunk prisms for trees, none for plants;
  a **PhysicalMaterial** per footstep surface; lightmap UVs on for buildings;
- builds **SkyAtmosphere**, the **sun** (DirectionalLight, atmosphere sun light) at the world's
  time of day, a real-time **SkyLight**, **ExponentialHeightFog** matching the world's haze,
  **VolumetricCloud** when the world has clouds and an unbound **PostProcessVolume** with the
  world's grade (exposure, contrast, saturation, bloom, vignette);
- creates `M_WB_Water` and places the ocean plane at sea level and the lake / river / waterfall
  surfaces with per-body instances, and **water volumes** (PhysicsVolumes with Water Volume on:
  characters swim) for lakes, river stretches and the ocean; places the **roads** (no collision)
  the **dungeons** (a level each, `Dungeons/L_<name>`, made next to your level once your level
  is saved). Each dungeon level has:
  - its architecture with complex collision;
  - its lights in lumens (hazy ones scatter in the volumetric fog);
  - its objects, its fog and fill light;
  - a PlayerStart, and a TriggerBox back to your level.

  Your level gets the portal sheets and a TriggerBox at each way in. The triggers are tagged
  `wb_portal`, `wb_level=<level>` and `wb_arrive=…`; travel is your game's (OpenLevel the
  `wb_level` tag's level on overlap). Also `Dungeons/<folder>/dungeon.json` for each dungeon's
  lights, mood, rooms and doorways;
  and the **caves** (rock with complex collision, crystal point lights; caves, mines,
  sinkholes, rock arches as static meshes — after the landscape is in, open its cave mouths with
  Landscape mode › Sculpt › Visibility over the white cells of `Caves/holes.png` (one pixel per
  landscape quad, north-west corner first) and give the landscape material an opacity mask);
- spawns a **PlayerStart** and **TargetPoint** markers (POIs, NPCs, spawn regions, resource
  nodes, fishing spots; details in the actor tags and `Data/gameplay.json`);
- places a **NavMeshBoundsVolume** over the world: Unreal builds its navmesh from the collision
  (*Build ▸ Build Paths*; for big worlds turn on World Partitioned navmesh in the project's
  navigation settings). `Data/Navmesh/` has Skyfold Studio's own navmesh for game servers;
- creates the landscape material `M_WB_Landscape` (weight-blended layers with the world's
  photoscan textures at their real tiling, roughness and AO from their mask maps) and writes
  `Landscape/heightmap_ue.png` plus `Landscape/weights_ue/*.png` oriented for this editor's axes.

## 2. Import the landscape (manual)

Landscape mode (Shift+2) ▸ **Manage ▸ New ▸ Import from File**:

| Setting | Value |
|---|---|
| Heightmap File | `Landscape/heightmap_ue.png` ({n} × {n}, 16-bit) |
| Material | `/Game/WorldBuilder/{world}/Materials/M_WB_Landscape` |
| Section Size | 63 × 63 quads |
| Sections Per Component | 2 × 2 |
| Number of Components | {comps} × {comps} |
| Location | X {lx}, Y {ly}, Z {lz} |
| Rotation | 0, 0, 0 |
| Scale | X {sx}, Y {sy}, Z {sz} |

Layers (weight-blended; *Create Layer Info* for each, then pick its file):

| Layer | Weightmap | Tiling |
|---|---|---|
{layers}
**Big worlds** (more than 4033 samples a side) come as World Partition tiles of 2017 samples
sharing their edges: `Landscape/Tiles/heightmap_x<i>_y<j>.png` (and `<layer>_x<i>_y<j>.png`
weightmaps). In a World Partition level pick `heightmap_x0_y0.png` in *Import from File*:
Unreal finds the other tiles by their names; use the same Location, Scale and material.

The values above assume Unreal's usual glTF mapping (X = east, Y = south, Z = up); the script
prints the same table for the axes it actually measured, and `heightmap_ue.png` always matches
them. How they were computed: the world is {size} m across with {n} samples per side, so X / Y
scale = metres per sample × 100. Heights span {lo} … {hi} m in 16 bits; Unreal maps a sample `v`
to `(v − 32768) × ScaleZ / 128` cm above the actor, so `ScaleZ = 12800 × (max − min) / 65535`
and `Z = 100 × min + 32768 × ScaleZ / 128`.

## Files

`Landscape/` heightmap (`heightmap.png` 16-bit, `heightmap.r16` little-endian, north-west corner
first), weightmaps and layer textures; `Models/` one GLB per asset (variant nodes `<asset>_vN`
with LOD children `<asset>_vN_LODk`); `Water/`, `Roads/`; `Data/worldbuilder_unreal.json`
(everything the script reads), `gameplay.json` (the documented server format,
`docs/server-data.md`), `lighting.json`, `Navmesh/`; `Textures/` the model textures, written once
and referred to by the GLBs; `manifest.json`; `CREDITS.md`.
"#;
