//! The area-prompt measurement harness. `WB_PROMPT_EVAL=<cases.json>` (with `WB_DEMO`, `WB_SHOT`)
//! runs each case — `{"name", "prompt", "polygon": [[x, z], …] | "center" + "radius",
//! "backend"?, "model"?}` — through the real agent backend on the demo world and records:
//! before / after screenshots (top-down and angled), wall time, tool calls, success, the agent's
//! summary, and the edits outside the selection (heights, ground, biomes, objects, water: should
//! all be 0). After each case the run is undone and the world compared again (undo must restore
//! it exactly). Per case: `<out>/<name>/result.json` + PNGs; every launch rebuilds
//! `<out>/report.json` and `<out>/contact_sheet.png` from all case folders, so long evaluations
//! can be split over several launches.
//!
//! A case may add `"refine": "<follow-up prompt>"` (sent once the first run is done) or replace
//! the agent with a `"script": [[tool, {args}], …]` (for testing tools and generators).
//!
//! `WB_PROMPT_EVAL_OUT=<dir>` (default: next to the cases file), `WB_PROMPT_EVAL_ONLY=a,b` runs
//! some cases, `WB_PROMPT_EVAL_JUDGE=1` adds a judge pass: the same backend (Codex) rates the
//! after images against the prompt (0-10, with reasons). `WB_PROMPT_EVAL_START=<frame>` (default
//! 420) waits for the demo world.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use bevy::prelude::*;
use serde_json::{Value, json};

use super::selection::Selection;
use super::{PromptSession, Request, RunState};
use crate::terrain::layers::LAYER_COUNT;

pub fn register(app: &mut App) {
    if std::env::var("WB_PROMPT_EVAL").is_ok() {
        app.add_systems(Update, run_eval.after(crate::tools::ToolSystems::Commit));
    }
}

/// The world as it was before a case (what "outside" edits are measured against).
struct Snapshot {
    heights: Vec<f32>,
    splat: Vec<[u8; LAYER_COUNT]>,
    biomes: Vec<u8>,
    objects: HashMap<u64, (String, [f32; 3])>,
    water: Vec<(String, u32, Vec2, String)>,
}

fn snapshot(world: &World) -> Snapshot {
    let t = world.resource::<crate::terrain::Terrain>();
    let store = world.resource::<crate::objects::ObjectStore>();
    let w = world.resource::<crate::water::WaterBodies>();
    let mut water = vec![];
    for r in &w.rivers {
        let p = r
            .points
            .get(r.points.len() / 2)
            .map(|p| Vec2::new(p[0], p[2]))
            .unwrap_or_default();
        water.push((
            "river".to_string(),
            r.id,
            p,
            serde_json::to_string(r).unwrap_or_default(),
        ));
    }
    for l in &w.lakes {
        water.push((
            "lake".to_string(),
            l.id,
            Vec2::from_array(l.seed),
            serde_json::to_string(l).unwrap_or_default(),
        ));
    }
    Snapshot {
        heights: t.heights.clone(),
        splat: t.splat.clone(),
        biomes: t.biomes.clone(),
        objects: store
            .items
            .iter()
            .map(|(id, p)| (*id, (p.asset.clone(), p.pos)))
            .collect(),
        water,
    }
}

