//! Polls gamepads into input events and plays the rumble the systems ask for.
//!
//! # Responsibilities
//!
//! - Own the process's [`GamepadContext`] and give each gamepad a
//!   [`PlayerId`] slot when it connects ([`Gamepads::poll`]).
//! - Translate the library's buttons and axes into the engine's.
//! - Play the rumble the systems ask for, keeping each effect alive while it
//!   runs ([`Gamepads::play_rumble`]).
//!
//! # Design
//!
//! A gamepad is not a window event. The `gilrs` library (imported as
//! `gamepad_library`) reads gamepads from the operating system by itself. A
//! frontend asks for the changes once per frame, before it runs the frame, and
//! pushes them like any other input.
//!
//! Slots are given out in connection order, the lowest free one first. A fifth
//! gamepad is ignored until a slot frees.
//!
//! Dropping an `Effect` stops it, so a rumble that is playing must be kept
//! alive until its duration has passed. That deadline also covers a gamepad
//! that never reports completion (some XInput pads).

// Standard library
use std::time::Duration;

// External crates
use gamepad_library::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Replay, Ticks};
use gamepad_library::Gilrs as GamepadContext;
use gamepad_library::{Axis, Button, EventType, GamepadId};
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::{debug, info, warn};
use pill_engine::input::GAMEPAD_SLOT_COUNT;
use pill_engine::{ButtonState, GamepadAxis, GamepadButton, InputEvent, PlayerId, RumbleRequest};

// =============================================================================
// Gamepads
// =============================================================================

/// A rumble effect that is still playing.
struct ActiveRumble {
    /// The gamepad it plays on.
    gamepad_id: GamepadId,
    /// Kept alive while it plays. Dropping it stops the effect.
    _effect: Effect,
    /// When the effect started playing.
    started_at: Instant,
    /// How long the effect plays.
    play_duration: Duration,
}

/// The gamepads, their player slots and the rumble playing on them.
pub struct Gamepads {
    /// The platform's gamepad context, or `None` when the gamepad library
    /// failed to start.
    gamepad_context: Option<GamepadContext>,
    /// The gamepad holding each [`PlayerId`] slot.
    slots: [Option<GamepadId>; GAMEPAD_SLOT_COUNT],
    /// Gamepads that were connected before the first poll. The first poll
    /// announces them.
    gamepads_connected_at_start: Vec<GamepadId>,
    /// Effects still playing.
    active_rumble_effects: Vec<ActiveRumble>,
}

impl Gamepads {
    /// Opens the platform's gamepad support.
    ///
    /// Never fails. On a platform without gamepad support the result reports
    /// no gamepads, and the reason is logged.
    pub fn new() -> Self {
        let gamepad_context = match GamepadContext::new() {
            Ok(gamepad_context) => Some(gamepad_context),
            // A working context that will never see a gamepad.
            Err(gamepad_library::Error::NotImplemented(gamepad_context)) => {
                warn!(
                    target: telemetry_target::ENGINE,
                    "gamepads are not supported on this platform"
                );
                Some(gamepad_context)
            }
            Err(error) => {
                warn!(
                    target: telemetry_target::ENGINE,
                    error = %error,
                    "gamepads unavailable: the gamepad library failed to start"
                );
                None
            }
        };
        // The library reports a gamepad that was already plugged in through
        // `gamepads()`, not as a connection event.
        let gamepads_connected_at_start = gamepad_context
            .as_ref()
            .map(|gamepad_context| gamepad_context.gamepads().map(|(id, _)| id).collect())
            .unwrap_or_default();
        Self {
            gamepad_context,
            slots: [None; GAMEPAD_SLOT_COUNT],
            gamepads_connected_at_start,
            active_rumble_effects: Vec::new(),
        }
    }

