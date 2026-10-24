//! The Characters tab: a codex of everything that lives in the world, in one big window instead
//! of the usual side panel.
//!
//! - Left: the codex. **Characters** (the models: imported, rigged here, or built in),
//!   **Player** (who Walk mode plays as), **NPCs** (the people placed in this world) and
//!   **Creatures** (the monster types spawn areas draw from), as cards with portraits.
//! - Middle: the stage, showing the selected one's character live (drag to turn, scroll to
//!   zoom), or straight-on with the body markers to drag while fixing a rig.
//! - Right: the inspector for what's selected (`view_char`: a model's rig, animations, moves and
//!   export; `view_codex`: an NPC's basics, behavior, talk and moves, a creature, the player).
//!
//! NPCs and creatures are the world's Gameplay records (`crate::gameplay`), so exports and
//! spawn areas keep working; they're managed here.

use std::sync::{Arc, Mutex};

use autorig::markers::{Marker, Markers};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, pos2, vec2};

use super::anims::Category;
use super::npc::NpcEdit;
use super::stage::{Stage, Thumbs, Want};
use super::{AnimLibrary, CharacterEvent, Characters, OnImport, WorldCharacters};
use crate::editor::{Editor, Tab};
use crate::gameplay::{Gameplay, NpcRole};
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// Adding an NPC: the next click on the ground stands `character` there.
#[derive(Clone, Debug, PartialEq)]
pub struct Placing {
    pub character: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Preview,
    Markers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Characters,
    Player,
    Npcs,
    Creatures,
}

/// What the inspector shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Sel {
    Character(String),
    Player,
    Npc(u32),
    Creature(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CharTab {
    #[default]
    Rig,
    Animations,
    Moves,
    Export,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NpcTab {
    #[default]
    Basics,
    Behavior,
    Talk,
    Moves,
}

#[derive(Resource, Default)]
pub struct CharacterView {
    pub section: Section,
    pub sel: Option<Sel>,
    /// the character on the stage (follows the selection)
    pub selected: Option<String>,
    pub mode: Mode,
    pub char_tab: CharTab,
    pub npc_tab: NpcTab,
    /// markers being edited: (character, markers)
    pub markers: Option<(String, Markers)>,
    pub(super) dragging: Option<Marker>,
    /// move one side's marker alone (off: its twin follows, mirrored)
    pub one_side: bool,
    pub placing: Option<Placing>,
    /// the window is put away to look at the world (an NPC, its route)
    pub looking: bool,
    pub(super) search: String,
    pub(super) category: Option<Category>,
    pub(super) filter: Filter,
    pub(super) anim_import: Option<Arc<Mutex<Option<Result<Vec<String>, String>>>>>,
    /// (message, error?, until) under the stage
    pub(super) note: Option<(String, bool, f64)>,
    /// a character just made: (id, until) its textures are prepared the first time it shows
    pub(super) fresh: Option<(String, f64)>,
    /// an edit of the Gameplay records is being recorded (one undo step a burst)
    pub(super) editing_until: f64,
}

/// The codex grid's filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Filter {
    #[default]
    All,
    Mine,
    Builtin,
    Role(NpcRole),
}

impl CharacterView {
    /// Selects a library character (and shows it).
    pub fn select_character(&mut self, id: &str) {
        self.section = Section::Characters;
        self.sel = Some(Sel::Character(id.to_string()));
        self.mode = Mode::Preview;
        self.markers = None;
    }
    pub fn note(&mut self, msg: impl Into<String>, err: bool, now: f64) {
        self.note = Some((msg.into(), err, now + if err { 8.0 } else { 4.0 }));
    }
}

/// Everything the window reads and edits.
#[derive(SystemParam)]
pub struct ViewData<'w> {
    pub ed: ResMut<'w, Editor>,
    pub v: ResMut<'w, CharacterView>,
    pub chars: ResMut<'w, Characters>,
    pub lib: ResMut<'w, AnimLibrary>,
    pub stage: ResMut<'w, Stage>,
    pub world: ResMut<'w, WorldCharacters>,
    pub meta: ResMut<'w, crate::world::WorldMeta>,
    pub gp: ResMut<'w, Gameplay>,
    pub thumbs: ResMut<'w, Thumbs>,
    pub npc_edit: ResMut<'w, NpcEdit>,
    pub history: ResMut<'w, crate::history::History>,
    pub store: Res<'w, crate::objects::ObjectStore>,
    pub orbit: ResMut<'w, crate::camera::OrbitCamera>,
    pub view3: ResMut<'w, super::player::PlayerView>,
    pub lighting: Res<'w, crate::sky::Lighting>,
    pub walk: ResMut<'w, crate::walk::WalkState>,
}

