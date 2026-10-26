//! The Marketplace: a full-window view over the editor (top bar › Marketplace, Objects › Packs,
//! ⌘K, a locked tile, `market_view` over MCP) for browsing the asset packs and the
//! skyfoldstudio.com listings by their pictures, and getting them.
//!
//! - Browse: a featured banner, the website's category chips (with counts), price and sort, then
//!   Skyfold's packs (the catalog) and the community's listings as picture cards. A catalog pack
//!   sold as a listing shows that listing's pictures; one without uses its shipped thumbnails.
//! - A card opens its page: the gallery (← →), what it is, the one action (Get / Buy / Download /
//!   Browse its assets), its free samples to place, the description and its website page.
//! - My library: the packs this account owns or that are free, Refresh, Restore purchases and,
//!   in developer builds, the unlock.
//!
//! Pictures come from the website through `site::SiteUi::image` (disk-cached, decoded off the main
//! thread). Buying goes through the same paths as before (`ui::get_pack`, `SiteUi::get`): the
//! checkout happens on the website.

use bevy_egui::egui::{
    self, Align, Color32, CornerRadius, FontId, Key, Layout, Rect, Sense, Stroke,
    StrokeKind, TextureId, Ui, Vec2, pos2, vec2,
};

use super::site::{self, IMG_CARD, IMG_FULL, Listing};
use super::ui::{
    GOLD, GREEN, PriceFilter, account_card, ago, contents, developer, get_pack, hero, open_url,
    pack_row, start_placing, status, tips,
};
use super::{Market, Pack, PackState};
use crate::thumbs::Subject;
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// The view's two pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Browse,
    Library,
}

/// What a card's page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// a catalog pack (its id)
    Pack(String),
    /// a community listing (its id)
    Listing(String),
}

/// The listings' order (the website's `sort`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Popular,
    New,
    Rated,
    PriceLow,
}

impl Sort {
    const ALL: [Sort; 4] = [Sort::Popular, Sort::New, Sort::Rated, Sort::PriceLow];
    fn id(self) -> &'static str {
        match self {
            Sort::Popular => "popular",
            Sort::New => "new",
            Sort::Rated => "rated",
            Sort::PriceLow => "price_asc",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Sort::Popular => "Popular",
            Sort::New => "Newest",
            Sort::Rated => "Top rated",
            Sort::PriceLow => "Price: low to high",
        }
    }
}

/// A picture: a website picture, an asset's thumbnail, or none yet (an icon stands in).
#[derive(Clone)]
enum Pic {
    Web(egui::TextureHandle),
    Thumb(TextureId),
    None,
}

/// A card in the grid.
struct Card {
    item: Item,
    title: String,
    by: String,
    official: bool,
    /// "Pack · 585 assets"
    meta: String,
    /// "$49.99" / "Free" / "Owned", and its colour
    price: (String, Color32),
    rating: Option<(f32, u32)>,
    pic: Pic,
}

/// The filters as a website query.
fn query(s: &UiState) -> site::Query {
    let m = &s.market;
    site::Query {
        q: m.applied.trim().to_string(),
        category: m.category.clone(),
        price: match m.vprice {
            PriceFilter::All => String::new(),
            PriceFilter::Free => "free".into(),
            PriceFilter::Paid => "paid".into(),
        },
        sort: m.sort.id().into(),
    }
}

/// The view (every frame; drawn only while open).
pub fn show(ctx: &egui::Context, s: &mut UiState) {
    // a locked tile (or anything else) asked for a pack's page
    if let Some(id) = s.market.card.take() {
        s.market.open = true;
        s.market.view = View::Browse;
        s.market.detail = Some(Item::Pack(id));
        s.market.shot = 0;
    }
    if !s.market.open {
        s.market.opened_on = None;
        return;
    }
    // a rail tab clicked while it's open: show that tab instead
    match s.market.opened_on {
        None => s.market.opened_on = Some(s.ed.tab),
        Some(t) if t != s.ed.tab => {
            s.market.open = false;
            s.market.opened_on = None;
            return;
        }
        _ => {}
    }
    // Esc: a page goes back to the grid, the grid closes the view
    if ctx.input(|i| i.key_pressed(Key::Escape)) && !ctx.egui_wants_keyboard_input() {
        if s.market.site.signing_in {
            s.market.site.signing_in = false;
        } else if s.market.detail.take().is_none() {
            s.market.open = false;
            return;
        }
    }
    // the search applies once typing pauses
    let now = ctx.input(|i| i.time);
    if s.market.search != s.market.applied && now - s.market.search_at > 0.35 {
        s.market.applied = s.market.search.clone();
    }
    // the listings for the current filters
    let q = query(s);
    let site = &mut s.market.site;
    if site.view.query != q || (site.view.loaded.is_none() && !site.view.loading) {
        site.load_view(q, false);
    }
    // (pictures and pages arrive from background threads)
    ctx.request_repaint_after(std::time::Duration::from_millis(200));

    let m = super::snapshot();
    let screen = ctx.content_rect();
    let rect = Rect::from_min_max(
        pos2(
            screen.left() + crate::ui::shell::PANEL_X,
            screen.top() + crate::ui::shell::TOP,
        ),
        pos2(screen.right() - 12.0, screen.bottom() - 12.0),
    );
    egui::Area::new(egui::Id::new("marketplace"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            let frame = glass_solid_frame()
                .corner_radius(CornerRadius::same(R_PANEL))
                .inner_margin(egui::Margin::same(18));
            glass(ui, frame, |ui| {
                let inner = rect.size() - vec2(38.0, 38.0);
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                header(ui, s, &m, now);
                ui.add_space(12.0);
                match s.market.detail.clone() {
                    Some(item) => page(ui, s, &m, &item),
                    None => match s.market.view {
                        View::Browse => browse(ui, s, &m),
                        View::Library => library(ui, s, &m),
                    },
                }
            });
        });
}

