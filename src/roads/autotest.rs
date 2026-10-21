//! Automation hooks for verifying roads (run after the demo world is built, through the same
//! edit code as the tools):
//!
//! - `WB_ROADS="kind:x,z;x,z;…|kind:…"` draws roads; `WB_ROADS=demo` builds a test network on the
//!   demo river (a cobbled road and a dirt road crossing it, a gravel branch, a footpath, a trail
//!   and an auto-routed highway).
//! - `WB_ROAD_ROUTE="kind:x0,z0,x1,z1|…"` auto-routes roads.
//! - `WB_ROAD_SEL=<id>` selects a road (for the inspector); `WB_ROAD_MODE=draw|route|edit|remove`
//!   picks the tool; `WB_ROAD_DRAFT="kind:x,z;x,z;…"` puts a road being drawn (live preview).
//! - `WB_ROAD_TEST=undo|remove|io|mask` (comma-separated) checks undo / redo, removing
//!   every road, save / load, the mask and exporter helpers, or the ribbon's terrain heights
//!   against the terrain mesh (logs `ROADS TEST … PASS / FAIL`; `WB_ROAD_TEST_DIR` receives the files).
//! - `WB_ROAD_SHOTS=<dir>` takes a series of screenshots from viewpoints derived from the roads
//!   (overview, each bridge from the side, eye-level views along each road, a junction, a
//!   close-up of an edge), then exits. Keep `WB_SHOT_FRAMES` above the sequence's length.
//! - `WB_ROAD_WALK=<dir>` walks across the first bridge in walk mode (with `WB_RUN=jog`), logs
//!   the walker's height against the deck and takes screenshots on the way.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use super::build::{Edit, RoadWorld};
use super::render::{RoadFloors, RoadVisuals};
use super::{Road, RoadKind, RoadOptions};

pub fn build(app: &mut App) {
    if std::env::vars().any(|(k, _)| k.starts_with("WB_ROAD")) {
        app.add_systems(
            Update,
            (scripted, scripted_checks, shots, walk_test)
                .chain()
                .after(crate::tools::ToolSystems::Commit),
        );
    }
}

fn ground_pt(rw: &RoadWorld, p: Vec2) -> [f32; 3] {
    [p.x, rw.terrain.height_at(p.x, p.y), p.y]
}

fn create(rw: &mut RoadWorld, kind: RoadKind, pts: Vec<[f32; 3]>, label: &str) -> Option<u32> {
    let t0 = std::time::Instant::now();
    let r = rw.apply(
        Edit::Create(Road::new(
            0,
            kind,
            pts,
            kind.default_width(),
            RoadOptions::default(),
        )),
        label,
    );
    info!(
        "ROADS scripted {} -> {r:?} in {:?}",
        kind.name(),
        t0.elapsed()
    );
    r.ok()
}

fn route(rw: &mut RoadWorld, kind: RoadKind, a: Vec2, b: Vec2) -> Option<u32> {
    let t0 = std::time::Instant::now();
    let base = super::ground::Base {
        t: &rw.terrain,
        g: &rw.roads.ground,
    };
    let input = super::route::prepare(&base, &rw.water, a, b, kind, kind.default_width());
    let pts = super::route::solve(&input);
    info!("ROADS route search {:?}", t0.elapsed());
    let pts = pts?.iter().map(|p| ground_pt(rw, *p)).collect();
    create(rw, kind, pts, "Auto route")
}

