//! Heightfield terrain: data model, quadtree LOD meshes, GPU textures and the terrain material.

pub mod ab;
pub mod bake;
pub mod brush;
pub mod continent;
pub mod erosion;
pub mod gpu;
pub mod holes;
pub mod layers;
pub mod material;
pub mod mesh;
pub mod mips;
pub mod overlay;
pub mod realm_land;
pub mod textures;
pub mod worldgen;

use bevy::prelude::*;
use layers::{LAYER_COUNT, Layer};

use crate::noise::smoothstep;

/// Inclusive rectangle in grid (vertex) coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridRect {
    pub x0: usize,
    pub z0: usize,
    pub x1: usize,
    pub z1: usize,
}

impl GridRect {
    pub fn union(self, o: GridRect) -> GridRect {
        GridRect {
            x0: self.x0.min(o.x0),
            z0: self.z0.min(o.z0),
            x1: self.x1.max(o.x1),
            z1: self.z1.max(o.z1),
        }
    }
    pub fn width(&self) -> usize {
        self.x1 - self.x0 + 1
    }
    pub fn height(&self) -> usize {
        self.z1 - self.z0 + 1
    }
    pub fn expand(self, by: usize, n: usize) -> GridRect {
        GridRect {
            x0: self.x0.saturating_sub(by),
            z0: self.z0.saturating_sub(by),
            x1: (self.x1 + by).min(n - 1),
            z1: (self.z1 + by).min(n - 1),
        }
    }
}

fn union_opt(a: Option<GridRect>, b: GridRect) -> Option<GridRect> {
    Some(match a {
        Some(a) => a.union(b),
        None => b,
    })
}

/// Auto-texturing rules: the "style" of the terrain. Mirrored by the terrain shader.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StyleRules {
    /// Height above sea level where snow starts (m).
    pub snow_line: f32,
    /// Vertical blend band of the snow line (m).
    pub snow_blend: f32,
    /// Slope (0 = flat, 1 = vertical) where exposed rock begins / is fully rock.
    pub rock_slope: f32,
    /// Beach height above sea level (m).
    pub beach_height: f32,
    /// 0..1, how much dry grass mixes into meadows.
    pub dryness: f32,
    /// 0..1, wet sheen (rain).
    pub wetness: f32,
    /// 0..1, how much damp soil, mud and stream gravel show where rain gathers (gullies,
    /// stream beds, valley floors; from the terrain's flow map).
    #[serde(default = "default_wet_gullies")]
    pub wet_gullies: f32,
}

fn default_wet_gullies() -> f32 {
    1.0
}

impl Default for StyleRules {
    fn default() -> Self {
        Self {
            snow_line: 260.0,
            snow_blend: 40.0,
            rock_slope: 0.42,
            beach_height: 2.5,
            dryness: 0.35,
            wetness: 0.0,
            wet_gullies: default_wet_gullies(),
        }
    }
}

#[derive(Resource)]
pub struct Terrain {
    /// Cells per side; vertices per side is `res + 1`.
    pub res: usize,
    /// Meters per cell.
    pub cell: f32,
    pub heights: Vec<f32>,
    /// Hand-painted layer weights per vertex. The sum (≤255) is how much paint overrides the
    /// automatic style rules.
    pub splat: Vec<[u8; LAYER_COUNT]>,
    /// Zone id per vertex (0 = no zone).
    pub zones: Vec<u8>,
    /// Biome per vertex (0 = none, else `BiomePreset::id`): drives ground, grass, plants,
    /// ambience and weather wherever no zone overrides them.
    pub biomes: Vec<u8>,
    /// Where rain gathers, per vertex (0..255, `erosion::flow_map`): derived from the heights in
    /// the background after edits (`bigworld`), never saved. The ground style shows it as damp
    /// soil and stream gravel (`style.wet_gullies`).
    pub flow: Vec<u8>,
    pub sea_level: f32,
    pub style: StyleRules,
    pub dirty_height: Option<GridRect>,
    pub dirty_splat: Option<GridRect>,
    pub dirty_zone: Option<GridRect>,
    /// Region whose grass density must be recomputed (heights or paint changed).
    pub dirty_grass: Option<GridRect>,
    /// Region whose terrain meshes must be rebuilt.
    pub dirty_mesh: Option<GridRect>,
    /// Region where the flow map (or `style.wet_gullies`) changed: refresh it on the GPU.
    pub dirty_flow: Option<GridRect>,
    /// Bumped when the whole terrain is replaced (new world / load); GPU resources are rebuilt.
    pub generation: u64,
    /// Bumped on any height change (lakes, objects, etc. can react).
    pub height_revision: u64,
    /// Bumped on any zone-map change.
    pub zone_revision: u64,
    /// Bumped on any biome-map change (procedural nature follows the biomes).
    pub biome_revision: u64,
    /// The zone overlay shows the biome map instead (biome brush active). View state, not saved.
    pub show_biomes: bool,
}

