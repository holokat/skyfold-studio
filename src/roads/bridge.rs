//! Procedural bridges, built piece by piece along the road's own path (so curved and sloping
//! crossings work) and textured with photoscanned CC0 materials (`assets/textures/materials`):
//!
//! - Timber trestle (trails, paths, dirt and gravel roads): individual bevelled deck planks with
//!   jittered size, gaps, angle and tone; stringers under them; bents of posts driven into the
//!   river bed with cap beams and cross bracing; railings with posts, top and mid rails.
//! - Stone arch bridge (cobbled and paved roads): segmental arches between piers with pointed
//!   cutwaters, a dressed-stone arch ring, barrel vaults, spandrel walls, parapets with bevelled
//!   coping and end blocks. The road surface itself continues over the deck.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{
    ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor,
};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::plan::{RoadPlan, Span};
use crate::noise::Rng;
use crate::terrain::Terrain;

/// Abutment length at each end of a stone bridge and the width of its piers (m).
const ABUT: f32 = 1.8;
const PIER: f32 = 1.7;
/// Parapet height above the deck and wall thickness (m).
const PARAPET_H: f32 = 0.9;
const PARAPET_T: f32 = 0.45;
/// The largest radial height of a voussoir above an arch intrados: its 0.55 m ring plus the
/// deterministic 0.12 m jitter, with a small clearance under the deck.
const ARCH_RING_CLEARANCE: f32 = 0.70;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot {
    /// deck planks
    Planks,
    /// beams, posts, rails
    Timber,
    /// walls, vaults, piers
    Stone,
    /// arch ring, coping, end blocks
    Dressed,
}

/// Material id and texture repeat (m) per slot.
const SLOTS: [(Slot, &str, f32); 4] = [
    (Slot::Planks, "wood_rough_sawn_dark", 1.0),
    (Slot::Timber, "wood_rough_sawn_dark", 1.2),
    (Slot::Stone, "stone_castle_wall", 2.0),
    (Slot::Dressed, "stone_ashlar_dark", 1.9),
];

fn tile(slot: Slot) -> f32 {
    SLOTS
        .iter()
        .find(|s| s.0 == slot)
        .map(|s| s.2)
        .unwrap_or(1.0)
}

#[derive(Resource, Default)]
pub struct BridgeMaterials {
    handles: HashMap<Slot, Handle<StandardMaterial>>,
    /// the surface style (`art_style::surfaces`) the textures are in
    art: crate::art_style::ArtStyle,
}

impl BridgeMaterials {
    pub fn get(&self, slot: Slot) -> Handle<StandardMaterial> {
        self.handles.get(&slot).cloned().unwrap_or_default()
    }
}

pub fn build(app: &mut App) {
    app.init_resource::<BridgeMaterials>()
        .add_systems(Startup, init_materials)
        .add_systems(Update, restyle);
}

/// The world's art style changed: the bridges' materials take the other style's textures.
fn restyle(
    mut mats: ResMut<BridgeMaterials>,
    assets: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    meta: Res<crate::world::WorldMeta>,
) {
    let art = crate::art_style::surfaces(meta.art_style);
    if art == mats.art || mats.handles.is_empty() {
        return;
    }
    mats.art = art;
    for (slot, id, _) in SLOTS {
        let Some(h) = mats.handles.get(&slot) else {
            continue;
        };
        if let Some(mut m) = materials.get_mut(h) {
            let (albedo, normal, orm) = slot_textures(&assets, id, art);
            m.base_color_texture = Some(albedo);
            m.normal_map_texture = Some(normal);
            m.metallic_roughness_texture = Some(orm.clone());
            m.occlusion_texture = Some(orm);
        }
    }
}

/// A slot's albedo, normal and ORM maps in the given style.
fn slot_textures(
    assets: &AssetServer,
    id: &str,
    art: crate::art_style::ArtStyle,
) -> (Handle<Image>, Handle<Image>, Handle<Image>) {
    let repeat = |srgb: bool| {
        move |s: &mut ImageLoaderSettings| {
            s.is_srgb = srgb;
            s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                mipmap_filter: ImageFilterMode::Linear,
                anisotropy_clamp: 8,
                ..default()
            });
        }
    };
    let dir = format!("textures/materials/{id}");
    let load = |file: &str, srgb: bool| -> Handle<Image> {
        assets
            .load_builder()
            .with_settings(repeat(srgb))
            .load(crate::art_style::texture_rel(&format!("{dir}/{file}"), art))
    };
    (
        load("albedo.png", true),
        load("normal.png", false),
        load("orm.png", false),
    )
}

fn init_materials(
    mut mats: ResMut<BridgeMaterials>,
    assets: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    meta: Res<crate::world::WorldMeta>,
) {
    let art = crate::art_style::surfaces(meta.art_style);
    mats.art = art;
    for (slot, id, _) in SLOTS {
        let (albedo, normal, orm) = slot_textures(&assets, id, art);
        let m = StandardMaterial {
            base_color_texture: Some(albedo),
            normal_map_texture: Some(normal),
            metallic_roughness_texture: Some(orm.clone()),
            occlusion_texture: Some(orm),
            perceptual_roughness: 1.0,
            metallic: 1.0,
            reflectance: 0.35,
            ..default()
        };
        mats.handles.insert(slot, materials.add(m));
    }
}

