//! Dungeons in the engine exports. Each dungeon's architecture becomes one GLB in its map's own
//! space: a node per level for walls, floors and stairs, and one per level for ceilings. Both get
//! the target's collider suffix. Liquids and the exit's portal sheet get nodes without colliders.
//! Its objects become instances of the export's models (`dungeon_instances`). Its description
//! (`dungeons::export::describe`) goes alongside. The world's way-in sheets go in one more GLB.
//! The engines' writers (godot / unity / unreal) build a scene or level per dungeon from these,
//! with triggers at the ways in and the way out; the generic export writes the files only.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::core::{self, Ctx, Inst, ModelOut, Snapshot};
use super::glb::{Glb, Prim, Wrap};
use crate::dungeons::export::{DungeonExport, Geometry, Trigger};
use crate::dungeons::plan::Liquid;
use crate::dungeons::styles::{Slot, Style, Tex};

/// One dungeon as written: its files (relative to the export's root) and description.
pub struct Written<'a> {
    pub d: &'a DungeonExport,
    /// a name safe for files, scenes and levels (`the_sunken_crypt`)
    pub safe: String,
    pub glb: Option<String>,
    pub inst: Vec<Vec<Inst>>,
    pub json: Value,
}

/// The dungeons to write (an error while a dungeon's map is open: see `dungeons::export::gather`).
pub fn list(s: &Snapshot) -> Result<&[DungeonExport], String> {
    s.dungeons.as_deref().map_err(|e| e.clone())
}

/// A name safe for files and engine scenes from a dungeon's folder (`001_the_sunken_crypt` →
/// `the_sunken_crypt`; unique in the world because of the number).
pub fn safe(d: &DungeonExport) -> String {
    let s = core::safe_name(&d.slug);
    let trimmed = s.trim_start_matches(|c: char| c.is_ascii_digit() || c == '_');
    format!(
        "d{}_{}",
        d.id,
        if trimmed.is_empty() {
            "dungeon"
        } else {
            trimmed
        }
    )
}

/// Writes every dungeon's GLB (`<dir>/<slug>/dungeon.glb`, colliding nodes ending in `suffix`)
/// and works out its instances. `write` stores a GLB (the target's own: Godot also writes its
/// `.import`). The world's portal sheets go to `<dir>/portals.glb` when there are any.
pub fn write_all<'a>(
    s: &'a Snapshot,
    ctx: &Ctx,
    models: &[ModelOut],
    dir: &str,
    suffix: &str,
    write: &dyn Fn(&str, Vec<u8>) -> Result<(), String>,
) -> Result<(Vec<Written<'a>>, Option<String>), String> {
    write_all_with(s, ctx, models, dir, suffix, write, &|rel, b| {
        ctx.write(rel, b)
    })
}

