//! Baseview WindowHandler implementation for Slint.

use crate::event_translation::translate_event;
use crate::platform::set_pending_window;
use crate::{SlintMouseControl, SlintState};
use nih_plug::prelude::GuiContext;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::WindowAdapter;
use slint::{LogicalPosition, PhysicalSize};
use std::cell::RefCell;
use std::num::{NonZeroU32, NonZeroIsize};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::io::Write;

fn debug_log(msg: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/nih_plug_slint_debug.log")
    {
        let _ = writeln!(file, "{}", msg);
    }
}

/// Install a panic hook that logs panic details to our debug log
fn install_panic_hook() {
    use std::sync::Once;
    static HOOK_INSTALLED: Once = Once::new();

    HOOK_INSTALLED.call_once(|| {
        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let msg = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic payload".to_string()
            };

            let location = if let Some(loc) = panic_info.location() {
                format!("{}:{}:{}", loc.file(), loc.line(), loc.column())
            } else {
                "unknown location".to_string()
            };

            debug_log(&format!("PANIC DETAILS: {} at {}", msg, location));

            // Don't call original hook - we're handling panics ourselves
            let _ = &original_hook;
        }));
    });
}

/// The Slint window handler that implements baseview's WindowHandler trait.
pub struct SlintWindowHandler<C: slint::ComponentHandle + 'static> {
    #[allow(dead_code)]
    gui_context: Arc<dyn GuiContext>,
    slint_state: Arc<SlintState>,

    /// The Slint window adapter (MinimalSoftwareWindow)
    slint_window: Rc<MinimalSoftwareWindow>,

    /// The Slint component instance
    _component: C,

    /// Softbuffer context
    _sb_context: softbuffer::Context<SoftbufferWindowHandleAdapter>,

    /// Softbuffer surface for blitting pixels
    sb_surface: softbuffer::Surface<SoftbufferWindowHandleAdapter, SoftbufferWindowHandleAdapter>,

    /// Pixel buffer for rendering (RGBA format)
    pixel_buffer: RefCell<Vec<slint::Rgb8Pixel>>,

    /// Physical dimensions of the window
    physical_width: u32,
    physical_height: u32,

    /// Current scaling factor
    scale_factor: f32,

    /// Last known mouse position for events that don't include position
    last_mouse_position: RefCell<LogicalPosition>,

    /// Track whether a mouse button is currently pressed (for drag-outside-window handling)
    mouse_button_pressed: RefCell<bool>,

    /// Mouse control for unbounded movement
    mouse_control: SlintMouseControl,

    /// Whether unbounded mouse movement is currently active
    unbounded_active: RefCell<bool>,
}

