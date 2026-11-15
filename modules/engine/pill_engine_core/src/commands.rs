//! Deferred command queue for structural ECS mutations.
//!
//! # Responsibilities
//!
//! - Provides [`Commands`], the handle a system uses to queue structural edits
//!   without holding `&mut World`: creating and destroying entities, and
//!   adding and removing components.
//! - Implements the two-phase frame lifecycle: systems queue commands while
//!   they run, and the engine applies them after the last system finishes.
//! - Re-exports [`CommandError`], the failure a queued command reports when the
//!   flush cannot apply it.
//! - Provides [`DeferredEntityBuilder`], which collects the components of a new
//!   entity over several calls and then queues its creation.
//!
//! # Design
//!
//! Systems never mutate the world's structure directly. They queue, and the
//! flush that follows the last system applies each command:
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
//! Queuing instead of applying is what lets systems run in parallel without
//! locks. It also keeps an archetype's rows fixed while a query iterates them,
//! because no entity moves to another archetype before the flush.
//!
//! ## Usage Example
//!
//! ```no_run
//! # use pill_engine::*;
//! # #[derive(Debug, Clone)] struct Health { current: f32 }
//! # impl Component for Health {}
//! fn combat_system(mut query: Query<(&Health, Entity)>, mut commands: Commands) {
//!     for (health, entity) in query.iter_mut() {
//!         if health.current <= 0.0 {
//!             // Queue for destruction - doesn't happen immediately!
//!             commands.destroy_entity(entity);
//!         }
//!     }
//! }
//! // After all systems have run, the engine applies the queue and the dead
//! // entities are actually removed.
//! ```

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::warn;

// Current crate
use crate::archetype::ComponentColumns;
use crate::component::{Component, ComponentId};
use crate::entity::Entity;
use crate::error::WorldError;
use crate::world::World;

// =============================================================================
// ComponentAdder
// =============================================================================

/// Trait for adding a component with its concrete type preserved.
///
/// Must be `Send` to support parallel execution of systems.
pub trait ComponentAdder: Send {
    /// Returns the [`ComponentId`] of the component this adder will insert.
    ///
    /// Used to compute the target archetype before the component value itself
    /// is written into storage.
    fn component_id(&self) -> ComponentId;

    /// Writes the held component into the given type-erased storage.
    ///
    /// The caller is responsible for having created the storage row for this
    /// adder's component type before invoking this method.
    ///
    /// # Errors
    ///
    /// Returns the column's refusal when the column will not take the write. A
    /// caller must report that error and never drop it. By the time this method
    /// runs, the entity's row is already in the archetype's other columns, so a
    /// dropped failure leaves this column one row short. Every later lookup of
    /// this component would then read another entity's value.
    fn add_component_to_storage(
        self: Box<Self>,
        new_storage: &mut ComponentColumns,
    ) -> Result<(), WorldError>;
}

// =============================================================================
// TypedComponentAdder
// =============================================================================

/// Typed component adder that knows the concrete type `T`.
///
/// Wraps the concrete component value so it can be transported through the
/// type-erased [`DeferredCommand`] queue without losing its native type.
struct TypedComponentAdder<T: Component> {
    /// The concrete component value to insert when the command executes.
    component: T,
}

impl<T: Component + Send> ComponentAdder for TypedComponentAdder<T> {
    fn component_id(&self) -> ComponentId {
        ComponentId::of::<T>()
    }

    fn add_component_to_storage(
        self: Box<Self>,
        new_storage: &mut ComponentColumns,
    ) -> Result<(), WorldError> {
        // The storage row for `T` was allocated by the caller; append the
        // component value to it to finish the insertion.
        new_storage.column_of_mut::<T>().push::<T>(self.component);
        Ok(())
    }
}

// =============================================================================
// ByteComponentAdder
// =============================================================================

/// Type-erased component adder that writes raw bytes into a native column.
///
/// It is how a caller that holds only a component id and its bytes - the C#
/// backend, the editor's command batches, the scene loader - adds a component
/// without naming the concrete Rust type. The bytes must match the component's
/// layout in memory exactly; the caller that queues the command checks the
/// size, not this adder.
pub struct ByteComponentAdder {
    /// Native component identity the bytes belong to.
    component_id: ComponentId,
    /// The component value as raw bytes, in its native layout.
    component_bytes: Vec<u8>,
}

impl ByteComponentAdder {
    /// Creates an adder that writes `bytes` as the value of the native
    /// component `component_id`.
    pub fn new(component_id: ComponentId, bytes: Vec<u8>) -> Self {
        Self {
            component_id,
            component_bytes: bytes,
        }
    }
}

impl ComponentAdder for ByteComponentAdder {
    fn component_id(&self) -> ComponentId {
        self.component_id
    }

    fn add_component_to_storage(
        self: Box<Self>,
        new_storage: &mut ComponentColumns,
    ) -> Result<(), WorldError> {
        // The storage row for the native component was allocated by the
        // caller; copy the raw ABI bytes into it.
        assert!(
            self.component_id.is_native_storage(),
            "byte component adder requires a native component id"
        );
        let column = new_storage
            .get_mut(self.component_id)
            .expect("native column must exist for a registered component");
        // The refusal must not be dropped. By the time this runs, the entity's
        // row is already in every other column of the destination archetype. A
        // column left one row short would shift every later row of this
        // component onto another entity's value.
        column.push_bytes(&self.component_bytes)
    }
}

// =============================================================================
// DeferredCommand
// =============================================================================

/// A structural edit that waits in the queue for the flush to apply it.
//
// Every variant acts on one entity, so every variant name ends in `Entity`.
// The suffix is the subject of the name, not noise: without it the variants
// would be `Create`, `AddComponentTo` and `Destroy`, which read worse at the
// match sites.
#[allow(clippy::enum_variant_names)]
enum DeferredCommand {
    /// Create a new entity carrying native and type-erased components.
    CreateEntity {
        /// The entity handle, allocated before the command was queued.
        entity: Entity,
        /// Native components to insert into the new archetype's storage.
        component_adders: Vec<Box<dyn ComponentAdder>>,
        /// Type-erased runtime components to write after the row exists.
        descriptor_components: Vec<(ComponentId, Vec<u8>)>,
    },
    /// Add a native component to an existing entity.
    AddComponentToEntity {
        /// The target entity.
        entity: Entity,
        /// The typed adder that writes the component during execution.
        component_adder: Box<dyn ComponentAdder>,
    },
    /// Add a type-erased runtime component to an existing entity.
    AddDescriptorComponentToEntity {
        /// The target entity.
        entity: Entity,
        /// Runtime ID of the component type to write.
        component_id: ComponentId,
        /// The raw bytes of the component value to store.
        bytes: Vec<u8>,
    },
    /// Remove a component from an existing entity.
    RemoveComponentFromEntity {
        /// The target entity.
        entity: Entity,
        /// Runtime ID of the component type to remove.
        component_id: ComponentId,
    },
    /// Destroy an entity entirely.
    DestroyEntity {
        /// The entity to remove from the world.
        entity: Entity,
    },
}

