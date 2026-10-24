//! Shade under the trees: how much of the sky a spot near the viewer still sees through the
//! crowns above it. Screen-space ambient occlusion only reaches a metre or two, so a forest floor
//! under a closed canopy used to get the full sky light of an open field. This map darkens the
//! sky (ambient) light under and inside the crowns; direct sunlight keeps its shadow maps.
//!
//! A 768 m square around the camera on a 2 m grid, rebuilt on the CPU when the viewer moves on
//! (16 m snaps) or the trees change: every tree near the viewer (procedural and placed) splats its
//! crown disc, the coverage is blurred over about a crown's height (the sky a spot sees is a wide
//! cone, not the column straight up), and crown base and top heights are kept for the shaders.
//! It lives in one storage buffer (`CANOPY_SHADE`) bound at 120 by the terrain, grass and foliage
//! materials; `canopy_shade.wesl::canopy_sky` reads it. Outside the square the light is unchanged.
//!
//! Per texel: `pack2x16float(sky visibility, crown top y)`, `pack2x16float(crown base y, cover)`.
//! Header: (origin x, origin z, texel m, texels per side), (strength, fade start, fade end, how
//! much of the meadow grass the crowns thin out).
//!
//! The cover (how much of the ground the crowns overhang, 0..1, lightly blurred) also stays on the
//! CPU (`CanopyShade::cover_at`): ground cover grows its forest undergrowth from it, and the grass
//! shader thins the meadow there (`canopy_grass`).
//!
//! `WB_SHADELOG=1` logs each rebuild: crowns, shaded texels, the least sky and the sky at the viewer.

use bevy::asset::{RenderAssetUsages, uuid_handle};
use bevy::prelude::*;
use bevy::render::render_resource::RenderPipelineDescriptor;
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderDefVal;

use super::NatureAppearance;
use super::render::Near;
use crate::camera::MainCamera;
use crate::objects::{Category, Library, ObjectStore};

pub const CANOPY_SHADE: Handle<ShaderBuffer> =
    uuid_handle!("3b8e6f0a-51c4-4d2e-9a7f-c2d18e5b4a90");

/// Texels per side and their size (m): 768 m, past the trees' mesh LODs (320 m).
const N: usize = 384;
const TEXEL: f32 = 2.0;
/// The square moves in these steps, so walking doesn't rebuild it every frame.
const SNAP: f32 = 16.0;
/// Sky hidden by a fully closed canopy at 100% (real forest floors see 10–30% of the sky).
const MAX_SHADE: f32 = 0.85;
/// Coverage blurs (texels, two box passes each): the crown right overhead hides the steep middle
/// of the sky, the canopy around a spot its lower rings. A spot under one crown of an open grove
/// loses about half its sky; deep in a closed wood about 70%.
const NEAR_BLUR: usize = 1;
const WIDE_BLUR: usize = 5;
const NEAR_SHARE: f32 = 0.6;
/// Crown heights spread further than the wide blur, so shade never meets an unknown height.
const HEIGHT_BLUR: usize = 6;
/// The shade fades out towards the square's edge (m from its centre).
const FADE: (f32, f32) = (300.0, 370.0);
const HEADER_WORDS: usize = 8;

#[derive(Resource, Default)]
pub struct CanopyShade {
    origin: Option<Vec2>,
    key: u64,
    timer: f32,
    /// the crowns' cover per texel (empty: no trees near)
    cover: Vec<f32>,
    /// bumped whenever the map is rebuilt
    pub revision: u64,
}

impl CanopyShade {
    /// How much of the ground at `p` the crowns overhang (0 open ground .. 1 a closed canopy).
    pub fn cover_at(&self, p: Vec2) -> f32 {
        let Some(origin) = self.origin else { return 0.0 };
        if self.cover.len() != N * N {
            return 0.0;
        }
        let g = (p - origin) / TEXEL - 0.5;
        let i = g.floor();
        if i.x < 0.0 || i.y < 0.0 || i.x >= (N - 1) as f32 || i.y >= (N - 1) as f32 {
            return 0.0;
        }
        let f = g - i;
        let (x, z) = (i.x as usize, i.y as usize);
        let at = |x: usize, z: usize| self.cover[z * N + x];
        let a = at(x, z) + (at(x + 1, z) - at(x, z)) * f.x;
        let b = at(x, z + 1) + (at(x + 1, z + 1) - at(x, z + 1)) * f.x;
        a + (b - a) * f.y
    }
}

