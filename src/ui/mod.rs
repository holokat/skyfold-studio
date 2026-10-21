//! Editor UI (egui): top bar, vertical tool rail, contextual tool panel + inspector,
//! bottom time-of-day bar, and viewport overlays (zone labels, brush info, toasts).

mod auto_zones;
pub mod icons;
pub mod palette;
mod panels;
pub mod pictures;
pub mod shell;
pub mod theme;
pub mod tips;
pub mod widgets;
mod zone_labels;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Order, RichText, Stroke, pos2, vec2,
};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use egui_phosphor::regular as icon;

use crate::audio::{AmbienceState, AudioSettings, SoundEvent};
use crate::camera::{MainCamera, OrbitCamera, Pointer};
use crate::editor::{Editor, Tab};
use crate::history::History;
use crate::objects::{Category, Library, ObjectStore, PlaceSettings, ScatterSettings, Wind};
use crate::sky::{Lighting, SkyState};
use crate::terrain::Terrain;
use crate::terrain::layers::{LAYER_COUNT, Layer};
use crate::terrain::worldgen::WorldPreset;
use crate::tools::{EditorAction, Stroke as ToolStroke};
use crate::walk::WalkState;
use crate::water::{WaterBodies, WaterStyle};
use crate::world::{WorldMeta, Zones};
use crate::zones::ZoneStats;
use theme::*;
use widgets::*;

/// Actions requested by the UI, handled by `io` and friends.
#[derive(Message, Clone, Debug)]
pub enum UiAction {
    /// `biomes`: also generate climate zones, ground, vegetation and water styles
    NewWorld {
        preset: WorldPreset,
        res: usize,
        cell: f32,
        seed: u32,
        biomes: Option<crate::biomegen::Climate>,
        art_style: crate::art_style::ArtStyle,
        appearance: crate::nature::NatureAppearance,
    },
    /// File › New › From a picture: build a new world of `size_m` from the picture (a map,
    /// sketch or painting), via the reference-map builder
    NewWorldFromPicture {
        path: std::path::PathBuf,
        size_m: f32,
        art_style: crate::art_style::ArtStyle,
        appearance: crate::nature::NatureAppearance,
    },
    Regenerate {
        preset: WorldPreset,
        seed: u32,
        biomes: Option<crate::biomegen::Climate>,
    },
    Save,
    SaveAs,
    Open,
    Export,
    ImportModel,
    /// File › Save / Save As answered: save there
    SaveTo(std::path::PathBuf),
    /// File › Open answered: open that world
    OpenPath(std::path::PathBuf),
    /// models downloaded from the marketplace (`from`: the listing's title), imported like File › Import
    ImportFiles {
        files: Vec<std::path::PathBuf>,
        from: String,
    },
    ToggleWalk,
    ErodeWorld,
    SmoothWorld,
    ClearPaint,
    PopulateWorld,
    ClearNature,
}

/// Halo layout: the top islands end at y 48 (panels start 12 below), the rail at x 50 (the tool
/// panel starts 12 right of it); the tool panel and inspector widths.
pub const TOP_H: f32 = shell::TOP - 12.0;
pub const RAIL_W: f32 = shell::PANEL_X - 12.0;
pub const LEFT_W: f32 = shell::PANEL_W;
pub const RIGHT_W: f32 = shell::INSP_W;

