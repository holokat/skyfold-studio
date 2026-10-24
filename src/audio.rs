//! Sound: automatic footstep-surface classification and playback, zone ambience beds with
//! day/night layers and weather. The wind, rain, running water, surf, fire, cave drips and the sea
//! heard from under water are synthesised live (`synth`), following the weather; the other beds and
//! the footsteps are recordings in `assets/audio` (every take of a surface at the same loudness, so
//! the level of each surface and gait is set here).

mod synth;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::audio::{AddAudioSource, AudioSink, AudioSinkPlayback, Volume};
use bevy::prelude::*;

pub use synth::{Synth, Voice};

use crate::objects::{Library, ObjectStore};
use crate::sky::SkyState;
use crate::terrain::Terrain;
use crate::terrain::layers::{Layer, Surface};
use crate::water::WaterBodies;
use crate::world::{Soundscape, Zones};

#[derive(Resource, Clone)]
pub struct AudioSettings {
    pub master: f32,
    pub ambience: f32,
    pub footsteps: f32,
    /// Play zone ambience while editing (follows the camera focus).
    pub preview_in_editor: bool,
    pub muted: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        // (not saved between sessions: every launch starts here) footsteps sit under the world
        Self {
            master: 0.8,
            ambience: 0.55,
            footsteps: 0.7,
            preview_in_editor: true,
            muted: false,
        }
    }
}

#[derive(Resource, Default)]
pub struct AudioLib {
    steps: HashMap<Surface, Vec<Handle<AudioSource>>>,
    loops: HashMap<String, Handle<AudioSource>>,
    /// the synthesised voices, by the stem they play as
    synths: HashMap<&'static str, Handle<Synth>>,
    /// the weather they follow (set by `update_ambience`)
    controls: Arc<synth::Controls>,
    pub ui: HashMap<String, Handle<AudioSource>>,
}

impl AudioLib {
    pub fn synth(&self, voice: Voice) -> Option<Handle<Synth>> {
        self.synths.get(voice.stem()).cloned()
    }
}

/// Play a one-shot footstep / UI sound.
#[derive(Message, Clone, Debug)]
pub enum SoundEvent {
    /// One footstep at full stride (the editor's "Hear it" previews).
    Footstep(Surface),
    /// A footfall while walking: how loud it is depends on the gait.
    Step(Surface, StepKind),
    Ui(&'static str),
}

/// How a footfall lands: sets its level (and the landing's heavier, lower sound).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Crouch,
    Walk,
    Run,
    Sprint,
    /// A hoof beat while riding.
    Hoof,
    /// Landing from a fall or jump.
    Land,
}

impl StepKind {
    /// Level relative to a running step (dB).
    pub fn gain_db(self) -> f32 {
        match self {
            StepKind::Crouch => -10.0,
            StepKind::Walk => -3.0,
            StepKind::Run => 0.0,
            StepKind::Sprint => 1.5,
            StepKind::Hoof => 1.0,
            StepKind::Land => 3.0,
        }
    }

    /// Playback speed: a landing (and a horse) is a little heavier and lower.
    fn speed(self) -> f32 {
        match self {
            StepKind::Land => 0.94,
            StepKind::Hoof => 0.92,
            _ => 1.0,
        }
    }
}

/// How loud each ground is underfoot (dB, solid stone = 0): soft ground is clearly quieter than
/// stone, wood and metal. The takes themselves are all equally loud.
pub fn surface_gain_db(s: Surface) -> f32 {
    match s {
        Surface::Stone => 0.0,
        Surface::Marble => 0.0,
        Surface::Metal => 1.0,
        Surface::WetStone => -1.0,
        Surface::Cobble => -1.0,
        Surface::Wood => -1.0,
        Surface::Crystal => -1.0,
        Surface::Ice => -2.0,
        Surface::Bone => -2.0,
        Surface::Gravel => -2.0,
        Surface::Water => -2.0,
        Surface::Cinder => -3.0,
        Surface::Salt => -4.0,
        Surface::Mud => -4.0,
        Surface::Dirt => -5.0,
        Surface::Leaves => -5.0,
        Surface::Snow => -6.0,
        Surface::Grass => -7.0,
        Surface::Loam => -8.0,
        Surface::Sand => -8.0,
        Surface::Ash => -8.0,
        Surface::Carpet => -11.0,
    }
}