// =============================================================================
// CommandError
// =============================================================================

pub use crate::error::CommandError;

// =============================================================================
// CommandQueue
// =============================================================================

/// The queue of deferred structural edits for one frame.
///
/// A system that needs to create or destroy an entity, or add or remove a
/// component, appends the edit here instead of changing the world. The engine
/// applies every queued edit in a separate phase after all systems have run.
pub struct CommandQueue {
    /// The deferred operations queued this frame, in submission order.
    commands: Vec<DeferredCommand>,
}

impl CommandQueue {
    /// Creates an empty command queue.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Throws away every queued command without applying it.
    ///
    /// This is the rollback half of the queue-without-applying pair: a step
    /// that queued commands and then failed calls it, so none of those commands
    /// reaches the world.
    pub(crate) fn clear(&mut self) {
        self.commands.clear();
    }
}

impl CommandQueue {
    /// Queues the creation of an entity with the given components.
    ///
    /// `entity` must already be allocated from the world's free list. The
    /// entity does not exist in the world until the queue is flushed.
    pub fn create_entity(&mut self, entity: Entity, components: Vec<Box<dyn ComponentAdder>>) {
        self.commands.push(DeferredCommand::CreateEntity {
            entity,
            component_adders: components,
            descriptor_components: Vec::new(),
        });
    }

    /// Queues the creation of an entity that carries both native components
    /// and type-erased runtime components.
    pub fn create_mixed_entity(
        &mut self,
        entity: Entity,
        native_components: Vec<Box<dyn ComponentAdder>>,
        descriptor_components: Vec<(ComponentId, Vec<u8>)>,
    ) {
        self.commands.push(DeferredCommand::CreateEntity {
            entity,
            component_adders: native_components,
            descriptor_components,
        });
    }

    /// Queues adding a component to an entity.
    pub fn add_component_to_entity<T>(&mut self, entity: Entity, component: T)
    where
        T: Component + Send,
    {
        self.commands.push(DeferredCommand::AddComponentToEntity {
            entity,
            component_adder: Box::new(TypedComponentAdder { component }),
        });
    }

    /// Queues a native component that the caller has already built as a
    /// [`ComponentAdder`].
    pub fn add_component_adder_to_entity(
        &mut self,
        entity: Entity,
        component_adder: Box<dyn ComponentAdder>,
    ) {
        self.commands.push(DeferredCommand::AddComponentToEntity {
            entity,
            component_adder,
        });
    }

    /// Queues adding an existing runtime component, identified by its
    /// component id and written from `bytes`.
    pub fn add_descriptor_component_to_entity(
        &mut self,
        entity: Entity,
        component_id: ComponentId,
        bytes: Vec<u8>,
    ) {
        self.commands
            .push(DeferredCommand::AddDescriptorComponentToEntity {
                entity,
                component_id,
                bytes,
            });
    }

    /// Queues removing a component from an entity.
    pub fn remove_component_from_entity<T: Component>(&mut self, entity: Entity) {
        self.commands
            .push(DeferredCommand::RemoveComponentFromEntity {
                entity,
                component_id: ComponentId::of::<T>(),
            });
    }

    /// Queues removing a component selected by its runtime component id.
    pub fn remove_component_by_id(&mut self, entity: Entity, component_id: ComponentId) {
        self.commands
            .push(DeferredCommand::RemoveComponentFromEntity {
                entity,
                component_id,
            });
    }

    /// Queues destroying an entity.
    pub fn destroy_entity(&mut self, entity: Entity) {
        self.commands
            .push(DeferredCommand::DestroyEntity { entity });
    }

    /// Executes every queued command against the world and empties the queue.
    ///
    /// The engine calls this at the end of a frame, after every system has run.
    /// The one-shot `run_deferred_commands` and `flush_deferred_commands`
    /// helpers call it too.
    ///
    /// # Errors
    ///
    /// With `exit_on_error` set, returns every command failure the flush
    /// collected. With it cleared, the same failures are logged through the
    /// telemetry warn target and the profile error channel instead, and the
    /// flush reports success.
    pub(crate) fn execute_queued_commands(
        &mut self,
        world: &mut World,
        exit_on_error: bool,
    ) -> Result<(), Vec<CommandError>> {
        // Step 1: Record the pending count for diagnostics and open the profiling scope.
        let pending_command_count = self.commands.len();
        world.commands_executed_this_frame = pending_command_count;
        let zone = crate::profile_scope!(
            "execute commands",
            [("Deferred commands to execute: {}", pending_command_count)]
        );

        // Step 2: Drain the queue and dispatch each command to its executor.
        let mut errors = Vec::new();
        let mut succeeded_command_count = 0usize;

        for command in self.commands.drain(..) {
            // Every executor reports a failure by appending to `errors`, so a
            // command that appended nothing succeeded. Counting the vector
            // length keeps one source of truth. An unconditional
            // `succeeded_command_count += 1` would also count the failures of
            // the create arms.
            let error_count_before = errors.len();
            match command {
                DeferredCommand::CreateEntity {
                    entity,
                    component_adders,
                    descriptor_components,
                } => {
                    // A creation can fail to write one or more components after
                    // the row exists. Every failure is collected instead of
                    // panicking inside the flush, and a creation that did not
                    // finish is not counted as succeeded.
                    if let Err(mut failures) = Self::execute_create_entity(
                        world,
                        entity,
                        component_adders,
                        descriptor_components,
                    ) {
                        errors.append(&mut failures);
                    }
                }

                DeferredCommand::AddComponentToEntity {
                    entity,
                    component_adder,
                } => {
                    Self::execute_add_component(world, entity, component_adder, &mut errors);
                }

                DeferredCommand::AddDescriptorComponentToEntity {
                    entity,
                    component_id,
                    bytes,
                } => {
                    Self::execute_add_descriptor_component(
                        world,
                        entity,
                        component_id,
                        bytes,
                        &mut errors,
                    );
                }

                DeferredCommand::RemoveComponentFromEntity {
                    entity,
                    component_id,
                } => {
                    Self::execute_remove_component(world, entity, component_id, &mut errors);
                }

                DeferredCommand::DestroyEntity { entity } => {
                    Self::execute_destroy_entity(world, entity, &mut errors);
                }
            }
            if errors.len() == error_count_before {
                succeeded_command_count += 1;
            }
        }

        // Step 3: Report success/failure totals to the profiler.
        zone.text(format_args!(
            "{} succeeded, {} errors",
            succeeded_command_count,
            errors.len(),
        ));

        // Step 4: Return the failures, or log them and carry on.
        if errors.is_empty() {
            Ok(())
        } else if exit_on_error {
            Err(errors)
        } else {
            for err in &errors {
                crate::profile_error!("deferred command failed: {}", err);
                warn!(
                    target: telemetry_target::ECS,
                    error = %err,
                    "deferred command failed"
                );
            }
            Ok(())
        }
    }

