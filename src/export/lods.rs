//! Levels of detail for the engine exports, built like the editor's own (`model_render::lod`).
//!
//! Packed models (Poly Haven, Skyborn and hand-painted buildings, user imports) arrive as one
//! detailed scene. Above `MIN_TRIS` triangles they get up to two simplified levels from the same
//! error-bounded, seam-aware simplifier the editor uses (a near level about 0.4 % of the model's
//! size off, a far one 6× that which lets seams go and drops loose bits), and each level takes
//! over where its error is about a pixel (`PX_K` × error). The file is reorganised like the
//! generated trees and scans: a variant node `<asset>_v0` with LOD children `<asset>_v0_LOD0…`;
//! LOD0 holds the original node tree untouched (animations and skins keep their nodes), the
//! simplified levels are single meshes with the model's own materials. Unreal (one static mesh per
//! glTF mesh) gets every level baked into one mesh.
//!
//! The terrain gets crack-free chunk LODs for Godot (`terrain_glb`): each chunk at 1, 2, 4… times
//! the sample spacing with a skirt hanging below its border, and a switch distance per level
//! from its measured height error.

use serde_json::{Value, json};

use super::core::{TerrainOut, VariantOut};
use super::glb::{self, FlatPart, Glb, Material, Prim};
use crate::model_render::simplify::simplify;

/// Models with fewer triangles keep their single level (as in the editor).
const MIN_TRIS: usize = 3000;
/// A level shows from `PX_K` × its error (m) on: about 1.3 px on a 2000 px tall 60° view.
pub const PX_K: f32 = 1500.0;
/// Terrain chunks switch at about 4 px of height error (like Unity's terrain pixel error): a
/// vertex's full vertical error is mostly seen at a slant, and models' 1.3 px would keep
/// mountainsides at full detail for kilometres.
const TERRAIN_PX_K: f32 = PX_K / 3.0;
/// A level must drop at least this share of the previous level's triangles to be kept.
const MIN_GAIN: f32 = 0.25;

/// The levels' errors (m): a near one about 0.4 % of the model's size, a far one 6× that.
pub fn errors(size: f32) -> [f32; 2] {
    let e = (0.004 * size).clamp(0.003, 0.06);
    [e, e * 6.0]
}

/// (min, max) corners of some parts.
pub fn bounds(parts: &[FlatPart]) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in parts.iter().flat_map(|p| &p.pos) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if lo[0] > hi[0] {
        return ([0.0; 3], [0.0; 3]);
    }
    (lo, hi)
}

