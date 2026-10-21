//! Tree options (`options.js`), JSON loading with ez-tree's merge semantics,
//! and the bundled presets.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Tree archetype. Deciduous trees taper by `branch.taper`, grow a terminal
/// branch from the tip of every branch and put a leaf on the final tip;
/// evergreens taper to a point and shorten child branches towards the top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeType {
    /// `"deciduous"`.
    #[default]
    Deciduous,
    /// `"evergreen"`.
    Evergreen,
}

/// Leaf card layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Billboard {
    /// One quad per leaf (`"single"`).
    Single,
    /// Two perpendicular quads per leaf (`"double"`).
    #[default]
    Double,
}

/// A 2D value serialized as `{ "x": .., "y": .. }`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct XY {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
}

/// A 3D value serialized as `{ "x": .., "y": .., "z": .. }`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct XYZ {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

/// Bark parameters (`options.bark`). Texture maps are left to the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BarkOptions {
    /// Informational texture-set identifier (e.g. `"Bark001"`); not used by
    /// the generator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Bark tint as `0xRRGGBB`.
    pub tint: u32,
    /// Use face normals for shading (material hint, not used by the generator).
    pub flat_shading: bool,
    /// Apply the bark texture (material hint, not used by the generator).
    pub textured: bool,
    /// Bark texture scale. `x` sets the number of texture wraps around a
    /// branch (`max(1, round(baseRadius * x))`); `y` divides the ring `v`
    /// coordinate (ez-tree applies it at runtime as `texture.repeat.y`, this
    /// port bakes it into the UVs).
    pub texture_scale: XY,
}

impl Default for BarkOptions {
    fn default() -> Self {
        Self {
            kind: "Bark001".into(),
            tint: 0xffffff,
            flat_shading: false,
            textured: true,
            texture_scale: XY { x: 1.0, y: 1.0 },
        }
    }
}

/// Growth force (`options.branch.force`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForceOptions {
    /// Direction branches are bent towards (normalized internally).
    pub direction: XYZ,
    /// Bend per section, divided by the section radius. Negative bends away.
    pub strength: f64,
}

impl Default for ForceOptions {
    fn default() -> Self {
        Self {
            direction: XYZ {
                x: 0.0,
                y: 1.0,
                z: 0.0,
            },
            strength: 0.01,
        }
    }
}

/// Branch parameters (`options.branch`).
///
/// Per-level values are stored as `[_; 4]` indexed by branch level (0 =
/// trunk). In JSON they are objects keyed `"0"`..`"3"`; `angle` and `start`
/// only have levels 1–3 and `children` only levels 0–2, exactly like
/// `options.js` (the unused slot is 0).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchOptions {
    /// Number of recursion levels, 0 = trunk only. Values above 3 are treated
    /// as 3 (the per-level tables only have four entries).
    #[serde(deserialize_with = "de_count")]
    pub levels: usize,
    /// Angle of child branches relative to their parent, degrees (levels 1–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 1, 3>",
        deserialize_with = "de_levels_f64::<_, 1, 3>"
    )]
    pub angle: [f64; 4],
    /// Number of child branches per branch (levels 0–2).
    #[serde(
        serialize_with = "ser_levels::<_, _, 0, 2>",
        deserialize_with = "de_levels_count::<_, 0, 2>"
    )]
    pub children: [usize; 4],
    /// External growth force.
    pub force: ForceOptions,
    /// Random curl per section (levels 0–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 0, 3>",
        deserialize_with = "de_levels_f64::<_, 0, 3>"
    )]
    pub gnarliness: [f64; 4],
    /// Branch length (levels 0–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 0, 3>",
        deserialize_with = "de_levels_f64::<_, 0, 3>"
    )]
    pub length: [f64; 4],
    /// Branch radius; for levels ≥ 1 a multiplier of the parent radius at
    /// the attach point (levels 0–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 0, 3>",
        deserialize_with = "de_levels_f64::<_, 0, 3>"
    )]
    pub radius: [f64; 4],
    /// Sections (rings) per branch (levels 0–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 0, 3>",
        deserialize_with = "de_levels_count::<_, 0, 3>"
    )]
    pub sections: [usize; 4],
    /// Radial segments per ring (levels 0–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 0, 3>",
        deserialize_with = "de_levels_count::<_, 0, 3>"
    )]
    pub segments: [usize; 4],
    /// Fraction of the parent's length where children start (levels 1–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 1, 3>",
        deserialize_with = "de_levels_f64::<_, 1, 3>"
    )]
    pub start: [f64; 4],
    /// Radius taper along the branch, deciduous only (levels 0–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 0, 3>",
        deserialize_with = "de_levels_f64::<_, 0, 3>"
    )]
    pub taper: [f64; 4],
    /// Twist per section, radians (levels 0–3).
    #[serde(
        serialize_with = "ser_levels::<_, _, 0, 3>",
        deserialize_with = "de_levels_f64::<_, 0, 3>"
    )]
    pub twist: [f64; 4],
}

