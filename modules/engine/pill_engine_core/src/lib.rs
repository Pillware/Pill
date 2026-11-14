//! The engine core: an archetype-based Entity Component System and everything
//! built on it.
//!
//! # Responsibilities
//!
//! - Declares all public modules that compose the engine: world, queries,
//!   scheduler, assets, persistence, hot-patch machinery.
//! - Re-exports the public types at the crate root.
//! - Configures the Tracy profiled allocator when the `profiling-memory`
//!   feature is active.
//!
//! # Design
//!
//! Projects, extensions and the macros never name this crate directly. They
//! use `pill_engine`, a facade that re-exports everything here under the same
//! paths and adds the per-DLL `inventory` registries. This crate holds the
//! code every DLL shares, and never reads a registry: when it needs something
//! from one, the calling DLL passes it in. It must never depend on
//! `pill_engine`.
//!
//! The module ABI is **Rust-to-Rust by design**: a loaded project or module
//! receives an [`EngineApi`] with a pointer to the host's engine, and calls
//! the typed API through it. There is no language-neutral plugin table; see
//! [`api`] for why one would need a second copy of the whole API.

// The derive macros expand to `::pill_engine::...` paths, so this crate needs
// the name `pill_engine` in scope to use its own derives. `TransformComponent`
// derives `PillLayout` here rather than carrying hand-written offsets. Only
// `PillLayout` can be used here: the other derives submit into registries,
// which live in the facade.
extern crate self as pill_engine;

// =============================================================================
// Constants
// =============================================================================

/// Tracy profiled allocator that reports every allocation in Tracy's memory
/// view.
///
/// Only active with the opt-in `profiling-memory` feature. The sampling rate
/// comes from
/// [`crate::config::PROFILING_MEMORY_ALLOCATIONS_SAMPLING_FREQUENCY`].
///
/// It is declared here and not in the `profiling` module because it is
/// generic: every DLL that links it creates its own copy, and that copy calls
/// Tracy's C entry points directly. Those entry points exist only inside
/// `pill_core.dll`, which exports Rust symbols alone, so a separate extension
/// DLL fails to link with "undefined symbol ___tracy_emit_memory_*".
#[cfg(feature = "profiling-memory")]
#[global_allocator]
static PROFILED_ALLOCATOR: tracy_client::ProfiledAllocator<std::alloc::System> =
    tracy_client::ProfiledAllocator::new(
        std::alloc::System,
        crate::config::PROFILING_MEMORY_ALLOCATIONS_SAMPLING_FREQUENCY,
    );

// =============================================================================
// Public Modules
// =============================================================================

/// The engine API table a loaded project or module receives at its entry
/// point.
pub mod api;

/// Archetype-based component storage with structure-of-arrays layout.
pub mod archetype;

/// Many-per-type asset storage addressed by generational handle.
///
/// The counterpart to [`resource`] for data a world holds several of - meshes,
/// textures, materials - where the type alone does not name one value. The
/// store is itself a resource, so it reaches systems through the `Res` /
/// `ResMut` parameters.
pub mod asset;

/// Every asset of a project by guid and by path, loaded or not.
pub mod asset_database;

/// The YAML document format every asset file shares.
pub mod asset_document;

/// Importing assets by file extension, for code that does not know their type.
pub mod asset_import_registry;

/// Loading assets and scenes from game code by path.
pub mod asset_loading;

/// Metadata files beside an asset's source (`<source_file>.meta`).
pub mod asset_metadata;

/// Saved references to assets, by guid, that point at the asset in a later
/// run.
pub mod asset_reference;

/// Assets stored as their own file in `res`, with no source file.
pub mod asset_standalone;

/// Where an asset's bytes come from: mounted packs and the filesystem.
pub mod asset_store;

/// Deferred command queue for structural ECS mutations.
pub mod commands;

/// Components the engine defines because more than one consumer needs them.
pub mod common_components;

/// Component trait, type identification, and change-detection primitives.
pub mod component;

/// Generic type-erased component field access for editor-style tools.
pub mod component_field;

/// Compile-time field layouts of components and value types, emitted by the
/// derive macros.
pub mod component_registry;