#[derive(SystemParam)]
pub struct UiState<'w> {
    pub ed: ResMut<'w, Editor>,
    pub terrain: ResMut<'w, Terrain>,
    pub zones: ResMut<'w, Zones>,
    pub auto_zones: ResMut<'w, crate::zones::auto::AutoZones>,
    pub lighting: ResMut<'w, Lighting>,
    pub sky: Res<'w, SkyState>,
    pub lib: Option<Res<'w, Library>>,
    pub store: ResMut<'w, ObjectStore>,
    pub scatter: ResMut<'w, ScatterSettings>,
    pub place: ResMut<'w, PlaceSettings>,
    pub water: ResMut<'w, WaterBodies>,
    pub water_style: ResMut<'w, WaterStyle>,
    pub history: ResMut<'w, History>,
    pub meta: ResMut<'w, WorldMeta>,
    pub wind: ResMut<'w, Wind>,
    pub audio: ResMut<'w, AudioSettings>,
    pub ambience: ResMut<'w, AmbienceState>,
    pub walk: Res<'w, WalkState>,
    pub orbit: ResMut<'w, OrbitCamera>,
    pub pointer: Res<'w, Pointer>,
    pub stats: Res<'w, ZoneStats>,
    pub stroke: Res<'w, ToolStroke>,
    pub motion: ResMut<'w, crate::wind::Motion>,
    pub graphics: ResMut<'w, crate::streaming::Graphics>,
    pub amounts: ResMut<'w, crate::objects::NatureAmounts>,
    pub appearance: ResMut<'w, crate::nature::NatureAppearance>,
    pub surface_lib: Option<Res<'w, crate::surfaces::SurfaceLibrary>>,
    pub surface_choices: ResMut<'w, crate::surfaces::SurfaceChoices>,
    pub terrain_lib: Res<'w, crate::terrain::textures::TerrainLibrary>,
    pub layer_choices: ResMut<'w, crate::terrain::textures::TerrainLayerChoices>,
    pub gameplay: ResMut<'w, crate::gameplay::Gameplay>,
    pub gp_ed: ResMut<'w, crate::gameplay::GameplayEditor>,
    pub roads: ResMut<'w, crate::roads::Roads>,
    pub road_tool: ResMut<'w, crate::roads::RoadTool>,
    /// fitted objects' pads (the selected object's "Level the ground" switch)
    pub fit: crate::placement::FitParams<'w>,
    pub analysis: ResMut<'w, crate::analysis::Analysis>,
    pub autosave: ResMut<'w, crate::io::Autosave>,
    pub map: ResMut<'w, crate::worldmap::WorldMap>,
    pub reference: ResMut<'w, crate::refmap::RefMap>,
    pub frames: Res<'w, crate::frame_stats::FrameStats>,
    pub real_time: Res<'w, Time<Real>>,
    pub bigworld: ResMut<'w, crate::bigworld::BigWorld>,
    pub mcp: ResMut<'w, crate::mcp::McpServer>,
    pub prompt: crate::prompt::PromptUi<'w>,
    /// procedural nature: painted density and the picked instance
    pub nature: ResMut<'w, crate::nature::NatureField>,
    pub nature_brush: ResMut<'w, crate::nature::brush::NatureBrush>,
    pub nature_rules: Option<Res<'w, crate::nature::Rules>>,
    pub nature_masks: Res<'w, crate::nature::Masks>,
    /// rendered thumbnails and the live preview (crate::thumbs)
    pub previews: ResMut<'w, crate::thumbs::Previews>,
    pub gallery: ResMut<'w, panels::gallery::Gallery>,
    /// Objects › Set pieces (graveyard, boneyard…)
    pub sets: ResMut<'w, crate::setpieces::SetPieceSettings>,
    /// Objects › Set pieces › Village / Town (crate::towns)
    pub towns: ResMut<'w, crate::towns::TownSettings>,
    /// Asset packs, pack cards and the Objects tab's pack filter (crate::market)
    pub market: ResMut<'w, crate::market::ui::MarketUi>,
    /// Objects › Scenes (crate::scenes)
    pub scenes: ResMut<'w, crate::scenes::Scenes>,
    /// Nature › Fields (crate::fields)
    pub fields: ResMut<'w, crate::fields::Fields>,
    pub field_tool: ResMut<'w, crate::fields::ui::FieldTool>,
    /// Terrain › Caves (crate::caves)
    pub caves: Res<'w, crate::caves::Caves>,
    pub cave_tool: ResMut<'w, crate::caves::ui::CaveTool>,
    pub cave_world: ResMut<'w, crate::caves::render::CaveWorld>,
    /// Dungeons (crate::dungeons): the world's list, the open dungeon's map, the builder
    pub dungeons: ResMut<'w, crate::dungeons::Dungeons>,
    pub dungeon_map: ResMut<'w, crate::dungeons::DungeonMap>,
    pub dungeon_tool: ResMut<'w, crate::dungeons::ui::DungeonTool>,
    pub sheet_ui: ResMut<'w, crate::dungeons::sheets::ui::SheetUi>,
    pub dungeon_view: ResMut<'w, crate::dungeons::render::DungeonView>,
    pub dungeon_world: Res<'w, crate::dungeons::render::DungeonWorld>,
    pub dungeon_session: Res<'w, crate::dungeons::session::Session>,
    /// the editor's work light (B)
    pub work_light: ResMut<'w, crate::camera::WorkLight>,
    /// Lighting › Performance › Render resolution (crate::render_scale)
    pub render_scale: ResMut<'w, crate::render_scale::RenderScale>,
    /// Lighting › Performance › Save power in the background (crate::background)
    pub background: ResMut<'w, crate::background::BackgroundThrottle>,
}

