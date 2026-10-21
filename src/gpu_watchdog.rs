//! GPU resets. When a frame runs too long or a shader faults, macOS stops the GPU and resets it
//! ("Caused GPU Hang Error" in the system log) and throws away every command buffer in flight.
//! wgpu's Metal backend counts a failed command buffer as completed and says nothing, so the
//! editor carried on drawing with whatever the reset left behind: textures whose upload was
//! discarded, temporal histories and lookup tables half written. The picture went black and green
//! and stayed that way until a restart (2026-10-16, a minute into a new dungeon).
//!
//! The canary: after each frame's work an empty command buffer is committed on wgpu's own queue,
//! so it runs in order behind that frame. A reset discards it ("victim of GPU error/recovery") and
//! it ends in an error. The main world then saves unsaved work (in place, or to the recovery slot)
//! and asks to restart Skyfold, which reopens the same world (or dungeon map).
//!
//! MCP: `gpu_status`, `app_restart`. Test hook: `WB_GPU_FAULT_TEST=<frames>` reports a reset
//! after that many frames without touching the GPU.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, Id, Margin, RichText, Stroke, vec2};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use serde_json::{Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::ui::theme::*;
use crate::ui::widgets;

/// Set by the render world when a canary failed; shared with the main world.
#[derive(Resource, Clone, Default)]
pub struct GpuFault(Arc<Mutex<Option<String>>>);

impl GpuFault {
    fn report(&self, what: String) {
        let mut f = self.0.lock().unwrap();
        if f.is_none() {
            *f = Some(what);
        }
    }

    fn take(&self) -> Option<String> {
        self.0.lock().unwrap().take()
    }
}

/// What the editor knows about a reset (main world).
#[derive(Resource, Default)]
pub struct GpuReset {
    /// Real time of the reset (s) and what was seen.
    pub at: Option<(f32, String)>,
    /// Where the unsaved work went (or that there was none).
    pub saved: String,
    /// The notice is up.
    pub notice: bool,
    /// Restart asked for (the notice's button or `app_restart`).
    pub restart: bool,
}

pub struct GpuWatchdogPlugin;

impl Plugin for GpuWatchdogPlugin {
    fn build(&self, app: &mut App) {
        let fault = GpuFault::default();
        app.insert_resource(fault.clone())
            .init_resource::<GpuReset>()
            .add_systems(Last, (react, restart).chain())
            .add_systems(EguiPrimaryContextPass, notice);
        register_mcp(app);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .insert_resource(fault)
                .init_resource::<Canaries>()
                .add_systems(Render, canary.in_set(RenderSystems::Cleanup));
        }
    }
}

/// The canaries still on the GPU, oldest first.
#[derive(Resource, Default)]
struct Canaries {
    #[cfg(target_os = "macos")]
    pending: std::collections::VecDeque<metal::Canary>,
    frames: u32,
}

#[cfg(target_os = "macos")]
mod metal {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandQueue};

    /// An empty command buffer committed behind a frame's work.
    pub struct Canary(Retained<ProtocolObject<dyn MTLCommandBuffer>>);
    // SAFETY: only its status is read after commit; Metal command buffers may be queried from any
    // thread.
    unsafe impl Send for Canary {}
    unsafe impl Sync for Canary {}

    pub enum State {
        Running,
        Done,
        Failed,
    }

    impl Canary {
        pub fn state(&self) -> State {
            match self.0.status() {
                MTLCommandBufferStatus::Completed => State::Done,
                MTLCommandBufferStatus::Error => State::Failed,
                _ => State::Running,
            }
        }
    }

    pub fn commit(queue: &wgpu::Queue) -> Option<Canary> {
        // SAFETY: the queue is only used to make a command buffer, which is committed at once
        // (as wgpu-hal does for its own internal buffers).
        let hal = unsafe { queue.as_hal::<wgpu::hal::api::Metal>() }?;
        let cb = hal.as_raw().commandBuffer()?;
        cb.commit();
        Some(Canary(cb))
    }
}

