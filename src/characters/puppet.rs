//! A spawned, animated character: its rigged glTF scene, with every bone's transform written
//! each frame from a library clip retargeted onto its skeleton (no Bevy animation graph: one
//! clip at a time, retargeted on the fly, so any clip plays on any character).

use std::collections::HashMap;
use std::sync::Arc;

use autorig::retarget::{PlayOpts, Retarget};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;

use super::rigging::{Gaze, RigRuntime, RigSpecs, WorldHost};
use super::{AnimLibrary, Characters, Entry};

#[derive(Component, Debug)]
pub struct Puppet {
    pub character: String,
    pub rig_key: String,
    /// animation library key; `None` holds the rest pose
    pub anim: Option<String>,
    pub time: f32,
    pub speed: f32,
    pub playing: bool,
    /// keep the hips over the spot (locomotion clips walk on the spot)
    pub in_place: bool,
    /// render layer for the scene's meshes (the preview stage); `None`: the world's
    pub layer: Option<usize>,
    bones: Vec<Option<Entity>>,
    resolved: bool,
    /// the scene: shown once it and its textures have loaded
    scene: Handle<bevy::world_serialization::WorldAsset>,
    loaded: bool,
    /// play the clip once and hold its last frame (`finished` then turns true)
    pub once: bool,
    pub finished: bool,
    /// the clip it's fading out of: (key, time), and the fade left (s)
    prev: Option<(String, f32)>,
    fade: f32,
    /// turns its head to the viewer / player (off for the player's own body)
    pub gaze: bool,
    /// its rig behaviors (feet, look-at, style, moving parts), built once its skeleton is in
    pub rig: Option<RigRuntime>,
    /// a clip for the upper body (spine, arms, head) over the moves: holding a weapon ready,
    /// aiming, reloading while walking
    pub upper: Option<UpperLayer>,
    /// the pose round a held weapon (`weapons::hold`): spine pitch, arms on the aim line
    pub hold: Option<super::weapons::hold::Hold>,
    /// the held weapon's frame this frame (model space), written when `hold` is set
    pub held: Option<Mat4>,
    /// which bones the upper layer drives (from `spine_01` up), once resolved
    upper_mask: Vec<bool>,
}

/// An upper-body clip over the moves (`Puppet::upper`).
#[derive(Clone, Debug, PartialEq)]
pub struct UpperLayer {
    pub anim: String,
    pub time: f32,
    pub speed: f32,
    /// no loop: hold the end (`finished` then turns true)
    pub once: bool,
    pub finished: bool,
    /// how much it covers the moves (eases toward `target`)
    pub weight: f32,
    pub target: f32,
}

impl UpperLayer {
    pub fn new(anim: &str, once: bool) -> Self {
        Self {
            anim: anim.to_string(),
            time: 0.0,
            speed: 1.0,
            once,
            finished: false,
            weight: 0.0,
            target: 1.0,
        }
    }
}

/// How fast the upper layer fades in and out (weight per second).
const UPPER_FADE: f32 = 6.0;

/// The bones from `spine_01` up (the UE / UAL standard skeleton).
fn upper_mask(sk: &autorig::Skeleton) -> Vec<bool> {
    let mut m = vec![false; sk.bones.len()];
    for (i, b) in sk.bones.iter().enumerate() {
        m[i] = b.name == "spine_01" || b.parent.is_some_and(|p| p < i && m[p]);
    }
    m
}

/// Crossfade between clips (s).
const FADE: f32 = 0.2;

impl Puppet {
    pub fn new(
        e: &Entry,
        anim: Option<String>,
        layer: Option<usize>,
        scene: Handle<bevy::world_serialization::WorldAsset>,
    ) -> Self {
        Self {
            character: e.meta.id.clone(),
            rig_key: e.rig_key(),
            anim,
            time: 0.0,
            speed: 1.0,
            playing: true,
            in_place: true,
            layer,
            bones: vec![],
            resolved: false,
            scene,
            loaded: false,
            once: false,
            finished: false,
            prev: None,
            fade: 0.0,
            gaze: true,
            rig: None,
            upper: None,
            hold: None,
            held: None,
            upper_mask: vec![],
        }
    }
    /// Plays `key` (crossfading from what was playing); `once`: no loop, hold the end.
    pub fn play(&mut self, key: &str, once: bool) {
        if self.anim.as_deref() == Some(key) {
            self.once = once;
            return;
        }
        self.prev = self.anim.take().map(|a| (a, self.time));
        self.fade = if self.prev.is_some() { FADE } else { 0.0 };
        self.anim = Some(key.to_string());
        self.time = 0.0;
        self.once = once;
        self.finished = false;
    }
    /// Plays `key` again from its start (a second attack of the same kind).
    pub fn restart(&mut self, key: &str, once: bool) {
        if self.anim.as_deref() == Some(key) {
            self.prev = Some((key.to_string(), self.time));
            self.fade = FADE;
            self.time = 0.0;
            self.finished = false;
            self.once = once;
        } else {
            self.play(key, once);
        }
    }
    /// Sets the upper-body clip (`None` fades it out); the same clip keeps playing.
    pub fn set_upper(&mut self, anim: Option<&str>, once: bool) {
        match (anim, self.upper.as_mut()) {
            (Some(a), Some(u)) if u.anim == a => {
                u.target = 1.0;
                u.once = once;
            }
            (Some(a), u) => {
                // (from another upper clip: cut over at the weight it had)
                let w = u.map_or(0.0, |u| u.weight * 0.5);
                let mut l = UpperLayer::new(a, once);
                l.weight = w;
                self.upper = Some(l);
            }
            (None, Some(u)) => u.target = 0.0,
            (None, None) => {}
        }
    }
    /// Plays the upper-body clip `anim` again from its start.
    pub fn restart_upper(&mut self, anim: &str, once: bool) {
        self.set_upper(Some(anim), once);
        if let Some(u) = self.upper.as_mut() {
            u.time = 0.0;
            u.finished = false;
        }
    }
    /// Spawned, posed and its textures in.
    pub fn ready(&self) -> bool {
        self.resolved && self.loaded
    }
}

