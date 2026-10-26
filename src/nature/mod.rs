//! Procedural nature (PCG): trees, rocks, plants, shrubs, debris and pebbles are not stored
//! objects but rules plus painted density maps, placed deterministically per 32 m cell wherever
//! something needs them — like Unreal's PCG / procedural foliage or Horizon's GPU placement.
//!
//! - `rules`: which vegetation preset grows where (biomes, painted presets, the land itself).
//! - `field`: the painted density maps (sparse 512 m chunks on an 8 m grid) and hand removals,
//!   saved with the world as `nature.bin`.
//! - `place`: the deterministic placement of one cell, region or the whole world.
//! - `render`: render entities for the instances near the viewer (GPU-culled, LODs on the GPU,
//!   level-of-detail distances that shrink where nature grows denser), thinned billboard chunks
//!   out to the draw distance; `canopy`: the forest canopy beyond it (zoomed-out views).
//! - `brush`: painting density (grow, thin, erase per category, species or ecosystem), picking,
//!   removing and pinning single instances. The MCP tools are in `mcp/tools/nature.rs`.
//! - `migrate`: old worlds' tree, rock, plant and debris objects become density on load.
//! - `export`: density maps + rules for engines (instances are baked by the exporters).
//!
//! Stored objects remain only for what someone placed explicitly, plus pinned instances.

pub mod alien;
pub mod appearance;
pub mod brush;
pub mod canopy;
pub mod export;
pub mod field;
pub mod migrate;
pub mod place;
pub mod render;
pub mod rules;
pub mod shade;

use std::sync::OnceLock;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

pub use appearance::{ForestDiversity, ForestLayout, GroundCover, NatureAppearance};
pub use field::NatureField;
pub use place::{Ctx, Instance, Masks};
pub use rules::Rules;

/// History key for world-level forest and grass appearance controls.
pub const APPEARANCE_UNDO_KEY: &str = "nature_appearance";

use crate::objects::{Library, NatureAmounts, ObjectStore};
use crate::procgen::meshes::AssetKind;
use crate::terrain::Terrain;
use crate::water::WaterBodies;
use crate::world::{WorldMeta, Zones};

/// What `world.json` records about the procedural nature (`io.rs`).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct NatureDoc {
    /// 0: an old world whose nature is objects (converted on load); 1: procedural
    #[serde(default)]
    pub version: u32,
    /// the density maps (`nature.bin`), when anything is painted or removed
    #[serde(default)]
    pub file: Option<String>,
    /// Global look controls live with procedural nature even when no `nature.bin` exists.
    #[serde(default)]
    pub appearance: NatureAppearance,
}

/// What a save records: the `world.json` entry and the bytes of `nature.bin` (none when nothing
/// is painted). Encoding is a copy of the painted chunks (a few ms); it is cached per revision,
/// so autosaves of a world whose nature didn't change cost nothing.
static SNAPSHOT: std::sync::Mutex<Option<(u64, std::sync::Arc<Vec<u8>>)>> =
    std::sync::Mutex::new(None);

