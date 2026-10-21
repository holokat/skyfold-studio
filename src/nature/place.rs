//! Deterministic placement: what grows in one 32 m cell, computed on demand from the rules, the
//! painted density maps, the land and the exclusion masks. The same cell always yields the same
//! instances (same ids, same transforms), so nothing needs storing: the renderer, colliders,
//! queries, exports and analyses all evaluate the cells they need.
//!
//! Each category has a fixed blue-noise pattern of candidate slots per cell (shifted and turned
//! per cell, so no tiling shows). A slot grows if a hash of it falls under the local density;
//! the species is then picked by the local layer weights. Slope, altitude, water, roads, placed
//! objects, zone rules and hand removals filter the result.

use std::collections::HashMap;
use std::sync::OnceLock;

use bevy::prelude::*;

use super::field::{NatureField, TEXEL, texel_of};
use super::rules::{CATS, ForestEcology, NONE, Rules};
use super::{ForestDiversity, NatureAppearance};
use crate::objects::{Category, Library, ObjectStore, Placed};
use crate::procgen::meshes::AssetKind;
use crate::terrain::Terrain;
use crate::water::WaterBodies;
use crate::world::Zones;

/// Placement cell size (m).
pub const CELL: f32 = 32.0;
const TPC: usize = (CELL / TEXEL) as usize;
/// Candidate slots per cell and category (the most that can ever grow there).
const SLOTS: [usize; 4] = [110, 40, 256, 48];

/// Bit set on every procedural instance id (placed objects count up from 1).
pub const PROC_BIT: u64 = 1 << 63;

pub fn is_procedural(id: u64) -> bool {
    id & PROC_BIT != 0
}

/// One procedural placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub id: u64,
    pub kind: AssetKind,
    /// library entry index
    pub entry: u32,
    pub variant: u8,
    /// category index (`rules::CATS`)
    pub cat: u8,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: f32,
}

impl Instance {
    /// As a placed object (pinning, exports, analyses that read objects).
    pub fn placed(&self) -> Placed {
        Placed {
            group: 0,
            id: self.id,
            asset: crate::objects::builtin_id(self.kind),
            variant: self.variant as u32,
            stretch: None,
            pos: self.pos.to_array(),
            rot: self.rot.to_array(),
            scale: self.scale,
            locked: false,
        }
    }
}

/// The cell an instance id belongs to.
pub fn cell_of_id(id: u64) -> IVec2 {
    let sx = ((id >> 40) & 0xF_FFFF) as i32;
    let sz = ((id >> 20) & 0xF_FFFF) as i32;
    // 20-bit two's complement
    IVec2::new((sx << 12) >> 12, (sz << 12) >> 12)
}

pub fn cell_of(p: Vec2) -> IVec2 {
    IVec2::new((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32)
}

/// Everything placement reads.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub terrain: &'a Terrain,
    pub water: &'a WaterBodies,
    pub zones: &'a Zones,
    pub lib: &'a Library,
    pub field: &'a NatureField,
    pub rules: &'a Rules,
    pub masks: &'a Masks,
    /// world-wide amount per category (Nature tab dials)
    pub amounts: [f32; 4],
    /// Plants, flowers, mushrooms, berries, independently thinned from stable candidates.
    pub flora_amounts: [f32; 4],
    pub appearance: NatureAppearance,
    pub seed: u32,
}

impl<'a> Ctx<'a> {
    /// Zone forest setting, or the world setting when the zone inherits it.
    pub fn forest_at(&self, p: Vec2) -> ForestDiversity {
        self.zones
            .by_id(self.terrain.zone_at(p.x, p.y))
            .and_then(|z| z.forest)
            .unwrap_or(self.appearance.forest)
    }

