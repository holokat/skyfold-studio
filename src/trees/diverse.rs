//! Bamboo geometry that is deliberately separate from ez-tree's single-trunk skeleton.
//! Every LOD reads the same deterministic culm, node, and canopy anchors, then retains a
//! smaller subset. This keeps a tall grove coherent while the triangle count falls with range.

use std::f32::consts::TAU;

use bevy::prelude::Vec3;
use ez_tree::Buffers;

use crate::plants::Lods;

/// A small grove reads as bamboo more readily than a dense, shrub-like clump. The bases use a
/// golden-angle spiral, so neighboring instances do not resolve into a repeated hexagon.
const CULMS: usize = 6;
/// One UV-V unit is one internode. At 12--22 m this yields 0.35--0.65 m node spacing.
const NODES: usize = 34;
const LEAF_NODES: [usize; 5] = [19, 23, 27, 30, 33];
const TWIG_SLOTS: usize = 8;
const SPRAY_CARDS_PER_BRANCH: usize = 2;
const LOD_CULMS: [&[usize]; 3] = [&[0, 1, 2, 3, 4, 5], &[0, 1, 3, 5], &[0, 3, 5]];
const LOD_LEAF_NODES: [&[usize]; 3] = [&LEAF_NODES, &[19, 27, 33], &[27, 33]];

