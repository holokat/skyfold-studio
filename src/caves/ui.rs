//! Terrain › Caves in the Halo layout: the 5-tool grid and each tool's options in the tool panel
//! (what, size, levels, look, inside, features, the dungeon door) with the caves in this world;
//! the selected cave's card in the inspector (its recipe, Shuffle, grow, levels, chambers to
//! lock, the dungeon door, x-ray, walk in); the dig brush and key hints in the dock; and what
//! happens in the viewport: a preview where a click would put the cave and which way it would
//! run, "+" handles on dead-end chambers that grow the cave that way, entrance markers, the dig
//! brush, and the capsule over the view while the camera is in a cave.
//!
//! The panels only ask (`CaveTool::requests`); `apply_requests` makes every change through
//! `edit` (one undo step each, the same as the MCP tools).

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, pos2, vec2};

use super::edit::{self, NewCave};
use super::layout;
use super::model::*;
use super::render::CaveWorld;
use crate::camera::{MainCamera, OrbitCamera, Pointer};
use crate::editor::Tab;
use crate::io::WorldData;
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tool {
    #[default]
    Add,
    Arch,
    Dig,
    Fill,
    Select,
}

/// A change asked for by the panels.
#[derive(Clone, Debug)]
pub enum Request {
    Kind(u32, CaveKind),
    Size(u32, CaveSize),
    Levels(u32, u8),
    Up(u32, bool),
    /// a pool's lip (cave, chamber)
    Rim(u32, u16, RimStyle),
    Theme(u32, Theme),
    Darkness(u32, Darkness),
    Features(u32, Features),
    Name(u32, String),
    Dungeon(u32, Option<String>),
    Instancing(u32, Instancing),
    Shuffle(u32),
    Extend {
        id: u32,
        from: Option<u16>,
        heading: Option<f32>,
        down: bool,
    },
    Lock(u32, u16, bool),
    Remove(u32),
    WalkIn(u32),
    LookInside(u32),
    /// the camera into this chamber
    ViewChamber(u32, u16),
    /// the camera into the cave and Objects › Place
    Decorate(u32),
    /// the camera out in front of the cave's way in
    Leave(u32),
    /// walk on from where the camera is (in a cave)
    WalkHere,
}

/// What the next click makes.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub kind: CaveKind,
    pub size: CaveSize,
    pub levels: u8,
    /// the levels climb into the hill
    pub up: bool,
    pub theme: Theme,
    pub darkness: Darkness,
    pub features: Features,
    pub dungeon: bool,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            kind: CaveKind::Cave,
            size: CaveSize::Medium,
            levels: 1,
            up: false,
            theme: Theme::Auto,
            darkness: Darkness::Dim,
            features: Features::default(),
            dungeon: false,
        }
    }
}

#[derive(Resource)]
pub struct CaveTool {
    pub tool: Tool,
    pub recipe: Recipe,
    pub selected: Option<u32>,
    pub dig_radius: f32,
    /// where the cursor meets the ground or a cave's rock (caves aware; the terrain's own pointer
    /// hits the ground over a cave mouth)
    pub hit: Option<Vec3>,
    pub requests: Vec<Request>,
    /// the name being typed (applied when the field is left)
    name_edit: Option<(u32, String)>,
    dungeon_edit: Option<(u32, String)>,
    /// the press started on a "+" handle (chamber, heading)
    pressed_handle: Option<(u16, f32)>,
    drag_start: Option<Vec3>,
    dig_path: Vec<Vec3>,
}

impl Default for CaveTool {
    fn default() -> Self {
        Self {
            tool: Tool::Add,
            recipe: Recipe::default(),
            selected: None,
            dig_radius: 2.5,
            hit: None,
            requests: vec![],
            name_edit: None,
            dungeon_edit: None,
            pressed_handle: None,
            drag_start: None,
            dig_path: vec![],
        }
    }
}

/// The bottom bar's left-mouse hint.
pub fn lmb_hint(t: &CaveTool) -> &'static str {
    match t.tool {
        Tool::Add => "Click · add it here   + handle · grow the cave",
        Tool::Arch => "Drag · from one foot to the other",
        Tool::Dig => "Drag · dig through rock",
        Tool::Fill => "Drag · fill rock in",
        Tool::Select => "Click · pick a cave",
    }
}

// ------------------------------------------------------------------ small Halo bits (private)

/// The tools in grid order: (tool, Central icon, name).
const CAVE_TOOLS: [(Tool, &str, &str); 5] = [
    (Tool::Add, "PlusMedium", "Add a cave"),
    (Tool::Arch, "Bridge", "Rock arch"),
    (Tool::Dig, "Shovel", "Dig"),
    (Tool::Fill, "Bucket", "Fill"),
    (Tool::Select, "Cursor1", "Select"),
];

/// The kinds a click can make (arches have their own tool).
const KINDS: [CaveKind; 5] = [
    CaveKind::Cave,
    CaveKind::Mine,
    CaveKind::Pit,
    CaveKind::Passage,
    CaveKind::Cavern,
];

fn kind_icon(k: CaveKind) -> &'static str {
    match k {
        CaveKind::Cave => "Shovel",
        CaveKind::Mine => "Hammer2",
        CaveKind::Pit => "ArrowDown",
        CaveKind::Passage => "ArrowsRepeat",
        CaveKind::Cavern => "EyeSlash",
        CaveKind::Arch => "Bridge",
    }
}

fn tool_hint(t: Tool) -> &'static str {
    match t {
        Tool::Add => {
            "Click the ground and a cave is laid out for you: its mouth opened, its rock built, plants cleared. Change anything afterwards."
        }
        Tool::Arch => CaveKind::Arch.hint(),
        Tool::Dig => {
            "Drag over rock or ground to dig it away: widen a tunnel, cut an arch through a ridge, open a way into a hidden cavern."
        }
        Tool::Fill => {
            "Drag to fill rock back in: close a tunnel, narrow a passage, build up a ledge."
        }
        Tool::Select => "Click a cave (its entrance or its rock) to change it.",
    }
}

/// One line of text, cut with an ellipsis at `max_w`.
fn one_line(
    p: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = max_w.max(8.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    p.layout_job(job)
}

/// A labelled text field: dim label, then a dark well.
fn text_field(ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str) -> egui::Response {
    if !label.is_empty() {
        ui.label(RichText::new(label).size(12.5).color(TEXT_DIM));
    }
    egui::Frame::new()
        .fill(WELL)
        .corner_radius(CornerRadius::same(R_INPUT))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .frame(egui::Frame::NONE)
                    .hint_text(RichText::new(hint).color(TEXT_MUTE))
                    .desired_width(ui.available_width()),
            )
        })
        .inner
}

/// A full-width soft button in the danger colour (Remove …).
fn danger_button(ui: &mut egui::Ui, icon: &str, text: &str) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.is_pointer_button_down_on() {
            Color32::from_white_alpha(46)
        } else if resp.hovered() {
            SOFT_HOVER
        } else {
            SOFT
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_BUTTON), fill);
        let g = ui
            .painter()
            .layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), DANGER);
        let iw = 20.0;
        let x = rect.center().x - (g.size().x + iw) * 0.5;
        paint_icon(ui, pos2(x + 7.0, rect.center().y), icon, 14.0, DANGER);
        ui.painter()
            .galley(pos2(x + iw, rect.center().y - g.size().y * 0.5), g, DANGER);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Soft icon buttons sharing one row evenly; returns which was clicked. `None` entries are skipped.
fn button_row(ui: &mut egui::Ui, buttons: &[Option<(&str, &str, &str)>]) -> Option<usize> {
    let shown: Vec<(usize, (&str, &str, &str))> = buttons
        .iter()
        .enumerate()
        .filter_map(|(i, b)| b.map(|b| (i, b)))
        .collect();
    if shown.is_empty() {
        return None;
    }
    let mut out = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let n = shown.len() as f32;
        let w = ((ui.available_width() - 6.0 * (n - 1.0)) / n).floor();
        for (i, (icon, text, tip_text)) in &shown {
            let r = icon_text_button(ui, icon, text, Some(w));
            let r = if tip_text.is_empty() {
                r
            } else {
                r.on_hover_text(*tip_text)
            };
            if r.clicked() {
                out = Some(*i);
            }
        }
    });
    out
}

