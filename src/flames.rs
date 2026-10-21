//! Fire. AetherFX's flame effects (MIT, github.com/holokat/AetherFX) — its wall torch, brazier,
//! campfire and lantern candle — baked to seamless looping flipbooks by
//! `tools/flames/bake_flames.py` (`assets/textures/flames/`), played on a sheet facing the eye
//! (`shaders/flame_flipbook.wesl`). One sheet per fire: real simulated fire for the cost of a
//! quad. The light a fire throws is its own light source (a dungeon light, a campfire's light):
//! the sheet is only what the eye sees burning.
//!
//! The library models' `fire` anchors (braziers, torch stands, fire bowls and pits, candles; the
//! style packs' `fx_fire_<n>` nodes, `crate::fx_anchors`) burn here too: the nearest get a flame
//! sized by the anchor (a candle, a torch, a brazier, a bed of fire) and the nearest few a
//! flickering light. Each fire is one flame: a wide one (a pit, a bowl, a campfire) burns as wide
//! as its bed at the foot, narrowing as it rises (the sheet spread at its foot: `spread`). So do the older models' painted flames (the hand-painted
//! pack's `flame` material: glowing cones on torches, braziers, kilns, beacons): the cones are
//! taken out of the scenes and real fire burns where each one stood, as tall and as wide.

use std::collections::HashMap;
use std::sync::LazyLock;

use bevy::asset::RenderAssetUsages;
use bevy::gltf::GltfMaterialName;
use bevy::image::{ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use bevy::world_serialization::WorldInstanceReady;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg, position_arg, position_schema};

/// The baked fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FireKind {
    /// a wall torch's tall narrow flame
    Torch,
    /// a wall brazier's fuller, taller flame
    Brazier,
    /// a bed of fire: a campfire, a brazier bowl
    Campfire,
    /// a candle's or lantern's small flame
    Candle,
}

impl FireKind {
    pub const ALL: [FireKind; 4] = [
        FireKind::Torch,
        FireKind::Brazier,
        FireKind::Campfire,
        FireKind::Candle,
    ];
    /// How wide the baked flame is low down, as a share of its frame's width (measured on the
    /// sheets: the flames are round-bodied columns, not wide at the foot).
    pub fn body(self) -> f32 {
        match self {
            FireKind::Torch => 0.39,
            FireKind::Brazier => 0.36,
            FireKind::Campfire => 0.53,
            FireKind::Candle => 0.58,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            FireKind::Torch => "torch",
            FireKind::Brazier => "brazier",
            FireKind::Campfire => "campfire",
            FireKind::Candle => "candle",
        }
    }
    pub fn sheet(self) -> &'static Sheet {
        &MANIFEST.fires[self.id()]
    }
}

/// A baked flipbook (`flames.json`): its grid and pace, the frame's size in metres and where the
/// flame's foot sits in it (from the bottom, 0..1).
#[derive(Clone, Debug, Deserialize)]
pub struct Sheet {
    pub file: String,
    pub frames: u32,
    pub columns: u32,
    pub rows: u32,
    pub fps: f32,
    pub width_m: f32,
    pub height_m: f32,
    pub foot_v: f32,
}

#[derive(Deserialize)]
struct Manifest {
    fires: std::collections::HashMap<String, Sheet>,
}

static MANIFEST: LazyLock<Manifest> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/textures/flames/flames.json"))
        .expect("flames.json")
});

/// A fire's sheet on disk (the export copies it).
pub fn sheet_path(kind: FireKind) -> std::path::PathBuf {
    std::path::PathBuf::from(crate::assets_path())
        .join("textures/flames")
        .join(&kind.sheet().file)
}

/// How much taller the frame is than the flame in it (the bake leaves room above and below).
const FRAME_OVER_FLAME: f32 = 1.2;

impl Sheet {
    /// The sheet for a flame `height` m tall: its width, height and how far its bottom sits below
    /// the flame's foot.
    pub fn frame(&self, height: f32) -> (f32, f32, f32) {
        let h = height * FRAME_OVER_FLAME;
        (h * self.width_m / self.height_m, h, h * self.foot_v)
    }
}

/// How far a flame may be spread at its foot (more smears the sheet).
pub const MAX_SPREAD: f32 = 3.0;

/// How many times wider than the sheet's own a fire's flame is at its foot, so it fills its bed
/// (`radius`, most of it): 1 for torches and candles, more for pits, bowls and campfires. The
/// sheet narrows back to its own width as the flame rises (`shaders/flame_flipbook.wesl`).
pub fn spread(kind: FireKind, height: f32, radius: f32) -> f32 {
    if kind == FireKind::Candle || kind == FireKind::Torch {
        return 1.0;
    }
    let own = kind.sheet().frame(height).0 * kind.body();
    (radius * 2.0 * 0.9 / own.max(1e-3)).clamp(1.0, MAX_SPREAD)
}

