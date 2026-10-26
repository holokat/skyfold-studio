//! One-click export of the world to game engines (top bar ▸ Export, ⌘E): a generic package, a
//! Godot 4 project that opens directly, a Unity 6 folder with an editor importer, and an Unreal
//! Engine 5 folder with a Python import script.
//!
//! The dialog (`ui`) picks the target, folder and options; pressing Export takes a snapshot of the
//! world on the main thread (`core::snapshot`) and writes everything on a background thread with
//! progress and cancel. Every target is built from the same engine-independent pieces (`core`:
//! heightmap, splat weights, layer textures, GLB models, instances, water, roads, gameplay,
//! lighting) plus its own engine files (`generic`, `godot`, `unity`, `unreal`). Game-ready extras:
//! levels of detail for packed models and Godot's terrain (`lods`), collision proxies, physics
//! surfaces and water volumes (`collision`), one shared texture folder (`textures`), terrain
//! tiles and world cells for big worlds (`tiles`), the navmesh (`nav`) and the caves' crystal
//! lights (`lights`). The same exports are MCP tools (`export_generic`, `export_godot`,
//! `export_unity`, `export_unreal`).
//!
//! Test hook: `WB_EXPORT=<target>:<dir>[:<terrain>:<textures>]` exports once the demo world is
//! built (like the MCP tools) and logs `EXPORT <target> -> <summary>` (see `export_hook`);
//! `WB_EXPORT_DIALOG=1|<folder>` opens the dialog (and presses Export), see `ui::dialog_hook`.

mod collision;
pub(crate) mod core;
mod dungeons;
mod generic;
pub(crate) mod glb;
mod godot;
mod impostor;
mod lights;
mod lods;
mod nav;
mod textures;
mod tiles;
mod ui;
mod unity;
mod unreal;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use serde_json::{Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};

pub use ui::request_dialog;

/// A saved scene as one `.glb` (every piece in place round the scene's middle, ground level at
/// y = 0, first LODs), for Unreal, Unity, Godot or Blender. Returns a summary.
pub fn scene_glb(
    world: &World,
    scene: &crate::scenes::file::SceneFile,
    path: &std::path::Path,
) -> Result<String, String> {
    let snap = core::snapshot(world);
    let tmp = std::env::temp_dir().join(format!("skyfold-scene-glb-{}", std::process::id()));
    let mut opts = ExportOptions::new(Target::Generic, tmp, snap.terrain.res);
    opts.collision = false;
    let ctx = core::Ctx::new(
        opts,
        Arc::new(Mutex::new(Progress::default())),
        Arc::new(AtomicBool::new(false)),
    );
    let bundled: std::collections::HashMap<&str, &str> = scene
        .models
        .iter()
        .map(|b| (b.asset.as_str(), b.file.as_str()))
        .collect();
    let mut models: Vec<(String, Vec<u8>)> = vec![];
    let mut left = vec![];
    for (asset, _) in scene.assets() {
        let bytes = if let Some(ai) = snap.assets.iter().position(|a| a.id == asset) {
            core::model(&snap, ai, 1024, &ctx)
                .ok()
                .flatten()
                .map(|b| b.bytes_vec)
        } else if let Some(f) = bundled.get(asset.as_str()) {
            glb::pack(
                &crate::scenes::file::folder(&scene.id)
                    .join("models")
                    .join(f),
                1024,
            )
            .ok()
        } else {
            None
        };
        match bytes {
            Some(b) => models.push((asset, b)),
            None => left.push(asset),
        }
    }
    let parts: Vec<glb::MergePart> = models
        .iter()
        .map(|(asset, bytes)| glb::MergePart {
            name: core::safe_name(asset),
            glb: bytes,
            instances: scene
                .pieces
                .iter()
                .filter(|p| &p.asset == asset)
                .map(|p| {
                    let s3 = Vec3::from_array(p.stretch.unwrap_or([1.0; 3])) * p.scale;
                    (
                        p.variant,
                        Mat4::from_scale_rotation_translation(
                            s3,
                            Quat::from_array(p.rot),
                            Vec3::from_array(p.at),
                        )
                        .to_cols_array(),
                    )
                })
                .collect(),
        })
        .collect();
    if parts.is_empty() {
        return Err("none of its models can be exported".into());
    }
    let out = glb::merge(&parts).ok_or("a model in it uses data the exporter can't combine")?;
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, &out).map_err(|e| e.to_string())?;
    let placed: usize = parts.iter().map(|p| p.instances.len()).sum();
    let mut summary = format!(
        "{placed} pieces, {} models, {:.1} MB",
        parts.len(),
        out.len() as f64 / 1e6
    );
    if !left.is_empty() {
        summary.push_str(&format!("; left out: {}", left.join(", ")));
    }
    Ok(summary)
}