fn canary(
    queue: Res<bevy::render::renderer::RenderQueue>,
    fault: Res<GpuFault>,
    mut c: ResMut<Canaries>,
) {
    c.frames += 1;
    if let Some(n) = std::env::var("WB_GPU_FAULT_TEST")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        && c.frames == n
    {
        fault.report("simulated (WB_GPU_FAULT_TEST)".into());
    }
    #[cfg(target_os = "macos")]
    {
        while let Some(front) = c.pending.front() {
            match front.state() {
                metal::State::Running => break,
                metal::State::Done => {}
                metal::State::Failed => {
                    fault.report("a command buffer was discarded by a GPU error or reset".into())
                }
            }
            c.pending.pop_front();
        }
        // (the GPU far behind: don't pile more up)
        if c.pending.len() < 8
            && let Some(k) = metal::commit(&queue)
        {
            c.pending.push_back(k);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = queue;
}

/// A reset was seen: log it, keep the unsaved work, put the notice up.
fn react(world: &mut World) {
    let Some(what) = world.resource::<GpuFault>().take() else {
        return;
    };
    let now = world.resource::<Time<Real>>().elapsed_secs();
    let mut r = world.resource_mut::<GpuReset>();
    if r.at.is_some() {
        return;
    }
    r.at = Some((now, what.clone()));
    error!("GPU reset detected ({what}): the picture can't be trusted until Skyfold restarts");
    let dirty = world.resource::<crate::world::WorldMeta>().dirty;
    let _ = world.run_system_cached(crate::io::save_recovery);
    let meta = world.resource::<crate::world::WorldMeta>();
    let saved = match (&meta.path, dirty, meta.dirty) {
        (_, false, _) => "Nothing was unsaved.".to_string(),
        (Some(p), true, false) => format!(
            "Your work was saved to {}.",
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        _ => "Unsaved work was kept in worlds/_recovery.world.".to_string(),
    };
    let mut r = world.resource_mut::<GpuReset>();
    r.saved = saved;
    r.notice = true;
}

/// Restart asked for: save, start a new Skyfold once this one has quit, quit.
fn restart(world: &mut World) {
    if !std::mem::take(&mut world.resource_mut::<GpuReset>().restart) {
        return;
    }
    let _ = world.run_system_cached(crate::io::save_recovery);
    match relaunch() {
        Ok(()) => info!("restarting Skyfold"),
        Err(e) => error!("could not restart Skyfold: {e}"),
    }
    world.write_message(AppExit::Success);
}

/// Starts this program again (same arguments and environment) once this process has ended, so
/// the new one gets the window, the MCP port and `.last`. Automated runs only quit.
fn relaunch() -> std::io::Result<()> {
    if std::env::var("WB_SHOT").is_ok() || std::env::var("WB_FRESH").is_ok() {
        return Ok(());
    }
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; shift; exec "$@""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(exe)
        .args(std::env::args_os().skip(1))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

fn notice(mut contexts: EguiContexts, mut r: ResMut<GpuReset>) {
    if !r.notice {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let frame = egui::Frame::new()
        .fill(GLASS_SOLID)
        .stroke(Stroke::new(1.0, LINE_STRONG))
        .corner_radius(CornerRadius::same(R_PANEL))
        .inner_margin(Margin::same(22))
        .shadow(egui::epaint::Shadow {
            offset: [0, 12],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(160),
        });
    egui::Modal::new(Id::new("gpu_reset")).backdrop_color(Color32::from_black_alpha(150)).frame(frame).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), egui::Sense::hover());
            widgets::paint_icon(ui, rect.center(), "ExclamationTriangle", 24.0, WARN);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("The graphics card was reset").font(FontId::new(17.0, semibold())).color(TEXT));
                ui.add(egui::Label::new(RichText::new("A frame took the GPU too long, so macOS stopped and reset it. Some of what Skyfold had on the GPU was lost, so the picture can turn black, green or patchy until Skyfold restarts.").color(TEXT_DIM)).wrap());
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(&r.saved).color(TEXT_DIM)).wrap());
            });
        });
        ui.add_space(16.0);
        if widgets::primary_icon_button(ui, "ArrowRotateClockwise", "Restart Skyfold").on_hover_text("Saves, then reopens this world as it is now").clicked() {
            r.restart = true;
            r.notice = false;
        }
        ui.add_space(6.0);
        if widgets::wide_icon_button(ui, "CrossSmall","Keep working without restarting").on_hover_text("The picture may stay wrong").clicked() {
            r.notice = false;
        }
    });
}

fn register_mcp(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "gpu_status",
        description: "Whether the graphics card was reset while Skyfold ran (macOS resets the GPU when a frame takes it too long or a shader faults; Skyfold watches for it). After a reset the picture and screenshots can be wrong (black, green, patchy textures) until Skyfold restarts: app_restart. Read-only.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: gpu_status,
    })
    .mcp_tool(McpTool {
        name: "app_restart",
        description: "Saves unsaved work (in place, or the recovery slot for an untitled world) and restarts Skyfold Studio, which reopens the same world or dungeon map. For after a GPU reset (gpu_status). This MCP connection ends; reconnect after a few seconds.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: app_restart,
    });
}

fn gpu_status(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    let now = world.resource::<Time<Real>>().elapsed_secs();
    let r = world.resource::<GpuReset>();
    let v = match &r.at {
        Some((at, what)) => {
            json!({"reset": true, "seconds_ago": (now - at).round(), "seen": what, "saved": r.saved, "advice": "restart Skyfold (app_restart) before trusting the picture or screenshots"})
        }
        None => json!({"reset": false}),
    };
    Ok(vec![McpContent::Json(v)])
}

fn app_restart(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    world.resource_mut::<GpuReset>().restart = true;
    Ok(vec![McpContent::Text(
        "Saving and restarting Skyfold; reconnect in a few seconds.".into(),
    )])
}
