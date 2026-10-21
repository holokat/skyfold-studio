//! Tuning harness for the editor's campfire smoke: prints how high the tracers get over time.
//! `cargo run -p particle_fluids --release --example campfire -- [half] [height] [budget] [heat] [cooling] [radius]`
use glam::Vec3;
use particle_fluids::*;

fn main() {
    let a: Vec<f32> = std::env::args()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    let get = |i: usize, d: f32| a.get(i).copied().unwrap_or(d);
    let (half, h, budget, heat, cooling, radius) = (
        get(0, 0.75),
        get(1, 4.6),
        get(2, 3200.0),
        get(3, 7.0),
        get(4, 0.3),
        get(5, 0.24),
    );
    let bounds = Aabb::new(Vec3::new(-half, -0.05, -half), Vec3::new(half, h, half));
    let mut cfg = SimConfig::new(
        Some(Container::closed(bounds)),
        ParticleSizing::Budget(budget as usize),
    );
    cfg.substeps = Some(2);
    let mut sim = Simulation::new(cfg);
    sim.add_smoke(SmokeOptions {
        source: Some(Vec3::new(0.0, 0.3, 0.0)),
        radius,
        rate: 1600.0,
        lifetime: 4.5,
        heat,
        cooling,
        wind: Vec3::ZERO,
        wind_coupling: 0.8,
        wind_tunnel: true,
        ..Default::default()
    })
    .unwrap();
    sim.start().unwrap();
    println!("particle radius {:.3}", sim.particle_radius());
    let mut pos = vec![];
    let t0 = std::time::Instant::now();
    for s in 0..360 {
        sim.advance(1.0 / 60.0).unwrap();
        if s % 60 == 59 {
            sim.smoke_tracers(&mut pos, None);
            let n = pos.len().max(1) as f32;
            let my = pos.iter().map(|p| p.y).sum::<f32>() / n;
            let top = pos.iter().filter(|p| p.y > h * 0.5).count();
            println!(
                "t={:.0}s tracers {} mean y {:.2} m, above half-height {}",
                (s + 1) as f32 / 60.0,
                pos.len(),
                my,
                top
            );
        }
    }
    println!(
        "{:.1} ms per step",
        t0.elapsed().as_secs_f32() * 1000.0 / 360.0
    );
}
