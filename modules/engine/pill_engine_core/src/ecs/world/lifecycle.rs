//! The script lifecycle passes: telling a component it started existing, and
//! telling a script it started running.
//!
//! # Responsibilities
//!
//! - Holds the queue of script components that started existing and have not
//!   been told yet, with the cause each hook receives.
//! - Runs the `on_create` pass and then the `on_start` pass, each ordered by its
//!   own `#[order(n)]` and then by type name and entity.
//! - Records an attachment at the two places a component can start existing:
//!   the entity insert and the component-add migration.
//!
//! # Design
//!
//! The queue holds handles, never pointers or borrows: an entity, the
//! registered type's name, and the cause. An entry is validated when it is
//! called - the entity must still exist and must still carry the type - so a
//! script removed before its first start is skipped instead of resurrected. A
//! type name resolves through the registration table at call time, which is
//! what lets the name outlive a reload: the retired image's hook bodies are
//! gone, and the arriving image's registration replaces them.
//!
//! Both passes run at one point in the frame, before `on_update`, and the
//! frame's flush follows them. A command a hook queues therefore lands in the
//! queue that flush drains, unless the hook itself runs inside a removal, which
//! the frame's own flush drives - that case leaves its commands for the next
//! flush point.
//!
//! A play session fills the queue itself: every script the edit world holds is
//! told it started existing when Play is pressed, which is what makes the first
//! frames of a session look like a scene load.

use super::scripting::{ScriptCreateHook, ScriptDestroyHook, ScriptStartHook, ScriptStopHook};
use super::*;
use crate::engine::RunMode;
use crate::scripting::{
    CreateCause, CreateInfo, DestroyCause, DestroyInfo, StartCause, StartInfo, StopCause, StopInfo,
};

// =============================================================================
// Pending Script Event
// =============================================================================

/// One script component waiting for its `on_create` and `on_start` passes.
pub(super) struct PendingScriptEvent {
    /// The entity carrying the component.
    pub(super) entity: Entity,
    /// The registered type's key, as `register_script_component` recorded it.
    pub(super) script_type: String,
    /// Why the component started existing.
    pub(super) create_cause: CreateCause,
    /// The entity it was copied from, when a paste or a duplicate named one.
    pub(super) duplicated_from: Option<Entity>,
    /// Why the script starts running, or `None` when this entry only reports a
    /// component starting to exist. A restored edit world has not started
    /// running; a play session tells it so through its own scan.
    pub(super) start_cause: Option<StartCause>,
}

// =============================================================================
// Recording
// =============================================================================

impl World {
    /// Records that `component_id` just started existing on `entity`.
    ///
    /// Does nothing for a component that is not a registered script, which is
    /// every component type that does not carry a `#[script]` declaration. The
    /// event is dropped later if the entity loses the component before the
    /// passes run.
    pub(super) fn record_script_attachment(&mut self, entity: Entity, component_id: ComponentId) {
        let Some(script_type) = self
            .script_registrations
            .iter()
            .find(|(_, registration)| registration.component_id == component_id)
            .map(|(type_name, _)| type_name.clone())
        else {
            return;
        };
        self.pending_script_events.push(PendingScriptEvent {
            entity,
            script_type,
            create_cause: self.change_origin.create_cause(),
            duplicated_from: self.change_origin.duplicated_from(),
            start_cause: Some(StartCause::SpawnedDuringPlay),
        });
    }

    /// Queues every script the world holds, as Play does.
    pub(crate) fn queue_scripts_for_play_start(&mut self) {
        self.queue_every_script(CreateCause::PlayStarted, Some(StartCause::PlayEntered));
    }

    /// Queues every script the restored edit world holds, as Stop does.
    ///
    /// Nothing starts running here: the edit world is back in the editor and
    /// no script has begun a session.
    pub(crate) fn queue_scripts_for_play_stop_restore(&mut self) {
        self.queue_every_script(CreateCause::PlayStopRestored, None);
    }

    /// Queues every script the world holds, with the causes a session boundary
    /// reports.
    ///
    /// The queue is emptied first: an event still waiting from the other side
    /// of the boundary is covered by the entries this call adds, and
    /// dispatching both would tell a script it was created twice.
    fn queue_every_script(&mut self, create_cause: CreateCause, start_cause: Option<StartCause>) {
        self.pending_script_events.clear();
        for (type_name, registration) in &self.script_registrations {
            let Some(script_bit) = self.component_registry.get_bit(&registration.component_id)
            else {
                continue;
            };
            for archetype in self.storage.archetypes.values() {
                if !archetype.has_component_bit(script_bit) {
                    continue;
                }
                for &entity in &archetype.entities {
                    self.pending_script_events.push(PendingScriptEvent {
                        entity,
                        script_type: type_name.clone(),
                        create_cause,
                        duplicated_from: None,
                        start_cause,
                    });
                }
            }
        }
    }
}

