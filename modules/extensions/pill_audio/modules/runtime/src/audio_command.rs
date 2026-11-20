//! The [`AudioCommand`] enum, which holds the action an audio source has been
//! asked to perform on the next frame.
//!
//! # Responsibilities
//!
//! - Declares the actions an
//!   [`AudioSourceComponent`](crate::AudioSourceComponent) stores, which the
//!   audio system carries out on the next frame.
//!
//! # Design
//!
//! A caller that changes a component cannot reach the
//! [`AudioManager`](crate::AudioManager), so the requested action is kept in
//! the component as plain data. [`audio_system`](crate::audio_system) reads
//! the action and performs it, because the system holds both the sources and
//! the manager.

// External crates
use serde::{Deserialize, Serialize};

// =============================================================================
// AudioCommand
// =============================================================================

/// An action an [`AudioSourceComponent`] has been asked to perform, carried
/// out on the next frame.
///
/// The component stores one command, not a queue. Each command sets a
/// complete state on its own, so only the last one set in a frame is carried
/// out. A caller that plays and then stops before the next frame gets the
/// stop.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AudioCommand {
    /// No action is pending.
    #[default]
    None = 0,
    /// Starts playback from the beginning. A source that is already playing
    /// is restarted.
    Play = 1,
    /// Pauses playback and keeps the current position.
    Pause = 2,
    /// Stops playback and returns the sink to the pool, so another source
    /// can use it.
    Stop = 3,
}
