//! Raised, sheet-authored bridge decks.  They are persistent authored roads rather than placed
//! props, so the road renderer, collision and exports all see the same walkable structure.
//!
//! Existing sheet maps get one conservative additive pass.  The only state persisted in the
//! map document is the road ids made by that pass; a later deletion or edit is therefore always
//! the player's choice and is never "repaired" again.

use std::path::Path;

use bevy::math::Vec2;
use serde::{Deserialize, Serialize};

use super::Blueprint;
use super::blueprint::LinkKind;
use super::land::{self, LinkW, Site};
use crate::dungeons::geom;
use crate::dungeons::{MapDoc, MapKind};
use crate::roads::{
    AuthoredRoute, AuthoredSpan, AuthoredSpanKind, Road, RoadKind, RoadOptions, Roads,
};

/// Bump only for an intentionally compatible, one-shot migration.  Once set, missing source
/// roads are user changes, not a reason to synthesize replacements.
pub const REVISION: u32 = 1;
const WATER_CLEARANCE: f32 = 0.7;
const DRY_CHASM_DEPTH: f32 = 8.0;
const APPROACH: f32 = 7.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub revision: u32,
    /// Authored road ids from this sheet.  This list is deliberately not reconciled against the
    /// road list on later opens: deleting one is a valid manual edit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_roads: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
struct Hazard {
    start: f32,
    end: f32,
    support_y: f32,
    kind: AuthoredSpanKind,
}

#[derive(Clone, Debug)]
struct BridgeDef {
    from: u8,
    to: u8,
    width: f32,
    kind: RoadKind,
    points: Vec<[f32; 3]>,
    spans: Vec<Hazard>,
}

/// Install a new sheet map's bridge decks.  The caller invokes this after all sheet water is
/// final, and before its newly-created `MapDoc` is saved.
pub fn install_new(roads: &mut Roads, bp: &Blueprint, site: &Site) -> State {
    install(roads, compile(bp, site), false)
}

/// One safe, additive reconciliation for an already-saved map.  If the sheet reference is
/// absent or corrupt, do nothing at all.  Existing roads are never rebuilt or modified.
pub fn reconcile_loaded(dir: &Path, roads: &mut Roads, map: &mut Option<MapDoc>) -> bool {
    let Some(doc) = map.as_ref() else {
        return false;
    };
    if doc.kind != MapKind::Realm || doc.sheet_bridges.revision >= REVISION {
        return false;
    }
    let sheet = dir.join("sheet");
    let Ok(text) = std::fs::read_to_string(sheet.join("blueprint.json")) else {
        return false;
    };
    let Ok(bp) = serde_json::from_str::<Blueprint>(&text) else {
        return false;
    };
    let Ok(build) = std::fs::read_to_string(sheet.join("build.json")) else {
        return false;
    };
    let Ok(build) = serde_json::from_str::<serde_json::Value>(&build) else {
        return false;
    };
    let Some(y0) = build.get("floor").and_then(serde_json::Value::as_f64) else {
        return false;
    };
    let y0 = y0 as f32;
    if !y0.is_finite() || !valid_blueprint(&bp) {
        return false;
    }
    let mut site = Site::new(&bp, y0, 0);
    // Island sheets keep the original picture beside the blueprint.  Its water mask is the
    // authoritative sea shape, so reuse it for legacy crossing detection when available.
    if site.sea.is_some() {
        if let Some(frame) = bp.picture.as_ref() {
            let picture = std::fs::read_dir(&sheet).ok().and_then(|entries| {
                entries.flatten().map(|entry| entry.path()).find(|path| {
                    path.extension().is_some_and(|ext| {
                        matches!(
                            ext.to_string_lossy().to_ascii_lowercase().as_str(),
                            "png" | "jpg" | "jpeg"
                        )
                    })
                })
            });
            if let Some(mask) = picture
                .and_then(|path| image::open(path).ok())
                .and_then(|image| land::WaterMask::from_picture(&image.to_rgba8(), frame, bp.size))
            {
                site.water_mask = Some(mask);
            }
        }
    }
    reconcile(roads, map.as_mut().expect("checked above"), &bp, &site)
}

fn reconcile(roads: &mut Roads, doc: &mut MapDoc, bp: &Blueprint, site: &Site) -> bool {
    if doc.kind != MapKind::Realm || doc.sheet_bridges.revision >= REVISION {
        return false;
    }
    doc.sheet_bridges = install(roads, compile(bp, site), true);
    true
}

