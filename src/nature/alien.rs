//! Alien flora: the green of every plant turned to another hue (crimson scrub, violet heather,
//! teal fronds) without new assets. Off by default.
//!
//! The scanned leaves keep their texture, light and dark; only their colour moves: a pixel is
//! recoloured as much as it is green (bark, stems, dry straw, rock and flowers keep theirs), to
//! the target hue with about its own value and a scaled saturation, the leaves' natural hue spread
//! kept around it (`alien_flora.wesl`). Trees, bushes and plants (`foliage.wesl`), the meadow
//! grass (`grass.wesl`), the distant-tree pictures (taken again) and the far canopy follow.
//!
//! Modes (`FloraMode`) make plants something other than recoloured leaves: charred (burnt
//! grey-black, the bark blackened too), frosted (pale icy white-blue with a glassy sheen), glowing
//! (recoloured and softly glowing in the hue, readable at night), petrified (stone grey-ochre, the
//! whole plant). The mode rides in the hue's channel as its whole part.
//!
//! The shaders get the setting through channels their buffers already have spare, so their
//! layouts are the same with or without it: the foliage wind buffer's `gust.zw` (0 = off) and the
//! grass buffer's `warm.w` / `cold.w` (1 = off).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Named looks for [`AlienFlora`]: (name, hue in degrees, saturation).
pub const PRESETS: [(&str, f32, f32); 6] = [
    ("crimson", 352.0, 1.35),
    ("scarlet", 8.0, 1.4),
    ("magenta", 320.0, 1.3),
    ("violet", 278.0, 1.15),
    ("teal", 178.0, 1.2),
    ("amber", 36.0, 1.3),
];

/// What alien flora does to the plants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FloraMode {
    /// The green turns to the hue (the leaves' own light and dark kept).
    #[default]
    Recolour,
    /// Burnt: dull grey-black leaves, the bark blackened too (hue: a trace of colour; saturation
    /// 1 ≈ a faint warm tint, 0 pure grey).
    Charred,
    /// Pale icy leaves with a glassy sheen, the bark rimed (hue ~195–210 for blue-white ice).
    Frosted,
    /// Recoloured to the hue and glowing softly in it: readable at night.
    Glow,
    /// Stone: the whole plant grey-ochre, matte (hue ~35 ochre, ~210 cool slate).
    Petrified,
}

impl FloraMode {
    pub const ALL: [FloraMode; 5] = [
        FloraMode::Recolour,
        FloraMode::Charred,
        FloraMode::Frosted,
        FloraMode::Glow,
        FloraMode::Petrified,
    ];
    pub fn id(self) -> &'static str {
        match self {
            FloraMode::Recolour => "recolour",
            FloraMode::Charred => "charred",
            FloraMode::Frosted => "frosted",
            FloraMode::Glow => "glow",
            FloraMode::Petrified => "petrified",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            FloraMode::Recolour => "Recolour",
            FloraMode::Charred => "Charred",
            FloraMode::Frosted => "Frosted",
            FloraMode::Glow => "Glowing",
            FloraMode::Petrified => "Petrified",
        }
    }
    fn index(self) -> f32 {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as f32
    }
    /// How much of the non-green parts (bark, stems, flowers) the mode changes too.
    fn whole(self) -> f32 {
        match self {
            FloraMode::Charred => 0.6,
            FloraMode::Frosted => 0.4,
            FloraMode::Petrified => 0.85,
            FloraMode::Recolour | FloraMode::Glow => 0.0,
        }
    }
}

/// The world's alien flora (`NatureAppearance::alien`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlienFlora {
    pub enabled: bool,
    /// Target hue in degrees (0 red, 352 crimson, 320 magenta, 278 violet, 178 teal, 36 amber).
    pub hue: f32,
    /// How much of the green turns, 0..1.
    pub amount: f32,
    /// Saturation of the new colour relative to the leaves' own, 0..2.5.
    pub saturation: f32,
    /// What the plants become (recoloured by default).
    pub mode: FloraMode,
}

impl Default for AlienFlora {
    fn default() -> Self {
        Self {
            enabled: false,
            hue: 352.0,
            amount: 1.0,
            saturation: 1.35,
            mode: FloraMode::Recolour,
        }
    }
}