/// Differences between the world now and `s`, outside and inside `poly`.
fn diff(world: &World, s: &Snapshot, poly: &[Vec2]) -> Value {
    let t = world.resource::<crate::terrain::Terrain>();
    let inside_poly = |p: Vec2| crate::biomes::point_in_polygon(p, poly);
    let n = t.n();
    let (mut oh, mut os, mut ob, mut omax) = (0usize, 0usize, 0usize, 0f32);
    let (mut ih, mut isp, mut imax_up, mut imax_down) = (0usize, 0usize, 0f32, 0f32);
    for j in 0..n {
        for i in 0..n {
            let k = j * n + i;
            let dh = t.heights[k] - s.heights[k];
            let ds = t.splat[k] != s.splat[k];
            let db = t.biomes[k] != s.biomes[k];
            if dh == 0.0 && !ds && !db {
                continue;
            }
            if inside_poly(t.grid_to_world(i, j)) {
                ih += (dh.abs() > 0.01) as usize;
                isp += ds as usize;
                imax_up = imax_up.max(dh);
                imax_down = imax_down.min(dh);
            } else {
                oh += (dh != 0.0) as usize;
                os += ds as usize;
                ob += db as usize;
                omax = omax.max(dh.abs());
            }
        }
    }
    let store = world.resource::<crate::objects::ObjectStore>();
    let (mut o_out, mut added_in, mut removed_in) = (0usize, 0usize, 0usize);
    for (id, p) in &store.items {
        let pos = Vec2::new(p.pos[0], p.pos[2]);
        match s.objects.get(id) {
            None if inside_poly(pos) => added_in += 1,
            None => o_out += 1,
            Some((a, q)) if a != &p.asset || *q != p.pos => {
                if !inside_poly(pos) || !inside_poly(Vec2::new(q[0], q[2])) {
                    o_out += 1;
                }
            }
            _ => {}
        }
    }
    for (id, (_, q)) in &s.objects {
        if !store.items.contains_key(id) {
            if inside_poly(Vec2::new(q[0], q[2])) {
                removed_in += 1;
            } else {
                o_out += 1;
            }
        }
    }
    let now = snapshot(world).water;
    let (mut w_out, mut w_in) = (0usize, 0usize);
    for (kind, id, p, ser) in &now {
        let was = s.water.iter().find(|(k, i, _, _)| k == kind && i == id);
        if was.is_none_or(|w| &w.3 != ser) {
            if inside_poly(*p) {
                w_in += 1
            } else {
                w_out += 1
            }
        }
    }
    for (kind, id, p, _) in &s.water {
        if !now.iter().any(|(k, i, _, _)| k == kind && i == id) {
            if inside_poly(*p) {
                w_in += 1
            } else {
                w_out += 1
            }
        }
    }
    let cell2 = t.cell * t.cell;
    json!({
        "outside": { "height_cells": oh, "max_height_change_m": omax, "ground_paint_cells": os, "biome_cells": ob, "objects": o_out, "water_bodies": w_out },
        "inside": { "height_changed_m2": (ih as f32 * cell2).round(), "paint_changed_m2": (isp as f32 * cell2).round(), "max_raise_m": imax_up, "max_lower_m": imax_down, "objects_added": added_in, "objects_removed": removed_in, "water_bodies": w_in }
    })
}

fn outside_total(d: &Value) -> u64 {
    let o = &d["outside"];
    [
        "height_cells",
        "ground_paint_cells",
        "biome_cells",
        "objects",
        "water_bodies",
    ]
    .iter()
    .map(|k| o[*k].as_u64().unwrap_or(0))
    .sum()
}

#[derive(Default)]
enum Phase {
    #[default]
    Start,
    Setup,
    ShotBefore(u32),
    Running,
    ShotAfter(u32),
    Judge(Option<std::sync::Mutex<Receiver<Result<(f32, String), String>>>>),
    Undo(u32),
    Next,
    Done,
}

#[derive(Default)]
struct EvalState {
    frame: u32,
    cases: Vec<Value>,
    out: PathBuf,
    index: usize,
    phase: Phase,
    before: Option<Snapshot>,
    started: Option<Instant>,
    result: Value,
    /// A refine prompt was sent (its first run's calls and summary are kept here).
    refined: Option<(u32, String)>,
}

fn case_dir(st: &EvalState) -> PathBuf {
    st.out
        .join(st.cases[st.index]["name"].as_str().unwrap_or("case"))
}

