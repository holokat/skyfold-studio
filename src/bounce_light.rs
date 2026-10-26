//! Bounce light while walking. Sunlit walls and cobbles throw warm light into the shade, and
//! narrow lanes, arcades and rooms get darker where buildings hide the sky: the light round the
//! walker is worked out in the background as an irradiance volume (Bevy's light probes: an
//! ambient cube, the light arriving from each of the six sides, every 2 m of a 64 × 24 × 64 m box)
//! that follows them through towns and set pieces. The editor's orbit view keeps the plain sky
//! light. (Dungeons have their own, `dungeons::bounce`.)
//!
//! **Calibrated to the sky light it replaces.** Inside its box the volume *replaces* the
//! environment map's diffuse light (Bevy takes the first diffuse source it finds: lightmap,
//! irradiance volume, environment map; both are multiplied by the albedo the same way), so it
//! starts from exactly that light. The atmosphere's environment cubemap is read back from the GPU
//! every half second, and each side of a voxel begins as the cosine-weighted sky that the
//! environment map's own filtered diffuse gives a surface facing that way: the same units, drawn
//! at the same intensity (`SkyState::bounce_fill`). Only what the rays find changes it, so in
//! open ground nothing does and the lighting looks the same with and without the probe.
//!
//! **The rays.** From each voxel 64 fixed directions march (a 3D DDA) through 0.5 m occupancy
//! cells built from the walker's colliders (`motion::collide`: the column grids of buildings,
//! walls and props, every floor, ceiling and roof of walk-in buildings) and the terrain:
//! - a ray that escapes keeps its share of the sky as the environment map holds it;
//! - a ray down onto open ground keeps it too (the cubemap's lower half is the atmosphere's
//!   sunlit ground), but ground in a building's shadow passes on only the light reaching it;
//! - a ray that hits a wall, a roof or an awning sees that surface instead: its albedo (by the
//!   collider's surface: warm wood, pale stone and plaster, darker cobble) × (sunlight, when one
//!   more ray from it reaches the sun, + the light arriving at it).
//!
//! What arrives at the surfaces the rays hit is worked out twice: first as part of the plain sky,
//! then from the first pass's own volume, so light bounces twice (a shaded wall across from a
//! sunlit one passes some of its glow on; a room's walls only pass on what its windows let in).
//! Open ground in a building's shadow is lit the same way, against the sunlit ground the
//! environment map assumes everywhere: that is where the plain sky light was most wrong.
//!
//! The sun's colour and strength come from the same cubemap: its ground straight down is the
//! atmosphere's 0.3-albedo ground lit by the sun through the air, which is how Bevy shades
//! sunlit surfaces too, so the bounce matches the sunlight on screen at every hour.
//!
//! **Following the walker.** Worked out again when they have moved 8 m, when the sun has moved a
//! degree and a half or the sky's brightness changed by 4 %, or when colliders round them
//! arrive; a new volume cross-fades in over most of a second (the old one shifted onto the new
//! box voxel for voxel, the plain sky light where it didn't reach), so nothing pops. The outer
//! voxels fade to the plain sky light so the box's edge never shows. The work (0.1–0.2 s for the
//! 12 288 voxels × 64 rays, two bounces) runs on one thread of the async compute pool; the main
//! thread only gathers the colliders (shared `Arc`s, themselves built in the background) and a
//! 1 m terrain height grid, and uploads the 0.6 MB texture while a fade runs (about a millisecond
//! a frame at most; the read-back sky is decoded in the background too). The GPU cost is
//! Bevy's: PBR samples the volume per pixel instead of the environment map's diffuse.
//!
//! **Beyond the box.** With an irradiance volume in the view, Bevy's PBR takes no diffuse light
//! from the environment map at all, even where no volume reaches (everything past the box
//! turned black). So a second, one-voxel volume holds the plain sky light over everything in
//! view, with a huge falloff so its weight is tiny wherever the walker's box also reaches.
//!
//! Indoors (`motion::indoors`) the rays find the roof themselves, so the volume isn't cut like
//! the environment map's sky fill; it takes over part of the warm indoor fill (`sky.rs`): rooms
//! are lit by their windows and doors and darken deep inside.

use std::sync::{Arc, OnceLock};

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{
    EnvironmentMapLight, GeneratedEnvironmentMapLight, IrradianceVolume, LightProbe,
};
use bevy::prelude::*;
use bevy::render::gpu_readback::{ReadbackComplete, ReadbackOnce};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use crate::camera::MainCamera;
use crate::motion::collide::{ColliderMeshes, Colliders, ObjCollider};
use crate::objects::{Library, ObjectStore};
use crate::sky::{Lighting, SkyState};
use crate::terrain::Terrain;
use crate::terrain::layers::Surface;
use crate::walk::WalkState;

/// Voxel size (m) and the box in voxels: 64 × 24 × 64 m.
const CELL: f32 = 2.0;
const RES: UVec3 = UVec3::new(32, 12, 32);
/// The box reaches this far below the walker's feet (m).
const BELOW: f32 = 6.0;
/// Moved this far from the box's centre (m, either way along x or z; half that up or down): the
/// box is worked out again round them.
const STEP: f32 = 8.0;
/// Occupancy cells (m), and how far the occupancy reaches past the box: sideways, below, above.
const OCC: f32 = 0.5;
const MARGIN: f32 = 16.0;
const OCC_BELOW: f32 = 4.0;
const OCC_ABOVE: f32 = 12.0;
/// Rays per voxel, and how far one looks (m) before it counts as open.
const RAYS: usize = 64;
const REACH: f32 = 24.0;
/// The atmosphere's ground albedo (`Atmosphere::earth`): the environment map's lower half is
/// that ground, lit by the sun.
const ATMO_ALBEDO: f32 = 0.3;
/// How much of its own sky light a surface among buildings sees (walls see half the sky at
/// most, and the buildings opposite hide some of that).
const SKY_SEEN: f32 = 0.5;
/// Cube faces per side of the downsampled sky (the readback is 128²; the sky is smooth).
const SKY_RES: usize = 32;
/// Seconds a new volume takes to fade in, and between sky readbacks.
const FADE: f32 = 0.8;
const READ_EVERY: f32 = 0.5;
/// The size (m) of the one-voxel volume holding the plain sky light beyond the box: past the
/// far plane.
const BEYOND: f32 = 200_000.0;
/// Bevy shades an irradiance volume's light as `albedo × irradiance` but the environment map's
/// as `(albedo × (1 − specular share) + multiple scattering) × irradiance`, a couple of percent
/// apart for rough surfaces. Measured with `WB_BOUNCE=flat` (every voxel the plain sky light):
/// the meadow and the town square came out 0.1 and 0.2 % brighter with the volume than without,
/// so no correction is needed; the factor stays here in case a material model changes that.
const DIFFUSE: f32 = 1.0;
/// Occupancy codes: air, terrain, and 2 + `Surface::ALL` index for object cells.
const AIR: u8 = 0;
const GROUND: u8 = 1;

/// The bounce light's state (`update`), and what `sky.rs` and MCP ask of it.
#[derive(Resource, Default)]
pub struct BounceLight {
    /// 0..1: how fully a worked-out volume lights the walker's surroundings (it fades in when it
    /// first arrives; 0 in the editor view, with the setting off, in dungeons)
    pub cover: f32,
    /// the last recompute: milliseconds on its background thread, and how many so far
    pub last_ms: f32,
    pub builds: u32,
    s: Option<Box<State>>,
    /// a sky readback in flight (its entity, the cubemap's size, when it was asked for), and its
    /// data once it lands
    reading: Option<(Entity, UVec2, f32)>,
    raw: Option<Vec<u8>>,
}

