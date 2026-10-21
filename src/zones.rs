//! Zone bookkeeping: label anchors / areas computed from the painted zone map.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::mcp_tools::McpAppExt;
use crate::terrain::Terrain;

pub mod auto;

#[derive(Resource, Default)]
pub struct ZoneStats {
    /// World-space label anchor (centre of mass, on the ground) per zone id.
    pub anchor: HashMap<u8, Vec3>,
    /// Area in km² per zone id.
    pub area_km2: HashMap<u8, f32>,
    revision: u64,
}

pub struct ZonesPlugin;

impl Plugin for ZonesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneStats>()
            .init_resource::<auto::AutoZones>()
            .mcp_tool(auto::mcp_tool())
            .add_systems(Update, auto::drive.after(crate::tools::ToolSystems::Commit))
            .add_systems(Update, update_stats);
    }
}

fn update_stats(
    terrain: Res<Terrain>,
    mut stats: ResMut<ZoneStats>,
    time: Res<Time>,
    mut cooldown: Local<f32>,
) {
    *cooldown -= time.delta_secs();
    if stats.revision == terrain.zone_revision || *cooldown > 0.0 {
        return;
    }
    *cooldown = 0.25;
    stats.revision = terrain.zone_revision;
    // sample every few vertices: plenty for label placement
    let step = (terrain.res / 256).max(1);
    let mut sum: HashMap<u8, (Vec2, f32)> = HashMap::new();
    for j in (0..terrain.n()).step_by(step) {
        for i in (0..terrain.n()).step_by(step) {
            let z = terrain.zones[terrain.idx(i, j)];
            if z > 0 {
                let e = sum.entry(z).or_default();
                e.0 += terrain.grid_to_world(i, j);
                e.1 += 1.0;
            }
        }
    }
    stats.anchor.clear();
    stats.area_km2.clear();
    let cell_area = (terrain.cell * step as f32).powi(2) / 1.0e6;
    for (z, (s, c)) in sum {
        let mut p = s / c;
        // if the centroid falls outside the zone (crescent shapes), snap to the nearest member
        if terrain.zone_at(p.x, p.y) != z {
            let mut best = (f32::MAX, p);
            for j in (0..terrain.n()).step_by(step) {
                for i in (0..terrain.n()).step_by(step) {
                    if terrain.zones[terrain.idx(i, j)] == z {
                        let q = terrain.grid_to_world(i, j);
                        let d = q.distance_squared(p);
                        if d < best.0 {
                            best = (d, q);
                        }
                    }
                }
            }
            p = best.1;
        }
        let h = terrain.height_at(p.x, p.y).max(terrain.sea_level);
        stats.anchor.insert(z, Vec3::new(p.x, h, p.y));
        stats.area_km2.insert(z, c * cell_area);
    }
}
