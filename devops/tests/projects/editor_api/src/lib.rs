//! The editor API's test project: one entity to select and one asset to edit.
//!
//! # Responsibilities
//!
//! - Spawn one entity, so the suite has a scene entity to select, edit and
//!   delete besides the four the `pill_test` module keeps.
//! - Report the project is ready with a line carrying the folder's name, the
//!   convention a test project follows so a headless run can be checked.
//!
//! # Design
//!
//! The project owns data, never editor behaviour: the callbacks the suite
//! drives live in the `pill_test` extension's `_editor` crate, which only an
//! editor host loads. Nothing here names an editor type, which is what keeps
//! a shipping or web build of this project identical to a game's.
//!
//! The spawn happens in `register` and only when the world holds no entity
//! carrying the tag, because a reload of this project repeats `register`: an
//! entity the reader deleted must stay deleted.

// External crates
use pill_engine::*;
use pill_rendering::TransformComponent;

// =============================================================================
// Components
// =============================================================================

/// Marks the entity this project spawns.
///
/// Persistable, so the editor's own reload keeps it and a second generation
/// does not spawn a duplicate.
#[component(persistable)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EditorApiProbe {
    /// A value the suite writes through the Inspector.
    pub value: f32,
}

// =============================================================================
// Entry points
// =============================================================================

/// Spawns the project's entity on every generation that finds none.
///
/// # Errors
///
/// Returns 1 when the entity cannot be created, which the host reads as a
/// failed generation.
#[pill_project(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

/// The half a reload repeats: it must be idempotent.
pub fn register(engine: &mut Engine) -> u32 {
    // Step 1: is there a probe entity already?
    let existing = {
        let mut query = Query::<&EditorApiProbe>::new(engine.world_mut());
        query.iter_mut().count()
    };
    if existing == 0 {
        if let Err(error) = engine
            .world_mut()
            .create_entity()
            .with(EditorApiProbe::default())
            .with(TransformComponent::default())
            .build()
        {
            tracing::error!("[editor_api] probe entity not spawned: {error}");
            return 1;
        }
    }
    tracing::info!("[editor_api] ready entities={}", existing.max(1));
    0
}

// the editor api area's reload
