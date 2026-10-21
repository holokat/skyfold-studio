//! Locking objects, and the viewport's right-click menu.
//!
//! Right-click an object (Build › Objects; a click, not a drag, so right-drag still orbits) for
//! its menu: Lock / Unlock, Duplicate, Delete, Save as scene. A locked object (or a set piece's
//! whole set) can't be moved, turned, resized, lifted or deleted until it's unlocked: the handles
//! don't grab it, Delete and Z / X leave it, and MCP edits refuse it. It still selects, copies
//! (the copy is unlocked) and rides terrain edits. Locked objects wear a small padlock badge on
//! the Objects tab (one per locked set), and a locked selection is outlined in amber instead of
//! showing handles. MCP: `objects_lock`.

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, LayerId, Order, Stroke, pos2, vec2,
};
use egui_phosphor::regular as icon;

use super::footprint::{Footprint, ModelBounds};
use crate::camera::{MainCamera, Pointer};
use crate::editor::{Editor, Tab};
use crate::history::History;
use crate::objects::{Library, ObjectStore};
use crate::terrain::Terrain;
use crate::ui::theme::*;

pub const AMBER: Color32 = Color32::from_rgb(255, 196, 92);

/// The open right-click menu (its object, and where it opened, in egui points).
#[derive(Resource, Default)]
pub struct ObjectMenu {
    pub open: Option<(u64, egui::Pos2)>,
    /// where the right button went down (logical px), to tell a click from an orbit drag
    press: Option<Vec2>,
    /// "Save as scene" was chosen for these objects (`crate::scenes` takes it)
    pub save_scene: Option<Vec<u64>>,
}

/// The objects a lock / unlock of `id` covers: its whole set, or itself.
pub fn lock_targets(store: &ObjectStore, id: u64) -> Vec<u64> {
    match store.items.get(&id).map(|p| p.group) {
        Some(0) => vec![id],
        Some(g) => store.group_members(g),
        None => vec![],
    }
}

/// Locks or unlocks `id` (with its set) as one undo step.
pub fn set_lock(store: &mut ObjectStore, history: &mut History, id: u64, locked: bool) -> usize {
    let ids = lock_targets(store, id);
    if ids.is_empty() {
        return 0;
    }
    history.begin(if locked { "Lock" } else { "Unlock" });
    for &i in &ids {
        history.touch_object(store, i);
    }
    store.set_locked(&ids, locked);
    history.commit(store);
    ids.len()
}

/// A right click (press and release within a few pixels) on an object opens its menu; on open
/// ground, or anywhere else, it closes.
#[allow(clippy::too_many_arguments)]
pub(super) fn menu_input(
    ed: Res<Editor>,
    pointer: Res<Pointer>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    lib: Option<Res<Library>>,
    bounds: Res<ModelBounds>,
    mut store: ResMut<ObjectStore>,
    mut menu: ResMut<ObjectMenu>,
    mut contexts: EguiContexts,
) {
    if ed.tab != Tab::Objects {
        menu.open = None;
        menu.press = None;
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        menu.open = None;
    }
    if buttons.just_pressed(MouseButton::Right) {
        menu.press = if pointer.over_ui {
            None
        } else {
            pointer.screen
        };
    }
    if !buttons.just_released(MouseButton::Right) {
        return;
    }
    let (Some(down), Some(up)) = (menu.press.take(), pointer.screen) else {
        return;
    };
    if down.distance(up) > 5.0 {
        return; // an orbit drag
    }
    let Some(lib) = lib else { return };
    let zoom = contexts.ctx_mut().map(|c| c.zoom_factor()).unwrap_or(1.0);
    match super::handles::pick_box(&store, &lib, &bounds, pointer.ray, pointer.hit) {
        Some(id) => {
            store.selected = Some(id);
            menu.open = Some((id, pos2(up.x / zoom, up.y / zoom)));
        }
        None => menu.open = None,
    }
}