#[derive(SystemParam)]
pub struct UiOut<'w> {
    pub actions: MessageWriter<'w, UiAction>,
    pub editor_actions: MessageWriter<'w, EditorAction>,
    pub sounds: MessageWriter<'w, SoundEvent>,
    pub biome: MessageWriter<'w, crate::biomes::ApplyBiomeToZone>,
    pub motion: MessageWriter<'w, crate::motion::MotionAction>,
}

/// Cached egui textures (layer previews; asset thumbnails live in `crate::thumbs`).
#[derive(Default)]
pub struct UiTextures {
    pub layers: Vec<egui::TextureHandle>,
    pub installed: bool,
    pub real_swatches: bool,
    /// terrain texture revision the swatches were made from
    pub swatch_rev: u32,
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<UiAction>()
            .init_resource::<panels::gallery::Gallery>()
            .add_systems(
                EguiPrimaryContextPass,
                editor_ui.run_if(crate::mcp::ui_visible),
            );
    }
}

#[allow(clippy::too_many_arguments)]
fn editor_ui(
    // native file dialogs (exports, imports) must open on the main thread on macOS
    _main: bevy::ecs::system::NonSendMarker,
    mut contexts: EguiContexts,
    mut s: UiState,
    mut out: UiOut,
    mut tex: Local<UiTextures>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    time: Res<Time>,
    terrain_tex: Res<crate::terrain::textures::TerrainTextures>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    if !tex.installed {
        theme::install(&ctx);
        tex.layers = (0..LAYER_COUNT)
            .map(|i| make_layer_preview(&ctx, Layer::from_index(i)))
            .collect();
        tex.installed = true;
        // fonts registered this frame only become usable next frame
        return Ok(());
    }
    if (!tex.real_swatches || tex.swatch_rev != terrain_tex.revision)
        && terrain_tex.status == crate::terrain::textures::TextureStatus::Ready
    {
        tex.swatch_rev = terrain_tex.revision;
        for (i, sw) in terrain_tex.swatches.iter().enumerate() {
            if let Some((w, h, px)) = sw {
                let img = egui::ColorImage::from_rgba_unmultiplied([*w as usize, *h as usize], px);
                tex.layers[i] =
                    ctx.load_texture(format!("layer_real_{i}"), img, egui::TextureOptions::LINEAR);
            }
        }
        tex.real_swatches = true;
    }
    let screen = ctx.content_rect();

    // a camera path is playing (or being captured): nothing on top of the picture
    if s.walk.cinematic {
        return Ok(());
    }
    if s.walk.active {
        walk_hud(&ctx, &mut s, screen);
        shell::toast(&ctx, &s, screen);
        return Ok(());
    }

    shell::tab_key(&ctx);
    shell::top(&ctx, &mut s, &mut out, screen);
    shell::rail(&ctx, &mut s, &mut out, screen);
    panels::left_panel(&ctx, &mut s, &mut out, &tex, screen);
    let inspector = panels::right_panel(&ctx, &mut s, &mut out, &tex, screen);
    panels::dock(&ctx, &mut s, &tex, screen, inspector);
    panels::poll(&mut s);
    let _ = &diagnostics;
    crate::caves::ui::inside_bar(&ctx, &mut s, screen);
    crate::dungeons::ui::map_bar(&ctx, &mut s, screen);
    crate::dungeons::ui::guide_bar(&ctx, &mut s);
    if let Ok((cam, cam_t)) = camera.single() {
        zone_labels::draw(&ctx, &s, cam, cam_t, inspector);
        shell::status(&ctx, &s, cam, cam_t, screen);
        coast_preview(&ctx, &s, cam, cam_t);
    }
    brush_tooltip(&ctx, &s);
    shell::toast(&ctx, &s, screen);
    auto_zones::show(&ctx, &mut s, screen);
    palette::show(&ctx, &mut s, &mut out);
    if s.ed.show_new_world {
        new_world_dialog(&ctx, &mut s, &mut out);
    }
    if s.gallery.open {
        panels::gallery::window(&ctx, &mut s);
    }
    crate::market::ui::windows(&ctx, &mut s, &mut out);
    crate::scenes::ui::windows(&ctx, &mut s);
    if s.ed.show_help {
        help_window(&ctx, &mut s);
    }
    if s.mcp.show_panel {
        let now = s.real_time.elapsed_secs();
        crate::mcp::connect_window(&ctx, &mut s.mcp, now);
    }
    if std::env::var_os("WB_WIDGETS").is_some() {
        widgets::widget_gallery(&ctx);
    }
    let _ = time;
    Ok(())
}

