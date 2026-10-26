//! MCP: `weapon_list`, `weapon_equip`, `weapon_unequip`, `weapon_state`, `weapon_use`,
//! `characters_view`.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::{ActionKind, Armory, UseCmd, WeaponCatalog, WeaponDef};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, arg, opt_arg};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "weapon_list",
        description: "The library's weapons (models with a `weapon` dict: the weapons pack): id, name, use (melee / bow / gun), class, hands, sockets, clips with lengths, whether it aims down sights and its zoom, and the weapon sets on the number keys 1-4.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: list,
    })
    .mcp_tool(McpTool {
        name: "weapon_equip",
        description: "Put a library weapon in the player's hand (Walk mode; the Characters lab's player character holds it by its grip socket in the hand bone, a bow in the left hand; in first person, or without a player character, it shows in front of the camera). `id`: the weapon (weapon_list), or leave it out with `slot` to take that set's weapon. `slot` (1-4) also stores `id` in that weapon set (number keys switch sets while walking; the same key again or 0 puts it away). `grip_offset` [x, y, z] (m, weapon space) and `grip_rot_deg` [x, y, z] fine-tune the grip (not saved). Blades play their `ignite` clip once. Returns weapon_state.",
        input_schema: || json!({"type": "object", "properties": {
            "id": {"type": "string"}, "slot": {"type": "integer", "minimum": 1, "maximum": 4},
            "grip_offset": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3},
            "grip_rot_deg": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3}}}),
        run: equip,
    })
    .mcp_tool(McpTool {
        name: "weapon_unequip",
        description: "Put the player's weapon away (the hands are free: unarmed attacks again). Returns weapon_state.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: unequip,
    })
    .mcp_tool(McpTool {
        name: "weapon_state",
        description: "The player's weapon: what's equipped, its use, state (holstered / ready / swing / drawing / drawn / releasing / firing / reloading), raise (0 at the side .. 1 on the aim line), ads (down the sights / zoom 0..1), aiming, the weapon sets, counters (shots, swings, arrows), the clip the weapon plays, the view (third person or first) and the latest actions (the combat hook `WeaponAction`: kind, origin, aim direction, a blade's tip).",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: state,
    })
    .mcp_tool(McpTool {
        name: "weapon_use",
        description: "Use the equipped weapon as the mouse would (applied next frame). `action`: attack (blade: a light attack, `heavy: true` a heavy one; clicking again while one plays chains the combo), draw (bow: start pulling and keep holding: it stays at full draw, `hold` clip), release (bow: loose the arrow), fire (gun: one shot from the firing stance; bow: release), aim (`on` true/false: guns aim down the sights, bows zoom while drawn), reload (gun). No damage or projectiles: each swing, shot and arrow is a WeaponAction (see weapon_state). Returns weapon_state after `wait_frames` (default 2).",
        input_schema: || json!({"type": "object", "properties": {
            "action": {"type": "string", "enum": ["attack", "draw", "release", "fire", "aim", "reload"]},
            "heavy": {"type": "boolean"}, "on": {"type": "boolean"},
            "wait_frames": {"type": "integer", "minimum": 0, "maximum": 600}}, "required": ["action"]}),
        run: use_,
    })
    .mcp_tool(McpTool {
        name: "characters_view",
        description: "The walking player's view: `third_person` true (the camera outside the character, over the shoulder) or false (first person: through the eyes, an equipped weapon in front of the camera). `orbit_deg` turns the third-person camera round the character and `time_scale` slows the world, for previews. Without arguments it reads the view. Needs a player character for third person (characters_set_player) and a camera style that allows first person (characters_camera first_person).",
        input_schema: || json!({"type": "object", "properties": {"third_person": {"type": "boolean"},
            "time_scale": {"type": "number", "minimum": 0.02, "maximum": 1, "description": "preview: slow the whole world down (1 = normal) to catch a swing or a shot in a screenshot"},
            "orbit_deg": {"type": "number", "description": "preview: turn the third-person camera round the character by this many degrees (90: from its left side, 180: from the front); the aim keeps the unturned view. 0 resets."}}}),
        run: view,
    });
}

fn r3(v: Vec3) -> Value {
    let r = |x: f32| (x as f64 * 1000.0).round() / 1000.0;
    json!([r(v.x), r(v.y), r(v.z)])
}

