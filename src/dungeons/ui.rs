//! The Dungeons tab (in the world) and the dungeon builder (in a dungeon's map).
//!
//! World: "Create a dungeon" → pick what kind (a layout or a realm) → pick its way in (click the
//! ground for a portal, an object, a spot, or a cave) → its card: name, how the game runs it, its
//! ways in, and "Open builder".
//!
//! Builder (Dungeon › Layout · Look · Lights): one-click layouts, drawing rooms right on the floor
//! of the level being edited (the view cut away above it), levels, a room's card (the inspector);
//! styles and moods; Auto-light and lights placed by hand. A glass capsule at the top says where
//! you are and takes you back to the world; the dock holds the drawing tool and its keys.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, FontId, RichText};

use super::edit::{NewDungeon, NewEntrance};
use super::lights::{LightDef, LightKind, Mount};
use super::model::*;
use super::mood;
use super::plan::{Ceiling, Liquid, Pillars, Role, RoomKind, Shape};
use super::presets::{self, Size};

use super::styles;
use crate::editor::Tab;
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// What a click in the view does next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    /// the way in of the dungeon being created
    New(EntranceKind),
    /// another way in of dungeon `id`
    Add(u32, EntranceKind),
}

/// Drawing tools of the builder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Draw {
    #[default]
    Select,
    Room,
    Round,
    Octagon,
    Hall,
    Corridor,
    Cavern,
    Stairs,
    Pit,
    Pool,
    Dais,
    Bridge,
    Gallery,
    Balcony,
    Chasm,
    Window,
    Alcove,
    Spiral,
    Erase,
    /// on a wall: a door (through to the room behind it, or a locked one in a niche)
    WallDoor,
    WallSecret,
    WallTorch,
    WallLantern,
}

impl Draw {
    /// Things put on a wall with a click (Layout › On the walls).
    pub const WALL: [Draw; 6] = [
        Draw::WallDoor,
        Draw::WallSecret,
        Draw::Window,
        Draw::Alcove,
        Draw::WallTorch,
        Draw::WallLantern,
    ];
    pub const ALL: [Draw; 17] = [
        Draw::Select,
        Draw::Room,
        Draw::Hall,
        Draw::Gallery,
        Draw::Round,
        Draw::Octagon,
        Draw::Corridor,
        Draw::Cavern,
        Draw::Stairs,
        Draw::Spiral,
        Draw::Balcony,
        Draw::Pit,
        Draw::Chasm,
        Draw::Pool,
        Draw::Dais,
        Draw::Bridge,
        Draw::Erase,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Draw::Select => "Select",
            Draw::Room => "Room",
            Draw::Round => "Round",
            Draw::Octagon => "Octagon",
            Draw::Hall => "Hall",
            Draw::Corridor => "Corridor",
            Draw::Cavern => "Cavern",
            Draw::Stairs => "Stairs",
            Draw::Pit => "Pit",
            Draw::Pool => "Pool",
            Draw::Dais => "Dais",
            Draw::Bridge => "Bridge",
            Draw::Gallery => "Gallery hall",
            Draw::Balcony => "Balcony",
            Draw::Chasm => "Chasm",
            Draw::Window => "Window",
            Draw::Alcove => "Alcove",
            Draw::Spiral => "Spiral stair",
            Draw::Erase => "Erase",
            Draw::WallDoor => "Door",
            Draw::WallSecret => "Secret door",
            Draw::WallTorch => "Torch",
            Draw::WallLantern => "Lantern",
        }
    }
    /// Its Central icon.
    pub fn icon(self) -> &'static str {
        match self {
            Draw::Select => "Cursor1",
            Draw::Room => "SquarePlaceholder",
            Draw::Round => "FormCircle",
            Draw::Octagon => "FormOctagon",
            Draw::Hall => "Columns3",
            Draw::Corridor => "LineThickness",
            Draw::Cavern => "FlowerShape",
            Draw::Stairs => "ChartWaterfall",
            Draw::Pit => "ArrowDown",
            Draw::Pool => "Drop",
            Draw::Dais => "FormSquare",
            Draw::Bridge => "Bridge",
            Draw::Gallery => "Columns3Wide",
            Draw::Balcony => "LayoutTop",
            Draw::Chasm => "Split",
            Draw::Window => "Window",
            Draw::Alcove => "LayoutBottom",
            Draw::Spiral => "ArrowRotateClockwise",
            Draw::Erase => "Eraser",
            Draw::WallDoor => "Door",
            Draw::WallSecret => "Keyhole",
            Draw::WallTorch => "Torch",
            Draw::WallLantern => "LightbulbGlow",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Draw::Select => {
                "Click a room to pick it (drag it to move it; Delete removes it). Drag a wall to push or pull it; Alt-drag a wall for an alcove; Shift-drag a room up or down for its height"
            }
            Draw::WallDoor => {
                "Click a wall: a door through to the room behind it (a short passage is cut), or a locked door set in the wall"
            }
            Draw::WallSecret => {
                "Click a wall: a secret door through to the room behind it (or set in the wall), hidden as wall"
            }
            Draw::WallTorch => "Click a wall: a torch in its sconce",
            Draw::WallLantern => "Click a wall: a lantern on its bracket",
            Draw::Room => "Drag on the floor: a room from corner to corner",
            Draw::Round => "Drag from the middle out: a round room",
            Draw::Octagon => "Drag from the middle out: an eight-sided room",
            Draw::Hall => "Drag corner to corner: a tall pillared hall",
            Draw::Corridor => "Click points along the way; double-click or Enter to finish",
            Draw::Cavern => "Click round its edge (or drag a loop); Enter to finish: natural rock",
            Draw::Stairs => {
                "Click where the stairs start, then where they end (on the level below)"
            }
            Draw::Pit => "Drag from the middle out: a shaft down to the level below",
            Draw::Pool => "Drag corner to corner: a sunken pool of water, lava or slime",
            Draw::Dais => "Drag corner to corner inside a room: a raised platform with steps",
            Draw::Bridge => "Click its two ends: a walkway across a pit or a chasm",
            Draw::Gallery => {
                "Drag corner to corner: a hall two storeys high with a gallery round it, an arcade and a stair up (rooms of the level above open onto it)"
            }
            Draw::Balcony => {
                "Drag corner to corner inside a tall room: a gallery at the level above's floor (railings where it overlooks the drop)"
            }
            Draw::Chasm => {
                "Click points along it; double-click or Enter to finish: a deep rift in the rock (bridge it)"
            }
            Draw::Window => {
                "Click a wall: an arched window through to the room beyond (or a blind one)"
            }
            Draw::Alcove => {
                "Click a wall: an arched recess in it (a statue, a shrine, a sarcophagus)"
            }
            Draw::Spiral => {
                "Drag from the middle out: a round tower stair winding to the level above (or below: the stairs' up / down switch); its landing faces the nearest room there"
            }
            Draw::Erase => "Click a room to remove it",
        }
    }
    fn kind(self) -> RoomKind {
        match self {
            Draw::Hall => RoomKind::Hall,
            Draw::Corridor => RoomKind::Corridor,
            Draw::Cavern => RoomKind::Cavern,
            Draw::Stairs => RoomKind::Stairs,
            Draw::Pit => RoomKind::Pit,
            Draw::Pool => RoomKind::Pool,
            Draw::Dais => RoomKind::Dais,
            Draw::Bridge => RoomKind::Bridge,
            Draw::Balcony => RoomKind::Balcony,
            Draw::Chasm => RoomKind::Chasm,
            _ => RoomKind::Room,
        }
    }
    /// Drawn by clicking points (not dragging).
    pub fn by_points(self) -> bool {
        matches!(
            self,
            Draw::Corridor | Draw::Cavern | Draw::Stairs | Draw::Bridge | Draw::Chasm
        )
    }
}

/// A drag on a room's wall or height (Select): what it started from.
#[derive(Clone)]
pub struct WallDrag {
    pub room: u32,
    pub mode: WallMode,
    /// the point of the wall grabbed (xz), the height grabbed at, the wall's outward direction
    pub at: Vec2,
    pub y: f32,
    pub out: Vec2,
    /// the regular shape before the drag (an irregular room's: the one it was made from)
    pub shape: Shape,
    pub height: f32,
    pub screen: Vec2,
    /// how far it is pushed out (+) or pulled in (−)
    pub d: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WallMode {
    /// the wall moves (the room grows or shrinks on that side)
    Push,
    /// an alcove is pushed out of the wall
    Alcove,
    /// the ceiling goes up or down
    Height,
}

/// What the panels and the view ask for (done by `apply_requests` with the world).
pub enum Request {
    Create(NewDungeon),
    AddEntrance(u32, NewEntrance),
    RemoveEntrance(u32, u32),
    Remove(u32),
    Def(u32, String, Box<dyn FnOnce(&mut DungeonDef) + Send + Sync>),
    Entrance(
        u32,
        u32,
        String,
        Box<dyn FnOnce(&mut Entrance) + Send + Sync>,
    ),
    /// a portal's frame style (asset id; "" = the liquid alone)
    SetFrame(u32, u32, String),
    Open(u32),
    Back,
    WalkIn,
    Map(String, Box<dyn FnOnce(&mut MapDoc) + Send + Sync>),
    /// the way back out's frame, colour or spot (its frame object goes along: `edit::way_out`)
    WayOut(String, Box<dyn FnOnce(&mut WayOut) + Send + Sync>),
    Layout {
        layout: String,
        size: Size,
        room_size: presets::RoomSize,
        irregular: f32,
        seed: u32,
    },
    AutoLight {
        density: f32,
    },
    /// a light with its fixture (a torch's sconce, a lantern's bracket) on the wall at a point
    WallFixture {
        kind: super::lights::LightKind,
        at: Vec3,
        normal: Vec3,
    },
    /// a light with its fixture standing on the floor at a point (a brazier, candles)
    FloorFixture {
        kind: super::lights::LightKind,
        at: Vec3,
    },
    /// Auto-light / Dress for one room (the walking build bar)
    AutoLightRoom(u32),
    DressRoom(u32),
    ClearAutoLights,
    Dress,
    ClearDressing,
    GoTo(Vec3, f32),
    TopView,
    /// a hint for the user (a tool that couldn't do what was asked)
    Toast(String),
    /// "Age the walls": cracks, stains, drips, blood, soot and runes laid out again
    AgeWalls {
        density: f32,
    },
    ClearAged,
    /// draw the minimaps and the walk check
    Survey,
}

#[derive(Resource)]
pub struct DungeonTool {
    pub requests: Vec<Request>,
    // ---- in the world
    pub creating: bool,
    pub new_layout: String,
    pub new_realm: bool,
    /// a new realm's land: shape, size, seed (`realm_shapes`)
    pub new_land: super::realm_shapes::Choice,
    /// a new temple compound's theme, courts and order (`compound`)
    pub new_compound: super::compound::Options,
    /// a new realm's design sheet (`sheets`), and the family the picker shows
    pub new_sheet: Option<super::sheets::SheetRef>,
    pub sheet_family: String,
    pub new_size: Size,
    pub new_room_size: presets::RoomSize,
    pub new_irregular: f32,
    pub new_art: super::styles::Art,
    pub new_style: String,
    pub new_mood: String,
    pub new_name: String,
    pub new_instancing: Instancing,
    pub new_color: PortalColor,
    /// a new portal's frame (`edit::FRAMES`); None: the first there is; Some(""): the liquid alone
    pub new_frame: Option<String>,
    /// an object way in to put down new (`edit::ENTRANCE_PROPS`) rather than pick
    pub new_prop: Option<String>,
    pub pick: Option<Pick>,
    pub name_edit: Option<(u32, String)>,
    pub confirm_remove: Option<u32>,
    // ---- in a dungeon's map
    pub draw: Draw,
    /// a drag being drawn: where it started (on the level's floor)
    pub drag_from: Option<Vec2>,
    /// points clicked so far (corridors, caverns, stairs, bridges)
    pub path: Vec<Vec2>,
    pub selected_room: Option<u32>,
    pub selected_light: Option<u32>,
    /// a light being dragged (Lights tab): its id and its offset from the cursor
    pub moving_light: Option<(u32, Vec3)>,
    /// a wall being pushed or pulled, an alcove being pushed out, a room's height being dragged
    pub wall_drag: Option<WallDrag>,
    /// a door asked for on a plain wall: its opening was cut; it goes into the doorway once the
    /// dungeon is rebuilt (kind, where, the build it waits for to change)
    pub pending_doors: Vec<(super::pieces::PieceKind, Vec3, u64)>,
    /// the walking build bar (Enter while walking a dungeon)
    pub build_bar: bool,
    pub build_text: String,
    /// what the walker looks at while the bar is open: the point, its surface's normal, the
    /// room it's in (`viewport::walk_look`)
    pub look: Option<(Vec3, Vec3, Option<u32>)>,
    /// words for the area agent, for the room being walked (`viewport::walk_prompt`)
    pub walk_prompt: Option<String>,
    /// the agent's progress while walking ("" when none runs)
    pub walk_status: String,
    /// the Layout tab's Pieces: clicks place this kind (doors snap into doorways)
    pub placing_piece: Option<super::pieces::PieceKind>,
    pub selected_piece: Option<u32>,
    pub placing_light: Option<LightKind>,
    /// Look tab: clicks put this decal on the surface under the cursor (Alt-click removes one)
    pub placing_decal: Option<super::decals::DecalKind>,
    pub decal_size: f32,
    pub age_density: f32,
    pub layout: String,
    pub size: Size,
    pub room_size: presets::RoomSize,
    pub irregular: f32,
    pub stairs_up: bool,
    /// grid the drawing snaps to (m); 0 = none
    pub snap: f32,
    pub light_density: f32,
    /// where the cursor meets the edited level's floor
    pub hover: Option<Vec2>,
    /// a room being dragged: (id, where the drag started, its shape then)
    pub moving: Option<(u32, Vec2, Shape)>,
    /// a piece being dragged: its id and where it sits from the pointer
    pub moving_piece: Option<(u32, Vec3)>,
    /// the way out being dragged: where its foot sits from the pointer
    pub moving_exit: Option<Vec3>,
    /// sliders change the map live; this step is recorded when the mouse lets go
    pub live: Option<(String, Arc<MapDoc>)>,
    pub live_list: Option<(String, Arc<Dungeons>)>,
    /// a room's name being typed: (room, text)
    pub room_name: Option<(u32, String)>,
    /// Create a dungeon › Describe it: the description, ideas, pictures, and the dungeon being
    /// made from them (`describe`)
    pub describe: super::describe::Describe,
}

impl Default for DungeonTool {
    fn default() -> Self {
        Self {
            requests: vec![],
            creating: false,
            new_layout: "crypt".into(),
            new_realm: false,
            new_land: Default::default(),
            new_compound: Default::default(),
            new_sheet: None,
            sheet_family: String::new(),
            new_size: Size::Medium,
            new_room_size: Default::default(),
            new_irregular: 0.0,
            new_art: super::styles::Art::HandPainted,
            new_style: String::new(),
            new_mood: String::new(),
            new_name: String::new(),
            new_instancing: Instancing::Party,
            new_color: PortalColor::Arcane,
            new_frame: None,
            new_prop: None,
            pick: None,
            name_edit: None,
            confirm_remove: None,
            draw: Draw::Select,
            drag_from: None,
            path: vec![],
            selected_room: None,
            selected_light: None,
            moving_light: None,
            wall_drag: None,
            pending_doors: Vec::new(),
            build_bar: false,
            build_text: String::new(),
            look: None,
            walk_prompt: None,
            walk_status: String::new(),
            placing_light: None,
            placing_decal: None,
            placing_piece: None,
            selected_piece: None,
            moving_piece: None,
            decal_size: 1.6,
            age_density: 1.0,
            layout: "crypt".into(),
            size: Size::Medium,
            room_size: Default::default(),
            irregular: 0.0,
            stairs_up: false,
            snap: 1.0,
            light_density: 1.0,
            hover: None,
            moving: None,
            moving_exit: None,
            live: None,
            live_list: None,
            room_name: None,
            describe: Default::default(),
        }
    }
}

fn snap(p: Vec2, s: f32) -> Vec2 {
    if s <= 0.0 { p } else { (p / s).round() * s }
}

// ------------------------------------------------------------------ Halo pieces for the dungeon panels

/// Small widgets the dungeon panels share, on top of `ui::widgets` (labelled rows, text fields in
/// a dark well, colour rows, equal-width button rows, closed folds, the capsule's buttons).
pub(crate) mod kit {
    use std::ops::RangeInclusive;

