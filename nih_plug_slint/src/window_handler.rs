//! Baseview WindowHandler implementation for Slint.

use crate::event_translation::translate_event;
use crate::SlintState;
use nih_plug::prelude::GuiContext;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::{LogicalPosition, PhysicalSize};
use std::cell::RefCell;
use std::num::{NonZeroU32, NonZeroIsize};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

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
}

impl<C: slint::ComponentHandle + 'static> SlintWindowHandler<C> {
    pub fn new<F>(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        slint_state: Arc<SlintState>,
        component_factory: Arc<F>,
        scale_factor: f32,
    ) -> Self
    where
        F: Fn(Arc<dyn GuiContext>) -> C + Send + Sync + 'static,
    {
        let (unscaled_width, unscaled_height) = slint_state.size();
        let physical_width = (unscaled_width as f32 * scale_factor).round() as u32;
        let physical_height = (unscaled_height as f32 * scale_factor).round() as u32;

        // Create softbuffer context and surface
        let target = baseview_window_to_surface_target(window);
        let sb_context =
            softbuffer::Context::new(target.clone()).expect("could not get softbuffer context");
        let mut sb_surface = softbuffer::Surface::new(&sb_context, target)
            .expect("could not create softbuffer surface");

        sb_surface
            .resize(
                NonZeroU32::new(physical_width).unwrap_or(NonZeroU32::new(1).unwrap()),
                NonZeroU32::new(physical_height).unwrap_or(NonZeroU32::new(1).unwrap()),
            )
            .unwrap();

        // Create the Slint window adapter
        // The platform's create_window_adapter returns a MinimalSoftwareWindow
        let slint_window: Rc<MinimalSoftwareWindow> = MinimalSoftwareWindow::new(
            slint::platform::software_renderer::RepaintBufferType::ReusedBuffer,
        );

        // Set the window size
        slint_window.set_size(PhysicalSize::new(physical_width, physical_height));

        // Create the component
        let component = component_factory(Arc::clone(&gui_context));

        // Show the component in the window
        component.show().expect("Failed to show Slint component");

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
        }
    }
}

impl<C: slint::ComponentHandle + 'static> baseview::WindowHandler for SlintWindowHandler<C> {
    fn on_frame(&mut self, _window: &mut baseview::Window) {
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
            buffer.present().unwrap();
        }
    }

    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        // Handle window resize specially
        if let baseview::Event::Window(baseview::WindowEvent::Resized(window_info)) = &event {
            let logical_size = window_info.logical_size();
            self.slint_state.size.store((
                logical_size.width.round() as u32,
                logical_size.height.round() as u32,
            ));

            self.physical_width = window_info.physical_size().width;
            self.physical_height = window_info.physical_size().height;

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
        }

        // Track mouse position for events that need it
        if let baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, .. }) = &event {
            *self.last_mouse_position.borrow_mut() = LogicalPosition::new(
                position.x as f32 / self.scale_factor,
                position.y as f32 / self.scale_factor,
            );
        }

        // Translate and dispatch the event
        if let Some(mut slint_event) = translate_event(&event, self.scale_factor) {
            // Fill in mouse position for events that need it
            let last_pos = *self.last_mouse_position.borrow();
            match &mut slint_event {
                slint::platform::WindowEvent::PointerPressed { position, .. }
                | slint::platform::WindowEvent::PointerReleased { position, .. }
                | slint::platform::WindowEvent::PointerScrolled { position, .. } => {
                    *position = last_pos;
                }
                _ => {}
            }

            self.slint_window.dispatch_event(slint_event);
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
