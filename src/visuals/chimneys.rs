//! Chimney smoke: every `smoke` effect anchor of the placed models (the style packs' `fx_smoke_<n>`
//! nodes, `crate::fx_anchors`: chimneys of houses, inns and smithies, forge stacks, steaming
//! cauldrons) breathes a column of smoke that rises, bends over downwind and thins out. Grey-white
//! wood smoke by default; an anchor whose `look` is `steam` steams (whiter, shorter); a forge's or
//! smithy's stack smokes darker.
//!
//! Each column is one small mesh of camera-facing puffs animated in `chimney_smoke.wesl`, one
//! material shared by every column: no work per puff on the CPU. The puffs are lit by a small model
//! of their own (the sun and moon through one tap of their shadow cascade, scattered forwards so
//! backlit smoke glows, the sky's fill, the volumetric fog in front) rather than Bevy's full PBR,
//! whose light loops cost too much over large overlapping blended puffs; far columns draw half
//! their puffs and puffs near the eye are capped in size. Only the nearest columns within range are
//! drawn (`RANGE`, `MAX_COLUMNS`; far ones fade out first), so a town of sixty chimneys stays
//! cheap. The
//! wind blowing at the viewer (`wind::WindState`, gusts and all) bends them; they move on the
//! world's clock (still while editing from the overview, like the rest of the world).
//!
//! On/off and thickness are saved with the world (`sky::Lighting::air`). MCP: `smoke`.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use serde_json::{Value, json};

use crate::fx_anchors::{FxAnchors, world_anchor};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg, position_arg, position_schema};
use crate::noise::Rng;

/// How far (m) from the eye columns are drawn (fading out over the last quarter), and how many.
const RANGE: f32 = 520.0;
const FADE_FROM: f32 = 380.0;
const MAX_COLUMNS: usize = 64;

/// How a smoke anchor smokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmokeLook {
    /// a hearth's grey-white wood smoke
    Smoke,
    /// a cauldron's or a kettle's steam: whiter, shorter
    Steam,
    /// a forge's or smithy's stack: darker coal smoke
    Forge,
}

impl SmokeLook {
    pub fn id(self) -> &'static str {
        match self {
            SmokeLook::Smoke => "smoke",
            SmokeLook::Steam => "steam",
            SmokeLook::Forge => "forge",
        }
    }
    fn code(self) -> f32 {
        match self {
            SmokeLook::Smoke => 0.0,
            SmokeLook::Steam => 1.0,
            SmokeLook::Forge => 2.0,
        }
    }
}

/// One smoking anchor of a placed object.
#[derive(Clone, Debug, PartialEq)]
pub struct SmokeSource {
    /// (object id, anchor index among the model's anchors)
    pub key: (u64, u16),
    pub model: String,
    pub look: SmokeLook,
    /// the chimney's mouth (world)
    pub mouth: Vec3,
    /// its radius (m)
    pub radius: f32,
    /// how tall the column rises before it has thinned away (m)
    pub column: f32,
}

/// The look of a smoke anchor: its `look` extra, else a forge's or a smithy's stack, else smoke.
pub fn look_of(model: &str, a: &crate::fx_anchors::FxAnchor) -> SmokeLook {
    match a.extra_str("look") {
        Some("steam") => SmokeLook::Steam,
        Some("forge") => SmokeLook::Forge,
        Some(_) => SmokeLook::Smoke,
        None if model.contains("forge") || model.contains("smith") => SmokeLook::Forge,
        None => SmokeLook::Smoke,
    }
}

/// Every placed object's smoke anchors, as smoke sources (sorted by key). The anchor's `height`
/// is the stretch of dense smoke the model's maker meant; the column carries on a few times
/// that, thinning out (steam vanishes much sooner).
pub fn sources<'a>(
    objects: impl Iterator<Item = &'a crate::objects::Placed>,
    anchors: &FxAnchors,
) -> Vec<SmokeSource> {
    let mut out = vec![];
    for p in objects {
        for (k, a) in anchors
            .of(&p.asset)
            .iter()
            .enumerate()
            .filter(|(_, a)| a.kind == "smoke")
        {
            let w = world_anchor(p, a);
            let look = look_of(&p.asset, a);
            let h = if a.height > 0.0 { w.height } else { 3.0 };
            let column = match look {
                SmokeLook::Steam => (h * 1.7).clamp(2.0, 10.0),
                _ => (h * 4.5).clamp(6.0, 30.0),
            };
            out.push(SmokeSource {
                key: (p.id, k as u16),
                model: p.asset.clone(),
                look,
                mouth: w.pos,
                radius: if a.radius > 0.0 { w.radius } else { 0.3 },
                column,
            });
        }
    }
    out.sort_by_key(|s| s.key);
    out
}

