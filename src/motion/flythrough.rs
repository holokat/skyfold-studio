//! Flythroughs: camera paths for cinematics and trailers. A path is a list of keyframes taken from
//! the current view (position, heading, pitch, field of view), each with the time to the next one
//! and an easing for that stretch. The camera follows a cubic Hermite spline through the keys
//! with tangents taken over time (Catmull–Rom on uneven timing), so it flows through the keys
//! with no jolt in speed; an eased stretch slows into its keys. Playback hides the editor, lets
//! the world animate and can run the day forward (time-lapse).
//!
//! Most paths are flights made from places picked on the ground (`Route`): the keys are generated
//! from the places, a style (treetop glide, bird's-eye, showcase) and a speed; one place makes a
//! circle around it. Keys can still be added and tuned by hand.
//!
//! Capture renders the path frame by frame at a fixed time step (each frame advances exactly
//! 1/fps, however long it takes to render) and streams the frames to ffmpeg, which scales them to
//! the chosen resolution and encodes an H.264 MP4 into the chosen folder (PNG frames can be kept
//! too; without ffmpeg only PNG frames are written). Progress shows in the window title; Esc
//! cancels.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::time::TimeUpdateStrategy;
use bevy::window::PrimaryWindow;
use serde::{Deserialize, Serialize};

use super::MotionData;
use crate::camera::{MainCamera, OrbitCamera};
use crate::walk::WalkState;

/// How the camera moves along one stretch between two keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ease {
    /// keep flowing through the keys
    #[default]
    Linear,
    /// slow into both keys (a stop at each)
    InOut,
    /// start slowly
    In,
    /// arrive slowly
    Out,
}

impl Ease {
    pub const ALL: [Ease; 4] = [Ease::Linear, Ease::InOut, Ease::In, Ease::Out];
    pub fn name(self) -> &'static str {
        match self {
            Ease::Linear => "Flow",
            Ease::InOut => "Ease in-out",
            Ease::In => "Ease in",
            Ease::Out => "Ease out",
        }
    }
    fn apply(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Ease::Linear => u,
            Ease::InOut => u * u * (3.0 - 2.0 * u),
            Ease::In => u * u,
            Ease::Out => 1.0 - (1.0 - u) * (1.0 - u),
        }
    }
}

/// One keyframe.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct CameraKey {
    pub pos: [f32; 3],
    /// heading (radians, 0 = looking towards −z) and pitch (radians, up positive)
    pub yaw: f32,
    pub pitch: f32,
    /// vertical field of view (degrees)
    #[serde(default = "default_fov")]
    pub fov: f32,
    /// seconds to the next key
    pub dur: f32,
    /// how the stretch to the next key moves
    #[serde(default)]
    pub ease: Ease,
}

fn default_fov() -> f32 {
    50.0
}

/// A named camera path.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CameraPath {
    pub id: u32,
    pub name: String,
    pub keys: Vec<CameraKey>,
    /// Hours the day advances over the whole path during playback (0 = time stands still).
    #[serde(default)]
    pub timelapse_hours: f32,
    #[serde(default)]
    pub looped: bool,
    /// The places this flight was made from (its keys are generated from them); `None` for a
    /// path built from camera keys, or once its keys were edited by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
}

/// How a flight made from places flies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlightStyle {
    /// low over the treetops, looking ahead
    #[default]
    Glide,
    /// higher, looking ahead and down over the land
    Aerial,
    /// high up, turning to look at each place as it comes
    Showcase,
}

impl FlightStyle {
    pub const ALL: [FlightStyle; 3] = [
        FlightStyle::Glide,
        FlightStyle::Aerial,
        FlightStyle::Showcase,
    ];
    pub fn name(self) -> &'static str {
        match self {
            FlightStyle::Glide => "Treetop glide",
            FlightStyle::Aerial => "Bird's-eye",
            FlightStyle::Showcase => "Showcase",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            FlightStyle::Glide => "Skims about 30 m over the ground, looking ahead.",
            FlightStyle::Aerial => "About 95 m up, looking ahead and down over the land.",
            FlightStyle::Showcase => "High up, turning to look at each place as it comes.",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            FlightStyle::Glide => "glide",
            FlightStyle::Aerial => "aerial",
            FlightStyle::Showcase => "showcase",
        }
    }
    /// metres above the ground
    fn altitude(self) -> f32 {
        match self {
            FlightStyle::Glide => 30.0,
            FlightStyle::Aerial => 95.0,
            FlightStyle::Showcase => 170.0,
        }
    }
    /// cruising speed at the normal pace (m/s)
    fn cruise(self) -> f32 {
        match self {
            FlightStyle::Glide => 16.0,
            FlightStyle::Aerial => 32.0,
            FlightStyle::Showcase => 26.0,
        }
    }
    /// how far it looks down while looking ahead (radians)
    fn look_down(self) -> f32 {
        match self {
            FlightStyle::Glide => 7f32.to_radians(),
            FlightStyle::Aerial => 24f32.to_radians(),
            FlightStyle::Showcase => 30f32.to_radians(),
        }
    }
    /// radius of the circle around a single place (m)
    fn orbit_radius(self) -> f32 {
        match self {
            FlightStyle::Glide => 70.0,
            FlightStyle::Aerial => 160.0,
            FlightStyle::Showcase => 280.0,
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|x| x.id() == s)
    }
}