fn def_json(w: &WeaponDef) -> Value {
    json!({
        "id": w.id, "name": w.name, "use": w.usage().id(), "class": w.class, "subclass": w.subclass,
        "hands": w.hands, "hold": w.hold, "support": w.support, "left_hand": w.left_hand,
        "ads": w.ads, "zoom": (w.zoom() as f64 * 100.0).round() / 100.0, "sockets": w.sockets,
        "clips": w.clips.iter().map(|(k, c)| json!({"name": k, "length_s": (c.length as f64 * 1000.0).round() / 1000.0, "loop": c.looped})).collect::<Vec<_>>(),
        "fire_clip": w.fire_clip, "reload_clip": w.reload_clip, "equip_clip": w.equip_clip,
        "draw_clip": w.draw_clip, "hold_clip": w.hold_clip, "release_clip": w.release_clip,
    })
}

fn list(world: &mut World, _: Value) -> Result<Vec<McpContent>, String> {
    let cat = world.resource::<WeaponCatalog>();
    let a = world.resource::<Armory>();
    // (weapons of asset packs this account hasn't got stay out of sight, as in objects_library)
    let market = crate::market::snapshot();
    let usable: Vec<Value> = cat.by_id.values().filter(|w| market.usable(&w.id)).map(def_json).collect();
    let locked = cat.by_id.len() - usable.len();
    Ok(vec![McpContent::Json(json!({
        "weapons": usable,
        "locked_in_packs": locked,
        "sets": a.sets,
    }))])
}

fn state_json(world: &mut World) -> Value {
    let clip = {
        let mut q = world.query::<(&super::drive::HeldWeapon, &crate::model_render::clips::ModelClips)>();
        q.iter(world)
            .next()
            .map(|(_, mc)| (mc.current.clone(), mc.player()))
    };
    let clip = clip.map(|(cur, player)| {
        let p = player.and_then(|p| world.get::<AnimationPlayer>(p));
        let time = cur.as_ref().and_then(|_| p).and_then(|p| p.playing_animations().next().map(|(_, a)| a.seek_time()));
        json!({"clip": cur.as_ref().map(|c| c.0.clone()), "mode": cur.as_ref().map(|c| c.1.id()), "time_s": time})
    });
    let view = world.resource::<crate::characters::player::PlayerView>();
    let (third, seen) = (view.third_person, view.seen);
    let walking = world.resource::<crate::walk::WalkState>().active;
    let cat = world.resource::<WeaponCatalog>();
    let a = world.resource::<Armory>();
    let def = a.equipped.as_ref().and_then(|id| cat.get(id));
    json!({
        "walking": walking,
        "equipped": a.equipped, "name": def.map(|d| d.name.clone()), "use": def.map(|d| d.usage().id()),
        "state": a.state.id(),
        "move": match &a.state { super::WState::Swing { mv, combo, .. } => json!({"id": mv.id, "combo": combo}), _ => Value::Null },
        "raise": (a.raise as f64 * 100.0).round() / 100.0, "ads": (a.ads as f64 * 100.0).round() / 100.0, "view_orbit_deg": a.view_orbit_deg,
        "aiming": a.aiming, "drawing_held": a.mcp_draw,
        "sets": a.sets, "shots": a.shots, "swings": a.swings, "arrows": a.arrows,
        "weapon_clip": clip,
        "view": if seen { "third_person" } else { "first_person" }, "third_person_wanted": third,
        "grip_offset": r3(a.grip_offset), "grip_rot_deg": r3(a.grip_rot_deg),
        "actions": a.log.iter().rev().take(8).map(|x| json!({
            "kind": match x.kind {
                ActionKind::Equip => "equip".to_string(), ActionKind::Unequip => "unequip".into(),
                ActionKind::Swing { heavy, combo } => format!("swing{} {combo}", if heavy { " heavy" } else { "" }),
                ActionKind::Shot => "shot".into(), ActionKind::Arrow { draw } => format!("arrow draw {:.2}", draw),
                ActionKind::Reload => "reload".into(),
            },
            "weapon": x.weapon, "origin": r3(x.origin), "dir": r3(x.dir), "tip": x.tip.map(r3), "time_s": (x.time as f64 * 100.0).round() / 100.0,
        })).collect::<Vec<_>>(),
    })
}

