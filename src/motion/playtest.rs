//! Playtest: play the world the way a player will. A session starts at the gameplay player start
//! (else where the camera looks), in walk mode with a game HUD (`hud.rs`): a compass strip with the
//! points of interest, a minimap, the zone banner, a session clock, distance and discoveries.
//! Walking into a POI's radius discovers it (a toast and a sting). Every second a breadcrumb of
//! the route is recorded; when the session ends (Esc) a summary shows the time, distance,
//! discoveries and the longest stretch without finding anything — the pacing a designer wants to
//! see. The last ten sessions are kept with the world, drawn in the editor as route lines and a
//! heat map, and export as JSON.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::MotionData;
use crate::gameplay::Gameplay;
use crate::terrain::Terrain;
use crate::walk::{WalkStart, WalkState};

/// Sessions kept with the world.
pub const KEEP: usize = 10;
/// Seconds between breadcrumbs.
const CRUMB: f32 = 1.0;

/// A finished playtest session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub id: u32,
    /// When it started (seconds since the Unix epoch).
    pub started: u64,
    /// seconds
    pub duration: f32,
    /// metres travelled
    pub distance: f32,
    pub discoveries: Vec<Discovery>,
    /// POIs the world had when it was played
    #[serde(default)]
    pub poi_count: usize,
    /// The route: one "x,y,z" breadcrumb (0.1 m) per second, space separated (compact on disk).
    pub route: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Discovery {
    /// seconds into the session
    pub t: f32,
    pub poi: u32,
    pub name: String,
}

impl Session {
    pub fn points(&self) -> Vec<Vec3> {
        self.route
            .split_whitespace()
            .filter_map(|p| {
                let mut it = p.split(',').filter_map(|v| v.parse::<f32>().ok());
                Some(Vec3::new(it.next()?, it.next()?, it.next()?))
            })
            .collect()
    }
    /// The longest stretch (s) without a discovery: start → first, between, last → end.
    pub fn longest_gap(&self) -> f32 {
        let mut last = 0.0f32;
        let mut gap = 0.0f32;
        for d in &self.discoveries {
            gap = gap.max(d.t - last);
            last = d.t;
        }
        gap.max(self.duration - last)
    }
    /// Plain JSON for other tools: the route as [x, y, z, t] rows.
    pub fn export_json(&self) -> serde_json::Value {
        let pts: Vec<[f32; 4]> = self
            .points()
            .iter()
            .enumerate()
            .map(|(i, p)| [p.x, p.y, p.z, i as f32 * CRUMB])
            .collect();
        serde_json::json!({
            "id": self.id,
            "started_unix": self.started,
            "duration_s": self.duration,
            "distance_m": self.distance,
            "poi_count": self.poi_count,
            "longest_gap_s": self.longest_gap(),
            "discoveries": self.discoveries.iter().map(|d| serde_json::json!({ "t": d.t, "poi": d.poi, "name": d.name })).collect::<Vec<_>>(),
            "route_columns": ["x", "y", "z", "t"],
            "route": pts,
        })
    }
    /// One-line description for lists and MCP.
    pub fn label(&self) -> String {
        format!(
            "{} · {:.2} km · {} found",
            clock(self.duration),
            self.distance / 1000.0,
            self.discoveries.len()
        )
    }
}

/// `mm:ss` (or `h:mm:ss`).
pub fn clock(s: f32) -> String {
    let s = s.max(0.0) as u32;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// The session being played.
#[derive(Default)]
pub struct Live {
    pub started: u64,
    pub t: f32,
    pub distance: f32,
    last: Option<Vec3>,
    next_crumb: f32,
    pub route: Vec<Vec3>,
    pub found: Vec<Discovery>,
    pub poi_count: usize,
    /// (title, subtitle, age s)
    pub toast: Option<(String, String, f32)>,
}

/// Playtest state: the live session and what the editor shows of past ones.
#[derive(Resource, Default)]
pub struct Playtest {
    pub live: Option<Live>,
    /// Summary of the session that just ended (shown once back in the editor).
    pub summary: Option<Session>,
    /// Sessions whose route is drawn in the editor.
    pub trails: Vec<u32>,
    /// Show the heat map of the drawn routes on the terrain.
    pub heat: bool,
    heat_key: Option<(Vec<u32>, usize)>,
    heat_image: Option<Handle<Image>>,
}

pub struct PlaytestPlugin;

impl Plugin for PlaytestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Playtest>().add_systems(
            Update,
            (
                start,
                track.after(crate::walk::WalkSystems),
                draw_trails,
                heat_overlay,
            ),
        );
    }
}

