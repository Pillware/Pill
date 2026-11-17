//! The editor API an `_editor` crate registers against: callbacks that hear
//! about the editor's edits.
//!
//! # Responsibilities
//!
//! - Hold the callbacks a module registered ([`EditorHookRegistry`]) behind a
//!   liveness marker the executable keeps, and the queue a module registers
//!   through ([`EditorHookRequests`]).
//! - Define the payloads a callback reads ([`FieldEdit`], [`FieldEditPhase`],
//!   [`FieldEditCause`]) and the verdict it answers ([`HookVerdict`]).
//! - Give a callback its context ([`HookContext`]): the world, the editor
//!   state, the commands it queues and the notices it may post.
//! - Dispatch the callbacks at the frame boundary ([`HookSession`]) and turn
//!   their answers into the applied, recorded batch.
//!
//! # Design
//!
//! The registry stores owned strings and `fn` pointers only, tied to a
//! [`ModuleLiveness`](crate::panels::ModuleLiveness) the executable holds - the
//! same contract module panels follow, because a callback address points into
//! the registering image and must not be called after that image unloads. A
//! module cannot register directly: it queues a [`HookRequest`] in its
//! `register`, and the executable or the browser binds it.
//!
//! A callback runs at the frame boundary inside the shared drain, between the
//! commands of one batch, so it has the world and can queue commands that join
//! the same undo step. Before-callbacks may replace or refuse a field edit;
//! after-callbacks react and queue follow-ups. A follow-up never fires the
//! callbacks again, and nothing a callback queues survives a history replay:
//! the step being replayed already holds the result.
//!
//! # Writing an `_editor` crate
//!
//! An `_editor` crate is an ordinary extension crate with the suffix, living
//! beside the runtime crate it extends
//! (`extensions/<folder>/<folder>_editor`). It depends on `pill_editor_core`
//! next to its own runtime crate, declares `editor_frontend = "egui"` under
//! `[package.metadata.pill]`, and only an editor host ever builds it. Nothing
//! else has to be added: the host discovers the crate by its folder, loads it
//! after the runtime crates, and reloads it whenever one of them reloads.
//!
//! ```ignore
//! use pill_editor_core::hooks::{
//!     EditorHookRequests, FieldEdit, FieldEditHookRequest, HookContext, HookRequest, HookVerdict,
//! };
//!
//! const SOURCE: &str = env!("CARGO_PKG_NAME");
//!
//! fn clamp(context: &mut HookContext<'_>, edit: &FieldEdit<'_, MyComponent>) -> HookVerdict {
//!     if edit.counter > 10 {
//!         HookVerdict::Replace(FieldValue::U32(10))
//!     } else {
//!         HookVerdict::Keep
//!     }
//! }
//!
//! pub fn register(engine: &mut Engine) -> u32 {
//!     let world = engine.world_mut();
//!     EditorHookRequests::ensure(world);
//!     if let Some(requests) = world.get_resource_mut::<EditorHookRequests>() {
//!         requests.push(FieldEditHookRequest::new::<MyComponent>(SOURCE, clamp));
//!     }
//!     0
//! }
//! ```
//!
//! What the context offers, in one list:
//!
//! - `world()` and `world_mut()`, the live world;
//! - `selection()`, `gizmo()`, `active_tool()`, `viewport()` and `cause()`, the
//!   editor state at the moment of the call;
//! - `queue(command)` for a command that joins the current step, plus the
//!   builders `queue_add_component`, `queue_remove_component`,
//!   `queue_create_entity`, `queue_destroy_entity`, `queue_selection` and
//!   `queue_gizmo`, which queue exactly what the editor's own rows queue;
//! - `post_notice(level, text)` for a message the editor shows as a toast.
//!
//! What a verdict does: [`HookVerdict::Keep`] applies the edit as planned,
//! [`HookVerdict::Replace`] applies another value and keeps the planned one as
//! the step's inverse, and [`HookVerdict::Refuse`] drops the whole batch with
//! an editor notice and logs `[editor] hook refused`.
//!
//! Which commands record: a component add or remove, an entity create or
//! destroy, a selection change and a field write are edits, so undo walks back
//! over them. A gizmo change is a control, so it never records. An edit a
//! callback queues joins the step of the edit it answered; an edit queued by a
//! callback answering a difference (the selection, the tool, the element slot)
//! becomes one step of its own, labelled `hook {source}`.
//!
//! Reload rules: `register` runs on every generation, so a reload rebinds the
//! arriving image's callbacks in place of the old ones and no event fires
//! twice. A command queued under [`FieldEditCause::HistoryRestored`] is
//! dropped, because the step being replayed already holds the result, and the
//! drop is reported once per drain.

/// What a callback reads and queues while it runs.
pub mod context;
/// The frame-boundary dispatcher that runs the callbacks.
pub mod dispatch;
/// The payloads a callback receives and the verdict it answers.
pub mod events;
/// The callbacks a module registered, and their liveness markers.
pub mod registry;
/// The queue a module registers its callbacks through.
pub mod requests;
/// The last reported selection and tool state, for the diff-driven events.
pub mod watch;

pub use context::{HookContext, HookNotice, HookNotices};
pub use dispatch::HookSession;
pub use events::{
    AssetEdit, AssetEditKind, ComponentEdit, ElementSelectionChanged, EntityEdit, FieldEdit,
    FieldEditCause, FieldEditPhase, FieldEditView, HistoryEdit, HookVerdict, PlayEdit, SceneEdit,
    SelectionChanged, SelectionSnapshot, ToolActivatedEdit,
};
pub use registry::{
    EditorHookRegistry, FieldEditAfterHook, FieldEditBeforeHook, HookCallback, HookEvent,
    HookRegistration,
};
pub use requests::{EditorHookRequests, FieldEditHookRequest, HookRequest};
pub use watch::HookWatch;

#[cfg(test)]
mod hooks_tests;
