//! The Gameplay tab's viewport tools: click to place (player start, POIs, NPCs, resource nodes,
//! ore deposits), drag to set facing or slide what was just placed, click pins to select and drag
//! them, drag a selected spawn region's or deposit area's corners, draw spawn regions and deposit
//! areas corner by corner or as a lasso, remove.
//! Keys: Enter / double-click finish a region, Esc cancels it (or clears the selection),
//! Backspace drops the last corner, Delete removes the selection, Ctrl/⌘ D duplicates it.
//!
//! Each mouse stroke is one undo step (begun by `tools::update_stroke`, committed at its end).

use bevy::prelude::*;
use bevy_egui::egui::pos2;

use super::edit::{self, Ground};
use super::ores::{self, Ore, Site};
use super::{Gameplay, GameplayEditor, GameplaySel, GameplayTool, OreMode, markers};
use crate::camera::{MainCamera, Pointer};
use crate::editor::{Editor, Tab};
use crate::history::History;
use crate::objects::{Library, ObjectStore};
use crate::roads::Roads;
use crate::terrain::Terrain;
use crate::tools::{Stroke, ToolSystems};
use crate::water::WaterBodies;
use crate::world::{WorldMeta, Zones};

pub(super) fn build(app: &mut App) {
    app.add_systems(
        Update,
        (tool_input, tool_keys).chain().in_set(ToolSystems::Apply),
    )
    .add_systems(Update, test_hooks.after(crate::debug::demo_content));
}

#[derive(Default, Clone, Copy)]
enum Drag {
    #[default]
    None,
    /// Slide an item; `offset` = anchor − cursor at the grab. `placed`: by this click.
    Move {
        sel: GameplaySel,
        offset: Vec2,
        placed: bool,
    },
    /// A corner of a spawn region or deposit area.
    Corner { sel: GameplaySel, idx: usize },
    /// Turn the player start or an NPC toward the cursor.
    Facing { sel: GameplaySel },
    /// The first corner of a spawn region: dragging on turns it into a lasso.
    Lasso,
}

/// Yaw facing the way the camera looks (new player starts and NPCs face into the screen).
fn camera_yaw(cam_t: &GlobalTransform) -> f32 {
    let f = cam_t.forward();
    (-f.z).atan2(f.x)
}

/// The deposit area (drawn over spawn regions) or spawn region under a ground point.
fn spawn_at(gp: &Gameplay, p: Vec2) -> Option<GameplaySel> {
    gp.ore_areas
        .iter()
        .rev()
        .find(|a| a.contains(p))
        .map(|a| GameplaySel::OreArea(a.id))
        .or_else(|| {
            gp.spawns
                .iter()
                .rev()
                .find(|s| s.contains(p))
                .map(|s| GameplaySel::Spawn(s.id))
        })
}

/// Turns the outline into a deposit area of `ore` (and its usual companion) inside the caller's
/// undo step. Returns false when the outline isn't usable (and says why).
pub fn finish_area(
    gp: &mut Gameplay,
    site: &Site,
    history: &mut History,
    ed: &mut Editor,
    ore: Ore,
    pts: &[Vec2],
) -> bool {
    let poly: Vec<[f32; 2]> = pts.iter().map(|p| [p.x, p.y]).collect();
    if poly.len() < 3 || edit::polygon_area(&poly) < 100.0 {
        ed.toast("A deposit area needs at least 3 corners around ~10 × 10 m or more");
        return false;
    }
    history.set_label("Draw deposit area");
    history.touch_gameplay(gp);
    match ores::add_area(gp, site, poly, ores::default_table(ore), None) {
        Ok(id) => {
            gp.selected = Some(GameplaySel::OreArea(id));
            if let Some(a) = gp.ore_areas.iter().find(|a| a.id == id) {
                let n = gp.nodes.iter().filter(|n| n.area == Some(id)).count();
                let hint = if n == 0 {
                    " · no suitable ground inside (water, cliffs, roads?)"
                } else {
                    ""
                };
                ed.toast(format!(
                    "{}: {n} deposits ({}){hint}",
                    a.name,
                    a.mix_label()
                ));
            }
            true
        }
        Err(e) => {
            ed.toast(e);
            false
        }
    }
}