fn state(world: &mut World, _: Value) -> Result<Vec<McpContent>, String> {
    Ok(vec![McpContent::Json(state_json(world))])
}

fn vec3_arg(args: &Value, key: &str) -> Result<Option<Vec3>, String> {
    Ok(opt_arg::<[f32; 3]>(args, key)?.map(Vec3::from_array))
}

/// Answers once the scene had `frames` frames to apply the change.
fn after(frames: u32) -> McpContent {
    let mut n = 0;
    McpContent::deferred(move |world: &mut World| {
        n += 1;
        (n > frames).then(|| Ok(vec![McpContent::Json(state_json(world))]))
    })
}

fn equip(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id = opt_arg::<String>(&args, "id")?.filter(|s| !s.is_empty());
    let slot = opt_arg::<usize>(&args, "slot")?;
    if let Some(s) = slot
        && !(1..=4).contains(&s)
    {
        return Err("slot is 1-4".into());
    }
    {
        let cat = world.resource::<WeaponCatalog>();
        if let Some(i) = &id
            && cat.get(i).is_none()
        {
            let all: Vec<&str> = cat.by_id.keys().map(String::as_str).collect();
            return Err(format!("no weapon `{i}` (weapons: {})", all.join(", ")));
        }
    }
    if let Some(i) = &id {
        crate::market::snapshot().check(i)?;
    }
    let off = vec3_arg(&args, "grip_offset")?;
    let rot = vec3_arg(&args, "grip_rot_deg")?;
    let mut a = world.resource_mut::<Armory>();
    let pick = match (id, slot) {
        (Some(i), Some(s)) => {
            a.sets[s - 1] = Some(i.clone());
            Some(i)
        }
        (Some(i), None) => Some(i),
        (None, Some(s)) => Some(a.sets[s - 1].clone().ok_or(format!("weapon set {s} is empty"))?),
        (None, None) if off.is_some() || rot.is_some() => a.equipped.clone(),
        (None, None) => return Err("give `id` or `slot`".into()),
    };
    a.equipped = pick;
    if let Some(o) = off {
        a.grip_offset = o;
    }
    if let Some(r) = rot {
        a.grip_rot_deg = r;
    }
    let walking = world.resource::<crate::walk::WalkState>().active;
    let mut out = vec![];
    if !walking {
        out.push(McpContent::Text("Equipped; it shows in hand while walking (walk_start).".into()));
    }
    out.push(after(3));
    Ok(out)
}

fn unequip(world: &mut World, _: Value) -> Result<Vec<McpContent>, String> {
    world.resource_mut::<Armory>().equipped = None;
    Ok(vec![after(2)])
}

fn use_(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let action: String = arg(&args, "action")?;
    let cmd = match action.as_str() {
        "attack" => UseCmd::Attack {
            heavy: opt_arg::<bool>(&args, "heavy")?.unwrap_or(false),
        },
        "draw" => UseCmd::Draw,
        "release" => UseCmd::Release,
        "fire" => UseCmd::Fire,
        "aim" => UseCmd::Aim(opt_arg::<bool>(&args, "on")?.unwrap_or(true)),
        "reload" => UseCmd::Reload,
        a => return Err(format!("unknown action `{a}`")),
    };
    let wait = opt_arg::<u32>(&args, "wait_frames")?.unwrap_or(2);
    let mut a = world.resource_mut::<Armory>();
    if a.equipped.is_none() {
        return Err("nothing equipped (weapon_equip)".into());
    }
    a.queue.push(cmd);
    Ok(vec![after(wait)])
}

fn view(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    if let Some(t) = opt_arg::<bool>(&args, "third_person")? {
        world.resource_mut::<crate::characters::player::PlayerView>().third_person = t;
    }
    if let Some(s) = opt_arg::<f32>(&args, "time_scale")? {
        world
            .resource_mut::<Time<Virtual>>()
            .set_relative_speed(s.clamp(0.02, 1.0));
    }
    if let Some(o) = opt_arg::<f32>(&args, "orbit_deg")? {
        world.resource_mut::<Armory>().view_orbit_deg = o;
    }
    Ok(vec![after(2)])
}