pub const CANCELLED: &str = "Export cancelled";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Generic,
    Godot,
    Unity,
    Unreal,
}

impl Target {
    pub const ALL: [Target; 4] = [
        Target::Generic,
        Target::Godot,
        Target::Unity,
        Target::Unreal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Target::Generic => "Generic",
            Target::Godot => "Godot 4",
            Target::Unity => "Unity 6",
            Target::Unreal => "Unreal 5",
        }
    }

    pub fn folder(self) -> &'static str {
        match self {
            Target::Generic => "Generic",
            Target::Godot => "Godot",
            Target::Unity => "Unity",
            Target::Unreal => "Unreal",
        }
    }

    /// Heightmap vertices per side for a nominal size (512 … 16384): Unity and the generic /
    /// Godot meshes use 2ⁿ + 1, Unreal its landscape sizes (63-quad sections, 2 × 2 per component).
    /// Beyond 4096 the terrain is exported in tiles (`tiles::split`): 2049-sample tiles, Unreal
    /// 2017-sample ones.
    pub fn terrain_vertices(self, nominal: usize) -> usize {
        match self {
            Target::Unreal => match nominal {
                ..=512 => 505,
                ..=1024 => 1009,
                ..=2048 => 2017,
                ..=4096 => 4033,
                _ => nominal.next_power_of_two().min(16384) / 2048 * 2016 + 1,
            },
            _ => nominal.next_power_of_two().clamp(256, 16384) + 1,
        }
    }
}

/// What to export, and where.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub target: Target,
    /// The export folder itself (created; Skyfold Studio's own files in it are replaced).
    pub dir: PathBuf,
    /// Nominal heightmap size: 512 … 16384 (tiled beyond 4096).
    pub terrain: usize,
    /// Largest texture side: 512, 1024 or 2048.
    pub texture_size: u32,
    pub trees: bool,
    /// rocks, debris, structures, props and imported models
    pub props: bool,
    /// small plants and the grass map
    pub plants: bool,
    pub water: bool,
    pub gameplay: bool,
    pub roads: bool,
    pub collision: bool,
    /// Generic: the terrain as chunked GLB meshes too (Godot always gets them).
    pub terrain_meshes: bool,
    /// The caves' rock simplified for the engine (the editor keeps every triangle).
    pub optimize_caves: bool,
    /// Models' images written once into a shared textures folder (GLBs refer to them by URI)
    /// instead of embedded in every GLB.
    pub shared_textures: bool,
    /// Skyfold Studio's navmesh in the export (for servers; Godot's navigation region) and the
    /// engines' native navmesh set up by their import scripts.
    pub navmesh: bool,
}

impl ExportOptions {
    pub fn new(target: Target, dir: PathBuf, world_res: usize) -> Self {
        Self {
            target,
            dir,
            terrain: world_res.next_power_of_two().clamp(512, 16384),
            texture_size: 1024,
            trees: true,
            props: true,
            plants: true,
            water: true,
            gameplay: true,
            roads: true,
            collision: true,
            terrain_meshes: true,
            optimize_caves: true,
            shared_textures: true,
            navmesh: true,
        }
    }
}

/// Where a running export is.
#[derive(Default, Clone)]
pub struct Progress {
    pub stage: String,
    pub fraction: f32,
}