/// Turns the outline into a spawn region (inside the caller's undo step). Returns false when the
/// outline isn't usable (and says why).
pub fn finish_region(
    gp: &mut Gameplay,
    g: &Ground,
    history: &mut History,
    ed: &mut Editor,
    creatures: &[u32],
    pts: &[Vec2],
) -> bool {
    let poly: Vec<[f32; 2]> = pts.iter().map(|p| [p.x, p.y]).collect();
    if poly.len() < 3 || edit::polygon_area(&poly) < 150.0 {
        ed.toast("A spawn region needs at least 3 corners around ~15 × 15 m or more");
        return false;
    }
    history.set_label("Draw spawn region");
    history.touch_gameplay(gp);
    match edit::add_spawn(gp, g, poly, creatures, None) {
        Ok(id) => {
            gp.selected = Some(GameplaySel::Spawn(id));
            if let Some(r) = gp.spawns.iter().find(|s| s.id == id) {
                let hint = if r.entries.is_empty() {
                    " · add creatures in the inspector"
                } else {
                    ""
                };
                ed.toast(format!(
                    "{}: {:.2} km², {}{hint}",
                    r.name,
                    r.area() / 1.0e6,
                    edit::level_label(r.level)
                ));
            }
            true
        }
        Err(e) => {
            ed.toast(e);
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn tool_input(
    stroke: Res<Stroke>,
    pointer: Res<Pointer>,
    mut ed: ResMut<Editor>,
    mut gp_ed: ResMut<GameplayEditor>,
    mut gp: ResMut<Gameplay>,
    mut history: ResMut<History>,
    (terrain, zones, water): (Res<Terrain>, Res<Zones>, Res<WaterBodies>),
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    time: Res<Time<Real>>,
    store: Res<ObjectStore>,
    (lib, roads): (Option<Res<Library>>, Option<Res<Roads>>),
    mut drag: Local<Drag>,
    mut last_click: Local<Option<(f32, Vec2)>>,
    mut area_moved: Local<bool>,
) {
    // an inspector edit left open when the panel went away (another tab, walk mode)
    if gp_ed.editing && !gp_ed.inspector_drawn && !stroke.active {
        history.commit(&store);
        gp_ed.editing = false;
    }
    gp_ed.inspector_drawn = false;
    let drawing = gp_ed.tool == GameplayTool::Spawn
        || (gp_ed.tool == GameplayTool::Ore && gp_ed.ore_mode == OreMode::Area);
    if !drawing && !gp_ed.draft.is_empty() {
        gp_ed.draft.clear();
    }
    if ed.tab != Tab::Gameplay {
        gp_ed.draft.clear();
        *drag = Drag::None;
        return;
    }
    let Ok((cam, cam_t)) = camera.single() else {
        return;
    };
    let g = Ground {
        terrain: &terrain,
        zones: &zones,
        water: &water,
        nature: None,
    };
    let site = Site {
        g: &g,
        store: Some(&store),
        lib: lib.as_deref(),
        roads: roads.as_deref(),
    };
    let at = pointer
        .screen
        .map(|s| pos2(s.x, s.y))
        .unwrap_or(pos2(-1e4, -1e4));

    if stroke.just_started {
        *drag = Drag::None;
        *area_moved = false;
        let Some(hit) = stroke.hit else { return };
        let p = hit.xz();
        let tool = gp_ed.tool;
        let pins = markers::layout(&gp, &gp_ed.view, &g, cam, cam_t, true);
        let pin = markers::pick_pin(&pins, at);
        let corner = markers::pick_vertex(&gp, &g, cam, cam_t, at);

        // spawn regions and deposit areas: every click adds a corner (pins stay grabbable before
        // the first one)
        if drawing && (!gp_ed.draft.is_empty() || (pin.is_none() && corner.is_none())) {
            let now = time.elapsed_secs();
            let double = last_click
                .is_some_and(|(t, q)| now - t < 0.4 && q.distance(Vec2::new(at.x, at.y)) < 8.0);
            *last_click = Some((now, Vec2::new(at.x, at.y)));
            let closes = gp_ed.draft.len() >= 3
                && cam
                    .world_to_viewport(cam_t, gp_ed.draft[0] + Vec3::Y * 0.3)
                    .is_ok_and(|s| s.distance(Vec2::new(at.x, at.y)) < 12.0);
            if (double || closes) && gp_ed.draft.len() >= 3 {
                let pts: Vec<Vec2> = gp_ed.draft.iter().map(|q| q.xz()).collect();
                let creatures = gp_ed.spawn_creatures.clone();
                let done = if tool == GameplayTool::Ore {
                    finish_area(&mut gp, &site, &mut history, &mut ed, gp_ed.ore, &pts)
                } else {
                    finish_region(&mut gp, &g, &mut history, &mut ed, &creatures, &pts)
                };
                if done {
                    gp_ed.draft.clear();
                }
                return;
            }
            if !double {
                gp_ed.draft.push(hit);
            }
            if gp_ed.draft.len() == 1 {
                *drag = Drag::Lasso;
            }
            return;
        }
        if tool == GameplayTool::Remove {
            if let Some(sel) = pin.or_else(|| spawn_at(&gp, p)) {
                history.set_label(format!("Remove {}", edit::sel_label(&gp, sel)));
                history.touch_gameplay(&gp);
                if let Some(what) = edit::remove_sel(&mut gp, sel) {
                    ed.toast(format!("Removed {what}"));
                }
            }
            return;
        }
        if let Some((sel, idx)) = corner {
            gp.selected = Some(sel);
            *drag = Drag::Corner { sel, idx };
            return;
        }
        if let Some(sel) = pin {
            gp.selected = Some(sel);
            let anchor = gp.anchor(sel).map(|a| a.xz()).unwrap_or(p);
            *drag = Drag::Move {
                sel,
                offset: anchor - p,
                placed: false,
            };
            return;
        }
        let yaw = camera_yaw(cam_t);
        match tool {
            GameplayTool::PlayerStart => {
                history.set_label("Set player start");
                history.touch_gameplay(&gp);
                edit::set_player_start(&mut gp, &g, p, yaw);
                gp.selected = Some(GameplaySel::PlayerStart);
                *drag = Drag::Facing {
                    sel: GameplaySel::PlayerStart,
                };
                ed.toast("Player start set: drag to turn it, or set its facing in the inspector");
            }
            GameplayTool::Poi => {
                let kind = gp_ed.poi_kind;
                history.set_label(format!("Place {}", kind.name()));
                history.touch_gameplay(&gp);
                let id = edit::add_poi(&mut gp, &g, kind, p, None);
                let sel = GameplaySel::Poi(id);
                gp.selected = Some(sel);
                *drag = Drag::Move {
                    sel,
                    offset: Vec2::ZERO,
                    placed: true,
                };
            }
            GameplayTool::Npc => {
                let role = gp_ed.npc_role;
                history.set_label(format!("Place {}", role.name()));
                history.touch_gameplay(&gp);
                let id = edit::add_npc(&mut gp, &g, role, p, yaw, None, None);
                let sel = GameplaySel::Npc(id);
                gp.selected = Some(sel);
                *drag = Drag::Facing { sel };
                if let Some(q) = gp
                    .npcs
                    .last()
                    .and_then(|n| n.poi)
                    .and_then(|pid| gp.pois.iter().find(|q| q.id == pid))
                {
                    ed.toast(format!("Joined {} (nearest point of interest)", q.name));
                }
            }
            GameplayTool::Node => {
                let (kind, variant, tier) =
                    (gp_ed.node_kind, gp_ed.node_variant.clone(), gp_ed.node_tier);
                history.set_label(format!("Place {} node", kind.name()));
                history.touch_gameplay(&gp);
                let id = edit::add_node(&mut gp, &g, kind, p, Some(variant), Some(tier));
                let sel = GameplaySel::Node(id);
                gp.selected = Some(sel);
                *drag = Drag::Move {
                    sel,
                    offset: Vec2::ZERO,
                    placed: true,
                };
            }
            GameplayTool::Ore => {
                let ore = gp_ed.ore;
                history.set_label(format!("Place {} deposit", ore.name()));
                history.touch_gameplay(&gp);
                let id = ores::add_deposit(&mut gp, &g, ore, p);
                let sel = GameplaySel::Node(id);
                gp.selected = Some(sel);
                *drag = Drag::Move {
                    sel,
                    offset: Vec2::ZERO,
                    placed: true,
                };
                if g.water_depth(p) > 0.3 {
                    ed.toast(format!("{} deposit placed under water", ore.name()));
                }
            }
            _ => {
                // select (and slide) the region under the cursor, else clear the selection
                gp.selected = spawn_at(&gp, p);
                if let Some(sel) = gp.selected {
                    let anchor = gp.anchor(sel).map(|a| a.xz()).unwrap_or(p);
                    *drag = Drag::Move {
                        sel,
                        offset: anchor - p,
                        placed: false,
                    };
                }
            }
        }
        return;
    }

    if stroke.active
        && let Some(hit) = stroke.hit
    {
        let p = hit.xz();
        match *drag {
            Drag::Move {
                sel,
                offset,
                placed,
            } => {
                let target = p + offset;
                if gp
                    .anchor(sel)
                    .is_some_and(|a| a.xz().distance(target) > 0.05)
                {
                    if !placed {
                        history.set_label(format!("Move {}", edit::sel_label(&gp, sel)));
                    }
                    history.touch_gameplay(&gp);
                    edit::move_to(&mut gp, &g, sel, target);
                    *area_moved |= matches!(sel, GameplaySel::OreArea(_));
                }
            }
            Drag::Corner { sel, idx } => {
                if markers::region_outline(&gp, sel)
                    .and_then(|o| o.get(idx))
                    .is_some_and(|v| Vec2::new(v[0], v[1]).distance(p) > 0.05)
                {
                    history.set_label(format!("Reshape {}", edit::sel_label(&gp, sel)));
                    history.touch_gameplay(&gp);
                    match sel {
                        GameplaySel::Spawn(id) => {
                            if let Some(v) = gp
                                .spawns
                                .iter_mut()
                                .find(|s| s.id == id)
                                .and_then(|r| r.polygon.get_mut(idx))
                            {
                                *v = [p.x, p.y];
                            }
                        }
                        GameplaySel::OreArea(id) => {
                            if let Some(v) = gp
                                .ore_areas
                                .iter_mut()
                                .find(|a| a.id == id)
                                .and_then(|a| a.polygon.get_mut(idx))
                            {
                                *v = [p.x, p.y];
                            }
                            ores::rescatter(&mut gp, id, &site);
                        }
                        _ => {}
                    }
                    gp.dirty = true;
                }
            }
            Drag::Facing { sel } => {
                if let Some(a) = gp.anchor(sel) {
                    let d = p - a.xz();
                    if d.length() > 1.5 {
                        let yaw = (-d.y).atan2(d.x);
                        history.touch_gameplay(&gp);
                        match sel {
                            GameplaySel::PlayerStart => {
                                if let Some(s) = &mut gp.player_start {
                                    s.yaw = yaw;
                                }
                            }
                            GameplaySel::Npc(id) => {
                                if let Some(n) = gp.npcs.iter_mut().find(|n| n.id == id) {
                                    n.yaw = yaw;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Drag::Lasso | Drag::None => {}
        }
    }

    if stroke.just_ended {
        // a long drag from the first corner: the drawn loop is the region (or area)
        if let Drag::Lasso = *drag
            && gp_ed.draft.len() == 1
        {
            let path: Vec<Vec2> = stroke.path.iter().map(|q| q.xz()).collect();
            let len: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
            // (deposit areas may be small: a shorter loop and finer corners do)
            let (min_len, tol) = if gp_ed.tool == GameplayTool::Ore {
                (40.0, 2.0)
            } else {
                (60.0, 2.5)
            };
            if path.len() >= 8 && len > min_len {
                let pts = edit::simplify(&path, tol);
                let creatures = gp_ed.spawn_creatures.clone();
                if gp_ed.tool == GameplayTool::Ore {
                    finish_area(&mut gp, &site, &mut history, &mut ed, gp_ed.ore, &pts);
                } else {
                    finish_region(&mut gp, &g, &mut history, &mut ed, &creatures, &pts);
                }
                gp_ed.draft.clear();
            }
        }
        // a moved deposit area: its deposits settle on the ground they were moved to
        if let Drag::Move {
            sel: GameplaySel::OreArea(id),
            ..
        } = *drag
            && std::mem::take(&mut *area_moved)
        {
            ores::rescatter(&mut gp, id, &site);
        }
        *drag = Drag::None;
    }
}

#[allow(clippy::too_many_arguments)]
fn tool_keys(
    keys: Res<ButtonInput<KeyCode>>,
    pointer: Res<Pointer>,
    stroke: Res<Stroke>,
    walk: Res<crate::walk::WalkState>,
    mut ed: ResMut<Editor>,
    mut gp_ed: ResMut<GameplayEditor>,
    mut gp: ResMut<Gameplay>,
    mut history: ResMut<History>,
    (terrain, zones, water, store): (Res<Terrain>, Res<Zones>, Res<WaterBodies>, Res<ObjectStore>),
    mut meta: ResMut<WorldMeta>,
    (lib, roads): (Option<Res<Library>>, Option<Res<Roads>>),
) {
    if ed.tab != Tab::Gameplay || pointer.keyboard_captured || walk.active || stroke.active {
        return;
    }
    let g = Ground {
        terrain: &terrain,
        zones: &zones,
        water: &water,
        nature: None,
    };
    let cmd = keys.any_pressed([
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
    ]);
    let drafting = !gp_ed.draft.is_empty();
    if keys.just_pressed(KeyCode::Escape) {
        if drafting {
            gp_ed.draft.clear();
            ed.toast(if gp_ed.tool == GameplayTool::Ore {
                "Deposit area cancelled"
            } else {
                "Spawn region cancelled"
            });
        } else {
            gp.selected = None;
        }
    }
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) && drafting {
        let pts: Vec<Vec2> = gp_ed.draft.iter().map(|q| q.xz()).collect();
        let creatures = gp_ed.spawn_creatures.clone();
        let site = Site {
            g: &g,
            store: Some(&store),
            lib: lib.as_deref(),
            roads: roads.as_deref(),
        };
        history.begin("Draw spawn region");
        let done = if gp_ed.tool == GameplayTool::Ore {
            finish_area(&mut gp, &site, &mut history, &mut ed, gp_ed.ore, &pts)
        } else {
            finish_region(&mut gp, &g, &mut history, &mut ed, &creatures, &pts)
        };
        if done {
            gp_ed.draft.clear();
            meta.dirty = true;
        }
        history.commit(&store);
    }
    if keys.just_pressed(KeyCode::Backspace) && drafting {
        gp_ed.draft.pop();
        return;
    }
    let Some(sel) = gp.selected.filter(|s| gp.exists(*s)) else {
        return;
    };
    if keys.any_just_pressed([KeyCode::Delete, KeyCode::Backspace]) {
        history.begin(format!("Remove {}", edit::sel_label(&gp, sel)));
        history.touch_gameplay(&gp);
        if let Some(what) = edit::remove_sel(&mut gp, sel) {
            ed.toast(format!("Removed {what}"));
        }
        history.commit(&store);
        meta.dirty = true;
    } else if cmd && keys.just_pressed(KeyCode::KeyD) {
        history.begin(format!("Duplicate {}", edit::sel_label(&gp, sel)));
        history.touch_gameplay(&gp);
        match edit::duplicate(&mut gp, &g, sel) {
            Some(copy) => {
                gp.selected = Some(copy);
                ed.toast(format!("Duplicated: {}", edit::sel_label(&gp, copy)));
            }
            None => ed.toast("There is only one player start"),
        }
        history.commit(&store);
        meta.dirty = true;
    }
}

/// Test hooks (with `WB_SHOT` runs): `WB_GP_TOOL=select|start|poi|npc|spawn|node|ore|scatter|remove`
/// picks a tool; `WB_GP_SELECT=<id>|start|poi|npc|spawn|node|area` selects an item (the last of
/// its kind) once the `WB_MCP_CALL`s have run;
/// `WB_GP_UNDO=n` / `WB_GP_REDO=m` then undo n steps and redo m (logging the counts);
/// `WB_GP_SEQ=key=value,…;…` sets up each `WB_SEQ` step (same timing): `tool=<tool>`,
/// `sel=<kind>[:index]|none` (kind as for `WB_GP_SELECT`, index from the first), `lib=<creature
/// index>` (opens it in the library editor), `draft=x:z|x:z|…`, `ore=<ore id>`,
/// `oremode=single|area`;
/// `WB_ORE_SHOWCASE=x,z[,spacing]` places one deposit of every ore in a 5 × 5 grid (a row per
/// tier, common at −z) centred on x, z at frame 200;
/// `WB_GP_IOTEST=<dir>` saves the world, wipes it, loads it back and compares the gameplay
/// layer, then writes the server data (logs `GPTEST … PASS/FAIL`; `WB_GP_IOTEST_FRAME` moves it
/// later); `WB_GP_PERF=1` (with
/// `markers`' own timing) from frame 600 on shows and hides every marker in turns of 300 frames
/// and logs the median frame time of each turn.
fn parse_tool(t: &str) -> GameplayTool {
    match t {
        "start" => GameplayTool::PlayerStart,
        "poi" => GameplayTool::Poi,
        "npc" => GameplayTool::Npc,
        "spawn" => GameplayTool::Spawn,
        "node" => GameplayTool::Node,
        "ore" => GameplayTool::Ore,
        "scatter" => GameplayTool::Scatter,
        "remove" => GameplayTool::Remove,
        _ => GameplayTool::Select,
    }
}

#[allow(clippy::too_many_arguments)]
fn test_hooks(
    mut frame: Local<u32>,
    mut data: crate::io::WorldData,
    mut gp_ed: ResMut<GameplayEditor>,
    mut steps: MessageWriter<crate::tools::EditorAction>,
    time: Res<Time<Real>>,
    mut frame_ms: Local<Vec<f32>>,
) {
    *frame += 1;
    let f = *frame;
    if std::env::var("WB_GP_PERF").is_ok() && f >= 600 {
        let on = ((f - 600) / 300).is_multiple_of(2);
        let v = &mut gp_ed.view;
        (v.pois, v.npcs, v.spawns, v.nodes) = (on, on, on, on);
        frame_ms.push(time.delta_secs() * 1000.0);
        if (f - 600) % 300 == 299 {
            frame_ms.sort_by(|a, b| a.total_cmp(b));
            let n = frame_ms.len();
            info!(
                "GPPERF markers {}: frame median {:.2} ms, p90 {:.2} ms",
                if on { "shown" } else { "hidden" },
                frame_ms[n / 2],
                frame_ms[n * 9 / 10]
            );
            frame_ms.clear();
        }
    }
    if f == 2
        && let Ok(t) = std::env::var("WB_GP_TOOL")
    {
        gp_ed.tool = parse_tool(&t);
    }
    if f == 200
        && let Ok(spec) = std::env::var("WB_ORE_SHOWCASE")
    {
        let v: Vec<f32> = spec
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        if v.len() >= 2 {
            let step = v.get(2).copied().unwrap_or(5.0);
            let gp = &mut *data.gameplay;
            let g = Ground {
                terrain: &data.terrain,
                zones: &data.zones,
                water: &data.water,
                nature: None,
            };
            for (i, o) in Ore::ALL.into_iter().enumerate() {
                let p = Vec2::new(
                    v[0] + ((i % 5) as f32 - 2.0) * step,
                    v[1] + ((i / 5) as f32 - 2.0) * step,
                );
                ores::add_deposit(gp, &g, o, p);
            }
            info!(
                "ORESHOW placed {} deposits around {:.0},{:.0}",
                Ore::ALL.len(),
                v[0],
                v[1]
            );
        }
    }
    if let Ok(seq) = std::env::var("WB_GP_SEQ")
        && f > 150
        && (f - 151).is_multiple_of(110)
        && let Some(step) = seq.split(';').nth(((f - 151) / 110) as usize)
    {
        for kv in step.split(',') {
            let Some((k, v)) = kv.split_once('=') else {
                continue;
            };
            match k.trim() {
                "tool" => gp_ed.tool = parse_tool(v),
                "sel" => {
                    let gp = &mut *data.gameplay;
                    let (kind, i) = v
                        .split_once(':')
                        .map(|(a, b)| (a, b.parse::<usize>().ok()))
                        .unwrap_or((v, None));
                    let pick =
                        |ids: Vec<u32>| i.map_or(ids.last().copied(), |i| ids.get(i).copied());
                    gp.selected = match kind {
                        "start" => gp.player_start.map(|_| GameplaySel::PlayerStart),
                        "poi" => pick(gp.pois.iter().map(|q| q.id).collect()).map(GameplaySel::Poi),
                        "npc" => pick(gp.npcs.iter().map(|q| q.id).collect()).map(GameplaySel::Npc),
                        "spawn" => {
                            pick(gp.spawns.iter().map(|q| q.id).collect()).map(GameplaySel::Spawn)
                        }
                        "node" => {
                            pick(gp.nodes.iter().map(|q| q.id).collect()).map(GameplaySel::Node)
                        }
                        "area" => pick(gp.ore_areas.iter().map(|q| q.id).collect())
                            .map(GameplaySel::OreArea),
                        _ => None,
                    };
                }
                "ore" => {
                    if let Some(o) = Ore::parse(v) {
                        gp_ed.ore = o;
                    }
                }
                "oremode" => {
                    gp_ed.ore_mode = if v == "area" {
                        OreMode::Area
                    } else {
                        OreMode::Single
                    }
                }
                "lib" => {
                    gp_ed.creature = v
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| data.gameplay.creatures.get(i))
                        .map(|c| c.id)
                }
                "draft" => {
                    gp_ed.draft = v
                        .split('|')
                        .filter_map(|xz| {
                            let (x, z) = xz.split_once(':')?;
                            let (x, z) =
                                (x.trim().parse::<f32>().ok()?, z.trim().parse::<f32>().ok()?);
                            Some(Vec3::new(x, data.terrain.height_at(x, z), z))
                        })
                        .collect();
                }
                _ => {}
            }
        }
    }
    let calls = std::env::var("WB_MCP_CALL")
        .map(|c| c.split(';').filter(|x| !x.trim().is_empty()).count() as u32)
        .unwrap_or(0);
    let start: u32 = std::env::var("WB_MCP_FRAME")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(360);
    let after = start + calls + 3;
    if f == after {
        let gp = &mut *data.gameplay;
        if let Ok(s) = std::env::var("WB_GP_SELECT") {
            gp.selected = match s.as_str() {
                "start" => gp.player_start.map(|_| GameplaySel::PlayerStart),
                "poi" => gp.pois.last().map(|q| GameplaySel::Poi(q.id)),
                "npc" => gp.npcs.last().map(|q| GameplaySel::Npc(q.id)),
                "spawn" => gp.spawns.last().map(|q| GameplaySel::Spawn(q.id)),
                "node" => gp.nodes.last().map(|q| GameplaySel::Node(q.id)),
                "area" => gp.ore_areas.last().map(|q| GameplaySel::OreArea(q.id)),
                id => id.parse().ok().and_then(|id| gp.find(id)),
            };
        }
    }
    let count = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
    };
    let (undo, redo) = (count("WB_GP_UNDO"), count("WB_GP_REDO"));
    let gp = &data.gameplay;
    let state = || {
        format!(
            "start={} pois={} npcs={} spawns={} creatures={} nodes={} ore_areas={}",
            gp.player_start.is_some(),
            gp.pois.len(),
            gp.npcs.len(),
            gp.spawns.len(),
            gp.creatures.len(),
            gp.nodes.len(),
            gp.ore_areas.len()
        )
    };
    if undo + redo > 0 {
        if f == after {
            info!("GPTEST before undo: {}", state());
        }
        if f > after && f <= after + undo {
            steps.write(crate::tools::EditorAction::Undo);
        }
        if f == after + undo + 1 {
            info!("GPTEST after {undo} undo: {}", state());
        }
        if f > after + undo + 1 && f <= after + undo + 1 + redo {
            steps.write(crate::tools::EditorAction::Redo);
        }
        if f == after + undo + redo + 2 {
            info!("GPTEST after {redo} redo: {}", state());
        }
    }
    let io_frame = std::env::var("WB_GP_IOTEST_FRAME")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(after + undo + redo + 4);
    if f == io_frame
        && let Ok(dir) = std::env::var("WB_GP_IOTEST")
    {
        let dir = std::path::PathBuf::from(dir);
        let path = dir.join("gptest.world");
        let before = serde_json::to_value(&*data.gameplay).unwrap_or_default();
        if let Err(e) = crate::io::save_world(&path, &data) {
            return error!("GPTEST save FAIL {e}");
        }
        crate::io::new_world(
            &mut data,
            crate::terrain::worldgen::WorldPreset::Ocean,
            256,
            4.0,
            1,
        );
        let wiped =
            data.gameplay.pois.len() + data.gameplay.spawns.len() + data.gameplay.nodes.len();
        if let Err(e) = crate::io::load_world(&path, &mut data) {
            return error!("GPTEST load FAIL {e}");
        }
        let after = serde_json::to_value(&*data.gameplay).unwrap_or_default();
        let gp = &data.gameplay;
        if before == after && wiped == 0 {
            info!(
                "GPTEST roundtrip PASS start={} pois={} npcs={} spawns={} creatures={} nodes={}",
                gp.player_start.is_some(),
                gp.pois.len(),
                gp.npcs.len(),
                gp.spawns.len(),
                gp.creatures.len(),
                gp.nodes.len()
            );
        } else {
            error!("GPTEST roundtrip FAIL (wiped {wiped})\nbefore {before}\nafter  {after}");
        }
        match super::write_server_data(
            &dir.join("export"),
            gp,
            &data.terrain,
            &data.zones,
            &data.water,
            &data.meta.name,
        ) {
            Ok(p) => info!("GPTEST server data PASS {}", p.display()),
            Err(e) => error!("GPTEST server data FAIL {e}"),
        }
    }
}