/// The empty map (no shade) must exist before any material using it is prepared.
pub fn init_buffer(mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let mut words = vec![0u32; HEADER_WORDS + 2];
    words[3] = 0f32.to_bits();
    let _ = buffers.insert(
        &CANOPY_SHADE,
        ShaderBuffer::new(words, RenderAssetUsages::RENDER_WORLD),
    );
}

/// Materials binding the map (at 120) tell their shaders so (`CANOPY_SHADE`): a shader loaded by
/// an older build, without the binding, then still runs.
pub fn shader_def(descriptor: &mut RenderPipelineDescriptor) {
    let def = ShaderDefVal::Bool("CANOPY_SHADE".into(), true);
    descriptor.vertex.shader_defs.push(def.clone());
    if let Some(f) = descriptor.fragment.as_mut() {
        f.shader_defs.push(def);
    }
}

/// One crown: centre xz, radius, base and top y, opacity.
struct Crown {
    c: Vec2,
    r: f32,
    base: f32,
    top: f32,
    alpha: f32,
}

pub fn update_canopy_shade(
    mut state: ResMut<CanopyShade>,
    near: Res<Near>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    appearance: Res<NatureAppearance>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    time: Res<Time>,
) {
    let (Some(lib), Ok(cam)) = (lib, cam.single()) else {
        return;
    };
    state.timer -= time.delta_secs();
    if state.timer > 0.0 {
        return;
    }
    state.timer = 0.25;
    let a = appearance.sanitized();
    let strength = a.canopy_shade;
    let thin = 1.0 - a.ground.grass_under_trees;
    let half = N as f32 * TEXEL * 0.5;
    let origin = ((cam.translation().xz() - half) / SNAP).floor() * SNAP;
    let centre = origin + half;

    // (always: the undergrowth and the meadow's thinning follow the crowns whatever the shade)
    let mut crowns = Vec::new();
    {
        let mut found = Vec::new();
        near.query(centre, half * 1.42, &mut found);
        for i in &found {
            let Some(e) = lib.entries.get(i.entry as usize) else {
                continue;
            };
            crowns.extend(crown(e, i.variant as usize, i.pos, i.scale));
        }
        for id in store.query(centre, half * 1.42) {
            let Some(p) = store.items.get(&id) else {
                continue;
            };
            let Some(e) = lib.get(&p.asset) else { continue };
            crowns.extend(crown(e, p.variant as usize, Vec3::from(p.pos), p.scale));
        }
    }
    // rebuild only when the square moved, the trees changed or the setting did
    let key = crowns.iter().fold(
        (strength.to_bits() as u64) ^ (thin.to_bits() as u64) << 7 ^ (crowns.len() as u64) << 40,
        |h, c| {
            h.rotate_left(5)
                ^ (c.c.x.to_bits() as u64)
                ^ ((c.c.y.to_bits() as u64) << 17)
                ^ (c.top.to_bits() as u64)
        },
    );
    if state.origin == Some(origin) && state.key == key {
        return;
    }
    state.origin = Some(origin);
    state.key = key;
    let (words, cover) = build(origin, &crowns, strength, thin);
    state.cover = cover;
    state.revision += 1;
    if std::env::var("WB_SHADELOG").is_ok() {
        let vis: Vec<f32> = (0..N * N)
            .map(|i| half::f16::from_bits(words[HEADER_WORDS + i * 2] as u16).to_f32())
            .collect();
        let g = ((cam.translation().xz() - origin) / TEXEL).as_ivec2();
        let here = vis[(g.y.clamp(0, N as i32 - 1) as usize) * N + g.x.clamp(0, N as i32 - 1) as usize];
        let shaded = vis.iter().filter(|v| **v < 0.95).count();
        let min = vis.iter().cloned().fold(1.0, f32::min);
        info!(
            "SHADE {} crowns, {shaded} texels shaded, min sky {min:.2}, here {here:.2}, strength {strength}",
            crowns.len()
        );
    }
    let _ = buffers.insert(
        &CANOPY_SHADE,
        ShaderBuffer::new(words, RenderAssetUsages::RENDER_WORLD),
    );
}

