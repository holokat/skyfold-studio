//! The working parts of a dungeon:
//! - doors (single or double, in its doorways), portcullises, iron bars and secret doors that
//!   pass for wall;
//! - levers that work them, pressure plates and spike traps;
//! - markers the game reads: an arena, a checkpoint, a loot spot, a spawn point.
//!
//! They are built from the style's own wood, iron and stone, so there are no models to ship. The
//! template layouts put doors and gates in their doorways (`for_layout`); dungeons drawn from
//! scratch get none. Every piece is placed, moved, switched, linked or removed by hand or by MCP.
//! While walking, the interact key (E) opens a door or pulls a lever (and whatever it's linked
//! to); stepping on a pressure plate works its links. A closed door, gate or set of bars blocks
//! the way (`blockers`: the dungeon's space). Exported as nodes with their data for the game's
//! own logic.

use std::collections::{HashMap, HashSet};

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use serde::{Deserialize, Serialize};

use super::mesh::{Doorway, MeshData};
use super::model::DungeonMap;
use super::plan::Role;
use super::styles::Slot;
use crate::camera::MainCamera;

/// What a piece is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PieceKind {
    #[default]
    Door,
    DoubleDoor,
    Portcullis,
    Bars,
    SecretDoor,
    Lever,
    PressurePlate,
    SpikeTrap,
    Arena,
    Checkpoint,
    Loot,
    Spawn,
}

impl PieceKind {
    pub const ALL: [PieceKind; 12] = [
        PieceKind::Door,
        PieceKind::DoubleDoor,
        PieceKind::Portcullis,
        PieceKind::Bars,
        PieceKind::SecretDoor,
        PieceKind::Lever,
        PieceKind::PressurePlate,
        PieceKind::SpikeTrap,
        PieceKind::Arena,
        PieceKind::Checkpoint,
        PieceKind::Loot,
        PieceKind::Spawn,
    ];
    pub fn id(self) -> &'static str {
        match self {
            PieceKind::Door => "door",
            PieceKind::DoubleDoor => "double_door",
            PieceKind::Portcullis => "portcullis",
            PieceKind::Bars => "bars",
            PieceKind::SecretDoor => "secret_door",
            PieceKind::Lever => "lever",
            PieceKind::PressurePlate => "pressure_plate",
            PieceKind::SpikeTrap => "spike_trap",
            PieceKind::Arena => "arena",
            PieceKind::Checkpoint => "checkpoint",
            PieceKind::Loot => "loot",
            PieceKind::Spawn => "spawn",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            PieceKind::Door => "Door",
            PieceKind::DoubleDoor => "Double door",
            PieceKind::Portcullis => "Portcullis",
            PieceKind::Bars => "Bars",
            PieceKind::SecretDoor => "Secret door",
            PieceKind::Lever => "Lever",
            PieceKind::PressurePlate => "Pressure plate",
            PieceKind::SpikeTrap => "Spike trap",
            PieceKind::Arena => "Arena",
            PieceKind::Checkpoint => "Checkpoint",
            PieceKind::Loot => "Loot spot",
            PieceKind::Spawn => "Spawn point",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// Stands in a doorway (and fills it).
    pub fn in_doorway(self) -> bool {
        matches!(
            self,
            PieceKind::Door
                | PieceKind::DoubleDoor
                | PieceKind::Portcullis
                | PieceKind::Bars
                | PieceKind::SecretDoor
        )
    }
    /// Only a mark for the game (nothing to see in it).
    pub fn is_marker(self) -> bool {
        matches!(
            self,
            PieceKind::Arena | PieceKind::Checkpoint | PieceKind::Loot | PieceKind::Spawn
        )
    }
    /// The interact key works it.
    pub fn usable(self) -> bool {
        matches!(
            self,
            PieceKind::Door | PieceKind::DoubleDoor | PieceKind::Lever | PieceKind::SecretDoor
        )
    }
    /// Its usual size (width, height).
    pub fn size(self) -> (f32, f32) {
        match self {
            PieceKind::Door => (1.6, 2.6),
            PieceKind::DoubleDoor => (3.0, 3.2),
            PieceKind::Portcullis | PieceKind::Bars => (3.0, 3.4),
            PieceKind::SecretDoor => (1.6, 2.6),
            PieceKind::Lever => (0.4, 1.3),
            PieceKind::PressurePlate => (1.0, 0.06),
            PieceKind::SpikeTrap => (2.0, 0.5),
            PieceKind::Arena => (12.0, 0.0),
            PieceKind::Checkpoint | PieceKind::Loot | PieceKind::Spawn => (1.5, 0.0),
        }
    }
}

/// A piece: its foot's middle, which way it faces (yaw: 0 = +z; a door faces into the room it
/// opens into), its size, whether it starts open (a door, a gate; a lever pulled; a trap's spikes
/// up), what it works (levers, plates: other pieces' ids), and a label for the game.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Piece {
    pub id: u32,
    pub kind: PieceKind,
    pub pos: [f32; 3],
    #[serde(default)]
    pub yaw: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub links: Vec<u32>,
    /// placed by the template layout (replaced when it lays out again)
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub label: String,
}