/// Per vertex, a hash of everything but its position (what makes two corners one wedge).
fn wedges(p: &FlatPart) -> Vec<u64> {
    let mut h = vec![0xcbf2_9ce4_8422_2325u64; p.pos.len()];
    let mut mix = |i: usize, v: &[f32]| {
        for x in v {
            h[i] = (h[i] ^ x.to_bits() as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for i in 0..p.pos.len() {
        if let Some(n) = p.nrm.get(i) {
            mix(i, n);
        }
        if let Some(u) = p.uv.get(i) {
            mix(i, u);
        }
        if let Some(t) = p.tan.get(i) {
            mix(i, t);
        }
        if let Some(c) = p.color.get(i) {
            mix(i, c);
        }
    }
    h
}

/// The vertices of `src` that `tris` keep. The near level (`own_normals`) keeps everything of the
/// source vertices; the far level, which let seams and creases go, gets normals from its own
/// faces (smooth across faces within 50° of each other, split at sharper creases) and drops the
/// tangents (they would no longer match; engines rebuild them).
fn level_part(src: &FlatPart, tris: &[[u32; 3]], own_normals: bool) -> FlatPart {
    use bevy::math::Vec3;
    let pos = &src.pos;
    let mut out = FlatPart {
        pos: vec![],
        nrm: vec![],
        tan: vec![],
        uv: vec![],
        color: vec![],
        idx: Vec::with_capacity(tris.len() * 3),
        material: src.material,
    };
    let push = |out: &mut FlatPart, i: u32, n: Option<[f32; 3]>| -> u32 {
        let i = i as usize;
        out.pos.push(pos[i]);
        match n {
            Some(n) => out.nrm.push(n),
            None => {
                if let Some(v) = src.nrm.get(i) {
                    out.nrm.push(*v);
                }
                if let Some(v) = src.tan.get(i) {
                    out.tan.push(*v);
                }
            }
        }
        if let Some(v) = src.uv.get(i) {
            out.uv.push(*v);
        }
        if let Some(v) = src.color.get(i) {
            out.color.push(*v);
        }
        out.pos.len() as u32 - 1
    };
    if own_normals {
        let mut map: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for t in tris {
            for &i in t {
                let v = match map.get(&i) {
                    Some(v) => *v,
                    None => {
                        let v = push(&mut out, i, None);
                        map.insert(i, v);
                        v
                    }
                };
                out.idx.push(v);
            }
        }
        return out;
    }
    let fnorm: Vec<Vec3> = tris
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| Vec3::from_array(pos[i as usize]));
            (b - a).cross(c - a)
        })
        .collect();
    let mut around: std::collections::HashMap<[u32; 3], Vec<u32>> =
        std::collections::HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for &i in t {
            around
                .entry(pos[i as usize].map(f32::to_bits))
                .or_default()
                .push(ti as u32);
        }
    }
    let mut key: std::collections::HashMap<(u32, [i32; 3]), u32> = std::collections::HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        let own = fnorm[ti].normalize_or_zero();
        for &i in t {
            let mut n = Vec3::ZERO;
            for &tj in &around[&pos[i as usize].map(f32::to_bits)] {
                let f = fnorm[tj as usize];
                if own.dot(f.normalize_or_zero()) > 0.64 {
                    n += f;
                }
            }
            let n = n.normalize_or(own).to_array();
            let q = n.map(|x| (x * 1024.0).round() as i32);
            let v = match key.get(&(i, q)) {
                Some(v) => *v,
                None => {
                    let v = push(&mut out, i, Some(n));
                    key.insert((i, q), v);
                    v
                }
            };
            out.idx.push(v);
        }
    }
    out
}

/// Simplified levels of a model's parts (model units, metres): `(error, parts)` per level,
/// nearest first; empty for light models.
pub fn levels(parts: &[FlatPart], size: f32) -> Vec<(f32, Vec<FlatPart>)> {
    let total: usize = parts.iter().map(|p| p.idx.len() / 3).sum();
    if total < MIN_TRIS {
        return vec![];
    }
    let [e1, e2] = errors(size);
    let mut out = vec![];
    let mut prev = total;
    for (err, far) in [(e1, false), (e2, true)] {
        let lvl: Vec<FlatPart> = parts
            .iter()
            .map(|p| {
                let tris: Vec<[u32; 3]> =
                    p.idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
                let kept = if far {
                    // texture seams and hard edges are sub-pixel there; loose bits under the error go
                    simplify(&p.pos, &vec![0u64; p.pos.len()], &tris, err, err, None)
                } else {
                    // keeps every vertex whose normal its neighbours can't stand in for within 10°
                    let w = wedges(p);
                    let n = (p.nrm.len() == p.pos.len())
                        .then_some((&p.nrm[..], 10f32.to_radians().cos()));
                    simplify(&p.pos, &w, &tris, err, 0.0, n)
                };
                level_part(p, &kept, !far)
            })
            .filter(|p| !p.idx.is_empty())
            .collect();
        let tris: usize = lvl.iter().map(|p| p.idx.len() / 3).sum();
        if tris > 0 && (tris as f32) < prev as f32 * (1.0 - MIN_GAIN) {
            prev = tris;
            out.push((err, lvl));
        }
    }
    out
}