/// A test network around the demo river.
fn demo_network(rw: &mut RoadWorld) {
    let Some(river) = rw.water.rivers.first().cloned() else {
        warn!("ROADS demo network: no river in this world");
        return;
    };
    // where the river has water, every 5 m along it
    let line: Vec<Vec2> = river.points.iter().map(|p| Vec2::new(p[0], p[2])).collect();
    let mut wet: Vec<(Vec2, Vec2)> = vec![];
    for w in line.windows(2) {
        let n = (w[0].distance(w[1]) / 5.0).ceil().max(1.0) as usize;
        let d = (w[1] - w[0]).normalize_or(Vec2::X);
        for k in 0..n {
            let p = w[0].lerp(w[1], k as f32 / n as f32);
            let g = rw.terrain.height_at(p.x, p.y);
            if rw
                .water
                .water_level(Vec3::new(p.x, g, p.y))
                .is_some_and(|l| l > g + 0.5)
                && wet.last().is_none_or(|l| l.0.distance(p) > 4.0)
            {
                wet.push((p, d));
            }
        }
    }
    if wet.len() < 4 {
        warn!("ROADS demo network: the river is dry");
        return;
    }
    info!("ROADS demo river: {} wet points", wet.len());
    let n = wet.len();
    let at = |f: f32| {
        let (p, d) = wet[((n as f32 * f) as usize).min(n - 1)];
        (p, d, d.perp())
    };
    // cobbled road across the river (stone bridge)
    let (p1, d1, n1) = at(0.3);
    let a = [
        p1 - n1 * 150.0 + d1 * 25.0,
        p1 - n1 * 55.0,
        p1 + n1 * 55.0,
        p1 + n1 * 150.0 - d1 * 30.0,
    ];
    info!("ROADS demo cobble crossing at {:.0},{:.0}", p1.x, p1.y);
    create(
        rw,
        RoadKind::Cobblestone,
        a.iter().map(|p| ground_pt(rw, *p)).collect(),
        "Draw road",
    );
    // dirt road across further down (timber bridge)
    let (p2, d2, n2) = at(0.8);
    let b = [
        p2 - n2 * 120.0 - d2 * 20.0,
        p2 - n2 * 30.0,
        p2 + n2 * 30.0,
        p2 + n2 * 120.0 + d2 * 15.0,
    ];
    info!("ROADS demo dirt crossing at {:.0},{:.0}", p2.x, p2.y);
    create(
        rw,
        RoadKind::DirtRoad,
        b.iter().map(|p| ground_pt(rw, *p)).collect(),
        "Draw road",
    );
    // gravel branch off the cobbled road (T junction)
    let j = p1 + n1 * 110.0 - d1 * 12.0;
    let base = super::ground::Base {
        t: &rw.terrain,
        g: &rw.roads.ground,
    };
    let cob = super::plan::plan_all(&rw.roads, &base, &rw.water)
        .into_iter()
        .find(|p| p.kind == RoadKind::Cobblestone);
    let j = cob
        .and_then(|c| c.nearest(j, 40.0).map(|(_, s)| c.point_at(s)))
        .unwrap_or(j);
    let c = [j, j + d1 * 70.0 + n1 * 25.0, j + d1 * 160.0 + n1 * 10.0];
    info!("ROADS demo junction at {:.0},{:.0}", j.x, j.y);
    create(
        rw,
        RoadKind::Gravel,
        c.iter().map(|p| ground_pt(rw, *p)).collect(),
        "Draw road",
    );
    // footpath from the dirt road's end to the gravel road's end
    let d = [b[3], (b[3] + c[2]) * 0.5 + n1 * 30.0, c[2]];
    create(
        rw,
        RoadKind::Footpath,
        d.iter().map(|p| ground_pt(rw, *p)).collect(),
        "Draw road",
    );
    // forest trail off the cobbled road's start
    let e = [
        a[0],
        a[0] - n1 * 60.0 + d1 * 50.0,
        a[0] - n1 * 90.0 + d1 * 130.0,
    ];
    create(
        rw,
        RoadKind::Trail,
        e.iter().map(|p| ground_pt(rw, *p)).collect(),
        "Draw road",
    );
    // a paved highway routed from the cobbled road's far end into the hills
    let from = a[3];
    let to = from + n1 * 380.0 - d1 * 250.0;
    let to = Vec2::new(
        to.x.clamp(-rw.terrain.half() + 50.0, rw.terrain.half() - 50.0),
        to.y.clamp(-rw.terrain.half() + 50.0, rw.terrain.half() - 50.0),
    );
    route(rw, RoadKind::PavedStone, from, to);
}

