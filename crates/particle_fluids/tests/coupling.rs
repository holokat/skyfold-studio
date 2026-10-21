//! Fluid–solid coupling (buoyancy), baked mesh colliders, moving primitives,
//! and the liquid surface field.

use particle_fluids::glam::{Quat, Vec3};
use particle_fluids::*;

/// The reference `examples/floating.ts`: a light body floats, a heavy one sinks.
#[test]
fn light_body_floats_heavy_body_sinks() {
    let bounds = Aabb::new(Vec3::new(-0.35, 0.0, -0.2), Vec3::new(0.35, 0.7, 0.2));
    let mut cfg = SimConfig::new(Some(Container::tank(bounds)), ParticleSizing::Radius(0.015));
    cfg.substeps = Some(6);
    let mut sim = Simulation::new(cfg);
    sim.add_liquid(LiquidOptions {
        fill: LiquidFill::Box(Aabb::new(
            Vec3::new(-0.35, 0.0, -0.2),
            Vec3::new(0.35, 0.3, 0.2),
        )),
        ..Default::default()
    })
    .unwrap();
    let light = sim
        .add_softbody(SoftbodyOptions {
            density: 400.0,
            ..SoftbodyOptions::new(TriangleMesh::cuboid(
                Vec3::new(-0.15, 0.2, 0.0),
                Vec3::splat(0.06),
            ))
        })
        .unwrap();
    let heavy = sim
        .add_softbody(SoftbodyOptions {
            density: 2000.0,
            ..SoftbodyOptions::new(TriangleMesh::cuboid(
                Vec3::new(0.15, 0.2, 0.0),
                Vec3::splat(0.06),
            ))
        })
        .unwrap();
    for _ in 0..150 {
        sim.advance(FIXED_DT).unwrap();
    }
    let (cl, _) = sim.softbody_frame(light).unwrap();
    let (ch, _) = sim.softbody_frame(heavy).unwrap();
    eprintln!(
        "light body centre y {:.3}, heavy body centre y {:.3}",
        cl.y, ch.y
    );
    assert!(ch.y < 0.1, "heavy body should rest on the floor: {}", ch.y);
    assert!(
        cl.y > ch.y + 0.08,
        "light body should float above the heavy one: {} vs {}",
        cl.y,
        ch.y
    );
}

#[test]
fn liquid_pours_around_a_baked_mesh_collider() {
    let bounds = Aabb::new(Vec3::new(-0.4, 0.0, -0.4), Vec3::new(0.4, 0.8, 0.4));
    let mut sim = Simulation::new(SimConfig::new(
        Some(Container::tank(bounds)),
        ParticleSizing::Radius(0.012),
    ));
    let sphere = TriangleMesh::uv_sphere(Vec3::ZERO, 0.15, 32, 20);
    let sdf = bake_mesh_to_sdf(&sphere, 32, None).unwrap();
    let ball = sim.add_sdf_collider(
        sdf,
        Vec3::new(0.0, 0.2, 0.0),
        Quat::IDENTITY,
        1.0,
        None,
        ColliderOptions::default(),
    );
    let water = sim
        .add_liquid(LiquidOptions {
            fill: LiquidFill::Box(Aabb::new(
                Vec3::new(-0.12, 0.42, -0.12),
                Vec3::new(0.12, 0.55, 0.12),
            )),
            ..Default::default()
        })
        .unwrap();
    let mut p = Vec::new();
    let mut deepest = f32::MAX;
    for _ in 0..120 {
        sim.advance(FIXED_DT).unwrap();
        sim.liquid_particles(water, &mut p, None);
        for q in &p {
            deepest = deepest.min((*q - Vec3::new(0.0, 0.2, 0.0)).length() - 0.15);
        }
    }
    eprintln!("closest approach to the mesh surface: {deepest:.4}");
    assert!(deepest > 0.0, "a particle entered the mesh: {deepest}");
    // The liquid flowed over the ball and down to the floor around it.
    let low = p.iter().filter(|q| q.y < 0.1).count();
    assert!(low > p.len() / 2, "{low} of {} reached the floor", p.len());
    sim.set_collider_enabled(ball, false);
}

