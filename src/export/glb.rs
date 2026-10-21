//! A small glTF 2.0 binary (GLB) writer for the exporters (meshes, PBR materials, embedded PNG /
//! JPEG textures, a node tree), and a packer that turns a `.gltf` with external buffers and images
//! (the Poly Haven layout) into one self-contained `.glb`, capping texture sizes (`.glb` sources
//! too). `Glb::open` appends meshes and nodes to an existing file (level-of-detail and collision
//! nodes for packed models), `flat_parts` reads a model's triangles with their node transforms
//! applied, and `map_images` rewrites or moves out the embedded images (shared texture folders).

use std::path::Path;

use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    Repeat,
    Clamp,
}

/// A metallic-roughness material.
pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    pub base_tex: Option<usize>,
    pub normal_tex: Option<usize>,
    pub roughness: f32,
    pub metallic: f32,
    /// Alpha-tested cut-out with this cutoff.
    pub mask: Option<f32>,
    pub double_sided: bool,
}

impl Material {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            base_color: [1.0; 4],
            base_tex: None,
            normal_tex: None,
            roughness: 0.8,
            metallic: 0.0,
            mask: None,
            double_sided: false,
        }
    }
}

/// One triangle list of a mesh.
#[derive(Default)]
pub struct Prim<'a> {
    pub positions: &'a [[f32; 3]],
    pub normals: Option<&'a [[f32; 3]]>,
    pub tangents: Option<&'a [[f32; 4]]>,
    pub uvs: Option<&'a [[f32; 2]]>,
    /// linear RGBA vertex colours (`COLOR_0`)
    pub colors: Option<&'a [[f32; 4]]>,
    pub indices: &'a [u32],
    pub material: Option<usize>,
}

pub struct Glb {
    /// `extensionsUsed` / `extensionsRequired` carried over from a source file
    extensions: Option<(Value, Value)>,
    /// top-level entries of an opened file this writer doesn't manage (samplers, animations,
    /// skins, cameras, extensions…), written back unchanged
    extra: serde_json::Map<String, Value>,
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    images: Vec<Value>,
    textures: Vec<Value>,
    materials: Vec<Value>,
    meshes: Vec<Value>,
    nodes: Vec<Value>,
}

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

impl Glb {
    pub fn new() -> Self {
        Self {
            extensions: None,
            extra: Default::default(),
            bin: vec![],
            views: vec![],
            accessors: vec![],
            images: vec![],
            textures: vec![],
            materials: vec![],
            meshes: vec![],
            nodes: vec![],
        }
    }