impl BounceLight {
    /// The light the volume shows at `p` (trilinear between voxel centres, as Bevy samples it)
    /// and the plain sky light, per side (−X, +X, −Y, +Y, −Z, +Z); None outside the box.
    pub fn sample(&self, p: Vec3) -> Option<([Vec3; 6], [Vec3; 6])> {
        let s = self.s.as_ref()?;
        let lo = s.lo?;
        let sky = s.built.as_ref().map(|b| b.sky.env)?;
        let n = RES.as_ivec3();
        if s.shown.len() != (n.x * n.y * n.z) as usize {
            return None;
        }
        if (p - lo).cmplt(Vec3::ZERO).any() || (p - lo).cmpgt(RES.as_vec3() * CELL).any() {
            return None;
        }
        Some((
            std::array::from_fn(|side| lerp_side(&s.shown, lo, p, side)),
            sky,
        ))
    }

    /// One line for MCP (`lighting_set`).
    pub fn describe(&self, on: bool) -> String {
        match (on, self.cover > 0.0) {
            (false, _) => "Bounce light off.".into(),
            (true, false) => "Bounce light on (while walking).".into(),
            (true, true) => format!(
                "Bounce light on: a 64 × 24 × 64 m volume round the walker, worked out {} times (last {:.0} ms in the background).",
                self.builds, self.last_ms
            ),
        }
    }
}

struct State {
    sky: Option<Arc<SkyLight>>,
    /// a read-back sky being decoded in the background
    decoding: Option<Task<Option<SkyLight>>>,
    next_read: f32,
    /// the box being shown (its low corner, on the 2 m lattice)
    lo: Option<Vec3>,
    task: Option<(Built, Task<Out>)>,
    built: Option<Built>,
    /// what is on the GPU now, the fade's ends and its start time
    shown: Vec<[Vec3; 6]>,
    from: Vec<[Vec3; 6]>,
    to: Vec<[Vec3; 6]>,
    fade_t0: Option<f32>,
    last_upload: f32,
    image: Option<Handle<Image>>,
    probe: Option<Entity>,
    /// the plain sky light everywhere beyond the box (`update`), and the sky it holds
    beyond: Option<(Entity, Handle<Image>)>,
    beyond_sky: Option<Arc<SkyLight>>,
    /// colliders: next `ensure`, next fingerprint check, and the latest fingerprint
    ensure_at: f32,
    check_at: f32,
    colliders: u64,
}

/// What a volume was worked out for.
#[derive(Clone)]
struct Built {
    lo: Vec3,
    sun: Vec3,
    env: [f32; 6],
    sky: Arc<SkyLight>,
    colliders: u64,
    at: f32,
}

/// The fixed geometry of the sky's sampling: the downsampled cubemap's texels (world direction,
/// solid angle), the ray directions, which ray's cone each texel falls in, and per ray the
/// cosine weight it carries to each voxel side (Σ max(0, d·n) dω / π over its cone).
struct Table {
    dirs: Vec<Vec3>,
    omega: Vec<f32>,
    rays: [Vec3; RAYS],
    cone: Vec<u8>,
    g: [[f32; 6]; RAYS],
}

/// The six voxel sides, in Bevy's order (−X, +X, −Y, +Y, −Z, +Z).
const SIDES: [Vec3; 6] = [
    Vec3::NEG_X,
    Vec3::X,
    Vec3::NEG_Y,
    Vec3::Y,
    Vec3::NEG_Z,
    Vec3::Z,
];

/// World direction of a cube texel (u, v in −1..1): Bevy's `sample_cube_dir`, z flipped back
/// (cubemaps are left-handed).
fn cube_dir(face: usize, u: f32, v: f32) -> Vec3 {
    let d = match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    }
    .normalize();
    Vec3::new(d.x, d.y, -d.z)
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let s = SKY_RES as f32;
        let mut dirs = Vec::with_capacity(6 * SKY_RES * SKY_RES);
        let mut omega = Vec::with_capacity(6 * SKY_RES * SKY_RES);
        for face in 0..6 {
            for y in 0..SKY_RES {
                for x in 0..SKY_RES {
                    let (u, v) = (
                        (x as f32 + 0.5) / s * 2.0 - 1.0,
                        (y as f32 + 0.5) / s * 2.0 - 1.0,
                    );
                    dirs.push(cube_dir(face, u, v));
                    omega.push((2.0 / s).powi(2) / (1.0 + u * u + v * v).powf(1.5));
                }
            }
        }
        // a Fibonacci sphere: even cones, no pole clustering
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        let rays: [Vec3; RAYS] = std::array::from_fn(|i| {
            let y = 1.0 - (i as f32 + 0.5) / RAYS as f32 * 2.0;
            let r = (1.0 - y * y).sqrt();
            let a = golden * i as f32;
            Vec3::new(a.cos() * r, y, a.sin() * r)
        });
        let cone: Vec<u8> = dirs
            .iter()
            .map(|d| {
                (0..RAYS)
                    .max_by(|&a, &b| rays[a].dot(*d).total_cmp(&rays[b].dot(*d)))
                    .unwrap() as u8
            })
            .collect();
        let mut g = [[0.0; 6]; RAYS];
        for (t, d) in dirs.iter().enumerate() {
            for (f, n) in SIDES.iter().enumerate() {
                g[cone[t] as usize][f] += d.dot(*n).max(0.0) * omega[t] / std::f32::consts::PI;
            }
        }
        Table {
            dirs,
            omega,
            rays,
            cone,
            g,
        }
    })
}

/// The sky as the environment map holds it (unit intensity), as the rays need it.
struct SkyLight {
    /// per ray and side: its cone's share of the side's diffuse light (Σ L max(0, d·n) dω / π)
    c: [[Vec3; 6]; RAYS],
    /// per side: the environment map's diffuse light (Σ over the rays of `c`)
    env: [Vec3; 6],
    /// direct sunlight a white surface facing the sun throws back (E / π, colour and all), and
    /// the direction towards the sun it was read for
    sun: Vec3,
    sun_dir: Vec3,
    /// the sunlit ground straight down (what the environment map's lower half is)
    nadir: Vec3,
    /// open ground in a shadow under the plain sky: its radiance over the sunlit ground's, per
    /// channel (for the log)
    kappa: Vec3,
}

impl SkyLight {
    /// From the radiance of each downsampled texel (`Table::dirs`).
    fn new(rad: &[Vec3], sun_dir: Vec3) -> Self {
        let t = table();
        let mut c = [[Vec3::ZERO; 6]; RAYS];
        let (mut nadir, mut nw) = (Vec3::ZERO, 0.0);
        for (i, d) in t.dirs.iter().enumerate() {
            let l = rad[i];
            for (f, n) in SIDES.iter().enumerate() {
                c[t.cone[i] as usize][f] +=
                    l * (d.dot(*n).max(0.0) * t.omega[i] / std::f32::consts::PI);
            }
            if d.y < -0.9 {
                nadir += l * t.omega[i];
                nw += t.omega[i];
            }
        }
        let nadir = nadir / nw.max(1e-6);
        let mut env = [Vec3::ZERO; 6];
        for ck in &c {
            for f in 0..6 {
                env[f] += ck[f];
            }
        }
        // the ground below is lit as ATMO_ALBEDO / π × E⊥ × cos(sun): E⊥ / π is what a white
        // surface facing the sun throws back
        let up = sun_dir.y;
        let sun = nadir / (ATMO_ALBEDO * up.max(0.04)) * crate::noise::smoothstep(-0.01, 0.04, up);
        let kappa = (ATMO_ALBEDO * env[3] * SKY_SEEN / nadir.max(Vec3::splat(1e-4))).min(Vec3::ONE);
        SkyLight {
            c,
            env,
            sun,
            sun_dir,
            nadir,
            kappa,
        }
    }

    /// Luminance of each side's sky light (what a recompute is compared by).
    fn levels(&self) -> [f32; 6] {
        self.env.map(|e| e.dot(Vec3::new(0.2126, 0.7152, 0.0722)))
    }

