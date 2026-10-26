//! Scene tools: lighting, art style, weather and wind, the camera, and screenshots.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use serde_json::{Map, Value, json};

use super::*;
use crate::camera::OrbitCamera;
use crate::clouds::{CloudPreset, CloudQuality};
use crate::look::{LookParams, LookPreset, ToneCurve};
use crate::mcp::McpServer;
use crate::mcp_tools::{
    McpAppExt, McpContent, McpTool, bearing_dir, bearing_schema, compass, dir_bearing,
    view_bearing, view_yaw,
};
use crate::objects::Wind;
use crate::sky::{Lighting, ShadowQuality};
use crate::terrain::Terrain;
use crate::world::Weather;

const TIMES: [(&str, f32); 5] = [
    ("dawn", 6.2),
    ("morning", 9.0),
    ("noon", 12.5),
    ("dusk", 18.6),
    ("night", 23.0),
];

fn camera_props() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("preset".into(), json!({ "type": "string", "enum": ["overview", "top"], "description": "overview: the whole world at an angle; top: straight down over the whole world" }));
    m.insert("focus".into(), json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3, "description": "[x, z] the camera orbits and looks at (kept on the ground)" }));
    m.insert("yaw_deg".into(), bearing_schema("the direction the camera looks (0: from the south looking north; 90: from the west looking east)"));
    m.insert("pitch_deg".into(), json!({ "type": "number", "minimum": 2, "maximum": 87, "description": "angle above the horizon (2 low and cinematic … 87 top-down)" }));
    m.insert("distance".into(), json!({ "type": "number", "minimum": 3, "maximum": 60000, "description": "metres from the focus" }));
    m.insert("eye".into(), json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "camera position [x, y, z]; with `target` instead of focus / yaw / pitch / distance" }));
    m.insert("target".into(), json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3, "description": "what the camera at `eye` looks at, [x, z] (ground) or [x, y, z]" }));
    m.insert("smooth".into(), json!({ "type": "boolean", "default": false, "description": "glide there like the editor does instead of cutting" }));
    m
}

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "lighting_set",
        description: "Time of day, sun, moon and shadows (Lighting tab). `hour` 0..24 (or `time`: dawn, morning, noon, dusk, night); the day cycle can `play`. A setting, not an undo step; saved with the world. \
`cloud_sea` (also in weather_set) lays a low deck of sunlit cloud in the valleys and canyons, peaks standing out of it (off by default). \
`render_resolution` (Lighting › Performance) is how many pixels the 3D view draws before it is upscaled to the window (the panels stay sharp): the biggest frame-rate setting; saved for this computer, not with the world.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "hour": { "type": "number", "minimum": 0, "maximum": 24 },
                    "time": { "type": "string", "enum": TIMES.map(|t| t.0) },
                    "play": { "type": "boolean", "description": "run the 24 h day/night cycle" },
                    "day_minutes": { "type": "number", "minimum": 0.5, "maximum": 60, "description": "real minutes per in-game day while playing" },
                    "sun_tilt": { "type": "number", "minimum": 5, "maximum": 75, "description": "degrees the sun path leans (noon elevation = 90 − tilt)" },
                    "sun_azimuth": bearing_schema("where the sun rises (default 110, east-south-east; the sun sets opposite)"),
                    "sun_strength": { "type": "number", "minimum": 0.2, "maximum": 2 },
                    "moon_strength": { "type": "number", "minimum": 0, "maximum": 3 },
                    "moon_phase": { "type": "number", "minimum": 0, "maximum": 1, "description": "0 new, 0.25 first quarter, 0.5 full, 0.75 last quarter" },
                    "night_brightness": { "type": "number", "minimum": 0, "maximum": 3 },
                    "exposure": { "type": "number", "minimum": -3, "maximum": 3, "description": "EV" },
                    "haze_enabled": { "type": "boolean", "description": "master switch for outdoor distance fog, atmospheric colour haze, lit air, mist and sun rays; keeps their amounts for later restoration and leaves dungeon and underwater fog alone" },
                    "haze": { "type": "number", "minimum": 0, "maximum": 1, "description": "atmospheric haze" },
                    "shadow_quality": enum_schema(ids(&[ShadowQuality::Low, ShadowQuality::Medium, ShadowQuality::High]), "sun shadow resolution"),
                    "contact_shadows": { "type": "boolean", "description": "fine screen-space shadows where things touch the ground (costs frame rate)" },
                    "ambient_occlusion": enum_schema(ids(&crate::camera::AmbientOcclusion::ALL), "screen-space ambient occlusion quality: darkens creases and the ground round objects so they sit in the world (default medium; costs frame rate with the window's pixels; off turns it off). Its strength is the look's `occlusion` param (look_set)"),
                    "anti_aliasing": enum_schema(ids(&crate::camera::AntiAliasing::ALL), "smaa: crisp, but foliage and thin lines shimmer in motion; taa (default): temporal, steady foliage, slightly softer"),
                    "forest_contours": { "type": "object", "description": "soften dense tree contours in the distance while preserving foreground trees", "properties": {
                        "enable": { "type": "boolean" },
                        "strength": { "type": "number", "minimum": 0, "maximum": 1 },
                        "start_distance": { "type": "number", "minimum": 30, "maximum": 1000, "description": "metres from the camera where contour softening begins" }
                    } },
                    "eye_adaptation": enum_schema(ids(&crate::camera_fx::EyeAdaptation::ALL), "the eye adapting to what is on screen, round the time of day's exposure (up to about +1.5 EV into the dark, -1.25 EV into the light): walking (default) adapts while walking and in camera paths (a shaded lane or a building opens up over a couple of seconds; stepping back into the sun dazzles for a moment) and keeps the editor view's exposure fixed; always also adapts the editor view, at half strength; off: the fixed exposure everywhere. Caves and dungeons keep their own exposure"),
                    "depth_of_field": enum_schema(ids(&crate::camera_fx::DepthOfFieldMode::ALL), "cinematic depth of field (bokeh): off (default); camera_paths: while a camera path plays or captures; always: the editor view too, for stills and screenshots (never while walking)"),
                    "dof_focus": { "type": "number", "minimum": 0, "maximum": 20000, "description": "depth of field focus distance (m); 0 = automatic: what is at the centre of the view, racked to smoothly" },
                    "dof_f_stops": { "type": "number", "minimum": 0.5, "maximum": 32, "description": "depth of field aperture in f-stops (default 2): lower = shallower focus. The blur follows a real lens (the field of view sets the focal length), so close shots and narrow fields of view blur most; a wide landscape stays sharp" },
                    "volumetric_light": enum_schema(ids(&crate::volumetrics::VolumetricQuality::ALL), "lit air (default medium): sunbeams streaming between trees and buildings and through windows, the glow round the sun, ground mist, weather fog that the sun shines into, halos round lamps and fires. Off falls back to flat distance fog. Higher = finer shafts, more GPU (the cost is per froxel, not per pixel). How hazy / misty is the look's `air` and `mist` (look_set)"),
                    "stars": { "type": "boolean" },
                    "cloud_sea": crate::clouds::sea::CloudSea::mcp_schema(),
                    "bounce_light": { "type": "boolean", "description": "bounce light while walking (default on): sunlit walls, streets and squares throw warm light into the shade round them, and narrow lanes, arcades and rooms get darker where buildings hide the sky (a light volume round the walker, worked out in the background as they go; open ground looks the same either way). The editor view keeps the plain sky light" },
                    "render_resolution": enum_schema(crate::render_scale::RenderResolution::ALL.iter().map(|r| r.id().to_string()).collect(), "percent of the window's pixels (per axis) the 3D view renders: auto (75 on Retina / 5K displays, else 100), 100, 85, 75, 67 or 50"),
                    "save_power_in_background": { "type": "boolean", "description": "while another app is in front the editor updates 10×/s instead of 60 (MCP calls keep it at full speed); saved for this computer" }
                }
            })
        },
        run: |w, a| run(w, lighting, a),
    });
    app.mcp_tool(McpTool {
        name: "look_set",
        description: "The art style (Style tab): a `preset` regrades the whole world (natural, nordic, cinematic; the scene templates epic_sunbeams, overcast, lavender, golden_vintage, rose_haze, hellfire, enchanted, golden_hour also set their time of day, sky and haze unless `apply_scene` is false). \
`strength` blends from natural (0) to the full look (1). `params` fine-tunes single values of the look (exposure, temperature, saturation, contrast, bloom, vignette, haze, tone…: see its properties; colours as [r, g, b]). A setting, saved with the world.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "preset": enum_schema(ids(&LookPreset::ALL), "art style"),
                    "strength": { "type": "number", "minimum": 0, "maximum": 1 },
                    "apply_scene": { "type": "boolean", "default": true },
                    "params": { "type": "object", "description": "look parameters to override", "properties": look_params_schema() },
                    "reset_params": { "type": "boolean", "description": "drop fine-tuning: back to the preset as designed" }
                }
            })
        },
        run: |w, a| run(w, look, a),
    });
    app.mcp_tool(McpTool {
        name: "look_shuffle",
        description: "Style › Fine-tune › Shuffle settings: new values for every fine-tune setting of the world's art style (colour grade, split toning, light, haze, sky tint, hazy air, ground mist, lens, brushwork), drawn from a mood — sunlit, crisp alpine, misty, storybook, dusky, moody film, faded vintage, eerie, dreamy pastel, bold heroic; by default mostly one that suits the style — and blended with the style as designed, so the style itself stays (Overcast stays Overcast). Call again for another; look_style_save keeps one; look_set reset_params goes back to the style as designed. `style` first switches to that built-in style (as look_set preset, without its scene); `mood` picks the mood; `seed` repeats a shuffle; `time_of_day` also sets an hour that suits it.",
        input_schema: || json!({ "type": "object", "properties": {
            "style": enum_schema(ids(&LookPreset::ALL), "the built-in style to shuffle round (default: the world's own)"),
            "mood": { "type": "string", "enum": crate::look_shuffle::moods().iter().map(|m| m.to_lowercase().replace(' ', "_")).collect::<Vec<_>>() },
            "seed": { "type": "integer", "minimum": 0 },
            "time_of_day": { "type": "boolean", "default": false }
        } }),
        run: |w, a| {
            let seed = a.get("seed").and_then(|v| v.as_u64()).unwrap_or_else(|| fastrand::u64(..));
            let mood = opt_str(&a, "mood")?.map(|m| m.replace('_', " "));
            let style = opt_pick(&a, "style", &LookPreset::ALL, "style", LookPreset::name)?;
            let mut l = w.resource_mut::<Lighting>();
            if let Some(p) = style {
                l.look.select(p);
            }
            let preset = l.look.preset;
            let sh = crate::look_shuffle::shuffle(seed, preset, mood.as_deref());
            l.look.custom = Some(sh.params);
            l.look.saved = None;
            if a.get("time_of_day").and_then(|v| v.as_bool()).unwrap_or(false) {
                l.hour = sh.hour;
            }
            w.resource_mut::<crate::world::WorldMeta>().dirty = true;
            json_out(json!({ "style": snake(&format!("{:?}", preset)), "mood": sh.mood, "seed": seed, "hour": (sh.hour * 10.0).round() / 10.0, "params": sh.params, "note": "look_style_save keeps it; look_set reset_params undoes it" }))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_style_list",
        description: "The user's own saved art styles (Style tab › My styles): each one's name, the built-in style it was made from and its file. Saved styles live on this computer (worlds/styles) and are shared as .skystyle files; look_style_apply puts one on the world.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, _a| {
            let current = w.resource::<Lighting>().look.saved.clone();
            let list: Vec<Value> = crate::look_styles::list()
                .iter()
                .map(|s| json!({ "name": s.name, "based_on": snake(&format!("{:?}", s.based_on)), "file": crate::look_styles::dir().join(crate::look_styles::file_name(&s.name)).display().to_string() }))
                .collect();
            json_out(json!({ "styles": list, "in_use": current, "folder": crate::look_styles::dir().display().to_string() }))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_style_save",
        description: "Saves the world's art style as it shows now (colour grade, light, haze, lens, ambient occlusion, sun rays: everything look_set sets, its strength folded in) as the user's own style under `name` (default: the next free \"My style N\"), to apply to any world later or share. A style of the same name is replaced. The world then uses the saved style.",
        input_schema: || json!({ "type": "object", "properties": { "name": { "type": "string", "maxLength": 48 } } }),
        run: |w, a| {
            let name = opt_str(&a, "name")?.map(str::to_string).unwrap_or_else(crate::look_styles::next_name);
            let style = crate::look_styles::SavedStyle::from_look(&name, &w.resource::<Lighting>().look);
            let path = crate::look_styles::save(&style)?;
            style.apply(&mut w.resource_mut::<Lighting>().look);
            w.resource_mut::<crate::world::WorldMeta>().dirty = true;
            text(format!("Saved the style as \"{}\" ({}).", style.name, path.display()))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_style_apply",
        description: "Puts one of the user's saved styles (look_style_list) on the world: its values become the world's look (saved with the world, so it keeps the look even without the style file). Fine-tune further with look_set (its params).",
        input_schema: || json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
        run: |w, a| {
            let name = opt_str(&a, "name")?.ok_or("`name` is required")?;
            let style = crate::look_styles::find(name).ok_or_else(|| format!("no saved style named `{name}` (look_style_list)"))?;
            style.apply(&mut w.resource_mut::<Lighting>().look);
            w.resource_mut::<crate::world::WorldMeta>().dirty = true;
            text(format!("The world now uses the style \"{}\" (made from {}).", style.name, style.based_on.name()))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_style_delete",
        description: "Deletes one of the user's saved styles from this computer (worlds already using it keep their look).",
        input_schema: || json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
        run: |_w, a| {
            let name = opt_str(&a, "name")?.ok_or("`name` is required")?;
            crate::look_styles::delete(name)?;
            text(format!("Deleted the style \"{name}\"."))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_style_export",
        description: "Writes one of the user's saved styles to a .skystyle file at `path` (absolute) to share it; look_style_import adds such a file on another computer.",
        input_schema: || json!({ "type": "object", "properties": { "name": { "type": "string" }, "path": { "type": "string", "description": "absolute path of the .skystyle file to write" } }, "required": ["name", "path"] }),
        run: |_w, a| {
            let name = opt_str(&a, "name")?.ok_or("`name` is required")?;
            let path = opt_str(&a, "path")?.ok_or("`path` is required")?;
            let out = crate::look_styles::export(name, std::path::Path::new(path))?;
            text(format!("Wrote \"{name}\" to {}.", out.display()))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_style_import",
        description: "Adds a shared .skystyle file (absolute `path`) to the user's saved styles (a name already taken gets a number) and, unless `apply` is false, puts it on the world.",
        input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "apply": { "type": "boolean", "default": true } }, "required": ["path"] }),
        run: |w, a| {
            let path = opt_str(&a, "path")?.ok_or("`path` is required")?;
            let style = crate::look_styles::import(std::path::Path::new(path))?;
            if opt_bool(&a, "apply")?.unwrap_or(true) {
                style.apply(&mut w.resource_mut::<Lighting>().look);
                w.resource_mut::<crate::world::WorldMeta>().dirty = true;
            }
            text(format!("Imported the style \"{}\".", style.name))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_lut_import",
        description: "Loads the user's own colour LUT onto the world's art style (Style › Fine-tune › Pro grading › LUT): a .cube file (from DaVinci Resolve, Photoshop, Lightroom…), a LUT strip picture (.png: Unreal 256 × 16, Unity / Godot 1024 × 32) or a HALD picture. It is applied after the style's own colour grade, faded by `strength` (0..1, default 1), saved with the world and in saved styles, and exported to Unreal, Unity and Godot. It stays when another style is picked; look_lut_set clears it. LUTs are expected to take display (sRGB / Rec.709) colour.",
        input_schema: || json!({ "type": "object", "properties": {
            "path": { "type": "string", "description": "absolute path of the .cube or LUT picture" },
            "strength": { "type": "number", "minimum": 0, "maximum": 1, "default": 1 },
            "name": { "type": "string", "maxLength": 80, "description": "shown in the UI (default: the file name)" }
        }, "required": ["path"] }),
        run: |w, a| {
            let path = std::path::PathBuf::from(opt_str(&a, "path")?.ok_or("`path` is required")?);
            let lut = crate::grade::files::load(&path)?;
            let name = opt_str(&a, "name")?.map(str::to_string).unwrap_or_else(|| file_stem(&path));
            let strength = num(&a, "strength", 1.0)?.clamp(0.0, 1.0);
            set_user_lut(w, Some(crate::grade::UserLut::new(&name, lut)), strength);
            text(format!("The world's style now uses the LUT \"{name}\" at {:.0}%.", strength * 100.0))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_lut_set",
        description: "The world's own colour LUT (look_lut_import, look_grade_import): `strength` (0..1) fades it, `clear` removes it. With neither, says which LUT is on and how strong.",
        input_schema: || json!({ "type": "object", "properties": {
            "strength": { "type": "number", "minimum": 0, "maximum": 1 },
            "clear": { "type": "boolean" }
        } }),
        run: |w, a| {
            if opt_bool(&a, "clear")?.unwrap_or(false) {
                let had = w.resource::<Lighting>().look.lut.is_some();
                set_user_lut(w, None, 1.0);
                return text(if had { "Removed the LUT." } else { "The world has no LUT." }.to_string());
            }
            if let Some(s) = a.get("strength").and_then(Value::as_f64) {
                let mut l = w.resource_mut::<Lighting>();
                if l.look.lut.is_none() {
                    return Err("the world has no LUT: look_lut_import loads one".into());
                }
                l.look.lut_strength = (s as f32).clamp(0.0, 1.0);
                w.resource_mut::<crate::world::WorldMeta>().dirty = true;
            }
            let look = &w.resource::<Lighting>().look;
            json_out(match &look.lut {
                Some(l) => json!({ "lut": l.name, "strength": look.lut_strength, "size": l.lut.size }),
                None => json!({ "lut": null }),
            })
        },
    });
    app.mcp_tool(McpTool {
        name: "look_lut_export",
        description: "Writes the world's whole colour grade as it shows now (the art style's saturation, contrast, split toning, lift, greens, vibrance and the user's LUT; not exposure, light or haze) as a .cube LUT at `path` (absolute), for DaVinci Resolve, Photoshop, Unreal, Unity or any app that reads .cube. It takes and gives display (sRGB) colour after tonemapping.",
        input_schema: || json!({ "type": "object", "properties": {
            "path": { "type": "string", "description": "absolute path of the .cube file to write" },
            "size": { "type": "integer", "minimum": 17, "maximum": 65, "default": 33 }
        }, "required": ["path"] }),
        run: |w, a| {
            let mut path = std::path::PathBuf::from(opt_str(&a, "path")?.ok_or("`path` is required")?);
            if !path.is_absolute() {
                return Err("`path` must be absolute".into());
            }
            if path.extension().is_none_or(|e| e != "cube") {
                path.set_extension("cube");
            }
            let n = num(&a, "size", 33.0)?.clamp(17.0, 65.0) as usize;
            let look = &w.resource::<Lighting>().look;
            let lk = look.effective();
            let user = look.user_lut();
            let lut = crate::grade::Lut3::from_fn(n, |c| crate::grade::srgb(&lk, user, c));
            let title = w.resource::<crate::world::WorldMeta>().name.clone();
            std::fs::write(&path, crate::grade::files::write_cube(&lut, &title))
                .map_err(|e| format!("could not write {}: {e}", path.display()))?;
            text(format!("Wrote the grade ({n}³) to {}.", path.display()))
        },
    });
    app.mcp_tool(McpTool {
        name: "look_grade_capture",
        description: "Step 1 of grading in another app (Photoshop, Lightroom, Affinity, GIMP, Resolve): saves the view as it looks now (editor UI hidden) to `path` (absolute .png) with a neutral colour strip along its bottom edge. Grade that picture with whole-picture adjustments only (curves, levels, hue / saturation, colour balance, selective colour, a LUT; no painting, cropping or resizing), save it as PNG or TIFF, then look_grade_import reads the grade back from the strip as the world's LUT.",
        input_schema: || json!({ "type": "object", "properties": {
            "path": { "type": "string", "description": "absolute path of the .png to write" }
        }, "required": ["path"] }),
        run: grade_capture,
    });
    app.mcp_tool(McpTool {
        name: "look_grade_import",
        description: "Step 2 of grading in another app: reads the colour strip of a picture made by look_grade_capture and graded elsewhere (absolute `path`, .png or .tif; JPEG works but is less exact) and puts that grade on the world as its LUT (on top of the style's own grade, `strength` 0..1, default 1).",
        input_schema: || json!({ "type": "object", "properties": {
            "path": { "type": "string" },
            "strength": { "type": "number", "minimum": 0, "maximum": 1, "default": 1 }
        }, "required": ["path"] }),
        run: |w, a| {
            let path = std::path::PathBuf::from(opt_str(&a, "path")?.ok_or("`path` is required")?);
            let img = image::open(&path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
            let lut = crate::grade::files::from_round_trip(&img.into_rgb32f())?;
            if lut.is_identity(1.5 / 255.0) {
                return Err("the colour strip is unchanged: grade the picture in the other app first (whole-picture adjustments), then save it and import it".into());
            }
            let name = format!("Graded: {}", file_stem(&path));
            let strength = num(&a, "strength", 1.0)?.clamp(0.0, 1.0);
            set_user_lut(w, Some(crate::grade::UserLut::new(&name, lut)), strength);
            text(format!("The world's style now uses the grade from {} as its LUT.", path.display()))
        },
    });
    app.mcp_tool(McpTool {
        name: "weather_set",
        description: "Clouds, weather and wind (Weather tab). A `cloud_preset` picks a sky (clear, fair_weather, scattered, broken, overcast, cirrus_only, towering) or fine-tune cover / type / thickness / cirrus; `weather` previews one weather everywhere (\"follow_zones\" lets zones and biomes decide: rain in jungles, snow on tundra); \
`rain_and_snow` false keeps everything dry. Wind drives trees, grass, smoke, the sea, cloth and moving parts (windmill sails turn faster in a stronger wind). Settings, saved with the world. \
`cloud_sea` lays a low deck of cloud in the valleys and canyons (peaks standing out of it; off by default). \
`preview_motion` true animates wind, cloth and moving parts while editing too (otherwise the world holds still until walk mode). \
Rain: the ground, roads and placed models get wet as it rains (in about a minute) and dry over minutes after; `wetness` holds them at a wetness (puddles full at 1; -1 follows the weather again). \
`puddles` is how much standing water rain leaves, `lightning` how often it strikes in thunderstorms (now and then in heavy rain), `thunder` its sound; `lightning_now` strikes once at once, in front of the camera.",
        input_schema: || {
            let mut w = vec!["follow_zones".to_string()];
            w.extend(ids(&Weather::ALL));
            json!({
                "type": "object",
                "properties": {
                    "cloud_preset": enum_schema(ids(&CloudPreset::ALL), "sky preset"),
                    "randomize_sky": { "type": "boolean", "description": "a random pleasant sky" },
                    "cloud_cover": { "type": "number", "minimum": 0, "maximum": 1 },
                    "cloud_type": { "type": "number", "minimum": 0, "maximum": 1, "description": "0 flat stratus … 0.5 stratocumulus … 1 towering cumulus" },
                    "cloud_thickness": { "type": "number", "minimum": 0.3, "maximum": 2 },
                    "cirrus": { "type": "number", "minimum": 0, "maximum": 1 },
                    "cloud_speed": { "type": "number", "minimum": 0, "maximum": 5 },
                    "cloud_shadows": { "type": "number", "minimum": 0, "maximum": 1 },
                    "cloud_seed": { "type": "integer", "minimum": 0, "description": "a different cloud pattern" },
                    "cloud_quality": enum_schema(ids(&CloudQuality::ALL), "cost of the volumetric clouds"),
                    "cloud_sea": crate::clouds::sea::CloudSea::mcp_schema(),
                    "weather": enum_schema(w, "weather everywhere"),
                    "rain_and_snow": { "type": "boolean" },
                    "wind_speed": { "type": "number", "minimum": 0, "maximum": 25, "description": "m/s; also how loud the wind is heard (silent when calm)" },
                    "wind_direction_deg": bearing_schema("direction the wind blows towards"),
                    "gustiness": { "type": "number", "minimum": 0, "maximum": 1, "description": "also how much of the wind heard comes in gusts" },
                    "preview_motion": { "type": "boolean", "description": "animate wind, cloth and moving parts while editing (the Weather tab's Preview motion)" },
                    "wetness": { "type": "number", "minimum": -1, "maximum": 1, "description": "hold how wet everything is: 0 dry … 1 soaked with full puddles (not saved); -1 follows the weather again" },
                    "puddles": { "type": "number", "minimum": 0, "maximum": 1, "description": "standing water rain leaves: 0 none … 1 a lot (saved)" },
                    "lightning": { "type": "number", "minimum": 0, "maximum": 3, "description": "how often lightning strikes in thunderstorms: 0 never, 1 normal, 3 violent (saved)" },
                    "thunder": { "type": "boolean", "description": "thunder after each strike (saved)" },
                    "lightning_now": { "type": "boolean", "description": "strike once now, in front of the camera (any weather)" }
                }
            })
        },
        run: |w, a| run(w, weather, a),
    });
    app.mcp_tool(McpTool {
        name: "camera_set",
        description: "Move the editor camera (it orbits a focus point on the ground): `focus` + `yaw_deg` + `pitch_deg` + `distance` (any subset), or `eye` + `target`, or a `preset`. Returns the camera. Screenshots show what it sees.",
        input_schema: || json!({ "type": "object", "properties": camera_props() }),
        run: |w, a| run(w, camera, a),
    });
    app.mcp_tool(McpTool {
        name: "camera_pick",
        description: "What a click at a point of the view would hit, the way the editor's cursor does: the terrain where it is drawn, and in caves their floors, walls and roofs (also from a camera inside a cave). `at` is [x, y] across the view from its top left (0..1 each; default the middle). Returns the point, what it is (ground, cave floor, cave wall, cave roof) and the rock's normal there.",
        input_schema: || json!({ "type": "object", "properties": { "at": { "type": "array", "items": { "type": "number", "minimum": 0, "maximum": 1 }, "minItems": 2, "maxItems": 2, "default": [0.5, 0.5] } } }),
        run: |w, a| run(w, camera_pick, a),
    });
    app.mcp_tool(McpTool {
        name: "camera_work_light",
        description: "The editor's work light (the B key; the Work light button over the view in a cave): a lamp over where the camera looks (in a cave, in the rock over its roof) and a softer one behind the camera, so dark caves and night scenes can be seen while decorating. Editing only: it is off in walk mode, never saved or exported, and shows in screenshots while on. `on` switches it; without it, just reports.",
        input_schema: || json!({ "type": "object", "properties": { "on": { "type": "boolean" } } }),
        run: |w, a| {
            let mut wl = w.resource_mut::<crate::camera::WorkLight>();
            if let Some(on) = opt_bool(&a, "on")? {
                wl.on = on;
            }
            json_out(json!({ "work_light": wl.on }))
        },
    });
    app.mcp_tool(McpTool {
        name: "screenshot",
        description: "Look at the world: renders the editor view and returns it as a PNG image (`max_size` px on the long side, default 1024). The editor UI is hidden unless `hide_ui` is false. \
Optionally moves the camera first (`camera`: the camera_set arguments) and waits for the view to settle. `clear_sky` holds clouds, fog, rain and snow off while it captures (biome and zone weather included). `save_path` (absolute .png) also saves the full-size image. The editor window must not be minimised.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "camera": { "type": "object", "properties": camera_props(), "description": "camera_set arguments" },
                    "hide_ui": { "type": "boolean", "default": true },
                    "clear_sky": { "type": "boolean", "default": false, "description": "no clouds, fog, rain or snow in this capture" },
                    "max_size": { "type": "integer", "minimum": 64, "maximum": 4096, "default": 1024 },
                    "settle_frames": { "type": "integer", "minimum": 1, "maximum": 600, "description": "frames to wait before capturing (default 4, or 30 after a camera move so streaming and clouds settle)" },
                    "save_path": { "type": "string", "description": "absolute path of a .png to save the full-size image to" }
                }
            })
        },
        run: screenshot,
    });
}

