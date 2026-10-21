//! Visual theme (Halo): translucent dark glass islands, white as the only accent, SF Compact
//! typography with Geist Mono / SF Mono for numbers (falls back to egui defaults) and Phosphor icons.
//! Colour carries meaning only: OK green, warning amber, danger red, plus data colours.

use std::sync::Arc;

use bevy_egui::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Margin,
    Stroke, TextStyle, Visuals, epaint::Shadow, epaint::text::VariationCoords, style::HandleShape,
};

// surfaces
/// Panels, rail, top islands, dock (~0.56).
pub const GLASS: Color32 = Color32::from_rgba_unmultiplied_const(16, 19, 25, 143);
/// Menus, palette, windows (~0.94).
pub const GLASS_SOLID: Color32 = Color32::from_rgba_unmultiplied_const(18, 21, 27, 240);
/// 1 px rim / separators (0.10).
pub const LINE: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 26);
/// Menu / window rim, keycap border (0.16).
pub const LINE_STRONG: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 41);
/// Tile / button fill (0.08).
pub const SOFT: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 20);
/// Hover, segmented "on", selected list row (0.14).
pub const SOFT_HOVER: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 36);
/// Inputs, dropdowns, segmented track (black 0.24).
pub const WELL: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 61);
/// Toggle track when off, slider rail rest (0.16).
pub const TRACK: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 41);
/// Boxed sections ("Generate a world", "Erosion"): black 0.16.
pub const BOX: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 41);
/// Inner top highlight of glass islands (0.06).
pub const HIGHLIGHT: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 15);
/// A white (primary) fill while hovered.
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(233, 237, 243);

// text
pub const TEXT: Color32 = Color32::from_rgb(244, 246, 249);
/// 0.66
pub const TEXT_DIM: Color32 = Color32::from_rgba_unmultiplied_const(236, 240, 246, 168);
/// 0.42
pub const TEXT_MUTE: Color32 = Color32::from_rgba_unmultiplied_const(236, 240, 246, 107);

// accent = white
pub const ACCENT: Color32 = Color32::WHITE;
/// Text / icon on a white fill.
pub const ON_ACCENT: Color32 = Color32::from_rgb(10, 12, 16);

// semantic
pub const OK: Color32 = Color32::from_rgb(95, 220, 154);
pub const WARN: Color32 = Color32::from_rgb(242, 193, 78);
pub const DANGER: Color32 = Color32::from_rgb(255, 123, 114);

// Pre-Halo names, kept so older call sites keep compiling while the tabs move over.
pub const ACCENT_SOFT: Color32 = SOFT_HOVER;
pub const CARD_BG: Color32 = SOFT;
pub const CARD_HOVER: Color32 = SOFT_HOVER;
pub const BORDER: Color32 = LINE;

// radii
pub const R_PANEL: u8 = 16;
pub const R_RAIL: u8 = 19;
pub const R_TILE: u8 = 10;
pub const R_BUTTON: u8 = 9;
pub const R_INPUT: u8 = 9;
pub const R_INNER: u8 = 7;
pub const R_SMALL: u8 = 7;
pub const R_WINDOW: u8 = 20;
pub const R_MENU: u8 = 12;

pub const SEMIBOLD: &str = "semibold";
pub const MEDIUM: &str = "medium";
pub const MONO_SEMIBOLD: &str = "mono-semibold";

pub fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

/// Medium weight (list-row titles).
pub fn medium() -> FontFamily {
    FontFamily::Name(MEDIUM.into())
}

/// Numbers, values, keycaps (Geist Mono / SF Mono at medium weight).
pub fn mono() -> FontFamily {
    FontFamily::Monospace
}

/// Semibold mono (the time readout).
pub fn mono_semibold() -> FontFamily {
    FontFamily::Name(MONO_SEMIBOLD.into())
}