    /// From the read-back cubemap (Rgba16Float, `size`² per face, rows padded to 256 bytes).
    fn from_readback(data: &[u8], size: UVec2, sun_dir: Vec3) -> Option<Self> {
        let (w, h) = (size.x as usize, size.y as usize);
        let row = (w * 8).div_ceil(256) * 256;
        if w < SKY_RES || h < SKY_RES || data.len() < row * h * 6 {
            return None;
        }
        let (bx, by) = (w / SKY_RES, h / SKY_RES);
        let f = |o: usize| half::f16::from_le_bytes([data[o], data[o + 1]]).to_f32();
        let mut rad = Vec::with_capacity(6 * SKY_RES * SKY_RES);
        for face in 0..6 {
            for y in 0..SKY_RES {
                for x in 0..SKY_RES {
                    let mut acc = Vec3::ZERO;
                    for j in 0..by {
                        for i in 0..bx {
                            let o = face * row * h + (y * by + j) * row + (x * bx + i) * 8;
                            acc += Vec3::new(f(o), f(o + 2), f(o + 4));
                        }
                    }
                    let v = acc / (bx * by) as f32;
                    rad.push(if v.is_finite() {
                        v.max(Vec3::ZERO)
                    } else {
                        Vec3::ZERO
                    });
                }
            }
        }
        Some(Self::new(&rad, sun_dir))
    }
}

/// Linear albedo of a collider surface (what its walls and roofs throw back).
fn albedo(code: u8) -> Vec3 {
    use Surface::*;
    let Some(s) = code
        .checked_sub(2)
        .and_then(|i| Surface::ALL.get(i as usize))
    else {
        // terrain seen sideways (a slope above the voxel)
        return Vec3::new(0.24, 0.22, 0.18);
    };
    match s {
        Wood => Vec3::new(0.34, 0.24, 0.15),
        // masonry and plaster both say stone
        Stone | WetStone => Vec3::new(0.36, 0.33, 0.29),
        Cobble | Gravel => Vec3::new(0.30, 0.28, 0.25),
        Marble | Bone => Vec3::new(0.55, 0.51, 0.45),
        Sand => Vec3::new(0.45, 0.38, 0.28),
        Salt | Snow | Ice | Crystal => Vec3::new(0.65, 0.66, 0.68),
        Carpet => Vec3::new(0.40, 0.30, 0.22),
        Metal => Vec3::new(0.25, 0.25, 0.26),
        Grass | Leaves => Vec3::new(0.14, 0.18, 0.08),
        Loam | Dirt | Mud => Vec3::new(0.22, 0.18, 0.14),
        Ash | Cinder => Vec3::new(0.12, 0.11, 0.10),
        Water => Vec3::new(0.05, 0.06, 0.07),
        Concrete | Tile => Vec3::new(0.40, 0.39, 0.37),
        Asphalt | Rubber => Vec3::new(0.08, 0.08, 0.08),
        Glass => Vec3::new(0.20, 0.24, 0.26),
        Grate => Vec3::new(0.20, 0.20, 0.21),
    }
}

/// A solid box for the occupancy (tests, and anything that isn't a collider).
#[derive(Clone, Copy)]
struct Piece {
    lo: Vec2,
    hi: Vec2,
    bottom: f32,
    top: f32,
    surface: Surface,
}

/// What a recompute starts from (gathered on the main thread).
struct Input {
    lo: Vec3,
    colliders: Vec<Arc<ObjCollider>>,
    pieces: Vec<Piece>,
    /// terrain heights every 1 m from `h_lo`, `hn` a side
    heights: Vec<f32>,
    h_lo: Vec2,
    hn: usize,
    sky: Arc<SkyLight>,
}

struct Out {
    faces: Vec<[Vec3; 6]>,
    ms: f32,
}

/// The occupancy the rays march through.
struct Occ {
    lo: Vec3,
    n: IVec3,
    cells: Vec<u8>,
    /// the highest layer holding anything solid
    top: i32,
    /// per column: does its ground see the sun (0 / 1), and the ground's height
    lit: Vec<f32>,
    ground: Vec<f32>,
}

/// One ray direction's DDA constants.
#[derive(Clone, Copy)]
struct Dir {
    d: Vec3,
    step: IVec3,
    delta: Vec3,
}

impl Dir {
    fn new(d: Vec3) -> Self {
        let s = |x: f32| {
            if x > 0.0 {
                1
            } else if x < 0.0 {
                -1
            } else {
                0
            }
        };
        let dl = |x: f32| {
            if x != 0.0 {
                OCC / x.abs()
            } else {
                f32::INFINITY
            }
        };
        Dir {
            d,
            step: IVec3::new(s(d.x), s(d.y), s(d.z)),
            delta: Vec3::new(dl(d.x), dl(d.y), dl(d.z)),
        }
    }
}

/// What a ray found: the cell's code, the axis it entered through (0 x, 1 y, 2 z) and the column.
struct Hit {
    code: u8,
    axis: usize,
    col: usize,
    t: f32,
}

impl Occ {
    #[inline]
    fn idx(&self, c: IVec3) -> usize {
        (c.x + self.n.x * (c.z + self.n.z * c.y)) as usize
    }
    fn at(&self, p: Vec3) -> u8 {
        let c = ((p - self.lo) / OCC).floor().as_ivec3();
        if c.y < 0 {
            return GROUND;
        }
        if c.x < 0 || c.z < 0 || c.x >= self.n.x || c.z >= self.n.z || c.y >= self.n.y {
            return AIR;
        }
        self.cells[self.idx(c)]
    }

    /// Marches from `o` along `dir` up to `reach` m: the first solid cell, or None (open: out of
    /// the top, past everything, out of the sides, or farther than `reach`). `soft`: terrain
    /// blocks only where the ray is truly under the ground (the half-metre cells make a staircase
    /// of every slope, and a ray towards the sun from the ground would trip over the next step).
    fn march(&self, o: Vec3, dir: &Dir, reach: f32, soft: bool) -> Option<Hit> {
        let p = (o - self.lo) / OCC;
        let mut c = p.floor().as_ivec3();
        let d = dir.d;
        let next = |i: i32, s: i32, pi: f32, di: f32| {
            if s > 0 {
                ((i + 1) as f32 - pi) * OCC / di
            } else if s < 0 {
                (pi - i as f32) * OCC / -di
            } else {
                f32::INFINITY
            }
        };
        let mut tm = Vec3::new(
            next(c.x, dir.step.x, p.x, d.x),
            next(c.y, dir.step.y, p.y, d.y),
            next(c.z, dir.step.z, p.z, d.z),
        );
        let mut t = 0.0;
        let mut axis = 1;
        loop {
            if c.y >= self.n.y || (c.y > self.top && dir.step.y >= 0) {
                return None;
            }
            if c.x < 0 || c.z < 0 || c.x >= self.n.x || c.z >= self.n.z {
                return None;
            }
            let col = (c.x + self.n.x * c.z) as usize;
            if c.y < 0 {
                return Some(Hit {
                    code: GROUND,
                    axis: 1,
                    col,
                    t,
                });
            }
            let v = self.cells[self.idx(c)];
            if v != AIR && !(soft && v == GROUND && o.y + d.y * t > self.ground[col] - 0.35) {
                return Some(Hit {
                    code: v,
                    axis,
                    col,
                    t,
                });
            }
            if tm.x < tm.y && tm.x < tm.z {
                c.x += dir.step.x;
                t = tm.x;
                tm.x += dir.delta.x;
                axis = 0;
            } else if tm.y < tm.z {
                c.y += dir.step.y;
                t = tm.y;
                tm.y += dir.delta.y;
                axis = 1;
            } else {
                c.z += dir.step.z;
                t = tm.z;
                tm.z += dir.delta.z;
                axis = 2;
            }
            if t > reach {
                return None;
            }
        }
    }

