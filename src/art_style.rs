//! The world's art style: **Realistic** (CC0 photoscans) or **Hand-painted** (the Hand-painted
//! Fantasy pack). It is part of the world (`WorldMeta::art_style`, saved in world.json, Realistic
//! by default) and decides two things:
//!
//! - **What the engine paints with.** Every texture under `assets/textures/` has a painted twin
//!   under `assets/textures/handpainted/` with the same layout (`tools/stylepack/
//!   process_hp_surfaces.py`): the terrain layers and the terrain library (paint, roads), the
//!   object materials, tree bark and leaf cards, cactus skin and the ore surfaces. In a
//!   hand-painted world each loader takes the twin (`texture`), falling back to the realistic
//!   file wherever no painted one exists (a few modern materials: aluminium, plywood, denim…).
//!   Switching swaps the textures of the existing materials and arrays; no shader binding
//!   changes. The grass takes stylised colours (`grass::update_params`), and an untouched Natural
//!   look gets a painted baseline (`painted_look`) in the same undo step.
//! - **What the builder offers.** Objects, Nature and `objects_library` list only the world's
//!   style (`fits`): photoscans in a realistic world, the hand-painted pack in a painted one.
//!   Imported models and the procedural plants whose textures follow the style (ez-tree trees and
//!   bushes, palms, cacti, reeds, lily pads: `follows_style`) count as every style. "Show other
//!   styles" (Objects tab; `objects_library` style "all") mixes them for one session.
//!
//! **Nature in a hand-painted world** (`nature::Rules::build`): biome vegetation keeps the
//! procedural trees and bushes (painted bark and leaves), palms, cacti, reeds, lily pads,
//! dedicated painted wildflowers, mushrooms and berry plants, and the grass; it leaves out every
//! photoscanned rock, boulder, cliff, shrub, fern, grass tuft and piece of dead wood. A painted
//! forest is painted trees and bushes over painted forest floor and grass, rock shows as the
//! ground's own painted rock layer.
//!
//! The painted surfaces are part of the free, open-source studio (they ship with the app): any
//! world can be hand-painted. Only the hand-painted models are sold (`crate::market`).
//!
//! MCP: `world_set_art_style` (one undo step); `world_info` reports `art_style`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::history::History;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::objects::{Category, LibraryEntry, PHOTOREAL};
use crate::procgen::meshes::AssetKind;
use crate::world::WorldMeta;

/// The world's art style. Serialised with the asset style ids (`objects::STYLES`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArtStyle {
    /// CC0 photoscans
    #[default]
    #[serde(rename = "photoreal", alias = "realistic")]
    Realistic,
    /// the studio's free painted surfaces
    #[serde(rename = "handpainted", alias = "hand_painted", alias = "hand-painted")]
    HandPainted,
}

impl ArtStyle {
    pub const ALL: [ArtStyle; 2] = [ArtStyle::Realistic, ArtStyle::HandPainted];

    /// The asset style id (`objects::STYLES`): "photoreal" or "handpainted".
    pub fn id(self) -> &'static str {
        match self {
            ArtStyle::Realistic => PHOTOREAL,
            ArtStyle::HandPainted => "handpainted",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ArtStyle::Realistic => "Realistic",
            ArtStyle::HandPainted => "Hand-painted",
        }
    }

    /// A style from an id or a common way of naming it.
    pub fn parse(s: &str) -> Option<ArtStyle> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_")
            .as_str()
        {
            "photoreal" | "realistic" | "real" | "photo" | "photoscan" | "photoscans" | "scans" => {
                Some(ArtStyle::Realistic)
            }
            "handpainted" | "hand_painted" | "hp" | "painted" | "stylized" | "stylised"
            | "fantasy" => Some(ArtStyle::HandPainted),
            _ => None,
        }
    }

    pub fn painted(self) -> bool {
        self == ArtStyle::HandPainted
    }

    /// For people: what the style means for the world.
    pub fn blurb(self) -> &'static str {
        match self {
            ArtStyle::Realistic => {
                "Photoscanned ground, rock, bark and materials; the builder offers photoreal assets."
            }
            ArtStyle::HandPainted => {
                "Painted ground, rock, bark, leaves and materials; the builder offers hand-painted assets."
            }
        }
    }
}

