//! Markers: where the sheet's object key puts something, at its size — a see-through box (or
//! column) with the key's letter and name over it, standing in until the user (or an agent, or the
//! build's auto-fill) puts a model there. Filling a marker places an ordinary object and keeps the
//! marker (hidden) so it can be swapped again.
//!
//! - `expand`: every copy the key counts, from the sheet's placements (the missing copies spread
//!   round the zone the tagged one stands in, off the boss rings and the paths).
//! - `suggest`: the installed models that fit a marker (name and size), best first.
//! - `draw`: the boxes and labels over the viewport (egui, like the gameplay pins).

use std::collections::HashMap;
use std::sync::OnceLock;

use bevy::math::{Vec2, Vec3};
use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, FontId, Id, LayerId, Order, Pos2, Shape, Stroke, pos2};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use serde::{Deserialize, Serialize};

use super::blueprint::{Blueprint, Group, Role};
use super::land::{Site, w};
use crate::dungeons::geom;
use crate::terrain::Terrain;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: u32,
    pub letter: String,
    pub name: String,
    pub group: Group,
    /// width × depth × height, m
    pub size: [f32; 3],
    /// where it stands (world x, z)
    pub at: [f32; 2],
    /// the way its front faces (degrees, the objects' yaw)
    pub yaw: f32,
    /// the zone it stands in (0: between zones)
    pub zone: u8,
    /// the model put there (None: still a marker)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filled: Option<String>,
}

/// Kinds that the land itself makes (stairs, bridges, decks) or that grow with the biome (trees,
/// plants): no markers for them.
fn made_by_land(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "stair",
        "step",
        "ramp",
        "bridge",
        "deck",
        "causeway",
        "walkway",
        "boardwalk",
        "path",
        "road",
        "tree",
        "bush",
        "shrub",
        "plant",
        "flower",
        "fern",
        "palm",
        "grass",
        "hedge",
        "vine",
    ]
    .iter()
    .any(|k| n.contains(k))
}

fn is_wall(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("wall") || n.contains("palisade") || n.contains("fence") || n.contains("hedge")
}

/// A front facing from `p` toward `q` as an object's yaw (degrees; its +Z toward `q`).
pub fn yaw_toward(p: Vec2, q: Vec2) -> f32 {
    let f = (q - p).normalize_or(Vec2::Y);
    (-f.x).atan2(f.y).to_degrees()
}

