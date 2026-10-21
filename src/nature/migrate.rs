//! Old worlds stored every tree, rock, plant and piece of debris as an object (hundreds of
//! thousands). On load those become density instead: each species' objects are counted on the
//! 8 m density grid, softened over a few texels (the old random scatter's noise averages out,
//! clearings and painted patches stay) and painted as that species' density. The rules stay off
//! where nothing was painted, so the converted world grows what it had, where it had it — until
//! an area is regrown (biome brush) or the world is populated again. Lake shore plants stay
//! objects (their lake owns them).

use std::collections::HashMap;

use bevy::prelude::*;

use super::field::{CHUNK, NatureField, TEXEL, texel_of};
use super::rules::cat_index;
use crate::objects::{Library, Placed, kind_category};
use crate::procgen::meshes::AssetKind;
use crate::water::WaterBodies;

/// Nature objects of an old world waiting to become density (set by `io::load_world`).
#[derive(Resource, Default)]
pub struct Pending(pub Option<Vec<Placed>>);

/// Splits a loaded old world's objects into the ones to keep and the nature to convert.
pub fn split(
    objects: Vec<Placed>,
    lib: Option<&Library>,
    keep_ids: &std::collections::HashSet<u64>,
) -> (Vec<Placed>, Vec<Placed>) {
    let Some(lib) = lib else {
        return (objects, vec![]);
    };
    objects.into_iter().partition(|o| {
        keep_ids.contains(&o.id)
            || !lib
                .get(&o.asset)
                .is_some_and(|e| e.kind.is_some() && cat_index(e.category).is_some())
    })
}

/// Converts pending nature objects once the library exists.
pub fn run_pending(
    mut pending: ResMut<Pending>,
    mut field: ResMut<NatureField>,
    water: Res<WaterBodies>,
    lib: Option<Res<Library>>,
    mut ed: ResMut<crate::editor::Editor>,
    mut meta: ResMut<crate::world::WorldMeta>,
) {
    if pending.0.is_none() || water.dirty {
        return;
    }
    let Some(lib) = lib else { return };
    let objects = pending.0.take().unwrap_or_default();
    let t0 = std::time::Instant::now();
    let n = objects.len();
    let species = convert(&objects, &mut field, &lib);
    // saving writes the compact form
    meta.dirty = true;
    info!(
        "nature: converted {n} nature objects ({species} species) into density in {:?}",
        t0.elapsed()
    );
    ed.toast(format!("This world's {n} trees, rocks and plants are now procedural nature (saved as density maps from now on)"));
}

/// Softening radius (texels) per category: sparse trees, rocks and debris average over more.
const SOFT: [i32; 4] = [2, 2, 1, 2];

fn convert(objects: &[Placed], field: &mut NatureField, lib: &Library) -> usize {
    // per (chunk, species) a dense grid of density (per m²)
    let n = CHUNK as i32;
    let mut grids: HashMap<((i32, i32), AssetKind), Box<[f32]>> = HashMap::new();
    for o in objects {
        let Some(k) = lib.get(&o.asset).and_then(|e| e.kind) else {
            continue;
        };
        let Some(cat) = cat_index(kind_category(k)) else {
            continue;
        };
        let r = SOFT[cat];
        let w = 1.0 / (((2 * r + 1) * (2 * r + 1)) as f32 * TEXEL * TEXEL);
        let t = texel_of(o.pos[0], o.pos[2]);
        for dz in -r..=r {
            for dx in -r..=r {
                let q = t + IVec2::new(dx, dz);
                let c = (q.x.div_euclid(n), q.y.div_euclid(n));
                let g = grids
                    .entry((c, k))
                    .or_insert_with(|| vec![0.0; CHUNK * CHUNK].into_boxed_slice());
                g[(q.y.rem_euclid(n) * n + q.x.rem_euclid(n)) as usize] += w;
            }
        }
    }
    let species: std::collections::HashSet<AssetKind> = grids.keys().map(|(_, k)| *k).collect();
    // the converted world grows what it had, where it had it
    field.chunks.clear();
    field.rules_off = true;
    for (((cx, cz), kind), g) in grids {
        for (i, d) in g.iter().enumerate() {
            if *d > 0.0 {
                let t = IVec2::new(cx * n + (i as i32 % n), cz * n + (i as i32 / n));
                field.set_add(t, kind, *d);
            }
        }
    }
    field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    species.len()
}