/// Runs one export on the calling thread. Returns the summary (files, sizes, counts, warnings).
pub fn run(s: &core::Snapshot, ctx: &core::Ctx) -> Result<Value, String> {
    // (a dungeon's map open: its world isn't the one loaded)
    if let Err(e) = &s.dungeons {
        return Err(e.clone());
    }
    let t0 = std::time::Instant::now();
    let fresh = !ctx.root.exists();
    std::fs::create_dir_all(&ctx.root).map_err(|e| format!("{}: {e}", ctx.root.display()))?;
    let details = match ctx.opts.target {
        Target::Generic => generic::export(s, ctx),
        Target::Godot => godot::export(s, ctx),
        Target::Unity => unity::export(s, ctx),
        Target::Unreal => unreal::export(s, ctx),
    };
    // animated characters (Labs): the same files for every target
    let details = details.and_then(|mut d| {
        if let Some(c) = crate::characters::export::write(&s.characters, ctx)?
            && let Some(o) = d.as_object_mut()
        {
            o.insert("characters".into(), c);
        }
        Ok(d)
    });
    match details {
        Ok(details) => {
            let _ = ctx.stage("Done", 1.0);
            Ok(json!({
                "target": ctx.opts.target.name(),
                "dir": ctx.root.display().to_string(),
                "seconds": (t0.elapsed().as_secs_f32() * 10.0).round() / 10.0,
                "files": core::file_summary(ctx),
                "details": details,
                "warnings": ctx.warnings(),
            }))
        }
        Err(e) => {
            // a cancelled first export leaves nothing behind
            if e == CANCELLED && fresh {
                let _ = std::fs::remove_dir_all(&ctx.root);
            }
            Err(e)
        }
    }
}

/// The default parent folder for exports.
pub fn exports_dir() -> PathBuf {
    crate::paths::exports_dir()
}

pub struct ExportPlugin;

impl Plugin for ExportPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ui::ExportUiPlugin);
        for target in Target::ALL {
            app.mcp_tool(McpTool {
                name: match target {
                    Target::Generic => "export_generic",
                    Target::Godot => "export_godot",
                    Target::Unity => "export_unity",
                    Target::Unreal => "export_unreal",
                },
                description: match target {
                    Target::Generic => "Export the world as an engine-neutral package: 16-bit heightmap (PNG + RAW, exact height scale; also tiles beyond 4097 samples), splat weight maps, the chosen photoscan layer textures, chunked terrain GLB, every used model as GLB (variants + LODs, packed models too, + collision proxy nodes) with its textures in one shared folder, instances.json (with world cells), grass density, water and road meshes with swim volumes, gameplay JSON in the documented server format, the navmesh, lighting and a manifest. Returns files, sizes and counts.",
                    Target::Godot => "Export the world as a complete Godot 4.4+ project (project.godot + world.tscn) that opens directly: terrain tiles of chunks with crack-free LODs, a splat shader and HeightMapShape3D collision, MultiMeshInstance3D vegetation / rocks / props with LODs and simplified collision proxies with physics materials, water shader and swim volumes, a NavigationRegion3D, crystal lights, WorldEnvironment + sun and moon at the world's time of day with the editor's haze, a weather runtime (zone / biome weather blended at the camera, cloud sky, rain and snow, weather fog, volumetric haze and ground mist, lightning), Marker3D gameplay markers and a fly/walk camera. Returns files, sizes and counts.",
                    Target::Unity => "Export the world for Unity 6: a WorldBuilder folder to drop into Assets with an editor importer (Tools ▸ Skyfold Studio ▸ Import World, batch entry WorldBuilder.Importer.ImportFromCommandLine) that builds Terrain tiles (heightmap, TerrainLayers, alphamaps, trees with LOD groups to the draw distance, detail grass), props per world cell with LODs, collision proxies and physics materials, lightmap UVs for buildings, water with swim volumes, a baked NavMesh, crystal lights, sun / sky / fog and markers. Models need glTFast (com.unity.cloud.gltfast); AI Navigation is optional. Returns files, sizes and counts.",
                    Target::Unreal => "Export the world for Unreal Engine 5.4+: landscape heightmap + weightmaps with the exact Landscape X/Y/Z scale for this world (World Partition tiles for big worlds), GLB models with LODs and collision proxies, and import_world.py (Python Editor Script Plugin) that imports meshes, places instanced static meshes per world cell with collision and physical materials, builds sky atmosphere / sun / fog / post process, water planes and water volumes, crystal lights, a NavMeshBoundsVolume and markers. Returns files, sizes and counts.",
                },
                input_schema: schema,
                run: match target {
                    Target::Generic => |w, a| mcp_export(w, a, Target::Generic),
                    Target::Godot => |w, a| mcp_export(w, a, Target::Godot),
                    Target::Unity => |w, a| mcp_export(w, a, Target::Unity),
                    Target::Unreal => |w, a| mcp_export(w, a, Target::Unreal),
                },
            });
        }
        app.add_systems(Update, export_hook.after(crate::debug::demo_content));
    }
}

