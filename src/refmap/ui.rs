//! Land tab › Trace a Picture: the reference map panel (picture, placement, reading, building),
//! the region list (rename, biome, height role, merge, split, delete) and the viewport tools —
//! drag the picture to move it, its corner handles to scale, its knob to rotate; lasso areas to
//! ignore; draw a line to split a region; click a region to select it (⌫ deletes it).

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, LayerId, Order, RichText, Stroke, pos2, vec2,
};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use egui_phosphor::regular as icon;

use super::analyse::{Op, Role};
use super::build::{self, BuildRequest};
use super::paint::region_color;
use super::{Placement, RefMap, RefRequest, RefTool, RefView};
use crate::biomes::BiomePreset;
use crate::camera::{MainCamera, Pointer};
use crate::editor::{Editor, LandTool, Tab};
use crate::history::History;
use crate::terrain::Terrain;
use crate::tools::Stroke as ToolStroke;
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

pub(super) fn build(app: &mut App) {
    app.add_systems(
        Update,
        (
            pick_picture,
            viewport_tools.in_set(crate::tools::ToolSystems::Apply),
            run_build.after(crate::tools::ToolSystems::Commit),
        ),
    )
    .add_systems(
        EguiPrimaryContextPass,
        draw_viewport.run_if(crate::mcp::ui_visible),
    );
}

/// The biome's short name for a `BiomePreset` name.
pub fn biome_label(name: &str) -> &'static str {
    super::build::preset_of(name).short()
}

fn km(v: f32) -> String {
    if v >= 1000.0 {
        format!("{:.2} km", v / 1000.0)
    } else {
        format!("{v:.0} m")
    }
}

