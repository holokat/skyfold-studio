//! Roads and paths: splines through control points with a surface kind and width. The road tool
//! builds them (terrain bed, surface, cleared vegetation, bridges, props); the pacing analysis,
//! the world map and the exporters read their centrelines (`Road::centreline`), the ribbon mesh
//! (`mesh::road_ribbon_mesh`) and the road mask (`mesh::road_mask`).
//!
//! - `plan`: dense centreline samples, the smoothed longitudinal profile and bridge spans.
//! - `ground`: the ground under the roads before they were built (heights and paint), so a road
//!   can move or go away without leaving a trace; carving the bed and painting the shoulders.
//! - `build`: edit operations (create / update / remove) with vegetation clearing, lamp posts and
//!   undo, shared by the tools, the inspector and MCP.
//! - `route`: A* auto routing over the terrain (grade, curvature, water crossings).
//! - `mesh` / `render` / `bridge`: the surface ribbon, its material and the procedural bridges.
//! - `tool`: the Roads tab tools and their viewport overlay; `mcp`: the MCP tools.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::objects::Placed;
use crate::terrain::layers::{Layer, Surface};

mod autotest;
pub mod bridge;
pub mod build;
pub mod ground;
pub mod mcp;
pub mod mesh;
pub mod plan;
pub mod render;
pub mod route;
pub mod tool;

pub use tool::{RoadTool, RoadToolMode};

#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct Roads {
    #[serde(default)]
    pub roads: Vec<Road>,
    #[serde(default)]
    pub next_id: u32,
    /// The ground under the roads before they were built.
    #[serde(default)]
    pub ground: ground::GroundStore,
    /// Meshes / terrain need rebuilding.
    #[serde(skip)]
    pub dirty: bool,
    #[serde(skip)]
    pub selected: Option<u32>,
    /// The terrain was regenerated: rebuild every road on the new ground (see `build::regrade`).
    #[serde(skip)]
    pub regrade: bool,
}

impl Roads {
    pub fn alloc_id(&mut self) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn get(&self, id: u32) -> Option<&Road> {
        self.roads.iter().find(|r| r.id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoadKind {
    Trail,
    Footpath,
    DirtRoad,
    Gravel,
    Cobblestone,
    PavedStone,
}

/// How a kind of road is built and looks.
pub struct KindStyle {
    /// Terrain-library material of the surface (`assets/textures/terrain_library/<id>`).
    pub texture: &'static str,
    pub blurb: &'static str,
    /// Paint layer under the surface (seen from afar; no grass grows on it) and on the worn
    /// verges beside it.
    pub under: Layer,
    pub shoulder: Layer,
    /// How far past the edge the under layer reaches (m, at least a third of a terrain cell) and
    /// how strongly it is painted: built roads clear the grass completely, paths let some grow.
    pub under_reach: f32,
    pub under_paint: f32,
    /// Worn shoulder band beside the surface (m) and how strongly it is painted.
    pub shoulder_w: f32,
    pub shoulder_paint: f32,
    /// Steepest grade the auto router aims for, and on bridge approaches.
    pub max_grade: f32,
    /// Profile smoothing (m): how far bumps are levelled out along the road.
    pub smooth: f32,
    /// Deepest cut / highest fill the bed makes to keep the profile smooth (m).
    pub cut: f32,
    /// Soft edge width (m) and how ragged the edge is (m).
    pub edge: f32,
    pub breakup: f32,
    /// Cart ruts worn into the surface (0..1).
    pub ruts: f32,
    /// Footstep sound of the surface.
    pub surface: Surface,
    /// Stone arch bridges (else timber trestles).
    pub stone_bridge: bool,
    /// Lamp post spacing along the road (m, 0: none).
    pub lamps: f32,
}

impl RoadKind {
    pub const ALL: [RoadKind; 6] = [
        RoadKind::Trail,
        RoadKind::Footpath,
        RoadKind::DirtRoad,
        RoadKind::Gravel,
        RoadKind::Cobblestone,
        RoadKind::PavedStone,
    ];
    pub fn name(self) -> &'static str {
        match self {
            RoadKind::Trail => "Forest Trail",
            RoadKind::Footpath => "Footpath",
            RoadKind::DirtRoad => "Dirt Road",
            RoadKind::Gravel => "Gravel Road",
            RoadKind::Cobblestone => "Cobblestone Road",
            RoadKind::PavedStone => "Paved Highway",
        }
    }
    /// Short label for tiles.
    pub fn short(self) -> &'static str {
        match self {
            RoadKind::Trail => "Trail",
            RoadKind::Footpath => "Footpath",
            RoadKind::DirtRoad => "Dirt",
            RoadKind::Gravel => "Gravel",
            RoadKind::Cobblestone => "Cobble",
            RoadKind::PavedStone => "Highway",
        }
    }
    /// Typical width (m).
    pub fn default_width(self) -> f32 {
        match self {
            RoadKind::Trail => 1.6,
            RoadKind::Footpath => 2.5,
            RoadKind::DirtRoad => 4.5,
            RoadKind::Gravel => 5.0,
            RoadKind::Cobblestone => 6.0,
            RoadKind::PavedStone => 8.0,
        }
    }
    /// Travel speed on this surface relative to open ground (pacing analysis, mounts).
    pub fn travel_factor(self) -> f32 {
        match self {
            RoadKind::Trail => 1.05,
            RoadKind::Footpath => 1.1,
            RoadKind::DirtRoad => 1.2,
            RoadKind::Gravel => 1.25,
            RoadKind::Cobblestone => 1.3,
            RoadKind::PavedStone => 1.35,
        }
    }
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }
    pub fn style(self) -> &'static KindStyle {
        &STYLES[self.index()]
    }
    /// Parses a kind from its name, variant or a prefix of either ("cobble", "Paved Highway",
    /// "dirt_road", "paved"…).
    pub fn parse(s: &str) -> Option<RoadKind> {
        let key: String = s
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        if key.len() < 4 {
            return None;
        }
        Self::ALL.into_iter().find(|k| {
            let n: String = k
                .name()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            let v = format!("{k:?}").to_lowercase();
            [n, v, k.short().to_lowercase()]
                .iter()
                .any(|c| c.starts_with(&key))
        })
    }
}