    /// An existing GLB to add meshes and nodes to: everything in it stays (node and mesh indices
    /// too, so its animations and skins keep working), new items go after it. Textures can't be
    /// added (the file keeps its own samplers). `None` for files with external buffers.
    pub fn open(glb: &[u8]) -> Option<Glb> {
        let doc = glb_json(glb)?;
        if doc
            .get("buffers")
            .and_then(|b| b.as_array())
            .is_some_and(|b| b.len() > 1 || b.iter().any(|b| b.get("uri").is_some()))
        {
            return None;
        }
        let arr = |k: &str| {
            doc.get(k)
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
        };
        let managed = [
            "asset",
            "scene",
            "scenes",
            "nodes",
            "meshes",
            "materials",
            "textures",
            "images",
            "accessors",
            "bufferViews",
            "buffers",
            "extensionsUsed",
            "extensionsRequired",
        ];
        let extra = doc
            .as_object()?
            .iter()
            .filter(|(k, _)| !managed.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Some(Glb {
            extensions: Some((
                doc.get("extensionsUsed").cloned().unwrap_or(Value::Null),
                doc.get("extensionsRequired")
                    .cloned()
                    .unwrap_or(Value::Null),
            )),
            extra,
            bin: glb_bin(glb)?.to_vec(),
            views: arr("bufferViews"),
            accessors: arr("accessors"),
            images: arr("images"),
            textures: arr("textures"),
            materials: arr("materials"),
            meshes: arr("meshes"),
            nodes: arr("nodes"),
        })
    }

    /// A new file with the images, textures and materials of `glb` (same indices) and nothing
    /// else: for rebuilding a model's geometry while keeping its look.
    pub fn materials_of(glb: &[u8]) -> Option<Glb> {
        let doc = glb_json(glb)?;
        let bin = glb_bin(glb)?;
        let arr = |k: &str| {
            doc.get(k)
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
        };
        let views = arr("bufferViews");
        let mut g = Glb::new();
        g.extensions = Some((
            doc.get("extensionsUsed").cloned().unwrap_or(Value::Null),
            doc.get("extensionsRequired")
                .cloned()
                .unwrap_or(Value::Null),
        ));
        for img in arr("images") {
            if let Some(uri) = img.get("uri") {
                // (a data URI or an already shared texture file)
                g.images.push(
                    json!({ "uri": uri, "name": img.get("name").cloned().unwrap_or(Value::Null) }),
                );
                continue;
            }
            let v = views.get(img.get("bufferView")?.as_u64()? as usize)?;
            let off = v.get("byteOffset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
            let len = v.get("byteLength")?.as_u64()? as usize;
            g.image(
                bin.get(off..off + len)?,
                img.get("mimeType")
                    .and_then(|m| m.as_str())
                    .unwrap_or("image/png"),
                img.get("name").and_then(|m| m.as_str()).unwrap_or(""),
            );
        }
        let samplers = arr("samplers");
        for t in arr("textures") {
            let clamp = t
                .get("sampler")
                .and_then(|s| samplers.get(s.as_u64()? as usize))
                .and_then(|s| s.get("wrapS"))
                .and_then(|w| w.as_u64())
                == Some(33071);
            let i = g.texture(
                t.get("source").and_then(|s| s.as_u64()).unwrap_or(0) as usize,
                if clamp { Wrap::Clamp } else { Wrap::Repeat },
            );
            // texture-format extensions (KTX2, WebP) keep their image sources
            if let Some(ext) = t.get("extensions") {
                g.textures[i]["extensions"] = ext.clone();
            }
        }
        for m in arr("materials") {
            g.material_json(m);
        }
        Some(g)
    }

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = t.into();
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn floats<const N: usize>(&mut self, data: &[[f32; N]], ty: &str, bounds: bool) -> usize {
        let view = self.view(bytemuck::cast_slice(data), Some(ARRAY_BUFFER));
        let mut a =
            json!({ "bufferView": view, "componentType": 5126, "count": data.len(), "type": ty });
        if bounds {
            let (mut mn, mut mx) = ([f32::MAX; N], [f32::MIN; N]);
            for v in data {
                for k in 0..N {
                    mn[k] = mn[k].min(v[k]);
                    mx[k] = mx[k].max(v[k]);
                }
            }
            a["min"] = json!(mn.to_vec());
            a["max"] = json!(mx.to_vec());
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn indices(&mut self, idx: &[u32], vertices: usize) -> usize {
        let (view, ty) = if vertices <= 65535 {
            let short: Vec<u16> = idx.iter().map(|&i| i as u16).collect();
            (
                self.view(bytemuck::cast_slice(&short), Some(ELEMENT_ARRAY_BUFFER)),
                5123,
            )
        } else {
            (
                self.view(bytemuck::cast_slice(idx), Some(ELEMENT_ARRAY_BUFFER)),
                5125,
            )
        };
        self.accessors.push(json!({ "bufferView": view, "componentType": ty, "count": idx.len(), "type": "SCALAR" }));
        self.accessors.len() - 1
    }

    /// An embedded PNG or JPEG image.
    pub fn image(&mut self, bytes: &[u8], mime: &str, name: &str) -> usize {
        let view = self.view(bytes, None);
        self.images
            .push(json!({ "bufferView": view, "mimeType": mime, "name": name }));
        self.images.len() - 1
    }

    /// A texture sampling `image` with trilinear filtering.
    pub fn texture(&mut self, image: usize, wrap: Wrap) -> usize {
        self.textures
            .push(json!({ "source": image, "sampler": if wrap == Wrap::Repeat { 0 } else { 1 } }));
        self.textures.len() - 1
    }

    /// A material as glTF JSON (texture indices must match this file's textures).
    pub fn material_json(&mut self, v: Value) -> usize {
        self.materials.push(v);
        self.materials.len() - 1
    }

    pub fn material(&mut self, m: &Material) -> usize {
        let mut pbr = json!({ "baseColorFactor": m.base_color, "metallicFactor": m.metallic, "roughnessFactor": m.roughness });
        if let Some(t) = m.base_tex {
            pbr["baseColorTexture"] = json!({ "index": t });
        }
        let mut v =
            json!({ "name": m.name, "pbrMetallicRoughness": pbr, "doubleSided": m.double_sided });
        if let Some(t) = m.normal_tex {
            v["normalTexture"] = json!({ "index": t });
        }
        if let Some(c) = m.mask {
            v["alphaMode"] = "MASK".into();
            v["alphaCutoff"] = c.into();
        }
        self.materials.push(v);
        self.materials.len() - 1
    }

    /// A mesh from its primitives (empty ones are skipped). `None` if nothing is left.
    pub fn mesh(&mut self, name: &str, prims: &[Prim]) -> Option<usize> {
        let mut out = vec![];
        for p in prims
            .iter()
            .filter(|p| !p.positions.is_empty() && p.indices.len() >= 3)
        {
            let mut attrs = json!({ "POSITION": self.floats(p.positions, "VEC3", true) });
            if let Some(n) = p.normals.filter(|n| n.len() == p.positions.len()) {
                attrs["NORMAL"] = self.floats(n, "VEC3", false).into();
            }
            if let Some(t) = p.tangents.filter(|t| t.len() == p.positions.len()) {
                attrs["TANGENT"] = self.floats(t, "VEC4", false).into();
            }
            if let Some(uv) = p.uvs.filter(|u| u.len() == p.positions.len()) {
                attrs["TEXCOORD_0"] = self.floats(uv, "VEC2", false).into();
            }
            if let Some(c) = p.colors.filter(|c| c.len() == p.positions.len()) {
                attrs["COLOR_0"] = self.floats(c, "VEC4", false).into();
            }
            let mut prim = json!({ "attributes": attrs, "indices": self.indices(p.indices, p.positions.len()), "mode": 4 });
            if let Some(m) = p.material {
                prim["material"] = m.into();
            }
            out.push(prim);
        }
        if out.is_empty() {
            return None;
        }
        self.meshes.push(json!({ "name": name, "primitives": out }));
        Some(self.meshes.len() - 1)
    }

    pub fn node(&mut self, name: &str, mesh: Option<usize>, children: Vec<usize>) -> usize {
        let mut n = json!({ "name": name });
        if let Some(m) = mesh {
            n["mesh"] = m.into();
        }
        if !children.is_empty() {
            n["children"] = json!(children);
        }
        self.nodes.push(n);
        self.nodes.len() - 1
    }

    /// Places a node (relative to its parent): a translation and a rotation (x, y, z, w).
    pub fn set_transform(&mut self, node: usize, translation: [f32; 3], rotation: [f32; 4]) {
        if let Some(n) = self.nodes.get_mut(node) {
            n["translation"] = json!(translation);
            n["rotation"] = json!(rotation);
        }
    }

    /// The finished `.glb` file with `roots` as its scene.
    pub fn finish(self, roots: &[usize]) -> Vec<u8> {
        let mut doc = json!({
            "asset": { "version": "2.0", "generator": "Skyfold Studio" },
            "scene": 0,
            "scenes": [{ "nodes": roots }],
            "samplers": [
                { "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 },
                { "magFilter": 9729, "minFilter": 9987, "wrapS": 33071, "wrapT": 33071 },
            ],
        });
        // an opened file's own samplers, animations, skins…
        for (k, v) in self.extra {
            doc[k] = v;
        }
        // glTF forbids empty top-level arrays
        for (key, list) in [
            ("nodes", self.nodes),
            ("meshes", self.meshes),
            ("materials", self.materials),
            ("textures", self.textures),
            ("images", self.images),
            ("accessors", self.accessors),
            ("bufferViews", self.views),
        ] {
            if !list.is_empty() {
                doc[key] = Value::Array(list);
            }
        }
        if !self.bin.is_empty() {
            doc["buffers"] = json!([{ "byteLength": self.bin.len() }]);
        }
        if let Some((used, required)) = self.extensions {
            if used.as_array().is_some_and(|a| !a.is_empty()) {
                doc["extensionsUsed"] = used;
            }
            if required.as_array().is_some_and(|a| !a.is_empty()) {
                doc["extensionsRequired"] = required;
            }
        }
        assemble(&doc, self.bin)
    }
}

/// GLB container: 12-byte header, JSON chunk (space padded), BIN chunk (zero padded).
fn assemble(doc: &Value, mut bin: Vec<u8>) -> Vec<u8> {
    let mut json = serde_json::to_vec(doc).unwrap_or_default();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
    }
    out
}

/// `%20`-style escapes in glTF URIs.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = vec![];
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Reads a glTF URI: a `data:` URI or a file next to the `.gltf`.
fn read_uri(base: &Path, uri: &str) -> Result<Vec<u8>, String> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, data) = rest.split_once(";base64,").ok_or("unsupported data URI")?;
        return base64_decode(data).ok_or_else(|| "bad base64 data URI".to_string());
    }
    let p = base.join(percent_decode(uri));
    std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Downscales an encoded PNG / JPEG so neither side exceeds `max` (same format, `None` if it
/// already fits or can't be decoded).
fn shrink_image(bytes: &[u8], mime: &str, max: u32) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    if img.width().max(img.height()) <= max {
        return None;
    }
    let k = max as f32 / img.width().max(img.height()) as f32;
    let (w, h) = (
        ((img.width() as f32 * k).round() as u32).max(1),
        ((img.height() as f32 * k).round() as u32).max(1),
    );
    let small = img.resize_exact(w, h, image::imageops::FilterType::Triangle);
    if mime == "image/jpeg" {
        Some(super::core::jpeg(&small.to_rgb8(), 90))
    } else {
        let mut out = std::io::Cursor::new(vec![]);
        small.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some(out.into_inner())
    }
}

/// One model of a merged GLB (`merge`): its GLB, which variant to take (exported trees and
/// scans hold one root per variant, `<name>_v<k>`, each with LOD children: the first LOD is
/// taken) and where each copy stands (column-major 4 × 4 matrices).
pub struct MergePart<'a> {
    pub name: String,
    pub glb: &'a [u8],
    pub instances: Vec<(u32, [f32; 16])>,
}

/// Several GLBs as one self-contained GLB: each part's buffers, images, materials and meshes
/// once, and a node tree per instance (meshes shared). Skins, cameras and animations are left
/// out. `None` when a part can't be read or uses external or compressed data.
pub fn merge(parts: &[MergePart]) -> Option<Vec<u8>> {
    let mut out: std::collections::BTreeMap<&str, Vec<Value>> = Default::default();
    let mut bin: Vec<u8> = vec![];
    let mut roots = vec![];
    let (mut used, mut required): (Vec<Value>, Vec<Value>) = (vec![], vec![]);
    for part in parts {
        let doc = glb_json(part.glb)?;
        let pbin = glb_bin(part.glb).unwrap_or(&[]);
        let arr = |k: &str| {
            doc.get(k)
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
        };
        let len = |out: &std::collections::BTreeMap<&str, Vec<Value>>, k: &str| {
            out.get(k).map_or(0, |v| v.len()) as u64
        };
        let (bv0, acc0, img0, tex0, smp0, mat0, mesh0) = (
            len(&out, "bufferViews"),
            len(&out, "accessors"),
            len(&out, "images"),
            len(&out, "textures"),
            len(&out, "samplers"),
            len(&out, "materials"),
            len(&out, "meshes"),
        );
        while !bin.len().is_multiple_of(8) {
            bin.push(0);
        }
        let boff = bin.len() as u64;
        bin.extend_from_slice(pbin);
        let add = |v: &mut Value, k: &str, by: u64| {
            if let Some(i) = v.get(k).and_then(|x| x.as_u64()) {
                v[k] = json!(i + by);
            }
        };
        for mut bv in arr("bufferViews") {
            if bv.get("buffer").and_then(|b| b.as_u64()).unwrap_or(0) != 0 {
                return None;
            }
            bv["buffer"] = json!(0);
            bv["byteOffset"] =
                json!(bv.get("byteOffset").and_then(|o| o.as_u64()).unwrap_or(0) + boff);
            out.entry("bufferViews").or_default().push(bv);
        }
        for mut a in arr("accessors") {
            add(&mut a, "bufferView", bv0);
            if let Some(sp) = a.get_mut("sparse") {
                add(&mut sp["indices"], "bufferView", bv0);
                add(&mut sp["values"], "bufferView", bv0);
            }
            out.entry("accessors").or_default().push(a);
        }
        for mut im in arr("images") {
            if im.get("bufferView").is_some() {
                add(&mut im, "bufferView", bv0);
            } else if !im
                .get("uri")
                .and_then(|u| u.as_str())
                .is_some_and(|u| u.starts_with("data:"))
            {
                return None;
            }
            out.entry("images").or_default().push(im);
        }
        out.entry("samplers").or_default().extend(arr("samplers"));
        for mut t in arr("textures") {
            add(&mut t, "source", img0);
            add(&mut t, "sampler", smp0);
            if let Some(ext) = t.get_mut("extensions").and_then(|e| e.as_object_mut()) {
                for e in ext.values_mut() {
                    add(e, "source", img0);
                }
            }
            out.entry("textures").or_default().push(t);
        }
        // texture references sit in "...Texture" objects, extensions included
        fn retex(v: &mut Value, by: u64, in_tex: bool) {
            match v {
                Value::Object(m) => {
                    for (k, x) in m.iter_mut() {
                        if in_tex && k == "index" {
                            if let Some(i) = x.as_u64() {
                                *x = json!(i + by);
                            }
                        } else {
                            retex(x, by, k.ends_with("Texture"));
                        }
                    }
                }
                Value::Array(a) => a.iter_mut().for_each(|x| retex(x, by, false)),
                _ => {}
            }
        }
        for mut m in arr("materials") {
            retex(&mut m, tex0, false);
            out.entry("materials").or_default().push(m);
        }
        for mut m in arr("meshes") {
            for p in m
                .get_mut("primitives")
                .and_then(|p| p.as_array_mut())
                .into_iter()
                .flatten()
            {
                if p.get("extensions")
                    .is_some_and(|e| e.get("KHR_draco_mesh_compression").is_some())
                {
                    return None;
                }
                if let Some(at) = p.get_mut("attributes").and_then(|a| a.as_object_mut()) {
                    for x in at.values_mut() {
                        *x = json!(x.as_u64()? + acc0);
                    }
                }
                add(p, "indices", acc0);
                add(p, "material", mat0);
                for t in p
                    .get_mut("targets")
                    .and_then(|t| t.as_array_mut())
                    .into_iter()
                    .flatten()
                {
                    if let Some(t) = t.as_object_mut() {
                        for x in t.values_mut() {
                            *x = json!(x.as_u64()? + acc0);
                        }
                    }
                }
            }
            out.entry("meshes").or_default().push(m);
        }
        for (k, list) in [
            ("extensionsUsed", &mut used),
            ("extensionsRequired", &mut required),
        ] {
            for e in arr(k) {
                if !list.contains(&e) {
                    list.push(e);
                }
            }
        }
        // which roots: the instance's variant (and its first LOD), else the whole scene
        let nodes = arr("nodes");
        let scene = doc.get("scene").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
        let name_of = |i: usize| {
            nodes
                .get(i)
                .and_then(|n| n.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string()
        };
        // (collision proxies, `<name>_v<k>_COL`, are never drawn)
        let scene_roots: Vec<usize> = doc
            .get("scenes")
            .and_then(|s| s.get(scene))
            .and_then(|s| s.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_u64())
                    .map(|x| x as usize)
                    .collect::<Vec<usize>>()
            })
            .unwrap_or_else(|| (0..nodes.len()).collect())
            .into_iter()
            .filter(|&r| !name_of(r).ends_with("_COL"))
            .collect();
        let variants = !scene_roots.is_empty()
            && scene_roots.iter().all(|&r| {
                name_of(r)
                    .rsplit_once("_v")
                    .is_some_and(|(_, k)| k.parse::<u32>().is_ok())
            });
        let pick = |variant: u32| -> Vec<usize> {
            if !variants {
                return scene_roots.clone();
            }
            let r = scene_roots[(variant as usize).min(scene_roots.len() - 1)];
            let kids: Vec<usize> = nodes[r]
                .get("children")
                .and_then(|c| c.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64())
                        .map(|x| x as usize)
                        .collect()
                })
                .unwrap_or_default();
            match kids.first() {
                Some(&k) if name_of(k).contains("_LOD") => vec![k],
                _ => vec![r],
            }
        };
        fn copy(
            nodes: &[Value],
            i: usize,
            mesh0: u64,
            out: &mut Vec<Value>,
            depth: usize,
        ) -> Option<u64> {
            if depth > 64 {
                return None;
            }
            let mut n = nodes.get(i)?.clone();
            let kids: Vec<usize> = n
                .get("children")
                .and_then(|c| c.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64())
                        .map(|x| x as usize)
                        .collect()
                })
                .unwrap_or_default();
            let new: Vec<u64> = kids
                .into_iter()
                .filter_map(|k| copy(nodes, k, mesh0, out, depth + 1))
                .collect();
            if let Some(o) = n.as_object_mut() {
                o.remove("skin");
                o.remove("camera");
                o.remove("children");
                if let Some(m) = o.get("mesh").and_then(|m| m.as_u64()) {
                    o.insert("mesh".into(), json!(m + mesh0));
                }
                if !new.is_empty() {
                    o.insert("children".into(), json!(new));
                }
            }
            out.push(n);
            Some(out.len() as u64 - 1)
        }
        for (k, (variant, m)) in part.instances.iter().enumerate() {
            let list = out.entry("nodes").or_default();
            let kids: Vec<u64> = pick(*variant)
                .into_iter()
                .filter_map(|r| copy(&nodes, r, mesh0, list, 0))
                .collect();
            list.push(json!({ "name": format!("{}_{k}", part.name), "matrix": m.to_vec(), "children": kids }));
            roots.push(list.len() - 1);
        }
    }
    let mut doc = json!({ "asset": { "version": "2.0", "generator": "Skyfold Studio" }, "scene": 0, "scenes": [{ "nodes": roots }] });
    for (k, list) in out {
        if !list.is_empty() {
            doc[k] = Value::Array(list);
        }
    }
    if !bin.is_empty() {
        doc["buffers"] = json!([{ "byteLength": bin.len() }]);
    }
    if !used.is_empty() {
        doc["extensionsUsed"] = Value::Array(used);
    }
    if !required.is_empty() {
        doc["extensionsRequired"] = Value::Array(required);
    }
    Some(assemble(&doc, bin))
}

