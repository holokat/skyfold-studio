//! Characters in exports (every target): `characters/<id>.glb` for each character ticked
//! "Include in exports" or used by the world (an NPC's or creature's model, the player), holding
//! its mesh, the standard skeleton (Unreal 5 mannequin bone names, so Unreal's retargeter and
//! Unity's / Godot's humanoid mapping take it as is), the skin and its animations baked onto it
//! (its chosen set plus every clip its moves use); and `characters/characters.json`: the
//! characters, their moves, the player, every NPC with its character, behavior, schedule,
//! interactions and dialogue, and which creatures look like which character.

use std::path::PathBuf;

use autorig::retarget::{PlayOpts, Retarget};
use bevy::prelude::*;
use serde_json::{Value, json};

use super::moves::{self, Move};
use super::{Characters, WorldCharacters};
use crate::export::core::Ctx;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub characters: Vec<Item>,
    pub player: Option<String>,
    /// (npc id, name, role, position, facing, life)
    pub npcs: Vec<(u32, String, String, [f32; 3], f32, super::npc::NpcLife)>,
    /// (creature id, name, character)
    pub creatures: Vec<(u32, String, String)>,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub file: PathBuf,
    pub height: f32,
    pub anims: Vec<String>,
    /// move id → animation key
    pub moves: Vec<(String, String)>,
}

pub fn snapshot(world: &World) -> Snapshot {
    if !super::labs_enabled() {
        return Snapshot::default();
    }
    let (Some(chars), Some(wc), Some(gp)) = (
        world.get_resource::<Characters>(),
        world.get_resource::<WorldCharacters>(),
        world.get_resource::<crate::gameplay::Gameplay>(),
    ) else {
        return Snapshot::default();
    };
    let used = |id: &str| {
        wc.player.as_deref() == Some(id)
            || gp.npcs.iter().any(|n| n.life.character.as_deref() == Some(id))
            || gp.creatures.iter().any(|c| c.character.as_deref() == Some(id))
    };
    let characters = chars
        .entries
        .iter()
        .filter(|e| e.rigged() && (e.meta.export || used(&e.meta.id)))
        .filter_map(|e| {
            let mut anims = e.meta.export_anims.clone();
            let mv: Vec<(String, String)> = Move::ALL
                .iter()
                .map(|m| (m.id().to_string(), moves::anim_for(&e.meta.moves, *m)))
                .collect();
            let npc_moves = gp
                .npcs
                .iter()
                .filter(|n| n.life.character.as_deref() == Some(e.meta.id.as_str()))
                .flat_map(|n| n.life.moves.values().cloned());
            for k in mv.iter().map(|(_, k)| k.clone()).chain(npc_moves) {
                if !anims.contains(&k) {
                    anims.push(k);
                }
            }
            Some(Item {
                id: e.meta.id.clone(),
                name: e.meta.name.clone(),
                file: e.rig_file()?,
                height: e.meta.height,
                anims,
                moves: mv,
            })
        })
        .collect();
    Snapshot {
        characters,
        player: wc.player.clone(),
        npcs: gp
            .npcs
            .iter()
            .filter(|n| n.life.character.is_some())
            .map(|n| (n.id, n.name.clone(), n.role.name().to_string(), n.pos, n.yaw, n.life.clone()))
            .collect(),
        creatures: gp
            .creatures
            .iter()
            .filter_map(|c| Some((c.id, c.name.clone(), c.character.clone()?)))
            .collect(),
    }
}

