//! Unreal Engine 5 target: a folder with the landscape heightmap and weightmaps (Unreal landscape
//! sizes, the exact Location / Scale for this world in the README), GLB models, water and road
//! meshes, and `import_world.py` (Python Editor Script Plugin, UE 5.4+) that imports the meshes,
//! places hierarchical instanced static meshes, builds sky / sun / fog / post process, water,
//! roads and markers, creates the landscape material and orients the landscape files. Unreal's
//! Python API can't create a landscape from heightmap files (only overwrite an existing one's
//! heights from a render target): the plugin below does, else it's a manual import (and for now
//! always for World Partition tiled big worlds, `tiles`).
//!
//! The Skyfold Studio plugin (`Plugins/SkyfoldStudio`, C++ source the project compiles) creates
//! the landscape from those files and carries the runtime: zone / biome weather and atmosphere,
//! the explorer (the editor's walk mode), portals and fires. The script is defensive (every step
//! logs and carries on, and without the plugin it builds the static world). It measures how the
//! glTF importer maps axes with a probe mesh (`Data/axis_probe.glb`) instead of assuming it.

use bevy::math::Vec3;
use serde_json::{Value, json};

use super::core::{self, Ctx, Snapshot, TerrainOut};
use super::generic::clean;
use super::glb::{Glb, Material, Prim};

const SCRIPT: &str = include_str!("unreal/import_world.py");