/// Packs a `.gltf` (or copies a `.glb`) into one self-contained GLB, capping textures at
/// `max_tex` pixels (embedded `.glb` images too).
pub fn pack(path: &Path, max_tex: u32) -> Result<Vec<u8>, String> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("glb"))
    {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let capped = map_images(&bytes, |_, b, mime, _| {
            let small = if mime == "image/png" || mime == "image/jpeg" {
                shrink_image(b, mime, max_tex)
            } else {
                None
            };
            small.map(|s| ImageOut::Embed(s, mime.to_string()))
        });
        return Ok(capped.unwrap_or(bytes));
    }
    let base = path.parent().unwrap_or(Path::new("."));
    let mut doc: Value = serde_json::from_str(
        &std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    drop_painted_flames(&mut doc);
    let mut bin: Vec<u8> = vec![];
    // every buffer goes into the one BIN chunk; buffer views are rebased onto it
    let mut offsets = vec![];
    for b in doc
        .get("buffers")
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default()
    {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        offsets.push(bin.len());
        let uri = b
            .get("uri")
            .and_then(|u| u.as_str())
            .ok_or("buffer without uri")?;
        bin.extend(read_uri(base, uri)?);
    }
    if let Some(views) = doc.get_mut("bufferViews").and_then(|v| v.as_array_mut()) {
        for v in views {
            let b = v.get("buffer").and_then(|b| b.as_u64()).unwrap_or(0) as usize;
            let off = v.get("byteOffset").and_then(|o| o.as_u64()).unwrap_or(0) as usize
                + offsets.get(b).copied().unwrap_or(0);
            v["buffer"] = 0.into();
            v["byteOffset"] = off.into();
        }
    }
    // images with URIs become buffer views
    let images = doc
        .get("images")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    let mut new_images = vec![];
    let mut new_views = vec![];
    let n_views = doc
        .get("bufferViews")
        .and_then(|v| v.as_array())
        .map_or(0, |v| v.len());
    for mut img in images {
        if let Some(uri) = img.get("uri").and_then(|u| u.as_str()).map(str::to_string) {
            let bytes = read_uri(base, &uri)?;
            let lower = uri.to_ascii_lowercase();
            let mime = if lower.ends_with(".png") || lower.starts_with("data:image/png") {
                "image/png"
            } else if lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.starts_with("data:image/jpeg")
            {
                "image/jpeg"
            } else if lower.ends_with(".ktx2") {
                "image/ktx2"
            } else if lower.ends_with(".webp") {
                "image/webp"
            } else {
                "application/octet-stream"
            };
            let bytes = if mime == "image/png" || mime == "image/jpeg" {
                shrink_image(&bytes, mime, max_tex).unwrap_or(bytes)
            } else {
                bytes
            };
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            new_views
                .push(json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len() }));
            bin.extend(bytes);
            let obj = img.as_object_mut().ok_or("bad image")?;
            obj.remove("uri");
            obj.insert("bufferView".into(), (n_views + new_views.len() - 1).into());
            obj.insert("mimeType".into(), mime.into());
        }
        new_images.push(img);
    }
    if !new_images.is_empty() {
        doc["images"] = Value::Array(new_images);
    }
    if !new_views.is_empty() {
        let views = doc.get_mut("bufferViews").and_then(|v| v.as_array_mut());
        match views {
            Some(v) => v.extend(new_views),
            None => doc["bufferViews"] = Value::Array(new_views),
        }
    }
    if bin.is_empty() {
        doc.as_object_mut().map(|o| o.remove("buffers"));
    } else {
        doc["buffers"] = json!([{ "byteLength": bin.len() }]);
    }
    Ok(assemble(&doc, bin))
}

