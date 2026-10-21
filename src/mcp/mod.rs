//! MCP server: any agent (Claude Code, Claude Desktop, Cursor…) can drive the editor over the
//! Model Context Protocol and build anything a person can.
//!
//! - `http`: the Streamable HTTP transport (`POST http://127.0.0.1:<port>/mcp`, JSON responses)
//!   on a background thread. It answers `initialize`, `ping` and `tools/list` itself and queues
//!   `tools/call` for the main thread.
//! - `McpExec` (here): runs queued calls with exclusive world access between frames (before
//!   `Update`, within a time budget, never in the middle of a user's brush stroke) and finishes
//!   deferred results (screenshots…) over later frames.
//! - `bridge`: `world_builder --mcp-stdio` proxies stdio JSON-RPC to the running editor.
//! - `calls`: how every call runs: arguments checked against the tool's schema (`validate`),
//!   `items` for add-style tools and the `batch` tool (both one undo step).
//! - `tools`: the tools for the editor's own features (other features register theirs through
//!   `mcp_tools`); `ui`: the top-bar status chip and the connect panel.
//!
//! Settings (on/off, port) live in the shared prefs (`mcp_enabled`, `mcp_port`).
//! Test hooks: `WB_MCP_PORT=<port>` starts the server in automated runs (`WB_SHOT`), where it is
//! otherwise off; `WB_MCP=0` turns it off; `WB_MCP_PANEL=1` opens the connect panel.

pub mod bridge;
mod calls;
#[cfg(test)]
mod checks;
mod http;
mod tools;
mod ui;
mod validate;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde_json::{Value, json};

use crate::mcp_tools::{McpContent, McpDeferred, McpTools};

pub use calls::call_tool;
pub use ui::{connect_window, status_info};

pub const DEFAULT_PORT: u16 = 7740;
/// Main-thread time per frame spent on queued calls (at least one call always runs).
const FRAME_BUDGET: Duration = Duration::from_millis(12);
/// A deferred result that hasn't finished by then fails.
const DEFERRED_TIMEOUT: Duration = Duration::from_secs(240);

/// Finished tool output, ready to send.
pub(crate) enum Out {
    Text(String),
    Png(Vec<u8>),
}

pub(crate) type CallResult = Result<Vec<Out>, String>;

/// A `tools/call` waiting for the main thread.
pub(crate) struct Job {
    pub name: String,
    pub args: Value,
    pub reply: mpsc::Sender<CallResult>,
}

/// State shared with the HTTP threads.
#[derive(Default)]
pub(crate) struct Shared {
    pub queue: Mutex<VecDeque<Job>>,
    /// `tools/list` result (rebuilt when the registry changes).
    pub tools: RwLock<Value>,
    /// Sessions the client ended (DELETE): later requests with them get 404.
    pub ended: Mutex<std::collections::HashSet<String>>,
}

/// One finished call, for the status chip, the panel and `get_log`.
#[derive(Clone)]
pub struct CallRecord {
    pub name: String,
    pub ok: bool,
    pub ms: f32,
    /// Real time (s since start) it finished.
    pub at: f32,
    pub error: Option<String>,
}

#[derive(Resource)]
pub struct McpServer {
    pub enabled: bool,
    pub port: u16,
    /// Why the server isn't listening (port in use…).
    pub error: Option<String>,
    pub running_port: Option<u16>,
    pub tool_count: usize,
    pub calls: u64,
    pub recent: VecDeque<CallRecord>,
    pub show_panel: bool,
    /// Port being edited in the panel.
    pub port_edit: u16,
    /// Screenshots in progress that want the editor UI hidden.
    pub hide_ui: u32,
    shared: Arc<Shared>,
    handle: Option<http::Handle>,
    /// Settings may be saved (not in automated runs, not before the first apply).
    persist: bool,
}

impl McpServer {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.persist = true;
    }
    pub fn set_port(&mut self, port: u16) {
        self.port = port;
        self.persist = true;
    }
}

fn automated_run() -> bool {
    std::env::var("WB_SHOT").is_ok() || std::env::var("WB_FRESH").is_ok()
}

