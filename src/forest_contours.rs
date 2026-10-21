//! Optional distance-aware softening for dense forest silhouettes.

use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin, StandardMaterial};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};

/// Mesh tag read by the forest prepass shader. Keep this value in sync with
/// `FOREST_TAG` in `forest_prepass.wesl`.
pub const FOREST_TAG: u32 = 0x4652_5354; // ASCII "FRST"

/// A canopy-only material that writes the forest tag into the normal prepass without foliage
/// wind. Its base is otherwise a regular [`StandardMaterial`].
pub type ForestMaskMaterial = ExtendedMaterial<StandardMaterial, ForestMaskExt>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct ForestMaskExt {}

impl MaterialExtension for ForestMaskExt {
    fn prepass_fragment_shader() -> ShaderRef {
        "shaders/forest_prepass.wesl".into()
    }
}

/// Registers the canopy material used by the distant-forest normal mask.
pub struct ForestMaskPlugin;

impl Plugin for ForestMaskPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<ForestMaskMaterial>::default());
    }
}

/// Settings for blending dense distant tree contours while preserving nearby trees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ForestContours {
    pub enabled: bool,
    /// Contour blend amount, normalized to 0..1.
    pub strength: f32,
    /// Distance in metres where the blend begins.
    pub start_distance: f32,
}

impl ForestContours {
    pub const MIN_START_DISTANCE: f32 = 30.0;
    pub const MAX_START_DISTANCE: f32 = 1_000.0;

    /// Keeps values supplied outside the editor controls within the supported range.
    pub fn clamp(&mut self) {
        let defaults = Self::default();
        self.strength = if self.strength.is_finite() {
            self.strength.clamp(0.0, 1.0)
        } else {
            defaults.strength
        };
        self.start_distance = if self.start_distance.is_finite() {
            self.start_distance
                .clamp(Self::MIN_START_DISTANCE, Self::MAX_START_DISTANCE)
        } else {
            defaults.start_distance
        };
    }
}

impl Default for ForestContours {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 0.65,
            start_distance: 120.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_missing_serialized_fields() {
        let contours: ForestContours = serde_json::from_str("{}").unwrap();
        assert_eq!(contours, ForestContours::default());
    }

    #[test]
    fn old_world_lighting_stays_off_and_enabled_world_roundtrips() {
        let mut lighting: crate::sky::Lighting = serde_json::from_str(r#"{"hour":14.0}"#).unwrap();
        assert!(!lighting.forest_contours.enabled);
        lighting.forest_contours = ForestContours {
            enabled: true,
            strength: 0.8,
            start_distance: 230.0,
        };
        let saved = serde_json::to_string(&lighting).unwrap();
        let restored: crate::sky::Lighting = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.forest_contours, lighting.forest_contours);
        assert_eq!(restored.hour, 14.0);
    }

    #[test]
    fn clamps_editor_ranges() {
        let mut contours = ForestContours {
            enabled: true,
            strength: 2.0,
            start_distance: 1.0,
        };

        contours.clamp();

        assert_eq!(contours.strength, 1.0);
        assert_eq!(contours.start_distance, ForestContours::MIN_START_DISTANCE);
    }

    #[test]
    fn resets_non_finite_values() {
        let mut contours = ForestContours {
            enabled: true,
            strength: f32::NAN,
            start_distance: f32::INFINITY,
        };

        contours.clamp();

        assert_eq!(contours.strength, ForestContours::default().strength);
        assert_eq!(
            contours.start_distance,
            ForestContours::default().start_distance
        );
    }
}
