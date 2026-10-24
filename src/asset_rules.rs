//! The automatic asset gate, editor side. Its rules live in `tools/asset_rules.json`, the single
//! source of truth the Python gate (`tools/asset_gate.py`: releases, pack scans, the walk test)
//! reads too; they are embedded here because `tools/` doesn't ship with the app.
//!
//! The editor runs the cheap checks, straight from a glTF / GLB: triangles (drawn, instances
//! counted), triangle density over the surface, materials, draw primitives, own textures, size,
//! origin, unused materials and images, collider size. A model's kind (small prop, prop, large
//! piece, building, scene, nature) comes from its facts exactly as the gate derives it, so the same
//! budgets apply. Three users:
//!
//! - File › Import: a very heavy model gets a friendly one-time card (never a block) saying what
//!   it costs ("This model has 480k triangles…"); the check runs on a background thread.
//! - The `objects_check_model` MCP tool: the same report as JSON, for a library id or a file.
//! - `cargo test`: every engine pack's `models.json` against the triangle and material budgets,
//!   straight from the manifests; legacy violations come from `tools/asset_gate_baseline.json`
//!   and may not get worse.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use bevy::math::{DMat3, DMat4, DQuat, DVec3};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};

/// The rules, as shipped (`tools/asset_rules.json`).
pub const RULES_JSON: &str = include_str!("../tools/asset_rules.json");

pub static RULES: LazyLock<Rules> =
    LazyLock::new(|| serde_json::from_str(RULES_JSON).expect("tools/asset_rules.json parses"));

#[derive(Deserialize)]
pub struct Rules {
    pub version: u32,
    pub walk_in_groups: Vec<String>,
    pub kinds: Vec<Kind>,
    pub rules: Vec<Rule>,
}

/// A kind of model and the facts that make one (any of `when`, first kind matching wins).
#[derive(Deserialize)]
pub struct Kind {
    pub id: String,
    pub name: String,
    pub when: Vec<Map<String, Value>>,
}

#[derive(Deserialize)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub metric: String,
    #[serde(default)]
    pub unit: String,
    pub why: String,
    /// "min": a value below the limit breaks it (default: above)
    #[serde(default)]
    pub compare: Option<String>,
    /// per kind id (or "default"): warn / fail thresholds
    pub limits: HashMap<String, Option<Limit>>,
    #[serde(default)]
    pub only_when: Vec<Map<String, Value>>,
    #[serde(default)]
    pub skip_when: Vec<Map<String, Value>>,
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Limit {
    #[serde(default)]
    pub warn: Option<f64>,
    #[serde(default)]
    pub fail: Option<f64>,
}

/// What the kinds and rules look at, worked out the same way as `asset_gate.py facts_of`.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub pack: String,
    pub category: String,
    pub group: String,
    pub walkable: bool,
    pub walk_in: bool,
    pub composite: bool,
    /// ships and pier pieces sit on the water (`LibraryEntry::floats`)
    pub floats: bool,
    pub format: String,
    pub max_dim: f64,
    pub height: f64,
    pub footprint: f64,
}

impl Facts {
    /// Facts from a model's manifest fields and size (glTF Y-up metres: [x, height, z]).
    /// `walkable`: the manifest's flag, else everything but props (as `objects::model_library`).
    pub fn new(
        pack: &str,
        category: &str,
        group: &str,
        walkable: Option<bool>,
        composite: bool,
        format: &str,
        size: [f64; 3],
    ) -> Self {
        let walkable = walkable.unwrap_or(category != "props");
        Facts {
            pack: pack.into(),
            category: category.into(),
            group: group.into(),
            walkable,
            walk_in: walkable && RULES.walk_in_groups.iter().any(|g| g == group),
            composite,
            floats: group == "ships" && category != "props",
            format: format.into(),
            max_dim: size[0].max(size[1]).max(size[2]),
            height: size[1],
            footprint: size[0] * size[2],
        }
    }

    fn get(&self, k: &str) -> Value {
        match k {
            "pack" => json!(self.pack),
            "category" => json!(self.category),
            "group" => json!(self.group),
            "walkable" => json!(self.walkable),
            "walk_in" => json!(self.walk_in),
            "composite" => json!(self.composite),
            "floats" => json!(self.floats),
            "format" => json!(self.format),
            "max_dim" => json!(self.max_dim),
            "height" => json!(self.height),
            "footprint" => json!(self.footprint),
            _ => Value::Null,
        }
    }

    /// A condition: {fact: value} (equal), {fact: [values]} (one of), {fact: {"ge": x, "lt": y}}.
    fn matches(&self, when: &Map<String, Value>) -> bool {
        when.iter().all(|(k, v)| {
            let x = self.get(k);
            match v {
                Value::Array(a) => a.contains(&x),
                Value::Object(o) => {
                    let n = x.as_f64().unwrap_or(0.0);
                    o.get("ge").and_then(Value::as_f64).is_none_or(|g| n >= g)
                        && o.get("lt").and_then(Value::as_f64).is_none_or(|l| n < l)
                }
                _ => x == *v,
            }
        })
    }