/// Forget the cached `nature.bin` (another world was opened: its revisions start over).
pub fn forget_snapshot() {
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn snapshot(
    field: &NatureField,
    appearance: NatureAppearance,
) -> (NatureDoc, Option<std::sync::Arc<Vec<u8>>>) {
    let appearance = appearance.sanitized();
    static CACHE: &std::sync::Mutex<Option<(u64, std::sync::Arc<Vec<u8>>)>> = &SNAPSHOT;
    if field.chunks.is_empty() && field.excluded.is_empty() && !field.fallback && !field.rules_off {
        return (
            NatureDoc {
                version: 1,
                file: None,
                appearance,
            },
            None,
        );
    }
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = match cache.as_ref() {
        Some((rev, b)) if *rev == field.revision => b.clone(),
        _ => {
            let b = std::sync::Arc::new(field.encode());
            *cache = Some((field.revision, b.clone()));
            b
        }
    };
    (
        NatureDoc {
            version: 1,
            file: Some("nature.bin".into()),
            appearance,
        },
        Some(bytes),
    )
}

/// Reads a world's density maps (an old or unpainted world: none).
pub fn load(dir: &std::path::Path, doc: &NatureDoc) -> Result<NatureField, String> {
    let mut f = match &doc.file {
        Some(name) => {
            NatureField::decode(&std::fs::read(dir.join(name)).map_err(|e| e.to_string())?)?
        }
        None => NatureField::default(),
    };
    f.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    Ok(f)
}

pub struct NaturePlugin;

impl Plugin for NaturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NatureField>()
            .init_resource::<NatureAppearance>()
            .init_resource::<Masks>()
            .init_resource::<render::Near>()
            .init_resource::<render::Billboards>()
            .init_resource::<canopy::Canopy>()
            .init_resource::<shade::CanopyShade>()
            .init_resource::<brush::NatureBrush>()
            .init_resource::<migrate::Pending>()
            .init_resource::<Watch>()
            .init_resource::<Seed>()
            .add_systems(PreStartup, shade::init_buffer)
            .add_systems(
                Update,
                (init_rules, apply_undo, migrate::run_pending).chain(),
            )
            .add_systems(Update, alien::follow)
            .add_systems(
                Update,
                brush::paint_tool.in_set(crate::tools::ToolSystems::Apply),
            )
            .add_systems(
                Update,
                brush::pick_tool.after(crate::tools::ToolSystems::Apply),
            )
            .add_systems(
                PostUpdate,
                (
                    update_masks,
                    watch_world,
                    render::update_near,
                    render::update_billboards,
                    canopy::update_canopy,
                    shade::update_canopy_shade,
                    clear_dirty,
                )
                    .chain()
                    .before(crate::objects::ObjectSync),
            );
    }
}

/// Asset kinds by their discriminant (`AssetKind as u8`).
pub fn kind_from_u8(v: u8) -> Option<AssetKind> {
    static T: OnceLock<Vec<Option<AssetKind>>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![None; 256];
        for k in AssetKind::ALL {
            t[*k as usize] = Some(*k);
        }
        t
    })[v as usize]
}

/// Everything placement needs, as system parameters.
#[derive(SystemParam)]
pub struct NatureParams<'w> {
    pub terrain: Res<'w, Terrain>,
    pub water: Res<'w, WaterBodies>,
    pub zones: Res<'w, Zones>,
    pub lib: Option<Res<'w, Library>>,
    pub field: Res<'w, NatureField>,
    pub rules: Option<Res<'w, Rules>>,
    pub masks: Res<'w, Masks>,
    pub amounts: Res<'w, NatureAmounts>,
    pub appearance: Res<'w, NatureAppearance>,
    pub seed: Res<'w, Seed>,
}

/// The world seed as placement sees it (kept from `WorldMeta` by `watch_world`, so systems that
/// change the meta can still read nature).
#[derive(Resource, Default)]
pub struct Seed(pub u32);

impl NatureParams<'_> {
    pub fn ctx(&self) -> Option<Ctx<'_>> {
        Some(Ctx {
            terrain: &self.terrain,
            water: &self.water,
            zones: &self.zones,
            lib: self.lib.as_deref()?,
            field: &self.field,
            rules: self.rules.as_deref()?,
            masks: &self.masks,
            amounts: amounts(&self.amounts),
            flora_amounts: self.amounts.flora_amounts(),
            appearance: *self.appearance,
            seed: self.seed.0,
        })
    }
}

/// The per-category world amounts (trees, rocks, plants, debris). Flora categories use their
/// own family amounts during placement, so the legacy Plants category stays neutral here.
pub fn amounts(a: &NatureAmounts) -> [f32; 4] {
    [a.trees, a.rocks, 1.0, a.debris]
}

/// The world amounts captured with a nature undo step. Grass has its own renderer and remains
/// outside procedural nature history.
fn history_amounts(a: &NatureAmounts) -> [f32; 7] {
    [
        a.trees,
        a.rocks,
        a.plants,
        a.debris,
        a.flowers,
        a.mushrooms,
        a.berries,
    ]
}

/// A placement context from the world's resources (MCP tools, exporters, map, analyses).
pub fn world_ctx(world: &World) -> Option<Ctx<'_>> {
    Some(Ctx {
        terrain: world.get_resource::<Terrain>()?,
        water: world.get_resource::<WaterBodies>()?,
        zones: world.get_resource::<Zones>()?,
        lib: world.get_resource::<Library>()?,
        field: world.get_resource::<NatureField>()?,
        rules: world.get_resource::<Rules>()?,
        masks: world.get_resource::<Masks>()?,
        amounts: amounts(world.get_resource::<NatureAmounts>()?),
        flora_amounts: world.get_resource::<NatureAmounts>()?.flora_amounts(),
        appearance: *world.get_resource::<NatureAppearance>()?,
        seed: world.get_resource::<WorldMeta>()?.seed,
    })
}