    /// Ecological guard shared by near slots and the far canopy. It prevents a legacy palm or
    /// cold-climate layer from gaining tropical bamboo or temperate blossoms merely because the
    /// optional diversity switch is on.
    pub fn forest_ecology_at(&self, p: Vec2) -> ForestEcology {
        use crate::biomes::BiomePreset;
        use crate::world::Biome;

        let zone = self.zones.by_id(self.terrain.zone_at(p.x, p.y));
        if let Some(biome) = zone.filter(|z| z.biome_enabled).map(|z| z.biome) {
            return match biome {
                Biome::Temperate => ForestEcology::Temperate,
                Biome::Autumn => ForestEcology::Autumn,
                Biome::Tropical
                    if self.terrain.height_at(p.x, p.y) - self.terrain.sea_level >= 8.0 =>
                {
                    ForestEcology::Jungle
                }
                Biome::Boreal | Biome::Alpine | Biome::Tundra => ForestEcology::Cold,
                Biome::Desert | Biome::Swamp | Biome::Volcanic | Biome::Tropical => {
                    ForestEcology::Excluded
                }
            };
        }
        match BiomePreset::from_id(self.terrain.biome_at(p.x, p.y)) {
            Some(
                BiomePreset::TemperateForest | BiomePreset::Meadow | BiomePreset::HauntedWoods,
            ) => ForestEcology::Temperate,
            Some(BiomePreset::AutumnWoods) => ForestEcology::Autumn,
            Some(BiomePreset::Jungle) => ForestEcology::Jungle,
            Some(BiomePreset::Taiga) => ForestEcology::Cold,
            Some(
                BiomePreset::TropicalCoast
                | BiomePreset::Desert
                | BiomePreset::Alpine
                | BiomePreset::Tundra
                | BiomePreset::Swamp
                | BiomePreset::Volcanic,
            ) => ForestEcology::Excluded,
            // The old no-biome fallback stays conservative near water and snow, and otherwise
            // behaves like the temperate legacy mixed-forest layer.
            None => {
                let altitude = self.terrain.height_at(p.x, p.y) - self.terrain.sea_level;
                if altitude < 5.0 {
                    ForestEcology::Excluded
                } else if altitude > self.terrain.style.snow_line * 0.4 {
                    ForestEcology::Cold
                } else {
                    ForestEcology::Temperate
                }
            }
        }
    }
}

/// Roads and placed objects nature keeps clear of, bucketed per cell.
#[derive(Resource, Default)]
pub struct Masks {
    roads: HashMap<IVec2, Vec<(Vec2, Vec2, f32)>>,
    objects: HashMap<IVec2, Vec<(Vec2, f32)>>,
    /// structures' footprints (rectangles: nothing grows under a floor or through a wall)
    boxes: HashMap<IVec2, Vec<crate::placement::Footprint>>,
    /// every road segment and object disc (to tell what an edit changed)
    segments: Vec<[u32; 5]>,
    /// (centre, radius, and a rectangle's heading and shape)
    discs: Vec<[u32; 4]>,
    pub(super) road_sig: u64,
    pub(super) store_rev: Option<u64>,
}

impl Masks {
    /// Rebuilds the road mask; returns the areas that changed.
    pub(super) fn set_roads(&mut self, roads: &crate::roads::Roads) -> Vec<Rect> {
        let mut segments = vec![];
        self.roads.clear();
        for r in &roads.roads {
            let pts = r.centreline(3.0);
            let hw = r.width * 0.5;
            for w in pts.windows(2) {
                let (a, b) = (Vec2::new(w[0].x, w[0].z), Vec2::new(w[1].x, w[1].z));
                segments.push([
                    a.x.to_bits(),
                    a.y.to_bits(),
                    b.x.to_bits(),
                    b.y.to_bits(),
                    hw.to_bits(),
                ]);
                let pad = hw + 4.0;
                let c0 = cell_of(a.min(b) - pad);
                let c1 = cell_of(a.max(b) + pad);
                for z in c0.y..=c1.y {
                    for x in c0.x..=c1.x {
                        self.roads
                            .entry(IVec2::new(x, z))
                            .or_default()
                            .push((a, b, hw));
                    }
                }
            }
        }
        segments.sort_unstable();
        let changed = changed(&self.segments, &segments, |s| {
            let (a, b, hw) = (
                Vec2::new(f32::from_bits(s[0]), f32::from_bits(s[1])),
                Vec2::new(f32::from_bits(s[2]), f32::from_bits(s[3])),
                f32::from_bits(s[4]),
            );
            Rect::from_corners(a.min(b) - hw - 4.0, a.max(b) + hw + 4.0)
        });
        self.segments = segments;
        changed
    }

