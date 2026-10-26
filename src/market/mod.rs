//! Asset packs and the marketplace.
//!
//! A *style* is a look (photoreal, hand-painted…; `crate::objects::STYLES`); a *pack* is a
//! sellable set of assets in one style from one publisher (`catalog::Pack`). Every bundled model
//! library belongs to a pack (`objects::ModelLibrary::pack_id`, matched by the asset id prefix:
//! `hp:inn` is in the library with prefix `hp`); built-in procedural assets, food props and the
//! user's imported models belong to none and are always usable.
//!
//! The one access rule (`Market::access`): an asset is usable if it's in no pack, its pack is
//! free, owned or developer-unlocked, or it's one of its pack's free samples (the catalog's
//! `free_assets`, 10 per first-party pack). Placing (Objects tab, Gallery, set pieces, ⌘D) and
//! every MCP tool that places or pictures an asset go through it; already placed objects of a
//! locked pack still render, select, move and delete, so worlds open for everyone.
//!
//! The catalog and entitlements come from a backend (`backend`): the marketplace over HTTP,
//! or the bundled catalog. They load at startup (the cached copy first, so it works offline)
//! and refresh in the background every 10 minutes and on Refresh; no frame waits on the network.
//! The state lives in a private snapshot that only this module replaces: entitlements change
//! through the backend, the prefs (`market_owned`) or the developer unlock in the Asset packs
//! window (`SKYFOLD_MARKET_DEV=owner` or prefs `market_dev_unlock`), never through MCP — the only
//! MCP tool here, `market_packs`, reads.

pub mod backend;
pub mod catalog;
pub mod install;
pub mod mcp;
pub mod site;
pub mod ui;
pub mod view;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use bevy::prelude::*;

use backend::{EntitlementError, Source};
pub use catalog::Pack;

/// What this account may do with an asset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access<'a> {
    /// in no pack (built-ins, food, imports) or in a free one
    Free,
    /// its pack is owned (or developer-unlocked)
    Owned,
    /// one of a paid pack's free samples
    Sample(&'a Pack),
    /// in a pack this account hasn't got
    Locked(&'a Pack),
}

impl Access<'_> {
    pub fn usable(self) -> bool {
        !matches!(self, Access::Locked(_))
    }
}

/// A pack for this account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackState {
    Free,
    Owned,
    Locked,
}

/// The developer unlock (every pack usable on this computer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dev {
    Off,
    /// `SKYFOLD_MARKET_DEV=owner`
    Env,
    /// the Asset packs window's switch (prefs `market_dev_unlock`)
    Prefs,
}

/// A bundled model library's pack: its id prefix, pack id, style and model count.
#[derive(Clone, Debug)]
struct LibPack {
    prefix: String,
    pack: String,
    style: String,
    models: usize,
    /// a shipped preview (`objects::PREVIEWS`): its models are listed, not installed
    preview: bool,
}

/// The bundled libraries' packs (read once).
fn lib_packs() -> Arc<Vec<LibPack>> {
    if let Some(l) = LIBS.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return l;
    }
    let l = Arc::new(read_lib_packs());
    *LIBS.write().unwrap_or_else(|e| e.into_inner()) = Some(l.clone());
    l
}

static LIBS: RwLock<Option<Arc<Vec<LibPack>>>> = RwLock::new(None);

fn read_lib_packs() -> Vec<LibPack> {
    {
        #[derive(serde::Deserialize)]
        struct Count {
            #[serde(default)]
            models: Vec<serde::de::IgnoredAny>,
        }
        let dir = std::path::PathBuf::from(crate::assets_path()).join("models");
        crate::objects::model_libraries(&dir)
            .into_iter()
            .map(|l| {
                let models = std::fs::read_to_string(l.dir.join("models.json"))
                    .ok()
                    .and_then(|s| serde_json::from_str::<Count>(&s).ok())
                    .map(|c| c.models.len())
                    .unwrap_or(0);
                LibPack {
                    prefix: l.prefix,
                    pack: l.pack_id,
                    style: l.pack,
                    models,
                    preview: l.preview,
                }
            })
            .collect()
    }
}

/// A pack was just installed (or removed): count the libraries again and rebuild the snapshot.
pub fn reload_installed() {
    *LIBS.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(read_lib_packs()));
    let cur = snapshot();
    install(cur.with_dev(cur.dev));
}

