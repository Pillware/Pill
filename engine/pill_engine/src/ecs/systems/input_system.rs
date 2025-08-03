#![cfg(feature = "rendering")]

use crate::{
    engine::Engine,
    ecs::{InputComponent, InputEvent, GamepadAxis, GamepadButton},
};

use pill_core::Vector2f;
use anyhow::Result;
use winit::event::{ElementState, MouseScrollDelta};

// gilrs + a lazy static so we keep it alive across frames
use gilrs::{Gilrs, EventType, Button, Axis};
use once_cell::sync::Lazy;
use std::sync::Mutex;

static GILRS: Lazy<Mutex<Gilrs>> =
    Lazy::new(|| Mutex::new(Gilrs::new().expect("failed to init gilrs")));

pub fn input_system(engine: &mut Engine) -> Result<()> {
    // ── 1 · poll gilrs and push fresh events into the queue ───────────────────
    {
        let mut gilrs = GILRS.lock().unwrap();
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) => engine.input_queue.push_back(
                    InputEvent::GamepadButton { button: b.into(), state: ElementState::Pressed }),
                EventType::ButtonReleased(b, _) => engine.input_queue.push_back(
                    InputEvent::GamepadButton { button: b.into(), state: ElementState::Released }),
                EventType::AxisChanged(a, v, _) => engine.input_queue.push_back(
                    InputEvent::GamepadAxis { axis: a.into(), value: v }),
                _ => {}
            }
        }
    }

    // ── 2 · clear one-frame flags on InputComponent ───────────────────────────
    let input_comp = engine.get_global_component_mut::<InputComponent>()?;
    input_comp.clear_transients();
    drop(input_comp); // release borrow so we can mut-borrow again later

    // ── 3 · consume the queue ─────────────────────────────────────────────────
    while let Some(event) = engine.input_queue.pop_front() {
        let input = engine.get_global_component_mut::<InputComponent>()?;

        match event {
            // keyboard ---------------------------------------------------------
            InputEvent::KeyboardKey { key, state } =>
                input.set_key(key, state),

            // mouse ------------------------------------------------------------
            InputEvent::MouseButton { key, state } =>
                input.set_mouse_button(key, state),

            InputEvent::MouseWheel { delta } => match delta {
                MouseScrollDelta::LineDelta(x, y) =>
                    input.set_mouse_scroll_delta(Vector2f::new(x, y)),
                MouseScrollDelta::PixelDelta(d) =>
                    input.set_mouse_scroll_pixel_delta(Vector2f::new(d.x as f32, d.y as f32)),
            },

            InputEvent::MouseDelta { delta } =>
                input.set_mouse_delta(delta),
            InputEvent::MousePosition { position } =>
                input.set_mouse_position(position),

            // game-pad ---------------------------------------------------------
            InputEvent::GamepadButton { button, state } =>
                input.set_gamepad_button(button, state),
            InputEvent::GamepadAxis { axis, value } =>
                input.set_gamepad_axis(axis, value),
        }
    }

    Ok(())
}

// ───────── gilrs → internal enum conversions ─────────
impl From<Button> for GamepadButton {
    fn from(b: Button) -> Self {
        match b {
            // ABXY (now South-East-West-North)
            Button::South        => GamepadButton::A,
            Button::East         => GamepadButton::B,
            Button::West         => GamepadButton::X,
            Button::North        => GamepadButton::Y,

            // bumpers (both Trigger & Trigger2 map here)
            Button::LeftTrigger  | Button::LeftTrigger2  => GamepadButton::LeftBumper,
            Button::RightTrigger | Button::RightTrigger2 => GamepadButton::RightBumper,

            // centre buttons
            Button::Select       => GamepadButton::Back,
            Button::Start        => GamepadButton::Start,
            Button::Mode         => GamepadButton::Guide,

            // sticks
            Button::LeftThumb    => GamepadButton::LeftStick,
            Button::RightThumb   => GamepadButton::RightStick,

            // d-pad
            Button::DPadUp       => GamepadButton::DPadUp,
            Button::DPadDown     => GamepadButton::DPadDown,
            Button::DPadLeft     => GamepadButton::DPadLeft,
            Button::DPadRight    => GamepadButton::DPadRight,

            // fallback
            _ => GamepadButton::Guide,
        }
    }
}
impl From<Axis> for GamepadAxis {
    fn from(a: Axis) -> Self {
        use Axis::*;
        match a {
            LeftStickX  => GamepadAxis::LeftStickX,
            LeftStickY  => GamepadAxis::LeftStickY,
            RightStickX => GamepadAxis::RightStickX,
            RightStickY => GamepadAxis::RightStickY,
            LeftZ       => GamepadAxis::LeftTrigger,
            RightZ      => GamepadAxis::RightTrigger,
            _           => GamepadAxis::LeftStickX,
        }
    }
}

