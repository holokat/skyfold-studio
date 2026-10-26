//! "Select an area and say what you want": the user lassos part of the world (any tab, the top
//! bar's sparkle button or G), types a prompt ("Turn this into an area with volcanoes") and an
//! AI agent edits just that area through the editor's own MCP tools while the user watches. The
//! result is one undo step with Keep / Undo / Refine.
//!
//! - `selection`: the lasso, the outline on the ground and the feathered mask raster;
//! - `mask`: the systemic edit mask (every edit stays inside the selection);
//! - `tools`: the selection, `area_edit` and `terrain_feature` MCP tools; `features`: landforms;
//! - `agent`: the backends (Codex CLI, Claude Code CLI, an OpenAI-compatible API key);
//! - here: a run's lifecycle — a private MCP endpoint offering only the area tools, the step
//!   feed, undo grouping, stop and timeout; `panel`: the floating prompt panel;
//! - `eval`: the measurement harness (`WB_PROMPT_EVAL=<cases.json>`).
//!
//! Test hooks: `WB_PROMPT_PANEL="<prompt>"` opens the panel with that prompt typed in as soon as
//! a selection exists (e.g. from `WB_MCP_CALL="selection_set {…}"`); `WB_PROMPT_SEND=1` also
//! sends it.

pub mod agent;
mod eval;
pub mod features;
pub mod mask;
pub(crate) mod panel;
pub mod selection;
pub mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde_json::{Value, json};

use crate::mcp::{CallResult, Out, PrivateEndpoint};
use agent::{AgentEvent, AgentHandle, Backend, BackendStatus};
pub(crate) use panel::connection_controls;
pub use panel::{
    IMAGE_EXTS, PromptUi, button_clicked, button_state, first_use_callout, image_thumb,
};
pub use selection::Selection;

/// The longest a run may take.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Tool calls a run may make (the agent is asked to stay under ~40).
const MAX_CALLS: u32 = 70;
/// Tool calls a run shaping a whole new dungeon may make (asked to stay under ~60).
const MAX_CALLS_WHOLE: u32 = 100;

/// What the agent is told (every backend).
pub const INSTRUCTIONS: &str = "You are a world designer working inside Skyfold Studio, a 3D editor for MMO game worlds, through its MCP tools (server \"world_builder\"). \
The user selected ONE area of the world and asked for a change there. Make it a beautiful, believable, hand-crafted place that fits the request.

