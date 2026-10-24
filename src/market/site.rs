//! skyfoldstudio.com in the editor: the community listings (public, no sign-in), free downloads,
//! and buying paid ones with the account's access token through the website's own checkout.
//!
//! - Browsing: `GET /api/listings?q=&cursor=` (published listings; `priceCents`, `coverUrl`,
//!   `creator`…). Covers come from their `coverUrl`.
//! - Free: `GET /api/listings/:id/files/download` when the website serves it to anyone, else a
//!   signed-in account acquires it (quote → purchase, settled at once) and downloads it.
//! - Paid (signed in): the website's MCP endpoint (`POST /mcp`, bearer token): `get_asset_quote`
//!   → `purchase_asset` (mode `checkout`) → the Checkout page opens in the browser → the order is
//!   polled (`get_purchase`) until paid → `get_download` → the file. Payment happens only on the
//!   website; the editor never sees card details.
//! - Downloads are imported like File › Import (a pack's ZIP is unpacked first) and show under
//!   Objects › Imported.
//!
//! The token is an API key of the account (Account › API keys on the website; the default
//! permissions, read + acquire, plus "Prepare paid purchases" (`prepare`) for paid ones), kept
//! in the prefs (`market_token`). No MCP tool of
//! this editor can buy or read the token.
//!
//! The website serves a listing's file only to an account that has acquired it, so a free
//! community listing also needs a sign-in for now (the editor's own free packs never do).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

use super::backend;

pub const DEFAULT_SITE: &str = "https://skyfoldstudio.com";

/// An MCP error that means "accept the marketplace terms first", followed by where.
const TERMS: &str = "terms:";

/// Where the account's API keys are made (Account › API keys).
pub fn keys_url() -> String {
    format!("{}/account.html?view=api", base())
}

/// The website's address (`SKYFOLD_SITE_URL`, prefs `site_url`, or skyfoldstudio.com).
pub fn base() -> String {
    std::env::var("SKYFOLD_SITE_URL")
        .ok()
        .or_else(|| crate::prefs::get("site_url").and_then(|v| v.as_str().map(str::to_string)))
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| s.starts_with("http"))
        .unwrap_or_else(|| DEFAULT_SITE.to_string())
}

/// The account's access token (prefs `market_token`, or `SKYFOLD_MARKET_TOKEN`).
pub fn token() -> Option<String> {
    backend::Config::load().token
}

pub fn set_token(t: Option<&str>) {
    crate::prefs::set(
        "market_token",
        t.map(|t| json!(t.trim())).unwrap_or(Value::Null),
    );
}

/// A published community listing.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// "asset" (one model) or "pack" (a ZIP of several)
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub price_cents: u64,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub cover_url: Option<String>,
    #[serde(default)]
    pub creator: Option<Creator>,
    #[serde(default)]
    pub rating_average: f32,
    #[serde(default)]
    pub rating_count: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Creator {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub username: String,
    /// one of the studio's own accounts (an Official badge on its listings)
    #[serde(default)]
    pub official: bool,
}

impl Listing {
    pub fn is_free(&self) -> bool {
        self.price_cents == 0
    }
    pub fn price_label(&self) -> String {
        if self.is_free() {
            return "Free".into();
        }
        super::catalog::Price {
            amount_minor: self.price_cents,
            currency: if self.currency.is_empty() {
                "USD".into()
            } else {
                self.currency.clone()
            },
        }
        .label()
    }
    pub fn by(&self) -> String {
        self.creator
            .as_ref()
            .map(|c| {
                if c.display_name.is_empty() {
                    c.username.clone()
                } else {
                    c.display_name.clone()
                }
            })
            .unwrap_or_default()
    }
    /// From one of the studio's own accounts.
    pub fn official(&self) -> bool {
        self.creator.as_ref().is_some_and(|c| c.official)
    }
    pub fn page_url(&self, base: &str) -> String {
        format!("{base}/listing/{}", enc(&self.id))
    }
}

