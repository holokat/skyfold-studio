//! Stroke tracking (mouse down → drag → up), the terrain-based tools (Land, Sculpt, Paint,
//! Zones), undo grouping and editor keyboard shortcuts.

use bevy::prelude::*;

use crate::camera::Pointer;
use crate::editor::{Editor, LandTool, PaintTool, SculptTool, Tab, ZoneTool};
use crate::history::History;
use crate::objects::ObjectStore;
use crate::terrain::Terrain;
use crate::terrain::brush::{self, BrushArgs};
use crate::water::WaterBodies;
use crate::world::{WorldMeta, Zones};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolSystems {
    Input,
    Apply,
    Commit,
}

#[derive(Resource, Default)]
pub struct Stroke {
    pub active: bool,
    pub just_started: bool,
    pub just_ended: bool,
    /// Alt / Cmd held: inverse action (lower, erase, remove).
    pub inverse: bool,
    pub hit: Option<Vec3>,
    pub last_hit: Option<Vec3>,
    pub start_hit: Option<Vec3>,
    /// Ground path drawn during the stroke (coast outlines, rivers).
    pub path: Vec<Vec3>,
    /// xz bounds touched by the stroke.
    pub bounds: Option<(Vec2, Vec2)>,
    pub tab: Tab,
}

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Stroke>()
            .init_resource::<History>()
            .configure_sets(
                Update,
                (ToolSystems::Input, ToolSystems::Apply, ToolSystems::Commit)
                    .chain()
                    .after(crate::camera::CameraSystems),
            )
            .add_systems(Update, update_stroke.in_set(ToolSystems::Input))
            .add_systems(Update, terrain_tools.in_set(ToolSystems::Apply))
            .add_systems(
                Update,
                (commit_history, shortcuts)
                    .chain()
                    .in_set(ToolSystems::Commit),
            );
    }
}

fn update_stroke(
    mut stroke: ResMut<Stroke>,
    pointer: Res<Pointer>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    mut history: ResMut<History>,
) {
    stroke.just_started = false;
    stroke.just_ended = false;
    stroke.last_hit = stroke.hit;
    stroke.hit = pointer.hit;
    stroke.inverse = keys.pressed(KeyCode::AltLeft)
        || keys.pressed(KeyCode::AltRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight);
    let blocked = walk.active || walk.cinematic || keys.pressed(KeyCode::Space);
    if buttons.just_pressed(MouseButton::Left) && !pointer.over_ui && !blocked && !stroke.active {
        stroke.active = true;
        stroke.just_started = true;
        stroke.start_hit = pointer.hit;
        stroke.last_hit = pointer.hit;
        stroke.path.clear();
        stroke.bounds = None;
        stroke.tab = ed.tab;
        history.begin(stroke_label(&ed));
    } else if stroke.active && (!buttons.pressed(MouseButton::Left) || ed.tab != stroke.tab) {
        stroke.active = false;
        stroke.just_ended = true;
    }
    if (stroke.active || stroke.just_ended)
        && let Some(h) = stroke.hit
    {
        if stroke.path.last().is_none_or(|l| l.distance(h) > 2.0) {
            stroke.path.push(h);
        }
        let r = ed.active_brush().map(|b| b.size).unwrap_or(5.0);
        let (mn, mx) = (h.xz() - Vec2::splat(r), h.xz() + Vec2::splat(r));
        stroke.bounds = Some(match stroke.bounds {
            Some((a, b)) => (a.min(mn), b.max(mx)),
            None => (mn, mx),
        });
    }
}