impl Terrain {
    pub fn new(res: usize, cell: f32, sea_level: f32) -> Self {
        let n = res + 1;
        Self {
            res,
            cell,
            heights: vec![-20.0; n * n],
            splat: vec![[0; LAYER_COUNT]; n * n],
            zones: vec![0; n * n],
            biomes: vec![0; n * n],
            flow: vec![0; n * n],
            sea_level,
            style: StyleRules::default(),
            dirty_height: None,
            dirty_splat: None,
            dirty_zone: None,
            dirty_grass: Some(GridRect {
                x0: 0,
                z0: 0,
                x1: n - 1,
                z1: n - 1,
            }),
            dirty_mesh: None,
            dirty_flow: None,
            generation: 0,
            height_revision: 0,
            zone_revision: 0,
            biome_revision: 0,
            show_biomes: false,
        }
    }

    #[inline]
    pub fn n(&self) -> usize {
        self.res + 1
    }
    #[inline]
    pub fn size(&self) -> f32 {
        self.res as f32 * self.cell
    }
    #[inline]
    pub fn half(&self) -> f32 {
        self.size() * 0.5
    }
    #[inline]
    pub fn idx(&self, i: usize, j: usize) -> usize {
        j * (self.res + 1) + i
    }

    #[inline]
    pub fn world_to_grid(&self, x: f32, z: f32) -> Vec2 {
        Vec2::new((x + self.half()) / self.cell, (z + self.half()) / self.cell)
    }
    #[inline]
    pub fn grid_to_world(&self, i: usize, j: usize) -> Vec2 {
        Vec2::new(
            i as f32 * self.cell - self.half(),
            j as f32 * self.cell - self.half(),
        )
    }
    pub fn in_bounds(&self, x: f32, z: f32) -> bool {
        let h = self.half();
        x >= -h && x <= h && z >= -h && z <= h
    }

    #[inline]
    pub fn h(&self, i: usize, j: usize) -> f32 {
        self.heights[j * (self.res + 1) + i]
    }

