//! Light fixtures of library models: the `light` effect anchors (`fx_light_<n>` glTF nodes, listed
//! in a library's `models.json` `fx` as `{"kind": "light", "pos": [x, y, z], "radius": range_m,
//! "color": "#ffd9a8", "lumens": 800, "spot": {"dir": [x, y, z], "angle_deg": 70}, "shadows":
//! true, "night_only": false}`) shine as real Bevy point or spot lights: downlights, cove strips,
//! lanterns, path and street lights. They follow the placed objects (moved, turned and sized with
//! them: reach and lumens grow with the size) and reload with the manifests (`fx_anchors`).
//!
//! Budgeted like the fires' lights: the nearest [`MAX_LIGHTS`] within [`LIGHT_RANGE`] of the eye
//! shine, and of those the nearest few that want them (`shadows`, default on unless the light is
//! `night_only`) cast shadows (`dungeons::lights::shadow_budget`, at most [`MAX_SHADOWS`]).
//! `night_only` lights (street and path lights) come on at dusk and are off by day.
//!
//! Strength: `lumens` are the fixture's real output. The editor's exposure is set for daylight
//! (about EV 14.6 by day, 10.4 at night, ~12.7 indoors by day) where a real room is seen at EV
//! 6–7, so a fixture shines [`LUMEN_SCALE`] times its lumens at night (on the fires' scale: a
//! candle's 12 lm burn as 2 500, a torch's ~150 as 25 000; an 800 lm downlight casts a clear pool)
//! and a little more as the eye's exposure closes by day
//! ([`DAY_ADAPT`] of the stops), so rooms stay lit by day while exterior accents read subtle in
//! sunlight.
//!
//! MCP: `lights` lists them; exports carry them (`lights_json`).

use std::collections::HashMap;

use bevy::prelude::*;
use serde_json::{Value, json};

use crate::fx_anchors::{FxAnchor, FxAnchors, world_anchor};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg, position_arg, position_schema};
use crate::objects::{ObjectStore, Placed};

/// How far (m) from the eye fixtures shine, how many at once, and at most how many cast shadows.
pub const LIGHT_RANGE: f32 = 70.0;
pub const MAX_LIGHTS: usize = 24;
pub const MAX_SHADOWS: usize = 4;
/// Engine intensity per real lumen at night (the night exposure, EV 10.4).
pub const LUMEN_SCALE: f32 = 120.0;
/// The night exposure the scale is set for, and the share of the stops above it that the
/// fixtures follow (the eye adapting: 0 = fixed, 1 = as bright as at night whatever the exposure).
const NIGHT_EV: f32 = 10.4;
const DAY_ADAPT: f32 = 0.6;
/// Defaults: reach when an anchor gives none, lumens, colour (a warm 3000 K white).
const DEFAULT_RANGE: f32 = 8.0;
const DEFAULT_LUMENS: f32 = 800.0;
const DEFAULT_COLOR: [f32; 3] = [1.0, 0.85, 0.66];

/// One `light` anchor's settings (model space).
#[derive(Clone, Debug, PartialEq)]
pub struct LightSpec {
    pub pos: Vec3,
    /// metres
    pub range: f32,
    /// sRGB 0..1
    pub color: [f32; 3],
    pub lumens: f32,
    /// a spot: its direction (model space, unit) and full cone angle (degrees)
    pub spot: Option<(Vec3, f32)>,
    pub shadows: bool,
    pub night_only: bool,
}

/// `#rrggbb` (or `rrggbb`) → sRGB 0..1.
pub fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some([c(0)?, c(2)?, c(4)?])
}

