//! Scenes: a decorated spot saved for reuse — a shrine with its lanterns and offerings, a temple
//! inside and out — placed again in any world exactly as it was laid out, exported, shared or
//! listed on the marketplace.
//!
//! - Save: Objects › Scenes › Save a scene, then draw a loop round the objects (like a set piece
//!   or a lake); or right-click an object or set › Save as scene. A dialog names it. Every placed
//!   object whose middle is inside the loop is kept with its exact placement (each piece's height
//!   above the ground is kept, so it sits right on other ground). The user's own imported models
//!   are copied into the scene, so it works anywhere (`file`).
//! - Place: click a scene's tile, then the ground (Z / X turn it first). It lands as one set
//!   (`placement::group`): the handles move, turn and resize it whole, Ungroup frees the pieces.
//!   Buildings level their ground, nature clears out of its area and the pieces then sit on the
//!   levelled ground. Pieces from asset packs this account hasn't got are left out (and named).
//! - Export: the scene as a `.zip` (opens in any Skyfold Studio), or as one `.glb` model with
//!   every piece in place for Unreal, Unity, Godot or Blender (`export::scene_glb`).
//! - Sell or share: List on marketplace packages the zip and a cover picture for
//!   skyfoldstudio.com and says which packs a buyer needs (`ui`). Bought scenes download straight
//!   into the library (`market::site`).
//!
//! MCP: `scenes_list`, `scenes_save`, `scenes_place`, `scenes_export` (`mcp`).

pub mod file;
pub(crate) mod mcp;
pub mod ui;

use bevy::prelude::*;

use crate::editor::{ObjectTool, Tab};
use crate::io::WorldData;
use file::SceneFile;

pub struct ScenesPlugin;

impl Plugin for ScenesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Scenes>()
            .add_systems(
                Update,
                (from_menu, capture_loop, place_click)
                    .chain()
                    .after(crate::placement::handles::input)
                    .before(crate::tools::ToolSystems::Apply),
            )
            .add_systems(Update, cursor)
            .add_systems(Update, export_requests);
        mcp::register(app);
    }
}

/// What the Scenes tab is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Mode {
    #[default]
    Off,
    /// the next loop drawn on the ground picks the objects to save
    Capture,
    /// clicks on the ground place this scene
    Place(String),
}

/// The Save dialog: what's going in, and its name.
#[derive(Clone, Debug)]
pub struct SaveDraft {
    pub ids: Vec<u64>,
    pub outline: Option<Vec<Vec2>>,
    pub name: String,
    pub description: String,
}

#[derive(Resource, Default)]
pub struct Scenes {
    /// the library (read from disk when the tab first shows, and after each change)
    pub list: Vec<SceneFile>,
    pub loaded: bool,
    pub mode: Mode,
    /// placing: the scene's turn (radians, Z / X)
    pub yaw: f32,
    pub save: Option<SaveDraft>,
    /// the List on marketplace dialog (a scene id), and its rights box
    pub sell: Option<String>,
    pub sell_ok: bool,
    /// renaming (scene id, new name)
    pub rename: Option<(String, String)>,
    /// Delete pressed once (scene id): the second press deletes
    pub confirm_delete: Option<String>,
    /// a `.glb` export to write (scene id, file) — needs the whole world (`export_requests`)
    pub export_glb: Option<(String, std::path::PathBuf)>,
}

impl Scenes {
    pub fn reload(&mut self) {
        self.list = file::list();
        self.loaded = true;
    }
    pub fn get(&self, id: &str) -> Option<&SceneFile> {
        self.list.iter().find(|s| s.id == id)
    }
    pub fn capturing(&self) -> bool {
        self.mode == Mode::Capture
    }
    pub fn placing(&self) -> Option<&str> {
        match &self.mode {
            Mode::Place(id) => Some(id),
            _ => None,
        }
    }
    /// A name nobody has yet: "Scene 3".
    pub fn next_name(&self) -> String {
        (1..)
            .map(|k| format!("Scene {k}"))
            .find(|n| !self.list.iter().any(|s| &s.name == n))
            .unwrap_or_default()
    }
}