/// One page of listings: `{listings, nextCursor}`.
pub fn parse_listings(body: &[u8]) -> Result<(Vec<Listing>, Option<String>), String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Page {
        #[serde(default)]
        listings: Vec<Listing>,
        #[serde(default)]
        next_cursor: Option<String>,
    }
    let p: Page = serde_json::from_slice(body).map_err(|e| format!("unreadable listings: {e}"))?;
    Ok((p.listings, p.next_cursor))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The published listings matching `q` (a page; `cursor` continues).
pub fn listings(
    base: &str,
    q: &str,
    cursor: Option<&str>,
) -> Result<(Vec<Listing>, Option<String>), String> {
    let mut url = format!("{base}/api/listings?limit=48");
    if !q.trim().is_empty() {
        url.push_str(&format!("&q={}", enc(q.trim())));
    }
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", enc(c)));
    }
    let (code, body) = backend::get(&url, None)?;
    if !(200..300).contains(&code) {
        return Err(format!("skyfoldstudio.com answered {code}"));
    }
    parse_listings(&body)
}

/// A call to the website's MCP endpoint: the tool's result (its structured content, or its first
/// text block read as JSON).
pub fn mcp(base: &str, token: &str, tool: &str, args: Value) -> Result<Value, String> {
    rpc(
        base,
        token,
        "tools/call",
        json!({ "name": tool, "arguments": args }),
    )
}

/// One JSON-RPC request to the website's MCP endpoint.
fn rpc(base: &str, token: &str, method: &str, params: Value) -> Result<Value, String> {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let (code, body) = backend::post_json(&format!("{base}/mcp"), Some(token), &req.to_string())?;
    if code == 401 || code == 403 {
        return Err("skyfoldstudio.com didn't accept that API key (expired or revoked?): sign in again with a new one".into());
    }
    if !(200..300).contains(&code) {
        return Err(format!("skyfoldstudio.com answered {code}"));
    }
    if method != "tools/call" {
        return Ok(Value::Null);
    }
    parse_mcp(&body)
}

/// A JSON-RPC answer, plain or as server-sent events (`data: {...}` lines).
pub fn parse_mcp(body: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(body);
    let msg: Value = match serde_json::from_str::<Value>(text.trim()) {
        Ok(v) => v,
        Err(_) => text
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .find(|v| v.get("result").is_some() || v.get("error").is_some())
            .ok_or("an unreadable answer from skyfoldstudio.com")?,
    };
    if let Some(e) = msg.get("error") {
        // the buyer hasn't accepted the marketplace terms yet: where to accept them
        if let Some(u) = e["data"]["acceptanceUrl"].as_str() {
            return Err(format!("{TERMS}{u}"));
        }
        // a key without "Prepare paid purchases" can't buy
        if e["code"].as_i64() == Some(-32003) {
            return Err("this API key can't buy paid listings: make a new one with Prepare paid purchases ticked (Account › API keys) and sign in with it".into());
        }
        return Err(e["message"]
            .as_str()
            .unwrap_or("the website refused")
            .to_string());
    }
    let r = &msg["result"];
    if r["isError"].as_bool() == Some(true) {
        let t = r["content"][0]["text"]
            .as_str()
            .unwrap_or("the website refused");
        return Err(t.to_string());
    }
    if let Some(s) = r.get("structuredContent").filter(|v| !v.is_null()) {
        return Ok(s.clone());
    }
    r["content"][0]["text"]
        .as_str()
        .and_then(|t| serde_json::from_str(t).ok())
        .ok_or_else(|| "an unreadable tool result from skyfoldstudio.com".to_string())
}