pub type FlipbookMaterial = ExtendedMaterial<StandardMaterial, FlipbookExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct FlipbookParams {
    /// columns, rows, frames, frames per second
    pub grid: Vec4,
    /// the light's colour (linear), w: how much the fire takes it (0: its own colours)
    pub tint: Vec4,
    /// strength, how much it hides behind it, soft edge against walls (m), spare
    pub look: Vec4,
    /// `volumetrics::FroxelFog::params` (w 0: no volumetric light)
    pub fog: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct FlipbookExt {
    #[uniform(100)]
    pub params: FlipbookParams,
    #[texture(101)]
    #[sampler(102)]
    pub sheet: Handle<Image>,
    /// The volumetric light's integrated volume (`volumetrics::FroxelFog`): a fire far off in
    /// the fog glows through it dimmed, like everything else there.
    #[texture(103, dimension = "3d")]
    #[sampler(104)]
    pub fog_volume: Option<Handle<Image>>,
}

impl MaterialExtension for FlipbookExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/flame_flipbook.wesl".into()
    }
}

pub struct FlamesPlugin;

impl Plugin for FlamesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<FlipbookMaterial>::default())
            .init_resource::<AnchorFires>()
            .init_resource::<PaintedFlames>()
            .add_observer(on_scene_ready)
            .add_systems(
                Update,
                (find_painted, anchor_fires, face_fires)
                    .chain()
                    .after(crate::sky::SkySystems),
            )
            .add_systems(PostUpdate, fog_fires);
        app.mcp_tool(McpTool {
            name: "fires",
            description: "Fire in the world: the placed objects' fires (their models' `fire` anchors — braziers, torch stands, fire bowls and pits, candles — and every campfire), each burning as AetherFX's simulated flames baked to a looping flipbook (a candle, a torch, a brazier or a bed of fire, chosen by the anchor's size; a pit, a bowl or a campfire burns as one flame as wide as its bed at the foot, narrowing as it rises: `spread`), the nearest also throwing a flickering light. Lists the fires near `position` ([x, z]) within `radius` (m, default 120), nearest first (at most `limit`, default 50): object id, anchor, fire, foot, height, width, whether it has a light of its own and whether it is burning now (near enough to the eye). Also the baked fires (`kinds`). Dungeon fires are their lights (`dungeons_lights`). Read-only.",
            input_schema: || {
                json!({ "type": "object", "properties": {
                    "position": position_schema("centre of the search"),
                    "radius": { "type": "number", "description": "metres (default 120)" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 } } })
            },
            run: fires_tool,
        });
    }
}

fn fires_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let centre = position_arg(&args, "position")?.map(|(p, _)| p);
    let radius = opt_arg::<f32>(&args, "radius")?.unwrap_or(120.0).max(0.1);
    let limit = opt_arg::<usize>(&args, "limit")?
        .unwrap_or(50)
        .clamp(1, 500);
    let af = world.resource::<AnchorFires>();
    let r2 = |v: f32| (v as f64 * 100.0).round() / 100.0;
    let mut rows: Vec<(f32, Value)> = af
        .fires
        .iter()
        .filter_map(|f| {
            let d = centre
                .map(|c| c.distance(Vec2::new(f.foot.x, f.foot.z)))
                .unwrap_or(0.0);
            if centre.is_some() && d > radius {
                return None;
            }
            let burning = af.flames.contains_key(&f.key);
            Some((
                d,
                json!({
                    "object": f.key.0,
                    "anchor": if f.key.1 == u16::MAX { Value::Null } else { json!(f.key.1) },
                    "fire": f.kind.id(),
                    "foot": [r2(f.foot.x), r2(f.foot.y), r2(f.foot.z)],
                    "height": r2(f.height),
                    "width": r2(f.radius * 2.0),
                    "spread": r2(f.spread()),
                    "light": f.light,
                    "burning": burning,
                    "lit": af.lights.contains_key(&f.key),
                }),
            ))
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total = rows.len();
    let kinds: Vec<Value> = FireKind::ALL
        .iter()
        .map(|k| {
            let s = k.sheet();
            json!({ "fire": k.id(), "frames": s.frames, "fps": s.fps, "frame_m": [s.width_m, s.height_m] })
        })
        .collect();
    Ok(vec![McpContent::Json(
        json!({ "fires": rows.into_iter().take(limit).map(|r| r.1).collect::<Vec<_>>(), "fires_total": total, "burning_now": af.flames.len(), "lights_now": af.lights.len(), "kinds": kinds }),
    )])
}

/// How far a light's colour is from ordinary firelight (0 ordinary … 1 a magic colour).
pub fn magic(color: [f32; 3]) -> f32 {
    let c = Vec3::from_array(color).normalize_or(Vec3::ONE);
    let fire = Vec3::new(1.0, 0.62, 0.3).normalize();
    ((1.0 - c.dot(fire)) * 9.0).clamp(0.0, 1.0)
}

/// A fire's material: `kind`'s flipbook burning in `color` (sRGB; ordinary firelight keeps the
/// simulation's own colours) at `strength`.
pub fn material(
    kind: FireKind,
    color: [f32; 3],
    strength: f32,
    server: &AssetServer,
) -> FlipbookMaterial {
    let s = kind.sheet();
    let sheet = server
        .load_builder()
        .with_settings(|o: &mut ImageLoaderSettings| {
            o.is_srgb = true;
            o.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
        })
        .load(format!("textures/flames/{}", s.file));
    let c = Color::srgb_from_array(color).to_linear();
    let tint = Vec3::new(c.red, c.green, c.blue);
    let tint = tint / tint.max_element().max(1e-3);
    ExtendedMaterial {
        base: StandardMaterial {
            alpha_mode: AlphaMode::Premultiplied,
            unlit: true,
            cull_mode: None,
            ..default()
        },
        extension: FlipbookExt {
            params: FlipbookParams {
                grid: Vec4::new(s.columns as f32, s.rows as f32, s.frames as f32, s.fps),
                tint: tint.extend(magic(color)),
                look: Vec4::new(
                    strength,
                    0.35,
                    if kind == FireKind::Candle { 0.03 } else { 0.18 },
                    0.0,
                ),
                fog: Vec4::ZERO,
            },
            sheet,
            fog_volume: None,
        },
    }
}

/// A sheet one metre square standing on its foot (x −0.5..0.5, y 0..1), facing +z; `seed`
/// (0..1) in the red of its colour so neighbouring fires don't burn alike, its foot's `spread`
/// (/ 4) in the green.
pub fn quad(seed: f32, spread: f32) -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.5, 0.0, 0.0],
            [0.5, 0.0, 0.0],
            [0.5, 1.0, 0.0],
            [-0.5, 1.0, 0.0f32],
        ],
    );
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0f32]; 4]);
    m.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0f32]],
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[seed, spread.clamp(1.0, 4.0) / 4.0, 0.0, 1.0f32]; 4],
    );
    m.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    m
}

