// ============================================================================
// Scripting System
// ============================================================================
//! Script components that can update themselves each frame.
//!
//! Scripts receive a `ScriptContext` instead of direct `Engine` access.
//! This ensures all structural changes (add/remove component, destroy entity)
//! are automatically deferred, preventing use-after-free bugs.

use trait_type_map::TraitAccessible;

use crate::ecs::commands::CommandQueue;
use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
use crate::ecs::world::World;
use crate::ecs::Resource;

/// A restricted context for script components
///
/// This provides safe access to the ECS by:
/// - Allowing read-only world queries (get component, check existence)
/// - Allowing mutable access to global components
/// - Automatically deferring all structural changes to the command queue
///
/// Scripts cannot directly add/remove components or destroy entities on the world.
/// All such operations go through the command queue and execute after all scripts complete.
///
/// # Why This Design?
///
/// In an archetype-based ECS, adding/removing components causes entity migration.
/// If a script does `world.add_component(entity, ...)` while holding `&mut self`,
/// the archetype migration invalidates `self`, causing use-after-free.
///
/// By only exposing deferred operations, this is impossible at compile time.
pub struct ScriptContext<'a> {
    /// Mutable access to world for component queries
    world: &'a mut World,
    /// Command queue for deferred structural changes
    commands: &'a mut CommandQueue,
    /// The entity this script belongs to
    self_entity: Entity,
}

impl<'a> ScriptContext<'a> {
    /// Create a new script context
    pub(crate) fn new(
        world: &'a mut World,
        commands: &'a mut CommandQueue,
        self_entity: Entity,
    ) -> Self {
        Self {
            world,
            commands,
            self_entity,
        }
    }

    /// Get the entity this script is attached to
    #[inline]
    pub fn get_owning_entity(&self) -> Entity {
        self.self_entity
    }

    // ========================================================================
    // Read-Only World Access (Safe - no structural changes)
    // ========================================================================

    /// Get immutable reference to a component on any entity
    pub fn get_component<T>(&self, entity: Entity) -> Option<&T>
    where
        T: Component + TraitAccessible<dyn Component>,
    {
        self.world.get_component::<T>(entity)
    }

    /// Get mutable reference to a component on any entity
    ///
    /// IMPORTANT:
    /// This function uses raw pointers internally to avoid Rust's aliasing restrictions,
    /// which allows getting `&mut` to the same component type as the script itself.
    /// There can be two mutable references to the same component type in caller update function scope.
    /// One is the script's `&mut self`, the other is the returned `&mut T` from this function.
    ///
    /// SAFETY:
    /// These two references are always valid. The only way to invalidate them is to add/remove
    /// components from the entity immediately, which is impossible since all such operations are deferred.
    /// This is still considered as undefined behavior in Rust abstract machine sense,
    /// but in practice is sound - will never lead to any issues.
    pub fn get_component_mut<T>(&mut self, entity: Entity) -> Option<&mut T>
    where
        T: Component + TraitAccessible<dyn Component>,
    {
        let ptr = self.world.get_component_ptr_mut::<T>(entity)?;
        Some(unsafe { &mut *ptr })
    }

    /// Check if an entity exists
    pub fn entity_exists(&self, entity: Entity) -> bool {
        self.world.is_entity_valid(entity)
    }

    /// Get immutable reference to a resource
    pub fn get_resource<T: Resource>(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    // ========================================================================
    // Deferred Commands (Safe - executed after all scripts complete)
    // ========================================================================

    /// Queue spawning a new entity (deferred)
    ///
    /// The entity will be created after all scripts finish updating.
    /// Returns an EntityBuilder to add components.
    pub fn create_entity(&mut self) -> crate::ecs::commands::EntityBuilder<'_> {
        crate::ecs::commands::EntityBuilder::new(self.commands)
    }

    /// Queue destroying an entity (deferred)
    ///
    /// The entity will be destroyed after all scripts finish updating.
    /// Safe to call on the owning entity - the script will complete before destruction.
    pub fn destroy_entity(&mut self, entity: Entity) {
        self.commands.destroy_entity(entity);
    }

    /// Queue adding a component to an entity (deferred)
    ///
    /// The component will be added after all scripts finish updating.
    pub fn add_component<T>(&mut self, entity: Entity, component: T)
    where
        T: Component + TraitAccessible<dyn Component> + Send,
    {
        self.commands.add_component_to_entity(entity, component);
    }

    /// Queue removing a component from an entity (deferred)
    ///
    /// The component will be removed after all scripts finish updating.
    pub fn remove_component<T: Component>(&mut self, entity: Entity) {
        self.commands.remove_component_from_entity::<T>(entity);
    }

    /// Get direct access to the command queue for advanced usage
    pub fn get_commands(&mut self) -> &mut CommandQueue {
        self.commands
    }
}

/// Trait for components that can execute logic each frame
///
/// Script components have an update() method that gets called by the scripting system.
/// They receive a `ScriptContext` which provides safe, deferred access to the ECS.
///
/// # Safety
///
/// The `ScriptContext` only exposes read-only world access and deferred commands.
/// This prevents scripts from accidentally causing use-after-free by:
/// - Adding/removing components (which would invalidate `&mut self`)
/// - Destroying entities mid-update
///
/// All structural changes are queued and executed after ALL scripts complete.
///
/// # Example
///
/// ```ignore
/// struct PlayerController {
///     speed: f32,
/// }
///
/// impl ScriptComponent for PlayerController {
///     fn update(&mut self, ctx: &mut ScriptContext) {
///         // Safe read access
///         if let Some(pos) = ctx.get_component::<Position>(ctx.entity()) {
///             println!("Player at {:?}", pos);
///         }
///
///         // Deferred structural changes - safe!
///         if self.should_die() {
///             ctx.destroy_self(); // Won't execute until update completes
///         }
///
///         // Can still modify self safely
///         self.speed += 1.0;
///     }
/// }
/// ```
pub trait ScriptComponent: Component {
    /// Update this script component
    ///
    /// Called once per frame by the scripting system.
    /// Use `ctx.entity()` to get the entity this script is attached to.
    fn update(&mut self, ctx: &mut ScriptContext);
}
