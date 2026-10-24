//! glTF 2.0 / GLB reader. Images keep their original encoded bytes.

use super::{frame_count, sniff_mime, top4, Src, SrcAnim, SrcMesh, SrcNode, SrcSkin, SrcTrack, FPS};
use crate::model::*;
use ::gltf::animation::util::ReadOutputs;
use ::gltf::animation::{Interpolation, Property};
use ::gltf::image::Source;
use glam::{Mat4, Quat, Vec2, Vec3};
use std::path::Path;

pub(crate) fn load(path: &Path) -> Result<Src, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file = ::gltf::Gltf::from_slice(&bytes)
        .or_else(|_| ::gltf::Gltf::from_slice_without_validation(&bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let base = path.parent();
    let doc = file.document;
    let buffers = ::gltf::import_buffers(&doc, base, file.blob)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let buf = |b: ::gltf::Buffer| buffers.get(b.index()).map(|d| &d.0[..]);

    let mut src = Src::default();

    // Nodes and their parents.
    src.nodes = doc
        .nodes()
        .map(|n| SrcNode {
            name: n.name().map(str::to_owned).unwrap_or_else(|| format!("node{}", n.index())),
            parent: None,
            local: Mat4::from_cols_array_2d(&n.transform().matrix()),
        })
        .collect();
    for n in doc.nodes() {
        for c in n.children() {
            src.nodes[c.index()].parent = Some(n.index());
        }
    }

    // Images (PNG / JPEG only) and materials.
    let mut image_map: Vec<Option<usize>> = Vec::new();
    for img in doc.images() {
        let (data, mime): (Option<Vec<u8>>, Option<&str>) = match img.source() {
            Source::View { view, mime_type } => {
                let d = buf(view.buffer())
                    .and_then(|b| b.get(view.offset()..view.offset() + view.length()))
                    .map(<[u8]>::to_vec);
                (d, Some(mime_type))
            }
            Source::Uri { uri, mime_type } => (read_uri(uri, base), mime_type),
        };
        let entry = data.and_then(|bytes| {
            let mime = sniff_mime(&bytes)
                .or(mime.filter(|m| *m == "image/png" || *m == "image/jpeg"))?;
            src.images.push(Image {
                name: img.name().map(str::to_owned).unwrap_or_else(|| format!("image{}", img.index())),
                mime: mime.to_owned(),
                bytes,
            });
            Some(src.images.len() - 1)
        });
        image_map.push(entry);
    }
    let tex = |t: ::gltf::Texture| image_map.get(t.source().index()).copied().flatten().map(|image| TexRef { image });
    for m in doc.materials() {
        if m.index().is_none() {
            continue;
        }
        let pbr = m.pbr_metallic_roughness();
        let strength = m.emissive_strength().unwrap_or(1.0);
        let e = m.emissive_factor();
        src.materials.push(Material {
            name: m.name().unwrap_or_default().to_owned(),
            base_color: pbr.base_color_factor(),
            base_color_tex: pbr.base_color_texture().and_then(|i| tex(i.texture())),
            normal_tex: m.normal_texture().and_then(|i| tex(i.texture())),
            metallic_roughness_tex: pbr.metallic_roughness_texture().and_then(|i| tex(i.texture())),
            occlusion_tex: m.occlusion_texture().and_then(|i| tex(i.texture())),
            emissive: [e[0] * strength, e[1] * strength, e[2] * strength],
            emissive_tex: m.emissive_texture().and_then(|i| tex(i.texture())),
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            alpha: match m.alpha_mode() {
                ::gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                ::gltf::material::AlphaMode::Mask => AlphaMode::Mask,
                ::gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            },
            alpha_cutoff: m.alpha_cutoff().unwrap_or(0.5),
            double_sided: m.double_sided(),
        });
    }

    // Skins.
    for s in doc.skins() {
        let joints: Vec<usize> = s.joints().map(|j| j.index()).collect();
        let mut ibm: Vec<Mat4> = s
            .reader(buf)
            .read_inverse_bind_matrices()
            .map(|it| it.map(|m| Mat4::from_cols_array_2d(&m)).collect())
            .unwrap_or_default();
        ibm.resize(joints.len(), Mat4::IDENTITY);
        src.skins.push(SrcSkin { joints, inverse_bind: ibm });
    }

    // Meshes: one per triangle primitive of every node that has a mesh.
    let world = super::world_matrices(&src.nodes);
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        let prims: Vec<_> = mesh
            .primitives()
            .filter(|p| p.mode() == ::gltf::mesh::Mode::Triangles)
            .collect();
        let base_name = mesh.name().or(node.name()).unwrap_or("mesh").to_owned();
        for (pi, p) in prims.iter().enumerate() {
            let r = p.reader(buf);
            let Some(positions) = r.read_positions() else { continue };
            let positions: Vec<Vec3> = positions.map(Vec3::from).collect();
            let n = positions.len();
            let normals: Vec<Vec3> = r
                .read_normals()
                .map(|it| it.map(Vec3::from).collect())
                .filter(|v: &Vec<Vec3>| v.len() == n)
                .unwrap_or_default();
            let uvs: Vec<Vec2> = r
                .read_tex_coords(0)
                .map(|it| it.into_f32().map(Vec2::from).collect())
                .filter(|v: &Vec<Vec2>| v.len() == n)
                .unwrap_or_default();
            let indices: Vec<u32> = match r.read_indices() {
                Some(i) => i.into_u32().filter(|&i| (i as usize) < n).collect(),
                None => (0..n as u32).collect(),
            };
            let indices = indices[..indices.len() / 3 * 3].to_vec();
            let skin = match (node.skin(), r.read_joints(0), r.read_weights(0)) {
                (Some(s), Some(j), Some(w)) => {
                    let skin: Vec<VertexSkin> = j
                        .into_u16()
                        .zip(w.into_f32())
                        .map(|(j, w)| {
                            let mut inf: Vec<(u16, f32)> = (0..4).map(|k| (j[k], w[k])).collect();
                            top4(&mut inf).unwrap_or_default()
                        })
                        .collect();
                    (skin.len() == n).then_some((s.index(), skin))
                }
                _ => None,
            };
            src.meshes.push(SrcMesh {
                name: if prims.len() > 1 { format!("{base_name}.{pi}") } else { base_name.clone() },
                node: node.index(),
                to_world: world[node.index()],
                positions,
                normals,
                uvs,
                indices,
                material: p.material().index(),
                skin,
            });
        }
    }

    // Animations, resampled.
    for a in doc.animations() {
        struct Chan {
            node: usize,
            prop: Property,
            interp: Interpolation,
            times: Vec<f32>,
            values: Vec<[f32; 4]>,
        }
        let mut chans: Vec<Chan> = Vec::new();
        for c in a.channels() {
            let r = c.reader(buf);
            let (Some(times), Some(out)) = (r.read_inputs(), r.read_outputs()) else { continue };
            let times: Vec<f32> = times.collect();
            let values: Vec<[f32; 4]> = match out {
                ReadOutputs::Translations(it) => it.map(|v| [v[0], v[1], v[2], 0.0]).collect(),
                ReadOutputs::Scales(it) => it.map(|v| [v[0], v[1], v[2], 0.0]).collect(),
                ReadOutputs::Rotations(it) => it.into_f32().collect(),
                ReadOutputs::MorphTargetWeights(_) => continue,
            };
            let interp = c.sampler().interpolation();
            // CUBICSPLINE stores (in-tangent, value, out-tangent) per key: keep the values.
            let values: Vec<[f32; 4]> = if interp == Interpolation::CubicSpline {
                values.chunks_exact(3).map(|k| k[1]).collect()
            } else {
                values
            };
            if times.is_empty() || values.len() < times.len() {
                continue;
            }
            src.bone_hint.resize(src.nodes.len(), false);
            src.bone_hint[c.target().node().index()] = true;
            chans.push(Chan { node: c.target().node().index(), prop: c.target().property(), interp, times, values });
        }
        if chans.is_empty() {
            continue;
        }
        let duration = chans.iter().filter_map(|c| c.times.last().copied()).fold(0.0f32, f32::max);
        let frames = frame_count(duration);
        let mut tracks: Vec<SrcTrack> = Vec::new();
        for c in &chans {
            let ti = match tracks.iter().position(|t| t.node == c.node) {
                Some(i) => i,
                None => {
                    tracks.push(SrcTrack { node: c.node, ..Default::default() });
                    tracks.len() - 1
                }
            };
            let t = &mut tracks[ti];
            let sample = |f: usize| sample(&c.times, &c.values, c.interp, c.prop, f as f32 / FPS);
            match c.prop {
                Property::Rotation => t.rot = (0..frames).map(|f| Quat::from_array(sample(f)).normalize()).collect(),
                Property::Translation => t.pos = (0..frames).map(|f| Vec3::from_slice(&sample(f))).collect(),
                Property::Scale => t.scale = (0..frames).map(|f| Vec3::from_slice(&sample(f))).collect(),
                Property::MorphTargetWeights => {}
            }
        }
        src.anims.push(SrcAnim {
            name: a.name().map(str::to_owned).unwrap_or_else(|| format!("clip{}", a.index())),
            frames,
            tracks,
        });
    }
    Ok(src)
}

/// Value of a keyed channel at `t` (clamped to the keys' range).
fn sample(times: &[f32], values: &[[f32; 4]], interp: Interpolation, prop: Property, t: f32) -> [f32; 4] {
    let i = times.partition_point(|&k| k <= t);
    if i == 0 {
        return values[0];
    }
    if i >= times.len() {
        return values[times.len() - 1];
    }
    let (a, b) = (values[i - 1], values[i]);
    if interp == Interpolation::Step {
        return a;
    }
    let span = times[i] - times[i - 1];
    let f = if span > 0.0 { (t - times[i - 1]) / span } else { 0.0 };
    if prop == Property::Rotation {
        Quat::from_array(a).normalize().slerp(Quat::from_array(b).normalize(), f).to_array()
    } else {
        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * f)
    }
}

/// Bytes of an image uri: a base64 data uri or a file relative to the glTF.
fn read_uri(uri: &str, base: Option<&Path>) -> Option<Vec<u8>> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, data) = rest.split_once(";base64,")?;
        return base64_decode(data);
    }
    let rel = percent_decode(uri);
    let p = base.map(|b| b.join(&rel)).unwrap_or_else(|| rel.into());
    std::fs::read(p).ok()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Standard (and URL-safe) base64, padding optional.
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}