impl<C: slint::ComponentHandle + 'static> SlintWindowHandler<C> {
    pub fn new<F>(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        slint_state: Arc<SlintState>,
        component_factory: Arc<F>,
        mouse_control: SlintMouseControl,
        scale_factor: f32,
        component_weak_out: Arc<parking_lot::Mutex<Option<slint::Weak<C>>>>,
    ) -> Self
    where
        F: Fn(Arc<dyn GuiContext>, SlintMouseControl) -> C + Send + Sync + 'static,
    {
        install_panic_hook();
        debug_log("SlintWindowHandler::new() starting");

        let (unscaled_width, unscaled_height) = slint_state.size();
        let physical_width = (unscaled_width as f32 * scale_factor).round() as u32;
        let physical_height = (unscaled_height as f32 * scale_factor).round() as u32;
        debug_log(&format!(
            "Window size: {}x{} (scale: {})",
            physical_width, physical_height, scale_factor
        ));

        // Create softbuffer context and surface
        debug_log("Creating softbuffer context...");
        let target = baseview_window_to_surface_target(window);
        let sb_context = match softbuffer::Context::new(target.clone()) {
            Ok(ctx) => {
                debug_log("Softbuffer context created successfully");
                ctx
            }
            Err(e) => {
                debug_log(&format!("FAILED to create softbuffer context: {:?}", e));
                panic!("could not get softbuffer context: {:?}", e);
            }
        };

        debug_log("Creating softbuffer surface...");
        let mut sb_surface = match softbuffer::Surface::new(&sb_context, target) {
            Ok(surface) => {
                debug_log("Softbuffer surface created successfully");
                surface
            }
            Err(e) => {
                debug_log(&format!("FAILED to create softbuffer surface: {:?}", e));
                panic!("could not create softbuffer surface: {:?}", e);
            }
        };

        debug_log("Resizing softbuffer surface...");
        sb_surface
            .resize(
                NonZeroU32::new(physical_width).unwrap_or(NonZeroU32::new(1).unwrap()),
                NonZeroU32::new(physical_height).unwrap_or(NonZeroU32::new(1).unwrap()),
            )
            .unwrap();
        debug_log("Softbuffer surface resized");

        // Create the Slint window adapter
        debug_log("Creating MinimalSoftwareWindow...");
        let slint_window: Rc<MinimalSoftwareWindow> = MinimalSoftwareWindow::new(
            slint::platform::software_renderer::RepaintBufferType::ReusedBuffer,
        );
        debug_log("MinimalSoftwareWindow created");

        // Set the scale factor first so Slint knows how to interpret the physical size
        slint_window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
            scale_factor,
        });

        // Set the window size
        slint_window.set_size(PhysicalSize::new(physical_width, physical_height));

        // Set this window as the pending window so the component will use it
        debug_log("Setting pending window...");
        set_pending_window(slint_window.clone());

        // Create the component - it will use our window via the platform
        debug_log("Creating Slint component...");
        let component = component_factory(Arc::clone(&gui_context), mouse_control.clone());
        debug_log("Slint component created");

        // Store the weak reference for param change callbacks
        *component_weak_out.lock() = Some(component.as_weak());

        // Show the component in the window
        debug_log("Showing Slint component...");
        component.show().expect("Failed to show Slint component");
        debug_log("Slint component shown");

        // Mark the window as active so Slint processes input events
        slint_window.dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(true));
        debug_log("Window marked as active");

        // Request an initial redraw
        slint_window.request_redraw();

        // Allocate pixel buffer
        let pixel_count = (physical_width * physical_height) as usize;
        let pixel_buffer = vec![slint::Rgb8Pixel::default(); pixel_count];

        Self {
            gui_context,
            slint_state,
            slint_window,
            _component: component,
            _sb_context: sb_context,
            sb_surface,
            pixel_buffer: RefCell::new(pixel_buffer),
            physical_width,
            physical_height,
            scale_factor,
            last_mouse_position: RefCell::new(LogicalPosition::default()),
            mouse_button_pressed: RefCell::new(false),
            mouse_control,
            unbounded_active: RefCell::new(false),
        }
    }
}

impl<C: slint::ComponentHandle + 'static> SlintWindowHandler<C> {
    /// Process any pending cursor control requests immediately.
    /// Called from both on_frame() and on_event() to ensure responsive cursor restoration.
    fn process_cursor_requests(&mut self, window: &mut baseview::Window) {
        if let Some((enable, restore_position)) = self.mouse_control.take_request() {
            if enable && !*self.unbounded_active.borrow() {
                window.enable_unbounded_mouse_movement(true, restore_position);
                *self.unbounded_active.borrow_mut() = true;
            } else if !enable && *self.unbounded_active.borrow() {
                window.enable_unbounded_mouse_movement(false, false);
                *self.unbounded_active.borrow_mut() = false;
            }
        }
    }

    fn on_frame_inner(&mut self) {
        // DEBUG: Uncomment below to test if softbuffer blit works (should show red)
        // if let Ok(mut buffer) = self.sb_surface.buffer_mut() {
        //     for pixel in buffer.iter_mut() {
        //         *pixel = 0x00FF0000; // Red
        //     }
        //     let _ = buffer.present();
        // }
        // return;

        // Update Slint timers and animations
        slint::platform::update_timers_and_animations();

        // Request a redraw for animations
        self.slint_window.request_redraw();

        // Render if needed
        self.slint_window.draw_if_needed(|renderer| {
            let mut pixel_buffer = self.pixel_buffer.borrow_mut();
            renderer.render(&mut pixel_buffer, self.physical_width as usize);
        });

        // Blit to softbuffer
        if let Ok(mut buffer) = self.sb_surface.buffer_mut() {
            let pixel_buffer = self.pixel_buffer.borrow();
            for (i, pixel) in pixel_buffer.iter().enumerate() {
                // Convert RGBA8 to ARGB32 (softbuffer format)
                // Format: 0x00RRGGBB (softbuffer on macOS uses 0RGB)
                let r = pixel.r as u32;
                let g = pixel.g as u32;
                let b = pixel.b as u32;
                buffer[i] = (r << 16) | (g << 8) | b;
            }
            // Don't unwrap - just ignore present errors
            let _ = buffer.present();
        }
    }
}

