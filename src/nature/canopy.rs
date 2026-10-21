//! The far field: a forest canopy instead of individual trees (hierarchical LOD). Beyond the
//! billboards each 256 m chunk is one coarse sheet draped over the land, raised to the crowns'
//! height and coloured by the species where the rules and density maps grow trees, tucked under
//! the ground elsewhere. A 16 km overview draws a few hundred of these, never millions of trees.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoCpuCulling, VisibilityRange};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::prelude::*;
use rayon::prelude::*;

use super::field::texel_of;
use super::render::{NatureEntity, billboard_end};
use super::rules::NONE;
use super::{Ctx, NatureParams};
use crate::camera::MainCamera;
use crate::procgen::meshes::AssetKind;
use crate::streaming::Graphics;

/// Canopy chunk size (m) and grid step (m).
pub const CHUNK: f32 = 256.0;
const STEP: f32 = 16.0;
const VERTS: usize = (CHUNK / STEP) as usize + 1;
/// How far the canopy reaches at least (m), whatever the draw distance.
const FAR: f32 = 16_000.0;

#[derive(Resource, Default)]
pub struct Canopy {
    map: HashMap<IVec2, (Option<Entity>, u64)>,
    stale: HashSet<IVec2>,
    material: Option<Handle<crate::forest_contours::ForestMaskMaterial>>,
    last_end: f32,
    /// where the last full pass looked from, and whether chunks were left to build
    last_pass: Option<Vec3>,
    pending: bool,
}

/// Typical crown colour (linear) of a tree species seen from afar.
fn crown_color(k: AssetKind) -> [f32; 3] {
    use AssetKind::*;
    let s = match k {
        Pine | Fir | Spruce | Cedar => [0.13, 0.2, 0.11],
        SnowyPine => [0.42, 0.47, 0.46],
        Oak => [0.2, 0.28, 0.1],
        Birch => [0.3, 0.37, 0.14],
        Maple => [0.55, 0.3, 0.1],
        GoldenMaple | Ginkgo => [0.62, 0.48, 0.08],
        SakuraPink => [0.72, 0.28, 0.42],
        SakuraWhite => [0.85, 0.78, 0.72],
        Sequoia => [0.2, 0.3, 0.1],
        Bamboo => [0.24, 0.42, 0.12],
        Willow => [0.27, 0.33, 0.14],
        Palm => [0.27, 0.35, 0.13],
        DeadTree => [0.3, 0.26, 0.2],
        Cactus => [0.25, 0.32, 0.18],
        _ => [0.2, 0.27, 0.12],
    };
    // a sunlit crown seen from afar reads lighter than its shaded leaves up close
    s.map(|c| Color::srgb(c * 1.6, c * 1.6, c * 1.6).to_linear().red)
}

/// Tree cover at a spot: (cover 0..1, crown height m, colour).
fn cover_at(ctx: &Ctx, p: Vec2) -> (f32, f32, [f32; 3]) {
    let t = ctx.terrain;
    if !t.in_bounds(p.x, p.y) {
        return (0.0, 0.0, [0.0; 3]);
    }
    let tx = texel_of(p.x, p.y);
    let tex = ctx.field.texel(tx);
    let (pi, f) = ctx
        .rules
        .preset_at(t, ctx.zones, p, ctx.seed, tex.preset, ctx.field.fallback);
    let k = f * tex.mult[0] * ctx.amounts[0];
    // tree species growing here: the preset's layers and painted ones (kind, entry, per m², scale)
    let mut trees: Vec<(AssetKind, usize, f32, f32)> = vec![];
    if pi != NONE {
        let forest = ctx.forest_at(p);
        trees.extend(ctx.rules.presets[pi as usize].cats[0].iter().map(|l| {
            let kind =
                ctx.rules
                    .resolve_tree(l.kind, p, ctx.seed, forest, ctx.forest_ecology_at(p));
            (
                kind,
                ctx.rules.kind_entry[kind as usize].unwrap_or(l.entry),
                l.density * k,
                (l.scale.0 + l.scale.1) * 0.5,
            )
        }));
    }
    let mut adds = vec![];
    ctx.field.adds(tx, &mut adds);
    for (kind, d) in adds {
        let Some(kind) = super::kind_from_u8(kind)
            .filter(|k| crate::objects::kind_category(*k) == crate::objects::Category::Trees)
        else {
            continue;
        };
        if let Some(entry) = ctx.rules.kind_entry[kind as usize] {
            trees.push((kind, entry, d * ctx.amounts[0], 1.0));
        }
    }
    let (mut area, mut h, mut w, mut col) = (0.0, 0.0, 0.0, [0.0f32; 3]);
    for (kind, entry, d, scale) in trees {
        let e = &ctx.lib.entries[entry];
        let width = e
            .tree
            .as_ref()
            .and_then(|t| t.variants.first())
            .map(|v| v.width)
            .unwrap_or(6.0)
            * scale;
        area += d * width * width * 0.6;
        h += d * e.height * scale;
        let c = crown_color(kind);
        for i in 0..3 {
            col[i] += c[i] * d;
        }
        w += d;
    }
    if w <= 0.0 {
        return (0.0, 0.0, [0.0; 3]);
    }
    // the land: no canopy in water or on the beach
    let ground = t.height_at(p.x, p.y);
    if ground - t.sea_level < 0.5 || ctx.water.is_underwater(Vec3::new(p.x, ground, p.y), 0.3) {
        return (0.0, 0.0, [0.0; 3]);
    }
    (1.0 - (-area).exp(), h / w, col.map(|c| c / w))
}