/// A dim label then a widget filling the rest of the row.
fn labelled<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT_DIM));
        ui.add_space(4.0);
        add(ui)
    })
    .inner
}

// ------------------------------------------------------------------ tool panel

fn theme_label(t: Theme, resolved: Option<Theme>) -> String {
    match (t, resolved) {
        (Theme::Auto, Some(r)) if r != Theme::Auto => format!("Auto ({})", r.name()),
        (Theme::Auto, _) => "Auto (from the biome)".into(),
        (t, _) => t.name().into(),
    }
}

/// The look theme as a full-width dropdown.
fn theme_combo(ui: &mut egui::Ui, id: &str, value: &mut Theme, resolved: Option<Theme>) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width())
        .selected_text(theme_label(*value, resolved))
        .dropdown_ui(ui, |ui| {
            for t in Theme::ALL {
                if menu_check(ui, *value == t, theme_label(t, None)).clicked() && *value != t {
                    *value = t;
                    changed = true;
                }
            }
        });
    changed
}

fn features_ui(ui: &mut egui::Ui, f: &mut Features, theme: Theme) -> bool {
    let mut changed = false;
    let formations = match theme {
        Theme::Ice => "Icicles & ice columns",
        Theme::Sandstone | Theme::Volcanic | Theme::Mine => "Formations (none in this rock)",
        _ => "Stalactites & columns",
    };
    for (v, label, tip) in [
        (
            &mut f.formations,
            formations,
            "Stalactites hanging from the roofs, stalagmites under them, columns where they meet — giant ones in great halls",
        ),
        (
            &mut f.pools,
            "Pools & lakes",
            "Still water in hollows of some chambers (wade in and out); lakes in great halls",
        ),
        (
            &mut f.rubble,
            "Fallen rock",
            "Boulders and scree on the floors",
        ),
        (
            &mut f.skylights,
            "Daylight holes",
            "Shafts of daylight through holes in the roofs of big chambers near the surface",
        ),
        (
            &mut f.crystals,
            "Glowing crystals",
            "Crystal clusters that glow and light the chambers — towering ones in great halls",
        ),
        (
            &mut f.bridges,
            "Ledges, bridges & arches",
            "Where the way changes level in a tall chamber it climbs round the wall on a ledge (a rock bridge higher up); natural arches stand over the way",
        ),
        (
            &mut f.waterfalls,
            "Waterfalls",
            "Water pouring from high in the walls of tall chambers into their pools",
        ),
        (
            &mut f.cobwebs,
            "Cobwebs",
            "Webs in the corners and across dead-end passages",
        ),
    ] {
        changed |= toggle(ui, v, label).on_hover_text(tip).changed();
    }
    changed
}

/// Levels 1–4, and (with more than one) Down or Up beside it. Returns (levels changed, direction changed).
fn levels_ui(ui: &mut egui::Ui, levels: &mut u8, up: &mut bool) -> (bool, bool) {
    let (mut changed, mut dir) = (false, false);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = ui.available_width();
        let lw = if *levels > 1 {
            ((w - 6.0) * 0.5).floor()
        } else {
            w
        };
        ui.allocate_ui(vec2(lw, 26.0), |ui| {
            let mut l = *levels;
            if segmented(ui, &mut l, &[(1u8, "1"), (2, "2"), (3, "3"), (4, "4")]) {
                *levels = l;
                changed = true;
            }
        });
        if *levels > 1 {
            ui.allocate_ui(vec2(ui.available_width(), 26.0), |ui| {
                let mut u = *up;
                if segmented_icons(
                    ui,
                    &mut u,
                    &[(false, "ArrowDown", "Down"), (true, "ArrowUp", "Up")],
                    false,
                ) {
                    *up = u;
                    dir = true;
                }
            });
        }
    });
    hint_small(
        ui,
        "Each level deeper (Down), or climbing into the hill (Up, where the hill is high enough).",
    );
    (changed, dir)
}

fn size_hint(ui: &mut egui::Ui, size: CaveSize) {
    match size {
        CaveSize::Giant => hint_small(
            ui,
            "XXL: a great hall 60–80 m across (about 30 m high) with chambers round it.",
        ),
        CaveSize::Colossal => hint_small(
            ui,
            "XXXL: two vast halls 130–180 m across (about 55 m high): a whole underground world.",
        ),
        _ => {}
    }
}

/// What a cave is, in a line ("Cave · 7 chambers · 2 levels · Limestone").
fn summary(c: &Cave) -> String {
    if c.kind == CaveKind::Arch {
        format!("Rock arch · {} rock", c.theme().name())
    } else {
        format!(
            "{} · {} chambers · {} level{} · {}",
            c.kind.name(),
            c.chambers.len(),
            c.levels,
            if c.levels == 1 { "" } else { "s" },
            c.theme().name()
        )
    }
}

pub fn caves_left(ui: &mut egui::Ui, s: &mut UiState) {
    crate::ui::tips::card(
        ui,
        "caves",
        "Caves",
        &[
            "Pick what to make below, then click a hillside (a cave mouth in a rock face) or flat ground (a sinkhole).",
            "Change its size, levels, look and features on the right. Shuffle lays it out anew; lock chambers you like first.",
            "Click a + to grow the cave that way. X-ray looks down into it; Walk in to explore it.",
            "Dig and Fill reshape rock by hand: tunnels, arches cut through ridges, a way into a hidden cavern.",
        ],
    );
    let t = &mut s.cave_tool;
    section_fold(ui, "cave_tools", "Caves & arches", "", |ui| {
        let items: Vec<(&str, &str, &str)> =
            CAVE_TOOLS.iter().map(|(_, i, n)| (*i, *n, "")).collect();
        let sel = CAVE_TOOLS.iter().position(|c| c.0 == t.tool);
        if let Some(i) = tool_grid(ui, 3, &items, sel) {
            t.tool = CAVE_TOOLS[i].0;
        }
        dim(ui, tool_hint(t.tool));
    });
    match t.tool {
        Tool::Add => {
            section_fold(ui, "cave_what", "What", "", |ui| {
                let cur = KINDS.iter().position(|k| *k == t.recipe.kind);
                if let Some(i) = card_grid(ui, 3, KINDS.len(), |ui, i, w| {
                    icon_card(ui, kind_icon(KINDS[i]), KINDS[i].name(), cur == Some(i), w)
                        .on_hover_text(KINDS[i].hint())
                }) {
                    let k = KINDS[i];
                    t.recipe.kind = k;
                    if k == CaveKind::Mine {
                        t.recipe.features.formations = false;
                    }
                }
                hint_small(ui, t.recipe.kind.hint());
            });
            section_fold(ui, "cave_size", "Size", "", |ui| {
                let mut size = t.recipe.size;
                if segmented(ui, &mut size, &CaveSize::ALL.map(|s| (s, s.name()))) {
                    t.recipe.size = size;
                }
                size_hint(ui, t.recipe.size);
            });
            section_fold(ui, "cave_levels", "Levels", "", |ui| {
                let r = &mut t.recipe;
                let _ = levels_ui(ui, &mut r.levels, &mut r.up);
            });
            section_fold(ui, "cave_look", "Look", "", |ui| {
                theme_combo(ui, "new_cave_theme", &mut t.recipe.theme, None);
                ui.add_space(2.0);
                labelled(ui, "Inside", |ui| {
                    let mut d = t.recipe.darkness;
                    if segmented(ui, &mut d, &Darkness::ALL.map(|d| (d, d.name()))) {
                        t.recipe.darkness = d;
                    }
                });
            });
            section_fold(ui, "cave_features", "Features", "", |ui| {
                let th = if t.recipe.theme == Theme::Auto {
                    Theme::Limestone
                } else {
                    t.recipe.theme
                };
                features_ui(ui, &mut t.recipe.features, th);
            });
            toggle(ui, &mut t.recipe.dungeon, "A dungeon behind a door at the end")
                .on_hover_text("A door in the deepest chamber: in the game it loads the dungeon's own map (a party instance, shared, or a raid)");
        }
        Tool::Arch => {
            section_fold(ui, "cave_arch_look", "Look", "", |ui| {
                theme_combo(ui, "new_arch_theme", &mut t.recipe.theme, None);
            });
        }
        Tool::Dig | Tool::Fill => hint_small(ui, "The brush radius is in the dock below."),
        Tool::Select => {}
    }
    if !s.caves.list.is_empty() {
        let note = if s.cave_world.busy() {
            format!("{} · building rock…", s.caves.list.len())
        } else {
            s.caves.list.len().to_string()
        };
        section_fold(ui, "cave_list", "In this world", &note, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for c in &s.caves.list {
                let sel = t.selected == Some(c.id);
                let meta = if c.kind == CaveKind::Arch {
                    ""
                } else {
                    c.size.name()
                };
                if list_row(
                    ui,
                    Lead::Icon(kind_icon(c.kind)),
                    &c.name,
                    &summary(c),
                    meta,
                    sel,
                )
                .clicked()
                {
                    t.selected = Some(c.id);
                    // frame it
                    let (lo, hi) = c.bounds();
                    let e = Vec3::from_array(c.entrance);
                    let r = (hi - lo).xz().length().max(30.0);
                    s.orbit
                        .set_view(e, c.heading + std::f32::consts::PI, 0.55, r * 1.1);
                }
            }
            if t.selected.is_none() {
                hint_small(
                    ui,
                    "Pick a cave (Select, or its name here) to change it in the inspector.",
                );
            }
        });
    }
}

