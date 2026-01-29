//! An [`Editor`] implementation for Slint.

use crate::platform::ensure_slint_platform;
use crate::window_handler::SlintWindowHandler;
use crate::{SlintMouseControl, SlintState};
use baseview::{Size, WindowHandle, WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::{Editor, GuiContext, ParentWindowHandle};
use parking_lot::Mutex;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn debug_log(msg: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/nih_plug_slint_debug.log")
    {
        let _ = writeln!(file, "{}", msg);
    }
}

/// Type alias for the param values changed callback.
pub type ParamChangedCallback<C> = Box<dyn Fn(&C) + Send + Sync>;

/// An [`Editor`] implementation that uses Slint for rendering.
pub(crate) struct SlintEditor<C, F>
where
    C: slint::ComponentHandle + 'static,
    F: Fn(Arc<dyn GuiContext>, SlintMouseControl) -> C + Send + Sync + 'static,
{
    pub(crate) slint_state: Arc<SlintState>,
    pub(crate) component_factory: Arc<F>,
    /// The scaling factor reported by the host, if any. On macOS this will never be set and we
    /// should use the system scaling factor instead.
    pub(crate) scaling_factor: AtomicCell<Option<f32>>,
    /// Optional callback invoked when parameter values change from the host.
    pub(crate) on_param_values_changed: Option<ParamChangedCallback<C>>,
    /// Weak reference to the component, set when spawn() is called.
    pub(crate) component_weak: Arc<Mutex<Option<slint::Weak<C>>>>,
}

/// This version of `baseview` uses a different version of `raw_window_handle` than NIH-plug, so we
/// need to adapt it ourselves.
struct ParentWindowHandleAdapter(nih_plug::editor::ParentWindowHandle);

unsafe impl HasRawWindowHandle for ParentWindowHandleAdapter {
    fn raw_window_handle(&self) -> RawWindowHandle {
        match self.0 {
            ParentWindowHandle::X11Window(window) => {
                let mut handle = raw_window_handle::XcbWindowHandle::empty();
                handle.window = window;
                RawWindowHandle::Xcb(handle)
            }
            ParentWindowHandle::AppKitNsView(ns_view) => {
                let mut handle = raw_window_handle::AppKitWindowHandle::empty();
                handle.ns_view = ns_view;
                RawWindowHandle::AppKit(handle)
            }
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                let mut handle = raw_window_handle::Win32WindowHandle::empty();
                handle.hwnd = hwnd;
                RawWindowHandle::Win32(handle)
            }
        }
    }
}

impl<C, F> SlintEditor<C, F>
where
    C: slint::ComponentHandle + 'static,
    F: Fn(Arc<dyn GuiContext>, SlintMouseControl) -> C + Send + Sync + 'static,
{
    /// Invoke the param changed callback if one is set and the component is alive.
    fn invoke_param_changed_callback(&self) {
        if let Some(callback) = &self.on_param_values_changed {
            if let Some(component) = self.component_weak.lock().as_ref().and_then(|w| w.upgrade())
            {
                callback(&component);
            }
        }
    }
}

impl<C, F> Editor for SlintEditor<C, F>
where
    C: slint::ComponentHandle + 'static,
    F: Fn(Arc<dyn GuiContext>, SlintMouseControl) -> C + Send + Sync + 'static,
{
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        debug_log("Editor::spawn() called");

        // Ensure the Slint platform is set up
        ensure_slint_platform();

        let (unscaled_width, unscaled_height) = self.slint_state.scaled_logical_size();
        let scaling_factor = self.scaling_factor.load();
        let user_scale_factor = self.slint_state.user_scale_factor();
        debug_log(&format!(
            "Editor size: {}x{}, system scaling: {:?}, user scale: {}",
            unscaled_width, unscaled_height, scaling_factor, user_scale_factor
        ));

        let gui_context = Arc::clone(&context);
        let slint_state = Arc::clone(&self.slint_state);
        let component_factory = Arc::clone(&self.component_factory);
        let component_weak = Arc::clone(&self.component_weak);

        // Create the mouse control that will be passed to the component factory
        let mouse_control = SlintMouseControl::new();

        debug_log("Opening baseview window...");
        let window = baseview::Window::open_parented(
            &ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Slint Plugin Window"),
                // Baseview should be doing the DPI scaling for us
                size: Size::new(unscaled_width as f64, unscaled_height as f64),
                // NOTE: For some reason passing 1.0 here causes the UI to be scaled on macOS but
                //       not the mouse events.
                scale: scaling_factor
                    .map(|factor| WindowScalePolicy::ScaleFactor(factor as f64))
                    .unwrap_or(WindowScalePolicy::SystemScaleFactor),

            },
            move |window: &mut baseview::Window<'_>| -> SlintWindowHandler<C> {
                SlintWindowHandler::new(
                    window,
                    gui_context,
                    slint_state,
                    component_factory,
                    mouse_control,
                    scaling_factor.unwrap_or(1.0),
                    component_weak,
                )
            },
        );

        self.slint_state.open.store(true, Ordering::Release);
        Box::new(SlintEditorHandle {
            slint_state: self.slint_state.clone(),
            window,
        })
    }

    fn size(&self) -> (u32, u32) {
        self.slint_state.size()
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        // If the editor is currently open then the host must not change the current HiDPI scale as
        // we don't have a way to handle that. Ableton Live does this.
        if self.slint_state.is_open() {
            return false;
        }

        self.scaling_factor.store(Some(factor));
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {
        // For individual param changes, we also call the callback
        self.invoke_param_changed_callback();
    }

    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}

    fn param_values_changed(&self) {
        self.invoke_param_changed_callback();
    }
}

/// The window handle used for [`SlintEditor`].
struct SlintEditorHandle {
    slint_state: Arc<SlintState>,
    window: WindowHandle,
}

/// The window handle enum stored within 'WindowHandle' contains raw pointers. Is there a way around
/// having this requirement?
unsafe impl Send for SlintEditorHandle {}

impl Drop for SlintEditorHandle {
    fn drop(&mut self) {
        self.slint_state.open.store(false, Ordering::Release);
        // XXX: This should automatically happen when the handle gets dropped, but apparently not
        self.window.close();
    }
}
