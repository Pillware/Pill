//! The callbacks a module registered, and the marker that keeps them honest.
//!
//! # Responsibilities
//!
//! - Name the events a callback can answer ([`HookEvent`]).
//! - Hold each registration with the callback's erased function pointer and
//!   the liveness marker of the crate that registered it
//!   ([`HookRegistration`]).
//! - Hand the live callbacks of one event to the drain ([`EditorHookRegistry`]).
//!
//! # Design
//!
//! The registry is a world resource of the shared `dylib`, so every editor
//! crate reads one identity. A registration stores owned strings and `fn`
//! pointers only; the executable keeps the strong
//! [`ModuleLiveness`](crate::panels::ModuleLiveness) handle and the registry a
//! [`Weak`], which is the panel contract: a callback address points into the
//! registering image, and a strong count of zero means that image is gone.
//! Registering again from the same crate replaces the callback it registered
//! for the same event, so a reload cannot double a callback.

// Standard library
use std::sync::{Arc, Weak};

// Current crate
use crate::hooks::events::{apply_field_edit_after, apply_field_edit_before};
use crate::hooks::{FieldEditView, HookContext, HookVerdict};
use crate::panels::ModuleLiveness;

// =============================================================================
// Types
// =============================================================================

/// One event a callback can answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// Before a field edit applies; the callback may replace or refuse it.
    FieldEditBefore,
    /// After a field edit applied; the callback reacts and may queue
    /// follow-ups.
    FieldEditAfter,
}

impl HookEvent {
    /// The event's wire name, as the log lines spell it.
    pub fn name(self) -> &'static str {
        match self {
            Self::FieldEditBefore => "field-edit-before",
            Self::FieldEditAfter => "field-edit-after",
        }
    }
}

/// A field-edit before-callback, erased to plain data.
///
/// The callback is stored as an address because a `fn` pointer with the
/// component's type parameters has no common type across components. The
/// applier that turns it back into the typed function is stored beside it, so
/// the pair can only have been built by
/// [`FieldEditBeforeHook::of`](Self) with one `T`.
#[derive(Clone, Copy)]
pub struct FieldEditBeforeHook {
    /// The typed callback, as an address; see [`HookCallback`].
    callback: usize,
    /// The monomorphized applier that casts the address back and calls it.
    apply: unsafe fn(usize, &mut HookContext<'_>, &FieldEditView<'_>) -> HookVerdict,
}

impl FieldEditBeforeHook {
    /// Erases one typed before-callback for component `T`.
    pub(crate) fn of<T: pill_engine::ComponentFields>(
        callback: fn(&mut HookContext<'_>, &crate::hooks::FieldEdit<'_, T>) -> HookVerdict,
    ) -> Self {
        Self {
            callback: callback as usize,
            apply: apply_field_edit_before::<T>,
        }
    }
}

impl std::fmt::Debug for FieldEditBeforeHook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FieldEditBeforeHook")
            .field("callback", &self.callback)
            .finish()
    }
}

/// A field-edit after-callback, erased to plain data.
#[derive(Clone, Copy)]
pub struct FieldEditAfterHook {
    /// The typed callback, as an address; see [`HookCallback`].
    callback: usize,
    /// The monomorphized applier that casts the address back and calls it.
    apply: unsafe fn(usize, &mut HookContext<'_>, &FieldEditView<'_>),
}

impl FieldEditAfterHook {
    /// Erases one typed after-callback for component `T`.
    pub(crate) fn of<T: pill_engine::ComponentFields>(
        callback: fn(&mut HookContext<'_>, &crate::hooks::FieldEdit<'_, T>),
    ) -> Self {
        Self {
            callback: callback as usize,
            apply: apply_field_edit_after::<T>,
        }
    }
}

impl std::fmt::Debug for FieldEditAfterHook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FieldEditAfterHook")
            .field("callback", &self.callback)
            .finish()
    }
}

