//! Waterfalls: drag from a cliff top (or a river's edge) downhill. The water leaves the lip and
//! falls ballistically, hugging the rock where it can't clear it. Two layered sheets follow that
//! drop: a translucent outer sheet, glassy over the lip and streaking white lower down, around an
//! aerated white core; fine bright spray flies off the foot, a faint mist rises and drifts from
//! it (soft where it meets the ground and the water), and foam boils out and is carried across
//! the plunge pool, which is carved there unless the fall lands in water already: a real basin
//! (a drawn lake that holds its water, dug into the slope behind a raised lip). Width, flow, mist
//! and the water style (any preset's colours, glow included) are editable; waterfalls are saved
//! with the world. Like the rest of the world they hold still while motion is frozen.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};

use super::{WaterBodies, WaterStyle};
use crate::terrain::Terrain;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Waterfall {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    /// The falling sheet's centreline, top to bottom (y follows the drop).
    pub path: Vec<[f32; 3]>,
    pub width: f32,
    /// 0.2 (a trickle, torn into strands) .. 3 (a thundering torrent)
    pub flow: f32,
    /// 0..1 spray and mist at the foot
    pub mist: f32,
    pub style: WaterStyle,
    /// The plunge pool (a drawn lake) carved at the foot, if any.
    #[serde(default)]
    pub pool: Option<u32>,
}

impl Waterfall {
    pub fn top(&self) -> Vec3 {
        Vec3::from_array(self.path[0])
    }
    pub fn bottom(&self) -> Vec3 {
        Vec3::from_array(*self.path.last().unwrap())
    }
    pub fn drop(&self) -> f32 {
        self.top().y - self.bottom().y
    }
    /// Horizontal distance from `p` to the fall's footprint.
    pub fn distance_xz(&self, p: Vec2) -> f32 {
        self.path
            .windows(2)
            .map(|w| {
                let (a, b) = (Vec2::new(w[0][0], w[0][2]), Vec2::new(w[1][0], w[1][2]));
                let ab = b - a;
                let u = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                p.distance(a + ab * u)
            })
            .fold(f32::MAX, f32::min)
    }
}

/// The fall's path from a dragged top and foot: launched over the lip, falling under gravity,
/// never cutting into the ground, landing at the foot's water or ground level.
pub fn build_path(
    t: &Terrain,
    water: &WaterBodies,
    a: Vec3,
    b: Vec3,
    flow: f32,
) -> Result<(Vec<Vec3>, bool), &'static str> {
    let (mut a, mut b) = (a, b);
    if a.y < b.y {
        std::mem::swap(&mut a, &mut b);
    }
    // the top sits on the river or lake it spills from, else just over the ground
    let top_y = water
        .water_level(Vec3::new(a.x, t.height_at(a.x, a.z), a.z))
        .unwrap_or(t.height_at(a.x, a.z) + 0.15);
    let foot_ground = t.height_at(b.x, b.z);
    let foot_water = water.surface_at(Vec3::new(b.x, foot_ground, b.z), t.sea_level);
    let lands_in_water = foot_water > foot_ground + 0.3;
    let foot_y = foot_water.max(foot_ground);
    if top_y - foot_y < 1.5 {
        return Err(
            "Drag from higher ground down to lower ground (at least a couple of metres of drop)",
        );
    }
    Ok((
        ballistic(
            t,
            Vec3::new(a.x, top_y, a.z),
            Vec3::new(b.x, foot_y, b.z),
            flow,
        ),
        lands_in_water,
    ))
}

