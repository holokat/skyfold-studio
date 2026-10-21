//! MCP tools of the Analyze tab. Each computes what's out of date right away (on the main
//! thread), stores the results so the tab shows them too, and returns a short JSON summary:
//! numbers first, then the top items with world positions.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use serde_json::{Value, json};

use super::doctor::{self, Fix, Severity};
use super::job::{self, Progress};
use super::travel::fmt_time;
use super::{Analysis, TravelSource, WorldRefs};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "analyze_walkability",
        description: "Classify the world for someone on foot on a ~2 m grid: walkable, wading, steep (above the max walk slope), cliff, blocked by objects, swimming. Optional overrides (saved with the world): max_slope (deg, default 40), cliff_slope (deg, 60), swim_depth (m, 1.2), objects_block (bool), steep ('slide'|'climb'|'block'). Returns area shares in % and km².",
        input_schema: || json!({"type":"object","properties":{"max_slope":{"type":"number"},"cliff_slope":{"type":"number"},"swim_depth":{"type":"number"},"objects_block":{"type":"boolean"},"steep":{"type":"string","enum":["slide","climb","block"]}}}),
        run: walkability,
    })
    .mcp_tool(McpTool {
        name: "analyze_travel_time",
        description: "Travel time on foot (or mounted) from a start to every walkable spot: Dijkstra over the walk grid, slower uphill and swimming, faster on roads. `from`: 'start' (player start, default), 'poi:<id>', or [x, z] world metres. Optional run_speed (m/s, 6.5), mounted (bool), mount_speed (m/s). Returns median / p90 / max seconds, % reachable and % within 1 / 2 / 5 min, and the farthest point.",
        input_schema: || json!({"type":"object","properties":{"from":{"description":"'start', 'poi:<id>' or [x, z]","oneOf":[{"type":"string"},{"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2}]},"run_speed":{"type":"number"},"mounted":{"type":"boolean"},"mount_speed":{"type":"number"}}}),
        run: travel_time,
    })
    .mcp_tool(McpTool {
        name: "analyze_content_gaps",
        description: "Time on foot to the nearest content (POIs, NPCs, spawn regions, resource nodes, fish schools; roads optional) for all reachable walkable ground. Ground farther than threshold_s (default 45 s) is a dead zone. Returns the dead share and the biggest gaps (position, area, worst time).",
        input_schema: || json!({"type":"object","properties":{"threshold_s":{"type":"number"},"fish":{"type":"boolean"},"roads":{"type":"boolean"}}}),
        run: content_gaps,
    })
    .mcp_tool(McpTool {
        name: "analyze_level_flow",
        description: "Zones that border each other across walkable ground, with the level jump (difference of average levels), open and closed border length, and a suggestion where the jump exceeds threshold (default 5 levels).",
        input_schema: || json!({"type":"object","properties":{"threshold":{"type":"integer"}}}),
        run: level_flow,
    })
    .mcp_tool(McpTool {
        name: "analyze_reachability",
        description: "Flood fill on foot from the player start (or `from` [x, z] when the world has none): reachable share, unreachable walkable pockets, traps (can get in, can't walk out), and POIs / NPCs / spawns / nodes that can't be reached.",
        input_schema: || json!({"type":"object","properties":{"from":{"type":"array","items":{"type":"number"}}}}),
        run: reachability,
    })
    .mcp_tool(McpTool {
        name: "doctor_run",
        description: "Run the World Doctor: floating / buried objects, plants under water, overlapping large objects, spawn regions on steep ground or water, POIs / NPCs under water or unreachable, traps, zones without content, unnamed zones / POIs, gameplay outside zones, roads steeper than 15%, object hotspots per 128 m chunk (budget, default 2500). Returns counts by severity and issues with ids (for doctor_fix).",
        input_schema: || json!({"type":"object","properties":{"budget":{"type":"integer"},"limit":{"type":"integer","description":"max issues listed (default 40)"}}}),
        run: doctor_run,
    })
    .mcp_tool(McpTool {
        name: "doctor_fix",
        description: "Apply the safe fix of a World Doctor issue (snap to ground, remove drowned / overlapping / excess objects, name a zone) as one undoable step. Pass `id` (from doctor_run), or `check` (e.g. 'floating') to fix every issue of that check. Re-runs the doctor afterwards.",
        input_schema: || json!({"type":"object","properties":{"id":{"type":"integer"},"check":{"type":"string"}}}),
        run: doctor_fix,
    })
    .mcp_tool(McpTool {
        name: "navmesh_export",
        description: "Export a navmesh from the walkability grid into `dir`: navmesh.obj, navmesh.json (vertices, polygons, areas walk / road / swim, costs, adjacency), walkable_heightfield.png (16-bit) and walkable_areas.png. Returns polygon / vertex counts and connectivity.",
        input_schema: || json!({"type":"object","properties":{"dir":{"type":"string"}},"required":["dir"]}),
        run: navmesh_export,
    });
}

/// Computes every stale part now (blocking).
pub fn ensure(world: &mut World, doctor: bool) -> Result<(), String> {
    let mut state: SystemState<(WorldRefs, Res<crate::camera::OrbitCamera>, ResMut<Analysis>)> =
        SystemState::new(world);
    let (out, fps) = {
        let (w, orbit, mut a) = state.get_mut(world).map_err(|e| e.to_string())?;
        if w.gameplay.player_start.is_none() && a.fallback_origin.is_none() {
            a.fallback_origin = Some(orbit.focus.xz());
        }
        a.want_doctor |= doctor;
        let (origin, source) = a.origins(&w.gameplay);
        let inputs = w.inputs();
        let fps = job::fingerprints(&inputs, &a.settings, &origin, &source);
        let stale = a.parts.stale(&fps, doctor);
        if !stale.iter().any(|s| *s) {
            return Ok(());
        }
        let snap = inputs.snapshot(&a.settings, origin, source, stale[5]);
        (
            job::compute(&snap, &a.parts, fps, doctor, &Progress::default()),
            fps,
        )
    };
    let mut a = world.resource_mut::<Analysis>();
    if out.parts.doctor.as_ref().map(|d| d.0) != a.parts.doctor.as_ref().map(|d| d.0) {
        a.fixed.clear();
    }
    a.parts = out.parts;
    a.fps = fps;
    let t: Vec<String> = out
        .timings
        .iter()
        .map(|(n, ms)| format!("{n} {ms:.0} ms"))
        .collect();
    info!("ANALYSIS (mcp) {}", t.join(", "));
    a.timings = out.timings;
    Ok(())
}

fn settings(world: &mut World, f: impl FnOnce(&mut super::AnalysisSettings)) {
    let mut a = world.resource_mut::<Analysis>();
    let before = a.settings.clone();
    f(&mut a.settings);
    if a.settings != before {
        a.settings_changed = true;
    }
}

fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}
fn r3(v: f32) -> f64 {
    (v as f64 * 1000.0).round() / 1000.0
}
fn xz(p: Vec3) -> Value {
    json!([r1(p.x), r1(p.z)])
}
fn timings(a: &Analysis) -> Value {
    a.timings
        .iter()
        .map(|(n, ms)| (n.to_string(), json!(ms.round())))
        .collect::<serde_json::Map<_, _>>()
        .into()
}
fn ok(v: Value) -> Result<Vec<McpContent>, String> {
    Ok(vec![McpContent::Json(v)])
}

fn walkability(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let (ms, cs, sd, ob, steep) = (
        opt_arg::<f32>(&args, "max_slope")?,
        opt_arg::<f32>(&args, "cliff_slope")?,
        opt_arg::<f32>(&args, "swim_depth")?,
        opt_arg::<bool>(&args, "objects_block")?,
        opt_arg::<String>(&args, "steep")?,
    );
    settings(world, |s| {
        if let Some(v) = ms {
            s.max_walk_slope = v.clamp(5.0, 85.0);
        }
        if let Some(v) = cs {
            s.cliff_slope = v.clamp(10.0, 89.0);
        }
        if let Some(v) = sd {
            s.swim_depth = v.clamp(0.3, 5.0);
        }
        if let Some(v) = ob {
            s.objects_block = v;
        }
        match steep.as_deref() {
            Some("climb") => s.steep = super::SteepRule::Climb,
            Some("block") => s.steep = super::SteepRule::Block,
            Some("slide") => s.steep = super::SteepRule::SlideDown,
            _ => {}
        }
    });
    ensure(world, false)?;
    let a = world.resource::<Analysis>();
    let g = a.grid().ok_or("no grid")?;
    let c = g.counts();
    let inside: usize = c[..7].iter().sum();
    let km2 = g.cell_area_km2();
    let share = |k: usize| json!({"pct": r1(c[k] as f32 * 100.0 / inside.max(1) as f32), "km2": r3((c[k] as f64 * km2) as f32)});
    let roads = g
        .road
        .iter()
        .zip(&g.class)
        .filter(|(r, c)| **r > 0 && c.land())
        .count();
    ok(json!({
        "grid": {"cells": g.len(), "side": g.n, "cell_m": r3(g.cell)},
        "rules": {"max_walk_slope_deg": a.settings.max_walk_slope, "cliff_deg": a.settings.cliff_slope, "steep": format!("{:?}", a.settings.steep), "swim_depth_m": a.settings.swim_depth, "objects_block": a.settings.objects_block},
        "walkable": share(0), "wading": share(1), "steep": share(2), "cliff": share(3), "blocked_by_objects": share(4), "swim": share(5), "open_sea": share(6),
        "walkable_share_of_land_pct": r1((c[0] + c[1]) as f32 * 100.0 / c[..5].iter().sum::<usize>().max(1) as f32),
        "road_km2": r3((roads as f64 * km2) as f32),
        "ms": timings(a),
    }))
}

fn travel_time(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let from = args.get("from").cloned();
    let (rs, mounted, msp) = (
        opt_arg::<f32>(&args, "run_speed")?,
        opt_arg::<bool>(&args, "mounted")?,
        opt_arg::<f32>(&args, "mount_speed")?,
    );
    let src = match &from {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s == "start" => Some(TravelSource::PlayerStart),
        Some(Value::String(s)) if s.starts_with("poi:") => Some(TravelSource::Poi(
            s[4..].trim().parse().map_err(|_| "from: poi:<id>")?,
        )),
        Some(Value::Array(v)) if v.len() == 2 => Some(TravelSource::Point([
            v[0].as_f64().ok_or("from: [x, z]")? as f32,
            v[1].as_f64().ok_or("from: [x, z]")? as f32,
        ])),
        Some(Value::Object(o)) => Some(TravelSource::Point([
            o.get("x").and_then(|v| v.as_f64()).ok_or("from: {x, z}")? as f32,
            o.get("z").and_then(|v| v.as_f64()).ok_or("from: {x, z}")? as f32,
        ])),
        _ => return Err("from: 'start', 'poi:<id>' or [x, z]".into()),
    };
    settings(world, |s| {
        if let Some(src) = src {
            s.source = src;
        }
        if let Some(v) = rs {
            s.run_speed = v.clamp(1.0, 30.0);
        }
        if let Some(v) = mounted {
            s.mounted = v;
        }
        if let Some(v) = msp {
            s.mount_speed = v.clamp(1.0, 60.0);
        }
    });
    ensure(world, false)?;
    let a = world.resource::<Analysis>();
    let t = a.parts.travel.as_ref().ok_or("no travel result")?.1.clone();
    if t.source.is_none() {
        return Err("no start: the world has no player start, POI or point to measure from".into());
    }
    ok(json!({
        "from": t.label, "source": t.source.map(xz),
        "median_s": r1(t.median), "p90_s": r1(t.p90), "max_s": r1(t.max), "max": fmt_time(t.max),
        "farthest": t.farthest.map(xz),
        "reachable_pct": r1(t.reachable_pct), "within_1min_pct": r1(t.within[0]), "within_2min_pct": r1(t.within[1]), "within_5min_pct": r1(t.within[2]),
        "speeds": {"run": a.settings.run_speed, "swim": a.settings.swim_speed, "mounted": a.settings.mounted, "mount": a.settings.mount_speed},
        "ms": timings(a),
    }))
}

fn content_gaps(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let (th, fish, roads) = (
        opt_arg::<f32>(&args, "threshold_s")?,
        opt_arg::<bool>(&args, "fish")?,
        opt_arg::<bool>(&args, "roads")?,
    );
    settings(world, |s| {
        if let Some(v) = th {
            s.gap_threshold = v.clamp(5.0, 900.0);
        }
        if let Some(v) = fish {
            s.gap_fish = v;
        }
        if let Some(v) = roads {
            s.gap_roads = v;
        }
    });
    ensure(world, false)?;
    let a = world.resource::<Analysis>();
    let g = a.parts.gaps.as_ref().ok_or("no gap result")?.1.clone();
    let gaps: Vec<Value> = g
        .regions
        .iter()
        .take(8)
        .map(|r| json!({"at": xz(r.pos), "area_km2": r3(r.area_km2), "worst_s": if r.max_s.is_finite() { json!(r1(r.max_s)) } else { json!("unreachable") }}))
        .collect();
    ok(json!({
        "content_points": g.sources, "threshold_s": g.threshold,
        "walkable_km2": r3(g.walk_km2), "dead_km2": r3(g.dead_km2), "dead_pct": r1(g.dead_pct),
        "note": if g.sources == 0 { "no content yet: every spot is a dead zone" } else { "" },
        "biggest_gaps": gaps,
        "ms": timings(a),
    }))
}

fn level_flow(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let th = opt_arg::<u32>(&args, "threshold")?;
    settings(world, |s| {
        if let Some(v) = th {
            s.level_jump = v.clamp(1, 60);
        }
    });
    ensure(world, false)?;
    let zones = job::zone_infos(world.resource::<crate::world::Zones>());
    let a = world.resource::<Analysis>();
    let f = a.parts.flow.as_ref().ok_or("no flow result")?.1.clone();
    let name = |id: u8| {
        zones
            .iter()
            .find(|z| z.id == id)
            .map(|z| format!("{} (Lv {}–{})", z.name, z.level_min, z.level_max))
            .unwrap_or(format!("zone {id}"))
    };
    let borders: Vec<Value> = f
        .borders
        .iter()
        .take(20)
        .map(|b| {
            json!({
                "low": name(b.low), "high": name(b.high), "jump": r1(b.jump), "flagged": b.flagged,
                "open_border_m": b.open_m.round(), "closed_border_m": b.closed_m.round(),
                "crossings": b.segments.iter().take(4).map(|s| json!({"at": xz(s.pos), "len_m": s.len_m.round()})).collect::<Vec<_>>(),
                "suggestion": if b.flagged { json!(super::flow::suggestion(b, &zones, f.threshold)) } else { Value::Null },
            })
        })
        .collect();
    ok(
        json!({"zones": zones.len(), "threshold": f.threshold, "flagged": f.borders.iter().filter(|b| b.flagged).count(), "borders": borders, "ms": timings(a)}),
    )
}

fn reachability(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    if let Some(v) = opt_arg::<[f32; 2]>(&args, "from")? {
        let mut a = world.resource_mut::<Analysis>();
        a.fallback_origin = Some(Vec2::from_array(v));
    }
    ensure(world, false)?;
    let a = world.resource::<Analysis>();
    let r = a.parts.reach.as_ref().ok_or("no reach result")?.1.clone();
    ok(json!({
        "from": r.origin_label, "origin": r.origin.map(xz),
        "walkable_km2": r3(r.land_km2), "reachable_km2": r3(r.reachable_km2), "unreachable_km2": r3(r.unreachable_km2), "trap_km2": r3(r.trap_km2),
        "reachable_pct": r1(r.reachable_km2 * 100.0 / r.land_km2.max(1e-6)),
        "pockets": r.pockets.iter().take(8).map(|p| json!({"at": xz(p.pos), "area_km2": r3(p.area_km2)})).collect::<Vec<_>>(),
        "traps": r.traps.iter().take(8).map(|p| json!({"at": xz(p.pos), "area_m2": (p.area_km2 * 1.0e6).round()})).collect::<Vec<_>>(),
        "unreachable_items": r.items.iter().map(|i| json!({"kind": i.kind, "id": i.id, "name": i.name, "at": xz(i.pos), "why": i.why})).collect::<Vec<_>>(),
        "ms": timings(a),
    }))
}

fn report_json(a: &Analysis, limit: usize) -> Value {
    let Some(rep) = a.report() else {
        return json!({"error": "no report"});
    };
    let live: Vec<&doctor::Issue> = rep
        .issues
        .iter()
        .filter(|i| !a.fixed.contains(&i.id))
        .collect();
    let mut by_check: std::collections::BTreeMap<&str, usize> = Default::default();
    for i in &live {
        *by_check.entry(i.check.id()).or_default() += 1;
    }
    json!({
        "errors": live.iter().filter(|i| i.severity == Severity::Error).count(),
        "warnings": live.iter().filter(|i| i.severity == Severity::Warning).count(),
        "info": live.iter().filter(|i| i.severity == Severity::Info).count(),
        "by_check": by_check,
        "objects_checked": rep.objects,
        "issues": live.iter().take(limit).map(|i| json!({"id": i.id, "check": i.check.id(), "severity": i.severity.id(), "title": i.title, "at": xz(i.pos), "fix": i.fix.as_ref().map(|f| f.label())})).collect::<Vec<_>>(),
        "ms": r1(rep.ms),
    })
}

fn doctor_run(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    if let Some(b) = opt_arg::<u32>(&args, "budget")? {
        settings(world, |s| s.chunk_budget = b.clamp(50, 1_000_000));
    }
    let limit = opt_arg::<usize>(&args, "limit")?.unwrap_or(40);
    ensure(world, true)?;
    ok(report_json(world.resource::<Analysis>(), limit))
}

fn doctor_fix(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id = opt_arg::<u32>(&args, "id")?;
    let check = opt_arg::<String>(&args, "check")?;
    if id.is_none() && check.is_none() {
        return Err("pass `id` or `check`".into());
    }
    ensure(world, true)?;
    let fixes: Vec<(u32, Fix)> = {
        let a = world.resource::<Analysis>();
        let rep = a.report().ok_or("no report")?;
        rep.issues
            .iter()
            .filter(|i| {
                !a.fixed.contains(&i.id)
                    && (Some(i.id) == id || check.as_deref() == Some(i.check.id()))
            })
            .filter_map(|i| i.fix.clone().map(|f| (i.id, f)))
            .collect()
    };
    if fixes.is_empty() {
        return Err("no fixable issue matches".into());
    }
    let msg = {
        let mut state: SystemState<(
            Res<crate::terrain::Terrain>,
            ResMut<crate::objects::ObjectStore>,
            Option<Res<crate::objects::Library>>,
            ResMut<crate::world::Zones>,
            ResMut<crate::history::History>,
            ResMut<crate::world::WorldMeta>,
        )> = SystemState::new(world);
        let (terrain, mut store, lib, mut zones, mut history, mut meta) =
            state.get_mut(world).map_err(|e| e.to_string())?;
        let refs: Vec<&Fix> = fixes.iter().map(|(_, f)| f).collect();
        let msg = doctor::apply(
            &refs,
            &terrain,
            &mut store,
            lib.as_deref(),
            &mut zones,
            &mut history,
        );
        meta.dirty = true;
        msg
    };
    {
        let mut a = world.resource_mut::<Analysis>();
        a.fixed.extend(fixes.iter().map(|(i, _)| *i));
    }
    let fixed = fixes.len();
    ensure(world, true)?;
    let after = report_json(world.resource::<Analysis>(), 10);
    ok(
        json!({"fixed_issues": fixed, "result": msg, "undo": "one undo step (⌘Z / Undo)", "after": after}),
    )
}

fn navmesh_export(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let dir: String = crate::mcp_tools::arg(&args, "dir")?;
    let dir = std::path::PathBuf::from(dir);
    let t = std::time::Instant::now();
    let st = super::export_navmesh(world, &dir)?;
    ok(json!({
        "dir": dir.display().to_string(),
        "files": ["navmesh.obj", "navmesh.json", "walkable_heightfield.png", "walkable_areas.png"],
        "polygons": st.polygons, "vertices": st.vertices, "components": st.components, "largest_component_pct": r1(st.largest_pct),
        "per_area": st.per_area.iter().map(|(a, c)| (a.name().to_string(), json!(c))).collect::<serde_json::Map<_, _>>(),
        "ms": t.elapsed().as_millis(),
    }))
}
