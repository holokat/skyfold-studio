//! The market in the editor: the Asset packs window (packs by style: owned ✓, free or price,
//! Refresh; the developer unlock folded away), a pack's card (what's in it, its free samples to
//! try, "Get pack"), the pack chips under the Objects tab's Style switch and the lock / "free"
//! badges on asset tiles.

use bevy::prelude::*;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Rect, RichText, Stroke, vec2,
};
use egui_phosphor::regular as icon;

use super::{Access, Dev, Market, Pack, PackState};
use crate::editor::{ObjectTool, Tab};
use crate::thumbs::Subject;
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// The market's windows and the Objects tab's pack filter (kept while the editor runs).
#[derive(Resource)]
pub struct MarketUi {
    /// Objects tab, second tier: single assets or whole packs
    pub browse: Browse,
    /// Objects tab: free, paid or every asset / pack
    pub price: PriceFilter,
    /// the Asset packs window
    pub open: bool,
    /// the pack whose card is showing
    pub card: Option<String>,
    /// Objects tab: only this pack's assets (empty: every pack of the chosen style)
    pub filter: String,
    /// Refresh was pressed (handled by `market::refresh`)
    pub refresh: bool,
    /// a refresh is running
    pub(super) busy: bool,
    /// skyfoldstudio.com: community listings, sign-in, downloads
    pub site: super::site::SiteUi,
}

impl Default for MarketUi {
    /// (`WB_MARKET=packs` opens Asset packs, `WB_MARKET=card:<pack id>` a pack's card,
    /// `WB_MARKET=filter:<pack id>` picks a pack, `browse:packs` the Packs tab (`browse:signin`: with
    /// the website sign-in card open) and
    /// `price:free|paid` the price filter, for screenshot tests)
    fn default() -> Self {
        let env = std::env::var("WB_MARKET").unwrap_or_default();
        let filter = env.strip_prefix("filter:").unwrap_or_default().to_string();
        let browse = match env.as_str() {
            "browse:packs" | "browse:signin" => Browse::Packs,
            "browse:scenes" => Browse::Scenes,
            _ => Browse::Assets,
        };
        let price = match env.as_str() {
            "price:free" => PriceFilter::Free,
            "price:paid" => PriceFilter::Paid,
            _ => PriceFilter::All,
        };
        Self {
            browse,
            price,
            open: env == "packs",
            card: env.strip_prefix("card:").map(str::to_string),
            filter,
            refresh: false,
            busy: false,
            site: super::site::SiteUi {
                signing_in: env == "browse:signin",
                ..Default::default()
            },
        }
    }
}

/// The Objects tab's second tier: browse single assets, or whole packs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Browse {
    #[default]
    Assets,
    Packs,
    /// the user's saved scenes (crate::scenes) and the asset packs' ready-made scenes
    Scenes,
}

/// Free (usable without buying: free packs, built-ins, a paid pack's samples) or paid (needs a
/// pack, owned or not).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PriceFilter {
    #[default]
    All,
    Free,
    Paid,
}

impl PriceFilter {
    pub fn keeps_asset(self, m: &Market, asset: &str) -> bool {
        match self {
            PriceFilter::All => true,
            PriceFilter::Free => !m.is_paid(asset),
            PriceFilter::Paid => m.is_paid(asset),
        }
    }

    pub fn keeps_pack(self, p: &Pack) -> bool {
        match self {
            PriceFilter::All => true,
            PriceFilter::Free => p.is_free(),
            PriceFilter::Paid => !p.is_free(),
        }
    }
}

const GOLD: Color32 = Color32::from_rgb(255, 214, 120);
const GREEN: Color32 = Color32::from_rgb(74, 196, 120);

/// "✓ Owned", "Free" or "🔒 $14.99", and its colour.
fn status(m: &Market, p: &Pack) -> (String, Color32) {
    match m.state(p) {
        PackState::Owned if m.dev() != Dev::Off && !m.owned.contains(&p.id) => {
            (format!("{} Unlocked", icon::LOCK_SIMPLE_OPEN), GREEN)
        }
        PackState::Owned => (format!("{} Owned", icon::CHECK), GREEN),
        PackState::Free => ("Free".into(), GREEN),
        PackState::Locked => (format!("{} {}", icon::LOCK_SIMPLE, p.price_label()), GOLD),
    }
}