impl Piece {
    pub fn new(kind: PieceKind, pos: Vec3, yaw: f32) -> Piece {
        let (width, height) = kind.size();
        Piece {
            id: 0,
            kind,
            pos: pos.to_array(),
            yaw,
            width,
            height,
            open: false,
            links: vec![],
            auto: false,
            label: String::new(),
        }
    }
    pub fn position(&self) -> Vec3 {
        Vec3::from_array(self.pos)
    }
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }
}

/// A piece standing in a doorway: its middle and facing (into the room), its width and height.
pub fn at_doorway(kind: PieceKind, d: &Doorway) -> Piece {
    let mid = (d.a + d.b) * 0.5;
    let yaw = d.inward.x.atan2(d.inward.y);
    let mut p = Piece::new(kind, Vec3::new(mid.x, d.floor, mid.y), yaw);
    p.width = d.a.distance(d.b).max(0.8);
    p.height = (d.top - d.floor).clamp(1.8, 8.0);
    p
}

/// The doorway nearest `p` (within `reach` m) of a built dungeon.
pub fn nearest_doorway(doorways: &[Doorway], p: Vec3, reach: f32) -> Option<&Doorway> {
    doorways
        .iter()
        .filter(|d| p.y > d.floor - 1.0 && p.y < d.top + 1.0)
        .map(|d| (((d.a + d.b) * 0.5).distance(Vec2::new(p.x, p.z)), d))
        .filter(|(dist, _)| *dist < reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|x| x.1)
}

/// The template layouts' doors and gates: a portcullis into the boss's room, bars into prison
/// cells, a stout door into treasure rooms and crypts, a door in some other doorways (double
/// ones in wide doorways). One per opening (a doorway is found from both sides).
pub fn for_layout(plan: &super::plan::Plan, doorways: &[Doorway], seed: u32) -> Vec<Piece> {
    let mut rng = crate::noise::Rng::new(seed as u64 ^ 0xd00f);
    let mut out: Vec<Piece> = Vec::new();
    for d in doorways {
        let mid = (d.a + d.b) * 0.5;
        if out
            .iter()
            .any(|p| Vec2::new(p.pos[0], p.pos[2]).distance(mid) < 1.5)
        {
            continue;
        }
        let width = d.a.distance(d.b);
        if width < 0.9 || width > 6.5 {
            continue;
        }
        let role = plan.room(d.room).map(|r| r.role).unwrap_or(Role::None);
        let kind = match role {
            Role::Boss | Role::Throne | Role::Arena => Some(PieceKind::Portcullis),
            Role::Prison => Some(PieceKind::Bars),
            Role::Treasure | Role::Crypt | Role::Library => Some(if width > 3.0 {
                PieceKind::DoubleDoor
            } else {
                PieceKind::Door
            }),
            Role::Entrance => None,
            _ if rng.f32() < 0.35 => Some(if width > 3.0 {
                PieceKind::DoubleDoor
            } else {
                PieceKind::Door
            }),
            _ => None,
        };
        let Some(kind) = kind else { continue };
        let mut p = at_doorway(kind, d);
        p.auto = true;
        // (bars into a cell stand shut; doors and gates start shut, the boss's gate too)
        p.open = false;
        out.push(p);
    }
    out
}

/// A map's doorways (builds it: tens of milliseconds).
pub fn doorways_of(m: &super::model::MapDoc) -> Vec<Doorway> {
    let c = super::render::compile(&m.plan, &m.style);
    let idx = super::render::style_index(&m.plan, &m.style);
    super::mesh::build(&c, &|r| idx.get(&r.id).copied().unwrap_or(0)).doorways
}

