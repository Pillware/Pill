//! Applies the extension command family.
//!
//! # Responsibilities
//!
//! - Apply `ExtensionEdit` by queueing its payload for the extension that
//!   recorded it, and answer with the inverse that queues the state it
//!   replaced.
//!
//! # Design
//!
//! One `match` over the family's own variant, called from
//! [`EditorCommand::apply_one`](super::EditorCommand). The world is never
//! touched here: only the extension can read the payload, so the extension
//! applies it on its next update. A command of another family hits
//! `unreachable!`.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::Engine;

// Current crate
use super::CommandOutcome;
use super::*;
use crate::extension_edits::{ExtensionEditDelivery, ExtensionEditInbox};

// =============================================================================
// Free Functions
// =============================================================================

/// Applies one extension command.
///
/// # Errors
///
/// Returns a message when the world holds no `ExtensionEditInbox`, so a replay
/// that could not reach its extension rolls the history step back.
pub(super) fn apply(
    engine: &mut Engine,
    command: &EditorCommand,
) -> Result<CommandOutcome, String> {
    match command {
        EditorCommand::ExtensionEdit {
            channel,
            subject,
            payload,
            previous_payload,
        } => {
            let world = engine.world_mut();
            let Some(inbox) = world.get_resource_mut::<ExtensionEditInbox>() else {
                return Err(format!("no extension edit inbox for channel {channel}"));
            };
            inbox.push(ExtensionEditDelivery {
                channel: channel.clone(),
                subject: subject.clone(),
                payload: payload.clone(),
            });
            pill_core::info!(
                target: telemetry_target::EDITOR,
                channel = channel.as_str(),
                subject = subject.as_str(),
                bytes = payload.len(),
                "[editor] extension edit queued"
            );
            Ok(CommandOutcome::inverse(EditorCommand::ExtensionEdit {
                channel: channel.clone(),
                subject: subject.clone(),
                payload: previous_payload.clone(),
                previous_payload: payload.clone(),
            }))
        }
        _ => unreachable!("not an extension command: {command:?}"),
    }
}
