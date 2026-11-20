//! The audio extension: sound assets, playback components and the system that
//! plays them.
//!
//! # Responsibilities
//!
//! - Defines the [`Sound`] asset: encoded audio bytes held by the
//!   [`AssetManager`](pill_engine::AssetManager) and decoded for each playback.
//! - Defines the [`AudioListenerComponent`] and [`AudioSourceComponent`]
//!   components, and the [`AudioManager`] resource that owns the output stream
//!   and its sink pools.
//! - Registers [`audio_system`], which drives playback on every frame.
//!
//! # Design
//!
//! **A sound is an asset, not a resource.** A world holds many sounds, so
//! [`Sound`] implements [`Asset`](pill_engine::Asset) and is referenced by
//! `Handle<Sound>`. The handle carries a generation: after a sound is unloaded,
//! a handle that names it finds nothing, so a component that still holds the
//! handle plays nothing. The asset never has to clear the components that name
//! it.
//!
//! **The audio manager is a resource** because the machine has exactly one
//! output device, and its rodio stream must stay open while any sink plays. A
//! resource lives as long as the world, which is the lifetime the stream needs.
//!
//! **A component records the request; the system performs it.** A caller that
//! edits a component cannot reach the manager, so the requested action is kept
//! as a plain [`AudioCommand`] field and carried out by [`audio_system`]. The
//! component stays `#[repr(C)]` plain data, which is what lets it survive a hot
//! reload and be read from C#.
//!
//! ## Spatial audio
//!
//! A spatial sink places a sound at a 3D point. The transform's
//! [`translation`](pill_engine::TransformComponent::translation) is that point:
//! an emitter follows its entity's translation, and a listener's ears are
//! placed around the listener's own translation.
//!
//! [`AudioListenerComponent`] keeps its own `facing_degrees` instead of reading
//! the transform's quaternion, and [`ear_positions`] is the one function that
//! turns a listener's placement into the two points rodio pans against.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::*;

// =============================================================================
// Function inventory
// =============================================================================

// The build script scans this crate and writes `function_inventory.rs`, which
// holds one address entry for every function a patch can reach. Including it
// here is what makes those functions resolvable by their qualified paths, with
// nothing annotated in this file. It has to sit at the crate root because the
// generated entries name the modules from the crate root.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Modules
// =============================================================================

/// The [`AudioCommand`] enum: the action a source has been asked to take next.
pub mod audio_command;
/// The [`AudioListenerComponent`] component: where the world is heard from.
pub mod audio_listener_component;
/// The [`AudioLoadQueue`] resource: sound bytes a project asks the module to
/// load.
pub mod audio_load_queue;
/// The [`AudioManager`] resource: the output device and its sink pools.
pub mod audio_manager;
/// The [`AudioSourceComponent`] component: an entity that plays a sound.
pub mod audio_source_component;
/// The per-frame system that drives playback.
pub mod audio_system;
/// [`ear_positions`]: turns a listener's position and facing into its two ear
/// points.
pub mod listener_geometry;
/// The [`Sound`] asset: an audio file held as its encoded bytes.
pub mod sound;
/// The [`SoundType`] enum: whether a sound plays at a position in the world.
pub mod sound_type;

// =============================================================================
// Re-exports
// =============================================================================

// The module that declares a type is an implementation detail. A caller names
// `pill_audio::AudioSourceComponent` wherever the type's file lives.
pub use audio_command::AudioCommand;
pub use audio_listener_component::AudioListenerComponent;
pub use audio_load_queue::AudioLoadQueue;
pub use audio_manager::{
    AudioManager, AudioSettings, AudioSettingsAsset, DEFAULT_AMBIENT_SINK_COUNT,
    DEFAULT_SPATIAL_SINK_COUNT, EAR_SEPARATION,
};
pub use audio_source_component::{AudioSourceComponent, NO_SINK};
pub use audio_system::audio_system;
pub use listener_geometry::ear_positions;
pub use sound::{Sound, SoundImportSettings, SoundLoadError, SUPPORTED_AUDIO_FORMATS};
pub use sound_type::SoundType;

// =============================================================================
// Registration
// =============================================================================