// ------------------------------------------------------------------ inspector (the selected cave)

/// A chamber row: its place on the way in, what it is, and camera / grow / lock buttons. Returns
/// (camera, grow, lock) clicks.
fn chamber_row(ui: &mut egui::Ui, n: usize, ch: &Chamber) -> (bool, bool, bool) {
    let w = ui.available_width();
    let mut out = (false, false, false);
    // (the hover fill goes under the buttons: its slot is taken first, filled in after)
    let bg = ui.painter().add(egui::Shape::Noop);
    let row = ui.allocate_ui_with_layout(
        vec2(w, 38.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.set_min_size(vec2(w, 38.0));
            ui.spacing_mut().item_spacing.x = 1.0;
            ui.add_space(2.0);
            out.2 = icon_toggle(
                ui,
                if ch.locked { "Lock" } else { "Unlocked" },
                if ch.locked {
                    "Locked: Shuffle keeps it"
                } else {
                    "Lock: Shuffle keeps it"
                },
                ch.locked,
            )
            .clicked();
            out.1 = icon_button(ui, "PlusMedium", "Grow a tunnel and chamber from here").clicked();
            out.0 = icon_button(ui, "Camera1", "Go in there with the camera").clicked();
            ui.allocate_exact_size(vec2(ui.available_width(), 38.0), egui::Sense::hover())
                .0
        },
    );
    let (rect, text) = (row.response.rect, row.inner);
    if !ui.is_rect_visible(rect) {
        return out;
    }
    if ui.rect_contains_pointer(rect) {
        ui.painter().set(
            bg,
            egui::Shape::rect_filled(rect, CornerRadius::same(R_INPUT), SOFT),
        );
    }
    let p = ui.painter();
    let ng = p.layout_no_wrap(n.to_string(), FontId::new(11.0, mono()), TEXT_MUTE);
    p.galley(
        pos2(rect.left() + 8.0, rect.center().y - ng.size().y * 0.5),
        ng,
        TEXT_MUTE,
    );
    let tx = rect.left() + 28.0;
    let x = text.right();
    let extra = match (ch.pool, ch.skylight) {
        (true, true) => " · pool, daylight",
        (true, false) => " · pool",
        (false, true) => " · daylight",
        _ => "",
    };
    let tg = one_line(
        p,
        &format!("Chamber {}", ch.id),
        FontId::new(13.0, medium()),
        TEXT,
        x - tx - 4.0,
    );
    let sg = one_line(
        p,
        &format!(
            "level {} · {:.0}×{:.0} m{extra}",
            ch.level + 1,
            ch.size[0] * 2.0,
            ch.size[2] * 2.0
        ),
        FontId::proportional(11.5),
        TEXT_MUTE,
        x - tx - 4.0,
    );
    let top = rect.center().y - (tg.size().y + sg.size().y) * 0.5;
    let th = tg.size().y;
    p.galley(pos2(tx, top), tg, TEXT);
    p.galley(pos2(tx, top + th), sg, TEXT_MUTE);
    out
}