pub type ChimneyMaterial = ExtendedMaterial<StandardMaterial, ChimneyExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct ChimneyParams {
    /// xy: wind direction (xz), z: wind speed with its gusts (m/s), w: clock (s)
    pub wind: Vec4,
    /// x: thickness, y: fade-out starts (m from the eye), z: gone by (m), w: spare
    pub look: Vec4,
    /// rgb: wood smoke at the mouth (linear), a: grey level high up
    pub smoke: Vec4,
    /// rgb: forge smoke at the mouth, a: grey level high up
    pub forge: Vec4,
    /// `volumetrics::FroxelFog::params` (w 0: no volumetric light)
    pub fog: Vec4,
    /// rgb: the sky's fill light (before exposure, `air::sky_fill`), w: spare
    pub sky: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct ChimneyExt {
    #[uniform(100)]
    pub params: ChimneyParams,
    #[texture(101)]
    #[sampler(102)]
    pub noise: Handle<Image>,
    /// The volumetric light's integrated volume (`volumetrics::FroxelFog`): smoke in the fog is
    /// veiled by it like everything else (None while it is off).
    #[texture(103, dimension = "3d")]
    #[sampler(104)]
    pub fog_volume: Option<Handle<Image>>,
}

impl MaterialExtension for ChimneyExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/chimney_smoke.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/chimney_smoke.wesl".into()
    }
}

/// A drawn column.
#[derive(Component)]
pub struct SmokeColumn;

/// The smoke anchors in the world and the columns drawn now.
#[derive(Resource, Default)]
pub struct Chimneys {
    seen: Option<(u64, u64)>,
    rescan: f32,
    pub sources: Vec<SmokeSource>,
    /// drawn columns by key (with where their mouth was, to rebuild one that moved)
    pub drawn: HashMap<(u64, u16), (Entity, Vec3)>,
    material: Option<Handle<ChimneyMaterial>>,
    eval: f32,
    clock: f32,
    gust: f32,
}