fn rand(seed: u64, n: u32) -> f32 {
    let mut x = seed ^ (n as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    ((x ^ (x >> 31)) >> 40) as f32 / ((1u32 << 24) - 1) as f32
}

fn leaf_v(b: &mut Buffers, p: Vec3, n: Vec3, uv: [f32; 2], sway: f32) {
    b.positions.push(p.to_array());
    b.normals.push(n.normalize_or(Vec3::Y).to_array());
    b.uvs.push(uv);
    b.sway.push(sway.clamp(0.0, 1.0));
}

fn bark_v(b: &mut Buffers, p: Vec3, n: Vec3, uv: [f32; 2], bark: [f32; 3], sway: f32) {
    leaf_v(b, p, n, uv, sway);
    b.bark.push(bark);
}

fn culm_base(seed: u64, culm: usize, height: f32) -> Vec3 {
    if culm == 0 {
        return Vec3::new(0.0, -0.10, 0.0);
    }
    let angle = culm as f32 * 2.399_963 + rand(seed, 100 + culm as u32) * 0.34;
    let radius = (culm as f32 / (CULMS - 1) as f32).sqrt() * height * 0.112;
    Vec3::new(angle.cos() * radius, -0.10, angle.sin() * radius)
}

/// Fixed, slightly bowed axial anchors. All LODs call this exact function before selecting a
/// culm or an upper branch node, so a transition cannot move a surviving attachment point.
fn culm_node(seed: u64, culm: usize, height: f32, node: usize) -> Vec3 {
    let base = culm_base(seed, culm, height);
    let t = node as f32 / NODES as f32;
    let angle = culm as f32 * 2.399_963 + rand(seed, 100 + culm as u32) * 0.34;
    let outward = Vec3::new(angle.cos(), 0.0, angle.sin());
    let sideways = Vec3::new(-angle.sin(), 0.0, angle.cos());
    let total_height = height * (0.88 + rand(seed, 180 + culm as u32) * 0.15);
    let lean = height * (0.018 + rand(seed, 200 + culm as u32) * 0.034);
    let bow = (t * TAU * (0.72 + rand(seed, 220 + culm as u32) * 0.28)).sin()
        * height
        * (0.0035 + rand(seed, 240 + culm as u32) * 0.0045)
        * (t * (1.0 - t) * 4.0);
    base + Vec3::Y * total_height * t + outward * lean * t.powf(1.65) + sideways * bow
}

fn culm_point(seed: u64, culm: usize, height: f32, axial: f32) -> Vec3 {
    let axial = axial.clamp(0.0, NODES as f32);
    let lower = axial.floor() as usize;
    if lower >= NODES {
        return culm_node(seed, culm, height, NODES);
    }
    culm_node(seed, culm, height, lower).lerp(
        culm_node(seed, culm, height, lower + 1),
        axial - lower as f32,
    )
}

fn culm_radius(height: f32, axial: f32) -> f32 {
    let t = axial / NODES as f32;
    height * 0.0074 * (1.0 - t * 0.43).max(0.52)
}

/// Samples at the shoulder, collar and trailing shoulder of every node. `axial` is also the
/// canonical bark V coordinate: a material repeats once in each integer-wide internode.
fn culm_profile(lod: usize) -> Vec<(f32, f32)> {
    let offsets: &[f32] = match lod {
        0 => &[0.0, 0.075, 0.155],
        _ => &[0.0, 0.060, 0.145],
    };
    let mut profile = vec![(0.0, 1.045)];
    for node in 1..=NODES {
        profile.push((node as f32 - 0.145, 1.0));
        profile.push((node as f32 - offsets[1], 1.105));
        profile.push((node as f32 + offsets[0], 1.115));
        if node < NODES {
            profile.push((node as f32 + offsets[2], 1.012));
        }
    }
    profile.sort_by(|a, b| a.0.total_cmp(&b.0));
    profile
}

fn culm_frame(tangent: Vec3) -> (Vec3, Vec3) {
    let radial_x = (Vec3::X - tangent * tangent.x).normalize_or(Vec3::Z);
    (radial_x, tangent.cross(radial_x).normalize_or(Vec3::Z))
}

/// Adds a full curved culm with raised, smooth node collars. Its UV contract is intentional:
/// U wraps once (0..1) around a culm and V stays in global internode coordinates across every
/// profile ring and every LOD. `bark` mirrors this for the bamboo material path in `trees.rs`.
fn culm_surface(b: &mut Buffers, seed: u64, culm: usize, height: f32, lod: usize, sides: usize) {
    let profile = culm_profile(lod);
    let sides = sides.max(3);
    let base = b.positions.len() as u32;
    let ring = sides as u32 + 1;
    for (k, &(axial, collar)) in profile.iter().enumerate() {
        let p = culm_point(seed, culm, height, axial);
        let before = profile[k.saturating_sub(1)];
        let after = profile[(k + 1).min(profile.len() - 1)];
        let tangent = (culm_point(seed, culm, height, after.0)
            - culm_point(seed, culm, height, before.0))
        .normalize_or(Vec3::Y);
        let arc = culm_point(seed, culm, height, after.0)
            .distance(culm_point(seed, culm, height, before.0))
            .max(0.0001);
        let r = culm_radius(height, axial) * collar;
        let slope = (culm_radius(height, after.0) * after.1
            - culm_radius(height, before.0) * before.1)
            / arc;
        let (x, z) = culm_frame(tangent);
        for side in 0..=sides {
            let u = side as f32 / sides as f32;
            let theta = u * TAU;
            let radial = x * theta.cos() + z * theta.sin();
            let normal = (radial - tangent * slope).normalize_or(radial);
            bark_v(
                b,
                p + radial * r,
                normal,
                [u, axial],
                [u, axial, 1.0],
                (axial / NODES as f32).powf(1.45) * 0.42,
            );
        }
    }
    for row in 0..profile.len() as u32 - 1 {
        for side in 0..sides as u32 {
            let a = base + row * ring + side;
            let c = a + ring;
            b.indices.extend_from_slice(&[a, c, a + 1, a + 1, c, c + 1]);
        }
    }
}

/// A short, tapering woody twig. Its UVs are local because it is not a primary bamboo culm.
fn twig_tube(b: &mut Buffers, a: Vec3, c: Vec3, r0: f32, r1: f32, sides: usize, sway: f32) {
    let sides = sides.max(3);
    let axis = (c - a).normalize_or(Vec3::Y);
    let (x, z) = culm_frame(axis);
    let base = b.positions.len() as u32;
    for (ring, (p, radius)) in [(a, r0.max(0.001)), (c, r1.max(0.001))]
        .into_iter()
        .enumerate()
    {
        for side in 0..=sides {
            let u = side as f32 / sides as f32;
            let theta = u * TAU;
            let normal = (x * theta.cos() + z * theta.sin()).normalize_or(Vec3::Y);
            bark_v(
                b,
                p + normal * radius,
                normal,
                [u, ring as f32],
                [u, ring as f32, TAU * r0],
                sway * ring as f32,
            );
        }
    }
    let ring = sides as u32 + 1;
    for side in 0..sides as u32 {
        b.indices.extend_from_slice(&[
            base + side,
            base + ring + side,
            base + side + 1,
            base + side + 1,
            base + ring + side,
            base + ring + side + 1,
        ]);
    }
}

/// A folded photo-card for the square eight-leaf twig spray texture. The alpha texture supplies
/// the fine individual leaves; the broad card preserves their complete silhouette at distance.
fn spray_card(b: &mut Buffers, base: Vec3, up: Vec3, normal: Vec3, width: f32, sway: f32) {
    let normal = normal.normalize_or(Vec3::Z);
    let side = normal.cross(up).normalize_or(Vec3::X) * width * 0.5;
    let fold = normal * width * 0.075;
    let top = base + up;
    let first = b.positions.len() as u32;
    for (p, uv) in [
        (base - side, [0.0, 0.0]),
        (base + fold, [0.5, 0.0]),
        (base + side, [1.0, 0.0]),
        (top - side, [0.0, 1.0]),
        (top + fold, [0.5, 1.0]),
        (top + side, [1.0, 1.0]),
    ] {
        leaf_v(b, p, normal, uv, sway);
    }
    b.indices.extend_from_slice(&[
        first,
        first + 1,
        first + 4,
        first,
        first + 4,
        first + 3,
        first + 1,
        first + 2,
        first + 5,
        first + 2,
        first + 5,
        first + 4,
    ]);
}

fn has_leaf_node(lod: usize, node: usize) -> bool {
    LOD_LEAF_NODES[lod].contains(&node)
}

fn branch_count(_seed: u64, _culm: usize, _anchor_index: usize) -> usize {
    3
}

/// Canonical radial slots are independent of LOD. Lower levels cull complete branchlets rather
/// than re-aiming them, preserving every surviving attachment and silhouette direction.
fn branch_slot(seed: u64, culm: usize, anchor_index: usize, local: usize) -> usize {
    let first =
        (rand(seed, 1_180 + culm as u32 * 29 + anchor_index as u32) * TWIG_SLOTS as f32) as usize;
    (first + local * 3) % TWIG_SLOTS
}

fn keeps_branchlet(lod: usize, local: usize) -> bool {
    match lod {
        0 => true,
        1 => local != 1,
        _ => local == 0,
    }
}

fn spray_cards_per_culm(seed: u64, culm: usize, lod: usize) -> usize {
    LEAF_NODES
        .iter()
        .enumerate()
        .filter(|&(_, &node)| has_leaf_node(lod, node))
        .map(|(anchor_index, _)| {
            (0..branch_count(seed, culm, anchor_index))
                .filter(|&local| keeps_branchlet(lod, local))
                .count()
                * SPRAY_CARDS_PER_BRANCH
        })
        .sum()
}

fn canopy(
    seed: u64,
    culm: usize,
    height: f32,
    lod: usize,
    bark: &mut Buffers,
    leaves: &mut Buffers,
) {
    let culm_angle = culm as f32 * 2.399_963 + rand(seed, 100 + culm as u32) * 0.34;
    for (anchor_index, &node) in LEAF_NODES.iter().enumerate() {
        if !has_leaf_node(lod, node) {
            continue;
        }
        let axial = node as f32;
        let p = culm_point(seed, culm, height, axial);
        let radius = culm_radius(height, axial) * 1.10;
        for local in 0..branch_count(seed, culm, anchor_index) {
            if !keeps_branchlet(lod, local) {
                continue;
            }
            let slot = branch_slot(seed, culm, anchor_index, local);
            let azimuth =
                culm_angle + slot as f32 * TAU / TWIG_SLOTS as f32 + anchor_index as f32 * 0.23;
            let horizontal = Vec3::new(azimuth.cos(), 0.0, azimuth.sin());
            let side = Vec3::new(-horizontal.z, 0.0, horizontal.x);
            let length = height
                * (0.044 + rand(seed, 1_230 + slot as u32 + anchor_index as u32 * 17) * 0.022);
            let rise = 0.24 + rand(seed, 1_260 + slot as u32) * 0.24 - anchor_index as f32 * 0.035;
            let branch_axis =
                (horizontal * 0.82 + Vec3::Y * rise + side * 0.10).normalize_or(horizontal);
            let tip = p + branch_axis * length;
            twig_tube(
                bark,
                p + horizontal * radius,
                tip,
                radius * 0.20,
                radius * 0.055,
                [5, 4, 3][lod],
                0.56 + axial / NODES as f32 * 0.30,
            );
            for card in 0..SPRAY_CARDS_PER_BRANCH {
                let sign = if card == 0 { -1.0 } else { 1.0 };
                let fork_axis = (branch_axis * 0.42 + side * sign * 0.56 + Vec3::Y * 0.32)
                    .normalize_or(branch_axis);
                let fork_len =
                    height * (0.014 + rand(seed, 1_310 + slot as u32 + card as u32 * 11) * 0.008);
                let fork_tip = tip + fork_axis * fork_len;
                twig_tube(
                    bark,
                    tip - branch_axis * height * 0.007,
                    fork_tip,
                    radius * 0.075,
                    radius * 0.026,
                    [4, 3, 3][lod],
                    0.76 + axial / NODES as f32 * 0.18,
                );
                let spray_height =
                    height * (0.050 + rand(seed, 1_370 + slot as u32 + card as u32 * 13) * 0.025);
                let spray_up = (Vec3::Y * (0.76 + anchor_index as f32 * 0.025)
                    + fork_axis * 0.28
                    + side * sign * 0.18)
                    .normalize_or(Vec3::Y)
                    * spray_height;
                let spray_width = spray_height
                    * (0.55 + rand(seed, 1_440 + slot as u32 + card as u32 * 7) * 0.25);
                let spray_normal = (horizontal * 0.62 + side * sign * 0.72 + Vec3::Y * 0.08)
                    .normalize_or(horizontal);
                spray_card(
                    leaves,
                    fork_tip - spray_up * 0.17,
                    spray_up,
                    spray_normal,
                    spray_width,
                    0.82 + axial / NODES as f32 * 0.14,
                );
            }
        }
    }
}

pub fn bamboo(seed: u64, height: f32) -> Lods {
    let mut out = Vec::with_capacity(3);
    for lod in 0..3 {
        let mut bark = Buffers::default();
        let mut leaves = Buffers::default();
        for &culm in LOD_CULMS[lod] {
            culm_surface(&mut bark, seed, culm, height, lod, [10, 7, 5][lod]);
            canopy(seed, culm, height, lod, &mut bark, &mut leaves);
        }
        out.push((bark, Some(leaves)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_buffer_valid(b: &Buffers, bark: bool) {
        assert_eq!(b.positions.len(), b.normals.len());
        assert_eq!(b.positions.len(), b.uvs.len());
        assert_eq!(b.positions.len(), b.sway.len());
        assert_eq!(b.indices.len() % 3, 0);
        assert!(b.positions.iter().flatten().all(|v| v.is_finite()));
        assert!(b.normals.iter().flatten().all(|v| v.is_finite()));
        assert!(b.uvs.iter().flatten().all(|v| v.is_finite()));
        assert!(b.indices.iter().all(|&i| (i as usize) < b.positions.len()));
        if bark {
            assert_eq!(b.positions.len(), b.bark.len());
            assert!(b.bark.iter().all(|p| p.iter().all(|v| v.is_finite())));
        } else {
            assert!(b.bark.is_empty());
            assert!(b.uvs.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
        }
    }

    #[test]
    fn bamboo_buffers_are_valid_and_lod_budgets_decrease() {
        let lods = bamboo(73, 17.0);
        assert_eq!(lods.len(), 3);
        let mut previous = usize::MAX;
        for (bark, leaves) in lods {
            assert_buffer_valid(&bark, true);
            let leaves = leaves.expect("bamboo foliage");
            assert_buffer_valid(&leaves, false);
            let triangles = bark.triangle_count() + leaves.triangle_count();
            assert!(
                triangles < previous,
                "LOD triangle budgets must strictly decrease"
            );
            previous = triangles;
        }
    }

    #[test]
    fn bamboo_culm_anchors_and_leaf_attachments_are_lod_stable() {
        let seed = 73;
        let height = 17.0;
        for &culm in LOD_CULMS[2] {
            for node in 0..=NODES {
                let point = culm_node(seed, culm, height, node);
                assert_eq!(point, culm_point(seed, culm, height, node as f32));
            }
        }
        for lod in 1..3 {
            assert!(
                LOD_CULMS[lod]
                    .iter()
                    .all(|culm| LOD_CULMS[0].contains(culm))
            );
            assert!(
                LOD_LEAF_NODES[lod]
                    .iter()
                    .all(|node| LOD_LEAF_NODES[0].contains(node))
            );
        }
        assert!(keeps_branchlet(0, 0));
        assert!(keeps_branchlet(1, 0));
        assert!(keeps_branchlet(2, 0));
        assert!(!keeps_branchlet(1, 1));
        assert!(!keeps_branchlet(2, 1));
    }

    #[test]
    fn bamboo_near_canopy_has_twenty_to_thirty_five_spray_cards_per_culm() {
        for culm in 0..CULMS {
            assert!((20..=35).contains(&spray_cards_per_culm(73, culm, 0)));
        }
        assert!(spray_cards_per_culm(73, 0, 0) > spray_cards_per_culm(73, 0, 1));
        assert!(spray_cards_per_culm(73, 0, 1) > spray_cards_per_culm(73, 0, 2));
    }

    #[test]
    fn bamboo_culm_uvs_wrap_once_and_keep_global_node_v() {
        for lod in 0..3 {
            let profile = culm_profile(lod);
            assert_eq!(profile.first().map(|p| p.0), Some(0.0));
            assert!(profile.windows(2).all(|w| w[0].0 < w[1].0));
            for node in 1..=NODES {
                let collar = profile
                    .iter()
                    .find(|&&(v, _)| v == node as f32)
                    .expect("every canonical node has a collar");
                let lower_shoulder = profile
                    .iter()
                    .find(|&&(v, _)| (v - (node as f32 - 0.145)).abs() < f32::EPSILON)
                    .expect("every collar has a lower shoulder");
                assert!(collar.1 > lower_shoulder.1);
            }
            let mut bark = Buffers::default();
            let sides = [10, 7, 5][lod];
            culm_surface(&mut bark, 11, 0, 17.0, lod, sides);
            for ring in bark.uvs.chunks_exact(sides + 1) {
                assert_eq!(ring.first().map(|uv| uv[0]), Some(0.0));
                assert_eq!(ring.last().map(|uv| uv[0]), Some(1.0));
                assert!(ring.windows(2).all(|w| w[0][1] == w[1][1]));
            }
            assert!(bark.uvs.iter().any(|uv| uv[1] == NODES as f32));
        }
    }
}
