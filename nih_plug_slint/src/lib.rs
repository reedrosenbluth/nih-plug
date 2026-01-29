//! [Slint](https://slint.dev/) editor support for NIH plug.
//!
//! This crate provides a [`create_slint_editor()`] function that can be used to create
//! an [`Editor`] implementation using the Slint GUI framework. It uses software rendering
//! via Slint's `MinimalSoftwareWindow` and blits the result to the window using softbuffer.
//!
//! # Compatibility Note
//!
//! This crate is **incompatible with `nih_plug_iced`** due to a fontconfig native library
//! conflict. Slint uses `yeslogic-fontconfig-sys` while iced uses `servo-fontconfig-sys`,
//! and Cargo only allows one crate to link a given native library. If you need both GUI
//! frameworks, you'll need to use separate workspaces.

#![allow(clippy::type_complexity)]

use crossbeam::atomic::AtomicCell;
use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::{Editor, GuiContext, ParamSetter};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

mod editor;
mod event_translation;
mod platform;
mod window_handler;

pub use editor::ParamChangedCallback;
pub use slint;

/// Control for unbounded mouse movement during drag operations.
///
/// When enabled, the cursor is hidden and its position is frozen visually.
/// Mouse movement is reported as deltas from the starting position,
/// allowing unlimited drag range without the cursor hitting screen edges.
///
/// This is useful for implementing DAW-style knob and slider controls where
/// users can drag vertically or horizontally without being constrained by
/// the screen boundaries.
///
/// # Example
///
/// ```ignore
/// // In your Slint component factory:
/// ui.on_knob_drag_started({
///     let mc = mouse_control.clone();
///     move || mc.enable_unbounded_movement(true)  // restore position after drag
/// });
///
/// ui.on_knob_drag_ended({
///     let mc = mouse_control.clone();
///     move || mc.disable_unbounded_movement()
/// });
/// ```
#[derive(Clone)]
pub struct SlintMouseControl {
    /// Request state: Option<(enable, restore_position)>
    request: Arc<AtomicCell<Option<(bool, bool)>>>,
}

impl SlintMouseControl {
    /// Create a new mouse control instance.
    pub(crate) fn new() -> Self {
        Self {
            request: Arc::new(AtomicCell::new(None)),
        }
    }

    /// Enable unbounded mouse movement for drag operations.
    ///
    /// When enabled:
    /// - The cursor is hidden
    /// - The cursor position is frozen (visually)
    /// - Mouse movement events report deltas from the starting position
    ///
    /// # Arguments
    ///
    /// * `restore_position` - If true, the cursor returns to its original position
    ///   when disabled. If false, the cursor stays where it ended up (accumulated position).
    pub fn enable_unbounded_movement(&self, restore_position: bool) {
        self.request.store(Some((true, restore_position)));
    }

    /// Disable unbounded mouse movement.
    ///
    /// The cursor will become visible again. If `restore_position` was true when
    /// enabled, the cursor will return to its original position.
    pub fn disable_unbounded_movement(&self) {
        self.request.store(Some((false, false)));
    }

    /// Check if unbounded movement is currently requested to be enabled.
    pub fn is_unbounded_requested(&self) -> bool {
        matches!(self.request.load(), Some((true, _)))
    }

    /// Take and clear any pending request.
    pub(crate) fn take_request(&self) -> Option<(bool, bool)> {
        self.request.swap(None)
    }
}

