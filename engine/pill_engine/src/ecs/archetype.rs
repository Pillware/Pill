// ============================================================================
// Archetype Storage System
// ============================================================================
//! Archetype-based component storage.
//!
//! An archetype is a unique combination of component types. All entities with
//! the same set of components are stored together in the same archetype for
//! cache-friendly iteration.
//!
//! ## Storage Layout
//!
//! Archetypes use a Structure of Arrays (SoA) layout rather than an
//! Array of Structures (AoS). This means components of the same type are
//! stored contiguously in memory:
//!
//! ```text
//! Archetype [Position, Velocity]
//! ┌─────────────────────────────────────────────────┐
//! │ Entities:    [E1,     E2,     E3,     E4    ]   │
//! │ Positions:   [Pos1,   Pos2,   Pos3,   Pos4  ]   │
//! │ Velocities:  [Vel1,   Vel2,   Vel3,   Vel4  ]   │
//! └─────────────────────────────────────────────────┘
//! ```
//!
//! ## Cache Efficiency
//!
//! When iterating over all entities with Position+Velocity:
//! - SoA (this design): Sequential memory access, excellent cache utilization
//! - AoS alternative: Scattered access, poor cache performance
//!
//! The tradeoff is that accessing all components of a single entity requires
//! multiple array lookups, but this is rare compared to bulk iteration
//! in ECS-style approaches.
//!
//! ## Entity Removal
//!
//! When an entity is removed, we use swap-remove to maintain dense storage:
//! 1. Swap the removed entity with the last entity in each component array
//! 2. Pop the last element (now the removed entity's data)
//! 3. Update the swapped entity's location in the entity_locations map
//!
//! This keeps arrays dense without gaps, maintaining O(1) removal.

use std::collections::HashMap;

use trait_type_map::{TraitTypeMap, VecFamily};

use crate::ecs::component::{Component, ComponentId, ComponentMask};
use crate::ecs::entity::Entity;

/// Type for component storage factory functions
/// These create empty storage for a specific component type
pub type StorageFactory = Box<dyn Fn(&mut TraitTypeMap<dyn Component, VecFamily>) + Send + Sync>;

/// ArchetypeId uniquely identifies an archetype (a unique combination of components)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArchetypeId(pub usize);

pub struct Archetype {
    pub id: ArchetypeId,
    pub component_types: Vec<ComponentId>, // Still needed for iteration/lookup
    pub component_mask: ComponentMask,     // Fast bitmask for query matching
    pub component_storages: TraitTypeMap<dyn Component, VecFamily>,
    pub entities: Vec<Entity>,
}

impl Archetype {
    /// Create a new archetype with storage for the specified component types
    ///
    /// The storage_factories map provides a way to create storage for each component type
    /// by ComponentId (TypeId). This allows archetype creation without knowing the concrete types.
    pub fn new(
        id: ArchetypeId,
        component_types: Vec<ComponentId>,
        component_mask: ComponentMask,
        storage_factories: &HashMap<ComponentId, StorageFactory>,
    ) -> Self {
        let mut component_storages = TraitTypeMap::new();

        // Register storage for each component type using the factory
        for &component_id in &component_types {
            let factory = storage_factories.get(&component_id)
                .unwrap_or_else(|| panic!(
                    "Component type {:?} not registered. Call world.register_component::<T>() first.",
                    component_id
                ));
            factory(&mut component_storages);
        }

        Self {
            id,
            component_types,
            component_mask,
            component_storages,
            entities: Vec::new(),
        }
    }

    /// Check if this archetype contains entities with the specified component
    ///
    /// Uses bitmask for O(1) lookup instead of linear search through component types.
    #[inline]
    pub fn has_component_bit(&self, bit: u8) -> bool {
        self.component_mask.has_bit(bit)
    }

    /// Check if this archetype contains entities with the specified component type
    ///
    /// Note: This uses O(n) linear search. Prefer `has_component_bit` with a
    /// pre-looked-up bit index for hot paths.
    pub fn has_component<T: Component>(&self) -> bool {
        self.component_types.contains(&ComponentId::of::<T>())
    }

    /// Check if this archetype matches the required component mask for a query
    pub fn matches_mask(&self, required_mask: &ComponentMask) -> bool {
        self.component_mask.contains_all(required_mask)
    }

    /// Get the number of entities in this archetype
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Get the number of entities in this archetype (alias for `len`)
    ///
    /// Provided for API consistency with other collection types.
    #[inline]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Check if this archetype contains no entities
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Print information about this archetype (component names and entity count)
    pub fn print_info(&self, registry: &crate::ecs::component::ComponentRegistry) {
        let component_names: Vec<String> = self
            .component_types
            .iter()
            .map(|component_id| {
                registry
                    .get_name(component_id)
                    .unwrap_or("Unknown")
                    .to_string()
            })
            .collect();

        println!(
            "Archetype {:?}: {} entities, components: [{}]",
            self.id,
            self.entities.len(),
            component_names.join(", ")
        );
    }
}
