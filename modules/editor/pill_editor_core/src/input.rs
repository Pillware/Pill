//! Turns the editor's own window events into one frame of egui input.
//!
//! # Responsibilities
//!
//! - [`raw_input`]: builds one frame of [`RawInput`] from the events the
//!   frontend collected for the shell, plus the shell's geometry.
//! - [`map_key`], [`pointer_button`] and [`modifiers`]: the pure conversions
//!   it composes. Each maps one engine value (a key, a mouse button, the
//!   modifier state) to its egui counterpart.
//!
//! # Design
//!
//! The shell reads [`EguiShellInput::events`] and never the game's `Input`.
//! A key typed into an Inspector field belongs to the UI, and a project
//! system must not see it. The frontend sorts window events by window. The
//! editor's own windows fill this list, and the game's window fills the
//! engine's input. This bridge then replays the list as egui events. It
//! carries the pointer position and the modifier state forward as it goes,
//! so each event reports the state that was current at its place in the
//! list.

// External crates
use egui::{Event, ImeEvent, Modifiers, MouseWheelUnit, Pos2, RawInput, Rect, Vec2};
use pill_core::math::Vector2f;
use pill_engine::input::{
    ButtonState, InputEvent, KeyCode, Modifiers as EngineModifiers, MouseButton, ScrollDelta,
};

// Current crate
use crate::boundary::EguiShellInput;

// =============================================================================
// Types
// =============================================================================

/// The state the bridge carries from one event to the next while it replays
/// the shell's list.
///
/// egui's pointer events carry their own position and modifier state, but the
/// engine reports those as separate events. This type remembers the last value
/// of each, so a button press reports where the pointer was and which modifier
/// keys were held at that point in the list.
#[derive(Default)]
struct EventReplayState {
    /// The modifier state the last `ModifiersChanged` event reported.
    modifiers: Modifiers,
    /// The pointer's last known position, in logical points.
    pointer_position: Option<Pos2>,
}

