//! The catalog: what asset packs exist, their style, publisher, price and free samples (the
//! format the marketplace backend sends, `docs/marketplace-api.md`, and the bundled
//! `assets/market/catalog.json`).

use serde::{Deserialize, Serialize};

/// A sellable set of assets in one style, from one publisher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pack {
    /// stable id, e.g. "skyfold_studio" (a library manifest's `pack`)
    pub id: String,
    pub name: String,
    /// the look ("photoreal", "handpainted"…; `crate::objects::STYLES` names them)
    pub style: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub description: String,
    /// None: free for everyone
    #[serde(default)]
    pub price: Option<Price>,
    /// asset ids anyone may use without the pack (a paid pack's samples)
    #[serde(default)]
    pub free_assets: Vec<String>,
    /// how many assets the pack has (when it isn't installed here to count)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_count: Option<u32>,
    /// where "Get pack" sends the user (opened in the browser)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// the asset that stands for the pack (its card's turntable)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero_asset: Option<String>,
    /// installed here but missing from the catalog: not for sale (yet), so locked
    #[serde(skip)]
    pub unlisted: bool,
    /// how many of its models are installed here
    #[serde(skip)]
    pub installed: usize,
}

impl Pack {
    pub fn is_free(&self) -> bool {
        self.price.is_none() && !self.unlisted
    }

    /// Its free sample `id`?
    pub fn has_sample(&self, id: &str) -> bool {
        self.free_assets.iter().any(|a| a == id)
    }

    /// How many assets it has: the ones installed here, else the catalog's count.
    pub fn assets(&self) -> usize {
        if self.installed > 0 {
            self.installed
        } else {
            self.asset_count.unwrap_or(0) as usize
        }
    }

    /// "$14.99", "Free", or "Not for sale yet".
    pub fn price_label(&self) -> String {
        match (&self.price, self.unlisted) {
            (_, true) => "Not for sale yet".into(),
            (Some(p), _) => p.label(),
            (None, _) => "Free".into(),
        }
    }
}

/// A price in the currency's minor unit (cents): `{ "amount_minor": 1499, "currency": "USD" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    pub amount_minor: u64,
    /// ISO 4217 code
    pub currency: String,
}

impl Price {
    /// "$14.99", "€9.50", "¥1200", "149.00 SEK".
    pub fn label(&self) -> String {
        let cur = self.currency.to_ascii_uppercase();
        // currencies without a minor unit
        let whole = matches!(cur.as_str(), "JPY" | "KRW" | "VND" | "CLP" | "ISK" | "HUF");
        let amount = if whole {
            format!("{}", self.amount_minor)
        } else {
            format!("{}.{:02}", self.amount_minor / 100, self.amount_minor % 100)
        };
        match cur.as_str() {
            "USD" => format!("${amount}"),
            "EUR" => format!("€{amount}"),
            "GBP" => format!("£{amount}"),
            "JPY" => format!("¥{amount}"),
            "CAD" => format!("CA${amount}"),
            "AUD" => format!("A${amount}"),
            _ => format!("{amount} {cur}"),
        }
    }
}

/// The catalog document: `{ "version": 1, "packs": [...] }`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default = "one")]
    pub version: u32,
    pub packs: Vec<Pack>,
}

fn one() -> u32 {
    1
}

/// Reads and checks a catalog: every pack needs an id, a name and a style, ids are unique,
/// currencies are 3-letter codes. Unknown fields are ignored (newer backends may send more).
pub fn parse(json: &str) -> Result<Catalog, String> {
    let c: Catalog = serde_json::from_str(json).map_err(|e| format!("unreadable catalog: {e}"))?;
    if c.version > 1 {
        // a newer format keeps the fields this one reads; say so in the log only
        bevy::log::info!(
            "MARKET catalog version {} (this build reads version 1)",
            c.version
        );
    }
    let mut seen = std::collections::HashSet::new();
    for p in &c.packs {
        if p.id.trim().is_empty() || p.name.trim().is_empty() || p.style.trim().is_empty() {
            return Err(format!(
                "catalog pack `{}` needs an id, a name and a style",
                p.id
            ));
        }
        if !seen.insert(p.id.as_str()) {
            return Err(format!("catalog lists pack `{}` twice", p.id));
        }
        if let Some(pr) = &p.price
            && (pr.currency.len() != 3 || !pr.currency.chars().all(|c| c.is_ascii_alphabetic()))
        {
            return Err(format!(
                "catalog pack `{}`: currency `{}` isn't an ISO 4217 code",
                p.id, pr.currency
            ));
        }
    }
    Ok(c)
}

/// The catalog bundled with the app (`assets/market/catalog.json`).
pub fn bundled() -> Vec<Pack> {
    let path = std::path::PathBuf::from(crate::assets_path()).join("market/catalog.json");
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| parse(&s))
    {
        Ok(c) => c.packs,
        Err(e) => {
            bevy::log::warn!("MARKET bundled catalog {}: {e}", path.display());
            vec![]
        }
    }
}

/// Entitlements: `{ "owned": ["pack_id", …] }`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Entitlements {
    #[serde(default)]
    pub owned: Vec<String>,
}