    /// Executes a queued entity-creation command.
    ///
    /// Creates the archetype row for the entity and writes both its native
    /// and type-erased components. The component data is trusted: the bytes
    /// that arrive from the C# side are checked before they are queued.
    ///
    /// # Errors
    ///
    /// Returns `DuplicateComponent` when the component list names one id
    /// twice. That refusal happens before the row exists, so the slot the
    /// caller was given is recycled.
    ///
    /// Returns `ComponentWriteFailed` for a component that could not be
    /// written. That failure happens after the insert and cannot be undone:
    /// the row is already in every other column, so there is nothing to roll
    /// back to and the entity stays live.
    fn execute_create_entity(
        world: &mut World,
        entity: Entity,
        component_adders: Vec<Box<dyn ComponentAdder>>,
        descriptor_components: Vec<(ComponentId, Vec<u8>)>,
    ) -> Result<(), Vec<CommandError>> {
        // Step 1: Collect the full component-ID set that defines the new archetype.
        let mut component_ids: Vec<ComponentId> = component_adders
            .iter()
            .map(|adder| adder.component_id())
            .collect();
        component_ids.extend(descriptor_components.iter().map(|(id, _)| *id));

        // Step 2: Refuse a repeated id before the row exists. This set defines
        // the archetype's columns, so a duplicate would push two rows for one
        // entity row, and the archetype insert would panic when it found the id
        // already present. The callers that can pass duplicates
        // (`create_descriptor_entity` and the C# create path) remove them first;
        // this entry point reports the duplicate instead of guessing which of
        // the two values was meant.
        component_ids.sort_unstable();
        if let Some(duplicate) = component_ids
            .windows(2)
            .find(|pair| pair[0] == pair[1])
            .map(|pair| pair[0])
        {
            world.recycle_refused_entity(entity);
            return Err(vec![CommandError::DuplicateComponent {
                entity,
                component_id: duplicate,
            }]);
        }

        // Step 3: Insert the entity row and write its native components into
        // storage.
        //
        // A command never supplies the stable id: the world creates one, and a
        // `ReserveEntity` call from the C# side reads it back afterwards.
        //
        // By the time an adder runs, the entity's row is already in every other
        // column. An adder that refuses therefore cannot be retried or undone
        // here. Its failure is carried out of the closure and reported with the
        // rest of the batch, because dropping it would leave the entity's
        // columns out of step from then on.
        let mut write_failure: Option<(ComponentId, WorldError)> = None;
        world.insert_entity_with_components(entity, component_ids, None, |storage| {
            for component_adder in component_adders {
                let component_id = component_adder.component_id();
                if let Err(error) = component_adder.add_component_to_storage(storage) {
                    write_failure = Some((component_id, error));
                    break;
                }
            }
        });

        // Step 4: Write the type-erased components once the entity row exists.
        //
        // These were checked when the command was queued, not now. A reload can
        // retire a component type between the queue and the flush, and then
        // this names storage that no longer exists. The failure is collected
        // and reported with the rest of the batch. A panic here would unwind
        // inside the flush and, for a C# project, across the C ABI.
        let mut errors = Vec::new();
        if let Some((component_id, error)) = write_failure {
            errors.push(CommandError::ComponentWriteFailed {
                entity,
                component_id,
                reason: error.to_string(),
            });
        }
        for (component_id, bytes) in descriptor_components {
            if let Err(error) = world.set_descriptor_component_bytes(entity, component_id, &bytes) {
                errors.push(CommandError::ComponentWriteFailed {
                    entity,
                    component_id,
                    reason: error.to_string(),
                });
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Executes a queued type-erased component addition.
    ///
    /// Rejects an entity that no longer exists and a component the entity
    /// already has, then writes the bytes into the entity's component storage.
    fn execute_add_descriptor_component(
        world: &mut World,
        entity: Entity,
        component_id: ComponentId,
        bytes: Vec<u8>,
        errors: &mut Vec<CommandError>,
    ) {
        // Step 1: Reject commands that reference an entity which no longer exists.
        if !world.is_entity_valid(entity) {
            errors.push(CommandError::EntityNotFound {
                entity,
                operation: "add_component",
            });
            return;
        }
        // Step 2: Reject components the entity already carries.
        let already_present = world
            .storage
            .entity_locations
            .get(&entity)
            .and_then(|location| world.storage.archetypes.get(&location.archetype_id))
            .is_some_and(|archetype| archetype.component_types.contains(&component_id));
        if already_present {
            errors.push(CommandError::ComponentAlreadyExists {
                entity,
                component_id,
            });
            return;
        }
        // Step 3: Write the bytes into the component storage.
        //
        // They were checked when the command was queued, not now. A reload can
        // retire a component type between the queue and the flush, and then
        // this names storage that no longer exists. That failure joins the rest
        // of the batch, because a panic here would unwind inside the flush and,
        // for a C# project, across the C ABI.
        if let Err(error) = world.add_descriptor_component(entity, component_id, &bytes) {
            errors.push(CommandError::ComponentWriteFailed {
                entity,
                component_id,
                reason: error.to_string(),
            });
        }
    }

    /// Executes a queued native component addition.
    ///
    /// Moves the entity to a new archetype that includes the component, copying
    /// every component it already has and writing the new one. It runs the same
    /// code as the eager [`World::add_component`], so the checks for an
    /// unregistered type and for a component the entity already has apply here
    /// too.
    fn execute_add_component(
        world: &mut World,
        entity: Entity,
        component_adder: Box<dyn ComponentAdder>,
        errors: &mut Vec<CommandError>,
    ) {
        let component_id = component_adder.component_id();
        // The closure given to the migration reports a refused write. On an
        // error the migration puts the source archetype back and undoes the
        // destination row, so a column that will not take the new component
        // fails the whole migration instead of leaving a half-written archetype
        // behind.
        if let Err(error) = world.try_add_component_by_id(entity, component_id, |new_storage| {
            component_adder.add_component_to_storage(new_storage)
        }) {
            errors.push(Self::command_error_of(
                entity,
                "add_component",
                component_id,
                error,
            ));
        }
    }

    /// Executes a queued component removal.
    ///
    /// Moves the entity to a new archetype without the component, or destroys
    /// it when no components are left. It runs the same code as the eager
    /// removal.
    fn execute_remove_component(
        world: &mut World,
        entity: Entity,
        component_id: ComponentId,
        errors: &mut Vec<CommandError>,
    ) {
        if let Err(error) = world.try_remove_component_by_id(entity, component_id) {
            errors.push(Self::command_error_of(
                entity,
                "remove_component",
                component_id,
                error,
            ));
        }
    }

    /// Executes a queued entity destruction.
    ///
    /// Records `CommandError::EntityNotFound` when the entity is already gone,
    /// matching the behaviour of the other command executors.
    fn execute_destroy_entity(world: &mut World, entity: Entity, errors: &mut Vec<CommandError>) {
        if !world.destroy_entity(entity) {
            errors.push(CommandError::EntityNotFound {
                entity,
                operation: "destroy_entity",
            });
        }
    }

    /// Turns the world's error for a refused structural edit into a command
    /// error.
    ///
    /// The two executors that add and remove components use it, so a rule that
    /// refuses an edit reports the same way whichever command queued it.
    fn command_error_of(
        entity: Entity,
        operation: &'static str,
        component_id: ComponentId,
        error: WorldError,
    ) -> CommandError {
        match error {
            // A stale location record leaves the entity unreachable, which is
            // what the flush reports.
            WorldError::EntityNotFound | WorldError::ArchetypeMissing { .. } => {
                CommandError::EntityNotFound { entity, operation }
            }
            WorldError::DescriptorComponentAlreadyPresent => CommandError::ComponentAlreadyExists {
                entity,
                component_id,
            },
            WorldError::DescriptorComponentMissing => CommandError::ComponentNotFound {
                entity,
                component_id,
            },
            // A type this world no longer registers, so there is no column to
            // write - the same report a queued creation gives.
            WorldError::DescriptorComponentNotRegistered { .. } => {
                CommandError::ComponentWriteFailed {
                    entity,
                    component_id,
                    reason: error.to_string(),
                }
            }
            other => CommandError::MigrationFailed {
                entity,
                reason: other.to_string(),
            },
        }
    }

    /// Returns whether no commands are currently queued.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pill_engine::commands::CommandQueue;
    /// let queue = CommandQueue::new();
    /// assert!(queue.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Commands
// =============================================================================

/// The handle a system uses to queue entity operations for later.
///
/// It gives the system the command queue and the world the queued entities are
/// allocated from. [`Self::create_entity`] takes the entity id from the world's
/// free list right away, so the caller can keep it even though the entity does
/// not exist in the world until the queue is flushed.
pub struct Commands<'a> {
    /// The queue that deferred operations are appended to.
    command_queue: &'a mut CommandQueue,
    /// The world the entity ids are allocated from, and that the flush later
    /// mutates.
    world: &'a mut World,
}

impl<'a> Commands<'a> {
    /// Wraps a command queue and a world for one frame's system execution.
    pub(crate) fn new(command_queue: &'a mut CommandQueue, world: &'a mut World) -> Self {
        Self {
            command_queue,
            world,
        }
    }

    /// Starts building a new entity; its creation is queued when it is built.
    ///
    /// The entity id is taken from the free list immediately and returned by
    /// [`.build()`](DeferredEntityBuilder::build). The entity does not appear in
    /// the world until the queue is flushed at the end of the frame.
    pub fn create_entity(&mut self) -> DeferredEntityBuilder<'_> {
        let entity = self.world.allocate_entity();
        DeferredEntityBuilder {
            command_queue: self.command_queue,
            allocated_entity: entity,
            component_adders: Vec::with_capacity(
                crate::config::EntityBuilderConfig::DEFAULT_COMPONENTS_CAPACITY,
            ),
        }
    }

    /// Queues adding a component to an entity; the flush applies it later.
    pub fn add_component_to_entity<T>(&mut self, entity: Entity, component: T)
    where
        T: Component + Send,
    {
        self.command_queue
            .add_component_to_entity(entity, component);
    }

    /// Queues removing a component from an entity; the flush applies it later.
    pub fn remove_component_from_entity<T: Component>(&mut self, entity: Entity) {
        self.command_queue.remove_component_from_entity::<T>(entity);
    }

    /// Queues destroying an entity; the entity is removed at the flush.
    pub fn destroy_entity(&mut self, entity: Entity) {
        self.command_queue.destroy_entity(entity);
    }
}

// =============================================================================
// DeferredEntityBuilder
// =============================================================================

/// Builds an entity's component list and queues its creation.
///
/// [`World::EntityBuilder`](crate::world::EntityBuilder) creates the entity
/// immediately. This builder queues the creation instead. It still takes the
/// entity id from the free list when it is constructed, so
/// [`build()`](Self::build) can return the handle at once. The entity becomes
/// visible to queries when the current frame's queue is flushed.
pub struct DeferredEntityBuilder<'a> {
    /// The queue the finished create command is appended to.
    command_queue: &'a mut CommandQueue,
    /// The entity handle allocated from the free list at construction time.
    allocated_entity: Entity,
    /// Native components accumulated via [`with`](Self::with).
    component_adders: Vec<Box<dyn ComponentAdder>>,
}

impl<'a> DeferredEntityBuilder<'a> {
    /// Creates a builder whose entity id is already allocated from the world.
    ///
    /// [`ScriptContext::create_entity`](crate::scripting::ScriptContext::create_entity)
    /// calls this. [`Commands::create_entity`] builds the same state itself,
    /// because it already has the world it took the handle from.
    pub fn new(command_queue: &'a mut CommandQueue, world: &mut World) -> Self {
        Self {
            command_queue,
            allocated_entity: world.allocate_entity(),
            component_adders: Vec::with_capacity(
                crate::config::EntityBuilderConfig::DEFAULT_COMPONENTS_CAPACITY,
            ),
        }
    }