impl Default for BranchOptions {
    fn default() -> Self {
        Self {
            levels: 3,
            angle: [0.0, 70.0, 60.0, 60.0],
            children: [7, 7, 5, 0],
            force: ForceOptions::default(),
            gnarliness: [0.15, 0.2, 0.3, 0.02],
            length: [20.0, 20.0, 10.0, 1.0],
            radius: [1.5, 0.7, 0.7, 0.7],
            sections: [12, 10, 8, 6],
            segments: [8, 6, 4, 3],
            start: [0.0, 0.4, 0.3, 0.3],
            taper: [0.7, 0.7, 0.7, 0.7],
            twist: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

/// Leaf parameters (`options.leaves`). The texture map is left to the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeafOptions {
    /// Informational leaf texture identifier (e.g. `"oak"`); not used by the
    /// generator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Single or double (crossed) leaf cards.
    pub billboard: Billboard,
    /// Leaf angle relative to the parent branch, degrees.
    pub angle: f64,
    /// Leaves per final-level branch.
    #[serde(deserialize_with = "de_count")]
    pub count: usize,
    /// Fraction of the branch length where leaves start.
    pub start: f64,
    /// Leaf card size.
    pub size: f64,
    /// Relative size variance (`size * (1 ± sizeVariance)`).
    pub size_variance: f64,
    /// Leaf tint as `0xRRGGBB`.
    pub tint: u32,
    /// Alpha cutoff for the leaf texture (material hint).
    pub alpha_test: f64,
    /// Bend leaf normals outwards from the leaf origin for a rounded canopy.
    pub rounded_normals: bool,
}

impl Default for LeafOptions {
    fn default() -> Self {
        Self {
            kind: "oak".into(),
            billboard: Billboard::Double,
            angle: 10.0,
            count: 1,
            start: 0.0,
            size: 2.5,
            size_variance: 0.7,
            tint: 0xffffff,
            alpha_test: 0.5,
            rounded_normals: true,
        }
    }
}

/// All generator options (`TreeOptions` in `options.js`).
///
/// Deserializing (including [`TreeOptions::from_json`]) *merges* the input
/// into [`TreeOptions::default`] the way ez-tree's `TreeOptions.copy` does:
/// keys missing from the input keep their defaults (e.g. presets don't set
/// `leaves.roundedNormals`, so it stays `true`), and unknown keys (such as
/// `trellis`) are ignored. Trellis support is not ported.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TreeOptions {
    /// RNG seed.
    pub seed: i64,
    /// Tree archetype (JSON key `type`).
    #[serde(rename = "type")]
    pub kind: TreeType,
    /// Bark parameters.
    pub bark: BarkOptions,
    /// Branch parameters.
    pub branch: BranchOptions,
    /// Leaf parameters.
    pub leaves: LeafOptions,
}

/// Strict mirror of [`TreeOptions`] used after merging into the defaults.
#[derive(Deserialize)]
struct RawTreeOptions {
    seed: i64,
    #[serde(rename = "type")]
    kind: TreeType,
    bark: BarkOptions,
    branch: BranchOptions,
    leaves: LeafOptions,
}

impl<'de> Deserialize<'de> for TreeOptions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let source = Value::deserialize(deserializer)?;
        Self::from_value(&source).map_err(D::Error::custom)
    }
}

/// Names of the bundled presets (ez-tree's presets minus `trellis`).
pub const PRESET_NAMES: &[&str] = &[
    "ash_small",
    "ash_medium",
    "ash_large",
    "aspen_small",
    "aspen_medium",
    "aspen_large",
    "bush_1",
    "bush_2",
    "bush_3",
    "oak_small",
    "oak_medium",
    "oak_large",
    "pine_small",
    "pine_medium",
    "pine_large",
];