/// The crown of a tree library entry placed at `pos` (none for anything but leafy trees).
fn crown(
    e: &crate::objects::LibraryEntry,
    variant: usize,
    pos: Vec3,
    scale: f32,
) -> Option<Crown> {
    if e.category != Category::Trees {
        return None;
    }
    let tree = e.tree.as_ref()?;
    let v = tree.variants.get(variant).or(tree.variants.first())?;
    let height = v.height * scale;
    if height < 2.5 {
        return None;
    }
    // bare trees (dead, winter) still break up some of the sky
    let leafy = v.lods.first().is_some_and(|l| l.1.is_some());
    Some(Crown {
        c: pos.xz(),
        r: (v.width * scale * 0.42).max(1.2),
        base: pos.y + height * 0.3,
        top: pos.y + height,
        alpha: if leafy { 0.88 } else { 0.3 },
    })
}

/// The packed map (header + texels) and the CPU copy of the cover.
fn build(origin: Vec2, crowns: &[Crown], strength: f32, thin: f32) -> (Vec<u32>, Vec<f32>) {
    let mut words = vec![0u32; HEADER_WORDS + N * N * 2];
    let header = [
        origin.x,
        origin.y,
        TEXEL,
        N as f32,
        strength,
        FADE.0,
        FADE.1,
        thin,
    ];
    for (w, v) in words.iter_mut().zip(header) {
        *w = v.to_bits();
    }
    if crowns.is_empty() {
        // an empty square: the shaders see full sky and no crowns everywhere
        words[3] = 0f32.to_bits();
        return (words, vec![]);
    }
    // transmittance (product over the crowns), and coverage-weighted crown heights
    let mut trans = vec![1.0f32; N * N];
    let mut top = vec![0.0f32; N * N];
    let mut base = vec![0.0f32; N * N];
    let mut weight = vec![0.0f32; N * N];
    for c in crowns {
        let g = (c.c - origin) / TEXEL;
        let rt = c.r / TEXEL;
        let (x0, x1) = ((g.x - rt).floor().max(0.0), (g.x + rt).ceil().min(N as f32 - 1.0));
        let (z0, z1) = ((g.y - rt).floor().max(0.0), (g.y + rt).ceil().min(N as f32 - 1.0));
        if x0 > x1 || z0 > z1 {
            continue;
        }
        for z in z0 as usize..=z1 as usize {
            for x in x0 as usize..=x1 as usize {
                let d = Vec2::new(x as f32 + 0.5, z as f32 + 0.5).distance(g) / rt;
                // dense middle, thinning towards the crown's rim
                let w = 1.0 - smoothstep(0.7, 1.0, d);
                if w <= 0.0 {
                    continue;
                }
                let i = z * N + x;
                let a = c.alpha * w;
                trans[i] *= 1.0 - a;
                top[i] += c.top * a;
                base[i] += c.base * a;
                weight[i] += a;
            }
        }
    }
    let mut near: Vec<f32> = trans.iter().map(|t| 1.0 - t).collect();
    let mut wide = near.clone();
    for _ in 0..2 {
        box_blur(&mut near, NEAR_BLUR);
        box_blur(&mut wide, WIDE_BLUR);
        box_blur(&mut top, HEIGHT_BLUR);
        box_blur(&mut base, HEIGHT_BLUR);
        box_blur(&mut weight, HEIGHT_BLUR);
    }
    let k = MAX_SHADE * strength.min(1.0);
    for i in 0..N * N {
        let cover = NEAR_SHARE * near[i] + (1.0 - NEAR_SHARE) * wide[i];
        let vis = 1.0 - k * cover.clamp(0.0, 1.0);
        let (t, b) = if weight[i] > 1e-4 {
            (top[i] / weight[i], base[i] / weight[i])
        } else {
            (0.0, 0.0)
        };
        words[HEADER_WORDS + i * 2] = pack2x16float(vis, t);
        words[HEADER_WORDS + i * 2 + 1] = pack2x16float(b, near[i]);
    }
    (words, near)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Separable box blur of radius `r` texels (running sums; edges clamp).
fn box_blur(v: &mut [f32], r: usize) {
    let mut row = vec![0.0f32; N];
    let n = (2 * r + 1) as f32;
    let at = |i: isize| i.clamp(0, N as isize - 1) as usize;
    for pass in 0..2 {
        for line in 0..N {
            let idx = |k: usize| if pass == 0 { line * N + k } else { k * N + line };
            let mut sum = 0.0;
            for k in -(r as isize)..=r as isize {
                sum += v[idx(at(k))];
            }
            for k in 0..N {
                row[k] = sum / n;
                sum += v[idx(at(k as isize + r as isize + 1))] - v[idx(at(k as isize - r as isize))];
            }
            for k in 0..N {
                v[idx(k)] = row[k];
            }
        }
    }
}

/// WGSL's `pack2x16float` (round to nearest half).
fn pack2x16float(a: f32, b: f32) -> u32 {
    half::f16::from_f32(a).to_bits() as u32 | (half::f16::from_f32(b).to_bits() as u32) << 16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack(w: u32) -> (f32, f32) {
        (
            half::f16::from_bits(w as u16).to_f32(),
            half::f16::from_bits((w >> 16) as u16).to_f32(),
        )
    }

    fn texel(words: &[u32], origin: Vec2, p: Vec2) -> (f32, f32, f32) {
        let g = ((p - origin) / TEXEL).floor();
        let i = g.y as usize * N + g.x as usize;
        let (vis, top) = unpack(words[HEADER_WORDS + i * 2]);
        let (base, _) = unpack(words[HEADER_WORDS + i * 2 + 1]);
        (vis, top, base)
    }

    #[test]
    fn a_closed_canopy_hides_most_of_the_sky_and_open_ground_keeps_it() {
        let origin = Vec2::new(-384.0, -384.0);
        // a 60 m wide stand of 20 m trees, 5 m apart, around the origin
        let mut crowns = vec![];
        for z in -6..=6 {
            for x in -6..=6 {
                crowns.push(Crown {
                    c: Vec2::new(x as f32 * 5.0, z as f32 * 5.0),
                    r: 3.5,
                    base: 106.0,
                    top: 120.0,
                    alpha: 0.8,
                });
            }
        }
        let (words, cover) = build(origin, &crowns, 1.0, 1.0);
        let (vis, top, base) = texel(&words, origin, Vec2::ZERO);
        let shade = CanopyShade {
            origin: Some(origin),
            cover,
            ..default()
        };
        assert!(shade.cover_at(Vec2::ZERO) > 0.7, "{}", shade.cover_at(Vec2::ZERO));
        assert!(shade.cover_at(Vec2::new(120.0, 0.0)) < 0.01);
        assert!(vis < 0.35, "forest floor sees {vis}");
        assert!((top - 120.0).abs() < 1.0 && (base - 106.0).abs() < 1.0, "{top} {base}");
        let (open, _, _) = texel(&words, origin, Vec2::new(120.0, 0.0));
        assert!(open > 0.999, "open ground sees {open}");
        // the edge of the stand is in between
        let (edge, _, _) = texel(&words, origin, Vec2::new(32.0, 0.0));
        assert!(edge > vis && edge < open, "{vis} < {edge} < {open}");
    }

    #[test]
    fn zero_strength_leaves_the_light_unchanged_but_keeps_the_cover() {
        let origin = Vec2::ZERO;
        let crowns = [Crown {
            c: Vec2::splat(100.0),
            r: 4.0,
            base: 5.0,
            top: 15.0,
            alpha: 0.8,
        }];
        let (words, cover) = build(origin, &crowns, 0.0, 1.0);
        let (vis, _, _) = texel(&words, origin, Vec2::splat(100.0));
        assert_eq!(vis, 1.0, "no shade at 0%");
        assert!(!cover.is_empty(), "the crowns' cover is kept for the undergrowth");
    }
}
