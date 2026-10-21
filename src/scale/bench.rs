//! Performance benchmark harness (automation only).
//!
//! `WB_BENCH="all;-trees;-plants;noshadow"` runs one measuring phase per entry, each
//! `WB_BENCH_SECS` long (default 6) after a 1.5 s settle, starting `WB_BENCH_START` seconds after
//! launch (default 25: time for a world to load and stream in). Every phase logs
//! `BENCH <name> fps … median … p90 … p99 … peak … cpu …` (peak: the longest frame, settling
//! included: a setting change's hitch) and the app exits after the last one.
//! Phase entries:
//! - `all`: everything as is;
//! - `-trees`, `-plants`, `-rocks`, `-debris`, `-structures`, `-objects`: hide those placed objects;
//! - `-billboards`, `-grass`, `-nature` (procedural nature; `-n0`…`-n5` one of its layers, see
//!   `NatureEntity`): hide that layer;
//! - `noshadow`: sun shadows off from this phase on (Bevy can't turn them back on mid-run);
//! - `detail=0.7`: the graphics detail setting from this phase on; `draw=900`: the draw distance
//!   (walk mode's V presets are `draw=900+detail=0.6`, `draw=3000+detail=1`, `draw=7000+detail=1.6`);
//! - `scale=0.75`: the render resolution from this phase on (`render_scale`);
//! - `contact=1` / `contact=0`: contact shadows on / off from this phase on (a pipeline rebuild:
//!   give the phase time, `WB_BENCH_SECS`);
//! - `shadows=low|medium|high`: the sun shadow quality; `filter=hardware|gaussian|temporal`: the
//!   main camera's shadow filtering, from this phase on.
//!
//! Entries combine with `+` (e.g. `-trees+-plants`). Besides wall-clock frame times (which other
//! processes on a busy machine inflate) each phase reports the process CPU time per frame summed
//! over all threads (`cpu`), which they don't, and the entity counts.

use bevy::prelude::*;

use crate::nature::render::NatureEntity;
use crate::objects::{Category, Library, ObjectStore, PlacedObject};

pub struct BenchPlugin;

impl Plugin for BenchPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var("WB_BENCH").is_err() {
            return;
        }
        app.init_resource::<Bench>().add_systems(Last, run_phases);
    }
}

/// Stress-world hooks (automation): `WB_STRESS_AMOUNTS="trees=5,plants=1.5"` sets the world's
/// nature amounts at startup without the UI's 0..2 limit; `WB_NATURE_COUNT=<frame>` logs how many
/// procedural instances the whole world grows (`NATURE COUNT …`) at that frame; `WB_WALK_LOG=1`
/// logs the explorer's position every quarter second (`WALKPOS x z`, collision tests).
pub struct StressPlugin;

impl Plugin for StressPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var("WB_STRESS_AMOUNTS").is_ok() || std::env::var("WB_NATURE_COUNT").is_ok() {
            app.add_systems(Update, stress_hooks);
        }
        if std::env::var("WB_WALK_LOG").is_ok() {
            app.add_systems(Update, walk_log);
        }
    }
}

fn walk_log(walk: Res<crate::walk::WalkState>, time: Res<Time>, mut next: Local<f32>) {
    if walk.active && time.elapsed_secs() >= *next {
        *next = time.elapsed_secs() + 0.25;
        info!("WALKPOS {:.2} {:.2}", walk.position.x, walk.position.z);
    }
}

fn stress_hooks(world: &mut World, mut frame: Local<u32>) {
    *frame += 1;
    if let Ok(spec) = std::env::var("WB_STRESS_AMOUNTS") {
        let mut a = world.resource_mut::<crate::objects::NatureAmounts>();
        for kv in spec.split(',') {
            let Some((k, v)) = kv.split_once('=') else {
                continue;
            };
            let Ok(v) = v.trim().parse::<f32>() else {
                continue;
            };
            let slot = match k.trim() {
                "trees" => &mut a.trees,
                "rocks" => &mut a.rocks,
                "plants" => &mut a.plants,
                "debris" => &mut a.debris,
                _ => continue,
            };
            if *slot != v {
                *slot = v;
            }
        }
    }
    if std::env::var("WB_NATURE_COUNT")
        .ok()
        .and_then(|f| f.parse::<u32>().ok())
        == Some(*frame)
    {
        let t0 = std::time::Instant::now();
        if let Some(n) =
            crate::nature::world_ctx(world).map(|ctx| crate::nature::place::count_world(&ctx))
        {
            info!(
                "NATURE COUNT trees {} rocks {} plants {} debris {} total {} in {:?}",
                n[0],
                n[1],
                n[2],
                n[3],
                n.iter().sum::<usize>(),
                t0.elapsed()
            );
        }
    }
}