const STYLES: [KindStyle; 6] = [
    KindStyle {
        texture: "compact_soil",
        blurb: "Narrow packed-earth trail through woods and hills.",
        under: Layer::ForestFloor,
        shoulder: Layer::ForestFloor,
        under_reach: 0.0,
        under_paint: 1.0,
        shoulder_w: 1.4,
        shoulder_paint: 0.4,
        max_grade: 0.22,
        smooth: 3.0,
        cut: 0.8,
        edge: 0.55,
        breakup: 0.45,
        ruts: 0.0,
        surface: Surface::Dirt,
        stone_bridge: false,
        lamps: 0.0,
    },
    KindStyle {
        texture: "brown_dirt",
        blurb: "Worn footpath across meadows, between farms.",
        under: Layer::Dirt,
        shoulder: Layer::Dirt,
        under_reach: -0.4,
        under_paint: 0.75,
        shoulder_w: 1.0,
        shoulder_paint: 0.25,
        max_grade: 0.18,
        smooth: 4.0,
        cut: 1.0,
        edge: 0.5,
        breakup: 0.4,
        ruts: 0.0,
        surface: Surface::Dirt,
        stone_bridge: false,
        lamps: 0.0,
    },
    KindStyle {
        texture: "stony_trail",
        blurb: "Compacted dirt and stones with cart ruts.",
        under: Layer::Dirt,
        shoulder: Layer::Dirt,
        under_reach: 0.8,
        under_paint: 1.0,
        shoulder_w: 1.6,
        shoulder_paint: 0.45,
        max_grade: 0.14,
        smooth: 7.0,
        cut: 2.0,
        edge: 0.5,
        breakup: 0.35,
        ruts: 1.0,
        surface: Surface::Dirt,
        stone_bridge: false,
        lamps: 0.0,
    },
    KindStyle {
        texture: "gravel_path",
        blurb: "Crushed-stone road with gravel spilling at the edges.",
        under: Layer::Dirt,
        shoulder: Layer::Dirt,
        under_reach: 1.0,
        under_paint: 1.0,
        shoulder_w: 1.6,
        shoulder_paint: 0.45,
        max_grade: 0.12,
        smooth: 8.0,
        cut: 2.5,
        edge: 0.4,
        breakup: 0.3,
        ruts: 0.45,
        surface: Surface::Gravel,
        stone_bridge: false,
        lamps: 0.0,
    },
    KindStyle {
        texture: "medieval_muddy_cobbles",
        blurb: "Old cobbled road between towns, lamp posts along it.",
        under: Layer::Gravel,
        shoulder: Layer::Gravel,
        under_reach: 1.2,
        under_paint: 1.0,
        shoulder_w: 1.2,
        shoulder_paint: 0.45,
        max_grade: 0.10,
        smooth: 9.0,
        cut: 3.0,
        edge: 0.18,
        breakup: 0.14,
        ruts: 0.0,
        surface: Surface::Cobble,
        stone_bridge: true,
        lamps: 32.0,
    },
    KindStyle {
        texture: "stone_road_pavers",
        blurb: "Wide highway of laid stone pavers with lamp posts.",
        under: Layer::Gravel,
        shoulder: Layer::Gravel,
        under_reach: 1.2,
        under_paint: 1.0,
        shoulder_w: 1.4,
        shoulder_paint: 0.5,
        max_grade: 0.08,
        smooth: 12.0,
        cut: 4.0,
        edge: 0.16,
        breakup: 0.12,
        ruts: 0.0,
        surface: Surface::Stone,
        stone_bridge: true,
        lamps: 28.0,
    },
];