pub fn caves_right(ui: &mut egui::Ui, s: &mut UiState) {
    let t = &mut s.cave_tool;
    let Some(c) = t.selected.and_then(|id| s.caves.get(id)).cloned() else {
        dim(
            ui,
            "Pick a cave (Select, or its name on the left) to change it, or add one with the tools on the left.",
        );
        if s.cave_world.busy() {
            hint_small(ui, "Building rock…");
        }
        return;
    };
    let id = c.id;
    let arch = c.kind == CaveKind::Arch;
    hint_small(ui, &summary(&c));
    if s.cave_world.busy() {
        hint_small(ui, "Building rock…");
    }
    // name
    let buf = match &mut t.name_edit {
        Some((i, b)) if *i == id => b,
        _ => {
            t.name_edit = Some((id, c.name.clone()));
            &mut t.name_edit.as_mut().unwrap().1
        }
    };
    let r = text_field(ui, "Name", buf, "Name");
    let renamed = (r.lost_focus() && *buf != c.name && !buf.trim().is_empty())
        .then(|| buf.trim().to_string());
    if let Some(n) = renamed {
        t.requests.push(Request::Name(id, n));
    }
    ui.add_space(2.0);
    // the main actions
    let (lbl, ic, tip_text) = if c.up {
        ("Up", "ArrowUp", "A ramp up to a new chamber a level higher")
    } else {
        (
            "Down",
            "ArrowDown",
            "A ramp down to a new chamber a level lower",
        )
    };
    let grow = (!arch).then_some(("PlusMedium", "Grow", "A new tunnel and chamber from the deepest chamber, on away from the entrance (or click a + in the view)"));
    match button_row(
        ui,
        &[
            Some(("Shuffle", "Shuffle", "A new layout (locked chambers stay)")),
            grow,
            (!arch).then_some((ic, lbl, tip_text)),
        ],
    ) {
        Some(0) => t.requests.push(Request::Shuffle(id)),
        Some(1) => t.requests.push(Request::Extend {
            id,
            from: None,
            heading: None,
            down: false,
        }),
        Some(2) => t.requests.push(Request::Extend {
            id,
            from: None,
            heading: None,
            down: true,
        }),
        _ => {}
    }
    if !arch {
        section_fold(ui, "cave_insp_recipe", "Recipe", "", |ui| {
            dropdown_row(ui, "What", "cave_insp_kind", c.kind.name(), |ui| {
                for k in KINDS {
                    if menu_row(ui, kind_icon(k), k.name(), "", c.kind == k)
                        .on_hover_text(k.hint())
                        .clicked()
                        && c.kind != k
                    {
                        t.requests.push(Request::Kind(id, k));
                    }
                }
            });
            ui.add_space(2.0);
            let mut size = c.size;
            if segmented(ui, &mut size, &CaveSize::ALL.map(|s| (s, s.name()))) {
                t.requests.push(Request::Size(id, size));
            }
            size_hint(ui, c.size);
            ui.add_space(2.0);
            let (mut lv, mut up) = (c.levels, c.up);
            let (lc, uc) = levels_ui(ui, &mut lv, &mut up);
            if lc {
                t.requests.push(Request::Levels(id, lv));
            }
            if uc {
                t.requests.push(Request::Up(id, up));
            }
        });
    }
    section_fold(ui, "cave_insp_look", "Look", "", |ui| {
        let mut th = c.theme;
        if theme_combo(ui, "cave_theme", &mut th, Some(c.theme())) {
            t.requests.push(Request::Theme(id, th));
        }
        if !arch {
            ui.add_space(2.0);
            labelled(ui, "Inside", |ui| {
                let mut d = c.darkness;
                if segmented(ui, &mut d, &Darkness::ALL.map(|d| (d, d.name()))) {
                    t.requests.push(Request::Darkness(id, d));
                }
            });
            let mut f = c.features;
            if features_ui(ui, &mut f, c.theme()) {
                t.requests.push(Request::Features(id, f));
            }
        }
    });
    if !arch {
        // chambers (lock the ones to keep), in the order a visitor reaches them
        section_fold(
            ui,
            "cave_insp_chambers",
            "Chambers",
            &c.chambers.len().to_string(),
            |ui| {
                hint_small(
                    ui,
                    "Lock the chambers you like: Shuffle lays the rest out round them.",
                );
                let depth: std::collections::HashMap<u16, f32> =
                    c.path_lengths().into_iter().collect();
                let mut rows: Vec<&Chamber> = c.chambers.iter().collect();
                rows.sort_by(|a, b| {
                    depth
                        .get(&a.id)
                        .unwrap_or(&0.0)
                        .total_cmp(depth.get(&b.id).unwrap_or(&0.0))
                });
                ui.spacing_mut().item_spacing.y = 2.0;
                for (n, ch) in rows.into_iter().enumerate() {
                    let (cam, grow, lock) = chamber_row(ui, n + 1, ch);
                    if cam {
                        t.requests.push(Request::ViewChamber(id, ch.id));
                    }
                    if grow {
                        t.requests.push(Request::Extend {
                            id,
                            from: Some(ch.id),
                            heading: None,
                            down: false,
                        });
                    }
                    if lock {
                        t.requests.push(Request::Lock(id, ch.id, !ch.locked));
                    }
                    // its pool's lip: round the styles with the arrows (Auto: picked for it)
                    if ch.pool && super::dress::pool_of(&c, ch).is_some() {
                        ui.horizontal(|ui| {
                        ui.add_space(26.0);
                        ui.label(RichText::new("Pool rim").size(12.0).color(TEXT_DIM));
                        let shown = if ch.rim == RimStyle::Auto { format!("Auto · {}", c.rim_of(ch).name()) } else { ch.rim.name().to_string() };
                        if icon_button(ui, "ChevronLeftSmall", "The previous style").clicked() {
                            t.requests.push(Request::Rim(id, ch.id, ch.rim.step(-1)));
                        }
                        ui.label(RichText::new(shown).size(12.0).color(TEXT));
                        if icon_button(ui, "ChevronRightSmall", "The next style: rimstone, scalloped gours, terraces, boulders, a flowstone shelf, a crystal crust").clicked() {
                            t.requests.push(Request::Rim(id, ch.id, ch.rim.step(1)));
                        }
                    });
                    }
                }
            },
        );
    }
    // look into it, go in
    section_fold(
        ui,
        "cave_insp_inside",
        if arch { "See it" } else { "Look inside" },
        "",
        |ui| {
            let mut xr = s.cave_world.xray.is_some();
            let roof = c
                .chambers
                .iter()
                .map(|c| c.pos[1] + c.size[1])
                .fold(c.entrance[1], f32::max);
            let floor = c
                .chambers
                .iter()
                .map(|c| c.pos[1])
                .fold(c.entrance[1], f32::min);
            if toggle(ui, &mut xr, "X-ray: look down into it")
                .on_hover_text(
                    "The ground over the caves opens and everything above the cut is hidden",
                )
                .changed()
            {
                s.cave_world.xray = xr.then_some(roof - 0.5);
            }
            if let Some(h) = &mut s.cave_world.xray {
                let mut v = *h;
                if slider_row(ui, "Cut at", &mut v, (floor + 1.0)..=(roof + 2.0), |v| {
                    format!("{v:.0} m")
                })
                .changed()
                {
                    *h = v;
                }
                // jump to a level
                let mut levels: Vec<(u8, f32)> = vec![];
                for ch in &c.chambers {
                    let top = ch.pos[1] + ch.size[1] - 0.5;
                    match levels.iter_mut().find(|l| l.0 == ch.level) {
                        Some(l) => l.1 = l.1.max(top),
                        None => levels.push((ch.level, top)),
                    }
                }
                levels.sort_by_key(|l| l.0);
                if levels.len() > 1 {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                        for (l, top) in levels {
                            if small_button(ui, &format!("Level {}", l + 1))
                                .on_hover_text("Cut just under this level's roof")
                                .clicked()
                            {
                                *h = top;
                            }
                        }
                    });
                }
            }
            if !arch {
                let walk = c.tunnels.iter().any(|tn| tn.a == OUTSIDE).then_some((
                    "Footsteps",
                    "Walk in",
                    "Explore it on foot (Esc returns)",
                ));
                match button_row(
                    ui,
                    &[
                        walk,
                        Some((
                            "Camera1",
                            "Look inside",
                            "Put the camera in its first chamber",
                        )),
                    ],
                ) {
                    Some(0) => t.requests.push(Request::WalkIn(id)),
                    Some(1) => t.requests.push(Request::LookInside(id)),
                    _ => {}
                }
                if wide_icon_button(ui, "Package", "Decorate inside").on_hover_text("The camera goes into the first chamber and Objects › Place opens: click the floor or a wall to put things there").clicked() {
                t.requests.push(Request::Decorate(id));
            }
                toggle(ui, &mut s.work_light.on, "Work light (B)").on_hover_text("A lamp over where you're working and a softer one behind the camera, while editing only: dark caves can be decorated. Walk shows the real darkness.");
            }
        },
    );
    if !arch {
        // the dungeon
        section_fold(ui, "cave_insp_dungeon", "Dungeon", "", |ui| {
            let mut on = c.dungeon.is_some();
            if toggle(ui, &mut on, "A dungeon through a portal at the end").changed() {
                t.requests.push(Request::Dungeon(id, on.then(String::new)));
            }
            if let Some(dg) = &c.dungeon {
                hint_small(
                    ui,
                    "A liquid portal fills a doorway in the deepest chamber. In the game it opens the dungeon's own map; build that map with Open dungeon builder (it's also in the Dungeons tab).",
                );
                let buf = match &mut t.dungeon_edit {
                    Some((i, b)) if *i == id => b,
                    _ => {
                        t.dungeon_edit = Some((id, dg.name.clone()));
                        &mut t.dungeon_edit.as_mut().unwrap().1
                    }
                };
                let r = text_field(ui, "Dungeon name", buf, "Dungeon name");
                let renamed = (r.lost_focus() && *buf != dg.name && !buf.trim().is_empty())
                    .then(|| buf.trim().to_string());
                if let Some(n) = renamed {
                    t.requests.push(Request::Dungeon(id, Some(n)));
                }
                ui.add_space(2.0);
                let mut inst = dg.instancing;
                if segmented(ui, &mut inst, &Instancing::ALL.map(|i| (i, i.name()))) {
                    t.requests.push(Request::Instancing(id, inst));
                }
                ui.add_space(2.0);
                match s.dungeons.by_cave(id) {
                    Some((did, _)) => {
                        if wide_icon_button(ui, "Castle", "Open dungeon builder").on_hover_text("Save the world and open this dungeon's own map to build it (Back to world returns)").clicked() {
                            s.dungeon_tool.requests.push(crate::dungeons::ui::Request::Open(did));
                        }
                    }
                    None => hint_small(ui, "Its dungeon is being set up…"),
                }
            }
        });
    }
    ui.add_space(6.0);
    if danger_button(ui, "TrashCan", "Remove")
        .on_hover_text(if arch {
            "Remove this arch (undoable)"
        } else {
            "Remove this cave (undoable)"
        })
        .clicked()
    {
        t.requests.push(Request::Remove(id));
    }
}