/// The template layout's doors and gates for a map (ids given), replacing the earlier ones the
/// layout placed (pieces placed by hand stay).
pub fn place_for_layout(m: &mut super::model::MapDoc) {
    m.pieces.retain(|p| !p.auto);
    if m.plan.rooms.len() < 2 {
        return;
    }
    for mut p in for_layout(&m.plan, &doorways_of(m), m.plan.seed) {
        p.id = m.alloc_piece();
        m.pieces.push(p);
    }
}

// ------------------------------------------------------------------ the walls they make

/// A closed door, gate or set of bars as a box the walker can't pass: middle, turn, half size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blocker {
    pub center: Vec3,
    pub rot: Quat,
    pub half: Vec3,
}

impl Blocker {
    /// Signed distance (negative inside).
    pub fn sdf(&self, p: Vec3) -> f32 {
        let q = (self.rot.inverse() * (p - self.center)).abs() - self.half;
        q.max(Vec3::ZERO).length() + q.max_element().min(0.0)
    }
}

/// What stands in the way now (`open`: the pieces open at the moment).
pub fn blockers(pieces: &[Piece], open: &HashMap<u32, bool>) -> Vec<Blocker> {
    pieces
        .iter()
        .filter(|p| {
            p.kind.in_doorway()
                && !(open.get(&p.id).copied().unwrap_or(p.open) && p.kind != PieceKind::Bars)
        })
        .map(|p| Blocker {
            center: p.position() + Vec3::Y * p.height * 0.5,
            rot: p.rotation(),
            half: Vec3::new(p.width * 0.5, p.height * 0.5, 0.12),
        })
        .collect()
}

// ------------------------------------------------------------------ their shapes

/// A box from `lo` to `hi` (local space), its faces textured in metres / `tile`.
fn block(m: &mut MeshData, lo: Vec3, hi: Vec3, tile: f32) {
    let c = [
        Vec3::new(lo.x, lo.y, lo.z),
        Vec3::new(hi.x, lo.y, lo.z),
        Vec3::new(lo.x, hi.y, lo.z),
        Vec3::new(hi.x, hi.y, lo.z),
        Vec3::new(lo.x, lo.y, hi.z),
        Vec3::new(hi.x, lo.y, hi.z),
        Vec3::new(lo.x, hi.y, hi.z),
        Vec3::new(hi.x, hi.y, hi.z),
    ];
    const FACES: [([usize; 4], [f32; 3]); 6] = [
        ([0, 4, 6, 2], [-1.0, 0.0, 0.0]),
        ([1, 3, 7, 5], [1.0, 0.0, 0.0]),
        ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
        ([2, 6, 7, 3], [0.0, 1.0, 0.0]),
        ([0, 2, 3, 1], [0.0, 0.0, -1.0]),
        ([4, 5, 7, 6], [0.0, 0.0, 1.0]),
    ];
    for (f, n) in FACES {
        let n = Vec3::from_array(n);
        let p = [c[f[0]], c[f[1]], c[f[2]], c[f[3]]];
        let uv = |v: Vec3| {
            if n.y.abs() > 0.5 {
                Vec2::new(v.x, v.z) / tile
            } else if n.x.abs() > 0.5 {
                Vec2::new(v.z, -v.y) / tile
            } else {
                Vec2::new(v.x, -v.y) / tile
            }
        };
        m.push_quad(p, n, [uv(p[0]), uv(p[1]), uv(p[2]), uv(p[3])]);
    }
}

