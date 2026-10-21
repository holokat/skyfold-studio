//! Auras: a living atmosphere round a model, from its effect anchors (`fx_anchors`). An
//! `aura_dark` anchor (the dark spire's) wraps the model in a dark aura: dark wisps curling up
//! round it and purple motes spiralling up, a low mist creeping round its foot, a slowly pulsing
//! purple light that also breathes through the model's glowing fissures (the anchor's `pulse`
//! material), and a low drone with whispers that fades in as you come near (played by
//! `audio` with the other ambience).
//!
//! Cheap by design: the particles are a fixed set of camera-facing quads animated entirely in the
//! vertex shader (`shaders/aura.wesl`: no CPU work per particle, two draws per aura), and only the
//! few auras nearest the camera (`AuraSettings::max_live`, within `range`) get one of the pooled
//! slots, fading in and out as they come and go, so a world full of spires costs what a handful
//! does. Like smoke and cloth the aura moves on the world's clock: frozen while editing from the
//! overview (a still frame), alive while walking or with Preview motion.
//!
//! New aura kinds are a row in `STYLES` (colours, light, sound loops). MCP: `auras` (settings and
//! what is live), `effect_anchors` (where effects attach).

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::audio::{AudioSink, AudioSinkPlayback};
use bevy::gltf::GltfMaterialName;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::camera::MainCamera;
use crate::fx_anchors::{FxAnchors, world_anchor};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};
use crate::objects::ObjectStore;
use crate::sky::SkyState;

/// How an aura kind looks and sounds.
pub struct AuraStyle {
    /// the anchor kind ("aura_dark")
    pub kind: &'static str,
    /// the vapour's colour (linear) and opacity
    pub smoke: [f32; 3],
    pub smoke_alpha: f32,
    /// the motes and the vapour's inner light (linear, display units)
    pub glow: [f32; 3],
    /// the light's colour (sRGB) and lumens at full strength by night
    pub light: [f32; 3],
    pub lumens: f32,
    /// seconds per breath of the light and the model's glow
    pub pulse_period: f32,
    /// ambience loops (`assets/audio/ambience/<stem>.mp3`) and their volumes close by
    pub sounds: &'static [(&'static str, f32)],
    /// defaults when the anchor gives no radius / height (m)
    pub radius: f32,
    pub height: f32,
}

pub const STYLES: &[AuraStyle] = &[AuraStyle {
    kind: "aura_dark",
    smoke: [0.035, 0.018, 0.05],
    smoke_alpha: 0.6,
    glow: [1.6, 0.45, 3.2],
    light: [0.62, 0.3, 1.0],
    lumens: 120_000.0,
    pulse_period: 4.6,
    // Skyfold's own loops (assets/audio/dungeon/CREDITS.md): the void drone, haunted whispers
    sounds: &[("amb_dng_void_0", 0.8), ("amb_dng_haunted_0", 0.5)],
    radius: 6.0,
    height: 18.0,
}];

/// The style of an anchor kind (None: not an aura).
pub fn style(kind: &str) -> Option<&'static AuraStyle> {
    STYLES.iter().find(|s| s.kind == kind)
}

/// Every aura sound loop (loaded by `audio`).
pub fn sound_stems() -> Vec<&'static str> {
    STYLES
        .iter()
        .flat_map(|s| s.sounds.iter().map(|x| x.0))
        .collect()
}

/// Wisps, mist and motes per aura (kinds 0, 1 and 2 of `aura.wesl`).
const WISPS: usize = 72;
const MIST: usize = 24;
const MOTES: usize = 150;
/// Slots at most (`max_live` picks how many are used).
const MAX_SLOTS: usize = 6;
/// Sound: full this far beyond the aura's radius, silent from `SOUND_FAR` beyond it (m).
const SOUND_NEAR: f32 = 6.0;
const SOUND_FAR: f32 = 65.0;

/// The aura settings (an editor preference, `prefs` key "auras"; never saved with a world).
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuraSettings {
    pub enabled: bool,
    /// 0..2: particles, light and glow (0 hides them)
    pub strength: f32,
    /// how near the camera an aura comes alive (m)
    pub range: f32,
    /// auras alive at once (the nearest)
    pub max_live: usize,
    /// 0..2: the sound loops' volume
    pub sound: f32,
}

