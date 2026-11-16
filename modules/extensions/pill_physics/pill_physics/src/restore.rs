//! Drops the simulation when the world it mirrors was restored.
//!
//! # Responsibilities
//!
//! - Compares the world's restore epoch with the one the rapier world was
//!   built against, and clears the simulation when they differ.
//!
//! # Design
//!
//! The check is a system of its own rather than a step inside
//! [`physics_sync`](crate::sync): that system only runs while the game does,
//! and the simulation has to be dropped on the first edit frame after Stop.
//! The restored entity allocator can hand out a handle that a play-time body
//! still occupies, so anything that asks the manager about an entity between
//! Stop and the next Play must not see the session's bodies.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{system, Res, ResMut, RestoreEpoch, SystemError};

// Current crate
use crate::manager::PhysicsManager;

// =============================================================================
// physics_restore
// =============================================================================

/// Clears the simulation after a play session put the edit world back.
///
/// It opts in to edit mode, so it runs while the editor is not playing, and its
/// order puts it ahead of the simulation so a Play in the frame right after a
/// Stop already starts from the restored components. Bodies return on that
/// Play, from those components.
///
/// # Errors
///
/// Returns [`SystemError::MissingResource`] when the physics manager is absent,
/// which only happens in a world this module was not registered in.
#[system(name = "physics_restore", order = 899, run_in_edit_mode = true)]
pub fn physics_restore(
    restore_epoch: Res<RestoreEpoch>,
    mut manager: ResMut<PhysicsManager>,
) -> Result<(), SystemError> {
    let Some(mut manager) = manager.get_mut() else {
        return Err(SystemError::MissingResource {
            name: String::from("PhysicsManager"),
        });
    };
    // A world built without an engine has no epoch to compare against, and
    // nothing restores it either.
    let Some(restore_epoch) = restore_epoch.get() else {
        return Ok(());
    };
    if manager.clear_if_world_was_restored(restore_epoch.0) {
        pill_core::info!(
            target: telemetry_target::ECS,
            restore_epoch = restore_epoch.0,
            "[physics] cleared the simulation after a world restore; bodies return on the next Play"
        );
    }
    Ok(())
}