/// `write_all`, with where other files go (`write_file`: the decals' pictures).
#[allow(clippy::too_many_arguments)]
pub fn write_all_with<'a>(
    s: &'a Snapshot,
    ctx: &Ctx,
    models: &[ModelOut],
    dir: &str,
    suffix: &str,
    write: &dyn Fn(&str, Vec<u8>) -> Result<(), String>,
    write_file: &dyn Fn(&str, &[u8]) -> Result<(), String>,
) -> Result<(Vec<Written<'a>>, Option<String>), String> {
    let list = list(s)?;
    // the decals' pictures, once for every dungeon
    let mut kinds: Vec<crate::dungeons::decals::DecalKind> = list
        .iter()
        .filter_map(|d| d.map.as_ref())
        .flat_map(|m| m.decals.iter().map(|x| x.kind))
        .collect();
    kinds.sort_by_key(|k| *k as u8);
    kinds.dedup();
    for k in &kinds {
        write_file(
            &format!("{dir}/decals/{}.png", k.id()),
            &crate::dungeons::decals::png(*k),
        )?;
    }

    let mut out = vec![];
    for (k, d) in list.iter().enumerate() {
        ctx.stage(
            &format!("Dungeon {}", d.name),
            0.9 + 0.03 * k as f32 / list.len().max(1) as f32,
        )?;
        let g = d.map.as_ref().and_then(crate::dungeons::export::geometry);
        let safe = safe(d);
        let glb = match &g {
            Some(g) => {
                let rel = format!("{dir}/{}/dungeon.glb", d.slug);
                write(
                    &rel,
                    dungeon_glb(d, g, ctx.opts.texture_size, suffix, &safe),
                )?;
                Some(rel)
            }
            None => {
                if d.kind == crate::dungeons::model::MapKind::Interior {
                    ctx.warn(format!("dungeon '{}': its map was never opened (open it once in Skyfold Studio to build it)", d.name));
                } else {
                    ctx.warn(format!("realm '{}': realms export as worlds of their own (open it and export it); its ways in and links are in dungeons.json", d.name));
                }
                None
            }
        };
        let inst = dungeon_instances(s, models, &d.objects);
        let mut json = crate::dungeons::export::describe(d, g.as_ref());
        // its minimaps (a picture per level, and where each lies)
        if let Some(m) = d.map.as_ref().filter(|m| {
            m.kind == crate::dungeons::model::MapKind::Interior && !m.plan.rooms.is_empty()
        }) {
            let c = crate::dungeons::render::compile(&m.plan, &m.style);
            let exit = d.exit.as_ref().map(|t| t.center - Vec3::Y * t.half.y);
            let mut maps = vec![];
            for lv in crate::dungeons::survey::levels(m) {
                let Some(mm) = crate::dungeons::survey::minimap(m, &c, lv, exit) else {
                    continue;
                };
                let rel = format!("{dir}/{}/minimap_L{lv}.png", d.slug);
                write_file(&rel, &crate::dungeons::survey::png(&mm))?;
                maps.push(json!({ "level": lv, "file": rel, "origin": [mm.origin.x, mm.origin.y], "metres_per_pixel": mm.mpp, "size": [mm.image.width(), mm.image.height()], "about": "x east with the columns, z with the rows (Skyfold axes)" }));
            }
            json["minimaps"] = json!(maps);
        }
        // its own track (or its demo track), copied next to its minimaps
        if let Some(m) = d.map.as_ref()
            && let Some(src) = m.music.path(d.folder.as_deref())
            && let Ok(bytes) = std::fs::read(&src)
            && let Some(name) = src.file_name().map(|n| n.to_string_lossy().to_string())
        {
            let rel = format!("{dir}/{}/{name}", d.slug);
            write_file(&rel, &bytes)?;
            json["music"] = json!({ "file": rel, "volume": m.music.volume, "ambience_under": 1.0 - m.music.hush, "loop": true });
        }
        json["safe_name"] = json!(safe);
        json["glb"] = json!(glb);
        json["pieces"] = json!(
            d.map
                .as_ref()
                .map(|m| pieces_json(m, &safe, &|p| p, &|q| q))
                .unwrap_or_default()
        );
        json["decal_pictures"] = json!(
            kinds
                .iter()
                .map(|k| (
                    k.id().to_string(),
                    json!(format!("{dir}/decals/{}.png", k.id()))
                ))
                .collect::<serde_json::Map<_, _>>()
        );
        out.push(Written {
            d,
            safe,
            glb,
            inst,
            json,
        });
    }
    let sheets = portals_glb(list);
    let portals = match sheets {
        Some(b) => {
            let rel = format!("{dir}/portals.glb");
            write(&rel, b)?;
            Some(rel)
        }
        None => None,
    };
    Ok((out, portals))
}

/// The fires the export burns (the world's and its dungeons'): which baked fires it carries.
pub fn flame_kinds(s: &Snapshot) -> Vec<crate::flames::FireKind> {
    let mut v: Vec<_> = s
        .fires
        .iter()
        .map(|f| f.kind)
        .chain(
            s.dungeons
                .iter()
                .flatten()
                .flat_map(|d| d.fires.iter().map(|f| f.kind)),
        )
        .collect();
    v.sort_by_key(|k| k.id());
    v.dedup();
    v
}