fn file_stem(p: &std::path::Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "LUT".into())
}

fn set_user_lut(w: &mut World, lut: Option<crate::grade::UserLut>, strength: f32) {
    let mut l = w.resource_mut::<Lighting>();
    l.look.lut = lut;
    l.look.lut_strength = strength;
    w.resource_mut::<crate::world::WorldMeta>().dirty = true;
}

/// `look_grade_capture`: a screenshot (as `screenshot` takes it) with the neutral strip added.
fn grade_capture(world: &mut World, args: Value) -> R {
    let path = std::path::PathBuf::from(opt_str(&args, "path")?.ok_or("`path` is required")?);
    if !path.is_absolute() || path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("png")) {
        return Err("`path` must be an absolute path ending in .png".into());
    }
    let tmp = std::env::temp_dir().join(format!("skyfold_grade_capture_{}.png", std::process::id()));
    let mut shot = screenshot(world, json!({ "save_path": tmp.display().to_string(), "max_size": 64 }))?;
    let Some(McpContent::Deferred(crate::mcp_tools::McpDeferred(mut poll))) = shot.pop() else {
        return Err("the screenshot could not start".into());
    };
    Ok(vec![McpContent::deferred(move |w| {
        let done = poll(w)?;
        Some(done.and_then(|_| {
            let view = image::open(&tmp)
                .map_err(|e| format!("could not read the screenshot: {e}"))?
                .into_rgb8();
            let _ = std::fs::remove_file(&tmp);
            crate::grade::files::with_strip(&view)
                .save(&path)
                .map_err(|e| format!("could not save {}: {e}", path.display()))?;
            Ok(vec![McpContent::Text(format!(
                "Saved {} with its colour strip. Grade it in your photo app with whole-picture adjustments only, save it as PNG or TIFF, then import it (look_grade_import).",
                path.display()
            ))])
        }))
    })])
}

