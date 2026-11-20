//! The owner that a module, the renderer or the project registers its ECS
//! systems under.
//!
//! # Responsibilities
//!
//! - Give each part that registers ECS systems an owner its systems are
//!   recorded under.
//! - Run an entry point with that owner set around the call.
//!
//! # Design
//!
//! An owner is a tag on each registered system that says what registered it:
//! one extension, the renderer, the project, or the engine for its own
//! systems. A system gets whichever owner was set while the entry point that
//! declared it ran. That is why the caller must set an owner around that call
//! ([`register_module`]).
//!
//! The owner decides three things: what a reload clears, what a pause
//! suspends, and how the editor's system list labels a row. A statically
//! linked build and the reloading host both take it from [`extension_owner`]
//! and [`renderer_owner`], so a module registers under the same owner in
//! either mode.

// External crates
use pill_engine::{Engine, SystemOwner};

// =============================================================================
// Constants
// =============================================================================

/// The scope the project registers its ECS systems under: no owner set, so
/// they carry [`SystemOwner::PROJECT`].
pub const PROJECT_SCOPE: Option<SystemOwner> = None;

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the owner that the extension at `index` registers its systems
/// under.
///
/// `index` counts from zero in load order. With a renderer, its data crate is
/// first.
pub fn extension_owner(index: usize) -> SystemOwner {
    SystemOwner::extension(index)
}

/// Returns the owner that a renderer registers its systems under.
///
/// The engine reserves this owner ([`SystemOwner::RENDERER`]) instead of
/// taking "one past the extensions". A host can skip an extension and keep
/// that module's owner reserved for a later load, so a count of the loaded
/// extensions can name an owner that is already taken. Deriving the renderer's
/// owner from that count would give two subjects the same owner, and a reload
/// of either would then clear the other's systems.
pub fn renderer_owner() -> SystemOwner {
    SystemOwner::RENDERER
}

/// Runs `init` with the owner from `scope` in effect and returns the status
/// `init` reported.
///
/// Every ECS system `init` registers is recorded under that owner. With no
/// scope, no owner is set and the systems carry [`SystemOwner::PROJECT`], the
/// owner the engine uses outside a module's registration.
pub fn register_module(
    engine: &mut Engine,
    scope: Option<SystemOwner>,
    init: impl FnOnce(&mut Engine) -> u32,
) -> u32 {
    match scope {
        Some(system_owner) => {
            engine.begin_module_registration(system_owner);
            let status = init(engine);
            engine.end_module_registration();
            status
        }
        None => init(engine),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Extensions are one-based, and the renderer's owner is reserved outside
    /// the extension range whatever the host loaded.
    #[test]
    fn owners_follow_the_load_order() {
        for index in 0..4usize {
            assert_eq!(extension_owner(index).0, index as u64 + 1);
            assert_ne!(renderer_owner(), extension_owner(index));
            assert_eq!(extension_owner(index).extension_index(), Some(index));
        }
        assert_eq!(renderer_owner(), SystemOwner::RENDERER);
        assert_eq!(renderer_owner().extension_index(), None);
        assert_ne!(renderer_owner(), SystemOwner::PROJECT);
        assert_ne!(renderer_owner(), SystemOwner::ENGINE);
    }

    /// A scoped entry point's status is returned, and the unscoped path still
    /// runs its entry point.
    #[test]
    fn a_scoped_entry_point_reports_its_status() {
        let mut engine = Engine::new();
        assert_eq!(
            register_module(&mut engine, Some(extension_owner(0)), |_| 7),
            7
        );
        assert_eq!(register_module(&mut engine, PROJECT_SCOPE, |_| 0), 0);
    }
}
