//! Saving power in the background. Bevy keeps an unfocused window updating about sixty times a
//! second, so an editor left open behind other apps kept a few cores and the GPU busy. While the
//! editor isn't the front window it now updates `BACKGROUND_HZ` times a second instead. It stays
//! at full speed while an agent drives it over MCP (every request wakes the event loop at once and
//! keeps it awake for `MCP_AWAKE`), while a flythrough is being captured, and in automated runs
//! (`WB_SHOT`, `WB_FRESH`, `WB_BENCH`, `WB_MCP_CALL`: their windows are unfocused on purpose).
//! The `background_throttle` pref (default on) or `WB_BG_THROTTLE=0` turns it off; `=1` forces
//! it on even in an automated run.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::winit::{EventLoopProxyWrapper, UpdateMode, WinitSettings, WinitUserEvent};

/// Updates per second while the editor is in the background.
const BACKGROUND_HZ: f64 = 10.0;
/// How long the editor stays at full speed after an MCP request.
const MCP_AWAKE: Duration = Duration::from_secs(4);

static START: OnceLock<Instant> = OnceLock::new();
/// When the last MCP request arrived (ms since `START`; 0 = never).
static LAST_MCP_MS: AtomicU64 = AtomicU64::new(0);
/// Wakes the winit event loop from another thread (set once the loop exists).
static WAKER: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Called on the MCP HTTP threads for every tool call: a sleeping editor wakes at once.
pub fn mcp_request() {
    LAST_MCP_MS.store(now_ms().max(1), Ordering::Relaxed);
    if let Some(wake) = WAKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        wake();
    }
}

/// Whether the editor slows down in the background (the `background_throttle` pref).
#[derive(Resource)]
pub struct BackgroundThrottle(pub bool);

impl Default for BackgroundThrottle {
    fn default() -> Self {
        // `WB_BG_THROTTLE=1` forces it on (testing it in an automated run), `=0` off
        match std::env::var("WB_BG_THROTTLE").as_deref() {
            Ok("1") => return Self(true),
            Ok("0") => return Self(false),
            _ => {}
        }
        let automated = ["WB_SHOT", "WB_FRESH", "WB_BENCH", "WB_MCP_CALL"]
            .iter()
            .any(|k| std::env::var(k).is_ok());
        Self(!automated && crate::prefs::get_bool("background_throttle").unwrap_or(true))
    }
}

pub struct BackgroundPlugin;

impl Plugin for BackgroundPlugin {
    fn build(&self, app: &mut App) {
        START.get_or_init(Instant::now);
        app.init_resource::<BackgroundThrottle>()
            .add_systems(Startup, install_waker)
            .add_systems(Last, (update_mode, save_pref));
    }
}

/// Saves the setting when it really changes (the UI checkbox borrows it mutably every frame).
fn save_pref(throttle: Res<BackgroundThrottle>, mut saved: Local<Option<bool>>) {
    match *saved {
        None => *saved = Some(throttle.0),
        Some(v) if v != throttle.0 => {
            *saved = Some(throttle.0);
            crate::prefs::set("background_throttle", serde_json::json!(throttle.0));
        }
        _ => {}
    }
}

fn install_waker(proxy: Option<Res<EventLoopProxyWrapper>>) {
    let Some(proxy) = proxy else { return };
    let proxy = (**proxy).clone();
    *WAKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(move || {
        let _ = proxy.send_event(WinitUserEvent::WakeUp);
    }));
}

fn update_mode(
    mut settings: ResMut<WinitSettings>,
    throttle: Res<BackgroundThrottle>,
    fly: Option<Res<crate::motion::flythrough::Flythrough>>,
) {
    let last = LAST_MCP_MS.load(Ordering::Relaxed);
    let mcp_awake = last > 0 && now_ms().saturating_sub(last) < MCP_AWAKE.as_millis() as u64;
    let capturing = fly.is_some_and(|f| {
        f.request.is_some() || f.playing.as_ref().is_some_and(|p| p.capture.is_some())
    });
    let hz = if throttle.0 && !mcp_awake && !capturing {
        BACKGROUND_HZ
    } else {
        60.0
    };
    // (Bevy's own unfocused default is this low-power mode at 60 Hz)
    let want = UpdateMode::reactive_low_power(Duration::from_secs_f64(1.0 / hz));
    if settings.unfocused_mode != want {
        settings.unfocused_mode = want;
    }
}
