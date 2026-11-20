//! The per-frame system driving every [`AudioSourceComponent`].
//!
//! # Responsibilities
//!
//! - Turns queued [`AudioLoadQueue`] bytes into [`Sound`] assets.
//! - Places the active listener's ears.
//! - Carries out each source's pending [`AudioCommand`].
//! - Moves each playing spatial source's emitter to its entity, and returns
//!   sinks whose sound has ended to the pool.
//!
//! # Design
//!
//! Takes its state as ordinary system parameters instead of `&mut Engine`.
//! The scheduler reads the access pattern from the signature, so it never
//! runs this system at the same time as another one that touches the same
//! resources.

// External crates
use pill_engine::{system, AssetManager, Query, ResMut, SystemError, TransformComponent};

// Current crate
use crate::audio_command::AudioCommand;
use crate::audio_listener_component::AudioListenerComponent;
use crate::audio_manager::AudioManager;
use crate::audio_source_component::{AudioSourceComponent, NO_SINK};
use crate::listener_geometry::ear_positions;
use crate::sound::Sound;
use crate::sound_type::SoundType;
use crate::AudioLoadQueue;

// =============================================================================
// audio_system
// =============================================================================

/// Drives every [`AudioSourceComponent`] once per frame.
///
/// Queued sound loads become [`Sound`] assets first. Then four steps run, in
/// this order:
///
/// 1. the active listener's ears are placed, so panning matches this frame;
/// 2. each source's pending [`AudioCommand`] is carried out;
/// 3. each playing spatial source's emitter is moved to its entity;
/// 4. sinks whose sound has finished are returned to the pool.
///
/// Step 4 is what makes a fixed pool workable. Without it, a source would keep
/// its sink after its sound ended, and the pool would run out of voices.
///
/// With no [`AudioManager`] installed the function only drains the load
/// queue, which is the headless case. See [`AudioManager::new`].
///
/// # Errors
///
/// Returns [`SystemError::Failure`] when the asset store refuses to store a
/// queued sound under its name. The name is checked first, so this is not
/// expected to happen.
#[system(name = "pill_audio")]
pub fn audio_system(
    mut audio_manager: ResMut<AudioManager>,
    mut asset_manager: ResMut<AssetManager>,
    mut load_queue: ResMut<AudioLoadQueue>,
    mut listeners: Query<(&AudioListenerComponent, &TransformComponent)>,
    mut sources: Query<(&mut AudioSourceComponent, &TransformComponent)>,
) -> Result<(), SystemError> {
    let Some(mut asset_manager) = asset_manager.get_mut() else {
        return Ok(());
    };
    if let Some(mut load_queue) = load_queue.get_mut() {
        for (name, bytes) in std::mem::take(&mut load_queue.sounds) {
            if asset_manager.get_by_name::<Sound>(&name).is_none() {
                let sound = Sound::from_bytes(std::path::Path::new(&name), bytes);
                // The check above rules a name clash out, so this error is
                // passed on to the engine instead of being dropped.
                asset_manager
                    .add_named(name, sound)
                    .map_err(|error| SystemError::Failure {
                        message: error.to_string(),
                    })?;
            }
        }
    }
    // Step 1: find the active listener. There is one pair of ears, so the
    // first enabled listener wins and the rest are ignored.
    let mut ears = None;
    for (listener, transform) in listeners.iter_mut() {
        if listener.enabled {
            ears = Some(ear_positions(
                transform.translation,
                listener.facing_degrees,
            ));
            break;
        }
    }

    // There is no manager on a machine without an audio device. The components
    // still exist, and their commands simply stay pending.
    let Some(mut audio_manager) = audio_manager.get_mut() else {
        return Ok(());
    };

    if let Some((left, right)) = ears {
        audio_manager.set_ears(left, right);
    }

    for (mut source, transform) in sources.iter_mut() {
        // `play_on_awake` is consumed once. Clearing it here stops a reload
        // from restarting every sound, because a reload runs registration
        // again but keeps the world's components.
        if source.play_on_awake {
            source.play_on_awake = false;
            if source.command == AudioCommand::None {
                source.command = AudioCommand::Play;
            }
        }

        let sound_type = source.sound_type;
        let command = std::mem::replace(&mut source.command, AudioCommand::None);

        // Step 2: carry out the pending command.
        match command {
            AudioCommand::Play => {
                // A source that already holds a sink is restarted. The sink is
                // released first, because a second decoder appended to it
                // would play after the first sound instead of replacing it.
                if source.has_sink() {
                    audio_manager.release(sound_type, source.sink);
                    source.sink = NO_SINK;
                }
                let sound = asset_manager.get::<Sound>(source.sound);
                if let Some(sound) = sound {
                    if let Some(sink_index) = audio_manager.acquire(sound_type) {
                        if audio_manager.start(sound_type, sink_index, sound, source.volume) {
                            source.sink = sink_index;
                            source.is_playing = true;
                        } else {
                            // The bytes could not be decoded, so the sink goes
                            // straight back to the pool instead of being held
                            // while it plays nothing.
                            audio_manager.release(sound_type, sink_index);
                            source.is_playing = false;
                        }
                    }
                }
            }
            AudioCommand::Pause => {
                if source.has_sink() {
                    audio_manager.pause(sound_type, source.sink);
                    source.is_playing = false;
                }
            }
            AudioCommand::Stop => {
                if source.has_sink() {
                    audio_manager.release(sound_type, source.sink);
                    source.sink = NO_SINK;
                    source.is_playing = false;
                }
            }
            AudioCommand::None => {}
        }

        // Step 3: move a playing spatial source's emitter to its entity and
        // copy the source's volume onto its sink.
        if source.has_sink() && source.is_playing {
            if sound_type == SoundType::Spatial {
                audio_manager.set_emitter(source.sink, transform.translation);
            }
            audio_manager.set_volume(sound_type, source.sink, source.volume);
        }

        // Step 4: return the sink of a playing source to the pool once its
        // sound has ended.
        if source.has_sink()
            && source.is_playing
            && audio_manager.is_finished(sound_type, source.sink)
        {
            audio_manager.release(sound_type, source.sink);
            source.sink = NO_SINK;
            source.is_playing = false;
        }
    }

    Ok(())
}
