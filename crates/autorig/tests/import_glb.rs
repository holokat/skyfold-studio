//! Import (glTF / FBX / OBJ) and GLB export tests. Tests on real files skip when the file
//! isn't on this machine.

use autorig::*;
use glam::{Mat4, Quat, Vec2, Vec3};
use std::path::{Path, PathBuf};

const EXT: &str = "/Users/k/Documents/Codex/2026-09-05/http-127-0-0-1-4174/work/external-animations";
const QUATERNIUS_ZIP: &str =
    "/Users/k/Documents/Codex/2026-09-05/http-127-0-0-1-4174/work/external-animations/quaternius-ual1-standard.zip";
/// Mixamo "Ch36" character, cm units, embedded textures, two stacks.
const MIXAMO_FBX: &str = "/Users/k/code/character-studio/data/assets/cd513462-2f27-416b-af4d-09d4e3542be2.fbx";
/// A ~1 m tall Mixamo-rigged Blender re-export (mesh nodes scaled 100).
const TRIPO_FBX: &str = "/Users/k/code/character-studio/data/assets/9f0f5c29-d4dd-4730-a387-ea72ac54a4a3.fbx";
/// Mixamo animation without skin (skeleton only).
const MIXAMO_ANIM_FBX: &str = "/Users/k/code/character-studio/data/assets/1547f47f-8528-4102-9c00-cc676934fe7e.fbx";

fn existing(p: &str) -> Option<PathBuf> {
    let p = PathBuf::from(p);
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("skipping: {} not found", p.display());
        None
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("autorig-tests");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn bone_world(m: &Model, name: &str) -> Vec3 {
    let sk = m.skeleton.as_ref().unwrap();
    let i = sk.find(name).unwrap_or_else(|| panic!("no bone {name}"));
    sk.rest_world()[i].w_axis.truncate()
}

/// Skinned vertices at rest: every bone's rest world × inverse bind should leave them in place.
fn max_bind_error(m: &Model) -> f32 {
    let Some(sk) = &m.skeleton else { return 0.0 };
    let w = sk.rest_world();
    let skin_m: Vec<Mat4> = w.iter().zip(&sk.bones).map(|(w, b)| *w * b.inverse_bind).collect();
    let mut err = 0.0f32;
    for mesh in &m.meshes {
        let Some(skin) = &mesh.skin else { continue };
        for (p, s) in mesh.positions.iter().zip(skin).step_by(7) {
            let mut q = Vec3::ZERO;
            for k in 0..4 {
                q += skin_m[s.joints[k] as usize].transform_point3(*p) * s.weights[k];
            }
            err = err.max(q.distance(*p));
        }
    }
    err
}

fn check_invariants(m: &Model) {
    if let Some(sk) = &m.skeleton {
        for (i, b) in sk.bones.iter().enumerate() {
            if let Some(p) = b.parent {
                assert!(p < i, "bone {} parent after child", b.name);
            }
        }
        for mesh in &m.meshes {
            if let Some(skin) = &mesh.skin {
                assert_eq!(skin.len(), mesh.positions.len());
                for s in skin {
                    let sum: f32 = s.weights.iter().sum();
                    assert!((sum - 1.0).abs() < 1e-3, "weights sum {sum}");
                    assert!(s.joints.iter().all(|&j| (j as usize) < sk.bones.len()));
                }
            }
        }
    }
    for mesh in &m.meshes {
        assert_eq!(mesh.normals.len(), mesh.positions.len());
        assert!(mesh.uvs.is_empty() || mesh.uvs.len() == mesh.positions.len());
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.positions.len()));
        if let Some(mi) = mesh.material {
            assert!(mi < m.materials.len());
        }
    }
    for c in &m.clips {
        assert_eq!(c.fps, 30.0);
        for t in &c.tracks {
            for len in [t.rot.len(), t.pos.len(), t.scale.len()] {
                assert!(len == 0 || len == c.frames, "clip {} track len {len} != {}", c.name, c.frames);
            }
        }
    }
    for mat in &m.materials {
        for t in [mat.base_color_tex, mat.normal_tex, mat.metallic_roughness_tex, mat.occlusion_tex, mat.emissive_tex]
            .into_iter()
            .flatten()
        {
            assert!(t.image < m.images.len());
        }
    }
    for img in &m.images {
        assert!(img.mime == "image/png" || img.mime == "image/jpeg");
    }
}