/// Whether the Place tool's clicks belong to the Scenes tab (capturing or placing).
pub fn active(scenes: &Scenes, ed: &crate::editor::Editor) -> bool {
    scenes.mode != Mode::Off && ed.tab == Tab::Objects && ed.object_tool == ObjectTool::Place
}

/// Right-click › Save as scene: the dialog for that object or set.
fn from_menu(mut menu: ResMut<crate::placement::lock::ObjectMenu>, mut scenes: ResMut<Scenes>) {
    let Some(ids) = menu.save_scene.take() else {
        return;
    };
    if !scenes.loaded {
        scenes.reload();
    }
    let name = scenes.next_name();
    scenes.save = Some(SaveDraft {
        ids,
        outline: None,
        name,
        description: String::new(),
    });
}

/// Capturing: the loop drawn picks the objects, then the Save dialog opens.
fn capture_loop(stroke: Res<crate::tools::Stroke>, mut scenes: ResMut<Scenes>, mut d: WorldData) {
    if !scenes.capturing() || !active(&scenes, &d.ed) || !stroke.just_ended {
        return;
    }
    let raw: Vec<Vec2> = stroke.path.iter().map(|p| p.xz()).collect();
    let Some(outline) = crate::setpieces::clean_outline(&raw) else {
        d.ed.toast("Hold the mouse and draw a loop round the objects to save, like drawing a lake");
        return;
    };
    let ids = file::inside(&d.store, &outline);
    if ids.is_empty() {
        d.ed.toast(
            "There are no placed objects inside that loop — draw it round the objects to save",
        );
        return;
    }
    if !scenes.loaded {
        scenes.reload();
    }
    let name = scenes.next_name();
    scenes.mode = Mode::Off;
    scenes.save = Some(SaveDraft {
        ids,
        outline: Some(outline),
        name,
        description: String::new(),
    });
}

/// Placing: Z / X turn it, a click on the ground places it, Esc stops.
fn place_click(
    stroke: Res<crate::tools::Stroke>,
    keys: Res<ButtonInput<KeyCode>>,
    pointer: Res<crate::camera::Pointer>,
    handles: Res<crate::placement::Handles>,
    mut scenes: ResMut<Scenes>,
    mut d: WorldData,
) {
    let Some(id) = scenes.placing().map(str::to_string) else {
        return;
    };
    if !active(&scenes, &d.ed) {
        return;
    }
    if !pointer.keyboard_captured {
        if keys.just_pressed(KeyCode::Escape) {
            scenes.mode = Mode::Off;
            return;
        }
        if keys.just_pressed(KeyCode::KeyZ) {
            scenes.yaw += 15f32.to_radians();
        }
        if keys.just_pressed(KeyCode::KeyX) {
            scenes.yaw -= 15f32.to_radians();
        }
    }
    if !stroke.just_started || handles.took_press {
        return;
    }
    let Some(hit) = stroke.hit else { return };
    let Some(scene) = scenes.get(&id).cloned() else {
        scenes.mode = Mode::Off;
        return;
    };
    match place(&mut d, &scene, hit.xz(), scenes.yaw) {
        Ok(p) => {
            let left = p.left_out();
            d.ed.toast(if left.is_empty() {
                format!(
                    "{} placed: {} pieces, as one set — drag it to move, the knob turns it",
                    scene.name,
                    p.ids.len()
                )
            } else {
                format!("{} placed without {left}", scene.name)
            });
        }
        Err(e) => d.ed.toast(e),
    }
}

/// What a placement did.
pub struct Placement {
    pub group: u64,
    pub ids: Vec<u64>,
    /// assets left out: (asset, why)
    pub skipped: Vec<(String, String)>,
}