impl LightSpec {
    /// The light of a `light` anchor (None for other kinds).
    pub fn of(a: &FxAnchor) -> Option<LightSpec> {
        if a.kind != "light" {
            return None;
        }
        let x = &a.extras;
        let num = |v: Option<&Value>| v.and_then(|v| v.as_f64()).map(|v| v as f32).filter(|v| v.is_finite());
        let night_only = x.get("night_only").and_then(|v| v.as_bool()).unwrap_or(false);
        let spot = x.get("spot").and_then(|s| {
            let d = s.get("dir")?.as_array()?;
            let f = |i: usize| d.get(i).and_then(|v| v.as_f64()).map(|v| v as f32);
            let dir = Vec3::new(f(0)?, f(1)?, f(2)?).try_normalize()?;
            let angle = num(s.get("angle_deg")).unwrap_or(70.0).clamp(5.0, 175.0);
            Some((dir, angle))
        });
        Some(LightSpec {
            pos: a.pos,
            range: if a.radius > 0.0 { a.radius } else { DEFAULT_RANGE },
            color: x.get("color").and_then(|v| v.as_str()).and_then(parse_hex).unwrap_or(DEFAULT_COLOR),
            lumens: num(x.get("lumens")).unwrap_or(DEFAULT_LUMENS).max(0.0),
            spot,
            shadows: x.get("shadows").and_then(|v| v.as_bool()).unwrap_or(!night_only),
            night_only,
        })
    }
}

/// A fixture light placed in the world.
#[derive(Clone, Debug, PartialEq)]
pub struct FixtureLight {
    /// (object id, anchor index in the model's `fx`)
    pub key: (u64, u16),
    pub pos: Vec3,
    /// a spot's world direction and full cone angle (degrees)
    pub spot: Option<(Vec3, f32)>,
    pub range: f32,
    /// real lumens (grown with the object's size)
    pub lumens: f32,
    pub color: [f32; 3],
    pub shadows: bool,
    pub night_only: bool,
}

/// Where `l` (anchor `k` of `a`) shines on the placed object `p`.
pub fn place(p: &Placed, k: usize, a: &FxAnchor, l: &LightSpec) -> FixtureLight {
    let w = world_anchor(p, a);
    let s = p.scale3();
    let grow = s.max_element().max(1e-3);
    FixtureLight {
        key: (p.id, k as u16),
        pos: w.pos,
        spot: l.spot.map(|(d, ang)| ((p.rotation() * (d * s)).normalize_or(Vec3::NEG_Y), ang)),
        range: l.range * grow,
        lumens: l.lumens * grow * grow,
        color: l.color,
        shadows: l.shadows,
        night_only: l.night_only,
    }
}

/// Every fixture light of `objects`, by key.
pub fn lights_for<'a>(objects: impl Iterator<Item = &'a Placed>, anchors: &FxAnchors) -> Vec<FixtureLight> {
    let mut out = vec![];
    for p in objects {
        for (k, a) in anchors.of(&p.asset).iter().enumerate() {
            if let Some(l) = LightSpec::of(a) {
                out.push(place(p, k, a, &l));
            }
        }
    }
    out.sort_by_key(|l| l.key);
    out
}

