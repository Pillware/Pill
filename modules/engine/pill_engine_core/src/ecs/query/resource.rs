//! [`Res`] and [`ResMut`] system parameters for resource access in systems.
//!
//! # Responsibilities
//!
//! - Provides [`Res<T>`] to read the world's one value of type `T` from a system.
//! - Provides [`ResMut<T>`] to read and write that value, with change tracking.
//!
//! # Design
//!
//! These types implement [`SystemParam`](crate::system::SystemParam), so a
//! system can take them as parameters. The scheduler records which resources
//! each system reads and writes, so two systems that would conflict on one
//! resource never run at the same time.

// Current crate
use crate::query::change_detection::Mut;
use crate::resource::Resource;
use crate::world::World;

// =============================================================================
// Res
// =============================================================================

/// Immutable resource access for systems.
///
/// Use `Res<T>` as a system parameter to read a resource without changing it.
/// The scheduler counts this as a read, so several systems can read the same
/// resource at the same time.
///
/// # Examples
/// ```no_run
/// # use pill_engine::*;
/// # #[derive(Debug)] struct ProjectTime { elapsed: f32 }
/// # impl Resource for ProjectTime {}
/// fn my_system(time: Res<ProjectTime>) {
///     if let Some(time) = time.get() {
///         println!("Elapsed: {}", time.elapsed);
///     }
/// }
/// ```
pub struct Res<'w, T: Resource> {
    /// The world the resource is fetched from.
    world: &'w World,
    /// Ties the wrapper to the resource type `T`; the value stays in the world.
    _resource_type: std::marker::PhantomData<T>,
}

impl<'w, T: Resource> Res<'w, T> {
    /// Creates a new [`Res`] wrapper around the given [`World`].
    ///
    /// The system runner builds this through
    /// [`SystemParam::fetch`](crate::system::SystemParam::fetch); calling it
    /// directly is not the intended path.
    pub fn new(world: &'w World) -> Self {
        Self {
            world,
            _resource_type: std::marker::PhantomData,
        }
    }

    /// Returns an immutable reference to the resource.
    ///
    /// Returns `None` if the resource has not been inserted into the World.
    pub fn get(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }
}

// =============================================================================
// ResMut
// =============================================================================

/// Mutable resource access for systems, with change tracking.
///
/// Use `ResMut<T>` as a system parameter to read and write a resource. The
/// scheduler counts this as a write, so no other system that touches the same
/// resource runs at the same time.
///
/// `get_mut()` returns a [`Mut<'_, T>`]. A write through it sets the
/// resource's `changed` tick to the current world tick, which is how other
/// systems see that the value changed.
///
/// # Examples
/// ```no_run
/// # use pill_engine::*;
/// # #[derive(Debug)] struct ProjectTime { elapsed: f32, delta: f32 }
/// # impl Resource for ProjectTime {}
/// fn my_system(mut time: ResMut<ProjectTime>) {
///     if let Some(mut time) = time.get_mut() {
///         time.elapsed += time.delta; // this write marks the resource as changed
///     }
/// }
/// ```
pub struct ResMut<'w, T: Resource> {
    /// The world the resource is fetched from.
    world: &'w mut World,
    /// Ties the wrapper to the resource type `T`; the value stays in the world.
    _resource_type: std::marker::PhantomData<T>,
}

impl<'w, T: Resource> ResMut<'w, T> {
    /// Creates a new [`ResMut`] wrapper around the given [`World`].
    ///
    /// The system runner builds this through
    /// [`SystemParam::fetch`](crate::system::SystemParam::fetch); calling it
    /// directly is not the intended path.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if another `ResMut` for the same resource is
    /// still alive, because one resource may only be borrowed mutably once.
    pub fn new(world: &'w mut World) -> Self {
        // Take the debug write lock here and release it on drop, not around
        // each `get_mut` call, so the lock covers the wrapper's whole life,
        // which is one system call. See `World::debug_acquire_resource_lock`.
        #[cfg(debug_assertions)]
        world.debug_acquire_resource_lock(crate::resource::ResourceId::of::<T>());
        Self {
            world,
            _resource_type: std::marker::PhantomData,
        }
    }

    /// Returns an immutable reference to the resource.
    ///
    /// Returns `None` if the resource has not been inserted into the World.
    pub fn get(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// Returns mutable, change-tracking access to the resource.
    ///
    /// The returned [`Mut<'_, T>`] holds both the value and its
    /// change-detection ticks. Writing through it (`DerefMut`) sets
    /// `ticks.changed` to the current world tick.
    ///
    /// Returns `None` if the resource has not been inserted into the World.
    pub fn get_mut(&mut self) -> Option<Mut<'_, T>> {
        self.world.get_resource_mut_tracked::<T>()
    }
}

