//! Chest lids of library and imported glTF models. A glTF node whose extras hold
//! `{"lid": {"axis": [x, y, z], "open_deg": d, "leaf": [x, y, z], "size": [x, y, z]}}` (the style
//! packs' `Asset.lid_node`) turns about `axis` (node space, through the node's origin: the hinge
//! along the chest's back) up to `d` degrees; `leaf` is the lid's centre from the hinge, `size` its
//! extent. A child node with `{"flap": {"axis": [...], "lag_deg": l}}` (a drape's or pennant's
//! hanging end) turns back against its lid once the lid has opened past `l` degrees, so it keeps
//! hanging down.
//!
//! While walking, the chest the explorer looks at within reach shows a hint by the crosshair;
//! the interact key (E) opens it (the lid creaks back, a sound at the chest) and shuts it again.
//! Agents list, open and shut chests with the MCP tools `chests` and `interact`; `walk_state`
//! reports the chest in reach. Lids are never part of a model's walk-in collider. The same models
//! carry a glTF animation "lid_open" (shut to open, 1 s) for other engines.

use bevy::audio::{SpatialListener, SpatialScale, Volume};
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{self, Align2, Color32, CornerRadius, FontId, Id, Order, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::audio::AudioSettings;
use crate::camera::MainCamera;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg, position_arg, position_schema};
use crate::walk::WalkState;

/// The explorer's eye within this of a lid's edge (metres) can reach it...
const REACH: f32 = 1.8;
/// ...looking within this of its centre (radians; wider for a big lid close by).
const AIM: f32 = 0.4;
/// Seconds to swing fully open / to fall shut.
const OPEN_S: f32 = 0.95;
const CLOSE_S: f32 = 0.5;
/// The first-time tip (dismissed once a chest has been opened).
const TIP: &str = "chest_lids";

/// A chest's hinged lid: its shut rotation, hinge axis (local), how far it opens, where it is.
#[derive(Component, Debug, Clone)]
pub struct Lid {
    pub axis: Vec3,
    /// Fully open (radians).
    pub open: f32,
    /// The lid's centre from the hinge (node space).
    pub leaf: Vec3,
    /// Half the lid's diagonal (node space): how big a target it is.
    pub radius: f32,
    /// Opening (true) or shutting.
    pub want: bool,
    /// 0 shut .. 1 open, along the swing (eased into the angle).
    pub t: f32,
    /// The placed model it belongs to.
    pub root: Entity,
    base: Quat,
}

impl Lid {
    /// Current swing (radians).
    pub fn angle(&self) -> f32 {
        let t = self.t.clamp(0.0, 1.0);
        self.open * t * t * (3.0 - 2.0 * t)
    }

    /// 0 shut .. 1 wide open.
    pub fn openness(&self) -> f32 {
        self.angle() / self.open.max(1e-3)
    }
}

/// A drape's or pennant's hanging end, hinged on its lid's front edge.
#[derive(Component, Debug, Clone)]
pub struct Flap {
    axis: Vec3,
    lag: f32,
    lid: Entity,
    base: Quat,
}

/// The chest the explorer can open right now (looked at, within reach).
#[derive(Resource, Default)]
pub struct LidFocus {
    pub entity: Option<Entity>,
    pub distance: f32,
}

#[derive(Resource)]
struct LidSounds {
    open: Vec<Handle<AudioSource>>,
    close: Vec<Handle<AudioSource>>,
}

fn vec3(v: &Value, k: &str) -> Option<Vec3> {
    let a = v.get(k)?.as_array()?;
    Some(Vec3::new(
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    ))
}

