//! Automation for ground-level look checks.
//!
//! `WB_VSEQ=name@x,z,yaw,pitch,dist[@key=value…];name2@…` takes a screenshot per step (into
//! `WB_VSEQ_DIR`), each after `WB_VSEQ_STEP` frames (default 150) of settling, starting at frame
//! `WB_VSEQ_START` (default 200, later when a world is loaded first with `WB_MCP_CALL`). Keys:
//! `hour`, `walk=foot|fly|dive` (the explorer stands / hovers / dives at x,z looking along yaw),
//! `y` (metres above the ground when flying, below the sea surface when diving), `look` (the
//! explorer's pitch, radians).

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::camera::OrbitCamera;
use crate::motion::movement::{EYE_DIVE, InWater, MoveMode};
use crate::sky::Lighting;
use crate::terrain::Terrain;
use crate::walk::{WalkStart, WalkState};

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[allow(clippy::too_many_arguments)]
pub fn vis_seq(
    mut frame: Local<u32>,
    mut pending_look: Local<Option<f32>>,
    mut pending_view: Local<Option<[f32; 5]>>,
    mut dive_at: Local<Option<Vec3>>,
    mut commands: Commands,
    mut orbit: ResMut<OrbitCamera>,
    mut lighting: ResMut<Lighting>,
    mut walk: ResMut<WalkState>,
    terrain: Res<Terrain>,
    mut exit: MessageWriter<AppExit>,
    mut actions: MessageWriter<crate::ui::UiAction>,
) {
    let Ok(seq) = std::env::var("WB_VSEQ") else {
        return;
    };
    *frame += 1;
    let start = env_u32("WB_VSEQ_START", 200);
    let step = env_u32("WB_VSEQ_STEP", 150).max(20);
    let f = *frame;
    if f < start {
        return;
    }
    if let Some(v) = *pending_view
        && !walk.active
    {
        orbit.set_view(Vec3::new(v[0], 0.0, v[1]), v[2], v[3], v[4]);
        orbit.snap();
        *pending_view = None;
    }
    if let Some(p) = pending_look.take() {
        if walk.active {
            walk.pitch = p;
        } else {
            *pending_look = Some(p);
        }
    }
    // a diver is held in place under the surface (so the shot frames the same view every time)
    if let Some(p) = *dive_at
        && walk.active
        && walk.request.is_none()
    {
        walk.water = InWater::Diving;
        walk.eye_off = EYE_DIVE;
        walk.position = p;
        walk.velocity = Vec3::ZERO;
    }
    let steps: Vec<&str> = seq.split(';').filter(|x| !x.trim().is_empty()).collect();
    let k = ((f - start) / step) as usize;
    let phase = (f - start) % step;
    if k >= steps.len() {
        if phase == 30 && k == steps.len() {
            exit.write(AppExit::Success);
        }
        return;
    }
    let parts: Vec<&str> = steps[k].split('@').collect();
    if phase == 0 {
        *dive_at = None;
        let v: Vec<f32> = parts
            .get(1)
            .map(|c| c.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or_default();
        let mut walk_mode = None;
        let mut fly_y = 30.0;
        let mut look = None;
        let mut diving = false;
        for kv in parts.iter().skip(2) {
            let Some((key, val)) = kv.split_once('=') else {
                continue;
            };
            match key {
                "hour" => lighting.hour = val.parse().unwrap_or(lighting.hour),
                "walk" => {
                    walk_mode = Some(if val == "fly" {
                        MoveMode::Fly
                    } else {
                        MoveMode::Foot
                    });
                    diving = val == "dive";
                }
                "y" => fly_y = val.parse().unwrap_or(fly_y),
                "look" => look = val.parse().ok(),
                _ => {}
            }
        }
        if v.len() == 5 {
            match walk_mode {
                Some(mode) => {
                    let bed = terrain.height_at(v[0], v[1]);
                    let ground = bed.max(terrain.sea_level);
                    let y = if mode == MoveMode::Fly {
                        ground + fly_y
                    } else {
                        ground
                    };
                    if diving {
                        let eye = (terrain.sea_level - fly_y.min(30.0)).max(bed + 0.7);
                        info!(
                            "VISLOG dive: sea bed {bed:.1}, eye {eye:.1} (sea level {:.1})",
                            terrain.sea_level
                        );
                        *dive_at = Some(Vec3::new(v[0], eye - EYE_DIVE, v[1]));
                    }
                    walk.request = Some(WalkStart {
                        pos: Some(Vec3::new(v[0], y, v[1])),
                        yaw: Some(v[2]),
                        mode: Some(mode),
                        playtest: false,
                    });
                    *pending_look = Some(look.unwrap_or(-0.05));
                }
                None => {
                    if walk.active {
                        // leave walk mode first (it sets its own orbit view on the way out)
                        actions.write(crate::ui::UiAction::ToggleWalk);
                    }
                    *pending_view = Some([v[0], v[1], v[2], v[3], v[4]]);
                }
            }
        }
    }
    if phase == step - 1 {
        let dir = std::env::var("WB_VSEQ_DIR").unwrap_or_else(|_| ".".into());
        let path = format!("{dir}/{}.png", parts[0]);
        info!("VISLOG shot {path}");
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}