/// The right-click menu, the padlock badges over locked objects and the locked selection's
/// amber outline.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    mut frame: Local<u32>,
    mut contexts: EguiContexts,
    mut ed: ResMut<Editor>,
    walk: Res<crate::walk::WalkState>,
    mut store: ResMut<ObjectStore>,
    mut history: ResMut<History>,
    mut menu: ResMut<ObjectMenu>,
    lib: Option<Res<Library>>,
    bounds: Res<ModelBounds>,
    terrain: Res<Terrain>,
    water: Res<crate::water::WaterBodies>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    caves: Res<crate::caves::render::CaveWorld>,
) -> Result {
    if ed.tab != Tab::Objects || walk.active || walk.cinematic {
        return Ok(());
    }
    let (Some(lib), Ok((cam, cam_t))) = (lib, camera.single()) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    if !ctx.fonts(|f| f.definitions().families.contains_key(&semibold())) {
        return Ok(());
    }
    let k = 1.0 / ctx.zoom_factor();
    let proj = |w: Vec3| {
        cam.world_to_viewport(cam_t, w)
            .ok()
            .map(|v| pos2(v.x * k, v.y * k))
    };
    let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::new("object_locks")));
    let screen = ctx.content_rect();

    // padlocks: one per locked object, one per locked set (over its middle)
    let eye = cam_t.translation();
    let mut sets = std::collections::BTreeSet::new();
    let mut tops: Vec<Vec3> = vec![];
    // (the locked selection wears its "Locked" label instead)
    let sel_group = store
        .selected
        .and_then(|id| store.items.get(&id))
        .map(|p| (p.id, p.group));
    for p in store.items.values().filter(|p| {
        p.locked && sel_group.is_none_or(|(id, g)| p.id != id && (g == 0 || p.group != g))
    }) {
        if p.group != 0 {
            sets.insert(p.group);
            continue;
        }
        let Some(e) = lib.get(&p.asset) else { continue };
        let h = bounds.of(e).1.y * p.scale3().y;
        tops.push(p.translation() + Vec3::Y * (h + 0.6));
    }
    for g in sets {
        if let Some(f) = super::group::frame(&store, &lib, &bounds, g) {
            tops.push(f.translation() + Vec3::Y * (f.scale3().y + 1.0));
        }
    }
    tops.sort_by(|a, b| a.distance_squared(eye).total_cmp(&b.distance_squared(eye)));
    for top in tops.into_iter().take(300) {
        if top.distance(eye) > 900.0 {
            break;
        }
        let Some(c) = proj(top).filter(|c| screen.contains(*c)) else {
            continue;
        };
        badge(&painter, c, k);
    }

    // the locked selection: amber footprint and a "Locked" pill instead of handles
    if let Some(id) = store.selected.filter(|&id| store.is_locked(id))
        && let Some(sel) = store.items.get(&id)
    {
        let (fp, top) = match sel.group {
            0 => lib.get(&sel.asset).map(|e| {
                let b = bounds.of(e);
                (
                    Footprint::of(sel, b),
                    sel.translation() + Vec3::Y * (b.1.y * sel.scale3().y),
                )
            }),
            g => super::group::frame(&store, &lib, &bounds, g).map(|f| {
                (
                    Footprint::of(&f, super::group::FRAME_BOUNDS),
                    f.translation() + Vec3::Y * f.scale3().y,
                )
            }),
        }
        .unzip();
        if let (Some(fp), Some(top)) = (fp, top) {
            let y = sel.pos[1].min(top.y);
            let ring: Vec<egui::Pos2> = fp
                .corners()
                .iter()
                .chain(std::iter::once(&fp.corners()[0]))
                .filter_map(|c| proj(Vec3::new(c.x, y, c.y)))
                .collect();
            if ring.len() == 5 {
                painter.add(egui::Shape::line(
                    ring.clone(),
                    Stroke::new(3.5 * k, Color32::from_black_alpha(110)),
                ));
                painter.add(egui::Shape::dashed_line(
                    &ring,
                    Stroke::new(1.8 * k, AMBER),
                    7.0 * k,
                    4.0 * k,
                ));
            }
            if let Some(c) = proj(top + Vec3::Y * 0.6) {
                let text = format!("{}  Locked · right-click to unlock", icon::LOCK_SIMPLE);
                let galley = painter.layout_no_wrap(text, FontId::new(12.0 * k, semibold()), AMBER);
                let r = egui::Rect::from_center_size(
                    c - vec2(0.0, 14.0 * k),
                    galley.size() + vec2(16.0 * k, 8.0 * k),
                );
                painter.rect_filled(
                    r,
                    CornerRadius::same((r.height() / 2.0) as u8),
                    Color32::from_rgba_unmultiplied(15, 20, 28, 230),
                );
                painter.rect_stroke(
                    r,
                    CornerRadius::same((r.height() / 2.0) as u8),
                    Stroke::new(1.0 * k, AMBER.gamma_multiply(0.6)),
                    egui::StrokeKind::Inside,
                );
                painter.galley(r.min + vec2(8.0 * k, 4.0 * k), galley, AMBER);
            }
        }
    }

    // `WB_OBJECT_MENU=<frame>` (screenshot tests): at that frame the newest locked object is
    // selected and its menu opened
    if let Some(at) = std::env::var("WB_OBJECT_MENU")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    {
        *frame += 1;
        if *frame == at
            && let Some(p) = store
                .items
                .values()
                .filter(|p| p.locked && p.group == 0)
                .max_by_key(|p| p.id)
                .cloned()
            && let Some(c) = proj(p.translation() + Vec3::Y * 2.0)
        {
            store.selected = Some(p.id);
            menu.open = Some((p.id, c));
        }
    }
    // the menu
    let Some((id, at)) = menu.open else {
        return Ok(());
    };
    let Some(p) = store.items.get(&id).cloned() else {
        menu.open = None;
        return Ok(());
    };
    let locked = store.is_locked(id);
    let members = if p.group != 0 {
        store.group_members(p.group).len()
    } else {
        1
    };
    let title = if p.group != 0 {
        format!("Set · {members} pieces")
    } else {
        lib.get(&p.asset)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| p.asset.clone())
    };
    let mut choice = None;
    let area = egui::Area::new(Id::new("object_menu"))
        .order(Order::Foreground)
        .fixed_pos(at + vec2(4.0, 4.0))
        .constrain(true)
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(210.0);
                    ui.label(egui::RichText::new(title).small().color(TEXT_DIM));
                    ui.separator();
                    let item =
                        |ui: &mut egui::Ui, glyph: &str, label: &str, key: &str, enabled: bool| {
                            let text = egui::RichText::new(format!("{glyph}  {label}"));
                            let r = ui.add_enabled(
                                enabled,
                                egui::Button::new(text)
                                    .shortcut_text(key)
                                    .frame(false)
                                    .min_size(vec2(210.0, 24.0)),
                            );
                            r.clicked()
                        };
                    let what = if p.group != 0 { " set" } else { "" };
                    if locked {
                        if item(
                            ui,
                            icon::LOCK_SIMPLE_OPEN,
                            &format!("Unlock{what}"),
                            "",
                            true,
                        ) {
                            choice = Some("unlock");
                        }
                    } else if item(ui, icon::LOCK_SIMPLE, &format!("Lock{what}"), "", true) {
                        choice = Some("lock");
                    }
                    if item(ui, icon::COPY, &format!("Duplicate{what}"), "⌘D", true) {
                        choice = Some("duplicate");
                    }
                    if item(ui, icon::FLOPPY_DISK, "Save as scene…", "", true) {
                        choice = Some("scene");
                    }
                    ui.separator();
                    if item(ui, icon::TRASH, &format!("Delete{what}"), "⌫", !locked) {
                        choice = Some("delete");
                    }
                });
        });
    if area.response.clicked_elsewhere() && choice.is_none() {
        menu.open = None;
    }
    let Some(choice) = choice else { return Ok(()) };
    menu.open = None;
    match choice {
        "lock" | "unlock" => {
            let n = set_lock(&mut store, &mut history, id, choice == "lock");
            ed.toast(match (choice, n) {
                ("lock", 1) => {
                    "Locked: it stays put until you unlock it (right-click › Unlock)".to_string()
                }
                ("lock", n) => {
                    format!("Locked the set ({n} pieces): it stays put until you unlock it")
                }
                (_, 1) => "Unlocked".to_string(),
                (_, n) => format!("Unlocked the set ({n} pieces)"),
            });
        }
        "duplicate" => {
            let copy = if p.group != 0 {
                super::group::duplicate(&mut store, &lib, &bounds, &terrain, p.group)
            } else {
                crate::objects::duplicate(
                    &mut store,
                    Some(&lib),
                    &terrain,
                    &water,
                    Some(&caves),
                    id,
                )
                .into_iter()
                .collect()
            };
            if copy.is_empty() {
                ed.toast("That can't be copied (it's in an asset pack you haven't got)");
            } else {
                history.begin(if p.group != 0 {
                    "Duplicate set"
                } else {
                    "Duplicate object"
                });
                let first = copy[0].id;
                for mut c in copy {
                    c.locked = false;
                    history.touch_object(&store, c.id);
                    store.insert(c);
                }
                store.selected = Some(first);
                history.commit(&store);
            }
        }
        "scene" => menu.save_scene = Some(lock_targets(&store, id)),
        "delete" if !locked => {
            history.begin(if p.group != 0 {
                "Delete set"
            } else {
                "Delete object"
            });
            for m in lock_targets(&store, id) {
                history.touch_object(&store, m);
                store.remove(m);
            }
            history.commit(&store);
        }
        _ => {}
    }
    Ok(())
}

/// A padlock badge centred on `c`.
fn badge(painter: &egui::Painter, c: egui::Pos2, k: f32) {
    let r = 10.0 * k;
    painter.circle_filled(c, r + 1.5 * k, Color32::from_black_alpha(90));
    painter.circle_filled(c, r, Color32::from_rgba_unmultiplied(18, 22, 30, 225));
    painter.circle_stroke(c, r, Stroke::new(1.0 * k, AMBER.gamma_multiply(0.7)));
    painter.text(
        c,
        Align2::CENTER_CENTER,
        icon::LOCK_SIMPLE,
        FontId::proportional(11.5 * k),
        AMBER,
    );
}
