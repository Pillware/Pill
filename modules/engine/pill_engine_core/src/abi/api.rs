//! The table the host hands to every loaded artifact at its entry point.
//!
//! # Responsibilities
//!
//! - Defines [`EngineApi`], the `#[repr(C)]` struct every project and module
//!   DLL receives through its `pill_module_init` export.
//! - Builds one for a live engine with [`EngineApi::new`].
//!
//! # Design
//!
//! **Both sides of the module ABI are Rust.** The host owns the
//! [`Engine`](crate::Engine) and hands each loaded artifact a pointer to it.
//! The artifact casts `engine_handle` back to `&mut Engine` and calls the full
//! typed API. A language-neutral ABI would need one function pointer per
//! engine operation. The engine's surface is generic Rust
//! (`register_component::<T>`, typed queries), and such a table cannot express
//! it without a second implementation of the whole API.
//!
//! What crosses the DLL boundary is a `#[repr(C)]` struct, not a bare pointer.
//! It leaves room to add a field without changing the signature of every
//! entry point. Adding or removing a field still changes the struct layout
//! every module is compiled against, so every module and project must be
//! rebuilt together. Development does that automatically, because the host
//! builds them all from source at startup.

// Standard library
use std::ffi::c_void;

// Current crate
use crate::engine::Engine;

// =============================================================================
// EngineApi
// =============================================================================

/// The engine table a project or module DLL receives at its entry point.
///
/// # Safety
///
/// `engine_handle` is only valid while the host's [`Engine`](crate::Engine)
/// lives and while the artifact is loaded. The artifact must not keep the
/// `&mut Engine` it builds from the handle once `pill_module_init` returns.
///
/// # Examples
///
/// ```ignore
/// #[no_mangle]
/// pub unsafe extern "C" fn pill_module_init(api: *const EngineApi) -> u32 {
///     let api = unsafe { &*api };
///     let engine: &mut Engine = unsafe { &mut *(api.engine_handle as *mut Engine) };
///     run_artifact_entry(engine, |_engine| {}, register, init)
/// }
/// ```
///
/// In practice no artifact writes that by hand: `#[pill_project]` and
/// `#[pill_module]` generate the entry point, including the `catch_unwind`
/// guard that keeps a panic from unwinding across the C ABI boundary, and the
/// entry point declares what the artifact's macros declare - its components,
/// systems, resources and asset types - before the artifact's own `register`
/// runs. An artifact's own code never names its systems, resources or assets;
/// they arrive by declaration.
#[repr(C)]
pub struct EngineApi {
    /// Opaque handle to the engine instance, cast back to `&mut Engine` by the
    /// receiving artifact.
    pub engine_handle: *mut c_void,
}

impl EngineApi {
    /// Builds the table that points at the given engine.
    ///
    /// The engine must outlive every artifact that receives the result. The
    /// host therefore boxes its engine before it builds this table: the handle
    /// points into that box, so moving the host does not move the engine and
    /// the pointer stays valid.
    pub fn new(engine: &mut Engine) -> Self {
        Self {
            engine_handle: engine as *mut Engine as *mut c_void,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The handle addresses the engine it was built from.
    ///
    /// Every generated entry point relies on this contract: `pill_module_init`
    /// casts the field back and expects the host's engine.
    #[test]
    fn the_handle_casts_back_to_the_engine_it_was_built_from() {
        let mut engine = Engine::new();
        let expected = &mut engine as *mut Engine;
        let engine_api = EngineApi::new(&mut engine);

        assert_eq!(
            engine_api.engine_handle as *mut Engine, expected,
            "the handle must address the engine it was built from"
        );

        // SAFETY: the handle was just built from `engine`, which is still alive
        // and not otherwise borrowed, so this is the same unique reference the
        // generated entry points reconstruct.
        let restored: &mut Engine = unsafe { &mut *(engine_api.engine_handle as *mut Engine) };
        assert_eq!(
            restored.world().entity_count(),
            0,
            "a fresh world is reachable through the handle"
        );
    }

    /// Every module DLL is compiled against this layout, so changing it means
    /// rebuilding every module at the same time. This test pins the layout, so
    /// such a change cannot happen by accident.
    #[test]
    fn the_table_is_one_pointer_wide() {
        assert_eq!(
            std::mem::size_of::<EngineApi>(),
            std::mem::size_of::<*mut c_void>(),
            "EngineApi carries exactly the engine handle"
        );
        assert_eq!(
            std::mem::align_of::<EngineApi>(),
            std::mem::align_of::<*mut c_void>(),
        );
    }
}
