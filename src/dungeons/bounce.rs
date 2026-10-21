//! Bounce light. The light of a dungeon's fires, lamps and shafts is thrown back off its walls
//! and floors, so a torch-lit room glows warm all round (the sides facing away included) while a
//! dark corridor next to it stays dark.
//!
//! This is an irradiance volume (Bevy's light probes: Half-Life 2 ambient cubes) worked out in
//! the background from the lights and the rock. Each light reaches a voxel only along a clear line
//! through the same space the walker walks, so no light leaks through walls. Worked out again when
//! the plan or the lights change.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{IrradianceVolume, LightProbe};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use super::lights::LightDef;
use super::model::DungeonMap;
use super::plan::Compiled;

/// How much of the light the stone throws back (an average dungeon albedo), and a softening.
const ALBEDO: f32 = 0.3;

/// A light as the bounce sees it: where, its colour (linear), lumens, reach.
#[derive(Clone, Debug, PartialEq)]
struct Src {
    pos: Vec3,
    color: Vec3,
    lumens: f32,
    range: f32,
}

/// The worked-out volume: its box and the texture's data (Rgba16Float, (Rx, 2Ry, 3Rz)).
struct Out {
    lo: Vec3,
    hi: Vec3,
    res: UVec3,
    data: Vec<u8>,
}

#[derive(Resource, Default)]
pub struct Bounce {
    want: u64,
    built: u64,
    task: Option<(u64, Task<Option<Out>>)>,
    since: f32,
    probe: Option<Entity>,
}

fn fingerprint(c: &Arc<Compiled>, lights: &[Src]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (Arc::as_ptr(c) as usize).hash(&mut h);
    for l in lights {
        for v in l.pos.to_array().iter().chain(l.color.to_array().iter()) {
            v.to_bits().hash(&mut h);
        }
        l.lumens.to_bits().hash(&mut h);
        l.range.to_bits().hash(&mut h);
    }
    h.finish()
}

fn sources(m: &super::model::MapDoc) -> Vec<Src> {
    m.lights
        .iter()
        .map(|l: &LightDef| {
            let c = Color::srgb_from_array(m.mood.tinted(l.color)).to_linear();
            let (lumens, range, _) = m.all_lights.apply(l);
            Src {
                pos: l.position(),
                color: Vec3::new(c.red, c.green, c.blue),
                lumens,
                range: range.max(1.0),
            }
        })
        .collect()
}

