use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

use serde::Serialize;

const NORMAL_CANDIDATE_LIMIT: usize = 18;
const HARD_CANDIDATE_LIMIT: usize = 24;

#[derive(Clone, Debug, Serialize)]
pub(super) struct Region {
    pub id: u8,
    pub map_rgb: [u8; 3],
    pub anchor: [f32; 2],
    pub sampled_cells: usize,
    pub elevation: [f32; 2],
    pub biomes: Vec<(u8, usize)>,
    pub adjacent: Vec<u8>,
    pub start_distance: f32,
    pub contains_start: bool,
    pub touches_start_guard: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Partition {
    pub side: usize,
    /// Candidate IDs on eligible (dry and unpainted) samples. Zero is water or preserved paint.
    pub labels: Vec<u8>,
    /// Cached nearest-candidate lookup for output samples that land between coarse samples.
    resolved: Vec<u8>,
    pub regions: Vec<Region>,
}

#[derive(Copy, Clone)]
struct Heap {
    cost: f32,
    at: usize,
    owner: u8,
}
impl Eq for Heap {}
impl PartialEq for Heap {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost && self.at == other.at && self.owner == other.owner
    }
}
impl Ord for Heap {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.at.cmp(&self.at))
            .then_with(|| other.owner.cmp(&self.owner))
    }
}
impl PartialOrd for Heap {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn color(id: u8) -> [u8; 3] {
    let h = id.wrapping_mul(67);
    [
        70 + h % 160,
        70 + h.wrapping_mul(3) % 150,
        70 + h.wrapping_mul(7) % 140,
    ]
}

fn neighbors8(side: usize, at: usize) -> impl Iterator<Item = (usize, f32)> {
    let x = at % side;
    let y = at / side;
    let left = x.checked_sub(1);
    let up = y.checked_sub(1);
    [
        left.map(|nx| (y * side + nx, 1.0)),
        (x + 1 < side).then_some((y * side + x + 1, 1.0)),
        up.map(|ny| (ny * side + x, 1.0)),
        (y + 1 < side).then_some(((y + 1) * side + x, 1.0)),
        match (left, up) {
            (Some(nx), Some(ny)) => Some((ny * side + nx, std::f32::consts::SQRT_2)),
            _ => None,
        },
        match (x + 1 < side, up) {
            (true, Some(ny)) => Some((ny * side + x + 1, std::f32::consts::SQRT_2)),
            _ => None,
        },
        match (left, y + 1 < side) {
            (Some(nx), true) => Some(((y + 1) * side + nx, std::f32::consts::SQRT_2)),
            _ => None,
        },
        (x + 1 < side && y + 1 < side)
            .then_some(((y + 1) * side + x + 1, std::f32::consts::SQRT_2)),
    ]
    .into_iter()
    .flatten()
}

fn hash01(x: i32, y: i32) -> f32 {
    let mut n = (x as u32).wrapping_mul(0x1f12_3bb5) ^ (y as u32).wrapping_mul(0x5f35_6495);
    n ^= n >> 16;
    n = n.wrapping_mul(0x7feb_352d);
    n ^= n >> 15;
    n = n.wrapping_mul(0x846c_a68b);
    ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
}
fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// A low-frequency deterministic field breaks perfect ties on flat land without cell-scale noise.
fn organic_field(side: usize) -> Vec<f32> {
    let spacing = (side / 9).clamp(5, 14) as f32;
    (0..side * side)
        .map(|at| {
            let gx = (at % side) as f32 / spacing;
            let gy = (at / side) as f32 / spacing;
            let ix = gx.floor() as i32;
            let iy = gy.floor() as i32;
            let tx = smoothstep(gx - ix as f32);
            let ty = smoothstep(gy - iy as f32);
            let a = hash01(ix, iy);
            let b = hash01(ix + 1, iy);
            let c = hash01(ix, iy + 1);
            let d = hash01(ix + 1, iy + 1);
            let top = a + (b - a) * tx;
            top + ((c + (d - c) * tx) - top) * ty
        })
        .collect()
}

fn local_height(height: &[f32], eligible: &[bool], side: usize) -> Vec<f32> {
    (0..height.len())
        .map(|at| {
            if !eligible[at] {
                return height[at];
            }
            let mut total = height[at];
            let mut count = 1.0;
            for (next, _) in neighbors8(side, at) {
                if eligible[next] {
                    total += height[next];
                    count += 1.0;
                }
            }
            total / count
        })
        .collect()
}

fn height_step(height: &[f32], eligible: &[bool], side: usize) -> f32 {
    let mut total = 0.0;
    let mut count = 0usize;
    for at in 0..height.len() {
        if !eligible[at] {
            continue;
        }
        for (next, _) in neighbors8(side, at) {
            if eligible[next] && next > at {
                total += (height[at] - height[next]).abs();
                count += 1;
            }
        }
    }
    (total / count.max(1) as f32).max(0.05)
}

#[allow(clippy::too_many_arguments)]
fn edge_cost(
    a: usize,
    b: usize,
    step: f32,
    height: &[f32],
    smooth_height: &[f32],
    biome: &[u8],
    organic: &[f32],
    height_unit: f32,
    sample_cell: f32,
) -> f32 {
    let raw_slope = (height[a] - height[b]).abs() / height_unit;
    let broad_slope = (smooth_height[a] - smooth_height[b]).abs() / height_unit;
    let terrain_barrier = (raw_slope * 0.5 + broad_slope * 0.5).min(4.0);
    let biome_barrier = if biome[a] == biome[b] { 0.0 } else { 0.65 };
    // On terrain with no measurable relief, this becomes a gentle coherent travel field. It
    // breaks the mechanical symmetry of a flat Voronoi split without competing with ridges.
    let organic_barrier = organic[b] * 0.30 + (organic[a] - organic[b]).abs() * 0.45;
    step * sample_cell * (1.0 + terrain_barrier * 0.75 + biome_barrier + organic_barrier)
}

#[allow(clippy::too_many_arguments)]
fn weighted_distances(
    side: usize,
    eligible: &[bool],
    height: &[f32],
    smooth_height: &[f32],
    biome: &[u8],
    organic: &[f32],
    height_unit: f32,
    sample_cell: f32,
    seeds: &[(usize, u8)],
) -> Vec<f32> {
    let mut out = vec![f32::INFINITY; eligible.len()];
    let mut heap = BinaryHeap::new();
    for &(at, owner) in seeds {
        out[at] = 0.0;
        heap.push(Heap {
            cost: 0.0,
            at,
            owner,
        });
    }
    while let Some(Heap { cost, at, .. }) = heap.pop() {
        if cost != out[at] {
            continue;
        }
        for (next, step) in neighbors8(side, at) {
            if !eligible[next] {
                continue;
            }
            let next_cost = cost
                + edge_cost(
                    at,
                    next,
                    step,
                    height,
                    smooth_height,
                    biome,
                    organic,
                    height_unit,
                    sample_cell,
                );
            if next_cost < out[next] {
                out[next] = next_cost;
                heap.push(Heap {
                    cost: next_cost,
                    at: next,
                    owner: 0,
                });
            }
        }
    }
    out
}

fn components(side: usize, eligible: &[bool]) -> (Vec<usize>, Vec<Vec<usize>>) {
    let mut component = vec![usize::MAX; eligible.len()];
    let mut groups = Vec::new();
    for first in 0..eligible.len() {
        if !eligible[first] || component[first] != usize::MAX {
            continue;
        }
        let id = groups.len();
        let mut queue = VecDeque::from([first]);
        let mut group = Vec::new();
        component[first] = id;
        while let Some(at) = queue.pop_front() {
            group.push(at);
            for (next, _) in neighbors8(side, at) {
                if eligible[next] && component[next] == usize::MAX {
                    component[next] = id;
                    queue.push_back(next);
                }
            }
        }
        groups.push(group);
    }
    (component, groups)
}

fn edge_clearance(side: usize, eligible: &[bool]) -> Vec<u16> {
    let mut clearance = vec![u16::MAX; eligible.len()];
    let mut queue = VecDeque::new();
    for at in 0..eligible.len() {
        if !eligible[at] {
            continue;
        }
        let x = at % side;
        let y = at / side;
        let boundary = x == 0
            || y == 0
            || x + 1 == side
            || y + 1 == side
            || neighbors8(side, at).any(|(next, _)| !eligible[next]);
        if boundary {
            clearance[at] = 0;
            queue.push_back(at);
        }
    }
    while let Some(at) = queue.pop_front() {
        let next_distance = clearance[at].saturating_add(1);
        for (next, _) in neighbors8(side, at) {
            if eligible[next] && clearance[next] > next_distance {
                clearance[next] = next_distance;
                queue.push_back(next);
            }
        }
    }
    clearance
}

fn grid_world(at: usize, side: usize, sample_cell: f32, half: f32) -> [f32; 2] {
    [
        (at % side) as f32 * sample_cell - half,
        (at / side) as f32 * sample_cell - half,
    ]
}

fn start_nearest(
    eligible: &[bool],
    side: usize,
    start: [f32; 2],
    sample_cell: f32,
    half: f32,
) -> usize {
    eligible
        .iter()
        .enumerate()
        .filter(|(_, eligible)| **eligible)
        .min_by(|(a, _), (b, _)| {
            let pa = grid_world(*a, side, sample_cell, half);
            let pb = grid_world(*b, side, sample_cell, half);
            let da = (pa[0] - start[0]).powi(2) + (pa[1] - start[1]).powi(2);
            let db = (pb[0] - start[0]).powi(2) + (pb[1] - start[1]).powi(2);
            da.total_cmp(&db).then_with(|| a.cmp(b))
        })
        .map(|(at, _)| at)
        .expect("the caller checked that an eligible sample exists")
}

fn compact_labels(labels: &mut [u8]) -> Vec<u8> {
    let mut ids: Vec<u8> = labels.iter().copied().filter(|id| *id != 0).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut remap = [0u8; 256];
    for (next, old) in ids.iter().enumerate() {
        remap[*old as usize] = (next + 1) as u8;
    }
    for label in labels {
        *label = remap[*label as usize];
    }
    ids
}

fn merge_slivers(side: usize, labels: &mut [u8], height: &[f32], biome: &[u8], min_area: usize) {
    loop {
        let mut sizes = BTreeMap::<u8, usize>::new();
        for &id in labels.iter().filter(|id| **id != 0) {
            *sizes.entry(id).or_default() += 1;
        }
        let Some((&small, _)) = sizes.iter().find(|(_, area)| **area < min_area) else {
            break;
        };
        if sizes.len() <= 1 {
            break;
        }
        let mut score = BTreeMap::<u8, f32>::new();
        for at in 0..labels.len() {
            if labels[at] != small {
                continue;
            }
            for (next, _) in neighbors8(side, at) {
                let other = labels[next];
                if other == 0 || other == small {
                    continue;
                }
                let terrain_match = 1.0 / (1.0 + (height[at] - height[next]).abs());
                *score.entry(other).or_default() +=
                    terrain_match + if biome[at] == biome[next] { 0.35 } else { 0.0 };
            }
        }
        let Some((&winner, _)) = score
            .iter()
            .max_by(|(a_id, a), (b_id, b)| a.total_cmp(b).then_with(|| b_id.cmp(a_id)))
        else {
            // An island has no dry neighbor; retaining it is safer than crossing water or paint.
            break;
        };
        for label in labels.iter_mut() {
            if *label == small {
                *label = winner;
            }
        }
    }
    compact_labels(labels);
}

fn core_depths(side: usize, labels: &[u8]) -> Vec<u16> {
    let mut depth = vec![u16::MAX; labels.len()];
    let mut queue = VecDeque::new();
    for at in 0..labels.len() {
        let id = labels[at];
        if id == 0 {
            continue;
        }
        let x = at % side;
        let y = at / side;
        if x == 0
            || y == 0
            || x + 1 == side
            || y + 1 == side
            || neighbors8(side, at).any(|(next, _)| labels[next] != id)
        {
            depth[at] = 0;
            queue.push_back(at);
        }
    }
    while let Some(at) = queue.pop_front() {
        let id = labels[at];
        let next_depth = depth[at].saturating_add(1);
        for (next, _) in neighbors8(side, at) {
            if labels[next] == id && depth[next] > next_depth {
                depth[next] = next_depth;
                queue.push_back(next);
            }
        }
    }
    depth
}

fn resolved_labels(side: usize, labels: &[u8]) -> Vec<u8> {
    let mut resolved = labels.to_vec();
    let mut distance = vec![u16::MAX; labels.len()];
    let mut queue = VecDeque::new();
    for (at, &id) in labels.iter().enumerate() {
        if id != 0 {
            distance[at] = 0;
            queue.push_back(at);
        }
    }
    while let Some(at) = queue.pop_front() {
        let next_distance = distance[at].saturating_add(1);
        for (next, _) in neighbors8(side, at) {
            if distance[next] > next_distance
                || (distance[next] == next_distance && resolved[at] < resolved[next])
            {
                distance[next] = next_distance;
                resolved[next] = resolved[at];
                queue.push_back(next);
            }
        }
    }
    resolved
}

impl Partition {
    pub fn build(
        side: usize,
        dry: Vec<bool>,
        height: Vec<f32>,
        biome: Vec<u8>,
        start: [f32; 2],
        half: f32,
        sample_cell: f32,
    ) -> Result<Self, String> {
        if side < 2
            || dry.len() != side * side
            || height.len() != dry.len()
            || biome.len() != dry.len()
            || !sample_cell.is_finite()
            || sample_cell <= 0.0
        {
            return Err("Invalid terrain candidate grid".into());
        }
        let count = dry.iter().filter(|eligible| **eligible).count();
        if count == 0 {
            return Err("There is no dry unzoned terrain for candidates".into());
        }
        let (component, groups) = components(side, &dry);
        if groups.len() > HARD_CANDIDATE_LIMIT {
            return Err(format!(
                "Terrain has {} disconnected unzoned land pieces; auto zones supports at most {HARD_CANDIDATE_LIMIT}",
                groups.len()
            ));
        }
        let smooth_height = local_height(&height, &dry, side);
        let organic = organic_field(side);
        let height_unit = height_step(&height, &dry, side);
        let clearance = edge_clearance(side, &dry);
        let nearest_start = start_nearest(&dry, side, start, sample_cell, half);
        // Give the planner enough spatial steps for progression. It can merge candidates,
        // but cannot split a coarse region that would force late-game land beside the start.
        let target = (count / 350)
            .clamp(8, NORMAL_CANDIDATE_LIMIT)
            .max(groups.len());

        // Each disconnected land piece gets an interior seed. The nearest dry candidate is tied
        // to the actual player position even when the player stands inside preserved paint.
        let mut seeds = Vec::<(usize, u8)>::new();
        let start_component = component[nearest_start];
        for (group_id, group) in groups.iter().enumerate() {
            let seed = if group_id == start_component {
                nearest_start
            } else {
                *group
                    .iter()
                    .max_by_key(|at| (clearance[**at], std::cmp::Reverse(**at)))
                    .expect("components are nonempty")
            };
            seeds.push((seed, (seeds.len() + 1) as u8));
        }
        // Farthest-point sampling uses terrain-aware physical travel distance and favors interior land.
        while seeds.len() < target {
            let distances = weighted_distances(
                side,
                &dry,
                &height,
                &smooth_height,
                &biome,
                &organic,
                height_unit,
                sample_cell,
                &seeds,
            );
            let candidate = (0..dry.len())
                .filter(|at| dry[*at] && !seeds.iter().any(|(seed, _)| *seed == *at))
                .max_by(|a, b| {
                    let a_score = distances[*a] * (0.60 + clearance[*a].min(10) as f32 * 0.04);
                    let b_score = distances[*b] * (0.60 + clearance[*b].min(10) as f32 * 0.04);
                    a_score.total_cmp(&b_score).then_with(|| b.cmp(a))
                });
            let Some(seed) = candidate else { break };
            seeds.push((seed, (seeds.len() + 1) as u8));
        }

        let mut labels = vec![0u8; dry.len()];
        let mut costs = vec![f32::INFINITY; dry.len()];
        let mut heap = BinaryHeap::new();
        for &(at, owner) in &seeds {
            labels[at] = owner;
            costs[at] = 0.0;
            heap.push(Heap {
                cost: 0.0,
                at,
                owner,
            });
        }
        while let Some(Heap { cost, at, owner }) = heap.pop() {
            if cost != costs[at] || owner != labels[at] {
                continue;
            }
            for (next, step) in neighbors8(side, at) {
                if !dry[next] {
                    continue;
                }
                let next_cost = cost
                    + edge_cost(
                        at,
                        next,
                        step,
                        &height,
                        &smooth_height,
                        &biome,
                        &organic,
                        height_unit,
                        sample_cell,
                    );
                if next_cost < costs[next]
                    || (next_cost == costs[next] && (labels[next] == 0 || owner < labels[next]))
                {
                    costs[next] = next_cost;
                    labels[next] = owner;
                    heap.push(Heap {
                        cost: next_cost,
                        at: next,
                        owner,
                    });
                }
            }
        }
        let min_area = (count / (seeds.len().max(1) * 6)).clamp(12, 96);
        merge_slivers(side, &mut labels, &height, &biome, min_area);
        let ids = compact_labels(&mut labels);
        let depths = core_depths(side, &labels);
        let resolved = resolved_labels(side, &labels);
        let start_source = [(nearest_start, 0u8)];
        let mut start_cost = weighted_distances(
            side,
            &dry,
            &height,
            &smooth_height,
            &biome,
            &organic,
            height_unit,
            sample_cell,
            &start_source,
        );
        let nearest_world = grid_world(nearest_start, side, sample_cell, half);
        let start_offset =
            ((nearest_world[0] - start[0]).powi(2) + (nearest_world[1] - start[1]).powi(2)).sqrt();
        for cost in &mut start_cost {
            if cost.is_finite() {
                *cost += start_offset;
            }
        }

        let mut regions = Vec::with_capacity(ids.len());
        for id in 1..=ids.len() as u8 {
            let members: Vec<usize> = labels
                .iter()
                .enumerate()
                .filter_map(|(at, label)| (*label == id).then_some(at))
                .collect();
            let anchor_at = *members
                .iter()
                .max_by_key(|at| (depths[**at], std::cmp::Reverse(**at)))
                .expect("each compact label has members");
            let mut elevation = [f32::INFINITY, f32::NEG_INFINITY];
            let mut biomes = BTreeMap::<u8, usize>::new();
            let mut adjacent = BTreeSet::new();
            let mut touches_start_guard = false;
            let mut contains_start = false;
            for &at in &members {
                elevation[0] = elevation[0].min(height[at]);
                elevation[1] = elevation[1].max(height[at]);
                *biomes.entry(biome[at]).or_default() += 1;
                let world = grid_world(at, side, sample_cell, half);
                let d2 = (world[0] - start[0]).powi(2) + (world[1] - start[1]).powi(2);
                touches_start_guard |= d2 <= 180.0_f32.powi(2);
                contains_start |= at == nearest_start;
                for (next, _) in neighbors8(side, at) {
                    if labels[next] != 0 && labels[next] != id {
                        adjacent.insert(labels[next]);
                    }
                }
            }
            regions.push(Region {
                id,
                map_rgb: color(id),
                anchor: grid_world(anchor_at, side, sample_cell, half),
                sampled_cells: members.len(),
                elevation,
                biomes: biomes.into_iter().collect(),
                adjacent: adjacent.into_iter().collect(),
                start_distance: start_cost[anchor_at],
                contains_start,
                touches_start_guard,
            });
        }
        Ok(Self {
            side,
            labels,
            resolved,
            regions,
        })
    }