fn valid_blueprint(bp: &Blueprint) -> bool {
    bp.size.iter().copied().all(f32::is_finite)
        && bp.links.iter().all(|link| {
            link.pts.len() >= 2
                && link.width.is_finite()
                && link.pts.iter().flatten().copied().all(f32::is_finite)
        })
}

fn install(roads: &mut Roads, defs: Vec<BridgeDef>, avoid_manual: bool) -> State {
    let mut state = State {
        revision: REVISION,
        source_roads: vec![],
    };
    for def in defs {
        if avoid_manual && roads.roads.iter().any(|road| overlaps(road, &def)) {
            bevy::log::warn!(
                "sheet bridge {}→{} skipped: it overlaps an existing road",
                def.from,
                def.to
            );
            continue;
        }
        let id = roads.alloc_id();
        let mut road = Road::new(
            id,
            def.kind,
            def.points,
            def.width,
            RoadOptions {
                bed: false,
                clear: false,
                bridges: true,
                props: false,
            },
        );
        road.name = format!("Sheet bridge {} to {}", def.from, def.to);
        road.authored = Some(AuthoredRoute {
            spans: def
                .spans
                .into_iter()
                .map(|s| AuthoredSpan {
                    start: s.start,
                    end: s.end,
                    support_y: s.support_y,
                    kind: s.kind,
                })
                .collect(),
        });
        roads.roads.push(road);
        state.source_roads.push(id);
    }
    if !state.source_roads.is_empty() {
        roads.dirty = true;
    }
    state
}

fn overlaps(road: &Road, candidate: &BridgeDef) -> bool {
    let line = road.centreline(3.0);
    let cand: Vec<Vec2> = candidate
        .points
        .iter()
        .map(|p| Vec2::new(p[0], p[2]))
        .collect();
    if line.len() < 2 || cand.len() < 2 {
        return false;
    }
    let reach = (road.width.max(0.5) + candidate.width.max(0.5)) * 0.5;
    let len = geom::polyline_len(&cand).max(0.01);
    candidate.spans.iter().any(|span| {
        let a = geom::polyline_at(&cand, span.start * len).0;
        let b = geom::polyline_at(&cand, span.end * len).0;
        line.iter()
            .any(|p| geom::polyline_dist(&[a, b], Vec2::new(p.x, p.z)).0 < reach)
    })
}

fn compile(bp: &Blueprint, site: &Site) -> Vec<BridgeDef> {
    bp.links
        .iter()
        .zip(&site.links)
        .filter_map(|(source, link)| {
            if source.kind == LinkKind::Causeway || link.len <= 0.1 {
                return None;
            }
            let mut spans = hazards(site, link);
            if source.kind != LinkKind::Bridge && spans.is_empty() {
                return None;
            }
            // A sheet calling out a bridge is intentional even when the hazard reader could not
            // retain a thin, dry chasm.  Keep that deck suspended over its middle rather than
            // turning it into a terrain-filled causeway.
            if source.kind == LinkKind::Bridge && spans.is_empty() {
                let low = link
                    .profile
                    .iter()
                    .flat_map(|p| [p.2, p.3])
                    .fold(site.y0, f32::min);
                spans.push(Hazard {
                    start: 0.15,
                    end: 0.85,
                    support_y: low - DRY_CHASM_DEPTH,
                    kind: AuthoredSpanKind::Chasm,
                });
            }
            let points = deck_points(link, &spans);
            (points.len() >= 2 && points.iter().flatten().copied().all(f32::is_finite)).then_some(
                BridgeDef {
                    from: source.from,
                    to: source.to,
                    width: (link.half_w * 2.0).clamp(1.0, 20.0),
                    kind: bridge_kind(&bp.family),
                    points,
                    spans,
                },
            )
        })
        .collect()
}

/// Sheet families with masonry landmarks use the stone deck generated by the road bridge
/// renderer.  Tropical sheets keep their lighter timber footbridge character.
fn bridge_kind(family: &str) -> RoadKind {
    match family {
        "tropical" => RoadKind::Footpath,
        "arid" | "cold" | "temperate" | "twilight" | "volcanic" => RoadKind::Cobblestone,
        _ => RoadKind::Cobblestone,
    }
}