/// The open world's style, for code without the ECS (loader threads, library builders).
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> ArtStyle {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        ArtStyle::HandPainted
    } else {
        ArtStyle::Realistic
    }
}

fn set_current(a: ArtStyle) {
    CURRENT.store(a.painted() as u8, Ordering::Relaxed);
}

/// The style the engine paints with: the world's (the painted surfaces are free for every
/// world).
pub fn surfaces(a: ArtStyle) -> ArtStyle {
    a
}

fn textures_root() -> PathBuf {
    PathBuf::from(crate::assets_path()).join("textures")
}

/// The painted twin of a texture under `assets/textures/` (whether or not it exists).
pub fn painted_twin(path: &Path) -> Option<PathBuf> {
    let root = textures_root();
    let rest = path.strip_prefix(&root).ok()?;
    (!rest.starts_with("handpainted")).then(|| root.join("handpainted").join(rest))
}

/// The file to load for the texture `path` (under `assets/textures/`) in style `art`: its painted
/// twin in a hand-painted world when there is one (and the pack is usable), else `path` itself.
pub fn texture(path: &Path, art: ArtStyle) -> PathBuf {
    if surfaces(art).painted()
        && let Some(twin) = painted_twin(path)
        && twin.is_file()
    {
        return twin;
    }
    path.to_path_buf()
}

/// Like `texture` for a whole set of files that must come from the same style (an albedo and its
/// normal map): the painted twins only if every one of them exists.
pub fn textures<const N: usize>(paths: [PathBuf; N], art: ArtStyle) -> [PathBuf; N] {
    if surfaces(art).painted() {
        let twins: Vec<PathBuf> = paths
            .iter()
            .filter_map(|p| painted_twin(p).filter(|t| t.is_file()))
            .collect();
        if twins.len() == N {
            return twins.try_into().unwrap_or(paths);
        }
    }
    paths
}

/// An assets-relative texture path ("textures/materials/x/albedo.png") in style `art`.
pub fn texture_rel(rel: &str, art: ArtStyle) -> String {
    if surfaces(art).painted()
        && rel.starts_with("textures/")
        && !rel.starts_with("textures/handpainted/")
    {
        let painted = format!("textures/handpainted/{}", &rel["textures/".len()..]);
        if Path::new(&crate::assets_path()).join(&painted).is_file() {
            return painted;
        }
    }
    rel.to_string()
}

// ------------------------------------------------------------------ what the builder offers

/// Procedural plants whose textures follow the world's style (painted bark and leaf cards in a
/// hand-painted world): they belong to every style.
pub fn follows_style(kind: AssetKind) -> bool {
    use AssetKind::*;
    matches!(
        kind,
        Pine | Fir
            | Spruce
            | SnowyPine
            | Cedar
            | Oak
            | Birch
            | Maple
            | GoldenMaple
            | SakuraPink
            | SakuraWhite
            | Ginkgo
            | Sequoia
            | Willow
            | Palm
            | Bamboo
            | DeadTree
            | Bush
            | Reeds
            | Cactus
            | LilyPads
            | Flowers
            | Mushrooms
            | Lavender
            | WildStrawberry
            | Blueberry
            | Blackberry
    )
}

/// Whether a library entry belongs to the style `style` (an asset style id; empty: every
/// style). Imported models and the style-following plants fit every style.
pub fn fits(e: &LibraryEntry, style: &str) -> bool {
    style.is_empty()
        || e.category == Category::Imported
        || e.pack == style
        || e.kind.is_some_and(follows_style)
}

/// Whether nature of `kind` is part of a world of style `art` (biome vegetation, ecosystem
/// brushes, the Nature tab's lists): everything in a realistic world; in a hand-painted one only
/// the style-following plants (no photoscanned rocks, shrubs, ferns, grass tufts or dead wood).
/// A species painted on purpose with "Show other styles" still grows.
pub fn kind_grows(kind: AssetKind, art: ArtStyle) -> bool {
    !art.painted() || follows_style(kind)
}

