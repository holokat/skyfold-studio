//! Named glTF animation clips on library models: a placed model (previews, MCP `model_clip`) or
//! an equipped weapon (`crate::characters::weapons`) plays one of its file's clips once, in a
//! loop, backwards (a blade going out), or held at a moment (a bow at full draw).
//!
//! Bevy's glTF loader puts an `AnimationPlayer` on the scene's animation root and tags every
//! animated node; nothing plays until something asks. `ModelClips` on the model's root entity
//! (the one holding its `WorldAssetRoot`) loads the file's `Gltf` asset, builds one animation
//! graph with a node per named clip once the scene is in, and applies the latest request. Models
//! nobody asks to animate never get a graph, so the hand-driven parts (`spin`, `swing`, `door`,
//! `lift`, `lid`) keep their nodes to themselves; a clip asked for on such a model moves the same
//! nodes for as long as it plays.

use std::collections::HashMap;

use bevy::animation::RepeatAnimation;
use bevy::gltf::Gltf;
use bevy::prelude::*;

/// How a clip plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClipMode {
    /// once, holding its last frame
    Once,
    Loop,
    /// once backwards, from its end to its start (sheathing what `ignite` lit)
    Reverse,
    /// stopped at this time (s, clamped into the clip)
    Hold(f32),
}

impl ClipMode {
    pub fn id(self) -> &'static str {
        match self {
            ClipMode::Once => "once",
            ClipMode::Loop => "loop",
            ClipMode::Reverse => "reverse",
            ClipMode::Hold(_) => "hold",
        }
    }
    /// From an MCP / JSON id (`hold` needs its time).
    pub fn from_id(id: &str, at: f32) -> Option<ClipMode> {
        Some(match id {
            "once" => ClipMode::Once,
            "loop" => ClipMode::Loop,
            "reverse" => ClipMode::Reverse,
            "hold" => ClipMode::Hold(at),
            _ => return None,
        })
    }
}

/// A request: play `clip` (None: stop every clip, the parts stay where they are).
#[derive(Clone, Debug, PartialEq)]
pub struct ClipRequest {
    pub clip: Option<String>,
    pub mode: ClipMode,
    pub speed: f32,
}

/// Clip playback for one model instance (put it on the entity holding the `WorldAssetRoot`).
#[derive(Component)]
pub struct ModelClips {
    gltf: Handle<Gltf>,
    /// the scene's animation player, once found
    player: Option<Entity>,
    /// clip name → (graph node, length s)
    nodes: HashMap<String, (AnimationNodeIndex, f32)>,
    ready: bool,
    pending: Option<ClipRequest>,
    /// what plays now
    pub current: Option<(String, ClipMode)>,
    /// bumped each time a request is applied (callers wait on it)
    pub applied: u32,
}

impl ModelClips {
    /// For the model whose glTF is at `path` (an assets path, as `LibraryEntry::import_path`).
    pub fn new(server: &AssetServer, path: &str) -> Self {
        Self {
            gltf: server.load(path.to_string()),
            player: None,
            nodes: HashMap::new(),
            ready: false,
            pending: None,
            current: None,
            applied: 0,
        }
    }
    /// Plays `clip` (it waits for the model to load).
    pub fn play(&mut self, clip: &str, mode: ClipMode) {
        self.play_at(clip, mode, 1.0);
    }
    pub fn play_at(&mut self, clip: &str, mode: ClipMode, speed: f32) {
        self.pending = Some(ClipRequest {
            clip: Some(clip.to_string()),
            mode,
            speed,
        });
    }
    pub fn stop(&mut self) {
        self.pending = Some(ClipRequest {
            clip: None,
            mode: ClipMode::Once,
            speed: 1.0,
        });
    }
    /// The file's clips with their lengths (s), once loaded.
    pub fn clips(&self) -> Vec<(String, f32)> {
        let mut v: Vec<(String, f32)> = self.nodes.iter().map(|(k, v)| (k.clone(), v.1)).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }
    pub fn ready(&self) -> bool {
        self.ready
    }
    pub fn has(&self, clip: &str) -> bool {
        self.nodes.contains_key(clip)
    }
    #[allow(dead_code)]
    pub fn length(&self, clip: &str) -> Option<f32> {
        self.nodes.get(clip).map(|n| n.1)
    }
    /// Nothing is waiting to be applied.
    pub fn idle(&self) -> bool {
        self.pending.is_none()
    }
    /// The scene's animation player entity, once found.
    pub fn player(&self) -> Option<Entity> {
        self.player
    }
}