/// Raw preset JSON, verbatim copies of ez-tree's `src/lib/presets/*.json`.
const PRESET_JSON: &[(&str, &str)] = &[
    ("ash_small", include_str!("../presets/ash_small.json")),
    ("ash_medium", include_str!("../presets/ash_medium.json")),
    ("ash_large", include_str!("../presets/ash_large.json")),
    ("aspen_small", include_str!("../presets/aspen_small.json")),
    ("aspen_medium", include_str!("../presets/aspen_medium.json")),
    ("aspen_large", include_str!("../presets/aspen_large.json")),
    ("bush_1", include_str!("../presets/bush_1.json")),
    ("bush_2", include_str!("../presets/bush_2.json")),
    ("bush_3", include_str!("../presets/bush_3.json")),
    ("oak_small", include_str!("../presets/oak_small.json")),
    ("oak_medium", include_str!("../presets/oak_medium.json")),
    ("oak_large", include_str!("../presets/oak_large.json")),
    ("pine_small", include_str!("../presets/pine_small.json")),
    ("pine_medium", include_str!("../presets/pine_medium.json")),
    ("pine_large", include_str!("../presets/pine_large.json")),
];

impl TreeOptions {
    /// Loads a bundled preset. Accepts the snake_case names in
    /// [`PRESET_NAMES`] as well as ez-tree's display names (`"Ash Large"`).
    pub fn preset(name: &str) -> Option<TreeOptions> {
        let key = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        PRESET_JSON
            .iter()
            .find(|(preset, _)| *preset == key)
            .map(|(_, json)| Self::from_json(json).expect("bundled preset JSON is valid"))
    }

    /// The raw JSON of a bundled preset, by snake_case name.
    pub fn preset_json(name: &str) -> Option<&'static str> {
        PRESET_JSON
            .iter()
            .find(|(preset, _)| *preset == name)
            .map(|(_, json)| *json)
    }

    /// Parses ez-tree JSON (a preset or a saved tree), merging it into the
    /// defaults like `tree.loadFromJson` / `TreeOptions.copy`.
    pub fn from_json(s: &str) -> Result<TreeOptions, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Merges a parsed JSON value into the defaults (see [`TreeOptions`]).
    pub fn from_value(source: &Value) -> Result<TreeOptions, serde_json::Error> {
        let mut merged = serde_json::to_value(TreeOptions::default())?;
        merge(&mut merged, source);
        let raw = RawTreeOptions::deserialize(merged)?;
        Ok(TreeOptions {
            seed: raw.seed,
            kind: raw.kind,
            bark: raw.bark,
            branch: raw.branch,
            leaves: raw.leaves,
        })
    }

    /// Serializes to ez-tree compatible JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("options serialize")
    }
}

/// `TreeOptions.copy(source, target)`: only keys the target already has are
/// copied; plain objects recurse, everything else is assigned.
fn merge(target: &mut Value, source: &Value) {
    let (Value::Object(target), Value::Object(source)) = (target, source) else {
        return;
    };
    for (key, value) in source {
        if let Some(slot) = target.get_mut(key) {
            if value.is_object() {
                merge(slot, value);
            } else {
                *slot = value.clone();
            }
        }
    }
}

fn ser_levels<S, T, const LO: usize, const HI: usize>(
    values: &[T; 4],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: Serialize,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(HI - LO + 1))?;
    for (level, value) in values.iter().enumerate().take(HI + 1).skip(LO) {
        map.serialize_entry(&level.to_string(), value)?;
    }
    map.end()
}

fn de_levels<'de, D, T, const LO: usize, const HI: usize>(
    deserializer: D,
    convert: impl Fn(f64) -> Result<T, String>,
) -> Result<[T; 4], D::Error>
where
    D: Deserializer<'de>,
    T: Default + Copy,
{
    let map = std::collections::BTreeMap::<String, f64>::deserialize(deserializer)?;
    let mut out = [T::default(); 4];
    for (level, slot) in out.iter_mut().enumerate().take(HI + 1).skip(LO) {
        let value = map
            .get(&level.to_string())
            .ok_or_else(|| D::Error::custom(format!("missing level key \"{level}\"")))?;
        *slot = convert(*value).map_err(D::Error::custom)?;
    }
    Ok(out)
}

