//! The host's file watchers: one worker per watched tree, each reporting to
//! the main thread at a frame boundary.
//!
//! # Responsibilities
//!
//! - Hold the machinery every watcher shares: the debounced worker thread, the
//!   rule for which file events count as an edit, and the trigger log a reload
//!   is timed from.
//! - Watch the project's source tree for edits that start a reload.
//! - Watch the project's asset directory, so an edited asset is imported again
//!   while the host runs.
//! - Watch a shader source tree, so an edited `.hlsl` is cooked again while the
//!   host runs.
//!
//! # Design
//!
//! All three have the same shape: a worker thread that runs host code only, a
//! channel the main thread drains at the frame boundary, and no call into a
//! module image. A reload can therefore swap an image at any moment without
//! leaving a thread inside code that is going away. What they watch, and what
//! they queue, is the only thing that differs.

// =============================================================================
// Module Declarations
// =============================================================================

/// The project's asset directory: an edit there becomes an [`AssetChange`] the
/// main thread imports again.
mod asset;
/// A shader source tree: an edit is cooked again and queued as one output per
/// changed file.
#[cfg(feature = "rendering")]
mod shader;
/// The settled watcher every worker is built on, the event filter they share,
/// and the project's source-tree watcher.
mod shared;

// =============================================================================
// Re-exports
// =============================================================================

/// What an edit in the project's asset directory means for the source asset it
/// belongs to.
pub(crate) use asset::AssetChange;
/// Watches the project's asset directory and queues the changes it sees.
pub(crate) use asset::AssetWatcher;
/// Watches one shader source tree, cooks an edited tree on a worker thread and
/// queues the results.
#[cfg(feature = "rendering")]
pub(crate) use shader::ShaderWatcher;
/// Spawns the worker that watches one tree and bumps the reload generation on
/// an edit.
pub(crate) use shared::spawn_source_watcher;
/// What fired a reload: the paths that changed and when the watcher saw them.
pub(crate) use shared::SourceTrigger;
/// The most recent triggers, kept so the frame loop can time a reload from the
/// save that caused it.
pub(crate) use shared::TriggerLog;