impl Default for AuraSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 1.0,
            range: 180.0,
            max_live: 4,
            sound: 1.0,
        }
    }
}

impl AuraSettings {
    fn clamp(mut self) -> Self {
        self.strength = self.strength.clamp(0.0, 2.0);
        self.range = self.range.clamp(40.0, 600.0);
        self.max_live = self.max_live.clamp(1, MAX_SLOTS);
        self.sound = self.sound.clamp(0.0, 2.0);
        self
    }

    fn load() -> Self {
        crate::prefs::get("auras")
            .and_then(|v| serde_json::from_value::<AuraSettings>(v).ok())
            .unwrap_or_default()
            .clamp()
    }
}

pub type AuraMaterial = ExtendedMaterial<StandardMaterial, AuraExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct AuraParams {
    /// xyz: the anchor (world), w: radius (m)
    pub centre: Vec4,
    /// x: height (m), y: strength (0 hides), z: layer (0 smoke and mist, 1 motes), w: clock (s)
    pub shape: Vec4,
    /// rgb: the vapour's colour (linear), w: its opacity
    pub smoke: Vec4,
    /// rgb: the glow (linear), w: night 0..1
    pub glow: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct AuraExt {
    #[uniform(100)]
    pub params: AuraParams,
}

impl MaterialExtension for AuraExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/aura.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/aura.wesl".into()
    }
}

/// A placed object's aura (from one of its anchors).
#[derive(Clone, Debug)]
struct Cand {
    object: u64,
    model: String,
    style: usize,
    pos: Vec3,
    radius: f32,
    height: f32,
    /// the model's material whose glow breathes with the light
    pulse: Option<String>,
    /// its own place in the breath
    phase: f32,
}

impl Cand {
    /// Distance from `p` to the aura's lower body (the column from its foot up 60% of its height).
    fn reach(&self, p: Vec3) -> f32 {
        let y = p.y.clamp(self.pos.y, self.pos.y + self.height * 0.6);
        p.distance(Vec3::new(self.pos.x, y, self.pos.z))
    }
}

/// One pooled aura: its two particle layers, its light and whom it serves.
struct Slot {
    smoke: Entity,
    glow: Entity,
    light: Entity,
    smoke_mat: Handle<AuraMaterial>,
    glow_mat: Handle<AuraMaterial>,
    cand: Option<Cand>,
    /// faded in 0..1
    level: f32,
    /// the model's glowing parts given a material of their own: (entity, its own material)
    pulse_parts: Vec<(Entity, Handle<StandardMaterial>)>,
    /// own material → (the shared material it stands in for, that one's emissive)
    pulse_mats: HashMap<
        AssetId<StandardMaterial>,
        (
            Handle<StandardMaterial>,
            Handle<StandardMaterial>,
            LinearRgba,
        ),
    >,
    /// the render entity the parts were found under (rescanned when it changes)
    pulse_root: Option<Entity>,
    pulse_scan: f32,
}

/// The auras' state: candidates, the slot pool, the clock and the sound levels.
#[derive(Resource, Default)]
pub struct Auras {
    cands: Vec<Cand>,
    seen: Option<(u64, u64)>,
    rescan: f32,
    slots: Vec<Slot>,
    meshes: Option<(Handle<Mesh>, Handle<Mesh>)>,
    clock: f32,
    /// (stem, level 0..2) the aura loops want, before the master and ambience volumes
    sounds: Vec<(&'static str, f32)>,
}

impl Auras {
    /// The aura loops and how loud they want to be (0..2, before the ambience volume).
    pub fn sounds(&self) -> &[(&'static str, f32)] {
        &self.sounds
    }
}

#[derive(Component)]
struct AuraPart;

#[derive(Component)]
struct AuraLight;

pub struct AurasPlugin;

impl Plugin for AurasPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<AuraMaterial>::default())
            .insert_resource(AuraSettings::load())
            .init_resource::<Auras>()
            .add_systems(Update, update_auras.after(crate::sky::SkySystems));
        app.mcp_tool(McpTool {
            name: "auras",
            description: "Auras round models, from their effect anchors (see effect_anchors): hp:dark_spire's `aura_dark` wraps it in dark wisps and purple motes rising, a low mist creeping round its base, a slowly pulsing purple light that also breathes through its glowing fissures, and a low drone with whispers that fades in as you approach. Only the nearest few within `range` of the camera are alive (they fade in and out); they move while the world animates (walking, Preview motion) and hold still while editing. Returns the settings, the live auras (object, model, distance, fade) and the aura sounds (wanted level; playing volume and position while a loop plays). Pass any of `enabled`, `strength` (0..2: particles, light and glow; 0 hides them), `range` (m, 40..600), `max_live` (1..6 auras at once) or `sound` (0..2: the loops' volume) to change them (saved as an editor preference, not with the world).",
            input_schema: || {
                json!({ "type": "object", "properties": {
                    "enabled": { "type": "boolean" },
                    "strength": { "type": "number", "minimum": 0, "maximum": 2 },
                    "range": { "type": "number", "minimum": 40, "maximum": 600, "description": "metres from the camera" },
                    "max_live": { "type": "integer", "minimum": 1, "maximum": MAX_SLOTS },
                    "sound": { "type": "number", "minimum": 0, "maximum": 2 } } })
            },
            run: auras_tool,
        });
    }
}

