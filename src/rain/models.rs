//! Wet placed models: the glTF models' `StandardMaterial`s darken (porous surfaces soak up water:
//! deeper, slightly richer colour) and turn glossy as `RainState::surface` rises, so roofs, walls,
//! cobbled floors and props catch the sky and the lamps through the existing reflections.
//!
//! A CPU pass, not a shader: model materials are Bevy's own `StandardMaterial` (shared by every
//! instance of a model), so each one is lerped from its cached dry look. It runs only when the
//! wetness has moved a step (`STEP`), a few hundred materials a frame (`PER_FRAME`), and writes a
//! material only when its values change, so a steady drizzle or a dry day costs nothing.
//! Left alone: unlit, emissive (lamps, embers, glowing runes), see-through (glass, water,
//! alpha-blended decals) and transmissive materials; metals keep their colour and only lose a
//! little roughness. A material someone else edits meanwhile becomes its new dry look.
//! Indoors (`motion::indoors`) and on a dungeon map without sky the models are dry: a model's
//! interior shares its materials with its outside.

use std::collections::HashMap;

use bevy::prelude::*;

use super::RainState;
use crate::noise::smoothstep;

/// Wetness change that triggers a pass over the materials.
const STEP: f32 = 1.0 / 32.0;
/// Materials updated per frame during a pass.
const PER_FRAME: usize = 256;
/// Fully soaked porous material: albedo × this.
const WET_DARKEN: f32 = 0.6;
/// Fully soaked porous material: roughness goes towards this.
const WET_ROUGH: f32 = 0.2;

pub(super) fn register(app: &mut App) {
    app.init_resource::<WetMaterials>()
        .add_systems(Update, apply.after(super::track));
}

/// The dry look of one model material, and what the pass last wrote to it.
#[derive(Clone, Copy, Debug)]
struct Dry {
    color: LinearRgba,
    rough: f32,
    /// 1 porous (stone, wood, plaster, cloth) … 0 metal; None: never wetted
    porous: Option<f32>,
    wrote: (LinearRgba, f32),
}

/// Cached dry looks of the model materials, and the pass in progress.
#[derive(Resource, Default)]
pub struct WetMaterials {
    known: HashMap<AssetId<StandardMaterial>, Dry>,
    /// wetness the materials are at (or being moved to)
    applied: f32,
    queue: Vec<AssetId<StandardMaterial>>,
    /// materials written in the pass in progress (logged when it ends)
    written: usize,
}

impl WetMaterials {
    /// A copy of a model material with its dry colour and roughness (for code that clones a
    /// material into another kind, like cloth: the copy must not keep the rain's look).
    pub fn dry_copy(
        &self,
        id: AssetId<StandardMaterial>,
        m: &StandardMaterial,
    ) -> StandardMaterial {
        let mut c = m.clone();
        if let Some(d) = self
            .known
            .get(&id)
            .filter(|d| same(d.wrote, (lin(m.base_color), m.perceptual_roughness)))
        {
            c.base_color = Color::LinearRgba(d.color);
            c.perceptual_roughness = d.rough;
        }
        c
    }
}

fn lin(c: Color) -> LinearRgba {
    c.to_linear()
}

fn same(a: (LinearRgba, f32), b: (LinearRgba, f32)) -> bool {
    let d = (a.0.red - b.0.red)
        .abs()
        .max((a.0.green - b.0.green).abs())
        .max((a.0.blue - b.0.blue).abs())
        .max((a.0.alpha - b.0.alpha).abs());
    d < 1e-5 && (a.1 - b.1).abs() < 1e-5
}

/// How porous a material is (None: leave it alone).
fn porosity(m: &StandardMaterial) -> Option<f32> {
    let e = m.emissive;
    if m.unlit
        || !matches!(m.alpha_mode, AlphaMode::Opaque | AlphaMode::Mask(_))
        || m.specular_transmission > 0.0
        || m.diffuse_transmission > 0.0
        || m.emissive_texture.is_some()
        || e.red.max(e.green).max(e.blue) > 1e-3
    {
        return None;
    }
    Some(1.0 - smoothstep(0.15, 0.7, m.metallic))
}

/// The wet look of a dry colour and roughness at wetness `w`.
fn wet_look(color: LinearRgba, rough: f32, porous: f32, w: f32) -> (LinearRgba, f32) {
    if w <= 0.0 {
        return (color, rough);
    }
    let k = w * porous;
    // darker, and a little richer (water fills the pores: less scattering off the surface)
    let lum = color.red * 0.2126 + color.green * 0.7152 + color.blue * 0.0722;
    let sat = 1.0 + 0.25 * k;
    let dark = 1.0 - (1.0 - WET_DARKEN) * k;
    let ch = |c: f32| ((lum + (c - lum) * sat) * dark).max(0.0);
    let c = LinearRgba::new(ch(color.red), ch(color.green), ch(color.blue), color.alpha);
    // a film of water: glossy; metals were smooth-ish already and only lose a little roughness
    let floor = WET_ROUGH + (0.45 - WET_ROUGH) * (1.0 - porous);
    let r = if rough > floor {
        rough + (floor - rough) * (w * 0.9)
    } else {
        rough
    };
    (c, r)
}

