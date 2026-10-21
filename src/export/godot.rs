//! Godot 4 target: a complete project (`project.godot` + `world.tscn`) that opens directly in
//! Godot 4.3 or newer. The scene holds the WorldEnvironment (physical sky, filmic tonemap, haze
//! fog, grade), the sun at the world's time of day, the ocean, Marker3D gameplay markers and a
//! fly / walk camera; small `@tool` scripts build the heavy parts from `world_builder/` when the
//! scene loads (in the editor too, as internal children that are never saved into the scene):
//! terrain chunks with the splat shader and a HeightMapShape3D collider, MultiMeshInstance3D
//! vegetation / rocks / props per chunk and LOD with visibility ranges, and lakes / rivers with
//! the water shader. `world_builder/scripts/wb_check.gd` verifies a project headless.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::core::{self, Ctx, Inst, LayerTex, ModelOut, Snapshot, TerrainOut};
use super::generic::clean;

const TERRAIN_SHADER: &str = include_str!("godot/terrain.gdshader");
const WATER_SHADER: &str = include_str!("godot/water.gdshader");
const TERRAIN_GD: &str = include_str!("godot/wb_terrain.gd");
const INSTANCES_GD: &str = include_str!("godot/wb_instances.gd");
const WATER_GD: &str = include_str!("godot/wb_water.gd");
const CAMERA_GD: &str = include_str!("godot/wb_camera.gd");
const CHECK_GD: &str = include_str!("godot/wb_check.gd");
const GRASS_GD: &str = include_str!("godot/wb_grass.gd");
const PORTAL_GD: &str = include_str!("godot/wb_portal.gd");
const FLICKER_GD: &str = include_str!("godot/wb_flicker.gd");
const PIECE_GD: &str = include_str!("godot/wb_piece.gd");
const FLAME_SHADER: &str = include_str!("godot/wb_flame.gdshader");

const DATA: &str = "world_builder";

fn res(rel: &str) -> String {
    format!("res://{DATA}/{rel}")
}

/// Godot's text float (always with a decimal point).
fn f(v: f32) -> String {
    if !v.is_finite() {
        return "0.0".into();
    }
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    }
}

fn color(c: [f32; 3]) -> String {
    format!("Color({}, {}, {}, 1)", f(c[0]), f(c[1]), f(c[2]))
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `Transform3D(...)`: basis rows, then the origin.
fn transform(m: Mat3, o: Vec3) -> String {
    let r = |i: usize| [m.x_axis[i], m.y_axis[i], m.z_axis[i]];
    let v: Vec<String> = [r(0), r(1), r(2)]
        .iter()
        .flatten()
        .chain(o.to_array().iter())
        .map(|v| f(*v))
        .collect();
    format!("Transform3D({})", v.join(", "))
}

/// Zenith and horizon sky colours (linear) for the world's hour: clear blue by day, warm at
/// sunrise / sunset, dark blue at night; haze pales the horizon.
fn sky_colors(light: &core::LightOut, haze: f32) -> (Vec3, Vec3) {
    let sun_y = light.sun_dir.y;
    let night = light.night;
    let sunset = (1.0 - (sun_y * 4.0).abs()).clamp(0.0, 1.0) * (1.0 - night);
    let haze = haze.clamp(0.0, 1.0);
    let day_top = Vec3::new(0.13, 0.29, 0.62).lerp(Vec3::new(0.33, 0.42, 0.55), haze);
    let day_horizon = Vec3::new(0.42, 0.56, 0.7).lerp(Vec3::new(0.6, 0.65, 0.7), haze);
    let top = day_top
        .lerp(Vec3::new(0.16, 0.18, 0.36), sunset)
        .lerp(Vec3::new(0.004, 0.007, 0.02), night);
    let horizon = day_horizon
        .lerp(Vec3::new(0.9, 0.5, 0.28), sunset)
        .lerp(Vec3::new(0.02, 0.03, 0.06), night);
    (top, horizon)
}

fn linear_to_srgb(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| crate::terrain::textures::lin_to_srgb(v) as f32 / 255.0)
}

/// Texture-array PNG (slices in a grid, left to right, top to bottom) and its grid size.
fn array_png<P: image::Pixel<Subpixel = u8> + 'static>(
    slices: Vec<image::ImageBuffer<P, Vec<u8>>>,
) -> Option<(image::ImageBuffer<P, Vec<u8>>, u32, u32)> {
    let size = slices.iter().map(|i| i.width()).min()?;
    let cols = (slices.len() as u32).clamp(1, 4);
    let rows = (slices.len() as u32).div_ceil(cols);
    let mut out = image::ImageBuffer::<P, Vec<u8>>::new(size * cols, size * rows);
    for (k, s) in slices.into_iter().enumerate() {
        let s = if s.width() == size {
            s
        } else {
            image::imageops::resize(&s, size, size, image::imageops::FilterType::Triangle)
        };
        image::imageops::replace(
            &mut out,
            &s,
            (k as u32 % cols * size) as i64,
            (k as u32 / cols * size) as i64,
        );
    }
    Some((out, cols, rows))
}

fn array_import(file: &str, cols: u32, rows: u32) -> String {
    format!(
        "[remap]\n\nimporter=\"2d_array_texture\"\ntype=\"CompressedTexture2DArray\"\n\n[deps]\n\nsource_file=\"{}\"\n\n[params]\n\ncompress/mode=2\ncompress/high_quality=false\ncompress/lossy_quality=0.7\ncompress/hdr_compression=1\ncompress/channel_pack=0\nmipmaps/generate=true\nmipmaps/limit=-1\nslices/horizontal={cols}\nslices/vertical={rows}\n",
        res(file)
    )
}

/// A model texture: VRAM compressed with mipmaps (normal maps marked so Godot packs them right).
fn texture_import(file: &str, normal: bool) -> String {
    format!(
        "[remap]\n\nimporter=\"texture\"\ntype=\"CompressedTexture2D\"\n\n[deps]\n\nsource_file=\"{}\"\n\n[params]\n\ncompress/mode=2\ncompress/high_quality=false\ncompress/lossy_quality=0.7\ncompress/hdr_compression=1\ncompress/normal_map={}\ncompress/channel_pack=0\nmipmaps/generate=true\nmipmaps/limit=-1\nroughness/mode=0\nroughness/src_normal=\"\"\nprocess/fix_alpha_border=true\nprocess/premult_alpha=false\nprocess/normal_map_invert_y=false\nprocess/hdr_as_srgb=false\nprocess/hdr_clamp_exposure=false\nprocess/size_limit=0\ndetect_3d/compress_to=0\n",
        res(file),
        if normal { 1 } else { 2 }
    )
}

/// Data textures must stay exact: lossless, no alpha-border fix (it would rewrite weights).
fn data_import(file: &str) -> String {
    format!(
        "[remap]\n\nimporter=\"texture\"\ntype=\"CompressedTexture2D\"\n\n[deps]\n\nsource_file=\"{}\"\n\n[params]\n\ncompress/mode=0\ncompress/high_quality=false\ncompress/lossy_quality=0.7\ncompress/hdr_compression=1\ncompress/normal_map=0\ncompress/channel_pack=0\nmipmaps/generate=false\nmipmaps/limit=-1\nroughness/mode=0\nroughness/src_normal=\"\"\nprocess/fix_alpha_border=false\nprocess/premult_alpha=false\nprocess/normal_map_invert_y=false\nprocess/hdr_as_srgb=false\nprocess/hdr_clamp_exposure=false\nprocess/size_limit=0\ndetect_3d/compress_to=0\n",
        res(file)
    )
}

/// GLB import settings. Godot's automatic LODs (on top of Skyfold Studio's own) everywhere but on
/// trees and plants, whose two-sided leaf cards they would simplify away, and the terrain, which
/// has its own crack-free chunk levels. Static meshes get lightmap UV2s (`light_baking` = static
/// lightmaps) at a texel size suiting their scale.
fn scene_import(file: &str, lods: bool, lightmap_texel: f32) -> String {
    format!(
        "[remap]\n\nimporter=\"scene\"\ntype=\"PackedScene\"\n\n[deps]\n\nsource_file=\"{}\"\n\n[params]\n\nmeshes/generate_lods={lods}\nmeshes/create_shadow_meshes=true\nmeshes/light_baking={}\nmeshes/lightmap_texel_size={}\n",
        res(file),
        if lightmap_texel > 0.0 { 2 } else { 1 },
        f(lightmap_texel.max(0.2)),
    )
}

/// Writes a GLB with its import settings (no automatic LODs, no lightmap UVs).
fn write_glb(ctx: &Ctx, file: &str, bytes: &[u8]) -> Result<(), String> {
    write_glb_with(ctx, file, bytes, false, 0.0)
}

fn write_glb_with(
    ctx: &Ctx,
    file: &str,
    bytes: &[u8],
    lods: bool,
    lightmap_texel: f32,
) -> Result<(), String> {
    ctx.write_glb(&format!("{DATA}/{file}"), bytes.to_vec())?;
    ctx.write(
        &format!("{DATA}/{file}.import"),
        scene_import(file, lods, lightmap_texel).as_bytes(),
    )
}

/// Chunk size (m) for single-LOD models (small plants): the smallest that holds a dozen
/// instances per block on average, so sparse kinds don't become thousands of tiny nodes.
fn chunk_size(list: &[Inst]) -> f32 {
    let mut c: f32 = 64.0;
    while c < 256.0 {
        if list.len() >= groups(list, c).len() * 12 {
            break;
        }
        c *= 2.0;
    }
    c
}

/// Instances per variant and map cell of `c` metres.
fn groups(list: &[Inst], c: f32) -> std::collections::BTreeMap<(u32, i32, i32), Vec<&Inst>> {
    let mut g: std::collections::BTreeMap<(u32, i32, i32), Vec<&Inst>> =
        std::collections::BTreeMap::new();
    for i in list {
        g.entry((
            i.variant,
            (i.pos[0] / c).floor() as i32,
            (i.pos[2] / c).floor() as i32,
        ))
        .or_default()
        .push(i);
    }
    g
}

