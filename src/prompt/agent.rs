//! Agent backends for area prompts, all driving the editor through the run's private MCP
//! endpoint (so every backend gets the same tools, edit mask, step feed and undo grouping):
//!
//! - Codex CLI (`codex exec --json --ephemeral`, ChatGPT login): the MCP server is given with
//!   `-c` overrides only (the user's `~/.codex/config.toml` is ignored, never changed); shell,
//!   web search, apps, browser and image tools are off, the sandbox is read-only in an empty temp
//!   directory and our tools are pre-approved.
//! - Claude Code CLI (`claude -p --output-format stream-json`): only our MCP server
//!   (`--strict-mcp-config`), built-in tools off (`--tools ""`), our tools pre-allowed.
//! - An API key for any OpenAI-compatible provider (OpenRouter, OpenAI, Anthropic, a local
//!   Ollama…): a Chat Completions tool-calling loop over the same tools, sent with `curl` (the key
//!   goes through stdin, never the command line).
//!
//! "Sign in with Claude" runs `claude auth login` in the background (`ClaudeLogin`): it opens the
//! browser, and takes a pasted code when the browser can't hand it back.
//!
//! Each run is a background thread; it reports `AgentEvent`s, can be stopped (the process group
//! is killed) and times out. Detection (`detect`) also runs off the main thread.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Backend {
    Codex,
    ClaudeCode,
    /// An API key for an OpenAI-compatible provider (`Provider`).
    Api,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::Codex, Backend::ClaudeCode, Backend::Api];
    pub fn id(self) -> &'static str {
        match self {
            Backend::Codex => "codex",
            Backend::ClaudeCode => "claude_code",
            Backend::Api => "api",
        }
    }
    pub fn from_id(s: &str) -> Option<Backend> {
        // (saved before the API backend took any provider)
        if s == "anthropic_api" {
            return Some(Backend::Api);
        }
        Self::ALL.into_iter().find(|b| b.id() == s)
    }
    pub fn name(self) -> &'static str {
        match self {
            Backend::Codex => "Codex",
            Backend::ClaudeCode => "Claude Code",
            Backend::Api => "API key",
        }
    }
    /// Models offered in the panel (first = default). The API backend's come from its provider
    /// (`detect`), or `Provider::models`.
    pub fn models(self) -> &'static [&'static str] {
        match self {
            // (only until the installed Codex's own catalog is read: `detect`)
            Backend::Codex => &["gpt-6.1-sol", "gpt-6-astra", "gpt-6-sol"],
            Backend::ClaudeCode => &["opus", "sonnet"],
            Backend::Api => &[],
        }
    }
}

/// An OpenAI-compatible API the API backend offers by name (any other base URL works too).
pub struct Provider {
    pub name: &'static str,
    pub base: &'static str,
    /// Environment variable with its key ("" = none).
    pub env: &'static str,
    pub key_hint: &'static str,
    /// Where to get a key ("" = no key needed).
    pub keys_at: &'static str,
    /// Suggested models when its catalog can't be read.
    pub models: &'static [&'static str],
}

pub const PROVIDERS: [Provider; 4] = [
    Provider {
        name: "OpenRouter",
        base: "https://openrouter.ai/api/v1",
        env: "OPENROUTER_API_KEY",
        key_hint: "sk-or-…",
        keys_at: "openrouter.ai/keys",
        models: &[],
    },
    Provider {
        name: "OpenAI",
        base: "https://api.openai.com/v1",
        env: "OPENAI_API_KEY",
        key_hint: "sk-…",
        keys_at: "platform.openai.com/api-keys",
        models: &[],
    },
    Provider {
        name: "Anthropic",
        base: "https://api.anthropic.com/v1",
        env: "ANTHROPIC_API_KEY",
        key_hint: "sk-ant-…",
        keys_at: "console.anthropic.com",
        models: &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
    },
    Provider {
        name: "Local (Ollama)",
        base: "http://localhost:11434/v1",
        env: "",
        key_hint: "no key needed",
        keys_at: "",
        models: &[],
    },
];

/// The named provider at this base URL (None: a custom one).
pub fn provider(base: &str) -> Option<&'static Provider> {
    let base = base.trim().trim_end_matches('/');
    PROVIDERS.iter().find(|p| p.base == base)
}

/// A base URL curl can be given safely: http(s), no spaces or quotes.
pub fn valid_base(base: &str) -> bool {
    let b = base.trim();
    (b.starts_with("https://") || b.starts_with("http://"))
        && b.len() > 8
        && !b.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\')
}

/// Where the API backend sends requests, and with which key.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiConn {
    pub base: String,
    pub key: Option<String>,
}

impl ApiConn {
    /// Has what it needs for a run: a key, or a provider that takes none (a local server).
    pub fn ready(&self) -> bool {
        valid_base(&self.base)
            && (self.key.is_some() || provider(&self.base).is_some_and(|p| p.keys_at.is_empty()))
    }
    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base.trim().trim_end_matches('/'))
    }
}

/// What `detect` found for one backend.
#[derive(Clone, Debug)]
pub struct BackendStatus {
    pub backend: Backend,
    pub ready: bool,
    pub path: Option<PathBuf>,
    /// One line for the panel ("Logged in with ChatGPT", how to log in…).
    pub note: String,
    /// Codex: feature flags this version knows (we turn the risky ones off).
    pub features: Vec<String>,
    /// Codex: the models its catalog lists (best first).
    pub models: Vec<ModelInfo>,
}

/// A model the backend offers.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub slug: String,
    pub name: String,
    pub description: String,
    /// reasoning efforts it supports ("low", "medium", "high", "xhigh", "max", "ultra")
    pub efforts: Vec<String>,
    pub default_effort: String,
}

