//! Glowing objects' own colour and light (crystals). A placed crystal without one keeps its
//! made look (violet glow, no light of its own); with a [`Glow`] it takes that colour — its
//! body tinted, its glow that colour — and its light: 0 is dark (no glow, no light), 1 a soft
//! lamp-like glow with a point light round it, up to 10 a beacon.
//!
//! Editor: Objects › selected crystal › Glow (colour swatches, a picker and Light). MCP:
//! `objects_place` items' `color` / `light`, and `objects_glow`. Saved with the world
//! (`world.json` › `object_glow`, by object id).

use std::collections::HashMap;

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::{LibraryEntry, Placed};
use crate::procgen::meshes::AssetKind;
use super::{FoliageExt, FoliageMaterial};

/// A glowing object's colour and light.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Glow {
    /// sRGB, 0..1
    pub color: [f32; 3],
    /// 0 = dark, 1 = a soft glow and light, up to [`MAX_LIGHT`]
    pub light: f32,
}

pub const MAX_LIGHT: f32 = 10.0;

/// The crystals' made colour (their violet).
pub const DEFAULT_COLOR: [f32; 3] = [0.62, 0.52, 1.0];

/// Point-light lumens at light 1 for a crystal at scale 1 (about two outdoor torches).
const LUMENS: f32 = 50_000.0;

impl Default for Glow {
    fn default() -> Self {
        Glow {
            color: DEFAULT_COLOR,
            light: 1.0,
        }
    }
}

impl Glow {
    pub fn sanitized(self) -> Self {
        Glow {
            color: self.color.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 1.0 }),
            light: if self.light.is_finite() {
                self.light.clamp(0.0, MAX_LIGHT)
            } else {
                1.0
            },
        }
    }
    /// `#rrggbb`
    pub fn hex(&self) -> String {
        let c = self.color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    }
    /// The colour's name when it's one of [`COLORS`].
    pub fn color_name(&self) -> Option<&'static str> {
        COLORS
            .iter()
            .find(|(_, c)| c.iter().zip(&self.color).all(|(a, b)| (a - b).abs() < 0.01))
            .map(|(n, _)| *n)
    }
    /// Material key: colour and light rounded so near-equal glows share one material.
    fn key(&self) -> [u16; 4] {
        let q = |v: f32| (v * 255.0).round() as u16;
        [
            q(self.color[0]),
            q(self.color[1]),
            q(self.color[2]),
            (self.light * 20.0).round() as u16,
        ]
    }
}

/// The named colours (the swatches, and what `color` accepts by name), sRGB.
pub const COLORS: &[(&str, [f32; 3])] = &[
    ("red", [1.0, 0.16, 0.14]),
    ("orange", [1.0, 0.46, 0.12]),
    ("amber", [1.0, 0.7, 0.2]),
    ("yellow", [1.0, 0.92, 0.3]),
    ("green", [0.3, 1.0, 0.36]),
    ("teal", [0.18, 0.95, 0.78]),
    ("cyan", [0.3, 0.88, 1.0]),
    ("blue", [0.24, 0.48, 1.0]),
    ("violet", DEFAULT_COLOR),
    ("purple", [0.66, 0.26, 1.0]),
    ("pink", [1.0, 0.42, 0.76]),
    ("white", [0.92, 0.95, 1.0]),
];

/// Other names for those colours (gems and shades people ask for).
const ALIASES: &[(&str, &str)] = &[
    ("crimson", "red"),
    ("ruby", "red"),
    ("scarlet", "red"),
    ("blood", "red"),
    ("fire", "orange"),
    ("gold", "amber"),
    ("golden", "amber"),
    ("topaz", "amber"),
    ("citrine", "yellow"),
    ("emerald", "green"),
    ("jade", "green"),
    ("lime", "green"),
    ("turquoise", "teal"),
    ("aqua", "cyan"),
    ("ice", "cyan"),
    ("icy", "cyan"),
    ("frost", "cyan"),
    ("sapphire", "blue"),
    ("azure", "blue"),
    ("amethyst", "purple"),
    ("magenta", "pink"),
    ("rose", "pink"),
    ("quartz", "white"),
    ("diamond", "white"),
    ("clear", "white"),
];

/// A colour by name (red, emerald, …), `#rrggbb`, or `[r, g, b]` sRGB 0..1 (or 0..255).
pub fn parse_color(v: &serde_json::Value) -> Result<[f32; 3], String> {
    let bad = || {
        format!(
            "`color` must be a name ({}…), \"#rrggbb\" or [r, g, b] (sRGB 0..1)",
            COLORS.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")
        )
    };
    match v {
        serde_json::Value::String(s) => {
            let s = s.trim().to_lowercase();
            if let Some(h) = s.strip_prefix('#') {
                let n = u32::from_str_radix(h, 16).map_err(|_| bad())?;
                if h.len() != 6 {
                    return Err(bad());
                }
                return Ok([(n >> 16) & 255, (n >> 8) & 255, n & 255].map(|c| c as f32 / 255.0));
            }
            named(&s).ok_or_else(bad)
        }
        serde_json::Value::Array(a) if a.len() == 3 => {
            let f: Vec<f32> = a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
            if f.len() != 3 || f.iter().any(|x| !x.is_finite() || *x < 0.0) {
                return Err(bad());
            }
            let k = if f.iter().any(|x| *x > 1.0) { 255.0 } else { 1.0 };
            Ok([f[0], f[1], f[2]].map(|x| (x / k).clamp(0.0, 1.0)))
        }
        _ => Err(bad()),
    }
}