/// Arch count and rise of a stone bridge `len` long that crosses `wet` metres of water.
pub fn stone_layout(len: f32, wet: f32) -> (usize, f32) {
    let inner = (len - 2.0 * ABUT).max(4.0);
    let n = ((wet + 2.0) / 14.0).ceil().max(1.0) as usize;
    let n = n.min(((inner + PIER) / (4.0 + PIER)).floor().max(1.0) as usize);
    let a = (inner - (n - 1) as f32 * PIER) / n as f32;
    (n, (a * 0.32).clamp(1.2, 5.0).min(a * 0.5))
}

// ---------------------------------------------------------------- mesh building

#[derive(Default)]
struct MeshBuf {
    pos: Vec<[f32; 3]>,
    nor: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    col: Vec<[f32; 4]>,
    idx: Vec<u32>,
}

impl MeshBuf {
    /// A flat polygon (3 or 4 points) facing `n` (winding fixed up automatically).
    fn poly(&mut self, p: &[Vec3], n: Vec3, uv: &[Vec2], tint: [f32; 4]) {
        let base = self.pos.len() as u32;
        for (q, t) in p.iter().zip(uv) {
            self.pos.push(q.to_array());
            self.nor.push(n.to_array());
            self.uv.push(t.to_array());
            self.col.push(tint);
        }
        let flip = (p[1] - p[0]).cross(p[2] - p[0]).dot(n) < 0.0;
        let mut tri = |a: u32, b: u32, c: u32| {
            if flip {
                self.idx.extend_from_slice(&[base + a, base + c, base + b]);
            } else {
                self.idx.extend_from_slice(&[base + a, base + b, base + c]);
            }
        };
        tri(0, 1, 2);
        if p.len() == 4 {
            tri(0, 2, 3);
        }
    }

    /// A box with chamfered edges: centre, axes (x along its length, y up, z across), half sizes,
    /// bevel; UVs are metres / tile from the box's own corner plus `uv_off`.
    #[allow(clippy::too_many_arguments)]
    fn bevel_box(
        &mut self,
        c: Vec3,
        ax: [Vec3; 3],
        h: Vec3,
        b: f32,
        tile: f32,
        uv_off: Vec2,
        tint: [f32; 4],
    ) {
        let b = b.min(h.min_element() * 0.45);
        let hv = [h.x, h.y, h.z];
        // a point on the face of axis `k` (sign s[k]) at corner signs `s`
        let at = |k: usize, s: [f32; 3]| -> Vec3 {
            let mut p = c;
            for a in 0..3 {
                let d = if a == k { hv[a] } else { hv[a] - b };
                p += ax[a] * s[a] * d;
            }
            p
        };
        // texture coordinates by the face's dominant axis (planks: grain along x)
        let uvp = |p: Vec3, k: usize| -> Vec2 {
            let l = p - c;
            let (x, y, z) = (
                l.dot(ax[0]) + hv[0],
                l.dot(ax[1]) + hv[1],
                l.dot(ax[2]) + hv[2],
            );
            let v = match k {
                0 => Vec2::new(z, y),
                1 => Vec2::new(x, z),
                _ => Vec2::new(x, y),
            };
            v / tile + uv_off
        };
        let signs = [-1.0f32, 1.0];
        // faces
        for k in 0..3 {
            let (u, v) = ((k + 1) % 3, (k + 2) % 3);
            for &sk in &signs {
                let mut s = [0.0; 3];
                s[k] = sk;
                let mut pts = vec![];
                for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    s[u] = su;
                    s[v] = sv;
                    pts.push(at(k, s));
                }
                let uvs: Vec<Vec2> = pts.iter().map(|p| uvp(*p, k)).collect();
                self.poly(&pts, ax[k] * sk, &uvs, tint);
            }
        }
        if b <= 1e-4 {
            return;
        }
        // edge chamfers
        for k in 0..3 {
            for j in k + 1..3 {
                let m = 3 - k - j;
                for &sk in &signs {
                    for &sj in &signs {
                        let mut pts = vec![];
                        for (face, sm) in [(k, -1.0), (k, 1.0), (j, 1.0), (j, -1.0)] {
                            let mut s = [0.0; 3];
                            s[k] = sk;
                            s[j] = sj;
                            s[m] = sm;
                            pts.push(at(face, s));
                        }
                        let n = (ax[k] * sk + ax[j] * sj).normalize();
                        let dom = if hv[k] < hv[j] { j } else { k };
                        let uvs: Vec<Vec2> = pts.iter().map(|p| uvp(*p, dom)).collect();
                        self.poly(&pts, n, &uvs, tint);
                    }
                }
            }
        }
        // corners
        for &sx in &signs {
            for &sy in &signs {
                for &sz in &signs {
                    let s = [sx, sy, sz];
                    let pts = [at(0, s), at(1, s), at(2, s)];
                    let n = (ax[0] * sx + ax[1] * sy + ax[2] * sz).normalize();
                    let uvs: Vec<Vec2> = pts.iter().map(|p| uvp(*p, 1)).collect();
                    self.poly(&pts, n, &uvs, tint);
                }
            }
        }
    }

    /// A beam of rectangular section (half sizes `hs` = across, up; chamfer `b`) swept along a
    /// path of frames (point, along, across, up). UV u runs along the path.
    fn sweep(
        &mut self,
        path: &[(Vec3, Vec3, Vec3, Vec3)],
        hs: Vec2,
        b: f32,
        tile: f32,
        uv_off: Vec2,
        tint: [f32; 4],
    ) {
        if path.len() < 2 {
            return;
        }
        let b = b.min(hs.min_element() * 0.45);
        // octagonal profile (rectangle with chamfered corners), counter-clockwise
        let prof: Vec<Vec2> = vec![
            Vec2::new(hs.x, -hs.y + b),
            Vec2::new(hs.x, hs.y - b),
            Vec2::new(hs.x - b, hs.y),
            Vec2::new(-hs.x + b, hs.y),
            Vec2::new(-hs.x, hs.y - b),
            Vec2::new(-hs.x, -hs.y + b),
            Vec2::new(-hs.x + b, -hs.y),
            Vec2::new(hs.x - b, -hs.y),
        ];
        let np = prof.len();
        let mut along = vec![0.0f32];
        for w in path.windows(2) {
            along.push(along.last().unwrap() + w[0].0.distance(w[1].0));
        }
        let mut perim = vec![0.0f32];
        for k in 0..np {
            perim.push(perim.last().unwrap() + prof[k].distance(prof[(k + 1) % np]));
        }
        for k in 0..np {
            let (a, bq) = (prof[k], prof[(k + 1) % np]);
            let en = Vec2::new(bq.y - a.y, a.x - bq.x).normalize();
            for w in 0..path.len() - 1 {
                let (p0, _, l0, u0) = path[w];
                let (p1, _, l1, u1) = path[w + 1];
                let q = [
                    p0 + l0 * a.x + u0 * a.y,
                    p0 + l0 * bq.x + u0 * bq.y,
                    p1 + l1 * bq.x + u1 * bq.y,
                    p1 + l1 * a.x + u1 * a.y,
                ];
                let n = (l0 * en.x + u0 * en.y).normalize();
                let uv = [
                    Vec2::new(along[w], perim[k]) / tile + uv_off,
                    Vec2::new(along[w], perim[k + 1]) / tile + uv_off,
                    Vec2::new(along[w + 1], perim[k + 1]) / tile + uv_off,
                    Vec2::new(along[w + 1], perim[k]) / tile + uv_off,
                ];
                self.poly(&q, n, &uv, tint);
            }
        }
        // end caps
        for (end, sgn) in [(0usize, -1.0f32), (path.len() - 1, 1.0)] {
            let (p, t, l, u) = path[end];
            let c = p;
            for k in 0..np {
                let (a, bq) = (prof[k], prof[(k + 1) % np]);
                let pts = [c, p + l * a.x + u * a.y, p + l * bq.x + u * bq.y];
                let uv: Vec<Vec2> = [Vec2::ZERO, a, bq]
                    .iter()
                    .map(|v| (*v + hs) / tile + uv_off)
                    .collect();
                self.poly(&pts, t * sgn, &uv, tint);
            }
        }
    }

    fn mesh(self) -> Option<Mesh> {
        if self.idx.is_empty() {
            return None;
        }
        let mut m = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.pos);
        m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.nor);
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uv);
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.col);
        m.insert_indices(Indices::U32(self.idx));
        m.generate_tangents().ok()?;
        Some(m)
    }
}

