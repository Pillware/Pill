//! Script components: the declared lifecycle method table and the per-frame
//! update pass.
//!
//! # Responsibilities
//!
//! - Registers a script type from its `#[script]` declaration: the component
//!   bit, the update adapter and the order it runs in.
//! - Caches the script types that implement `on_update`, sorted by
//!   `(order, type name)`, and rebuilds the cache only when a registration
//!   changes it.
//! - Runs `on_update` on every script of every listed type, once per frame,
//!   through a context that defers structural changes to the command queue.
//!
//! # Design
//!
//! A script component is an ordinary component first: it registers through the
//! same path as any other, and the script half is one table keyed by the type's
//! name. Each frame, dispatch asks the registry for the component's bit instead
//! of storing it, so a type that was removed and registered again on a new bit
//! is still found.
//!
//! Dispatch goes through plain function pointers rather than trait objects, so
//! an adapter must be a non-capturing function: it cannot capture state and has
//! no closure environment in which to keep the raw pointers, or anything else,
//! after the call. The adapter is created inside the generic registration call,
//! so its code is compiled into the artifact that declared the script rather
//! than into the engine dylib. Registering a type twice replaces its entry, so
//! it has one adapter per lifecycle method, not two.
//!
//! The adapter receives raw pointers to the world and the command queue because
//! the dispatch loop already borrows the archetype storage it passes in, so it
//! cannot lend out `&mut World` at the same time. Each pointer is valid only
//! for the call that receives it.

use super::*;
use crate::engine::RunMode;
use crate::scripting::{CreateInfo, DestroyInfo, StartInfo, StopInfo};

// =============================================================================
// Script Registration
// =============================================================================

/// Calls `on_update` on one script row.
///
/// Arguments: the component storage, the row index, the entity, and raw
/// pointers to the world and the command queue, from which the adapter builds
/// a [`ScriptContext`] for one call.
///
/// # Safety
///
/// The two raw-pointer arguments are valid only for the duration of the call
/// that passes them. An implementation must not store either one, and a caller
/// must pass pointers that stay valid for the whole call. Being a plain `fn`
/// and not a closure is part of the contract: it cannot capture, so it has no
/// closure state in which to keep them.
pub(super) type ScriptUpdateAdapter =
    fn(&mut ComponentColumns, usize, Entity, *mut World, *mut CommandQueue);

/// Calls `on_create` on one script row.
///
/// Arguments are [`ScriptUpdateAdapter`]'s plus the information about why the
/// component started existing.
///
/// # Safety
///
/// The two raw-pointer arguments are valid only for the duration of the call
/// that passes them, exactly as [`ScriptUpdateAdapter`] states.
pub(super) type ScriptCreateAdapter =
    fn(&mut ComponentColumns, usize, Entity, *mut World, *mut CommandQueue, &CreateInfo);

/// Calls `on_start` on one script row.
///
/// Arguments are [`ScriptUpdateAdapter`]'s plus the information about why the
/// script started running.
///
/// # Safety
///
/// The two raw-pointer arguments are valid only for the duration of the call
/// that passes them, exactly as [`ScriptUpdateAdapter`] states.
pub(super) type ScriptStartAdapter =
    fn(&mut ComponentColumns, usize, Entity, *mut World, *mut CommandQueue, &StartInfo);

/// Calls `on_stop` on one script row.
///
/// Arguments are [`ScriptUpdateAdapter`]'s plus the information about why the
/// script stopped running.
///
/// # Safety
///
/// The two raw-pointer arguments are valid only for the duration of the call
/// that passes them, exactly as [`ScriptUpdateAdapter`] states.
pub(super) type ScriptStopAdapter =
    fn(&mut ComponentColumns, usize, Entity, *mut World, *mut CommandQueue, &StopInfo);

/// Calls `on_destroy` on one script row.
///
/// Arguments are [`ScriptUpdateAdapter`]'s plus the information about why the
/// component stopped existing.
///
/// # Safety
///
/// The two raw-pointer arguments are valid only for the duration of the call
/// that passes them, exactly as [`ScriptUpdateAdapter`] states.
pub(super) type ScriptDestroyAdapter =
    fn(&mut ComponentColumns, usize, Entity, *mut World, *mut CommandQueue, &DestroyInfo);

/// One registered script type.
pub(super) struct ScriptRegistration {
    /// The type's component id, resolved to a bit at dispatch time so a
    /// re-registration on a new bit is still found.
    pub(super) component_id: ComponentId,
    /// The adapter that calls `on_update`, absent when the declaration does
    /// not implement it.
    pub(super) on_update: Option<ScriptUpdateAdapter>,
    /// Sort key of the update pass.
    pub(super) on_update_order: i32,
    /// The adapter that calls `on_create`, absent when the declaration does
    /// not implement it.
    pub(super) on_create: Option<ScriptCreateAdapter>,
    /// Sort key of the `on_create` pass.
    pub(super) on_create_order: i32,
    /// The adapter that calls `on_start`, absent when the declaration does not
    /// implement it.
    pub(super) on_start: Option<ScriptStartAdapter>,
    /// Sort key of the `on_start` pass.
    pub(super) on_start_order: i32,
    /// The adapter that calls `on_stop`, absent when the declaration does not
    /// implement it.
    pub(super) on_stop: Option<ScriptStopAdapter>,
    /// Sort key of the `on_stop` pass.
    pub(super) on_stop_order: i32,
    /// The adapter that calls `on_destroy`, absent when the declaration does
    /// not implement it.
    pub(super) on_destroy: Option<ScriptDestroyAdapter>,
    /// Sort key of the `on_destroy` pass.
    pub(super) on_destroy_order: i32,
    /// Whether the script runs while the editor edits as well as while it
    /// plays.
    pub(super) run_in_edit_mode: bool,
}