/// The packs and what this account owns: an immutable snapshot (`snapshot()`), replaced whole
/// when something changes.
pub struct Market {
    /// the catalog as the backend sent it
    catalog: Vec<Pack>,
    /// the catalog plus installed packs it doesn't list (unlisted: locked)
    packs: Vec<Pack>,
    by_prefix: HashMap<String, usize>,
    owned: BTreeSet<String>,
    dev: Dev,
    pub source: Source,
    /// signed in to the marketplace (always with the bundled catalog)
    pub signed_in: bool,
    /// the last refresh's problems (shown in Asset packs)
    pub error: Option<String>,
    /// when the backend last answered (unix seconds; 0: not yet)
    pub checked_at: u64,
}

impl Market {
    fn build(
        catalog: Vec<Pack>,
        owned: impl IntoIterator<Item = String>,
        dev: Dev,
        source: Source,
        libs: &[LibPack],
    ) -> Market {
        let mut packs = catalog.clone();
        let mut by_prefix = HashMap::new();
        for l in libs {
            let i = match packs.iter().position(|p| p.id == l.pack) {
                Some(i) => i,
                None => {
                    packs.push(Pack {
                        id: l.pack.clone(),
                        name: crate::objects::pack_label(&l.pack.replace('_', " ")),
                        style: l.style.clone(),
                        publisher: String::new(),
                        description: String::new(),
                        price: None,
                        free_assets: vec![],
                        asset_count: None,
                        listing_id: None,
                        purchase_url: None,
                        thumbnail_url: None,
                        hero_asset: None,
                        unlisted: true,
                        installed: 0,
                        previewed: 0,
                    });
                    packs.len() - 1
                }
            };
            if l.preview {
                packs[i].previewed += l.models;
            } else {
                packs[i].installed += l.models;
            }
            by_prefix.insert(l.prefix.clone(), i);
        }
        Market {
            catalog,
            packs,
            by_prefix,
            owned: owned.into_iter().collect(),
            dev,
            signed_in: true,
            source,
            error: None,
            checked_at: 0,
        }
    }

    /// Every pack: the catalog's, then installed ones it doesn't list.
    pub fn packs(&self) -> &[Pack] {
        &self.packs
    }

    pub fn pack(&self, id: &str) -> Option<&Pack> {
        self.packs.iter().find(|p| p.id == id)
    }

    pub fn pack_index(&self, id: &str) -> Option<usize> {
        self.packs.iter().position(|p| p.id == id)
    }

    /// The pack an asset belongs to (by its id prefix), as an index into `packs`.
    /// A paid asset: in a paid pack and not one of its free samples (owned or not).
    pub fn is_paid(&self, asset: &str) -> bool {
        self.pack_of(asset)
            .is_some_and(|p| !p.is_free() && !p.has_sample(asset))
    }

    pub fn pack_index_of(&self, asset: &str) -> Option<usize> {
        let (prefix, _) = asset.split_once(':')?;
        self.by_prefix.get(prefix).copied()
    }

    pub fn pack_of(&self, asset: &str) -> Option<&Pack> {
        self.pack_index_of(asset).map(|i| &self.packs[i])
    }

    pub fn state(&self, p: &Pack) -> PackState {
        if p.is_free() {
            PackState::Free
        } else if self.dev != Dev::Off || self.owned.contains(&p.id) {
            PackState::Owned
        } else {
            PackState::Locked
        }
    }

