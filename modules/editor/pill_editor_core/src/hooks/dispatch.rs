//! Runs the registered callbacks at the frame boundary.
//!
//! # Responsibilities
//!
//! - Snapshot the live callbacks before a drain touches the world
//!   ([`HookSession::new`]).
//! - Run the before-callbacks over a whole batch
//!   ([`HookSession::prepare_field_batch`]), applying their replacements and
//!   dropping the batch on a refusal.
//! - Run the after-callbacks per applied command
//!   ([`HookSession::after_field_command`]) and hold the commands they queue.
//! - Publish the notices the callbacks posted ([`HookNotices`]).
//!
//! # Design
//!
//! The session copies the live callbacks out of the registry up front, so the
//! world can be borrowed mutably for a callback without borrowing the registry
//! it came from. Before-callbacks run over the entire batch first, which is
//! what makes a refusal drop the whole batch instead of leaving part of a
//! multi-selection edit applied and unrecorded. Follow-ups are applied by the
//! observed `apply_recorded`, never through the session, so a follow-up cannot
//! re-enter a callback.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{Engine, Entity, World};

// Current crate
use crate::editor_state::{CommandObserver, EditorCommand, EditorSelection};
use crate::hooks::context::{HookContext, HookNotice, HookNotices};
use crate::hooks::events::{FieldEditCause, FieldEditPhase, FieldEditView, HookVerdict};
use crate::hooks::registry::{EditorHookRegistry, HookEvent, LiveHook};
use crate::notices::NoticeLevel;

// =============================================================================
// The session
// =============================================================================

/// One drain's callbacks, notices and follow-ups.
pub struct HookSession {
    /// The live before-callbacks, copied out of the registry.
    before: Vec<LiveHook>,
    /// The live after-callbacks, copied out of the registry.
    after: Vec<LiveHook>,
    /// The notices the callbacks posted this drain.
    notices: Vec<HookNotice>,
    /// The commands the callbacks queued for the current command.
    followups: Vec<EditorCommand>,
    /// What made the current batch happen.
    cause: FieldEditCause,
    /// The interaction phase of the current batch.
    phase: FieldEditPhase,
}

impl HookSession {
    /// Builds a session from the world's live registrations.
    pub(crate) fn new(world: &World) -> Self {
        let (before, after) = match world.get_resource::<EditorHookRegistry>() {
            Some(registry) => (
                registry.live(HookEvent::FieldEditBefore),
                registry.live(HookEvent::FieldEditAfter),
            ),
            None => (Vec::new(), Vec::new()),
        };
        Self {
            before,
            after,
            notices: Vec::new(),
            followups: Vec::new(),
            cause: FieldEditCause::Authored,
            phase: FieldEditPhase::Committed,
        }
    }