/// Footsteps against the rest of the mix (dB): the takes are mastered quiet (peaks intact), so a
/// running step on stone lands a little above the ambience and soft ground well under it.
const FOOTSTEP_MIX_DB: f32 = 6.0;
/// Pitch and level variation per footfall (±2.5 %, ±1.5 dB): enough to never sound looped.
const STEP_PITCH: f32 = 0.025;
const STEP_JITTER_DB: f32 = 1.5;

/// The takes a surface played last: per foot, and the latest of all.
#[derive(Clone, Copy, Default)]
struct LastTakes {
    foot: [Option<usize>; 2],
    latest: Option<usize>,
}

/// The take for the next footfall of `foot` (0 / 1) from `n` takes (`r` in 0..1): even takes are one
/// foot and odd takes the other (alternate steps of the same walk), never the take just played,
/// nor the one this foot played last.
fn pick_take(n: usize, foot: usize, last: LastTakes, r: f32) -> usize {
    let fresh = |i: &usize| Some(*i) != last.latest && (n < 4 || Some(*i) != last.foot[foot]);
    let pool: Vec<usize> = if n >= 4 {
        (0..n).filter(|i| i % 2 == foot).filter(fresh).collect()
    } else {
        (0..n).filter(fresh).collect()
    };
    if pool.is_empty() {
        return 0;
    }
    pool[((r * pool.len() as f32) as usize).min(pool.len() - 1)]
}

/// Which takes each surface played last, and which foot falls next.
#[derive(Default)]
struct StepMemory {
    last: HashMap<Surface, LastTakes>,
    foot: usize,
}

#[derive(Component)]
struct AmbienceLayer {
    stem: String,
    target: f32,
    current: f32,
}

/// What the ambience system is currently playing (for the UI).
#[derive(Resource, Default, Clone)]
pub struct AmbienceState {
    pub soundscape: Option<Soundscape>,
    pub auto: bool,
    /// Preview a soundscape regardless of zones (Audio / Zones tab button).
    pub preview: Option<(Soundscape, f32)>,
}

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioSettings>()
            .init_resource::<AudioLib>()
            .init_resource::<AmbienceState>()
            .add_message::<SoundEvent>()
            .add_audio_source::<Synth>()
            .add_systems(Startup, load_audio)
            .add_systems(Update, (play_one_shots, update_ambience));
    }
}

fn load_audio(
    mut lib: ResMut<AudioLib>,
    server: Res<AssetServer>,
    mut synths: ResMut<Assets<Synth>>,
) {
    for voice in Voice::ALL {
        let h = synths.add(Synth {
            voice,
            controls: lib.controls.clone(),
        });
        lib.synths.insert(voice.stem(), h);
    }
    let dir = std::path::PathBuf::from(crate::assets_path()).join("audio/steps");
    for s in Surface::ALL {
        // as many takes as there are files (step_<stem>_0, _1, ...); the usual count if none are found
        let on_disk = (0..32)
            .take_while(|i| {
                dir.join(format!("step_{}_{i}.mp3", s.sound_stem()))
                    .is_file()
            })
            .count();
        let n = if on_disk > 0 {
            on_disk
        } else {
            s.variant_count() as usize
        };
        let v = (0..n)
            .map(|i| server.load(format!("audio/steps/step_{}_{}.mp3", s.sound_stem(), i)))
            .collect();
        lib.steps.insert(s, v);
    }
    let mut stems: Vec<&str> = vec!["amb_storm_0", WIND_LOOP, GUST_LOOP];
    stems.extend(crate::visuals::SOUND_STEMS);
    stems.extend(crate::auras::sound_stems());
    for sc in Soundscape::ALL {
        for night in [false, true] {
            for (stem, _) in sc.layers(night) {
                stems.push(stem);
            }
        }
    }
    stems.sort();
    stems.dedup();
    // (the synthesised voices' loop files are only for exports)
    stems.retain(|s| Voice::from_stem(s).is_none());
    for stem in stems {
        lib.loops.insert(
            stem.to_string(),
            server.load(format!("audio/ambience/{stem}.mp3")),
        );
    }
    for ui in [
        "zone_discovered_0",
        "ui_click_0",
        "landmark_discovered_0",
    ] {
        lib.ui
            .insert(ui.to_string(), server.load(format!("audio/ui/{ui}.mp3")));
    }
}

