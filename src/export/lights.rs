//! The caves' glowing crystals as engine point lights. Their lumens are what the editor uses for
//! an eye adjusted to a cave (`sky::CAVE_EV`, about EV 9.6 by day): engines without physical
//! light units get a plain intensity, Unreal (lumens, auto exposure that adapts much further
//! into the dark) gets them scaled to a real adapted eye.

use serde_json::Value;

pub struct CaveLight {
    /// world metres
    pub pos: [f32; 3],
    /// linear RGB
    pub color: [f32; 3],
    pub lumens: f32,
    /// metres
    pub range: f32,
}

impl CaveLight {
    /// Intensity for unitless engine lights (Unity built-in / URP, Godot): about 1.5 for the
    /// smallest crystal, up to 8.
    pub fn intensity(&self) -> f32 {
        ((self.lumens / 300_000.0).sqrt() * 1.5).clamp(0.5, 8.0)
    }

    /// Lumens for an eye adapted to a dark cave (around EV 3 rather than the editor's ~9.6).
    pub fn physical_lumens(&self) -> f32 {
        self.lumens / 100.0
    }
}

/// Every crystal light of the caves' JSON (`core::CavesOut::json`).
pub fn cave_lights(caves: &Value) -> Vec<CaveLight> {
    let f3 = |v: &Value| -> Option<[f32; 3]> {
        Some([
            v.get(0)?.as_f64()? as f32,
            v.get(1)?.as_f64()? as f32,
            v.get(2)?.as_f64()? as f32,
        ])
    };
    caves
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|c| {
            c.get("crystal_lights")
                .and_then(|l| l.as_array())
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|l| {
            Some(CaveLight {
                pos: f3(l.get("position")?)?,
                color: f3(l.get("color_linear")?).unwrap_or([1.0; 3]),
                lumens: l.get("lumens")?.as_f64()? as f32,
                range: l.get("range")?.as_f64()? as f32,
            })
        })
        .collect()
}
