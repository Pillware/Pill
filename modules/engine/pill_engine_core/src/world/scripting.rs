//! Script components: registration and per-frame dispatch.
//!
//! # Responsibilities
//!
//! - Registers a component type that also carries an `update` method, and
//!   records the per-type updater that calls it.
//! - Runs every script component once per frame, through a context that defers
//!   structural changes to the command queue.
//!
//! # Design
//!
//! A script component is an ordinary component first: it registers through the
//! same path as any other, and the script half is one table of updaters keyed by
//! component id. Each frame, dispatch asks the registry for the component's bit
//! instead of storing it, so a type that was removed and registered again on a
//! new bit is still found. Dispatch goes through a plain function pointer
//! rather than a trait object, so an updater must be a non-capturing function:
//! it cannot capture state and has no closure environment in which to keep the
//! raw pointers, or anything else, after the call. Registering a type twice
//! overwrites its entry, so it has one updater, not two.
//!
//! The updater receives raw pointers to the world and the command queue because
//! the dispatch loop already borrows the archetype storage it passes in, so it
//! cannot lend out `&mut World` at the same time. Each pointer is valid only
//! for the call that receives it.

use super::*;

// =============================================================================
// Script Updater Type
// =============================================================================

/// Function that updates a script component.
///
/// Arguments: the component storage, the row index, the entity, and raw
/// pointers to the world and the command queue, from which the updater builds
/// a [`ScriptContext`] for one dispatch.
///
/// # Safety
///
/// The two raw-pointer arguments are valid only for the duration of the call
/// that passes them. An implementation must not store either one, and a caller
/// must pass pointers that stay valid for the whole call. Being a plain `fn`
/// and not a closure is part of the contract: it cannot capture, so it has no
/// closure state in which to keep them.
pub(super) type ScriptUpdater =
    fn(&mut ComponentColumns, usize, Entity, *mut World, *mut CommandQueue);

// =============================================================================
// World - Script Components
// =============================================================================

impl World {
    /// Registers a component type whose `update` runs once per frame per
    /// entity.
    ///
    /// Registers `T` as an ordinary component first, then records the updater
    /// `update_scripts` dispatches through. Registering the same type again
    /// replaces its updater instead of adding a second one.
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

        // Step 2: Store the updater. The closure captures nothing, so it
        // coerces to the function pointer. The raw pointers are passed again
        // on every dispatch.
        self.script_updaters.insert(
            ComponentId::of::<T>(),
            (|storage: &mut ComponentColumns,
              index: usize,
              entity: Entity,
              world_ptr: *mut World,
              commands_ptr: *mut CommandQueue| {
                let component = storage.column_of_mut::<T>().get_mut::<T>(index);
                // SAFETY: `world_ptr` and `commands_ptr` are derived from
                // `&mut World` / `&mut CommandQueue` that are valid for the
                // entire duration of `update_scripts`, which is the sole
                // caller of every stored updater. An updater is a plain `fn`
                // with no closure environment, so it has nowhere to keep the
                // pointers for a later call.
                unsafe {
                    let mut script_context =
                        ScriptContext::new(&mut *world_ptr, &mut *commands_ptr, entity);
                    component.update(&mut script_context);
                }
            }) as ScriptUpdater,
        );
    }

    /// Runs `update` on every script component of every entity that carries
    /// one, once per frame.
    ///
    /// Scripts receive a [`ScriptContext`] with component and resource reads, a
    /// mutable component borrow, and the command queue. Every structural change
    /// (add/remove component, destroy entity) is deferred there, so it is
    /// applied after all scripts have finished and no script can migrate a row
    /// out from under itself.
    pub(crate) fn update_scripts(&mut self, commands: &mut CommandQueue) {
        let _zone = crate::profile_scope!(
            "update scripts",
            [(
                "Script component types in world: {}",
                self.script_updaters.len()
            )]
        );
        // Step 1: Reserve the per-frame work list and capture raw pointers to
        // self and the command queue before any field borrows on self.
        let entity_count = self.storage.entity_locations.len();
        let mut entities_to_update: Vec<(Entity, ArchetypeId, usize)> =
            Vec::with_capacity(entity_count);

        // Taken once, before any field borrow of `self`.
        let world_ptr = self as *mut World;
        let commands_ptr = commands as *mut CommandQueue;

        // Step 2: For each registered script type, gather every entity that
        // carries it. The bit is read from the registry here rather than
        // stored at registration time, so a type that was removed and
        // registered again on another bit is still found.
        for (&component_id, &updater) in &self.script_updaters {
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
            // across runs, then dispatch each one to its updater with the
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
    }
}
