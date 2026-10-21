//! Tulip construction. The flower is an open goblet made from six overlapping arc-shaped
//! tepals; its silhouette belongs to the geometry, while the atlas supplies veins and bloom.

use super::{
    ONE, Paint, PaintState, Plant, Regions, Rng, TAU, TULIP_COLOURS, Vec3, c, curve, dir_at,
    tulip_aux,
};

const PETALS: usize = 6;

/// Builds a tulip with stable landmarks at every level of detail. LOD only reduces arc samples:
/// every version retains the same open, six-petal goblet rather than dropping to flower cards.
pub(super) fn build(rng: &mut Rng, lod: u8, colour: usize, reg: &Regions) -> Plant {
    let stem_h = rng.range(0.38, 0.5);
    let yaw = rng.f32() * TAU;
    let bend = Vec3::new(rng.range(-0.018, 0.018), 0.0, rng.range(-0.018, 0.018));
    // Shared, bounded variation keeps a plant's landmarks coherent at every LOD. It changes the
    // overall cup, never individual petals, so the two overlapping whorls still meet cleanly.
    let head_height = rng.range(0.96, 1.0);
    let openness = rng.range(0.97, 1.0);
    let flower = Vec3::Y * stem_h + bend;
    // The raised inner rim reaches 120 mm above the flower base; retain a little headroom so
    // culling and wind bounds include the entire open cup.
    let mut p = Plant::new(stem_h + 0.13);
    let (r, g, b) = TULIP_COLOURS[colour % TULIP_COLOURS.len()];
    let petal = c(r, g, b);

    // Tulip stems are slender, round, and just bowed enough to avoid a field of rigid sticks.
    p.paint(reg, "green_stem", c(76, 122, 66), 0.62);
    p.part(tulip_aux::part(tulip_aux::STEM, stem_h));
    let stem = curve(Vec3::ZERO, flower, bend * 0.55, [2, 1, 1][lod as usize]);
    p.tube(
        &stem,
        &[0.0045, 0.004, 0.0035],
        [4, 3, 3][lod as usize],
        &[ONE * 0.92, ONE, ONE * 0.94],
        0.72,
        0.0,
        1.0,
    );

    p.paint(reg, "tulip_leaf", c(76, 122, 80), 0.58);
    p.part(tulip_aux::part(tulip_aux::LEAF, stem_h));
    for i in 0..[2, 1, 0][lod as usize] {
        let a = yaw + i as f32 * TAU / 3.0 + if i == 1 { 0.32 } else { -0.12 };
        let len = stem_h * (0.52 - i as f32 * 0.035);
        let base = Vec3::Y * (0.008 + i as f32 * 0.024) + Vec3::new(a.cos(), 0.0, a.sin()) * 0.006;
        p.blade(
            base,
            dir_at(a, 0.72 - i as f32 * 0.05),
            len,
            0.092 - i as f32 * 0.008,
            1.15,
            0.52,
            [3, 1, 1][lod as usize],
            ONE * (0.76 + i as f32 * 0.035),
            ONE * (0.98 - i as f32 * 0.025),
            0.72,
        );
    }

    // A low, shadowed centre remains visible from the open top, without filling the cup.
    if lod < 2 {
        p.plain(c(90, 68, 28), 0.74);
        p.part(tulip_aux::part(tulip_aux::CORE, stem_h));
        p.disc(
            flower + Vec3::Y * 0.018,
            bevy::prelude::Quat::IDENTITY,
            0.014,
            1,
            [5, 4][lod as usize],
            0.0015,
            ONE * 0.7,
            0.5,
        );
    }

    p.paint(reg, "tulip_petal", petal, 0.44);
    p.part(tulip_aux::part(tulip_aux::PETAL, stem_h));
    let texture_tint = if p.textured() { petal } else { ONE };
    p.paint = PaintState(Some(Paint {
        base: petal,
        ..p.cur()
    }));
    let v_samples: &[f32] = match lod {
        // Additional samples are only around the lower bulge and shoulder, where the cup curves.
        0 => &[0.0, 0.12, 0.36, 0.68, 1.0],
        1 => &[0.0, 0.50, 1.0],
        _ => &[0.0, 1.0],
    };
    for petal_index in 0..PETALS {
        let centre = yaw + petal_index as f32 * TAU / PETALS as f32;
        let outer = petal_index % 2 == 0;
        let variation = 0.94 + ((petal_index as f32 * 2.39996 + yaw).sin() + 1.0) * 0.025;
        tepal_arc(
            &mut p,
            flower,
            centre,
            v_samples,
            [4, 2, 1][lod as usize],
            texture_tint * variation * if outer { 0.96 } else { 1.03 },
            outer,
            lod == 2,
            head_height,
            openness,
        );
    }
    p
}

