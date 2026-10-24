//! The sound of a dungeon (from the dungeon sound library, `audio/dungeon/sounds.json`): the
//! mood's ambience bed and layer; the nearest fires, crystals, lava and portals heard where they
//! are (louder as you come close); and now and then a sound of the place's theme from somewhere
//! off in the dark (bones settling, a far roar, a drip). The world's own ambience is hushed
//! while you are underground. Plays while walking (and in the editor when Audio › preview is on).

use std::collections::HashMap;

use bevy::audio::{PlaybackMode, Volume};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::lights::LightKind;
use super::model::*;
use crate::camera::MainCamera;

#[derive(Clone, Debug, Deserialize)]
pub struct SoundEntry {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub file_lufs: Vec<f32>,
    #[serde(default)]
    pub radius_m: Option<f32>,
    #[serde(default)]
    pub interval_seconds: Option<serde_json::Value>,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Theme {
    #[serde(default)]
    pub beds: Vec<String>,
    #[serde(default)]
    pub loops: Vec<String>,
    #[serde(default)]
    pub emitters: Vec<String>,
    #[serde(default)]
    pub step_surfaces: Vec<String>,
}

/// The dungeon sound library.
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub sounds: HashMap<String, SoundEntry>,
    pub themes: HashMap<String, Theme>,
}

impl Library {
    pub fn load() -> Library {
        #[derive(Deserialize)]
        struct Doc {
            sounds: Vec<SoundEntry>,
            #[serde(default)]
            themes: HashMap<String, Theme>,
        }
        let path = std::path::PathBuf::from(crate::assets_path()).join("audio/dungeon/sounds.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Library::default();
        };
        match serde_json::from_str::<Doc>(&text) {
            Ok(d) => Library {
                sounds: d.sounds.into_iter().map(|s| (s.id.clone(), s)).collect(),
                themes: d.themes,
            },
            Err(e) => {
                warn!("dungeon sounds: {e}");
                Library::default()
            }
        }
    }
    /// A take's file and its gain to bring it to a common loudness (linear).
    pub fn take(&self, id: &str, k: usize) -> Option<(String, f32)> {
        let s = self.sounds.get(id)?;
        if s.files.is_empty() {
            return None;
        }
        let i = k % s.files.len();
        let target = match s.kind.as_str() {
            "bed" => -24.0,
            "loop" => -21.0,
            _ => -18.0,
        };
        let lufs = s.file_lufs.get(i).copied().unwrap_or(target);
        // (quiet peaky takes are lifted, but never more than 9 dB)
        let gain = 10f32.powf(((target - lufs).clamp(-12.0, 9.0)) / 20.0);
        Some((s.files[i].clone(), gain))
    }
}

/// A sound heard where it is: (id of the sound, position).
fn emitters(map: &MapDoc, portals: &super::portal::Portals) -> Vec<(String, Vec3, u64)> {
    let mut out = Vec::new();
    for l in &map.lights {
        let id = match l.kind {
            LightKind::Torch => "loop_torch",
            LightKind::Brazier => "loop_brazier",
            LightKind::HangingBrazier => "loop_fire_big",
            LightKind::Candles => "loop_candles",
            LightKind::Crystal => "loop_crystal_hum",
            _ => continue,
        };
        out.push((id.to_string(), l.position(), l.id as u64));
    }
    for r in &map.plan.rooms {
        let id = match r.liquid {
            super::plan::Liquid::Lava => "loop_lava",
            super::plan::Liquid::Water if r.kind == super::plan::RoomKind::Pool => "loop_fountain",
            super::plan::Liquid::Slime | super::plan::Liquid::Blood => "loop_acid_pool",
            super::plan::Liquid::Arcane => "loop_crystal_hum",
            _ => continue,
        };
        let c = r.shape.centre();
        out.push((
            id.to_string(),
            Vec3::new(c.x, map.plan.level_y(r.level), c.y),
            1_000_000 + r.id as u64,
        ));
    }
    for (i, w) in portals.placed.iter().enumerate() {
        out.push((
            portal_loop(w.color).to_string(),
            w.tf.translation + Vec3::Y * 1.5,
            2_000_000 + i as u64,
        ));
    }
    out
}

