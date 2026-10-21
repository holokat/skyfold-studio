//! Putting a laid-out town into the world, as one undo step: the streets become real roads
//! (`roads::build`: bed, surface, cleared vegetation, bridges; editable in the Roads tab), the
//! buildings ordinary placed objects that level the ground under them (`placement::fit`), then
//! the props, trees and walls on the finished ground, the square's paint, a soft lawn over the
//! open ground between the buildings (grassy lands), and nature cleared off the plots only.
//! Every object of one town shares one group id.
//!
//! A click gives an area of the size's extent round the point, turned so the main street runs
//! the flattest way (`click_frame`); a drawn loop is filled exactly.

use std::collections::BTreeMap;

use bevy::ecs::system::{SystemParam, SystemState};
use bevy::prelude::*;

use super::kinds::{
    Catalog, Extra, Style, TownSize, Wanted, default_counts, draw_up, parse_type, type_index,
};
use super::layout::{Obb, PaintArea, PieceRole, Site, Spec, Town, layout, poly_area, rot2};
use crate::history::History;
use crate::noise::{Rng, fbm, smoothstep};
use crate::objects::{Category, Library, ObjectStore, Placed};
use crate::placement::{Footprint, ModelBounds};
use crate::roads::build::{Edit, RoadWorld};
use crate::roads::{Road, RoadOptions};
use crate::terrain::Terrain;
use crate::terrain::layers::{LAYER_COUNT, Layer};
use crate::water::WaterBodies;
use crate::world::Biome;

/// Where a town goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Area {
    /// one click: the size's area round this point (world x, z), turned to suit the ground
    Click(Vec2),
    /// the same, turned to this facing (its long side along `rot2(X, facing)`)
    ClickTurned(Vec2, f32),
    /// a drawn outline (world x, z) and the point its front faces
    Outline(Vec<Vec2>, Vec2),
}

/// A town to place.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub size: TownSize,
    pub area: Area,
    pub style: Style,
    pub roads: bool,
    pub walls: bool,
    /// the buildings asked for, in people's words ("smithy", "houses") with counts; None: the
    /// size's own mix
    pub buildings: Option<Vec<(String, usize)>>,
    pub seed: u32,
    /// streets lead on out past the area (not when filling a selection)
    pub lead_out: bool,
    /// its name (None: one made up from the seed)
    pub name: Option<String>,
}

/// What was placed.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub name: String,
    pub size: TownSize,
    pub style: Style,
    pub area: f32,
    pub centre: Vec2,
    /// the outline it fills (world x, z)
    pub outline: Vec<Vec2>,
    pub group: u64,
    pub ids: Vec<u64>,
    pub roads: Vec<u32>,
    pub counts: BTreeMap<String, usize>,
    pub buildings: usize,
    pub notes: Vec<String>,
}

/// The ground in a town's frame.
struct WorldSite<'a> {
    t: &'a Terrain,
    water: &'a WaterBodies,
    centre: Vec2,
    facing: f32,
}

impl WorldSite<'_> {
    fn world(&self, p: Vec2) -> Vec2 {
        self.centre + rot2(p, self.facing)
    }
}

impl Site for WorldSite<'_> {
    fn height(&self, p: Vec2) -> f32 {
        let w = self.world(p);
        self.t.height_at(w.x, w.y)
    }
    fn wet(&self, p: Vec2) -> bool {
        wet_at(self.t, self.water, self.world(p))
    }
}

fn wet_at(t: &Terrain, water: &WaterBodies, w: Vec2) -> bool {
    if !t.in_bounds(w.x, w.y) {
        return true;
    }
    let g = t.height_at(w.x, w.y);
    crate::roads::plan::water_surface(water, t.sea_level, w, g) > g + 0.05
}