/// "76 assets · 10 free to try" / "76 assets".
fn contents(m: &Market, p: &Pack) -> String {
    let n = p.assets();
    let samples = p.free_assets.len();
    if m.state(p) == PackState::Locked && samples > 0 {
        format!("{n} assets · {samples} free to try")
    } else {
        format!("{n} assets")
    }
}

/// The asset that stands for a pack here: its hero, else a sample, else any of its models.
fn hero(s: &UiState, m: &Market, p: &Pack) -> Option<String> {
    let lib = s.lib.as_deref()?;
    p.hero_asset
        .iter()
        .chain(p.free_assets.iter())
        .find(|a| lib.get(a).is_some())
        .cloned()
        .or_else(|| {
            let i = m.pack_index(&p.id)?;
            lib.entries
                .iter()
                .find(|e| m.pack_index_of(&e.id) == Some(i))
                .map(|e| e.id.clone())
        })
}

/// The lock badge (locked) or a "FREE" tag (a paid pack's free sample) over an asset tile.
pub fn badge(ui: &egui::Ui, rect: Rect, a: Access) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    match a {
        Access::Locked(_) => {
            let c = rect.right_top() + vec2(-14.0, 14.0);
            p.circle_filled(c, 10.5, Color32::from_black_alpha(200));
            p.circle_stroke(
                c,
                10.5,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 214, 120, 120)),
            );
            p.text(
                c,
                Align2::CENTER_CENTER,
                icon::LOCK_SIMPLE,
                FontId::proportional(12.5),
                GOLD,
            );
        }
        Access::Sample(_) => {
            let r = Rect::from_min_size(rect.left_top() + vec2(6.0, 6.0), vec2(34.0, 15.0));
            p.rect_filled(
                r,
                CornerRadius::same(7),
                Color32::from_rgba_unmultiplied(28, 128, 72, 230),
            );
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                "FREE",
                FontId::new(9.0, semibold()),
                Color32::WHITE,
            );
        }
        _ => {}
    }
}

/// Hover over a locked tile: its name, pack and price.
pub fn locked_hover(resp: egui::Response, name: &str, p: &Pack) -> egui::Response {
    resp.on_hover_ui(|ui| {
        ui.label(RichText::new(name).strong());
        ui.label(
            RichText::new(format!(
                "{}  {} · {}",
                icon::LOCK_SIMPLE,
                p.name,
                p.price_label()
            ))
            .color(GOLD),
        );
        ui.label(
            RichText::new("Click to see the pack")
                .small()
                .color(TEXT_DIM),
        );
    })
}

/// The Objects tab's pack filter as an index into `m.packs()` (None: every pack).
pub fn filter(s: &UiState, m: &Market) -> Option<usize> {
    if s.market.filter.is_empty() {
        None
    } else {
        m.pack_index(&s.market.filter)
    }
}

/// Objects › Asset packs: every pack as a card (by style), free / paid filtered; click one for its
/// card, or browse its assets.
pub fn packs_browser(ui: &mut egui::Ui, s: &mut UiState) {
    let m = super::snapshot();
    let price = s.market.price;
    let mut any = false;
    for style in m.styles() {
        let packs: Vec<&Pack> = m
            .packs()
            .iter()
            .filter(|p| p.style == style && price.keeps_pack(p))
            .collect();
        if packs.is_empty() {
            continue;
        }
        any = true;
        section(ui, &crate::objects::pack_label(&style));
        ui.add_space(2.0);
        for p in packs {
            if pack_row(ui, s, &m, p) {
                s.market.card = Some(p.id.clone());
            }
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                if ui
                    .small_button(format!(
                        "{}  Browse its {} assets",
                        icon::SQUARES_FOUR,
                        p.assets()
                    ))
                    .clicked()
                {
                    s.market.filter = p.id.clone();
                    s.market.browse = Browse::Assets;
                    s.place.style = p.style.clone();
                    s.place.group.clear();
                }
                if m.state(p) == PackState::Locked
                    && ui
                        .small_button(format!(
                            "{}  Get pack — {}",
                            icon::SHOPPING_CART,
                            p.price_label()
                        ))
                        .clicked()
                {
                    get_pack(s, p);
                }
            });
            ui.add_space(8.0);
        }
    }
    if !any {
        dim(
            ui,
            match price {
                PriceFilter::Free => "No free packs.",
                PriceFilter::Paid => "No paid packs.",
                PriceFilter::All => "No packs yet.",
            },
        );
    }
    ui.add_space(6.0);
    community(ui, s);
}

