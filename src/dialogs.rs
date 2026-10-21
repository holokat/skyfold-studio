//! Native file dialogs that never stop a frame.
//!
//! A blocking dialog opened inside a system runs the platform's modal loop in the middle of a
//! frame; on macOS that stalls (or deadlocks) the window's event loop: the editor froze on
//! "Add a reference image". Instead each dialog is opened from a helper thread with rfd's async
//! API (on macOS a sheet over the editor window, created on the main thread between frames), and
//! its answer is collected by key on a later frame:
//!
//! ```ignore
//! if clicked { dialogs::pick_file("refmap", rfd::AsyncFileDialog::new().add_filter(…)) }
//! if let Some(path) = dialogs::take_file("refmap") { … }
//! ```
//!
//! One dialog at a time: asking while one is open does nothing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

static ANSWERS: LazyLock<Mutex<HashMap<&'static str, Vec<PathBuf>>>> =
    LazyLock::new(Default::default);
static OPEN: AtomicBool = AtomicBool::new(false);

fn run<F>(key: &'static str, ask: F)
where
    F: std::future::Future<Output = Vec<PathBuf>> + Send + 'static,
{
    if OPEN.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        let picked = bevy::tasks::block_on(ask);
        if !picked.is_empty() {
            ANSWERS
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(key, picked);
        }
        OPEN.store(false, Ordering::Relaxed);
    });
}

pub fn pick_file(key: &'static str, d: rfd::AsyncFileDialog) {
    run(key, async move {
        d.pick_file()
            .await
            .map(|f| vec![f.path().to_path_buf()])
            .unwrap_or_default()
    });
}

pub fn pick_files(key: &'static str, d: rfd::AsyncFileDialog) {
    run(key, async move {
        d.pick_files()
            .await
            .map(|v| v.iter().map(|f| f.path().to_path_buf()).collect())
            .unwrap_or_default()
    });
}

pub fn pick_folder(key: &'static str, d: rfd::AsyncFileDialog) {
    run(key, async move {
        d.pick_folder()
            .await
            .map(|f| vec![f.path().to_path_buf()])
            .unwrap_or_default()
    });
}

pub fn save_file(key: &'static str, d: rfd::AsyncFileDialog) {
    run(key, async move {
        d.save_file()
            .await
            .map(|f| vec![f.path().to_path_buf()])
            .unwrap_or_default()
    });
}

/// The files picked for `key`, once (None until answered; a cancelled dialog never answers).
pub fn take_files(key: &str) -> Option<Vec<PathBuf>> {
    ANSWERS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(key)
}

/// The one file or folder picked for `key`, once.
pub fn take_file(key: &str) -> Option<PathBuf> {
    take_files(key).and_then(|v| v.into_iter().next())
}
