//! Material tools: the photoscanned texture behind each ground layer, the building materials per
//! part type, and the automatic ground style (snow line, rock on slopes, beaches, dry grass).

use bevy::prelude::*;
use serde_json::{Map, Value, json};

use super::*;
use crate::mcp_tools::{McpAppExt, McpTool};
use crate::surfaces::{Slot, SurfaceChoices, SurfaceLibrary};
use crate::terrain::Terrain;
use crate::terrain::layers::{Layer, Surface};
use crate::terrain::textures::{TerrainLayerChoices, TerrainLibrary};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "materials_list",
        description: "The photoscanned CC0 material libraries: ground textures for the paint layers (grass … paving, plus city: asphalt, slate walkways, pavers, terrazzo, deck plating; id, name, category, average colour and the colour it reads as from afar, such as dark green, khaki or grey) with each layer's current choice, and building materials (wood, metal, stone, fabric, roofs, concrete, plastic, tech panels, facades…, with their colours too) with each building part's current choice and the categories it accepts. Also the automatic ground style values. Entries marked downloads_when_used are in a free content pack this computer hasn't downloaded yet (the city & sci-fi surfaces): choosing one with materials_set downloads it in the background (content_packs shows progress) and it shows once installed.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| run(w, list, a),
    });
    app.mcp_tool(McpTool {
        name: "materials_set",
        description: "Choose materials (Paint tab › layer texture, Objects tab › Building materials) and the automatic ground style. `ground`: {layer: texture id} repaints everywhere that layer shows (\"\" = built-in; footstep sounds follow). \
`buildings`: {part: material id} (\"\" = default) for posts, planks, roofs… of every structure. `style`: snow_line (m above sea), snow_blend (m), rock_slope (0.15 rock only on cliffs … 0.9 rock on gentle slopes), beach_height (m), dryness (0..1 dry grass), wet_gullies (0..1 damp soil, mud and stream gravel where rain gathers). `footsteps`: {layer: surface} overrides the footstep sound a ground layer makes (Audio › Footsteps by ground; picking a texture sets it to the texture's own). Settings saved with the world (not undo steps); ids from materials_list.",
        input_schema: || {
            let mut ground = Map::new();
            for l in Layer::ALL {
                ground.insert(snake(&format!("{l:?}")), json!({ "type": "string" }));
            }
            let mut parts = Map::new();
            for s in Slot::ALL {
                parts.insert(snake(&format!("{s:?}")), json!({ "type": "string" }));
            }
            json!({
                "type": "object",
                "properties": {
                    "ground": { "type": "object", "properties": ground, "description": "layer → ground texture id" },
                    "buildings": { "type": "object", "properties": parts, "description": "building part → material id" },
                    "style": {
                        "type": "object",
                        "properties": {
                            "snow_line": { "type": "number", "minimum": 20, "maximum": 800 },
                            "snow_blend": { "type": "number", "minimum": 2, "maximum": 150 },
                            "rock_slope": { "type": "number", "minimum": 0.15, "maximum": 0.9 },
                            "beach_height": { "type": "number", "minimum": 0, "maximum": 12 },
                            "dryness": { "type": "number", "minimum": 0, "maximum": 1 },
                            "wet_gullies": { "type": "number", "minimum": 0, "maximum": 1 }
                        }
                    },
                    "footsteps": { "type": "object", "description": "ground layer → footstep surface", "additionalProperties": { "type": "string", "enum": Surface::ALL.iter().map(|x| snake(&format!("{x:?}"))).collect::<Vec<_>>() } }
                }
            })
        },
        run: |w, a| run(w, set, a),
    });
}