    /// Rebuilds the placed-object mask; returns the areas that changed.
    pub(super) fn set_objects(
        &mut self,
        store: &ObjectStore,
        lib: &Library,
        bounds: &crate::placement::ModelBounds,
    ) -> Vec<Rect> {
        let mut discs = vec![];
        self.objects.clear();
        self.boxes.clear();
        for o in store.items.values() {
            let Some(e) = lib.get(&o.asset) else { continue };
            if matches!(e.category, Category::Structures | Category::Imported) {
                let f = crate::placement::Footprint::of(o, bounds.of(e));
                let r = f.half.length();
                discs.push([
                    f.center.x.to_bits(),
                    f.center.y.to_bits(),
                    r.to_bits(),
                    f.ax.x.to_bits() ^ f.half.x.to_bits().rotate_left(16),
                ]);
                let c0 = cell_of(f.center - r - 5.0);
                let c1 = cell_of(f.center + r + 5.0);
                for z in c0.y..=c1.y {
                    for x in c0.x..=c1.x {
                        self.boxes.entry(IVec2::new(x, z)).or_default().push(f);
                    }
                }
                continue;
            }
            // placed trees and rocks keep just their trunk / body clear
            let r = match e.category {
                Category::Props => e.radius * o.scale * 0.8,
                Category::Trees => 1.2 * o.scale,
                _ => e.radius * o.scale * 0.7,
            };
            let p = Vec2::new(o.pos[0], o.pos[2]);
            discs.push([p.x.to_bits(), p.y.to_bits(), r.to_bits(), 0]);
            let c0 = cell_of(p - r - 3.0);
            let c1 = cell_of(p + r + 3.0);
            for z in c0.y..=c1.y {
                for x in c0.x..=c1.x {
                    self.objects
                        .entry(IVec2::new(x, z))
                        .or_default()
                        .push((p, r));
                }
            }
        }
        discs.sort_unstable();
        let changed = changed(&self.discs, &discs, |d| {
            Rect::from_center_half_size(
                Vec2::new(f32::from_bits(d[0]), f32::from_bits(d[1])),
                Vec2::splat(f32::from_bits(d[2]) + 5.0),
            )
        });
        self.discs = discs;
        changed
    }

    fn blocked(&self, cell: IVec2, p: Vec2, r: f32) -> bool {
        if let Some(v) = self.roads.get(&cell)
            && v.iter().any(|(a, b, hw)| dist_seg(p, *a, *b) < hw + r)
        {
            return true;
        }
        // (trees keep their crowns off walls and roofs: a wider berth)
        self.boxes
            .get(&cell)
            .is_some_and(|v| v.iter().any(|f| f.sdist(p) < r * 2.0 + 0.4))
            || self
                .objects
                .get(&cell)
                .is_some_and(|v| v.iter().any(|(c, cr)| c.distance(p) < cr + r))
    }
}

/// The areas of the items in one sorted list but not the other.
fn changed<T: Ord + Copy>(old: &[T], new: &[T], area: impl Fn(&T) -> Rect) -> Vec<Rect> {
    let mut out = vec![];
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) if a == b => {
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if a < b => {
                out.push(area(a));
                i += 1;
            }
            (Some(a), None) => {
                out.push(area(a));
                i += 1;
            }
            (_, Some(b)) => {
                out.push(area(b));
                j += 1;
            }
            (None, None) => break,
        }
    }
    out
}

fn dist_seg(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

#[inline]
fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[inline]
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Blue-noise candidate slots on the cell torus, per category.
fn patterns() -> &'static [Vec<Vec2>; 4] {
    static P: OnceLock<[Vec<Vec2>; 4]> = OnceLock::new();
    P.get_or_init(|| {
        std::array::from_fn(|c| {
            let mut rng = crate::noise::Rng::new(0x51ab_0000 + c as u64);
            let mut pts: Vec<Vec2> = Vec::with_capacity(SLOTS[c]);
            for _ in 0..SLOTS[c] {
                let mut best = Vec2::ZERO;
                let mut best_d = -1.0;
                for _ in 0..16 {
                    let q = Vec2::new(rng.f32(), rng.f32()) * CELL;
                    let d = pts
                        .iter()
                        .map(|s| {
                            let dx = (s.x - q.x).abs().min(CELL - (s.x - q.x).abs());
                            let dz = (s.y - q.y).abs().min(CELL - (s.y - q.y).abs());
                            dx * dx + dz * dz
                        })
                        .fold(f32::MAX, f32::min);
                    if d > best_d {
                        best_d = d;
                        best = q;
                    }
                }
                pts.push(best);
            }
            pts
        })
    })
}

