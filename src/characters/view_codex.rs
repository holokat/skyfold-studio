//! The codex inspectors for the world's living things: an NPC (Basics, Behavior, Talk, Moves),
//! a creature type, and the player. Edits go to the Gameplay records, one undo step a burst.

use bevy::prelude::*;
use bevy_egui::egui;
use egui::vec2;

use super::npc::{Activity, Attitude, Behavior, NpcLife, Slot};
use super::view::{NpcTab, Sel, ViewData};
use super::view_char::{header, moves_editor};
use crate::gameplay::{Aggression, NpcRole};
use crate::ui::widgets::*;

/// A menu choosing a rigged library character (or none).
fn character_menu(ui: &mut egui::Ui, d: &ViewData, label: &str, salt: &str, cur: &mut Option<String>) -> bool {
    let text = cur
        .as_ref()
        .and_then(|c| d.chars.get(c))
        .map(|e| e.meta.name.clone())
        .unwrap_or_else(|| "None".into());
    let mut pick: Option<Option<String>> = None;
    dropdown_row(ui, label, salt, text, |ui| {
        if menu_item(ui, "None", "", cur.is_none()).clicked() {
            pick = Some(None);
        }
        for e in d.chars.entries.iter().filter(|e| e.rigged()) {
            let on = cur.as_deref() == Some(e.meta.id.as_str());
            if menu_item(ui, e.meta.name.as_str(), &format!("{:.2} m", e.meta.height), on).clicked() {
                pick = Some(Some(e.meta.id.clone()));
            }
        }
    });
    match pick {
        Some(p) if p != *cur => {
            *cur = p;
            true
        }
        _ => false,
    }
}

fn hhmm(h: f32) -> String {
    let m = (h.rem_euclid(24.0) * 60.0).round() as u32;
    format!("{:02}:{:02}", m / 60 % 24, m % 60)
}