fn master(settings: &AudioSettings) -> f32 {
    if settings.muted { 0.0 } else { settings.master }
}

/// The wind loops, played from the wind actually blowing (`Soundscape::wind_exposure`).
const WIND_LOOP: &str = "amb_wind_0";
const GUST_LOOP: &str = "amb_near_gusts_0";

/// How loud the wind loop plays for the wind heard (m/s): silent in still air, a soft hiss in a
/// light breeze (0.4 at 2.75 m/s), full from a strong breeze up (the cap keeps a gale on a ridge,
/// gusts and all, from clipping).
fn wind_loudness(speed: f32) -> f32 {
    crate::noise::smoothstep(1.0, 3.0, speed) * (speed.max(0.0) / 8.0).powf(0.8).min(1.1)
}

/// The wind and gust loop levels (before the ambience volume) for the wind heard (m/s, already
/// times the place's exposure), the place's own gust share and the world's gustiness (0..1).
/// Gusts join everywhere once the wind gets strong.
fn wind_mix(heard: f32, gusts: f32, gustiness: f32) -> (f32, f32) {
    let w = wind_loudness(heard);
    let share = (gusts + 0.7 * crate::noise::smoothstep(7.0, 15.0, heard)).min(0.9);
    (w, w * share * (0.5 + gustiness.clamp(0.0, 1.0)))
}

/// Linear volume of one footfall (before the per-step variation).
fn step_volume(settings: &AudioSettings, s: Surface, kind: StepKind) -> f32 {
    master(settings)
        * settings.footsteps
        * 10f32.powf((FOOTSTEP_MIX_DB + surface_gain_db(s) + kind.gain_db()) / 20.0)
}

fn play_one_shots(
    mut commands: Commands,
    mut events: MessageReader<SoundEvent>,
    lib: Res<AudioLib>,
    settings: Res<AudioSettings>,
    mut memory: Local<StepMemory>,
) {
    for e in events.read() {
        match *e {
            SoundEvent::Footstep(s) | SoundEvent::Step(s, _) => {
                let kind = if let SoundEvent::Step(_, k) = *e {
                    k
                } else {
                    StepKind::Run
                };
                let Some(v) = lib.steps.get(&s) else { continue };
                if v.is_empty() {
                    continue;
                }
                // feet alternate (a landing is both feet: the next step starts over)
                let foot = memory.foot;
                memory.foot = if kind == StepKind::Land { 0 } else { 1 - foot };
                let last = memory.last.entry(s).or_default();
                let i = pick_take(v.len(), foot, *last, fastrand::f32());
                last.foot[foot] = Some(i);
                last.latest = Some(i);
                let jitter = 10f32.powf((fastrand::f32() * 2.0 - 1.0) * STEP_JITTER_DB / 20.0);
                let speed = kind.speed() * (1.0 + (fastrand::f32() * 2.0 - 1.0) * STEP_PITCH);
                commands.spawn((
                    AudioPlayer(v[i].clone()),
                    PlaybackSettings::DESPAWN
                        .with_volume(Volume::Linear(step_volume(&settings, s, kind) * jitter))
                        .with_speed(speed),
                ));
            }
            SoundEvent::Ui(name) => {
                if let Some(h) = lib.ui.get(name) {
                    commands.spawn((
                        AudioPlayer(h.clone()),
                        PlaybackSettings::DESPAWN
                            .with_volume(Volume::Linear(master(&settings) * 0.7)),
                    ));
                }
            }
        }
    }
}