/// Registers this module's components, resources and system with the engine.
///
/// This is the half a reload repeats: the components, resources and system
/// whose code moved with the arriving image. It must be idempotent, because a
/// first load reaches it too, immediately before [`init`].
///
/// The [`AudioManager`] is installed only when the machine has an audio
/// device. Without one the world still gets the components and the system, so
/// queued sounds are still stored; the system then has nothing to play them
/// with. That is what lets this module run on a machine with no sound card.
///
/// Returns 0.
pub fn register(engine: &mut Engine) -> u32 {
    // Step 1: Create the shared sound-load queue if the world has none.
    if engine.world().get_resource::<AudioLoadQueue>().is_none() {
        engine
            .world_mut()
            .insert_resource(AudioLoadQueue::default());
    }
    // Step 2: Register the components. Register each one through its derive's
    // registrar, so it arrives with its field layout. The Inspector's rows and
    // the managed codegen read that layout; without it a component is an
    // opaque blob to both.
    audio_listener_component::register(engine.world_mut());
    audio_source_component::register(engine.world_mut());
    // The playback system reads both positions from the transform, so a module
    // without one is a module that plays nothing. A renderer's data crate
    // usually registers it; a headless project loading this module alone has
    // no renderer, which is why the module does it itself. Registration is
    // idempotent, so a world that already has it is unaffected.
    pill_engine::register_transform_component(engine.world_mut());
    // The sound asset type needs no call: `Sound` declares itself, and the
    // entry point declares it before this function runs, which is what
    // re-points the asset store's table at the generation still mapped.

    // Step 3: Declare the settings asset the device is tuned through. The
    // declaration is repeated on every generation, so the surface never points
    // at code the reload unloaded.
    engine
        .world_mut()
        .register_settings_asset_manager::<AudioManager>();
    // The same for how its document draws: the sink counts explain themselves
    // and cannot be changed while the device runs, and the ear separation is a
    // slider rather than a free number.
    if let Some(registry) = engine
        .world_mut()
        .get_resource_mut::<pill_engine::ImportRegistry>()
    {
        registry.declare_field_decorators(
            "pill_audio::AudioSettingsAsset",
            &[
                (
                    "ambient_sinks",
                    pill_engine::FieldDecorator::Tooltip(
                        "Voices for sounds that play without a position. Fixed when the device opens; restart the host to change it."
                            .to_owned(),
                    ),
                ),
                (
                    "spatial_sinks",
                    pill_engine::FieldDecorator::Tooltip(
                        "Voices for sounds that play at a position. Fixed when the device opens; restart the host to change it."
                            .to_owned(),
                    ),
                ),
                (
                    "ear_separation",
                    pill_engine::FieldDecorator::Range {
                        min: 0.1,
                        max: 10.0,
                    },
                ),
            ],
        );
    }

    // Step 4: Install the manager once, if an output device exists.
    // Build the device only once. Registration runs again after every reload,
    // and replacing a live manager would stop whatever is playing.
    if engine.world().get_resource::<AudioManager>().is_none() {
        match AudioManager::with_defaults() {
            Some(audio_manager) => {
                // The manager insert, not the plain one: it records the
                // settings surface the project configures the device with.
                engine
                    .world_mut()
                    .insert_settings_asset_manager(audio_manager);
            }
            None => {
                pill_core::warn!(
                    target: telemetry_target::ECS,
                    "no audio output device; pill_audio loaded without playback"
                );
            }
        }
    }
    // Step 4: Announce the registration. The playback system needs no call of
    // its own: `audio_system` declares itself, so the engine registers it
    // before this function runs, on every generation.
    pill_core::info!(
        target: telemetry_target::ECS,
        ambient_sinks = DEFAULT_AMBIENT_SINK_COUNT,
        spatial_sinks = DEFAULT_SPATIAL_SINK_COUNT,
        "pill_audio module registered"
    );
    0
}

/// The artifact's one-time entry point.
///
/// The module keeps nothing that has to run once: everything it owns is
/// registration, and [`register`] re-runs all of it on every generation. This
/// function exists because a monolithic build loads the module once and calls
/// this entry point alone, so it defers to [`register`].
///
/// Public so the generated `host_module_pill_audio` wrapper can carry it as
/// `pill_module_init`, the entry point the host looks up in a loaded DLL.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::{Engine, Handle};

    /// The source's `sound` field is a typed asset reference. Reading it back
    /// through the component field API names the shared `Sound` type, which is
    /// what lets the Inspector draw the field as an asset row and the picker
    /// list sounds. Writing a handle through the same API reaches the
    /// component the playback system reads, and a handle of another asset type
    /// is refused.
    #[test]
    fn the_audio_sources_sound_is_a_writable_asset_reference() {
        // Step 1: Create an entity with a source holding a known handle.
        let mut engine = Engine::new();
        assert_eq!(register(&mut engine), 0);
        let world = engine.world_mut();
        let entity = world
            .create_entity()
            .with(AudioSourceComponent::new(Handle::from_raw(2, 1)))
            .build()
            .unwrap();
        let component_name =
            pill_engine::component::ComponentRegistry::registered_name::<AudioSourceComponent>();
        // Step 2: Read the field back; it must name the shared Sound type.
        assert_eq!(
            world
                .read_component_field(entity, &component_name, "sound")
                .unwrap(),
            pill_engine::FieldValue::Asset {
                asset_type: Some("pill_audio::Sound".to_owned()),
                index: 2,
                generation: 1,
            }
        );

        // Step 3: Write another sound's handle and check the component took it.
        world
            .write_component_field(
                entity,
                &component_name,
                "sound",
                pill_engine::FieldValue::Asset {
                    asset_type: Some("pill_audio::Sound".to_owned()),
                    index: 4,
                    generation: 0,
                },
            )
            .expect("another sound's handle writes");
        assert_eq!(
            world
                .get_component::<AudioSourceComponent>(entity)
                .unwrap()
                .sound,
            Handle::from_raw(4, 0)
        );

        // Step 4: A handle of another asset type must be refused.
        let refused_write = world.write_component_field(
            entity,
            &component_name,
            "sound",
            pill_engine::FieldValue::Asset {
                asset_type: Some("pill_master_renderer::assets::Mesh".to_owned()),
                index: 0,
                generation: 0,
            },
        );
        assert!(
            matches!(
                refused_write,
                Err(pill_engine::component_field::ComponentFieldError::AssetTypeMismatch { .. })
            ),
            "{refused_write:?}"
        );
    }
}