    /// Bilinear height at a world position (clamped to the terrain edge).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let g = self.world_to_grid(x, z);
        let max = self.res as f32;
        let gx = g.x.clamp(0.0, max);
        let gz = g.y.clamp(0.0, max);
        let i0 = (gx.floor() as usize).min(self.res - 1);
        let j0 = (gz.floor() as usize).min(self.res - 1);
        let fx = gx - i0 as f32;
        let fz = gz - j0 as f32;
        let h00 = self.h(i0, j0);
        let h10 = self.h(i0 + 1, j0);
        let h01 = self.h(i0, j0 + 1);
        let h11 = self.h(i0 + 1, j0 + 1);
        let a = h00 + (h10 - h00) * fx;
        let b = h01 + (h11 - h01) * fx;
        a + (b - a) * fz
    }

    /// Lowest ground under a disc of radius `r` (centre and eight points round its rim): a trunk
    /// standing at `x, z` must reach down to it, or a gap opens on the downhill side.
    pub fn min_height_around(&self, x: f32, z: f32, r: f32) -> f32 {
        let mut lo = self.height_at(x, z);
        for k in 0..8 {
            let q = Vec2::new(x, z) + Vec2::from_angle(k as f32 * std::f32::consts::FRAC_PI_4) * r;
            lo = lo.min(self.height_at(q.x, q.y));
        }
        lo
    }

    /// How much of the flow map shows as damp ground at a vertex (0..1, × `style.wet_gullies`):
    /// none in dry biomes (deserts, volcanic wastes: their washes stay dry sand and ash, never
    /// mossy), blended across biome borders.
    pub fn gully_wet(&self, i: usize, j: usize) -> f32 {
        let damp =
            |id: u8| crate::biomes::BiomePreset::from_id(id).is_none_or(|b| b.damp_gullies());
        let k: f32 = crate::visuals::ecotone::biome_weights(self, self.grid_to_world(i, j), 0)
            .iter()
            .filter(|(id, _)| damp(*id))
            .map(|(_, w)| w)
            .sum();
        self.style.wet_gullies * k
    }

    /// Smooth normal at a grid vertex.
    pub fn normal_at_grid(&self, i: usize, j: usize) -> Vec3 {
        let r = self.res;
        let hl = self.h(i.saturating_sub(1), j);
        let hr = self.h((i + 1).min(r), j);
        let hd = self.h(i, j.saturating_sub(1));
        let hu = self.h(i, (j + 1).min(r));
        let dx = (hr - hl) / (self.cell * (((i + 1).min(r) - i.saturating_sub(1)) as f32));
        let dz = (hu - hd) / (self.cell * (((j + 1).min(r) - j.saturating_sub(1)) as f32));
        Vec3::new(-dx, 1.0, -dz).normalize()
    }

    pub fn normal_at(&self, x: f32, z: f32) -> Vec3 {
        let e = self.cell;
        let dx = (self.height_at(x + e, z) - self.height_at(x - e, z)) / (2.0 * e);
        let dz = (self.height_at(x, z + e) - self.height_at(x, z - e)) / (2.0 * e);
        Vec3::new(-dx, 1.0, -dz).normalize()
    }

    /// Grid rect covering a world-space circle.
    pub fn rect_for_circle(&self, center: Vec2, radius: f32) -> Option<GridRect> {
        let g0 = self.world_to_grid(center.x - radius, center.y - radius);
        let g1 = self.world_to_grid(center.x + radius, center.y + radius);
        let max = self.res as f32;
        if g1.x < 0.0 || g1.y < 0.0 || g0.x > max || g0.y > max {
            return None;
        }
        Some(GridRect {
            x0: g0.x.floor().clamp(0.0, max) as usize,
            z0: g0.y.floor().clamp(0.0, max) as usize,
            x1: g1.x.ceil().clamp(0.0, max) as usize,
            z1: g1.y.ceil().clamp(0.0, max) as usize,
        })
    }

    pub fn mark_heights_dirty(&mut self, rect: GridRect) {
        self.dirty_height = union_opt(self.dirty_height, rect);
        self.dirty_grass = union_opt(self.dirty_grass, rect.expand(1, self.n()));
        self.height_revision += 1;
        // normals at patch borders depend on neighbours: expand by one vertex.
        self.dirty_mesh = union_opt(self.dirty_mesh, rect.expand(1, self.n()));
    }
    pub fn mark_splat_dirty(&mut self, rect: GridRect) {
        self.dirty_splat = union_opt(self.dirty_splat, rect);
        self.dirty_grass = union_opt(self.dirty_grass, rect);
    }
    pub fn mark_zone_dirty(&mut self, rect: GridRect) {
        self.dirty_zone = union_opt(self.dirty_zone, rect);
        self.zone_revision += 1;
    }
    /// The biome map changed: grass restyles (its styles blend over ±3 vertices), and damp gullies
    /// follow (dry biomes have none; they blend over the ecotone band).
    pub fn mark_biome_dirty(&mut self, rect: GridRect) {
        self.dirty_grass = union_opt(self.dirty_grass, rect.expand(3, self.n()));
        let band = (60.0 / self.cell).ceil() as usize;
        self.dirty_flow = union_opt(self.dirty_flow, rect.expand(band, self.n()));
        self.biome_revision += 1;
        if self.show_biomes {
            self.dirty_zone = union_opt(self.dirty_zone, rect);
        }
    }
    pub fn mark_all_dirty(&mut self) {
        let n = self.n();
        let all = GridRect {
            x0: 0,
            z0: 0,
            x1: n - 1,
            z1: n - 1,
        };
        self.dirty_height = Some(all);
        self.dirty_splat = Some(all);
        self.dirty_zone = Some(all);
        self.dirty_grass = Some(all);
        self.dirty_mesh = Some(all);
        self.dirty_flow = Some(all);
        self.height_revision += 1;
        self.zone_revision += 1;
        self.biome_revision += 1;
    }

    /// March a ray against the heightfield. Returns the hit point.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<Vec3> {
        let dir = dir.normalize();
        let half = self.half();
        // Clip the ray to the terrain's horizontal bounds (with a vertical slab that always contains it).
        let mut t0 = 0.0f32;
        let mut t1 = max_dist;
        for (o, d) in [(origin.x, dir.x), (origin.z, dir.z)] {
            if d.abs() < 1e-6 {
                if o < -half || o > half {
                    return None;
                }
            } else {
                let a = (-half - o) / d;
                let b = (half - o) / d;
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
            }
        }
        if t0 > t1 {
            return None;
        }
        let step = self.cell * 0.5;
        let mut t = t0;
        let mut prev_t = t;
        let above = |t: f32| {
            let p = origin + dir * t;
            p.y - self.height_at(p.x, p.z)
        };
        let mut prev = above(t);
        if prev < 0.0 {
            return Some(origin + dir * t);
        }
        // adaptive step: big steps when far above terrain
        while t < t1 {
            let s = (prev * 0.5).max(step);
            t = (t + s).min(t1);
            let cur = above(t);
            if cur < 0.0 {
                // bisection refine
                let (mut a, mut b) = (prev_t, t);
                for _ in 0..20 {
                    let m = 0.5 * (a + b);
                    if above(m) < 0.0 {
                        b = m;
                    } else {
                        a = m;
                    }
                }
                return Some(origin + dir * b);
            }
            prev = cur;
            prev_t = t;
            if t >= t1 {
                break;
            }
        }
        None
    }

    /// Automatic (rule based) layer weights at a vertex — the CPU mirror of the shader rules,
    /// without the fine per-pixel noise.
    pub fn auto_weights(&self, i: usize, j: usize) -> [f32; LAYER_COUNT] {
        let h = self.h(i, j);
        let n = self.normal_at_grid(i, j);
        let p = self.grid_to_world(i, j);
        let macro_n = crate::noise::value2(p.x * 0.004, p.y * 0.004, 17) - 0.5;
        let mid_n = crate::noise::value2(p.x * 0.05, p.y * 0.05, 29) - 0.5;
        let dry_n = dry_noise(p.x, p.y);
        auto_weights(&self.style, self.sea_level, h, n.y, macro_n, mid_n, dry_n)
    }

    /// Final blended weights (auto rules overridden by paint).
    pub fn final_weights(&self, i: usize, j: usize) -> [f32; LAYER_COUNT] {
        let mut auto = self.auto_weights(i, j);
        let paint = self.splat[self.idx(i, j)];
        let mut s = 0.0;
        for v in paint {
            s += v as f32 / 255.0;
        }
        let s = s.min(1.0);
        // painted ground wins over the automatic snow line (as the shader does)
        auto[Layer::Snow as usize] *= 1.0 - smoothstep(0.0, 0.6, s);
        let mut out = [0.0; LAYER_COUNT];
        for k in 0..LAYER_COUNT {
            out[k] = auto[k] * (1.0 - s) + paint[k] as f32 / 255.0;
        }
        // where rain gathers, painted or not (biomes paint most of the land)
        if let Some(f) = self.flow.get(self.idx(i, j)) {
            let p = self.grid_to_world(i, j);
            let mid_n = crate::noise::value2(p.x * 0.05, p.y * 0.05, 29) - 0.5;
            flow_weights(
                &mut out,
                *f as f32 / 255.0 * self.gully_wet(i, j),
                self.normal_at_grid(i, j).y,
                mid_n,
            );
        }
        out
    }

    pub fn dominant_layer_at(&self, x: f32, z: f32) -> Layer {
        let g = self.world_to_grid(x, z);
        let i = (g.x.round().max(0.0) as usize).min(self.res);
        let j = (g.y.round().max(0.0) as usize).min(self.res);
        let w = self.final_weights(i, j);
        let mut best = 0;
        for k in 1..LAYER_COUNT {
            if w[k] > w[best] {
                best = k;
            }
        }
        Layer::from_index(best)
    }

    pub fn biome_at(&self, x: f32, z: f32) -> u8 {
        if !self.in_bounds(x, z) {
            return 0;
        }
        let g = self.world_to_grid(x, z);
        let i = (g.x.round() as usize).min(self.res);
        let j = (g.y.round() as usize).min(self.res);
        self.biomes[self.idx(i, j)]
    }
    pub fn zone_at(&self, x: f32, z: f32) -> u8 {
        if !self.in_bounds(x, z) {
            return 0;
        }
        let g = self.world_to_grid(x, z);
        let i = (g.x.round() as usize).min(self.res);
        let j = (g.y.round() as usize).min(self.res);
        self.zones[self.idx(i, j)]
    }
}