// ------------------------------------------------------------------ changing it

const UNDO_KEY: &str = "art_style";

/// The look a hand-painted world gets when its look was untouched (the default Natural): the
/// Natural grade with the soft tone curve, richer colour, a touch of vibrance, softened textures
/// and a little painterly filter, so the painted surfaces read as paint.
pub fn painted_look() -> crate::look::LookParams {
    crate::look::LookParams {
        saturation: 1.18,
        vibrance: 0.15,
        texture_soften: 0.65,
        painterly: 0.15,
        tone: crate::look::ToneCurve::Soft,
        ..crate::look::LookPreset::Natural.params()
    }
}

/// Whether a look is the untouched default (Natural as designed, nothing edited or saved).
fn untouched(l: &crate::look::Look) -> bool {
    l.preset == crate::look::LookPreset::Natural && l.custom.is_none() && l.saved.is_none()
}

/// What a style change records: the style and the world's look (the switch may set the look's
/// painted baseline).
type Snap = (ArtStyle, crate::look::Look);

/// Sets the world's art style as one undo step. Returns false when nothing changed.
/// Switching to Hand-painted with an untouched look also gives it the painted baseline
/// (`painted_look`), and switching back takes that baseline off again (an edited or saved look
/// is left alone either way): part of the same undo step. Setting Hand-painted on a world that
/// already is gives an untouched look the baseline too (a look chosen after the switch).
pub fn set_style(
    meta: &mut WorldMeta,
    lighting: &mut crate::sky::Lighting,
    history: &mut History,
    art: ArtStyle,
) -> bool {
    let before: Snap = (meta.art_style, lighting.look.clone());
    if meta.art_style == art {
        if !(art.painted() && untouched(&lighting.look)) {
            return false;
        }
        lighting.look.custom = Some(painted_look());
        history.push_ext(
            "Painted look",
            UNDO_KEY,
            Arc::new(before),
            Arc::new((art, lighting.look.clone())),
        );
        meta.dirty = true;
        return true;
    }
    let look = &mut lighting.look;
    if art.painted() && untouched(look) {
        look.custom = Some(painted_look());
    } else if !art.painted()
        && look.preset == crate::look::LookPreset::Natural
        && look.saved.is_none()
        && look.custom == Some(painted_look())
    {
        look.custom = None;
    }
    history.push_ext(
        format!("Art style: {}", art.name()),
        UNDO_KEY,
        Arc::new(before),
        Arc::new((art, lighting.look.clone())),
    );
    meta.art_style = art;
    meta.dirty = true;
    // (`current()` follows at the start of the next frame: `sync_current`)
    true
}

/// Applies a style while creating a fresh world. Unlike [`set_style`], this does not add an undo
/// step: the old world has already been discarded. It keeps the same painted-look baseline rules
/// as a normal style switch and updates the process-wide style for loaders that run this frame.
pub fn initialize_new_world(
    meta: &mut WorldMeta,
    lighting: &mut crate::sky::Lighting,
    art: ArtStyle,
) {
    meta.art_style = art;
    let look = &mut lighting.look;
    if art.painted() && untouched(look) {
        look.custom = Some(painted_look());
    } else if !art.painted()
        && look.preset == crate::look::LookPreset::Natural
        && look.saved.is_none()
        && look.custom == Some(painted_look())
    {
        look.custom = None;
    }
    set_current(art);
}

pub struct ArtStylePlugin;

