//! Properties of the per-vertex bark frame (an extension, not in ez-tree): one entry per bark
//! vertex, in range, and the same distances along the branches at every level of detail.

use ez_tree::{Detail, PRESET_NAMES, TreeOptions};

#[test]
fn bark_frame_covers_bark_vertices_only() {
    for name in PRESET_NAMES {
        let opts = TreeOptions::preset(name).unwrap();
        let skeleton = ez_tree::grow(&opts);
        let mut longest = None;
        for lod in 0..Detail::LOD_COUNT {
            let mesh = ez_tree::mesh(&opts, &skeleton, &Detail::lod(lod));
            assert_eq!(
                mesh.branches.bark.len(),
                mesh.branches.vertex_count(),
                "{name} LOD{lod}"
            );
            assert!(mesh.leaves.bark.is_empty(), "{name} LOD{lod}: leaves");
            for &[around, along, girth] in &mesh.branches.bark {
                assert!(
                    (0.0..=1.0).contains(&around) && along >= 0.0 && girth > 0.0,
                    "{name} LOD{lod}"
                );
            }
            // every branch keeps its last ring, so the longest distance can't depend on the LOD
            let max = mesh
                .branches
                .bark
                .iter()
                .map(|b| b[1])
                .fold(0.0f32, f32::max);
            let first = *longest.get_or_insert(max);
            assert!(
                (max - first).abs() <= first * 1e-6,
                "{name} LOD{lod}: {max} vs {first}"
            );
        }
    }
}