/// The Skyfold Studio icon for the top bar (decoded once).
fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    static LOGO: std::sync::OnceLock<egui::TextureHandle> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| {
        let img =
            image::load_from_memory(include_bytes!("../../assets/branding/skyfold_icon_128.png"))
                .map(|i| i.to_rgba8())
                .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
        let c = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        );
        ctx.load_texture("skyfold_logo", c, egui::TextureOptions::LINEAR)
    })
    .clone()
}

fn project(cam: &Camera, cam_t: &GlobalTransform, p: Vec3) -> Option<egui::Pos2> {
    let v = cam.world_to_viewport(cam_t, p).ok()?;
    Some(pos2(v.x, v.y))
}

/// While drawing a coastline or river, show the path being drawn.
fn coast_preview(ctx: &egui::Context, s: &UiState, cam: &Camera, cam_t: &GlobalTransform) {
    let drawing = s.stroke.active
        && ((s.ed.tab == Tab::Land && s.ed.land_tool == crate::editor::LandTool::DrawCoast)
            || (s.ed.tab == Tab::Water
                && matches!(
                    s.ed.water_tool,
                    crate::editor::WaterTool::River
                        | crate::editor::WaterTool::DrawLake
                        | crate::editor::WaterTool::Waterfall
                ))
            || (s.ed.tab == Tab::Zones
                && s.ed.zone_tool == crate::editor::ZoneTool::Biome
                && s.ed.biome_outline)
            || set_piece_loop(s)
            || s.field_tool.drawing(&s.ed));
    if !drawing || s.stroke.path.len() < 2 {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        Order::Background,
        Id::new("coast_preview"),
    ));
    let pts: Vec<egui::Pos2> = s
        .stroke
        .path
        .iter()
        .filter_map(|p| project(cam, cam_t, *p + Vec3::Y * 1.0))
        .collect();
    let col = match s.ed.tab {
        Tab::Water => Color32::from_rgb(90, 180, 255),
        Tab::Zones => color32(s.ed.biome_preset.def().color),
        _ => Color32::from_rgb(255, 215, 110),
    };
    painter.add(egui::Shape::line(pts.clone(), Stroke::new(3.0, col)));
    if (matches!(s.ed.tab, Tab::Land | Tab::Zones)
        || (s.ed.tab == Tab::Water && s.ed.water_tool == crate::editor::WaterTool::DrawLake)
        || set_piece_loop(s)
        || s.field_tool.drawing(&s.ed))
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

/// A set piece's area is being drawn (Objects › Set pieces).
fn set_piece_loop(s: &UiState) -> bool {
    s.ed.tab == Tab::Objects
        && s.ed.object_tool == crate::editor::ObjectTool::Place
        && (s.sets.selected.is_some() || s.towns.selected.is_some() || s.scenes.capturing())
}