/// Title, Browse / My library, search, account and close.
fn header(ui: &mut Ui, s: &mut UiState, m: &Market, now: f64) {
    ui.horizontal(|ui| {
        ui.set_height(34.0);
        let (r, _) = ui.allocate_exact_size(vec2(26.0, 34.0), Sense::hover());
        paint_icon(ui, r.center(), "Store1", 22.0, TEXT);
        ui.label(
            egui::RichText::new("Marketplace")
                .font(FontId::new(20.0, semibold()))
                .color(TEXT),
        );
        ui.add_space(16.0);
        let mut v = s.market.view;
        ui.allocate_ui(vec2(230.0, 30.0), |ui| {
            if segmented(ui, &mut v, &[(View::Browse, "Browse"), (View::Library, "My library")]) {
                s.market.view = v;
                s.market.detail = None;
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if icon_button(ui, "CrossMedium", "Close (Esc)").clicked() {
                s.market.open = false;
            }
            account(ui, s);
            if m.packs().iter().any(|p| m.state(p) == PackState::Locked) {
                ui.label(egui::RichText::new(source_line(m)).small().color(TEXT_MUTE));
            }
            if s.market.view == View::Browse && s.market.detail.is_none() {
                ui.allocate_ui(vec2(320.0f32.min(ui.available_width() * 0.5), 30.0), |ui| {
                    let r = search_field(ui, &mut s.market.search, "Search packs, assets, creators");
                    if r.changed() {
                        s.market.search_at = now;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        s.market.applied = s.market.search.clone();
                    }
                });
            }
        });
    });
}

/// Where the packs and prices come from, and when they were last checked.
fn source_line(m: &Market) -> String {
    match (&m.source, m.checked_at) {
        (super::backend::Source::Bundled, _) => "Built-in catalog".to_string(),
        (_, 0) => "Marketplace not reached yet".to_string(),
        (_, at) => format!("Checked {}", ago(at)),
    }
}

/// Signed in (and Sign out), or Sign in (opens the API key card).
fn account(ui: &mut Ui, s: &mut UiState) {
    if s.market.site.is_signed_in() {
        if small_button(ui, "Sign out")
            .on_hover_text("Forget this computer's skyfoldstudio.com API key")
            .clicked()
        {
            s.market.site.sign_out();
        }
        ui.label(
            egui::RichText::new("Signed in")
                .font(FontId::new(12.0, semibold()))
                .color(OK),
        );
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 24.0), Sense::hover());
        paint_icon(ui, r.center(), "User", 15.0, OK);
    } else if small_icon_button(ui, "User", "Sign in")
        .on_hover_text("Sign in with your Skyfold account to buy listings and restore purchases")
        .clicked()
    {
        s.market.site.signing_in = !s.market.site.signing_in;
    }
}

/// The sign-in card, when it's open, above whatever page shows.
fn sign_in_box(ui: &mut Ui, s: &mut UiState) {
    if !s.market.site.signing_in || s.market.site.is_signed_in() {
        return;
    }
    ui.horizontal(|ui| {
        ui.allocate_ui(vec2(460.0f32.min(ui.available_width()), 0.0), |ui| account_card(ui, s));
    });
    ui.add_space(12.0);
}

// ------------------------------------------------------------------ browse

