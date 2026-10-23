//! Unity 6 target: a `WorldBuilder/` folder to drop into a project's `Assets/`, with the world's
//! data under `WorldBuilder/Worlds/<World>/` and an editor importer (`Tools ▸ Skyfold Studio ▸
//! Import World`, batch entry `WorldBuilder.Importer.ImportFromCommandLine`) that builds the scene:
//! Terrain from the RAW heightmap, TerrainLayers from the photoscan textures, alphamaps, trees
//! (LOD groups, trunk colliders), grass and small plants as terrain details, rocks / props /
//! buildings as prefabs, water, roads, sun / sky / fog and markers. Models are GLB and need
//! glTFast. Everything is converted to Unity's space here (glTFast's convention: mirror X).

use bevy::prelude::*;
use serde_json::{Value, json};

use super::core::{self, Ctx, Inst, ModelOut, Snapshot, TerrainOut};
use super::generic::clean;
use crate::objects::Category;
use crate::procgen::meshes::AssetKind;

const IMPORTER_CS: &str = include_str!("unity/WorldBuilderImporter.cs");
const MARKER_CS: &str = include_str!("unity/WorldBuilderMarker.cs");
const CAMERA_CS: &str = include_str!("unity/WorldBuilderFlyCamera.cs");
const WATER_CS: &str = include_str!("unity/WorldBuilderWaterVolume.cs");
const NAVMESH_CS: &str = include_str!("unity/WorldBuilderNavMesh.cs");
const PORTAL_CS: &str = include_str!("unity/WorldBuilderPortal.cs");
const FLICKER_CS: &str = include_str!("unity/WorldBuilderFlicker.cs");
const PIECE_CS: &str = include_str!("unity/WorldBuilderPiece.cs");
const WATER_SHADER: &str = include_str!("unity/WorldBuilderWater.shader");
const SKY_SHADER: &str = include_str!("unity/WorldBuilderSky.shader");

/// glTFast's conversion: X is mirrored.
fn pos(p: [f32; 3]) -> [f32; 3] {
    [-p[0], p[1], p[2]]
}

fn rot(q: [f32; 4]) -> [f32; 4] {
    [q[0], -q[1], -q[2], q[3]]
}

/// Bilinear sample of a per-vertex byte map at a world position.
fn sample(t: &TerrainOut, map: &[u8], x: f32, z: f32) -> f32 {
    let n = t.n;
    let gx = ((x + t.half) / t.cell).clamp(0.0, (n - 1) as f32);
    let gz = ((z + t.half) / t.cell).clamp(0.0, (n - 1) as f32);
    let (i0, j0) = (
        (gx.floor() as usize).min(n - 2),
        (gz.floor() as usize).min(n - 2),
    );
    let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
    let v = |i: usize, j: usize| map[j * n + i] as f32;
    (v(i0, j0) * (1.0 - fx) + v(i0 + 1, j0) * fx) * (1.0 - fz)
        + (v(i0, j0 + 1) * (1.0 - fx) + v(i0 + 1, j0 + 1) * fx) * fz
}

/// The world's look in URP terms (`WorldBuilderImporter.BuildLook`), from the Godot export's
/// side-by-side calibrations: the same haze, sky colours and night exposure.
struct UnityLook {
    /// URP `TonemappingMode`
    tonemap: &'static str,
    /// URP post exposure (EV)
    ev: f32,
    bloom: f32,
    vignette: f32,
    haze: super::godot::Haze,
    /// sky radiance (sRGB-coded) for the sky shader and the water's reflections
    sky_top: [f32; 3],
    sky_horizon: [f32; 3],
    /// the key light and the sky's irradiance in the editor's display units (the water optics)
    water_sun: [f32; 3],
    water_sky: [f32; 3],
}

impl UnityLook {
    /// Stops URP's tonemappers need to land at the editor's brightness on sunlit ground.
    const CALIBRATION_EV: f32 = 0.0;
    /// URP's Neutral / ACES curves render the Godot-calibrated sky darker: measured side by side.
    const SKY_GAIN: f32 = 1.8;

    fn of(s: &Snapshot, light: &core::LightOut) -> Self {
        use crate::look::ToneCurve;
        let night = super::godot::Night::of(s, light);
        let wl = super::godot::water_light(s, light);
        let ev = light.exposure + night.ev + Self::CALIBRATION_EV;
        let (top, horizon) = super::godot::sky_colors(light, s.look.haze * light.haze);
        // the sky's radiance as the Godot sky shader draws it (its energy under the exposure)
        // and URP's tone curves need
        let energy = night.sky_energy() * Self::SKY_GAIN;
        let srgb = |c: Vec3| (c * energy).to_array().map(|v| crate::terrain::textures::lin_to_srgb(v) as f32 / 255.0);
        UnityLook {
            tonemap: match s.look.tone {
                ToneCurve::Punchy | ToneCurve::Classic => "ACES",
                ToneCurve::Filmic | ToneCurve::Soft => "Neutral",
            },
            ev,
            bloom: light.bloom * 1.5,
            vignette: light.vignette * 0.5,
            haze: super::godot::Haze::of(s, light, 2f32.powf(ev)),
            sky_top: srgb(top),
            sky_horizon: srgb(horizon),
            water_sun: wl.1.to_array(),
            water_sky: wl.2.to_array(),
        }
    }
}