/// A piece's moving parts: (mesh, material slot — wood / iron / stone, where it hinges or rests
/// shut, and how it moves open: turned about Y, or slid).
pub struct Part {
    pub mesh: MeshData,
    pub material: Mat,
    pub closed: Transform,
    pub open: Transform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mat {
    Wood,
    Iron,
    Stone,
}

/// A door leaf `w` wide and `h` tall, hinged at its local origin, running along +x: upright
/// planks with iron bands and a ring pull.
fn leaf(w: f32, h: f32, flip: bool) -> (MeshData, MeshData) {
    let (mut wood, mut iron) = (MeshData::default(), MeshData::default());
    let planks = ((w / 0.24).round() as usize).max(2);
    let pw = w / planks as f32;
    for k in 0..planks {
        let x0 = k as f32 * pw + 0.006;
        let jag = if k % 2 == 0 { 0.0 } else { -0.03 };
        block(
            &mut wood,
            Vec3::new(x0, 0.02, -0.05),
            Vec3::new(x0 + pw - 0.012, h + jag, 0.05),
            1.2,
        );
    }
    for y in [0.35, h - 0.45, h * 0.5] {
        block(
            &mut iron,
            Vec3::new(0.02, y, -0.065),
            Vec3::new(w - 0.02, y + 0.08, 0.065),
            1.0,
        );
    }
    let rx = if flip { 0.18 } else { w - 0.18 };
    block(
        &mut iron,
        Vec3::new(rx - 0.06, h * 0.45, 0.06),
        Vec3::new(rx + 0.06, h * 0.45 + 0.12, 0.1),
        1.0,
    );
    (wood, iron)
}

/// A piece's parts in its own space (x across, y up, z the way it faces).
pub fn parts(p: &Piece) -> Vec<Part> {
    let (w, h) = (p.width, p.height);
    let id = Transform::IDENTITY;
    match p.kind {
        PieceKind::Door => {
            let (wood, iron) = leaf(w, h, false);
            let hinge = Transform::from_xyz(-w * 0.5, 0.0, 0.0);
            let open = hinge.with_rotation(Quat::from_rotation_y(-1.4));
            vec![
                Part {
                    mesh: wood,
                    material: Mat::Wood,
                    closed: hinge,
                    open,
                },
                Part {
                    mesh: iron,
                    material: Mat::Iron,
                    closed: hinge,
                    open,
                },
            ]
        }
        PieceKind::DoubleDoor => {
            let half = w * 0.5;
            let (wa, ia) = leaf(half, h, false);
            let (wb, ib) = leaf(half, h, true);
            let ha = Transform::from_xyz(-w * 0.5, 0.0, 0.0);
            // (the right leaf: mirrored, hinged at the right jamb)
            let hb = Transform::from_xyz(w * 0.5, 0.0, 0.0)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
            let oa = ha.with_rotation(Quat::from_rotation_y(-1.4));
            let ob = Transform::from_xyz(w * 0.5, 0.0, 0.0)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::PI + 1.4));
            vec![
                Part {
                    mesh: wa,
                    material: Mat::Wood,
                    closed: ha,
                    open: oa,
                },
                Part {
                    mesh: ia,
                    material: Mat::Iron,
                    closed: ha,
                    open: oa,
                },
                Part {
                    mesh: wb,
                    material: Mat::Wood,
                    closed: hb,
                    open: ob,
                },
                Part {
                    mesh: ib,
                    material: Mat::Iron,
                    closed: hb,
                    open: ob,
                },
            ]
        }
        PieceKind::Portcullis | PieceKind::Bars => {
            let mut iron = MeshData::default();
            let n = ((w / 0.22).floor() as usize).max(2);
            for k in 0..=n {
                let x = -w * 0.5 + w * k as f32 / n as f32;
                block(
                    &mut iron,
                    Vec3::new(x - 0.035, 0.0, -0.035),
                    Vec3::new(x + 0.035, h, 0.035),
                    1.0,
                );
                if p.kind == PieceKind::Portcullis {
                    // spiked feet
                    block(
                        &mut iron,
                        Vec3::new(x - 0.015, -0.12, -0.015),
                        Vec3::new(x + 0.015, 0.0, 0.015),
                        1.0,
                    );
                }
            }
            let rows = if p.kind == PieceKind::Portcullis {
                ((h / 0.55).floor() as usize).max(2)
            } else {
                2
            };
            for k in 0..rows {
                let y = 0.25 + (h - 0.4) * k as f32 / (rows - 1).max(1) as f32;
                block(
                    &mut iron,
                    Vec3::new(-w * 0.5, y, -0.045),
                    Vec3::new(w * 0.5, y + 0.07, 0.045),
                    1.0,
                );
            }
            let open = if p.kind == PieceKind::Portcullis {
                Transform::from_xyz(0.0, h - 0.3, 0.0)
            } else {
                id
            };
            vec![Part {
                mesh: iron,
                material: Mat::Iron,
                closed: id,
                open,
            }]
        }
        PieceKind::SecretDoor => {
            let mut stone = MeshData::default();
            block(
                &mut stone,
                Vec3::new(-w * 0.5, 0.0, -0.15),
                Vec3::new(w * 0.5, h, 0.15),
                2.0,
            );
            vec![Part {
                mesh: stone,
                material: Mat::Stone,
                closed: id,
                open: Transform::from_xyz(w * 0.95, 0.0, -0.35),
            }]
        }
        PieceKind::Lever => {
            let mut iron = MeshData::default();
            let mut wood = MeshData::default();
            block(
                &mut iron,
                Vec3::new(-0.16, -0.22, -0.02),
                Vec3::new(0.16, 0.22, 0.06),
                1.0,
            );
            block(
                &mut wood,
                Vec3::new(-0.03, 0.0, 0.0),
                Vec3::new(0.03, 0.55, 0.06),
                1.0,
            );
            block(
                &mut iron,
                Vec3::new(-0.05, 0.5, -0.01),
                Vec3::new(0.05, 0.62, 0.08),
                1.0,
            );
            let pivot = Transform::from_xyz(0.0, h * 0.5, 0.05);
            let closed = pivot.with_rotation(Quat::from_rotation_x(0.65));
            let open = pivot.with_rotation(Quat::from_rotation_x(-0.65));
            vec![
                Part {
                    mesh: iron,
                    material: Mat::Iron,
                    closed: Transform::from_xyz(0.0, h * 0.5, 0.0),
                    open: Transform::from_xyz(0.0, h * 0.5, 0.0),
                },
                Part {
                    mesh: wood,
                    material: Mat::Wood,
                    closed,
                    open,
                },
            ]
        }
        PieceKind::PressurePlate => {
            let mut stone = MeshData::default();
            block(
                &mut stone,
                Vec3::new(-w * 0.5, 0.0, -w * 0.5),
                Vec3::new(w * 0.5, 0.06, w * 0.5),
                1.0,
            );
            vec![Part {
                mesh: stone,
                material: Mat::Stone,
                closed: id,
                open: Transform::from_xyz(0.0, -0.04, 0.0),
            }]
        }
        PieceKind::SpikeTrap => {
            let mut iron = MeshData::default();
            block(
                &mut iron,
                Vec3::new(-w * 0.5, 0.0, -w * 0.5),
                Vec3::new(w * 0.5, 0.03, w * 0.5),
                1.0,
            );
            let mut spikes = MeshData::default();
            let n = ((w / 0.3).floor() as usize).max(2);
            for i in 0..n {
                for j in 0..n {
                    let (x, z) = (
                        -w * 0.5 + (i as f32 + 0.5) * w / n as f32,
                        -w * 0.5 + (j as f32 + 0.5) * w / n as f32,
                    );
                    block(
                        &mut spikes,
                        Vec3::new(x - 0.025, -0.45, z - 0.025),
                        Vec3::new(x + 0.025, 0.0, z + 0.025),
                        1.0,
                    );
                }
            }
            vec![
                Part {
                    mesh: iron,
                    material: Mat::Iron,
                    closed: id,
                    open: id,
                },
                Part {
                    mesh: spikes,
                    material: Mat::Iron,
                    closed: id,
                    open: Transform::from_xyz(0.0, h, 0.0),
                },
            ]
        }
        _ => vec![],
    }
}