/// Scripted roads (see the module docs).
fn scripted(mut frame: Local<u32>, mut rw: RoadWorld, mut tool: ResMut<super::RoadTool>) {
    *frame += 1;
    // `WB_ROAD_TEST=remove`: remove every road again (the checks compare with the world before)
    if *frame == 330
        && std::env::var("WB_ROAD_TEST").is_ok_and(|t| t.split(',').any(|x| x == "remove"))
    {
        let ids: Vec<u32> = rw.roads.roads.iter().map(|r| r.id).collect();
        for id in ids {
            let r = rw.apply(Edit::Remove(id), "Remove road");
            info!("ROADS TEST remove {id} -> {r:?}");
        }
    }
    if *frame != 200 {
        return;
    }
    if let Ok(m) = std::env::var("WB_ROAD_MODE") {
        use super::RoadToolMode as M;
        tool.mode = match m.as_str() {
            "route" => M::Route,
            "edit" => M::Edit,
            "remove" => M::Remove,
            _ => M::Draw,
        };
    }
    if let Ok(spec) = std::env::var("WB_ROAD_DRAFT") {
        let (kind, pts) = spec.split_once(':').unwrap_or(("dirt", &spec));
        if let Some(k) = RoadKind::parse(kind) {
            tool.kind = k;
            tool.width = k.default_width();
        }
        tool.draft = pts
            .split(';')
            .filter_map(|p| {
                let v: Vec<f32> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (v.len() == 2).then(|| Vec3::from_array(ground_pt(&rw, Vec2::new(v[0], v[1]))))
            })
            .collect();
    }
    if let Ok(spec) = std::env::var("WB_ROADS") {
        if spec == "demo" {
            demo_network(&mut rw);
        } else {
            for one in spec.split('|').filter(|s| !s.trim().is_empty()) {
                let (kind, pts) = one.split_once(':').unwrap_or(("dirt", one));
                let Some(kind) = RoadKind::parse(kind) else {
                    warn!("ROADS unknown kind {kind}");
                    continue;
                };
                let pts: Vec<[f32; 3]> = pts
                    .split(';')
                    .filter_map(|p| {
                        let v: Vec<f32> =
                            p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        (v.len() == 2).then(|| ground_pt(&rw, Vec2::new(v[0], v[1])))
                    })
                    .collect();
                create(&mut rw, kind, pts, "Draw road");
            }
        }
    }
    if let Ok(spec) = std::env::var("WB_ROAD_ROUTE") {
        for one in spec.split('|').filter(|s| !s.trim().is_empty()) {
            let (kind, v) = one.split_once(':').unwrap_or(("dirt", one));
            let kind = RoadKind::parse(kind).unwrap_or(RoadKind::DirtRoad);
            let v: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() == 4 {
                route(&mut rw, kind, Vec2::new(v[0], v[1]), Vec2::new(v[2], v[3]));
            }
        }
    }
    if let Ok(id) = std::env::var("WB_ROAD_SEL") {
        rw.roads.selected = id.parse().ok();
    }
    let base = super::ground::Base {
        t: &rw.terrain,
        g: &rw.roads.ground,
    };
    let plans = super::plan::plan_all(&rw.roads, &base, &rw.water);
    for r in &rw.roads.roads {
        let p = r
            .points
            .iter()
            .map(|p| format!("{:.0},{:.0}", p[0], p[2]))
            .collect::<Vec<_>>()
            .join(";");
        let grade = plans.iter().find(|pl| pl.id == r.id).map(|pl| {
            // where the steepest 5 m are
            let k = 5;
            let worst = (0..pl.h.len().saturating_sub(k))
                .map(|i| {
                    (
                        (pl.h[i + k] - pl.h[i]).abs()
                            / (pl.samples[i + k].s - pl.samples[i].s).max(0.1),
                        i,
                    )
                })
                .fold((0.0f32, 0usize), |a, b| if b.0 > a.0 { b } else { a });
            format!(
                "max grade {:.0}% at s {:.0} (spans {:?})",
                worst.0 * 100.0,
                pl.samples[worst.1].s,
                pl.spans
                    .iter()
                    .map(|sp| (pl.samples[sp.i0].s as i32, pl.samples[sp.i1].s as i32))
                    .collect::<Vec<_>>()
            )
        });
        info!(
            "ROADS road {} {} width {} cleared {} lamps {} {} points {p}",
            r.id,
            r.kind.name(),
            r.width,
            r.cleared.len(),
            r.prop_ids.len(),
            grade.unwrap_or_default()
        );
    }
}