/// The catalog packs the filters keep, in catalog order (paid first, so the featured one leads).
fn packs_shown(s: &mut UiState, m: &Market) -> Vec<Pack> {
    let q = s.market.applied.trim().to_lowercase();
    let cat = s.market.category.clone();
    let price = s.market.vprice;
    let mut v: Vec<Pack> = m
        .packs()
        .iter()
        .filter(|p| !p.unlisted || m.state(p) != PackState::Locked)
        .filter(|p| price.keeps_pack(p))
        .filter(|p| {
            q.is_empty()
                || p.name.to_lowercase().contains(&q)
                || p.description.to_lowercase().contains(&q)
                || p.publisher.to_lowercase().contains(&q)
                || p.style.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    if !(cat.is_empty() || cat == "all") {
        v.retain(|p| {
            p.listing_id
                .as_deref()
                .and_then(|id| s.market.site.listing(id))
                .is_some_and(|l| l.category == cat)
        });
    }
    v.sort_by_key(|p| p.is_free());
    v
}

/// A catalog pack's pictures: its listing's (website), else its hero and samples' thumbnails.
fn pack_pictures(s: &mut UiState, m: &Market, p: &Pack, max: u32) -> Vec<Pic> {
    let urls = p
        .listing_id
        .as_deref()
        .and_then(|id| s.market.site.listing(id))
        .map(|l| l.pictures())
        .unwrap_or_default();
    if !urls.is_empty() {
        return urls
            .iter()
            .map(|u| s.market.site.image(u, max).map_or(Pic::None, Pic::Web))
            .collect();
    }
    let mut ids: Vec<String> = hero(s, m, p).into_iter().collect();
    for a in &p.free_assets {
        if !ids.contains(a) && s.lib.as_deref().and_then(|l| l.get(a)).is_some() {
            ids.push(a.clone());
        }
    }
    ids.iter()
        .map(|id| {
            s.previews
                .thumb(&Subject::Asset(id.clone()))
                .map_or(Pic::None, Pic::Thumb)
        })
        .collect()
}

fn pack_card(s: &mut UiState, m: &Market, p: &Pack) -> Card {
    let pic = pack_pictures(s, m, p, IMG_CARD)
        .into_iter()
        .next()
        .unwrap_or(Pic::None);
    let listing = p
        .listing_id
        .as_deref()
        .and_then(|id| s.market.site.listing(id))
        .cloned();
    let (st, col) = status(m, p);
    Card {
        item: Item::Pack(p.id.clone()),
        title: p.name.clone(),
        by: if p.publisher.is_empty() {
            "Skyfold".into()
        } else {
            p.publisher.clone()
        },
        official: listing.as_ref().is_some_and(|l| l.official()) || p.listing_id.is_some(),
        meta: format!(
            "{} · {}",
            crate::objects::pack_label(&p.style),
            assets_label(p.assets())
        ),
        price: (plain(&st), col),
        rating: listing
            .filter(|l| l.rating_count > 0)
            .map(|l| (l.rating_average, l.rating_count)),
        pic,
    }
}

fn listing_card(s: &mut UiState, l: &Listing) -> Card {
    let pic = l
        .pictures()
        .first()
        .and_then(|u| s.market.site.image(u, IMG_CARD))
        .map_or(Pic::None, Pic::Web);
    let what = if l.kind == "pack" { "Pack" } else { "Asset" };
    let meta = match (l.piece_count, l.category_label.is_empty()) {
        (Some(n), _) if l.kind == "pack" => format!("{what} · {}", assets_label(n as usize)),
        (_, false) => format!("{what} · {}", l.category_label),
        _ => what.to_string(),
    };
    Card {
        item: Item::Listing(l.id.clone()),
        title: l.title.clone(),
        by: l.by(),
        official: l.official(),
        meta,
        price: (
            l.price_label(),
            if l.is_free() { GREEN } else { GOLD },
        ),
        rating: (l.rating_count > 0).then_some((l.rating_average, l.rating_count)),
        pic,
    }
}

/// "1 asset" / "585 assets".
fn assets_label(n: usize) -> String {
    if n == 1 {
        "1 asset".into()
    } else {
        format!("{n} assets")
    }
}

/// A status without its Phosphor glyph ("✓ Owned" → "Owned"; icon glyphs are private-use).
fn plain(st: &str) -> String {
    st.trim_start_matches(|c: char| ('\u{e000}'..='\u{f8ff}').contains(&c))
        .trim()
        .to_string()
}

fn browse(ui: &mut Ui, s: &mut UiState, m: &Market) {
    // categories (with the website's counts), price, sort
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let facets = s.market.site.view.facets.clone();
        let mut pick = None;
        let all = s.market.category.is_empty() || s.market.category == "all";
        if chip(ui, "All", facets.get("all").map(|n| *n as usize), all).clicked() {
            pick = Some(String::new());
        }
        for (id, label) in site::CATEGORIES {
            let n = facets.get(id).copied().unwrap_or(0) as usize;
            if chip(ui, label, (!facets.is_empty()).then_some(n), s.market.category == id).clicked() {
                pick = Some(id.to_string());
            }
        }
        if let Some(c) = pick {
            s.market.category = c;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            egui::ComboBox::from_id_salt("market_sort")
                .selected_text(s.market.sort.label())
                .width(160.0)
                .dropdown_ui(ui, |ui| {
                    for o in Sort::ALL {
                        menu_option(ui, &mut s.market.sort, o, o.label());
                    }
                });
            ui.allocate_ui(vec2(190.0, 30.0), |ui| {
                segmented(
                    ui,
                    &mut s.market.vprice,
                    &[
                        (PriceFilter::All, "All"),
                        (PriceFilter::Free, "Free"),
                        (PriceFilter::Paid, "Paid"),
                    ],
                );
            });
        });
    });
    ui.add_space(12.0);
    let packs = packs_shown(s, m);
    let catalog_listings: Vec<String> = m
        .packs()
        .iter()
        .filter_map(|p| p.listing_id.clone())
        .collect();
    let listings: Vec<Listing> = s
        .market
        .site
        .view
        .listings
        .iter()
        .filter(|l| !catalog_listings.contains(&l.id))
        .cloned()
        .collect();
    let filtered = !s.market.applied.trim().is_empty()
        || !(s.market.category.is_empty() || s.market.category == "all")
        || s.market.vprice != PriceFilter::All;
    let mut open: Option<Item> = None;
    egui::ScrollArea::vertical()
        .id_salt("market_browse")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 6.0);
            sign_in_box(ui, s);
            tips(ui);
            // the featured pack
            if !filtered
                && let Some(p) = packs.iter().find(|p| !p.is_free()).or(packs.first())
                && banner(ui, s, m, p)
            {
                open = Some(Item::Pack(p.id.clone()));
            }
            if !packs.is_empty() {
                group_title(ui, "Skyfold packs", &format!("{}", packs.len()));
                let cards: Vec<Card> = packs.iter().map(|p| pack_card(s, m, p)).collect();
                if let Some(i) = grid(ui, &cards) {
                    open = Some(cards[i].item.clone());
                }
                ui.add_space(18.0);
            }
            let view = &s.market.site.view;
            let total = view.total.map(|t| t.saturating_sub(
                view.listings.len().saturating_sub(listings.len()) as u64,
            ));
            group_title(
                ui,
                "Community · skyfoldstudio.com",
                &total.map(|t| t.to_string()).unwrap_or_default(),
            );
            if listings.is_empty() {
                let view = &s.market.site.view;
                if view.loading && view.loaded.is_none() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        dim(ui, "Loading listings…");
                    });
                } else if let Some(e) = view.error.clone() {
                    warn_line(ui, &format!("Couldn't reach skyfoldstudio.com ({e})."));
                    if small_button(ui, "Try again").clicked() {
                        s.market.site.view.loaded = None;
                    }
                } else if filtered {
                    dim(ui, "No community listings match.");
                } else {
                    dim(
                        ui,
                        "No community listings yet. Creators publish them in the Creator studio at skyfoldstudio.com/studio.",
                    );
                }
            } else {
                let cards: Vec<Card> = listings.iter().map(|l| listing_card(s, l)).collect();
                if let Some(i) = grid(ui, &cards) {
                    open = Some(cards[i].item.clone());
                }
                if s.market.site.view.next.is_some() {
                    ui.add_space(10.0);
                    ui.vertical_centered(|ui| {
                        if s.market.site.view.loading {
                            ui.spinner();
                        } else if small_button(ui, "Show more").clicked() {
                            let q = s.market.site.view.query.clone();
                            s.market.site.load_view(q, true);
                        }
                    });
                }
            }
            ui.add_space(12.0);
        });
    if let Some(item) = open {
        s.market.detail = Some(item);
        s.market.shot = 0;
    }
}