/// The left panel while Trace a Picture is the Land tool.
pub fn reference_left(ui: &mut egui::Ui, s: &mut UiState) {
    let size = s.terrain.size();
    let r = &mut *s.reference;
    let before = r.doc.clone();
    section(ui, "Reference map");
    if !r.has_picture() {
        dim(
            ui,
            "Lay a map picture over the world like tracing paper: concept art, a sketch or a real map (PNG or JPG, north up). The editor reads its coast, water and regions and builds a first draft.",
        );
        ui.add_space(6.0);
        if primary_button(ui, &format!("{}  Load a picture…", icon::IMAGE)).clicked() {
            r.request = Some(RefRequest::Load);
        }
        if let Some(st) = &r.status {
            ui.add_space(4.0);
            ui.label(RichText::new(st).small().color(DANGER));
        }
        return;
    }
    // the picture
    if let Some(pic) = &r.picture {
        let name = pic
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} {name}", icon::IMAGE)).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(
                    ui,
                    icon::TRASH,
                    "Remove the picture (the world stays as it is)",
                )
                .clicked()
                {
                    r.request = Some(RefRequest::Remove);
                }
                if icon_button(ui, icon::FOLDER_OPEN, "Load another picture").clicked() {
                    r.request = Some(RefRequest::Load);
                }
            });
        });
        dim(
            ui,
            &format!(
                "{} × {} px · {:.1} m per px",
                pic.w,
                pic.h,
                r.doc.placement.width / pic.w as f32
            ),
        );
    }
    ui.horizontal(|ui| {
        toggle(ui, &mut r.doc.visible, "Show on the terrain");
    });
    slider_row(ui, "Opacity", &mut r.doc.opacity, 0.05..=1.0, |v| {
        format!("{:.0}%", v * 100.0)
    });
    segmented(
        ui,
        &mut r.doc.view,
        &[
            (RefView::Picture, "Picture"),
            (RefView::Analysis, "Regions"),
        ],
    );
    separator(ui);

    section(ui, "Place");
    segmented(
        ui,
        &mut r.tool,
        &[
            (RefTool::Place, "Move"),
            (RefTool::Ignore, "Ignore"),
            (RefTool::Split, "Split"),
        ],
    );
    dim(
        ui,
        match r.tool {
            RefTool::Place => {
                "Drag the picture to move it; drag a corner to scale, the knob to turn it. Click a region to select it."
            }
            RefTool::Ignore => {
                "Lasso parts of the picture that aren't map (title, legend, lettering). ⌥-click an area to bring it back."
            }
            RefTool::Split => "Draw a line across a region to cut it in two.",
        },
    );
    let p = &mut r.doc.placement;
    egui::Grid::new("ref_place")
        .num_columns(4)
        .spacing(vec2(6.0, 4.0))
        .show(ui, |ui| {
            ui.label(RichText::new("X").color(TEXT_DIM));
            ui.add(
                egui::DragValue::new(&mut p.center[0])
                    .speed(5.0)
                    .suffix(" m"),
            );
            ui.label(RichText::new("Z").color(TEXT_DIM));
            ui.add(
                egui::DragValue::new(&mut p.center[1])
                    .speed(5.0)
                    .suffix(" m"),
            );
            ui.end_row();
            ui.label(RichText::new("Width").color(TEXT_DIM));
            ui.add(
                egui::DragValue::new(&mut p.width)
                    .speed(10.0)
                    .range(50.0..=200_000.0)
                    .suffix(" m"),
            );
            ui.label(RichText::new("Turn").color(TEXT_DIM));
            ui.add(egui::DragValue::new(&mut p.rotation).speed(0.5).suffix("°"));
            ui.end_row();
        });
    slider_row(ui, "Sea margin", &mut r.doc.build.margin, 0.0..=0.25, |v| {
        format!("{:.0}%", v * 100.0)
    });
    if wide_button(ui, &format!("{}  Fit to the world", icon::ARROWS_OUT))
        .on_hover_text(
            "Fill the world with the picture, keeping its proportions and the sea margin",
        )
        .clicked()
    {
        let aspect = r.aspect();
        r.doc.placement = Placement::fit(size, aspect, r.doc.build.margin);
    }
    separator(ui);

    section(ui, "Read the picture");
    let mut n = r.doc.settings.regions as f32;
    slider_row(ui, "Regions", &mut n, 0.0..=24.0, |v| {
        if v < 0.5 {
            "Auto".into()
        } else {
            format!("{v:.0}")
        }
    })
    .on_hover_text("How many land regions to find (Auto picks by how varied the picture is)");
    r.doc.settings.regions = n.round() as u32;
    slider_row(
        ui,
        "Sensitivity",
        &mut r.doc.settings.sensitivity,
        0.0..=1.0,
        |v| format!("{:.0}%", v * 100.0),
    )
    .on_hover_text("Higher keeps smaller islets, rivers and regions");
    toggle(ui, &mut r.doc.settings.auto_ignore, "Ignore legend panels")
        .on_hover_text("Framed boxes such as a map legend are left out");
    let status = if r.analysing() {
        format!("{} Reading the picture…", icon::HOURGLASS)
    } else if let Some(a) = &r.analysis {
        format!(
            "{} regions · {} lakes · {} rivers",
            a.regions.len(),
            a.lakes.len(),
            a.rivers.len()
        )
    } else {
        String::new()
    };
    ui.label(RichText::new(status).color(TEXT));
    if !r.doc.ignore.is_empty() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} ignore areas", r.doc.ignore.len()))
                    .small()
                    .color(TEXT_DIM),
            );
            if ui.small_button("Clear").clicked() {
                r.doc.ignore.clear();
            }
        });
    }
    if !r.doc.ops.is_empty() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} region edits", r.doc.ops.len()))
                    .small()
                    .color(TEXT_DIM),
            );
            if ui
                .small_button("Reset")
                .on_hover_text("Back to the regions as read")
                .clicked()
            {
                r.doc.ops.clear();
            }
        });
    }
    separator(ui);

    section(ui, "Build the draft");
    let b = &mut r.doc.build;
    ui.label(RichText::new("World").color(TEXT_DIM));
    segmented(
        ui,
        &mut b.world_size,
        &[
            (0.0, "This one"),
            (2048.0, "2 km"),
            (4096.0, "4 km"),
            (8192.0, "8 km"),
        ],
    );
    slider_row(
        ui,
        "Mountain height",
        &mut b.relief.peak_height,
        0.0..=1500.0,
        |v| {
            if v < 1.0 {
                "Auto".into()
            } else {
                format!("{v:.0} m")
            }
        },
    );
    slider_row(ui, "Erosion", &mut b.relief.erosion, 0.0..=1.0, |v| {
        format!("{:.0}%", v * 100.0)
    });
    slider_row(
        ui,
        "Coast roughness",
        &mut b.relief.roughness,
        0.0..=1.0,
        |v| format!("{:.0}%", v * 100.0),
    );
    toggle(
        ui,
        &mut b.relief.shading,
        "Relief from the picture's shading",
    )
    .on_hover_text("Painted hill shading adds detail to hills and mountains");
    toggle(ui, &mut b.vegetation, "Grow vegetation");
    toggle(ui, &mut b.zones, "Create a zone per region")
        .on_hover_text("Zones are yours to define: off unless you want the regions as zones");
    ui.add_space(4.0);
    let label = if r.doc.build.world_size > 0.0 {
        format!(
            "{}  Build a new {} world",
            icon::SPARKLE,
            km(r.doc.build.world_size)
        )
    } else {
        format!("{}  Build draft world", icon::SPARKLE)
    };
    if primary_button(ui, &label)
        .on_hover_text(
            "Coast, heights, biomes, lakes and rivers from the picture, as one undo step",
        )
        .clicked()
    {
        r.request = Some(RefRequest::Build);
    }
    dim(
        ui,
        "Replaces the ground, water and nature with the draft (structures stay). Takes a few seconds; undo goes back in one step.",
    );

    // edits made in the panel: one undo step per drag / click (showing, hiding and opacity are
    // views, not edits)
    let mut edited = before.clone();
    (edited.opacity, edited.visible, edited.view) = (r.doc.opacity, r.doc.visible, r.doc.view);
    if r.doc != edited && r.drag_before.is_none() {
        r.drag_before = Some(edited);
    }
    if r.drag_before.is_some()
        && !ui.input(|i| i.pointer.any_down())
        && !ui.ctx().egui_wants_keyboard_input()
    {
        r.commit_drag(&mut s.history, "Reference map");
    }
}

