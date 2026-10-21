//! Decals on a dungeon's walls and floors: cracks, damp stains and drips, blood, scorch marks and
//! glowing runes, projected onto the surfaces (Bevy's forward decals). Their pictures are painted
//! here (`picture`), so there are no textures to ship. "Age the walls" (`age`) lays them out by
//! each room's style, purpose and lights; any can be added, moved or removed by hand. Saved with
//! the map (`MapDoc::decals`) and exported (Godot `Decal` nodes; the data and pictures for the
//! other engines).

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::ExtendedMaterial;
use bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use serde::{Deserialize, Serialize};

use super::model::{DungeonMap, MapDoc};
use super::plan::{Compiled, Role, RoomKind};
use crate::noise::{Rng, fbm};

/// What a decal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecalKind {
    #[default]
    Crack,
    Stain,
    Drip,
    Blood,
    Scorch,
    Rune,
}

impl DecalKind {
    pub const ALL: [DecalKind; 6] = [
        DecalKind::Crack,
        DecalKind::Stain,
        DecalKind::Drip,
        DecalKind::Blood,
        DecalKind::Scorch,
        DecalKind::Rune,
    ];
    pub fn id(self) -> &'static str {
        match self {
            DecalKind::Crack => "crack",
            DecalKind::Stain => "stain",
            DecalKind::Drip => "drip",
            DecalKind::Blood => "blood",
            DecalKind::Scorch => "scorch",
            DecalKind::Rune => "rune",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            DecalKind::Crack => "Crack",
            DecalKind::Stain => "Stain",
            DecalKind::Drip => "Drips",
            DecalKind::Blood => "Blood",
            DecalKind::Scorch => "Scorch",
            DecalKind::Rune => "Rune circle",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// Its colour (sRGB) and glow unless told otherwise.
    pub fn defaults(self) -> ([f32; 3], f32) {
        match self {
            DecalKind::Crack => ([0.08, 0.07, 0.06], 0.0),
            DecalKind::Stain => ([0.16, 0.15, 0.09], 0.0),
            DecalKind::Drip => ([0.12, 0.14, 0.1], 0.0),
            DecalKind::Blood => ([0.32, 0.02, 0.02], 0.0),
            DecalKind::Scorch => ([0.03, 0.025, 0.02], 0.0),
            DecalKind::Rune => ([0.45, 0.65, 1.0], 3.0),
        }
    }
}

/// A decal: where it lies, which way the surface under it faces, how big, turned how far about
/// that facing, its colour (sRGB) and glow (runes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decal {
    pub id: u32,
    pub kind: DecalKind,
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub size: f32,
    #[serde(default)]
    pub spin: f32,
    pub color: [f32; 3],
    #[serde(default)]
    pub glow: f32,
    /// laid out by "Age the walls" (replaced when it runs again)
    #[serde(default)]
    pub auto: bool,
}

impl Decal {
    pub fn new(kind: DecalKind, pos: Vec3, normal: Vec3, size: f32) -> Decal {
        let (color, glow) = kind.defaults();
        Decal {
            id: 0,
            kind,
            pos: pos.to_array(),
            normal: normal.normalize_or(Vec3::Y).to_array(),
            size,
            spin: 0.0,
            color,
            glow,
            auto: false,
        }
    }
    /// Its place: the projector's +Y along the surface's facing, turned about it.
    pub fn transform(&self) -> Transform {
        let n = Vec3::from_array(self.normal).normalize_or(Vec3::Y);
        Transform::from_translation(Vec3::from_array(self.pos))
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, n) * Quat::from_rotation_y(self.spin))
            .with_scale(Vec3::new(self.size, self.size.min(1.2), self.size))
    }
}

// ------------------------------------------------------------------ the pictures

const PX: usize = 256;

