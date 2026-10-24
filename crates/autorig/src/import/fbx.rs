//! FBX and OBJ reader through ufbx, converted on load to Y-up right-handed metres.

use super::{frame_count, sniff_mime, top4, Src, SrcAnim, SrcMesh, SrcNode, SrcSkin, SrcTrack, FPS};
use crate::model::*;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn mat4(m: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(m.m00 as f32, m.m10 as f32, m.m20 as f32, 0.0),
        Vec4::new(m.m01 as f32, m.m11 as f32, m.m21 as f32, 0.0),
        Vec4::new(m.m02 as f32, m.m12 as f32, m.m22 as f32, 0.0),
        Vec4::new(m.m03 as f32, m.m13 as f32, m.m23 as f32, 1.0),
    )
}

fn v3(v: ufbx::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

fn quat(q: ufbx::Quat) -> Quat {
    Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32).normalize()
}

pub(crate) fn load(path: &Path) -> Result<Src, String> {
    let name = path.to_str().ok_or("path is not UTF-8")?;
    let y_up = ufbx::CoordinateAxes::right_handed_y_up();
    let opts = ufbx::LoadOpts {
        target_axes: y_up,
        target_unit_meters: 1.0,
        // Scale the geometry and translations themselves, so bones keep unit scale.
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        inherit_mode_handling: ufbx::InheritModeHandling::Compensate,
        generate_missing_normals: true,
        load_external_files: true,
        ignore_missing_external_files: true,
        obj_unit_meters: 1.0,
        obj_axes: y_up,
        ..Default::default()
    };
    let scene = ufbx::load_file(name, opts).map_err(|e| format!("{}: {}", path.display(), e.description))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("clip").to_owned();

    let mut src = Src::default();
    src.nodes = scene
        .nodes
        .iter()
        .map(|n| SrcNode {
            name: n.element.name.to_string(),
            parent: n.parent.as_ref().map(|p| p.element.typed_id as usize),
            local: mat4(&n.node_to_parent),
        })
        .collect();
    src.bone_hint = scene.nodes.iter().map(|n| n.bone.is_some()).collect();

    let mut mats = Materials { dir, by_material: HashMap::new(), by_texture: HashMap::new(), by_path: HashMap::new() };
    let mut skin_of: HashMap<u32, (usize, Vec<Option<u16>>)> = HashMap::new();

    for node in scene.nodes.iter() {
        let Some(mesh) = node.mesh.as_ref() else { continue };
        let mesh: &ufbx::Mesh = mesh;
        if mesh.num_faces == 0 || !mesh.vertex_position.exists {
            continue;
        }

        // Skin (first deformer) → a SrcSkin shared by every instance.
        let skin = mesh.skin_deformers.first().map(|d| {
            let d: &ufbx::SkinDeformer = d;
            let (si, cmap) = skin_of
                .entry(d.element.element_id)
                .or_insert_with(|| {
                    let mut joints = Vec::new();
                    let mut ibm = Vec::new();
                    let cmap = d
                        .clusters
                        .iter()
                        .map(|c| {
                            let b = c.bone_node.as_ref()?;
                            joints.push(b.element.typed_id as usize);
                            ibm.push(mat4(&c.geometry_to_bone));
                            Some((joints.len() - 1) as u16)
                        })
                        .collect();
                    src.skins.push(SrcSkin { joints, inverse_bind: ibm });
                    (src.skins.len() - 1, cmap)
                })
                .clone();
            (d, si, cmap)
        });

        let parts: Vec<(u32, Vec<u32>)> = if mesh.material_parts.is_empty() {
            vec![(0, (0..mesh.num_faces as u32).collect())]
        } else {
            mesh.material_parts.iter().map(|p| (p.index, p.face_indices.to_vec())).collect()
        };
        let multi = parts.iter().filter(|p| !p.1.is_empty()).count() > 1;
        let mut tri = vec![0u32; mesh.max_face_triangles.max(1) * 3];
        for (pi, (mat_index, faces)) in parts.iter().enumerate() {
            if faces.is_empty() {
                continue;
            }
            let material = node
                .materials
                .get(*mat_index as usize)
                .or_else(|| mesh.materials.get(*mat_index as usize))
                .map(|m| mats.material(m, &mut src));
            let mut out = SrcMesh {
                name: if multi { format!("{}.{pi}", node.element.name) } else { node.element.name.to_string() },
                node: node.element.typed_id as usize,
                to_world: mat4(&node.geometry_to_world),
                material,
                ..Default::default()
            };
            let mut weights: Vec<VertexSkin> = Vec::new();
            // Corners merge when position, normal and uv agree (by value: FBX often stores
            // normals and uvs per corner).
            let mut seen: HashMap<(u32, [u32; 5]), u32> = HashMap::new();
            let attr = |a: &ufbx::List<u32>, c: usize| a.get(c).copied().unwrap_or(0) as usize;
            for &fi in faces {
                let face = mesh.faces[fi as usize];
                let n = mesh.triangulate_face(&mut tri, face) as usize;
                for &c in &tri[..n * 3] {
                    let c = c as usize;
                    let pi = attr(&mesh.vertex_position.indices, c);
                    let nrm = if mesh.vertex_normal.exists {
                        mesh.vertex_normal.values.get(attr(&mesh.vertex_normal.indices, c)).map(|v| v3(*v))
                    } else {
                        None
                    };
                    // FBX / OBJ uv origin is bottom-left; glTF's is top-left.
                    let uv = if mesh.vertex_uv.exists {
                        mesh.vertex_uv
                            .values
                            .get(attr(&mesh.vertex_uv.indices, c))
                            .map(|v| Vec2::new(v.x as f32, 1.0 - v.y as f32))
                    } else {
                        None
                    };
                    let (nv, uvv) = (nrm.unwrap_or(Vec3::ZERO), uv.unwrap_or(Vec2::ZERO));
                    let key = (pi as u32, [nv.x, nv.y, nv.z, uvv.x, uvv.y].map(f32::to_bits));
                    let next = out.positions.len() as u32;
                    let vi = *seen.entry(key).or_insert(next);
                    if vi == next {
                        out.positions.push(v3(mesh.vertex_position.values[pi]));
                        if mesh.vertex_normal.exists {
                            out.normals.push(nv);
                        }
                        if mesh.vertex_uv.exists {
                            out.uvs.push(uvv);
                        }
                        if let Some((d, _, cmap)) = &skin {
                            let v = mesh.vertex_indices[c] as usize;
                            let mut inf: Vec<(u16, f32)> = Vec::new();
                            if let Some(sv) = d.vertices.get(v) {
                                let b = sv.weight_begin as usize;
                                for w in d.weights.get(b..b + sv.num_weights as usize).unwrap_or(&[]) {
                                    if let Some(Some(j)) = cmap.get(w.cluster_index as usize) {
                                        inf.push((*j, w.weight as f32));
                                    }
                                }
                            }
                            weights.push(top4(&mut inf).unwrap_or_default());
                        }
                    }
                    out.indices.push(vi);
                }
            }
            if let Some((_, si, _)) = &skin {
                out.skin = Some((*si, weights));
            }
            src.meshes.push(out);
        }
    }

    // Animation stacks, sampled on every node at or above a skin joint.
    let mut sampled = vec![false; src.nodes.len()];
    let skin_joints = src.skins.iter().flat_map(|s| s.joints.iter().copied());
    let hinted = (0..src.nodes.len()).filter(|&i| src.skins.is_empty() && src.bone_hint[i]);
    let starts: Vec<usize> = skin_joints.chain(hinted).collect();
    {
        for j in starts {
            let mut p = Some(j);
            while let Some(i) = p {
                if sampled[i] {
                    break;
                }
                sampled[i] = true;
                p = src.nodes[i].parent;
            }
        }
    }
    let mut names: HashMap<String, usize> = HashMap::new();
    for stack in scene.anim_stacks.iter() {
        let keyed = stack.layers.iter().any(|l| !l.anim_values.is_empty());
        if !keyed {
            continue;
        }
        let (t0, t1) = (stack.time_begin, stack.time_end.max(stack.time_begin));
        let frames = frame_count((t1 - t0) as f32);
        let mut tracks = Vec::new();
        for (i, node) in scene.nodes.iter().enumerate() {
            if !sampled[i] || node.is_root {
                continue;
            }
            let mut t = SrcTrack { node: i, ..Default::default() };
            for f in 0..frames {
                let time = t0 + f as f64 / FPS as f64;
                let x = ufbx::evaluate_transform(&stack.anim, node, time.min(t1));
                t.rot.push(quat(x.rotation));
                t.pos.push(v3(x.translation));
                t.scale.push(v3(x.scale));
            }
            if t.scale.iter().all(|s| s.abs_diff_eq(Vec3::ONE, 1e-4)) {
                t.scale.clear();
            }
            tracks.push(t);
        }
        let raw = stack.element.name.to_string();
        let mut name = raw.rsplit('|').next().unwrap_or(&raw).trim().to_owned();
        if name.is_empty() || name == "mixamo.com" {
            name = stem.clone();
        }
        let n = names.entry(name.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            name = format!("{name} {}", *n);
        }
        src.anims.push(SrcAnim { name, frames, tracks });
    }
    Ok(src)
}