impl Plugin for ArtStylePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(First, sync_current)
            .add_systems(Update, (apply_undo, sync_place_style));
        app.mcp_tool(McpTool {
            name: "world_set_art_style",
            description: "Set the world's art style: `photoreal` (Realistic: CC0 photoscanned ground, rock, bark and materials) or `handpainted` (Hand-painted: the studio's free painted ground, rock, bark, leaves, materials and ore surfaces). \
The terrain, roads, built objects, trees and ore deposits repaint at once; objects_library, the Objects tab and the Nature tab then offer only that style's assets (imported models and the procedural trees, bushes, palms, cacti, reeds and lily pads fit both). \
In a hand-painted world biome nature uses painted trees, bushes, grass, wildflowers, lavender, mushrooms and berry plants; photographic ferns, grass tufts, rocks and dead wood are excluded. \
An untouched Natural look also gets a painted baseline with the switch (soft tone curve, richer colour, softened textures, a touch of painterly; switching back takes it off); an edited or saved look is left alone. One undo step. Town, dungeon and encounter tools default to this style.",
            input_schema: || json!({ "type": "object", "properties": { "style": { "type": "string", "enum": ["photoreal", "handpainted"], "description": "photoreal (Realistic) or handpainted (Hand-painted)" } }, "required": ["style"] }),
            run: mcp_set,
        });
    }
}

/// Keeps `current()` in step with the open world (loads, new worlds, undo).
fn sync_current(meta: Res<WorldMeta>) {
    if meta.art_style != current() {
        set_current(meta.art_style);
    }
}

/// The Objects tab shows the world's style: its filter follows the world (a new style resets
/// "Show other styles"); picking another style anywhere (a pack card, the Gallery) turns "Show
/// other styles" on.
pub fn sync_place_style(meta: Res<WorldMeta>, mut place: ResMut<crate::objects::PlaceSettings>) {
    let w = meta.art_style.id();
    if place.world_style != w {
        place.world_style = w.to_string();
        place.mix_styles = false;
        if place.style != w {
            place.style = w.to_string();
            place.group.clear();
        }
    } else if !place.mix_styles && place.style != w {
        place.mix_styles = true;
    }
}

/// Undo / redo of a style change.
fn apply_undo(
    mut history: ResMut<History>,
    mut meta: ResMut<WorldMeta>,
    mut lighting: ResMut<crate::sky::Lighting>,
) {
    if !history.ext_apply.iter().any(|(k, _)| *k == UNDO_KEY) {
        return;
    }
    let mut keep = vec![];
    for (key, snap) in std::mem::take(&mut history.ext_apply) {
        if key != UNDO_KEY {
            keep.push((key, snap));
            continue;
        }
        if let Some((a, look)) = snap.downcast_ref::<Snap>() {
            meta.art_style = *a;
            meta.dirty = true;
            set_current(*a);
            if lighting.look != *look {
                lighting.look = look.clone();
            }
        }
    }
    history.ext_apply = keep;
}