    use bevy_egui::egui::{
        self, Color32, CornerRadius, FontId, Response, RichText, Sense, Stroke, StrokeKind,
        TextureId, Ui, pos2, vec2,
    };

    use crate::ui::theme::*;
    use crate::ui::widgets::*;

    /// A dim label, then the rest of the row for `add` (a segmented control, a dropdown…).
    pub fn lab_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(RichText::new(label).color(TEXT_DIM));
            add(ui)
        })
        .inner
    }

    /// The dark well of an input, full width.
    pub fn well<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
        egui::Frame::new()
            .fill(WELL)
            .corner_radius(CornerRadius::same(R_INPUT))
            .inner_margin(egui::Margin::symmetric(9, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add(ui)
            })
            .inner
    }

    /// A text field: its label above (dim), the input in a dark well.
    pub fn field(ui: &mut Ui, label: &str, text: &mut String, hint: &str) -> Response {
        if !label.is_empty() {
            ui.label(RichText::new(label).color(TEXT_DIM));
        }
        well(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .hint_text(RichText::new(hint).color(TEXT_MUTE))
                    .desired_width(ui.available_width()),
            )
        })
    }

    /// A multi-line text field in a dark well.
    pub fn text_area(ui: &mut Ui, text: &mut String, hint: &str, rows: usize) -> Response {
        well(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .hint_text(RichText::new(hint).color(TEXT_MUTE))
                    .font(FontId::proportional(13.5))
                    .desired_rows(rows)
                    .desired_width(ui.available_width()),
            )
        })
    }

    /// A small heading inside a section (12 px, dim).
    pub fn sub_head(ui: &mut Ui, text: &str) {
        ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
    }

    /// A Central icon on its own in a row of text.
    pub fn icon_label(ui: &mut Ui, icon: &str, size: f32, color: Color32) -> Response {
        let (r, resp) = ui.allocate_exact_size(vec2(size + 2.0, size + 2.0), Sense::hover());
        paint_icon(ui, r.center(), icon, size, color);
        resp
    }

    /// A colour: a 26 px swatch (click: the picker), its label, its hex code. The value is egui's
    /// linear RGB, as `color_edit_button_rgb` takes it.
    pub fn color_row(ui: &mut Ui, label: &str, rgb: &mut [f32; 3]) -> Response {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let r = ui
                .scope(|ui| {
                    ui.spacing_mut().interact_size = vec2(26.0, 26.0);
                    ui.color_edit_button_rgb(rgb)
                })
                .inner;
            let c = Color32::from(egui::Rgba::from_rgb(rgb[0], rgb[1], rgb[2]));
            let p = ui.painter();
            let sw = r.rect.expand(1.0);
            p.rect_filled(sw, CornerRadius::same(8), c);
            p.rect_stroke(
                sw,
                CornerRadius::same(8),
                Stroke::new(
                    1.0,
                    if r.hovered() {
                        Color32::from_white_alpha(110)
                    } else {
                        LINE_STRONG
                    },
                ),
                StrokeKind::Inside,
            );
            ui.label(RichText::new(label).color(TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()))
                        .font(FontId::new(11.0, mono()))
                        .color(TEXT_MUTE),
                );
            });
            r
        })
        .inner
    }

    /// A round colour to pick (portal colours): a white ring with a dark gap when picked.
    pub fn color_dot(ui: &mut Ui, c: Color32, on: bool, tip: &str) -> Response {
        let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
        let p = ui.painter();
        let cc = r.center();
        if on {
            p.circle_filled(cc, 11.0, Color32::WHITE);
            p.circle_filled(cc, 9.0, ON_ACCENT);
            p.circle_filled(cc, 7.5, c);
        } else {
            p.circle_filled(cc, 9.5, c);
            p.circle_stroke(
                cc,
                9.5,
                Stroke::new(
                    1.0,
                    Color32::from_white_alpha(if resp.hovered() { 140 } else { 50 }),
                ),
            );
        }
        resp.on_hover_text(tip)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// How a button in a [`btn_row`] looks.
    #[derive(Clone, Copy, PartialEq)]
    pub enum Btn {
        /// soft
        Soft,
        /// white: the call to action
        Pri,
        /// soft, red on hover (Remove…)
        Bad,
        /// red: confirming a removal, stopping a run
        Danger,
    }

    /// A full-width button of kind `k`.
    pub fn button_of(ui: &mut Ui, icon: &str, text: &str, k: Btn) -> Response {
        match k {
            Btn::Soft => wide_icon_button(ui, icon, text),
            Btn::Pri => primary_icon_button(ui, icon, text),
            Btn::Bad => colored_button(ui, icon, text, false, 30.0),
            Btn::Danger => colored_button(ui, icon, text, true, 30.0),
        }
    }

    /// Buttons side by side, equal widths (the prototype's `.btns`).
    pub fn btn_row(ui: &mut Ui, items: &[(&str, &str, Btn)]) -> Vec<Response> {
        let n = items.len().max(1) as f32;
        let w = ((ui.available_width() - 6.0 * (n - 1.0)) / n).floor();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            items
                .iter()
                .map(|(ic, t, k)| {
                    ui.allocate_ui(vec2(w, 30.0), |ui| {
                        ui.set_width(w);
                        button_of(ui, ic, t, *k)
                    })
                    .inner
                })
                .collect()
        })
        .inner
    }

    /// A soft button whose text turns red on hover (`solid`: a red button), full width.
    pub fn colored_button(ui: &mut Ui, icon: &str, text: &str, solid: bool, h: f32) -> Response {
        let w = ui.available_width();
        colored_button_w(ui, icon, text, solid, w, h)
    }

    fn colored_button_w(
        ui: &mut Ui,
        icon: &str,
        text: &str,
        solid: bool,
        w: f32,
        h: f32,
    ) -> Response {
        let size = if h < 28.0 { 12.0 } else { 13.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        if ui.is_rect_visible(rect) {
            let hot = resp.hovered();
            let down = resp.is_pointer_button_down_on();
            let fill = match (solid, hot, down) {
                (true, _, true) => DANGER.gamma_multiply(0.85),
                (true, true, _) => DANGER.lerp_to_gamma(Color32::WHITE, 0.12),
                (true, ..) => DANGER,
                (false, _, true) => Color32::from_white_alpha(46),
                (false, true, _) => SOFT_HOVER,
                _ => SOFT,
            };
            let fg = if solid {
                ON_ACCENT
            } else if hot {
                DANGER
            } else {
                TEXT
            };
            let p = ui.painter();
            p.rect_filled(
                rect,
                CornerRadius::same(if h < 28.0 { R_SMALL } else { R_BUTTON }),
                fill,
            );
            let g = p.layout_no_wrap(text.to_owned(), FontId::new(size, semibold()), fg);
            let iw = if icon.is_empty() { 0.0 } else { size + 6.0 };
            let x = rect.center().x - (g.size().x + iw) * 0.5;
            if !icon.is_empty() {
                paint_icon(
                    ui,
                    pos2(x + size * 0.5, rect.center().y),
                    icon,
                    size + 1.0,
                    fg,
                );
            }
            ui.painter()
                .galley(pos2(x + iw, rect.center().y - g.size().y * 0.5), g, fg);
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// A small red button sized to its text (Stop).
    pub fn small_danger_button(ui: &mut Ui, icon: &str, text: &str) -> Response {
        let g =
            ui.painter()
                .layout_no_wrap(text.to_owned(), FontId::new(12.0, semibold()), ON_ACCENT);
        let w = g.size().x + 16.0 + if icon.is_empty() { 0.0 } else { 18.0 };
        colored_button_w(ui, icon, text, true, w, 24.0)
    }

    /// A fold ([`section_fold`]) that starts closed; its open state is remembered like any other.
    pub fn fold_closed<R>(
        ui: &mut Ui,
        salt: &str,
        title: &str,
        note: &str,
        body: impl FnOnce(&mut Ui) -> R,
    ) -> Option<R> {
        let id = ui.make_persistent_id(("section_fold", salt));
        ui.data_mut(|d| {
            d.get_persisted_mut_or(id, false);
        });
        section_fold(ui, salt, title, note, body)
    }

    fn check(p: &egui::Painter, c: egui::Pos2, color: Color32) {
        let s = 4.5;
        p.add(egui::Shape::line(
            vec![
                c + vec2(-s, 0.1 * s),
                c + vec2(-0.3 * s, 0.8 * s),
                c + vec2(s, -0.8 * s),
            ],
            Stroke::new(1.6, color),
        ));
    }

    /// A dropdown row led by a colour swatch (dungeon styles), a check on the current one.
    pub fn swatch_item(ui: &mut Ui, color: Color32, label: &str, checked: bool) -> Response {
        lead_item(ui, Lead2::Swatch(color), label, checked)
    }

    /// A dropdown row led by a little picture (realm land shapes), a check on the current one.
    pub fn picture_item(
        ui: &mut Ui,
        tex: Option<TextureId>,
        label: &str,
        checked: bool,
    ) -> Response {
        lead_item(ui, Lead2::Picture(tex), label, checked)
    }

    #[derive(Clone, Copy)]
    enum Lead2 {
        Swatch(Color32),
        Picture(Option<TextureId>),
    }

    fn lead_item(ui: &mut Ui, lead: Lead2, label: &str, checked: bool) -> Response {
        let (lw, h) = match lead {
            Lead2::Swatch(_) => (14.0, 28.0),
            Lead2::Picture(_) => (34.0, 34.0),
        };
        let g = ui
            .painter()
            .layout_no_wrap(label.to_owned(), FontId::proportional(13.0), TEXT);
        let mut resp = ui.allocate_response(
            vec2(9.0 + lw + 10.0 + g.size().x + 18.0 + 22.0, h),
            Sense::click(),
        );
        let rect = resp.rect;
        if resp.clicked() {
            resp.mark_changed();
            ui.close();
        }
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect, CornerRadius::same(8), SOFT_HOVER);
        } else if checked {
            p.rect_filled(rect, CornerRadius::same(8), SOFT);
        }
        let lr = egui::Rect::from_center_size(
            pos2(rect.left() + 9.0 + lw * 0.5, rect.center().y),
            vec2(lw, if lw > 20.0 { 26.0 } else { lw }),
        );
        match lead {
            Lead2::Swatch(c) => {
                p.rect_filled(lr, CornerRadius::same(4), c);
                p.rect_stroke(
                    lr,
                    CornerRadius::same(4),
                    Stroke::new(1.0, LINE_STRONG),
                    StrokeKind::Inside,
                );
            }
            Lead2::Picture(Some(t)) => {
                let a = lr.height() / lr.width();
                let uv =
                    egui::Rect::from_min_max(pos2(0.0, 0.5 - a * 0.5), pos2(1.0, 0.5 + a * 0.5));
                egui::Image::new((t, lr.size()))
                    .uv(uv)
                    .corner_radius(5)
                    .paint_at(ui, lr);
            }
            Lead2::Picture(None) => {
                p.rect_filled(lr, CornerRadius::same(5), SOFT);
            }
        }
        let p = ui.painter();
        p.galley(
            pos2(lr.right() + 10.0, rect.center().y - g.size().y * 0.5),
            g,
            TEXT,
        );
        if checked {
            check(p, pos2(rect.right() - 15.0, rect.center().y), TEXT);
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// A whole number with − / + (the value `skip` is stepped over). Returns true when changed.
    pub fn stepper(
        ui: &mut Ui,
        label: &str,
        v: &mut i32,
        range: RangeInclusive<i32>,
        skip: i32,
    ) -> bool {
        let mut changed = false;
        let step = |v: i32, d: i32| if v + d == skip { v + 2 * d } else { v + d };
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icon_button(ui, "PlusSmall", "One level up").clicked()
                    && range.contains(&step(*v, 1))
                {
                    *v = step(*v, 1);
                    changed = true;
                }
                ui.label(
                    RichText::new(format!("{v}"))
                        .font(FontId::new(12.0, mono()))
                        .color(TEXT),
                );
                if icon_button(ui, "MinusSmall", "One level down").clicked()
                    && range.contains(&step(*v, -1))
                {
                    *v = step(*v, -1);
                    changed = true;
                }
            });
        });
        changed
    }

    /// A button in a glass capsule (icon + text, 28 px): soft when `on`, white when `white`.
    pub fn cap_button(ui: &mut Ui, icon: &str, label: &str, on: bool, white: bool) -> Response {
        let font = FontId::new(
            12.5,
            if white {
                semibold()
            } else {
                egui::FontFamily::Proportional
            },
        );
        let g = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);
        let w = 10.0 + 14.0 + 6.0 + g.size().x + 11.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
        if ui.is_rect_visible(rect) {
            let hot = resp.hovered();
            let p = ui.painter();
            let bg = if white {
                Some(if resp.is_pointer_button_down_on() {
                    Color32::from_rgb(214, 220, 228)
                } else if hot {
                    ACCENT_HOVER
                } else {
                    Color32::WHITE
                })
            } else if on {
                Some(SOFT_HOVER)
            } else if hot {
                Some(SOFT)
            } else {
                None
            };
            if let Some(bg) = bg {
                p.rect_filled(rect, CornerRadius::same(9), bg);
            }
            let col = if white {
                ON_ACCENT
            } else if hot || on {
                TEXT
            } else {
                TEXT_DIM
            };
            paint_icon(
                ui,
                pos2(rect.left() + 10.0 + 7.0, rect.center().y),
                icon,
                14.0,
                col,
            );
            ui.painter().galley_with_override_text_color(
                pos2(rect.left() + 30.0, rect.center().y - g.size().y * 0.5),
                g,
                col,
            );
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// A thin divider spanning a capsule's height.
    pub fn cap_sep(ui: &mut Ui) {
        let (r, _) = ui.allocate_exact_size(vec2(1.0, 28.0), Sense::hover());
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                pos2(r.left(), r.top() - 5.0),
                pos2(r.right(), r.bottom() + 5.0),
            ),
            0.0,
            LINE,
        );
    }

    /// A pill button with an icon (the walking build bar).
    pub fn icon_chip(ui: &mut Ui, icon: &str, label: &str) -> Response {
        let g = ui
            .painter()
            .layout_no_wrap(label.to_owned(), FontId::proportional(12.5), TEXT);
        let (rect, resp) = ui.allocate_exact_size(
            vec2(10.0 + 14.0 + 6.0 + g.size().x + 11.0, 28.0),
            Sense::click(),
        );
        if ui.is_rect_visible(rect) {
            let hot = resp.hovered();
            let p = ui.painter();
            p.rect_filled(
                rect,
                CornerRadius::same(255),
                if hot { SOFT_HOVER } else { SOFT },
            );
            p.rect_stroke(
                rect,
                CornerRadius::same(255),
                Stroke::new(1.0, LINE),
                StrokeKind::Inside,
            );
            paint_icon(
                ui,
                pos2(rect.left() + 17.0, rect.center().y),
                icon,
                14.0,
                if hot { TEXT } else { TEXT_DIM },
            );
            ui.painter().galley(
                pos2(rect.left() + 30.0, rect.center().y - g.size().y * 0.5),
                g,
                TEXT,
            );
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

use kit::*;

// ------------------------------------------------------------------ Central icons

/// The one-click layouts' and the realms' icons (Central names; the presets keep their own
/// glyphs for the guided setup).
pub(crate) fn layout_icon(id: &str) -> &'static str {
    match id {
        "crypt" => "Ghost",
        "catacombs" => "Grid",
        "prison_keep" => "Lock",
        "sun_temple" => "Sun",
        "cathedral" => "Castle",
        "dwarven_halls" => "Anvil",
        "underground_city" => "Buildings",
        "great_caverns" => "FormPyramide",
        "goblin_cave" => "Goblin",
        "spider_den" => "Ladybug",
        "sewers" => "Drop",
        "mine" => "Hammer",
        "clockwork" => "SettingsGear1",
        "frost_halls" => "SnowFlakes",
        "infernal" => "Fire1",
        "ruined_fortress" => "Tower",
        "empty" => "SquarePlaceholder",
        "desert_realm" => "SunHigh",
        "frozen_realm" => "SnowFlakes",
        "jungle_realm" => "Tree",
        "ember_realm" => "Fire1",
        "twilight_realm" => "MoonStar",
        "wild_realm" => "Earth",
        id if id == super::compound::LAYOUT => "Bank",
        _ => "Castle",
    }
}

/// A dungeon's icon by its kind and layout.
pub(crate) fn def_icon(kind: MapKind, layout: &str) -> &'static str {
    match (kind, layout_icon(layout)) {
        (MapKind::Realm, "Castle") => "CloudySun",
        (_, ic) => ic,
    }
}

