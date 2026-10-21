//! Navigation for the engine exports. Skyfold Studio's own navmesh (`analysis::navmesh`: walkable
//! ground, roads and swimmable water from the walkability grid; one layer, so cave floors and
//! upper storeys aren't in it) goes into every export folder for game servers, and Godot builds
//! its NavigationRegion3D from it; Unity and Unreal bake their native navmeshes from the
//! exported collision in their import scripts (see their READMEs).

use bevy::prelude::*;
use serde_json::{Value, json};

use super::core::Ctx;

/// The navmesh files (name, bytes), built now from the world's walkability grid (blocking: the
/// grid is computed if the Analyze tab hasn't got a fresh one). Empty if it can't be built.
pub fn capture(world: &mut World) -> Vec<(String, Vec<u8>)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static RUN: AtomicU32 = AtomicU32::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "skyfold-navmesh-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let t0 = std::time::Instant::now();
    let out = match crate::analysis::export_navmesh(world, &tmp) {
        Ok(st) => {
            info!(
                "export: navmesh {} polygons, {} vertices in {:.1} s",
                st.polygons,
                st.vertices,
                t0.elapsed().as_secs_f32()
            );
            std::fs::read_dir(&tmp)
                .map(|d| {
                    d.flatten()
                        .filter_map(|e| {
                            Some((
                                e.file_name().to_str()?.to_string(),
                                std::fs::read(e.path()).ok()?,
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        Err(e) => {
            warn!("export: no navmesh ({e})");
            vec![]
        }
    };
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

/// Writes the navmesh files into `dir` (below the export root). Returns what was written.
pub fn write(ctx: &Ctx, files: &[(String, Vec<u8>)], dir: &str) -> Result<Value, String> {
    if files.is_empty() {
        return Ok(Value::Null);
    }
    for (name, bytes) in files {
        ctx.write(&format!("{dir}/{name}"), bytes)?;
    }
    let mut names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
    names.sort_unstable();
    Ok(json!({
        "folder": dir,
        "files": names,
        "format": "navmesh.json: world-builder-navmesh v1 (vertices, convex polygons counter-clockwise seen from above with area, cost and neighbours); navmesh.obj: the same polygons grouped by area; walkable_heightfield.png / walkable_areas.png: the walkability grid",
        "note": "one walkable layer from the terrain, roads, bridges and water: cave floors and upper storeys of buildings are not in it",
    }))
}
