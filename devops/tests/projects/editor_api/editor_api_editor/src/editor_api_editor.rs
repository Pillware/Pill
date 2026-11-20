//! The `editor_api` project's own editor crate: a project reaching the editor
//! API without being an extension.
//!
//! # Responsibilities
//!
//! - Register one editor callback through the shared request queue, so the
//!   suite can prove a project's editor part hears the editor and that a
//!   project reload rebinds it.
//! - Log what it heard under `[test] project hook`, which is what the editor
//!   suite's `editor_api` area reads.
//!
//! # Design
//!
//! The crate is a loadable artifact like any other: it lives beside the
//! project's manifest, `Cargo.toml` and this file are the whole convention, and
//! an editor host finds it by that convention alone. Nothing in the project
//! crate names it, so a standalone, shipping or web build never builds it and
//! never knows it exists.
//!
//! It holds no state of its own: every value it needs arrives in the callback
//! payload, which is what lets a reload replace the image without repeating
//! anything.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_editor_core::hooks::{EditorHookRequests, FieldEdit, FieldEditHookRequest, HookContext};
use pill_engine::{pill_module, Engine};
use pill_test::{ModuleTest, ModuleTestField};

// =============================================================================
// Constants
// =============================================================================

/// The crate registering the callback, which the log lines and the liveness
/// marker are keyed by.
pub const SOURCE: &str = env!("CARGO_PKG_NAME");

// =============================================================================
// The callback
// =============================================================================

/// Logs every applied write of the probe component's accumulated time.
pub fn after_probe_edit(_context: &mut HookContext<'_>, edit: &FieldEdit<'_, ModuleTest>) {
    if edit.field != ModuleTestField::AccumulatedTime {
        return;
    }
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = %edit.entity,
        value = ?edit.current,
        cause = edit.cause.name(),
        "[test] project hook field-edit-after"
    );
}

// =============================================================================
// Registration
// =============================================================================

/// Queues the crate's one request, which the editor host binds at its frame
/// boundary.
///
/// # Errors
///
/// Returns 1 when the world has no request queue. The host reads any non-zero
/// status as a failed generation and keeps the project running without the
/// editor part.
pub fn register(engine: &mut Engine) -> u32 {
    let world = engine.world_mut();
    EditorHookRequests::ensure(world);
    let Some(requests) = world.get_resource_mut::<EditorHookRequests>() else {
        return 1;
    };
    requests.push(FieldEditHookRequest::after::<ModuleTest>(
        SOURCE,
        after_probe_edit,
    ));
    pill_core::info!(
        target: telemetry_target::EDITOR,
        source = SOURCE,
        callbacks = 1,
        "[test] project hook registered"
    );
    0
}

/// The artifact's one-time entry point.
///
/// Everything this crate owns is registration, which [`register`] repeats on
/// every generation, so this defers to it.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}