/// What a reasoning effort is called in the panel.
pub fn effort_name(e: &str) -> String {
    match e {
        "xhigh" => "Extra high".into(),
        e => {
            let mut c = e.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    }
}

/// The models in `codex debug models` output that the picker lists (by priority).
pub fn parse_codex_models(json: &str) -> Vec<ModelInfo> {
    let v: Value = serde_json::from_str(json.trim()).unwrap_or(Value::Null);
    let list = v
        .get("models")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<(i64, ModelInfo)> = list
        .iter()
        .filter(|m| m["visibility"].as_str().is_none_or(|v| v == "list"))
        .filter_map(|m| {
            let slug = m["slug"].as_str()?.to_string();
            let efforts: Vec<String> = m["supported_reasoning_levels"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            x.get("effort")
                                .and_then(Value::as_str)
                                .or(x.as_str())
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let info = ModelInfo {
                name: m["display_name"].as_str().unwrap_or(&slug).to_string(),
                description: m["description"].as_str().unwrap_or("").to_string(),
                default_effort: m["default_reasoning_level"]
                    .as_str()
                    .unwrap_or("medium")
                    .to_string(),
                efforts,
                slug,
            };
            Some((m["priority"].as_i64().unwrap_or(999), info))
        })
        .collect();
    out.sort_by_key(|(p, _)| *p);
    out.into_iter().map(|(_, m)| m).collect()
}

/// Directories GUI apps don't have on their PATH but CLIs live in.
fn search_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let mut v: Vec<PathBuf> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        v.push(PathBuf::from(d));
    }
    for d in [
        ".npm-global/bin",
        ".local/bin",
        ".claude/local",
        ".bun/bin",
        ".volta/bin",
    ] {
        v.push(home.join(d));
    }
    v
}

/// PATH for spawned CLIs (the codex launcher runs on node).
fn child_path() -> String {
    let mut seen = vec![];
    for d in search_dirs() {
        let s = d.to_string_lossy().to_string();
        if !seen.contains(&s) {
            seen.push(s);
        }
    }
    seen.join(":")
}