/// One property per look parameter (from the parameters themselves, so it can't go stale).
fn look_params_schema() -> Map<String, Value> {
    let mut m = Map::new();
    if let Ok(Value::Object(o)) = serde_json::to_value(LookParams::default()) {
        for (k, v) in o {
            let p = match (k.as_str(), v) {
                ("tone", _) => enum_schema(ids(&ToneCurve::ALL), "tone curve"),
                ("glare", _) => {
                    json!({ "type": "number", "minimum": 0, "maximum": 2, "description": "sun glare: the soft veiling glare and faint starburst round a bright sun in view, hidden by what stands in front of it and by cloud over it (Natural 0.4; Cinematic, Golden Hour, Epic Sunbeams 0.75–1)" })
                }
                ("flare", _) => {
                    json!({ "type": "number", "minimum": 0, "maximum": 2, "description": "lens flare: ghosts (hexagonal aperture images, rings) strung along the line from the sun through the centre of the view (0 none; Natural 0.12, Cinematic 0.55)" })
                }
                (_, Value::Array(_)) => color_schema("colour"),
                _ => json!({ "type": "number" }),
            };
            m.insert(k, p);
        }
    }
    m
}

fn lighting(
    In(args): In<Value>,
    mut lighting: ResMut<Lighting>,
    mut meta: ResMut<crate::world::WorldMeta>,
    mut rs: ResMut<crate::render_scale::RenderScale>,
    mut bg: ResMut<crate::background::BackgroundThrottle>,
    adapt: Res<crate::camera_fx::Adaptation>,
    bounce: Res<crate::bounce_light::BounceLight>,
) -> R {
    if let Some(r) = opt_str(&args, "render_resolution")? {
        rs.setting = crate::render_scale::RenderResolution::parse(r)
            .ok_or("`render_resolution` is one of auto, 100, 85, 75, 67, 50")?;
    }
    if let Some(b) = opt_bool(&args, "save_power_in_background")? {
        bg.0 = b;
    }
    let l = &mut *lighting;
    if let Some(t) = opt_str(&args, "time")? {
        l.hour = TIMES
            .iter()
            .find(|x| x.0 == t)
            .ok_or_else(|| format!("`time` must be one of {}", TIMES.map(|t| t.0).join(", ")))?
            .1;
    }
    if let Some(h) = opt_num(&args, "hour")? {
        l.hour = h.rem_euclid(24.0).min(23.99);
    }
    if let Some(p) = opt_bool(&args, "play")? {
        l.playing = p;
    }
    if let Some(on) = opt_bool(&args, "haze_enabled")? {
        l.haze_enabled = on;
    }
    for (k, f, lo, hi) in [
        ("day_minutes", &mut l.day_minutes, 0.5, 60.0),
        ("sun_tilt", &mut l.sun_tilt, 5.0, 75.0),
        ("sun_strength", &mut l.sun_strength, 0.2, 2.0),
        ("moon_strength", &mut l.moon_strength, 0.0, 3.0),
        ("moon_phase", &mut l.moon_phase, 0.0, 1.0),
        ("night_brightness", &mut l.night_brightness, 0.0, 3.0),
        ("exposure", &mut l.exposure_comp, -3.0, 3.0),
        ("haze", &mut l.haze, 0.0, 1.0),
    ] {
        if let Some(v) = opt_num(&args, k)? {
            *f = v.clamp(lo, hi);
        }
    }
    if let Some(b) = opt_num(&args, "sun_azimuth")? {
        // (internally 0 = rising due east)
        l.sun_azimuth = (b - 90.0 + 180.0).rem_euclid(360.0) - 180.0;
    }
    if let Some(q) = opt_pick(
        &args,
        "shadow_quality",
        &[
            ShadowQuality::Low,
            ShadowQuality::Medium,
            ShadowQuality::High,
        ],
        "shadow quality",
        |q| match q {
            ShadowQuality::Low => "low",
            ShadowQuality::Medium => "medium",
            ShadowQuality::High => "high",
        },
    )? {
        l.shadow_quality = q;
    }
    if let Some(b) = opt_bool(&args, "contact_shadows")? {
        l.contact_shadows = b;
    }
    if let Some(q) = opt_pick(
        &args,
        "ambient_occlusion",
        &crate::camera::AmbientOcclusion::ALL,
        "ambient occlusion",
        crate::camera::AmbientOcclusion::name,
    )? {
        l.ambient_occlusion = q;
    }
    if let Some(a) = opt_pick(
        &args,
        "anti_aliasing",
        &crate::camera::AntiAliasing::ALL,
        "anti-aliasing",
        crate::camera::AntiAliasing::name,
    )? {
        l.anti_aliasing = a;
    }
    match args.get("forest_contours") {
        None | Some(Value::Null) => {}
        Some(contours) if contours.is_object() => {
            if let Some(enabled) = opt_bool(contours, "enable")? {
                l.forest_contours.enabled = enabled;
            }
            if let Some(strength) = opt_num(contours, "strength")? {
                l.forest_contours.strength = strength;
            }
            if let Some(start_distance) = opt_num(contours, "start_distance")? {
                l.forest_contours.start_distance = start_distance;
            }
            l.forest_contours.clamp();
        }
        Some(_) => return Err("`forest_contours` must be an object".into()),
    }
    if let Some(q) = opt_pick(
        &args,
        "volumetric_light",
        &crate::volumetrics::VolumetricQuality::ALL,
        "volumetric light",
        crate::volumetrics::VolumetricQuality::name,
    )? {
        l.volumetrics = q;
    }
    if let Some(b) = opt_bool(&args, "stars")? {
        l.stars = b;
    }
    if let Some(v) = args.get("cloud_sea") {
        l.cloud_sea.apply_mcp(v)?;
    }
    if let Some(b) = opt_bool(&args, "bounce_light")? {
        l.bounce_light = b;
    }
    if let Some(m) = opt_pick(
        &args,
        "eye_adaptation",
        &crate::camera_fx::EyeAdaptation::ALL,
        "eye adaptation",
        crate::camera_fx::EyeAdaptation::name,
    )? {
        l.eye_adaptation = m;
    }
    if let Some(m) = opt_pick(
        &args,
        "depth_of_field",
        &crate::camera_fx::DepthOfFieldMode::ALL,
        "depth of field",
        crate::camera_fx::DepthOfFieldMode::name,
    )? {
        l.depth_of_field.mode = m;
    }
    if let Some(f) = opt_num(&args, "dof_focus")? {
        l.depth_of_field.focus = f.clamp(0.0, 20_000.0);
    }
    if let Some(f) = opt_num(&args, "dof_f_stops")? {
        l.depth_of_field.f_stops = f.clamp(0.5, 32.0);
    }
    meta.dirty = true;
    let h = l.hour;
    text(
        format!(
            "Time {:02}:{:02}{}, sun strength {:.2}, tilt {:.0}°, rising at {:.0}°, exposure {:+.1} EV, haze {} ({:.2}), moon phase {:.2}. Ambient occlusion {}, anti-aliasing {}, volumetric light {}. Render resolution {} (world_info shows the pixels).",
            h as u32,
            (h.fract() * 60.0) as u32,
            if l.playing {
                " (day cycle playing)"
            } else {
                ""
            },
            l.sun_strength,
            l.sun_tilt,
            compass(l.sun_azimuth + 90.0),
            l.exposure_comp,
            if l.haze_enabled { "on" } else { "off" },
            l.haze,
            l.moon_phase,
            l.ambient_occlusion.name(),
            l.anti_aliasing.name(),
            l.volumetrics.name(),
            rs.setting.label()
        ) + &camera_fx_line(l, &adapt)
            + " "
            + &bounce.describe(l.bounce_light)
            + " "
            + &l.cloud_sea.describe(),
    )
}