// ------------------------------------------------------------------ viewport

/// Where the cursor meets the ground or a cave's rock (the pointer is caves-aware:
/// `CaveWorld::pick`).
pub(super) fn pick(
    pointer: Res<Pointer>,
    ed: Res<crate::editor::Editor>,
    mut tool: ResMut<CaveTool>,
) {
    tool.hit = None;
    if ed.tab != Tab::Caves || pointer.over_ui {
        return;
    }
    tool.hit = pointer.hit;
}

/// "+" handles of a cave: one past each dead-end chamber, pointing on away from its tunnel.
pub fn handles(c: &Cave) -> Vec<(u16, Vec3, f32)> {
    let mut out = vec![];
    for ch in &c.chambers {
        let links: Vec<&Tunnel> = c
            .tunnels
            .iter()
            .filter(|t| t.a == ch.id || t.b == ch.id)
            .collect();
        if links.len() != 1 {
            continue;
        }
        let t = links[0];
        let pts: Vec<[f32; 3]> = if t.b == ch.id {
            t.pts.clone()
        } else {
            t.pts.iter().rev().copied().collect()
        };
        let Some(prev) = pts
            .iter()
            .rev()
            .find(|p| Vec2::new(p[0] - ch.pos[0], p[2] - ch.pos[2]).length() > 1.5)
        else {
            continue;
        };
        let away = Vec2::new(ch.pos[0] - prev[0], ch.pos[2] - prev[2]).normalize_or(Vec2::Y);
        let r = ch.size[0].min(ch.size[2]) * 0.8;
        let at = Vec3::new(
            ch.pos[0] + away.x * r,
            ch.pos[1] + 1.2,
            ch.pos[2] + away.y * r,
        );
        out.push((ch.id, at, away.x.atan2(away.y)));
    }
    out
}

fn handle_under(
    c: &Cave,
    screen: Vec2,
    cam: &Camera,
    cam_t: &GlobalTransform,
) -> Option<(u16, f32)> {
    handles(c)
        .into_iter()
        .filter_map(|(id, p, hd)| {
            cam.world_to_viewport(cam_t, p)
                .ok()
                .map(|v| (v.distance(screen), id, hd))
        })
        .filter(|(d, ..)| *d < 18.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id, hd)| (id, hd))
}