/// Reads a node's `lid` extras: (axis, open radians, lid centre, lid size); None when it has none.
pub fn parse_lid(extras: &str) -> Option<(Vec3, f32, Vec3, Vec3)> {
    let v: Value = serde_json::from_str(extras).ok()?;
    let d = v.get("lid")?;
    let axis = vec3(d, "axis")?.try_normalize()?;
    let open = (d.get("open_deg").and_then(|x| x.as_f64()).unwrap_or(100.0) as f32)
        .clamp(5.0, 180.0)
        .to_radians();
    let leaf = vec3(d, "leaf").unwrap_or(Vec3::ZERO);
    let size = vec3(d, "size").unwrap_or(Vec3::splat(leaf.length() * 2.0));
    Some((axis, open, leaf, size))
}

/// Reads a node's `flap` extras: (axis, lag radians).
pub fn parse_flap(extras: &str) -> Option<(Vec3, f32)> {
    let v: Value = serde_json::from_str(extras).ok()?;
    let d = v.get("flap")?;
    Some((
        vec3(d, "axis")?.try_normalize()?,
        (d.get("lag_deg").and_then(|x| x.as_f64()).unwrap_or(15.0) as f32).to_radians(),
    ))
}

pub fn register(app: &mut App) {
    app.init_resource::<LidFocus>()
        .add_observer(on_scene_ready)
        .add_systems(Startup, load_sounds)
        .add_systems(
            Update,
            (aim, press, swing).chain().after(crate::walk::WalkSystems),
        )
        .add_systems(Update, ensure_listener)
        .add_systems(bevy_egui::EguiPrimaryContextPass, hint);
    app.mcp_tool(McpTool {
        name: "chests",
        description: "Chests with lids that open (the hand-painted pack's chests, their scenes, the Poly Haven treasure chest) near `position` ([x, z]; default: the explorer while walking, else the whole world) within `radius` (m, default 25): name, position, distance, how open (0 shut .. 1 open) and whether it's the one in reach. While walking, the explorer opens or shuts the chest they look at with E (the lid swings back with a creak, contents inside). Pass `state` (open, closed or toggle) to set the listed chests (`nearest: true`: only the nearest), with their sounds.",
        input_schema: || json!({"type": "object", "properties": {
            "position": position_schema("centre of the search"),
            "radius": {"type": "number", "description": "metres (default 25)"},
            "state": {"type": "string", "enum": ["open", "closed", "toggle"]},
            "nearest": {"type": "boolean", "description": "only the nearest chest"}}}),
        run: chests,
    })
    .mcp_tool(McpTool {
        name: "interact",
        description: "While walking, press the interact key (E) as a player would: opens or shuts the chest the explorer looks at within reach (walk_state's `interactable` says what that is). Returns what happened; fails when nothing is in reach.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: interact,
    });
}

fn load_sounds(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(LidSounds {
        open: (0..2)
            .map(|i| server.load(format!("audio/props/chest_open_{i}.mp3")))
            .collect(),
        close: (0..2)
            .map(|i| server.load(format!("audio/props/chest_close_{i}.mp3")))
            .collect(),
    });
}

/// Positional sounds need ears: the main camera's (unless something already listens).
fn ensure_listener(
    mut commands: Commands,
    cams: Query<Entity, (With<MainCamera>, Without<SpatialListener>)>,
    ears: Query<(), With<SpatialListener>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    if !ears.is_empty() {
        *done = true;
        return;
    }
    if let Some(c) = cams.iter().next() {
        commands.entity(c).insert(SpatialListener::new(0.3));
        *done = true;
    }
}

/// A model's scene is in: its lid nodes get their `Lid`, the flaps hung from them their `Flap`.
pub fn on_scene_ready(
    ev: On<WorldInstanceReady>,
    children: Query<&Children>,
    nodes: Query<(&GltfExtras, &Transform)>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let root = ev.event().entity;
    for c in children.iter_descendants(root) {
        let Ok((x, tf)) = nodes.get(c) else { continue };
        if let Some((axis, open, leaf, size)) = parse_lid(&x.value) {
            let radius = size.length() * 0.5;
            commands.entity(c).insert(Lid {
                axis,
                open,
                leaf,
                radius: radius.max(0.2),
                want: false,
                t: 0.0,
                root,
                base: tf.rotation,
            });
        } else if let Some((axis, lag)) = parse_flap(&x.value) {
            let Ok(up) = parents.get(c) else { continue };
            let lid = up.parent();
            if nodes
                .get(lid)
                .is_ok_and(|(px, _)| parse_lid(&px.value).is_some())
            {
                commands.entity(c).insert(Flap {
                    axis,
                    lag,
                    lid,
                    base: tf.rotation,
                });
            }
        }
    }
}