fn brush_tooltip(ctx: &egui::Context, s: &UiState) {
    let (Some(screen), Some(hit)) = (s.pointer.screen, s.pointer.hit) else {
        return;
    };
    if s.pointer.over_ui {
        return;
    }
    let Some(b) = s.ed.active_brush() else { return };
    let label = match s.ed.tab {
        Tab::Land => match s.ed.land_tool {
            crate::editor::LandTool::PaintLand => "Paint Land".to_string(),
            crate::editor::LandTool::PaintSea => "Paint Sea".to_string(),
            _ => String::new(),
        },
        Tab::Sculpt => s.ed.sculpt_tool.name().to_string(),
        Tab::Paint => format!("Paint {}", s.ed.paint_layer.name()),
        Tab::Nature => match s.scatter.preset {
            Some(i) => crate::objects::presets()
                .get(i)
                .map(|p| p.name.to_string())
                .unwrap_or_default(),
            None => format!("Paint {}", s.scatter.category.name()),
        },
        Tab::Zones => s
            .zones
            .selected
            .and_then(|i| s.zones.list.get(i))
            .map(|z| format!("Zone: {}", z.name))
            .unwrap_or("Select a zone".into()),
        _ => String::new(),
    };
    let alt = hit.y - s.terrain.sea_level;
    let text = format!(
        "{label}\nSize: {:.0} m   Strength: {:.1}\nAltitude: {:.0} m",
        b.size, b.strength, alt
    );
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("brush_tip")));
    let pos = pos2(screen.x + 22.0, screen.y + 18.0);
    let galley = painter.layout(text, FontId::proportional(12.0), Color32::WHITE, 220.0);
    let r = egui::Rect::from_min_size(pos, galley.size() + vec2(16.0, 12.0));
    painter.rect_filled(
        r,
        CornerRadius::same(7),
        Color32::from_rgba_unmultiplied(12, 14, 18, 215),
    );
    painter.galley(pos + vec2(8.0, 6.0), galley, Color32::WHITE);
}