/// The models this account can place (the bundled libraries'; a locked pack's are listed apart).
pub fn catalog(world: &World) -> Catalog {
    let mut c = Catalog::default();
    let (Some(lib), Some(bounds)) = (
        world.get_resource::<Library>(),
        world.get_resource::<ModelBounds>(),
    ) else {
        return c;
    };
    let market = crate::market::snapshot();
    for e in lib
        .entries
        .iter()
        .filter(|e| ["hp:", "sb:", "ph:"].iter().any(|p| e.id.starts_with(p)))
    {
        if market.usable(&e.id) {
            let (a, b) = bounds.of(e);
            c.assets
                .insert(e.id.clone(), (a * e.default_scale, b * e.default_scale));
        } else {
            c.locked.insert(e.id.clone());
        }
    }
    c
}

/// The area a click gives: the size's ellipse round `at`, turned so its main street runs the
/// flattest dry way across. (centre, facing, outline in the town's frame.)
pub fn click_frame(
    t: &Terrain,
    water: &WaterBodies,
    at: Vec2,
    size: TownSize,
    turn: Option<f32>,
    seed: u32,
) -> Result<(Vec2, f32, Vec<Vec2>), String> {
    let what = size.name().to_lowercase();
    if !t.in_bounds(at.x, at.y) {
        return Err(format!("[{:.0}, {:.0}] is outside the world", at.x, at.y));
    }
    if wet_at(t, water, at) {
        return Err(format!("That's water: click on dry land for the {what}"));
    }
    let e = size.extent();
    // how much of the area is dry land
    let (mut dry, mut all) = (0, 0);
    for i in -4..=4 {
        for j in -4..=4 {
            let l = Vec2::new(i as f32 / 4.0, j as f32 / 4.0);
            if l.length() <= 1.0 {
                all += 1;
                dry += usize::from(!wet_at(t, water, at + l * e.max_element()));
            }
        }
    }
    if dry * 10 < all * 4 {
        return Err(format!(
            "Mostly water round here: click further inland for the {what} (it needs about {:.0} × {:.0} m)",
            e.x * 2.0,
            e.y * 2.0
        ));
    }
    let theta = match turn {
        Some(f) => -f,
        None => {
            // the turn whose main street is flattest and driest (its cross street counts too)
            let cost = |th: f32| {
                let dir = Vec2::from_angle(th);
                let line = |d: Vec2, half: f32| {
                    let mut c = 0.0;
                    let mut prev: Option<f32> = None;
                    for i in 0..=20 {
                        let p = at + d * (-half + 2.0 * half * i as f32 / 20.0);
                        if wet_at(t, water, p) {
                            c += 40.0;
                            continue;
                        }
                        let h = t.height_at(p.x, p.y);
                        if let Some(q) = prev {
                            c += ((h - q) / (half / 10.0)).abs() * 10.0;
                        }
                        prev = Some(h);
                    }
                    c
                };
                line(dir, e.x) + line(dir.perp(), e.y) * if size.is_town() { 0.5 } else { 0.2 }
            };
            (0..12)
                .map(|k| k as f32 * std::f32::consts::PI / 12.0)
                .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))
                .unwrap_or(0.0)
        }
    };
    let facing = -theta;
    // a slightly irregular ellipse, kept inside the world
    let mut rng = Rng::new(seed as u64 ^ 0x5EED);
    let (p1, p2) = (rng.range(0.0, 6.3), rng.range(0.0, 6.3));
    let pts = (0..36)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 36.0;
            let wobble = 1.0 + 0.035 * (a * 3.0 + p1).sin() + 0.025 * (a * 5.0 + p2).sin();
            let mut l = Vec2::new(a.cos() * e.x, a.sin() * e.y) * wobble;
            while l.length() > 10.0 && {
                let w = at + rot2(l, facing);
                !t.in_bounds(w.x, w.y)
            } {
                l *= 0.95;
            }
            l
        })
        .collect();
    Ok((at, facing, pts))
}