/// Where each level stops being drawn: where the next one starts (`PX_K` × its error; `errs`
/// are the simplified levels' errors), the last at `end`. Levels that would only start near or
/// beyond `end` are left out, so the result has one entry per level that shows.
pub fn lod_ends(errs: &[f32], end: f32) -> Vec<f32> {
    let mut ends = vec![];
    for err in errs {
        let start = PX_K * err;
        if start >= end * 0.9 {
            break;
        }
        ends.push(start);
    }
    ends.push(end);
    ends
}

/// A packed model, reorganised into one variant with levels of detail.
pub struct Packed {
    pub bytes: Vec<u8>,
    pub variant: VariantOut,
    /// where each level stops being drawn (m)
    pub lod_end: Vec<f32>,
}

/// `src`: the packed GLB; `end`: where the model stops being drawn; `flatten`: bake every level
/// into one mesh (Unreal). `None` when the file can't be read that way (skinned, compressed or
/// sparse data): it is then exported as it is.
pub fn packed(src: &[u8], safe: &str, end: f32, flatten: bool) -> Option<Packed> {
    let doc = glb::glb_json(src)?;
    if doc
        .get("skins")
        .and_then(|s| s.as_array())
        .is_some_and(|s| !s.is_empty())
    {
        return None;
    }
    let parts = glb::flat_parts(src, None)?;
    if parts.is_empty() {
        return None;
    }
    let (lo, hi) = bounds(&parts);
    let size = (hi[0] - lo[0])
        .max(hi[1] - lo[1])
        .max(hi[2] - lo[2])
        .max(0.05);
    let lv = levels(&parts, size);
    let errs: Vec<f32> = lv.iter().map(|l| l.0).collect();
    let lod_end = lod_ends(&errs, end);
    let shown = lod_end.len() - 1;
    let vname = format!("{safe}_v0");
    let mut g = if flatten {
        Glb::materials_of(src)?
    } else {
        Glb::open(src)?
    };
    let mut lods = vec![];
    let mut names = vec![];
    let lod0 = format!("{vname}_LOD0");
    if flatten {
        let prims: Vec<Prim> = parts.iter().map(FlatPart::prim).collect();
        let m = g.mesh(&lod0, &prims)?;
        lods.push(g.node(&lod0, Some(m), vec![]));
    } else {
        let scene = doc.get("scene").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
        let roots: Vec<usize> = doc
            .get("scenes")
            .and_then(|s| s.get(scene))
            .and_then(|s| s.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_u64().map(|v| v as usize))
                    .collect()
            })
            .unwrap_or_default();
        lods.push(g.node(&lod0, None, roots));
    }
    names.push(lod0);
    for (k, (_, lparts)) in lv.iter().enumerate().take(shown) {
        let name = format!("{vname}_LOD{}", k + 1);
        let prims: Vec<Prim> = lparts.iter().map(FlatPart::prim).collect();
        let Some(m) = g.mesh(&name, &prims) else {
            break;
        };
        lods.push(g.node(&name, Some(m), vec![]));
        names.push(name);
    }
    let mut lod_end = lod_end[..names.len() - 1].to_vec();
    lod_end.push(end);
    let root = g.node(&vname, None, lods);
    Some(Packed {
        bytes: g.finish(&[root]),
        variant: VariantOut {
            node: vname,
            lods: names,
            height: (hi[1] - lo[1]).max(0.05),
            width: (hi[0] - lo[0]).max(hi[2] - lo[2]).max(0.05),
            trunk: 0.0,
            collision: String::new(),
        },
        lod_end,
    })
}

// ---------------------------------------------------------------------------------------------
// terrain

