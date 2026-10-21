//! Automation for motion tests (all only with their env var set), plus the deferred "get into
//! the water" step MCP's `walk_start` uses:
//!
//! - `WB_PLAYTEST=1`: start a playtest at frame 40 (like `WB_WALK`); `WB_POIS=1` first adds a
//!   player start and a few points of interest around the camera focus (test worlds only).
//! - `WB_TELEPORT=x,z[,yaw]`: where walking starts (instead of the camera focus).
//! - `WB_MOTION=fly|mount|swim|dive[:depth]|crouch`: the mode, 20 frames after walking starts.
//! - `WB_HOLD=W+Shift+Space…` (W A S D Space Shift Alt C Ctrl): keys held from 60 frames after
//!   walking starts (`WB_HOLD_FRAMES=n` stops after n frames).
//! - `WB_PITCH=-0.3`: look up / down (radians) once walking.
//! - `WB_APPROACH=Boulder[,jump]`: put the walker 7 m from the nearest such object facing it
//!   and walk into it (jumping when `jump`), to test collisions.
//! - `WB_MLOG=1`: log the explorer every 10 frames (`MLOG …`) and every collision event
//!   (`COLL blocked by …`, `COLL standing on …`, `COLL stepped up …`).
//! - `WB_PANEL=paths|playtests@frame`: open the Paths & Playtests window at that frame.
//! - `WB_PICK=dx,dz;dx,dz…@frame`: start picking flight places at that frame, with these places
//!   already clicked (metres from the camera focus; `@frame` alone starts with none; `@frame!`
//!   makes the flight straight away).
//! - `WB_FLYPATH=1`: build a four-key camera path around the camera focus at frame 380 and play
//!   it; `WB_CAPTURE=<dir>` captures it instead (1280×720, 30 fps; `WB_CAPTURE_SECS` shortens the
//!   path).
//! - `WB_MSCRIPT="frame:cmd args;…"`: a scenario in one run (frames counted from walk start):
//!   `tp x,z[,yaw]`, `tpy x,y,z[,yaw]` (exact height: a building's floor), `mode fly|mount|foot`,
//!   `water swim|dive [depth]`, `pitch p`, `yaw y`,
//!   `hold W+Shift`, `release`, `approach Boulder`, `wade d` (the nearest spot where the sea is
//!   d metres deep, facing out to sea), `shot name` (into `WB_SHOT`'s folder), `bench` (logs the
//!   CPU cost of a frame's collision and water queries here), `exit`.

use bevy::prelude::*;

use super::movement::MoveMode;
use crate::walk::{WalkStart, WalkState};

/// Things to do once walk mode is up (from MCP `walk_start` and the test hooks).
#[derive(Resource, Default)]
pub struct Pending {
    /// ("swim" | "dive", depth)
    pub water: Option<(String, f32)>,
}

pub struct HooksPlugin;

impl Plugin for HooksPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pending>()
            .add_systems(
                Update,
                (
                    apply_pending,
                    script.before(crate::walk::WalkSystems),
                    scenario.before(crate::walk::WalkSystems),
                    pick_hook,
                )
                    .chain(),
            )
            .add_systems(Last, log_motion);
    }
}

fn apply_pending(world: &mut World) {
    let ready = {
        let w = world.resource::<WalkState>();
        w.active && w.request.is_none()
    };
    if !ready {
        return;
    }
    if let Some((m, depth)) = world.resource_mut::<Pending>().water.take()
        && let Err(e) = super::mcp::enter_water(world, m == "dive", depth)
    {
        warn!("MLOG can't {m}: {e}");
    }
}

fn pick_hook(
    mut frame: Local<u32>,
    mut mui: ResMut<super::hud::MotionUi>,
    orbit: Res<crate::camera::OrbitCamera>,
) {
    *frame += 1;
    let Some((pts, at)) = std::env::var("WB_PICK").ok().and_then(|v| {
        v.split_once('@')
            .map(|(a, b)| (a.to_string(), b.to_string()))
    }) else {
        return;
    };
    let finish = at.ends_with('!');
    if at.trim_end_matches('!').parse::<u32>().ok() != Some(*frame) {
        return;
    }
    let f = orbit.focus.xz();
    let places = pts
        .split(';')
        .filter_map(|p| {
            let (x, z) = p.split_once(',')?;
            Some(f + Vec2::new(x.trim().parse().ok()?, z.trim().parse().ok()?))
        })
        .collect();
    mui.open = true;
    let mut pk = super::pick::Picking::new(None, places);
    pk.finish = finish;
    mui.picking = Some(pk);
}