/// A decal's picture: white where it marks the surface (alpha), tinted by its colour.
pub fn picture(kind: DecalKind) -> Vec<u8> {
    let mut a = vec![0f32; PX * PX];
    let seed = kind as u32 * 977 + 13;
    let f = |x: f32, y: f32, o: u32, s: u32| fbm(x, y, o, 2.0, 0.5, s) * 0.5 + 0.5;
    match kind {
        DecalKind::Crack => {
            // branching cracks from the middle
            let mut rng = Rng::new(seed as u64);
            let mut stack: Vec<(Vec2, f32, f32, u32)> = (0..3)
                .map(|k| {
                    (
                        Vec2::splat(0.5),
                        k as f32 * 2.1 + rng.range(-0.4, 0.4),
                        0.012,
                        0,
                    )
                })
                .collect();
            while let Some((mut p, mut dir, w, depth)) = stack.pop() {
                let steps = 26 - depth * 6;
                for _ in 0..steps {
                    dir += rng.range(-0.5, 0.5);
                    let q = p + Vec2::from_angle(dir) * 0.018;
                    stamp_line(&mut a, p, q, w);
                    p = q;
                    if depth < 2 && rng.f32() < 0.08 {
                        stack.push((
                            p,
                            dir + rng.range(0.6, 1.2) * if rng.f32() < 0.5 { 1.0 } else { -1.0 },
                            w * 0.65,
                            depth + 1,
                        ));
                    }
                    if p.x < 0.04 || p.y < 0.04 || p.x > 0.96 || p.y > 0.96 {
                        break;
                    }
                }
            }
        }
        DecalKind::Stain => {
            for j in 0..PX {
                for i in 0..PX {
                    let (x, y) = (i as f32 / PX as f32, j as f32 / PX as f32);
                    let r = Vec2::new(x - 0.5, y - 0.5).length() * 2.0;
                    let n = f(x * 3.0, y * 3.0, 4, seed);
                    a[j * PX + i] = smooth(0.45, 0.75, n + (1.0 - r) * 0.55 - 0.3)
                        * (1.0 - smooth(0.75, 1.0, r))
                        * (0.55 + 0.45 * f(x * 11.0, y * 11.0, 2, seed + 5));
                }
            }
        }
        DecalKind::Drip => {
            // streaks running down from the top edge
            for j in 0..PX {
                for i in 0..PX {
                    let (x, y) = (i as f32 / PX as f32, j as f32 / PX as f32);
                    let col = f(x * 18.0, 0.3, 2, seed);
                    let len = 0.35 + col * 0.6;
                    let fade = (1.0 - (y / len).min(1.0)).powf(1.6);
                    let edge = 1.0 - smooth(0.42, 0.5, (x - 0.5).abs());
                    a[j * PX + i] = smooth(0.48, 0.62, col)
                        * fade
                        * edge
                        * (0.6 + 0.4 * f(x * 40.0, y * 6.0, 2, seed + 3));
                }
            }
        }
        DecalKind::Blood => {
            let mut rng = Rng::new(seed as u64);
            for j in 0..PX {
                for i in 0..PX {
                    let (x, y) = (i as f32 / PX as f32, j as f32 / PX as f32);
                    let r = Vec2::new(x - 0.5, y - 0.5).length() * 2.0;
                    a[j * PX + i] =
                        smooth(0.52, 0.6, f(x * 4.0, y * 4.0, 4, seed) + (0.6 - r) * 0.6);
                }
            }
            // droplets thrown out round it
            for _ in 0..28 {
                let ang = rng.range(0.0, std::f32::consts::TAU);
                let d = rng.range(0.25, 0.46);
                let p = Vec2::splat(0.5) + Vec2::from_angle(ang) * d;
                stamp_disc(&mut a, p, rng.range(0.006, 0.02));
            }
        }
        DecalKind::Scorch => {
            for j in 0..PX {
                for i in 0..PX {
                    let (x, y) = (i as f32 / PX as f32, j as f32 / PX as f32);
                    let r = Vec2::new(x - 0.5, y - 0.5).length() * 2.0;
                    let n = f(x * 5.0, y * 5.0, 4, seed);
                    a[j * PX + i] = ((1.0 - r).max(0.0).powf(1.3) * (0.55 + 0.6 * n)).min(1.0);
                }
            }
        }
        DecalKind::Rune => {
            // a ring of glyphs between two circles, a star inside
            let c = Vec2::splat(0.5);
            ring(&mut a, c, 0.45, 0.012);
            ring(&mut a, c, 0.36, 0.008);
            ring(&mut a, c, 0.16, 0.006);
            let mut rng = Rng::new(seed as u64);
            for k in 0..12 {
                let ang = k as f32 / 12.0 * std::f32::consts::TAU;
                let base = c + Vec2::from_angle(ang) * 0.405;
                let t = Vec2::from_angle(ang + std::f32::consts::FRAC_PI_2);
                let rad = Vec2::from_angle(ang);
                // a little glyph: two or three strokes
                for _ in 0..(2 + rng.below(2)) {
                    let p = base + t * rng.range(-0.025, 0.025) + rad * rng.range(-0.025, 0.025);
                    let q = base + t * rng.range(-0.025, 0.025) + rad * rng.range(-0.025, 0.025);
                    stamp_line(&mut a, p, q, 0.004);
                }
            }
            for k in 0..5 {
                let p = c + Vec2::from_angle(k as f32 * std::f32::consts::TAU / 5.0) * 0.36;
                let q = c + Vec2::from_angle((k + 2) as f32 * std::f32::consts::TAU / 5.0) * 0.36;
                stamp_line(&mut a, p, q, 0.005);
            }
        }
    }
    let mut out = Vec::with_capacity(PX * PX * 4);
    for v in a {
        let v = (v.clamp(0.0, 1.0) * 255.0) as u8;
        out.extend_from_slice(&[255, 255, 255, v]);
    }
    out
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn stamp_disc(a: &mut [f32], c: Vec2, r: f32) {
    let (x0, x1) = (
        ((c.x - r) * PX as f32) as i32 - 1,
        ((c.x + r) * PX as f32) as i32 + 1,
    );
    let (y0, y1) = (
        ((c.y - r) * PX as f32) as i32 - 1,
        ((c.y + r) * PX as f32) as i32 + 1,
    );
    for j in y0.max(0)..=y1.min(PX as i32 - 1) {
        for i in x0.max(0)..=x1.min(PX as i32 - 1) {
            let p = Vec2::new((i as f32 + 0.5) / PX as f32, (j as f32 + 0.5) / PX as f32);
            let v = 1.0 - smooth(r * 0.6, r, p.distance(c));
            let k = j as usize * PX + i as usize;
            a[k] = a[k].max(v);
        }
    }
}

fn stamp_line(a: &mut [f32], p: Vec2, q: Vec2, w: f32) {
    let n = ((p.distance(q) / (w * 0.5).max(0.001)).ceil() as usize).max(1);
    for k in 0..=n {
        stamp_disc(a, p.lerp(q, k as f32 / n as f32), w);
    }
}

fn ring(a: &mut [f32], c: Vec2, r: f32, w: f32) {
    let n = (r * 600.0) as usize;
    for k in 0..n {
        let p = c + Vec2::from_angle(k as f32 / n as f32 * std::f32::consts::TAU) * r;
        stamp_disc(a, p, w);
    }
}

/// A decal's picture as an image (clamped at its edges).
fn decal_image(kind: DecalKind) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: PX as u32,
            height: PX as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        picture(kind),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        ..ImageSamplerDescriptor::linear()
    });
    img
}