    /// Adds a component to the entity being created.
    pub fn with<T>(mut self, component: T) -> Self
    where
        T: Component + Send,
    {
        self.component_adders
            .push(Box::new(TypedComponentAdder { component }));
        self
    }

    /// Finishes the builder and queues the entity's creation.
    ///
    /// Returns the entity handle at once. The entity does not appear in world
    /// queries until the queue is flushed at the end of the frame, but the
    /// handle is valid and can be stored for later use.
    pub fn build(self) -> Entity {
        let entity = self.allocated_entity;
        self.command_queue
            .create_entity(entity, self.component_adders);
        entity
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Boxes a component into a type-erased [`ComponentAdder`] that keeps `T`.
///
/// The typed adder behind it (`TypedComponentAdder`) is private, so this is
/// how a caller builds the list [`CommandQueue::create_mixed_entity`] takes, or
/// the single adder [`CommandQueue::add_component_adder_to_entity`] takes. The
/// adder reports `ComponentId::of::<T>()` and moves the value into its column.
pub fn boxed_component_adder<T>(component: T) -> Box<dyn ComponentAdder>
where
    T: Component + Send,
{
    Box::new(TypedComponentAdder { component })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard};

    use super::*;
    use crate::archetype::Blittability;
    use crate::test_fixtures::{Health, Position, Velocity};
    use crate::world::World;

    /// Counts how many `OwningComponent` values are alive.
    ///
    /// An archetype move must not change it: the row travels bitwise, so no
    /// value is created and none is destroyed on the way.
    static OWNING_COMPONENTS_ALIVE: AtomicUsize = AtomicUsize::new(0);

    /// Serializes the tests that read the process-wide live count, which would
    /// otherwise see each other's values while cargo runs them in parallel.
    static OWNING_COMPONENT_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Takes the live-count lock and resets the counter.
    ///
    /// A poisoning left by an earlier failed test is ignored: the counter is
    /// reset either way, and a second failure reported as a poisoned lock would
    /// hide the assertion that actually matters.
    fn owning_component_test_guard() -> MutexGuard<'static, ()> {
        let guard = OWNING_COMPONENT_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        OWNING_COMPONENTS_ALIVE.store(0, Ordering::SeqCst);
        guard
    }