/// The erased callback of one registration.
#[derive(Clone, Copy, Debug)]
pub enum HookCallback {
    /// A before-callback for a field edit.
    FieldEditBefore(FieldEditBeforeHook),
    /// An after-callback for a field edit.
    FieldEditAfter(FieldEditAfterHook),
}

impl HookCallback {
    /// The event this callback answers.
    pub fn event(self) -> HookEvent {
        match self {
            Self::FieldEditBefore(_) => HookEvent::FieldEditBefore,
            Self::FieldEditAfter(_) => HookEvent::FieldEditAfter,
        }
    }

    /// Calls the callback with one field edit.
    ///
    /// # Safety
    ///
    /// The callback and its applier are a pair the request constructors built
    /// together; the applier expects the address it was stored with.
    pub(crate) unsafe fn call_field_edit(
        self,
        context: &mut HookContext<'_>,
        view: &FieldEditView<'_>,
    ) -> Option<HookVerdict> {
        match self {
            Self::FieldEditBefore(before) => {
                // SAFETY: the request constructors store a callback and its
                // monomorphized applier as one pair, and neither is ever
                // replaced without the other.
                Some(unsafe { (before.apply)(before.callback, context, view) })
            }
            Self::FieldEditAfter(after) => {
                // SAFETY: same contract as the before arm.
                unsafe { (after.apply)(after.callback, context, view) };
                None
            }
        }
    }
}

/// One registered callback and the marker of the crate that registered it.
#[derive(Clone, Debug)]
pub struct HookRegistration {
    /// The crate that registered it.
    pub source: String,
    /// The event it answers.
    pub event: HookEvent,
    /// The erased callback.
    pub callback: HookCallback,
    /// The marker of the registering module: it goes dead when the module
    /// unloads.
    liveness: Weak<ModuleLiveness>,
}

impl HookRegistration {
    /// Returns whether the module that registered this callback is still
    /// loaded.
    pub fn alive(&self) -> bool {
        self.liveness.strong_count() > 0
    }
}

/// One live callback, copied out of the registry for the drain.
#[derive(Clone, Debug)]
pub(crate) struct LiveHook {
    /// The crate that registered it, as the log lines name it.
    pub(crate) source: String,
    /// The erased callback.
    pub(crate) callback: HookCallback,
}

/// Every callback a module registered, live or not.
#[derive(Debug, Default)]
pub struct EditorHookRegistry {
    /// The registrations, oldest first.
    registrations: Vec<HookRegistration>,
}

impl EditorHookRegistry {
    /// Registers one callback, replacing the callback the same crate registered
    /// for the same event.
    ///
    /// Dead registrations are dropped on the way through, so a reload leaves
    /// one live entry per crate and event instead of one per generation.
    pub fn register(
        &mut self,
        source: &str,
        event: HookEvent,
        callback: HookCallback,
        liveness: &Arc<ModuleLiveness>,
    ) {
        self.registrations.retain(|registration| {
            registration.alive() && !(registration.source == source && registration.event == event)
        });
        self.registrations.push(HookRegistration {
            source: source.to_string(),
            event,
            callback,
            liveness: Arc::downgrade(liveness),
        });
    }

    /// The live callbacks of one event, in registration order.
    pub(crate) fn live(&self, event: HookEvent) -> Vec<LiveHook> {
        self.registrations
            .iter()
            .filter(|registration| registration.event == event && registration.alive())
            .map(|registration| LiveHook {
                source: registration.source.clone(),
                callback: registration.callback,
            })
            .collect()
    }

    /// Returns every registration, in registration order.
    pub fn entries(&self) -> &[HookRegistration] {
        &self.registrations
    }

    /// Registers the resource, inserting a default only when the world has
    /// none, so a reload keeps the live registrations.
    pub fn ensure(world: &mut pill_engine::World) {
        world.register_resource::<Self>();
        if world.get_resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
    }
}

impl pill_engine::Resource for EditorHookRegistry {
    /// Infrastructure, not game state: Play leaves this value alone and Stop
    /// does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_editor_core::hooks::EditorHookRegistry")
    }
}