/// A placement context from `WorldData` (the editor's world systems and tools).
pub fn data_ctx<'a>(d: &'a crate::io::WorldData) -> Option<Ctx<'a>> {
    Some(Ctx {
        terrain: &d.terrain,
        water: &d.water,
        zones: &d.zones,
        lib: d.lib.as_deref()?,
        field: &d.nature,
        rules: d.nature_rules.as_deref()?,
        masks: &d.nature_masks,
        amounts: amounts(&d.amounts),
        flora_amounts: d.amounts.flora_amounts(),
        appearance: *d.appearance,
        seed: d.meta.seed,
    })
}

/// Every placed object plus every procedural instance of the given categories (`rules::CATS`
/// indices) in the world, as placed objects (exports and analyses that read objects).
/// Procedural ids have `place::PROC_BIT` set.
pub fn all_placed(
    store: &ObjectStore,
    ctx: Option<&Ctx>,
    cats: &[usize],
) -> Vec<crate::objects::Placed> {
    let mut out: Vec<crate::objects::Placed> = store.items.values().cloned().collect();
    if let Some(ctx) = ctx {
        out.extend(place::place_world(ctx, cats).iter().map(Instance::placed));
    }
    out
}

/// Auto-populate: land without a biome grows nature too, painted edits and hand removals are
/// dropped and the dials go back to 100% (one undo step).
pub fn populate(
    field: &mut NatureField,
    amounts: &mut NatureAmounts,
    history: &mut crate::history::History,
) {
    let before = whole_snapshot(field, Some(amounts));
    field.fallback = true;
    field.rules_off = false;
    field.chunks.clear();
    field.excluded.clear();
    amounts.trees = 1.0;
    amounts.rocks = 1.0;
    amounts.plants = 1.0;
    amounts.flowers = 1.0;
    amounts.mushrooms = 1.0;
    amounts.berries = 1.0;
    amounts.debris = 1.0;
    field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    history.push_ext(
        "Auto-populate nature",
        brush::UNDO_KEY,
        std::sync::Arc::new(before),
        std::sync::Arc::new(whole_snapshot(field, Some(amounts))),
    );
}

/// Clear nature: the procedural dials go to zero (one undo step).
pub fn clear(
    field: &mut NatureField,
    amounts: &mut NatureAmounts,
    history: &mut crate::history::History,
) {
    let before = whole_snapshot(field, Some(amounts));
    amounts.trees = 0.0;
    amounts.rocks = 0.0;
    amounts.plants = 0.0;
    amounts.flowers = 0.0;
    amounts.mushrooms = 0.0;
    amounts.berries = 0.0;
    amounts.debris = 0.0;
    field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    history.push_ext(
        "Clear nature",
        brush::UNDO_KEY,
        std::sync::Arc::new(before),
        std::sync::Arc::new(whole_snapshot(field, Some(amounts))),
    );
}

fn whole_snapshot(field: &NatureField, amounts: Option<&NatureAmounts>) -> field::NatureUndo {
    let keys: Vec<(i32, i32)> = field.chunks.keys().copied().collect();
    let mut u = field.snapshot(&keys);
    u.whole = true;
    u.excluded = Some(field.excluded.clone());
    u.fallback = Some(field.fallback);
    u.rules_off = Some(field.rules_off);
    u.amounts = amounts.map(history_amounts);
    u
}

/// A world rebuilt from scratch (from a picture): the old painted nature goes, `texel` gives
/// each 8 m texel's per-category multipliers and painted preset (index + 1; 0 = the biome's own),
/// or `None` for what the rules say. Without `rules_on` nothing grows (no vegetation wanted).
/// Part of the open undo step.
pub fn repaint_world(
    field: &mut NatureField,
    history: &mut crate::history::History,
    label: &str,
    half: f32,
    rules_on: bool,
    texel: impl Fn(Vec2) -> Option<([f32; 4], u8)> + Sync,
) {
    let before = whole_snapshot(field, None);
    field.chunks.clear();
    field.excluded.clear();
    field.rules_off = !rules_on;
    if rules_on {
        field.set_texels(Vec2::splat(-half), Vec2::splat(half), texel);
    }
    field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    history.push_ext(
        label.to_string(),
        brush::UNDO_KEY,
        std::sync::Arc::new(before),
        std::sync::Arc::new(whole_snapshot(field, None)),
    );
}