pub(super) fn npc(ui: &mut egui::Ui, d: &mut ViewData, id: u32, now: f64) {
    let Some(orig) = d.gp.npcs.iter().find(|n| n.id == id).cloned() else {
        return;
    };
    let mut n = orig.clone();
    let sub = format!(
        "{} · {}{}",
        n.role.name(),
        n.life.attitude.name(),
        if n.life.faction.is_empty() { String::new() } else { format!(" · {}", n.life.faction) }
    );
    header(ui, d, n.life.character.as_deref(), &mut n.name, true, &sub);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 12.0) / 3.0;
        ui.allocate_ui(vec2(w, 32.0), |ui| {
            if primary_icon_button(ui, "ArrowsAllSides", "Move")
                .on_hover_text("Click the ground where it should stand")
                .clicked()
            {
                let p = Vec3::from(n.pos);
                d.orbit.focus = p;
                d.orbit.distance = d.orbit.distance.clamp(10.0, 40.0);
                d.npc_edit.stop();
                d.npc_edit.moving = Some(id);
            }
        });
        ui.allocate_ui(vec2(w, 32.0), |ui| {
            if wide_icon_button(ui, "EyeOpen", "Show").on_hover_text("Look at it in the world").clicked() {
                let p = Vec3::from(n.pos);
                d.orbit.focus = p;
                d.orbit.distance = d.orbit.distance.clamp(6.0, 14.0);
                d.v.looking = true;
            }
        });
        ui.allocate_ui(vec2(w, 32.0), |ui| {
            toggle(ui, &mut d.npc_edit.preview, "Live")
                .on_hover_text("Act out behaviors in the editor too (they always do while walking)");
        });
    });
    if d.v.sel.is_none() {
        return;
    }
    ui.add_space(8.0);
    let mut tab = d.v.npc_tab;
    if segmented(
        ui,
        &mut tab,
        &[
            (NpcTab::Basics, "Basics"),
            (NpcTab::Behavior, "Behavior"),
            (NpcTab::Talk, "Talk"),
            (NpcTab::Moves, "Moves"),
        ],
    ) {
        d.v.npc_tab = tab;
    }
    ui.add_space(8.0);
    match d.v.npc_tab {
        NpcTab::Basics => {
            character_menu(ui, d, "Model", "npc_model", &mut n.life.character);
            let mut role = n.role;
            dropdown_row(ui, "Role", "npc_role", role.name().to_string(), |ui| {
                for r in NpcRole::ALL {
                    menu_option(ui, &mut role, r, r.name());
                }
            });
            n.role = role;
            let mut att = n.life.attitude;
            segmented(ui, &mut att, &Attitude::ALL.map(|a| (a, a.name())));
            n.life.attitude = att;
            ui.horizontal(|ui| {
                dim(ui, "Faction");
                ui.add(egui::TextEdit::singleline(&mut n.life.faction).hint_text("e.g. Town guard").desired_width(f32::INFINITY));
            });
            let mut deg = (n.yaw.to_degrees()).rem_euclid(360.0);
            if slider_row(ui, "Facing", &mut deg, 0.0..=359.0, |v| format!("{v:.0}°")).changed() {
                n.yaw = deg.to_radians();
            }
            let cur = n
                .poi
                .and_then(|p| d.gp.pois.iter().find(|q| q.id == p))
                .map(|q| q.name.clone())
                .unwrap_or_else(|| "No place".into());
            let pois: Vec<(u32, String)> = d.gp.pois.iter().map(|q| (q.id, q.name.clone())).collect();
            dropdown_row(ui, "Belongs to", "npc_poi", cur, |ui| {
                menu_option(ui, &mut n.poi, None, "No place");
                for (pid, name) in &pois {
                    menu_option(ui, &mut n.poi, Some(*pid), name.as_str());
                }
            });
            hint_small(ui, "Notes");
            ui.add(egui::TextEdit::multiline(&mut n.notes).desired_rows(2).desired_width(f32::INFINITY));
            hint_small(ui, &format!("Stands at {:.0}, {:.0}", n.pos[0], n.pos[2]));
            ui.add_space(12.0);
            if small_icon_button(ui, "TrashCan", "Delete this NPC").clicked() {
                d.gp_edit(now - 10.0, "Delete NPC", |gp| gp.npcs.retain(|x| x.id != id));
                d.v.sel = None;
                return;
            }
        }
        NpcTab::Behavior => behavior(ui, d, &mut n.life, id),
        NpcTab::Talk => {
            section_box(ui, "npc_interactions", "Interactions", |ui| {
                toggle(ui, &mut n.life.can_talk, "Can talk");
                toggle(ui, &mut n.life.can_trade, "Can trade");
                toggle(ui, &mut n.life.gives_quests, "Gives quests");
                toggle(ui, &mut n.life.can_join, "Can join the party");
                toggle(ui, &mut n.life.attackable, "Can be attacked");
            });
            ui.add_space(8.0);
            section_box(ui, "npc_dialogue", "Dialogue", |ui| {
                for (label, text, hint) in [
                    ("Greeting", &mut n.life.lines.greet, "Hello there!"),
                    ("While idle", &mut n.life.lines.idle, "Such a beautiful day…"),
                    ("Farewell", &mut n.life.lines.farewell, "Safe travels."),
                ] {
                    hint_small(ui, label);
                    ui.add(egui::TextEdit::multiline(text).hint_text(hint).desired_rows(1).desired_width(f32::INFINITY));
                }
                hint_small(ui, "Said over its head while you walk past; exported with it.");
            });
        }
        NpcTab::Moves => {
            let inherit = n
                .life
                .character
                .as_ref()
                .and_then(|c| d.chars.get(c))
                .map(|e| e.meta.moves.clone())
                .unwrap_or_default();
            hint_small(ui, "Its own choice of clips; the rest are its model's.");
            moves_editor(ui, d, &mut n.life.moves, Some(&inherit), "npc_moves");
        }
    }
    if n != orig {
        d.gp_edit(now, "Edit NPC", |gp| {
            if let Some(x) = gp.npcs.iter_mut().find(|x| x.id == id) {
                *x = n;
            }
        });
    }
}