impl ViewData<'_> {
    /// Applies an edit to the Gameplay records as one undo step per burst of edits (a slider
    /// dragged, a name typed).
    pub fn gp_edit(&mut self, now: f64, label: &str, f: impl FnOnce(&mut Gameplay)) {
        if now > self.v.editing_until {
            self.history.begin(label);
            self.history.touch_gameplay(&self.gp);
            f(&mut self.gp);
            self.history.commit(&self.store);
        } else {
            f(&mut self.gp);
        }
        self.v.editing_until = now + 0.8;
        self.gp.dirty = true;
        self.meta.dirty = true;
    }
}

pub struct ViewPlugin;

impl Plugin for ViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            view.after(crate::ui::editor_ui)
                .run_if(crate::mcp::ui_visible),
        );
    }
}

/// The window's area: right of the rail, under the top bar.
fn window_rect(screen: Rect) -> Rect {
    Rect::from_min_max(
        pos2(screen.left() + crate::ui::shell::PANEL_X, screen.top() + crate::ui::shell::TOP),
        pos2(screen.right() - 12.0, screen.bottom() - 12.0),
    )
}

fn view(
    _main: bevy::ecs::system::NonSendMarker,
    mut contexts: EguiContexts,
    mut d: ViewData,
    mut events: MessageReader<CharacterEvent>,
    time: Res<Time<Real>>,
) -> Result {
    let now = time.elapsed_secs_f64();
    for ev in events.read() {
        match &ev.result {
            Ok(()) => {
                let id = ev.id.clone();
                d.v.select_character(&id);
                let name = d.chars.get(&id).map(|e| e.meta.name.clone()).unwrap_or_default();
                d.v.note(format!("{name} is ready"), false, now);
                d.v.fresh = Some((id, now + 8.0));
            }
            Err(e) => d.v.note(e.clone(), true, now),
        }
    }
    if d.ed.tab != Tab::Characters || d.walk.active || !super::labs_enabled() {
        d.v.placing = None;
        d.v.looking = false;
        d.npc_edit.stop();
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?.clone();
    let screen = ctx.content_rect();
    // put away: placing an NPC, drawing a route, picking a spot, looking at the world
    if d.v.placing.is_some() || d.npc_edit.busy() || d.v.looking {
        world_bar(&ctx, &mut d);
        return Ok(());
    }
    d.lib.ensure();
    resolve_selection(&mut d);
    let rect = window_rect(screen);
    let left_w = 300.0;
    let right_w = 360.0;
    egui::Area::new(egui::Id::new("characters_view"))
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(&ctx, |ui| {
            let frame = glass_solid_frame()
                .corner_radius(CornerRadius::same(16))
                .inner_margin(egui::Margin::same(12))
                .shadow(egui::epaint::Shadow::NONE);
            glass(ui, frame, |ui| {
                let inner = rect.size() - vec2(26.0, 26.0);
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    column(ui, left_w, inner.y, |ui| codex_column(ui, &mut d, now));
                    let mid_w = (inner.x - left_w - right_w - 24.0).max(200.0);
                    column(ui, mid_w, inner.y, |ui| {
                        let ViewData { v, chars, lib, stage, .. } = &mut d;
                        stage_column(ui, v, chars, lib, stage, now);
                    });
                    column(ui, right_w, inner.y, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("characters_inspector")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(right_w - 14.0);
                                match d.v.sel.clone() {
                                    Some(Sel::Character(id)) => super::view_char::inspector(ui, &mut d, &id, now),
                                    Some(Sel::Npc(id)) => super::view_codex::npc(ui, &mut d, id, now),
                                    Some(Sel::Creature(id)) => super::view_codex::creature(ui, &mut d, id, now),
                                    Some(Sel::Player) => super::view_codex::player(ui, &mut d, now),
                                    None => empty_inspector(ui, &d),
                                }
                            });
                    });
                });
            });
        });
    Ok(())
}

