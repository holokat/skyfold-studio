//! Image models, from three kinds of backend the user picks between (nobody is locked into one):
//!
//! - **Codex** (`codex`): the Codex CLI with the user's ChatGPT sign-in and its `$imagegen` skill
//!   (`codex.rs`). No key.
//! - **Cloudflare Workers AI** (`cloudflare`): FLUX models on the user's Cloudflare account
//!   (account id + API token, `cloudflare.rs`).
//! - **Black Forest Labs** (`bfl`): FLUX straight from BFL's API with the user's own key (`bfl.rs`).
//!
//! A model is named `backend:model` ("cloudflare:@cf/black-forest-labs/flux-2-klein-9b"). Each
//! backend has a built-in list and takes any other model id the user adds (a new FLUX the day it
//! ships). Generation is blocking and runs on a worker thread (`super::jobs`). HTTP goes through
//! the system `curl` like the prompt agent's and the marketplace's calls: keys travel in curl's
//! config on stdin, never on its command line, and are never logged.

pub mod bfl;
pub mod cloudflare;
pub mod codex;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BackendKind {
    Codex,
    Cloudflare,
    Bfl,
}

impl BackendKind {
    pub const ALL: [BackendKind; 3] = [BackendKind::Codex, BackendKind::Cloudflare, BackendKind::Bfl];
    pub fn id(self) -> &'static str {
        match self {
            BackendKind::Codex => "codex",
            BackendKind::Cloudflare => "cloudflare",
            BackendKind::Bfl => "bfl",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id() == s)
    }
    pub fn name(self) -> &'static str {
        match self {
            BackendKind::Codex => "Codex (ChatGPT)",
            BackendKind::Cloudflare => "Cloudflare Workers AI",
            BackendKind::Bfl => "Black Forest Labs",
        }
    }
    /// The Keychain account of its key ("" = no key).
    pub fn key_account(self) -> &'static str {
        match self {
            BackendKind::Codex => "",
            BackendKind::Cloudflare => "cloudflare_api_token",
            BackendKind::Bfl => "bfl_api_key",
        }
    }
    /// Where to get a key.
    pub fn keys_at(self) -> &'static str {
        match self {
            BackendKind::Codex => "",
            BackendKind::Cloudflare => "dash.cloudflare.com/profile/api-tokens",
            BackendKind::Bfl => "dashboard.bfl.ai",
        }
    }
    /// Its built-in models.
    pub fn models(self) -> &'static [ModelSpec] {
        match self {
            BackendKind::Codex => codex::MODELS,
            BackendKind::Cloudflare => cloudflare::MODELS,
            BackendKind::Bfl => bfl::MODELS,
        }
    }
}

/// How a model takes its picture size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeStyle {
    /// width and height in pixels, rounded to a multiple of `step`, `min..=max` per side
    Pixels { step: u32, min: u32, max: u32 },
    /// an aspect ratio string ("16:9")
    Ratio,
    /// square only
    Square,
    /// said in words (Codex)
    Words,
}

/// A built-in model.
#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub note: &'static str,
    /// reference images it takes (0 = text only)
    pub max_refs: usize,
    pub size: SizeStyle,
}

/// A model the dropdown offers: built in, or an id the user added.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub backend: BackendKind,
    pub id: String,
    pub name: String,
    pub note: String,
    pub max_refs: usize,
    pub size: SizeStyle,
}

impl Model {
    /// "backend:model"
    pub fn key(&self) -> String {
        format!("{}:{}", self.backend.id(), self.id)
    }
    pub fn from_spec(b: BackendKind, s: &ModelSpec) -> Self {
        Model {
            backend: b,
            id: s.id.into(),
            name: s.name.into(),
            note: s.note.into(),
            max_refs: s.max_refs,
            size: s.size,
        }
    }
    /// A model id the user typed: a built-in's settings when it is one, else guessed from the id.
    pub fn custom(b: BackendKind, id: &str) -> Self {
        if let Some(s) = b.models().iter().find(|s| s.id == id) {
            return Self::from_spec(b, s);
        }
        let flux2 = id.contains("flux-2") || id.contains("flux-3");
        let (max_refs, size) = match b {
            BackendKind::Codex => (4, SizeStyle::Words),
            BackendKind::Cloudflare if !id.starts_with("@cf/") && flux2 => (10, SizeStyle::Ratio),
            BackendKind::Cloudflare if flux2 => (4, cloudflare::FLUX2_SIZE),
            BackendKind::Cloudflare => (0, SizeStyle::Square),
            BackendKind::Bfl if id.contains("kontext") || id.contains("ultra") => (1, SizeStyle::Ratio),
            BackendKind::Bfl => (if flux2 { 8 } else { 0 }, bfl::FLUX2_SIZE),
        };
        Model {
            backend: b,
            id: id.into(),
            name: id.rsplit('/').next().unwrap_or(id).to_string(),
            note: "Added by you".into(),
            max_refs,
            size,
        }
    }
}