fn build(ctx: &Ctx, c: IVec2) -> (IVec2, Option<Mesh>, u64) {
    let o = c.as_vec2() * CHUNK;
    let n = VERTS + 2;
    let mut ys = vec![0.0f32; n * n];
    let mut cols = vec![[0.0f32; 4]; n * n];
    let mut any = false;
    let t = ctx.terrain;
    for j in 0..n {
        for i in 0..n {
            let p = o + (Vec2::new(i as f32, j as f32) - 1.0) * STEP;
            let (cover, h, col) = cover_at(ctx, p);
            let ground = t.height_at(
                p.x.clamp(-t.half(), t.half()),
                p.y.clamp(-t.half(), t.half()),
            );
            // crowns bunch into irregular masses; thin cover sinks out of sight
            let n1 = crate::noise::fbm(p.x * 0.02, p.y * 0.02, 2, 2.0, 0.5, 991) * 0.5 + 0.5;
            let up = crate::noise::smoothstep(0.12, 0.45, cover);
            let y = ground + h * (0.55 + 0.3 * n1) * up
                - 3.0 * (1.0 - crate::noise::smoothstep(0.05, 0.2, cover));
            ys[j * n + i] = y;
            // crowns and gaps: fine noise breaks the sheet into canopy texture
            let n2 = crate::noise::value2(p.x * 0.11, p.y * 0.11, 993);
            let shade = (0.6 + 0.4 * n1) * (0.75 + 0.45 * n2);
            cols[j * n + i] = [col[0] * shade, col[1] * shade, col[2] * shade, 1.0];
            any |= cover > 0.05;
        }
    }
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        for (y, c) in ys.iter().zip(&cols) {
            y.to_bits().hash(&mut hs);
            c.map(f32::to_bits).hash(&mut hs);
        }
        hs.finish()
    };
    if !any {
        return (c, None, hash);
    }
    let mut pos = Vec::with_capacity(VERTS * VERTS);
    let mut nor = Vec::with_capacity(VERTS * VERTS);
    let mut col = Vec::with_capacity(VERTS * VERTS);
    for j in 1..n - 1 {
        for i in 1..n - 1 {
            let y = ys[j * n + i];
            pos.push([(i - 1) as f32 * STEP, y, (j - 1) as f32 * STEP]);
            let dx = (ys[j * n + i + 1] - ys[j * n + i - 1]) / (2.0 * STEP);
            let dz = (ys[(j + 1) * n + i] - ys[(j - 1) * n + i]) / (2.0 * STEP);
            // crowns scatter light: normals lean up so the sheet never looks like a slab
            nor.push(
                (Vec3::new(-dx, 1.0, -dz).normalize() + Vec3::Y * 0.6)
                    .normalize()
                    .to_array(),
            );
            col.push(cols[j * n + i]);
        }
    }
    let mut idx: Vec<u16> = vec![];
    for j in 0..VERTS - 1 {
        for i in 0..VERTS - 1 {
            let a = (j * VERTS + i) as u16;
            let b = a + 1;
            let cc = a + VERTS as u16;
            let d = cc + 1;
            idx.extend_from_slice(&[a, cc, b, b, cc, d]);
        }
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U16(idx));
    (c, Some(m), hash)
}

fn chunk_rect(c: IVec2) -> Rect {
    let o = c.as_vec2() * CHUNK;
    Rect::from_corners(o, o + CHUNK)
}

