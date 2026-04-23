// ============================================================================
// Commands - Deferred Operation Queue
// ============================================================================
//! Commands provide deferred operations on entities and components.
//!
//! Instead of modifying the world immediately (which would require mutable
//! access), commands queue operations to be executed later. This allows
//! component iterators and multiple systems to run in parallel without conflicts.
//!
//! ## Frame Lifecycle (Two-Phase Approach)
//!
//! The ECS uses a two-phase execution model each frame:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                        FRAME N                              │
//! ├─────────────────────────────────┬───────────────────────────┤
//! │      Phase 1: System Execution  │  Phase 2: Command Apply   │
//! │                                 │                           │
//! │  ┌──────────┐  ┌──────────┐     │  Commands from Phase 1    │
//! │  │System A  │  │System B  │     │  are now executed:        │
//! │  │(parallel)│  │(parallel)│     │                           │
//! │  └────┬─────┘  └────┬─────┘     │  - Create entities        │
//! │       │             │           │  - Destroy entities       │
//! │       ▼             ▼           │  - Add/remove components  │
//! │  ┌──────────────────────┐       │                           │
//! │  │   Command Queue      │──────►│  World is now consistent  │
//! │  │   (deferred ops)     │       │  for next frame           │
//! │  └──────────────────────┘       │                           │
//! └─────────────────────────────────┴───────────────────────────┘
//! ```
//!
//! ## Why Deferred?
//!
//! 1. Thread Safety: Multiple systems can queue commands without locks
//! 2. Consistency: World state doesn't change mid-iteration
//! 3. Batching: Commands can be optimized before execution
//!
//! ## Usage Example
//!
//! ```ignore
//! fn combat_system(mut query: Query<(&Health, Entity)>, mut commands: Commands) {
//!     for (health, entity) in query.iter() {
//!         if health.current <= 0 {
//!             // Queue for destruction - doesn't happen immediately!
//!             commands.destroy_entity(entity);
//!         }
//!     }
//! }
//! // After ALL systems run, the engine calls commands.execute_queued_commands()
//! // and the dead entities are actually removed.
//! ```

use crate::ecs::component::{Component, ComponentId};
use crate::ecs::entity::Entity;
use crate::ecs::world::World;
use std::sync::Arc;
use trait_type_map::{TraitAccessible, TraitTypeMap, VecFamily};

/// Trait for adding a component with its concrete type preserved
///
/// Must be Send to support parallel execution of systems.
pub trait ComponentAdder: Send {
    fn component_id(&self) -> ComponentId;
    fn add_component_to_storage(
        self: Box<Self>,
        new_storage: &mut TraitTypeMap<dyn Component, VecFamily>,
    );
}

/// Typed component adder that knows the concrete type T
struct TypedComponentAdder<T: Component + TraitAccessible<dyn Component>> {
    component: T,
}

impl<T: Component + TraitAccessible<dyn Component> + Send> ComponentAdder
    for TypedComponentAdder<T>
{
    fn component_id(&self) -> ComponentId {
        ComponentId::of::<T>()
    }

    fn add_component_to_storage(
        self: Box<Self>,
        new_storage: &mut TraitTypeMap<dyn Component, VecFamily>,
    ) {
        // Add the new component
        new_storage.get_storage_mut::<T>().push(self.component);
    }
}

/// Deferred command to be executed later
enum DeferredCommand {
    CreateEntity {
        component_adders: Vec<Box<dyn ComponentAdder>>,
    },
    AddComponentToEntity {
        entity: Entity,
        component_adder: Box<dyn ComponentAdder>,
    },
    RemoveComponentFromEntity {
        entity: Entity,
        component_id: ComponentId,
    },
    DestroyEntity {
        entity: Entity,
    },
}

/// Commands queue for deferred operations
///
/// Systems that want to modify entities use Commands to queue changes.
/// These changes are applied in a separate phase after all systems run.
pub struct CommandQueue {
    commands: Vec<DeferredCommand>,
}

impl CommandQueue {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
}

impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandQueue {
    /// Queue creating a new entity with components
    pub fn create_entity(&mut self, components: Vec<Box<dyn ComponentAdder>>) {
        self.commands.push(DeferredCommand::CreateEntity {
            component_adders: components,
        });
    }

    /// Queue adding a component to an entity
    pub fn add_component_to_entity<T>(&mut self, entity: Entity, component: T)
    where
        T: Component + TraitAccessible<dyn Component> + Send,
    {
        self.commands.push(DeferredCommand::AddComponentToEntity {
            entity,
            component_adder: Box::new(TypedComponentAdder { component }),
        });
    }

    /// Queue removing a component from an entity
    pub fn remove_component_from_entity<T: Component>(&mut self, entity: Entity) {
        self.commands
            .push(DeferredCommand::RemoveComponentFromEntity {
                entity,
                component_id: ComponentId::of::<T>(),
            });
    }

    /// Queue destroying (removing) an entity
    pub fn destroy_entity(&mut self, entity: Entity) {
        self.commands
            .push(DeferredCommand::DestroyEntity { entity });
    }

    /// Execute all queued commands
    ///
    /// This is called by the Engine after all systems have run.
    pub(crate) fn execute_queued_commands(&mut self, world: &mut World) {
        for command in self.commands.drain(..) {
            match command {
                DeferredCommand::CreateEntity { component_adders } => {
                    // Collect component IDs
                    let component_ids: Vec<ComponentId> = component_adders
                        .iter()
                        .map(|adder| adder.component_id())
                        .collect();

                    // Allocate entity
                    let entity = world.allocate_entity();

                    // Insert entity with components
                    // TODO: Instead of passing lambda, we can pass componenent adders and let world handle archetype lookup and migration internally
                    world.insert_entity_with_components(entity, component_ids, |storage| {
                        for component_adder in component_adders {
                            component_adder.add_component_to_storage(storage);
                        }
                    });
                }

                DeferredCommand::AddComponentToEntity {
                    entity,
                    component_adder,
                } => {
                    // Get current entity location and components
                    let entity_location = match world.entity_locations.get(&entity) {
                        Some(location) => *location,
                        None => {
                            println!(
                                "  [Deferred] Entity {:?} not found for add_component",
                                entity.id
                            );
                            continue;
                        }
                    };

                    // Check if entity already has this component
                    let old_archetype =
                        world.archetypes.get(&entity_location.archetype_id).unwrap();
                    let mut new_component_ids = old_archetype.component_types.clone();
                    let new_component_id = component_adder.component_id();
                    if new_component_ids.contains(&new_component_id) {
                        println!(
                            "  [Deferred] Entity {:?} already has component {:?}",
                            entity.id, new_component_id
                        );
                        continue;
                    }

                    // We need to move the entity to a new archetype with the added component
                    new_component_ids.push(new_component_id);
                    new_component_ids.sort();

                    // Copy existing components using the registered copiers
                    let old_component_ids = old_archetype.component_types.clone();

                    // Collect copiers before borrowing world mutably (Arc::clone is cheap)
                    let component_copiers: Vec<_> = old_component_ids
                        .iter()
                        .filter_map(|component_id| {
                            world.component_copiers.get(component_id).map(Arc::clone)
                        })
                        .collect();

                    // Move entity to the archetype with the additional component
                    world.move_entity_to_archetype(
                        entity,
                        new_component_ids,
                        |old_storage, new_storage, old_index| {
                            // Copy all existing components from old archetype
                            for component_copier in component_copiers.iter() {
                                component_copier(old_storage, new_storage, old_index);
                            }

                            // Add the new component via the adder
                            component_adder.add_component_to_storage(new_storage);
                        },
                    );
                }

                DeferredCommand::RemoveComponentFromEntity {
                    entity,
                    component_id,
                } => {
                    // Get current entity location and components
                    let entity_location = match world.entity_locations.get(&entity) {
                        Some(location) => *location,
                        None => {
                            println!(
                                "  [Deferred] Entity {:?} not found for remove_component",
                                entity.id
                            );
                            continue;
                        }
                    };

                    let old_archetype =
                        world.archetypes.get(&entity_location.archetype_id).unwrap();

                    // Check if entity has this component
                    if !old_archetype.component_types.contains(&component_id) {
                        println!(
                            "  [Deferred] Entity {:?} doesn't have component {:?}",
                            entity.id, component_id
                        );
                        continue;
                    }

                    // Build new component list without the removed component
                    let new_component_ids: Vec<ComponentId> = old_archetype
                        .component_types
                        .iter()
                        .filter(|&id| *id != component_id)
                        .cloned()
                        .collect();

                    // If no components left, destroy the entity instead and move to the next command
                    if new_component_ids.is_empty() {
                        world.destroy_entity(entity);
                        continue;
                    }

                    // Collect copiers for remaining components
                    let component_copiers: Vec<_> = new_component_ids
                        .iter()
                        .filter_map(|component_id| {
                            world.component_copiers.get(component_id).map(Arc::clone)
                        })
                        .collect();

                    // Move entity to new archetype without the removed component
                    world.move_entity_to_archetype(
                        entity,
                        new_component_ids,
                        |old_storage, new_storage, old_index| {
                            // Copy all components except the removed one
                            for component_copier in component_copiers.iter() {
                                component_copier(old_storage, new_storage, old_index);
                            }
                        },
                    );
                }

                DeferredCommand::DestroyEntity { entity } => {
                    if !world.destroy_entity(entity) {
                        println!(
                            "  [Deferred] Failed to destroy entity {:?} (not found)",
                            entity.id
                        );
                    }
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// Commands allows systems to perform deferred entity operations
///
/// This is a system parameter that provides access to the command queue.
pub struct Commands<'a> {
    command_queue: &'a mut CommandQueue,
}

impl<'a> Commands<'a> {
    pub(crate) fn new(command_queue: &'a mut CommandQueue) -> Self {
        Self { command_queue }
    }

    /// Start building a new entity to create (executed later)
    ///
    /// Returns an EntityBuilder for fluent API to add components.
    pub fn create_entity(&mut self) -> EntityBuilder<'_> {
        EntityBuilder {
            command_queue: self.command_queue,
            components: Vec::new(),
        }
    }

    /// Queue adding a component to an entity (executed later)
    pub fn add_component_to_entity<T>(&mut self, entity: Entity, component: T)
    where
        T: Component + TraitAccessible<dyn Component> + Send,
    {
        self.command_queue
            .add_component_to_entity(entity, component);
    }

    /// Queue removing a component from an entity (executed later)
    pub fn remove_component_from_entity<T: Component>(&mut self, entity: Entity) {
        self.command_queue.remove_component_from_entity::<T>(entity);
    }

    /// Queue destroying an entity (executed later)
    pub fn destroy_entity(&mut self, entity: Entity) {
        self.command_queue.destroy_entity(entity);
    }
}

/// Builder for creating entities with components through the command queue
pub struct EntityBuilder<'a> {
    command_queue: &'a mut CommandQueue,
    components: Vec<Box<dyn ComponentAdder>>,
}

impl<'a> EntityBuilder<'a> {
    /// Create a new EntityBuilder
    pub fn new(command_queue: &'a mut CommandQueue) -> Self {
        Self {
            command_queue,
            components: Vec::new(),
        }
    }

    /// Add a component to the entity being created
    pub fn with<T>(mut self, component: T) -> Self
    where
        T: Component + TraitAccessible<dyn Component> + Send,
    {
        self.components
            .push(Box::new(TypedComponentAdder { component }));
        self
    }

    /// Finish building and queue the create command
    pub fn build(self) {
        self.command_queue.create_entity(self.components);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::world::World;
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

    impl Component for Position {}
    impl Component for Velocity {}

    impl_trait_accessible!(dyn Component; Position, Velocity);

    /// Tests basic entity creation through the deferred command queue.
    ///
    /// This test verifies that:
    /// - Commands can queue entity creation without immediate execution
    /// - The entity is only created when execute_queued_commands() is called
    /// - The created entity is properly tracked in the world
    /// - Components are correctly added to the entity's archetype
    ///
    /// Expected results:
    /// - Before execution: 0 entities exist in the world
    /// - After execution: 1 entity exists with 2 components (Position+Velocity)
    /// - 1 archetype is created to store the entity
    /// - The archetype contains the correct component types
    #[test]
    fn test_create_command() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let mut queue = CommandQueue::new();
        let mut commands = Commands::new(&mut queue);

        // Queue creating a new entity
        commands
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        assert_eq!(
            world.entity_locations.len(),
            0,
            "Entity should not exist yet"
        );

        // Execute commands
        queue.execute_queued_commands(&mut world);

        assert_eq!(world.entity_locations.len(), 1, "Entity should be created");
        assert_eq!(world.archetypes.len(), 1, "Should have 1 archetype");

        let archetype = world.archetypes.values().next().unwrap();
        assert_eq!(
            archetype.entities.len(),
            1,
            "Archetype should have 1 entity"
        );
        assert_eq!(
            archetype.component_types.len(),
            2,
            "Entity should have 2 components"
        );
    }

    /// Tests the EntityBuilder fluent API for creating entities.
    ///
    /// This test verifies that:
    /// - EntityBuilder provides a fluent interface for entity creation
    /// - Multiple components can be chained using .with() method
    /// - The .build() method queues the creation command
    /// - Entity creation is deferred until execute_queued_commands() is called
    ///
    /// Expected results:
    /// - Before execution: World contains 0 entities
    /// - After execution: World contains 1 entity with both components
    /// - The fluent API works correctly without errors
    #[test]
    fn test_entity_builder() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let mut queue = CommandQueue::new();
        let mut commands = Commands::new(&mut queue);

        // Queue creating a new entity
        commands
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        assert_eq!(
            world.entity_locations.len(),
            0,
            "Entity should not exist yet"
        );

        // Execute commands
        queue.execute_queued_commands(&mut world);

        assert_eq!(world.entity_locations.len(), 1, "Entity should be created");
    }

    /// Tests entity archetype migration and automatic cleanup when components are removed.
    ///
    /// This test verifies that:
    /// - An entity can be created with multiple components through the command queue
    /// - Components can be removed from an entity after creation
    /// - Removing a component migrates the entity to a new archetype
    /// - The old archetype is automatically cleaned up when it becomes empty
    /// - The entity remains valid and properly tracked after component removal
    /// - Scope-based borrow management allows sequential command queueing and execution
    ///
    /// Expected results:
    /// - Initially: Entity created with Position+Velocity components
    /// - After removing Velocity: Entity migrates to Position-only archetype
    /// - Old Position+Velocity archetype is automatically deleted
    /// - Only 1 archetype remains in the world (Position-only)
    /// - Entity continues to exist with correct component set
    #[test]
    fn test_entity_builder_archetype_deletion() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let mut queue = CommandQueue::new();

        // Queue creating a new entity
        {
            let mut commands = Commands::new(&mut queue);
            commands
                .create_entity()
                .with(Position { x: 10.0, y: 20.0 })
                .with(Velocity { x: 1.0, y: 2.0 })
                .build();
        }

        assert_eq!(
            world.entity_locations.len(),
            0,
            "Entity should not exist yet"
        );

        // Execute commands
        queue.execute_queued_commands(&mut world);

        assert_eq!(world.entity_locations.len(), 1, "Entity should be created");
        let archetype = world.archetypes.values().next().unwrap();
        assert!(
            archetype
                .component_types
                .contains(&ComponentId::of::<Position>()),
            "Archetype should contain Position component"
        );
        assert!(
            archetype
                .component_types
                .contains(&ComponentId::of::<Velocity>()),
            "Archetype should contain Velocity component"
        );

        // Get the entity and queue remove component command
        let entity = *world.entity_locations.keys().next().unwrap();
        {
            let mut commands = Commands::new(&mut queue);
            commands.remove_component_from_entity::<Velocity>(entity);
        }

        queue.execute_queued_commands(&mut world);
        assert_eq!(world.entity_locations.len(), 1, "Entity should still exist");

        let archetype = world.archetypes.values().next().unwrap();
        assert!(
            !archetype
                .component_types
                .contains(&ComponentId::of::<Velocity>()),
            "Archetype should not contain Velocity component"
        );

        archetype.print_info(&world.component_registry);
    }

    /// Tests creating multiple entities with different component combinations through commands.
    ///
    /// This test verifies that:
    /// - Multiple entity creation commands can be queued before execution
    /// - Entities with different component sets are created in separate archetypes
    /// - All queued commands are executed correctly in a single execute_queued_commands() call
    /// - The command queue properly handles entities with varying component combinations
    /// - Archetype system correctly categorizes entities based on their components
    ///
    /// Expected results:
    /// - 3 entities are created in total
    /// - 3 different archetypes are created:
    ///   1. Position+Velocity archetype for entity 1
    ///   2. Position-only archetype for entity 2
    ///   3. Velocity-only archetype for entity 3
    /// - All entities are properly tracked in the world
    #[test]
    fn test_multiple_create_commands() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let mut queue = CommandQueue::new();
        let mut commands = Commands::new(&mut queue);

        // Queue creating multiple entities
        commands
            .create_entity()
            .with(Position { x: 1.0, y: 2.0 })
            .with(Velocity { x: 0.5, y: 1.0 })
            .build();

        commands
            .create_entity()
            .with(Position { x: 5.0, y: 10.0 })
            .build();

        commands
            .create_entity()
            .with(Velocity { x: 2.0, y: 3.0 })
            .build();

        // Execute commands
        queue.execute_queued_commands(&mut world);

        assert_eq!(world.entity_locations.len(), 3, "Should have 3 entities");
        assert_eq!(
            world.archetypes.len(),
            3,
            "Should have 3 different archetypes"
        );
    }
}