/// What a background job reports to the editor.
#[derive(Debug)]
pub enum Event {
    Listings {
        listings: Vec<Listing>,
        next: Option<String>,
        append: bool,
    },
    Cover {
        id: String,
        rgba: Vec<u8>,
        size: [usize; 2],
    },
    Failed(String),
    Note(String),
    /// files to import (a listing's model, or a pack's unpacked models)
    Downloaded {
        title: String,
        files: Vec<PathBuf>,
    },
    /// a scene listing, now in Objects › Scenes (its id)
    SceneAdded {
        title: String,
        id: String,
    },
    /// the Checkout page is open: the purchase finishes there
    Checkout {
        title: String,
    },
    /// a catalog pack is now owned (paid, or found among the account's purchases)
    Owned {
        pack: String,
    },
    /// a bought pack's library is installed (`super::install`): its models go in the library
    PackInstalled {
        pack: String,
        title: String,
    },
    /// the API key was checked: Ok (saved) or why it was turned down
    SignedIn(Result<(), String>),
    /// a job ended (a listing's id when it was a download)
    Done(Option<String>),
}

/// The website's state in the editor (inside `MarketUi`).
#[derive(Default)]
pub struct SiteUi {
    pub listings: Vec<Listing>,
    pub next: Option<String>,
    pub loaded: bool,
    pub busy: usize,
    pub error: Option<String>,
    pub query: String,
    pub covers: std::collections::HashMap<String, bevy_egui::egui::TextureHandle>,
    pub cover_pending: std::collections::HashSet<String>,
    pub token_input: String,
    /// the sign-in card is open
    pub signing_in: bool,
    /// an API key is saved (read from the prefs once, then kept up to date here)
    pub signed_in: Option<bool>,
    /// listings being bought or downloaded (their ids)
    pub working: std::collections::HashSet<String>,
    pub inbox: Arc<Mutex<Vec<Event>>>,
}

impl SiteUi {
    fn send(inbox: &Arc<Mutex<Vec<Event>>>, e: Event) {
        inbox.lock().unwrap_or_else(|p| p.into_inner()).push(e);
    }

    /// Loads the first page (or the next one).
    pub fn load(&mut self, more: bool) {
        let inbox = self.inbox.clone();
        let (q, cursor) = (
            self.query.clone(),
            if more { self.next.clone() } else { None },
        );
        self.busy += 1;
        self.error = None;
        std::thread::spawn(move || {
            let base = base();
            match listings(&base, &q, cursor.as_deref()) {
                Ok((l, next)) => Self::send(
                    &inbox,
                    Event::Listings {
                        listings: l,
                        next,
                        append: more,
                    },
                ),
                Err(e) => Self::send(&inbox, Event::Failed(e)),
            }
            Self::send(&inbox, Event::Done(None));
        });
    }

    pub fn is_signed_in(&mut self) -> bool {
        *self.signed_in.get_or_insert_with(|| token().is_some())
    }

    /// Checks a pasted API key with the website (it lists its tools for a good key) and saves it.
    pub fn sign_in(&mut self, key: String) {
        let inbox = self.inbox.clone();
        self.busy += 1;
        std::thread::spawn(move || {
            let key = key.trim().to_string();
            let r = rpc(&base(), &key, "tools/list", json!({})).map(|_| set_token(Some(&key)));
            Self::send(&inbox, Event::SignedIn(r));
            Self::send(&inbox, Event::Done(None));
        });
    }

    pub fn sign_out(&mut self) {
        set_token(None);
        self.signed_in = Some(false);
    }

    /// Fetches and decodes a listing's cover (once).
    pub fn cover(&mut self, l: &Listing) {
        let Some(url) = l.cover_url.clone() else {
            return;
        };
        if !self.cover_pending.insert(l.id.clone()) {
            return;
        }
        let (inbox, id) = (self.inbox.clone(), l.id.clone());
        std::thread::spawn(move || {
            let full = if url.starts_with("http") {
                url
            } else {
                format!("{}{url}", base())
            };
            if let Ok((200, bytes)) = backend::get(&full, None)
                && let Ok(img) = image::load_from_memory(&bytes)
            {
                let img = img.thumbnail(320, 200).to_rgba8();
                let size = [img.width() as usize, img.height() as usize];
                Self::send(
                    &inbox,
                    Event::Cover {
                        id,
                        rgba: img.into_raw(),
                        size,
                    },
                );
            }
        });
    }

