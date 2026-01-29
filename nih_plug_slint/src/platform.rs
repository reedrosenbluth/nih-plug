//! Custom Slint Platform implementation for NIH-plug.
//!
//! Since `slint::platform::set_platform()` can only be called once per process,
//! we use a global platform that can handle multiple plugin instances.

use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{Platform, PlatformError, WindowAdapter};
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn debug_log(msg: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/nih_plug_slint_debug.log")
    {
        let _ = writeln!(file, "{}", msg);
    }
}

static PLATFORM_START_TIME: OnceLock<Instant> = OnceLock::new();

thread_local! {
    /// Thread-local storage for the window to use when creating a component.
    /// This allows us to inject our own MinimalSoftwareWindow into component creation.
    static PENDING_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

/// Sets the window that should be used for the next component creation on this thread.
/// The window will be consumed when `create_window_adapter` is called.
pub fn set_pending_window(window: Rc<MinimalSoftwareWindow>) {
    PENDING_WINDOW.with(|cell| {
        *cell.borrow_mut() = Some(window);
    });
}

/// Ensures the Slint platform is initialized. This function is idempotent and safe
/// to call multiple times - it will only initialize the platform once.
pub fn ensure_slint_platform() {
    debug_log("ensure_slint_platform() called");

    // Initialize the start time - this will only run once
    PLATFORM_START_TIME.get_or_init(|| {
        debug_log("First-time platform initialization...");
        let start_time = Instant::now();

        let platform = NihPlugSlintPlatform;
        match slint::platform::set_platform(Box::new(platform)) {
            Ok(()) => {
                debug_log("Slint platform set successfully");
            }
            Err(e) => {
                debug_log(&format!("FAILED to set Slint platform: {:?}", e));
                panic!("Failed to set Slint platform - another platform may already be set: {:?}", e);
            }
        }

        start_time
    });

    debug_log("ensure_slint_platform() completed");
}

/// Custom Slint platform for NIH-plug integration.
///
/// This platform provides minimal functionality needed for software rendering:
/// - Creates `MinimalSoftwareWindow` adapters for each window
/// - Provides timing information via `duration_since_start()`
///
/// Note: This does NOT implement `run_event_loop()` as baseview drives the event loop.
struct NihPlugSlintPlatform;

impl Platform for NihPlugSlintPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        // Check if there's a pending window set via set_pending_window()
        let pending = PENDING_WINDOW.with(|cell| cell.borrow_mut().take());

        if let Some(window) = pending {
            // Use the pre-created window
            debug_log("Using pending window!");
            Ok(window)
        } else {
            // Create a new MinimalSoftwareWindow as fallback
            debug_log("Creating fallback window (NOT using our window!)");
            Ok(MinimalSoftwareWindow::new(
                slint::platform::software_renderer::RepaintBufferType::ReusedBuffer,
            ))
        }
    }

    fn duration_since_start(&self) -> Duration {
        PLATFORM_START_TIME
            .get()
            .map(|start| start.elapsed())
            .unwrap_or_default()
    }

    // We don't implement run_event_loop() because baseview drives the event loop.
    // The default implementation returns an error, which is correct for our use case.
}
