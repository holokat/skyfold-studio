//! Where the catalog and this account's entitlements come from: the marketplace backend over
//! HTTP (`SKYFOLD_MARKET_URL` / prefs `market_url`, token in prefs `market_token`; the contract
//! is `docs/marketplace-api.md`) or, with no address set, the catalog bundled with the app and
//! entitlements from the prefs (`market_owned`). Fetches run on a background thread; the last
//! good answer is cached in the prefs (`market_cache`) so the packs work offline.
//!
//! HTTP goes through the system `curl` like the prompt agent's API calls: no TLS stack in the
//! build, and the token travels through curl's stdin, never its command line.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::catalog::{self, Entitlements, Pack};

/// Where the market's data comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// the catalog bundled with the app, entitlements from the prefs
    Bundled,
    /// a marketplace backend at this base address
    Marketplace(String),
}

/// The address and token, from the environment first, then the prefs.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub url: Option<String>,
    pub token: Option<String>,
}

impl Config {
    pub fn load() -> Config {
        let pick = |env: &str, key: &str| {
            std::env::var(env)
                .ok()
                .or_else(|| crate::prefs::get(key).and_then(|v| v.as_str().map(str::to_string)))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        Config {
            url: pick("SKYFOLD_MARKET_URL", "market_url")
                .map(|u| u.trim_end_matches('/').to_string()),
            token: pick("SKYFOLD_MARKET_TOKEN", "market_token"),
        }
    }
}

/// Why the entitlements couldn't be read.
#[derive(Clone, Debug, PartialEq)]
pub enum EntitlementError {
    /// no token, or the backend turned it down (401 / 403): nothing is owned
    SignedOut(String),
    /// the backend couldn't be reached or answered badly: keep what was known
    Unreachable(String),
}

/// A source of packs and entitlements.
pub trait MarketBackend: Send + Sync {
    fn source(&self) -> Source;
    /// Every pack on offer.
    fn catalog(&self) -> Result<Vec<Pack>, String>;
    /// The ids of the packs this account owns.
    fn entitlements(&self) -> Result<Vec<String>, EntitlementError>;
}

/// The backend for a config.
pub fn backend(cfg: &Config) -> Box<dyn MarketBackend> {
    match &cfg.url {
        Some(url) => Box::new(HttpBackend {
            base: url.clone(),
            token: cfg.token.clone(),
        }),
        None => Box::new(LocalBackend),
    }
}

/// No backend: the bundled catalog, and the packs listed in the prefs (`market_owned`).
pub struct LocalBackend;

/// The pack ids the prefs list as owned (`market_owned`).
pub fn local_owned() -> Vec<String> {
    crate::prefs::get("market_owned")
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

/// Records a pack as owned on this computer (prefs `market_owned`): a purchase the website
/// confirmed. Never removed by a refresh, sign-out or update, so a bought pack stays usable.
pub fn record_owned(pack: &str) {
    let mut owned = local_owned();
    if !owned.iter().any(|p| p == pack) {
        owned.push(pack.to_string());
        crate::prefs::set("market_owned", serde_json::json!(owned));
    }
}

impl MarketBackend for LocalBackend {
    fn source(&self) -> Source {
        Source::Bundled
    }
    fn catalog(&self) -> Result<Vec<Pack>, String> {
        Ok(catalog::bundled())
    }
    fn entitlements(&self) -> Result<Vec<String>, EntitlementError> {
        Ok(local_owned())
    }
}

/// The marketplace backend: `GET {base}/v1/catalog` and `GET {base}/v1/entitlements` (bearer
/// token).
pub struct HttpBackend {
    pub base: String,
    pub token: Option<String>,
}

impl MarketBackend for HttpBackend {
    fn source(&self) -> Source {
        Source::Marketplace(self.base.clone())
    }
    fn catalog(&self) -> Result<Vec<Pack>, String> {
        let (code, body) = get(&format!("{}/v1/catalog", self.base), None)?;
        if !(200..300).contains(&code) {
            return Err(format!(
                "the marketplace catalog answered {code}{}",
                reason(&body)
            ));
        }
        catalog::parse(&String::from_utf8_lossy(&body)).map(|c| c.packs)
    }
    fn entitlements(&self) -> Result<Vec<String>, EntitlementError> {
        let Some(token) = &self.token else {
            return Err(EntitlementError::SignedOut(
                "not signed in to the marketplace".into(),
            ));
        };
        let (code, body) = get(&format!("{}/v1/entitlements", self.base), Some(token))
            .map_err(EntitlementError::Unreachable)?;
        match code {
            200..=299 => serde_json::from_slice::<Entitlements>(&body)
                .map(|e| e.owned)
                .map_err(|e| {
                    EntitlementError::Unreachable(format!("unreadable entitlements: {e}"))
                }),
            401 | 403 => Err(EntitlementError::SignedOut(format!(
                "the marketplace turned the sign-in down ({code}{})",
                reason(&body)
            ))),
            _ => Err(EntitlementError::Unreachable(format!(
                "the marketplace entitlements answered {code}{}",
                reason(&body)
            ))),
        }
    }
}

/// `{"error": {"message": …}}` in an error answer, as ": message".
fn reason(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(|m| format!(": {m}")))
        .unwrap_or_default()
}

/// A GET with curl: (status, body). The address and headers go through stdin (`-K -`).
pub(super) fn get(url: &str, token: Option<&str>) -> Result<(u16, Vec<u8>), String> {
    request(url, token, None, 20)
}

/// A JSON POST with curl (the website's MCP endpoint): (status, body).
pub(super) fn post_json(
    url: &str,
    token: Option<&str>,
    body: &str,
) -> Result<(u16, Vec<u8>), String> {
    request(url, token, Some(body), 40)
}

/// A large download (models, packs; a whole pack is over 100 MB): up to half an hour.
pub(super) fn download(url: &str, token: Option<&str>) -> Result<(u16, Vec<u8>), String> {
    request(url, token, None, 1800)
}

fn request(
    url: &str,
    token: Option<&str>,
    body: Option<&str>,
    timeout: u32,
) -> Result<(u16, Vec<u8>), String> {
    let safe = |s: &str| {
        !s.chars()
            .any(|c| c == '"' || c == '\\' || c.is_control() || c == ' ')
    };
    if !(url.starts_with("https://") || url.starts_with("http://")) || !safe(url) {
        return Err(format!(
            "`{url}` isn't a marketplace address (http:// or https://)"
        ));
    }
    if token.is_some_and(|t| !safe(t)) {
        return Err("the marketplace token has characters a token can't have".into());
    }
    let accept = if body.is_some() {
        "application/json, text/event-stream"
    } else {
        "application/json"
    };
    // (max-filesize: no answer may fill memory; the biggest pack download is well under 4 GB)
    let mut config = format!(
        "url = \"{url}\"\nheader = \"Accept: {accept}\"\nheader = \"User-Agent: SkyfoldStudio/{}\"\nlocation\nmax-filesize = 4294967296\n",
        env!("CARGO_PKG_VERSION")
    );
    if let Some(t) = token {
        config.push_str(&format!("header = \"Authorization: Bearer {t}\"\n"));
    }
    if let Some(b) = body {
        // (a quoted config value: backslashes and quotes escaped)
        let esc = b
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ");
        config.push_str(&format!(
            "header = \"Content-Type: application/json\"\ndata-raw = \"{esc}\"\n"
        ));
    }
    let curl = if std::path::Path::new("/usr/bin/curl").is_file() {
        "/usr/bin/curl"
    } else {
        "curl"
    };
    let mut c = Command::new(curl)
        .args([
            "-sS",
            "--max-time",
            &timeout.to_string(),
            "--connect-timeout",
            "8",
            "-K",
            "-",
            "-w",
            "\n%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    if let Some(mut si) = c.stdin.take() {
        let _ = si.write_all(config.as_bytes());
    }
    let out = c.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "can't reach the marketplace: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let split = out
        .stdout
        .iter()
        .rposition(|&b| b == b'\n')
        .ok_or("no answer from the marketplace")?;
    let code = std::str::from_utf8(&out.stdout[split + 1..])
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .ok_or("no answer from the marketplace")?;
    Ok((code, out.stdout[..split].to_vec()))
}

/// The last good answer of a marketplace (prefs `market_cache`): its catalog, and what the
/// account with that token owned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cache {
    pub source: String,
    /// a hash of the token the entitlements were read with (0: none)
    #[serde(default)]
    pub account: u64,
    /// unix seconds
    #[serde(default)]
    pub at: u64,
    pub packs: Vec<Pack>,
    #[serde(default)]
    pub owned: Vec<String>,
}

pub fn account(token: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(t) = token else { return 0 };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish().max(1)
}

/// The cache of the marketplace at `url`; its entitlements only for the same account.
pub fn cache_load(url: &str, token: Option<&str>) -> Option<Cache> {
    let mut c: Cache = serde_json::from_value(crate::prefs::get("market_cache")?).ok()?;
    if c.source != url {
        return None;
    }
    if c.account != account(token) || token.is_none() {
        c.owned.clear();
    }
    Some(c)
}

pub fn cache_save(c: &Cache) {
    if let Ok(v) = serde_json::to_value(c) {
        crate::prefs::set("market_cache", v);
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-shot mock marketplace on localhost (tiny_http): answers `n` requests.
    fn mock(
        n: usize,
    ) -> (
        String,
        std::thread::JoinHandle<Vec<(String, Option<String>)>>,
    ) {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("mock server");
        let port = server.server_addr().to_ip().expect("ip address").port();
        let handle = std::thread::spawn(move || {
            let mut seen = vec![];
            for _ in 0..n {
                let Ok(req) = server.recv() else { break };
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.to_string());
                let (code, body) = match (req.url(), auth.as_deref()) {
                    ("/v1/catalog", _) => (200, r#"{"version":1,"packs":[{"id":"skyfold_studio","name":"Hand-painted Fantasy","style":"handpainted","publisher":"Skyfold Studio","price":{"amount_minor":1299,"currency":"EUR"},"free_assets":["hp:well"],"purchase_url":"https://example.invalid/hp","extra_field":true}]}"#.to_string()),
                    ("/v1/entitlements", Some("Bearer good-token")) => (200, r#"{"owned":["skyfold_studio"]}"#.to_string()),
                    ("/v1/entitlements", _) => (401, r#"{"error":{"code":"unauthorized","message":"token expired"}}"#.to_string()),
                    _ => (404, "{}".to_string()),
                };
                seen.push((req.url().to_string(), auth));
                let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(code));
            }
            seen
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    #[test]
    fn http_backend_reads_the_catalog_and_entitlements() {
        let (base, server) = mock(3);
        let good = HttpBackend {
            base: base.clone(),
            token: Some("good-token".into()),
        };
        let packs = good.catalog().expect("catalog");
        assert_eq!(packs.len(), 1);
        assert_eq!(
            (packs[0].id.as_str(), packs[0].price_label().as_str()),
            ("skyfold_studio", "€12.99")
        );
        assert_eq!(
            packs[0].purchase_url.as_deref(),
            Some("https://example.invalid/hp")
        );
        assert_eq!(good.entitlements(), Ok(vec!["skyfold_studio".to_string()]));
        // a rejected token: signed out, the backend's reason in the message
        let bad = HttpBackend {
            base: base.clone(),
            token: Some("old-token".into()),
        };
        match bad.entitlements() {
            Err(EntitlementError::SignedOut(m)) => {
                assert!(m.contains("401") && m.contains("token expired"), "{m}")
            }
            other => panic!("{other:?}"),
        }
        let seen = server.join().unwrap();
        // the token only goes to the entitlements, as a bearer header
        assert_eq!(seen[0], ("/v1/catalog".to_string(), None));
        assert_eq!(seen[1].1.as_deref(), Some("Bearer good-token"));
        // no token: signed out without asking; nobody listening: unreachable
        let none = HttpBackend { base, token: None };
        assert!(matches!(
            none.entitlements(),
            Err(EntitlementError::SignedOut(_))
        ));
        let gone = HttpBackend {
            base: "http://127.0.0.1:9".into(),
            token: Some("t".into()),
        };
        assert!(gone.catalog().is_err());
        assert!(matches!(
            gone.entitlements(),
            Err(EntitlementError::Unreachable(_))
        ));
        // addresses and tokens can't smuggle curl config
        assert!(get("http://x\"\nurl = \"file:///etc/passwd", None).is_err());
        assert!(get("file:///etc/passwd", None).is_err());
        assert!(get("http://127.0.0.1:9", Some("a\"b")).is_err());
    }
}