/// Plants scattered as terrain details (lily pads float on water, so they stay objects).
fn is_detail(s: &Snapshot, m: &ModelOut) -> bool {
    let a = &s.assets[m.asset];
    a.category == Category::Plants && m.generated && a.kind != Some(AssetKind::LilyPads)
}

pub fn export(s: &Snapshot, ctx: &Ctx) -> Result<Value, String> {
    let world = core::safe_name(&s.name);
    let base = format!("WorldBuilder/Worlds/{world}");
    clean(
        ctx,
        &[
            &base,
            "WorldBuilder/Editor",
            "WorldBuilder/Runtime",
            "WorldBuilder/Shaders",
            "Server",
            "README.md",
            "CREDITS.md",
            "manifest.json",
            "Packages-manifest-snippet.json",
        ],
    );
    let o = &ctx.opts;
    if o.shared_textures {
        ctx.shared.enable(&format!("{base}/Textures"));
    }
    let n = o.target.terrain_vertices(o.terrain);
    ctx.stage("Terrain heights and weights", 0.02)?;
    let t = core::terrain(s, n);
    // terrain tiles (one up to 4097 samples; 2049-sample tiles sharing their edges beyond):
    // tile (x, z) counts along Unity +X and +Z
    let (tiles, tn) = super::tiles::split(o.target, n);
    let q = tn - 1;
    let tile_size = t.size / tiles as f32;
    // heights: rows run along Unity +Z (south), columns along Unity +X (= world west)
    let h16 = t.heights16();
    let mut hu = Vec::with_capacity(n * n);
    for j in 0..n {
        for xi in 0..n {
            hu.push(h16[j * n + (n - 1 - xi)]);
        }
    }
    for tj in 0..tiles {
        for ti in 0..tiles {
            let tile = super::tiles::block(&hu, n, ti * q, tj * q, tn, tn);
            ctx.write(
                &format!("{base}/Terrain/heightmap_{ti}_{tj}.bytes"),
                bytemuck::cast_slice(&tile),
            )?;
        }
    }
    drop(hu);
    // alphamaps at texel centres: [z][x][layer]
    let a = n - 1;
    let texel = t.size / a as f32;
    let rows: Vec<Vec<u8>> = core::par_map(a, |az| {
        let mut row = Vec::with_capacity(a * t.used.len());
        for ax in 0..a {
            let (x, z) = (
                -(-t.half + (ax as f32 + 0.5) * texel),
                -t.half + (az as f32 + 0.5) * texel,
            );
            for w in &t.weights {
                row.push(sample(&t, w, x, z).round() as u8);
            }
        }
        row
    });
    let alpha = rows.concat();
    let lc = t.used.len();
    for tj in 0..tiles {
        for ti in 0..tiles {
            let tile = super::tiles::block(&alpha, a * lc, ti * q * lc, tj * q, q * lc, q);
            ctx.write(&format!("{base}/Terrain/alphamaps_{ti}_{tj}.bytes"), &tile)?;
        }
    }
    drop(alpha);
    ctx.stage("Terrain layer textures", 0.08)?;
    let layers = core::layer_textures(s, &t.used, o.texture_size)?;
    for l in &layers {
        ctx.write(
            &format!("{base}/Terrain/Layers/{}_albedo.png", l.key),
            &core::png_rgb(&l.albedo),
        )?;
        ctx.write(
            &format!("{base}/Terrain/Layers/{}_normal.png", l.key),
            &core::png_rgb(&l.normal),
        )?;
        ctx.write(
            &format!("{base}/Terrain/Layers/{}_mask.png", l.key),
            &core::png_rgba(&l.mask),
        )?;
    }
    let layer_json: Vec<Value> = layers
        .iter()
        .map(|l| {
            let smooth = l.mask.pixels().map(|p| p.0[3] as f32).sum::<f32>()
                / (l.mask.width() * l.mask.height()).max(1) as f32
                / 255.0;
            json!({
                "name": l.name, "key": l.key,
                "albedo": format!("Terrain/Layers/{}_albedo.png", l.key),
                "normal": format!("Terrain/Layers/{}_normal.png", l.key),
                "mask": format!("Terrain/Layers/{}_mask.png", l.key),
                "tile": l.tile_m, "smoothness": smooth,
            })
        })
        .collect();

    let models = core::models(s, ctx, &format!("{base}/Models"), 0.15, 0.8)?;
    let inst = core::instances(s, &models);
    ctx.stage("Trees, details and props", 0.82)?;
    let mut model_json = vec![];
    let mut details = vec![];
    let mut detail_maps: Vec<Vec<u8>> = vec![];
    // detail cells: at most 1024 per side and tile (4 m on a 4 km world)
    let dres_tile = q.min(1024);
    let dres = dres_tile * tiles;
    let cell = t.size / dres as f32;
    let cell_of = |p: [f32; 3]| -> Option<usize> {
        // Unity X' = -x: detail column from the west edge of Unity space
        let u = pos(p);
        let (dx, dz) = (
            ((u[0] + t.half) / cell).floor(),
            ((u[2] + t.half) / cell).floor(),
        );
        (dx >= 0.0 && dz >= 0.0 && (dx as usize) < dres && (dz as usize) < dres)
            .then(|| dz as usize * dres + dx as usize)
    };
    for (m, list) in models.iter().zip(&inst) {
        if list.is_empty() {
            continue;
        }
        let asset = &s.assets[m.asset];
        let detail = is_detail(s, m);
        if detail {
            for v in 0..m.variants.len() {
                let mine: Vec<&Inst> = list.iter().filter(|i| i.variant as usize == v).collect();
                if mine.is_empty() {
                    continue;
                }
                let mut map = vec![0u8; dres * dres];
                for i in &mine {
                    if let Some(c) = cell_of(i.pos) {
                        map[c] = map[c].saturating_add(1);
                    }
                }
                let (lo, hi) = mine.iter().fold((f32::MAX, f32::MIN), |(l, h), i| {
                    (l.min(i.scale), h.max(i.scale))
                });
                details.push(json!({ "model": m.safe, "variant": v, "minScale": lo, "maxScale": hi, "align": if asset.kind == Some(AssetKind::Fern) { 1.0 } else { 0.0 } }));
                detail_maps.push(map);
            }
        }
        model_json.push(model_entry(s, m, list, detail));
    }
    // models only the dungeons use: prefabs without world instances
    if let Ok(list) = super::dungeons::list(s) {
        let used: std::collections::HashSet<&str> = list
            .iter()
            .flat_map(|d| d.objects.iter().map(|o| o.asset.as_str()))
            .collect();
        for (m, l) in models.iter().zip(&inst) {
            if l.is_empty() && used.contains(s.assets[m.asset].id.as_str()) {
                model_json.push(model_entry(s, m, &[], false));
            }
        }
    }
    // grass tufts from the grass map
    let mut grass_tufts = 0usize;
    if o.plants
        && let Some(g) = core::grass(s, n)
        && let Some((glb, variants)) = core::grass_tufts(o.texture_size)
    {
        let file = "Models/grass_tufts.glb";
        ctx.write_glb(&format!("{base}/{file}"), glb)?;
        let density: Vec<u8> = g.iter().map(|p| p[0]).collect();
        let k = variants.len().max(1);
        let base_maps = detail_maps.len();
        for v in 0..k {
            details.push(json!({ "model": "grass_tufts", "variant": v, "minScale": 0.8, "maxScale": 1.3, "align": 0.0 }));
            detail_maps.push(vec![0u8; dres * dres]);
        }
        // about one tuft per m² in full grass, spread over the variants
        let per_cell = cell * cell * 1.0;
        let mut rng = crate::noise::Rng::new(4242);
        for az in 0..dres {
            for ax in 0..dres {
                let (x, z) = (
                    -(-t.half + (ax as f32 + 0.5) * cell),
                    -t.half + (az as f32 + 0.5) * cell,
                );
                let d = sample(&t, &density, x, z) / 255.0;
                let want = d * per_cell;
                let mut count = want.floor() as u32 + (rng.f32() < want.fract()) as u32;
                while count > 0 {
                    let v = rng.below(k as u32) as usize;
                    let c = &mut detail_maps[base_maps + v][az * dres + ax];
                    *c = c.saturating_add(1);
                    count -= 1;
                    grass_tufts += 1;
                }
            }
        }
        model_json.push(json!({
            "name": "Grass tufts", "safeName": "grass_tufts", "file": file, "category": "Plants", "generated": true,
            "tree": false, "detail": true, "shadows": false, "collider": "none",
            "variants": variants.iter().map(|v| json!({ "node": v.node, "lods": v.lods, "height": v.height, "width": v.width, "trunk": 0.0 })).collect::<Vec<_>>(),
            "lodEnd": [75.0], "data": [],
        }));
    }
    if !detail_maps.is_empty() {
        for tj in 0..tiles {
            for ti in 0..tiles {
                let tile: Vec<u8> = detail_maps
                    .iter()
                    .flat_map(|m| {
                        super::tiles::block(
                            m,
                            dres,
                            ti * dres_tile,
                            tj * dres_tile,
                            dres_tile,
                            dres_tile,
                        )
                    })
                    .collect();
                ctx.write(&format!("{base}/Terrain/details_{ti}_{tj}.bytes"), &tile)?;
            }
        }
    }

    ctx.stage("Water, roads, gameplay", 0.9)?;
    let style = |st: &crate::water::WaterStyle| json!({ "shallow": st.shallow, "deep": st.deep, "clarity": st.clarity, "transparency": st.transparency, "roughness": st.roughness, "shoreFoam": st.shore_foam, "extinct": crate::water::water_optics(st).0.to_array(), "inscat": crate::water::water_optics(st).1.to_array() });
    let mut water = json!({ "enabled": false });
    if o.water {
        let (glb, bodies) = core::water(s);
        if let Some(b) = &glb {
            ctx.write_glb(&format!("{base}/Water/water.glb"), b.clone())?;
        }
        ctx.write(
            &format!("{base}/Water/water_normal.png"),
            &core::water_normal_png(512),
        )?;
        water = json!({
            "enabled": true,
            "ocean": s.terrain.heights.iter().any(|h| *h < s.terrain.sea_level),
            "seaLevel": s.terrain.sea_level,
            "oceanStyle": style(&s.ocean),
            "mesh": if glb.is_some() { "Water/water.glb" } else { "" },
            "normalMap": "Water/water_normal.png",
            "bodies": bodies.iter().map(|b| json!({ "node": b.node, "kind": b.kind, "name": b.name, "style": style(&b.style) })).collect::<Vec<_>>(),
            // swim volumes in Unity space (mirrored X turns the yaw the other way)
            "volumes": super::collision::water_volumes(s).iter().map(|v| json!({
                "kind": v.kind, "name": v.name, "center": pos(v.center), "size": v.size, "yawDegrees": -v.yaw.to_degrees(), "surface": v.surface,
            })).collect::<Vec<_>>(),
        });
    }
    let mut roads = "";
    if o.roads {
        let (glb, list) = core::roads(s, o.texture_size);
        if let Some(b) = glb {
            ctx.write_glb(&format!("{base}/Roads/roads.glb"), b)?;
            ctx.write_json(&format!("{base}/Data/roads.json"), &core::roads_json(&list))?;
            roads = "Roads/roads.glb";
        }
    }
    // caves: their rock, and the terrain cells they open (Unity columns run west: mirrored)
    let caves = core::caves(s, o.texture_size, "", o.optimize_caves);
    let mut caves_glb = "";
    // the crystals' glow as point lights (colour in sRGB like the sun's)
    let cave_lights: Vec<Value> = super::lights::cave_lights(&caves.json)
        .iter()
        .map(|l| json!({ "position": pos(l.pos), "color": l.color.map(|v| crate::terrain::textures::lin_to_srgb(v) as f32 / 255.0), "intensity": l.intensity(), "range": l.range }))
        .collect();
    if let Some(b) = &caves.glb {
        ctx.write_glb(&format!("{base}/Caves/caves.glb"), b.clone())?;
        ctx.write_json(
            &format!("{base}/Data/caves.json"),
            &json!({ "caves": caves.json, "holes": caves.holes }),
        )?;
        caves_glb = "Caves/caves.glb";
    }
    // terrain holes per tile: (row, column) pairs of tile cells
    let a1 = n - 1;
    let mut unity_holes: Vec<Vec<u32>> = vec![vec![]; tiles * tiles];
    for j in 0..a1 {
        for i in 0..a1 {
            if t.hole(i, j) {
                let (r, c) = (j, a1 - 1 - i);
                let k = (r / q).min(tiles - 1) * tiles + (c / q).min(tiles - 1);
                unity_holes[k].push((r % q) as u32);
                unity_holes[k].push((c % q) as u32);
            }
        }
    }
    let tile_json: Vec<Value> = (0..tiles * tiles)
        .map(|k| {
            let (ti, tj) = (k % tiles, k / tiles);
            json!({
                "x": ti, "z": tj,
                "position": [-t.half + ti as f32 * tile_size, t.lo, -t.half + tj as f32 * tile_size],
                "heightmap": format!("Terrain/heightmap_{ti}_{tj}.bytes"),
                "alphamaps": format!("Terrain/alphamaps_{ti}_{tj}.bytes"),
                "detailMaps": if detail_maps.is_empty() { String::new() } else { format!("Terrain/details_{ti}_{tj}.bytes") },
                "holes": unity_holes[k],
            })
        })
        .collect();
    let markers = if o.gameplay { core::markers(s) } else { vec![] };
    if o.gameplay {
        ctx.write_json(
            &format!("{base}/Data/gameplay.json"),
            &core::gameplay_json(s),
        )?;
    }
    // for game servers (outside the WorldBuilder folder: not for Unity's Assets)
    let navmesh = super::nav::write(ctx, &s.navmesh, "Server/navmesh")?;
    let light = core::lighting(s);
    let ulook = UnityLook::of(s, &light);
    // the look's post-tonemap grade as a URP Color Lookup strip (Unity's rows run bottom up)
    ctx.write(
        &format!("{base}/Look/grade_lut.png"),
        &core::png_rgb(&image::imageops::flip_vertical(&super::godot::look::grade_lut(&s.look))),
    )?;
    ctx.write_json(&format!("{base}/Data/lighting.json"), &light.json(s))?;
    // the shared model textures and what they are (the importer sets normal maps and linear data
    // before it builds anything)
    // dungeons: a scene each (built by the importer), triggers at their ways in
    let (dungeons_json, ways_json, sheets) = dungeons(s, ctx, &models, &base)?;
    super::dungeons::write_flame_sheets(s, &format!("{base}/Flames"), &|rel, b| ctx.write(rel, b))?;
    ctx.shared.write_index(ctx)?;
    let textures: Vec<Value> = ctx
        .shared
        .list()
        .iter()
        .map(|(f, r)| json!({ "file": format!("Textures/{f}"), "role": r.name() }))
        .collect();
    let srgb = |c: [f32; 3]| c.map(|v| crate::terrain::textures::lin_to_srgb(v) as f32 / 255.0);
    let (eye, look) = match s.gameplay.player_start.filter(|_| o.gameplay) {
        Some(p) => {
            let e = Vec3::from_array(p.pos) + Vec3::Y * 1.7;
            (e, e + Vec3::new(p.yaw.cos(), -0.05, -p.yaw.sin()))
        }
        None => s.camera,
    };
    let light_json = json!({
        "hour": light.hour,
        "sunDirection": pos(light.sun_dir.to_array()),
        "sunColor": srgb(light.sun_color),
        "intensity": (light.sun_strength * 1.25).max(0.02),
        "fog": light.fog,
        "fogDensity": light.fog_density_exp2,
        "fogColor": srgb(light.fog_color),
        "haze": light.haze,
        "exposure": light.exposure,
        "contrast": light.contrast,
        "saturation": light.saturation,
        "tonemap": ulook.tonemap,
        "postExposure": ulook.ev,
        "bloom": ulook.bloom,
        "vignette": ulook.vignette,
        "lut": "Look/grade_lut.png",
        "hazeDensity": ulook.haze.density,
        "hazeColor": ulook.haze.color,
        "skyTop": ulook.sky_top,
        "skyHorizon": ulook.sky_horizon,
        "waterSun": ulook.water_sun,
        "waterSky": ulook.water_sky,
    });
    let doc = json!({
        "version": 1,
        "name": s.name,
        "safeName": world,
        "size": t.size,
        "heightMin": t.lo,
        "heightMax": t.hi,
        "seaLevel": t.sea,
        "heightmapResolution": tn,
        "alphamapResolution": q,
        "tilesPerSide": tiles,
        "tileSize": tile_size,
        "tiles": tile_json,
        "cellSize": super::tiles::CELL_M,
        "layers": layer_json,
        "textures": textures,
        "treeDistance": s.draw_distance.clamp(600.0, 5000.0),
        "models": model_json,
        "detailResolution": dres_tile,
        "details": details,
        "water": water,
        "roads": roads,
        "caves": caves_glb,
        "caveLights": cave_lights,
        "dungeons": dungeons_json,
        "waysIn": ways_json,
        "portalSheets": sheets,
        "fires": fires_unity(&s.fires),
        "flames": crate::flames::FireKind::ALL.iter().map(|k| {
            let s = k.sheet();
            json!({ "fire": k.id(), "file": format!("Flames/{}.png", k.id()), "columns": s.columns, "rows": s.rows, "frames": s.frames, "fps": s.fps })
        }).collect::<Vec<_>>(),
        // Unity's own navmesh, baked by the importer from the exported colliders
        "navMesh": { "build": o.navmesh, "voxelSize": 0.3, "agentRadius": 0.4, "agentHeight": 1.8, "maxSlope": 45.0, "stepHeight": 0.45 },
        "light": light_json,
        "markers": markers.iter().map(|m| {
            let d = (-m.yaw.cos(), -m.yaw.sin());
            json!({
                "kind": m.kind, "id": m.id, "name": m.name, "type": m.sub, "position": pos(m.pos),
                "yawDegrees": d.0.atan2(d.1).to_degrees(), "radius": m.radius, "notes": m.notes,
                "polygon": m.polygon.iter().flat_map(|p| [-p[0], p[1]]).collect::<Vec<f32>>(),
            })
        }).collect::<Vec<_>>(),
        "camera": { "position": pos(eye.to_array()), "target": pos(look.to_array()) },
    });
    ctx.write_json(&format!("{base}/worldbuilder.json"), &doc)?;
    ctx.write(
        "WorldBuilder/Editor/WorldBuilderImporter.cs",
        IMPORTER_CS.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Runtime/WorldBuilderMarker.cs",
        MARKER_CS.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Runtime/WorldBuilderFlyCamera.cs",
        CAMERA_CS.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Runtime/WorldBuilderWaterVolume.cs",
        WATER_CS.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Runtime/WorldBuilderNavMesh.cs",
        NAVMESH_CS.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Runtime/WorldBuilderPortal.cs",
        PORTAL_CS.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Runtime/WorldBuilderFlicker.cs",
        FLICKER_CS.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Runtime/WorldBuilderPiece.cs",
        PIECE_CS.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Shaders/WorldBuilderWater.shader",
        WATER_SHADER.as_bytes(),
    )?;
    ctx.write(
        "WorldBuilder/Shaders/WorldBuilderSky.shader",
        SKY_SHADER.as_bytes(),
    )?;
    ctx.write_json("Packages-manifest-snippet.json", &json!({ "dependencies": { "com.unity.cloud.gltfast": "6.20.0", "com.unity.ai.navigation": "2.0.16" } }))?;
    ctx.write("CREDITS.md", core::credits(s, &layers).as_bytes())?;
    let counts = core::counts(s, &models, &inst);
    ctx.write_json(
        "manifest.json",
        &json!({
            "format": "world-builder-engine-export/1",
            "target": "unity6",
            "world": s.name,
            "terrain": t.meta(),
            "unity_terrain": { "position": [-t.half, t.lo, -t.half], "size": [t.size, t.hi - t.lo, t.size], "tiles_per_side": tiles, "tile_size": tile_size, "heightmap_resolution": tn, "alphamap_resolution": q },
            "data": format!("{base}/worldbuilder.json"),
            "lighting": light.json(s),
            "counts": counts,
            "grass_tufts": grass_tufts,
            "navmesh": navmesh,
        }),
    )?;
    ctx.write(
        "README.md",
        README
            .replace("{name}", &s.name)
            .replace("{world}", &world)
            .as_bytes(),
    )?;
    Ok(
        json!({ "terrain_vertices": n, "terrain_tiles": tiles * tiles, "alphamap": q, "layers": layers.len(), "detail_layers": detail_maps.len(), "grass_tufts": grass_tufts, "markers": markers.len(), "counts": counts }),
    )
}