/// Splits "backend:model".
pub fn parse_key(key: &str) -> Option<(BackendKind, &str)> {
    let (b, m) = key.split_once(':')?;
    let b = BackendKind::from_id(b)?;
    (!m.trim().is_empty()).then_some((b, m))
}

/// A model id the user may add: no spaces, quotes or path tricks.
pub fn valid_model_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && id.len() <= 120
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@/-_.:".contains(c))
}

/// The usual aspect ratios.
pub const ASPECTS: [&str; 5] = ["1:1", "4:3", "3:4", "16:9", "9:16"];

/// (w, h) of an aspect string.
pub fn ratio(aspect: &str) -> (u32, u32) {
    aspect
        .split_once(':')
        .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))
        .filter(|&(a, b): &(u32, u32)| a > 0 && b > 0)
        .unwrap_or((1, 1))
}

/// Pixel size for an aspect: about one megapixel, rounded to `step`, each side in `min..=max`.
pub fn pixel_size(aspect: &str, step: u32, min: u32, max: u32) -> (u32, u32) {
    let (a, b) = ratio(aspect);
    let r = a as f64 / b as f64;
    let area = 1024.0 * 1024.0;
    let w = (area * r).sqrt();
    let h = w / r;
    let fit = |v: f64| -> u32 {
        let s = step.max(1) as f64;
        let v = ((v / s).round() * s) as u32;
        v.clamp(min, max) / step.max(1) * step.max(1)
    };
    (fit(w), fit(h))
}

/// One generation request (a job makes `count` pictures).
#[derive(Clone, Debug)]
pub struct GenRequest {
    pub model: Model,
    pub prompt: String,
    pub count: u32,
    pub aspect: String,
    /// first picture's seed (the next ones count up from it); None = random
    pub seed: Option<u64>,
    /// reference image files
    pub references: Vec<PathBuf>,
}

/// What a backend needs to reach its service (read on the worker thread).
#[derive(Clone, Debug, Default)]
pub struct Creds {
    pub cloudflare_account: String,
    pub key: Option<String>,
    pub bfl_base: String,
}

/// One picture back from a model.
pub struct Output {
    pub bytes: Vec<u8>,
    pub seed: Option<u64>,
}

/// Generates the request's pictures (blocking). `say` reports progress; `cancel` stops between
/// pictures (and the Codex process).
pub fn generate(
    req: &GenRequest,
    creds: &Creds,
    work: &Path,
    say: &dyn Fn(String),
    cancel: &AtomicBool,
) -> Result<Vec<Output>, String> {
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    match req.model.backend {
        BackendKind::Codex => codex::generate(req, work, say, cancel),
        BackendKind::Cloudflare => cloudflare::generate(req, creds, work, say, cancel),
        BackendKind::Bfl => bfl::generate(req, creds, work, say, cancel),
    }
}

/// A random seed (fits every backend's integer).
pub fn random_seed() -> u64 {
    fastrand::u64(1..2_147_483_647)
}