/// Material and texture conversion with caches.
struct Materials<'a> {
    dir: &'a Path,
    by_material: HashMap<u32, usize>,
    by_texture: HashMap<u32, Option<usize>>,
    by_path: HashMap<PathBuf, Option<usize>>,
}

impl Materials<'_> {
    fn material(&mut self, m: &ufbx::Material, src: &mut Src) -> usize {
        if let Some(&i) = self.by_material.get(&m.element.element_id) {
            return i;
        }
        let pbr = &m.pbr;
        let mut tex = |map: &ufbx::MaterialMap| -> Option<TexRef> {
            let t = map.texture.as_ref()?;
            self.texture(t, src).map(|image| TexRef { image })
        };
        let base_color_tex = tex(&pbr.base_color).or_else(|| tex(&m.fbx.diffuse_color));
        let normal_tex = tex(&pbr.normal_map).or_else(|| tex(&m.fbx.normal_map));
        let emissive_tex = tex(&pbr.emission_color);
        let occlusion_tex = tex(&pbr.ambient_occlusion);
        let val = |map: &ufbx::MaterialMap, d: f32| if map.has_value { map.value_vec4.x as f32 } else { d };
        let c = pbr.base_color.value_vec4;
        let k = val(&pbr.base_factor, 1.0);
        let mut base = if base_color_tex.is_some() {
            [1.0, 1.0, 1.0, 1.0]
        } else if pbr.base_color.has_value {
            [c.x as f32 * k, c.y as f32 * k, c.z as f32 * k, 1.0]
        } else {
            [0.8, 0.8, 0.8, 1.0]
        };
        let opacity = val(&pbr.opacity, 1.0).clamp(0.0, 1.0);
        base[3] = opacity;
        let e = pbr.emission_color.value_vec4;
        let ek = val(&pbr.emission_factor, 1.0);
        let emissive = if pbr.emission_color.has_value {
            [e.x as f32 * ek, e.y as f32 * ek, e.z as f32 * ek]
        } else {
            [0.0; 3]
        };
        src.materials.push(Material {
            name: m.element.name.to_string(),
            base_color: base,
            base_color_tex,
            normal_tex,
            occlusion_tex,
            emissive,
            emissive_tex,
            metallic: val(&pbr.metalness, 0.0).clamp(0.0, 1.0),
            roughness: val(&pbr.roughness, 0.8).clamp(0.0, 1.0),
            alpha: if opacity < 0.999 { AlphaMode::Blend } else { AlphaMode::Opaque },
            ..Default::default()
        });
        let i = src.materials.len() - 1;
        self.by_material.insert(m.element.element_id, i);
        i
    }

    /// The texture's image: embedded content, else a PNG / JPEG file found beside the model.
    fn texture(&mut self, t: &ufbx::Texture, src: &mut Src) -> Option<usize> {
        if let Some(&i) = self.by_texture.get(&t.element.element_id) {
            return i;
        }
        let file: &ufbx::Texture = t.file_textures.first().map(|f| &**f).unwrap_or(t);
        let name = file.element.name.to_string();
        let result = if !file.content.is_empty() {
            push_image(src, name, file.content.to_vec())
        } else {
            let base = |s: &str| s.rsplit(['/', '\\']).next().unwrap_or(s).to_owned();
            let mut cands: Vec<PathBuf> = Vec::new();
            for f in [&file.absolute_filename, &file.filename] {
                if !f.is_empty() {
                    cands.push(PathBuf::from(f.to_string()));
                }
            }
            if !file.relative_filename.is_empty() {
                cands.push(self.dir.join(file.relative_filename.to_string().replace('\\', "/")));
            }
            for f in [&file.filename, &file.relative_filename] {
                if !f.is_empty() {
                    let b = base(f);
                    cands.push(self.dir.join(&b));
                    for sub in ["textures", "Textures", "tex", "images"] {
                        cands.push(self.dir.join(sub).join(&b));
                    }
                }
            }
            match cands.into_iter().find(|p| p.is_file()) {
                Some(p) => {
                    if let Some(&i) = self.by_path.get(&p) {
                        i
                    } else {
                        let i = std::fs::read(&p).ok().and_then(|b| push_image(src, name, b));
                        self.by_path.insert(p, i);
                        i
                    }
                }
                None => None,
            }
        };
        self.by_texture.insert(t.element.element_id, result);
        result
    }
}

fn push_image(src: &mut Src, name: String, bytes: Vec<u8>) -> Option<usize> {
    let mime = sniff_mime(&bytes)?;
    src.images.push(Image { name, mime: mime.to_owned(), bytes });
    Some(src.images.len() - 1)
}