    pub fn label_at(&self, x: usize, y: usize, n: usize) -> u8 {
        if n == 0 {
            return 0;
        }
        // The output grid is usually much finer than the candidate sample. A bilinear label
        // vote avoids expanding every coarse cell into a visibly square block while retaining
        // the fixed candidate geometry and the cached zero-cell fallback.
        let scale = (self.side - 1) as f32 / (n - 1).max(1) as f32;
        let gx = x.min(n - 1) as f32 * scale;
        let gy = y.min(n - 1) as f32 * scale;
        let x0 = gx.floor() as usize;
        let y0 = gy.floor() as usize;
        let x1 = (x0 + 1).min(self.side - 1);
        let y1 = (y0 + 1).min(self.side - 1);
        let tx = gx - x0 as f32;
        let ty = gy - y0 as f32;
        let mut vote = BTreeMap::<u8, f32>::new();
        for (sx, sy, weight) in [
            (x0, y0, (1.0 - tx) * (1.0 - ty)),
            (x1, y0, tx * (1.0 - ty)),
            (x0, y1, (1.0 - tx) * ty),
            (x1, y1, tx * ty),
        ] {
            let at = sy * self.side + sx;
            let id = if self.labels[at] != 0 {
                self.labels[at]
            } else {
                self.resolved[at]
            };
            *vote.entry(id).or_default() += weight;
        }
        vote.into_iter()
            .max_by(|(a_id, a), (b_id, b)| a.total_cmp(b).then_with(|| b_id.cmp(a_id)))
            .map(|(id, _)| id)
            .unwrap_or(0)
    }

