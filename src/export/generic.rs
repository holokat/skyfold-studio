//! The engine-neutral package: every shared building block as plain files (PNG / RAW / GLB /
//! JSON) with a manifest and a README explaining the formats, for engines and tools without a
//! dedicated exporter. The helpers here also write the parts Unity and Unreal share.

use serde_json::{Value, json};

use super::core::{self, Ctx, LayerTex, Snapshot, TerrainOut};

/// Removes the folders and files a previous export of this kind wrote (only Skyfold Studio's own).
pub fn clean(ctx: &Ctx, entries: &[&str]) {
    for e in entries {
        let p = ctx.root.join(e);
        if p.is_dir() {
            let _ = std::fs::remove_dir_all(&p);
        } else if p.is_file() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// `heightmap.png` (16-bit), `heightmap.r16` (little-endian) and optionally `heightmap.f32`.
pub fn write_heightmaps(ctx: &Ctx, dir: &str, t: &TerrainOut, f32_too: bool) -> Result<(), String> {
    let h16 = t.heights16();
    ctx.write(&format!("{dir}/heightmap.r16"), bytemuck::cast_slice(&h16))?;
    ctx.write(
        &format!("{dir}/heightmap.png"),
        &core::png_gray16(t.n as u32, t.n as u32, h16),
    )?;
    if f32_too {
        ctx.write(
            &format!("{dir}/heightmap.f32"),
            bytemuck::cast_slice(&t.heights),
        )?;
    }
    Ok(())
}

/// One grayscale weight map per used layer (`weights/<layer>.png`, 0..255, summing to 255).
pub fn write_weightmaps(ctx: &Ctx, dir: &str, t: &TerrainOut) -> Result<Vec<String>, String> {
    let mut files = vec![];
    for (k, &l) in t.used.iter().enumerate() {
        let f = format!(
            "{dir}/weights/{}.png",
            crate::terrain::textures::LAYER_FILES[l]
        );
        ctx.write(
            &f,
            &core::png_gray8(t.n as u32, t.n as u32, t.weights[k].clone()),
        )?;
        files.push(f);
    }
    Ok(files)
}

/// `layers/<layer>/albedo.png`, `normal.png` (OpenGL), `mask.png` (R metal, G AO, B height,
/// A smoothness).
pub fn write_layers(ctx: &Ctx, dir: &str, layers: &[LayerTex]) -> Result<(), String> {
    for l in layers {
        ctx.write(
            &format!("{dir}/{}/albedo.png", l.key),
            &core::png_rgb(&l.albedo),
        )?;
        ctx.write(
            &format!("{dir}/{}/normal.png", l.key),
            &core::png_rgb(&l.normal),
        )?;
        ctx.write(
            &format!("{dir}/{}/mask.png", l.key),
            &core::png_rgba(&l.mask),
        )?;
    }
    Ok(())
}

pub fn layers_json(
    t: &TerrainOut,
    layers: &[LayerTex],
    tex_dir: &str,
    weights_dir: Option<&str>,
) -> Value {
    json!(
        layers
            .iter()
            .map(|l| {
                let slot = t.used.iter().position(|&u| u == l.index).unwrap_or(0);
                let mut v = json!({
                    "index": l.index,
                    "name": l.name,
                    "key": l.key,
                    "material": l.material,
                    "tile_m": l.tile_m,
                    "footstep_surface": l.surface.sound_stem(),
                    "albedo": format!("{tex_dir}/{}/albedo.png", l.key),
                    "normal": format!("{tex_dir}/{}/normal.png", l.key),
                    "mask": format!("{tex_dir}/{}/mask.png", l.key),
                    "used_slot": slot,
                });
                if let Some(w) = weights_dir {
                    v["weightmap"] = json!(format!("{w}/weights/{}.png", l.key));
                }
                v
            })
            .collect::<Vec<_>>()
    )
}

pub fn export(s: &Snapshot, ctx: &Ctx) -> Result<Value, String> {
    clean(
        ctx,
        &[
            "terrain",
            "models",
            "textures",
            "navmesh",
            "water",
            "roads",
            "caves",
            "maps",
            "flames",
            "manifest.json",
            "instances.json",
            "gameplay.json",
            "markers.json",
            "lighting.json",
            "README.md",
            "CREDITS.md",
        ],
    );
    let o = &ctx.opts;
    if o.shared_textures {
        ctx.shared.enable("textures");
    }
    let n = o.target.terrain_vertices(o.terrain);
    ctx.stage("Terrain heights and weights", 0.02)?;
    let t = core::terrain(s, n);
    write_heightmaps(ctx, "terrain", &t, true)?;
    // big worlds also as tiles of 2049 samples sharing their edges (x east, z south)
    let (tiles, tn) = super::tiles::split(o.target, n);
    if tiles > 1 {
        let (q, h16) = (tn - 1, t.heights16());
        for j in 0..tiles {
            for i in 0..tiles {
                let tile = super::tiles::block(&h16, n, i * q, j * q, tn, tn);
                ctx.write(
                    &format!("terrain/tiles/heightmap_x{i}_z{j}.r16"),
                    bytemuck::cast_slice(&tile),
                )?;
                ctx.write(
                    &format!("terrain/tiles/heightmap_x{i}_z{j}.png"),
                    &core::png_gray16(tn as u32, tn as u32, tile),
                )?;
            }
        }
    }
    let weight_files = write_weightmaps(ctx, "terrain", &t)?;
    // fixed splat packing: layer l → splat_<l / 4>.png channel l % 4 (all 12 layer slots)
    for k in 0..3 {
        let mut px = vec![0u8; n * n * 4];
        for (slot, &l) in t.used.iter().enumerate() {
            if l / 4 == k {
                for (v, w) in t.weights[slot].iter().enumerate() {
                    px[v * 4 + l % 4] = *w;
                }
            }
        }
        ctx.write(
            &format!("terrain/splat_{k}.png"),
            &core::png_rgba(&image::RgbaImage::from_raw(n as u32, n as u32, px).ok_or("splat")?),
        )?;
    }
    ctx.stage("Terrain layer textures", 0.1)?;
    let layers = core::layer_textures(s, &t.used, o.texture_size)?;
    write_layers(ctx, "terrain/layers", &layers)?;
    if o.terrain_meshes {
        ctx.stage("Terrain meshes", 0.18)?;
        ctx.write("terrain/terrain.glb", &core::terrain_glb(&t, 16))?;
    }
    if o.plants
        && let Some(g) = core::grass(s, n)
    {
        let px: Vec<u8> = g.into_iter().flatten().collect();
        ctx.write(
            "terrain/grass.png",
            &core::png_rgba(&image::RgbaImage::from_raw(n as u32, n as u32, px).ok_or("grass")?),
        )?;
    }
    // procedural nature as density maps + rules (the instances are baked into instances.json)
    if let Some(nat) = &s.nature {
        for (c, map) in nat.maps.iter().enumerate() {
            let name = crate::nature::rules::CATS[c].name().to_lowercase();
            ctx.write(
                &format!("nature/density_{name}.png"),
                &core::png_gray8(nat.size as u32, nat.size as u32, map.clone()),
            )?;
        }
        ctx.write(
            "nature/rules.json",
            serde_json::to_string_pretty(&nat.rules)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )?;
    }
    ctx.stage("Maps", 0.22)?;
    let maps = core::generic_maps(s, ctx, "maps")?;
    let models = core::models(s, ctx, "models", 0.25, 0.85)?;
    let inst = core::instances(s, &models);
    // (each instance's world cell too: `cells`, one index per instance)
    let mut ij = core::instances_json(s, &models, &inst, "models");
    let half = s.terrain.half();
    if let Some(list) = ij["models"].as_array_mut() {
        for (v, li) in list.iter_mut().zip(&inst) {
            v["cells"] = json!(super::tiles::cell_indices(half, li.iter().map(|i| i.pos)).0);
        }
    }
    ij["cells"] = super::tiles::cells_json(half);
    ctx.write_json("instances.json", &ij)?;
    ctx.stage("Water, roads, gameplay", 0.88)?;
    let water = if o.water {
        let (glb, bodies) = core::water(s);
        if let Some(b) = glb {
            ctx.write_glb("water/water.glb", b)?;
        }
        ctx.write("water/water_normal.png", &core::water_normal_png(512))?;
        let v = json!({
            "ocean": { "sea_level_m": s.terrain.sea_level, "style": core::style_json(&s.ocean) },
            "volumes": super::collision::water_volumes(s).iter().map(|v| v.json()).collect::<Vec<_>>(),
            "bodies": bodies.iter().map(|b| json!({ "kind": b.kind, "name": b.name, "node": b.node, "style": core::style_json(&b.style) })).collect::<Vec<_>>(),
            "mesh": "water/water.glb",
            "normal_map": "water/water_normal.png",
        });
        ctx.write_json("water/water.json", &v)?;
        v
    } else {
        Value::Null
    };
    let roads = if o.roads {
        let (glb, list) = core::roads(s, o.texture_size);
        if let Some(b) = glb {
            ctx.write_glb("roads/roads.glb", b)?;
        }
        let v = core::roads_json(&list);
        ctx.write_json("roads/roads.json", &v)?;
        v
    } else {
        Value::Null
    };
    let caves = core::caves(s, o.texture_size, "", o.optimize_caves);
    let caves_v = if caves.glb.is_some() || !caves.holes.is_empty() {
        if let Some(b) = &caves.glb {
            ctx.write_glb("caves/caves.glb", b.clone())?;
        }
        if let Some(b) = &caves.holes_png {
            ctx.write("caves/holes.png", b)?;
        }
        let v = json!({ "caves": caves.json, "mesh": caves.glb.as_ref().map(|_| "caves/caves.glb"), "holes": { "cells": caves.holes, "mask": caves.holes_png.as_ref().map(|_| "caves/holes.png"), "note": "terrain cells (i, j) — cell (i, j) spans heightmap vertices i..i+1, j..j+1 — whose triangles are left out: the cave meshes draw the ground there" } });
        ctx.write_json("caves/caves.json", &v)?;
        json!("caves/caves.json")
    } else {
        Value::Null
    };
    // dungeons: their maps' architecture, objects and links (before the shared texture index)
    let dungeons = super::dungeons::generic(s, ctx, &models)?;
    if o.gameplay {
        ctx.write_json("gameplay.json", &core::gameplay_json(s))?;
        ctx.write_json("markers.json", &core::markers_json(s))?;
    }
    let navmesh = super::nav::write(ctx, &s.navmesh, "navmesh")?;
    let light = core::lighting(s);
    ctx.write_json("lighting.json", &light.json(s))?;
    ctx.write("CREDITS.md", core::credits(s, &layers).as_bytes())?;
    let counts = core::counts(s, &models, &inst);
    let textures = ctx.shared.write_index(ctx)?;
    let manifest = json!({
        "format": "world-builder-engine-export/1",
        "target": "generic",
        "world": s.name,
        "seed": s.seed,
        "coordinates": "metres, Y up, +X east, +Z south, right-handed (glTF); world (0,0) is the map centre; heightmap pixel (0,0) is the north-west corner, columns run east, rows run south",
        "terrain": {
            "meta": t.meta(),
            "heightmap": { "png16": "terrain/heightmap.png", "raw_r16_le": "terrain/heightmap.r16", "raw_f32_le": "terrain/heightmap.f32" },
            "splat": { "maps": ["terrain/splat_0.png", "terrain/splat_1.png", "terrain/splat_2.png"], "packing": "layer index l → splat_<l/4>.png channel RGBA[l%4]; weights sum to 255" },
            "weightmaps": weight_files,
            "layers": layers_json(&t, &layers, "terrain/layers", Some("terrain")),
            "meshes": o.terrain_meshes.then_some("terrain/terrain.glb (node chunk_XX_ZZ per 1/16 of the map; UV 0..1 over the whole terrain)"),
            "tiles": (tiles > 1).then(|| json!({ "per_side": tiles, "samples": tn, "files": "terrain/tiles/heightmap_x<i>_z<j>.png / .r16 (i east, j south; neighbouring tiles share their edge samples; same height decode)" })),
            "grass": "terrain/grass.png: R density 0..1, G dryness, B tint (0 cold … 1 warm), A blade height × 2",
            "style": &s.terrain.style,
        },
        "maps": maps,
        "nature": s.nature.is_some().then_some("nature/rules.json + nature/density_<category>.png: procedural nature as density maps and rules (for engine-side scattering); every instance is also baked into instances.json"),
        "models": "instances.json (one entry per model: file, variant nodes with LOD nodes, LOD end distances, collider, flat instance data)",
        "water": water,
        "roads": roads,
        "caves": caves_v,
        "dungeons": (!dungeons.is_null()).then_some("dungeons/dungeons.json (each dungeon: dungeons/<folder>/dungeon.glb, dungeon.json, instances.json; the world's portal sheets: dungeons/portals.glb)"),
        "fires": crate::flames::fires_json(&s.fires),
        "flames": crate::dungeons::export::flame_sheets_json(),
        "gameplay": o.gameplay.then_some("gameplay.json"),
        "lighting": "lighting.json",
        "shared_textures": textures,
        "navmesh": navmesh,
        "counts": counts,
    });
    super::dungeons::write_flame_sheets(s, "flames", &|rel, b| ctx.write(rel, b))?;
    ctx.write_json("manifest.json", &manifest)?;
    ctx.write("README.md", README.replace("{name}", &s.name).as_bytes())?;
    Ok(json!({ "terrain_vertices": n, "layers": layers.len(), "counts": counts }))
}

const README: &str = r#"# {name} — Skyfold Studio export (generic)

An engine-neutral copy of the world. `manifest.json` describes everything; this file explains the
formats.

## Coordinates

Metres, Y up, +X east, +Z south, right-handed (the glTF convention). World (0, 0) is the map
centre. Heightmap pixel (0, 0) is the north-west corner: columns run east (+X), rows run south (+Z).

## Terrain

- `terrain/heightmap.png` (16-bit grey), `heightmap.r16` (raw little-endian u16) and
  `heightmap.f32` (raw little-endian metres). Decode the 16-bit values with
  `height_m = height_min_m + value / 65535 * (height_max_m - height_min_m)` (both in
  `manifest.json` → `terrain.meta`). The map spans `size_m` metres; vertices are
  `metres_per_vertex` apart.
- `terrain/splat_0..2.png`: final ground-material weights (automatic style rules + paint) of the
  12 layers: layer *l* is in `splat_<l/4>.png`, channel RGBA[*l* % 4]. Weights sum to 255.
  `terrain/weights/<layer>.png` has the same per used layer as greyscale.
- `terrain/layers/<layer>/`: the world's chosen CC0 photoscan material per layer at the chosen
  texture size: `albedo.png` (sRGB), `normal.png` (tangent space, OpenGL / +Y convention),
  `mask.png` (R metallic, G ambient occlusion, B height, A smoothness — Unity's mask-map layout).
  Tile them every `tile_m` metres in world XZ.
- `terrain/terrain.glb`: the terrain as 16 × 16 mesh chunks (UV 0..1 over the whole map, for the
  splat maps). Worlds beyond 4097 samples a side also come as `terrain/tiles/heightmap_x<i>_z<j>`
  (2049-sample tiles sharing their edges, same height decode).
- `terrain/grass.png`: R grass density, G dryness, B tint (0 cold … 1 warm), A blade height / 2.

## Models and instances

`models/<asset>.glb`, one per asset, each holding one node per variant (`<asset>_v0`, `_v1`, …)
with one child per level of detail (`<asset>_v0_LOD0`, …; LOD0 is the finest). Packed models
(Poly Haven, Skyborn, hand-painted, imports) keep their original node tree under `_v0_LOD0` and
get simplified levels (within about a pixel of error at their switch distance). A tree's last
level is an impostor: two crossed quads with a picture of the tree, drawn to the draw distance.
Leaves are alpha-tested (`alphaMode MASK`, double-sided). A root `<asset>_v<k>_COL` per variant is its
collision proxy (never drawn): a convex hull for props and rocks, a simplified shape for walk-in
buildings and bridges, a trunk prism for trees. Their images are written once into `textures/`
(`textures/textures.json` gives each one's role: colour, normal map or data) and the GLBs refer
to them by relative URI.

`instances.json` lists per model: the GLB, variant and LOD node names, where each LOD stops being
drawn (`lod_end_m`), whether it casts shadows, a collider hint (`capsule` trunk per variant with
`trunk_radius_m`, `convex` hull, `mesh` triangle collider, `none`; the proxy node in
`collision_node`), `physics` (footstep surface, friction, bounciness) and `data`: a flat array of
`variant, x, y, z, qx, qy, qz, qw, scale` per instance, with `cells`: each instance's 512 m world
cell (layout in the top-level `cells`).

## Water, roads, gameplay, lighting

- `water/water.json`: sea level, the ocean's look and every lake / river / waterfall with its
  style, and `volumes`: swim boxes (lakes, river stretches, the ocean; centre, size, yaw,
  surface) (colours are sRGB; `absorption_per_m` and `scattering` for physically based shading);
  `water/water.glb` holds their surface meshes; `water/water_normal.png` is a tileable ripple map.
- `roads/roads.json`: centrelines sampled every 2 m on the ground, width, surface kind and travel
  factor; `roads/roads.glb` holds textured ribbon meshes.
- `caves/caves.glb`: every cave, mine, sinkhole and rock arch as world-space meshes (a node
  `cave_<id>_<name>` each: rock walls and floors with their photoscanned textures, mine timbers,
  still pools). Where a cave opens the ground the terrain must be cut: `caves/holes.png` (one
  pixel per terrain cell, white = no terrain triangles there; the cave mesh draws that ground)
  and the same cells in `caves/caves.json`, with each cave's entrance, chambers and dungeon
  portal (the way into the dungeon's own map, with its name and instancing). Lanterns, crystals
  and crates are ordinary instances in `instances.json`.
- `dungeons/dungeons.json`: the world's dungeons, maps of their own. For each underground one,
  `dungeons/<folder>/` holds:
  - `dungeon.glb`: its architecture in its own space, one node per level. Walls, floors and
    stairs are `<name>_L<level>`, ceilings `<name>_L<level>_ceiling`; use both as colliders.
    `<name>_liquids` and `<name>_exit` (the way out's glowing sheet) don't collide.
  - `instances.json`: its light fixtures, dressing and placed objects, in the same format as the
    world's, using the same models.
  - `dungeon.json`: its light sources (colour, lumens, reach, spot cones, flicker), mood
    (ambient, fog, exposure, haze, dust, ambience), rooms (outlines, floors, roles) and doorways.
    It also holds its player start, its exit trigger and its ways in. Each way in is a trigger
    box in the world (centre, quaternion, half size) with where players come back out.

  `dungeons/portals.glb` holds the world's portal sheets. Realms are worlds of their own: open
  one and export it.
- `gameplay.json`: the documented server format (`docs/server-data.md`): player start, POIs,
  NPCs, spawn regions, the creature library, resource nodes, ore deposits and fishing spots, each
  with its zone and level range; `markers.json` the same as a flat list of markers.
- `navmesh/`: Skyfold Studio's navmesh for game servers (`navmesh.json` convex polygons with
  areas, costs and neighbours, `navmesh.obj`, walkability maps); one walkable layer, so cave
  floors and upper storeys aren't in it.
- `lighting.json`: time of day, sun direction / colour / strength, haze (exponential-squared fog
  density per metre), cloud cover and the colour grade (exposure, contrast, saturation, bloom).
- `maps/`: footstep surfaces, zones and biomes at the terrain's own resolution, with legends in
  `manifest.json`. `zones.lighting` lists each zone's own lighting & style (brightness,
  contrast, colour, sun, shadow fill, fog, glow, vignette and an optional art style) to
  crossfade in while the player is inside; a biome's `lighting` applies where no zone sets one.

`CREDITS.md` lists the CC0 sources of every texture and model.
"#;