/// The land was replaced (Generate): painted nature edits and hand removals belonged to the old
/// landscape and go; the rules grow nature on the new one. Part of the open undo step.
pub fn reset_paint(field: &mut NatureField, history: &mut crate::history::History, label: &str) {
    if field.chunks.is_empty() && field.excluded.is_empty() && !field.rules_off {
        return;
    }
    let before = whole_snapshot(field, None);
    field.chunks.clear();
    field.excluded.clear();
    field.rules_off = false;
    field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    history.push_ext(
        label.to_string(),
        brush::UNDO_KEY,
        std::sync::Arc::new(before),
        std::sync::Arc::new(whole_snapshot(field, None)),
    );
}

/// What an area paint does (`paint_region`).
#[derive(Clone, Copy, Debug)]
pub enum AreaOp {
    /// grow the target at this density (1 = natural)
    Add(f32),
    /// keep this share (0..1) of what grows there (0 = remove it)
    Keep(f32),
}

/// Paints nature over an area at once (MCP tools, the area prompt, landforms that clear their
/// ground): a preset, species or categories grow at a density, or what grows there thins out.
/// An undo step of its own, or part of the open one.
#[allow(clippy::too_many_arguments)]
pub fn paint_region(
    field: &mut NatureField,
    history: &mut crate::history::History,
    label: &str,
    lo: Vec2,
    hi: Vec2,
    inside: impl Fn(Vec2) -> bool,
    target: &field::Target,
    op: AreaOp,
) {
    use field::{ADD_MAX, PaintMode, TEXEL, Target};
    let cats: Vec<usize> = match target {
        Target::Category(c) => vec![*c],
        Target::Species(ks) => {
            let mut v: Vec<usize> = ks
                .iter()
                .filter_map(|k| rules::cat_index(crate::objects::kind_category(*k)))
                .collect();
            v.dedup();
            v
        }
        _ => (0..4).collect(),
    };
    let keys = NatureField::chunks_in((lo + hi) * 0.5, (hi - lo).max_element() * 0.5 + TEXEL);
    let before = field.snapshot(&keys);
    match (op, target) {
        (AreaOp::Add(density), Target::Preset(_)) => {
            field.paint_area(lo, hi, &inside, PaintMode::Grow, target, 1.0);
            field.set_area(lo, hi, &inside, &cats, density);
        }
        (AreaOp::Add(density), Target::Species(ks)) => {
            for k in ks {
                let ci = rules::cat_index(crate::objects::kind_category(*k)).unwrap_or(2);
                let per_m2 = rules::CATS[ci].base_density() / 1000.0 * density;
                let t0 = field::texel_of(lo.x, lo.y);
                let t1 = field::texel_of(hi.x, hi.y);
                for tz in t0.y..=t1.y {
                    for tx in t0.x..=t1.x {
                        let t = IVec2::new(tx, tz);
                        if inside((t.as_vec2() + 0.5) * TEXEL) {
                            field.set_add(t, *k, per_m2.min(ADD_MAX[ci]));
                        }
                    }
                }
            }
            // the species' categories come back where they were erased
            field.paint_area(lo, hi, &inside, PaintMode::Grow, target, 0.0);
            field.touch(Rect::from_corners(lo, hi));
        }
        (AreaOp::Add(density), _) => field.set_area(lo, hi, &inside, &cats, density),
        (AreaOp::Keep(keep), _) => field.scale_area(lo, hi, &inside, target, keep.clamp(0.0, 1.0)),
    }
    history.push_ext(
        label.to_string(),
        brush::UNDO_KEY,
        std::sync::Arc::new(before),
        std::sync::Arc::new(field.snapshot(&keys)),
    );
}