fn list(
    In(_): In<Value>,
    tlib: Res<TerrainLibrary>,
    layers: Res<TerrainLayerChoices>,
    slib: Option<Res<SurfaceLibrary>>,
    parts: Res<SurfaceChoices>,
    t: Res<Terrain>,
    ed: Res<crate::editor::Editor>,
) -> R {
    let mut want: Vec<std::path::PathBuf> = tlib
        .materials
        .iter()
        .map(|m| {
            crate::terrain::textures::library_file(&m.id, "albedo.png")
        })
        .collect();
    if let Some(sl) = &slib {
        want.extend(
            sl.entries
                .iter()
                .filter(|e| e.tiling)
                .map(|e| crate::surfaces::item_file(&e.id, "albedo.png")),
        );
    }
    let colors = mean_colors(&want);
    let color = |path: std::path::PathBuf, v: &mut Value| {
        if let Some(c) = colors.get(&path) {
            v["color_srgb"] = json!(c.map(|x| (x * 1000.0).round() / 1000.0));
            v["hex"] = json!(format!(
                "#{:02x}{:02x}{:02x}",
                (c[0] * 255.0).round() as u8,
                (c[1] * 255.0).round() as u8,
                (c[2] * 255.0).round() as u8
            ));
            v["reads_as"] = json!(reads_as(*c));
        }
    };
    let ground: Vec<Value> = tlib
        .materials
        .iter()
        .map(|m| {
            let mut v = json!({ "id": m.id, "name": m.name, "category": m.category });
            if !crate::terrain::textures::library_available(&m.id) {
                v["downloads_when_used"] = json!(true);
            }
            color(
                crate::terrain::textures::library_file(&m.id, "albedo.png"),
                &mut v,
            );
            v
        })
        .collect();
    let layer_now: Map<String, Value> = Layer::ALL
        .iter()
        .map(|l| (snake(&format!("{l:?}")), json!(layers.get(*l as usize))))
        .collect();
    let footsteps: Map<String, Value> = Layer::ALL
        .iter()
        .map(|l| {
            let s = ed.layer_surface[*l as usize];
            (snake(&format!("{l:?}")), json!(snake(&format!("{s:?}"))))
        })
        .collect();
    let building: Vec<Value> = slib
        .as_ref()
        .map(|s| {
            s.entries
                .iter()
                .filter(|e| e.tiling)
                .map(|e| {
                    let mut v = json!({ "id": e.id, "name": e.name, "category": e.category });
                    if !crate::surfaces::available(&e.id) {
                        v["downloads_when_used"] = json!(true);
                    }
                    color(crate::surfaces::item_file(&e.id, "albedo.png"), &mut v);
                    v
                })
                .collect()
        })
        .unwrap_or_default();
    let parts_now: Map<String, Value> = Slot::ALL
        .iter()
        .map(|s| {
            (
                snake(&format!("{s:?}")),
                json!({ "name": s.name(), "current": parts.get(*s), "accepts": s.categories() }),
            )
        })
        .collect();
    let st = &t.style;
    json_out(json!({
        "ground_textures": ground,
        "ground_layers": layer_now,
        "building_materials": building,
        "building_parts": parts_now,
        "style": { "snow_line": st.snow_line, "snow_blend": st.snow_blend, "rock_slope": st.rock_slope, "beach_height": st.beach_height, "dryness": st.dryness, "wet_gullies": st.wet_gullies },
        "footsteps": footsteps,
        "note": "\"\" means the built-in / default material; color_srgb is each texture's average colour (what it reads as from a distance)",
    }))
}