/// A cosy name for a settlement (deterministic per seed).
pub fn town_name(seed: u32) -> String {
    const A: &[&str] = &[
        "Oak", "Ash", "Elm", "Thorn", "Mill", "Stone", "Brook", "Wil", "Hawk", "Raven", "Bram",
        "Fern", "Marsh", "Glen", "Briar", "Kings", "Red", "Black", "White", "Wolf", "Deer",
        "Holly", "Alder", "Birch", "Cold", "Fair", "Long", "High", "Hazel", "Willow", "Amber",
        "Moss",
    ];
    const B: &[&str] = &[
        "ford", "wick", "ton", "bury", "field", "stead", "dale", "brook", "moor", "holm", "by",
        "thorpe", "ham", "well", "bridge", "worth", "mere", "cross", "haven", "vale", "hollow",
        "combe",
    ];
    let mut r = Rng::new(seed as u64 ^ 0x7A3E);
    let a = A[r.below(A.len() as u32) as usize];
    let mut b = B[r.below(B.len() as u32) as usize];
    if a.to_lowercase().ends_with(&b[..1]) {
        b = "ton";
    }
    format!("{a}{b}")
}

/// Everything placing a town's objects touches.
#[derive(SystemParam)]
struct TownWorld<'w> {
    terrain: ResMut<'w, Terrain>,
    history: ResMut<'w, History>,
    store: ResMut<'w, ObjectStore>,
    lib: Option<Res<'w, Library>>,
    water: Res<'w, WaterBodies>,
    fit: crate::placement::FitParams<'w>,
    nature: ResMut<'w, crate::nature::NatureField>,
    meta: ResMut<'w, crate::world::WorldMeta>,
}

/// Structures already standing in the area and roads already crossing it, in the town's frame:
/// the new town builds round them.
fn in_the_way(
    world: &World,
    centre: Vec2,
    facing: f32,
    reach: f32,
) -> (Vec<Obb>, Vec<(Vec2, f32)>) {
    let mut obbs = vec![];
    let (Some(lib), Some(bounds)) = (
        world.get_resource::<Library>(),
        world.get_resource::<ModelBounds>(),
    ) else {
        return (obbs, vec![]);
    };
    let store = world.resource::<ObjectStore>();
    for id in store.query(centre, reach) {
        let Some(p) = store.items.get(&id) else {
            continue;
        };
        let Some(e) = lib
            .get(&p.asset)
            .filter(|e| e.category == Category::Structures)
        else {
            continue;
        };
        let fp = Footprint::of(p, bounds.of(e));
        obbs.push(Obb {
            c: rot2(fp.center - centre, -facing),
            ax: rot2(fp.ax, -facing),
            half: fp.half,
        });
    }
    let mut pts = vec![];
    for r in &world.resource::<crate::roads::Roads>().roads {
        for q in r.centreline(1.0) {
            let w = q.xz();
            if w.distance(centre) < reach {
                pts.push((rot2(w - centre, -facing), r.width * 0.5));
            }
        }
    }
    (obbs, pts)
}

