//! Editor state: the active tab (vertical rail), tools, brush settings and view overlays.

use bevy::prelude::*;

use crate::camera::Pointer;
use crate::sky::{Lighting, SkyState};
use crate::terrain::Terrain;
use crate::terrain::layers::{Layer, Surface};
use crate::terrain::material::{MAX_ZONES, TerrainGpu, TerrainMaterial};
use crate::terrain::worldgen::WorldPreset;
use crate::world::Zones;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum Tab {
    #[default]
    Land,
    Sculpt,
    Paint,
    Caves,
    Water,
    Roads,
    Nature,
    Objects,
    Zones,
    Gameplay,
    Lighting,
    Weather,
    Style,
    Audio,
    Analyze,
    Map,
    /// the world's dungeons; in a dungeon's map, its layout (`crate::dungeons`)
    Dungeons,
    DungeonLook,
    DungeonLights,
}

impl Tab {
    pub const ALL: [Tab; 19] = [
        Tab::Land,
        Tab::Sculpt,
        Tab::Paint,
        Tab::Caves,
        Tab::Water,
        Tab::Roads,
        Tab::Nature,
        Tab::Objects,
        Tab::Zones,
        Tab::Gameplay,
        Tab::Lighting,
        Tab::Weather,
        Tab::Style,
        Tab::Audio,
        Tab::Analyze,
        Tab::Map,
        Tab::Dungeons,
        Tab::DungeonLook,
        Tab::DungeonLights,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Tab::Land => "Land",
            Tab::Sculpt => "Sculpt",
            Tab::Paint => "Paint",
            Tab::Caves => "Caves",
            Tab::Water => "Water",
            Tab::Roads => "Roads",
            Tab::Nature => "Nature",
            Tab::Objects => "Objects",
            Tab::Zones => "Zones",
            Tab::Gameplay => "Gameplay",
            Tab::Lighting => "Lighting",
            Tab::Weather => "Weather",
            Tab::Style => "Style",
            Tab::Audio => "Audio",
            Tab::Analyze => "Analyze",
            Tab::Map => "Map",
            Tab::Dungeons => "Dungeons",
            Tab::DungeonLook => "Dungeon look",
            Tab::DungeonLights => "Dungeon lights",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Tab::Land => "Draw continents: paint land out of the sea, or outline a coast",
            Tab::Sculpt => "Shape mountains, hills, valleys and cliffs",
            Tab::Paint => "Paint ground materials; erase returns to automatic styling",
            Tab::Caves => {
                "Caves, mines, sinkholes, rock arches and dungeon entrances: click the ground"
            }
            Tab::Water => "Set the ocean level, draw rivers, fill lakes",
            Tab::Roads => {
                "Draw roads and paths: they follow the land, clear the way and bridge rivers"
            }
            Tab::Nature => "Paint forests, rocks, plants and debris",
            Tab::Objects => "Place structures and imported models",
            Tab::Zones => "Paint zones and set their level ranges, weather and audio",
            Tab::Gameplay => {
                "Player start, points of interest, NPCs, creature spawns and resource nodes"
            }
            Tab::Lighting => "Time of day, sun, moon, haze and shadows",
            Tab::Weather => "Clouds, wind, rain, snow and fog",
            Tab::Style => "Art style: switch the whole world between realistic and stylised looks",
            Tab::Audio => "Footstep surfaces and ambience",
            Tab::Analyze => "Pacing, walkability, level flow and the world doctor",
            Tab::Map => "Generate the in-game world map and minimap",
            Tab::Dungeons if crate::dungeons::interior() => {
                "Lay the dungeon out in one click, or draw rooms, halls, corridors, caverns and stairs on its floor"
            }
            Tab::Dungeons if crate::dungeons::in_dungeon() => {
                "A realm: build it with the world's tools"
            }
            Tab::Dungeons => {
                "Create dungeons, give them ways in (portals, objects, spots, caves) and open their builders"
            }
            Tab::DungeonLook => {
                "The dungeon's style (walls, floors, trims, pillars), dressing and mood (light, fog, sound)"
            }
            Tab::DungeonLights => {
                "Torches, braziers, candles, crystals, shafts of light: by Auto-light or by hand"
            }
        }
    }
    pub fn group(self) -> TabGroup {
        match self {
            Tab::Land | Tab::Sculpt | Tab::Paint | Tab::Caves => TabGroup::Terrain,
            Tab::Water => TabGroup::Water,
            Tab::Nature => TabGroup::Nature,
            Tab::Objects | Tab::Roads => TabGroup::Build,
            Tab::Zones | Tab::Gameplay => TabGroup::Gameplay,
            Tab::Lighting | Tab::Weather | Tab::Style | Tab::Audio => TabGroup::Atmosphere,
            Tab::Analyze | Tab::Map => TabGroup::Review,
            Tab::Dungeons | Tab::DungeonLook | Tab::DungeonLights => TabGroup::Dungeons,
        }
    }
}