    /// The model's kind (`asset_rules.json` kinds, first match).
    pub fn kind(&self) -> &'static Kind {
        let kinds = &RULES.kinds;
        kinds
            .iter()
            .find(|k| k.when.iter().any(|w| self.matches(w)))
            .unwrap_or(&kinds[kinds.len() - 1])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Fail,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub rule: &'static str,
    pub name: &'static str,
    pub severity: Severity,
    pub value: f64,
    pub limit: f64,
    /// the rule wants at least `limit` (else at most)
    pub at_least: bool,
    pub unit: &'static str,
    pub why: &'static str,
}

impl Finding {
    pub fn to_json(&self) -> Value {
        json!({
            "rule": self.rule, "name": self.name,
            "severity": if self.severity == Severity::Fail { "fail" } else { "warn" },
            "value": round3(self.value), "limit": self.limit, "compare": if self.at_least { "min" } else { "max" },
            "unit": self.unit, "why": self.why,
        })
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Every rule the metrics break (rules whose metric isn't given are skipped).
pub fn evaluate(metrics: &HashMap<&str, f64>, kind: &str, facts: &Facts) -> Vec<Finding> {
    let mut out = vec![];
    for r in &RULES.rules {
        let Some(&v) = metrics.get(r.metric.as_str()) else {
            continue;
        };
        if !r.only_when.is_empty() && !r.only_when.iter().any(|w| facts.matches(w)) {
            continue;
        }
        if r.skip_when.iter().any(|w| facts.matches(w)) {
            continue;
        }
        let Some(Some(lim)) = r.limits.get(kind).or_else(|| r.limits.get("default")) else {
            continue;
        };
        let at_least = r.compare.as_deref() == Some("min");
        for (sev, x) in [(Severity::Fail, lim.fail), (Severity::Warn, lim.warn)] {
            let Some(x) = x else { continue };
            if if at_least { v < x } else { v > x } {
                out.push(Finding {
                    rule: &r.id,
                    name: &r.name,
                    severity: sev,
                    value: v,
                    limit: x,
                    at_least,
                    unit: &r.unit,
                    why: &r.why,
                });
                break;
            }
        }
    }
    out
}

// ------------------------------------------------------------------ reading a glTF cheaply

/// What the cheap checks measure in a glTF / GLB.
#[derive(Clone, Debug, Default)]
pub struct ModelStats {
    /// triangles drawn (every instance)
    pub tris: u64,
    /// triangles stored (each mesh once)
    pub unique_tris: u64,
    pub draw_prims: u64,
    /// materials drawn / declared
    pub materials: usize,
    pub materials_declared: usize,
    pub meshes: usize,
    pub mesh_nodes: usize,
    pub vertices: u64,
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// m² of surface (drawn)
    pub area: f64,
    /// triangles the walker collides with (door leaves and chest lids left out)
    pub collider_tris: u64,
    pub unused_materials: usize,
    pub unused_images: usize,
    pub missing_textures: usize,
    /// pixel sizes of the textures used (files found)
    pub texture_px: Vec<(u32, u32)>,
}

impl ModelStats {
    pub fn size(&self) -> [f64; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// A scene placed from pieces (several meshes, some drawn more than once), when no
    /// manifest says.
    pub fn looks_composite(&self) -> bool {
        self.meshes >= 3 && self.tris as f64 > 1.15 * self.unique_tris as f64
    }

    /// The metrics the rules compare (names as in `asset_gate.py metrics_of`); every texture
    /// counts as the model's own (nothing else shares it here).
    pub fn metrics(&self, walkable: bool) -> HashMap<&'static str, f64> {
        let size = self.size();
        let c = [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ];
        let half = [(size[0] * 0.5).max(1e-6), (size[2] * 0.5).max(1e-6)];
        let off = [c[0].abs() / half[0], c[1].abs() / half[1]];
        // a wall piece pivots on its back face, a hanging one at its top: exempt
        let wall = off[0].max(off[1]) >= 0.85 && off[0].min(off[1]) <= 0.4;
        let hanging =
            self.max[1].abs() <= (0.05 * size[1]).max(0.05) && self.min[1] < -0.5 * size[1];
        let px: Vec<u32> = self.texture_px.iter().map(|p| p.0.max(p.1)).collect();
        let mut m = HashMap::from([
            ("tris", self.tris as f64),
            ("unique_tris", self.unique_tris as f64),
            ("materials", self.materials as f64),
            ("draw_prims", self.draw_prims as f64),
            ("tri_density", self.tris as f64 / self.area.max(1e-6)),
            ("unique_textures", self.texture_px.len() as f64),
            (
                "unique_texture_px",
                px.iter().copied().max().unwrap_or(0) as f64,
            ),
            (
                "unique_texture_mpix",
                self.texture_px
                    .iter()
                    .map(|p| p.0 as f64 * p.1 as f64)
                    .sum::<f64>()
                    / 1e6,
            ),
            ("missing_textures", self.missing_textures as f64),
            (
                "origin_offset",
                if wall || hanging {
                    0.0
                } else {
                    off[0].max(off[1])
                },
            ),
            (
                "float_m",
                if wall || hanging {
                    0.0
                } else {
                    self.min[1].max(0.0)
                },
            ),
            (
                "sunk_share",
                if wall || hanging {
                    0.0
                } else {
                    (-self.min[1]).max(0.0) / size[1].max(1e-6)
                },
            ),
            ("max_dim", size[0].max(size[1]).max(size[2])),
            (
                "verts_per_tri",
                self.vertices as f64 / self.unique_tris.max(1) as f64,
            ),
            ("unused_materials", self.unused_materials as f64),
            ("unused_images", self.unused_images as f64),
        ]);
        if walkable {
            m.insert("collider_tris", self.collider_tris as f64);
        }
        m
    }
}

/// Reads a .gltf / .glb (and its buffers) and measures it.
pub fn read_model(path: &Path) -> Result<ModelStats, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let (doc, glb_bin): (Value, Option<Vec<u8>>) = if bytes.starts_with(b"glTF") {
        let mut off = 12;
        let (mut doc, mut bin) = (None, None);
        while off + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            let kind = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap());
            let chunk = bytes.get(off + 8..off + 8 + len).ok_or("truncated GLB")?;
            match kind {
                0x4E4F534A => {
                    doc = Some(serde_json::from_slice(chunk).map_err(|e| format!("GLB JSON: {e}"))?)
                }
                0x004E4942 => bin = Some(chunk.to_vec()),
                _ => {}
            }
            off += 8 + len;
        }
        (doc.ok_or("GLB without JSON")?, bin)
    } else {
        (
            serde_json::from_slice(&bytes).map_err(|e| format!("glTF JSON: {e}"))?,
            None,
        )
    };
    let arr = |k: &str| {
        doc.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let (accessors, views, buffers_j, meshes_j, nodes, materials, textures, images) = (
        arr("accessors"),
        arr("bufferViews"),
        arr("buffers"),
        arr("meshes"),
        arr("nodes"),
        arr("materials"),
        arr("textures"),
        arr("images"),
    );
    let mut buffers: Vec<Option<Vec<u8>>> = vec![None; buffers_j.len()];
    let mut buffer = |i: usize| -> Result<Vec<u8>, String> {
        if buffers.get(i).is_some_and(Option::is_some) {
            return Ok(buffers[i].clone().unwrap_or_default());
        }
        let b = buffers_j.get(i).ok_or("buffer index out of range")?;
        let data = match b.get("uri").and_then(Value::as_str) {
            None => glb_bin.clone().unwrap_or_default(),
            Some(u) if u.starts_with("data:") => {
                base64_decode(u.split_once(',').map(|x| x.1).unwrap_or(""))
            }
            Some(u) => {
                std::fs::read(base.join(unescape(u))).map_err(|e| format!("buffer {u}: {e}"))?
            }
        };
        if let Some(slot) = buffers.get_mut(i) {
            *slot = Some(data.clone());
        }
        Ok(data)
    };
    // an accessor as rows of f64 (positions) or as indices
    let mut read = |ai: usize| -> Result<(Vec<f64>, usize), String> {
        let a = accessors.get(ai).ok_or("accessor index out of range")?;
        let count = a.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
        let ncomp = match a.get("type").and_then(Value::as_str) {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            _ => return Err("unsupported accessor type".into()),
        };
        let ct = a
            .get("componentType")
            .and_then(Value::as_u64)
            .unwrap_or(5126);
        let csize = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            _ => 4,
        };
        let normalized = a
            .get("normalized")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut out = vec![0.0f64; count * ncomp];
        let Some(vi) = a.get("bufferView").and_then(Value::as_u64) else {
            return Ok((out, ncomp));
        };
        let v = views.get(vi as usize).ok_or("bufferView out of range")?;
        let buf = buffer(v.get("buffer").and_then(Value::as_u64).unwrap_or(0) as usize)?;
        let start = v.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize
            + a.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let stride = v
            .get("byteStride")
            .and_then(Value::as_u64)
            .map(|s| s as usize)
            .unwrap_or(csize * ncomp);
        for i in 0..count {
            for c in 0..ncomp {
                let o = start + i * stride + c * csize;
                let b = buf.get(o..o + csize).ok_or("accessor past its buffer")?;
                let x = match ct {
                    5126 => f32::from_le_bytes(b.try_into().unwrap()) as f64,
                    5125 => u32::from_le_bytes(b.try_into().unwrap()) as f64,
                    5123 => {
                        u16::from_le_bytes(b.try_into().unwrap()) as f64
                            / if normalized { 65535.0 } else { 1.0 }
                    }
                    5122 => (i16::from_le_bytes(b.try_into().unwrap()) as f64
                        / if normalized { 32767.0 } else { 1.0 })
                    .max(-1.0),
                    5121 => b[0] as f64 / if normalized { 255.0 } else { 1.0 },
                    _ => (b[0] as i8 as f64 / if normalized { 127.0 } else { 1.0 }).max(-1.0),
                };
                out[i * ncomp + c] = x;
            }
        }
        Ok((out, ncomp))
    };
    // the meshes drawn by the default scene, with their world matrices
    let roots: Vec<usize> = match doc
        .get("scenes")
        .and_then(Value::as_array)
        .and_then(|s| s.get(doc.get("scene").and_then(Value::as_u64).unwrap_or(0) as usize))
    {
        Some(s) => s
            .get("nodes")
            .and_then(Value::as_array)
            .map(|n| {
                n.iter()
                    .filter_map(|x| x.as_u64().map(|x| x as usize))
                    .collect()
            })
            .unwrap_or_default(),
        None => {
            let kids: HashSet<u64> = nodes
                .iter()
                .flat_map(|n| {
                    n.get("children")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|c| c.as_u64())
                .collect();
            (0..nodes.len())
                .filter(|i| !kids.contains(&(*i as u64)))
                .collect()
        }
    };
    let mut inst: Vec<(usize, DMat4, bool)> = vec![];
    let mut stack: Vec<(usize, DMat4, bool)> = roots
        .into_iter()
        .map(|r| (r, DMat4::IDENTITY, false))
        .collect();
    while let Some((i, parent, moving)) = stack.pop() {
        if inst.len() > 200_000 || stack.len() > 200_000 {
            break;
        }
        let Some(n) = nodes.get(i) else { continue };
        let m = parent * node_matrix(n);
        let ex = n.get("extras");
        let moving = moving
            || ex.is_some_and(|x| {
                x.get("door").is_some() || x.get("lid").is_some() || x.get("flap").is_some()
            });
        if n.get("name").and_then(Value::as_str) == Some("collision") {
            continue;
        }
        if let Some(mi) = n.get("mesh").and_then(Value::as_u64) {
            inst.push((mi as usize, m, moving));
        }
        for c in n
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
        {
            stack.push((c as usize, m, moving));
        }
    }
    // each mesh once: its primitives' triangles (local), materials and vertex counts
    struct Prim {
        mat: Option<u64>,
        pos: Vec<f64>,
        tris: Vec<[u32; 3]>,
    }
    let mut cache: HashMap<usize, Vec<Prim>> = HashMap::new();
    let mut st = ModelStats {
        min: [f64::MAX; 3],
        max: [f64::MIN; 3],
        materials_declared: materials.len(),
        ..default()
    };
    let mut used_mats: HashSet<Option<u64>> = HashSet::new();
    for (mi, m, moving) in &inst {
        if !cache.contains_key(mi) {
            let mut prims = vec![];
            for p in meshes_j
                .get(*mi)
                .and_then(|x| x.get("primitives"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(pa) = p
                    .get("attributes")
                    .and_then(|a| a.get("POSITION"))
                    .and_then(Value::as_u64)
                else {
                    continue;
                };
                let (pos, nc) = read(pa as usize)?;
                if nc != 3 {
                    continue;
                }
                let nv = pos.len() / 3;
                let idx: Vec<u32> = match p.get("indices").and_then(Value::as_u64) {
                    Some(ii) => read(ii as usize)?.0.into_iter().map(|x| x as u32).collect(),
                    None => (0..nv as u32).collect(),
                };
                let tris: Vec<[u32; 3]> = match p.get("mode").and_then(Value::as_u64).unwrap_or(4) {
                    4 => idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect(),
                    5 => idx
                        .windows(3)
                        .enumerate()
                        .map(|(k, t)| {
                            if k % 2 == 0 {
                                [t[0], t[1], t[2]]
                            } else {
                                [t[1], t[0], t[2]]
                            }
                        })
                        .collect(),
                    6 if idx.len() >= 3 => {
                        idx[1..].windows(2).map(|t| [idx[0], t[0], t[1]]).collect()
                    }
                    _ => vec![],
                };
                let tris: Vec<[u32; 3]> = tris
                    .into_iter()
                    .filter(|t| t.iter().all(|&i| (i as usize) < nv))
                    .collect();
                st.unique_tris += tris.len() as u64;
                st.vertices += nv as u64;
                prims.push(Prim {
                    mat: p.get("material").and_then(Value::as_u64),
                    pos,
                    tris,
                });
            }
            cache.insert(*mi, prims);
        }
        st.mesh_nodes += 1;
        let a = DMat3::from_mat4(*m);
        let det = a.determinant();
        let cof = if det.abs() > 1e-12 {
            a.inverse().transpose() * det
        } else {
            DMat3::ZERO
        };
        for p in &cache[mi] {
            if p.tris.is_empty() {
                continue;
            }
            st.tris += p.tris.len() as u64;
            st.draw_prims += 1;
            used_mats.insert(p.mat);
            if !moving {
                st.collider_tris += p.tris.len() as u64;
            }
            let v = |i: u32| {
                DVec3::new(
                    p.pos[i as usize * 3],
                    p.pos[i as usize * 3 + 1],
                    p.pos[i as usize * 3 + 2],
                )
            };
            for t in &p.tris {
                let (a0, b0, c0) = (v(t[0]), v(t[1]), v(t[2]));
                st.area += (cof * (b0 - a0).cross(c0 - a0)).length() * 0.5;
            }
            for k in 0..p.pos.len() / 3 {
                let w = m.transform_point3(v(k as u32));
                for c in 0..3 {
                    st.min[c] = st.min[c].min(w[c]);
                    st.max[c] = st.max[c].max(w[c]);
                }
            }
        }
    }
    if st.min[0] > st.max[0] {
        st.min = [0.0; 3];
        st.max = [0.0; 3];
    }
    st.meshes = cache.len();
    st.materials = used_mats.len();
    st.unused_materials = (0..materials.len() as u64)
        .filter(|i| !used_mats.contains(&Some(*i)))
        .count();
    // images the used materials sample (core and extension texture slots)
    let mut used_imgs: HashSet<u64> = HashSet::new();
    for mi in used_mats.iter().flatten() {
        let mut refs = vec![];
        texture_refs(
            materials.get(*mi as usize).unwrap_or(&Value::Null),
            "",
            &mut refs,
        );
        for ti in refs {
            let t = textures.get(ti as usize);
            let src = t
                .and_then(|t| t.get("source"))
                .and_then(Value::as_u64)
                .or_else(|| {
                    t.and_then(|t| t.get("extensions"))
                        .and_then(Value::as_object)
                        .and_then(|e| {
                            e.values()
                                .find_map(|x| x.get("source").and_then(Value::as_u64))
                        })
                });
            if let Some(s) = src {
                used_imgs.insert(s);
            }
        }
    }
    st.unused_images = (0..images.len() as u64)
        .filter(|i| !used_imgs.contains(i))
        .count();
    for i in &used_imgs {
        let Some(uri) = images
            .get(*i as usize)
            .and_then(|im| im.get("uri"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if uri.starts_with("data:") {
            continue;
        }
        let p = base.join(unescape(uri));
        if !p.is_file() {
            st.missing_textures += 1;
        } else if let Ok(d) = image::image_dimensions(&p) {
            st.texture_px.push(d);
        }
    }
    Ok(st)
}

fn node_matrix(n: &Value) -> DMat4 {
    let f = |k: &str| {
        n.get(k).and_then(Value::as_array).map(|a| {
            a.iter()
                .map(|x| x.as_f64().unwrap_or(0.0))
                .collect::<Vec<f64>>()
        })
    };
    if let Some(m) = f("matrix").filter(|m| m.len() == 16) {
        return DMat4::from_cols_slice(&m);
    }
    let t = f("translation")
        .filter(|v| v.len() == 3)
        .unwrap_or(vec![0.0; 3]);
    let r = f("rotation")
        .filter(|v| v.len() == 4)
        .unwrap_or(vec![0.0, 0.0, 0.0, 1.0]);
    let s = f("scale").filter(|v| v.len() == 3).unwrap_or(vec![1.0; 3]);
    DMat4::from_scale_rotation_translation(
        DVec3::new(s[0], s[1], s[2]),
        DQuat::from_xyzw(r[0], r[1], r[2], r[3]).normalize(),
        DVec3::new(t[0], t[1], t[2]),
    )
}

/// Texture indices a material samples: any `…Texture` object with an `index`.
fn texture_refs(v: &Value, key: &str, out: &mut Vec<u64>) {
    match v {
        Value::Object(o) => {
            if key.ends_with("Texture")
                && let Some(i) = o.get("index").and_then(Value::as_u64)
            {
                out.push(i);
            }
            for (k, x) in o {
                texture_refs(x, k, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| texture_refs(x, key, out)),
        _ => {}
    }
}

/// A URI's %-escapes undone (file names with spaces).
fn unescape(u: &str) -> String {
    let b = u.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(x) = u8::from_str_radix(&u[i + 1..i + 3], 16)
        {
            out.push(x);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64_decode(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for v in s.bytes().filter_map(val) {
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

// ------------------------------------------------------------------ reports

/// A model's check: its kind, measured numbers and every rule it breaks.
pub struct Report {
    pub file: PathBuf,
    pub kind: &'static Kind,
    pub facts: Facts,
    pub stats: ModelStats,
    pub metrics: HashMap<&'static str, f64>,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn fails(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Fail)
    }

    /// Too heavy for its kind: the triangle, density or collider budget fails.
    pub fn heavy(&self) -> bool {
        self.fails()
            .any(|f| matches!(f.rule, "tris" | "unique_tris" | "tri_density" | "collider"))
    }

    pub fn to_json(&self) -> Value {
        let size = self.stats.size();
        let verdict = if self.fails().next().is_some() {
            "fail"
        } else if self.findings.is_empty() {
            "pass"
        } else {
            "warn"
        };
        let mut metrics: Vec<(&&str, &f64)> = self.metrics.iter().collect();
        metrics.sort_by(|a, b| a.0.cmp(b.0));
        json!({
            "file": self.file.display().to_string(),
            "kind": self.kind.id, "kind_name": self.kind.name,
            "verdict": verdict,
            "size_m": size.map(round3),
            "walkable": self.facts.walkable, "walk_in": self.facts.walk_in, "scene": self.facts.composite,
            "materials_declared": self.stats.materials_declared,
            "metrics": metrics.into_iter().map(|(k, v)| (k.to_string(), json!(round3(*v)))).collect::<Map<String, Value>>(),
            "findings": self.findings.iter().map(Finding::to_json).collect::<Vec<_>>(),
            "rules_version": RULES.version,
            "note": "The cheap checks. Releases and pack imports also run the walk test, welding, degenerate-triangle and instancing checks (tools/asset_gate.py).",
        })
    }
}

/// Measures and checks a model file; facts not given come from the file itself.
pub fn check_file(
    path: &Path,
    pack: &str,
    category: &str,
    group: &str,
    walkable: Option<bool>,
    scene: Option<bool>,
) -> Result<Report, String> {
    let stats = read_model(path)?;
    let size = stats.size();
    let format = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("glb"))
    {
        "glb"
    } else {
        "gltf"
    };
    let facts = Facts::new(
        pack,
        category,
        group,
        walkable,
        scene.unwrap_or_else(|| stats.looks_composite()),
        format,
        size,
    );
    let kind = facts.kind();
    let metrics = stats.metrics(facts.walkable);
    let findings = evaluate(&metrics, &kind.id, &facts);
    Ok(Report {
        file: path.to_path_buf(),
        kind,
        facts,
        stats,
        metrics,
        findings,
    })
}

// ------------------------------------------------------------------ the import card

/// Heavy imports waiting for their card: (card id, title, lines).
static CARDS: Mutex<Vec<(String, String, Vec<String>)>> = Mutex::new(vec![]);

/// File › Import: checks the copied model in the background and queues a one-time card when
/// it's very heavy (imports are walkable, as `io::imported_entry` makes them).
pub fn check_import(path: PathBuf, name: String) {
    std::thread::spawn(move || {
        let r = match check_file(&path, "imported", "", "", Some(true), None) {
            Ok(r) => r,
            Err(e) => return warn!("asset check of {}: {e}", path.display()),
        };
        info!(
            "asset check {}: {} ({} tris, {} findings)",
            path.display(),
            r.kind.id,
            r.stats.tris,
            r.findings.len()
        );
        if !r.heavy() {
            return;
        }
        let id = format!(
            "heavy_model:{}",
            path.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        if crate::ui::tips::seen(&id) {
            return;
        }
        let tris = r.stats.tris;
        let budget = r
            .findings
            .iter()
            .find(|f| f.rule == "tris")
            .map(|f| f.limit);
        let mut lines = vec![format!(
            "{name} has {} triangles. Skyfold simplifies it at a distance, but it may slow walking inside it and every scene it's in.",
            short(tris as f64)
        )];
        if let Some(b) = budget {
            lines.push(format!(
                "{} models like it usually stay under {}.",
                r.kind.name,
                short(b)
            ));
        }
        lines.push(
            "For a lighter copy, decimate it in Blender (Decimate modifier) and import it again."
                .into(),
        );
        CARDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id, "A heavy model".into(), lines));
    });
}

fn short(n: f64) -> String {
    if n >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{n:.0}")
    }
}

/// The card of the first heavy import not yet dismissed (bottom right, over the viewport).
fn heavy_card(mut contexts: EguiContexts) {
    let first = {
        let mut q = CARDS.lock().unwrap_or_else(|e| e.into_inner());
        q.retain(|c| !crate::ui::tips::seen(&c.0));
        q.first().cloned()
    };
    let Some((id, title, lines)) = first else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else { return };
    // (the editor's fonts arrive with its first UI frame: a card checked at startup waits for them)
    if !ctx.fonts(|f| f.families().contains(&crate::ui::theme::semibold())) {
        return;
    }
    egui::Window::new("heavy_model_card")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -120.0))
        .default_width(340.0)
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            ui.set_width(340.0);
            let steps: Vec<&str> = lines.iter().map(String::as_str).collect();
            crate::ui::tips::card(ui, &id, &title, &steps);
        });
}

// ------------------------------------------------------------------ the MCP tool

pub struct AssetRulesPlugin;

impl Plugin for AssetRulesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            heavy_card.run_if(crate::mcp::ui_visible),
        )
        .add_systems(Startup, test_hook);
        mcp(app);
    }
}