impl<C: slint::ComponentHandle + 'static> baseview::WindowHandler for SlintWindowHandler<C> {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        // Wrap everything in catch_unwind to prevent panics from aborting in C callback
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Poll for mouse control requests
            self.process_cursor_requests(window);

            self.on_frame_inner();
        }));

        if let Err(e) = result {
            debug_log(&format!("PANIC in on_frame: {:?}", e));
        }
    }

    fn on_event(
        &mut self,
        window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        // Wrap in catch_unwind to prevent panics from aborting in C callback
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let status = self.on_event_inner(event);

            // Process cursor control requests immediately after event dispatch.
            // This ensures that when a PointerReleased event triggers drag_ended(),
            // the cursor restoration happens immediately rather than waiting for
            // the next on_frame() call (which may be delayed up to 15ms or more).
            self.process_cursor_requests(window);

            status
        }));

        match result {
            Ok(status) => status,
            Err(e) => {
                debug_log(&format!("PANIC in on_event: {:?}", e));
                baseview::EventStatus::Ignored
            }
        }
    }
}

impl<C: slint::ComponentHandle + 'static> SlintWindowHandler<C> {
    fn on_event_inner(&mut self, event: baseview::Event) -> baseview::EventStatus {
        // Handle window resize specially
        if let baseview::Event::Window(baseview::WindowEvent::Resized(window_info)) = &event {
            let logical_size = window_info.logical_size();
            let physical_size = window_info.physical_size();
            let new_scale_factor = window_info.scale() as f32;

            debug_log(&format!(
                "RESIZE: logical={}x{}, physical={}x{}, scale={}, old_scale={}",
                logical_size.width, logical_size.height,
                physical_size.width, physical_size.height,
                new_scale_factor, self.scale_factor
            ));

            self.slint_state.size.store((
                logical_size.width.round() as u32,
                logical_size.height.round() as u32,
            ));

            self.physical_width = physical_size.width;
            self.physical_height = physical_size.height;

            // Update scale factor from actual window info (fixes Retina display rendering)
            if (new_scale_factor - self.scale_factor).abs() > 0.001 {
                debug_log(&format!("Updating scale factor from {} to {}", self.scale_factor, new_scale_factor));
                self.scale_factor = new_scale_factor;
                // Inform Slint of the scale factor change
                self.slint_window.dispatch_event(
                    slint::platform::WindowEvent::ScaleFactorChanged {
                        scale_factor: new_scale_factor,
                    },
                );
            }

            // Resize softbuffer surface
            if let (Some(w), Some(h)) = (
                NonZeroU32::new(self.physical_width),
                NonZeroU32::new(self.physical_height),
            ) {
                let _ = self.sb_surface.resize(w, h);
            }

            // Resize pixel buffer
            let pixel_count = (self.physical_width * self.physical_height) as usize;
            self.pixel_buffer.borrow_mut().resize(pixel_count, slint::Rgb8Pixel::default());

            // Update Slint window size
            self.slint_window
                .set_size(PhysicalSize::new(self.physical_width, self.physical_height));

            // Also dispatch a Resized event with logical size to ensure layout is recomputed
            self.slint_window.dispatch_event(slint::platform::WindowEvent::Resized {
                size: slint::LogicalSize::new(
                    logical_size.width as f32,
                    logical_size.height as f32,
                ),
            });

            // Request a redraw after resize
            self.slint_window.request_redraw();
        }

        // Track mouse position for events that need it
        if let baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, .. }) = &event {
            // On macOS, baseview reports coordinates in logical (post-scaled) units,
            // so we should NOT divide by scale_factor. The coordinates are already correct.
            // In unbounded mode, baseview now handles delta tracking and reports virtual positions.
            let logical_x = (position.x as f32).max(0.0);
            let logical_y = (position.y as f32).max(0.0);

            *self.last_mouse_position.borrow_mut() = LogicalPosition::new(logical_x, logical_y);
        }

        // Track mouse button state for drag-outside-window handling
        if let baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed { .. }) = &event {
            *self.mouse_button_pressed.borrow_mut() = true;
        }
        if let baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased { .. }) = &event {
            *self.mouse_button_pressed.borrow_mut() = false;
        }

        // Translate and dispatch the event
        let is_button_pressed = *self.mouse_button_pressed.borrow();
        if let Some(mut slint_event) = translate_event(&event, self.scale_factor, is_button_pressed) {
            // Fill in mouse position for events that need it
            let last_pos = *self.last_mouse_position.borrow();
            match &mut slint_event {
                slint::platform::WindowEvent::PointerPressed { position, .. } => {
                    *position = last_pos;
                }
                slint::platform::WindowEvent::PointerReleased { position, .. } => {
                    *position = last_pos;
                }
                slint::platform::WindowEvent::PointerScrolled { position, .. } => {
                    *position = last_pos;
                }
                _ => {}
            }

            // Use try_dispatch_event to catch any errors
            match self.slint_window.try_dispatch_event(slint_event) {
                Ok(()) => {}
                Err(e) => {
                    debug_log(&format!("Event dispatch error: {:?}", e));
                }
            }

            // Process timers/animations after event dispatch - this may be needed
            // for Slint to fully process the event
            slint::platform::update_timers_and_animations();

            // Request a redraw after processing events
            self.slint_window.request_redraw();

            baseview::EventStatus::Captured
        } else {
            baseview::EventStatus::Ignored
        }
    }
}