/// `WB_ROAD_TEST`: undo / redo the scripted roads, or save, wipe and reload the world, and check
/// that heights, objects and roads come back exactly (`ROADS TEST … PASS / FAIL` lines).
fn scripted_checks(
    mut frame: Local<u32>,
    mut base: Local<Option<(f64, usize, usize)>>,
    mut built: Local<Option<(f64, usize, usize)>>,
    mut data: crate::io::WorldData,
    vis: Res<RoadVisuals>,
) {
    let Ok(test) = std::env::var("WB_ROAD_TEST") else {
        return;
    };
    *frame += 1;
    let f = *frame;
    let sum = |d: &crate::io::WorldData| {
        (
            d.terrain.heights.iter().map(|h| *h as f64).sum::<f64>(),
            d.store.count(),
            d.roads.roads.len(),
        )
    };
    if f == 195 {
        *base = Some(sum(&data));
    }
    if f == 320 {
        *built = Some(sum(&data));
        info!(
            "ROADS TEST built: roads {} objects {} ground vertices {}",
            data.roads.roads.len(),
            data.store.count(),
            data.roads.ground.covered_count()
        );
    }
    let (b0, b1) = ((*base).unwrap_or_default(), (*built).unwrap_or_default());
    let check = |name: &str, want: (f64, usize, usize), got: (f64, usize, usize)| {
        let ok = (want.0 - got.0).abs() < 0.05 && want.1 == got.1 && want.2 == got.2;
        info!(
            "ROADS TEST {name} {} (heights {:.3} vs {:.3}, objects {} vs {}, roads {} vs {})",
            if ok { "PASS" } else { "FAIL" },
            got.0,
            want.0,
            got.1,
            want.1,
            got.2,
            want.2
        );
    };
    let step = |d: &mut crate::io::WorldData, redo: bool| {
        let crate::io::WorldData {
            history,
            terrain,
            store,
            water,
            zones,
            gameplay,
            roads,
            ..
        } = d;
        history.step(redo, terrain, store, water, zones, gameplay, roads)
    };
    for test in test.split(',') {
        match (test, f) {
            ("undo", 330) => {
                let n = b1.2 - b0.2;
                for _ in 0..n {
                    let l = step(&mut data, false);
                    info!("ROADS TEST undo {l:?}");
                }
            }
            ("undo", 400) => {
                check("undo", b0, sum(&data));
                let n = b1.2 - b0.2;
                for _ in 0..n {
                    step(&mut data, true);
                }
            }
            ("undo", 470) => check("redo", b1, sum(&data)),
            ("undo", 540) => info!(
                "ROADS TEST visuals after redo: {} roads drawn",
                vis.plans.len()
            ),
            ("remove", 400) => {
                check("remove", b0, sum(&data));
                info!(
                    "ROADS TEST remove: ground vertices left {}",
                    data.roads.ground.covered_count()
                );
            }
            ("io", 330) => {
                let dir = std::path::PathBuf::from(
                    std::env::var("WB_ROAD_TEST_DIR").unwrap_or_else(|_| ".".into()),
                )
                .join("roads_test.world");
                let covered = data.roads.ground.covered_count();
                match crate::io::save_world(&dir, &data) {
                    Ok(()) => info!("ROADS TEST save PASS {}", dir.display()),
                    Err(e) => return error!("ROADS TEST save FAIL {e}"),
                }
                crate::io::new_world(
                    &mut data,
                    crate::terrain::worldgen::WorldPreset::Ocean,
                    256,
                    4.0,
                    1,
                );
                match crate::io::load_world(&dir, &mut data) {
                    Ok(()) => info!("ROADS TEST load PASS"),
                    Err(e) => return error!("ROADS TEST load FAIL {e}"),
                }
                check("io", b1, sum(&data));
                let back = data.roads.ground.covered_count();
                info!(
                    "ROADS TEST io ground {} ({} vs {})",
                    if back == covered { "PASS" } else { "FAIL" },
                    back,
                    covered
                );
            }
            ("io", 420) => info!(
                "ROADS TEST visuals after load: {} roads drawn",
                vis.plans.len()
            ),
            // ("surface": the ribbon was checked against the old fixed terrain chunks, which the
            // quadtree terrain replaced; at full detail its triangulation is the same as
            // `mesh::surface_height`)
            ("mask", 300) => {
                // the helpers other systems use: the road mask and the exporter ribbon
                let mask = super::mesh::road_mask(&data.roads, &data.terrain, 1024);
                let covered = mask.coverage.iter().filter(|c| **c > 127).count();
                let px = data.terrain.size() / 1024.0;
                let area: f32 = data.roads.roads.iter().map(|r| r.length() * r.width).sum();
                info!(
                    "ROADS TEST mask: {covered} px ≈ {:.0} m² (roads ≈ {area:.0} m²)",
                    covered as f32 * px * px
                );
                if let Some(r) = data.roads.roads.first() {
                    let kind = mask.kind_at(&data.terrain, r.points[0][0], r.points[0][2]);
                    let mesh = super::mesh::road_ribbon_mesh(
                        &data.roads,
                        r.id,
                        &data.terrain,
                        &data.water,
                    );
                    info!(
                        "ROADS TEST mask kind at road {} start: {kind:?}; ribbon mesh {} vertices",
                        r.id,
                        mesh.map(|m| m.count_vertices()).unwrap_or(0)
                    );
                }
                if let Ok(dir) = std::env::var("WB_ROAD_TEST_DIR") {
                    let img = image::GrayImage::from_raw(1024, 1024, mask.coverage)
                        .map(|i| i.save(format!("{dir}/road_mask.png")));
                    info!("ROADS TEST mask image {:?}", img.map(|r| r.is_ok()));
                }
            }
            _ => {}
        }
    }
}