impl Default for McpServer {
    fn default() -> Self {
        let env_port = std::env::var("WB_MCP_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok());
        let port = env_port
            .or(crate::prefs::get_u64("mcp_port").map(|p| p as u16))
            .unwrap_or(DEFAULT_PORT);
        let enabled = match std::env::var("WB_MCP").ok().as_deref() {
            Some("0") => false,
            Some(_) => true,
            None if automated_run() => env_port.is_some(),
            None => crate::prefs::get_bool("mcp_enabled").unwrap_or(true),
        };
        Self {
            enabled,
            port,
            error: None,
            running_port: None,
            tool_count: 0,
            calls: 0,
            recent: VecDeque::new(),
            show_panel: std::env::var("WB_MCP_PANEL").is_ok(),
            port_edit: port,
            hide_ui: 0,
            shared: Arc::new(Shared::default()),
            handle: None,
            persist: false,
        }
    }
}

/// Calls whose results are still being produced over frames.
struct Pending {
    name: String,
    reply: Reply,
    done: Vec<Out>,
    waiting: VecDeque<McpDeferred>,
    started: Instant,
    /// The editor's toast when the call started (did the tool's own code show one?).
    toast_before: Option<(String, f32)>,
}

enum Reply {
    Http(mpsc::Sender<CallResult>),
    /// `WB_MCP_CALL` test calls: log the result.
    Log,
}

#[derive(Resource, Default)]
struct McpExec {
    pending: Vec<Pending>,
}

/// Recent editor messages (toasts) for `get_log`.
#[derive(Resource, Default)]
pub struct McpLog {
    pub toasts: VecDeque<(f32, String)>,
    last: Option<(String, f32)>,
}

pub struct McpPlugin;

impl Plugin for McpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<McpServer>()
            .init_resource::<McpExec>()
            .init_resource::<McpLog>()
            .add_systems(PreUpdate, execute.after(bevy::input::InputSystems))
            .add_systems(Last, (apply_settings, record_toasts));
        tools::register(app);
    }
}

/// Starts / stops / restarts the HTTP server when the settings change; saves them.
fn apply_settings(mut server: ResMut<McpServer>, registry: Option<Res<McpTools>>) {
    let s = &mut *server;
    // tools/list snapshot (tools register while the app is built; refresh if that changes)
    if let Some(reg) = registry
        && reg.tools.len() != s.tool_count
    {
        s.tool_count = reg.tools.len();
        *s.shared.tools.write().unwrap_or_else(|e| e.into_inner()) = tools::list_json(&reg.tools);
    }
    let want = s.enabled.then_some(s.port);
    if want != s.running_port || (s.enabled && s.handle.is_none() && s.error.is_none()) {
        if let Some(h) = s.handle.take() {
            h.stop();
            info!("MCP server stopped");
        }
        s.running_port = None;
        s.error = None;
        // calls still queued can't be answered any more
        for job in s
            .shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = job
                .reply
                .send(Err("The editor's MCP server was turned off".into()));
        }
        if let Some(port) = want {
            match http::start(port, s.shared.clone()) {
                Ok(h) => {
                    info!(
                        "MCP server listening on http://127.0.0.1:{port}/mcp ({} tools)",
                        s.tool_count
                    );
                    s.handle = Some(h);
                }
                Err(e) => {
                    warn!("MCP server could not start on port {port}: {e}");
                    s.error = Some(e);
                }
            }
            s.running_port = Some(port);
        }
    }
    if s.persist {
        s.persist = false;
        crate::prefs::set("mcp_enabled", json!(s.enabled));
        crate::prefs::set("mcp_port", json!(s.port));
    }
}

/// Runs queued tool calls and advances deferred results.
fn execute(world: &mut World) {
    poll_pending(world);
    // never interleave with a brush stroke the user is drawing (its undo step is open)
    if world.resource::<crate::tools::Stroke>().active {
        return;
    }
    let shared = world.resource::<McpServer>().shared.clone();
    let t0 = Instant::now();
    loop {
        let Some(job) = shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
        else {
            break;
        };
        let started = Instant::now();
        let toast_before = world.resource::<crate::editor::Editor>().toast.clone();
        let out = call_tool(world, &job.name, job.args);
        finish(
            world,
            job.name,
            Reply::Http(job.reply),
            out,
            started,
            toast_before,
        );
        if t0.elapsed() > FRAME_BUDGET {
            break;
        }
    }
}