impl EventReplayState {
    /// Turns `event` into its egui events and appends them to `egui_events`.
    ///
    /// The pointer position and modifier state are updated as it goes, so the
    /// next event is reported with them. Events the shell has no use for (raw
    /// motion, gamepads, focus) append nothing.
    fn push_egui_events(
        &mut self,
        event: &InputEvent,
        scale_factor: f32,
        egui_events: &mut Vec<Event>,
    ) {
        let to_logical_position =
            |position: Vector2f| Pos2::new(position.x / scale_factor, position.y / scale_factor);
        match event {
            InputEvent::ModifiersChanged {
                modifiers: engine_modifiers,
            } => {
                self.modifiers = modifiers(*engine_modifiers);
            }
            InputEvent::CursorMoved { position } => {
                let logical_position = to_logical_position(*position);
                self.pointer_position = Some(logical_position);
                egui_events.push(Event::PointerMoved(logical_position));
            }
            InputEvent::CursorLeft => {
                self.pointer_position = None;
                egui_events.push(Event::PointerGone);
            }
            InputEvent::MouseButton {
                button: engine_button,
                state: button_state,
            } => {
                // Without a position egui cannot attribute the press to a
                // widget, so a button event before any move is dropped.
                let (Some(position), Some(pointer)) =
                    (self.pointer_position, pointer_button(*engine_button))
                else {
                    return;
                };
                egui_events.push(Event::PointerButton {
                    pos: position,
                    button: pointer,
                    pressed: *button_state == ButtonState::Pressed,
                    modifiers: self.modifiers,
                });
            }
            InputEvent::MouseWheel {
                delta: scroll_delta,
            } => {
                let (unit, delta) = match scroll_delta {
                    ScrollDelta::Lines(lines) => {
                        (MouseWheelUnit::Line, Vec2::new(lines.x, lines.y))
                    }
                    ScrollDelta::Pixels(pixels) => (
                        MouseWheelUnit::Point,
                        Vec2::new(pixels.x / scale_factor, pixels.y / scale_factor),
                    ),
                };
                egui_events.push(Event::MouseWheel {
                    unit,
                    delta,
                    modifiers: self.modifiers,
                    phase: egui::TouchPhase::Move,
                });
            }
            InputEvent::Key {
                key,
                state: button_state,
            } => {
                if let Some(key) = map_key(*key) {
                    egui_events.push(Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed: *button_state == ButtonState::Pressed,
                        repeat: false,
                        modifiers: self.modifiers,
                    });
                }
            }
            InputEvent::KeyRepeat { key } => {
                if let Some(key) = map_key(*key) {
                    egui_events.push(Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed: true,
                        repeat: true,
                        modifiers: self.modifiers,
                    });
                }
            }
            InputEvent::TextInput { text } => egui_events.push(Event::Text(text.clone())),
            InputEvent::ImePreedit { text, .. } => {
                egui_events.push(Event::Ime(ImeEvent::Preedit {
                    text: text.clone(),
                    active_range_chars: None,
                }));
            }
            InputEvent::ImeCommit { text } => {
                egui_events.push(Event::Ime(ImeEvent::Commit(text.clone())));
            }
            // Raw motion, focus and gamepads are not for the shell. The game
            // receives them through its own input, and the editor's cameras
            // read the pointer themselves.
            InputEvent::MouseMotion { .. }
            | InputEvent::FocusLost
            | InputEvent::GamepadConnected { .. }
            | InputEvent::GamepadDisconnected { .. }
            | InputEvent::GamepadButton { .. }
            | InputEvent::GamepadAxis { .. } => {}
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds one frame of egui input from the shell's events and geometry.
///
/// The engine reports the cursor position and pixel scroll deltas in physical
/// pixels, so they are divided by `shell_input.scale_factor`. The screen size
/// is already in logical points and is used as it is.
///
/// A scale factor that is zero or negative falls back to one, so the divisions
/// stay finite.
pub fn raw_input(shell_input: &EguiShellInput) -> RawInput {
    let scale_factor = if shell_input.scale_factor > 0.0 {
        shell_input.scale_factor
    } else {
        1.0
    };
    // Step 1: build the state the events are replayed against. The pointer
    // starts where the frontend last saw it, in logical points. The modifier
    // state starts empty, because the frontend sends the current one first.
    let mut event_state = EventReplayState {
        modifiers: Modifiers::default(),
        pointer_position: shell_input
            .cursor
            .map(|position| Pos2::new(position.x / scale_factor, position.y / scale_factor)),
    };
    // Step 2: replay the events, in the order they happened.
    let mut egui_events = Vec::new();
    for event in &shell_input.events {
        event_state.push_egui_events(event, scale_factor, &mut egui_events);
    }
    // Step 3: the OS clipboard's text. egui pastes only from an `Event::Paste`,
    // and the clipboard is not one of the window events, so the frontend reads
    // it once per paste chord and the text is appended here.
    if let Some(text) = shell_input.paste_text.as_ref() {
        egui_events.push(Event::Paste(text.clone()));
    }

    RawInput {
        screen_rect: Some(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(shell_input.screen_size.x, shell_input.screen_size.y),
        )),
        time: Some(shell_input.time),
        focused: shell_input.focused,
        events: egui_events,
        ..Default::default()
    }
}

/// Returns the egui pointer button for an engine mouse button.
///
/// The side buttons (`Back` and `Forward`) have no mapping, so they return
/// `None` and their presses never reach egui.
pub fn pointer_button(button: MouseButton) -> Option<egui::PointerButton> {
    Some(match button {
        MouseButton::Left => egui::PointerButton::Primary,
        MouseButton::Right => egui::PointerButton::Secondary,
        MouseButton::Middle => egui::PointerButton::Middle,
        _ => return None,
    })
}

/// Copies the engine's modifier state into egui's.
///
/// Ctrl becomes both `ctrl` and `command`, because egui's `command` means the
/// platform's shortcut key. The engine's Super key is dropped, since egui's
/// `Modifiers` has no field for it.
pub fn modifiers(engine_modifiers: EngineModifiers) -> Modifiers {
    Modifiers {
        alt: engine_modifiers.alt,
        ctrl: engine_modifiers.control,
        shift: engine_modifiers.shift,
        mac_cmd: false,
        command: engine_modifiers.control,
    }
}

