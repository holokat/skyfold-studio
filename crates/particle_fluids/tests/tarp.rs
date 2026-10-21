//! Liquid poured onto a sloped, corner-pinned tarp runs off its low edge
//! (the reference `demo/presets/tarp.ts` scene), wetting the canvas.

use particle_fluids::glam::{Quat, Vec3};
use particle_fluids::*;

const WIDTH: f32 = 0.9;
const DEPTH: f32 = 0.72;
const FRONT_HEIGHT: f32 = 0.5;
const SLOPE: f32 = 0.2;

#[test]
fn liquid_runs_off_sloped_tarp_and_wets_it() {
    let r = 0.012;
    let along = DEPTH.hypot(SLOPE);
    let tilt = (SLOPE / DEPTH).atan();
    let bounds = Aabb::new(Vec3::new(-0.9, 0.0, -0.6), Vec3::new(0.9, 1.3, 0.8));
    let mut cfg = SimConfig::new(Some(Container::open(bounds)), ParticleSizing::Radius(r));
    cfg.reserve = 2000;
    cfg.substeps = Some(8);
    cfg.contact_friction = (0.05, 0.03);
    let mut sim = Simulation::new(cfg);
    let mut tarp = ClothOptions {
        width: WIDTH,
        height: along,
        position: Vec3::new(0.0, FRONT_HEIGHT + SLOPE / 2.0, 0.0),
        // Lay flat with the top row at the back, then raise the back edge.
        rotation: Quat::from_rotation_x(tilt - std::f32::consts::FRAC_PI_2),
        // Neighbors overlap so no drop fits through; the tarp skips self-collision.
        spacing: 1.0 / 0.55,
        self_collision: false,
        weight: 10.0,
        stretch_compliance: 1e-8,
        softness: 0.2,
        drag: 0.2,
        lift: 0.02,
        damping: None,
        ..Default::default()
    };
    // Each corner hangs from a 2×2 patch, like a grommet.
    let (cols, rows) = tarp.grid_size(r);
    let corner = |c: u32, rw: u32| rw * (cols + 1) + c;
    tarp.pins = ClothPins::Indices(
        [0, cols - 1]
            .iter()
            .flat_map(|&c| {
                [0, rows - 1].map(|rw| {
                    [
                        corner(c, rw),
                        corner(c + 1, rw),
                        corner(c, rw + 1),
                        corner(c + 1, rw + 1),
                    ]
                })
            })
            .flatten()
            .collect(),
    );
    let cloth = sim.add_cloth(tarp).unwrap();
    let water = sim
        .add_liquid(LiquidOptions {
            params: LiquidParams {
                adhesion: 0.15,
                ..Default::default()
            },
            kill_height: Some(0.25),
            ..Default::default()
        })
        .unwrap();
    sim.start().unwrap();
    let (gc, gr) = sim.cloth_grid(cloth).unwrap();
    assert_eq!((gc, gr), (cols + 1, rows + 1));

    // A nozzle 20% of the way from the back edge, 15 cm above the canvas.
    let back_z = -DEPTH / 2.0;
    let pour = Vec3::new(0.0, FRONT_HEIGHT + SLOPE * 0.8 + 0.15, back_z + 0.2 * DEPTH);
    let spacing = 2.0 * r;
    let mut disc = Vec::new();
    for i in -1i32..=1 {
        for j in -1i32..=1 {
            if i * i + j * j <= 2 {
                disc.push(Vec3::new(i as f32 * spacing, 0.0, j as f32 * spacing));
            }
        }
    }
    let interval = (2.0 * spacing / 9.81f32).sqrt();
    let mut since = f32::INFINITY;
    let mut emitted = 0;
    let mut retired = Vec::new();
    for step in 0..240 {
        since += FIXED_DT;
        if step < 150 && since >= interval {
            since = 0.0;
            for d in &disc {
                if sim
                    .emit_liquid(water, pour + *d, Vec3::new(0.0, -0.5, 0.0))
                    .is_ok()
                {
                    emitted += 1;
                }
            }
        }
        sim.advance(FIXED_DT).unwrap();
        retired.extend_from_slice(sim.retired());
    }
    let mut p = Vec::new();
    sim.liquid_particles(water, &mut p, None);
    let off_front = retired
        .iter()
        .filter(|r| r.position.z > DEPTH / 2.0 - 0.05)
        .count();
    let leaked = retired.iter().filter(|r| r.position.z < 0.0).count();
    eprintln!(
        "emitted {emitted}, on/near tarp {}, retired {} ({off_front} past the front edge, {leaked} behind the middle)",
        p.len(),
        retired.len()
    );
    assert!(emitted > 250);
    // Most of the poured liquid ran down the slope and off the low edge.
    assert!(
        off_front as f32 > 0.4 * emitted as f32,
        "only {off_front} ran off the front"
    );
    // Very little fell through the canvas near the pour point.
    assert!((leaked as f32) < 0.1 * emitted as f32, "{leaked} leaked");
    assert!(
        retired
            .iter()
            .all(|r| r.reason == RetireReason::BelowKillHeight
                || r.reason == RetireReason::LeftContainer)
    );

    // The canvas got wet where the liquid ran, and stayed dry at the far corners.
    let wet = sim.cloth_wetness(cloth);
    let max_wet = wet.iter().copied().fold(0.0f32, f32::max);
    let corner_wet = wet[0].max(wet[cols as usize]);
    eprintln!("max wetness {max_wet:.3}, back corners {corner_wet:.3}");
    assert!(max_wet > 0.3, "max wetness {max_wet}");
    assert!(corner_wet < 0.05, "corner wetness {corner_wet}");
    let mut surface = ClothSurface::new(2, true);
    sim.fill_cloth_surface(cloth, &mut surface);
    assert!(surface.wetness.iter().copied().fold(0.0f32, f32::max) > 0.2);
}
