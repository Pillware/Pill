//! The [`AudioManager`] resource: the audio output device and its sink pools.
//!
//! # Responsibilities
//!
//! - Owns the rodio output stream. The stream must stay open while any sound
//!   plays through it, because dropping it stops every sound.
//! - Hands out a sink from one of two fixed pools, one pool per [`SoundType`],
//!   and takes it back when the sound using it ends.
//!
//! # Design
//!
//! There is exactly one audio device. A value a system builds per frame would
//! be dropped at the end of that frame, which silences the sound it just
//! started, so the device lives in a resource instead.
//!
//! The sinks are pooled instead of created on demand. Creating one makes rodio
//! allocate a mixer channel, and doing that while a frame is being built would
//! put an allocation on the audio path, which must not stall. The size of a
//! pool is therefore also the limit on how many voices play at once.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{asset, resource, AssetLoadResult, AssetManager, SettingsOutcome};
use serde::{Deserialize, Serialize};

// Current crate
use crate::sound::Sound;
use crate::sound_type::SoundType;

// =============================================================================
// Constants
// =============================================================================

/// Sinks the manager creates for non-positional playback.
///
/// One sink is one voice, so this is also the limit on how many non-positional
/// sounds play at once. The pool is built once, in [`AudioManager::new`],
/// because rodio allocates a mixer channel per sink and a pool that grew
/// mid-frame would allocate on the audio path.
pub const DEFAULT_AMBIENT_SINK_COUNT: usize = 16;

/// Sinks the manager creates for positional playback, and so the limit on how
/// many positional sounds play at once.
pub const DEFAULT_SPATIAL_SINK_COUNT: usize = 16;

/// Half the distance between the listener's ears, in world units.
///
/// rodio pans a spatial sink by the distance between each ear and the emitter,
/// so this value decides how strong the stereo effect is.
pub const EAR_SEPARATION: f32 = 1.0;

// =============================================================================
// AudioManager
// =============================================================================

/// Owns the output stream and the pooled sinks that play through it.
///
/// The manager must outlive every sound playing through it, because dropping
/// the stream stops all audio. It is a resource for that reason, and because
/// there is exactly one audio device.
///
/// It declares its settings surface in the module's `register`
/// ([`AudioSettingsAsset`]): the applier points into that image, so every
/// generation re-points it.
///
/// It is a [`PlayState::Keep`](pill_engine::PlayState::Keep) resource: the
/// output stream belongs to the machine, so there is nothing to copy at Play,
/// and putting a pre-play copy back would drop the live stream and stop the
/// audio. What plays is decided by the entities' components, which the session
/// does restore.
#[resource(play_state = "keep")]
pub struct AudioManager {
    /// The output stream. Nothing reads it after construction, but dropping it
    /// silences every sink, so it is held for its lifetime alone.
    _stream: rodio::OutputStream,
    /// Handle used to build sinks against the stream. Also handed out by
    /// [`AudioManager::stream_handle`].
    stream_handle: rodio::OutputStreamHandle,
    /// Non-positional sinks, indexed by the sink index a source stores in
    /// [`AudioSourceComponent::sink`].
    ambient_sinks: Vec<rodio::Sink>,
    /// Positional sinks, indexed the same way.
    spatial_sinks: Vec<rodio::SpatialSink>,
    /// Indices of the ambient sinks that no source holds.
    free_ambient_sink_indices: Vec<u32>,
    /// Indices of the spatial sinks that no source holds.
    free_spatial_sink_indices: Vec<u32>,
    /// Half the distance between the listener's ears, as the project set it.
    ///
    /// The sinks were built with [`EAR_SEPARATION`]; a settings document may
    /// move them while the device runs, so the value the manager was built with
    /// is kept to tell a real change from a repeated one.
    ear_separation: f32,
}

// SAFETY: every field is `Send` in rodio 0.20 except `OutputStream`, which is
// not, because the cpal stream inside it belongs to the thread that created it
// on some platforms. The engine only reaches this resource from a system, and
// the scheduler serialises `ResMut<AudioManager>` against every other
// accessor, so one thread touches the value at a time. It must also not move
// between threads while a sink plays, and the resource store never moves it: a
// resource is written once into world memory and read in place from then on.
unsafe impl Send for AudioManager {}
// SAFETY: `Sync` needs nothing beyond what `Send` above argues - the scheduler
// hands out one accessor at a time, so two threads never share the value.
unsafe impl Sync for AudioManager {}

// =============================================================================
// AudioSettings
// =============================================================================