/// One screenshot viewpoint: name, focus, yaw, pitch, distance.
type View = (String, Vec3, f32, f32, f32);

/// Yaw of an orbit camera looking along `dir` (xz).
fn yaw_along(dir: Vec2) -> f32 {
    (-dir.x).atan2(-dir.y)
}

/// Viewpoints derived from the roads (see the module docs).
fn views(vis: &RoadVisuals, t: &crate::terrain::Terrain) -> Vec<View> {
    let mut out: Vec<View> = vec![];
    let plans = &vis.plans;
    if plans.is_empty() {
        return out;
    }
    let ground = |p: Vec2| Vec3::new(p.x, t.height_at(p.x, p.y), p.y);
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in plans {
        for s in &p.samples {
            lo = lo.min(s.p);
            hi = hi.max(s.p);
        }
    }
    let c = (lo + hi) * 0.5;
    out.push((
        "overview".into(),
        ground(c),
        0.5,
        0.95,
        (hi - lo).max_element() * 0.95,
    ));
    out.push((
        "overview_low".into(),
        ground(c),
        2.2,
        0.42,
        (hi - lo).max_element() * 0.7,
    ));
    for p in plans.iter() {
        for (k, sp) in p.spans.iter().enumerate() {
            let mid = (sp.i0 + sp.i1) / 2;
            let s = p.samples[mid];
            let len = p.samples[sp.i1].s - p.samples[sp.i0].s;
            let side = s.dir.perp();
            let name = format!("bridge_{}_{k}", format!("{:?}", p.kind).to_lowercase());
            out.push((
                format!("{name}_side"),
                ground(s.p),
                yaw_along(side) + 0.35,
                0.16,
                len * 0.75 + 14.0,
            ));
            out.push((
                format!("{name}_deck"),
                ground(p.samples[sp.i0.saturating_sub(12)].p),
                yaw_along(s.dir),
                0.12,
                12.0,
            ));
            out.push((
                format!("{name}_quarter"),
                ground(s.p),
                yaw_along(side) - 0.7,
                0.3,
                len * 0.9 + 18.0,
            ));
        }
    }
    // where roads join
    for p in plans.iter() {
        for e in [p.samples.first(), p.samples.last()].into_iter().flatten() {
            if plans
                .iter()
                .any(|o| o.id != p.id && o.nearest(e.p, o.hw + 1.0).is_some())
            {
                let back = if std::ptr::eq(e, &p.samples[0]) {
                    e.dir
                } else {
                    -e.dir
                };
                out.push((
                    format!("junction_{}_{}", p.id, out.len()),
                    ground(e.p),
                    yaw_along(back) + 0.4,
                    0.7,
                    22.0 + p.hw * 2.0,
                ));
            }
        }
    }
    for p in plans.iter() {
        let n = p.samples.len();
        let i = (n as f32 * 0.35) as usize;
        if i + 25 >= n {
            continue;
        }
        let k = format!("{:?}_{}", p.kind, p.id).to_lowercase();
        // eye level, looking along the road
        out.push((
            format!("walk_{k}"),
            ground(p.samples[i + 14].p),
            yaw_along(p.samples[i].dir),
            0.1,
            14.0,
        ));
        // the edge from above
        let s = p.samples[i];
        out.push((
            format!("edge_{k}"),
            ground(s.p + s.dir.perp() * p.hw),
            yaw_along(s.dir) + 1.2,
            1.05,
            7.0 + p.hw,
        ));
        out.push((
            format!("mid_{k}"),
            ground(s.p),
            yaw_along(s.dir) + 0.6,
            0.45,
            26.0 + p.hw * 3.0,
        ));
    }
    out
}