    /// The access rule.
    pub fn access(&self, asset: &str) -> Access<'_> {
        let Some(p) = self.pack_of(asset) else {
            return Access::Free;
        };
        match self.state(p) {
            PackState::Free => Access::Free,
            PackState::Owned => Access::Owned,
            PackState::Locked if p.has_sample(asset) => Access::Sample(p),
            PackState::Locked => Access::Locked(p),
        }
    }

    pub fn usable(&self, asset: &str) -> bool {
        self.access(asset).usable()
    }

    /// Ok if usable, else why not (worded for agents, who pass it on to the user).
    pub fn check(&self, asset: &str) -> Result<(), String> {
        match self.access(asset) {
            Access::Locked(p) if p.unlisted => Err(format!(
                "`{asset}` is in the {} pack, which isn't for sale yet: it can't be used on this account",
                p.name
            )),
            Access::Locked(p) => Err(format!(
                "`{asset}` is in the {} pack ({}), which this account hasn't unlocked — ask the user to get it in Asset packs (Objects tab); its free samples are listed by objects_library",
                p.name,
                p.price_label()
            )),
            _ => Ok(()),
        }
    }

    /// A set (set pieces) needs its packs whole, not just their samples: Ok, or which pack it
    /// needs.
    pub fn check_set<'a>(
        &self,
        what: &str,
        assets: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), String> {
        for a in assets {
            if let Some(p) = self.pack_of(a)
                && self.state(p) == PackState::Locked
            {
                return Err(format!(
                    "The {what} needs the {} pack ({}), which this account hasn't unlocked: get it in Objects › Asset packs",
                    p.name,
                    p.price_label()
                ));
            }
        }
        Ok(())
    }

    /// Whether any pack is locked (something to explain).
    pub fn any_locked(&self) -> bool {
        self.packs
            .iter()
            .any(|p| self.state(p) == PackState::Locked)
    }

    pub fn dev(&self) -> Dev {
        self.dev
    }

    /// The styles with packs, photoreal first.
    pub fn styles(&self) -> Vec<String> {
        let mut s: Vec<String> = self.packs.iter().map(|p| p.style.clone()).collect();
        s.sort_by_key(|x| (x != crate::objects::PHOTOREAL, x.clone()));
        s.dedup();
        s
    }

    /// A copy with another developer unlock.
    fn with_dev(&self, dev: Dev) -> Market {
        let mut m = Market::build(
            self.catalog.clone(),
            self.owned.iter().cloned(),
            dev,
            self.source.clone(),
            &lib_packs(),
        );
        m.signed_in = self.signed_in;
        m.error = self.error.clone();
        m.checked_at = self.checked_at;
        m
    }
}

// ------------------------------------------------------------------ the snapshot

static MARKET: LazyLock<RwLock<Arc<Market>>> = LazyLock::new(|| RwLock::new(Arc::new(startup())));

/// The current packs and entitlements (cheap: an `Arc`; take one per frame or per call).
pub fn snapshot() -> Arc<Market> {
    MARKET.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn install(m: Market) {
    *MARKET.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(m);
}

/// Usable on this account?
pub fn usable(asset: &str) -> bool {
    snapshot().usable(asset)
}

/// Ok, or why the asset can't be used (for agents).
pub fn check(asset: &str) -> Result<(), String> {
    snapshot().check(asset)
}

/// Ok, or which pack a set needs.
pub fn check_set<'a>(what: &str, assets: impl Iterator<Item = &'a str>) -> Result<(), String> {
    snapshot().check_set(what, assets)
}

/// Usable? If not, the editor says why (a toast).
pub fn allow(asset: &str) -> bool {
    let m = snapshot();
    match m.access(asset) {
        Access::Locked(p) => {
            notify(format!(
                "That's from the {} pack ({}): get it in Objects › Asset packs to place or copy it",
                p.name,
                p.price_label()
            ));
            false
        }
        _ => true,
    }
}

static NOTICE: Mutex<Option<String>> = Mutex::new(None);

/// A message for the user from code that can't show one (shown as a toast next frame).
fn notify(msg: String) {
    *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg);
}

fn take_notice() -> Option<String> {
    NOTICE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Developer overrides are available only in debug builds and the studio's own `studio_dev`
/// builds, and are always opt-in. Release builds ignore both the environment override and any
/// saved developer preference.
pub const DEV_UNLOCK_DEFAULT: bool = false;

pub(super) const fn developer_unlock_available() -> bool {
    cfg!(debug_assertions) || cfg!(feature = "studio_dev")
}

/// Marks a `studio_dev` binary, so `tools/release/package_macos.py` refuses to ship it.
#[cfg(feature = "studio_dev")]
#[used]
static STUDIO_DEV_MARKER: [u8; 24] = *b"SKYFOLD_STUDIO_DEV_BUILD";

fn dev_setting() -> Dev {
    if !developer_unlock_available() {
        return Dev::Off;
    }
    match std::env::var("SKYFOLD_MARKET_DEV")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("owner") => Dev::Env,
        Some("off") => Dev::Off,
        _ if crate::prefs::get_bool("market_dev_unlock").unwrap_or(DEV_UNLOCK_DEFAULT) => {
            Dev::Prefs
        }
        _ => Dev::Off,
    }
}