/// Every copy of every key object, as markers (at most `cap`).
pub fn expand(bp: &Blueprint, site: &Site, cap: usize) -> Vec<Marker> {
    let mut out: Vec<Marker> = vec![];
    let mut id = 1u32;
    let zone_of = |p: Vec2| {
        site.zones
            .iter()
            .filter(|z| z.inside(p) > -4.0)
            .max_by(|a, b| a.inside(p).total_cmp(&b.inside(p)))
            .map(|z| z.num)
            .unwrap_or(0)
    };
    let blocked = |p: Vec2, r: f32| {
        site.zones
            .iter()
            .any(|z| z.ring.is_some_and(|(c, rr)| p.distance(c) < rr + r + 1.5))
            || site.links.iter().any(|l| l.near(p).0 < l.half_w + r * 0.5)
            || site.edge(p) > -4.0
    };
    for o in &bp.objects {
        if made_by_land(&o.name) || out.len() >= cap {
            continue;
        }
        // (a wall is a run along the zone's edge: at least 12 m of it)
        let size = if is_wall(&o.name) {
            [o.size[0].max(12.0), o.size[1].clamp(1.0, 4.0), o.size[2]]
        } else {
            o.size
        };
        let r = size[0].max(size[1]) * 0.5;
        let listed: Vec<Vec2> = bp
            .placements
            .iter()
            .filter(|p| p.letter.eq_ignore_ascii_case(&o.letter))
            .map(|p| w(p.at))
            .collect();
        let want = (o.count.max(listed.len() as u32).max(1) as usize).min(24);
        let mut spots: Vec<(Vec2, f32)> = vec![];
        for p in &listed {
            let z = site.zone(zone_of(*p));
            let face = z.map_or(*p + Vec2::Y, |z| z.c);
            let yaw = if is_wall(&o.name) {
                wall_yaw(z, *p)
            } else {
                yaw_toward(*p, face)
            };
            spots.push((*p, yaw));
        }
        // the missing copies round the zone the first one stands in (else the biggest zone that
        // isn't a boss arena), on a ring a little in from its edge
        if spots.len() < want {
            let home = listed
                .first()
                .map(|p| zone_of(*p))
                .filter(|n| *n > 0)
                .and_then(|n| site.zone(n))
                .or_else(|| {
                    site.zones
                        .iter()
                        .filter(|z| z.role != Role::Final)
                        .max_by(|a, b| (a.h.x * a.h.y).total_cmp(&(b.h.x * b.h.y)))
                });
            if let Some(z) = home {
                let need = want - spots.len();
                let tries = need * 6 + 12;
                let start = (o.letter.bytes().next().unwrap_or(b'A') as f32) * 0.37;
                for i in 0..tries {
                    if spots.len() >= want {
                        break;
                    }
                    let a = start + std::f32::consts::TAU * i as f32 / tries as f32;
                    // (a point on the zone's edge, `inset` m in)
                    let dir = Vec2::from_angle(a);
                    let mut edge = z.c;
                    let mut t = 0.0;
                    while t < z.reach() + 4.0 {
                        let q = z.c + dir * t;
                        if z.inside(q) < r + 2.5 {
                            break;
                        }
                        edge = q;
                        t += 1.0;
                    }
                    if edge.distance(z.c) < 2.0
                        || blocked(edge, r)
                        || spots.iter().any(|(s, _)| s.distance(edge) < r * 2.0 + 2.0)
                    {
                        continue;
                    }
                    let yaw = if is_wall(&o.name) {
                        wall_yaw(Some(z), edge)
                    } else {
                        yaw_toward(edge, z.c)
                    };
                    spots.push((edge, yaw));
                }
            }
        }
        for (p, yaw) in spots.into_iter().take(want) {
            if out.len() >= cap {
                break;
            }
            out.push(Marker {
                id,
                letter: o.letter.clone(),
                name: o.name.clone(),
                group: o.group,
                size,
                at: [p.x, p.y],
                yaw,
                zone: zone_of(p),
                filled: None,
            });
            id += 1;
        }
    }
    out
}

/// A wall runs along the zone's edge (its length across its front).
fn wall_yaw(z: Option<&super::land::ZoneW>, p: Vec2) -> f32 {
    match z {
        Some(z) => yaw_toward(p, z.c),
        None => 0.0,
    }
}

// ------------------------------------------------------------------ the installed models that fit

#[derive(Clone, Debug)]
struct Model {
    id: String,
    name: String,
    group: String,
    /// x, height, z
    size: [f32; 3],
}

