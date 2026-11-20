//! The audio example: one sound, one listener and one source that plays it.
//!
//! # Responsibilities
//!
//! - Loads `res/audio/blip.wav` through the asset store.
//! - Spawns the listener the extension hears from and the emitter it plays.
//! - Reports once when the source starts playing, so a headless run can be
//!   checked without a speaker.
//!
//! # Design
//!
//! `pill_audio` owns the output device and the system that plays; a project
//! only decides what exists. A listener entity says where the world is heard
//! from, and a source entity says what plays. Both carry a
//! [`TransformComponent`], because the extension reads its positions from
//! there.
//!
//! The source is created with [`AudioSourceComponent::playing`], which sets the
//! flag the extension turns into a play request on its next run. From then on
//! the component is the extension's to write: `is_playing` and `sink` are its
//! answers, and this example only reads them.
//!
//! A machine with no audio output is a supported posture. The extension logs a
//! warning and inserts no manager, the source never starts, and this example
//! still announces that the scene is ready - so the line a headless run waits
//! for does not depend on a device existing.
//!
//! Every line this example writes starts with `[audio_scene]`, its folder name.
//! That is the convention the extension-example suite reads: a run passes when
//! the log carries `[<folder name>]`, so a new example states its own name once
//! and never needs an entry in a test.

// Standard library
use std::sync::atomic::{AtomicBool, Ordering};

// External crates
use pill_audio::{AudioListenerComponent, AudioSourceComponent, Sound};
use pill_engine::*;

// =============================================================================
// Constants
// =============================================================================

/// Where the example's sound lives, below the project's `res` directory.
const SOUND_PATH: &str = "audio/blip.wav";

/// Set once the source has been seen playing, so the report is written once
/// per loaded generation instead of every frame.
static PLAYBACK_REPORTED: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Entry points
// =============================================================================

/// Builds the scene once, on the project's first load.
///
/// Returns `0` when the scene is ready, or `1` when the sound or an entity
/// could not be created - the host treats a non-zero status as a failed load.
#[pill_project(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    // Step 1: Load the sound. A failure here means the example's own resource
    // is missing or unreadable, which is a failed example rather than a
    // degraded one.
    let sound = match engine.load_asset::<Sound>(SOUND_PATH) {
        Ok(handle) => handle,
        Err(reason) => {
            tracing::error!("[audio_scene] sound not loaded: {reason}");
            return 1;
        }
    };

    // Step 2: Where the world is heard from.
    let world = engine.world_mut();
    if let Err(error) = world
        .create_entity()
        .with(AudioListenerComponent::default())
        .with(TransformComponent::default())
        .build()
    {
        tracing::error!("[audio_scene] listener not spawned: {error}");
        return 1;
    }

    // Step 3: What plays, and from where. The emitter sits to one side so a
    // spatial sink has something to pan against.
    let emitter_transform = TransformComponent {
        translation: [1.0, 0.0, 0.0],
        ..TransformComponent::default()
    };
    if let Err(error) = world
        .create_entity()
        .with(AudioSourceComponent::playing(sound))
        .with(emitter_transform)
        .build()
    {
        tracing::error!("[audio_scene] source not spawned: {error}");
        return 1;
    }

    tracing::info!("[audio_scene] ready sound={SOUND_PATH}");
    0
}

/// The registration half.
///
/// The report system needs no call of its own: [`report_playback`] declares
/// itself, so the engine registers it on every generation and a reload never
/// leaves the project with two.
pub fn register(_engine: &mut Engine) -> u32 {
    0
}

// =============================================================================
// Systems
// =============================================================================

/// Logs once, on the first frame a source reports that it is playing.
#[system(name = "audio_example_report", order = 110)]
fn report_playback(mut sources: Query<&AudioSourceComponent>) {
    if PLAYBACK_REPORTED.load(Ordering::Relaxed) {
        return;
    }
    for source in sources.iter_mut() {
        if source.is_playing {
            PLAYBACK_REPORTED.store(true, Ordering::Relaxed);
            tracing::info!("[audio_scene] playing sink={}", source.sink);
            return;
        }
    }
}