/// One texel's growth for one category: the preset, its weight and the added species.
#[derive(Clone, Copy, Default)]
struct TexelRule {
    preset: u8,
    /// density factor × multiplier × amount for the preset's layers
    k: f32,
    /// the preset part of the density (per m²)
    base: f32,
    /// total density including painted species (per m²)
    total: f32,
    /// classic (no-biome) rules: zone biomes filter species
    fallback: bool,
}

/// Instances of the given categories in one cell, appended to `out`.
pub fn place_cell(ctx: &Ctx, cell: IVec2, cats: &[usize], out: &mut Vec<Instance>) {
    let t = ctx.terrain;
    let origin = cell.as_vec2() * CELL;
    let half = t.half();
    if origin.x > half || origin.y > half || origin.x + CELL < -half || origin.y + CELL < -half {
        return;
    }
    // the rules on the cell's 4×4 texels (shared by the categories)
    let t0 = texel_of(origin.x + 1.0, origin.y + 1.0);
    let mut presets = [(NONE, 0.0f32, false); TPC * TPC];
    let mut mults = [[1.0f32; 4]; TPC * TPC];
    let mut any_species = false;
    for j in 0..TPC {
        for i in 0..TPC {
            let tx = t0 + IVec2::new(i as i32, j as i32);
            let p = (tx.as_vec2() + 0.5) * TEXEL;
            let tex = ctx.field.texel(tx);
            let (pi, f) =
                ctx.rules
                    .preset_at(t, ctx.zones, p, ctx.seed, tex.preset, ctx.field.fallback);
            let fallback = tex.preset == 0 && t.biome_at(p.x, p.y) == 0;
            presets[j * TPC + i] = (pi, f, fallback);
            mults[j * TPC + i] = tex.mult;
            any_species |= ctx.field.has_species(tx);
        }
    }
    // painted species (added, or thinned out of the rules' mix)
    let mut buf: Vec<(u8, f32)> = vec![];
    let mut species: Vec<Species> = vec![];
    if any_species {
        for j in 0..TPC {
            for i in 0..TPC {
                let t = t0 + IVec2::new(i as i32, j as i32);
                ctx.field.adds(t, &mut buf);
                let adds = buf.clone();
                ctx.field.species_mults(t, &mut buf);
                species.push(Species {
                    adds,
                    mults: buf.clone(),
                });
            }
        }
    }
    for &cat in cats {
        let mut rules = [TexelRule::default(); TPC * TPC];
        let mut any = false;
        for (k, r) in rules.iter_mut().enumerate() {
            let (pi, f, fallback) = presets[k];
            let base = match pi {
                NONE => 0.0,
                _ if any_species && !species[k].mults.is_empty() => ctx.rules.presets[pi as usize]
                    .cats[cat]
                    .iter()
                    .map(|l| l.density * species[k].mult(l.kind))
                    .sum(),
                _ => ctx.rules.presets[pi as usize].total[cat],
            };
            let kk = f * mults[k][cat] * ctx.amounts[cat];
            let extra: f32 = if any_species {
                species[k]
                    .adds
                    .iter()
                    .filter(|(kind, _)| kind_cat(*kind) == Some(cat))
                    .map(|(_, d)| d * ctx.amounts[cat].max(0.0))
                    .sum()
            } else {
                0.0
            };
            *r = TexelRule {
                preset: pi,
                k: kk,
                base: base * kk,
                total: base * kk + extra,
                fallback,
            };
            any |= r.total > 0.0;
        }
        if !any {
            continue;
        }
        place_cat(
            ctx,
            cell,
            cat,
            &rules,
            if any_species { Some(&species) } else { None },
            out,
        );
    }
}

/// Painted species at one texel.
struct Species {
    /// added density (kind, per m²)
    adds: Vec<(u8, f32)>,
    /// share of the rules' density kept (kind, 0..1)
    mults: Vec<(u8, f32)>,
}

impl Species {
    fn mult(&self, kind: AssetKind) -> f32 {
        self.mults
            .iter()
            .find(|(k, _)| *k == kind as u8)
            .map(|(_, m)| *m)
            .unwrap_or(1.0)
    }
}

fn kind_cat(kind: u8) -> Option<usize> {
    super::kind_from_u8(kind)
        .and_then(|k| super::rules::cat_index(crate::objects::kind_category(k)))
}