fn summary(label: &str, m: &Model) {
    let (lo, hi) = m.bounds().unwrap_or_default();
    eprintln!(
        "{label}: {} meshes, {} verts, {} tris, {} materials, {} images, {} bones, {} clips, bounds {lo} .. {hi}, bind err {:.5}",
        m.meshes.len(),
        m.vertex_count(),
        m.triangle_count(),
        m.materials.len(),
        m.images.len(),
        m.skeleton.as_ref().map_or(0, |s| s.bones.len()),
        m.clips.len(),
        max_bind_error(m),
    );
}

/// A two-bone skinned quad strip with a material, an image and a clip.
fn synthetic() -> Model {
    let mut positions = Vec::new();
    let mut skin = Vec::new();
    let mut uvs = Vec::new();
    for i in 0..4 {
        let y = i as f32 * 0.5;
        for x in [-0.1f32, 0.1] {
            positions.push(Vec3::new(x, y, 0.0));
            uvs.push(Vec2::new((x + 0.1) * 5.0, y / 1.5));
            let w = (y / 1.5).clamp(0.0, 1.0);
            skin.push(VertexSkin { joints: [0, 1, 0, 0], weights: [1.0 - w, w, 0.0, 0.0] });
        }
    }
    let mut indices = Vec::new();
    for i in 0..3u32 {
        let a = i * 2;
        indices.extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
    }
    let mut skeleton = Skeleton {
        bones: vec![
            Bone { name: "root".into(), parent: None, rest: Xform::IDENTITY, inverse_bind: Mat4::IDENTITY },
            Bone {
                name: "child".into(),
                parent: Some(0),
                rest: Xform { t: Vec3::new(0.0, 0.75, 0.0), ..Xform::IDENTITY },
                inverse_bind: Mat4::IDENTITY,
            },
        ],
    };
    skeleton.bind_to_rest();
    // 1x1 PNG
    let png: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1,
        8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF,
        0xC0, 0xF0, 0x1F, 0, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44,
        0xAE, 0x42, 0x60, 0x82,
    ];
    let frames = 10;
    let clip = Clip {
        name: "wave".into(),
        fps: 30.0,
        frames,
        tracks: vec![Track {
            bone: 1,
            rot: (0..frames).map(|f| Quat::from_rotation_z(f as f32 * 0.1)).collect(),
            pos: vec![],
            scale: vec![],
        }],
    };
    let mut m = Model {
        meshes: vec![Mesh {
            name: "strip".into(),
            positions,
            normals: vec![],
            uvs,
            indices,
            material: Some(0),
            skin: Some(skin),
        }],
        materials: vec![Material {
            name: "skin".into(),
            base_color_tex: Some(TexRef { image: 0 }),
            emissive: [2.0, 1.0, 0.0],
            alpha: AlphaMode::Mask,
            alpha_cutoff: 0.3,
            double_sided: true,
            ..Default::default()
        }],
        images: vec![Image { name: "px".into(), mime: "image/png".into(), bytes: png }],
        skeleton: Some(skeleton),
        clips: vec![clip],
    };
    m.ensure_normals();
    m
}

fn write_and_reload(m: &Model, name: &str) -> Model {
    let bytes = glb::write(m, &m.clips);
    // The gltf crate validates its own import.
    gltf::Gltf::from_slice(&bytes).expect("valid glTF");
    let path = scratch(name);
    std::fs::write(&path, &bytes).unwrap();
    import::load(&path).expect("reload")
}

