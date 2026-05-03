// ============================================================================
// World - Central ECS State Management
// ============================================================================
//! The World is the central container for all ECS data.
//!
//! It manages entities, archetypes, and provides the primary interface for
//! creating entities and managing components.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use trait_type_map::{TraitAccessible, TraitTypeMap, VecFamily};

use crate::ecs::archetype::{Archetype, ArchetypeId, StorageFactory};
use crate::ecs::commands::CommandQueue;
use crate::ecs::component::{Component, ComponentId, ComponentMask, ComponentRegistry};
use crate::ecs::entity::Entity;
use crate::ecs::resource::{Resource, ResourceId};
use crate::ecs::scripting::{ScriptComponent, ScriptContext};

/// Function that copies a component from one storage to another at given indices
type ComponentCopier = Arc<
    dyn Fn(
            &TraitTypeMap<dyn Component, VecFamily>,
            &mut TraitTypeMap<dyn Component, VecFamily>,
            usize,
        ) + Send
        + Sync,
>;

/// Function that updates a script component
/// Takes: (storage, index, entity, world_ptr, commands_ptr)
/// Uses raw pointers to safely create ScriptContext within the closure
type ScriptUpdater = Arc<
    dyn Fn(
            &mut TraitTypeMap<dyn Component, VecFamily>,
            usize,
            Entity,
            *mut World,
            *mut CommandQueue,
        ) + Send
        + Sync,
>;

/// EntityLocation tracks where an entity is stored in the archetype system
#[derive(Clone, Copy)]
pub(crate) struct EntityLocation {
    pub(crate) archetype_id: ArchetypeId,
    pub(crate) index_in_archetype: usize,
}

/// Error type for `add_component` operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddComponentError {
    /// The entity does not exist (was destroyed or never created)
    EntityNotFound,
    /// The entity already has a component of this type
    ComponentAlreadyExists,
}

impl std::fmt::Display for AddComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddComponentError::EntityNotFound => write!(f, "entity not found"),
            AddComponentError::ComponentAlreadyExists => {
                write!(f, "component already exists on entity")
            }
        }
    }
}

impl std::error::Error for AddComponentError {}

/// Error type for `remove_component` operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveComponentError {
    /// The entity does not exist (was destroyed or never created)
    EntityNotFound,
    /// The entity does not have a component of this type
    ComponentNotFound,
}

impl std::fmt::Display for RemoveComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoveComponentError::EntityNotFound => write!(f, "entity not found"),
            RemoveComponentError::ComponentNotFound => write!(f, "component not found on entity"),
        }
    }
}

impl std::error::Error for RemoveComponentError {}

/// World manages all entities, archetypes, and global components
///
/// This is the central hub of the ECS. It:
/// - Allocates entity IDs
/// - Manages archetype storage
/// - Tracks entity locations
/// - Stores global (singleton) components
/// - Maintains component type registry for creating archetype storage
pub struct World {
    next_free_entity_id: u64,
    /// Free list of recycled entity IDs with their next generation. Stored as (id, next_generation) pairs
    free_entity_ids: Vec<(u64, u32)>,
    /// All archetypes in the world
    pub(crate) archetypes: HashMap<ArchetypeId, Archetype>,
    /// Next available archetype ID
    next_free_archetype_id: usize,
    /// Tracks where each entity is located in the archetype system
    pub(crate) entity_locations: HashMap<Entity, EntityLocation>,
    /// Lookup table mapping component masks to archetype IDs
    archetype_lookup: HashMap<ComponentMask, ArchetypeId>,
    /// Global components not attached to any entity
    pub(crate) global_components: HashMap<ComponentId, Box<dyn Any + Send>>,
    /// Storage factories for creating component storage by TypeId
    storage_factories: HashMap<ComponentId, StorageFactory>,
    /// Component copiers for moving entities between archetypes
    pub(crate) component_copiers: HashMap<ComponentId, ComponentCopier>,
    /// Script component types (ComponentId, component mask bit)
    script_components: Vec<(ComponentId, u8)>,
    /// Script updaters for calling update() on script components
    script_updaters: HashMap<ComponentId, ScriptUpdater>,
    /// Component registry for bit indices and names
    pub(crate) component_registry: ComponentRegistry,
    /// Resources (singleton data) stored by type
    pub(crate) resources: HashMap<ResourceId, Box<dyn Any + Send + Sync>>,
}

