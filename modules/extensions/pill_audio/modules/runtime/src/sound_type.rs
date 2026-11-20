//! Defines [`SoundType`], which says whether a sound is positioned in the
//! world.
//!
//! # Responsibilities
//!
//! - Declares the two playback modes. Each mode selects which of the two sink
//!   pools a source draws from.
//!
//! # Design
//!
//! The source component, the manager and the system all name this type, so it
//! has a file of its own. The manager switches on it to pick one of the two
//! sink pools, so every playback operation reads it.

// External crates
use serde::{Deserialize, Serialize};

// =============================================================================
// SoundType
// =============================================================================

/// How a sound is positioned relative to the listener.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SoundType {
    /// Not positioned in space: the sound plays at the volume its source sets,
    /// wherever the entity is. Use this for music and UI sounds.
    Ambient = 0,
    /// Positioned in space: panned left or right, and made quieter as the
    /// distance between the entity and the listener's ears grows.
    #[default]
    Spatial = 1,
}