#[test]
fn synthetic_round_trip() {
    let m = synthetic();
    let bytes = glb::write(&m, &m.clips);
    let full = gltf::import_slice(&bytes).expect("gltf import");
    assert_eq!(full.0.skins().next().unwrap().joints().count(), 2);
    let r = write_and_reload(&m, "synthetic.glb");
    check_invariants(&r);
    assert_eq!(r.vertex_count(), m.vertex_count());
    assert_eq!(r.triangle_count(), m.triangle_count());
    let (a, b) = (m.skeleton.as_ref().unwrap(), r.skeleton.as_ref().unwrap());
    assert_eq!(b.bones.len(), a.bones.len());
    for (x, y) in a.bones.iter().zip(&b.bones) {
        assert_eq!(x.name, y.name);
        assert_eq!(x.parent, y.parent);
        assert!(x.rest.t.distance(y.rest.t) < 1e-5);
        assert!(x.inverse_bind.abs_diff_eq(y.inverse_bind, 1e-5));
    }
    let (sa, sb) = (m.meshes[0].skin.as_ref().unwrap(), r.meshes[0].skin.as_ref().unwrap());
    for (p, x) in m.meshes[0].positions.iter().zip(sa) {
        let q = r.meshes[0].positions.iter().position(|q| q.distance(*p) < 1e-6).unwrap();
        let wa: f32 = (0..4).filter(|&k| x.joints[k] == 1).map(|k| x.weights[k]).sum();
        let wb: f32 = (0..4).filter(|&k| sb[q].joints[k] == 1).map(|k| sb[q].weights[k]).sum();
        assert!((wa - wb).abs() < 1e-5);
    }
    assert!(r.meshes[0].uvs[3].distance(m.meshes[0].uvs[3]) < 1e-6);
    assert_eq!(r.images.len(), 1);
    assert_eq!(r.images[0].bytes, m.images[0].bytes);
    let mat = &r.materials[0];
    assert_eq!(mat.base_color_tex, Some(TexRef { image: 0 }));
    assert_eq!(mat.alpha, AlphaMode::Mask);
    assert!((mat.alpha_cutoff - 0.3).abs() < 1e-6);
    assert!(mat.double_sided);
    assert!((mat.emissive[0] - 2.0).abs() < 1e-5 && (mat.emissive[1] - 1.0).abs() < 1e-5);
    assert_eq!(r.clips.len(), 1);
    let c = &r.clips[0];
    assert_eq!((c.name.as_str(), c.frames), ("wave", 10));
    let t = c.tracks.iter().find(|t| t.bone == 1).unwrap();
    assert!(t.rot[9].angle_between(Quat::from_rotation_z(0.9)) < 1e-4);
}

#[test]
fn ual1_glb() {
    let Some(p) = existing(&format!("{EXT}/UAL1_Standard.glb")) else { return };
    let m = import::load(&p).unwrap();
    summary("UAL1 glb", &m);
    check_invariants(&m);
    let sk = m.skeleton.as_ref().unwrap();
    assert_eq!(sk.bones.len(), 65, "{:?}", sk.bones.iter().map(|b| &b.name).collect::<Vec<_>>());
    assert!(sk.find("pelvis").is_some() && sk.find("hand_l").is_some());
    assert_eq!(m.clips.len(), 43);
    let (lo, hi) = m.bounds().unwrap();
    assert!((hi.y - lo.y - 1.83).abs() < 0.05, "height {}", hi.y - lo.y);
    let hand = bone_world(&m, "hand_l");
    assert!((hand.x - 0.74).abs() < 0.03, "hand_l {hand}");
    assert!(max_bind_error(&m) < 1e-3);

    // Round trip through our writer.
    let r = write_and_reload(&m, "ual1.glb");
    check_invariants(&r);
    assert_eq!(r.vertex_count(), m.vertex_count());
    assert_eq!(r.skeleton.as_ref().unwrap().bones.len(), 65);
    assert_eq!(r.clips.len(), 43);
    for (a, b) in m.clips.iter().zip(&r.clips) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.frames, b.frames);
    }
    assert!(bone_world(&r, "hand_l").distance(hand) < 1e-4);
    assert!(max_bind_error(&r) < 1e-3);
}