fn behavior(ui: &mut egui::Ui, d: &mut ViewData, life: &mut NpcLife, id: u32) {
    let mut b = life.behavior;
    segmented(ui, &mut b, &Behavior::ALL.map(|x| (x, x.name())));
    life.behavior = b;
    dim(ui, b.hint());
    ui.add_space(6.0);
    match b {
        Behavior::Stand => {}
        Behavior::Wander => {
            slider_row(ui, "Area radius", &mut life.wander_radius, 2.0..=80.0, |v| format!("{v:.0} m"));
        }
        Behavior::Patrol => {
            let drawing = d.npc_edit.drawing_route == Some(id);
            stat_row(ui, "Waypoints", &life.route.len().to_string());
            ui.horizontal(|ui| {
                if small_icon_button(ui, "Pencil", if drawing { "Drawing…" } else { "Draw the route" }).clicked() {
                    d.npc_edit.drawing_route = Some(id);
                }
                if !life.route.is_empty() && small_button(ui, "Clear").clicked() {
                    life.route.clear();
                }
            });
            toggle(ui, &mut life.route_loop, "Round and round (off: there and back)");
        }
        Behavior::Schedule => {
            hint_small(ui, &format!("It's {} in the world now.", hhmm(d.lighting.hour)));
            let mut remove = None;
            let mut pick = None;
            let current = life.slot_at(d.lighting.hour).cloned();
            for (k, s) in life.schedule.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let live = current.as_ref() == Some(&*s);
                    ui.add(
                        egui::DragValue::new(&mut s.hour)
                            .range(0.0..=23.98)
                            .speed(0.05)
                            .custom_formatter(|v, _| hhmm(v as f32)),
                    );
                    let mut a = s.activity;
                    egui::ComboBox::from_id_salt(("npc_slot", id, k))
                        .selected_text(a.name())
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for x in Activity::ALL {
                                ui.selectable_value(&mut a, x, x.name());
                            }
                        });
                    s.activity = a;
                    let spot = if s.at.is_some() { "MapPin" } else { "Target" };
                    if small_icon_button(ui, spot, "")
                        .on_hover_text(if s.at.is_some() { "Its spot (click to move)" } else { "At home: click to pick a spot" })
                        .clicked()
                    {
                        pick = Some(k);
                    }
                    if small_icon_button(ui, "CrossSmall", "").on_hover_text("Remove").clicked() {
                        remove = Some(k);
                    }
                    if live {
                        ok_line(ui, "now");
                    }
                });
            }
            if let Some(k) = remove {
                life.schedule.remove(k);
            }
            if let Some(k) = pick {
                d.npc_edit.picking_spot = Some((id, k));
            }
            ui.horizontal(|ui| {
                if small_icon_button(ui, "PlusSmall", "Add a line").clicked() {
                    let hour = life.schedule.iter().map(|s| s.hour).fold(6.0f32, f32::max) + 2.0;
                    life.schedule.push(Slot { hour: hour.min(23.5), activity: Activity::Stand, at: None });
                }
                if small_button(ui, "Fill a typical day").clicked() {
                    life.typical_day();
                }
            });
            slider_row(ui, "Walk-around radius", &mut life.wander_radius, 2.0..=80.0, |v| format!("{v:.0} m"));
        }
    }
    ui.add_space(6.0);
    section_fold(ui, ("npc_more", id), "More", "", |ui| {
        slider_row(ui, "Leash", &mut life.leash, 2.0..=200.0, |v| format!("{v:.0} m"));
        hint_small(ui, "Never further than this from its spot.");
        slider_row(ui, "Walking pace", &mut life.speed, 0.4..=2.5, |v| format!("{v:.1}×"));
    });
    ui.add_space(4.0);
    hint_small(ui, "NPCs live out their behavior while you walk (switch on Live to see it here).");
}

