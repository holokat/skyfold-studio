//! Soft-body cube dropped on a floor: comes to rest with its volume preserved.

use particle_fluids::glam::Vec3;
use particle_fluids::*;

fn bbox(points: &[Vec3]) -> (Vec3, Vec3) {
    points.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), &p| (lo.min(p), hi.max(p)),
    )
}

fn gyration(points: &[Vec3]) -> f32 {
    let c = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    (points
        .iter()
        .map(|p| (*p - c).length_squared())
        .sum::<f32>()
        / points.len() as f32)
        .sqrt()
}

fn drop_cube(mode: ShapeMatching) {
    let bounds = Aabb::new(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
    let mut sim = Simulation::new(SimConfig::new(
        Some(Container::tank(bounds)),
        ParticleSizing::Radius(0.0167),
    ));
    let mesh = TriangleMesh::cuboid(Vec3::new(0.0, 0.4, 0.0), Vec3::splat(0.1));
    let body = sim
        .add_softbody(SoftbodyOptions {
            shape_matching: mode,
            ..SoftbodyOptions::new(mesh.clone())
        })
        .unwrap();
    sim.start().unwrap();
    let n = sim.softbody_count(body);
    assert_eq!(n, 216);
    let mut p = Vec::new();
    sim.softbody_particles(body, &mut p);
    let (lo0, hi0) = bbox(&p);
    let rest_volume = (hi0 - lo0).x * (hi0 - lo0).y * (hi0 - lo0).z;
    let rest_gyration = gyration(&p);
    let binding = sim.bind_softbody_mesh(body, &mesh.vertices, &[]).unwrap();
    let mut speeds = Vec::new();
    for step in 0..240 {
        sim.advance(FIXED_DT).unwrap();
        if step >= 200 {
            let mut v = Vec::new();
            sim.for_each_particle(|_, _, _, vel| v.push(vel.length()));
            speeds.push(v.iter().sum::<f32>() / v.len() as f32);
        }
    }
    sim.softbody_particles(body, &mut p);
    assert!(p.iter().all(|x| x.is_finite()));
    let (lo, hi) = bbox(&p);
    let volume = (hi - lo).x * (hi - lo).y * (hi - lo).z;
    let g = gyration(&p);
    let mean_speed = speeds.iter().sum::<f32>() / speeds.len() as f32;
    eprintln!(
        "{mode:?}: rests at y {:.4}..{:.4}, bbox volume {:.5} (rest {rest_volume:.5}), gyration {g:.4} (rest {rest_gyration:.4}), speed {mean_speed:.4}",
        lo.y, hi.y, volume
    );
    // Rests on the floor (particle centres one radius up), not through it.
    assert!(lo.y > 0.0 && lo.y < 0.03, "bottom at {}", lo.y);
    // Came to rest.
    assert!(mean_speed < 0.02, "mean speed {mean_speed}");
    // Volume preserved within tolerance.
    assert!(
        (volume / rest_volume - 1.0).abs() < 0.15,
        "volume {volume} vs {rest_volume}"
    );
    assert!(
        (g / rest_gyration - 1.0).abs() < 0.08,
        "gyration {g} vs {rest_gyration}"
    );
    // The skinned mesh follows the particles.
    let (mut pos, mut nrm) = (Vec::new(), Vec::new());
    sim.skin_softbody(&binding, &mut pos, &mut nrm);
    let (slo, shi) = bbox(&pos);
    assert!(
        slo.y > -0.02 && (shi.y - slo.y - 0.2).abs() < 0.04,
        "skin {slo} {shi}"
    );
}

#[test]
fn local_softbody_cube_rests_and_keeps_volume() {
    drop_cube(ShapeMatching::Local);
}

#[test]
fn global_softbody_cube_rests_and_keeps_volume() {
    drop_cube(ShapeMatching::Global);
}