/// Where a fire's sheet stands to face the eye from `eye`: its flame's foot at `foot`, `height`
/// m tall, `spread` times its own width (its mesh's `spread`: the shader narrows it back as it
/// rises), turning about the upright only, so it never tips over.
pub fn facing(kind: FireKind, foot: Vec3, height: f32, spread: f32, eye: Vec3) -> Transform {
    let (w, h, below) = kind.sheet().frame(height);
    let to = eye - foot;
    Transform::from_translation(foot - Vec3::Y * below)
        .with_rotation(Quat::from_rotation_y(to.x.atan2(to.z)))
        .with_scale(Vec3::new(w * spread, h, 1.0))
}

/// The bucket a spread's sheets share a mesh in (tenths).
pub fn spread_key(spread: f32) -> u8 {
    (spread.clamp(1.0, 4.0) * 10.0).round() as u8
}

// ------------------------------------------------------------------ the models' painted flames

/// The material the hand-painted pack paints its flame cones with.
pub const PAINTED_FLAME: &str = "flame";

/// A model's scene is in: its painted flame cones go (real fire burns there: `anchor_fires`).
/// Taken out, not hidden: the distance levels (`model_render::lod`) copy the meshes they find.
fn on_scene_ready(
    ev: On<WorldInstanceReady>,
    children: Query<&Children>,
    names: Query<&GltfMaterialName>,
    mut commands: Commands,
) {
    for c in children.iter_descendants(ev.event().entity) {
        if names.get(c).is_ok_and(|n| n.0 == PAINTED_FLAME) {
            commands.entity(c).despawn();
        }
    }
}

/// Where the library models' painted flames were (model space: foot, radius, height), found
/// once in the background when the library is in.
#[derive(Resource, Default)]
struct PaintedFlames {
    by_model: HashMap<String, Vec<(Vec3, f32, f32)>>,
    task: Option<Task<HashMap<String, Vec<(Vec3, f32, f32)>>>>,
    started: bool,
    revision: u64,
}

fn find_painted(mut pf: ResMut<PaintedFlames>, lib: Option<Res<crate::objects::Library>>) {
    if !pf.started {
        let Some(lib) = lib else { return };
        let files: Vec<(String, std::path::PathBuf)> = lib
            .entries
            .iter()
            .filter_map(|e| {
                Some((
                    e.id.clone(),
                    crate::asset_file(e.import_path.as_ref()?),
                ))
            })
            .collect();
        if files.is_empty() {
            return;
        }
        pf.started = true;
        pf.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            files
                .into_iter()
                .filter_map(|(id, path)| {
                    painted_flames(&path)
                        .filter(|v| !v.is_empty())
                        .map(|v| (id, v))
                })
                .collect()
        }));
    }
    if let Some(t) = pf.task.as_mut()
        && let Some(found) = block_on(future::poll_once(t))
    {
        pf.task = None;
        pf.by_model = found;
        pf.revision += 1;
    }
}

