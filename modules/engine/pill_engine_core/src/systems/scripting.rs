//! Script components, their lifecycle methods and the declaration behind them.
//!
//! # Responsibilities
//!
//! - Defines [`ScriptComponent`] - the five lifecycle methods the engine calls,
//!   and the information each one receives about why it ran.
//! - Defines [`ScriptDeclaration`] and [`ScriptDeclarationData`], what
//!   `#[script]` reads from an implementation block.
//! - Provides [`ScriptContext`] - a restricted view of the ECS that queues
//!   every structural change.
//!
//! # Design
//!
//! A script gets a [`ScriptContext`], never the [`crate::Engine`] itself. The
//! context has no way to add or remove a component, or to destroy an entity,
//! except by queueing the command. So a script cannot move its own row out of
//! the storage its `&mut self` points into, which would leave that reference
//! dangling.
//!
//! A script declares its lifecycle methods instead of announcing them at
//! registration: `#[script]` on the implementation block reads which lifecycle
//! methods the block defines, each lifecycle method's `#[order(n)]`, and
//! whether the script also runs while editing, and writes them into
//! [`ScriptDeclaration::DECLARATION`]. The world reads that value once per
//! registration, so a reload points every lifecycle method at the arriving
//! image.
//!
//! Every lifecycle method has a no-op default, so a script implements only the
//! ones it needs. The engine calls `on_create` when the component starts
//! existing, `on_start` when the script starts running, `on_update` once per
//! frame while it runs, `on_stop` when it stops running, and `on_destroy` when
//! the component stops existing.

// Standard library

// External crates

// Current crate
use crate::commands::CommandQueue;
use crate::component::Component;
use crate::engine::RunMode;
use crate::entity::Entity;
use crate::world::World;
use crate::Resource;

// =============================================================================
// ScriptContext
// =============================================================================

/// A restricted view of the ECS for script components.
///
/// The context gives a script:
/// - read-only world queries: a component's value, an entity's existence, and
///   the first entity that carries a component type;
/// - a mutable borrow of a component on any entity;
/// - read-only access to resources;
/// - deferred structural changes: create or destroy an entity, add or remove
///   a component.
///
/// In an archetype-based ECS, adding or removing a component moves the entity's
/// row into another archetype. A script that did that through `&mut World`
/// while holding `&mut self` would be left with a reference into storage the
/// entity has left. This type never hands out `&mut World`, and every
/// structural change it offers is queued, so a script cannot do that.
pub struct ScriptContext<'a> {
    /// Mutable access to the world for component queries
    world: &'a mut World,
    /// The command queue the deferred structural changes go into
    commands: &'a mut CommandQueue,
    /// The entity this script is attached to
    owning_entity: Entity,
}

impl<'a> ScriptContext<'a> {
    /// Creates a new script context.
    pub(crate) fn new(
        world: &'a mut World,
        commands: &'a mut CommandQueue,
        owning_entity: Entity,
    ) -> Self {
        Self {
            world,
            commands,
            owning_entity,
        }
    }

    /// Gets the entity this script is attached to.
    #[inline]
    pub fn get_owning_entity(&self) -> Entity {
        self.owning_entity
    }

    // =========================================================================
    // World Access
    // =========================================================================

    /// Gets an immutable reference to a component on any entity.
    ///
    /// Returns `None` if the entity does not exist or does not carry `T`.
    pub fn get_component<T>(&self, entity: Entity) -> Option<&T>
    where
        T: Component,
    {
        self.world.get_component::<T>(entity)
    }