#[test]
fn mannequin_glb() {
    let Some(p) = existing(&format!("{EXT}/Mannequin_F.glb")) else { return };
    let m = import::load(&p).unwrap();
    summary("Mannequin_F glb", &m);
    check_invariants(&m);
    assert!(m.vertex_count() > 0);
    assert!(m.skeleton.is_some());
    assert!(max_bind_error(&m) < 1e-3);
    let r = write_and_reload(&m, "mannequin.glb");
    check_invariants(&r);
    assert_eq!(r.vertex_count(), m.vertex_count());
}

/// Unzips the Quaternius pack once into the temp dir; returns the Unity FBX.
fn quaternius_fbx() -> Option<PathBuf> {
    existing(QUATERNIUS_ZIP)?;
    let dir = scratch("quaternius");
    let fbx = dir.join("Universal Animation Library[Standard]/Unity/UAL1_Standard.fbx");
    if !fbx.is_file() {
        let ok = std::process::Command::new("unzip")
            .args(["-o", "-q", QUATERNIUS_ZIP, "-d"])
            .arg(&dir)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("skipping: unzip failed");
            return None;
        }
    }
    fbx.is_file().then_some(fbx)
}

#[test]
fn ual1_fbx_matches_glb() {
    let (Some(fbx), Some(glb)) = (quaternius_fbx(), existing(&format!("{EXT}/UAL1_Standard.glb"))) else { return };
    let f = import::load(&fbx).unwrap();
    let g = import::load(&glb).unwrap();
    summary("UAL1 fbx", &f);
    check_invariants(&f);
    let (flo, fhi) = f.bounds().unwrap();
    let (glo, ghi) = g.bounds().unwrap();
    eprintln!("fbx bounds {flo} {fhi}; glb {glo} {ghi}");
    assert!(((fhi.y - flo.y) - (ghi.y - glo.y)).abs() < 0.03);
    let fs = f.skeleton.as_ref().unwrap();
    let mut compared = 0;
    for name in ["pelvis", "hand_l", "hand_r", "head", "foot_l", "lowerarm_r", "calf_l"] {
        let (Some(_), Some(_)) = (fs.find(name), g.skeleton.as_ref().unwrap().find(name)) else { continue };
        let (a, b) = (bone_world(&f, name), bone_world(&g, name));
        eprintln!("{name}: fbx {a} glb {b}");
        assert!(a.distance(b) < 0.03, "{name}: fbx {a} vs glb {b}");
        compared += 1;
    }
    assert!(compared >= 5);
    assert!(max_bind_error(&f) < 1e-3, "bind err {}", max_bind_error(&f));
    eprintln!("fbx clips: {:?}", f.clips.iter().map(|c| (&c.name, c.frames)).take(6).collect::<Vec<_>>());
    assert!(!f.clips.is_empty());

    // Same clip, same frame → same pose (model-space hand position).
    let pose = |m: &Model, clip: &Clip, frame: usize, bone: &str| -> Vec3 {
        let sk = m.skeleton.as_ref().unwrap();
        let mut local: Vec<Xform> = sk.bones.iter().map(|b| b.rest).collect();
        for t in &clip.tracks {
            if let Some(r) = t.rot.get(frame) {
                local[t.bone].r = *r;
            }
            if let Some(p) = t.pos.get(frame) {
                local[t.bone].t = *p;
            }
            if let Some(s) = t.scale.get(frame) {
                local[t.bone].s = *s;
            }
        }
        let mut w: Vec<Mat4> = Vec::new();
        for (i, b) in sk.bones.iter().enumerate() {
            let m = local[i].matrix();
            w.push(b.parent.map_or(m, |p| w[p] * m));
        }
        w[sk.find(bone).unwrap()].w_axis.truncate()
    };
    let mut matched = 0;
    for gc in &g.clips {
        let Some(fc) = f.clips.iter().find(|c| c.name == gc.name) else { continue };
        let fr = (gc.frames.min(fc.frames)) / 2;
        let (a, b) = (pose(&f, fc, fr, "hand_r"), pose(&g, gc, fr, "hand_r"));
        assert!(a.distance(b) < 0.05, "{} frame {fr}: fbx {a} glb {b}", gc.name);
        matched += 1;
    }
    eprintln!("matched {matched} clips by name");
    assert!(matched > 0, "no clip names in common");
}

