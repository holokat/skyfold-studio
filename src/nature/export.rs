//! Procedural nature for game engines, two ways: baked (every instance, through the exporters'
//! object instances) and as data an engine can scatter itself (Unreal procedural foliage, Unity
//! detail layers, Godot MultiMesh): a density map per category plus the rules.

use bevy::prelude::*;
use rayon::prelude::*;
use serde_json::{Value, json};

use super::Ctx;
use super::field::{TEXEL, texel_of};
use super::rules::{CATS, NONE};

/// Density maps (one byte per 8 m texel, north-west first) and the rules they scale.
pub struct NatureExport {
    /// texels per side; texel (0, 0) is the world's north-west corner
    pub size: usize,
    /// per category: density per texel, 255 = that category's `max_per_m2` in `rules`
    pub maps: [Vec<u8>; 4],
    /// `nature/rules.json`: scales, maps and the presets' layers
    pub rules: Value,
}

/// Rule density (per m², before slope / altitude / water / road checks) per category at a spot.
fn density_at(ctx: &Ctx, p: Vec2) -> [f32; 4] {
    let tex = ctx.field.texel(texel_of(p.x, p.y));
    let (pi, f) = ctx.rules.preset_at(
        ctx.terrain,
        ctx.zones,
        p,
        ctx.seed,
        tex.preset,
        ctx.field.fallback,
    );
    let mut out = [0.0; 4];
    let tx = texel_of(p.x, p.y);
    let mut thins = vec![];
    ctx.field.species_mults(tx, &mut thins);
    let family_amount = |kind| {
        super::rules::flora_group(kind).map_or(1.0, |g| ctx.flora_amounts[g].clamp(0.0, 2.0))
    };
    if pi != NONE {
        for (c, o) in out.iter_mut().enumerate() {
            *o = ctx.rules.presets[pi as usize].cats[c]
                .iter()
                .map(|l| {
                    let thin = thins
                        .iter()
                        .find(|(k, _)| *k == l.kind as u8)
                        .map_or(1.0, |(_, m)| *m);
                    l.density * thin * family_amount(l.kind)
                })
                .sum::<f32>()
                * f
                * tex.mult[c]
                * ctx.amounts[c];
        }
    }
    let mut adds = vec![];
    ctx.field.adds(texel_of(p.x, p.y), &mut adds);
    for (kind, d) in adds {
        if let Some(kind) = super::kind_from_u8(kind)
            && let Some(c) = super::rules::cat_index(crate::objects::kind_category(kind))
        {
            out[c] += d * ctx.amounts[c] * family_amount(kind);
        }
    }
    out
}

pub fn export(ctx: &Ctx) -> NatureExport {
    let half = ctx.terrain.half();
    let size = (ctx.terrain.size() / TEXEL).ceil() as usize;
    let rows: Vec<Vec<[f32; 4]>> = (0..size)
        .into_par_iter()
        .map(|j| {
            (0..size)
                .map(|i| {
                    density_at(
                        ctx,
                        Vec2::new(
                            -half + (i as f32 + 0.5) * TEXEL,
                            -half + (j as f32 + 0.5) * TEXEL,
                        ),
                    )
                })
                .collect()
        })
        .collect();
    let max_per_m2: [f32; 4] =
        std::array::from_fn(|c| rows.iter().flatten().map(|d| d[c]).fold(1e-6, f32::max));
    let maps = std::array::from_fn(|c| {
        rows.iter()
            .flatten()
            .map(|d| (d[c] / max_per_m2[c] * 255.0).round().clamp(0.0, 255.0) as u8)
            .collect()
    });
    let presets: Vec<Value> = ctx
        .rules
        .presets
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "layers": p.cats.iter().flatten().map(|l| json!({
                    "asset": ctx.lib.entries[l.entry].id,
                    "category": crate::objects::kind_category(l.kind).name().to_lowercase(),
                    "per_1000_m2": l.density * 1000.0,
                    "effective_per_1000_m2": l.density * 1000.0 * super::rules::flora_group(l.kind).map_or(ctx.amounts[super::rules::cat_index(crate::objects::kind_category(l.kind)).unwrap()], |g| ctx.flora_amounts[g]),
                    "flora_family": super::rules::flora_group(l.kind).map(|g| ["plants", "flowers", "mushrooms", "berries"][g]),
                    "scale": [l.scale.0, l.scale.1],
                    "max_slope_deg": l.max_slope.min(38.0),
                    "altitude_m": [l.alt.0, l.alt.1],
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let rules = json!({
        "texel_m": TEXEL,
        "size": size,
        "categories": CATS.iter().map(|c| c.name().to_lowercase()).collect::<Vec<_>>(),
        "max_per_m2": max_per_m2,
        "maps": CATS.iter().map(|c| format!("nature/density_{}.png", c.name().to_lowercase())).collect::<Vec<_>>(),
        "appearance": ctx.appearance.sanitized(),
        "forest_diversity": "When enabled, automatic legacy tree layers resolve through the saved forest layout; Natural groves uses broad deterministic patches, Mixed uses tighter variation. Explicit painted species stay exact. Temperate and autumn forests can add maples, ginkgo and sakura; sequoias are rare temperate conifer anchors; bamboo is only resolved in jungle ecology.",
        "placement": "Each map is the category's density (instances per m² = value / 255 × max_per_m2) before the land rules: nothing on slopes steeper than the layer's max_slope (38° at most), outside its altitude range (m above sea), under water or on the beach (except reeds and lily pads), on roads, or inside placed objects' footprints. Species mix: the presets' effective_per_1000_m2 values, which already include the independent family abundance. Do not multiply world_amounts into these effective values or density maps again. The editor's exact instances are in instances.json (baked).",
        "presets": presets,
        "world_amounts": { "trees": ctx.amounts[0], "rocks": ctx.amounts[1], "plants": ctx.flora_amounts[0], "flowers": ctx.flora_amounts[1], "mushrooms": ctx.flora_amounts[2], "berries": ctx.flora_amounts[3], "debris": ctx.amounts[3] },
    });
    NatureExport { size, maps, rules }
}