    /// Hands every gamepad change since the last poll to `push_input`.
    ///
    /// Call it once per frame, before running the frame.
    pub fn poll(&mut self, mut push_input: impl FnMut(InputEvent)) {
        // Step 1: Announce the gamepads that were connected at startup.
        for gamepad_id in std::mem::take(&mut self.gamepads_connected_at_start) {
            self.connect(gamepad_id, &mut push_input);
        }

        // Step 2: Translate what happened since the last poll.
        while let Some(event) = self
            .gamepad_context
            .as_mut()
            .and_then(GamepadContext::next_event)
        {
            let gamepad_id = event.id;
            match event.event {
                EventType::Connected => self.connect(gamepad_id, &mut push_input),
                EventType::Disconnected => self.disconnect(gamepad_id, &mut push_input),
                EventType::ButtonPressed(button, _) => {
                    self.push_button(gamepad_id, button, ButtonState::Pressed, &mut push_input);
                }
                EventType::ButtonReleased(button, _) => {
                    self.push_button(gamepad_id, button, ButtonState::Released, &mut push_input);
                }
                // On most pads the triggers' travel arrives as a button value,
                // not an axis.
                EventType::ButtonChanged(Button::LeftTrigger2, trigger_value, _) => {
                    self.push_axis(
                        gamepad_id,
                        GamepadAxis::LeftTrigger,
                        trigger_value,
                        &mut push_input,
                    );
                }
                EventType::ButtonChanged(Button::RightTrigger2, trigger_value, _) => {
                    self.push_axis(
                        gamepad_id,
                        GamepadAxis::RightTrigger,
                        trigger_value,
                        &mut push_input,
                    );
                }
                EventType::AxisChanged(axis, axis_value, _) => {
                    if let Some(axis) = gamepad_axis(axis) {
                        self.push_axis(gamepad_id, axis, axis_value, &mut push_input);
                    }
                }
                EventType::ForceFeedbackEffectCompleted => {
                    self.active_rumble_effects
                        .retain(|rumble| rumble.gamepad_id != gamepad_id);
                }
                // A repeat is not a press, another button's value is not an
                // axis, and a dropped event carries nothing.
                _ => {}
            }
        }

        // Step 3: Drop effects whose duration has passed.
        self.active_rumble_effects
            .retain(|rumble| rumble.started_at.elapsed() < rumble.play_duration);
    }

    /// Plays every rumble request on the gamepads that support force feedback.
    ///
    /// Call it after a frame, with the requests its systems made. A request
    /// for an empty slot or a gamepad without force feedback is dropped, and
    /// so is every request when there is no gamepad context.
    pub fn play_rumble(&mut self, requests: Vec<RumbleRequest>) {
        let Some(gamepad_context) = self.gamepad_context.as_mut() else {
            return;
        };
        for request in requests {
            let Some(gamepad_id) = self.slots[request.player.index()] else {
                continue;
            };
            if !gamepad_context.gamepad(gamepad_id).is_ff_supported() {
                continue;
            }
            match rumble_effect(gamepad_context, gamepad_id, &request).and_then(|effect| {
                effect.play()?;
                Ok(effect)
            }) {
                Ok(effect) => self.active_rumble_effects.push(ActiveRumble {
                    gamepad_id,
                    _effect: effect,
                    started_at: Instant::now(),
                    play_duration: request.duration,
                }),
                Err(error) => warn!(
                    target: telemetry_target::ENGINE,
                    player = ?request.player,
                    error = %error,
                    "gamepad rumble failed"
                ),
            }
        }
    }

    /// Gives `gamepad_id` the lowest free slot and announces it.
    fn connect(&mut self, gamepad_id: GamepadId, push_input: &mut impl FnMut(InputEvent)) {
        if self.player_of(gamepad_id).is_some() {
            return;
        }
        let Some(index) = self.slots.iter().position(Option::is_none) else {
            debug!(
                target: telemetry_target::ENGINE,
                slots = GAMEPAD_SLOT_COUNT,
                "gamepad connected, but every player slot is taken; ignoring it"
            );
            return;
        };
        self.slots[index] = Some(gamepad_id);
        let player = PlayerId::ALL[index];
        if let Some(gamepad_context) = &self.gamepad_context {
            info!(
                target: telemetry_target::ENGINE,
                player = ?player,
                name = gamepad_context.gamepad(gamepad_id).name(),
                "gamepad connected"
            );
        }
        push_input(InputEvent::GamepadConnected { player });
    }

    /// Frees `gamepad_id`'s slot and announces it.
    fn disconnect(&mut self, gamepad_id: GamepadId, push_input: &mut impl FnMut(InputEvent)) {
        let Some(player) = self.player_of(gamepad_id) else {
            return;
        };
        self.slots[player.index()] = None;
        self.active_rumble_effects
            .retain(|rumble| rumble.gamepad_id != gamepad_id);
        info!(target: telemetry_target::ENGINE, player = ?player, "gamepad disconnected");
        push_input(InputEvent::GamepadDisconnected { player });
    }

    /// Pushes a button change for a gamepad that holds a slot.
    fn push_button(
        &self,
        gamepad_id: GamepadId,
        button: Button,
        button_state: ButtonState,
        push_input: &mut impl FnMut(InputEvent),
    ) {
        if let (Some(player), Some(button)) = (self.player_of(gamepad_id), gamepad_button(button)) {
            push_input(InputEvent::GamepadButton {
                player,
                button,
                state: button_state,
            });
        }
    }