/// The pack picker under the Style switch ("Pack: All packs ▾", each with its status): narrows
/// the tiles to one pack; a locked pack picked shows how to get it. A drop-down, so it stays
/// compact however many packs there are.
pub fn pack_chips(ui: &mut egui::Ui, s: &mut UiState) {
    let m = super::snapshot();
    if m.any_locked() && !s.market.open {
        tips(ui);
    }
    let style = s.place.style.clone();
    let price = s.market.price;
    let packs: Vec<&Pack> = m
        .packs()
        .iter()
        .filter(|p| (style.is_empty() || p.style == style) && p.listed() && price.keeps_pack(p))
        .collect();
    if packs.is_empty() {
        s.market.filter.clear();
        return;
    }
    if !packs.iter().any(|p| p.id == s.market.filter) {
        s.market.filter.clear();
    }
    let current = m
        .pack(&s.market.filter)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "All packs".into());
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{}  Pack", icon::PACKAGE))
                .small()
                .color(TEXT_DIM),
        );
        egui::ComboBox::from_id_salt("objects_pack")
            .selected_text(current)
            .width(ui.available_width() - 8.0)
            .dropdown_ui(ui, |ui| {
                if menu_check(ui, s.market.filter.is_empty(), "All packs").clicked() {
                    pick = Some(String::new());
                }
                for p in &packs {
                    let (st, col) = status(&m, p);
                    let mut job = egui::text::LayoutJob::default();
                    job.append(
                        &p.name,
                        0.0,
                        egui::TextFormat::simple(FontId::proportional(13.0), TEXT),
                    );
                    job.append(
                        &format!("   {st}"),
                        0.0,
                        egui::TextFormat::simple(FontId::proportional(12.0), col),
                    );
                    let r = menu_check(ui, s.market.filter == p.id, job);
                    if r.on_hover_text(format!(
                        "{} · by {} · {}",
                        crate::objects::pack_label(&p.style),
                        p.publisher,
                        contents(&m, p)
                    ))
                    .clicked()
                    {
                        pick = Some(p.id.clone());
                    }
                }
            });
    });
    if let Some(f) = pick {
        s.market.filter = f;
    }
    // a locked pack picked: what's free and how to get the rest
    if let Some(p) = m.pack(&s.market.filter)
        && m.state(p) == PackState::Locked
    {
        ui.add_space(4.0);
        Frame::new()
            .fill(Color32::from_rgba_unmultiplied(255, 214, 120, 18))
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 214, 120, 70),
            ))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}  {}", icon::LOCK_SIMPLE, contents(&m, p)))
                            .small()
                            .color(GOLD),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("See pack").clicked() {
                            s.market.card = Some(p.id.clone());
                        }
                    });
                });
            });
    }
    ui.add_space(2.0);
}

/// Asset packs and pack cards, and the market's messages as toasts (every frame).
pub fn windows(ctx: &egui::Context, s: &mut UiState, out: &mut crate::ui::UiOut) {
    if let Some(msg) = super::take_notice() {
        s.ed.toast(msg);
    }
    site_inbox(ctx, s, out);
    if s.market.open {
        packs_window(ctx, s);
    }
    if s.market.card.is_some() {
        card_window(ctx, s);
    }
}

/// The one-time tips card about asset packs (Objects tab and Asset packs).
fn tips(ui: &mut egui::Ui) {
    crate::ui::tips::card(
        ui,
        "asset_packs",
        "Asset packs",
        &[
            "Assets come in packs, grouped by style: photoreal scans, hand-painted fantasy and more to come.",
            "Free packs are everyone's. A paid pack lets you try some of its assets for free (a FREE tag); the rest show a lock until you get it.",
            "Click a locked tile, or Asset packs, to see what's in a pack. Worlds that use a pack you don't have still open, and what's there can be moved or deleted.",
        ],
    );
}

/// When the market was last checked, for people ("2 min ago").
fn ago(at: u64) -> String {
    let d = super::backend::now().saturating_sub(at);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86400),
    }
}