/// The puffs of one column: camera-facing quads expanded in the shader. Position: the mouth;
/// vertex colour: (phase 0..1, seed, size at the mouth (m), opacity); normal: (column height,
/// mouth radius, look + 0.9 × LOD rank: far off only the lower half of the ranks draw).
pub fn column_mesh(s: &SmokeSource) -> Mesh {
    let mut rng = Rng::new(s.key.0.wrapping_mul(0x9e37_79b9) ^ (s.key.1 as u64 * 7919 + 17));
    let count = match s.look {
        SmokeLook::Steam => 16,
        _ => 30,
    };
    let (mut pos, mut uv, mut col, mut nor, mut idx) = (vec![], vec![], vec![], vec![], vec![]);
    for k in 0..count {
        let a = rng.f32() * std::f32::consts::TAU;
        let r = s.radius * 0.4 * rng.f32().sqrt();
        let c = s.mouth + Vec3::new(a.cos() * r, 0.0, a.sin() * r);
        // evenly spread phases (jittered): the column is never empty or clumped
        let phase = (k as f32 + rng.f32() * 0.8) / count as f32;
        let size = s.radius * rng.range(1.3, 2.0);
        let base = pos.len() as u32;
        let (sd, op) = (rng.f32(), rng.range(0.7, 1.0));
        // (ranks interleaved along the column, so dropping the upper half thins it evenly)
        let rank = if k % 2 == 0 {
            rng.range(0.0, 0.48)
        } else {
            rng.range(0.52, 1.0)
        };
        for corner in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
            pos.push(c.to_array());
            uv.push(corner);
            nor.push([s.column, s.radius, s.look.code() + 0.9 * rank]);
            col.push([phase, sd, size, op]);
        }
        idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
    }
    // two unused corner vertices: the mesh's bounds cover the whole column as it rises and bends
    // (GPU culling goes by them; the puffs themselves are expanded in the shader)
    // (downwind a puff travels up to ~0.55 × the wind × its rise time: 14 m/s at most)
    let reach = s.column * 8.0 + 10.0;
    for c in [
        s.mouth - Vec3::new(reach, 1.0, reach),
        s.mouth + Vec3::new(reach, s.column * 1.3 + 4.0, reach),
    ] {
        pos.push(c.to_array());
        uv.push([0.0, 0.0]);
        nor.push([s.column, s.radius, 0.0]);
        col.push([0.0, 0.5, 0.0, 0.0]);
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

#[allow(clippy::too_many_arguments)]
fn update_chimneys(
    mut commands: Commands,
    mut ch: ResMut<Chimneys>,
    store: Res<crate::objects::ObjectStore>,
    anchors: Res<FxAnchors>,
    lighting: Res<crate::sky::Lighting>,
    wind: Res<crate::wind::WindState>,
    motion: Res<crate::wind::Motion>,
    tex: Option<Res<crate::ocean::OceanTextures>>,
    cam: Query<(&GlobalTransform, Option<&DistanceFog>), With<crate::camera::MainCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<ChimneyMaterial>>,
    time: Res<Time>,
    fog: Option<Res<crate::volumetrics::FroxelFog>>,
    sky: Res<crate::sky::SkyState>,
) {
    let Some(tex) = tex else { return };
    let ch = &mut *ch;
    let dt = time.delta_secs().min(0.1);
    // (the anchors change only with the objects or the index: looked up at most twice a second,
    // painting objects changes the store every frame)
    let key = (store.revision, anchors.revision);
    ch.rescan -= dt;
    if ch.seen != Some(key) && (ch.rescan <= 0.0 || ch.seen.is_none_or(|s| s.1 != key.1)) {
        ch.seen = Some(key);
        ch.rescan = 0.5;
        ch.sources = sources(store.items.values(), &anchors);
        ch.eval = -1.0;
    }
    let settings = &lighting.air;
    let mat = ch
        .material
        .get_or_insert_with(|| {
            mats.add(ChimneyMaterial {
                base: StandardMaterial {
                    base_color: Color::WHITE,
                    alpha_mode: AlphaMode::Premultiplied,
                    unlit: true,
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                },
                extension: ChimneyExt {
                    params: ChimneyParams::default(),
                    noise: tex.noise.clone(),
                    fog_volume: None,
                },
            })
        })
        .clone();
    let Ok((cam, haze)) = cam.single() else {
        return;
    };
    let eye = cam.translation();
    // which columns are drawn (four times a second)
    ch.eval -= dt;
    if ch.eval <= 0.0 {
        ch.eval = 0.25;
        let mut want: Vec<(f32, usize)> = if settings.smoke && settings.smoke_strength > 0.0 {
            ch.sources
                .iter()
                .enumerate()
                .map(|(i, s)| (s.mouth.distance(eye), i))
                .filter(|(d, _)| *d < RANGE)
                .collect()
        } else {
            vec![]
        };
        want.sort_by(|a, b| a.0.total_cmp(&b.0));
        want.truncate(MAX_COLUMNS);
        let keep: HashMap<(u64, u16), Vec3> = want
            .iter()
            .map(|&(_, i)| (ch.sources[i].key, ch.sources[i].mouth))
            .collect();
        let gone: Vec<(u64, u16)> = ch
            .drawn
            .iter()
            .filter(|(k, (_, m))| keep.get(*k).is_none_or(|w| w.distance(*m) > 1e-3))
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            if let Some((e, _)) = ch.drawn.remove(&k) {
                commands.entity(e).despawn();
            }
        }
        for &(_, i) in &want {
            let s = &ch.sources[i];
            if ch.drawn.contains_key(&s.key) {
                continue;
            }
            let e = commands
                .spawn((
                    Name::new(format!("Chimney smoke {}:{}", s.key.0, s.key.1)),
                    SmokeColumn,
                    Mesh3d(meshes.add(column_mesh(s))),
                    MeshMaterial3d(mat.clone()),
                    Transform::IDENTITY,
                    NotShadowCaster,
                    // (the puffs are expanded in the vertex shader: the mesh bounds are its corners)
                    bevy::camera::visibility::NoFrustumCulling,
                ))
                .id();
            ch.drawn.insert(s.key, (e, s.mouth));
        }
    }
    if ch.drawn.is_empty() {
        return;
    }
    // the world's clock, the wind at the viewer with its gusts (eased: a puff never jumps)
    if motion.live {
        ch.clock = (ch.clock + dt) % 36_000.0;
    }
    let g = wind.gust(time.elapsed_secs());
    ch.gust += (g - ch.gust) * (1.0 - (-dt * 0.7).exp());
    let dir = wind.dir.normalize_or(Vec2::X);
    let speed = (wind.speed * ch.gust).clamp(0.2, 14.0);
    if let Some(mut m) = mats.get_mut(&mat) {
        let p = &mut m.extension.params;
        p.wind = Vec4::new(dir.x, dir.y, speed, ch.clock);
        p.look = Vec4::new(
            0.42 * settings.smoke_strength.clamp(0.0, 2.0),
            FADE_FROM,
            RANGE,
            0.0,
        );
        p.smoke = Vec4::new(0.42, 0.42, 0.44, 0.62);
        p.forge = Vec4::new(0.22, 0.21, 0.2, 0.48);
        p.sky = super::air::sky_fill(&sky, haze).extend(0.0);
        let (volume, params) = super::air::froxel(fog.as_deref());
        p.fog = params;
        if m.extension.fog_volume != volume {
            m.extension.fog_volume = volume;
        }
    }
}

