//! Test hooks for the analysis (environment variables, used with `WB_SHOT` runs):
//!
//! - `WB_ANALYZE=walk|travel|gaps|flow|reach|doctor` opens the Analyze tab on that view at frame
//!   260 (`doctor` also runs the checks).
//! - `WB_ANALYSIS_SEED=1` adds synthetic test content once the demo is built (frame 220): a
//!   player start, POIs (one under water, one unnamed, one on a sheer mesa), NPCs (one in the
//!   sea), spawn regions (one on the steepest mountainside), resource nodes, two roads, an
//!   unnamed high-level zone bordering the starter zone (needs `WB_DEMO=1,zones`), a floating
//!   boulder, a buried and a drowned tree, two cliff rocks inside each other, a chunk crammed
//!   with plants, and a pit with sheer walls.
//! - `WB_ANALYSIS_SRC=x,z` measures travel time from that point.
//! - `WB_ANALYSIS_GOTO=gap:N|flow:N|issue:N|pocket:N|x,z,dist` frames the camera once results are in.
//! - `WB_ANALYSIS_SEQ=view@x,z,yaw,pitch,dist;…` (with `WB_ANALYSIS_SEQ_DIR`) screenshots each
//!   view in turn once results are in, then quits.
//! - `WB_ANALYSIS_UNDO=frame[,frame…]` presses Undo at those frames.
//! - `WB_ANALYSIS_SLOPE=deg` sets the max walk slope at frame 2 (with `WB_IOTEST`: checks the
//!   setting survives save / load).

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use serde_json::json;

use super::{Analysis, TravelSource, View};
use crate::editor::Tab;
use crate::gameplay::Gameplay;
use crate::objects::Placed;

pub fn register(app: &mut App) {
    if std::env::vars().any(|(k, _)| k.starts_with("WB_ANALY")) {
        app.add_systems(
            Update,
            (
                seed.after(crate::debug::demo_content),
                open,
                goto,
                sequence,
                undo,
            ),
        );
    }
}

fn frame_counter(f: &mut u32) -> u32 {
    *f += 1;
    *f
}

