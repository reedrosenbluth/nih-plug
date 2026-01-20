//! Custom Slint Platform implementation for NIH-plug.
//!
//! Since `slint::platform::set_platform()` can only be called once per process,
//! we use a global platform that can handle multiple plugin instances.

use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{Platform, PlatformError, WindowAdapter};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static PLATFORM_START_TIME: OnceLock<Instant> = OnceLock::new();

/// Ensures the Slint platform is initialized. This function is idempotent and safe
/// to call multiple times - it will only initialize the platform once.
pub fn ensure_slint_platform() {
    // Initialize the start time - this will only run once
    PLATFORM_START_TIME.get_or_init(|| {
        let start_time = Instant::now();

        let platform = NihPlugSlintPlatform;
        slint::platform::set_platform(Box::new(platform))
            .expect("Failed to set Slint platform - another platform may already be set");

        start_time
    });
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
        // Create a new MinimalSoftwareWindow for each call.
        // ReusedBuffer means we reuse the same buffer between frames for efficiency.
        Ok(MinimalSoftwareWindow::new(
            slint::platform::software_renderer::RepaintBufferType::ReusedBuffer,
        ))
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