    /// Gets a mutable reference to a component on any entity.
    ///
    /// Returns `None` if the entity does not exist or does not carry `T`.
    ///
    /// The reference can point at the script's own component. If the script
    /// asks for its own type, `&mut self` and the returned `&mut T` alias,
    /// which Rust calls undefined behaviour. The API accepts that because it
    /// cannot break in practice. The only thing that can invalidate either
    /// reference is a structural change to the entity, and this context only
    /// queues those.
    pub fn get_component_mut<T>(&mut self, entity: Entity) -> Option<&mut T>
    where
        T: Component,
    {
        // Step 1: Get a raw pointer to the component's storage slot. The
        // lookup returns `None` when the entity is missing or does not carry `T`.
        let component_pointer = self.world.get_component_ptr_mut::<T>(entity)?;

        // Step 2: Turn the raw pointer back into a `&mut T` reference.
        // SAFETY: `World::get_component_ptr_mut` either returns `None` (already
        // handled by `?`) or a valid, aligned, non-null pointer to a live `T`
        // component. The `&mut self` borrow lasts for the whole lifetime of the
        // returned reference, and every structural change (add/remove
        // component, destroy entity) is queued, so the storage slot cannot be
        // moved or freed while the reference is alive. A second `&mut T` for
        // the script's own component type is what the API is for, as the doc
        // comment above says.
        Some(unsafe { &mut *component_pointer })
    }

    /// Checks whether an entity exists.
    ///
    /// Returns `false` for a destroyed entity and for an old handle whose id
    /// has been reused.
    pub fn entity_exists(&self, entity: Entity) -> bool {
        self.world.is_entity_valid(entity)
    }

    /// Gets an immutable reference to a resource.
    ///
    /// Returns `None` if the world holds no resource of type `T`.
    pub fn get_resource<T: Resource>(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// Finds the first entity that carries `T`.
    ///
    /// This is the one lookup the methods above cannot express: a component on
    /// an entity the script neither owns nor holds a handle to - the first path
    /// in the world, say. It walks the archetypes and takes the first entity of
    /// the first one that carries `T`, so it answers `None` when nothing does
    /// and the same entity every frame while the world's structure holds. It is
    /// read-only: nothing it finds can be changed without the queued commands.
    pub fn find_first<T: Component>(&self) -> Option<Entity> {
        self.world
            .archetypes_iter()
            .filter(|archetype| archetype.has_component::<T>())
            .flat_map(|archetype| archetype.entities.iter().copied())
            .next()
    }

    // =========================================================================
    // Deferred Commands
    // =========================================================================

    /// Queues creating a new entity.
    ///
    /// The entity id is allocated now, and the entity joins the world when the
    /// queued commands run, after every script in the frame has returned. The
    /// returned [`crate::commands::DeferredEntityBuilder`] adds components to
    /// it.
    pub fn create_entity(&mut self) -> crate::commands::DeferredEntityBuilder<'_> {
        crate::commands::DeferredEntityBuilder::new(self.commands, self.world)
    }

    /// Queues destroying an entity.
    ///
    /// The command runs after every script in the frame has returned, so a
    /// script may queue the destruction of the entity it runs on.
    pub fn destroy_entity(&mut self, entity: Entity) {
        self.commands.destroy_entity(entity);
    }

    /// Queues adding a component to an entity.
    ///
    /// The command runs after every script in the frame has returned.
    pub fn add_component<T>(&mut self, entity: Entity, component: T)
    where
        T: Component + Send,
    {
        self.commands.add_component_to_entity(entity, component);
    }

    /// Queues removing a component from an entity.
    ///
    /// The command runs after every script in the frame has returned.
    pub fn remove_component<T: Component>(&mut self, entity: Entity) {
        self.commands.remove_component_from_entity::<T>(entity);
    }

    /// Returns the command queue, so a script can queue a command this context
    /// has no method for.
    ///
    /// Queued commands still run only after every script in the frame has
    /// returned.
    pub fn get_commands(&mut self) -> &mut CommandQueue {
        self.commands
    }
}

// =============================================================================
// Lifecycle Method Information
// =============================================================================

