//! Weapons the player character equips and uses (Characters lab, Walk mode), in first and third
//! person: swords swing, bows draw → hold → release, guns fire from a firing stance or down the
//! sights with a narrower view. No damage, projectiles or effects yet: every swing, shot and
//! loosed arrow is a `WeaponAction` message (the hook the detailed combat port will read), and
//! melee swings also go to `creatures::Strikes` as the unarmed attacks do.
//!
//! - `WeaponCatalog`: every library model with a `weapon` dict in its `models.json` (class,
//!   hands, sockets, clips — the weapons pack's conventions, docs/weapons.md).
//! - `Armory`: the weapon sets on the number keys, what is equipped and what it is doing
//!   (`WState`), plus the moves table melee swings come from (`Moveset`).
//! - `hold`: the grip in the hand and the upper-body pose on the aim line.
//! - `drive`: equip / unequip, input → state, the weapon's own clips (`model_render::clips`), the
//!   character's upper-body clips and holds, the first-person view model and the zoom.
//! - `mcp`: `weapon_list`, `weapon_equip`, `weapon_unequip`, `weapon_state`, `weapon_use`,
//!   `characters_view`.

pub mod drive;
pub mod hold;
mod mcp;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::prelude::*;
use serde_json::Value;

/// How a weapon is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Melee,
    Bow,
    Gun,
}

impl Use {
    pub fn id(self) -> &'static str {
        match self {
            Use::Melee => "melee",
            Use::Bow => "bow",
            Use::Gun => "gun",
        }
    }
}

/// One clip of a weapon model.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponClip {
    pub length: f32,
    pub looped: bool,
}

/// A weapon of the library (`models.json` `weapon`).
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponDef {
    /// library id ("photoreal:wpn_halo_sidearm")
    pub id: String,
    pub name: String,
    /// assets path of its glTF
    pub gltf: String,
    /// pistol, rifle, melee, bow, heavy, unconventional
    pub class: String,
    pub subclass: Option<String>,
    pub hands: u8,
    /// the socket at the holding hand (the weapon's origin)
    pub hold: String,
    /// the other hand's socket
    pub support: Option<String>,
    /// held in the left hand (bows for a right-handed archer)
    pub left_hand: bool,
    pub ads: bool,
    pub ads_fov_deg: Option<f32>,
    pub aim: Option<String>,
    pub muzzle: Option<String>,
    pub fire_clip: Option<String>,
    pub reload_clip: Option<String>,
    pub equip_clip: Option<String>,
    pub draw_clip: Option<String>,
    pub hold_clip: Option<String>,
    pub release_clip: Option<String>,
    pub clips: BTreeMap<String, WeaponClip>,
    pub sockets: Vec<String>,
}

impl WeaponDef {
    pub fn usage(&self) -> Use {
        match self.class.as_str() {
            "melee" => Use::Melee,
            "bow" => Use::Bow,
            _ => Use::Gun,
        }
    }
    /// The hand holding its body.
    pub fn main_side(&self) -> hold::Side {
        if self.left_hand {
            hold::Side::Left
        } else {
            hold::Side::Right
        }
    }
    pub fn grip_style(&self) -> hold::GripStyle {
        match self.usage() {
            Use::Melee => hold::GripStyle::Upright,
            _ => hold::GripStyle::Along,
        }
    }
    pub fn clip_len(&self, name: Option<&str>) -> Option<f32> {
        name.and_then(|n| self.clips.get(n)).map(|c| c.length)
    }
    /// How narrow aiming makes the view (1: no zoom). `ads_fov_deg` is read against a 70°
    /// game view; bows zoom a little while drawn.
    pub fn zoom(&self) -> f32 {
        match self.usage() {
            Use::Gun if self.ads => (self.ads_fov_deg.unwrap_or(55.0) / 70.0).clamp(0.2, 1.0),
            Use::Bow => 0.72,
            _ => 1.0,
        }
    }