/// The particle quads of one layer: positions are seeds in the unit cube (the shader places
/// them), vertex colour (kind / 4, phase, size, seed), uv the corner.
fn layer_mesh(layers: &[(u32, usize)], seed: u64) -> Mesh {
    let mut rng = crate::noise::Rng::new(seed);
    let (mut pos, mut uv, mut col, mut nor, mut idx) = (vec![], vec![], vec![], vec![], vec![]);
    for &(kind, n) in layers {
        for _ in 0..n {
            let p = [rng.f32(), rng.f32(), rng.f32()];
            let size = match kind {
                0 => rng.range(0.75, 1.35),
                1 => rng.range(0.8, 1.2),
                _ => rng.range(0.7, 1.6),
            };
            let c = [kind as f32 / 4.0, rng.f32(), size, rng.f32()];
            let base = pos.len() as u32;
            for corner in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
                pos.push(p);
                uv.push(corner);
                col.push(c);
                nor.push([0.0, 1.0, 0.0]);
            }
            idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
        }
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

fn lin(c: [f32; 3]) -> Vec3 {
    Vec3::from_array(c)
}

/// The auras of every placed object with aura anchors (nothing to look through when no library
/// model has one).
fn candidates(store: &ObjectStore, anchors: &FxAnchors) -> Vec<Cand> {
    let mut out = vec![];
    if !anchors
        .by_model
        .values()
        .flatten()
        .any(|a| style(&a.kind).is_some())
    {
        return out;
    }
    for p in store.items.values() {
        for a in anchors.of(&p.asset) {
            let Some(si) = STYLES.iter().position(|s| s.kind == a.kind) else {
                continue;
            };
            let st = &STYLES[si];
            let w = world_anchor(p, a);
            let k = p.scale3();
            out.push(Cand {
                object: p.id,
                model: p.asset.clone(),
                style: si,
                pos: w.pos,
                radius: if a.radius > 0.0 {
                    w.radius
                } else {
                    st.radius * k.x.max(k.z)
                },
                height: if a.height > 0.0 {
                    w.height
                } else {
                    st.height * k.y
                },
                pulse: a.extra_str("pulse").map(str::to_string),
                phase: ((p.id.wrapping_mul(2_654_435_761) % 1000) as f32 / 1000.0)
                    * std::f32::consts::TAU,
            });
        }
    }
    out.sort_by_key(|c| c.object);
    out
}

/// The auras that should be alive: the `max` nearest to `cam` within `range` (indices into
/// `cands`, nearest first).
fn wanted(cands: &[Cand], cam: Vec3, range: f32, max: usize) -> Vec<usize> {
    let mut near: Vec<(f32, usize)> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (c.reach(cam), i))
        .filter(|(d, _)| *d < range)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.into_iter().take(max).map(|x| x.1).collect()
}

/// How loud an aura's loops are at distance `d` from it (1 close by, 0 far off).
pub fn sound_falloff(d: f32, radius: f32) -> f32 {
    1.0 - crate::noise::smoothstep(radius + SOUND_NEAR, radius + SOUND_FAR, d)
}

/// The breath of the light and the glow: 0..1, on the aura's clock.
fn breath(clock: f32, period: f32, phase: f32) -> f32 {
    0.5 + 0.5 * (clock * std::f32::consts::TAU / period + phase).sin()
}

