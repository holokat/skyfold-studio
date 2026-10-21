//! Prints triangle counts per default LOD level and the bounding box of every
//! bundled preset, plus generation timings.
//!
//! ```text
//! cargo run -p ez_tree --release --example stats
//! ```

use std::time::Instant;

use ez_tree::{Detail, PRESET_NAMES, TreeOptions};

fn main() {
    println!(
        "| preset | type | LOD0 tris (bark+leaf) | LOD1 tris | LOD2 tris | LOD0 verts | height | width x/z | grow ms | mesh LOD0 ms |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for name in PRESET_NAMES {
        let opts = TreeOptions::preset(name).expect("bundled preset");

        let t0 = Instant::now();
        let skeleton = ez_tree::grow(&opts);
        let grow_ms = t0.elapsed().as_secs_f64() * 1e3;

        let t1 = Instant::now();
        let full = ez_tree::mesh(&opts, &skeleton, &Detail::lod(0));
        let mesh_ms = t1.elapsed().as_secs_f64() * 1e3;

        let lods: Vec<_> = (1..Detail::LOD_COUNT)
            .map(|lod| ez_tree::mesh(&opts, &skeleton, &Detail::lod(lod)))
            .collect();

        let tris = |m: &ez_tree::TreeMesh| {
            format!(
                "{} ({}+{})",
                m.triangle_count(),
                m.branches.triangle_count(),
                m.leaves.triangle_count()
            )
        };
        println!(
            "| {name} | {:?} | {} | {} | {} | {} | {:.1} | {:.1} / {:.1} | {grow_ms:.2} | {mesh_ms:.2} |",
            opts.kind,
            tris(&full),
            tris(&lods[0]),
            tris(&lods[1]),
            full.vertex_count(),
            full.height(),
            full.max[0] - full.min[0],
            full.max[2] - full.min[2],
        );
    }
}