#[allow(clippy::too_many_arguments)]
fn set(
    In(args): In<Value>,
    tlib: Res<TerrainLibrary>,
    mut layers: ResMut<TerrainLayerChoices>,
    slib: Option<Res<SurfaceLibrary>>,
    mut parts: ResMut<SurfaceChoices>,
    mut t: ResMut<Terrain>,
    mut ed: ResMut<crate::editor::Editor>,
    mut meta: ResMut<crate::world::WorldMeta>,
) -> R {
    let mut done = vec![];
    // check everything before changing anything
    let mut ground = vec![];
    if let Some(Value::Object(m)) = args.get("ground") {
        for (k, v) in m {
            let layer = pick(&Layer::ALL, k, "ground layer", Layer::name)?;
            let id = v
                .as_str()
                .ok_or_else(|| format!("ground.{k}: a texture id string"))?;
            let mat = tlib.materials.iter().find(|m| m.id == id);
            if !id.is_empty() && mat.is_none() {
                return Err(format!(
                    "unknown ground texture `{id}` (see materials_list)"
                ));
            }
            ground.push((layer, id.to_string(), mat.map(|m| m.surface.clone())));
        }
    }
    let mut footsteps = vec![];
    if let Some(Value::Object(m)) = args.get("footsteps") {
        for (k, v) in m {
            let layer = pick(&Layer::ALL, k, "ground layer", Layer::name)?;
            let name = v.as_str().unwrap_or_default();
            let surf = Surface::ALL
                .iter()
                .find(|x| snake(&format!("{x:?}")) == snake(name) || x.sound_stem() == name)
                .copied()
                .ok_or_else(|| format!("footsteps.{k}: unknown surface `{name}`"))?;
            footsteps.push((layer, surf));
        }
    }
    let mut building = vec![];
    if let Some(Value::Object(m)) = args.get("buildings") {
        for (k, v) in m {
            let slot = pick(&Slot::ALL, k, "building part", Slot::name)?;
            let id = v
                .as_str()
                .ok_or_else(|| format!("buildings.{k}: a material id string"))?;
            if !id.is_empty()
                && !slib
                    .as_ref()
                    .is_some_and(|s| s.entries.iter().any(|e| e.id == id && e.tiling))
            {
                return Err(format!(
                    "unknown building material `{id}` (see materials_list)"
                ));
            }
            building.push((slot, id.to_string()));
        }
    }
    for (layer, id, surface) in ground {
        // footsteps follow the material, as in the Paint tab
        if let Some(surf) =
            surface.and_then(|s| Surface::ALL.iter().find(|x| format!("{x:?}") == s).copied())
        {
            ed.layer_surface[layer as usize] = surf;
        }
        layers.set(layer as usize, &id);
        done.push(format!(
            "{} ground: {}",
            layer.name(),
            if id.is_empty() { "built-in" } else { &id }
        ));
    }
    for (slot, id) in building {
        parts.set(slot, &id);
        done.push(format!(
            "{}: {}",
            slot.name(),
            if id.is_empty() { "default" } else { &id }
        ));
    }
    if let Some(s) = args.get("style") {
        let st = &mut t.style;
        for (k, f, lo, hi) in [
            ("snow_line", &mut st.snow_line, 20.0, 800.0),
            ("snow_blend", &mut st.snow_blend, 2.0, 150.0),
            ("rock_slope", &mut st.rock_slope, 0.15, 0.9),
            ("beach_height", &mut st.beach_height, 0.0, 12.0),
            ("dryness", &mut st.dryness, 0.0, 1.0),
            ("wet_gullies", &mut st.wet_gullies, 0.0, 1.0),
        ] {
            if let Some(v) = opt_num(s, k)? {
                *f = v.clamp(lo, hi);
                done.push(format!("{k} {:.2}", *f));
            }
        }
    }
    for (layer, surf) in footsteps {
        ed.layer_surface[layer as usize] = surf;
        done.push(format!("{} footsteps: {}", layer.name(), surf.sound_stem()));
    }
    if done.is_empty() {
        return Err("nothing to change: give `ground`, `buildings`, `style` or `footsteps`".into());
    }
    meta.dirty = true;
    text(format!("{}.", done.join("; ")))
}