/// How fast a flight made from places goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlightSpeed {
    Slow,
    #[default]
    Normal,
    Fast,
}

impl FlightSpeed {
    pub const ALL: [FlightSpeed; 3] = [FlightSpeed::Slow, FlightSpeed::Normal, FlightSpeed::Fast];
    pub fn name(self) -> &'static str {
        match self {
            FlightSpeed::Slow => "Slow",
            FlightSpeed::Normal => "Normal",
            FlightSpeed::Fast => "Fast",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            FlightSpeed::Slow => "slow",
            FlightSpeed::Normal => "normal",
            FlightSpeed::Fast => "fast",
        }
    }
    fn factor(self) -> f32 {
        match self {
            FlightSpeed::Slow => 0.55,
            FlightSpeed::Normal => 1.0,
            FlightSpeed::Fast => 1.9,
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|x| x.id() == s)
    }
}

/// The places a flight goes through, in order, and how it flies.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Route {
    /// [x, z] in metres
    pub places: Vec<[f32; 2]>,
    #[serde(default)]
    pub style: FlightStyle,
    #[serde(default)]
    pub speed: FlightSpeed,
}

/// The camera keys of a flight through `route`'s places: a smooth curve through them (centripetal
/// Catmull–Rom, so it never loops between close places), high enough over the ground around each
/// stretch to clear hills, with the height smoothed so the camera doesn't bob. One place makes a
/// circle around it.
pub fn flight_keys(route: &Route, terrain: &crate::terrain::Terrain) -> Vec<CameraKey> {
    let mut pts: Vec<Vec2> = vec![];
    for p in &route.places {
        let p = Vec2::from_array(*p);
        if p.is_finite() && pts.last().is_none_or(|l: &Vec2| l.distance(p) > 2.0) {
            pts.push(p);
        }
    }
    let ground = |p: Vec2| terrain.height_at(p.x, p.y).max(terrain.sea_level);
    let speed = route.style.cruise() * route.speed.factor();
    match pts.len() {
        0 => vec![],
        1 => orbit_keys(pts[0], route.style, speed, &ground),
        _ => route_keys(&pts, route.style, speed, &ground),
    }
}

/// The view heading (0 = looking towards −z) along an (x, z) direction.
fn heading(d: Vec2) -> f32 {
    (-d.x).atan2(-d.y)
}

fn centripetal(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, u: f32) -> Vec2 {
    let knot = |a: Vec2, b: Vec2| a.distance(b).sqrt().max(1e-3);
    let (t1, t2, t3) = {
        let t1 = knot(p0, p1);
        let t2 = t1 + knot(p1, p2);
        (t1, t2, t2 + knot(p2, p3))
    };
    let t = t1 + (t2 - t1) * u;
    let a1 = p0 * ((t1 - t) / t1) + p1 * (t / t1);
    let a2 = p1 * ((t2 - t) / (t2 - t1)) + p2 * ((t - t1) / (t2 - t1));
    let a3 = p2 * ((t3 - t) / (t3 - t2)) + p3 * ((t - t2) / (t3 - t2));
    let b1 = a1 * ((t2 - t) / t2) + a2 * (t / t2);
    let b2 = a2 * ((t3 - t) / (t3 - t1)) + a3 * ((t - t1) / (t3 - t1));
    b1 * ((t2 - t) / (t2 - t1)) + b2 * ((t - t1) / (t2 - t1))
}