/// The engine's tunable constants and the parallel-execution settings.
pub mod config;

/// Immediate debug drawing: lines, shapes, labels and gizmo handles. Any
/// system can draw them, and every renderer draws them.
pub mod debug_draw;

/// A periodic report of ECS state, switched on with
/// `Engine::set_ecs_diagnostics_interval` and printed at that interval.
pub mod diagnostics;

/// Engine-owned native dynamic buffer, re-exported from `pill_core`.
///
/// The type lives beside the allocation service that owns its blocks, so a
/// module can name it without depending on the whole ECS. It is re-exported
/// here so `pill_engine::DynamicBuffer` is found where users look for it: a
/// component field is its usual place.
pub mod dynamic_buffer {
    pub use pill_core::dynamic_buffer::*;
}

/// System registration, frame execution, and the parallel dispatch of
/// systems.
pub mod engine;

/// Lightweight entity handles with generation-based invalidation.
pub mod entity;

/// Typed error system for the ECS engine.
pub mod error;

/// Per-function hot patching: stable dispatch slots for registered systems.
pub mod hot_patch;

/// Keyboard, mouse and gamepad state maintained by the engine and read through
/// `Res<Input>`.
pub mod input;

/// The C ABI a mirrored Rust function crosses to reach C#: argument slots,
/// result encodings, and the channels that carry an error message or a
/// returned string.
pub mod mirror;

/// The extension ABI revision the host and every module compare at load time.
pub mod module_abi;

/// Component persistence and schema migration for hot-reload.
pub mod persistence;

/// Re-exports the profiling API from `pill_core`.
pub mod profiling;

/// Query system for efficient iteration over entities with specific components.
pub mod query;

/// Singleton resources stored in the [`World`], not attached to entities.
pub mod resource;

/// The saved form of asset and entity references: a guid and a stable id, plus
/// what a later run needs to find the target again.
pub mod saved_references;

/// A scene: a world's entities, components and references as one standalone
/// asset that loads back into a world.
pub mod scene;

/// Dependency analysis and parallel batch scheduling for system execution.
pub mod scheduler;

/// Script components: a component trait with an update step, and a context
/// that defers structural changes until it is safe to apply them.
pub mod scripting;

/// Settings assets that configure manager resources: the bindings that point a
/// manager at one, the file a host creates when the project has none, and the
/// change detection that re-applies an edited file.
pub mod manager_settings;

/// Manager settings: the settings asset a manager resource reads, its
/// conventional path, and the applier that moves its document into the live
/// value.
pub mod settings;

/// The startup scene: the `.scene` a project's settings name, loaded before
/// the project's own `init` fills the world.
pub mod startup_scene;

/// How a plain function's parameters are pulled from the world so the function
/// can run as a system.
pub mod system;

/// Frame timing maintained by the engine and read through `Res<Time>`.
pub mod time;

/// Type names as the rest of the engine sees them: `pill_engine::...` for the
/// engine's own types.
pub mod type_names;

/// Central ECS state container - entities, archetypes, components, and
/// resources.
pub mod world;

// =============================================================================
// Test Fixtures
// =============================================================================

/// The components the crate's unit tests build worlds out of.
#[cfg(test)]
mod test_fixtures;

// =============================================================================
// Public Re-exports
// =============================================================================

