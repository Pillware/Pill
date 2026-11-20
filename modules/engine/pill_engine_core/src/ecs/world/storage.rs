//! The entity-side half of a world: everything an entity owns, in one value.
//!
//! # Responsibilities
//!
//! - Declares [`EntityStorage`]: the archetypes and their columns, the entity
//!   allocator and its free list, the locations map, the stable-id maps and the
//!   archetype generation counter.
//! - Provides the moves the play-mode machinery needs: replacing the live
//!   storage with another value, and running a reload pass over both the live
//!   and the held one.
//!
//! # Design
//!
//! A [`World`](crate::world::World) keeps its entity data in one
//! `EntityStorage` and may hold a second one aside. Everything that is not
//! entity data - the component registry, the per-type function tables, the
//! resources and the per-frame bookkeeping - stays on the world, because both
//! storages are read through the same registry and the same tables.
//!
//! Holding a storage is what lets the editor run a game and come back to the
//! world it had: the live storage is the running one, and the held storage is
//! the edit state. A reload has to reach both, because a column's function
//! table points into the image that registered the type; a held column left
//! un-re-pointed would call code that is no longer mapped.
//!
//! Change-detection ticks live in the columns, so they travel with the storage
//! and a restore leaves them where they were.
//!
//! # Invariants
//!
//! - `archetype_generation` goes up by one whenever an archetype is added or
//!   removed, and whenever a storage is swapped in, so every query's cached
//!   archetype list is recomputed.
//! - A storage's `entity_locations`, `entity_ids` and `entity_handles` describe
//!   exactly the entities in its archetypes.

// Standard library
use std::collections::HashMap;

// Current crate
use crate::archetype::{Archetype, ArchetypeId, ComponentColumn, ComponentColumns, StorageFactory};
use crate::component::{Component, ComponentId};
use crate::entity::{Entity, EntityId};
use crate::world::{EntityLocation, World};

// =============================================================================
// EntityStorage
// =============================================================================

/// Every piece of world state an entity owns, as one value.
///
/// The world keeps one of these live and may keep a second one held aside; see
/// the module header for why both exist.
#[derive(Default)]
pub(crate) struct EntityStorage {
    // --- Entity allocation ---
    /// Next fresh entity ID handed out when the free list is empty.
    pub(crate) next_fresh_entity_id: u64,
    /// Free list of recycled entity IDs, each with the generation it will be
    /// handed out with. Stored as (id, generation) pairs.
    pub(crate) free_entity_ids: Vec<(u64, u32)>,
    /// The stable id of every live entity, keyed by its handle.
    pub(crate) entity_ids: HashMap<Entity, EntityId>,
    /// Reverse of `entity_ids`, so a saved reference finds its entity in O(1).
    pub(crate) entity_handles: HashMap<EntityId, Entity>,
    /// Seeds the counter-derived ids used when the platform has no random
    /// source; see `World::mint_entity_id`.
    pub(crate) entity_id_fallback_counter: u64,

    // --- Archetypes, columns and entity locations ---
    /// All archetypes in this storage, keyed by their component mask.
    pub(crate) archetypes: HashMap<ArchetypeId, Archetype>,
    /// The archetype and row each entity lives in.
    pub(crate) entity_locations: HashMap<Entity, EntityLocation>,

    // --- Bookkeeping ---
    /// Counter that goes up by one whenever an archetype is added or removed,
    /// or the storage is swapped for another value.
    ///
    /// Queries cache the list of archetypes that match them and compare this
    /// counter on each run: if it has not changed, the cached list is still
    /// correct.
    pub(crate) archetype_generation: u64,
}

impl EntityStorage {
    /// Creates an empty storage.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Records that the archetype set changed, so query caches are recomputed.
    pub(crate) fn bump_archetype_generation(&mut self) {
        self.archetype_generation = self.archetype_generation.wrapping_add(1);
    }

    /// Returns the number of live entities in this storage.
    pub(crate) fn entity_count(&self) -> usize {
        self.entity_locations.len()
    }

    /// Returns how many component rows this storage holds.
    ///
    /// One entity with three components contributes three rows, so the figure
    /// describes the work a copy does rather than the size of the world.
    pub(crate) fn component_row_count(&self) -> usize {
        self.archetypes
            .values()
            .map(|archetype| archetype.entity_count() * archetype.component_storages.len())
            .sum()
    }
}

