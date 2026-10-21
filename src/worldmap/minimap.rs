//! The minimap widget for walk / playtest HUDs: a circular, optionally rotating window onto the
//! generated map with the player arrow, a north tick and nearby places.
//!
//! Quick use (what walk mode does), from any egui pass with the `WorldMap` resource:
//! ```ignore
//! crate::worldmap::minimap::hud(ctx, &mut map, &gameplay, walk.position, walk.yaw, screen);
//! ```
//! It draws at the top right, below the FPS readout, from the generated map (or a quick minimal
//! map made automatically the first time walk mode starts without one). For a custom layout call
//! [`draw`] with a [`Source`] (`WorldMap::minimap_source`), a [`View`] and the markers to show.

use bevy::math::{Vec2, Vec3};
use bevy_egui::egui::{self, Color32, FontId, Pos2, Stroke, pos2, vec2};

use super::WorldMap;
use super::labels::kind_color;
use crate::gameplay::{Gameplay, PoiKind};

/// A map texture covering the world square from `world_min` (world xz, north-west corner) with
/// side `world_size` metres.
#[derive(Clone, Copy, Debug)]
pub struct Source {
    pub texture: egui::TextureId,
    pub world_min: Vec2,
    pub world_size: f32,
}

/// Where the player is and how the minimap shows it.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Player position (world xz).
    pub player: Vec2,
    /// Facing as walk mode stores it: 0 looks north (−Z), increasing counter-clockwise.
    pub yaw: f32,
    /// World radius shown (m).
    pub radius_m: f32,
    /// Turn the map so the player's facing is up (else north is up).
    pub rotate: bool,
}

/// A place to mark.
#[derive(Clone, Debug)]
pub struct Marker {
    pub world: Vec2,
    /// `None`: the player start.
    pub kind: Option<PoiKind>,
}

impl View {
    /// Screen offset (px, y down) → world offset (m).
    fn to_world(self, s: Vec2, k: f32) -> Vec2 {
        if self.rotate {
            let (sn, cs) = self.yaw.sin_cos();
            let fwd = Vec2::new(-sn, -cs);
            let right = Vec2::new(cs, -sn);
            (right * s.x - fwd * s.y) * k
        } else {
            s * k
        }
    }
    /// World offset (m) → screen offset (px).
    fn to_screen(self, w: Vec2, k: f32) -> Vec2 {
        if self.rotate {
            let (sn, cs) = self.yaw.sin_cos();
            let fwd = Vec2::new(-sn, -cs);
            let right = Vec2::new(cs, -sn);
            Vec2::new(w.dot(right), -w.dot(fwd)) / k
        } else {
            w / k
        }
    }
}