fn key(name: &str) -> Option<KeyCode> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "w" => KeyCode::KeyW,
        "a" => KeyCode::KeyA,
        "s" => KeyCode::KeyS,
        "d" => KeyCode::KeyD,
        "space" => KeyCode::Space,
        "shift" => KeyCode::ShiftLeft,
        "alt" => KeyCode::AltLeft,
        "c" => KeyCode::KeyC,
        "ctrl" => KeyCode::ControlLeft,
        _ => return None,
    })
}

#[allow(clippy::too_many_arguments)]
fn script(
    mut frame: Local<u32>,
    mut walking: Local<u32>,
    mut walk: ResMut<WalkState>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut pt: ResMut<super::playtest::Playtest>,
    mut gameplay: ResMut<crate::gameplay::Gameplay>,
    orbit: Res<crate::camera::OrbitCamera>,
    terrain: Res<crate::terrain::Terrain>,
    store: Res<crate::objects::ObjectStore>,
    mut pending: ResMut<Pending>,
    mut data: ResMut<super::MotionData>,
    mut fly: ResMut<super::flythrough::Flythrough>,
    mut actions: MessageWriter<super::MotionAction>,
) {
    *frame += 1;
    let env = |k: &str| std::env::var(k).ok();
    if let Some((tab, at)) = env("WB_PANEL").as_deref().and_then(|p| p.split_once('@'))
        && at.parse::<u32>().ok() == Some(*frame)
    {
        actions.write(super::MotionAction::OpenPanel(if tab == "playtests" {
            super::PanelTab::Playtests
        } else {
            super::PanelTab::Paths
        }));
    }
    if *frame == 30 && env("WB_POIS").is_some() {
        let f = orbit.focus;
        gameplay.player_start = Some(crate::gameplay::PlayerStart {
            pos: [f.x, terrain.height_at(f.x, f.z), f.z],
            yaw: std::f32::consts::FRAC_PI_2,
        });
        for (k, (name, kind, dx, dz)) in [
            (
                "Old Watchtower",
                crate::gameplay::PoiKind::Landmark,
                0.0,
                -45.0,
            ),
            ("Hunter's Camp", crate::gameplay::PoiKind::Camp, 140.0, 60.0),
            (
                "Shrine of Tides",
                crate::gameplay::PoiKind::Shrine,
                -210.0,
                -160.0,
            ),
            (
                "Sunset Vista",
                crate::gameplay::PoiKind::Vista,
                320.0,
                -260.0,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let (x, z) = (f.x + dx, f.z + dz);
            let id = gameplay.alloc_id();
            gameplay.pois.push(crate::gameplay::Poi {
                id,
                kind,
                level: None,
                name: name.into(),
                pos: [x, terrain.height_at(x, z), z],
                radius: 25.0 + k as f32 * 5.0,
                notes: String::new(),
                tags: vec![],
            });
        }
    }
    if *frame == 40 && env("WB_PLAYTEST").is_some() {
        super::playtest::begin(&mut pt, &mut walk, &gameplay, orbit.focus);
    }
    if *frame == 38
        && let Some(t) = env("WB_TELEPORT")
    {
        let v: Vec<f32> = t.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if v.len() >= 2 {
            let pos = Vec3::new(v[0], terrain.height_at(v[0], v[1]), v[1]);
            match &mut walk.request {
                Some(r) => r.pos = Some(pos),
                None => {
                    walk.request = Some(WalkStart {
                        pos: Some(pos),
                        yaw: v.get(2).copied(),
                        ..default()
                    })
                }
            }
            if let (Some(r), Some(y)) = (&mut walk.request, v.get(2)) {
                r.yaw = Some(*y);
            }
        }
    }
    if !walk.active {
        *walking = 0;
        return;
    }
    *walking += 1;
    let w = *walking;
    if w == 5
        && let Some(p) = env("WB_PITCH").and_then(|p| p.parse::<f32>().ok())
    {
        walk.pitch = p;
    }
    if w == 20
        && let Some(m) = env("WB_MOTION")
    {
        let (m, arg) = m.split_once(':').unwrap_or((&m, ""));
        match m {
            "fly" => walk.set_mode(MoveMode::Fly),
            "mount" => walk.set_mode(MoveMode::Mount),
            "swim" | "dive" => pending.water = Some((m.to_string(), arg.parse().unwrap_or(3.0))),
            "crouch" => keys.press(KeyCode::KeyC),
            _ => {}
        }
    }
    if w == 25
        && let Some(spec) = env("WB_APPROACH")
    {
        let (asset, jump) = spec
            .split_once(',')
            .map(|(a, j)| (a.to_string(), j == "jump"))
            .unwrap_or((spec.clone(), false));
        match nearest(&store, &asset, walk.position) {
            Some(o) => {
                let target = o.translation();
                // approach from the side with the gentlest ground
                let dir = Vec2::new(0.6, 0.8);
                let from = target.xz() + dir * 7.0;
                let yaw = (-(target.x - from.x)).atan2(-(target.z - from.y));
                walk.teleport(
                    Vec3::new(from.x, terrain.height_at(from.x, from.y), from.y),
                    Some(yaw),
                );
                walk.pitch = -0.1;
                info!(
                    "COLL approach {asset}#{} at ({:.1}, {:.1}, {:.1}) scale {:.2}{}",
                    o.id,
                    target.x,
                    target.y,
                    target.z,
                    o.scale,
                    if jump { " (jumping)" } else { "" }
                );
            }
            None => warn!("COLL no {asset} within 800 m"),
        }
    }
    // held keys (pressed every frame: input is reset each frame in automated runs)
    let limit = env("WB_HOLD_FRAMES")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(u32::MAX);
    if w > 60 && w < limit.saturating_add(60) {
        if let Some(spec) = env("WB_HOLD") {
            for k in spec.split('+').filter_map(key) {
                keys.press(k);
            }
        }
        if env("WB_APPROACH").is_some() {
            keys.press(KeyCode::KeyW);
            if env("WB_APPROACH").is_some_and(|a| a.ends_with(",jump")) && w % 40 == 0 {
                keys.press(KeyCode::Space);
            }
        }
        if env("WB_MOTION").as_deref() == Some("crouch") {
            keys.press(KeyCode::KeyC);
        }
    }
    // camera path test
    if *frame == 380 && env("WB_FLYPATH").is_some() {
        let f = orbit.focus;
        let secs = env("WB_CAPTURE_SECS")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(4.0);
        let id = data.alloc_id();
        let ground = |x: f32, z: f32| terrain.height_at(x, z).max(terrain.sea_level);
        let mut keys_v = vec![];
        for k in 0..4 {
            let a = k as f32 * 0.6;
            let (x, z) = (f.x + a.cos() * 120.0, f.z + a.sin() * 120.0);
            let pos = [x, ground(x, z) + 35.0 + k as f32 * 6.0, z];
            let yaw = (-(f.x - x)).atan2(-(f.z - z));
            keys_v.push(super::flythrough::CameraKey {
                pos,
                yaw,
                pitch: -0.22,
                fov: 50.0,
                dur: secs / 3.0,
                ease: super::flythrough::Ease::Linear,
            });
        }
        data.paths.push(super::flythrough::CameraPath {
            id,
            name: "Test path".into(),
            keys: keys_v,
            timelapse_hours: 1.0,
            looped: false,
            route: None,
        });
        let capture = env("WB_CAPTURE").map(|dir| super::flythrough::CaptureSettings {
            dir: dir.into(),
            fps: 30,
            width: 1280,
            height: 720,
            keep_frames: false,
        });
        fly.play(id, capture);
    }
}

/// `WB_MLOG`: the explorer's state and every collision event.
fn log_motion(
    walk: Res<WalkState>,
    store: Res<crate::objects::ObjectStore>,
    colliders: Res<super::collide::Colliders>,
    lib: Option<Res<crate::objects::Library>>,
    mut n: Local<u32>,
    mut last: Local<(Option<u64>, Option<u64>)>,
) {
    if std::env::var("WB_MLOG").is_err() || !walk.active {
        return;
    }
    *n += 1;
    let Some((out, _)) = &walk.last else { return };
    let blocked = out.contact.blocked_by.as_ref().map(|b| b.0);
    let on = out.on_object.map(|o| o.0);
    let name = |id: u64| {
        store
            .items
            .get(&id)
            .map(|o| format!("{}#{id}", o.asset))
            .unwrap_or(format!("#{id}"))
    };
    if blocked != last.0
        && let Some(id) = blocked
    {
        let fp = lib
            .as_ref()
            .and_then(|l| colliders.footprint(&store, l, id))
            .map(|f| format!(" (base radius {:.2} m, height {:.1} m)", f.1, f.2))
            .unwrap_or_default();
        info!(
            "COLL blocked by {}{fp} at ({:.2}, {:.2}, {:.2})",
            name(id),
            walk.position.x,
            walk.position.y,
            walk.position.z
        );
    }
    if on != last.1
        && let Some(id) = on
    {
        info!("COLL standing on {} at y {:.2}", name(id), walk.position.y);
    }
    if out.stepped > 0.08 {
        info!("COLL stepped up {:.2} m", out.stepped);
    }
    *last = (blocked, on);
    if *n % 10 == 0 {
        let p = walk.position;
        info!(
            "MLOG pos ({:.2}, {:.2}, {:.2}) mode {:?} water {:?} gait {:?} speed {:.2} grounded {} eye {:.2} breath {:.2} submerged {:.0}",
            p.x,
            p.y,
            p.z,
            walk.mode,
            walk.water,
            walk.gait,
            walk.velocity.length(),
            walk.grounded,
            walk.eye_off,
            walk.breath,
            walk.submerged
        );
    }
}

/// The nearest placed object of an asset to `p` (within 800 m).
fn nearest(
    store: &crate::objects::ObjectStore,
    asset: &str,
    p: Vec3,
) -> Option<crate::objects::Placed> {
    store
        .query(p.xz(), 800.0)
        .into_iter()
        .filter_map(|id| store.items.get(&id))
        .filter(|o| o.asset == asset)
        .min_by(|a, b| {
            a.translation()
                .distance_squared(p)
                .total_cmp(&b.translation().distance_squared(p))
        })
        .cloned()
}

/// `WB_MSCRIPT`: timed steps once walking.
#[allow(clippy::too_many_arguments)]
fn scenario(
    mut commands: Commands,
    mut walking: Local<u32>,
    mut held: Local<Vec<KeyCode>>,
    mut walk: ResMut<WalkState>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    terrain: Res<crate::terrain::Terrain>,
    store: Res<crate::objects::ObjectStore>,
    mut pending: ResMut<Pending>,
    mut exit: MessageWriter<AppExit>,
    (colliders, water, ocean, waves, time): (
        Res<super::collide::Colliders>,
        Res<crate::water::WaterBodies>,
        Res<crate::water::WaterStyle>,
        Res<crate::ocean::OceanWaves>,
        Res<Time>,
    ),
) {
    let Ok(script) = std::env::var("WB_MSCRIPT") else {
        return;
    };
    if !walk.active {
        return;
    }
    *walking += 1;
    let now = *walking;
    for k in held.iter() {
        keys.press(*k);
    }
    for step in script.split(';') {
        let Some((f, cmd)) = step.trim().split_once(':') else {
            continue;
        };
        if f.trim().parse::<u32>().ok() != Some(now) {
            continue;
        }
        let (op, arg) = cmd.trim().split_once(' ').unwrap_or((cmd.trim(), ""));
        let nums: Vec<f32> = arg
            .split([',', ' '])
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        info!("MSCRIPT {now}: {op} {arg}");
        match op {
            "tp" if nums.len() >= 2 => {
                let pos = Vec3::new(nums[0], terrain.height_at(nums[0], nums[1]), nums[1]);
                walk.set_mode(MoveMode::Foot);
                walk.teleport(pos, nums.get(2).copied());
            }
            "tpy" if nums.len() >= 3 => {
                walk.set_mode(MoveMode::Foot);
                walk.teleport(Vec3::new(nums[0], nums[1], nums[2]), nums.get(3).copied());
            }
            "mode" => walk.set_mode(match arg {
                "fly" => MoveMode::Fly,
                "mount" => MoveMode::Mount,
                _ => MoveMode::Foot,
            }),
            "water" => {
                let (m, d) = arg.split_once(' ').unwrap_or((arg, "3"));
                pending.water = Some((m.to_string(), d.parse().unwrap_or(3.0)));
            }
            "wade" if !nums.is_empty() => {
                let p = walk.position.xz();
                let want = nums[0];
                let mut best: Option<(f32, Vec2)> = None;
                for j in -150..=150 {
                    for i in -150..=150 {
                        let q = p + Vec2::new(i as f32, j as f32) * 4.0;
                        let d = terrain.sea_level - terrain.height_at(q.x, q.y);
                        if (d - want).abs() < 0.15 && best.is_none_or(|b| q.distance(p) < b.0) {
                            best = Some((q.distance(p), q));
                        }
                    }
                }
                match best {
                    Some((_, q)) => {
                        let n = terrain.normal_at(q.x, q.y);
                        let down = Vec2::new(n.x, n.z).normalize_or(Vec2::Y);
                        walk.set_mode(MoveMode::Foot);
                        walk.teleport(
                            Vec3::new(q.x, terrain.height_at(q.x, q.y), q.y),
                            Some((-down.x).atan2(-down.y)),
                        );
                        info!("MSCRIPT wading at ({:.0}, {:.0}), {want} m deep", q.x, q.y);
                    }
                    None => warn!("MSCRIPT no water {want} m deep near here"),
                }
            }
            "pitch" if !nums.is_empty() => walk.pitch = nums[0],
            "yaw" if !nums.is_empty() => walk.yaw = nums[0],
            "hold" => *held = arg.split('+').filter_map(key).collect(),
            "release" => held.clear(),
            "approach" => match nearest(&store, arg, walk.position) {
                Some(o) => {
                    let t = o.translation();
                    let from = t.xz() + Vec2::new(0.6, 0.8) * 7.0;
                    let yaw = (-(t.x - from.x)).atan2(-(t.z - from.y));
                    walk.set_mode(MoveMode::Foot);
                    walk.teleport(
                        Vec3::new(from.x, terrain.height_at(from.x, from.y), from.y),
                        Some(yaw),
                    );
                    walk.pitch = -0.12;
                    info!(
                        "COLL approach {arg}#{} at ({:.1}, {:.1}, {:.1}) scale {:.2}",
                        o.id, t.x, t.y, t.z, o.scale
                    );
                }
                None => warn!("COLL no {arg} within 800 m"),
            },
            "shot" => {
                let dir = std::env::var("WB_SHOT")
                    .ok()
                    .and_then(|p| std::path::Path::new(&p).parent().map(|d| d.to_path_buf()))
                    .unwrap_or_default();
                let path = dir.join(format!("{arg}.png"));
                info!("MSCRIPT shot {}", path.display());
                commands
                    .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                    .observe(bevy::render::view::screenshot::save_to_disk(path));
            }
            "bench" => {
                // what walk mode asks every frame: push-out, floor, the water surface (×3)
                let p = walk.position;
                let n = 2000;
                let t0 = std::time::Instant::now();
                let mut acc = 0.0;
                for k in 0..n {
                    let q = p + Vec3::new((k % 7) as f32 * 0.05, 0.0, (k % 5) as f32 * 0.05);
                    let (r, _) = colliders.resolve(&store, q, super::collide::Body::WALKER);
                    acc += r.x
                        + colliders
                            .floor(&store, q, 0.22, 0.45)
                            .map(|f| f.0)
                            .unwrap_or(0.0);
                    for _ in 0..3 {
                        acc += super::waves::surface_at(
                            &terrain,
                            &water,
                            &ocean,
                            &waves,
                            q.xz(),
                            &time,
                        )
                        .unwrap_or(0.0);
                    }
                }
                let near = colliders.near(&store, p.xz(), 2.0).len();
                info!(
                    "MBENCH {:.1} µs per frame of collision + water queries ({near} colliders within 2 m; {acc:.0})",
                    t0.elapsed().as_secs_f64() * 1e6 / n as f64
                );
            }
            "exit" => {
                exit.write(AppExit::Success);
            }
            _ => warn!("MSCRIPT: unknown step `{step}`"),
        }
    }
}