/// Irregular dry-patch noise in [-0.5, 0.5] (three octaves of value noise). Mirrored in the shader.
pub fn dry_noise(x: f32, z: f32) -> f32 {
    let v = crate::noise::value2;
    v(x * 0.0031, z * 0.0031, 41) * 0.55
        + v(x * 0.0113, z * 0.0113, 43) * 0.3
        + v(x * 0.041, z * 0.041, 47) * 0.15
        - 0.5
}

/// Shared auto-texturing rule function. `ny` is the up component of the normal.
/// `macro_n` / `mid_n` / `dry_n` are noise values in [-0.5, 0.5].
pub fn auto_weights(
    style: &StyleRules,
    sea: f32,
    h: f32,
    ny: f32,
    macro_n: f32,
    mid_n: f32,
    dry_n: f32,
) -> [f32; LAYER_COUNT] {
    let slope = 1.0 - ny;
    let mut w = [0.0f32; LAYER_COUNT];
    let alt = h - sea;
    // meadow grass with dry patches (drier higher up)
    let dry =
        smoothstep(0.2, 0.55, dry_n * 1.6 + style.dryness - 0.25 + alt / 900.0).clamp(0.0, 1.0);
    let mut grass = 1.0 - dry;
    let mut dry_grass = dry;
    // dirt on moderate slopes
    let dirt = smoothstep(
        style.rock_slope * 0.45,
        style.rock_slope * 0.85,
        slope + mid_n * 0.06,
    );
    // rock on steep slopes
    let rock = smoothstep(
        style.rock_slope * 0.8,
        style.rock_slope * 1.15,
        slope + mid_n * 0.05,
    );
    // beaches and sea floor
    let beach = 1.0
        - smoothstep(
            style.beach_height - 1.0,
            style.beach_height + 1.5,
            alt + mid_n * 2.5,
        );
    let under = 1.0 - smoothstep(-6.0, -1.5, alt);
    // snow above the snow line, thinning on cliffs
    let snow = smoothstep(
        style.snow_line - style.snow_blend,
        style.snow_line + style.snow_blend,
        alt + macro_n * 60.0 + mid_n * 25.0,
    ) * (1.0 - smoothstep(0.62, 0.85, slope));

    let mut dirt_w = dirt * (1.0 - rock);
    grass *= 1.0 - dirt;
    dry_grass *= 1.0 - dirt;
    let rock_w = rock;
    grass *= 1.0 - rock;
    dry_grass *= 1.0 - rock;
    // beach replaces everything but steep rock
    let sand = beach * (1.0 - rock * 0.7);
    grass *= 1.0 - sand;
    dry_grass *= 1.0 - sand;
    dirt_w *= 1.0 - sand;
    let mut rock_w = rock_w * (1.0 - sand);
    // seabed: sand with gravel patches
    let gravel = under * smoothstep(0.0, 0.3, mid_n + 0.1) * 0.6;
    let sand = sand.max(under) * (1.0 - gravel);

    let keep = 1.0 - snow;
    grass *= keep;
    dry_grass *= keep;
    dirt_w *= keep;
    rock_w *= keep;
    let sand = sand * keep;

    w[Layer::Grass as usize] = grass;
    w[Layer::DryGrass as usize] = dry_grass;
    w[Layer::Dirt as usize] = dirt_w;
    w[Layer::Rock as usize] = rock_w;
    w[Layer::Sand as usize] = sand;
    w[Layer::Gravel as usize] = gravel * keep;
    w[Layer::Snow as usize] = snow;
    // normalise
    let s: f32 = w.iter().sum();
    if s > 1e-4 {
        for v in &mut w {
            *v /= s;
        }
    } else {
        w[0] = 1.0;
    }
    w
}

