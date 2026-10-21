//! Towns in one click: a whole hamlet, village, town or city laid out and placed from the
//! available assets (Objects tab › Set pieces › Village / Town, or the MCP tool
//! `objects_place_town`, which the area prompt's agent uses for requests like "add a village with
//! a blacksmith and an inn").
//!
//! - `kinds`: sizes, styles, the building types and the words for them, the default mix and the
//!   catalog of models this account can place (a locked pack's buildings are skipped, said so).
//! - `layout` (pure, deterministic per seed): composed like a hand-made MMO town: a road through
//!   it, a square or green beside the road with a landmark, the key buildings framing it, a few
//!   homes well apart along lanes, farmsteads and a windmill or tower outside, trees between,
//!   restrained dressing, a palisade only when asked.
//! - `place`: into the world as one undo step: real roads, fitted buildings, props, paint,
//!   cleared nature; every object of a town shares one group id.
//! - `ui`: the tiles and the card (size picker, folded options, Shuffle); `mcp`: the MCP tool.
//!
//! One click on the ground places a town of the picked size there, turned so its main street
//! runs the flattest way; dragging a loop fills that exact area. Shuffle re-rolls the last one
//! in place with the next seed (and the current options).

use bevy::prelude::*;

use crate::editor::{ObjectTool, Tab};

pub mod kinds;
pub mod layout;
mod mcp;
pub mod place;
pub mod ui;

pub use kinds::{Style, TownSize};
use place::{Area, Report, Request, place_town};

/// The last town placed in the editor (Shuffle re-rolls it in place).
#[derive(Clone, Debug)]
pub struct LastTown {
    pub req: Request,
    pub ids: Vec<u64>,
    pub roads: Vec<u32>,
}

/// Objects tab › Set pieces › Village / Town (kept for the session).
#[derive(Resource)]
pub struct TownSettings {
    /// the size a click places (None: not placing a town)
    pub selected: Option<TownSize>,
    pub style: Style,
    /// the streets as real roads
    pub roads: bool,
    /// a palisade round a town or city (None: none, MMO towns lie open in the land)
    pub walls: Option<bool>,
    pub seed: u32,
    pub last: Option<LastTown>,
    /// Shuffle was pressed (handled by `shuffle`)
    pub shuffle: bool,
    /// the cursor preview: where, turned how, for which size
    preview: Option<(Vec2, f32, TownSize, bool)>,
}

impl Default for TownSettings {
    fn default() -> Self {
        Self {
            selected: None,
            style: Style::Auto,
            roads: true,
            walls: None,
            seed: 1,
            last: None,
            shuffle: false,
            preview: None,
        }
    }
}

impl TownSettings {
    pub fn walls_for(&self, size: TownSize) -> bool {
        size.is_town() && self.walls.unwrap_or(false)
    }
}

pub struct TownsPlugin;

impl Plugin for TownsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TownSettings>()
            .add_systems(Update, test_hooks)
            .add_systems(
                Update,
                editor_place.in_set(crate::tools::ToolSystems::Apply),
            )
            .add_systems(
                Update,
                (shuffle, cursor).after(crate::tools::ToolSystems::Commit),
            );
        mcp::register(app);
    }
}

/// Test hooks (screenshots): `WB_TOWN=<hamlet|village|town|city>` picks that town tile in
/// Objects › Set pieces; `WB_TOWN_STYLE=<auto|handpainted|photoreal>` its style;
/// `WB_TOWN_SHUFFLE=<frame>` presses Shuffle then.
fn test_hooks(
    mut frame: Local<u32>,
    mut towns: ResMut<TownSettings>,
    mut sets: ResMut<crate::setpieces::SetPieceSettings>,
    mut place: ResMut<crate::objects::PlaceSettings>,
    mut ed: ResMut<crate::editor::Editor>,
) {
    *frame += 1;
    if std::env::var("WB_TOWN_SHUFFLE")
        .ok()
        .and_then(|f| f.parse::<u32>().ok())
        == Some(*frame)
    {
        towns.shuffle = true;
    }
    if *frame > 1 {
        return;
    }
    if let Some(s) = std::env::var("WB_TOWN_STYLE")
        .ok()
        .and_then(|s| Style::parse(&s))
    {
        towns.style = s;
    }
    if let Some(size) = std::env::var("WB_TOWN")
        .ok()
        .and_then(|k| TownSize::parse(&k))
    {
        towns.selected = Some(size);
        sets.selected = None;
        place.selected = None;
        ed.object_tool = ObjectTool::Place;
    }
}

fn toast(world: &mut World, msg: String) {
    world.resource_mut::<crate::editor::Editor>().toast(msg);
}

/// What the editor says after a town went down.
fn placed_message(r: &Report) -> String {
    let mut msg = format!(
        "{}: a {} of {} buildings",
        r.name,
        r.size.name().to_lowercase(),
        r.buildings
    );
    match r.roads.len() {
        0 => {}
        1 => msg.push_str(" along its street"),
        n => msg.push_str(&format!(" on {n} streets")),
    }
    msg.push_str(" — Shuffle re-rolls it");
    if let Some(n) = r.notes.first() {
        msg.push_str(&format!(". {n}"));
    }
    msg
}