/// The first snapshot: the cached catalog of the marketplace (or the bundled one) and the
/// entitlements known so far. Tests get the bundled catalog, nothing owned, no unlock.
fn startup() -> Market {
    let libs = &lib_packs();
    if cfg!(test) {
        return Market::build(catalog::bundled(), [], Dev::Off, Source::Bundled, libs);
    }
    let cfg = backend::Config::load();
    let dev = dev_setting();
    let m = match &cfg.url {
        Some(url) => {
            let cache = backend::cache_load(url, cfg.token.as_deref());
            let at = cache.as_ref().map(|c| c.at).unwrap_or(0);
            let (packs, owned) = cache
                .map(|c| (c.packs, c.owned))
                .unwrap_or_else(|| (catalog::bundled(), vec![]));
            // (packs bought through the website stay owned whatever the marketplace says)
            let owned = owned.into_iter().chain(backend::local_owned());
            let mut m = Market::build(packs, owned, dev, Source::Marketplace(url.clone()), libs);
            m.signed_in = cfg.token.is_some();
            m.checked_at = at;
            m
        }
        None => Market::build(
            catalog::bundled(),
            backend::local_owned(),
            dev,
            Source::Bundled,
            libs,
        ),
    };
    info!(
        "MARKET {} packs ({}), owned {:?}, developer unlock {:?}",
        m.packs.len(),
        source_label(&m.source),
        m.owned,
        m.dev
    );
    m
}

pub fn source_label(s: &Source) -> String {
    match s {
        Source::Bundled => "built-in catalog".into(),
        Source::Marketplace(u) => format!("marketplace {u}"),
    }
}

/// The developer unlock switch (Asset packs window only; never MCP).
fn set_dev_unlock(on: bool) {
    if !developer_unlock_available() {
        return;
    }
    crate::prefs::set("market_dev_unlock", serde_json::json!(on));
    let cur = snapshot();
    let dev = if cur.dev == Dev::Env {
        Dev::Env
    } else if on {
        Dev::Prefs
    } else {
        Dev::Off
    };
    install(cur.with_dev(dev));
    info!("MARKET developer unlock {dev:?}");
}

// ------------------------------------------------------------------ refreshing

/// What a background fetch brought back.
struct Fetched {
    source: Source,
    token: Option<String>,
    catalog: Result<Vec<Pack>, String>,
    owned: Result<Vec<String>, EntitlementError>,
}

/// The background refresh: due time and the running fetch.
#[derive(Resource, Default)]
struct Refresher {
    next: f64,
    job: Option<Arc<Mutex<Option<Fetched>>>>,
}

/// How often entitlements and the catalog are checked again (s).
const REFRESH_EVERY: f64 = 600.0;

fn refresh(time: Res<Time<Real>>, mut r: ResMut<Refresher>, mut ui: ResMut<ui::MarketUi>) {
    if let Some(job) = &r.job {
        let done = job.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(f) = done else { return };
        r.job = None;
        ui.busy = false;
        apply(f);
    }
    let now = time.elapsed_secs_f64();
    if now < r.next && !std::mem::take(&mut ui.refresh) {
        return;
    }
    r.next = now + REFRESH_EVERY;
    ui.busy = true;
    let slot = Arc::new(Mutex::new(None));
    r.job = Some(slot.clone());
    std::thread::spawn(move || {
        let cfg = backend::Config::load();
        let b = backend::backend(&cfg);
        let f = Fetched {
            source: b.source(),
            token: cfg.token.clone(),
            catalog: b.catalog(),
            owned: b.entitlements(),
        };
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    });
}

