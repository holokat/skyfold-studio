//! Binary glTF 2.0 (GLB) writer for a [`Model`] and its clips.
//!
//! Layout: a root node "Character" holds the skeleton's root bones and one node per mesh.
//! Skinned meshes use the single skin (joints = every bone, in order); clips become LINEAR
//! animations sampled at frame times `i / fps`.

use crate::model::*;
use glam::Vec3;
use serde_json::{json, Value};

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;

/// Accumulates the binary chunk, its buffer views and accessors.
#[derive(Default)]
struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Bin {
    /// Appends `bytes` 4-byte aligned as a new buffer view; returns its index.
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let mut v = json!({ "buffer": 0, "byteOffset": self.data.len(), "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.data.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(&mut self, bytes: &[u8], target: Option<u32>, ctype: u32, count: usize, ty: &str) -> usize {
        let view = self.view(bytes, target);
        self.accessors.push(json!({ "bufferView": view, "componentType": ctype, "count": count, "type": ty }));
        self.accessors.len() - 1
    }

    fn floats(&mut self, v: &[f32], target: Option<u32>, ty: &str, width: usize) -> usize {
        self.accessor(&f32_bytes(v), target, FLOAT, v.len() / width, ty)
    }
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn vec3s(v: &[Vec3]) -> Vec<f32> {
    v.iter().flat_map(|p| p.to_array()).collect()
}

/// Encodes `model` (meshes, materials, images, skeleton) and `clips` as GLB bytes.
pub fn write(model: &Model, clips: &[Clip]) -> Vec<u8> {
    let mut bin = Bin::default();
    let bones: &[Bone] = model.skeleton.as_ref().map(|s| &s.bones[..]).unwrap_or(&[]);
    let nb = bones.len();

    // Nodes: 0 = Character, 1..=nb = bones, then meshes.
    let mut nodes: Vec<Value> = vec![json!({ "name": "Character" })];
    let mut root_children: Vec<usize> = Vec::new();
    let mut bone_children: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (i, b) in bones.iter().enumerate() {
        match b.parent {
            Some(p) if p < i => bone_children[p].push(i + 1),
            _ => root_children.push(i + 1),
        }
    }
    for (i, b) in bones.iter().enumerate() {
        let r = b.rest.r.normalize();
        let mut n = json!({
            // (Bevy only animates nodes whose whole path is named)
            "name": if b.name.is_empty() { format!("bone{i}") } else { b.name.clone() },
            "translation": b.rest.t.to_array(),
            "rotation": r.to_array(),
            "scale": b.rest.s.to_array(),
        });
        if !bone_children[i].is_empty() {
            n["children"] = json!(bone_children[i]);
        }
        nodes.push(n);
    }

    // Images, textures (one per image), materials.
    let images: Vec<Value> = model
        .images
        .iter()
        .map(|img| {
            let view = bin.view(&img.bytes, None);
            json!({ "name": img.name, "bufferView": view, "mimeType": img.mime })
        })
        .collect();
    let textures: Vec<Value> = (0..model.images.len()).map(|i| json!({ "sampler": 0, "source": i })).collect();
    let tex = |t: &Option<TexRef>| t.filter(|t| t.image < model.images.len()).map(|t| json!({ "index": t.image }));
    let mut emissive_strength = false;
    let materials: Vec<Value> = model
        .materials
        .iter()
        .map(|m| {
            let mut pbr = json!({
                "baseColorFactor": m.base_color,
                "metallicFactor": m.metallic,
                "roughnessFactor": m.roughness,
            });
            if let Some(t) = tex(&m.base_color_tex) {
                pbr["baseColorTexture"] = t;
            }
            if let Some(t) = tex(&m.metallic_roughness_tex) {
                pbr["metallicRoughnessTexture"] = t;
            }
            let mut v = json!({ "name": m.name, "pbrMetallicRoughness": pbr });
            if let Some(t) = tex(&m.normal_tex) {
                v["normalTexture"] = t;
            }
            if let Some(t) = tex(&m.occlusion_tex) {
                v["occlusionTexture"] = t;
            }
            if let Some(t) = tex(&m.emissive_tex) {
                v["emissiveTexture"] = t;
            }
            // emissiveFactor is clamped to [0, 1]; brighter goes in KHR_materials_emissive_strength.
            let peak = m.emissive.iter().copied().fold(0.0f32, f32::max);
            if peak > 1.0 {
                v["emissiveFactor"] = json!(m.emissive.map(|c| c / peak));
                v["extensions"] = json!({ "KHR_materials_emissive_strength": { "emissiveStrength": peak } });
                emissive_strength = true;
            } else if peak > 0.0 {
                v["emissiveFactor"] = json!(m.emissive);
            }
            match m.alpha {
                AlphaMode::Opaque => {}
                AlphaMode::Mask => {
                    v["alphaMode"] = json!("MASK");
                    v["alphaCutoff"] = json!(m.alpha_cutoff);
                }
                AlphaMode::Blend => v["alphaMode"] = json!("BLEND"),
            }
            if m.double_sided {
                v["doubleSided"] = json!(true);
            }
            v
        })
        .collect();

    // Meshes.
    let mut meshes: Vec<Value> = Vec::new();
    let mut any_skinned = false;
    for m in &model.meshes {
        let n = m.positions.len();
        if n == 0 || m.indices.len() < 3 {
            continue;
        }
        let (lo, hi) = m
            .positions
            .iter()
            .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let pos = bin.floats(&vec3s(&m.positions), Some(ARRAY_BUFFER), "VEC3", 3);
        bin.accessors[pos]["min"] = json!(lo.to_array());
        bin.accessors[pos]["max"] = json!(hi.to_array());
        let mut attrs = json!({ "POSITION": pos });
        if m.normals.len() == n {
            let normals: Vec<Vec3> = m.normals.iter().map(|v| v.normalize_or(Vec3::Y)).collect();
            attrs["NORMAL"] = json!(bin.floats(&vec3s(&normals), Some(ARRAY_BUFFER), "VEC3", 3));
        }
        if m.uvs.len() == n {
            let uv: Vec<f32> = m.uvs.iter().flat_map(|v| v.to_array()).collect();
            attrs["TEXCOORD_0"] = json!(bin.floats(&uv, Some(ARRAY_BUFFER), "VEC2", 2));
        }
        let skinned = nb > 0 && m.skin.as_ref().is_some_and(|s| s.len() == n);
        if skinned {
            let skin = m.skin.as_ref().unwrap();
            let mut joints: Vec<u8> = Vec::with_capacity(n * 8);
            let mut weights: Vec<f32> = Vec::with_capacity(n * 4);
            for s in skin {
                let mut w = s.weights.map(|w| if w.is_finite() && w > 0.0 { w } else { 0.0 });
                let mut j = s.joints.map(|j| if (j as usize) < nb { j } else { 0 });
                let sum: f32 = w.iter().sum();
                if sum > 0.0 {
                    w = w.map(|x| x / sum);
                } else {
                    (w, j) = ([1.0, 0.0, 0.0, 0.0], [0; 4]);
                }
                for k in 0..4 {
                    // (unused slots point at joint 0 with weight 0)
                    let jk = if w[k] > 0.0 { j[k] } else { 0 };
                    joints.extend_from_slice(&jk.to_le_bytes());
                }
                weights.extend_from_slice(&w);
            }
            attrs["JOINTS_0"] = json!(bin.accessor(&joints, Some(ARRAY_BUFFER), UNSIGNED_SHORT, n, "VEC4"));
            attrs["WEIGHTS_0"] = json!(bin.floats(&weights, Some(ARRAY_BUFFER), "VEC4", 4));
            any_skinned = true;
        }
        let idx: Vec<u8> = m.indices[..m.indices.len() / 3 * 3].iter().flat_map(|i| i.to_le_bytes()).collect();
        let indices = bin.accessor(&idx, Some(ELEMENT_ARRAY_BUFFER), UNSIGNED_INT, idx.len() / 4, "SCALAR");
        let mut prim = json!({ "attributes": attrs, "indices": indices, "mode": 4 });
        if let Some(mi) = m.material.filter(|&i| i < materials.len()) {
            prim["material"] = json!(mi);
        }
        let name = if m.name.is_empty() { format!("mesh{}", meshes.len()) } else { m.name.clone() };
        let mut node = json!({ "name": name, "mesh": meshes.len() });
        if skinned {
            node["skin"] = json!(0);
        }
        meshes.push(json!({ "name": name, "primitives": [prim] }));
        root_children.push(nodes.len());
        nodes.push(node);
    }
    if !root_children.is_empty() {
        nodes[0]["children"] = json!(root_children);
    }

    // Skin.
    let mut skins: Vec<Value> = Vec::new();
    if nb > 0 && any_skinned {
        let ibm: Vec<f32> = bones.iter().flat_map(|b| b.inverse_bind.to_cols_array()).collect();
        let acc = bin.floats(&ibm, None, "MAT4", 16);
        let root = bones.iter().position(|b| b.parent.is_none()).unwrap_or(0) + 1;
        skins.push(json!({
            "name": "Skeleton",
            "joints": (1..=nb).collect::<Vec<_>>(),
            "inverseBindMatrices": acc,
            "skeleton": root,
        }));
    }

    // Animations.
    let mut animations: Vec<Value> = Vec::new();
    for c in clips {
        let frames = c.frames.max(1);
        let fps = if c.fps > 0.0 { c.fps } else { 30.0 };
        let mut samplers: Vec<Value> = Vec::new();
        let mut channels: Vec<Value> = Vec::new();
        let mut input: Option<usize> = None;
        for t in &c.tracks {
            if t.bone >= nb {
                continue;
            }
            let chans: [(&str, Vec<f32>, &str, usize); 3] = [
                ("rotation", rot_floats(&t.rot), "VEC4", t.rot.len()),
                ("translation", vec3s(&t.pos), "VEC3", t.pos.len()),
                ("scale", vec3s(&t.scale), "VEC3", t.scale.len()),
            ];
            for (path, data, ty, len) in chans {
                if len != frames {
                    continue;
                }
                let inp = *input.get_or_insert_with(|| {
                    let times: Vec<f32> = (0..frames).map(|i| i as f32 / fps).collect();
                    let a = bin.floats(&times, None, "SCALAR", 1);
                    bin.accessors[a]["min"] = json!([0.0]);
                    bin.accessors[a]["max"] = json!([times[frames - 1]]);
                    a
                });
                let out = bin.floats(&data, None, ty, if ty == "VEC4" { 4 } else { 3 });
                samplers.push(json!({ "input": inp, "output": out, "interpolation": "LINEAR" }));
                channels.push(json!({ "sampler": samplers.len() - 1, "target": { "node": t.bone + 1, "path": path } }));
            }
        }
        if !channels.is_empty() {
            animations.push(json!({ "name": c.name, "samplers": samplers, "channels": channels }));
        }
    }

    let mut root = json!({
        "asset": { "version": "2.0", "generator": "Skyfold Studio autorig" },
        "scene": 0,
        "scenes": [{ "name": "Character", "nodes": [0] }],
        "nodes": nodes,
    });
    let mut put = |k: &str, v: Vec<Value>| {
        if !v.is_empty() {
            root[k] = Value::Array(v);
        }
    };
    put("meshes", meshes);
    put("materials", materials);
    put("textures", textures);
    put("images", images);
    put("skins", skins);
    put("animations", animations);
    if !model.images.is_empty() {
        root["samplers"] = json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]);
    }
    if emissive_strength {
        root["extensionsUsed"] = json!(["KHR_materials_emissive_strength"]);
    }
    while !bin.data.len().is_multiple_of(4) {
        bin.data.push(0);
    }
    if !bin.data.is_empty() {
        root["buffers"] = json!([{ "byteLength": bin.data.len() }]);
        root["bufferViews"] = Value::Array(std::mem::take(&mut bin.views));
        root["accessors"] = Value::Array(std::mem::take(&mut bin.accessors));
    }

    let mut js = serde_json::to_vec(&root).expect("json");
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let has_bin = !bin.data.is_empty();
    let total = 12 + 8 + js.len() + if has_bin { 8 + bin.data.len() } else { 0 };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&js);
    if has_bin {
        out.extend_from_slice(&(bin.data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin.data);
    }
    out
}

/// Quaternions as xyzw floats, normalized and sign-continuous (no flips between frames).
fn rot_floats(q: &[glam::Quat]) -> Vec<f32> {
    let mut prev: Option<glam::Quat> = None;
    q.iter()
        .flat_map(|&r| {
            let mut r = r.normalize();
            if prev.is_some_and(|p| p.dot(r) < 0.0) {
                r = -r;
            }
            prev = Some(r);
            r.to_array()
        })
        .collect()
}