/// Keeps the selection valid and puts its character on the stage.
fn resolve_selection(d: &mut ViewData) {
    let valid = match &d.v.sel {
        Some(Sel::Character(id)) => d.chars.get(id).is_some(),
        Some(Sel::Npc(id)) => d.gp.npcs.iter().any(|n| n.id == *id),
        Some(Sel::Creature(id)) => d.gp.creatures.iter().any(|c| c.id == *id),
        Some(Sel::Player) => true,
        None => false,
    };
    if !valid {
        d.v.sel = match d.v.section {
            Section::Characters => d.chars.entries.first().map(|e| Sel::Character(e.meta.id.clone())),
            Section::Player => Some(Sel::Player),
            Section::Npcs => d.gp.npcs.first().map(|n| Sel::Npc(n.id)),
            Section::Creatures => d.gp.creatures.first().map(|c| Sel::Creature(c.id)),
        };
    }
    let shown = match &d.v.sel {
        Some(Sel::Character(id)) => Some(id.clone()),
        Some(Sel::Npc(id)) => d
            .gp
            .npcs
            .iter()
            .find(|n| n.id == *id)
            .and_then(|n| n.life.character.clone()),
        Some(Sel::Creature(id)) => d
            .gp
            .creatures
            .iter()
            .find(|c| c.id == *id)
            .and_then(|c| c.character.clone()),
        Some(Sel::Player) => d.world.player.clone(),
        None => None,
    };
    if shown != d.v.selected {
        d.v.selected = shown.clone();
        d.v.mode = Mode::Preview;
        d.v.markers = None;
        // an NPC, a creature or the player: the stage plays its own idle
        let idle = match &d.v.sel {
            Some(Sel::Npc(id)) => d
                .gp
                .npcs
                .iter()
                .find(|n| n.id == *id)
                .map(|n| super::npc::anim_of(&n.life, &d.chars, super::moves::Move::Idle)),
            Some(Sel::Creature(_)) | Some(Sel::Player) => shown
                .as_ref()
                .and_then(|c| d.chars.get(c))
                .map(|e| super::moves::anim_for(&e.meta.moves, super::moves::Move::Idle)),
            _ => None,
        };
        if let Some(a) = idle {
            d.stage.anim = Some(a);
            d.stage.playing = true;
            d.stage.seek = Some(0.0);
        }
    }
    if !matches!(d.v.sel, Some(Sel::Character(_))) {
        d.v.mode = Mode::Preview;
    }
}

fn column(ui: &mut egui::Ui, w: f32, h: f32, body: impl FnOnce(&mut egui::Ui)) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.expand(2.0).intersect(ui.clip_rect()));
    child.set_width(w);
    body(&mut child);
}

fn empty_inspector(ui: &mut egui::Ui, d: &ViewData) {
    let (title_text, steps): (&str, &[&str]) = match d.v.section {
        Section::Npcs => (
            "No NPCs yet",
            &["Pick a character in Characters and press Add as NPC.", "Click the ground where it should stand."],
        ),
        Section::Creatures => ("No creatures yet", &["Press New creature, then give it a model."]),
        _ => (
            "Characters",
            &[
                "Import a model standing in a T- or A-pose (FBX, GLB, OBJ or a .zip).",
                "It's rigged for you; check it with any animation.",
                "Use it as the player, or add it as an NPC.",
            ],
        ),
    };
    ui.label(egui::RichText::new(title_text).size(16.0).color(TEXT).strong());
    ui.add_space(6.0);
    for (i, s) in steps.iter().enumerate() {
        dim(ui, &format!("{}. {s}", i + 1));
    }
}

// ------------------------------------------------------------------ left: the codex