// =============================================================================
// World - Script Components
// =============================================================================

impl World {
    /// Registers a component type the engine runs as a script.
    ///
    /// Registers `T` as an ordinary component first, then records the lifecycle
    /// methods its `#[script]` declaration states. Registering the same type
    /// again replaces its record instead of adding a second one, which is what
    /// a reload does: the arriving image's lifecycle method bodies replace the
    /// retired image's.
    ///
    /// The orders and the edit-mode flag are owned values, so nothing here
    /// points into the artifact's static data after the call. The adapters are
    /// plain function pointers into the image that declared the script, and a
    /// registration from a new image points them at the new image.
    pub fn register_script_component<T>(&mut self)
    where
        T: ScriptComponent,
    {
        let _zone = crate::profile_scope!(
            "register script component",
            [(
                "Script component type being registered: {}",
                std::any::type_name::<T>()
            )]
        );
        // Step 1: Register it as an ordinary component, so the registry holds
        // a bit to address its column by.
        self.register_component::<T>();

        // Step 2: Build one adapter per lifecycle method the declaration
        // implements. Every closure captures nothing, so it coerces to the
        // function pointer; the raw pointers are passed again on every call.
        let declaration = T::DECLARATION;
        let update_adapter = (|storage: &mut ComponentColumns,
                               index: usize,
                               entity: Entity,
                               world_ptr: *mut World,
                               commands_ptr: *mut CommandQueue| {
            let component = storage.column_of_mut::<T>().get_mut::<T>(index);
            // SAFETY: `world_ptr` and `commands_ptr` are derived from
            // `&mut World` / `&mut CommandQueue` that are valid for the whole
            // call this adapter serves, which is the only caller of every
            // stored adapter. An adapter is a plain `fn` with no closure
            // environment, so it has nowhere to keep the pointers for a later
            // call.
            unsafe {
                let mut script_context =
                    ScriptContext::new(&mut *world_ptr, &mut *commands_ptr, entity);
                component.on_update(&mut script_context);
            }
        }) as ScriptUpdateAdapter;
        let on_update = declaration.implements_on_update.then_some(update_adapter);

        let create_adapter = (|storage: &mut ComponentColumns,
                               index: usize,
                               entity: Entity,
                               world_ptr: *mut World,
                               commands_ptr: *mut CommandQueue,
                               create_info: &CreateInfo| {
            let component = storage.column_of_mut::<T>().get_mut::<T>(index);
            // SAFETY: as in the update adapter above - both pointers are valid
            // for the whole call, and nothing stores them.
            unsafe {
                let mut script_context =
                    ScriptContext::new(&mut *world_ptr, &mut *commands_ptr, entity);
                component.on_create(&mut script_context, create_info);
            }
        }) as ScriptCreateAdapter;
        let on_create = declaration.implements_on_create.then_some(create_adapter);

        let start_adapter = (|storage: &mut ComponentColumns,
                              index: usize,
                              entity: Entity,
                              world_ptr: *mut World,
                              commands_ptr: *mut CommandQueue,
                              start_info: &StartInfo| {
            let component = storage.column_of_mut::<T>().get_mut::<T>(index);
            // SAFETY: as in the update adapter above - both pointers are valid
            // for the whole call, and nothing stores them.
            unsafe {
                let mut script_context =
                    ScriptContext::new(&mut *world_ptr, &mut *commands_ptr, entity);
                component.on_start(&mut script_context, start_info);
            }
        }) as ScriptStartAdapter;
        let on_start = declaration.implements_on_start.then_some(start_adapter);

        let stop_adapter = (|storage: &mut ComponentColumns,
                             index: usize,
                             entity: Entity,
                             world_ptr: *mut World,
                             commands_ptr: *mut CommandQueue,
                             stop_info: &StopInfo| {
            let component = storage.column_of_mut::<T>().get_mut::<T>(index);
            // SAFETY: as in the update adapter above - both pointers are valid
            // for the whole call, and nothing stores them.
            unsafe {
                let mut script_context =
                    ScriptContext::new(&mut *world_ptr, &mut *commands_ptr, entity);
                component.on_stop(&mut script_context, stop_info);
            }
        }) as ScriptStopAdapter;
        let on_stop = declaration.implements_on_stop.then_some(stop_adapter);

        let destroy_adapter = (|storage: &mut ComponentColumns,
                                index: usize,
                                entity: Entity,
                                world_ptr: *mut World,
                                commands_ptr: *mut CommandQueue,
                                destroy_info: &DestroyInfo| {
            let component = storage.column_of_mut::<T>().get_mut::<T>(index);
            // SAFETY: as in the update adapter above - both pointers are valid
            // for the whole call, and nothing stores them.
            unsafe {
                let mut script_context =
                    ScriptContext::new(&mut *world_ptr, &mut *commands_ptr, entity);
                component.on_destroy(&mut script_context, destroy_info);
            }
        }) as ScriptDestroyAdapter;
        let on_destroy = declaration.implements_on_destroy.then_some(destroy_adapter);

        // Step 3: Record it under the type's name, and drop the cached update
        // order so the next frame rebuilds it.
        self.script_registrations.insert(
            std::any::type_name::<T>().to_owned(),
            ScriptRegistration {
                component_id: ComponentId::of::<T>(),
                on_update,
                on_update_order: declaration.on_update_order,
                on_create,
                on_create_order: declaration.on_create_order,
                on_start,
                on_start_order: declaration.on_start_order,
                on_stop,
                on_stop_order: declaration.on_stop_order,
                on_destroy,
                on_destroy_order: declaration.on_destroy_order,
                run_in_edit_mode: declaration.run_in_edit_mode,
            },
        );
        self.script_update_order = None;
    }

