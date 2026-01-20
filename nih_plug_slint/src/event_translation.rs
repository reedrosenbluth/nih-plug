//! Event translation from baseview events to Slint WindowEvents.

use keyboard_types::{Key, KeyState, KeyboardEvent};
use slint::platform::WindowEvent;
use slint::{LogicalPosition, LogicalSize};

/// Translates a baseview event to a Slint WindowEvent.
/// Returns `None` if the event doesn't have a corresponding Slint event.
pub fn translate_event(event: &baseview::Event, scale_factor: f32) -> Option<WindowEvent> {
    match event {
        baseview::Event::Mouse(mouse_event) => translate_mouse_event(mouse_event, scale_factor),
        baseview::Event::Keyboard(keyboard_event) => translate_keyboard_event(keyboard_event),
        baseview::Event::Window(window_event) => translate_window_event(window_event, scale_factor),
    }
}

fn translate_mouse_event(event: &baseview::MouseEvent, scale_factor: f32) -> Option<WindowEvent> {
    match event {
        baseview::MouseEvent::CursorMoved {
            position,
            modifiers: _,
        } => Some(WindowEvent::PointerMoved {
            position: LogicalPosition::new(
                position.x as f32 / scale_factor,
                position.y as f32 / scale_factor,
            ),
        }),
        baseview::MouseEvent::ButtonPressed { button, modifiers: _ } => {
            let slint_button = translate_mouse_button(*button)?;
            Some(WindowEvent::PointerPressed {
                position: LogicalPosition::default(), // Position will be set from last move
                button: slint_button,
            })
        }
        baseview::MouseEvent::ButtonReleased { button, modifiers: _ } => {
            let slint_button = translate_mouse_button(*button)?;
            Some(WindowEvent::PointerReleased {
                position: LogicalPosition::default(), // Position will be set from last move
                button: slint_button,
            })
        }
        baseview::MouseEvent::WheelScrolled { delta, modifiers: _ } => {
            let (delta_x, delta_y) = match delta {
                baseview::ScrollDelta::Lines { x, y } => {
                    // Convert lines to pixels (typical line height)
                    (*x as f32 * 20.0, *y as f32 * 20.0)
                }
                baseview::ScrollDelta::Pixels { x, y } => (*x as f32, *y as f32),
            };
            Some(WindowEvent::PointerScrolled {
                position: LogicalPosition::default(),
                delta_x,
                delta_y,
            })
        }
        baseview::MouseEvent::CursorEntered => Some(WindowEvent::PointerMoved {
            position: LogicalPosition::default(),
        }),
        baseview::MouseEvent::CursorLeft => Some(WindowEvent::PointerExited),
        // Drag and drop events - not currently supported by Slint
        baseview::MouseEvent::DragEntered { .. }
        | baseview::MouseEvent::DragMoved { .. }
        | baseview::MouseEvent::DragLeft
        | baseview::MouseEvent::DragDropped { .. } => None,
    }
}

fn translate_mouse_button(button: baseview::MouseButton) -> Option<slint::platform::PointerEventButton> {
    match button {
        baseview::MouseButton::Left => Some(slint::platform::PointerEventButton::Left),
        baseview::MouseButton::Right => Some(slint::platform::PointerEventButton::Right),
        baseview::MouseButton::Middle => Some(slint::platform::PointerEventButton::Middle),
        baseview::MouseButton::Back => Some(slint::platform::PointerEventButton::Other),
        baseview::MouseButton::Forward => Some(slint::platform::PointerEventButton::Other),
        baseview::MouseButton::Other(_) => Some(slint::platform::PointerEventButton::Other),
    }
}

fn translate_keyboard_event(event: &KeyboardEvent) -> Option<WindowEvent> {
    let text = key_to_text(&event.key);
    match event.state {
        KeyState::Down => Some(WindowEvent::KeyPressed { text: text.into() }),
        KeyState::Up => Some(WindowEvent::KeyReleased { text: text.into() }),
    }
}

fn key_to_text(key: &Key) -> String {

    match key {
        Key::Character(s) => s.clone(),
        Key::Enter => "\n".to_string(),
        Key::Tab => "\t".to_string(),
        Key::Backspace => "\u{0008}".to_string(), // Backspace character
        Key::Delete => "\u{007F}".to_string(),    // Delete character
        Key::Escape => "\u{001B}".to_string(),    // Escape character
        Key::ArrowUp => String::new(),            // Special keys don't produce text
        Key::ArrowDown => String::new(),
        Key::ArrowLeft => String::new(),
        Key::ArrowRight => String::new(),
        Key::Home => String::new(),
        Key::End => String::new(),
        Key::PageUp => String::new(),
        Key::PageDown => String::new(),
        _ => String::new(),
    }
}

fn translate_window_event(event: &baseview::WindowEvent, _scale_factor: f32) -> Option<WindowEvent> {
    match event {
        baseview::WindowEvent::Resized(window_info) => {
            let logical_size = window_info.logical_size();
            Some(WindowEvent::Resized {
                size: LogicalSize::new(logical_size.width as f32, logical_size.height as f32),
            })
        }
        baseview::WindowEvent::Focused => Some(WindowEvent::WindowActiveChanged(true)),
        baseview::WindowEvent::Unfocused => Some(WindowEvent::WindowActiveChanged(false)),
        baseview::WindowEvent::WillClose => Some(WindowEvent::CloseRequested),
    }
}
