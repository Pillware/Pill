use crate::{
    engine::Engine,
    ecs::{ InputComponent, InputEvent, GamepadAxis, GamepadButton, HapticCommand, InFlight },
};

use pill_core::{ Vector2f };

use anyhow::{ Result, Context, Error };
use winit::event::{ ElementState, MouseScrollDelta };

// use a lazy static GILRS instance
use gilrs::{ Gilrs, EventType, Axis, Button, GamepadId };
use gilrs::ff::{Effect, EffectBuilder, BaseEffect, BaseEffectType, Replay, Ticks};
use once_cell::sync::Lazy;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

pub(crate) static GILRS: Lazy<Mutex<Gilrs>> = Lazy::new(|| Mutex::new(Gilrs::new().expect("Failed to initialize Gilrs")));

pub fn input_system(engine: &mut Engine) -> Result<()> {
    // Poll GILRS first
    {
        let mut gilrs_input_system = GILRS.lock().unwrap();
        while let Some(ev) = gilrs_input_system.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) => engine.input_queue.push_back(InputEvent::GamepadButton { id: ev.id, button: b.into(), state: ElementState::Pressed }),
                EventType::ButtonRepeated(b, _) => engine.input_queue.push_back(InputEvent::GamepadButton { id: ev.id, button: b.into(), state: ElementState::Pressed }), // TODO: do we want to treat repeated press differently?
                EventType::ButtonReleased(b, _) => engine.input_queue.push_back(InputEvent::GamepadButton { id: ev.id, button: b.into(), state: ElementState::Released }),
                EventType::AxisChanged(a, v, _) => engine.input_queue.push_back(InputEvent::GamepadAxis { id: ev.id, axis: a.into(), value: v }),
                EventType::Connected => engine.input_queue.push_back(InputEvent::GamepadConnected { id: ev.id }),
                EventType::Disconnected => engine.input_queue.push_back(InputEvent::GamepadDisconnected { id: ev.id }),
                EventType::ForceFeedbackEffectCompleted => engine.input_queue.push_back(InputEvent::GamepadForceFeedbackEffectCompleted { id: ev.id }),
                _ => {},
            }
        }
    }

    {
        let input_component = engine.get_global_component_mut::<InputComponent>()?;
        input_component.clear_transient_states();

        // If the input component has just been created, initialize the gamepad states
        if input_component.gamepad_id_to_player.is_empty() {
            let gilrs_input_system = GILRS.lock().unwrap();
            for (id, gamepad) in gilrs_input_system.gamepads() {
                input_component.connect_gamepad(id);
            }
        }
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

            // Gamepad force feedback completion
            InputEvent::GamepadForceFeedbackEffectCompleted { id } => {
                input_component.complete_force_feedback_effect(id);
            },
        }
    }

    Ok(())
}

pub fn haptics_system(engine: &mut Engine) -> Result<()> {
    let mut gilrs_input_system = GILRS.lock().unwrap();

    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Because some controllers (e.g. Xbox360) do not report when an effect has completed
    // Remove any in-flight effects that have completed
    {
        let now = std::time::Instant::now();
        input_component.in_flight_ff.retain(|in_flight| {
            if now >= in_flight.end_at {
                false
            } else {
                true
            }
        });
    }

    while let Some(command) = input_component.haptics.pop_front() {
        let index = command_player_index(&command)?;
        let Some(gamepad_id) = input_component.gamepad_ids.get(index).unwrap() else {
            continue; // No gamepad mapped for this player
        };

        // Is the haptic feedback supported?
        {
            let gamepad = gilrs_input_system.gamepad(*gamepad_id);
            if !gamepad.is_ff_supported() {
                continue;
            }
        }

        match command {
            HapticCommand::Rumble { player_id: _, weak, strong, duration_ms } => {
                let effect = create_rumble_effect(&mut gilrs_input_system, &[*gamepad_id], weak, strong, duration_ms)?;
                effect.play()?;
                let end_at = Instant::now() + Duration::from_millis(duration_ms as u64);
                input_component.in_flight_ff.push(InFlight { id: *gamepad_id, effect, end_at });
            },
            HapticCommand::PlayEffect { player_id: _, effect, duration_ms } => {
                let gamepad = gilrs_input_system.gamepad(*gamepad_id);
                effect.add_gamepad(&gamepad)?;
                effect.play()?;
                let end_at = Instant::now() + Duration::from_millis(duration_ms as u64);
                input_component.in_flight_ff.push(InFlight { id: *gamepad_id, effect, end_at });
            },
        }
    }
    Ok(())
}

#[inline]
fn command_player_index(command: &HapticCommand) -> Result<usize> {
    let pid = match command {
        HapticCommand::Rumble { player_id, .. } |
        HapticCommand::PlayEffect { player_id, .. } => *player_id,
    };
    Ok(pid as usize)
}

fn create_rumble_effect(gilrs_input_system: &mut Gilrs, recipients: &[GamepadId], weak: f32, strong: f32, duration_ms: u32) -> Result<Effect> {
    let dur = Ticks::from_ms(duration_ms);
    let effect = EffectBuilder::new()
        .add_effect(BaseEffect {
            kind: BaseEffectType::Weak { magnitude: (weak.clamp(0.0, 1.0) * 65_535.0) as u16 },
            scheduling: Replay { play_for: dur, ..Default::default() },
            ..Default::default()
        })
        .add_effect(BaseEffect {
            kind: BaseEffectType::Strong { magnitude: (strong.clamp(0.0, 1.0) * 65_535.0) as u16 },
            scheduling: Replay { play_for: dur, ..Default::default() },
            ..Default::default()
        })
        .gamepads(recipients)
        .finish(gilrs_input_system)?;
    Ok(effect)
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
            Axis::DPadX => GamepadAxis::DPadX,
            Axis::DPadY => GamepadAxis::DPadY,
            _ => GamepadAxis::LeftStickX, // Handle other axes as LeftStickX
        }
    }
}
