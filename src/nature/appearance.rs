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

/// A world-level nature appearance. Zone overrides only replace `forest`; grass remains global.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NatureAppearance {
    #[serde(default)]
    pub forest: ForestDiversity,
    #[serde(default)]
    pub grass_lushness: f32,
}

impl Default for NatureAppearance {
    fn default() -> Self {
        Self {
            forest: ForestDiversity::default(),
            grass_lushness: 0.0,
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
        };
        assert_eq!(appearance.sanitized().grass_lushness, 0.0);
        assert_eq!(
            serde_json::to_value(appearance.forest).unwrap()["layout"],
            "mixed"
        );
    }
}
