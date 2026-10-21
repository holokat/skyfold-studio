//! Changing caves: add one where it was clicked, change its recipe (laid out again), Shuffle,
//! grow a branch, lock a chamber, dig or fill by hand, remove. Every change is one undo step
//! (the caves are a `History` feature snapshot, key `UNDO_KEY`), and it dresses the ground round
//! each opening the same step: rock and dirt painted on, plants cleared (no tree in a cave mouth).
//! The UI and the MCP tools both come here.

use std::sync::Arc;

use bevy::prelude::*;

use super::layout;
use super::model::*;
use crate::io::WorldData;
use crate::noise::{Rng, fbm, smoothstep};
use crate::terrain::Terrain;
use crate::terrain::layers::{LAYER_COUNT, Layer};

pub const UNDO_KEY: &str = "caves";

/// What to make (the Caves tab's settings or an MCP call).
#[derive(Clone, Debug)]
pub struct NewCave {
    pub kind: CaveKind,
    pub size: CaveSize,
    pub levels: u8,
    /// the levels climb into the hill instead of going down
    pub up: bool,
    pub theme: Theme,
    pub features: Features,
    pub darkness: Darkness,
    pub seed: Option<u32>,
    /// where it was clicked (x, z)
    pub at: Vec2,
    /// heading into the cave (None: straight uphill, or away from `toward` on flat ground)
    pub heading: Option<f32>,
    /// the camera (x, z): flat-ground caves run away from it, arches face it
    pub toward: Vec2,
    pub name: Option<String>,
    /// a dungeon door at the end (its name)
    pub dungeon: Option<String>,
    /// arches: the far foot (x, z) (None: a default span across the view)
    pub span_to: Option<Vec2>,
}

impl Default for NewCave {
    fn default() -> Self {
        Self {
            kind: CaveKind::Cave,
            size: CaveSize::Medium,
            levels: 1,
            up: false,
            theme: Theme::Auto,
            features: Features::default(),
            darkness: Darkness::Dim,
            seed: None,
            at: Vec2::ZERO,
            heading: None,
            toward: Vec2::ZERO,
            name: None,
            dungeon: None,
            span_to: None,
        }
    }
}

/// A name that fits the cave (deterministic per seed).
pub fn auto_name(kind: CaveKind, theme: Theme, seed: u32) -> String {
    let mut r = Rng::new(seed as u64 ^ 0x4e41_4d45);
    let adj: &[&str] = match theme {
        Theme::Ice => &["Frostfang", "Rimewater", "Glacier", "Hoarfrost", "Icevein"],
        Theme::Mossy => &["Mossy", "Fernshade", "Verdant", "Dripleaf", "Greenwater"],
        Theme::Sandstone => &["Sunbaked", "Redrock", "Dune", "Ochre", "Windcut"],
        Theme::Volcanic => &["Ember", "Cinder", "Ashen", "Obsidian", "Smoulder"],
        Theme::Crystal => &["Glimmer", "Crystal", "Starlit", "Prism", "Shimmer"],
        Theme::Mine => &[
            "Copperseam",
            "Old Iron",
            "Deepvein",
            "Silverseam",
            "Blackrock",
        ],
        _ => &["Echoing", "Weeping", "Greystone", "Dripstone", "Whispering"],
    };
    let noun: &[&str] = match kind {
        CaveKind::Mine => &["Mine", "Diggings", "Delve", "Workings"],
        CaveKind::Pit => &["Sinkhole", "Pit", "Maw", "Well"],
        CaveKind::Passage => &["Passage", "Tunnel", "Gap", "Throughway"],
        CaveKind::Cavern => &["Cavern", "Vault", "Undercroft"],
        CaveKind::Arch => &["Arch", "Gate", "Bridge"],
        CaveKind::Cave => &["Hollow", "Grotto", "Caverns", "Deeps", "Halls"],
    };
    format!(
        "{} {}",
        adj[r.below(adj.len() as u32) as usize],
        noun[r.below(noun.len() as u32) as usize]
    )
}