impl Placement {
    /// "3 pieces from Skyfold Studio Collection (not owned)", or empty.
    pub fn left_out(&self) -> String {
        let mut why: Vec<(String, usize)> = vec![];
        for (_, w) in &self.skipped {
            match why.iter_mut().find(|x| &x.0 == w) {
                Some(x) => x.1 += 1,
                None => why.push((w.clone(), 1)),
            }
        }
        why.iter()
            .map(|(w, n)| format!("{n} piece{} {w}", if *n == 1 { "" } else { "s" }))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Places `scene` with its middle at `at`, turned by `yaw` (radians, as `Quat::from_rotation_y`),
/// as one undo step and one set.
pub fn place(
    d: &mut WorldData,
    scene: &SceneFile,
    at: Vec2,
    yaw: f32,
) -> Result<Placement, String> {
    if !d.terrain.in_bounds(at.x, at.y) {
        return Err(format!("[{:.0}, {:.0}] is outside the world", at.x, at.y));
    }
    // the scene's own models join this project's imports
    let installed = file::install_models(scene)?;
    let lib = d
        .lib
        .as_mut()
        .ok_or("the asset library is still loading; try again in a moment")?;
    let mut rename = std::collections::HashMap::new();
    for (old, imp) in &installed {
        if lib.get(&imp.id).is_none() {
            lib.add(crate::io::imported_entry(imp));
        }
        rename.insert(old.clone(), imp.id.clone());
    }
    let market = crate::market::snapshot();
    let q = Quat::from_rotation_y(yaw);
    // (a click's stroke may already hold the open undo step: it takes the name)
    d.history.begin(format!("Place {}", scene.name));
    d.history.set_label(format!("Place {}", scene.name));
    let group = d.store.new_id();
    let mut ids = vec![];
    let mut above = vec![];
    let mut skipped = vec![];
    let lib = d
        .lib
        .as_deref()
        .ok_or("the asset library is still loading")?;
    for pc in &scene.pieces {
        let asset = rename
            .get(&pc.asset)
            .cloned()
            .unwrap_or_else(|| pc.asset.clone());
        let Some(e) = lib.get(&asset) else {
            skipped.push((asset, "missing from this project".to_string()));
            continue;
        };
        if !market.usable(&e.id) {
            let pack = market
                .pack_of(&e.id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "a pack".into());
            skipped.push((asset, format!("from {pack} (not owned)")));
            continue;
        }
        let r = q * Vec3::new(pc.at[0], 0.0, pc.at[2]);
        let (x, z) = (at.x + r.x, at.y + r.z);
        if !d.terrain.in_bounds(x, z) {
            skipped.push((asset, "past the world's edge".to_string()));
            continue;
        }
        let id = d.store.new_id();
        d.history.touch_object(&d.store, id);
        let rot = (q * Quat::from_array(pc.rot)).normalize();
        d.store.insert(crate::objects::Placed {
            id,
            asset,
            variant: pc.variant.min(e.meshes.len().max(1) as u32 - 1),
            pos: [x, d.terrain.height_at(x, z) + pc.at[1], z],
            rot: rot.to_array(),
            scale: pc.scale,
            stretch: pc.stretch,
            group,
            locked: false,
        });
        ids.push(id);
        above.push(pc.at[1]);
    }
    if ids.is_empty() {
        d.history.commit(&d.store);
        return Err(format!("Nothing of {} could be placed here", scene.name));
    }
    // buildings level their ground; everything else then sits on the ground as it is now
    {
        let WorldData {
            terrain,
            history,
            store,
            lib,
            water,
            fit,
            ..
        } = &mut *d;
        if let Some(lib) = lib.as_deref() {
            let mut w = crate::placement::FitWorld {
                terrain,
                history,
                store,
                lib,
                water,
                found: &mut fit.found,
                bounds: &fit.bounds,
            };
            crate::placement::fit::fit_objects(&mut w, &ids, &[]);
        }
    }
    for (id, up) in ids.iter().zip(&above) {
        if d.fit.found.pads.contains_key(id) {
            continue;
        }
        if let Some(mut p) = d.store.items.get(id).cloned() {
            p.pos[1] = d.terrain.height_at(p.pos[0], p.pos[2]) + up;
            d.store.move_to(p);
        }
    }
    // nature keeps out of it
    let outline: Vec<Vec2> = if scene.outline.len() >= 3 {
        scene
            .outline
            .iter()
            .map(|p| at + (q * Vec3::new(p[0], 0.0, p[1])).xz())
            .collect()
    } else {
        let pts: Vec<Vec2> = ids
            .iter()
            .filter_map(|id| d.store.items.get(id))
            .map(|p| Vec2::new(p.pos[0], p.pos[2]))
            .collect();
        let (lo, hi) = pts.iter().fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        );
        let (lo, hi) = (lo - 2.0, hi + 2.0);
        vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)]
    };
    let (lo, hi) = outline.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(*p), hi.max(*p)),
    );
    let poly = outline.clone();
    crate::nature::paint_region(
        &mut d.nature,
        &mut d.history,
        "Scene",
        lo,
        hi,
        move |p| crate::setpieces::poly_sdf(&poly, p) < 0.5,
        &crate::nature::field::Target::All,
        crate::nature::AreaOp::Keep(0.0),
    );
    d.history.commit(&d.store);
    d.store.selected = ids.first().copied();
    d.meta.dirty = true;
    Ok(Placement {
        group,
        ids,
        skipped,
    })
}