/// Returns the egui key for an engine key, or `None` for a key egui does not name.
pub fn map_key(key: KeyCode) -> Option<egui::Key> {
    use egui::Key;
    Some(match key {
        KeyCode::KeyA => Key::A,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyG => Key::G,
        KeyCode::KeyH => Key::H,
        KeyCode::KeyI => Key::I,
        KeyCode::KeyJ => Key::J,
        KeyCode::KeyK => Key::K,
        KeyCode::KeyL => Key::L,
        KeyCode::KeyM => Key::M,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyO => Key::O,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyT => Key::T,
        KeyCode::KeyU => Key::U,
        KeyCode::KeyV => Key::V,
        KeyCode::KeyW => Key::W,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyY => Key::Y,
        KeyCode::KeyZ => Key::Z,
        KeyCode::Digit0 => Key::Num0,
        KeyCode::Digit1 => Key::Num1,
        KeyCode::Digit2 => Key::Num2,
        KeyCode::Digit3 => Key::Num3,
        KeyCode::Digit4 => Key::Num4,
        KeyCode::Digit5 => Key::Num5,
        KeyCode::Digit6 => Key::Num6,
        KeyCode::Digit7 => Key::Num7,
        KeyCode::Digit8 => Key::Num8,
        KeyCode::Digit9 => Key::Num9,
        KeyCode::Numpad0 => Key::Num0,
        KeyCode::Numpad1 => Key::Num1,
        KeyCode::Numpad2 => Key::Num2,
        KeyCode::Numpad3 => Key::Num3,
        KeyCode::Numpad4 => Key::Num4,
        KeyCode::Numpad5 => Key::Num5,
        KeyCode::Numpad6 => Key::Num6,
        KeyCode::Numpad7 => Key::Num7,
        KeyCode::Numpad8 => Key::Num8,
        KeyCode::Numpad9 => Key::Num9,
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,
        KeyCode::F13 => Key::F13,
        KeyCode::F14 => Key::F14,
        KeyCode::F15 => Key::F15,
        KeyCode::F16 => Key::F16,
        KeyCode::F17 => Key::F17,
        KeyCode::F18 => Key::F18,
        KeyCode::F19 => Key::F19,
        KeyCode::F20 => Key::F20,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Space => Key::Space,
        KeyCode::Tab => Key::Tab,
        KeyCode::Escape => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Minus | KeyCode::NumpadSubtract => Key::Minus,
        KeyCode::Equal => Key::Equals,
        KeyCode::Comma | KeyCode::NumpadDecimal => Key::Comma,
        KeyCode::Period => Key::Period,
        KeyCode::Slash => Key::Slash,
        KeyCode::Backslash => Key::Backslash,
        KeyCode::Semicolon => Key::Semicolon,
        KeyCode::Quote => Key::Quote,
        KeyCode::BracketLeft => Key::OpenBracket,
        KeyCode::BracketRight => Key::CloseBracket,
        _ => return None,
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_egui() {
        assert_eq!(map_key(KeyCode::KeyA), Some(egui::Key::A));
        assert_eq!(map_key(KeyCode::Digit7), Some(egui::Key::Num7));
        assert_eq!(map_key(KeyCode::ArrowUp), Some(egui::Key::ArrowUp));
        assert_eq!(map_key(KeyCode::F12), Some(egui::Key::F12));
        assert_eq!(map_key(KeyCode::LaunchMail), None);
    }

    #[test]
    fn modifiers_map_to_egui() {
        let engine_modifiers = EngineModifiers {
            shift: true,
            control: true,
            alt: false,
            super_key: true,
        };
        let egui_modifiers = modifiers(engine_modifiers);
        assert!(egui_modifiers.shift);
        assert!(egui_modifiers.ctrl);
        assert!(egui_modifiers.command);
        assert!(!egui_modifiers.alt);
    }

    #[test]
    fn raw_input_carries_geometry_and_replays_the_events() {
        let mut shell_input = EguiShellInput {
            screen_size: Vector2f::new(800.0, 600.0),
            scale_factor: 2.0,
            focused: true,
            time: 12.5,
            frames_per_second: 117.0,
            ..Default::default()
        };
        let egui_input = raw_input(&shell_input);
        assert_eq!(
            egui_input.screen_rect,
            Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)))
        );
        assert_eq!(egui_input.time, Some(12.5));
        assert!(egui_input.focused);
        // The shell reads its own event list, so a frame with no events has
        // no input at all. Nothing is filled in from the game's state.
        assert!(egui_input.events.is_empty());

        shell_input.events = vec![InputEvent::CursorLeft];
        let egui_input = raw_input(&shell_input);
        assert!(matches!(egui_input.events.as_slice(), [Event::PointerGone]));
    }

    #[test]
    fn a_key_carries_the_modifiers_in_force_when_it_happened() {
        let shell_input = EguiShellInput {
            events: vec![
                InputEvent::ModifiersChanged {
                    modifiers: EngineModifiers {
                        control: true,
                        ..Default::default()
                    },
                },
                InputEvent::Key {
                    key: KeyCode::KeyD,
                    state: ButtonState::Pressed,
                },
                InputEvent::ModifiersChanged {
                    modifiers: EngineModifiers::default(),
                },
                InputEvent::Key {
                    key: KeyCode::KeyD,
                    state: ButtonState::Released,
                },
            ],
            ..Default::default()
        };
        let egui_input = raw_input(&shell_input);
        let [Event::Key {
            modifiers, pressed, ..
        }, Event::Key {
            modifiers: released_modifiers,
            pressed: released,
            ..
        }] = egui_input.events.as_slice()
        else {
            panic!("expected two key events, got {:?}", egui_input.events);
        };
        assert!(modifiers.ctrl);
        assert!(pressed);
        assert!(!released_modifiers.ctrl);
        assert!(!released);
    }

    #[test]
    fn a_button_press_lands_where_the_pointer_was() {
        let shell_input = EguiShellInput {
            scale_factor: 2.0,
            events: vec![
                InputEvent::CursorMoved {
                    position: Vector2f::new(200.0, 100.0),
                },
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: ButtonState::Pressed,
                },
            ],
            ..Default::default()
        };
        let egui_input = raw_input(&shell_input);
        let [Event::PointerMoved(moved), Event::PointerButton {
            pos: position,
            button: pointer,
            pressed,
            ..
        }] = egui_input.events.as_slice()
        else {
            panic!("expected a move and a press, got {:?}", egui_input.events);
        };
        assert_eq!(*moved, Pos2::new(100.0, 50.0));
        assert_eq!(*position, Pos2::new(100.0, 50.0));
        assert_eq!(*pointer, egui::PointerButton::Primary);
        assert!(pressed);
    }

    #[test]
    fn a_button_before_any_move_is_dropped() {
        let shell_input = EguiShellInput {
            events: vec![InputEvent::MouseButton {
                button: MouseButton::Left,
                state: ButtonState::Released,
            }],
            ..Default::default()
        };
        assert!(raw_input(&shell_input).events.is_empty());
    }

    #[test]
    fn a_seeded_cursor_places_a_release_that_has_no_move() {
        let shell_input = EguiShellInput {
            scale_factor: 2.0,
            cursor: Some(Vector2f::new(200.0, 100.0)),
            events: vec![InputEvent::MouseButton {
                button: MouseButton::Left,
                state: ButtonState::Released,
            }],
            ..Default::default()
        };
        let egui_input = raw_input(&shell_input);
        let [Event::PointerButton {
            pos: position,
            pressed,
            ..
        }] = egui_input.events.as_slice()
        else {
            panic!("expected one release, got {:?}", egui_input.events);
        };
        assert_eq!(*position, Pos2::new(100.0, 50.0));
        assert!(!pressed);
    }

    #[test]
    fn text_and_ime_commit_forward_to_egui() {
        let shell_input = EguiShellInput {
            events: vec![
                InputEvent::TextInput {
                    text: "zzz".to_owned(),
                },
                InputEvent::ImePreedit {
                    text: "zo".to_owned(),
                    cursor: None,
                },
                InputEvent::ImeCommit {
                    text: "zoo".to_owned(),
                },
            ],
            ..Default::default()
        };
        let egui_input = raw_input(&shell_input);
        assert_eq!(
            egui_input.events,
            vec![
                Event::Text("zzz".to_owned()),
                Event::Ime(ImeEvent::Preedit {
                    text: "zo".to_owned(),
                    active_range_chars: None,
                }),
                Event::Ime(ImeEvent::Commit("zoo".to_owned())),
            ]
        );
    }
}