/// The fall's centreline from the lip `a` to the foot `b` (their heights are the water's): launched
/// over the lip, falling under gravity, never cutting into the ground.
pub fn ballistic(t: &Terrain, a: Vec3, b: Vec3, flow: f32) -> Vec<Vec3> {
    let (top_y, foot_y) = (a.y, b.y);
    let drop = top_y - foot_y;
    let horiz = (b.xz() - a.xz()).length();
    let dir = (b.xz() - a.xz()).normalize_or(Vec2::X);
    const G: f32 = 9.81;
    // launch speed over the lip; slow enough that the water lands by the end of the drag
    let mut v0 = 1.2 + flow * 1.6;
    let t_fall = (2.0 * drop.max(0.1) / G).sqrt();
    v0 = v0.min(horiz.max(0.3) / t_fall);
    let n = 28;
    let mut pts = Vec::with_capacity(n);
    let len = horiz.max(0.3);
    for i in 0..n {
        let s = len * i as f32 / (n - 1) as f32;
        let q = a.xz() + dir * s;
        let fall = 0.5 * G * (s / v0.max(0.05)).powi(2);
        let ground = t.height_at(q.x, q.y) + 0.3;
        let y = (top_y - fall).max(ground).max(foot_y).min(top_y);
        pts.push(Vec3::new(q.x, y, q.y));
    }
    // the last point is the foot
    if let Some(l) = pts.last_mut() {
        l.y = foot_y;
    }
    // a vertical cliff: give the sheet a little horizontal room so it isn't a zero-width ribbon
    if horiz < 0.6 {
        for (i, p) in pts.iter_mut().enumerate() {
            let k = i as f32 / (n - 1) as f32;
            let off = dir * (k * k * 1.5);
            p.x = a.x + off.x;
            p.z = a.z + off.y;
        }
    }
    pts
}

/// Where a fall's plunge pool goes: its centre (a little downstream of the impact), radius (never
/// under a few terrain cells, so the grid can shape the bowl) and water level. On a slope the pool
/// is mostly dug into the hillside and partly held behind a raised lip: the surface sits below the
/// foot by three quarters of what the land falls away across the pool (beyond the first metre).
pub fn pool_site(t: &Terrain, foot: Vec3, dir: Vec2, width: f32) -> (Vec2, f32, f32) {
    let r = (width * 0.75 + 2.5).max(t.cell * 2.5);
    let c = foot.xz() + dir * r * 0.3;
    let mut ring: Vec<f32> = (0..48)
        .map(|k| {
            let a = k as f32 / 48.0 * std::f32::consts::TAU;
            let q = c + Vec2::new(a.cos(), a.sin()) * r * 1.1;
            t.height_at(q.x, q.y)
        })
        .collect();
    ring.sort_by(f32::total_cmp);
    let ground = t.height_at(foot.x, foot.z);
    let falls_away = (ground - ring[4] - 1.0).max(0.0);
    (
        c,
        r,
        (ground + 0.15 - 0.75 * falls_away).max(t.sea_level + 0.3),
    )
}

/// The fall's centreline cut where it first reaches the water at `level` (the rest would lie flat
/// on the surface), resampled to as many evenly spaced points as before.
pub fn land_on(path: &[Vec3], level: f32) -> Vec<Vec3> {
    let n = path.len();
    let Some(i) = path
        .iter()
        .position(|p| p.y <= level + 0.05)
        .filter(|i| *i >= 1)
    else {
        return path.to_vec();
    };
    let (a, b) = (path[i - 1], path[i]);
    let u = ((a.y - level) / (a.y - b.y).max(1e-4)).clamp(0.0, 1.0);
    let mut cut: Vec<Vec3> = path[..i].to_vec();
    cut.push(a.lerp(b, u).with_y(level));
    // even spacing along the cut path
    let lens: Vec<f32> = cut.windows(2).map(|w| w[0].distance(w[1])).collect();
    let total: f32 = lens.iter().sum();
    if total < 0.5 {
        return path.to_vec();
    }
    let mut out = Vec::with_capacity(n);
    let (mut seg, mut start) = (0usize, 0.0f32);
    for k in 0..n {
        let d = total * k as f32 / (n - 1) as f32;
        while seg < lens.len() - 1 && d > start + lens[seg] {
            start += lens[seg];
            seg += 1;
        }
        let u = ((d - start) / lens[seg].max(1e-5)).clamp(0.0, 1.0);
        out.push(cut[seg].lerp(cut[seg + 1], u));
    }
    out
}

/// Where a river's surface drops steeply (at least 2.5 m, steeper than ~25°): the stretches a
/// waterfall should pour down, as (lip, foot) on the river's surface.
pub fn river_drops(points: &[[f32; 3]]) -> Vec<(Vec3, Vec3)> {
    let mut out = vec![];
    let mut i = 0;
    while i + 1 < points.len() {
        // grow the steep stretch while each step keeps falling steeply
        let mut j = i;
        while j + 1 < points.len() {
            let (p, q) = (Vec3::from_array(points[j]), Vec3::from_array(points[j + 1]));
            if (p.y - q.y) < 0.47 * p.xz().distance(q.xz()) {
                break;
            }
            j += 1;
        }
        let (a, b) = (Vec3::from_array(points[i]), Vec3::from_array(points[j]));
        if j > i && a.y - b.y >= 2.5 {
            out.push((a, b));
        }
        i = j.max(i + 1);
    }
    out
}