/// The eye adaptation and depth of field part of `lighting_set`'s reply.
fn camera_fx_line(l: &Lighting, adapt: &crate::camera_fx::Adaptation) -> String {
    let d = &l.depth_of_field;
    let dof = match d.mode {
        crate::camera_fx::DepthOfFieldMode::Off => "depth of field off".to_string(),
        m => format!(
            "depth of field {} (f/{:.1}, focus {})",
            m.name().to_lowercase(),
            d.f_stops,
            if d.focus > 0.0 {
                format!("{:.1} m", d.focus)
            } else {
                "automatic".into()
            }
        ),
    };
    format!(" Camera: {}; {}.", adapt.describe(l.eye_adaptation), dof)
}

fn look(
    In(args): In<Value>,
    mut l: ResMut<Lighting>,
    mut meta: ResMut<crate::world::WorldMeta>,
) -> R {
    if let Some(p) = opt_pick(&args, "preset", &LookPreset::ALL, "look", LookPreset::name)? {
        l.look.select(p);
        l.scifi_look = None;
        if opt_bool(&args, "apply_scene")?.unwrap_or(true)
            && let Some((hour, clouds, haze)) = p.scene()
        {
            l.hour = hour;
            clouds.apply(&mut l, fastrand::u64(..));
            l.haze = haze;
        }
    }
    if opt_bool(&args, "reset_params")?.unwrap_or(false) {
        l.look.custom = None;
        l.look.saved = None;
    }
    if let Some(s) = opt_num(&args, "strength")? {
        l.look.strength = s.clamp(0.0, 1.0);
    }
    if let Some(Value::Object(over)) = args.get("params") {
        let mut v = serde_json::to_value(l.look.base()).map_err(|e| e.to_string())?;
        let known = v
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let given = Value::Object(over.clone());
        for (k, val) in over {
            if !known.contains(k) {
                return Err(format!(
                    "unknown look parameter `{k}`; known: {}",
                    known.join(", ")
                ));
            }
            v[k] = if v[k].is_array() {
                json!(opt_color(&given, k)?)
            } else if k == "tone" {
                let s = val
                    .as_str()
                    .ok_or("`tone` is one of filmic, punchy, soft, classic")?;
                json!(format!(
                    "{:?}",
                    pick(&ToneCurve::ALL, s, "tone curve", ToneCurve::name)?
                ))
            } else {
                val.clone()
            };
        }
        let p: LookParams =
            serde_json::from_value(v).map_err(|e| format!("look parameters: {e}"))?;
        l.look.custom = Some(p);
    }
    meta.dirty = true;
    text(format!(
        "Look: {} ({}) at {:.0}%{}. Time {:02}:{:02}, cloud cover {:.0}%, haze {:.2}.",
        l.look.preset.name(),
        l.look.preset.inspired_by(),
        l.look.strength * 100.0,
        if l.look.custom.is_some() {
            ", fine-tuned"
        } else {
            ""
        },
        l.hour as u32,
        (l.hour.fract() * 60.0) as u32,
        l.cloud_coverage * 100.0,
        l.haze
    ))
}