/// Picks a soundscape for places that aren't inside a zone with ambience rules.
pub fn auto_soundscape(
    terrain: &Terrain,
    store: &ObjectStore,
    lib: Option<&Library>,
    nature: &crate::nature::render::Near,
    p: Vec3,
) -> Soundscape {
    let alt = terrain.height_at(p.x, p.z) - terrain.sea_level;
    if alt > terrain.style.snow_line {
        return Soundscape::Frost;
    }
    if alt > terrain.style.snow_line * 0.65 {
        return Soundscape::Mountain;
    }
    // near the ocean?
    let mut coast = alt < 6.0;
    if !coast {
        for a in 0..8 {
            let ang = a as f32 / 8.0 * std::f32::consts::TAU;
            let q = p + Vec3::new(ang.cos(), 0.0, ang.sin()) * 70.0;
            if terrain.in_bounds(q.x, q.z) && terrain.height_at(q.x, q.z) < terrain.sea_level {
                coast = true;
                break;
            }
        }
    }
    if coast {
        return Soundscape::Coast;
    }
    if let Some(lib) = lib {
        let trees = store
            .query(p.xz(), 45.0)
            .iter()
            .filter(|id| {
                store
                    .items
                    .get(id)
                    .and_then(|o| lib.get(&o.asset))
                    .is_some_and(|e| e.category == crate::objects::Category::Trees)
            })
            .count();
        // procedural trees near the viewer
        let mut near = vec![];
        nature.query(p.xz(), 45.0, &mut near);
        let trees = trees + near.iter().filter(|i| i.cat == 0).count();
        if trees >= 5 {
            return Soundscape::Forest;
        }
    }
    let layer = terrain.dominant_layer_at(p.x, p.z);
    match layer {
        Layer::Sand => Soundscape::Desert,
        Layer::Mud => Soundscape::Marsh,
        Layer::DarkRock => Soundscape::Volcanic,
        Layer::Snow | Layer::Ice => Soundscape::Frost,
        _ => Soundscape::Meadow,
    }
}