fn schema() -> Value {
    let flag = |d: &str| json!({ "type": "boolean", "description": d });
    json!({
        "type": "object",
        "properties": {
            "dir": { "type": "string", "description": "Export folder (created). A relative path goes under the project's exports/ folder. Skyfold Studio's own files in it are replaced." },
            "terrain_resolution": { "type": "integer", "enum": [512, 1024, 2048, 4096, 8192, 16384], "description": "Nominal heightmap size (Unity 2ⁿ+1, Unreal 505/1009/2017/4033). Beyond 4096 the terrain is exported as tiles at full resolution (Unity / Godot / generic 2049-sample tiles, Unreal 2017-sample World Partition tiles). Default: the world's own resolution." },
            "texture_size": { "type": "integer", "enum": [512, 1024, 2048], "description": "Largest texture side (default 1024)." },
            "trees": flag("Trees (default true)"),
            "props": flag("Rocks, debris, structures and props (default true)"),
            "plants": flag("Small plants and the grass map (default true)"),
            "water": flag("Ocean, lakes, rivers, waterfalls (default true)"),
            "gameplay": flag("Gameplay markers: player start, POIs, NPCs, spawn regions, resource nodes, fishing spots (default true)"),
            "roads": flag("Road meshes and centrelines (default true)"),
            "collision": flag("Object colliders (default true; terrain always collides)"),
            "terrain_meshes": flag("Generic only: chunked GLB terrain meshes (default true)"),
            "optimize_caves": flag("Simplify the caves' rock for the engine: far fewer triangles, within a few centimetres of the editor's shape (default true; the editor keeps every triangle)"),
            "navmesh": flag("Navigation: Skyfold Studio's navmesh (JSON + OBJ + walkability maps) for game servers, a Godot NavigationRegion3D built from it, and the engine's own navmesh baked from the exported collision by the Unity / Unreal import scripts (default true)"),
            "shared_textures": flag("Write every model texture once into a shared textures folder that the GLB files refer to (default true; false embeds the images in each GLB)"),
        },
        "required": ["dir"],
    })
}

fn options_from(world: &World, args: &Value, target: Target) -> Result<ExportOptions, String> {
    let dir: String = crate::mcp_tools::arg(args, "dir")?;
    let dir = PathBuf::from(dir);
    let dir = if dir.is_absolute() {
        dir
    } else {
        exports_dir().join(dir)
    };
    let res = world.resource::<crate::terrain::Terrain>().res;
    let mut o = ExportOptions::new(target, dir, res);
    if let Some(r) = opt_arg::<usize>(args, "terrain_resolution")? {
        o.terrain = r.next_power_of_two().clamp(512, 16384);
    }
    if let Some(t) = opt_arg::<u32>(args, "texture_size")? {
        o.texture_size = t.next_power_of_two().clamp(256, 4096);
    }
    for (key, field) in [
        ("trees", &mut o.trees),
        ("props", &mut o.props),
        ("plants", &mut o.plants),
        ("water", &mut o.water),
        ("gameplay", &mut o.gameplay),
        ("roads", &mut o.roads),
        ("collision", &mut o.collision),
        ("terrain_meshes", &mut o.terrain_meshes),
        ("optimize_caves", &mut o.optimize_caves),
        ("shared_textures", &mut o.shared_textures),
        ("navmesh", &mut o.navmesh),
    ] {
        if let Some(v) = opt_arg::<bool>(args, key)? {
            *field = v;
        }
    }
    Ok(o)
}