#[allow(clippy::too_many_arguments)]
fn weather(
    In(args): In<Value>,
    mut lighting: ResMut<Lighting>,
    mut wind: ResMut<Wind>,
    mut motion: ResMut<crate::wind::Motion>,
    mut meta: ResMut<crate::world::WorldMeta>,
    mut rain: ResMut<crate::rain::RainState>,
    mut bolt: ResMut<crate::rain::lightning::Lightning>,
) -> R {
    let l = &mut *lighting;
    let mut sky = None;
    if let Some(p) = opt_pick(
        &args,
        "cloud_preset",
        &CloudPreset::ALL,
        "cloud preset",
        CloudPreset::name,
    )? {
        p.apply(l, fastrand::u64(..));
        sky = Some(p.name());
    }
    if opt_bool(&args, "randomize_sky")?.unwrap_or(false) {
        sky = Some(crate::clouds::randomize_clouds(l, fastrand::u64(..)).name());
    }
    for (k, f, lo, hi) in [
        ("cloud_cover", &mut l.cloud_coverage, 0.0, 1.0),
        ("cloud_type", &mut l.cloud_type, 0.0, 1.0),
        ("cloud_thickness", &mut l.cloud_density, 0.3, 2.0),
        ("cirrus", &mut l.cirrus, 0.0, 1.0),
        ("cloud_speed", &mut l.cloud_speed, 0.0, 5.0),
        ("cloud_shadows", &mut l.cloud_shadows, 0.0, 1.0),
    ] {
        if let Some(v) = opt_num(&args, k)? {
            *f = v.clamp(lo, hi);
        }
    }
    if let Some(s) = opt_num(&args, "cloud_seed")? {
        l.cloud_seed = s.max(0.0) as u32;
    }
    if let Some(q) = opt_pick(
        &args,
        "cloud_quality",
        &CloudQuality::ALL,
        "cloud quality",
        CloudQuality::name,
    )? {
        l.cloud_quality = q;
    }
    if let Some(v) = args.get("cloud_sea") {
        l.cloud_sea.apply_mcp(v)?;
    }
    if let Some(w) = opt_str(&args, "weather")? {
        l.weather_override = if w == "follow_zones" {
            None
        } else {
            Some(pick(&Weather::ALL, w, "weather", Weather::name)?)
        };
    }
    if let Some(b) = opt_bool(&args, "rain_and_snow")? {
        l.precipitation = b;
    }
    if let Some(s) = opt_num(&args, "wind_speed")? {
        wind.speed = s.clamp(0.0, 25.0);
    }
    if let Some(a) = opt_num(&args, "wind_direction_deg")? {
        wind.dir = bearing_dir(a);
    }
    if let Some(g) = opt_num(&args, "gustiness")? {
        wind.gustiness = g.clamp(0.0, 1.0);
    }
    if let Some(b) = opt_bool(&args, "preview_motion")? {
        motion.preview = b;
    }
    if let Some(v) = opt_num(&args, "wetness")? {
        rain.hold = (v >= 0.0).then(|| v.min(1.0));
    }
    if let Some(v) = opt_num(&args, "puddles")? {
        l.rain.puddles = v.clamp(0.0, 1.0);
    }
    if let Some(v) = opt_num(&args, "lightning")? {
        l.rain.lightning = v.clamp(0.0, 3.0);
    }
    if let Some(b) = opt_bool(&args, "thunder")? {
        l.rain.thunder = b;
    }
    if opt_bool(&args, "lightning_now")?.unwrap_or(false) {
        bolt.now = true;
    }
    meta.dirty = true;
    text(
        format!(
            "Sky{}: cover {:.0}%, type {:.2}, thickness {:.2}, cirrus {:.2}. Weather: {}{}. Wind {:.1} m/s ({}) towards {:.0}°, gustiness {:.2}. Motion while editing: {}.",
            sky.map(|s| format!(" {s}")).unwrap_or_default(),
            l.cloud_coverage * 100.0,
            l.cloud_type,
            l.cloud_density,
            l.cirrus,
            l.weather_override
                .map(|w| w.name())
                .unwrap_or("following zones and biomes"),
            if l.precipitation {
                ""
            } else {
                " (no rain or snow)"
            },
            wind.speed,
            crate::wind::describe(wind.speed),
            dir_bearing(wind.dir),
            wind.gustiness,
            if motion.preview {
                "on"
            } else {
                "off (still until walk mode)"
            }
        ) + &format!(
            " Wet: {:.0}% ({}), puddles {:.0}% full (amount {:.0}%), rain {:.0}%. Lightning {:.1}× (thunder {}), {} strikes so far{}.",
            rain.hold.unwrap_or(rain.surface) * 100.0,
            if rain.hold.is_some() {
                "held"
            } else {
                "following the weather"
            },
            rain.hold.unwrap_or(rain.puddles) * 100.0,
            l.rain.puddles * 100.0,
            rain.rain * 100.0,
            l.rain.lightning,
            if l.rain.thunder { "on" } else { "off" },
            bolt.strikes,
            if bolt.now { "; striking now" } else { "" }
        ) + " "
            + &l.cloud_sea.describe(),
    )
}