// =============================================================================
// Copying a storage
// =============================================================================

/// One type's row-cloning function: copies every row of the source column into
/// the destination column, clones included.
///
/// It is monomorphised in the DLL that registered the type, so it is only ever
/// called while that image is mapped. The engine re-registers it on every
/// reload with the other per-type functions.
pub(crate) type CloneColumnFn = fn(&ComponentColumn, &mut ComponentColumn) -> Result<(), ()>;

/// The component types a storage copy could not include.
///
/// Every name is listed, not just the first: a person who has to derive
/// `Clone` on a handful of components wants the whole list in one go.
///
/// The play session turns these names into a refusal, so the type is compiled
/// in ahead of its caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UncopyableComponents {
    /// The registered names of the types the copy could not include.
    pub(crate) type_names: Vec<String>,
}

/// The clone function a `Clone` component's registration records.
fn clone_column<T: Component + Clone>(
    source: &ComponentColumn,
    destination: &mut ComponentColumn,
) -> Result<(), ()> {
    for index in 0..source.len() {
        let value = source.rows::<T>()[index].clone();
        destination.push::<T>(value);
        copy_row_ticks(source, destination, index);
    }
    Ok(())
}

/// Copies a column whose rows own no heap data, byte for byte.
fn copy_column_bytes(
    source: &ComponentColumn,
    destination: &mut ComponentColumn,
) -> Result<(), ()> {
    for index in 0..source.len() {
        let row = source.bytes(index).ok_or(())?;
        destination.push_bytes(row).map_err(|_| ())?;
        copy_row_ticks(source, destination, index);
    }
    Ok(())
}

/// Puts the source's change-detection ticks on the row just pushed.
///
/// The ticks travel with the storage, so a copy leaves change detection where
/// the original had it.
fn copy_row_ticks(source: &ComponentColumn, destination: &mut ComponentColumn, index: usize) {
    if let Some(ticks) = source.row_ticks(index) {
        destination.set_row_ticks(index, *ticks);
    }
}

impl EntityStorage {
    /// Copies every entity, component and counter of this storage.
    ///
    /// Entity handles, the allocator's next id and free list, the stable ids,
    /// the locations and the change ticks are identical in the copy. Each
    /// component column is copied by the rule that fits its type:
    ///
    /// - a type that recorded a clone function is cloned row by row;
    /// - a foreign (descriptor) column and a native type whose declared layout
    ///   holds no `vec:`, `dynbuf:` or `string` field are copied byte for byte;
    /// - anything else makes the whole copy fail, naming every such type.
    ///
    /// # Errors
    ///
    /// Returns [`UncopyableComponents`] with one name per type that is neither
    /// cloneable nor plain data. A column whose component id no longer has a
    /// storage factory is uncopyable for the same reason: its rows can only be
    /// read through code the reload has replaced.
    ///
    /// The play session is the caller, so it is compiled in ahead of one.
    pub(crate) fn duplicate(&self, world: &World) -> Result<EntityStorage, UncopyableComponents> {
        let mut duplicate = EntityStorage {
            next_fresh_entity_id: self.next_fresh_entity_id,
            free_entity_ids: self.free_entity_ids.clone(),
            entity_ids: self.entity_ids.clone(),
            entity_handles: self.entity_handles.clone(),
            entity_id_fallback_counter: self.entity_id_fallback_counter,
            archetypes: HashMap::with_capacity(self.archetypes.len()),
            entity_locations: self.entity_locations.clone(),
            archetype_generation: self.archetype_generation,
        };
        let mut uncopyable_type_names: Vec<String> = Vec::new();

        for (archetype_id, archetype) in &self.archetypes {
            let mut copied_columns = ComponentColumns::new();
            for &component_id in &archetype.component_types {
                let Some(source_column) = archetype.component_storages.get(component_id) else {
                    continue;
                };
                let Some(mut destination_column) =
                    new_column_for(world, component_id, source_column)
                else {
                    push_type_name(&mut uncopyable_type_names, world, component_id);
                    continue;
                };
                let copied = match world.component_clone_functions.get(&component_id) {
                    Some(clone_column) => clone_column(source_column, &mut destination_column),
                    None if copies_as_bytes(world, component_id) => {
                        copy_column_bytes(source_column, &mut destination_column)
                    }
                    None => {
                        push_type_name(&mut uncopyable_type_names, world, component_id);
                        continue;
                    }
                };
                match copied {
                    Ok(()) => copied_columns.insert(component_id, destination_column),
                    Err(()) => push_type_name(&mut uncopyable_type_names, world, component_id),
                }
            }
            duplicate.archetypes.insert(
                *archetype_id,
                Archetype {
                    id: archetype.id,
                    component_types: archetype.component_types.clone(),
                    component_mask: archetype.component_mask,
                    component_storages: copied_columns,
                    entities: archetype.entities.clone(),
                },
            );
        }

        if !uncopyable_type_names.is_empty() {
            return Err(UncopyableComponents {
                type_names: uncopyable_type_names,
            });
        }
        Ok(duplicate)
    }
}