/// Runs one call from another endpoint (`PrivateEndpoint`) exactly like the server's own
/// (checked arguments, the selection's edit mask, deferred results).
pub(crate) fn run_job(world: &mut World, job: Job) {
    let started = Instant::now();
    let toast_before = world.resource::<crate::editor::Editor>().toast.clone();
    let out = call_tool(world, &job.name, job.args);
    finish(
        world,
        job.name,
        Reply::Http(job.reply),
        out,
        started,
        toast_before,
    );
}

/// A second MCP endpoint on its own port that offers only some tools (an area prompt's agent
/// uses one, so it works whatever the user's MCP settings are and can't reach world-wide tools).
/// Its owner takes the queued calls (`take_job`) and runs them with `run_job`.
pub(crate) struct PrivateEndpoint {
    pub port: u16,
    shared: Arc<Shared>,
    handle: Option<http::Handle>,
    allowed: Vec<&'static str>,
}

impl PrivateEndpoint {
    pub fn open(tools: &[crate::mcp_tools::McpTool]) -> Result<Self, String> {
        // a free port (the OS picks one)
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared::default());
        *shared.tools.write().unwrap_or_else(|e| e.into_inner()) = tools::list_json(tools);
        let handle = http::start(port, shared.clone())?;
        Ok(Self {
            port,
            shared,
            handle: Some(handle),
            allowed: tools.iter().map(|t| t.name).collect(),
        })
    }
    /// The next queued call; calls to tools this endpoint doesn't offer are refused here.
    pub fn take_job(&self) -> Option<Job> {
        loop {
            let job = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()?;
            if self.allowed.contains(&job.name.as_str()) {
                return Some(job);
            }
            let _ = job.reply.send(Err(format!(
                "`{}` isn't available while editing an area",
                job.name
            )));
        }
    }
}

impl Drop for PrivateEndpoint {
    fn drop(&mut self) {
        for job in self
            .shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = job.reply.send(Err("The area edit has ended".into()));
        }
        if let Some(h) = self.handle.take() {
            h.stop();
        }
    }
}

/// Sends a result now or parks its deferred parts.
fn finish(
    world: &mut World,
    name: String,
    reply: Reply,
    out: Result<Vec<McpContent>, String>,
    started: Instant,
    toast_before: Option<(String, f32)>,
) {
    let mut p = Pending {
        name,
        reply,
        done: vec![],
        waiting: VecDeque::new(),
        started,
        toast_before,
    };
    match out {
        Ok(contents) => {
            collect(&mut p, contents);
            if p.waiting.is_empty() {
                complete(world, p, Ok(()));
            } else {
                world.resource_mut::<McpExec>().pending.push(p);
            }
        }
        Err(e) => complete(world, p, Err(e)),
    }
}

fn collect(p: &mut Pending, contents: Vec<McpContent>) {
    for c in contents {
        match c {
            McpContent::Text(t) => p.done.push(Out::Text(t)),
            McpContent::Json(v) => p.done.push(Out::Text(json_text(&v))),
            McpContent::Png(b) => p.done.push(Out::Png(b)),
            McpContent::Deferred(d) => p.waiting.push_back(d),
        }
    }
}

/// JSON as text: indented when small, compact when large (agents pay per token).
pub(crate) fn json_text(v: &Value) -> String {
    let compact = v.to_string();
    if compact.len() < 1500 {
        serde_json::to_string_pretty(v).unwrap_or(compact)
    } else {
        compact
    }
}

fn poll_pending(world: &mut World) {
    let list = std::mem::take(&mut world.resource_mut::<McpExec>().pending);
    let mut keep = vec![];
    for mut p in list {
        let mut failed = None;
        while let Some(mut d) = p.waiting.pop_front() {
            if p.started.elapsed() > DEFERRED_TIMEOUT {
                failed = Some("Timed out waiting for the editor (is its window minimised or hidden? Rendering pauses then).".to_string());
                break;
            }
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (d.0)(world)));
            match res {
                Ok(None) => {
                    p.waiting.push_front(d);
                    break;
                }
                Ok(Some(Ok(more))) => collect(&mut p, more),
                Ok(Some(Err(e))) => {
                    failed = Some(e);
                    break;
                }
                Err(_) => {
                    failed = Some(format!(
                        "The tool `{}` failed internally while finishing",
                        p.name
                    ));
                    break;
                }
            }
        }
        if let Some(e) = failed {
            complete(world, p, Err(e));
        } else if p.waiting.is_empty() {
            complete(world, p, Ok(()));
        } else {
            keep.push(p);
        }
    }
    let idle = keep.is_empty();
    world.resource_mut::<McpExec>().pending.extend(keep);
    if idle {
        // (a screenshot that failed or timed out can't leave the UI hidden)
        world.resource_mut::<McpServer>().hide_ui = 0;
    }
}