/// `WB_ROAD_SHOTS=<dir>`: the screenshot sequence.
fn shots(
    mut frame: Local<u32>,
    mut list: Local<Vec<View>>,
    mut commands: Commands,
    vis: Res<RoadVisuals>,
    terrain: Res<crate::terrain::Terrain>,
    mut orbit: ResMut<crate::camera::OrbitCamera>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(dir) = std::env::var("WB_ROAD_SHOTS") else {
        return;
    };
    *frame += 1;
    const START: u32 = 330;
    const STEP: u32 = 70;
    let f = *frame;
    if f == START {
        *list = views(&vis, &terrain);
        if let Ok(only) = std::env::var("WB_ROAD_SHOTS_ONLY") {
            list.retain(|v| only.split(',').any(|o| v.0.starts_with(o)));
        }
        info!("ROADS shots: {} views", list.len());
    }
    if f < START {
        return;
    }
    let k = ((f - START) / STEP) as usize;
    let phase = (f - START) % STEP;
    if k >= list.len() {
        if phase == 20 {
            exit.write(AppExit::Success);
        }
        return;
    }
    let (name, focus, yaw, pitch, dist) = list[k].clone();
    if phase == 0 {
        orbit.set_view(focus, yaw, pitch, dist);
        orbit.snap();
    }
    if phase == STEP - 1 {
        let path = format!("{dir}/{name}.png");
        info!("ROADS shot {path}");
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}

/// `WB_ROAD_WALK=<dir>`: walk over the first bridge (with `WB_RUN=jog`).
#[allow(clippy::too_many_arguments)]
fn walk_test(
    mut frame: Local<u32>,
    mut commands: Commands,
    vis: Res<RoadVisuals>,
    terrain: Res<crate::terrain::Terrain>,
    floors: Res<RoadFloors>,
    mut walk: ResMut<crate::walk::WalkState>,
    mut orbit: ResMut<crate::camera::OrbitCamera>,
    mut actions: MessageWriter<crate::ui::UiAction>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(dir) = std::env::var("WB_ROAD_WALK") else {
        return;
    };
    *frame += 1;
    let f = *frame;
    let kind = std::env::var("WB_ROAD_WALK_KIND")
        .ok()
        .and_then(|k| RoadKind::parse(&k));
    let Some((p, sp)) = vis
        .plans
        .iter()
        .filter(|p| kind.is_none_or(|k| p.kind == k))
        .find_map(|p| p.spans.first().map(|sp| (p.clone(), sp.clone())))
    else {
        if f == 400 {
            warn!("ROADS walk: no bridge");
            exit.write(AppExit::Success);
        }
        return;
    };
    if f == 330 {
        // start on the road ~25 m before the bridge, facing it
        let i = sp.i0.saturating_sub(25);
        let s = p.samples[i];
        let start = Vec3::new(s.p.x, terrain.height_at(s.p.x, s.p.y), s.p.y);
        orbit.set_view(start, yaw_along(s.dir), 0.3, 20.0);
        orbit.snap();
    }
    if f == 340 {
        actions.write(crate::ui::UiAction::ToggleWalk);
    }
    // steer along the road (the walker only holds forward)
    if walk.active
        && let Some((_, s)) = p.nearest(walk.position.xz(), 30.0)
    {
        let ahead = p.point_at(s + 4.0) - walk.position.xz();
        if ahead.length() > 0.5 {
            walk.yaw = yaw_along(ahead.normalize());
        }
    }
    if walk.active && f.is_multiple_of(10) {
        let q = walk.position;
        let deck = floors.floor_at(q).map(|d| d.0);
        let g = terrain.height_at(q.x, q.z);
        let near = p.nearest(q.xz(), 50.0).map(|(d, s)| (d, s, p.height_at(s)));
        info!(
            "ROADS walk f{f} pos {:.1},{:.2},{:.1} ground {g:.2} deck {deck:?} road {near:?}",
            q.x, q.y, q.z
        );
    }
    let shots_at = [460u32, 560, 660, 760, 860];
    if let Some(k) = shots_at.iter().position(|x| *x == f) {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(format!("{dir}/walk_{k}.png")));
    }
    if f == 900 {
        exit.write(AppExit::Success);
    }
}