fn route_keys(
    pts: &[Vec2],
    style: FlightStyle,
    speed: f32,
    ground: &dyn Fn(Vec2) -> f32,
) -> Vec<CameraKey> {
    let n = pts.len();
    // the curve every ~4 m, and how far along it each place is
    let mut dense: Vec<Vec2> = vec![];
    let mut place_idx = vec![];
    for i in 0..n - 1 {
        let p0 = if i == 0 {
            pts[0] * 2.0 - pts[1]
        } else {
            pts[i - 1]
        };
        let p3 = if i + 2 < n {
            pts[i + 2]
        } else {
            pts[n - 1] * 2.0 - pts[n - 2]
        };
        place_idx.push(dense.len());
        let steps = ((pts[i].distance(pts[i + 1]) / 4.0).ceil() as usize).clamp(2, 5000);
        for s in 0..steps {
            dense.push(centripetal(
                p0,
                pts[i],
                pts[i + 1],
                p3,
                s as f32 / steps as f32,
            ));
        }
    }
    place_idx.push(dense.len());
    dense.push(pts[n - 1]);
    let mut arc = vec![0.0f32; dense.len()];
    for k in 1..dense.len() {
        arc[k] = arc[k - 1] + dense[k].distance(dense[k - 1]);
    }
    let len = *arc.last().unwrap();
    let place_arc: Vec<f32> = place_idx.iter().map(|&i| arc[i]).collect();
    let end_dir =
        (dense[dense.len() - 1] - dense[dense.len().saturating_sub(3)]).normalize_or(Vec2::NEG_Y);
    let start_dir = (dense[2.min(dense.len() - 1)] - dense[0]).normalize_or(Vec2::NEG_Y);
    // the curve at an arc length (straight on past either end)
    let at = |s: f32| -> Vec2 {
        if s <= 0.0 {
            return dense[0] + start_dir * s;
        }
        if s >= len {
            return dense[dense.len() - 1] + end_dir * (s - len);
        }
        let k = arc.partition_point(|&a| a < s).clamp(1, dense.len() - 1);
        let f = (s - arc[k - 1]) / (arc[k] - arc[k - 1]).max(1e-4);
        dense[k - 1].lerp(dense[k], f)
    };
    let step = (len / 45.0).clamp(20.0, 220.0);
    let m = ((len / step).ceil() as usize).max(1);
    let samples: Vec<f32> = (0..=m).map(|j| len * j as f32 / m as f32).collect();
    let alt = style.altitude();
    // the highest ground around each key's stretch, plus the flying height
    let need: Vec<f32> = samples
        .iter()
        .map(|&s| {
            let (lo, hi) = (
                arc.partition_point(|&a| a < s - step * 0.8),
                arc.partition_point(|&a| a <= s + step * 0.8),
            );
            dense[lo..hi.max(lo + 1).min(dense.len())]
                .iter()
                .map(|p| ground(*p))
                .fold(ground(at(s)), f32::max)
                + alt
        })
        .collect();
    let mut h = need.clone();
    for _ in 0..4 {
        let prev = h.clone();
        for j in 0..h.len() {
            let a = prev[j.saturating_sub(1)];
            let b = prev[(j + 1).min(h.len() - 1)];
            h[j] = (0.25 * a + 0.5 * prev[j] + 0.25 * b).max(need[j]);
        }
    }
    let fov = if style == FlightStyle::Glide {
        55.0
    } else {
        50.0
    };
    let mut keys: Vec<CameraKey> = vec![];
    for (j, &s) in samples.iter().enumerate() {
        let q = at(s);
        let pos = Vec3::new(q.x, h[j], q.y);
        let (yaw, pitch) = match style {
            FlightStyle::Showcase => {
                // the next place ahead (the last one at the end)
                let k = place_arc.iter().position(|&a| a > s + 1.0).unwrap_or(n - 1);
                let target = Vec3::new(pts[k].x, ground(pts[k]) + 2.0, pts[k].y);
                let d = target - pos;
                let flat = d.xz().length();
                let yaw = if flat > alt * 0.4 {
                    heading(d.xz())
                } else {
                    keys.last()
                        .map(|k| k.yaw)
                        .unwrap_or(heading(at(s + step) - q))
                };
                (yaw, d.y.atan2(flat.max(1.0)).clamp(-1.0, 0.1))
            }
            _ => {
                // look a few seconds ahead: turns are anticipated, not swung through
                let look = (step * 1.2).max(alt * 0.9);
                (heading(at(s + look) - q), -style.look_down())
            }
        };
        if let Some(prev) = keys.last_mut() {
            prev.dur = (Vec3::from_array(prev.pos).distance(pos) / speed).max(0.3);
        }
        keys.push(CameraKey {
            pos: pos.to_array(),
            yaw,
            pitch,
            fov,
            dur: 4.0,
            ease: Ease::Linear,
        });
    }
    keys
}

fn orbit_keys(
    c: Vec2,
    style: FlightStyle,
    speed: f32,
    ground: &dyn Fn(Vec2) -> f32,
) -> Vec<CameraKey> {
    let r = style.orbit_radius();
    let gc = ground(c);
    let ring = (0..48).map(|i| {
        let a = std::f32::consts::TAU * i as f32 / 48.0;
        ground(c + Vec2::new(a.sin(), a.cos()) * r)
    });
    let h = ring.fold(gc, f32::max) + style.altitude() * 0.7;
    let steps = 16;
    let dur = (std::f32::consts::TAU * r / steps as f32 / speed).max(0.3);
    (0..=steps)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / steps as f32;
            let p = c + Vec2::new(a.sin(), a.cos()) * r;
            let pitch = (gc + 2.0 - h).atan2(r);
            CameraKey {
                pos: [p.x, h, p.y],
                yaw: heading(c - p),
                pitch,
                fov: 50.0,
                dur,
                ease: Ease::Linear,
            }
        })
        .collect()
}

impl CameraPath {
    /// Seconds from the first key to the last.
    pub fn duration(&self) -> f32 {
        if self.keys.len() < 2 {
            return 0.0;
        }
        self.keys[..self.keys.len() - 1]
            .iter()
            .map(|k| k.dur.max(0.05))
            .sum()
    }