/// Draws the minimap: a disc of `radius` px centred on `centre`.
pub fn draw(
    painter: &egui::Painter,
    centre: Pos2,
    radius: f32,
    src: &Source,
    view: &View,
    markers: &[Marker],
) {
    let k = view.radius_m / radius;
    let c = Vec2::new(centre.x, centre.y);
    // frame shadow and backing
    painter.circle_filled(
        centre + vec2(0.0, 2.0),
        radius + 5.0,
        Color32::from_black_alpha(90),
    );
    painter.circle_filled(centre, radius + 3.0, Color32::from_rgb(18, 22, 30));
    // the map, as a textured fan
    let mut mesh = egui::Mesh::with_texture(src.texture);
    let uv = |s: Vec2| {
        let w = view.player + view.to_world(s, k);
        let t = (w - src.world_min) / src.world_size;
        pos2(t.x, t.y)
    };
    mesh.vertices.push(egui::epaint::Vertex {
        pos: centre,
        uv: uv(Vec2::ZERO),
        color: Color32::WHITE,
    });
    let n = 72;
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let s = Vec2::new(a.cos(), a.sin()) * radius;
        mesh.vertices.push(egui::epaint::Vertex {
            pos: pos2(c.x + s.x, c.y + s.y),
            uv: uv(s),
            color: Color32::WHITE,
        });
        if i > 0 {
            mesh.add_triangle(0, i as u32, i as u32 + 1);
        }
    }
    painter.add(egui::Shape::mesh(mesh));

    // places: inside the disc, or (towns, quest hubs, flight points) pinned to the rim
    for m in markers {
        let s = view.to_screen(m.world - view.player, k);
        let d = s.length();
        let pinned = matches!(
            m.kind,
            Some(PoiKind::Town | PoiKind::QuestHub | PoiKind::FlightPoint)
        );
        let (s, faded) = if d <= radius - 6.0 {
            (s, false)
        } else if pinned && d < radius * 8.0 {
            (s / d * (radius - 6.0), true)
        } else {
            continue;
        };
        let p = centre + vec2(s.x, s.y);
        let col = kind_color(m.kind);
        let col =
            Color32::from_rgba_unmultiplied(col[0], col[1], col[2], if faded { 170 } else { 255 });
        let r = if m.kind == Some(PoiKind::Town) {
            5.5
        } else {
            4.5
        };
        painter.circle_filled(p, r + 1.5, Color32::from_black_alpha(110));
        painter.circle_filled(p, r, col);
        painter.circle_stroke(p, r, Stroke::new(1.2, Color32::WHITE));
    }

    // rim and the north tick
    painter.circle_stroke(
        centre,
        radius,
        Stroke::new(2.5, Color32::from_rgb(214, 196, 150)),
    );
    painter.circle_stroke(
        centre,
        radius + 2.5,
        Stroke::new(1.0, Color32::from_black_alpha(160)),
    );
    let north = view
        .to_screen(Vec2::new(0.0, -1.0), 1.0)
        .normalize_or(Vec2::NEG_Y);
    let np = centre + vec2(north.x, north.y) * (radius + 1.0);
    painter.circle_filled(np, 8.0, Color32::from_rgb(28, 32, 40));
    painter.circle_stroke(np, 8.0, Stroke::new(1.2, Color32::from_rgb(214, 196, 150)));
    painter.text(
        np,
        egui::Align2::CENTER_CENTER,
        "N",
        FontId::proportional(10.5),
        Color32::from_rgb(240, 226, 190),
    );

    // the player
    let dir = if view.rotate {
        Vec2::NEG_Y
    } else {
        view.to_screen(Vec2::new(-view.yaw.sin(), -view.yaw.cos()), 1.0)
            .normalize_or(Vec2::NEG_Y)
    };
    let side = Vec2::new(-dir.y, dir.x);
    let tip = c + dir * 9.0;
    let l = c - dir * 6.0 + side * 6.0;
    let r = c - dir * 6.0 - side * 6.0;
    let back = c - dir * 3.0;
    let pts: Vec<Pos2> = [tip, l, back, r].iter().map(|p| pos2(p.x, p.y)).collect();
    painter.add(egui::Shape::convex_polygon(
        pts,
        Color32::from_rgb(255, 214, 92),
        Stroke::new(1.5, Color32::from_rgb(40, 30, 10)),
    ));
}

/// Where the HUD minimap sits (centre, radius): top right, below the frame-rate badge, its
/// frame-time graph and the draw-distance chip.
pub fn hud_spot(screen: egui::Rect) -> (Pos2, f32) {
    let radius = 92.0;
    (
        pos2(
            screen.max.x - 18.0 - radius - 3.0,
            screen.min.y + 196.0 + radius,
        ),
        radius,
    )
}

/// The standard HUD minimap (top right, under the FPS readout) for walk / playtest mode.
pub fn hud(
    ctx: &egui::Context,
    map: &mut WorldMap,
    gameplay: &Gameplay,
    player: Vec3,
    yaw: f32,
    screen: egui::Rect,
) {
    map.ensure_textures(ctx);
    let Some(src) = map.minimap_source() else {
        return;
    };
    let markers = markers(map, gameplay);
    let (centre, radius) = hud_spot(screen);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("minimap"),
    ));
    draw(
        &painter,
        centre,
        radius,
        &src,
        &View {
            player: Vec2::new(player.x, player.z),
            yaw,
            radius_m: 260.0,
            rotate: true,
        },
        &markers,
    );
}

/// Places to mark: the generated map's icons, else the world's POIs and player start.
pub fn markers(map: &WorldMap, gameplay: &Gameplay) -> Vec<Marker> {
    if let Some(r) = &map.result
        && !r.pois.is_empty()
    {
        return r.pois.clone();
    }
    let mut v: Vec<Marker> = gameplay
        .pois
        .iter()
        .map(|p| Marker {
            world: Vec2::new(p.pos[0], p.pos[2]),
            kind: Some(p.kind),
        })
        .collect();
    if let Some(s) = &gameplay.player_start {
        v.push(Marker {
            world: Vec2::new(s.pos[0], s.pos[2]),
            kind: None,
        });
    }
    v
}