fn to_mesh(d: MeshData) -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, d.pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, d.nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, d.uv);
    m.insert_indices(Indices::U32(d.idx));
    let _ = m.generate_tangents();
    m
}

// ------------------------------------------------------------------ in the view

#[derive(Component)]
pub struct PiecePart {
    id: u32,
    closed: Transform,
    open: Transform,
    /// 0 shut … 1 open
    t: f32,
}

#[derive(Component)]
pub struct PieceRoot {
    id: u32,
}

/// The pieces as they stand while walking (opened, pulled, sprung), apart from how they were
/// placed (`Piece::open`): playing doesn't change the map.
#[derive(Resource, Default)]
pub struct PieceState {
    pub open: HashMap<u32, bool>,
    shown: HashMap<u32, (Entity, Piece)>,
    mats: HashMap<(u16, Mat), Handle<StandardMaterial>>,
    /// a plate stood on (works its links once per step on)
    pressed: HashSet<u32>,
    /// the piece the interact key acts on (and how far)
    pub focus: Option<(u32, f32)>,
}

impl PieceState {
    pub fn is_open(&self, p: &Piece) -> bool {
        self.open.get(&p.id).copied().unwrap_or(p.open)
    }
}

/// Builds and animates the pieces; walking resets their state to how they were placed.
#[allow(clippy::too_many_arguments)]
pub(super) fn sync(
    mut commands: Commands,
    mut st: ResMut<PieceState>,
    map: Res<DungeonMap>,
    walk: Res<crate::walk::WalkState>,
    mut was_walking: Local<bool>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    server: Res<AssetServer>,
    mut parts_q: Query<(&mut PiecePart, &mut Transform)>,
    time: Res<Time>,
) {
    let Some(m) = map
        .0
        .as_ref()
        .filter(|m| m.kind == super::model::MapKind::Interior)
    else {
        for (_, (e, _)) in st.shown.drain() {
            commands.entity(e).despawn();
        }
        st.open.clear();
        return;
    };
    if walk.active != *was_walking {
        *was_walking = walk.active;
        st.open.clear();
        st.pressed.clear();
    }
    if map.is_changed() {
        let want: HashMap<u32, &Piece> = m.pieces.iter().map(|p| (p.id, p)).collect();
        let stale: Vec<u32> = st
            .shown
            .iter()
            .filter(|(id, (_, p))| want.get(id).is_none_or(|w| *w != p))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some((e, _)) = st.shown.remove(&id) {
                commands.entity(e).despawn();
            }
        }
        let si = super::styles::index(&m.style);
        for p in &m.pieces {
            if st.shown.contains_key(&p.id) || p.kind.is_marker() {
                continue;
            }
            let open = st.is_open(p);
            let root = commands
                .spawn((
                    Name::new(format!("Dungeon {} {}", p.kind.id(), p.id)),
                    PieceRoot { id: p.id },
                    Transform::from_translation(p.position()).with_rotation(p.rotation()),
                    Visibility::default(),
                ))
                .id();
            for part in parts(p) {
                let mat = st
                    .mats
                    .entry((si as u16, part.material))
                    .or_insert_with(|| mats.add(material(si, part.material, &server)))
                    .clone();
                let t = if open { 1.0 } else { 0.0 };
                let tf = if open { part.open } else { part.closed };
                let child = commands
                    .spawn((
                        PiecePart {
                            id: p.id,
                            closed: part.closed,
                            open: part.open,
                            t,
                        },
                        Mesh3d(meshes.add(to_mesh(part.mesh))),
                        MeshMaterial3d(mat),
                        tf,
                    ))
                    .id();
                commands.entity(root).add_child(child);
            }
            st.shown.insert(p.id, (root, p.clone()));
        }
    }
    // swing toward open or shut
    let dt = time.delta_secs().min(0.1);
    let by: HashMap<u32, &Piece> = m.pieces.iter().map(|p| (p.id, p)).collect();
    for (mut part, mut tf) in &mut parts_q {
        let Some(p) = by.get(&part.id) else { continue };
        let target = if st.is_open(p) { 1.0 } else { 0.0 };
        if (part.t - target).abs() < 1e-4 {
            continue;
        }
        let speed = if p.kind == PieceKind::Portcullis {
            0.45
        } else {
            1.1
        };
        part.t += (target - part.t).clamp(-dt * speed, dt * speed);
        let k = part.t * part.t * (3.0 - 2.0 * part.t);
        *tf = Transform {
            translation: part.closed.translation.lerp(part.open.translation, k),
            rotation: part.closed.rotation.slerp(part.open.rotation, k),
            scale: Vec3::ONE,
        };
    }
}