impl AlienFlora {
    /// The "crimson alien scrub" look.
    pub fn crimson() -> Self {
        Self {
            enabled: true,
            ..default()
        }
    }

    pub(super) fn sanitized(self) -> Self {
        let f = |v: f32, def: f32, lo: f32, hi: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                def
            }
        };
        let d = Self::default();
        Self {
            enabled: self.enabled,
            hue: f(self.hue, d.hue, -720.0, 720.0).rem_euclid(360.0),
            amount: f(self.amount, d.amount, 0.0, 1.0),
            saturation: f(self.saturation, d.saturation, 0.0, 2.5),
            mode: self.mode,
        }
    }

    /// The shaders' pair (see `alien_flora.wesl`): x the mode's index plus the hue in turns, y the
    /// saturation in hundredths plus the amount (below 1) as its fraction. (0, 0) when off.
    pub fn pack(&self) -> Vec2 {
        let s = self.sanitized();
        if !s.enabled || s.amount <= 0.0 {
            return Vec2::ZERO;
        }
        Vec2::new(
            s.mode.index() + (s.hue / 360.0).min(0.9999),
            (s.saturation * 100.0).round() + s.amount.min(0.999),
        )
    }

    /// The same recolouring on the CPU (linear RGB), for colours baked into meshes (the far
    /// canopy). Mirrors `alien_flora.wesl::alien_tint`.
    pub fn recolor(&self, c: [f32; 3]) -> [f32; 3] {
        let p = self.pack();
        if p == Vec2::ZERO {
            return c;
        }
        let (hue, amount, sat) = (p.x.fract(), p.y.fract(), p.y.floor() / 100.0);
        let mode = self.sanitized().mode;
        let s = c.map(|v| v.max(0.0).powf(1.0 / 2.2));
        let max = s[0].max(s[1]).max(s[2]);
        let min = s[0].min(s[1]).min(s[2]);
        let chroma = max - min;
        if max <= 1e-4 {
            return c;
        }
        let green = if chroma > 1e-4 {
            greenness(s, chroma)
        } else {
            0.0
        };
        let k = amount * green.max(mode.whole());
        if k <= 0.0 {
            return c;
        }
        let t = match mode {
            FloraMode::Charred => hsv(hue, (0.15 * sat).clamp(0.0, 1.0), max * 0.3 + 0.02),
            FloraMode::Frosted => hsv(hue, (0.22 * sat).clamp(0.0, 1.0), (0.6 + max * 0.45).min(0.96)),
            FloraMode::Petrified => {
                hsv(hue, (0.1 * sat).clamp(0.0, 1.0), (max * 0.55 + 0.3).min(0.8))
            }
            FloraMode::Recolour | FloraMode::Glow => {
                let h_src = source_hue(s, max, chroma);
                let h = (hue + (h_src - 0.27) * 0.5).rem_euclid(1.0);
                let sv = ((chroma / max).max(0.4) * sat).clamp(0.0, 1.0);
                hsv(h, sv, (max * 1.15).min(1.0))
            }
        };
        (0..3)
            .map(|i| (s[i] + (t[i] - s[i]) * k).powf(2.2))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap_or(c)
    }

    /// The `alien_flora` MCP tool's schema.
    pub fn mcp_schema() -> Value {
        json!({ "type": "object", "properties": {
            "enable": { "type": "boolean", "description": "alien flora on/off (default off)" },
            "preset": { "type": "string", "enum": PRESETS.map(|p| p.0), "description": "a named colour (sets hue and saturation; crimson: red alien scrub)" },
            "mode": { "type": "string", "enum": FloraMode::ALL.map(|m| m.id()), "description": "what the plants become: recolour (green turns to the hue, default), charred (burnt grey-black, bark blackened; hue 20, saturation 1), frosted (pale icy, glassy sheen, bark rimed; hue 200, saturation 1), glow (recoloured and softly glowing in the hue, readable at night; e.g. hue 178 teal, saturation 1.6), petrified (whole plant stone grey-ochre, matte; hue 35, saturation 1)" },
            "hue": { "type": "number", "minimum": 0, "maximum": 360, "description": "target hue in degrees: 0 red, 352 crimson, 320 magenta, 278 violet, 178 teal, 36 amber" },
            "amount": { "type": "number", "minimum": 0, "maximum": 1, "description": "how much of the green turns (default 1)" },
            "saturation": { "type": "number", "minimum": 0, "maximum": 2.5, "description": "saturation relative to the leaves' own (default 1.35)" }
        } })
    }

    /// Applies an `alien_flora` MCP call.
    pub fn apply_mcp(&mut self, a: &Value) -> Result<(), String> {
        if let Some(p) = a.get("preset").and_then(|v| v.as_str()) {
            let (_, hue, sat) = PRESETS
                .iter()
                .find(|x| x.0 == p)
                .ok_or_else(|| {
                    format!(
                        "`preset` is one of {}",
                        PRESETS.map(|p| p.0).join(", ")
                    )
                })?;
            self.hue = *hue;
            self.saturation = *sat;
            self.enabled = true;
        }
        if let Some(m) = a.get("mode") {
            let m = m.as_str().ok_or("`mode` is a string")?;
            self.mode = FloraMode::ALL
                .into_iter()
                .find(|x| x.id() == m)
                .ok_or_else(|| format!("`mode` is one of {}", FloraMode::ALL.map(|m| m.id()).join(", ")))?;
            self.enabled = true;
        }
        if let Some(b) = a.get("enable") {
            self.enabled = b.as_bool().ok_or("`enable` is a boolean")?;
        }
        for (k, f, lo, hi) in [
            ("hue", &mut self.hue, 0.0, 360.0),
            ("amount", &mut self.amount, 0.0, 1.0),
            ("saturation", &mut self.saturation, 0.0, 2.5),
        ] {
            if let Some(v) = a.get(k) {
                let v = v.as_f64().ok_or_else(|| format!("`{k}` is a number"))? as f32;
                *f = v.clamp(lo, hi);
            }
        }
        *self = self.sanitized();
        Ok(())
    }

    pub fn describe(&self) -> String {
        if !self.enabled {
            return "Alien flora: off (plants keep their own colours).".into();
        }
        let name = PRESETS
            .iter()
            .find(|p| (p.1 - self.hue).abs() < 0.5 && (p.2 - self.saturation).abs() < 0.01)
            .map(|p| format!(" ({})", p.0))
            .unwrap_or_default();
        format!(
            "Alien flora: on{name}, mode {}, hue {:.0}°, amount {:.2}, saturation {:.2}.",
            self.mode.id(),
            self.hue,
            self.amount,
            self.saturation
        )
    }
}