fn way_icon(k: EntranceKind) -> &'static str {
    match k {
        EntranceKind::Portal => "BlackHole",
        EntranceKind::Object => "Door",
        EntranceKind::Point => "MapPin",
        EntranceKind::Cave => "Shovel",
    }
}

fn light_icon(k: LightKind) -> &'static str {
    match k {
        LightKind::Torch => "Torch",
        LightKind::Brazier => "Fire1",
        LightKind::HangingBrazier => "Bucket",
        LightKind::Chandelier => "LightbulbGlow",
        LightKind::Candles => "Fire2",
        LightKind::Lantern => "LightBulb",
        LightKind::Crystal => "Diamond",
        LightKind::WorkLamp => "SunLow",
        LightKind::Shaft => "SunHigh",
        LightKind::Glow => "Sparkle",
        LightKind::Spot => "Spotlight",
    }
}

fn piece_icon(k: super::pieces::PieceKind) -> &'static str {
    use super::pieces::PieceKind as P;
    match k {
        P::Door | P::DoubleDoor => "Door",
        P::Portcullis => "Grid",
        P::Bars => "Columns3",
        P::SecretDoor => "Keyhole",
        P::Lever => "Pointer",
        P::PressurePlate => "FormSquare",
        P::SpikeTrap => "ExclamationTriangle",
        P::Arena => "Sword",
        P::Checkpoint => "Flag1",
        P::Loot => "Treasure",
        P::Spawn => "Goblin",
    }
}

/// A portal colour as one swatch (between its core and its rim).
pub(crate) fn portal_rgb(pc: PortalColor) -> Color32 {
    let (core, rim) = pc.colors();
    Color32::from_rgb(
        ((core[0] * 0.5 + rim[0] * 0.5) * 255.0) as u8,
        ((core[1] * 0.5 + rim[1] * 0.5) * 255.0) as u8,
        ((core[2] * 0.5 + rim[2] * 0.5) * 255.0) as u8,
    )
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

fn m1(v: f32) -> String {
    format!("{v:.1} m")
}

// ------------------------------------------------------------------ the world: Dungeons tab

pub fn dungeons_left(ui: &mut egui::Ui, s: &mut UiState) {
    // a dungeon being made from a description (Describe it): its progress, then its result
    super::describe::progress_card(ui, s);
    if s.dungeon_map.0.is_some() {
        // (in a dungeon's map this tab is the builder)
        layout_left(ui, s);
        return;
    }
    crate::ui::tips::card(
        ui,
        "dungeons_intro",
        "Dungeons",
        &[
            "Press Create a dungeon and describe it in your own words (or pick an idea): it's laid out, styled, lit and dressed for you, and a connected AI shapes it to match.",
            "Or choose step by step what kind it is: a crypt, a temple, caverns, a realm under its own sky…",
            "Pick how players get in: a liquid portal you place anywhere, an object (a cellar door, a well), a spot, or the end of a cave.",
            "Press Open builder: the dungeon is a map of its own. Lay it out in one click, draw rooms, style and light it, then go Back to the world.",
        ],
    );
    if s.dungeon_tool.creating {
        create_flow(ui, s);
        return;
    }
    let t = &mut *s.dungeon_tool;
    match t.pick {
        None => {
            if primary_icon_button(ui, "PlusMedium", "Create a dungeon").clicked() {
                t.creating = true;
            }
        }
        Some(p) => {
            // (also in the dock and over the view)
            let k = match p {
                Pick::New(k) | Pick::Add(_, k) => k,
            };
            let mut cancel = false;
            section_box(ui, "dg_picking", "Pick the way in", |ui| {
                dim(ui, k.hint());
                cancel = wide_icon_button(ui, "CrossSmall", "Cancel (Esc)").clicked();
            });
            if cancel {
                t.pick = None;
            }
        }
    }
    // the list
    let n = s.dungeons.list.len();
    let mut pick = None;
    section_fold(ui, "dg_list", "In this world", &n.to_string(), |ui| {
        if n == 0 {
            hint_small(ui, "No dungeons yet.");
        }
        ui.spacing_mut().item_spacing.y = 2.0;
        for d in &s.dungeons.list {
            let sel = s.dungeons.selected == Some(d.id);
            let ways = d.entrances.len();
            let kind = if d.kind == MapKind::Realm {
                presets::realm(&d.layout).map(|r| r.name).unwrap_or("Realm")
            } else {
                presets::get(&d.layout).map(|p| p.name).unwrap_or("Dungeon")
            };
            let sub = format!(
                "{} · {} · {} way{} in",
                kind,
                d.instancing.name(),
                ways,
                if ways == 1 { "" } else { "s" }
            );
            if list_row(
                ui,
                Lead::Icon(def_icon(d.kind, &d.layout)),
                &d.name,
                &sub,
                "",
                sel,
            )
            .clicked()
            {
                pick = Some(d.id);
            }
        }
    });
    if let Some(id) = pick {
        s.dungeons.selected = Some(id);
    }
    hint_small(
        ui,
        "Dungeons are maps of their own behind a way in. Open one to build it: rooms, halls, stairs, mood and lights.",
    );
}

fn create_flow(ui: &mut egui::Ui, s: &mut UiState) {
    // two ways to make one: describe it, or choose step by step
    let t = &mut *s.dungeon_tool;
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            cancel = small_icon_button(ui, "CrossSmall", "Cancel").clicked();
            let mut page = t.describe.page;
            if segmented(
                ui,
                &mut page,
                &[(false, "Step by step"), (true, "Describe it")],
            ) {
                t.describe.page = page;
                if !page {
                    t.describe.picking = false;
                }
            }
        });
    });
    if cancel {
        t.creating = false;
        t.describe.picking = false;
        return;
    }
    if t.describe.page {
        super::describe::page(ui, s);
        return;
    }
    step_by_step(ui, s);
}

/// Create a dungeon › Step by step: the art, what kind, its way in.
fn step_by_step(ui: &mut egui::Ui, s: &mut UiState) {
    let ctx = ui.ctx().clone();
    let lib = s.lib.as_deref();
    let t = &mut *s.dungeon_tool;
    let mut toast = None;
    section_fold(ui, "dg_art", "1 · Art style", "", |ui| {
        toast = art_picker(ui, &ctx, &mut t.new_art);
    });
    section_fold(ui, "dg_kind", "2 · What kind of dungeon?", "", |ui| {
        kind_choice(ui, t)
    });
    // the way in: picked here, put down with the button
    let way_id = egui::Id::new("dungeon_new_way");
    let prop_id = egui::Id::new("dungeon_new_way_prop");
    let mut way: EntranceKind = ctx
        .data(|d| d.get_temp(way_id))
        .unwrap_or(EntranceKind::Portal);
    let mut prop: Option<String> = ctx.data(|d| d.get_temp(prop_id)).unwrap_or(None);
    let have = entrance_props(lib);
    section_fold(ui, "dg_way", "3 · How do players get in?", "", |ui| {
        let items: Vec<(&str, &str, &str)> = EntranceKind::ALL
            .iter()
            .map(|k| (way_icon(*k), k.name(), k.hint()))
            .collect();
        let sel = EntranceKind::ALL.iter().position(|k| *k == way);
        if let Some(i) = tool_grid(ui, 2, &items, sel) {
            way = EntranceKind::ALL[i];
        }
        match way {
            EntranceKind::Portal => {
                if let Some(f) = frame_dropdown(
                    ui,
                    "dg_new_frame",
                    lib,
                    t.new_frame
                        .as_deref()
                        .or(super::edit::default_frame(lib))
                        .unwrap_or(""),
                ) {
                    t.new_frame = Some(f);
                }
                portal_colors(ui, &mut t.new_color);
            }
            EntranceKind::Cave => {
                portal_colors(ui, &mut t.new_color);
            }
            EntranceKind::Object if !have.is_empty() => {
                // or one of the ready-made ways in, put down where the ground is clicked
                sub_head(ui, "Or put down");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                    if chip(ui, "One in the world", None, prop.is_none()).on_hover_text(EntranceKind::Object.hint()).clicked() {
                        prop = None;
                    }
                    for (a, name) in &have {
                        if chip(ui, name, None, prop.as_deref() == Some(a.as_str())).on_hover_text("Click the ground next: it stands there and leads into the dungeon (no portal)").clicked() {
                            prop = Some(a.clone());
                        }
                    }
                });
            }
            _ => {}
        }
        hint_small(ui, way.hint());
    });
    ctx.data_mut(|d| {
        d.insert_temp(way_id, way);
        d.insert_temp(prop_id, prop.clone());
    });
    if primary_icon_button(ui, "Castle", "Create and put down the way in")
        .on_hover_text(way.hint())
        .clicked()
    {
        t.new_prop = if way == EntranceKind::Object {
            prop.filter(|p| have.iter().any(|(a, _)| a == p))
        } else {
            None
        };
        t.pick = Some(Pick::New(way));
        t.creating = false;
    }
    if wide_icon_button(ui, "ArrowRight", "No way in yet")
        .on_hover_text("Make the dungeon now; add its way in later")
        .clicked()
    {
        let n = new_dungeon(t, None);
        t.requests.push(Request::Create(n));
        t.creating = false;
    }
    if let Some(m) = toast {
        s.ed.toast(m);
    }
}

/// Create a dungeon › 2: underground (a layout, its size and rooms) or a realm (its land), how
/// the game runs it, its name.
fn kind_choice(ui: &mut egui::Ui, t: &mut DungeonTool) {
    let mut realm = t.new_realm;
    if segmented_icons(
        ui,
        &mut realm,
        &[
            (false, "Castle", "Underground"),
            (true, "CloudySun", "Realm (open sky)"),
        ],
        false,
    ) {
        t.new_realm = realm;
        t.new_layout = if realm {
            presets::realms()[0].id.into()
        } else {
            "crypt".into()
        };
        t.new_land = super::realm_shapes::Choice::for_realm(&t.new_layout, t.new_land.seed);
    }
    if t.new_realm {
        let mut picked = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for r in presets::realms() {
                if chip(ui, r.name.trim_end_matches(" Realm"), None, t.new_layout == r.id).on_hover_text(r.blurb).clicked() && t.new_layout != r.id {
                    picked = Some(r.id);
                }
            }
            if chip(ui, "From a design sheet", None, t.new_layout == super::sheets::LAYOUT).on_hover_text("An open-air dungeon built from a painted level-design sheet: its zones, paths, stairs, water and boss rings shaped into the land, with markers where its objects go").clicked() && t.new_layout != super::sheets::LAYOUT {
                picked = Some(super::sheets::LAYOUT);
            }
        });
        if let Some(id) = picked {
            t.new_layout = id.into();
            // (its own shape and size to start from)
            t.new_land = super::realm_shapes::Choice::for_realm(id, t.new_land.seed);
        }
        if let Some(r) = presets::realm(&t.new_layout) {
            hint_small(ui, r.blurb);
        }
        // the land: its shape, size and a preview to shuffle (a temple compound: its theme,
        // courts and order, and its layout on the land)
        if t.new_layout == super::compound::LAYOUT {
            super::compound::picker::picker(ui, &mut t.new_compound, &mut t.new_land);
        } else if t.new_layout == super::sheets::LAYOUT {
            super::sheets::ui::picker(ui, t);
        } else {
            super::realm_shapes::picker(ui, &mut t.new_land, &t.new_layout);
        }
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for p in presets::all() {
                if chip(ui, p.name, None, t.new_layout == p.id)
                    .on_hover_text(p.blurb)
                    .clicked()
                {
                    t.new_layout = p.id.into();
                }
            }
        });
        if let Some(p) = presets::get(&t.new_layout) {
            hint_small(ui, p.blurb);
        }
        lab_row(ui, "Size", |ui| {
            segmented(
                ui,
                &mut t.new_size,
                &[
                    (Size::Small, "Small"),
                    (Size::Medium, "Medium"),
                    (Size::Large, "Large"),
                ],
            )
        });
        room_size_row(ui, &mut t.new_room_size);
        irregular_row(ui, &mut t.new_irregular);
    }
    lab_row(ui, "Runs as", |ui| {
        segmented(
            ui,
            &mut t.new_instancing,
            &[
                (Instancing::Shared, "Shared"),
                (Instancing::Party, "Party"),
                (Instancing::Raid, "Raid"),
            ],
        )
    });
    hint_small(
        ui,
        "How the game runs it: one copy for everyone, a private copy per group, or a raid.",
    );
    field(ui, "Name", &mut t.new_name, "made for you");
}

/// Hand-painted or photoreal (photoreal: coming soon). Returns a toast for a click on the one
/// that isn't ready.
fn art_picker(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    art: &mut super::styles::Art,
) -> Option<&'static str> {
    use super::styles::Art;
    let mut toast = None;
    let w = ((ui.available_width() - 6.0) / 2.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (a, pic, sub) in [
            (
                Art::HandPainted,
                "art_handpainted",
                "Painted textures and models",
            ),
            (Art::Photoreal, "art_photoreal", "Coming soon"),
        ] {
            let ready = a.ready();
            let r = ui
                .scope(|ui| {
                    if !ready {
                        ui.multiply_opacity(0.55);
                    }
                    image_card(
                        ui,
                        crate::ui::pictures::get(ctx, pic),
                        a.name(),
                        sub,
                        *art == a,
                        egui::vec2(w, (w * 0.625).round()),
                    )
                })
                .inner;
            let r = if ready {
                r
            } else {
                r.on_hover_text(
                    "Photoreal dungeons (photoscanned walls, floors and props) are being made",
                )
            };
            if r.clicked() {
                if ready {
                    *art = a;
                } else {
                    toast = Some("Photoreal dungeons are being made: hand-painted for now");
                }
            }
        }
    });
    toast
}

/// The portal styles (frames) as a dropdown: returns the one picked ("" = the liquid alone).
fn frame_dropdown(
    ui: &mut egui::Ui,
    id: &str,
    lib: Option<&crate::objects::Library>,
    current: &str,
) -> Option<String> {
    let mut out = None;
    let shown = super::edit::FRAMES
        .iter()
        .find(|f| f.asset == current)
        .map(|f| f.name.to_string())
        .unwrap_or_else(|| {
            if current.is_empty() {
                "Liquid only".into()
            } else {
                lib.and_then(|l| l.get(current))
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| current.to_string())
            }
        });
    dropdown_row(ui, "Frame", id, shown, |ui| {
        for f in super::edit::FRAMES {
            let have = lib.is_some_and(|l| l.get(f.asset).is_some());
            let r = ui
                .add_enabled_ui(have, |ui| menu_check(ui, current == f.asset, f.name))
                .inner;
            let r = if have {
                r
            } else {
                r.on_disabled_hover_text(
                    "Being modelled: it shows up here when it's in the library",
                )
            };
            if r.clicked() {
                out = Some(f.asset.to_string());
            }
        }
        if menu_check(ui, current.is_empty(), "Liquid only")
            .on_hover_text("No frame: the liquid stands on its own (put it in a doorway you built)")
            .clicked()
        {
            out = Some(String::new());
        }
    });
    out
}