/// Lays out and places a town (one undo step: its own, or the stroke being recorded).
pub fn place_town(world: &mut World, req: &Request) -> Result<Report, String> {
    let t0 = std::time::Instant::now();
    let cat = catalog(world);
    if cat.assets.is_empty() {
        return Err("The asset library is still loading: try again in a moment".into());
    }
    let style = req.style.resolve(&cat);
    let (centre, facing, local, size) = {
        let t = world.resource::<Terrain>();
        let water = world.resource::<WaterBodies>();
        match &req.area {
            Area::Click(at) => {
                let (c, f, l) = click_frame(t, water, *at, req.size, None, req.seed)?;
                (c, f, l, req.size)
            }
            Area::ClickTurned(at, turn) => {
                let (c, f, l) = click_frame(t, water, *at, req.size, Some(*turn), req.seed)?;
                (c, f, l, req.size)
            }
            Area::Outline(poly, toward) => {
                if poly.len() < 3 {
                    return Err("an area needs at least 3 corners".into());
                }
                let (c, f, l) = crate::setpieces::area_frame(poly, *toward);
                let size = TownSize::for_area(req.size.is_town(), poly_area(&l));
                (c, f, l, size)
            }
        }
    };
    let area = poly_area(&local);
    let name = req
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| town_name(req.seed));
    // the buildings
    let mut rng = Rng::new(req.seed as u64 ^ 0xB0B5);
    let mix = match &req.buildings {
        Some(list) => {
            let mut counts: Vec<(usize, usize)> = vec![];
            let mut stalls: Option<usize> = None;
            let mut notes = vec![];
            for (word, k) in list {
                match parse_type(word) {
                    Some(Wanted::Building(i)) => match counts.iter_mut().find(|c| c.0 == i) {
                        Some(c) => c.1 += k,
                        None => counts.push((i, *k)),
                    },
                    Some(Wanted::Extra(Extra::Stall)) => *stalls.get_or_insert(0) += k,
                    Some(Wanted::Extra(Extra::Well)) => {}
                    None => {
                        notes.push(format!("`{word}`: not a building this can place — skipped"))
                    }
                }
            }
            let mut m = draw_up(&counts, size, style, &cat, true, &mut rng);
            m.stalls = stalls;
            notes.append(&mut m.notes);
            m.notes = notes;
            m
        }
        None => {
            let mut ids = default_counts(size, size.buildings_for(area), &mut rng);
            // a river or lake by it: a mill on its bank in place of one home
            let t = world.resource::<Terrain>();
            let water = world.resource::<WaterBodies>();
            let site = WorldSite {
                t,
                water,
                centre,
                facing,
            };
            if size != TownSize::Hamlet
                && water_near(&site, &local)
                && let Some(h) = ids.iter_mut().find(|(id, k)| *id == "house" && *k > 1)
            {
                h.1 -= 1;
                ids.push(("sawmill", 1));
            }
            let counts: Vec<(usize, usize)> = ids
                .into_iter()
                .filter_map(|(id, k)| Some((type_index(id)?, k)))
                .collect();
            draw_up(&counts, size, style, &cat, false, &mut rng)
        }
    };
    let reach = local.iter().map(|p| p.length()).fold(0.0, f32::max) + 20.0;
    let (obstacles, keep_off) = in_the_way(world, centre, facing, reach);
    let biome = biome_at(world, centre);
    let trees = trees_for(world, biome);
    let town = {
        let t = world.resource::<Terrain>();
        let water = world.resource::<WaterBodies>();
        let site = WorldSite {
            t,
            water,
            centre,
            facing,
        };
        let spec = Spec {
            size,
            style,
            walls: req.walls && size.is_town(),
            mix: &mix,
            seed: req.seed,
            lead_out: req.lead_out,
            obstacles: &obstacles,
            keep_off: &keep_off,
            trees: &trees,
        };
        layout(&local, &spec, &cat, &site)?
    };
    let buildings = town.buildings().count();
    if buildings == 0 && !mix.wants.is_empty() {
        return Err(format!(
            "No building fits here (steep ground, water or other buildings in the way): try flatter, open ground for the {}",
            size.name().to_lowercase()
        ));
    }
    let label = format!("Place {}", size.name().to_lowercase());
    let own = !world.resource::<History>().is_recording();
    {
        let mut h = world.resource_mut::<History>();
        if own {
            h.begin(label.clone());
        } else {
            h.set_label(label.clone());
        }
    }
    let roads = if req.roads {
        make_roads(world, &town, centre, facing, &name, &label)
    } else {
        vec![]
    };
    let lawn = grassy(biome).then_some(local.as_slice());
    let placed = stamp(world, &town, centre, facing, req.seed, size.name(), lawn);
    if own {
        world
            .resource_scope(|world, mut h: Mut<History>| h.commit(world.resource::<ObjectStore>()));
    }
    let (group, ids) = placed?;
    info!(
        "TOWN {name}: laid out and placed in {:.0} ms ({} objects, {} roads)",
        t0.elapsed().as_secs_f32() * 1000.0,
        ids.len(),
        roads.len()
    );
    let mut notes = town.notes.clone();
    if req.walls && !size.is_town() {
        notes.push("Walls are for towns and cities: this village has none".into());
    }
    let outline = local.iter().map(|p| centre + rot2(*p, facing)).collect();
    Ok(Report {
        name,
        size,
        style,
        area,
        centre,
        outline,
        group,
        ids,
        roads,
        counts: town.counts.clone(),
        buildings,
        notes,
    })
}