    /// Pushes an axis change for a gamepad that holds a slot.
    fn push_axis(
        &self,
        gamepad_id: GamepadId,
        axis: GamepadAxis,
        axis_value: f32,
        push_input: &mut impl FnMut(InputEvent),
    ) {
        if let Some(player) = self.player_of(gamepad_id) {
            push_input(InputEvent::GamepadAxis {
                player,
                axis,
                value: axis_value,
            });
        }
    }

    /// Returns the slot `gamepad_id` holds, if any.
    fn player_of(&self, gamepad_id: GamepadId) -> Option<PlayerId> {
        self.slots
            .iter()
            .position(|slot| *slot == Some(gamepad_id))
            .and_then(PlayerId::from_index)
    }
}

impl Default for Gamepads {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds a two-motor rumble effect for `request` on `gamepad_id`.
///
/// # Errors
///
/// Returns the gamepad library's error when the gamepad refuses the effect.
fn rumble_effect(
    gamepad_context: &mut GamepadContext,
    gamepad_id: GamepadId,
    request: &RumbleRequest,
) -> Result<Effect, gamepad_library::ff::Error> {
    let duration_milliseconds = u32::try_from(request.duration.as_millis()).unwrap_or(u32::MAX);
    let scheduling = Replay {
        play_for: Ticks::from_ms(duration_milliseconds),
        ..Default::default()
    };
    // Motor strengths are 16-bit magnitudes.
    let magnitude = |strength: f32| (strength.clamp(0.0, 1.0) * f32::from(u16::MAX)) as u16;
    EffectBuilder::new()
        .add_effect(BaseEffect {
            kind: BaseEffectType::Weak {
                magnitude: magnitude(request.weak),
            },
            scheduling,
            ..Default::default()
        })
        .add_effect(BaseEffect {
            kind: BaseEffectType::Strong {
                magnitude: magnitude(request.strong),
            },
            scheduling,
            ..Default::default()
        })
        .gamepads(&[gamepad_id])
        .finish(gamepad_context)
}

/// Returns the engine's button for a library button, or `None` when the engine
/// does not track it (the extra `C`/`Z` buttons and unknown ones).
///
/// The library names face buttons by position. The engine names them after the
/// Xbox layout, where X is on the left and Y on top.
fn gamepad_button(button: Button) -> Option<GamepadButton> {
    match button {
        Button::South => Some(GamepadButton::A),
        Button::East => Some(GamepadButton::B),
        Button::West => Some(GamepadButton::X),
        Button::North => Some(GamepadButton::Y),
        Button::LeftTrigger => Some(GamepadButton::LeftBumper),
        Button::RightTrigger => Some(GamepadButton::RightBumper),
        Button::LeftTrigger2 => Some(GamepadButton::LeftTrigger),
        Button::RightTrigger2 => Some(GamepadButton::RightTrigger),
        Button::Select => Some(GamepadButton::Back),
        Button::Start => Some(GamepadButton::Start),
        Button::Mode => Some(GamepadButton::Mode),
        Button::LeftThumb => Some(GamepadButton::LeftStick),
        Button::RightThumb => Some(GamepadButton::RightStick),
        Button::DPadUp => Some(GamepadButton::DPadUp),
        Button::DPadDown => Some(GamepadButton::DPadDown),
        Button::DPadLeft => Some(GamepadButton::DPadLeft),
        Button::DPadRight => Some(GamepadButton::DPadRight),
        _ => None,
    }
}

/// Returns the engine's axis for a library axis, or `None` when the engine
/// does not track it.
fn gamepad_axis(axis: Axis) -> Option<GamepadAxis> {
    match axis {
        Axis::LeftStickX => Some(GamepadAxis::LeftStickX),
        Axis::LeftStickY => Some(GamepadAxis::LeftStickY),
        Axis::RightStickX => Some(GamepadAxis::RightStickX),
        Axis::RightStickY => Some(GamepadAxis::RightStickY),
        Axis::LeftZ => Some(GamepadAxis::LeftTrigger),
        Axis::RightZ => Some(GamepadAxis::RightTrigger),
        Axis::DPadX => Some(GamepadAxis::DPadX),
        Axis::DPadY => Some(GamepadAxis::DPadY),
        _ => None,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_buttons_follow_the_xbox_layout() {
        assert_eq!(gamepad_button(Button::South), Some(GamepadButton::A));
        assert_eq!(gamepad_button(Button::West), Some(GamepadButton::X));
        assert_eq!(gamepad_button(Button::North), Some(GamepadButton::Y));
        assert_eq!(gamepad_button(Button::C), None);
    }

    #[test]
    fn trigger_axes_map_to_trigger_travel() {
        assert_eq!(gamepad_axis(Axis::LeftZ), Some(GamepadAxis::LeftTrigger));
        assert_eq!(gamepad_axis(Axis::Unknown), None);
    }
}