fn find(name: &str) -> Option<PathBuf> {
    search_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Codex: the newest of the one on the PATH and the copies bundled with the ChatGPT / Codex apps
/// (an app's copy is often newer and knows newer models).
pub fn find_codex() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = find("codex").into_iter().collect();
    for app in ["/Applications/ChatGPT.app", "/Applications/Codex.app"] {
        let p = PathBuf::from(app).join("Contents/Resources/codex-cli/bin/codex");
        if p.is_file() {
            cands.push(p);
        }
    }
    cands
        .into_iter()
        .filter_map(|p| {
            let (_, v) = run_quick(&p, &["--version"])?;
            // "codex-cli 0.162.0-alpha.17.2" → [0, 162, 0]
            let num = v
                .split_whitespace()
                .last()?
                .split('-')
                .next()?
                .split('.')
                .map(|x| x.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>();
            Some((num, p))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p)
}

/// Claude Code: on the PATH, or the copy bundled with the Claude desktop app (newest version).
fn find_claude() -> Option<PathBuf> {
    if let Some(p) = find("claude") {
        return Some(p);
    }
    let base = PathBuf::from(std::env::var("HOME").ok()?)
        .join("Library/Application Support/Claude/claude-code");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&base)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort_by_key(|p| version_key(p));
    for v in versions.into_iter().rev() {
        for e in std::fs::read_dir(&v).ok()?.flatten() {
            let p = e.path().join("claude.app/Contents/MacOS/claude");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn version_key(p: &Path) -> Vec<u32> {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .split('.')
        .map(|s| s.parse().unwrap_or(0))
        .collect()
}

fn run_quick(cmd: &Path, args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new(cmd)
        .args(args)
        .env("PATH", child_path())
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    s += &String::from_utf8_lossy(&out.stderr);
    Some((out.status.success(), s))
}

/// The provider's key from its environment variable (`OPENROUTER_API_KEY`, …), when the panel
/// has none.
pub fn env_api_key(base: &str) -> Option<String> {
    let p = provider(base).filter(|p| !p.env.is_empty())?;
    std::env::var(p.env).ok().filter(|k| !k.trim().is_empty())
}

/// Finds the backends and whether they're logged in, and the API provider's models (blocking:
/// call off the main thread).
pub fn detect(api: ApiConn) -> Vec<BackendStatus> {
    let mut out = vec![];
    // Codex
    let codex = find_codex();
    let mut st = BackendStatus {
        backend: Backend::Codex,
        ready: false,
        path: codex.clone(),
        note: String::new(),
        features: vec![],
        models: vec![],
    };
    match &codex {
        None => {
            st.note = "Not installed: `npm install -g @openai/codex`, then `codex login`.".into()
        }
        Some(p) => match run_quick(p, &["login", "status"]) {
            Some((true, s)) if s.to_lowercase().contains("logged in") => {
                st.ready = true;
                st.note = s
                    .lines()
                    .next()
                    .unwrap_or("Logged in")
                    .trim()
                    .replace("using", "with");
                if let Some((_, f)) = run_quick(p, &["features", "list"]) {
                    st.features = f
                        .lines()
                        .filter_map(|l| l.split_whitespace().next())
                        .map(str::to_string)
                        .collect();
                }
                if let Some((true, m)) = run_quick(p, &["debug", "models"]) {
                    st.models = parse_codex_models(&m);
                }
            }
            Some(_) => {
                st.note = "Installed but not logged in: run `codex login` in Terminal.".into()
            }
            None => st.note = "Found but could not run it.".into(),
        },
    }
    out.push(st);
    // Claude Code
    let claude = find_claude();
    let mut st = BackendStatus {
        backend: Backend::ClaudeCode,
        ready: false,
        path: claude.clone(),
        note: String::new(),
        features: vec![],
        models: vec![],
    };
    match &claude {
        None => st.note = "Not installed (claude.com/claude-code).".into(),
        Some(p) => match run_quick(p, &["auth", "status"]) {
            Some((_, s)) => {
                let v: Value = serde_json::from_str(s.trim()).unwrap_or(Value::Null);
                if v["loggedIn"].as_bool() == Some(true) {
                    st.ready = true;
                    st.note = format!(
                        "Logged in ({})",
                        v["authMethod"].as_str().unwrap_or("account")
                    );
                } else {
                    st.note = "Not signed in: use Sign in with Claude.".into();
                }
            }
            None => st.note = "Found but could not run it.".into(),
        },
    }
    out.push(st);
    // API
    let name = provider(&api.base).map_or("the provider", |p| p.name);
    // (a public catalog, like OpenRouter's, suggests models before a key is in)
    let models = valid_base(&api.base).then(|| api_models(&api));
    let (ready, models, note) = match (api.ready(), models) {
        (false, m) => (
            false,
            m.and_then(Result::ok).unwrap_or_default(),
            format!("Needs an API key for {name}."),
        ),
        (true, Some(Ok(m))) => {
            let n = m.len();
            (true, m, format!("Connected to {name} ({n} models)"))
        }
        (true, Some(Err(e))) => (true, vec![], format!("Key set (couldn't list {name}'s models: {e})")),
        (true, None) => (true, vec![], "Key set".into()),
    };
    out.push(BackendStatus {
        backend: Backend::Api,
        ready,
        path: None,
        note,
        features: vec![],
        models,
    });
    out
}

/// The provider's model catalog (`GET {base}/models`), tool-capable ones only where it says;
/// Claude models first.
pub fn api_models(api: &ApiConn) -> Result<Vec<ModelInfo>, String> {
    let v = curl_json(api, &api.url("models"), None, 20)?;
    Ok(parse_api_models(&v))
}

pub fn parse_api_models(v: &Value) -> Vec<ModelInfo> {
    let list = v["data"].as_array().or(v["models"].as_array());
    let mut out: Vec<ModelInfo> = list
        .into_iter()
        .flatten()
        // (OpenRouter lists what each model supports: our runs need tool calls)
        // (batch-only variants answer hours later)
        .filter(|m| m["id"].as_str().is_some_and(|id| !id.ends_with(":batch")))
        .filter(|m| {
            m["supported_parameters"]
                .as_array()
                .is_none_or(|a| a.iter().any(|p| p == "tools"))
        })
        .filter_map(|m| {
            let slug = m["id"].as_str()?.to_string();
            Some(ModelInfo {
                name: m["name"]
                    .as_str()
                    .or(m["display_name"].as_str())
                    .unwrap_or(&slug)
                    .to_string(),
                slug,
                description: String::new(),
                efforts: vec![],
                default_effort: String::new(),
            })
        })
        .collect();
    // the strongest Claude first (the default), then the rest in the provider's order
    out.sort_by_key(|m| {
        let s = &m.slug;
        if s.contains("claude-opus") {
            0
        } else if s.contains("claude-sonnet") {
            1
        } else if s.contains("claude") {
            2
        } else {
            3
        }
    });
    out
}

/// One run of an agent.
pub struct RunSpec {
    pub backend: Backend,
    pub model: String,
    /// Codex: reasoning effort ("medium", "high", "xhigh", …); `None` = the model's default.
    pub effort: Option<String>,
    /// Codex: feature flags it knows (from `detect`).
    pub features: Vec<String>,
    pub exe: Option<PathBuf>,
    pub instructions: String,
    pub prompt: String,
    /// The run's private MCP endpoint.
    pub port: u16,
    /// API backend: the provider and its key.
    pub api: Option<ApiConn>,
    /// API backend: the conversation so far (refine continues it).
    pub messages: Vec<Value>,
    /// Claude Code: session to resume (refine).
    pub resume: Option<String>,
    pub timeout: Duration,
    pub workdir: PathBuf,
    /// Reference images for the agent (copies in `workdir`, at most 1600 px).
    pub images: Vec<PathBuf>,
}

/// A reference image prepared for an agent: scaled to at most 1600 px on its long side and
/// written as PNG into the run's folder. Returns the copy's path.
pub fn prepare_image(src: &Path, workdir: &Path, index: usize) -> Result<PathBuf, String> {
    let img = image::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let (w, h) = (img.width(), img.height());
    let k = 1600.0 / w.max(h) as f32;
    let img = if k < 1.0 {
        img.resize(
            (w as f32 * k) as u32,
            (h as f32 * k) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let out = workdir.join(format!("reference_{}.png", index + 1));
    img.to_rgb8().save(&out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Progress outside tool calls ("Starting Codex…").
    Status(String),
    /// What the agent says between tool calls.
    Message(String),
    Finished {
        summary: String,
        /// Claude Code session id (refine resumes it).
        session: Option<String>,
        /// API backend: the whole conversation (refine continues it).
        messages: Vec<Value>,
    },
    Failed(String),
}

pub struct AgentHandle {
    /// (A mutex keeps the handle shareable as part of a resource.)
    pub rx: Mutex<Receiver<AgentEvent>>,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl AgentHandle {
    /// Stops the run (kills the agent's process group).
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        kill(&self.child);
    }
}

impl Drop for AgentHandle {
    /// (closing the editor or ending a run never leaves an agent running)
    fn drop(&mut self) {
        if !self.cancel.load(Ordering::Relaxed) {
            self.stop();
        }
    }
}

fn kill(child: &Arc<Mutex<Option<Child>>>) {
    if let Some(c) = child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        // (already finished: its pid may belong to someone else by now)
        if matches!(c.try_wait(), Ok(Some(_))) {
            return;
        }
        // the CLI and everything it started (node launcher → native binary)
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(format!("-{}", c.id()))
            .status();
        std::thread::sleep(Duration::from_millis(150));
        let _ = Command::new("/bin/kill")
            .arg("-KILL")
            .arg(format!("-{}", c.id()))
            .status();
        let _ = c.kill();
    }
}

pub fn start(spec: RunSpec) -> AgentHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let child = Arc::new(Mutex::new(None));
    let (c2, ch2) = (cancel.clone(), child.clone());
    let _ = std::thread::Builder::new()
        .name("prompt-agent".into())
        .spawn(move || {
            let deadline = Instant::now() + spec.timeout;
            let mins = (spec.timeout.as_secs() as f32 / 60.0).round();
            // the watchdog: kill on timeout
            let (c3, ch3, tx3) = (c2.clone(), ch2.clone(), tx.clone());
            std::thread::spawn(move || {
                while Instant::now() < deadline {
                    if c3.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                c3.store(true, Ordering::Relaxed);
                let _ = tx3.send(AgentEvent::Failed(format!("Timed out after {mins} min")));
                kill(&ch3);
            });
            let res = match (stub_path(), spec.backend) {
                (Some(script), _) => run_stub(&spec, &script, &tx, &c2),
                (None, Backend::Codex) => run_codex(&spec, &tx, &ch2, &c2),
                (None, Backend::ClaudeCode) => run_claude(&spec, &tx, &ch2, &c2),
                (None, Backend::Api) => run_api(&spec, &tx, &c2),
            };
            c2.store(true, Ordering::Relaxed);
            if let Err(e) = res {
                let _ = tx.send(AgentEvent::Failed(e));
            }
        });
    AgentHandle {
        rx: Mutex::new(rx),
        cancel,
        child,
    }
}

/// Spawns a CLI in its own process group with stdout piped; feeds `stdin_text`.
fn spawn(
    mut cmd: Command,
    stdin_text: &str,
    child: &Arc<Mutex<Option<Child>>>,
) -> Result<std::process::ChildStdout, String> {
    use std::os::unix::process::CommandExt;
    cmd.env("PATH", child_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut c = cmd
        .spawn()
        .map_err(|e| format!("could not start the agent: {e}"))?;
    if let Some(mut si) = c.stdin.take() {
        let _ = si.write_all(stdin_text.as_bytes());
    }
    let out = c.stdout.take().ok_or("no output from the agent")?;
    // keep stderr drained (a full pipe would block the CLI); its tail explains failures
    if let Some(mut se) = c.stderr.take() {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = se.read_to_string(&mut buf);
            let tail: String = buf
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !tail.trim().is_empty() {
                bevy::log::info!("prompt agent stderr (tail):\n{tail}");
            }
        });
    }
    *child.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
    Ok(out)
}

/// Waits for the CLI to exit (polling, so `stop` can still take the lock and kill it).
fn wait(child: &Arc<Mutex<Option<Child>>>) -> Option<std::process::ExitStatus> {
    loop {
        match child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()?
            .try_wait()
        {
            Ok(Some(s)) => return Some(s),
            Ok(None) => {}
            Err(_) => return None,
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ------------------------------------------------------------------ test stub

/// `WB_AGENT_STUB=<script.json>`: every run is a scripted stand-in for the AI (automated tests of
/// the run's plumbing without a real agent). The script: `{"calls": [{"name": "…", "arguments":
/// {…}}, …], "summary": "…"}`; its calls go through the run's private MCP endpoint like a real
/// agent's (tools outside the run's allowlist fail the same way).
pub fn stub_path() -> Option<PathBuf> {
    std::env::var("WB_AGENT_STUB")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
}

fn run_stub(
    spec: &RunSpec,
    script: &Path,
    tx: &Sender<AgentEvent>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let text = std::fs::read_to_string(script)
        .map_err(|e| format!("stub script {}: {e}", script.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("stub script: {e}"))?;
    let _ = tx.send(AgentEvent::Status(format!(
        "Test stub: {} scripted calls ({} reference image{})",
        v["calls"].as_array().map_or(0, |a| a.len()),
        spec.images.len(),
        if spec.images.len() == 1 { "" } else { "s" }
    )));
    let listed = mcp_rpc(spec.port, 1, "tools/list", json!({}))?;
    let n = listed["tools"].as_array().map_or(0, |a| a.len());
    let _ = tx.send(AgentEvent::Message(format!("{n} tools offered.")));
    for (i, c) in v["calls"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        std::thread::sleep(Duration::from_millis(250));
        let name = c["name"].as_str().unwrap_or("").to_string();
        let r = mcp_rpc(
            spec.port,
            10 + i as u64,
            "tools/call",
            json!({ "name": name, "arguments": c.get("arguments").cloned().unwrap_or(json!({})) }),
        );
        let first = match &r {
            Ok(res) => res["content"]
                .as_array()
                .and_then(|a| a.iter().find_map(|x| x["text"].as_str()))
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>(),
            Err(e) => format!("error: {e}"),
        };
        bevy::log::info!("agent stub: {name} -> {first}");
    }
    let summary = v["summary"]
        .as_str()
        .unwrap_or("Done (test stub).")
        .to_string();
    let _ = tx.send(AgentEvent::Finished {
        summary,
        session: None,
        messages: vec![],
    });
    Ok(())
}

// ------------------------------------------------------------------ Codex

/// The `codex exec` arguments (the prompt comes on stdin).
/// The variable Codex reads the MCP key from (`bearer_token_env_var`).
const MCP_KEY_ENV: &str = "SKYFOLD_MCP_KEY";

/// Writes a file only this user can read (it holds the MCP key).
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    std::io::Write::write_all(&mut opts.open(path)?, text.as_bytes())
}

pub fn codex_args(spec: &RunSpec) -> Vec<String> {
    let mut a: Vec<String> = [
        "exec",
        "--json",
        "--ephemeral",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "-s",
        "read-only",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend([
        "-m".into(),
        spec.model.clone(),
        "-C".into(),
        spec.workdir.to_string_lossy().to_string(),
    ]);
    let mut c = |kv: String| {
        a.push("-c".into());
        a.push(kv);
    };
    if let Some(e) = &spec.effort {
        c(format!("model_reasoning_effort=\"{e}\""));
    }
    c("approval_policy=\"never\"".into());
    c("web_search=\"disabled\"".into());
    c(format!(
        "mcp_servers.world_builder.url=\"http://127.0.0.1:{}/mcp\"",
        spec.port
    ));
    // the key comes from the environment (run_codex), never the command line
    c(format!("mcp_servers.world_builder.bearer_token_env_var=\"{MCP_KEY_ENV}\""));
    c("mcp_servers.world_builder.default_tools_approval_mode=\"approve\"".into());
    c("mcp_servers.world_builder.tool_timeout_sec=300".into());
    c("mcp_servers.world_builder.startup_timeout_sec=30".into());
    // everything but our MCP tools off (only flags this version knows). `code_mode_host` stays
    // on: newer Codex versions call every tool, MCP ones included, through it.
    for f in [
        "shell_tool",
        "unified_exec",
        "apps",
        "browser_use",
        "browser_use_external",
        "computer_use",
        "image_generation",
        "in_app_browser",
        "multi_agent",
        "plugins",
        "tool_suggest",
        "goals",
        "skill_mcp_dependency_install",
        "hooks",
    ] {
        if spec.features.iter().any(|k| k == f) {
            a.push("--disable".into());
            a.push(f.into());
        }
    }
    for img in &spec.images {
        a.push("-i".into());
        a.push(img.to_string_lossy().to_string());
    }
    a.push("-".into());
    a
}

/// One `codex exec --json` line → events (and the last message, the summary).
pub fn parse_codex_line(line: &str, last_message: &mut String) -> Vec<AgentEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![];
    };
    let mut out = vec![];
    match v["type"].as_str().unwrap_or("") {
        "item.completed" => {
            let item = &v["item"];
            match item["type"].as_str().unwrap_or("") {
                "agent_message" => {
                    let t = item["text"].as_str().unwrap_or("").trim().to_string();
                    if !t.is_empty() {
                        *last_message = t.clone();
                        out.push(AgentEvent::Message(t));
                    }
                }
                "error" => out.push(AgentEvent::Status(format!(
                    "Note: {}",
                    item["message"].as_str().unwrap_or("error")
                ))),
                _ => {}
            }
        }
        "item.started" if v["item"]["type"] == "reasoning" => {
            out.push(AgentEvent::Status("Thinking…".into()))
        }
        "turn.failed" => out.push(AgentEvent::Failed(
            v["error"]["message"]
                .as_str()
                .unwrap_or("the turn failed")
                .to_string(),
        )),
        "error" => out.push(AgentEvent::Failed(
            v["message"]
                .as_str()
                .unwrap_or("Codex reported an error")
                .to_string(),
        )),
        _ => {}
    }
    out
}

fn run_codex(
    spec: &RunSpec,
    tx: &Sender<AgentEvent>,
    child: &Arc<Mutex<Option<Child>>>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let exe = spec
        .exe
        .clone()
        .or_else(|| find("codex"))
        .ok_or("Codex is not installed")?;
    let _ = tx.send(AgentEvent::Status(format!(
        "Starting Codex ({})…",
        spec.model
    )));
    let mut cmd = Command::new(&exe);
    cmd.args(codex_args(spec))
        .current_dir(&spec.workdir)
        .env(MCP_KEY_ENV, crate::mcp::key::get())
        .env_remove("SKYFOLD_MARKET_TOKEN");
    let stdin = format!("{}\n\n{}", spec.instructions, spec.prompt);
    let out = spawn(cmd, &stdin, child)?;
    let mut last = String::new();
    let mut failed = None;
    for line in BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        for ev in parse_codex_line(&line, &mut last) {
            if let AgentEvent::Failed(e) = &ev {
                failed = Some(e.clone());
                continue;
            }
            let _ = tx.send(ev);
        }
    }
    let status = wait(child);
    if cancel.load(Ordering::Relaxed) && failed.is_none() && !status.is_some_and(|s| s.success()) {
        return Err("Stopped".into());
    }
    match (failed, status) {
        (Some(e), _) => Err(e),
        (None, Some(s)) if s.success() => {
            let _ = tx.send(AgentEvent::Finished {
                summary: last,
                session: None,
                messages: vec![],
            });
            Ok(())
        }
        (None, s) => Err(format!(
            "Codex stopped unexpectedly ({})",
            s.map(|s| s.to_string()).unwrap_or_default()
        )),
    }
}

// ------------------------------------------------------------------ Claude Code

pub fn claude_args(spec: &RunSpec, mcp_config: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--mcp-config".into(),
        mcp_config.to_string_lossy().to_string(),
        "--strict-mcp-config".into(),
        "--tools".into(),
        // (reading files only to look at the attached reference images, in its own folder)
        if spec.images.is_empty() {
            "".into()
        } else {
            "Read".into()
        },
        "--allowedTools".into(),
        if spec.images.is_empty() {
            "mcp__world_builder".into()
        } else {
            // Read only inside the run folder (the images), not the user's files or keys
            format!(
                "mcp__world_builder,Read(/{}/**)",
                spec.workdir.to_string_lossy().trim_end_matches('/')
            )
        },
        "--permission-mode".into(),
        "dontAsk".into(),
        "--model".into(),
        spec.model.clone(),
        "--append-system-prompt".into(),
        spec.instructions.clone(),
    ];
    if let Some(s) = &spec.resume {
        a.push("--resume".into());
        a.push(s.clone());
    }
    a
}

/// One `claude -p --output-format stream-json` line → events; `result` lines finish the run.
pub fn parse_claude_line(line: &str, session: &mut Option<String>) -> Vec<AgentEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![];
    };
    if let Some(s) = v["session_id"].as_str() {
        *session = Some(s.to_string());
    }
    let mut out = vec![];
    match v["type"].as_str().unwrap_or("") {
        "assistant" => {
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                if b["type"] == "text"
                    && let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty())
                {
                    out.push(AgentEvent::Message(t.trim().to_string()));
                }
            }
        }
        "result" => {
            let text = v["result"].as_str().unwrap_or("").trim().to_string();
            if v["is_error"].as_bool() == Some(true)
                || v["subtype"].as_str().is_some_and(|s| s != "success")
            {
                out.push(AgentEvent::Failed(if text.is_empty() {
                    format!("Claude Code: {}", v["subtype"].as_str().unwrap_or("error"))
                } else {
                    text
                }));
            } else {
                out.push(AgentEvent::Finished {
                    summary: text,
                    session: session.clone(),
                    messages: vec![],
                });
            }
        }
        _ => {}
    }
    out
}

fn run_claude(
    spec: &RunSpec,
    tx: &Sender<AgentEvent>,
    child: &Arc<Mutex<Option<Child>>>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let exe = spec
        .exe
        .clone()
        .or_else(find_claude)
        .ok_or("Claude Code is not installed")?;
    let cfg = spec.workdir.join("mcp.json");
    let conf = json!({ "mcpServers": { "world_builder": { "type": "http", "url": format!("http://127.0.0.1:{}/mcp", spec.port), "headers": { "Authorization": crate::mcp::key::bearer() } } } });
    write_private(&cfg, &conf.to_string()).map_err(|e| e.to_string())?;
    let _ = tx.send(AgentEvent::Status(format!(
        "Starting Claude Code ({})…",
        spec.model
    )));
    let mut cmd = Command::new(&exe);
    cmd.args(claude_args(spec, &cfg))
        .current_dir(&spec.workdir)
        .env_remove("SKYFOLD_MARKET_TOKEN");
    let out = spawn(cmd, &spec.prompt, child)?;
    let mut session = spec.resume.clone();
    let mut done = false;
    for line in BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        for ev in parse_claude_line(&line, &mut session) {
            done |= matches!(ev, AgentEvent::Finished { .. } | AgentEvent::Failed(_));
            let _ = tx.send(ev);
        }
    }
    let status = wait(child);
    if done {
        return Ok(());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Stopped".into());
    }
    Err(format!(
        "Claude Code stopped without an answer ({}). Is it logged in?",
        status.map(|s| s.to_string()).unwrap_or_default()
    ))
}