/// Builds an empty column of the same kind as `source_column`.
///
/// `None` when the component id no longer has a storage factory: the column
/// outlived its registration, and only the retiring image's code could read it.
fn new_column_for(
    world: &World,
    component_id: ComponentId,
    source_column: &ComponentColumn,
) -> Option<ComponentColumn> {
    match world.storage_factories.get(&component_id) {
        Some(StorageFactory::Native(column_info)) => {
            ComponentColumn::from_native_info(*column_info, source_column.schema_hash()).ok()
        }
        Some(StorageFactory::Descriptor(layout)) => ComponentColumn::new(layout.clone()).ok(),
        None => None,
    }
}

/// Whether a column's rows may be copied byte for byte.
///
/// True for a foreign (descriptor) column, whose rows are plain data by
/// construction, and for a native type that declared a field layout with no
/// container field. A byte copy of a type that owns heap data would free the
/// same buffer twice, so a type without a layout is not copied that way.
fn copies_as_bytes(world: &World, component_id: ComponentId) -> bool {
    if matches!(
        world.storage_factories.get(&component_id),
        Some(StorageFactory::Descriptor(_))
    ) {
        return true;
    }
    world
        .component_field_layouts
        .get(&component_id)
        .is_some_and(|layout| {
            layout
                .fields()
                .iter()
                .all(|field| !crate::component_field::is_container_tag(field.type_tag))
        })
}

/// Adds a component type's registered name to `names`, once.
fn push_type_name(names: &mut Vec<String>, world: &World, component_id: ComponentId) {
    let name = world
        .component_registry
        .registered_components()
        .find(|(registered_id, _, _)| *registered_id == component_id)
        .map(|(_, _, name)| name.to_string())
        .unwrap_or_else(|| format!("component {:?}", component_id));
    if !names.contains(&name) {
        names.push(name);
    }
}

// =============================================================================
// Copying the game-state resources
// =============================================================================

/// One resource type's cloning function: copies the live value into a new
/// erased resource, or answers `None` when the value is not that type.
///
/// Like [`CloneColumnFn`], it is monomorphised in the DLL that inserted the
/// value, so it is called only while that image is mapped.
pub(crate) type ResourceCloneFn =
    fn(&crate::resource::ErasedResource) -> Option<crate::resource::ErasedResource>;

/// The copies of the game-state resources, taken at Play and put back at Stop.
///
/// Only [`PlayState::Restore`] resources are here. A `Keep` or `Rebuild`
/// resource is absent by design, so Stop leaves its live value alone.
#[derive(Default)]
pub(crate) struct ResourceCopy {
    /// The copied value of each restored resource, keyed by its id.
    pub(crate) values: HashMap<crate::resource::ResourceId, crate::resource::ErasedResource>,
    /// The persisted JSON of each restored resource the engine could not copy
    /// as a value, keyed by its id.
    ///
    /// A resource that is neither `Clone` nor a foreign payload but declares
    /// persistence - a settings document, a save file's shape - is copied by
    /// its own byte form. Stop restores it through the same deserializer a
    /// snapshot load uses, so the value that comes back is the type's own.
    pub(crate) persisted_values: HashMap<crate::resource::ResourceId, Vec<u8>>,
    /// The types whose copy a reload dropped because it dropped the live value
    /// too, keyed by their id, so Stop can name them and leaves their live
    /// value alone.
    ///
    /// A copy is a value of a module's type, so the reload releases it while
    /// the image is still mapped. There is nothing left to put back, and the
    /// value the game holds under that id is the one the person sees; Stop
    /// warns rather than silently keeping it.
    pub(crate) dropped_by_reload: Vec<(crate::resource::ResourceId, String)>,
}