/// Appends a block of transforms relative to its centre (MultiMesh buffer rows) and returns
/// the block's index entry.
fn pack(bin: &mut Vec<f32>, variant: u32, items: &[&Inst], cx: i32, cz: i32, c: f32) -> Value {
    let y = items.iter().map(|i| i.pos[1]).sum::<f32>() / items.len() as f32;
    let centre = Vec3::new((cx as f32 + 0.5) * c, y, (cz as f32 + 0.5) * c);
    let entry = json!({ "variant": variant, "center": centre.to_array(), "offset": bin.len() * 4, "count": items.len() });
    for i in items {
        let r = Mat3::from_quat(Quat::from_array(i.rot).normalize())
            * Mat3::from_diagonal(Vec3::from_array(i.stretch.unwrap_or([1.0; 3])) * i.scale);
        let o = Vec3::from_array(i.pos) - centre;
        for row in 0..3 {
            bin.extend_from_slice(&[r.x_axis[row], r.y_axis[row], r.z_axis[row], o[row]]);
        }
    }
    entry
}

/// Writes `instances/<model>.bin` and returns the model's index entry. Godot switches a
/// MultiMeshInstance3D's LOD by its distance as a whole, so instances are grouped by map cells:
/// models with LODs draw their last LOD in coarse cells (few draw calls far away) and the nearer
/// LODs in fine cells that only show while their coarse cell is close (`visibility_parent`), so
/// nothing is drawn twice and LOD0 stays near the camera. Each block lists what it draws as
/// (LOD, begin, end) with distances widened by the cell's reach (they're measured to its centre).
fn model_entry(s: &Snapshot, ctx: &Ctx, m: &ModelOut, list: &[Inst]) -> Result<Value, String> {
    model_entry_in(s, ctx, m, list, "instances")
}

/// `model_entry` with its packed transforms in `bins` (under the data folder).
fn model_entry_in(
    s: &Snapshot,
    ctx: &Ctx,
    m: &ModelOut,
    list: &[Inst],
    bins: &str,
) -> Result<Value, String> {
    let a = &s.assets[m.asset];
    let e = &m.lod_end;
    let mut bin: Vec<f32> = Vec::with_capacity(list.len() * 24);
    let mut blocks = vec![];
    if e.len() < 2 {
        let c = chunk_size(list);
        let end = e.first().copied().unwrap_or(s.draw_distance) + c * 0.71;
        for ((v, cx, cz), items) in groups(list, c) {
            let mut b = pack(&mut bin, v, &items, cx, cz, c);
            b["draw"] = json!([[0.0, 0.0, end]]);
            blocks.push(b);
        }
    } else {
        // fine cells about the size of the LOD 0 range, coarse ones of the range where the last LOD
        // takes over
        let p2 = |x: f32| 2f32.powf(x.max(1.0).log2().ceil());
        let cf = p2(e[0] * 1.6).clamp(32.0, 128.0);
        let cc = p2(e[e.len() - 2] * 0.8).clamp(cf, 512.0);
        let last = e.len() - 1;
        let (begin, end) = (e[last - 1] + cc * 0.71, e[last] + cc * 0.71);
        for ((v, cx, cz), items) in groups(list, cc) {
            let mut b = pack(&mut bin, v, &items, cx, cz, cc);
            b["cell"] = json!([cx, cz]);
            b["draw"] = json!([[last as f32, begin, end]]);
            blocks.push(b);
        }
        let reach = cf * 0.71;
        let draw: Vec<[f32; 3]> = (0..last)
            .map(|k| {
                [
                    k as f32,
                    if k == 0 { 0.0 } else { e[k - 1] + reach },
                    if k + 1 == last { 0.0 } else { e[k] + reach },
                ]
            })
            .collect();
        let per = (cc / cf) as i32;
        for ((v, fx, fz), items) in groups(list, cf) {
            let mut b = pack(&mut bin, v, &items, fx, fz, cf);
            b["parent"] = json!([fx.div_euclid(per), fz.div_euclid(per)]);
            b["draw"] = json!(draw);
            blocks.push(b);
        }
    }
    let file = format!("{DATA}/{bins}/{}.bin", m.safe);
    ctx.write(&file, bytemuck::cast_slice(&bin))?;
    Ok(json!({
        "name": a.name,
        "glb": res(&format!("models/{}", m.file)),
        "generated": m.generated,
        "variants": m.variants.iter().map(|v| json!({ "node": v.node, "lods": v.lods, "trunk": v.trunk, "height": v.height, "collision": v.collision })).collect::<Vec<_>>(),
        "shadows": m.shadows,
        "collider": m.collider.name(),
        "physics": super::collision::physics_json(a.surface),
        "bin": format!("res://{file}"),
        "blocks": blocks,
    }))
}

fn water_params(st: &crate::water::WaterStyle, ocean: bool, river: bool) -> Value {
    json!({
        "shallow_color": st.shallow,
        "deep_color": st.deep,
        "clarity": st.clarity,
        "transparency": st.transparency,
        "wave_height": st.waves,
        "wave_speed": st.wave_speed,
        "roughness_amount": (st.roughness * 0.5).clamp(0.02, 1.0),
        "foam": st.foam,
        "shore_foam": st.shore_foam,
        "flow": st.flow * if river { 1.6 } else { 0.4 },
        "flow_dir": [st.flow_dir.to_radians().cos(), st.flow_dir.to_radians().sin()],
        "along_uv": river,
        "ocean": ocean,
        "emissive_color": st.emissive,
        "emissive_strength": st.emissive_strength,
        "metallic_amount": st.metallic,
    })
}

struct Scene {
    ext: Vec<String>,
    sub: Vec<String>,
    nodes: Vec<String>,
}

impl Scene {
    fn ext(&mut self, ty: &str, path: &str) -> String {
        let id = format!("{}_wb", self.ext.len() + 1);
        self.ext.push(format!(
            "[ext_resource type=\"{ty}\" path=\"{path}\" id=\"{id}\"]"
        ));
        format!("ExtResource(\"{id}\")")
    }
    fn sub(&mut self, ty: &str, body: &str) -> String {
        let id = format!("{ty}_{}", self.sub.len() + 1);
        self.sub
            .push(format!("[sub_resource type=\"{ty}\" id=\"{id}\"]\n{body}"));
        format!("SubResource(\"{id}\")")
    }
    fn node(&mut self, header: &str, body: &str) {
        self.nodes.push(if body.is_empty() {
            header.to_string()
        } else {
            format!("{header}\n{body}")
        });
    }
    fn finish(self) -> String {
        let steps = self.ext.len() + self.sub.len() + 1;
        let mut out = format!("[gd_scene load_steps={steps} format=3]\n\n");
        for e in self.ext {
            out.push_str(&e);
            out.push('\n');
        }
        out.push('\n');
        for s in self.sub {
            out.push_str(&s);
            out.push_str("\n\n");
        }
        for n in self.nodes {
            out.push_str(&n);
            out.push_str("\n\n");
        }
        out
    }
}

/// Node name safe for Godot (no `. : @ / " %`), unique by its id.
fn node_name(prefix: &str, id: u32, name: &str) -> String {
    format!("{prefix}_{id}_{}", core::safe_name(name))
}