fn check_mixamo(path: &str, label: &str, height: (f32, f32)) -> Option<Model> {
    let p = existing(path)?;
    let m = import::load(&p).unwrap();
    summary(label, &m);
    check_invariants(&m);
    let sk = m.skeleton.as_ref().unwrap();
    let hips = sk.bones.iter().find(|b| b.name.ends_with("Hips")).expect("Hips bone");
    assert!(hips.name.starts_with("mixamorig"), "{}", hips.name);
    let (lo, hi) = m.bounds().unwrap();
    let h = hi.y - lo.y;
    assert!(h > height.0 && h < height.1, "height {h} m");
    for b in &sk.bones {
        assert!(b.rest.s.abs_diff_eq(Vec3::ONE, 0.02), "{} scale {}", b.name, b.rest.s);
    }
    let lh = sk.bones.iter().find(|b| b.name.ends_with("LeftHand")).unwrap();
    let lh = bone_world(&m, &lh.name.clone());
    assert!(lh.x > 0.1, "left hand on +X: {lh}");
    assert!(max_bind_error(&m) < 2e-3, "bind err {}", max_bind_error(&m));
    for img in &m.images {
        eprintln!("  image {} {} {} bytes", img.name, img.mime, img.bytes.len());
    }
    eprintln!("  clips {:?}", m.clips.iter().map(|c| (&c.name, c.frames, c.tracks.len())).collect::<Vec<_>>());
    let r = write_and_reload(&m, &format!("{label}.glb"));
    check_invariants(&r);
    assert_eq!(r.vertex_count(), m.vertex_count());
    assert_eq!(r.images.len(), m.images.len());
    assert_eq!(r.clips.len(), m.clips.len());
    assert!(max_bind_error(&r) < 2e-3);
    Some(m)
}

#[test]
fn mixamo_character_fbx() {
    let Some(m) = check_mixamo(MIXAMO_FBX, "mixamo_ch36", (1.6, 1.9)) else { return };
    assert!(!m.images.is_empty(), "embedded textures");
    assert!(m.materials.iter().any(|t| t.base_color_tex.is_some()));
    assert!(m.clips.iter().any(|c| c.name == "cd513462-2f27-416b-af4d-09d4e3542be2"));
}

#[test]
fn mixamo_reexport_fbx() {
    check_mixamo(TRIPO_FBX, "mixamo_tripo", (0.9, 1.1));
}

#[test]
fn mixamo_animation_only_fbx() {
    let Some(p) = existing(MIXAMO_ANIM_FBX) else { return };
    let m = import::load(&p).unwrap();
    summary("mixamo anim", &m);
    check_invariants(&m);
    let sk = m.skeleton.as_ref().expect("skeleton from limb nodes");
    assert!(sk.bones.len() >= 50, "{} bones", sk.bones.len());
    let hips = sk.bones.iter().position(|b| b.name.ends_with("Hips")).unwrap();
    let y = sk.rest_world()[hips].w_axis.y;
    assert!(y > 0.8 && y < 1.2, "hips at {y} m");
    assert_eq!(m.clips.len(), 1);
    let c = &m.clips[0];
    assert_eq!(c.name, "1547f47f-8528-4102-9c00-cc676934fe7e");
    assert_eq!(c.frames, 48);
    let t = c.tracks.iter().find(|t| t.bone == hips).unwrap();
    assert_eq!(t.pos.len(), 48);
    assert!(t.pos.iter().all(|p| p.y > 0.5 && p.y < 1.5), "hips track in metres");
}