/// A portal's hum by its colour.
fn portal_loop(c: PortalColor) -> &'static str {
    match c {
        PortalColor::Void | PortalColor::Blood => "loop_portal_void",
        PortalColor::Fel | PortalColor::Tide => "loop_portal_fel",
        PortalColor::Fire => "loop_portal_fire",
        PortalColor::Holy => "loop_portal_holy",
        PortalColor::Frost => "loop_portal_frost",
        PortalColor::Arcane => "loop_portal_arcane",
    }
}

#[derive(Component)]
pub struct DungeonAudio {
    key: u64,
    gain: f32,
    target: f32,
    current: f32,
}

/// A dungeon's own music or ambience track (Look › Mood › Music, MCP `dungeons_music`): a sound
/// file kept in the dungeon's folder, playing on a loop over the mood's ambience wherever you
/// are in it (and in the export). Or one of the demo tracks that ship with the app (`demo`): a
/// new dungeon starts with the one that suits its layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Music {
    /// the file in the dungeon's folder ("" = none; it wins over the demo track)
    pub file: String,
    /// a demo track (its id in `audio/music/music.json`; "" = none)
    pub demo: String,
    pub volume: f32,
    /// how far the mood's ambience bed is turned down under it (0..1)
    pub hush: f32,
}

impl Default for Music {
    fn default() -> Self {
        Self {
            file: String::new(),
            demo: String::new(),
            volume: 0.7,
            hush: 0.5,
        }
    }
}

impl Music {
    /// The sound file that plays: the dungeon's own (in its folder), else the demo track's.
    pub fn path(&self, folder: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
        if !self.file.is_empty() {
            return folder.map(|f| f.join(&self.file));
        }
        demo_track(&self.demo).map(|t| std::path::PathBuf::from(crate::assets_path()).join(&t.file))
    }
}

/// A demo track that ships with the app (`audio/music/music.json`, CC0 or Skyfold's own:
/// tools/audio/open/build_music.py): one per layout and realm (`kind` "layout") and one per mood
/// ("mood"); `suits` names which.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DemoTrack {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub suits: String,
    /// relative to the assets folder
    pub file: String,
    #[serde(default)]
    pub seconds: f32,
    #[serde(default)]
    pub description: String,
}

/// The demo tracks (the manifest ships with the app: read once).
pub fn demo_tracks() -> &'static [DemoTrack] {
    static TRACKS: std::sync::OnceLock<Vec<DemoTrack>> = std::sync::OnceLock::new();
    TRACKS.get_or_init(|| {
        #[derive(Deserialize)]
        struct Doc {
            tracks: Vec<DemoTrack>,
        }
        let path = std::path::PathBuf::from(crate::assets_path()).join("audio/music/music.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return vec![];
        };
        serde_json::from_str::<Doc>(&text)
            .map(|d| d.tracks)
            .unwrap_or_else(|e| {
                warn!("demo music: {e}");
                vec![]
            })
    })
}

pub fn demo_track(id: &str) -> Option<&'static DemoTrack> {
    demo_tracks().iter().find(|t| t.id == id)
}

/// The demo track a new dungeon starts with: its layout's (or realm's), else its mood's ("" = none).
pub fn demo_for(layout: &str, mood: &str) -> String {
    let all = demo_tracks();
    all.iter()
        .find(|t| t.kind == "layout" && t.suits == layout)
        .or_else(|| all.iter().find(|t| t.kind == "mood" && t.suits == mood))
        .map(|t| t.id.clone())
        .unwrap_or_default()
}

/// The kinds of sound file a track can be (what the audio decoder reads).
pub const MUSIC_EXTS: [&str; 3] = ["mp3", "ogg", "oga"];

/// Copies `src` into the dungeon's folder as its track (replacing an earlier one there);
/// returns the file's name in the folder.
pub fn import_music(
    folder: &std::path::Path,
    src: &std::path::Path,
    old: &str,
) -> Result<String, String> {
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !MUSIC_EXTS.contains(&ext.as_str()) {
        return Err(format!("{} is not an MP3 or Ogg file", src.display()));
    }
    let stem: String = src
        .file_stem()
        .map(|s| {
            s.to_string_lossy()
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .take(60)
                .collect()
        })
        .unwrap_or_default();
    let name = format!("music_{stem}.{ext}");
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let dst = folder.join(&name);
    if dst != src {
        std::fs::copy(src, &dst).map_err(|e| format!("can't copy {}: {e}", src.display()))?;
    }
    if !old.is_empty() && old != name {
        let _ = std::fs::remove_file(folder.join(old));
    }
    Ok(name)
}