/// A fetch's answer becomes the snapshot: a failed part keeps what was known (the cache).
fn apply(f: Fetched) {
    let cur = snapshot();
    let same = cur.source == f.source;
    let mut errors = vec![];
    let catalog = match f.catalog {
        Ok(p) => Some(p),
        Err(e) => {
            errors.push(e);
            None
        }
    };
    let (owned, signed_in) = match f.owned {
        Ok(o) => (Some(o), true),
        Err(EntitlementError::SignedOut(e)) => {
            if f.token.is_some() {
                errors.push(e);
            }
            (Some(vec![]), false)
        }
        Err(EntitlementError::Unreachable(e)) => {
            errors.push(e);
            (None, same && cur.signed_in)
        }
    };
    let fresh = catalog.is_some() || owned.is_some();
    let packs = catalog.unwrap_or_else(|| {
        if same {
            cur.catalog.clone()
        } else {
            catalog::bundled()
        }
    });
    let owned = owned.unwrap_or_else(|| {
        if same {
            cur.owned.iter().cloned().collect()
        } else {
            vec![]
        }
    });
    // a bought pack is never taken away by a refresh (signed out, offline, another backend)
    let owned = owned.into_iter().chain(backend::local_owned());
    let mut m = Market::build(packs, owned, cur.dev, f.source.clone(), &lib_packs());
    m.signed_in = signed_in;
    m.checked_at = if fresh {
        backend::now()
    } else {
        cur.checked_at
    };
    m.error = (!errors.is_empty()).then(|| errors.join("; "));
    if let Source::Marketplace(url) = &f.source
        && fresh
    {
        backend::cache_save(&backend::Cache {
            source: url.clone(),
            account: backend::account(f.token.as_deref()),
            at: m.checked_at,
            packs: m.catalog.clone(),
            owned: m.owned.iter().cloned().collect(),
        });
    }
    info!(
        "MARKET refreshed from the {}: {} packs, owned {:?}{}",
        source_label(&m.source),
        m.catalog.len(),
        m.owned,
        m.error
            .as_ref()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default()
    );
    install(m);
}

pub struct MarketPlugin;