pub fn export(s: &Snapshot, ctx: &Ctx) -> Result<Value, String> {
    clean(
        ctx,
        &[
            DATA,
            "server",
            "world.tscn",
            "README.md",
            "CREDITS.md",
            "manifest.json",
        ],
    );
    let o = &ctx.opts;
    if o.shared_textures {
        ctx.shared.enable(&format!("{DATA}/textures"));
    }
    let n = o.target.terrain_vertices(o.terrain);
    ctx.stage("Terrain heights and weights", 0.02)?;
    let t = core::terrain(s, n);
    super::generic::write_heightmaps(ctx, &format!("{DATA}/terrain"), &t, false)?;
    // (cave mouths: the collider drops away under them; the caves' own colliders hold the ground)
    let mut coll: Vec<f32> = t.heights.iter().map(|h| h / t.cell).collect();
    for j in 0..t.n - 1 {
        for i in 0..t.n - 1 {
            if t.hole(i, j) {
                for (a, b) in [(i, j), (i + 1, j), (i, j + 1), (i + 1, j + 1)] {
                    coll[b * t.n + a] = (t.heights[b * t.n + a] - 60.0) / t.cell;
                }
            }
        }
    }
    // terrain tiles (one up to 4097 samples, 2049-sample tiles beyond): each its meshes, splat
    // maps, material and height-map collider
    let (tiles, tn) = super::tiles::split(o.target, n);
    let q = tn - 1;
    let tile_size = t.size / tiles as f32;
    // heights for the grass scatter: the whole map, at most 4097 samples a side
    let gstep = (n - 1) / (n - 1).min(4096);
    let gn = (n - 1) / gstep + 1;
    let gh: Vec<f32> = (0..gn * gn)
        .map(|k| coll[(k / gn) * gstep * n + (k % gn) * gstep] / gstep as f32)
        .collect();
    ctx.write(
        &format!("{DATA}/terrain/heights.f32"),
        bytemuck::cast_slice(&gh),
    )?;
    ctx.stage("Terrain layer textures", 0.08)?;
    let layers = core::layer_textures(s, &t.used, o.texture_size)?;
    write_arrays(ctx, &layers)?;
    ctx.stage("Terrain meshes", 0.16)?;
    let mut tile_list = vec![];
    for tj in 0..tiles {
        for ti in 0..tiles {
            let tag = format!("{ti}_{tj}");
            let (i0, j0) = (ti * q, tj * q);
            ctx.write(
                &format!("{DATA}/terrain/collision_{tag}.f32"),
                bytemuck::cast_slice(&super::tiles::block(&coll, n, i0, j0, tn, tn)),
            )?;
            // splat: used layer k → splat_<k/4> channel k%4 (the arrays hold the used layers in order)
            let mut splats = vec![];
            for k in 0..3 {
                let mut px = vec![0u8; tn * tn * 4];
                for slot in (k * 4..(k * 4 + 4).min(t.used.len())).collect::<Vec<_>>() {
                    for (v, w) in super::tiles::block(&t.weights[slot], n, i0, j0, tn, tn)
                        .into_iter()
                        .enumerate()
                    {
                        px[v * 4 + slot % 4] = w;
                    }
                }
                let file = if tiles == 1 {
                    format!("terrain/splat_{k}.png")
                } else {
                    format!("terrain/splat_{k}_{tag}.png")
                };
                ctx.write(
                    &format!("{DATA}/{file}"),
                    &core::png_rgba(
                        &image::RgbaImage::from_raw(tn as u32, tn as u32, px).ok_or("splat")?,
                    ),
                )?;
                ctx.write(
                    &format!("{DATA}/{file}.import"),
                    data_import(&file).as_bytes(),
                )?;
                splats.push(file);
            }
            // chunks of at most 128 quads a side, each with its own levels of detail (`lods::terrain_glb`)
            let chunks = q / (q / 16).clamp(1, 128);
            let (glb, lods) = super::lods::terrain_glb(&t, i0, j0, tn, chunks);
            let (mesh, lods_file, mat) = (
                format!("terrain/terrain_{tag}.glb"),
                format!("terrain/terrain_lods_{tag}.json"),
                format!("terrain/terrain_{tag}.tres"),
            );
            write_glb(ctx, &mesh, &glb)?;
            ctx.write_json(&format!("{DATA}/{lods_file}"), &lods)?;
            let origin = [-t.half + i0 as f32 * t.cell, -t.half + j0 as f32 * t.cell];
            ctx.write(
                &format!("{DATA}/{mat}"),
                terrain_material(&t, &layers, &splats, origin, tile_size).as_bytes(),
            )?;
            tile_list.push(json!({
                "mesh": res(&mesh), "lods": res(&lods_file), "material": res(&mat),
                "collision": res(&format!("terrain/collision_{tag}.f32")), "vertices": tn,
                "center": [origin[0] + tile_size * 0.5, origin[1] + tile_size * 0.5],
            }));
        }
    }
    ctx.write_json(
        &format!("{DATA}/terrain/tiles.json"),
        &json!({ "cell": t.cell, "tiles": tile_list }),
    )?;

    // meadow grass: photoscanned tufts scattered around the camera from the grass map
    let mut grass = false;
    if o.plants
        && let Some(g) = core::grass(s, gn)
        && let Some((glb, _)) = core::grass_tufts(o.texture_size)
    {
        let px: Vec<u8> = g.into_iter().flatten().collect();
        ctx.write(
            &format!("{DATA}/terrain/grass.png"),
            &core::png_rgba(&image::RgbaImage::from_raw(gn as u32, gn as u32, px).ok_or("grass")?),
        )?;
        ctx.write(
            &format!("{DATA}/terrain/grass.png.import"),
            data_import("terrain/grass.png").as_bytes(),
        )?;
        write_glb(ctx, "terrain/grass_tufts.glb", &glb)?;
        grass = true;
    }
    let models = core::models(s, ctx, &format!("{DATA}/models"), 0.2, 0.8)?;
    for m in &models {
        let file = format!("models/{}", m.file);
        let a = &s.assets[m.asset];
        let cards = m.generated
            && matches!(
                a.category,
                crate::objects::Category::Trees | crate::objects::Category::Plants
            );
        // buildings and other big static pieces bake lightmaps; foliage and small props use probes
        let texel = if matches!(
            a.category,
            crate::objects::Category::Structures | crate::objects::Category::Imported
        ) {
            0.25
        } else {
            0.0
        };
        ctx.write(
            &format!("{DATA}/{file}.import"),
            scene_import(&file, !cards, texel).as_bytes(),
        )?;
    }
    let inst = core::instances(s, &models);
    ctx.stage("Instances", 0.82)?;
    let mut entries = vec![];
    for (m, list) in models.iter().zip(&inst) {
        if !list.is_empty() {
            entries.push(model_entry(s, ctx, m, list)?);
        }
    }
    ctx.write_json(
        &format!("{DATA}/instances/instances.json"),
        &json!({ "models": entries }),
    )?;

    ctx.stage("Water, roads, gameplay", 0.88)?;
    let mut water_bodies = 0;
    if o.water {
        let (glb, bodies) = core::water(s);
        if let Some(b) = &glb {
            write_glb(ctx, "water/water.glb", b)?;
        }
        water_bodies = bodies.len();
        ctx.write(
            &format!("{DATA}/water/water_normal.png"),
            &core::water_normal_png(512),
        )?;
        let data = json!({
            "mesh": if glb.is_some() { res("water/water.glb") } else { String::new() },
            "shader": res("shaders/water.gdshader"),
            "normal_map": res("water/water_normal.png"),
            "bodies": bodies.iter().map(|b| json!({ "kind": b.kind, "name": b.name, "node": b.node, "params": water_params(&b.style, false, b.kind != "lake") })).collect::<Vec<_>>(),
            "volumes": super::collision::water_volumes(s).iter().map(|v| v.json()).collect::<Vec<_>>(),
        });
        ctx.write_json(&format!("{DATA}/water/water.json"), &data)?;
    }
    let roads = if o.roads {
        core::roads(s, o.texture_size)
    } else {
        (None, vec![])
    };
    if let Some(b) = &roads.0 {
        write_glb_with(ctx, "roads/roads.glb", b, true, 0.0)?;
        ctx.write_json(
            &format!("{DATA}/roads/roads.json"),
            &core::roads_json(&roads.1),
        )?;
    }
    // caves: their rock (with import colliders: `-col` nodes) and what they are
    let caves = core::caves(s, o.texture_size, "-col", o.optimize_caves);
    if let Some(b) = &caves.glb {
        write_glb_with(ctx, "caves/caves.glb", b, true, 0.0)?;
        ctx.write_json(
            &format!("{DATA}/caves/caves.json"),
            &json!({ "caves": caves.json, "holes": caves.holes }),
        )?;
    }
    if o.gameplay {
        ctx.write_json(
            &format!("{DATA}/data/gameplay.json"),
            &core::gameplay_json(s),
        )?;
    }
    // dungeons: a scene each, and triggers at their ways in (before the shared texture index)
    let dungeons = dungeon_scenes(s, ctx, &models)?;
    super::dungeons::write_flame_sheets(s, &format!("{DATA}/flames"), &|rel, b| ctx.write(rel, b))?;
    // the navmesh: for game servers (a folder Godot doesn't import) and as the scene's
    // NavigationRegion3D
    let navmesh = super::nav::write(ctx, &s.navmesh, "server/navmesh")?;
    let mut nav_region = false;
    if !navmesh.is_null() {
        ctx.write("server/.gdignore", b"")?;
        if let Some(tres) = s
            .navmesh
            .iter()
            .find(|f| f.0 == "navmesh.json")
            .and_then(|f| navigation_mesh(&f.1))
        {
            ctx.write(&format!("{DATA}/navigation/navmesh.tres"), tres.as_bytes())?;
            nav_region = true;
        }
    }
    let light = core::lighting(s);
    ctx.write_json(&format!("{DATA}/data/lighting.json"), &light.json(s))?;
    let probes: Vec<[f32; 3]> = [(1, 1), (2, 2), (3, 1), (1, 3), (3, 3), (2, 1)]
        .iter()
        .map(|&(a, b)| {
            let (i, j) = (n * a / 4, n * b / 4);
            [
                -t.half + i as f32 * t.cell,
                -t.half + j as f32 * t.cell,
                t.heights[j * n + i],
            ]
        })
        .collect();
    let total: usize = inst.iter().map(|l| l.len()).sum();
    ctx.write_json(
        &format!("{DATA}/data/check.json"),
        &json!({ "instances": total, "probes": probes }),
    )?;

    ctx.stage("Scene and project", 0.94)?;
    for (file, text) in [
        ("shaders/terrain.gdshader", TERRAIN_SHADER),
        ("shaders/water.gdshader", WATER_SHADER),
        ("scripts/wb_terrain.gd", TERRAIN_GD),
        ("scripts/wb_instances.gd", INSTANCES_GD),
        ("scripts/wb_water.gd", WATER_GD),
        ("scripts/wb_camera.gd", CAMERA_GD),
        ("scripts/wb_check.gd", CHECK_GD),
        ("scripts/wb_grass.gd", GRASS_GD),
        ("scripts/wb_portal.gd", PORTAL_GD),
        ("scripts/wb_flicker.gd", FLICKER_GD),
        ("scripts/wb_piece.gd", PIECE_GD),
        ("scripts/wb_flame.gdshader", FLAME_SHADER),
    ] {
        ctx.write(&format!("{DATA}/{file}"), text.as_bytes())?;
    }
    let markers = if o.gameplay { core::markers(s) } else { vec![] };
    let ocean = o.water && has_sea(s);
    let cave_lights = super::lights::cave_lights(&caves.json);
    let scene = world_scene(
        s,
        &t,
        &light,
        &markers,
        !entries.is_empty(),
        ocean,
        water_bodies > 0,
        roads.0.is_some(),
        caves.glb.is_some(),
        grass,
        &cave_lights,
        nav_region,
        (gn, t.size / (gn - 1) as f32),
        &dungeons,
    );
    ctx.write("world.tscn", scene.as_bytes())?;
    ctx.write("project.godot", project(s).as_bytes())?;
    ctx.write("icon.svg", ICON.as_bytes())?;
    // the shared model textures: VRAM-compressed, normal maps flagged as such
    for (file, role) in ctx.shared.list() {
        let rel = format!("textures/{file}");
        ctx.write(
            &format!("{DATA}/{rel}.import"),
            texture_import(&rel, role == super::textures::Role::Normal).as_bytes(),
        )?;
    }
    ctx.shared.write_index(ctx)?;
    ctx.write("CREDITS.md", core::credits(s, &layers).as_bytes())?;
    let counts = core::counts(s, &models, &inst);
    ctx.write_json(
        "manifest.json",
        &json!({
            "format": "world-builder-engine-export/1",
            "target": "godot4",
            "world": s.name,
            "terrain": t.meta(),
            "layers": super::generic::layers_json(&t, &layers, "(texture arrays)", None),
            "models": models.iter().zip(&inst).map(|(m, l)| m.json(&s.assets[m.asset], &format!("{DATA}/models"), l.len())).collect::<Vec<_>>(),
            "lighting": light.json(s),
            "counts": counts,
        }),
    )?;
    ctx.write("README.md", README.replace("{name}", &s.name).as_bytes())?;
    Ok(
        json!({ "terrain_vertices": n, "layers": layers.len(), "multimesh_models": entries.len(), "water_bodies": water_bodies, "markers": markers.len(), "counts": counts }),
    )
}

