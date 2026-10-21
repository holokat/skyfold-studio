//! Fields round the world: patchworks of fields on flat, dry lowland — near villages first —
//! each a block of a few rectangular fields with lanes between them, turned alike, with crops
//! suited to the biome (wheat, barley, corn and hay in temperate land; rye, potatoes and
//! cabbages in the cold; vineyards, sunflowers and lavender where it's warm; rice and tea in the
//! tropics; none in deserts, swamps or volcanic land).

use bevy::prelude::*;

use super::{Crop, PlantWorld};
use crate::noise::Rng;
use crate::objects::ObjectStore;
use crate::world::Biome;

/// What the generator was asked for.
pub struct Auto {
    /// patchworks to lay out
    pub clusters: usize,
    /// only inside this rectangle (world x, z)
    pub area: Option<(Vec2, Vec2)>,
    /// only these crops (empty: by biome)
    pub crops: Vec<Crop>,
    pub seed: u32,
}

fn biome(w: &PlantWorld, p: Vec2) -> Biome {
    crate::biomes::BiomePreset::from_id(w.terrain.biome_at(p.x, p.y))
        .map(|b| b.def().biome)
        .unwrap_or(Biome::Temperate)
}

/// Crops for a biome, with weights (empty: no farming there).
fn crops_for(b: Biome) -> &'static [(Crop, f32)] {
    use Crop::*;
    match b {
        Biome::Temperate | Biome::Autumn => &[
            (Wheat, 3.0),
            (Barley, 2.0),
            (Corn, 2.0),
            (Hay, 2.0),
            (Potatoes, 1.0),
            (Sunflowers, 1.0),
            (Plowed, 1.0),
            (Cabbages, 0.7),
            (Pumpkins, 0.7),
            (Rye, 0.7),
            (Carrots, 0.5),
            (Strawberries, 0.4),
            (Tulips, 0.3),
            (Hops, 0.3),
            (Vineyard, 0.4),
            (Lavender, 0.3),
        ],
        Biome::Boreal | Biome::Alpine | Biome::Tundra => &[
            (Rye, 2.0),
            (Barley, 2.0),
            (Potatoes, 2.0),
            (Hay, 2.5),
            (Cabbages, 1.0),
            (Plowed, 1.0),
        ],
        Biome::Tropical => &[(Rice, 3.0), (Tea, 2.0), (Corn, 1.0), (Cotton, 0.6)],
        Biome::Desert | Biome::Swamp | Biome::Volcanic => &[],
    }
}

fn pick(rng: &mut Rng, list: &[(Crop, f32)]) -> Option<Crop> {
    let total: f32 = list.iter().map(|x| x.1).sum();
    let mut t = rng.f32() * total;
    for (c, w) in list {
        if t < *w {
            return Some(*c);
        }
        t -= w;
    }
    list.last().map(|x| x.0)
}

/// Ground fit for farming at `p` (radius `r`): flat (≤ ~6°), dry, above the sea, below the
/// mountains, no objects. `None` where not.
fn site_ok(
    w: &PlantWorld,
    store: &ObjectStore,
    p: Vec2,
    r: f32,
    max_slope_deg: f32,
) -> Option<f32> {
    let t = &*w.terrain;
    let cos = max_slope_deg.to_radians().cos();
    let mut flat = 0.0;
    for k in 0..9 {
        let q = if k == 0 {
            p
        } else {
            p + Vec2::from_angle(k as f32 * std::f32::consts::TAU / 8.0) * r
        };
        if !t.in_bounds(q.x, q.y) {
            return None;
        }
        let h = t.height_at(q.x, q.y);
        if h < t.sea_level + 1.5 || h > t.sea_level + 280.0 {
            return None;
        }
        if w.water.is_underwater(Vec3::new(q.x, h, q.y), 0.3) {
            return None;
        }
        let n = t.normal_at(q.x, q.y);
        if n.y < cos {
            return None;
        }
        // not on beaches, bare rock or snow
        let g = t.world_to_grid(q.x, q.y);
        let w = t.final_weights(
            (g.x.round().max(0.0) as usize).min(t.n() - 1),
            (g.y.round().max(0.0) as usize).min(t.n() - 1),
        );
        use crate::terrain::layers::Layer;
        if w[Layer::Sand as usize]
            + w[Layer::Rock as usize]
            + w[Layer::DarkRock as usize]
            + w[Layer::Snow as usize]
            + w[Layer::Ice as usize]
            + w[Layer::Gravel as usize]
            > 0.35
        {
            return None;
        }
        flat += n.y;
    }
    if !store.query(p, r * 1.2).is_empty() {
        return None;
    }
    if w.fields
        .list
        .iter()
        .any(|f| f.contains(p) || crate::setpieces::poly_sdf(&f.poly(), p) < r)
    {
        return None;
    }
    Some(flat / 9.0)
}