/// Exports synchronously (the editor waits; agents get the summary).
fn mcp_export(world: &mut World, args: Value, target: Target) -> Result<Vec<McpContent>, String> {
    let opts = options_from(world, &args, target)?;
    let navmesh = if opts.navmesh {
        nav::capture(world)
    } else {
        vec![]
    };
    let mut snap = core::snapshot(world);
    snap.navmesh = navmesh;
    let ctx = core::Ctx::new(
        opts,
        Arc::new(Mutex::new(Progress::default())),
        Arc::new(AtomicBool::new(false)),
    );
    run(&snap, &ctx).map(|v| vec![McpContent::Json(v)])
}

/// `WB_EXPORT=<generic|godot|unity|unreal>:<dir>[:<terrain>:<textures>]` (several separated by
/// `;`): exports at frame `WB_EXPORT_FRAME` (default 300, after the demo world), logs `EXPORT …`,
/// then quits (`WB_EXPORT_STAY=1` keeps running). `WB_EXPORT_SYNTH=1` first adds sample gameplay
/// items and roads through the data model, for testing those exports in worlds that have none.
fn export_hook(world: &mut World) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static FRAME: AtomicU32 = AtomicU32::new(0);
    let Ok(spec) = std::env::var("WB_EXPORT") else {
        return;
    };
    let f = FRAME.fetch_add(1, Ordering::Relaxed);
    let start: u32 = std::env::var("WB_EXPORT_FRAME")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    if f != start {
        return;
    }
    if std::env::var("WB_EXPORT_SYNTH").is_ok() {
        synth_content(world);
    }
    for job in spec.split(';').map(str::trim).filter(|j| !j.is_empty()) {
        let parts: Vec<&str> = job.split(':').collect();
        let target = match parts[0] {
            "generic" => Target::Generic,
            "godot" => Target::Godot,
            "unity" => Target::Unity,
            "unreal" => Target::Unreal,
            other => {
                warn!("EXPORT: unknown target {other}");
                continue;
            }
        };
        let mut args = json!({ "dir": parts.get(1).copied().unwrap_or("hook") });
        if let Some(t) = parts.get(2).and_then(|v| v.parse::<u32>().ok()) {
            args["terrain_resolution"] = t.into();
        }
        if let Some(t) = parts.get(3).and_then(|v| v.parse::<u32>().ok()) {
            args["texture_size"] = t.into();
        }
        match mcp_export(world, args, target) {
            Ok(out) => {
                for c in out {
                    if let McpContent::Json(v) = c {
                        info!("EXPORT {} -> {v}", target.name());
                    }
                }
            }
            Err(e) => warn!("EXPORT {} failed: {e}", target.name()),
        }
    }
    if std::env::var("WB_EXPORT_STAY").is_err() {
        world.write_message(AppExit::Success);
    }
}