/// The dungeons for `worldbuilder.json` (Unity space): each one's GLB, objects, lights, mood,
/// start and way out (the importer builds a scene per dungeon), the ways in (triggers in the
/// world scene) and the world's portal sheets.
fn dungeons(
    s: &Snapshot,
    ctx: &Ctx,
    models: &[ModelOut],
    base: &str,
) -> Result<(Vec<Value>, Vec<Value>, String), String> {
    if super::dungeons::list(s)?.is_empty() {
        return Ok((vec![], vec![], String::new()));
    }
    let (written, sheets) = super::dungeons::write_all(
        s,
        ctx,
        models,
        &format!("{base}/Dungeons"),
        "",
        &|rel, b| ctx.write_glb(rel, b),
    )?;
    let rel = |p: &str| p.strip_prefix(&format!("{base}/")).unwrap_or(p).to_string();
    let srgb = |c: [f32; 3]| c.map(|v| crate::terrain::textures::lin_to_srgb(v) as f32 / 255.0);
    let eye = Vec3::Y * 1.7;
    let look = |(p, l): (Vec3, Vec3)| json!({ "position": pos((p + eye).to_array()), "look": pos(l.to_array()) });
    let trigger = |t: &crate::dungeons::export::Trigger| json!({ "center": pos(t.center.to_array()), "rotation": rot(t.rot.to_array()), "halfSize": t.half.to_array() });
    let mut dungeons = vec![];
    let mut ways = vec![];
    for w in &written {
        let d = w.d;
        let Some(glb) = &w.glb else { continue };
        ctx.write_json(&format!("{base}/Dungeons/{}/dungeon.json", d.slug), &w.json)?;
        let scene = format!("{}_{}", core::safe_name(&s.name), w.safe);
        let objects: Vec<Value> = models
            .iter()
            .zip(&w.inst)
            .filter(|(_, l)| !l.is_empty())
            .map(|(m, l)| json!({ "safeName": m.safe, "data": l.iter().flat_map(|i| core::inst_row(i, pos(i.pos), rot(i.rot))).collect::<Vec<f64>>(), "stretch": core::stretch_rows(l) }))
            .collect();
        let lights: Vec<Value> = d.map.as_ref().map(|m| {
            m.lights.iter().map(|l| {
                let c = Color::srgb_from_array(m.mood.tinted(l.color)).to_linear();
                json!({
                    "position": pos(l.pos), "direction": pos(l.direction().to_array()), "color": srgb([c.red, c.green, c.blue]),
                    "intensity": ((l.lumens / 300_000.0).sqrt() * 1.5).clamp(0.4, 8.0), "range": l.range,
                    "spot": l.cone > 0.0, "spotAngle": l.cone, "shadows": l.shadows, "flicker": l.flicker, "seed": l.id,
                })
            }).collect()
        }).unwrap_or_default();
        let mood = d.map.as_ref().map(|m| {
            let lin = |c: [f32; 3]| {
                let l = Color::srgb_from_array(c).to_linear();
                [l.red, l.green, l.blue]
            };
            json!({
                "ambient": srgb(lin(m.mood.fill)), "ambientIntensity": (m.mood.fill_strength * 0.6).clamp(0.02, 1.5),
                "fogColor": srgb(lin(m.mood.fog)), "fogDensity": 0.003 + m.mood.fog_density.clamp(0.0, 1.0).powi(2) * 0.045,
                "exposure": m.mood.exposure, "haze": m.mood.haze,
            })
        });
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
            v["scene"] = json!(scene);
            v["via"] = json!(wi.entrance);
            v["arrive"] = look(start);
            v["label"] = json!(format!("{} ({})", d.name, wi.label));
            ways.push(v);
        }
        let sky = d.map.as_ref().map(crate::dungeons::export::sky_json).filter(|v| v["mode"] != "none").map(|v| {
            let f3 = |k: &str| [v[k][0].as_f64().unwrap_or(0.0) as f32, v[k][1].as_f64().unwrap_or(-1.0) as f32, v[k][2].as_f64().unwrap_or(0.0) as f32];
            json!({ "mode": v["mode"], "lightDirection": pos(f3("light_direction")), "intensity": v["intensity"], "color": v["color"] })
        });
        dungeons.push(json!({
            "name": d.name, "safeName": w.safe, "scene": scene, "glb": rel(glb),
            "objects": objects, "lights": lights, "mood": mood, "sky": sky,
            "start": look(start), "exit": exit,
            "pieces": d.map.as_ref().map(|m| super::dungeons::pieces_json(m, &w.safe, &pos, &rot)).unwrap_or_default(),
            "fires": fires_unity(&d.fires),
        }));
    }
    Ok((dungeons, ways, sheets.map(|p| rel(&p)).unwrap_or_default()))
}