/// Releases the debug write lock when a `ResMut` is dropped, which is what
/// keeps the lock limited to one system.
///
/// A system's parameters are fetched just before it runs and dropped when it
/// returns, so the lock is held exactly while a second holder would be a bug.
#[cfg(debug_assertions)]
impl<T: Resource> Drop for ResMut<'_, T> {
    fn drop(&mut self) {
        self.world
            .debug_release_resource_lock(crate::resource::ResourceId::of::<T>());
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, Resource};

    #[derive(Debug, Default)]
    struct Counter {
        count: u32,
    }
    impl Resource for Counter {}

    fn add_one(mut counter: ResMut<Counter>) {
        if let Some(mut counter) = counter.get_mut() {
            counter.count += 1;
        }
    }

    fn add_ten(mut counter: ResMut<Counter>) {
        if let Some(mut counter) = counter.get_mut() {
            counter.count += 10;
        }
    }

    /// Two systems writing one resource is an ordinary pattern. The scheduler
    /// sees the conflict and runs them in separate batches, one after the
    /// other. The debug write lock must cover one system, not the whole frame,
    /// or the second system would panic on a concurrency check that is not
    /// actually failing.
    #[test]
    fn two_systems_may_write_one_resource_in_the_same_frame() {
        let mut engine = Engine::new();
        engine.world_mut().insert_resource(Counter::default());
        engine.register_system("add_one", add_one);
        engine.register_system("add_ten", add_ten);

        engine.process_frame().unwrap();

        assert_eq!(
            engine.world().get_resource::<Counter>().unwrap().count,
            11,
            "both systems must have run"
        );
    }

    /// The same system taking the resource every frame is fine too. Each frame
    /// builds a new `ResMut`, and the lock is released when it drops.
    #[test]
    fn a_writer_may_run_every_frame() {
        let mut engine = Engine::new();
        engine.world_mut().insert_resource(Counter::default());
        engine.register_system("add_one", add_one);

        for _ in 0..3 {
            engine.process_frame().unwrap();
        }
        assert_eq!(engine.world().get_resource::<Counter>().unwrap().count, 3);
    }

    /// One system taking the resource mutably more than once, in sequence, is
    /// ordinary code. The lock is held per wrapper, not per `get_mut()` call,
    /// so this must not look like overlapping access.
    #[test]
    fn one_system_may_take_the_resource_twice() {
        fn writes_twice(mut counter: ResMut<Counter>) {
            {
                let mut first_write = counter.get_mut().expect("resource is present");
                first_write.count += 1;
            }
            let mut second_write = counter.get_mut().expect("resource is present");
            second_write.count += 1;
        }

        let mut engine = Engine::new();
        engine.world_mut().insert_resource(Counter::default());
        engine.register_system("writes_twice", writes_twice);
        engine.process_frame().unwrap();

        assert_eq!(engine.world().get_resource::<Counter>().unwrap().count, 2);
    }

    /// Releasing on drop must not weaken the check: two `ResMut` wrappers alive
    /// at once over one resource is exactly what it catches, and still panics.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "already mutably borrowed")]
    fn two_live_guards_over_one_resource_still_assert() {
        let mut world = crate::World::new();
        world.insert_resource(Counter::default());

        let live_write_guard = ResMut::<Counter>::new(&mut world);
        let world_pointer: *mut crate::World = live_write_guard.world;
        // SAFETY: the alias is deliberate and is exactly what the test is
        // about: a second `ResMut` built while the first is alive. The pointer
        // comes from a live borrow, and only this thread touches the world.
        // Neither wrapper is used to read or write, so the two `&mut World`
        // never access memory at the same time.
        let _second = ResMut::<Counter>::new(unsafe { &mut *world_pointer });
    }

    /// A lock released by one wrapper can be taken again by the next, which is
    /// what lets two systems write one resource in the same frame.
    #[test]
    fn a_released_lock_can_be_retaken() {
        let mut world = crate::World::new();
        world.insert_resource(Counter::default());

        for _ in 0..3 {
            let mut resource_write_guard = ResMut::<Counter>::new(&mut world);
            resource_write_guard
                .get_mut()
                .expect("resource is present")
                .count += 1;
        }
        assert_eq!(world.get_resource::<Counter>().unwrap().count, 3);
    }
}