/// The road's frame at arc length `s`: point on the deck surface, along (with slope), across
/// (horizontal, to the left), up.
fn frame(plan: &RoadPlan, s: f32) -> (Vec3, Vec3, Vec3, Vec3) {
    let p = plan.point_at(s);
    let y = plan.height_at(s);
    let d = 0.5;
    let a = plan.point_at(s - d);
    let b = plan.point_at(s + d);
    let dir2 = (b - a).normalize_or(Vec2::X);
    let dy = (plan.height_at(s + d) - plan.height_at(s - d)) / (a.distance(b).max(1e-3));
    let along = Vec3::new(dir2.x, dy, dir2.y).normalize();
    let across = Vec3::new(-dir2.y, 0.0, dir2.x);
    let up = across.cross(along).normalize();
    (Vec3::new(p.x, y, p.y), along, across, up)
}

fn shade(rng: &mut Rng, base: f32, var: f32) -> [f32; 4] {
    let k = base + rng.range(-var, var);
    [k * rng.range(0.97, 1.03), k, k * rng.range(0.95, 1.02), 1.0]
}

/// Meshes (one per material) of the bridge over `sp`.
pub fn bridge_meshes(plan: &RoadPlan, sp: &Span, t: &Terrain) -> Vec<(Mesh, Slot)> {
    let bufs = if sp.stone {
        stone_bridge(plan, sp, t)
    } else {
        timber_bridge(plan, sp, t)
    };
    bufs.into_iter()
        .filter_map(|(slot, b)| b.mesh().map(|m| (m, slot)))
        .collect()
}