/// The right panel while Trace a Picture is the Land tool: the regions.
pub fn reference_right(ui: &mut egui::Ui, s: &mut UiState) {
    let r = &mut *s.reference;
    title(ui, "Regions");
    let Some(an) = r.analysis.clone() else {
        dim(
            ui,
            if r.analysing() {
                "Reading the picture…"
            } else {
                "Load a picture to see its regions here."
            },
        );
        return;
    };
    dim(
        ui,
        "Each region becomes a biome with its own lie of the land. Click one here or on the map.",
    );
    ui.add_space(4.0);
    let mut op: Option<(String, Op)> = None;
    for (i, g) in an.regions.iter().enumerate() {
        let sel = r.selected == Some(i);
        let c = region_color(i);
        let resp = ui
            .horizontal(|ui| {
                let (sw, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    sw,
                    CornerRadius::same(4),
                    Color32::from_rgb(c[0], c[1], c[2]),
                );
                let text = RichText::new(format!("{}  {}", g.id, g.name)).color(if sel {
                    Color32::WHITE
                } else {
                    TEXT
                });
                let l = ui.selectable_label(sel, text);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{:.0}%", g.frac * 100.0))
                            .small()
                            .color(TEXT_DIM),
                    );
                });
                l
            })
            .inner;
        if resp.clicked() {
            r.selected = if sel { None } else { Some(i) };
        }
        ui.label(
            RichText::new(format!(
                "      {} · {}",
                biome_label(&g.biome),
                g.role.name()
            ))
            .small()
            .color(TEXT_DIM),
        );
        if !sel {
            continue;
        }
        // details of the selected region
        egui::Frame::new()
            .fill(CARD_BG)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let buf_id = Id::new(("ref_name", i, g.id));
                let mut name = ui
                    .data_mut(|d| d.get_temp::<String>(buf_id))
                    .unwrap_or_else(|| g.name.clone());
                let te = ui.add(egui::TextEdit::singleline(&mut name).hint_text("Name"));
                if te.changed() {
                    ui.data_mut(|d| d.insert_temp(buf_id, name.clone()));
                }
                if te.lost_focus() && name.trim() != g.name && !name.trim().is_empty() {
                    op = Some((
                        "Rename region".into(),
                        Op::Label {
                            at: g.pole,
                            name: Some(name.trim().to_string()),
                            biome: None,
                            role: None,
                        },
                    ));
                    ui.data_mut(|d| d.remove::<String>(buf_id));
                }
                let cur = super::build::preset_of(&g.biome);
                egui::ComboBox::from_id_salt(("ref_biome", i))
                    .selected_text(format!("{} {}", cur.icon(), cur.def().name))
                    .width(ui.available_width())
                    .dropdown_ui(ui, |ui| {
                        for b in BiomePreset::ALL {
                            if menu_check(ui, b == cur, format!("{} {}", b.icon(), b.def().name))
                                .clicked()
                                && b != cur
                            {
                                op = Some((
                                    "Region biome".into(),
                                    Op::Label {
                                        at: g.pole,
                                        name: None,
                                        biome: Some(format!("{b:?}")),
                                        role: None,
                                    },
                                ));
                            }
                        }
                    });
                egui::ComboBox::from_id_salt(("ref_role", i))
                    .selected_text(format!("Ground: {}", g.role.name()))
                    .width(ui.available_width())
                    .dropdown_ui(ui, |ui| {
                        for ro in Role::ALL {
                            if menu_check(ui, ro == g.role, ro.name()).clicked() && ro != g.role {
                                op = Some((
                                    "Region height".into(),
                                    Op::Label {
                                        at: g.pole,
                                        name: None,
                                        biome: None,
                                        role: Some(ro),
                                    },
                                ));
                            }
                        }
                    });
                if g.biome != g.suggested_biome || g.role != g.suggested_role {
                    dim(
                        ui,
                        &format!(
                            "Suggested: {} · {}",
                            biome_label(&g.suggested_biome),
                            g.suggested_role.name()
                        ),
                    );
                }
                egui::ComboBox::from_id_salt(("ref_merge", i))
                    .selected_text("Merge with…")
                    .width(ui.available_width())
                    .dropdown_ui(ui, |ui| {
                        for (j, o) in an.regions.iter().enumerate() {
                            if j != i
                                && menu_check(ui, false, format!("{}  {}", o.id, o.name)).clicked()
                            {
                                op = Some((
                                    "Merge regions".into(),
                                    Op::Merge {
                                        a: g.pole,
                                        b: o.pole,
                                    },
                                ));
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{} Split", icon::SCISSORS))
                        .on_hover_text("Then draw a line across the region on the map")
                        .clicked()
                    {
                        r.tool = RefTool::Split;
                    }
                    if ui
                        .button(format!("{} Delete", icon::TRASH))
                        .on_hover_text("Not a region: it becomes what surrounds it (⌫)")
                        .clicked()
                    {
                        op = Some(("Delete region".into(), Op::Delete { at: g.pole }));
                    }
                });
            });
    }
    if let Some((label, o)) = op {
        r.push_op(&mut s.history, &label, o);
    }
}

