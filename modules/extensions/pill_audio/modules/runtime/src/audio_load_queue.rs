//! A queue through which a project asks the audio extension to load sound bytes.
//!
//! # Responsibilities
//!
//! - Carry encoded sound bytes from a project to the running playback DLL,
//!   which turns them into [`Sound`](crate::Sound) assets on its next frame.
//! - Keep one pending load per playback name, with the newest bytes winning.
//!
//! # Design
//!
//! [`Sound`](crate::Sound) is a shared asset type, so a project that links the
//! crate can also register a sound itself and take the handle directly. The
//! queue is for bytes a project hands over at runtime without naming the type:
//! the module drains it, so the project never has to know when loading happens.

// Standard library
use std::collections::BTreeMap;

// External crates
use pill_engine::resource;

// =============================================================================
// AudioLoadQueue
// =============================================================================

/// Queued sound loads, keyed by playback name, waiting for the audio system.
///
/// The shared name is what lets a project and this module reach one queue: an
/// id built from `TypeId` differs between the two images.
#[resource(shared = "pill_audio::AudioLoadQueue")]
#[repr(C)]
#[derive(Default)]
pub struct AudioLoadQueue {
    /// The encoded bytes of each queued sound, by playback name.
    pub(crate) sounds: BTreeMap<String, Vec<u8>>,
}

impl AudioLoadQueue {
    /// Queues an encoded sound under its playback name.
    ///
    /// Queuing the same name again replaces the bytes that were still waiting.
    pub fn load(&mut self, playback_name: &str, encoded_bytes: &[u8]) {
        self.sounds
            .insert(playback_name.to_owned(), encoded_bytes.to_vec());
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audio_system, AudioListenerComponent, AudioSourceComponent, Sound};
    use pill_engine::{AssetManager, Engine, TransformComponent};

    /// Queued bytes reach the asset store as [`Sound`] assets even with no
    /// audio device installed, which is the headless case.
    #[test]
    fn playback_system_consumes_requests_without_an_audio_device() {
        let mut engine = Engine::new();
        engine
            .world_mut()
            .register_component::<TransformComponent>();
        engine
            .world_mut()
            .register_component::<AudioListenerComponent>();
        engine
            .world_mut()
            .register_component::<AudioSourceComponent>();
        let mut queue = AudioLoadQueue::default();
        queue.load("test", &[1, 2, 3]);
        engine.world_mut().insert_resource(queue);
        engine.register_system("audio", audio_system);
        engine.process_frame().unwrap();
        assert!(engine.system_failures().is_empty());
        assert!(engine
            .world()
            .get_resource::<AudioLoadQueue>()
            .unwrap()
            .sounds
            .is_empty());
        let assets = engine.world().get_resource::<AssetManager>().unwrap();
        assert_eq!(
            assets.get_by_name::<Sound>("test").unwrap().bytes(),
            &[1, 2, 3]
        );
    }
}