    /// Builds the occupancy round a probe box from the colliders, pieces and terrain.
    fn build(inp: &Input) -> Self {
        let lo = inp.lo - Vec3::new(MARGIN, OCC_BELOW, MARGIN);
        let size =
            RES.as_vec3() * CELL + Vec3::new(2.0 * MARGIN, OCC_BELOW + OCC_ABOVE, 2.0 * MARGIN);
        let n = (size / OCC).round().as_ivec3();
        let mut o = Occ {
            lo,
            n,
            cells: vec![AIR; (n.x * n.y * n.z) as usize],
            top: -1,
            lit: vec![1.0; (n.x * n.z) as usize],
            ground: vec![0.0; (n.x * n.z) as usize],
        };
        // terrain: every cell whose centre is below the ground
        let hgt = |x: f32, z: f32| {
            let g = (Vec2::new(x, z) - inp.h_lo)
                .clamp(Vec2::ZERO, Vec2::splat((inp.hn - 1) as f32 - 0.001));
            let (i, j) = (g.x as usize, g.y as usize);
            let (fx, fz) = (g.x - i as f32, g.y - j as f32);
            let at =
                |i: usize, j: usize| inp.heights[j.min(inp.hn - 1) * inp.hn + i.min(inp.hn - 1)];
            let a = at(i, j) + (at(i + 1, j) - at(i, j)) * fx;
            let b = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * fx;
            a + (b - a) * fz
        };
        for z in 0..n.z {
            for x in 0..n.x {
                let h = hgt(lo.x + (x as f32 + 0.5) * OCC, lo.z + (z as f32 + 0.5) * OCC);
                o.ground[(x + n.x * z) as usize] = h;
                let top = (((h - lo.y) / OCC - 0.5).ceil() as i32).clamp(0, n.y);
                for y in 0..top {
                    let i = o.idx(IVec3::new(x, y, z));
                    o.cells[i] = GROUND;
                }
                o.top = o.top.max(top - 1);
            }
        }
        // objects: each piece fills the cells its footprint covers (a piece smaller than a cell
        // fills the one its centre is in), from its bottom to its top
        let mark = |o: &mut Occ, plo: Vec2, phi: Vec2, b: f32, t: f32, s: Surface| {
            if t - b < 0.05 {
                return;
            }
            let code = 2 + Surface::ALL.iter().position(|x| *x == s).unwrap_or(6) as u8;
            let cell = |v: f32, o0: f32| ((v - o0) / OCC).floor() as i32;
            let span = |a: f32, b: f32, o0: f32, nmax: i32| -> (i32, i32) {
                let (c0, c1) = if b - a < OCC {
                    let c = cell((a + b) * 0.5, o0);
                    (c, c)
                } else {
                    (cell(a + 0.25 * OCC, o0), cell(b - 0.25 * OCC, o0))
                };
                (c0.max(0), c1.min(nmax - 1))
            };
            let (x0, x1) = span(plo.x, phi.x, o.lo.x, o.n.x);
            let (z0, z1) = span(plo.y, phi.y, o.lo.z, o.n.z);
            let y0 = cell(b, o.lo.y).max(0);
            let y1 = cell(t, o.lo.y).min(o.n.y - 1);
            if x0 > x1 || z0 > z1 || y0 > y1 {
                return;
            }
            for z in z0..=z1 {
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        let i = o.idx(IVec3::new(x, y, z));
                        o.cells[i] = code;
                    }
                }
            }
            o.top = o.top.max(y1);
        };
        for c in &inp.colliders {
            if c.max.x < lo.x
                || c.max.z < lo.z
                || c.min.x > lo.x + size.x
                || c.min.z > lo.z + size.z
            {
                continue;
            }
            c.pieces(|plo, phi, b, t, s| mark(&mut o, plo, phi, b, t, s));
        }
        for p in &inp.pieces {
            mark(&mut o, p.lo, p.hi, p.bottom, p.top, p.surface);
        }
        // does the ground of each column see the sun?
        let sun = inp.sky.sun_dir;
        if sun.y > 0.0 && inp.sky.sun.max_element() > 0.0 {
            let dir = Dir::new(sun);
            for z in 0..n.z {
                for x in 0..n.x {
                    let k = (x + n.x * z) as usize;
                    let p = Vec3::new(
                        lo.x + (x as f32 + 0.5) * OCC,
                        o.ground[k].max(lo.y) + 0.1,
                        lo.z + (z as f32 + 0.5) * OCC,
                    );
                    o.lit[k] = if o.march(p, &dir, 120.0, true).is_some() {
                        0.0
                    } else {
                        1.0
                    };
                }
            }
        }
        o
    }
}

/// What one ray from a voxel found (kept for the second pass).
#[derive(Clone, Copy, Default)]
struct RayHit {
    /// distance to the face it hit (m)
    t: f32,
    /// what it hit (`AIR`: nothing, open sky)
    code: u8,
    /// the side the face looks to (its normal; the ground: up)
    side: u8,
    /// the sun reaches that face (or that patch of ground)
    sun: bool,
}

/// The light a voxel at `p` gathers from its rays, per side: `ambient(point, side)` is the light
/// arriving at a face the rays hit (pass one: part of the plain sky; pass two: the first pass's
/// own volume there, so shaded walls and ground also pass on the light of the sunlit ones).
fn gather(
    sky: &SkyLight,
    p: Vec3,
    hits: &[RayHit],
    ambient: &dyn Fn(Vec3, usize) -> Vec3,
) -> [Vec3; 6] {
    let t = table();
    let mut e = [Vec3::ZERO; 6];
    for (k, h) in hits.iter().enumerate() {
        let d = t.rays[k];
        if h.code == AIR || (h.code == GROUND && d.y < 0.0 && h.sun) {
            // open sky, or sunlit open ground: the environment map's own light that way
            for f in 0..6 {
                e[f] += sky.c[k][f];
            }
            continue;
        }
        if h.code == GROUND && d.y < 0.0 {
            // ground in a shadow: lit by what reaches it, against the sunlit ground the
            // environment map holds there
            let q = (ATMO_ALBEDO * ambient(p + d * h.t, 3) / sky.nadir.max(Vec3::splat(1e-4)))
                .min(Vec3::ONE);
            for f in 0..6 {
                e[f] += sky.c[k][f] * q;
            }
            continue;
        }
        // a wall, roof or awning (or a slope above): the light it throws back
        let side = h.side as usize;
        let mut l = ambient(p + d * (h.t - 0.05), side);
        if h.sun {
            l += sky.sun * SIDES[side].dot(sky.sun_dir).max(0.0);
        }
        let l = albedo(h.code) * l;
        for f in 0..6 {
            e[f] += l * t.g[k][f];
        }
    }
    e
}

/// Voxels in the rock or inside solid objects take their neighbours' light (fragments on a
/// wall or the ground sample between them and the voxels in the air).
fn dilate(faces: &mut [[Vec3; 6]], valid: &[bool]) {
    let n = faces.len();
    let idx = |x: i32, y: i32, z: i32| -> Option<usize> {
        (x >= 0
            && y >= 0
            && z >= 0
            && (x as u32) < RES.x
            && (y as u32) < RES.y
            && (z as u32) < RES.z)
            .then(|| (x as u32 + RES.x * (y as u32 + RES.y * z as u32)) as usize)
    };
    let mut valid = valid.to_vec();
    for _ in 0..4 {
        let (prev, was) = (faces.to_vec(), valid.clone());
        for i in 0..n {
            if was[i] {
                continue;
            }
            let (x, y, z) = (
                (i as u32 % RES.x) as i32,
                ((i as u32 / RES.x) % RES.y) as i32,
                (i as u32 / (RES.x * RES.y)) as i32,
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
                    && was[j]
                {
                    for f in 0..6 {
                        acc[f] += prev[j][f];
                    }
                    k += 1.0;
                }
            }
            if k > 0.0 {
                faces[i] = acc.map(|v| v / k);
                valid[i] = true;
            }
        }
    }
}