pub type WaterfallMaterial = ExtendedMaterial<StandardMaterial, WaterfallExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct FallParams {
    /// rgb: water colour (linear), w: mode (0 sheet, 1 mist and spray, 2 plunge foam)
    pub color: Vec4,
    /// rgb: depth colour (linear), w: width (m)
    pub deep: Vec4,
    /// rgb: glow colour × strength × darkness
    pub glow: Vec4,
    /// rgb: inner glow (emissive) × strength, w: the fall's clock while motion is frozen (s;
    /// negative: it runs with the world's)
    pub emis: Vec4,
    /// x: flow, y: mist, z: sheet length (m), w: drop (m)
    pub flow: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct WaterfallExt {
    #[uniform(100)]
    pub params: FallParams,
    #[texture(101)]
    #[sampler(102)]
    pub noise: Handle<Image>,
    #[texture(103)]
    pub foam: Handle<Image>,
}

impl MaterialExtension for WaterfallExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/waterfall.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/waterfall.wesl".into()
    }
}

/// Render entities of one waterfall (despawned when the water rebuilds).
#[derive(Component)]
pub struct FallMesh {
    pub id: u32,
    pub material: Handle<WaterfallMaterial>,
}

fn lin(c: [f32; 3]) -> Vec3 {
    let l = Color::srgb(c[0], c[1], c[2]).to_linear();
    Vec3::new(l.red, l.green, l.blue)
}

pub fn fall_params(f: &Waterfall, mode: f32, darkness: f32, clock: f32) -> FallParams {
    let s = &f.style;
    let len: f32 = f
        .path
        .windows(2)
        .map(|w| Vec3::from_array(w[0]).distance(Vec3::from_array(w[1])))
        .sum();
    FallParams {
        color: lin(s.shallow).extend(mode),
        deep: lin(s.deep).extend(f.width),
        glow: (lin(s.glow) * s.glow_strength * darkness).extend(0.0),
        emis: (lin(s.emissive) * s.emissive_strength).extend(clock),
        flow: Vec4::new(f.flow, f.mist, len, f.drop()),
    }
}

/// The falling water: two ribbons along the path, widening as they fall and bowed out a little,
/// an aerated core (vertex colour w = 0) a little behind and narrower than the translucent outer
/// sheet (w = 1). Where the water slides down a slope instead of falling free, both lift off the
/// rock a little so the tumbling white water reads, and they drape over the ground wherever it
/// rises across the slope. Vertex colour: (progress down the fall, steepness, edge 0..1..0,
/// layer); uv: (across 0..1, metres along); the normal faces the rock (as the triangles do).
pub fn sheet_mesh(f: &Waterfall, t: &Terrain) -> Mesh {
    sheet_mesh_over(f, |x, z| t.height_at(x, z))
}