/// Sample gameplay items and roads (test hook only; real worlds get them from the Gameplay and
/// Roads tabs).
fn synth_content(world: &mut World) {
    use crate::gameplay::*;
    let (half, h) = {
        let t = world.resource::<crate::terrain::Terrain>();
        let pts: Vec<(f32, f32)> = vec![
            (-0.2, 0.22),
            (-0.15, 0.25),
            (-0.25, 0.18),
            (-0.1, 0.1),
            (0.05, 0.2),
            (-0.3, 0.3),
        ];
        let half = t.half();
        (
            half,
            pts.iter()
                .map(|&(x, z)| [x * half, t.height_at(x * half, z * half), z * half])
                .collect::<Vec<[f32; 3]>>(),
        )
    };
    let mut g = world.resource_mut::<Gameplay>();
    g.player_start = Some(PlayerStart {
        pos: h[0],
        yaw: 0.6,
    });
    let ids: Vec<u32> = (0..7).map(|_| g.alloc_id()).collect();
    let [a, b, c, d, e, f, wolf] = ids[..] else {
        return;
    };
    g.pois.push(Poi {
        id: a,
        level: None,
        kind: PoiKind::Town,
        name: "Saltwind Harbour".into(),
        pos: h[1],
        radius: 60.0,
        notes: "Starting town".into(),
        tags: vec!["safe".into()],
    });
    g.pois.push(Poi {
        id: b,
        level: None,
        kind: PoiKind::Dungeon,
        name: "Old Mine".into(),
        pos: h[3],
        radius: 25.0,
        notes: String::new(),
        tags: vec![],
    });
    g.npcs.push(Npc {
        id: c,
        name: "Marta the Smith".into(),
        role: NpcRole::Vendor,
        pos: h[2],
        yaw: 1.2,
        poi: Some(a),
        notes: String::new(),
        life: Default::default(),
    });
    g.creatures.push(CreatureType {
        id: wolf,
        name: "Grey Wolf".into(),
        family: "wolf".into(),
        level: [2, 4],
        aggression: Aggression::Hostile,
        elite: false,
        notes: String::new(),
        character: None,
    });
    let c4 = h[4];
    g.spawns.push(SpawnRegion {
        id: d,
        name: "Wolf Meadow".into(),
        polygon: vec![
            [c4[0] - 60.0, c4[2] - 40.0],
            [c4[0] + 60.0, c4[2] - 50.0],
            [c4[0] + 50.0, c4[2] + 50.0],
            [c4[0] - 55.0, c4[2] + 40.0],
        ],
        entries: vec![SpawnEntry {
            creature: wolf,
            weight: 1.0,
        }],
        density: 40.0,
        max_alive: 12,
        respawn_secs: 120.0,
        level: [2, 4],
        pack: [1, 3],
        behaviour: SpawnBehaviour::Roam,
        notes: String::new(),
    });
    g.nodes.push(ResourceNode {
        id: e,
        auto: false,
        kind: ResourceKind::Ore,
        variant: "Copper Vein".into(),
        tier: 1,
        pos: h[5],
        respawn_secs: 300.0,
        ore: None,
        area: None,
    });
    g.nodes.push(ResourceNode {
        id: f,
        auto: false,
        kind: ResourceKind::Herb,
        variant: "Silverleaf".into(),
        tier: 1,
        pos: h[4],
        respawn_secs: 180.0,
        ore: None,
        area: None,
    });
    g.dirty = true;
    let mut roads = world.resource_mut::<crate::roads::Roads>();
    let rid = roads.alloc_id();
    roads.roads.push(crate::roads::Road {
        authored: None,
        options: Default::default(),
        cleared: vec![],
        prop_ids: vec![],
        id: rid,
        name: "Harbour Road".into(),
        kind: crate::roads::RoadKind::Cobblestone,
        points: vec![h[0], h[1], h[3], h[4]],
        width: 6.0,
    });
    let rid = roads.alloc_id();
    roads.roads.push(crate::roads::Road {
        authored: None,
        options: Default::default(),
        cleared: vec![],
        prop_ids: vec![],
        id: rid,
        name: "Mine Trail".into(),
        kind: crate::roads::RoadKind::Trail,
        points: vec![h[3], h[5]],
        width: 1.8,
    });
    roads.dirty = true;
    // a little camp of Poly Haven and food models by the player start (packed-glTF exports)
    let camp: Vec<(&str, f32, f32, f32)> = vec![
        ("ph:wooden_picnic_table", 6.0, 3.0, 0.4),
        ("ph:stone_fire_pit", 10.0, -2.0, 0.0),
        ("ph:wine_barrel_01", 4.0, 6.0, 1.1),
        ("ph:wine_barrel_01", 5.0, 7.0, 2.3),
        ("ph:street_lamp_01", 2.0, -3.0, 0.0),
        ("props:food_apple", 6.0, 3.0, 0.0),
    ];
    let placed: Vec<crate::objects::Placed> = {
        let t = world.resource::<crate::terrain::Terrain>();
        camp.iter()
            .map(|&(asset, dx, dz, yaw)| {
                let (x, z) = (h[0][0] + dx, h[0][2] + dz);
                // the apple sits on the table top
                let lift = if asset.starts_with("props:") {
                    0.78
                } else {
                    0.0
                };
                crate::objects::Placed {
                    group: 0,
                    id: 0,
                    asset: asset.into(),
                    variant: 0,
                    pos: [x, t.height_at(x, z) + lift, z],
                    rot: Quat::from_rotation_y(yaw).to_array(),
                    scale: 1.0,
                    stretch: None,
                    locked: false,
                    glow: None,
                }
            })
            .collect()
    };
    let mut store = world.resource_mut::<crate::objects::ObjectStore>();
    for mut p in placed {
        p.id = store.new_id();
        store.insert(p);
    }
    info!("EXPORT: synthetic gameplay items, roads and a camp added (half size {half})");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small island world with no objects (no GPU, no library needed).
    fn tiny_world() -> core::Snapshot {
        let mut terrain = crate::terrain::Terrain::new(64, 8.0, 0.0);
        let n = terrain.n();
        for j in 0..n {
            for i in 0..n {
                let (x, z) = (i as f32 / 64.0 - 0.5, j as f32 / 64.0 - 0.5);
                terrain.heights[j * n + i] = 60.0 - (x * x + z * z).sqrt() * 200.0;
            }
        }
        core::Snapshot {
            name: "Test Isle".into(),
            seed: 1,
            terrain,
            layer_choices: Default::default(),
            library: vec![],
            layer_surface: crate::terrain::layers::Layer::ALL.map(|l| l.default_surface()),
            assets: vec![],
            objects: vec![],
            water: Default::default(),
            ocean: Default::default(),
            lighting: Default::default(),
            look: Default::default(),
            wind: [1.0, 0.0, 2.0, 0.5],
            draw_distance: 3000.0,
            detail: 1.0,
            gameplay: Default::default(),
            roads: Default::default(),
            zones: vec![],
            grass: None,
            grass_amount: 1.0,
            grass_height: 1.0,
            grass_lushness: 0.0,
            painted: false,
            canopy_shade: 1.0,
            camera: (Vec3::new(0.0, 200.0, 300.0), Vec3::ZERO),
            nature: None,
            caves: Default::default(),
            cave_holes: Default::default(),
            navmesh: vec![],
            dungeons: Ok(vec![]),
            fires: vec![],
            lights: vec![],
            characters: Default::default(),
        }
    }

    fn ctx(target: Target, dir: PathBuf, cancel: bool) -> core::Ctx {
        let mut o = ExportOptions::new(target, dir, 512);
        o.texture_size = 256;
        core::Ctx::new(
            o,
            Arc::new(Mutex::new(Progress::default())),
            Arc::new(AtomicBool::new(cancel)),
        )
    }

    #[test]
    fn exports_every_target_and_cancels_cleanly() {
        let s = tiny_world();
        let base = std::env::temp_dir().join(format!("wb_export_test_{}", std::process::id()));
        for t in Target::ALL {
            let dir = base.join(t.folder());
            let summary = run(&s, &ctx(t, dir.clone(), false))
                .unwrap_or_else(|e| panic!("{}: {e}", t.name()));
            assert!(
                summary["files"]["count"].as_u64().unwrap_or(0) > 5,
                "{}",
                t.name()
            );
            assert!(
                dir.join("README.md").exists() && dir.join("manifest.json").exists(),
                "{}",
                t.name()
            );
        }
        // the heightmap round-trips with the manifest's scale
        let m: Value = serde_json::from_str(
            &std::fs::read_to_string(base.join("Generic/manifest.json")).unwrap(),
        )
        .unwrap();
        let meta = &m["terrain"]["meta"];
        let n = meta["vertices_per_side"].as_u64().unwrap() as usize;
        let raw = std::fs::read(base.join("Generic/terrain/heightmap.r16")).unwrap();
        let v = u16::from_le_bytes([
            raw[(n / 2 * n + n / 2) * 2],
            raw[(n / 2 * n + n / 2) * 2 + 1],
        ]) as f32;
        let (lo, hi) = (
            meta["height_min_m"].as_f64().unwrap() as f32,
            meta["height_max_m"].as_f64().unwrap() as f32,
        );
        let h = lo + v / 65535.0 * (hi - lo);
        assert!((h - s.terrain.height_at(0.0, 0.0)).abs() < 0.05, "{h}");
        // a cancelled first export leaves nothing behind
        let dir = base.join("cancelled");
        assert_eq!(
            run(&s, &ctx(Target::Godot, dir.clone(), true))
                .err()
                .as_deref(),
            Some(CANCELLED)
        );
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