#[allow(clippy::too_many_arguments)]
fn update_auras(
    mut commands: Commands,
    mut au: ResMut<Auras>,
    settings: Res<AuraSettings>,
    anchors: Res<FxAnchors>,
    store: Res<ObjectStore>,
    sky: Res<SkyState>,
    (motion, time): (Res<crate::wind::Motion>, Res<Time>),
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut aura_mats: ResMut<Assets<AuraMaterial>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    mut lights: Query<
        (&mut PointLight, &mut Transform, &mut Visibility),
        (With<AuraLight>, Without<AuraPart>),
    >,
    mut parts: Query<(&mut Transform, &mut Visibility), (With<AuraPart>, Without<AuraLight>)>,
    (children, named): (
        Query<&Children>,
        Query<(&GltfMaterialName, &MeshMaterial3d<StandardMaterial>)>,
    ),
) {
    let au = &mut *au;
    let dt = time.delta_secs().min(0.1);
    if motion.live {
        au.clock = (au.clock + dt) % 3600.0;
    }
    // the placed auras change only when the objects or the anchors do (looked up at most twice a
    // second: painting objects changes the store every frame)
    let key = (store.revision, anchors.revision);
    au.rescan -= dt;
    if au.seen != Some(key) && (au.rescan <= 0.0 || au.seen.is_none_or(|s| s.1 != key.1)) {
        au.seen = Some(key);
        au.rescan = 0.5;
        au.cands = candidates(&store, &anchors);
    }
    // sounds: every aura near the listener (walker or camera focus), loudest wins
    let mut sounds: Vec<(&'static str, f32)> = vec![];
    if settings.enabled && settings.strength > 0.0 {
        for c in &au.cands {
            let k = sound_falloff(c.reach(sky.viewer_pos), c.radius)
                * settings.sound
                * settings.strength.min(1.0);
            if k <= 0.0 {
                continue;
            }
            for &(stem, v) in STYLES[c.style].sounds {
                match sounds.iter_mut().find(|s| s.0 == stem) {
                    Some(s) => s.1 = s.1.max(v * k),
                    None => sounds.push((stem, v * k)),
                }
            }
        }
    }
    au.sounds = sounds;
    let Ok(cam) = cam.single() else { return };
    let cam_p = cam.translation();
    let want: Vec<usize> = if settings.enabled && settings.strength > 0.0 {
        wanted(&au.cands, cam_p, settings.range, settings.max_live)
    } else {
        vec![]
    };
    // fade the slots toward their auras (in at 1/s, out at 1.5/s); a faded-out slot is free
    let mut taken: Vec<u64> = vec![];
    for s in &mut au.slots {
        let Some((object, style)) = s.cand.as_ref().map(|c| (c.object, c.style)) else {
            continue;
        };
        let keep = want
            .iter()
            .any(|&i| au.cands[i].object == object && au.cands[i].style == style);
        // (follow the object if it moved)
        if let Some(n) = au
            .cands
            .iter()
            .find(|n| n.object == object && n.style == style)
        {
            s.cand = Some(n.clone());
        }
        s.level = (s.level + if keep { dt } else { -1.5 * dt }).clamp(0.0, 1.0);
        if keep || s.level > 0.0 {
            taken.push(object);
        } else {
            release(&mut commands, s);
        }
    }
    // give the newly wanted auras a free slot (made on first need)
    for &i in &want {
        let c = &au.cands[i];
        if taken.contains(&c.object) {
            continue;
        }
        if !au.slots.iter().any(|s| s.cand.is_none()) && au.slots.len() < MAX_SLOTS {
            let slot = new_slot(&mut commands, &mut meshes, &mut aura_mats, &mut au.meshes);
            au.slots.push(slot);
        }
        let Some(s) = au.slots.iter_mut().find(|s| s.cand.is_none()) else {
            break;
        };
        if std::env::var("WB_AURA_LOG").is_ok() {
            info!("aura on: object {} ({}) at {:?}", c.object, c.model, c.pos);
        }
        s.cand = Some(c.clone());
        s.level = 0.0;
        s.pulse_root = None;
        s.pulse_scan = 0.0;
        taken.push(c.object);
    }
    // drive the live ones
    let night = sky.night.max(sky.weather.dim * 0.5);
    for s in &mut au.slots {
        let Some(c) = s.cand.clone() else {
            for e in [s.smoke, s.glow] {
                if let Ok((_, mut v)) = parts.get_mut(e) {
                    v.set_if_neq(Visibility::Hidden);
                }
            }
            if let Ok((_, _, mut v)) = lights.get_mut(s.light) {
                v.set_if_neq(Visibility::Hidden);
            }
            continue;
        };
        let st = &STYLES[c.style];
        let d = c.reach(cam_p);
        let fade =
            s.level * (1.0 - crate::noise::smoothstep(settings.range * 0.7, settings.range, d));
        let k = fade * settings.strength;
        let b = breath(au.clock, st.pulse_period, c.phase);
        for (e, m, layer) in [(s.smoke, &s.smoke_mat, 0.0), (s.glow, &s.glow_mat, 1.0)] {
            if let Ok((mut tf, mut v)) = parts.get_mut(e) {
                tf.translation = c.pos;
                v.set_if_neq(if k > 0.0 {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                });
            }
            if let Some(mut mat) = aura_mats.get_mut(m) {
                let glow = lin(st.glow) * (0.75 + 0.5 * b) * (0.45 + 0.55 * night);
                mat.extension.params = AuraParams {
                    centre: c.pos.extend(c.radius),
                    shape: Vec4::new(c.height, k, layer, au.clock),
                    smoke: lin(st.smoke).extend(st.smoke_alpha),
                    glow: glow.extend(night),
                };
            }
        }
        if let Ok((mut pl, mut tf, mut v)) = lights.get_mut(s.light) {
            tf.translation = c.pos + Vec3::Y * (c.height * 0.22);
            pl.color = Color::srgb(st.light[0], st.light[1], st.light[2]);
            pl.range = c.radius * 3.6;
            pl.intensity = st.lumens * k * (0.65 + 0.35 * b) * (0.25 + 0.75 * night);
            v.set_if_neq(if k > 0.0 {
                Visibility::Visible
            } else {
                Visibility::Hidden
            });
        }
        // the model's glowing parts breathe with the light: they get a material of their own
        if let Some(name) = &c.pulse {
            let root = store.entities.get(&c.object).copied();
            if root != s.pulse_root {
                restore(&mut commands, s);
                s.pulse_root = root;
                s.pulse_scan = 0.0;
            }
            // (a respawned scene: its parts have their shared material again)
            if s.pulse_parts
                .first()
                .is_some_and(|(e, _)| named.get(*e).is_err())
            {
                s.pulse_parts.clear();
                s.pulse_mats.clear();
            }
            s.pulse_scan -= dt;
            if s.pulse_parts.is_empty()
                && s.pulse_scan <= 0.0
                && let Some(root) = root
            {
                // (the scene may still be loading: look again in a moment)
                s.pulse_scan = 0.5;
                for e in children.iter_descendants(root) {
                    let Ok((n, m)) = named.get(e) else { continue };
                    if n.0 != *name {
                        continue;
                    }
                    let own = match s
                        .pulse_mats
                        .values()
                        .find(|(orig, _, _)| orig.id() == m.0.id())
                    {
                        Some((_, own, _)) => own.clone(),
                        None => {
                            let Some(src) = std_mats.get(&m.0).cloned() else {
                                continue;
                            };
                            let base = src.emissive;
                            let own = std_mats.add(src);
                            s.pulse_mats
                                .insert(own.id(), (m.0.clone(), own.clone(), base));
                            own
                        }
                    };
                    commands.entity(e).insert(MeshMaterial3d(own.clone()));
                    s.pulse_parts.push((e, own));
                }
            }
            for (_, own, base) in s.pulse_mats.values() {
                if let Some(mut m) = std_mats.get_mut(own) {
                    m.emissive = *base * (1.0 + (0.55 + 0.9 * b - 1.0) * fade.min(1.0));
                }
            }
        }
    }
}

/// A new slot: its two particle layers and its light, hidden.
fn new_slot(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<AuraMaterial>,
    shared: &mut Option<(Handle<Mesh>, Handle<Mesh>)>,
) -> Slot {
    let (smoke_mesh, glow_mesh) = shared
        .get_or_insert_with(|| {
            (
                meshes.add(layer_mesh(&[(0, WISPS), (1, MIST)], 0xa0_7a)),
                meshes.add(layer_mesh(&[(2, MOTES)], 0x5ba7)),
            )
        })
        .clone();
    let smoke_mat = mats.add(AuraMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 1.0,
            reflectance: 0.2,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        extension: AuraExt {
            params: AuraParams::default(),
        },
    });
    let glow_mat = mats.add(AuraMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Add,
            unlit: true,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        extension: AuraExt {
            params: AuraParams::default(),
        },
    });
    let part =
        |commands: &mut Commands, name: &str, mesh: Handle<Mesh>, mat: Handle<AuraMaterial>| {
            commands
                .spawn((
                    Name::new(name.to_string()),
                    AuraPart,
                    Mesh3d(mesh),
                    MeshMaterial3d(mat),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    NotShadowCaster,
                    NotShadowReceiver,
                    // (placed in the vertex shader: the mesh's own bounds mean nothing)
                    bevy::camera::visibility::NoFrustumCulling,
                ))
                .id()
        };
    let smoke = part(commands, "Aura smoke", smoke_mesh, smoke_mat.clone());
    let glow = part(commands, "Aura motes", glow_mesh, glow_mat.clone());
    let light = commands
        .spawn((
            Name::new("Aura light"),
            AuraLight,
            PointLight {
                color: Color::WHITE,
                intensity: 0.0,
                range: 20.0,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::IDENTITY,
            Visibility::Hidden,
        ))
        .id();
    Slot {
        smoke,
        glow,
        light,
        smoke_mat,
        glow_mat,
        cand: None,
        level: 0.0,
        pulse_parts: vec![],
        pulse_mats: HashMap::new(),
        pulse_root: None,
        pulse_scan: 0.0,
    }
}

