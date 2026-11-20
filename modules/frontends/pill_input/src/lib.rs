//! Input translation shared by the frontends.
//!
//! # Responsibilities
//!
//! - Translate winit window and device events into the engine's
//!   [`InputEvent`](pill_engine::InputEvent)s ([`winit_events`], feature
//!   `winit`).
//! - Own the gamepads: poll them into input events and play the rumble the
//!   game's systems request ([`gamepads`], feature `gamepad`, native only).
//!
//! # Design
//!
//! The engine defines what input is (`pill_engine::input`) and links no
//! platform library; `pill_player` does not link winit either. Each frontend
//! receives its platform's events in its own loop, so the translation lives
//! here. Every frontend that handles winit events (`pill_standalone`,
//! `pill_editor`, `pill_web`, `pill_web_editor`) uses this crate instead of
//! repeating it. Only `pill_standalone` polls gamepads and plays the rumble
//! its systems request; the others pass window input only.
//!
//! Frontends forward the translated events through `FrameDriver::push_input`
//! between frames. A frontend that polls gamepads gives the requests from
//! `FrameDriver::take_rumble_requests` to [`gamepads::Gamepads::play_rumble`]
//! after the frame.

/// Gamepads: polled into input events, with rumble played on request.
#[cfg(all(feature = "gamepad", not(target_arch = "wasm32")))]
pub mod gamepads;

/// winit's window and device events as engine input events.
#[cfg(feature = "winit")]
pub mod winit_events;