fn packs_window(ctx: &egui::Context, s: &mut UiState) {
    let m = super::snapshot();
    // Esc closes it (a pack card over it first)
    let mut open = !(s.market.card.is_none()
        && !ctx.egui_wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(egui::Key::Escape)));
    let h = (ctx.content_rect().height() - crate::ui::TOP_H - 120.0).clamp(300.0, 760.0);
    egui::Window::new(
        RichText::new(format!("{}  Asset packs", icon::PACKAGE))
            .font(FontId::new(16.0, semibold())),
    )
    .id(egui::Id::new("asset_packs"))
    .collapsible(false)
    .resizable(false)
    .fixed_size(vec2(520.0, h))
    .anchor(Align2::CENTER_CENTER, vec2(0.0, crate::ui::TOP_H * 0.5))
    .open(&mut open)
    .show(ctx, |ui| {
        tips(ui);
        // where the packs come from, and Refresh
        ui.horizontal(|ui| {
            let line = match (&m.source, m.checked_at) {
                (super::backend::Source::Bundled, _) => {
                    "Built-in catalog · purchases open soon".to_string()
                }
                (_, 0) => "Marketplace · not reached yet".to_string(),
                (_, at) if m.signed_in => format!("Marketplace · checked {}", ago(at)),
                (_, at) => format!("Marketplace · not signed in · checked {}", ago(at)),
            };
            ui.label(RichText::new(line).small().color(TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                crate::ui::tips::help_button(ui, "asset_packs");
                if s.market.busy {
                    ui.add(egui::Spinner::new().size(14.0));
                } else if ui
                    .button(format!("{} Refresh", icon::ARROWS_CLOCKWISE))
                    .on_hover_text("Check the marketplace again for prices and the packs you own")
                    .clicked()
                {
                    s.market.refresh = true;
                }
                if m.packs().iter().any(|p| p.listing_id.is_some())
                    && s.market.site.is_signed_in()
                    && ui
                        .button("Restore purchases")
                        .on_hover_text("Find the packs your Skyfold account bought and install any missing here")
                        .clicked()
                {
                    s.market.site.restore(m.packs().to_vec());
                }
            });
        });
        if let Some(e) = &m.error {
            ui.label(
                RichText::new(format!("{}  {e}", icon::WARNING))
                    .small()
                    .color(DANGER),
            );
        }
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("asset_packs_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for style in m.styles() {
                    section(ui, &crate::objects::pack_label(&style));
                    ui.add_space(2.0);
                    for p in m.packs().iter().filter(|p| p.style == style) {
                        if pack_row(ui, s, &m, p) {
                            s.market.card = Some(p.id.clone());
                        }
                        ui.add_space(6.0);
                    }
                }
                if super::developer_unlock_available() {
                    ui.add_space(6.0);
                    developer(ui, &m);
                }
            });
    });
    if !open {
        s.market.open = false;
    }
}

