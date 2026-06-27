//! Handle module — ECS entity/resource handles (work in progress).
//!
//! This module is being refactored as part of the ECS rework (rework_ecs branch).
//! Currently contains minimal type stubs so the crate compiles.

// ---------------------------------------------------------------------------
// Placeholder types
// ---------------------------------------------------------------------------

/// ECS entity handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle {
    pub(crate) idx: u32,
    pub(crate) version: u32,
}

/// Generic resource pool (maps Handle → T).
#[derive(Debug)]
pub struct ResourcePool<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> ResourcePool<T> {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> Default for ResourcePool<T> {
    fn default() -> Self {
        Self::new()
    }
}