fn hazards(site: &Site, link: &LinkW) -> Vec<Hazard> {
    let steps = (link.len.ceil() as usize).clamp(8, 4096);
    let mut out = vec![];
    let mut run: Option<(usize, AuthoredSpanKind, f32)> = None;
    for i in 0..=steps {
        let s = link.len * i as f32 / steps as f32;
        let hazard = hazard_at(site, link, s);
        match (run, hazard) {
            (None, Some((kind, support_y))) => run = Some((i, kind, support_y)),
            (Some((start, kind, support_y)), Some((next, next_support))) if kind == next => {
                run = Some((start, kind, support_y.min(next_support)));
            }
            (Some((start, kind, support_y)), next) => {
                push_hazard(&mut out, start, i.saturating_sub(1), steps, kind, support_y);
                run = next.map(|(kind, support_y)| (i, kind, support_y));
            }
            (None, None) => {}
        }
    }
    if let Some((start, kind, support_y)) = run {
        push_hazard(&mut out, start, steps, steps, kind, support_y);
    }
    out
}

fn push_hazard(
    out: &mut Vec<Hazard>,
    start: usize,
    end: usize,
    steps: usize,
    kind: AuthoredSpanKind,
    support_y: f32,
) {
    if end <= start || !support_y.is_finite() {
        return;
    }
    let start = start as f32 / steps as f32;
    let end = end as f32 / steps as f32;
    if end - start < 1.0 / steps as f32 {
        return;
    }
    out.push(Hazard {
        start,
        end,
        support_y,
        kind,
    });
}

fn hazard_at(site: &Site, link: &LinkW, s: f32) -> Option<(AuthoredSpanKind, f32)> {
    if !land::over_hazard(site, link, s) {
        return None;
    }
    let p = geom::polyline_at(&link.pts, s).0;
    if let Some((_, floor)) = site
        .chasms
        .iter()
        .find(|(outline, _)| geom::poly_sdf(outline, p) >= 0.0)
    {
        return Some((AuthoredSpanKind::Chasm, *floor));
    }
    if let Some((_, level)) = site
        .water
        .iter()
        .find(|(outline, _)| geom::poly_sdf(outline, p) >= 0.0)
    {
        return Some((AuthoredSpanKind::Water, *level));
    }
    site.sea.map(|level| (AuthoredSpanKind::Water, level))
}