/// Where a session starts: the player start, else the camera focus.
pub fn spawn_point(gameplay: &Gameplay, focus: Vec3) -> (Vec3, Option<f32>) {
    match &gameplay.player_start {
        // (gameplay yaw: 0 = +x, counter-clockwise from above; walk yaw: 0 = −z)
        Some(s) => (
            Vec3::from_array(s.pos),
            Some(s.yaw - std::f32::consts::FRAC_PI_2),
        ),
        None => (focus, None),
    }
}

/// Begin a session (from the UI, MCP or tests).
pub fn begin(pt: &mut Playtest, walk: &mut WalkState, gameplay: &Gameplay, focus: Vec3) {
    let (pos, yaw) = spawn_point(gameplay, focus);
    walk.request = Some(WalkStart {
        pos: Some(pos),
        yaw,
        mode: None,
        playtest: true,
    });
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    pt.live = Some(Live {
        started,
        poi_count: gameplay.pois.len(),
        ..default()
    });
    pt.summary = None;
}

fn start(
    mut actions: MessageReader<super::MotionAction>,
    mut pt: ResMut<Playtest>,
    mut walk: ResMut<WalkState>,
    gameplay: Res<Gameplay>,
    orbit: Res<crate::camera::OrbitCamera>,
) {
    for a in actions.read() {
        if matches!(a, super::MotionAction::Playtest) {
            begin(&mut pt, &mut walk, &gameplay, orbit.focus);
        }
    }
}

/// Clock, distance, breadcrumbs and discoveries; the session ends when walking does.
#[allow(clippy::too_many_arguments)]
fn track(
    mut pt: ResMut<Playtest>,
    walk: Res<WalkState>,
    gameplay: Res<Gameplay>,
    time: Res<Time>,
    mut data: ResMut<MotionData>,
    mut sounds: MessageWriter<crate::audio::SoundEvent>,
    mut meta: ResMut<crate::world::WorldMeta>,
) {
    let Some(live) = &mut pt.live else { return };
    // still waiting for walk mode to pick up the start request
    if walk.request.is_some() {
        return;
    }
    if !walk.active || !walk.playtest {
        let live = pt.live.take().unwrap_or_default();
        if live.t < 1.0 {
            return;
        }
        let route = live
            .route
            .iter()
            .map(|p| format!("{:.1},{:.1},{:.1}", p.x, p.y, p.z))
            .collect::<Vec<_>>()
            .join(" ");
        let s = Session {
            id: data.alloc_id(),
            started: live.started,
            duration: live.t,
            distance: live.distance,
            discoveries: live.found,
            poi_count: live.poi_count,
            route,
        };
        info!(
            "PLAYTEST ended: {} (longest gap {:.0} s)",
            s.label(),
            s.longest_gap()
        );
        pt.trails = vec![s.id];
        pt.summary = Some(s.clone());
        data.sessions.push(s);
        let n = data.sessions.len();
        if n > KEEP {
            data.sessions.drain(..n - KEEP);
        }
        meta.dirty = true;
        return;
    }
    let dt = time.delta_secs().min(0.1);
    live.t += dt;
    let p = walk.position;
    if let Some(l) = live.last {
        let d = l.distance(p);
        // (a teleport is not travel)
        if d < 60.0 * dt.max(0.02) {
            live.distance += d;
        }
    }
    live.last = Some(p);
    if live.t >= live.next_crumb {
        live.next_crumb += CRUMB;
        live.route.push(p);
    }
    for poi in &gameplay.pois {
        if live.found.iter().any(|d| d.poi == poi.id) {
            continue;
        }
        let d = Vec2::new(poi.pos[0], poi.pos[2]).distance(p.xz());
        if d <= poi.radius.max(5.0) {
            live.found.push(Discovery {
                t: live.t,
                poi: poi.id,
                name: poi.name.clone(),
            });
            live.toast = Some((
                poi.name.clone(),
                format!("{} discovered", poi.kind.name()),
                0.0,
            ));
            sounds.write(crate::audio::SoundEvent::Ui("landmark_discovered_0"));
            info!("PLAYTEST discovered {} at {:.0} s", poi.name, live.t);
        }
    }
    if let Some(t) = &mut live.toast {
        t.2 += dt;
        if t.2 > 4.5 {
            live.toast = None;
        }
    }
}