/// A model file's painted flames: each connected piece of its `flame` geometry (cones touching
/// or overlapping count as one fire) as (foot, radius, height) in model space.
pub fn painted_flames(path: &std::path::Path) -> Option<Vec<(Vec3, f32, f32)>> {
    let bytes = std::fs::read(path).ok()?;
    let glb = if bytes.starts_with(b"glTF") {
        bytes
    } else {
        // a .gltf: only read on when it paints a flame; its buffer from beside it, made a GLB
        let mut doc: Value = serde_json::from_slice(&bytes).ok()?;
        if !doc["materials"]
            .as_array()
            .is_some_and(|m| m.iter().any(|m| m["name"] == PAINTED_FLAME))
        {
            return Some(vec![]);
        }
        let uri = doc["buffers"][0]["uri"].as_str()?.to_string();
        let bin = std::fs::read(path.parent()?.join(uri)).ok()?;
        doc["buffers"] = json!([{ "byteLength": bin.len() }]);
        glb_of(&doc, &bin)
    };
    let doc = crate::export::glb::glb_json(&glb)?;
    let flame: Vec<usize> = doc["materials"]
        .as_array()?
        .iter()
        .enumerate()
        .filter(|(_, m)| m["name"] == PAINTED_FLAME)
        .map(|(i, _)| i)
        .collect();
    if flame.is_empty() {
        return Some(vec![]);
    }
    let parts = crate::export::glb::flat_parts(&glb, None)?;
    let mut boxes: Vec<(Vec3, Vec3)> = vec![];
    for part in parts
        .iter()
        .filter(|p| p.material.is_some_and(|m| flame.contains(&m)))
    {
        boxes.extend(pieces(&part.pos, &part.idx));
    }
    // cones that touch or overlap are one fire
    loop {
        let mut merged = false;
        'outer: for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                if (a.0 - Vec3::splat(0.04)).cmple(b.1).all()
                    && (b.0 - Vec3::splat(0.04)).cmple(a.1).all()
                {
                    boxes[i] = (a.0.min(b.0), a.1.max(b.1));
                    boxes.swap_remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            break;
        }
    }
    Some(
        boxes
            .into_iter()
            .map(|(lo, hi)| {
                (
                    Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5),
                    ((hi.x - lo.x).max(hi.z - lo.z) * 0.5).max(0.01),
                    (hi.y - lo.y).max(0.02),
                )
            })
            .collect(),
    )
}