fn mcp_set(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let s = args
        .get("style")
        .and_then(|v| v.as_str())
        .ok_or("missing argument `style` (photoreal or handpainted)")?;
    let art = ArtStyle::parse(s)
        .ok_or_else(|| format!("unknown style `{s}` (photoreal or handpainted)"))?;
    let was = world.resource::<WorldMeta>().art_style;
    let changed = world.resource_scope(|world, mut history: Mut<History>| {
        world.resource_scope(|world, mut lighting: Mut<crate::sky::Lighting>| {
            let mut meta = world.resource_mut::<WorldMeta>();
            set_style(&mut meta, &mut lighting, &mut history, art)
        })
    });
    let mut note = format!("The world's art style is {} ({}).", art.name(), art.id());
    if !changed {
        note.push_str(" (It already was.)");
    } else if was == art {
        note.push_str(
            " It already was; its untouched Natural look got the painted baseline (one undo step).",
        );
    } else {
        note.push_str(" The terrain, roads, objects, trees and ores repaint over the next moments; the builder now offers this style's assets.");
    }
    Ok(vec![McpContent::Json(
        json!({ "art_style": art.id(), "name": art.name(), "changed": changed, "note": note }),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_default() {
        assert_eq!(ArtStyle::default(), ArtStyle::Realistic);
        for a in ArtStyle::ALL {
            assert_eq!(ArtStyle::parse(a.id()), Some(a));
            let j = serde_json::to_string(&a).unwrap();
            assert_eq!(j, format!("\"{}\"", a.id()));
            assert_eq!(serde_json::from_str::<ArtStyle>(&j).unwrap(), a);
        }
        assert_eq!(ArtStyle::parse("hand_painted"), Some(ArtStyle::HandPainted));
        assert_eq!(ArtStyle::parse("Realistic"), Some(ArtStyle::Realistic));
        assert_eq!(
            serde_json::from_str::<ArtStyle>("\"realistic\"").unwrap(),
            ArtStyle::Realistic
        );
        assert!(ArtStyle::parse("gothic").is_none());
    }

    #[test]
    fn painted_twins_fall_back() {
        let root = textures_root();
        let grass = root.join("terrain/grass_albedo.png");
        assert_eq!(
            painted_twin(&grass),
            Some(root.join("handpainted/terrain/grass_albedo.png"))
        );
        assert_eq!(texture(&grass, ArtStyle::Realistic), grass);
        // a material that was never painted keeps its realistic file
        let alu = root.join("materials/metal_aluminium/albedo.png");
        assert_eq!(texture(&alu, ArtStyle::HandPainted), alu);
        assert_eq!(
            texture_rel(
                "textures/materials/metal_aluminium/albedo.png",
                ArtStyle::HandPainted
            ),
            "textures/materials/metal_aluminium/albedo.png"
        );
        // a painted twin is never twinned again
        assert!(painted_twin(&root.join("handpainted/terrain/grass_albedo.png")).is_none());
    }

    #[test]
    fn the_painted_look_baseline_goes_with_the_switch() {
        use crate::look::{Look, LookParams};
        let (mut meta, mut light, mut hist) = (
            WorldMeta::default(),
            crate::sky::Lighting::default(),
            History::default(),
        );
        light.look = Look::default();
        assert!(set_style(
            &mut meta,
            &mut light,
            &mut hist,
            ArtStyle::HandPainted
        ));
        assert_eq!(light.look.custom, Some(painted_look()));
        assert_eq!(
            hist.undo_label().as_deref(),
            Some("Art style: Hand-painted")
        );
        // switching back takes the baseline off
        assert!(set_style(
            &mut meta,
            &mut light,
            &mut hist,
            ArtStyle::Realistic
        ));
        assert_eq!(light.look, Look::default());
        // an edited look is left alone
        light.look.custom = Some(LookParams {
            saturation: 0.5,
            ..Default::default()
        });
        assert!(set_style(
            &mut meta,
            &mut light,
            &mut hist,
            ArtStyle::HandPainted
        ));
        assert_eq!(light.look.custom.map(|c| c.saturation), Some(0.5));
        // a hand-painted world whose look was reset to Natural gets the baseline on request, once
        light.look = Look::default();
        assert!(set_style(
            &mut meta,
            &mut light,
            &mut hist,
            ArtStyle::HandPainted
        ));
        assert_eq!(light.look.custom, Some(painted_look()));
        assert!(!set_style(
            &mut meta,
            &mut light,
            &mut hist,
            ArtStyle::HandPainted
        ));
        let p = painted_look();
        assert!(
            p.saturation >= 1.15
                && p.saturation <= 1.2
                && p.vibrance > 0.0
                && (0.5..=0.8).contains(&p.texture_soften)
                && (p.painterly - 0.15).abs() < 1e-6
        );
        assert_eq!(p.tone, crate::look::ToneCurve::Soft);
    }

    #[test]
    fn nature_rule() {
        // painted worlds keep the style-following plants and drop the photoscans
        for k in [
            AssetKind::Oak,
            AssetKind::Pine,
            AssetKind::Bush,
            AssetKind::Palm,
            AssetKind::Reeds,
        ] {
            assert!(kind_grows(k, ArtStyle::HandPainted), "{k:?}");
        }
        for k in [
            AssetKind::Boulder,
            AssetKind::CliffRock,
            AssetKind::Fern,
            AssetKind::TallGrass,
            AssetKind::FallenLog,
            AssetKind::DesertShrub,
            AssetKind::Pebbles,
        ] {
            assert!(!kind_grows(k, ArtStyle::HandPainted), "{k:?}");
            assert!(kind_grows(k, ArtStyle::Realistic), "{k:?}");
        }
    }
}