fn snapshot(c: &Caves) -> Arc<Caves> {
    Arc::new(c.clone())
}

/// Records the change from `before` to the caves now (one step with whatever else the open step
/// holds) and closes the step.
fn finish(d: &mut WorldData, label: &str, before: Arc<Caves>) {
    let after = snapshot(&d.caves);
    if *after != *before {
        d.history
            .push_ext(label.to_string(), UNDO_KEY, before, after);
        d.meta.dirty = true;
    }
    d.history.commit(&d.store);
}

/// Adds a cave (or arch) where `n.at` was clicked. Returns its id.
pub fn create(d: &mut WorldData, n: &NewCave) -> Result<u32, String> {
    let t = &*d.terrain;
    if !t.in_bounds(n.at.x, n.at.y) {
        return Err(format!(
            "[{:.0}, {:.0}] is outside the world",
            n.at.x, n.at.y
        ));
    }
    let ground = t.height_at(n.at.x, n.at.y);
    if ground < t.sea_level + 0.5 && n.kind != CaveKind::Cavern && n.kind != CaveKind::Arch {
        return Err(
            "that spot is under water: click dry land (a hillside makes the best cave mouth)"
                .into(),
        );
    }
    let seed = n.seed.unwrap_or_else(|| fastrand::u32(..));
    let (uphill, _) = layout::uphill(t, n.at, n.toward);
    let heading = n.heading.unwrap_or(uphill);
    let mut cave = Cave {
        id: 0,
        name: String::new(),
        kind: n.kind,
        size: n.size,
        levels: n.levels.clamp(1, 4),
        up: n.up,
        theme: n.theme,
        resolved: Theme::Auto,
        features: n.features,
        darkness: n.darkness,
        seed,
        entrance: [n.at.x, ground, n.at.y],
        heading,
        mouth: Mouth::None,
        chambers: vec![],
        tunnels: vec![],
        dungeon: None,
        arch: None,
        props: vec![],
    };
    if n.kind == CaveKind::Arch {
        // a span across the view by default (its opening faces the camera)
        let to = n.span_to.unwrap_or_else(|| {
            let v = (n.at - n.toward).normalize_or(Vec2::Y);
            n.at + Vec2::new(-v.y, v.x) * 16.0
        });
        let (a, b) = (n.at, to);
        let span = a.distance(b).clamp(6.0, 60.0);
        let mid = (a + b) * 0.5;
        let dir = (b - a).normalize_or(Vec2::X);
        let (a, b) = (mid - dir * span * 0.5, mid + dir * span * 0.5);
        let mut r = Rng::new(seed as u64);
        cave.arch = Some(ArchSpec {
            a: a.to_array(),
            b: b.to_array(),
            height: span * r.range(0.45, 0.7),
            thickness: (span * r.range(0.16, 0.24)).clamp(2.2, 9.0),
        });
        cave.entrance = [mid.x, t.height_at(mid.x, mid.y), mid.y];
        if cave.theme == Theme::Auto {
            cave.resolved = Theme::for_biome(
                crate::biomes::BiomePreset::from_id(t.biome_at(mid.x, mid.y)),
                CaveKind::Arch,
            );
        }
    } else {
        layout::layout(&mut cave, t);
        if cave.chambers.is_empty() {
            return Err("no room for a cave there (too close to the world's edge?)".into());
        }
    }
    cave.name = n
        .name
        .clone()
        .unwrap_or_else(|| auto_name(cave.kind, cave.theme(), seed));
    if let Some(dn) = &n.dungeon {
        cave.dungeon = Some(Dungeon {
            name: if dn.is_empty() {
                format!("{} Depths", cave.name)
            } else {
                dn.clone()
            },
            chamber: 0,
            door: [0.0; 3],
            door_yaw: 0.0,
            instancing: Instancing::Party,
        });
        layout::place_door(&mut cave);
    }
    let label = format!("Add {}", cave.kind.name().to_lowercase());
    d.history.begin(label.clone());
    let before = snapshot(&d.caves);
    cave.id = d.caves.alloc_id();
    let id = cave.id;
    dress_ground(d, &cave);
    d.caves.list.push(cave);
    super::dress::refurnish(d, id);
    finish(d, &label, before);
    Ok(id)
}