/// How green a (gamma) colour is, 0..1: yellow-green to blue-green foliage, not browns or greys.
fn greenness(s: [f32; 3], chroma: f32) -> f32 {
    let lead = s[1] - s[0].max(s[2]) * 0.92;
    smoothstep(0.0, 0.35, lead / chroma.max(1e-4)) * smoothstep(0.005, 0.03, chroma)
}

fn source_hue(s: [f32; 3], max: f32, chroma: f32) -> f32 {
    let h = if max == s[0] {
        ((s[1] - s[2]) / chroma).rem_euclid(6.0)
    } else if max == s[1] {
        (s[2] - s[0]) / chroma + 2.0
    } else {
        (s[0] - s[1]) / chroma + 4.0
    };
    h / 6.0
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    [0.0f32, 2.0 / 3.0, 1.0 / 3.0].map(|o| {
        let k = ((h + o).fract() * 6.0 - 3.0).abs() - 1.0;
        v * (1.0 - s + s * k.clamp(0.0, 1.0))
    })
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Distant-tree pictures are taken with the foliage shader: a new flora colour takes them again.
pub(super) fn follow(
    appearance: Res<super::NatureAppearance>,
    atlas: Option<ResMut<crate::trees::ImpostorAtlas>>,
    mut last: Local<Option<Vec2>>,
) {
    let p = appearance.alien.pack();
    if *last == Some(p) {
        return;
    }
    let first = last.is_none();
    *last = Some(p);
    if let Some(mut atlas) = atlas
        && !(first && p == Vec2::ZERO)
    {
        atlas.retake();
    }
}

/// The Land tab's nature dials: the switch, the colour chips, then hue, amount and saturation.
pub fn controls(ui: &mut bevy_egui::egui::Ui, a: &mut AlienFlora) {
    use crate::ui::widgets::{chip, slider_row, toggle};
    toggle(ui, &mut a.enabled, "Alien flora").on_hover_text("Turns the green of every tree, bush, plant and blade of grass to another colour: crimson scrub, violet heather, teal fronds. Bark, flowers and rock keep theirs.");
    if !a.enabled {
        return;
    }
    // what the plants become
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = bevy_egui::egui::vec2(4.0, 4.0);
        for m in FloraMode::ALL {
            let tip = match m {
                FloraMode::Recolour => "The green turns to the colour below.",
                FloraMode::Charred => "Burnt: dull grey-black leaves, the bark blackened.",
                FloraMode::Frosted => "Pale icy leaves with a glassy sheen, the bark rimed.",
                FloraMode::Glow => "Recoloured and glowing softly in the colour: readable at night.",
                FloraMode::Petrified => "The whole plant turned to stone.",
            };
            if chip(ui, m.name(), None, a.mode == m).on_hover_text(tip).clicked() {
                a.mode = m;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = bevy_egui::egui::vec2(4.0, 4.0);
        for (name, hue, sat) in PRESETS {
            let on = (a.hue - hue).abs() < 0.5 && (a.saturation - sat).abs() < 0.01;
            let mut label = name.to_string();
            label[..1].make_ascii_uppercase();
            if chip(ui, &label, None, on).clicked() {
                a.hue = hue;
                a.saturation = sat;
            }
        }
    });
    slider_row(ui, "Hue", &mut a.hue, 0.0..=360.0, |v| format!("{v:.0}°"));
    slider_row(ui, "Amount", &mut a.amount, 0.0..=1.0, |v| {
        format!("{:.0}%", v * 100.0)
    });
    slider_row(ui, "Saturation", &mut a.saturation, 0.0..=2.5, |v| {
        format!("{v:.2}×")
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_and_leaves_colours_alone() {
        let a: AlienFlora = serde_json::from_str("{}").unwrap();
        assert!(!a.enabled);
        assert_eq!(a.pack(), Vec2::ZERO);
        assert_eq!(a.recolor([0.1, 0.3, 0.05]), [0.1, 0.3, 0.05]);
    }

    #[test]
    fn crimson_turns_green_red_and_keeps_bark() {
        let a = AlienFlora::crimson();
        let leaf = a.recolor([0.06, 0.2, 0.03]);
        assert!(leaf[0] > leaf[1] * 2.0, "{leaf:?}");
        let bark = [0.2, 0.12, 0.07];
        assert_eq!(a.recolor(bark), bark);
        let p = a.pack();
        assert!((p.y.fract() - 0.999).abs() < 1e-3 && p.y.floor() == 135.0);
        // (recolour keeps the old packing exactly)
        assert!(p.x < 1.0 && (p.x - 352.0 / 360.0).abs() < 1e-6);
    }

    #[test]
    fn modes_ride_in_the_hue_channel_and_change_the_plant() {
        let a: AlienFlora = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert_eq!(a.mode, FloraMode::Recolour);
        let leaf = [0.06, 0.2, 0.03];
        let bark = [0.2, 0.12, 0.07];
        let with = |mode, hue| AlienFlora {
            enabled: true,
            hue,
            saturation: 1.0,
            mode,
            ..default()
        };
        for (i, m) in FloraMode::ALL.into_iter().enumerate() {
            let p = with(m, 200.0).pack();
            assert_eq!(p.x.floor() as usize, i);
            assert!((p.x.fract() - 200.0 / 360.0).abs() < 1e-4);
        }
        let charred = with(FloraMode::Charred, 20.0);
        let l = charred.recolor(leaf);
        assert!(l[1] < 0.06 && (l[0] - l[1]).abs() < 0.03, "{l:?}");
        assert!(charred.recolor(bark)[0] < bark[0], "bark blackens");
        let frost = with(FloraMode::Frosted, 200.0).recolor(leaf);
        assert!(frost.iter().all(|v| *v > 0.3) && frost[2] >= frost[0], "{frost:?}");
        let stone = with(FloraMode::Petrified, 35.0).recolor(bark);
        assert!(stone[0] > bark[0] && (stone[0] - stone[2]) < 0.15, "{stone:?}");
        let mut b = AlienFlora::default();
        b.apply_mcp(&json!({"mode": "glow", "hue": 178})).unwrap();
        assert!(b.enabled && b.mode == FloraMode::Glow);
        assert!(b.apply_mcp(&json!({"mode": "wet"})).is_err());
    }
}