    /// Reads a model entry of a `models.json` (None without a `weapon` dict).
    pub fn parse(prefix: &str, folder: &str, m: &Value) -> Option<WeaponDef> {
        let w = m.get("weapon")?.as_object()?;
        let s = |k: &str| w.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let id = m.get("id")?.as_str()?;
        let clips = w
            .get("clips")
            .and_then(|c| c.as_object())
            .map(|c| {
                c.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            WeaponClip {
                                length: v.get("length").and_then(|x| x.as_f64()).unwrap_or(0.0)
                                    as f32,
                                looped: v.get("loop").and_then(|x| x.as_bool()).unwrap_or(false),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(WeaponDef {
            id: format!("{prefix}:{id}"),
            name: m.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string(),
            gltf: format!("models/{folder}/{}", m.get("gltf")?.as_str()?),
            class: s("class").unwrap_or_else(|| "pistol".into()),
            subclass: s("subclass"),
            hands: w.get("hands").and_then(|v| v.as_u64()).unwrap_or(1) as u8,
            hold: s("hold").unwrap_or_else(|| "grip_r".into()),
            support: s("support"),
            left_hand: s("hold_hand").as_deref() == Some("left"),
            ads: w.get("ads").and_then(|v| v.as_bool()).unwrap_or(false),
            ads_fov_deg: w.get("ads_fov_deg").and_then(|v| v.as_f64()).map(|v| v as f32),
            aim: s("aim"),
            muzzle: s("muzzle"),
            fire_clip: s("fire_clip"),
            reload_clip: s("reload_clip"),
            equip_clip: s("equip_clip"),
            draw_clip: s("draw_clip"),
            hold_clip: s("hold_clip"),
            release_clip: s("release_clip"),
            clips,
            sockets: w
                .get("sockets")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
        })
    }
}

/// Every weapon of the bundled and installed libraries, by library id.
#[derive(Resource, Default)]
pub struct WeaponCatalog {
    pub by_id: BTreeMap<String, WeaponDef>,
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
}

impl WeaponCatalog {
    pub fn load(dir: &Path) -> Self {
        let mut out = WeaponCatalog::default();
        for lib in crate::objects::model_libraries(dir) {
            let path = lib.dir.join("models.json");
            out.stamps.push((path.clone(), modified(&path)));
            let Some(doc) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            else {
                continue;
            };
            for m in doc.get("models").and_then(|m| m.as_array()).into_iter().flatten() {
                if let Some(w) = WeaponDef::parse(&lib.prefix, &lib.folder, m) {
                    out.by_id.insert(w.id.clone(), w);
                }
            }
        }
        out
    }
    pub fn get(&self, id: &str) -> Option<&WeaponDef> {
        self.by_id.get(id)
    }
    fn stale(&self) -> bool {
        self.stamps.iter().any(|(p, t)| modified(p) != *t)
    }
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn models_dir() -> PathBuf {
    PathBuf::from(crate::assets_path()).join("models")
}

// ------------------------------------------------------------------------------- moves

/// One melee move (the shape of the combat port's move tables): its character clip, the part
/// of it that plays (fractions), from when the next attack may cut in, and when the strike lands.
#[derive(Clone, Debug, PartialEq)]
pub struct MeleeMove {
    pub id: &'static str,
    /// animation library key
    pub clip: &'static str,
    pub start: f32,
    pub end: f32,
    /// a queued next attack starts from here
    pub cancel: f32,
    pub speed: f32,
    /// strike moments (fractions of the clip): each sends a `WeaponAction::Swing`
    pub hits: &'static [f32],
    pub heavy: bool,
}

/// A weapon's moves: its ready stance and attacks.
#[derive(Clone, Debug)]
pub struct Moveset {
    /// upper-body clip while ready (None: the moves' own arms)
    pub ready: Option<&'static str>,
    pub light: &'static [MeleeMove],
    pub heavy: Option<MeleeMove>,
}

const SWORD_LIGHT: &[MeleeMove] = &[
    MeleeMove { id: "sword.a", clip: "ual2/Sword_Regular_A", start: 0.0, end: 0.92, cancel: 0.55, speed: 1.15, hits: &[0.38], heavy: false },
    MeleeMove { id: "sword.b", clip: "ual2/Sword_Regular_B", start: 0.0, end: 0.92, cancel: 0.55, speed: 1.15, hits: &[0.36], heavy: false },
    MeleeMove { id: "sword.c", clip: "ual2/Sword_Regular_C", start: 0.0, end: 0.95, cancel: 0.62, speed: 1.1, hits: &[0.42], heavy: false },
];
const SWORD_HEAVY: MeleeMove = MeleeMove {
    id: "sword.heavy",
    clip: "ual2/Sword_Heavy_Combo",
    start: 0.0,
    end: 0.95,
    cancel: 0.8,
    speed: 1.05,
    hits: &[0.3, 0.62],
    heavy: true,
};

/// The moves a weapon of this use fights with (bundled CC0 UAL clips).
pub fn moveset(u: Use) -> Moveset {
    match u {
        Use::Melee => Moveset {
            ready: Some("ual1/Sword_Idle"),
            light: SWORD_LIGHT,
            heavy: Some(SWORD_HEAVY),
        },
        Use::Gun => Moveset {
            ready: Some("ual1/Pistol_Idle_Loop"),
            light: &[],
            heavy: None,
        },
        Use::Bow => Moveset {
            ready: None,
            light: &[],
            heavy: None,
        },
    }
}

/// Upper-body clips of the ranged stances.
pub const GUN_AIM_CLIP: &str = "ual1/Pistol_Aim_Neutral";
pub const GUN_RELOAD_CLIP: &str = "ual1/Pistol_Reload";

// ------------------------------------------------------------------------------- state

/// What the equipped weapon is doing.
#[derive(Clone, Debug, PartialEq)]
pub enum WState {
    /// nothing in hand
    Holstered,
    /// in hand (just drawn: `t` since equip)
    Ready,
    /// a melee move playing: which, the time into it (s, clip time), the strikes already sent,
    /// the next attack queued
    Swing {
        mv: MeleeMove,
        combo: usize,
        t: f32,
        sent: usize,
        queued: Option<bool>,
    },
    /// pulling the string (s)
    Drawing { t: f32 },
    /// at full draw, held (s)
    Drawn { t: f32 },
    /// the string snapping back (s)
    Releasing { t: f32 },
    /// a shot just fired (s since)
    Firing { t: f32 },
    Reloading { t: f32, len: f32 },
}

impl WState {
    pub fn id(&self) -> &'static str {
        match self {
            WState::Holstered => "holstered",
            WState::Ready => "ready",
            WState::Swing { .. } => "swing",
            WState::Drawing { .. } => "drawing",
            WState::Drawn { .. } => "drawn",
            WState::Releasing { .. } => "releasing",
            WState::Firing { .. } => "firing",
            WState::Reloading { .. } => "reloading",
        }
    }
}

/// What happened (the combat hook): read `MessageReader<WeaponAction>`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActionKind {
    Equip,
    Unequip,
    /// a melee strike moment: `origin` the blade root, `tip` its point
    Swing { heavy: bool, combo: usize },
    /// a gun shot leaving the muzzle
    Shot,
    /// an arrow loosed, with the draw it had (0..1)
    Arrow { draw: f32 },
    Reload,
}

#[derive(Message, Clone, Debug, PartialEq)]
pub struct WeaponAction {
    pub weapon: String,
    pub kind: ActionKind,
    /// world space: the muzzle / nock / blade root (the character's chest when unknown)
    pub origin: Vec3,
    /// the aim (where the camera looks), or the swing's facing
    pub dir: Vec3,
    /// the blade's tip for swings
    pub tip: Option<Vec3>,
    /// seconds since walking began (simulation time)
    pub time: f32,
}

/// Commands from MCP `weapon_use` (applied next frame, like the mouse).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UseCmd {
    Attack { heavy: bool },
    Draw,
    Release,
    Fire,
    Aim(bool),
    Reload,
}

/// The player's weapons: sets on the number keys, what's in hand and what it is doing.
#[derive(Resource)]
pub struct Armory {
    /// weapon sets on 1..4 (library ids)
    pub sets: [Option<String>; 4],
    /// what's in hand
    pub equipped: Option<String>,
    pub state: WState,
    /// the aim / zoom held (right mouse, or MCP `aim on`)
    pub aiming: bool,
    /// MCP holds: the bow's string pulled, the sights up
    pub mcp_draw: bool,
    pub mcp_aim: bool,
    pub queue: Vec<UseCmd>,
    /// a hand-tuned grip (weapon space: metres, degrees) over the computed one
    pub grip_offset: Vec3,
    pub grip_rot_deg: Vec3,
    /// how raised the weapon is (0 at the side .. 1 on the aim line) and how far down the sights
    pub raise: f32,
    pub ads: f32,
    /// the recent actions (newest last), for `weapon_state`
    pub log: Vec<WeaponAction>,
    pub shots: u32,
    pub swings: u32,
    pub arrows: u32,
    /// the camera's aim (world)
    pub aim_dir: Vec3,
    /// a preview turn of the third-person camera round the character (degrees; the aim keeps
    /// the unturned camera's): look at the stance from the side or the front
    pub view_orbit_deg: f32,
    defaults_done: bool,
    /// last stance clip asked of the weapon (so it isn't re-asked every frame)
    pub(crate) clip_asked: Option<String>,
    /// seconds since the last shot (the firing stance lowers after a while)
    pub(crate) since_shot: f32,
}

impl Default for Armory {
    fn default() -> Self {
        Self {
            sets: Default::default(),
            equipped: None,
            state: WState::Holstered,
            aiming: false,
            mcp_draw: false,
            mcp_aim: false,
            queue: vec![],
            grip_offset: Vec3::ZERO,
            grip_rot_deg: Vec3::ZERO,
            raise: 0.0,
            ads: 0.0,
            log: vec![],
            shots: 0,
            swings: 0,
            arrows: 0,
            aim_dir: Vec3::NEG_Z,
            clip_asked: None,
            view_orbit_deg: 0.0,
            defaults_done: false,
            since_shot: 99.0,
        }
    }
}

impl Armory {
    /// The body turns to where the camera looks (aiming, drawing, firing, swinging).
    pub fn faces_aim(&self) -> bool {
        self.equipped.is_some()
            && (self.aiming
                || !matches!(self.state, WState::Holstered | WState::Ready)
                || self.raise > 0.5)
    }
    /// The weapon module takes the mouse buttons (no unarmed attacks / block while armed).
    pub fn takes_mouse(&self) -> bool {
        self.equipped.is_some()
    }
    /// Fills empty sets (once) from the catalog's weapons this account may use: 1 a blade,
    /// 2 a bow, 3 a gun.
    pub fn default_sets(&mut self, cat: &WeaponCatalog) {
        if self.defaults_done || self.sets.iter().any(|s| s.is_some()) {
            return;
        }
        self.defaults_done = true;
        let market = crate::market::snapshot();
        for (slot, u) in [(0, Use::Melee), (1, Use::Bow), (2, Use::Gun)] {
            self.sets[slot] = cat
                .by_id
                .values()
                .find(|w| w.usage() == u && market.usable(&w.id))
                .map(|w| w.id.clone());
        }
    }
}

static GUN_IN_HAND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A gun is in the walking player's hand (R reloads it instead of mounting).
pub fn gun_in_hand() -> bool {
    GUN_IN_HAND.load(std::sync::atomic::Ordering::Relaxed)
}

/// The equipped weapon's systems (after the camera is placed, before puppets pose).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WeaponSystems;

pub struct WeaponsPlugin;

impl Plugin for WeaponsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(WeaponCatalog::load(&models_dir()))
            .init_resource::<Armory>()
            .add_message::<WeaponAction>()
            .configure_sets(
                Update,
                WeaponSystems
                    .after(super::play_camera::PlayCameraSystems)
                    .before(super::puppet::PuppetSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(Update, (reload, drive::drive).chain().in_set(WeaponSystems));
        mcp::register(app);
    }
}

/// Re-reads the catalog when a manifest changed (checked every 2 s).
fn reload(mut cat: ResMut<WeaponCatalog>, time: Res<Time>, mut timer: Local<f32>) {
    *timer -= time.delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 2.0;
    if cat.stale() {
        *cat = WeaponCatalog::load(&models_dir());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_weapon_entry() {
        let m: Value = serde_json::from_str(
            r#"{"id": "wpn_x", "name": "X", "gltf": "wpn_x/wpn_x.gltf", "weapon": {"class": "bow", "hands": 2,
            "hold": "grip_l", "support": "grip_r", "hold_hand": "left", "draw_clip": "draw", "hold_clip": "hold",
            "release_clip": "release", "clips": {"draw": {"length": 0.9, "loop": false}, "hold": {"length": 1.2, "loop": true}},
            "sockets": ["grip_l", "grip_r", "nock"]}}"#,
        )
        .unwrap();
        let w = WeaponDef::parse("photoreal", "stylepacks/photoreal", &m).unwrap();
        assert_eq!(w.id, "photoreal:wpn_x");
        assert_eq!(w.gltf, "models/stylepacks/photoreal/wpn_x/wpn_x.gltf");
        assert_eq!(w.usage(), Use::Bow);
        assert_eq!(w.main_side(), hold::Side::Left);
        assert_eq!(w.clip_len(Some("draw")), Some(0.9));
        assert!(w.clips["hold"].looped);
        assert!(w.zoom() < 1.0);
        let plain: Value = serde_json::from_str(r#"{"id": "crate", "gltf": "c.gltf"}"#).unwrap();
        assert!(WeaponDef::parse("p", "f", &plain).is_none());
    }

    #[test]
    fn the_bundled_weapons_load() {
        let cat = WeaponCatalog::load(&PathBuf::from("assets/models"));
        let Some(gun) = cat.get("photoreal:wpn_halo_sidearm") else {
            return; // (the pack's files aren't in this checkout)
        };
        assert_eq!(gun.usage(), Use::Gun);
        assert_eq!(gun.fire_clip.as_deref(), Some("fire"));
        assert!(gun.sockets.iter().any(|s| s == "aim"));
        let mut a = Armory::default();
        a.default_sets(&cat);
        // default sets only take weapons the market lets this machine use (a test run has no
        // entitlements loaded), so expect one set per usable weapon class, which may be none
        let market = crate::market::snapshot();
        let usable = [Use::Melee, Use::Bow, Use::Gun]
            .into_iter()
            .filter(|u| cat.by_id.values().any(|w| w.usage() == *u && market.usable(&w.id)))
            .count();
        assert_eq!(a.sets.iter().filter(|s| s.is_some()).count(), usable);
    }

    #[test]
    fn move_tables_are_in_order() {
        for m in SWORD_LIGHT.iter().chain([&SWORD_HEAVY]) {
            assert!(m.start < m.cancel && m.cancel <= m.end && m.end <= 1.0, "{}", m.id);
            assert!(m.hits.iter().all(|h| *h > m.start && *h < m.end), "{}", m.id);
        }
    }
}