/// One pack in Asset packs: its picture, name, publisher, contents and status. True if clicked.
fn pack_row(ui: &mut egui::Ui, s: &mut UiState, m: &Market, p: &Pack) -> bool {
    let hero = hero(s, m, p);
    let (st, col) = status(m, p);
    let r = Frame::new()
        .fill(CARD_BG)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (img, _) = ui.allocate_exact_size(vec2(84.0, 64.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(img, CornerRadius::same(7), Color32::from_black_alpha(60));
                if let Some(t) = hero
                    .as_ref()
                    .and_then(|h| s.previews.thumb(&Subject::Asset(h.clone())))
                {
                    egui::Image::new((t, img.size()))
                        .corner_radius(CornerRadius::same(7))
                        .paint_at(ui, img);
                } else {
                    ui.painter().text(
                        img.center(),
                        Align2::CENTER_CENTER,
                        icon::PACKAGE,
                        FontId::proportional(22.0),
                        TEXT_DIM,
                    );
                }
                ui.vertical(|ui| {
                    // (the name wraps on its own line: long names never run into the status)
                    ui.add(
                        egui::Label::new(
                            RichText::new(&p.name)
                                .font(FontId::new(14.0, semibold()))
                                .color(Color32::WHITE),
                        )
                        .wrap(),
                    );
                    ui.label(RichText::new(st).small().strong().color(col));
                    let by = if p.publisher.is_empty() {
                        String::new()
                    } else {
                        format!("by {} · ", p.publisher)
                    };
                    ui.label(
                        RichText::new(format!("{by}{}", contents(m, p)))
                            .small()
                            .color(TEXT_DIM),
                    );
                    if !p.description.is_empty() {
                        let mut job = egui::text::LayoutJob::simple(
                            p.description.clone(),
                            FontId::proportional(11.5),
                            TEXT,
                            ui.available_width(),
                        );
                        job.wrap.max_rows = 2;
                        job.wrap.overflow_character = Some('…');
                        ui.label(job);
                    }
                });
            });
        });
    let resp = r
        .response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("See what's in it");
    if resp.hovered() {
        ui.painter().rect_stroke(
            resp.rect,
            CornerRadius::same(10),
            Stroke::new(1.0, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    resp.clicked()
}

/// The developer unlock, folded away: every pack usable on this computer (the studio's own
/// builds). Not reachable over MCP.
fn developer(ui: &mut egui::Ui, m: &Market) {
    egui::CollapsingHeader::new(RichText::new(format!("{}  Developer", icon::WRENCH)).small().color(TEXT_DIM)).id_salt("market_dev").default_open(false).show(ui, |ui| {
        if m.dev() == Dev::Env {
            dim(ui, "Every pack is unlocked for this run (SKYFOLD_MARKET_DEV=owner).");
            return;
        }
        let mut on = m.dev() == Dev::Prefs;
        if toggle(ui, &mut on, "Unlock all packs (this computer only)").changed() {
            super::set_dev_unlock(on);
        }
        dim(ui, "For the studio's own builds: every pack works here whatever this account owns. Agents can't change this.");
    });
}

/// A pack's card: what it is, its hero turning, its free samples to try, and "Get pack".
fn card_window(ctx: &egui::Context, s: &mut UiState) {
    let m = super::snapshot();
    let Some(p) = s.market.card.as_deref().and_then(|id| m.pack(id)) else {
        s.market.card = None;
        return;
    };
    let state = m.state(p);
    let hero = hero(s, &m, p);
    let mut open = true;
    let mut close =
        ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !ctx.egui_wants_keyboard_input();
    let mut place: Option<String> = None;
    egui::Window::new(
        RichText::new(format!("{}  Asset pack", icon::PACKAGE)).font(FontId::new(16.0, semibold())),
    )
    .id(egui::Id::new("pack_card"))
    .order(egui::Order::Foreground)
    .collapsible(false)
    .resizable(false)
    .fixed_size(vec2(420.0, 0.0))
    .anchor(Align2::CENTER_CENTER, vec2(0.0, crate::ui::TOP_H * 0.5))
    .open(&mut open)
    .show(ctx, |ui| {
        ui.set_width(420.0);
        let by = if p.publisher.is_empty() {
            String::new()
        } else {
            format!(" · by {}", p.publisher)
        };
        let lines = [
            format!("{}{by}", crate::objects::pack_label(&p.style)),
            contents(&m, p),
        ];
        match &hero {
            Some(h) => crate::thumbs::ui::card(
                ui,
                &mut s.previews,
                &Subject::Asset(h.clone()),
                &p.name,
                &lines,
            ),
            None => {
                title(ui, &p.name);
                lines.iter().for_each(|l| dim(ui, l));
            }
        }
        if !p.description.is_empty() {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(&p.description).color(TEXT)).wrap());
        }
        // its free samples, to place right away
        let lib = s.lib.as_deref();
        let samples: Vec<(String, String)> = p
            .free_assets
            .iter()
            .filter_map(|a| {
                lib.and_then(|l| l.get(a))
                    .map(|e| (e.id.clone(), e.name.clone()))
            })
            .collect();
        if state == PackState::Locked && !samples.is_empty() {
            section(ui, "Free to try");
            dim(ui, "Click one to place it.");
            ui.add_space(2.0);
            let w = ((ui.available_width() - 4.0 * 6.0) / 5.0).floor();
            for row in samples.chunks(5) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (id, name) in row {
                        if crate::thumbs::ui::tile(
                            ui,
                            &mut s.previews,
                            &Subject::Asset(id.clone()),
                            icon::BASKET,
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
        }
        ui.add_space(8.0);
        let working = s.market.site.working.contains(&p.id);
        match state {
            // bought, but its files aren't on this computer (a new computer, a reinstall)
            PackState::Owned if m.owned.contains(&p.id) && p.installed == 0 => {
                ui.label(
                    RichText::new(format!("{}  Owned: download it to use it here.", icon::CHECK_CIRCLE))
                        .color(GREEN),
                );
                ui.add_space(2.0);
                if working {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        dim(ui, "Downloading and installing…");
                    });
                } else if p.listing_id.is_some()
                    && primary_button(ui, &format!("{}  Download pack", icon::DOWNLOAD_SIMPLE)).clicked()
                {
                    if s.market.site.is_signed_in() {
                        s.market.site.reinstall(p);
                    } else {
                        s.market.browse = Browse::Packs;
                        s.market.site.signing_in = true;
                        close = true;
                        s.ed.toast("Sign in with the Skyfold account that bought it, then press Download pack");
                    }
                }
            }
            PackState::Owned => {
                ui.label(
                    RichText::new(format!(
                        "{}  {}",
                        icon::CHECK_CIRCLE,
                        if m.owned.contains(&p.id) {
                            "Owned: every asset in it is yours to place."
                        } else {
                            "Unlocked on this computer (developer)."
                        }
                    ))
                    .color(GREEN),
                );
            }
            PackState::Free => {
                ui.label(
                    RichText::new(format!("{}  Free for everyone.", icon::CHECK_CIRCLE))
                        .color(GREEN),
                );
            }
            PackState::Locked if p.unlisted => dim(ui, "This pack isn't for sale yet."),
            PackState::Locked if working => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    dim(ui, "Finish paying on the checkout page in your browser: it installs here by itself.");
                });
            }
            PackState::Locked => {
                if primary_button(
                    ui,
                    &format!("{}  Get pack — {}", icon::SHOPPING_BAG, p.price_label()),
                )
                .clicked()
                {
                    get_pack(s, p);
                }
                ui.add_space(2.0);
                dim(ui, "Bought it already? Restore purchases in Asset packs.");
            }
        }
    });
    if let Some(id) = place {
        start_placing(s, &id);
        close = true;
    }
    if !open || close {
        s.market.card = None;
    }
}