fn catalog() -> &'static Vec<Model> {
    static C: OnceLock<Vec<Model>> = OnceLock::new();
    C.get_or_init(|| {
        let path = std::path::Path::new("assets/models/stylepacks/handpainted/models.json");
        let path = if path.exists() {
            path.to_path_buf()
        } else {
            crate::asset_file("models/stylepacks/handpainted/models.json")
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return vec![];
        };
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
            return vec![];
        };
        doc["models"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        let s = m["size_m"].as_array()?;
                        Some(Model {
                            id: m["id"].as_str()?.to_string(),
                            name: m["name"].as_str().unwrap_or_default().to_string(),
                            group: m["group"].as_str().unwrap_or_default().to_string(),
                            size: [
                                s.first()?.as_f64()? as f32,
                                s.get(1)?.as_f64()? as f32,
                                s.get(2)?.as_f64()? as f32,
                            ],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// What a sheet's object name asks for → words that find it among the models' ids and names.
const WORDS: &[(&[&str], &[&str])] = &[
    (
        &["brazier", "fire bowl", "firebowl", "fire pit"],
        &["brazier", "fire_bowl", "firebowl"],
    ),
    (&["lamp", "lantern", "torch"], &["lantern", "lamp", "torch"]),
    (&["pillar", "column"], &["pillar", "column"]),
    (
        &[
            "obelisk",
            "monolith",
            "standing stone",
            "rune stone",
            "runestone",
            "menhir",
            "monument",
        ],
        &[
            "obelisk",
            "monolith",
            "standing_stone",
            "rune_stone",
            "menhir",
        ],
    ),
    (
        &["statue", "colossus", "idol", "effigy"],
        &["statue", "idol"],
    ),
    (&["arch", "gateway", "gate"], &["arch", "gate", "trilithon"]),
    (&["watchtower", "tower", "turret"], &["tower"]),
    (&["tent", "pavilion"], &["tent", "pavilion"]),
    (
        &["crate", "barrel", "supply", "supplies", "sack", "stack"],
        &["crate", "barrel", "sacks", "supply"],
    ),
    (&["cart", "wagon"], &["cart", "wagon"]),
    (&["banner", "flag", "standard"], &["banner", "flag"]),
    (&["cage"], &["cage"]),
    (&["rubble", "debris"], &["rubble", "debris"]),
    (&["throne"], &["throne"]),
    (&["well"], &["well"]),
    (&["fountain"], &["fountain"]),
    (&["shrine"], &["shrine"]),
    (&["anvil"], &["anvil"]),
    (
        &["forge", "furnace", "smelter"],
        &["forge", "furnace", "smelter"],
    ),
    (
        &["boat", "ship", "longship", "galleon", "barge"],
        &["boat", "ship"],
    ),
    (
        &["skull", "ribcage", "bones", "skeleton"],
        &["skull", "ribcage", "bone"],
    ),
    (&["weapon rack", "rack"], &["rack"]),
    (
        &["grave", "tomb", "sarcophag", "crypt", "mausoleum"],
        &["grave", "tomb", "sarcophag", "crypt", "mausoleum"],
    ),
    (&["bench"], &["bench"]),
    (&["stone circle", "dolmen"], &["stone_circle", "dolmen"]),
    (
        &["hut", "house", "cottage", "cabin"],
        &["hut", "house", "cottage", "cabin"],
    ),
    (&["temple"], &["temple"]),
];

/// The installed models that fit a marker, best first: (asset id, uniform scale).
pub fn suggest(m: &Marker, n: usize) -> Vec<(String, f32)> {
    let name = m.name.to_ascii_lowercase();
    let wants: Vec<&[&str]> = WORDS
        .iter()
        .filter(|(asks, _)| asks.iter().any(|a| name.contains(a)))
        .map(|(_, finds)| *finds)
        .collect();
    if wants.is_empty() {
        return vec![];
    }
    let mut scored: Vec<(f32, String, f32)> = vec![];
    for model in catalog() {
        let id = model.id.to_ascii_lowercase();
        let mname = model.name.to_ascii_lowercase();
        let hit = wants
            .iter()
            .flat_map(|f| f.iter())
            .map(|f| {
                if id.contains(f) {
                    2.0
                } else if mname.contains(&f.replace('_', " ")) {
                    1.0
                } else {
                    0.0
                }
            })
            .fold(0.0, f32::max);
        if hit == 0.0 || model.size[1] < 0.05 {
            continue;
        }
        let scale = (m.size[2] / model.size[1]).max(0.01);
        // (structures and statues may be scaled up further than props)
        let big =
            m.group == Group::Structure || name.contains("statue") || name.contains("colossus");
        if !(0.35..=if big { 3.6 } else { 2.6 }).contains(&scale) {
            continue;
        }
        // (its footprint at that scale against the marker's)
        let foot = (model.size[0].max(model.size[2]) * scale) / m.size[0].max(m.size[1]).max(0.5);
        let mut score = hit - scale.ln().abs() - 0.6 * foot.ln().abs();
        if model.group == "encounters" {
            score -= 0.6;
        }
        scored.push((score, model.id.clone(), scale));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(n)
        .map(|(_, id, s)| (format!("hp:{id}"), (s * 100.0).round() / 100.0))
        .collect()
}

/// The `objects_place` item that fills a marker with `asset` at `scale`.
pub fn place_item(m: &Marker, asset: &str, scale: f32) -> serde_json::Value {
    serde_json::json!({ "asset": asset, "position": [m.at[0], m.at[1]], "yaw_deg": (m.yaw * 100.0).round() / 100.0, "scale": scale })
}

// ------------------------------------------------------------------ drawing

pub(super) fn register(app: &mut App) {
    app.add_systems(EguiPrimaryContextPass, draw);
}

fn color(g: Group, filled: bool, sel: bool) -> Color32 {
    let base = match g {
        Group::Structure => Color32::from_rgb(92, 170, 255),
        Group::Prop => Color32::from_rgb(255, 186, 74),
    };
    if sel {
        Color32::WHITE
    } else if filled {
        base.gamma_multiply(0.35)
    } else {
        base
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    mut contexts: EguiContexts,
    map: Res<crate::dungeons::DungeonMap>,
    mut show: ResMut<super::ui::SheetUi>,
    walk: Res<crate::walk::WalkState>,
    terrain: Res<Terrain>,
    camera: Query<(&Camera, &GlobalTransform), With<crate::camera::MainCamera>>,
) -> Result {
    let Some(doc) = map.0.as_ref() else {
        return Ok(());
    };
    if doc.markers.is_empty() || !show.on || walk.cinematic || (walk.active && !show.walk) {
        return Ok(());
    }
    let Ok((cam, cam_t)) = camera.single() else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::new("sheet_markers")));
    let eye = cam_t.translation();
    let proj = |p: Vec3| cam.world_to_viewport(cam_t, p).ok().map(|v| pos2(v.x, v.y));
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let mut hovered: Option<(u32, f32)> = None;
    // (far ones first, so near labels draw on top)
    let mut order: Vec<&Marker> = doc
        .markers
        .iter()
        .filter(|m| m.filled.is_none() || show.selected == Some(m.id))
        .collect();
    order.sort_by(|a, b| {
        let da = eye.distance(Vec3::new(a.at[0], eye.y, a.at[1]));
        let db = eye.distance(Vec3::new(b.at[0], eye.y, b.at[1]));
        db.total_cmp(&da)
    });
    for m in order {
        let c = Vec2::new(m.at[0], m.at[1]);
        let y = terrain.height_at(c.x, c.y);
        let dist = eye.distance(Vec3::new(c.x, y, c.y));
        if dist > 900.0 {
            continue;
        }
        let sel = show.selected == Some(m.id) || show.hovered == Some(m.id);
        let col = color(m.group, m.filled.is_some(), sel);
        let [sw, sd, sh] = m.size;
        let a = m.yaw.to_radians();
        // (the box's corners: its width across its front, its depth along it)
        let ax = geom::rot(Vec2::new(sw * 0.5, 0.0), -a);
        let az = geom::rot(Vec2::new(0.0, sd * 0.5), -a);
        let foot = [c - ax - az, c + ax - az, c + ax + az, c - ax + az];
        let lo: Vec<Option<Pos2>> = foot
            .iter()
            .map(|q| {
                proj(Vec3::new(
                    q.x,
                    terrain.height_at(q.x, q.y).max(y - 1.0),
                    q.y,
                ))
            })
            .collect();
        let hi: Vec<Option<Pos2>> = foot
            .iter()
            .map(|q| proj(Vec3::new(q.x, y + sh, q.y)))
            .collect();
        let fade = 1.0 - ((dist - 300.0) / 600.0).clamp(0.0, 0.8);
        let stroke = Stroke::new(if sel { 2.2 } else { 1.4 }, col.gamma_multiply(fade));
        if let (Some(l), Some(h)) = (
            lo.iter().copied().collect::<Option<Vec<_>>>(),
            hi.iter().copied().collect::<Option<Vec<_>>>(),
        ) {
            painter.add(Shape::convex_polygon(
                h.clone(),
                col.gamma_multiply(0.16 * fade),
                Stroke::NONE,
            ));
            for i in 0..4 {
                painter.line_segment([l[i], l[(i + 1) % 4]], stroke);
                painter.line_segment([h[i], h[(i + 1) % 4]], stroke);
                painter.line_segment([l[i], h[i]], stroke);
            }
            // its front edge marked
            painter.line_segment(
                [h[2], h[3]],
                Stroke::new(stroke.width + 1.2, col.gamma_multiply(fade)),
            );
        }
        if let Some(top) = proj(Vec3::new(c.x, y + sh + 1.0, c.y)) {
            let text = if dist < 220.0 {
                format!("{} · {}", m.letter, m.name)
            } else {
                m.letter.clone()
            };
            let font = FontId::proportional(if dist < 220.0 { 12.0 } else { 11.0 });
            let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
            let r = egui::Rect::from_center_size(top, galley.size() + egui::vec2(10.0, 4.0));
            painter.rect_filled(r, 4.0, Color32::from_black_alpha((150.0 * fade) as u8));
            painter.rect_stroke(
                r,
                4.0,
                Stroke::new(1.0, col.gamma_multiply(fade)),
                egui::StrokeKind::Inside,
            );
            painter.galley(r.min + egui::vec2(5.0, 2.0), galley, Color32::WHITE);
            if pointer.is_some_and(|p| r.expand(2.0).contains(p))
                && hovered.is_none_or(|(_, d)| dist < d)
            {
                hovered = Some((m.id, dist));
            }
        }
    }
    show.hovered = hovered.map(|h| h.0);
    if let Some((id, _)) = hovered {
        if ctx.input(|i| i.pointer.primary_clicked()) {
            show.selected = Some(id);
        }
    }
    Ok(())
}

/// Markers by zone, for the panel and MCP.
pub fn by_zone(markers: &[Marker]) -> HashMap<u8, Vec<&Marker>> {
    let mut m: HashMap<u8, Vec<&Marker>> = HashMap::new();
    for k in markers {
        m.entry(k.zone).or_default().push(k);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_braziers_and_pillars_from_the_pack() {
        if !crate::asset_file("models/stylepacks/handpainted/models.json").is_file() {
            return; // public source deliberately excludes this optional paid pack
        }
        let m = Marker {
            id: 1,
            letter: "G".into(),
            name: "Brazier".into(),
            group: Group::Prop,
            size: [1.5, 1.5, 2.0],
            at: [0.0, 0.0],
            yaw: 0.0,
            zone: 1,
            filled: None,
        };
        let s = suggest(&m, 3);
        assert!(!s.is_empty(), "a brazier in the hand-painted pack");
        assert!(s[0].0.starts_with("hp:"));
        let m = Marker {
            name: "Astrolabe dome".into(),
            size: [22.0, 14.0, 12.0],
            ..m
        };
        assert!(suggest(&m, 3).is_empty(), "no keyword: stays a marker");
    }

    #[test]
    fn every_copy_gets_a_marker_off_the_rings_and_paths() {
        let mut bp = crate::dungeons::sheets::land::tests_sample();
        bp.objects = vec![
            super::super::blueprint::ObjectKind {
                letter: "C".into(),
                name: "Pillar".into(),
                group: Group::Structure,
                size: [2.0, 2.0, 8.0],
                count: 8,
            },
            super::super::blueprint::ObjectKind {
                letter: "D".into(),
                name: "Stair".into(),
                group: Group::Structure,
                size: [9.0, 12.0, 4.0],
                count: 3,
            },
        ];
        bp.placements = vec![super::super::blueprint::Placement {
            letter: "C".into(),
            at: [-110.0, 65.0],
            facing: None,
        }];
        let site = Site::new(&bp, 10.0, 1);
        let ms = expand(&bp, &site, 200);
        assert_eq!(ms.iter().filter(|m| m.letter == "C").count(), 8, "{ms:?}");
        assert!(
            ms.iter().all(|m| m.letter != "D"),
            "stairs are made by the land"
        );
        let z2 = site.zone(2).unwrap();
        let (rc, rr) = z2.ring.unwrap();
        for m in ms.iter().skip(1) {
            assert!(
                Vec2::new(m.at[0], m.at[1]).distance(rc) > rr,
                "off the ring: {m:?}"
            );
        }
    }
}