/// The file dialog for a picture (it answers on a later frame, `crate::dialogs`), and removing it.
fn pick_picture(
    _main: NonSendMarker,
    mut r: ResMut<RefMap>,
    terrain: Res<Terrain>,
    mut history: ResMut<History>,
    mut ed: ResMut<Editor>,
) {
    // (the dialog answers on a later frame)
    if let Some(path) = crate::dialogs::take_file("refmap_picture") {
        let before = r.snapshot();
        match r.load(&path, terrain.size()) {
            Ok(()) => {
                r.record(&mut history, "Load reference picture", before);
                ed.toast("Picture laid over the world: reading its regions…");
            }
            Err(e) => r.status = Some(e),
        }
        return;
    }
    match r.request {
        Some(RefRequest::Load) => {
            r.request = None;
            crate::dialogs::pick_file(
                "refmap_picture",
                rfd::AsyncFileDialog::new()
                    .set_title("Load a map picture")
                    .add_filter("Pictures", &["png", "jpg", "jpeg"]),
            );
        }
        Some(RefRequest::Remove) => {
            r.request = None;
            let before = r.snapshot();
            r.clear();
            r.record(&mut history, "Remove reference picture", before);
            ed.toast("Reference picture removed (undo brings it back)");
        }
        _ => {}
    }
}