/// The bounds of each connected piece of a mesh (vertices welded at a millimetre).
fn pieces(pos: &[[f32; 3]], idx: &[u32]) -> Vec<(Vec3, Vec3)> {
    let mut weld: HashMap<[i32; 3], usize> = HashMap::new();
    let ids: Vec<usize> = pos
        .iter()
        .map(|p| {
            let k = [
                (p[0] * 1000.0).round() as i32,
                (p[1] * 1000.0).round() as i32,
                (p[2] * 1000.0).round() as i32,
            ];
            let n = weld.len();
            *weld.entry(k).or_insert(n)
        })
        .collect();
    let mut parent: Vec<usize> = (0..weld.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for t in idx.chunks_exact(3) {
        let a = root(&mut parent, ids[t[0] as usize]);
        for &v in &t[1..] {
            let b = root(&mut parent, ids[v as usize]);
            if a != b {
                parent[b] = a;
            }
        }
    }
    let mut out: HashMap<usize, (Vec3, Vec3)> = HashMap::new();
    for (i, p) in pos.iter().enumerate() {
        let r = root(&mut parent, ids[i]);
        let v = Vec3::from_array(*p);
        let e = out.entry(r).or_insert((v, v));
        e.0 = e.0.min(v);
        e.1 = e.1.max(v);
    }
    out.into_values().collect()
}

/// A GLB from a glTF document and its one buffer.
fn glb_of(doc: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json = serde_json::to_vec(doc).unwrap_or_default();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let mut bin = bin.to_vec();
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

// ------------------------------------------------------------------ the models' fire anchors

/// How far (m) anchor fires burn round the eye, and how many at once; how many have a light.
const RANGE: f32 = 90.0;
const MAX_FLAMES: usize = 140;
const LIGHT_RANGE: f32 = 50.0;
const MAX_LIGHTS: usize = 12;

/// One fire of a placed object (or underground of a dungeon light): which object and anchor, the
/// fire, its foot, how tall and wide.
#[derive(Clone, Debug, PartialEq)]
pub struct Fire {
    pub key: (u64, u16),
    pub kind: FireKind,
    pub foot: Vec3,
    pub height: f32,
    pub radius: f32,
    /// throws its own light (a campfire's comes with its smoke: `crate::effects`; a dungeon
    /// fixture's is its dungeon light)
    pub light: bool,
    /// the colour it burns (sRGB; a dungeon light's, tinted by its mood: a green torch burns green)
    pub color: [f32; 3],
}

/// Ordinary firelight (sRGB).
pub const FIRE: [f32; 3] = [1.0, 0.62, 0.3];

/// Which fire an anchor is, by its size: a candle's (under 25 cm), a torch's (narrower than
/// 0.3 m), a bed of fire (a pit, a forge, a cauldron, a big bowl: wide for its height, or wider
/// than 0.75 m: the fuller sheet, spread less at its foot) or a brazier's.
pub fn kind_for(radius: f32, height: f32) -> FireKind {
    if height <= 0.25 {
        FireKind::Candle
    } else if radius < 0.29 {
        FireKind::Torch
    } else if height < radius * 2.2 || radius >= 0.38 {
        FireKind::Campfire
    } else {
        FireKind::Brazier
    }
}

impl Fire {
    /// How wide its one flame spreads at the foot (`spread`): a pit, a bowl or a campfire burns
    /// as one fire as wide as its bed, not as several thin flames side by side.
    pub fn spread(&self) -> f32 {
        spread(self.kind, self.height, self.radius)
    }
}

/// A fire's light: lumens and reach (the world's, scaled by night; underground, a dungeon's).
fn light_of(kind: FireKind, underground: bool) -> (f32, f32) {
    use crate::dungeons::lights::LightKind;
    if underground {
        let k = match kind {
            FireKind::Torch => LightKind::Torch,
            FireKind::Brazier | FireKind::Campfire => LightKind::Brazier,
            FireKind::Candle => LightKind::Candles,
        };
        let d = k.defaults();
        return (d.1, d.2);
    }
    match kind {
        FireKind::Torch => (25_000.0, 10.0),
        FireKind::Brazier => (45_000.0, 14.0),
        FireKind::Campfire => (60_000.0, 16.0),
        FireKind::Candle => (2_500.0, 4.0),
    }
}

/// A fire's sheet that turns to face the eye every frame (its flame's foot in the world).
#[derive(Component, Clone, Copy, Debug)]
pub struct FireSheet {
    pub kind: FireKind,
    pub foot: Vec3,
    pub height: f32,
    pub spread: f32,
}

/// A fire sheet for `kind` at `foot`, `height` m tall, `spread` at its foot: what to spawn (its
/// mesh's seed picks its own phase and pace; the mesh carries the same spread).
pub fn fire_bundle(
    kind: FireKind,
    foot: Vec3,
    height: f32,
    spread: f32,
    mesh: Handle<Mesh>,
    mat: Handle<FlipbookMaterial>,
) -> impl Bundle {
    (
        Name::new("Fire"),
        FireSheet {
            kind,
            foot,
            height,
            spread,
        },
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        facing(kind, foot, height, spread, foot + Vec3::Z),
        NotShadowCaster,
        NotShadowReceiver,
    )
}

/// Turns every fire sheet to the eye.
fn face_fires(
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut q: Query<(&FireSheet, &mut Transform)>,
) {
    let Ok(cam) = cam.single() else { return };
    let eye = cam.translation();
    for (f, mut tf) in &mut q {
        *tf = facing(f.kind, f.foot, f.height, f.spread, eye);
    }
}

#[derive(Component)]
struct AnchorLight {
    seed: u32,
    lumens: f32,
}

#[derive(Resource, Default)]
pub struct AnchorFires {
    seen: Option<(u64, u64)>,
    rescan: f32,
    fires: Vec<Fire>,
    flames: HashMap<(u64, u16), (Entity, Vec3, f32, [u8; 3], u8)>,
    lights: HashMap<(u64, u16), Entity>,
    /// the sheets' meshes by (seed, `spread_key`)
    meshes: HashMap<(u8, u8), Handle<Mesh>>,
    mats: HashMap<(FireKind, [u8; 3]), Handle<FlipbookMaterial>>,
    /// the objects that burn here (a dungeon light whose fixture is one draws no flame of its own)
    pub objects: std::collections::HashSet<u64>,
    eval: f32,
}

/// Every placed object's fires: its fire anchors or else its painted flames (`painted`: a library
/// model's, by id), and every campfire's bed of fire. Underground (`dungeon`), a dungeon light's
/// fixture (its torch sconce, its brazier) burns in the light's colour without a light of its own
/// (the dungeon light is it), as does any fire right at a dungeon light.
pub fn fires_for<'a>(
    objects: impl Iterator<Item = &'a crate::objects::Placed>,
    anchors: &crate::fx_anchors::FxAnchors,
    lib: Option<&crate::objects::Library>,
    painted: &mut dyn FnMut(&str) -> Vec<(Vec3, f32, f32)>,
    dungeon: Option<&crate::dungeons::model::MapDoc>,
) -> Vec<Fire> {
    let (lights, fixtures): (Vec<(Vec3, [f32; 3])>, HashMap<u64, [f32; 3]>) = match dungeon {
        Some(m) => (
            m.lights
                .iter()
                .map(|l| (l.position(), m.mood.tinted(l.color)))
                .collect(),
            m.lights
                .iter()
                .filter_map(|l| Some((l.fixture?, m.mood.tinted(l.color))))
                .collect(),
        ),
        None => (vec![], HashMap::new()),
    };
    // (no light of its own at a dungeon light; its colour)
    let at_light = |id: u64, w: Vec3| -> Option<[f32; 3]> {
        fixtures
            .get(&id)
            .copied()
            .or_else(|| lights.iter().find(|l| l.0.distance(w) < 1.2).map(|l| l.1))
    };
    let mut out = vec![];
    for p in objects {
        if let Some(e) = lib.and_then(|l| l.get(&p.asset))
            && crate::effects::is_campfire(e.kind, &e.id)
        {
            let s = p.scale3();
            out.push(Fire {
                key: (p.id, u16::MAX),
                kind: FireKind::Campfire,
                foot: p.translation() + Vec3::Y * 0.1 * s.y,
                height: 0.85 * s.y,
                radius: 0.4 * s.x.max(s.z),
                light: false,
                color: FIRE,
            });
            // (its painted flames are this bed of fire)
            continue;
        }
        for (k, a) in anchors
            .of(&p.asset)
            .iter()
            .enumerate()
            .filter(|(_, a)| a.kind == "fire")
        {
            let w = crate::fx_anchors::world_anchor(p, a);
            let lit = at_light(p.id, w.pos);
            let (radius, height) = (
                if a.radius > 0.0 { w.radius } else { 0.3 },
                if a.height > 0.0 { w.height } else { 0.8 },
            );
            out.push(Fire {
                key: (p.id, k as u16),
                kind: kind_for(radius, height),
                foot: w.pos,
                height,
                radius,
                light: lit.is_none(),
                color: lit.unwrap_or(FIRE),
            });
        }
        // the painted flames (a model with fire anchors of its own has its fires there)
        if anchors.of(&p.asset).iter().any(|a| a.kind == "fire") {
            continue;
        }
        for (k, (foot, r, h)) in painted(&p.asset).into_iter().enumerate() {
            let a = crate::fx_anchors::FxAnchor {
                kind: "fire".into(),
                pos: foot,
                radius: r,
                height: h,
                extras: Default::default(),
            };
            let w = crate::fx_anchors::world_anchor(p, &a);
            let lit = at_light(p.id, w.pos);
            // (real fire licks higher than a painted cone)
            let height = w.height * 1.4;
            out.push(Fire {
                key: (p.id, 1000 + k as u16),
                kind: kind_for(w.radius, height),
                foot: w.pos,
                height,
                radius: w.radius,
                light: lit.is_none(),
                color: lit.unwrap_or(FIRE),
            });
        }
    }
    out.sort_by_key(|f| f.key);
    out
}

/// The fires an export carries for `objects` (and underground, `dungeon`'s lights whose fixture
/// doesn't burn, as `dungeons::fx` draws them): as the editor burns them, the painted flames read
/// from the models' files when the background look hasn't finished.
pub fn export_fires(
    world: &World,
    objects: &[crate::objects::Placed],
    dungeon: Option<&crate::dungeons::model::MapDoc>,
) -> Vec<Fire> {
    let Some(anchors) = world.get_resource::<crate::fx_anchors::FxAnchors>() else {
        return vec![];
    };
    let lib = world.get_resource::<crate::objects::Library>();
    let found = world
        .get_resource::<PaintedFlames>()
        .filter(|p| p.task.is_none() && p.started);
    let mut cache: HashMap<String, Vec<(Vec3, f32, f32)>> = HashMap::new();
    let mut painted = |id: &str| -> Vec<(Vec3, f32, f32)> {
        if let Some(f) = found {
            return f.by_model.get(id).cloned().unwrap_or_default();
        }
        cache
            .entry(id.to_string())
            .or_insert_with(|| {
                lib.and_then(|l| l.get(id))
                    .and_then(|e| e.import_path.as_ref())
                    .and_then(|p| {
                        painted_flames(&crate::asset_file(p))
                    })
                    .unwrap_or_default()
            })
            .clone()
    };
    let mut fires = fires_for(objects.iter(), anchors, lib, &mut painted, dungeon);
    // (a campfire's light is its smoke's in the editor, `crate::effects`: the export's is its own)
    for f in fires
        .iter_mut()
        .filter(|f| f.key.1 == u16::MAX && dungeon.is_none())
    {
        f.light = true;
    }
    if let Some(m) = dungeon {
        let burning: std::collections::HashSet<u64> = fires.iter().map(|f| f.key.0).collect();
        for l in m
            .lights
            .iter()
            .filter(|l| !l.fixture.is_some_and(|o| burning.contains(&o)))
        {
            for (k, (kind, foot, height)) in
                crate::dungeons::fx::flames_of(l).into_iter().enumerate()
            {
                let radius = crate::dungeons::fx::bed_of(l.kind);
                fires.push(Fire {
                    key: (u64::MAX - l.id as u64, k as u16),
                    kind,
                    foot,
                    height,
                    radius,
                    light: false,
                    color: m.mood.tinted(l.color),
                });
            }
        }
    }
    fires
}

/// The fires for the engines: which baked fire (`flames/<fire>.png`, its grid in `flames.json`),
/// its foot, how tall it burns, its frame (a billboard turning about the upright, 1.2 × the flame
/// tall, its bottom `frame_bottom`; `spread` times the sheet's width: the sheet is spread that
/// much at the flame's foot and narrows back to its own width as it rises), its colour (linear;
/// `magic` 0..1 how much the fire takes it), strength, and whether it throws a light of its own
/// (with its lumens and reach).
pub fn fires_json(fires: &[Fire]) -> Vec<Value> {
    fires
        .iter()
        .map(|f| {
            let spread = f.spread();
            let (w, h, below) = f.kind.sheet().frame(f.height);
            let w = w * spread;
            let c = Color::srgb_from_array(f.color).to_linear();
            let (lumens, range) = light_of(f.kind, false);
            json!({
                "fire": f.kind.id(),
                "foot": f.foot.to_array(),
                "height": f.height,
                "width": f.radius * 2.0,
                "frame": [w, h],
                "spread": spread,
                "frame_bottom": f.foot.y - below,
                "color_linear": [c.red, c.green, c.blue],
                "magic": magic(f.color),
                "strength": if f.kind == FireKind::Candle { 2.6 } else { 2.2 },
                "light": f.light,
                "light_lumens": lumens,
                "light_range": range,
                "light_position": (f.foot + Vec3::Y * f.height * 0.45).to_array(),
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn anchor_fires(
    mut commands: Commands,
    mut af: ResMut<AnchorFires>,
    store: Res<crate::objects::ObjectStore>,
    anchors: Res<crate::fx_anchors::FxAnchors>,
    painted: Res<PaintedFlames>,
    lib: Option<Res<crate::objects::Library>>,
    dungeon: Res<crate::dungeons::model::DungeonMap>,
    sky: Res<crate::sky::SkyState>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<FlipbookMaterial>>,
    server: Res<AssetServer>,
    time: Res<Time>,
    mut lights: Query<(&AnchorLight, &mut PointLight)>,
) {
    let af = &mut *af;
    let dt = time.delta_secs().min(0.1);
    // (the placed fires change only with the objects or the anchors: looked up at most twice a
    // second, painting objects changes the store every frame)
    let key = (
        store.revision,
        anchors.revision + painted.revision * 1_000_000,
    );
    af.rescan -= dt;
    if (af.seen != Some(key) && (af.rescan <= 0.0 || af.seen.is_none_or(|s| s.1 != key.1)))
        || dungeon.is_changed()
    {
        af.seen = Some(key);
        af.rescan = 0.5;
        af.fires = fires_for(
            store.items.values(),
            &anchors,
            lib.as_deref(),
            &mut |id| painted.by_model.get(id).cloned().unwrap_or_default(),
            dungeon.0.as_ref(),
        );
        af.objects = af.fires.iter().map(|f| f.key.0).collect();
        af.eval = -1.0;
    }
    let Ok(cam) = cam.single() else { return };
    let eye = cam.translation();
    // which burn (four times a second)
    af.eval -= dt;
    if af.eval <= 0.0 {
        af.eval = 0.25;
        let mut near: Vec<(f32, usize)> = af
            .fires
            .iter()
            .enumerate()
            .map(|(i, f)| (f.foot.distance(eye), i))
            .filter(|(d, _)| *d < RANGE)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        let q = |c: [f32; 3]| {
            [
                (c[0] * 15.0) as u8,
                (c[1] * 15.0) as u8,
                (c[2] * 15.0) as u8,
            ]
        };
        let mut want: HashMap<(u64, u16), (FireKind, Vec3, f32, [f32; 3], f32)> = HashMap::new();
        for &(_, i) in near.iter().take(MAX_FLAMES) {
            let f = &af.fires[i];
            want.insert(f.key, (f.kind, f.foot, f.height, f.color, f.spread()));
        }
        let gone: Vec<_> = af
            .flames
            .iter()
            .filter(|(k, (_, foot, h, c, sp))| {
                want.get(*k).is_none_or(|w| {
                    w.1 != *foot || w.2 != *h || q(w.3) != *c || spread_key(w.4) != *sp
                })
            })
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            if let Some((e, ..)) = af.flames.remove(&k) {
                commands.entity(e).despawn();
            }
        }
        for (k, (kind, foot, h, color, spread)) in want {
            if af.flames.contains_key(&k) {
                continue;
            }
            let strength = if kind == FireKind::Candle { 2.6 } else { 2.2 };
            let mat = af
                .mats
                .entry((kind, q(color)))
                .or_insert_with(|| mats.add(material(kind, color, strength, &server)))
                .clone();
            let seed = (((k.0 as usize).wrapping_mul(7) + k.1 as usize * 3) % 8) as u8;
            let mesh = af
                .meshes
                .entry((seed, spread_key(spread)))
                .or_insert_with(|| meshes.add(quad(seed as f32 / 8.0 + 0.07, spread)))
                .clone();
            let e = commands
                .spawn(fire_bundle(kind, foot, h, spread, mesh, mat))
                .id();
            af.flames
                .insert(k, (e, foot, h, q(color), spread_key(spread)));
        }
        // the nearest few throw light
        let underground = dungeon.interior();
        let lit: Vec<usize> = near
            .iter()
            .filter(|(d, i)| *d < LIGHT_RANGE && af.fires[*i].light)
            .take(MAX_LIGHTS)
            .map(|x| x.1)
            .collect();
        let keys: Vec<(u64, u16)> = lit.iter().map(|&i| af.fires[i].key).collect();
        let stale: Vec<_> = af
            .lights
            .keys()
            .filter(|k| !keys.contains(k))
            .copied()
            .collect();
        for k in stale {
            if let Some(e) = af.lights.remove(&k) {
                commands.entity(e).despawn();
            }
        }
        for &i in &lit {
            let f = &af.fires[i];
            if af.lights.contains_key(&f.key) {
                continue;
            }
            let (lumens, range) = light_of(f.kind, underground);
            let color = Color::srgb(1.0, 0.58, 0.26);
            let e = commands
                .spawn((
                    Name::new("Fire light"),
                    AnchorLight {
                        seed: (f.key.0 as u32)
                            .wrapping_mul(31)
                            .wrapping_add(f.key.1 as u32),
                        lumens,
                    },
                    PointLight {
                        color,
                        intensity: 0.0,
                        range,
                        radius: (f.radius * 0.5).max(0.03),
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    Transform::from_translation(f.foot + Vec3::Y * f.height * 0.45),
                ))
                .id();
            af.lights.insert(f.key, e);
        }
    }
    // the lights flicker (fuller at night; the sheets turn to the eye in `face_fires`)
    let t = time.elapsed_secs();
    let night = if dungeon.interior() {
        1.0
    } else {
        0.15 + 0.85 * sky.night
    };
    for (l, mut pl) in &mut lights {
        pl.intensity = l.lumens * crate::dungeons::lights::flicker(t * 1.3, l.seed, 0.4) * night;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fire_has_its_sheet_and_the_sheets_are_bundled() {
        for k in FireKind::ALL {
            let s = k.sheet();
            assert!(s.frames >= 8 && s.frames <= s.columns * s.rows, "{k:?}");
            assert!(
                s.width_m > 0.0 && s.height_m > 0.0 && (0.0..0.5).contains(&s.foot_v),
                "{k:?}"
            );
            let path = crate::asset_file(&format!("textures/flames/{}", s.file));
            assert!(path.exists(), "{path:?}");
        }
    }

    #[test]
    fn anchors_burn_as_the_fire_their_size_says() {
        assert_eq!(kind_for(0.025, 0.07), FireKind::Candle);
        assert_eq!(kind_for(0.03, 0.08 * 1.4), FireKind::Candle);
        assert_eq!(kind_for(0.12, 0.4), FireKind::Torch);
        assert_eq!(kind_for(0.276, 0.805), FireKind::Torch);
        assert_eq!(kind_for(0.3, 0.9), FireKind::Brazier);
        assert_eq!(kind_for(0.357, 1.05), FireKind::Brazier);
        assert_eq!(kind_for(0.544, 1.44), FireKind::Campfire);
        assert_eq!(kind_for(0.45, 0.9), FireKind::Campfire);
        assert_eq!(kind_for(1.0, 0.8), FireKind::Campfire);
        // a wide pit burns as one flame spread over its bed at the foot; torches and candles keep
        // their own width
        let pit = Fire {
            key: (5, 0),
            kind: FireKind::Campfire,
            foot: Vec3::ZERO,
            height: 0.8,
            radius: 1.0,
            light: true,
            color: FIRE,
        };
        assert_eq!(pit.spread(), MAX_SPREAD);
        assert_eq!(fires_json(std::slice::from_ref(&pit)).len(), 1);
        // the stone fire pits and the hand-painted brazier: wider at the foot than the sheet
        assert_eq!(kind_for(0.41, 1.1), FireKind::Campfire);
        assert!(spread(kind_for(0.41, 1.1), 1.1, 0.41) > 1.2);
        assert!(spread(kind_for(0.296, 0.98), 0.98, 0.296) > 1.5);
        assert_eq!(spread(FireKind::Torch, 0.5, 0.12), 1.0);
        assert_eq!(spread(FireKind::Candle, 0.08, 0.03), 1.0);
    }

    #[test]
    fn painted_flame_cones_become_fires_where_they_stood() {
        let dir = crate::asset_file("models/stylepacks/handpainted");
        let sconce = dir.join("torch_sconce/torch_sconce.gltf");
        if !sconce.exists() {
            return; // (the pack's models are gitignored paid content)
        }
        let f = painted_flames(&sconce).expect("readable");
        assert_eq!(f.len(), 1, "{f:?}");
        let (foot, r, h) = f[0];
        // on the torch's head, near its top (the sconce is 1.13 m tall)
        assert!(
            foot.y > 0.6 && foot.y < 1.1 && r < 0.2 && h > 0.05 && h < 0.5,
            "{f:?}"
        );
        assert_eq!(kind_for(r, h * 1.4), FireKind::Torch);
        let brazier = painted_flames(&dir.join("brazier/brazier.gltf")).expect("readable");
        assert!(!brazier.is_empty() && brazier.len() <= 3, "{brazier:?}");
        // a model without painted flames has none
        assert!(
            painted_flames(&dir.join("anvil/anvil.gltf")).is_some_and(|v| v.is_empty())
                || !dir.join("anvil/anvil.gltf").exists()
        );
    }

    #[test]
    fn firelight_keeps_its_colours_and_magic_fire_takes_its_own() {
        assert!(magic([1.0, 0.62, 0.3]) < 0.05);
        assert!(magic([1.0, 0.55, 0.25]) < 0.2);
        assert!(magic([0.3, 1.0, 0.4]) > 0.95);
        assert!(magic([0.4, 0.5, 1.0]) > 0.95);
    }
}

/// Hands the volumetric light's volume to the fire materials (only when it changes: a new grid, or
/// a new reach as the editor camera zooms).
fn fog_fires(fog: Res<crate::volumetrics::FroxelFog>, mut mats: ResMut<Assets<FlipbookMaterial>>) {
    let on = fog.params.w > 0.5;
    let volume = on.then(|| fog.image.clone());
    let params = if on { fog.params } else { Vec4::ZERO };
    let stale = |m: &FlipbookMaterial| {
        m.extension.fog_volume != volume
            || (m.extension.params.fog - params).abs().max_element()
                > 0.01 * params.y.abs().max(0.1)
    };
    let ids: Vec<_> = mats
        .iter()
        .filter(|(_, m)| stale(m))
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        if let Some(mut m) = mats.get_mut(id) {
            m.extension.fog_volume = volume.clone();
            m.extension.params.fog = params;
        }
    }
}