    /// Gets a listing: a free one downloads straight away; a paid one goes through the website's
    /// Checkout (signed in), then downloads when it's paid.
    pub fn get(&mut self, l: &Listing) {
        if !self.working.insert(l.id.clone()) {
            return;
        }
        let (inbox, l) = (self.inbox.clone(), l.clone());
        let token = token();
        self.busy += 1;
        std::thread::spawn(move || {
            let base = base();
            let r = if l.is_free() {
                get_free(&base, token.as_deref(), &l, &inbox)
            } else {
                buy(&base, token.as_deref(), &l, &inbox)
            };
            match r {
                Ok(Saved::Models(files)) => Self::send(
                    &inbox,
                    Event::Downloaded {
                        title: l.title.clone(),
                        files,
                    },
                ),
                Ok(Saved::Scene(id)) => Self::send(
                    &inbox,
                    Event::SceneAdded {
                        title: l.title.clone(),
                        id,
                    },
                ),
                Err(e) => Self::send(&inbox, Event::Failed(format!("{}: {e}", l.title))),
            }
            Self::send(&inbox, Event::Done(Some(l.id.clone())));
        });
    }
}

impl SiteUi {
    /// "Get pack" for a catalog pack sold as a website listing: Checkout in the browser, then,
    /// once it's paid, owned for good and installed.
    pub fn buy_pack(&mut self, pack: &super::Pack) {
        let Some(listing) = pack.listing_id.clone() else {
            return;
        };
        if !self.working.insert(pack.id.clone()) {
            return;
        }
        let (inbox, id, name) = (self.inbox.clone(), pack.id.clone(), pack.name.clone());
        let token = token();
        self.busy += 1;
        std::thread::spawn(move || {
            let base = base();
            let r = (|| {
                let token = token.ok_or("sign in with your Skyfold account to buy this pack")?;
                let l = Listing {
                    id: listing.clone(),
                    title: name.clone(),
                    ..Default::default()
                };
                // bought before (another computer, a reinstall)? never charge twice
                let pack = super::snapshot().pack(&id).cloned();
                if let Some(p) = &pack
                    && let Ok(mut found) = restore_orders(&base, &token, std::slice::from_ref(p), &inbox)
                    && let Some(order) = found.remove(&id)
                {
                    return install_order(&base, &token, &id, &name, &order, &inbox);
                }
                let order = acquire(&base, &token, &l, &inbox)?;
                // paid: it's theirs now, whatever happens to the download
                own(&id, &listing, &order, &inbox);
                install_order(&base, &token, &id, &name, &order, &inbox)
            })();
            if let Err(e) = r {
                Self::send(&inbox, Event::Failed(format!("{name}: {e}")));
            }
            Self::send(&inbox, Event::Done(Some(id)));
        });
    }

    /// Downloads and installs an owned pack again (a new computer, or files removed).
    pub fn reinstall(&mut self, pack: &super::Pack) {
        if !self.working.insert(pack.id.clone()) {
            return;
        }
        let (inbox, p) = (self.inbox.clone(), pack.clone());
        let token = token();
        self.busy += 1;
        std::thread::spawn(move || {
            let base = base();
            let r = (|| {
                let token = token.ok_or("sign in with the Skyfold account that bought it")?;
                let order = match purchases().get(&p.id).and_then(|r| r["order"].as_str()) {
                    Some(o) => o.to_string(),
                    None => restore_orders(&base, &token, std::slice::from_ref(&p), &inbox)?
                        .remove(&p.id)
                        .ok_or("this account hasn't bought or created it (check you're signed in with the account that did)")?,
                };
                install_order(&base, &token, &p.id, &p.name, &order, &inbox)
            })();
            if let Err(e) = r {
                Self::send(&inbox, Event::Failed(format!("{}: {e}", p.name)));
            }
            Self::send(&inbox, Event::Done(Some(p.id.clone())));
        });
    }

