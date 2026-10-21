//! Properties of the per-vertex wind sway weight (an extension, not in
//! ez-tree): zero at the trunk base, within [0, 1], increasing along every
//! branch and continuous across branch junctions.

use ez_tree::{Detail, PRESET_NAMES, Skeleton, TreeOptions};
use glam::DVec3;

fn skeletons() -> impl Iterator<Item = (&'static str, TreeOptions, Skeleton)> {
    PRESET_NAMES.iter().map(|&name| {
        let opts = TreeOptions::preset(name).unwrap();
        let skeleton = ez_tree::grow(&opts);
        (name, opts, skeleton)
    })
}

#[test]
fn trunk_base_is_still_and_weights_are_normalized() {
    for (name, opts, skeleton) in skeletons() {
        for lod in 0..Detail::LOD_COUNT {
            let mesh = ez_tree::mesh(&opts, &skeleton, &Detail::lod(lod));
            // The first ring of the trunk is the first `segments + 1` vertices.
            let ring =
                (opts.branch.segments[0] as f64 * Detail::lod(lod).segment_factor).round() as usize;
            for (i, &s) in mesh.branches.sway.iter().take(ring.max(3) + 1).enumerate() {
                assert_eq!(s, 0.0, "{name} LOD{lod}: trunk base vertex {i}");
            }
            for &s in mesh.branches.sway.iter().chain(&mesh.leaves.sway) {
                assert!((0.0..=1.0).contains(&s), "{name} LOD{lod}: sway {s}");
            }
        }
    }
}

#[test]
fn hierarchy_rises_along_branches() {
    for (name, opts, skeleton) in skeletons() {
        let span = 1.0 / (opts.branch.levels as f64 + 1.0);
        for (b, branch) in skeleton.branches.iter().enumerate() {
            let first = branch.sections[0].hierarchy;
            let last = branch.sections.last().unwrap().hierarchy;
            assert!((last - first - span).abs() < 1e-12, "{name} branch {b}");
            assert!(last <= 1.0 + 1e-12, "{name} branch {b}: p = {last}");
            for pair in branch.sections.windows(2) {
                assert!(pair[1].hierarchy > pair[0].hierarchy, "{name} branch {b}");
            }
        }
        assert_eq!(
            skeleton.branches[0].sections[0].hierarchy, 0.0,
            "{name}: trunk base"
        );
    }
}

/// Every non-trunk branch starts on a segment of some branch one level up;
/// its base `p` must equal the parent's `p` interpolated at that point.
#[test]
fn hierarchy_is_continuous_at_junctions() {
    for (name, _opts, skeleton) in skeletons() {
        for (b, child) in skeleton
            .branches
            .iter()
            .enumerate()
            .filter(|(_, br)| br.level > 0)
        {
            let base = child.sections[0];
            let mut found = false;
            'search: for parent in skeleton
                .branches
                .iter()
                .filter(|p| p.level + 1 == child.level)
            {
                for pair in parent.sections.windows(2) {
                    let (a, c) = (pair[0], pair[1]);
                    let ab = c.origin - a.origin;
                    let len2 = ab.length_squared();
                    let alpha = if len2 > 0.0 {
                        (base.origin - a.origin).dot(ab) / len2
                    } else {
                        0.0
                    };
                    if !(-1e-9..=1.0 + 1e-9).contains(&alpha) {
                        continue;
                    }
                    let on_segment: DVec3 = a.origin + ab * alpha;
                    if on_segment.distance(base.origin) > 1e-9 {
                        continue;
                    }
                    let expected = a.hierarchy + (c.hierarchy - a.hierarchy) * alpha;
                    assert!(
                        (base.hierarchy - expected).abs() < 1e-9,
                        "{name} branch {b}: base p {} vs parent p {expected}",
                        base.hierarchy
                    );
                    found = true;
                    break 'search;
                }
            }
            assert!(
                found,
                "{name} branch {b} (level {}): no parent segment found",
                child.level
            );
        }
    }
}

#[test]
fn leaf_sway_follows_attach_point_and_card_height() {
    for (name, opts, skeleton) in skeletons() {
        let mesh = ez_tree::mesh(&opts, &skeleton, &Detail::FULL);
        let per_leaf = 8; // double billboard: two 4-vertex cards
        assert_eq!(
            mesh.leaves.positions.len(),
            skeleton.leaves.len() * per_leaf,
            "{name}"
        );
        for (l, leaf) in skeleton.leaves.iter().enumerate() {
            let base = leaf.hierarchy.clamp(0.0, 1.0).powf(1.5);
            for v in l * per_leaf..(l + 1) * per_leaf {
                let expected = (base + 0.15 * f64::from(mesh.leaves.uvs[v][1])).clamp(0.0, 1.0);
                assert!(
                    (f64::from(mesh.leaves.sway[v]) - expected).abs() < 1e-6,
                    "{name} leaf {l}"
                );
            }
        }
    }
}