fn codex_column(ui: &mut egui::Ui, d: &mut ViewData, now: f64) {
    title(ui, "Characters");
    hint_small(ui, "Labs · testing");
    ui.add_space(6.0);
    let mut sec = d.v.section;
    if segmented(
        ui,
        &mut sec,
        &[
            (Section::Characters, "Models"),
            (Section::Player, "Player"),
            (Section::Npcs, "NPCs"),
            (Section::Creatures, "Creatures"),
        ],
    ) {
        d.v.section = sec;
        d.v.filter = Filter::All;
        d.v.sel = match sec {
            Section::Characters => d.v.selected.clone().filter(|id| d.chars.get(id).is_some()).map(Sel::Character),
            Section::Player => Some(Sel::Player),
            Section::Npcs => d.gp.npcs.first().map(|n| Sel::Npc(n.id)),
            Section::Creatures => d.gp.creatures.first().map(|c| Sel::Creature(c.id)),
        };
    }
    ui.add_space(8.0);
    match d.v.section {
        Section::Characters | Section::Player => models_grid(ui, d, now),
        Section::Npcs => npcs_grid(ui, d, now),
        Section::Creatures => creatures_grid(ui, d, now),
    }
}

/// A portrait card (the character's picture, or an icon while there's none).
fn portrait_card(
    ui: &mut egui::Ui,
    d: &mut ViewData,
    character: Option<&str>,
    name: &str,
    selected: bool,
    w: f32,
) -> egui::Response {
    let tex = character
        .and_then(|c| d.chars.get(c))
        .and_then(|e| d.thumbs.get(&e.rig_key()));
    if let Some(c) = character
        && tex.is_none()
        && !d.thumbs.wanted.iter().any(|x| x == c)
    {
        d.thumbs.wanted.push(c.to_string());
    }
    match character {
        // (no model yet: an icon)
        None => icon_card(ui, "User", name, selected, w),
        Some(_) => photo_card(ui, tex, name, selected, w),
    }
}

fn models_grid(ui: &mut egui::Ui, d: &mut ViewData, now: f64) {
    if d.v.section == Section::Player {
        dim(ui, "Pick who you play as in Walk mode.");
    }
    if primary_icon_button(ui, "Import", "Import a model…").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .set_title("Import a character")
            .add_filter("3D models", &["fbx", "glb", "gltf", "obj", "zip"])
            .pick_file()
    {
        d.chars.import(&path, OnImport::Auto);
        d.v.note("Importing…", false, now);
    }
    hint_small(ui, "FBX, GLB / glTF, OBJ or a .zip of one; standing in a T- or A-pose.");
    ui.add_space(4.0);
    let mut f = d.v.filter;
    if chips(
        ui,
        &mut f,
        &[(Filter::All, "All", None), (Filter::Mine, "Mine", None), (Filter::Builtin, "Built in", None)],
    ) {
        d.v.filter = f;
    }
    ui.add_space(4.0);
    // imports still running
    let pending: Vec<String> = d
        .chars
        .jobs
        .iter()
        .filter(|j| d.chars.get(&j.id).is_none())
        .map(|j| j.status.lock().unwrap().clone())
        .collect();
    for status in pending {
        list_row(ui, Lead::Icon("LoadingCircle"), "Importing", &status, "", false);
        ui.ctx().request_repaint();
    }
    let list: Vec<(String, String, bool)> = d
        .chars
        .entries
        .iter()
        .filter(|e| match d.v.filter {
            Filter::Mine => e.builtin.is_none(),
            Filter::Builtin => e.builtin.is_some(),
            _ => true,
        })
        .map(|e| (e.meta.id.clone(), e.meta.name.clone(), d.world.player.as_deref() == Some(e.meta.id.as_str())))
        .collect();
    egui::ScrollArea::vertical()
        .id_salt("codex_models")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let picked = card_grid(ui, 3, list.len(), |ui, i, w| {
                let (id, name, player) = &list[i];
                let sel = match (&d.v.sel, d.v.section) {
                    (_, Section::Player) => *player,
                    (Some(Sel::Character(s)), _) => s == id,
                    _ => false,
                };
                let label = if *player { format!("{name} ★") } else { name.clone() };
                portrait_card(ui, d, Some(id), &label, sel, w)
            });
            if let Some(i) = picked {
                let id = list[i].0.clone();
                if d.v.section == Section::Player {
                    if d.chars.get(&id).is_some_and(|e| e.rigged()) {
                        d.world.player = Some(id);
                        d.meta.dirty = true;
                        d.v.sel = Some(Sel::Player);
                    } else {
                        d.v.note("Rig it first", true, now);
                    }
                } else {
                    d.v.select_character(&id);
                }
            }
        });
}

