//! The camera's own optics, the part of the cinematic look that belongs to the eye or the lens
//! rather than to the light: eye adaptation, sun glare and lens flare, depth of field.
//!
//! * `meter`: a light meter on the GPU. One small compute dispatch a frame reads a grid of the HDR
//!   view into a centre-weighted histogram of *scene* luminance (the frame's exposure taken out)
//!   and the depth round the centre of the view, and reads the result back (a few frames late).
//! * `adaptation`: the eye. The procedural exposure (`sky::update_sun_moon`: sun height, night,
//!   weather, indoors, caves) stays the anchor; adaptation opens or closes up to about ±1.5 EV
//!   round it as what is on screen is darker or brighter than that exposure expects (a shaded
//!   market lane, under an arcade, looking out of a door), quickly into the light and slowly into
//!   the dark, the way eyes do. Lighting › Camera › Eye adaptation, MCP `lighting_set`
//!   `eye_adaptation`.
//! * `flare`: when the camera looks towards a bright sun, a soft wide veiling glare, a faint
//!   starburst and a chain of ghosts along the line from the sun through the centre of the view,
//!   hidden by whatever stands in front of the sun and fading with cloud over it. The look's
//!   `glare` and `flare` (Style › Fine-tune › Lens; `look_set` params).
//! * `dof`: Bevy's bokeh depth of field for cinematic shots only (camera paths, or stills in the
//!   editor view), focused on what is at the centre of the view unless a distance is given.
//!   Lighting › Camera › Depth of field, MCP `lighting_set` `depth_of_field`.

pub mod adaptation;
pub mod dof;
pub mod flare;
pub mod meter;

use bevy::prelude::*;

pub use adaptation::{Adaptation, EyeAdaptation, INDOOR_SHARE};
pub use dof::{DepthOfFieldMode, DepthOfFieldSettings};

pub struct CameraFxPlugin;

impl Plugin for CameraFxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((meter::MeterPlugin, flare::LensFlarePlugin))
            .init_resource::<Adaptation>()
            .add_systems(
                Update,
                adaptation::adapt
                    .before(crate::sky::SkySystems)
                    .after(crate::camera::CameraSystems),
            )
            .add_systems(
                PostUpdate,
                dof::update_dof.before(bevy::transform::TransformSystems::Propagate),
            );
    }
}