/// The shut lid's centre (world space), and how big it is there.
pub fn lid_centre(lid: &Lid, gt: &GlobalTransform) -> (Vec3, f32) {
    let (scale, rot, pivot) = gt.to_scale_rotation_translation();
    let shut = rot * Quat::from_axis_angle(lid.axis, lid.angle()).inverse();
    (
        pivot + shut * (scale * lid.leaf),
        lid.radius * scale.max_element(),
    )
}

/// How well the eye at `eye` looking along `fwd` aims at a lid at `c` (radius r): None when out
/// of reach or looked past, else a score (lower is better).
fn aim_score(eye: Vec3, fwd: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let to = c - eye;
    let dist = to.length();
    let edge = (dist - r).max(0.0);
    if edge > REACH {
        return None;
    }
    let ang = fwd.angle_between(to);
    let allow = AIM.max((r / dist.max(0.05)).atan());
    (ang <= allow).then_some(ang / allow + 0.5 * edge / REACH)
}

/// Picks the chest the explorer looks at within reach.
fn aim(
    walk: Res<WalkState>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    lids: Query<(Entity, &Lid, &GlobalTransform, &InheritedVisibility)>,
    mut focus: ResMut<LidFocus>,
) {
    focus.entity = None;
    if !walk.active || walk.cinematic {
        return;
    }
    let Ok(cam) = cam.single() else { return };
    let (eye, fwd) = (cam.translation(), cam.forward().as_vec3());
    let mut best: Option<(f32, Entity, f32)> = None;
    for (e, lid, gt, vis) in &lids {
        if !vis.get() {
            continue;
        }
        let (c, r) = lid_centre(lid, gt);
        if let Some(s) = aim_score(eye, fwd, c, r)
            && best.is_none_or(|b| s < b.0)
        {
            best = Some((s, e, (c - eye).length()));
        }
    }
    if let Some((_, e, d)) = best {
        focus.entity = Some(e);
        focus.distance = d;
    }
}

/// Opens a shut (or shutting) lid, shuts an open one; plays its sound at `at`. Returns whether it
/// is now opening.
fn toggle(
    commands: &mut Commands,
    lid: &mut Lid,
    at: Vec3,
    sounds: Option<&LidSounds>,
    settings: &AudioSettings,
) -> bool {
    lid.want = !lid.want;
    play(commands, lid.want, at, sounds, settings);
    lid.want
}

fn play(
    commands: &mut Commands,
    opening: bool,
    at: Vec3,
    sounds: Option<&LidSounds>,
    settings: &AudioSettings,
) {
    let Some(s) = sounds else { return };
    let v = if opening { &s.open } else { &s.close };
    if v.is_empty() {
        return;
    }
    let vol = if settings.muted { 0.0 } else { settings.master } * if opening { 1.0 } else { 0.9 };
    commands.spawn((
        Name::new("Chest lid sound"),
        AudioPlayer(v[fastrand::usize(..v.len())].clone()),
        PlaybackSettings::DESPAWN
            .with_volume(Volume::Linear(vol))
            .with_speed(0.94 + fastrand::f32() * 0.12)
            .with_spatial(true)
            .with_spatial_scale(SpatialScale::new(0.45)),
        Transform::from_translation(at),
    ));
}