    /// The view at `t` seconds: (position, yaw, pitch, fov in degrees).
    pub fn sample(&self, t: f32) -> Option<(Vec3, f32, f32, f32)> {
        let n = self.keys.len();
        if n == 0 {
            return None;
        }
        let k0 = &self.keys[0];
        if n == 1 {
            return Some((Vec3::from_array(k0.pos), k0.yaw, k0.pitch, k0.fov));
        }
        // key times; headings unwrapped so the camera turns the short way
        let mut times = Vec::with_capacity(n);
        let mut acc = 0.0;
        for k in &self.keys {
            times.push(acc);
            acc += k.dur.max(0.05);
        }
        let mut yaws = Vec::with_capacity(n);
        for (i, k) in self.keys.iter().enumerate() {
            let y = if i == 0 {
                k.yaw
            } else {
                let prev: f32 = yaws[i - 1];
                prev + (k.yaw - prev + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI
            };
            yaws.push(y);
        }
        let end = times[n - 1];
        let t = t.clamp(0.0, end);
        let i = times.iter().rposition(|&x| x <= t).unwrap_or(0).min(n - 2);
        let span = times[i + 1] - times[i];
        let u = self.keys[i].ease.apply((t - times[i]) / span.max(1e-4));
        // values as vectors: x, y, z, yaw, pitch, fov
        let val = |j: usize| {
            let k = &self.keys[j];
            [k.pos[0], k.pos[1], k.pos[2], yaws[j], k.pitch, k.fov]
        };
        // tangent at key j (per second): a central difference over time; ends keep their speed
        let tangent = |j: usize| -> [f32; 6] {
            let (a, b) = (j.saturating_sub(1), (j + 1).min(n - 1));
            let dt = (times[b] - times[a]).max(1e-4);
            let (va, vb) = (val(a), val(b));
            std::array::from_fn(|c| (vb[c] - va[c]) / dt)
        };
        let (p0, p1, m0, m1) = (val(i), val(i + 1), tangent(i), tangent(i + 1));
        let (u2, u3) = (u * u, u * u * u);
        let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
        let h10 = u3 - 2.0 * u2 + u;
        let h01 = -2.0 * u3 + 3.0 * u2;
        let h11 = u3 - u2;
        let r: [f32; 6] = std::array::from_fn(|c| {
            h00 * p0[c] + h10 * span * m0[c] + h01 * p1[c] + h11 * span * m1[c]
        });
        Some((
            Vec3::new(r[0], r[1], r[2]),
            r[3],
            r[4].clamp(-1.5, 1.5),
            r[5].clamp(10.0, 120.0),
        ))
    }
}

/// A key from a camera transform.
pub fn key_from_view(t: &GlobalTransform, fov_deg: f32, dur: f32) -> CameraKey {
    let f = t.forward();
    CameraKey {
        pos: t.translation().to_array(),
        yaw: (-f.x).atan2(-f.z),
        pitch: f.y.clamp(-1.0, 1.0).asin(),
        fov: fov_deg,
        dur,
        ease: Ease::Linear,
    }
}

/// A sensible time (s) to fly from one key to the next: ~18 m/s, at least two seconds.
pub fn auto_duration(a: &CameraKey, b: [f32; 3]) -> f32 {
    (Vec3::from_array(a.pos).distance(Vec3::from_array(b)) / 18.0).clamp(2.0, 30.0)
}

/// Output of a capture.
#[derive(Clone, Debug)]
pub struct CaptureSettings {
    pub dir: PathBuf,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub keep_frames: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        let dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Movies")
            .join("Skyfold Studio");
        Self {
            dir,
            fps: 30,
            width: 1920,
            height: 1080,
            keep_frames: false,
        }
    }
}

/// Where ffmpeg is, if installed.
pub fn ffmpeg_path() -> Option<PathBuf> {
    for p in [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ] {
        if std::path::Path::new(p).is_file() {
            return Some(p.into());
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join("ffmpeg"))
            .find(|p| p.is_file())
    })
}

/// What the encoder thread reports.
#[derive(Clone, Debug, Default)]
pub struct CaptureStatus {
    pub written: usize,
    pub done: bool,
    pub error: Option<String>,
    pub output: Option<PathBuf>,
}