/// Clicks and drags in the viewport.
#[allow(clippy::too_many_arguments)]
pub(super) fn viewport(
    stroke: Res<crate::tools::Stroke>,
    pointer: Res<Pointer>,
    mut tool: ResMut<CaveTool>,
    cam: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut d: WorldData,
) {
    if d.ed.tab != Tab::Caves {
        return;
    }
    let Ok((camera, cam_t)) = cam.single() else {
        return;
    };
    let eye = cam_t.translation();
    if stroke.just_started {
        tool.drag_start = tool.hit;
        tool.dig_path.clear();
        tool.pressed_handle = match (tool.selected.and_then(|id| d.caves.get(id)), pointer.screen) {
            (Some(c), Some(sc)) => handle_under(c, sc, camera, cam_t),
            _ => None,
        };
    }
    // a "+" handle grows the selected cave that way
    if stroke.just_ended
        && let Some((from, hd)) = tool.pressed_handle.take()
    {
        if let Some(id) = tool.selected {
            match edit::extend(&mut d, id, Some(from), Some(hd), false) {
                Ok(_) => d.ed.toast("Grew the cave: a new tunnel and chamber"),
                Err(e) => d.ed.toast(e),
            }
        }
        return;
    }
    match tool.tool {
        Tool::Add | Tool::Arch => {
            if !stroke.just_ended {
                return;
            }
            let (Some(start), Some(end)) = (tool.drag_start, tool.hit) else {
                d.ed.toast("Click the ground where it should be");
                return;
            };
            let dragged = start.xz().distance(end.xz());
            let r = &tool.recipe;
            let n = if tool.tool == Tool::Arch {
                NewCave {
                    kind: CaveKind::Arch,
                    theme: r.theme,
                    at: start.xz(),
                    toward: eye.xz(),
                    span_to: (dragged > 4.0).then_some(end.xz()),
                    ..default()
                }
            } else {
                if dragged > 6.0 {
                    // (a drag pans or orbits; only a click places)
                    return;
                }
                let mut f = r.features;
                if r.theme == Theme::Crystal {
                    f.crystals = true;
                }
                NewCave {
                    kind: r.kind,
                    size: r.size,
                    levels: r.levels,
                    up: r.up,
                    theme: r.theme,
                    darkness: r.darkness,
                    features: f,
                    at: end.xz(),
                    toward: eye.xz(),
                    dungeon: r.dungeon.then(String::new),
                    ..default()
                }
            };
            match edit::create(&mut d, &n) {
                Ok(id) => {
                    tool.selected = Some(id);
                    tool.name_edit = None;
                    tool.dungeon_edit = None;
                    let c = d.caves.get(id).unwrap();
                    let msg = if c.kind == CaveKind::Arch {
                        format!("{} added — Shuffle for another shape", c.name)
                    } else {
                        format!(
                            "{} added: {} chambers — Shuffle re-rolls it, + grows it",
                            c.name,
                            c.chambers.len()
                        )
                    };
                    d.ed.toast(msg);
                }
                Err(e) => d.ed.toast(e),
            }
        }
        Tool::Dig | Tool::Fill => {
            if stroke.active
                && let Some(h) = tool.hit
                && tool
                    .dig_path
                    .last()
                    .is_none_or(|l| l.distance(h) > tool.dig_radius * 0.5)
            {
                tool.dig_path.push(h);
            }
            if stroke.just_ended && !tool.dig_path.is_empty() {
                let pts = std::mem::take(&mut tool.dig_path);
                let fill = tool.tool == Tool::Fill;
                if let Err(e) = edit::dig(&mut d, pts, tool.dig_radius, fill) {
                    d.ed.toast(e);
                }
            }
        }
        Tool::Select => {
            if !stroke.just_ended {
                return;
            }
            let Some(h) = tool.hit else { return };
            // the cave whose entrance (or rock) is nearest the click
            let best = d
                .caves
                .list
                .iter()
                .map(|c| {
                    let (lo, hi) = c.bounds();
                    let inside = h.x >= lo.x - 4.0
                        && h.x <= hi.x + 4.0
                        && h.z >= lo.z - 4.0
                        && h.z <= hi.z + 4.0;
                    let dist =
                        Vec3::from_array(c.entrance).distance(h) - if inside { 20.0 } else { 0.0 };
                    (dist, c.id)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match best {
                Some((dist, id)) if dist < 30.0 => {
                    tool.selected = Some(id);
                    tool.name_edit = None;
                    tool.dungeon_edit = None;
                }
                _ => tool.selected = None,
            }
        }
    }
}

/// Makes the changes the panels asked for.
pub(super) fn apply_requests(
    mut tool: ResMut<CaveTool>,
    mut d: WorldData,
    mut walk: ResMut<crate::walk::WalkState>,
    mut cw: ResMut<CaveWorld>,
) {
    if tool.requests.is_empty() {
        return;
    }
    for r in std::mem::take(&mut tool.requests) {
        let res: Result<(), String> = match r {
            Request::Kind(id, k) => edit::update(&mut d, id, "Change cave kind", |c| {
                std::mem::replace(&mut c.kind, k) != k
            }),
            Request::Size(id, v) => edit::update(&mut d, id, "Change cave size", |c| {
                std::mem::replace(&mut c.size, v) != v
            }),
            Request::Levels(id, v) => edit::update(&mut d, id, "Change cave levels", |c| {
                std::mem::replace(&mut c.levels, v) != v
            }),
            Request::Up(id, v) => edit::update(&mut d, id, "Change cave levels", |c| {
                std::mem::replace(&mut c.up, v) != v
            }),
            Request::Rim(id, ch, v) => edit::update(&mut d, id, "Change pool rim", |c| {
                c.chambers
                    .iter_mut()
                    .find(|x| x.id == ch)
                    .is_some_and(|x| std::mem::replace(&mut x.rim, v) != v)
            }),
            Request::Theme(id, v) => edit::update(&mut d, id, "Change cave look", |c| {
                std::mem::replace(&mut c.theme, v) != v
            }),
            Request::Darkness(id, v) => edit::update(&mut d, id, "Change cave darkness", |c| {
                std::mem::replace(&mut c.darkness, v) != v
            }),
            Request::Features(id, v) => edit::update(&mut d, id, "Change cave features", |c| {
                std::mem::replace(&mut c.features, v) != v
            }),
            Request::Name(id, n) => edit::update(&mut d, id, "Rename cave", |c| {
                std::mem::replace(&mut c.name, n.clone()) != n
            }),
            Request::Dungeon(id, n) => edit::update(&mut d, id, "Cave dungeon", |c| {
                let before = c.dungeon.clone();
                match n {
                    None => c.dungeon = None,
                    Some(n) => {
                        let name = if n.is_empty() {
                            format!("{} Depths", c.name)
                        } else {
                            n
                        };
                        match &mut c.dungeon {
                            Some(dg) => dg.name = name,
                            None => {
                                c.dungeon = Some(Dungeon {
                                    name,
                                    chamber: 0,
                                    door: [0.0; 3],
                                    door_yaw: 0.0,
                                    instancing: Instancing::Party,
                                })
                            }
                        }
                    }
                }
                c.dungeon != before
            }),
            Request::Instancing(id, i) => {
                edit::update(&mut d, id, "Cave dungeon", |c| match &mut c.dungeon {
                    Some(dg) if dg.instancing != i => {
                        dg.instancing = i;
                        true
                    }
                    _ => false,
                })
            }
            Request::Shuffle(id) => edit::shuffle(&mut d, id, None).map(|_| ()),
            Request::Extend {
                id,
                from,
                heading,
                down,
            } => edit::extend(&mut d, id, from, heading, down).map(|_| ()),
            Request::Lock(id, ch, on) => edit::lock(&mut d, id, ch, on),
            Request::Remove(id) => {
                tool.selected = None;
                edit::remove(&mut d, id)
            }
            Request::WalkIn(id) => match d.caves.get(id).and_then(walk_in_point) {
                Some((p, yaw)) => {
                    cw.xray = None;
                    walk.request = Some(crate::walk::WalkStart {
                        pos: Some(p),
                        yaw: Some(yaw),
                        mode: None,
                        playtest: false,
                    });
                    Ok(())
                }
                None => Err("this cave has no way in".into()),
            },
            Request::LookInside(id) | Request::Decorate(id) => match d
                .caves
                .get(id)
                .and_then(|c| first_chamber(c).map(|ch| (c.clone(), ch.id)))
            {
                Some((c, ch)) => {
                    cw.xray = None;
                    view_chamber(&mut d.orbit, &cw, &c, ch);
                    if matches!(r, Request::Decorate(_)) {
                        d.ed.tab = Tab::Objects;
                        d.ed.object_tool = crate::editor::ObjectTool::Place;
                        d.ed.toast(format!("Inside {}: pick something on the left, then click the floor or a wall · ◀ ▶ at the top go chamber to chamber", c.name));
                    }
                    Ok(())
                }
                None => Err("this cave has no chambers".into()),
            },
            Request::ViewChamber(id, ch) => match d.caves.get(id).cloned() {
                Some(c) if c.chamber(ch).is_some() => {
                    cw.xray = None;
                    view_chamber(&mut d.orbit, &cw, &c, ch);
                    Ok(())
                }
                _ => Err("that chamber isn't there any more".into()),
            },
            Request::Leave(id) => match d.caves.get(id) {
                Some(c) => {
                    view_outside(&mut d.orbit, c);
                    Ok(())
                }
                None => Ok(()),
            },
            Request::WalkHere => {
                let (p, yaw) = (d.orbit.focus, d.orbit.yaw);
                cw.xray = None;
                walk.request = Some(crate::walk::WalkStart {
                    pos: Some(p + Vec3::Y * 1.0),
                    yaw: Some(yaw),
                    mode: None,
                    playtest: false,
                });
                Ok(())
            }
        };
        if let Err(e) = res {
            d.ed.toast(e);
        }
    }
}

/// A spot just inside the way in, looking in.
pub fn walk_in_point(c: &Cave) -> Option<(Vec3, f32)> {
    let t = c.tunnels.iter().find(|t| t.a == OUTSIDE)?;
    let pts: Vec<Vec3> = t.pts.iter().map(|p| Vec3::from_array(*p)).collect();
    // a few metres in past the mouth
    let e = Vec3::from_array(c.entrance);
    let p = pts
        .iter()
        .find(|p| Vec2::new(p.x - e.x, p.z - e.z).length() > 4.0)
        .copied()
        .or(pts.last().copied())?;
    let d = Vec2::new(p.x - e.x, p.z - e.z).normalize_or(layout::dir(c.heading));
    // (walk-mode yaw looks along (−sin, −cos))
    Some((p + Vec3::Y * 1.0, (-d.x).atan2(-d.y)))
}

// ------------------------------------------------------------------ the camera in a cave

/// The chamber nearest the way in (the first a visitor reaches).
pub fn first_chamber(c: &Cave) -> Option<&Chamber> {
    let depth: std::collections::HashMap<u16, f32> = c.path_lengths().into_iter().collect();
    c.chambers.iter().min_by(|a, b| {
        depth
            .get(&a.id)
            .unwrap_or(&f32::MAX)
            .total_cmp(depth.get(&b.id).unwrap_or(&f32::MAX))
    })
}

/// The chambers in the order a visitor reaches them.
pub fn chamber_order(c: &Cave) -> Vec<u16> {
    let depth: std::collections::HashMap<u16, f32> = c.path_lengths().into_iter().collect();
    let mut ids: Vec<u16> = c.chambers.iter().map(|ch| ch.id).collect();
    ids.sort_by(|a, b| {
        depth
            .get(a)
            .unwrap_or(&f32::MAX)
            .total_cmp(depth.get(b).unwrap_or(&f32::MAX))
    });
    ids
}

/// Puts the camera in chamber `ch` of cave `c`: turning round a spot on its floor near the middle,
/// from the side with the longest view across it (the rock as built; the chamber's size before
/// it is), at about eye height.
pub fn view_chamber(orbit: &mut OrbitCamera, cw: &CaveWorld, c: &Cave, ch: u16) {
    let Some(ch) = c.chamber(ch) else { return };
    let mid = ch.center();
    // a spot on the floor near the middle (a pool or a stalagmite may stand right there)
    let ring = (0..=16).map(|k| {
        if k == 0 {
            Vec2::ZERO
        } else {
            Vec2::from_angle(k as f32 * 0.785) * (1.5 + (k / 8) as f32 * 2.0)
        }
    });
    let focus = ring
        .map(|o| mid + Vec3::new(o.x, 0.0, o.y))
        .find_map(|p| {
            cw.floor_under(p + Vec3::Y * 1.2, 6.0)
                .map(|y| Vec3::new(p.x, y, p.z))
        })
        .unwrap_or(mid);
    // without the rock yet: from the way in, looking in
    let e = Vec3::from_array(c.entrance);
    let back = Vec2::new(e.x - mid.x, e.z - mid.z).normalize_or(Vec2::Y);
    let (mut yaw, mut dist) = (
        back.x.atan2(back.y),
        (ch.size[0].min(ch.size[2]) * 0.8).max(3.0),
    );
    if cw.volume.is_some() {
        let pivot = focus + Vec3::Y * crate::camera::CAVE_LIFT;
        let mut longest = 0.0;
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            let free = cw
                .raycast(pivot, OrbitCamera::offset_of(a, 0.2, 1.0), 120.0)
                .map_or(120.0, |(_, t)| t);
            if free > longest {
                (longest, yaw) = (free, a);
            }
        }
        dist = (longest * 0.8).clamp(2.0, ch.radius() * 1.6 + 4.0);
    }
    orbit.set_view(focus, yaw, 0.2, dist);
    orbit.snap();
}

/// Puts the camera outside the cave's way in, looking at it.
pub fn view_outside(orbit: &mut OrbitCamera, c: &Cave) {
    let e = Vec3::from_array(c.entrance);
    let dir = layout::dir(c.heading);
    // (the cave runs in along `dir`: the camera stands back the other way)
    orbit.set_view(e, (-dir.x).atan2(-dir.y), 0.35, 28.0);
    orbit.snap();
}

/// The cave and chamber a spot on a cave's floor is in: (cave id, chamber id).
pub fn locate(caves: &Caves, cw: &CaveWorld, p: Vec3) -> Option<(u32, u16)> {
    let up = p + Vec3::Y * 1.2;
    let c = cw
        .owner_at(up)
        .and_then(|i| caves.list.get(i))
        .or_else(|| {
            caves.list.iter().find(|c| {
                let (lo, hi) = c.bounds();
                up.cmpge(lo - Vec3::splat(2.0)).all() && up.cmple(hi + Vec3::splat(2.0)).all()
            })
        })?;
    // the chamber whose floor this is most within (tunnels: the nearest)
    let ch = c.chambers.iter().min_by(|a, b| {
        let f = |ch: &Chamber| {
            (ch.center() - p).xz().length() / ch.radius().max(1.0) + (ch.pos[1] - p.y).abs() * 0.15
        };
        f(a).total_cmp(&f(b))
    })?;
    Some((c.id, ch.id))
}

/// The capsule over the view while the camera is in a cave (top-centre, under the islands): which
/// cave and chamber, back and on through its chambers, decorate, work light, walk, leave; and how
/// the camera moves in here.
pub fn inside_bar(ctx: &egui::Context, s: &mut UiState, screen: egui::Rect) {
    if !s.orbit.in_cave() {
        return;
    }
    let Some((id, ch)) = locate(&s.caves, &s.cave_world, s.orbit.focus) else {
        return;
    };
    let Some(c) = s.caves.get(id) else { return };
    let order = chamber_order(c);
    let at = order.iter().position(|&x| x == ch).unwrap_or(0);
    let n = order.len();
    let name = c.name.clone();
    let level = c.chamber(ch).map(|x| x.level + 1).unwrap_or(1);
    let levels = c.chambers.iter().map(|x| x.level).max().unwrap_or(0) + 1;
    let decorating = s.ed.tab == Tab::Objects;
    let mut req = vec![];
    // centred over the view between the tool panel and the inspector
    let inspector = ctx.memory(|m| {
        m.areas().is_visible(&egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("right_panel"),
        ))
    });
    let cx = crate::ui::shell::dock_center_x(ctx, screen, inspector);
    let pos = egui::pos2(cx, screen.min.y + crate::ui::shell::TOP);
    egui::Area::new(egui::Id::new("cave_inside_bar")).order(egui::Order::Middle).pivot(egui::Align2::CENTER_TOP).fixed_pos(pos).show(ctx, |ui| {
        glass(ui, glass_frame().corner_radius(CornerRadius::same(R_PANEL)).inner_margin(egui::Margin { left: 12, right: 8, top: 6, bottom: 7 }), |ui| {
            // where: the cave and chamber, back and on through its chambers; what to do here
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.set_min_height(30.0);
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 28.0), egui::Sense::hover());
                paint_icon(ui, r.center(), "Shovel", 15.0, TEXT);
                ui.label(RichText::new(&name).font(FontId::new(13.5, semibold())).color(TEXT));
                let where_ = if levels > 1 { format!("Chamber {} of {n} · level {level}", at + 1) } else { format!("Chamber {} of {n}", at + 1) };
                ui.label(RichText::new(where_).font(FontId::new(11.5, mono())).color(TEXT_DIM));
                bar_sep(ui);
                if ui.add_enabled_ui(at > 0, |ui| icon_button(ui, "ChevronLeftSmall", "Back: the chamber before, towards the way in")).inner.clicked() {
                    req.push(Request::ViewChamber(id, order[at - 1]));
                }
                if ui.add_enabled_ui(at + 1 < n, |ui| icon_button(ui, "ChevronRightSmall", "Next: the next chamber in")).inner.clicked() {
                    req.push(Request::ViewChamber(id, order[at + 1]));
                }
                bar_sep(ui);
                if !decorating && small_icon_button(ui, "Package", "Decorate").on_hover_text("Objects › Place: click the floor or a wall to put things here").clicked() {
                    req.push(Request::Decorate(id));
                }
                let on = s.work_light.on;
                if icon_toggle(ui, "LightBulb", "Work light (B)", on)
                    .on_hover_text("A lamp over where you're working and a softer one behind the camera, while editing only — Walk shows the cave's real darkness")
                    .clicked()
                {
                    s.work_light.on = !on;
                }
                if small_icon_button(ui, "Footsteps", "Walk").on_hover_text("Walk on from here (Esc returns)").clicked() {
                    req.push(Request::WalkHere);
                }
                if small_icon_button(ui, "ArrowOutOfBox", "Leave").on_hover_text("Back outside, in front of the way in").clicked() {
                    req.push(Request::Leave(id));
                }
            });
            let (how, more) = if decorating && s.ed.object_tool == crate::editor::ObjectTool::Place {
                ("Click the floor or a wall to place · WASD walk · right-drag look around", "A click on a wall or the roof sets the object on the floor in front of it; with Align to terrain slope on (or for things that always follow the slope) it sticks to the rock instead.")
            } else {
                ("WASD walk the floor · right-drag look around · scroll zoom · F jump to the cursor", "The camera stays inside the cave: it slides along the walls and keeps in front of the rock. Leave (or walking out of the mouth) takes it back outside.")
            };
            ui.add(egui::Label::new(RichText::new(how).size(11.5).color(TEXT_MUTE)).wrap()).on_hover_text(more);
        });
    });
    s.cave_tool.requests.extend(req);
}