    /// Runs `on_update` on every script component of every registered type
    /// that implements it, once per frame.
    ///
    /// Scripts receive a [`ScriptContext`] with component and resource reads, a
    /// mutable component borrow, and the command queue. Every structural change
    /// (add/remove component, destroy entity) is deferred there, so it is
    /// applied after all scripts have finished and no script can migrate a row
    /// out from under itself.
    ///
    /// `run_mode` decides which types take part: while the editor edits, only a
    /// script whose declaration opted in to edit mode runs.
    pub(crate) fn update_scripts(&mut self, commands: &mut CommandQueue, run_mode: RunMode) {
        let _zone = crate::profile_scope!(
            "update scripts",
            [(
                "Script component types in world: {}",
                self.script_registrations.len()
            )]
        );
        // Step 1: Reserve the per-frame work list, take the cached type order,
        // and capture raw pointers to self and the command queue before any
        // field borrows on self.
        let entity_count = self.storage.entity_locations.len();
        let mut entities_to_update: Vec<(Entity, ArchetypeId, usize)> =
            Vec::with_capacity(entity_count);
        let ordered_types = self.take_script_update_order();

        // Taken once, before any field borrow of `self`.
        let world_ptr = self as *mut World;
        let commands_ptr = commands as *mut CommandQueue;

        // Step 2: For each script type in the cached order, gather every
        // entity that carries it. The bit is read from the registry here
        // rather than stored at registration time, so a type that was removed
        // and registered again on another bit is still found.
        for type_name in &ordered_types {
            let Some(registration) = self.script_registrations.get(type_name) else {
                continue;
            };
            if run_mode == RunMode::Editing && !registration.run_in_edit_mode {
                continue;
            }
            let Some(updater) = registration.on_update else {
                continue;
            };
            let component_id = registration.component_id;
            let Some(script_bit) = self.component_registry.get_bit(&component_id) else {
                continue;
            };

            // Every entity of an archetype whose mask carries the script bit.
            for (archetype_id, archetype) in &self.storage.archetypes {
                if archetype.has_component_bit(script_bit) {
                    for (index, &entity) in archetype.entities.iter().enumerate() {
                        entities_to_update.push((entity, *archetype_id, index));
                    }
                }
            }

            // Step 3: Sort the gathered entities for deterministic order
            // across runs, then dispatch each one to its adapter with the
            // captured raw pointers.
            entities_to_update.sort_by_key(|(_, archetype_id, index)| (*archetype_id, *index));

            for (entity, archetype_id, index) in entities_to_update.drain(..) {
                if let Some(archetype) = self.storage.archetypes.get_mut(&archetype_id) {
                    updater(
                        &mut archetype.component_storages,
                        index,
                        entity,
                        world_ptr,
                        commands_ptr,
                    );
                }
            }
        }

        // Step 4: Put the order back, so the next frame reuses it instead of
        // sorting again.
        self.script_update_order = Some(ordered_types);
    }

    /// Returns the order the update pass walks the registered types in,
    /// rebuilding it when a registration dropped it.
    pub(super) fn take_script_update_order(&mut self) -> Vec<String> {
        match self.script_update_order.take() {
            Some(order) => order,
            None => self.build_script_update_order(),
        }
    }

    /// Sorts the registered types that implement `on_update` by their own
    /// order and then by type name, so equal orders run in a fixed sequence.
    pub(super) fn build_script_update_order(&self) -> Vec<String> {
        let mut entries: Vec<(i32, &String)> = self
            .script_registrations
            .iter()
            .filter(|(_, registration)| registration.on_update.is_some())
            .map(|(type_name, registration)| (registration.on_update_order, type_name))
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
        entries
            .into_iter()
            .map(|(_, type_name)| type_name.clone())
            .collect()
    }
}