/// "Get pack": the pack's listing on skyfoldstudio.com (Checkout in the browser, then it
/// installs here), else its marketplace page in the browser, or (no store yet) a note.
fn get_pack(s: &mut UiState, p: &Pack) {
    if p.listing_id.is_some() {
        if !s.market.site.is_signed_in() {
            // the sign-in card is in Objects › Packs
            s.market.browse = Browse::Packs;
            s.market.site.signing_in = true;
            s.market.card = None;
            s.market.open = false;
            s.ed.toast(format!(
                "Sign in with your Skyfold account to buy {}, then press Get pack again",
                p.name
            ));
        } else {
            s.market.site.buy_pack(p);
        }
        return;
    }
    match p.purchase_url.as_deref() {
        Some(url) if open_url(url) => s.ed.toast(format!(
            "Opening {} in your browser — come back and Refresh once it's yours",
            p.name
        )),
        Some(_) => s.ed.toast("Couldn't open the marketplace page"),
        None => s.ed.toast("Purchases open soon"),
    }
}

/// Opens an http(s) address in the browser (never during automated runs).
pub(crate) fn open_url(url: &str) -> bool {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return false;
    }
    if std::env::var("WB_SHOT").is_ok() {
        info!("MARKET would open {url}");
        return true;
    }
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.is_ok()
}

/// Starts placing a (free sample) asset: Objects › Place, its style shown.
fn start_placing(s: &mut UiState, id: &str) {
    let Some(e) = s.lib.as_deref().and_then(|l| l.get(id)) else {
        return;
    };
    let (cat, style) = (e.category, e.pack.clone());
    s.ed.tab = Tab::Objects;
    s.ed.object_tool = ObjectTool::Place;
    s.place.category = cat;
    s.place.style = style;
    s.place.selected = Some(id.to_string());
    s.sets.selected = None;
    s.market.open = false;
    s.ed.toast("Click the ground to place it");
}