/// A biome was painted: painted nature edits inside the area are dropped so the biome's own
/// nature grows there, `density` × its natural amount (an undo step of its own, or part of the
/// open group).
pub fn regrow_area(
    field: &mut NatureField,
    history: &mut crate::history::History,
    min: Vec2,
    max: Vec2,
    inside: impl Fn(Vec2) -> bool,
    density: f32,
) {
    let scaled = (density - 1.0).abs() > 0.01;
    let area = Rect::from_corners(min, max);
    let keys: Vec<(i32, i32)> = if scaled {
        NatureField::chunks_in(
            area.center(),
            area.half_size().max_element() * std::f32::consts::SQRT_2,
        )
    } else {
        field.chunks.keys().copied().collect()
    };
    let keys: Vec<(i32, i32)> = keys
        .into_iter()
        .filter(|k| {
            let o = Vec2::new(k.0 as f32, k.1 as f32) * field::CHUNK as f32 * field::TEXEL;
            let r = Rect::from_corners(o, o + field::CHUNK as f32 * field::TEXEL);
            !r.intersect(area).is_empty()
        })
        .collect();
    if keys.is_empty() {
        return;
    }
    let before = field.snapshot(&keys);
    let mut changed = field.reset_where(min, max, &inside);
    if scaled {
        field.set_area(min, max, &inside, &[0, 1, 2, 3], density);
        changed = true;
    }
    if changed {
        history.push_ext(
            "Regrow nature",
            brush::UNDO_KEY,
            std::sync::Arc::new(before),
            std::sync::Arc::new(field.snapshot(&keys)),
        );
    }
}

/// The rule tables, once the library has been built, and again for another world art style
/// (a hand-painted world grows no photoscans): then nature is placed again everywhere.
pub(crate) fn init_rules(
    mut commands: Commands,
    lib: Option<Res<Library>>,
    rules: Option<Res<Rules>>,
    meta: Res<WorldMeta>,
    mut field: ResMut<NatureField>,
) {
    let Some(lib) = lib else { return };
    match rules {
        None => commands.insert_resource(Rules::build(&lib, meta.art_style)),
        Some(r) if r.art != meta.art_style => {
            commands.insert_resource(Rules::build(&lib, meta.art_style));
            field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
        }
        Some(_) => {}
    }
}

/// Undo / redo of nature paint (see `History::push_ext`).
fn apply_undo(
    mut history: ResMut<crate::history::History>,
    mut field: ResMut<NatureField>,
    mut amounts: ResMut<NatureAmounts>,
    mut appearance: ResMut<NatureAppearance>,
) {
    let appearance_changes: Vec<_> = history
        .ext_apply
        .iter()
        .filter(|(k, _)| *k == APPEARANCE_UNDO_KEY)
        .map(|(_, s)| s.clone())
        .collect();
    history.ext_apply.retain(|(k, _)| *k != APPEARANCE_UNDO_KEY);
    for snapshot in appearance_changes {
        if let Some(saved) = snapshot.downcast_ref::<NatureAppearance>() {
            *appearance = saved.sanitized();
        }
    }
    if !history.ext_apply.iter().any(|(k, _)| *k == brush::UNDO_KEY) {
        return;
    }
    let mine: Vec<_> = history
        .ext_apply
        .iter()
        .filter(|(k, _)| *k == brush::UNDO_KEY)
        .map(|(_, s)| s.clone())
        .collect();
    history.ext_apply.retain(|(k, _)| *k != brush::UNDO_KEY);
    for s in mine {
        if let Some(u) = s.downcast_ref::<field::NatureUndo>() {
            field.restore(u);
            if let Some(a) = u.amounts {
                [
                    amounts.trees,
                    amounts.rocks,
                    amounts.plants,
                    amounts.debris,
                    amounts.flowers,
                    amounts.mushrooms,
                    amounts.berries,
                ] = a;
            }
        }
    }
}

/// Update saved appearance as one undoable world-level edit.
pub fn set_appearance(
    appearance: &mut NatureAppearance,
    history: &mut crate::history::History,
    after: NatureAppearance,
) {
    let before = appearance.sanitized();
    let after = after.sanitized();
    if before == after {
        return;
    }
    *appearance = after;
    history.push_ext(
        "Set nature appearance",
        APPEARANCE_UNDO_KEY,
        std::sync::Arc::new(before),
        std::sync::Arc::new(after),
    );
}