#[allow(clippy::too_many_arguments)]
pub fn update_canopy(
    mut commands: Commands,
    mut canopy: ResMut<Canopy>,
    params: NatureParams,
    graphics: Res<Graphics>,
    mut mats: ResMut<Assets<crate::forest_contours::ForestMaskMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    stroke: Res<crate::tools::Stroke>,
) {
    let (Some(ctx), Ok(cam)) = (params.ctx(), cam.single()) else {
        return;
    };
    let p = cam.translation();
    let start = billboard_end(&graphics);
    // the canopy is cheap: it carries on to the horizon (zoomed-out views see every forest)
    let dd = graphics.draw_distance.max(FAR);
    let mat = canopy
        .material
        .get_or_insert_with(|| {
            mats.add(crate::forest_contours::ForestMaskMaterial {
                base: StandardMaterial {
                    base_color: Color::WHITE,
                    perceptual_roughness: 0.95,
                    reflectance: 0.08,
                    ..default()
                },
                extension: crate::forest_contours::ForestMaskExt::default(),
            })
        })
        .clone();
    if canopy.last_end != start {
        // the hand-over distance moved: every chunk's visibility range changes
        canopy.last_end = start;
        let all: Vec<IVec2> = canopy.map.keys().copied().collect();
        canopy.stale.extend(all);
    }
    for r in &params.field.dirty {
        let keys: Vec<IVec2> = canopy
            .map
            .keys()
            .copied()
            .filter(|c| !chunk_rect(*c).intersect(*r).is_empty())
            .collect();
        canopy.stale.extend(keys);
    }
    if (canopy.stale.is_empty() || stroke.active)
        && !canopy.pending
        && !graphics.is_changed()
        && canopy.last_pass.is_some_and(|q| q.distance(p) < 32.0)
    {
        return;
    }
    canopy.last_pass = Some(p);
    let t = ctx.terrain;
    let dist = |c: IVec2| {
        let r = chunk_rect(c);
        let q = p.xz().clamp(r.min, r.max);
        let g = if t.in_bounds(q.x, q.y) {
            t.height_at(q.x, q.y)
        } else {
            t.sea_level
        };
        (q.distance(p.xz()).powi(2) + (p.y - g).powi(2)).sqrt()
    };
    let far = |c: IVec2| {
        let r = chunk_rect(c);
        let d = (p.xz() - r.center()).abs() + CHUNK * 0.5;
        d.length()
    };
    let mut gone = vec![];
    canopy.map.retain(|c, (e, _)| {
        let keep = dist(*c) <= dd + 200.0 && far(*c) >= start * 0.8 - 200.0;
        if !keep && let Some(e) = e.take() {
            gone.push(e);
        }
        keep
    });
    // (painting: the far field catches up once the stroke ends)
    // (chunks stay stale until rebuilt: a big change takes several frames' budgets)
    let Canopy { stale, map, .. } = &mut *canopy;
    stale.retain(|c| map.contains_key(c));
    let mut todo: Vec<(f32, IVec2)> = if stroke.active {
        vec![]
    } else {
        canopy.stale.iter().map(|c| (dist(*c), *c)).collect()
    };
    let half = t.half();
    let c0 = (p.xz() / CHUNK - dd / CHUNK).floor().as_ivec2();
    let c1 = (p.xz() / CHUNK + dd / CHUNK).floor().as_ivec2();
    for z in c0.y..=c1.y {
        for x in c0.x..=c1.x {
            let c = IVec2::new(x, z);
            let r = chunk_rect(c);
            if r.max.x < -half
                || r.max.y < -half
                || r.min.x > half
                || r.min.y > half
                || canopy.map.contains_key(&c)
            {
                continue;
            }
            let d = dist(c);
            if d <= dd && far(c) >= start * 0.8 {
                todo.push((d, c));
            }
        }
    }
    if !todo.is_empty() {
        todo.sort_by(|a, b| a.0.total_cmp(&b.0));
        let t0 = Instant::now();
        let mut at = 0;
        while at < todo.len() && t0.elapsed().as_secs_f32() < 0.003 {
            let batch = &todo[at..(at + 16).min(todo.len())];
            at += batch.len();
            let built: Vec<(IVec2, Option<Mesh>, u64)> =
                batch.par_iter().map(|(_, c)| build(&ctx, *c)).collect();
            for (c, mesh, hash) in built {
                canopy.stale.remove(&c);
                if canopy.map.get(&c).is_some_and(|(_, h)| *h == hash) {
                    continue;
                }
                if let Some((Some(e), _)) = canopy.map.remove(&c) {
                    gone.push(e);
                }
                let e = mesh.map(|m| {
                    let o = c.as_vec2() * CHUNK;
                    commands
                        .spawn((
                            Name::new("Forest canopy"),
                            Mesh3d(meshes.add(m)),
                            MeshMaterial3d(mat.clone()),
                            Transform::from_translation(Vec3::new(o.x, 0.0, o.y)),
                            // fades in as the billboards fade out
                            // (whole chunks: they appear just inside the draw distance, where the
                            // billboards are thinnest and fading out)
                            VisibilityRange {
                                start_margin: start * 0.9..start * 0.9,
                                end_margin: dd..dd,
                                use_aabb: true,
                            },
                            NotShadowCaster,
                            NoCpuCulling,
                            NatureEntity(5),
                            MeshTag::new(crate::forest_contours::FOREST_TAG),
                        ))
                        .id()
                });
                canopy.map.insert(c, (e, hash));
            }
        }
        canopy.pending = at < todo.len();
    } else {
        canopy.pending = false;
    }
    for e in gone {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.despawn();
        }
    }
}