/// The ready-made ways in there are (a cellar door, a trapdoor, an old well): (asset, name).
fn entrance_props(lib: Option<&crate::objects::Library>) -> Vec<(String, String)> {
    super::edit::ENTRANCE_PROPS
        .iter()
        .filter_map(|a| {
            lib.and_then(|l| l.get(a))
                .map(|e| (a.to_string(), e.name.clone()))
        })
        .collect()
}

/// The portal colours as round swatches.
fn portal_colors(ui: &mut egui::Ui, c: &mut PortalColor) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        ui.label(RichText::new("Colour").color(TEXT_DIM));
        ui.add_space(4.0);
        for pc in PortalColor::ALL {
            if color_dot(ui, portal_rgb(pc), *c == pc, pc.name()).clicked() && *c != pc {
                *c = pc;
                changed = true;
            }
        }
    });
    changed
}

/// Rooms: Normal / Large / Extra large (`presets::RoomSize`), under the how-many-rooms choice.
fn room_size_row(ui: &mut egui::Ui, v: &mut presets::RoomSize) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new("Rooms").color(TEXT_DIM)).on_hover_text("How big the rooms and halls are: Large and Extra large make every room wider and taller (and the passages between them longer)");
        let opts: Vec<(presets::RoomSize, &str)> = presets::RoomSize::ALL.iter().map(|r| (*r, r.name())).collect();
        segmented(ui, v, &opts);
    });
}

/// Irregular rooms 0..100 % (0: off): most rooms laid out with uneven walls, a little or a lot
/// (each room's card turns it on or off, reshapes it, sets its own unevenness).
fn irregular_row(ui: &mut egui::Ui, v: &mut f32) {
    slider_row(ui, "Irregular rooms", v, 0.0..=1.0, |x| if x < 0.01 { "Off".into() } else { format!("{:.0}%", x * 100.0) }).on_hover_text(
        "Most rooms and halls get uneven walls: alcoves and jogs, so they're wider in some places than others (round rooms bulge). A little: barely uneven; all the way: every room its own shape. Each room's card can change its own",
    );
}

fn new_dungeon(t: &DungeonTool, way_in: Option<NewEntrance>) -> NewDungeon {
    NewDungeon {
        name: t.new_name.clone(),
        kind: if t.new_realm {
            MapKind::Realm
        } else {
            MapKind::Interior
        },
        layout: t.new_layout.clone(),
        size: t.new_size,
        room_size: t.new_room_size,
        irregularity: t.new_irregular,
        art: t.new_art,
        style: t.new_style.clone(),
        mood: t.new_mood.clone(),
        instancing: t.new_instancing,
        way_in,
        shape: t.new_realm.then_some(t.new_land.shape),
        realm_size: t.new_realm.then_some(t.new_land.size),
        seed: t.new_realm.then_some(t.new_land.seed),
        compound: (t.new_realm && t.new_layout == super::compound::LAYOUT)
            .then_some(t.new_compound),
        sheet: (t.new_realm && t.new_layout == super::sheets::LAYOUT)
            .then(|| t.new_sheet.clone())
            .flatten(),
    }
}

/// The inspector: the picked dungeon's card (in the world); in a dungeon's map the selected light
/// (Lights), piece or room, or the plan in numbers.
pub fn dungeons_right(ui: &mut egui::Ui, s: &mut UiState) {
    if s.dungeon_map.0.is_some() {
        if s.ed.tab == Tab::DungeonLights && selected_light(s).is_some() {
            light_card(ui, s);
            return;
        }
        if let Some(id) = s.dungeon_tool.selected_piece.filter(|id| {
            s.dungeon_map
                .0
                .as_ref()
                .is_some_and(|m| m.pieces.iter().any(|p| p.id == *id))
        }) {
            piece_card(ui, s, id);
            return;
        }
        room_card(ui, s);
        return;
    }
    let Some(id) = s.dungeons.selected else {
        hint_small(
            ui,
            "Pick a dungeon on the left to see its ways in and open its builder.",
        );
        return;
    };
    dungeon_card(ui, s, id);
}

/// The picked dungeon (world): its name, Open builder, how the game runs it, its ways in, Remove.
fn dungeon_card(ui: &mut egui::Ui, s: &mut UiState, id: u32) {
    let Some(d) = s.dungeons.get(id).cloned() else {
        return;
    };
    let lib = s.lib.as_deref();
    let t = &mut *s.dungeon_tool;
    // name (kept when typing is done)
    let buf = match &mut t.name_edit {
        Some((i, b)) if *i == id => b,
        _ => {
            t.name_edit = Some((id, d.name.clone()));
            &mut t.name_edit.as_mut().unwrap().1
        }
    };
    let r = field(ui, "Name", buf, "");
    if r.lost_focus() && buf.trim() != d.name && !buf.trim().is_empty() {
        let name = buf.trim().to_string();
        t.requests.push(Request::Def(
            id,
            "Rename dungeon".into(),
            Box::new(move |d| d.name = name),
        ));
    }
    let kind = if d.kind == MapKind::Realm {
        presets::realm(&d.layout).map(|r| r.name).unwrap_or("Realm")
    } else {
        presets::get(&d.layout).map(|p| p.name).unwrap_or("Dungeon")
    };
    hint_small(
        ui,
        &format!(
            "{} · {}",
            kind,
            if d.kind == MapKind::Realm {
                "a world of its own under a sky"
            } else {
                "underground"
            }
        ),
    );
    if let Some(sh) = d.shape.filter(|_| d.kind == MapKind::Realm) {
        hint_small(
            ui,
            &format!(
                "{} · {} across · seed {}",
                sh.name(),
                d.realm_size.across(),
                d.seed.unwrap_or(0)
            ),
        );
    }
    if primary_icon_button(ui, "Castle", "Open builder")
        .on_hover_text(
            "Save the world and open this dungeon's own map to build it (Back to world returns)",
        )
        .clicked()
    {
        t.requests.push(Request::Open(id));
    }
    hint_small(
        ui,
        "Saves the world and opens this dungeon's own map. Back to world returns.",
    );
    // how the game runs it (the sliders change it live: one undo step when the mouse lets go)
    let mut live_edit = None;
    section_fold(ui, "dg_runs", "How the game runs it", "", |ui| {
        let mut inst = d.instancing;
        if segmented(
            ui,
            &mut inst,
            &[
                (Instancing::Shared, "Shared"),
                (Instancing::Party, "Party"),
                (Instancing::Raid, "Raid"),
            ],
        ) {
            t.requests.push(Request::Def(
                id,
                "Dungeon instancing".into(),
                Box::new(move |d| d.instancing = inst),
            ));
        }
        let (mut lo, mut hi, mut players) =
            (d.levels[0] as f32, d.levels[1] as f32, d.players as f32);
        let a = slider_row(ui, "Levels from", &mut lo, 1.0..=80.0, |v| {
            format!("{v:.0}")
        })
        .changed();
        let b = slider_row(ui, "Levels to", &mut hi, 1.0..=80.0, |v| format!("{v:.0}")).changed();
        let c = slider_row(ui, "Players", &mut players, 1.0..=40.0, |v| {
            format!("{v:.0}")
        })
        .changed();
        if a || b || c {
            live_edit = Some((
                lo.round() as u8,
                hi.round().max(lo) as u8,
                players.round() as u8,
            ));
        }
    });
    if let Some((lo, hi, p)) = live_edit {
        if t.live_list.is_none() {
            t.live_list = Some(("Dungeon levels".into(), Arc::new((*s.dungeons).clone())));
        }
        if let Some(x) = s.dungeons.get_mut(id) {
            x.levels = [lo, hi];
            x.players = p;
        }
        s.meta.dirty = true;
        return;
    }
    // its ways in
    section_fold(
        ui,
        "dg_ways",
        "Ways in",
        &d.entrances.len().to_string(),
        |ui| {
            if d.entrances.is_empty() {
                hint_small(ui, "No way in yet: add one below.");
            }
            for e in &d.entrances {
                ui.horizontal(|ui| {
                    icon_label(ui, way_icon(e.kind), 15.0, TEXT_DIM);
                    let what = if e.label.is_empty() {
                        e.kind.name().to_string()
                    } else {
                        e.label.clone()
                    };
                    ui.label(
                        RichText::new(what)
                            .font(FontId::new(13.0, medium()))
                            .color(TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if icon_button(ui, "TrashCan", "Remove this way in").clicked() {
                            t.requests.push(Request::RemoveEntrance(id, e.id));
                        }
                        if icon_button(ui, "ArrowUpRight", "Go to it").clicked() {
                            t.requests
                                .push(Request::GoTo(Vec3::from_array(e.pos), e.yaw));
                        }
                    });
                });
                if matches!(e.kind, EntranceKind::Portal | EntranceKind::Cave) {
                    let mut c = e.color;
                    if portal_colors(ui, &mut c) {
                        t.requests.push(Request::Entrance(
                            id,
                            e.id,
                            "Portal colour".into(),
                            Box::new(move |e| e.color = c),
                        ));
                    }
                }
                if e.kind == EntranceKind::Portal {
                    let cur = e
                        .object
                        .and_then(|o| s.store.items.get(&o))
                        .map(|p| p.asset.clone())
                        .unwrap_or_default();
                    if let Some(f) =
                        frame_dropdown(ui, &format!("dg_frame_{}_{}", id, e.id), lib, &cur)
                        && f != cur
                    {
                        t.requests.push(Request::SetFrame(id, e.id, f));
                    }
                }
                ui.add_space(2.0);
            }
            sub_head(ui, "Add a way in");
            ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for k in EntranceKind::ALL {
                if small_icon_button(ui, way_icon(k), k.name()).on_hover_text(k.hint()).clicked() {
                    t.new_prop = None;
                    t.pick = Some(Pick::Add(id, k));
                }
            }
            for (a, name) in entrance_props(lib) {
                if small_icon_button(ui, "Door", &name).on_hover_text("Click the ground next: it stands there and leads into the dungeon (no portal)").clicked() {
                    t.new_prop = Some(a);
                    t.pick = Some(Pick::Add(id, EntranceKind::Object));
                }
            }
        });
        },
    );
    ui.add_space(2.0);
    if t.confirm_remove == Some(id) {
        ui.label(RichText::new("Remove this dungeon and its portals? (Its map is kept aside in the world's folder.)").size(12.5).color(DANGER));
        let r = btn_row(
            ui,
            &[("TrashCan", "Remove", Btn::Danger), ("", "Keep", Btn::Soft)],
        );
        if r[0].clicked() {
            t.requests.push(Request::Remove(id));
            t.confirm_remove = None;
        }
        if r[1].clicked() {
            t.confirm_remove = None;
        }
    } else if button_of(ui, "TrashCan", "Remove dungeon", Btn::Bad).clicked() {
        t.confirm_remove = Some(id);
    }
}

// ------------------------------------------------------------------ the builder

/// Applies a change to the open map now (the view follows at once) and records one undo step
/// when the mouse lets go.
fn live(s: &mut UiState, label: &str, f: impl FnOnce(&mut MapDoc)) {
    let Some(m) = s.dungeon_map.0.as_mut() else {
        return;
    };
    if s.dungeon_tool.live.is_none() {
        s.dungeon_tool.live = Some((label.to_string(), Arc::new(m.clone())));
    }
    f(m);
    s.meta.dirty = true;
}

/// Records a change as one step.
fn change(s: &mut UiState, label: &str, f: impl FnOnce(&mut MapDoc) + Send + Sync + 'static) {
    s.dungeon_tool
        .requests
        .push(Request::Map(label.to_string(), Box::new(f)));
}

/// Layout › Draw, in the prototype's order (the room shapes, then Window and Alcove, then Erase).
const DRAW_TILES: [Draw; 19] = [
    Draw::Select,
    Draw::Room,
    Draw::Round,
    Draw::Octagon,
    Draw::Hall,
    Draw::Corridor,
    Draw::Cavern,
    Draw::Stairs,
    Draw::Pit,
    Draw::Pool,
    Draw::Dais,
    Draw::Bridge,
    Draw::Gallery,
    Draw::Balcony,
    Draw::Chasm,
    Draw::Spiral,
    Draw::Window,
    Draw::Alcove,
    Draw::Erase,
];

/// Layout › On the walls.
const WALL_TILES: [Draw; 4] = [
    Draw::WallDoor,
    Draw::WallSecret,
    Draw::WallTorch,
    Draw::WallLantern,
];

fn draw_grid(ui: &mut egui::Ui, t: &mut DungeonTool, tiles: &[Draw]) {
    let items: Vec<(&str, &str, &str)> = tiles
        .iter()
        .map(|d| (d.icon(), d.name(), d.hint()))
        .collect();
    let sel = tiles.iter().position(|d| *d == t.draw);
    if let Some(i) = tool_grid(ui, 4, &items, sel) {
        t.draw = tiles[i];
        t.path.clear();
        t.drag_from = None;
        t.placing_light = None;
    }
}

pub fn layout_left(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    if m.kind == MapKind::Realm {
        // (a realm made from a design sheet: the sheet, its build and its markers)
        crate::dungeons::sheets::ui::design_section(ui, s);
        dim(
            ui,
            "A realm: a world of its own under a sky. Build it with the world's tools (Terrain, Water, Nature, Build…); players arrive at the player start (Gameplay).",
        );
        hint_small(
            ui,
            "Players arrive at the player start; the way back out stands behind it.",
        );
        way_out_section(ui, s, true);
        if primary_icon_button(ui, "ArrowLeft", "Back to world")
            .on_hover_text("Save this map and return to the world")
            .clicked()
        {
            s.dungeon_tool.requests.push(Request::Back);
        }
        return;
    }
    let seed = m.plan.seed;
    crate::ui::tips::card(
        ui,
        "dungeon_builder",
        "Building a dungeon",
        &[
            "Pick a layout and press Lay out (Shuffle re-rolls it; lock rooms you like in their card first).",
            "Or draw: pick Room, Hall, Corridor, Cavern, Stairs… and drag or click on the floor. The view is cut away above the level you edit.",
            "Look gives it a style and a mood; Lights hangs torches and braziers for you (or place your own).",
            "Walk in to explore it on foot. Back to world saves it and returns.",
        ],
    );
    let t = &mut *s.dungeon_tool;
    // one-click layouts
    section_fold(ui, "dl_layout", "One-click layout", "", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let shown = presets::get(&t.layout).map(|p| p.name).unwrap_or("Layout");
            let w = (ui.available_width() - 6.0 - 100.0).max(80.0);
            egui::ComboBox::from_id_salt("builder_layout")
                .width(w)
                .height(420.0)
                .selected_text(shown)
                .dropdown_ui(ui, |ui| {
                    for p in presets::all() {
                        if menu_check(ui, t.layout == p.id, p.name)
                            .on_hover_text(p.blurb)
                            .clicked()
                        {
                            t.layout = p.id.into();
                        }
                    }
                });
            segmented(
                ui,
                &mut t.size,
                &[(Size::Small, "S"), (Size::Medium, "M"), (Size::Large, "L")],
            );
        });
        if let Some(p) = presets::get(&t.layout) {
            hint_small(ui, p.blurb);
        }
        room_size_row(ui, &mut t.room_size);
        irregular_row(ui, &mut t.irregular);
        let r = btn_row(
            ui,
            &[
                ("MagicWand", "Lay out", Btn::Soft),
                ("Shuffle", "Shuffle", Btn::Soft),
            ],
        );
        if r[0]
            .clone()
            .on_hover_text(
                "Lay the whole dungeon out from this layout (locked rooms stay; one undo step)",
            )
            .clicked()
        {
            t.requests.push(Request::Layout {
                layout: t.layout.clone(),
                size: t.size,
                room_size: t.room_size,
                irregular: t.irregular,
                seed,
            });
        }
        if r[1]
            .clone()
            .on_hover_text("The same layout, laid out differently (locked rooms stay)")
            .clicked()
        {
            t.requests.push(Request::Layout {
                layout: t.layout.clone(),
                size: t.size,
                room_size: t.room_size,
                irregular: t.irregular,
                seed: seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223) & 0x7fff_ffff,
            });
        }
    });
    // drawing
    section_fold(ui, "dl_draw", "Draw", "", |ui| {
        draw_grid(ui, t, &DRAW_TILES);
        if !WALL_TILES.contains(&t.draw) {
            hint_small(ui, t.draw.hint());
        }
    });
    section_fold(ui, "dl_walls", "On the walls", "", |ui| {
        draw_grid(ui, t, &WALL_TILES);
        if WALL_TILES.contains(&t.draw) {
            hint_small(ui, t.draw.hint());
        }
        lab_row(ui, "Snap", |ui| {
            segmented(
                ui,
                &mut t.snap,
                &[(0.0, "Off"), (0.5, "0.5 m"), (1.0, "1 m"), (2.0, "2 m")],
            )
        });
    });
    pieces_section(ui, s);
    way_out_section(ui, s, false);
    levels_section(ui, s);
}