/// The audio device's tunable settings, as the project's settings asset
/// describes them.
///
/// Pool sizes are fixed when the device opens: a pool grows by allocating a
/// mixer channel per sink, and a source's `sink` index would point into a pool
/// that changed under it. A document that asks for different sizes is refused
/// with that reason, and the host is restarted to change them. Ear separation
/// can move while the device runs, so it is applied live.
///
/// This is the document of [`AudioSettingsAsset`]: the settings file is a real
/// asset (`res/settings/pill_audio.audio_settings`), so an edit is checked
/// against this type before it reaches the device, and the Inspector can draw
/// its fields.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Voices for non-positional playback.
    pub ambient_sinks: usize,
    /// Voices for positional playback.
    pub spatial_sinks: usize,
    /// Half the distance between the listener's ears, in world units.
    pub ear_separation: f32,
}

impl std::default::Default for AudioSettings {
    /// The values [`AudioManager::with_defaults`] builds.
    fn default() -> Self {
        Self {
            ambient_sinks: DEFAULT_AMBIENT_SINK_COUNT,
            spatial_sinks: DEFAULT_SPATIAL_SINK_COUNT,
            ear_separation: EAR_SEPARATION,
        }
    }
}

// =============================================================================
// AudioSettingsAsset
// =============================================================================

/// The standalone asset a project tunes the audio device through.
///
/// Its document is [`AudioSettings`], so the file at
/// `res/settings/pill_audio.audio_settings` names the same three fields the
/// manager applies. The loaded value is only a carrier: the manager reads the
/// document, not the asset.
///
/// The attribute declares the type, so every generation of the module registers
/// it with the world's asset store without a call in `register`.
#[asset(standalone)]
#[derive(Debug, Default)]
pub struct AudioSettingsAsset {
    /// The settings the file holds.
    pub settings: AudioSettings,
}

impl pill_engine::Asset for AudioSettingsAsset {
    fn shared_name() -> Option<&'static str> {
        Some("pill_audio::AudioSettingsAsset")
    }
}

impl pill_engine::StandaloneAsset for AudioSettingsAsset {
    type Document = AudioSettings;
    const FILE_EXTENSION: &'static str = "audio_settings";

    fn from_document(
        _name: &str,
        document: AudioSettings,
        _assets: &AssetManager,
    ) -> AssetLoadResult<Self> {
        Ok(Self { settings: document })
    }
}

// =============================================================================
// SettingsAssetManager
// =============================================================================

impl pill_engine::SettingsAssetManager for AudioManager {
    type Settings = AudioSettingsAsset;

    fn settings_name() -> &'static str {
        "pill_audio"
    }

    fn apply_settings(&mut self, settings: &AudioSettings) -> SettingsOutcome {
        // Step 1: Refuse a document no device could run with. A file is
        // hand-editable, so zero voices and a zero stereo spread have to be
        // refused rather than trusted.
        if settings.ambient_sinks < 1 || settings.spatial_sinks < 1 {
            pill_core::warn!(
                target: telemetry_target::ECS,
                manager = "pill_audio",
                error = "both sink counts must be at least 1",
                "[settings] audio settings refused; the manager keeps its values"
            );
            return SettingsOutcome::Refused;
        }
        if !settings.ear_separation.is_finite() || settings.ear_separation <= 0.0 {
            pill_core::warn!(
                target: telemetry_target::ECS,
                manager = "pill_audio",
                error = "ear_separation must be greater than 0",
                "[settings] audio settings refused; the manager keeps its values"
            );
            return SettingsOutcome::Refused;
        }
        // Step 2: A pool size cannot change under a playing source.
        let current = self.settings();
        if settings.ambient_sinks != current.ambient_sinks
            || settings.spatial_sinks != current.spatial_sinks
        {
            pill_core::warn!(
                target: telemetry_target::ECS,
                ambient_sinks = settings.ambient_sinks,
                spatial_sinks = settings.spatial_sinks,
                "[settings] audio sink counts are fixed when the device opens; restart the host to change them"
            );
            return SettingsOutcome::Refused;
        }
        // Step 3: Moving the ears is the one change a running device takes:
        // every sink already open reads its positions from the device state.
        if settings.ear_separation != self.ear_separation {
            self.set_ears(
                [-settings.ear_separation, 0.0, 0.0],
                [settings.ear_separation, 0.0, 0.0],
            );
            self.ear_separation = settings.ear_separation;
        }
        pill_core::info!(
            target: telemetry_target::ECS,
            ambient_sinks = settings.ambient_sinks,
            spatial_sinks = settings.spatial_sinks,
            ear_separation = settings.ear_separation,
            "[settings] audio settings applied"
        );
        SettingsOutcome::Applied
    }
}