fn walk_hud(ctx: &egui::Context, s: &mut UiState, screen: egui::Rect) {
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("walk_hud")));
    // crosshair
    let c = screen.center();
    painter.circle_stroke(c, 3.0, Stroke::new(1.5, Color32::from_white_alpha(160)));
    // zone + surface readout (top left)
    let zone = s.zones.by_id(s.sky.viewer_zone);
    let biome = crate::biomes::BiomePreset::from_id(
        s.terrain.biome_at(s.walk.position.x, s.walk.position.z),
    )
    .map(|b| b.def().name);
    let zone_name = zone
        .map(|z| z.name.clone())
        .or(biome.map(|b| b.to_string()))
        .unwrap_or("Wilderness".into());
    let lvl = zone.map(|z| z.level_label()).unwrap_or_default();
    let surf = s.walk.surface.map(|x| x.name()).unwrap_or("Swimming");
    let amb = s.ambience.soundscape.map(|x| x.name()).unwrap_or("-");
    let h = s.lighting.hour;
    // temperature: the zone's, else the biome's, cooler at night and in rain
    let base_temp = zone
        .filter(|z| z.biome_preset.is_some() || (z.temperature - 16.0).abs() > 0.01)
        .map(|z| z.temperature)
        .or(crate::biomes::BiomePreset::from_id(
            s.terrain.biome_at(s.walk.position.x, s.walk.position.z),
        )
        .map(|b| b.def().temperature));
    let temp = base_temp
        .map(|t| t - 6.0 * s.sky.night - 3.0 * s.sky.weather.rain)
        .map(|t| format!("\nTemperature: {t:.0} °C"))
        .unwrap_or_default();
    let lines = match s
        .dungeon_map
        .0
        .as_ref()
        .filter(|m| m.kind == crate::dungeons::model::MapKind::Interior)
    {
        // underground: the dungeon and its mood (no sky, no weather)
        Some(m) => format!(
            "{}\nGround: {surf}\nMood: {}",
            m.name,
            crate::dungeons::mood::name(&m.mood.preset)
        ),
        None => format!(
            "{zone_name}  {lvl}\nGround: {surf}\nAmbience: {amb}{}{temp}\nTime: {:02}:{:02}",
            if s.ambience.auto { " (auto)" } else { "" },
            h as u32,
            (h.fract() * 60.0) as u32
        ),
    };
    // (a playtest shows its own session panel there)
    if !s.walk.playtest {
        let galley = painter.layout(lines, FontId::proportional(13.0), Color32::WHITE, 320.0);
        let r = egui::Rect::from_min_size(
            screen.min + vec2(18.0, 18.0),
            galley.size() + vec2(20.0, 16.0),
        );
        painter.rect_filled(
            r,
            CornerRadius::same(9),
            Color32::from_rgba_unmultiplied(10, 12, 16, 190),
        );
        painter.galley(r.min + vec2(10.0, 8.0), galley, Color32::WHITE);
    }
    // frame rate (top right): green at 60, yellow around 40, red below 25
    let fps = s.frames.fps;
    let k = ((fps - 25.0) / 35.0).clamp(0.0, 1.0);
    let col = if k < 0.5 {
        Color32::from_rgb(235, (70.0 + 300.0 * k) as u8, 60)
    } else {
        Color32::from_rgb((235.0 - 350.0 * (k - 0.5)) as u8, 220, 90)
    };
    let fg = painter.layout_no_wrap(format!("{fps:.0}"), FontId::new(44.0, semibold()), col);
    let lg = painter.layout_no_wrap("FPS".into(), FontId::proportional(13.0), TEXT_DIM);
    let bw = (fg.size().x + 44.0).max(160.0);
    let r = egui::Rect::from_min_size(
        pos2(screen.max.x - 18.0 - bw, screen.min.y + 18.0),
        vec2(bw, fg.size().y + 8.0),
    );
    painter.rect_filled(
        r,
        CornerRadius::same(9),
        Color32::from_rgba_unmultiplied(10, 12, 16, 190),
    );
    painter.galley(r.min + vec2(10.0, 4.0), fg, col);
    painter.galley(
        pos2(r.max.x - 10.0 - lg.size().x, r.max.y - 8.0 - lg.size().y),
        lg,
        TEXT_DIM,
    );
    // frame-time history: stutter shows as spikes here instead of making the number jump
    let gw = bw;
    let gr = egui::Rect::from_min_size(pos2(r.max.x - gw, r.max.y + 6.0), vec2(gw, 46.0));
    painter.rect_filled(
        gr,
        CornerRadius::same(7),
        Color32::from_rgba_unmultiplied(10, 12, 16, 170),
    );
    let plot = gr.shrink2(vec2(8.0, 6.0));
    let plot = egui::Rect::from_min_max(plot.min, pos2(plot.max.x, plot.max.y - 12.0));
    let to_y = |ms: f32| plot.max.y - (ms / 66.7).clamp(0.0, 1.0) * plot.height();
    for (ms, a) in [(16.7, 70u8), (33.3, 50u8)] {
        painter.line_segment(
            [pos2(plot.min.x, to_y(ms)), pos2(plot.max.x, to_y(ms))],
            Stroke::new(1.0, Color32::from_white_alpha(a)),
        );
    }
    let bars: Vec<f32> = s.frames.recent_ms(120).collect();
    let bar_w = plot.width() / 120.0;
    for (i, ms) in bars.iter().enumerate() {
        let x = plot.max.x - (bars.len() - i) as f32 * bar_w;
        let c = if *ms > 50.0 {
            Color32::from_rgb(235, 80, 60)
        } else if *ms > 20.0 {
            Color32::from_rgb(235, 190, 70)
        } else {
            Color32::from_rgb(90, 200, 110)
        };
        painter.rect_filled(
            egui::Rect::from_min_max(
                pos2(x, to_y(*ms)),
                pos2(x + bar_w.max(1.0) - 0.3, plot.max.y),
            ),
            0.0,
            c,
        );
    }
    let note = format!(
        "{:.1} ms  ·  worst {:.0} ms",
        s.frames.median_ms, s.frames.worst_ms
    );
    painter.text(
        pos2(gr.min.x + 8.0, gr.max.y - 4.0),
        Align2::LEFT_BOTTOM,
        note,
        FontId::proportional(10.5),
        TEXT_DIM,
    );
    let r = egui::Rect::from_min_max(r.min, pos2(r.max.x, gr.max.y));
    let dg = painter.layout_no_wrap(
        format!("Draw: {}  ·  V", s.graphics.preset_name()),
        FontId::proportional(12.5),
        TEXT,
    );
    let dr = egui::Rect::from_min_size(
        pos2(r.max.x - dg.size().x - 20.0, r.max.y + 6.0),
        dg.size() + vec2(20.0, 10.0),
    );
    painter.rect_filled(
        dr,
        CornerRadius::same(7),
        Color32::from_rgba_unmultiplied(10, 12, 16, 170),
    );
    painter.galley(dr.min + vec2(10.0, 5.0), dg, TEXT);
    // controls
    let ctrl = crate::motion::movement::key_hints(&s.walk);
    let g = painter.layout_no_wrap(ctrl, FontId::proportional(12.5), TEXT);
    let r =
        egui::Rect::from_center_size(pos2(c.x, screen.max.y - 28.0), g.size() + vec2(24.0, 12.0));
    painter.rect_filled(
        r,
        CornerRadius::same(8),
        Color32::from_rgba_unmultiplied(10, 12, 16, 170),
    );
    painter.galley(r.min + vec2(12.0, 6.0), g, TEXT);
    // building while walking a dungeon (Enter)
    crate::dungeons::ui::walk_build_bar(ctx, s, screen);
    // (inside an underground dungeon: its own map of the level)
    if !crate::dungeons::survey::hud(
        ctx,
        &s.dungeon_map,
        &s.dungeon_world,
        s.walk.position,
        s.walk.yaw,
        screen,
    ) {
        crate::worldmap::minimap::hud(
            ctx,
            &mut s.map,
            &s.gameplay,
            s.walk.position,
            s.walk.yaw,
            screen,
        );
    }
    // zone entry banner
    if let Some((name, lvl, age)) = &s.walk.banner {
        let a = (1.0 - ((*age - 3.0) / 1.0).clamp(0.0, 1.0)) * (*age / 0.4).min(1.0);
        if a > 0.0 {
            let y = screen.min.y + screen.height() * 0.22;
            painter.text(
                pos2(c.x, y),
                Align2::CENTER_CENTER,
                name,
                FontId::new(36.0, semibold()),
                Color32::from_rgba_unmultiplied(255, 236, 200, (255.0 * a) as u8),
            );
            painter.text(
                pos2(c.x, y + 34.0),
                Align2::CENTER_CENTER,
                lvl,
                FontId::proportional(17.0),
                Color32::from_rgba_unmultiplied(230, 230, 240, (220.0 * a) as u8),
            );
        }
    }
}