/// Triangle count of a GLB (from its index accessors, or vertex counts for unindexed lists).
pub fn triangles(glb: &[u8]) -> usize {
    let Some(doc) = glb_json(glb) else { return 0 };
    let acc = doc.get("accessors").and_then(|a| a.as_array());
    let count = |i: &Value| {
        i.as_u64()
            .and_then(|i| acc?.get(i as usize)?.get("count")?.as_u64())
            .unwrap_or(0) as usize
    };
    let mut tris = 0;
    for m in doc
        .get("meshes")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
    {
        for p in m
            .get("primitives")
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
        {
            tris += match p.get("indices") {
                Some(i) => count(i) / 3,
                None => {
                    p.get("attributes")
                        .and_then(|a| a.get("POSITION"))
                        .map(count)
                        .unwrap_or(0)
                        / 3
                }
            };
        }
    }
    tris
}

/// The BIN chunk of a GLB (empty if it has none).
fn glb_bin(glb: &[u8]) -> Option<&[u8]> {
    let len = u32::from_le_bytes(glb.get(12..16)?.try_into().ok()?) as usize;
    let at = 20 + len;
    if glb.len() < at + 8 {
        return Some(&[]);
    }
    let blen = u32::from_le_bytes(glb[at..at + 4].try_into().ok()?) as usize;
    glb.get(at + 8..at + 8 + blen)
}