/// Everything [`ScriptComponent::on_create`] is told about why it ran.
///
/// The editor, the scene loader and the play session all produce the same
/// `on_create` call; only [`CreateInfo::cause`] says which one did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreateInfo {
    /// What made the component start existing.
    pub cause: CreateCause,
    /// The mode the engine was in: a script can be created while editing a
    /// scene and while playing one.
    pub mode: RunMode,
    /// For a copied component, the entity it was copied from, when the caller
    /// knows which entity that was.
    pub duplicated_from: Option<Entity>,
}

/// What made a script component start existing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CreateCause {
    /// Game code added it, through the frame's command queue or the eager
    /// world API.
    Spawned,
    /// Play started, so every script the edit world held started running.
    PlayStarted,
    /// Code outside the simulation added it: the editor, a tool, a test.
    Authored,
    /// Loading a scene attached it.
    SceneLoaded,
    /// An undo or a redo restored it.
    HistoryRestored,
    /// Stopping play restored the edit world's copy of it.
    PlayStopRestored,
    /// A paste or a duplicate copied it from another entity.
    Duplicated,
}

/// Everything [`ScriptComponent::on_start`] is told about why it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StartInfo {
    /// What made the script start running.
    pub cause: StartCause,
}

/// What made a script start running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartCause {
    /// Play was pressed, so a script the edit world already held started
    /// running.
    PlayEntered,
    /// The component was added while the game was running, so it starts on the
    /// frame it appears.
    SpawnedDuringPlay,
}

/// Everything [`ScriptComponent::on_stop`] is told about why it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StopInfo {
    /// What made the script stop running.
    pub cause: StopCause,
}

/// What made a script stop running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopCause {
    /// Play was stopped, so every running script stopped.
    PlayExited,
    /// The component was removed while the game was running.
    Removed,
}

/// Everything [`ScriptComponent::on_destroy`] is told about why it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DestroyInfo {
    /// What made the component stop existing.
    pub cause: DestroyCause,
}

/// What made a script component stop existing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DestroyCause {
    /// Game code removed it or destroyed its entity.
    Despawned,
    /// Code outside the simulation removed it: the editor, a tool, a test.
    Authored,
    /// Unloading a scene removed it.
    SceneUnloaded,
    /// An undo or a redo removed it.
    HistoryRestored,
    /// Stopping play discarded the running copy of it.
    PlayStopRestored,
}

// =============================================================================
// ScriptDeclaration
// =============================================================================

/// What one `#[script]` implementation block declares.
///
/// The engine reads this once per registration: which lifecycle methods the
/// block defines, the order each one runs in, and whether the script also runs
/// while editing. A lifecycle method that is not implemented has no order the
/// engine would ever read.
///
/// `#[script]` writes the value; a hand-written implementation is only for the
/// engine's own tests, which cannot reach the facade the attribute expands
/// against. The builders below keep such an implementation short.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptDeclarationData {
    /// Whether the block implements `on_create`.
    pub implements_on_create: bool,
    /// Whether the block implements `on_start`.
    pub implements_on_start: bool,
    /// Whether the block implements `on_update`.
    pub implements_on_update: bool,
    /// Whether the block implements `on_stop`.
    pub implements_on_stop: bool,
    /// Whether the block implements `on_destroy`.
    pub implements_on_destroy: bool,
    /// The `#[order(n)]` value of `on_create`.
    pub on_create_order: i32,
    /// The `#[order(n)]` value of `on_start`.
    pub on_start_order: i32,
    /// The `#[order(n)]` value of `on_update`.
    pub on_update_order: i32,
    /// The `#[order(n)]` value of `on_stop`.
    pub on_stop_order: i32,
    /// The `#[order(n)]` value of `on_destroy`.
    pub on_destroy_order: i32,
    /// Whether the script runs while the editor edits as well as while it
    /// plays.
    pub run_in_edit_mode: bool,
}

impl ScriptDeclarationData {
    /// A declaration that defines no lifecycle method and runs only while
    /// playing.
    pub const NONE: Self = Self {
        implements_on_create: false,
        implements_on_start: false,
        implements_on_update: false,
        implements_on_stop: false,
        implements_on_destroy: false,
        on_create_order: 0,
        on_start_order: 0,
        on_update_order: 0,
        on_stop_order: 0,
        on_destroy_order: 0,
        run_in_edit_mode: false,
    };