/// Routes of the chosen sessions as lines over the ground (in the editor).
fn draw_trails(
    pt: Res<Playtest>,
    data: Res<MotionData>,
    walk: Res<WalkState>,
    terrain: Res<Terrain>,
    mut gizmos: Gizmos,
    mut parsed: Local<std::collections::HashMap<u32, Vec<Vec3>>>,
) {
    if walk.active || walk.cinematic || pt.trails.is_empty() {
        return;
    }
    // (a session's route never changes: parse it once)
    parsed.retain(|id, _| data.sessions.iter().any(|s| s.id == *id));
    for (k, s) in data
        .sessions
        .iter()
        .filter(|s| pt.trails.contains(&s.id))
        .enumerate()
    {
        let hue = (k as f32 * 67.0 + 30.0) % 360.0;
        let pts = parsed.entry(s.id).or_insert_with(|| s.points());
        let n = pts.len().max(1) as f32;
        // lifted a little above the ground, fading from start to end
        let line = pts.iter().enumerate().map(|(i, p)| {
            let y = p.y.max(terrain.height_at(p.x, p.z)) + 1.2;
            (
                Vec3::new(p.x, y, p.z),
                Color::hsla(hue, 0.85, 0.62, 0.45 + 0.55 * i as f32 / n),
            )
        });
        gizmos.linestrip_gradient(line);
        for d in &s.discoveries {
            if let Some(p) = pts.get((d.t / CRUMB) as usize) {
                let y = p.y.max(terrain.height_at(p.x, p.z)) + 1.2;
                gizmos.sphere(
                    Isometry3d::from_translation(Vec3::new(p.x, y, p.z)),
                    2.5,
                    Color::srgb(1.0, 0.85, 0.3),
                );
            }
        }
    }
}

/// Where players spent their time: a heat map of the drawn routes on the terrain.
fn heat_overlay(
    mut pt: ResMut<Playtest>,
    data: Res<MotionData>,
    terrain: Res<Terrain>,
    mut overlay: ResMut<crate::terrain::overlay::TerrainOverlay>,
    mut images: ResMut<Assets<Image>>,
) {
    if !pt.heat || pt.trails.is_empty() {
        overlay.clear("playtest");
        pt.heat_key = None;
        return;
    }
    // (built when the shown routes change; another tool may take the overlay over meanwhile)
    let key = (pt.trails.clone(), data.sessions.len());
    if pt.heat_key.as_ref() == Some(&key) {
        return;
    }
    pt.heat_key = Some(key);
    let n = 512usize;
    let size = terrain.size();
    let half = terrain.half();
    let mut acc = vec![0.0f32; n * n];
    for s in data.sessions.iter().filter(|s| pt.trails.contains(&s.id)) {
        // a second at each breadcrumb, splatted with a soft brush ~2 texels wide
        for p in s.points() {
            let g = (Vec2::new(p.x, p.z) + Vec2::splat(half)) / size * n as f32;
            for dj in -3i32..=3 {
                for di in -3i32..=3 {
                    let (i, j) = (g.x as i32 + di, g.y as i32 + dj);
                    if i < 0 || j < 0 || i >= n as i32 || j >= n as i32 {
                        continue;
                    }
                    let w = (-(di * di + dj * dj) as f32 / 4.0).exp();
                    acc[j as usize * n + i as usize] += w;
                }
            }
        }
    }
    let max = acc.iter().cloned().fold(1e-3, f32::max);
    let mut img = crate::terrain::overlay::new_overlay_image(n as u32, n as u32);
    if let Some(px) = img.data.as_mut() {
        for (k, a) in acc.iter().enumerate() {
            if *a <= 0.01 {
                continue;
            }
            // cool blue for passing through, hot yellow-white where time piled up
            let t = (a / max).powf(0.5);
            let c = Color::hsl(220.0 - 180.0 * t, 0.9, 0.45 + 0.25 * t).to_srgba();
            px[k * 4] = (c.red * 255.0) as u8;
            px[k * 4 + 1] = (c.green * 255.0) as u8;
            px[k * 4 + 2] = (c.blue * 255.0) as u8;
            px[k * 4 + 3] = (110.0 + 145.0 * t) as u8;
        }
    }
    let h = match &pt.heat_image {
        Some(h) => {
            let _ = images.insert(h, img);
            h.clone()
        }
        None => {
            let h = images.add(img);
            pt.heat_image = Some(h.clone());
            h
        }
    };
    overlay.show("playtest", h, 0.85);
}