/// Whether a river, lake or the sea comes into the area (or within a few metres of it).
fn water_near(site: &dyn Site, poly: &[Vec2]) -> bool {
    let (lo, hi) = poly.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(a, b), q| (a.min(*q), b.max(*q)),
    );
    let mut z = lo.y;
    while z <= hi.y {
        let mut x = lo.x;
        while x <= hi.x {
            let q = Vec2::new(x, z);
            if crate::setpieces::poly_sdf(poly, q) < 2.0 && site.wet(q) {
                return true;
            }
            x += 8.0;
        }
        z += 8.0;
    }
    false
}

/// The biome of the land at `at` (world x, z).
fn biome_at(world: &World, at: Vec2) -> Biome {
    let t = world.resource::<Terrain>();
    crate::biomes::BiomePreset::from_id(t.biome_at(at.x, at.y))
        .map(|b| b.def().biome)
        .unwrap_or(Biome::Temperate)
}

/// Lands whose open ground is grass (a town's lawn suits them; not deserts, tundra, ash).
fn grassy(b: Biome) -> bool {
    !matches!(b, Biome::Desert | Biome::Tundra | Biome::Volcanic)
}

/// Trees that suit the land (library ids), planted between the buildings like the big trees
/// of hand-made towns: broadleaves in temperate land, birch and spruce up north, palms in the
/// south, willows in a swamp.
fn trees_for(world: &World, biome: Biome) -> Vec<String> {
    use crate::procgen::meshes::AssetKind::{self, *};
    let Some(lib) = world.get_resource::<Library>() else {
        return vec![];
    };
    let kinds: &[AssetKind] = match biome {
        Biome::Temperate => &[Oak, Maple, Birch],
        Biome::Autumn => &[Maple, Oak, Birch],
        Biome::Boreal => &[Birch, Spruce],
        Biome::Alpine | Biome::Tundra => &[Spruce, Pine],
        Biome::Tropical | Biome::Desert => &[Palm],
        Biome::Swamp => &[Willow, Birch],
        Biome::Volcanic => &[],
    };
    kinds
        .iter()
        .map(|k| crate::objects::builtin_id(*k))
        .filter(|id| lib.get(id).is_some_and(|e| e.category == Category::Trees))
        .collect()
}

/// The town's streets as real roads (lamps are the town's own).
fn make_roads(
    world: &mut World,
    town: &Town,
    centre: Vec2,
    facing: f32,
    name: &str,
    label: &str,
) -> Vec<u32> {
    let mut st: SystemState<RoadWorld> = SystemState::new(world);
    let mut ids = vec![];
    {
        let Ok(mut rw) = st.get_mut(world) else {
            return ids;
        };
        for s in &town.streets {
            let pts: Vec<[f32; 3]> = s
                .line
                .samples(10.0)
                .into_iter()
                .map(|p| centre + rot2(p, facing))
                .filter(|w| rw.terrain.in_bounds(w.x, w.y))
                .map(|w| [w.x, rw.terrain.height_at(w.x, w.y), w.y])
                .collect();
            if pts.len() < 2 {
                continue;
            }
            let mut road = Road::new(
                0,
                s.kind,
                pts,
                s.width,
                RoadOptions {
                    props: false,
                    ..Default::default()
                },
            );
            road.name = format!("{name} {}", s.name.to_lowercase());
            match rw.apply(Edit::Create(road), label) {
                Ok(id) => ids.push(id),
                Err(e) => warn!("TOWN street: {e}"),
            }
        }
        rw.roads.selected = None;
    }
    st.apply(world);
    ids
}