/// One side of a volume (low corner `lo`) at `p`: trilinear between voxel centres, clamped to
/// the box, as Bevy samples it.
fn lerp_side(faces: &[[Vec3; 6]], lo: Vec3, p: Vec3, side: usize) -> Vec3 {
    let n = RES.as_ivec3();
    let g = ((p - lo) / CELL - 0.5).clamp(Vec3::ZERO, (n - 1).as_vec3());
    let i0 = g.floor().as_ivec3().min(n - 2).max(IVec3::ZERO);
    let f = g - i0.as_vec3();
    let mut out = Vec3::ZERO;
    for c in 0..8 {
        let o = IVec3::new(c & 1, (c >> 1) & 1, (c >> 2) & 1);
        let w = (if o.x == 1 { f.x } else { 1.0 - f.x })
            * (if o.y == 1 { f.y } else { 1.0 - f.y })
            * (if o.z == 1 { f.z } else { 1.0 - f.z });
        let v = (i0 + o).min(n - 1);
        out += faces[(v.x + n.x * (v.y + n.y * v.z)) as usize][side] * w;
    }
    out
}

/// Works the volume out: per voxel, the light arriving at each of its six sides. Two passes:
/// the first lights what the rays hit with part of the plain sky, the second with the first
/// pass's own volume (light bounced twice: a shaded wall across from a sunlit one passes some of
/// its glow on, shaded ground under a sky full of sunlit walls isn't black).
fn compute(inp: Input) -> Out {
    let t0 = std::time::Instant::now();
    let occ = Occ::build(&inp);
    let t = table();
    let sky = &*inp.sky;
    let dirs: Vec<Dir> = t.rays.iter().map(|d| Dir::new(*d)).collect();
    let sun_dir = Dir::new(sky.sun_dir);
    let sunny = sky.sun.max_element() > 0.0 && sky.sun_dir.y > 0.0;
    let n = (RES.x * RES.y * RES.z) as usize;
    let at = |i: usize| -> Vec3 {
        let (x, y, z) = (
            i as u32 % RES.x,
            (i as u32 / RES.x) % RES.y,
            i as u32 / (RES.x * RES.y),
        );
        inp.lo + (Vec3::new(x as f32, y as f32, z as f32) + 0.5) * CELL
    };
    let mut faces = vec![sky.env; n];
    let mut valid = vec![false; n];
    let mut hits = vec![RayHit::default(); n * RAYS];
    let first = |_: Vec3, side: usize| sky.env[side] * SKY_SEEN;
    for i in 0..n {
        let p = at(i);
        if occ.at(p) != AIR {
            continue;
        }
        valid[i] = true;
        let hs = &mut hits[i * RAYS..(i + 1) * RAYS];
        for (k, dir) in dirs.iter().enumerate() {
            let Some(h) = occ.march(p, dir, REACH, false) else {
                continue;
            };
            hs[k] = if h.code == GROUND && dir.d.y < 0.0 {
                RayHit {
                    t: h.t,
                    code: GROUND,
                    side: 3,
                    sun: occ.lit[h.col] > 0.5,
                }
            } else {
                let mut nrm = Vec3::ZERO;
                nrm[h.axis] = -dir.step[h.axis] as f32;
                // (back off into the air cell in front of the face, then towards the sun)
                let sun = sunny
                    && nrm.dot(sky.sun_dir) > 0.0
                    && occ
                        .march(p + dir.d * (h.t - 0.02), &sun_dir, 120.0, true)
                        .is_none();
                RayHit {
                    t: h.t,
                    code: h.code,
                    side: (h.axis * 2 + usize::from(nrm[h.axis] > 0.0)) as u8,
                    sun,
                }
            };
        }
        faces[i] = gather(sky, p, hs, &first);
    }
    dilate(&mut faces, &valid);
    // the second bounce
    let pass1 = faces.clone();
    let second = |q: Vec3, side: usize| lerp_side(&pass1, inp.lo, q, side);
    for i in 0..n {
        if valid[i] {
            faces[i] = gather(sky, at(i), &hits[i * RAYS..(i + 1) * RAYS], &second);
        }
    }
    dilate(&mut faces, &valid);
    // the box's outer voxels fade to the plain sky light (no seam where it ends)
    let s = |d: u32, w: f32| crate::noise::smoothstep(0.0, w, d as f32);
    for (i, f) in faces.iter_mut().enumerate() {
        let (x, y, z) = (
            i as u32 % RES.x,
            (i as u32 / RES.x) % RES.y,
            i as u32 / (RES.x * RES.y),
        );
        let w = s(x.min(RES.x - 1 - x), 3.0) * s(z.min(RES.z - 1 - z), 3.0) * s(RES.y - 1 - y, 2.0);
        for k in 0..6 {
            f[k] = sky.env[k] + (f[k] - sky.env[k]) * w;
        }
    }
    // (`WB_BOUNCE=flat`: the plain sky light in every voxel, for checking the calibration
    // against the environment map: the view must look the same with the volume and without)
    if std::env::var("WB_BOUNCE").is_ok_and(|v| v == "flat") {
        faces.iter_mut().for_each(|f| *f = sky.env);
    }
    Out {
        faces,
        ms: t0.elapsed().as_secs_f32() * 1000.0,
    }
}

/// Packs the sides into Bevy's (Rx, 2Ry, 3Rz) Rgba16Float layout. (As `irradiance_volume.wesl`
/// reads it: a side facing +axis from rows 0..Ry, −axis from Ry..2Ry; the axes one after the
/// other along z. Its doc comment has the halves the other way round.)
fn pack(faces: &[[Vec3; 6]]) -> Vec<u8> {
    let (rx, ry, rz) = (RES.x as usize, RES.y as usize, RES.z as usize);
    let (w, h) = (rx, ry * 2);
    let mut data = vec![0u8; w * h * rz * 3 * 8];
    for (i, f) in faces.iter().enumerate() {
        let (x, y, z) = (i % rx, (i / rx) % ry, i / (rx * ry));
        for (side, v) in f.iter().enumerate() {
            let (axis, negative) = (side / 2, side % 2 == 0);
            let t = y + if negative { ry } else { 0 };
            let p = z + axis * rz;
            let o = ((p * h + t) * w + x) * 8;
            for (k, comp) in [v.x, v.y, v.z, 1.0].iter().enumerate() {
                data[o + k * 2..o + k * 2 + 2]
                    .copy_from_slice(&half::f16::from_f32(comp.min(60_000.0)).to_le_bytes());
            }
        }
    }
    data
}

/// One voxel's six sides in the same layout (a 1 × 2 × 3 texture).
fn pack_one(env: &[Vec3; 6]) -> Vec<u8> {
    let mut data = vec![0u8; 6 * 8];
    for (side, v) in env.iter().enumerate() {
        let (axis, negative) = (side / 2, side % 2 == 0);
        let o = (axis * 2 + usize::from(negative)) * 8;
        for (k, comp) in [v.x, v.y, v.z, 1.0].iter().enumerate() {
            data[o + k * 2..o + k * 2 + 2]
                .copy_from_slice(&half::f16::from_f32(comp.min(60_000.0)).to_le_bytes());
        }
    }
    data
}

fn one_voxel_image(data: Vec<u8>) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: 1,
            height: 2,
            depth_or_array_layers: 3,
        },
        TextureDimension::D3,
        data,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

fn voxel_image(data: Vec<u8>) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: RES.x,
            height: RES.y * 2,
            depth_or_array_layers: RES.z * 3,
        },
        TextureDimension::D3,
        data,
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
    img
}