/// A reference image made small enough to send: at most `max_side` px, as PNG bytes.
pub fn shrink_reference(path: &Path, max_side: u32) -> Result<Vec<u8>, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let img = if img.width() > max_side || img.height() > max_side {
        img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = std::io::Cursor::new(Vec::new());
    img.to_rgb8()
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Standard base64.
pub fn base64_encode(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

/// Standard (or URL-safe) base64, whitespace ignored; a `data:…;base64,` prefix is dropped.
pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.split_once(";base64,").map_or(s, |(_, d)| d);
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return Err("the image data isn't base64".into()),
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// A value inside double quotes in a curl config file.
pub fn cfg_quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A private temp file (0600) under `dir`, removed when dropped.
pub struct TempFile(pub PathBuf);

impl TempFile {
    pub fn new(dir: &Path, name: &str, bytes: &[u8]) -> Result<Self, String> {
        let p = dir.join(format!("{name}-{:x}", fastrand::u64(..)));
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&p)
                .map_err(|e| e.to_string())?;
            f.write_all(bytes).map_err(|e| e.to_string())?;
        }
        Ok(TempFile(p))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A finished HTTP exchange.
pub struct Http {
    pub status: u32,
    pub body: Vec<u8>,
}

impl Http {
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
    /// The body as text, shortened (for errors; never contains the key).
    pub fn snippet(&self) -> String {
        String::from_utf8_lossy(&self.body).trim().chars().take(200).collect()
    }
}

/// Runs curl with `config` (a curl config: url, headers with the key, data / form lines) given
/// on stdin; the response body goes to a private temp file in `work`.
pub fn curl(config: &str, work: &Path, max_secs: u32) -> Result<Http, String> {
    let out_file = work.join(format!("resp-{:x}", fastrand::u64(..)));
    let config = format!(
        "{config}\noutput = {}\nwrite-out = \"%{{http_code}}\"\n",
        cfg_quote(&out_file.to_string_lossy())
    );
    let curl = if Path::new("/usr/bin/curl").is_file() { "/usr/bin/curl" } else { "curl" };
    let mut c = Command::new(curl)
        .args(["-sS", "--max-time", &max_secs.to_string(), "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    if let Some(mut si) = c.stdin.take() {
        let _ = si.write_all(config.as_bytes());
    }
    let out = c.wait_with_output().map_err(|e| e.to_string());
    let body = std::fs::read(&out_file).unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);
    let out = out?;
    if !out.status.success() {
        return Err(format!(
            "network error: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let status = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    Ok(Http { status, body })
}

/// A plain-words reason for an HTTP failure.
pub fn http_error(service: &str, h: &Http) -> String {
    let detail = h
        .json()
        .and_then(|v| {
            // Cloudflare: errors[].message; BFL: detail (string or list of {msg})
            v["errors"][0]["message"]
                .as_str()
                .or(v["detail"].as_str())
                .or(v["detail"][0]["msg"].as_str())
                .or(v["error"]["message"].as_str())
                .or(v["error"].as_str())
                .or(v["message"].as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| h.snippet());
    let why = match h.status {
        401 | 403 => "the key was refused (check it, and its permissions)",
        402 => "the account is out of credits",
        404 => "that model isn't there",
        413 => "the request is too big (smaller reference images?)",
        422 | 400 => "the request was refused",
        429 => "too many requests at once; try again in a minute",
        500..=599 => "the service had a problem",
        _ => "the request failed",
    };
    if detail.is_empty() {
        format!("{service}: {why} (HTTP {})", h.status)
    } else {
        format!("{service}: {why} (HTTP {}): {detail}", h.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_sizes() {
        assert_eq!(
            parse_key("cloudflare:@cf/black-forest-labs/flux-2-dev"),
            Some((BackendKind::Cloudflare, "@cf/black-forest-labs/flux-2-dev"))
        );
        assert_eq!(parse_key("nope:x"), None);
        assert_eq!(parse_key("bfl:"), None);
        assert!(valid_model_id("@cf/black-forest-labs/flux-3-pro"));
        assert!(!valid_model_id("flux 2"));
        assert!(!valid_model_id("../../etc"));
        assert_eq!(pixel_size("1:1", 16, 256, 1920), (1024, 1024));
        let (w, h) = pixel_size("16:9", 16, 256, 1920);
        assert!(w % 16 == 0 && h % 16 == 0 && w > h);
        assert_eq!(ratio("bad"), (1, 1));
        let m = Model::custom(BackendKind::Bfl, "flux-kontext-pro");
        assert_eq!(m.max_refs, 1);
        let m = Model::custom(BackendKind::Cloudflare, "@cf/black-forest-labs/flux-3-dev");
        assert_eq!(m.max_refs, 4);
    }

    #[test]
    fn base64_round_trip() {
        for s in [&b""[..], b"f", b"fo", b"foo", b"\x89PNG\r\n\x1a\n\0\xff"] {
            assert_eq!(base64_decode(&base64_encode(s)).unwrap(), s);
        }
        assert_eq!(base64_decode("data:image/jpeg;base64,Zm9v").unwrap(), b"foo");
        assert!(base64_decode("@@@").is_err());
    }

    #[test]
    fn config_quoting() {
        assert_eq!(cfg_quote("a \"b\"\\\nc"), "\"a \\\"b\\\"\\\\\\nc\"");
    }
}