impl Plugin for MarketPlugin {
    fn build(&self, app: &mut App) {
        LazyLock::force(&MARKET);
        app.init_resource::<ui::MarketUi>()
            .init_resource::<Refresher>()
            .add_systems(Update, refresh);
        mcp::register(app);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn developer_unlock_is_opt_in_and_never_available_in_release() {
        assert!(!super::DEV_UNLOCK_DEFAULT);
        assert_eq!(
            super::developer_unlock_available(),
            cfg!(debug_assertions) || cfg!(feature = "studio_dev")
        );
        if !super::developer_unlock_available() {
            assert_eq!(super::dev_setting(), super::Dev::Off);
        }
    }

    use super::*;

    fn pack(id: &str, prefix_price: Option<u64>, samples: &[&str]) -> Pack {
        Pack {
            id: id.into(),
            name: id.replace('_', " "),
            style: "handpainted".into(),
            publisher: "Skyfold Studio".into(),
            description: String::new(),
            price: prefix_price.map(|c| catalog::Price {
                amount_minor: c,
                currency: "USD".into(),
            }),
            free_assets: samples.iter().map(|s| s.to_string()).collect(),
            asset_count: None,
            listing_id: None,
            purchase_url: None,
            thumbnail_url: None,
            hero_asset: None,
            unlisted: false,
            installed: 0,
            previewed: 0,
        }
    }

    fn libs() -> Vec<LibPack> {
        let l = |prefix: &str, pack: &str| LibPack {
            prefix: prefix.into(),
            pack: pack.into(),
            style: "handpainted".into(),
            models: 5,
            preview: false,
        };
        vec![
            l("hp", "fantasy"),
            l("ph", "scans"),
            l("gx", "gothic_extra"),
        ]
    }

    fn market(owned: &[&str], dev: Dev) -> Market {
        let catalog = vec![
            pack("fantasy", Some(1499), &["hp:well", "hp:bench"]),
            pack("scans", None, &[]),
        ];
        Market::build(
            catalog,
            owned.iter().map(|s| s.to_string()),
            dev,
            Source::Bundled,
            &libs(),
        )
    }

    /// A paid pack shipped only as a preview (`objects::PREVIEWS`): listed with all its models,
    /// not installed, its samples usable and the rest locked; owning it doesn't make it installed.
    #[test]
    fn a_previewed_pack_is_listed_not_installed() {
        let libs = vec![LibPack {
            prefix: "hp".into(),
            pack: "fantasy".into(),
            style: "handpainted".into(),
            models: 585,
            preview: true,
        }];
        let catalog = vec![pack("fantasy", Some(4999), &["hp:well", "hp:bench"])];
        let m = Market::build(catalog.clone(), [], Dev::Off, Source::Bundled, &libs);
        let p = m.pack("fantasy").unwrap();
        assert_eq!((p.installed, p.previewed, p.assets()), (0, 585, 585));
        assert!(p.listed() && !p.unlisted);
        assert_eq!(m.access("hp:well"), Access::Sample(p));
        assert_eq!(m.access("hp:inn"), Access::Locked(p));
        assert!(m.check("hp:inn").is_err() && m.check("hp:bench").is_ok());
        // bought on the website but not downloaded yet: owned, still to install
        let m = Market::build(
            catalog,
            ["fantasy".to_string()],
            Dev::Off,
            Source::Bundled,
            &libs,
        );
        let p = m.pack("fantasy").unwrap();
        assert_eq!((m.state(p), p.installed), (PackState::Owned, 0));
    }

    #[test]
    fn the_access_rule() {
        let m = market(&[], Dev::Off);
        // no pack: built-ins, food, imports
        for id in [
            "WoodenBridge",
            "props:apple",
            "imported/my_tower",
            "zz:unknown_prefix",
        ] {
            assert_eq!(m.access(id), Access::Free, "{id}");
        }
        // a free pack
        assert_eq!(m.access("ph:barrel"), Access::Free);
        // a paid pack: its samples are usable, the rest locked
        let fantasy = m.pack("fantasy").unwrap();
        assert_eq!(m.access("hp:well"), Access::Sample(fantasy));
        assert_eq!(m.access("hp:inn"), Access::Locked(fantasy));
        assert!(!m.usable("hp:inn") && m.usable("hp:bench"));
        let e = m.check("hp:inn").unwrap_err();
        assert!(
            e.contains("`hp:inn`")
                && e.contains("fantasy pack ($14.99)")
                && e.contains("Asset packs"),
            "{e}"
        );
        assert!(m.check("hp:well").is_ok());
        // installed but not in the catalog: listed as an unlisted pack, locked (not free)
        let extra = m
            .pack("gothic_extra")
            .expect("installed packs the catalog lacks are listed");
        assert!(extra.unlisted && !extra.is_free() && extra.installed == 5);
        assert!(
            !m.usable("gx:tomb")
                && m.check("gx:tomb")
                    .unwrap_err()
                    .contains("isn't for sale yet")
        );
        assert!(m.any_locked());
        // owned
        let m = market(&["fantasy"], Dev::Off);
        assert_eq!(m.access("hp:inn"), Access::Owned);
        assert!(!m.usable("gx:tomb"));
        // the developer unlock opens everything, unlisted packs too
        for dev in [Dev::Env, Dev::Prefs] {
            let m = market(&[], dev);
            assert!(m.usable("hp:inn") && m.usable("gx:tomb") && !m.any_locked());
        }
    }

    #[test]
    fn set_pieces_need_their_whole_pack() {
        let m = market(&[], Dev::Off);
        // only samples still needs the pack: a set piece is a pack feature
        let e = m
            .check_set("graveyard", ["hp:well", "hp:bench"])
            .unwrap_err();
        assert!(e.contains("graveyard") && e.contains("fantasy"), "{e}");
        assert!(m.check_set("square", ["ph:barrel", "WoodenBridge"]).is_ok());
        assert!(
            market(&["fantasy"], Dev::Off)
                .check_set("graveyard", ["hp:inn"])
                .is_ok()
        );
    }

    /// Every set piece, drawn, shuffled or over MCP (`setpieces::stamp_area`), is refused
    /// without its pack.
    #[test]
    fn set_pieces_are_refused_without_their_pack() {
        let area: Vec<Vec2> = [(-14.0, -11.0), (14.0, -11.0), (14.0, 11.0), (-14.0, 11.0)]
            .iter()
            .map(|&(x, z)| Vec2::new(x, z))
            .collect();
        for kind in crate::setpieces::SetPieceKind::ALL {
            let lay = crate::setpieces::layout_area(kind, &area, 1).unwrap();
            let e = check_set(
                &kind.name().to_lowercase(),
                lay.pieces.iter().map(|p| p.asset.as_str()),
            )
            .unwrap_err();
            assert!(
                e.contains(&kind.name().to_lowercase())
                    && e.contains("Skyfold Studio Collection pack ($49.99)"),
                "{e}"
            );
        }
    }

    #[test]
    fn the_bundled_catalog() {
        let packs = catalog::bundled();
        let ids: Vec<&str> = packs.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "polyhaven_scans",
                "skyborn_village",
                "skyfold_photoreal",
                "skyfold_studio"
            ]
        );
        let m = Market::build(packs, [], Dev::Off, Source::Bundled, &lib_packs());
        // CC0 packs are free (Poly Haven's scans, and the public Skyborn village); the packs Skyfold
        // makes to sell are paid (the hand-painted collection with 10 free samples; the photoreal
        // set pieces too, the pavilion, the lift tower and the spire among them)
        assert_eq!(m.state(m.pack("polyhaven_scans").unwrap()), PackState::Free);
        assert_eq!(m.state(m.pack("skyborn_village").unwrap()), PackState::Free);
        assert_eq!(m.state(m.pack("skyfold_photoreal").unwrap()), PackState::Locked);
        for a in [
            "photoreal:marble_garden_pavilion",
            "photoreal:glass_lift_tower",
            "photoreal:sky_spire",
        ] {
            assert!(!m.usable(a), "{a} is paid");
        }
        let dir = std::path::PathBuf::from(crate::assets_path()).join("models");
        let lib: Vec<String> = crate::objects::model_libraries(&dir)
            .iter()
            .flat_map(|l| {
                let doc: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(l.dir.join("models.json")).unwrap(),
                )
                .unwrap();
                doc["models"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| format!("{}:{}", l.prefix, x["id"].as_str().unwrap()))
                    .collect::<Vec<_>>()
            })
            .collect();
        for id in ["skyfold_studio"] {
            let p = m.pack(id).unwrap();
            assert_eq!(
                (p.publisher.as_str(), p.free_assets.len()),
                ("Skyfold Studio", 10),
                "{id}"
            );
            assert!(
                p.price
                    .as_ref()
                    .is_some_and(|x| x.currency == "USD" && x.amount_minor > 0)
            );
            assert!(
                !p.unlisted && p.installed >= 30,
                "{id}: {} installed",
                p.installed
            );
            for a in p.free_assets.iter().chain(p.hero_asset.iter()) {
                assert!(lib.contains(a), "{id}: sample {a} isn't in the library");
                assert_eq!(m.pack_of(a).map(|x| x.id.as_str()), Some(id));
            }
            assert_eq!(m.state(p), PackState::Locked);
        }
        assert_eq!(m.pack("skyfold_studio").unwrap().price_label(), "$49.99");
        assert!(
            m.usable("hp:well")
                && !m.usable("hp:inn")
                && m.usable("ph:dutch_ship_large_01")
                && m.usable("sb:blacksmith")
        );
        assert_eq!(m.styles(), ["photoreal", "handpainted"]);
    }

    #[test]
    fn catalogs_are_checked() {
        assert!(
            catalog::parse(r#"{"packs":[{"id":"a","name":"A","style":"x","future_field":[1,2]}]}"#)
                .is_ok()
        );
        assert!(
            catalog::parse(r#"{"packs":[{"id":"a","name":"A"}]}"#).is_err(),
            "a style is required"
        );
        assert!(
            catalog::parse(
                r#"{"packs":[{"id":"a","name":"A","style":"x"},{"id":"a","name":"B","style":"x"}]}"#
            )
            .unwrap_err()
            .contains("twice")
        );
        assert!(catalog::parse(r#"{"packs":[{"id":"a","name":"A","style":"x","price":{"amount_minor":5,"currency":"dollars"}}]}"#).is_err());
        let p = |minor: u64, cur: &str| {
            catalog::Price {
                amount_minor: minor,
                currency: cur.into(),
            }
            .label()
        };
        assert_eq!(
            (p(999, "USD"), p(50, "eur"), p(1200, "JPY"), p(14900, "SEK")),
            (
                "$9.99".into(),
                "€0.50".into(),
                "¥1200".into(),
                "149.00 SEK".into()
            )
        );
    }
}