fn material(si: usize, m: Mat, server: &AssetServer) -> StandardMaterial {
    let st = &super::styles::all()[si.min(super::styles::all().len() - 1)];
    let tex = |slot: Slot| {
        let (a, n, _) = st.mat(slot).tex.paths();
        (server.load(a), n.map(|p| server.load(p)))
    };
    match m {
        Mat::Wood => {
            let (a, n) = tex(Slot::Beam);
            StandardMaterial {
                base_color_texture: Some(a),
                normal_map_texture: n,
                perceptual_roughness: 0.85,
                ..default()
            }
        }
        Mat::Stone => {
            let (a, n) = tex(Slot::Wall);
            StandardMaterial {
                base_color_texture: Some(a),
                normal_map_texture: n,
                perceptual_roughness: 0.9,
                ..default()
            }
        }
        Mat::Iron => StandardMaterial {
            base_color: Color::srgb(0.16, 0.15, 0.14),
            metallic: 0.85,
            perceptual_roughness: 0.5,
            ..default()
        },
    }
}

/// Toggles a piece and, for a lever or a plate, whatever it works (`map`'s pieces).
pub fn work(st: &mut PieceState, pieces: &[Piece], id: u32) {
    let Some(p) = pieces.iter().find(|p| p.id == id) else {
        return;
    };
    let now = !st.is_open(p);
    st.open.insert(id, now);
    if matches!(p.kind, PieceKind::Lever | PieceKind::PressurePlate) {
        for l in &p.links {
            if let Some(q) = pieces.iter().find(|q| q.id == *l) {
                let o = !st.is_open(q);
                st.open.insert(*l, o);
            }
        }
    }
}