/// The probe box for a walker standing at `feet`: centred on them (on the 2 m lattice), its
/// floor `BELOW` under their feet.
fn box_for(feet: Vec3) -> Vec3 {
    let snap = |v: f32| (v / CELL).round() * CELL;
    Vec3::new(
        snap(feet.x) - RES.x as f32 * CELL * 0.5,
        snap(feet.y - BELOW),
        snap(feet.z) - RES.z as f32 * CELL * 0.5,
    )
}

/// Is the box at `lo` still good for a walker at `feet`?
fn box_fits(lo: Vec3, feet: Vec3) -> bool {
    let c = lo + RES.as_vec3() * CELL * 0.5;
    (feet.x - c.x).abs() <= STEP
        && (feet.z - c.z).abs() <= STEP
        && (feet.y - (lo.y + BELOW)).abs() <= STEP * 0.5
}

/// Which buildings, walls and other big colliders round the box are ready (their shared
/// shapes' addresses): when it changes, the volume is worked out again. (Tree trunks and props
/// come and go as the walker moves; they don't call for a recompute of their own.)
fn fingerprint(cs: &[Arc<ObjCollider>]) -> u64 {
    cs.iter()
        .filter(|c| c.walk_in || (c.max - c.min).xz().max_element() > 3.0)
        .fold(0u64, |h, c| {
            h.wrapping_add((Arc::as_ptr(c) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        })
}

/// The longest the main thread spent in `update` in one frame (µs), for the log.
static MAIN_US: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static ENSURE_US: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

struct MainCost(std::time::Instant);

impl Drop for MainCost {
    fn drop(&mut self) {
        MAIN_US.fetch_max(
            self.0.elapsed().as_micros() as u32,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

pub struct BounceLightPlugin;

impl Plugin for BounceLightPlugin {
    fn build(&self, app: &mut App) {
        use crate::mcp_tools::{McpAppExt, McpTool};
        app.init_resource::<BounceLight>()
            .add_systems(Update, update.after(crate::sky::SkySystems));
        app.mcp_tool(McpTool {
            name: "bounce_light_probe",
            description: "What the bounce light (lighting_set `bounce_light`, while walking) does at a point: the light its volume gives a surface facing each of the six sides there, next to the plain sky light the editor view would give it (same units; the ratio is what occlusion and bounce changed: below 1 in shade and under roofs, above 1 beside sunlit walls). Without `position`, at the walker's eye.",
            input_schema: || serde_json::json!({"type": "object", "properties": {"position": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "[x, y, z] (m)"}}}),
            run: probe_tool,
        });
    }
}

fn probe_tool(
    world: &mut World,
    args: serde_json::Value,
) -> Result<Vec<crate::mcp_tools::McpContent>, String> {
    use serde_json::json;
    let p: Option<[f32; 3]> = crate::mcp_tools::opt_arg(&args, "position")?;
    let p = match p {
        Some(p) => Vec3::from_array(p),
        None => world
            .query_filtered::<&GlobalTransform, With<MainCamera>>()
            .single(world)
            .map(|t| t.translation())
            .map_err(|_| "no camera")?,
    };
    let b = world.resource::<BounceLight>();
    let Some((v, sky)) = b.sample(p) else {
        return Ok(vec![crate::mcp_tools::McpContent::Text(format!(
            "No bounce light volume at [{:.1}, {:.1}, {:.1}] ({}).",
            p.x,
            p.y,
            p.z,
            b.describe(world.resource::<Lighting>().bounce_light)
        ))]);
    };
    let lum = |v: Vec3| v.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    let names = ["-x", "+x", "-y", "+y", "-z", "+z"];
    let mut sides = serde_json::Map::new();
    for k in 0..6 {
        sides.insert(names[k].into(), json!({"volume": v[k].to_array().map(|c| c.round()), "sky": sky[k].to_array().map(|c| c.round()), "ratio": (lum(v[k]) / lum(sky[k]).max(1e-3) * 1000.0).round() / 1000.0}));
    }
    Ok(vec![crate::mcp_tools::McpContent::Json(
        json!({"position": p.to_array(), "sides": sides, "builds": b.builds, "last_ms": b.last_ms.round()}),
    )])
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    mut b: ResMut<BounceLight>,
    lighting: Res<Lighting>,
    sky: Res<SkyState>,
    walk: Res<WalkState>,
    mut colliders: ResMut<Colliders>,
    mut shapes: ResMut<ColliderMeshes>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    server: Res<AssetServer>,
    terrain: Option<Res<Terrain>>,
    mut images: ResMut<Assets<Image>>,
    env: Query<(&GeneratedEnvironmentMapLight, Option<&EnvironmentMapLight>), With<MainCamera>>,
    mut probes: Query<(&mut IrradianceVolume, &mut Transform)>,
    time: Res<Time<Real>>,
) {
    let _cost = MainCost(std::time::Instant::now());
    let now = time.elapsed_secs();
    let on =
        lighting.bounce_light && walk.active && !crate::dungeons::in_dungeon() && terrain.is_some();
    if !on {
        if let Some(s) = b.s.take() {
            for e in s.probe.into_iter().chain(s.beyond.map(|x| x.0)) {
                commands.entity(e).despawn();
            }
        }
        b.cover = 0.0;
        return;
    }
    let feet = walk.position;
    let b = &mut *b;
    let s = b.s.get_or_insert_with(|| {
        Box::new(State {
            sky: None,
            decoding: None,
            next_read: now,
            lo: None,
            task: None,
            built: None,
            shown: vec![],
            from: vec![],
            to: vec![],
            fade_t0: None,
            last_upload: 0.0,
            image: None,
            probe: None,
            beyond: None,
            beyond_sky: None,
            ensure_at: now,
            check_at: now,
            colliders: 0,
        })
    });

    // the sky: read the environment cubemap back every half second
    if let Some(raw) = b.raw.take()
        && let Some((e, size, _)) = b.reading.take()
    {
        commands.entity(e).despawn();
        let sun = sky.sun_dir;
        s.decoding = Some(
            AsyncComputeTaskPool::get()
                .spawn(async move { SkyLight::from_readback(&raw, size, sun) }),
        );
    }
    if let Some(task) = s.decoding.as_mut()
        && let Some(l) = block_on(future::poll_once(task))
    {
        s.decoding = None;
        if let Some(l) = l {
            s.sky = Some(Arc::new(l));
        }
    }
    // (one that never lands is given up on)
    if let Some((e, _, at)) = b.reading
        && now - at > 3.0
    {
        commands.entity(e).despawn();
        b.reading = None;
    }
    if b.reading.is_none()
        && s.decoding.is_none()
        && now >= s.next_read
        && let Ok((g, _)) = env.single()
        && let Some(img) = images.get(&g.environment_map)
    {
        let size = UVec2::new(
            img.texture_descriptor.size.width,
            img.texture_descriptor.size.height,
        );
        let e = commands
            .spawn((
                Name::new("Bounce light sky readback"),
                ReadbackOnce::texture(g.environment_map.clone()),
            ))
            .observe(|ev: On<ReadbackComplete>, mut b: ResMut<BounceLight>| {
                b.raw = Some(ev.data.clone())
            })
            .id();
        b.reading = Some((e, size, now));
        s.next_read = now + READ_EVERY;
    }

    // colliders round the box, built off the main thread (a dozen started at a time)
    if now >= s.ensure_at
        && let Some(lib) = &lib
    {
        let t0 = std::time::Instant::now();
        colliders.ensure_with(
            feet.xz(),
            RES.x as f32 * CELL * 0.5 + 4.0,
            &store,
            lib,
            &mut shapes,
            &server,
            12,
            true,
        );
        ENSURE_US.fetch_max(
            t0.elapsed().as_micros() as u32,
            std::sync::atomic::Ordering::Relaxed,
        );
        s.ensure_at = now + 0.15;
    }
    let reach = (RES.x as f32 * CELL * 0.5 + MARGIN) * std::f32::consts::SQRT_2;
    if now >= s.check_at
        && let Some(lo) = s.lo
    {
        let c = (lo + RES.as_vec3() * CELL * 0.5).xz();
        s.colliders = fingerprint(&colliders.near(&store, c, reach));
        s.check_at = now + 0.5;
    }

    // a finished volume: fade it in (the old one shifted onto its box, the plain sky where the
    // old box didn't reach)
    if let Some((built, task)) = s.task.as_mut()
        && let Some(out) = block_on(future::poll_once(task))
    {
        let built = built.clone();
        s.task = None;
        b.last_ms = out.ms;
        b.builds += 1;
        if b.builds <= 3
            || b.builds.is_multiple_of(10)
            || std::env::var("WB_BOUNCE").is_ok_and(|v| v == "log")
        {
            let k = s.sky.as_ref();
            info!(
                "bounce light: volume {} worked out in {:.0} ms, main thread at most {:.2} ms a frame since the last ({:.2} of it colliders) (sun {:.2}; sky light by side {:?}; sunlight / π {:?}, shade {:?})",
                b.builds,
                out.ms,
                MAIN_US.swap(0, std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0,
                ENSURE_US.swap(0, std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0,
                built.sun.y,
                built.env.map(|v| v.round()),
                k.map(|k| k.sun.round()),
                k.map(|k| (k.kappa * 100.0).round() / 100.0)
            );
        }
        let env = s.sky.as_ref().map(|k| k.env).unwrap_or([Vec3::ZERO; 6]);
        let n = out.faces.len();
        let mut from = vec![env; n];
        if let Some(old_lo) = s.lo.filter(|_| s.shown.len() == n) {
            let off = ((built.lo - old_lo) / CELL).round().as_ivec3();
            for (i, f) in from.iter_mut().enumerate() {
                let v = IVec3::new(
                    (i as u32 % RES.x) as i32,
                    ((i as u32 / RES.x) % RES.y) as i32,
                    (i as u32 / (RES.x * RES.y)) as i32,
                ) + off;
                if v.cmpge(IVec3::ZERO).all() && v.cmplt(RES.as_ivec3()).all() {
                    *f = s.shown[(v.x as u32 + RES.x * (v.y as u32 + RES.y * v.z as u32)) as usize];
                }
            }
        }
        s.from = from;
        s.to = out.faces;
        s.shown = s.from.clone();
        s.fade_t0 = Some(now);
        s.lo = Some(built.lo);
        s.built = Some(built);
    }

    // start a recompute when the walker left the box, the sky changed or colliders arrived
    if s.task.is_none()
        && let Some(k) = s.sky.clone()
        && let Some(terrain) = &terrain
    {
        let want = match &s.built {
            None => Some(box_for(feet)),
            Some(bt) => {
                let lo = if box_fits(bt.lo, feet) {
                    bt.lo
                } else {
                    box_for(feet)
                };
                let lv = k.levels();
                let sky_moved = bt.sun.dot(k.sun_dir) < 1.5f32.to_radians().cos()
                    || lv
                        .iter()
                        .zip(&bt.env)
                        .any(|(a, b)| (a - b).abs() > 0.04 * b.max(1e-3));
                let more = s.colliders != bt.colliders && now - bt.at > 1.0;
                (lo != bt.lo || sky_moved || more).then_some(lo)
            }
        };
        if let Some(lo) = want {
            // what the rays march through: colliders round the box, a 1 m height grid
            let c = (lo + RES.as_vec3() * CELL * 0.5).xz();
            let cs = colliders.near(&store, c, reach);
            let fp = fingerprint(&cs);
            let h_lo = lo.xz() - Vec2::splat(MARGIN + 1.0);
            let hn = (RES.x as f32 * CELL + 2.0 * MARGIN + 3.0) as usize;
            let mut heights = Vec::with_capacity(hn * hn);
            for j in 0..hn {
                for i in 0..hn {
                    heights.push(terrain.height_at(h_lo.x + i as f32, h_lo.y + j as f32));
                }
            }
            let built = Built {
                lo,
                sun: k.sun_dir,
                env: k.levels(),
                sky: k.clone(),
                colliders: fp,
                at: now,
            };
            s.colliders = fp;
            let inp = Input {
                lo,
                colliders: cs,
                pieces: vec![],
                heights,
                h_lo,
                hn,
                sky: k,
            };
            s.task = Some((
                built,
                AsyncComputeTaskPool::get().spawn(async move { compute(inp) }),
            ));
        }
    }

    // the fade, uploaded at most 30 times a second
    let Some(lo) = s.lo else { return };
    let mut upload = false;
    if let Some(t0) = s.fade_t0 {
        let k = ((now - t0) / FADE).clamp(0.0, 1.0);
        if k >= 1.0 || now - s.last_upload > 1.0 / 30.0 {
            let w = crate::noise::smoothstep(0.0, 1.0, k);
            for ((v, a), c) in s.shown.iter_mut().zip(&s.from).zip(&s.to) {
                for f in 0..6 {
                    v[f] = a[f] + (c[f] - a[f]) * w;
                }
            }
            upload = true;
            s.last_upload = now;
            if k >= 1.0 {
                s.fade_t0 = None;
            }
        }
    }
    let tf = Transform::from_translation(lo + RES.as_vec3() * CELL * 0.5)
        .with_scale(RES.as_vec3() * CELL);
    if upload {
        let data = pack(&s.shown);
        let have = s.image.as_ref().is_some_and(|h| images.contains(h));
        if have {
            if let Some(mut img) = s.image.as_ref().and_then(|h| images.get_mut(h)) {
                img.data = Some(data);
            }
        } else {
            s.image = Some(images.add(voxel_image(data)));
        }
    }
    // (between recomputes the volume follows the sky's brightness)
    let follow = match (&s.sky, &s.built) {
        (Some(k), Some(bt)) => (k.levels()[3] / bt.env[3].max(1e-3)).clamp(0.5, 2.0),
        _ => 1.0,
    };
    let intensity = sky.bounce_fill * DIFFUSE * follow;
    match s.probe.and_then(|e| probes.get_mut(e).ok()) {
        Some((mut v, mut t)) => {
            if (v.intensity - intensity).abs() > 1e-3 {
                v.intensity = intensity;
            }
            if upload && *t != tf {
                *t = tf;
            }
        }
        None => {
            if let Some(h) = s.image.clone() {
                s.probe = Some(
                    commands
                        .spawn((
                            Name::new("Bounce light"),
                            LightProbe::default(),
                            IrradianceVolume {
                                voxels: h,
                                intensity,
                                affects_lightmapped_meshes: false,
                            },
                            tf,
                        ))
                        .id(),
                );
            }
        }
    }
    // beyond the box: the plain sky light, as a one-voxel volume over everything in view (with an
    // irradiance volume in the view, Bevy's PBR takes no diffuse light from the environment map
    // at all, inside a volume or not); its tiny weight (huge falloff) leaves the box's own light
    // alone where they overlap
    if s.probe.is_some()
        && let Some(k) = s.sky.clone()
    {
        let tf = Transform::from_translation(feet).with_scale(Vec3::splat(BEYOND));
        let fill = env
            .single()
            .ok()
            .and_then(|(_, l)| l.map(|l| l.intensity))
            .unwrap_or(sky.bounce_fill)
            * DIFFUSE;
        let fresh = !s.beyond_sky.as_ref().is_some_and(|b| Arc::ptr_eq(b, &k));
        match &s.beyond {
            Some((e, h)) => {
                if fresh && let Some(mut img) = images.get_mut(h) {
                    img.data = Some(pack_one(&k.env));
                }
                if let Ok((mut v, mut t)) = probes.get_mut(*e) {
                    if (v.intensity - fill).abs() > 1e-3 {
                        v.intensity = fill;
                    }
                    if t.translation.distance(feet) > 100.0 {
                        *t = tf;
                    }
                }
            }
            None => {
                let h = images.add(one_voxel_image(pack_one(&k.env)));
                let e = commands
                    .spawn((
                        Name::new("Bounce light (beyond)"),
                        LightProbe {
                            falloff: Vec3::splat(1.0e3),
                        },
                        IrradianceVolume {
                            voxels: h.clone(),
                            intensity: fill,
                            affects_lightmapped_meshes: false,
                        },
                        tf,
                    ))
                    .id();
                s.beyond = Some((e, h));
            }
        }
        s.beyond_sky = Some(k);
    }
    // (the first volume fades in from the plain sky light: so does its share of the indoor fill)
    let first = s.built.as_ref().is_some_and(|_| b.builds <= 1);
    b.cover = match s.fade_t0 {
        Some(t0) if first => {
            crate::noise::smoothstep(0.0, 1.0, ((now - t0) / FADE).clamp(0.0, 1.0))
        }
        _ if s.probe.is_some() => 1.0,
        _ => 0.0,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sky: brighter towards the horizon and the sun's side, a sunlit ground below (as the
    /// atmosphere draws it).
    fn sky(sun: Vec3) -> Arc<SkyLight> {
        let t = table();
        let ground = ATMO_ALBEDO / std::f32::consts::PI * 100_000.0 * sun.y.max(0.0);
        let rad: Vec<Vec3> = t
            .dirs
            .iter()
            .map(|d| {
                if d.y >= 0.0 {
                    Vec3::new(0.6, 0.8, 1.0)
                        * (3000.0 + 2000.0 * (1.0 - d.y) + 1500.0 * d.dot(sun).max(0.0))
                } else {
                    Vec3::new(1.0, 0.95, 0.9) * ground
                }
            })
            .collect();
        Arc::new(SkyLight::new(&rad, sun))
    }

    fn input(pieces: Vec<Piece>, sun: Vec3) -> Input {
        let lo = Vec3::new(-32.0, -6.0, -32.0);
        let h_lo = lo.xz() - Vec2::splat(MARGIN + 1.0);
        let hn = (RES.x as f32 * CELL + 2.0 * MARGIN + 3.0) as usize;
        Input {
            lo,
            colliders: vec![],
            pieces,
            heights: vec![0.0; hn * hn],
            h_lo,
            hn,
            sky: sky(sun),
        }
    }

    fn voxel(p: Vec3) -> usize {
        let v = ((p - Vec3::new(-32.0, -6.0, -32.0)) / CELL)
            .floor()
            .as_uvec3();
        (v.x + RES.x * (v.y + RES.y * v.z)) as usize
    }

    fn lum(v: Vec3) -> f32 {
        v.dot(Vec3::new(0.2126, 0.7152, 0.0722))
    }

    /// The sky's cones add up to the whole sphere, and a surface facing up sees the sky's mean.
    #[test]
    fn sky_weights_cover_the_sphere() {
        let t = table();
        let total: f32 = t.omega.iter().sum();
        assert!((total - 4.0 * std::f32::consts::PI).abs() < 0.01, "{total}");
        // each side's cosine weights sum to one (a white uniform sky gives 1 everywhere)
        for f in 0..6 {
            let g: f32 = t.g.iter().map(|g| g[f]).sum();
            assert!((g - 1.0).abs() < 0.01, "side {f}: {g}");
        }
    }

    /// Open ground: every voxel is exactly the environment map's light (no brightening, no
    /// darkening), the sun shining or not.
    #[test]
    fn open_ground_matches_the_sky() {
        for sun in [
            Vec3::new(0.6, 0.5, 0.3).normalize(),
            Vec3::new(0.3, -0.2, 0.9).normalize(),
        ] {
            let inp = input(vec![], sun);
            let env = inp.sky.env;
            let out = compute(inp);
            for p in [
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(10.0, 3.0, -8.0),
                Vec3::new(-20.0, 12.0, 20.0),
            ] {
                let f = out.faces[voxel(p)];
                for k in 0..6 {
                    assert!(
                        (f[k] - env[k]).abs().max_element() <= 1e-3 * env[k].max_element().max(1.0),
                        "{p} side {k}: {} vs {}",
                        f[k],
                        env[k]
                    );
                }
            }
        }
    }

    /// Under a roof the sky from above is gone; beside a sunlit wall the side facing it gets
    /// its warm bounce (more than the open sky gives), and the ground in the wall's shadow less.
    #[test]
    fn roofs_shade_and_sunlit_walls_bounce() {
        let sun = Vec3::new(1.0, 0.8, 0.0).normalize();
        let roof = Piece {
            lo: Vec2::new(-6.0, -6.0),
            hi: Vec2::new(6.0, 6.0),
            bottom: 4.0,
            top: 4.4,
            surface: Surface::Stone,
        };
        // a wall 3 m east of x = 12, its west face in the sun? no: the sun is in the east, so its
        // sunlit face looks east; stand east of it
        let wall = Piece {
            lo: Vec2::new(14.0, -10.0),
            hi: Vec2::new(14.5, 10.0),
            bottom: -1.0,
            top: 9.0,
            surface: Surface::Wood,
        };
        let inp = input(vec![roof, wall], sun);
        let env = inp.sky.env;
        let out = compute(inp);
        let under = out.faces[voxel(Vec3::new(0.0, 1.0, 0.0))];
        assert!(
            lum(under[3]) < 0.5 * lum(env[3]),
            "under the roof, up: {} vs {}",
            lum(under[3]),
            lum(env[3])
        );
        // west of the wall (x = 12, in its shadow): the side facing it (+X) sees a shaded wall
        let west = out.faces[voxel(Vec3::new(12.5, 1.0, 0.0))];
        assert!(
            lum(west[1]) < lum(env[1]),
            "shaded wall: {} vs {}",
            lum(west[1]),
            lum(env[1])
        );
        // east of it (x = 16.5, sunlit side): the side facing it (−X) gets a warm, bright bounce
        let east = out.faces[voxel(Vec3::new(16.5, 1.0, 0.0))];
        assert!(
            lum(east[0]) > lum(env[0]),
            "sunlit wall: {} vs {}",
            lum(east[0]),
            lum(env[0])
        );
        assert!(
            east[0].x / east[0].z > env[0].x / env[0].z,
            "warmer: {} vs {}",
            east[0],
            env[0]
        );
    }

    /// The packing matches what Bevy's shader reads: +X of voxel (1, 2, 3) at (1, 2, 3), −X a
    /// half further down, +Y in the second third.
    #[test]
    fn packing_follows_the_shader() {
        let n = (RES.x * RES.y * RES.z) as usize;
        let mut faces = vec![[Vec3::ZERO; 6]; n];
        let i = (1 + RES.x * (2 + RES.y * 3)) as usize;
        faces[i] = [
            Vec3::splat(1.0),
            Vec3::splat(2.0),
            Vec3::splat(3.0),
            Vec3::splat(4.0),
            Vec3::splat(5.0),
            Vec3::splat(6.0),
        ];
        let data = pack(&faces);
        let (w, h) = (RES.x as usize, RES.y as usize * 2);
        let read = |x: usize, t: usize, p: usize| {
            half::f16::from_le_bytes([
                data[((p * h + t) * w + x) * 8],
                data[((p * h + t) * w + x) * 8 + 1],
            ])
            .to_f32()
        };
        assert_eq!(read(1, 2, 3), 2.0);
        assert_eq!(read(1, 2 + RES.y as usize, 3), 1.0);
        assert_eq!(read(1, 2, 3 + RES.z as usize), 4.0);
        assert_eq!(read(1, 2 + RES.y as usize, 3 + 2 * RES.z as usize), 5.0);
    }
}