/// The Build button (needs the whole world). The build blocks for a few seconds, so the "Building"
/// toast is shown for a couple of frames first.
fn run_build(world: &mut World, mut armed: Local<u8>) {
    if world.resource::<RefMap>().request != Some(RefRequest::Build) {
        *armed = 0;
        return;
    }
    *armed += 1;
    if *armed == 1 {
        world
            .resource_mut::<Editor>()
            .toast("Building the draft world…");
    }
    if *armed < 3 {
        return;
    }
    *armed = 0;
    let (settings, seed, art_style, appearance) = {
        let mut r = world.resource_mut::<RefMap>();
        r.request = None;
        let settings = r.doc.build.clone();
        let art_style = (settings.world_size > 0.0)
            .then_some(r.new_world_art_style)
            .flatten();
        let appearance = (settings.world_size > 0.0)
            .then_some(r.new_world_appearance)
            .flatten();
        (settings, fastrand::u32(0..999_999), art_style, appearance)
    };
    let new_size = (settings.world_size > 0.0).then_some(settings.world_size);
    if let Err(e) = build::build(
        world,
        BuildRequest {
            new_size,
            settings,
            seed,
            art_style,
            appearance,
        },
    ) {
        world
            .resource_mut::<Editor>()
            .toast(format!("Couldn't build: {e}"));
    }
}

#[derive(Default, Clone, Copy)]
enum Drag {
    #[default]
    None,
    Move {
        center: [f32; 2],
        hit: Vec2,
    },
    Scale {
        width: f32,
        dist: f32,
    },
    Rotate {
        rotation: f32,
        angle: f32,
    },
}

/// Screen positions of the picture's corners and rotation knob.
fn handles(
    r: &RefMap,
    t: &Terrain,
    cam: &Camera,
    cam_t: &GlobalTransform,
) -> Option<([Vec2; 4], Vec2)> {
    let aspect = r.aspect();
    let p = &r.doc.placement;
    let ground =
        |w: [f32; 2]| Vec3::new(w[0], t.height_at(w[0], w[1]).max(t.sea_level) + 2.0, w[1]);
    let corners = p
        .corners(aspect)
        .map(|c| cam.world_to_viewport(cam_t, ground(c)).ok());
    let knob_w = p.to_world([0.5, -0.06 / aspect.max(0.2)], aspect);
    let knob = cam.world_to_viewport(cam_t, ground(knob_w)).ok()?;
    Some(([corners[0]?, corners[1]?, corners[2]?, corners[3]?], knob))
}