/// The way back out: a portal like the way in players came through (its frame and colour) unless
/// changed here; where it stands (dragged in the view, turned here, or back behind the arrival).
fn way_out_section(ui: &mut egui::Ui, s: &mut UiState, open: bool) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    let realm = m.kind == MapKind::Realm;
    let cur = super::portal::exit_frame(m);
    let mut color = super::portal::exit_color(m);
    let own = m.way_out.frame.is_some() || m.way_out.color.is_some();
    let put = m.way_out.at.is_some();
    let at = super::portal::exit_at(m, &s.store, s.gameplay.player_start.as_ref());
    let lib = s.lib.as_deref();
    let t = &mut *s.dungeon_tool;
    let body = |ui: &mut egui::Ui| {
        hint_small(
            ui,
            if realm {
                "The portal that takes players back out. It looks like the way in they came through until you change it here; move its frame like any object (Build › Select)."
            } else {
                "The portal that takes players back out. It looks like the way in they came through until you change it here. Drag it on the floor (Select) to move it."
            },
        );
        if portal_colors(ui, &mut color) {
            t.requests.push(Request::WayOut(
                "Way out colour".into(),
                Box::new(move |w| w.color = Some(color)),
            ));
        }
        if let Some(f) = frame_dropdown(ui, "way_out_frame", lib, &cur)
            && f != cur
        {
            t.requests.push(Request::WayOut(
                "Way out style".into(),
                Box::new(move |w| w.frame = Some(f)),
            ));
        }
        if let Some((p, yaw)) = at {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Turn it").color(TEXT_DIM));
                for (ic, turn, tip) in [
                    ("ArrowRotateCounterClockwise", 15f32, "Turn it left 15°"),
                    ("ArrowRotateClockwise", -15.0, "Turn it right 15°"),
                ] {
                    if icon_button(ui, ic, tip).clicked() {
                        t.requests.push(Request::WayOut(
                            "Turn the way out".into(),
                            Box::new(move |w| {
                                w.at = Some(p.to_array());
                                w.yaw = yaw + turn.to_radians();
                            }),
                        ));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if small_icon_button(ui, "Target", "Go to it").clicked() {
                        t.requests.push(Request::GoTo(p, yaw));
                    }
                });
            });
        }
        let r = ui
            .add_enabled_ui(own, |ui| {
                wide_icon_button(ui, "SquareBehindSquare2", "Match the way in")
            })
            .inner;
        if r.on_hover_text("The frame and colour of the way in players came through")
            .clicked()
        {
            t.requests.push(Request::WayOut(
                "Way out matches the way in".into(),
                Box::new(|w| {
                    w.frame = None;
                    w.color = None;
                }),
            ));
        }
        let r = ui
            .add_enabled_ui(put, |ui| {
                wide_icon_button(ui, "ArrowUndoUp", "Behind the arrival")
            })
            .inner;
        if r.on_hover_text("Stand it behind where players arrive again")
            .clicked()
        {
            t.requests.push(Request::WayOut(
                "Way out behind the arrival".into(),
                Box::new(|w| {
                    w.at = None;
                    w.yaw = 0.0;
                }),
            ));
        }
    };
    if open {
        section_fold(ui, "dl_way_out_realm", "Way out", "", body);
    } else {
        fold_closed(ui, "dl_way_out", "Way out", "", body);
    }
}

/// Doors, gates, bars, secret doors, levers, plates, traps and markers: pick one, click the map.
fn pieces_section(ui: &mut egui::Ui, s: &mut UiState) {
    use super::pieces::PieceKind;
    let n = s
        .dungeon_map
        .0
        .as_ref()
        .map(|m| m.pieces.len())
        .unwrap_or(0);
    let t = &mut s.dungeon_tool;
    fold_closed(
        ui,
        "dl_pieces",
        "Doors, levers and traps",
        &n.to_string(),
        |ui| {
            hint_small(
                ui,
                "Pick one and click: doors, gates and bars go into the nearest doorway, levers on a wall, plates, traps and markers on the floor. Click one with Select to change it (or link a lever to what it works).",
            );
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for k in PieceKind::ALL {
                    let on = t.placing_piece == Some(k);
                    if chip(ui, k.name(), None, on).clicked() {
                        t.placing_piece = if on { None } else { Some(k) };
                        t.draw = Draw::Select;
                    }
                }
            });
            hint_small(
                ui,
                &format!(
                    "{n} pieces (the template layouts put doors and gates in some doorways: change or remove any)"
                ),
            );
        },
    );
}

/// The selected piece (inspector): its kind, size, state, what it works, its label; remove.
fn piece_card(ui: &mut egui::Ui, s: &mut UiState, id: u32) {
    use super::pieces::PieceKind;
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    let Some(p) = m.pieces.iter().find(|p| p.id == id).cloned() else {
        return;
    };
    // (what a lever or plate could work)
    let others: Vec<(u32, String)> = m
        .pieces
        .iter()
        .filter(|x| {
            x.id != id
                && !x.kind.is_marker()
                && !matches!(x.kind, PieceKind::Lever | PieceKind::PressurePlate)
        })
        .map(|x| (x.id, format!("{} #{}", x.kind.name(), x.id)))
        .collect();
    if p.auto {
        hint_small(
            ui,
            "Placed by the layout (laying out again replaces it; change it and it's yours).",
        );
    }
    let mut kind = p.kind;
    dropdown_row(ui, "Kind", ("piece_kind", id), kind.name(), |ui| {
        for k in PieceKind::ALL {
            menu_option(ui, &mut kind, k, k.name());
        }
    });
    if kind != p.kind {
        change(s, "Piece kind", move |m| {
            if let Some(x) = m.pieces.iter_mut().find(|x| x.id == id) {
                x.kind = kind;
                x.auto = false;
            }
        });
    }
    let (mut w, mut h) = (p.width, p.height);
    let mut changed = slider_row(ui, "Width", &mut w, 0.3..=16.0, m1).changed();
    if !p.kind.is_marker() {
        changed |= slider_row(ui, "Height", &mut h, 0.05..=8.0, m1).changed();
    }
    let mut yaw = p.yaw.to_degrees();
    changed |= slider_row(ui, "Turn", &mut yaw, -180.0..=180.0, |v| format!("{v:.0}°")).changed();
    if changed {
        live(s, "Piece size", move |m| {
            if let Some(x) = m.pieces.iter_mut().find(|x| x.id == id) {
                x.width = w;
                x.height = h;
                x.yaw = yaw.to_radians();
                x.auto = false;
            }
        });
    }
    if !p.kind.is_marker() {
        let mut open = p.open;
        let what = match p.kind {
            PieceKind::Lever => "Starts pulled",
            PieceKind::SpikeTrap => "Spikes up",
            PieceKind::Bars => "(bars don't open)",
            _ => "Starts open",
        };
        if p.kind != PieceKind::Bars && toggle(ui, &mut open, what).changed() {
            change(s, "Piece state", move |m| {
                if let Some(x) = m.pieces.iter_mut().find(|x| x.id == id) {
                    x.open = open;
                    x.auto = false;
                }
            });
        }
    }
    // what a lever or a plate works
    if matches!(p.kind, PieceKind::Lever | PieceKind::PressurePlate) {
        sub_head(ui, "Works");
        if others.is_empty() {
            hint_small(ui, "Place a door, gate, secret door or trap to link it to.");
        }
        for (oid, name) in others {
            let mut on = p.links.contains(&oid);
            if toggle(ui, &mut on, name).changed() {
                change(s, "Link", move |m| {
                    if let Some(x) = m.pieces.iter_mut().find(|x| x.id == id) {
                        if on {
                            x.links.push(oid);
                        } else {
                            x.links.retain(|l| *l != oid);
                        }
                        x.auto = false;
                    }
                });
            }
        }
    }
    let mut label = p.label.clone();
    if field(
        ui,
        "Label",
        &mut label,
        "for the game (a key's name, an event)",
    )
    .changed()
    {
        live(s, "Piece label", move |m| {
            if let Some(x) = m.pieces.iter_mut().find(|x| x.id == id) {
                x.label = label.clone();
            }
        });
    }
    if button_of(ui, "TrashCan", "Remove", Btn::Bad).clicked() {
        change(s, "Remove piece", move |m| {
            m.pieces.retain(|x| x.id != id);
            for x in &mut m.pieces {
                x.links.retain(|l| *l != id);
            }
        });
        s.dungeon_tool.selected_piece = None;
    }
}

fn levels_section(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    let mut levels: Vec<(i32, String, f32, usize)> = m
        .plan
        .levels
        .iter()
        .map(|l| {
            (
                l.id,
                l.name.clone(),
                l.y,
                m.plan.rooms.iter().filter(|r| r.level == l.id).count(),
            )
        })
        .collect();
    levels.sort_by_key(|l| -l.0);
    let lo = levels.iter().map(|l| l.0).min().unwrap_or(0);
    let hi = levels.iter().map(|l| l.0).max().unwrap_or(0);
    let mut add = None;
    let mut reqs = vec![];
    let v = &mut *s.dungeon_view;
    section_fold(ui, "dl_levels", "Levels", &levels.len().to_string(), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for (id, name, y, n) in &levels {
            let sel = v.level == *id;
            let title = if name.is_empty() {
                format!("Level {id}")
            } else {
                name.clone()
            };
            let sub = format!("{:+.0} m · {} room{}", y, n, if *n == 1 { "" } else { "s" });
            if list_row(
                ui,
                Lead::Icon(if sel { "EyeOpen" } else { "LayersTwo" }),
                &title,
                &sub,
                "",
                sel,
            )
            .on_hover_text("Edit this level (the view is cut away above it)")
            .clicked()
            {
                v.level = *id;
            }
        }
        ui.spacing_mut().item_spacing.y = 6.0;
        let r = btn_row(
            ui,
            &[
                ("ArrowDown", "Level below", Btn::Soft),
                ("ArrowUp", "Level above", Btn::Soft),
            ],
        );
        if r[0].clicked() {
            add = Some(lo - 1);
        }
        if r[1].clicked() {
            add = Some(hi + 1);
        }
        toggle(
            ui,
            &mut v.cut,
            "Cut away above the level (see into the rooms)",
        )
        .on_hover_text(
            "Off: ceilings and the levels above show (also the Ceilings button over the view)",
        );
        toggle(ui, &mut v.show_lights, "Show lights and outlines");
        toggle(
            ui,
            &mut v.work_light,
            "Work light (off: lit as in the game)",
        )
        .on_hover_text(
            "A readable light everywhere while building; walking always shows the real lighting",
        );
        let r = btn_row(
            ui,
            &[
                ("Camera1", "Top view", Btn::Soft),
                ("Footsteps", "Walk in", Btn::Soft),
            ],
        );
        if r[0]
            .clone()
            .on_hover_text("Look straight down on the level")
            .clicked()
        {
            reqs.push(Request::TopView);
        }
        if r[1]
            .clone()
            .on_hover_text("Explore it on foot from where players arrive")
            .clicked()
        {
            reqs.push(Request::WalkIn);
        }
        if wide_icon_button(ui, "Map", "Minimaps and walk check").on_hover_text("Draws a map of each level (saved with the dungeon as minimap_L<level>.png; the export carries them) and checks every room can be walked to from where players arrive").clicked() {
            reqs.push(Request::Survey);
        }
    });
    if let Some(id) = add {
        change(s, "Add level", move |m| m.plan.ensure_level(id));
        s.dungeon_view.level = id;
    }
    s.dungeon_tool.requests.extend(reqs);
}