/// Fires in Unity space: a particle each at the frame's middle, its size the frame's; ordinary
/// fire keeps the sheet's colours, a magic one takes its light's; the ones with a light of their
/// own, its position, colour, strength and reach.
fn fires_unity(fires: &[crate::flames::Fire]) -> Vec<Value> {
    let srgb = |c: [f32; 3]| c.map(|v| crate::terrain::textures::lin_to_srgb(v) as f32 / 255.0);
    crate::flames::fires_json(fires)
        .iter()
        .map(|f| {
            let n = |v: &Value| v.as_f64().unwrap_or(0.0) as f32;
            // (a stock particle can't narrow as it rises: a spread fire is as wide as its middle)
            let spread = f["spread"].as_f64().unwrap_or(1.0).max(1.0) as f32;
            let (fw, fh) = (n(&f["frame"][0]) / spread * (1.0 + spread) * 0.5, n(&f["frame"][1]));
            let mid = [n(&f["foot"][0]), n(&f["frame_bottom"]) + fh * 0.5, n(&f["foot"][2])];
            let c = [n(&f["color_linear"][0]), n(&f["color_linear"][1]), n(&f["color_linear"][2])];
            let magic = n(&f["magic"]);
            let mx = c[0].max(c[1]).max(c[2]).max(1e-3);
            let tint = [1.0 + (c[0] / mx - 1.0) * magic, 1.0 + (c[1] / mx - 1.0) * magic, 1.0 + (c[2] / mx - 1.0) * magic];
            let lp = [n(&f["light_position"][0]), n(&f["light_position"][1]), n(&f["light_position"][2])];
            json!({
                "fire": f["fire"], "position": pos(mid), "size": [fw, fh], "color": srgb(tint),
                "light": f["light"], "lightPosition": pos(lp), "lightRange": f["light_range"],
                "lightIntensity": ((n(&f["light_lumens"]) / 300_000.0).sqrt() * 1.5).clamp(0.2, 4.0),
            })
        })
        .collect()
}

