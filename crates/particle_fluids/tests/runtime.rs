//! Heightfield terrain, runtime emission and retirement, and determinism.

use particle_fluids::glam::Vec3;
use particle_fluids::*;

fn slope_terrain(slope: f32) -> Heightfield {
    let mut hf = Heightfield::from_fn(Vec3::new(-1.0, 0.0, -0.5), 0.05, 41, 21, |x, z| {
        0.4 + slope * x + 0.02 * (6.0 * z).sin()
    });
    hf.mu_s = 0.1;
    hf.mu_k = 0.05;
    hf
}

#[test]
fn particles_on_sloped_heightfield_stay_above_and_slide_downhill() {
    let bounds = Aabb::new(Vec3::new(-1.0, -0.5, -0.5), Vec3::new(1.0, 1.5, 0.5));
    let mut sim = Simulation::new(SimConfig::new(
        Some(Container::open(bounds)),
        ParticleSizing::Radius(0.01),
    ));
    sim.set_heightfield(Some(slope_terrain(0.35)));
    let water = sim
        .add_liquid(LiquidOptions {
            fill: LiquidFill::Box(Aabb::new(
                Vec3::new(0.2, 0.75, -0.08),
                Vec3::new(0.36, 0.85, 0.08),
            )),
            keep_outside_container: true,
            ..Default::default()
        })
        .unwrap();
    sim.start().unwrap();
    let hf = sim.heightfield().unwrap().clone();
    let mut p = Vec::new();
    sim.liquid_particles(water, &mut p, None);
    let n = p.len();
    let x0 = p.iter().map(|q| q.x).sum::<f32>() / n as f32;
    let mut min_clearance = f32::MAX;
    for _ in 0..150 {
        sim.advance(FIXED_DT).unwrap();
        sim.liquid_particles(water, &mut p, None);
        for q in &p {
            min_clearance = min_clearance.min(q.y - hf.height(q.x, q.z));
        }
    }
    let x1 = p.iter().map(|q| q.x).sum::<f32>() / n as f32;
    eprintln!("{n} particles, min clearance {min_clearance:.4}, mean x {x0:.3} -> {x1:.3}");
    assert_eq!(p.len(), n);
    assert!(
        min_clearance > 0.0,
        "a particle ended below the terrain: {min_clearance}"
    );
    assert!(x1 < x0 - 0.2, "did not slide downhill: {x0} -> {x1}");
}

#[test]
fn soft_body_rests_on_heightfield() {
    let mut sim = Simulation::new(SimConfig::new(None, ParticleSizing::Radius(0.0167)));
    let mut hf = slope_terrain(0.0);
    hf.mu_s = 0.6;
    hf.mu_k = 0.5;
    sim.set_heightfield(Some(hf.clone()));
    let body = sim
        .add_softbody(SoftbodyOptions::new(TriangleMesh::cuboid(
            Vec3::new(0.0, 0.7, 0.0),
            Vec3::splat(0.1),
        )))
        .unwrap();
    for _ in 0..180 {
        sim.advance(FIXED_DT).unwrap();
    }
    let mut p = Vec::new();
    sim.softbody_particles(body, &mut p);
    let clearance = p
        .iter()
        .map(|q| q.y - hf.height(q.x, q.z))
        .fold(f32::MAX, f32::min);
    assert!(clearance > 0.0 && clearance < 0.04, "clearance {clearance}");
}

#[test]
fn rain_retires_on_terrain_with_splash_reports() {
    let bounds = Aabb::new(Vec3::new(-1.0, -0.5, -0.5), Vec3::new(1.0, 1.5, 0.5));
    let mut cfg = SimConfig::new(Some(Container::open(bounds)), ParticleSizing::Radius(0.01));
    cfg.reserve = 64;
    let mut sim = Simulation::new(cfg);
    sim.set_heightfield(Some(slope_terrain(0.2)));
    let rain = sim
        .add_liquid(LiquidOptions {
            retire_on_heightfield: true,
            ..Default::default()
        })
        .unwrap();
    sim.start().unwrap();
    for k in 0..10 {
        sim.emit_liquid(
            rain,
            Vec3::new(-0.4 + 0.08 * k as f32, 0.9, 0.0),
            Vec3::new(0.0, -3.0, 0.0),
        )
        .unwrap();
    }
    let mut splashes = Vec::new();
    for _ in 0..60 {
        sim.advance(FIXED_DT).unwrap();
        splashes.extend_from_slice(sim.retired());
    }
    let hf = sim.heightfield().unwrap();
    assert_eq!(splashes.len(), 10, "every drop splashed");
    for s in &splashes {
        assert_eq!(s.reason, RetireReason::TouchedHeightfield);
        assert!((s.position.y - hf.height(s.position.x, s.position.z)).abs() < 0.05);
        assert!(s.velocity.y < 0.0);
    }
    assert_eq!(sim.liquid_count(rain), 0);
    assert_eq!(sim.free_slots(), 64);
}