/// What a clip is doing right now (for `weapon_state` / `model_clip`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipStatus {
    pub clip: Option<String>,
    pub mode: Option<&'static str>,
    pub time: f32,
    pub finished: bool,
}

/// The status of `mc` (`player`: its animation player, `ModelClips::player`).
pub fn status(mc: &ModelClips, player: Option<&AnimationPlayer>) -> ClipStatus {
    let Some((name, mode)) = &mc.current else {
        return ClipStatus::default();
    };
    let mut st = ClipStatus {
        clip: Some(name.clone()),
        mode: Some(mode.id()),
        ..default()
    };
    if let (Some(p), Some((node, _))) = (player, mc.nodes.get(name))
        && let Some(a) = p.animation(*node)
    {
        st.time = a.seek_time();
        st.finished = a.is_finished();
    }
    st
}

/// Builds graphs once models are in and applies requests.
pub fn drive(
    mut models: Query<(Entity, &mut ModelClips)>,
    children: Query<&Children>,
    mut players: Query<&mut AnimationPlayer>,
    gltfs: Res<Assets<Gltf>>,
    clips: Res<Assets<AnimationClip>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut commands: Commands,
) {
    for (root, mut mc) in &mut models {
        if !mc.ready {
            let Some(g) = gltfs.get(&mc.gltf) else {
                continue;
            };
            let Some(player) = std::iter::once(root)
                .chain(children.iter_descendants(root))
                .find(|e| players.contains(*e))
            else {
                continue;
            };
            let mut graph = AnimationGraph::new();
            let mut nodes = HashMap::new();
            for (name, h) in &g.named_animations {
                let len = clips.get(h).map(|c| c.duration()).unwrap_or(0.0);
                let n = graph.add_clip(h.clone(), 1.0, graph.root);
                nodes.insert(name.to_string(), (n, len));
            }
            commands
                .entity(player)
                .insert(AnimationGraphHandle(graphs.add(graph)));
            mc.nodes = nodes;
            mc.player = Some(player);
            mc.ready = true;
            // (the graph handle lands with the commands: apply from the next frame on)
            continue;
        }
        // (the scene can be spawned again, e.g. when its textures are swapped for compressed
        // ones: find the new player, build its graph, and replay what was playing)
        if mc.player.is_none_or(|pe| !players.contains(pe)) {
            mc.ready = false;
            mc.player = None;
            if mc.pending.is_none()
                && let Some((c, m)) = mc.current.take()
            {
                mc.pending = Some(ClipRequest {
                    clip: Some(c),
                    mode: m,
                    speed: 1.0,
                });
            }
            continue;
        }
        let Some(req) = mc.pending.take() else {
            continue;
        };
        let Some(pe) = mc.player else { continue };
        let Ok(mut p) = players.get_mut(pe) else {
            continue;
        };
        p.stop_all();
        mc.applied = mc.applied.wrapping_add(1);
        let Some(name) = req.clip else {
            mc.current = None;
            continue;
        };
        let Some(&(node, len)) = mc.nodes.get(&name) else {
            warn!("model clip `{name}` not in the file");
            mc.current = None;
            continue;
        };
        let speed = if req.speed.is_finite() && req.speed > 0.0 {
            req.speed
        } else {
            1.0
        };
        let a = p.start(node);
        match req.mode {
            ClipMode::Once => {
                a.set_repeat(RepeatAnimation::Never).set_speed(speed);
            }
            ClipMode::Loop => {
                a.set_repeat(RepeatAnimation::Forever).set_speed(speed);
            }
            ClipMode::Reverse => {
                a.set_repeat(RepeatAnimation::Never)
                    .set_speed(-speed)
                    .seek_to(len);
            }
            ClipMode::Hold(t) => {
                a.set_repeat(RepeatAnimation::Never)
                    .seek_to(t.clamp(0.0, len))
                    .pause();
            }
        }
        mc.current = Some((name, req.mode));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip() {
        for m in [ClipMode::Once, ClipMode::Loop, ClipMode::Reverse, ClipMode::Hold(0.4)] {
            assert_eq!(ClipMode::from_id(m.id(), 0.4), Some(m));
        }
        assert_eq!(ClipMode::from_id("spin", 0.0), None);
    }
}

// ------------------------------------------------------------------------------------- MCP

use serde_json::{Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool, arg, opt_arg};

pub fn register(app: &mut App) {
    app.add_systems(Update, drive);
    app.mcp_tool(McpTool {
        name: "model_clip",
        description: "Play one of a placed model's own glTF animation clips (previews: a weapon's `fire` / `reload`, a blade's `ignite`, a bow's `draw` / `hold` / `release`, any model's named clips). `id`: the placed object (objects_query). `clip`: the clip's name; leave it out to only list the clips. `mode`: once (default; holds the last frame), loop, reverse (once backwards, e.g. a blade going out), hold (stopped at `at` seconds, e.g. a bow at full draw). `speed` (default 1). `stop: true` stops every clip (the parts stay where they are). Waits for the model to load; returns its clips with their lengths and what plays. Clips only move the model's own parts; spinning sails, rocking lanterns, doors and lifts keep working as before.",
        input_schema: || {
            json!({ "type": "object", "properties": {
                "id": { "type": "integer", "description": "placed object id" },
                "clip": { "type": "string" },
                "mode": { "type": "string", "enum": ["once", "loop", "reverse", "hold"] },
                "at": { "type": "number", "description": "hold: the time to stop at (s)" },
                "speed": { "type": "number" },
                "stop": { "type": "boolean" } }, "required": ["id"] })
        },
        run: model_clip,
    });
}

fn model_clip(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: u64 = arg(&args, "id")?;
    let path = {
        let store = world.resource::<crate::objects::ObjectStore>();
        let p = store.items.get(&id).ok_or_else(|| format!("no placed object {id}"))?;
        let lib = world
            .get_resource::<crate::objects::Library>()
            .ok_or("the library isn't loaded yet")?;
        lib.get(&p.asset)
            .and_then(|e| e.import_path.clone())
            .ok_or_else(|| format!("{} is not a glTF model (no clips)", p.asset))?
    };
    let ent = world
        .query::<(Entity, &crate::objects::PlacedObject)>()
        .iter(world)
        .find(|(_, po)| po.id == id)
        .map(|(e, _)| e)
        .ok_or_else(|| format!("object {id} isn't spawned"))?;
    if world.get::<ModelClips>(ent).is_none() {
        let mc = ModelClips::new(world.resource::<AssetServer>(), &path);
        world.entity_mut(ent).insert(mc);
    }
    let clip = opt_arg::<String>(&args, "clip")?;
    let mode = opt_arg::<String>(&args, "mode")?.unwrap_or_else(|| "once".into());
    let at = opt_arg::<f32>(&args, "at")?.unwrap_or(0.0);
    let mode = ClipMode::from_id(&mode, at).ok_or_else(|| format!("unknown mode `{mode}`"))?;
    let speed = opt_arg::<f32>(&args, "speed")?.unwrap_or(1.0);
    let stop = opt_arg::<bool>(&args, "stop")?.unwrap_or(false);
    {
        let mut mc = world.get_mut::<ModelClips>(ent).unwrap();
        if stop {
            mc.stop();
        } else if let Some(c) = &clip {
            mc.play_at(c, mode, speed);
        }
    }
    let mut frames = 0u32;
    Ok(vec![McpContent::deferred(move |world: &mut World| {
        frames += 1;
        let Some(mc) = world.get::<ModelClips>(ent) else {
            return Some(Err("the object went away".into()));
        };
        if !(mc.ready() && mc.idle()) {
            return (frames > 1200).then(|| Err("the model didn't load in time".into()));
        }
        if let Some(c) = &clip
            && !stop
            && !mc.has(c)
        {
            return Some(Err(format!(
                "no clip `{c}`; the model has: {}",
                mc.clips().iter().map(|c| c.0.as_str()).collect::<Vec<_>>().join(", ")
            )));
        }
        let player = mc.player().and_then(|p| world.get::<AnimationPlayer>(p));
        let st = status(mc, player);
        Some(Ok(vec![McpContent::Json(json!({
            "id": id,
            "clips": mc.clips().iter().map(|(n, l)| json!({ "name": n, "length_s": (l * 1000.0).round() / 1000.0 })).collect::<Vec<_>>(),
            "playing": st.clip, "mode": st.mode,
        }))]))
    })])
}