/// How much further out than the rest of nature trees are cleared (m): enough that crowns
/// don't hang over the roofs; the trees between the plots stay.
const TREE_CLEAR: f32 = 3.5;

/// Places the town's objects (buildings first, fitted; then props, trees and walls on the
/// finished ground), paints the square (and a lawn over the open ground inside `lawn`, the
/// outline in the town's frame) and clears nature off the plots. Returns the group id and the
/// object ids.
fn stamp(
    world: &mut World,
    town: &Town,
    centre: Vec2,
    facing: f32,
    seed: u32,
    what: &str,
    lawn: Option<&[Vec2]>,
) -> Result<(u64, Vec<u64>), String> {
    let mut st: SystemState<TownWorld> = SystemState::new(world);
    let out = {
        let mut d = st
            .get_mut(world)
            .map_err(|e| format!("the world isn't ready: {e}"))?;
        let TownWorld {
            terrain,
            history,
            store,
            lib,
            water,
            fit,
            nature,
            meta,
        } = &mut d;
        let lib: &Library = lib
            .as_deref()
            .ok_or("the asset library is still loading; try again in a moment")?;
        let group = store.new_id();
        let mut ids = vec![];
        let put = |terrain: &Terrain,
                   history: &mut History,
                   store: &mut ObjectStore,
                   pc: &super::layout::Piece|
         -> Option<u64> {
            let e = lib.get(&pc.asset)?;
            let at = centre + rot2(pc.at, facing);
            if !terrain.in_bounds(at.x, at.y) {
                return None;
            }
            let turn = Quat::from_rotation_y(facing + pc.yaw);
            let (y, lean) = if pc.span > 0.0 {
                // a wall or fence section: level across, tilted along its run to meet the ground
                let ax = turn * Vec3::X;
                let half = Vec2::new(ax.x, ax.z) * pc.span * 0.5;
                let (h0, h1) = (
                    terrain.height_at(at.x - half.x, at.y - half.y),
                    terrain.height_at(at.x + half.x, at.y + half.y),
                );
                let sink = if e.category == Category::Structures {
                    0.15
                } else {
                    0.06
                };
                (
                    (h0 + h1) * 0.5 - sink,
                    Quat::from_rotation_z((h1 - h0).atan2(pc.span)),
                )
            } else {
                // on a slope, sink a little more so no edge floats
                let n = terrain.normal_at(at.x, at.y);
                let slope = (1.0 - n.y * n.y).max(0.0).sqrt() / n.y.max(0.2);
                (
                    terrain.height_at(at.x, at.y) - 0.05 - slope * e.radius.min(2.0) * 0.45,
                    Quat::IDENTITY,
                )
            };
            let id = store.new_id();
            history.touch_object(store, id);
            let mut p = Placed {
                group,
                id,
                asset: e.id.clone(),
                variant: 0,
                pos: [at.x, y, at.y],
                rot: (turn * lean).to_array(),
                scale: 1.0,
                stretch: None,
                locked: false,
            };
            p.set_scale3(pc.scale * e.default_scale);
            store.insert(p);
            Some(id)
        };
        // buildings, fitted to the ground
        let mut built = vec![];
        for pc in town.pieces.iter().filter(|p| p.role == PieceRole::Building) {
            if let Some(id) = put(terrain, history, store, pc) {
                built.push(id);
            }
        }
        {
            let mut w = crate::placement::FitWorld {
                terrain,
                history,
                store,
                lib,
                water,
                found: &mut fit.found,
                bounds: &fit.bounds,
            };
            crate::placement::fit::fit_objects(&mut w, &built, &[]);
        }
        ids.extend(built);
        // props, fences and walls on the levelled ground (walls follow it: never levelled)
        let mut walls = vec![];
        for pc in town.pieces.iter().filter(|p| p.role != PieceRole::Building) {
            if let Some(id) = put(terrain, history, store, pc) {
                if pc.role == PieceRole::Wall
                    && lib
                        .get(&pc.asset)
                        .is_some_and(|e| e.category == Category::Structures)
                {
                    walls.push(id);
                }
                ids.push(id);
            }
        }
        if !walls.is_empty() {
            let choice: Vec<(u64, Option<bool>)> =
                walls.iter().map(|id| (*id, Some(false))).collect();
            let mut w = crate::placement::FitWorld {
                terrain,
                history,
                store,
                lib,
                water,
                found: &mut fit.found,
                bounds: &fit.bounds,
            };
            crate::placement::fit::fit_objects(&mut w, &walls, &choice);
        }
        if let Some(poly) = lawn {
            paint_lawn(terrain, history, centre, facing, town, poly, seed);
        }
        for (k, p) in town.paint.iter().enumerate() {
            paint_ground(
                terrain,
                history,
                centre,
                facing,
                p,
                seed.wrapping_add(k as u32 * 101),
            );
        }
        // nature off the plots, the square and the walls; trees a little further out
        if !town.clear.is_empty() {
            let r = town
                .clear
                .iter()
                .map(|o| o.c.length() + o.radius())
                .fold(0.0, f32::max)
                + TREE_CLEAR;
            let (lo, hi) = (centre - Vec2::splat(r), centre + Vec2::splat(r));
            let clear = town.clear.clone();
            let inside = move |q: Vec2, m: f32| {
                let l = rot2(q - centre, -facing);
                clear.iter().any(|o| o.sdist(l) < m)
            };
            let c2 = inside.clone();
            crate::nature::paint_region(
                nature,
                history,
                what,
                lo,
                hi,
                move |q| inside(q, 0.0),
                &crate::nature::field::Target::All,
                crate::nature::AreaOp::Keep(0.0),
            );
            let trees = crate::nature::field::Target::Category(
                crate::nature::rules::cat_index(Category::Trees).unwrap_or(0),
            );
            crate::nature::paint_region(
                nature,
                history,
                what,
                lo,
                hi,
                move |q| c2(q, TREE_CLEAR),
                &trees,
                crate::nature::AreaOp::Keep(0.0),
            );
        }
        store.selected = None;
        meta.dirty = true;
        Ok((group, ids))
    };
    st.apply(world);
    out
}