/// A capture in progress.
pub struct Capture {
    pub settings: CaptureSettings,
    pub total: usize,
    /// frames requested so far
    pub next: usize,
    tx: Option<SyncSender<(usize, Image)>>,
    inflight: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    pub status: Arc<Mutex<CaptureStatus>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

/// Saved editor state restored after playback.
struct Saved {
    orbit: (Vec3, f32, f32, f32),
    hour: f32,
    playing: bool,
    title: String,
}

/// Path playback (preview or capture).
pub struct Playback {
    pub path: u32,
    pub t: f32,
    pub capture: Option<Capture>,
    saved: Option<Saved>,
    start_hour: f32,
}

#[derive(Resource, Default)]
pub struct Flythrough {
    pub playing: Option<Playback>,
    /// a request to play (path id, capture), picked up next frame
    pub request: Option<(u32, Option<CaptureSettings>)>,
    /// result of the last capture (for the UI and MCP)
    pub last: Option<CaptureStatus>,
}

impl Flythrough {
    pub fn play(&mut self, path: u32, capture: Option<CaptureSettings>) {
        self.request = Some((path, capture));
    }
}

pub struct FlythroughPlugin;

impl Plugin for FlythroughPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Flythrough>().add_systems(
            Update,
            (
                playback
                    .after(crate::camera::CameraSystems)
                    .after(crate::walk::WalkSystems),
                draw_paths,
            ),
        );
    }
}

fn start_capture(settings: CaptureSettings, total: usize, name: &str) -> Result<Capture, String> {
    std::fs::create_dir_all(&settings.dir)
        .map_err(|e| format!("can't create {}: {e}", settings.dir.display()))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let base = format!("{safe}_{stamp}");
    let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, Image)>(4);
    let inflight = Arc::new(AtomicUsize::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(CaptureStatus::default()));
    let worker = {
        let (s, inflight, cancel, status) = (
            settings.clone(),
            inflight.clone(),
            cancel.clone(),
            status.clone(),
        );
        std::thread::Builder::new()
            .name("flythrough-encoder".into())
            .spawn(move || encode(rx, s, base, total, inflight, cancel, status))
            .map_err(|e| e.to_string())?
    };
    Ok(Capture {
        settings,
        total,
        next: 0,
        tx: Some(tx),
        inflight,
        cancel,
        status,
        worker: Some(worker),
    })
}