#[test]
fn moving_paddle_pushes_liquid() {
    let bounds = Aabb::new(Vec3::new(-0.4, 0.0, -0.15), Vec3::new(0.4, 0.5, 0.15));
    let mut sim = Simulation::new(SimConfig::new(
        Some(Container::tank(bounds)),
        ParticleSizing::Radius(0.012),
    ));
    let water = sim
        .add_liquid(LiquidOptions {
            fill: LiquidFill::Box(Aabb::new(
                Vec3::new(-0.4, 0.0, -0.15),
                Vec3::new(0.4, 0.1, 0.15),
            )),
            ..Default::default()
        })
        .unwrap();
    let paddle = sim.add_box(
        Vec3::new(-0.3, 0.1, 0.0),
        Vec3::new(0.02, 0.1, 0.14),
        Quat::IDENTITY,
        ColliderOptions::default(),
    );
    let mut p = Vec::new();
    sim.start().unwrap();
    sim.liquid_particles(water, &mut p, None);
    let x0 = p.iter().map(|q| q.x).sum::<f32>() / p.len() as f32;
    for step in 0..60 {
        let t = (step + 1) as f32 * FIXED_DT;
        sim.move_collider(paddle, Vec3::new(-0.3 + 0.4 * t, 0.1, 0.0), Quat::IDENTITY);
        sim.advance(FIXED_DT).unwrap();
    }
    sim.liquid_particles(water, &mut p, None);
    let x1 = p.iter().map(|q| q.x).sum::<f32>() / p.len() as f32;
    let paddle_x = -0.3 + 0.4 * 60.0 * FIXED_DT;
    let behind = p
        .iter()
        .filter(|q| q.x < paddle_x - 0.03 && q.y < 0.18)
        .count();
    eprintln!("mean x {x0:.3} -> {x1:.3}, {behind} particles left inside the paddle's path");
    assert!(
        x1 > x0 + 0.03,
        "the paddle did not push the liquid: {x0} -> {x1}"
    );
}

#[test]
fn liquid_surface_field_marks_the_liquid() {
    let bounds = Aabb::new(Vec3::new(-0.3, 0.0, -0.2), Vec3::new(0.3, 0.5, 0.2));
    let mut sim = Simulation::new(SimConfig::new(
        Some(Container::tank(bounds)),
        ParticleSizing::Radius(0.012),
    ));
    let water = sim
        .add_liquid(LiquidOptions {
            fill: LiquidFill::Box(Aabb::new(
                Vec3::new(-0.3, 0.0, -0.2),
                Vec3::new(0.3, 0.15, 0.2),
            )),
            ..Default::default()
        })
        .unwrap();
    for _ in 0..30 {
        sim.advance(FIXED_DT).unwrap();
    }
    let mut field = LiquidSurfaceField::default();
    sim.build_liquid_surface(water, &LiquidSurfaceOptions::default(), &mut field);
    let [nx, ny, nz] = field.resolution;
    assert_eq!(field.data.len(), (nx * ny * nz) as usize);
    let at = |p: Vec3| {
        let g = ((p - field.origin) / field.voxel).floor();
        field.data[(g.x as u32 + nx * (g.y as u32 + ny * g.z as u32)) as usize]
    };
    let inside = at(Vec3::new(0.0, 0.05, 0.0));
    let above = at(Vec3::new(0.0, 0.4, 0.0));
    eprintln!(
        "field {:?} voxel {:.4}: inside {inside:?}, above {above:?}",
        field.resolution, field.voxel
    );
    assert!(
        inside[0] < 0.0,
        "inside the liquid φ must be negative: {inside:?}"
    );
    assert!(
        above[0] >= FIELD_BAND - 1e-3,
        "far above, φ is the band: {above:?}"
    );
    assert!(inside[1] > 0.5, "volume fraction inside: {inside:?}");
    // Some bricks are empty (skippable) and some aren't.
    assert!(field.bricks.iter().any(|&b| b) && field.bricks.iter().any(|&b| !b));
}