fn npcs_grid(ui: &mut egui::Ui, d: &mut ViewData, now: f64) {
    // "Add an NPC": with the character on the stage (or the first rigged one)
    let candidate = d
        .v
        .selected
        .clone()
        .filter(|c| d.chars.get(c).is_some_and(|e| e.rigged()))
        .or_else(|| d.chars.entries.iter().find(|e| e.rigged()).map(|e| e.meta.id.clone()));
    let label = candidate
        .as_ref()
        .and_then(|c| d.chars.get(c))
        .map(|e| format!("Add {} as an NPC", e.meta.name))
        .unwrap_or_else(|| "Add an NPC".into());
    if primary_icon_button(ui, "PlusMedium", &label).clicked() {
        match candidate {
            Some(c) => d.v.placing = Some(Placing { character: c }),
            None => d.v.note("Rig a character first", true, now),
        }
    }
    hint_small(ui, "Or pick a model in Models and press Add as NPC.");
    ui.add_space(4.0);
    // roles present
    let mut roles: Vec<NpcRole> = d.gp.npcs.iter().map(|n| n.role).collect();
    roles.sort_by_key(|r| NpcRole::ALL.iter().position(|x| x == r));
    roles.dedup();
    let mut opts: Vec<(Filter, &str, Option<usize>)> = vec![(Filter::All, "All", Some(d.gp.npcs.len()))];
    for r in &roles {
        opts.push((Filter::Role(*r), r.name(), Some(d.gp.npcs.iter().filter(|n| n.role == *r).count())));
    }
    let mut f = d.v.filter;
    if chips(ui, &mut f, &opts) {
        d.v.filter = f;
    }
    ui.add_space(4.0);
    let list: Vec<(u32, Option<String>, String)> = d
        .gp
        .npcs
        .iter()
        .filter(|n| match d.v.filter {
            Filter::Role(r) => n.role == r,
            _ => true,
        })
        .map(|n| (n.id, n.life.character.clone(), n.name.clone()))
        .collect();
    if list.is_empty() {
        dim(ui, "No NPCs in this world yet.");
    }
    egui::ScrollArea::vertical()
        .id_salt("codex_npcs")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let picked = card_grid(ui, 3, list.len(), |ui, i, w| {
                let (id, c, name) = &list[i];
                let sel = d.v.sel == Some(Sel::Npc(*id));
                portrait_card(ui, d, c.as_deref(), name, sel, w)
            });
            if let Some(i) = picked {
                d.v.sel = Some(Sel::Npc(list[i].0));
                d.gp.selected = Some(crate::gameplay::GameplaySel::Npc(list[i].0));
            }
        });
}

fn creatures_grid(ui: &mut egui::Ui, d: &mut ViewData, now: f64) {
    if primary_icon_button(ui, "PlusMedium", "New creature").clicked() {
        let mut id = 0;
        d.gp_edit(now - 10.0, "New creature", |gp| {
            id = crate::gameplay::edit::add_creature(
                gp,
                "",
                "",
                [1, 3],
                crate::gameplay::Aggression::Hostile,
                false,
            );
        });
        d.v.sel = Some(Sel::Creature(id));
    }
    hint_small(ui, "The monsters and animals spawn areas (Gameplay › Spawns) draw from.");
    ui.add_space(4.0);
    let list: Vec<(u32, Option<String>, String)> = d
        .gp
        .creatures
        .iter()
        .map(|c| (c.id, c.character.clone(), c.name.clone()))
        .collect();
    if list.is_empty() {
        dim(ui, "No creatures yet.");
    }
    egui::ScrollArea::vertical()
        .id_salt("codex_creatures")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let picked = card_grid(ui, 3, list.len(), |ui, i, w| {
                let (id, c, name) = &list[i];
                let sel = d.v.sel == Some(Sel::Creature(*id));
                portrait_card(ui, d, c.as_deref(), name, sel, w)
            });
            if let Some(i) = picked {
                d.v.sel = Some(Sel::Creature(list[i].0));
            }
        });
}