/// The Skyfold Studio plugin (source; Unreal compiles it with the project): what the import
/// script can't do from Python, such as creating the Landscape.
const PLUGIN: &[(&str, &str)] = &[
    ("SkyfoldStudio.uplugin", include_str!("unreal/Plugin/SkyfoldStudio/SkyfoldStudio.uplugin")),
    ("Source/SkyfoldStudioEditor/SkyfoldStudioEditor.Build.cs", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudioEditor/SkyfoldStudioEditor.Build.cs")),
    ("Source/SkyfoldStudioEditor/Public/SkyfoldStudioLibrary.h", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudioEditor/Public/SkyfoldStudioLibrary.h")),
    ("Source/SkyfoldStudioEditor/Private/SkyfoldStudioLibrary.cpp", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudioEditor/Private/SkyfoldStudioLibrary.cpp")),
    ("Source/SkyfoldStudioEditor/Private/SkyfoldStudioEditorModule.cpp", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudioEditor/Private/SkyfoldStudioEditorModule.cpp")),    ("Source/SkyfoldStudio/SkyfoldStudio.Build.cs", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudio/SkyfoldStudio.Build.cs")),
    ("Source/SkyfoldStudio/Public/SkyfoldAtmosphere.h", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudio/Public/SkyfoldAtmosphere.h")),
    ("Source/SkyfoldStudio/Public/SkyfoldCharacter.h", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudio/Public/SkyfoldCharacter.h")),
    ("Source/SkyfoldStudio/Public/SkyfoldPortal.h", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudio/Public/SkyfoldPortal.h")),
    ("Source/SkyfoldStudio/Private/SkyfoldStudioModule.cpp", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudio/Private/SkyfoldStudioModule.cpp")),
    ("Source/SkyfoldStudio/Private/SkyfoldAtmosphere.cpp", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudio/Private/SkyfoldAtmosphere.cpp")),
    ("Source/SkyfoldStudio/Private/SkyfoldCharacter.cpp", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudio/Private/SkyfoldCharacter.cpp")),
    ("Source/SkyfoldStudio/Private/SkyfoldPortal.cpp", include_str!("unreal/Plugin/SkyfoldStudio/Source/SkyfoldStudio/Private/SkyfoldPortal.cpp")),
];

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

/// The weather and play runtime's data (the plugin's `ASkyfoldAtmosphere`, as Godot's
/// `wb_atmosphere.gd`): the zone and biome maps (one byte per terrain vertex, rows south), the
/// weathers, zones (weather, clouds, wind, haze, mood, play rules) and biomes, the world's
/// weather and wind, and the fog colours under Unreal's exposure.
fn write_atmosphere(s: &Snapshot, ctx: &Ctx, light: &core::LightOut, exposure: f32) -> Result<(), String> {
    let t = &s.terrain;
    let l = &s.lighting;
    let lk = &s.look;
    ctx.write("Data/zones.r8", &t.zones)?;
    ctx.write("Data/biomes.r8", &t.biomes)?;
    let (weathers, zones, biomes) = super::godot::weather_tables(s);
    let id = |w: crate::world::Weather| format!("{w:?}").to_lowercase();
    ctx.write_json(
        "Data/atmosphere.json",
        &json!({
            "grid": { "vertices": t.n(), "cell": t.cell, "zones": "Data/zones.r8", "biomes": "Data/biomes.r8" },
            "sea_level": t.sea_level,
            "fade_seconds": crate::look::ZONE_FADE,
            "weathers": weathers,
            "world": {
                "clouds": l.cloud_coverage,
                "cloud_speed": l.cloud_speed,
                "precipitation": l.precipitation,
                "lightning": l.rain.lightning,
                "override": l.weather_override.map(id),
                "haze_enabled": light.haze_enabled,
                "haze": l.haze,
                "night": light.night,
                "wind": { "dir": [s.wind[0], s.wind[1]], "speed": s.wind[2], "gustiness": s.wind[3] },
            },
            "look": { "haze": lk.haze, "color_haze": lk.color_haze, "haze_distance": lk.haze_distance, "air": lk.air, "sunbeams": lk.sunbeams, "mist": lk.mist },
            "fog": super::godot::atmosphere_fog(s, light, exposure),
            "zones": zones,
            "biomes": biomes,
        }),
    )
}

/// One tuft of the editor's meadow grass (`grass::build_patch`'s recipe: blades leaning out from
/// their roots, curving over, golden-angle azimuths) at the world's blade height, for Unreal's
/// landscape grass. UV: x across the blade, y from the tip (0) to the root (1).
fn grass_clump(s: &Snapshot) -> Vec<u8> {
    let mut rng = crate::noise::Rng::new(0x5EED_6A55);
    let scale = s.grass_height.clamp(0.3, 2.5);
    let (mut pos, mut nrm, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
    for k in 0..18u32 {
        let az = k as f32 * 2.399_96 + (rng.f32() - 0.5) * 0.6;
        let root = 0.13 * rng.f32().sqrt();
        let lean = 0.12 + 0.45 * rng.f32();
        let curve = 0.15 + 0.35 * rng.f32();
        let h = (0.3 + 0.22 * rng.f32()) * scale;
        let w = 0.016 + 0.008 * rng.f32();
        let (dx, dz) = (az.cos(), az.sin());
        // across the blade, square to its lean
        let (sx, sz) = (-dz, dx);
        let base = pos.len() as u32;
        for t in [0.0f32, 0.3, 0.62, 1.0] {
            let out = (lean.sin() * t + curve * t * t) * h + root;
            let up = (lean.cos() * t - 0.35 * curve * t * t) * h;
            let half = w * (1.0 - 0.85 * t) * 0.5;
            for side in [-1.0f32, 1.0] {
                pos.push([dx * out + sx * half * side, up, dz * out + sz * half * side]);
                // (lit from above, as the editor lights its blades)
                nrm.push([0.0, 1.0, 0.0]);
                uv.push([(side + 1.0) * 0.5, 1.0 - t]);
            }
        }
        for r in 0..3u32 {
            let a = base + r * 2;
            idx.extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
        }
    }
    let mut g = super::glb::Glb::new();
    let mut m = super::glb::Material::new("grass_blade");
    m.double_sided = true;
    let mat = g.material(&m);
    let mesh = g.mesh(
        "grass_clump",
        &[super::glb::Prim { positions: &pos, normals: Some(&nrm), uvs: Some(&uv), indices: &idx, material: Some(mat), ..Default::default() }],
    );
    let node = g.node("grass_clump", mesh, vec![]);
    g.finish(&[node])
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
            "Look",
            "import_world.py",
            "Plugins",
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
            let (ext, ins) = crate::water::water_optics(st);
            json!({ "shallow_linear": [c.red, c.green, c.blue], "transparency": st.transparency, "roughness": st.roughness, "clarity": st.clarity, "extinct": ext.to_array(), "inscat": ins.to_array(), "shore_foam": st.shore_foam })
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
            &json!({ "caves": caves.json, "holes": caves.holes, "holes_grid": s.terrain.res }),
        )?;
        caves_glb = "Caves/caves.glb";
    }
    if let Some(b) = &caves.holes_png {
        ctx.write("Caves/holes.png", b)?;
    }
    // meadow grass: the editor's grass map (density, dryness, tint, height per terrain vertex,
    // rows south) and one tuft for Unreal's landscape grass
    let mut grass = Value::Null;
    if o.plants
        && let Some(g) = core::grass(s, s.terrain.n())
    {
        let gn = s.terrain.n() as u32;
        let px: Vec<u8> = g.into_iter().flatten().collect();
        ctx.write("Data/grass.png", &core::png_rgba(&image::RgbaImage::from_raw(gn, gn, px).ok_or("grass")?))?;
        ctx.write("Models/grass_clump.glb", &grass_clump(s))?;
        // (base, lush tip, dry tip), linear, as the Godot export's grass
        let colours: [[f32; 3]; 3] = if s.painted {
            [[0.12, 0.2, 0.04], [0.38, 0.54, 0.1], [0.66, 0.5, 0.16]]
        } else {
            [[0.085, 0.115, 0.03], [0.3, 0.42, 0.1], [0.55, 0.45, 0.2]]
        };
        grass = json!({
            "map": "Data/grass.png", "map_vertices": gn, "clump": "Models/grass_clump.glb",
            "base": colours[0], "tip": colours[1], "dry": colours[2],
            "amount": s.grass_amount, "lushness": s.grass_lushness.clamp(0.0, 1.0),
            "reach_m": crate::grass::RANGES[2] * s.detail.clamp(0.65, 1.5),
        });
    }
    let markers = if o.gameplay { core::markers(s) } else { vec![] };
    if o.gameplay {
        ctx.write_json("Data/gameplay.json", &core::gameplay_json(s))?;
    }
    let navmesh = super::nav::write(ctx, &s.navmesh, "Data/Navmesh")?;
    let light = core::lighting(s);
    ctx.write_json("Data/lighting.json", &light.json(s))?;
    // the look: exposure with the editor's night opening, the haze as the Godot and Unity exports
    // calibrate it, the post-tonemap grade as Unreal's 256 x 16 colour grading LUT
    let night = super::godot::Night::of(s, &light);
    let ev = light.exposure + night.ev;
    let haze = super::godot::Haze::of(s, &light, 2f32.powf(ev));
    write_atmosphere(s, ctx, &light, 2f32.powf(ev))?;
    ctx.write(
        "Look/grade_lut.png",
        &core::png_rgb(&super::godot::look::grade_lut_sized(&s.look, s.lighting.look.user_lut(), 16)),
    )?;
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
            "category": format!("{:?}", s.assets[m.asset].category),
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
            // weapons: imported as a skeletal mesh (its moving nodes and sockets become bones)
            // with an AnimSequence per clip, placed as SkeletalMeshActors
            "weapon": m.weapon,
        })).collect::<Vec<_>>(),
        "water": water,
        "grass": grass,
        "roads": roads,
        "caves": caves_glb,
        "dungeons": dungeons_json,
        "ways_in": ways_json,
        "fires": crate::flames::fires_json(&s.fires),
        "fixture_lights": crate::fixture_lights::lights_json(&s.lights, light.night),
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
            "cloud_base_m": crate::clouds::CLOUD_BASE,
            "exposure": light.exposure,
            "contrast": light.contrast,
            "saturation": light.saturation,
            "bloom": light.bloom * 4.0,
            "vignette": light.vignette,
            "ev": ev,
            "haze_density": if light.haze_enabled { haze.density } else { 0.0 },
            "haze_color": haze.color,
            "lut": "Look/grade_lut.png",
        },
        // instances are placed as one HISM actor per model variant and world cell of this size
        "cell_m": super::tiles::CELL_M,
        "markers": markers.iter().map(|m| json!({ "kind": m.kind, "id": m.id, "name": m.name, "type": m.sub, "position": m.pos, "yaw": m.yaw, "radius": m.radius })).collect::<Vec<_>>(),
    });
    ctx.write_json("Data/worldbuilder_unreal.json", &doc)?;
    ctx.write("import_world.py", SCRIPT.as_bytes())?;
    for (path, text) in PLUGIN {
        ctx.write(&format!("Plugins/SkyfoldStudio/{path}"), text.as_bytes())?;
    }
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

Exported from Skyfold Studio for **Unreal Engine 5.4 or newer** (tested with 5.8): the world,
its nature, props, water, sky and look, its weather and zones, its dungeons and an explorer that
walks it as Skyfold Studio's walk mode does. Everything logs to the Output Log (filter
`WorldBuilder` and `SkyfoldStudio`).

## 1. Add the Skyfold Studio plugin

Copy `Plugins/SkyfoldStudio` into your project's `Plugins` folder and open the project: Unreal
offers to build it (it's source, like any code plugin: Xcode on macOS, Visual Studio on Windows).
It creates the **Landscape** for the import script (Unreal's Python can't) and brings the world's
runtime:

- **SkyfoldAtmosphere**: the zone under the camera sets the weather (a zone's own weather, else
  the painted biome's), its lighting mood and haze, eased in as Skyfold Studio does: rain and
  snow round the camera, weather fog, the sun dimmed and the eye opened under storm cloud,
  lightning, wind, wet ground; the zone's play rules (flying, mounts, rested area, temperature)
  and entry banner for game code (`OnZoneChanged`);
- **SkyfoldGameMode** / **SkyfoldCharacter** (set on the level: press Play): Skyfold Studio's walk
  mode — WASD, mouse look, Shift sprint, Alt walk, C crouch, Space jump, F fly, R ride, swimming
  with breath in the water volumes, the zone banner (`bEnforceZoneRules` keeps the zones' flying
  and riding rules);
- **SkyfoldPortal**: the ways into the dungeons and back (OpenLevel, arriving where the editor's
  way in does); **SkyfoldFire**: the flame flipbooks with their flickering lights.

Without the plugin the script still builds everything static and tells you what's missing (the
landscape is then a manual import: section 3).

## 2. Run the import script

1. *Edit ▸ Plugins*: enable **Python Editor Script Plugin** (and keep **Interchange** / glTF
   import on, the default). Restart the editor.
2. Open (or create) the level to fill, then *Tools ▸ Execute Python Script…* ▸ `import_world.py`
   (or in the Output Log, Python mode: `py "<this folder>/import_world.py"`).

It imports everything into `/Game/WorldBuilder/{world}/` and:

- creates the **Landscape** (heights, the world's ground layers weight-blended with their
  photoscan textures at their real tiling, the cave mouths cut open with the visibility layer);
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
  time of day, a real-time **SkyLight**, **ExponentialHeightFog** with the world's haze (as the
  Godot and Unity exports calibrate it), **VolumetricCloud** when the world has clouds and an
  unbound **PostProcessVolume** with the world's look: manual exposure at the editor's, bloom,
  vignette and its colour grade (saturation, contrast, split toning, lift, foliage hue,
  vibrance, the user's own LUT) as a colour grading LUT (`Look/grade_lut.png`); the **SkyfoldAtmosphere** over them,
  the **fires** and the **SkyfoldGameMode** (with the plugin);
- creates `M_WB_Water` (Single Layer Water with each style's optics from Skyfold Studio: its
  scattering and absorption, so the bed shows through the shallows and deep water takes the depth
  colour) and places the ocean plane at sea level and the lake / river / waterfall surfaces with
  per-body instances, and **water volumes** (PhysicsVolumes with Water Volume on:
  characters swim) for lakes, river stretches and the ocean; places the **roads** (no collision)
  the **dungeons** (a level each, `Dungeons/L_<name>`, made next to your level once your level
  is saved). Each dungeon level has:
  - its architecture with complex collision;
  - its lights in lumens (hazy ones scatter in the volumetric fog);
  - its objects, its fog and fill light;
  - a PlayerStart, and a portal back to your level.

  Your level gets the portal sheets and a portal at each way in: with the plugin SkyfoldPortals
  that open the level and put the explorer where the editor's way in does, else TriggerBoxes for
  your game's travel. Both are tagged `wb_portal`, `wb_level=<level>` and `wb_arrive=…`. Also
  `Dungeons/<folder>/dungeon.json` for each dungeon's lights, mood, rooms and doorways;
  and the **caves** (rock with complex collision, crystal point lights; caves, mines,
  sinkholes, rock arches as static meshes — the landscape's cave mouths are cut open with the
  plugin; by hand, paint Landscape mode › Sculpt › Visibility over the white cells of
  `Caves/holes.png` (one pixel per landscape quad, north-west corner first));
- spawns a **PlayerStart** and **TargetPoint** markers (POIs, NPCs, spawn regions, resource
  nodes, fishing spots; details in the actor tags and `Data/gameplay.json`);
- places a **NavMeshBoundsVolume** over the world: Unreal builds its navmesh from the collision
  (*Build ▸ Build Paths*; for big worlds turn on World Partitioned navmesh in the project's
  navigation settings). `Data/Navmesh/` has Skyfold Studio's own navmesh for game servers;
- creates the landscape material `M_WB_Landscape` (weight-blended layers with the world's
  photoscan textures at their real tiling, roughness and AO from their mask maps, the cave mouths'
  opacity mask) and writes `Landscape/heightmap_ue.png` plus `Landscape/weights_ue/*.png`
  oriented for this editor's axes.

## 3. The landscape by hand (without the plugin, or World Partition tiles)

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
`docs/server-data.md`), `lighting.json`, `atmosphere.json` with `zones.r8` / `biomes.r8` (the
weather runtime's data: weathers, zones with their weather, mood and play rules, biomes; the zone
and biome maps, one byte per terrain vertex, north-west corner first), `Navmesh/`; `Look/` the
colour grading LUT; `Flames/` the flame flipbooks; `Plugins/SkyfoldStudio` the plugin (source);
`Textures/` the model textures, written once and referred to by the GLBs; `manifest.json`;
`CREDITS.md`.
"#;
