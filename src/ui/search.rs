//! ⌘K settings search: every labelled control in the panels, inspectors and docks (indexed from
//! the panel code by `build.rs`), and the reveal that takes you to one: its tab and tool, the
//! panels shown, its sections opened, the control scrolled into view and ringed for a moment.

use bevy_egui::egui::{self, Align, CornerRadius, Id, Rect, Stroke, StrokeKind};

use super::UiState;
use crate::editor::{Tab, TabGroup};

/// Which island a setting is drawn in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Dock,
}

pub struct Setting {
    pub label: &'static str,
    /// Slider, Switch, Dropdown, Colour, Readout, Button, Section or Tool
    pub kind: &'static str,
    pub tab: Tab,
    pub side: Side,
    /// the Nature tab's fields (true) or wild nature (false) mode
    pub fields: Option<bool>,
    /// the tool it belongs to (`set_tool` id), `u16::MAX` for none
    pub tool: u16,
    pub tool_name: &'static str,
    /// the sections around it, outermost first
    pub sections: &'static [&'static str],
}

mod index {
    use super::{Setting, Side, UiState};
    use crate::editor::Tab;
    include!(concat!(env!("OUT_DIR"), "/settings_index.rs"));
}
pub use index::SETTINGS;

impl Setting {
    /// Shown on the rail now (dungeon-only tabs only inside a dungeon, and so on).
    pub fn visible(&self) -> bool {
        let g = self.tab.group();
        TabGroup::visible().contains(&g) && g.tabs().contains(&self.tab)
    }

    /// Where it lives: "Atmosphere › Weather › Rain".
    pub fn path(&self) -> String {
        let g = self.tab.group();
        let mut p = vec![g.name()];
        if g.tabs().len() > 1 {
            p.push(TabGroup::sub_name(self.tab));
        }
        if self.fields == Some(true) {
            p.push("Fields");
        }
        if !self.tool_name.is_empty() {
            p.push(self.tool_name);
        }
        p.extend(self.sections.iter().copied());
        p.join(" › ")
    }

    pub fn icon(&self) -> &'static str {
        match self.kind {
            "Slider" => "SettingsSliderHor",
            "Switch" => "SettingsSliderThree",
            "Dropdown" => "ChevronGrabberVertical",
            "Colour" => "ColorSwatch",
            "Readout" => "Gauge",
            "Button" => "CursorClick",
            "Section" => "ListBullets",
            "Tool" => "Hammer",
            _ => "SettingsGear1",
        }
    }
}

/// How long the ring stays (s).
const SHOW: f64 = 2.6;

#[derive(Clone, Default)]
struct Reveal {
    label: String,
    /// sections to open (a section's own title too, when it's the target)
    open: Vec<String>,
    start: f64,
}

fn reveal_id() -> Id {
    Id::new("halo_search_reveal")
}

fn active(ctx: &egui::Context) -> Option<(Reveal, f64)> {
    let r: Reveal = ctx.data(|d| d.get_temp(reveal_id()))?;
    let t = ctx.input(|i| i.time) - r.start;
    (t < SHOW).then_some((r, t))
}

/// Take the user to `set`: its tab, mode and tool, both panels shown, then (over the next frames)
/// its sections opened and the control scrolled into view and ringed.
pub fn go(ctx: &egui::Context, s: &mut UiState, set: &Setting) {
    s.ed.tab = set.tab;
    if set.tab == Tab::Nature {
        s.field_tool.on = set.fields.unwrap_or(false);
    }
    index::set_tool(s, set.tool);
    let mut sh = super::shell::get(ctx);
    sh.panel_open = true;
    sh.insp_open |= set.side == Side::Right;
    super::shell::set(ctx, sh);
    let mut open: Vec<String> = set.sections.iter().map(|t| t.to_string()).collect();
    if set.kind == "Section" {
        open.push(set.label.to_string());
    }
    // (read the clock outside the data lock: egui's locks don't nest)
    let start = ctx.input(|i| i.time);
    ctx.data_mut(|d| {
        d.insert_temp(
            reveal_id(),
            Reveal {
                label: set.label.to_string(),
                open,
                start,
            },
        )
    });
}

/// A section fold asks whether a reveal wants it open.
pub fn wants_open(ui: &egui::Ui, title: &str) -> bool {
    active(ui.ctx()).is_some_and(|(r, _)| r.open.iter().any(|t| t.eq_ignore_ascii_case(title)))
}

/// Widgets call this with their rect and label: the revealed one is scrolled to and ringed.
pub fn mark(ui: &egui::Ui, rect: Rect, label: &str) {
    let Some((r, t)) = active(ui.ctx()) else {
        return;
    };
    if !r.label.eq_ignore_ascii_case(label.trim()) {
        return;
    }
    // keep scrolling while sections above it open and the layout settles
    if t < 0.35 {
        ui.scroll_to_rect(rect, Some(Align::Center));
    }
    let fade = (1.0 - ((t - 1.6) / (SHOW - 1.6)).clamp(0.0, 1.0)) as f32;
    let pulse = 0.65 + 0.35 * ((t * 5.0).sin() as f32).abs();
    ui.painter().rect_stroke(
        rect.expand(4.0),
        CornerRadius::same(9),
        Stroke::new(1.5, egui::Color32::WHITE.gamma_multiply(fade * pulse)),
        StrokeKind::Outside,
    );
    ui.painter().rect_filled(
        rect.expand(4.0),
        CornerRadius::same(9),
        egui::Color32::from_white_alpha((14.0 * fade) as u8),
    );
    ui.ctx().request_repaint();
}

/// The settings matching `q` (every word in the label or its path), best first.
pub fn find(q: &str) -> Vec<usize> {
    let q = q.trim().to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }
    let mut hits: Vec<(usize, usize, usize)> = SETTINGS
        .iter()
        .enumerate()
        .filter(|(_, st)| st.visible())
        .filter_map(|(i, st)| {
            let label = st.label.to_lowercase();
            let hay = format!("{label} {}", st.path().to_lowercase());
            if !words.iter().all(|w| hay.contains(w)) {
                return None;
            }
            let score = if label == q {
                0
            } else if label.starts_with(&q) {
                1
            } else if label
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w.starts_with(words[0]))
            {
                2
            } else {
                3
            };
            // controls before the section headers named like them
            let section = (st.kind == "Section") as usize;
            Some((score * 2 + section, st.path().len(), i))
        })
        .collect();
    hits.sort();
    hits.into_iter().map(|h| h.2).collect()
}

/// Asked for from outside the UI (MCP): handled at the start of the next UI frame.
pub enum Request {
    /// reveal `SETTINGS[i]`
    Open(usize),
    /// show the ⌘K palette with these words typed
    Palette(String),
}

static PENDING: std::sync::Mutex<Option<Request>> = std::sync::Mutex::new(None);

pub fn request(r: Request) {
    if let Ok(mut p) = PENDING.lock() {
        *p = Some(r);
    }
}

/// Run a pending [`Request`] (called once per frame before the panels are drawn).
pub fn poll(ctx: &egui::Context, s: &mut UiState) {
    let Some(r) = PENDING.lock().ok().and_then(|mut p| p.take()) else {
        return;
    };
    match r {
        Request::Open(i) => {
            if let Some(st) = SETTINGS.get(i) {
                super::palette::close(ctx);
                go(ctx, s, st);
            }
        }
        Request::Palette(q) => super::palette::open_with(ctx, &q),
    }
}