/// Changes a cave's recipe and lays it out again (same seed: the layout only changes where the
/// recipe does). `f` returns false when nothing changed.
pub fn update(
    d: &mut WorldData,
    id: u32,
    label: &str,
    f: impl FnOnce(&mut Cave) -> bool,
) -> Result<(), String> {
    let mut cave = d
        .caves
        .get(id)
        .ok_or_else(|| format!("no cave {id}"))?
        .clone();
    if !f(&mut cave) {
        return Ok(());
    }
    relayout(&mut cave, &d.terrain);
    d.history.begin(label.to_string());
    let before = snapshot(&d.caves);
    dress_ground(d, &cave);
    if let Some(c) = d.caves.get_mut(id) {
        *c = cave;
    }
    super::dress::refurnish(d, id);
    finish(d, label, before);
    Ok(())
}

/// Lays a cave out again from its recipe (arches: their span from the feet).
fn relayout(cave: &mut Cave, t: &Terrain) {
    if cave.kind == CaveKind::Arch {
        if cave.theme == Theme::Auto {
            let e = Vec3::from_array(cave.entrance);
            cave.resolved = Theme::for_biome(
                crate::biomes::BiomePreset::from_id(t.biome_at(e.x, e.z)),
                CaveKind::Arch,
            );
        }
        if let Some(a) = &mut cave.arch {
            let mut r = Rng::new(cave.seed as u64);
            let span = Vec2::from_array(a.a).distance(Vec2::from_array(a.b));
            a.height = span * r.range(0.45, 0.7);
            a.thickness = (span * r.range(0.16, 0.24)).clamp(2.2, 9.0);
        }
        return;
    }
    layout::layout(cave, t);
}

/// Shuffle: a new layout (next seed, or `seed`), locked chambers kept.
pub fn shuffle(d: &mut WorldData, id: u32, seed: Option<u32>) -> Result<u32, String> {
    let mut new_seed = 0;
    update(d, id, "Shuffle cave", |c| {
        c.seed = seed.unwrap_or_else(|| c.seed.wrapping_mul(0x2545_f491).wrapping_add(0x9e37_79b9));
        new_seed = c.seed;
        true
    })?;
    Ok(new_seed)
}

/// Grows a tunnel and a chamber from chamber `from` (default: the deepest) towards `heading`
/// (default: away from the way in), a level lower with `down`.
pub fn extend(
    d: &mut WorldData,
    id: u32,
    from: Option<u16>,
    heading: Option<f32>,
    down: bool,
) -> Result<u16, String> {
    let mut cave = d
        .caves
        .get(id)
        .ok_or_else(|| format!("no cave {id}"))?
        .clone();
    if cave.kind == CaveKind::Arch {
        return Err("arches have no chambers to grow from".into());
    }
    let from = from
        .or_else(|| cave.deepest())
        .ok_or("the cave has no chambers")?;
    let c = cave
        .chamber(from)
        .ok_or_else(|| format!("no chamber {from}"))?
        .clone();
    let hd = heading.unwrap_or_else(|| {
        let e = Vec3::from_array(cave.entrance);
        let away = Vec2::new(c.pos[0] - e.x, c.pos[2] - e.z);
        if away.length() > 1.0 {
            away.x.atan2(away.y)
        } else {
            cave.heading
        }
    });
    let new = layout::extend(&mut cave, &d.terrain, from, hd, down)?;
    if cave.dungeon.is_some() {
        layout::place_door(&mut cave);
    }
    let label = "Extend cave";
    d.history.begin(label);
    let before = snapshot(&d.caves);
    dress_ground(d, &cave);
    if let Some(x) = d.caves.get_mut(id) {
        *x = cave;
    }
    super::dress::refurnish(d, id);
    finish(d, label, before);
    Ok(new)
}