pub struct ChimneysPlugin;

impl Plugin for ChimneysPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<ChimneyMaterial>::default())
            .init_resource::<Chimneys>()
            .add_systems(Update, update_chimneys.after(crate::sky::SkySystems));
        app.mcp_tool(McpTool {
            name: "smoke",
            description: "Chimney smoke: every `smoke` effect anchor of the placed models (chimneys of houses, inns and smithies, forge stacks, steaming cauldrons; `effect_anchors` lists them) breathes a column of smoke that rises, bends over downwind with the wind blowing at the viewer, thins out and fades; grey-white wood smoke, whiter shorter steam where the anchor says `look: steam`, darker smoke from forges and smithies. Lit by the sun and sky like the rest of the scene. The nearest 64 within 520 m are drawn (they fade out from 380 m). \
`enabled` turns all chimney smoke on or off, `strength` sets how thick it is (0..2, default 1); saved with the world. Lists the smoke sources near `position` ([x, z]) within `radius` (m, default 300; without a position: the whole world), nearest first (at most `limit`, default 50): object id, model, anchor, look, the chimney's mouth, the column's height and whether it is drawn now.",
            input_schema: || {
                json!({ "type": "object", "properties": {
                    "enabled": { "type": "boolean", "description": "chimney smoke on or off (default on)" },
                    "strength": { "type": "number", "minimum": 0, "maximum": 2, "description": "how thick the smoke is (default 1)" },
                    "position": position_schema("centre of the search"),
                    "radius": { "type": "number", "description": "metres (default 300)" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 } } })
            },
            run: smoke_tool,
        });
    }
}