/// The picked room's card (the builder's inspector), or the plan in numbers when none is picked.
pub fn room_card(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    if m.kind == MapKind::Realm {
        hint_small(
            ui,
            "Players arrive at the player start; the way back out stands behind it.",
        );
        return;
    }
    let Some(id) = s
        .dungeon_tool
        .selected_room
        .filter(|id| m.plan.room(*id).is_some())
    else {
        let tris = s.dungeon_world.triangles;
        stat_row(ui, "Rooms", &m.plan.rooms.len().to_string());
        stat_row(ui, "Levels", &m.plan.levels.len().to_string());
        stat_row(
            ui,
            "Triangles",
            &format!(
                "{}{}",
                tris,
                if s.dungeon_world.busy() {
                    " · building…"
                } else {
                    ""
                }
            ),
        );
        hint_small(
            ui,
            "Click a room with Select to change it: its size, height, ceiling, pillars, what it's for, its own style.",
        );
        return;
    };
    let r = m.plan.room(id).unwrap().clone();
    let level_y = m.plan.level_y(r.level);
    let art = m.art;
    hint_small(
        ui,
        &format!(
            "{} on level {} · {}",
            r.kind.name(),
            r.level,
            r.shape.kind_name()
        ),
    );
    // name (kept when typing is done)
    let t = &mut *s.dungeon_tool;
    if t.room_name.as_ref().is_none_or(|(i, _)| *i != id) {
        t.room_name = Some((id, r.name.clone()));
    }
    let buf = &mut t.room_name.as_mut().unwrap().1;
    let resp = field(ui, "Name", buf, r.kind.name());
    if resp.lost_focus() && *buf != r.name {
        let name = buf.clone();
        change(s, "Rename room", move |m| {
            if let Some(x) = m.plan.room_mut(id) {
                x.name = name;
            }
        });
    }
    // kind
    let mut kind = r.kind;
    dropdown_row(ui, "Kind", ("room_kind", id), kind.name(), |ui| {
        for k in RoomKind::ALL {
            menu_option(ui, &mut kind, k, k.name());
        }
    });
    if kind != r.kind {
        change(s, "Room kind", move |m| {
            if let Some(x) = m.plan.room_mut(id) {
                x.kind = kind;
                x.height = kind.default_height();
                if kind == RoomKind::Cavern {
                    x.ceiling = Ceiling::Rock;
                }
                if kind == RoomKind::Stairs && x.to_level.is_none() {
                    x.to_level = Some(x.level - 1);
                }
                if kind == RoomKind::Spiral && x.to_level.is_none() {
                    x.to_level = Some(x.level + 1);
                }
            }
        });
    }
    // size (of the regular shape an irregular room was made from)
    let mut shape = r
        .irregular
        .as_ref()
        .map_or_else(|| r.shape.clone(), |i| i.base.clone());
    let mut resized = false;
    match &mut shape {
        Shape::Rect { half, yaw, .. } => {
            let (mut w, mut d, mut a) = (half[0] * 2.0, half[1] * 2.0, yaw.to_degrees());
            resized |= slider_row(ui, "Width", &mut w, 2.0..=120.0, m1).changed();
            resized |= slider_row(ui, "Length", &mut d, 2.0..=160.0, m1).changed();
            resized |=
                slider_row(ui, "Turn", &mut a, -180.0..=180.0, |v| format!("{v:.0}°")).changed();
            *half = [w * 0.5, d * 0.5];
            *yaw = a.to_radians();
        }
        Shape::Round { r: rad, .. } | Shape::Ngon { r: rad, .. } => {
            let mut d = *rad * 2.0;
            resized |= slider_row(ui, "Across", &mut d, 2.0..=200.0, m1).changed();
            *rad = d * 0.5;
        }
        Shape::Path { width, .. } => {
            resized |= slider_row(ui, "Width", width, 1.5..=24.0, m1).changed();
        }
        Shape::Poly { .. } => {}
    }
    if resized {
        live(s, "Resize room", move |m| {
            if let Some(x) = m.plan.room_mut(id) {
                x.set_shape(shape);
            }
        });
    }
    // uneven walls
    if r.can_be_irregular() {
        let mut on = r.irregular.is_some();
        ui.horizontal(|ui| {
            if toggle(ui, &mut on, "Irregular").on_hover_text("Uneven walls: alcoves and jogs, so it's wider in some places than others (a round room bulges)").changed() {
                let seed = fastrand::u32(..);
                change(s, if on { "Irregular room" } else { "Regular room" }, move |m| {
                    if let Some(x) = m.plan.room_mut(id) {
                        if on { x.set_irregular(seed, 0.6) } else { x.set_regular() }
                    }
                });
            }
            if let Some(ir) = r.irregular.clone() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if small_icon_button(ui, "Dice5", "Reshape").on_hover_text("Shuffle its shape: a new uneven outline").clicked() {
                        let seed = fastrand::u32(..);
                        change(s, "Reshape room", move |m| {
                            if let Some(x) = m.plan.room_mut(id) {
                                x.set_irregular(seed, ir.amount);
                            }
                        });
                    }
                });
            }
        });
        if let Some(ir) = r.irregular.clone() {
            let mut a = ir.amount;
            if slider_row(ui, "Unevenness", &mut a, 0.1..=1.0, pct).changed() {
                live(s, "Room unevenness", move |m| {
                    if let Some(x) = m.plan.room_mut(id) {
                        x.set_irregular(ir.seed, a);
                    }
                });
            }
        }
    }
    let mut h = r.height;
    let label = match r.kind {
        RoomKind::Dais => "Rise",
        RoomKind::Pit | RoomKind::Chasm => "Opening height",
        _ => "Height",
    };
    let max = if r.kind == RoomKind::Dais { 4.0 } else { 60.0 };
    // (bridges and balconies have no height: their deck is all there is)
    if !matches!(r.kind, RoomKind::Bridge | RoomKind::Balcony)
        && slider_row(ui, label, &mut h, 0.3..=max, m1).changed()
    {
        live(s, "Room height", move |m| {
            if let Some(x) = m.plan.room_mut(id) {
                x.height = h;
            }
        });
    }
    if matches!(
        r.kind,
        RoomKind::Room
            | RoomKind::Hall
            | RoomKind::Corridor
            | RoomKind::Pool
            | RoomKind::Bridge
            | RoomKind::Balcony
    ) {
        let mut f = r.floor;
        let range = if r.kind == RoomKind::Balcony {
            -14.0..=24.0
        } else {
            -8.0..=8.0
        };
        if slider_row(
            ui,
            if r.kind == RoomKind::Pool {
                "Depth"
            } else {
                "Floor"
            },
            &mut f,
            range,
            |v| format!("{v:+.1} m"),
        )
        .changed()
        {
            live(s, "Room floor", move |m| {
                if let Some(x) = m.plan.room_mut(id) {
                    x.floor = f;
                }
            });
        }
    }
    if r.kind.is_pit() {
        let mut d = r.depth;
        if slider_row(ui, "Depth", &mut d, 2.0..=120.0, |v| format!("{v:.0} m")).changed() {
            live(s, "Pit depth", move |m| {
                if let Some(x) = m.plan.room_mut(id) {
                    x.depth = d;
                }
            });
        }
    }
    if r.kind.is_pit() || r.kind == RoomKind::Pool {
        let mut l = r.liquid;
        dropdown_row(ui, "Fill", ("room_fill", id), l.name(), |ui| {
            for k in Liquid::ALL {
                menu_option(ui, &mut l, k, k.name());
            }
        });
        if l != r.liquid {
            change(s, "Liquid", move |m| {
                if let Some(x) = m.plan.room_mut(id) {
                    x.liquid = l;
                }
            });
        }
    }
    if r.kind == RoomKind::Stairs {
        let mut smooth = r.smooth;
        if toggle(ui, &mut smooth, "A smooth ramp (no steps)").changed() {
            change(s, "Ramp", move |m| {
                if let Some(x) = m.plan.room_mut(id) {
                    x.smooth = smooth;
                }
            });
        }
    }
    if matches!(r.kind, RoomKind::Stairs | RoomKind::Spiral) {
        let mut to = r.to_level.unwrap_or(r.level - 1);
        stepper(ui, "Leads to level", &mut to, -20..=20, r.level);
        if Some(to) != r.to_level && to != r.level {
            change(s, "Stairs", move |m| {
                if let Some(x) = m.plan.room_mut(id) {
                    x.to_level = Some(to);
                }
                m.plan.ensure_level(to);
            });
        }
    }
    if r.kind.is_space() && !r.kind.is_pit() && r.kind != RoomKind::Pool {
        let mut c = r.ceiling;
        dropdown_row(ui, "Ceiling", ("room_ceiling", id), c.name(), |ui| {
            for k in Ceiling::ALL {
                menu_option(ui, &mut c, k, k.name());
            }
        });
        if c != r.ceiling {
            change(s, "Ceiling", move |m| {
                if let Some(x) = m.plan.room_mut(id) {
                    x.ceiling = c;
                }
            });
        }
        let mut p = r.pillars;
        dropdown_row(ui, "Pillars", ("room_pillars", id), p.name(), |ui| {
            for k in Pillars::ALL {
                menu_option(ui, &mut p, k, k.name());
            }
        });
        if p != r.pillars {
            change(s, "Pillars", move |m| {
                if let Some(x) = m.plan.room_mut(id) {
                    x.pillars = p;
                }
            });
        }
        let mut role = r.role;
        dropdown_row(ui, "For", ("room_role", id), role.name(), |ui| {
            for k in Role::ALL {
                menu_option(ui, &mut role, k, k.name());
            }
        });
        if role != r.role {
            change(s, "Room role", move |m| {
                if let Some(x) = m.plan.room_mut(id) {
                    x.role = role;
                }
            });
        }
    }
    // its own style
    let own = r.style.clone();
    let shown = own
        .as_deref()
        .map(|s| styles::get(s).name)
        .unwrap_or("The dungeon's");
    let mut pick: Option<Option<String>> = None;
    dropdown_row(ui, "Style", ("room_style", id), shown, |ui| {
        if menu_check(ui, own.is_none(), "The dungeon's").clicked() {
            pick = Some(None);
        }
        for st in styles::of(art) {
            let c = Color32::from_rgb(st.swatch[0], st.swatch[1], st.swatch[2]);
            if swatch_item(ui, c, st.name, own.as_deref() == Some(st.id)).clicked() {
                pick = Some(Some(st.id.to_string()));
            }
        }
    });
    if let Some(p) = pick {
        change(s, "Room style", move |m| {
            if let Some(x) = m.plan.room_mut(id) {
                x.style = p;
            }
        });
    }
    let mut locked = r.locked;
    if toggle(ui, &mut locked, "Keep on Shuffle")
        .on_hover_text("Locked: Lay out and Shuffle keep this room as it is")
        .changed()
    {
        change(s, "Lock room", move |m| {
            if let Some(x) = m.plan.room_mut(id) {
                x.locked = locked;
            }
        });
    }
    let b = btn_row(
        ui,
        &[
            ("Target", "Go to", Btn::Soft),
            ("TrashCan", "Remove", Btn::Bad),
        ],
    );
    if b[0].clicked() {
        let c = r.shape.centre();
        let y = level_y + r.floor;
        s.dungeon_tool
            .requests
            .push(Request::GoTo(Vec3::new(c.x, y, c.y), 0.0));
    }
    if b[1].clicked() {
        s.dungeon_tool.requests.push(Request::Map(
            "Remove room".into(),
            Box::new(move |m| {
                m.plan.remove(id);
            }),
        ));
        s.dungeon_tool.selected_room = None;
    }
}

pub fn look_left(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    let cur = m.style.clone();
    let art = m.art;
    let weathering = m.weathering;
    let n_decals = m.decals.len();
    // the style: a dropdown with each style's swatch
    let mut pick = None;
    section_fold(
        ui,
        "dk_style",
        "Style",
        &format!("{} styles", art.name()),
        |ui| {
            let st = styles::get(&cur);
            egui::ComboBox::from_id_salt("dungeon_style")
                .width(ui.available_width())
                .height(420.0)
                .selected_text(st.name)
                .dropdown_ui(ui, |ui| {
                    for st in styles::of(art) {
                        let c = Color32::from_rgb(st.swatch[0], st.swatch[1], st.swatch[2]);
                        if swatch_item(ui, c, st.name, st.id == cur)
                            .on_hover_text(format!(
                                "{} ({})",
                                st.blurb,
                                if st.pack == "handpainted" {
                                    "hand-painted"
                                } else {
                                    "photoreal"
                                }
                            ))
                            .clicked()
                        {
                            pick = Some(st.id.to_string());
                        }
                    }
                });
            hint_small(
                ui,
                "What the walls, floors, ceilings, trims and pillars are made of. A room can have its own (its card).",
            );
        },
    );
    if let Some(st) = pick {
        change(s, "Dungeon style", move |m| {
            m.style = st.clone();
            // ceilings follow the style
            let style = styles::get(&st);
            for r in &mut m.plan.rooms {
                if r.style.is_none() && r.kind != RoomKind::Cavern && r.ceiling != Ceiling::Open {
                    r.ceiling = style.ceiling_for(r.kind);
                }
            }
        });
    }
    // weathering, and Age the walls
    let mut v = weathering;
    let mut changed = false;
    let mut req = None;
    let t = &mut s.dungeon_tool;
    section_fold(ui, "dk_weather", "Weathering", "", |ui| {
        hint_small(
            ui,
            "How time has worn the stone, on top of its style's own: grime at the foot of the walls and in corners, moss where it's damp, wet stone by the water, puddles on the floors.",
        );
        changed |= slider_row(ui, "Moss", &mut v.moss, 0.0..=2.5, pct).changed();
        changed |= slider_row(ui, "Grime", &mut v.grime, 0.0..=2.0, pct).changed();
        changed |= slider_row(ui, "Wet", &mut v.wet, 0.0..=2.5, pct).changed();
        changed |= slider_row(ui, "Puddles", &mut v.puddles, 0.0..=2.5, pct).changed();
        changed |= slider_row(ui, "Rubble", &mut v.rubble, 0.0..=3.0, pct).changed();
        hint_small(
            ui,
            "Age the walls: cracks, damp drips and stains, soot over the fires, blood where there's been fighting, glowing rune circles in ritual rooms.",
        );
        slider_row(ui, "How much", &mut t.age_density, 0.3..=3.0, pct);
        let r = btn_row(
            ui,
            &[
                ("Brush", "Age the walls", Btn::Soft),
                ("Eraser", "Clear", Btn::Soft),
            ],
        );
        if r[0].clicked() {
            req = Some(Request::AgeWalls {
                density: t.age_density,
            });
        }
        if r[1]
            .clone()
            .on_hover_text("Remove what Age the walls laid (the decals you placed stay)")
            .clicked()
        {
            req = Some(Request::ClearAged);
        }
    });
    // paint your own decals (their size is in the dock)
    fold_closed(ui, "dk_decals", "Decals", &n_decals.to_string(), |ui| {
        hint_small(
            ui,
            "Paint your own: pick one and click the walls or floor (Alt-click removes the nearest).",
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for k in super::decals::DecalKind::ALL {
                let on = t.placing_decal == Some(k);
                if chip(ui, k.name(), None, on)
                    .on_hover_text("Click a wall or floor to put one there (Esc stops)")
                    .clicked()
                {
                    t.placing_decal = if on { None } else { Some(k) };
                }
            }
        });
        hint_small(ui, &format!("{n_decals} decals"));
    });
    if let Some(r) = req {
        t.requests.push(r);
    }
    if changed {
        live(s, "Weathering", move |m| m.weathering = v);
    }
    // dressing
    let mut reqs = vec![];
    section_fold(ui, "dk_dress", "Dressing", "", |ui| {
        hint_small(
            ui,
            "Furnish every room by what it's for (its card's For): crypts, prisons, libraries, forges, treasure rooms, lairs… Ordinary objects: move or delete any, add more from Objects.",
        );
        let r = btn_row(
            ui,
            &[
                ("Armchair", "Dress rooms", Btn::Soft),
                ("Eraser", "Clear", Btn::Soft),
            ],
        );
        if r[0].clicked() {
            reqs.push(Request::Dress);
        }
        if r[1]
            .clone()
            .on_hover_text(
                "Remove what Dress put in (the light fixtures and your own objects stay)",
            )
            .clicked()
        {
            reqs.push(Request::ClearDressing);
        }
    });
    s.dungeon_tool.requests.extend(reqs);
    mood_section(ui, s);
}

fn mood_section(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    let md = m.mood.clone();
    let sky = m.sky;
    let cur = md.preset.clone();
    let mut open = false;
    section_fold(ui, "dk_mood", "Mood", &mood::name(&cur), |_| open = true);
    if !open {
        music_rows(ui, s, false);
        return;
    }
    let mut pick = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for (id, name, blurb) in mood::PRESETS {
            if chip(ui, name, None, cur == id)
                .on_hover_text(blurb)
                .clicked()
            {
                pick = Some(id);
            }
        }
    });
    if let Some(id) = pick
        && let Some(md) = mood::preset(id)
    {
        change(s, "Mood", move |m| m.mood = md);
    }
    let mut fill = md.fill_strength;
    if slider_row(ui, "Fill light", &mut fill, 0.0..=1.5, pct).changed() {
        live(s, "Fill light", move |m| m.mood.fill_strength = fill);
    }
    let mut c = md.fill;
    if color_row(ui, "Fill colour", &mut c).changed() {
        live(s, "Fill colour", move |m| m.mood.fill = c);
    }
    let mut ex = md.exposure;
    if slider_row(ui, "Brightness", &mut ex, -3.0..=3.0, |v| {
        format!("{v:+.1}")
    })
    .changed()
    {
        live(s, "Exposure", move |m| m.mood.exposure = ex);
    }
    let mut fd = md.fog_density;
    if slider_row(ui, "Fog", &mut fd, 0.0..=1.0, pct).changed() {
        live(s, "Fog", move |m| m.mood.fog_density = fd);
    }
    let mut fc = md.fog;
    if color_row(ui, "Fog colour", &mut fc).changed() {
        live(s, "Fog colour", move |m| m.mood.fog = fc);
    }
    let mut hz = md.haze;
    if slider_row(ui, "Haze", &mut hz, 0.0..=1.0, pct).on_hover_text("Light hanging in the air: glows round big fires, beams under light shafts (each light's Haze box)").changed() {
        live(s, "Haze", move |m| m.mood.haze = hz);
    }
    let mut mt = md.motes;
    if slider_row(ui, "Dust in the air", &mut mt, 0.0..=1.0, pct)
        .on_hover_text("Dust drifting round you as you walk (embers in forge and infernal moods)")
        .changed()
    {
        live(s, "Dust", move |m| m.mood.motes = mt);
    }
    // the sky over rooms open to it (Ceiling: Open)
    let mut k = sky;
    dropdown_row(ui, "Over open rooms", "dungeon_sky", sky.name(), |ui| {
        for x in super::model::SkyMode::ALL {
            menu_option(ui, &mut k, x, x.name());
        }
    })
    .on_hover_text("Rooms whose ceiling is Open: darkness overhead, or a sky with the sun or moon shining in (courtyards, ruined halls, a city under the sky)");
    if k != sky {
        change(s, "Sky", move |m| m.sky = k);
    }
    let mut ta = md.tint_amount;
    if slider_row(ui, "Tint the lights", &mut ta, 0.0..=1.0, pct).changed() {
        live(s, "Light tint", move |m| m.mood.tint_amount = ta);
    }
    let mut tc = md.tint;
    if color_row(ui, "Light tint", &mut tc).changed() {
        live(s, "Light tint", move |m| m.mood.tint = tc);
    }
    let mut vol = md.volume;
    if slider_row(ui, "Sound", &mut vol, 0.0..=1.5, pct).changed() {
        live(s, "Ambience volume", move |m| m.mood.volume = vol);
    }
    hint_small(
        ui,
        &format!(
            "Ambience: {}{}",
            md.sound.trim_start_matches("amb_"),
            if md.sound2.is_empty() {
                String::new()
            } else {
                format!(" + {}", md.sound2.trim_start_matches("amb_near_"))
            }
        ),
    );
    music_rows(ui, s, true);
}