// Core engine types, re-exported so one `use` reaches them.
pub use api::EngineApi;
pub use asset::{
    Asset, AssetBindingError, AssetBindingResult, AssetGuid, AssetLoadError, AssetLoadResult,
    AssetLoader, AssetManager, Handle,
};
pub use asset_database::{AssetDatabase, AssetRecord, GuidConflict};
pub use asset_import_registry::AssetIdentity;
pub use asset_import_registry::{
    ErasedImportError, ErasedImportOutcome, FieldDecorator, ImportRegistrationError,
    ImportRegistry, ScanReport,
};
pub use asset_loading::{import_scene_document, load_scene_asset, scene_database};
pub use asset_metadata::{
    AssetImport, AssetImportError, ImportOutcome, ImportedAsset, MetadataPolicy, MetadataSource,
    ReimportOutcome,
};
pub use asset_reference::AssetReference;
pub use asset_standalone::{render_standalone, StandaloneAsset};
pub use commands::{CommandError, Commands};
pub use common_components::{register_transform_component, TransformComponent};
pub use component::{Component, ComponentId, ComponentTicks, Tick};
pub use component_field::{parse_enum_tag, ComponentFieldError, EnumFieldTag, FieldValue};
pub use debug_draw::{
    DebugColor, DebugDraw, DebugFrame, DebugLabel, DebugLifetime, DebugLine, DebugStyle, GizmoLine,
    GizmoTriangle, LabelAnchor,
};
pub use engine::{Engine, SystemOwner, SystemSnapshot};
pub use entity::{Entity, EntityId};
pub use error::{EngineError, SystemError, SystemFailure};
pub use hot_patch::{HotPatchError, HotPatchRegistry, HotSlot, PlainSlot};
pub use input::{
    ButtonState, GamepadAxis, GamepadButton, Input, InputEvent, KeyCode, Modifiers, MouseButton,
    PlayerId, RumbleRequest, ScrollDelta,
};
pub use manager_settings::{
    ManagerSettingsApplied, ManagerSettingsFailure, ManagerSettingsReport, ManagerSettingsSurface,
    MissingManagerSettings,
};
pub use persistence::{PersistResourceManifestEntry, ResourceSnapshot};
pub use pill_core::DynamicBuffer;
pub use query::{
    Added, BatchStats, Changed, Or, Query, QueryFilter, QueryTarget, Res, ResMut, With, Without,
};
pub use resource::{KeptLayoutDeclaration, ResHandle, Resource, ResourceId};
pub use saved_references::{
    restore_asset_reference, saved_reference, SavedAssetReference, SavedEntityLink,
    SavedEntityLinks, SavedReference,
};
pub use scene::{load_scene, save_scene, Scene, SceneDocument, SceneEntity, SceneLoad, SceneSave};
pub use scheduler::{SystemAccess, SystemScheduler, TypeKey};
pub use scripting::{ScriptComponent, ScriptContext};
pub use settings::{SettingsAssetManager, SettingsAssetOps, SettingsOutcome};
pub use startup_scene::{apply_startup_scene, queue_startup_scene, StartupScene};
pub use time::Time;

// Serde derives re-exported so downstream components can derive serialization without a direct dependency.
pub use serde::{Deserialize, Serialize};

// The type-map crate an asset type implements `TraitAccessible` from, re-exported
// so a project or module declaring an asset needs no dependency of its own: the
// macro is reached as `pill_engine::trait_type_map::impl_trait_accessible!`, and
// its expansion resolves through this path.
pub use trait_type_map;

// Tracing re-exported to keep telemetry under a single flat namespace.
pub use tracing;

// Registration macros, re-exported here so the facade's glob re-export carries
// them to every project and module. The `inventory::submit` they expand to is
// re-exported by the facade, which owns the registries.
pub use pill_engine_macros::{
    pill_hot, pill_hot_fn, pill_hot_resolver, pill_mirror_fn, pill_mirror_impl, pill_mirror_method,
    pill_mirror_object, pill_mirror_resource, pill_module, pill_project, pill_value_type, system,
    PillAsset, PillComponent, PillLayout, PillMirror, PillResource,
};

// World container and its entity-builder and error types.
pub use world::{
    AddComponentError, BuildError, ColumnChunk, EntityBuilder, EntityRow, RemoveComponentError,
    World,
};

// =============================================================================
// Profiling Macro Re-exports
// =============================================================================

// The profiling code and its `#[macro_export]` macros are written in
// `pill_core`'s `profiling` module. Re-exporting the macros here keeps this
// crate's own `crate::profile_*` call sites compiling and gives downstream
// users one flat namespace.
pub use pill_core::{
    profile_error, profile_frame_mark, profile_init, profile_message, profile_non_continuous_frame,
    profile_plot, profile_plot_config, profile_scope, profile_scope_detail, profile_scope_fine,
    profile_secondary_frame_mark, profile_thread, profile_warn,
};