/// Is this one of the models' materials (loaded from a glTF: `…/x.gltf#Material3`)?
fn is_model_material(server: &AssetServer, id: AssetId<StandardMaterial>) -> bool {
    server
        .get_path(id)
        .is_some_and(|p| p.label().is_some_and(|l| l.starts_with("Material")))
}

#[allow(clippy::too_many_arguments)]
fn apply(
    st: Res<RainState>,
    server: Res<AssetServer>,
    mut events: MessageReader<AssetEvent<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut wet: ResMut<WetMaterials>,
    indoors: Option<Res<crate::motion::indoors::Indoors>>,
    interior: Option<Res<crate::dungeons::render::InteriorLook>>,
) {
    let w = &mut *wet;
    // new model materials (their dry look, then the current wetness); forget removed ones
    for ev in events.read() {
        match *ev {
            AssetEvent::Added { id } => {
                if w.known.contains_key(&id) || !is_model_material(&server, id) {
                    continue;
                }
                let Some(m) = materials.get(id) else { continue };
                let (color, rough) = (lin(m.base_color), m.perceptual_roughness);
                w.known.insert(
                    id,
                    Dry {
                        color,
                        rough,
                        porous: porosity(m),
                        wrote: (color, rough),
                    },
                );
                if w.applied > 0.0 {
                    w.queue.push(id);
                }
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                w.known.remove(&id);
            }
            _ => {}
        }
    }

    // how wet the models should be here
    let inside = indoors.map(|i| i.amount.max(i.cave)).unwrap_or(0.0);
    let underground = interior.is_some_and(|i| i.active && !i.sky);
    let target = if underground {
        0.0
    } else {
        st.surface * (1.0 - smoothstep(0.1, 0.6, inside))
    };
    let target = super::quantize(target);
    let moved = (target - w.applied).abs() >= STEP
        || (target == 0.0 && w.applied > 0.0)
        || (target == 1.0 && w.applied < 1.0);
    if moved {
        w.applied = target;
        w.queue = w
            .known
            .iter()
            .filter(|(_, d)| d.porous.is_some())
            .map(|(id, _)| *id)
            .collect();
    }
    if w.queue.is_empty() {
        return;
    }
    let n = w.queue.len().saturating_sub(PER_FRAME);
    let batch: Vec<_> = w.queue.drain(n..).collect();
    for id in batch {
        let Some(m) = materials.get(id) else { continue };
        let Some(d) = w.known.get_mut(&id) else {
            continue;
        };
        let now = (lin(m.base_color), m.perceptual_roughness);
        // edited by someone else since the last write: that is its dry look now
        if !same(now, d.wrote) {
            d.color = now.0;
            d.rough = now.1;
            d.porous = porosity(m);
        }
        let Some(porous) = d.porous else {
            d.wrote = now;
            continue;
        };
        let want = wet_look(d.color, d.rough, porous, w.applied);
        if !same(now, want)
            && let Some(mut m) = materials.get_mut(id)
        {
            m.base_color = Color::LinearRgba(want.0);
            m.perceptual_roughness = want.1;
            w.written += 1;
        }
        d.wrote = want;
    }
    if w.queue.is_empty() && w.written > 0 {
        info!(
            "RAIN wet models: {} materials at wetness {:.2} ({} model materials known)",
            w.written,
            w.applied,
            w.known.len()
        );
        w.written = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_is_untouched_and_wet_is_darker_and_glossier() {
        let c = LinearRgba::new(0.4, 0.3, 0.2, 1.0);
        assert!(same(wet_look(c, 0.9, 1.0, 0.0), (c, 0.9)));
        let (wc, wr) = wet_look(c, 0.9, 1.0, 1.0);
        assert!(wc.green < c.green * 0.7 && wr < 0.35);
        // metals keep their colour
        let (mc, mr) = wet_look(c, 0.6, 0.0, 1.0);
        assert!(same((mc, 0.0), (c, 0.0)) && mr < 0.6 && mr > 0.4);
        // already glossy stays as it is
        assert!((wet_look(c, 0.1, 1.0, 1.0).1 - 0.1).abs() < 1e-6);
    }

    #[test]
    fn lamps_and_glass_stay_dry() {
        let lamp = StandardMaterial {
            emissive: LinearRgba::rgb(3.0, 2.0, 1.0),
            ..default()
        };
        let glass = StandardMaterial {
            alpha_mode: AlphaMode::Blend,
            ..default()
        };
        assert!(porosity(&lamp).is_none() && porosity(&glass).is_none());
        assert_eq!(
            porosity(&StandardMaterial {
                metallic: 0.0,
                ..default()
            }),
            Some(1.0)
        );
    }
}