/// The tool rail's entries: each holds one or more tabs (shown as sub-tabs over the panel).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum TabGroup {
    Terrain,
    Water,
    Nature,
    Build,
    Gameplay,
    Atmosphere,
    Review,
    Dungeons,
}

impl TabGroup {
    pub const ALL: [TabGroup; 8] = [
        TabGroup::Terrain,
        TabGroup::Water,
        TabGroup::Nature,
        TabGroup::Build,
        TabGroup::Dungeons,
        TabGroup::Gameplay,
        TabGroup::Atmosphere,
        TabGroup::Review,
    ];
    /// The groups on the rail: all of them in the world (and in a realm); in a dungeon's
    /// underground map, the dungeon's own first and the world's that make sense there.
    pub fn visible() -> Vec<TabGroup> {
        if crate::dungeons::interior() {
            vec![
                TabGroup::Dungeons,
                TabGroup::Build,
                TabGroup::Gameplay,
                TabGroup::Atmosphere,
                TabGroup::Review,
            ]
        } else if crate::dungeons::in_dungeon() {
            let mut v = vec![TabGroup::Dungeons];
            v.extend(
                Self::ALL
                    .iter()
                    .copied()
                    .filter(|g| *g != TabGroup::Dungeons),
            );
            v
        } else {
            Self::ALL.to_vec()
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            TabGroup::Terrain => "Terrain",
            TabGroup::Water => "Water",
            TabGroup::Nature => "Nature",
            TabGroup::Build => "Build",
            TabGroup::Gameplay => "Gameplay",
            TabGroup::Atmosphere => "Atmosphere",
            TabGroup::Review => "Review",
            TabGroup::Dungeons if crate::dungeons::in_dungeon() => "Dungeon",
            TabGroup::Dungeons => "Dungeons",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            TabGroup::Terrain => "Land, sculpting, ground paint and caves",
            TabGroup::Water => "The ocean, rivers, lakes, waterfalls and fish",
            TabGroup::Nature => "Forests, rocks, plants and debris",
            TabGroup::Build => "Objects, structures and roads",
            TabGroup::Gameplay => "Zones, the player start, places, NPCs, spawns and resources",
            TabGroup::Atmosphere => "Lighting, weather, art style and sound",
            TabGroup::Review => "Check the world (pacing, walkability) and make its map",
            TabGroup::Dungeons if crate::dungeons::in_dungeon() => {
                "This dungeon: its layout, look, lights; Back to world"
            }
            TabGroup::Dungeons => "Dungeons: maps of their own behind portals, doors and cave ends",
        }
    }
    /// Its tabs, in sub-tab order.
    pub fn tabs(self) -> &'static [Tab] {
        match self {
            TabGroup::Terrain => &[Tab::Land, Tab::Sculpt, Tab::Paint, Tab::Caves],
            TabGroup::Water => &[Tab::Water],
            TabGroup::Nature => &[Tab::Nature],
            TabGroup::Build if crate::dungeons::interior() => &[Tab::Objects],
            TabGroup::Build => &[Tab::Objects, Tab::Roads],
            TabGroup::Gameplay if crate::dungeons::interior() => &[Tab::Gameplay],
            TabGroup::Gameplay => &[Tab::Zones, Tab::Gameplay],
            TabGroup::Atmosphere if crate::dungeons::interior() => &[Tab::Style, Tab::Audio],
            TabGroup::Atmosphere => &[Tab::Lighting, Tab::Weather, Tab::Style, Tab::Audio],
            TabGroup::Review if crate::dungeons::interior() => &[Tab::Analyze],
            TabGroup::Review => &[Tab::Analyze, Tab::Map],
            TabGroup::Dungeons if crate::dungeons::interior() => {
                &[Tab::Dungeons, Tab::DungeonLook, Tab::DungeonLights]
            }
            TabGroup::Dungeons => &[Tab::Dungeons],
        }
    }
    /// What the sub-tab of `t` is called inside this group.
    pub fn sub_name(t: Tab) -> &'static str {
        match t {
            Tab::Gameplay => "Gameplay",
            Tab::Objects => "Objects",
            Tab::Dungeons if crate::dungeons::in_dungeon() => "Layout",
            Tab::DungeonLook => "Look",
            Tab::DungeonLights => "Lights",
            t => t.name(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LandTool {
    #[default]
    PaintLand,
    PaintSea,
    DrawCoast,
    /// Trace a Picture: the reference map (`crate::refmap`).
    Reference,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SculptTool {
    #[default]
    Raise,
    Lower,
    Smooth,
    Flatten,
    Mountain,
    Hills,
    Terrace,
    Erode,
}

impl SculptTool {
    pub const ALL: [SculptTool; 8] = [
        SculptTool::Raise,
        SculptTool::Lower,
        SculptTool::Smooth,
        SculptTool::Flatten,
        SculptTool::Mountain,
        SculptTool::Hills,
        SculptTool::Terrace,
        SculptTool::Erode,
    ];
    pub fn name(self) -> &'static str {
        match self {
            SculptTool::Raise => "Raise",
            SculptTool::Lower => "Lower",
            SculptTool::Smooth => "Smooth",
            SculptTool::Flatten => "Flatten",
            SculptTool::Mountain => "Mountains",
            SculptTool::Hills => "Hills",
            SculptTool::Terrace => "Terraces",
            SculptTool::Erode => "Erode",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            SculptTool::Raise => "Raise the ground (hold Alt to lower)",
            SculptTool::Lower => "Dig down (hold Alt to raise)",
            SculptTool::Smooth => "Soften bumps and sharp edges",
            SculptTool::Flatten => "Level the ground to the height where the stroke started",
            SculptTool::Mountain => "Grow rugged mountain ridges (Alt carves canyons)",
            SculptTool::Hills => "Add rolling hills and bumps",
            SculptTool::Terrace => "Cut natural terraces and plateaus",
            SculptTool::Erode => "Simulate rain erosion: gullies and sediment fans",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PaintTool {
    #[default]
    Paint,
    Erase,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WaterTool {
    #[default]
    River,
    /// flood-fill a basin with a click
    Lake,
    /// draw a lake outline (smoothed, carved, shore blended)
    DrawLake,
    /// expand / shrink / smooth a drawn lake's shore
    ShapeLake,
    Waterfall,
    Fish,
    Select,
    Remove,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScatterMode {
    #[default]
    Add,
    Remove,
    /// procedural nature: fewer, gradually
    Thin,
    /// one of the picked species where you click (a placed object)
    Place,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ObjectTool {
    #[default]
    Place,
    Select,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ZoneTool {
    #[default]
    Paint,
    Erase,
    /// Paint (or outline) an area as a biome.
    Biome,
}

#[derive(Clone, Copy, Debug)]
pub struct Brush {
    /// radius in meters
    pub size: f32,
    pub strength: f32,
    pub falloff: f32,
}

#[derive(Resource)]
pub struct Editor {
    pub tab: Tab,
    /// The tab last open in each rail group (a group opens on it).
    pub group_tab: std::collections::HashMap<TabGroup, Tab>,
    pub land_tool: LandTool,
    pub sculpt_tool: SculptTool,
    pub paint_tool: PaintTool,
    pub water_tool: WaterTool,
    pub scatter_mode: ScatterMode,
    pub object_tool: ObjectTool,
    pub zone_tool: ZoneTool,
    pub biome_preset: crate::biomes::BiomePreset,
    /// Biome tool draws an outline instead of painting.
    pub biome_outline: bool,

    pub land_brush: Brush,
    pub sculpt_brush: Brush,
    pub paint_brush: Brush,
    pub nature_brush: Brush,
    /// Nature › Fields is open (no nature brush then; `fields::ui::FieldTool` drives it)
    pub nature_fields: bool,
    pub zone_brush: Brush,

    pub paint_layer: Layer,
    /// Height of painted land above sea level (m)
    pub land_height: f32,
    /// 0..1 coastline raggedness
    pub coast_roughness: f32,
    pub terrace_step: f32,
    pub river_width: f32,
    pub river_depth: f32,
    pub lake_fill: f32,
    pub new_river_style: crate::water::WaterStyle,
    pub new_lake_style: crate::water::WaterStyle,
    /// lake drawing, shore brush and waterfall settings
    pub water_tools: crate::water::WaterToolSettings,
    pub fish_species: crate::fish::FishSpecies,
    pub fish_count: u32,
    pub fish_radius: f32,

    pub show_zone_boundaries: bool,
    pub show_zone_labels: bool,
    pub show_level_ranges: bool,
    pub show_zone_fill: bool,
    pub surface_overlay: bool,

    pub new_world_preset: WorldPreset,
    /// File › New's art-style draft. It is applied only after the new world is created.
    pub new_world_art_style: crate::art_style::ArtStyle,
    /// File › New's nature appearance draft. Cancel never writes the open world's controls.
    pub new_world_appearance: crate::nature::NatureAppearance,
    pub new_world_size: usize,
    pub new_world_seed: u32,
    /// File › New: lay out farm fields once the world is made (crate::fields::auto)
    pub new_world_fields: bool,
    pub new_world_biomes: bool,
    pub new_world_climate: crate::biomegen::Climate,
    pub show_new_world: bool,
    pub show_help: bool,
    pub asset_search: String,
    /// Paint tab: material search, and a material waiting for a slot to replace (all 12 in use).
    pub material_search: String,
    pub paint_replace: Option<String>,
    /// Objects: per-axis stretch shown (proportions unlocked).
    pub scale_unlocked: bool,
    /// Status-bar message and its age
    pub toast: Option<(String, f32)>,
    /// Override footstep surface per terrain layer.
    pub layer_surface: [Surface; 12],
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            tab: Tab::Land,
            group_tab: Default::default(),
            nature_fields: false,
            land_tool: LandTool::PaintLand,
            sculpt_tool: SculptTool::Raise,
            paint_tool: PaintTool::Paint,
            water_tool: WaterTool::River,
            scatter_mode: ScatterMode::Add,
            object_tool: ObjectTool::Place,
            zone_tool: ZoneTool::Paint,
            biome_preset: crate::biomes::BiomePreset::TemperateForest,
            biome_outline: false,
            land_brush: Brush {
                size: 220.0,
                strength: 0.6,
                falloff: 0.6,
            },
            sculpt_brush: Brush {
                size: 90.0,
                strength: 0.5,
                falloff: 0.7,
            },
            paint_brush: Brush {
                size: 30.0,
                strength: 0.6,
                falloff: 0.5,
            },
            nature_brush: Brush {
                size: 60.0,
                strength: 0.6,
                falloff: 0.3,
            },
            zone_brush: Brush {
                size: 150.0,
                strength: 1.0,
                falloff: 0.0,
            },
            paint_layer: Layer::Grass,
            land_height: 8.0,
            coast_roughness: 0.55,
            terrace_step: 12.0,
            river_width: 18.0,
            river_depth: 4.0,
            lake_fill: 4.0,
            new_river_style: crate::water::WaterType::MountainStream.style(),
            new_lake_style: crate::water::WaterType::ClearLake.style(),
            water_tools: Default::default(),
            fish_species: crate::fish::FishSpecies::Trout,
            fish_count: 12,
            fish_radius: 14.0,
            show_zone_boundaries: true,
            show_zone_labels: true,
            show_level_ranges: true,
            show_zone_fill: true,
            surface_overlay: false,
            new_world_preset: WorldPreset::Island,
            new_world_art_style: crate::art_style::ArtStyle::Realistic,
            new_world_appearance: default(),
            new_world_size: 2048,
            new_world_seed: 7,
            new_world_fields: true,
            new_world_biomes: true,
            new_world_climate: crate::biomegen::Climate::Varied,
            show_new_world: false,
            show_help: false,
            asset_search: String::new(),
            material_search: String::new(),
            paint_replace: None,
            scale_unlocked: false,
            toast: None,
            layer_surface: std::array::from_fn(|i| Layer::from_index(i).default_surface()),
        }
    }
}

impl Editor {
    /// The brush used by the active tab (None for tabs without a terrain brush).
    pub fn active_brush(&self) -> Option<Brush> {
        match self.tab {
            Tab::Land if !matches!(self.land_tool, LandTool::DrawCoast | LandTool::Reference) => {
                Some(self.land_brush)
            }
            Tab::Sculpt => Some(self.sculpt_brush),
            Tab::Paint => Some(self.paint_brush),
            Tab::Nature if !self.nature_fields => Some(self.nature_brush),
            Tab::Zones if !(self.zone_tool == ZoneTool::Biome && self.biome_outline) => {
                Some(self.zone_brush)
            }
            Tab::Water if self.water_tool == WaterTool::ShapeLake => Some(self.water_tools.brush),
            _ => None,
        }
    }
    pub fn active_brush_mut(&mut self) -> Option<&mut Brush> {
        match self.tab {
            Tab::Land => Some(&mut self.land_brush),
            Tab::Sculpt => Some(&mut self.sculpt_brush),
            Tab::Paint => Some(&mut self.paint_brush),
            Tab::Nature if !self.nature_fields => Some(&mut self.nature_brush),
            Tab::Zones => Some(&mut self.zone_brush),
            Tab::Water if self.water_tool == WaterTool::ShapeLake => {
                Some(&mut self.water_tools.brush)
            }
            _ => None,
        }
    }
    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), 0.0));
    }
}

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Editor>()
            .add_systems(Update, (brush_size_input, tick_toast))
            .add_systems(
                PostUpdate,
                update_terrain_params.before(crate::terrain::TerrainSystems::Gpu),
            );
    }
}

fn tick_toast(mut ed: ResMut<Editor>, time: Res<Time>) {
    if let Some((_, age)) = &mut ed.toast {
        *age += time.delta_secs();
        if *age > 6.0 {
            ed.toast = None;
        }
    }
}

fn brush_size_input(
    mut ed: ResMut<Editor>,
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    pointer: Res<Pointer>,
    time: Res<Time>,
) {
    if pointer.keyboard_captured {
        return;
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let mut factor = 1.0;
    if keys.pressed(KeyCode::BracketRight) {
        factor *= 1.0 + 1.5 * time.delta_secs();
    }
    if keys.pressed(KeyCode::BracketLeft) {
        factor /= 1.0 + 1.5 * time.delta_secs();
    }
    if shift && !pointer.over_ui {
        let d = scroll.delta.y + scroll.delta.x;
        let k = match scroll.unit {
            bevy::input::mouse::MouseScrollUnit::Line => 0.1,
            bevy::input::mouse::MouseScrollUnit::Pixel => 0.004,
        };
        factor *= (d * k).exp();
    }
    if factor != 1.0
        && let Some(b) = ed.active_brush_mut()
    {
        b.size = (b.size * factor).clamp(2.0, 2000.0);
    }
}

/// Pushes editor + sky state into the terrain material uniforms.
#[allow(clippy::too_many_arguments)]
fn update_terrain_params(
    ed: Res<Editor>,
    pointer: Res<Pointer>,
    mut terrain: ResMut<Terrain>,
    zones: Res<Zones>,
    sky: Res<SkyState>,
    lighting: Res<Lighting>,
    gpu: Option<Res<TerrainGpu>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    walking: Res<crate::walk::WalkState>,
    ocean: Res<crate::water::WaterStyle>,
    look: Res<crate::look::LookState>,
    terrain_tex: Res<crate::terrain::textures::TerrainTextures>,
    mcp: Option<Res<crate::mcp::McpServer>>,
    rain: Res<crate::rain::RainState>,
) {
    // Clean screenshots, walking and cinematics: no brush ring or editor overlays. This is a
    // draw-time gate; the editor's saved visibility choices remain untouched.
    let clean = !crate::mcp::ui_visible(mcp);
    let editor_terrain_visuals =
        !clean && crate::terrain::overlay::editor_terrain_visuals_visible(&walking);
    let Some(gpu) = gpu else { return };
    let Some(old) = materials
        .get(&gpu.material)
        .map(|m| m.extension.params.clone())
    else {
        return;
    };
    let mut params = old.clone();
    let p = &mut params;
    p.grid = Vec4::new(
        terrain.half(),
        terrain.cell,
        terrain.n() as f32,
        terrain.sea_level,
    );
    let s = &terrain.style;
    p.style = Vec4::new(s.snow_line, s.snow_blend, s.rock_slope, s.beach_height);
    p.style2 = Vec4::new(
        s.dryness,
        // (the rain's wetness follows the weather with a slow dry-out: `rain::RainState`)
        (s.wetness + rain.surface_q()).min(1.0),
        time.elapsed_secs(),
        sky.weather.snow * 160.0,
    );
    p.sun = sky
        .sun_dir
        .extend(lighting.cloud_shadows * (1.0 - sky.night));
    p.clouds = Vec4::new(
        sky.weather.clouds,
        1.0,
        sky.cloud_offset.x,
        sky.cloud_offset.y,
    );
    let sc = Color::srgb(ocean.shallow[0], ocean.shallow[1], ocean.shallow[2]).to_linear();
    p.water = Vec4::new(sc.red, sc.green, sc.blue, ocean.clarity);
    p.tex.z = look.params.detail;
    // metres per repeat per layer (built-in or the world's library choice)
    let t = &terrain_tex.tiles;
    if t[0] > 0.0 {
        p.layer_tile =
            std::array::from_fn(|i| Vec4::new(t[i * 4], t[i * 4 + 1], t[i * 4 + 2], t[i * 4 + 3]));
    }
    p.tex.w = look.params.texture_soften;
    // rain puddles and the drops' ripples on them (`rain`, `terrain.wesl`)
    p.overlay_image.y = rain.puddles_q(&lighting.rain);
    p.overlay_image.z = if p.overlay_image.y > 0.0 {
        rain.rain_q()
    } else {
        0.0
    };

    // brush cursor
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    p.brush_color.w = 0.0;
    if editor_terrain_visuals
        && let (Some(hit), Some(brush)) = (pointer.hit, ed.active_brush())
        && !pointer.over_ui
    {
        p.brush = Vec4::new(hit.x, hit.z, brush.size, brush.falloff);
        let c: Color = match ed.tab {
            Tab::Land => {
                if (ed.land_tool == crate::editor::LandTool::PaintSea) ^ alt {
                    Color::srgb(0.3, 0.65, 1.0)
                } else {
                    Color::srgb(1.0, 0.85, 0.45)
                }
            }
            Tab::Sculpt => {
                if alt {
                    Color::srgb(1.0, 0.55, 0.3)
                } else {
                    Color::srgb(0.85, 0.93, 1.0)
                }
            }
            Tab::Paint => {
                if ed.paint_tool == PaintTool::Erase || alt {
                    Color::srgb(1.0, 0.4, 0.4)
                } else {
                    let sw = ed.paint_layer.swatch();
                    Color::srgb_u8(sw[0], sw[1], sw[2]).mix(&Color::WHITE, 0.35)
                }
            }
            Tab::Nature => {
                if ed.scatter_mode == ScatterMode::Remove || alt {
                    Color::srgb(1.0, 0.45, 0.35)
                } else {
                    Color::srgb(0.55, 1.0, 0.5)
                }
            }
            Tab::Zones => {
                if ed.zone_tool == ZoneTool::Erase || alt {
                    Color::srgb(1.0, 0.4, 0.4)
                } else {
                    zones
                        .selected
                        .and_then(|i| zones.list.get(i))
                        .map(|z| Color::srgb(z.color[0], z.color[1], z.color[2]))
                        .unwrap_or(Color::WHITE)
                }
            }
            Tab::Water => match (ed.water_tools.shape_mode, alt) {
                (crate::water::lakes::ShapeMode::Smooth, _) => Color::srgb(0.75, 0.9, 1.0),
                (crate::water::lakes::ShapeMode::Shrink, false)
                | (crate::water::lakes::ShapeMode::Expand, true) => Color::srgb(1.0, 0.7, 0.4),
                _ => Color::srgb(0.35, 0.75, 1.0),
            },
            _ => Color::WHITE,
        };
        let l = c.to_linear();
        p.brush_color = Vec4::new(l.red, l.green, l.blue, 1.0);
    }

    // overlays (the biome brush shows the biome map in place of the zones)
    let zones_tab = ed.tab == Tab::Zones;
    let biome_view = zones_tab && ed.zone_tool == ZoneTool::Biome;
    if terrain.show_biomes != biome_view {
        terrain.show_biomes = biome_view;
        let n = terrain.n();
        terrain.mark_zone_dirty(crate::terrain::GridRect {
            x0: 0,
            z0: 0,
            x1: n - 1,
            z1: n - 1,
        });
    }
    let mode = if !editor_terrain_visuals {
        0.0
    } else if ed.surface_overlay || ed.tab == Tab::Audio {
        2.0
    } else if biome_view || (zones_tab && ed.show_zone_fill) {
        1.0
    } else {
        0.0
    };
    let show_bounds =
        editor_terrain_visuals && ed.show_zone_boundaries && (zones_tab || ed.tab == Tab::Lighting);
    let highlight = if editor_terrain_visuals && zones_tab {
        zones
            .selected
            .and_then(|i| zones.list.get(i))
            .map(|z| z.id as f32)
            .unwrap_or(-1.0)
    } else {
        -1.0
    };
    p.overlay = Vec4::new(
        mode,
        if biome_view { 0.5 } else { 0.22 },
        if show_bounds { 1.0 } else { 0.0 },
        highlight,
    );
    p.zone_colors = [Vec4::ZERO; MAX_ZONES];
    if biome_view {
        p.overlay.w = -1.0;
        for b in crate::biomes::BiomePreset::ALL {
            let d = b.def();
            let c = Color::srgb(d.color[0], d.color[1], d.color[2]).to_linear();
            p.zone_colors[b.id() as usize] = Vec4::new(c.red, c.green, c.blue, 1.0);
        }
    } else {
        for z in &zones.list {
            if (z.id as usize) < MAX_ZONES && z.visible {
                let c = Color::srgb(z.color[0], z.color[1], z.color[2]).to_linear();
                p.zone_colors[z.id as usize] = Vec4::new(c.red, c.green, c.blue, 1.0);
            }
        }
    }
    for (i, s) in ed.layer_surface.iter().enumerate() {
        let c = s.overlay_color();
        let l = Color::srgb_u8(c[0], c[1], c[2]).to_linear();
        p.layer_overlay[i] = Vec4::new(l.red, l.green, l.blue, 1.0);
    }
    // only a real change is written: touching the material rebuilds its bind group and
    // re-specialises every terrain mesh (the time alone doesn't count: no terrain shader animates
    // with it). The cloud shadows' drift, which never stops in any wind, goes in steps of
    // `CLOUD_STEP`: invisible in their soft edges, a few writes a second instead of every frame.
    params.style2.z = old.style2.z;
    if (params.clouds.zw() - old.clouds.zw()).length() < CLOUD_STEP {
        (params.clouds.z, params.clouds.w) = (old.clouds.z, old.clouds.w);
    }
    if uniform_bytes(&params) != uniform_bytes(&old)
        && let Some(mut m) = materials.get_mut(&gpu.material)
    {
        params.style2.z = time.elapsed_secs();
        m.extension.params = params;
    }
}

/// How far (m) the cloud field drifts before the terrain's cloud shadows follow.
const CLOUD_STEP: f32 = 1.5;

/// The terrain uniforms as the GPU gets them (to tell whether anything changed).
fn uniform_bytes(p: &crate::terrain::material::TerrainParams) -> Vec<u8> {
    let mut b = bevy::render::render_resource::encase::UniformBuffer::new(Vec::<u8>::new());
    let _ = b.write(p);
    b.into_inner()
}