#[allow(clippy::too_many_arguments)]
fn viewport_tools(
    stroke: Res<ToolStroke>,
    pointer: Res<Pointer>,
    keys: Res<ButtonInput<KeyCode>>,
    mut r: ResMut<RefMap>,
    ed: Res<Editor>,
    terrain: Res<Terrain>,
    mut history: ResMut<History>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut drag: Local<Drag>,
) {
    if !(ed.tab == Tab::Land && ed.land_tool == LandTool::Reference) || !r.has_picture() {
        return;
    }
    let aspect = r.aspect();
    if !pointer.over_ui
        && !pointer.keyboard_captured
        && (keys.just_pressed(KeyCode::Backspace) || keys.just_pressed(KeyCode::Delete))
        && let (Some(i), Some(an)) = (r.selected, r.analysis.clone())
        && let Some(g) = an.regions.get(i)
    {
        r.push_op(&mut history, "Delete region", Op::Delete { at: g.pole });
        r.selected = None;
    }
    if stroke.just_started {
        *drag = Drag::None;
        if r.tool == RefTool::Place
            && let Some(hit) = stroke.start_hit
        {
            let c = Vec2::from_array(r.doc.placement.center);
            if let (Some(screen), Ok((cam, cam_t))) = (pointer.screen, camera.single())
                && let Some((corners, knob)) = handles(&r, &terrain, cam, cam_t)
            {
                let near = |q: Vec2| q.distance(screen) < 14.0;
                if near(knob) {
                    *drag = Drag::Rotate {
                        rotation: r.doc.placement.rotation,
                        angle: (hit.xz() - c).to_angle(),
                    };
                } else if corners.iter().any(|q| near(*q)) {
                    *drag = Drag::Scale {
                        width: r.doc.placement.width,
                        dist: hit.xz().distance(c).max(1.0),
                    };
                }
            }
            if matches!(*drag, Drag::None) {
                let uv = r.doc.placement.to_uv(hit.xz().to_array(), aspect);
                if (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]) {
                    *drag = Drag::Move {
                        center: r.doc.placement.center,
                        hit: hit.xz(),
                    };
                }
            }
            r.drag_before = Some(r.doc.clone());
        }
    }
    if stroke.active
        && let Some(hit) = stroke.hit
    {
        let c = Vec2::from_array(r.doc.placement.center);
        let moved = stroke.path.len() > 2;
        match *drag {
            Drag::Move { center, hit: h0 } if moved => {
                let d = hit.xz() - h0;
                r.doc.placement.center = [center[0] + d.x, center[1] + d.y];
            }
            Drag::Scale { width, dist } => {
                r.doc.placement.width = (width * hit.xz().distance(c) / dist).max(50.0)
            }
            Drag::Rotate { rotation, angle } => {
                let a = (hit.xz() - c).to_angle();
                r.doc.placement.rotation = (rotation + (a - angle).to_degrees()).rem_euclid(360.0);
            }
            _ => {}
        }
    }
    if !stroke.just_ended {
        return;
    }
    let path: Vec<[f32; 2]> = stroke
        .path
        .iter()
        .map(|p| r.doc.placement.to_uv(p.xz().to_array(), aspect))
        .collect();
    let click = stroke.path.len() <= 3;
    match r.tool {
        RefTool::Place => {
            if click {
                r.drag_before = None;
                if let (Some(an), Some(h)) = (r.analysis.clone(), stroke.start_hit) {
                    r.selected = an.region_at(r.doc.placement.to_uv(h.xz().to_array(), aspect));
                }
            } else {
                r.commit_drag(&mut history, "Place reference picture");
            }
        }
        RefTool::Ignore => {
            if click || stroke.inverse {
                // ⌥-click: bring an ignored area back
                if let Some(h) = stroke.start_hit {
                    let uv = r.doc.placement.to_uv(h.xz().to_array(), aspect);
                    r.edit(&mut history, "Remove ignore area", |d| {
                        let n = d.ignore.len();
                        d.ignore.retain(|p| !super::paint::in_poly(p, uv));
                        d.ignore.len() != n
                    });
                }
            } else if path.len() >= 4 {
                let mut poly = path.clone();
                poly.dedup();
                r.edit(&mut history, "Ignore area", |d| {
                    d.ignore.push(poly);
                    true
                });
            }
        }
        RefTool::Split => {
            if path.len() >= 2 && !click {
                r.push_op(&mut history, "Split region", Op::Split { line: path });
            }
        }
    }
    *drag = Drag::None;
}