fn case_poly(c: &Value) -> Option<Vec<Vec2>> {
    let pt = |v: &Value| {
        Some(Vec2::new(
            v.get(0)?.as_f64()? as f32,
            v.get(1)?.as_f64()? as f32,
        ))
    };
    if let Some(p) = c["polygon"].as_array() {
        return p.iter().map(pt).collect();
    }
    let ctr = pt(&c["center"])?;
    let r = c["radius"].as_f64()? as f32;
    // a slightly irregular loop, like a hand-drawn lasso
    Some(
        (0..40)
            .map(|k| {
                let a = k as f32 / 40.0 * std::f32::consts::TAU;
                ctr + Vec2::from_angle(a)
                    * r
                    * (1.0 + 0.07 * (a * 3.0).sin() + 0.04 * (a * 7.0 + 1.0).cos())
            })
            .collect(),
    )
}

fn shoot(world: &mut World, dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let args =
        json!({ "views": ["top", "angled"], "save_dir": dir.to_string_lossy(), "max_size": 256 });
    let tool = world
        .resource::<crate::mcp_tools::McpTools>()
        .tools
        .iter()
        .find(|t| t.name == "selection_screenshot")
        .cloned();
    if let Some(tool) = tool
        && let Ok(out) = (tool.run)(world, args)
    {
        for c in out {
            if let crate::mcp_tools::McpContent::Deferred(d) = c {
                crate::mcp::log_deferred(world, "selection_screenshot", d);
            }
        }
    }
}

fn shots_done(dir: &Path) -> bool {
    dir.join("top.png").is_file() && dir.join("angled.png").is_file()
}