/// A thin divider inside the in-cave capsule.
fn bar_sep(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 20.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, LINE);
}

// ------------------------------------------------------------------ drawing

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct CaveGizmos;

pub(super) fn setup_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = store.config_mut::<CaveGizmos>();
    // (always on top: the chambers are under the ground)
    cfg.depth_bias = -1.0;
    cfg.line.width = 2.5;
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    ed: Res<crate::editor::Editor>,
    tool: Res<CaveTool>,
    caves: Res<Caves>,
    terrain: Res<crate::terrain::Terrain>,
    stroke: Res<crate::tools::Stroke>,
    pointer: Res<Pointer>,
    walk: Res<crate::walk::WalkState>,
    cam: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut gizmos: Gizmos<CaveGizmos>,
) {
    if ed.tab != Tab::Caves || walk.active {
        return;
    }
    let flat = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    let gold = Color::srgb(1.0, 0.82, 0.35);
    let blue = Color::srgb(0.45, 0.8, 1.0);
    // entrances
    for c in &caves.list {
        let e = Vec3::from_array(c.entrance);
        let sel = tool.selected == Some(c.id);
        let col = if sel {
            gold
        } else {
            Color::srgba(1.0, 1.0, 1.0, 0.55)
        };
        gizmos.circle(
            Isometry3d::new(e + Vec3::Y * 0.3, flat),
            if sel { 3.5 } else { 2.5 },
            col,
        );
        gizmos.line(e + Vec3::Y * 0.3, e + Vec3::Y * 7.0, col);
    }
    // the selected cave: its chambers' floors and the "+" handles
    if let Some(c) = tool.selected.and_then(|id| caves.get(id)) {
        let hovered = match (cam.single(), pointer.screen) {
            (Ok((camera, cam_t)), Some(sc)) => handle_under(c, sc, camera, cam_t),
            _ => None,
        };
        for ch in &c.chambers {
            let p = ch.center() + Vec3::Y * 0.25;
            let col = if ch.locked {
                gold.with_alpha(0.8)
            } else {
                blue.with_alpha(0.45)
            };
            let (s, co) = ch.yaw.sin_cos();
            let pts: Vec<Vec3> = (0..=40)
                .map(|k| {
                    let a = k as f32 / 40.0 * std::f32::consts::TAU;
                    let l = Vec2::new(a.cos() * ch.size[0] * 0.8, a.sin() * ch.size[2] * 0.8);
                    p + Vec3::new(l.x * co + l.y * s, 0.0, -l.x * s + l.y * co)
                })
                .collect();
            gizmos.linestrip(pts, col);
        }
        for (id, at, _) in handles(c) {
            let hot = hovered.is_some_and(|h| h.0 == id);
            let col = if hot {
                gold
            } else {
                Color::srgb(0.55, 1.0, 0.6)
            };
            let r = if hot { 1.6 } else { 1.2 };
            gizmos.circle(Isometry3d::new(at, flat), r, col);
            gizmos.line(at - Vec3::X * r * 0.6, at + Vec3::X * r * 0.6, col);
            gizmos.line(at - Vec3::Z * r * 0.6, at + Vec3::Z * r * 0.6, col);
        }
    }
    // what a click would do
    let Some(h) = tool.hit else { return };
    match tool.tool {
        Tool::Add if !stroke.active => {
            let eye = cam
                .single()
                .map(|(_, t)| t.translation().xz())
                .unwrap_or(h.xz());
            let (hd, slope) = layout::uphill(&terrain, h.xz(), eye);
            let dir = layout::dir(hd);
            let flat_ground = slope < 0.22 || tool.recipe.kind == CaveKind::Pit;
            if flat_ground && tool.recipe.kind != CaveKind::Cavern {
                // a pit: its rim
                gizmos.circle(Isometry3d::new(h + Vec3::Y * 0.4, flat), 9.0, gold);
            } else {
                gizmos.circle(Isometry3d::new(h + Vec3::Y * 0.3, flat), 3.0, gold);
                // which way it runs
                let a = h + Vec3::Y * 1.0;
                let b = a + Vec3::new(dir.x, 0.0, dir.y) * 14.0;
                gizmos.arrow(a, b, gold);
            }
        }
        Tool::Arch => {
            if stroke.active
                && let Some(s) = tool.drag_start
            {
                gizmos.line(s + Vec3::Y * 0.5, h + Vec3::Y * 0.5, gold);
                gizmos.circle(Isometry3d::new(s + Vec3::Y * 0.3, flat), 2.0, gold);
            }
            gizmos.circle(Isometry3d::new(h + Vec3::Y * 0.3, flat), 2.0, gold);
        }
        Tool::Dig | Tool::Fill => {
            let col = if tool.tool == Tool::Dig {
                Color::srgb(1.0, 0.55, 0.35)
            } else {
                Color::srgb(0.5, 0.85, 1.0)
            };
            for q in [
                Quat::IDENTITY,
                flat,
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ] {
                gizmos.circle(Isometry3d::new(h, q), tool.dig_radius, col);
            }
            if tool.dig_path.len() > 1 {
                gizmos.linestrip(tool.dig_path.clone(), col.with_alpha(0.6));
            }
        }
        _ => {}
    }
}