/// The picture's frame and handles, the line being drawn, ignore areas and region labels.
#[allow(clippy::too_many_arguments)]
fn draw_viewport(
    mut contexts: EguiContexts,
    r: Res<RefMap>,
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    terrain: Res<Terrain>,
    stroke: Res<ToolStroke>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) -> Result {
    if !(ed.tab == Tab::Land && ed.land_tool == LandTool::Reference)
        || !r.has_picture()
        || walk.active
        || walk.cinematic
    {
        return Ok(());
    }
    let Ok((cam, cam_t)) = camera.single() else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    if !ctx.fonts(|f| f.definitions().families.contains_key(&semibold())) {
        return Ok(());
    }
    let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::new("refmap")));
    let aspect = r.aspect();
    let place = r.doc.placement;
    let t = &*terrain;
    let screen = |uv: [f32; 2]| -> Option<egui::Pos2> {
        let w = place.to_world(uv, aspect);
        let v = cam
            .world_to_viewport(
                cam_t,
                Vec3::new(w[0], t.height_at(w[0], w[1]).max(t.sea_level) + 2.0, w[1]),
            )
            .ok()?;
        Some(pos2(v.x, v.y))
    };
    // frame
    let mut frame = vec![];
    for (a, b) in [
        ([0.0, 0.0], [1.0, 0.0]),
        ([1.0, 0.0], [1.0, 1.0]),
        ([1.0, 1.0], [0.0, 1.0]),
        ([0.0, 1.0], [0.0, 0.0]),
    ] {
        for s in 0..24 {
            let f = s as f32 / 24.0;
            if let Some(p) = screen([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]) {
                frame.push(p);
            }
        }
    }
    if let Some(f) = frame.first().copied() {
        frame.push(f);
    }
    painter.add(egui::Shape::line(
        frame,
        Stroke::new(1.5, Color32::from_white_alpha(170)),
    ));
    if r.tool == RefTool::Place
        && let Some((corners, knob)) = handles(&r, t, cam, cam_t)
    {
        for c in corners {
            painter.circle(
                pos2(c.x, c.y),
                6.0,
                Color32::WHITE,
                Stroke::new(1.5, ACCENT),
            );
        }
        if let Some(top) = screen([0.5, 0.0]) {
            painter.line_segment(
                [top, pos2(knob.x, knob.y)],
                Stroke::new(1.5, Color32::from_white_alpha(170)),
            );
        }
        painter.circle(
            pos2(knob.x, knob.y),
            7.0,
            ACCENT,
            Stroke::new(1.5, Color32::WHITE),
        );
    }
    // ignore areas
    for poly in &r.doc.ignore {
        let mut pts: Vec<egui::Pos2> = poly.iter().filter_map(|q| screen(*q)).collect();
        if let Some(f) = pts.first().copied() {
            pts.push(f);
        }
        painter.add(egui::Shape::dashed_line(
            &pts,
            Stroke::new(1.5, Color32::from_rgb(230, 230, 230)),
            7.0,
            5.0,
        ));
    }
    // the line being drawn
    if stroke.active && matches!(r.tool, RefTool::Ignore | RefTool::Split) && stroke.path.len() >= 2
    {
        let pts: Vec<egui::Pos2> = stroke
            .path
            .iter()
            .filter_map(|p| cam.world_to_viewport(cam_t, *p + Vec3::Y).ok())
            .map(|v| pos2(v.x, v.y))
            .collect();
        let col = if r.tool == RefTool::Split {
            Color32::from_rgb(255, 215, 110)
        } else {
            Color32::from_rgb(235, 235, 235)
        };
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.5, col)));
        if r.tool == RefTool::Ignore
            && let (Some(a), Some(b)) = (pts.first(), pts.last())
        {
            painter.add(egui::Shape::dashed_line(
                &[*b, *a],
                Stroke::new(1.5, col.gamma_multiply(0.7)),
                8.0,
                6.0,
            ));
        }
    }
    // region labels
    if r.doc.view == RefView::Analysis
        && r.doc.visible
        && let Some(an) = &r.analysis
    {
        let mut taken: Vec<egui::Rect> = vec![];
        for (i, g) in an.regions.iter().enumerate() {
            let Some(p0) = screen(g.pole) else { continue };
            if !ctx.content_rect().contains(p0) {
                continue;
            }
            // the first free spot near the pole (larger regions are labelled first)
            let Some(p) = [0.0, 1.0, -1.0, 2.0, -2.0]
                .iter()
                .map(|k| p0 + vec2(0.0, k * 42.0))
                .find(|q| {
                    !taken
                        .iter()
                        .any(|t| t.intersects(egui::Rect::from_center_size(*q, vec2(120.0, 38.0))))
                })
            else {
                continue;
            };
            let sel = r.selected == Some(i);
            let f1 = FontId::new(13.0, semibold());
            let f2 = FontId::proportional(11.0);
            let sub = format!("{} · {}", biome_label(&g.biome), g.role.name());
            let g1 =
                painter.layout_no_wrap(format!("{} {}", g.id, g.name), f1.clone(), Color32::WHITE);
            let g2 = painter.layout_no_wrap(sub.clone(), f2.clone(), Color32::WHITE);
            let rect =
                egui::Rect::from_center_size(p, vec2(g1.size().x.max(g2.size().x) + 18.0, 38.0));
            taken.push(rect);
            let c = region_color(i);
            painter.rect_filled(
                rect,
                CornerRadius::same(7),
                Color32::from_rgba_unmultiplied(12, 14, 18, if sel { 235 } else { 190 }),
            );
            painter.rect_stroke(
                rect,
                CornerRadius::same(7),
                Stroke::new(
                    if sel { 2.5 } else { 1.5 },
                    Color32::from_rgb(c[0], c[1], c[2]),
                ),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center() - vec2(0.0, 7.0),
                Align2::CENTER_CENTER,
                format!("{} {}", g.id, g.name),
                f1,
                Color32::WHITE,
            );
            painter.text(
                rect.center() + vec2(0.0, 9.0),
                Align2::CENTER_CENTER,
                sub,
                f2,
                Color32::from_rgb(210, 216, 226),
            );
        }
    }
    Ok(())
}