/// One model for `worldbuilder.json`, its instances flattened in Unity space.
fn model_entry(s: &Snapshot, m: &ModelOut, list: &[Inst], detail: bool) -> Value {
    let a = &s.assets[m.asset];
    let data: Vec<f64> = if detail {
        vec![]
    } else {
        list.iter()
            .flat_map(|i| core::inst_row(i, pos(i.pos), rot(i.rot)))
            .collect()
    };
    let stretch = if detail {
        vec![]
    } else {
        core::stretch_rows(list)
    };
    json!({
        "name": a.name,
        "safeName": m.safe,
        "file": format!("Models/{}", m.file),
        "category": a.category.name(),
        "generated": m.generated,
        "tree": a.category == Category::Trees && m.generated,
        "detail": detail,
        "stretch": stretch,
        "shadows": m.shadows,
        "collider": m.collider.name(),
        // big static pieces (buildings, bridges) bake lightmaps: LOD0 gets a secondary UV set
        "lightmap": m.collider == core::Collider::Mesh && a.category != Category::Trees,
        "physics": super::collision::physics_json(a.surface),
        "variants": m.variants.iter().map(|v| json!({ "node": v.node, "lods": v.lods, "height": v.height, "width": v.width, "trunk": v.trunk, "collision": v.collision })).collect::<Vec<_>>(),
        "lodEnd": m.lod_end,
        "data": data,
    })
}