/// The resource types a copy could not include.
///
/// Every name is listed, so one refusal tells a person everything to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UncopyableResources {
    /// The names of the resources the copy could not include.
    pub(crate) type_names: Vec<String>,
}

/// The clone function a `Clone` resource's insertion records.
fn clone_resource<T: crate::resource::Resource + Clone>(
    source: &crate::resource::ErasedResource,
) -> Option<crate::resource::ErasedResource> {
    let value = source.get::<T>()?.clone();
    Some(crate::resource::ErasedResource::new(value))
}

impl World {
    /// Records `T`'s clone function, so a Play copy can duplicate this
    /// resource's value.
    ///
    /// The insert path calls it through
    /// [`CloneResourceProbe`](crate::resource::CloneResourceProbe) for a
    /// resource that implements `Clone`.
    pub fn record_resource_clone<T: crate::resource::Resource + Clone>(&mut self) {
        self.resource_clone_functions
            .insert(crate::resource::ResourceId::of::<T>(), clone_resource::<T>);
    }

    /// Copies every resource the play mode restores.
    ///
    /// A [`PlayState::Restore`](crate::resource::PlayState::Restore) resource is
    /// copied by its recorded clone function, byte for byte when it is a foreign
    /// payload, or as its persisted bytes when the type declares persistence and
    /// is not `Clone`. A `Keep` or `Rebuild` resource is not copied: Stop leaves
    /// those live.
    ///
    /// # Errors
    ///
    /// Returns [`UncopyableResources`] naming every `Restore` resource that is
    /// none of those three. Nothing is changed when it fails.
    ///
    /// The play session is the caller, so it is compiled in ahead of one.
    pub(crate) fn copy_game_state_resources(&self) -> Result<ResourceCopy, UncopyableResources> {
        let mut copy = ResourceCopy::default();
        let mut uncopyable_type_names: Vec<String> = Vec::new();

        for (resource_id, resource) in &self.resources {
            let play_state = self
                .resource_play_states
                .get(resource_id)
                .copied()
                .unwrap_or_default();
            if play_state != crate::resource::PlayState::Restore {
                continue;
            }
            if let Some(clone_resource) = self.resource_clone_functions.get(resource_id) {
                if let Some(copied) = clone_resource(resource) {
                    copy.values.insert(*resource_id, copied);
                    continue;
                }
            }
            if resource.is_foreign() {
                let schema_hash = self
                    .resource_factories
                    .get(resource_id)
                    .and_then(|ops| ops.schema_hash)
                    .unwrap_or(0);
                copy.values.insert(
                    *resource_id,
                    crate::resource::ErasedResource::new_foreign(
                        resource.bytes(),
                        resource.align(),
                        schema_hash,
                    ),
                );
                continue;
            }
            // A persistable resource that is not `Clone` is copied by its own
            // persisted bytes, which Stop reads back through the type's
            // deserializer.
            if let Some(serialize) = self.persist_resource_serializers.get(resource_id) {
                if let Some(bytes) = serialize(self) {
                    copy.persisted_values.insert(*resource_id, bytes);
                    continue;
                }
            }
            let name = self
                .resource_factories
                .get(resource_id)
                .and_then(|ops| ops.declaring_type_name)
                .map(str::to_string)
                .unwrap_or_else(|| format!("resource {:?}", resource_id));
            if !uncopyable_type_names.contains(&name) {
                uncopyable_type_names.push(name);
            }
        }

        if !uncopyable_type_names.is_empty() {
            return Err(UncopyableResources {
                type_names: uncopyable_type_names,
            });
        }
        Ok(copy)
    }
}

// =============================================================================
// World — The Held Slot
// =============================================================================

impl World {
    /// Records `T`'s row-cloning function, so a storage copy can clone rows of
    /// this type.
    ///
    /// The generated registration code calls it through
    /// [`CloneValueProbe`](crate::component_registry::CloneValueProbe) for a
    /// component that implements `Clone`.
    pub fn record_component_clone<T: Component + Clone>(&mut self) {
        self.component_clone_functions
            .insert(ComponentId::of::<T>(), clone_column::<T>);
    }