/// Locks (kept by Shuffle) or unlocks a chamber.
pub fn lock(d: &mut WorldData, id: u32, chamber: u16, locked: bool) -> Result<(), String> {
    let label = if locked {
        "Lock chamber"
    } else {
        "Unlock chamber"
    };
    let before = snapshot(&d.caves);
    let c = d.caves.get_mut(id).ok_or_else(|| format!("no cave {id}"))?;
    let ch = c
        .chambers
        .iter_mut()
        .find(|c| c.id == chamber)
        .ok_or_else(|| format!("no chamber {chamber}"))?;
    ch.locked = locked;
    d.history.begin(label);
    finish(d, label, before);
    Ok(())
}

pub fn remove(d: &mut WorldData, id: u32) -> Result<(), String> {
    if d.caves.get(id).is_none() {
        return Err(format!("no cave {id}"));
    }
    let label = "Remove cave";
    d.history.begin(label);
    let before = snapshot(&d.caves);
    if let Some(c) = d.caves.get(id).cloned() {
        super::dress::unfurnish(d, &c);
    }
    d.caves.list.retain(|c| c.id != id);
    finish(d, label, before);
    Ok(())
}

/// A freehand dig (or fill) stroke along `pts`, `radius` wide.
pub fn dig(d: &mut WorldData, pts: Vec<Vec3>, radius: f32, fill: bool) -> Result<(), String> {
    if pts.is_empty() {
        return Err("no points".into());
    }
    let label = if fill { "Fill rock" } else { "Dig" };
    d.history.begin(label);
    let before = snapshot(&d.caves);
    // simplify: a point every ~radius
    let mut kept: Vec<[f32; 3]> = vec![];
    for p in pts {
        if kept
            .last()
            .is_none_or(|q| Vec3::from_array(*q).distance(p) >= radius * 0.6)
        {
            kept.push(p.to_array());
        }
    }
    d.caves.strokes.push(DigStroke {
        pts: kept,
        radius: radius.clamp(0.5, 12.0),
        fill,
    });
    finish(d, label, before);
    Ok(())
}

/// The openings of a cave in the ground: (centre x, z, radius).
pub fn openings(c: &Cave) -> Vec<(Vec2, f32)> {
    let mut out = vec![];
    let e = Vec2::new(c.entrance[0], c.entrance[2]);
    match c.mouth {
        Mouth::Hillside | Mouth::Adit => {
            let w = c
                .tunnels
                .iter()
                .find(|t| t.a == OUTSIDE)
                .map(|t| t.width)
                .unwrap_or(4.0);
            out.push((e + layout::dir(c.heading) * 2.0, w + 5.0));
        }
        Mouth::Shaft => out.push((e, 7.0)),
        Mouth::Pit => {
            if let Some(ch) = c.chambers.first() {
                let r0 = ch.size[0].min(ch.size[2]) * 0.85;
                out.push((Vec2::new(ch.pos[0], ch.pos[2]), r0 * 1.35 + 2.0 + 5.0));
            }
        }
        Mouth::None => {}
    }
    for t in c.tunnels.iter().filter(|t| t.b == OUTSIDE) {
        if let Some(p) = t.pts.last() {
            out.push((Vec2::new(p[0], p[2]), t.width + 5.0));
        }
    }
    for ch in c.chambers.iter().filter(|c| c.skylight) {
        out.push((
            Vec2::new(ch.pos[0], ch.pos[2]),
            ch.size[0].min(ch.size[2]) * 0.4 + 4.0,
        ));
    }
    if let Some(a) = &c.arch {
        out.push((Vec2::from_array(a.a), a.thickness + 3.0));
        out.push((Vec2::from_array(a.b), a.thickness + 3.0));
    }
    out
}