/// The encoder thread: frames in order into ffmpeg (scaled to the output size) and / or PNGs.
fn encode(
    rx: Receiver<(usize, Image)>,
    s: CaptureSettings,
    base: String,
    total: usize,
    inflight: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    let set_err = |e: String| {
        if let Ok(mut st) = status.lock() {
            st.error = Some(e);
        }
    };
    let ffmpeg = ffmpeg_path();
    let mp4 = s.dir.join(format!("{base}.mp4"));
    let frames_dir = s.dir.join(format!("{base}_frames"));
    let keep = s.keep_frames || ffmpeg.is_none();
    if keep && let Err(e) = std::fs::create_dir_all(&frames_dir) {
        set_err(format!("can't create {}: {e}", frames_dir.display()));
    }
    let mut child: Option<std::process::Child> = None;
    let mut pending: BTreeMap<usize, Image> = BTreeMap::new();
    let mut next = 0usize;
    let mut failed = false;
    while next < total && !cancel.load(Ordering::Relaxed) {
        // (wake up now and then to notice a cancel even if a frame never arrives)
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok((i, img)) => {
                pending.insert(i, img);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        while let Some(img) = pending.remove(&next) {
            inflight.fetch_sub(1, Ordering::Relaxed);
            let size = img.texture_descriptor.size;
            let (w, h) = (size.width, size.height);
            let bgra = matches!(
                img.texture_descriptor.format,
                bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb
                    | bevy::render::render_resource::TextureFormat::Bgra8Unorm
            );
            let Some(data) = img.data else {
                next += 1;
                continue;
            };
            if let Some(ff) = &ffmpeg
                && child.is_none()
                && !failed
            {
                let vf = format!(
                    "scale={0}:{1}:force_original_aspect_ratio=increase:flags=lanczos,crop={0}:{1},setsar=1",
                    s.width, s.height
                );
                let spawned = std::process::Command::new(ff)
                    .args([
                        "-loglevel",
                        "error",
                        "-y",
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        if bgra { "bgra" } else { "rgba" },
                        "-s",
                        &format!("{w}x{h}"),
                        "-r",
                        &s.fps.to_string(),
                        "-i",
                        "-",
                    ])
                    .args([
                        "-vf",
                        &vf,
                        "-c:v",
                        "libx264",
                        "-preset",
                        "medium",
                        "-crf",
                        "17",
                        "-pix_fmt",
                        "yuv420p",
                        "-movflags",
                        "+faststart",
                    ])
                    .arg(&mp4)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .spawn();
                match spawned {
                    Ok(c) => child = Some(c),
                    Err(e) => {
                        failed = true;
                        set_err(format!("ffmpeg didn't start: {e}"));
                    }
                }
            }
            if let Some(c) = &mut child
                && let Some(stdin) = c.stdin.as_mut()
                && let Err(e) = stdin.write_all(&data)
            {
                set_err(format!("ffmpeg stopped: {e}"));
                failed = true;
            }
            if keep {
                let mut rgba = data;
                if bgra {
                    for px in rgba.chunks_exact_mut(4) {
                        px.swap(0, 2);
                    }
                }
                if let Some(im) = image::RgbaImage::from_raw(w, h, rgba) {
                    let im = image::imageops::resize(
                        &image::DynamicImage::ImageRgba8(im).to_rgb8(),
                        s.width,
                        s.height,
                        image::imageops::FilterType::Triangle,
                    );
                    if let Err(e) = im.save(frames_dir.join(format!("frame_{:05}.png", next + 1))) {
                        set_err(format!("frame {}: {e}", next + 1));
                    }
                }
            }
            next += 1;
            if let Ok(mut st) = status.lock() {
                st.written = next;
            }
        }
    }
    let cancelled = cancel.load(Ordering::Relaxed);
    if let Some(mut c) = child {
        drop(c.stdin.take());
        if cancelled {
            let _ = c.kill();
            let _ = c.wait();
            let _ = std::fs::remove_file(&mp4);
        } else {
            match c.wait_with_output() {
                Ok(o) if o.status.success() => {}
                Ok(o) => set_err(format!(
                    "ffmpeg failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                        .lines()
                        .last()
                        .unwrap_or("?")
                )),
                Err(e) => set_err(format!("ffmpeg: {e}")),
            }
        }
    }
    if let Ok(mut st) = status.lock() {
        st.done = true;
        if !cancelled && st.error.is_none() {
            st.output = Some(if ffmpeg.is_some() { mp4 } else { frames_dir });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn playback(
    mut commands: Commands,
    mut fly: ResMut<Flythrough>,
    data: Res<MotionData>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut walk: ResMut<WalkState>,
    mut orbit: ResMut<OrbitCamera>,
    mut lighting: ResMut<crate::sky::Lighting>,
    mut cam: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut strategy: ResMut<TimeUpdateStrategy>,
    mut ed: ResMut<crate::editor::Editor>,
) {
    // start a requested playback (ending the one playing, if any)
    if fly.request.is_some() && fly.playing.is_some() {
        if let Some(c) = fly.playing.as_ref().and_then(|p| p.capture.as_ref()) {
            c.cancel.store(true, Ordering::Relaxed);
        }
        finish(
            &mut fly,
            &mut walk,
            &mut orbit,
            &mut lighting,
            &mut window,
            &mut strategy,
            &mut ed,
            &mut cam,
        );
    }
    if let Some((id, capture)) = fly.request.take() {
        let Some(path) = data.paths.iter().find(|p| p.id == id) else {
            ed.toast("That camera path no longer exists");
            return;
        };
        if path.keys.len() < 2 {
            ed.toast("A camera path needs at least two keys");
            return;
        }
        if walk.active {
            ed.toast("Leave walk mode to play a camera path");
            return;
        }
        let capture = match capture {
            Some(s) => {
                let frames = (path.duration() * s.fps as f32).ceil() as usize + 1;
                match start_capture(s, frames, &path.name) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        ed.toast(format!("Capture failed: {e}"));
                        return;
                    }
                }
            }
            None => None,
        };
        let title = window.single().map(|w| w.title.clone()).unwrap_or_default();
        let saved = Saved {
            orbit: (orbit.focus, orbit.yaw, orbit.pitch, orbit.distance),
            hour: lighting.hour,
            playing: lighting.playing,
            title,
        };
        fly.playing = Some(Playback {
            path: id,
            t: 0.0,
            capture,
            saved: Some(saved),
            start_hour: lighting.hour,
        });
        walk.cinematic = true;
        orbit.enabled = false;
        lighting.playing = false;
    }
    let Some(pb) = &mut fly.playing else { return };
    let Some(path) = data.paths.iter().find(|p| p.id == pb.path) else {
        return finish(
            &mut fly,
            &mut walk,
            &mut orbit,
            &mut lighting,
            &mut window,
            &mut strategy,
            &mut ed,
            &mut cam,
        );
    };
    let total = path.duration();
    let mut stop = keys.just_pressed(KeyCode::Escape);
    match &mut pb.capture {
        Some(c) => {
            if stop {
                c.cancel.store(true, Ordering::Relaxed);
            } else if c.next < c.total {
                // render the next frame if the encoder keeps up (else hold the clock still)
                if c.inflight.load(Ordering::Relaxed) < 6 {
                    pb.t = c.next as f32 / c.settings.fps as f32;
                    let idx = c.next;
                    c.next += 1;
                    c.inflight.fetch_add(1, Ordering::Relaxed);
                    let tx = c.tx.clone();
                    commands.spawn(Screenshot::primary_window()).observe(
                        move |shot: On<ScreenshotCaptured>| {
                            if let Some(tx) = &tx {
                                let mut msg = (idx, shot.image.clone());
                                // (blocks only if the encoder is far behind; it never is: see `inflight`)
                                loop {
                                    match tx.try_send(msg) {
                                        Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                                        Err(TrySendError::Full(m)) => {
                                            msg = m;
                                            std::thread::sleep(std::time::Duration::from_millis(2));
                                        }
                                    }
                                }
                            }
                        },
                    );
                    *strategy = TimeUpdateStrategy::ManualDuration(
                        std::time::Duration::from_secs_f64(1.0 / c.settings.fps as f64),
                    );
                } else {
                    *strategy = TimeUpdateStrategy::ManualDuration(std::time::Duration::ZERO);
                }
                let written = c.status.lock().map(|s| s.written).unwrap_or(0);
                if let Ok(mut w) = window.single_mut() {
                    w.title = format!(
                        "Skyfold Studio — capturing {} / {} frames (Esc cancels)",
                        written, c.total
                    );
                }
            } else {
                // all frames requested: wait for the encoder
                *strategy = TimeUpdateStrategy::ManualDuration(std::time::Duration::ZERO);
                drop(c.tx.take());
                if c.status.lock().is_ok_and(|s| s.done) {
                    stop = true;
                } else if let Ok(mut w) = window.single_mut() {
                    w.title = "Skyfold Studio — encoding video…".into();
                }
            }
            if c.cancel.load(Ordering::Relaxed) {
                drop(c.tx.take());
                stop = c.status.lock().is_ok_and(|s| s.done)
                    || c.worker.as_ref().is_some_and(|w| w.is_finished());
            }
        }
        None => {
            pb.t += time.delta_secs();
            if pb.t > total {
                if path.looped {
                    pb.t -= total.max(0.01);
                } else {
                    stop = true;
                }
            }
        }
    }
    if let Some((pos, yaw, pitch, fov)) = path.sample(pb.t)
        && let Ok((mut t, mut proj)) = cam.single_mut()
    {
        t.translation = pos;
        t.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        if let Projection::Perspective(p) = &mut *proj {
            p.fov = fov.to_radians();
        }
        // the sky, zone weather and streaming follow the camera
        orbit.focus = pos;
    }
    if path.timelapse_hours.abs() > 0.001 {
        lighting.hour = (pb.start_hour + path.timelapse_hours * (pb.t / total.max(0.01)).min(1.0))
            .rem_euclid(24.0);
    }
    if stop {
        finish(
            &mut fly,
            &mut walk,
            &mut orbit,
            &mut lighting,
            &mut window,
            &mut strategy,
            &mut ed,
            &mut cam,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    fly: &mut Flythrough,
    walk: &mut WalkState,
    orbit: &mut OrbitCamera,
    lighting: &mut crate::sky::Lighting,
    window: &mut Query<&mut Window, With<PrimaryWindow>>,
    strategy: &mut TimeUpdateStrategy,
    ed: &mut crate::editor::Editor,
    cam: &mut Query<(&mut Transform, &mut Projection), With<MainCamera>>,
) {
    let Some(mut pb) = fly.playing.take() else {
        return;
    };
    walk.cinematic = false;
    *strategy = TimeUpdateStrategy::Automatic;
    if let Some(s) = pb.saved.take() {
        orbit.enabled = true;
        orbit.set_view(s.orbit.0, s.orbit.1, s.orbit.2, s.orbit.3);
        orbit.snap();
        lighting.hour = s.hour;
        lighting.playing = s.playing;
        if let Ok(mut w) = window.single_mut() {
            w.title = s.title;
        }
    }
    if let Ok((_, mut proj)) = cam.single_mut()
        && let Projection::Perspective(p) = &mut *proj
    {
        p.fov = 50f32.to_radians();
    }
    if let Some(mut c) = pb.capture.take() {
        drop(c.tx.take());
        if let Some(w) = c.worker.take() {
            let _ = w.join();
        }
        let st = c.status.lock().map(|s| s.clone()).unwrap_or_default();
        match (&st.output, &st.error) {
            (Some(out), None) => {
                info!(
                    "FLYTHROUGH captured {} frames to {}",
                    st.written,
                    out.display()
                );
                ed.toast(format!(
                    "Saved {} ({} frames, {}×{} @ {} fps)",
                    out.display(),
                    st.written,
                    c.settings.width,
                    c.settings.height,
                    c.settings.fps
                ));
            }
            (_, Some(e)) => {
                warn!("FLYTHROUGH capture failed: {e}");
                ed.toast(format!("Capture failed: {e}"));
            }
            _ => ed.toast("Capture cancelled"),
        }
        fly.last = Some(st);
    }
}

/// The paths in the editor: the spline and a little camera at each key (while the motion window
/// is open on its paths tab).
fn draw_paths(
    data: Res<MotionData>,
    ui: Res<super::hud::MotionUi>,
    walk: Res<WalkState>,
    fly: Res<Flythrough>,
    mut gizmos: Gizmos,
) {
    if walk.active || fly.playing.is_some() || !ui.open || ui.tab != super::PanelTab::Paths {
        return;
    }
    for path in &data.paths {
        let sel = ui.path == Some(path.id);
        let col = if sel {
            Color::srgb(1.0, 0.78, 0.25)
        } else {
            Color::srgba(0.7, 0.8, 1.0, 0.6)
        };
        let total = path.duration();
        if path.keys.len() >= 2 {
            let n = ((total * 8.0) as usize).clamp(16, 2000);
            gizmos.linestrip(
                (0..=n).filter_map(|i| path.sample(total * i as f32 / n as f32).map(|s| s.0)),
                col,
            );
        }
        for (i, k) in path.keys.iter().enumerate() {
            let p = Vec3::from_array(k.pos);
            let rot = Quat::from_euler(EulerRot::YXZ, k.yaw, k.pitch, 0.0);
            let fwd = rot * Vec3::NEG_Z;
            let (r, u) = (rot * Vec3::X, rot * Vec3::Y);
            let s = 3.0;
            let c = p + fwd * s * 1.6;
            let corners = [
                c + (r + u * 0.6) * s,
                c + (-r + u * 0.6) * s,
                c + (-r - u * 0.6) * s,
                c + (r - u * 0.6) * s,
            ];
            let kc = if sel && ui.key == Some(i) {
                Color::WHITE
            } else {
                col
            };
            for q in corners {
                gizmos.line(p, q, kc);
            }
            gizmos.linestrip(
                corners.iter().copied().chain(std::iter::once(corners[0])),
                kc,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::Terrain;

    /// 1 km of ground with a 120 m hill in the middle.
    fn hill() -> Terrain {
        let mut t = Terrain::new(256, 4.0, 0.0);
        let n = t.n();
        for j in 0..n {
            for i in 0..n {
                let p = t.grid_to_world(i, j);
                let k = t.idx(i, j);
                t.heights[k] =
                    5.0 + 120.0 * (-(p.length_squared()) / (2.0 * 90.0f32.powi(2))).exp();
            }
        }
        t
    }

    fn fwd(k: &CameraKey) -> Vec3 {
        Quat::from_euler(EulerRot::YXZ, k.yaw, k.pitch, 0.0) * Vec3::NEG_Z
    }

    #[test]
    fn flights_clear_the_hill_and_look_where_they_go() {
        let t = hill();
        for style in FlightStyle::ALL {
            let route = Route {
                places: vec![[-400.0, -50.0], [0.0, 20.0], [380.0, -10.0], [300.0, 300.0]],
                style,
                speed: FlightSpeed::Normal,
            };
            let keys = flight_keys(&route, &t);
            assert!(keys.len() >= 10, "{style:?}: {} keys", keys.len());
            let path = CameraPath {
                id: 1,
                name: "t".into(),
                keys: keys.clone(),
                timelapse_hours: 0.0,
                looped: false,
                route: Some(route.clone()),
            };
            let alt = style.altitude();
            for k in &keys {
                let clear = k.pos[1] - t.height_at(k.pos[0], k.pos[2]);
                assert!(
                    clear >= alt - 0.5,
                    "{style:?}: key {clear:.1} m over the ground"
                );
            }
            // between keys too (the spline may swing a little low, never into the hill)
            let total = path.duration();
            let mut lowest = f32::INFINITY;
            for i in 0..=400 {
                let (p, ..) = path.sample(total * i as f32 / 400.0).unwrap();
                lowest = lowest.min(p.y - t.height_at(p.x, p.z));
            }
            assert!(lowest > alt * 0.6, "{style:?}: dips to {lowest:.1} m");
            // ahead-looking styles face along the flight
            if style != FlightStyle::Showcase {
                for w in keys.windows(2) {
                    let travel = (Vec3::from_array(w[1].pos) - Vec3::from_array(w[0].pos))
                        .with_y(0.0)
                        .normalize();
                    let d = fwd(&w[0]).with_y(0.0).normalize().dot(travel);
                    assert!(d > 0.6, "{style:?}: looks sideways ({d:.2})");
                }
            }
            // a sensible pace: tens of seconds for ~1.3 km
            assert!(total > 15.0 && total < 200.0, "{style:?}: {total:.0} s");
        }
    }

    #[test]
    fn one_place_circles_it() {
        let t = hill();
        let route = Route {
            places: vec![[0.0, 0.0]],
            style: FlightStyle::Aerial,
            speed: FlightSpeed::Fast,
        };
        let keys = flight_keys(&route, &t);
        assert_eq!(keys.len(), 17);
        assert!(
            Vec3::from_array(keys[0].pos).distance(Vec3::from_array(keys[16].pos)) < 0.01,
            "closes the loop"
        );
        for k in &keys {
            let to_centre =
                (Vec3::new(0.0, t.height_at(0.0, 0.0), 0.0) - Vec3::from_array(k.pos)).normalize();
            assert!(fwd(k).dot(to_centre) > 0.99, "looks at the place");
            assert!(k.pos[1] > 125.0, "above the hilltop");
        }
        // duplicate clicks collapse to one place
        let twice = Route {
            places: vec![[10.0, 10.0], [10.5, 10.2]],
            ..route
        };
        assert_eq!(flight_keys(&twice, &t).len(), 17);
        assert!(
            flight_keys(
                &Route {
                    places: vec![],
                    ..twice
                },
                &t
            )
            .is_empty()
        );
    }
}
