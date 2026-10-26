//! Undoable canyonlands edits shared by Sculpt › Canyonlands and the `terrain_canyonlands` MCP
//! tool: snapshot the ground, generate (`canyons`), and optionally fill the lake basins on the
//! canyon floors with water, each lake at its own tier's height.

use bevy::prelude::*;

use super::Terrain;
use super::canyons::{self, CanyonParams, CanyonReport, LakeSite};
use crate::history::History;
use crate::objects::{Library, ObjectStore};
use crate::water::WaterBodies;

/// Canyonlands over the whole world (`area` None; it also turns on banded cliff rock,
/// `StyleRules::strata_bands`) or a disc (`area` = centre, radius m), with lakes in the basins
/// when `fill_lakes`. Records into the history step being recorded, or makes
/// its own undo step ("Canyonlands") when none is. None when the disc misses the world.
#[allow(clippy::too_many_arguments)]
pub fn canyonlands(
    t: &mut Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    lib: Option<&Library>,
    water: &mut WaterBodies,
    params: &CanyonParams,
    seed: u32,
    area: Option<(Vec2, f32)>,
    fill_lakes: bool,
) -> Option<(CanyonReport, usize)> {
    let rect = match area {
        Some((c, r)) => canyons::canyon_rect(t, c, r)?,
        None => super::GridRect {
            x0: 0,
            z0: 0,
            x1: t.n() - 1,
            z1: t.n() - 1,
        },
    };
    let own_step = !h.is_recording();
    if own_step {
        h.begin("Canyonlands");
    }
    h.touch_terrain(t, rect);
    let report = match area {
        Some((c, r)) => canyons::generate_canyonlands_region(t, params, seed, c, r),
        None => Some(canyons::generate_canyonlands(t, params, seed)),
    };
    // the whole world as canyonlands: its cliffs show banded rock layers
    if area.is_none() && report.is_some() {
        t.style.strata_bands = t.style.strata_bands.max(0.8);
    }
    let lakes = match (&report, fill_lakes) {
        (Some(rep), true) => fill_basins(t, h, store, lib, water, &rep.lake_sites),
        _ => 0,
    };
    if own_step {
        h.commit(store);
    }
    report.map(|r| (r, lakes))
}

/// Water in each basin, a metre below its rim: one lake per site. Returns how many were made.
pub fn fill_basins(
    t: &mut Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    lib: Option<&Library>,
    water: &mut WaterBodies,
    sites: &[LakeSite],
) -> usize {
    let Some(shape) = crate::water::lakes::SHAPES.iter().find(|s| s.id == "lake") else {
        return 0;
    };
    let mut made = 0;
    for s in sites {
        // the shore loop just inside the dug bowl (its edge wanders by up to ~45%)
        let outline: Vec<Vec2> = (0..24)
            .map(|k| s.center + Vec2::from_angle(k as f32 / 24.0 * std::f32::consts::TAU) * s.radius * 0.6)
            .collect();
        if crate::water::create_drawn_lake_at_level(
            t,
            h,
            store,
            lib,
            water,
            &outline,
            shape,
            0.6,
            0.0,
            Some(s.rim - 1.2),
        )
        .is_ok()
        {
            made += 1;
        }
    }
    made
}