pub(super) fn creature(ui: &mut egui::Ui, d: &mut ViewData, id: u32, now: f64) {
    let Some(orig) = d.gp.creatures.iter().find(|c| c.id == id).cloned() else {
        return;
    };
    let mut c = orig.clone();
    let sub = format!(
        "Level {}–{} · {:?}{}",
        c.level[0],
        c.level[1],
        c.aggression,
        if c.elite { " · elite" } else { "" }
    );
    header(ui, d, c.character.as_deref(), &mut c.name, true, &sub);
    ui.add_space(8.0);
    character_menu(ui, d, "Model", "creature_model", &mut c.character);
    if c.character.is_none() {
        hint_small(ui, "Give it a model: it then roams its spawn areas while you walk.");
    }
    ui.horizontal(|ui| {
        dim(ui, "Family");
        ui.add(egui::TextEdit::singleline(&mut c.family).hint_text("wolf, bandit, spider…").desired_width(f32::INFINITY));
    });
    let mut lo = c.level[0] as f32;
    let mut hi = c.level[1] as f32;
    slider_row(ui, "Lowest level", &mut lo, 1.0..=100.0, |v| format!("{v:.0}"));
    slider_row(ui, "Highest level", &mut hi, 1.0..=100.0, |v| format!("{v:.0}"));
    c.level = [lo.round() as u8, (hi.round() as u8).max(lo.round() as u8)];
    let mut a = c.aggression;
    segmented(
        ui,
        &mut a,
        &[(Aggression::Passive, "Passive"), (Aggression::Neutral, "Neutral"), (Aggression::Hostile, "Hostile")],
    );
    c.aggression = a;
    toggle(ui, &mut c.elite, "Elite");
    hint_small(ui, "Notes");
    ui.add(egui::TextEdit::multiline(&mut c.notes).desired_rows(2).desired_width(f32::INFINITY));
    ui.add_space(8.0);
    // where it lives
    let areas: Vec<(String, f32)> = d
        .gp
        .spawns
        .iter()
        .filter(|s| s.entries.iter().any(|e| e.creature == id))
        .map(|s| (s.name.clone(), s.area() / 1e6))
        .collect();
    section_box(ui, "creature_spawns", "Spawn areas", |ui| {
        if areas.is_empty() {
            dim(ui, "Not in any spawn area yet.");
        }
        for (name, km2) in &areas {
            stat_row(ui, name, &format!("{km2:.2} km²"));
        }
        if small_icon_button(ui, "SelectLasso", "Draw spawn areas").clicked() {
            d.ed.tab = crate::editor::Tab::Gameplay;
        }
        hint_small(ui, "Spawn areas are drawn in Gameplay › Spawns.");
    });
    if let Some(cid) = c.character.clone()
        && small_icon_button(ui, "User", "Open its model").clicked()
    {
        d.v.select_character(&cid);
    }
    ui.add_space(10.0);
    if small_icon_button(ui, "TrashCan", "Delete this creature").clicked() {
        d.gp_edit(now - 10.0, "Delete creature", |gp| {
            gp.creatures.retain(|x| x.id != id);
            for s in &mut gp.spawns {
                s.entries.retain(|e| e.creature != id);
            }
        });
        d.v.sel = None;
        return;
    }
    if c != orig {
        d.gp_edit(now, "Edit creature", |gp| {
            if let Some(x) = gp.creatures.iter_mut().find(|x| x.id == id) {
                *x = c;
            }
        });
    }
}

pub(super) fn player(ui: &mut egui::Ui, d: &mut ViewData, now: f64) {
    let cur = d.world.player.clone();
    let mut name = cur
        .as_ref()
        .and_then(|c| d.chars.get(c))
        .map(|e| e.meta.name.clone())
        .unwrap_or_else(|| "No player character".into());
    header(
        ui,
        d,
        cur.as_deref(),
        &mut name,
        false,
        if cur.is_some() { "Walk mode plays as it" } else { "Walk mode is first person" },
    );
    ui.add_space(8.0);
    let mut c = cur.clone();
    if character_menu(ui, d, "Character", "player_model", &mut c) {
        d.world.player = c;
        d.meta.dirty = true;
    }
    hint_small(ui, "Or click a card on the left.");
    ui.add_space(8.0);
    section_box(ui, "player_view", "View", |ui| {
        toggle(ui, &mut d.view3.third_person, "Third person (P switches while walking)");
        slider_row(ui, "Camera distance", &mut d.view3.distance, 1.5..=8.0, |v| format!("{v:.1} m"));
    });
    ui.add_space(8.0);
    section_box(ui, "player_controls", "Controls", |ui| {
        for (k, what) in [
            ("W A S D", "Move"),
            ("Shift", "Sprint"),
            ("Alt / Caps", "Walk"),
            ("Space", "Jump"),
            ("C", "Crouch"),
            ("Left click", "Light attack"),
            ("Right click", "Heavy attack"),
            ("Q (hold)", "Block"),
            ("X", "Dodge roll"),
            ("P", "Third / first person"),
        ] {
            stat_row(ui, what, k);
        }
        hint_small(ui, "Its moves (which clip for each) are in Models › Moves.");
    });
    ui.add_space(8.0);
    if cur.is_some() && primary_icon_button(ui, "Footsteps", "Walk as it").clicked() {
        d.walk.request = Some(crate::walk::WalkStart::default());
        d.v.note("Walking…", false, now);
    }
    let _ = Sel::Player;
}
