//! Long-range attachment tethers (Kim et al. 2012), port of `cloth/tetherBuild.ts`.

use super::graph::ClothGraph;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// One tether: a free particle kept within `rest_radius` of a pinned anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tether {
    /// Cloth-local index of the free particle.
    pub particle: u32,
    /// Cloth-local index of the nearest pinned vertex in one pinned island.
    pub anchor: u32,
    /// Geodesic rest distance along the cloth, times `1 + stretch_tolerance`, m.
    pub rest_radius: f32,
}

#[derive(PartialEq)]
struct Entry(f64, u32);
impl Eq for Entry {}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap on distance, then index for determinism.
        other
            .0
            .partial_cmp(&self.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| other.1.cmp(&self.1))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Up to `max_attachments` tethers per free particle, one per nearest pinned
/// island, anchored at that island's nearest pinned vertex (geodesic distance
/// along rest edge lengths).
pub fn build_tethers(
    graph: &ClothGraph,
    stretch_tolerance: f32,
    max_attachments: usize,
) -> Vec<Tether> {
    let n = graph.positions.len();
    let pinned: Vec<bool> = graph.inv_mass.iter().map(|&w| w == 0.0).collect();
    if !pinned.iter().any(|&p| p) {
        return Vec::new();
    }
    let mut adj: Vec<Vec<(u32, f64)>> = vec![Vec::new(); n];
    for (e, &[a, b]) in graph.distance_pairs.iter().enumerate() {
        let w = graph.distance_rest_lengths[e] as f64;
        adj[a as usize].push((b, w));
        adj[b as usize].push((a, w));
    }
    // Islands: connected components of the pinned-only subgraph.
    let mut island = vec![-1i32; n];
    let mut islands = 0;
    for s in 0..n {
        if !pinned[s] || island[s] != -1 {
            continue;
        }
        let id = islands;
        islands += 1;
        let mut stack = vec![s];
        island[s] = id;
        while let Some(u) = stack.pop() {
            for &(to, _) in &adj[u] {
                let to = to as usize;
                if pinned[to] && island[to] == -1 {
                    island[to] = id;
                    stack.push(to);
                }
            }
        }
    }
    let mut per_particle: Vec<Vec<(i32, f64, u32)>> = vec![Vec::new(); n];
    for id in 0..islands {
        let mut dist = vec![f64::INFINITY; n];
        let mut nearest = vec![u32::MAX; n];
        let mut heap = BinaryHeap::new();
        for i in 0..n {
            if pinned[i] && island[i] == id {
                dist[i] = 0.0;
                nearest[i] = i as u32;
                heap.push(Entry(0.0, i as u32));
            }
        }
        while let Some(Entry(d, u)) = heap.pop() {
            let u = u as usize;
            if d > dist[u] {
                continue;
            }
            for &(to, w) in &adj[u] {
                let nd = d + w;
                if nd < dist[to as usize] {
                    dist[to as usize] = nd;
                    nearest[to as usize] = nearest[u];
                    heap.push(Entry(nd, to));
                }
            }
        }
        for v in 0..n {
            if !pinned[v] && dist[v].is_finite() {
                per_particle[v].push((id, dist[v], nearest[v]));
            }
        }
    }
    let scale = 1.0 + stretch_tolerance as f64;
    let mut out = Vec::new();
    for (v, list) in per_particle.iter_mut().enumerate() {
        if pinned[v] {
            continue;
        }
        list.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        for &(_, d, anchor) in list.iter().take(max_attachments) {
            out.push(Tether {
                particle: v as u32,
                anchor,
                rest_radius: (d * scale) as f32,
            });
        }
    }
    out
}
