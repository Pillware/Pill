//! The payloads the editor API's callbacks receive.
//!
//! # Responsibilities
//!
//! - Say which part of a field interaction an edit belongs to
//!   ([`FieldEditPhase`]) and what made it happen ([`FieldEditCause`]).
//! - Define the type-erased field payload the registry hands around
//!   ([`FieldEditView`]) and the typed view a callback reads ([`FieldEdit`]).
//! - Define what a before-callback decides ([`HookVerdict`]).
//! - Erase a typed callback to a plain `fn` pointer, and call it back with the
//!   component type it was registered for.

// Standard library
use std::any::type_name;

// External crates
use pill_engine::component_field::FieldValue;
use pill_engine::{ComponentFieldId, ComponentFields, Entity};

// Current crate
use crate::hooks::HookContext;

// =============================================================================
// Phases and causes
// =============================================================================

/// Which part of one field interaction an edit belongs to.
///
/// A drag or scrub reports [`Self::Interactive`] on every frame it changes
/// something and one [`Self::Committed`] when the pointer is released. A
/// checkbox, an enum pick, an asset pick, a text field that lost focus and a
/// scripted or panel write report one [`Self::Committed`] edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEditPhase {
    /// One frame of a continuous interaction.
    Interactive,
    /// The interaction ended, or it was a one-shot edit.
    Committed,
}

impl FieldEditPhase {
    /// The phase's wire name, as the log lines spell it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Committed => "committed",
        }
    }
}

/// What made a field edit happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEditCause {
    /// A person or a script authored the edit.
    Authored,
    /// The editor replayed a history step; the step already holds the result,
    /// so a callback's follow-ups are dropped.
    HistoryRestored,
}

impl FieldEditCause {
    /// The cause's wire name, as the log lines spell it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Authored => "authored",
            Self::HistoryRestored => "history_restored",
        }
    }
}

// =============================================================================
// Verdict
// =============================================================================

/// What a before-callback decides about a field edit.
#[derive(Clone, Debug, PartialEq)]
pub enum HookVerdict {
    /// Apply the edit as planned.
    Keep,
    /// Apply this value instead; the edit's history step keeps the original
    /// value as its inverse.
    Replace(FieldValue),
    /// Refuse the edit. The whole batch it belongs to is dropped, nothing is
    /// recorded, and the reason becomes an editor notice.
    Refuse(String),
}

// =============================================================================
// Payloads
// =============================================================================

/// One field edit, as the type-erased registry hands it to a callback.
///
/// `component` and `field_name` borrow the queued command for the call only;
/// the values are owned copies.
#[derive(Clone, Debug)]
pub struct FieldEditView<'a> {
    /// The entity the edit targets.
    pub entity: Entity,
    /// The component's registered type name.
    pub component: &'a str,
    /// The field's name, as the field layer spells it.
    pub field_name: &'a str,
    /// The array element the edit targets, when it targets one.
    pub index: Option<usize>,
    /// The value before the edit.
    pub previous: FieldValue,
    /// The value the edit proposes for a before-callback, or the applied value
    /// for an after-callback.
    pub current: FieldValue,
    /// What made the edit happen.
    pub cause: FieldEditCause,
    /// The interaction phase the edit belongs to.
    pub phase: FieldEditPhase,
    /// Whether the entity is the selection's primary.
    pub is_primary: bool,
}

/// One field edit, typed by the component and its generated field enum.
///
/// A callback registered with [`FieldEditHookRequest::new`](crate::hooks::FieldEditHookRequest::new)
/// receives this view only for the component type it named; an edit of another
/// component, or of a field the type does not have, never reaches it.
pub struct FieldEdit<'a, T: ComponentFields> {
    /// The entity the edit targets.
    pub entity: Entity,
    /// The field, as the component's generated enum names it.
    pub field: T::Field,
    /// The field's text name, for log lines.
    pub field_name: &'a str,
    /// The array element the edit targets, when it targets one.
    pub index: Option<usize>,
    /// The value before the edit.
    pub previous: FieldValue,
    /// The value the edit proposes for a before-callback, or the applied value
    /// for an after-callback.
    pub current: FieldValue,
    /// What made the edit happen.
    pub cause: FieldEditCause,
    /// The interaction phase the edit belongs to.
    pub phase: FieldEditPhase,
    /// Whether the entity is the selection's primary.
    pub is_primary: bool,
}

// =============================================================================
// Erasure
// =============================================================================

/// Whether `component` names `T`, by its registered name or its Rust path.
///
/// A command carries the name the world registered the component under; an
/// editor test may queue the Rust path instead, so both are accepted.
pub(crate) fn component_matches<T: ComponentFields>(component: &str) -> bool {
    component == pill_engine::component::ComponentRegistry::registered_name::<T>()
        || component == type_name::<T>()
}

/// Calls one typed before-callback with the view of the component it named.
///
/// # Safety
///
/// `callback` must be a `fn(&mut HookContext<'_>, &FieldEdit<'_, T>) ->
/// HookVerdict` stored by [`FieldEditBeforeHook::of::<T>`] with this same `T`.
/// The registry only ever pairs a callback with the applier it was stored with,
/// and drops both together when the registering module unloads.
pub(crate) unsafe fn apply_field_edit_before<T: ComponentFields>(
    callback: usize,
    context: &mut HookContext<'_>,
    view: &FieldEditView<'_>,
) -> HookVerdict {
    if !component_matches::<T>(view.component) {
        return HookVerdict::Keep;
    }
    let Some(field) = T::Field::from_name(view.field_name) else {
        return HookVerdict::Keep;
    };
    // SAFETY: the caller's contract states that `callback` names a function
    // with exactly this signature for this `T`.
    let callback: fn(&mut HookContext<'_>, &FieldEdit<'_, T>) -> HookVerdict =
        unsafe { core::mem::transmute(callback) };
    let edit = FieldEdit {
        entity: view.entity,
        field,
        field_name: view.field_name,
        index: view.index,
        previous: view.previous.clone(),
        current: view.current.clone(),
        cause: view.cause,
        phase: view.phase,
        is_primary: view.is_primary,
    };
    callback(context, &edit)
}

/// Calls one typed after-callback with the view of the component it named.
///
/// # Safety
///
/// `callback` must be a `fn(&mut HookContext<'_>, &FieldEdit<'_, T>)` stored by
/// [`FieldEditAfterHook::of::<T>`] with this same `T`, under the same rule as
/// [`apply_field_edit_before`].
pub(crate) unsafe fn apply_field_edit_after<T: ComponentFields>(
    callback: usize,
    context: &mut HookContext<'_>,
    view: &FieldEditView<'_>,
) {
    if !component_matches::<T>(view.component) {
        return;
    }
    let Some(field) = T::Field::from_name(view.field_name) else {
        return;
    };
    // SAFETY: the caller's contract states that `callback` names a function
    // with exactly this signature for this `T`.
    let callback: fn(&mut HookContext<'_>, &FieldEdit<'_, T>) =
        unsafe { core::mem::transmute(callback) };
    let edit = FieldEdit {
        entity: view.entity,
        field,
        field_name: view.field_name,
        index: view.index,
        previous: view.previous.clone(),
        current: view.current.clone(),
        cause: view.cause,
        phase: view.phase,
        is_primary: view.is_primary,
    };
    callback(context, &edit);
}