fn open(mut frame: Local<u32>, mut ed: ResMut<crate::editor::Editor>, mut a: ResMut<Analysis>) {
    let f = frame_counter(&mut frame);
    if f == 2
        && let Some(v) = std::env::var("WB_ANALYSIS_SLOPE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
    {
        a.settings.max_walk_slope = v;
    }
    if f != 260 {
        return;
    }
    if let Ok(v) = std::env::var("WB_ANALYZE")
        && let Some(view) = View::ALL.into_iter().find(|x| x.id() == v)
    {
        ed.tab = Tab::Analyze;
        a.view = view;
        a.want_doctor |= view == View::Doctor;
        info!("ANALYSIS hook: opened {view:?}");
    }
    if let Ok(s) = std::env::var("WB_ANALYSIS_SRC") {
        let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if v.len() == 2 {
            a.settings.source = TravelSource::Point([v[0], v[1]]);
        }
    }
}

fn undo(mut frame: Local<u32>, mut out: MessageWriter<crate::tools::EditorAction>) {
    let f = frame_counter(&mut frame);
    if let Ok(s) = std::env::var("WB_ANALYSIS_UNDO")
        && s.split(',').any(|x| x.trim().parse::<u32>() == Ok(f))
    {
        info!("ANALYSIS hook: undo at frame {f}");
        out.write(crate::tools::EditorAction::Undo);
    }
}

/// Results for every part are in and nothing is running.
fn settled(a: &Analysis) -> bool {
    a.running.is_none()
        && a.parts.grid.is_some()
        && a.parts.travel.is_some()
        && a.parts.gaps.is_some()
        && a.parts.flow.is_some()
        && (!a.want_doctor || a.parts.doctor.is_some())
}

fn focus_on(orbit: &mut crate::camera::OrbitCamera, p: Vec3, dist: f32) {
    orbit.set_view(p, orbit.yaw, 0.95, dist);
    orbit.snap();
}

fn goto(
    mut done: Local<bool>,
    mut frame: Local<u32>,
    a: Res<Analysis>,
    mut orbit: ResMut<crate::camera::OrbitCamera>,
) {
    let f = frame_counter(&mut frame);
    let Ok(spec) = std::env::var("WB_ANALYSIS_GOTO") else {
        return;
    };
    if *done || f < 280 || !settled(&a) {
        return;
    }
    *done = true;
    let (what, n) = spec
        .split_once(':')
        .map(|(a, b)| (a, b.parse::<usize>().unwrap_or(1).max(1) - 1))
        .unwrap_or((spec.as_str(), 0));
    let target = match what {
        "gap" => a
            .parts
            .gaps
            .as_ref()
            .and_then(|g| g.1.regions.get(n).map(|r| r.pos)),
        "flow" => a.parts.flow.as_ref().and_then(|f| {
            f.1.borders
                .iter()
                .filter(|b| b.flagged)
                .flat_map(|b| b.segments.iter())
                .nth(n)
                .map(|s| s.pos)
        }),
        "issue" => a.report().and_then(|r| r.issues.get(n).map(|i| i.pos)),
        "pocket" => a
            .parts
            .reach
            .as_ref()
            .and_then(|r| r.1.pockets.get(n).map(|p| p.pos)),
        _ => {
            let v: Vec<f32> = spec
                .split(',')
                .filter_map(|x| x.trim().parse().ok())
                .collect();
            (v.len() >= 2).then(|| Vec3::new(v[0], 0.0, v[1]))
        }
    };
    if let Some(p) = target {
        info!("ANALYSIS hook: goto {spec} -> {p:.0}");
        focus_on(&mut orbit, p, 420.0);
    }
}

/// One screenshot per view once results are in.
fn sequence(
    mut state: Local<(u32, Option<u32>)>,
    mut commands: Commands,
    mut a: ResMut<Analysis>,
    mut ed: ResMut<crate::editor::Editor>,
    mut orbit: ResMut<crate::camera::OrbitCamera>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(seq) = std::env::var("WB_ANALYSIS_SEQ") else {
        return;
    };
    let steps: Vec<&str> = seq.split(';').filter(|s| !s.trim().is_empty()).collect();
    state.0 += 1;
    let f = state.0;
    if f < 300 {
        return;
    }
    ed.tab = Tab::Analyze;
    a.want_doctor = true;
    // start once everything is computed
    let Some(start) = state.1 else {
        if settled(&a) {
            state.1 = Some(f);
        }
        return;
    };
    const STEP: u32 = 70;
    let k = ((f - start) / STEP) as usize;
    let phase = (f - start) % STEP;
    if k >= steps.len() {
        if phase == 20 {
            exit.write(AppExit::Success);
        }
        return;
    }
    let parts: Vec<&str> = steps[k].split('@').collect();
    if phase == 0 {
        if let Some(v) = View::ALL.into_iter().find(|v| v.id() == parts[0].trim()) {
            a.view = v;
        }
        if let Some(c) = parts.get(1) {
            let v: Vec<f32> = c.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if v.len() == 5 {
                orbit.set_view(Vec3::new(v[0], 0.0, v[1]), v[2], v[3], v[4]);
                orbit.snap();
            }
        }
    }
    if phase == STEP - 1 {
        let dir = std::env::var("WB_ANALYSIS_SEQ_DIR").unwrap_or_else(|_| ".".into());
        let path = format!("{dir}/{k:02}_{}.png", parts[0].trim());
        info!("ANALYSIS hook: shot {path}");
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}

/// Synthetic gameplay, roads and planted problems for testing (see the module doc).
fn seed(mut frame: Local<u32>, mut done: Local<bool>, mut data: crate::io::WorldData) {
    let f = frame_counter(&mut frame);
    if *done
        || f < 220
        || std::env::var("WB_ANALYSIS_SEED").is_err()
        || data.meta.name != "Demo World"
    {
        return;
    }
    *done = true;
    let t0 = std::time::Instant::now();
    let half = data.terrain.half();
    let camp = Vec2::new(-0.2, 0.22) * half;
    let hgt = |t: &crate::terrain::Terrain, p: Vec2| t.height_at(p.x, p.y);
    let p3 = |t: &crate::terrain::Terrain, p: Vec2| [p.x, t.height_at(p.x, p.y), p.y];

    // a sea spot 3–8 m deep (south of the island centre, like the demo's fish)
    let sea = {
        let t = &*data.terrain;
        let mut spot = camp;
        'search: for k in 0..24 {
            let ang = std::f32::consts::FRAC_PI_2 + (k as f32 - 12.0) * 0.06;
            let dir = Vec2::new(ang.cos(), ang.sin());
            let mut r = 200.0;
            while r < half {
                let q = dir * r;
                let g = t.height_at(q.x, q.y);
                if g < t.sea_level - 3.0 && g > t.sea_level - 8.0 {
                    spot = q;
                    break 'search;
                }
                r += 8.0;
            }
        }
        spot
    };
    // the steepest ~120 m patch of land (for a spawn region on a mountainside)
    let steep = {
        let t = &*data.terrain;
        let mut best = (0.0f32, camp);
        let mut z = -half + 200.0;
        while z < half - 200.0 {
            let mut x = -half + 200.0;
            while x < half - 200.0 {
                if t.height_at(x, z) > t.sea_level + 5.0 {
                    let mut s = 0.0;
                    for (dx, dz) in [
                        (0.0, 0.0),
                        (40.0, 0.0),
                        (-40.0, 0.0),
                        (0.0, 40.0),
                        (0.0, -40.0),
                    ] {
                        s += 1.0 - t.normal_at(x + dx, z + dz).y;
                    }
                    if s > best.0 {
                        best = (s, Vec2::new(x, z));
                    }
                }
                x += 60.0;
            }
            z += 60.0;
        }
        best.1
    };
    info!("ANALYSIS seed: camp {camp:.0}, sea {sea:.0}, steep {steep:.0}");

    // ---- terrain: a sheer mesa (unreachable top) and a pit (trap)
    data.history.begin("Analysis test terrain");
    let mesa = camp + Vec2::new(320.0, 160.0);
    let pit = camp + Vec2::new(-180.0, -220.0);
    // the mesa's rim is a cliff; the pit's walls are steep enough to slide down but not climb
    for (c, r, dh, edge) in [(mesa, 48.0f32, 26.0f32, 3.0f32), (pit, 34.0, -18.0, 9.0)] {
        let t = &mut *data.terrain;
        let Some(rect) = t.rect_for_circle(c, r + edge + 4.0) else {
            continue;
        };
        data.history.touch_terrain(t, rect);
        let base = t.height_at(c.x, c.y);
        for j in rect.z0..=rect.z1 {
            for i in rect.x0..=rect.x1 {
                let p = t.grid_to_world(i, j);
                let d = p.distance(c);
                let w = 1.0 - crate::noise::smoothstep(r - edge, r + edge, d);
                if w > 0.0 {
                    let k = t.idx(i, j);
                    let target = base + dh;
                    t.heights[k] = t.heights[k] + (target - t.heights[k]) * w;
                }
            }
        }
        t.mark_heights_dirty(rect);
    }
    // an unnamed level-30 zone on the starter zone's west side
    if data.zones.list.iter().any(|z| z.id == 1) && data.zones.next_id().is_some() {
        data.history.touch_zones(&data.zones);
        let id = data.zones.next_id().unwrap();
        let mut z = crate::world::Zone::new(id, format!("Zone {id}"));
        z.level_min = 30;
        z.level_max = 35;
        data.zones.list.push(z);
        let t = &mut *data.terrain;
        for k in 0..=8 {
            let p = Vec2::new(-0.47, 0.06).lerp(Vec2::new(-0.38, 0.32), k as f32 / 8.0) * half;
            crate::terrain::brush::paint_zone(t, &mut data.history, p, 150.0, id, None);
        }
    }

    // ---- objects with problems
    let put = |data: &mut crate::io::WorldData, asset: &str, p: Vec2, dy: f32, scale: f32| {
        let id = data.store.new_id();
        let y = data.terrain.height_at(p.x, p.y) + dy;
        data.history.touch_object(&data.store, id);
        data.store.insert(Placed {
            group: 0,
            id,
            asset: asset.into(),
            variant: 0,
            pos: [p.x, y, p.y],
            rot: Quat::IDENTITY.to_array(),
            scale,
            stretch: None,
            locked: false,
        });
    };
    put(&mut data, "Boulder", camp + Vec2::new(28.0, 26.0), 3.0, 1.2);
    put(&mut data, "Oak", camp + Vec2::new(-34.0, 22.0), -7.0, 1.0);
    put(&mut data, "Pine", sea + Vec2::new(12.0, 0.0), 0.0, 1.0);
    put(
        &mut data,
        "CliffRock",
        camp + Vec2::new(70.0, 64.0),
        -0.6,
        1.0,
    );
    put(
        &mut data,
        "CliffRock",
        camp + Vec2::new(71.2, 64.8),
        -0.6,
        0.8,
    );
    let c = crate::objects::chunk_of(camp.x + 170.0, camp.y - 150.0);
    let hot = (Vec2::new(c.0 as f32, c.1 as f32) + 0.5) * crate::objects::CHUNK;
    let mut rng = crate::noise::Rng::new(5);
    for k in 0..2800 {
        let p = hot + Vec2::new(rng.range(-55.0, 55.0), rng.range(-55.0, 55.0));
        put(
            &mut data,
            if k % 2 == 0 { "Flowers" } else { "TallGrass" },
            p,
            -0.1,
            1.0,
        );
    }

    // ---- gameplay (built from JSON, so fields other features add later fill in their defaults)
    fn de<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> T {
        serde_json::from_value(v).expect("test content matches the data model")
    }
    let t = &*data.terrain;
    let gp = &mut *data.gameplay;
    gp.player_start = Some(de(
        json!({"pos": p3(t, camp + Vec2::new(8.0, 12.0)), "yaw": 0.0}),
    ));
    let poi = |gp: &mut Gameplay, kind: &str, name: &str, p: [f32; 3]| {
        let id = gp.alloc_id();
        gp.pois.push(de(
            json!({"id": id, "kind": kind, "name": name, "pos": p, "radius": 30.0}),
        ));
    };
    let town = camp + Vec2::new(60.0, -40.0);
    poi(gp, "Town", "Emberwick", p3(t, town));
    poi(
        gp,
        "QuestHub",
        "Old Mill",
        p3(t, camp + Vec2::new(520.0, -60.0)),
    );
    poi(
        gp,
        "Dungeon",
        "Sunken Crypt",
        [sea.x, t.height_at(sea.x, sea.y), sea.y],
    );
    poi(gp, "Landmark", "", p3(t, camp + Vec2::new(-260.0, 160.0)));
    poi(gp, "Vista", "Mesa Lookout", [mesa.x, hgt(t, mesa), mesa.y]);
    poi(
        gp,
        "Camp",
        "Pinewatch Camp",
        p3(t, Vec2::new(0.15, -0.2) * half),
    );
    let npc = |gp: &mut Gameplay, role: &str, name: &str, p: [f32; 3]| {
        let id = gp.alloc_id();
        gp.npcs.push(de(
            json!({"id": id, "name": name, "role": role, "pos": p, "yaw": 0.0}),
        ));
    };
    npc(
        gp,
        "Vendor",
        "Mara the Smith",
        p3(t, town + Vec2::new(6.0, 3.0)),
    );
    npc(gp, "QuestGiver", "", p3(t, town + Vec2::new(-5.0, 6.0)));
    npc(
        gp,
        "Ambient",
        "Drowned Dan",
        [sea.x + 20.0, t.height_at(sea.x + 20.0, sea.y), sea.y],
    );
    let square = |c: Vec2, r: f32| {
        vec![
            [c.x - r, c.y - r],
            [c.x + r, c.y - r],
            [c.x + r, c.y + r],
            [c.x - r, c.y + r],
        ]
    };
    let wolf = gp.alloc_id();
    gp.creatures.push(de(json!({"id": wolf, "name": "Grey Wolf", "family": "wolf", "level": [2, 4], "aggression": "Hostile"})));
    let spawn = |gp: &mut Gameplay, name: &str, poly: Vec<[f32; 2]>| {
        let id = gp.alloc_id();
        gp.spawns.push(de(json!({
            "id": id, "name": name, "polygon": poly, "entries": [{"creature": wolf, "weight": 1.0}],
            "density": 40.0, "respawn_secs": 90.0, "level": [2, 4], "pack": [1, 3], "behaviour": "Roam",
        })));
    };
    spawn(
        gp,
        "Wolf Meadow",
        square(camp + Vec2::new(-120.0, -60.0), 60.0),
    );
    spawn(gp, "Cliff Harpies", square(steep, 70.0));
    spawn(
        gp,
        "Spawn Region",
        square(sea + Vec2::new(0.0, -90.0), 60.0),
    );
    let node = |gp: &mut Gameplay, kind: &str, v: &str, p: [f32; 3]| {
        let id = gp.alloc_id();
        gp.nodes.push(de(json!({"id": id, "kind": kind, "variant": v, "tier": 1, "pos": p, "respawn_secs": 120.0})));
    };
    node(
        gp,
        "Ore",
        "Copper Vein",
        p3(t, camp + Vec2::new(200.0, 120.0)),
    );
    node(
        gp,
        "Herb",
        "Silverleaf",
        p3(t, camp + Vec2::new(-80.0, 90.0)),
    );
    gp.dirty = true;

    // ---- roads: one up into the highlands, one along the starter basin
    let road = |t: &crate::terrain::Terrain, pts: &[Vec2]| {
        pts.iter().map(|p| p3(t, *p)).collect::<Vec<_>>()
    };
    let id = data.roads.alloc_id();
    let pts = road(
        t,
        &[
            camp,
            Vec2::new(-0.12, 0.05) * half,
            Vec2::new(0.05, -0.12) * half,
            Vec2::new(0.15, -0.2) * half,
        ],
    );
    data.roads.roads.push(de(json!({"id": id, "name": "King's Road", "kind": "Cobblestone", "points": pts, "width": 6.0})));
    let id = data.roads.alloc_id();
    let pts = road(
        t,
        &[
            camp + Vec2::new(-200.0, 40.0),
            camp + Vec2::new(0.0, -10.0),
            camp + Vec2::new(250.0, -30.0),
            camp + Vec2::new(520.0, -60.0),
        ],
    );
    data.roads.roads.push(de(
        json!({"id": id, "name": "Mill Lane", "kind": "DirtRoad", "points": pts, "width": 4.5}),
    ));
    data.roads.dirty = true;
    let store = &*data.store;
    data.history.commit(store);
    info!(
        "ANALYSIS seed: done in {:?} ({} objects, {} POIs, {} roads)",
        t0.elapsed(),
        data.store.count(),
        data.gameplay.pois.len(),
        data.roads.roads.len()
    );
}