    /// Declares an `on_create` that runs in `order`.
    #[must_use]
    pub const fn with_on_create(mut self, order: i32) -> Self {
        self.implements_on_create = true;
        self.on_create_order = order;
        self
    }

    /// Declares an `on_start` that runs in `order`.
    #[must_use]
    pub const fn with_on_start(mut self, order: i32) -> Self {
        self.implements_on_start = true;
        self.on_start_order = order;
        self
    }

    /// Declares an `on_update` that runs in `order`.
    #[must_use]
    pub const fn with_on_update(mut self, order: i32) -> Self {
        self.implements_on_update = true;
        self.on_update_order = order;
        self
    }

    /// Declares an `on_stop` that runs in `order`.
    #[must_use]
    pub const fn with_on_stop(mut self, order: i32) -> Self {
        self.implements_on_stop = true;
        self.on_stop_order = order;
        self
    }

    /// Declares an `on_destroy` that runs in `order`.
    #[must_use]
    pub const fn with_on_destroy(mut self, order: i32) -> Self {
        self.implements_on_destroy = true;
        self.on_destroy_order = order;
        self
    }

    /// Declares that the script also runs while the editor edits.
    #[must_use]
    pub const fn with_edit_mode(mut self) -> Self {
        self.run_in_edit_mode = true;
        self
    }
}

/// The declaration every script component carries.
///
/// `#[script]` implements this for the `impl ScriptComponent for ...` block it
/// is attached to, which is why a script component cannot be written without
/// it: [`ScriptComponent`] requires this trait, and nothing else implements it
/// for a type the attribute did not see.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` is a script component without a `#[script]` declaration",
    label = "add `#[script]` above this implementation",
    note = "the engine takes the lifecycle methods, their orders and the edit-mode \
            flag from that declaration, so a script cannot leave them implicit"
)]
pub trait ScriptDeclaration {
    /// Everything the `#[script]` attribute read from the implementation
    /// block.
    const DECLARATION: ScriptDeclarationData;
}

// =============================================================================
// ScriptComponent
// =============================================================================

/// A component the engine runs as a script, with a lifecycle method for each
/// step of its life.
///
/// Every lifecycle method has a no-op default, so an implementation defines
/// only the ones it needs, and each one is ordered by its own `#[order(n)]`:
///
/// ```ignore
/// #[derive(PillComponent)]
/// struct PlayerController {
///     speed: f32,
/// }
///
/// #[script]
/// impl ScriptComponent for PlayerController {
///     fn on_update(&mut self, script_context: &mut ScriptContext) {
///         // Read a component through the context.
///         if let Some(position) = script_context
///             .get_component::<Position>(script_context.get_owning_entity())
///         {
///             println!("player at {position:?}");
///         }
///
///         // Structural changes are only queued.
///         if self.speed > 100.0 {
///             script_context.destroy_entity(script_context.get_owning_entity());
///         }
///
///         self.speed += 1.0;
///     }
/// }
/// ```
///
/// The lifecycle methods mirror Unity's `MonoBehaviour` messages:
/// [`on_create`](ScriptComponent::on_create) is `Awake`,
/// [`on_start`](ScriptComponent::on_start) is `Start`,
/// [`on_update`](ScriptComponent::on_update) is `Update`,
/// [`on_stop`](ScriptComponent::on_stop) has no Unity equivalent, and
/// [`on_destroy`](ScriptComponent::on_destroy) is `OnDestroy`.
///
/// Every lifecycle method receives a [`ScriptContext`], which offers read
/// access to components and resources, a mutable borrow of a component, and
/// structural commands that are queued. Adding or removing a component and
/// destroying an entity all happen after every script in the frame has
/// returned.
///
/// A script component needs the `#[script]` attribute: without it the type
/// does not implement [`ScriptDeclaration`], and implementing this trait
/// therefore does not compile.
pub trait ScriptComponent: Component + ScriptDeclaration {
    /// Runs when the component starts existing.
    ///
    /// This is Unity's `Awake`. It fires when the component is added to an
    /// entity - by game code, by the editor, by a scene load, by an undo or by
    /// the stop of a play session - and once for every script in the world
    /// when play starts. [`CreateInfo::cause`] says which of those happened.
    ///
    /// It fires only where the script runs: always while playing, and while
    /// editing only for a script declared with `#[script(edit_mode)]`.
    fn on_create(&mut self, _script_context: &mut ScriptContext, _create_info: &CreateInfo) {}