fn timber_bridge(plan: &RoadPlan, sp: &Span, t: &Terrain) -> Vec<(Slot, MeshBuf)> {
    let mut planks = MeshBuf::default();
    let mut timber = MeshBuf::default();
    let mut rng = Rng::new(plan.id as u64 * 7919 + sp.i0 as u64);
    let (s0, s1) = (plan.samples[sp.i0].s, plan.samples[sp.i1].s);
    let hw = plan.hw;
    let deck_w = hw * 2.0 + 0.3;
    const PLANK_T: f32 = 0.07;
    const STR_H: f32 = 0.26;
    const CAP: f32 = 0.24;

    // ---- deck planks, one by one
    let mut s = s0 + 0.12;
    while s < s1 - 0.12 {
        let w = rng.range(0.21, 0.27);
        let (p, al, ac, up) = frame(plan, s + w * 0.5);
        let yaw = rng.range(-0.014, 0.014);
        let q = Quat::from_axis_angle(up, yaw);
        let len = deck_w + rng.range(-0.08, 0.08);
        let c = p + up * (-PLANK_T * 0.5 + rng.range(-0.006, 0.004)) + ac * rng.range(-0.05, 0.05);
        let ax = [q * ac, up, q * al];
        let tint = shade(&mut rng, 1.0, 0.13);
        planks.bevel_box(
            c,
            ax,
            Vec3::new(len * 0.5, PLANK_T * 0.5, w * 0.5),
            0.012,
            tile(Slot::Planks),
            Vec2::new(rng.f32() * 7.0, rng.f32() * 7.0),
            tint,
        );
        s += w + rng.range(0.008, 0.02);
    }

    // ---- stringers under the planks
    let n_str = if hw < 1.4 {
        2
    } else if hw < 2.6 {
        3
    } else {
        4
    };
    let path_at = |off: f32, dy: f32, a: f32, b: f32| -> Vec<(Vec3, Vec3, Vec3, Vec3)> {
        let n = ((b - a) / 0.8).ceil().max(1.0) as usize;
        (0..=n)
            .map(|k| {
                let (p, al, ac, up) = frame(plan, a + (b - a) * k as f32 / n as f32);
                (p + ac * off + up * dy, al, ac, up)
            })
            .collect()
    };
    for k in 0..n_str {
        let off = -(hw - 0.3) + 2.0 * (hw - 0.3) * k as f32 / (n_str - 1) as f32;
        let tint = shade(&mut rng, 0.7, 0.06);
        timber.sweep(
            &path_at(off, -PLANK_T - STR_H * 0.5, s0 + 0.05, s1 - 0.05),
            Vec2::new(0.08, STR_H * 0.5),
            0.015,
            tile(Slot::Timber),
            Vec2::new(rng.f32(), rng.f32()),
            tint,
        );
    }

    // ---- bents: cap beam, posts into the bed, cross bracing
    let len = s1 - s0;
    let bays = (len / 4.5).ceil().max(1.0) as usize;
    let mut bent_at = vec![];
    for k in 0..=bays {
        bent_at.push(s0 + 0.45 + (len - 0.9) * k as f32 / bays as f32);
    }
    let cap_top = -PLANK_T - STR_H;
    for &bs in &bent_at {
        let (p, al, ac, up) = frame(plan, bs);
        let flat_al = Vec3::new(al.x, 0.0, al.z).normalize_or(Vec3::X);
        let cap_c = p + up * (cap_top - CAP * 0.5);
        let tint = shade(&mut rng, 0.8, 0.06);
        timber.bevel_box(
            cap_c,
            [ac, Vec3::Y, flat_al],
            Vec3::new(deck_w * 0.5 + 0.25, CAP * 0.5, CAP * 0.5),
            0.02,
            tile(Slot::Timber),
            Vec2::new(rng.f32(), rng.f32()),
            tint,
        );
        let cap_bottom = cap_c.y - CAP * 0.5;
        let posts: Vec<f32> = if hw < 2.0 {
            vec![-(hw - 0.1), hw - 0.1]
        } else {
            vec![-(hw - 0.1), 0.0, hw - 0.1]
        };
        let mut post_feet = vec![];
        for &o in &posts {
            let q = cap_c + ac * o;
            let g = crate::roads::mesh::surface_height(t, q.x, q.z);
            let foot = g - 0.6;
            let height = cap_bottom - foot;
            if cap_bottom - g < 0.25 {
                continue;
            }
            let tint = shade(&mut rng, 0.78, 0.07);
            let lean = Quat::from_axis_angle(flat_al, rng.range(-0.012, 0.012));
            let c = Vec3::new(q.x, foot + height * 0.5, q.z);
            timber.bevel_box(
                c,
                [lean * ac, lean * Vec3::Y, flat_al],
                Vec3::new(0.11, height * 0.5, 0.11),
                0.02,
                tile(Slot::Timber),
                Vec2::new(rng.f32(), rng.f32()),
                tint,
            );
            post_feet.push((q, g));
        }
        // the end bents stand on the bank: a sill beam under the cap instead of posts
        if post_feet.is_empty() {
            let g = crate::roads::mesh::surface_height(t, cap_c.x, cap_c.z);
            let top = cap_bottom.max(g + 0.05);
            let bottom = g - 0.35;
            let c = Vec3::new(cap_c.x, (top + bottom) * 0.5, cap_c.z);
            timber.bevel_box(
                c,
                [ac, Vec3::Y, flat_al],
                Vec3::new(deck_w * 0.5 + 0.35, (top - bottom) * 0.5, 0.17),
                0.03,
                tile(Slot::Timber),
                Vec2::new(rng.f32(), rng.f32()),
                shade(&mut rng, 0.7, 0.05),
            );
        }
        // X bracing between the outer posts where they stand tall
        if post_feet.len() >= 2 {
            let (a, ga) = post_feet[0];
            let (b, gb) = post_feet[post_feet.len() - 1];
            let low = ga.max(gb).max(sp.water) + 0.35;
            let top = cap_bottom - 0.15;
            if top - low > 1.2 {
                for (from, to) in [((a, top), (b, low)), ((a, low), (b, top))] {
                    let p0 = Vec3::new(from.0.x, from.1, from.0.z) + flat_al * 0.15;
                    let p1 = Vec3::new(to.0.x, to.1, to.0.z) + flat_al * 0.15;
                    let along = (p1 - p0).normalize();
                    let side = flat_al;
                    let upv = side.cross(along).normalize();
                    let path = [(p0, along, side, upv), (p1, along, side, upv)];
                    timber.sweep(
                        &path,
                        Vec2::new(0.03, 0.09),
                        0.01,
                        tile(Slot::Timber),
                        Vec2::new(rng.f32(), rng.f32()),
                        shade(&mut rng, 0.8, 0.06),
                    );
                }
            }
        }
    }

    // ---- railings: posts, top rail, mid rail
    let mut rail_at = vec![];
    for w in bent_at.windows(2) {
        rail_at.push(w[0]);
        rail_at.push((w[0] + w[1]) * 0.5);
    }
    rail_at.push(*bent_at.last().unwrap());
    for side in [-1.0f32, 1.0] {
        let off = side * (deck_w * 0.5 + 0.075);
        for &rs in &rail_at {
            let (p, al, ac, _) = frame(plan, rs);
            let flat_al = Vec3::new(al.x, 0.0, al.z).normalize_or(Vec3::X);
            let bottom = p.y - PLANK_T - STR_H;
            let top = p.y + 1.08;
            let c = Vec3::new(p.x, (top + bottom) * 0.5, p.z) + ac * off;
            timber.bevel_box(
                c,
                [ac, Vec3::Y, flat_al],
                Vec3::new(0.055, (top - bottom) * 0.5, 0.055),
                0.012,
                tile(Slot::Timber),
                Vec2::new(rng.f32(), rng.f32()),
                shade(&mut rng, 0.85, 0.07),
            );
        }
        let (a, b) = (rail_at[0] - 0.08, *rail_at.last().unwrap() + 0.08);
        let top = path_at(off, 1.0, a, b);
        timber.sweep(
            &top,
            Vec2::new(0.05, 0.06),
            0.015,
            tile(Slot::Timber),
            Vec2::new(rng.f32(), rng.f32()),
            shade(&mut rng, 0.88, 0.05),
        );
        let mid = path_at(off + side * 0.01, 0.52, a, b);
        timber.sweep(
            &mid,
            Vec2::new(0.035, 0.045),
            0.01,
            tile(Slot::Timber),
            Vec2::new(rng.f32(), rng.f32()),
            shade(&mut rng, 0.85, 0.05),
        );
    }
    vec![(Slot::Planks, planks), (Slot::Timber, timber)]
}