/// A grid section's title with a muted count.
fn group_title(ui: &mut Ui, title: &str, count: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .font(FontId::new(15.0, semibold()))
                .color(TEXT),
        );
        if !count.is_empty() {
            ui.label(
                egui::RichText::new(count)
                    .font(FontId::new(12.0, mono()))
                    .color(TEXT_MUTE),
            );
        }
    });
    ui.add_space(8.0);
}

/// The featured banner: the pack's cover big, what it is and its price. True if clicked.
fn banner(ui: &mut Ui, s: &mut UiState, m: &Market, p: &Pack) -> bool {
    let w = ui.available_width();
    let h = (w * 0.30).clamp(220.0, 320.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let pics = pack_pictures(s, m, p, IMG_FULL);
        let pic_rect = Rect::from_min_max(rect.min, pos2(rect.left() + w * 0.62, rect.bottom()));
        ui.painter()
            .rect_filled(rect, CornerRadius::same(14), CARD_BG);
        paint_pic(
            ui,
            pics.first().unwrap_or(&Pic::None),
            pic_rect,
            CornerRadius { nw: 14, sw: 14, ne: 0, se: 0 },
            resp.hovered(),
        );
        let listing = p
            .listing_id
            .as_deref()
            .and_then(|id| s.market.site.listing(id))
            .cloned();
        let text = Rect::from_min_max(pos2(pic_rect.right() + 22.0, rect.top() + 22.0), rect.max - vec2(22.0, 22.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(text)
                .layout(Layout::top_down(Align::Min)),
        );
        let ui = &mut child;
        ui.label(
            egui::RichText::new("FEATURED")
                .font(FontId::new(10.5, mono_semibold()))
                .color(TEXT_MUTE),
        );
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(&p.name)
                    .font(FontId::new(24.0, semibold()))
                    .color(TEXT),
            )
            .wrap(),
        );
        by_line(ui, &p.publisher, p.listing_id.is_some());
        ui.add_space(6.0);
        let desc = listing
            .as_ref()
            .map(|l| l.description.clone())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| p.description.clone());
        let mut job = egui::text::LayoutJob::simple(
            first_paragraph(&desc),
            FontId::proportional(13.0),
            TEXT_DIM,
            ui.available_width(),
        );
        job.wrap.max_rows = 4;
        job.wrap.overflow_character = Some('…');
        ui.label(job);
        ui.add_space(10.0);
        let (st, col) = status(m, p);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(plain(&st))
                    .font(FontId::new(16.0, semibold()))
                    .color(col),
            );
            ui.label(egui::RichText::new(contents(m, p)).small().color(TEXT_DIM));
        });
        ui.add_space(8.0);
        if primary_icon_button(ui, "Package", "See what's inside").clicked() {
            return true;
        }
    }
    resp.clicked()
}

/// The first paragraph of a description (a blank line ends it).
fn first_paragraph(d: &str) -> String {
    d.split("\n\n").next().unwrap_or(d).replace('\n', " ")
}

/// "by Skyfold Studio · Official".
fn by_line(ui: &mut Ui, by: &str, official: bool) {
    if by.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(egui::RichText::new(format!("by {by}")).color(TEXT_DIM));
        if official {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
            paint_icon(ui, r.center(), "CheckCircle2", 13.0, TEXT);
            ui.label(
                egui::RichText::new("Official")
                    .font(FontId::new(12.0, semibold()))
                    .color(TEXT),
            );
        }
    });
}

/// Cards in rows filling the width. The clicked card's index.
fn grid(ui: &mut Ui, cards: &[Card]) -> Option<usize> {
    const GAP: f32 = 14.0;
    let w = ui.available_width();
    let cols = ((w + GAP) / (250.0 + GAP)).floor().max(1.0) as usize;
    let cw = ((w - GAP * (cols - 1) as f32) / cols as f32).floor();
    let mut clicked = None;
    for (r, row) in cards.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for (i, c) in row.iter().enumerate() {
                if card(ui, c, cw).clicked() {
                    clicked = Some(r * cols + i);
                }
            }
        });
        ui.add_space(GAP);
    }
    clicked
}