/// E: open or shut the chest in reach.
fn press(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    walk: Res<WalkState>,
    pointer: Res<crate::camera::Pointer>,
    focus: Res<LidFocus>,
    mut lids: Query<(&mut Lid, &GlobalTransform)>,
    sounds: Option<Res<LidSounds>>,
    settings: Res<AudioSettings>,
) {
    if !walk.active || pointer.keyboard_captured || !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    // (⌘E / Ctrl+E is export)
    if keys.any_pressed([
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
    ]) {
        return;
    }
    let Some(e) = focus.entity else { return };
    let Ok((mut lid, gt)) = lids.get_mut(e) else {
        return;
    };
    let (c, _) = lid_centre(&lid, gt);
    if toggle(&mut commands, &mut lid, c, sounds.as_deref(), &settings) {
        crate::ui::tips::mark(TIP, true);
    }
}

/// Swings lids toward open or shut, and their flaps back against them.
fn swing(
    time: Res<Time>,
    mut lids: Query<(&mut Lid, &mut Transform)>,
    mut flaps: Query<(&Flap, &mut Transform), Without<Lid>>,
) {
    let dt = time.delta_secs().min(0.1);
    for (mut lid, mut tf) in &mut lids {
        let target = if lid.want { 1.0 } else { 0.0 };
        if (lid.t - target).abs() < 1e-5 {
            continue;
        }
        let step = dt / if lid.want { OPEN_S } else { CLOSE_S };
        lid.t += (target - lid.t).clamp(-step, step);
        tf.rotation = lid.base * Quat::from_axis_angle(lid.axis, lid.angle());
    }
    for (f, mut tf) in &mut flaps {
        let Ok((lid, _)) = lids.get(f.lid) else {
            continue;
        };
        let back = (lid.angle() - f.lag).max(0.0);
        let r = f.base * Quat::from_axis_angle(f.axis, -back);
        if tf.rotation != r {
            tf.rotation = r;
        }
    }
}

/// The hint by the crosshair: the key and what it does (and, the first times, how chests work).
fn hint(
    mut contexts: EguiContexts,
    walk: Res<WalkState>,
    focus: Res<LidFocus>,
    lids: Query<&Lid>,
) -> Result {
    if !walk.active || walk.cinematic {
        return Ok(());
    }
    let Some(lid) = focus.entity.and_then(|e| lids.get(e).ok()) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("lid_hint")));
    let c = screen.center() + vec2(0.0, 58.0);
    let what = if lid.want {
        "Close chest"
    } else {
        "Open chest"
    };
    let label = painter.layout_no_wrap(
        what.to_string(),
        FontId::new(14.0, crate::ui::theme::semibold()),
        Color32::WHITE,
    );
    let key = 22.0;
    let w = key + 10.0 + label.size().x;
    let r = egui::Rect::from_center_size(c, vec2(w + 24.0, 34.0));
    painter.rect_filled(
        r,
        CornerRadius::same(9),
        Color32::from_rgba_unmultiplied(10, 14, 20, 200),
    );
    let kr = egui::Rect::from_min_size(pos2(r.min.x + 12.0, c.y - key / 2.0), vec2(key, key));
    painter.rect_filled(kr, CornerRadius::same(5), Color32::from_rgb(255, 214, 120));
    painter.rect_stroke(
        kr,
        CornerRadius::same(5),
        Stroke::new(1.0, Color32::from_rgb(140, 100, 40)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        kr.center(),
        Align2::CENTER_CENTER,
        "E",
        FontId::new(13.0, crate::ui::theme::semibold()),
        Color32::from_rgb(40, 28, 10),
    );
    painter.galley(
        pos2(kr.max.x + 10.0, c.y - label.size().y / 2.0),
        label,
        Color32::WHITE,
    );
    if !crate::ui::tips::seen(TIP) {
        painter.text(
            pos2(c.x, r.max.y + 12.0),
            Align2::CENTER_CENTER,
            "Chests open: look inside, press E again to shut it",
            FontId::proportional(12.5),
            Color32::from_rgba_unmultiplied(235, 238, 245, 210),
        );
    }
    Ok(())
}