    /// A component that owns a heap allocation. A move that clones the row and
    /// drops the original therefore looks different from a bitwise copy, and
    /// not only in the live counter.
    #[derive(Debug)]
    struct OwningComponent {
        owned_values: Vec<u32>,
    }

    impl OwningComponent {
        fn new(owned_values: Vec<u32>) -> Self {
            OWNING_COMPONENTS_ALIVE.fetch_add(1, Ordering::SeqCst);
            Self { owned_values }
        }
    }

    impl Clone for OwningComponent {
        fn clone(&self) -> Self {
            Self::new(self.owned_values.clone())
        }
    }

    impl Drop for OwningComponent {
        fn drop(&mut self) {
            OWNING_COMPONENTS_ALIVE.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl Component for OwningComponent {}

    /// A queued creation stays invisible until the flush, then appears as one
    /// entity in one archetype carrying both components.
    #[test]
    fn test_create_command() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let mut queue = CommandQueue::new();
        let mut commands = Commands::new(&mut queue, &mut world);

        commands
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        assert_eq!(
            world.storage.entity_locations.len(),
            0,
            "Entity should not exist yet"
        );

        queue.execute_queued_commands(&mut world, false).unwrap();

        assert_eq!(
            world.storage.entity_locations.len(),
            1,
            "Entity should be created"
        );
        assert_eq!(world.storage.archetypes.len(), 1, "Should have 1 archetype");

        let archetype = world.storage.archetypes.values().next().unwrap();
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

    /// The builder's chained `with` calls queue one deferred creation.
    #[test]
    fn test_entity_builder() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let mut queue = CommandQueue::new();
        let mut commands = Commands::new(&mut queue, &mut world);

        commands
            .create_entity()
            .with(Position { x: 10.0, y: 20.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        assert_eq!(
            world.storage.entity_locations.len(),
            0,
            "Entity should not exist yet"
        );

        queue.execute_queued_commands(&mut world, false).unwrap();

        assert_eq!(
            world.storage.entity_locations.len(),
            1,
            "Entity should be created"
        );
    }

    #[test]
    fn type_erased_commands_create_add_remove_and_destroy_through_migrations() {
        let mut world = World::new();
        world.register_component::<Position>();
        let descriptor_a = world
            .register_component_descriptor(
                0xA1,
                "Project.DescriptorA",
                4,
                4,
                1,
                Blittability::engine_verified(),
            )
            .unwrap();
        let descriptor_b = world
            .register_component_descriptor(
                0xB2,
                "Project.DescriptorB",
                4,
                4,
                2,
                Blittability::engine_verified(),
            )
            .unwrap();
        let entity = world.reserve_entity();
        let mut queue = CommandQueue::new();

        queue.create_mixed_entity(
            entity,
            vec![boxed_component_adder(Position { x: 3.0, y: 4.0 })],
            vec![(descriptor_a, 11_u32.to_ne_bytes().to_vec())],
        );
        assert!(
            !world.is_entity_valid(entity),
            "creation must remain deferred"
        );
        queue.execute_queued_commands(&mut world, true).unwrap();
        assert_eq!(world.entity_count(), 1);
        assert_eq!(world.get_component::<Position>(entity).unwrap().x, 3.0);
        assert_eq!(
            world
                .descriptor_component_bytes(entity, descriptor_a)
                .unwrap(),
            11_u32.to_ne_bytes()
        );

        queue.add_descriptor_component_to_entity(
            entity,
            descriptor_b,
            22_u32.to_ne_bytes().to_vec(),
        );
        queue.remove_component_by_id(entity, descriptor_a);
        queue.execute_queued_commands(&mut world, true).unwrap();
        assert!(world
            .descriptor_component_bytes(entity, descriptor_a)
            .is_none());
        assert_eq!(
            world
                .descriptor_component_bytes(entity, descriptor_b)
                .unwrap(),
            22_u32.to_ne_bytes()
        );
        assert_eq!(world.get_component::<Position>(entity).unwrap().y, 4.0);

        queue.destroy_entity(entity);
        assert!(
            world.is_entity_valid(entity),
            "destruction must remain deferred"
        );
        queue.execute_queued_commands(&mut world, true).unwrap();
        assert!(!world.is_entity_valid(entity));
    }

    #[test]
    fn type_erased_command_rejects_a_stale_entity_generation() {
        let mut world = World::new();
        world.register_component::<Position>();
        let entity = world
            .create_entity()
            .with(Position { x: 1.0, y: 2.0 })
            .build()
            .unwrap();
        assert!(world.destroy_entity(entity));
        let replacement = world.reserve_entity();
        assert_eq!(replacement.id(), entity.id());
        assert_ne!(replacement.generation(), entity.generation());

        let mut queue = CommandQueue::new();
        queue.destroy_entity(entity);
        let errors = queue.execute_queued_commands(&mut world, true).unwrap_err();
        assert!(matches!(
            errors.as_slice(),
            [CommandError::EntityNotFound {
                operation: "destroy_entity",
                ..
            }]
        ));
    }

    /// A queued creation that lists one component twice is refused before the
    /// entity row exists, naming the component and leaving no partial entity.
    #[test]
    fn mixed_create_with_duplicate_id_reports_error() {
        let mut world = World::new();
        world.register_component::<Position>();
        let entity = world.reserve_entity();
        let archetype_count_before = world.storage.archetypes.len();

        let mut queue = CommandQueue::new();
        queue.create_mixed_entity(
            entity,
            vec![
                boxed_component_adder(Position { x: 1.0, y: 2.0 }),
                boxed_component_adder(Position { x: 3.0, y: 4.0 }),
            ],
            Vec::new(),
        );
        let errors = queue.execute_queued_commands(&mut world, true).unwrap_err();
        assert!(matches!(
            errors.as_slice(),
            [CommandError::DuplicateComponent { component_id, .. }]
                if *component_id == ComponentId::of::<Position>()
        ));

        assert!(
            !world.storage.entity_locations.contains_key(&entity),
            "a refused creation leaves no entity behind"
        );
        assert_eq!(
            world.storage.archetypes.len(),
            archetype_count_before,
            "no archetype was created for the refused set"
        );
    }

    /// A component that owns memory moves across an archetype migration; it is
    /// never cloned and dropped.
    ///
    /// A deep clone followed by a drop would cost an allocation and a free on
    /// every archetype change for a component that holds a `Vec`, and any
    /// `Clone` impl with side effects would run during a move the engine treats
    /// as a plain relocation. The live count proves it: the count may not change
    /// while the entity moves, and it must reach zero exactly once when the
    /// entity is destroyed.
    #[test]
    fn a_migration_moves_an_owning_component_rather_than_cloning_it() {
        let _guard = owning_component_test_guard();
        let mut world = World::new();
        world.register_component::<OwningComponent>();
        world.register_component::<Health>();

        let entity = world
            .create_entity()
            .with(OwningComponent::new(vec![7, 8, 9]))
            .build()
            .unwrap();
        assert_eq!(OWNING_COMPONENTS_ALIVE.load(Ordering::SeqCst), 1);

        // Adding a component migrates the entity to another archetype, which is
        // what carries the owning component across.
        let mut queue = CommandQueue::new();
        queue.add_component_adder_to_entity(entity, boxed_component_adder(Health(5)));
        queue.execute_queued_commands(&mut world, true).unwrap();

        assert_eq!(
            OWNING_COMPONENTS_ALIVE.load(Ordering::SeqCst),
            1,
            "a move creates no second value and destroys no first one"
        );
        assert_eq!(
            world
                .get_component::<OwningComponent>(entity)
                .expect("the component came across")
                .owned_values,
            vec![7, 8, 9],
            "the moved row still owns its allocation"
        );

        // Removing it migrates the entity back, again carrying the row.
        let mut queue = CommandQueue::new();
        queue.remove_component_from_entity::<Health>(entity);
        queue.execute_queued_commands(&mut world, true).unwrap();
        assert_eq!(
            OWNING_COMPONENTS_ALIVE.load(Ordering::SeqCst),
            1,
            "the return migration is a move as well"
        );

        // Destroying the entity is the one place the value is released, and it
        // is released exactly once.
        assert!(world.destroy_entity(entity));
        assert_eq!(
            OWNING_COMPONENTS_ALIVE.load(Ordering::SeqCst),
            0,
            "the moved row is dropped exactly once, by whoever owns it last"
        );
    }

    /// A component the migration leaves behind is dropped, not leaked.
    ///
    /// The counterpart of the test above. The source archetype forgets the rows
    /// it handed over and drops the ones it did not, so removing a component
    /// that owns memory frees that memory at once.
    #[test]
    fn a_removed_owning_component_is_dropped_by_the_archetype_it_leaves() {
        let _guard = owning_component_test_guard();
        let mut world = World::new();
        world.register_component::<OwningComponent>();
        world.register_component::<Health>();

        let entity = world
            .create_entity()
            .with(OwningComponent::new(vec![1, 2]))
            .with(Health(3))
            .build()
            .unwrap();
        assert_eq!(OWNING_COMPONENTS_ALIVE.load(Ordering::SeqCst), 1);

        let mut queue = CommandQueue::new();
        queue.remove_component_from_entity::<OwningComponent>(entity);
        queue.execute_queued_commands(&mut world, true).unwrap();

        assert_eq!(
            OWNING_COMPONENTS_ALIVE.load(Ordering::SeqCst),
            0,
            "the component that did not travel is released by the source archetype"
        );
        assert!(world.get_component::<OwningComponent>(entity).is_none());
        assert_eq!(world.get_component::<Health>(entity).unwrap().0, 3);
        assert!(world.destroy_entity(entity));
    }

    /// Removing a component migrates the entity to the smaller archetype and
    /// retires the one it left once that archetype is empty.
    #[test]
    fn test_entity_builder_archetype_deletion() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let mut queue = CommandQueue::new();

        {
            let mut commands = Commands::new(&mut queue, &mut world);
            commands
                .create_entity()
                .with(Position { x: 10.0, y: 20.0 })
                .with(Velocity { x: 1.0, y: 2.0 })
                .build();
        }

        assert_eq!(
            world.storage.entity_locations.len(),
            0,
            "Entity should not exist yet"
        );

        queue.execute_queued_commands(&mut world, false).unwrap();

        assert_eq!(
            world.storage.entity_locations.len(),
            1,
            "Entity should be created"
        );
        let archetype = world.storage.archetypes.values().next().unwrap();
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

        let entity = *world.storage.entity_locations.keys().next().unwrap();
        {
            let mut commands = Commands::new(&mut queue, &mut world);
            commands.remove_component_from_entity::<Velocity>(entity);
        }

        queue.execute_queued_commands(&mut world, false).unwrap();
        assert_eq!(
            world.storage.entity_locations.len(),
            1,
            "Entity should still exist"
        );

        let archetype = world.storage.archetypes.values().next().unwrap();
        assert!(
            !archetype
                .component_types
                .contains(&ComponentId::of::<Velocity>()),
            "Archetype should not contain Velocity component"
        );

        archetype.print_info(&world.component_registry);
    }

    /// Three queued creates with different component sets produce three
    /// archetypes in one flush.
    #[test]
    fn test_multiple_create_commands() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        let mut queue = CommandQueue::new();
        let mut commands = Commands::new(&mut queue, &mut world);

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

        queue.execute_queued_commands(&mut world, false).unwrap();

        assert_eq!(
            world.storage.entity_locations.len(),
            3,
            "Should have 3 entities"
        );
        assert_eq!(
            world.storage.archetypes.len(),
            3,
            "Should have 3 different archetypes"
        );
    }