/// Mean colours of texture files (sRGB 0..1, averaged in linear light like the eye sees a
/// texture from afar). Decoded once per file and kept for the session.
fn mean_colors(
    paths: &[std::path::PathBuf],
) -> std::collections::HashMap<std::path::PathBuf, [f32; 3]> {
    use rayon::prelude::*;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, Option<[f32; 3]>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let todo: Vec<&std::path::PathBuf> = {
        let c = cache.lock().unwrap_or_else(|e| e.into_inner());
        paths.iter().filter(|p| !c.contains_key(*p)).collect()
    };
    let found: Vec<(std::path::PathBuf, Option<[f32; 3]>)> = todo
        .par_iter()
        .map(|p| ((*p).clone(), mean_color(p)))
        .collect();
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    c.extend(found);
    paths
        .iter()
        .filter_map(|p| c.get(p).copied().flatten().map(|v| (p.clone(), v)))
        .collect()
}

fn mean_color(path: &std::path::Path) -> Option<[f32; 3]> {
    let img = image::open(path).ok()?.to_rgb8();
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let lut: Vec<f32> = (0..=255u8).map(lin).collect();
    let mut sum = [0f64; 3];
    for p in img.pixels() {
        for k in 0..3 {
            sum[k] += lut[p.0[k] as usize] as f64;
        }
    }
    let n = (img.width() as f64 * img.height() as f64).max(1.0);
    let srgb = |l: f64| {
        let l = (l / n) as f32;
        if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        }
    };
    Some([srgb(sum[0]), srgb(sum[1]), srgb(sum[2])])
}

/// A plain colour word for an sRGB colour ("dark green", "khaki", "grey"…).
pub(super) fn reads_as(c: [f32; 3]) -> String {
    let (r, g, b) = (c[0], c[1], c[2]);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) * 0.5;
    let d = mx - mn;
    let s = if d < 1e-5 {
        0.0
    } else {
        d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-5)
    };
    if s < 0.1 || d < 0.04 {
        return match l {
            l if l < 0.12 => "black",
            l if l < 0.3 => "dark grey",
            l if l < 0.6 => "grey",
            l if l < 0.85 => "light grey",
            _ => "white",
        }
        .into();
    }
    let h = if mx == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let shade = |name: &str| {
        if l < 0.25 {
            format!("dark {name}")
        } else if l > 0.7 {
            format!("light {name}")
        } else {
            name.to_string()
        }
    };
    // earth tones first: muted or dark reds, oranges and yellows
    match h {
        h if (15.0..35.0).contains(&h) && (l < 0.45 || s < 0.55) => (if l < 0.25 {
            "dark brown"
        } else if l > 0.6 {
            "tan"
        } else {
            "brown"
        })
        .into(),
        h if (35.0..70.0).contains(&h) && (s < 0.55 || l < 0.3) => (if l > 0.55 {
            "khaki"
        } else if l < 0.2 {
            "dark olive"
        } else if l < 0.4 && h < 45.0 {
            "brown"
        } else {
            "olive"
        })
        .into(),
        h if (70.0..160.0).contains(&h) && s < 0.25 => shade("grey-green"),
        h if !(15.0..345.0).contains(&h) && l < 0.35 => "dark red".into(),
        h if h < 15.0 || h >= 345.0 => shade("red"),
        h if h < 45.0 => shade("orange"),
        h if h < 70.0 => shade("yellow"),
        h if h < 160.0 => shade("green"),
        h if h < 200.0 => shade("teal"),
        h if h < 255.0 => shade("blue"),
        h if h < 290.0 => shade("purple"),
        _ => shade("pink"),
    }
}

#[cfg(test)]
mod tests {
    use super::reads_as;

    #[test]
    fn colour_words() {
        assert_eq!(reads_as([0.1, 0.25, 0.08]), "dark green");
        assert_eq!(reads_as([0.765, 0.69, 0.569]), "khaki");
        assert_eq!(reads_as([0.545, 0.353, 0.169]), "brown");
        assert_eq!(reads_as([0.5, 0.5, 0.5]), "grey");
        assert_eq!(reads_as([0.05, 0.05, 0.06]), "black");
        assert_eq!(reads_as([0.8, 0.62, 0.45]), "tan");
        assert_eq!(reads_as([0.2, 0.3, 0.7]), "blue");
        assert_eq!(reads_as([0.36, 0.34, 0.25]), "olive");
    }
}