/// Gives the model's glowing parts their shared material back.
fn restore(commands: &mut Commands, s: &mut Slot) {
    for (e, own) in s.pulse_parts.drain(..) {
        if let Some((orig, _, _)) = s.pulse_mats.get(&own.id())
            && let Ok(mut ec) = commands.get_entity(e)
        {
            ec.try_insert(MeshMaterial3d(orig.clone()));
        }
    }
    s.pulse_mats.clear();
}

/// Frees a slot (its aura has faded out).
fn release(commands: &mut Commands, s: &mut Slot) {
    if std::env::var("WB_AURA_LOG").is_ok()
        && let Some(c) = &s.cand
    {
        info!("aura off: object {} ({})", c.object, c.model);
    }
    restore(commands, s);
    s.cand = None;
    s.level = 0.0;
    s.pulse_root = None;
}

fn r2(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn auras_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let mut s = world.resource::<AuraSettings>().clone();
    let before = s.clone();
    if let Some(v) = opt_arg::<bool>(&args, "enabled")? {
        s.enabled = v;
    }
    if let Some(v) = opt_arg::<f32>(&args, "strength")? {
        s.strength = v;
    }
    if let Some(v) = opt_arg::<f32>(&args, "range")? {
        s.range = v;
    }
    if let Some(v) = opt_arg::<usize>(&args, "max_live")? {
        s.max_live = v;
    }
    if let Some(v) = opt_arg::<f32>(&args, "sound")? {
        s.sound = v;
    }
    let s = s.clamp();
    if s != before {
        crate::prefs::set("auras", serde_json::to_value(&s).unwrap_or(Value::Null));
        world.insert_resource(s.clone());
    }
    let cam = world
        .query_filtered::<&GlobalTransform, With<MainCamera>>()
        .iter(world)
        .next()
        .map(|g| g.translation());
    let au = world.resource::<Auras>();
    let live: Vec<Value> = au
        .slots
        .iter()
        .filter_map(|sl| {
            let c = sl.cand.as_ref()?;
            Some(json!({
                "object": c.object, "model": c.model, "kind": STYLES[c.style].kind,
                "position": [r2(c.pos.x), r2(c.pos.y), r2(c.pos.z)], "radius": r2(c.radius), "height": r2(c.height),
                "distance": cam.map(|p| r2(c.reach(p))), "fade": r2(sl.level), "glow_parts": sl.pulse_parts.len(),
            }))
        })
        .collect();
    let wanted: Vec<(&'static str, f32)> = au.sounds.clone();
    let (total, clock) = (au.cands.len(), au.clock);
    let mut q = world.query::<(&Name, Option<&AudioSink>)>();
    let playing: HashMap<String, (f32, f32, bool)> = q
        .iter(world)
        .filter_map(|(n, sink)| {
            let stem = n.as_str().strip_prefix("Ambience ")?;
            let sink = sink?;
            Some((
                stem.to_string(),
                (
                    sink.volume().to_linear(),
                    sink.position().as_secs_f32(),
                    sink.is_paused(),
                ),
            ))
        })
        .collect();
    let sounds: Vec<Value> = sound_stems()
        .into_iter()
        .map(|stem| {
            let w = wanted.iter().find(|x| x.0 == stem).map(|x| x.1).unwrap_or(0.0);
            let p = playing.get(stem);
            json!({ "stem": stem, "wanted": r2(w), "playing": p.is_some_and(|p| !p.2), "volume": p.map(|p| r2(p.0)), "position_s": p.map(|p| r2(p.1)) })
        })
        .collect();
    // (clock_s: the auras' own clock, which runs only while the world animates)
    Ok(vec![McpContent::Json(
        json!({ "settings": s, "auras_placed": total, "clock_s": r2(clock), "live": live, "sounds": sounds }),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(object: u64, x: f32) -> Cand {
        Cand {
            object,
            model: "hp:dark_spire".into(),
            style: 0,
            pos: Vec3::new(x, 0.0, 0.0),
            radius: 7.5,
            height: 24.0,
            pulse: None,
            phase: 0.0,
        }
    }

    #[test]
    fn the_nearest_auras_in_range_come_alive() {
        let cands = vec![cand(1, 300.0), cand(2, 50.0), cand(3, 20.0), cand(4, 120.0)];
        assert_eq!(wanted(&cands, Vec3::ZERO, 180.0, 2), vec![2, 1]);
        assert_eq!(wanted(&cands, Vec3::ZERO, 180.0, 6), vec![2, 1, 3]);
        assert!(wanted(&cands, Vec3::new(1000.0, 0.0, 0.0), 180.0, 4).is_empty());
        // reach: measured to the aura's column, not its foot
        let c = cand(1, 0.0);
        assert!((c.reach(Vec3::new(10.0, 12.0, 0.0)) - 10.0).abs() < 1e-5);
        assert!((c.reach(Vec3::new(0.0, 30.0, 0.0)) - (30.0 - 24.0 * 0.6)).abs() < 1e-4);
    }

    #[test]
    fn sound_fades_in_as_you_approach() {
        assert_eq!(sound_falloff(5.0, 7.5), 1.0);
        assert_eq!(sound_falloff(200.0, 7.5), 0.0);
        let (a, b, c) = (
            sound_falloff(20.0, 7.5),
            sound_falloff(40.0, 7.5),
            sound_falloff(60.0, 7.5),
        );
        assert!(a > b && b > c && c > 0.0, "{a} {b} {c}");
    }

    #[test]
    fn styles_and_settings() {
        let s = style("aura_dark").unwrap();
        assert!(s.sounds.iter().all(|(stem, _)| {
            crate::asset_file(&format!("audio/ambience/{stem}.mp3"))
                .is_file()
        }));
        assert!(style("fire").is_none());
        let c = AuraSettings {
            strength: 9.0,
            range: 1.0,
            max_live: 0,
            sound: -1.0,
            enabled: true,
        }
        .clamp();
        assert_eq!(
            (c.strength, c.range, c.max_live, c.sound),
            (2.0, 40.0, 1, 0.0)
        );
        // old or partial prefs fill in the defaults
        let p: AuraSettings = serde_json::from_value(json!({ "strength": 0.5 })).unwrap();
        assert_eq!((p.strength, p.range, p.enabled), (0.5, 180.0, true));
        let b = breath(0.0, 4.0, 0.0);
        assert!((0.0..=1.0).contains(&b));
    }

    #[test]
    fn particle_layers() {
        let m = layer_mesh(&[(0, WISPS), (1, MIST)], 1);
        assert_eq!(m.count_vertices(), (WISPS + MIST) * 4);
        assert_eq!(m.indices().map(|i| i.len()), Some((WISPS + MIST) * 6));
    }
}
