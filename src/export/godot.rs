//! Godot 4 target: a complete project (`project.godot` + `world.tscn`) that opens directly in
//! Godot 4.4 or newer. The scene holds the WorldEnvironment (sky shader, filmic tonemap, haze
//! fog, grade), the sun (and moon) at the world's time of day, the weather and atmosphere runtime
//! (`wb_atmosphere.gd`: zone / biome weather, clouds, rain and snow, fog and mist), the ocean,
//! Marker3D gameplay markers and a
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
const GRASS_SHADER: &str = include_str!("godot/wb_grass.gdshader");
const PORTAL_GD: &str = include_str!("godot/wb_portal.gd");
const FLICKER_GD: &str = include_str!("godot/wb_flicker.gd");
const PIECE_GD: &str = include_str!("godot/wb_piece.gd");
const FLAME_SHADER: &str = include_str!("godot/wb_flame.gdshader");
const SKY_SHADER: &str = include_str!("godot/wb_sky.gdshader");
const POST_SHADER: &str = include_str!("godot/wb_post.gdshader");

pub(super) mod look;
const ATMOSPHERE_GD: &str = include_str!("godot/wb_atmosphere.gd");

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
/// sunrise / sunset, dark blue at night; haze pales the horizon. The day colours (with the 0.5
/// day energy in `Night::sky_energy`) are fitted to the editor's sky measured side by side.
pub(super) fn sky_colors(light: &core::LightOut, haze: f32) -> (Vec3, Vec3) {
    let sun_y = light.sun_dir.y;
    let night = light.night;
    let sunset = (1.0 - (sun_y * 4.0).abs()).clamp(0.0, 1.0) * (1.0 - night);
    let haze = haze.clamp(0.0, 1.0);
    let day_top = Vec3::new(0.15, 0.27, 0.43).lerp(Vec3::new(0.33, 0.42, 0.55), haze);
    let day_horizon = Vec3::new(0.13, 0.229, 0.371).lerp(Vec3::new(0.6, 0.65, 0.7), haze);
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

/// `glow` scales the emission (`Night::emission`).
fn water_params(st: &crate::water::WaterStyle, ocean: bool, river: bool, glow: f32) -> Value {
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
        "emissive_strength": st.emissive_strength * glow,
        "metallic_amount": st.metallic,
        "sea_hs": crate::ocean::SeaState::at((st.waves * 1.6).clamp(0.0, 4.0)).hs(),
        "molten": st.behaviour == crate::water::WaterBehaviour::Molten,
        "extinct": crate::water::water_optics(st).0.to_array(),
        "inscat": crate::water::water_optics(st).1.to_array(),
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

    // meadow grass: the editor's blade field (its patch meshes, grass map and settings), rebuilt
    // round the camera by `wb_grass.gd` / `wb_grass.gdshader`
    let mut grass = false;
    if o.plants && let Some(g) = core::grass(s, gn) {
        let px: Vec<u8> = g.into_iter().flatten().collect();
        ctx.write(
            &format!("{DATA}/terrain/grass.png"),
            &core::png_rgba(&image::RgbaImage::from_raw(gn as u32, gn as u32, px).ok_or("grass")?),
        )?;
        ctx.write(
            &format!("{DATA}/terrain/grass.png.import"),
            data_import("terrain/grass.png").as_bytes(),
        )?;
        write_grass(s, ctx, gn, t.size / (gn - 1) as f32)?;
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
        let glow = Night::of(s, &core::lighting(s)).emission();
        if let Some(b) = &glb {
            write_glb(ctx, "water/water.glb", b)?;
        }
        water_bodies = bodies.len();
        ctx.write(
            &format!("{DATA}/water/water_normal.png"),
            &core::water_normal_png(512),
        )?;
        // the editor's tiling noise and foam mat (its surf, lace and white water read them)
        for (name, img) in [
            ("noise", crate::ocean::noise_texture()),
            ("foam", crate::ocean::foam_texture()),
        ] {
            let n = img.texture_descriptor.size.width;
            let data = img.data.as_ref().ok_or("water texture")?;
            let base = data[..(n * n * 4) as usize].to_vec();
            let file = format!("water/{name}.png");
            ctx.write(
                &format!("{DATA}/{file}"),
                &core::png_rgba(&image::RgbaImage::from_raw(n, n, base).ok_or("water texture")?),
            )?;
            ctx.write(
                &format!("{DATA}/{file}.import"),
                data_import(&file)
                    .replace("mipmaps/generate=false", "mipmaps/generate=true")
                    .as_bytes(),
            )?;
        }
        let data = json!({
            "mesh": if glb.is_some() { res("water/water.glb") } else { String::new() },
            "shader": res("shaders/water.gdshader"),
            "normal_map": res("water/water_normal.png"),
            "noise": res("water/noise.png"),
            "heights": { "file": res("terrain/heights.f32"), "vertices": gn, "cell": t.size / (gn - 1) as f32 },
            "foam": res("water/foam.png"),
            "bodies": bodies.iter().map(|b| json!({ "kind": b.kind, "name": b.name, "node": b.node, "params": water_params(&b.style, false, b.kind != "lake", glow) })).collect::<Vec<_>>(),
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
    let canopy = write_canopy(s, ctx, &models, &inst)?;
    write_atmosphere(s, ctx, canopy)?;
    // the look's post-tonemap grade as a 3D colour-correction LUT
    let lut_n = look::LUT_SIZE;
    ctx.write(
        &format!("{DATA}/look/grade_lut.png"),
        &core::png_rgb(&look::grade_lut(&s.look)),
    )?;
    ctx.write(
        &format!("{DATA}/look/grade_lut.png.import"),
        format!(
            "[remap]\n\nimporter=\"3d_texture\"\ntype=\"CompressedTexture3D\"\n\n[deps]\n\nsource_file=\"{}\"\n\n[params]\n\ncompress/mode=0\ncompress/high_quality=false\ncompress/lossy_quality=0.7\ncompress/hdr_compression=1\ncompress/channel_pack=0\nmipmaps/generate=false\nmipmaps/limit=-1\nslices/horizontal={lut_n}\nslices/vertical=1\n",
            res("look/grade_lut.png")
        )
        .as_bytes(),
    )?;
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
        ("shaders/wb_grass.gdshader", GRASS_SHADER),
        ("scripts/wb_portal.gd", PORTAL_GD),
        ("scripts/wb_flicker.gd", FLICKER_GD),
        ("scripts/wb_piece.gd", PIECE_GD),
        ("scripts/wb_flame.gdshader", FLAME_SHADER),
        ("shaders/wb_sky.gdshader", SKY_SHADER),
        ("shaders/wb_post.gdshader", POST_SHADER),
        ("scripts/wb_atmosphere.gd", ATMOSPHERE_GD),
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
    // how much standing rain water each layer holds (`terrain.wesl` puddles), and snow
    let mut holds = vec![0.0f32; 12];
    let mut snow = vec![0.0f32; 12];
    for (k, l) in layers.iter().enumerate().take(12) {
        tiles[k] = l.tile_m;
        tri[k] = matches!(l.index, 6 | 7) as i32;
        holds[k] = [0.12, 0.1, 0.35, 0.9, 1.0, 0.7, 0.55, 0.55, 0.15, 0.0, 1.0, 0.0]
            .get(l.index)
            .copied()
            .unwrap_or(0.0);
        snow[k] = (l.index == 9) as u32 as f32;
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
shader_parameter/layer_holds = PackedFloat32Array({holds})
shader_parameter/layer_snow = PackedFloat32Array({snow})
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
        holds = list(holds.iter().map(|v| f(*v)).collect()),
        snow = list(snow.iter().map(|v| f(*v)).collect()),
        ox = f(origin[0]),
        oz = f(origin[1]),
        size = f(size),
        sea = f(t.sea),
    )
}

/// The editor's meadow for `wb_grass.gd`: its three blade patch meshes (raw buffers: positions,
/// uv, uv2, custom0, indices), the heights it stands on (`terrain/heights.f32`, `gn` a side,
/// `gcell` apart, divided by `gcell`) and its settings: the editor's colour set for the art style,
/// the grass amount, lushness, blade height and level-of-detail ranges.
fn write_grass(s: &Snapshot, ctx: &Ctx, gn: usize, gcell: f32) -> Result<(), String> {
    use bevy::mesh::VertexAttributeValues as V;
    let realistic = !s.painted;
    let mut meshes = vec![];
    for (lod, m) in crate::grass::export_patch_meshes(realistic).iter().enumerate() {
        let f3 = |a| match m.attribute(a) {
            Some(V::Float32x3(v)) => v.iter().flatten().copied().collect::<Vec<f32>>(),
            _ => vec![],
        };
        let f2 = |a| match m.attribute(a) {
            Some(V::Float32x2(v)) => v.iter().flatten().copied().collect::<Vec<f32>>(),
            _ => vec![],
        };
        let pos = f3(bevy::mesh::Mesh::ATTRIBUTE_POSITION);
        let uv = f2(bevy::mesh::Mesh::ATTRIBUTE_UV_0);
        let uv2 = f2(bevy::mesh::Mesh::ATTRIBUTE_UV_1);
        let col: Vec<f32> = match m.attribute(bevy::mesh::Mesh::ATTRIBUTE_COLOR) {
            Some(V::Float32x4(v)) => v.iter().flatten().copied().collect(),
            _ => vec![],
        };
        let idx: Vec<u32> = match m.indices() {
            Some(bevy::mesh::Indices::U32(i)) => i.clone(),
            Some(i) => i.iter().map(|v| v as u32).collect(),
            None => vec![],
        };
        let nv = pos.len() / 3;
        let mut bin: Vec<u8> = vec![];
        for part in [&pos, &uv, &uv2, &col] {
            bin.extend_from_slice(bytemuck::cast_slice(part));
        }
        bin.extend_from_slice(bytemuck::cast_slice(&idx));
        let file = format!("grass/patch_lod{lod}");
        ctx.write(&format!("{DATA}/{file}.bin"), &bin)?;
        ctx.write_json(
            &format!("{DATA}/{file}.json"),
            &json!({ "bin": res(&format!("{file}.bin")), "vertices": nv, "indices": idx.len() }),
        )?;
        meshes.push(res(&format!("{file}.json")));
    }
    // (base, lush tip, dry tip, cold tip, warm tip), linear, as `grass::update_params`
    let colours: [[f32; 3]; 5] = if s.painted {
        [[0.12, 0.2, 0.04], [0.38, 0.54, 0.1], [0.66, 0.5, 0.16], [0.2, 0.36, 0.15], [0.44, 0.56, 0.08]]
    } else {
        [[0.085, 0.115, 0.03], [0.3, 0.42, 0.1], [0.55, 0.45, 0.2], [0.16, 0.27, 0.12], [0.36, 0.5, 0.07]]
    };
    let k = s.detail.clamp(0.65, 1.5);
    let r = crate::grass::RANGES;
    ctx.write_json(
        &format!("{DATA}/grass/grass.json"),
        &json!({
            "shader": res("shaders/wb_grass.gdshader"),
            "meshes": meshes,
            "density": res("terrain/grass.png"),
            "heights": res("terrain/heights.f32"),
            "height_vertices": gn,
            "height_cell": gcell,
            "half": s.terrain.half(),
            "sea_level": s.terrain.sea_level,
            "ranges": [r[0] * k.min(1.0), r[1] * k.min(1.2), r[2] * k],
            "params": {
                "base_c": colours[0], "tip_c": colours[1], "dry_c": colours[2], "cold_c": colours[3], "warm_c": colours[4],
                "amount": s.grass_amount,
                "realistic": realistic as u32,
                "lushness": s.grass_lushness.clamp(0.0, 1.0),
                "height_scale": s.grass_height.clamp(0.3, 2.5),
            },
        }),
    )
}

/// The shade under the tree crowns for the whole world (`nature::shade`, whose map the editor
/// builds round the camera): every tree's crown disc splatted on a grid, the cover blurred over
/// about a crown's height, and the crowns' base and top heights. Written as `atmosphere/canopy.f32`
/// (sky visibility, crown top, crown base per texel). Returns its grid, or `None` with no trees.
fn write_canopy(
    s: &Snapshot,
    ctx: &Ctx,
    models: &[ModelOut],
    inst: &[Vec<Inst>],
) -> Result<Option<Value>, String> {
    struct Crown {
        c: Vec2,
        r: f32,
        base: f32,
        top: f32,
        alpha: f32,
    }
    let mut crowns = vec![];
    for (m, list) in models.iter().zip(inst) {
        if s.assets[m.asset].category != crate::objects::Category::Trees {
            continue;
        }
        for i in list {
            let Some(v) = m.variants.get(i.variant as usize).or(m.variants.first()) else {
                continue;
            };
            let height = v.height * i.scale;
            if height < 2.5 {
                continue;
            }
            crowns.push(Crown {
                c: Vec2::new(i.pos[0], i.pos[2]),
                r: (v.width * i.scale * 0.42).max(1.2),
                base: i.pos[1] + height * 0.3,
                top: i.pos[1] + height,
                alpha: 0.88,
            });
        }
    }
    if crowns.is_empty() || s.canopy_shade <= 0.0 {
        return Ok(None);
    }
    // the editor's 2 m texels where the map stays small, coarser for big worlds (blurs scaled)
    let size = s.terrain.half() * 2.0;
    let texel = (size / 2048.0).max(2.0);
    let n = (size / texel).ceil() as usize;
    let origin = Vec2::splat(-s.terrain.half());
    let mut trans = vec![1.0f32; n * n];
    let mut top = vec![0.0f32; n * n];
    let mut base = vec![0.0f32; n * n];
    let mut weight = vec![0.0f32; n * n];
    for c in &crowns {
        let g = (c.c - origin) / texel;
        let rt = c.r / texel;
        let (x0, x1) = ((g.x - rt).floor().max(0.0), (g.x + rt).ceil().min(n as f32 - 1.0));
        let (z0, z1) = ((g.y - rt).floor().max(0.0), (g.y + rt).ceil().min(n as f32 - 1.0));
        if x0 > x1 || z0 > z1 {
            continue;
        }
        for z in z0 as usize..=z1 as usize {
            for x in x0 as usize..=x1 as usize {
                let d = Vec2::new(x as f32 + 0.5, z as f32 + 0.5).distance(g) / rt;
                let w = 1.0 - crate::noise::smoothstep(0.7, 1.0, d);
                if w <= 0.0 {
                    continue;
                }
                let k = z * n + x;
                let a = c.alpha * w;
                trans[k] *= 1.0 - a;
                top[k] += c.top * a;
                base[k] += c.base * a;
                weight[k] += a;
            }
        }
    }
    let blur = |v: &mut Vec<f32>, metres: f32| {
        let r = ((metres / texel).round() as usize).max(1);
        // separable box blur
        let mut tmp = v.clone();
        for z in 0..n {
            let mut acc = 0.0;
            let row = z * n;
            for x in 0..=r.min(n - 1) {
                acc += v[row + x];
            }
            for x in 0..n {
                let lo = x.saturating_sub(r);
                let hi = (x + r).min(n - 1);
                tmp[row + x] = acc / (hi - lo + 1) as f32;
                if x + r + 1 < n {
                    acc += v[row + x + r + 1];
                }
                if x >= r {
                    acc -= v[row + x - r];
                }
            }
        }
        for x in 0..n {
            let mut acc = 0.0;
            for z in 0..=r.min(n - 1) {
                acc += tmp[z * n + x];
            }
            for z in 0..n {
                let lo = z.saturating_sub(r);
                let hi = (z + r).min(n - 1);
                v[z * n + x] = acc / (hi - lo + 1) as f32;
                if z + r + 1 < n {
                    acc += tmp[(z + r + 1) * n + x];
                }
                if z >= r {
                    acc -= tmp[(z - r) * n + x];
                }
            }
        }
    };
    let mut near: Vec<f32> = trans.iter().map(|t| 1.0 - t).collect();
    let mut wide = near.clone();
    for _ in 0..2 {
        // (the editor's radii at 2 m texels: 1, 5 and 6 texels)
        blur(&mut near, 2.0);
        blur(&mut wide, 10.0);
        blur(&mut top, 12.0);
        blur(&mut base, 12.0);
        blur(&mut weight, 12.0);
    }
    let k = 0.85 * s.canopy_shade.min(1.0);
    let mut out = Vec::with_capacity(n * n * 3);
    for i in 0..n * n {
        let cover = 0.6 * near[i] + 0.4 * wide[i];
        let (t, b) = if weight[i] > 1e-4 {
            (top[i] / weight[i], base[i] / weight[i])
        } else {
            (0.0, 0.0)
        };
        out.extend_from_slice(&[1.0 - k * cover.clamp(0.0, 1.0), t, b]);
    }
    ctx.write(
        &format!("{DATA}/atmosphere/canopy.f32"),
        bytemuck::cast_slice(&out),
    )?;
    Ok(Some(json!({
        "file": res("atmosphere/canopy.f32"),
        "origin": [origin.x, origin.y],
        "texel": texel,
        "size": n,
        // the editor's map ends ~350 m round the camera: its shade fades out there
        "fade": [300.0, 370.0],
    })))
}

/// The weather and atmosphere data `wb_atmosphere.gd` runs on: the zone and biome maps (the
/// terrain's own grid), every weather's parameters (`Weather::params`), each zone's weather,
/// clouds, wind, haze and lighting mood, each biome's weather and mood, the world's clouds, wind/// The distance fog colours the weather runtimes blend between (sRGB under the engine's
/// exposure): clear haze, storm, sunlit weather fog and dust, and the clear air and zone fog
/// densities.
pub(super) fn atmosphere_fog(s: &Snapshot, light: &core::LightOut, exposure: f32) -> Value {
    let lk = &s.look;
    let colour_haze = if light.haze_enabled { lk.color_haze.clamp(0.0, 1.0) } else { 0.0 };
    let haze = Haze::of(s, light, exposure);
    let storm = Vec3::new(0.42, 0.45, 0.5) * (1.0 - light.night * 0.9);
    let dust = Vec3::new(0.75, 0.6, 0.4) * (1.0 - light.night * 0.85);
    json!({
        "air": Haze::AIR,
        "zone_density": crate::sky::ZONE_FOG_DENSITY,
        "clear_color": haze.color,
        "storm_color": Haze::tone(s, storm, colour_haze, exposure),
        // weather fog is sunlit air: a light warm grey (calibrated against the editor's
        // fog, rain and snow), dark at night
        "weather_color": linear_to_srgb(
            (Vec3::new(0.80, 0.74, 0.64) * (1.0 - light.night * 0.9) * 0.36 / exposure)
                .min(Vec3::ONE)
                .to_array()
        ),
        "dust_color": Haze::tone(s, dust, colour_haze, exposure),
    })
}

/// The weather tables both engines' runtimes read: weathers by id (`clear`, `cloudy`, `rain`,
/// `storm`, `snow`, `fog`, `sandstorm`) with their parameters, the zones by map id (their weather,
/// clouds, wind, haze, lighting mood and play rules: entry banner, flying, mounts, rested area)
/// and the biome presets by map id (their weather and mood).
pub(super) fn weather_tables(
    s: &Snapshot,
) -> (
    serde_json::Map<String, Value>,
    serde_json::Map<String, Value>,
    serde_json::Map<String, Value>,
) {
    use crate::world::Weather;
    // weathers by id (`clear`, `cloudy`, `rain`, `storm`, `snow`, `fog`, `sandstorm`)
    let id = |w: Weather| format!("{w:?}").to_lowercase();
    let params = |w: Weather| {
        let p = w.params();
        json!({ "name": w.name(), "clouds": p.clouds, "fog": p.fog, "wet": p.wet, "rain": p.rain, "snow": p.snow, "dim": p.dim, "dust": p.dust })
    };
    let weathers: serde_json::Map<String, Value> =
        Weather::ALL.iter().map(|w| (id(*w), params(*w))).collect();
    let zones: serde_json::Map<String, Value> = s
        .zones
        .iter()
        .map(|z| {
            (
                z.id.to_string(),
                json!({
                    "name": z.name,
                    "weather_enabled": z.weather_enabled,
                    "weather": id(z.weather),
                    "intensity": z.weather_intensity,
                    "clouds_enabled": z.clouds_enabled,
                    "cloud_cover": z.cloud_cover,
                    "wind_enabled": z.wind_enabled,
                    "wind_speed": z.wind_speed,
                    "haze_mode": serde_json::to_value(z.haze.mode).unwrap_or(Value::Null),
                    "haze_amount": z.haze.amount,
                    "light": z.own_light(),
                    "banner": format!("{}  ·  {}", z.level_label(), z.zone_type.name()),
                    "level_min": z.level_min,
                    "level_max": z.level_max,
                    "allow_flying": z.allow_flying,
                    "allow_mounts": z.allow_mounts,
                    "rested_area": z.rested_area,
                    "temperature_c": z.temperature,
                }),
            )
        })
        .collect();
    let biomes: serde_json::Map<String, Value> = crate::biomes::BiomePreset::ALL
        .iter()
        .map(|b| {
            let d = b.def();
            let light = (d.mood != crate::world::Mood::Natural).then(|| d.mood.light());
            (
                b.id().to_string(),
                json!({ "name": d.name, "weather": id(d.weather), "intensity": d.weather_intensity, "light": light }),
            )
        })
        .collect();
    (weathers, zones, biomes)
}


/// and look, and the fog colours for clear, stormy and dusty air under this export's exposure.
fn write_atmosphere(s: &Snapshot, ctx: &Ctx, canopy: Option<Value>) -> Result<(), String> {
    use crate::world::Weather;
    let t = &s.terrain;
    let n = t.n();
    for (name, map) in [("zones", &t.zones), ("biomes", &t.biomes)] {
        let file = format!("atmosphere/{name}.png");
        ctx.write(
            &format!("{DATA}/{file}"),
            &core::png_gray8(n as u32, n as u32, map.clone()),
        )?;
        ctx.write(
            &format!("{DATA}/{file}.import"),
            data_import(&file).as_bytes(),
        )?;
    }
    // the editor's cloud cover field (`clouds::cover_map`), the same clouds and cloud shadows
    let cn = 1024u32;
    let cover: Vec<u8> = core::par_map(cn as usize, |y| {
        (0..cn)
            .map(|x| {
                let v = crate::clouds::cloud_noise(
                    (x as f32 + 0.5) / cn as f32,
                    (y as f32 + 0.5) / cn as f32,
                );
                (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            })
            .collect::<Vec<u8>>()
    })
    .into_iter()
    .flatten()
    .collect();
    ctx.write(
        &format!("{DATA}/atmosphere/cloud_cover.png"),
        &core::png_gray8(cn, cn, cover),
    )?;
    ctx.write(
        &format!("{DATA}/atmosphere/cloud_cover.png.import"),
        data_import("atmosphere/cloud_cover.png")
            .replace("mipmaps/generate=false", "mipmaps/generate=true")
            .as_bytes(),
    )?;
    // the volumetric clouds' noise: base shape (128³) and detail (32³) as 3D textures (slices in
    // a grid), and the cirrus veil
    for (name, img, cols) in [
        ("cloud_base", crate::clouds::base_volume(), 16u32),
        ("cloud_detail", crate::clouds::detail_volume(), 8u32),
    ] {
        let n = img.texture_descriptor.size.width;
        let data = img.data.as_ref().ok_or("cloud volume")?;
        let rows = n / cols;
        let mut out = image::RgbaImage::new(n * cols, n * rows);
        for z in 0..n {
            let (gx, gy) = (z % cols * n, z / cols * n);
            for y in 0..n {
                for x in 0..n {
                    let k = (((z * n + y) * n + x) * 4) as usize;
                    out.put_pixel(gx + x, gy + y, image::Rgba([data[k], data[k + 1], data[k + 2], data[k + 3]]));
                }
            }
        }
        let file = format!("atmosphere/{name}.png");
        ctx.write(&format!("{DATA}/{file}"), &core::png_rgba(&out))?;
        ctx.write(
            &format!("{DATA}/{file}.import"),
            format!(
                "[remap]\n\nimporter=\"3d_texture\"\ntype=\"CompressedTexture3D\"\n\n[deps]\n\nsource_file=\"{}\"\n\n[params]\n\ncompress/mode=0\ncompress/high_quality=false\ncompress/lossy_quality=0.7\ncompress/hdr_compression=1\ncompress/channel_pack=0\nmipmaps/generate=false\nmipmaps/limit=-1\nslices/horizontal={cols}\nslices/vertical={rows}\n",
                res(&file)
            )
            .as_bytes(),
        )?;
    }
    {
        let img = crate::clouds::cirrus_map();
        let n = img.texture_descriptor.size.width;
        let data = img.data.as_ref().ok_or("cirrus")?;
        let base = data[..(n * n * 4) as usize].to_vec();
        ctx.write(
            &format!("{DATA}/atmosphere/cirrus.png"),
            &core::png_rgba(&image::RgbaImage::from_raw(n, n, base).ok_or("cirrus")?),
        )?;
        ctx.write(
            &format!("{DATA}/atmosphere/cirrus.png.import"),
            data_import("atmosphere/cirrus.png")
                .replace("mipmaps/generate=false", "mipmaps/generate=true")
                .as_bytes(),
        )?;
    }
    // the editor's shore map: (level of the nearest water, distance to its edge, kind, 1) per texel
    let (sm, shore) = crate::visuals::shore::build(&s.terrain, &s.water);
    ctx.write(
        &format!("{DATA}/atmosphere/shore.f32"),
        bytemuck::cast_slice(&shore),
    )?;
    let light = core::lighting(s);
    let night = Night::of(s, &light);
    let exposure = godot_exposure(&light, &night);
    let lk = &s.look;
    let l = &s.lighting;
    let wl = water_light(s, &light);
    let (weathers, zones, biomes) = weather_tables(s);
    let id = |w: Weather| format!("{w:?}").to_lowercase();
    ctx.write_json(
        &format!("{DATA}/atmosphere/atmosphere.json"),
        &json!({
            "grid": { "vertices": n, "cell": t.cell },
            "sea_level": t.sea_level,
            "zones_map": res("atmosphere/zones.png"),
            "biomes_map": res("atmosphere/biomes.png"),
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
            "fog": atmosphere_fog(s, &light, exposure),
            "zones": zones,
            "biomes": biomes,
            "clouds": {
                "cover": res("atmosphere/cloud_cover.png"),
                "tile": crate::clouds::CLOUD_TILE,
                "base": crate::clouds::CLOUD_BASE,
                "offset": crate::clouds::seed_offset(l.cloud_seed).to_array(),
                "speed": l.cloud_speed,
                "shadows": l.cloud_shadows,
                "base_tex": res("atmosphere/cloud_base.png"),
                "detail_tex": res("atmosphere/cloud_detail.png"),
                "cirrus_tex": res("atmosphere/cirrus.png"),
                "type": l.cloud_type.clamp(0.0, 1.0),
                "density": l.cloud_density.clamp(0.05, 4.0),
                "cirrus": l.cirrus.clamp(0.0, 1.0),
                "steps": l.cloud_quality.march().0,
                "light_steps": l.cloud_quality.march().1,
            },
            "canopy": canopy,
            "shore": {
                "file": res("atmosphere/shore.f32"), "size": sm,
                "sea_hs": crate::ocean::SeaState::at((s.ocean.waves * 1.6).clamp(0.0, 4.0)).hs(),
                "shore_foam": s.ocean.shore_foam,
                "water": Vec3::from_array(s.ocean.shallow.map(|c| crate::terrain::textures::srgb_to_lin_lut()[(c.clamp(0.0, 1.0) * 255.0) as usize])).to_array(),
            },
            "rain": { "puddles": l.rain.puddles },
            "water_light": {
                "sun_dir": wl.0.to_array(), "sun_disp": wl.1.to_array(),
                "sky_clear": wl.2.to_array(), "sky_storm": wl.3.to_array(), "sky_dust": wl.4.to_array(),
            },
        }),
    )
}

/// The key light (sun, or the moon at night) and the sky's irradiance in the editor's display units,
/// as `water::update_water_materials` hands them to its water shader (illuminance × exposure / π,
/// warmed when low; the fog colour × 0.8), for Godot's water: (direction, key, sky clear, sky
/// under storm cloud, sky in dust), linear.
pub(super) fn water_light(s: &Snapshot, light: &core::LightOut) -> (Vec3, Vec3, Vec3, Vec3, Vec3) {
    let l = &s.lighting;
    let lk = &s.look;
    let sun_y = light.sun_dir.y;
    let day = crate::noise::smoothstep(-0.2, 0.18, sun_y);
    let twilight = (1.0 - crate::noise::smoothstep(-0.04, 0.01, sun_y))
        * crate::noise::smoothstep(-0.22, -0.10, sun_y);
    let ev = 10.4 + 4.2 * day - 1.85 * twilight + l.exposure_comp
        - lk.exposure
        - lk.night_lift * light.night;
    let exp = 1.0 / (2f32.powf(ev) * 1.2);
    let (dir, col, illum) = if light.night <= 0.5 {
        let above = crate::noise::smoothstep(-0.06, 0.02, sun_y);
        (
            light.sun_dir,
            lk.sun_color(),
            130_000.0 * l.sun_strength * lk.sun * above.max(0.0001),
        )
    } else {
        let m = crate::sky::moon_direction(l);
        let lit = 0.12 + 0.88 * crate::sky::moon_illumination(l.moon_phase);
        let above = crate::noise::smoothstep(-0.02, 0.1, m.y);
        (
            m,
            Vec3::ONE,
            crate::sky::MOON_LUX * l.moon_strength * lit * light.night * above,
        )
    };
    let warm = crate::noise::smoothstep(0.0, 0.35, dir.y);
    let tint = Vec3::new(1.0, 0.62, 0.38).lerp(Vec3::ONE, warm);
    let key = col * tint * (illum * exp / std::f32::consts::PI);
    let fog = Vec3::from_array(light.fog_color) * 0.8;
    let storm = Vec3::new(0.42, 0.45, 0.5) * (1.0 - light.night * 0.9) * 0.8;
    let dust = Vec3::new(0.75, 0.6, 0.4) * (1.0 - light.night * 0.85) * 0.8;
    (dir, key, fog, storm, dust)
}

/// The editor's outdoor haze as one exponential Godot fog, calibrated side by side against the
/// editor's view: most of what it shows far off is the physical sky's aerial perspective (on
/// whenever haze is on, `sky::update_sun_moon`: its reach follows the haze switch, not the
/// amount), then the distance fog of the haze amount and the look's colour haze (`look_post`),
/// whose tint also colours the fog.
pub(super) struct Haze {
    /// per metre
    pub(super) density: f32,
    /// sRGB, under Godot's exposure
    pub(super) color: [f32; 3],
    sun_scatter: f32,
}

impl Haze {
    /// Aerial perspective of the editor's atmosphere near the ground (per metre).
    const AIR: f32 = 5.0e-5;
    /// The editor's fog colour → Godot's (before exposure): darker, and bluer for the sky's own
    /// scattered light in it.
    const TINT: Vec3 = Vec3::new(0.27, 0.29, 0.34);

    pub(super) fn of(s: &Snapshot, light: &core::LightOut, exposure: f32) -> Self {
        let lk = &s.look;
        let on = if light.haze_enabled { 1.0 } else { 0.0 };
        let colour_haze = lk.color_haze.clamp(0.0, 1.0) * on;
        let density = on * Self::AIR
            + light.fog_density_exp(3000.0)
            + colour_haze / lk.haze_distance.max(10.0) * 0.5;
        Haze {
            density,
            color: Self::tone(s, Vec3::from_array(light.fog_color), colour_haze, exposure),
            sun_scatter: 0.35 * (1.0 - light.night) * 0.3 * on,
        }
    }

    /// An editor fog colour (linear) as Godot's fog colour (sRGB): tinted by the look's colour
    /// haze as the editor does, with Godot's exposure taken out.
    fn tone(s: &Snapshot, mut c: Vec3, colour_haze: f32, exposure: f32) -> [f32; 3] {
        let lk = &s.look;
        if colour_haze > 0.0 {
            let hz = Color::srgb(lk.haze_color[0], lk.haze_color[1], lk.haze_color[2]).to_linear();
            let hz = Vec3::new(hz.red, hz.green, hz.blue);
            let lum = |v: Vec3| v.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(1e-4);
            c = c.lerp(hz * (lum(c) / lum(hz)), (colour_haze * 0.8).min(1.0));
        }
        // the editor's fog colour isn't exposed with the scene; Godot's is
        linear_to_srgb((c * Self::TINT / exposure).min(Vec3::ONE).to_array())
    }
}

/// Godot's tonemap exposure for the editor's: its exposure, the night opening and the stops Godot's
/// tonemappers need to land at the editor's brightness (measured side by side on sunlit ground).
fn godot_exposure(light: &core::LightOut, night: &Night) -> f32 {
    const CALIBRATION_EV: f32 = 0.3;
    2f32.powf(light.exposure + night.ev + CALIBRATION_EV).clamp(0.25, 64.0)
}

/// The editor's night ambient fill colour (sRGB).
const NIGHT_FILL_COLOR: [f32; 3] = [0.55, 0.62, 0.85];

/// How the editor lights the world at night, in Godot's terms (light energy 1.1 = the editor's
/// default sun, `sky::update_sun_moon`).
pub(super) struct Night {
    /// stops the exposure opens up by
    pub(super) ev: f32,
    moon: f32,
    /// unit vector towards the moon
    moon_dir: Vec3,
    /// ambient fill energy
    fill: f32,
    /// 0 day … 1 night
    night: f32,
}

impl Night {
    /// The sky's energy under the opened exposure: most of the gain undone, so the night sky stays
    /// a deep blue (all of it leaves a flat grey).
    pub(super) fn sky_energy(&self) -> f32 {
        2f32.powf(-0.6 * self.ev) * (0.5 + 0.5 * self.night)
    }

    /// Self-lit surfaces (lava, glowing water) look the same by day and by night in the editor:
    /// their emission undoes the opened exposure.
    fn emission(&self) -> f32 {
        2f32.powf(-self.ev)
    }

    pub(super) fn of(s: &Snapshot, light: &core::LightOut) -> Self {
        let l = &s.lighting;
        let sun_y = light.sun_dir.y;
        let day = crate::noise::smoothstep(-0.2, 0.18, sun_y);
        let twilight = (1.0 - crate::noise::smoothstep(-0.04, 0.01, sun_y))
            * crate::noise::smoothstep(-0.22, -0.10, sun_y);
        let moon_dir = crate::sky::moon_direction(l);
        let lit = 0.12 + 0.88 * crate::sky::moon_illumination(l.moon_phase);
        let above = crate::noise::smoothstep(-0.02, 0.1, moon_dir.y);
        let per_lux = 1.1 / 130_000.0;
        Night {
            ev: (14.6 - 10.4) * (1.0 - day) + 1.85 * twilight + s.look.night_lift * light.night,
            moon: crate::sky::MOON_LUX * l.moon_strength * lit * light.night * above * per_lux,
            moon_dir,
            fill: crate::sky::NIGHT_FILL * light.night * l.night_brightness * per_lux,
            night: light.night,
        }
    }
}

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
    // night: the editor's eye opens up (EV 14.6 by day, 10.4 at night, more in the twilight when
    // the night fill is only fading in), and a moon and a cool ambient fill light the land
    let night = Night::of(s, light);
    let (top, horizon) = sky_colors(light, s.look.haze * light.haze);
    // the sky shader: these colours, the sun, the cloud deck (the weather moves it,
    // `wb_atmosphere.gd`), cirrus and stars
    let sky_shader = sc.ext("Shader", &res("shaders/wb_sky.gdshader"));
    let base_tex = sc.ext("CompressedTexture3D", &res("atmosphere/cloud_base.png"));
    let detail_tex = sc.ext("CompressedTexture3D", &res("atmosphere/cloud_detail.png"));
    let cirrus_tex = sc.ext("Texture2D", &res("atmosphere/cirrus.png"));
    let l = &s.lighting;
    let (steps, light_steps, _) = l.cloud_quality.march();
    let sky_mat = sc.sub(
        "ShaderMaterial",
        &format!(
            "shader = {sky_shader}\nshader_parameter/top_color = {}\nshader_parameter/horizon_color = {}\nshader_parameter/sky_energy = {}\nshader_parameter/cloud_cover = {}\nshader_parameter/cloud_type = {}\nshader_parameter/cloud_density = {}\nshader_parameter/cirrus = {}\nshader_parameter/cloud_seed = {}\nshader_parameter/night = {}\nshader_parameter/stars = {}\nshader_parameter/sky_tint = {}\nshader_parameter/sky_tint_amount = {}\nshader_parameter/base_tex = {base_tex}\nshader_parameter/detail_tex = {detail_tex}\nshader_parameter/cirrus_tex = {cirrus_tex}\nshader_parameter/steps = {}\nshader_parameter/light_steps = {}",
            color(linear_to_srgb(top.to_array())),
            color(linear_to_srgb(horizon.to_array())),
            f(night.sky_energy()),
            f(l.cloud_coverage),
            f(l.cloud_type),
            f(l.cloud_density),
            f(l.cirrus),
            f((l.cloud_seed % 10_000) as f32),
            f(light.night),
            f(if l.stars { 1.0 } else { 0.0 }),
            color(s.look.sky_tint),
            f(s.look.sky_tint_amount * (0.35 + 0.65 * (1.0 - light.night))),
            f(steps),
            f(light_steps),
        ),
    );
    // (drifting clouds re-light the scene every frame: realtime radiance at 256 px)
    let sky = sc.sub(
        "Sky",
        &format!("sky_material = {sky_mat}\nprocess_mode = 3\nradiance_size = 3"),
    );
    let exposure = godot_exposure(light, &night);
    let haze = Haze::of(s, light, exposure);
    let lut = sc.ext("Texture3D", &res("look/grade_lut.png"));
    let env = sc.sub(
        "Environment",
        &format!(
            "background_mode = 2\nsky = {sky}\nambient_light_source = 3\nambient_light_color = {fill_col}\nambient_light_energy = 1.0\nambient_light_sky_contribution = {sky_share}\nreflected_light_source = 2\n\
             tonemap_mode = {tonemap}\ntonemap_exposure = {exposure}\ntonemap_white = 6.0\nssao_enabled = true\nssao_radius = 1.5\nssao_intensity = 1.5\n\
             glow_enabled = true\nglow_intensity = {glow}\nglow_bloom = {bloom}\n\
             fog_enabled = {fog}\nfog_light_color = {fog_col}\nfog_density = {fog_density}\nfog_sun_scatter = {sun_scatter}\nfog_aerial_perspective = 0.0\nfog_sky_affect = 0.0\n\
             volumetric_fog_enabled = false\nvolumetric_fog_length = 800.0\nvolumetric_fog_sky_affect = 0.0\nvolumetric_fog_ambient_inject = 0.5\n\
             adjustment_enabled = true\nadjustment_contrast = 1.0\nadjustment_saturation = 1.0\nadjustment_color_correction = {lut}",
            // (the look's saturation and contrast are in the LUT, after tonemapping as in the editor)
            tonemap = look::tonemap_mode(s.look.tone),
            lut = lut,
            exposure = f(exposure),
            // (Godot scales the sky's ambient by the energy too: the fill's level is in its colour)
            // by day most of the ambient is warm light bounced off the ground (the editor's
            // environment light has it; Godot's sky alone reads too blue and too bright in the
            // shade: calibrated side by side), at night the editor's cool fill
            fill_col = color(linear_to_srgb({
                let [r, g, b] = NIGHT_FILL_COLOR;
                let c = Color::srgb(r, g, b).to_linear();
                let day = Vec3::from_array(light.sun_color)
                    * Vec3::new(0.084, 0.068, 0.040)
                    * light.sun_strength.min(1.5)
                    * (1.0 - light.night);
                [
                    c.red * night.fill + day.x,
                    c.green * night.fill + day.y,
                    c.blue * night.fill + day.z,
                ]
            })),
            sky_share = f(0.3 * (1.0 - light.night) + 0.1 * light.night),
            glow = f(0.4 + light.bloom * 2.0),
            bloom = f(light.bloom * 0.3),
            fog = haze.density > 0.0,
            fog_col = color(haze.color),
            fog_density = f(haze.density),
            sun_scatter = f(haze.sun_scatter),
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
            f(light.sun_strength * 1.1),
            f(light.hour)
        ),
    );
    if night.moon > 0.0 {
        let z = night.moon_dir;
        let x = Vec3::Y.cross(z).normalize_or(Vec3::X);
        sc.node(
            "[node name=\"Moon\" type=\"DirectionalLight3D\" parent=\".\"]",
            &format!(
                "transform = {}\nlight_color = Color(0.75, 0.82, 1, 1)\nlight_energy = {}\nshadow_enabled = {}\ndirectional_shadow_mode = 2\ndirectional_shadow_max_distance = 300.0",
                transform(Mat3::from_cols(x, z.cross(x), z), Vec3::new(0.0, 500.0, 0.0)),
                f(night.moon),
                light.night > 0.6 && z.y > 0.05
            ),
        );
    }
    // weather, zone moods, clouds, rain and snow, fog and mist as the camera moves
    let atmosphere = sc.ext("Script", &res("scripts/wb_atmosphere.gd"));
    sc.node(
        "[node name=\"Atmosphere\" type=\"Node3D\" parent=\".\"]",
        &format!(
            "script = {atmosphere}\ndata_path = {}",
            quote(&res("atmosphere/atmosphere.json"))
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
        let noise = sc.ext("Texture2D", &res("water/noise.png"));
        let foam = sc.ext("Texture2D", &res("water/foam.png"));
        let p = water_params(&s.ocean, true, false, night.emission());
        let mut body = format!(
            "render_priority = 0\nshader = {shader}\nshader_parameter/ripple_normal = {ripples}\nshader_parameter/noise_tex = {noise}\nshader_parameter/foam_tex = {foam}"
        );
        for (k, v) in p.as_object().into_iter().flatten() {
            let val = match v {
                Value::Array(a) if a.len() == 3 && (k == "extinct" || k == "inscat") => format!(
                    "Vector3({}, {}, {})",
                    f(a[0].as_f64().unwrap_or(0.0) as f32),
                    f(a[1].as_f64().unwrap_or(0.0) as f32),
                    f(a[2].as_f64().unwrap_or(0.0) as f32)
                ),
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
    fire_nodes(&mut sc, &s.fires, Some(0.15 + 0.85 * light.night));
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
                "script = {script}\ndata_path = {}",
                quote(&res("grass/grass.json"))
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
                    // (unshaded text: undo the exposure so it reads the same by day and by night)
                    &format!("transform = {}\nbillboard = 1\npixel_size = 0.02\nmodulate = {}\nfont_size = 48\noutline_size = 10\ntext = {}", transform(Mat3::IDENTITY, Vec3::new(0.0, 4.0, 0.0)), color([1.0 / exposure; 3]), quote(&m.name)),
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
    // the look's vignette and film grain over the whole view
    if s.look.vignette > 1e-3 || s.look.grain > 1e-4 {
        let post = sc.ext("Shader", &res("shaders/wb_post.gdshader"));
        let mat = sc.sub(
            "ShaderMaterial",
            &format!(
                "render_priority = 127\nshader = {post}\nshader_parameter/vignette = {}\nshader_parameter/grain = {}",
                f(s.look.vignette),
                f(s.look.grain)
            ),
        );
        let quad = sc.sub("QuadMesh", "size = Vector2(1, 1)");
        sc.node(
            "[node name=\"LookPost\" type=\"MeshInstance3D\" parent=\"Camera\"]",
            &format!("transform = Transform3D(1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, -1)\ncast_shadow = 0\nextra_cull_margin = 16384.0\nignore_occlusion_culling = true\nmesh = {quad}\nmaterial_override = {mat}"),
        );
    }
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
    fire_nodes(&mut sc, &d.fires, None);
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
/// `lights`: each fire's light too, at this share of its full (night-time) strength, as the editor
/// dims fires by day.
fn fire_nodes(sc: &mut Scene, fires: &[crate::flames::Fire], lights: Option<f32>) {
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
        if let Some(share) = lights
            && x["light"] == true
        {
            let energy =
                ((num(&x["light_lumens"]) / 300_000.0).sqrt() * 1.5).clamp(0.2, 4.0) * share;
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
    let light = core::lighting(s);
    let night = Night::of(s, &light);
    let (dir, key, sky, _, _) = water_light(s, &light);
    let v3 = |v: Vec3| format!("Vector3({}, {}, {})", f(v.x), f(v.y), f(v.z));
    let globals = format!(
        "[shader_globals]\n\nwb_sun_dir={{\n\"type\": \"vec3\",\n\"value\": {}\n}}\nwb_sun_disp={{\n\"type\": \"vec3\",\n\"value\": {}\n}}\nwb_sky_disp={{\n\"type\": \"vec3\",\n\"value\": {}\n}}\nwb_exposure={{\n\"type\": \"float\",\n\"value\": {}\n}}\nwb_heights={{\n\"type\": \"sampler2D\",\n\"value\": \"\"\n}}\nwb_hgrid={{\n\"type\": \"vec4\",\n\"value\": Vector4(0, 0, 0, 0)\n}}\nwb_cover={{\n\"type\": \"sampler2D\",\n\"value\": \"\"\n}}\nwb_cloud={{\n\"type\": \"vec4\",\n\"value\": Vector4(0, 0, 0, 0)\n}}\nwb_canopy={{\n\"type\": \"sampler2D\",\n\"value\": \"\"\n}}\nwb_canopy_grid={{\n\"type\": \"vec4\",\n\"value\": Vector4(0, 0, 0, 0)\n}}\nwb_canopy_cam={{\n\"type\": \"vec4\",\n\"value\": Vector4(0, 0, 300, 370)\n}}\nwb_shore={{\n\"type\": \"sampler2D\",\n\"value\": \"\"\n}}\nwb_shore_p={{\n\"type\": \"vec4\",\n\"value\": Vector4(0, 0, 0, 0)\n}}\nwb_rain={{\n\"type\": \"vec4\",\n\"value\": Vector4(0, 0, 0, 0)\n}}\nwb_water_tint={{\n\"type\": \"vec3\",\n\"value\": Vector3(0, 0, 0)\n}}\n",
        v3(dir),
        v3(key),
        v3(sky),
        f(godot_exposure(&light, &night))
    );
    format!(
        r#"; Engine configuration file, written by Skyfold Studio.

config_version=5

[application]

config/name={name}
config/description="Exported from Skyfold Studio"
run/main_scene="res://world.tscn"
config/features=PackedStringArray("4.4", "Forward Plus")
config/icon="res://icon.svg"

[display]

window/size/viewport_width=1600
window/size/viewport_height=900

[rendering]

lights_and_shadows/directional_shadow/size=8192
lights_and_shadows/directional_shadow/soft_shadow_filter_quality=3
anti_aliasing/quality/msaa_3d=2
textures/default_filters/anisotropic_filtering_level=4
{sharpen}
{globals}"#,
        name = quote(&s.name),
        globals = globals,
        // the look's sharpening: FSR at full resolution sharpens contrast-adaptively (0 sharpest)
        sharpen = if s.look.sharpen > 0.01 {
            format!(
                "scaling_3d/mode=1\nscaling_3d/scale=1.0\nscaling_3d/fsr_sharpness={}\n",
                f((1.0 - s.look.sharpen.clamp(0.0, 1.0)) * 2.0)
            )
        } else {
            String::new()
        }
    )
}

const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128"><rect width="128" height="128" rx="24" fill="#3d7f5a"/><path d="M14 100 L50 44 L68 70 L82 52 L114 100 Z" fill="#e8f0e8"/></svg>
"##;

const README: &str = r#"# {name} — Godot 4 project

Exported from Skyfold Studio. Open it with **Godot 4.4 or newer**: in the Project Manager choose
*Import*, pick this folder's `project.godot`, open it (the first import takes a minute) and press
**F5**.

Controls: **WASD** move, **Q / E** down / up, **right mouse** (or click to capture the mouse) look,
**Shift** fast, **F** switches between flying and walking, **Esc** frees the mouse, **H** hides the
hints.

## What's in the scene (`world.tscn`)

- **WorldEnvironment**: physical sky, filmic tonemap, SSAO, glow, the world's haze as fog and its
  colour grade (contrast, saturation); **Sun**: a DirectionalLight3D at the world's time of day;
  at night a **Moon** (its phase and strength), the editor's cool night fill as ambient light and
  the exposure opened up the way the editor's eye adapts.
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
- **Grass** (`wb_grass.gd`, `wb_grass.gdshader`, in game): Skyfold Studio's meadow, blade for
  blade: the editor's patch of blade clumps per 8 m cell round the camera at three levels of
  detail, shaped by a port of its grass shader from the world's grass map (density, dryness,
  climate tint, height) and bending in the Atmosphere's wind and gusts.
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
- **Atmosphere** (`wb_atmosphere.gd`, data in `world_builder/atmosphere/`): the weather as
  Skyfold Studio runs it. The zone under the camera sets it (its own weather, else the painted
  biome's; `zones.png` / `biomes.png` map them), with its lighting mood and haze, easing across
  borders. It drives the cloud deck of the sky shader (`wb_sky.gdshader`: the hour's colours,
  sun, clouds, cirrus, stars), rain and snow round the camera, the distance fog, volumetric haze
  (sunbeams), weather fog and ground mist (FogVolumes), the sun's dimming and the exposure under
  heavy cloud, and lightning in storms. Preview a weather with its `weather_override`; game code
  can read `current_weather()` and `current_zone()`.
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