const README: &str = r#"# {name} — Unity 6 export

Exported from Skyfold Studio for **Unity 6** (built-in render pipeline or URP).

1. Copy the `WorldBuilder` folder into your project's `Assets/` folder.
2. Add **glTFast** (the models are GLB files) and, for a rebakeable navmesh, **AI Navigation**:
   *Window ▸ Package Manager ▸ + ▸ Install package by name…* → `com.unity.cloud.gltfast` and
   `com.unity.ai.navigation`, or add the lines from `Packages-manifest-snippet.json` to
   `Packages/manifest.json`.
3. When Unity has imported everything, run **Tools ▸ Skyfold Studio ▸ Import World**. It builds
   and saves the scene `Assets/WorldBuilder/Worlds/{world}/{world}.unity`.

Batch mode (CI, no editor UI):

    Unity -batchmode -nographics -projectPath <project> \
          -executeMethod WorldBuilder.Importer.ImportFromCommandLine [-wbWorld {world}] [-wbNoNavMesh] -quit -logFile -

The log line `WBIMPORT OK …` lists what was built (`WBIMPORT FAILED …` and exit code 1 on error).

## What the importer builds

- **Terrain** from `Terrain/heightmap_<x>_<z>.bytes` (16-bit, Unity orientation) with the exact
  height range of the world: one tile up to 4097 samples, 2049-sample tiles joined with
  `SetNeighbors` beyond (8–16 km worlds keep their full resolution); a **TerrainLayer** per used
  ground layer (the world's photoscan albedo, normal and mask maps — R metallic, G AO, B height,
  A smoothness — at their real tiling size) and the splat **alphamaps**.