impl AudioManager {
    /// The settings this device is running with.
    pub fn settings(&self) -> AudioSettings {
        AudioSettings {
            ambient_sinks: self.ambient_sinks.len(),
            spatial_sinks: self.spatial_sinks.len(),
            ear_separation: self.ear_separation,
        }
    }

    /// Opens the default output device and builds both sink pools.
    ///
    /// Returns `None` when there is no audio device: a headless CI runner, a
    /// container, a machine with audio turned off. That is an ordinary
    /// condition rather than an error. [`crate::register`] then installs no
    /// manager, and [`crate::audio_system`] only drains queued sound loads.
    pub fn new(ambient_sink_count: usize, spatial_sink_count: usize) -> Option<Self> {
        // Step 1: Open the default output device.
        let (stream, stream_handle) = rodio::OutputStream::try_default().ok()?;

        // Step 2: Build one sink per voice in each pool.
        let mut ambient_sinks = Vec::with_capacity(ambient_sink_count);
        for _ in 0..ambient_sink_count {
            ambient_sinks.push(rodio::Sink::try_new(&stream_handle).ok()?);
        }

        let mut spatial_sinks = Vec::with_capacity(spatial_sink_count);
        for _ in 0..spatial_sink_count {
            spatial_sinks.push(
                rodio::SpatialSink::try_new(
                    &stream_handle,
                    [0.0, 0.0, 0.0],
                    [-EAR_SEPARATION, 0.0, 0.0],
                    [EAR_SEPARATION, 0.0, 0.0],
                )
                .ok()?,
            );
        }

        // Step 3: Fill the free lists back to front, so `pop` hands out index
        // 0 first and the pools run in index order.
        let free_ambient_sink_indices = (0..ambient_sink_count as u32).rev().collect();
        let free_spatial_sink_indices = (0..spatial_sink_count as u32).rev().collect();

        Some(Self {
            _stream: stream,
            stream_handle,
            ambient_sinks,
            spatial_sinks,
            free_ambient_sink_indices,
            free_spatial_sink_indices,
            ear_separation: EAR_SEPARATION,
        })
    }

    /// Opens the default device with the default pool sizes.
    pub fn with_defaults() -> Option<Self> {
        Self::new(DEFAULT_AMBIENT_SINK_COUNT, DEFAULT_SPATIAL_SINK_COUNT)
    }

    /// Takes a free sink index from the pool that `sound_type` names.
    ///
    /// Returns `None` when every sink of that kind is busy, which limits the
    /// number of voices instead of allocating more under load.
    pub fn acquire(&mut self, sound_type: SoundType) -> Option<u32> {
        match sound_type {
            SoundType::Ambient => self.free_ambient_sink_indices.pop(),
            SoundType::Spatial => self.free_spatial_sink_indices.pop(),
        }
    }

    /// Returns a sink to its pool, stopping whatever it was playing.
    ///
    /// Stopping is what makes the sink reusable. rodio queues a new source
    /// behind whatever the sink still holds, so leftover audio from the last
    /// sound would play first; a stopped sink ends its current sound and drops
    /// the queued ones within a few milliseconds. Handing back an index the
    /// pool already holds is refused, because a double free would give one
    /// sink to two sources.
    pub fn release(&mut self, sound_type: SoundType, sink_index: u32) {
        let unassigned_sink_indices = match sound_type {
            SoundType::Ambient => &mut self.free_ambient_sink_indices,
            SoundType::Spatial => &mut self.free_spatial_sink_indices,
        };
        if unassigned_sink_indices.contains(&sink_index) {
            return;
        }
        match sound_type {
            SoundType::Ambient => {
                if let Some(pooled_sink) = self.ambient_sinks.get(sink_index as usize) {
                    pooled_sink.stop();
                }
            }
            SoundType::Spatial => {
                if let Some(pooled_sink) = self.spatial_sinks.get(sink_index as usize) {
                    pooled_sink.stop();
                }
            }
        }
        unassigned_sink_indices.push(sink_index);
    }

    /// Returns whether the sink has audio queued and is not paused.
    ///
    /// An index with no sink of that kind reports `false`.
    pub fn is_playing(&self, sound_type: SoundType, sink_index: u32) -> bool {
        match sound_type {
            SoundType::Ambient => self
                .ambient_sinks
                .get(sink_index as usize)
                .is_some_and(|pooled_sink| !pooled_sink.empty() && !pooled_sink.is_paused()),
            SoundType::Spatial => self
                .spatial_sinks
                .get(sink_index as usize)
                .is_some_and(|pooled_sink| !pooled_sink.empty() && !pooled_sink.is_paused()),
        }
    }

