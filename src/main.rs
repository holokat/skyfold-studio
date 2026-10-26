//! Skyfold Studio — a paintbrush-simple 3D world editor for MMO worlds.

mod analysis;
mod art_style;
mod asset_rules;
mod audio;
mod auras;
mod background;
mod bigworld;
mod biomegen;
mod biomes;
mod bounce_light;
mod camera;
mod camera_fx;
mod caves;
mod celestial;
mod characters;
mod clouds;
mod content;
mod debug;
mod dialogs;
mod dungeons;
mod editor;
mod effects;
mod export;
mod fields;
mod fish;
mod fixture_lights;
mod flames;
mod flora;
mod forest_contours;
mod frame_stats;
mod fx_anchors;
mod gameplay;
mod gpu_watchdog;
mod grade;
mod grass;
mod groundcover;
mod haze;
mod history;
mod images;
mod io;
mod look;
mod look_post;
mod look_shuffle;
mod look_styles;
mod market;
mod mcp;
mod mcp_tools;
mod model_render;
mod motion;
mod nature;
mod noise;
mod objects;
mod ocean;
mod photoscan;
mod pipeline_warmup;
mod placement;
mod plants;
mod paths;
mod prefs;
mod procgen;
mod prompt;
mod rain;
mod refmap;
mod render_scale;
mod roads;
mod scale;
mod scenes;
mod setpieces;
mod setup;
mod sky;
mod streaming;
mod sun_shafts;
mod surfaces;
mod terrain;
mod thumbs;
mod tools;
mod towns;
mod trees;
mod ui;
mod updates;
mod visuals;
mod volumetrics;
mod walk;
mod water;
mod water_render;
mod weather;
mod wind;
mod world;
mod world_kind;
mod worldmap;
mod zones;

use bevy::asset::AssetApp;
use bevy::asset::io::AssetSourceBuilder;
use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};
use bevy_egui::EguiPlugin;