/// Spawns `e` (rigged) as a puppet at `tf`.
pub fn spawn(
    commands: &mut Commands,
    server: &AssetServer,
    e: &Entry,
    tf: Transform,
    anim: Option<String>,
    layer: Option<usize>,
) -> Option<Entity> {
    let path = e.display_path();
    let scene = server.load(GltfAssetLabel::Scene(0).from_asset(path));
    let mut ec = commands.spawn((
        Name::new(format!("Character {}", e.meta.name)),
        tf,
        Visibility::Inherited,
        Puppet::new(e, anim, layer, scene.clone()),
    ));
    if let Some(l) = layer {
        ec.insert(RenderLayers::layer(l));
    }
    ec.with_child(WorldAssetRoot(scene));
    Some(ec.id())
}

/// Retargeters, one per (character rig, animation source).
#[derive(Resource, Default)]
pub struct Retargets(HashMap<(String, String), Arc<Retarget>>);

/// Puppets posing (weapons set their holds before it).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PuppetSystems;

pub struct PuppetPlugin;

impl Plugin for PuppetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Retargets>()
            .add_systems(
                Update,
                drive
                    .in_set(PuppetSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut puppets: Query<(Entity, &mut Puppet)>,
    children: Query<&Children>,
    names: Query<&Name>,
    meshes: Query<(), With<Mesh3d>>,
    mut tfs: Query<&mut Transform>,
    mut chars: ResMut<Characters>,
    mut lib: ResMut<AnimLibrary>,
    mut cache: ResMut<Retargets>,
    time: Res<Time>,
    server: Res<AssetServer>,
    mut commands: Commands,
    (mut specs, gaze, terrain, wind, globals): (
        ResMut<RigSpecs>,
        Res<Gaze>,
        Option<Res<crate::terrain::Terrain>>,
        Option<Res<crate::wind::WindState>>,
        Query<&GlobalTransform>,
    ),
) {
    if puppets.is_empty() {
        return;
    }
    let lib_ready = lib.ensure();
    let dt = time.delta_secs().min(0.1);
    for (root, mut p) in &mut puppets {
        let Some(entry) = chars.get(&p.character).cloned() else {
            continue;
        };
        let sk = if entry.rigged() {
            match chars.skeleton(&entry) {
                Some(sk) => sk,
                None => continue,
            }
        } else {
            // (not rigged yet: shown as modelled)
            Arc::new(autorig::Skeleton::default())
        };
        if !p.loaded && server.is_loaded_with_dependencies(&p.scene) {
            p.loaded = true;
        }
        if !p.resolved {
            // the scene's bones, by name, once it has spawned
            let mut by_name: HashMap<&str, Entity> = HashMap::new();
            let mut parts = vec![];
            for d in children.iter_descendants(root) {
                if let Ok(n) = names.get(d) {
                    by_name.insert(n.as_str(), d);
                }
                if meshes.contains(d) {
                    parts.push(d);
                }
            }
            let bones: Vec<Option<Entity>> = sk
                .bones
                .iter()
                .map(|b| by_name.get(b.name.as_str()).copied())
                .collect();
            if parts.is_empty() || bones.iter().filter(|b| b.is_some()).count() * 2 < bones.len()
            {
                continue;
            }
            for e in parts {
                // (animated skinned meshes leave their rest-pose bounds)
                let mut ec = commands.entity(e);
                ec.insert(NoFrustumCulling);
                if let Some(l) = p.layer {
                    ec.insert(RenderLayers::layer(l));
                }
            }
            debug!(
                "character {}: {} of {} bones found",
                p.character,
                bones.iter().filter(|b| b.is_some()).count(),
                bones.len()
            );
            p.bones = bones;
            p.upper_mask = upper_mask(&sk);
            p.resolved = true;
        }
        if p.playing {
            p.time += dt * p.speed;
        }
        if sk.bones.is_empty() {
            continue;
        }
        let Some(key) = p.anim.clone() else {
            // the rest pose (the model as it was modelled)
            for (b, bone) in p.bones.iter().zip(&sk.bones) {
                if let Some(b) = b
                    && let Ok(mut tf) = tfs.get_mut(*b)
                {
                    *tf = Transform {
                        translation: bone.rest.t,
                        rotation: bone.rest.r,
                        scale: bone.rest.s,
                    };
                }
            }
            continue;
        };
        if !lib_ready {
            continue;
        }
        let (rig_key, in_place) = (p.rig_key.clone(), p.in_place);
        let mut pose_of = |key: &str, t: f32, once: bool| {
            let anim = lib.find(key)?.clone();
            let (source, clip) = lib.clip(&anim);
            let r = cache
                .0
                .entry((rig_key.clone(), source.id.clone()))
                .or_insert_with(|| Arc::new(Retarget::new(&source.skeleton, &sk)))
                .clone();
            let looped = anim.looped && !once;
            let pose = r.pose(clip, t, PlayOpts { looped, in_place });
            Some((pose, clip.duration(), looped))
        };
        let once = p.once;
        let Some((mut pose, duration, looped)) = pose_of(&key, p.time, once) else {
            continue;
        };
        if !looped {
            if p.time >= duration {
                p.finished = true;
            }
            if once {
                p.time = p.time.min(duration);
            } else if p.time > duration + 0.6 {
                // (previewing a one-shot: it replays after a short hold)
                p.time = 0.0;
            }
        }
        // fading out of the previous clip
        if p.fade > 0.0
            && let Some((pk, pt)) = p.prev.clone()
        {
            let pt = pt + dt * p.speed;
            if let Some((old, _, _)) = pose_of(&pk, pt, false) {
                let w = (p.fade / FADE).clamp(0.0, 1.0);
                for (x, o) in pose.iter_mut().zip(&old) {
                    x.t = x.t.lerp(o.t, w);
                    x.r = x.r.slerp(if x.r.dot(o.r) < 0.0 { -o.r } else { o.r }, w);
                    x.s = x.s.lerp(o.s, w);
                }
            }
            p.prev = Some((pk, pt));
            p.fade -= dt;
        } else {
            p.prev = None;
        }
        // the upper body's own clip over the moves
        if let Some(mut u) = p.upper.take() {
            if p.playing {
                u.time += dt * u.speed;
            }
            let step = UPPER_FADE * dt;
            u.weight = if u.target > u.weight {
                (u.weight + step).min(u.target)
            } else {
                (u.weight - step).max(u.target)
            };
            if let Some((up, dur, looped)) = pose_of(&u.anim, u.time, u.once) {
                if !looped && u.time >= dur {
                    u.finished = true;
                    u.time = u.time.min(dur);
                }
                let w = u.weight.clamp(0.0, 1.0);
                for ((x, o), m) in pose.iter_mut().zip(&up).zip(&p.upper_mask) {
                    if *m {
                        x.t = x.t.lerp(o.t, w);
                        x.r = x.r.slerp(if x.r.dot(o.r) < 0.0 { -o.r } else { o.r }, w);
                    }
                }
            }
            if u.weight > 0.0 || u.target > 0.0 {
                p.upper = Some(u);
            }
        }
        // the rig on top of the clip: style, feet, look-at, moving parts
        RigRuntime::refresh(&mut p.rig, &entry, &sk, &mut specs);
        let root = globals.get(root).map(|g| g.to_matrix()).unwrap_or(Mat4::IDENTITY);
        let stage = p.layer.is_some();
        let host = WorldHost {
            // (the stage floor is flat)
            terrain: if stage { None } else { terrain.as_deref() },
            look: if !p.gaze {
                None
            } else if stage {
                gaze.stage
            } else {
                gaze.world
            },
            wind: if stage {
                Vec3::ZERO
            } else {
                wind.as_deref()
                    .map(|w| w.velocity(time.elapsed_secs()) * 0.8)
                    .unwrap_or_default()
            },
        };
        let rig_dt = if p.playing { dt } else { 0.0 };
        if let Some(r) = p.rig.as_mut() {
            r.rig.evaluate(&sk, &mut pose, root, rig_dt, &host);
        }
        // a held weapon: the spine leans into the aim, the arms reach for the aim line
        p.held = match p.hold.clone() {
            Some(h) => super::weapons::hold::apply(&sk, &mut pose, &h),
            None => None,
        };
        for (b, x) in p.bones.iter().zip(&pose) {
            if let Some(b) = b
                && let Ok(mut tf) = tfs.get_mut(*b)
            {
                tf.translation = x.t;
                tf.rotation = x.r;
                tf.scale = x.s;
            }
        }
    }
}