/// Look › Mood › Music: a demo track that ships with the app, or the dungeon's own (a sound file
/// copied into its folder). `shown` false: only the file dialog's answer is taken (Mood folded).
fn music_rows(ui: &mut egui::Ui, s: &mut UiState, shown: bool) {
    let Some(mu) = s.dungeon_map.0.as_ref().map(|m| m.music.clone()) else {
        return;
    };
    if let Some(src) = crate::dialogs::take_file("dungeon_music") {
        match s
            .meta
            .path
            .clone()
            .ok_or_else(|| "save the world first".to_string())
            .and_then(|folder| super::sound::import_music(&folder, &src, &mu.file))
        {
            Ok(name) => {
                change(s, "Music", move |m| {
                    m.music.file = name;
                    m.music.demo.clear();
                });
                s.ed.toast(
                    "Music set: it plays as you walk the dungeon (and here with Audio › preview)",
                );
            }
            Err(e) => s.ed.toast(e),
        }
    }
    if !shown {
        return;
    }
    let demo = super::sound::demo_track(&mu.demo);
    let label = if !mu.file.is_empty() {
        format!("Your own: {}", mu.file.trim_start_matches("music_"))
    } else {
        demo.map(|t| t.name.clone())
            .unwrap_or_else(|| "None".into())
    };
    // Some(None): no music; Some(Some(id)): a demo track
    let mut pick: Option<Option<String>> = None;
    let mut own = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new("Music").color(TEXT_DIM)).on_hover_text("Loops while players are in this dungeon, over its ambience: a demo track, or a track of your own (MP3 or Ogg)");
        egui::ComboBox::from_id_salt("dungeon_music").width(ui.available_width()).height(360.0).selected_text(label).dropdown_ui(ui, |ui| {
            if menu_check(ui, mu.file.is_empty() && demo.is_none(), "None").clicked() {
                pick = Some(None);
            }
            for (kind, heading) in [("layout", "For dungeon types"), ("mood", "For moods")] {
                ui.add_space(4.0);
                ui.label(RichText::new(heading).size(11.0).color(TEXT_MUTE));
                for t in super::sound::demo_tracks().iter().filter(|t| t.kind == kind) {
                    if menu_check(ui, mu.file.is_empty() && mu.demo == t.id, &t.name).on_hover_text(&t.description).clicked() {
                        pick = Some(Some(t.id.clone()));
                    }
                }
            }
            ui.add_space(4.0);
            if menu_check(ui, !mu.file.is_empty(), "Your own file…").on_hover_text("An MP3 or Ogg file (copied into the dungeon's folder)").clicked() {
                own = true;
            }
        });
    });
    if own {
        crate::dialogs::pick_file(
            "dungeon_music",
            rfd::AsyncFileDialog::new().add_filter("Music (MP3, Ogg)", &super::sound::MUSIC_EXTS),
        );
    }
    if let Some(p) = pick {
        change(s, "Music", move |m| {
            m.music.file.clear();
            m.music.demo = p.unwrap_or_default();
        });
    }
    if !mu.file.is_empty() || demo.is_some() {
        let mut v = mu.volume;
        if slider_row(ui, "Music volume", &mut v, 0.0..=1.5, pct).changed() {
            live(s, "Music volume", move |m| m.music.volume = v);
        }
        let mut under = 1.0 - mu.hush;
        if slider_row(ui, "Ambience under it", &mut under, 0.0..=1.0, pct)
            .on_hover_text("How loud the mood's ambience stays while the music plays")
            .changed()
        {
            live(s, "Music", move |m| m.music.hush = 1.0 - under);
        }
    }
}

pub fn lights_left(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    crate::ui::tips::card(
        ui,
        "dungeon_lights",
        "Lighting a dungeon",
        &[
            "Auto-light hangs torches along the walls, braziers round big rooms, candles in crypts and chandeliers over halls, in the style's own fixtures.",
            "Every light can be changed: click its marker, then its colour, strength, reach, flicker and direction.",
            "Add your own: pick a kind below, then click a wall, the floor or the ceiling.",
        ],
    );
    let n = m.lights.len();
    let auto_n = m.lights.iter().filter(|l| l.auto).count();
    let selected = selected_light(s).is_some();
    dim(ui, &format!("{n} lights ({auto_n} from Auto-light)"));
    let t = &mut *s.dungeon_tool;
    slider_row(ui, "How many", &mut t.light_density, 0.4..=2.5, pct);
    if primary_icon_button(ui, "MagicWand", "Auto-light").on_hover_text("Light every room by its kind, role and style (replaces earlier auto lights; yours stay)").clicked() {
        t.requests.push(Request::AutoLight { density: t.light_density });
    }
    if wide_icon_button(ui, "Eraser", "Remove auto lights").clicked() {
        t.requests.push(Request::ClearAutoLights);
    }
    section_fold(ui, "dg_add_light", "Add a light", "", |ui| {
        let items: Vec<(&str, &str, &str)> = LightKind::ALL
            .iter()
            .map(|k| (light_icon(*k), k.name(), ""))
            .collect();
        let sel = t
            .placing_light
            .and_then(|p| LightKind::ALL.iter().position(|k| *k == p));
        if let Some(i) = tool_grid(ui, 4, &items, sel) {
            let k = LightKind::ALL[i];
            t.placing_light = if t.placing_light == Some(k) {
                None
            } else {
                Some(k)
            };
            t.draw = Draw::Select;
        }
        match t.placing_light {
            Some(k) => hint_small(ui, mount_hint(k.mount())),
            None if !selected => hint_small(
                ui,
                "Click a light's marker in the view to change it; drag it to move it (its torch or crystal comes along).",
            ),
            None => {}
        }
    });
    all_lights(ui, s);
}

fn mount_hint(m: Mount) -> &'static str {
    match m {
        Mount::Wall => "Click a wall: it hangs there, facing out",
        Mount::Floor => "Click the floor: it stands there",
        Mount::Ceiling => "Click the floor under where it should hang",
        Mount::Free => "Click where it should be (it floats 2 m over the floor)",
    }
}

/// Lights › All lights: every light at once (on top of each one's own values), and the colour
/// they all lean towards (the mood's light tint, also in Look › Mood).
fn all_lights(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    let (all, tint, ta) = (m.all_lights, m.mood.tint, m.mood.tint_amount);
    let mut open = false;
    section_fold(ui, "dg_all_lights", "All lights", "", |_| open = true);
    if !open {
        return;
    }
    if all != super::lights::AllLights::default()
        && small_icon_button(ui, "ArrowRotateCounterClockwise", "Reset")
            .on_hover_text("Every light back to its own values")
            .clicked()
    {
        change(s, "All lights", |m| m.all_lights = Default::default());
    }
    let mut b = all.brightness;
    if slider_row_log(ui, "Brightness", &mut b, 0.1..=4.0, pct)
        .on_hover_text("Every light brighter or dimmer at once")
        .changed()
    {
        live(s, "All lights brightness", move |m| {
            m.all_lights.brightness = b
        });
    }
    let mut r = all.reach;
    if slider_row(ui, "Reach", &mut r, 0.3..=3.0, pct)
        .on_hover_text("How far every light carries")
        .changed()
    {
        live(s, "All lights reach", move |m| m.all_lights.reach = r);
    }
    let mut f = all.flicker;
    if slider_row(ui, "Flicker", &mut f, 0.0..=2.5, pct)
        .on_hover_text("How much every fire flickers (0: steady)")
        .changed()
    {
        live(s, "All lights flicker", move |m| m.all_lights.flicker = f);
    }
    let mut tc = tint;
    if color_row(ui, "Colour", &mut tc)
        .on_hover_text("The colour every fire and lamp leans towards, and how far")
        .changed()
    {
        live(s, "Light tint", move |m| m.mood.tint = tc);
    }
    let mut tam = ta;
    if slider_row(ui, "Colour amount", &mut tam, 0.0..=1.0, pct).changed() {
        live(s, "Light tint", move |m| m.mood.tint_amount = tam);
    }
}

fn selected_light(s: &UiState) -> Option<LightDef> {
    let m = s.dungeon_map.0.as_ref()?;
    s.dungeon_tool
        .selected_light
        .and_then(|id| m.lights.iter().find(|l| l.id == id))
        .cloned()
}

/// The selected light (the builder's inspector, Lights): colour, strength, reach, flicker,
/// direction, height, shadows, haze; remove.
fn light_card(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(l) = selected_light(s) else { return };
    let id = l.id;
    let set = |s: &mut UiState, label: &str, f: Box<dyn FnOnce(&mut LightDef)>| {
        let id = id;
        live(s, label, move |m| {
            if let Some(x) = m.lights.iter_mut().find(|x| x.id == id) {
                x.auto = false;
                f(x);
            }
        });
    };
    if l.auto {
        hint_small(ui, "Hung by Auto-light (change it and it's yours).");
    }
    let mut c = l.color;
    if color_row(ui, "Colour", &mut c).changed() {
        set(s, "Light colour", Box::new(move |x| x.color = c));
    }
    let mut lm = l.lumens;
    if slider_row_log(ui, "Strength", &mut lm, 5_000.0..=8_000_000.0, |v| {
        format!("{:.0}k lm", v / 1000.0)
    })
    .changed()
    {
        set(s, "Light strength", Box::new(move |x| x.lumens = lm));
    }
    let mut r = l.range;
    if slider_row(ui, "Reach", &mut r, 1.0..=80.0, |v| format!("{v:.0} m")).changed() {
        set(s, "Light reach", Box::new(move |x| x.range = r));
    }
    let mut f = l.flicker;
    if slider_row(ui, "Flicker", &mut f, 0.0..=1.0, pct).changed() {
        set(s, "Light flicker", Box::new(move |x| x.flicker = f));
    }
    let mut cone = l.cone;
    if slider_row(ui, "Cone (0: all round)", &mut cone, 0.0..=150.0, |v| {
        if v <= 0.0 {
            "all round".into()
        } else {
            format!("{v:.0}°")
        }
    })
    .changed()
    {
        set(s, "Light cone", Box::new(move |x| x.cone = cone));
    }
    if l.cone > 0.0 || l.kind.is_spot() {
        let (mut yaw, mut tilt) = (l.dir[0].to_degrees(), l.dir[1].to_degrees());
        if slider_row(ui, "Points towards", &mut yaw, -180.0..=180.0, |v| {
            format!("{v:.0}°")
        })
        .changed()
        {
            set(
                s,
                "Light direction",
                Box::new(move |x| x.dir[0] = yaw.to_radians()),
            );
        }
        if slider_row(ui, "Tilt down", &mut tilt, -90.0..=90.0, |v| {
            format!("{v:.0}°")
        })
        .changed()
        {
            set(
                s,
                "Light direction",
                Box::new(move |x| x.dir[1] = tilt.to_radians()),
            );
        }
    }
    let mut y = l.pos[1];
    if slider_row(ui, "Height", &mut y, l.pos[1] - 10.0..=l.pos[1] + 10.0, m1).changed() {
        set(s, "Light height", Box::new(move |x| x.pos[1] = y));
    }
    let mut sh = l.shadows;
    if toggle(ui, &mut sh, "Casts shadows (the nearest few do)").changed() {
        change(s, "Light shadows", move |m| {
            if let Some(x) = m.lights.iter_mut().find(|x| x.id == id) {
                x.shadows = sh;
            }
        });
    }
    let mut hz = l.haze;
    let what = if l.cone > 0.0 {
        "a beam of light through the mood's haze"
    } else {
        "a glow round it in the mood's haze"
    };
    if toggle(ui, &mut hz, format!("Haze: {what}"))
        .on_hover_text("How thick the haze is: Look › Mood › Haze")
        .changed()
    {
        change(s, "Light haze", move |m| {
            if let Some(x) = m.lights.iter_mut().find(|x| x.id == id) {
                x.haze = hz;
            }
        });
    }
    if button_of(ui, "TrashCan", "Remove light", Btn::Bad).clicked() {
        change(s, "Remove light", move |m| m.lights.retain(|x| x.id != id));
        s.dungeon_tool.selected_light = None;
    }
}

// ------------------------------------------------------------------ over the view

/// Where an island at the top centre goes: under `below` (an area drawn this frame, if it shows),
/// else under the environment pill, else at the top.
fn top_centre_y(ctx: &egui::Context, screen: egui::Rect, below: &[&str]) -> f32 {
    ctx.memory(|mm| {
        for id in below {
            let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new(*id));
            if mm.areas().is_visible(&layer)
                && let Some(r) = mm.area_rect(egui::Id::new(*id))
            {
                return r.bottom() - screen.top() + 8.0;
            }
        }
        12.0
    })
}

/// The "Back to world" capsule over a dungeon's map (top centre): where you are, Ceilings,
/// Walk in, Back to world.
pub fn map_bar(ctx: &egui::Context, s: &mut UiState, screen: egui::Rect) {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return;
    };
    if s.walk.active {
        return;
    }
    let name = m.name.clone();
    let realm = m.kind == MapKind::Realm;
    let icon = if realm { "CloudySun" } else { "Castle" };
    let world = match (&s.dungeon_session.world_name, &s.dungeon_session.world) {
        (n, _) if !n.is_empty() => n.clone(),
        (_, Some(p)) => p
            .file_stem()
            .map(|f| f.to_string_lossy().replace('_', " "))
            .unwrap_or_else(|| "the world".into()),
        _ => "the world".into(),
    };
    let y = top_centre_y(ctx, screen, &["halo_env"]);
    let mut back = false;
    let mut walk = false;
    let mut ceilings = false;
    let cut = s.dungeon_view.cut;
    egui::Area::new(egui::Id::new("dungeon_map_bar"))
        .order(egui::Order::Middle)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, y))
        .show(ctx, |ui| {
            let frame = glass_frame()
                .corner_radius(egui::CornerRadius::same(13))
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 5,
                    top: 5,
                    bottom: 5,
                });
            glass(ui, frame, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    ui.set_height(28.0);
                    icon_label(ui, icon, 15.0, TEXT);
                    ui.label(
                        RichText::new(&name)
                            .font(FontId::new(13.5, semibold()))
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} of {}",
                            if realm { "A realm" } else { "A dungeon" },
                            world
                        ))
                        .font(FontId::new(11.5, mono()))
                        .color(TEXT_MUTE),
                    );
                    cap_sep(ui);
                    // the builder cuts the view away above the edited level to see into the rooms
                    if !realm
                        && cap_button(
                            ui,
                            if cut { "EyeSlash" } else { "LayersTwo" },
                            "Ceilings",
                            !cut,
                            false,
                        )
                        .on_hover_text(if cut {
                            "Ceilings are hidden so you can see into the rooms: click to show them"
                        } else {
                            "Ceilings are shown: click to hide them and see into the rooms"
                        })
                        .clicked()
                    {
                        ceilings = true;
                    }
                    if cap_button(ui, "Footsteps", "Walk in", false, false)
                        .on_hover_text("Explore it on foot from where players arrive")
                        .clicked()
                    {
                        walk = true;
                    }
                    if cap_button(ui, "ArrowLeft", "Back to world", false, true)
                        .on_hover_text("Save this map and return to the world")
                        .clicked()
                    {
                        back = true;
                    }
                });
            });
        });
    if ceilings {
        s.dungeon_view.cut = !cut;
    }
    if back {
        s.dungeon_tool.requests.push(Request::Back);
    }
    if walk {
        s.dungeon_tool.requests.push(Request::WalkIn);
    }
}

