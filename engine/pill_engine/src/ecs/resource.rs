// ============================================================================
// Resource System
// ============================================================================
//! Resources are singleton data stored in the World, not attached to entities.
//!
//! Resources represent global/shared state such as time, input, configuration,
//! asset stores, etc. They can be accessed by systems through `Res<T>` (immutable)
//! and `ResMut<T>` (mutable) system parameters.
//!
//! ## Resource Handles
//!
//! `ResHandle<T>` provides a lightweight, typed reference to a resource that can
//! be stored and passed around without borrowing the World. Handles can be used
//! to retrieve the resource later from the World.
//!
//! ## Usage
//!
//! ```ignore
//! // Define a resource
//! #[derive(Debug)]
//! struct GameTime { delta: f32, elapsed: f32 }
//! impl Resource for GameTime {}
//!
//! // Insert into world
//! world.insert_resource(GameTime { delta: 0.016, elapsed: 0.0 });
//!
//! // Get a handle (cheap, copyable)
//! let handle = ResHandle::<GameTime>::new();
//!
//! // Use handle to access the resource later
//! let time = handle.get(&world).unwrap();
//!
//! // Use in systems via Res/ResMut
//! fn my_system(time: Res<GameTime>) {
//!     println!("Elapsed: {}", time.get().unwrap().elapsed);
//! }
//! ```

use std::any::TypeId;
use std::marker::PhantomData;

use crate::ecs::world::World;

/// Resource marker trait - resources are singleton data stored in the World.
///
/// Unlike components, resources are not attached to entities. They represent
/// global/shared state such as time, input, configuration, etc.
///
/// Resources must be Send + Sync to support parallel system access.
pub trait Resource: Send + Sync + 'static {}

/// ResourceId uniquely identifies a resource type using its TypeId
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub TypeId);

impl ResourceId {
    pub fn of<T: Resource>() -> Self {
        ResourceId(TypeId::of::<T>())
    }
}

/// A lightweight, typed handle to a resource in the World.
///
/// `ResHandle<T>` is a zero-cost abstraction that stores the resource's type
/// information. Handles are `Copy`, `Clone`, `Send`, and `Sync`, making them
/// easy to store and pass around without borrowing the World.
///
/// Use handles when you need to:
/// - Store a reference to a resource type for later access
/// - Pass resource type information between systems or phases
/// - Defer resource access to a later point
///
/// # Example
/// ```ignore
/// #[derive(Debug)]
/// struct Score(u32);
/// impl Resource for Score {}
///
/// // Create a handle
/// let handle = ResHandle::<Score>::new();
///
/// // Insert the resource
/// world.insert_resource(Score(0));
///
/// // Use the handle to access the resource
/// let score = handle.get(&world).unwrap();
/// assert_eq!(score.0, 0);
///
/// // Mutably access via handle
/// let score = handle.get_mut(&mut world).unwrap();
/// score.0 += 10;
/// ```
pub struct ResHandle<T: Resource> {
    _phantom: PhantomData<T>,
}

impl<T: Resource> ResHandle<T> {
    /// Create a new handle for a resource type.
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }

    /// Get the `ResourceId` for this handle's resource type.
    pub fn id(&self) -> ResourceId {
        ResourceId::of::<T>()
    }

    /// Get an immutable reference to the resource from the World.
    ///
    /// Returns `None` if the resource has not been inserted.
    pub fn get<'w>(&self, world: &'w World) -> Option<&'w T> {
        world.get_resource::<T>()
    }

    /// Get a mutable reference to the resource from the World.
    ///
    /// Returns `None` if the resource has not been inserted.
    pub fn get_mut<'w>(&self, world: &'w mut World) -> Option<&'w mut T> {
        world.get_resource_mut::<T>()
    }

    /// Check if the resource exists in the World.
    pub fn exists(&self, world: &World) -> bool {
        world.has_resource::<T>()
    }
}

impl<T: Resource> Default for ResHandle<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Resource> Clone for ResHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Resource> Copy for ResHandle<T> {}

// SAFETY: ResHandle contains only PhantomData and carries no actual data.
// It is purely a type-level marker.
unsafe impl<T: Resource> Send for ResHandle<T> {}
unsafe impl<T: Resource> Sync for ResHandle<T> {}

impl<T: Resource> std::fmt::Debug for ResHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ResHandle<{}>", std::any::type_name::<T>())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Score(u32);
    impl Resource for Score {}

    #[derive(Debug, PartialEq)]
    struct GameTime {
        delta: f32,
        elapsed: f32,
    }
    impl Resource for GameTime {}

    #[test]
    fn test_resource_id() {
        let id1 = ResourceId::of::<Score>();
        let id2 = ResourceId::of::<Score>();
        let id3 = ResourceId::of::<GameTime>();

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_res_handle_new() {
        let handle = ResHandle::<Score>::new();
        assert_eq!(handle.id(), ResourceId::of::<Score>());
    }

    #[test]
    fn test_res_handle_default() {
        let handle = ResHandle::<Score>::default();
        assert_eq!(handle.id(), ResourceId::of::<Score>());
    }

    #[test]
    fn test_res_handle_copy_clone() {
        let handle = ResHandle::<Score>::new();
        let handle2 = handle;
        let handle3 = handle.clone();
        // All handles refer to the same resource type
        assert_eq!(handle.id(), handle2.id());
        assert_eq!(handle.id(), handle3.id());
    }

    #[test]
    fn test_res_handle_debug() {
        let handle = ResHandle::<Score>::new();
        let debug_str = format!("{:?}", handle);
        assert!(debug_str.contains("ResHandle"));
        assert!(debug_str.contains("Score"));
    }

    #[test]
    fn test_res_handle_get() {
        let mut world = World::new();
        let handle = ResHandle::<Score>::new();

        // Resource doesn't exist yet
        assert!(!handle.exists(&world));
        assert!(handle.get(&world).is_none());

        // Insert resource
        world.insert_resource(Score(42));

        // Now accessible via handle
        assert!(handle.exists(&world));
        assert_eq!(handle.get(&world).unwrap().0, 42);
    }

    #[test]
    fn test_res_handle_get_mut() {
        let mut world = World::new();
        let handle = ResHandle::<Score>::new();

        world.insert_resource(Score(0));

        // Mutate via handle
        handle.get_mut(&mut world).unwrap().0 += 10;
        assert_eq!(handle.get(&world).unwrap().0, 10);

        handle.get_mut(&mut world).unwrap().0 += 5;
        assert_eq!(handle.get(&world).unwrap().0, 15);
    }

    #[test]
    fn test_res_handle_missing_resource() {
        let world = World::new();
        let handle = ResHandle::<Score>::new();

        assert!(!handle.exists(&world));
        assert!(handle.get(&world).is_none());
    }

    #[test]
    fn test_multiple_handles_different_types() {
        let mut world = World::new();

        let score_handle = ResHandle::<Score>::new();
        let time_handle = ResHandle::<GameTime>::new();

        world.insert_resource(Score(100));
        world.insert_resource(GameTime {
            delta: 0.016,
            elapsed: 0.0,
        });

        assert_eq!(score_handle.get(&world).unwrap().0, 100);
        assert_eq!(time_handle.get(&world).unwrap().delta, 0.016);

        // Different types, different IDs
        assert_ne!(score_handle.id(), time_handle.id());
    }
}