/// Create an [`Editor`] instance using a [Slint](https://slint.dev/) GUI. The [`SlintState`]
/// passed to this function contains the GUI's initial size, and this is kept in sync whenever
/// the GUI gets resized. You can also use this to know if the GUI is open, so you can avoid
/// performing potentially expensive calculations while the GUI is not open. If you want this
/// size to be persisted when restoring a plugin instance, then you can store it in a
/// `#[persist = "key"]` field on your parameters struct.
///
/// The `component_factory` closure receives the [`GuiContext`] wrapped in an [`Arc`] and a
/// [`SlintMouseControl`] for controlling cursor behavior during drag operations. The factory
/// is called each time the editor window is opened.
///
/// See [`SlintState::from_size()`].
///
/// # Example
///
/// ```ignore
/// // In build.rs:
/// fn main() {
///     slint_build::compile("ui/plugin.slint").unwrap();
/// }
///
/// // In lib.rs:
/// slint::include_modules!();
///
/// fn editor(&mut self, _: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
///     let params = self.params.clone();
///
///     create_slint_editor(
///         self.params.editor_state.clone(),
///         move |gui_context, mouse_control| {
///             let ui = MyPluginUI::new().unwrap();
///
///             // Bind parameter to slider
///             ui.set_gain(params.gain.value());
///             ui.on_gain_changed({
///                 let gui_context = gui_context.clone();
///                 let params = params.clone();
///                 move |value| {
///                     let setter = ParamSetter::new(gui_context.as_ref());
///                     setter.begin_set_parameter(&params.gain);
///                     setter.set_parameter(&params.gain, value);
///                     setter.end_set_parameter(&params.gain);
///                 }
///             });
///
///             // Enable unbounded movement during knob drag
///             ui.on_knob_drag_started({
///                 let mc = mouse_control.clone();
///                 move || mc.enable_unbounded_movement(true)
///             });
///
///             ui.on_knob_drag_ended({
///                 let mc = mouse_control.clone();
///                 move || mc.disable_unbounded_movement()
///             });
///
///             ui
///         },
///     )
/// }
/// ```
pub fn create_slint_editor<C, F>(
    slint_state: Arc<SlintState>,
    component_factory: F,
) -> Option<Box<dyn Editor>>
where
    C: slint::ComponentHandle + 'static,
    F: Fn(Arc<dyn GuiContext>, SlintMouseControl) -> C + Send + Sync + 'static,
{
    create_slint_editor_with_param_callback(slint_state, component_factory, None)
}

/// Like [`create_slint_editor`], but with a callback that is invoked when parameter values
/// change from the host (automation, preset load, generic UI, etc.).
///
/// The callback receives a reference to the Slint component, allowing you to update
/// the UI to reflect the new parameter values.
///
/// # Example
///
/// ```ignore
/// create_slint_editor_with_param_callback(
///     self.params.editor_state.clone(),
///     move |gui_context, mouse_control| {
///         let ui = MyPluginUI::new().unwrap();
///         // ... setup ...
///         ui
///     },
///     Some(Arc::new({
///         let params = params.clone();
///         move |ui: &MyPluginUI| {
///             ui.set_gain(params.gain.value());
///             // ... update other params ...
///         }
///     })),
/// )
/// ```
pub fn create_slint_editor_with_param_callback<C, F>(
    slint_state: Arc<SlintState>,
    component_factory: F,
    on_param_values_changed: Option<editor::ParamChangedCallback<C>>,
) -> Option<Box<dyn Editor>>
where
    C: slint::ComponentHandle + 'static,
    F: Fn(Arc<dyn GuiContext>, SlintMouseControl) -> C + Send + Sync + 'static,
{
    Some(Box::new(editor::SlintEditor {
        slint_state,
        component_factory: Arc::new(component_factory),

        // TODO: We can't get the size of the window when baseview does its own scaling, so if the
        //       host does not set a scale factor on Windows or Linux we should just use a factor of
        //       1. That may make the GUI tiny but it also prevents it from getting cut off.
        #[cfg(target_os = "macos")]
        scaling_factor: AtomicCell::new(None),
        #[cfg(not(target_os = "macos"))]
        scaling_factor: AtomicCell::new(Some(1.0)),

        on_param_values_changed,
        emit_parameters_changed_event: Arc::new(AtomicBool::new(false)),
    }))
}

/// State for a `nih_plug_slint` editor.
#[derive(Debug, Serialize, Deserialize)]
pub struct SlintState {
    /// The window's size in logical pixels before applying `user_scale_factor`.
    #[serde(with = "nih_plug::params::persist::serialize_atomic_cell")]
    size: AtomicCell<(u32, u32)>,