fn deck_points(link: &LinkW, spans: &[Hazard]) -> Vec<[f32; 3]> {
    let mut marks = vec![0.0, link.len];
    let mut at = 0.0;
    for pair in link.pts.windows(2) {
        at += pair[0].distance(pair[1]);
        marks.push(at);
    }
    // The terrain compiler puts stair/terrace transitions at these boundaries.  Keep each one
    // as an authored control point so an otherwise long straight segment never interpolates
    // through a landing or up a stair that the sheet deliberately placed.
    for &(a, b, _, _) in &link.profile {
        marks.extend([a, b]);
    }
    for span in spans {
        let (a, b) = (span.start * link.len, span.end * link.len);
        marks.extend([(a - APPROACH).max(0.0), a, b, (b + APPROACH).min(link.len)]);
    }
    marks.sort_by(f32::total_cmp);
    marks.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    marks
        .into_iter()
        .map(|s| {
            let base = link.height(s);
            let mut y = base;
            for span in spans {
                let (a, b) = (span.start * link.len, span.end * link.len);
                let deck = base.max(span.support_y + WATER_CLEARANCE);
                if s >= a && s <= b {
                    y = y.max(deck);
                } else if s >= (a - APPROACH).max(0.0) && s < a {
                    let t = (s - (a - APPROACH).max(0.0)) / (a - (a - APPROACH).max(0.0)).max(0.01);
                    y = y.max(base + (deck - base) * t);
                } else if s > b && s <= (b + APPROACH).min(link.len) {
                    let t = 1.0 - (s - b) / ((b + APPROACH).min(link.len) - b).max(0.01);
                    y = y.max(base + (deck - base) * t);
                }
            }
            let p = geom::polyline_at(&link.pts, s).0;
            [p.x, y, p.y]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn dry_chasm() -> Blueprint {
        let mut bp = super::super::land::tests_sample();
        bp.links[0].kind = LinkKind::Bridge;
        bp.chasms[0] = super::super::blueprint::Chasm {
            name: "Rift".into(),
            outline: vec![
                [-122.0, -12.0],
                [-96.0, -12.0],
                [-96.0, 12.0],
                [-122.0, 12.0],
            ],
            floor: -14.0,
        };
        bp
    }

    #[test]
    fn dry_chasm_has_finite_authored_span() {
        let bp = dry_chasm();
        let site = Site::new(&bp, 12.0, 1);
        let defs = compile(&bp, &site);
        assert!(!defs.is_empty());
        assert!(
            defs.iter()
                .flat_map(|d| &d.points)
                .flatten()
                .all(|v| v.is_finite())
        );
        let link = &site.links[0];
        for &(a, b, _, _) in &link.profile {
            for s in [a, b] {
                let p = geom::polyline_at(&link.pts, s).0;
                assert!(
                    defs[0]
                        .points
                        .iter()
                        .any(|q| Vec2::new(q[0], q[2]).distance(p) < 0.02)
                );
            }
        }
        assert!(
            defs.iter()
                .flat_map(|d| &d.spans)
                .any(|s| s.kind == AuthoredSpanKind::Chasm)
        );
    }

    #[test]
    fn explicit_bridge_without_read_hazard_falls_back() {
        let mut bp = super::super::land::tests_sample();
        bp.links[0].kind = LinkKind::Bridge;
        let defs = compile(&bp, &Site::new(&bp, 0.0, 1));
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].spans[0].kind, AuthoredSpanKind::Chasm);
    }

    #[test]
    fn library_bridges_compile_to_finite_profiles() {
        let root = crate::asset_file("dungeon_sheets");
        let mut sheets = 0usize;
        let mut explicit = 0usize;
        let mut compiled = 0usize;
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let path = entry.path().join("blueprint.json");
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let bp: Blueprint = serde_json::from_str(&text).unwrap();
            sheets += 1;
            explicit += bp
                .links
                .iter()
                .filter(|l| l.kind == LinkKind::Bridge)
                .count();
            let defs = compile(&bp, &Site::new(&bp, 9.0, 1));
            compiled += defs.len();
            for def in defs {
                assert!(def.width.is_finite() && def.width > 0.0);
                assert!(def.points.iter().flatten().all(|v| v.is_finite()));
                assert!(def.spans.iter().all(|s| {
                    s.start.is_finite()
                        && s.end.is_finite()
                        && s.start >= 0.0
                        && s.end <= 1.0
                        && s.end > s.start
                        && s.support_y.is_finite()
                }));
            }
        }
        assert!(sheets >= 61, "expected the bundled sheet library");
        assert!(explicit >= 84, "expected the bundled bridge corpus");
        assert!(compiled >= explicit, "every explicit bridge gets a deck");
    }

    #[test]
    fn migration_is_one_shot_even_after_a_source_road_is_deleted() {
        let bp = dry_chasm();
        let site = Site::new(&bp, 12.0, 1);
        let mut map = MapDoc::new(1, "Sheet", MapKind::Realm);
        let mut roads = Roads::default();
        assert!(reconcile(&mut roads, &mut map, &bp, &site));
        assert!(!map.sheet_bridges.source_roads.is_empty());
        let made = roads.roads.len();
        assert!(!reconcile(&mut roads, &mut map, &bp, &site));
        assert_eq!(roads.roads.len(), made, "reload does not duplicate bridges");
        roads.roads.clear();
        assert!(!reconcile(&mut roads, &mut map, &bp, &site));
        assert!(
            roads.roads.is_empty(),
            "a deleted sheet bridge stays deleted"
        );
    }

    #[test]
    fn manual_overlap_is_skipped_without_touching_the_road() {
        let bp = dry_chasm();
        let site = Site::new(&bp, 12.0, 1);
        let existing = compile(&bp, &site).pop().unwrap();
        let mut roads = Roads::default();
        roads.roads.push(Road::new(
            77,
            RoadKind::Footpath,
            existing.points,
            existing.width,
            RoadOptions::default(),
        ));
        let mut map = MapDoc::new(1, "Sheet", MapKind::Realm);
        assert!(reconcile(&mut roads, &mut map, &bp, &site));
        assert_eq!(roads.roads.len(), 1);
        assert!(map.sheet_bridges.source_roads.is_empty());
    }

    #[test]
    fn missing_or_corrupt_sheet_never_mutates_a_legacy_map() {
        let mut map = Some(MapDoc::new(1, "Legacy", MapKind::Realm));
        let before = map.clone();
        let mut roads = Roads::default();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let nowhere = std::env::temp_dir().join(format!("sheet-bridges-no-source-{stamp}"));
        assert!(!reconcile_loaded(&nowhere, &mut roads, &mut map));
        assert_eq!(map, before);
        assert!(roads.roads.is_empty());

        let dir = std::env::temp_dir().join(format!("sheet-bridges-corrupt-{stamp}"));
        std::fs::create_dir_all(dir.join("sheet")).unwrap();
        std::fs::write(dir.join("sheet/blueprint.json"), "{not json").unwrap();
        std::fs::write(dir.join("sheet/build.json"), r#"{"floor": 9.0}"#).unwrap();
        assert!(!reconcile_loaded(&dir, &mut roads, &mut map));
        assert_eq!(map, before);
        assert!(roads.roads.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