/// A picture card: the picture (16:10), the price on it, title, creator, meta and rating.
fn card(ui: &mut Ui, c: &Card, w: f32) -> egui::Response {
    let ih = (w * 0.625).round();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ih + 84.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hovered = resp.hovered();
    let p = ui.painter();
    p.rect_filled(
        rect,
        CornerRadius::same(12),
        if hovered { CARD_HOVER } else { CARD_BG },
    );
    if hovered {
        p.rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(1.0, Color32::from_white_alpha(70)),
            StrokeKind::Inside,
        );
    }
    let img = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(w - 12.0, ih - 6.0));
    paint_pic(ui, &c.pic, img, CornerRadius::same(8), hovered);
    // the price, on the picture
    let p = ui.painter();
    let g = p.layout_no_wrap(c.price.0.clone(), FontId::new(12.0, semibold()), c.price.1);
    let pill = Rect::from_min_size(
        pos2(img.right() - g.size().x - 22.0, img.top() + 8.0),
        g.size() + vec2(14.0, 6.0),
    );
    p.rect_filled(pill, CornerRadius::same(255), Color32::from_black_alpha(170));
    p.galley(pill.min + vec2(7.0, 3.0), g, c.price.1);
    // title, creator, meta
    let x = rect.left() + 12.0;
    let tw = w - 24.0;
    let mut y = img.bottom() + 10.0;
    let g = clipped(p, &c.title, FontId::new(14.0, semibold()), TEXT, tw);
    let gh = g.size().y;
    p.galley(pos2(x, y), g, TEXT);
    y += gh + 3.0;
    let by = if c.by.is_empty() {
        String::new()
    } else if c.official {
        format!("by {}  ·  Official", c.by)
    } else {
        format!("by {}", c.by)
    };
    let g = clipped(p, &by, FontId::proportional(12.0), TEXT_DIM, tw);
    let gh = g.size().y;
    p.galley(pos2(x, y), g, TEXT_DIM);
    y += gh + 6.0;
    let rating = c
        .rating
        .map(|(a, n)| format!("★ {a:.1} ({n})"))
        .unwrap_or_default();
    let rg = p.layout_no_wrap(rating, FontId::new(11.0, mono()), TEXT_DIM);
    let g = clipped(
        p,
        &c.meta,
        FontId::new(11.0, mono()),
        TEXT_MUTE,
        tw - rg.size().x - 8.0,
    );
    p.galley(pos2(x, y), g, TEXT_MUTE);
    p.galley(pos2(rect.right() - 12.0 - rg.size().x, y), rg, TEXT_DIM);
    resp
}

/// One line of text cut to `w` with an ellipsis.
fn clipped(
    p: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, w.max(10.0));
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    job.wrap.break_anywhere = true;
    p.layout_job(job)
}

/// A picture in `rect`: website pictures fill it (cropped); thumbnails sit on a soft well; none
/// yet: a package icon.
fn paint_pic(ui: &Ui, pic: &Pic, rect: Rect, rad: CornerRadius, hovered: bool) {
    match pic {
        Pic::Web(t) => {
            let ts = t.size_vec2();
            let (ca, ia) = (rect.width() / rect.height().max(1.0), ts.x / ts.y.max(1.0));
            let mut uv = if ia > ca {
                Rect::from_center_size(pos2(0.5, 0.5), vec2(ca / ia, 1.0))
            } else {
                Rect::from_center_size(pos2(0.5, 0.5), vec2(1.0, ia / ca))
            };
            if hovered {
                uv = uv.shrink2(uv.size() * 0.015);
            }
            egui::Image::new((t.id(), rect.size()))
                .uv(uv)
                .corner_radius(rad)
                .paint_at(ui, rect);
        }
        Pic::Thumb(t) => {
            card_well(ui, rect, hovered);
            let side = rect.height().min(rect.width()) * if hovered { 0.94 } else { 0.9 };
            egui::Image::new((*t, Vec2::splat(side)))
                .paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(side)));
        }
        Pic::None => {
            ui.painter().rect_filled(rect, rad, WELL);
            paint_icon(ui, rect.center(), "Package", 26.0, TEXT_MUTE);
        }
    }
}

// ------------------------------------------------------------------ a card's page