/// Intrados (underside) height of a segmental arch of span `a` and rise `r` springing at `ys`,
/// at `x` from its start.
fn arch_y(x: f32, a: f32, r: f32, ys: f32) -> f32 {
    let rad = (a * a * 0.25 + r * r) / (2.0 * r);
    let cy = ys + r - rad;
    let dx = x - a * 0.5;
    cy + (rad * rad - dx * dx).max(0.0).sqrt()
}

fn authored_arch_spring(plan: &RoadPlan, sp: &Span, rise: f32, default: f32) -> f32 {
    if !plan.authored {
        return default;
    }
    let deck = plan.h[sp.i0..=sp.i1]
        .iter()
        .copied()
        .filter(|y| y.is_finite())
        .fold(f32::INFINITY, f32::min);
    if deck.is_finite() {
        default.min(deck - rise - ARCH_RING_CLEARANCE)
    } else {
        default
    }
}

fn stone_bridge(plan: &RoadPlan, sp: &Span, t: &Terrain) -> Vec<(Slot, MeshBuf)> {
    let mut stone = MeshBuf::default();
    let mut dressed = MeshBuf::default();
    let (s0, s1) = (plan.samples[sp.i0].s, plan.samples[sp.i1].s);
    let len = s1 - s0;
    let hw = plan.hw;
    let hb = hw + PARAPET_T;
    let n = sp.arches.max(1);
    let inner = (len - 2.0 * ABUT).max(4.0);
    let pier = if n > 1 { PIER } else { 0.0 };
    let a = (inner - (n - 1) as f32 * pier) / n as f32;
    let r = sp.rise.min(a * 0.5).max(0.5);
    // Imported decks may run only a little above water. Keep the intrados and dressed arch ring
    // below that deck by lowering its spring datum; ordinary procedural bridges retain theirs.
    let ys = authored_arch_spring(plan, sp, r, sp.water + 0.4);
    let arches: Vec<(f32, f32)> = (0..n)
        .map(|k| {
            let x0 = ABUT + k as f32 * (a + pier) + (len - 2.0 * ABUT - inner).max(0.0) * 0.5;
            (x0, x0 + a)
        })
        .collect();
    let in_arch = |x: f32| {
        arches
            .iter()
            .find(|(x0, x1)| x > *x0 + 1e-3 && x < *x1 - 1e-3)
            .copied()
    };
    let ground = |x: f32| {
        let (p, _, _, _) = frame(plan, s0 + x);
        crate::roads::mesh::surface_height(t, p.x, p.z).min(sp.water) - 1.2
    };
    let st = tile(Slot::Stone);
    let dt = tile(Slot::Dressed);
    let white = [1.0, 1.0, 1.0, 1.0];

    // stations along the bridge: every 0.5 m, every arch boundary twice (pier side, arch side)
    let mut xs: Vec<(f32, Option<f32>)> = vec![];
    let mut x = 0.0;
    while x < len {
        xs.push((x, None));
        x += 0.5;
    }
    xs.push((len, None));
    for (x0, x1) in &arches {
        xs.push((*x0, Some(ys)));
        xs.push((*x1, Some(ys)));
    }
    xs.sort_by(|p, q| {
        p.0.total_cmp(&q.0)
            .then((p.1.is_some()).cmp(&q.1.is_some()))
    });
    // bottom of the wall at each station: the arch's intrados, else down into the bed
    let bottom = |x: f32, forced: Option<f32>| -> f32 {
        if let Some(y) = forced {
            return y;
        }
        match in_arch(x) {
            Some((x0, x1)) => arch_y(x - x0, x1 - x0, r, ys),
            None => ground(x),
        }
    };
    // at an arch boundary the pier-side station comes first going into an arch, last coming out
    let mut stations: Vec<(f32, f32)> = vec![];
    for (x, forced) in &xs {
        let b = bottom(*x, *forced);
        let entering = arches.iter().any(|(x0, _)| (x - x0).abs() < 1e-4);
        if forced.is_some() {
            if entering {
                stations.push((*x, ground(*x)));
                stations.push((*x, b));
            } else {
                stations.push((*x, b));
                stations.push((*x, ground(*x)));
            }
        } else {
            stations.push((*x, b));
        }
    }
    stations.dedup_by(|p, q| (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4);

    // ---- spandrel walls with parapets, both faces
    for side in [-1.0f32, 1.0] {
        for w in stations.windows(2) {
            let ((xa, ba), (xb, bb)) = (w[0], w[1]);
            if (xb - xa).abs() < 1e-4 {
                continue;
            }
            let (pa, _, aca, _) = frame(plan, s0 + xa);
            let (pb, _, acb, _) = frame(plan, s0 + xb);
            let ta = pa.y + PARAPET_H;
            let tb = pb.y + PARAPET_H;
            let qa = pa + aca * side * hb;
            let qb = pb + acb * side * hb;
            let pts = [
                Vec3::new(qa.x, ba, qa.z),
                Vec3::new(qb.x, bb, qb.z),
                Vec3::new(qb.x, tb, qb.z),
                Vec3::new(qa.x, ta, qa.z),
            ];
            let uv = [
                Vec2::new(xa, ba) / st,
                Vec2::new(xb, bb) / st,
                Vec2::new(xb, tb) / st,
                Vec2::new(xa, ta) / st,
            ];
            stone.poly(&pts, aca * side, &uv, white);
        }
        // parapet inner face
        for w in xs.windows(2) {
            let (xa, xb) = (w[0].0, w[1].0);
            if (xb - xa).abs() < 1e-4 {
                continue;
            }
            let (pa, _, aca, _) = frame(plan, s0 + xa);
            let (pb, _, acb, _) = frame(plan, s0 + xb);
            let qa = pa + aca * side * hw;
            let qb = pb + acb * side * hw;
            let pts = [qa, qb, qb + Vec3::Y * PARAPET_H, qa + Vec3::Y * PARAPET_H];
            let uv = [
                Vec2::new(xa, pa.y) / st,
                Vec2::new(xb, pb.y) / st,
                Vec2::new(xb, pb.y + PARAPET_H) / st,
                Vec2::new(xa, pa.y + PARAPET_H) / st,
            ];
            stone.poly(&pts, -aca * side, &uv, white);
        }
        // coping along the parapet top
        let n_c = (len / 0.6).ceil() as usize;
        let path: Vec<(Vec3, Vec3, Vec3, Vec3)> = (0..=n_c)
            .map(|k| {
                let (p, al, ac, _) = frame(plan, s0 + len * k as f32 / n_c as f32);
                let flat = Vec3::new(al.x, 0.0, al.z).normalize_or(Vec3::X);
                (
                    p + ac * side * (hw + PARAPET_T * 0.5) + Vec3::Y * (PARAPET_H + 0.08),
                    flat,
                    ac,
                    Vec3::Y,
                )
            })
            .collect();
        dressed.sweep(
            &path,
            Vec2::new(PARAPET_T * 0.5 + 0.06, 0.09),
            0.03,
            dt,
            Vec2::new(0.3 * side, 0.1),
            white,
        );
        // end blocks
        for (x, dirk) in [(0.25f32, 1.0f32), (len - 0.25, -1.0)] {
            let (p, al, ac, _) = frame(plan, s0 + x);
            let flat = Vec3::new(al.x, 0.0, al.z).normalize_or(Vec3::X) * dirk;
            let c = p + ac * side * (hw + PARAPET_T * 0.5) + Vec3::Y * (PARAPET_H * 0.5 + 0.18);
            dressed.bevel_box(
                c,
                [flat, Vec3::Y, ac],
                Vec3::new(0.38, PARAPET_H * 0.5 + 0.2, PARAPET_T * 0.5 + 0.1),
                0.04,
                dt,
                Vec2::new(0.2, 0.7),
                white,
            );
        }
    }

    // ---- deck under the road surface (shows at its soft edges)
    for w in xs.windows(2) {
        let (xa, xb) = (w[0].0, w[1].0);
        if (xb - xa).abs() < 1e-4 {
            continue;
        }
        let (pa, _, aca, _) = frame(plan, s0 + xa);
        let (pb, _, acb, _) = frame(plan, s0 + xb);
        let d = Vec3::Y * -0.004;
        let pts = [
            pa - aca * hw + d,
            pb - acb * hw + d,
            pb + acb * hw + d,
            pa + aca * hw + d,
        ];
        let uv = [
            Vec2::new(-hw, xa) / dt,
            Vec2::new(-hw, xb) / dt,
            Vec2::new(hw, xb) / dt,
            Vec2::new(hw, xa) / dt,
        ];
        dressed.poly(&pts, Vec3::Y, &uv, [0.9, 0.9, 0.9, 1.0]);
    }

    // ---- barrel vaults and the arch rings
    for (x0, x1) in &arches {
        let steps = ((x1 - x0) / 0.4).ceil().max(4.0) as usize;
        let span = x1 - x0;
        let rad = (span * span * 0.25 + r * r) / (2.0 * r);
        let cy = ys + r - rad;
        let mut arc = 0.0;
        let mut prev: Option<(Vec3, Vec3, f32, f32)> = None;
        for k in 0..=steps {
            let x = x0 + span * k as f32 / steps as f32;
            let y = arch_y(x - x0, span, r, ys);
            let (p, _, ac, _) = frame(plan, s0 + x);
            let c = Vec3::new(p.x, y, p.z);
            if let Some((pc, pac, px, py)) = prev {
                arc += Vec2::new(x - px, y - py).length();
                let pts = [pc - pac * hb, c - ac * hb, c + ac * hb, pc + pac * hb];
                let uv = [
                    Vec2::new(arc - 0.4, -hb) / st,
                    Vec2::new(arc, -hb) / st,
                    Vec2::new(arc, hb) / st,
                    Vec2::new(arc - 0.4, hb) / st,
                ];
                // facing the circle's centre, i.e. down into the opening
                let (_, al, _, _) = frame(plan, s0 + (x + px) * 0.5);
                let flat = Vec3::new(al.x, 0.0, al.z).normalize_or(Vec3::X);
                let to_c =
                    flat * (x0 + span * 0.5 - (x + px) * 0.5) + Vec3::Y * (cy - (y + py) * 0.5);
                stone.poly(&pts, to_c.normalize_or(Vec3::NEG_Y), &uv, white);
            }
            prev = Some((c, ac, x, y));
        }
        // dressed voussoir ring on both faces, standing 4 cm proud of the wall
        let half_ang = ((span * 0.5) / rad).clamp(-1.0, 1.0).asin();
        let ring = 0.55;
        for side in [-1.0f32, 1.0] {
            let stones = ((half_ang * 2.0 * rad) / 0.42).round().max(5.0) as usize;
            for k in 0..stones {
                let a0 = -half_ang + 2.0 * half_ang * k as f32 / stones as f32;
                let a1 = -half_ang + 2.0 * half_ang * (k + 1) as f32 / stones as f32;
                let gap = 0.012 / rad;
                let pt = |ang: f32, rr: f32, out: f32| {
                    let x = x0 + span * 0.5 + rr * ang.sin();
                    let y = cy + rr * ang.cos();
                    let (p, _, ac, _) = frame(plan, s0 + x);
                    Vec3::new(p.x, y, p.z) + ac * side * (hb + out)
                };
                let (ac0, ac1) = (a0 + gap, a1 - gap);
                let jitter = ((k * 7919 + plan.id as usize) % 5) as f32 * 0.03;
                let outer = rad + ring + jitter;
                let face = [
                    pt(ac0, rad, 0.04),
                    pt(ac1, rad, 0.04),
                    pt(ac1, outer, 0.04),
                    pt(ac0, outer, 0.04),
                ];
                let (_, al, ac, _) = frame(plan, s0 + x0 + span * 0.5);
                let flat = Vec3::new(al.x, 0.0, al.z).normalize_or(Vec3::X);
                let uvf = [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.42, 0.0),
                    Vec2::new(0.42, ring),
                    Vec2::new(0.0, ring),
                ]
                .map(|v| v / dt + Vec2::new(k as f32 * 0.37, 0.0));
                let tint = [0.92 + jitter, 0.92 + jitter, 0.9 + jitter, 1.0];
                dressed.poly(&face, ac * side, &uvf, tint);
                // the stone's top and sides (its 4 cm depth)
                let back = [pt(ac0, outer, 0.0), pt(ac1, outer, 0.0)];
                let top = [face[3], face[2], back[1], back[0]];
                let mid_ang = (ac0 + ac1) * 0.5;
                let n_top = (flat * mid_ang.sin() + Vec3::Y * mid_ang.cos()).normalize_or(Vec3::Y);
                dressed.poly(&top, n_top, &uvf, tint);
                // the joints' faces, facing along the ring
                for (fa, fb, ang, sgn) in [
                    (face[0], face[3], ac0, -1.0f32),
                    (face[1], face[2], ac1, 1.0),
                ] {
                    let back_a = fa - ac * side * 0.04;
                    let back_b = fb - ac * side * 0.04;
                    let n_side = (flat * ang.cos() - Vec3::Y * ang.sin()) * sgn;
                    dressed.poly(&[fa, fb, back_b, back_a], n_side, &uvf, tint);
                }
            }
        }
    }

    // ---- cutwaters on both faces of every pier
    for w in arches.windows(2) {
        let (pa, pb) = (w[0].1, w[1].0);
        let mid = (pa + pb) * 0.5;
        let top = ys + r * 0.55;
        for side in [-1.0f32, 1.0] {
            let (p, _, ac, _) = frame(plan, s0 + mid);
            let (pl, _, acl, _) = frame(plan, s0 + pa);
            let (pr, _, acr, _) = frame(plan, s0 + pb);
            let gl = ground(pa);
            let gr = ground(pb);
            let bot = gl.min(gr);
            let base_l = pl + acl * side * hb;
            let base_r = pr + acr * side * hb;
            let nose = p + ac * side * (hb + (pb - pa) * 0.6);
            let y = |q: Vec3, h: f32| Vec3::new(q.x, h, q.z);
            // two slanted faces, facing away from the pier
            let centre = (base_l + base_r) * 0.5;
            for (q0, q1) in [(base_l, nose), (nose, base_r)] {
                let pts = [y(q0, bot), y(q1, bot), y(q1, top), y(q0, top)];
                let n = (q1 - q0).cross(Vec3::Y).normalize_or(Vec3::X);
                let n = if n.dot((q0 + q1) * 0.5 - centre) < 0.0 {
                    -n
                } else {
                    n
                };
                let uv = [
                    Vec2::new(0.0, bot),
                    Vec2::new(q0.distance(q1), bot),
                    Vec2::new(q0.distance(q1), top),
                    Vec2::new(0.0, top),
                ]
                .map(|v| v / st);
                stone.poly(&pts, n, &uv, white);
            }
            // pyramid cap up to the wall
            let apex = y((base_l + base_r) * 0.5, top + (pb - pa) * 0.45);
            for (q0, q1) in [(base_l, nose), (nose, base_r)] {
                let pts = [y(q0, top), y(q1, top), apex];
                let n = (y(q1, top) - y(q0, top))
                    .cross(apex - y(q0, top))
                    .normalize();
                let n = if n.y < 0.0 { -n } else { n };
                let uv = [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.7, 0.0),
                    Vec2::new(0.35, 0.6),
                ];
                dressed.poly(&pts, n, &uv, white);
            }
        }
    }

    // ---- end faces of the body (mostly inside the embankments)
    for (x, sgn) in [(0.0f32, -1.0f32), (len, 1.0)] {
        let (p, al, ac, _) = frame(plan, s0 + x);
        let b = ground(x);
        let flat = Vec3::new(al.x, 0.0, al.z).normalize_or(Vec3::X);
        let (l, r) = (p - ac * hb, p + ac * hb);
        let top = p.y - 0.02;
        let pts = [
            Vec3::new(l.x, b, l.z),
            Vec3::new(r.x, b, r.z),
            Vec3::new(r.x, top, r.z),
            Vec3::new(l.x, top, l.z),
        ];
        let uv = [
            Vec2::new(-hb, b),
            Vec2::new(hb, b),
            Vec2::new(hb, top),
            Vec2::new(-hb, top),
        ]
        .map(|v| v / st);
        stone.poly(&pts, flat * sgn, &uv, white);
    }
    vec![(Slot::Stone, stone), (Slot::Dressed, dressed)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roads::plan::Sample;
    use crate::roads::{RoadKind, RoadOptions};

    #[test]
    fn authored_low_deck_keeps_arch_ring_under_the_deck() {
        let plan = RoadPlan {
            id: 1,
            kind: RoadKind::Cobblestone,
            options: RoadOptions::default(),
            authored: true,
            hw: 2.0,
            samples: vec![
                Sample {
                    p: Vec2::ZERO,
                    y: 0.7,
                    s: 0.0,
                    dir: Vec2::X,
                },
                Sample {
                    p: Vec2::new(4.0, 0.0),
                    y: 0.7,
                    s: 4.0,
                    dir: Vec2::X,
                },
                Sample {
                    p: Vec2::new(8.0, 0.0),
                    y: 0.7,
                    s: 8.0,
                    dir: Vec2::X,
                },
            ],
            ground: vec![0.0; 3],
            h: vec![0.7; 3],
            bed: vec![0.0; 3],
            spans: vec![],
            reach: 3.0,
            lo: Vec2::splat(-3.0),
            hi: Vec2::splat(11.0),
        };
        let span = Span {
            i0: 0,
            i1: 2,
            water: 0.0,
            deck: 0.7,
            stone: true,
            arches: 1,
            rise: 0.5,
        };
        let spring = authored_arch_spring(&plan, &span, 0.5, 0.4);
        assert!(spring.is_finite());
        assert!(spring + 0.5 + ARCH_RING_CLEARANCE <= 0.7 + 1e-5);
    }
}