#[allow(clippy::too_many_arguments)]
fn update_ambience(
    mut commands: Commands,
    settings: Res<AudioSettings>,
    lib: Res<AudioLib>,
    sky: Res<SkyState>,
    zones: Res<Zones>,
    terrain: Res<Terrain>,
    store: Res<ObjectStore>,
    objects: Option<Res<Library>>,
    nature: Res<crate::nature::render::Near>,
    walk: Res<crate::walk::WalkState>,
    mut state: ResMut<AmbienceState>,
    mut layers: Query<(Entity, &mut AmbienceLayer, Option<&mut AudioSink>)>,
    time: Res<Time>,
    mut timer: Local<f32>,
    mut cached: Local<Option<Soundscape>>,
    (water, auras, wind, indoors): (
        Res<WaterBodies>,
        Res<crate::auras::Auras>,
        Res<crate::wind::WindState>,
        Option<Res<crate::motion::indoors::Indoors>>,
    ),
) {
    let night = sky.night > 0.5;
    let active = walk.active || settings.preview_in_editor || state.preview.is_some();
    // re-evaluate the automatic soundscape twice per second
    *timer -= time.delta_secs();
    if *timer <= 0.0 {
        *timer = 0.5;
        *cached = Some(auto_soundscape(
            &terrain,
            &store,
            objects.as_deref(),
            &nature,
            sky.viewer_pos,
        ));
    }
    let mut zone_vol = 1.0;
    let (scape, auto) = if let Some((s, v)) = state.preview {
        zone_vol = v;
        (s, false)
    } else if let Some(z) = zones.by_id(sky.viewer_zone).filter(|z| z.audio_enabled) {
        zone_vol = z.audio_volume;
        (z.soundscape, false)
    } else if let Some(b) =
        crate::biomes::BiomePreset::from_id(terrain.biome_at(sky.viewer_pos.x, sky.viewer_pos.z))
    {
        (b.def().soundscape, false)
    } else {
        (cached.unwrap_or(Soundscape::Meadow), true)
    };
    state.soundscape = Some(scape);
    state.auto = auto;

    let mut want: HashMap<String, f32> = HashMap::new();
    if active {
        // (under water the world above is muffled: motion::sfx plays the sea heard from below)
        let base = master(&settings)
            * settings.ambience
            * zone_vol
            * if walk.active { 1.0 } else { 0.6 }
            * (1.0 - 0.85 * walk.submerged);
        // rain / storm drown out birds
        let wet = (sky.weather.rain).clamp(0.0, 1.0);
        for (stem, v) in scape.layers(night) {
            *want.entry(stem.to_string()).or_default() += v * base * (1.0 - wet * 0.6);
        }
        if wet > 0.02 {
            *want.entry("amb_storm_0".to_string()).or_default() += wet * base;
        }
        // the wind blowing here (zone overrides and storms included, gusts swelling and fading),
        // as much of it as the place is exposed to; walls and cave rock keep it out
        let (exposure, gusts) = scape.wind_exposure();
        let (inside, cave) = indoors
            .as_ref()
            .map(|i| (i.amount, i.cave))
            .unwrap_or((0.0, 0.0));
        let shelter = (1.0 - 0.8 * inside) * (1.0 - cave);
        let heard = wind.speed * wind.gust(time.elapsed_secs()) * exposure;
        let (w, g) = wind_mix(heard, gusts, wind.gustiness);
        // a blizzard howls (dust storms already blow harder: see wind::WindState)
        let blizzard = if exposure > 0.0 && sky.weather.snow > 0.2 {
            0.5 * sky.weather.snow.min(1.0)
        } else {
            0.0
        };
        *want.entry(WIND_LOOP.to_string()).or_default() += (w + blizzard) * base * shelter;
        *want.entry(GUST_LOOP.to_string()).or_default() += g * base * shelter;
        // the synthesised wind brightens as it blows harder; the rain thickens
        lib.controls.set((heard / 14.0).max(blizzard), wet);
        // waterfalls, rivers and lava nearby
        for (stem, v) in crate::visuals::near_water_sounds(&water, sky.viewer_pos) {
            *want.entry(stem.to_string()).or_default() += v * base;
        }
        // auras round models nearby (a dark spire's drone and whispers), louder as you approach
        for &(stem, v) in auras.sounds() {
            *want.entry(stem.to_string()).or_default() += v * base;
        }
    }

    let dt = time.delta_secs();
    let mut existing = vec![];
    for (e, mut l, sink) in &mut layers {
        l.target = want.get(&l.stem).copied().unwrap_or(0.0);
        let rate = 0.6 * dt;
        l.current += (l.target - l.current).clamp(-rate, rate);
        if let Some(mut s) = sink {
            s.set_volume(Volume::Linear(l.current.max(0.0)));
        }
        if l.target <= 0.0 && l.current <= 0.001 {
            commands.entity(e).despawn();
        }
        existing.push(l.stem.clone());
    }
    for (stem, v) in want {
        if v <= 0.001 || existing.contains(&stem) {
            continue;
        }
        let name = Name::new(format!("Ambience {stem}"));
        let silent = Volume::Linear(0.0);
        let layer = AmbienceLayer {
            stem: stem.clone(),
            target: v,
            current: 0.0,
        };
        if let Some(h) = lib.synths.get(stem.as_str()) {
            // an endless stream: played once, it never ends
            commands.spawn((
                name,
                layer,
                AudioPlayer(h.clone()),
                PlaybackSettings::ONCE.with_volume(silent),
            ));
        } else if let Some(h) = lib.loops.get(&stem) {
            commands.spawn((
                name,
                layer,
                AudioPlayer(h.clone()),
                PlaybackSettings::LOOP.with_volume(silent),
            ));
        }
    }
}