/// What a page shows, from a pack or a listing.
struct Shown {
    title: String,
    by: String,
    official: bool,
    pics: Vec<Pic>,
    description: String,
    facts: Vec<(&'static str, String)>,
    link: Option<String>,
}

fn page(ui: &mut Ui, s: &mut UiState, m: &Market, item: &Item) {
    let base = site::base();
    let (shown, pack, listing) = match item {
        Item::Pack(id) => {
            let Some(p) = m.pack(id).cloned() else {
                s.market.detail = None;
                return;
            };
            let listing = p
                .listing_id
                .as_deref()
                .and_then(|id| s.market.site.listing(id))
                .cloned();
            let mut facts = vec![
                ("Style", crate::objects::pack_label(&p.style)),
                ("Assets", p.assets().to_string()),
            ];
            if let Some(l) = &listing {
                if !l.category_label.is_empty() {
                    facts.push(("Category", l.category_label.clone()));
                }
                if !l.license.is_empty() {
                    facts.push(("License", license(&l.license)));
                }
                if l.rating_count > 0 {
                    facts.push(("Rating", format!("★ {:.1} ({})", l.rating_average, l.rating_count)));
                }
            }
            let shown = Shown {
                title: p.name.clone(),
                by: p.publisher.clone(),
                official: p.listing_id.is_some(),
                pics: pack_pictures(s, m, &p, IMG_FULL),
                description: listing
                    .as_ref()
                    .map(|l| l.description.clone())
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| p.description.clone()),
                facts,
                link: listing.as_ref().map(|l| l.page_url(&base)),
            };
            (shown, Some(p), listing)
        }
        Item::Listing(id) => {
            let Some(l) = s.market.site.listing(id).cloned() else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    dim(ui, "Loading…");
                });
                return;
            };
            let mut facts = vec![("Type", if l.kind == "pack" { "Pack".to_string() } else { "Asset".to_string() })];
            if let Some(n) = l.piece_count {
                facts.push(("Assets", n.to_string()));
            }
            if !l.category_label.is_empty() {
                facts.push(("Category", l.category_label.clone()));
            }
            if !l.license.is_empty() {
                facts.push(("License", license(&l.license)));
            }
            if l.rating_count > 0 {
                facts.push(("Rating", format!("★ {:.1} ({})", l.rating_average, l.rating_count)));
            }
            let pics = l
                .pictures()
                .iter()
                .map(|u| s.market.site.image(u, IMG_FULL).map_or(Pic::None, Pic::Web))
                .collect();
            let shown = Shown {
                title: l.title.clone(),
                by: l.by(),
                official: l.official(),
                pics,
                description: l.description.clone(),
                facts,
                link: Some(l.page_url(&base)),
            };
            (shown, None, Some(l))
        }
    };
    // back
    ui.horizontal(|ui| {
        if small_icon_button(ui, "ArrowLeft", "Back").clicked() {
            s.market.detail = None;
        }
        ui.label(
            egui::RichText::new(match item {
                Item::Pack(_) => "Skyfold packs",
                Item::Listing(_) => "Community",
            })
            .small()
            .color(TEXT_MUTE),
        );
    });
    if s.market.detail.is_none() {
        return;
    }
    ui.add_space(10.0);
    sign_in_box(ui, s);
    let n = shown.pics.len();
    if n > 0 {
        s.market.shot = s.market.shot.min(n - 1);
        if !ui.ctx().egui_wants_keyboard_input() {
            if ui.input(|i| i.key_pressed(Key::ArrowRight)) {
                s.market.shot = (s.market.shot + 1) % n;
            }
            if ui.input(|i| i.key_pressed(Key::ArrowLeft)) {
                s.market.shot = (s.market.shot + n - 1) % n;
            }
        }
    }
    let avail = ui.available_size();
    let left_w = ((avail.x - 24.0) * 0.6).floor();
    let right_w = avail.x - 24.0 - left_w;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        column(ui, left_w, avail.y, |ui| gallery(ui, s, &shown.pics));
        column(ui, right_w, avail.y, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("market_page")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(right_w - 12.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&shown.title)
                                .font(FontId::new(24.0, semibold()))
                                .color(TEXT),
                        )
                        .wrap(),
                    );
                    by_line(ui, &shown.by, shown.official);
                    ui.add_space(12.0);
                    match (&pack, &listing) {
                        (Some(p), _) => pack_actions(ui, s, m, p),
                        (None, Some(l)) => listing_actions(ui, s, l),
                        _ => {}
                    }
                    if let Some(url) = &shown.link
                        && wide_icon_button(ui, "ArrowUpRight", "View on skyfoldstudio.com").clicked()
                        && !open_url(url)
                    {
                        s.ed.toast("Couldn't open skyfoldstudio.com");
                    }
                    ui.add_space(14.0);
                    for (k, v) in &shown.facts {
                        stat_row(ui, k, v);
                    }
                    if let Some(p) = &pack {
                        samples(ui, s, m, p);
                    }
                    if !shown.description.is_empty() {
                        ui.add_space(14.0);
                        description(ui, &shown.description);
                    }
                    ui.add_space(12.0);
                });
        });
    });
}

/// "custom" → "Custom license", "CC0" stays.
fn license(l: &str) -> String {
    if l.eq_ignore_ascii_case("custom") {
        "Custom license".into()
    } else {
        l.to_string()
    }
}

fn column(ui: &mut Ui, w: f32, h: f32, body: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    child.set_clip_rect(rect.expand(2.0).intersect(ui.clip_rect()));
    child.set_width(w);
    body(&mut child);
}

/// The big picture with ← → and a counter, the strip of all of them below.
fn gallery(ui: &mut Ui, s: &mut UiState, pics: &[Pic]) {
    let w = ui.available_width();
    let h = (w * 0.625).min(ui.available_height() - 96.0).max(160.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let n = pics.len();
    let i = s.market.shot.min(n.saturating_sub(1));
    paint_pic(
        ui,
        pics.get(i).unwrap_or(&Pic::None),
        rect,
        CornerRadius::same(12),
        false,
    );
    if n > 1 {
        for (dir, x, icon) in [(-1i32, rect.left() + 26.0, "ChevronLeftSmall"), (1, rect.right() - 26.0, "ChevronRightSmall")] {
            let c = pos2(x, rect.center().y);
            let r = ui.interact(
                Rect::from_center_size(c, Vec2::splat(36.0)),
                ui.id().with(("market_arrow", dir)),
                Sense::click(),
            );
            let p = ui.painter();
            p.circle_filled(
                c,
                18.0,
                Color32::from_black_alpha(if r.hovered() { 210 } else { 150 }),
            );
            paint_icon(ui, c, icon, 20.0, Color32::WHITE);
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                s.market.shot = ((i as i32 + dir).rem_euclid(n as i32)) as usize;
            }
        }
        let p = ui.painter();
        let g = p.layout_no_wrap(
            format!("{} / {n}", i + 1),
            FontId::new(11.0, mono_semibold()),
            Color32::WHITE,
        );
        let pill = Rect::from_min_size(
            pos2(rect.right() - g.size().x - 22.0, rect.bottom() - g.size().y - 18.0),
            g.size() + vec2(12.0, 6.0),
        );
        p.rect_filled(pill, CornerRadius::same(255), Color32::from_black_alpha(170));
        p.galley(pill.min + vec2(6.0, 3.0), g, Color32::WHITE);
        // the strip
        ui.add_space(10.0);
        egui::ScrollArea::horizontal()
            .id_salt("market_strip")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    for (k, pic) in pics.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(vec2(112.0, 70.0), Sense::click());
                        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        paint_pic(ui, pic, r, CornerRadius::same(7), resp.hovered());
                        if k == i {
                            ui.painter().rect_stroke(
                                r,
                                CornerRadius::same(7),
                                Stroke::new(2.0, Color32::WHITE),
                                StrokeKind::Outside,
                            );
                        } else if !resp.hovered() {
                            ui.painter().rect_filled(r, CornerRadius::same(7), Color32::from_black_alpha(80));
                        }
                        if resp.clicked() {
                            s.market.shot = k;
                        }
                    }
                });
            });
    }
}