    /// A queued create that cannot write one of its components is reported; it
    /// is neither counted as a success nor allowed to panic.
    ///
    /// The component's bytes were checked when the command was queued, but the
    /// world can change before the flush. Here the storage column disappears, as
    /// it does when a reload is only partly applied. `execute_create_entity`
    /// must report `ComponentWriteFailed` through the error list, because a
    /// panic here would unwind through `process_frame` and, for a C# project,
    /// across the C ABI.
    #[test]
    fn a_queued_create_that_cannot_write_a_component_is_reported() {
        let mut world = World::new();
        let descriptor_a = world
            .register_component_descriptor(
                0xA1,
                "Project.DescriptorA",
                4,
                4,
                1,
                Blittability::engine_verified(),
            )
            .unwrap();

        // First flush materialises entity A, which creates the archetype with
        // the descriptor storage column.
        let entity_a = world.reserve_entity();
        let mut queue = CommandQueue::new();
        queue.create_mixed_entity(
            entity_a,
            vec![],
            vec![(descriptor_a, 11_u32.to_ne_bytes().to_vec())],
        );
        queue.execute_queued_commands(&mut world, false).unwrap();
        assert!(world.is_entity_valid(entity_a));

        // Simulate a reload that was only partly applied: the archetype keeps
        // the component in `component_types` but loses its storage column.
        world
            .storage
            .archetypes
            .values_mut()
            .next()
            .unwrap()
            .component_storages
            .remove(descriptor_a);

        // A second queued create reuses the existing archetype, so the row is
        // materialised but the component write fails.
        let entity_b = world.reserve_entity();
        queue.create_mixed_entity(
            entity_b,
            vec![],
            vec![(descriptor_a, 22_u32.to_ne_bytes().to_vec())],
        );
        let errors = queue
            .execute_queued_commands(&mut world, true)
            .expect_err("a component that cannot be written must fail the flush");

        assert!(matches!(
            errors.as_slice(),
            [CommandError::ComponentWriteFailed {
                entity,
                component_id,
                ..
            }] if *entity == entity_b && *component_id == descriptor_a
        ));
    }