/// Keeps the road and placed-object masks in step with the world.
#[allow(clippy::too_many_arguments)]
fn update_masks(
    mut masks: ResMut<Masks>,
    roads: Res<crate::roads::Roads>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    bounds: Res<crate::placement::ModelBounds>,
    stroke: Res<crate::tools::Stroke>,
    mut field: ResMut<NatureField>,
) {
    // only where something changed is nature placed again (many changes: everywhere)
    let mut changed = vec![];
    // (the signature over every road point is only worth taking when the roads were touched;
    // checked before anything can return early, or a change could be missed)
    if roads.is_changed() {
        let mut sig: u64 = roads.roads.len() as u64;
        for r in &roads.roads {
            sig = sig
                .wrapping_mul(31)
                .wrapping_add(r.width.to_bits() as u64 ^ (r.points.len() as u64) << 32);
            for p in &r.points {
                sig = sig
                    .wrapping_mul(31)
                    .wrapping_add((p[0].to_bits() as u64) << 32 | p[2].to_bits() as u64);
            }
        }
        if sig != masks.road_sig {
            masks.road_sig = sig;
            changed.extend(masks.set_roads(&roads));
        }
    }
    // placed objects: after each edit (not while dragging one around)
    if let Some(lib) = lib
        && masks.store_rev != Some(store.revision)
        && !stroke.active
    {
        masks.store_rev = Some(store.revision);
        changed.extend(masks.set_objects(&store, &lib, &bounds));
    }
    if changed.len() > 64 {
        field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    } else {
        for r in changed {
            field.touch(r);
        }
    }
}

/// What the placements were last evaluated against (anything else that changes them).
#[derive(Resource, Default, PartialEq, Clone)]
struct Watch {
    generation: u64,
    heights: u64,
    zones: u64,
    biomes: u64,
    water: u64,
    zone_rules: u64,
    amounts: [u32; 7],
    seed: u32,
    fallback: bool,
    forest: ForestDiversity,
}

/// Re-evaluates everything (diffed: only cells whose instances changed are rebuilt) when the
/// land, water, zones, world amounts or seed changed — once a stroke ends, not every dab.
#[allow(clippy::too_many_arguments)]
fn watch_world(
    mut watch: ResMut<Watch>,
    terrain: Res<Terrain>,
    water: Res<WaterBodies>,
    zones: Res<Zones>,
    amounts: Res<NatureAmounts>,
    appearance: Res<NatureAppearance>,
    meta: Res<WorldMeta>,
    stroke: Res<crate::tools::Stroke>,
    mut field: ResMut<NatureField>,
    mut seed: ResMut<Seed>,
) {
    if seed.0 != meta.seed {
        seed.0 = meta.seed;
    }
    if stroke.active {
        return;
    }
    let mut wsig: u64 = (water.rivers.len() as u64) << 32 | water.lakes.len() as u64;
    for l in &water.lakes {
        wsig = wsig.wrapping_mul(31).wrapping_add(
            l.level.to_bits() as u64 ^ (l.outline.len() as u64) << 32 ^ l.seed[0].to_bits() as u64,
        );
    }
    for r in &water.rivers {
        wsig = wsig.wrapping_mul(31).wrapping_add(r.points.len() as u64);
    }
    wsig = wsig
        .wrapping_mul(31)
        .wrapping_add(water.level_map.len() as u64 ^ (water.dirty as u64) << 40);
    let mut zsig: u64 = zones.list.len() as u64;
    for z in &zones.list {
        zsig = zsig.wrapping_mul(31).wrapping_add(
            z.id as u64
                | (z.limit_placement as u64) << 8
                | (z.biome_enabled as u64) << 9
                | (z.biome as u64) << 10
                | (z.forest
                    .map(|f| f.enabled as u64 | ((f.layout as u64) << 1))
                    .unwrap_or(4))
                    << 16,
        );
    }
    let now = Watch {
        generation: terrain.generation,
        heights: terrain.height_revision,
        zones: terrain.zone_revision,
        biomes: terrain.biome_revision,
        water: wsig,
        zone_rules: zsig,
        amounts: history_amounts(&amounts).map(f32::to_bits),
        seed: meta.seed,
        fallback: field.fallback,
        forest: appearance.forest,
    };
    if *watch != now {
        *watch = now;
        field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    }
}

/// The renderers have seen this frame's changes.
fn clear_dirty(mut field: ResMut<NatureField>) {
    if !field.dirty.is_empty() {
        field.dirty.clear();
    }
}