fn camera_text(o: &OrbitCamera) -> String {
    let (f, e) = (o.focus, o.eye());
    format!(
        "Camera: focus [{:.0}, {:.0}] (ground {:.0} m), yaw {:.0}°, pitch {:.0}°, distance {:.0} m; eye at [{:.0}, {:.0}, {:.0}].",
        f.x,
        f.z,
        f.y,
        view_bearing(o.yaw),
        o.pitch.to_degrees(),
        o.distance,
        e.x,
        e.y,
        e.z
    )
}

/// Applies camera_set arguments.
fn camera(
    In(args): In<Value>,
    mut orbit: ResMut<OrbitCamera>,
    t: Res<Terrain>,
    caves: Res<crate::caves::render::CaveWorld>,
) -> R {
    let ground = |p: Vec2| t.height_at(p.x, p.y).max(t.sea_level);
    let (mut focus, mut yaw, mut pitch, mut dist) =
        (orbit.focus, orbit.yaw, orbit.pitch, orbit.distance);
    match opt_str(&args, "preset")? {
        Some("overview") => {
            (focus, yaw, pitch, dist) = (
                Vec3::new(0.0, ground(Vec2::ZERO), t.size() * 0.08),
                0.35,
                0.62,
                t.size() * 0.8,
            );
        }
        Some("top") => {
            (focus, yaw, pitch, dist) = (
                Vec3::new(0.0, ground(Vec2::ZERO), 0.0),
                0.0,
                1.52,
                t.size() * 1.05,
            );
        }
        Some(p) => return Err(format!("unknown camera preset `{p}` (overview or top)")),
        None => {}
    }
    if let Some(eye) = args.get("eye") {
        let e: Vec<f32> = eye
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_f64())
                    .map(|v| v as f32)
                    .collect()
            })
            .unwrap_or_default();
        if e.len() != 3 {
            return Err("`eye` must be [x, y, z]".into());
        }
        let tv = args.get("target").ok_or("`eye` needs a `target`")?;
        let tf: Vec<f32> = tv
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_f64())
                    .map(|v| v as f32)
                    .collect()
            })
            .unwrap_or_default();
        let target = match tf[..] {
            [x, z] => Vec3::new(x, ground(Vec2::new(x, z)), z),
            [x, y, z] => Vec3::new(x, y, z),
            _ => return Err("`target` must be [x, z] or [x, y, z]".into()),
        };
        // the camera orbits a focus on the ground: aim through the target down to the ground
        // (or to a cave's floor when the target is in a cave)
        let eye = Vec3::new(e[0], e[1], e[2]);
        let cave_fy = caves.floor_under(target + Vec3::Y * 1.2, 6.0);
        let fy = cave_fy.unwrap_or_else(|| ground(target.xz()));
        // (in a cave it turns round a point at eye height over the floor)
        let lift = if cave_fy.is_some() {
            crate::camera::CAVE_LIFT
        } else {
            0.0
        };
        let off = eye - Vec3::new(target.x, fy + lift, target.z);
        dist = off.length().max(3.0);
        pitch = (off.y / dist).clamp(-1.0, 1.0).asin();
        yaw = off.x.atan2(off.z);
        focus = Vec3::new(target.x, fy, target.z);
    }
    if let Some(f) = opt_xz(&args, "focus")? {
        focus = Vec3::new(f.x, ground(f), f.y);
    }
    if let Some(y) = opt_num(&args, "yaw_deg")? {
        yaw = view_yaw(y);
    }
    if let Some(p) = opt_num(&args, "pitch_deg")? {
        pitch = p.to_radians();
    }
    if let Some(d) = opt_num(&args, "distance")? {
        dist = d;
    }
    let lim = t.half() * 1.3;
    focus.x = focus.x.clamp(-lim, lim);
    focus.z = focus.z.clamp(-lim, lim);
    // (in a cave the camera may look up: at a roof, out of a mouth)
    let in_cave = caves.in_space(focus + Vec3::Y * 1.2)
        || args
            .get("eye")
            .is_some_and(|_| caves.in_space(focus + OrbitCamera::offset_of(yaw, pitch, dist)));
    let low = if in_cave { -1.3 } else { 0.03 };
    orbit.set_view(focus, yaw, pitch.clamp(low, 1.52), dist.clamp(3.0, 60000.0));
    if !opt_bool(&args, "smooth")?.unwrap_or(false) {
        orbit.snap();
    }
    text(camera_text(&orbit))
}