/// Walking: what the interact key (E) works (the usable piece looked at within reach), E itself,
/// and pressure plates underfoot.
#[allow(clippy::too_many_arguments)]
pub(super) fn interact(
    mut st: ResMut<PieceState>,
    map: Res<DungeonMap>,
    walk: Res<crate::walk::WalkState>,
    keys: Res<ButtonInput<KeyCode>>,
    pointer: Res<crate::camera::Pointer>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
) {
    st.focus = None;
    let Some(m) = map.0.as_ref() else { return };
    if !walk.active {
        return;
    }
    let Ok(c) = cam.single() else { return };
    let (eye, fwd) = (c.translation(), c.forward().as_vec3());
    st.focus = m
        .pieces
        .iter()
        .filter(|p| p.kind.usable())
        .map(|p| {
            let centre = p.position() + Vec3::Y * (p.height * 0.5).min(1.2);
            (
                p.id,
                centre.distance(eye),
                (centre - eye).normalize_or_zero().dot(fwd),
            )
        })
        .filter(|(_, d, look)| *d < 2.8 && *look > 0.55)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, d, _)| (id, d));
    if let Some((id, _)) = st.focus
        && !pointer.keyboard_captured
        && keys.just_pressed(KeyCode::KeyE)
        && !keys.any_pressed([
            KeyCode::SuperLeft,
            KeyCode::SuperRight,
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            KeyCode::AltLeft,
            KeyCode::AltRight,
        ])
    {
        work(&mut st, &m.pieces, id);
    }
    // pressure plates underfoot
    let feet = walk.position;
    let on: Vec<u32> = m
        .pieces
        .iter()
        .filter(|p| {
            p.kind == PieceKind::PressurePlate
                && Vec2::new(p.pos[0] - feet.x, p.pos[2] - feet.z)
                    .abs()
                    .max_element()
                    < p.width * 0.5
                && (feet.y - p.pos[1]).abs() < 0.6
        })
        .map(|p| p.id)
        .collect();
    for id in &on {
        if !st.pressed.contains(id) {
            work(&mut st, &m.pieces, *id);
        }
    }
    st.pressed = on.into_iter().collect();
}