/// Writes the flipbooks of the fires the export burns as `<dir>/<fire>.png`.
pub fn write_flame_sheets(
    s: &Snapshot,
    dir: &str,
    write_file: &dyn Fn(&str, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let kinds = flame_kinds(s);
    for k in &kinds {
        let b = std::fs::read(crate::flames::sheet_path(*k))
            .map_err(|e| format!("flame sheet {}: {e}", k.id()))?;
        write_file(&format!("{dir}/{}.png", k.id()), &b)?;
    }
    // (AetherFX's licence goes with its fire)
    if !kinds.is_empty()
        && let Ok(l) = std::fs::read(
            crate::flames::sheet_path(crate::flames::FireKind::Torch)
                .with_file_name("LICENSE-AetherFX.txt"),
        )
    {
        write_file(&format!("{dir}/LICENSE-AetherFX.txt"), &l)?;
    }
    Ok(())
}

/// The objects placed in a dungeon's map as instances of the export's models (as
/// `core::instances` does for the world's).
pub fn dungeon_instances(
    s: &Snapshot,
    models: &[ModelOut],
    objects: &[crate::objects::Placed],
) -> Vec<Vec<Inst>> {
    let by_id: std::collections::HashMap<&str, usize> = models
        .iter()
        .enumerate()
        .map(|(k, m)| (s.assets[m.asset].id.as_str(), k))
        .collect();
    let mut out: Vec<Vec<Inst>> = (0..models.len()).map(|_| vec![]).collect();
    for o in objects {
        if let Some(&k) = by_id.get(o.asset.as_str()) {
            let nv = models[k].variants.len().max(1) as u32;
            out[k].push(Inst {
                variant: o.variant.min(nv - 1),
                pos: o.pos,
                rot: o.rot,
                scale: o.scale,
                stretch: o.stretch,
            });
        }
    }
    out
}

/// The `dungeons.json` index every target writes: the dungeons, their files, how they link.
pub fn index_json(
    s: &Snapshot,
    written: &[Written],
    portals: Option<&str>,
    extra: impl Fn(&Written) -> Value,
) -> Value {
    json!({
        "format": "skyfold-dungeons/1",
        "world": s.name,
        "about": "Dungeons are maps of their own: underground ones are built here (dungeon.glb in the map's own space: L<level> walls/floors/stairs and L<level>_ceiling nodes collide, _liquids and _exit don't), realms export as worlds of their own. Ways in: triggers in the world (center, rotation quaternion x y z w, half size; Y up, metres) and where players come back out; exit: the trigger in the dungeon back to the world. Positions are in Skyfold Studio's axes (Y up, right-handed) unless the target converts them.",
        "portal_sheets": portals,
        "flames": crate::dungeons::export::flame_sheets_json(),
        "dungeons": written.iter().map(|w| {
            let mut v = w.json.clone();
            if let (Some(o), Some(e)) = (v.as_object_mut(), extra(w).as_object()) {
                for (k, x) in e {
                    o.insert(k.clone(), x.clone());
                }
            }
            v
        }).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------------------------------------
// the architecture

struct Tex3 {
    base: usize,
    normal: Option<usize>,
    orm: Option<usize>,
}

/// One dungeon's GLB in its map's own space.
pub fn dungeon_glb(d: &DungeonExport, g: &Geometry, tex: u32, suffix: &str, safe: &str) -> Vec<u8> {
    let mut glb = Glb::new();
    // (under a sky the walls and roofs are seen from outside too)
    let double = d
        .map
        .as_ref()
        .is_some_and(|m| m.sky != crate::dungeons::model::SkyMode::None);
    let mut textures: std::collections::HashMap<String, Option<Tex3>> = Default::default();
    let mut mats: std::collections::HashMap<(&'static str, Slot), usize> = Default::default();
    let mut material = |glb: &mut Glb, st: &'static Style, slot: Slot| -> usize {
        if let Some(m) = mats.get(&(st.id, slot)) {
            return *m;
        }
        let m = st.mat(slot);
        let key = format!("{:?}", m.tex);
        if !textures.contains_key(&key) {
            let t = load_tex(glb, m.tex, tex);
            textures.insert(key.clone(), t);
        }
        let t = textures
            .get(&key)
            .and_then(|t| t.as_ref())
            .map(|t| (t.base, t.normal, t.orm));
        let tint = Color::srgb_from_array(m.tint).to_linear();
        let mut pbr = json!({
            "baseColorFactor": [tint.red, tint.green, tint.blue, 1.0],
            "metallicFactor": m.metal,
            "roughnessFactor": m.rough,
        });
        let mut v = json!({ "name": format!("{}_{}", st.id, format!("{slot:?}").to_lowercase()), "doubleSided": double });
        if let Some((base, normal, orm)) = t {
            pbr["baseColorTexture"] = json!({ "index": base });
            if let Some(n) = normal {
                v["normalTexture"] = json!({ "index": n });
            }
            if let Some(o) = orm {
                // glTF's packing is the same: occlusion R, roughness G, metal B
                pbr["metallicRoughnessTexture"] = json!({ "index": o });
                pbr["roughnessFactor"] = json!(1.0);
                pbr["metallicFactor"] = json!(1.0);
                v["occlusionTexture"] = json!({ "index": o });
            }
        }
        v["pbrMetallicRoughness"] = pbr;
        let i = glb.material_json(v);
        mats.insert((st.id, slot), i);
        i
    };
    let mut levels: Vec<i32> = g.parts.iter().map(|p| p.level).collect();
    levels.sort_unstable();
    levels.dedup();
    let mut roots = vec![];
    for lv in levels {
        for ceiling in [false, true] {
            let parts: Vec<_> = g
                .parts
                .iter()
                .filter(|p| p.level == lv && p.ceiling == ceiling)
                .collect();
            if parts.is_empty() {
                continue;
            }
            let ms: Vec<usize> = parts
                .iter()
                .map(|p| material(&mut glb, p.style, p.slot))
                .collect();
            // the grime baked into the vertex colours with the corner shading (engines multiply
            // the albedo by them; the editor's moss and wet sheen are its own shader's)
            let cols: Vec<Vec<[f32; 4]>> = parts.iter().map(|p| baked_colors(d, p)).collect();
            let prims: Vec<Prim> = parts
                .iter()
                .zip(&ms)
                .zip(&cols)
                .map(|((p, m), col)| Prim {
                    positions: &p.mesh.pos,
                    normals: Some(&p.mesh.nrm),
                    tangents: None,
                    uvs: Some(&p.mesh.uv),
                    colors: Some(col),
                    indices: &p.mesh.idx,
                    material: Some(*m),
                })
                .collect();
            let name = format!(
                "{safe}_L{}{}{suffix}",
                level_tag(lv),
                if ceiling { "_ceiling" } else { "" }
            );
            if let Some(mesh) = glb.mesh(&name, &prims) {
                roots.push(glb.node(&name, Some(mesh), vec![]));
            }
        }
    }
    // liquids: no colliders (swim / pain volumes are the game's to add from the rooms)
    if !g.liquids.is_empty() {
        let mut liquid_mats: std::collections::HashMap<Liquid, usize> = Default::default();
        let ms: Vec<usize> = g
            .liquids
            .iter()
            .map(|(_, l, _)| {
                *liquid_mats
                    .entry(*l)
                    .or_insert_with(|| glb.material_json(liquid_material(*l)))
            })
            .collect();
        let prims: Vec<Prim> = g
            .liquids
            .iter()
            .zip(&ms)
            .map(|((_, _, m), mi)| Prim {
                positions: &m.pos,
                normals: Some(&m.nrm),
                tangents: None,
                uvs: Some(&m.uv),
                colors: None,
                indices: &m.idx,
                material: Some(*mi),
            })
            .collect();
        let name = format!("{safe}_liquids");
        if let Some(mesh) = glb.mesh(&name, &prims) {
            roots.push(glb.node(&name, Some(mesh), vec![]));
        }
    }
    // doors, gates, bars, secret doors, levers, plates and traps: a node each (at its foot,
    // facing its way) with its moving parts under it, placed as it starts (shut or open); no
    // colliders (the engines' scripts add a box that opens with it)
    if let Some(m) = d
        .map
        .as_ref()
        .filter(|m| m.pieces.iter().any(|p| !p.kind.is_marker()))
    {
        let st = crate::dungeons::styles::get(&m.style);
        let iron = glb.material_json(json!({ "name": "piece_iron", "pbrMetallicRoughness": { "baseColorFactor": [0.022, 0.02, 0.019, 1.0], "metallicFactor": 0.85, "roughnessFactor": 0.5 } }));
        let mut kids = vec![];
        for p in m.pieces.iter().filter(|p| !p.kind.is_marker()) {
            let name = piece_node(safe, p.id);
            let mut parts = vec![];
            for (k, part) in crate::dungeons::pieces::parts(p).into_iter().enumerate() {
                let mat = match part.material {
                    crate::dungeons::pieces::Mat::Wood => material(&mut glb, st, Slot::Beam),
                    crate::dungeons::pieces::Mat::Stone => material(&mut glb, st, Slot::Wall),
                    crate::dungeons::pieces::Mat::Iron => iron,
                };
                let prim = Prim {
                    positions: &part.mesh.pos,
                    normals: Some(&part.mesh.nrm),
                    tangents: None,
                    uvs: Some(&part.mesh.uv),
                    colors: None,
                    indices: &part.mesh.idx,
                    material: Some(mat),
                };
                let pn = format!("{name}_{k}");
                let Some(mesh) = glb.mesh(&pn, &[prim]) else {
                    continue;
                };
                let n = glb.node(&pn, Some(mesh), vec![]);
                let tf = if p.open { part.open } else { part.closed };
                glb.set_transform(n, tf.translation.to_array(), tf.rotation.to_array());
                parts.push(n);
            }
            let n = glb.node(&name, None, parts);
            glb.set_transform(n, p.pos, p.rotation().to_array());
            kids.push(n);
        }
        if !kids.is_empty() {
            roots.push(glb.node(&format!("{safe}_pieces"), None, kids));
        }
    }
    // the way back out: its glowing sheet
    if let Some(t) = &d.exit
        && let Some(n) = sheet_node(&mut glb, t, &format!("{safe}_exit"))
    {
        roots.push(n);
    }
    glb.finish(&roots)
}

/// A piece's node in the dungeon's GLB (its parts are `<node>_0`, `<node>_1`…).
pub fn piece_node(safe: &str, id: u32) -> String {
    format!("{safe}_piece{id}")
}

/// The map's pieces for the engines, positions and rotations through `pos` / `rot` (an engine's
/// own axes): kind, foot, facing, size, how it starts, what it works, its label; for the ones
/// with parts their nodes in the GLB and each part's shut and open pose (relative to the piece);
/// whether it blocks the way while shut.
pub fn pieces_json(
    m: &crate::dungeons::model::MapDoc,
    safe: &str,
    pos: &dyn Fn([f32; 3]) -> [f32; 3],
    rot: &dyn Fn([f32; 4]) -> [f32; 4],
) -> Vec<Value> {
    use crate::dungeons::pieces::PieceKind;
    let pose = |t: Transform| json!({ "position": pos(t.translation.to_array()), "rotation": rot(t.rotation.to_array()) });
    m.pieces
        .iter()
        .map(|p| {
            let marker = p.kind.is_marker();
            let parts: Vec<Value> = if marker {
                vec![]
            } else {
                crate::dungeons::pieces::parts(p).into_iter().enumerate().map(|(k, part)| json!({ "node": format!("{}_{k}", piece_node(safe, p.id)), "closed": pose(part.closed), "open": pose(part.open) })).collect()
            };
            json!({
                "id": p.id,
                "kind": p.kind.id(),
                "name": p.kind.name(),
                "position": pos(p.pos),
                "rotation": rot(p.rotation().to_array()),
                "yaw_deg": p.yaw.to_degrees(),
                "width": p.width,
                "height": p.height,
                "open": p.open,
                "links": p.links,
                "label": p.label,
                "auto": p.auto,
                "marker": marker,
                "usable": p.kind.usable(),
                "blocks": p.kind.in_doorway(),
                "node": (!marker).then(|| piece_node(safe, p.id)),
                "parts": parts,
                "seconds": if p.kind == PieceKind::Portcullis { 2.2 } else { 0.9 },
            })
        })
        .collect()
}

/// A part's vertex colours for the engines: corner shading times the grime the editor's surface
/// shader lays at the foot of the walls and in corners (`shaders/dungeon_surface.wesl`).
fn baked_colors(d: &DungeonExport, p: &crate::dungeons::export::Part) -> Vec<[f32; 4]> {
    let Some(map) = d.map.as_ref() else {
        return p.mesh.col.iter().map(|c| [c[0], c[1], c[2], 1.0]).collect();
    };
    let amount = crate::dungeons::render::surface_params(p.style, p.slot, &map.weathering, map.art)
        .amounts
        .y;
    let smooth = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    p.mesh
        .col
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let ao = c[0];
            let h = p
                .mesh
                .aux
                .get(i)
                .map(|a| p.mesh.pos[i][1] - a[0])
                .unwrap_or(10.0);
            // (the foot of the walls: a floor gets its grime from the corners)
            let wall = if p.mesh.nrm[i][1].abs() < 0.4 {
                1.0
            } else {
                0.0
            };
            let g = (((1.0 - smooth(0.0, 1.4, h)) * 0.75 * wall + (1.0 - ao) * 0.9) * amount)
                .clamp(0.0, 1.0);
            [
                ao * (1.0 - 0.45 * g),
                ao * (1.0 - 0.5 * g),
                ao * (1.0 - 0.55 * g),
                1.0,
            ]
        })
        .collect()
}

/// Levels as node names: `0`, `1`, `m1` (below the ground level).
fn level_tag(l: i32) -> String {
    if l < 0 {
        format!("m{}", -l)
    } else {
        l.to_string()
    }
}

/// A style's textures as GLB textures (JPEG; scans' ORM maps too).
fn load_tex(g: &mut Glb, t: Tex, size: u32) -> Option<Tex3> {
    let (a, n, o) = t.paths();
    let root = std::path::PathBuf::from(crate::assets_path());
    let load = |g: &mut Glb, rel: &str, name: &str| -> Option<usize> {
        let img = image::open(root.join(rel)).ok()?.to_rgb8();
        let img = core::fit(img, size);
        let i = g.image(&core::jpeg(&img, 90), "image/jpeg", name);
        Some(g.texture(i, Wrap::Repeat))
    };
    let stem = std::path::Path::new(&a)
        .with_extension("")
        .to_string_lossy()
        .replace('/', "_");
    let base = load(g, &a, &stem)?;
    Some(Tex3 {
        base,
        normal: n
            .as_deref()
            .and_then(|p| load(g, p, &format!("{stem}_normal"))),
        orm: o
            .as_deref()
            .and_then(|p| load(g, p, &format!("{stem}_orm"))),
    })
}

fn liquid_material(l: Liquid) -> Value {
    let (base, emissive, rough, blend) = match l {
        Liquid::Water | Liquid::None => ([0.02, 0.05, 0.06, 0.86], [0.0; 3], 0.04, true),
        Liquid::Lava => ([0.2, 0.04, 0.01, 1.0], [1.0, 0.23, 0.04], 0.8, false),
        Liquid::Slime => ([0.12, 0.25, 0.04, 0.92], [0.06, 0.17, 0.02], 0.15, true),
        Liquid::Blood => ([0.18, 0.01, 0.01, 0.95], [0.0; 3], 0.08, true),
        Liquid::Arcane => ([0.1, 0.12, 0.3, 0.85], [0.18, 0.31, 1.0], 0.1, true),
    };
    let mut v = json!({
        "name": format!("liquid_{}", l.id()),
        "pbrMetallicRoughness": { "baseColorFactor": base, "metallicFactor": 0.0, "roughnessFactor": rough },
        "emissiveFactor": emissive,
    });
    if blend {
        v["alphaMode"] = json!("BLEND");
    }
    v
}

/// A portal's glowing sheet standing in its trigger (an emissive arch; engines may swap in an
/// animated material of their own).
fn sheet_node(g: &mut Glb, t: &Trigger, name: &str) -> Option<usize> {
    let (w, h) = t.sheet?;
    let m = crate::dungeons::portal::sheet_mesh(w, h, crate::dungeons::edit::Sheet::Arch);
    let read3 = |id| match m.attribute(id) {
        Some(bevy::mesh::VertexAttributeValues::Float32x3(v)) => v.clone(),
        _ => vec![],
    };
    let uv = match m.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(bevy::mesh::VertexAttributeValues::Float32x2(v)) => v.clone(),
        _ => vec![],
    };
    let idx: Vec<u32> = m
        .indices()
        .map(|i| i.iter().map(|x| x as u32).collect())
        .unwrap_or_default();
    // into world space: the sheet's foot at the trigger's foot, facing as it does
    let foot = t.center - t.rot * Vec3::new(0.0, t.half.y, 0.0);
    let pos: Vec<[f32; 3]> = read3(Mesh::ATTRIBUTE_POSITION)
        .into_iter()
        .map(|p| (foot + t.rot * Vec3::from_array(p)).to_array())
        .collect();
    let nrm: Vec<[f32; 3]> = read3(Mesh::ATTRIBUTE_NORMAL)
        .into_iter()
        .map(|n| (t.rot * Vec3::from_array(n)).to_array())
        .collect();
    let c = t.color;
    let mat = g.material_json(json!({
        "name": "portal_liquid",
        "pbrMetallicRoughness": { "baseColorFactor": [c[0] * 0.3, c[1] * 0.3, c[2] * 0.3, 0.85], "metallicFactor": 0.0, "roughnessFactor": 0.1 },
        "emissiveFactor": c,
        "alphaMode": "BLEND",
        "doubleSided": true,
    }));
    let prim = Prim {
        positions: &pos,
        normals: Some(&nrm),
        tangents: None,
        uvs: (!uv.is_empty()).then_some(&uv[..]),
        colors: None,
        indices: &idx,
        material: Some(mat),
    };
    let mesh = g.mesh(name, &[prim])?;
    Some(g.node(name, Some(mesh), vec![]))
}

/// The world's way-in sheets (portals and caves' ends) in one GLB, in the world's space.
fn portals_glb(list: &[DungeonExport]) -> Option<Vec<u8>> {
    let mut g = Glb::new();
    let mut roots = vec![];
    for d in list {
        for w in &d.ways_in {
            if let Some(n) = sheet_node(
                &mut g,
                &w.trigger,
                &format!("{}_way_in_{}", safe(d), w.entrance),
            ) {
                roots.push(n);
            }
        }
    }
    (!roots.is_empty()).then(|| g.finish(&roots))
}

/// Generic export: `dungeons/<slug>/dungeon.glb`, `dungeon.json`, `instances.json` (the same
/// format as the world's), `dungeons/portals.glb` and `dungeons/dungeons.json`.
pub fn generic(s: &Snapshot, ctx: &Ctx, models: &[ModelOut]) -> Result<Value, String> {
    if list(s)?.is_empty() {
        return Ok(Value::Null);
    }
    let (written, portals) = write_all(s, ctx, models, "dungeons", "", &|rel, b| {
        ctx.write_glb(rel, b)
    })?;
    for w in &written {
        let mut v = w.json.clone();
        v["instances"] = json!(format!("dungeons/{}/instances.json", w.d.slug));
        ctx.write_json(&format!("dungeons/{}/dungeon.json", w.d.slug), &v)?;
        ctx.write_json(
            &format!("dungeons/{}/instances.json", w.d.slug),
            &core::instances_json(s, models, &w.inst, "models"),
        )?;
    }
    let index = index_json(
        s,
        &written,
        portals.as_deref(),
        |w| json!({ "dungeon_json": format!("dungeons/{}/dungeon.json", w.d.slug) }),
    );
    ctx.write_json("dungeons/dungeons.json", &index)?;
    Ok(json!({ "dungeons": written.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeons::model::{DungeonDef, MapDoc, MapKind};
    use crate::dungeons::presets;

    fn crypt() -> DungeonExport {
        let p = presets::get("crypt").unwrap();
        let mut map = MapDoc::new(1, "Crypt", MapKind::Interior);
        map.plan = presets::generate(
            p,
            presets::Size::Small,
            presets::RoomSize::Normal,
            0.0,
            7,
            p.style,
        );
        map.style = p.style.to_string();
        crate::dungeons::pieces::place_for_layout(&mut map);
        let def = DungeonDef {
            id: 1,
            name: "Crypt".into(),
            kind: MapKind::Interior,
            instancing: Default::default(),
            levels: [10, 15],
            players: 5,
            entrances: vec![],
            folder: "001_crypt".into(),
            layout: "crypt".into(),
            style: String::new(),
            size: presets::Size::Small,
            room_size: Default::default(),
            irregularity: 0.0,
            art: Default::default(),
            mood: String::new(),
            notes: String::new(),
            shape: None,
            realm_size: Default::default(),
            seed: None,
            compound: None,
            sheet: None,
        };
        let exit = Some(Trigger {
            center: Vec3::new(0.0, 2.0, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(1.5, 2.0, 0.6),
            sheet: Some((3.0, 4.0)),
            color: [0.6, 0.8, 1.0],
        });
        DungeonExport {
            id: 1,
            name: "Crypt".into(),
            slug: "001_crypt".into(),
            kind: MapKind::Interior,
            def,
            map: Some(map),
            objects: vec![],
            start: None,
            exit,
            ways_in: vec![],
            fires: vec![],
            folder: None,
        }
    }

    #[test]
    fn a_dungeon_exports_as_one_glb_with_a_node_per_level() {
        let d = crypt();
        let g = crate::dungeons::export::geometry(d.map.as_ref().unwrap()).expect("built");
        assert!(g.triangles > 1000, "{} triangles", g.triangles);
        let b = dungeon_glb(&d, &g, 256, "-col", &safe(&d));
        assert_eq!(&b[0..4], b"glTF");
        let json = super::super::glb::glb_json(&b).expect("a readable GLB");
        let names: Vec<&str> = json["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();
        assert!(names.contains(&"d1_crypt_L0-col"), "{names:?}");
        assert!(names.contains(&"d1_crypt_L0_ceiling-col"), "{names:?}");
        assert!(names.contains(&"d1_crypt_exit"), "{names:?}");
        // the style's textures are in it
        assert!(json["images"].as_array().is_some_and(|i| i.len() >= 2));
        // its doors and gates: a node each with its parts, named as dungeon.json says
        let m = d.map.as_ref().unwrap();
        assert!(!m.pieces.is_empty(), "the crypt's template has doors");
        assert!(names.contains(&"d1_crypt_pieces"), "{names:?}");
        let pieces = pieces_json(m, &safe(&d), &|p| p, &|q| q);
        for p in &pieces {
            let node = p["node"].as_str().unwrap();
            assert!(names.contains(&node), "{node} missing");
            for part in p["parts"].as_array().unwrap() {
                assert!(names.contains(&part["node"].as_str().unwrap()), "{part}");
            }
        }
    }
}
