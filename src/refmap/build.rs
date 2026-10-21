//! Building the draft world from the analysed picture, as one undo step: the heights (see
//! `relief`), the biome map and its ground with organic borders, lakes and rivers through the
//! Water tab's own code, biome vegetation, and (only when asked) a zone per region.

use std::sync::Arc;
use std::time::Instant;

use bevy::prelude::*;
use rayon::prelude::*;
use serde_json::{Value, json};

use super::analyse::{Analysis, NONE, Role};
use super::relief::{self, Draft, Grid};
use super::{BuildSettings, Placement, RefMap};
use crate::biomes::{BiomePreset, ground_target, restyle_water};
use crate::io::WorldData;
use crate::objects::Category;
use crate::terrain::GridRect;
use crate::terrain::layers::LAYER_COUNT;
use crate::world::Zone;

/// What to build.
pub struct BuildRequest {
    /// A new world of this size (m) instead of the open one.
    pub new_size: Option<f32>,
    pub settings: BuildSettings,
    pub seed: u32,
    /// File › New's style draft when this build replaces the current world.
    pub art_style: Option<crate::art_style::ArtStyle>,
    pub appearance: Option<crate::nature::NatureAppearance>,
}

/// The biome a region asked for.
pub fn preset_of(name: &str) -> BiomePreset {
    BiomePreset::ALL
        .into_iter()
        .find(|b| format!("{b:?}") == name)
        .unwrap_or_default()
}