/// A decal's picture as a PNG (the exports).
pub fn png(kind: DecalKind) -> Vec<u8> {
    let img = image::RgbaImage::from_raw(PX as u32, PX as u32, picture(kind)).unwrap_or_default();
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

// ------------------------------------------------------------------ aging

/// "Age the walls": decals by each room's style, purpose and lights. `density` 1 is the
/// style's own; ids are given by the caller.
pub fn age(m: &MapDoc, c: &Compiled, density: f32) -> Vec<Decal> {
    let mut out = Vec::new();
    let damp = matches!(
        m.mood.theme.as_str(),
        "sewer" | "cavern" | "fungal" | "sunken" | "haunted"
    ) || m.style.contains("sewer")
        || m.style.contains("overgrown")
        || m.style.contains("cave");
    for r in &c.rooms {
        if !r.kind.is_space()
            || r.kind.is_pit()
            || matches!(r.kind, RoomKind::Pool | RoomKind::Stairs | RoomKind::Spiral)
        {
            continue;
        }
        let mut rng = Rng::new(r.seed as u64 ^ 0xdeca1);
        let style = super::styles::get(r.style.as_deref().unwrap_or(&m.style));
        let wear = (style.weathering()[1] + style.rubble()) * 0.5 * density;
        let lp = super::geom::resample_loop(&r.shape.outline(), 1.0);
        let perim = lp.len() as f32;
        let top = (r.height - 0.6).min(5.0).max(1.5);
        // a spot on a wall (not a doorway): the point, the way into the room
        let wall_spot = |rng: &mut Rng| -> Option<(Vec2, Vec2)> {
            for _ in 0..6 {
                let i = rng.below(lp.len() as u32) as usize;
                let (a, b) = (lp[i], lp[(i + 1) % lp.len()]);
                let mid = (a + b) * 0.5;
                let nrm = (b - a).perp().normalize_or_zero();
                let inward = if r.inside(mid + nrm * 0.2) > 0.0 {
                    nrm
                } else {
                    -nrm
                };
                let floor = r.floor_at(mid + inward * 0.3);
                if !c.air(Vec3::new(
                    mid.x - inward.x * 0.3,
                    floor + 1.5,
                    mid.y - inward.y * 0.3,
                )) {
                    return Some((mid, inward));
                }
            }
            None
        };
        // cracks up the walls
        for _ in 0..((perim / 9.0 * wear).round() as usize) {
            if let Some((p, n)) = wall_spot(&mut rng) {
                let y = r.floor_at(p + n * 0.3) + rng.range(0.6, top);
                let mut d = Decal::new(
                    DecalKind::Crack,
                    Vec3::new(p.x, y, p.y),
                    Vec3::new(n.x, 0.0, n.y),
                    rng.range(0.9, 2.4),
                );
                d.spin = rng.range(0.0, std::f32::consts::TAU);
                out.push(d);
            }
        }
        // damp: drips down from high up, stains at the foot of the walls
        if damp || style.weathering()[2] > 0.3 {
            for _ in 0..((perim / 12.0 * density).round() as usize) {
                if let Some((p, n)) = wall_spot(&mut rng) {
                    let s = rng.range(1.2, 2.6);
                    let y = r.floor_at(p + n * 0.3) + top - s * 0.45;
                    out.push(Decal::new(
                        DecalKind::Drip,
                        Vec3::new(p.x, y, p.y),
                        Vec3::new(n.x, 0.0, n.y),
                        s,
                    ));
                }
            }
        }
        for _ in 0..((perim / 14.0 * wear).round() as usize) {
            if let Some((p, n)) = wall_spot(&mut rng) {
                let q = p + n * rng.range(0.4, 1.4);
                let mut d = Decal::new(
                    DecalKind::Stain,
                    Vec3::new(q.x, r.floor_at(q), q.y),
                    Vec3::Y,
                    rng.range(1.4, 3.2),
                );
                d.spin = rng.range(0.0, std::f32::consts::TAU);
                out.push(d);
            }
        }
        // blood where there's been fighting or worse
        if matches!(
            r.role,
            Role::Prison | Role::Lair | Role::Arena | Role::Boss | Role::Ritual
        ) {
            for _ in 0..(1 + rng.below(3)) {
                let at = r.shape.centre()
                    + Vec2::from_angle(rng.range(0.0, std::f32::consts::TAU))
                        * rng.range(0.0, r.inr * 0.6);
                if r.inside(at) > 0.5 {
                    let mut d = Decal::new(
                        DecalKind::Blood,
                        Vec3::new(at.x, r.floor_at(at), at.y),
                        Vec3::Y,
                        rng.range(0.8, 2.0),
                    );
                    d.spin = rng.range(0.0, std::f32::consts::TAU);
                    out.push(d);
                }
            }
        }
        // a rune circle on the floor of rituals, shrines and the boss's room
        if matches!(r.role, Role::Ritual | Role::Shrine | Role::Boss) && r.inr > 3.0 {
            let at = r.shape.centre();
            let mut d = Decal::new(
                DecalKind::Rune,
                Vec3::new(at.x, r.floor_at(at), at.y),
                Vec3::Y,
                (r.inr * 1.1).min(7.0),
            );
            let tint = m.mood.tinted([0.45, 0.65, 1.0]);
            d.color = tint;
            out.push(d);
        }
    }
    // soot over the fires: on the wall above a torch, the floor round a brazier
    for l in &m.lights {
        let p = Vec3::from_array(l.pos);
        match l.kind {
            super::lights::LightKind::Torch | super::lights::LightKind::Lantern => {
                // the wall behind: the nearest wall within a metre
                let q = Vec2::new(p.x, p.z);
                let dir = [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y]
                    .into_iter()
                    .map(|d| {
                        (
                            d,
                            (1..=10)
                                .map(|k| k as f32 * 0.1)
                                .find(|s| !c.air(Vec3::new(q.x + d.x * s, p.y, q.y + d.y * s))),
                        )
                    })
                    .filter_map(|(d, s)| s.map(|s| (d, s)))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((d, s)) = dir {
                    let w = q + d * (s - 0.02);
                    out.push(Decal::new(
                        DecalKind::Scorch,
                        Vec3::new(w.x, p.y + 0.55, w.y),
                        Vec3::new(-d.x, 0.0, -d.y),
                        1.1,
                    ));
                }
            }
            super::lights::LightKind::Brazier => {
                let mut d = Decal::new(
                    DecalKind::Scorch,
                    Vec3::new(p.x, p.y - 1.3, p.z),
                    Vec3::Y,
                    2.2,
                );
                d.color = [0.05, 0.04, 0.035];
                out.push(d);
            }
            _ => {}
        }
    }
    for d in &mut out {
        d.auto = true;
    }
    out
}

// ------------------------------------------------------------------ in the view

#[derive(Component)]
pub struct DecalEntity {
    id: u32,
}

#[derive(Resource, Default)]
pub struct Decals {
    images: HashMap<DecalKind, Handle<Image>>,
    mats: HashMap<(DecalKind, [u8; 4]), Handle<ForwardDecalMaterial<StandardMaterial>>>,
    shown: HashMap<u32, (Entity, Decal)>,
}

/// The map's decals as forward decals (the cut-away hides those above the cut).
pub(super) fn sync(
    mut commands: Commands,
    mut dec: ResMut<Decals>,
    map: Res<DungeonMap>,
    caves: Res<crate::caves::render::CaveWorld>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<ForwardDecalMaterial<StandardMaterial>>>,
    mut vis: Query<&mut Visibility, With<DecalEntity>>,
) {
    let Some(m) = map
        .0
        .as_ref()
        .filter(|m| m.kind == super::model::MapKind::Interior)
    else {
        for (_, (e, _)) in dec.shown.drain() {
            commands.entity(e).despawn();
        }
        return;
    };
    if map.is_changed() {
        let want: HashMap<u32, &Decal> = m.decals.iter().map(|d| (d.id, d)).collect();
        let stale: Vec<u32> = dec
            .shown
            .iter()
            .filter(|(id, (_, d))| want.get(id).is_none_or(|w| *w != d))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some((e, _)) = dec.shown.remove(&id) {
                commands.entity(e).despawn();
            }
        }
        for d in &m.decals {
            if dec.shown.contains_key(&d.id) {
                continue;
            }
            let img = dec
                .images
                .entry(d.kind)
                .or_insert_with(|| images.add(decal_image(d.kind)))
                .clone();
            let key = (
                d.kind,
                [
                    (d.color[0] * 255.0) as u8,
                    (d.color[1] * 255.0) as u8,
                    (d.color[2] * 255.0) as u8,
                    (d.glow * 20.0).min(255.0) as u8,
                ],
            );
            let mat = dec
                .mats
                .entry(key)
                .or_insert_with(|| {
                    let c = Color::srgb(d.color[0], d.color[1], d.color[2]);
                    let lin = c.to_linear();
                    mats.add(ExtendedMaterial {
                        base: StandardMaterial {
                            base_color: c,
                            base_color_texture: Some(img.clone()),
                            emissive: LinearRgba::rgb(
                                lin.red * d.glow * 4.0,
                                lin.green * d.glow * 4.0,
                                lin.blue * d.glow * 4.0,
                            ),
                            emissive_texture: (d.glow > 0.0).then(|| img.clone()),
                            alpha_mode: AlphaMode::Blend,
                            perceptual_roughness: if d.kind == DecalKind::Blood {
                                0.25
                            } else {
                                0.9
                            },
                            ..default()
                        },
                        extension: ForwardDecalMaterialExt {
                            depth_fade_factor: 0.6,
                        },
                    })
                })
                .clone();
            let e = commands
                .spawn((
                    Name::new(format!("Dungeon decal {}", d.kind.id())),
                    DecalEntity { id: d.id },
                    ForwardDecal,
                    MeshMaterial3d(mat),
                    d.transform(),
                ))
                .id();
            dec.shown.insert(d.id, (e, d.clone()));
        }
    }
    // (the editing view's cut)
    let cut = caves.dungeon.as_ref().and_then(|s| s.cut);
    for (_, (e, d)) in dec.shown.iter() {
        if let Ok(mut v) = vis.get_mut(*e) {
            let want = if cut.is_some_and(|c| d.pos[1] > c) {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
            if *v != want {
                *v = want;
            }
        }
    }
}