- **Trees** as terrain tree instances: one prefab per species variant with a **LODGroup** (the
  Skyfold Studio LODs and switch distances, ending in a crossed-quad impostor — a picture of the
  tree — that reaches the world's draw distance: Unity's terrain billboards only work for Tree
  Editor trees, never for LOD-group prefabs) and a trunk **CapsuleCollider**.
- **Grass and small plants** as terrain details: photoscanned grass tufts scattered from the
  world's grass map, and every fern, flower and bush at its cell (instanced detail meshes).
- **Rocks, logs, buildings and props** as prefab instances (static), grouped per 512 m world cell
  (`Props/Cell x_z/<model>`). Every model has **levels of detail** (Poly Haven, Skyborn,
  hand-painted and imported models too: simplified from the original within a pixel of error)
  and a **collision proxy** instead of its render mesh: a convex hull (under 255 triangles) for
  props, rocks and logs, a simplified shape for buildings and bridges (floors, stairs and doors
  within a few centimetres), each with a **PhysicsMaterial** for its footstep surface. Buildings
  get **lightmap UVs** (LOD0, `Generated/Meshes`) and contribute to baked GI; the rest use
  light probes.
- **Water**: the ocean plane at sea level and the lake / river / waterfall surfaces with the
  `WorldBuilder/Water` shader (URP and built-in): the editor's water optics — the water column
  from the ground heights (baked to `Generated/Materials/WaterGround.asset`), the bed fading per
  colour channel through each body's water and depth colours, sun and sky light scattered in it,
  Fresnel sky reflection, sun glints, a foam line on the shore, the haze — and **swim volumes** (trigger boxes on the Water
  layer with `WorldBuilderWaterVolume`: lakes, river stretches, the ocean); **roads** as textured
  meshes; **caves** (caves, mines, sinkholes, rock arches) as textured meshes with mesh colliders
  (not their pools and cobwebs) and their glowing crystals as point lights, the terrain cut open
  where they meet it (Terrain holes); `Data/caves.json` lists their chambers and dungeon portals.