/// Blends the ground paint toward a layer over an area with a soft, ragged edge.
fn paint_ground(
    t: &mut Terrain,
    history: &mut History,
    centre: Vec2,
    facing: f32,
    p: &PaintArea,
    seed: u32,
) {
    let c = centre + rot2(p.obb.c, facing);
    let Some(rect) = t.rect_for_circle(c, p.obb.radius() + p.round + 2.5) else {
        return;
    };
    history.touch_terrain(t, rect);
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let w = t.grid_to_world(i, j);
            let local = rot2(w - centre, -facing);
            let edge = fbm(w.x / 6.0, w.y / 6.0, 2, 2.0, 0.5, seed) * 1.4;
            let k = smoothstep(1.0, -1.2, p.sdf(local) + edge) * p.strength;
            if k <= 0.004 {
                continue;
            }
            let layer = match p.patches {
                Some(l) if fbm(w.x / 4.5, w.y / 4.5, 2, 2.0, 0.5, seed + 9) > 0.2 => l,
                _ => p.layer,
            };
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

/// A soft, patchy lawn over the open ground between the buildings: grass blended over what the
/// land paints there (often dry patches), fading out past the outline. The streets' shoulders,
/// the square and the trodden aprons round the buildings keep their own ground; rock, snow and
/// sand stay as they are.
fn paint_lawn(
    t: &mut Terrain,
    history: &mut History,
    centre: Vec2,
    facing: f32,
    town: &Town,
    poly: &[Vec2],
    seed: u32,
) {
    let r = poly.iter().map(|p| p.length()).fold(0.0, f32::max) + 8.0;
    let Some(rect) = t.rect_for_circle(centre, r) else {
        return;
    };
    history.touch_terrain(t, rect);
    let streets: Vec<(&super::layout::Line, f32)> = town
        .streets
        .iter()
        .map(|s| (&s.line, s.width * 0.5))
        .collect();
    let built: Vec<Obb> = town
        .pieces
        .iter()
        .filter(|p| p.role == PieceRole::Building)
        .filter_map(|p| p.obb)
        .collect();
    let (grass, keep) = (
        Layer::Grass as usize,
        [
            Layer::Rock,
            Layer::DarkRock,
            Layer::Snow,
            Layer::Ice,
            Layer::Sand,
            Layer::Cobblestone,
            Layer::Mud,
        ]
        .map(|l| l as usize),
    );
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let w = t.grid_to_world(i, j);
            let l = rot2(w - centre, -facing);
            let edge = fbm(w.x / 11.0, w.y / 11.0, 2, 2.0, 0.5, seed ^ 0x1A3) * 8.0;
            let inside = smoothstep(4.0, -12.0, crate::setpieces::poly_sdf(poly, l) + edge);
            if inside <= 0.01 {
                continue;
            }
            // (keep a road's shoulder, the square and a building's apron)
            let off = streets
                .iter()
                .map(|(line, hw)| line.nearest(l).1 - hw)
                .fold(f32::MAX, f32::min);
            let plot = built.iter().map(|o| o.sdist(l)).fold(f32::MAX, f32::min);
            let sq = town.square.map(|s| s.sdist(l)).unwrap_or(f32::MAX);
            let room =
                smoothstep(2.0, 4.5, off) * smoothstep(2.6, 5.0, plot) * smoothstep(1.0, 3.0, sq);
            // (mostly grass, the odd worn patch: enough to win over the dry ground it covers)
            let patches = smoothstep(
                -0.35,
                0.05,
                fbm(w.x / 7.0, w.y / 7.0, 2, 2.0, 0.5, seed ^ 0x77),
            );
            let k = (0.92 * inside * room * (0.55 + 0.45 * patches)).clamp(0.0, 0.92);
            if k <= 0.01 {
                continue;
            }
            let fw = t.final_weights(i, j);
            if keep.iter().map(|l| fw[*l]).sum::<f32>() > 0.35 {
                continue;
            }
            let idx = t.idx(i, j);
            let s = &mut t.splat[idx];
            for (li, v) in s.iter_mut().enumerate().take(LAYER_COUNT) {
                let target = if li == grass { 255.0 } else { 0.0 };
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

/// Takes a placed town away again (Shuffle): its objects (their ground comes back) and its
/// roads. Inside the caller's undo step.
pub fn remove_town(world: &mut World, ids: &[u64], roads: &[u32]) {
    {
        let mut st: SystemState<TownWorld> = SystemState::new(world);
        if let Ok(mut d) = st.get_mut(world) {
            let TownWorld {
                terrain,
                history,
                store,
                lib,
                water,
                fit,
                ..
            } = &mut d;
            let gone: Vec<u64> = ids
                .iter()
                .copied()
                .filter(|id| store.items.contains_key(id))
                .collect();
            for id in &gone {
                history.touch_object(store, *id);
                store.remove(*id);
            }
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
                crate::placement::fit::sync(&mut w, Some(&gone));
            }
        }
        st.apply(world);
    }
    let mut st: SystemState<RoadWorld> = SystemState::new(world);
    if let Ok(mut rw) = st.get_mut(world) {
        for id in roads {
            if rw.roads.get(*id).is_some()
                && let Err(e) = rw.apply(Edit::Remove(*id), "Shuffle town")
            {
                warn!("TOWN street {id}: {e}");
            }
        }
    }
    st.apply(world);
}