    /// Restore purchases: every catalog pack this account bought becomes owned here, and the
    /// ones not installed yet are downloaded.
    pub fn restore(&mut self, packs: Vec<super::Pack>) {
        let inbox = self.inbox.clone();
        let token = token();
        self.busy += 1;
        std::thread::spawn(move || {
            let base = base();
            let r = (|| {
                let token = token.ok_or("sign in with your Skyfold account first")?;
                let found = restore_orders(&base, &token, &packs, &inbox)?;
                if found.is_empty() {
                    Self::send(
                        &inbox,
                        Event::Note("No asset packs bought or created with this account yet".into()),
                    );
                }
                for p in packs.iter().filter(|p| found.contains_key(&p.id)) {
                    if p.installed == 0
                        && let Err(e) = install_order(&base, &token, &p.id, &p.name, &found[&p.id], &inbox)
                    {
                        Self::send(&inbox, Event::Failed(format!("{}: {e}", p.name)));
                    }
                }
                Ok::<(), String>(())
            })();
            if let Err(e) = r {
                Self::send(&inbox, Event::Failed(e));
            }
            Self::send(&inbox, Event::Done(None));
        });
    }
}

/// The packs bought on this computer (prefs `market_purchases`): pack id → its listing and
/// order, so it can be downloaded again.
pub fn purchases() -> serde_json::Map<String, Value> {
    crate::prefs::get("market_purchases")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// A pack is paid for (or this account created it): owned for good on this computer, and its
/// order kept for downloads.
fn own(pack: &str, listing: &str, order: &str, inbox: &Arc<Mutex<Vec<Event>>>) {
    backend::record_owned(pack);
    let mut all = purchases();
    all.insert(
        pack.to_string(),
        json!({ "listing": listing, "order": order, "at": backend::now() }),
    );
    crate::prefs::set("market_purchases", Value::Object(all));
    SiteUi::send(inbox, Event::Owned { pack: pack.to_string() });
}

/// The account's completed orders for these catalog packs (by their listings): each found pack
/// is recorded as owned. Pack id → order id.
fn restore_orders(
    base: &str,
    token: &str,
    packs: &[super::Pack],
    inbox: &Arc<Mutex<Vec<Event>>>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let r = mcp(base, token, "list_purchases", json!({})).map_err(|e| {
        if e.contains("Unknown tool") {
            "skyfoldstudio.com can't list purchases yet: download it from your purchases on skyfoldstudio.com".to_string()
        } else {
            e
        }
    })?;
    let mut found = std::collections::HashMap::new();
    for o in r["purchases"].as_array().into_iter().flatten() {
        let status = o["status"].as_str().unwrap_or("");
        let (Some(asset), Some(order)) = (o["assetId"].as_str(), o["id"].as_str()) else {
            continue;
        };
        if status != "fulfilled" {
            continue;
        }
        let listing = asset.strip_prefix("listing:").unwrap_or(asset);
        for p in packs {
            if p.listing_id.as_deref() == Some(listing) && !found.contains_key(&p.id) {
                own(&p.id, listing, order, inbox);
                found.insert(p.id.clone(), order.to_string());
            }
        }
    }
    // a creator owns the packs they list (drafts too) without buying them; an older website
    // without the tool just has none
    let created = match mcp(base, token, "list_my_listings", json!({})) {
        Ok(r) => r,
        Err(e) if e.contains("Unknown tool") => Value::Null,
        Err(e) => return Err(e),
    };
    for l in created["listings"].as_array().into_iter().flatten() {
        let Some(listing) = l["id"].as_str() else {
            continue;
        };
        if !l["downloadAvailable"].as_bool().unwrap_or(false) {
            continue;
        }
        for p in packs {
            if p.listing_id.as_deref() == Some(listing) && !found.contains_key(&p.id) {
                let order = format!("{CREATOR}{listing}");
                own(&p.id, listing, &order, inbox);
                found.insert(p.id.clone(), order);
            }
        }
    }
    Ok(found)
}

/// The "order" of a pack this account created: its listing, downloaded from the creator's own
/// files (`/api/me/listings/<id>/download`) instead of a purchase.
const CREATOR: &str = "creator:";

/// A created pack's download path (None: a real order, asked of `get_download`).
fn creator_download(order: &str) -> Option<String> {
    order
        .strip_prefix(CREATOR)
        .map(|listing| format!("/api/me/listings/{}/download", enc(listing)))
}

/// Downloads an order's files and installs them as `pack`.
fn install_order(
    base: &str,
    token: &str,
    pack: &str,
    name: &str,
    order: &str,
    inbox: &Arc<Mutex<Vec<Event>>>,
) -> Result<(), String> {
    SiteUi::send(
        inbox,
        Event::Note(format!("Downloading {name}… it installs by itself")),
    );
    let bytes = order_bytes(base, token, order)?;
    super::install::install(&bytes, pack)?;
    SiteUi::send(
        inbox,
        Event::PackInstalled {
            pack: pack.to_string(),
            title: name.to_string(),
        },
    );
    Ok(())
}

/// A free listing: its file straight from the website, or acquired with the account first.
fn get_free(
    base: &str,
    token: Option<&str>,
    l: &Listing,
    inbox: &Arc<Mutex<Vec<Event>>>,
) -> Result<Saved, String> {
    let (code, body) = backend::download(
        &format!("{base}/api/listings/{}/files/download", l.id),
        None,
    )?;
    if (200..300).contains(&code) && !body.is_empty() {
        return save(l, &body);
    }
    // the website wants it acquired (free) by an account
    let token = token.ok_or("sign in with your Skyfold account to add this free asset")?;
    SiteUi::send(
        inbox,
        Event::Note(format!("Adding {} to your Skyfold library…", l.title)),
    );
    let order = acquire(base, token, l, inbox)?;
    fetch_order(base, token, l, &order)
}

/// A paid listing: Checkout in the browser, then the download once it's paid.
fn buy(
    base: &str,
    token: Option<&str>,
    l: &Listing,
    inbox: &Arc<Mutex<Vec<Event>>>,
) -> Result<Saved, String> {
    let token = token.ok_or("sign in with your Skyfold account to buy this")?;
    let order = acquire(base, token, l, inbox)?;
    fetch_order(base, token, l, &order)
}

/// Quote → purchase; for a paid one, opens Checkout and waits (up to 20 minutes) until it's
/// paid. Returns the order id.
fn acquire(
    base: &str,
    token: &str,
    l: &Listing,
    inbox: &Arc<Mutex<Vec<Event>>>,
) -> Result<String, String> {
    let asset = format!("listing:{}", l.id);
    let q = mcp(base, token, "get_asset_quote", json!({ "assetId": asset }))?;
    let quote = q["quote"]["id"]
        .as_str()
        .or_else(|| q["quoteId"].as_str())
        .or_else(|| q["id"].as_str())
        .ok_or("the website didn't quote it")?
        .to_string();
    let key = format!("skyfold-studio-{}-{}", l.id, fastrand::u64(..));
    let p = match mcp(
        base,
        token,
        "purchase_asset",
        json!({ "quoteId": quote, "idempotencyKey": key, "mode": "checkout" }),
    ) {
        Err(e) if e.starts_with(TERMS) => {
            let url = &e[TERMS.len()..];
            super::ui::open_url(url);
            return Err("accept the marketplace terms on skyfoldstudio.com (open in your browser), then try again".into());
        }
        r => r?,
    };
    let purchase = if p.get("purchase").is_some() {
        &p["purchase"]
    } else {
        &p
    };
    let order = purchase["id"]
        .as_str()
        .or_else(|| purchase["orderId"].as_str())
        .ok_or("the website didn't start the purchase")?
        .to_string();
    let status = |v: &Value| v["status"].as_str().unwrap_or("").to_string();
    if status(purchase) == "fulfilled" {
        return Ok(order);
    }
    let checkout = purchase["checkoutUrl"]
        .as_str()
        .or_else(|| p["checkoutUrl"].as_str())
        .ok_or("the website didn't open a checkout")?;
    if !super::ui::open_url(checkout) {
        return Err(format!("couldn't open the checkout page: {checkout}"));
    }
    SiteUi::send(
        inbox,
        Event::Checkout {
            title: l.title.clone(),
        },
    );
    // the order settles when the website hears from the payment provider
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs() < 20 * 60 {
        std::thread::sleep(std::time::Duration::from_secs(4));
        let o = mcp(base, token, "get_purchase", json!({ "orderId": order }))?;
        let o = if o.get("purchase").is_some() {
            o["purchase"].clone()
        } else {
            o
        };
        match status(&o).as_str() {
            "fulfilled" => return Ok(order),
            "failed" | "expired" | "canceled" | "cancelled" => {
                return Err("the payment didn't go through".into());
            }
            _ => {}
        }
    }
    Err(
        "the payment wasn't finished in time (it will show in your Skyfold library when it is)"
            .into(),
    )
}

/// The bought (or acquired) file.
fn fetch_order(base: &str, token: &str, l: &Listing, order: &str) -> Result<Saved, String> {
    save(l, &order_bytes(base, token, order)?)
}

/// An order's download.
fn order_bytes(base: &str, token: &str, order: &str) -> Result<Vec<u8>, String> {
    let url = match creator_download(order) {
        Some(path) => path,
        None => {
            let d = mcp(base, token, "get_download", json!({ "orderId": order }))?;
            d["downloadUrl"]
                .as_str()
                .ok_or("the website gave no download")?
                .to_string()
        }
    };
    let full = if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{base}{url}")
    };
    let (code, body) = backend::download(&full, Some(token))?;
    if !(200..300).contains(&code) {
        return Err(format!("the download answered {code}"));
    }
    Ok(body)
}

