//! Liquid block collapsing in a tank (the reference's dam-break scene).

use particle_fluids::glam::Vec3;
use particle_fluids::*;

fn dam_break(radius: f32) -> (Simulation, LiquidHandle, Aabb) {
    let bounds = Aabb::new(Vec3::new(-0.3, 0.0, -0.2), Vec3::new(0.3, 0.6, 0.2));
    let mut sim = Simulation::new(SimConfig::new(
        Some(Container::tank(bounds)),
        ParticleSizing::Radius(radius),
    ));
    let water = sim
        .add_liquid(LiquidOptions {
            fill: LiquidFill::Box(Aabb::new(
                Vec3::new(-0.3, 0.0, -0.2),
                Vec3::new(-0.06, 0.3, 0.2),
            )),
            ..Default::default()
        })
        .unwrap();
    sim.start().unwrap();
    (sim, water, bounds)
}

#[test]
fn liquid_block_settles_without_exploding() {
    let r = 0.012;
    let (mut sim, water, bounds) = dam_break(r);
    let count = sim.liquid_count(water);
    assert!(count > 1500, "{count} particles");
    let spacing = 2.0 * r;
    let volume = count as f32 * spacing.powi(3);
    let floor = bounds.size().x * bounds.size().z;
    let expected_mean_y = 0.5 * volume / floor;
    let mut positions = Vec::new();
    let mut velocities = Vec::new();
    for step in 0..240 {
        sim.advance(FIXED_DT).unwrap();
        if step % 30 == 29 {
            sim.liquid_particles(water, &mut positions, Some(&mut velocities));
            let max_speed = velocities.iter().map(|v| v.length()).fold(0.0f32, f32::max);
            assert!(
                max_speed.is_finite() && max_speed < 5.0,
                "step {step}: max speed {max_speed}"
            );
        }
    }
    sim.liquid_particles(water, &mut positions, Some(&mut velocities));
    assert_eq!(positions.len(), count, "no liquid lost");
    for p in &positions {
        assert!(p.is_finite());
        assert!(bounds.expanded(1e-3).contains(*p), "escaped: {p}");
    }
    let mut rho = Vec::new();
    sim.liquid_densities(water, &mut rho);
    let compressed: Vec<f32> = rho.iter().copied().filter(|&d| d > 1000.0).collect();
    let mean_c = compressed.iter().sum::<f32>() / compressed.len().max(1) as f32;
    let std_c = (compressed.iter().map(|d| (d - mean_c).powi(2)).sum::<f32>()
        / compressed.len().max(1) as f32)
        .sqrt();
    let mean_all = rho.iter().sum::<f32>() / rho.len() as f32;
    eprintln!(
        "compressed n={} mean={mean_c:.1} std/mean={:.4}; all mean={mean_all:.1}",
        compressed.len(),
        std_c / mean_c
    );
    let mean_y = positions.iter().map(|p| p.y).sum::<f32>() / count as f32;
    let mean_speed = velocities.iter().map(|v| v.length()).sum::<f32>() / count as f32;
    eprintln!("mean y {mean_y:.4} (expected {expected_mean_y:.4}), mean speed {mean_speed:.4}");
    // Volume roughly conserved: the settled layer's mean height matches V / A.
    assert!(
        (mean_y / expected_mean_y - 1.0).abs() < 0.2,
        "mean y {mean_y} vs {expected_mean_y}"
    );
    // Comes to rest.
    assert!(mean_speed < 0.05, "mean speed {mean_speed}");
    // Spread across the whole floor.
    let max_x = positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    assert!(max_x > 0.2, "max x {max_x}");
}