/// Skyfold Studio's navmesh (`navmesh.json`) as a Godot NavigationMesh resource: its vertices and
/// convex polygons as they are (shared vertices, so neighbouring polygons connect).
fn navigation_mesh(json_bytes: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(json_bytes).ok()?;
    let verts = v.get("vertices")?.as_array()?;
    let mut out = String::from("[gd_resource type=\"NavigationMesh\" format=3]\n\n[resource]\n");
    out.push_str(
        "agent_radius = 0.4\nagent_height = 1.8\nagent_max_climb = 0.45\nagent_max_slope = 45.0\n",
    );
    out.push_str("vertices = PackedVector3Array(");
    let mut first = true;
    for p in verts {
        let p = p.as_array()?;
        for c in p.iter().take(3) {
            if !first {
                out.push_str(", ");
            }
            first = false;
            out.push_str(&f(c.as_f64()? as f32));
        }
    }
    out.push_str(")\npolygons = [");
    for (k, p) in v.get("polygons")?.as_array()?.iter().enumerate() {
        if k > 0 {
            out.push_str(", ");
        }
        let ids: Vec<String> = p
            .get("v")?
            .as_array()?
            .iter()
            .filter_map(|i| i.as_u64().map(|i| i.to_string()))
            .collect();
        out.push_str(&format!("PackedInt32Array({})", ids.join(", ")));
    }
    out.push_str("]\n");
    Some(out)
}

fn write_arrays(ctx: &Ctx, layers: &[LayerTex]) -> Result<(), String> {
    let albedo = array_png(layers.iter().map(|l| l.albedo.clone()).collect()).ok_or("no layers")?;
    let normal = array_png(layers.iter().map(|l| l.normal.clone()).collect()).ok_or("no layers")?;
    let mask = array_png(layers.iter().map(|l| l.mask.clone()).collect()).ok_or("no layers")?;
    for (name, png, cols, rows) in [
        ("albedo_array", core::png_rgb(&albedo.0), albedo.1, albedo.2),
        ("normal_array", core::png_rgb(&normal.0), normal.1, normal.2),
        ("mask_array", core::png_rgba(&mask.0), mask.1, mask.2),
    ] {
        let file = format!("terrain/{name}.png");
        ctx.write(&format!("{DATA}/{file}"), &png)?;
        ctx.write(
            &format!("{DATA}/{file}.import"),
            array_import(&file, cols, rows).as_bytes(),
        )?;
    }
    Ok(())
}