#[derive(Resource, Default)]
struct Bench {
    phase: usize,
    phase_start: f32,
    frames: Vec<f32>,
    cpu: Vec<f32>,
    last_cpu: f64,
    /// The longest frame of the phase, settling included (the hitch of a setting change).
    peak: f32,
}

/// CPU time (ms) this process has used so far, over all its threads.
pub fn process_cpu_ms() -> f64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: u32, out: *mut Timespec) -> i32;
    }
    // CLOCK_PROCESS_CPUTIME_ID (macOS 12, Linux 2)
    const CLOCK: u32 = if cfg!(target_os = "macos") { 12 } else { 2 };
    let mut t = Timespec { sec: 0, nsec: 0 };
    // SAFETY: plain libc call writing into a local struct of the matching layout
    if unsafe { clock_gettime(CLOCK, &mut t) } != 0 {
        return 0.0;
    }
    t.sec as f64 * 1000.0 + t.nsec as f64 / 1.0e6
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn run_phases(
    mut b: ResMut<Bench>,
    time: Res<Time<Real>>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    mut objects: Query<(&PlacedObject, &mut Visibility), Without<NatureEntity>>,
    mut named: Query<(&Name, &mut Visibility), (Without<PlacedObject>, Without<NatureEntity>)>,
    mut grass: Query<
        &mut Visibility,
        (
            With<MeshMaterial3d<crate::grass::GrassMaterial>>,
            Without<Name>,
            Without<PlacedObject>,
            Without<NatureEntity>,
        ),
    >,
    mut nature: Query<(&NatureEntity, &mut Visibility), Without<PlacedObject>>,
    near: Res<crate::nature::render::Near>,
    mut graphics: ResMut<crate::streaming::Graphics>,
    meshes: Query<&ViewVisibility, With<Mesh3d>>,
    all: Query<()>,
    mut lights: Query<&mut DirectionalLight>,
    mut exit: MessageWriter<AppExit>,
    (mut render_scale, mut lighting, mut filtering): (
        ResMut<crate::render_scale::RenderScale>,
        ResMut<crate::sky::Lighting>,
        Query<&mut bevy::light::ShadowFilteringMethod, With<crate::camera::MainCamera>>,
    ),
) {
    let Ok(spec) = std::env::var("WB_BENCH") else {
        return;
    };
    let phases: Vec<&str> = spec
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let secs: f32 = std::env::var("WB_BENCH_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6.0);
    let start: f32 = std::env::var("WB_BENCH_START")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25.0);
    let now = time.elapsed_secs();
    let cpu_now = process_cpu_ms();
    let cpu = (cpu_now - b.last_cpu) as f32;
    b.last_cpu = cpu_now;
    if now < start {
        return;
    }
    if b.phase >= phases.len() {
        exit.write(AppExit::Success);
        return;
    }
    if b.phase_start == 0.0 {
        b.phase_start = now;
    }
    let name = phases[b.phase];
    let parts: Vec<&str> = name.split('+').collect();
    let hide = |what: &str| parts.contains(&what);
    // apply the phase's toggles every frame (streaming keeps spawning objects)
    let hidden_cats: Vec<Category> = [
        ("-trees", Category::Trees),
        ("-plants", Category::Plants),
        ("-rocks", Category::Rocks),
        ("-debris", Category::Debris),
        ("-structures", Category::Structures),
    ]
    .iter()
    .filter(|(k, _)| hide(k))
    .map(|(_, c)| *c)
    .collect();
    let all_objects = hide("-objects");
    if let Some(lib) = lib.as_deref() {
        for (o, mut v) in &mut objects {
            let cat = store
                .items
                .get(&o.id)
                .and_then(|p| lib.get(&p.asset))
                .map(|e| e.category);
            let want = if all_objects || cat.is_some_and(|c| hidden_cats.contains(&c)) {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
            if *v != want {
                *v = want;
            }
        }
    }
    if hide("-billboards") {
        for (n, mut v) in &mut named {
            if n.as_str() == "Tree billboards" && *v != Visibility::Hidden {
                *v = Visibility::Hidden;
            }
        }
    }
    for (n, mut v) in &mut nature {
        let want = if hide("-nature") || hide(&format!("-n{}", n.0)) {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *v != want {
            *v = want;
        }
    }
    // `detail=0.7`: the graphics detail for this phase; `draw=900`: the draw distance
    if let Some(d) = parts
        .iter()
        .find_map(|p| p.strip_prefix("detail="))
        .and_then(|d| d.parse::<f32>().ok())
        && graphics.detail != d
    {
        graphics.detail = d;
    }
    if let Some(d) = parts
        .iter()
        .find_map(|p| p.strip_prefix("draw="))
        .and_then(|d| d.parse::<f32>().ok())
        && graphics.draw_distance != d
    {
        graphics.draw_distance = d;
    }
    // `scale=0.75`, `contact=1`, `shadows=medium`, `filter=hardware`: settings from this phase on
    let value = |key: &str| parts.iter().find_map(|p| p.strip_prefix(key));
    if let Some(v) = value("scale=").and_then(|v| v.parse::<f32>().ok())
        && render_scale.force != Some(v)
    {
        render_scale.force = Some(v);
    }
    if let Some(on) = value("contact=").map(|v| v == "1")
        && lighting.contact_shadows != on
    {
        lighting.contact_shadows = on;
    }
    if let Some(q) = value("shadows=") {
        let q = match q {
            "low" => crate::sky::ShadowQuality::Low,
            "medium" => crate::sky::ShadowQuality::Medium,
            _ => crate::sky::ShadowQuality::High,
        };
        if lighting.shadow_quality != q {
            lighting.shadow_quality = q;
        }
    }
    if let Some(f) = value("filter=") {
        use bevy::light::ShadowFilteringMethod as M;
        let m = match f {
            "hardware" => M::Hardware2x2,
            "temporal" => M::Temporal,
            _ => M::Gaussian,
        };
        for mut c in &mut filtering {
            if *c != m {
                *c = m;
            }
        }
    }
    if hide("-grass") {
        for mut v in &mut grass {
            *v = Visibility::Hidden;
        }
    }
    if hide("noshadow") {
        for mut l in &mut lights {
            if l.shadow_maps_enabled {
                l.shadow_maps_enabled = false;
            }
        }
    }
    let t = now - b.phase_start;
    b.peak = b.peak.max(time.delta_secs() * 1000.0);
    if t > 1.5 {
        let dt = time.delta_secs() * 1000.0;
        b.frames.push(dt);
        b.cpu.push(cpu);
    }
    if t >= secs + 1.5 {
        let mut f = std::mem::take(&mut b.frames);
        let mut c = std::mem::take(&mut b.cpu);
        f.sort_by(|a, b| a.total_cmp(b));
        c.sort_by(|a, b| a.total_cmp(b));
        let q = |v: &[f32], p: f32| {
            v.get(((v.len() as f32 * p) as usize).min(v.len().saturating_sub(1)))
                .copied()
                .unwrap_or(0.0)
        };
        let mean = f.iter().sum::<f32>() / f.len().max(1) as f32;
        let visible = meshes.iter().filter(|v| v.get()).count();
        info!(
            "BENCH {name:<22} fps {:6.1}  median {:6.2} ms  p90 {:6.2}  p99 {:6.2}  peak {:6.1}  cpu median {:6.2} ms  entities {}  meshes {}  visible {}  nature {} ({} entities)  frames {}",
            1000.0 / mean.max(0.001),
            q(&f, 0.5),
            q(&f, 0.9),
            q(&f, 0.99),
            std::mem::take(&mut b.peak),
            q(&c, 0.5),
            all.iter().count(),
            meshes.iter().count(),
            visible,
            near.count(),
            near.entities,
            f.len()
        );
        b.phase += 1;
        b.phase_start = now;
    }
}