// ------------------------------------------------------------------ API key (OpenAI-compatible)

const MAX_TURNS: usize = 80;

/// The MCP `tools/list` result → Chat Completions tool definitions.
pub fn chat_tools(list: &Value) -> Vec<Value> {
    list["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| json!({ "type": "function", "function": { "name": t["name"], "description": t["description"], "parameters": t["inputSchema"] } }))
        .collect()
}

/// Standard base64 (for image data URLs).
fn base64_encode(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

fn image_part(mime: &str, data_b64: &str) -> Value {
    json!({ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{data_b64}") } })
}

/// One Chat Completions request body (the system prompt goes first each time).
pub fn chat_request(
    model: &str,
    system: &str,
    tools: &[Value],
    messages: &[Value],
    base: &str,
) -> Value {
    let mut all = vec![json!({ "role": "system", "content": system })];
    all.extend(messages.iter().cloned());
    let mut v = json!({ "model": model, "messages": all, "tools": tools });
    // (OpenAI's reasoning models take only the newer name)
    let limit = if base.contains("api.openai.com") {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    v[limit] = json!(16000);
    v
}

/// The answer's message (`choices[0].message`) and why it stopped.
pub fn chat_message(resp: &Value) -> (Value, String) {
    let c = &resp["choices"][0];
    (
        c["message"].clone(),
        c["finish_reason"].as_str().unwrap_or("").to_string(),
    )
}

/// The tool calls in a message: (id, name, input).
pub fn chat_tool_calls(msg: &Value) -> Vec<(String, String, Value)> {
    msg["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let f = &t["function"];
            // (arguments arrive as a JSON string; some providers send the object)
            let input = match &f["arguments"] {
                Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
                Value::Object(_) => f["arguments"].clone(),
                _ => json!({}),
            };
            (
                t["id"].as_str().unwrap_or("").to_string(),
                f["name"].as_str().unwrap_or("").to_string(),
                input,
            )
        })
        .collect()
}

pub fn chat_text(msg: &Value) -> String {
    match &msg["content"] {
        Value::String(s) => s.trim().to_string(),
        Value::Array(a) => a
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// An MCP `tools/call` result → a `tool` message (its text) and image parts for a following user
/// message (tool messages take text only).
pub fn chat_tool_result(id: &str, mcp_result: &Value) -> (Value, Vec<Value>) {
    let mut text = vec![];
    let mut images = vec![];
    for c in mcp_result["content"].as_array().into_iter().flatten() {
        match c["type"].as_str() {
            Some("image") => images.push(image_part(
                c["mimeType"].as_str().unwrap_or("image/png"),
                c["data"].as_str().unwrap_or(""),
            )),
            _ => text.push(c["text"].as_str().unwrap_or("").to_string()),
        }
    }
    let mut body = text.join("\n");
    if mcp_result["isError"].as_bool() == Some(true) {
        body = format!("Error: {body}");
    }
    if !images.is_empty() {
        body += &format!("\n({} image(s) follow in the next message)", images.len());
    }
    (
        json!({ "role": "tool", "tool_call_id": id, "content": body }),
        images,
    )
}

/// JSON-RPC to the run's endpoint.
fn mcp_rpc(port: u16, id: u64, method: &str, params: Value) -> Result<Value, String> {
    let body =
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
    let (status, _, text) = crate::mcp::bridge::post(port, &body, None)
        .map_err(|e| format!("editor unreachable: {e}"))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("bad answer from the editor ({status}): {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(e["message"].as_str().unwrap_or("MCP error").to_string());
    }
    Ok(v["result"].clone())
}

/// GETs (or POSTs `body` to) the provider with curl; the key and headers go through stdin
/// (`-K -`), a body through a private temp file.
fn curl_json(api: &ApiConn, url: &str, body: Option<&Value>, max_secs: u32) -> Result<Value, String> {
    if !valid_base(url) {
        return Err(format!("not a usable URL: {url}"));
    }
    let mut config = format!("url = \"{url}\"\n");
    if let Some(k) = &api.key {
        let k = k.trim();
        if k.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\') {
            return Err("the API key has spaces or quotes in it".into());
        }
        config += &format!("header = \"Authorization: Bearer {k}\"\n");
        if url.contains("api.anthropic.com") {
            config += &format!("header = \"x-api-key: {k}\"\nheader = \"anthropic-version: 2023-06-01\"\n");
        }
    }
    if url.contains("openrouter.ai") {
        config += "header = \"HTTP-Referer: https://skyfoldstudio.com\"\nheader = \"X-Title: Skyfold Studio\"\n";
    }
    let file = body
        .map(|b| -> Result<PathBuf, String> {
            let f = std::env::temp_dir().join(format!(
                "skyfold-api-{}-{}.json",
                std::process::id(),
                fastrand::u64(..)
            ));
            std::fs::write(&f, b.to_string()).map_err(|e| e.to_string())?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600));
            Ok(f)
        })
        .transpose()?;
    if let Some(f) = &file {
        config += &format!(
            "header = \"content-type: application/json\"\ndata-binary = \"@{}\"\n",
            f.display()
        );
    }
    let run = || -> Result<std::process::Output, String> {
        let mut c = Command::new("/usr/bin/curl")
            .args(["-sS", "--max-time", &max_secs.to_string(), "-K", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("curl: {e}"))?;
        if let Some(mut si) = c.stdin.take() {
            let _ = si.write_all(config.as_bytes());
        }
        c.wait_with_output().map_err(|e| e.to_string())
    };
    let out = run();
    if let Some(f) = &file {
        let _ = std::fs::remove_file(f);
    }
    let out = out?;
    if !out.status.success() {
        return Err(format!(
            "network error: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        let t = String::from_utf8_lossy(&out.stdout);
        format!("unreadable answer: {}", t.trim().chars().take(160).collect::<String>())
    })?;
    if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
        let msg = e["message"]
            .as_str()
            .or(e.as_str())
            .unwrap_or("error")
            .to_string();
        return Err(msg);
    }
    Ok(v)
}

fn run_api(spec: &RunSpec, tx: &Sender<AgentEvent>, cancel: &AtomicBool) -> Result<(), String> {
    let api = spec.api.clone().filter(ApiConn::ready).ok_or("Add an API key first")?;
    let name = provider(&api.base).map_or("the API", |p| p.name);
    let _ = tx.send(AgentEvent::Status(format!(
        "Calling {name} ({})…",
        spec.model
    )));
    let tools = chat_tools(&mcp_rpc(spec.port, 1, "tools/list", json!({}))?);
    let mut messages = spec.messages.clone();
    if spec.images.is_empty() {
        messages.push(json!({ "role": "user", "content": spec.prompt }));
    } else {
        let mut content: Vec<Value> = spec
            .images
            .iter()
            .filter_map(|p| std::fs::read(p).ok())
            .map(|b| image_part("image/png", &base64_encode(&b)))
            .collect();
        content.push(json!({ "type": "text", "text": spec.prompt }));
        messages.push(json!({ "role": "user", "content": content }));
    }
    let url = api.url("chat/completions");
    let mut rpc_id = 10;
    let mut summary = String::new();
    for _ in 0..MAX_TURNS {
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        let resp = curl_json(
            &api,
            &url,
            Some(&chat_request(
                &spec.model,
                &spec.instructions,
                &tools,
                &messages,
                &api.base,
            )),
            600,
        )
        .map_err(|e| format!("{name}: {e}"))?;
        let (msg, finish) = chat_message(&resp);
        if msg.is_null() {
            return Err(format!("{name} sent no answer"));
        }
        let calls = chat_tool_calls(&msg);
        // (only what every provider takes back: reasoning and extra fields stay out)
        let mut back = json!({ "role": "assistant", "content": msg["content"] });
        if !calls.is_empty() {
            back["tool_calls"] = msg["tool_calls"].clone();
        }
        messages.push(back);
        let said = chat_text(&msg);
        if !said.is_empty() {
            summary = said.clone();
            let _ = tx.send(AgentEvent::Message(said));
        }
        if !calls.is_empty() {
            let mut images = vec![];
            for (id, name, input) in calls {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Stopped".into());
                }
                rpc_id += 1;
                let r = mcp_rpc(
                    spec.port,
                    rpc_id,
                    "tools/call",
                    json!({ "name": name, "arguments": input }),
                )
                .unwrap_or_else(
                    |e| json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
                );
                let (m, imgs) = chat_tool_result(&id, &r);
                messages.push(m);
                images.extend(imgs);
            }
            if !images.is_empty() {
                let mut content = vec![json!({ "type": "text", "text": "The images from those tool results:" })];
                content.extend(images);
                messages.push(json!({ "role": "user", "content": content }));
            }
            continue;
        }
        match finish.as_str() {
            "content_filter" => return Err("The model declined this request.".into()),
            "length" => {
                messages.push(json!({ "role": "user", "content": "Continue, and finish with a short summary." }));
            }
            _ => {
                let _ = tx.send(AgentEvent::Finished {
                    summary,
                    session: None,
                    messages,
                });
                return Ok(());
            }
        }
    }
    let _ = tx.send(AgentEvent::Finished {
        summary,
        session: None,
        messages,
    });
    Ok(())
}

// ------------------------------------------------------------------ Sign in with Claude

/// `claude auth login` running in the background: it opens the browser; when the browser can't
/// hand the sign-in back, the page shows a code to paste (`submit_code`).
pub struct ClaudeLogin {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Option<std::process::ChildStdin>,
    lines: Mutex<Receiver<String>>,
    /// The sign-in page (to open again).
    pub url: Option<String>,
    /// Finished: signed in, or why not.
    pub done: Option<Result<(), String>>,
    output: Vec<String>,
}

impl ClaudeLogin {
    pub fn start() -> Result<ClaudeLogin, String> {
        use std::os::unix::process::CommandExt;
        let exe = find_claude().ok_or("Claude Code is not installed (claude.com/claude-code)")?;
        let mut c = Command::new(exe)
            .args(["auth", "login", "--claudeai"])
            .env("PATH", child_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("couldn't start Claude Code: {e}"))?;
        let (tx, rx) = mpsc::channel();
        // (its prompt has no newline: read raw chunks)
        for out in [
            c.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>),
            c.stderr.take().map(|o| Box::new(o) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut out = out;
                let mut buf = [0u8; 4096];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 || tx.send(String::from_utf8_lossy(&buf[..n]).into()).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(ClaudeLogin {
            stdin: c.stdin.take(),
            child: Arc::new(Mutex::new(Some(c))),
            lines: Mutex::new(rx),
            url: None,
            done: None,
            output: vec![],
        })
    }

    /// Reads its output and whether it has finished (call every frame).
    pub fn poll(&mut self) {
        if self.done.is_some() {
            return;
        }
        if let Ok(rx) = self.lines.lock() {
            while let Ok(s) = rx.try_recv() {
                self.output.push(s);
            }
        }
        if self.url.is_none() {
            let all = self.output.concat();
            self.url = all
                .split_whitespace()
                .find(|w| w.starts_with("https://"))
                .map(str::to_string);
        }
        let status = self
            .child
            .lock()
            .ok()
            .and_then(|mut c| c.as_mut()?.try_wait().ok().flatten());
        if let Some(st) = status {
            self.done = Some(if st.success() {
                Ok(())
            } else {
                let all = self.output.concat();
                let last = all
                    .lines()
                    .map(str::trim)
                    .rfind(|l| !l.is_empty() && !l.starts_with("Paste code"))
                    .unwrap_or("sign-in didn't finish")
                    .to_string();
                Err(last)
            });
        }
    }

    /// The code the sign-in page shows when it can't hand the sign-in back by itself.
    pub fn submit_code(&mut self, code: &str) {
        if let Some(si) = self.stdin.as_mut() {
            let _ = si.write_all(format!("{}\n", code.trim()).as_bytes());
            let _ = si.flush();
        }
    }
}

impl Drop for ClaudeLogin {
    /// (closing the editor or the panel never leaves a sign-in running)
    fn drop(&mut self) {
        if self.done.is_none() {
            kill(&self.child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RunSpec {
        RunSpec {
            backend: Backend::Codex,
            model: "gpt-6.1-sol".into(),
            effort: Some("xhigh".into()),
            features: vec!["shell_tool".into(), "apps".into()],
            exe: None,
            instructions: "be good".into(),
            prompt: "make volcanoes".into(),
            port: 7811,
            api: None,
            messages: vec![],
            resume: Some("abc".into()),
            timeout: Duration::from_secs(60),
            workdir: PathBuf::from("/tmp/x"),
            images: vec![PathBuf::from("/tmp/x/reference_1.png")],
        }
    }

    #[test]
    fn codex_args_lock_down() {
        let a = codex_args(&spec());
        let s = a.join(" ");
        assert!(
            s.contains("--ignore-user-config")
                && s.contains("-s read-only")
                && s.contains("--ephemeral")
        );
        assert!(s.contains("mcp_servers.world_builder.url=\"http://127.0.0.1:7811/mcp\""));
        assert!(s.contains("mcp_servers.world_builder.bearer_token_env_var=\"SKYFOLD_MCP_KEY\""));
        assert!(!s.contains(crate::mcp::key::get()), "the key never goes on the command line");
        assert!(s.contains("default_tools_approval_mode=\"approve\""));
        assert!(s.contains("--disable shell_tool") && s.contains("--disable apps"));
        assert!(s.contains("-m gpt-6.1-sol") && s.contains("model_reasoning_effort=\"xhigh\""));
        assert!(s.contains("-i /tmp/x/reference_1.png"));
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        // flags this version doesn't know are not passed
        assert!(!s.contains("--disable hooks"));
        assert_eq!(a.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn codex_model_catalog() {
        let json = r#"{"models":[
            {"slug":"gpt-6-astra","display_name":"GPT-6-Astra","visibility":"list","priority":2,"default_reasoning_level":"medium","description":"Frontier","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"}]},
            {"slug":"gpt-reserve","visibility":"hide","priority":1},
            {"slug":"gpt-6.1-sol","display_name":"GPT-6.1-Sol","visibility":"list","priority":1,"default_reasoning_level":"low","supported_reasoning_levels":[{"effort":"medium"},{"effort":"high"}]}]}"#;
        let m = parse_codex_models(json);
        assert_eq!(
            m.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(),
            ["gpt-6.1-sol", "gpt-6-astra"]
        );
        assert_eq!(m[1].efforts, ["low", "medium", "high", "xhigh"]);
        assert_eq!(m[0].name, "GPT-6.1-Sol");
        assert_eq!(effort_name("xhigh"), "Extra high");
        assert_eq!(effort_name("medium"), "Medium");
        assert!(parse_codex_models("not json").is_empty());
    }

    #[test]
    fn codex_events() {
        let mut last = String::new();
        let ev = parse_codex_line(
            r#"{"type":"item.completed","item":{"id":"i","type":"agent_message","text":"Built two volcanoes."}}"#,
            &mut last,
        );
        assert!(matches!(&ev[0], AgentEvent::Message(t) if t == "Built two volcanoes."));
        assert_eq!(last, "Built two volcanoes.");
        let ev = parse_codex_line(
            r#"{"type":"turn.failed","error":{"message":"quota"}}"#,
            &mut last,
        );
        assert!(matches!(&ev[0], AgentEvent::Failed(t) if t == "quota"));
        assert!(parse_codex_line("not json", &mut last).is_empty());
    }

    #[test]
    fn claude_args_and_events() {
        let a = claude_args(&spec(), Path::new("/tmp/x/mcp.json"));
        let s = a.join(" ");
        assert!(
            s.contains("--strict-mcp-config")
                && s.contains("--allowedTools mcp__world_builder")
                && s.contains("--resume abc")
        );
        let i = a.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(a[i + 1], "Read");
        assert!(s.contains("--allowedTools mcp__world_builder,Read(//tmp/x/**)"), "{s}");
        let mut session = None;
        let ev = parse_claude_line(
            r#"{"type":"system","subtype":"init","session_id":"s1"}"#,
            &mut session,
        );
        assert!(ev.is_empty());
        assert_eq!(session.as_deref(), Some("s1"));
        let ev = parse_claude_line(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Done: a canyon.","session_id":"s1"}"#,
            &mut session,
        );
        assert!(
            matches!(&ev[0], AgentEvent::Finished { summary, session: Some(s), .. } if summary == "Done: a canyon." && s == "s1")
        );
    }

    #[test]
    fn api_round_trip() {
        let list = json!({ "tools": [{ "name": "selection_info", "description": "d", "inputSchema": { "type": "object", "properties": {} } }] });
        let tools = chat_tools(&list);
        assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
        let msgs = [json!({ "role": "user", "content": "hi" })];
        let req = chat_request("anthropic/claude", "sys", &tools, &msgs, "https://openrouter.ai/api/v1");
        assert_eq!(req["messages"][0]["role"], "system");
        assert_eq!(req["messages"][1]["content"], "hi");
        assert_eq!(req["max_tokens"], 16000);
        assert_eq!(req["tools"][0]["function"]["name"], "selection_info");
        let o = chat_request("gpt", "sys", &tools, &msgs, "https://api.openai.com/v1");
        assert!(o.get("max_tokens").is_none() && o["max_completion_tokens"] == 16000);
        let resp = json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": "Looking first.",
            "tool_calls": [{ "id": "call_1", "type": "function", "function": { "name": "selection_screenshot", "arguments": "{\"views\":[\"top\"]}" } }]
        } }] });
        let (msg, finish) = chat_message(&resp);
        assert_eq!(finish, "tool_calls");
        assert_eq!(chat_text(&msg), "Looking first.");
        let calls = chat_tool_calls(&msg);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "selection_screenshot");
        assert_eq!(calls[0].2["views"][0], "top");
        let mcp = json!({ "content": [{ "type": "image", "data": "AAAA", "mimeType": "image/png" }, { "type": "text", "text": "Top view" }], "isError": false });
        let (m, imgs) = chat_tool_result("call_1", &mcp);
        assert_eq!(m["role"], "tool");
        assert_eq!(m["tool_call_id"], "call_1");
        assert!(m["content"].as_str().unwrap().starts_with("Top view"));
        assert_eq!(imgs[0]["image_url"]["url"], "data:image/png;base64,AAAA");
        let (e, _) = chat_tool_result("c", &json!({ "content": [{ "type": "text", "text": "bad" }], "isError": true }));
        assert_eq!(e["content"], "Error: bad");
    }

    #[test]
    fn providers_and_models() {
        assert_eq!(Backend::from_id("anthropic_api"), Some(Backend::Api));
        assert!(provider("https://openrouter.ai/api/v1/").is_some());
        assert!(!valid_base("openrouter.ai") && !valid_base("https://a b"));
        let local = ApiConn { base: "http://localhost:11434/v1".into(), key: None };
        assert!(local.ready());
        let or = ApiConn { base: "https://openrouter.ai/api/v1".into(), key: None };
        assert!(!or.ready());
        let v = json!({ "data": [
            { "id": "x/no-tools", "supported_parameters": ["temperature"] },
            { "id": "openai/gpt", "name": "GPT", "supported_parameters": ["tools"] },
            { "id": "anthropic/claude-haiku-5.5", "supported_parameters": ["tools"] },
            { "id": "anthropic/claude-opus-5.5", "supported_parameters": ["tools"] },
            { "id": "anthropic/claude-opus-5.5:batch", "supported_parameters": ["tools"] }
        ] });
        let m = parse_api_models(&v);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].slug, "anthropic/claude-opus-5.5");
        assert_eq!(m[1].slug, "anthropic/claude-haiku-5.5");
    }
}
