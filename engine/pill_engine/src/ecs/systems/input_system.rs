use crate::{
    engine::Engine,
    ecs::{ InputComponent, InputEvent, GamepadAxis, GamepadButton },
};

use pill_core::{ Vector2f };

use anyhow::{ Result, Context, Error };
use winit::event::{ ElementState, MouseScrollDelta };

// use a lazy static GILRS instance
use gilrs::{ Gilrs, EventType, Axis, Button };
use once_cell::sync::Lazy;
use std::sync::Mutex;

static GILRS: Lazy<Mutex<Gilrs>> = Lazy::new(|| Mutex::new(Gilrs::new().expect("Failed to initialize Gilrs")));

pub fn input_system(engine: &mut Engine) -> Result<()> {
    // Poll GILRS first
    {
        let mut gilrs = GILRS.lock().unwrap();
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) => engine.input_queue.push_back(InputEvent::GamepadButton { id: usize::from(ev.id), button: b.into(), state: ElementState::Pressed }),
                EventType::ButtonRepeated(b, _) => engine.input_queue.push_back(InputEvent::GamepadButton { id: usize::from(ev.id), button: b.into(), state: ElementState::Pressed }), // TODO: this should be handled differently?
                EventType::ButtonReleased(b, _) => engine.input_queue.push_back(InputEvent::GamepadButton { id: usize::from(ev.id), button: b.into(), state: ElementState::Released }),
                EventType::AxisChanged(a, v, _) => engine.input_queue.push_back(InputEvent::GamepadAxis { id: usize::from(ev.id), axis: a.into(), value: v }),
                EventType::Connected => engine.input_queue.push_back(InputEvent::GamepadConnected { id: usize::from(ev.id) }),
                EventType::Disconnected => engine.input_queue.push_back(InputEvent::GamepadDisconnected { id: usize::from(ev.id) }),
                EventType::ForceFeedbackEffectCompleted => {}, // ignore // TODO: what to do?
                _ => {},
            }
        }
    }

    {
        let input_component = engine.get_global_component_mut::<InputComponent>()?;
        input_component.clear_transient_states();
    }

    while let Some(event) = engine.input_queue.pop_front() {
        let input_component = engine.get_global_component_mut::<InputComponent>()?;

        match event {
            // Keyboard keys
            InputEvent::KeyboardKey { key, state } => {
                input_component.set_key(key, state);
            },

            // Mouse buttons
            InputEvent::MouseButton {key, state} => {
                input_component.set_mouse_button(key, state);
            },

            // Mouse scroll
            InputEvent::MouseWheel { delta } => {
                match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        input_component.set_mouse_scroll_delta(Vector2f::new(x, y));
                    },

                    MouseScrollDelta::PixelDelta(delta) => {
                        input_component.set_mouse_scroll_pixel_delta(Vector2f::new(delta.x as f32, delta.y as f32));
                    },
                }
            },

            // Mouse delta
            InputEvent::MouseDelta { delta } => {
                input_component.set_mouse_delta(delta);
            },

            // Mouse position
            InputEvent::MousePosition { position} => {
                input_component.set_mouse_position(position);
            },

            // Gamepad buttons
            InputEvent::GamepadButton { id, button, state } => {
                input_component.set_gamepad_button(id, button, state);
            },

            // Gamepad axes
            InputEvent::GamepadAxis { id, axis, value } => {
                input_component.set_gamepad_axis(id, axis, value);
            },

            // Gamepad connection events
            InputEvent::GamepadConnected { id } => {
                input_component.connect_gamepad(id);
            },

            InputEvent::GamepadDisconnected { id } => {
                input_component.disconnect_gamepad(id);
            },
        }
    }

    Ok(())
}

// GILRS -> input enum conversions
impl From<Button> for GamepadButton {
    fn from(button: Button) -> Self {
        match button {
            // ABXY
            Button::South => GamepadButton::A,
            Button::East => GamepadButton::B,
            Button::North => GamepadButton::X,
            Button::West => GamepadButton::Y,
            // Triggers and bumpers
            Button::LeftTrigger => GamepadButton::LeftBumper,
            Button::LeftTrigger2 => GamepadButton::LeftTrigger,
            Button::RightTrigger => GamepadButton::RightBumper,
            Button::RightTrigger2 => GamepadButton::RightTrigger,
            // Menus
            Button::Select => GamepadButton::Back,
            Button::Start => GamepadButton::Start,
            Button::Mode => GamepadButton::Mode,
            // DPad
            Button::DPadUp => GamepadButton::DPadUp,
            Button::DPadDown => GamepadButton::DPadDown,
            Button::DPadLeft => GamepadButton::DPadLeft,
            Button::DPadRight => GamepadButton::DPadRight,
            // Sticks
            Button::LeftThumb => GamepadButton::LeftStick,
            Button::RightThumb => GamepadButton::RightStick,
            _ => GamepadButton::Mode, // Handle other buttons as Mode
        }
    }
}

impl From<Axis> for GamepadAxis {
    fn from(axis: Axis) -> Self {
        match axis {
            Axis::LeftStickX => GamepadAxis::LeftStickX,
            Axis::LeftStickY => GamepadAxis::LeftStickY,
            Axis::RightStickX => GamepadAxis::RightStickX,
            Axis::RightStickY => GamepadAxis::RightStickY,
            Axis::LeftZ => GamepadAxis::LeftTrigger,
            Axis::RightZ => GamepadAxis::RightTrigger,
            _ => GamepadAxis::LeftStickX, // Handle other axes as LeftStickX
        }
    }
}
