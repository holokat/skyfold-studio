//! Smoke rises above its source; side wind bends the plume downwind.

use particle_fluids::glam::Vec3;
use particle_fluids::*;

/// The reference `examples/smoke.ts` scene: 5000 air particles in a 1 × 1.9 × 1 m
/// box, a 0.17 m heated source on the floor.
fn plume(wind: Vec3, tunnel: bool) -> (Simulation, Vec3) {
    let bounds = Aabb::new(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.9, 0.5));
    let mut sim = Simulation::new(SimConfig::new(
        Some(Container::closed(bounds)),
        ParticleSizing::Budget(5000),
    ));
    sim.add_smoke(SmokeOptions {
        radius: 0.17,
        rate: 2500.0,
        lifetime: 4.0,
        wind,
        wind_tunnel: tunnel,
        ..Default::default()
    })
    .unwrap();
    sim.start().unwrap();
    (sim, Vec3::new(0.0, 0.0, 0.0))
}

fn centroid(sim: &Simulation) -> (Vec3, usize) {
    let mut p = Vec::new();
    sim.smoke_tracers(&mut p, None);
    (
        p.iter().copied().sum::<Vec3>() / p.len().max(1) as f32,
        p.len(),
    )
}

#[test]
fn smoke_rises_above_the_source() {
    let (mut sim, source) = plume(Vec3::ZERO, false);
    assert_eq!(sim.gravity(), Vec3::new(0.0, -1.0, 0.0));
    let cap = sim.smoke_tracer_capacity();
    let mut max_count = 0;
    for _ in 0..240 {
        sim.advance(FIXED_DT).unwrap();
        max_count = max_count.max(sim.smoke_tracer_count());
    }
    let (c, n) = centroid(&sim);
    let mut air = Vec::new();
    let mut temperature = Vec::new();
    sim.smoke_air(&mut air, Some(&mut temperature));
    let hot = temperature.iter().filter(|&&t| t > 0.3).count();
    eprintln!(
        "{} air, {n} tracers (max {max_count}, capacity {cap}), centroid {c:.3}, {hot} hot air particles",
        air.len()
    );
    assert!(c.y > source.y + 0.2, "plume centroid at {}", c.y);
    assert!(c.x.abs() < 0.12 && c.z.abs() < 0.12, "plume leaned to {c}");
    assert!(max_count <= cap && max_count <= (2500.0 * 4.0 * 1.05) as usize);
    assert!(hot > 0);
    // Air stays inside the closed container.
    assert!(air.iter().all(|p| p.y > -1e-3 && p.y < 1.9 + 1e-3));
}

#[test]
fn side_wind_bends_the_plume_downwind() {
    let (mut calm, _) = plume(Vec3::ZERO, true);
    let (mut windy, _) = plume(Vec3::new(1.5, 0.0, 0.0), true);
    let mut max_count = 0;
    for _ in 0..180 {
        calm.advance(FIXED_DT).unwrap();
        windy.advance(FIXED_DT).unwrap();
        max_count = max_count.max(windy.smoke_tracer_count());
    }
    let (c0, n0) = centroid(&calm);
    let (c1, n1) = centroid(&windy);
    eprintln!("calm centroid {c0:.3} ({n0} tracers), windy centroid {c1:.3} ({n1} tracers)");
    assert!(
        c1.x > c0.x + 0.12,
        "plume did not drift downwind: {c0} vs {c1}"
    );
    // Tracers leaving the downwind side retire, so the windy plume holds fewer.
    assert!(n1 < n0, "{n1} vs {n0}");
    assert!(max_count <= windy.smoke_tracer_capacity());
    // Air re-enters upwind: none of it escapes the tunnel.
    let mut air = Vec::new();
    windy.smoke_air(&mut air, None);
    assert!(air.iter().all(|p| p.x >= -0.5 - 1e-3 && p.x <= 0.5 + 1e-3));
}

#[test]
fn smoke_source_moves_and_volume_builds() {
    let (mut sim, _) = plume(Vec3::ZERO, false);
    sim.set_smoke_source(Vec3::new(0.25, 0.0, 0.0));
    sim.set_smoke_rate(1500.0);
    sim.set_smoke_heat(4.0);
    for _ in 0..120 {
        sim.advance(FIXED_DT).unwrap();
    }
    let (c, _) = centroid(&sim);
    assert!(c.x > 0.1, "plume follows the moved source: {c}");
    let mut vol = SmokeVolume::default();
    sim.build_smoke_volume(&SmokeVolumeOptions::default(), &mut vol);
    let [nx, ny, nz] = vol.resolution;
    assert_eq!(vol.density.len(), (nx * ny * nz) as usize);
    let max = vol.density.iter().copied().fold(0.0f32, f32::max);
    assert!(max > 0.0);
    assert!(vol.light.iter().all(|&l| (0.0..=1.0 + 1e-6).contains(&l)));
    // Densest column sits over the moved source.
    let (mut best, mut best_x) = (0.0, 0);
    for x in 0..nx {
        let s: f32 = (0..nz)
            .flat_map(|z| (0..ny).map(move |y| (x, y, z)))
            .map(|(x, y, z)| vol.density[(x + nx * (y + ny * z)) as usize])
            .sum();
        if s > best {
            best = s;
            best_x = x;
        }
    }
    let world_x = vol.min.x + (best_x as f32 + 0.5) / nx as f32 * (vol.max.x - vol.min.x);
    assert!(world_x > 0.05, "densest column at x {world_x}");
}