    /// Runs the before-callbacks over one batch.
    ///
    /// Returns `false` when a callback refused the batch: nothing may apply, no
    /// history step may be recorded, and the refusal is already a notice and a
    /// log line.
    pub(crate) fn prepare_field_batch(
        &mut self,
        engine: &mut Engine,
        commands: &mut [EditorCommand],
        cause: FieldEditCause,
        phase: FieldEditPhase,
    ) -> bool {
        self.cause = cause;
        self.phase = phase;
        if self.before.is_empty() {
            return true;
        }
        for command in commands.iter_mut() {
            let Some(entity) = field_command_entity(command) else {
                continue;
            };
            let selection = selection_copy(engine.world());
            let primary = is_primary(&selection, entity);
            let Some(mut view) = before_view(engine.world(), command, cause, phase, primary) else {
                continue;
            };
            let mut replacement: Option<pill_engine::component_field::FieldValue> = None;
            for hook in &self.before {
                let selection = selection_copy(engine.world());
                let mut context = HookContext::new(
                    engine.world_mut(),
                    &selection,
                    &mut self.followups,
                    &mut self.notices,
                    cause,
                );
                // SAFETY: the request constructors store a callback and its
                // applier as one pair, and the registry copies both together.
                let verdict = unsafe { hook.callback.call_field_edit(&mut context, &view) };
                match verdict {
                    Some(HookVerdict::Keep) | None => {}
                    Some(HookVerdict::Replace(value)) => {
                        // A replay must reproduce the step, not a callback's
                        // opinion of it: the step already holds the result.
                        if cause != FieldEditCause::HistoryRestored {
                            view.current = value.clone();
                            replacement = Some(value);
                        }
                    }
                    Some(HookVerdict::Refuse(reason)) => {
                        // A callback may not veto the history it is watching;
                        // the remaining callbacks still see the edit.
                        if cause == FieldEditCause::HistoryRestored {
                            continue;
                        }
                        self.notices.push(HookNotice {
                            level: NoticeLevel::Warning,
                            text: format!("{} refused the edit: {reason}", hook.source),
                        });
                        pill_core::warn!(
                            target: telemetry_target::EDITOR,
                            source = hook.source.as_str(),
                            event = HookEvent::FieldEditBefore.name(),
                            entity = %entity,
                            field = view.field_name,
                            reason = reason.as_str(),
                            "[editor] hook refused"
                        );
                        return false;
                    }
                }
            }
            if let Some(value) = replacement {
                write_replacement(command, value);
            }
        }
        true
    }

    /// Runs the after-callbacks for one command that applied successfully.
    pub(crate) fn after_field_command(
        &mut self,
        engine: &mut Engine,
        command: &EditorCommand,
        inverse: Option<&EditorCommand>,
    ) {
        if self.after.is_empty() {
            return;
        }
        let Some(entity) = field_command_entity(command) else {
            return;
        };
        let selection = selection_copy(engine.world());
        let primary = is_primary(&selection, entity);
        let Some(view) = after_view(
            engine.world(),
            command,
            inverse,
            self.cause,
            self.phase,
            primary,
        ) else {
            return;
        };
        let mut context = HookContext::new(
            engine.world_mut(),
            &selection,
            &mut self.followups,
            &mut self.notices,
            self.cause,
        );
        for hook in &self.after {
            // SAFETY: the request constructors store a callback and its
            // applier as one pair, and the registry copies both together.
            unsafe {
                hook.callback.call_field_edit(&mut context, &view);
            }
        }
    }

    /// Fold this drain's notices into the world's queue for the executable.
    pub(crate) fn publish_notices(&mut self, world: &mut World) {
        if self.notices.is_empty() {
            return;
        }
        HookNotices::ensure(world);
        if let Some(queue) = world.get_resource_mut::<HookNotices>() {
            for notice in self.notices.drain(..) {
                queue.push(notice);
            }
        }
    }
}

impl CommandObserver for HookSession {
    fn after_command(
        &mut self,
        engine: &mut Engine,
        _index: usize,
        command: &EditorCommand,
        inverse: Option<&EditorCommand>,
    ) {
        self.after_field_command(engine, command, inverse);
    }

    fn take_followups(&mut self) -> Vec<EditorCommand> {
        std::mem::take(&mut self.followups)
    }
}

// =============================================================================
// Field command views
// =============================================================================

/// The entity one field command targets, or `None` for every other command.
fn field_command_entity(command: &EditorCommand) -> Option<Entity> {
    match command {
        EditorCommand::SetField { entity, .. }
        | EditorCommand::SetArrayElement { entity, .. }
        | EditorCommand::SetFieldAsset { entity, .. }
        | EditorCommand::SetFieldEntity { entity, .. } => Some(*entity),
        _ => None,
    }
}

/// The shared selection, or a default one when the world has none.
fn selection_copy(world: &World) -> EditorSelection {
    world
        .get_resource::<EditorSelection>()
        .cloned()
        .unwrap_or_default()
}