#[test]
fn emission_past_capacity_fails_and_retirement_frees_slots() {
    let bounds = Aabb::new(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
    let mut cfg = SimConfig::new(Some(Container::open(bounds)), ParticleSizing::Radius(0.01));
    cfg.reserve = 50;
    let mut sim = Simulation::new(cfg);
    let drops = sim
        .add_liquid(LiquidOptions {
            kill_height: Some(0.3),
            ..Default::default()
        })
        .unwrap();
    sim.start().unwrap();
    assert_eq!(sim.capacity(), 50);
    let mut ok = 0;
    let mut full = 0;
    for k in 0..60 {
        let x = -0.4 + 0.1 * (k % 9) as f32;
        let z = -0.4 + 0.1 * (k / 9) as f32;
        match sim.emit_liquid(drops, Vec3::new(x, 0.8, z), Vec3::ZERO) {
            Ok(_) => ok += 1,
            Err(SimError::NoFreeSlots) => full += 1,
            Err(e) => panic!("{e}"),
        }
    }
    assert_eq!((ok, full), (50, 10));
    assert_eq!(sim.free_slots(), 0);
    let mut retired = 0;
    for _ in 0..40 {
        sim.advance(FIXED_DT).unwrap();
        retired += sim.retired().len();
    }
    assert_eq!(retired, 50, "every drop fell below the kill height");
    assert!(
        sim.retired()
            .iter()
            .all(|r| r.reason == RetireReason::BelowKillHeight)
    );
    assert_eq!(sim.free_slots(), 50);
    assert_eq!(sim.liquid_count(drops), 0);
    // Freed slots are reused.
    for k in 0..50 {
        sim.emit_liquid(
            drops,
            Vec3::new(-0.4 + 0.016 * k as f32, 0.8, 0.0),
            Vec3::ZERO,
        )
        .unwrap();
    }
    assert!(sim.emit_liquid(drops, Vec3::ZERO, Vec3::ZERO).is_err());
    sim.advance(FIXED_DT).unwrap();
    assert_eq!(sim.liquid_count(drops), 50);
}

/// A scene with liquid, cloth, colliders and runtime emission.
fn mixed_scene(threads: Option<usize>) -> Vec<Vec3> {
    let bounds = Aabb::new(Vec3::new(-0.4, 0.0, -0.3), Vec3::new(0.4, 0.8, 0.3));
    let mut cfg = SimConfig::new(Some(Container::tank(bounds)), ParticleSizing::Radius(0.014));
    cfg.reserve = 200;
    cfg.threads = threads;
    cfg.substeps = Some(6);
    let mut sim = Simulation::new(cfg);
    let water = sim
        .add_liquid(LiquidOptions {
            fill: LiquidFill::Box(Aabb::new(
                Vec3::new(-0.4, 0.0, -0.3),
                Vec3::new(-0.1, 0.3, 0.3),
            )),
            ..Default::default()
        })
        .unwrap();
    let cloth = sim
        .add_cloth(ClothOptions {
            width: 0.3,
            height: 0.3,
            position: Vec3::new(0.2, 0.5, 0.0),
            pins: ClothPins::TopCorners,
            wind: Vec3::new(0.0, 0.0, 1.0),
            ..Default::default()
        })
        .unwrap();
    let ball = sim.add_sphere(Vec3::new(0.0, 0.15, 0.0), 0.08, ColliderOptions::default());
    for step in 0..60 {
        if step % 5 == 0 {
            let _ = sim.emit_liquid(water, Vec3::new(0.2, 0.75, 0.0), Vec3::new(0.0, -1.0, 0.0));
        }
        let t = step as f32 * FIXED_DT;
        sim.move_collider(
            ball,
            Vec3::new(0.1 * t.sin(), 0.15, 0.0),
            particle_fluids::glam::Quat::IDENTITY,
        );
        sim.advance(FIXED_DT).unwrap();
    }
    let mut out = Vec::new();
    sim.for_each_particle(|_, _, p, v| {
        out.push(p);
        out.push(v);
    });
    let _ = cloth;
    out
}

#[test]
fn runs_are_deterministic_at_any_thread_count() {
    let a = mixed_scene(Some(1));
    let b = mixed_scene(Some(1));
    let c = mixed_scene(Some(4));
    let d = mixed_scene(None);
    assert!(!a.is_empty());
    assert_eq!(a, b, "same inputs, same result");
    assert_eq!(a, c, "single- and multi-threaded runs match bit for bit");
    assert_eq!(a, d);
}