#[test]
fn obj_with_mtl() {
    // A tiny OBJ + MTL + PNG written to the temp dir.
    let dir = scratch("obj");
    std::fs::create_dir_all(&dir).unwrap();
    let png = synthetic().images[0].bytes.clone();
    std::fs::write(dir.join("px.png"), png).unwrap();
    std::fs::write(dir.join("quad.mtl"), "newmtl red\nKd 1 0 0\nmap_Kd px.png\n").unwrap();
    std::fs::write(
        dir.join("quad.obj"),
        "mtllib quad.mtl\no quad\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nusemtl red\nf 1/1 2/2 3/3 4/4\n",
    )
    .unwrap();
    let m = import::load(&dir.join("quad.obj")).unwrap();
    summary("obj", &m);
    check_invariants(&m);
    assert_eq!(m.triangle_count(), 2);
    assert_eq!(m.vertex_count(), 4);
    assert!(m.skeleton.is_none());
    assert_eq!(m.materials.len(), 1);
    assert_eq!(m.materials[0].base_color_tex, Some(TexRef { image: 0 }));
    let (lo, hi) = m.bounds().unwrap();
    assert!(lo.abs_diff_eq(Vec3::ZERO, 1e-5) && hi.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-5), "{lo} {hi}");
}

#[test]
fn unsupported_extension() {
    assert!(import::load(Path::new("/nonexistent/thing.xyz")).is_err());
    assert!(import::load(Path::new("/nonexistent/thing.glb")).is_err());
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// A .gltf (data-uri buffer) with two skins on disjoint joints: the bigger one is kept and
/// the other mesh is frozen, unskinned, where its rest pose puts it.
#[test]
fn disjoint_skins_keep_largest() {
    let mut bin: Vec<u8> = Vec::new();
    let f = |bin: &mut Vec<u8>, v: &[f32]| bin.extend(v.iter().flat_map(|x| x.to_le_bytes()));
    f(&mut bin, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]); // 0: positions, 36 B
    for _ in 0..3 {
        bin.extend([0u16, 1, 0, 0].iter().flat_map(|x| x.to_le_bytes())); // 36: joints, 24 B
    }
    for _ in 0..3 {
        f(&mut bin, &[0.5, 0.5, 0.0, 0.0]); // 60: weights, 48 B
    }
    let id = Mat4::IDENTITY.to_cols_array();
    for _ in 0..3 {
        f(&mut bin, &id); // 108: 3 inverse binds, 192 B
    }
    let gltf = serde_json::json!({
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": [0, 2, 3, 4] }],
        "nodes": [
            { "name": "a0", "children": [1] },
            { "name": "a1", "translation": [0.0, 0.0, 0.0] },
            { "name": "b0", "translation": [1.0, 0.0, 0.0] },
            { "name": "meshA", "mesh": 0, "skin": 0 },
            { "name": "meshB", "mesh": 1, "skin": 1 }
        ],
        "buffers": [{ "byteLength": bin.len(), "uri": format!("data:application/octet-stream;base64,{}", base64(&bin)) }],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
            { "buffer": 0, "byteOffset": 36, "byteLength": 24 },
            { "buffer": 0, "byteOffset": 60, "byteLength": 48 },
            { "buffer": 0, "byteOffset": 108, "byteLength": 128 },
            { "buffer": 0, "byteOffset": 236, "byteLength": 64 }
        ],
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0] },
            { "bufferView": 1, "componentType": 5123, "count": 3, "type": "VEC4" },
            { "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4" },
            { "bufferView": 3, "componentType": 5126, "count": 2, "type": "MAT4" },
            { "bufferView": 4, "componentType": 5126, "count": 1, "type": "MAT4" }
        ],
        "meshes": [
            { "primitives": [{ "attributes": { "POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2 } }] },
            { "primitives": [{ "attributes": { "POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2 } }] }
        ],
        "skins": [
            { "joints": [0, 1], "inverseBindMatrices": 3 },
            { "joints": [2], "inverseBindMatrices": 4 }
        ]
    });
    let path = scratch("two_skins.gltf");
    std::fs::write(&path, serde_json::to_vec(&gltf).unwrap()).unwrap();
    let m = import::load(&path).unwrap();
    check_invariants(&m);
    let sk = m.skeleton.as_ref().unwrap();
    assert_eq!(sk.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["a0", "a1"]);
    assert_eq!(m.meshes.len(), 2);
    assert!(m.meshes[0].skin.is_some());
    assert!(m.meshes[1].skin.is_none());
    assert!(m.meshes[1].positions[0].abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-6));
    assert_eq!(m.meshes[0].normals.len(), 3, "normals generated");
}