    /// Returns whether the sink has drained everything appended to it.
    ///
    /// An index with no sink of that kind counts as finished.
    pub fn is_finished(&self, sound_type: SoundType, sink_index: u32) -> bool {
        match sound_type {
            SoundType::Ambient => self
                .ambient_sinks
                .get(sink_index as usize)
                .is_none_or(rodio::Sink::empty),
            SoundType::Spatial => self
                .spatial_sinks
                .get(sink_index as usize)
                .is_none_or(rodio::SpatialSink::empty),
        }
    }

    /// Queues `sound` on `sink_index` and starts playing it at `volume`.
    ///
    /// Returns `false` and leaves the sink untouched when the bytes cannot be
    /// decoded or when `sink_index` names no sink of that kind.
    pub(crate) fn start(
        &self,
        sound_type: SoundType,
        sink_index: u32,
        sound: &Sound,
        volume: f32,
    ) -> bool {
        let Some(decoder) = sound.decoder() else {
            return false;
        };
        match sound_type {
            SoundType::Ambient => {
                let Some(pooled_sink) = self.ambient_sinks.get(sink_index as usize) else {
                    return false;
                };
                pooled_sink.append(decoder);
                pooled_sink.set_volume(volume);
                pooled_sink.play();
            }
            SoundType::Spatial => {
                let Some(pooled_sink) = self.spatial_sinks.get(sink_index as usize) else {
                    return false;
                };
                pooled_sink.append(decoder);
                pooled_sink.set_volume(volume);
                pooled_sink.play();
            }
        }
        true
    }

    /// Halts a sink without returning it to the pool.
    pub(crate) fn pause(&self, sound_type: SoundType, sink_index: u32) {
        match sound_type {
            SoundType::Ambient => {
                if let Some(pooled_sink) = self.ambient_sinks.get(sink_index as usize) {
                    pooled_sink.pause();
                }
            }
            SoundType::Spatial => {
                if let Some(pooled_sink) = self.spatial_sinks.get(sink_index as usize) {
                    pooled_sink.pause();
                }
            }
        }
    }

    /// Sets a sink's playback volume, where 1.0 is the sound's own level.
    pub(crate) fn set_volume(&self, sound_type: SoundType, sink_index: u32, volume: f32) {
        match sound_type {
            SoundType::Ambient => {
                if let Some(pooled_sink) = self.ambient_sinks.get(sink_index as usize) {
                    pooled_sink.set_volume(volume);
                }
            }
            SoundType::Spatial => {
                if let Some(pooled_sink) = self.spatial_sinks.get(sink_index as usize) {
                    pooled_sink.set_volume(volume);
                }
            }
        }
    }

    /// Moves a spatial sink's emitter to a world position.
    pub(crate) fn set_emitter(&self, sink_index: u32, emitter_position: [f32; 3]) {
        if let Some(pooled_sink) = self.spatial_sinks.get(sink_index as usize) {
            pooled_sink.set_emitter_position(emitter_position);
        }
    }

    /// Moves the ears of every spatial sink.
    ///
    /// The ears belong to the listener, not to a sound, so they are set on
    /// every spatial sink at once instead of per source.
    pub(crate) fn set_ears(&self, left_ear_position: [f32; 3], right_ear_position: [f32; 3]) {
        for pooled_sink in &self.spatial_sinks {
            pooled_sink.set_left_ear_position(left_ear_position);
            pooled_sink.set_right_ear_position(right_ear_position);
        }
    }

    /// Returns the stream handle, for code that builds its own sinks.
    pub fn stream_handle(&self) -> &rodio::OutputStreamHandle {
        &self.stream_handle
    }

    /// Returns how many sinks of each kind are unassigned, as
    /// `(ambient, spatial)`.
    pub fn free_counts(&self) -> (usize, usize) {
        (
            self.free_ambient_sink_indices.len(),
            self.free_spatial_sink_indices.len(),
        )
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // Current crate
    use super::*;

    /// The settings asset carries exactly the document it was built from, which
    /// is what a created file holds and what the applier reads back.
    #[test]
    fn the_settings_asset_carries_its_document() {
        let settings = AudioSettings {
            ambient_sinks: 4,
            spatial_sinks: 5,
            ear_separation: 0.5,
        };
        let asset = <AudioSettingsAsset as pill_engine::StandaloneAsset>::from_document(
            "settings/pill_audio.audio_settings",
            settings,
            &AssetManager::new(),
        )
        .expect("the document builds the asset");

        assert_eq!(asset.settings, settings);
        assert_eq!(
            <AudioSettingsAsset as pill_engine::StandaloneAsset>::FILE_EXTENSION,
            "audio_settings"
        );
    }
}
