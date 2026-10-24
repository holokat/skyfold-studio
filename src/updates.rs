//! Auto-updates through Sparkle 2 (macOS, the packaged app only). The release script
//! (`tools/release/release_macos.sh`) embeds `Sparkle.framework` and writes the feed settings into
//! Info.plist (`SUFeedURL`, `SUPublicEDKey`, daily checks). At launch the app loads the framework
//! from its bundle and starts Sparkle's standard updater: it checks the feed in the background and
//! shows its own window when there's a new version (notes, Install Update, then Install and
//! Relaunch). Cargo builds have no framework and skip all of this.
//!
//! Relaunching quits through AppKit, past Bevy's window-close save, so the updater's delegate
//! holds the relaunch until the editor has saved (as closing the window does).

use std::sync::atomic::{AtomicBool, Ordering};

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;

/// Sparkle is running (the packaged app with an update feed).
static AVAILABLE: AtomicBool = AtomicBool::new(false);
/// "Check for Updates…" was asked for (UI, MCP); handled on the main thread.
static CHECK: AtomicBool = AtomicBool::new(false);
/// Sparkle is waiting to relaunch: save first.
static SAVE: AtomicBool = AtomicBool::new(false);
/// Saved: let Sparkle relaunch.
static SAVED: AtomicBool = AtomicBool::new(false);

pub struct UpdatesPlugin;

impl Plugin for UpdatesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (main_thread, save_before_relaunch));
    }
}

/// Whether this build can update itself (shows "Check for Updates…").
pub fn available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

/// Check the feed now; Sparkle shows what it finds. False when this build can't update.
pub fn check_now() -> bool {
    if available() {
        CHECK.store(true, Ordering::Relaxed);
    }
    available()
}

/// Starts Sparkle once, runs asked-for checks and, after the save, the held relaunch.
fn main_thread(_main: NonSendMarker, mut started: Local<bool>) {
    if !*started {
        *started = true;
        #[cfg(target_os = "macos")]
        if mac::start() {
            AVAILABLE.store(true, Ordering::Relaxed);
            info!("updates: Sparkle started");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if CHECK.swap(false, Ordering::Relaxed) {
            mac::check();
        }
        if mac::relaunch_waiting() && !SAVE.load(Ordering::Relaxed) {
            SAVE.store(true, Ordering::Relaxed);
        }
        if SAVED.swap(false, Ordering::Relaxed) {
            SAVE.store(false, Ordering::Relaxed);
            mac::relaunch();
        }
    }
}

fn save_before_relaunch(mut data: crate::io::WorldData, auto: Res<crate::io::Autosave>) {
    if SAVE.load(Ordering::Relaxed) && !SAVED.load(Ordering::Relaxed) {
        crate::io::save_before_quit(&mut data, &auto);
        SAVED.store(true, Ordering::Relaxed);
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;

    use block2::{Block, RcBlock};
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject};
    use objc2::{MainThreadOnly, define_class, msg_send};
    use objc2_foundation::{MainThreadMarker, NSBundle, NSObjectProtocol, NSString};

    thread_local! {
        /// `SPUStandardUpdaterController`
        static CONTROLLER: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
        /// Sparkle keeps only a weak reference to its delegate.
        static DELEGATE: RefCell<Option<Retained<UpdaterDelegate>>> = const { RefCell::new(None) };
        /// Sparkle's "go on and relaunch", held until the world is saved.
        static RELAUNCH: RefCell<Option<RcBlock<dyn Fn()>>> = const { RefCell::new(None) };
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; no Drop.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[name = "SkyfoldUpdaterDelegate"]
        struct UpdaterDelegate;

        unsafe impl NSObjectProtocol for UpdaterDelegate {}

        /// `SPUUpdaterDelegate` (optional methods; Sparkle checks `respondsToSelector:`).
        impl UpdaterDelegate {
            #[unsafe(method(updater:shouldPostponeRelaunchForUpdate:untilInvokingBlock:))]
            fn postpone_relaunch(
                &self,
                _updater: &AnyObject,
                _item: &AnyObject,
                block: &Block<dyn Fn()>,
            ) -> Bool {
                // SAFETY: Sparkle passes a valid block; copying keeps it past this call.
                let held = unsafe { RcBlock::copy(block as *const Block<dyn Fn()> as *mut _) };
                match held {
                    Some(b) => {
                        RELAUNCH.with(|r| *r.borrow_mut() = Some(b));
                        Bool::YES
                    }
                    None => Bool::NO,
                }
            }
        }
    );

    /// Load the bundle's Sparkle.framework and start the updater. False in Cargo builds (no
    /// framework) and in bundles without a feed.
    pub fn start() -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let main = NSBundle::mainBundle();
        let feed = main.objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL"));
        let Some(frameworks) = main.privateFrameworksPath() else {
            return false;
        };
        let path = std::path::Path::new(&frameworks.to_string()).join("Sparkle.framework");
        if feed.is_none() || !path.is_dir() {
            return false;
        }
        let Some(bundle) = NSBundle::bundleWithPath(&NSString::from_str(&path.to_string_lossy()))
        else {
            return false;
        };
        // SAFETY: loads the framework we embedded and signed with the app
        if !unsafe { bundle.load() } {
            bevy::log::warn!("updates: couldn't load {}", path.display());
            return false;
        }
        let Some(class) = AnyClass::get(c"SPUStandardUpdaterController") else {
            bevy::log::warn!("updates: Sparkle.framework has no SPUStandardUpdaterController");
            return false;
        };
        let delegate = UpdaterDelegate::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's init.
        let delegate: Retained<UpdaterDelegate> = unsafe { msg_send![super(delegate), init] };
        // SAFETY: -[SPUStandardUpdaterController initWithStartingUpdater:updaterDelegate:userDriverDelegate:]
        let controller: Option<Retained<AnyObject>> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![
                alloc,
                initWithStartingUpdater: true,
                updaterDelegate: &*delegate,
                userDriverDelegate: None::<&AnyObject>
            ]
        };
        let Some(controller) = controller else {
            return false;
        };
        CONTROLLER.with(|c| *c.borrow_mut() = Some(controller));
        DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
        true
    }

    /// The "Check for Updates…" menu action.
    pub fn check() {
        CONTROLLER.with(|c| {
            if let Some(c) = c.borrow().as_ref() {
                // SAFETY: -[SPUStandardUpdaterController checkForUpdates:]
                let _: () = unsafe { msg_send![&**c, checkForUpdates: None::<&AnyObject>] };
            }
        });
    }

    pub fn relaunch_waiting() -> bool {
        RELAUNCH.with(|r| r.borrow().is_some())
    }

    /// Let Sparkle install and relaunch (it quits the app).
    pub fn relaunch() {
        if let Some(b) = RELAUNCH.with(|r| r.borrow_mut().take()) {
            b.call(());
        }
    }
}