fn stroke_label(ed: &Editor) -> String {
    match ed.tab {
        Tab::Land => match ed.land_tool {
            LandTool::PaintLand => "Paint land".into(),
            LandTool::PaintSea => "Paint sea".into(),
            LandTool::DrawCoast => "Draw coastline".into(),
            LandTool::Reference => "Reference map".into(),
        },
        Tab::Sculpt => format!("Sculpt: {}", ed.sculpt_tool.name()),
        Tab::Paint => format!("Paint {}", ed.paint_layer.name()),
        Tab::Caves => "Caves".into(),
        Tab::Water => "Water".into(),
        Tab::Nature => "Nature brush".into(),
        Tab::Objects => "Place object".into(),
        Tab::Zones if ed.zone_tool == ZoneTool::Biome => {
            format!("Biome: {}", ed.biome_preset.def().name)
        }
        Tab::Zones => "Paint zone".into(),
        Tab::Gameplay => "Gameplay edit".into(),
        Tab::Dungeons | Tab::DungeonLook | Tab::DungeonLights => "Dungeon".into(),
        _ => "Edit".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn terrain_tools(
    mut stroke: ResMut<Stroke>,
    mut ed: ResMut<Editor>,
    mut terrain: ResMut<Terrain>,
    mut history: ResMut<History>,
    zones: Res<Zones>,
    meta: Res<WorldMeta>,
    time: Res<Time>,
    mut flatten_target: Local<f32>,
    bigworld: Res<crate::bigworld::BigWorld>,
) {
    if !(stroke.active || stroke.just_ended) {
        return;
    }
    let dt = time.delta_secs().min(0.05);
    let inv = stroke.inverse;
    let seed = meta.seed;

    // the reference map has its own viewport tools (refmap::ui)
    if ed.tab == Tab::Land && ed.land_tool == LandTool::Reference {
        return;
    }
    // coast outline is applied once on release
    if ed.tab == Tab::Land && ed.land_tool == LandTool::DrawCoast {
        if stroke.just_ended && stroke.path.len() > 8 {
            let outline: Vec<Vec2> = stroke.path.iter().map(|p| p.xz()).collect();
            let (lh, rough) = (ed.land_height, ed.coast_roughness);
            brush::fill_outline(&mut terrain, &mut history, &outline, inv, lh, rough, seed);
            ed.toast(if inv {
                "Carved the outlined area into the sea"
            } else {
                "Raised a new landmass from your outline"
            });
        }
        return;
    }
    if !stroke.active {
        return;
    }
    let Some(hit) = stroke.hit else { return };
    if stroke.just_started {
        *flatten_target = hit.y;
    }

    // interpolate dabs between frames so fast strokes don't leave gaps
    let from = stroke.last_hit.unwrap_or(hit);
    let Some(b) = ed.active_brush() else { return };
    let seg = hit.xz().distance(from.xz());
    let spacing = (b.size * 0.3).max(0.5);
    let dabs = ((seg / spacing).ceil() as usize).clamp(1, 24);
    for d in 0..dabs {
        let t = (d + 1) as f32 / dabs as f32;
        let c = from.xz().lerp(hit.xz(), t);
        let args = BrushArgs {
            center: c,
            radius: b.size,
            falloff: b.falloff,
            strength: b.strength,
            dt: dt / dabs as f32,
        };
        match ed.tab {
            Tab::Land => {
                let dir = if (ed.land_tool == LandTool::PaintSea) ^ inv {
                    -1.0
                } else {
                    1.0
                };
                brush::paint_land(
                    &mut terrain,
                    &mut history,
                    &args,
                    dir,
                    ed.land_height,
                    ed.coast_roughness,
                    seed,
                );
            }
            Tab::Sculpt => match ed.sculpt_tool {
                SculptTool::Raise => brush::raise(
                    &mut terrain,
                    &mut history,
                    &args,
                    if inv { -1.0 } else { 1.0 },
                ),
                SculptTool::Lower => brush::raise(
                    &mut terrain,
                    &mut history,
                    &args,
                    if inv { 1.0 } else { -1.0 },
                ),
                SculptTool::Smooth => brush::smooth(&mut terrain, &mut history, &args),
                SculptTool::Flatten => {
                    brush::flatten(&mut terrain, &mut history, &args, *flatten_target)
                }
                SculptTool::Mountain => brush::mountain(
                    &mut terrain,
                    &mut history,
                    &args,
                    if inv { -1.0 } else { 1.0 },
                    seed + 11,
                ),
                SculptTool::Hills => brush::hills(
                    &mut terrain,
                    &mut history,
                    &args,
                    if inv { -1.0 } else { 1.0 },
                    seed + 13,
                ),
                SculptTool::Terrace => {
                    brush::terrace(&mut terrain, &mut history, &args, ed.terrace_step)
                }
                // one erosion step per frame (it routes water over the whole brush)
                SculptTool::Erode if d + 1 == dabs => brush::erode(
                    &mut terrain,
                    &mut history,
                    &BrushArgs { dt, ..args },
                    &bigworld.erosion_params(),
                ),
                SculptTool::Erode => {}
            },
            Tab::Paint => {
                let erase = (ed.paint_tool == PaintTool::Erase) ^ inv;
                brush::paint_layer(
                    &mut terrain,
                    &mut history,
                    &args,
                    ed.paint_layer as usize,
                    erase,
                );
            }
            Tab::Zones if ed.zone_tool == ZoneTool::Biome => {} // biomes::biome_tool
            Tab::Zones => {
                let erase = (ed.zone_tool == ZoneTool::Erase) ^ inv;
                let sel = zones.selected.and_then(|i| zones.list.get(i)).map(|z| z.id);
                match (erase, sel) {
                    (true, _) => brush::paint_zone(&mut terrain, &mut history, c, b.size, 0, None),
                    (false, Some(id)) => {
                        brush::paint_zone(&mut terrain, &mut history, c, b.size, id, None)
                    }
                    (false, None) => {
                        if stroke.just_started {
                            ed.toast("Add or select a zone first");
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let _ = &mut stroke;
}

fn commit_history(
    stroke: Res<Stroke>,
    mut history: ResMut<History>,
    store: Res<ObjectStore>,
    mut meta: ResMut<WorldMeta>,
) {
    if stroke.just_ended && history.is_recording() {
        history.commit(&store);
        meta.dirty = true;
    }
}

#[derive(Message, Clone, Debug)]
pub enum EditorAction {
    Undo,
    Redo,
}

#[allow(clippy::too_many_arguments)]
fn shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    pointer: Res<Pointer>,
    mut ed: ResMut<Editor>,
    mut history: ResMut<History>,
    mut terrain: ResMut<Terrain>,
    mut store: ResMut<ObjectStore>,
    mut water: ResMut<WaterBodies>,
    mut zones: ResMut<Zones>,
    mut actions: MessageReader<EditorAction>,
    stroke: Res<Stroke>,
    mut lighting: ResMut<crate::sky::Lighting>,
    mut gameplay: ResMut<crate::gameplay::Gameplay>,
    mut roads: ResMut<crate::roads::Roads>,
) {
    let cmd = keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
        || keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let mut undo = false;
    let mut redo = false;
    for a in actions.read() {
        match a {
            EditorAction::Undo => undo = true,
            EditorAction::Redo => redo = true,
        }
    }
    if !pointer.keyboard_captured {
        if cmd && keys.just_pressed(KeyCode::KeyZ) {
            if shift {
                redo = true;
            } else {
                undo = true;
            }
        }
        if cmd && keys.just_pressed(KeyCode::KeyY) {
            redo = true;
        }
        if !cmd {
            // 1–9 open the rail's groups; pressing a group's key again steps through its tabs
            let digits = [
                KeyCode::Digit1,
                KeyCode::Digit2,
                KeyCode::Digit3,
                KeyCode::Digit4,
                KeyCode::Digit5,
                KeyCode::Digit6,
                KeyCode::Digit7,
                KeyCode::Digit8,
                KeyCode::Digit9,
            ];
            for (k, g) in digits.into_iter().zip(crate::editor::TabGroup::visible()) {
                if keys.just_pressed(k) {
                    let tabs = g.tabs();
                    ed.tab = match tabs.iter().position(|t| *t == ed.tab) {
                        Some(i) => tabs[(i + 1) % tabs.len()],
                        None => ed.group_tab.get(&g).copied().unwrap_or(tabs[0]),
                    };
                }
            }
            // L cycles the art style
            if keys.just_pressed(KeyCode::KeyL) {
                let look = &mut lighting.look;
                let all = crate::look::LookPreset::ALL;
                let i = all.iter().position(|p| *p == look.preset).unwrap_or(0);
                let next = all[(i + if shift { all.len() - 1 } else { 1 }) % all.len()];
                look.select(next);
                ed.toast(format!("Style: {} ({})", next.name(), next.inspired_by()));
            }
            if keys.just_pressed(KeyCode::KeyH) {
                ed.show_help = !ed.show_help;
            }
        }
    }
    if stroke.active {
        return;
    }
    if undo || redo {
        match history.step(
            redo,
            &mut terrain,
            &mut store,
            &mut water,
            &mut zones,
            &mut gameplay,
            &mut roads,
        ) {
            Some(label) => ed.toast(format!(
                "{} {}",
                if redo { "Redo:" } else { "Undo:" },
                label
            )),
            None => ed.toast(if redo {
                "Nothing to redo"
            } else {
                "Nothing to undo"
            }),
        }
    }
}
