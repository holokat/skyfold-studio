//! Saved, world-wide controls for nature that change its look without changing painted density.

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

/// How automatic forests select their tree species when diversity is enabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForestLayout {
    /// Related species occur in broad deterministic patches.
    #[default]
    NaturalGroves,
    /// Adjacent tree slots deliberately vary their species.
    Mixed,
}

impl ForestLayout {
    pub const ALL: [Self; 2] = [Self::NaturalGroves, Self::Mixed];

    pub fn name(self) -> &'static str {
        match self {
            Self::NaturalGroves => "Natural groves",
            Self::Mixed => "Mixed colors",
        }
    }
}

/// Forest-specific part of [`NatureAppearance`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForestDiversity {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub layout: ForestLayout,
}

/// The ground cover (`crate::groundcover`): photo grass tufts in the meadows and forest undergrowth
/// (ferns and broad-leaved herbs) under the crowns, grown in clumps around the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundCover {
    /// ferns and herbs under the trees, 0..2 (1: a full woodland floor)
    #[serde(default = "full")]
    pub undergrowth: f32,
    /// the undergrowth's mix, 0 all ferns .. 1 all broad-leaved herbs
    #[serde(default = "half")]
    pub herbs: f32,
    /// photo grass tufts among the meadow blades, 0..2
    #[serde(default = "full")]
    pub tufts: f32,
    /// how much meadow grass still grows under the crowns, 0..1
    #[serde(default = "grass_under_trees")]
    pub grass_under_trees: f32,
    /// 0 an even spread .. 1 dense patches with bare ground between them
    #[serde(default = "clumping")]
    pub clumping: f32,
}

fn half() -> f32 {
    0.5
}
fn grass_under_trees() -> f32 {
    0.25
}
fn clumping() -> f32 {
    0.6
}

impl Default for GroundCover {
    fn default() -> Self {
        Self {
            undergrowth: 1.0,
            herbs: half(),
            tufts: 1.0,
            grass_under_trees: grass_under_trees(),
            clumping: clumping(),
        }
    }
}

impl GroundCover {
    fn sanitized(self) -> Self {
        let d = Self::default();
        let c = |v: f32, def: f32, max: f32| if v.is_finite() { v.clamp(0.0, max) } else { def };
        Self {
            undergrowth: c(self.undergrowth, d.undergrowth, 2.0),
            herbs: c(self.herbs, d.herbs, 1.0),
            tufts: c(self.tufts, d.tufts, 2.0),
            grass_under_trees: c(self.grass_under_trees, d.grass_under_trees, 1.0),
            clumping: c(self.clumping, d.clumping, 1.0),
        }
    }
}

/// A world-level nature appearance. Zone overrides only replace `forest`; grass remains global.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NatureAppearance {
    #[serde(default)]
    pub forest: ForestDiversity,
    #[serde(default)]
    pub grass_lushness: f32,
    /// Shade under the trees (`shade`): 0 keeps the open sky's light under the crowns, 1 a real
    /// forest's.
    #[serde(default = "full")]
    pub canopy_shade: f32,
    #[serde(default)]
    pub ground: GroundCover,
    /// Plants recoloured to an alien hue (`alien`), off by default.
    #[serde(default)]
    pub alien: super::alien::AlienFlora,
}

fn full() -> f32 {
    1.0
}

impl Default for NatureAppearance {
    fn default() -> Self {
        Self {
            forest: ForestDiversity::default(),
            grass_lushness: 0.0,
            canopy_shade: 1.0,
            ground: GroundCover::default(),
            alien: Default::default(),
        }
    }
}

impl NatureAppearance {
    /// Old or malformed documents retain the legacy look.
    pub fn sanitized(self) -> Self {
        Self {
            grass_lushness: if self.grass_lushness.is_finite() {
                self.grass_lushness.clamp(0.0, 1.0)
            } else {
                0.0
            },
            canopy_shade: if self.canopy_shade.is_finite() {
                self.canopy_shade.clamp(0.0, 1.0)
            } else {
                1.0
            },
            ground: self.ground.sanitized(),
            alien: self.alien.sanitized(),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_document_defaults_to_the_baseline_appearance() {
        let appearance: NatureAppearance = serde_json::from_str("{}").unwrap();
        assert_eq!(appearance, NatureAppearance::default());
        assert!(!appearance.forest.enabled);
        assert_eq!(appearance.forest.layout, ForestLayout::NaturalGroves);
    }

    #[test]
    fn appearance_serializes_the_public_layout_names_and_sanitizes_lushness() {
        let appearance = NatureAppearance {
            forest: ForestDiversity {
                enabled: true,
                layout: ForestLayout::Mixed,
            },
            grass_lushness: f32::INFINITY,
            canopy_shade: f32::NAN,
            ground: GroundCover {
                undergrowth: 9.0,
                ..GroundCover::default()
            },
            alien: Default::default(),
        };
        assert_eq!(appearance.sanitized().ground.undergrowth, 2.0);
        assert_eq!(appearance.sanitized().canopy_shade, 1.0);
        assert_eq!(appearance.sanitized().grass_lushness, 0.0);
        assert_eq!(
            serde_json::to_value(appearance.forest).unwrap()["layout"],
            "mixed"
        );
    }
}