/// Takes the painted flame cones (the hand-painted pack's `flame` material) out of a glTF
/// document: the export burns real fire where they stood (`crate::flames`). A mesh left with
/// nothing goes, its nodes keep their place.
pub fn drop_painted_flames(doc: &mut Value) {
    let flame: Vec<u64> = doc["materials"]
        .as_array()
        .map(|m| {
            m.iter()
                .enumerate()
                .filter(|(_, m)| m["name"] == crate::flames::PAINTED_FLAME)
                .map(|(i, _)| i as u64)
                .collect()
        })
        .unwrap_or_default();
    if flame.is_empty() {
        return;
    }
    let Some(meshes) = doc["meshes"].as_array_mut() else {
        return;
    };
    for m in meshes.iter_mut() {
        if let Some(p) = m["primitives"].as_array_mut() {
            p.retain(|p| !p["material"].as_u64().is_some_and(|i| flame.contains(&i)));
        }
    }
    // (glTF meshes need a primitive: empty ones go and the indices close up)
    let keep: Vec<bool> = meshes
        .iter()
        .map(|m| m["primitives"].as_array().is_some_and(|p| !p.is_empty()))
        .collect();
    if keep.iter().all(|k| *k) {
        return;
    }
    let mut remap = vec![None; keep.len()];
    let mut n = 0u64;
    for (i, k) in keep.iter().enumerate() {
        if *k {
            remap[i] = Some(n);
            n += 1;
        }
    }
    let mut i = 0;
    meshes.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    if let Some(nodes) = doc["nodes"].as_array_mut() {
        for node in nodes {
            if let Some(m) = node["mesh"].as_u64() {
                match remap.get(m as usize).copied().flatten() {
                    Some(to) => node["mesh"] = to.into(),
                    None => {
                        if let Some(o) = node.as_object_mut() {
                            o.remove("mesh");
                        }
                    }
                }
            }
        }
    }
}

/// One primitive of a model with its node transforms applied (`flat_parts`).
pub struct FlatPart {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub tan: Vec<[f32; 4]>,
    pub uv: Vec<[f32; 2]>,
    pub color: Vec<[f32; 4]>,
    pub idx: Vec<u32>,
    pub material: Option<usize>,
}

impl FlatPart {
    pub fn prim(&self) -> Prim<'_> {
        Prim {
            positions: &self.pos,
            normals: (!self.nrm.is_empty()).then_some(&self.nrm[..]),
            tangents: (!self.tan.is_empty()).then_some(&self.tan[..]),
            uvs: (!self.uv.is_empty()).then_some(&self.uv[..]),
            colors: (!self.color.is_empty()).then_some(&self.color[..]),
            indices: &self.idx,
            material: self.material,
        }
    }
}