// =============================================================================
// The Passes
// =============================================================================

impl World {
    /// Runs the `on_create` pass, then the `on_start` pass, for every script
    /// that started existing since the last frame.
    ///
    /// Both passes run only where scripts run. While the editor edits, a script
    /// takes part only when its declaration opted in to edit mode, and no
    /// script starts running: `on_start` belongs to a play session.
    pub(crate) fn run_script_lifecycle_passes(
        &mut self,
        commands: &mut CommandQueue,
        run_mode: RunMode,
    ) {
        if self.pending_script_events.is_empty() {
            return;
        }
        let events = std::mem::take(&mut self.pending_script_events);
        self.run_create_pass(&events, commands, run_mode);
        if run_mode == RunMode::Playing {
            self.run_start_pass(&events, commands);
        }
    }

    /// Calls `on_create` on each pending entry, lowest order first.
    fn run_create_pass(
        &mut self,
        events: &[PendingScriptEvent],
        commands: &mut CommandQueue,
        run_mode: RunMode,
    ) {
        let mut ordered: Vec<(i32, &PendingScriptEvent)> = Vec::new();
        for event in events {
            let Some(registration) = self.script_registrations.get(&event.script_type) else {
                continue;
            };
            if registration.on_create.is_none() {
                continue;
            }
            if run_mode == RunMode::Editing && !registration.run_in_edit_mode {
                continue;
            }
            ordered.push((registration.on_create_order, event));
        }
        sort_pending_events(&mut ordered);
        for (_, event) in ordered {
            let create_info = CreateInfo {
                cause: event.create_cause,
                mode: run_mode,
                duplicated_from: event.duplicated_from,
            };
            self.call_create_hook(commands, event, &create_info);
        }
    }

    /// Calls `on_start` on each pending entry, lowest order first.
    fn run_start_pass(&mut self, events: &[PendingScriptEvent], commands: &mut CommandQueue) {
        let mut ordered: Vec<(i32, &PendingScriptEvent)> = Vec::new();
        for event in events {
            let Some(registration) = self.script_registrations.get(&event.script_type) else {
                continue;
            };
            if registration.on_start.is_none() || event.start_cause.is_none() {
                continue;
            }
            ordered.push((registration.on_start_order, event));
        }
        sort_pending_events(&mut ordered);
        for (_, event) in ordered {
            let Some(start_cause) = event.start_cause else {
                continue;
            };
            let start_info = StartInfo { cause: start_cause };
            self.call_start_hook(commands, event, &start_info);
        }
    }

    /// Calls one entry's `on_create`, if the entity still carries the type.
    fn call_create_hook(
        &mut self,
        commands: &mut CommandQueue,
        event: &PendingScriptEvent,
        create_info: &CreateInfo,
    ) {
        // Taken before the row borrow below: the adapter receives the world and
        // the queue as raw pointers because the row borrow already covers the
        // storage. Both stay valid for this whole call, and the adapter is a
        // plain `fn` with no closure environment, so it cannot keep them.
        let world_pointer = self as *mut World;
        let commands_pointer = commands as *mut CommandQueue;
        let Some((adapter, component_id)) = self.create_adapter_of(&event.script_type) else {
            return;
        };
        let Some((storages, index)) = self.script_row(event.entity, component_id) else {
            return;
        };
        adapter(
            storages,
            index,
            event.entity,
            world_pointer,
            commands_pointer,
            create_info,
        );
    }

    /// Calls one entry's `on_start`, if the entity still carries the type.
    fn call_start_hook(
        &mut self,
        commands: &mut CommandQueue,
        event: &PendingScriptEvent,
        start_info: &StartInfo,
    ) {
        // As in `call_create_hook`: the pointers are taken before the row
        // borrow, and both stay valid for this whole call.
        let world_pointer = self as *mut World;
        let commands_pointer = commands as *mut CommandQueue;
        let Some((adapter, component_id)) = self.start_adapter_of(&event.script_type) else {
            return;
        };
        let Some((storages, index)) = self.script_row(event.entity, component_id) else {
            return;
        };
        adapter(
            storages,
            index,
            event.entity,
            world_pointer,
            commands_pointer,
            start_info,
        );
    }

    /// The `on_create` adapter and component id of a registered script type.
    fn create_adapter_of(&self, script_type: &str) -> Option<(ScriptCreateHook, ComponentId)> {
        let registration = self.script_registrations.get(script_type)?;
        Some((registration.on_create?, registration.component_id))
    }