/// A single tulip tepal wrapped around the flower axis. The three outer petals sit a few mm
/// outside the three raised inner petals, so their overlaps read as lobes rather than coplanar
/// fan blades. `v=1` always has a positive radial radius: the cup never folds through its axis.
fn tepal_arc(
    p: &mut Plant,
    flower: Vec3,
    centre: f32,
    v_samples: &[f32],
    across: usize,
    tint: Vec3,
    outer: bool,
    rim_centre: bool,
    head_height: f32,
    openness: f32,
) {
    let arc_half = if outer { 0.72 } else { 0.64 };
    let side_lift = if outer { 0.003 } else { 0.006 };
    let arch = if outer { 0.019 } else { 0.021 };
    let radial_offset = if outer { 0.0035 } else { 0.0 };
    // The highest-quality mesh has five points on a row and five rows. Fixed storage removes the
    // rows allocation and one ring allocation per row for every petal, while preserving order.
    let mut rows = [[0; 5]; 5];
    let mut row_lens = [0; 5];
    for (row, &v) in v_samples.iter().enumerate() {
        let (radius, dr_dv) = cup_radius(v, outer);
        let base_y = (0.090 * v + 0.003 * v * v + side_lift * v.powf(2.2)) * head_height;
        let dy_dv = (0.090 + 0.006 * v + side_lift * 2.2 * v.max(0.001).powf(1.2)) * head_height;
        // The far mesh spends its last leaf's six vertices on a rounded petal crest: two lower
        // corners join three rim points, avoiding a cheap pointed or square-cut silhouette.
        let segments = if rim_centre && row + 1 == v_samples.len() {
            across + 1
        } else {
            across
        };
        row_lens[row] = segments + 1;
        for i in 0..=segments {
            let u = i as f32 / segments as f32;
            let s = u * 2.0 - 1.0;
            let theta = centre + s * arc_half;
            let radial = Vec3::new(theta.cos(), 0.0, theta.sin());
            let azimuth = Vec3::new(-theta.sin(), 0.0, theta.cos());
            // Rounded lobes rise at their centre and feather back towards adjoining petals.
            let lobe = (1.0 - s * s).max(0.0);
            let rim_shape = arch * lobe * v.powf(3.0) * head_height;
            let dr_ds = -0.0035 * s * (0.35 + 0.65 * v);
            let r = radius * openness + radial_offset - 0.00175 * s * s * (0.35 + 0.65 * v);
            let position = flower + radial * r + Vec3::Y * (base_y + rim_shape);
            let across_tangent = azimuth * (r * arc_half)
                + radial * dr_ds
                + Vec3::Y * (-2.0 * arch * s * v.powf(3.0) * head_height);
            let along_tangent = radial * (dr_dv * openness - 0.0011375 * s * s)
                + Vec3::Y * (dy_dv + arch * 3.0 * lobe * v.max(0.001).powf(2.0) * head_height);
            let normal = across_tangent.cross(along_tangent).normalize_or(radial);
            let base_shade = 0.68 + 0.32 * v.powf(0.55);
            let seam_shade = 0.94 + 0.06 * lobe;
            // The atlas owns fine veins and bloom. Its central opaque portion avoids imposing a
            // second, pinched sprite outline over this already rounded mesh silhouette.
            rows[row][i] = p.vert(
                position,
                normal,
                tint * base_shade * seam_shade,
                0.9,
                0.24 + 0.52 * u,
                0.32 + 0.40 * v,
            );
        }
    }
    for row in 0..v_samples.len() - 1 {
        let (lower, upper) = (&rows[row], &rows[row + 1]);
        let (lower_len, upper_len) = (row_lens[row], row_lens[row + 1]);
        if lower_len == upper_len {
            for i in 0..lower_len - 1 {
                p.quad(lower[i], lower[i + 1], upper[i + 1], upper[i]);
            }
        } else {
            debug_assert!(lower_len == 2 && upper_len == 3);
            p.tri(lower[0], lower[1], upper[1]);
            p.tri(lower[0], upper[1], upper[0]);
            p.tri(lower[1], upper[2], upper[1]);
        }
    }
}

/// Radius and derivative for the tulip's narrow attachment, rounded lower bowl, upright shoulder
/// and open rim. At the rim this is 75–80% of the maximum body radius, leaving an unmistakably
/// open goblet without a wide hole around the stem.
fn cup_radius(v: f32, outer: bool) -> (f32, f32) {
    let (base, body, rim) = if outer {
        (0.005, 0.041, 0.032)
    } else {
        (0.006, 0.038, 0.031)
    };
    if v < 0.52 {
        // A quarter ellipse starts narrow but rounds quickly into an upright shoulder. The guarded
        // derivative keeps the attachment ring finite instead of evaluating 1 / sqrt(0).
        let x = v / 0.55;
        let curve = (x * (2.0 - x)).max(0.0).sqrt();
        let d_curve = if x <= 0.001 || curve <= 1e-4 {
            0.0
        } else {
            (1.0 - x) / (0.55 * curve)
        };
        (base + (body - base) * curve, (body - base) * d_curve)
    } else {
        let t = (v - 0.52) / 0.48;
        let smooth = t * t * (3.0 - 2.0 * t);
        let ds_dv = 6.0 * t * (1.0 - t) / 0.48;
        (body + (rim - body) * smooth, (rim - body) * ds_dv)
    }
}