/// What the website's background jobs reported: listings, covers, sign-in, downloads (imported
/// like File › Import) and problems (as toasts).
fn site_inbox(ctx: &egui::Context, s: &mut UiState, out: &mut crate::ui::UiOut) {
    let events: Vec<super::site::Event> = std::mem::take(
        &mut *s
            .market
            .site
            .inbox
            .lock()
            .unwrap_or_else(|p| p.into_inner()),
    );
    use super::site::Event;
    for e in events {
        let site = &mut s.market.site;
        match e {
            Event::Listings { listings, next, append } => {
                if !append {
                    site.listings.clear();
                }
                site.listings.extend(listings);
                site.next = next;
                site.loaded = true;
            }
            Event::Cover { id, rgba, size } => {
                let img = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
                let tex = ctx.load_texture(format!("listing-{id}"), img, egui::TextureOptions::LINEAR);
                site.covers.insert(id, tex);
            }
            Event::Failed(msg) => {
                site.error = Some(msg.clone());
                s.ed.toast(msg);
            }
            Event::Note(msg) => s.ed.toast(msg),
            Event::Checkout { title } => s.ed.toast(format!("Finish buying {title} on the checkout page in your browser — it downloads here once it's paid")),
            Event::Owned { pack } => {
                // (the snapshot reads the recorded purchase on its next refresh: now)
                s.market.refresh = true;
                let _ = pack;
            }
            Event::PackInstalled { pack, title } => {
                out.actions.write(crate::ui::UiAction::PackInstalled { pack, title });
            }
            Event::SignedIn(Ok(())) => {
                site.signed_in = Some(true);
                site.signing_in = false;
                site.token_input.clear();
                s.ed.toast("Signed in to skyfoldstudio.com");
            }
            Event::SignedIn(Err(msg)) => s.ed.toast(msg),
            Event::Downloaded { title, files } => {
                out.actions.write(crate::ui::UiAction::ImportFiles { files, from: title });
            }
            Event::SceneAdded { title, id } => {
                s.scenes.reload();
                s.market.browse = Browse::Scenes;
                s.ed.toast(format!("{title} is in your scenes (Objects › Scenes) — click it, then the ground, to place it"));
                let _ = id;
            }
            Event::Done(id) => {
                site.busy = site.busy.saturating_sub(1);
                if let Some(id) = id {
                    site.working.remove(&id);
                }
            }
        }
    }
    if s.market.site.busy > 0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Objects › Asset packs › Community: what creators publish on skyfoldstudio.com. Browsing
/// needs no account; buying (and, for now, the website's free listings) needs a sign-in.
fn community(ui: &mut egui::Ui, s: &mut UiState) {
    section(ui, "Community · skyfoldstudio.com");
    ui.add_space(2.0);
    if !s.market.site.loaded && s.market.site.busy == 0 && s.market.site.error.is_none() {
        s.market.site.load(false);
    }
    // search
    ui.horizontal(|ui| {
        let w = ui.available_width() - 30.0;
        let r = ui.add(
            egui::TextEdit::singleline(&mut s.market.site.query)
                .hint_text(format!("{}  Search listings", icon::MAGNIFYING_GLASS))
                .desired_width(w),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            s.market.site.load(false);
        }
        let busy = s.market.site.busy > 0;
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(icon::ARROWS_CLOCKWISE).min_size(vec2(24.0, 22.0)),
            )
            .on_hover_text("Refresh")
            .clicked()
        {
            s.market.site.error = None;
            s.market.site.load(false);
        }
    });
    ui.add_space(4.0);
    account_card(ui, s);
    ui.add_space(4.0);
    let price = s.market.price;
    let list: Vec<super::site::Listing> = s
        .market
        .site
        .listings
        .iter()
        .filter(|l| match price {
            PriceFilter::All => true,
            PriceFilter::Free => l.is_free(),
            PriceFilter::Paid => !l.is_free(),
        })
        .cloned()
        .collect();
    if list.is_empty() {
        let site = &s.market.site;
        if site.busy > 0 && !site.loaded {
            ui.horizontal(|ui| {
                ui.spinner();
                dim(ui, "Loading listings…");
            });
        } else if let Some(e) = site.error.clone() {
            dim(ui, &format!("Couldn't reach skyfoldstudio.com ({e})."));
        } else if !site.listings.is_empty() {
            dim(
                ui,
                if price == PriceFilter::Free {
                    "No free community listings."
                } else {
                    "No paid community listings."
                },
            );
        } else if !site.query.trim().is_empty() {
            dim(ui, "No listings match.");
        } else {
            dim(
                ui,
                "No community listings yet. Creators publish them in the Creator studio at skyfoldstudio.com/studio.",
            );
        }
        return;
    }
    for l in &list {
        listing_row(ui, s, l);
        ui.add_space(6.0);
    }
    if s.market.site.next.is_some()
        && ui
            .add_enabled(s.market.site.busy == 0, egui::Button::new("Show more"))
            .clicked()
    {
        s.market.site.load(true);
    }
}