/// A description: paragraphs, and "- " lines as bullets.
fn description(ui: &mut Ui, d: &str) {
    for para in d.split("\n\n") {
        let lines: Vec<&str> = para.lines().collect();
        for line in lines {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("• ")) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(egui::RichText::new("•").color(TEXT_MUTE));
                    ui.add(egui::Label::new(egui::RichText::new(b).color(TEXT_DIM)).wrap());
                });
            } else if t.len() < 40 && !t.ends_with('.') && !t.contains(". ") {
                // a short line on its own reads as a heading ("What's inside")
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(t)
                        .font(FontId::new(13.0, semibold()))
                        .color(TEXT),
                );
            } else {
                ui.add(egui::Label::new(egui::RichText::new(t).color(TEXT_DIM)).wrap());
            }
        }
        ui.add_space(8.0);
    }
}

/// A catalog pack's status and its one action (and Browse its assets when it's usable).
fn pack_actions(ui: &mut Ui, s: &mut UiState, m: &Market, p: &Pack) {
    let state = m.state(p);
    let working = s.market.site.working.contains(&p.id);
    let (st, col) = status(m, p);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(plain(&st))
                .font(FontId::new(20.0, semibold()))
                .color(col),
        );
        ui.label(egui::RichText::new(contents(m, p)).color(TEXT_DIM));
    });
    ui.add_space(8.0);
    match state {
        // bought, but its files aren't on this computer (a new computer, a reinstall)
        PackState::Owned if m.owned.contains(&p.id) && p.installed == 0 => {
            ok_line(ui, "Owned: download it to use it here.");
            ui.add_space(4.0);
            if working {
                ui.horizontal(|ui| {
                    ui.spinner();
                    dim(ui, "Downloading and installing…");
                });
            } else if p.listing_id.is_some()
                && primary_icon_button(ui, "ArrowDown", "Download pack").clicked()
            {
                if s.market.site.is_signed_in() {
                    s.market.site.reinstall(p);
                } else {
                    s.market.site.signing_in = true;
                    s.ed.toast("Sign in with the Skyfold account that bought it, then press Download pack");
                }
            }
        }
        PackState::Owned => ok_line(
            ui,
            if m.owned.contains(&p.id) {
                "Owned: every asset in it is yours to place."
            } else {
                "Unlocked on this computer (developer)."
            },
        ),
        PackState::Free => ok_line(ui, "Free for everyone."),
        PackState::Locked if p.unlisted => dim(ui, "This pack isn't for sale yet."),
        PackState::Locked if working => {
            ui.horizontal(|ui| {
                ui.spinner();
                dim(ui, "Finish paying on the checkout page in your browser: it installs here by itself.");
            });
        }
        PackState::Locked => {
            if primary_icon_button(ui, "Store1", &format!("Get pack · {}", p.price_label())).clicked() {
                get_pack(s, p);
            }
            ui.add_space(4.0);
            hint_small(ui, "Paid on skyfoldstudio.com, then it installs here by itself. Bought it already? Restore purchases in My library.");
        }
    }
    if p.listed() && state != PackState::Locked {
        ui.add_space(6.0);
        if wide_icon_button(ui, "LayoutGrid1", &format!("Browse its {}", assets_label(p.assets()))).clicked() {
            browse_assets(s, p);
        }
    }
    ui.add_space(6.0);
}

/// Objects › Place with just this pack's assets showing.
fn browse_assets(s: &mut UiState, p: &Pack) {
    s.market.filter = p.id.clone();
    s.market.browse = super::ui::Browse::Assets;
    s.place.style = p.style.clone();
    s.place.group.clear();
    s.ed.tab = crate::editor::Tab::Objects;
    s.market.open = false;
    s.market.detail = None;
}

/// A locked pack's free samples, to place right away.
fn samples(ui: &mut Ui, s: &mut UiState, m: &Market, p: &Pack) {
    if m.state(p) != PackState::Locked {
        return;
    }
    let lib = s.lib.as_deref();
    let samples: Vec<(String, String)> = p
        .free_assets
        .iter()
        .filter_map(|a| lib.and_then(|l| l.get(a)).map(|e| (e.id.clone(), e.name.clone())))
        .collect();
    if samples.is_empty() {
        return;
    }
    ui.add_space(14.0);
    section(ui, "Free to try");
    hint_small(ui, "Click one to place it in your world.");
    ui.add_space(4.0);
    let cols = 5;
    let w = ((ui.available_width() - (cols - 1) as f32 * 6.0) / cols as f32).floor();
    let mut place = None;
    for row in samples.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (id, name) in row {
                if crate::thumbs::ui::tile(
                    ui,
                    &mut s.previews,
                    &Subject::Asset(id.clone()),
                    egui_phosphor::regular::BASKET,
                    name,
                    false,
                    w,
                )
                .on_hover_text(format!("{name}: free to use"))
                .clicked()
                {
                    place = Some(id.clone());
                }
            }
        });
    }
    if let Some(id) = place {
        start_placing(s, &id);
    }
}

