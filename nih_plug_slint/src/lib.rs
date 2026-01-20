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

pub use slint;

/// Create an [`Editor`] instance using a [Slint](https://slint.dev/) GUI. The [`SlintState`]
/// passed to this function contains the GUI's initial size, and this is kept in sync whenever
/// the GUI gets resized. You can also use this to know if the GUI is open, so you can avoid
/// performing potentially expensive calculations while the GUI is not open. If you want this
/// size to be persisted when restoring a plugin instance, then you can store it in a
/// `#[persist = "key"]` field on your parameters struct.
///
/// The `component_factory` closure receives the [`GuiContext`] wrapped in an [`Arc`], which
/// can be used to create a [`ParamSetter`] for modifying parameter values. The factory is
/// called each time the editor window is opened.
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
///         move |gui_context| {
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
    F: Fn(Arc<dyn GuiContext>) -> C + Send + Sync + 'static,
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
    }))
}

/// State for a `nih_plug_slint` editor.
#[derive(Debug, Serialize, Deserialize)]
pub struct SlintState {
    /// The window's size in logical pixels before applying `scale_factor`.
    #[serde(with = "nih_plug::params::persist::serialize_atomic_cell")]
    size: AtomicCell<(u32, u32)>,

    /// Whether the editor's window is currently open.
    #[serde(skip)]
    open: AtomicBool,
}

impl<'a> PersistentField<'a, SlintState> for Arc<SlintState> {
    fn set(&self, new_value: SlintState) {
        self.size.store(new_value.size.load());
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
            open: AtomicBool::new(false),
        })
    }

    /// Returns a `(width, height)` pair for the current size of the GUI in logical pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size.load()
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