/// The sheet over whatever ground `ground` (x, z → height) describes (a fall inside a cave:
/// nothing to drape over, `f32::MIN`).
pub fn sheet_mesh_over(f: &Waterfall, ground: impl Fn(f32, f32) -> f32) -> Mesh {
    let pts: Vec<Vec3> = f.path.iter().map(|p| Vec3::from_array(*p)).collect();
    let n = pts.len();
    let horiz = (pts[n - 1].xz() - pts[0].xz()).normalize_or(Vec2::X);
    let side = Vec3::new(-horiz.y, 0.0, horiz.x);
    let cols = 11;
    let mut pos = vec![];
    let mut nor = vec![];
    let mut uv = vec![];
    let mut col = vec![];
    let mut idx = vec![];
    for (layer, scale, off, clear) in [(0.0f32, 0.72f32, -0.12f32, 0.2f32), (1.0, 1.0, 0.14, 0.35)]
    {
        let base = pos.len() as u32;
        let mut along = 0.0;
        for i in 0..n {
            if i > 0 {
                along += pts[i].distance(pts[i - 1]);
            }
            let p = i as f32 / (n - 1) as f32;
            let prev = pts[i.saturating_sub(1)];
            let next = pts[(i + 1).min(n - 1)];
            let tan = (next - prev).normalize_or(Vec3::NEG_Y);
            // away from the rock: out from a cliff, up off a slope
            let out = side
                .cross(tan)
                .normalize_or(Vec3::new(horiz.x, 0.0, horiz.y));
            let normal = -out;
            let steep = (-tan.y).clamp(0.0, 1.0);
            // (clear of the rock even down a steep wall; a cascade down a slope stands a little
            // prouder of it)
            let lift = 0.25 + (1.0 - steep) * 0.3;
            let w = f.width * (1.0 + 0.35 * p) * scale;
            for j in 0..cols {
                let u = j as f32 / (cols - 1) as f32;
                let x = (u - 0.5) * w;
                let bow = (1.0 - (2.0 * u - 1.0).powi(2)) * w * 0.06 * (0.3 + p);
                let mut q = pts[i] + side * x + out * (bow + off * (0.4 + p) + lift);
                q.y = q.y.max(ground(q.x, q.z) + clear);
                pos.push(q.to_array());
                nor.push(normal.to_array());
                uv.push([u, along]);
                col.push([p, steep, 1.0 - (2.0 * u - 1.0).abs(), layer]);
            }
        }
        for i in 0..n as u32 - 1 {
            for j in 0..cols as u32 - 1 {
                let a = base + i * cols as u32 + j;
                let b = a + cols as u32;
                idx.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

/// Mist and spray at the foot: camera-facing puffs (expanded in the vertex shader) that rise,
/// swell and fade on their own cycles around the impact, a few hanging lower along the fall, and
/// a burst of fine spray flung out of the impact (negative opacity marks spray). Vertex colour:
/// (phase, size, seed, opacity); uv: corner; normal.z: the level the fall lands at (the shader
/// keeps the puffs above it and fades them into it).
pub fn mist_mesh(f: &Waterfall) -> Mesh {
    let foot = f.bottom();
    let top = f.top();
    let mut rng = crate::noise::Rng::new(f.id as u64 * 31 + 7);
    let count = (6.0 + f.mist * 16.0 * (0.6 + 0.4 * f.flow.min(2.0))).round() as usize;
    let mut pos = vec![];
    let mut uv = vec![];
    let mut col = vec![];
    let mut nor = vec![];
    let mut idx = vec![];
    let lands_at = [0.0, 1.0, foot.y];
    let horiz = (foot.xz() - top.xz()).normalize_or(Vec2::X);
    let mut quad = |c: Vec3,
                    v: [f32; 4],
                    pos: &mut Vec<[f32; 3]>,
                    uv: &mut Vec<[f32; 2]>,
                    col: &mut Vec<[f32; 4]>,
                    nor: &mut Vec<[f32; 3]>| {
        let base = pos.len() as u32;
        for corner in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
            pos.push(c.to_array());
            uv.push(corner);
            nor.push(lands_at);
            col.push(v);
        }
        idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
    };
    for k in 0..count {
        // most puffs billow from the impact; a few hang along the lowest part of the fall
        let low = k % 6 != 5;
        let c = if low {
            let a = rng.f32() * std::f32::consts::TAU;
            let r = f.width * (0.1 + 0.5 * rng.f32().sqrt());
            foot + Vec3::new(
                a.cos() * r + horiz.x * f.width * 0.25,
                0.3 + rng.f32() * 0.9,
                a.sin() * r + horiz.y * f.width * 0.25,
            )
        } else {
            let p = 0.75 + rng.f32() * 0.2;
            let i = ((f.path.len() - 1) as f32 * p) as usize;
            Vec3::from_array(f.path[i])
                + Vec3::new(
                    rng.range(-0.3, 0.3) * f.width,
                    0.0,
                    rng.range(-0.3, 0.3) * f.width,
                )
        };
        let size = f.width * rng.range(0.3, 0.55) * (0.75 + 0.25 * f.flow.min(2.5));
        quad(
            c,
            [rng.f32(), size, rng.f32(), if low { 1.0 } else { 0.5 }],
            &mut pos,
            &mut uv,
            &mut col,
            &mut nor,
        );
    }
    // spray: droplets flung out of the impact on short ballistic arcs
    let spray = (10.0 + 24.0 * f.flow.min(3.0) * (0.4 + 0.6 * f.mist)).round() as usize;
    for _ in 0..spray {
        let a = rng.f32() * std::f32::consts::TAU;
        let r = f.width * 0.3 * rng.f32().sqrt();
        let c = foot + Vec3::new(a.cos() * r, 0.2, a.sin() * r);
        let size = f.width * rng.range(0.08, 0.16) * (0.8 + 0.2 * f.flow.min(2.5));
        quad(
            c,
            [rng.f32(), size, rng.f32(), -1.0],
            &mut pos,
            &mut uv,
            &mut col,
            &mut nor,
        );
    }
    // two unused corner vertices: the bounds cover the puffs as they rise and spread (GPU culling)
    let r = f.width * 3.0 + 8.0;
    for c in [
        foot - Vec3::new(r, 2.0, r),
        foot + Vec3::new(r, f.width * 3.0 + 12.0, r),
    ] {
        pos.push(c.to_array());
        uv.push([0.0, 0.0]);
        nor.push(lands_at);
        col.push([0.0, 0.0, 0.0, 0.0]);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

/// Foam boiling out of the impact over the pool (or the water the fall lands in): a disc on the
/// surface reaching across a plunge pool, uv = metres from the impact, vertex colour: (radius, 0,
/// 0, 1).
pub fn foam_mesh(f: &Waterfall, level: f32) -> Mesh {
    let foot = f.bottom();
    let radius =
        f.width * (0.9 + 0.35 * f.flow.min(3.0)) + if f.pool.is_some() { 4.0 } else { 0.0 };
    let (rings, segs) = (8, 48);
    let mut pos = vec![];
    let mut uv = vec![];
    let mut col = vec![];
    let mut nor = vec![];
    for r in 0..=rings {
        let k = r as f32 / rings as f32;
        for s in 0..segs {
            let a = s as f32 / segs as f32 * std::f32::consts::TAU;
            let o = Vec2::new(a.cos(), a.sin()) * radius * k;
            pos.push([foot.x + o.x, level + 0.04, foot.z + o.y]);
            uv.push(o.to_array());
            col.push([radius, 0.0, 0.0, 1.0]);
            nor.push([0.0, 1.0, 0.0]);
        }
    }
    let mut idx = vec![];
    for r in 0..rings as u32 {
        for s in 0..segs as u32 {
            let (a, b) = (r * segs as u32 + s, r * segs as u32 + (s + 1) % segs as u32);
            let (c, d) = (a + segs as u32, b + segs as u32);
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

fn new_material(
    mats: &mut Assets<WaterfallMaterial>,
    f: &Waterfall,
    mode: f32,
    tex: &crate::ocean::OceanTextures,
    darkness: f32,
) -> Handle<WaterfallMaterial> {
    // (update_falls sets the clock as soon as the material exists)
    mats.add(WaterfallMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.4,
            reflectance: 0.3,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        extension: WaterfallExt {
            params: fall_params(f, mode, darkness, -1.0),
            noise: tex.noise.clone(),
            foam: tex.foam.clone(),
        },
    })
}

/// Spawn the sheet, the mist and spray and the plunge foam of every waterfall.
pub fn spawn_falls(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<WaterfallMaterial>,
    tex: &crate::ocean::OceanTextures,
    falls: &[Waterfall],
    terrain: &Terrain,
    darkness: f32,
) {
    for f in falls {
        if f.path.len() < 2 {
            continue;
        }
        let sheet = new_material(mats, f, 0.0, tex, darkness);
        commands.spawn((
            Name::new(format!("Waterfall {}", f.id)),
            FallMesh {
                id: f.id,
                material: sheet.clone(),
            },
            Mesh3d(meshes.add(sheet_mesh(f, terrain))),
            MeshMaterial3d(sheet),
            Transform::IDENTITY,
            NotShadowCaster,
        ));
        if f.mist > 0.01 {
            let mist = new_material(mats, f, 1.0, tex, darkness);
            commands.spawn((
                Name::new(format!("Waterfall {} mist", f.id)),
                FallMesh {
                    id: f.id,
                    material: mist.clone(),
                },
                Mesh3d(meshes.add(mist_mesh(f))),
                MeshMaterial3d(mist),
                Transform::IDENTITY,
                NotShadowCaster,
                // the puffs are expanded in the vertex shader: the mesh bounds are only their centres
                bevy::camera::visibility::NoFrustumCulling,
            ));
        }
        // foam boils out where it lands in water (its plunge pool, a river, a lake or the sea)
        let foot = f.bottom();
        if f.pool.is_some() || foot.y > terrain.height_at(foot.x, foot.z) + 0.2 {
            let foam = new_material(mats, f, 2.0, tex, darkness);
            commands.spawn((
                Name::new(format!("Waterfall {} foam", f.id)),
                FallMesh {
                    id: f.id,
                    material: foam.clone(),
                },
                Mesh3d(meshes.add(foam_mesh(f, foot.y))),
                MeshMaterial3d(foam),
                Transform::IDENTITY,
                NotShadowCaster,
            ));
        }
    }
}

/// Spawns falls that are not the world's water (a cave's): sheet, mist and spray, foam where they
/// land in water at `level`s; no ground to drape over. Returns the entities (the owner despawns
/// them).
pub fn spawn_free(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<WaterfallMaterial>,
    tex: &crate::ocean::OceanTextures,
    falls: &[(Waterfall, Option<f32>)],
    darkness: f32,
) -> Vec<Entity> {
    let mut out = vec![];
    for (f, level) in falls {
        if f.path.len() < 2 {
            continue;
        }
        let sheet = new_material(mats, f, 0.0, tex, darkness);
        out.push(
            commands
                .spawn((
                    Name::new(format!("Waterfall {}", f.name)),
                    FallMesh {
                        id: f.id,
                        material: sheet.clone(),
                    },
                    Mesh3d(meshes.add(sheet_mesh_over(f, |_, _| f32::MIN))),
                    MeshMaterial3d(sheet),
                    Transform::IDENTITY,
                    NotShadowCaster,
                ))
                .id(),
        );
        if f.mist > 0.01 {
            let mist = new_material(mats, f, 1.0, tex, darkness);
            out.push(
                commands
                    .spawn((
                        Name::new(format!("Waterfall {} mist", f.name)),
                        FallMesh {
                            id: f.id,
                            material: mist.clone(),
                        },
                        Mesh3d(meshes.add(mist_mesh(f))),
                        MeshMaterial3d(mist),
                        Transform::IDENTITY,
                        NotShadowCaster,
                        bevy::camera::visibility::NoFrustumCulling,
                    ))
                    .id(),
            );
        }
        if let Some(l) = level {
            let foam = new_material(mats, f, 2.0, tex, darkness);
            out.push(
                commands
                    .spawn((
                        Name::new(format!("Waterfall {} foam", f.name)),
                        FallMesh {
                            id: f.id,
                            material: foam.clone(),
                        },
                        Mesh3d(meshes.add(foam_mesh(f, *l))),
                        MeshMaterial3d(foam),
                        Transform::IDENTITY,
                        NotShadowCaster,
                    ))
                    .id(),
            );
        }
    }
    out
}

/// Keep the materials in step with the falls' settings (live slider edits), the night glow and
/// the world's motion: frozen, the falls hold the moment they stopped at.
#[allow(clippy::too_many_arguments)]
pub fn update_falls(
    water: Res<WaterBodies>,
    sky: Res<crate::sky::SkyState>,
    motion: Res<crate::wind::Motion>,
    time: Res<Time>,
    q: Query<&FallMesh>,
    added: Query<(), Added<FallMesh>>,
    mut mats: ResMut<Assets<WaterfallMaterial>>,
    (mut last, mut frozen): (Local<(f32, u64)>, Local<Option<f32>>),
) {
    let clock = if motion.live {
        *frozen = None;
        -1.0
    } else {
        *frozen.get_or_insert(time.elapsed_secs_wrapped())
    };
    // a cheap fingerprint of the settings that only change uniforms
    let mut fp = 0u64;
    for f in &water.waterfalls {
        let s = &f.style;
        for v in [
            f.width,
            f.flow,
            f.mist,
            s.shallow[0],
            s.shallow[1],
            s.shallow[2],
            s.deep[0],
            s.deep[1],
            s.deep[2],
            s.glow_strength,
            s.emissive_strength,
            s.glow[0],
            s.glow[1],
            s.glow[2],
            s.emissive[0],
            s.emissive[1],
            s.emissive[2],
        ] {
            fp = fp.wrapping_mul(31).wrapping_add(v.to_bits() as u64);
        }
    }
    fp = fp.wrapping_mul(31).wrapping_add(clock.to_bits() as u64);
    let dark = sky.night;
    if (dark - last.0).abs() < 0.02 && fp == last.1 && added.is_empty() {
        return;
    }
    *last = (dark, fp);
    for m in &q {
        let Some(f) = water.waterfalls.iter().find(|f| f.id == m.id) else {
            continue;
        };
        if let Some(mut mat) = mats.get_mut(&m.material) {
            let mode = mat.extension.params.color.w;
            mat.extension.params = fall_params(f, mode, dark, clock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A river that runs gently, drops 6 m over 8 m and runs on finds exactly that fall.
    #[test]
    fn river_drops_found() {
        let mut pts = vec![];
        let mut y = 40.0;
        for k in 0..30 {
            if (10..12).contains(&k) {
                y -= 3.0;
            } else {
                y -= 0.1;
            }
            pts.push([k as f32 * 4.0, y, 0.0]);
        }
        let drops = river_drops(&pts);
        assert_eq!(drops.len(), 1, "{drops:?}");
        let (a, b) = drops[0];
        assert!(a.y - b.y >= 5.9 && a.y - b.y < 7.0);
        assert!(river_drops(&pts[..9]).is_empty());
    }

    /// A fall placed down a 45° hillside gets a plunge pool that really holds its water: the
    /// ground under the surface lies below it, a lip stands above it right round the pool (so
    /// no water hangs over the slope below), the surface mesh ends at the smooth outline, and
    /// the sheet ends on the water.
    #[test]
    fn plunge_pool_holds_its_water_on_a_slope() {
        let mut t = Terrain::new(128, 4.0, 0.0);
        let n = t.n();
        for j in 0..n {
            for i in 0..n {
                let p = t.grid_to_world(i, j);
                let k = t.idx(i, j);
                t.heights[k] = 150.0 - p.x;
            }
        }
        let (mut h, mut store, mut water) = (
            crate::history::History::default(),
            crate::objects::ObjectStore::default(),
            WaterBodies::default(),
        );
        let s = crate::water::WaterToolSettings::default();
        let a = Vec3::new(-20.0, t.height_at(-20.0, 0.0), 0.0);
        let b = Vec3::new(10.0, t.height_at(10.0, 0.0), 0.0);
        let id =
            crate::water::create_waterfall(&mut t, &mut h, &mut store, None, &mut water, a, b, &s)
                .unwrap();
        let f = water.waterfalls.iter().find(|f| f.id == id).unwrap();
        let lake = water.lakes.iter().find(|l| Some(l.id) == f.pool).unwrap();
        assert!(lake.contained);
        let poly: Vec<Vec2> = lake.outline.iter().map(|p| Vec2::from_array(*p)).collect();
        let c = crate::water::lakes::centroid(&poly);
        let level = lake.level;
        assert!(
            (f.bottom().y - level).abs() < 0.01,
            "the sheet ends on the water"
        );
        assert!(
            crate::water::lakes::inside(&poly, f.bottom().xz()),
            "the sheet lands in the pool"
        );
        for k in 0..72 {
            let dir = Vec2::from_angle(k as f32 / 72.0 * std::f32::consts::TAU);
            // out from the centre: under water up to the outline, then a lip above it
            let mut r = 0.0;
            let mut held = false;
            while r < 40.0 {
                let q = c + dir * r;
                let d = crate::water::lakes::sdf(&poly, q);
                let g = t.height_at(q.x, q.y);
                if d > 0.0 {
                    assert!(
                        g < level,
                        "dry ground {:.2} m above the water inside the outline at {q}",
                        g - level
                    );
                } else if g > level + 0.05 {
                    held = true;
                    break;
                }
                r += 0.25;
            }
            assert!(held, "the water runs out towards {dir}");
        }
        // the surface ends on the outline (no grid steps beyond it)
        let mesh = crate::water::lakes::lake_mesh(&poly, level);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!()
        };
        for p in pos {
            assert!(crate::water::lakes::sdf(&poly, Vec2::new(p[0], p[2])) > -0.3);
        }
    }
}