fn run_eval(world: &mut World, mut st: Local<EvalState>) {
    let st = &mut *st;
    st.frame += 1;
    match std::mem::take(&mut st.phase) {
        Phase::Start => {
            let start: u32 = std::env::var("WB_PROMPT_EVAL_START")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(420);
            if st.frame < start {
                return;
            }
            let path = PathBuf::from(std::env::var("WB_PROMPT_EVAL").unwrap_or_default());
            let cases: Vec<Value> = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let only: Vec<String> = std::env::var("WB_PROMPT_EVAL_ONLY")
                .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
                .unwrap_or_default();
            st.cases = cases
                .into_iter()
                .filter(|c| {
                    only.is_empty() || only.iter().any(|o| Some(o.as_str()) == c["name"].as_str())
                })
                .collect();
            st.out = std::env::var("WB_PROMPT_EVAL_OUT")
                .map(PathBuf::from)
                .unwrap_or_else(|_| path.parent().unwrap_or(Path::new(".")).join("prompt-eval"));
            info!("PROMPTEVAL {} cases → {}", st.cases.len(), st.out.display());
            st.index = 0;
            st.phase = if st.cases.is_empty() {
                Phase::Done
            } else {
                Phase::Setup
            };
        }
        Phase::Setup => {
            let c = st.cases[st.index].clone();
            let Some(poly) = case_poly(&c) else {
                error!(
                    "PROMPTEVAL case {} has no polygon / center+radius",
                    c["name"]
                );
                st.phase = Phase::Next;
                return;
            };
            world.resource_scope(|world, mut sel: Mut<Selection>| {
                let t = world.resource::<crate::terrain::Terrain>();
                if let Err(e) = sel.set(t, &poly) {
                    error!("PROMPTEVAL selection: {e}");
                }
            });
            {
                let mut s = world.resource_mut::<PromptSession>();
                if let Some(b) = c["backend"]
                    .as_str()
                    .and_then(super::agent::Backend::from_id)
                {
                    s.backend = b;
                }
                if let Some(m) = c["model"].as_str() {
                    let b = s.backend;
                    s.models.insert(b, m.to_string());
                }
                s.turns.clear();
                s.state = RunState::Idle;
            }
            let dir = case_dir(st);
            let _ = std::fs::remove_dir_all(&dir);
            st.result = json!({ "name": c["name"], "prompt": c["prompt"] });
            shoot(world, &dir.join("before"));
            st.phase = Phase::ShotBefore(0);
        }
        Phase::ShotBefore(n) => {
            if !shots_done(&case_dir(st).join("before")) && n < 3000 {
                st.phase = Phase::ShotBefore(n + 1);
                return;
            }
            // statuses must be known before sending
            if world.resource::<PromptSession>().statuses.is_none() {
                st.phase = Phase::ShotBefore(n + 1);
                return;
            }
            st.before = Some(snapshot(world));
            st.started = Some(Instant::now());
            if let Some(script) = st.cases[st.index]["script"].as_array().cloned() {
                run_script(world, &script);
            } else {
                let prompt = st.cases[st.index]["prompt"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                world
                    .resource_mut::<PromptSession>()
                    .requests
                    .push(Request::Send {
                        prompt,
                        refine: false,
                    });
            }
            info!("PROMPTEVAL case {} started", st.cases[st.index]["name"]);
            st.phase = Phase::Running;
        }
        Phase::Running => {
            let s = world.resource::<PromptSession>();
            // (the request is handled after this system: wait for it to start)
            if s.running()
                || (s.state == RunState::Idle
                    && s.error.is_none()
                    && st.started.is_some_and(|t| t.elapsed().as_secs_f32() < 5.0))
            {
                st.phase = Phase::Running;
                return;
            }
            // `refine`: a follow-up prompt once the first run is done (it keeps the conversation)
            if let Some(refine) = st.cases[st.index]["refine"].as_str()
                && st.refined.is_none()
                && s.state == RunState::Done
            {
                st.refined = Some((s.calls, s.summary.clone()));
                world
                    .resource_mut::<PromptSession>()
                    .requests
                    .push(Request::Send {
                        prompt: refine.to_string(),
                        refine: true,
                    });
                st.phase = Phase::Running;
                return;
            }
            let ok = s.state == RunState::Done;
            let wall = st.started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
            let failed = s.feed.iter().filter(|f| f.ok == Some(false)).count();
            let tools: Vec<String> = s.feed.iter().filter_map(|f| f.tool.clone()).collect();
            let r = &mut st.result;
            r["backend"] = json!(s.backend.name());
            r["model"] = json!(s.model_label());
            r["success"] = json!(ok);
            r["wall_time_s"] = json!(wall.round());
            r["tool_calls"] = json!(s.calls + st.refined.as_ref().map(|r| r.0).unwrap_or(0));
            if let Some((calls, summary)) = &st.refined {
                r["first_run"] = json!({ "tool_calls": calls, "summary": summary });
                r["refine"] = st.cases[st.index]["refine"].clone();
            }
            r["failed_calls"] = json!(failed);
            r["tools"] = json!(tools);
            r["summary"] = json!(s.summary);
            r["error"] = json!(s.error);
            r["steps"] = json!(s.feed.iter().map(|f| f.text.clone()).collect::<Vec<_>>());
            let poly = world
                .resource::<Selection>()
                .polygon()
                .map(|p| p.to_vec())
                .unwrap_or_default();
            if let Some(b) = &st.before {
                st.result["edits"] = diff(world, b, &poly);
                st.result["outside_edits_total"] = json!(outside_total(&st.result["edits"]));
            }
            info!(
                "PROMPTEVAL case {} finished ok={ok} in {wall:.0}s, {} calls, outside edits {}",
                st.cases[st.index]["name"],
                st.result["tool_calls"],
                st.result["outside_edits_total"]
            );
            shoot(world, &case_dir(st).join("after"));
            st.phase = Phase::ShotAfter(0);
        }
        Phase::ShotAfter(n) => {
            if !shots_done(&case_dir(st).join("after")) && n < 3000 {
                st.phase = Phase::ShotAfter(n + 1);
                return;
            }
            st.phase = if std::env::var("WB_PROMPT_EVAL_JUDGE").is_ok() {
                let dir = case_dir(st);
                let prompt = st.cases[st.index]["prompt"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let status = world
                    .resource::<PromptSession>()
                    .status(super::agent::Backend::Codex)
                    .cloned();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(judge(status, &dir, &prompt));
                });
                Phase::Judge(Some(std::sync::Mutex::new(rx)))
            } else {
                Phase::Undo(0)
            };
        }
        Phase::Judge(rx) => {
            let Some(rx) = rx else {
                st.phase = Phase::Undo(0);
                return;
            };
            let got = rx
                .lock()
                .map_err(|_| std::sync::mpsc::TryRecvError::Disconnected)
                .and_then(|r| r.try_recv());
            match got {
                Ok(Ok((score, reasons))) => {
                    st.result["judge"] = json!({ "score": score, "reasons": reasons });
                    st.phase = Phase::Undo(0);
                }
                Ok(Err(e)) => {
                    st.result["judge"] = json!({ "error": e });
                    st.phase = Phase::Undo(0);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => st.phase = Phase::Judge(Some(rx)),
                Err(_) => st.phase = Phase::Undo(0),
            }
        }
        Phase::Undo(n) => {
            if n == 0 {
                let step = world.resource::<PromptSession>().step;
                if step.is_some()
                    && step == world.resource::<crate::history::History>().undo_serial()
                {
                    world
                        .resource_mut::<PromptSession>()
                        .requests
                        .push(Request::Undo);
                }
            }
            // a refined case made two steps: the first run's goes too
            if n == 15 && st.refined.is_some() {
                world.write_message(crate::tools::EditorAction::Undo);
            }
            if n < 30 {
                st.phase = Phase::Undo(n + 1);
                return;
            }
            let poly = world
                .resource::<Selection>()
                .polygon()
                .map(|p| p.to_vec())
                .unwrap_or_default();
            if let Some(b) = &st.before {
                let d = diff(world, b, &poly);
                let rest: u64 = outside_total(&d)
                    + d["inside"]["height_changed_m2"].as_f64().unwrap_or(0.0) as u64
                    + d["inside"]["paint_changed_m2"].as_f64().unwrap_or(0.0) as u64
                    + d["inside"]["objects_added"].as_u64().unwrap_or(0)
                    + d["inside"]["objects_removed"].as_u64().unwrap_or(0);
                st.result["undo_restores_exactly"] = json!(rest == 0);
                if rest != 0 {
                    st.result["after_undo"] = d;
                }
            }
            let dir = case_dir(st);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(
                dir.join("result.json"),
                serde_json::to_string_pretty(&st.result).unwrap_or_default(),
            );
            st.phase = Phase::Next;
        }
        Phase::Next => {
            st.index += 1;
            st.before = None;
            st.refined = None;
            st.phase = if st.index < st.cases.len() {
                Phase::Setup
            } else {
                Phase::Done
            };
        }
        Phase::Done => {
            if !st.out.as_os_str().is_empty() {
                write_report(&st.out);
            }
            info!("PROMPTEVAL done");
            world.write_message(AppExit::Success);
            st.phase = Phase::Done;
        }
    }
}

/// A case's `script` (`[[tool, {args}], …]`) instead of an agent: the calls run at once (masked
/// like any MCP call) and become one undo step — for testing the tools and generators.
fn run_script(world: &mut World, script: &[Value]) {
    let mark = world.resource::<crate::history::History>().serial();
    world.resource_mut::<Selection>().forced += 1;
    let mut feed = vec![];
    for call in script {
        let (Some(name), args) = (call[0].as_str(), call.get(1).cloned().unwrap_or(json!({})))
        else {
            continue;
        };
        let tool = world
            .resource::<crate::mcp_tools::McpTools>()
            .tools
            .iter()
            .find(|t| t.name == name)
            .cloned();
        let res = match tool {
            Some(t) => (t.run)(world, args.clone()),
            None => Err(format!("no tool {name}")),
        };
        if world.resource::<crate::history::History>().is_recording() {
            world.resource_scope(|w, mut h: Mut<crate::history::History>| {
                h.commit(w.resource::<crate::objects::ObjectStore>())
            });
        }
        super::mask::settle_new_steps(world);
        let (ok, detail) = match res {
            Ok(out) => (
                true,
                out.iter()
                    .find_map(|c| {
                        if let crate::mcp_tools::McpContent::Text(t) = c {
                            Some(t.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
            ),
            Err(e) => (false, e),
        };
        info!(
            "PROMPTEVAL script {name}: {} {detail}",
            if ok { "ok" } else { "FAILED" }
        );
        feed.push(super::FeedItem {
            at: 0.0,
            text: super::describe_call(name, &args),
            tool: Some(name.to_string()),
            ok: Some(ok),
            detail: Some(detail),
        });
    }
    let step = world
        .resource_mut::<crate::history::History>()
        .squash_after(mark, "Script");
    world.resource_mut::<Selection>().forced -= 1;
    let mut s = world.resource_mut::<PromptSession>();
    s.calls = feed.len() as u32;
    s.summary = feed
        .iter()
        .filter_map(|f| f.detail.clone())
        .collect::<Vec<_>>()
        .join(" | ");
    s.feed = feed;
    s.step = step.filter(|x| *x > mark);
    s.state = RunState::Done;
    s.error = None;
}

/// The judge: the same backend (Codex) rates the before / after views against the prompt.
fn judge(
    status: Option<super::agent::BackendStatus>,
    dir: &Path,
    prompt: &str,
) -> Result<(f32, String), String> {
    let st = status
        .filter(|s| s.ready)
        .ok_or("Codex is not available for judging")?;
    let exe = st.path.ok_or("no codex")?;
    let work = std::env::temp_dir().join(format!("wb-judge-{}", fastrand::u32(..)));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let imgs = [
        "before/top.png",
        "before/angled.png",
        "after/top.png",
        "after/angled.png",
    ]
    .map(|p| dir.join(p));
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "exec",
        "--json",
        "--ephemeral",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "-s",
        "read-only",
        "-m",
        "gpt-5.5",
        "-C",
    ])
    .arg(&work);
    for f in [
        "shell_tool",
        "unified_exec",
        "apps",
        "browser_use",
        "computer_use",
        "image_generation",
        "multi_agent",
        "plugins",
    ] {
        if st.features.iter().any(|k| k == f) {
            cmd.args(["--disable", f]);
        }
    }
    for i in &imgs {
        cmd.arg("-i").arg(i);
    }
    cmd.arg("-");
    let text = format!(
        "You judge AI edits to a 3D game world. A user selected an area of the world (a thin light outline marks it on the ground) and asked: \"{prompt}\".\n\
Image 1: top-down view before. Image 2: angled view before. Image 3: top-down view after. Image 4: angled view after.\n\
Rate the AFTER state from 0 to 10: does it clearly fulfil the request, and does it look like a believable, good-looking part of a game world (natural shapes, fitting ground and vegetation, no obvious artefacts)? 7 means a solid result a game designer would keep.\n\
Reply with only a JSON object: {{\"score\": <number>, \"reasons\": \"<two or three sentences>\"}}"
    );
    use std::io::Write;
    let mut child = cmd
        .env(
            "PATH",
            format!(
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{}",
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&work);
    let mut last = String::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        super::agent::parse_codex_line(line, &mut last);
    }
    let start = last
        .find('{')
        .ok_or_else(|| format!("no JSON in the judge's answer: {last}"))?;
    let end = last.rfind('}').ok_or("no JSON end")?;
    let v: Value = serde_json::from_str(&last[start..=end]).map_err(|e| e.to_string())?;
    Ok((
        v["score"].as_f64().unwrap_or(0.0) as f32,
        v["reasons"].as_str().unwrap_or("").to_string(),
    ))
}

/// report.json and contact_sheet.png from every case folder in `out`.
fn write_report(out: &Path) {
    let mut cases: Vec<(PathBuf, Value)> = std::fs::read_dir(out)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            Some((
                p.clone(),
                serde_json::from_str::<Value>(
                    &std::fs::read_to_string(p.join("result.json")).ok()?,
                )
                .ok()?,
            ))
        })
        .collect();
    cases.sort_by(|a, b| a.0.cmp(&b.0));
    let n = cases.len();
    let ok = cases.iter().filter(|c| c.1["success"] == true).count();
    let scores: Vec<f64> = cases
        .iter()
        .filter_map(|c| c.1["judge"]["score"].as_f64())
        .collect();
    let summary = json!({
        "cases": n,
        "succeeded": ok,
        "mean_wall_time_s": cases.iter().filter_map(|c| c.1["wall_time_s"].as_f64()).sum::<f64>() / n.max(1) as f64,
        "mean_tool_calls": cases.iter().filter_map(|c| c.1["tool_calls"].as_f64()).sum::<f64>() / n.max(1) as f64,
        "outside_edits_total": cases.iter().filter_map(|c| c.1["outside_edits_total"].as_u64()).sum::<u64>(),
        "undo_exact": cases.iter().filter(|c| c.1["undo_restores_exactly"] == true).count(),
        "judge_mean": if scores.is_empty() { Value::Null } else { json!(scores.iter().sum::<f64>() / scores.len() as f64) },
        "judge_at_least_7": scores.iter().filter(|s| **s >= 7.0).count(),
    });
    let report = json!({ "summary": summary, "cases": cases.iter().map(|c| c.1.clone()).collect::<Vec<_>>() });
    let _ = std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );
    // contact sheet: one row per case — before top, after top, before angled, after angled
    let (tw, th, label) = (480u32, 300u32, 26u32);
    let sheet_w = tw * 4 + 5 * 6;
    let sheet_h = (th + label + 6) * n.max(1) as u32 + 6;
    let mut sheet = image::RgbImage::from_pixel(sheet_w, sheet_h, image::Rgb([16, 20, 28]));
    for (row, (dir, r)) in cases.iter().enumerate() {
        let y0 = 6 + row as u32 * (th + label + 6);
        let score = r["judge"]["score"]
            .as_f64()
            .map(|s| format!("  SCORE {s:.0}/10"))
            .unwrap_or_default();
        let title = format!(
            "{}  {}  {}S  {} CALLS  OUTSIDE {}{}",
            r["name"].as_str().unwrap_or("").to_uppercase(),
            if r["success"] == true { "OK" } else { "FAILED" },
            r["wall_time_s"].as_f64().unwrap_or(0.0),
            r["tool_calls"].as_u64().unwrap_or(0),
            r["outside_edits_total"].as_u64().unwrap_or(0),
            score
        );
        draw_text(&mut sheet, 8, y0 + 6, &title, 2, [220, 230, 240]);
        for (k, f) in [
            "before/top.png",
            "after/top.png",
            "before/angled.png",
            "after/angled.png",
        ]
        .iter()
        .enumerate()
        {
            if let Ok(img) = image::open(dir.join(f)) {
                let small = image::imageops::resize(
                    &img.to_rgb8(),
                    tw,
                    th,
                    image::imageops::FilterType::Triangle,
                );
                image::imageops::overlay(
                    &mut sheet,
                    &small,
                    (6 + k as u32 * (tw + 6)) as i64,
                    (y0 + label) as i64,
                );
            }
        }
    }
    let _ = sheet.save(out.join("contact_sheet.png"));
    info!("PROMPTEVAL report: {}", out.join("report.json").display());
}

/// 5×7 glyphs (rows top to bottom, 5 bits each) for the contact sheet's labels.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 17, 25, 21, 19, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [31, 2, 4, 2, 1, 17, 14],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 30, 1, 1, 17, 14],
        '6' => [6, 8, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 2, 12],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        '%' => [24, 25, 2, 4, 8, 19, 3],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        _ => [0; 7],
    }
}

fn draw_text(img: &mut image::RgbImage, x: u32, y: u32, s: &str, scale: u32, col: [u8; 3]) {
    let mut cx = x;
    for c in s.chars() {
        let g = glyph(c);
        for (row, bits) in g.iter().enumerate() {
            for b in 0..5 {
                if bits & (16 >> b) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let (px, py) = (cx + b * scale + dx, y + row as u32 * scale + dy);
                            if px < img.width() && py < img.height() {
                                img.put_pixel(px, py, image::Rgb(col));
                            }
                        }
                    }
                }
            }
        }
        cx += 6 * scale;
    }
}
