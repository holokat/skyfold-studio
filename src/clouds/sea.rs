//! The cloud sea: a low, dense deck of cloud lying in the valleys and canyons, with the peaks and
//! mesas standing out of it (seen from a cliff or a summit at sunset). Off by default.
//!
//! It is a second layer of the cloud trace (`clouds.wesl`): the same march, light and history as
//! the cumulus, with its own band, cover and a flatter, billowy profile. The settings reach the
//! shader through the unused `w` channels of the cloud uniforms, so the uniform layout is the
//! same with or without it (an extinction of 0 there means no sea):
//! `fill.w` base altitude, `fill_color.w` top altitude, `key_color.w` coverage,
//! `shade.w` softness, `fill_ambient.w` extinction (1/m).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Extinction (1/m) of the densest sea cloud at density 1: a stratocumulus deck, opaque within a
/// few tens of metres.
const SIGMA: f32 = 0.045;
/// Distances (m) over which the sea fades in from a camera above it (`clouds.wesl`,
/// `near_fade`; from inside or under the deck it is thin ground fog thickening into the bank).
pub(super) const NEAR_FADE: (f32, f32) = (20.0, 60.0);

/// The cloud sea (`Lighting::cloud_sea`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSea {
    pub enabled: bool,
    /// Height (m) of the deck's base above the terrain's sea level.
    pub altitude: f32,
    /// Depth (m) of the deck from its base to the highest billows.
    pub thickness: f32,
    /// 0 scattered banks in the valleys .. 1 one unbroken sea.
    pub coverage: f32,
    /// 0 crisp, billowy tops .. 1 soft, fog-like banks with a misty base.
    pub softness: f32,
    /// Optical thickness: 0.3 thin and see-through .. 1 normal .. 2 dense.
    pub density: f32,
}

impl Default for CloudSea {
    fn default() -> Self {
        Self {
            enabled: false,
            altitude: 40.0,
            thickness: 210.0,
            coverage: 0.85,
            softness: 0.35,
            density: 1.0,
        }
    }
}

impl CloudSea {
    /// Base and top altitude (world y) of the deck.
    pub fn band(&self, sea_level: f32) -> (f32, f32) {
        let base = sea_level + self.altitude.clamp(-200.0, 3000.0);
        (base, base + self.thickness.clamp(20.0, 1500.0))
    }

    /// The shader's values (see the module doc), all zero when off.
    pub(super) fn pack(&self, sea_level: f32) -> [f32; 5] {
        if !self.enabled {
            return [0.0; 5];
        }
        let (base, top) = self.band(sea_level);
        [
            base,
            top,
            self.coverage.clamp(0.0, 1.0),
            self.softness.clamp(0.0, 1.0),
            SIGMA * self.density.clamp(0.05, 4.0),
        ]
    }

    /// The `cloud_sea` object of the MCP tools (`weather_set`, `lighting_set`).
    pub fn mcp_schema() -> Value {
        json!({ "type": "object", "description": "cloud sea (off by default): a dense, sunlit deck of low cloud filling the valleys and canyons, with the peaks and mesas standing out of it; best seen from high ground, with a low sun. Part of the volumetric clouds (Weather tab)", "properties": {
            "enable": { "type": "boolean" },
            "altitude": { "type": "number", "minimum": -200, "maximum": 3000, "description": "height (m) of the deck's base above the sea level (default 40); terrain above altitude + thickness stands out of it" },
            "thickness": { "type": "number", "minimum": 20, "maximum": 1500, "description": "depth (m) of the deck up to its highest billows (default 210)" },
            "coverage": { "type": "number", "minimum": 0, "maximum": 1, "description": "0 scattered banks … 1 one unbroken sea (default 0.85)" },
            "softness": { "type": "number", "minimum": 0, "maximum": 1, "description": "0 crisp billowy tops … 1 soft fog-like banks (default 0.35)" },
            "density": { "type": "number", "minimum": 0.3, "maximum": 2, "description": "optical thickness (default 1)" }
        } })
    }

    /// Applies a `cloud_sea` object from an MCP call.
    pub fn apply_mcp(&mut self, v: &Value) -> Result<(), String> {
        let Some(o) = v.as_object() else {
            return Err("`cloud_sea` is an object".into());
        };
        if let Some(b) = o.get("enable") {
            self.enabled = b.as_bool().ok_or("`cloud_sea.enable` is a boolean")?;
        }
        for (k, f, lo, hi) in [
            ("altitude", &mut self.altitude, -200.0, 3000.0),
            ("thickness", &mut self.thickness, 20.0, 1500.0),
            ("coverage", &mut self.coverage, 0.0, 1.0),
            ("softness", &mut self.softness, 0.0, 1.0),
            ("density", &mut self.density, 0.3, 2.0),
        ] {
            if let Some(x) = o.get(k) {
                let x = x
                    .as_f64()
                    .ok_or_else(|| format!("`cloud_sea.{k}` is a number"))?;
                *f = (x as f32).clamp(lo, hi);
            }
        }
        Ok(())
    }

    /// One line for tool replies.
    pub fn describe(&self) -> String {
        if !self.enabled {
            return "Cloud sea: off.".into();
        }
        format!(
            "Cloud sea: base {:.0} m above sea level, {:.0} m deep, coverage {:.2}, softness {:.2}, density {:.2}.",
            self.altitude, self.thickness, self.coverage, self.softness, self.density
        )
    }
}

/// The Weather tab's controls (Clouds › Cloud sea): the switch, then the deck's shape.
pub fn controls(ui: &mut bevy_egui::egui::Ui, s: &mut CloudSea) {
    use crate::ui::widgets::{hint_small, slider_row, toggle};
    toggle(ui, &mut s.enabled, "Cloud sea").on_hover_text("A dense deck of low cloud filling the valleys and canyons, with the peaks and mesas standing out of it. Best seen from high ground with a low sun.");
    if !s.enabled {
        return;
    }
    slider_row(ui, "Height", &mut s.altitude, -200.0..=1500.0, |v| {
        format!("{v:.0} m")
    })
    .on_hover_text("The deck's base above the sea level: land higher than height + depth stands out of it.");
    slider_row(ui, "Depth", &mut s.thickness, 20.0..=800.0, |v| {
        format!("{v:.0} m")
    });
    slider_row(ui, "Cover", &mut s.coverage, 0.0..=1.0, |v| {
        format!("{:.0}%", v * 100.0)
    })
    .on_hover_text("Scattered banks … one unbroken sea.");
    slider_row(ui, "Softness", &mut s.softness, 0.0..=1.0, |v| {
        format!("{:.0}%", v * 100.0)
    })
    .on_hover_text("Crisp billowy tops … soft, fog-like banks.");
    slider_row(ui, "Thickness", &mut s.density, 0.3..=2.0, |v| format!("{v:.2}"));
    hint_small(ui, "Drawn with the volumetric clouds.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_and_packs_to_zero() {
        let s: CloudSea = serde_json::from_str("{}").unwrap();
        assert_eq!(s, CloudSea::default());
        assert_eq!(s.pack(12.0), [0.0; 5]);
    }

    #[test]
    fn mcp_sets_and_clamps() {
        let mut s = CloudSea::default();
        s.apply_mcp(&json!({ "enable": true, "altitude": 90, "coverage": 3 }))
            .unwrap();
        assert!(s.enabled);
        assert_eq!(s.coverage, 1.0);
        let p = s.pack(10.0);
        assert_eq!(p[0], 100.0);
        assert_eq!(p[1], 310.0);
        assert!(p[4] > 0.0);
    }
}