/// Per-road build options (all on by default).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoadOptions {
    /// Carve / fill the terrain into a smooth bed.
    pub bed: bool,
    /// Remove trees, plants and rocks in the way.
    pub clear: bool,
    /// Bridge rivers, lakes and straits.
    pub bridges: bool,
    /// Lamp posts along paved and cobbled roads.
    pub props: bool,
}

impl Default for RoadOptions {
    fn default() -> Self {
        Self {
            bed: true,
            clear: true,
            bridges: true,
            props: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Road {
    pub id: u32,
    pub name: String,
    pub kind: RoadKind,
    /// Control points in world space. Ordinary roads pass through every one using centripetal
    /// Catmull–Rom; authored routes use their y values as a linear deck profile.
    pub points: Vec<[f32; 3]>,
    pub width: f32,
    #[serde(default)]
    pub options: RoadOptions,
    /// An imported route whose control-point heights are the intended deck profile. Ordinary
    /// editor roads leave this absent and continue to follow the terrain profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authored: Option<AuthoredRoute>,
    /// Vegetation this road cleared (put back when the road moves away or is removed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared: Vec<Placed>,
    /// Lamp posts this road placed (replaced when it is rebuilt).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prop_ids: Vec<u64>,
}

impl Road {
    /// A road with no name yet (creating it names it after its kind and id).
    pub fn new(
        id: u32,
        kind: RoadKind,
        points: Vec<[f32; 3]>,
        width: f32,
        options: RoadOptions,
    ) -> Road {
        Road {
            id,
            name: String::new(),
            kind,
            points,
            width,
            options,
            authored: None,
            cleared: vec![],
            prop_ids: vec![],
        }
    }

    /// The centreline sampled every `step` metres (xz arc length), including both ends.
    /// y is interpolated from the control points; callers that need the ground use the terrain.
    pub fn centreline(&self, step: f32) -> Vec<Vec3> {
        let pts: Vec<Vec3> = self.points.iter().map(|p| Vec3::from_array(*p)).collect();
        if self.authored.is_some() {
            if !self.has_valid_authored_route() {
                return Vec::new();
            }
            return linear_centreline(&pts, step);
        }
        match pts.len() {
            0 => return Vec::new(),
            1 => return pts,
            _ => {}
        }
        let step = step.max(0.05);
        let mut out = vec![pts[0]];
        let n = pts.len();
        for i in 0..n - 1 {
            let p0 = if i == 0 {
                pts[0] * 2.0 - pts[1]
            } else {
                pts[i - 1]
            };
            let (p1, p2) = (pts[i], pts[i + 1]);
            let p3 = if i + 2 < n {
                pts[i + 2]
            } else {
                pts[n - 1] * 2.0 - pts[n - 2]
            };
            let seg = p1.xz().distance(p2.xz()).max(1e-3);
            let k = (seg / step).ceil().max(1.0) as usize;
            for j in 1..=k {
                out.push(catmull_rom(p0, p1, p2, p3, j as f32 / k as f32));
            }
        }
        out
    }

    /// Length along the ground plane (m).
    pub fn length(&self) -> f32 {
        self.centreline(2.0)
            .windows(2)
            .map(|w| w[0].xz().distance(w[1].xz()))
            .sum()
    }

    /// Carving / overlap priority: wider, finer roads win where roads meet.
    pub fn rank(&self) -> (usize, u32, u32) {
        (
            self.kind.index(),
            (self.width * 100.0) as u32,
            u32::MAX - self.id,
        )
    }

    /// Everything the built road depends on (control points, kind, width, options).
    pub fn def_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.kind.hash(&mut h);
        self.width.to_bits().hash(&mut h);
        for p in &self.points {
            p[0].to_bits().hash(&mut h);
            p[1].to_bits().hash(&mut h);
            p[2].to_bits().hash(&mut h);
        }
        (
            self.options.bed,
            self.options.clear,
            self.options.bridges,
            self.options.props,
        )
            .hash(&mut h);
        if let Some(authored) = &self.authored {
            true.hash(&mut h);
            for span in &authored.spans {
                span.start.to_bits().hash(&mut h);
                span.end.to_bits().hash(&mut h);
                span.support_y.to_bits().hash(&mut h);
                span.kind.hash(&mut h);
            }
        } else {
            false.hash(&mut h);
        }
        h.finish()
    }

    /// Imported deck data is deliberately strict: malformed input produces no route rather than
    /// letting non-finite coordinates reach mesh or terrain code.
    pub fn has_valid_authored_route(&self) -> bool {
        self.authored.is_some()
            && self.points.len() >= 2
            && self
                .points
                .iter()
                .all(|p| p.iter().copied().all(f32::is_finite))
            && self
                .points
                .windows(2)
                .any(|w| Vec2::new(w[0][0], w[0][2]).distance(Vec2::new(w[1][0], w[1][2])) > 1e-4)
    }
}

/// Explicit bridge/profile information imported with a road. The profile itself is held in
/// `Road::points`: `[x, deck_y, z]`, sampled as a linear polyline instead of a spline.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthoredRoute {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<AuthoredSpan>,
}

/// A structural bridge section on an [`AuthoredRoute`]. `start` and `end` are normalized XZ
/// arc-length positions in `0..=1`; `support_y` is the finite water surface or chasm datum used
/// by the bridge supports. Invalid imported spans are ignored by the planner.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthoredSpan {
    pub start: f32,
    pub end: f32,
    pub support_y: f32,
    #[serde(default)]
    pub kind: AuthoredSpanKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthoredSpanKind {
    #[default]
    Water,
    Chasm,
}

/// Samples an authored deck as straight segments. Heights interpolate only along each authored
/// segment, so terrain relief cannot bend an imported bridge profile.
fn linear_centreline(pts: &[Vec3], step: f32) -> Vec<Vec3> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let step = step.max(0.05);
    let mut out = vec![pts[0]];
    for pair in pts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let len = a.xz().distance(b.xz());
        if len <= 1e-4 {
            continue;
        }
        let n = (len / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            out.push(a.lerp(b, i as f32 / n as f32));
        }
    }
    out
}