/// What a click at `at` (0..1 across the view) would hit (`camera::pick`).
fn camera_pick(
    In(args): In<Value>,
    cam: Query<(&Camera, &GlobalTransform), With<crate::camera::MainCamera>>,
    t: Res<Terrain>,
    caves: Res<crate::caves::render::CaveWorld>,
    holes: Res<crate::terrain::holes::TerrainHoles>,
) -> R {
    let at: Vec<f32> = match args.get("at") {
        Some(v) if !v.is_null() => v
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_f64())
                    .map(|x| x as f32)
                    .collect()
            })
            .unwrap_or_default(),
        _ => vec![0.5, 0.5],
    };
    let [u, v] = at[..] else {
        return Err("`at` must be [x, y] (0..1 across the view)".into());
    };
    let (c, gt) = cam.single().map_err(|_| "no camera".to_string())?;
    let size = c
        .logical_viewport_size()
        .ok_or("the view has no size yet")?;
    let ray = c
        .viewport_to_world(gt, Vec2::new(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)) * size)
        .map_err(|e| format!("{e}"))?;
    let r2 = |x: f32| (x * 100.0).round() / 100.0;
    match crate::camera::pick(&t, Some(&caves), Some(&holes), ray) {
        None => json_out(json!({ "hit": null, "note": "nothing there (sky)" })),
        Some((p, n)) => {
            let what = match n {
                None => "ground",
                Some(n) if n.y >= 0.5 => "cave floor",
                Some(n) if n.y <= -0.5 => "cave roof",
                Some(_) => "cave wall",
            };
            json_out(
                json!({ "hit": [r2(p.x), r2(p.y), r2(p.z)], "surface": what, "normal": n.map(|n| [r2(n.x), r2(n.y), r2(n.z)]), "distance": r2(p.distance(ray.origin)), "camera_in_cave": caves.in_space(ray.origin) }),
            )
        }
    }
}