    /// A queued removal of the component whose storage vanished completes and
    /// leaves the entity consistent.
    ///
    /// When the source archetype releases its rows, it only goes through the
    /// components it actually has a column for. A component the destination does
    /// not take therefore has nothing left to release. The removal is exactly
    /// the repair the out-of-sync entity needs: it ends up in a well-formed
    /// archetype and the broken one is retired.
    #[test]
    fn a_queued_remove_of_a_component_whose_storage_vanished_completes() {
        let mut world = World::new();
        world.register_component::<Position>();
        let descriptor_a = world
            .register_component_descriptor(
                0xA1,
                "Project.DescriptorA",
                4,
                4,
                1,
                Blittability::engine_verified(),
            )
            .unwrap();

        // Create an entity carrying both a native and a descriptor component,
        // so removing the descriptor one migrates instead of destroying.
        let entity = world.reserve_entity();
        let mut queue = CommandQueue::new();
        queue.create_mixed_entity(
            entity,
            vec![boxed_component_adder(Position { x: 1.0, y: 2.0 })],
            vec![(descriptor_a, 11_u32.to_ne_bytes().to_vec())],
        );
        queue.execute_queued_commands(&mut world, false).unwrap();
        assert!(world.is_entity_valid(entity));

        // Simulate the out-of-sync state: the descriptor column disappears from
        // the archetype while its `component_types` entry survives.
        world
            .storage
            .archetypes
            .values_mut()
            .next()
            .unwrap()
            .component_storages
            .remove(descriptor_a);

        queue.remove_component_by_id(entity, descriptor_a);
        queue
            .execute_queued_commands(&mut world, true)
            .expect("removing the component whose column is gone has nothing to release");

        assert!(world.is_entity_valid(entity));
        assert_eq!(world.get_component::<Position>(entity).unwrap().x, 1.0);
        assert!(
            world
                .descriptor_component_bytes(entity, descriptor_a)
                .is_none(),
            "the removed component is gone from the entity"
        );
        assert_eq!(
            world.storage.archetypes.len(),
            1,
            "the archetype whose column vanished was emptied and retired"
        );
    }