fn de_levels_f64<'de, D, const LO: usize, const HI: usize>(
    deserializer: D,
) -> Result<[f64; 4], D::Error>
where
    D: Deserializer<'de>,
{
    de_levels::<D, f64, LO, HI>(deserializer, Ok)
}

fn de_levels_count<'de, D, const LO: usize, const HI: usize>(
    deserializer: D,
) -> Result<[usize; 4], D::Error>
where
    D: Deserializer<'de>,
{
    de_levels::<D, usize, LO, HI>(deserializer, count_from_f64)
}

/// Counts may arrive as `7` or `7.0`; anything else is an error.
fn count_from_f64(value: f64) -> Result<usize, String> {
    if value >= 0.0 && value.fract() == 0.0 && value <= u32::MAX as f64 {
        Ok(value as usize)
    } else {
        Err(format!(
            "expected a non-negative integer count, got {value}"
        ))
    }
}

fn de_count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
    count_from_f64(f64::deserialize(deserializer)?).map_err(D::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_options_js() {
        let o = TreeOptions::default();
        assert_eq!(o.seed, 0);
        assert_eq!(o.kind, TreeType::Deciduous);
        assert_eq!(o.branch.levels, 3);
        assert_eq!(o.branch.angle[1..], [70.0, 60.0, 60.0]);
        assert_eq!(o.branch.children[..3], [7, 7, 5]);
        assert_eq!(o.branch.sections, [12, 10, 8, 6]);
        assert_eq!(o.leaves.billboard, Billboard::Double);
        assert!(o.leaves.rounded_normals);
    }

    #[test]
    fn serialization_uses_js_keys() {
        let v = serde_json::to_value(TreeOptions::default()).unwrap();
        assert_eq!(v["type"], "deciduous");
        assert_eq!(v["branch"]["angle"]["1"], 70.0);
        assert!(v["branch"]["angle"].get("0").is_none());
        assert!(v["branch"]["children"].get("3").is_none());
        assert_eq!(v["leaves"]["sizeVariance"], 0.7);
        assert_eq!(v["bark"]["textureScale"]["y"], 1.0);
    }

    #[test]
    fn json_merges_into_defaults() {
        let o = TreeOptions::from_json(
            r#"{ "seed": 5, "type": "evergreen", "branch": { "angle": { "2": 12, "0": 99 } },
                 "leaves": { "count": 4.0 }, "trellis": { "enabled": true } }"#,
        )
        .unwrap();
        assert_eq!(o.seed, 5);
        assert_eq!(o.kind, TreeType::Evergreen);
        // Partial level maps only override the given levels; "0" is not an
        // angle key in options.js, so it is ignored like in TreeOptions.copy.
        assert_eq!(o.branch.angle, [0.0, 70.0, 12.0, 60.0]);
        assert_eq!(o.leaves.count, 4);
        assert_eq!(o.branch.length, BranchOptions::default().length);
        assert!(o.leaves.rounded_normals);
    }

    #[test]
    fn round_trips_through_json() {
        let o = TreeOptions::preset("oak_large").unwrap();
        assert_eq!(TreeOptions::from_json(&o.to_json()).unwrap(), o);
    }

    #[test]
    fn all_presets_load() {
        assert_eq!(PRESET_JSON.len(), PRESET_NAMES.len());
        for name in PRESET_NAMES {
            let o = TreeOptions::preset(name).unwrap_or_else(|| panic!("{name}"));
            assert!(o.leaves.rounded_normals, "{name}");
        }
        assert_eq!(
            TreeOptions::preset("Ash Large"),
            TreeOptions::preset("ash_large")
        );
        assert!(TreeOptions::preset("trellis").is_none());
        let ash = TreeOptions::preset("ash_large").unwrap();
        assert_eq!(ash.seed, 29919);
        assert_eq!(ash.bark.texture_scale, XY { x: 0.5, y: 5.0 });
        assert_eq!(ash.branch.gnarliness[3], 0.049999999999999996);
    }

    #[test]
    fn rejects_fractional_counts() {
        assert!(TreeOptions::from_json(r#"{ "leaves": { "count": 2.5 } }"#).is_err());
    }
}