/// How strongly a light shines now (0..1 of its full strength): `night_only` ones fade in at dusk.
pub fn on_share(night_only: bool, night: f32, underground: bool) -> f32 {
    if !night_only || underground {
        1.0
    } else {
        let t = ((night - 0.3) / 0.4).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

/// Engine intensity (Bevy lumens) for `lumens` real lumens at the eye's exposure `ev`.
pub fn engine_lumens(lumens: f32, ev: f32) -> f32 {
    lumens * LUMEN_SCALE * 2f32.powf(DAY_ADAPT * (ev - NIGHT_EV).clamp(-2.0, 5.0))
}

/// Which of `lights` shine for an eye at `eye` (the nearest [`MAX_LIGHTS`] within reach that are
/// on now) and which of those cast shadows (the nearest `shadow_budget` that want them).
pub fn pick(lights: &[FixtureLight], eye: Vec3, share: impl Fn(&FixtureLight) -> f32, shadow_budget: usize) -> (Vec<usize>, Vec<usize>) {
    let mut near: Vec<(f32, usize)> = lights
        .iter()
        .enumerate()
        .filter(|(_, l)| share(l) > 0.0 && l.lumens > 0.0)
        .map(|(i, l)| ((l.pos.distance(eye) - l.range).max(0.0), i))
        .filter(|(d, _)| *d < LIGHT_RANGE)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let on: Vec<usize> = near.iter().take(MAX_LIGHTS).map(|x| x.1).collect();
    let shadowed: Vec<usize> = on
        .iter()
        .copied()
        .filter(|&i| lights[i].shadows)
        .take(shadow_budget.min(MAX_SHADOWS))
        .collect();
    (on, shadowed)
}

#[derive(Component)]
struct LitFixture {
    key: (u64, u16),
}

#[derive(Resource, Default)]
pub struct FixtureLights {
    seen: Option<(u64, u64)>,
    rescan: f32,
    eval: f32,
    pub lights: Vec<FixtureLight>,
    /// the shining ones' entities (and whether each is a spot, casts shadows)
    on: HashMap<(u64, u16), (Entity, bool, bool)>,
}

pub struct FixtureLightsPlugin;

impl Plugin for FixtureLightsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FixtureLights>()
            .add_systems(Update, sync.after(crate::sky::SkySystems));
        app.mcp_tool(McpTool {
            name: "lights",
            description: "Light fixtures of placed models: their models' `light` effect anchors (downlights, cove strips, lanterns, path and street lights; models.json `fx` entries {kind: \"light\", pos, radius: reach m, color: \"#rrggbb\", lumens, spot: {dir, angle_deg} (a spot light, else a point light), shadows (default on unless night_only), night_only (on only at dusk and night)}) shining as real lights that follow the objects (sized with them). The nearest 24 within 70 m of the eye shine; the nearest few that want them cast shadows. Lists the fixture lights near `position` ([x, z]) within `radius` (m, default 120), nearest first (at most `limit`, default 50): object id, anchor index, position, point or spot (direction, angle), reach, lumens, colour, shadows, night_only, and whether it shines now (`on`, `shadowed`, `strength` 0..1 by the time of day). Read-only; the lights are part of the models.",
            input_schema: || {
                json!({ "type": "object", "properties": {
                    "position": position_schema("centre of the search"),
                    "radius": { "type": "number", "description": "metres (default 120)" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 } } })
            },
            run: lights_tool,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn sync(
    mut commands: Commands,
    mut fl: ResMut<FixtureLights>,
    store: Res<ObjectStore>,
    anchors: Res<FxAnchors>,
    sky: Res<crate::sky::SkyState>,
    dungeon: Res<crate::dungeons::model::DungeonMap>,
    graphics: Res<crate::streaming::Graphics>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    time: Res<Time>,
    mut points: Query<(&LitFixture, &mut PointLight), Without<SpotLight>>,
    mut spots: Query<(&LitFixture, &mut SpotLight), Without<PointLight>>,
) {
    let fl = &mut *fl;
    let dt = time.delta_secs().min(0.1);
    // (re-placed when the objects or the manifests change: at most twice a second while objects
    // are being painted or dragged, at once when the anchors reloaded)
    let key = (store.revision, anchors.revision);
    fl.rescan -= dt;
    if fl.seen != Some(key) && (fl.rescan <= 0.0 || fl.seen.is_none_or(|s| s.1 != key.1)) {
        fl.seen = Some(key);
        fl.rescan = 0.5;
        fl.lights = lights_for(store.items.values(), &anchors);
        fl.eval = -1.0;
    }
    let underground = dungeon.interior();
    let night = sky.night;
    let Ok(cam) = cam.single() else { return };
    fl.eval -= dt;
    if fl.eval <= 0.0 {
        fl.eval = 0.25;
        let budget = crate::dungeons::lights::shadow_budget(graphics.detail);
        let (on, shadowed) = pick(&fl.lights, cam.translation(), |l| on_share(l.night_only, night, underground), budget);
        let want: HashMap<(u64, u16), (usize, bool)> = on.iter().map(|&i| (fl.lights[i].key, (i, shadowed.contains(&i)))).collect();
        let gone: Vec<_> = fl
            .on
            .iter()
            .filter(|(k, (_, spot, sh))| {
                want.get(*k).is_none_or(|&(i, s)| fl.lights[i].spot.is_some() != *spot || s != *sh)
            })
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            if let Some((e, ..)) = fl.on.remove(&k) {
                commands.entity(e).despawn();
            }
        }
        for (k, (i, sh)) in want {
            if fl.on.contains_key(&k) {
                continue;
            }
            let l = &fl.lights[i];
            let color = Color::srgb(l.color[0], l.color[1], l.color[2]);
            let name = Name::new(format!("Fixture light {}:{}", k.0, k.1));
            let e = match l.spot {
                Some((dir, angle)) => {
                    let up = if dir.y.abs() > 0.98 { Vec3::Z } else { Vec3::Y };
                    let half = (angle * 0.5).to_radians();
                    commands
                        .spawn((
                            name,
                            LitFixture { key: k },
                            SpotLight {
                                color,
                                intensity: 0.0,
                                range: l.range,
                                radius: 0.04,
                                shadow_maps_enabled: sh,
                                outer_angle: half,
                                inner_angle: half * 0.7,
                                ..default()
                            },
                            Transform::from_translation(l.pos).looking_to(dir, up),
                        ))
                        .id()
                }
                None => commands
                    .spawn((
                        name,
                        LitFixture { key: k },
                        PointLight {
                            color,
                            intensity: 0.0,
                            range: l.range,
                            radius: 0.05,
                            shadow_maps_enabled: sh,
                            ..default()
                        },
                        Transform::from_translation(l.pos),
                    ))
                    .id(),
            };
            fl.on.insert(k, (e, l.spot.is_some(), sh));
        }
    }
    // every frame: strength by the time of day and the eye's exposure; place (an object moved)
    let by_key: HashMap<(u64, u16), &FixtureLight> = fl.lights.iter().map(|l| (l.key, l)).collect();
    let ev = sky.scene_ev;
    for (lf, mut pl) in &mut points {
        if let Some(l) = by_key.get(&lf.key) {
            pl.intensity = engine_lumens(l.lumens, ev) * on_share(l.night_only, night, underground);
            pl.range = l.range;
        }
    }
    for (lf, mut sl) in &mut spots {
        if let Some(l) = by_key.get(&lf.key) {
            sl.intensity = engine_lumens(l.lumens, ev) * on_share(l.night_only, night, underground);
            sl.range = l.range;
        }
    }
    for (k, (e, ..)) in &fl.on {
        if let Some(l) = by_key.get(k) {
            let tf = match l.spot {
                Some((dir, _)) => Transform::from_translation(l.pos).looking_to(dir, if dir.y.abs() > 0.98 { Vec3::Z } else { Vec3::Y }),
                None => Transform::from_translation(l.pos),
            };
            commands.entity(*e).insert(tf);
        }
    }
}

fn r2(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn hex(c: [f32; 3]) -> String {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", q(c[0]), q(c[1]), q(c[2]))
}

fn lights_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let centre = position_arg(&args, "position")?.map(|(p, _)| p);
    let radius = opt_arg::<f32>(&args, "radius")?.unwrap_or(120.0).max(0.1);
    let limit = opt_arg::<usize>(&args, "limit")?.unwrap_or(50).clamp(1, 500);
    let night = world.resource::<crate::sky::SkyState>().night;
    let underground = world.resource::<crate::dungeons::model::DungeonMap>().interior();
    let fl = world.resource::<FixtureLights>();
    let mut rows: Vec<(f32, Value)> = fl
        .lights
        .iter()
        .filter_map(|l| {
            let d = centre.map(|c| c.distance(Vec2::new(l.pos.x, l.pos.z))).unwrap_or(0.0);
            if centre.is_some() && d > radius {
                return None;
            }
            let on = fl.on.get(&l.key);
            Some((
                d,
                json!({
                    "object": l.key.0,
                    "anchor": l.key.1,
                    "position": [r2(l.pos.x), r2(l.pos.y), r2(l.pos.z)],
                    "type": if l.spot.is_some() { "spot" } else { "point" },
                    "direction": l.spot.map(|(d, _)| json!([r2(d.x), r2(d.y), r2(d.z)])),
                    "angle_deg": l.spot.map(|(_, a)| r2(a)),
                    "radius": r2(l.range),
                    "lumens": r2(l.lumens),
                    "color": hex(l.color),
                    "shadows": l.shadows,
                    "night_only": l.night_only,
                    "strength": r2(on_share(l.night_only, night, underground)),
                    "on": on.is_some(),
                    "shadowed": on.is_some_and(|o| o.2),
                }),
            ))
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total = rows.len();
    Ok(vec![McpContent::Json(json!({
        "lights": rows.into_iter().take(limit).map(|r| r.1).collect::<Vec<_>>(),
        "lights_total": total,
        "on_now": fl.on.len(),
        "budget": { "lights": MAX_LIGHTS, "range_m": LIGHT_RANGE, "shadows_max": MAX_SHADOWS },
    }))])
}

/// The fixture lights of `objects` for an export (as the editor places them).
pub fn export_lights(world: &World, objects: &[Placed]) -> Vec<FixtureLight> {
    world
        .get_resource::<FxAnchors>()
        .map(|a| lights_for(objects.iter(), a))
        .unwrap_or_default()
}

/// The fixture lights for the engines: position, point or spot (direction, full cone angle),
/// reach, real lumens (and the editor's night-time engine lumens), colour (sRGB and linear), shadows, night_only, and `on`: the share of its
/// strength it shines with at the exported hour (`night` 0 day … 1 night).
pub fn lights_json(lights: &[FixtureLight], night: f32) -> Vec<Value> {
    lights
        .iter()
        .map(|l| {
            let c = Color::srgb_from_array(l.color).to_linear();
            json!({
                "position": l.pos.to_array(),
                "spot": l.spot.is_some(),
                "direction": l.spot.map(|s| s.0).unwrap_or(Vec3::NEG_Y).to_array(),
                "angle_deg": l.spot.map(|s| s.1).unwrap_or(0.0),
                "range": l.range,
                "lumens": l.lumens,
                // (the editor's night-time intensity: on the fire lights' scale, `flames::fires_json`)
                "engine_lumens": l.lumens * LUMEN_SCALE,
                "color": l.color,
                "color_linear": [c.red, c.green, c.blue],
                "shadows": l.shadows,
                "night_only": l.night_only,
                "on": on_share(l.night_only, night, false),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(pos: [f32; 3], yaw_deg: f32, scale: f32) -> Placed {
        serde_json::from_value(json!({ "id": 7, "asset": "photoreal:x", "variant": 0, "pos": pos, "rot": Quat::from_rotation_y(yaw_deg.to_radians()).to_array(), "scale": scale })).unwrap()
    }

    #[test]
    fn reads_light_anchors() {
        let doc = json!({ "models": [{ "id": "x", "fx": [
            { "kind": "light", "pos": [0.0, 2.6, 0.0], "radius": 6.0, "color": "#ffd9a8", "lumens": 800, "spot": { "dir": [0, -1, 0], "angle_deg": 80 } },
            { "kind": "light", "pos": [1.0, 3.0, 0.0], "night_only": true },
            { "kind": "fire", "pos": [0.0, 1.0, 0.0] },
        ] }] });
        let m = crate::fx_anchors::manifest_anchors(&doc);
        let specs: Vec<LightSpec> = m[0].1.iter().filter_map(LightSpec::of).collect();
        assert_eq!(specs.len(), 2);
        let a = &specs[0];
        assert_eq!((a.range, a.lumens, a.shadows, a.night_only), (6.0, 800.0, true, false));
        assert!((a.color[0] - 1.0).abs() < 1e-3 && (a.color[1] - 217.0 / 255.0).abs() < 1e-3);
        assert_eq!(a.spot, Some((Vec3::NEG_Y, 80.0)));
        // defaults; a night-only light casts no shadows unless asked
        let b = &specs[1];
        assert_eq!((b.range, b.lumens, b.shadows, b.spot), (DEFAULT_RANGE, DEFAULT_LUMENS, false, None));
        assert!(parse_hex("#12").is_none() && parse_hex("00ff00") == Some([0.0, 1.0, 0.0]));
    }

    #[test]
    fn lights_follow_the_object_and_its_size() {
        let a = FxAnchor::parse(&json!({ "kind": "light", "pos": [1.0, 2.0, 0.0], "radius": 5.0, "lumens": 100, "spot": { "dir": [1, 0, 0], "angle_deg": 60 } })).unwrap();
        let s = LightSpec::of(&a).unwrap();
        let l = place(&placed([10.0, 0.0, 0.0], 90.0, 2.0), 3, &a, &s);
        assert_eq!(l.key, (7, 3));
        assert!(l.pos.distance(Vec3::new(10.0, 4.0, -2.0)) < 1e-4, "{:?}", l.pos);
        // model +x turns to world −z; twice the size: twice the reach, four times the lumens
        assert!(l.spot.unwrap().0.distance(Vec3::NEG_Z) < 1e-4);
        assert_eq!((l.range, l.lumens), (10.0, 400.0));
    }

    #[test]
    fn the_budget_keeps_the_nearest_and_their_shadows() {
        let mk = |i: u64, x: f32, shadows: bool, night_only: bool| FixtureLight {
            key: (i, 0),
            pos: Vec3::new(x, 0.0, 0.0),
            spot: None,
            range: 1.0,
            lumens: 800.0,
            color: DEFAULT_COLOR,
            shadows,
            night_only,
        };
        let mut lights: Vec<FixtureLight> = (0..40).map(|i| mk(i, i as f32 * 2.0, i % 2 == 0, false)).collect();
        lights.push(mk(99, 0.5, true, true));
        // by day the street light is off
        let (on, sh) = pick(&lights, Vec3::ZERO, |l| on_share(l.night_only, 0.0, false), 3);
        assert_eq!(on.len(), MAX_LIGHTS);
        assert_eq!(on[0], 0);
        assert!(!on.contains(&40));
        assert_eq!(sh, vec![0, 2, 4]);
        // at night it shines, nearest after the first
        let (on, _) = pick(&lights, Vec3::ZERO, |l| on_share(l.night_only, 1.0, false), 3);
        assert!(on.contains(&40));
        // nothing past the range
        let (on, _) = pick(&lights, Vec3::new(1000.0, 0.0, 0.0), |_| 1.0, 3);
        assert!(on.is_empty());
    }

    #[test]
    fn fixtures_are_brighter_as_the_eye_closes_by_day() {
        let night = engine_lumens(800.0, NIGHT_EV);
        assert!((night - 800.0 * LUMEN_SCALE).abs() < 1.0);
        let day = engine_lumens(800.0, 14.6);
        assert!(day > night * 4.0 && day < night * 8.0);
        assert_eq!(on_share(true, 0.0, false), 0.0);
        assert_eq!(on_share(true, 1.0, false), 1.0);
        assert_eq!(on_share(true, 0.0, true), 1.0);
        assert_eq!(on_share(false, 0.0, false), 1.0);
    }
}
