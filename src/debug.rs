//! Automation hooks for headless-ish verification.
//!
//! `WB_SHOT=out.png` saves a screenshot after `WB_SHOT_FRAMES` frames (default 90) and exits.
//! `WB_HOUR`, `WB_TAB`, `WB_CAM=x,z,yaw,pitch,dist` set up the scene first.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::camera::OrbitCamera;
use crate::editor::{Editor, Tab};
use crate::sky::Lighting;

pub struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var("WB_SHOT").is_ok() {
            // automated runs must not react to whatever the user is typing elsewhere
            app.add_systems(
                PreUpdate,
                (block_input, scripted_run)
                    .chain()
                    .after(bevy::input::InputSystems),
            );
        } else {
            app.add_systems(PreUpdate, scripted_run.after(bevy::input::InputSystems));
        }
        app.add_systems(Last, run_log)
            .add_systems(Update, (mcp_calls, audit_shots));
        if std::env::var("WB_OFFSCREEN").is_ok() {
            app.add_systems(Last, offscreen);
        }
        app.add_systems(Startup, apply_env)
            .add_systems(
                Update,
                scripted_stroke
                    .after(crate::tools::ToolSystems::Input)
                    .before(crate::tools::ToolSystems::Apply),
            )
            .add_systems(
                Update,
                (
                    shot,
                    log_fps,
                    perf_toggles,
                    demo_content.after(crate::tools::ToolSystems::Commit),
                    io_test.after(demo_content),
                ),
            )
            .add_systems(
                Update,
                (
                    water_test.after(demo_content),
                    water_seq.after(water_test),
                    preview_test.after(water_test),
                    gameplay_hooks,
                ),
            )
            .add_systems(bevy_egui::EguiPrimaryContextPass, hide_ui);
        if std::env::var_os("WB_PIPELINES").is_some() {
            use bevy::render::{Render, RenderApp, RenderSystems};
            if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
                render_app.add_systems(Render, trace_pipelines.in_set(RenderSystems::Cleanup));
            }
        }
    }
}

/// Records newly queued shader variants independently of Metal's disk cache.
/// This makes a warm-up regression observable even when the driver compiles a
/// previously seen shader quickly in a new app process.
fn trace_pipelines(
    cache: Res<bevy::render::render_resource::PipelineCache>,
    view_keys: Res<bevy::pbr::ViewKeyCache>,
    mut seen: Local<usize>,
    mut seen_views: Local<std::collections::HashSet<bevy::pbr::MeshPipelineKey>>,
) {
    use bevy::material::descriptor::PipelineDescriptor;
    use bevy::render::render_resource::CachedPipelineState;
    use bevy::shader::ShaderDefVal;
    let count = cache.pipelines().count();
    if count < *seen {
        *seen = 0;
        seen_views.clear();
    }
    for key in view_keys.values() {
        if seen_views.insert(*key) {
            info!("new material view key: {key:?}");
        }
    }
    for (id, pipeline) in cache.pipelines().enumerate().skip(*seen) {
        if let PipelineDescriptor::RenderPipelineDescriptor(desc) = &pipeline.descriptor {
            let irradiance = desc.fragment.as_ref().is_some_and(|fragment| {
                fragment.shader_defs.iter().any(|def| {
                    matches!(def,
                    ShaderDefVal::Bool(name, true) if name == "IRRADIANCE_VOLUME")
                })
            });
            let ready = matches!(pipeline.state, CachedPipelineState::Ok(_));
            info!(
                "new render pipeline id={id} label={:?} irradiance={irradiance} ready={ready}",
                desc.label
            );
        }
    }
    *seen = count;
}

/// `WB_OFFSCREEN=1` or `=WxH` (with `WB_SHOT`): the view (3D and panels) is presented into a 1600×1000 (or W×H) image and every
/// screenshot reads that image instead of the window — for test runs while the screen is locked
/// or asleep (the parked window then gets no frames and window screenshots come back black).
#[derive(Resource)]
struct Offscreen(Handle<Image>);

fn offscreen(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    cams: Query<Entity, With<crate::camera::MainCamera>>,
    target: Option<Res<Offscreen>>,
    mut shots: Query<&mut Screenshot, Added<Screenshot>>,
) {
    use bevy::camera::RenderTarget;
    use bevy::render::render_resource::{TextureFormat, TextureUsages};
    let Some(target) = target else {
        if cams.iter().next().is_some() {
            // `WB_OFFSCREEN=WxH` picks the size (default 1600×1000): shots that don't depend on the display
            let (w, h) = std::env::var("WB_OFFSCREEN")
                .ok()
                .and_then(|v| {
                    v.split_once('x')
                        .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))
                })
                .unwrap_or((1600u32, 1000u32));
            let mut image = Image::new_target_texture(
                w,
                h,
                TextureFormat::Rgba8Unorm,
                Some(TextureFormat::Rgba8UnormSrgb),
            );
            image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
            let h = images.add(image);
            // (the render-scale cameras present into it instead of the window)
            commands.insert_resource(crate::render_scale::OutputImage(h.clone()));
            commands.insert_resource(Offscreen(h));
        }
        return;
    };
    for mut shot in &mut shots {
        // Only redirect window captures. Asset preview captures already name their own image.
        if matches!(shot.0, RenderTarget::Window(_)) {
            shot.0 = RenderTarget::Image(target.0.clone().into());
        }
    }
}

static UI_HIDDEN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);

/// `WB_NOUI=1`: shrink the editor UI to nothing (clean renders); `WB_SEQ` steps toggle it.
fn hide_ui(mut contexts: bevy_egui::EguiContexts) {
    use std::sync::atomic::Ordering;
    let mut v = UI_HIDDEN.load(Ordering::Relaxed);
    if v == 2 {
        v = std::env::var("WB_NOUI").is_ok() as u8;
        UI_HIDDEN.store(v, Ordering::Relaxed);
    }
    if let Ok(ctx) = contexts.ctx_mut() {
        let want = if v == 1 { 0.02 } else { 1.0 };
        if (ctx.zoom_factor() - want).abs() > 1e-3 {
            ctx.set_zoom_factor(want);
        }
    }
}