    /// The `on_start` adapter and component id of a registered script type.
    fn start_adapter_of(&self, script_type: &str) -> Option<(ScriptStartHook, ComponentId)> {
        let registration = self.script_registrations.get(script_type)?;
        Some((registration.on_start?, registration.component_id))
    }

    /// The row of `entity` inside the archetype that carries `component_id`.
    ///
    /// Returns `None` for an entity that no longer exists, and for one that no
    /// longer carries the component: a script removed before its first start
    /// never hears about the start it missed.
    fn script_row(
        &mut self,
        entity: Entity,
        component_id: ComponentId,
    ) -> Option<(&mut ComponentColumns, usize)> {
        let script_bit = self.component_registry.get_bit(&component_id)?;
        let location = *self.storage.entity_locations.get(&entity)?;
        let archetype = self.storage.archetypes.get_mut(&location.archetype_id)?;
        if !archetype.has_component_bit(script_bit) {
            return None;
        }
        Some((
            &mut archetype.component_storages,
            location.index_in_archetype,
        ))
    }
}

/// Sorts pending events by hook order, then type name, then entity.
///
/// Type name before entity keeps one type's entities together, which is the
/// order the play-start scan reads them in, and the entity key makes the result
/// independent of the order the events were queued in.
fn sort_pending_events(ordered: &mut [(i32, &PendingScriptEvent)]) {
    ordered.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.script_type.cmp(&right.1.script_type))
            .then_with(|| left.1.entity.id.cmp(&right.1.entity.id))
            .then_with(|| left.1.entity.generation.cmp(&right.1.entity.generation))
    });
}

/// Sorts `(order, type name, entity)` triples the same way the pending events
/// are sorted, so every pass in a frame runs in one predictable sequence.
fn sort_hook_order(ordered: &mut [(i32, &String, Entity)]) {
    ordered.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(right.1))
            .then_with(|| left.2.id.cmp(&right.2.id))
            .then_with(|| left.2.generation.cmp(&right.2.generation))
    });
}

// =============================================================================
// Removal Passes
// =============================================================================

impl World {
    /// Runs `on_stop` and then `on_destroy` for the scripts an entity is about
    /// to lose, before the removal happens.
    ///
    /// `only` names the one component a removal takes, or `None` when the whole
    /// entity is going. `on_stop` fires only inside a play session, because a
    /// script that never started running has nothing to stop; `on_destroy`
    /// always fires.
    ///
    /// The component is still there for both calls, so a hook can read its own
    /// fields and the entity's other components up to the last moment.
    pub(crate) fn run_script_removal_passes(
        &mut self,
        entity: Entity,
        only: Option<ComponentId>,
        commands: &mut CommandQueue,
    ) {
        let script_types = self.script_types_on(entity, only);
        if script_types.is_empty() {
            return;
        }
        if self.held_entity_storage.is_some() {
            let mut ordered: Vec<(i32, &String)> = script_types
                .iter()
                .filter_map(|(type_name, _)| {
                    let registration = self.script_registrations.get(type_name)?;
                    registration
                        .on_stop
                        .map(|_| (registration.on_stop_order, type_name))
                })
                .collect();
            ordered.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
            let stop_info = StopInfo {
                cause: StopCause::Removed,
            };
            for (_, type_name) in ordered {
                self.call_stop_hook(commands, entity, type_name, &stop_info);
            }
        }

        let mut ordered: Vec<(i32, &String)> = script_types
            .iter()
            .filter_map(|(type_name, _)| {
                let registration = self.script_registrations.get(type_name)?;
                registration
                    .on_destroy
                    .map(|_| (registration.on_destroy_order, type_name))
            })
            .collect();
        ordered.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
        let destroy_info = DestroyInfo {
            cause: self.change_origin.destroy_cause(),
        };
        for (_, type_name) in ordered {
            self.call_destroy_hook(commands, entity, type_name, &destroy_info);
        }
    }