Rules:
- Work only inside the selected area. Everything outside is protected by an edit mask: changes there are discarded, and edits fade out over the area's soft edge, so let your design blend into the surroundings near the edge.
- Coordinates are metres: x east, z south, y up; heights are absolute (see sea_level). Use the numbers from selection_info.
- Start with selection_info, look with selection_screenshot, then make a short plan.
- Prefer the high-level tools: terrain_feature for landforms (volcano, mountain_range, ridge, crater, mesas, buttes, canyon, valley, dunes, hills, plateau, sea_cliffs, lagoon, swamp, lake), area_edit to paint ground, set a biome, scatter or clear nature and sculpt the whole area in metres (with height and slope filters), terrain_set_biome, nature_scatter, water_add_lake and water_add_river. Use terrain_sculpt and terrain_paint only for touch-ups.
- Settlements: for a hamlet, village, town or city (\"add a village with a blacksmith and an inn\", \"a walled town here\") call objects_place_town once, without position or polygon so it fills the selected area, with the buildings asked for in `buildings` (plain words and counts) and walls if asked. It lays out real streets, the square and every building facing its street, so don't place buildings one by one; do the landforms and ground first, and mention anything its notes say was skipped in your summary.
- Farm fields and flower beds: fields_add plants a crop in rows inside an outline (wheat, corn, sunflowers, lavender, tulips, vineyard, hops, rice, tea… see fields_list), fields_set changes one. Tulip fields take one `colour` (red, yellow, pink, white, purple, orange) and `holes`, so a picture or flag is made of several fields that don't overlap: a Japanese flag is a red tulip field with a disc outline plus a white tulip field (the rectangle) with that disc as a hole.
- Single flowers and plants are objects (objects_place, assets from objects_library category plants: Tulip, whose `variant` is the colour index of its variant_names, Flowers, Lavender, ferns, bushes, berries…); for many, nature_scatter with `assets` grows them as natural density instead.
- Crystals in any colour: the Crystals asset (objects_library search \"crystal\") takes `color` (red, green, blue, amber, ruby, emerald… or \"#rrggbb\") and `light` (0 dark … 1 a soft glow with a light round it … 10 a beacon) per item in objects_place; objects_glow recolours placed ones.
- Roads and paths: roads_create. Ready-made scenes: scenes_list then scenes_place, or objects_place_set_piece. Caves: caves_create. Gameplay markers: the gameplay_ tools.
- Never tell the user something can't be made before checking: objects_library (search it), fields_list (crops) and the descriptions of your tools.
- Work in this order: landforms, then water, then biome and ground, then vegetation and objects. For vegetation prefer a biome (area_edit biome or terrain_set_biome): it paints a natural mix of ground layers and grows its own vegetation. Use area_edit scatter or nature_scatter for accents and denser patches, and place single objects only as a few hero pieces.
- Unpainted ground styles itself: rock on steep slopes, snow above the snow line (selection_info: snow_line_above_sea), sand on low coasts. Paint overrides it, e.g. snow on high ground with area_edit paint snow + min_height, or rock only on steep faces with min_slope_deg.
- Very dark water presets (blackwater, void) read as black holes from above: use them sparingly.
- Avoid a stamped look: big areas of one solid ground layer or one uniform density read as fake from above. Mix two or three layers at partial strength, use filters (slope, height) so ground follows the land, vary density, and let the edges break up into the surroundings.
- Check your work with selection_screenshot after the big changes (its close view shows small plants and flowers, which are invisible from far away) and fix what looks wrong (bare or blotchy patches, harsh seams at the edge, trees on lava or in water, wrong colours).
- Budget: about 40 tool calls at most. Every call is undoable; the whole session becomes one undo step.
- Don't create zones, don't change lighting, weather, the camera or settings, don't save and don't use undo.
- Reference images: when the user attached pictures, look at them closely first and recreate what they show. A top-down map follows its layout (coasts, water, mountains, forests, roads, regions) at the selection's scale. A photo or painting of a place (seen from the ground or the air) is interpreted: rebuild its landforms and their proportions, water, ground colours and textures, vegetation (kinds and density), rocks and any buildings, from the matching tools and library assets. Say in the summary what you matched and what you couldn't.
- When the selection is the whole world and the image is a map of a whole land, start with the Trace a Picture tools: reference_load (the original image path is given in the request), reference_analyse, then reference_build for the first draft; then refine biomes, water, nature and details with the other tools.
- Finish with a short summary for the user (1-3 sentences, plain words, no tool names) of what you made.";

/// What the agent is told when the area is part of an underground dungeon (every backend).
pub const DUNGEON_INSTRUCTIONS: &str = "You are a level designer working inside Skyfold Studio, a 3D editor for MMO game worlds, through its MCP tools (server \"world_builder\"). \
An underground dungeon's map is open. The user selected ONE area of it (an outline on the floor plan) and asked for a change there. Make it a memorable, believable, hand-crafted place that fits the request and the dungeon around it.

How dungeons work:
- A dungeon is a plan of rooms on levels: rooms, halls, corridors, caverns, stairs, spiral stairs, pits, chasms, pools, daises, bridges and balconies. Each has a shape (rect with width/length/yaw, round, octagon, a drawn polygon, or a path with a width for corridors), a height, a floor offset, a ceiling (flat, beams, vault, pointed, stepped, open, rock), pillars, a role (what it's for: crypt, shrine, throne, arena, library, prison, forge, treasury, barracks…), an optional style of its own and can be made irregular (uneven walls). Walls, floors, ceilings, trims and pillars are built for you in the dungeon's style.
- Rooms join where they overlap: a corridor that reaches into a room makes a doorway; a room overlapping another widens it. Keep everything connected so players can walk through (dungeons_walkable checks it).
- Lights (torch, brazier, hanging fire, chandelier, candles, lantern, crystal, light shaft, glow, spot), pieces (doors, double doors, portcullises, bars, secret doors, levers, pressure plates, traps, chests and the game's markers: arena, checkpoint, loot, spawn), decals (cracks, stains, drips, blood, scorch marks, runes) and library objects (props: objects_library) dress a place.
- Coordinates are metres: x east, z south, y up. A room's floor_y is absolute; place lights, pieces and objects at heights from it.

Rules:
- Work only inside the selected outline: change the rooms that touch it (selection_info lists them) and add new rooms inside it. Don't touch the rest of the dungeon, don't change its whole style or mood, don't re-lay it out, and don't leave the dungeon.
- Start with selection_info, look with selection_screenshot (top view: the plan seen from above, cut away above the level; close: into the rooms), then make a short plan.
- Big changes first (rooms: add, reshape, remove, re-purpose, roles, ceilings, heights, liquids), then pieces, then lights, then decals and props. Use dungeons_catalog for the names of styles, roles, ceilings, pieces and light kinds.
- Light what you build: rooms need light sources that suit them (torches on walls, braziers in big rooms, candles in shrines and crypts, crystals in caverns). Lights you add are kept.
- Check connections with dungeons_walkable when you added or removed rooms, and look again with selection_screenshot after the big changes; fix what looks wrong.
- Budget: about 40 tool calls at most. Every call is undoable; the whole session becomes one undo step.
- Don't change lighting outside the dungeon, weather, the camera or settings, don't save and don't use undo.
- Reference images: when the user attached pictures, look at them closely first and recreate what they show (a floor plan: its rooms and layout; a picture of a place: its kind of rooms, materials, light and mood). Say in the summary what you matched and what you couldn't.
- Finish with a short summary for the user (1-3 sentences, plain words, no tool names) of what you made.";

/// What the agent is told when it shapes a whole new dungeon from the user's description
/// (Dungeons › Create a dungeon › Describe it; every backend).
pub const DUNGEON_SHAPE_INSTRUCTIONS: &str = "You are a level designer working inside Skyfold Studio, a 3D editor for MMO game worlds, through its MCP tools (server \"world_builder\"). \
The user described a dungeon they want, and a new underground dungeon was just made for it: laid out from the one-click layout that best matched their words, in a matching style and mood, then lit (Auto-light) and dressed (Dress). Its map is open and the whole of it is selected. \
Your job: turn it into the dungeon they described — a memorable, believable, hand-crafted place a game designer would be proud of.

How dungeons work:
- A dungeon is a plan of rooms on levels: rooms, halls, corridors, caverns, stairs, spiral stairs, pits, chasms, pools, daises, bridges and balconies. Each has a shape (rect with width/length/yaw, round, octagon, a drawn polygon, or a path with a width for corridors), a height, a floor offset, a ceiling (flat, beams, vault, pointed, stepped, open, rock), pillars, a role (what it's for: entrance, crypt, shrine, throne, arena, boss, library, prison, forge, treasury, barracks, lair…), a liquid for pools and pits (water, lava, slime, blood, arcane), an optional style of its own, and can be made irregular (uneven walls). Walls, floors, ceilings, trims and pillars are built for you in the dungeon's style.
- Rooms join where they overlap: a corridor that reaches into a room makes a doorway; a room overlapping another widens it. Keep everything connected so players can walk from the entrance to every room (dungeons_walkable checks it).
- Lights (torch, brazier, hanging fire, chandelier, candles, lantern, crystal, light shaft, glow, spot), pieces (doors, double doors, portcullises, bars, secret doors, levers, pressure plates, traps and the game's markers: arena, checkpoint, loot, spawn), decals (cracks, stains, drips, blood, scorch marks, runes) and library objects (objects_library) dress a place.
- Coordinates are metres: x east, z south, y up. A room's floor_y is absolute; place lights, pieces and objects at heights from it.

Work in this order:
1. Look: dungeons_state and dungeons_rooms, then selection_screenshot (top view). Compare what is there with the description and make a short plan: which rooms the description names (a collapsed forge, a flooded gallery, a throne room, the lair at the end…), the way through, the light and colours.
2. The whole: only if a different layout fits the description much better, call dungeons_layout once, first (it replaces every room, the auto lights and the dressing), then dungeons_wait. Set the style (dungeons_style, with weathering: moss, grime, wet, puddles), the mood (dungeons_mood: a preset, then fill, fog and tint for the light and colours described) and the music (dungeons_music: list the demo tracks and pick the one that suits it). If the description suggests a better name than the one it has, rename it (dungeons_set name).
3. The rooms: reshape, add, re-purpose and remove rooms so every place the description names exists and reads clearly — roles, ceilings, heights, pillars, liquids, irregular walls for natural or ruined places, levels and stairs for depth. Then dungeons_wait and dungeons_walkable; fix what is out of reach.
4. Pieces: doors, gates, secret doors, levers, traps, and the markers (a boss arena where the final fight is, loot, checkpoints).
5. Light: after big room changes call dungeons_lights_auto again (it lights every room in the style's fixtures), then add or recolour lights for the mood (colours as srgb), or tint every light at once with dungeons_lights_all.
6. Dress: after room changes call dungeons_dress again (it furnishes rooms by their role), then add a few hero objects (objects_library, objects_place) and decals (dungeons_decals_age; dungeons_decal_add for blood, scorch marks, runes) where the story needs them.
7. The way out (dungeons_way_out): a frame and colour that fit.
8. Check with selection_screenshot (top, then close views into the key rooms) and dungeons_walkable, and fix what looks wrong.

Rules:
- Stay in this dungeon: don't leave it, don't save, don't use undo, don't change the camera, lighting outside the dungeon or settings.
- Budget: about 60 tool calls (read several things at once where you can). Every call is undoable; the whole session becomes one undo step.
- Reference images: when the user attached pictures, look at them closely first and recreate what they show (a floor plan: its rooms and layout; a picture of a place: its kind of rooms, materials, light, colours and mood). Say in the summary what you matched and what you couldn't.
- Finish with a short summary for the user (1-3 sentences, plain words, no tool names) of the dungeon you made.";

/// Suggestion chips in the panel while an underground dungeon is open.
pub const DUNGEON_SUGGESTIONS: &[(&str, &str)] = &[
    (
        "Throne room",
        "Turn this into a grand throne room: a tall pillared hall with a raised dais for the throne, braziers either side and banners",
    ),
    (
        "Flooded",
        "Flood this part: knee-deep water pools, dripping walls, a few broken steps leading down into the water",
    ),
    (
        "Collapsed",
        "Make this part collapsed: rubble, cracked walls, a caved-in corridor and a narrow way round it",
    ),
    (
        "Treasure vault",
        "Make a treasure vault here behind a secret door, with chests, gold light and a pressure-plate trap",
    ),
    (
        "Crypt",
        "Turn this into a candlelit crypt with burial niches, sarcophagi and old bones",
    ),
    (
        "Library",
        "Make an ancient library here: shelves, reading tables, candles and a hidden reading room",
    ),
    (
        "Prison",
        "Make a prison block here: a corridor of small barred cells, a guard room and a torture chamber",
    ),
    (
        "Cavern",
        "Break this part open into a natural cavern with crystals and a pool",
    ),
    (
        "Boss arena",
        "Make a boss arena here: a big round room with a portcullis at the entrance, a dais, braziers and an arena marker",
    ),
    (
        "Shrine",
        "Make a quiet shrine here with an altar, runes on the floor and soft light falling from above",
    ),
];

/// Suggestion chips in the panel.
pub const SUGGESTIONS: &[(&str, &str)] = &[
    (
        "Volcanic",
        "Turn this into a volcanic area: a volcano with a glowing lava lake, black rock and ash",
    ),
    ("Dense forest", "Turn this into a dense, old pine forest"),
    (
        "Canyon",
        "Carve a deep canyon with a river running through it",
    ),
    (
        "Lake",
        "Make a calm lake with a small island and reeds along the shore",
    ),
    ("Desert dunes", "Turn this into a desert of tall sand dunes"),
    (
        "Swamp",
        "Turn this into a murky swamp with channels, ponds and dead trees",
    ),
    ("Mountains", "Raise snowy mountains with rocky peaks"),
    (
        "Meadow with flowers",
        "Make a gentle meadow full of wildflowers with a few trees",
    ),
    (
        "Mesas",
        "Make a red-rock desert with flat-topped mesas and buttes",
    ),
    (
        "Lagoon",
        "Make a tropical coastal lagoon with a sandy barrier and palms",
    ),
];

/// One line of the step feed.
pub struct FeedItem {
    /// Seconds since the run started.
    pub at: f32,
    pub text: String,
    /// A tool call (None: the agent talking or a status line).
    pub tool: Option<String>,
    /// Tool calls: None while running.
    pub ok: Option<bool>,
    /// The result's first line, or the error.
    pub detail: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunState {
    Idle,
    Running,
    Done,
    Failed,
}

/// An earlier prompt of this conversation (refine carries them).
#[derive(Clone)]
pub struct Turn {
    pub prompt: String,
    pub summary: String,
    pub tools: Vec<String>,
}

struct ActiveRun {
    endpoint: PrivateEndpoint,
    agent: AgentHandle,
    started: Instant,
    /// History serial when the run started (later steps merge into one).
    mark: u64,
    label: String,
    /// Calls waiting for their result: (feed line, our receiver, the HTTP thread's sender).
    inflight: Vec<(usize, Mutex<Receiver<CallResult>>, Sender<CallResult>)>,
    workdir: PathBuf,
    prompt: String,
    tools: Vec<String>,
}

/// Requests from the panel (and the eval harness), handled with world access.
pub enum Request {
    Send { prompt: String, refine: bool },
    Stop,
    Keep,
    Undo,
}

#[derive(Resource)]
pub struct PromptSession {
    pub input: String,
    pub refine_input: String,
    pub refining: bool,
    pub backend: Backend,
    /// The chosen model per backend (as saved; `model()` checks it against what's offered).
    pub models: HashMap<Backend, String>,
    /// Reference images attached to the next prompt (originals; copied for the agent per run).
    pub images: Vec<PathBuf>,
    /// The chosen reasoning effort per backend (Codex).
    pub efforts: HashMap<Backend, String>,
    pub statuses: Option<Vec<BackendStatus>>,
    detect_rx: Option<Mutex<Receiver<Vec<BackendStatus>>>>,
    pub api_key: String,
    pub remember_key: bool,
    /// The API backend's provider (an OpenAI-compatible base URL, `agent::PROVIDERS`).
    pub api_base: String,
    /// Sign in with Claude, while it runs (and its last error).
    pub login: Option<agent::ClaudeLogin>,
    pub login_code: String,
    pub login_error: Option<String>,
    pub state: RunState,
    run: Option<ActiveRun>,
    pub feed: Vec<FeedItem>,
    pub summary: String,
    pub error: Option<String>,
    pub elapsed: f32,
    pub calls: u32,
    /// The undo step the last run made.
    pub step: Option<u64>,
    pub turns: Vec<Turn>,
    api_messages: Vec<Value>,
    claude_session: Option<String>,
    pub panel_open: bool,
    pub requests: Vec<Request>,
    /// The conversation shapes a whole new dungeon (Dungeons › Describe it): Refine keeps to the
    /// whole dungeon and its tools; a new prompt or Keep ends it.
    pub whole_dungeon: bool,
}

impl Default for PromptSession {
    fn default() -> Self {
        // (`WB_PROMPT_BACKEND=<id>`: screenshots of a backend's controls)
        let backend = std::env::var("WB_PROMPT_BACKEND")
            .ok()
            .and_then(|b| Backend::from_id(&b))
            .or_else(|| {
                crate::prefs::get("prompt_backend")
                    .and_then(|v| v.as_str().and_then(Backend::from_id))
            })
            .unwrap_or(Backend::Codex);
        let models = Backend::ALL
            .into_iter()
            .map(|b| {
                let saved = crate::prefs::get(&format!("prompt_model_{}", b.id()))
                    .and_then(|v| v.as_str().map(str::to_string));
                (
                    b,
                    // (the API backend's come from its provider: none until picked)
                    saved.unwrap_or_else(|| {
                        b.models().first().map(|m| m.to_string()).unwrap_or_default()
                    }),
                )
            })
            .collect();
        let efforts = Backend::ALL
            .into_iter()
            .filter_map(|b| {
                Some((
                    b,
                    crate::prefs::get(&format!("prompt_effort_{}", b.id()))?
                        .as_str()?
                        .to_string(),
                ))
            })
            .collect();
        let mut s = Self {
            input: String::new(),
            refine_input: String::new(),
            refining: false,
            backend,
            models,
            efforts,
            images: vec![],
            statuses: None,
            detect_rx: None,
            api_key: saved_key().unwrap_or_default(),
            remember_key: saved_key().is_some(),
            api_base: saved_base(),
            login: None,
            login_code: String::new(),
            login_error: None,
            state: RunState::Idle,
            run: None,
            feed: vec![],
            summary: String::new(),
            error: None,
            elapsed: 0.0,
            calls: 0,
            step: None,
            turns: vec![],
            api_messages: vec![],
            claude_session: None,
            panel_open: false,
            requests: vec![],
            whole_dungeon: false,
        };
        s.detect();
        s
    }
}

/// Where a remembered API key lives (owner-only file, outside the project).
fn key_file() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var("HOME").ok()?)
            .join("Library/Application Support/WorldBuilder/api_key"),
    )
}

/// (where it was kept when the API backend was Anthropic's only)
fn old_key_file() -> Option<PathBuf> {
    Some(key_file()?.with_file_name("anthropic_api_key"))
}

fn read_key(f: Option<PathBuf>) -> Option<String> {
    std::fs::read_to_string(f?)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn saved_key() -> Option<String> {
    read_key(key_file()).or_else(|| read_key(old_key_file()))
}

/// The saved provider; a key kept from before is an Anthropic one.
fn saved_base() -> String {
    if let Some(b) = crate::prefs::get("api_base").and_then(|v| v.as_str().map(str::to_string)) {
        return b;
    }
    let p = if read_key(key_file()).is_none() && read_key(old_key_file()).is_some() {
        2
    } else {
        0
    };
    agent::PROVIDERS[p].base.to_string()
}

impl PromptSession {
    /// Re-checks which backends are installed and logged in (in the background).
    pub fn detect(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let api = self.api_conn();
        std::thread::spawn(move || {
            let _ = tx.send(agent::detect(api));
        });
        self.detect_rx = Some(Mutex::new(rx));
    }

    /// The API backend's provider and key (the panel's, else the provider's environment variable).
    pub fn api_conn(&self) -> agent::ApiConn {
        let base = self.api_base.trim().to_string();
        let key = Some(self.api_key.trim().to_string())
            .filter(|k| !k.is_empty())
            .or_else(|| agent::env_api_key(&base));
        agent::ApiConn { base, key }
    }

    /// Whether this backend can run now (the API's readiness is its key, which may have been
    /// typed in since the last check).
    pub fn ready(&self, b: Backend) -> bool {
        if b == Backend::Api {
            return self.api_conn().ready();
        }
        self.status(b).is_some_and(|st| st.ready)
    }

    pub fn set_api_base(&mut self, base: &str) {
        self.api_base = base.trim().to_string();
        crate::prefs::set("api_base", json!(self.api_base));
        self.detect();
    }

    /// Starts Sign in with Claude (`claude auth login`: the browser opens).
    pub fn start_claude_login(&mut self) {
        self.login_error = None;
        self.login_code.clear();
        match agent::ClaudeLogin::start() {
            Ok(l) => self.login = Some(l),
            Err(e) => self.login_error = Some(e),
        }
    }

    /// Follows a running sign-in; once signed in, checks the backends again.
    fn poll_login(&mut self) {
        let Some(l) = self.login.as_mut() else { return };
        l.poll();
        match l.done.take() {
            Some(Ok(())) => {
                self.login = None;
                self.detect();
            }
            Some(Err(e)) => {
                self.login = None;
                self.login_error = Some(e);
                self.detect();
            }
            None => {}
        }
    }

    pub fn status(&self, b: Backend) -> Option<&BackendStatus> {
        self.statuses.as_ref()?.iter().find(|s| s.backend == b)
    }

    pub fn running(&self) -> bool {
        self.state == RunState::Running
    }

    /// The models the panel offers for the current backend: Codex's own catalog once it has been
    /// read, else a built-in list.
    pub fn model_options(&self) -> Vec<agent::ModelInfo> {
        if let Some(st) = self.status(self.backend)
            && !st.models.is_empty()
        {
            return st.models.clone();
        }
        let fallback = match self.backend {
            Backend::Api => agent::provider(&self.api_base).map_or(&[][..], |p| p.models),
            b => b.models(),
        };
        fallback
            .iter()
            .map(|m| agent::ModelInfo {
                slug: m.to_string(),
                name: m.to_string(),
                description: String::new(),
                efforts: vec![],
                default_effort: String::new(),
            })
            .collect()
    }

    /// The model a run uses: the chosen one if it's still offered, else the best offered.
    pub fn model(&self) -> String {
        let opts = self.model_options();
        // (any model id the provider has: its list is only suggestions)
        if self.backend == Backend::Api {
            return match self.models.get(&self.backend) {
                Some(m) if !m.trim().is_empty() => m.trim().to_string(),
                _ => opts.first().map(|o| o.slug.clone()).unwrap_or_default(),
            };
        }
        match self.models.get(&self.backend) {
            Some(m) if opts.iter().any(|o| o.slug == *m) => m.clone(),
            _ => opts
                .first()
                .map(|o| o.slug.clone())
                .or_else(|| self.backend.models().first().map(|m| m.to_string()))
                .unwrap_or_default(),
        }
    }

    /// The reasoning effort a run uses (`None` when the model doesn't offer a choice).
    pub fn effort(&self) -> Option<String> {
        let model = self.model();
        let info = self.model_options().into_iter().find(|o| o.slug == model)?;
        if info.efforts.is_empty() {
            return None;
        }
        match self.efforts.get(&self.backend) {
            Some(e) if info.efforts.contains(e) => Some(e.clone()),
            _ if info.efforts.iter().any(|e| e == "medium") => Some("medium".into()),
            _ => Some(info.default_effort.clone()).filter(|e| !e.is_empty()),
        }
    }

    pub fn set_effort(&mut self, e: &str) {
        self.efforts.insert(self.backend, e.to_string());
        crate::prefs::set(&format!("prompt_effort_{}", self.backend.id()), json!(e));
    }

    /// "GPT-6.1-Sol · High" for the panel.
    pub fn model_label(&self) -> String {
        let m = self.model();
        let name = self
            .model_options()
            .into_iter()
            .find(|o| o.slug == m)
            .map(|o| o.name)
            .unwrap_or(m);
        match self.effort() {
            Some(e) => format!("{name} · {}", agent::effort_name(&e)),
            None => name,
        }
    }

    pub fn set_backend(&mut self, b: Backend) {
        self.backend = b;
        crate::prefs::set("prompt_backend", json!(b.id()));
    }

    pub fn set_model(&mut self, m: &str) {
        self.models.insert(self.backend, m.to_string());
        crate::prefs::set(&format!("prompt_model_{}", self.backend.id()), json!(m));
    }

    /// Saves (or forgets) the API key on this computer.
    pub fn store_key(&mut self) {
        let Some(f) = key_file() else { return };
        // (the old file goes either way: the key now lives in one place)
        if let Some(old) = old_key_file() {
            let _ = std::fs::remove_file(old);
        }
        if self.remember_key && !self.api_key.trim().is_empty() {
            if let Some(d) = f.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if std::fs::write(&f, self.api_key.trim()).is_ok() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600));
            }
        } else {
            let _ = std::fs::remove_file(&f);
        }
    }

    fn push(&mut self, text: impl Into<String>) {
        self.feed.push(FeedItem {
            at: self.elapsed,
            text: text.into(),
            tool: None,
            ok: None,
            detail: None,
        });
    }
}

pub struct PromptPlugin;

impl Plugin for PromptPlugin {
    fn build(&self, app: &mut App) {
        use crate::tools::ToolSystems;
        app.init_resource::<Selection>()
            .init_resource::<mask::MaskState>()
            .init_resource::<PromptSession>()
            .init_resource::<tools::ShotSky>()
            .add_systems(PreUpdate, drain_calls.after(bevy::input::InputSystems))
            .add_systems(
                Update,
                (selection::dungeon_floor, selection::lasso_input)
                    .chain()
                    .after(crate::camera::CameraSystems)
                    .before(ToolSystems::Input),
            )
            .add_systems(Update, dropped_images)
            .add_systems(
                Update,
                mask::settle_open_step
                    .after(ToolSystems::Apply)
                    .before(ToolSystems::Commit),
            )
            .add_systems(
                Update,
                (test_hooks, handle_requests, poll_run)
                    .chain()
                    .after(ToolSystems::Commit),
            )
            .add_systems(
                Last,
                (mask::settle_committed, selection::update_overlay).chain(),
            )
            .add_systems(Update, tools::hold_clear_sky.after(crate::sky::SkySystems))
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                panel::panel_ui.run_if(crate::mcp::ui_visible),
            );
        tools::register(app);
        eval::register(app);
    }
}

/// A short, plain description of a tool call for the step feed.
pub fn describe_call(name: &str, args: &Value) -> String {
    let s = |k: &str| {
        args.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .replace('_', " ")
    };
    match name {
        "selection_info" => "Studying the area…".into(),
        "selection_screenshot" => "Looking at the area…".into(),
        "terrain_sample" => "Measuring the ground…".into(),
        "world_info" => "Reading the world…".into(),
        "objects_library" | "materials_list" | "water_presets" => "Browsing the library…".into(),
        "objects_query" | "water_list" => "Checking what's there…".into(),
        "terrain_feature" => match s("feature").as_str() {
            "volcano" => "Raising a volcano…".into(),
            "mountain range" => "Raising a mountain range…".into(),
            "ridge" => "Raising a ridge…".into(),
            "crater" => "Blasting a crater…".into(),
            "mesas" | "buttes" => format!("Raising {}…", s("feature")),
            "canyon" => "Carving a canyon…".into(),
            "valley" => "Carving a valley…".into(),
            "dunes" => "Shaping sand dunes…".into(),
            "hills" => "Rolling out hills…".into(),
            "plateau" => "Lifting a plateau…".into(),
            "sea cliffs" => "Cutting sea cliffs…".into(),
            "lagoon" => "Shaping a lagoon…".into(),
            "swamp" => "Sinking a swamp…".into(),
            "lake" => "Filling a lake…".into(),
            f => format!("Building {f}…"),
        },
        "area_edit" => match s("op").as_str() {
            "paint" if args.get("erase").and_then(|v| v.as_bool()) == Some(true) => {
                "Restoring natural ground…".into()
            }
            "paint" => format!("Painting {}…", s("layer")),
            "raise" => "Lifting the ground…".into(),
            "lower" => "Lowering the ground…".into(),
            "flatten" => "Levelling the ground…".into(),
            "smooth" => "Smoothing the ground…".into(),
            "roughen" => "Roughening the ground…".into(),
            "terrace" => "Cutting terraces…".into(),
            "biome" => format!("Turning it into {}…", s("biome")),
            "scatter" => format!(
                "Planting {}…",
                if s("preset").is_empty() {
                    "nature".to_string()
                } else {
                    s("preset")
                }
            ),
            "clear nature" => "Clearing vegetation…".into(),
            o => format!("Area edit: {o}…"),
        },
        "terrain_sculpt" => match s("tool").as_str() {
            "raise" => "Raising ground…".into(),
            "lower" => "Digging…".into(),
            "smooth" => "Smoothing…".into(),
            "flatten" => "Flattening…".into(),
            "mountains" => "Growing peaks…".into(),
            "hills" => "Adding hills…".into(),
            "terraces" => "Cutting terraces…".into(),
            "erode" => "Eroding…".into(),
            t => format!("Sculpting ({t})…"),
        },
        "terrain_paint" => format!("Painting {}…", s("layer")),
        "terrain_set_biome" => format!("Turning it into {}…", s("biome")),
        "nature_scatter" if s("mode") == "remove" => "Clearing vegetation…".into(),
        "nature_scatter" => format!(
            "Planting {}…",
            if s("preset").is_empty() {
                "trees and plants".to_string()
            } else {
                s("preset")
            }
        ),
        "objects_place" => "Placing objects…".into(),
        "objects_transform" => "Adjusting objects…".into(),
        "objects_remove" => "Removing objects…".into(),
        "objects_glow" => "Colouring crystals…".into(),
        "water_add_lake" => "Filling a lake…".into(),
        "water_add_river" => "Drawing a river…".into(),
        "water_set_style" => "Restyling water…".into(),
        "water_remove" => "Removing water…".into(),
        "fish_add" => "Releasing fish…".into(),
        "dungeons_rooms" | "dungeons_state" | "dungeons_lights" | "dungeons_pieces"
        | "dungeons_decals" | "dungeons_catalog" => "Studying the dungeon…".into(),
        "dungeons_room_add" => format!(
            "Building a {}…",
            if s("kind").is_empty() {
                "room".to_string()
            } else {
                s("kind")
            }
        ),
        "dungeons_room_set" => "Reshaping a room…".into(),
        "dungeons_room_remove" => "Removing a room…".into(),
        "dungeons_wall_opening" => "Opening up a wall…".into(),
        "dungeons_gallery_hall" => "Raising a galleried hall…".into(),
        "dungeons_level" => "Adding a level…".into(),
        "dungeons_light_add" => format!(
            "Lighting it ({})…",
            if s("kind").is_empty() {
                "light".to_string()
            } else {
                s("kind")
            }
        ),
        "dungeons_light_set" | "dungeons_light_remove" => "Adjusting the light…".into(),
        "dungeons_piece_add" => format!(
            "Placing a {}…",
            if s("kind").is_empty() {
                "piece".to_string()
            } else {
                s("kind")
            }
        ),
        "dungeons_piece_set" | "dungeons_piece_remove" => "Adjusting pieces…".into(),
        "dungeons_decal_add" | "dungeons_decal_remove" => "Weathering the walls…".into(),
        "dungeons_walkable" => "Checking the way through…".into(),
        "dungeons_probe" | "dungeons_wait" => "Checking the space…".into(),
        "dungeons_layout" => format!(
            "Laying the dungeon out{}…",
            if s("layout").is_empty() {
                String::new()
            } else {
                format!(" ({})", s("layout"))
            }
        ),
        "dungeons_style" => "Choosing its stone and timber…".into(),
        "dungeons_mood" => "Setting the mood…".into(),
        "dungeons_music" => "Choosing the music…".into(),
        "dungeons_set" => "Naming it…".into(),
        "dungeons_lights_auto" => "Lighting every room…".into(),
        "dungeons_lights_all" => "Tuning all the lights…".into(),
        "dungeons_dress" => "Furnishing the rooms…".into(),
        "dungeons_decals_age" => "Ageing the walls…".into(),
        "dungeons_pieces_auto" => "Hanging doors and gates…".into(),
        "dungeons_way_out" => "Setting up the way out…".into(),
        "dungeons_wall" => "Working on a wall…".into(),
        "dungeons_minimap" => "Drawing the map…".into(),
        n => format!("{}…", n.replace('_', " ")),
    }
}

/// The prompt sent to the agent: the area's key numbers, earlier turns, the request.
fn compose_prompt(world: &mut World, text: &str, turns: &[Turn], carry_turns: bool) -> String {
    let poly = world
        .resource::<Selection>()
        .polygon()
        .map(|p| p.to_vec())
        .unwrap_or_default();
    if let Some(m) = world
        .resource::<crate::dungeons::DungeonMap>()
        .0
        .as_ref()
        .filter(|m| m.kind == crate::dungeons::model::MapKind::Interior)
    {
        let level = world
            .resource::<crate::dungeons::render::DungeonView>()
            .level;
        let (lo, hi) = selection::bounds(&poly);
        let info = crate::dungeons::selection_json(m, &poly, level);
        let rooms: Vec<String> = info["rooms"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| {
                        format!(
                            "{} ({}, level {})",
                            r["name"].as_str().unwrap_or("?"),
                            r["kind"].as_str().unwrap_or("?"),
                            r["level"]
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut s = format!(
            "Selected area of the dungeon \"{}\" ({:.0} × {:.0} m, from [{:.0}, {:.0}] to [{:.0}, {:.0}]), editing level {level}. Rooms touching it: {}. Call selection_info for the outline and details.\n",
            m.name,
            hi.x - lo.x,
            hi.y - lo.y,
            lo.x,
            lo.y,
            hi.x,
            hi.y,
            if rooms.is_empty() {
                "none (empty rock)".to_string()
            } else {
                rooms.join(", ")
            }
        );
        if carry_turns && !turns.is_empty() {
            s += "\nEarlier in this session (already done, in the dungeon now):\n";
            for (i, tn) in turns.iter().enumerate() {
                s += &format!(
                    "{}. Request: {}\n   You did: {}\n   Tools used: {}\n",
                    i + 1,
                    tn.prompt,
                    tn.summary,
                    tn.tools.join(", ")
                );
            }
            s += "\nNow refine it as asked:\n";
        }
        s += &format!("\nRequest: {text}\n");
        return s;
    }
    let t = world.resource::<crate::terrain::Terrain>();
    let (lo, hi) = selection::bounds(&poly);
    let mut hs: Vec<f32> = vec![];
    for k in 0..400 {
        let p = lo + (hi - lo) * Vec2::new((k % 20) as f32 / 19.0, (k / 20) as f32 / 19.0);
        if crate::biomes::point_in_polygon(p, &poly) {
            hs.push(t.height_at(p.x, p.y));
        }
    }
    hs.sort_by(|a, b| a.total_cmp(b));
    let (hmin, hmed, hmax) = (
        hs.first().copied().unwrap_or(0.0),
        hs.get(hs.len() / 2).copied().unwrap_or(0.0),
        hs.last().copied().unwrap_or(0.0),
    );
    let sea = hs.iter().filter(|h| **h < t.sea_level).count() as f32 / hs.len().max(1) as f32;
    let mut s = format!(
        "Selected area: {} ({:.0} × {:.0} m) around [{:.0}, {:.0}], from [{:.0}, {:.0}] to [{:.0}, {:.0}]. Ground {:.0}…{:.0} m (median {:.0}); sea level {:.0} m; {:.0}% of it is sea. Call selection_info for the outline and details.\n",
        selection::area_text(selection::area(&poly)),
        hi.x - lo.x,
        hi.y - lo.y,
        (lo.x + hi.x) * 0.5,
        (lo.y + hi.y) * 0.5,
        lo.x,
        lo.y,
        hi.x,
        hi.y,
        hmin,
        hmax,
        hmed,
        t.sea_level,
        sea * 100.0
    );
    if carry_turns && !turns.is_empty() {
        s += "\nEarlier in this session (already done, in the world now):\n";
        for (i, tn) in turns.iter().enumerate() {
            s += &format!(
                "{}. Request: {}\n   You did: {}\n   Tools used: {}\n",
                i + 1,
                tn.prompt,
                tn.summary,
                tn.tools.join(", ")
            );
        }
        s += "\nNow refine it as asked:\n";
    }
    s += &format!("\nRequest: {text}\n");
    s
}

/// The prompt for shaping the whole open dungeon from the user's description: what it was made
/// from (`made`, from Dungeons › Describe it), what it is now, earlier turns, the description.
fn compose_whole(
    world: &mut World,
    text: &str,
    made: Option<&str>,
    turns: &[Turn],
    carry_turns: bool,
) -> String {
    let poly = world
        .resource::<Selection>()
        .polygon()
        .map(|p| p.to_vec())
        .unwrap_or_default();
    let (lo, hi) = selection::bounds(&poly);
    let mut s = String::new();
    if let Some(m) = world.resource::<crate::dungeons::DungeonMap>().0.as_ref() {
        let p = &m.plan;
        s += &format!(
            "The open dungeon: \"{}\", {} rooms on {} level{}, {} lights, {} dressing objects, style {}, mood {}.\n",
            m.name,
            p.rooms.len(),
            p.levels.len(),
            if p.levels.len() == 1 { "" } else { "s" },
            m.lights.len(),
            m.props.len(),
            m.style,
            m.mood.preset
        );
    }
    if let Some(made) = made.filter(|m| !m.trim().is_empty()) {
        s += made.trim();
        s += "\n";
    }
    s += &format!(
        "The selection is the whole dungeon (from [{:.0}, {:.0}] to [{:.0}, {:.0}]); call selection_info for the details.\n",
        lo.x, lo.y, hi.x, hi.y
    );
    if carry_turns && !turns.is_empty() {
        s += "\nEarlier in this session (already done, in the dungeon now):\n";
        for (i, tn) in turns.iter().enumerate() {
            s += &format!(
                "{}. Request: {}\n   You did: {}\n   Tools used: {}\n",
                i + 1,
                tn.prompt,
                tn.summary,
                tn.tools.join(", ")
            );
        }
        s += "\nNow refine it as asked:\n";
        s += &format!("\nRequest: {text}\n");
    } else {
        s += &format!(
            "\nThe user's description of the dungeon they want:\n\"\"\"\n{text}\n\"\"\"\n"
        );
    }
    s
}

/// Shapes the whole open underground dungeon from a description with the connected AI (Dungeons ›
/// Describe it): the whole dungeon must already be selected. `made`: what it was made from (its
/// layout, style, mood… in plain words). The run shows in the prompt panel (Stop, then Keep /
/// Undo / Refine) like any area edit.
pub fn start_dungeon_shape(
    world: &mut World,
    text: &str,
    images: Vec<PathBuf>,
    made: String,
) -> Result<(), String> {
    if !world.resource::<crate::dungeons::DungeonMap>().interior() {
        return Err("No underground dungeon is open".into());
    }
    {
        // (a new dungeon: a conversation of its own)
        let mut s = world.resource_mut::<PromptSession>();
        s.images = images;
        s.turns.clear();
        s.api_messages.clear();
        s.claude_session = None;
    }
    let r = start_run_as(world, text.to_string(), false, Some(made), None);
    let mut s = world.resource_mut::<PromptSession>();
    match &r {
        Ok(()) => s.input = text.to_string(),
        Err(e) => s.error = Some(e.clone()),
    }
    r
}

/// Starts an area edit of the current selection (as the panel's Send does) in a conversation of its
/// own, with these reference images and `context` for the agent (not shown as the request); used
/// for a realm made from a description (the whole realm selected).
pub fn start_area_edit(
    world: &mut World,
    text: &str,
    images: Vec<PathBuf>,
    context: String,
) -> Result<(), String> {
    {
        let mut s = world.resource_mut::<PromptSession>();
        s.images = images;
        s.turns.clear();
        s.api_messages.clear();
        s.claude_session = None;
    }
    let r = start_run_as(world, text.to_string(), false, None, Some(context));
    if r.is_ok() {
        world.resource_mut::<PromptSession>().input = text.to_string();
    }
    if let Err(e) = &r {
        world.resource_mut::<PromptSession>().error = Some(e.clone());
    }
    r
}

/// The AI the next run would use is ready (or the test stub `WB_AGENT_STUB` stands in): its name.
pub fn ai_ready(s: &PromptSession) -> Option<&'static str> {
    if agent::stub_path().is_some() {
        return Some("Test stub");
    }
    s.ready(s.backend).then_some(s.backend.name())
}

/// The request's note on attached reference images (the copies the agent sees, and the
/// originals for reference_load).
fn image_note(originals: &[PathBuf], copies: &[PathBuf], whole_world: bool) -> String {
    if originals.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "\nThe user attached {} reference image{} (shown to you with this message",
        originals.len(),
        if originals.len() == 1 { "" } else { "s" }
    );
    s += if copies.is_empty() {
        ")."
    } else {
        "; copies in your working folder: "
    };
    s += &copies
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(", ");
    s += if copies.is_empty() { "\n" } else { ").\n" };
    s += &format!(
        "Original files (full size, for reference_load): {}\n",
        originals
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if whole_world {
        s += "The selection is the whole world.\n";
    }
    s
}

/// Starts a run (with world access).
fn start_run(world: &mut World, text: String, refine: bool) -> Result<(), String> {
    start_run_as(world, text, refine, None, None)
}

/// Starts a run. `whole`: the whole open dungeon is shaped from a description (what it was made
/// from); a refine of such a run stays whole-dungeon. `context`: more for the agent, after the
/// request.
fn start_run_as(
    world: &mut World,
    text: String,
    refine: bool,
    whole: Option<String>,
    context: Option<String>,
) -> Result<(), String> {
    if world.resource::<PromptSession>().running() {
        return Err("an edit is already running".into());
    }
    if world.resource::<Selection>().polygon().is_none() {
        return Err("Select an area first (G, or the sparkle button)".into());
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Describe what you want in the area".into());
    }
    let (backend, model, status, api, turns, messages, resume) = {
        let s = world.resource::<PromptSession>();
        (
            s.backend,
            (s.model(), s.effort()),
            s.status(s.backend).cloned(),
            s.api_conn(),
            if refine { s.turns.clone() } else { vec![] },
            if refine {
                s.api_messages.clone()
            } else {
                vec![]
            },
            if refine {
                s.claude_session.clone()
            } else {
                None
            },
        )
    };
    // (the API's readiness is the key, which may have been typed in since the check)
    let stub = agent::stub_path().is_some();
    if backend != Backend::Api
        && !stub
        && let Some(st) = &status
        && !st.ready
    {
        return Err(format!("{}: {}", backend.name(), st.note));
    }
    if backend == Backend::Api && !stub {
        if !api.ready() {
            return Err("Add an API key first".into());
        }
        if model.0.is_empty() {
            return Err("Pick a model for the API first".into());
        }
    }
    // the agent's tools, on an endpoint of its own (a dungeon's in an underground dungeon)
    let dungeon = world.resource::<crate::dungeons::DungeonMap>().interior();
    let whole_dungeon =
        dungeon && (whole.is_some() || (refine && world.resource::<PromptSession>().whole_dungeon));
    let allowed = |name: &str| {
        if whole_dungeon {
            tools::DUNGEON_SHAPE_TOOLS.contains(&name)
        } else if dungeon {
            tools::DUNGEON_AGENT_TOOLS.contains(&name)
        } else {
            tools::area_agent_allows(name)
        }
    };
    let tools: Vec<crate::mcp_tools::McpTool> = world
        .resource::<crate::mcp_tools::McpTools>()
        .tools
        .iter()
        .filter(|t| allowed(t.name))
        .cloned()
        .collect();
    let endpoint = PrivateEndpoint::open(&tools)?;
    let workdir = std::env::temp_dir().join(format!(
        "world-builder-agent-{}-{}",
        std::process::id(),
        fastrand::u32(..)
    ));
    std::fs::create_dir_all(&workdir).map_err(|e| e.to_string())?;
    // the API and Claude Code keep the real conversation; Codex runs are ephemeral, so earlier
    // turns travel in the prompt
    let carry = refine && backend == Backend::Codex;
    let prompt = if whole_dungeon {
        compose_whole(world, &text, whole.as_deref(), &turns, carry)
    } else {
        compose_prompt(world, &text, &turns, carry)
    };
    // reference images: shown with the first message (Codex runs are ephemeral: every message)
    let originals = if !refine || backend == Backend::Codex {
        world.resource::<PromptSession>().images.clone()
    } else {
        vec![]
    };
    let mut images = vec![];
    for (i, p) in originals.iter().enumerate() {
        images.push(agent::prepare_image(p, &workdir, i)?);
    }
    let whole_world = {
        let t = world.resource::<crate::terrain::Terrain>();
        let poly = world
            .resource::<Selection>()
            .polygon()
            .map(|p| p.to_vec())
            .unwrap_or_default();
        let (lo, hi) = selection::bounds(&poly);
        !dungeon && (hi - lo).min_element() >= t.size() * 0.9
    };
    let prompt = prompt
        + &context
            .map(|c| format!("\n{}\n", c.trim()))
            .unwrap_or_default()
        + &image_note(&originals, &images, whole_world);
    let (model, effort) = model;
    let spec = agent::RunSpec {
        backend,
        model,
        effort,
        features: status
            .as_ref()
            .map(|s| s.features.clone())
            .unwrap_or_default(),
        exe: status.as_ref().and_then(|s| s.path.clone()),
        instructions: if whole_dungeon {
            DUNGEON_SHAPE_INSTRUCTIONS
        } else if dungeon {
            DUNGEON_INSTRUCTIONS
        } else {
            INSTRUCTIONS
        }
        .to_string(),
        prompt,
        port: endpoint.port,
        api: Some(api),
        messages,
        resume,
        timeout: RUN_TIMEOUT,
        workdir: workdir.clone(),
        images,
    };
    let mark = world.resource::<crate::history::History>().serial();
    let handle = agent::start(spec);
    world.resource_mut::<Selection>().forced += 1;
    let label: String = format!(
        "{}: {}",
        if whole_dungeon {
            "Shape dungeon"
        } else {
            "Prompt"
        },
        text.chars().take(40).collect::<String>()
    );
    info!(
        "prompt run: {} ({}) on port {} — {text}",
        backend.name(),
        world.resource::<PromptSession>().model_label(),
        endpoint.port
    );
    let mut s = world.resource_mut::<PromptSession>();
    s.feed.clear();
    s.summary.clear();
    s.error = None;
    s.elapsed = 0.0;
    s.calls = 0;
    s.state = RunState::Running;
    s.refining = false;
    s.panel_open = true;
    s.whole_dungeon = whole_dungeon;
    s.run = Some(ActiveRun {
        endpoint,
        agent: handle,
        started: Instant::now(),
        mark,
        label,
        inflight: vec![],
        workdir,
        prompt: text,
        tools: vec![],
    });
    Ok(())
}

/// Ends the run: merges its edits into one undo step and records the turn.
fn finish_run(world: &mut World, ok: bool, summary: String, error: Option<String>) {
    let Some(run) = world.resource_mut::<PromptSession>().run.take() else {
        return;
    };
    let step = world
        .resource_mut::<crate::history::History>()
        .squash_after(run.mark, run.label.clone());
    let changed = step.is_some_and(|s| s > run.mark);
    {
        let mut sel = world.resource_mut::<Selection>();
        sel.forced = sel.forced.saturating_sub(1);
    }
    let _ = std::fs::remove_dir_all(&run.workdir);
    let elapsed = run.started.elapsed().as_secs_f32();
    let mut s = world.resource_mut::<PromptSession>();
    s.elapsed = elapsed;
    s.step = if changed { step } else { None };
    // a stopped or failed run that changed the world still offers Keep / Undo
    s.state = if ok || changed {
        RunState::Done
    } else {
        RunState::Failed
    };
    s.summary = if ok {
        summary.clone()
    } else if changed {
        format!("Stopped part-way ({} steps done).", s.calls)
    } else {
        String::new()
    };
    s.error = error.clone();
    if ok {
        s.turns.push(Turn {
            prompt: run.prompt.clone(),
            summary: if summary.is_empty() {
                "(no summary)".into()
            } else {
                summary.clone()
            },
            tools: run.tools.clone(),
        });
    }
    let calls = s.calls;
    info!(
        "prompt run finished: ok={ok} {elapsed:.0}s {calls} calls; {}",
        error.as_deref().unwrap_or(&summary)
    );
    drop(s);
    let msg = if ok {
        format!("Area edit done in {elapsed:.0} s ({calls} steps)")
    } else {
        format!("Area edit stopped: {}", error.unwrap_or_default())
    };
    world.resource_mut::<crate::editor::Editor>().toast(msg);
    world.resource_mut::<crate::world::WorldMeta>().dirty |= changed;
    drop(run.endpoint);
}

/// Runs the agent's queued tool calls (like the MCP server's own executor) and merges its edits
/// into one undo step as it goes.
fn drain_calls(world: &mut World) {
    if world.resource::<PromptSession>().run.is_none()
        || world.resource::<crate::tools::Stroke>().active
    {
        return;
    }
    let t0 = Instant::now();
    loop {
        let Some(mut job) = world
            .resource::<PromptSession>()
            .run
            .as_ref()
            .and_then(|r| r.endpoint.take_job())
        else {
            break;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let reply = std::mem::replace(&mut job.reply, tx);
        let over = {
            let mut s = world.resource_mut::<PromptSession>();
            s.calls += 1;
            let at = s.elapsed;
            let text = describe_call(&job.name, &job.args);
            s.feed.push(FeedItem {
                at,
                text,
                tool: Some(job.name.clone()),
                ok: None,
                detail: None,
            });
            let line = s.feed.len() - 1;
            let calls = s.calls;
            let run = s.run.as_mut().expect("run");
            run.inflight.push((line, Mutex::new(rx), reply));
            if !run.tools.contains(&job.name) {
                run.tools.push(job.name.clone());
            }
            calls
                > if s.whole_dungeon {
                    MAX_CALLS_WHOLE
                } else {
                    MAX_CALLS
                }
        };
        let outside = if over {
            Ok(())
        } else {
            area_guard(world, &job.name, &mut job.args)
        };
        if let Err(e) = outside {
            let _ = job.reply.send(Err(e));
        } else if over {
            let cap = if world.resource::<PromptSession>().whole_dungeon {
                MAX_CALLS_WHOLE
            } else {
                MAX_CALLS
            };
            let _ = job.reply.send(Err(format!("The tool-call budget ({cap}) is used up: stop editing and reply with your short summary.")));
        } else {
            crate::mcp::run_job(world, job);
        }
        // keep the whole run one undo step
        let (mark, label) = {
            let r = world
                .resource::<PromptSession>()
                .run
                .as_ref()
                .map(|r| (r.mark, r.label.clone()));
            let Some(x) = r else { break };
            x
        };
        world
            .resource_mut::<crate::history::History>()
            .squash_after(mark, label);
        if t0.elapsed() > Duration::from_millis(12) {
            break;
        }
    }
}

/// Arguments that hold world positions ([x, z] or [x, y, z], or lists of them).
const COORD_KEYS: &[&str] = &[
    "polygon", "points", "position", "path", "holes", "center", "centre", "at", "outline",
];

/// Collects the [x, z] of every position under a coordinate key of `v`.
fn coords(v: &Value, under: bool, out: &mut Vec<Vec2>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                coords(v, under || COORD_KEYS.contains(&k.as_str()), out);
            }
        }
        Value::Array(a) if under => {
            let nums: Vec<f32> = a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
            if nums.len() == a.len() && (nums.len() == 2 || nums.len() == 3) {
                out.push(Vec2::new(nums[0], nums[nums.len() - 1]));
            } else {
                for v in a {
                    coords(v, true, out);
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                coords(v, false, out);
            }
        }
        _ => {}
    }
}

/// Keeps an outdoor area agent's tools that the edit mask doesn't cover (fields, roads, caves,
/// gameplay markers) inside the selected area: positions more than a few metres outside it are
/// refused, `fields_auto` is limited to the area and `fields_remove` to fields in it.
fn area_guard(world: &mut World, name: &str, args: &mut Value) -> Result<(), String> {
    if !tools::unmasked(name) || world.resource::<crate::dungeons::DungeonMap>().interior() {
        return Ok(());
    }
    let Some(poly) = world.resource::<Selection>().polygon().map(|p| p.to_vec()) else {
        return Ok(());
    };
    const SLACK: f32 = 4.0;
    let inside = |p: Vec2| crate::setpieces::poly_sdf(&poly, p) <= SLACK;
    let (lo, hi) = selection::bounds(&poly);
    match name {
        "fields_auto" => {
            // (an `area` of its own is narrowed to the selection's bounds)
            let (mut a, mut b) = (lo, hi);
            if let Some(r) = args.get("area") {
                let p = |k: &str| {
                    r.get(k)
                        .and_then(|v| serde_json::from_value::<[f32; 2]>(v.clone()).ok())
                        .map(Vec2::from_array)
                };
                if let (Some(p0), Some(p1)) = (p("min"), p("max")) {
                    a = a.max(p0.min(p1));
                    b = b.min(p0.max(p1));
                }
            }
            if a.x >= b.x || a.y >= b.y {
                return Err("that area is outside the selected area".into());
            }
            args["area"] = serde_json::json!({ "min": [a.x, a.y], "max": [b.x, b.y] });
            return Ok(());
        }
        "fields_remove" | "fields_set" => {
            let fields = world.resource::<crate::fields::Fields>();
            let ids: Vec<u64> = match args.get("ids") {
                Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
                None => args.get("id").and_then(|v| v.as_u64()).into_iter().collect(),
            };
            if let Some(f) = ids
                .iter()
                .filter_map(|id| fields.get(*id))
                .find(|f| !inside(f.centre()))
            {
                return Err(format!(
                    "field {} is outside the selected area: leave it alone",
                    f.id
                ));
            }
            return Ok(());
        }
        _ => {}
    }
    let mut pts = vec![];
    coords(args, false, &mut pts);
    match pts.into_iter().find(|p| !inside(*p)) {
        Some(p) => Err(format!(
            "[{:.0}, {:.0}] is outside the selected area (from [{:.0}, {:.0}] to [{:.0}, {:.0}], outline in selection_info): keep everything inside it",
            p.x, p.y, lo.x, lo.y, hi.x, hi.y
        )),
        None => Ok(()),
    }
}

fn first_line(out: &[Out]) -> Option<String> {
    out.iter().find_map(|o| match o {
        Out::Text(t) => Some(t.lines().next().unwrap_or("").chars().take(160).collect()),
        Out::Png(_) => None,
    })
}

/// Agent events, finished calls, the clock; detection results.
fn poll_run(world: &mut World) {
    {
        let mut s = world.resource_mut::<PromptSession>();
        s.poll_login();
        let got = s
            .detect_rx
            .as_ref()
            .and_then(|rx| rx.lock().ok()?.try_recv().ok());
        if let Some(st) = got {
            // a backend that isn't ready gives way to one that is
            if !st.iter().any(|x| x.backend == s.backend && x.ready)
                && let Some(b) = st.iter().find(|x| x.ready).map(|x| x.backend)
                && crate::prefs::get("prompt_backend").is_none()
            {
                s.backend = b;
            }
            s.statuses = Some(st);
            s.detect_rx = None;
        }
    }
    let mut end: Option<(bool, String, Option<String>)> = None;
    {
        let mut s = world.resource_mut::<PromptSession>();
        let s = &mut *s;
        let Some(run) = s.run.as_mut() else { return };
        s.elapsed = run.started.elapsed().as_secs_f32();
        // results of calls (forwarded to the agent)
        let mut i = 0;
        while i < run.inflight.len() {
            let got = run.inflight[i]
                .1
                .lock()
                .map_err(|_| std::sync::mpsc::TryRecvError::Disconnected)
                .and_then(|r| r.try_recv());
            match got {
                Ok(res) => {
                    let (line, _, reply) = run.inflight.remove(i);
                    if let Some(f) = s.feed.get_mut(line) {
                        f.ok = Some(res.is_ok());
                        f.detail = match &res {
                            Ok(out) => first_line(out),
                            Err(e) => Some(e.chars().take(200).collect()),
                        };
                    }
                    let _ = reply.send(res);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => i += 1,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let (line, _, reply) = run.inflight.remove(i);
                    if let Some(f) = s.feed.get_mut(line) {
                        f.ok = Some(false);
                    }
                    let _ = reply.send(Err("The call was dropped".into()));
                }
            }
        }
        let mut events = vec![];
        if let Ok(rx) = run.agent.rx.lock() {
            events.extend(rx.try_iter());
        }
        for ev in events {
            match ev {
                AgentEvent::Status(t) => s.push(t),
                AgentEvent::Message(t) => s.push(t),
                AgentEvent::Finished {
                    summary,
                    session,
                    messages,
                } => {
                    if session.is_some() {
                        s.claude_session = session;
                    }
                    if !messages.is_empty() {
                        s.api_messages = messages;
                    }
                    end = Some((true, summary, None));
                }
                AgentEvent::Failed(e) => end = Some((false, String::new(), Some(e))),
            }
        }
    }
    if let Some((ok, summary, err)) = end {
        finish_run(world, ok, summary, err);
    }
}

/// `WB_PROMPT_PANEL` / `WB_PROMPT_SEND` (see the module doc).
fn test_hooks(mut done: Local<bool>, sel: Res<Selection>, mut s: ResMut<PromptSession>) {
    if *done || sel.polygon().is_none() || s.statuses.is_none() {
        return;
    }
    let Ok(text) = std::env::var("WB_PROMPT_PANEL") else {
        *done = true;
        return;
    };
    *done = true;
    s.panel_open = true;
    s.input = text.clone();
    if std::env::var("WB_PROMPT_SEND").is_ok() {
        s.requests.push(Request::Send {
            prompt: text,
            refine: false,
        });
    }
}

fn handle_requests(world: &mut World) {
    let reqs = std::mem::take(&mut world.resource_mut::<PromptSession>().requests);
    for r in reqs {
        match r {
            Request::Send { prompt, refine } => {
                if let Err(e) = start_run(world, prompt, refine) {
                    let mut s = world.resource_mut::<PromptSession>();
                    s.error = Some(e.clone());
                    s.panel_open = true;
                    drop(s);
                    world.resource_mut::<crate::editor::Editor>().toast(e);
                }
            }
            Request::Stop => {
                if let Some(run) = &world.resource::<PromptSession>().run {
                    run.agent.stop();
                }
                finish_run(world, false, String::new(), Some("Stopped".into()));
            }
            Request::Keep => {
                // done with this area: close the panel and drop the selection
                let mut s = world.resource_mut::<PromptSession>();
                s.state = RunState::Idle;
                s.feed.clear();
                s.input.clear();
                s.refining = false;
                s.panel_open = false;
                s.turns.clear();
                s.whole_dungeon = false;
                world.resource_mut::<selection::Selection>().clear();
                world
                    .resource_mut::<crate::editor::Editor>()
                    .toast("Kept. ⌘Z still undoes it.");
            }
            Request::Undo => {
                let step = world.resource::<PromptSession>().step;
                let top = world.resource::<crate::history::History>().undo_serial();
                if step.is_some() && step == top {
                    world.write_message(crate::tools::EditorAction::Undo);
                    let mut s = world.resource_mut::<PromptSession>();
                    s.state = RunState::Idle;
                    s.step = None;
                    s.turns.pop();
                    s.feed.clear();
                } else if step.is_none() {
                    world.resource_mut::<PromptSession>().state = RunState::Idle;
                } else {
                    world
                        .resource_mut::<crate::editor::Editor>()
                        .toast("Undo the edits made after this one first (⌘Z)");
                }
            }
        }
    }
}

/// An image dropped on the window becomes a reference image for the prompt: added to the open
/// panel, or (no panel) the panel opens on the whole world with it — "recreate this map". (While
/// Dungeons › Describe it is open, it goes to the dungeon's description instead.)
fn dropped_images(
    mut drops: MessageReader<bevy::window::FileDragAndDrop>,
    mut s: ResMut<PromptSession>,
    mut sel: ResMut<Selection>,
    terrain: Res<crate::terrain::Terrain>,
    mut ed: ResMut<crate::editor::Editor>,
    dungeon_tool: Option<Res<crate::dungeons::ui::DungeonTool>>,
) {
    let describing = dungeon_tool.is_some_and(|t| t.describe_takes_images())
        && ed.tab == crate::editor::Tab::Dungeons;
    for d in drops.read() {
        let bevy::window::FileDragAndDrop::DroppedFile { path_buf, .. } = d else {
            continue;
        };
        // (Images takes its own drops: crate::images::view::dropped_files)
        if describing || ed.tab == crate::editor::Tab::Images {
            continue;
        }
        let ext = path_buf
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if !panel::IMAGE_EXTS.contains(&ext.as_str()) || s.running() {
            continue;
        }
        if s.images.len() >= 4 {
            ed.toast("Up to 4 reference images");
            continue;
        }
        if !s.images.contains(path_buf) {
            s.images.push(path_buf.clone());
        }
        if sel.polygon().is_none() {
            panel::select_whole_world(&mut sel, &terrain);
            ed.toast("Reference image added: the whole world is selected; describe what to build from it");
        } else {
            ed.toast("Reference image added to the prompt");
        }
        s.panel_open = true;
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn area_agent_gets_build_tools_but_not_world_wide_ones() {
        for t in [
            "fields_add",
            "fields_set",
            "fields_list",
            "roads_create",
            "caves_create",
            "scenes_place",
            "objects_place_set_piece",
            "nature_remove",
            "water_add_waterfall",
            "gameplay_add_poi",
            "world_info",
            "selection_info",
        ] {
            assert!(tools::area_agent_allows(t), "{t} withheld");
        }
        for t in [
            "world_save",
            "world_load",
            "undo",
            "batch",
            "zones_create",
            "camera_set",
            "lighting_set",
            "water_set_ocean",
            "dungeons_create",
            "settings_open",
        ] {
            assert!(!tools::area_agent_allows(t), "{t} offered");
        }
    }

    #[test]
    fn coordinates_are_found_under_their_keys_only() {
        let v = json!({
            "crop": "tulips",
            "polygon": [[1, 2], [3, 4], [5, 6]],
            "holes": [[[7, 8], [9, 10], [11, 12]]],
            "scale": [1, 1, 1],
            "items": [{ "position": [13, 14, 15], "scale": [2, 2] }],
            "area": { "center": [16, 17], "radius": 5 }
        });
        let mut pts = vec![];
        coords(&v, false, &mut pts);
        let mut xs: Vec<f32> = pts.iter().map(|p| p.x).collect();
        xs.sort_by(f32::total_cmp);
        assert_eq!(xs, vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 16.0]);
        assert!(pts.contains(&Vec2::new(13.0, 15.0)));
    }
}