/// Classify what the ground sounds like at `p` (the feet of a walker).
pub fn classify_surface(
    p: Vec3,
    terrain: &Terrain,
    water: &WaterBodies,
    layer_surface: &[Surface; 12],
    standing_on_object: Option<Surface>,
) -> Option<Surface> {
    if let Some(s) = standing_on_object {
        return Some(s);
    }
    let ground = terrain.height_at(p.x, p.z);
    let water_level = water
        .water_level(Vec3::new(p.x, ground, p.z))
        .unwrap_or(f32::NEG_INFINITY)
        .max(terrain.sea_level);
    let depth = water_level - ground;
    if depth > 1.3 {
        return None; // swimming: no footsteps
    }
    if depth > 0.04 {
        return Some(Surface::Water);
    }
    let layer = terrain.dominant_layer_at(p.x, p.z);
    let mut s = layer_surface[layer as usize];
    // stone right by the water line is wet
    if s == Surface::Stone && depth > -0.6 {
        s = Surface::WetStone;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footstep_takes_alternate_feet_and_never_repeat() {
        for n in 1..=8 {
            let mut last = LastTakes::default();
            let mut foot = 0;
            for k in 0..400 {
                let i = pick_take(n, foot, last, fastrand::f32());
                assert!(i < n);
                if n >= 2 {
                    assert_ne!(Some(i), last.latest, "{n} takes: take {i} twice in a row");
                }
                if n >= 4 {
                    assert_eq!(
                        i % 2,
                        foot,
                        "{n} takes: even takes are one foot, odd the other"
                    );
                    assert_ne!(
                        Some(i),
                        last.foot[foot],
                        "{n} takes: same take on the same foot"
                    );
                }
                last.foot[foot] = Some(i);
                last.latest = Some(i);
                // feet alternate; now and then a landing, after which the next step is foot 0 again
                foot = if k % 37 == 5 { 0 } else { 1 - foot };
            }
        }
    }

    #[test]
    fn soft_ground_is_quieter_than_hard_ground() {
        let soft = [
            Surface::Grass,
            Surface::Sand,
            Surface::Snow,
            Surface::Carpet,
            Surface::Loam,
            Surface::Leaves,
        ];
        let hard = [
            Surface::Stone,
            Surface::Wood,
            Surface::Metal,
            Surface::Marble,
        ];
        for s in soft {
            for h in hard {
                assert!(
                    surface_gain_db(s) <= surface_gain_db(h) - 4.0,
                    "{s:?} vs {h:?}"
                );
            }
        }
        // crouching is much quieter than walking, walking than running, sprinting and landing louder
        assert!(StepKind::Crouch.gain_db() <= StepKind::Walk.gain_db() - 6.0);
        assert!(StepKind::Walk.gain_db() < StepKind::Run.gain_db());
        assert!(
            StepKind::Sprint.gain_db() > StepKind::Run.gain_db()
                && StepKind::Land.gain_db() > StepKind::Run.gain_db()
        );
    }

    #[test]
    fn wind_heard_follows_the_wind_speed() {
        // still air is silent, and it only gets louder from there
        assert_eq!(wind_mix(0.0, 0.6, 0.5), (0.0, 0.0));
        let mut last = (0.0, 0.0);
        for i in 1..=60 {
            let m = wind_mix(i as f32 * 0.5, 0.0, 0.5);
            assert!(m.0 >= last.0 && m.1 >= last.1, "{} m/s", i as f32 * 0.5);
            last = m;
        }
        // a gale is far louder than a breeze, and brings gusts even to a meadow
        let (breeze, gale) = (wind_mix(2.5, 0.0, 0.5), wind_mix(17.0, 0.0, 0.5));
        assert!(gale.0 > 2.5 * breeze.0 && breeze.1 == 0.0 && gale.1 > 0.5);
        // capped, however hard it blows or gusts
        let (w, g) = wind_mix(100.0, 0.8, 1.0);
        assert!(w <= 1.1 && g <= 1.1 * 0.9 * 1.5);

        // the world's default breeze (2.5 m/s, gustiness 0.5) sounds as the fixed mix used to
        let at = |s: Soundscape| {
            let (e, gusts) = s.wind_exposure();
            wind_mix(2.5 * e, gusts, 0.5)
        };
        let near = |a: f32, b: f32| (a - b).abs() < 0.05;
        let (m, d, k) = (
            at(Soundscape::Meadow),
            at(Soundscape::Mountain),
            at(Soundscape::Desert),
        );
        assert!(
            near(m.0, 0.4)
                && near(d.0, 0.8)
                && near(d.1, 0.5)
                && near(k.0, 0.55)
                && near(k.1, 0.35),
            "{m:?} {d:?} {k:?}"
        );
        // sheltered places hear next to none of it, caves none at all
        assert!(
            at(Soundscape::Forest).0 < 0.02
                && at(Soundscape::Town).0 < 0.02
                && at(Soundscape::Cave) == (0.0, 0.0)
        );
    }
}