fn main() {
    // `--mcp-stdio`: no editor, just the stdio ↔ HTTP bridge for MCP clients (mcp/bridge.rs)
    if std::env::args().any(|a| a == "--mcp-stdio") {
        return mcp::bridge::run();
    }
    // The watcher for this source starts with DefaultPlugins. Create it first: the bundle is
    // read-only in a DMG, and a new user's import directory does not exist yet.
    let user_assets = paths::user_assets_dir();
    if let Err(error) = std::fs::create_dir_all(&user_assets) {
        warn!("could not create user asset directory {user_assets:?}: {error}");
    }
    let user_assets = user_assets.to_string_lossy().into_owned();
    // the default source: the bundled assets, then bought packs for what the bundle lacks
    // (market::install); watched like the platform default
    let (bundled, packs) = (paths::bundled_assets_dir(), paths::packs_dir());
    App::new()
        .register_asset_source(
            bevy::asset::io::AssetSourceId::Default,
            AssetSourceBuilder::platform_default(&assets_path(), None).with_reader(move || {
                Box::new(market::install::OverlayReader::new(&bundled, &packs))
            }),
        )
        .register_asset_source(
            paths::USER_ASSET_SOURCE,
            AssetSourceBuilder::platform_default(&user_assets, None),
        )
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Skyfold Studio".into(),
                        resolution: match std::env::var("WB_SCALE")
                            .ok()
                            .and_then(|v| v.parse::<f32>().ok())
                        {
                            Some(f) => {
                                WindowResolution::new(1600, 1000).with_scale_factor_override(f)
                            }
                            None => WindowResolution::new(1600, 1000),
                        },
                        present_mode: if std::env::var("WB_NOVSYNC").is_ok() {
                            PresentMode::AutoNoVsync
                        } else {
                            PresentMode::AutoVsync
                        },
                        // automated screenshot runs open in the background without stealing focus
                        focused: std::env::var("WB_SHOT").is_err(),
                        // …parked almost entirely off the left screen edge, so they don't cover the
                        // user's windows (a window hidden behind others stops rendering on macOS,
                        // which would make the screenshot black)
                        position: if std::env::var("WB_SHOT").is_ok() {
                            bevy::window::WindowPosition::At(IVec2::new(-3150, 60))
                        } else {
                            bevy::window::WindowPosition::Automatic
                        },
                        ..default()
                    }),
                    // automated screenshot windows are click-through so they never steal the user's clicks
                    primary_cursor_options: Some(bevy::window::CursorOptions {
                        hit_test: std::env::var("WB_SHOT").is_err(),
                        ..default()
                    }),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: assets_path(),
                    watch_for_changes_override: Some(true),
                    ..default()
                }),
        )
        .add_plugins(EguiPlugin::default())
        .add_systems(Update, dock_icon)
        .add_plugins(updates::UpdatesPlugin)
        .init_resource::<world::Zones>()
        .init_resource::<world::WorldMeta>()
        .insert_resource(initial_terrain())
        .add_plugins((
            terrain::TerrainPlugin,
            camera::CameraPlugin,
            sky::SkyPlugin,
            clouds::CloudsPlugin,
            editor::EditorPlugin,
            walk::WalkPlugin,
            debug::DebugPlugin,
        ))
        .add_plugins((
            tools::ToolsPlugin,
            objects::ObjectsPlugin,
            water::WaterPlugin,
            audio::AudioPlugin,
            ui::UiPlugin,
            io::IoPlugin,
            zones::ZonesPlugin,
            fish::FishPlugin,
            thumbs::ThumbsPlugin,
            weather::WeatherPlugin,
            rain::RainPlugin,
            ocean::OceanPlugin,
            look::LookPlugin,
            biomes::BiomesPlugin,
        ))
        .add_plugins((
            trees::TreesPlugin,
            grass::GrassPlugin,
            effects::EffectsPlugin,
            wind::WindPlugin,
            streaming::StreamingPlugin,
            forest_contours::ForestMaskPlugin,
            surfaces::SurfacesPlugin,
        ))
        .add_plugins((
            fx_anchors::FxAnchorsPlugin,
            auras::AurasPlugin,
            flames::FlamesPlugin,
            fixture_lights::FixtureLightsPlugin,
        ))
        .add_plugins((
            gameplay::GameplayPlugin,
            roads::RoadsPlugin,
            frame_stats::FrameStatsPlugin,
            mcp::McpPlugin,
            motion::MotionPlugin,
        ))
        .add_plugins(background::BackgroundPlugin)
        .add_plugins((art_style::ArtStylePlugin, flora::FloraPlugin))
        .add_plugins(camera_fx::CameraFxPlugin)
        .add_plugins(analysis::AnalysisPlugin)
        .add_plugins(worldmap::WorldMapPlugin)
        .add_plugins(export::ExportPlugin)
        .add_plugins(characters::CharactersPlugin)
        .add_plugins(images::ImagesPlugin)
        .add_plugins((scale::ScalePlugin, nature::NaturePlugin))
        .add_plugins(groundcover::GroundCoverPlugin)
        .add_plugins(bigworld::BigWorldPlugin)
        .add_plugins(refmap::RefMapPlugin)
        .add_plugins(prompt::PromptPlugin)
        .add_plugins(visuals::VisualsPlugin)
        .add_plugins(volumetrics::VolumetricsPlugin)
        .add_plugins(gpu_watchdog::GpuWatchdogPlugin)
        .add_plugins(pipeline_warmup::PipelineWarmupPlugin)
        .add_plugins(bounce_light::BounceLightPlugin)
        .add_plugins(model_render::ModelRenderPlugin)
        .add_plugins(placement::PlacementPlugin)
        .add_plugins(setpieces::SetPiecesPlugin)
        .add_plugins(caves::CavesPlugin)
        .add_plugins(dungeons::DungeonsPlugin)
        .add_plugins(setup::SetupPlugin)
        .add_plugins((celestial::CelestialPlugin, world_kind::WorldKindPlugin))
        .add_plugins(towns::TownsPlugin)
        .add_plugins(scenes::ScenesPlugin)
        .add_plugins(fields::FieldsPlugin)
        .add_plugins((market::MarketPlugin, content::ContentPlugin))
        .add_plugins(asset_rules::AssetRulesPlugin)
        .init_resource::<mcp_tools::McpTools>()
        .add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default())
        .add_message::<tools::EditorAction>()
        .add_plugins(debug::gpu_diagnostics())
        .run();
}

/// Immutable bundled assets live in the app's Resources directory, or the checkout for dev.
pub fn assets_path() -> String {
    paths::bundled_assets_dir().to_string_lossy().into_owned()
}

/// Resolves a bundled or `user://` asset path for filesystem readers.
pub fn asset_file(path: &str) -> std::path::PathBuf {
    paths::asset_file(path)
}

fn initial_terrain() -> terrain::Terrain {
    let res = 1024;
    let cell = 4.0;
    let sea = 0.0;
    let mut t = terrain::Terrain::new(res, cell, sea);
    let t0 = std::time::Instant::now();
    t.heights =
        terrain::worldgen::build_world(terrain::worldgen::WorldPreset::Island, res, cell, sea, 7);
    info!("generated terrain in {:?}", t0.elapsed());
    t
}

/// The Skyfold Studio icon in the Dock (macOS shows the binary's generic icon otherwise); set
/// once, on the main thread, after the app is up.
fn dock_icon(_main: bevy::ecs::system::NonSendMarker, mut done: Local<bool>) {
    if *done {
        return;
    }
    *done = true;
    #[cfg(target_os = "macos")]
    {
        use objc2::AllocAnyThread;
        use objc2_app_kit::{NSApplication, NSImage};
        use objc2_foundation::{MainThreadMarker, NSData};
        let Some(mtm) = MainThreadMarker::new() else {
            *done = false;
            return;
        };
        let data = NSData::with_bytes(include_bytes!("../assets/branding/skyfold_icon_512.png"));
        if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
            let app = NSApplication::sharedApplication(mtm);
            unsafe { app.setApplicationIconImage(Some(&img)) };
        }
    }
}