/// Where rain gathers (`wet`: flow map × style strength), dry grass greens up along swales, the
/// banks of the wettest lines turn to dark damp soil, and only the narrow, gentle bed of a real
/// stream gets gravel. Applied to the final (painted) weights; mirrored by `flow_weights` in the
/// shader (which also darkens damp ground and greens it with moss).
pub fn flow_weights(w: &mut [f32; LAYER_COUNT], wet: f32, ny: f32, mid_n: f32) {
    if wet <= 0.01 {
        return;
    }
    let (g, d, f) = (
        Layer::Grass as usize,
        Layer::DryGrass as usize,
        Layer::ForestFloor as usize,
    );
    let green = smoothstep(0.25, 0.5, wet) * 0.85;
    let moved = w[d] * green;
    w[d] -= moved;
    w[g] += moved;
    let bed = smoothstep(0.86, 0.97, wet + mid_n * 0.08) * smoothstep(0.88, 0.97, ny);
    // (damp soil on the gentler banks: steep ones only darken, see the shader)
    let bank = smoothstep(0.5, 0.8, wet + mid_n * 0.15) * (1.0 - bed) * smoothstep(0.78, 0.9, ny);
    let soil = bank * 0.45;
    let stones = bed * 0.75;
    let veg = w[g] + w[d] + w[f];
    let k = 1.0 - soil - stones;
    w[g] *= k;
    w[d] *= k;
    w[f] *= k;
    w[Layer::Mud as usize] += veg * soil;
    w[Layer::Gravel as usize] += veg * stones;
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerrainSystems {
    /// Upload dirty texture regions / (re)create GPU resources.
    Gpu,
    /// Rebuild dirty / LOD-changed chunk meshes.
    Chunks,
}

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PostUpdate,
            (TerrainSystems::Gpu, TerrainSystems::Chunks)
                .chain()
                .before(bevy::camera::visibility::VisibilitySystems::CalculateBounds),
        )
        .add_plugins((
            material::TerrainMaterialPlugin,
            gpu::TerrainGpuPlugin,
            mesh::TerrainMeshPlugin,
            textures::TerrainTexturesPlugin,
            ab::TerrainAbPlugin,
            bake::TerrainBakePlugin,
        ))
        .init_resource::<overlay::TerrainOverlay>()
        .add_systems(Startup, overlay::init_placeholder)
        .add_systems(
            PostUpdate,
            overlay::apply_overlay.after(TerrainSystems::Gpu),
        );
    }
}