    /// A migration that cannot carry a column across reports `MigrationFailed`
    /// instead of panicking.
    ///
    /// The out-of-sync state arrives as a typed `WorldError`, which reaches the
    /// command error list. The migration refuses it before anything moves, so
    /// the entity stays where it was.
    #[test]
    fn a_queued_migration_that_cannot_carry_a_column_is_reported() {
        let mut world = World::new();
        world.register_component::<Position>();
        let descriptor_a = world
            .register_component_descriptor(
                0xA1,
                "Project.DescriptorA",
                4,
                4,
                1,
                Blittability::engine_verified(),
            )
            .unwrap();
        let descriptor_b = world
            .register_component_descriptor(
                0xA2,
                "Project.DescriptorB",
                4,
                4,
                1,
                Blittability::engine_verified(),
            )
            .unwrap();

        let entity = world.reserve_entity();
        let mut queue = CommandQueue::new();
        queue.create_mixed_entity(
            entity,
            vec![boxed_component_adder(Position { x: 1.0, y: 2.0 })],
            vec![
                (descriptor_a, 11_u32.to_ne_bytes().to_vec()),
                (descriptor_b, 22_u32.to_ne_bytes().to_vec()),
            ],
        );
        queue.execute_queued_commands(&mut world, false).unwrap();

        // The same out-of-sync state, this time on a component the migration has
        // to carry: removing `descriptor_b` leaves `descriptor_a` to travel, and
        // it has no column to travel from.
        world
            .storage
            .archetypes
            .values_mut()
            .next()
            .unwrap()
            .component_storages
            .remove(descriptor_a);

        queue.remove_component_by_id(entity, descriptor_b);
        let errors = queue
            .execute_queued_commands(&mut world, true)
            .expect_err("a migration that cannot complete must fail the flush");

        assert!(matches!(
            errors.as_slice(),
            [CommandError::MigrationFailed { entity: failed_entity, .. }]
                if *failed_entity == entity
        ));
        assert_eq!(
            world.descriptor_component_bytes(entity, descriptor_b),
            Some(&22_u32.to_ne_bytes()[..]),
            "a refused migration leaves the entity where it was, with the component the removal targeted still on it"
        );
        assert_eq!(world.get_component::<Position>(entity).unwrap().x, 1.0);
    }

    /// A raw-byte write that the destination column refuses is reported, not
    /// dropped.
    ///
    /// By the time the adder runs, the archetype already holds the entity's row
    /// in every other column. A dropped refusal would leave this column one row
    /// short: every later row of this component would read another entity's
    /// value, and a debug build would trip the archetype's one-row-per-column
    /// assertion at the next migration.
    #[test]
    fn a_native_component_write_the_column_refuses_is_reported() {
        let mut world = World::new();
        world.register_component::<Position>();
        let component_id = ComponentId::of::<Position>();
        let entity = world.reserve_entity();

        // One byte short of the layout `Position` was registered with, which is
        // what bytes sized for a layout that has since changed look like.
        let short_payload = vec![0_u8; std::mem::size_of::<Position>() - 1];

        let mut queue = CommandQueue::new();
        queue.create_mixed_entity(
            entity,
            vec![
                Box::new(ByteComponentAdder::new(component_id, short_payload))
                    as Box<dyn ComponentAdder>,
            ],
            Vec::new(),
        );
        let errors = queue
            .execute_queued_commands(&mut world, true)
            .expect_err("a refused native write must fail the flush");

        assert!(matches!(
            errors.as_slice(),
            [CommandError::ComponentWriteFailed { entity: failed_entity, component_id: failed_component, .. }]
                if *failed_entity == entity && *failed_component == component_id
        ));
    }

    /// A refused deferred creation puts its entity slot back.
    ///
    /// `Commands::create_entity` takes the handle from the free list before the
    /// command is queued. A create that the flush refuses must give that slot
    /// back; otherwise it is never handed out again, and the handle the caller
    /// holds names nothing forever.
    #[test]
    fn a_refused_deferred_create_releases_its_entity_slot() {
        let mut world = World::new();
        world.register_component::<Position>();
        let mut queue = CommandQueue::new();
        let entity = {
            let mut commands = Commands::new(&mut queue, &mut world);
            commands
                .create_entity()
                .with(Position { x: 1.0, y: 2.0 })
                .with(Position { x: 3.0, y: 4.0 })
                .build()
        };

        let errors = queue
            .execute_queued_commands(&mut world, true)
            .expect_err("a duplicated component must fail the flush");
        assert!(matches!(
            errors.as_slice(),
            [CommandError::DuplicateComponent { component_id, .. }]
                if *component_id == ComponentId::of::<Position>()
        ));
        assert!(
            !world.storage.entity_locations.contains_key(&entity),
            "a refused creation leaves no entity behind"
        );
        let recycled = world.allocate_entity();
        assert_eq!(
            recycled.id(),
            entity.id(),
            "the refused create released its slot, so the next allocation reuses it"
        );
        assert_ne!(
            recycled, entity,
            "the recycled slot carries the next generation, so a handle the caller kept cannot name it"
        );
    }

    /// A queued add of a type this world never registered is refused.
    ///
    /// The deferred counterpart of the eager refusal. An unregistered id has no
    /// bit in the archetype's component mask, so the destination archetype works
    /// out the same as the source, and the entity would store nothing while the
    /// flush counted the command as done.
    #[test]
    fn a_queued_add_of_an_unregistered_component_is_refused() {
        let mut world = World::new();
        world.register_component::<Position>();
        let entity = world
            .create_entity()
            .with(Position { x: 1.0, y: 2.0 })
            .build()
            .unwrap();

        let unregistered = ComponentId::of::<Health>();
        let mut queue = CommandQueue::new();
        queue.add_component_adder_to_entity(entity, boxed_component_adder(Health(5)));
        let errors = queue
            .execute_queued_commands(&mut world, true)
            .expect_err("an unregistered component must fail the flush");

        assert!(matches!(
            errors.as_slice(),
            [CommandError::ComponentWriteFailed { entity: failed_entity, component_id, .. }]
                if *failed_entity == entity && *component_id == unregistered
        ));
        assert!(
            world.get_component::<Health>(entity).is_none(),
            "a refused add stores nothing"
        );
        assert_eq!(
            world.get_component::<Position>(entity).unwrap().x,
            1.0,
            "the entity keeps the component it already had"
        );
    }
}
