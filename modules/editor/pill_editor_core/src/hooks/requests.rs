//! The queue a module registers its callbacks through.
//!
//! # Responsibilities
//!
//! - Define the plain request a module queues ([`HookRequest`]) and the
//!   resource the executable drains ([`EditorHookRequests`]).
//! - Provide the typed constructors an `_editor` crate calls
//!   ([`FieldEditHookRequest`]).
//!
//! # Design
//!
//! A request is taken once and never read twice, so a request from a module
//! that has unloaded cannot reach a live entry. The typed constructors
//! monomorphize their adapter in the calling crate, which is what keeps the
//! callback address inside the registering image: the executable binds the
//! request to that image's liveness marker and drops both together.

// External crates
use pill_engine::{ComponentFields, Resource};

// Current crate
use crate::hooks::events::{
    AssetEdit, ComponentEdit, ElementSelectionChanged, EntityEdit, HistoryEdit, HookVerdict,
    PlayEdit, SceneEdit, SelectionChanged, ToolActivatedEdit,
};
use crate::hooks::registry::{FieldEditAfterHook, FieldEditBeforeHook, HookCallback, HookEvent};
use crate::hooks::{FieldEdit, HookContext};

// =============================================================================
// Requests
// =============================================================================

/// One callback a module asks the editor to register.
///
/// Built by [`FieldEditHookRequest`], never by hand: the callback and its
/// applier are a pair only the constructors can form.
#[derive(Clone, Copy, Debug)]
pub struct HookRequest {
    /// The crate registering it.
    source: &'static str,
    /// The event it answers.
    event: HookEvent,
    /// The erased callback.
    callback: HookCallback,
}

impl HookRequest {
    /// The crate registering it.
    pub fn source(&self) -> &'static str {
        self.source
    }

    /// The event it answers.
    pub fn event(&self) -> HookEvent {
        self.event
    }

    /// The erased callback.
    pub fn callback(&self) -> HookCallback {
        self.callback
    }

    /// One request whose callback is already a plain function pointer.
    fn plain(source: &'static str, callback: HookCallback) -> HookRequest {
        HookRequest {
            source,
            event: callback.event(),
            callback,
        }
    }
}

/// The callback requests queued since the executable last looked.
///
/// A request is taken once, so one queued by a module that has since unloaded
/// cannot be bound to a live entry.
#[derive(Debug, Default)]
pub struct EditorHookRequests {
    /// The requests, oldest first.
    pending: Vec<HookRequest>,
}

impl EditorHookRequests {
    /// Queues one request.
    pub fn push(&mut self, request: HookRequest) {
        self.pending.push(request);
    }

    /// Takes every queued request and leaves the queue empty.
    pub fn drain(&mut self) -> Vec<HookRequest> {
        std::mem::take(&mut self.pending)
    }

    /// Registers the resource, inserting a default only when the world has
    /// none.
    pub fn ensure(world: &mut pill_engine::World) {
        world.register_resource::<Self>();
        if world.get_resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
    }
}

impl Resource for EditorHookRequests {
    /// Infrastructure, not game state: Play leaves this value alone and Stop
    /// does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_editor_core::hooks::EditorHookRequests")
    }
}

// =============================================================================
// Typed constructors
// =============================================================================

/// The typed field-edit constructors an `_editor` crate calls.
///
/// The `source` is the crate's own package name
/// (`env!("CARGO_PKG_NAME")`): the executable keys the liveness marker by it,
/// and the log lines name it.
pub struct FieldEditHookRequest;

// `new` returns the plain request a module queues, not a `FieldEditHookRequest`:
// the type only names the constructors, and the request is what the queue
// takes.
#[allow(clippy::new_ret_no_self)]
impl FieldEditHookRequest {
    /// Builds a before-callback for component `T`'s fields.
    ///
    /// The callback runs before the edit applies and may return
    /// [`HookVerdict::Replace`] to write another value or
    /// [`HookVerdict::Refuse`] to drop the batch and post a notice.
    pub fn new<T: ComponentFields>(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &FieldEdit<'_, T>) -> HookVerdict,
    ) -> HookRequest {
        HookRequest {
            source,
            event: HookEvent::FieldEditBefore,
            callback: HookCallback::FieldEditBefore(FieldEditBeforeHook::of::<T>(callback)),
        }
    }

    /// Builds an after-callback for component `T`'s fields.
    ///
    /// The callback runs after the edit applied, reads the applied value from
    /// its view, and may queue follow-up commands that join the same undo step.
    pub fn after<T: ComponentFields>(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &FieldEdit<'_, T>),
    ) -> HookRequest {
        HookRequest {
            source,
            event: HookEvent::FieldEditAfter,
            callback: HookCallback::FieldEditAfter(FieldEditAfterHook::of::<T>(callback)),
        }
    }
}

/// The plain-event constructors an `_editor` crate calls.
///
/// Each event carries one payload and the callback is a plain function
/// pointer, so nothing has to be monomorphized on the caller's side.
impl HookRequest {
    /// Builds a component add/remove callback.
    pub fn component_edit(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &ComponentEdit<'_>),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::ComponentEdit(callback))
    }
    /// Builds an entity create/destroy callback.
    pub fn entity_edit(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &EntityEdit),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::EntityEdit(callback))
    }

    /// Builds an asset-file edit callback.
    pub fn asset_edit(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &AssetEdit<'_>),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::AssetEdit(callback))
    }

    /// Builds an undo/redo callback.
    pub fn history_edit(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &HistoryEdit<'_>),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::HistoryEdit(callback))
    }

    /// Builds a selection-change callback.
    pub fn selection_changed(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &SelectionChanged),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::SelectionChanged(callback))
    }

    /// Builds a scene open/new callback.
    pub fn scene_edit(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &SceneEdit<'_>),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::SceneEdit(callback))
    }

    /// Builds a play/stop callback.
    pub fn play_edit(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &PlayEdit),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::PlayEdit(callback))
    }

    /// Builds an active-tool callback.
    pub fn tool_activated(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &ToolActivatedEdit),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::ToolActivated(callback))
    }

    /// Builds an element-selection callback.
    pub fn element_selection(
        source: &'static str,
        callback: fn(&mut HookContext<'_>, &ElementSelectionChanged),
    ) -> HookRequest {
        HookRequest::plain(source, HookCallback::ElementSelectionChanged(callback))
    }
}
