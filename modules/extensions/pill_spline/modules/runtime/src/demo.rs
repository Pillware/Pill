//! The demonstration path the module seeds when it loads.
//!
//! # Responsibilities
//!
//! - Define [`DEMO_SPLINE_COUNT`], the number of paths [`crate::register`]
//!   keeps in the world, and the points of the one it creates.
//!
//! The seed exists so a host started against any project shows a spline the
//! moment the module loads. Registration fills up to the target count rather
//! than spawning a new path on every generation, so a reload keeps the entities
//! already created.

// External crates
use pill_core::math::Vector3f;

// Current crate
use crate::component::Spline;

/// Number of demo splines [`crate::register`] keeps in the world.
///
/// Used only by the registration path, which every build compiles.
pub const DEMO_SPLINE_COUNT: usize = 1;

/// A demonstration path used to populate the world on first load.
///
/// Like [`DEMO_SPLINE_COUNT`], this exists for the registration path, which
/// every build compiles.
pub fn demo_spline() -> Spline {
    Spline::from_points(&[
        Vector3f::new(0.0, 0.0, 0.0),
        Vector3f::new(100.0, 150.0, 0.0),
        Vector3f::new(300.0, -50.0, 0.0),
        Vector3f::new(500.0, 100.0, 0.0),
    ])
}