/// Softbuffer uses raw_window_handle v6, but baseview uses raw_window_handle v5, so we need to
/// adapt it ourselves.
#[derive(Clone)]
struct SoftbufferWindowHandleAdapter {
    raw_display_handle: raw_window_handle_06::RawDisplayHandle,
    raw_window_handle: raw_window_handle_06::RawWindowHandle,
}

impl raw_window_handle_06::HasDisplayHandle for SoftbufferWindowHandleAdapter {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle_06::DisplayHandle<'_>, raw_window_handle_06::HandleError> {
        unsafe {
            Ok(raw_window_handle_06::DisplayHandle::borrow_raw(
                self.raw_display_handle,
            ))
        }
    }
}

impl raw_window_handle_06::HasWindowHandle for SoftbufferWindowHandleAdapter {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle_06::WindowHandle<'_>, raw_window_handle_06::HandleError> {
        unsafe {
            Ok(raw_window_handle_06::WindowHandle::borrow_raw(
                self.raw_window_handle,
            ))
        }
    }
}

fn baseview_window_to_surface_target(
    window: &baseview::Window<'_>,
) -> SoftbufferWindowHandleAdapter {
    use raw_window_handle::{HasRawDisplayHandle, HasRawWindowHandle};

    let raw_display_handle = window.raw_display_handle();
    let raw_window_handle = window.raw_window_handle();

    SoftbufferWindowHandleAdapter {
        raw_display_handle: match raw_display_handle {
            raw_window_handle::RawDisplayHandle::AppKit(_) => {
                raw_window_handle_06::RawDisplayHandle::AppKit(
                    raw_window_handle_06::AppKitDisplayHandle::new(),
                )
            }
            raw_window_handle::RawDisplayHandle::Xlib(handle) => {
                raw_window_handle_06::RawDisplayHandle::Xlib(
                    raw_window_handle_06::XlibDisplayHandle::new(
                        NonNull::new(handle.display),
                        handle.screen,
                    ),
                )
            }
            raw_window_handle::RawDisplayHandle::Xcb(handle) => {
                raw_window_handle_06::RawDisplayHandle::Xcb(
                    raw_window_handle_06::XcbDisplayHandle::new(
                        NonNull::new(handle.connection),
                        handle.screen,
                    ),
                )
            }
            raw_window_handle::RawDisplayHandle::Windows(_) => {
                raw_window_handle_06::RawDisplayHandle::Windows(
                    raw_window_handle_06::WindowsDisplayHandle::new(),
                )
            }
            _ => panic!("Unsupported display handle type"),
        },
        raw_window_handle: match raw_window_handle {
            raw_window_handle::RawWindowHandle::AppKit(handle) => {
                raw_window_handle_06::RawWindowHandle::AppKit(
                    raw_window_handle_06::AppKitWindowHandle::new(
                        NonNull::new(handle.ns_view).unwrap(),
                    ),
                )
            }
            raw_window_handle::RawWindowHandle::Xlib(handle) => {
                raw_window_handle_06::RawWindowHandle::Xlib(
                    raw_window_handle_06::XlibWindowHandle::new(handle.window),
                )
            }
            raw_window_handle::RawWindowHandle::Xcb(handle) => {
                raw_window_handle_06::RawWindowHandle::Xcb(
                    raw_window_handle_06::XcbWindowHandle::new(
                        NonZeroU32::new(handle.window).unwrap(),
                    ),
                )
            }
            raw_window_handle::RawWindowHandle::Win32(handle) => {
                let mut raw_handle = raw_window_handle_06::Win32WindowHandle::new(
                    NonZeroIsize::new(handle.hwnd as isize).unwrap(),
                );
                raw_handle.hinstance = NonZeroIsize::new(handle.hinstance as isize);
                raw_window_handle_06::RawWindowHandle::Win32(raw_handle)
            }
            _ => panic!("Unsupported window handle type"),
        },
    }
}
