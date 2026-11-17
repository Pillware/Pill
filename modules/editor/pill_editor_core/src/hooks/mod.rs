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
//! - Give a callback its context ([`HookContext`]): the world, the selection,
//!   the follow-up queue and the notices it may post.
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

pub use context::{HookContext, HookNotice, HookNotices};
pub use dispatch::HookSession;
pub use events::{FieldEdit, FieldEditCause, FieldEditPhase, FieldEditView, HookVerdict};
pub use registry::{
    EditorHookRegistry, FieldEditAfterHook, FieldEditBeforeHook, HookCallback, HookEvent,
    HookRegistration,
};
pub use requests::{EditorHookRequests, FieldEditHookRequest, HookRequest};

#[cfg(test)]
mod hooks_tests;