impl World {
    /// Create a new empty World
    pub fn new() -> Self {
        Self {
            next_free_entity_id: 0,
            free_entity_ids: Vec::new(),
            archetypes: HashMap::new(),
            next_free_archetype_id: 0,
            entity_locations: HashMap::new(),
            archetype_lookup: HashMap::new(),
            global_components: HashMap::new(),
            storage_factories: HashMap::new(),
            component_copiers: HashMap::new(),
            script_components: Vec::new(),
            script_updaters: HashMap::new(),
            component_registry: ComponentRegistry::new(),
            resources: HashMap::new(),
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// Register a component type with the World
    ///
    /// This must be called for each component type before it can be used.
    pub fn register_component<T>(&mut self)
    where
        T: Component + TraitAccessible<dyn Component> + Clone,
    {
        let component_id = ComponentId::of::<T>();

        // Register component (bit index + name)
        self.component_registry.register::<T>();

        self.storage_factories.insert(
            component_id,
            Box::new(|map: &mut TraitTypeMap<dyn Component, VecFamily>| {
                map.register_type_storage::<T>();
            }),
        );

        // Register copier function for this component type
        self.component_copiers.insert(
            component_id,
            Arc::new(
                |src: &TraitTypeMap<dyn Component, VecFamily>,
                 dst: &mut TraitTypeMap<dyn Component, VecFamily>,
                 index: usize| {
                    let component = src.get_storage::<T>().get(index);
                    dst.get_storage_mut::<T>().push(component.clone());
                },
            ),
        );
    }

    /// Register a script component type with the World
    ///
    /// Script components have an update() method that gets called by update_scripts().
    /// This must be called for each script component type before it can be used.
    pub fn register_script_component<T>(&mut self)
    where
        T: ScriptComponent + TraitAccessible<dyn Component> + Clone,
    {
        // First register as a normal component
        self.register_component::<T>();

        // Then track it as a script component
        let component_id = ComponentId::of::<T>();
        if let Some(bit) = self.component_registry.get_bit(&component_id) {
            self.script_components.push((component_id, bit));

            // Register updater callback for this script component
            self.script_updaters.insert(
                component_id,
                Arc::new(
                    |storage: &mut TraitTypeMap<dyn Component, VecFamily>,
                     index: usize,
                     entity: Entity,
                     world_ptr: *mut World,
                     commands_ptr: *mut CommandQueue| {
                        // Get mutable reference to the component
                        let component = storage.get_storage_mut::<T>().get_mut(index);
                        // SAFETY: We create a ScriptContext with mutable world access.
                        // This is safe because:
                        // - The script's own component is accessed via `storage`, not through world
                        // - Different component types have separate storage (no aliasing)
                        // - Structural changes are deferred through commands
                        unsafe {
                            let mut ctx =
                                ScriptContext::new(&mut *world_ptr, &mut *commands_ptr, entity);
                            component.update(&mut ctx);
                        }
                    },
                ),
            );
        }
    }

    /// Update all script components
    ///
    /// Calls update() on every script component in the world.
    /// Scripts receive a `ScriptContext` with:
    /// - Read-only world access for queries
    /// - Deferred command queue for structural changes
    ///
    /// This ensures all structural changes (add/remove component, destroy entity)
    /// are automatically deferred, preventing use-after-free bugs.
    pub(crate) fn update_scripts(&mut self, commands: &mut CommandQueue) {
        // Collect script component info to avoid borrow issues
        let script_info: Vec<(ComponentId, u8)> = self.script_components.clone();

        for (component_id, comp_bit) in script_info {
            // Get the updater for this component type
            let updater = match self.script_updaters.get(&component_id) {
                Some(u) => Arc::clone(u),
                None => continue,
            };

            // Collect entities that have this script component
            let mut entities_to_update: Vec<(Entity, ArchetypeId, usize)> = Vec::new();

            for (archetype_id, archetype) in &self.archetypes {
                // Check if this archetype has the script component using bitmask
                let mut mask = ComponentMask::empty();
                mask.set(comp_bit);

                if archetype.matches_mask(&mask) {
                    // Collect all entities in this archetype
                    for (index, &entity) in archetype.entities.iter().enumerate() {
                        entities_to_update.push((entity, *archetype_id, index));
                    }
                }
            }

            // Now update each entity's script component
            let world_ptr = self as *mut World;
            let commands_ptr = commands as *mut CommandQueue;

            for (entity, archetype_id, index) in entities_to_update {
                if let Some(archetype) = self.archetypes.get_mut(&archetype_id) {
                    // Call the updater with mutable storage access
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

    // ========================================================================
    // Resource Management
    // ========================================================================

    /// Insert a resource (singleton data not attached to any entity)
    ///
    /// Resources are global state such as time, input, configuration, etc.
    /// If a resource of this type already exists, it is replaced.
    pub fn insert_resource<T: Resource>(&mut self, resource: T) {
        self.resources
            .insert(ResourceId::of::<T>(), Box::new(resource));
    }

    /// Get immutable reference to a resource
    pub fn get_resource<T: Resource>(&self) -> Option<&T> {
        self.resources
            .get(&ResourceId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    /// Get mutable reference to a resource
    pub fn get_resource_mut<T: Resource>(&mut self) -> Option<&mut T> {
        self.resources
            .get_mut(&ResourceId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    /// Remove a resource and return it if it existed
    pub fn remove_resource<T: Resource>(&mut self) -> Option<T> {
        self.resources
            .remove(&ResourceId::of::<T>())
            .and_then(|boxed| boxed.downcast::<T>().ok())
            .map(|boxed| *boxed)
    }

    /// Check if a resource exists
    pub fn has_resource<T: Resource>(&self) -> bool {
        self.resources.contains_key(&ResourceId::of::<T>())
    }

    /// Check if an entity exists and is valid (not destroyed/recycled)
    ///
    /// Returns true if the entity exists in the world with the correct generation.
    /// Returns false if the entity was destroyed or if its ID was recycled with a new generation.
    pub fn is_entity_valid(&self, entity: Entity) -> bool {
        self.entity_locations.contains_key(&entity)
    }

    /// Get immutable reference to a component on an entity
    ///
    /// Returns None if the entity doesn't exist or doesn't have the component.
    pub fn get_component<T>(&self, entity: Entity) -> Option<&T>
    where
        T: Component + TraitAccessible<dyn Component>,
    {
        // Get component bit for O(1) archetype check
        let component_id = ComponentId::of::<T>();
        let bit = self.component_registry.get_bit(&component_id)?;

        // Get entity location
        let location = self.entity_locations.get(&entity)?;

        // Get archetype
        let archetype = self.archetypes.get(&location.archetype_id)?;

        // Check if archetype has this component type (O(1) bitmask check)
        if !archetype.has_component_bit(bit) {
            return None;
        }

        // Get component from storage
        Some(
            archetype
                .component_storages
                .get_storage::<T>()
                .get(location.index_in_archetype),
        )
    }

    /// Get mutable reference to a component on an entity
    ///
    /// Returns None if the entity doesn't exist or doesn't have the component.
    pub fn get_component_mut<T>(&mut self, entity: Entity) -> Option<&mut T>
    where
        T: Component + TraitAccessible<dyn Component>,
    {
        // Get component bit for O(1) archetype check
        let component_id = ComponentId::of::<T>();
        let bit = self.component_registry.get_bit(&component_id)?;

        // Get entity location
        let location = self.entity_locations.get(&entity)?;
        let archetype_id = location.archetype_id;
        let index = location.index_in_archetype;

        // Get archetype
        let archetype = self.archetypes.get_mut(&archetype_id)?;

        // Check if archetype has this component type (O(1) bitmask check)
        if !archetype.has_component_bit(bit) {
            return None;
        }

        // Get component from storage
        Some(
            archetype
                .component_storages
                .get_storage_mut::<T>()
                .get_mut(index),
        )
    }

    /// Get raw mutable pointer to a component on an entity
    ///
    /// This is used by ScriptContext to avoid aliasing issues when a script
    /// accesses components of its own type. By returning a raw pointer instead
    /// of `&mut T`, we opt out of Rust's noalias optimization.
    ///
    /// Returns None if the entity doesn't exist or doesn't have the component.
    pub(crate) fn get_component_ptr_mut<T>(&mut self, entity: Entity) -> Option<*mut T>
    where
        T: Component + TraitAccessible<dyn Component>,
    {
        // Get component bit for O(1) archetype check
        let component_id = ComponentId::of::<T>();
        let bit = self.component_registry.get_bit(&component_id)?;

        // Get entity location
        let location = self.entity_locations.get(&entity)?;
        let archetype_id = location.archetype_id;
        let index = location.index_in_archetype;

        // Get archetype
        let archetype = self.archetypes.get_mut(&archetype_id)?;

        // Check if archetype has this component type (O(1) bitmask check)
        if !archetype.has_component_bit(bit) {
            return None;
        }

        // Get raw pointer to component - avoids creating intermediate &mut
        let storage = archetype.component_storages.get_storage_mut::<T>();
        Some(storage.get_mut(index) as *mut T)
    }

    /// Allocate a new unique entity ID
    ///
    /// Reuses IDs from the free list when available, incrementing the generation
    /// to invalidate any stale handles. Otherwise allocates a fresh ID.
    pub(crate) fn allocate_entity(&mut self) -> Entity {
        // Try to reuse an ID from the free list
        if let Some((id, generation)) = self.free_entity_ids.pop() {
            Entity { id, generation }
        } else {
            // Allocate a fresh ID
            let entity = Entity {
                id: self.next_free_entity_id,
                generation: 0,
            };
            self.next_free_entity_id += 1;
            entity
        }
    }

    /// Get or create an archetype for a given set of components
    ///
    /// Archetypes are cached and reused for entities with the same component set.
    /// The lookup uses ComponentMask for O(1) hash lookup, avoiding repeated sorting.
    pub(crate) fn get_or_create_archetype(
        &mut self,
        component_ids: Vec<ComponentId>,
    ) -> ArchetypeId {
        // Build component mask first - this is used for the fast lookup path
        // The mask uniquely identifies the component set regardless of order
        let mut component_mask = ComponentMask::empty();
        for component_id in &component_ids {
            if let Some(bit) = self.component_registry.get_bit(component_id) {
                component_mask.set(bit);
            }
        }

        // Hot path: archetype already exists (most common case)
        if let Some(&archetype_id) = self.archetype_lookup.get(&component_mask) {
            return archetype_id;
        }

        // Cold path: create new archetype (only sort when actually creating)
        let mut sorted_ids = component_ids;
        sorted_ids.sort();

        let new_archetype_id = ArchetypeId(self.next_free_archetype_id);
        self.next_free_archetype_id += 1;

        // Create archetype with storage for all component types
        let new_archetype = Archetype::new(
            new_archetype_id,
            sorted_ids,
            component_mask,
            &self.storage_factories,
        );
        self.archetypes.insert(new_archetype_id, new_archetype);
        self.archetype_lookup
            .insert(component_mask, new_archetype_id);

        new_archetype_id
    }

    /// Start building a new entity
    ///
    /// Returns an EntityBuilder that allows fluent API for adding components.
    pub fn create_entity(&'_ mut self) -> EntityBuilder<'_> {
        let entity = self.allocate_entity();
        EntityBuilder {
            world: self,
            entity,
            components: Vec::new(),
        }
    }

    /// Insert an entity with its components into the appropriate archetype
    ///
    /// Note: With TraitTypeMap, we need concrete types to push components.
    /// Components are added via EntityBuilder which has access to concrete types.
    pub(crate) fn insert_entity_with_components<F>(
        &mut self,
        entity: Entity,
        component_ids: Vec<ComponentId>,
        insert_fn: F,
    ) where
        F: FnOnce(&mut TraitTypeMap<dyn Component, VecFamily>),
    {
        let archetype_id = self.get_or_create_archetype(component_ids);

        let archetype = self
            .archetypes
            .get_mut(&archetype_id)
            .expect("archetype must exist after get_or_create_archetype");
        let index: usize = archetype.entities.len();

        // Add entity to archetype
        archetype.entities.push(entity);

        // Use the provided closure to insert components with their concrete types
        insert_fn(&mut archetype.component_storages);

        self.entity_locations.insert(
            entity,
            EntityLocation {
                archetype_id,
                index_in_archetype: index,
            },
        );
    }

    /// Move an entity to a new archetype, preserving existing components
    ///
    /// This is used when adding/removing components from an existing entity.
    /// The move_fn closure receives:
    /// 1. Old archetype storage (to read existing components)
    /// 2. New archetype storage (to write all components)
    /// 3. Index of the entity in old archetype
    pub(crate) fn move_entity_to_archetype<F>(
        &mut self,
        entity: Entity,
        new_component_ids: Vec<ComponentId>,
        move_fn: F,
    ) where
        F: FnOnce(
            &TraitTypeMap<dyn Component, VecFamily>,
            &mut TraitTypeMap<dyn Component, VecFamily>,
            usize,
        ),
    {
        // Get current location
        let old_location = match self.entity_locations.get(&entity) {
            Some(loc) => *loc,
            None => {
                println!("  [Warning] Entity {:?} not found in world", entity.id);
                return;
            }
        };

        let old_archetype_id = old_location.archetype_id;
        let old_index = old_location.index_in_archetype;

        // Get or create new archetype
        let new_archetype_id = self.get_or_create_archetype(new_component_ids);

        // If same archetype, nothing to do (shouldn't happen for add_component)
        if old_archetype_id == new_archetype_id {
            println!(
                "  [Warning] Entity {:?} already has this component",
                entity.id
            );
            return;
        }

        // We need to:
        // 1. Copy components from old to new archetype
        // 2. Remove entity from old archetype
        // 3. Add entity to new archetype

        // SAFETY: We need to access two archetypes simultaneously
        // We ensure old_archetype_id != new_archetype_id above
        let old_arch_ptr = self
            .archetypes
            .get(&old_archetype_id)
            .expect("source archetype must exist during entity migration")
            as *const Archetype;
        let new_arch_ptr = self
            .archetypes
            .get_mut(&new_archetype_id)
            .expect("destination archetype must exist after get_or_create_archetype")
            as *mut Archetype;

        unsafe {
            let old_arch = &*old_arch_ptr;
            let new_arch = &mut *new_arch_ptr;

            let new_index = new_arch.entities.len();
            new_arch.entities.push(entity);

            // Call the move function to copy components
            move_fn(
                &old_arch.component_storages,
                &mut new_arch.component_storages,
                old_index,
            );

            // Update entity location
            self.entity_locations.insert(
                entity,
                EntityLocation {
                    archetype_id: new_archetype_id,
                    index_in_archetype: new_index,
                },
            );
        }

        // Remove entity from old archetype using swap_remove for O(1) removal
        let old_archetype = self.archetypes.get_mut(&old_archetype_id).unwrap();

        if old_index < old_archetype.entities.len() {
            old_archetype.entities.swap_remove(old_index);

            // Update the location of the entity that was swapped (if any)
            if old_index < old_archetype.entities.len() {
                let swapped_entity = old_archetype.entities[old_index];
                if let Some(swapped_location) = self.entity_locations.get_mut(&swapped_entity) {
                    swapped_location.index_in_archetype = old_index;
                }
            }

            // Also swap_remove from all component storages to keep them in sync
            let component_types: Vec<ComponentId> = old_archetype.component_types.clone();
            for component_id in component_types {
                if let Some(storage) = old_archetype
                    .component_storages
                    .get_trait_storage_mut(component_id.0)
                {
                    storage.swap_remove(old_index);
                }
            }
        }

        // Clean up empty archetype - remove it from world to prevent memory leaks
        if old_archetype.entities.is_empty() {
            let old_mask = old_archetype.component_mask;
            self.archetypes.remove(&old_archetype_id);
            self.archetype_lookup.remove(&old_mask);
        }
    }

    /// Remove an entity from the world completely
    ///
    /// This removes the entity from its archetype and updates all tracking structures.
    /// Returns true if the entity was found and removed, false otherwise.
    pub fn destroy_entity(&mut self, entity: Entity) -> bool {
        // Get current location
        let location = match self.entity_locations.remove(&entity) {
            Some(loc) => loc,
            None => return false, // Entity doesn't exist
        };

        let archetype = match self.archetypes.get_mut(&location.archetype_id) {
            Some(arch) => arch,
            None => return false,
        };

        let old_index = location.index_in_archetype;

        // Use swap_remove for O(1) removal
        if old_index < archetype.entities.len() {
            archetype.entities.swap_remove(old_index);

            // Update the location of the entity that was swapped (if any)
            if old_index < archetype.entities.len() {
                let swapped_entity = archetype.entities[old_index];
                if let Some(swapped_location) = self.entity_locations.get_mut(&swapped_entity) {
                    swapped_location.index_in_archetype = old_index;
                }
            }

            // Also swap_remove from all component storages to keep them in sync
            // Clone the component_types to avoid borrow issues
            let component_types: Vec<ComponentId> = archetype.component_types.clone();
            for component_id in component_types {
                if let Some(storage) = archetype
                    .component_storages
                    .get_trait_storage_mut(component_id.0)
                {
                    storage.swap_remove(old_index);
                }
            }
        }

        // Clean up empty archetype - remove it from world to prevent memory leaks
        let archetype_id = location.archetype_id;
        if archetype.entities.is_empty() {
            let mask = archetype.component_mask;
            self.archetypes.remove(&archetype_id);
            self.archetype_lookup.remove(&mask);
        }

        // Add entity ID to free list with incremented generation for recycling.
        // This allows the ID to be reused, but with a new generation to invalidate old handles.
        self.free_entity_ids
            .push((entity.id, entity.generation.wrapping_add(1)));

        true
    }

    /// Remove a component from an entity, moving it to a new archetype
    ///
    /// Returns `Ok(())` if the component was removed successfully.
    /// Returns `Err(RemoveComponentError::EntityNotFound)` if the entity doesn't exist.
    /// Returns `Err(RemoveComponentError::ComponentNotFound)` if the entity doesn't have the component.
    pub fn remove_component<T: Component>(
        &mut self,
        entity: Entity,
    ) -> Result<(), RemoveComponentError> {
        let component_id = ComponentId::of::<T>();

        // Get current location
        let location = match self.entity_locations.get(&entity) {
            Some(loc) => *loc,
            None => return Err(RemoveComponentError::EntityNotFound),
        };

        let old_archetype = match self.archetypes.get(&location.archetype_id) {
            Some(arch) => arch,
            None => return Err(RemoveComponentError::EntityNotFound),
        };

        // Check if entity has this component
        if !old_archetype.component_types.contains(&component_id) {
            return Err(RemoveComponentError::ComponentNotFound);
        }

        // Build new component list without the removed component
        let new_component_ids: Vec<ComponentId> = old_archetype
            .component_types
            .iter()
            .filter(|&id| *id != component_id)
            .cloned()
            .collect();

        // If no components left, destroy the entity instead
        if new_component_ids.is_empty() {
            self.destroy_entity(entity);
            return Ok(());
        }

        // Collect copiers for all components except the one being removed
        let copiers: Vec<_> = new_component_ids
            .iter()
            .filter_map(|component_id| self.component_copiers.get(component_id).map(Arc::clone))
            .collect();

        // Move entity to new archetype without the removed component
        self.move_entity_to_archetype(
            entity,
            new_component_ids,
            |old_storage, new_storage, old_index| {
                // Copy all components except the removed one
                for copier in copiers.iter() {
                    copier(old_storage, new_storage, old_index);
                }
            },
        );

        Ok(())
    }

    /// Add a component to an existing entity, moving it to a new archetype
    ///
    /// Returns `Ok(())` if the component was added successfully.
    /// Returns `Err(AddComponentError::EntityNotFound)` if the entity doesn't exist.
    /// Returns `Err(AddComponentError::ComponentAlreadyExists)` if the entity already has the component.
    pub fn add_component<T>(
        &mut self,
        entity: Entity,
        component: T,
    ) -> Result<(), AddComponentError>
    where
        T: Component + TraitAccessible<dyn Component> + Clone,
    {
        let component_id = ComponentId::of::<T>();

        // Get current location
        let location = match self.entity_locations.get(&entity) {
            Some(loc) => *loc,
            None => return Err(AddComponentError::EntityNotFound),
        };

        let old_archetype = match self.archetypes.get(&location.archetype_id) {
            Some(arch) => arch,
            None => return Err(AddComponentError::EntityNotFound),
        };

        // Check if entity already has this component
        if old_archetype.component_types.contains(&component_id) {
            return Err(AddComponentError::ComponentAlreadyExists);
        }

        // Build new component list with the added component
        let mut new_component_ids = old_archetype.component_types.clone();
        new_component_ids.push(component_id);
        new_component_ids.sort();

        // Collect copiers for existing components
        let copiers: Vec<_> = old_archetype
            .component_types
            .iter()
            .filter_map(|component_id| self.component_copiers.get(component_id).map(Arc::clone))
            .collect();

        // Move entity to new archetype with the additional component
        self.move_entity_to_archetype(
            entity,
            new_component_ids,
            |old_storage, new_storage, old_index| {
                // Copy all existing components
                for copier in copiers.iter() {
                    copier(old_storage, new_storage, old_index);
                }
                // Add the new component
                new_storage.get_storage_mut::<T>().push(component);
            },
        );

        Ok(())
    }

    /// Remove all empty archetypes from the world
    ///
    /// This cleans up archetypes that no longer contain any entities.
    /// Usually not necessary as empty archetypes can be reused, but useful for memory cleanup.
    pub fn cleanup_empty_archetypes(&mut self) {
        let empty_archetype_ids: Vec<ArchetypeId> = self
            .archetypes
            .iter()
            .filter(|(_, archetype)| archetype.entities.is_empty())
            .map(|(id, _)| *id)
            .collect();

        for archetype_id in empty_archetype_ids {
            if let Some(archetype) = self.archetypes.remove(&archetype_id) {
                // Also remove from lookup table
                self.archetype_lookup.remove(&archetype.component_mask);
            }
        }
    }

    /// Print information about all archetypes in the world
    ///
    /// This displays the component types and entity count for each archetype,
    /// useful for debugging and understanding the current state of the ECS.
    pub fn print_archetypes(&self) {
        println!(
            "\n=== World Archetypes (Total: {}) ===",
            self.archetypes.len()
        );
        for (_, archetype) in self.archetypes.iter() {
            archetype.print_info(&self.component_registry);
        }
        println!("Total entities: {}", self.entity_locations.len());
    }
}

/// Trait for inserting a component into storage
trait ComponentInserter {
    fn insert(self: Box<Self>, storage: &mut TraitTypeMap<dyn Component, VecFamily>);
    fn component_id(&self) -> ComponentId;
}

/// Implementation that captures the concrete component type
struct TypedComponentInserter<T: Component + TraitAccessible<dyn Component>> {
    component: T,
}

impl<T: Component + TraitAccessible<dyn Component>> ComponentInserter
    for TypedComponentInserter<T>
{
    fn insert(self: Box<Self>, storage: &mut TraitTypeMap<dyn Component, VecFamily>) {
        storage.get_storage_mut::<T>().push(self.component);
    }

    fn component_id(&self) -> ComponentId {
        ComponentId::of::<T>()
    }
}

/// Builder for constructing entities with components using a fluent API
///
/// Example:
/// ```ignore
/// world.create_entity()
///     .with(Transform { x: 0.0, y: 0.0, z: 0.0 })
///     .with(Velocity { x: 10.0 })
///     .build();
/// ```
pub struct EntityBuilder<'w> {
    world: &'w mut World,
    entity: Entity,
    components: Vec<Box<dyn ComponentInserter>>,
}

impl<'w> EntityBuilder<'w> {
    /// Add a component to the entity being built
    pub fn with<T>(mut self, component: T) -> Self
    where
        T: Component + TraitAccessible<dyn Component>,
    {
        self.components
            .push(Box::new(TypedComponentInserter { component }));
        self
    }

    /// Finish building and insert the entity into the world
    pub fn build(self) -> Entity {
        let entity = self.entity;
        let component_ids: Vec<ComponentId> =
            self.components.iter().map(|c| c.component_id()).collect();

        let components = self.components;
        self.world
            .insert_entity_with_components(entity, component_ids, |storage| {
                for inserter in components {
                    inserter.insert(storage);
                }
            });
        entity
    }
}

// --- Tests ---------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use trait_type_map::impl_trait_accessible;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Position {
        x: f32,
        y: f32,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Velocity {
        x: f32,
        y: f32,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Health {
        hp: i32,
    }

    impl Component for Position {}
    impl Component for Velocity {}
    impl Component for Health {}

    impl_trait_accessible!(dyn Component; Position, Velocity, Health);

    /// Tests creating multiple entities with different component combinations.
    ///
    /// This test verifies that:
    /// - Entities can be created with various combinations of components
    /// - Each unique component combination creates a separate archetype
    /// - All created entities are properly tracked in the world
    ///
    /// Expected results:
    /// - 3 entities should be created in total
    /// - 3 different archetypes should exist (Position+Velocity, Position, Position+Velocity+Health)
    /// - All entity IDs should be present in the entity_locations map
    #[test]
    fn test_create_entities_with_different_components() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world.register_component::<Health>();

        // Create entity with Position + Velocity
        let entity1 = world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        // Create entity with Position only
        let entity2 = world
            .create_entity()
            .with(Position { x: 5.0, y: 15.0 })
            .build();

        // Create entity with all three components
        let entity3 = world
            .create_entity()
            .with(Position { x: 100.0, y: 200.0 })
            .with(Velocity { x: 5.0, y: 10.0 })
            .with(Health { hp: 100 })
            .build();

        assert_eq!(world.entity_locations.len(), 3);
        assert_eq!(world.archetypes.len(), 3);
        assert!(world.entity_locations.contains_key(&entity1));
        assert!(world.entity_locations.contains_key(&entity2));
        assert!(world.entity_locations.contains_key(&entity3));

        // Print archetype information
        world.print_archetypes();

        // Verify each archetype's component mask matches expected components
        for (archetype_id, archetype) in world.archetypes.iter() {
            println!("\n--- Verifying Archetype {:?} ---", archetype_id);

            // Get component names
            let comp_names: Vec<String> = archetype
                .component_types
                .iter()
                .filter_map(|component_id| {
                    world
                        .component_registry
                        .get_name(component_id)
                        .map(String::from)
                })
                .collect();

            println!("Components: {:?}", comp_names);

            // Build expected mask from component types
            let mut expected_mask = ComponentMask::empty();
            for component_id in &archetype.component_types {
                if let Some(bit) = world.component_registry.get_bit(component_id) {
                    expected_mask.set(bit);
                    println!(
                        "  - {:?} -> bit {}",
                        world
                            .component_registry
                            .get_name(component_id)
                            .unwrap_or("Unknown"),
                        bit
                    );
                }
            }

            // Verify masks match
            assert_eq!(
                archetype.component_mask, expected_mask,
                "Archetype {:?} mask mismatch!\nActual:   {:?}\nExpected: {:?}",
                archetype_id, archetype.component_mask, expected_mask
            );

            println!("✓ Mask verified: {:?}", archetype.component_mask);
        }

        println!("\n✓ All 3 archetypes verified successfully!");
    }

    /// Tests adding a new component to an existing entity.
    ///
    /// This test verifies that:
    /// - A component can be added to an entity that doesn't already have it
    /// - The entity is migrated to a new archetype with the added component
    /// - Existing components on the entity are preserved during migration
    /// - The entity remains valid and tracked in the world
    /// - The old archetype is automatically cleaned up when it becomes empty
    ///
    /// Expected results:
    /// - add_component should return true (success)
    /// - The entity should still exist in entity_locations
    /// - Old archetype should be automatically removed, leaving 1 archetype
    #[test]
    fn test_add_component_to_entity() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world.register_component::<Health>();

        let entity = world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        assert_eq!(
            world.archetypes.len(),
            1,
            "Should have 1 archetype initially"
        );

        // Add Health component
        let result = world.add_component(entity, Health { hp: 50 });

        assert!(result.is_ok(), "Should successfully add component");
        assert!(world.entity_locations.contains_key(&entity));

        // Since this is the only entity, the old archetype should be automatically removed
        assert_eq!(
            world.archetypes.len(),
            1,
            "Should have 1 archetype after adding Health (old one auto-removed)"
        );

        world.print_archetypes();
    }

    /// Tests attempting to add a component to a non-existent entity.
    ///
    /// This test verifies that:
    /// - The system handles invalid entity IDs gracefully
    /// - No panic or crash occurs when operating on a fake entity
    /// - The operation correctly returns failure status
    ///
    /// Expected results:
    /// - add_component should return false (failure)
    /// - No side effects or modifications to the world state
    #[test]
    fn test_add_component_to_nonexistent_entity() {
        let mut world = World::new();
        world.register_component::<Position>();

        let fake_entity = Entity::new_for_test(9999, 0);
        let result = world.add_component(fake_entity, Position { x: 0.0, y: 0.0 });

        assert_eq!(
            result,
            Err(AddComponentError::EntityNotFound),
            "Should fail to add component to non-existent entity"
        );
    }

    /// Tests removing a component from an entity that has multiple components.
    ///
    /// This test verifies that:
    /// - A specific component can be removed from an entity
    /// - The entity is migrated to a new archetype without the removed component
    /// - Other components remain intact on the entity
    /// - The entity continues to exist in the world
    ///
    /// Expected results:
    /// - remove_component should return true (success)
    /// - The entity should still be tracked in entity_locations
    /// - The entity should be in a different archetype (Position+Health instead of Position+Velocity+Health)
    #[test]
    fn test_remove_component_from_entity() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world.register_component::<Health>();

        let entity = world
            .create_entity()
            .with(Position { x: 100.0, y: 200.0 })
            .with(Velocity { x: 5.0, y: 10.0 })
            .with(Health { hp: 100 })
            .build();

        // Remove Velocity component
        let result = world.remove_component::<Velocity>(entity);

        assert_eq!(world.archetypes.len(), 1, "Should have 1 archetype");

        assert!(result.is_ok(), "Should successfully remove component");
        assert!(world.entity_locations.contains_key(&entity));

        let location = world.entity_locations.get(&entity).unwrap();
        let archetype = world.archetypes.get(&location.archetype_id).unwrap();

        // Archetype should now only have Position and Health
        assert_eq!(
            archetype.component_types.len(),
            2,
            "Should have 2 component types"
        );

        // Archetype should contain Position and Health, but not Velocity. Checking component IDs.
        // Verify component IDs are as expected
        let position_id = ComponentId::of::<Position>();
        let health_id = ComponentId::of::<Health>();
        let velocity_id = ComponentId::of::<Velocity>();

        assert!(
            archetype.component_types.contains(&position_id),
            "Archetype should contain Position component"
        );
        assert!(
            archetype.component_types.contains(&health_id),
            "Archetype should contain Health component"
        );
        assert!(
            !archetype.component_types.contains(&velocity_id),
            "Archetype should not contain Velocity component"
        );
    }

    /// Tests attempting to remove a component from a non-existent entity.
    ///
    /// This test verifies that:
    /// - The system handles invalid entity IDs gracefully during removal
    /// - No panic occurs when trying to remove from a fake entity
    /// - The operation correctly reports failure
    ///
    /// Expected results:
    /// - remove_component should return false (failure)
    /// - No modifications to the world state
    #[test]
    fn test_remove_component_from_nonexistent_entity() {
        let mut world = World::new();
        world.register_component::<Velocity>();

        let fake_entity = Entity::new_for_test(9999, 0);
        let result = world.remove_component::<Velocity>(fake_entity);

        assert_eq!(
            result,
            Err(RemoveComponentError::EntityNotFound),
            "Should fail to remove component from non-existent entity"
        );
    }

    /// Tests removing the last component from an entity, which should destroy it.
    ///
    /// This test verifies that:
    /// - When an entity's last component is removed, the entity is automatically destroyed
    /// - No entities with zero components are left in the world
    /// - The entity is properly removed from all tracking structures
    /// - If entity count drops to zero, archetypes are cleaned up
    ///
    /// Expected results:
    /// - remove_component should return true (success)
    /// - The entity count should drop to 0
    /// - The entity should no longer exist in entity_locations
    /// - All archetypes should be removed if no entities remain
    #[test]
    fn test_remove_last_component_destroys_entity() {
        let mut world = World::new();
        world.register_component::<Position>();

        let entity = world
            .create_entity()
            .with(Position { x: 5.0, y: 15.0 })
            .build();

        assert_eq!(world.entity_locations.len(), 1);

        // Remove the only component - should destroy entity
        let result = world.remove_component::<Position>(entity);

        assert!(result.is_ok(), "Should successfully remove component");
        assert_eq!(
            world.entity_locations.len(),
            0,
            "Entity should be destroyed"
        );
        assert!(!world.entity_locations.contains_key(&entity));

        assert!(world.archetypes.is_empty(), "No archetypes should remain");
    }

    /// Tests destroying an entity and verifying other entities remain unaffected.
    ///
    /// This test verifies that:
    /// - An entity can be completely removed from the world
    /// - Destroying one entity doesn't affect other entities
    /// - The entity is removed from its archetype and all tracking structures
    /// - The total entity count decreases correctly
    ///
    /// Expected results:
    /// - destroy should return true (success)
    /// - Entity count should decrease from 2 to 1
    /// - The destroyed entity should no longer exist in entity_locations
    /// - The other entity should remain unaffected
    #[test]
    fn test_destroy_entity() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let entity1 = world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .build();

        let entity2 = world
            .create_entity()
            .with(Position { x: 5.0, y: 15.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        assert_eq!(world.entity_locations.len(), 2);

        // Destroy entity1
        let result = world.destroy_entity(entity1);

        assert!(result, "Should successfully destroy entity");
        assert_eq!(world.entity_locations.len(), 1);
        assert!(!world.entity_locations.contains_key(&entity1));
        assert!(world.entity_locations.contains_key(&entity2));
    }

    /// Tests attempting to destroy a non-existent entity.
    ///
    /// This test verifies that:
    /// - The system handles invalid entity IDs gracefully during destroy
    /// - No panic or crash occurs when destroying a fake entity
    /// - The operation correctly reports failure
    ///
    /// Expected results:
    /// - destroy should return false (failure)
    /// - No changes to the world state
    #[test]
    fn test_destroy_nonexistent_entity() {
        let mut world = World::new();
        let fake_entity = Entity::new_for_test(9999, 0);

        let result = world.destroy_entity(fake_entity);

        assert!(!result, "Should fail to destroy non-existent entity");
    }

    /// Tests that attempting to destroy an already-destroyed entity fails correctly.
    ///
    /// This test verifies that:
    /// - Once an entity is destroyed, it cannot be destroyed again
    /// - The system properly tracks which entities exist vs don't exist
    /// - Repeated destroy operations are safely rejected
    ///
    /// Expected results:
    /// - First destroy should return true (success)
    /// - Second destroy should return false (entity no longer exists)
    /// - No panic or invalid state from double-destroy attempt
    #[test]
    fn test_destroy_already_destroyed_entity() {
        let mut world = World::new();
        world.register_component::<Position>();

        let entity = world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .build();

        // First destroy should succeed
        let result1 = world.destroy_entity(entity);
        assert!(result1);

        // Second destroy should fail
        let result2 = world.destroy_entity(entity);
        assert!(!result2, "Should fail to destroy already-destroyed entity");
    }

    /// Tests the cleanup of empty archetypes after entities are destroyed.
    ///
    /// This test verifies that:
    /// - When all entities are removed from an archetype, it becomes empty
    /// - The cleanup_empty_archetypes method removes unused archetypes
    /// - Non-empty archetypes and their entities remain unaffected
    /// - Memory is properly reclaimed from empty archetype storage
    ///
    /// Expected results:
    /// - Initially 2 archetypes should exist
    /// - After destroying entity1 and cleanup, archetype count should decrease
    /// - entity2 should still exist and be properly tracked
    #[test]
    fn test_cleanup_empty_archetypes() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        // Create some entities
        let entity1 = world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .build();

        let entity2 = world
            .create_entity()
            .with(Position { x: 5.0, y: 15.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        let initial_archetypes = world.archetypes.len();
        assert_eq!(initial_archetypes, 2);

        // Destroy one entity, leaving one archetype empty
        world.destroy_entity(entity1);

        // Cleanup should remove empty archetype
        world.cleanup_empty_archetypes();

        assert!(world.archetypes.len() < initial_archetypes);
        assert!(world.entity_locations.contains_key(&entity2));
    }

    /// Tests entity migration between archetypes when components are added and removed.
    ///
    /// This test verifies that:
    /// - Adding a component moves the entity to a different archetype
    /// - Removing a component moves the entity to yet another archetype
    /// - Each archetype change is properly tracked with different archetype IDs
    /// - Component data is preserved during migrations
    ///
    /// Expected results:
    /// - Entity starts in archetype for (Position+Velocity)
    /// - After adding Health, entity moves to archetype for (Position+Velocity+Health)
    /// - After removing Velocity, entity moves to archetype for (Position+Health)
    /// - All three archetype IDs should be different from each other
    #[test]
    fn test_entity_archetype_migration() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world.register_component::<Health>();

        // Start with Position + Velocity
        let entity = world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        let initial_location = *world.entity_locations.get(&entity).unwrap();

        // Add Health - should migrate to new archetype
        world.add_component(entity, Health { hp: 100 }).unwrap();

        let after_add_location = *world.entity_locations.get(&entity).unwrap();
        assert_ne!(
            initial_location.archetype_id, after_add_location.archetype_id,
            "Entity should be in different archetype after adding component"
        );

        // Remove Velocity - should migrate to another archetype
        world.remove_component::<Velocity>(entity).unwrap();

        let after_remove_location = *world.entity_locations.get(&entity).unwrap();
        assert_ne!(
            after_add_location.archetype_id, after_remove_location.archetype_id,
            "Entity should be in different archetype after removing component"
        );
    }

    /// Tests that empty archetypes are automatically cleaned up when last entity moves.
    ///
    /// This test verifies that:
    /// - When the last entity in an archetype is moved to another archetype, the empty one is removed
    /// - The archetype is removed from both the archetypes map and the lookup table
    /// - No manual cleanup_empty_archetypes() call is needed
    /// - The world remains in a consistent state
    ///
    /// Expected results:
    /// - Initially 1 archetype exists (Position+Velocity)
    /// - After adding Health, 2 archetypes exist temporarily
    /// - The old archetype is automatically removed, leaving only 1 archetype
    /// - The entity is correctly tracked in the new archetype
    #[test]
    fn test_automatic_empty_archetype_cleanup() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world.register_component::<Health>();

        // Create single entity with Position + Velocity
        let entity = world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        assert_eq!(
            world.archetypes.len(),
            1,
            "Should have 1 archetype initially"
        );

        // Add Health - this should move entity to new archetype
        // The old archetype should be automatically removed since it becomes empty
        world.add_component(entity, Health { hp: 100 }).unwrap();

        assert_eq!(
            world.archetypes.len(),
            1,
            "Should still have 1 archetype after migration (old one auto-removed)"
        );
        assert!(world.entity_locations.contains_key(&entity));

        // Verify the entity is in the correct archetype with all 3 components
        let location = world.entity_locations.get(&entity).unwrap();
        let archetype = world.archetypes.get(&location.archetype_id).unwrap();
        assert_eq!(
            archetype.component_types.len(),
            3,
            "Entity should have 3 components"
        );

        println!("✓ Empty archetype automatically cleaned up after entity migration");
    }

    /// Test that archetype print_info displays component names and entity count
    #[test]
    fn test_archetype_print_info() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        // Create some entities
        world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        world
            .create_entity()
            .with(Position { x: 5.0, y: 15.0 })
            .with(Velocity { x: 0.5, y: 1.5 })
            .build();

        // Print info using the world helper method
        world.print_archetypes();

        // Verify archetype structure
        assert_eq!(world.entity_locations.len(), 2, "Should have 2 entities");
        assert_eq!(world.archetypes.len(), 1, "Should have 1 archetype");

        // Get the archetype and verify its contents
        let archetype = world.archetypes.values().next().unwrap();
        assert_eq!(
            archetype.entities.len(),
            2,
            "Archetype should contain 2 entities"
        );
        assert_eq!(
            archetype.component_types.len(),
            2,
            "Archetype should have 2 component types"
        );

        // Verify component names are registered and retrievable
        let comp_names: Vec<String> = archetype
            .component_types
            .iter()
            .filter_map(|component_id| {
                world
                    .component_registry
                    .get_name(component_id)
                    .map(String::from)
            })
            .collect();

        assert_eq!(comp_names.len(), 2, "Should have 2 component names");

        // Check that both expected component names are present
        let has_position = comp_names.iter().any(|name| name.contains("Position"));
        let has_velocity = comp_names.iter().any(|name| name.contains("Velocity"));

        assert!(
            has_position,
            "Should contain Position component, found: {:?}",
            comp_names
        );
        assert!(
            has_velocity,
            "Should contain Velocity component, found: {:?}",
            comp_names
        );

        println!("✓ Component names verified: {:?}", comp_names);
    }

    /// Tests entity generation system for safe ID recycling.
    ///
    /// This test verifies that:
    /// - Entity IDs are recycled after destruction
    /// - Generations are incremented when IDs are reused
    /// - Stale handles (old generation) cannot access recycled entities
    /// - New entities with recycled IDs work correctly
    ///
    /// Expected results:
    /// - Destroyed entity's ID should be reused for new entity
    /// - New entity should have same ID but different generation
    /// - Old handle should be invalid (is_entity_valid returns false)
    /// - Old handle should not access new entity's components
    #[test]
    fn test_entity_generations() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        // Create first entity
        let entity1 = world
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .build();

        println!("Entity1: id={}, gen={}", entity1.id, entity1.generation);
        assert_eq!(entity1.id, 0, "First entity should have ID 0");
        assert_eq!(
            entity1.generation, 0,
            "First entity should have generation 0"
        );

        // Verify entity1 exists and has component
        assert!(world.is_entity_valid(entity1), "Entity1 should be valid");
        assert!(
            world.get_component::<Position>(entity1).is_some(),
            "Entity1 should have Position"
        );

        // Destroy entity1
        let destroyed = world.destroy_entity(entity1);
        assert!(destroyed, "Entity1 should be destroyed successfully");

        // Verify entity1 is no longer valid
        assert!(
            !world.is_entity_valid(entity1),
            "Entity1 should be invalid after destruction"
        );
        assert!(
            world.get_component::<Position>(entity1).is_none(),
            "Destroyed entity should not have components"
        );

        // Create a new entity - should reuse ID 0 with generation 1
        let entity2 = world
            .create_entity()
            .with(Velocity { x: 5.0, y: 10.0 })
            .build();

        println!("Entity2: id={}, gen={}", entity2.id, entity2.generation);
        assert_eq!(entity2.id, 0, "New entity should reuse ID 0");
        assert_eq!(entity2.generation, 1, "New entity should have generation 1");

        // Verify entity2 is valid
        assert!(world.is_entity_valid(entity2), "Entity2 should be valid");
        assert!(
            world.get_component::<Velocity>(entity2).is_some(),
            "Entity2 should have Velocity"
        );

        // Critical: Old handle (entity1) should NOT access entity2's data
        assert!(
            !world.is_entity_valid(entity1),
            "Old handle should still be invalid"
        );
        assert!(
            world.get_component::<Velocity>(entity1).is_none(),
            "Old handle should not access new entity's components"
        );
        assert!(
            world.get_component::<Position>(entity1).is_none(),
            "Old handle should not access any components"
        );

        // Verify they are different entities (different hash/eq)
        assert_ne!(
            entity1, entity2,
            "Entities with different generations should not be equal"
        );

        println!("✓ Entity generation recycling works correctly!");
    }

    /// Tests multiple rounds of entity recycling.
    ///
    /// This test verifies that:
    /// - Multiple destroy/create cycles correctly increment generations
    /// - The free list works correctly with multiple recycled IDs
    /// - Generations wrap around safely (using wrapping_add)
    #[test]
    fn test_multiple_entity_recycling_rounds() {
        let mut world = World::new();
        world.register_component::<Position>();

        // Create and destroy the same ID multiple times
        let mut last_entity = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();
        assert_eq!(last_entity.id, 0);
        assert_eq!(last_entity.generation, 0);

        for round in 1..=5 {
            let old_entity = last_entity;
            world.destroy_entity(old_entity);

            let new_entity = world
                .create_entity()
                .with(Position {
                    x: round as f32,
                    y: 0.0,
                })
                .build();

            assert_eq!(new_entity.id, 0, "Should reuse ID 0 in round {}", round);
            assert_eq!(
                new_entity.generation, round,
                "Generation should be {} in round {}",
                round, round
            );

            // Old handle should be invalid
            assert!(!world.is_entity_valid(old_entity));
            // New handle should be valid
            assert!(world.is_entity_valid(new_entity));

            last_entity = new_entity;
        }

        println!("✓ Multiple recycling rounds work correctly!");
    }

    /// Tests that multiple entities can be recycled independently.
    ///
    /// This test verifies LIFO (stack) behavior of the free list.
    #[test]
    fn test_free_list_lifo_order() {
        let mut world = World::new();
        world.register_component::<Position>();

        // Create 3 entities
        let entity0 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();
        let entity1 = world
            .create_entity()
            .with(Position { x: 1.0, y: 1.0 })
            .build();
        let entity2 = world
            .create_entity()
            .with(Position { x: 2.0, y: 2.0 })
            .build();

        assert_eq!(entity0.id, 0);
        assert_eq!(entity1.id, 1);
        assert_eq!(entity2.id, 2);

        // Destroy in order: entity0, entity1, entity2
        world.destroy_entity(entity0);
        world.destroy_entity(entity1);
        world.destroy_entity(entity2);

        // Free list should be: [(0, 1), (1, 1), (2, 1)]
        // Pop order (LIFO): entity2's ID first, then entity1's, then entity0's

        let new_entity1 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();
        assert_eq!(new_entity1.id, 2, "Should pop ID 2 first (LIFO)");
        assert_eq!(new_entity1.generation, 1);

        let new_entity2 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();
        assert_eq!(new_entity2.id, 1, "Should pop ID 1 second");
        assert_eq!(new_entity2.generation, 1);

        let new_entity3 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();
        assert_eq!(new_entity3.id, 0, "Should pop ID 0 third");
        assert_eq!(new_entity3.generation, 1);

        // Next entity should get a fresh ID
        let new_entity4 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();
        assert_eq!(new_entity4.id, 3, "Should allocate fresh ID 3");
        assert_eq!(new_entity4.generation, 0);

        println!("✓ Free list LIFO order works correctly!");
    }

    /// Tests entity generations with multiple archetypes and component removal.
    ///
    /// This test verifies that:
    /// - Entities in different archetypes have independent generation tracking
    /// - Removing components (which moves entity to new archetype) preserves entity identity
    /// - Destroying entities from different archetypes correctly adds IDs to free list
    /// - Recycled IDs work correctly regardless of which archetype the original was in
    #[test]
    fn test_generations_with_multiple_archetypes_and_component_removal() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world.register_component::<Health>();

        // Create 3 entities:
        // entity1, entity2: Position + Velocity (same archetype)
        // entity3: Position + Health (different archetype)
        let entity1 = world
            .create_entity()
            .with(Position { x: 1.0, y: 1.0 })
            .with(Velocity { x: 10.0, y: 10.0 })
            .build();

        let entity2 = world
            .create_entity()
            .with(Position { x: 2.0, y: 2.0 })
            .with(Velocity { x: 20.0, y: 20.0 })
            .build();

        let entity3 = world
            .create_entity()
            .with(Position { x: 3.0, y: 3.0 })
            .with(Health { hp: 100 })
            .build();

        println!(
            "Created: entity1(id={}, gen={}), entity2(id={}, gen={}), entity3(id={}, gen={})",
            entity1.id,
            entity1.generation,
            entity2.id,
            entity2.generation,
            entity3.id,
            entity3.generation
        );

        assert_eq!(entity1.id, 0);
        assert_eq!(entity2.id, 1);
        assert_eq!(entity3.id, 2);
        assert_eq!(world.archetypes.len(), 2, "Should have 2 archetypes");

        // Remove Velocity from entity1 - moves it to Position-only archetype
        let old_entity1 = entity1;
        let removed = world.remove_component::<Velocity>(entity1);
        assert!(removed.is_ok(), "Should remove Velocity from entity1");

        // entity1 should still be valid with same id and generation (entity wasn't destroyed)
        assert!(
            world.is_entity_valid(entity1),
            "entity1 should still be valid after component removal"
        );
        assert!(
            world.get_component::<Position>(entity1).is_some(),
            "entity1 should still have Position"
        );
        assert!(
            world.get_component::<Velocity>(entity1).is_none(),
            "entity1 should not have Velocity"
        );

        // Destroy entity2 (from Position+Velocity archetype)
        let old_entity2 = entity2;
        world.destroy_entity(entity2);
        assert!(
            !world.is_entity_valid(old_entity2),
            "entity2 should be invalid after destruction"
        );

        // Destroy entity3 (from Position+Health archetype)
        let old_entity3 = entity3;
        world.destroy_entity(entity3);
        assert!(
            !world.is_entity_valid(old_entity3),
            "entity3 should be invalid after destruction"
        );

        // Free list should now have: [(1, 1), (2, 1)] (LIFO order)
        // entity1 (id=0) is still alive

        // Create new entity - should reuse ID 2 (last destroyed)
        let new_entity1 = world.create_entity().with(Health { hp: 50 }).build();

        println!(
            "new_entity1: id={}, gen={}",
            new_entity1.id, new_entity1.generation
        );
        assert_eq!(new_entity1.id, 2, "Should reuse ID 2 (LIFO)");
        assert_eq!(new_entity1.generation, 1, "Should have generation 1");

        // Old entity3 handle should NOT access new_entity1's data
        assert!(
            !world.is_entity_valid(old_entity3),
            "Old entity3 handle should be invalid"
        );
        assert!(
            world.get_component::<Health>(old_entity3).is_none(),
            "Old handle should not access new entity"
        );

        // Create another entity - should reuse ID 1
        let new_entity2 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .with(Velocity { x: 0.0, y: 0.0 })
            .build();

        println!(
            "new_entity2: id={}, gen={}",
            new_entity2.id, new_entity2.generation
        );
        assert_eq!(new_entity2.id, 1, "Should reuse ID 1");
        assert_eq!(new_entity2.generation, 1, "Should have generation 1");

        // Old entity2 handle should NOT access new_entity2's data
        assert!(
            !world.is_entity_valid(old_entity2),
            "Old entity2 handle should be invalid"
        );

        // Verify entity1 (never destroyed) still works with original handle
        assert!(
            world.is_entity_valid(old_entity1),
            "Original entity1 should still be valid"
        );
        let pos = world.get_component::<Position>(old_entity1).unwrap();
        assert_eq!(pos.x, 1.0, "entity1 Position should be preserved");

        // Destroy entity1 and verify recycling
        world.destroy_entity(entity1);
        assert!(
            !world.is_entity_valid(old_entity1),
            "entity1 should be invalid after destruction"
        );

        let new_entity3 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();
        println!(
            "new_entity3: id={}, gen={}",
            new_entity3.id, new_entity3.generation
        );
        assert_eq!(new_entity3.id, 0, "Should reuse ID 0");
        assert_eq!(new_entity3.generation, 1, "Should have generation 1");

        println!("✓ Generations with multiple archetypes and component removal work correctly!");
    }

    /// Tests that component data is correctly swap-removed when an entity is destroyed.
    ///
    /// This test exposes the bug where component data is NOT swap-removed from storage
    /// when an entity is destroyed, causing remaining entities to read stale/wrong data.
    ///
    /// Expected behavior
    /// - After destroying entity0, entity2 should still have its original Position (2.0, 2.0)
    /// - Currently, entity2 reads entity0's old Position (0.0, 0.0) - BUG!
    #[test]
    fn test_component_swap_remove_on_destroy() {
        let mut world = World::new();
        world.register_component::<Position>();

        // Create 3 entities in the same archetype
        let entity0 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();
        let entity1 = world
            .create_entity()
            .with(Position { x: 1.0, y: 1.0 })
            .build();
        let entity2 = world
            .create_entity()
            .with(Position { x: 2.0, y: 2.0 })
            .build();

        // Verify initial state
        assert_eq!(world.get_component::<Position>(entity0).unwrap().x, 0.0);
        assert_eq!(world.get_component::<Position>(entity1).unwrap().x, 1.0);
        assert_eq!(world.get_component::<Position>(entity2).unwrap().x, 2.0);

        // Archetype entity list: [entity0, entity1, entity2] (indices 0, 1, 2)
        // Component storage:     [Pos(0,0), Pos(1,1), Pos(2,2)]

        // Destroy entity0 (index 0)
        // Entity list swap_remove: entity2 moves from index 2 to index 0
        // Entity list becomes: [entity2, entity1] (entity2 now at index 0)
        //
        // BUG: Component storage is NOT updated!
        // Component storage still: [Pos(0,0), Pos(1,1), Pos(2,2)]
        //
        // Now entity2 has index 0, but component at index 0 is Pos(0,0) - WRONG!
        world.destroy_entity(entity0);

        // entity1 should still have its original position (index 1 unchanged)
        let pos1 = world.get_component::<Position>(entity1).unwrap();
        assert_eq!(pos1.x, 1.0, "entity1 Position.x should be 1.0");
        assert_eq!(pos1.y, 1.0, "entity1 Position.y should be 1.0");

        // entity2 was swapped to index 0 - it should still have Position(2.0, 2.0)
        // BUG: It actually reads Position(0.0, 0.0) because component storage wasn't swap-removed
        let pos2 = world.get_component::<Position>(entity2).unwrap();

        println!(
            "entity2 Position after entity0 destroyed: ({}, {})",
            pos2.x, pos2.y
        );
        println!("Expected: (2.0, 2.0), Got: ({}, {})", pos2.x, pos2.y);

        // This assertion FAILS because of the unimplemented component swap_remove
        assert_eq!(
            pos2.x, 2.0,
            "BUG: entity2 should have Position.x = 2.0, but got {} (entity0's old data)",
            pos2.x
        );
        assert_eq!(
            pos2.y, 2.0,
            "BUG: entity2 should have Position.y = 2.0, but got {} (entity0's old data)",
            pos2.y
        );

        println!("✓ Component swap_remove works correctly!");
    }

    /// Tests that component data is properly cleaned up when an entity migrates between archetypes.
    ///
    /// When an entity gains or loses a component, it moves to a different archetype.
    /// The old archetype must properly remove the entity's component data using swap_remove.
    /// Otherwise:
    /// 1. Memory leaks occur (orphaned component data)
    /// 2. Other entities in the old archetype may read wrong component data
    ///
    /// This test verifies:
    /// - Component data is removed from old archetype during migration
    /// - Other entities in the old archetype still have correct component data
    /// - The swapped entity (if any) correctly maps to its swapped component data
    #[test]
    fn test_component_cleanup_on_archetype_migration() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world.register_component::<Health>();

        // Create 3 entities with Position + Velocity in the same archetype
        let entity0 = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .with(Velocity { x: 100.0, y: 100.0 })
            .build();
        let entity1 = world
            .create_entity()
            .with(Position { x: 1.0, y: 1.0 })
            .with(Velocity { x: 101.0, y: 101.0 })
            .build();
        let entity2 = world
            .create_entity()
            .with(Position { x: 2.0, y: 2.0 })
            .with(Velocity { x: 102.0, y: 102.0 })
            .build();

        // Verify initial state
        assert_eq!(
            world.archetypes.len(),
            1,
            "Should have 1 archetype initially"
        );

        // Verify all entities have correct data
        assert_eq!(world.get_component::<Position>(entity0).unwrap().x, 0.0);
        assert_eq!(world.get_component::<Velocity>(entity0).unwrap().x, 100.0);
        assert_eq!(world.get_component::<Position>(entity1).unwrap().x, 1.0);
        assert_eq!(world.get_component::<Velocity>(entity1).unwrap().x, 101.0);
        assert_eq!(world.get_component::<Position>(entity2).unwrap().x, 2.0);
        assert_eq!(world.get_component::<Velocity>(entity2).unwrap().x, 102.0);

        // Now add Health to entity0 - this moves it to a NEW archetype (Position+Velocity+Health)
        // The old archetype (Position+Velocity) should swap_remove entity0's data
        // entity2 should be swapped into index 0
        world.add_component(entity0, Health { hp: 50 }).unwrap();

        // Verify entity0 moved to new archetype and has all components
        assert!(world.get_component::<Position>(entity0).is_some());
        assert!(world.get_component::<Velocity>(entity0).is_some());
        assert!(world.get_component::<Health>(entity0).is_some());
        assert_eq!(world.get_component::<Position>(entity0).unwrap().x, 0.0);
        assert_eq!(world.get_component::<Velocity>(entity0).unwrap().x, 100.0);
        assert_eq!(world.get_component::<Health>(entity0).unwrap().hp, 50);

        // CRITICAL: entity1 and entity2 should still have correct data in old archetype
        // If swap_remove wasn't applied to component storage, entity2 (now at index 0)
        // would incorrectly read entity0's old data!

        let pos1 = world.get_component::<Position>(entity1).unwrap();
        let vel1 = world.get_component::<Velocity>(entity1).unwrap();
        assert_eq!(pos1.x, 1.0, "entity1 Position.x should be 1.0");
        assert_eq!(vel1.x, 101.0, "entity1 Velocity.x should be 101.0");

        let pos2 = world.get_component::<Position>(entity2).unwrap();
        let vel2 = world.get_component::<Velocity>(entity2).unwrap();
        assert_eq!(
            pos2.x, 2.0,
            "entity2 Position.x should be 2.0, but got {} (possible swap_remove bug)",
            pos2.x
        );
        assert_eq!(
            vel2.x, 102.0,
            "entity2 Velocity.x should be 102.0, but got {} (possible swap_remove bug)",
            vel2.x
        );

        // Verify archetype count (old one should still exist with entity1, entity2)
        assert_eq!(world.archetypes.len(), 2, "Should have 2 archetypes now");

        // Now remove Velocity from entity1 - moves to Position-only archetype
        world.remove_component::<Velocity>(entity1).unwrap();

        // entity2 should still have correct data (it's now alone in Position+Velocity archetype)
        let pos2 = world.get_component::<Position>(entity2).unwrap();
        let vel2 = world.get_component::<Velocity>(entity2).unwrap();
        assert_eq!(pos2.x, 2.0, "entity2 Position.x should still be 2.0");
        assert_eq!(vel2.x, 102.0, "entity2 Velocity.x should still be 102.0");

        // entity1 should only have Position now
        assert!(world.get_component::<Position>(entity1).is_some());
        assert!(world.get_component::<Velocity>(entity1).is_none());
        assert_eq!(world.get_component::<Position>(entity1).unwrap().x, 1.0);

        println!("✓ Component cleanup on archetype migration works correctly!");
    }
}