/// A named colour (or an alias); also finds "dark red"-style names by their last word.
pub fn named(s: &str) -> Option<[f32; 3]> {
    let w = s.split_whitespace().last().unwrap_or(s);
    let w = ALIASES.iter().find(|a| a.0 == w).map(|a| a.1).unwrap_or(w);
    COLORS.iter().find(|c| c.0 == w).map(|c| c.1)
}

/// Is this word a colour name or alias (for library searches: "red crystals")?
pub fn is_color_word(w: &str) -> bool {
    named(w).is_some()
}

/// Whether placed objects of this entry take a colour and light of their own.
pub fn glows(e: &LibraryEntry) -> bool {
    e.kind == Some(AssetKind::Crystals)
}

/// Neutral crystal textures (built with the crystals: `visuals::formations::build_crystals`),
/// and the materials made from them, one per colour and light in use.
#[derive(Resource)]
pub struct GlowMaterials {
    pub albedo: Handle<Image>,
    pub glow: Handle<Image>,
    mats: HashMap<[u16; 4], Handle<FoliageMaterial>>,
}

impl GlowMaterials {
    pub fn new(albedo: Handle<Image>, glow: Handle<Image>) -> Self {
        GlowMaterials {
            albedo,
            glow,
            mats: HashMap::new(),
        }
    }

    /// The material of a crystal glowing `g` (made once per colour and light).
    pub fn material(&mut self, g: &Glow, foliage: &mut Assets<FoliageMaterial>) -> Handle<FoliageMaterial> {
        let (albedo, glow) = (self.albedo.clone(), self.glow.clone());
        self.mats
            .entry(g.key())
            .or_insert_with(|| {
                let c = Color::srgb(g.color[0], g.color[1], g.color[2]);
                let lin = c.to_linear();
                // the glow grows quickly at first, then slowly (light 1 ≈ the made crystals)
                let e = 1.2 * g.light.powf(0.7);
                foliage.add(FoliageMaterial {
                    base: StandardMaterial {
                        // the body is the colour too, so it reads in daylight
                        base_color: c,
                        base_color_texture: Some(albedo),
                        emissive: LinearRgba::new(lin.red * e, lin.green * e, lin.blue * e, 0.0),
                        emissive_texture: Some(glow),
                        // (rougher and less mirror-like than the made crystals: their own light
                        // would glint off the facets as hard dots, and the sky would grey the colour)
                        perceptual_roughness: 0.22,
                        reflectance: 0.2,
                        clearcoat: 0.25,
                        clearcoat_perceptual_roughness: 0.2,
                        ..default()
                    },
                    extension: FoliageExt::default(),
                })
            })
            .clone()
    }
}

/// The point light a glowing object carries (none at light 0): a child of its render entity,
/// low in the cluster (`height` in model units), so it lights the ground round it and its
/// highlights don't spot the crystal's faces.
pub fn spawn_light(c: &mut ChildSpawnerCommands, p: &Placed, g: &Glow, height: f32) {
    if g.light <= 0.01 {
        return;
    }
    let size = p.scale.clamp(0.2, 8.0);
    // (the render entity is scaled: the offset is in model units)
    let range = (9.0 * size * g.light.sqrt()).clamp(3.0, 60.0);
    c.spawn((
        PointLight {
            color: Color::srgb(g.color[0], g.color[1], g.color[2]),
            intensity: LUMENS * g.light * size * size,
            range,
            radius: 0.05 * size,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.0, height * 0.2, 0.0),
        NotShadowCaster,
    ));
}

/// `world.json` › `object_glow`: the store's glows by id.
pub fn to_save(items: &HashMap<u64, Placed>) -> std::collections::BTreeMap<u64, Glow> {
    items
        .values()
        .filter_map(|p| p.glow.map(|g| (p.id, g)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn colours_parse() {
        assert_eq!(parse_color(&json!("red")).unwrap(), COLORS[0].1);
        assert_eq!(parse_color(&json!("Ruby")).unwrap(), COLORS[0].1);
        assert_eq!(parse_color(&json!("dark red")).unwrap(), COLORS[0].1);
        assert_eq!(parse_color(&json!("#ff0000")).unwrap(), [1.0, 0.0, 0.0]);
        assert_eq!(parse_color(&json!([255, 0, 0])).unwrap(), [1.0, 0.0, 0.0]);
        assert_eq!(parse_color(&json!([0.5, 0.25, 1.0])).unwrap(), [0.5, 0.25, 1.0]);
        assert!(parse_color(&json!("plaid")).is_err());
        assert!(parse_color(&json!("#ff00")).is_err());
        assert!(is_color_word("red") && is_color_word("emerald") && !is_color_word("crystals"));
    }

    #[test]
    fn glow_names_and_bounds() {
        let g = Glow {
            color: COLORS[0].1,
            light: 99.0,
        }
        .sanitized();
        assert_eq!(g.light, MAX_LIGHT);
        assert_eq!(g.color_name(), Some("red"));
        assert_eq!(Glow::default().hex(), "#9e85ff");
    }
}