/// The hint by the crosshair: E and what it does.
pub(super) fn hint(
    mut contexts: EguiContexts,
    st: Res<PieceState>,
    map: Res<DungeonMap>,
    walk: Res<crate::walk::WalkState>,
) -> Result {
    use egui::{Align2, Color32, CornerRadius, FontId, Id, Order, Stroke, pos2, vec2};
    if !walk.active || walk.cinematic {
        return Ok(());
    }
    let Some((id, _)) = st.focus else {
        return Ok(());
    };
    let Some(p) = map
        .0
        .as_ref()
        .and_then(|m| m.pieces.iter().find(|p| p.id == id))
    else {
        return Ok(());
    };
    let open = st.is_open(p);
    let what = match p.kind {
        PieceKind::Lever => if open {
            "Push the lever back"
        } else {
            "Pull the lever"
        }
        .to_string(),
        PieceKind::SecretDoor => if open {
            "Close the passage"
        } else {
            "Push the loose stone"
        }
        .to_string(),
        k => format!(
            "{} the {}",
            if open { "Close" } else { "Open" },
            k.name().to_lowercase()
        ),
    };
    let ctx = contexts.ctx_mut()?.clone();
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        Order::Foreground,
        Id::new("dungeon_piece_hint"),
    ));
    let c = screen.center() + vec2(0.0, 58.0);
    let label = painter.layout_no_wrap(
        what,
        FontId::new(14.0, crate::ui::theme::semibold()),
        Color32::WHITE,
    );
    let key = 22.0;
    let w = key + 10.0 + label.size().x;
    let r = egui::Rect::from_center_size(c, vec2(w + 24.0, 34.0));
    painter.rect_filled(
        r,
        CornerRadius::same(9),
        Color32::from_rgba_unmultiplied(10, 14, 20, 200),
    );
    let kr = egui::Rect::from_min_size(pos2(r.min.x + 12.0, c.y - key / 2.0), vec2(key, key));
    painter.rect_filled(kr, CornerRadius::same(5), Color32::from_rgb(255, 214, 120));
    painter.rect_stroke(
        kr,
        CornerRadius::same(5),
        Stroke::new(1.0, Color32::from_rgb(140, 100, 40)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        kr.center(),
        Align2::CENTER_CENTER,
        "E",
        FontId::new(13.0, crate::ui::theme::semibold()),
        Color32::from_rgb(40, 28, 10),
    );
    painter.galley(
        pos2(kr.max.x + 10.0, c.y - label.size().y / 2.0),
        label,
        Color32::WHITE,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeons::model::{MapDoc, MapKind};
    use crate::dungeons::presets;

    fn map(preset: &str, seed: u32) -> MapDoc {
        let p = presets::get(preset).unwrap();
        let mut m = MapDoc::new(1, "Test", MapKind::Interior);
        m.plan = presets::generate(
            p,
            presets::Size::Small,
            presets::RoomSize::Normal,
            0.0,
            seed,
            p.style,
        );
        m.style = p.style.to_string();
        m
    }

    #[test]
    fn template_layouts_put_doors_and_gates_in_doorways_one_each() {
        let mut total = 0;
        for (preset, seed) in [("crypt", 3), ("ruined_fortress", 5), ("prison_keep", 9)] {
            let Some(_) = presets::get(preset) else {
                continue;
            };
            let mut m = map(preset, seed);
            place_for_layout(&mut m);
            let ways = doorways_of(&m);
            for p in &m.pieces {
                assert!(p.auto && p.kind.in_doorway(), "{preset}: {p:?}");
                let near = ways
                    .iter()
                    .map(|d| ((d.a + d.b) * 0.5).distance(Vec2::new(p.pos[0], p.pos[2])))
                    .fold(f32::MAX, f32::min);
                assert!(
                    near < 0.05,
                    "{preset}: piece {} is {near} m from a doorway",
                    p.id
                );
                assert!(
                    m.pieces
                        .iter()
                        .filter(|q| q.position().distance(p.position()) < 1.0)
                        .count()
                        == 1,
                    "{preset}: two pieces in one doorway"
                );
            }
            // ids are unique and a second layout replaces the first
            let ids: HashSet<u32> = m.pieces.iter().map(|p| p.id).collect();
            assert_eq!(ids.len(), m.pieces.len());
            let n = m.pieces.len();
            place_for_layout(&mut m);
            assert_eq!(
                m.pieces.len(),
                n,
                "{preset}: laying out again doubled the pieces"
            );
            total += n;
        }
        assert!(total > 0, "no template doors at all");
    }

    #[test]
    fn a_shut_door_blocks_the_way_an_open_one_doesnt_and_bars_always_do() {
        let mut door = Piece::new(PieceKind::Door, Vec3::new(2.0, 0.0, 0.0), 0.0);
        door.id = 1;
        let mut bars = Piece::new(PieceKind::Bars, Vec3::new(-2.0, 0.0, 0.0), 0.0);
        bars.id = 2;
        bars.open = true;
        let mut lever = Piece::new(PieceKind::Lever, Vec3::new(0.0, 0.0, 3.0), 0.0);
        lever.id = 3;
        lever.links = vec![1];
        let pieces = vec![door.clone(), bars, lever];
        let shut = blockers(&pieces, &HashMap::new());
        assert_eq!(
            shut.len(),
            2,
            "the shut door and the bars block (not the lever)"
        );
        assert!(
            shut[0].sdf(Vec3::new(2.0, 1.2, 0.0)) < 0.0,
            "in the doorway is inside the door"
        );
        assert!(
            shut[0].sdf(Vec3::new(2.0, 1.2, 1.0)) > 0.5,
            "a metre in front is clear"
        );
        // pulling the lever opens the door
        let mut st = PieceState::default();
        work(&mut st, &pieces, 3);
        assert!(st.is_open(&door));
        assert_eq!(
            blockers(&pieces, &st.open).len(),
            1,
            "only the bars block now"
        );
    }

    #[test]
    fn every_piece_with_a_shape_has_parts_with_triangles() {
        for k in PieceKind::ALL {
            let p = Piece::new(k, Vec3::ZERO, 0.0);
            let parts = parts(&p);
            assert_eq!(parts.is_empty(), k.is_marker(), "{k:?}");
            for part in parts {
                assert!(
                    !part.mesh.idx.is_empty() && part.mesh.idx.len() % 3 == 0,
                    "{k:?}"
                );
                assert_eq!(part.mesh.pos.len(), part.mesh.nrm.len());
            }
        }
    }
}