    /// Runs once, right before the script's first
    /// [`on_update`](ScriptComponent::on_update).
    ///
    /// This is Unity's `Start`. It fires when play starts, for every script
    /// the world already held, and for a component added while the game is
    /// running. It never fires while the editor edits, and never again after a
    /// reload or a hot patch.
    fn on_start(&mut self, _script_context: &mut ScriptContext, _start_info: &StartInfo) {}

    /// Runs once per frame while the script is running.
    ///
    /// This is Unity's `Update`. It runs while the game plays, and while the
    /// editor edits only for a script declared with `#[script(edit_mode)]`.
    fn on_update(&mut self, _script_context: &mut ScriptContext) {}

    /// Runs when the script stops running, before
    /// [`on_destroy`](ScriptComponent::on_destroy).
    ///
    /// It fires when play stops, for every running script, and when a running
    /// script's component is removed while the game plays. Releasing external
    /// resources here is what keeps a stopped session from leaking them.
    fn on_stop(&mut self, _script_context: &mut ScriptContext, _stop_info: &StopInfo) {}

    /// Runs when the component stops existing.
    ///
    /// This is Unity's `OnDestroy`. It fires for a removal or a destroy the
    /// command queue applies - which is every removal game code, a script and
    /// the editor make - and for the running copy a play session discards when
    /// it stops. A direct [`crate::World::remove_component`] or
    /// [`crate::World::destroy_entity`] call is the tooling path and reports
    /// nothing, because there is no command queue to hand the lifecycle method.
    /// The component is still readable during the call.
    fn on_destroy(&mut self, _script_context: &mut ScriptContext, _destroy_info: &DestroyInfo) {}
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod find_first_tests {
    use super::*;
    use crate::world::World;

    /// The component the test looks for, on some entities and not others.
    #[derive(Debug, Clone, Copy)]
    struct Marker;

    impl Component for Marker {}

    /// A second type nothing carries, so the empty answer is covered.
    #[derive(Debug, Clone, Copy)]
    struct Absent;

    impl Component for Absent {}

    #[test]
    fn find_first_answers_the_first_entity_that_carries_the_type() {
        // Step 1: A world with two entities, the first one carrying the marker.
        let mut world = World::new();
        world.register_component::<Marker>();
        world.register_component::<Absent>();
        let first = world.create_entity().build().expect("the entity builds");
        world
            .add_component(first, Marker)
            .expect("the marker attaches");
        let second = world.create_entity().build().expect("the entity builds");
        world
            .add_component(second, Marker)
            .expect("the marker attaches");
        let mut commands = CommandQueue::new();

        // Step 2: The lookup answers the first carrier, nothing for a type
        // nobody carries, and stays the same when asked again.
        {
            let context = ScriptContext::new(&mut world, &mut commands, first);
            assert_eq!(context.find_first::<Marker>(), Some(first));
            assert_eq!(context.find_first::<Marker>(), Some(first));
            assert_eq!(context.find_first::<Absent>(), None);
        }

        // Step 3: Removing it from the first entity moves the answer on.
        world
            .remove_component::<Marker>(first)
            .expect("the marker detaches");
        let context = ScriptContext::new(&mut world, &mut commands, second);
        assert_eq!(context.find_first::<Marker>(), Some(second));
    }
}