/// Works the volume out (in the background) when the dungeon or its lights changed; puts it in
/// the scene when ready (half a second after the last change, so dragging a light doesn't
/// recompute every frame).
pub(super) fn update(
    mut commands: Commands,
    mut b: ResMut<Bounce>,
    map: Res<DungeonMap>,
    world: Res<super::render::DungeonWorld>,
    mut images: ResMut<Assets<Image>>,
    mut probes: Query<(&mut Transform, &mut IrradianceVolume)>,
    time: Res<Time<Real>>,
) {
    let Some(m) = map
        .0
        .as_ref()
        .filter(|m| m.kind == super::model::MapKind::Interior)
    else {
        if let Some(e) = b.probe.take() {
            commands.entity(e).despawn();
        }
        b.want = 0;
        b.built = 0;
        return;
    };
    let Some(c) = world.compiled.clone() else {
        return;
    };
    let src = sources(m);
    let fp = fingerprint(&c, &src);
    if fp != b.want {
        b.want = fp;
        b.since = time.elapsed_secs();
    }
    if b.task.is_none() && b.want != b.built && time.elapsed_secs() - b.since > 0.5 {
        let want = b.want;
        b.task = Some((
            want,
            AsyncComputeTaskPool::get().spawn(async move { compute(&c, &src) }),
        ));
    }
    let Some((want, task)) = b.task.as_mut() else {
        return;
    };
    let want = *want;
    let Some(out) = block_on(future::poll_once(task)) else {
        return;
    };
    b.task = None;
    b.built = want;
    let Some(out) = out else {
        if let Some(e) = b.probe.take() {
            commands.entity(e).despawn();
        }
        return;
    };
    let mut img = Image::new(
        Extent3d {
            width: out.res.x,
            height: out.res.y * 2,
            depth_or_array_layers: out.res.z * 3,
        },
        TextureDimension::D3,
        out.data,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    let voxels = images.add(img);
    let tf = Transform::from_translation((out.lo + out.hi) * 0.5).with_scale(out.hi - out.lo);
    match b.probe.and_then(|e| probes.get_mut(e).ok()) {
        Some((mut t, mut v)) => {
            *t = tf;
            v.voxels = voxels;
        }
        None => {
            let e = commands
                .spawn((
                    Name::new("Dungeon bounce light"),
                    LightProbe::default(),
                    IrradianceVolume {
                        voxels,
                        intensity: 1.0,
                        affects_lightmapped_meshes: false,
                    },
                    tf,
                ))
                .id();
            b.probe = Some(e);
        }
    }
}

/// Is the line from `a` to `b` clear (through the air)?
fn clear(c: &Compiled, a: Vec3, b: Vec3) -> bool {
    let d = b - a;
    let len = d.length();
    if len < 0.4 {
        return true;
    }
    let dir = d / len;
    let mut t = 0.3;
    while t < len - 0.3 {
        let f = c.field(a + dir * t);
        if f < 0.02 {
            return false;
        }
        t += f.clamp(0.2, 2.0);
    }
    true
}

/// The volume: a voxel every couple of metres over the dungeon (at most 64 a side); in each, the
/// light bouncing in from each of the six sides.
fn compute(c: &Compiled, src: &[Src]) -> Option<Out> {
    use rayon::prelude::*;
    if src.is_empty() {
        return None;
    }
    let (lo, hi) = (c.lo, c.hi);
    let ext = (hi - lo).max(Vec3::splat(1.0));
    let cell = (ext.max_element() / 64.0).max(2.0);
    let res = (ext / cell).ceil().as_uvec3().max(UVec3::splat(2));
    let n = (res.x * res.y * res.z) as usize;
    let at = |i: usize| -> Vec3 {
        let (x, y, z) = (
            i as u32 % res.x,
            (i as u32 / res.x) % res.y,
            i as u32 / (res.x * res.y),
        );
        lo + (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / res.as_vec3() * ext
    };
    // per voxel: the six sides' radiance (−X +X −Y +Y −Z +Z)
    let mut faces: Vec<[Vec3; 6]> = (0..n)
        .into_par_iter()
        .map(|i| {
            let p = at(i);
            let mut out = [Vec3::ZERO; 6];
            if c.field(p) < -0.4 {
                return out;
            }
            for l in src {
                let d = l.pos.distance(p);
                if d > l.range * 1.4 {
                    continue;
                }
                // the light falling round here (as on a surface), windowed to its reach, and thrown
                // back by the stone round about: some from everywhere, more from the far side
                let win = (1.0 - (d / (l.range * 1.4)).powi(4)).max(0.0).powi(2);
                let e = l.lumens / (4.0 * std::f32::consts::PI * (d * d + 2.0)) * win;
                if e < 0.5 || !clear(c, p, l.pos) {
                    continue;
                }
                let rad = l.color * e * ALBEDO / std::f32::consts::PI;
                let to = (l.pos - p).normalize_or(Vec3::Y);
                let dirs = [
                    Vec3::NEG_X,
                    Vec3::X,
                    Vec3::NEG_Y,
                    Vec3::Y,
                    Vec3::NEG_Z,
                    Vec3::Z,
                ];
                for (k, n) in dirs.iter().enumerate() {
                    // a face looking away from the light sees the lit walls beyond it
                    out[k] +=
                        rad * (0.75 + 0.25 * (-n.dot(to)).max(0.0) + 0.15 * n.dot(to).max(0.0));
                }
            }
            out
        })
        .collect();
    // voxels in the rock take their nearest neighbours' light (fragments on the walls sample
    // between a voxel in the air and one in the rock)
    let idx = |x: i32, y: i32, z: i32| -> Option<usize> {
        (x >= 0
            && y >= 0
            && z >= 0
            && (x as u32) < res.x
            && (y as u32) < res.y
            && (z as u32) < res.z)
            .then(|| (x as u32 + y as u32 * res.x + z as u32 * res.x * res.y) as usize)
    };
    let solid: Vec<bool> = (0..n).map(|i| c.field(at(i)) < -0.4).collect();
    for _ in 0..2 {
        let prev = faces.clone();
        for i in 0..n {
            if !solid[i] {
                continue;
            }
            let (x, y, z) = (
                (i as u32 % res.x) as i32,
                ((i as u32 / res.x) % res.y) as i32,
                (i as u32 / (res.x * res.y)) as i32,
            );
            let mut acc = [Vec3::ZERO; 6];
            let mut k = 0.0;
            for (dx, dy, dz) in [
                (-1, 0, 0),
                (1, 0, 0),
                (0, -1, 0),
                (0, 1, 0),
                (0, 0, -1),
                (0, 0, 1),
            ] {
                if let Some(j) = idx(x + dx, y + dy, z + dz)
                    && prev[j].iter().any(|v| v.length_squared() > 0.0)
                {
                    for f in 0..6 {
                        acc[f] += prev[j][f];
                    }
                    k += 1.0;
                }
            }
            if k > 0.0 {
                for f in 0..6 {
                    faces[i][f] = acc[f] / k;
                }
            }
        }
    }
    // pack: (x, y + Ry·positive side, z + Rz·axis)
    let (w, h, dd) = (res.x as usize, res.y as usize * 2, res.z as usize * 3);
    let mut data = vec![0u8; w * h * dd * 8];
    for i in 0..n {
        let (x, y, z) = (
            (i as u32 % res.x) as usize,
            ((i as u32 / res.x) % res.y) as usize,
            (i as u32 / (res.x * res.y)) as usize,
        );
        for (side, v) in faces[i].iter().enumerate() {
            let axis = side / 2;
            let positive = side % 2 == 1;
            let t = y + if positive { res.y as usize } else { 0 };
            let p = z + axis * res.z as usize;
            let o = ((p * h + t) * w + x) * 8;
            for (k, comp) in [v.x, v.y, v.z, 1.0].iter().enumerate() {
                let b = half::f16::from_f32(*comp).to_le_bytes();
                data[o + k * 2] = b[0];
                data[o + k * 2 + 1] = b[1];
            }
        }
    }
    Some(Out {
        lo,
        hi: lo + ext,
        res,
        data,
    })
}