/// Whether `entity` is the selection's primary.
fn is_primary(selection: &EditorSelection, entity: Entity) -> bool {
    selection.primary() == Some(entity)
}

/// The payload a before-callback sees, or `None` when the command's previous
/// value cannot be read.
///
/// A value edit (`SetField`, `SetArrayElement`) carries the proposed value, so
/// a callback can replace it. A reference edit (`SetFieldAsset`,
/// `SetFieldEntity`) is not offered to the before pass: its command holds a
/// reference, not a value, and resolving it would duplicate the applier's
/// asset loading. Reference edits reach the after-callbacks with the value the
/// world holds.
fn before_view<'a>(
    world: &World,
    command: &'a EditorCommand,
    cause: FieldEditCause,
    phase: FieldEditPhase,
    primary: bool,
) -> Option<FieldEditView<'a>> {
    match command {
        EditorCommand::SetField {
            entity,
            component,
            field,
            value,
        } => Some(FieldEditView {
            entity: *entity,
            component,
            field_name: field,
            index: None,
            previous: world.read_component_field(*entity, component, field).ok()?,
            current: value.clone(),
            cause,
            phase,
            is_primary: primary,
        }),
        EditorCommand::SetArrayElement {
            entity,
            component,
            field,
            index,
            value,
        } => {
            let previous = crate::inspector::element_field(
                &world.read_component_field(*entity, component, field).ok()?,
                *index,
            )?;
            Some(FieldEditView {
                entity: *entity,
                component,
                field_name: field,
                index: Some(*index),
                previous,
                current: value.clone(),
                cause,
                phase,
                is_primary: primary,
            })
        }
        _ => None,
    }
}

/// The payload an after-callback sees: the applied value and the value the
/// command's inverse restores.
fn after_view<'a>(
    world: &World,
    command: &'a EditorCommand,
    inverse: Option<&EditorCommand>,
    cause: FieldEditCause,
    phase: FieldEditPhase,
    primary: bool,
) -> Option<FieldEditView<'a>> {
    match command {
        EditorCommand::SetField {
            entity,
            component,
            field,
            value,
        } => Some(FieldEditView {
            entity: *entity,
            component,
            field_name: field,
            index: None,
            previous: previous_from_inverse(inverse)?,
            current: value.clone(),
            cause,
            phase,
            is_primary: primary,
        }),
        EditorCommand::SetArrayElement {
            entity,
            component,
            field,
            index,
            value,
        } => Some(FieldEditView {
            entity: *entity,
            component,
            field_name: field,
            index: Some(*index),
            previous: previous_from_inverse(inverse)?,
            current: value.clone(),
            cause,
            phase,
            is_primary: primary,
        }),
        // A reference edit's applied value is what the world now holds.
        EditorCommand::SetFieldAsset {
            entity,
            component,
            field,
            ..
        }
        | EditorCommand::SetFieldEntity {
            entity,
            component,
            field,
            ..
        } => Some(FieldEditView {
            entity: *entity,
            component,
            field_name: field,
            index: None,
            previous: previous_from_inverse(inverse)?,
            current: world.read_component_field(*entity, component, field).ok()?,
            cause,
            phase,
            is_primary: primary,
        }),
        _ => None,
    }
}

/// The value one command's inverse puts back.
fn previous_from_inverse(
    inverse: Option<&EditorCommand>,
) -> Option<pill_engine::component_field::FieldValue> {
    match inverse? {
        EditorCommand::SetField { value, .. } | EditorCommand::SetArrayElement { value, .. } => {
            Some(value.clone())
        }
        _ => None,
    }
}

/// Writes a before-callback's replacement into its command.
fn write_replacement(command: &mut EditorCommand, value: pill_engine::component_field::FieldValue) {
    match command {
        EditorCommand::SetField { value: current, .. }
        | EditorCommand::SetArrayElement { value: current, .. } => *current = value,
        _ => {}
    }
}