fn complete(world: &mut World, p: Pending, res: Result<(), String>) {
    let ms = p.started.elapsed().as_secs_f32() * 1000.0;
    let at = world.resource::<Time<Real>>().elapsed_secs();
    let ok = res.is_ok();
    {
        let mut s = world.resource_mut::<McpServer>();
        s.calls += 1;
        s.recent.push_front(CallRecord {
            name: p.name.clone(),
            ok,
            ms,
            at,
            error: res.as_ref().err().cloned(),
        });
        s.recent.truncate(40);
    }
    match p.reply {
        Reply::Http(tx) => {
            if ok && !tools::READ_ONLY.contains(&p.name.as_str()) {
                agent_toast(world, &p.name, &p.done, &p.toast_before);
            }
            let _ = tx.send(res.map(|_| p.done));
        }
        Reply::Log => match res {
            Ok(()) => {
                for o in p.done {
                    match o {
                        Out::Text(t) => info!("MCPCALL {} -> {t}", p.name),
                        Out::Png(b) => info!("MCPCALL {} -> png {} bytes", p.name, b.len()),
                    }
                }
            }
            Err(e) => warn!("MCPCALL {} failed: {e}", p.name),
        },
    }
}

/// Shows what an agent just changed in the editor's status toast (unless the tool's own code
/// already showed one), so the person watching can follow along.
fn agent_toast(world: &mut World, name: &str, done: &[Out], before: &Option<(String, f32)>) {
    let mut ed = world.resource_mut::<crate::editor::Editor>();
    let own = match (before, &ed.toast) {
        (_, None) => false,
        (None, Some(_)) => true,
        // a new message, or the same one shown again (its age restarted)
        (Some((m0, a0)), Some((m1, a1))) => m0 != m1 || a1 < a0,
    };
    if own {
        return;
    }
    let summary = done.iter().find_map(|o| match o {
        Out::Text(t) if !t.starts_with(['{', '[']) => Some(
            t.split(". ")
                .next()
                .unwrap_or(t)
                .trim_end_matches('.')
                .to_string(),
        ),
        _ => None,
    });
    let mut msg = summary.unwrap_or_else(|| name.replace('_', " "));
    if msg.chars().count() > 110 {
        msg = msg.chars().take(107).collect::<String>() + "…";
    }
    ed.toast(format!("{}  Agent: {msg}", egui_phosphor::regular::ROBOT));
}

/// `WB_MCP_CALL` (debug.rs): a deferred result is logged when it finishes.
pub fn log_deferred(world: &mut World, name: &str, d: McpDeferred) {
    let p = Pending {
        name: name.to_string(),
        reply: Reply::Log,
        done: vec![],
        waiting: VecDeque::from([d]),
        started: Instant::now(),
        toast_before: None,
    };
    world.resource_mut::<McpExec>().pending.push(p);
}

/// Keeps the editor's recent messages for `get_log`.
fn record_toasts(ed: Res<crate::editor::Editor>, mut log: ResMut<McpLog>, time: Res<Time<Real>>) {
    let Some((msg, age)) = &ed.toast else { return };
    let new = match &log.last {
        Some((m, a)) => m != msg || *age < *a,
        None => true,
    };
    log.last = Some((msg.clone(), *age));
    if new {
        let t = time.elapsed_secs();
        log.toasts.push_front((t, msg.clone()));
        log.toasts.truncate(60);
    }
}

/// Whether the editor UI is drawn this frame (hidden for clean screenshots).
pub fn ui_visible(server: Option<Res<McpServer>>) -> bool {
    server.is_none_or(|s| s.hide_ui == 0)
}