/// `WB_HEAVY_CARD=<model.gltf>` (tests): check a model as if it had just been imported.
fn test_hook() {
    if let Ok(p) = std::env::var("WB_HEAVY_CARD") {
        let p = PathBuf::from(p);
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().replace(['_', '-'], " "))
            .unwrap_or_default();
        check_import(p, name);
    }
}

/// The asset rules' MCP tool.
pub fn mcp(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "objects_check_model",
        description: "Check a 3D model against the asset rules (the budgets every bundled model is held to): it measures triangles drawn, triangle density, materials, draw primitives, textures, size, origin, unused materials and images and the collider, works out the model's kind (small prop, prop, large piece, building, scene, nature) and returns the report as JSON with each number and every warn or fail with its limit and why. Give `asset` (an objects_library id of a glTF model: bundled or imported) or `path` (a .gltf / .glb file, absolute or under the assets folder). Read-only.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "asset": { "type": "string", "description": "library asset id (a glTF model)" },
                    "path": { "type": "string", "description": "a .gltf or .glb file: absolute, or relative to the assets folder" }
                }
            })
        },
        run: check_tool,
    });
}

fn check_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let report = match (
        opt_arg::<String>(&args, "asset")?,
        opt_arg::<String>(&args, "path")?,
    ) {
        (Some(id), _) => {
            let lib = world
                .get_resource::<crate::objects::Library>()
                .ok_or("the library isn't loaded yet")?;
            let e = lib
                .get(&id)
                .ok_or_else(|| format!("no asset `{id}` in the library (see objects_library)"))?;
            crate::market::check(&id)?;
            let rel = e.import_path.as_deref().ok_or_else(|| {
                format!(
                    "`{id}` is built into the editor (procedural or photoscanned), not a glTF model"
                )
            })?;
            let rel = rel.split('#').next().unwrap_or(rel);
            // models/<folder>/…: the pack is the folder (stylepacks/<pack>/… its pack)
            let mut parts = rel.split('/').skip(1);
            let pack = match parts.next() {
                Some(f @ ("stylepacks" | crate::objects::PREVIEWS)) => parts.next().unwrap_or(f),
                Some(f) => f,
                None => "imported",
            };
            let category = match e.category {
                crate::objects::Category::Props => "props",
                crate::objects::Category::Imported => "",
                _ => "structures",
            };
            check_file(
                &crate::asset_file(rel),
                if rel.starts_with("models/") {
                    pack
                } else {
                    "imported"
                },
                category,
                &e.group,
                Some(e.walkable),
                Some(e.whole_scene),
            )?
        }
        (None, Some(p)) => {
            let p = PathBuf::from(p);
            let p = if p.is_absolute() {
                p
            } else {
                crate::asset_file(&p.to_string_lossy())
            };
            if !p
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("gltf") || e.eq_ignore_ascii_case("glb"))
            {
                return Err("`path` must be a .gltf or .glb file".into());
            }
            check_file(&p, "imported", "", "", None, None)?
        }
        _ => {
            return Err(
                "give `asset` (an objects_library id) or `path` (a .gltf / .glb file)".into(),
            );
        }
    };
    Ok(vec![McpContent::Json(report.to_json())])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rules parse, every kind and limit names a real kind, and the kinds come out of the
    /// facts like the Python gate's.
    #[test]
    fn rules_parse_and_kinds_derive() {
        let r = &*RULES;
        assert!(r.rules.len() >= 20 && r.kinds.len() >= 6);
        let ids: HashSet<&str> = r.kinds.iter().map(|k| k.id.as_str()).collect();
        for rule in &r.rules {
            for k in rule.limits.keys() {
                assert!(
                    k == "default" || ids.contains(k.as_str()),
                    "{}: unknown kind {k}",
                    rule.id
                );
            }
        }
        let f = |cat: &str, group: &str, walkable: Option<bool>, scene: bool, size: [f64; 3]| {
            Facts::new("handpainted", cat, group, walkable, scene, "gltf", size)
                .kind()
                .id
                .clone()
        };
        assert_eq!(
            f(
                "structures",
                "encounters",
                Some(true),
                true,
                [30.0, 9.0, 26.0]
            ),
            "scene"
        );
        assert_eq!(
            f(
                "structures",
                "buildings",
                Some(true),
                false,
                [11.0, 12.0, 11.0]
            ),
            "building"
        );
        assert_eq!(
            f(
                "structures",
                "military",
                Some(false),
                false,
                [10.0, 5.0, 3.0]
            ),
            "large_piece"
        );
        assert_eq!(
            f("props", "furniture", None, false, [1.6, 1.0, 2.2]),
            "prop"
        );
        assert_eq!(
            f("props", "lighting", None, false, [0.12, 0.34, 0.39]),
            "small_prop"
        );
        assert_eq!(
            Facts::new("photoscan", "", "", None, false, "lods", [2.0, 1.7, 3.1])
                .kind()
                .id,
            "nature"
        );
    }

    /// The 1.6 m, 209k-triangle bed fails its budgets; a hand-painted prop passes.
    #[test]
    fn budgets_catch_heavy_models() {
        let facts = Facts::new(
            "skyborn",
            "props",
            "furniture",
            None,
            false,
            "gltf",
            [1.62, 1.06, 2.17],
        );
        let kind = &facts.kind().id;
        let m = HashMap::from([
            ("tris", 209_196.0),
            ("materials", 1.0),
            ("tri_density", 209_196.0 / 37.1),
        ]);
        let f = evaluate(&m, kind, &facts);
        assert!(
            f.iter()
                .any(|f| f.rule == "tris" && f.severity == Severity::Fail),
            "{f:?}"
        );
        assert!(
            f.iter()
                .any(|f| f.rule == "tri_density" && f.severity == Severity::Fail),
            "{f:?}"
        );
        let m = HashMap::from([
            ("tris", 1_610.0),
            ("materials", 3.0),
            ("tri_density", 160.0),
        ]);
        assert!(evaluate(&m, kind, &facts).is_empty());
    }

    /// Reads a bundled model the way the import check does.
    #[test]
    fn reads_a_gltf() {
        let p = crate::asset_file(
            "models/stylepacks/handpainted/orc_bloodtusk_pit/orc_bloodtusk_pit.gltf",
        );
        if !p.is_file() {
            return;
        }
        let r = check_file(
            &p,
            "handpainted",
            "structures",
            "encounters",
            Some(true),
            None,
        )
        .unwrap();
        assert_eq!(r.kind.id, "scene", "{:?}", r.facts);
        assert!(
            r.stats.tris > r.stats.unique_tris && r.stats.draw_prims > r.stats.materials as u64
        );
        assert!(
            r.metrics["tri_density"] > 5.0 && r.metrics["tri_density"] < 60.0,
            "{}",
            r.metrics["tri_density"]
        );
        assert!(r.stats.min[1].abs() < 0.2 && r.stats.size()[0] > 20.0);
    }

    /// Every model in the engine packs' manifests (`assets/models/*/models.json`, style packs
    /// included) against the triangle and material budgets of its kind, straight from the
    /// manifest. Legacy violations are in tools/asset_gate_baseline.json and may not get worse;
    /// a new violation fails.
    #[test]
    fn asset_budgets() {
        let root = crate::paths::project_dir();
        let baseline: Value = std::fs::read_to_string(root.join("tools/asset_gate_baseline.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({}));
        let legacy = |key: &str, rule: &str| {
            baseline
                .get("assets")
                .and_then(|a| a.get(key))
                .and_then(|e| e.get(rule))
                .and_then(Value::as_f64)
        };
        let mut manifests: Vec<PathBuf> = vec![];
        for dir in [
            root.join("assets/models"),
            root.join("assets/models/stylepacks"),
        ] {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path().join("models.json");
                if p.is_file() {
                    manifests.push(p);
                }
            }
        }
        manifests.sort();
        let (mut checked, mut problems) = (0, vec![]);
        for mf in &manifests {
            let folder = mf
                .parent()
                .and_then(|p| p.file_name())
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Ok(doc) = serde_json::from_str::<Value>(&std::fs::read_to_string(mf).unwrap())
            else {
                problems.push(format!("{}: doesn't parse", mf.display()));
                continue;
            };
            let pack = doc
                .get("pack")
                .and_then(Value::as_str)
                .unwrap_or(&folder)
                .to_string();
            for m in doc
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                let n = |k: &str| m.get(k).and_then(Value::as_f64);
                let id = s("id");
                let size: Vec<f64> = m
                    .get("size_m")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_f64).collect())
                    .unwrap_or_default();
                if size.len() != 3 {
                    problems.push(format!("{pack}:{id}: no size_m"));
                    continue;
                }
                let scene = m
                    .get("scene")
                    .and_then(Value::as_bool)
                    .unwrap_or(m.get("unique_tris").is_some_and(|u| !u.is_null()));
                let format = if s("gltf").ends_with(".glb") {
                    "glb"
                } else {
                    "gltf"
                };
                let facts = Facts::new(
                    &pack,
                    &s("category"),
                    &s("group"),
                    m.get("walkable").and_then(Value::as_bool),
                    scene,
                    format,
                    [size[0], size[1], size[2]],
                );
                let kind = facts.kind();
                let mut metrics = HashMap::new();
                if let Some(t) = n("polycount_tris") {
                    metrics.insert("tris", t);
                    metrics.insert("unique_tris", n("unique_tris").unwrap_or(t));
                }
                if let Some(x) = n("materials") {
                    metrics.insert("materials", x);
                }
                checked += 1;
                let key = format!("{pack}:{id}");
                for f in evaluate(&metrics, &kind.id, &facts)
                    .into_iter()
                    .filter(|f| f.severity == Severity::Fail)
                {
                    match legacy(&key, f.rule) {
                        // (the baseline records the glTF's measured value: allow a manifest's rounding)
                        Some(old) if f.value <= old * 1.001 + 1e-9 => {}
                        Some(old) => problems.push(format!(
                            "{key} [{}]: {} {} is worse than its baseline {old} (limit {})",
                            kind.id, f.rule, f.value, f.limit
                        )),
                        None => problems.push(format!(
                            "{key} [{}]: {} {} {} over the {} limit {} — {}",
                            kind.id, f.rule, f.value, f.unit, kind.name, f.limit, f.why
                        )),
                    }
                }
            }
        }
        assert!(
            checked > 100,
            "only {checked} models found in {} manifests",
            manifests.len()
        );
        assert!(
            problems.is_empty(),
            "{} model(s) break the asset budgets (tools/asset_rules.json; see docs/asset-rules.md):\n{}",
            problems.len(),
            problems.join("\n")
        );
    }
}