/// The splat material of one terrain tile (`splats`: its three splat maps; `origin` its
/// north-west corner, `size` its side).
fn terrain_material(
    t: &TerrainOut,
    layers: &[LayerTex],
    splats: &[String],
    origin: [f32; 2],
    size: f32,
) -> String {
    let mut tiles = vec![2.0f32; 12];
    let mut tri = vec![0i32; 12];
    for (k, l) in layers.iter().enumerate().take(12) {
        tiles[k] = l.tile_m;
        tri[k] = matches!(l.index, 6 | 7) as i32;
    }
    let list = |v: Vec<String>| v.join(", ");
    format!(
        r#"[gd_resource type="ShaderMaterial" load_steps=8 format=3]

[ext_resource type="Shader" path="{shader}" id="1_s"]
[ext_resource type="Texture2DArray" path="{albedo}" id="2_a"]
[ext_resource type="Texture2DArray" path="{normal}" id="3_n"]
[ext_resource type="Texture2DArray" path="{mask}" id="4_m"]
[ext_resource type="Texture2D" path="{s0}" id="5_s0"]
[ext_resource type="Texture2D" path="{s1}" id="6_s1"]
[ext_resource type="Texture2D" path="{s2}" id="7_s2"]

[resource]
render_priority = 0
shader = ExtResource("1_s")
shader_parameter/splat_0 = ExtResource("5_s0")
shader_parameter/splat_1 = ExtResource("6_s1")
shader_parameter/splat_2 = ExtResource("7_s2")
shader_parameter/albedo_array = ExtResource("2_a")
shader_parameter/normal_array = ExtResource("3_n")
shader_parameter/mask_array = ExtResource("4_m")
shader_parameter/layer_count = {count}
shader_parameter/layer_tile = PackedFloat32Array({tiles})
shader_parameter/layer_triplanar = PackedInt32Array({tri})
shader_parameter/terrain_origin = Vector2({ox}, {oz})
shader_parameter/terrain_size = {size}
shader_parameter/sea_level = {sea}
shader_parameter/normal_strength = 1.0
"#,
        shader = res("shaders/terrain.gdshader"),
        albedo = res("terrain/albedo_array.png"),
        normal = res("terrain/normal_array.png"),
        mask = res("terrain/mask_array.png"),
        s0 = res(&splats[0]),
        s1 = res(&splats[1]),
        s2 = res(&splats[2]),
        count = layers.len().min(12),
        tiles = list(tiles.iter().map(|v| f(*v)).collect()),
        tri = list(tri.iter().map(|v| v.to_string()).collect()),
        ox = f(origin[0]),
        oz = f(origin[1]),
        size = f(size),
        sea = f(t.sea),
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn world_scene(
    s: &Snapshot,
    t: &TerrainOut,
    light: &core::LightOut,
    markers: &[core::Marker],
    instances: bool,
    ocean: bool,
    water_bodies: bool,
    roads: bool,
    caves: bool,
    grass: bool,
    cave_lights: &[super::lights::CaveLight],
    nav_region: bool,
    grass_grid: (usize, f32),
    dungeons: &DungeonsOut,
) -> String {
    let mut sc = Scene {
        ext: vec![],
        sub: vec![],
        nodes: vec![],
    };
    let terrain_script = sc.ext("Script", &res("scripts/wb_terrain.gd"));
    let camera_script = sc.ext("Script", &res("scripts/wb_camera.gd"));

    // sky: a procedural gradient in the editor's colours for this hour (physical skies render
    // much greyer than Skyfold Studio's atmosphere at the same exposure)
    let (top, horizon) = sky_colors(light, s.look.haze * light.haze);
    let sky_mat = sc.sub(
        "ProceduralSkyMaterial",
        &format!(
            "sky_top_color = {}
sky_horizon_color = {}
sky_curve = 0.12
sky_energy_multiplier = 1.0
ground_bottom_color = Color(0.16, 0.17, 0.15, 1)
ground_horizon_color = {}
ground_curve = 0.05
sun_angle_max = 25.0
sun_curve = 0.1",
            color(linear_to_srgb(top.to_array())),
            color(linear_to_srgb(horizon.to_array())),
            color(linear_to_srgb(horizon.to_array()))
        ),
    );
    let sky = sc.sub("Sky", &format!("sky_material = {sky_mat}"));
    let fog_density = light.fog_density_exp(3000.0);
    let fog_col = linear_to_srgb(light.fog_color);
    let env = sc.sub(
        "Environment",
        &format!(
            "background_mode = 2\nsky = {sky}\nambient_light_source = 3\nambient_light_sky_contribution = 1.0\nreflected_light_source = 2\n\
             tonemap_mode = 2\ntonemap_exposure = {exposure}\ntonemap_white = 6.0\nssao_enabled = true\nssao_radius = 1.5\nssao_intensity = 1.5\n\
             glow_enabled = true\nglow_intensity = {glow}\nglow_bloom = {bloom}\n\
             fog_enabled = {fog}\nfog_light_color = {fog_col}\nfog_density = {fog_density}\nfog_aerial_perspective = 0.6\nfog_sky_affect = 0.0\n\
             adjustment_enabled = true\nadjustment_contrast = {contrast}\nadjustment_saturation = {saturation}",
            exposure = f(2f32.powf(light.exposure).clamp(0.25, 4.0)),
            glow = f(0.4 + light.bloom * 2.0),
            bloom = f(light.bloom * 0.3),
            fog = light.fog,
            fog_col = color(fog_col),
            fog_density = f(fog_density),
            contrast = f(light.contrast),
            saturation = f(light.saturation),
        ),
    );
    sc.node(
        "[node name=\"World\" type=\"Node3D\"]",
        "metadata/wb_world = true",
    );
    sc.node(
        "[node name=\"WorldEnvironment\" type=\"WorldEnvironment\" parent=\".\"]",
        &format!("environment = {env}"),
    );
    // the sun shines along its -Z: point +Z at the sun
    let z = light.sun_dir.normalize();
    let x = Vec3::Y.cross(z).normalize_or(Vec3::X);
    let y = z.cross(x);
    let sun_col = linear_to_srgb(light.sun_color);
    sc.node(
        "[node name=\"Sun\" type=\"DirectionalLight3D\" parent=\".\"]",
        &format!(
            "transform = {}\nlight_color = {}\nlight_energy = {}\nlight_angular_distance = 0.5\nshadow_enabled = true\ndirectional_shadow_mode = 2\ndirectional_shadow_max_distance = 500.0\ndirectional_shadow_blend_splits = true\nmetadata/wb_hour = {}",
            transform(Mat3::from_cols(x, y, z), Vec3::new(0.0, 500.0, 0.0)),
            color(sun_col),
            f((light.sun_strength * 1.1).max(0.02)),
            f(light.hour)
        ),
    );
    sc.node(
        "[node name=\"Terrain\" type=\"Node3D\" parent=\".\"]",
        &format!(
            "script = {terrain_script}\ntiles_path = {}",
            quote(&res("terrain/tiles.json")),
        ),
    );
    if ocean || water_bodies {
        let shader = sc.ext("Shader", &res("shaders/water.gdshader"));
        let ripples = sc.ext("Texture2D", &res("water/water_normal.png"));
        let p = water_params(&s.ocean, true, false);
        let mut body = format!(
            "render_priority = 0\nshader = {shader}\nshader_parameter/ripple_normal = {ripples}"
        );
        for (k, v) in p.as_object().into_iter().flatten() {
            let val = match v {
                Value::Array(a) if a.len() == 3 => color([
                    a[0].as_f64().unwrap_or(0.0) as f32,
                    a[1].as_f64().unwrap_or(0.0) as f32,
                    a[2].as_f64().unwrap_or(0.0) as f32,
                ]),
                Value::Array(a) if a.len() == 2 => format!(
                    "Vector2({}, {})",
                    f(a[0].as_f64().unwrap_or(0.0) as f32),
                    f(a[1].as_f64().unwrap_or(0.0) as f32)
                ),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => f(n.as_f64().unwrap_or(0.0) as f32),
                _ => continue,
            };
            body.push_str(&format!("\nshader_parameter/{k} = {val}"));
        }
        if ocean {
            let mat = sc.sub("ShaderMaterial", &body);
            let size = t.size * 4.0;
            let plane = sc.sub(
                "PlaneMesh",
                &format!(
                    "size = Vector2({}, {})\nsubdivide_width = 400\nsubdivide_depth = 400",
                    f(size),
                    f(size)
                ),
            );
            sc.node(
                "[node name=\"Ocean\" type=\"MeshInstance3D\" parent=\".\"]",
                &format!(
                    "transform = {}\ncast_shadow = 0\nmesh = {plane}\nmaterial_override = {mat}",
                    transform(Mat3::IDENTITY, Vec3::new(0.0, t.sea, 0.0))
                ),
            );
        }
        {
            // lakes, rivers and waterfalls, and the swim volumes (the ocean's too)
            let script = sc.ext("Script", &res("scripts/wb_water.gd"));
            sc.node(
                "[node name=\"Water\" type=\"Node3D\" parent=\".\"]",
                &format!(
                    "script = {script}\ndata_path = {}",
                    quote(&res("water/water.json"))
                ),
            );
        }
    }
    if roads {
        let glb = sc.ext("PackedScene", &res("roads/roads.glb"));
        sc.node(
            &format!("[node name=\"Roads\" parent=\".\" instance={glb}]"),
            "",
        );
    }
    if caves {
        let glb = sc.ext("PackedScene", &res("caves/caves.glb"));
        sc.node(
            &format!("[node name=\"Caves\" parent=\".\" instance={glb}]"),
            "",
        );
        // the glowing crystals
        if !cave_lights.is_empty() {
            sc.node(
                "[node name=\"CrystalLights\" type=\"Node3D\" parent=\".\"]",
                "",
            );
            for (k, l) in cave_lights.iter().enumerate() {
                sc.node(
                    &format!("[node name=\"Crystal_{k}\" type=\"OmniLight3D\" parent=\"CrystalLights\"]"),
                    &format!("transform = {}\nlight_color = {}\nlight_energy = {}\nomni_range = {}\nshadow_enabled = false", transform(Mat3::IDENTITY, Vec3::from_array(l.pos)), color(linear_to_srgb(l.color)), f(l.intensity()), f(l.range)),
                );
            }
        }
    }
    // the dungeons' ways in: their glowing sheets, and a trigger each that loads the dungeon
    if let Some(sheets) = &dungeons.sheets {
        let glb = sc.ext("PackedScene", &res(sheets));
        sc.node(
            &format!("[node name=\"PortalSheets\" parent=\".\" instance={glb}]"),
            "",
        );
    }
    if !dungeons.ways.is_empty() {
        let script = sc.ext("Script", &res("scripts/wb_portal.gd"));
        sc.node(
            "[node name=\"DungeonWaysIn\" type=\"Node3D\" parent=\".\"]",
            "",
        );
        for w in &dungeons.ways {
            sc.node(
                &format!(
                    "[node name=\"{}\" type=\"Node3D\" parent=\"DungeonWaysIn\"]",
                    w.name
                ),
                &portal_body(&script, w),
            );
        }
    }
    // the placed objects' fires (braziers, torch stands, fire pits, campfires): real fire and light
    fire_nodes(&mut sc, &s.fires, true);
    if nav_region {
        let nav = sc.ext("NavigationMesh", &res("navigation/navmesh.tres"));
        sc.node(
            "[node name=\"Navigation\" type=\"NavigationRegion3D\" parent=\".\"]",
            &format!("navigation_mesh = {nav}"),
        );
    }
    if instances {
        let script = sc.ext("Script", &res("scripts/wb_instances.gd"));
        sc.node(
            "[node name=\"Instances\" type=\"Node3D\" parent=\".\"]",
            &format!(
                "script = {script}\nindex_path = {}",
                quote(&res("instances/instances.json"))
            ),
        );
    }
    if grass {
        let script = sc.ext("Script", &res("scripts/wb_grass.gd"));
        sc.node(
            "[node name=\"Grass\" type=\"Node3D\" parent=\".\"]",
            &format!(
                "script = {script}\ntufts_path = {}\ndensity_path = {}\nheights_path = {}\nvertices = {}\ncell = {}",
                quote(&res("terrain/grass_tufts.glb")),
                quote(&res("terrain/grass.png")),
                quote(&res("terrain/heights.f32")),
                grass_grid.0,
                f(grass_grid.1)
            ),
        );
    }
    if !markers.is_empty() {
        sc.node("[node name=\"Gameplay\" type=\"Node3D\" parent=\".\"]", "");
        for m in markers {
            let name = node_name(m.kind, m.id, &m.name);
            // yaw 0 faces +X, counter-clockwise from above; a Godot node faces -Z
            let rot = Mat3::from_rotation_y(m.yaw - std::f32::consts::FRAC_PI_2);
            let mut body = format!(
                "transform = {}\ngizmo_extents = {}\nmetadata/wb_kind = {}\nmetadata/wb_id = {}\nmetadata/wb_name = {}\nmetadata/wb_type = {}\nmetadata/wb_radius = {}\nmetadata/wb_notes = {}",
                transform(rot, Vec3::from_array(m.pos)),
                f(m.radius.clamp(1.0, 50.0)),
                quote(m.kind),
                m.id,
                quote(&m.name),
                quote(&m.sub),
                f(m.radius),
                quote(&m.notes)
            );
            if !m.polygon.is_empty() {
                let pts: Vec<String> = m.polygon.iter().flat_map(|p| [f(p[0]), f(p[1])]).collect();
                body.push_str(&format!(
                    "\nmetadata/wb_polygon = PackedVector2Array({})",
                    pts.join(", ")
                ));
            }
            sc.node(
                &format!("[node name=\"{name}\" type=\"Marker3D\" parent=\"Gameplay\"]"),
                &body,
            );
            if m.kind != "fishing" {
                sc.node(
                    &format!("[node name=\"Label\" type=\"Label3D\" parent=\"Gameplay/{name}\"]"),
                    &format!("transform = {}\nbillboard = 1\npixel_size = 0.02\nfont_size = 48\noutline_size = 10\ntext = {}", transform(Mat3::IDENTITY, Vec3::new(0.0, 4.0, 0.0)), quote(&m.name)),
                );
            }
        }
    }
    // camera: at the player start if there is one, else where the editor camera was
    let (eye, look) = match s.gameplay.player_start.filter(|_| !markers.is_empty()) {
        Some(p) => {
            let pos = Vec3::from_array(p.pos) + Vec3::Y * 1.7;
            (pos, pos + Vec3::new(p.yaw.cos(), -0.05, -p.yaw.sin()))
        }
        None => s.camera,
    };
    let fwd = (look - eye).normalize_or(Vec3::NEG_Z);
    let zc = -fwd;
    let xc = Vec3::Y.cross(zc).normalize_or(Vec3::X);
    let yc = zc.cross(xc);
    sc.node(
        "[node name=\"Camera\" type=\"Camera3D\" parent=\".\"]",
        &format!("transform = {}\ncurrent = true\nfov = 50.0\nnear = 0.1\nfar = {}\nscript = {camera_script}\nsea_level = {}", transform(Mat3::from_cols(xc, yc, zc), eye), f(t.size * 3.0), f(t.sea)),
    );
    sc.node(
        "[node name=\"Hints\" type=\"CanvasLayer\" parent=\".\"]",
        "",
    );
    sc.node(
        "[node name=\"Label\" type=\"Label\" parent=\"Hints\"]",
        "offset_left = 16.0\noffset_top = 12.0\noffset_right = 900.0\noffset_bottom = 40.0\ntheme_override_colors/font_outline_color = Color(0, 0, 0, 1)\ntheme_override_constants/outline_size = 6\ntext = \"WASD move · Q/E down/up · right mouse or click to look · Shift fast · F fly/walk · Esc free mouse · H hide\"",
    );
    sc.finish()
}

// ---------------------------------------------------------------------------------------------
// dungeons

/// A trigger between the world and a dungeon (`wb_portal.gd`).
struct PortalNode {
    name: String,
    target: String,
    via: u32,
    center: Vec3,
    rot: Quat,
    half: Vec3,
    /// where players come out in the target: eye position, look
    arrive: (Vec3, Vec3),
    /// a dungeon's way out: per way in, where players come back out in the world
    arrivals: Vec<(u32, Vec3, Vec3)>,
}

#[derive(Default)]
struct DungeonsOut {
    /// the world's portal sheets (under the data folder)
    sheets: Option<String>,
    ways: Vec<PortalNode>,
}

fn v3(v: Vec3) -> String {
    format!("Vector3({}, {}, {})", f(v.x), f(v.y), f(v.z))
}

fn portal_body(script: &str, w: &PortalNode) -> String {
    let mut body = format!(
        "transform = {}\nscript = {script}\ntarget = {}\nhalf_size = {}\nvia = {}\narrive_position = {}\narrive_look = {}",
        transform(Mat3::from_quat(w.rot), w.center),
        quote(&w.target),
        v3(w.half),
        w.via,
        v3(w.arrive.0),
        v3(w.arrive.1)
    );
    if !w.arrivals.is_empty() {
        let items: Vec<String> = w
            .arrivals
            .iter()
            .map(|(k, p, l)| format!("\"{k}\": [{}, {}]", v3(*p), v3(*l)))
            .collect();
        body.push_str(&format!("\narrivals = {{{}}}", items.join(", ")));
    }
    body
}

/// Every dungeon as a scene of its own (`dungeon_<name>.tscn`) with its files under
/// `dungeons/<folder>/`; what the world scene needs to link to them.
fn dungeon_scenes(s: &Snapshot, ctx: &Ctx, models: &[ModelOut]) -> Result<DungeonsOut, String> {
    if super::dungeons::list(s)?.is_empty() {
        return Ok(DungeonsOut::default());
    }
    // (lightmap-ready: LightmapGI bakes the halls' bounce light)
    let (written, sheets) = super::dungeons::write_all_with(
        s,
        ctx,
        models,
        "dungeons",
        "-col",
        &|rel, b| write_glb_with(ctx, rel, &b, false, 0.25),
        &|rel, b| ctx.write(&format!("{DATA}/{rel}"), b),
    )?;
    let eye = Vec3::Y * 1.7;
    let mut out = DungeonsOut {
        sheets,
        ways: vec![],
    };
    let mut scenes = vec![];
    for w in &written {
        let d = w.d;
        let Some(glb) = &w.glb else { continue };
        let scene_file = format!("dungeon_{}.tscn", w.safe);
        // its objects
        let bins = format!("dungeons/{}/instances", d.slug);
        let mut entries = vec![];
        for (m, list) in models.iter().zip(&w.inst) {
            if !list.is_empty() {
                entries.push(model_entry_in(s, ctx, m, list, &bins)?);
            }
        }
        let index = format!("dungeons/{}/instances.json", d.slug);
        ctx.write_json(&format!("{DATA}/{index}"), &json!({ "models": entries }))?;
        ctx.write_json(&format!("{DATA}/dungeons/{}/dungeon.json", d.slug), &w.json)?;
        let start = d
            .start
            .map(|(p, l)| (p + eye, l))
            .unwrap_or((eye, Vec3::NEG_Z));
        // the way back out: to the world, at the way in players came through
        let exit = d.exit.as_ref().map(|t| PortalNode {
            name: "WayOut".into(),
            target: "res://world.tscn".into(),
            via: 0,
            center: t.center,
            rot: t.rot,
            half: t.half,
            arrive: d
                .ways_in
                .first()
                .map(|w| (w.arrive.0 + eye, w.arrive.1))
                .unwrap_or((eye, Vec3::NEG_Z)),
            arrivals: d
                .ways_in
                .iter()
                .map(|w| (w.entrance, w.arrive.0 + eye, w.arrive.1))
                .collect(),
        });
        let scene = dungeon_scene(s, d, glb, &index, start, exit.as_ref(), !entries.is_empty());
        ctx.write(&scene_file, scene.as_bytes())?;
        for wi in &d.ways_in {
            out.ways.push(PortalNode {
                name: format!("{}_{}", w.safe, wi.entrance),
                target: format!("res://{scene_file}"),
                via: wi.entrance,
                center: wi.trigger.center,
                rot: wi.trigger.rot,
                half: wi.trigger.half,
                arrive: start,
                arrivals: vec![],
            });
        }
        scenes.push((w.d.slug.clone(), scene_file));
    }
    let index = super::dungeons::index_json(
        s,
        &written,
        out.sheets.as_ref().map(|p| res(p)).as_deref(),
        |w| json!({ "scene": scenes.iter().find(|x| x.0 == w.d.slug).map(|x| format!("res://{}", x.1)) }),
    );
    ctx.write_json(&format!("{DATA}/dungeons/dungeons.json"), &index)?;
    Ok(out)
}

/// One dungeon's scene: its mood (environment, fog, volumetric haze), architecture (trimesh
/// colliders from the `-col` nodes), lights, objects, the way back out and a walking camera.
fn dungeon_scene(
    s: &Snapshot,
    d: &crate::dungeons::export::DungeonExport,
    glb: &str,
    index: &str,
    start: (Vec3, Vec3),
    exit: Option<&PortalNode>,
    instances: bool,
) -> String {
    let mut sc = Scene {
        ext: vec![],
        sub: vec![],
        nodes: vec![],
    };
    let camera_script = sc.ext("Script", &res("scripts/wb_camera.gd"));
    let mood = d.map.as_ref().map(|m| m.mood.clone()).unwrap_or_default();
    let lin = |c: [f32; 3]| {
        let l = Color::srgb_from_array(c).to_linear();
        [l.red, l.green, l.blue]
    };
    let haze = mood.haze.clamp(0.0, 1.0);
    let sky = d
        .map
        .as_ref()
        .map(crate::dungeons::export::sky_json)
        .filter(|v| v["mode"] != "none");
    let background = match &sky {
        Some(v) => {
            let night = v["mode"] == "night";
            let (top, horizon) = if night {
                ("Color(0.01, 0.015, 0.04, 1)", "Color(0.03, 0.04, 0.08, 1)")
            } else if v["mode"] == "dusk" {
                ("Color(0.2, 0.25, 0.45, 1)", "Color(0.95, 0.55, 0.35, 1)")
            } else {
                ("Color(0.3, 0.5, 0.85, 1)", "Color(0.72, 0.8, 0.9, 1)")
            };
            let mat = sc.sub("ProceduralSkyMaterial", &format!("sky_top_color = {top}\nsky_horizon_color = {horizon}\nground_bottom_color = Color(0.05, 0.05, 0.05, 1)\nground_horizon_color = {horizon}"));
            let s = sc.sub("Sky", &format!("sky_material = {mat}"));
            format!("background_mode = 2\nsky = {s}")
        }
        None => "background_mode = 1\nbackground_color = Color(0, 0, 0, 1)".to_string(),
    };
    let env = sc.sub(
        "Environment",
        &format!(
            "{background}\nambient_light_source = 2\nambient_light_color = {ambient}\nambient_light_energy = {ambient_energy}\nreflected_light_source = 1\n\
             tonemap_mode = 2\ntonemap_exposure = {exposure}\ntonemap_white = 6.0\nssao_enabled = true\nssao_radius = 1.2\nssao_intensity = 2.0\nssil_enabled = true\n\
             sdfgi_enabled = true\nsdfgi_use_occlusion = true\nsdfgi_bounce_feedback = 0.6\n\
             glow_enabled = true\nglow_intensity = 0.8\nglow_bloom = 0.1\n\
             fog_enabled = true\nfog_light_color = {fog}\nfog_density = {fog_density}\nfog_sky_affect = 0.0\n\
             volumetric_fog_enabled = {vol}\nvolumetric_fog_density = {vol_density}\nvolumetric_fog_albedo = {fog}\nvolumetric_fog_length = 48.0",
            ambient = color(linear_to_srgb(lin(mood.fill))),
            ambient_energy = f((mood.fill_strength * 0.6).clamp(0.02, 1.5)),
            exposure = f(2f32.powf(mood.exposure).clamp(0.25, 4.0)),
            fog = color(linear_to_srgb(lin(mood.fog))),
            fog_density = f(0.003 + mood.fog_density.clamp(0.0, 1.0).powi(2) * 0.045),
            vol = haze > 0.05,
            vol_density = f(0.004 + haze * 0.03),
        ),
    );
    sc.node(
        "[node name=\"Dungeon\" type=\"Node3D\"]",
        &format!("metadata/wb_dungeon = {}", quote(&d.name)),
    );
    sc.node(
        "[node name=\"WorldEnvironment\" type=\"WorldEnvironment\" parent=\".\"]",
        &format!("environment = {env}"),
    );
    // under a sky: its sun (or moon), the walls casting their shadows into the open rooms
    if let Some(v) = &sky {
        let f3 = |k: &str| -> Vec3 {
            Vec3::new(
                v[k][0].as_f64().unwrap_or(0.0) as f32,
                v[k][1].as_f64().unwrap_or(-1.0) as f32,
                v[k][2].as_f64().unwrap_or(0.0) as f32,
            )
        };
        // a directional light shines along its -Z
        let z = -f3("light_direction").normalize_or(Vec3::NEG_Y);
        let x = Vec3::Y.cross(z).try_normalize().unwrap_or(Vec3::X);
        sc.node(
            "[node name=\"Sun\" type=\"DirectionalLight3D\" parent=\".\"]",
            &format!("transform = {}\nlight_color = {}\nlight_energy = {}\nshadow_enabled = true\ndirectional_shadow_max_distance = 250.0", transform(Mat3::from_cols(x, z.cross(x), z), Vec3::new(0.0, 60.0, 0.0)), color(f3("color").to_array()), f(v["intensity"].as_f64().unwrap_or(1.0) as f32)),
        );
    }
    let arch = sc.ext(
        "PackedScene",
        &res(glb.trim_start_matches(&format!("{DATA}/"))),
    );
    sc.node(
        &format!("[node name=\"Architecture\" parent=\".\" instance={arch}]"),
        "",
    );
    // the lights: the brightest few cast shadows; fires flicker; hazy ones light the haze
    if let Some(m) = &d.map {
        let flicker = sc.ext("Script", &res("scripts/wb_flicker.gd"));
        sc.node("[node name=\"Lights\" type=\"Node3D\" parent=\".\"]", "");
        let mut order: Vec<usize> = (0..m.lights.len()).collect();
        order.sort_by(|a, b| m.lights[*b].lumens.total_cmp(&m.lights[*a].lumens));
        let shadowed: std::collections::HashSet<usize> = order
            .iter()
            .copied()
            .filter(|k| m.lights[*k].shadows)
            .take(12)
            .collect();
        for (k, l) in m.lights.iter().enumerate() {
            let c = color(linear_to_srgb(lin(m.mood.tinted(l.color))));
            let energy = ((l.lumens / 300_000.0).sqrt() * 1.5).clamp(0.4, 8.0);
            let fog_energy = if l.haze { 1.5 } else { 0.0 };
            let (ty, basis, extra) = if l.cone > 0.0 {
                // a spot shines along its -Z
                let z = -l.direction();
                let x = Vec3::Y.cross(z).try_normalize().unwrap_or(Vec3::X);
                (
                    "SpotLight3D",
                    Mat3::from_cols(x, z.cross(x), z),
                    format!(
                        "spot_range = {}\nspot_angle = {}",
                        f(l.range),
                        f((l.cone * 0.5).clamp(1.0, 89.0))
                    ),
                )
            } else {
                (
                    "OmniLight3D",
                    Mat3::IDENTITY,
                    format!("omni_range = {}", f(l.range)),
                )
            };
            let mut body = format!(
                "transform = {}\nlight_color = {c}\nlight_energy = {}\nlight_volumetric_fog_energy = {}\nshadow_enabled = {}\n{extra}",
                transform(basis, Vec3::from_array(l.pos)),
                f(energy),
                f(fog_energy),
                shadowed.contains(&k)
            );
            if l.flicker > 0.01 {
                body.push_str(&format!(
                    "\nscript = {flicker}\nbase_energy = {}\namount = {}\nseed = {}",
                    f(energy),
                    f(l.flicker),
                    l.id
                ));
            }
            sc.node(
                &format!(
                    "[node name=\"{}_{}\" type=\"{ty}\" parent=\"Lights\"]",
                    l.kind.id(),
                    l.id
                ),
                &body,
            );
        }
    }
    if instances {
        let script = sc.ext("Script", &res("scripts/wb_instances.gd"));
        sc.node(
            "[node name=\"Instances\" type=\"Node3D\" parent=\".\"]",
            &format!("script = {script}\nindex_path = {}", quote(&res(index))),
        );
    }
    // the fires' flames (their lights are the dungeon's)
    fire_nodes(&mut sc, &d.fires, false);
    // cracks, stains, drips, blood, soot, runes: Decal nodes projecting down their -Y onto the surfaces
    if let Some(m) = d.map.as_ref().filter(|m| !m.decals.is_empty()) {
        sc.node("[node name=\"Decals\" type=\"Node3D\" parent=\".\"]", "");
        let mut pics: std::collections::HashMap<crate::dungeons::decals::DecalKind, String> =
            Default::default();
        for x in &m.decals {
            let pic = pics
                .entry(x.kind)
                .or_insert_with(|| {
                    sc.ext(
                        "Texture2D",
                        &res(&format!("dungeons/decals/{}.png", x.kind.id())),
                    )
                })
                .clone();
            let tf = x.transform();
            let mut body = format!(
                "transform = {}\nsize = Vector3({}, 0.6, {})\ntexture_albedo = {pic}\nmodulate = {}\nupper_fade = 0.3\nlower_fade = 0.3\ncull_mask = 1",
                transform(Mat3::from_quat(tf.rotation), tf.translation),
                f(x.size),
                f(x.size),
                color(x.color)
            );
            if x.glow > 0.0 {
                body.push_str(&format!(
                    "\ntexture_emission = {pic}\nemission_energy = {}",
                    f(x.glow)
                ));
            }
            sc.node(
                &format!(
                    "[node name=\"{}_{}\" type=\"Decal\" parent=\"Decals\"]",
                    x.kind.id(),
                    x.id
                ),
                &body,
            );
        }
    }
    // doors, gates, levers, plates and traps (their parts are in the architecture's GLB; the
    // script works them), and the game's markers (arenas, checkpoints, loot, spawn spots)
    if let Some(m) = d.map.as_ref().filter(|m| !m.pieces.is_empty()) {
        let safe = super::dungeons::safe(d);
        let pose = |t: Transform| transform(Mat3::from_quat(t.rotation), t.translation);
        if m.pieces.iter().any(|p| !p.kind.is_marker()) {
            let script = sc.ext("Script", &res("scripts/wb_piece.gd"));
            sc.node("[node name=\"Pieces\" type=\"Node3D\" parent=\".\"]", "");
            for p in m.pieces.iter().filter(|p| !p.kind.is_marker()) {
                let parts = crate::dungeons::pieces::parts(p);
                let depth = if p.kind.in_doorway() { 0.24 } else { p.width };
                let poses = |open: bool| {
                    parts
                        .iter()
                        .map(|x| pose(if open { x.open } else { x.closed }))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                sc.node(
                    &format!("[node name=\"Piece{}\" type=\"Node3D\" parent=\"Pieces\"]", p.id),
                    &format!(
                        "script = {script}\nkind = {}\nnode_name = {}\nstart_open = {}\nblocks = {}\nusable = {}\nsize = Vector3({}, {}, {})\nclosed = [{}]\nopened = [{}]\nlinks = [{}]\nseconds = {}\nlabel = {}",
                        quote(p.kind.id()),
                        quote(&super::dungeons::piece_node(&safe, p.id)),
                        p.open,
                        p.kind.in_doorway(),
                        p.kind.usable(),
                        f(p.width),
                        f(p.height.max(0.05)),
                        f(depth),
                        poses(false),
                        poses(true),
                        p.links.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", "),
                        f(if p.kind == crate::dungeons::pieces::PieceKind::Portcullis { 2.2 } else { 0.9 }),
                        quote(&p.label),
                    ),
                );
            }
        }
        if m.pieces.iter().any(|p| p.kind.is_marker()) {
            sc.node("[node name=\"Markers\" type=\"Node3D\" parent=\".\"]", "");
            for p in m.pieces.iter().filter(|p| p.kind.is_marker()) {
                sc.node(
                    &format!("[node name=\"{}{}\" type=\"Marker3D\" parent=\"Markers\"]", p.kind.name().replace(' ', ""), p.id),
                    &format!("transform = {}\ngizmo_extents = {}\nmetadata/wb_kind = {}\nmetadata/wb_width = {}\nmetadata/wb_label = {}", transform(Mat3::from_quat(p.rotation()), p.position()), f((p.width * 0.5).max(0.25)), quote(p.kind.id()), f(p.width), quote(&p.label)),
                );
            }
        }
    }
    if let Some(x) = exit {
        let script = sc.ext("Script", &res("scripts/wb_portal.gd"));
        sc.node(
            "[node name=\"WayOut\" type=\"Node3D\" parent=\".\"]",
            &portal_body(&script, x),
        );
    }
    sc.node(
        "[node name=\"PlayerStart\" type=\"Marker3D\" parent=\".\"]",
        &format!(
            "transform = {}",
            transform(Mat3::IDENTITY, start.0 - Vec3::Y * 1.7)
        ),
    );
    let fwd = start.1.try_normalize().unwrap_or(Vec3::NEG_Z);
    let zc = -fwd;
    let xc = Vec3::Y.cross(zc).try_normalize().unwrap_or(Vec3::X);
    sc.node(
        "[node name=\"Camera\" type=\"Camera3D\" parent=\".\"]",
        &format!("transform = {}\ncurrent = true\nfov = 60.0\nnear = 0.05\nfar = 600.0\nscript = {camera_script}\nsea_level = -100000.0\nfly_speed = 12.0\nstart_walking = true\nray_from_eye = true\nstop_at_walls = true", transform(Mat3::from_cols(xc, zc.cross(xc), zc), start.0)),
    );
    sc.node(
        "[node name=\"Hints\" type=\"CanvasLayer\" parent=\".\"]",
        "",
    );
    sc.node(
        "[node name=\"Label\" type=\"Label\" parent=\"Hints\"]",
        &format!("offset_left = 16.0\noffset_top = 12.0\noffset_right = 900.0\noffset_bottom = 40.0\ntheme_override_colors/font_outline_color = Color(0, 0, 0, 1)\ntheme_override_constants/outline_size = 6\ntext = {}", quote(&format!("{} · WASD walk · mouse look · F fly/walk · Shift fast · E open / pull · the portal behind you leads back to {}", d.name, s.name))),
    );
    // what E would do (wb_piece.gd fills it in)
    sc.node(
        "[node name=\"Use\" type=\"Label\" parent=\"Hints\"]",
        "visible = false\nanchors_preset = 8\nanchor_left = 0.5\nanchor_top = 0.5\nanchor_right = 0.5\nanchor_bottom = 0.5\noffset_left = -220.0\noffset_top = 46.0\noffset_right = 220.0\noffset_bottom = 76.0\nhorizontal_alignment = 1\ntheme_override_colors/font_outline_color = Color(0, 0, 0, 1)\ntheme_override_constants/outline_size = 6",
    );
    sc.finish()
}

/// The fires' flames: AetherFX's fire baked to flipbooks, a quad each turning to the camera
/// (`wb_flame.gdshader`; a material per fire and colour) under a `Fires` node; `lights`: the
/// fires that throw a light of their own get a flickering OmniLight3D.
fn fire_nodes(sc: &mut Scene, fires: &[crate::flames::Fire], lights: bool) {
    let list = crate::flames::fires_json(fires);
    if list.is_empty() {
        return;
    }
    let shader = sc.ext("Shader", &res("scripts/wb_flame.gdshader"));
    let flicker = sc.ext("Script", &res("scripts/wb_flicker.gd"));
    let quad = sc.sub("QuadMesh", "center_offset = Vector3(0, 0.5, 0)");
    sc.node("[node name=\"Fires\" type=\"Node3D\" parent=\".\"]", "");
    let mut mats: std::collections::HashMap<String, String> = Default::default();
    let mut sheets: std::collections::HashMap<String, String> = Default::default();
    for (k, x) in list.iter().enumerate() {
        let Some(kind) = crate::flames::FireKind::ALL
            .into_iter()
            .find(|f| x["fire"] == f.id())
        else {
            continue;
        };
        let id = kind.id();
        let s = kind.sheet();
        let num = |v: &Value| v.as_f64().unwrap_or(0.0) as f32;
        let foot = Vec3::new(num(&x["foot"][0]), num(&x["foot"][1]), num(&x["foot"][2]));
        let c = [
            num(&x["color_linear"][0]),
            num(&x["color_linear"][1]),
            num(&x["color_linear"][2]),
        ];
        let magic = num(&x["magic"]);
        let mx = c[0].max(c[1]).max(c[2]).max(1e-3);
        let key = format!("{id}_{:.2}_{:.2}_{:.2}_{magic:.2}", c[0], c[1], c[2]);
        let sheet = sheets
            .entry(id.to_string())
            .or_insert_with(|| sc.ext("Texture2D", &res(&format!("flames/{id}.png"))))
            .clone();
        let mat = mats
            .entry(key)
            .or_insert_with(|| {
                sc.sub(
                    "ShaderMaterial",
                    &format!(
                        "render_priority = 1\nshader = {shader}\nshader_parameter/sheet = {sheet}\nshader_parameter/grid = Vector4({}, {}, {}, {})\nshader_parameter/strength = {}\nshader_parameter/tint = Vector4({}, {}, {}, {})\nshader_parameter/soft = {}",
                        s.columns,
                        s.rows,
                        s.frames,
                        f(s.fps),
                        f(num(&x["strength"])),
                        f(c[0] / mx),
                        f(c[1] / mx),
                        f(c[2] / mx),
                        f(magic),
                        f(if kind == crate::flames::FireKind::Candle { 0.03 } else { 0.18 }),
                    ),
                )
            })
            .clone();
        let (w, h) = (num(&x["frame"][0]), num(&x["frame"][1]));
        let at = Vec3::new(foot.x, num(&x["frame_bottom"]), foot.z);
        sc.node(
            &format!("[node name=\"{id}_{k}\" type=\"MeshInstance3D\" parent=\"Fires\"]"),
            &format!("transform = {}\nmesh = {quad}\nmaterial_override = {mat}\ncast_shadow = 0\ninstance_shader_parameters/seed = {}\ninstance_shader_parameters/spread = {}", transform(Mat3::from_diagonal(Vec3::new(w, h, 1.0)), at), f(((k as f32) * 0.618_034).fract()), f(x["spread"].as_f64().unwrap_or(1.0) as f32)),
        );
        if lights && x["light"] == true {
            let energy = ((num(&x["light_lumens"]) / 300_000.0).sqrt() * 1.5).clamp(0.2, 4.0);
            let p = Vec3::new(
                num(&x["light_position"][0]),
                num(&x["light_position"][1]),
                num(&x["light_position"][2]),
            );
            sc.node(
                &format!("[node name=\"{id}_{k}_light\" type=\"OmniLight3D\" parent=\"Fires\"]"),
                &format!("transform = {}\nlight_color = Color(1, 0.58, 0.26, 1)\nlight_energy = {}\nomni_range = {}\nscript = {flicker}\nbase_energy = {}\namount = 0.4\nseed = {k}", transform(Mat3::IDENTITY, p), f(energy), f(num(&x["light_range"])), f(energy)),
            );
        }
    }
}

/// Some of the world lies below sea level (the ocean is drawn).
fn has_sea(s: &Snapshot) -> bool {
    s.terrain.heights.iter().any(|h| *h < s.terrain.sea_level)
}

fn project(s: &Snapshot) -> String {
    format!(
        r#"; Engine configuration file, written by Skyfold Studio.

config_version=5

[application]

config/name={name}
config/description="Exported from Skyfold Studio"
run/main_scene="res://world.tscn"
config/features=PackedStringArray("4.3", "Forward Plus")
config/icon="res://icon.svg"

[display]

window/size/viewport_width=1600
window/size/viewport_height=900

[rendering]

lights_and_shadows/directional_shadow/size=8192
lights_and_shadows/directional_shadow/soft_shadow_filter_quality=3
anti_aliasing/quality/msaa_3d=2
textures/default_filters/anisotropic_filtering_level=4
"#,
        name = quote(&s.name)
    )
}

const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128"><rect width="128" height="128" rx="24" fill="#3d7f5a"/><path d="M14 100 L50 44 L68 70 L82 52 L114 100 Z" fill="#e8f0e8"/></svg>
"##;

const README: &str = r#"# {name} — Godot 4 project

Exported from Skyfold Studio. Open it with **Godot 4.3 or newer**: in the Project Manager choose
*Import*, pick this folder's `project.godot`, open it (the first import takes a minute) and press
**F5**.

Controls: **WASD** move, **Q / E** down / up, **right mouse** (or click to capture the mouse) look,
**Shift** fast, **F** switches between flying and walking, **Esc** frees the mouse, **H** hides the
hints.

## What's in the scene (`world.tscn`)

- **WorldEnvironment**: physical sky, filmic tonemap, SSAO, glow, the world's haze as fog and its
  colour grade (contrast, saturation); **Sun**: a DirectionalLight3D at the world's time of day.
- **Terrain** (`world_builder/scripts/wb_terrain.gd`): per tile (one up to 4097 samples,
  2049-sample tiles beyond, listed in `terrain/tiles.json`) the chunk meshes of
  `terrain_<x>_<z>.glb`, each chunk with its own levels of detail (skirted, so neighbours at
  different levels never crack; switch distances from their measured height error in
  `terrain_lods_<x>_<z>.json`), the splat material `terrain_<x>_<z>.tres` (shader
  `world_builder/shaders/terrain.gdshader`: the world's photoscan layers as texture arrays,
  height blending, triplanar rock, anti-tiling) and a HeightMapShape3D collider.
- **Ocean** and **Water** (`wb_water.gd`): the sea, lakes, rivers and waterfalls with
  `water.gdshader` (depth-based colour and clarity, refraction, ripples, shore foam, swell), and
  swim volumes: Area3D boxes (group `wb_water_volume`, `wb_surface` metadata) for lakes, river
  stretches and the ocean.
- **Instances** (`wb_instances.gd`): trees, plants, rocks and props as MultiMeshInstance3D per
  model, variant, map cell and level of detail (visibility ranges with fades; the far LOD in
  coarse cells that hand over to finer cells near the camera through `visibility_parent`). Every
  model has levels of detail (packed Poly Haven / Skyborn / hand-painted / imported models are
  simplified within a pixel of error; Godot's own automatic LODs are on for everything but the
  leaf-card trees and plants) and collides through its **collision proxy**: trunk capsules,
  convex hulls for props and rocks, simplified shapes for buildings (floors and stairs within a
  few centimetres), with a PhysicsMaterial for its footstep surface. Buildings import with
  lightmap UV2s (`light_baking` = static lightmaps).
- **Grass** (`wb_grass.gd`, in game): photoscanned grass tufts scattered around the camera from
  the world's grass map.
- **Roads**: textured road meshes (`world_builder/roads/roads.glb`).
- **Caves**: caves, mines, sinkholes and rock arches (`world_builder/caves/caves.glb`, one node
  per cave with a trimesh collider). The terrain is open where they cut the ground (no triangles,
  and the height-map collider drops away there); `world_builder/caves/caves.json` lists each
  cave's entrance, chambers and dungeon portal.
- **Dungeons**: each underground dungeon is a scene of its own, `dungeon_<name>.tscn`. It holds:
  - its architecture (`world_builder/dungeons/<folder>/dungeon.glb`: trimesh colliders on walls,
    floors, stairs and ceilings; light-baking UVs for a LightmapGI);
  - its lights (the brightest cast shadows, fires flicker with `wb_flicker.gd`, hazy ones light
    the volumetric fog);
  - its objects, its mood (ambient fill, fog, volumetric haze, exposure);
  - its fires (`Fires`: AetherFX's flames as flipbooks on quads turning to the camera,
    `wb_flame.gdshader`, sheets in `world_builder/flames/`);
  - its doors, gates, bars, secret doors, levers, plates and traps (`Pieces`, `wb_piece.gd`:
    their parts are nodes of the dungeon's GLB; on foot E works the one looked at, a lever or
    plate works what it's linked to, shut ones block; call `work()` from your code) and the
    game's markers (`Markers`: arenas, checkpoints, loot and spawn spots with `wb_*` metadata);
  - its minimaps (`world_builder/dungeons/<folder>/minimap_L<level>.png`, where each lies in
    `dungeon.json`);
  - its way out, and a camera walking from its start.

  In `world.tscn` the ways in are `DungeonWaysIn` nodes and the portal sheets are
  `PortalSheets`. Walking into a way in loads the dungeon; its way out brings you back in front
  of the way in you came through (`wb_portal.gd`). Each dungeon's lights, mood, rooms and
  doorways are also in `world_builder/dungeons/<folder>/dungeon.json`
  (`world_builder/dungeons/dungeons.json` lists them all).
- **Gameplay**: Marker3D per player start, POI, NPC, spawn region (outline in `wb_polygon`),
  resource node and fishing spot; details in the `wb_*` metadata and
  `world_builder/data/gameplay.json`.
- **Navigation**: a NavigationRegion3D with Skyfold Studio's navmesh
  (`world_builder/navigation/navmesh.tres`: walkable ground, roads and water; one layer, so cave
  floors and upper storeys aren't in it — rebake the region from the colliders for those).
- **CrystalLights**: the caves' glowing crystals as OmniLight3Ds.
- **Camera** with `wb_camera.gd`.

The scripts are `@tool` scripts: they build their content when the scene loads, in the editor
and in game, as internal children that are never saved into `world.tscn`. Edit the data in
`world_builder/` (or replace the scripts with your own systems). `manifest.json` lists models,
LOD distances, terrain scale and lighting values; `CREDITS.md` lists the CC0 sources. Model
textures are written once into `world_builder/textures/` (the GLBs refer to them).
`world_builder/data/gameplay.json` is the documented server format (`docs/server-data.md`);
`server/navmesh/` (not imported by Godot) has the navmesh files for game servers.

Headless check: `godot --headless --path . --script res://world_builder/scripts/wb_check.gd`.
"#;