// ------------------------------------------------------------------ MCP

/// The placed model a lid belongs to: (library id, name).
fn model_of(world: &World, root: Entity) -> (String, String) {
    let id = world
        .get::<crate::objects::PlacedObject>(root)
        .map(|p| p.id);
    let asset = id
        .and_then(|i| {
            world
                .resource::<crate::objects::ObjectStore>()
                .items
                .get(&i)
                .map(|o| o.asset.clone())
        })
        .unwrap_or_default();
    let name = world
        .get_resource::<crate::objects::Library>()
        .and_then(|l| l.get(&asset).map(|e| e.name.clone()))
        .unwrap_or_else(|| asset.clone());
    (asset, name)
}

fn describe(world: &World, e: Entity, dist: Option<f32>) -> Option<Value> {
    let lid = world.get::<Lid>(e)?;
    let gt = world.get::<GlobalTransform>(e)?;
    let (c, _) = lid_centre(lid, gt);
    let (asset, model) = model_of(world, lid.root);
    let node = world
        .get::<Name>(e)
        .map(|n| n.as_str().to_string())
        .unwrap_or_default();
    Some(json!({
        "name": if model.is_empty() { node.clone() } else { model },
        "asset": asset,
        "node": node,
        "position": [c.x, c.y, c.z],
        "distance": dist,
        "open": lid.openness(),
        "state": if lid.want { "open" } else { "closed" },
        "in_reach": world.resource::<LidFocus>().entity == Some(e),
    }))
}

/// The chest in reach, for `walk_state`.
pub fn focus_json(world: &mut World) -> Option<Value> {
    let f = world.resource::<LidFocus>();
    let (e, d) = (f.entity?, f.distance);
    let mut v = describe(world, e, Some(d))?;
    let open = world.get::<Lid>(e)?.want;
    v["kind"] = json!("chest");
    v["key"] = json!("E");
    v["hint"] = json!(if open { "Close chest" } else { "Open chest" });
    Some(v)
}

fn sound_at(world: &mut World, opening: bool, at: Vec3) {
    let settings = world.resource::<AudioSettings>().clone();
    let sounds = world.remove_resource::<LidSounds>();
    let mut q = bevy::ecs::world::CommandQueue::default();
    {
        let mut commands = Commands::new(&mut q, world);
        play(&mut commands, opening, at, sounds.as_ref(), &settings);
    }
    q.apply(world);
    if let Some(s) = sounds {
        world.insert_resource(s);
    }
}

