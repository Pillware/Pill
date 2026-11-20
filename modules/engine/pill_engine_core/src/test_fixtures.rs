//! The component types the crate's unit tests share, and one helper that
//! registers them all.
//!
//! # Responsibilities
//!
//! - Define the shared test fixtures: [`Position`], [`Velocity`] and
//!   [`Health`].
//! - Offer [`setup_world`], a [`World`] with all three registered, for tests
//!   whose subject is not registration itself.
//!
//! # Design
//!
//! The world, query and commands tests build their worlds out of the same
//! shapes, so a fixture cannot drift between those suites. If one did drift,
//! a passing test would prove less than it appears to.
//!
//! `Health` is a tuple struct on purpose: the query tests write it as a
//! literal throughout, and a named field would add nothing a comment does not
//! already say.

// Current crate
use crate::component::Component;
use crate::world::World;

// =============================================================================
// Position
// =============================================================================

/// A 2D position, shared by the crate's test suites as a fixture.
///
/// The plain `f32` fields keep it minimal, so a test can focus on the system
/// it checks rather than on component behaviour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Position {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
}

impl Component for Position {}

// =============================================================================
// Velocity
// =============================================================================

/// A 2D velocity, shared by the crate's test suites as a fixture.
///
/// It is paired with [`Position`] in tests for multi-component queries, and
/// used on its own where a filter needs a second component to choose between.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Velocity {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
}

impl Component for Velocity {}

// =============================================================================
// Health
// =============================================================================

/// A health value, shared by the crate's test suites as a fixture.
///
/// It is a tuple struct, so an entity holding it goes into a different
/// archetype from one holding [`Position`] or [`Velocity`]. The filter tests
/// rely on that to get sets of entities that do not overlap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Health(pub i32);

impl Component for Health {}

// =============================================================================
// Free Functions
// =============================================================================

/// Creates a fresh [`World`] with the three fixture components registered.
///
/// The registration order is fixed, so every run gives each component the
/// same bit index (its slot in a world's component mask). A test that needs
/// other components registered should add them to the returned world rather
/// than writing its own version of this helper.
pub(crate) fn setup_world() -> World {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();
    world.register_component::<Health>();
    world
}