/// Builds the draft. Returns a summary for the person or agent.
pub fn build(world: &mut World, req: BuildRequest) -> Result<Value, String> {
    let t0 = Instant::now();
    world.resource_mut::<RefMap>().analyse_now()?;
    let t_analyse = t0.elapsed().as_secs_f32();

    // a new world of the asked size (it starts as open ocean; the picture stays)
    if let Some(size) = req.new_size {
        let res = ((size / 4.0 / 16.0).round() as usize * 16).clamp(256, 4096);
        let keep = std::mem::take(&mut *world.resource_mut::<RefMap>());
        world
            .run_system_cached_with(new_world, (res, req.seed, req.art_style, req.appearance))
            .map_err(|e| e.to_string())?;
        let mut r = world.resource_mut::<RefMap>();
        *r = keep;
        // The File › New draft only becomes spent once its new world exists. Keeping it until
        // here lets a failed analysis be corrected and retried with the same chosen style.
        r.new_world_art_style = None;
        r.new_world_appearance = None;
        let (aspect, margin) = (r.aspect(), r.doc.build.margin);
        r.doc.placement = Placement::fit(res as f32 * 4.0, aspect, margin);
        // the next build reuses this world
        r.doc.build.world_size = 0.0;
    }

    let (an, base, place) = {
        let r = world.resource::<RefMap>();
        (
            r.analysis.clone().ok_or("no analysis")?,
            r.base.clone().ok_or("no analysis")?,
            r.doc.placement,
        )
    };
    if an.regions.is_empty() {
        return Err("the picture shows no land: check the ignore areas, or the sensitivity".into());
    }
    let grid = {
        let t = world.resource::<crate::terrain::Terrain>();
        Grid {
            res: t.res,
            cell: t.cell,
            sea: t.sea_level,
        }
    };
    let t1 = Instant::now();
    let draft = relief::relief(&an, &base, &place, &grid, &req.settings.relief, req.seed);
    let t_relief = t1.elapsed().as_secs_f32();
    let t2 = Instant::now();
    let job = ApplyJob {
        draft,
        an: an.clone(),
        density: world.resource::<RefMap>().density(),
        settings: req.settings.clone(),
        seed: req.seed,
        place,
    };
    let mut report = world
        .run_system_cached_with(apply, job)
        .map_err(|e| e.to_string())??;
    let t_apply = t2.elapsed().as_secs_f32();
    {
        let mut r = world.resource_mut::<RefMap>();
        // show the result, not the tracing paper
        r.doc.visible = false;
    }
    report["seconds"] = json!({ "analyse": round2(t_analyse), "heights": round2(t_relief), "apply": round2(t_apply), "total": round2(t0.elapsed().as_secs_f32()) });
    info!("REFMAP build {}", report);
    Ok(report)
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn new_world(
    In((res, seed, art_style, appearance)): In<(
        usize,
        u32,
        Option<crate::art_style::ArtStyle>,
        Option<crate::nature::NatureAppearance>,
    )>,
    mut d: WorldData,
) {
    crate::io::new_world_styled(
        &mut d,
        crate::terrain::worldgen::WorldPreset::Ocean,
        res,
        4.0,
        seed,
        art_style.unwrap_or_default(),
    );
    if let Some(appearance) = appearance {
        *d.appearance = appearance.sanitized();
    }
}

struct ApplyJob {
    draft: Draft,
    an: Arc<Analysis>,
    density: Option<super::DensityMap>,
    settings: BuildSettings,
    seed: u32,
    place: Placement,
}

/// Writes the draft into the world (one undo step).
fn apply(In(job): In<ApplyJob>, mut d: WorldData) -> Result<Value, String> {
    let ApplyJob {
        draft,
        an,
        density,
        settings,
        seed,
        place,
    } = job;
    let WorldData {
        terrain,
        zones,
        store,
        water,
        meta,
        history,
        lib,
        amounts,
        appearance,
        orbit,
        ed,
        nature,
        nature_rules,
        nature_masks,
        ..
    } = &mut d;
    let t = &mut **terrain;
    let n = t.n();
    let all = GridRect {
        x0: 0,
        z0: 0,
        x1: n - 1,
        z1: n - 1,
    };
    history.begin("Build from picture");
    history.touch_terrain(t, all);
    history.touch_water(water);
    if settings.zones {
        history.touch_zones(zones);
    }

    // the old nature and water belong to the old landscape (structures stay and follow the ground)
    if let Some(lib) = lib.as_deref() {
        let ids: Vec<u64> = store
            .items
            .values()
            .filter(|o| {
                lib.get(&o.asset)
                    .is_some_and(|e| Category::NATURE.contains(&e.category))
            })
            .map(|o| o.id)
            .collect();
        for id in ids {
            history.touch_object(store, id);
            store.remove(id);
        }
    }
    water.rivers.clear();
    water.lakes.clear();
    water.waterfalls.clear();

    // --- ground
    t.heights = draft.heights;
    t.mark_all_dirty();
    let peak = settings.relief.peak_for(t.size());
    let has_peaks = an.regions.iter().any(|r| r.role == Role::Peaks);
    // snow on the peaks, not on every mountain
    t.style.snow_line = if has_peaks {
        (peak * 0.8).max(120.0)
    } else {
        t.style.snow_line.max(peak * 1.1)
    };
    {
        let (terrain_ref, water_ref) = (&*t, &mut **water);
        water_ref.fish.retain(|f| {
            terrain_ref.height_at(f.center[0], f.center[2]) < terrain_ref.sea_level - 1.0
        });
    }

    // --- biome map and ground: each vertex its region's biome; the ground blends over the
    // wandering borders
    let biome_of: Vec<u8> = an
        .regions
        .iter()
        .map(|r| preset_of(&r.biome).id())
        .collect();
    let vbiome: Vec<u8> = draft
        .region
        .iter()
        .map(|r| if *r == NONE { 0 } else { biome_of[*r as usize] })
        .collect();
    {
        let tr = &*t;
        let rows: Vec<Vec<Option<[u8; LAYER_COUNT]>>> = (0..n)
            .into_par_iter()
            .map(|j| {
                (0..n)
                    .map(|i| {
                        let k = j * n + i;
                        if vbiome[k] == 0 {
                            return None;
                        }
                        let p = tr.grid_to_world(i, j);
                        // the biomes a few metres around (soft borders)
                        let mut acc = [0f32; LAYER_COUNT];
                        let mut cnt = 0.0;
                        let reach = 3usize;
                        for (di, dj) in [
                            (0i32, 0i32),
                            (reach as i32, 0),
                            (-(reach as i32), 0),
                            (0, reach as i32),
                            (0, -(reach as i32)),
                        ] {
                            let (ii, jj) = (
                                (i as i32 + di).clamp(0, n as i32 - 1) as usize,
                                (j as i32 + dj).clamp(0, n as i32 - 1) as usize,
                            );
                            let b = vbiome[jj * n + ii];
                            let Some(bp) = BiomePreset::from_id(if b == 0 { vbiome[k] } else { b })
                            else {
                                continue;
                            };
                            let tg = ground_target(tr, &bp.def(), p, seed);
                            for l in 0..LAYER_COUNT {
                                acc[l] += tg[l];
                            }
                            cnt += 1.0;
                        }
                        (cnt > 0.0).then(|| {
                            std::array::from_fn(|l| (acc[l] / cnt).round().clamp(0.0, 255.0) as u8)
                        })
                    })
                    .collect()
            })
            .collect();
        for (j, row) in rows.into_iter().enumerate() {
            for (i, s) in row.into_iter().enumerate() {
                let k = j * n + i;
                t.biomes[k] = vbiome[k];
                t.splat[k] = s.unwrap_or([0; LAYER_COUNT]);
                if settings.zones {
                    t.zones[k] = 0;
                }
            }
        }
        t.mark_splat_dirty(all);
        t.mark_biome_dirty(all);
        t.mark_zone_dirty(all);
    }

    // --- lakes (the Water tab's drawn lakes: basin, shore, shore plants)
    let mut lakes_made = 0;
    for l in &draft.lakes {
        let biome = if l.region == NONE {
            BiomePreset::TemperateForest
        } else {
            preset_of(&an.regions[l.region as usize].biome)
        };
        let shape = crate::water::lakes::shape_by_id(lake_shape(biome));
        let raw: Vec<Vec2> = densify_loop(&l.outline, 6.0);
        if let Ok(id) = crate::water::create_drawn_lake(
            t,
            history,
            store,
            lib.as_deref(),
            water,
            &raw,
            shape,
            1.0,
            1.0,
        ) && let Some(lake) = water.lakes.iter_mut().find(|x| x.id == id)
        {
            lake.name = format!("Lake {}", lakes_made + 1);
            lakes_made += 1;
        }
    }
    // --- rivers (downhill channels through their valleys)
    let mut rivers_made = 0;
    for r in &draft.rivers {
        let path: Vec<Vec3> = r
            .pts
            .iter()
            .filter(|p| t.in_bounds(p[0], p[1]))
            .map(|p| Vec3::new(p[0], t.height_at(p[0], p[1]), p[1]))
            .collect();
        let len: f32 = r
            .pts
            .windows(2)
            .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
            .sum();
        if path.len() < 4 || len < r.width * 3.0 {
            continue;
        }
        water.next_id += 1;
        let id = water.next_id;
        if let Some(mut river) = crate::water::make_river(
            t,
            history,
            &path,
            r.width,
            (r.width * 0.12).clamp(1.0, 4.0),
            id,
        ) {
            rivers_made += 1;
            river.name = format!("River {rivers_made}");
            water.rivers.push(river);
        }
    }
    water.selected = None;
    water.dirty = true;

    // --- vegetation and water styles per biome
    let mut present: Vec<BiomePreset> = an.regions.iter().map(|r| preset_of(&r.biome)).collect();
    present.sort_by_key(|b| b.id());
    present.dedup();
    grow_vegetation(
        t,
        nature,
        history,
        nature_rules.as_deref(),
        density.as_ref(),
        settings.vegetation,
        meta.seed,
    );
    let planted = match (lib.as_deref(), nature_rules.as_deref()) {
        (Some(lib), Some(rules)) if settings.vegetation => {
            let ctx = crate::nature::Ctx {
                terrain: t,
                water,
                zones,
                lib,
                field: nature,
                rules,
                masks: nature_masks,
                amounts: crate::nature::amounts(amounts),
                flora_amounts: amounts.flora_amounts(),
                appearance: **appearance,
                seed: meta.seed,
            };
            crate::nature::place::count_world(&ctx)
                .iter()
                .sum::<usize>()
        }
        _ => 0,
    };
    amounts.applied_plants = amounts.plants;
    for bp in &present {
        restyle_water(t, water, history, &bp.def(), bp.id());
    }

    // --- zones, only when asked (zones are the crafter's own)
    let mut zones_made = 0;
    if settings.zones {
        zones.list.retain(|z| !z.generated);
        for (ri, r) in an.regions.iter().enumerate() {
            let Some(id) = zones.next_id() else { break };
            let mut z = Zone::new(id, r.name.clone());
            crate::biomes::apply_zone_props(&mut z, preset_of(&r.biome));
            let c = super::paint::region_color(ri);
            z.color = [
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
            ];
            z.generated = true;
            for (k, reg) in draft.region.iter().enumerate() {
                if *reg == ri as u32 {
                    t.zones[k] = id;
                }
            }
            zones.list.push(z);
            zones_made += 1;
        }
        t.mark_zone_dirty(all);
    }

    history.commit(store);
    meta.dirty = true;

    // frame the picture's area
    let c = place.center;
    orbit.set_view(
        Vec3::new(c[0], 0.0, c[1] + place.width * 0.12),
        0.0,
        0.95,
        place.width * 0.95,
    );
    orbit.snap();

    let land =
        t.heights.iter().filter(|h| **h > t.sea_level).count() as f32 / t.heights.len() as f32;
    let hi = t.heights.iter().copied().fold(f32::MIN, f32::max);
    let km2 = |cells: usize| {
        (cells as f32 * (place.width / an.w as f32).powi(2) / 1.0e6 * 10.0).round() / 10.0
    };
    let regions: Vec<Value> = an
        .regions
        .iter()
        .map(|r| json!({ "id": r.id, "name": r.name, "biome": super::biome_id(&r.biome), "height_role": r.role.id(), "area_km2": km2(r.cells) }))
        .collect();
    ed.toast(format!(
        "Built the draft: {} regions, {lakes_made} lakes, {rivers_made} rivers{} (undo to go back)",
        an.regions.len(),
        if planted > 0 {
            format!(", {planted} trees, plants and rocks")
        } else {
            String::new()
        }
    ));
    Ok(json!({
        "world_size_m": t.size(),
        "land_pct": (land * 1000.0).round() / 10.0,
        "highest_m": hi.round(),
        "peak_height_m": peak.round(),
        "snow_line_m": t.style.snow_line.round(),
        "regions": regions,
        "lakes": lakes_made,
        "rivers": rivers_made,
        "planted": planted,
        "zones_created": zones_made,
        "undo": "one step: \"Build from picture\"",
    }))
}

/// The lake type that suits a biome.
fn lake_shape(b: BiomePreset) -> &'static str {
    match b {
        BiomePreset::Alpine | BiomePreset::Tundra => "alpine_lake",
        BiomePreset::Taiga => "tarn",
        BiomePreset::Desert => "oasis",
        BiomePreset::Swamp => "swamp_pond",
        BiomePreset::Volcanic => "volcanic_lake",
        BiomePreset::TropicalCoast | BiomePreset::Jungle => "tropical_lagoon",
        BiomePreset::HauntedWoods => "swamp_pond",
        BiomePreset::Meadow => "mirror_lake",
        _ => "forest_lake",
    }
}