fn new_world_dialog(ctx: &egui::Context, s: &mut UiState, out: &mut UiOut) {
    let mut open = true;
    // Keep the title bar and close button reachable on smaller viewports while the full form
    // remains available below the fold.
    let max_h = (ctx.content_rect().height() - 64.0).max(280.0);
    egui::Window::new(RichText::new("New world").font(FontId::new(17.0, semibold())))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .fixed_size(vec2(440.0, max_h))
        .open(&mut open)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("new_world_body")
                .max_height((max_h - 48.0).max(200.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if panels::bigworld_ui::new_world_contents(ui, s, out) {
                        s.ed.show_new_world = false;
                    }
                });
        });
    if !open {
        s.ed.show_new_world = false;
    }
}

fn help_window(ctx: &egui::Context, s: &mut UiState) {
    let mut open = true;
    egui::Window::new(RichText::new("How it works").font(FontId::new(17.0, semibold())))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .fixed_size(vec2(520.0, 0.0))
        .open(&mut open)
        .show(ctx, |ui| {
            let rows = [
                (
                    "1  Land",
                    "Paint continents out of the ocean, or draw a coastline outline",
                ),
                ("2  Sculpt", "Mountains, hills, cliffs, terraces, erosion"),
                (
                    "3  Paint",
                    "Ground materials. Snow, rock and beaches appear automatically",
                ),
                (
                    "4  Water",
                    "Sea level, rivers (drag), lakes (click a basin)",
                ),
                (
                    "5  Nature",
                    "Paint whole forests with biome brushes, or single species",
                ),
                (
                    "6  Objects",
                    "Place bridges, huts, ruins and your imported glTF models",
                ),
                (
                    "7  Zones",
                    "Paint zones; set level range, weather, ambience, biome",
                ),
                ("8  Lighting", "Time of day, sun path, clouds, fog, shadows"),
                ("9  Audio", "See and hear what every surface sounds like"),
            ];
            egui::Grid::new("help_grid")
                .num_columns(2)
                .spacing(vec2(18.0, 6.0))
                .show(ui, |ui| {
                    for (a, b) in rows {
                        ui.label(RichText::new(a).font(FontId::new(13.5, semibold())));
                        ui.label(RichText::new(b).color(TEXT_DIM));
                        ui.end_row();
                    }
                });
            separator(ui);
            ui.horizontal_wrapped(|ui| {
                keycap(ui, "LMB", "apply");
                keycap(ui, "⌥/⌘ + LMB", "inverse");
                keycap(ui, "RMB", "orbit");
                keycap(ui, "MMB / Space+LMB", "pan");
                keycap(ui, "Scroll / pinch", "zoom");
                keycap(ui, "WASD Q E", "move / turn");
                keycap(ui, "F", "focus cursor");
                keycap(ui, "B", "work light (dark caves, night)");
                keycap(ui, "[ ]", "brush size");
                keycap(ui, "⌘Z / ⇧⌘Z", "undo / redo");
                keycap(ui, "⌘S", "save");
                keycap(ui, "Z / X", "rotate selected object");
                keycap(ui, "Del", "delete selected");
            });
        });
    if !open {
        s.ed.show_help = false;
    }
}