/// A label by the cursor saying what a click makes (and the first-click guide).
pub(super) fn cursor_label(
    mut contexts: bevy_egui::EguiContexts,
    ed: Res<crate::editor::Editor>,
    tool: Res<CaveTool>,
    pointer: Res<Pointer>,
    stroke: Res<crate::tools::Stroke>,
    terrain: Res<crate::terrain::Terrain>,
    walk: Res<crate::walk::WalkState>,
) -> Result {
    if ed.tab != Tab::Caves || walk.active || pointer.over_ui || stroke.active {
        return Ok(());
    }
    let (Some(screen), Some(h)) = (pointer.screen, tool.hit) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let text = match tool.tool {
        Tool::Add => {
            let (_, slope) = layout::uphill(&terrain, h.xz(), h.xz());
            let k = tool.recipe.kind;
            let what = match k {
                CaveKind::Cave if slope < 0.22 => "a sinkhole (flat ground)".to_string(),
                CaveKind::Mine if slope < 0.22 => "a mine decline (flat ground)".to_string(),
                CaveKind::Cave => "a cave mouth".to_string(),
                CaveKind::Mine => "a mine entrance".to_string(),
                k => format!("a {}", k.name().to_lowercase()),
            };
            format!(
                "Click: {what}\n{} · {} level{}",
                tool.recipe.size.name(),
                tool.recipe.levels,
                if tool.recipe.levels == 1 { "" } else { "s" }
            )
        }
        Tool::Arch => "Drag from foot to foot (or click)".to_string(),
        Tool::Dig => format!("Drag: dig ({:.1} m)", tool.dig_radius),
        Tool::Fill => format!("Drag: fill ({:.1} m)", tool.dig_radius),
        Tool::Select => "Click a cave".to_string(),
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("cave_tip"),
    ));
    let galley = painter.layout(text, egui::FontId::proportional(12.0), TEXT, 240.0);
    let pos = egui::pos2(screen.x + 22.0, screen.y + 18.0);
    let r = egui::Rect::from_min_size(pos, galley.size() + egui::vec2(18.0, 12.0));
    // a dark-glass tip, like the chrome's
    painter.rect(
        r,
        CornerRadius::same(9),
        GLASS_SOLID,
        egui::Stroke::new(1.0, LINE_STRONG),
        egui::StrokeKind::Inside,
    );
    painter.galley(pos + egui::vec2(9.0, 6.0), galley, TEXT);
    Ok(())
}

// ------------------------------------------------------------------ Halo hooks (inspector, dock)

/// The inspector shows the selected cave (titled with its name); nothing selected, no inspector.
pub fn caves_inspector(s: &UiState) -> Option<(String, bool)> {
    s.cave_tool
        .selected
        .and_then(|id| s.caves.get(id))
        .map(|c| (c.name.clone(), true))
}

/// The tool dock: the tool, the dig brush, what a click makes, the key hints (and how to move
/// with the camera in a cave).
pub fn caves_dock(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    let t = &mut s.cave_tool;
    let (_, icon, name) = CAVE_TOOLS
        .iter()
        .find(|c| c.0 == t.tool)
        .copied()
        .unwrap_or(CAVE_TOOLS[0]);
    dock_title(ui, icon, name);
    dock_sep(ui);
    let mut hints: Vec<(&str, &str)> = match t.tool {
        Tool::Add => {
            let r = &t.recipe;
            dock_text(
                ui,
                &format!(
                    "{} · {} · {} level{}",
                    r.kind.name(),
                    r.size.name(),
                    r.levels,
                    if r.levels == 1 { "" } else { "s" }
                ),
            );
            dock_sep(ui);
            vec![("Click", "Add it here"), ("+", "Grow the cave")]
        }
        Tool::Arch => vec![("Drag", "From one foot to the other")],
        Tool::Dig | Tool::Fill => {
            slider_mini(ui, "Radius", &mut t.dig_radius, 0.8..=8.0, |v| {
                format!("{v:.1} m")
            });
            dock_sep(ui);
            vec![(
                "Drag",
                if t.tool == Tool::Dig {
                    "Dig through rock"
                } else {
                    "Fill rock in"
                },
            )]
        }
        Tool::Select => vec![("Click", "Pick a cave")],
    };
    if t.tool != Tool::Add && t.selected.is_some() {
        hints.push(("+", "Grow the cave"));
    }
    if s.orbit.in_cave() {
        hints.push(("RMB", "Look around"));
        hints.push(("WASD", "Walk the floor"));
        hints.push((
            "B",
            if s.work_light.on {
                "Light off"
            } else {
                "Work light"
            },
        ));
    }
    dock_hints(ui, &hints);
    true
}