fn densify_loop(p: &[[f32; 2]], step: f32) -> Vec<Vec2> {
    let mut out = vec![];
    for i in 0..p.len() {
        let (a, b) = (
            Vec2::from_array(p[i]),
            Vec2::from_array(p[(i + 1) % p.len()]),
        );
        let k = (a.distance(b) / step).ceil().max(1.0) as usize;
        for s in 0..k {
            out.push(a.lerp(b, s as f32 / k as f32));
        }
    }
    out
}

/// The biomes' own nature grows over the land (procedural: rules plus the density map), with
/// the picture's vegetation density (relative to its densest forest) steering how thick: where
/// the picture shows dark, dense forest a biome grows its densest ecosystem (its first
/// vegetation preset) and up to ~1.8× the trees, open light plains its sparsest with a few trees
/// (plants and debris follow more gently). Written into the nature density map (8 m texels: the
/// ecosystem and per-category multipliers; biome borders keep their organic ecotones), part of
/// the build's undo step; the old painted nature goes.
#[allow(clippy::too_many_arguments)]
fn grow_vegetation(
    t: &crate::terrain::Terrain,
    nature: &mut crate::nature::NatureField,
    history: &mut crate::history::History,
    rules: Option<&crate::nature::Rules>,
    density: Option<&super::DensityMap>,
    vegetation: bool,
    seed: u32,
) {
    // the picture's own scale: its densest forest (90th percentile of the land) reads as 1
    let full = density
        .map(|m| {
            let mut v: Vec<f32> = m.values.iter().copied().filter(|v| *v > 0.01).collect();
            v.sort_by(|a, b| a.total_cmp(b));
            v.get(v.len() * 9 / 10).copied().unwrap_or(1.0)
        })
        .unwrap_or(1.0)
        .clamp(0.05, 1.0);
    crate::nature::repaint_world(
        nature,
        history,
        "Build from picture",
        t.half(),
        vegetation,
        |p| {
            let (m, rules) = (density?, rules?);
            let (biome, claim) = crate::nature::rules::ecotone_biome(t, p, seed);
            let def = BiomePreset::from_id(biome)?.def();
            if def.vegetation.is_empty() {
                return None;
            }
            let dv =
                ((m.at(p.x, p.y) + m.at(p.x + 25.0, p.y + 25.0) + m.at(p.x - 25.0, p.y - 25.0))
                    / 3.0
                    / full)
                    .clamp(0.0, 1.0);
            // denser picture → the biome's denser ecosystem
            let patch = (crate::biomes::patch_noise(p, seed) * 0.5 + 0.5).clamp(0.0, 0.999);
            let pick = (patch * 0.4 + (1.0 - dv) * 0.6).clamp(0.0, 0.999);
            let name = def.vegetation[(pick * def.vegetation.len() as f32) as usize];
            let preset = rules.presets.iter().position(|r| r.name == name)?;
            // a painted ecosystem grows at full coverage: the biome's own density and its claim here
            let k = def.density * crate::nature::rules::POPULATE * claim;
            let trees = (0.15 + 1.7 * dv).min(1.8);
            let low = 0.5 + 0.8 * dv;
            let debris = 0.3 + 1.1 * dv;
            Some(([trees * k, k, low * k, debris * k], preset as u8 + 1))
        },
    );
}