fn chests(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let state: Option<String> = opt_arg(&args, "state")?;
    if let Some(s) = &state
        && !matches!(s.as_str(), "open" | "closed" | "close" | "shut" | "toggle")
    {
        return Err("`state` must be open, closed or toggle".into());
    }
    let nearest = opt_arg::<bool>(&args, "nearest")?.unwrap_or(false);
    let radius = opt_arg::<f32>(&args, "radius")?.unwrap_or(25.0).max(0.1);
    let centre = match position_arg(&args, "position")? {
        Some((p, _)) => Some(p),
        None => {
            let w = world.resource::<WalkState>();
            w.active.then(|| Vec2::new(w.position.x, w.position.z))
        }
    };
    let mut found: Vec<(f32, Entity, Vec3)> = Vec::new();
    let mut q = world.query::<(Entity, &Lid, &GlobalTransform)>();
    for (e, lid, gt) in q.iter(world) {
        let (c, _) = lid_centre(lid, gt);
        let d = centre.map_or(0.0, |p| p.distance(c.xz()));
        if d <= radius || centre.is_none() {
            found.push((d, e, c));
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    if nearest {
        found.truncate(1);
    }
    if let Some(s) = &state {
        for &(_, e, c) in &found {
            let Some(mut lid) = world.get_mut::<Lid>(e) else {
                continue;
            };
            let want = match s.as_str() {
                "open" => true,
                "toggle" => !lid.want,
                _ => false,
            };
            if lid.want != want {
                lid.want = want;
                sound_at(world, want, c);
            }
        }
    }
    let n = found.len();
    let list: Vec<Value> = found
        .iter()
        .take(100)
        .filter_map(|&(d, e, _)| describe(world, e, centre.map(|_| d)))
        .collect();
    Ok(vec![McpContent::Json(
        json!({"count": n, "chests": list, "set": state}),
    )])
}

fn interact(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    if !world.resource::<WalkState>().active {
        return Err("not walking: walk_start or teleport first".into());
    }
    let Some(e) = world.resource::<LidFocus>().entity else {
        return Err("nothing to interact with in reach: walk up to a chest (within about 2 m) and look at it".into());
    };
    let (c, opening) = {
        let Some(gt) = world.get::<GlobalTransform>(e).copied() else {
            return Err("the chest is gone".into());
        };
        let Some(mut lid) = world.get_mut::<Lid>(e) else {
            return Err("the chest is gone".into());
        };
        lid.want = !lid.want;
        let (c, _) = lid_centre(&lid, &gt);
        (c, lid.want)
    };
    sound_at(world, opening, c);
    if opening {
        crate::ui::tips::mark(TIP, true);
    }
    let v = describe(world, e, None).unwrap_or(Value::Null);
    Ok(vec![McpContent::Json(
        json!({"did": if opening { "opened" } else { "closed" }, "chest": v}),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lid_and_flap_extras() {
        let (axis, open, leaf, size) = parse_lid(r#"{"lid": {"axis": [-2, 0, 0], "open_deg": 90, "leaf": [0, 0.1, 0.5], "size": [1.5, 0.4, 1.0]}}"#).unwrap();
        assert_eq!(axis, Vec3::NEG_X);
        assert!((open - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(leaf, Vec3::new(0.0, 0.1, 0.5));
        assert_eq!(size, Vec3::new(1.5, 0.4, 1.0));
        assert!(parse_lid(r#"{"door": {"axis": [0, 1, 0]}}"#).is_none());
        let (axis, lag) = parse_flap(r#"{"flap": {"axis": [1, 0, 0], "lag_deg": 22}}"#).unwrap();
        assert_eq!(axis, Vec3::X);
        assert!((lag - 22f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn eases_open_and_finds_the_shut_centre() {
        let mut lid = Lid {
            axis: Vec3::NEG_X,
            open: 1.8,
            leaf: Vec3::new(0.0, 0.1, 0.5),
            radius: 0.8,
            want: true,
            t: 0.5,
            root: Entity::PLACEHOLDER,
            base: Quat::IDENTITY,
        };
        assert!((lid.angle() - 0.9).abs() < 1e-6);
        lid.t = 1.0;
        assert!((lid.openness() - 1.0).abs() < 1e-6);
        // the node turned open: its shut centre is still where the lid lies shut
        let gt = GlobalTransform::from(
            Transform::from_xyz(2.0, 0.6, 3.0)
                .with_rotation(Quat::from_axis_angle(Vec3::NEG_X, lid.angle())),
        );
        let (c, r) = lid_centre(&lid, &gt);
        assert!((c - Vec3::new(2.0, 0.7, 3.5)).length() < 1e-4);
        assert!((r - 0.8).abs() < 1e-6);
    }

    #[test]
    fn aims_at_the_chest_ahead_within_reach() {
        let eye = Vec3::new(0.0, 1.6, 0.0);
        let fwd = (Vec3::new(0.0, 0.5, -1.5) - eye).normalize();
        assert!(aim_score(eye, fwd, Vec3::new(0.0, 0.5, -1.5), 0.6).is_some());
        // behind the explorer, or too far
        assert!(aim_score(eye, -fwd, Vec3::new(0.0, 0.5, -1.5), 0.6).is_none());
        assert!(aim_score(eye, Vec3::NEG_Z, Vec3::new(0.0, 0.5, -6.0), 0.6).is_none());
    }
}
