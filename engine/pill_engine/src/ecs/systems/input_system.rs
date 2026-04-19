use crate::{
    ecs::{
        GamepadAxis, GamepadButton, GamepadEvent, HapticCommand, InputComponent, InputEvent,
        KeyboardEvent, MouseEvent,
    },
    engine::Engine,
};
use pill_core::Vector2f;

use anyhow::Result;
use winit::event::{ElementState, MouseScrollDelta};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use crate::ecs::InFlight;
    use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Replay, Ticks};
    use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
    use once_cell::sync::Lazy;
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };

    pub(crate) static GILRS: Lazy<Mutex<Gilrs>> =
        Lazy::new(|| Mutex::new(Gilrs::new().expect("Failed to initialize Gilrs")));

    pub fn input_system(engine: &mut Engine) -> Result<()> {
        {
            let mut gamepad_input_system = GILRS.lock().unwrap();
            while let Some(event) = gamepad_input_system.next_event() {
                match event.event {
                    EventType::ButtonPressed(b, _) => {
                        engine
                            .input_queue
                            .push_back(InputEvent::Gamepad(GamepadEvent::Button {
                                id: event.id,
                                button: b.into(),
                                state: ElementState::Pressed,
                            }))
                    }
                    EventType::ButtonRepeated(b, _) => {
                        engine
                            .input_queue
                            .push_back(InputEvent::Gamepad(GamepadEvent::Button {
                                id: event.id,
                                button: b.into(),
                                state: ElementState::Pressed,
                            }))
                    }
                    EventType::ButtonReleased(b, _) => {
                        engine
                            .input_queue
                            .push_back(InputEvent::Gamepad(GamepadEvent::Button {
                                id: event.id,
                                button: b.into(),
                                state: ElementState::Released,
                            }))
                    }
                    EventType::AxisChanged(a, v, _) => {
                        engine
                            .input_queue
                            .push_back(InputEvent::Gamepad(GamepadEvent::Axis {
                                id: event.id,
                                axis: a.into(),
                                value: v,
                            }))
                    }
                    EventType::Connected => {
                        engine
                            .input_queue
                            .push_back(InputEvent::Gamepad(GamepadEvent::Connected {
                                id: event.id,
                            }))
                    }
                    EventType::Disconnected => engine.input_queue.push_back(InputEvent::Gamepad(
                        GamepadEvent::Disconnected { id: event.id },
                    )),
                    EventType::ForceFeedbackEffectCompleted => {
                        engine.input_queue.push_back(InputEvent::Gamepad(
                            GamepadEvent::ForceFeedbackEffectCompleted { id: event.id },
                        ))
                    }
                    _ => {}
                }
            }
        }

        {
            let input_component = engine.get_global_component_mut::<InputComponent>()?;
            input_component.clear_transient_states();

            if input_component.gamepad_id_to_player.is_empty() {
                let gamepad_input_system = GILRS.lock().unwrap();
                for (id, _gamepad) in gamepad_input_system.gamepads() {
                    input_component.connect_gamepad(id);
                }
            }
        }

        process_input_queue(engine)
    }

    pub fn haptics_system(engine: &mut Engine) -> Result<()> {
        let mut gamepad_input_system = GILRS.lock().unwrap();

        let input_component = engine.get_global_component_mut::<InputComponent>()?;

        {
            let now = std::time::Instant::now();
            input_component
                .in_flight_force_feedback
                .retain(|in_flight| now < in_flight.end_at);
        }

        while let Some(command) = input_component.haptic_commands.pop_front() {
            let index = command_player_index(&command)?;
            let Some(gamepad_id) = input_component.gamepad_ids.get(index).unwrap() else {
                continue;
            };

            {
                let gamepad = gamepad_input_system.gamepad(*gamepad_id);
                if !gamepad.is_ff_supported() {
                    continue;
                }
            }

            match command {
                HapticCommand::Rumble {
                    player_id: _,
                    weak,
                    strong,
                    duration_ms,
                } => {
                    let effect = create_rumble_effect(
                        &mut gamepad_input_system,
                        &[*gamepad_id],
                        weak,
                        strong,
                        duration_ms,
                    )?;
                    effect.play()?;
                    let end_at = Instant::now() + Duration::from_millis(duration_ms as u64);
                    input_component.in_flight_force_feedback.push(InFlight {
                        id: *gamepad_id,
                        effect,
                        end_at,
                    });
                }
                HapticCommand::PlayEffect {
                    player_id: _,
                    effect,
                    duration_ms,
                } => {
                    let gamepad = gamepad_input_system.gamepad(*gamepad_id);
                    effect.add_gamepad(&gamepad)?;
                    effect.play()?;
                    let end_at = Instant::now() + Duration::from_millis(duration_ms as u64);
                    input_component.in_flight_force_feedback.push(InFlight {
                        id: *gamepad_id,
                        effect,
                        end_at,
                    });
                }
            }
        }
        Ok(())
    }

    #[inline]
    fn command_player_index(command: &HapticCommand) -> Result<usize> {
        let pid = match command {
            HapticCommand::Rumble { player_id, .. }
            | HapticCommand::PlayEffect { player_id, .. } => *player_id,
        };
        Ok(pid as usize)
    }

    fn create_rumble_effect(
        gamepad_input_system: &mut Gilrs,
        recipients: &[GamepadId],
        weak: f32,
        strong: f32,
        duration_ms: u32,
    ) -> Result<Effect> {
        let dur = Ticks::from_ms(duration_ms);
        let effect = EffectBuilder::new()
            .add_effect(BaseEffect {
                kind: BaseEffectType::Weak {
                    magnitude: (weak.clamp(0.0, 1.0) * 65_535.0) as u16,
                },
                scheduling: Replay {
                    play_for: dur,
                    ..Default::default()
                },
                ..Default::default()
            })
            .add_effect(BaseEffect {
                kind: BaseEffectType::Strong {
                    magnitude: (strong.clamp(0.0, 1.0) * 65_535.0) as u16,
                },
                scheduling: Replay {
                    play_for: dur,
                    ..Default::default()
                },
                ..Default::default()
            })
            .gamepads(recipients)
            .finish(gamepad_input_system)?;
        Ok(effect)
    }

    impl From<Button> for GamepadButton {
        fn from(button: Button) -> Self {
            match button {
                Button::South => GamepadButton::A,
                Button::East => GamepadButton::B,
                Button::North => GamepadButton::X,
                Button::West => GamepadButton::Y,
                Button::LeftTrigger => GamepadButton::LeftBumper,
                Button::LeftTrigger2 => GamepadButton::LeftTrigger,
                Button::RightTrigger => GamepadButton::RightBumper,
                Button::RightTrigger2 => GamepadButton::RightTrigger,
                Button::Select => GamepadButton::Back,
                Button::Start => GamepadButton::Start,
                Button::Mode => GamepadButton::Mode,
                Button::DPadUp => GamepadButton::DPadUp,
                Button::DPadDown => GamepadButton::DPadDown,
                Button::DPadLeft => GamepadButton::DPadLeft,
                Button::DPadRight => GamepadButton::DPadRight,
                Button::LeftThumb => GamepadButton::LeftStick,
                Button::RightThumb => GamepadButton::RightStick,
                _ => GamepadButton::Mode,
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
                _ => GamepadAxis::LeftStickX,
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;

    pub fn input_system(engine: &mut Engine) -> Result<()> {
        {
            let input_component = engine.get_global_component_mut::<InputComponent>()?;
            input_component.clear_transient_states();
        }
        process_input_queue(engine)
    }

    pub fn haptics_system(_engine: &mut Engine) -> Result<()> {
        Ok(())
    }
}

fn process_input_queue(engine: &mut Engine) -> Result<()> {
    while let Some(event) = engine.input_queue.pop_front() {
        let input_component = engine.get_global_component_mut::<InputComponent>()?;

        match event {
            InputEvent::Keyboard(KeyboardEvent::Key { key, state }) => {
                input_component.set_key(key, state);
            }

            InputEvent::Mouse(MouseEvent::Button { key, state }) => {
                input_component.set_mouse_button(key, state);
            }

            InputEvent::Mouse(MouseEvent::Wheel { delta }) => match delta {
                MouseScrollDelta::LineDelta(x, y) => {
                    input_component.set_mouse_scroll_delta(Vector2f::new(x, y));
                }

                MouseScrollDelta::PixelDelta(delta) => {
                    input_component.set_mouse_scroll_pixel_delta(Vector2f::new(
                        delta.x as f32,
                        delta.y as f32,
                    ));
                }
            },

            InputEvent::Mouse(MouseEvent::Delta { delta }) => {
                input_component.set_mouse_delta(delta);
            }

            InputEvent::Mouse(MouseEvent::Position { position }) => {
                input_component.set_mouse_position(position);
            }

            InputEvent::Gamepad(GamepadEvent::Button { id, button, state }) => {
                input_component.set_gamepad_button(id, button, state);
            }

            InputEvent::Gamepad(GamepadEvent::Axis { id, axis, value }) => {
                input_component.set_gamepad_axis(id, axis, value);
            }

            InputEvent::Gamepad(GamepadEvent::Connected { id }) => {
                input_component.connect_gamepad(id);
            }

            InputEvent::Gamepad(GamepadEvent::Disconnected { id }) => {
                input_component.disconnect_gamepad(id);
            }

            InputEvent::Gamepad(GamepadEvent::ForceFeedbackEffectCompleted { id }) => {
                input_component.complete_force_feedback_effect(id);
            }
        }
    }

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{haptics_system, input_system};

#[cfg(target_arch = "wasm32")]
pub use wasm::{haptics_system, input_system};
