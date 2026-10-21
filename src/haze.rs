//! One resolved outdoor-haze state. Worlds keep a master setting; painted zones can inherit it,
//! force it on with their own amount, or force it off without changing their lighting mood.

use serde::{Deserialize, Serialize};

/// A zone's relationship to the world's outdoor haze setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HazeMode {
    #[default]
    Inherit,
    On,
    Off,
}

impl HazeMode {
    pub const ALL: [Self; 3] = [Self::Inherit, Self::On, Self::Off];

    pub fn name(self) -> &'static str {
        match self {
            Self::Inherit => "Use world setting",
            Self::On => "On",
            Self::Off => "Off",
        }
    }
}

/// Saved zone override. `amount` stays available while the mode is Inherit or Off, ready for a
/// later On selection.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneHaze {
    pub mode: HazeMode,
    /// Used only when [`HazeMode::On`].
    pub amount: f32,
}

impl Default for ZoneHaze {
    fn default() -> Self {
        Self {
            mode: HazeMode::Inherit,
            amount: 0.15,
        }
    }
}

/// The resolved outdoor haze for the current viewer. `strength` is blended across zone borders;
/// `amount` is the unblended source amount (its product with `strength` is what renders).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HazeState {
    pub strength: f32,
    pub amount: f32,
    /// The zone providing an explicit On or Off override, if any.
    pub source_zone: Option<u8>,
}

impl HazeState {
    pub fn effective_amount(self) -> f32 {
        self.strength * self.amount
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let strength = self.strength + (other.strength - self.strength) * t;
        // Blend the rendered value, rather than raw amounts. Otherwise crossing from an off
        // zone which remembered a large amount into a low-on zone briefly makes the haze denser
        // than either endpoint.
        let rendered =
            self.effective_amount() + (other.effective_amount() - self.effective_amount()) * t;
        let raw_amount = self.amount + (other.amount - self.amount) * t;
        Self {
            strength,
            amount: if strength > 1e-4 {
                (rendered / strength).clamp(0.0, 1.0)
            } else {
                raw_amount.clamp(0.0, 1.0)
            },
            source_zone: if t < 0.5 {
                self.source_zone
            } else {
                other.source_zone
            },
        }
    }
}

/// Resolves a raw painted-zone override against the saved world setting. This deliberately does
/// not depend on a zone's lighting mood, so haze can be controlled on its own.
pub fn resolve(world_enabled: bool, world_amount: f32, zone: Option<(u8, ZoneHaze)>) -> HazeState {
    let world = HazeState {
        strength: if world_enabled { 1.0 } else { 0.0 },
        amount: world_amount.clamp(0.0, 1.0),
        source_zone: None,
    };
    match zone {
        Some((
            id,
            ZoneHaze {
                mode: HazeMode::On,
                amount,
            },
        )) => HazeState {
            strength: 1.0,
            amount: amount.clamp(0.0, 1.0),
            source_zone: Some(id),
        },
        Some((
            id,
            ZoneHaze {
                mode: HazeMode::Off,
                ..
            },
        )) => HazeState {
            strength: 0.0,
            amount: world.amount,
            source_zone: Some(id),
        },
        _ => world,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_on_can_override_a_world_off() {
        let h = resolve(
            false,
            0.2,
            Some((
                7,
                ZoneHaze {
                    mode: HazeMode::On,
                    amount: 0.8,
                },
            )),
        );
        assert_eq!((h.strength, h.amount, h.source_zone), (1.0, 0.8, Some(7)));
    }

    #[test]
    fn zone_off_can_override_a_world_on_without_losing_its_amount() {
        let saved = ZoneHaze {
            mode: HazeMode::Off,
            amount: 0.7,
        };
        assert_eq!(resolve(true, 0.2, Some((7, saved))).strength, 0.0);
        assert_eq!(
            resolve(
                true,
                0.2,
                Some((
                    7,
                    ZoneHaze {
                        mode: HazeMode::On,
                        ..saved
                    }
                ))
            )
            .effective_amount(),
            0.7
        );
    }

    #[test]
    fn inheritance_and_boundary_fade_keep_the_world_amount() {
        let world = resolve(true, 0.3, Some((7, ZoneHaze::default())));
        assert_eq!(
            (world.strength, world.amount, world.source_zone),
            (1.0, 0.3, None)
        );
        let off = resolve(
            true,
            0.3,
            Some((
                7,
                ZoneHaze {
                    mode: HazeMode::Off,
                    amount: 0.8,
                },
            )),
        );
        assert_eq!(
            world.lerp(off, 0.5),
            HazeState {
                strength: 0.5,
                amount: 0.3,
                source_zone: Some(7)
            }
        );
    }

    #[test]
    fn boundary_fade_never_overshoots_the_rendered_haze_amount() {
        let off = resolve(false, 0.9, None);
        let low_on = resolve(
            false,
            0.9,
            Some((
                7,
                ZoneHaze {
                    mode: HazeMode::On,
                    amount: 0.2,
                },
            )),
        );
        assert_eq!(off.lerp(low_on, 0.5).effective_amount(), 0.1);
        assert_eq!(low_on.lerp(off, 0.5).effective_amount(), 0.1);
    }
}