    /// Runs `on_stop(PlayExited)` and then `on_destroy(PlayStopRestored)` for
    /// every script in the play world, before the edit world comes back.
    ///
    /// The running rows are about to be discarded by the restore, which is the
    /// one destruction no command describes, so the session reports it here.
    pub(crate) fn run_play_stop_script_passes(&mut self, commands: &mut CommandQueue) {
        let mut entries: Vec<(String, Entity)> = Vec::new();
        for (type_name, registration) in &self.script_registrations {
            if registration.on_stop.is_none() && registration.on_destroy.is_none() {
                continue;
            }
            let Some(script_bit) = self.component_registry.get_bit(&registration.component_id)
            else {
                continue;
            };
            for archetype in self.storage.archetypes.values() {
                if !archetype.has_component_bit(script_bit) {
                    continue;
                }
                for &entity in &archetype.entities {
                    entries.push((type_name.clone(), entity));
                }
            }
        }
        if entries.is_empty() {
            return;
        }

        let mut stop_pass: Vec<(i32, &String, Entity)> = entries
            .iter()
            .filter_map(|(type_name, entity)| {
                let registration = self.script_registrations.get(type_name)?;
                registration
                    .on_stop
                    .map(|_| (registration.on_stop_order, type_name, *entity))
            })
            .collect();
        sort_hook_order(&mut stop_pass);
        let stop_info = StopInfo {
            cause: StopCause::PlayExited,
        };
        for (_, type_name, entity) in stop_pass {
            self.call_stop_hook(commands, entity, type_name, &stop_info);
        }

        let mut destroy_pass: Vec<(i32, &String, Entity)> = entries
            .iter()
            .filter_map(|(type_name, entity)| {
                let registration = self.script_registrations.get(type_name)?;
                registration
                    .on_destroy
                    .map(|_| (registration.on_destroy_order, type_name, *entity))
            })
            .collect();
        sort_hook_order(&mut destroy_pass);
        let destroy_info = DestroyInfo {
            cause: DestroyCause::PlayStopRestored,
        };
        for (_, type_name, entity) in destroy_pass {
            self.call_destroy_hook(commands, entity, type_name, &destroy_info);
        }
    }

    /// The registered scripts `entity` carries, optionally only the one with
    /// `only`'s id.
    fn script_types_on(
        &self,
        entity: Entity,
        only: Option<ComponentId>,
    ) -> Vec<(String, ComponentId)> {
        let Some(location) = self.storage.entity_locations.get(&entity).copied() else {
            return Vec::new();
        };
        let Some(archetype) = self.storage.archetypes.get(&location.archetype_id) else {
            return Vec::new();
        };
        let mut script_types: Vec<(String, ComponentId)> = Vec::new();
        for (type_name, registration) in &self.script_registrations {
            if only.is_some_and(|component_id| component_id != registration.component_id) {
                continue;
            }
            if registration.on_stop.is_none() && registration.on_destroy.is_none() {
                continue;
            }
            if !archetype
                .component_types
                .contains(&registration.component_id)
            {
                continue;
            }
            script_types.push((type_name.clone(), registration.component_id));
        }
        script_types
    }

    /// Calls one entity's `on_stop` for `script_type`.
    fn call_stop_hook(
        &mut self,
        commands: &mut CommandQueue,
        entity: Entity,
        script_type: &str,
        stop_info: &StopInfo,
    ) {
        // Taken before the row borrow, for the same reason as in
        // `call_create_hook`.
        let world_pointer = self as *mut World;
        let commands_pointer = commands as *mut CommandQueue;
        let Some((adapter, component_id)) = self.stop_adapter_of(script_type) else {
            return;
        };
        let Some((storages, index)) = self.script_row(entity, component_id) else {
            return;
        };
        adapter(
            storages,
            index,
            entity,
            world_pointer,
            commands_pointer,
            stop_info,
        );
    }

    /// Calls one entity's `on_destroy` for `script_type`.
    fn call_destroy_hook(
        &mut self,
        commands: &mut CommandQueue,
        entity: Entity,
        script_type: &str,
        destroy_info: &DestroyInfo,
    ) {
        // Taken before the row borrow, as in `call_stop_hook`.
        let world_pointer = self as *mut World;
        let commands_pointer = commands as *mut CommandQueue;
        let Some((adapter, component_id)) = self.destroy_adapter_of(script_type) else {
            return;
        };
        let Some((storages, index)) = self.script_row(entity, component_id) else {
            return;
        };
        adapter(
            storages,
            index,
            entity,
            world_pointer,
            commands_pointer,
            destroy_info,
        );
    }

    /// The `on_stop` adapter and component id of a registered script type.
    fn stop_adapter_of(&self, script_type: &str) -> Option<(ScriptStopHook, ComponentId)> {
        let registration = self.script_registrations.get(script_type)?;
        Some((registration.on_stop?, registration.component_id))
    }

    /// The `on_destroy` adapter and component id of a registered script type.
    fn destroy_adapter_of(&self, script_type: &str) -> Option<(ScriptDestroyHook, ComponentId)> {
        let registration = self.script_registrations.get(script_type)?;
        Some((registration.on_destroy?, registration.component_id))
    }
}