    /// A user scale factor that can be applied on top of system DPI scaling.
    /// This allows users to zoom the GUI independently of HiDPI settings.
    /// Defaults to 1.0 (no additional scaling).
    #[serde(with = "nih_plug::params::persist::serialize_atomic_cell")]
    user_scale_factor: AtomicCell<f64>,

    /// Whether the editor's window is currently open.
    #[serde(skip)]
    open: AtomicBool,
}

impl<'a> PersistentField<'a, SlintState> for Arc<SlintState> {
    fn set(&self, new_value: SlintState) {
        self.size.store(new_value.size.load());
        self.user_scale_factor.store(new_value.user_scale_factor.load());
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&SlintState) -> R,
    {
        f(self)
    }
}

impl SlintState {
    /// Initialize the GUI's state. This value can be passed to [`create_slint_editor()`]. The window
    /// size is in logical pixels, so before it is multiplied by the DPI scaling factor.
    pub fn from_size(width: u32, height: u32) -> Arc<SlintState> {
        Arc::new(SlintState {
            size: AtomicCell::new((width, height)),
            user_scale_factor: AtomicCell::new(1.0),
            open: AtomicBool::new(false),
        })
    }

    /// Initialize the GUI's state with a custom initial user scale factor.
    /// The window size is in logical pixels before any scaling is applied.
    pub fn from_size_with_scale(width: u32, height: u32, user_scale_factor: f64) -> Arc<SlintState> {
        Arc::new(SlintState {
            size: AtomicCell::new((width, height)),
            user_scale_factor: AtomicCell::new(user_scale_factor),
            open: AtomicBool::new(false),
        })
    }

    /// Returns a `(width, height)` pair for the current size of the GUI in logical pixels,
    /// after applying the user scale factor. This is the size reported to the host.
    pub fn size(&self) -> (u32, u32) {
        self.scaled_logical_size()
    }

    /// Returns a `(width, height)` pair for the current size of the GUI in logical pixels,
    /// after applying the user scale factor.
    pub fn scaled_logical_size(&self) -> (u32, u32) {
        let (width, height) = self.inner_logical_size();
        let scale = self.user_scale_factor.load();
        (
            (width as f64 * scale).round() as u32,
            (height as f64 * scale).round() as u32,
        )
    }

    /// Returns a `(width, height)` pair for the current size of the GUI in logical pixels,
    /// before applying the user scale factor.
    pub fn inner_logical_size(&self) -> (u32, u32) {
        self.size.load()
    }

    /// Returns the current user scale factor.
    pub fn user_scale_factor(&self) -> f64 {
        self.user_scale_factor.load()
    }

    /// Set the user scale factor. This will be persisted if the state is stored with `#[persist]`.
    /// The change will only take effect the next time the editor is opened.
    pub fn set_user_scale_factor(&self, scale: f64) {
        self.user_scale_factor.store(scale);
    }

    /// Whether the GUI is currently visible.
    // Called `is_open()` instead of `open()` to avoid the ambiguity.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }
}

/// A helper for working with parameters in Slint callbacks. This wraps a [`GuiContext`]
/// and provides convenient methods for parameter manipulation.
pub struct SlintParamContext {
    gui_context: Arc<dyn GuiContext>,
}

impl SlintParamContext {
    /// Create a new parameter context from a [`GuiContext`].
    pub fn new(gui_context: Arc<dyn GuiContext>) -> Self {
        Self { gui_context }
    }

    /// Create a [`ParamSetter`] for setting parameter values.
    pub fn setter(&self) -> ParamSetter<'_> {
        ParamSetter::new(self.gui_context.as_ref())
    }

    /// Get a reference to the underlying [`GuiContext`].
    pub fn gui_context(&self) -> &Arc<dyn GuiContext> {
        &self.gui_context
    }
}

impl Clone for SlintParamContext {
    fn clone(&self) -> Self {
        Self {
            gui_context: self.gui_context.clone(),
        }
    }
}