/// Height of the coarse grid (step `s`, vertex (i0, j0) its corner) at full-resolution vertex
/// (i, j): the two triangles of each quad as the meshes split them (a–c–b, b–c–d).
fn coarse_height(t: &TerrainOut, s: usize, i: usize, j: usize) -> f32 {
    let n = t.n;
    let h = |i: usize, j: usize| t.heights[j.min(n - 1) * n + i.min(n - 1)];
    let (i0, j0) = ((i / s) * s, (j / s) * s);
    let (i0, j0) = (i0.min(n - 1 - s), j0.min(n - 1 - s));
    let (u, v) = ((i - i0) as f32 / s as f32, (j - j0) as f32 / s as f32);
    let (a, b, c, d) = (h(i0, j0), h(i0 + s, j0), h(i0, j0 + s), h(i0 + s, j0 + s));
    if u + v <= 1.0 {
        a + u * (b - a) + v * (c - a)
    } else {
        d + (1.0 - u) * (c - d) + (1.0 - v) * (b - d)
    }
}

/// The terrain (or the tile of `tn` samples a side from sample (i0, j0)) as chunk meshes with
/// levels of detail: node `chunk_XX_ZZ_LODk` per chunk and level (LOD0 every sample, LODk every
/// 2ᵏ-th; chunks numbered over the whole map), each with a skirt hanging below its border so
/// chunks at different levels never show cracks. Returns the GLB and the switch distances
/// (`{ "<node>": [begin, end] }`, measured to the chunk's centre; end 0 = no limit).
pub fn terrain_glb(
    t: &TerrainOut,
    i0: usize,
    j0: usize,
    tn: usize,
    chunks: usize,
) -> (Vec<u8>, Value) {
    let c = (tn - 1) / chunks;
    let (ci0, cj0) = (i0 / c, j0 / c);
    // levels: down to 4 quads per chunk side, at most 5
    let mut steps = vec![1usize];
    while steps.len() < 5 && c / (steps[steps.len() - 1] * 2) >= 4 {
        steps.push(steps[steps.len() - 1] * 2);
    }
    // the height error of every level of every chunk
    let err: Vec<Vec<f32>> = super::core::par_map(chunks * chunks, |k| {
        let (cx, cz) = (ci0 + k % chunks, cj0 + k / chunks);
        steps
            .iter()
            .map(|&s| {
                let mut e = 0f32;
                if s > 1 {
                    for j in cz * c..=(cz + 1) * c {
                        for i in cx * c..=(cx + 1) * c {
                            e = e.max((coarse_height(t, s, i, j) - t.heights[j * t.n + i]).abs());
                        }
                    }
                }
                e
            })
            .collect()
    });
    let reach = c as f32 * t.cell * 0.71;
    let mut g = Glb::new();
    let mat = g.material(&Material {
        name: "terrain".into(),
        base_color: [0.45, 0.47, 0.38, 1.0],
        roughness: 0.9,
        ..Material::new("terrain")
    });
    let mut nodes = vec![];
    let mut ranges = serde_json::Map::new();
    for lz in 0..chunks {
        for lx in 0..chunks {
            let e = &err[lz * chunks + lx];
            let (cx, cz) = (ci0 + lx, cj0 + lz);
            // deep enough to cover the gap to any neighbour at any level (a neighbouring tile's
            // chunks: twice this one's own coarsest error)
            let mut worst = e[steps.len() - 1] * 2.0;
            for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, nz) = (lx as i32 + dx, lz as i32 + dz);
                if nx >= 0 && nz >= 0 && (nx as usize) < chunks && (nz as usize) < chunks {
                    worst = worst.max(err[nz as usize * chunks + nx as usize][steps.len() - 1]);
                }
            }
            let skirt = worst * 2.0 + 0.5;
            let mut begin = 0.0f32;
            for (li, &s) in steps.iter().enumerate() {
                let q = c / s;
                let (mut pos, mut nrm, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
                for j in 0..=q {
                    for i in 0..=q {
                        let (gi, gj) = (cx * c + i * s, cz * c + j * s);
                        pos.push([
                            -t.half + gi as f32 * t.cell,
                            t.heights[gj * t.n + gi],
                            -t.half + gj as f32 * t.cell,
                        ]);
                        nrm.push(t.normal(gi, gj));
                        uv.push([gi as f32 / (t.n - 1) as f32, gj as f32 / (t.n - 1) as f32]);
                    }
                }
                let row = (q + 1) as u32;
                for j in 0..q {
                    for i in 0..q {
                        // cave mouths stay open up close; far levels only leave out quads that
                        // are open all over (the caves' own ground covers the rest)
                        let open = (0..s).all(|dj| {
                            (0..s).all(|di| t.hole(cx * c + i * s + di, cz * c + j * s + dj))
                        });
                        if (s == 1 && t.hole(cx * c + i, cz * c + j)) || (s > 1 && open) {
                            continue;
                        }
                        let (i, j) = (i as u32, j as u32);
                        let (a, b, cc, d) = (
                            j * row + i,
                            j * row + i + 1,
                            (j + 1) * row + i,
                            (j + 1) * row + i + 1,
                        );
                        idx.extend_from_slice(&[a, cc, b, b, cc, d]);
                    }
                }
                // the skirt: the border loop (+x, +z, −x, −z) again, lowered: walls facing out
                let border: Vec<u32> = (0..q as u32)
                    .chain((0..q as u32).map(|j| j * row + q as u32))
                    .chain((0..q as u32).map(|i| q as u32 * row + q as u32 - i))
                    .chain((0..q as u32).map(|j| (q as u32 - j) * row))
                    .collect();
                let base = pos.len() as u32;
                for &v in &border {
                    let p = pos[v as usize];
                    pos.push([p[0], p[1] - skirt, p[2]]);
                    nrm.push(nrm[v as usize]);
                    uv.push(uv[v as usize]);
                }
                let m = border.len() as u32;
                for k in 0..m {
                    let (a, b) = (border[k as usize], border[((k + 1) % m) as usize]);
                    let (la, lb) = (base + k, base + (k + 1) % m);
                    idx.extend_from_slice(&[a, b, la, b, lb, la]);
                }
                let name = format!("chunk_{cx:02}_{cz:02}_LOD{li}");
                let end = if li + 1 < steps.len() {
                    (TERRAIN_PX_K * e[li + 1] + reach).max(begin + c as f32 * t.cell * 0.5)
                } else {
                    0.0
                };
                if let Some(m) = g.mesh(
                    &name,
                    &[Prim {
                        positions: &pos,
                        normals: Some(&nrm),
                        uvs: Some(&uv),
                        indices: &idx,
                        material: Some(mat),
                        ..Default::default()
                    }],
                ) {
                    nodes.push(g.node(&name, Some(m), vec![]));
                    ranges.insert(
                        name,
                        json!([(begin * 10.0).round() / 10.0, (end * 10.0).round() / 10.0]),
                    );
                }
                if end == 0.0 {
                    break;
                }
                begin = end;
            }
        }
    }
    let root = g.node("Terrain", None, nodes);
    (
        g.finish(&[root]),
        json!({ "levels": steps.len(), "chunks": chunks, "ranges": ranges }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bumpy grid of `n`² quads (enough triangles for levels of detail).
    fn bumpy(n: usize) -> FlatPart {
        let mut p = FlatPart {
            pos: vec![],
            nrm: vec![],
            tan: vec![],
            uv: vec![],
            color: vec![],
            idx: vec![],
            material: Some(0),
        };
        for j in 0..=n {
            for i in 0..=n {
                let (x, z) = (i as f32 / n as f32 * 4.0, j as f32 / n as f32 * 4.0);
                p.pos
                    .push([x, (x * 0.7).sin() * 0.3 + (z * 0.5).cos() * 0.2, z]);
                p.nrm.push([0.0, 1.0, 0.0]);
                p.uv.push([x, z]);
            }
        }
        let r = (n + 1) as u32;
        for j in 0..n as u32 {
            for i in 0..n as u32 {
                p.idx.extend_from_slice(&[
                    j * r + i,
                    (j + 1) * r + i,
                    j * r + i + 1,
                    j * r + i + 1,
                    (j + 1) * r + i,
                    (j + 1) * r + i + 1,
                ]);
            }
        }
        p
    }

    #[test]
    fn heavy_models_get_lighter_levels() {
        let parts = vec![bumpy(60)];
        let lv = levels(&parts, 4.0);
        assert!(!lv.is_empty());
        let tris = |l: &[FlatPart]| l.iter().map(|p| p.idx.len() / 3).sum::<usize>();
        let mut prev = tris(&parts);
        for (_, l) in &lv {
            assert!(tris(l) * 4 <= prev * 3, "{} vs {prev}", tris(l));
            prev = tris(l);
        }
        // light models keep one level
        assert!(levels(&[bumpy(10)], 4.0).is_empty());
        let ends = lod_ends(&[0.016, 0.096], 200.0);
        assert_eq!(ends.len(), 3);
        assert!(ends[0] < ends[1] && ends[1] < ends[2]);
        // a level that would only start beyond the end is dropped
        assert_eq!(lod_ends(&[0.016, 0.096], 90.0).len(), 2);
    }

    #[test]
    fn packed_models_keep_their_tree_under_lod0() {
        let part = bumpy(60);
        let mut g = Glb::new();
        let mat = g.material(&Material::new("m"));
        let m = g
            .mesh(
                "ground",
                &[Prim {
                    material: Some(mat),
                    ..part.prim()
                }],
            )
            .unwrap();
        let n = g.node("ground", Some(m), vec![]);
        let src = g.finish(&[n]);
        let p = packed(&src, "ph_x", 200.0, false).unwrap();
        assert_eq!(p.variant.node, "ph_x_v0");
        assert!(p.variant.lods.len() >= 2, "{:?}", p.variant.lods);
        assert_eq!(p.lod_end.len(), p.variant.lods.len());
        let doc = glb::glb_json(&p.bytes).unwrap();
        let nodes = doc["nodes"].as_array().unwrap();
        let lod0 = nodes.iter().find(|n| n["name"] == "ph_x_v0_LOD0").unwrap();
        assert_eq!(
            nodes[lod0["children"][0].as_u64().unwrap() as usize]["name"],
            "ground"
        );
        // the simplified levels are lighter, and the whole thing still flattens
        assert!(glb::triangles(&p.bytes) > glb::triangles(&src));
        assert!(glb::flatten(&p.bytes, "x").is_some());
        // Unreal: every level is one mesh
        let u = packed(&src, "ph_x", 200.0, true).unwrap();
        let doc = glb::glb_json(&u.bytes).unwrap();
        assert_eq!(
            doc["meshes"].as_array().unwrap().len(),
            u.variant.lods.len()
        );
    }

    #[test]
    fn terrain_chunks_have_levels_and_skirts() {
        let n = 129;
        let mut heights = vec![0.0; n * n];
        for j in 0..n {
            for i in 0..n {
                heights[j * n + i] =
                    ((i as f32) * 0.3).sin() * 4.0 + ((j as f32) * 0.2).cos() * 3.0;
            }
        }
        let t = TerrainOut {
            n,
            size: 512.0,
            cell: 4.0,
            half: 256.0,
            lo: -7.0,
            hi: 7.0,
            sea: 0.0,
            heights,
            used: vec![0],
            weights: vec![vec![255; n * n]],
            holes: vec![],
        };
        let (glb, ranges) = terrain_glb(&t, 0, 0, t.n, 4);
        let doc = glb::glb_json(&glb).unwrap();
        // 4 × 4 chunks of 32 quads: 4 levels each (32, 16, 8, 4 quads)
        assert_eq!(ranges["levels"], 4);
        assert_eq!(doc["meshes"].as_array().unwrap().len(), 16 * 4);
        let r = &ranges["ranges"];
        let (b1, e0) = (
            r["chunk_00_00_LOD1"][0].as_f64().unwrap(),
            r["chunk_00_00_LOD0"][1].as_f64().unwrap(),
        );
        assert_eq!(b1, e0);
        assert_eq!(r["chunk_00_00_LOD3"][1], 0.0);
    }
}