/// Placing: the scene's outline follows the cursor over the ground, turned as it will land.
fn cursor(
    scenes: Res<Scenes>,
    ed: Res<crate::editor::Editor>,
    pointer: Res<crate::camera::Pointer>,
    stroke: Res<crate::tools::Stroke>,
    terrain: Res<crate::terrain::Terrain>,
    mut gizmos: Gizmos,
) {
    let Some(id) = scenes.placing() else { return };
    if !active(&scenes, &ed) || pointer.over_ui || stroke.active {
        return;
    }
    let (Some(hit), Some(scene)) = (pointer.hit, scenes.get(id)) else {
        return;
    };
    let q = Quat::from_rotation_y(scenes.yaw);
    let on_ground = |p: Vec2| {
        let w = hit.xz() + (q * Vec3::new(p.x, 0.0, p.y)).xz();
        Vec3::new(w.x, terrain.height_at(w.x, w.y) + 0.3, w.y)
    };
    let gold = Color::srgba(1.0, 0.85, 0.4, 0.95);
    if scene.outline.len() >= 3 {
        // the loop, densified so it hugs the ground
        let mut pts = vec![];
        let n = scene.outline.len();
        for i in 0..=n {
            let (a, b) = (
                Vec2::from_array(scene.outline[i % n]),
                Vec2::from_array(scene.outline[(i + 1) % n]),
            );
            let steps = ((a.distance(b) / 2.0).ceil() as usize).max(1);
            for k in 0..steps {
                pts.push(on_ground(a.lerp(b, k as f32 / steps as f32)));
            }
        }
        gizmos.linestrip(pts, gold);
    }
    // each piece's spot, and which way the scene faces
    for pc in scene.pieces.iter().take(400) {
        gizmos.circle(
            Isometry3d::new(
                on_ground(Vec2::new(pc.at[0], pc.at[2])),
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ),
            0.35,
            gold.with_alpha(0.7),
        );
    }
    let size = scene.size();
    gizmos.arrow(
        on_ground(Vec2::ZERO),
        on_ground(Vec2::new(0.0, size.y * 0.5 + 2.0)),
        gold,
    );
}

/// Writes requested `.glb` exports (they read the whole world: models, materials).
fn export_requests(world: &mut World) {
    let Some((id, path)) = world.resource_mut::<Scenes>().export_glb.take() else {
        return;
    };
    let msg = match file::read(&id) {
        Some(scene) => match crate::export::scene_glb(world, &scene, &path) {
            Ok(summary) => format!("Exported {} to {} ({summary})", scene.name, path.display()),
            Err(e) => format!("Couldn't export {}: {e}", scene.name),
        },
        None => "That scene is gone".to_string(),
    };
    world.resource_mut::<crate::editor::Editor>().toast(msg);
}