fn place_cat(
    ctx: &Ctx,
    cell: IVec2,
    cat: usize,
    rules: &[TexelRule; TPC * TPC],
    species: Option<&Vec<Species>>,
    out: &mut Vec<Instance>,
) {
    let t = ctx.terrain;
    let origin = cell.as_vec2() * CELL;
    let slot_density = SLOTS[cat] as f32 / (CELL * CELL);
    let h = mix((ctx.seed as u64) << 32
        ^ (cat as u64) << 56
        ^ (cell.x as u32 as u64) << 20
        ^ cell.y as u32 as u64
        ^ 0x6a09_e667_f3bc_c909);
    let shift = Vec2::new(unit(mix(h ^ 1)), unit(mix(h ^ 2))) * CELL;
    let turn = (h >> 3) & 3;
    let pat = &patterns()[cat];
    let id_base = PROC_BIT
        | (cat as u64) << 60
        | ((cell.x as u32 as u64) & 0xF_FFFF) << 40
        | ((cell.y as u32 as u64) & 0xF_FFFF) << 20;
    for (s, q) in pat.iter().enumerate() {
        let q = match turn {
            0 => *q,
            1 => Vec2::new(CELL - q.y, q.x),
            2 => Vec2::new(CELL - q.x, CELL - q.y),
            _ => Vec2::new(q.y, CELL - q.x),
        };
        let local = Vec2::new((q.x + shift.x) % CELL, (q.y + shift.y) % CELL);
        let ti = ((local.y / TEXEL) as usize).min(TPC - 1) * TPC
            + ((local.x / TEXEL) as usize).min(TPC - 1);
        let r = &rules[ti];
        if r.total <= 0.0 {
            continue;
        }
        let hs = mix(h ^ (s as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        // Reserve candidates at the maximum flora abundance. Family changes thin their
        // own slots, so increasing flowers never relocates the berry bushes.
        let reserve = if cat == 2 { 2.0 } else { 1.0 };
        if unit(hs) * slot_density >= r.total * reserve {
            continue;
        }
        let id = id_base | s as u64;
        if ctx.field.excluded.contains(&id) {
            continue;
        }
        // which species
        let mut pick = unit(mix(hs ^ 7)) * r.total;
        let from_rule = r.preset != NONE && pick < r.base;
        let mut chosen: Option<(AssetKind, usize, (f32, f32), f32, (f32, f32))> = None;
        if from_rule {
            for l in &ctx.rules.presets[r.preset as usize].cats[cat] {
                let d = l.density * r.k * species.map(|s| s[ti].mult(l.kind)).unwrap_or(1.0);
                if pick < d {
                    chosen = Some((l.kind, l.entry, l.scale, l.max_slope, l.alt));
                    break;
                }
                pick -= d;
            }
        } else if let Some(species) = species {
            pick -= r.base;
            for (kind, d) in &species[ti].adds {
                if kind_cat(*kind) != Some(cat) {
                    continue;
                }
                let d = d * ctx.amounts[cat];
                if pick < d {
                    let k = super::kind_from_u8(*kind).unwrap();
                    if let Some(e) = ctx.rules.kind_entry[k as usize] {
                        // (painted rocks hold on steep ground too)
                        let steep = if CATS[cat] == Category::Rocks {
                            55.0
                        } else {
                            38.0
                        };
                        chosen = Some((k, e, (0.8, 1.2), steep, (-1000.0, 10_000.0)));
                    }
                    break;
                }
                pick -= d;
            }
        }
        let Some((mut kind, mut entry, scale_range, max_slope, alt_range)) = chosen else {
            continue;
        };
        if let Some(group) = super::rules::flora_group(kind) {
            let amount = ctx.flora_amounts[group].clamp(0.0, 2.0);
            if unit(mix(hs ^ 0xf10a)) * 2.0 >= amount {
                continue;
            }
        }
        let pos2 = origin + local;
        // Only baseline rule layers diversify. Explicit painting and pinned objects preserve the
        // exact species selected by the editor.
        if cat == 0 && from_rule {
            let resolved = ctx.rules.resolve_tree(
                kind,
                pos2,
                ctx.seed,
                ctx.forest_at(pos2),
                ctx.forest_ecology_at(pos2),
            );
            if let Some(resolved_entry) = ctx.rules.kind_entry[resolved as usize] {
                kind = resolved;
                entry = resolved_entry;
            }
        }
        if !t.in_bounds(pos2.x, pos2.y) {
            continue;
        }
        let ground = t.height_at(pos2.x, pos2.y);
        let alt = ground - t.sea_level;
        if alt < alt_range.0 || alt > alt_range.1 {
            continue;
        }
        let wet_ok = matches!(kind, AssetKind::Reeds | AssetKind::LilyPads);
        if !wet_ok
            && (alt < 0.4
                || ctx
                    .water
                    .is_underwater(Vec3::new(pos2.x, ground, pos2.y), 0.2))
        {
            continue;
        }
        let normal = t.normal_at(pos2.x, pos2.y);
        let slope = normal.y.clamp(-1.0, 1.0).acos().to_degrees();
        // trees, plants and fallen wood keep off ground steeper than ~38°; rocks and scree lie on
        // steep slopes (a volcano's flanks, a mountainside) up to their own limit
        let limit = if CATS[cat] == Category::Rocks {
            max_slope
        } else {
            max_slope.min(38.0f32.max(max_slope * 0.5))
        };
        if slope > limit {
            continue;
        }
        let e = &ctx.lib.entries[entry];
        let zid = t.zone_at(pos2.x, pos2.y);
        if let Some(z) = ctx.zones.by_id(zid) {
            if z.limit_placement {
                continue;
            }
            if r.fallback && z.biome_enabled && !e.biomes.contains(&z.biome) {
                continue;
            }
        }
        let scale = scale_range.0 + (scale_range.1 - scale_range.0) * unit(mix(hs ^ 11));
        let clear = match CATS[cat] {
            Category::Trees => 1.5,
            Category::Plants => 0.25,
            _ => e.radius * scale * 0.5,
        };
        if ctx.masks.blocked(cell, pos2, clear) {
            continue;
        }
        let yaw = unit(mix(hs ^ 13)) * std::f32::consts::TAU;
        let rot = if e.align {
            Quat::from_rotation_arc(Vec3::Y, normal) * Quat::from_rotation_y(yaw)
        } else {
            let lx = (unit(mix(hs ^ 17)) - 0.5) * 0.08;
            let lz = (unit(mix(hs ^ 19)) - 0.5) * 0.08;
            Quat::from_rotation_x(lx) * Quat::from_rotation_z(lz) * Quat::from_rotation_y(yaw)
        };
        // the base reaches below the lowest ground under the trunk's footprint (plus a margin for
        // the terrain mesh's facets), so no gap opens under it on the downhill side of a slope
        let foot = (e.radius * 0.15).max(0.4) * scale;
        let under = ground - t.min_height_around(pos2.x, pos2.y, foot);
        let sink = if matches!(
            kind,
            AssetKind::Mushrooms
                | AssetKind::Lavender
                | AssetKind::WildStrawberry
                | AssetKind::Blueberry
                | AssetKind::Blackberry
                | AssetKind::Flowers
        ) {
            // These meshes have a true root origin. Tree seating buries their fruit, caps, or
            // blossoms, including the photoreal and painted flower variants.
            0.015 * scale
        } else {
            (0.15 * scale + slope.to_radians().tan().min(1.5) * e.radius * scale * 0.15)
                .max(under + 0.12 * scale)
        };
        let nv = ctx.rules.kind_variants[kind as usize].max(1);
        let variant = (mix(hs ^ 23) % nv as u64) as u8;
        out.push(Instance {
            id,
            kind,
            entry: entry as u32,
            variant,
            cat: cat as u8,
            pos: Vec3::new(pos2.x, ground - sink, pos2.y),
            rot,
            scale,
        });
    }
}

/// Every instance of the given categories in a world rect.
pub fn place_rect(ctx: &Ctx, min: Vec2, max: Vec2, cats: &[usize]) -> Vec<Instance> {
    let c0 = cell_of(min);
    let c1 = cell_of(max);
    let mut out = vec![];
    for z in c0.y..=c1.y {
        for x in c0.x..=c1.x {
            place_cell(ctx, IVec2::new(x, z), cats, &mut out);
        }
    }
    out.retain(|i| i.pos.x >= min.x && i.pos.z >= min.y && i.pos.x <= max.x && i.pos.z <= max.y);
    out
}

/// Every instance of the given categories in the world (in parallel, cell rows at a time).
pub fn place_world(ctx: &Ctx, cats: &[usize]) -> Vec<Instance> {
    use rayon::prelude::*;
    let half = ctx.terrain.half();
    let c0 = cell_of(Vec2::splat(-half));
    let c1 = cell_of(Vec2::splat(half));
    (c0.y..=c1.y)
        .into_par_iter()
        .flat_map_iter(|z| {
            let mut row = vec![];
            for x in c0.x..=c1.x {
                place_cell(ctx, IVec2::new(x, z), cats, &mut row);
            }
            row
        })
        .collect()
}

/// How many instances of each category grow in the whole world (in parallel, nothing kept).
pub fn count_world(ctx: &Ctx) -> [usize; 4] {
    use rayon::prelude::*;
    let half = ctx.terrain.half();
    let c0 = cell_of(Vec2::splat(-half));
    let c1 = cell_of(Vec2::splat(half));
    (c0.y..=c1.y)
        .into_par_iter()
        .map(|z| {
            let mut n = [0usize; 4];
            let mut buf = vec![];
            for x in c0.x..=c1.x {
                buf.clear();
                place_cell(ctx, IVec2::new(x, z), &[0, 1, 2, 3], &mut buf);
                for i in &buf {
                    n[i.cat as usize] += 1;
                }
            }
            n
        })
        .reduce(|| [0; 4], |a, b| std::array::from_fn(|i| a[i] + b[i]))
}

/// How many instances of each category grow in a world rect where `inside` holds (in parallel,
/// nothing kept).
pub fn count_area(
    ctx: &Ctx,
    min: Vec2,
    max: Vec2,
    inside: &(dyn Fn(Vec2) -> bool + Sync),
) -> [usize; 4] {
    use rayon::prelude::*;
    let half = ctx.terrain.half();
    let c0 = cell_of(min.max(Vec2::splat(-half)));
    let c1 = cell_of(max.min(Vec2::splat(half)));
    (c0.y..=c1.y)
        .into_par_iter()
        .map(|z| {
            let mut n = [0usize; 4];
            let mut buf = vec![];
            for x in c0.x..=c1.x {
                buf.clear();
                place_cell(ctx, IVec2::new(x, z), &[0, 1, 2, 3], &mut buf);
                for i in &buf {
                    let p = i.pos.xz();
                    if p.cmpge(min).all() && p.cmple(max).all() && inside(p) {
                        n[i.cat as usize] += 1;
                    }
                }
            }
            n
        })
        .reduce(|| [0; 4], |a, b| std::array::from_fn(|i| a[i] + b[i]))
}

/// The instance with this id, if it still grows.
pub fn find(ctx: &Ctx, id: u64) -> Option<Instance> {
    if !is_procedural(id) {
        return None;
    }
    let cat = ((id >> 60) & 7) as usize;
    let mut v = vec![];
    place_cell(ctx, cell_of_id(id), &[cat], &mut v);
    v.into_iter().find(|i| i.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids_carry_their_cell() {
        for c in [
            IVec2::new(0, 0),
            IVec2::new(-1, 5),
            IVec2::new(-300, -301),
            IVec2::new(511, -512),
        ] {
            for cat in 0..4u64 {
                let id = PROC_BIT
                    | cat << 60
                    | ((c.x as u32 as u64) & 0xF_FFFF) << 40
                    | ((c.y as u32 as u64) & 0xF_FFFF) << 20
                    | 77;
                assert!(is_procedural(id));
                assert_eq!(cell_of_id(id), c);
            }
        }
        assert!(!is_procedural(1_338_529));
    }

    #[test]
    fn slot_patterns_are_blue_noise() {
        for (c, pts) in patterns().iter().enumerate() {
            assert_eq!(pts.len(), SLOTS[c]);
            // no two candidates closer than a third of the mean spacing (on the torus)
            let spacing = (CELL * CELL / SLOTS[c] as f32).sqrt();
            for (i, a) in pts.iter().enumerate() {
                for b in &pts[i + 1..] {
                    let dx = (a.x - b.x).abs().min(CELL - (a.x - b.x).abs());
                    let dz = (a.y - b.y).abs().min(CELL - (a.y - b.y).abs());
                    assert!((dx * dx + dz * dz).sqrt() > spacing / 3.0);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "place/flora_tests.rs"]
mod flora_tests;