/// One click on the ground places a town of the picked size there; a loop dragged round an area
/// fills it. One undo step with the stroke.
fn editor_place(world: &mut World, mut on_handle: Local<bool>) {
    let Some(size) = world.resource::<TownSettings>().selected else {
        return;
    };
    {
        let ed = world.resource::<crate::editor::Editor>();
        if ed.tab != Tab::Objects || ed.object_tool != ObjectTool::Place {
            return;
        }
    }
    let (started, ended) = {
        let s = world.resource::<crate::tools::Stroke>();
        (s.just_started, s.just_ended)
    };
    if started {
        // a press on an object or a handle selects or drags it instead
        *on_handle = world.resource::<crate::placement::Handles>().took_press;
    }
    if !ended || std::mem::take(&mut *on_handle) {
        return;
    }
    let (raw, start): (Vec<Vec2>, Option<Vec2>) = {
        let s = world.resource::<crate::tools::Stroke>();
        (
            s.path.iter().map(|p| p.xz()).collect(),
            s.start_hit.map(|h| h.xz()),
        )
    };
    let cam = world
        .query_filtered::<&GlobalTransform, With<crate::camera::MainCamera>>()
        .iter(world)
        .next()
        .map(|t| t.translation().xz());
    let area = match crate::setpieces::clean_outline(&raw) {
        Some(outline) => {
            let toward = cam.unwrap_or(outline[0]);
            Area::Outline(outline, toward)
        }
        None => match start {
            Some(at) => Area::Click(at),
            None => return,
        },
    };
    let req = {
        let s = world.resource::<TownSettings>();
        Request {
            size,
            area,
            style: s.style,
            roads: s.roads,
            walls: s.walls_for(size),
            buildings: None,
            seed: s.seed,
            lead_out: true,
            name: None,
        }
    };
    match place_town(world, &req) {
        Ok(r) => {
            info!(
                "TOWN {} placed: {} {} buildings, {} objects, {} roads over {:.0} m² ({:?})",
                r.name,
                r.size.id(),
                r.buildings,
                r.ids.len(),
                r.roads.len(),
                r.area,
                r.counts
            );
            toast(world, placed_message(&r));
            world.resource_mut::<TownSettings>().last = Some(LastTown {
                req,
                ids: r.ids,
                roads: r.roads,
            });
        }
        Err(e) => {
            warn!("TOWN {}: {e}", size.id());
            toast(world, e);
        }
    }
}

/// Shuffle: the last town again in the same place with the next seed and the current options
/// (one undo step), or just the next seed.
fn shuffle(world: &mut World) {
    let (last, seed) = {
        let mut s = world.resource_mut::<TownSettings>();
        if !std::mem::take(&mut s.shuffle) {
            return;
        }
        s.seed = s.seed.wrapping_add(1);
        (s.last.clone(), s.seed)
    };
    let Some(last) = last else { return };
    if !last.ids.iter().any(|id| {
        world
            .resource::<crate::objects::ObjectStore>()
            .items
            .contains_key(id)
    }) {
        world.resource_mut::<TownSettings>().last = None;
        return;
    }
    let req = {
        let s = world.resource::<TownSettings>();
        let size = s.selected.unwrap_or(last.req.size);
        Request {
            size,
            seed,
            style: s.style,
            roads: s.roads,
            walls: s.walls_for(size),
            ..last.req.clone()
        }
    };
    world
        .resource_mut::<crate::history::History>()
        .begin(format!("Shuffle {}", req.size.name().to_lowercase()));
    place::remove_town(world, &last.ids, &last.roads);
    let res = place_town(world, &req);
    world.resource_scope(|world, mut h: Mut<crate::history::History>| {
        h.commit(world.resource::<crate::objects::ObjectStore>())
    });
    match res {
        Ok(r) => {
            info!("TOWN shuffled {}: {} buildings", r.name, r.buildings);
            toast(world, placed_message(&r));
            world.resource_mut::<TownSettings>().last = Some(LastTown {
                req,
                ids: r.ids,
                roads: r.roads,
            });
        }
        Err(e) => {
            world.resource_mut::<TownSettings>().last = None;
            toast(world, e);
        }
    }
}

/// The outline a click would fill, under the cursor, with its main street's line.
fn cursor(
    mut towns: ResMut<TownSettings>,
    ed: Res<crate::editor::Editor>,
    pointer: Res<crate::camera::Pointer>,
    stroke: Res<crate::tools::Stroke>,
    terrain: Res<crate::terrain::Terrain>,
    water: Res<crate::water::WaterBodies>,
    mut gizmos: Gizmos,
) {
    let Some(size) = towns.selected else { return };
    if ed.tab != Tab::Objects
        || ed.object_tool != ObjectTool::Place
        || pointer.over_ui
        || stroke.active
    {
        return;
    }
    let Some(hit) = pointer.hit else { return };
    let at = hit.xz();
    // (turned again once the cursor moves on a few metres)
    let (facing, ok) = match towns.preview {
        Some((p, f, s, ok)) if p.distance(at) < 4.0 && s == size => (f, ok),
        _ => {
            let r = place::click_frame(&terrain, &water, at, size, None, towns.seed);
            let f = r.as_ref().map(|r| r.1).unwrap_or(0.0);
            towns.preview = Some((at, f, size, r.is_ok()));
            (f, r.is_ok())
        }
    };
    let e = size.extent();
    let col = if ok {
        Color::srgba(1.0, 0.85, 0.4, 0.9)
    } else {
        Color::srgba(1.0, 0.4, 0.3, 0.9)
    };
    let lift = |q: Vec2| Vec3::new(q.x, terrain.height_at(q.x, q.y) + 0.6, q.y);
    let ring: Vec<Vec3> = (0..=64)
        .map(|i| {
            lift(
                at + layout::rot2(
                    Vec2::from_angle(std::f32::consts::TAU * i as f32 / 64.0) * e,
                    facing,
                ),
            )
        })
        .collect();
    gizmos.linestrip(ring, col);
    let axis: Vec<Vec3> = (0..=24)
        .map(|i| {
            lift(at + layout::rot2(Vec2::new(-e.x + 2.0 * e.x * i as f32 / 24.0, 0.0), facing))
        })
        .collect();
    gizmos.linestrip(axis, col.with_alpha(0.5));
}