/// Bare ground round each opening (rock on slopes, dirt and gravel on the flat, ragged edge)
/// and no plants there (trees a little further out). Records into the open step.
fn dress_ground(d: &mut WorldData, c: &Cave) {
    let open = openings(c);
    let seed = c.seed;
    for (k, (centre, r)) in open.iter().enumerate() {
        paint_bare(
            &mut d.terrain,
            &mut d.history,
            *centre,
            *r,
            seed.wrapping_add(k as u32 * 97),
        );
        let (centre, r) = (*centre, *r);
        crate::nature::paint_region(
            &mut d.nature,
            &mut d.history,
            "Cave mouth",
            centre - Vec2::splat(r),
            centre + Vec2::splat(r),
            move |q| q.distance(centre) < r * 0.85,
            &crate::nature::field::Target::All,
            crate::nature::AreaOp::Keep(0.0),
        );
    }
    // an arch's span is kept clear of trees (none growing through it)
    if let Some(a) = &c.arch {
        let (pa, pb) = (Vec2::from_array(a.a), Vec2::from_array(a.b));
        let w = a.thickness * 1.5 + 4.0;
        let lo = pa.min(pb) - Vec2::splat(w);
        let hi = pa.max(pb) + Vec2::splat(w);
        let trees = crate::nature::field::Target::Category(
            crate::nature::rules::cat_index(crate::objects::Category::Trees).unwrap_or(0),
        );
        crate::nature::paint_region(
            &mut d.nature,
            &mut d.history,
            "Rock arch",
            lo,
            hi,
            move |q| super::sdf::seg_dist(q, pa, pb) < w,
            &trees,
            crate::nature::AreaOp::Keep(0.0),
        );
    }
}

fn paint_bare(
    t: &mut Terrain,
    history: &mut crate::history::History,
    centre: Vec2,
    r: f32,
    seed: u32,
) {
    let Some(rect) = t.rect_for_circle(centre, r + 3.0) else {
        return;
    };
    history.touch_terrain(t, rect);
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let w = t.grid_to_world(i, j);
            let edge = fbm(w.x / 5.0, w.y / 5.0, 2, 2.0, 0.5, seed) * 2.2;
            let k = smoothstep(r, r * 0.45, w.distance(centre) + edge);
            if k <= 0.004 {
                continue;
            }
            let ny = t.normal_at_grid(i, j).y;
            // (the cave's own copy of the ground uses the same: rock on slopes, dirt on the flat)
            let layer = if ny < 0.84 { Layer::Rock } else { Layer::Dirt };
            let idx = t.idx(i, j);
            let s = &mut t.splat[idx];
            for (l, v) in s.iter_mut().enumerate().take(LAYER_COUNT) {
                let target = if l == layer as usize { 255.0 } else { 0.0 };
                *v = (*v as f32 + (target - *v as f32) * k)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            let sum: u32 = s.iter().map(|v| *v as u32).sum();
            if sum > 255 {
                let f = 255.0 / sum as f32;
                s.iter_mut()
                    .for_each(|v| *v = (*v as f32 * f).floor() as u8);
            }
        }
    }
    t.mark_splat_dirty(rect);
}

/// Undo / redo of cave edits: restore the snapshot the history hands back.
pub(super) fn apply_undo(mut history: ResMut<crate::history::History>, mut caves: ResMut<Caves>) {
    if history.ext_apply.is_empty() {
        return;
    }
    let mut keep = vec![];
    for (key, snap) in std::mem::take(&mut history.ext_apply) {
        if key != UNDO_KEY {
            keep.push((key, snap));
            continue;
        }
        if let Some(c) = snap.downcast_ref::<Caves>() {
            *caves = c.clone();
        }
    }
    history.ext_apply = keep;
}
