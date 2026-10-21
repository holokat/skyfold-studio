//! Step-time benchmark.
//!
//! ```text
//! cargo run -p particle_fluids --release --example bench               # every scenario
//! cargo run -p particle_fluids --release --example bench -- liquid 600 # one scenario, 600 steps
//! RAYON_NUM_THREADS=1 cargo run ...                                    # serial cost
//! ```
//!
//! Each line reports wall time and process CPU time per 1/60 s step. With
//! one thread, CPU time is the serial compute cost even on a busy machine.

use particle_fluids::glam::Vec3;
use particle_fluids::*;
use std::time::Instant;

/// Process CPU time (user + system) in seconds, all threads.
#[cfg(unix)]
fn cpu_seconds() -> f64 {
    #[repr(C)]
    struct Timeval {
        sec: i64,
        usec: i64,
    }
    #[repr(C)]
    struct Rusage {
        utime: Timeval,
        stime: Timeval,
        rest: [i64; 14],
    }
    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }
    let mut u = Rusage {
        utime: Timeval { sec: 0, usec: 0 },
        stime: Timeval { sec: 0, usec: 0 },
        rest: [0; 14],
    };
    // SAFETY: getrusage fills the struct (RUSAGE_SELF = 0). `suseconds_t` is
    // 32-bit on macOS, so only the low half of `usec` is meaningful there.
    unsafe { getrusage(0, &mut u) };
    let secs = |t: &Timeval| t.sec as f64 + (t.usec as i32) as f64 * 1e-6;
    secs(&u.utime) + secs(&u.stime)
}

#[cfg(not(unix))]
fn cpu_seconds() -> f64 {
    0.0
}

fn time_steps(name: &str, sim: &mut Simulation, warmup: usize, steps: usize) {
    for _ in 0..warmup {
        sim.advance(FIXED_DT).unwrap();
    }
    let t = Instant::now();
    let c = cpu_seconds();
    for _ in 0..steps {
        sim.advance(FIXED_DT).unwrap();
    }
    let wall = t.elapsed().as_secs_f64() * 1000.0 / steps as f64;
    let cpu = (cpu_seconds() - c) * 1000.0 / steps as f64;
    println!(
        "{name:<34} {:>6} particles {:>2} substeps: {wall:7.3} ms/step wall, {cpu:7.3} ms/step CPU",
        sim.particle_count(),
        sim.substeps()
    );
}

fn liquid(count: usize, substeps: u32) -> (Simulation, LiquidHandle) {
    let bounds = Aabb::new(Vec3::new(-0.5, 0.0, -0.3), Vec3::new(0.5, 0.8, 0.3));
    let mut cfg = SimConfig::new(Some(Container::tank(bounds)), ParticleSizing::Budget(count));
    cfg.substeps = Some(substeps);
    let mut sim = Simulation::new(cfg);
    let water = sim
        .add_liquid(LiquidOptions {
            fill: LiquidFill::Box(Aabb::new(
                Vec3::new(-0.5, 0.0, -0.3),
                Vec3::new(-0.1, 0.5, 0.3),
            )),
            ..Default::default()
        })
        .unwrap();
    sim.start().unwrap();
    (sim, water)
}

/// The reference smoke example: air in a 1 × 1.9 × 1 m box, a 0.17 m source;
/// tracers live 6 s, so `rate` 3333/s keeps 20 000 alive.
fn smoke(air: usize, rate: f32) -> Simulation {
    let bounds = Aabb::new(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.9, 0.5));
    let mut sim = Simulation::new(SimConfig::new(
        Some(Container::closed(bounds)),
        ParticleSizing::Budget(air),
    ));
    sim.add_smoke(SmokeOptions {
        rate,
        radius: 0.17,
        ..Default::default()
    })
    .unwrap();
    sim.start().unwrap();
    sim
}

fn cloth(budget: usize) -> (Simulation, ClothHandle) {
    let mut sim = Simulation::new(SimConfig::new(None, ParticleSizing::Budget(budget)));
    let c = sim
        .add_cloth(ClothOptions {
            width: 1.2,
            height: 1.2,
            wind: Vec3::new(0.0, 0.0, 2.0),
            ..Default::default()
        })
        .unwrap();
    sim.add_sphere(Vec3::new(0.0, 0.6, 0.3), 0.2, ColliderOptions::default());
    sim.start().unwrap();
    (sim, c)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let only = args.next();
    let steps: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(120);
    let run = |name: &str| only.as_deref().is_none_or(|o| o == name);
    println!("rayon threads: {}", rayon::current_num_threads());
    if run("liquid") {
        time_steps("liquid dam break 5k", &mut liquid(5000, 3).0, 60, steps);
    }
    if run("liquid10k") {
        time_steps(
            "liquid dam break 10k",
            &mut liquid(10000, 3).0,
            60,
            steps / 2,
        );
    }
    if run("smoke") {
        let mut s = smoke(5000, 20000.0 / 6.0);
        for _ in 0..360 {
            s.advance(FIXED_DT).unwrap();
        }
        let name = format!("smoke 5k air + {} tracers", s.smoke_tracer_count());
        time_steps(&name, &mut s, 0, steps);
    }
    if run("cloth") {
        time_steps(
            "cloth 1.2 m square + sphere",
            &mut cloth(3000).0,
            30,
            steps / 2,
        );
    }
    if run("render") {
        let mut s = smoke(5000, 20000.0 / 6.0);
        for _ in 0..360 {
            s.advance(FIXED_DT).unwrap();
        }
        let mut vol = SmokeVolume::default();
        let t = Instant::now();
        for _ in 0..10 {
            s.build_smoke_volume(&SmokeVolumeOptions::default(), &mut vol);
        }
        println!(
            "smoke volume {:?} from {} tracers: {:.3} ms",
            vol.resolution,
            s.smoke_tracer_count(),
            t.elapsed().as_secs_f64() * 100.0
        );
        let (mut l, water) = liquid(5000, 3);
        for _ in 0..60 {
            l.advance(FIXED_DT).unwrap();
        }
        let mut field = LiquidSurfaceField::default();
        let t = Instant::now();
        for _ in 0..10 {
            l.build_liquid_surface(water, &LiquidSurfaceOptions::default(), &mut field);
        }
        println!(
            "liquid surface {:?}: {:.3} ms",
            field.resolution,
            t.elapsed().as_secs_f64() * 100.0
        );
        let (mut c, handle) = cloth(3000);
        c.advance(FIXED_DT).unwrap();
        let mut surface = ClothSurface::new(3, true);
        let t = Instant::now();
        for _ in 0..10 {
            c.fill_cloth_surface(handle, &mut surface);
        }
        println!(
            "cloth surface {} vertices: {:.3} ms",
            surface.positions.len(),
            t.elapsed().as_secs_f64() * 100.0
        );
    }
}