    /// Copies the live entity storage, with the same handles and every
    /// component cloned.
    ///
    /// The play session is its caller: it holds the copy while the game runs
    /// and swaps it back on Stop.
    ///
    /// # Errors
    ///
    /// Returns [`UncopyableComponents`] naming every component type the copy
    /// could not include. Nothing is changed when it fails.
    ///
    /// The play session is the caller, so it is compiled in ahead of one.
    pub(crate) fn copy_entity_storage(&self) -> Result<EntityStorage, UncopyableComponents> {
        self.storage.duplicate(self)
    }

    /// Puts `storage` into the held slot, replacing whatever it held.
    ///
    /// Replacing is deliberate: a caller that enters a second session without
    /// leaving the first gives up the older copy, and the archetype generation
    /// moves so no query keeps a list built from either value.
    pub(crate) fn hold_entity_storage(&mut self, storage: EntityStorage) {
        self.held_entity_storage = Some(storage);
        self.storage.bump_archetype_generation();
    }

    /// Takes the held storage out of its slot, leaving the world holding none.
    fn take_held_entity_storage(&mut self) -> Option<EntityStorage> {
        let taken = self.held_entity_storage.take();
        self.storage.bump_archetype_generation();
        taken
    }

    /// Puts the held storage back in the live slot and drops the play storage.
    ///
    /// This is the Stop half of a play session: the live storage holds whatever
    /// the game did, the held one holds the edit state, and after the call the
    /// edit state live again. The play storage's rows are dropped through the
    /// current image's tables, which is valid because it was built by them.
    ///
    /// Returns `false` when no storage was held, which means no session was
    /// running and there is nothing to restore.
    pub(crate) fn restore_held_entity_storage(&mut self) -> bool {
        let Some(mut held) = self.held_entity_storage.take() else {
            return false;
        };
        // `held` takes the play storage and is dropped at the end of the call.
        std::mem::swap(&mut self.storage, &mut held);
        self.storage.bump_archetype_generation();
        self.restore_epoch = self.restore_epoch.wrapping_add(1);
        // Systems cannot read the world, so the number is published as a
        // resource for the managers that mirror the world elsewhere.
        self.insert_resource(crate::resource::RestoreEpoch(self.restore_epoch));
        true
    }

    /// The epoch of the last restore; see `World::restore_epoch`.
    pub fn restore_epoch(&self) -> u64 {
        self.restore_epoch
    }

    /// Puts one resource value back, replacing whatever is live.
    ///
    /// The play session's Stop calls it for every copied `Restore` resource.
    pub(crate) fn put_back_resource(
        &mut self,
        resource_id: crate::resource::ResourceId,
        value: crate::resource::ErasedResource,
    ) {
        self.resources.insert(resource_id, value);
    }

    /// Removes one resource's live value.
    ///
    /// A `Restore` resource the game inserted during play has no copy, so Stop
    /// removes it rather than leaving a play-time value in the edit world.
    pub(crate) fn remove_resource_value(&mut self, resource_id: crate::resource::ResourceId) {
        self.resources.remove(&resource_id);
        self.resource_ticks.remove(&resource_id);
    }

    /// Runs `operation` over the live storage and, when the world holds one,
    /// over the held storage as well.
    ///
    /// The held storage is swapped into the live slot for the second run, so
    /// the operation is written once against `&mut World` and needs no
    /// storage argument. Nothing else in the world changes for that run except
    /// the archetype generation, which moves on both swaps.
    ///
    /// A reload pass uses this because a column's function table points into
    /// the image that registered its type: a held column left alone would call
    /// code the reload has already unmapped.
    pub(crate) fn run_on_every_entity_storage(&mut self, mut operation: impl FnMut(&mut World)) {
        operation(self);
        let Some(mut held) = self.take_held_entity_storage() else {
            return;
        };
        // The swap puts the held storage live for the second run and keeps the
        // live one in `held` until it goes back.
        std::mem::swap(&mut self.storage, &mut held);
        self.storage.bump_archetype_generation();
        operation(self);
        std::mem::swap(&mut self.storage, &mut held);
        self.storage.bump_archetype_generation();
        self.hold_entity_storage(held);
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh storage is empty, and its generation counter is what the
    /// swaps increment rather than a query's own state.
    #[test]
    fn a_new_storage_is_empty_and_its_generation_moves_on_request() {
        let mut storage = EntityStorage::new();
        assert_eq!(storage.entity_count(), 0);
        assert!(storage.archetypes.is_empty());
        let before = storage.archetype_generation;
        storage.bump_archetype_generation();
        assert_ne!(storage.archetype_generation, before);
    }
}