fn screenshot(world: &mut World, args: Value) -> R {
    let moved = match args.get("camera") {
        Some(c) if !c.is_null() => {
            run(world, camera, c.clone())?;
            true
        }
        _ => false,
    };
    let hide = opt_bool(&args, "hide_ui")?.unwrap_or(true);
    let max = num(&args, "max_size", 1024.0)?.clamp(64.0, 4096.0) as u32;
    let settle =
        num(&args, "settle_frames", if moved { 30.0 } else { 4.0 })?.clamp(1.0, 600.0) as u32;
    let save = opt_str(&args, "save_path")?.map(std::path::PathBuf::from);
    if let Some(p) = &save
        && (!p.is_absolute() || p.extension().is_none_or(|e| e != "png"))
    {
        return Err("`save_path` must be an absolute path ending in .png".into());
    }
    if hide {
        world.resource_mut::<McpServer>().hide_ui += 1;
    }
    // (held clear by the area prompt's capture hold, which needs the UI hidden)
    let clear = opt_bool(&args, "clear_sky")?.unwrap_or(false) && hide;
    if clear {
        world.resource_mut::<crate::prompt::tools::ShotSky>().0 += 1;
    }
    let slot: Arc<Mutex<Option<Image>>> = Arc::default();
    let mut frame = 0u32;
    let mut asked = false;
    let restore = move |w: &mut World| {
        if hide {
            let mut s = w.resource_mut::<McpServer>();
            s.hide_ui = s.hide_ui.saturating_sub(1);
        }
        if clear {
            let mut sky = w.resource_mut::<crate::prompt::tools::ShotSky>();
            sky.0 = sky.0.saturating_sub(1);
        }
    };
    Ok(vec![McpContent::deferred(move |w| {
        frame += 1;
        if !asked {
            if frame >= settle {
                let slot = slot.clone();
                w.spawn(Screenshot::primary_window())
                    .observe(move |ev: On<ScreenshotCaptured>| {
                        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(ev.image.clone());
                    });
                asked = true;
            }
            return None;
        }
        let Some(img) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            if frame > settle + 600 {
                restore(w);
                return Some(Err("The screenshot never arrived: the editor window may be minimised or hidden (it stops rendering then).".into()));
            }
            return None;
        };
        restore(w);
        let full = match img.try_into_dynamic() {
            Ok(d) => d.into_rgb8(),
            Err(e) => return Some(Err(format!("could not read the screenshot: {e:?}"))),
        };
        let (fw, fh) = full.dimensions();
        if let Some(p) = &save
            && let Err(e) = full.save(p)
        {
            return Some(Err(format!("could not save {}: {e}", p.display())));
        }
        let k = (max as f32 / fw.max(fh) as f32).min(1.0);
        let small = if k < 1.0 {
            image::imageops::resize(
                &full,
                ((fw as f32 * k).round() as u32).max(1),
                ((fh as f32 * k).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            )
        } else {
            full
        };
        let mut png = std::io::Cursor::new(Vec::new());
        if let Err(e) = small.write_to(&mut png, image::ImageFormat::Png) {
            return Some(Err(format!("could not encode the screenshot: {e}")));
        }
        let note = format!(
            "Screenshot {}×{} (window {fw}×{fh}){}. {}{}",
            small.width(),
            small.height(),
            if hide { ", editor UI hidden" } else { "" },
            camera_text(w.resource::<OrbitCamera>()),
            save.as_ref()
                .map(|p| format!(" Saved full size to {}.", p.display()))
                .unwrap_or_default()
        );
        Some(Ok(vec![
            McpContent::Png(png.into_inner()),
            McpContent::Text(note),
        ]))
    })])
}