/// Lays out fields; returns the new fields' ids. Plants into the open undo step.
pub fn auto(w: &mut PlantWorld, store: &ObjectStore, opts: &Auto) -> Result<Vec<u64>, String> {
    let half = w.terrain.half();
    let (lo, hi) = opts
        .area
        .unwrap_or((Vec2::splat(-half + 40.0), Vec2::splat(half - 40.0)));
    let mut rng = Rng::new(opts.seed as u64 * 977 + 11);
    // candidate sites on a jittered grid, scored by flatness and nearby buildings (farms ring
    // villages)
    let step = ((hi - lo).max_element() / 40.0).clamp(60.0, 160.0);
    let mut sites: Vec<(f32, Vec2)> = vec![];
    let mut y = lo.y + step * 0.5;
    while y < hi.y {
        let mut x = lo.x + step * 0.5;
        while x < hi.x {
            let p = Vec2::new(
                x + rng.range(-0.3, 0.3) * step,
                y + rng.range(-0.3, 0.3) * step,
            );
            if !crops_for(biome(w, p)).is_empty() || !opts.crops.is_empty() {
                if let Some(flat) = site_ok(w, store, p, 35.0, 8.0) {
                    let near = store.query(p, 380.0).len().min(40) as f32 / 40.0;
                    sites.push((flat + near * 0.6 + rng.f32() * 0.25, p));
                }
            }
            x += step;
        }
        y += step;
    }
    sites.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut chosen: Vec<Vec2> = vec![];
    for (_, p) in sites {
        if chosen.len() >= opts.clusters {
            break;
        }
        if chosen.iter().all(|c| c.distance(p) > 260.0) {
            chosen.push(p);
        }
    }
    if chosen.is_empty() {
        return Err("no flat, dry farmland found there (fields want gentle lowland away from water, buildings and other fields)".into());
    }
    let mut ids = vec![];
    for centre in chosen {
        let b = biome(w, centre);
        let list: Vec<(Crop, f32)> = if opts.crops.is_empty() {
            crops_for(b).to_vec()
        } else {
            opts.crops.iter().map(|c| (*c, 1.0)).collect()
        };
        // a block of fields: columns × rows of rectangles, lanes between them, all turned alike
        let turn = rng.f32() * std::f32::consts::PI;
        let rot = Quat::from_rotation_y(turn);
        let (cols, rows) = (2 + rng.below(2) as i32, 1 + rng.below(3) as i32);
        let widths: Vec<f32> = (0..cols).map(|_| rng.range(45.0, 105.0)).collect();
        let depths: Vec<f32> = (0..rows).map(|_| rng.range(32.0, 75.0)).collect();
        let lane = rng.range(3.0, 5.0);
        let total = Vec2::new(
            widths.iter().sum::<f32>() + lane * (cols - 1) as f32,
            depths.iter().sum::<f32>() + lane * (rows - 1) as f32,
        );
        let mut z0 = -total.y * 0.5;
        for (j, d) in depths.iter().enumerate() {
            let mut x0 = -total.x * 0.5;
            for (i, wd) in widths.iter().enumerate() {
                // (a gap in the block now and then: a meadow, a copse)
                if rng.f32() < 0.18 && (i + j) > 0 {
                    x0 += wd + lane;
                    continue;
                }
                let jitter = |rng: &mut Rng| Vec2::new(rng.range(-2.5, 2.5), rng.range(-2.5, 2.5));
                let j4 = [
                    jitter(&mut rng),
                    jitter(&mut rng),
                    jitter(&mut rng),
                    jitter(&mut rng),
                ];
                let (fx, fz) = (x0, z0);
                x0 += wd + lane;
                // the field, or (where it doesn't fit) a smaller one in the same corner
                let shape = |k: f32| -> Vec<Vec2> {
                    let (wk, dk) = (wd * k, d * k);
                    [
                        Vec2::new(fx, fz) + j4[0],
                        Vec2::new(fx + wk, fz) + j4[1],
                        Vec2::new(fx + wk, fz + dk) + j4[2],
                        Vec2::new(fx, fz + dk) + j4[3],
                    ]
                    .iter()
                    .map(|c| centre + (rot * Vec3::new(c.x, 0.0, c.y)).xz())
                    .collect()
                };
                let fits = |w: &PlantWorld, poly: &[Vec2]| {
                    let mid = poly.iter().copied().sum::<Vec2>() / 4.0;
                    // every corner, edge middle and the centre on farmland
                    (0..4)
                        .flat_map(|k| [poly[k], poly[k].lerp(poly[(k + 1) % 4], 0.5)])
                        .chain([mid])
                        .all(|q| site_ok(w, store, q, 2.0, 12.0).is_some())
                };
                let Some(poly) = [1.0, 0.65].into_iter().map(shape).find(|p| fits(w, p)) else {
                    continue;
                };
                let Some(crop) = pick(&mut rng, &list) else {
                    continue;
                };
                // rows along the field's long side, or across it now and then
                let rows_dir = if rng.f32() < 0.8 {
                    super::long_axis(&poly)
                } else {
                    super::long_axis(&poly) + std::f32::consts::FRAC_PI_2
                };
                if let Ok(id) = super::add(w, crop, &poly, Some(rows_dir), "Plant fields") {
                    ids.push(id);
                }
            }
            z0 += d + lane;
        }
    }
    if ids.is_empty() {
        return Err("the farmland found was too broken up for whole fields".into());
    }
    Ok(ids)
}