// ------------------------------------------------------------------ the world, the window put away

/// The guide bar over the world while the window is put away.
fn world_bar(ctx: &egui::Context, d: &mut ViewData) {
    let text = if let Some(p) = &d.v.placing {
        let name = d.chars.get(&p.character).map(|e| e.meta.name.clone()).unwrap_or_default();
        format!("Click the ground to stand {name} there as an NPC")
    } else if let Some(id) = d.npc_edit.moving {
        let name = d.gp.npcs.iter().find(|n| n.id == id).map(|n| n.name.clone()).unwrap_or_default();
        format!("Click the ground to move {name} there (click again to adjust)")
    } else if let Some(id) = d.npc_edit.drawing_route {
        let n = d.gp.npcs.iter().find(|n| n.id == id);
        format!(
            "Click the ground to add waypoints to {}'s route ({} so far)",
            n.map(|n| n.name.as_str()).unwrap_or("its"),
            n.map(|n| n.life.route.len()).unwrap_or(0)
        )
    } else if d.npc_edit.picking_spot.is_some() {
        "Click the ground where it should do that".to_string()
    } else {
        "Looking at the world".to_string()
    };
    let mut done = false;
    egui::Area::new(egui::Id::new("characters_world_bar"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, vec2(0.0, crate::ui::shell::TOP))
        .show(ctx, |ui| {
            glass(ui, glass_frame(), |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(text).color(TEXT).size(13.5));
                    ui.add_space(6.0);
                    kbd(ui, "Esc");
                    hint_small(ui, "or right-click to stop");
                    ui.add_space(6.0);
                    if small_button(ui, "Back to Characters").clicked() {
                        done = true;
                    }
                });
            });
        });
    if done {
        d.v.placing = None;
        d.v.looking = false;
        d.npc_edit.stop();
    }
    if d.v.looking && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        d.v.looking = false;
    }
}

// ------------------------------------------------------------------ middle: the stage