fn load(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// The mono face: Geist Mono when installed, else SF Mono (both variable: the weight is set per family).
fn mono_bytes() -> Option<Vec<u8>> {
    let home = std::env::var("HOME").unwrap_or_default();
    load(&format!(
        "{home}/Library/Fonts/GeistMono-VariableFont_wght.ttf"
    ))
    .or_else(|| load("/Library/Fonts/GeistMono-VariableFont_wght.ttf"))
    .or_else(|| load("/System/Library/Fonts/SFNSMono.ttf"))
}

fn weighted(bytes: Vec<u8>, wght: f32) -> Arc<FontData> {
    let tweak = FontTweak {
        coords: VariationCoords::new([(b"wght", wght)]),
        ..Default::default()
    };
    Arc::new(FontData::from_owned(bytes).tweak(tweak))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let regular = load("/Library/Fonts/SF-Compact-Display-Regular.otf");
    let semi = load("/Library/Fonts/SF-Compact-Display-Semibold.otf")
        .or_else(|| load("/Library/Fonts/SF-Compact-Display-Medium.otf"));
    let med = load("/Library/Fonts/SF-Compact-Display-Medium.otf");
    if let Some(r) = regular {
        fonts
            .font_data
            .insert("sf".into(), Arc::new(FontData::from_owned(r)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "sf".into());
    }
    let named =
        |fonts: &mut FontDefinitions, family: &str, face: Option<(String, Arc<FontData>)>| {
            let mut list = vec![];
            if let Some((name, data)) = face {
                fonts.font_data.insert(name.clone(), data);
                list.push(name);
            }
            fonts.families.insert(FontFamily::Name(family.into()), list);
        };
    named(
        &mut fonts,
        SEMIBOLD,
        semi.map(|b| ("sf-semibold".to_string(), Arc::new(FontData::from_owned(b)))),
    );
    named(
        &mut fonts,
        MEDIUM,
        med.map(|b| ("sf-medium".to_string(), Arc::new(FontData::from_owned(b)))),
    );
    let mono = mono_bytes();
    named(
        &mut fonts,
        MONO_SEMIBOLD,
        mono.clone()
            .map(|b| ("mono-600".to_string(), weighted(b, 600.0))),
    );
    if let Some(b) = mono {
        fonts
            .font_data
            .insert("mono-500".into(), weighted(b, 500.0));
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "mono-500".into());
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    // the named families and mono fall back to the proportional fonts (Phosphor icons, symbols)
    let prop = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for fam in [semibold(), medium(), mono_semibold(), FontFamily::Monospace] {
        let list = fonts.families.entry(fam).or_default();
        for f in &prop {
            if !list.contains(f) {
                list.push(f.clone());
            }
        }
    }
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(16.0, semibold())),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 8.0);
        s.button_padding = egui::vec2(11.0, 5.0);
        // (menus and egui tooltips share this frame margin)
        s.menu_margin = Margin::symmetric(6, 5);
        s.tooltip_width = 360.0;
        s.interact_size = egui::vec2(28.0, 28.0);
        s.slider_width = 150.0;
        s.slider_rail_height = 4.0;
        s.combo_width = 160.0;
        s.window_margin = Margin::same(16);
        s.icon_width = 15.0;
        s.icon_width_inner = 9.0;
        s.scroll.bar_width = 6.0;
        s.scroll.floating_allocated_width = 0.0;

        let mut v = Visuals::dark();
        v.override_text_color = Some(TEXT);
        v.weak_text_color = Some(TEXT_MUTE);
        v.panel_fill = GLASS;
        v.window_fill = GLASS_SOLID;
        v.window_stroke = Stroke::new(1.0, LINE_STRONG);
        v.window_corner_radius = CornerRadius::same(R_WINDOW);
        v.menu_corner_radius = CornerRadius::same(R_MENU);
        v.window_shadow = Shadow {
            offset: [0, 26],
            blur: 70,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        v.popup_shadow = Shadow {
            offset: [0, 14],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.window_highlight_topmost = false;
        v.extreme_bg_color = WELL;
        v.text_edit_bg_color = Some(WELL);
        v.code_bg_color = WELL;
        v.faint_bg_color = Color32::from_white_alpha(8);
        v.selection.bg_fill = SOFT_HOVER;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.text_cursor.stroke = Stroke::new(2.0, TEXT);
        v.hyperlink_color = TEXT;
        v.warn_fg_color = WARN;
        v.error_fg_color = DANGER;
        v.slider_trailing_fill = true;
        v.handle_shape = HandleShape::Circle;
        let r = CornerRadius::same(R_BUTTON);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SOFT;
        w.noninteractive.weak_bg_fill = SOFT;
        w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
        w.noninteractive.corner_radius = r;
        // bg_fill: tracks, rails, scroll handles; weak_bg_fill: buttons
        w.inactive.bg_fill = TRACK;
        w.inactive.weak_bg_fill = SOFT;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.5, TEXT_DIM);
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = Color32::from_white_alpha(70);
        w.hovered.weak_bg_fill = SOFT_HOVER;
        w.hovered.bg_stroke = Stroke::NONE;
        w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = Color32::from_white_alpha(90);
        w.active.weak_bg_fill = Color32::from_white_alpha(46);
        w.active.bg_stroke = Stroke::NONE;
        w.active.fg_stroke = Stroke::new(1.5, TEXT);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;
        w.open.bg_fill = SOFT_HOVER;
        w.open.weak_bg_fill = SOFT_HOVER;
        w.open.bg_stroke = Stroke::NONE;
        w.open.fg_stroke = Stroke::new(1.5, TEXT);
        w.open.corner_radius = r;
        style.visuals = v;
    });
}