/// Signed out: a line on what needs an account and a Sign in button (the card walks through
/// getting an API key). Signed in: who, and Sign out.
fn account_card(ui: &mut egui::Ui, s: &mut UiState) {
    if s.market.site.is_signed_in() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{}  Signed in to skyfoldstudio.com",
                    icon::USER_CIRCLE_CHECK
                ))
                .small()
                .color(GREEN),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Sign out").clicked() {
                    s.market.site.sign_out();
                }
            });
        });
        return;
    }
    let mut open_keys = false;
    Frame::new().fill(CARD_BG).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let site = &mut s.market.site;
        if !site.signing_in {
            ui.label(RichText::new("Browse freely. Sign in with your Skyfold account to buy listings and add community assets.").small().color(TEXT_DIM));
            ui.add_space(4.0);
            if ui.button(format!("{}  Sign in", icon::SIGN_IN)).clicked() {
                site.signing_in = true;
            }
            return;
        }
        ui.label(RichText::new("Sign in with an API key").strong());
        ui.add_space(2.0);
        ui.label(RichText::new("1. Open your account's API keys page and choose Create API key. Also tick Prepare paid purchases, so you can buy paid listings.").small().color(TEXT_DIM));
        if ui.small_button(format!("{}  Open API keys", icon::ARROW_SQUARE_OUT)).clicked() {
            open_keys = true;
        }
        ui.add_space(2.0);
        ui.label(RichText::new("2. Copy the key and paste it here:").small().color(TEXT_DIM));
        let r = ui.add(egui::TextEdit::singleline(&mut site.token_input).password(true).hint_text("API key").desired_width(ui.available_width()));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let ready = !site.token_input.trim().is_empty() && site.busy == 0;
            if ui.add_enabled(ready, egui::Button::new("Sign in")).clicked() || (enter && ready) {
                let key = site.token_input.clone();
                site.sign_in(key);
            }
            if ui.small_button("Cancel").clicked() {
                site.signing_in = false;
                site.token_input.clear();
            }
        });
        dim(ui, "The key stays on this computer. Agents can browse listings but never buy.");
    });
    if open_keys && !open_url(&super::site::keys_url()) {
        s.ed.toast("Couldn't open skyfoldstudio.com");
    }
}

/// A listing: cover, title, creator, price, and Get / Buy (and its page on the website).
fn listing_row(ui: &mut egui::Ui, s: &mut UiState, l: &super::site::Listing) {
    s.market.site.cover(l);
    let working = s.market.site.working.contains(&l.id);
    Frame::new()
        .fill(CARD_BG)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(tex) = s.market.site.covers.get(&l.id) {
                let w = ui.available_width();
                let [tw, th] = tex.size();
                let h = (w * th as f32 / tw.max(1) as f32).min(150.0);
                ui.add(
                    egui::Image::new(tex)
                        .fit_to_exact_size(vec2(w, h))
                        .corner_radius(CornerRadius::same(6)),
                );
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(&l.title).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(l.price_label()).color(if l.is_free() {
                        GREEN
                    } else {
                        GOLD
                    }));
                });
            });
            let mut meta = vec![];
            if !l.by().is_empty() {
                meta.push(if l.official() {
                    format!("by {} {} Official", l.by(), icon::SEAL_CHECK)
                } else {
                    format!("by {}", l.by())
                });
            }
            meta.push(if l.kind == "pack" {
                "Pack".into()
            } else {
                "Asset".into()
            });
            if l.rating_count > 0 {
                meta.push(format!("★ {:.1} ({})", l.rating_average, l.rating_count));
            }
            if !l.license.is_empty() {
                meta.push(l.license.clone());
            }
            ui.label(RichText::new(meta.join(" · ")).small().color(TEXT_DIM));
            if !l.description.is_empty() {
                let d: String = l.description.chars().take(140).collect();
                ui.label(
                    RichText::new(if l.description.chars().count() > 140 {
                        format!("{d}…")
                    } else {
                        d
                    })
                    .small()
                    .color(TEXT_DIM),
                );
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if working {
                    ui.spinner();
                    ui.label(
                        RichText::new(if l.is_free() {
                            "Adding…"
                        } else {
                            "Waiting for checkout…"
                        })
                        .small()
                        .color(TEXT_DIM),
                    );
                } else {
                    let label = if l.is_free() {
                        format!("{}  Get — Free", icon::DOWNLOAD_SIMPLE)
                    } else {
                        format!("{}  Buy — {}", icon::SHOPPING_CART, l.price_label())
                    };
                    if ui.button(label).clicked() {
                        if !l.is_free() && !s.market.site.is_signed_in() {
                            s.market.site.signing_in = true;
                            s.ed.toast("Sign in with your Skyfold account to buy this");
                        } else {
                            s.market.site.get(l);
                        }
                    }
                }
                if ui
                    .small_button(format!("{}  Page", icon::ARROW_SQUARE_OUT))
                    .on_hover_text("Open its page on skyfoldstudio.com")
                    .clicked()
                {
                    open_url(&l.page_url(&super::site::base()));
                }
            });
        });
}
