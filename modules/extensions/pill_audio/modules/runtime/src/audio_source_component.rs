//! The [`AudioSourceComponent`] component: an entity that plays a sound.
//!
//! # Responsibilities
//!
//! - Declares the component that holds the intent to play, plus the playback
//!   state [`audio_system`](crate::audio_system) writes back to it.
//!
//! # Design
//!
//! Every field is plain data, so the component survives a hot reload and can
//! be mirrored into C#. The sound is held as a [`Handle<Sound>`], the same way
//! a mesh or a material is referenced. A handle is `#[repr(C)]` two-integer
//! data with no allocation, so it crosses the module boundary safely. It also
//! names one loaded sound, so playing it needs no path lookup.

// External crates
use pill_engine::component;
use pill_engine::Handle;

// Current crate
use crate::audio_command::AudioCommand;
use crate::sound::Sound;
use crate::sound_type::SoundType;

// =============================================================================
// Constants
// =============================================================================

/// Sentinel for [`AudioSourceComponent::sink`] when no sink is held.
pub const NO_SINK: u32 = u32::MAX;

// =============================================================================
// AudioSourceComponent
// =============================================================================

/// Plays a [`Sound`] from an entity.
///
/// The component only records the intent to play. [`audio_system`](crate::audio_system)
/// performs it and writes the result back here.
///
/// The sound is a [`Handle<Sound>`], the same reference every other component
/// uses for its assets. A handle that no longer resolves - because its sound
/// was unloaded - plays nothing, so unloading a sound is safe.
#[component(persistable, shared)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioSourceComponent {
    /// Which sink pool to draw from.
    pub sound_type: SoundType,
    /// Playback volume, where 1.0 is the sound's own level.
    pub volume: f32,
    /// Whether to start playback the next time the system runs.
    ///
    /// The system turns the flag into a play request and clears it, so a
    /// reload does not restart the sound.
    pub play_on_awake: bool,
    /// What the source has been asked to do next.
    pub command: AudioCommand,
    /// Whether a sink is currently playing this source.
    ///
    /// Written by [`audio_system`](crate::audio_system); treat as read-only.
    pub is_playing: bool,
    /// Index of the sink serving this source, or `NO_SINK`.
    ///
    /// Written by [`audio_system`](crate::audio_system); treat as read-only.
    pub sink: u32,
    /// The sound this source plays.
    ///
    /// Until it is given a sound it holds [`Handle::INVALID`], which plays
    /// nothing.
    pub sound: Handle<Sound>,
}

impl core::default::Default for AudioSourceComponent {
    fn default() -> Self {
        Self {
            sound_type: SoundType::Spatial,
            volume: 1.0,
            play_on_awake: false,
            command: AudioCommand::None,
            is_playing: false,
            sink: NO_SINK,
            sound: Handle::INVALID,
        }
    }
}

impl AudioSourceComponent {
    /// A source that asks to play `sound` on the next update.
    pub fn playing(sound: Handle<Sound>) -> Self {
        Self {
            play_on_awake: true,
            sound,
            ..Self::default()
        }
    }

    /// A source bound to `sound` but silent until [`Self::play`] is called.
    pub fn new(sound: Handle<Sound>) -> Self {
        Self {
            sound,
            ..Self::default()
        }
    }

    /// Request playback from the start on the next frame.
    pub fn play(&mut self) {
        self.command = AudioCommand::Play;
    }

    /// Request a pause on the next frame.
    pub fn pause(&mut self) {
        self.command = AudioCommand::Pause;
    }

    /// Request a stop, releasing the sink, on the next frame.
    pub fn stop(&mut self) {
        self.command = AudioCommand::Stop;
    }

    /// Point this source at a different sound.
    ///
    /// Stops whatever is playing first. The sink keeps playing the old bytes
    /// until it is released, so changing the sound alone would leave the old
    /// one playing.
    pub fn set_sound(&mut self, sound: Handle<Sound>) {
        if self.sound != sound {
            self.sound = sound;
            self.command = AudioCommand::Stop;
        }
    }

    /// Whether a sink is currently assigned to this source.
    pub fn has_sink(&self) -> bool {
        self.sink != NO_SINK
    }
}

// =============================================================================
// Registration
// =============================================================================

/// Registers [`AudioSourceComponent`] with the world, field layout included.
///
/// The derive generates `__pill_register_AudioSourceComponent` as a private
/// item of this module, so [`crate::register`] reaches it through here rather
/// than by path. With the layout, the Inspector draws the `sound` handle as an
/// asset row and the managed mirror gets a typed field for it.
pub(crate) fn register(world: &mut pill_engine::World) {
    __pill_register_AudioSourceComponent(world);
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A handle is plain two-integer data, which is what lets the component
    /// cross the module boundary; `pill_engine` keeps that layout.
    #[test]
    fn the_sound_is_a_plain_handle() {
        let sound = Handle::from_raw(3, 1);
        let source = AudioSourceComponent::new(sound);
        assert_eq!(source.sound, sound);
        assert_eq!(source.sound.index(), 3);
        assert_eq!(
            AudioSourceComponent::default().sound,
            Handle::<Sound>::INVALID
        );
    }

    /// Pointing a source at a new sound stops the old one, because the sink
    /// still holds a decoder over the previous bytes.
    #[test]
    fn changing_the_sound_requests_a_stop() {
        let mut source = AudioSourceComponent::new(Handle::from_raw(1, 0));
        assert_eq!(source.command, AudioCommand::None);
        source.set_sound(Handle::from_raw(2, 0));
        assert_eq!(source.sound, Handle::from_raw(2, 0));
        assert_eq!(source.command, AudioCommand::Stop);
    }

    /// Setting the same sound again is not a change, so playback continues.
    #[test]
    fn setting_the_same_sound_is_a_no_op() {
        let sound = Handle::from_raw(1, 0);
        let mut source = AudioSourceComponent::new(sound);
        source.set_sound(sound);
        assert_eq!(source.command, AudioCommand::None);
    }

    /// The command field records the last intent expressed.
    #[test]
    fn commands_replace_one_another() {
        let mut source = AudioSourceComponent::new(Handle::from_raw(1, 0));
        source.play();
        assert_eq!(source.command, AudioCommand::Play);
        source.pause();
        assert_eq!(source.command, AudioCommand::Pause);
        source.stop();
        assert_eq!(source.command, AudioCommand::Stop);
    }

    /// A fresh source holds no sink, which `has_sink` reports through the
    /// sentinel rather than an `Option` the C# mirror could not read.
    #[test]
    fn a_new_source_holds_no_sink() {
        let source = AudioSourceComponent::default();
        assert!(!source.has_sink());
        assert_eq!(source.sink, NO_SINK);
        assert!(!source.is_playing);
    }

    /// `playing` is the shorthand for "start as soon as the world ticks".
    #[test]
    fn playing_marks_the_source_for_awake_playback() {
        let sound = Handle::from_raw(7, 2);
        let source = AudioSourceComponent::playing(sound);
        assert!(source.play_on_awake);
        assert_eq!(source.sound, sound);
    }
}