/// The guide pill (top centre) while a way in is being picked or a room drawn: what to do next.
pub fn guide_bar(ctx: &egui::Context, s: &mut UiState) {
    let (icon, head, sub) = if let Some(p) = s.dungeon_tool.pick {
        let k = match p {
            Pick::New(k) | Pick::Add(_, k) => k,
        };
        let prop = s
            .dungeon_tool
            .new_prop
            .as_deref()
            .filter(|_| k == EntranceKind::Object)
            .and_then(|a| s.lib.as_deref().and_then(|l| l.get(a)))
            .map(|e| e.name.clone());
        match prop {
            Some(name) => ("Door", format!("Put down the {}", name.to_lowercase()), "Click the ground where it goes: stepping onto it enters the dungeon  ·  Esc cancels".to_string()),
            None => (way_icon(k), "Pick the way in".to_string(), k.hint().to_string()),
        }
    } else if s
        .dungeon_map
        .0
        .as_ref()
        .is_some_and(|m| m.kind == MapKind::Interior)
        && s.ed.tab == Tab::Dungeons
        && s.dungeon_tool.draw != Draw::Select
        && s.dungeon_tool.placing_light.is_none()
    {
        let d = s.dungeon_tool.draw;
        let n = s.dungeon_tool.path.len();
        let head = if d.by_points() && n > 0 {
            format!(
                "{} · {} point{}",
                d.name(),
                n,
                if n == 1 { "" } else { "s" }
            )
        } else {
            d.name().to_string()
        };
        (d.icon(), head, format!("{}  ·  Esc cancels", d.hint()))
    } else {
        return;
    };
    let screen = ctx.content_rect();
    let y = top_centre_y(ctx, screen, &["dungeon_map_bar", "halo_env"]);
    egui::Area::new(egui::Id::new("dungeon_guide_bar"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, y))
        .show(ctx, |ui| {
            let frame = glass_frame()
                .corner_radius(egui::CornerRadius::same(16))
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 14,
                    top: 8,
                    bottom: 9,
                });
            glass(ui, frame, |ui| {
                ui.set_max_width(480.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    icon_label(ui, icon, 15.0, Color32::WHITE);
                    ui.label(
                        RichText::new(head)
                            .font(FontId::new(13.0, semibold()))
                            .color(TEXT),
                    );
                });
                ui.add(egui::Label::new(RichText::new(sub).size(12.0).color(TEXT_DIM)).wrap());
            });
        });
}

/// Building while walking a dungeon: Enter opens a bar over the walk view — quick things where
/// you look (a torch on the wall, a brazier on the floor) or in the room you're in (light it,
/// furnish it), and anything else in words for the area agent, on this room. Walking stops while
/// it's open; Esc closes it.
pub fn walk_build_bar(ctx: &egui::Context, s: &mut UiState, screen: egui::Rect) {
    let underground = s.walk.active && s.dungeon_map.interior();
    let t = &mut *s.dungeon_tool;
    if !underground {
        t.build_bar = false;
        return;
    }
    // the agent's progress (also once the bar is closed)
    if !t.walk_status.is_empty() {
        let p = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("walk_build_status"),
        ));
        let g = p.layout_no_wrap(t.walk_status.clone(), FontId::proportional(13.0), TEXT);
        let r = egui::Rect::from_center_size(
            egui::pos2(screen.center().x, screen.max.y - 64.0),
            g.size() + egui::vec2(28.0, 14.0),
        );
        p.add(soft_shadow(r, 12.0, Color32::from_black_alpha(120)));
        p.rect(
            r,
            egui::CornerRadius::same(255),
            GLASS_SOLID,
            egui::Stroke::new(1.0, LINE_STRONG),
            egui::StrokeKind::Inside,
        );
        p.galley(r.min + egui::vec2(14.0, 7.0), g, TEXT);
    }
    if !t.build_bar {
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !ctx.egui_wants_keyboard_input() {
            t.build_bar = true;
            t.build_text.clear();
        }
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        t.build_bar = false;
        return;
    }
    let look = t.look;
    let on_wall = look.is_some_and(|(_, n, _)| n.y.abs() < 0.5);
    let on_floor = look.is_some_and(|(_, n, _)| n.y > 0.6);
    let room = look.and_then(|l| l.2);
    let mut close = false;
    let mut req: Option<Request> = None;
    egui::Area::new(egui::Id::new("walk_build_bar")).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -96.0)).show(ctx, |ui| {
        glass(ui, glass_frame().inner_margin(egui::Margin::symmetric(14, 11)), |ui| {
            ui.set_width(560.0f32.min(screen.width() - 40.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                icon_label(ui, "Hammer", 15.0, Color32::WHITE);
                ui.label(RichText::new("Build here").font(FontId::new(13.0, semibold())).color(TEXT));
                ui.label(
                    RichText::new(match (on_wall, on_floor) {
                        (true, _) => "looking at a wall",
                        (_, true) => "looking at the floor",
                        _ => "look at a wall or the floor",
                    })
                    .size(11.5)
                    .color(TEXT_MUTE),
                );
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                let chip = |ui: &mut egui::Ui, icon: &str, label: &str, on: bool| ui.add_enabled_ui(on, |ui| icon_chip(ui, icon, label)).inner.clicked();
                if chip(ui, "Torch", "Torch on the wall", on_wall)
                    && let Some((p, n, _)) = look
                {
                    req = Some(Request::WallFixture { kind: LightKind::Torch, at: p, normal: n });
                }
                if chip(ui, "Fire1", "Brazier here", on_floor)
                    && let Some((p, _, _)) = look
                {
                    req = Some(Request::FloorFixture { kind: LightKind::Brazier, at: p });
                }
                if chip(ui, "Fire2", "Candles here", on_floor)
                    && let Some((p, _, _)) = look
                {
                    req = Some(Request::FloorFixture { kind: LightKind::Candles, at: p });
                }
                if chip(ui, "LightBulb", "Light this room", room.is_some())
                    && let Some(id) = room
                {
                    req = Some(Request::AutoLightRoom(id));
                }
                if chip(ui, "Armchair", "Furnish this room", room.is_some())
                    && let Some(id) = room
                {
                    req = Some(Request::DressRoom(id));
                }
            });
            let field = well(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut t.build_text).frame(egui::Frame::NONE).margin(egui::Margin::ZERO).hint_text(RichText::new("Or say it… e.g. this room feels empty · make this a shrine · flood it").color(TEXT_MUTE)).desired_width(f32::INFINITY))
            });
            if !field.has_focus() && t.build_text.is_empty() {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !t.build_text.trim().is_empty() {
                t.walk_prompt = Some(t.build_text.trim().to_string());
                t.build_text.clear();
                close = true;
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                dock_hints(ui, &[("↵", "Sends"), ("Esc", "Closes")]);
                ui.label(RichText::new("walking resumes when it closes").size(11.5).color(TEXT_MUTE));
            });
        });
    });
    if let Some(r) = req {
        t.requests.push(r);
        close = true;
    }
    if close {
        t.build_bar = false;
    }
}

// ------------------------------------------------------------------ Halo hooks (inspector, dock)

/// The inspector's title when the Dungeons tab has something to inspect (None hides it): the
/// picked dungeon (world); in an underground map the selected light (Lights), piece or room, or
/// the plan. Its body ([`dungeons_right`]) starts with content (the Halo header has the title).
pub fn dungeons_inspector(s: &UiState) -> Option<(String, bool)> {
    let Some(m) = s.dungeon_map.0.as_ref() else {
        return s
            .dungeons
            .selected
            .and_then(|id| s.dungeons.get(id))
            .map(|d| (d.name.clone(), true));
    };
    if m.kind == MapKind::Realm {
        return None;
    }
    let t = &*s.dungeon_tool;
    if s.ed.tab == Tab::DungeonLights
        && let Some(l) = selected_light(s)
    {
        return Some((
            format!("{}{}", l.kind.name(), if l.auto { " (auto)" } else { "" }),
            true,
        ));
    }
    if let Some(p) = t
        .selected_piece
        .and_then(|id| m.pieces.iter().find(|p| p.id == id))
    {
        return Some((format!("{} #{}", p.kind.name(), p.id), true));
    }
    if let Some(r) = t.selected_room.and_then(|id| m.plan.room(id)) {
        return Some((r.label(), true));
    }
    Some(("Plan".into(), true))
}

/// The keys and settings of the drawing tool in the dock.
fn draw_hints(d: Draw) -> &'static [(&'static str, &'static str)] {
    match d {
        Draw::Select => &[
            ("Click", "Select a room"),
            ("Drag", "Move it or a wall"),
            ("⌥ Drag", "Alcove"),
            ("⇧ Drag", "Height"),
            ("⌫", "Remove"),
        ],
        Draw::Erase => &[("Click", "Remove a room")],
        Draw::Corridor | Draw::Chasm => &[
            ("Click", "Add a point"),
            ("↵", "Finish"),
            ("⌫", "Undo a point"),
            ("Esc", "Cancel"),
        ],
        Draw::Cavern => &[
            ("Click", "Round its edge"),
            ("Drag", "A loop"),
            ("↵", "Finish"),
            ("Esc", "Cancel"),
        ],
        Draw::Stairs => &[
            ("Click", "Where they start"),
            ("Click", "Where they end"),
            ("Esc", "Cancel"),
        ],
        Draw::Bridge => &[
            ("Click", "One end"),
            ("Click", "The other"),
            ("Esc", "Cancel"),
        ],
        Draw::Round | Draw::Octagon | Draw::Pit | Draw::Spiral => &[
            ("Drag", "From the middle out"),
            ("⌥", "No snap"),
            ("Esc", "Cancel"),
        ],
        Draw::WallDoor
        | Draw::WallSecret
        | Draw::Window
        | Draw::Alcove
        | Draw::WallTorch
        | Draw::WallLantern => &[("Click", "A wall"), ("Esc", "Stop")],
        _ => &[
            ("Drag", "Corner to corner"),
            ("⌥", "No snap"),
            ("Esc", "Cancel"),
        ],
    }
}

/// The tool dock for the Dungeons tabs: picking a way in; in a dungeon's map the drawing tool
/// (with the stairs' up / down switch), the piece, light or decal being placed (the decal's size),
/// and their keys.
pub fn dungeons_dock(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    let t = &mut *s.dungeon_tool;
    // the world: picking a way in, or its spot (Describe it)
    if let Some(p) = t.pick {
        let k = match p {
            Pick::New(k) | Pick::Add(_, k) => k,
        };
        let prop = t
            .new_prop
            .as_deref()
            .filter(|_| k == EntranceKind::Object)
            .and_then(|a| s.lib.as_deref().and_then(|l| l.get(a)))
            .map(|e| e.name.clone());
        match &prop {
            Some(name) => dock_title(ui, "Door", &format!("Put down the {}", name.to_lowercase())),
            None => dock_title(ui, way_icon(k), "Pick the way in"),
        }
        dock_sep(ui);
        let click = match (k, prop.is_some()) {
            (_, true) | (EntranceKind::Portal, _) => "The ground",
            (EntranceKind::Object, _) => "An object",
            (EntranceKind::Point, _) => "A spot on the ground",
            (EntranceKind::Cave, _) => "A cave",
        };
        dock_hints(ui, &[("Click", click), ("Esc", "Cancel")]);
        if small_icon_button(ui, "CrossSmall", "Cancel").clicked() {
            t.pick = None;
        }
        return true;
    }
    if t.describe.picking {
        dock_title(ui, "MapPin", "Pick the spot");
        dock_sep(ui);
        dock_hints(ui, &[("Click", "Where its portal goes"), ("Esc", "Cancel")]);
        return true;
    }
    let Some(m) = s.dungeon_map.0.as_ref() else {
        dock_title(ui, "Castle", "Dungeons");
        dock_sep(ui);
        dock_hints(ui, &[("RMB", "Orbit"), ("Scroll", "Zoom")]);
        return true;
    };
    if m.kind == MapKind::Realm {
        dock_title(ui, "CloudySun", "Realm");
        dock_sep(ui);
        dock_text(ui, "Build it with the world's tools");
        dock_sep(ui);
        dock_hints(ui, &[("RMB", "Orbit"), ("Scroll", "Zoom")]);
        return true;
    }
    match s.ed.tab {
        Tab::DungeonLook => match t.placing_decal {
            Some(k) => {
                dock_title(ui, "Brush", k.name());
                dock_sep(ui);
                slider_mini(ui, "Size", &mut t.decal_size, 0.3..=8.0, m1);
                dock_sep(ui);
                dock_hints(
                    ui,
                    &[
                        ("Click", "A wall or the floor"),
                        ("⌥ Click", "Remove the nearest"),
                        ("Esc", "Stop"),
                    ],
                );
            }
            None => {
                dock_title(ui, "ColorPalette", "Look");
                dock_sep(ui);
                dock_hints(ui, &[("RMB", "Orbit"), ("Scroll", "Zoom")]);
            }
        },
        Tab::DungeonLights => match t.placing_light {
            Some(k) => {
                dock_title(ui, light_icon(k), k.name());
                dock_sep(ui);
                let click = match k.mount() {
                    Mount::Wall => "A wall",
                    Mount::Floor => "The floor",
                    Mount::Ceiling => "The floor under it",
                    Mount::Free => "Where it floats",
                };
                dock_hints(ui, &[("Click", click), ("Esc", "Stop")]);
            }
            None => {
                dock_title(ui, "LightbulbGlow", "Lights");
                dock_sep(ui);
                dock_hints(ui, &[("Click", "Select a light"), ("Drag", "Move it")]);
            }
        },
        _ => match t.placing_piece {
            Some(k) => {
                dock_title(ui, piece_icon(k), k.name());
                dock_sep(ui);
                dock_hints(
                    ui,
                    &[
                        ("Click", "Put it down"),
                        ("⌥ Click", "Remove the nearest"),
                        ("Esc", "Stop"),
                    ],
                );
            }
            None => {
                let d = t.draw;
                dock_title(ui, d.icon(), d.name());
                if matches!(d, Draw::Stairs | Draw::Spiral) {
                    dock_sep(ui);
                    let mut up = t.stairs_up;
                    ui.allocate_ui(egui::vec2(200.0, 26.0), |ui| {
                        ui.set_width(200.0);
                        segmented_solid(
                            ui,
                            &mut up,
                            &[(false, "Down a level"), (true, "Up a level")],
                        );
                    });
                    t.stairs_up = up;
                }
                if d.by_points() && !t.path.is_empty() {
                    dock_sep(ui);
                    let n = t.path.len();
                    dock_text(ui, &format!("{n} point{}", if n == 1 { "" } else { "s" }));
                }
                dock_sep(ui);
                dock_hints(ui, draw_hints(d));
            }
        },
    }
    true
}