/// What a download turned out to be.
#[derive(Debug, PartialEq)]
pub enum Saved {
    /// models to import
    Models(Vec<PathBuf>),
    /// a Skyfold scene, added to the scenes library (its id)
    Scene(String),
}

/// Writes a download to a temporary folder (a ZIP unpacked) and returns its models, or adds it
/// to the scenes when it's a scene (a `scene.json` inside).
fn save(l: &Listing, bytes: &[u8]) -> Result<Saved, String> {
    let dir = std::env::temp_dir().join("skyfold-market").join(&l.id);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = l
        .title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_matches('_').to_string();
    if bytes.starts_with(b"glTF") {
        let f = dir.join(format!(
            "{}.glb",
            if safe.is_empty() { "model" } else { &safe }
        ));
        std::fs::write(&f, bytes).map_err(|e| e.to_string())?;
        return Ok(Saved::Models(vec![f]));
    }
    if bytes.starts_with(b"PK") {
        let zip = dir.join("pack.zip");
        std::fs::write(&zip, bytes).map_err(|e| e.to_string())?;
        let out = dir.join("pack");
        let ok = std::process::Command::new("/usr/bin/unzip")
            .args(["-q", "-o"])
            .arg(&zip)
            .arg("-d")
            .arg(&out)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return Err("couldn't unpack the pack".into());
        }
        if out.join("scene.json").is_file()
            || std::fs::read_dir(&out)
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| e.path().join("scene.json").is_file())
        {
            return crate::scenes::file::import_folder(&out).map(Saved::Scene);
        }
        let mut found = vec![];
        let mut stack = vec![out];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| {
                    x.eq_ignore_ascii_case("glb") || x.eq_ignore_ascii_case("gltf")
                }) {
                    found.push(p);
                }
            }
        }
        found.sort();
        if found.is_empty() {
            return Err("the pack has no glTF / GLB models".into());
        }
        return Ok(Saved::Models(found));
    }
    Err("the download isn't a model (GLB) or a pack (ZIP)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_packs_download_from_the_creators_own_files() {
        assert_eq!(
            creator_download("creator:48b999a0-a171-4d02-b904-236f83d3eb05").as_deref(),
            Some("/api/me/listings/48b999a0-a171-4d02-b904-236f83d3eb05/download")
        );
        assert_eq!(creator_download("creator:../x").as_deref(), Some("/api/me/listings/..%2Fx/download"));
        assert_eq!(creator_download("6f1c0000-0000-4000-8000-000000000001"), None);
    }

    #[test]
    fn reads_the_websites_listings_and_tool_answers() {
        let page = br#"{"listings":[{"id":"a1","ownerId":"u","title":"Old Well","description":"d","kind":"asset","category":"props","license":"CC0","priceCents":499,"currency":"USD","coverUrl":"/api/media/b1","creator":{"id":"u","username":"kim","displayName":"Kim"},"ratingAverage":4.5,"ratingCount":2},{"id":"a2","title":"Free Rocks","kind":"pack","creator":{"id":"s","username":"skyfold","displayName":"Skyfold Studio","official":true}}],"nextCursor":"c2"}"#;
        let (l, next) = parse_listings(page).unwrap();
        assert_eq!((l.len(), next.as_deref()), (2, Some("c2")));
        // the studio's own listings carry the Official badge
        assert!(!l[0].official() && l[1].official() && l[1].by() == "Skyfold Studio");
        assert_eq!(
            (
                l[0].price_label().as_str(),
                l[0].by().as_str(),
                l[1].is_free()
            ),
            ("$4.99", "Kim", true)
        );
        // a JSON-RPC answer, plain and as server-sent events
        let plain = br#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"{\"quote\":{\"id\":\"q1\"}}"}]}}"#;
        assert_eq!(parse_mcp(plain).unwrap()["quote"]["id"], "q1");
        let sse = b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"structuredContent\":{\"orderId\":\"o1\"}}}\n\n";
        assert_eq!(parse_mcp(sse).unwrap()["orderId"], "o1");
        let err =
            br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32004,"message":"Download unavailable"}}"#;
        assert_eq!(parse_mcp(err).unwrap_err(), "Download unavailable");
        assert_eq!(enc("hand painted & co"), "hand%20painted%20%26%20co");
        // a terms refusal says where to accept them
        let terms = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32010,"message":"terms_acceptance_required","data":{"acceptanceUrl":"https://x/account#marketplace-terms"}}}"#;
        assert_eq!(
            parse_mcp(terms).unwrap_err(),
            format!("{TERMS}https://x/account#marketplace-terms")
        );
    }

    #[test]
    fn downloads_become_model_files() {
        let l = Listing {
            id: format!("test-{}", fastrand::u64(..)),
            title: "Old Well!".into(),
            ..Default::default()
        };
        // a GLB is kept as it is, named after the listing
        let Saved::Models(glb) = save(&l, b"glTF\x02\0\0\0rest").unwrap() else {
            panic!()
        };
        assert_eq!(glb[0].file_name().unwrap(), "old_well.glb");
        // a pack's ZIP is unpacked and its models found, however deep
        let dir = std::env::temp_dir().join(format!("{}-src", l.id));
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join("models/a.glb"), b"glTF").unwrap();
        std::fs::write(dir.join("readme.txt"), b"hi").unwrap();
        let zip = dir.join("p.zip");
        assert!(
            std::process::Command::new("/usr/bin/zip")
                .current_dir(&dir)
                .args(["-qr", "p.zip", "models", "readme.txt"])
                .status()
                .unwrap()
                .success()
        );
        let Saved::Models(files) = save(&l, &std::fs::read(&zip).unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("models/a.glb"), "{files:?}");
        assert!(save(&l, b"<html>").is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("skyfold-market").join(&l.id));
    }
}