    pub fn connected(&self, ids: &[u8]) -> bool {
        if ids.is_empty() {
            return false;
        }
        let set: BTreeSet<_> = ids.iter().copied().collect();
        let mut seen = BTreeSet::from([ids[0]]);
        let mut todo = VecDeque::from([ids[0]]);
        while let Some(id) = todo.pop_front() {
            if let Some(region) = self.regions.iter().find(|region| region.id == id) {
                for next in &region.adjacent {
                    if set.contains(next) && seen.insert(*next) {
                        todo.push_back(*next);
                    }
                }
            }
        }
        seen == set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn build(side: usize, dry: Vec<bool>, height: Vec<f32>, biome: Vec<u8>) -> Partition {
        Partition::build(
            side,
            dry,
            height,
            biome,
            [0.0, 0.0],
            (side - 1) as f32 / 2.0,
            1.0,
        )
        .unwrap()
    }

    #[test]
    fn flat_land_uses_compact_non_strip_candidates() {
        let side = 96;
        let partition = build(
            side,
            vec![true; side * side],
            vec![0.0; side * side],
            vec![0; side * side],
        );
        assert!((8..=NORMAL_CANDIDATE_LIMIT).contains(&partition.regions.len()));
        for region in &partition.regions {
            assert!(
                region.sampled_cells >= 48,
                "candidate {} was a sliver",
                region.id
            );
            let cells: Vec<_> = partition
                .labels
                .iter()
                .enumerate()
                .filter_map(|(at, id)| (*id == region.id).then_some((at % side, at / side)))
                .collect();
            let width = cells.iter().map(|(x, _)| x).max().unwrap()
                - cells.iter().map(|(x, _)| x).min().unwrap()
                + 1;
            let height = cells.iter().map(|(_, y)| y).max().unwrap()
                - cells.iter().map(|(_, y)| y).min().unwrap()
                + 1;
            assert!(
                width >= 4 && height >= 4,
                "candidate {} is a strip",
                region.id
            );
        }
    }

    #[test]
    fn steep_ridge_is_preferred_as_a_candidate_boundary() {
        let side = 64;
        let mut height = vec![0.0; side * side];
        for y in 0..side {
            for x in 31..33 {
                height[y * side + x] = 40.0;
            }
        }
        let partition = build(side, vec![true; side * side], height, vec![0; side * side]);
        let crossings = (0..side)
            .filter(|y| partition.labels[y * side + 30] != partition.labels[y * side + 33])
            .count();
        assert!(
            crossings > side / 3,
            "ridge did not influence candidate boundaries"
        );
    }

    #[test]
    fn islands_are_contained_and_every_eligible_cell_is_assigned() {
        let side = 30;
        let mut dry = vec![false; side * side];
        for y in 2..10 {
            for x in 2..10 {
                dry[y * side + x] = true;
            }
        }
        for y in 18..27 {
            for x in 19..28 {
                dry[y * side + x] = true;
            }
        }
        let partition = build(
            side,
            dry.clone(),
            vec![0.0; side * side],
            vec![0; side * side],
        );
        assert!(partition.regions.len() >= 2 && partition.regions.len() <= NORMAL_CANDIDATE_LIMIT);
        assert!(
            dry.iter()
                .enumerate()
                .all(|(at, eligible)| !eligible || partition.labels[at] != 0)
        );
        assert!(
            dry.iter()
                .enumerate()
                .all(|(at, eligible)| *eligible || partition.labels[at] == 0)
        );
    }

    #[test]
    fn preserved_mask_is_never_candidate_geometry() {
        let side = 32;
        let mut dry = vec![true; side * side];
        for y in 0..side {
            dry[y * side + 15] = false;
        }
        let partition = build(
            side,
            dry.clone(),
            vec![0.0; side * side],
            vec![0; side * side],
        );
        for at in 0..dry.len() {
            assert_eq!(partition.labels[at] != 0, dry[at]);
        }
        assert!(
            partition
                .regions
                .iter()
                .all(|region| region.sampled_cells > 0)
        );
    }

    #[test]
    fn excessive_islands_return_a_clear_error() {
        let side = 16;
        let mut dry = vec![false; side * side];
        for y in (0..side).step_by(3) {
            for x in (0..side).step_by(3) {
                dry[y * side + x] = true;
            }
        }
        let error = Partition::build(
            side,
            dry,
            vec![0.0; side * side],
            vec![0; side * side],
            [0.0, 0.0],
            8.0,
            1.0,
        )
        .unwrap_err();
        assert!(error.contains("disconnected"));
    }

    #[test]
    fn label_lookup_is_cached_for_output_grid_sampling() {
        let partition = build(
            5,
            vec![
                true, false, true, true, true, true, false, true, true, true, true, true, true,
                true, true, true, true, true, true, true, true, true, true, true, true,
            ],
            vec![0.0; 25],
            vec![0; 25],
        );
        assert_ne!(partition.label_at(0, 0, 17), 0);
        assert_ne!(partition.label_at(8, 8, 17), 0);
    }

    /// Opt-in terrain visual QA for a saved world, kept ignored so normal tests remain hermetic.
    /// `WB_PARTITION_QA_WORLD` is a `.world` directory and `WB_PARTITION_QA_IMAGE` is a PNG path.
    #[test]
    #[ignore]
    fn manual_saved_world_preview() {
        let Ok(dir) = std::env::var("WB_PARTITION_QA_WORLD") else {
            return;
        };
        let output = std::env::var("WB_PARTITION_QA_IMAGE")
            .expect("set WB_PARTITION_QA_IMAGE with WB_PARTITION_QA_WORLD");
        let root = std::path::Path::new(&dir);
        let doc: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("world.json")).expect("read world.json"),
        )
        .expect("parse world.json");
        let res = doc["res"].as_u64().expect("world res") as usize;
        let n = res + 1;
        let cell = doc["cell"].as_f64().expect("world cell") as f32;
        let sea = doc["sea_level"].as_f64().expect("world sea level") as f32;
        let pos = &doc["gameplay"]["player_start"]["pos"];
        let start = [
            pos[0].as_f64().expect("start x") as f32,
            pos[2].as_f64().expect("start z") as f32,
        ];
        let heights: Vec<f32> = std::fs::read(root.join("heights.f32"))
            .expect("read heights.f32")
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let zones = std::fs::read(root.join("zones.u8")).expect("read zones.u8");
        let biomes = std::fs::read(root.join("biomes.u8")).expect("read biomes.u8");
        assert_eq!(heights.len(), n * n);
        assert_eq!(zones.len(), n * n);
        assert_eq!(biomes.len(), n * n);
        let side = n.min(128);
        let mut dry = Vec::with_capacity(side * side);
        let mut sample_height = Vec::with_capacity(side * side);
        let mut sample_biome = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                let at = (y * (n - 1) / (side - 1)) * n + x * (n - 1) / (side - 1);
                dry.push(heights[at] > sea && zones[at] == 0);
                sample_height.push(heights[at]);
                sample_biome.push(biomes[at]);
            }
        }
        let partition = Partition::build(
            side,
            dry,
            sample_height,
            sample_biome,
            start,
            (n - 1) as f32 * cell * 0.5,
            (n - 1) as f32 * cell / (side - 1) as f32,
        )
        .expect("build partition");
        let mut image = image::RgbImage::new(side as u32, side as u32);
        for y in 0..side {
            for x in 0..side {
                let id = partition.labels[y * side + x];
                image.put_pixel(
                    x as u32,
                    y as u32,
                    image::Rgb(if id == 0 { [25, 52, 82] } else { color(id) }),
                );
            }
        }
        image.save(output).expect("write candidate preview");
    }
}