/// Writes the characters (off the main thread). `None` when there are none.
pub fn write(s: &Snapshot, ctx: &Ctx) -> Result<Option<Value>, String> {
    if s.characters.is_empty() {
        return Ok(None);
    }
    ctx.stage("Characters", 0.97)?;
    let lib = super::anims::load_all();
    let mut out = vec![];
    for c in &s.characters {
        ctx.check()?;
        let model = match autorig::import::load(&c.file) {
            Ok(m) => m,
            Err(e) => {
                ctx.warn(format!("character {}: {e}", c.name));
                continue;
            }
        };
        let Some(sk) = model.skeleton.clone() else {
            ctx.warn(format!("character {}: not rigged", c.name));
            continue;
        };
        let mut clips = vec![];
        let mut names = vec![];
        for key in &c.anims {
            let Some((src, clip)) = key.split_once('/').and_then(|(sid, cname)| {
                let s = lib.iter().find(|s| s.id == sid)?;
                let c = s.clips.iter().find(|c| c.name == cname)?;
                Some((s, c))
            }) else {
                ctx.warn(format!("character {}: no animation {key}", c.name));
                continue;
            };
            let r = Retarget::new(&src.skeleton, &sk);
            // (locomotion on the spot: the engine moves the character)
            let mut baked = r.bake(
                clip,
                PlayOpts {
                    looped: false,
                    in_place: true,
                },
            );
            baked.name = clip_name(key, &names);
            names.push(baked.name.clone());
            clips.push(baked);
        }
        let rel = format!("characters/{}.glb", c.id);
        ctx.write(&rel, &autorig::glb::write(&model, &clips))?;
        out.push(json!({
            "id": c.id,
            "name": c.name,
            "file": rel,
            "height_m": c.height,
            "animations": names,
            "moves": c.moves.iter().map(|(m, k)| (m.clone(), k.rsplit('/').next().unwrap_or(k).to_string())).collect::<std::collections::BTreeMap<_, _>>(),
        }));
    }
    let clip = |k: &str| k.rsplit('/').next().unwrap_or(k).to_string();
    let npcs: Vec<Value> = s
        .npcs
        .iter()
        .map(|(id, name, role, pos, yaw, life)| {
            json!({
                "id": id,
                "name": name,
                "role": role,
                "character": life.character,
                "position": pos,
                // compass bearing it faces: 0 north (−z), 90 east (+x)
                "facing_deg": crate::mcp_tools::facing_bearing(*yaw),
                "behavior": life.behavior,
                "wander_radius_m": life.wander_radius,
                "route": life.route,
                "route_loop": life.route_loop,
                "schedule": life.schedule,
                "leash_m": life.leash,
                "speed": life.speed,
                "attitude": life.attitude,
                "faction": life.faction,
                "interactions": {
                    "talk": life.can_talk, "trade": life.can_trade, "quests": life.gives_quests,
                    "join_party": life.can_join, "attackable": life.attackable,
                },
                "dialogue": life.lines,
                "moves": life.moves.iter().map(|(m, k)| (m.clone(), clip(k))).collect::<std::collections::BTreeMap<_, _>>(),
            })
        })
        .collect();
    let creatures: Vec<Value> = s
        .creatures
        .iter()
        .map(|(id, name, c)| json!({"id": id, "name": name, "character": c}))
        .collect();
    let doc = json!({
        "about": "Characters from Skyfold Studio (Labs). Coordinates as the world's GLB models: metres, +Y up, glTF axes. Skeleton: Unreal Engine 5 mannequin bone names (root, pelvis, spine_01..03, neck_01, Head, clavicle/upperarm/lowerarm/hand_l/r, fingers, thigh/calf/foot/ball_l/r). Animation names are the clips in each GLB; `moves` say which clip each state plays. Bundled animations: Quaternius Universal Animation Library (CC0).",
        "characters": out,
        "player": s.player,
        "npcs": npcs,
        "creatures": creatures,
    });
    ctx.write_json("characters/characters.json", &doc)?;
    Ok(Some(json!({
        "characters": s.characters.len(),
        "npcs": s.npcs.len(),
    })))
}

/// A clip's name in the export: the library clip's own, unique within the file.
fn clip_name(key: &str, taken: &[String]) -> String {
    let base = key.rsplit('/').next().unwrap_or(key).to_string();
    let mut n = base.clone();
    let mut i = 2;
    while taken.contains(&n) {
        n = format!("{base}_{i}");
        i += 1;
    }
    n
}
