//! Parity tests against the original JavaScript ez-tree.
//!
//! `tests/fixtures/*.json` were produced by `tools/gen_fixtures.mjs`, which
//! runs ez-tree's unmodified `tree.js` (three.js 0.179.1) under Node and
//! records, per preset and per default LOD level, vertex/index counts,
//! bounding boxes, buffer sums, order-sensitive checksums and the first and
//! last 64 vertices/indices of the bark and leaf geometry.
//!
//! Known, intentional differences that the comparison accounts for:
//! - Bark `v` coordinates: the port bakes in `1 / textureScale.y`.
//! - Indices: ez-tree stores them in a `Uint16Array`, so the reference holds
//!   them modulo 65536.
//!
//! Set `EZ_TREE_JS_DUMP=<dir>` (the `--full` output of the generator) to also
//! compare every component of every buffer.

use std::path::{Path, PathBuf};

use ez_tree::{Buffers, Detail, PRESET_NAMES, TreeOptions};
use serde_json::Value;

/// Absolute tolerances for per-vertex attributes. In practice every position
/// is bit-identical to the JS output; the tolerances only leave room for
/// last-bit differences between V8's and the platform's `sin`/`cos`/`atan2`/
/// `asin`, which show up as ~1e-16 noise in near-zero normal components.
const POS_TOL: f64 = 1e-3;
const NORMAL_TOL: f64 = 1e-5;
const UV_TOL: f64 = 1e-6;
/// Relative tolerance for whole-buffer sums and checksums.
const SUM_REL_TOL: f64 = 1e-7;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn load_json(file: &str) -> Value {
    let path = fixtures_dir().join(file);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn num(v: &Value) -> f64 {
    v.as_f64()
        .unwrap_or_else(|| panic!("expected a number, got {v}"))
}

fn count(v: &Value) -> usize {
    v.as_u64()
        .unwrap_or_else(|| panic!("expected a count, got {v}")) as usize
}

/// Comparison statistics, for reporting.
#[derive(Default, Debug)]
struct Stats {
    /// Per-vertex components compared.
    components: usize,
    /// ... of which were not bit-identical (after rounding the fixture's
    /// 9-digit decimals back to f32).
    not_bit_identical: usize,
    max_position_diff: f64,
    max_normal_diff: f64,
    max_uv_diff: f64,
    max_sum_rel_diff: f64,
}

impl Stats {
    /// Compares one f32 component against the fixture value.
    fn component(
        &mut self,
        what: &dyn Fn() -> String,
        actual: f32,
        expected: f64,
        tol: f64,
    ) -> f64 {
        let expected = expected as f32; // fixtures hold f32 values as 9 significant digits
        self.components += 1;
        if actual.to_bits() != expected.to_bits() {
            self.not_bit_identical += 1;
        }
        let diff = f64::from((actual - expected).abs());
        assert!(
            diff <= tol,
            "{}: rust {actual} vs js {expected} (diff {diff:e})",
            what()
        );
        diff
    }

    fn sum(&mut self, what: &str, actual: f64, expected: f64) {
        let rel = (actual - expected).abs() / expected.abs().max(1.0);
        assert!(
            rel <= SUM_REL_TOL,
            "{what}: rust {actual} vs js {expected} (rel diff {rel:e})"
        );
        self.max_sum_rel_diff = self.max_sum_rel_diff.max(rel);
    }
}

/// Mirrors `summarize()` in gen_fixtures.mjs, over the Rust buffers.
#[derive(Debug)]
struct Summary {
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
    pos_sum: [f64; 3],
    normal_sum: [f64; 3],
    uv_sum: [f64; 2],
    pos_weighted: f64,
    index_sum: f64,
    index_weighted: f64,
}

fn summarize(b: &Buffers) -> Summary {
    let mut s = Summary {
        bbox_min: [f64::INFINITY; 3],
        bbox_max: [f64::NEG_INFINITY; 3],
        pos_sum: [0.0; 3],
        normal_sum: [0.0; 3],
        uv_sum: [0.0; 2],
        pos_weighted: 0.0,
        index_sum: 0.0,
        index_weighted: 0.0,
    };
    for (i, p) in b.positions.iter().enumerate() {
        let p = p.map(f64::from);
        for (c, &x) in p.iter().enumerate() {
            s.bbox_min[c] = s.bbox_min[c].min(x);
            s.bbox_max[c] = s.bbox_max[c].max(x);
            s.pos_sum[c] += x;
            s.normal_sum[c] += f64::from(b.normals[i][c]);
        }
        s.uv_sum[0] += f64::from(b.uvs[i][0]);
        s.uv_sum[1] += f64::from(b.uvs[i][1]);
        s.pos_weighted += ((i % 97) + 1) as f64 * (p[0] + 2.0 * p[1] + 3.0 * p[2]);
    }
    for (i, &idx) in b.indices.iter().enumerate() {
        let idx = f64::from(idx & 0xffff); // JS Uint16Array semantics
        s.index_sum += idx;
        s.index_weighted += ((i % 89) + 1) as f64 * idx;
    }
    if b.positions.is_empty() {
        s.bbox_min = [0.0; 3];
        s.bbox_max = [0.0; 3];
    }
    s
}

/// Compares one mesh (bark or leaves) with its fixture summary. `v_scale`
/// is the bark `textureScale.y` baked into the port's `v` (1 for leaves).
fn compare_buffers(stats: &mut Stats, ctx: &str, b: &Buffers, js: &Value, v_scale: f64) {
    let n = b.positions.len();
    assert_eq!(n, count(&js["vertexCount"]), "{ctx}: vertex count");
    assert_eq!(
        b.indices.len(),
        count(&js["indexCount"]),
        "{ctx}: index count"
    );
    assert_eq!(b.normals.len(), n, "{ctx}: normal count");
    assert_eq!(b.uvs.len(), n, "{ctx}: uv count");
    assert_eq!(b.sway.len(), n, "{ctx}: sway count");

    // Explicit vertices and indices from both ends of the buffers.
    for part in ["head", "tail"] {
        let rows = &js[part];
        let start = count(&rows["start"]);
        for (k, row) in rows["positions"].as_array().unwrap().iter().enumerate() {
            let i = start + k;
            for c in 0..3 {
                let what = || format!("{ctx} v{i} position[{c}]");
                let d = stats.component(&what, b.positions[i][c], num(&row[c]), POS_TOL);
                stats.max_position_diff = stats.max_position_diff.max(d);
                let what = || format!("{ctx} v{i} normal[{c}]");
                let d = stats.component(
                    &what,
                    b.normals[i][c],
                    num(&rows["normals"][k][c]),
                    NORMAL_TOL,
                );
                stats.max_normal_diff = stats.max_normal_diff.max(d);
            }
            let uv = &rows["uvs"][k];
            let d = stats.component(
                &|| format!("{ctx} v{i} u"),
                b.uvs[i][0],
                num(&uv[0]),
                UV_TOL,
            );
            stats.max_uv_diff = stats.max_uv_diff.max(d);
            let expected_v = num(&uv[1]) / v_scale;
            let d = stats.component(&|| format!("{ctx} v{i} v"), b.uvs[i][1], expected_v, UV_TOL);
            stats.max_uv_diff = stats.max_uv_diff.max(d);
        }
        let index_start = if part == "head" {
            0
        } else {
            count(&rows["indexStart"])
        };
        for (k, idx) in rows["indices"].as_array().unwrap().iter().enumerate() {
            let i = index_start + k;
            assert_eq!(
                b.indices[i] & 0xffff,
                count(idx) as u32,
                "{ctx}: index {i} (mod 65536)"
            );
        }
    }

    // Whole-buffer aggregates (f64 sums of the f32 values, in buffer order).
    let s = summarize(b);
    for c in 0..3 {
        stats.sum(
            &format!("{ctx} bbox min[{c}]"),
            s.bbox_min[c],
            num(&js["bboxMin"][c]),
        );
        stats.sum(
            &format!("{ctx} bbox max[{c}]"),
            s.bbox_max[c],
            num(&js["bboxMax"][c]),
        );
        stats.sum(
            &format!("{ctx} position sum[{c}]"),
            s.pos_sum[c],
            num(&js["posSum"][c]),
        );
        stats.sum(
            &format!("{ctx} normal sum[{c}]"),
            s.normal_sum[c],
            num(&js["normalSum"][c]),
        );
    }
    stats.sum(&format!("{ctx} u sum"), s.uv_sum[0], num(&js["uvSum"][0]));
    // JS bark v is 0 or 1; the port stores f32(v / textureScale.y).
    let baked_one = f64::from((1.0 / v_scale) as f32);
    stats.sum(
        &format!("{ctx} v sum"),
        s.uv_sum[1],
        num(&js["uvSum"][1]) * baked_one,
    );
    stats.sum(
        &format!("{ctx} weighted position checksum"),
        s.pos_weighted,
        num(&js["posWeighted"]),
    );
    assert_eq!(
        s.index_sum,
        num(&js["indexSum"]),
        "{ctx}: index sum (mod 65536 values)"
    );
    assert_eq!(
        s.index_weighted,
        num(&js["indexWeighted"]),
        "{ctx}: weighted index checksum"
    );
}

#[test]
fn presets_match_js_reference() {
    let mut stats = Stats::default();
    for name in PRESET_NAMES {
        let fixture = load_json(&format!("{name}.json"));
        let opts = TreeOptions::preset(name).unwrap();
        let v_scale = opts.bark.texture_scale.y;
        assert_eq!(
            num(&fixture["textureScaleY"]),
            v_scale,
            "{name}: textureScale.y"
        );

        let skeleton = ez_tree::grow(&opts);
        assert_eq!(
            skeleton.branches.len(),
            count(&fixture["skeleton"]["branches"]),
            "{name}: branches"
        );
        assert_eq!(
            skeleton.leaves.len(),
            count(&fixture["skeleton"]["leaves"]),
            "{name}: leaves"
        );

        let lods = fixture["lods"].as_array().unwrap();
        assert_eq!(lods.len(), Detail::LOD_COUNT, "{name}: LOD count");
        for (lod, js) in lods.iter().enumerate() {
            let mesh = ez_tree::mesh(&opts, &skeleton, &Detail::lod(lod));
            let ctx = format!("{name} LOD{lod}");
            compare_buffers(
                &mut stats,
                &format!("{ctx} branches"),
                &mesh.branches,
                &js["branches"],
                v_scale,
            );
            compare_buffers(
                &mut stats,
                &format!("{ctx} leaves"),
                &mesh.leaves,
                &js["leaves"],
                1.0,
            );
        }
    }
    println!("{stats:#?}");
}

#[test]
fn rng_matches_js() {
    for case in load_json("rng.json").as_array().unwrap() {
        let seed = case["seed"].as_i64().unwrap();
        let mut rng = ez_tree::Rng::new(seed);
        for expected in case["values"].as_array().unwrap() {
            assert_eq!(rng.next_f64(), num(expected), "seed {seed}");
        }
        for expected in case["scaled"].as_array().unwrap() {
            assert_eq!(
                rng.random(0.3, -0.3),
                num(expected),
                "seed {seed}: random(0.3, -0.3)"
            );
        }
    }
}

/// Exhaustive comparison against complete buffers dumped by
/// `gen_fixtures.mjs --full <dir>`. Skipped unless `EZ_TREE_JS_DUMP` is set.
#[test]
fn full_buffers_match_js_dump() {
    let Some(dir) = std::env::var_os("EZ_TREE_JS_DUMP") else {
        eprintln!("EZ_TREE_JS_DUMP not set; skipping the full-buffer comparison");
        return;
    };
    let dir = PathBuf::from(dir);
    let read = |path: PathBuf| -> Vec<u8> {
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    };
    let read_f32 = |path| -> Vec<f32> {
        read(path)
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    };
    let read_u16 = |path| -> Vec<u16> {
        read(path)
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
            .collect()
    };

    let mut stats = Stats::default();
    let mut positions_not_bit_identical = 0;
    for name in PRESET_NAMES {
        let opts = TreeOptions::preset(name).unwrap();
        let skeleton = ez_tree::grow(&opts);
        for lod in 0..Detail::LOD_COUNT {
            let mesh = ez_tree::mesh(&opts, &skeleton, &Detail::lod(lod));
            for (part, b, v_scale) in [
                ("branches", &mesh.branches, opts.bark.texture_scale.y),
                ("leaves", &mesh.leaves, 1.0),
            ] {
                let file = |suffix: &str| dir.join(format!("{name}_lod{lod}_{part}_{suffix}.bin"));
                let (pos, nrm, uv, idx) = (
                    read_f32(file("pos")),
                    read_f32(file("nrm")),
                    read_f32(file("uv")),
                    read_u16(file("idx")),
                );
                let ctx = format!("{name} LOD{lod} {part}");
                assert_eq!(pos.len(), b.positions.len() * 3, "{ctx}: vertex count");
                assert_eq!(idx.len(), b.indices.len(), "{ctx}: index count");
                for (i, (&js, &rs)) in idx.iter().zip(&b.indices).enumerate() {
                    assert_eq!(u32::from(js), rs & 0xffff, "{ctx}: index {i}");
                }
                for (i, (&js, &rs)) in pos.iter().zip(b.positions.iter().flatten()).enumerate() {
                    if js.to_bits() != rs.to_bits() {
                        positions_not_bit_identical += 1;
                    }
                    let d = stats.component(
                        &|| format!("{ctx} position component {i}"),
                        rs,
                        js.into(),
                        POS_TOL,
                    );
                    stats.max_position_diff = stats.max_position_diff.max(d);
                }
                for (i, (&js, &rs)) in nrm.iter().zip(b.normals.iter().flatten()).enumerate() {
                    let d = stats.component(
                        &|| format!("{ctx} normal component {i}"),
                        rs,
                        js.into(),
                        NORMAL_TOL,
                    );
                    stats.max_normal_diff = stats.max_normal_diff.max(d);
                }
                for (i, (js, rs)) in uv.chunks_exact(2).zip(&b.uvs).enumerate() {
                    let d =
                        stats.component(&|| format!("{ctx} u {i}"), rs[0], js[0].into(), UV_TOL);
                    stats.max_uv_diff = stats.max_uv_diff.max(d);
                    let expected_v = f64::from(js[1]) / v_scale;
                    let d = stats.component(&|| format!("{ctx} v {i}"), rs[1], expected_v, UV_TOL);
                    stats.max_uv_diff = stats.max_uv_diff.max(d);
                }
            }
        }
    }
    println!("{stats:#?}\npositions not bit-identical: {positions_not_bit_identical}");
}