/// Centripetal Catmull–Rom (alpha 0.5): no cusps or self-intersections on uneven spacing.
fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let knot = |a: Vec3, b: Vec3| a.distance(b).max(1e-4).sqrt();
    let t0 = 0.0;
    let t1 = t0 + knot(p0, p1);
    let t2 = t1 + knot(p1, p2);
    let t3 = t2 + knot(p2, p3);
    let t = t1 + (t2 - t1) * t;
    let a1 = p0 * ((t1 - t) / (t1 - t0)) + p1 * ((t - t0) / (t1 - t0));
    let a2 = p1 * ((t2 - t) / (t2 - t1)) + p2 * ((t - t1) / (t2 - t1));
    let a3 = p2 * ((t3 - t) / (t3 - t2)) + p3 * ((t - t2) / (t3 - t2));
    let b1 = a1 * ((t2 - t) / (t2 - t0)) + a2 * ((t - t0) / (t2 - t0));
    let b2 = a2 * ((t3 - t) / (t3 - t1)) + a3 * ((t - t1) / (t3 - t1));
    b1 * ((t2 - t) / (t2 - t1)) + b2 * ((t - t1) / (t2 - t1))
}

pub struct RoadsPlugin;

impl Plugin for RoadsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Roads>();
        render::build(app);
        tool::build(app);
        autotest::build(app);
        mcp::register(app);
    }
}