/// Small procedural preview image per terrain layer for the material palette.
fn make_layer_preview(ctx: &egui::Context, layer: Layer) -> egui::TextureHandle {
    let n = 64;
    let base = layer.swatch();
    let mut px = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            let fx = x as f32 / n as f32 * 8.0;
            let fy = y as f32 / n as f32 * 8.0;
            let v1 = crate::noise::value2(fx, fy, 3 + layer as u32);
            let v2 = crate::noise::value2(fx * 3.1, fy * 3.1, 9 + layer as u32);
            let mut k = 0.78 + 0.28 * v1 + 0.18 * (v2 - 0.5);
            match layer {
                Layer::Cobblestone | Layer::Gravel => {
                    let cell = crate::noise::value2(fx * 1.3, fy * 1.3, 77);
                    k *= 0.7 + 0.5 * (cell * 6.0).fract().min(0.8);
                }
                Layer::Rock | Layer::DarkRock => {
                    k *= 0.85 + 0.25 * ((fy * 2.0 + v1 * 3.0).sin() * 0.5 + 0.5)
                }
                Layer::Sand => k *= 0.95 + 0.08 * ((fx * 3.0 + fy * 2.0 + v1 * 2.0).sin()),
                _ => {}
            }
            let c = |v: u8| ((v as f32 * k).clamp(0.0, 255.0)) as u8;
            px.push(Color32::from_rgb(c(base[0]), c(base[1]), c(base[2])));
        }
    }
    let img = egui::ColorImage::new([n, n], px);
    ctx.load_texture(
        format!("layer_{}", layer as usize),
        img,
        egui::TextureOptions::LINEAR,
    )
}

pub fn category_icon(c: Category) -> &'static str {
    match c {
        Category::Trees => icon::TREE_EVERGREEN,
        Category::Rocks => icon::DIAMOND,
        Category::Plants => icon::PLANT,
        Category::Debris => icon::TREE_STRUCTURE,
        Category::Structures => icon::HOUSE,
        Category::Props => icon::BASKET,
        Category::Imported => icon::DOWNLOAD_SIMPLE,
    }
}