- **Dungeons**: each underground dungeon is a scene of its own,
  `<world>_<dungeon>.unity`, next to the world's. Each scene has:
  - its architecture with mesh colliders;
  - its lights (the brightest cast soft shadows, fires flicker with `WorldBuilderFlicker`);
  - its objects, its mood (flat ambient fill, exponential fog);
  - its way out, and a camera walking from its start.

  The world's scene has a `Dungeon Ways In` group: the portal sheets and a `WorldBuilderPortal`
  trigger at each way in. Walking into one loads the dungeon; its way out brings you back in
  front of the way in you came through. The importer adds all the scenes to the build settings.
  `Dungeons/<folder>/dungeon.json` holds each dungeon's lights, mood, rooms and doorways.
- **NavMesh** baked from the colliders (a NavMeshSurface with AI Navigation installed, else the
  core builder's NavMeshData loaded by `WorldBuilderNavMesh`; `-wbNoNavMesh` skips it: big worlds
  take minutes).
- **Sun** (directional light at the world's time of day), the editor's **sky** colours for the
  hour with its sun (`WorldBuilder/Sky`, also the ambient light), **fog** matching the world's
  haze; in URP projects a global **Volume** (`Generated/Look.asset`) with the art style's tone
  curve, exposure, bloom, vignette and its colour grade (saturation, contrast, split toning,
  lift, foliage hue, vibrance) as a Color Lookup LUT (`Look/grade_lut.png`); gameplay **markers** (`WorldBuilderMarker` components: player start, POIs,
  NPCs, spawn regions with outlines, resource nodes, fishing spots) and a **camera** with
  `WorldBuilderFlyCamera` (WASD, right mouse to look, Shift fast, F walk / fly).

The full lighting and grade values are also in `Data/lighting.json`. Model textures are written
once into `Textures/` (the GLBs refer to them; `Textures/textures.json` lists which are normal
maps). `Data/gameplay.json` is the documented server format (`docs/server-data.md`: every item
with its zone and level range); `Server/navmesh/` (outside the WorldBuilder folder) has Skyfold
Studio's own navmesh for game servers. All positions are in Unity space (Skyfold Studio's +X
east is Unity -X, matching glTFast). `manifest.json` lists the terrain transform, counts and
lighting values; `CREDITS.md` lists the CC0 sources.
"#;