/// `WB_SEQ=name@x,z,yaw,pitch,dist[@key=value…];name2@…` takes several screenshots in one run
/// (into `WB_SEQ_DIR`), each after its settings have had 110 frames to settle. Keys: `ocean` /
/// `beh` (ocean preset / behaviour), `body` / `bbeh` (the selected body's preset / behaviour),
/// `sel` (ocean|lake|fall|river), `hour`, `waves`, `ui` (0|1), `tab`, `tool`.
fn water_seq(
    mut frame: Local<u32>,
    mut commands: Commands,
    mut data: crate::io::WorldData,
    mut exit: MessageWriter<AppExit>,
) {
    use crate::water::{BodyKind, WaterBehaviour, WaterSel};
    let Ok(seq) = std::env::var("WB_SEQ") else {
        return;
    };
    *frame += 1;
    let steps: Vec<&str> = seq.split(';').filter(|x| !x.trim().is_empty()).collect();
    const START: u32 = 150;
    const STEP: u32 = 110;
    let f = *frame;
    if f < START {
        return;
    }
    let k = ((f - START) / STEP) as usize;
    let phase = (f - START) % STEP;
    if k >= steps.len() {
        if phase == 30 && k == steps.len() {
            exit.write(AppExit::Success);
        }
        return;
    }
    let parts: Vec<&str> = steps[k].split('@').collect();
    if phase == 0 {
        if let Some(c) = parts.get(1) {
            let v: Vec<f32> = c.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if v.len() == 5 {
                data.orbit
                    .set_view(Vec3::new(v[0], 0.0, v[1]), v[2], v[3], v[4]);
                data.orbit.snap();
            }
        }
        for kv in parts.iter().skip(2) {
            let Some((key, val)) = kv.split_once('=') else {
                continue;
            };
            let beh = || {
                WaterBehaviour::ALL
                    .into_iter()
                    .find(|x| format!("{x:?}").eq_ignore_ascii_case(val))
            };
            match key {
                "ocean" => *data.water_style = crate::water::preset_style(val),
                "beh" => {
                    if let Some(b) = beh() {
                        data.water_style.set_behaviour(b, BodyKind::Ocean);
                    }
                }
                "waves" => data.water_style.waves = val.parse().unwrap_or(0.6),
                "hour" => data.lighting.hour = val.parse().unwrap_or(10.5),
                "ui" => UI_HIDDEN.store(
                    if val == "0" { 1 } else { 0 },
                    std::sync::atomic::Ordering::Relaxed,
                ),
                "tab" => {
                    if let Some(t) = Tab::ALL
                        .into_iter()
                        .find(|t| t.name().eq_ignore_ascii_case(val))
                    {
                        data.ed.tab = t;
                    }
                }
                "tool" => {
                    use crate::editor::WaterTool;
                    data.ed.water_tool = match val {
                        "drawlake" => WaterTool::DrawLake,
                        "shape" => WaterTool::ShapeLake,
                        "fill" => WaterTool::Lake,
                        "waterfall" => WaterTool::Waterfall,
                        "select" => WaterTool::Select,
                        _ => WaterTool::River,
                    };
                }
                "sel" => {
                    data.water.selected = match val {
                        "lake" => data.water.lakes.last().map(|l| WaterSel::Lake(l.id)),
                        "lake0" => data.water.lakes.first().map(|l| WaterSel::Lake(l.id)),
                        "fall" => data
                            .water
                            .waterfalls
                            .last()
                            .map(|w| WaterSel::Waterfall(w.id)),
                        "river" => data.water.rivers.first().map(|r| WaterSel::River(r.id)),
                        _ => Some(WaterSel::Ocean),
                    }
                }
                "body" => {
                    if let Some(sel) = data.water.selected
                        && let Some(st) = data.water.style_mut(sel)
                    {
                        *st = crate::water::preset_style(val);
                    }
                }
                "bbeh" => {
                    if let (Some(b), Some(sel)) = (beh(), data.water.selected)
                        && let Some(st) = data.water.style_mut(sel)
                    {
                        st.set_behaviour(b, BodyKind::Lake);
                    }
                }
                _ => {}
            }
        }
    }
    if phase == STEP - 1 {
        let dir = std::env::var("WB_SEQ_DIR").unwrap_or_else(|_| ".".into());
        let path = format!("{dir}/{}.png", parts[0]);
        info!("WXLOG seq shot {path}");
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}

/// Water design tests (run once the demo world is built, through the real tool code):
/// `WB_WATER_PRESET=<id>` gives the ocean a preset (`WB_BEHAVIOUR=<name>` a behaviour);
/// `WB_LAKE=x,z,rx,rz[,shape[,water preset]]` (several separated by `;`) draws a lake along a
/// jittery hand-drawn loop; `WB_FALL=x0,z0,x1,z1[,width]` places a waterfall;
/// `WB_WATER_TOOL=drawlake|shape|fill|waterfall|river|select` picks the tool and
/// `WB_SELECT=ocean|lake|fall|river` selects a body for the inspector;
/// `WB_SHAPE=x,z,mode,frames[,metres per frame]` brushes the selected drawn lake's shore
/// (expand|shrink|smooth) and re-carves it.
fn water_test(mut frame: Local<u32>, mut data: crate::io::WorldData, time: Res<Time>) {
    use crate::editor::WaterTool;
    use crate::water::{WaterSel, lakes};
    *frame += 1;
    let f = *frame;
    if f == 2 {
        if let Ok(id) = std::env::var("WB_WATER_PRESET") {
            *data.water_style = crate::water::preset_style(&id);
        }
        if let Ok(b) = std::env::var("WB_BEHAVIOUR")
            && let Some(b) = crate::water::WaterBehaviour::ALL
                .into_iter()
                .find(|x| format!("{x:?}").eq_ignore_ascii_case(&b))
        {
            data.water_style
                .set_behaviour(b, crate::water::BodyKind::Ocean);
        }
        if let Ok(t) = std::env::var("WB_WATER_TOOL") {
            data.ed.water_tool = match t.as_str() {
                "drawlake" => WaterTool::DrawLake,
                "shape" => WaterTool::ShapeLake,
                "fill" => WaterTool::Lake,
                "waterfall" => WaterTool::Waterfall,
                "select" => WaterTool::Select,
                "fish" => WaterTool::Fish,
                _ => WaterTool::River,
            };
        }
    }
    if f == 90 && std::env::var("WB_PROBE").is_ok() {
        // candidate spots for test lakes (flat land) and waterfalls (steep drops)
        let t = &*data.terrain;
        let half = t.half();
        let mut flat = vec![];
        let mut cliffs = vec![];
        let mut z = -half + 100.0;
        while z < half - 100.0 {
            let mut x = -half + 100.0;
            while x < half - 100.0 {
                let h = t.height_at(x, z);
                if h > t.sea_level + 4.0 && h < t.sea_level + 80.0 {
                    let mut var = 0.0f32;
                    for (dx, dz) in [
                        (-50.0, 0.0),
                        (50.0, 0.0),
                        (0.0, -50.0),
                        (0.0, 50.0),
                        (35.0, 35.0),
                        (-35.0, -35.0),
                    ] {
                        var = var.max((t.height_at(x + dx, z + dz) - h).abs());
                    }
                    flat.push((var, x, z, h));
                    for (dx, dz) in [(16.0, 0.0), (-16.0, 0.0), (0.0, 16.0), (0.0, -16.0)] {
                        let d = h - t.height_at(x + dx, z + dz);
                        if d > 8.0 {
                            cliffs.push((d, x, z, x + dx, z + dz, h));
                        }
                    }
                }
                x += 40.0;
            }
            z += 40.0;
        }
        flat.sort_by(|a, b| a.0.total_cmp(&b.0));
        cliffs.sort_by(|a, b| b.0.total_cmp(&a.0));
        for f in flat.iter().take(12) {
            info!(
                "WXLOG flat var {:.1} at {:.0},{:.0} h {:.1}",
                f.0, f.1, f.2, f.3
            );
        }
        for c in cliffs.iter().take(12) {
            info!(
                "WXLOG cliff drop {:.1} from {:.0},{:.0} to {:.0},{:.0} h {:.1}",
                c.0, c.1, c.2, c.3, c.4, c.5
            );
        }
        let r = data.water.rivers.first().map(|r| {
            (
                r.points.first().cloned(),
                r.points.get(r.points.len() / 2).cloned(),
                r.points.last().cloned(),
            )
        });
        info!("WXLOG river {r:?}");
    }
    if f == 90 {
        let lib = data.lib.as_deref();
        if let Ok(spec) = std::env::var("WB_LAKE") {
            for one in spec.split(';') {
                let v: Vec<&str> = one.split(',').map(|x| x.trim()).collect();
                let num = |i: usize| v.get(i).and_then(|x| x.parse::<f32>().ok());
                let (Some(x), Some(z), Some(rx), Some(rz)) = (num(0), num(1), num(2), num(3))
                else {
                    continue;
                };
                let shape_id = v.get(4).copied().unwrap_or("pond");
                let shape = lakes::SHAPES
                    .iter()
                    .find(|s| s.id == shape_id)
                    .unwrap_or(&lakes::SHAPES[0]);
                // a jittery hand-drawn loop
                let mut rng = crate::noise::Rng::new(7);
                let n = ((rx + rz) * 1.2) as usize + 12;
                let raw: Vec<Vec2> = (0..n)
                    .map(|k| {
                        let a = k as f32 / n as f32 * std::f32::consts::TAU;
                        let wob = 1.0 + 0.18 * (a * 2.0 + 1.3).sin() + 0.08 * (a * 5.0).sin();
                        Vec2::new(
                            x + a.cos() * rx * wob + rng.range(-1.2, 1.2),
                            z + a.sin() * rz * wob + rng.range(-1.2, 1.2),
                        )
                    })
                    .collect();
                data.history.begin("Test lake");
                match crate::water::create_drawn_lake(
                    &mut data.terrain,
                    &mut data.history,
                    &mut data.store,
                    lib,
                    &mut data.water,
                    &raw,
                    shape,
                    1.0,
                    1.0,
                ) {
                    Ok(id) => {
                        if let Some(wp) = v.get(5)
                            && let Some(l) = data.water.lakes.iter_mut().find(|l| l.id == id)
                        {
                            l.style = crate::water::preset_style(wp);
                        }
                        let l = data.water.lakes.iter().find(|l| l.id == id).unwrap();
                        info!(
                            "WXLOG lake {id} {} level {:.2} depth {:.1} plants {}",
                            shape.id,
                            l.level,
                            l.depth,
                            l.plant_ids.len()
                        );
                    }
                    Err(e) => info!("WXLOG lake failed: {e}"),
                }
                data.history.commit(&data.store);
            }
        }
        if let Ok(spec) = std::env::var("WB_FALL") {
            let v: Vec<f32> = spec
                .split(',')
                .filter_map(|x| x.trim().parse().ok())
                .collect();
            if v.len() >= 4 {
                let a = Vec3::new(v[0], data.terrain.height_at(v[0], v[1]), v[1]);
                let b = Vec3::new(v[2], data.terrain.height_at(v[2], v[3]), v[3]);
                let mut st = data.ed.water_tools.clone();
                if let Some(w) = v.get(4) {
                    st.fall_width = *w;
                }
                if let Ok(p) = std::env::var("WB_FALL_PRESET") {
                    st.fall_style = crate::water::preset_style(&p);
                }
                data.history.begin("Test waterfall");
                match crate::water::create_waterfall(
                    &mut data.terrain,
                    &mut data.history,
                    &mut data.store,
                    lib,
                    &mut data.water,
                    a,
                    b,
                    &st,
                ) {
                    Ok(id) => {
                        let wf = data.water.waterfalls.iter().find(|w| w.id == id).unwrap();
                        info!(
                            "WXLOG waterfall {id} drop {:.1} pool {:?} top {:?} foot {:?}",
                            wf.drop(),
                            wf.pool,
                            wf.top(),
                            wf.bottom()
                        );
                    }
                    Err(e) => info!("WXLOG waterfall failed: {e}"),
                }
                data.history.commit(&data.store);
            }
        }
        if let Ok(sel) = std::env::var("WB_SELECT") {
            data.water.selected = match sel.as_str() {
                "lake" => data.water.lakes.last().map(|l| WaterSel::Lake(l.id)),
                "fall" => data
                    .water
                    .waterfalls
                    .last()
                    .map(|w| WaterSel::Waterfall(w.id)),
                "river" => data.water.rivers.first().map(|r| WaterSel::River(r.id)),
                "fish" => data.water.fish.last().map(|f| WaterSel::Fish(f.id)),
                _ => Some(WaterSel::Ocean),
            };
        }
        if let Ok(p) = std::env::var("WB_BODY_PRESET")
            && let Some(sel) = data.water.selected
            && let Some(st) = data.water.style_mut(sel)
        {
            *st = crate::water::preset_style(&p);
        }
        if let Ok(b) = std::env::var("WB_BODY_BEHAVIOUR")
            && let Some(b) = crate::water::WaterBehaviour::ALL
                .into_iter()
                .find(|x| format!("{x:?}").eq_ignore_ascii_case(&b))
            && let Some(sel) = data.water.selected
            && let Some(st) = data.water.style_mut(sel)
        {
            st.set_behaviour(b, crate::water::BodyKind::Lake);
        }
    }
    // a scripted shore brush on the selected drawn lake
    if let Ok(spec) = std::env::var("WB_SHAPE") {
        let v: Vec<&str> = spec.split(',').collect();
        let (Some(x), Some(z)) = (
            v.first().and_then(|s| s.parse::<f32>().ok()),
            v.get(1).and_then(|s| s.parse::<f32>().ok()),
        ) else {
            return;
        };
        let mode = match v.get(2).copied().unwrap_or("expand") {
            "shrink" => lakes::ShapeMode::Shrink,
            "smooth" => lakes::ShapeMode::Smooth,
            _ => lakes::ShapeMode::Expand,
        };
        let frames = v.get(3).and_then(|s| s.parse::<u32>().ok()).unwrap_or(60);
        let Some(WaterSel::Lake(id)) = data.water.selected else {
            return;
        };
        if f == 120 {
            data.history.begin("Test shore");
            data.history.touch_water(&data.water);
        }
        if f >= 120 && f < 120 + frames {
            let b = data.ed.water_tools.brush;
            if let Some(l) = data.water.lakes.iter_mut().find(|l| l.id == id) {
                let per_frame = v
                    .get(4)
                    .and_then(|s| s.parse::<f32>().ok())
                    .unwrap_or(b.strength * 7.0 * time.delta_secs().clamp(1.0 / 60.0, 0.05));
                lakes::deform(&mut l.outline, Vec2::new(x, z), b.size, per_frame, mode);
            }
            data.water.dirty = true;
        }
        if f == 120 + frames {
            // the lake as it was when the stroke began (from the history snapshot is private): re-carve
            // against the original outline kept in a static
            if let Some(prev) = SHAPE_PREV.lock().unwrap().take() {
                let lib = data.lib.as_deref().map(|l| &*l);
                crate::water::recarve_lake(
                    &mut data.terrain,
                    &mut data.history,
                    &mut data.store,
                    lib,
                    &mut data.water,
                    id,
                    &prev,
                );
            }
            data.history.commit(&data.store);
        }
        if f == 119
            && let Some(l) = data.water.lakes.iter().find(|l| l.id == id)
        {
            *SHAPE_PREV.lock().unwrap() = Some(l.clone());
        }
    }
}

static SHAPE_PREV: std::sync::Mutex<Option<crate::water::Lake>> = std::sync::Mutex::new(None);

/// Thumbnail and preview tests: `WB_LIB_DUP=n` adds n grouped copies of every Poly Haven model
/// (a big library); `WB_PLACE=<asset id>` picks an asset to place in the Objects tab (with its
/// category); `WB_GROUP=<group>` picks a group chip; `WB_SEARCH=<text>` types into the asset
/// search; `WB_FISH=<species>` sets the Fish tool's species;
/// `WB_FISH_AT=x,z[,count[,radius[,species]]]` (several separated by `;`) releases schools once
/// the demo world is built, before `WB_SELECT` runs (logs `FISHLOG`).
fn preview_test(
    mut frame: Local<u32>,
    mut place: ResMut<crate::objects::PlaceSettings>,
    mut data: crate::io::WorldData,
) {
    *frame += 1;
    if *frame == 1
        && let Some(n) = std::env::var("WB_LIB_DUP")
            .ok()
            .and_then(|n| n.parse::<usize>().ok())
        && let Some(lib) = data.lib.as_mut()
    {
        // a bigger model library: copies of the Poly Haven models, spread over the groups
        const GROUPS: [&str; 11] = [
            "fortifications",
            "ships",
            "containers",
            "lighting",
            "furniture",
            "tools",
            "weapons",
            "decor",
            "village",
            "camp",
            "food",
        ];
        let src: Vec<usize> = (0..lib.entries.len())
            .filter(|&i| lib.entries[i].id.starts_with("ph:"))
            .collect();
        for k in 0..n {
            for (j, &i) in src.iter().enumerate() {
                let e = &lib.entries[i];
                let copy = crate::objects::LibraryEntry {
                    id: format!("{}~{k}", e.id),
                    name: format!("{} {}", e.name, k + 2),
                    category: e.category,
                    group: GROUPS[(j + k) % GROUPS.len()].into(),
                    kind: None,
                    import_path: e.import_path.clone(),
                    surface: e.surface,
                    radius: e.radius,
                    align: e.align,
                    walkable: e.walkable,
                    whole_scene: e.whole_scene,
                    biomes: e.biomes.clone(),
                    sway: false,
                    default_scale: e.default_scale,
                    meshes: vec![],
                    scene: e.scene.clone(),
                    height: e.height,
                    tree: None,
                    textured: false,
                    pack: e.pack.clone(),
                };
                lib.add(copy);
            }
        }
        info!("WB_LIB_DUP: library now {} entries", lib.entries.len());
    }
    if *frame == 2 {
        if let Ok(id) = std::env::var("WB_PLACE") {
            if let Some(e) = data.lib.as_deref().and_then(|l| l.get(&id)) {
                place.category = e.category;
            }
            place.selected = Some(id);
            data.ed.object_tool = crate::editor::ObjectTool::Place;
        }
        if let Ok(q) = std::env::var("WB_SEARCH") {
            data.ed.asset_search = q;
        }
        if let Ok(g) = std::env::var("WB_GROUP") {
            place.group = g;
        }
        if let Some(sp) = std::env::var("WB_FISH")
            .ok()
            .and_then(|f| crate::fish::FishSpecies::parse(&f))
        {
            data.ed.fish_species = sp;
        }
    }
    if *frame == 88
        && let Ok(spec) = std::env::var("WB_FISH_AT")
    {
        for one in spec.split(';') {
            let v: Vec<&str> = one.split(',').map(str::trim).collect();
            let num = |i: usize, d: f32| v.get(i).and_then(|x| x.parse::<f32>().ok()).unwrap_or(d);
            let p = Vec2::new(num(0, 0.0), num(1, 0.0));
            let sp = v
                .get(4)
                .and_then(|x| crate::fish::FishSpecies::parse(x))
                .unwrap_or(data.ed.fish_species);
            let t = &*data.terrain;
            let ground = Vec3::new(p.x, t.height_at(p.x, p.y), p.y);
            let surface = data.water.surface_at(ground, t.sea_level);
            data.water.next_id += 1;
            let id = data.water.next_id;
            let school = crate::fish::FishSchool::new(
                id,
                sp,
                Vec3::new(p.x, surface, p.y),
                num(2, 16.0) as u32,
                num(3, 14.0),
                t,
                &data.water,
            );
            info!(
                "FISHLOG school {id}: {} at {p} ({:.1} m deep, fresh {}) radius {:.1}",
                sp.name(),
                surface - ground.y,
                crate::fish::is_fresh(&data.water, t, p),
                school.radius
            );
            data.water.fish.push(school);
        }
    }
}

fn apply_env(
    mut lighting: ResMut<Lighting>,
    mut orbit: ResMut<OrbitCamera>,
    mut ed: ResMut<Editor>,
    mut ocean: ResMut<crate::water::WaterStyle>,
    mut wind: ResMut<crate::objects::Wind>,
) {
    if let Ok(w) = std::env::var("WB_WIND")
        && let Ok(w) = w.parse::<f32>()
    {
        wind.speed = w;
    }
    if let Ok(h) = std::env::var("WB_HOUR")
        && let Ok(h) = h.parse::<f32>()
    {
        lighting.hour = h;
    }
    if let Ok(t) = std::env::var("WB_TAB") {
        for tab in Tab::ALL {
            if tab.name().eq_ignore_ascii_case(&t) {
                ed.tab = tab;
            }
        }
    }
    if let Ok(c) = std::env::var("WB_CAM") {
        let v: Vec<f32> = c.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if v.len() == 5 {
            orbit.set_view(Vec3::new(v[0], 0.0, v[1]), v[2], v[3], v[4]);
            orbit.snap();
        }
    }
    if let Ok(w) = std::env::var("WB_WAVES")
        && let Ok(w) = w.parse::<f32>()
    {
        ocean.waves = w;
    }
    if std::env::var("WB_OVERLAY").is_ok() {
        ed.surface_overlay = true;
    }
    if let Ok(w) = std::env::var("WB_WEATHER") {
        lighting.weather_override = crate::world::Weather::ALL
            .into_iter()
            .find(|x| format!("{x:?}").eq_ignore_ascii_case(&w));
    }
    if let Ok(m) = std::env::var("WB_MOON")
        && let Ok(m) = m.parse::<f32>()
    {
        lighting.moon_phase = m;
    }
    if let Ok(l) = std::env::var("WB_LOOK") {
        for p in crate::look::LookPreset::ALL {
            if format!("{p:?}").eq_ignore_ascii_case(&l) || p.name().eq_ignore_ascii_case(&l) {
                lighting.look.select(p);
                if let Some((hour, clouds, haze)) = p.scene() {
                    lighting.hour = hour;
                    clouds.apply(&mut lighting, 7);
                    lighting.haze = haze;
                }
            }
        }
    }
    // WB_LOOK_SET="detail=1,sharpen=0": override single look parameters (tuning)
    if let Ok(set) = std::env::var("WB_LOOK_SET") {
        let mut v = serde_json::to_value(lighting.look.base()).unwrap_or_default();
        for kv in set.split(',') {
            if let Some((k, val)) = kv.split_once('=') {
                // numbers, or enum names such as tone=Punchy
                v[k.trim()] = match val.trim().parse::<f64>() {
                    Ok(f) => serde_json::json!(f),
                    Err(_) => serde_json::json!(val.trim()),
                };
            }
        }
        if let Ok(p) = serde_json::from_value(v) {
            lighting.look.custom = Some(p);
        }
    }
}

fn shot(
    mut commands: Commands,
    mut frame: Local<u32>,
    mut exit: MessageWriter<AppExit>,
    mut actions: MessageWriter<crate::ui::UiAction>,
) {
    let Ok(path) = std::env::var("WB_SHOT") else {
        return;
    };
    let frames: u32 = std::env::var("WB_SHOT_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    *frame += 1;
    if *frame == 40 && std::env::var("WB_WALK").is_ok() {
        actions.write(crate::ui::UiAction::ToggleWalk);
    }
    // WB_REGEN=<seed>: generate a new island once the demo world is in place
    if *frame == 300
        && let Ok(seed) = std::env::var("WB_REGEN")
        && let Ok(seed) = seed.parse::<u32>()
    {
        // WB_REGEN_PRESET=continent|archipelago|plains|island (default island)
        let preset = match std::env::var("WB_REGEN_PRESET")
            .unwrap_or_default()
            .as_str()
        {
            "continent" => crate::terrain::worldgen::WorldPreset::Continent,
            "archipelago" => crate::terrain::worldgen::WorldPreset::Archipelago,
            "plains" => crate::terrain::worldgen::WorldPreset::Plains,
            _ => crate::terrain::worldgen::WorldPreset::Island,
        };
        actions.write(crate::ui::UiAction::Regenerate {
            preset,
            seed,
            biomes: Some(crate::biomegen::Climate::Varied),
        });
    }
    if *frame == frames {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    // (an offscreen capture can take longer to read back)
    if *frame
        == frames
            + if std::env::var("WB_OFFSCREEN").is_ok() {
                90
            } else {
                20
            }
    {
        exit.write(AppExit::Success);
    }
}

fn log_fps(
    diag: Res<bevy::diagnostic::DiagnosticsStore>,
    // Virtual time caps long deltas at 250 ms, hiding the multi-second stalls this trace measures.
    time: Res<Time<Real>>,
    mut next: Local<f32>,
    meshes: Query<&ViewVisibility, With<Mesh3d>>,
    all: Query<Entity>,
    mut trace: Local<Vec<f32>>,
    frames: Res<crate::frame_stats::FrameStats>,
) {
    // WB_FRAMES=1: every frame time after the first 10 s, summarised in 4 s windows (spikes, periodicity)
    if std::env::var("WB_FRAMES").is_ok() && time.elapsed_secs() > 10.0 {
        trace.push(time.delta_secs() * 1000.0);
        if trace.len() >= 240 {
            let mut v = trace.clone();
            v.sort_by(|a, b| a.total_cmp(b));
            let spikes: Vec<usize> = trace
                .iter()
                .enumerate()
                .filter(|(_, d)| **d > v[v.len() / 2] * 1.6)
                .map(|(i, _)| i)
                .collect();
            let gaps: Vec<usize> = spikes.windows(2).map(|w| w[1] - w[0]).collect();
            info!(
                "frames ms: min {:.1} median {:.1} p90 {:.1} max {:.1}; {} spikes, gaps {:?}",
                v[0],
                v[v.len() / 2],
                v[v.len() * 9 / 10],
                v[v.len() - 1],
                spikes.len(),
                &gaps[..gaps.len().min(16)]
            );
            trace.clear();
        }
    }
    if std::env::var("WB_FPS").is_err() {
        return;
    }
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 2.0;
    // (Bevy's smoothed FPS is effectively the last frame only: see frame_stats)
    info!(
        "FPS {:.1}  frame median {:.2} ms  worst {:.1} ms  hitches {}",
        frames.fps, frames.median_ms, frames.worst_ms, frames.hitches
    );
    if std::env::var("WB_ENTITIES").is_ok() {
        let visible = meshes.iter().filter(|v| v.get()).count();
        info!(
            "  entities {}  meshes {}  visible meshes {visible}",
            all.iter().count(),
            meshes.iter().count()
        );
    }
    if std::env::var("WB_GPUDIAG").is_ok() {
        let mut v: Vec<(String, f64)> = diag
            .iter()
            .filter(|d| {
                d.path().as_str().contains("gpu") || d.path().as_str().contains("elapsed_cpu")
            })
            .filter_map(|d| d.smoothed().map(|s| (d.path().as_str().to_string(), s)))
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        if v.is_empty() {
            let all: Vec<String> = diag
                .iter()
                .map(|d| d.path().as_str().to_string())
                .take(30)
                .collect();
            info!("  diagnostics: {all:?}");
        }
        for (p, s) in v.iter().take(14) {
            info!("  {s:7.3} ms  {p}");
        }
    }
}

/// WB_PERF=noshadow,noatmos,noclouds,nostars,nosmaa,nobloom,noobjects,nograss,shadowhw to isolate costs.
fn perf_toggles(
    mut done: Local<bool>,
    mut objects: Query<&mut Visibility, With<crate::objects::PlacedObject>>,
    mut commands: Commands,
    mut lights: Query<&mut DirectionalLight>,
    cams: Query<Entity, With<crate::camera::MainCamera>>,
    mut vis: Query<(&Name, &mut Visibility), Without<crate::objects::PlacedObject>>,
) {
    let Ok(p) = std::env::var("WB_PERF") else {
        return;
    };
    // objects keep spawning while the demo world fills in: hide them every frame
    if p.split(',').any(|t| t == "noobjects") {
        for mut v in &mut objects {
            *v = Visibility::Hidden;
        }
    }
    if *done {
        return;
    }
    *done = true;
    for t in p.split(',') {
        match t {
            "noshadow" => {
                for mut l in &mut lights {
                    l.shadow_maps_enabled = false;
                }
            }
            "noatmos" => {
                for e in &cams {
                    commands.entity(e).remove::<bevy::pbr::AtmosphereSettings>();
                }
            }
            "noenv" => {
                for e in &cams {
                    commands
                        .entity(e)
                        .remove::<bevy::light::AtmosphereEnvironmentMapLight>();
                }
            }
            "nosmaa" => {
                for e in &cams {
                    commands.entity(e).remove::<bevy::anti_alias::smaa::Smaa>();
                }
            }
            // single-tap (2×2 hardware) shadow filtering instead of the Gaussian (comparisons)
            "shadowhw" => {
                for e in &cams {
                    commands
                        .entity(e)
                        .insert(bevy::light::ShadowFilteringMethod::Hardware2x2);
                }
            }
            "nobloom" => {
                for e in &cams {
                    commands
                        .entity(e)
                        .remove::<bevy::post_process::bloom::Bloom>();
                }
            }
            "noclouds" | "nostars" | "noocean" => {
                let want = match t {
                    "noclouds" => "Clouds",
                    "nostars" => "Stars",
                    _ => "Ocean",
                };
                for (n, mut v) in &mut vis {
                    if n.as_str() == want {
                        *v = Visibility::Hidden;
                    }
                }
            }
            _ => {}
        }
    }
}

/// GPU pass timings are only collected when profiling (they add timestamp queries).
pub fn gpu_diagnostics() -> impl Plugin {
    struct Gpu;
    impl Plugin for Gpu {
        fn build(&self, app: &mut App) {
            if std::env::var("WB_GPUDIAG").is_ok() {
                app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
            }
        }
    }
    Gpu
}

/// WB_DEMO=1: builds sample content through the real editing code paths (zones, river, lake,
/// auto-populated nature, a bridge, fish) so every feature can be checked visually.
#[allow(clippy::too_many_arguments)]
pub fn demo_content(mut done: Local<bool>, mut data: crate::io::WorldData) {
    if *done || std::env::var("WB_DEMO").is_err() {
        return;
    }
    let Some(lib) = data.lib.as_deref() else {
        return;
    };
    *done = true;
    let steps = std::env::var("WB_DEMO").unwrap_or_default();
    // "1" = everything except zones (worlds start without zones: the crafter adds them);
    // add ",zones" to test zone features
    let want = |s: &str| {
        steps
            .split(',')
            .any(|x| x == s || (x == "1" && s != "zones"))
    };
    use crate::terrain::brush;
    use crate::world::{Biome, Mood, Soundscape, Weather, Zone, ZoneType};
    let t0 = std::time::Instant::now();
    let h = &mut *data.history;
    h.begin("Demo");
    let t = &mut *data.terrain;
    let half = t.half();

    // --- zones: paint along rough polylines
    let mut z1 = Zone::new(1, "Starfall Basin".into());
    z1.level_min = 1;
    z1.level_max = 5;
    z1.description = "The crash site. A sheltered basin where the journey begins.".into();
    z1.soundscape = Soundscape::Meadow;
    let mut z2 = Zone::new(2, "Pinewatch Highlands".into());
    z2.level_min = 10;
    z2.level_max = 15;
    z2.weather_enabled = true;
    z2.weather = Weather::Snow;
    z2.weather_intensity = 0.6;
    z2.biome_enabled = true;
    z2.biome = Biome::Alpine;
    z2.soundscape = Soundscape::Mountain;
    z2.mood_enabled = true;
    z2.mood = Mood::Cool;
    let mut z3 = Zone::new(3, "Saltwind Coast".into());
    z3.level_min = 5;
    z3.level_max = 10;
    z3.zone_type = ZoneType::Contested;
    z3.soundscape = Soundscape::Coast;
    z3.weather_enabled = true;
    z3.weather = Weather::Cloudy;
    let paint = |t: &mut crate::terrain::Terrain,
                 h: &mut crate::history::History,
                 pts: &[(f32, f32)],
                 r: f32,
                 id: u8| {
        for w in pts.windows(2) {
            let a = Vec2::new(w[0].0, w[0].1) * half;
            let b = Vec2::new(w[1].0, w[1].1) * half;
            let n = (a.distance(b) / (r * 0.4)).ceil().max(1.0) as usize;
            for k in 0..=n {
                brush::paint_zone(t, h, a.lerp(b, k as f32 / n as f32), r, id, None);
            }
        }
    };
    if want("zones") {
        paint(t, h, &[(-0.35, 0.15), (-0.15, 0.3), (0.05, 0.25)], 260.0, 1);
        paint(t, h, &[(-0.1, -0.35), (0.15, -0.2), (0.3, -0.05)], 300.0, 2);
        paint(t, h, &[(0.25, 0.45), (0.45, 0.25), (0.55, 0.0)], 200.0, 3);
        data.zones.list = vec![z1, z2, z3];
        data.zones.selected = Some(0);
    }

    // --- river from the highlands down to the sea (find the highest point near a start)
    let mut best = (f32::MIN, Vec2::ZERO);
    for j in 0..40 {
        for i in 0..40 {
            let p = Vec2::new(-0.1 + i as f32 * 0.006, -0.3 + j as f32 * 0.006) * half;
            let hh = t.height_at(p.x, p.y);
            if hh > best.0 && hh < t.style.snow_line * 0.9 {
                best = (hh, p);
            }
        }
    }
    let start = best.1;
    // walk downhill to build the path
    let mut path = vec![Vec3::new(start.x, t.height_at(start.x, start.y), start.y)];
    let mut p = start;
    for _ in 0..400 {
        let n = t.normal_at(p.x, p.y);
        let dir = Vec2::new(n.x, n.z).normalize_or(Vec2::new(0.3, 1.0));
        p += (dir * 0.7 + Vec2::new(0.15, 0.6)).normalize() * 12.0;
        let hh = t.height_at(p.x, p.y);
        path.push(Vec3::new(p.x, hh, p.y));
        if hh < t.sea_level - 2.0 || !t.in_bounds(p.x, p.y) {
            break;
        }
    }
    data.water.next_id += 1;
    let rid = data.water.next_id;
    if !want("river") {
        path.clear();
    }
    // (no placed bridge: the procedural low-poly one is hidden; roads build real bridges)
    if let Some(r) = crate::water::make_river(t, h, &path, 22.0, 3.5, rid) {
        data.water.rivers.push(r);
    }
    data.water.dirty = true;

    // --- nature everywhere (procedural: it grows wherever the land suits it)
    if want("nature") {
        data.nature.fallback = true;
        data.nature
            .touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    }
    // WB_SHOWCASE=1: a row of every new plant type east of the camp (for screenshots)
    if std::env::var("WB_SHOWCASE").is_ok() {
        let c = Vec2::new(-0.2, 0.22) * half + Vec2::new(0.0, 30.0);
        let row = [
            ("Palm", 0),
            ("Palm", 1),
            ("Palm", 2),
            ("Cactus", 0),
            ("Cactus", 1),
            ("Cactus", 2),
            ("DesertShrub", 0),
            ("DesertShrub", 1),
            ("Flowers", 0),
            ("Flowers", 1),
            ("TallGrass", 0),
            ("TallGrass", 1),
            ("Reeds", 0),
            ("Reeds", 1),
        ];
        for (k, (asset, variant)) in row.iter().enumerate() {
            let q = c + Vec2::new(k as f32 * 4.5 - 30.0, 0.0);
            let y = t.height_at(q.x, q.y);
            let id = data.store.new_id();
            data.store.insert(crate::objects::Placed {
                group: 0,
                id,
                asset: asset.to_string(),
                variant: *variant,
                pos: [q.x, y, q.y],
                rot: Quat::IDENTITY.to_array(),
                scale: 1.0,
                stretch: None,
                locked: false,
                glow: None,
            });
        }
    }
    // WB_PROPS=1: every variant of the photoscanned props, one row per kind south-east of the camp
    // (centred on camp x + 10; the log lists each row's z), sunk like the scatter brush sinks them
    if std::env::var("WB_PROPS").is_ok() {
        let c = Vec2::new(-0.2, 0.22) * half + Vec2::new(10.0, -12.0);
        let mut z = 0.0;
        for asset in [
            "Flowers",
            "TallGrass",
            "Fern",
            "Pebbles",
            "Branches",
            "Stump",
            "Driftwood",
            "MossyRock",
            "Boulder",
            "RockCluster",
            "FallenLog",
            "Roots",
            "RockSpire",
            "CliffRock",
        ] {
            let Some(e) = lib.get(asset) else { continue };
            let step = e.radius * 2.0;
            z -= step;
            let n = e.meshes.len();
            info!("WB_PROPS: {asset} row at x {:.1}, z {:.1}", c.x, c.y + z);
            for v in 0..n {
                let q = c + Vec2::new((v as f32 - (n - 1) as f32 * 0.5) * step, z);
                let n = t.normal_at(q.x, q.y);
                let rot =
                    Quat::from_rotation_arc(Vec3::Y, n) * Quat::from_rotation_y(v as f32 * 1.3);
                let id = data.store.new_id();
                data.store.insert(crate::objects::Placed {
                    group: 0,
                    id,
                    asset: asset.into(),
                    variant: v as u32,
                    pos: [q.x, t.height_at(q.x, q.y) - 0.15, q.y],
                    rot: rot.to_array(),
                    scale: 1.0,
                    stretch: None,
                    locked: false,
                    glow: None,
                });
            }
            z -= step;
        }
    }
    // WB_FORMATIONS=1: tall rock formations for screenshots: sea stacks in the shallows off the
    // south coast (1.5–5 m deep) and a row of every hoodoo and rock spire variant west of the camp
    // (the log lists where, for WB_CAM)
    if std::env::var("WB_FORMATIONS").is_ok() {
        let mut stacks: Vec<Vec2> = vec![];
        for a in 0..60 {
            let ang = std::f32::consts::FRAC_PI_2 + (a as f32 - 30.0) * 0.012;
            let dir = Vec2::new(ang.cos(), ang.sin());
            let mut r = 200.0;
            while r < half {
                let q = dir * r;
                let g = t.height_at(q.x, q.y);
                if g < t.sea_level - 1.5 {
                    if g > t.sea_level - 5.0
                        && stacks.len() < 4
                        && stacks.iter().all(|s| s.distance(q) > 50.0)
                    {
                        stacks.push(q);
                    }
                    break;
                }
                r += 4.0;
            }
        }
        for (k, q) in stacks.iter().enumerate() {
            let g = t.height_at(q.x, q.y);
            info!(
                "WB_FORMATIONS: sea stack {k} at x {:.1}, z {:.1}, {:.1} m deep",
                q.x,
                q.y,
                t.sea_level - g
            );
            let id = data.store.new_id();
            let rot = Quat::from_rotation_y(k as f32 * 2.1);
            data.store.insert(crate::objects::Placed {
                group: 0,
                id,
                asset: "SeaStack".into(),
                variant: k as u32,
                pos: [q.x, g - 0.2, q.y],
                rot: rot.to_array(),
                scale: 0.8 + 0.15 * k as f32,
                stretch: None,
                locked: false,
                glow: None,
            });
        }
        let c = Vec2::new(-0.2, 0.22) * half + Vec2::new(-60.0, 0.0);
        for (row, asset) in ["Hoodoo", "RockSpire"].into_iter().enumerate() {
            let Some(e) = lib.get(asset) else { continue };
            let n = e.meshes.len();
            let step = e.radius * 2.5;
            let z = c.y + row as f32 * 45.0;
            info!("WB_FORMATIONS: {asset} row at x {:.1}, z {:.1}", c.x, z);
            for v in 0..n {
                let q = Vec2::new(c.x + (v as f32 - (n - 1) as f32 * 0.5) * step, z);
                let id = data.store.new_id();
                let rot = Quat::from_rotation_y(v as f32 * 1.3);
                data.store.insert(crate::objects::Placed {
                    group: 0,
                    id,
                    asset: asset.into(),
                    variant: v as u32,
                    pos: [q.x, t.height_at(q.x, q.y) - 0.15, q.y],
                    rot: rot.to_array(),
                    scale: 1.0,
                    stretch: None,
                    locked: false,
                    glow: None,
                });
            }
        }
    }
    // a camp in the starting zone
    let camp = Vec2::new(-0.2, 0.22) * half;
    for (k, dx, dz, s) in [
        ("Campfire", 0.0, 0.0, 1.0),
        ("Tent", 6.0, 2.0, 1.2),
        ("Crate", -4.0, 3.0, 1.0),
        ("Barrel", -5.0, 1.5, 1.0),
        ("Signpost", 3.0, -6.0, 1.0),
        ("Banner", -2.5, -4.0, 1.0),
        ("TarpShelter", -11.0, -6.0, 1.0),
    ] {
        let q = camp + Vec2::new(dx, dz);
        let id = data.store.new_id();
        data.store.insert(crate::objects::Placed {
            group: 0,
            id,
            asset: k.into(),
            variant: 0,
            pos: [q.x, t.height_at(q.x, q.y) - 0.05, q.y],
            rot: Quat::from_rotation_y(dx * 0.3).to_array(),
            scale: s,
            stretch: None,
            locked: false,
            glow: None,
        });
    }
    // --- fish in the sea off the south coast
    // walk outwards from the centre towards the south until the water is 3-8 m deep
    let mut sea_spot = None;
    'search: for a in 0..24 {
        let ang = std::f32::consts::FRAC_PI_2 + (a as f32 - 12.0) * 0.06;
        let dir = Vec2::new(ang.cos(), ang.sin());
        let mut r = 200.0;
        while r < half {
            let q = dir * r;
            let g = t.height_at(q.x, q.y);
            if g < t.sea_level - 3.0 && g > t.sea_level - 8.0 {
                sea_spot = Some(Vec3::new(q.x, t.sea_level, q.y));
                break 'search;
            }
            r += 8.0;
        }
    }
    if let Some(c) = sea_spot {
        info!("demo: fish at {c:?}");
    }
    if let Some(c) = sea_spot {
        data.water.next_id += 1;
        let id = data.water.next_id;
        let school = crate::fish::FishSchool::new(
            id,
            crate::fish::FishSpecies::ReefFish,
            c,
            30,
            25.0,
            t,
            &data.water,
        );
        data.water.fish.push(school);
    }
    data.history.commit(&data.store);
    data.meta.name = "Demo World".into();
    info!("demo content built in {:?}", t0.elapsed());
}

/// `WB_STROKE=x0,z0,x1,z1[,start_frame[,frames]]` drags a scripted brush stroke in a straight
/// line (for testing brush tools; set the tool with `WB_TAB`, `WB_BIOME=<preset>` [+ `WB_BIOME_OUTLINE=1`]).
/// With `WB_BIOME_OUTLINE`, the four values are a centre and radii and the stroke circles it.
/// `WB_BIOME_ZONE=<zone id>` applies `WB_BIOME` to a whole zone instead.
fn scripted_stroke(
    mut frame: Local<u32>,
    mut prev: Local<Option<Vec3>>,
    mut stroke: ResMut<crate::tools::Stroke>,
    mut ed: ResMut<Editor>,
    mut history: ResMut<crate::history::History>,
    terrain: Res<crate::terrain::Terrain>,
    mut apply: MessageWriter<crate::biomes::ApplyBiomeToZone>,
) {
    *frame += 1;
    let biome = std::env::var("WB_BIOME").ok().and_then(|b| {
        crate::biomes::BiomePreset::ALL
            .into_iter()
            .find(|p| format!("{p:?}").eq_ignore_ascii_case(&b))
    });
    if *frame == 2
        && let Some(p) = biome
    {
        ed.tab = Tab::Zones;
        ed.zone_tool = crate::editor::ZoneTool::Biome;
        ed.biome_preset = p;
        ed.biome_outline = std::env::var("WB_BIOME_OUTLINE").is_ok();
    }
    if *frame == 420
        && let Ok(t) = std::env::var("WB_TAB_AFTER")
        && let Some(tab) = Tab::ALL
            .into_iter()
            .find(|tab| tab.name().eq_ignore_ascii_case(&t))
    {
        ed.tab = tab;
    }
    if let (Some(p), Ok(z)) = (biome, std::env::var("WB_BIOME_ZONE")) {
        if *frame == 320
            && let Ok(zone) = z.parse::<u8>()
        {
            apply.write(crate::biomes::ApplyBiomeToZone { zone, preset: p });
        }
        return;
    }
    let Ok(spec) = std::env::var("WB_STROKE") else {
        return;
    };
    let v: Vec<f32> = spec
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if v.len() < 4 {
        return;
    }
    let start = v.get(4).copied().unwrap_or(320.0) as u32;
    let frames = v.get(5).copied().unwrap_or(40.0) as u32;
    if *frame < start || *frame > start + frames {
        return;
    }
    let t = (*frame - start) as f32 / frames as f32;
    let xz = if ed.biome_outline {
        let a = t * std::f32::consts::TAU * 1.02;
        Vec2::new(v[0] + v[2] * a.cos(), v[1] + v[3] * a.sin())
    } else {
        Vec2::new(v[0], v[1]).lerp(Vec2::new(v[2], v[3]), t)
    };
    let hit = Vec3::new(xz.x, terrain.height_at(xz.x, xz.y), xz.y);
    // update_stroke (no mouse) ends strokes and clears the hit every frame: drive it here instead
    stroke.last_hit = prev.or(Some(hit));
    stroke.hit = Some(hit);
    *prev = Some(hit);
    stroke.just_started = *frame == start;
    stroke.active = *frame < start + frames;
    stroke.just_ended = *frame == start + frames;
    if *frame == start {
        stroke.start_hit = Some(hit);
        stroke.path.clear();
        stroke.bounds = None;
        stroke.tab = ed.tab;
        history.begin("Scripted stroke");
    }
    if stroke.path.last().is_none_or(|l| l.distance(hit) > 2.0) {
        stroke.path.push(hit);
    }
}

/// `WB_RUN=jog|sprint|slow` (with `WB_WALK`): hold forward (and the gait key) once walking, to
/// test movement; `WB_RUN_LOG=1` prints the camera and feet height every frame.
fn scripted_run(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    walk: Res<crate::walk::WalkState>,
    mut frames: Local<u32>,
) {
    let Ok(gait) = std::env::var("WB_RUN") else {
        return;
    };
    if !walk.active {
        *frames = 0;
        return;
    }
    *frames += 1;
    if *frames > 60 {
        keys.press(KeyCode::KeyW);
        match gait.as_str() {
            "sprint" => keys.press(KeyCode::ShiftLeft),
            "slow" => keys.press(KeyCode::AltLeft),
            _ => {}
        }
    }
}

fn run_log(
    walk: Res<crate::walk::WalkState>,
    cam: Query<&Transform, With<crate::camera::MainCamera>>,
    time: Res<Time>,
) {
    if !walk.active || std::env::var("WB_RUN_LOG").is_err() {
        return;
    }
    if let Ok(t) = cam.single() {
        info!(
            "RUNLOG {:.5} {:.4} {:.4} {:.3}",
            time.delta_secs(),
            t.translation.y,
            walk.position.y,
            Vec2::new(walk.velocity.x, walk.velocity.z).length()
        );
    }
}

/// `WB_AUDIT=id,id…` (with `WB_AUDIT_AT=x,z`, `WB_AUDIT_DIR`): asset review shots in the real
/// renderer. Each library asset in turn is placed alone at the spot (fitted to the ground like the
/// editor does) and photographed twice — a three-quarter view (`<name>_a.png`) and a close
/// front view (`<name>_b.png`) — then removed; the app exits after the last one.
fn audit_shots(world: &mut World) {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    static FRAME: AtomicU32 = AtomicU32::new(0);
    static PLACED: AtomicU64 = AtomicU64::new(0);
    let Ok(spec) = std::env::var("WB_AUDIT") else {
        return;
    };
    let ids: Vec<&str> = spec
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let at: Vec<f32> = std::env::var("WB_AUDIT_AT")
        .unwrap_or_else(|_| "-300,470".into())
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let (x, z) = (
        at.first().copied().unwrap_or(-300.0),
        at.get(1).copied().unwrap_or(470.0),
    );
    const START: u32 = 240;
    const STEP: u32 = 120;
    let f = FRAME.fetch_add(1, Ordering::Relaxed);
    if f < START || world.get_resource::<crate::objects::Library>().is_none() {
        return;
    }
    let k = ((f - START) / STEP) as usize;
    let phase = (f - START) % STEP;
    let (i, shot) = (k / 2, k % 2);
    if i >= ids.len() {
        // (the last screenshot is read back over a few frames, offscreen ones especially)
        if phase == 90 {
            world.write_message(AppExit::Success);
        }
        return;
    }
    let id = ids[i];
    if shot == 0 && phase == 0 {
        let prev = PLACED.swap(0, Ordering::Relaxed);
        if prev != 0 {
            let _ = crate::mcp::call_tool(
                world,
                "objects_remove",
                serde_json::json!({ "ids": [prev] }),
            );
        }
        match crate::mcp::call_tool(
            world,
            "objects_place",
            serde_json::json!({ "items": [{ "asset": id, "position": [x, z] }] }),
        ) {
            Ok(out) => {
                for c in out {
                    if let crate::mcp_tools::McpContent::Json(v) = c
                        && let Some(n) = v["ids"][0].as_u64()
                    {
                        PLACED.store(n, Ordering::Relaxed);
                    }
                }
            }
            Err(e) => warn!("AUDIT {id}: {e}"),
        }
    }
    if phase == 0 {
        let (r, h) = world
            .resource::<crate::objects::Library>()
            .get(id)
            .map(|e| (e.radius.max(0.5), e.height.max(1.0)))
            .unwrap_or((5.0, 5.0));
        let size = r.max(h * 0.6);
        let (yaw, pitch, front) = if shot == 0 {
            (0.62f32, 0.26f32, size * 2.9 + 4.0)
        } else {
            (0.12, 0.10, size * 1.7 + 2.0)
        };
        // the orbit focus sits on the ground, so aim at a point behind the model: the view ray
        // then passes through the model's middle (h/2 up) on its way down
        let back = (h * if shot == 0 { 0.4 } else { 0.28 }) / pitch.tan();
        let dir = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let mut o = world.resource_mut::<OrbitCamera>();
        o.set_view(
            Vec3::new(x, 0.0, z) - dir * back,
            yaw,
            pitch,
            (back + front) / pitch.cos(),
        );
        o.snap();
    }
    if phase == STEP - 1 {
        let dir = std::env::var("WB_AUDIT_DIR").unwrap_or_else(|_| ".".into());
        let name = id.split(':').next_back().unwrap_or(id);
        let path = format!("{dir}/{name}_{}.png", if shot == 0 { "a" } else { "b" });
        world
            .commands()
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        info!("AUDIT shot {id} {}", if shot == 0 { "a" } else { "b" });
    }
}

/// `WB_MCP_CALL=tool {json args};tool2 {json}` calls registered MCP tools once the demo world
/// is built (`WB_MCP_FRAME`, default 360; one call per frame) and logs `MCPCALL <tool> -> …`.
/// `sleep N` between calls waits N frames.
fn mcp_calls(world: &mut World) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static FRAME: AtomicU32 = AtomicU32::new(0);
    let Ok(spec) = std::env::var("WB_MCP_CALL") else {
        return;
    };
    let f = FRAME.fetch_add(1, Ordering::Relaxed);
    let start: u32 = std::env::var("WB_MCP_FRAME")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(360);
    let calls: Vec<&str> = spec
        .split(';')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .flat_map(|c| match c.strip_prefix("sleep ") {
            Some(n) => vec!["sleep"; n.trim().parse().unwrap_or(0)],
            None => vec![c],
        })
        .collect();
    let Some(call) = f.checked_sub(start).and_then(|i| calls.get(i as usize)) else {
        return;
    };
    if *call == "sleep" {
        return;
    }
    let (name, json) = call.split_once(' ').unwrap_or((call, "{}"));
    let args: serde_json::Value = serde_json::from_str(json).unwrap_or_else(|e| {
        warn!("MCPCALL {name}: bad json ({e})");
        serde_json::Value::Object(Default::default())
    });
    match crate::mcp::call_tool(world, name, args) {
        Ok(out) => {
            for c in out {
                match c {
                    crate::mcp_tools::McpContent::Text(t) => info!("MCPCALL {name} -> {t}"),
                    crate::mcp_tools::McpContent::Json(v) => info!("MCPCALL {name} -> {v}"),
                    crate::mcp_tools::McpContent::Png(b) => {
                        info!("MCPCALL {name} -> png {} bytes", b.len())
                    }
                    crate::mcp_tools::McpContent::Deferred(d) => {
                        crate::mcp::log_deferred(world, name, d)
                    }
                }
            }
        }
        Err(e) => warn!("MCPCALL {name} failed: {e}"),
    }
}

fn block_input(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut motion: ResMut<bevy::input::mouse::AccumulatedMouseMotion>,
    mut scroll: ResMut<bevy::input::mouse::AccumulatedMouseScroll>,
) {
    keys.reset_all();
    mouse.reset_all();
    motion.delta = Vec2::ZERO;
    scroll.delta = Vec2::ZERO;
}

/// WB_IOTEST=<dir>: after the demo is built, save it, wipe the world, load it back, compare,
/// and export it — logs PASS/FAIL lines.
pub fn io_test(mut frame: Local<u32>, mut data: crate::io::WorldData) {
    let Ok(dir) = std::env::var("WB_IOTEST") else {
        return;
    };
    *frame += 1;
    if *frame != 60 {
        return;
    }
    let dir = std::path::PathBuf::from(dir);
    let before = (
        data.store.count(),
        data.zones.list.len(),
        data.water.rivers.len(),
        data.water.fish.len(),
        data.terrain.heights.iter().map(|h| *h as f64).sum::<f64>(),
    );
    let path = dir.join("test.world");
    match crate::io::save_world(&path, &data) {
        Ok(()) => info!("IOTEST save PASS {}", path.display()),
        Err(e) => return error!("IOTEST save FAIL {e}"),
    }
    crate::io::new_world(
        &mut data,
        crate::terrain::worldgen::WorldPreset::Ocean,
        256,
        4.0,
        1,
    );
    match crate::io::load_world(&path, &mut data) {
        Ok(()) => info!("IOTEST load PASS"),
        Err(e) => return error!("IOTEST load FAIL {e}"),
    }
    let after = (
        data.store.count(),
        data.zones.list.len(),
        data.water.rivers.len(),
        data.water.fish.len(),
        data.terrain.heights.iter().map(|h| *h as f64).sum::<f64>(),
    );
    if before.0 == after.0
        && before.1 == after.1
        && before.2 == after.2
        && before.3 == after.3
        && (before.4 - after.4).abs() < 1e-3
    {
        info!(
            "IOTEST roundtrip PASS objects={} zones={} rivers={} fish={}",
            after.0, after.1, after.2, after.3
        );
    } else {
        error!("IOTEST roundtrip FAIL before={before:?} after={after:?}");
    }
    match crate::io::export_world(&dir.join("export"), &data) {
        Ok(()) => info!("IOTEST export PASS"),
        Err(e) => error!("IOTEST export FAIL {e}"),
    }
}

/// `WB_GP_SELECT=poi|spawn|npc[@frame]` selects the first such gameplay item (default frame 500);
/// `WB_GP_TOOL=spawn|select|…` picks the Gameplay tool.
fn gameplay_hooks(
    mut frame: Local<u32>,
    mut gp: ResMut<crate::gameplay::Gameplay>,
    mut ed: ResMut<crate::gameplay::GameplayEditor>,
) {
    use crate::gameplay::{GameplaySel, GameplayTool};
    *frame += 1;
    if let Ok(t) = std::env::var("WB_GP_TOOL")
        && *frame == 5
    {
        ed.tool = match t.as_str() {
            "spawn" => GameplayTool::Spawn,
            "poi" => GameplayTool::Poi,
            "npc" => GameplayTool::Npc,
            "node" => GameplayTool::Node,
            "ore" => GameplayTool::Ore,
            "scatter" => GameplayTool::Scatter,
            _ => GameplayTool::Select,
        };
    }
    let Ok(v) = std::env::var("WB_GP_SELECT") else {
        return;
    };
    let (what, at) = v
        .split_once('@')
        .map(|(a, b)| (a.to_string(), b.parse().unwrap_or(500)))
        .unwrap_or((v.clone(), 500));
    if *frame != at {
        return;
    }
    gp.selected = match what.as_str() {
        "poi" => gp.pois.first().map(|q| GameplaySel::Poi(q.id)),
        "spawn" => gp.spawns.first().map(|r| GameplaySel::Spawn(r.id)),
        "npc" => gp.npcs.first().map(|n| GameplaySel::Npc(n.id)),
        "area" => gp.ore_areas.last().map(|a| GameplaySel::OreArea(a.id)),
        _ => None,
    };
}