fn smoke_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let enabled = opt_arg::<bool>(&args, "enabled")?;
    let strength = opt_arg::<f32>(&args, "strength")?;
    if enabled.is_some() || strength.is_some() {
        let mut l = world.resource_mut::<crate::sky::Lighting>();
        if let Some(e) = enabled {
            l.air.smoke = e;
        }
        if let Some(s) = strength {
            l.air.smoke_strength = s.clamp(0.0, 2.0);
        }
        world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    }
    let centre = position_arg(&args, "position")?.map(|(p, _)| p);
    let radius = opt_arg::<f32>(&args, "radius")?.unwrap_or(300.0).max(0.1);
    let limit = opt_arg::<usize>(&args, "limit")?
        .unwrap_or(50)
        .clamp(1, 500);
    let air = world.resource::<crate::sky::Lighting>().air.clone();
    // (the sources as the objects are now, even before the next frame rescans them)
    let list = sources(
        world
            .resource::<crate::objects::ObjectStore>()
            .items
            .values(),
        world.resource::<FxAnchors>(),
    );
    let ch = world.resource::<Chimneys>();
    let r1 = |v: f32| (v as f64 * 10.0).round() / 10.0;
    let mut rows: Vec<(f32, Value)> = list
        .iter()
        .filter_map(|s| {
            let d = centre.map(|c| c.distance(s.mouth.xz())).unwrap_or(0.0);
            if centre.is_some() && d > radius {
                return None;
            }
            Some((d, json!({ "object": s.key.0, "model": s.model, "anchor": s.key.1, "look": s.look.id(), "mouth": [r1(s.mouth.x), r1(s.mouth.y), r1(s.mouth.z)], "column_m": r1(s.column), "drawn": ch.drawn.contains_key(&s.key), "distance": centre.map(|_| r1(d)) })))
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total = rows.len();
    Ok(vec![McpContent::Json(json!({
        "enabled": air.smoke,
        "strength": air.smoke_strength,
        "sources": rows.into_iter().take(limit).map(|r| r.1).collect::<Vec<_>>(),
        "sources_total": total,
        "drawn_now": ch.drawn.len(),
        "range_m": RANGE,
        "max_columns": MAX_COLUMNS,
    }))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx_anchors::FxAnchor;
    use serde_json::Map;

    fn anchor(look: Option<&str>, height: f32) -> FxAnchor {
        let mut extras = Map::new();
        if let Some(l) = look {
            extras.insert("look".into(), json!(l));
        }
        FxAnchor {
            kind: "smoke".into(),
            pos: Vec3::new(2.0, 9.9, -1.3),
            radius: 0.3,
            height,
            extras,
        }
    }

    #[test]
    fn anchors_smoke_as_their_look_says() {
        assert_eq!(
            look_of("hp:house_large", &anchor(None, 2.5)),
            SmokeLook::Smoke
        );
        assert_eq!(
            look_of("hp:cindermaw_cauldron", &anchor(Some("steam"), 2.2)),
            SmokeLook::Steam
        );
        assert_eq!(
            look_of("hp:blacksmith_forge", &anchor(None, 3.0)),
            SmokeLook::Forge
        );
        assert_eq!(
            look_of("hp:blacksmith", &anchor(None, 4.0)),
            SmokeLook::Forge
        );
    }

    #[test]
    fn placed_chimneys_smoke_from_their_mouths() {
        let mut fx = FxAnchors::default();
        fx.by_model.insert(
            "hp:house_large".into(),
            vec![
                anchor(None, 2.5),
                FxAnchor {
                    kind: "fire".into(),
                    ..anchor(None, 1.0)
                },
            ],
        );
        fx.by_model
            .insert("hp:cauldron".into(), vec![anchor(Some("steam"), 2.2)]);
        let placed = |id: u64, asset: &str, pos: [f32; 3]| -> crate::objects::Placed {
            serde_json::from_value(json!({ "id": id, "asset": asset, "variant": 0, "pos": pos, "rot": [0.0, 0.0, 0.0, 1.0], "scale": 1.0 })).unwrap()
        };
        let objs = [
            placed(7, "hp:house_large", [100.0, 5.0, 50.0]),
            placed(3, "hp:cauldron", [0.0, 0.0, 0.0]),
            placed(9, "hp:plain", [0.0; 3]),
        ];
        let s = sources(objs.iter(), &fx);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].key, (3, 0));
        assert_eq!(s[0].look, SmokeLook::Steam);
        assert!(s[0].column < 6.0);
        let h = &s[1];
        assert_eq!((h.key, h.look), ((7, 0), SmokeLook::Smoke));
        assert!(
            h.mouth.distance(Vec3::new(102.0, 14.9, 48.7)) < 1e-3,
            "{:?}",
            h.mouth
        );
        assert!(h.column > 8.0 && h.column <= 30.0);
        let m = column_mesh(h);
        assert_eq!(m.count_vertices(), 30 * 4 + 2);
    }
}
