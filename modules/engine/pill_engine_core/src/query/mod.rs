//! Queries: iterating the entities that hold a given set of components.
//!
//! # Responsibilities
//!
//! - Defines the [`Query`] type as the primary entry point for component iteration.
//! - Provides [`QueryTarget`] for data shapes (`&T`, `&mut T`, `Entity`, tuples).
//! - Provides [`QueryFilter`] for predicates (`With`, `Without`, `Changed`, `Added`, `Or`).
//! - Implements sequential ([`QueryIterMut`]) and parallel ([`ParQueryIter`]) iterators.
//! - Re-exports [`Res`] / [`ResMut`] for resource access in systems.
//!
//! # Design
//!
//! A query has a target (the data it fetches for each row) and a filter (the
//! rows it leaves out). The target turns its component ids into one
//! [`ComponentMask`](crate::component::ComponentMask). The filter adds
//! `(include, exclude)` pairs of masks. An archetype passes the filter when it
//! holds every component in one pair's include mask and none in that pair's
//! exclude mask. The query yields an archetype only when it passes the target
//! mask and the filter. A filter with no restrictions adds no pairs, and then
//! the target mask alone decides.
//!
//! The target and the filter resolve an archetype's storage once and hand
//! worker threads raw pointers, so parallel iteration never repeats a lookup.
//!
//! [`query`] holds [`Query`] itself. The other modules are [`target`],
//! [`filter`], [`iter`], [`ptr`] (thread-safe raw-pointer wrappers),
//! [`resource`] ([`Res`] and [`ResMut`]), and [`change_detection`] (the `Mut`
//! smart pointer for tick tracking).
//!
//! # Examples
//!
//! ```no_run
//! # use pill_engine::*;
//! # #[derive(Debug, Clone)] struct Transform { x: f32, y: f32 }
//! # impl Component for Transform {}
//! # #[derive(Debug, Clone)] struct Velocity { x: f32, y: f32 }
//! # impl Component for Velocity {}
//! // Sequential iteration
//! fn movement_system(mut query: Query<(&mut Transform, &Velocity)>) {
//!     for (mut transform, velocity) in query.iter_mut() {
//!         transform.x += velocity.x * 0.016;
//!     }
//! }
//!
//! // Parallel iteration
//! fn physics_system(mut query: Query<(&mut Transform, &Velocity)>) {
//!     query.par_iter_mut().for_each(|(mut transform, velocity)| {
//!         transform.x += velocity.x * 0.016;
//!     });
//! }
//!
//! // Filter on change detection
//! fn react(mut query: Query<(Entity, &Transform), Changed<Transform>>) {
//!     for (entity, transform) in query.iter_mut() { /* ... */ }
//! }
//! ```

/// [`Mut`](change_detection::Mut) smart pointer with tick-based change
/// detection for components and resources.
pub(crate) mod change_detection;
mod filter;
mod iter;

/// Thread-safe raw-pointer wrappers shared by query targets and filters.
pub(crate) mod ptr;
#[allow(clippy::module_inception)]
mod query;
mod resource;
mod target;

// =============================================================================
// Re-exports
// =============================================================================

// Filter predicates and the query filter trait.
pub use filter::{Added, Changed, Or, QueryFilter, With, Without};

// Sequential and parallel query iterators, plus their batch statistics.
pub use iter::{BatchStats, ParQueryIter, QueryIterMut};

// Core query type and resource accessors.
pub use query::Query;
pub use resource::{Res, ResMut};
pub use target::{QueryTarget, ReadOnlyQueryTarget};

// =============================================================================
// Types
// =============================================================================

/// Cached per-archetype state for parallel iteration.
///
/// One record per matching archetype that holds at least one entity, built
/// once per `par_iter_mut` call. It keeps the target and filter state resolved
/// for that archetype, and the number of rows they were resolved against.
pub(crate) struct FilteredArchetypeRange<TargetState, FilterState> {
    /// Cached storage pointers this target resolved for the archetype.
    pub(crate) target_state: TargetState,
    /// Cached state the row filter needs for this archetype.
    pub(crate) filter_state: FilterState,
    /// Rows in the archetype when the states were built.
    pub(crate) entity_count: usize,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod query_tests;
