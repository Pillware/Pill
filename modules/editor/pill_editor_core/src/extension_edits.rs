//! Undoable edits whose state only an editor extension understands.
//!
//! # Responsibilities
//!
//! - Carry an extension's opaque state as [`ExtensionPayload`], cheap to
//!   clone and to keep in the history.
//! - Build the history step for such an edit ([`extension_edit_entry`]):
//!   the forward command applies the state after the edit, the inverse
//!   applies the state before it.
//! - Hand each replayed state back to its extension through
//!   [`ExtensionEditInbox`], keyed by the channel the extension named.
//!
//! # Design
//!
//! An extension here is any editor crate the host loads beside the editor:
//! an extension's editor crate, or a project's own editor crate.
//!
//! An extension changes something the editor's command vocabulary cannot
//! name - a mesh's geometry streamed in from another application - and still
//! wants Undo and Redo to reach it. It records the step itself, with the
//! state on both sides, and the history replays it like any other step. The
//! inverse batch runs through the frame-boundary drain, whose
//! `EditorCommand::ExtensionEdit` arm queues the payload here instead of
//! touching the world, and the extension applies it on its next update. The
//! editor never reads a payload; only the extension that wrote it does,
//! which is why the channel is a plain string.

// Standard library
use std::sync::Arc;

// External crates
use pill_engine::Resource;

// Current crate
use crate::editor_state::EditorCommand;
use crate::history::HistoryEntry;

// =============================================================================
// Constants
// =============================================================================

/// How many frames a delivery waits for its extension before it is dropped, so
/// the edits of an extension that unloaded cannot fill the inbox.
pub const EXTENSION_EDIT_DELIVERY_LIFETIME: u32 = 600;

// =============================================================================
// Types
// =============================================================================

/// An extension's encoding of one state, shared between the history step that
/// holds it and the delivery that replays it.
#[derive(Clone, PartialEq, Eq)]
pub struct ExtensionPayload(pub Arc<Vec<u8>>);

impl ExtensionPayload {
    /// Wraps the bytes an extension encoded.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Arc::new(bytes))
    }

    /// The encoded bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    /// How many bytes the payload holds; the history's memory budget counts
    /// them.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the payload holds no bytes.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// A payload can be megabytes of geometry, so it never prints its bytes.
impl std::fmt::Debug for ExtensionPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ExtensionPayload({} bytes)", self.0.len())
    }
}

/// One replayed state, waiting for the extension that owns its channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionEditDelivery {
    /// The channel the extension recorded the edit on.
    pub channel: String,
    /// What the edit is about, in the extension's own terms (a link id).
    pub subject: String,
    /// The state to apply.
    pub payload: ExtensionPayload,
}

/// The replayed extension states the drain queued since the extensions last
/// looked.
#[derive(Debug, Default)]
pub struct ExtensionEditInbox {
    /// The deliveries, oldest first, each with the frames it has waited.
    deliveries: Vec<(ExtensionEditDelivery, u32)>,
}

impl ExtensionEditInbox {
    /// Queues one replayed state, as the drain does when it applies a
    /// `ExtensionEdit` command.
    pub fn push(&mut self, delivery: ExtensionEditDelivery) {
        self.deliveries.push((delivery, 0));
    }

    /// Takes every queued delivery on `channel`, oldest first.
    pub fn take(&mut self, channel: &str) -> Vec<ExtensionEditDelivery> {
        let mut taken = Vec::new();
        self.deliveries.retain(|(delivery, _)| {
            if delivery.channel == channel {
                taken.push(delivery.clone());
                false
            } else {
                true
            }
        });
        taken
    }

    /// Ages every waiting delivery by one frame and drops the ones nobody
    /// took within [`EXTENSION_EDIT_DELIVERY_LIFETIME`].
    pub fn age(&mut self) {
        self.deliveries.retain_mut(|(_, frames_waited)| {
            *frames_waited += 1;
            *frames_waited < EXTENSION_EDIT_DELIVERY_LIFETIME
        });
    }

    /// Returns how many deliveries are waiting.
    pub fn len(&self) -> usize {
        self.deliveries.len()
    }

    /// Returns whether no delivery is waiting.
    pub fn is_empty(&self) -> bool {
        self.deliveries.is_empty()
    }

    /// Inserts the inbox into `world` when it holds none, so the drain always
    /// has somewhere to queue a replayed state.
    pub fn ensure(world: &mut pill_engine::World) {
        world.register_resource::<ExtensionEditInbox>();
        if world.get_resource::<ExtensionEditInbox>().is_none() {
            world.insert_resource(ExtensionEditInbox::default());
        }
    }
}

impl Resource for ExtensionEditInbox {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds the history step for an extension edit that already happened.
///
/// The extension pushes the step into the history itself, because the edit is
/// already in place: nothing has to be applied now. Undo then delivers
/// `before` on `channel`, and Redo delivers `after`.
pub fn extension_edit_entry(
    label: impl Into<String>,
    channel: &str,
    subject: &str,
    before: ExtensionPayload,
    after: ExtensionPayload,
) -> HistoryEntry {
    let forward = EditorCommand::ExtensionEdit {
        channel: channel.to_string(),
        subject: subject.to_string(),
        payload: after.clone(),
        previous_payload: before.clone(),
    };
    let inverse = EditorCommand::ExtensionEdit {
        channel: channel.to_string(),
        subject: subject.to_string(),
        payload: before,
        previous_payload: after,
    };
    HistoryEntry::engine(label, vec![forward], vec![inverse])
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extension_takes_only_its_own_channel() {
        let mut inbox = ExtensionEditInbox::default();
        for channel in ["mesh", "other", "mesh"] {
            inbox.push(ExtensionEditDelivery {
                channel: channel.to_string(),
                subject: "1".to_string(),
                payload: ExtensionPayload::new(vec![1]),
            });
        }
        assert_eq!(inbox.take("mesh").len(), 2);
        assert_eq!(inbox.len(), 1);
    }

    #[test]
    fn a_delivery_nobody_takes_ages_out() {
        let mut inbox = ExtensionEditInbox::default();
        inbox.push(ExtensionEditDelivery {
            channel: "gone".to_string(),
            subject: "1".to_string(),
            payload: ExtensionPayload::new(Vec::new()),
        });
        for _ in 0..EXTENSION_EDIT_DELIVERY_LIFETIME {
            inbox.age();
        }
        assert!(inbox.is_empty());
    }

    /// The step's inverse delivers the state before the edit, and its forward
    /// command the state after it.
    #[test]
    fn the_entry_swaps_the_payloads_for_its_inverse() {
        let before = ExtensionPayload::new(vec![1]);
        let after = ExtensionPayload::new(vec![2]);
        let entry = extension_edit_entry("edit", "mesh", "7", before.clone(), after.clone());
        let crate::history::HistoryEntryKind::Engine { forward, inverse } = entry.entry else {
            panic!("an extension edit is an engine step");
        };
        assert!(
            matches!(&forward[0], EditorCommand::ExtensionEdit { payload, .. } if *payload == after)
        );
        assert!(
            matches!(&inverse[0], EditorCommand::ExtensionEdit { payload, .. } if *payload == before)
        );
    }
}