/// A community listing's price and Get / Buy.
fn listing_actions(ui: &mut Ui, s: &mut UiState, l: &Listing) {
    ui.label(
        egui::RichText::new(l.price_label())
            .font(FontId::new(20.0, semibold()))
            .color(if l.is_free() { GREEN } else { GOLD }),
    );
    ui.add_space(8.0);
    if s.market.site.working.contains(&l.id) {
        ui.horizontal(|ui| {
            ui.spinner();
            dim(
                ui,
                if l.is_free() {
                    "Adding it to your assets…"
                } else {
                    "Finish paying on the checkout page in your browser: it downloads here by itself."
                },
            );
        });
    } else {
        let (icon, label) = if l.is_free() {
            ("ArrowDown", "Get · Free".to_string())
        } else {
            ("Store1", format!("Buy · {}", l.price_label()))
        };
        if primary_icon_button(ui, icon, &label).clicked() {
            if !l.is_free() && !s.market.site.is_signed_in() {
                s.market.site.signing_in = true;
                s.ed.toast("Sign in with your Skyfold account to buy this");
            } else {
                s.market.site.get(l);
            }
        }
        ui.add_space(4.0);
        hint_small(
            ui,
            "It lands in Objects › Imported (scenes in Objects › Scenes), ready to place.",
        );
    }
    ui.add_space(6.0);
}

// ------------------------------------------------------------------ my library

fn library(ui: &mut Ui, s: &mut UiState, m: &Market) {
    // where the packs come from, Refresh and Restore purchases
    ui.horizontal(|ui| {
        let line = match (&m.source, m.checked_at) {
            (super::backend::Source::Bundled, _) => "Built-in catalog".to_string(),
            (_, 0) => "Marketplace · not reached yet".to_string(),
            (_, at) if m.signed_in => format!("Marketplace · checked {}", ago(at)),
            (_, at) => format!("Marketplace · not signed in · checked {}", ago(at)),
        };
        ui.label(egui::RichText::new(line).small().color(TEXT_DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            crate::ui::tips::help_button(ui, "asset_packs");
            if s.market.busy {
                ui.spinner();
            } else if small_icon_button(ui, "ArrowRotateClockwise", "Refresh")
                .on_hover_text("Check the marketplace again for prices and the packs you own")
                .clicked()
            {
                s.market.refresh = true;
            }
            if m.packs().iter().any(|p| p.listing_id.is_some()) {
                let signed = s.market.site.is_signed_in();
                if small_icon_button(ui, "ArrowDown", "Restore purchases")
                    .on_hover_text("Find the packs your Skyfold account bought (or made) and install any missing here")
                    .clicked()
                {
                    if signed {
                        s.market.site.restore(m.packs().to_vec());
                    } else {
                        s.market.site.signing_in = true;
                    }
                }
            }
        });
    });
    if let Some(e) = &m.error {
        warn_line(ui, e);
    }
    ui.add_space(10.0);
    let mut open = None;
    egui::ScrollArea::vertical()
        .id_salt("market_library")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 6.0);
            sign_in_box(ui, s);
            tips(ui);
            let (mine, rest): (Vec<&Pack>, Vec<&Pack>) = m
                .packs()
                .iter()
                .filter(|p| !p.unlisted || m.state(p) != PackState::Locked)
                .partition(|p| m.state(p) != PackState::Locked);
            let w = ui.available_width().min(760.0);
            for (title, list) in [("Yours to use", mine), ("Not yet yours", rest)] {
                if list.is_empty() {
                    continue;
                }
                group_title(ui, title, &list.len().to_string());
                for p in list {
                    ui.allocate_ui(vec2(w, 0.0), |ui| {
                        ui.set_width(w);
                        if pack_row(ui, s, m, p) {
                            open = Some(Item::Pack(p.id.clone()));
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            if p.listed() && m.state(p) != PackState::Locked {
                                if small_icon_button(ui, "LayoutGrid1", &format!("Browse its {}", assets_label(p.assets()))).clicked() {
                                    browse_assets(s, p);
                                }
                            } else if m.state(p) == PackState::Locked
                                && small_icon_button(ui, "Store1", &format!("Get pack · {}", p.price_label())).clicked()
                            {
                                get_pack(s, p);
                            }
                            if m.owned.contains(&p.id)
                                && p.installed == 0
                                && p.listing_id.is_some()
                                && small_icon_button(ui, "ArrowDown", "Download pack").clicked()
                            {
                                open = Some(Item::Pack(p.id.clone()));
                            }
                        });
                    });
                    ui.add_space(16.0);
                }
                ui.add_space(8.0);
            }
            if super::developer_unlock_available() {
                developer(ui, m);
            }
        });
    if let Some(item) = open {
        s.market.view = View::Browse;
        s.market.detail = Some(item);
        s.market.shot = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_lose_their_glyph_and_descriptions_their_tail() {
        assert_eq!(plain("\u{e182} Owned"), "Owned");
        assert_eq!(plain("Free"), "Free");
        assert_eq!(plain("\u{e2d6} $49.99"), "$49.99");
        assert_eq!(plain("€12.00"), "€12.00");
        assert_eq!(first_paragraph("One\nline.\n\nNext"), "One line.");
        assert_eq!(license("custom"), "Custom license");
        assert_eq!(license("CC0"), "CC0");
        assert_eq!((assets_label(1).as_str(), assets_label(585).as_str()), ("1 asset", "585 assets"));
    }

    #[test]
    fn sorts_are_the_websites() {
        let ids: Vec<&str> = Sort::ALL.iter().map(|s| s.id()).collect();
        assert_eq!(ids, ["popular", "new", "rated", "price_asc"]);
    }
}
