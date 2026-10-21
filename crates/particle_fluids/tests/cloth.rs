//! Pinned cloth: hangs straight, keeps its edge lengths, and wind deflects it.

use particle_fluids::glam::{Quat, Vec3};
use particle_fluids::*;

fn curtain(wind: Vec3) -> (Simulation, ClothHandle) {
    let mut sim = Simulation::new(SimConfig::new(None, ParticleSizing::Radius(0.01)));
    let cloth = sim
        .add_cloth(ClothOptions {
            width: 0.6,
            height: 0.6,
            position: Vec3::new(0.0, 1.0, 0.0),
            pins: ClothPins::Top,
            wind,
            ..Default::default()
        })
        .unwrap();
    sim.start().unwrap();
    (sim, cloth)
}

/// Mean and max relative stretch of the grid's horizontal and vertical edges.
fn edge_stretch(sim: &Simulation, cloth: ClothHandle, width: f32, height: f32) -> (f32, f32) {
    let (cols, rows) = sim.cloth_grid(cloth).unwrap();
    let mut p = Vec::new();
    sim.cloth_particles(cloth, &mut p);
    let rest_x = width / (cols - 1) as f32;
    let rest_y = height / (rows - 1) as f32;
    let (mut sum, mut max, mut n) = (0.0f32, 0.0f32, 0);
    for r in 0..rows {
        for c in 0..cols {
            let i = (r * cols + c) as usize;
            if c + 1 < cols {
                let s = ((p[i + 1] - p[i]).length() / rest_x - 1.0).abs();
                sum += s;
                max = max.max(s);
                n += 1;
            }
            if r + 1 < rows {
                let s = ((p[i + cols as usize] - p[i]).length() / rest_y - 1.0).abs();
                sum += s;
                max = max.max(s);
                n += 1;
            }
        }
    }
    (sum / n as f32, max)
}

#[test]
fn pinned_top_cloth_hangs_straight() {
    let (mut sim, cloth) = curtain(Vec3::ZERO);
    let pins_before = sim.cloth_pin_positions(cloth);
    for _ in 0..120 {
        sim.advance(FIXED_DT).unwrap();
    }
    let mut p = Vec::new();
    sim.cloth_particles(cloth, &mut p);
    let (cols, rows) = sim.cloth_grid(cloth).unwrap();
    assert!(p.iter().all(|x| x.is_finite()));
    // Pins don't move.
    assert_eq!(sim.cloth_pin_positions(cloth), pins_before);
    // Hangs straight down in its plane: no sideways drift without wind.
    let max_z = p.iter().map(|x| x.z.abs()).fold(0.0f32, f32::max);
    assert!(max_z < 0.02, "max |z| {max_z}");
    // The bottom row hangs a full height below the top.
    let bottom: f32 = (0..cols)
        .map(|c| p[((rows - 1) * cols + c) as usize].y)
        .sum::<f32>()
        / cols as f32;
    let top = 1.3;
    assert!((top - bottom - 0.6).abs() < 0.03, "bottom row at {bottom}");
    let (mean, max) = edge_stretch(&sim, cloth, 0.6, 0.6);
    eprintln!("edge stretch mean {mean:.4} max {max:.4}");
    assert!(mean < 0.03, "mean stretch {mean}");
    assert!(max < 0.1, "max stretch {max}");
}

#[test]
fn wind_deflects_cloth() {
    let (mut sim, cloth) = curtain(Vec3::ZERO);
    for step in 0..180 {
        if step == 60 {
            sim.set_cloth_wind(cloth, Vec3::new(0.0, 0.0, 4.0));
        }
        sim.advance(FIXED_DT).unwrap();
    }
    let mut p = Vec::new();
    sim.cloth_particles(cloth, &mut p);
    let (cols, rows) = sim.cloth_grid(cloth).unwrap();
    let bottom_z: f32 = (0..cols)
        .map(|c| p[((rows - 1) * cols + c) as usize].z)
        .sum::<f32>()
        / cols as f32;
    eprintln!("bottom row z with wind: {bottom_z:.3}");
    assert!(bottom_z > 0.08, "bottom row z {bottom_z}");
    let (mean, _) = edge_stretch(&sim, cloth, 0.6, 0.6);
    assert!(mean < 0.05, "mean stretch {mean}");
}

#[test]
fn moving_pins_carry_the_cloth() {
    let (mut sim, cloth) = curtain(Vec3::ZERO);
    for step in 0..90 {
        let t = (step + 1) as f32 * FIXED_DT;
        sim.set_cloth_pin_transform(cloth, Vec3::new(0.5 * t, 1.0, 0.0), Quat::IDENTITY);
        sim.advance(FIXED_DT).unwrap();
    }
    let pins = sim.cloth_pin_positions(cloth);
    let expected_shift = 0.5 * 90.0 * FIXED_DT;
    let mean_x = pins.iter().map(|p| p.x).sum::<f32>() / pins.len() as f32;
    assert!((mean_x - expected_shift).abs() < 1e-3, "pins at {mean_x}");
    let mut p = Vec::new();
    sim.cloth_particles(cloth, &mut p);
    let mean_cloth_x = p.iter().map(|x| x.x).sum::<f32>() / p.len() as f32;
    assert!(
        mean_cloth_x > 0.4 * expected_shift,
        "cloth followed to {mean_cloth_x}"
    );
}

#[test]
fn cloth_surface_render_data() {
    let (mut sim, cloth) = curtain(Vec3::ZERO);
    for _ in 0..10 {
        sim.advance(FIXED_DT).unwrap();
    }
    let mut surface = ClothSurface::new(3, true);
    sim.fill_cloth_surface(cloth, &mut surface);
    let (cols, rows) = sim.cloth_grid(cloth).unwrap();
    let gx = (cols - 1) * 3 + 1;
    let gy = (rows - 1) * 3 + 1;
    assert_eq!(surface.positions.len() as u32, gx * gy);
    assert_eq!(surface.normals.len(), surface.positions.len());
    assert_eq!(surface.wetness.len(), surface.positions.len());
    assert_eq!(surface.indices.len() as u32, (gx - 1) * (gy - 1) * 6);
    // A flat curtain facing +z: normals point along +z.
    let mean_n = surface.normals.iter().copied().sum::<Vec3>() / surface.normals.len() as f32;
    assert!(mean_n.z > 0.9, "normals {mean_n}");
    // The first triangle winds counter-clockwise seen from the front (+z).
    let [a, b, c] = [0, 1, 2].map(|k| surface.positions[surface.indices[k] as usize]);
    assert!((b - a).cross(c - a).z > 0.0);
}