/// Every triangle primitive under the default scene (or under the node named `root`) with the
/// node transforms baked into its vertices (normals and tangents follow; mirrored nodes get their
/// winding flipped). `None` when the file uses data this can't transform (non-float or sparse
/// vertex data, non-triangle lists, compressed meshes).
pub fn flat_parts(glb: &[u8], root: Option<&str>) -> Option<Vec<FlatPart>> {
    use bevy::math::{Mat3, Mat4, Quat, Vec3, Vec4};
    let doc = glb_json(glb)?;
    let bin = glb_bin(glb)?;
    let arr = |k: &str| {
        doc.get(k)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let (accessors, views, nodes, meshes) = (
        arr("accessors"),
        arr("bufferViews"),
        arr("nodes"),
        arr("meshes"),
    );
    let view_bytes = |vi: usize| -> Option<(&[u8], usize)> {
        let v = views.get(vi)?;
        let off = v.get("byteOffset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
        let len = v.get("byteLength")?.as_u64()? as usize;
        Some((
            bin.get(off..off + len)?,
            v.get("byteStride").and_then(|s| s.as_u64()).unwrap_or(0) as usize,
        ))
    };
    // float vertex data as rows of `n` components (normalized u8 / u16 colours too)
    let floats = |ai: &Value, n: usize| -> Option<Vec<Vec<f32>>> {
        let a = accessors.get(ai.as_u64()? as usize)?;
        if a.get("sparse").is_some() {
            return None;
        }
        let (size, scale) = match (
            a.get("componentType")?.as_u64()?,
            a.get("normalized")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        ) {
            (5126, _) => (4, 0.0),
            (5121, true) => (1, 255.0),
            (5123, true) => (2, 65535.0),
            _ => return None,
        };
        let count = a.get("count")?.as_u64()? as usize;
        let (data, stride) = view_bytes(a.get("bufferView")?.as_u64()? as usize)?;
        let base = a.get("byteOffset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
        let stride = if stride == 0 { n * size } else { stride };
        (0..count)
            .map(|i| {
                (0..n)
                    .map(|c| {
                        let o = base + i * stride + c * size;
                        let b = data.get(o..o + size)?;
                        Some(match size {
                            4 => f32::from_le_bytes(b.try_into().ok()?),
                            2 => u16::from_le_bytes([b[0], b[1]]) as f32 / scale,
                            _ => b[0] as f32 / scale,
                        })
                    })
                    .collect()
            })
            .collect()
    };
    let indices = |ai: &Value| -> Option<Vec<u32>> {
        let a = accessors.get(ai.as_u64()? as usize)?;
        let count = a.get("count")?.as_u64()? as usize;
        let (data, _) = view_bytes(a.get("bufferView")?.as_u64()? as usize)?;
        let base = a.get("byteOffset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
        let size = match a.get("componentType")?.as_u64()? {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => return None,
        };
        (0..count)
            .map(|i| {
                let b = data.get(base + i * size..base + (i + 1) * size)?;
                Some(match size {
                    1 => b[0] as u32,
                    2 => u16::from_le_bytes([b[0], b[1]]) as u32,
                    _ => u32::from_le_bytes(b.try_into().ok()?),
                })
            })
            .collect()
    };
    let local = |n: &Value| -> Mat4 {
        if let Some(m) = n
            .get("matrix")
            .and_then(|m| m.as_array())
            .filter(|m| m.len() == 16)
        {
            let v: Vec<f32> = m.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
            return Mat4::from_cols_slice(&v);
        }
        let get = |k: &str, d: &[f32]| -> Vec<f32> {
            n.get(k)
                .and_then(|v| v.as_array())
                .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                .unwrap_or_else(|| d.to_vec())
        };
        let (t, r, s) = (
            get("translation", &[0.0; 3]),
            get("rotation", &[0.0, 0.0, 0.0, 1.0]),
            get("scale", &[1.0; 3]),
        );
        Mat4::from_scale_rotation_translation(
            Vec3::new(s[0], s[1], s[2]),
            Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize(),
            Vec3::new(t[0], t[1], t[2]),
        )
    };
    // world matrices of the default scene's mesh nodes (or of the named node's subtree)
    let scene = doc.get("scene").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
    let roots: Vec<usize> = match root {
        Some(name) => vec![
            nodes
                .iter()
                .position(|n| n.get("name").and_then(|v| v.as_str()) == Some(name))?,
        ],
        None => doc
            .get("scenes")
            .and_then(|s| s.get(scene))
            .and_then(|s| s.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_u64().map(|v| v as usize))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let mut stack: Vec<(usize, Mat4, usize)> =
        roots.iter().map(|&r| (r, Mat4::IDENTITY, 0)).collect();
    let mut parts: Vec<(usize, Mat4)> = vec![];
    while let Some((ni, parent, depth)) = stack.pop() {
        let n = nodes.get(ni)?;
        if depth > 64 {
            return None;
        }
        // (a named subtree is placed at the origin: its own transform is left out)
        let m = if root.is_some() && depth == 0 {
            Mat4::IDENTITY
        } else {
            parent * local(n)
        };
        if let Some(mi) = n.get("mesh").and_then(|m| m.as_u64()) {
            parts.push((mi as usize, m));
        }
        for c in n
            .get("children")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            stack.push((c.as_u64()? as usize, m, depth + 1));
        }
    }
    let mut out = vec![];
    for (mi, m) in parts {
        let nm = Mat3::from_mat4(m).inverse().transpose();
        let flip = Mat3::from_mat4(m).determinant() < 0.0;
        for p in meshes.get(mi)?.get("primitives")?.as_array()? {
            if p.get("mode").and_then(|v| v.as_u64()).unwrap_or(4) != 4
                || p.get("extensions")
                    .is_some_and(|e| e.get("KHR_draco_mesh_compression").is_some())
            {
                return None;
            }
            let at = p.get("attributes")?;
            let pos: Vec<[f32; 3]> = floats(at.get("POSITION")?, 3)?
                .iter()
                .map(|v| m.transform_point3(Vec3::new(v[0], v[1], v[2])).to_array())
                .collect();
            let nrm: Vec<[f32; 3]> = match at.get("NORMAL") {
                Some(a) => floats(a, 3)?
                    .iter()
                    .map(|v| {
                        (nm * Vec3::new(v[0], v[1], v[2]))
                            .normalize_or(Vec3::Y)
                            .to_array()
                    })
                    .collect(),
                None => vec![],
            };
            let tan: Vec<[f32; 4]> = match at.get("TANGENT") {
                Some(a) => floats(a, 4)?
                    .iter()
                    .map(|v| {
                        let t = m
                            .transform_vector3(Vec3::new(v[0], v[1], v[2]))
                            .normalize_or(Vec3::X);
                        Vec4::new(t.x, t.y, t.z, if flip { -v[3] } else { v[3] }).to_array()
                    })
                    .collect(),
                None => vec![],
            };
            let uv: Vec<[f32; 2]> = match at.get("TEXCOORD_0") {
                Some(a) => floats(a, 2)?.iter().map(|v| [v[0], v[1]]).collect(),
                None => vec![],
            };
            let color: Vec<[f32; 4]> = match at.get("COLOR_0") {
                Some(a) => {
                    let n = if accessors.get(a.as_u64()? as usize)?.get("type")?.as_str()? == "VEC3"
                    {
                        3
                    } else {
                        4
                    };
                    floats(a, n)?
                        .iter()
                        .map(|v| [v[0], v[1], v[2], if n == 4 { v[3] } else { 1.0 }])
                        .collect()
                }
                None => vec![],
            };
            let mut idx = match p.get("indices") {
                Some(i) => indices(i)?,
                None => (0..pos.len() as u32).collect(),
            };
            if idx.iter().any(|&i| i as usize >= pos.len()) {
                return None;
            }
            if flip {
                for t in idx.chunks_exact_mut(3) {
                    t.swap(1, 2);
                }
            }
            out.push(FlatPart {
                pos,
                nrm,
                tan,
                uv,
                color,
                idx,
                material: p
                    .get("material")
                    .and_then(|m| m.as_u64())
                    .map(|m| m as usize),
            });
        }
    }
    Some(out)
}

/// Bakes every mesh node of a GLB into one mesh under a single root node (node transforms
/// applied to the vertices), keeping materials, textures and images: engines that import one
/// static mesh per glTF mesh (Unreal) then get a multi-part model in one piece. `None` when the
/// file uses data this can't transform (see `flat_parts`). (Unreal's packed models are flattened
/// per level of detail by `lods::packed`.)
#[cfg(test)]
pub fn flatten(glb: &[u8], name: &str) -> Option<Vec<u8>> {
    let parts = flat_parts(glb, None)?;
    let mut g = Glb::materials_of(glb)?;
    let prims: Vec<Prim> = parts.iter().map(FlatPart::prim).collect();
    let mesh = g.mesh(name, &prims)?;
    let node = g.node(name, Some(mesh), vec![]);
    Some(g.finish(&[node]))
}

/// What becomes of an embedded image (`map_images`).
pub enum ImageOut {
    /// new bytes and MIME type, still embedded
    Embed(Vec<u8>, String),
    /// moved out: the image refers to this URI (relative to the GLB)
    Uri(String),
}

/// Rewrites the embedded images of a GLB: `f(image index, bytes, mime, name)` returns what the
/// image becomes (`None` keeps it). The binary chunk is rebuilt without the moved-out images and
/// every `bufferView` reference renumbered. `None` if the file can't be read.
pub fn map_images(
    glb: &[u8],
    mut f: impl FnMut(usize, &[u8], &str, &str) -> Option<ImageOut>,
) -> Option<Vec<u8>> {
    let mut doc = glb_json(glb)?;
    let bin = glb_bin(glb)?;
    if doc
        .get("buffers")
        .and_then(|b| b.as_array())
        .is_some_and(|b| b.len() > 1 || b.iter().any(|b| b.get("uri").is_some()))
    {
        return None;
    }
    let views = doc
        .get("bufferViews")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let view_bytes = |vi: usize| -> Option<&[u8]> {
        let v = views.get(vi)?;
        let off = v.get("byteOffset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
        bin.get(off..off + v.get("byteLength")?.as_u64()? as usize)
    };
    // per buffer view: Some(new bytes) to replace it, or dropped
    let mut replace: std::collections::HashMap<usize, Option<Vec<u8>>> =
        std::collections::HashMap::new();
    let mut changed = false;
    if let Some(images) = doc.get_mut("images").and_then(|i| i.as_array_mut()) {
        for (k, img) in images.iter_mut().enumerate() {
            let Some(vi) = img
                .get("bufferView")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
            else {
                continue;
            };
            let bytes = view_bytes(vi)?;
            let mime = img
                .get("mimeType")
                .and_then(|m| m.as_str())
                .unwrap_or("image/png")
                .to_string();
            let name = img
                .get("name")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            match f(k, bytes, &mime, &name) {
                None => {}
                Some(ImageOut::Embed(b, m)) => {
                    img["mimeType"] = json!(m);
                    replace.insert(vi, Some(b));
                    changed = true;
                }
                Some(ImageOut::Uri(uri)) => {
                    if let Some(o) = img.as_object_mut() {
                        o.remove("bufferView");
                        o.remove("mimeType");
                        o.insert("uri".into(), json!(uri));
                    }
                    replace.insert(vi, None);
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return Some(glb.to_vec());
    }
    let mut out_bin: Vec<u8> = vec![];
    let mut new_views = vec![];
    let mut remap: Vec<Option<usize>> = vec![];
    for (vi, v) in views.iter().enumerate() {
        let bytes: Vec<u8> = match replace.get(&vi) {
            Some(None) => {
                remap.push(None);
                continue;
            }
            Some(Some(b)) => b.clone(),
            None => view_bytes(vi)?.to_vec(),
        };
        while !out_bin.len().is_multiple_of(4) {
            out_bin.push(0);
        }
        let mut nv = v.clone();
        nv["buffer"] = json!(0);
        nv["byteOffset"] = json!(out_bin.len());
        nv["byteLength"] = json!(bytes.len());
        out_bin.extend_from_slice(&bytes);
        remap.push(Some(new_views.len()));
        new_views.push(nv);
    }
    // every "bufferView" key anywhere (accessors, sparse data, images, extensions)
    fn renumber(v: &mut Value, remap: &[Option<usize>]) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    if k == "bufferView" {
                        if let Some(i) = x.as_u64() {
                            *x = json!(remap.get(i as usize).copied().flatten().unwrap_or(0));
                        }
                    } else {
                        renumber(x, remap);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| renumber(x, remap)),
            _ => {}
        }
    }
    renumber(&mut doc, &remap);
    if new_views.is_empty() {
        if let Some(o) = doc.as_object_mut() {
            o.remove("bufferViews");
            o.remove("buffers");
        }
    } else {
        doc["bufferViews"] = Value::Array(new_views);
        doc["buffers"] = json!([{ "byteLength": out_bin.len() }]);
    }
    Some(assemble(&doc, out_bin))
}

/// The JSON chunk of a GLB.
pub fn glb_json(glb: &[u8]) -> Option<Value> {
    if glb.len() < 20 || &glb[..4] != b"glTF" {
        return None;
    }
    let len = u32::from_le_bytes(glb[12..16].try_into().ok()?) as usize;
    serde_json::from_slice(glb.get(20..20 + len)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_glb() {
        let mut g = Glb::new();
        let mat = g.material(&Material::new("m"));
        let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = g
            .mesh(
                "tri",
                &[Prim {
                    positions: &pos,
                    indices: &[0, 1, 2],
                    material: Some(mat),
                    ..Default::default()
                }],
            )
            .unwrap();
        let n = g.node("tri", Some(mesh), vec![]);
        let bytes = g.finish(&[n]);
        assert_eq!(&bytes[..4], b"glTF");
        assert_eq!(
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            bytes.len()
        );
        assert_eq!(bytes.len() % 4, 0);
        let doc = glb_json(&bytes).unwrap();
        assert_eq!(doc["accessors"][0]["max"][0], 1.0);
        assert_eq!(triangles(&bytes), 1);
    }

    #[test]
    fn flattens_node_transforms() {
        let mut g = Glb::new();
        let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = g
            .mesh(
                "tri",
                &[Prim {
                    positions: &pos,
                    indices: &[0, 1, 2],
                    ..Default::default()
                }],
            )
            .unwrap();
        let child = g.node("part", Some(mesh), vec![]);
        g.nodes[child]["translation"] = json!([10.0, 0.0, 0.0]);
        let root = g.node("root", None, vec![child]);
        let flat = flatten(&g.finish(&[root]), "model").unwrap();
        let doc = glb_json(&flat).unwrap();
        assert_eq!(doc["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(doc["accessors"][0]["min"][0], 10.0);
        assert_eq!(doc["accessors"][0]["max"][0], 11.0);
    }

    #[test]
    fn decodes_uris() {
        assert_eq!(percent_decode("a%20b.png"), "a b.png");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
    }

    #[test]
    fn merged_models_share_meshes_and_keep_their_textures() {
        // two little models: a textured quad, and a two-variant "tree" with LODs
        let quad = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let tri = [0u32, 1, 2];
        let mut a = Glb::new();
        let png = {
            let mut c = std::io::Cursor::new(vec![]);
            image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 10, 10, 255]))
                .write_to(&mut c, image::ImageFormat::Png)
                .unwrap();
            c.into_inner()
        };
        let img = a.image(&png, "image/png", "red");
        let tex = a.texture(img, Wrap::Repeat);
        let mat = a.material(&Material {
            base_tex: Some(tex),
            ..Material::new("red")
        });
        let m = a
            .mesh(
                "quad",
                &[Prim {
                    positions: &quad,
                    indices: &tri,
                    material: Some(mat),
                    ..Default::default()
                }],
            )
            .unwrap();
        let n = a.node("quad", Some(m), vec![]);
        let quad_glb = a.finish(&[n]);
        let mut b = Glb::new();
        let m0 = b
            .mesh(
                "t",
                &[Prim {
                    positions: &quad,
                    indices: &tri,
                    ..Default::default()
                }],
            )
            .unwrap();
        let mut roots = vec![];
        for v in 0..2 {
            let l0 = b.node(&format!("t_v{v}_LOD0"), Some(m0), vec![]);
            let l1 = b.node(&format!("t_v{v}_LOD1"), Some(m0), vec![]);
            roots.push(b.node(&format!("t_v{v}"), None, vec![l0, l1]));
        }
        let tree_glb = b.finish(&roots);
        let id = bevy::math::Mat4::IDENTITY.to_cols_array();
        let moved = bevy::math::Mat4::from_translation(bevy::math::Vec3::new(5.0, 0.0, 0.0))
            .to_cols_array();
        let out = merge(&[
            MergePart {
                name: "quad".into(),
                glb: &quad_glb,
                instances: vec![(0, id), (0, moved)],
            },
            MergePart {
                name: "tree".into(),
                glb: &tree_glb,
                instances: vec![(1, moved)],
            },
        ])
        .unwrap();
        let doc = glb_json(&out).unwrap();
        // meshes and the texture once, three placed copies at the top
        assert_eq!(doc["meshes"].as_array().unwrap().len(), 2);
        assert_eq!(doc["images"].as_array().unwrap().len(), 1);
        assert_eq!(doc["scenes"][0]["nodes"].as_array().unwrap().len(), 3);
        let nodes = doc["nodes"].as_array().unwrap();
        // the tree copy holds only its variant's first LOD, pointing at the tree's mesh
        let tree = nodes.iter().find(|n| n["name"] == "tree_0").unwrap();
        let kid = &nodes[tree["children"][0].as_u64().unwrap() as usize];
        assert_eq!(
            (kid["name"].as_str(), kid["mesh"].as_u64()),
            (Some("t_v1_LOD0"), Some(1))
        );
        assert_eq!(tree["children"].as_array().unwrap().len(), 1);
        assert_eq!(tree["matrix"][12], 5.0);
        // the quad's material still finds its texture, and the triangles survive
        assert_eq!(
            doc["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["index"],
            0
        );
        assert_eq!(triangles(&out), 2, "each mesh once");
    }
}