fn stage_column(
    ui: &mut egui::Ui,
    v: &mut CharacterView,
    chars: &mut Characters,
    lib: &AnimLibrary,
    stage: &mut Stage,
    now: f64,
) {
    let Some(id) = v.selected.clone() else {
        dim(ui, "Import a model to begin.");
        return;
    };
    let avail = ui.available_size();
    let bar_h = 34.0;
    let size = vec2(avail.x, (avail.y - bar_h - 8.0).max(120.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    stage.front = v.mode == Mode::Markers;
    stage.want = Some(Want {
        character: id.clone(),
        size: UVec2::new((size.x * ppp) as u32, (size.y * ppp) as u32),
    });
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(12), Color32::from_rgb(22, 23, 26));
    if let Some(tex) = stage.egui {
        p.image(
            tex,
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    if !stage.ready || chars.busy(&id).is_some() {
        let msg = chars
            .busy(&id)
            .map(|j| j.status.lock().unwrap().clone())
            .unwrap_or_else(|| "Loading…".into());
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            msg,
            egui::FontId::proportional(14.0),
            TEXT_DIM,
        );
        ui.ctx().request_repaint();
    }
    if let Some((fid, until)) = &v.fresh
        && *fid == id
        && now < *until
        && stage.ready
    {
        p.text(
            rect.left_top() + vec2(14.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Preparing its textures (the first time only)…",
            egui::FontId::proportional(12.0),
            TEXT_DIM,
        );
        ui.ctx().request_repaint();
    }
    if v.mode == Mode::Markers {
        markers_overlay(ui, rect, &resp, v, chars, stage, &id);
    } else {
        // turn and zoom
        if resp.dragged() {
            let d = resp.drag_delta();
            stage.orbit.yaw -= d.x * 0.008;
            stage.orbit.pitch = (stage.orbit.pitch + d.y * 0.006).clamp(-0.2, 1.2);
        }
        if resp.hovered() {
            let s = ui.input(|i| i.smooth_scroll_delta.y);
            if s != 0.0 {
                stage.orbit.zoom = (stage.orbit.zoom * (1.0 - s * 0.002)).clamp(0.35, 3.0);
            }
        }
    }
    // the note under the picture
    if let Some((msg, err, until)) = &v.note {
        if now < *until {
            let col = if *err { WARN } else { TEXT };
            let g = p.layout(
                msg.clone(),
                egui::FontId::proportional(12.5),
                col,
                rect.width() - 40.0,
            );
            let r = Rect::from_center_size(
                pos2(rect.center().x, rect.bottom() - 22.0 - g.size().y * 0.5),
                g.size() + vec2(20.0, 10.0),
            );
            p.rect_filled(r, CornerRadius::same(9), Color32::from_black_alpha(170));
            p.galley(r.min + vec2(10.0, 5.0), g, col);
            ui.ctx().request_repaint();
        } else {
            v.note = None;
        }
    }
    ui.add_space(6.0);
    playback_bar(ui, v, chars, lib, stage, &id);
}

fn playback_bar(
    ui: &mut egui::Ui,
    v: &CharacterView,
    chars: &Characters,
    lib: &AnimLibrary,
    stage: &mut Stage,
    id: &str,
) {
    if v.mode == Mode::Markers {
        hint_small(
            ui,
            "Drag each dot onto the body (front view). Then Rig with these markers.",
        );
        return;
    }
    let rigged = chars.get(id).is_some_and(|e| e.rigged());
    let anim = stage.anim.as_deref().and_then(|k| lib.find(k)).cloned();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let icon = if stage.playing { "Pause" } else { "Play" };
        if icon_button(ui, icon, if stage.playing { "Pause" } else { "Play" }).clicked() {
            stage.playing = !stage.playing;
        }
        let dur = anim.as_ref().map(|a| a.seconds).unwrap_or(0.0).max(0.01);
        let mut t = if anim.as_ref().is_some_and(|a| a.looped) {
            stage.time.rem_euclid(dur)
        } else {
            stage.time.min(dur)
        };
        let w = (ui.available_width() - 340.0).max(60.0);
        ui.allocate_ui(vec2(w, 22.0), |ui| {
            ui.set_width(w);
            if ui
                .add(TrackSlider::new(&mut t, 0.0..=dur).show_value(false))
                .changed()
            {
                stage.seek = Some(t);
                stage.playing = false;
            }
        });
        ui.label(
            egui::RichText::new(format!("{t:.1} / {dur:.1} s"))
                .size(11.0)
                .color(TEXT_DIM)
                .monospace(),
        );
        let mut speed = stage.speed;
        if segmented(ui, &mut speed, &[(0.5, "½×"), (1.0, "1×"), (2.0, "2×")]) {
            stage.speed = speed;
        }
        toggle(ui, &mut stage.in_place, "In place");
    });
    if !rigged {
        hint_small(ui, "Not rigged yet: rig it to play animations.");
    } else if let Some(a) = anim {
        hint_small(
            ui,
            &format!(
                "{} · {}",
                a.label,
                lib.sources.get(a.source).map(|s| s.license.as_str()).unwrap_or("")
            ),
        );
    }
}

/// The eight markers on the front view: drag to move (mirrored to the other side unless
/// `one_side`), with guide lines between them.
fn markers_overlay(
    ui: &mut egui::Ui,
    rect: Rect,
    resp: &egui::Response,
    v: &mut CharacterView,
    chars: &mut Characters,
    stage: &Stage,
    id: &str,
) {
    let Some(model) = chars.prepared_model(id) else {
        ui.ctx().request_repaint();
        return;
    };
    if v.markers.as_ref().is_none_or(|(mid, _)| mid != id) {
        let m = chars
            .get(id)
            .and_then(|e| e.meta.markers)
            .unwrap_or_else(|| autorig::markers::guess(&model));
        v.markers = Some((id.to_string(), m));
    }
    let to_screen = |p: Vec3| -> Option<Pos2> {
        let uv = stage.project(p)?;
        Some(rect.min + vec2(uv.x * rect.width(), uv.y * rect.height()))
    };
    let p = ui.painter_at(rect);
    let (_, m) = v.markers.clone().unwrap();
    // guide lines
    let line = Stroke::new(1.5, Color32::from_white_alpha(110));
    for (a, b) in [
        (Marker::Chin, Marker::Groin),
        (Marker::Groin, Marker::KneeL),
        (Marker::Groin, Marker::KneeR),
        (Marker::ElbowL, Marker::WristL),
        (Marker::ElbowR, Marker::WristR),
        (Marker::Chin, Marker::ElbowL),
        (Marker::Chin, Marker::ElbowR),
    ] {
        if let (Some(x), Some(y)) = (to_screen(m.get(a)), to_screen(m.get(b))) {
            p.line_segment([x, y], line);
        }
    }
    let pointer = ui.input(|i| i.pointer.interact_pos());
    if resp.drag_started()
        && let Some(at) = pointer
    {
        v.dragging = Marker::ALL
            .into_iter()
            .filter_map(|k| to_screen(m.get(k)).map(|s| (k, s.distance(at))))
            .filter(|(_, d)| *d < 16.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k);
    }
    if let (Some(k), Some(at)) = (v.dragging, pointer)
        && resp.dragged()
    {
        let uv = ((at - rect.min) / rect.size()).clamp(egui::Vec2::ZERO, egui::Vec2::splat(1.0));
        let old = m.get(k);
        if let Some(q) = stage.unproject(Vec2::new(uv.x, uv.y), old.z) {
            let (_, mm) = v.markers.as_mut().unwrap();
            mm.set(k, Vec3::new(q.x, q.y, old.z));
            if !v.one_side
                && let Some(twin) = twin(k)
            {
                let cx = mm.groin.x;
                let t = mm.get(twin);
                mm.set(twin, Vec3::new(2.0 * cx - q.x, q.y, t.z));
            }
        }
    }
    if resp.drag_stopped()
        && let Some(k) = v.dragging.take()
    {
        let (_, mm) = v.markers.as_mut().unwrap();
        mm.set(k, autorig::markers::with_depth(&model, mm.get(k)));
        if !v.one_side
            && let Some(t) = twin(k)
        {
            mm.set(t, autorig::markers::with_depth(&model, mm.get(t)));
        }
    }
    let (_, m) = v.markers.clone().unwrap();
    for k in Marker::ALL {
        let Some(s) = to_screen(m.get(k)) else {
            continue;
        };
        let hot = v.dragging == Some(k) || pointer.is_some_and(|a| a.distance(s) < 16.0);
        let col = marker_colour(k);
        p.circle_filled(s, if hot { 8.5 } else { 7.0 }, col);
        p.circle_stroke(s, if hot { 8.5 } else { 7.0 }, Stroke::new(2.0, Color32::from_black_alpha(200)));
        if hot {
            let g = p.layout_no_wrap(k.label().into(), egui::FontId::proportional(12.0), TEXT);
            let r = Rect::from_min_size(s + vec2(12.0, -10.0), g.size() + vec2(10.0, 6.0));
            p.rect_filled(r, CornerRadius::same(6), Color32::from_black_alpha(190));
            p.galley(r.min + vec2(5.0, 3.0), g, TEXT);
        }
    }
    if pointer.is_some_and(|a| rect.contains(a)) {
        ui.ctx().request_repaint();
    }
}

fn twin(k: Marker) -> Option<Marker> {
    Some(match k {
        Marker::WristL => Marker::WristR,
        Marker::WristR => Marker::WristL,
        Marker::ElbowL => Marker::ElbowR,
        Marker::ElbowR => Marker::ElbowL,
        Marker::KneeL => Marker::KneeR,
        Marker::KneeR => Marker::KneeL,
        _ => return None,
    })
}

/// Marker colours: meaning only (left warm, right cool, middle white).
fn marker_colour(k: Marker) -> Color32 {
    match k {
        Marker::Chin | Marker::Groin => Color32::WHITE,
        Marker::WristL | Marker::ElbowL | Marker::KneeL => Color32::from_rgb(255, 196, 120),
        _ => Color32::from_rgb(140, 200, 255),
    }
}


pub(super) fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