#[derive(Resource, Default)]
pub struct DungeonSound {
    lib: Option<Library>,
    /// the track's file and its loaded sound
    music: Option<(std::path::PathBuf, Handle<AudioSource>)>,
    /// we hushed the world's ambience
    hushed: bool,
    next_emitter: f32,
    pub playing: Vec<String>,
}

impl DungeonSound {
    pub fn library(&mut self) -> &Library {
        self.lib.get_or_insert_with(Library::load)
    }
}

const BED: u64 = u64::MAX;
const LAYER: u64 = u64::MAX - 1;
/// the track's keys (with its file's hash: a new file fades the old one out)
const MUSIC: u64 = 0x7e00_0000_0000_0000;

#[allow(clippy::too_many_arguments)]
pub(super) fn ambience(
    mut commands: Commands,
    mut ds: ResMut<DungeonSound>,
    map: Res<DungeonMap>,
    portals: Res<super::portal::Portals>,
    settings: Res<crate::audio::AudioSettings>,
    mut amb: ResMut<crate::audio::AmbienceState>,
    walk: Res<crate::walk::WalkState>,
    server: Res<AssetServer>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut playing: Query<(Entity, &mut DungeonAudio, Option<&mut AudioSink>)>,
    time: Res<Time<Real>>,
    meta: Res<crate::world::WorldMeta>,
    mut sources: ResMut<Assets<AudioSource>>,
) {
    let listening = walk.active || settings.preview_in_editor;
    let active = map.interior() && listening;
    // the world's ambience hushed underground (and given back on the way out)
    if map.interior() {
        if amb.preview.is_none() || ds.hushed {
            amb.preview = Some((crate::world::Soundscape::None, 0.0));
            ds.hushed = true;
        }
    } else if ds.hushed {
        amb.preview = None;
        ds.hushed = false;
    }
    let master = if settings.muted {
        0.0
    } else {
        settings.master * settings.ambience * if walk.active { 1.0 } else { 0.6 }
    };
    let mut want: HashMap<u64, (String, f32)> = HashMap::new();
    // in the world (and realms): the portals hum as you come near them
    if listening && !map.interior() {
        let ear = cam.single().map(|c| c.translation()).unwrap_or(Vec3::ZERO);
        let lib = ds.library().clone();
        let mut near: Vec<(f32, String, u64, f32)> = Vec::new();
        for (i, w) in portals.placed.iter().enumerate() {
            let id = portal_loop(w.color);
            let r = lib
                .sounds
                .get(id)
                .and_then(|s| s.radius_m)
                .unwrap_or(14.0)
                .max(10.0);
            let d = (w.tf.translation + Vec3::Y * 1.5).distance(ear);
            if d < r {
                near.push((
                    d,
                    id.to_string(),
                    2_000_000 + i as u64,
                    (1.0 - d / r).powi(2),
                ));
            }
        }
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, id, key, k) in near.into_iter().take(4) {
            want.insert(key, (id, k * master * 1.1));
        }
    }
    // the dungeon's own track (underground or in a realm), over the mood's ambience
    let mut music_key = None;
    if listening
        && let Some(m) = map.0.as_ref()
        && let Some(path) = m.music.path(meta.path.as_deref())
    {
        if ds.music.as_ref().is_none_or(|(p, _)| *p != path) {
            ds.music = std::fs::read(&path).ok().map(|bytes| {
                (
                    path.clone(),
                    sources.add(AudioSource {
                        bytes: bytes.into(),
                    }),
                )
            });
        }
        if ds.music.is_some() {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut h);
            let key = MUSIC | (h.finish() & 0xffff_ffff);
            // (named for `playing` every frame it plays, not only the frame it starts)
            let name = format!(
                "music {}",
                path.file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            );
            want.insert(
                key,
                (
                    name,
                    m.music.volume * if settings.muted { 0.0 } else { settings.master },
                ),
            );
            music_key = Some((key, m.music.hush));
        }
    }
    if active && let Some(m) = map.0.as_ref() {
        let vol = m.mood.volume;
        let under = music_key.map_or(1.0, |(_, hush)| 1.0 - hush.clamp(0.0, 1.0));
        want.insert(BED, (m.mood.sound.clone(), vol * master * under));
        if !m.mood.sound2.is_empty() {
            want.insert(LAYER, (m.mood.sound2.clone(), vol * master * 0.6));
        }
        let ear = cam.single().map(|c| c.translation()).unwrap_or(Vec3::ZERO);
        let lib = ds.library().clone();
        let mut near: Vec<(f32, String, u64, f32)> = emitters(m, &portals)
            .into_iter()
            .filter_map(|(id, p, key)| {
                let r = lib
                    .sounds
                    .get(&id)
                    .and_then(|s| s.radius_m)
                    .unwrap_or(10.0)
                    .max(4.0);
                let d = p.distance(ear);
                (d < r).then(|| (d, id, key, (1.0 - d / r).powi(2)))
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, id, key, k) in near.into_iter().take(8) {
            want.insert(key, (id, k * master * vol * 1.2));
        }
        // now and then, a sound of the place off in the dark
        let now = time.elapsed_secs();
        if now > ds.next_emitter {
            let theme = lib.themes.get(&m.mood.theme);
            if let Some(t) = theme.filter(|t| !t.emitters.is_empty())
                && ds.next_emitter > 0.0
            {
                let id = &t.emitters[fastrand::usize(..t.emitters.len())];
                if let Some((file, gain)) = lib.take(id, fastrand::usize(..8)) {
                    commands.spawn((
                        Name::new(format!("Dungeon sound {id}")),
                        AudioPlayer::<AudioSource>(server.load(file)),
                        PlaybackSettings::DESPAWN
                            .with_volume(Volume::Linear(
                                gain * master * vol * fastrand::f32().mul_add(0.35, 0.35),
                            ))
                            .with_speed(0.92 + fastrand::f32() * 0.16),
                    ));
                }
            }
            ds.next_emitter = now + 9.0 + fastrand::f32() * 16.0;
        }
    }
    // fade what plays towards what is wanted; start what's missing
    let dt = time.delta_secs();
    let mut have = Vec::new();
    for (e, mut a, sink) in &mut playing {
        a.target = want.get(&a.key).map(|w| w.1).unwrap_or(0.0);
        let rate = 0.8 * dt;
        a.current += (a.target - a.current).clamp(-rate, rate);
        if let Some(mut s) = sink {
            s.set_volume(Volume::Linear((a.current * a.gain).max(0.0)));
        }
        if a.target <= 0.0 && a.current <= 0.001 {
            commands.entity(e).despawn();
        } else {
            have.push(a.key);
        }
    }
    let lib = ds.library().clone();
    let mut names = Vec::new();
    for (key, (id, v)) in want {
        if v <= 0.001 || have.contains(&key) {
            names.push(id.clone());
            continue;
        }
        if music_key.is_some_and(|(k, _)| k == key)
            && let Some((_, handle)) = ds.music.as_ref()
        {
            names.push(id.clone());
            commands.spawn((
                Name::new("Dungeon music"),
                DungeonAudio {
                    key,
                    gain: 1.0,
                    target: v,
                    current: 0.0,
                },
                AudioPlayer::<AudioSource>(handle.clone()),
                PlaybackSettings {
                    mode: PlaybackMode::Loop,
                    volume: Volume::Linear(0.0),
                    ..default()
                },
            ));
            continue;
        }
        names.push(id.clone());
        let Some((file, gain)) = lib.take(&id, key as usize) else {
            continue;
        };
        commands.spawn((
            Name::new(format!("Dungeon loop {id}")),
            DungeonAudio {
                key,
                gain,
                target: v,
                current: 0.0,
            },
            AudioPlayer::<AudioSource>(server.load(file)),
            PlaybackSettings {
                mode: PlaybackMode::Loop,
                volume: Volume::Linear(0.0),
                ..default()
            },
        ));
    }
    names.sort();
    ds.playing = names;
    let _ = listening;
}
