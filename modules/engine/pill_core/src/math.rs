//! Type aliases for `glam`'s vector and matrix types, a color alias, and an
//! enum that names directions.
//!
//! # Responsibilities
//!
//! - Give engine code one set of names for `glam`'s vector and matrix types,
//!   and for an RGB color.
//! - Define [`Direction`], named axes for transform and input code.
//!
//! # Design
//!
//! The file holds type aliases and one enum with no data, and `glam` is its
//! only dependency. The enum lets calling code name an axis in words instead
//! of passing a `glam` vector.

// External crates
use glam::{IVec2, Mat3, Mat3A, Mat4, Vec2, Vec3, Vec4};

// =============================================================================
// Vector, matrix, and color aliases
// =============================================================================

/// A 2D vector of `i32` components.
pub type Vector2i = IVec2;

/// A 2D vector of `f32` components.
pub type Vector2f = Vec2;
/// A 3D vector of `f32` components.
pub type Vector3f = Vec3;
/// A 4D vector of `f32` components.
pub type Vector4f = Vec4;

/// An RGB color with no alpha channel, stored as a [`Vector3f`].
pub type Color = Vec3;

/// A 3x3 matrix of `f32` values, stored as three `Vec3` columns. `col` and
/// `row` read one column or row.
pub type Matrix3f = Mat3;
/// A 3x3 matrix of `f32` values whose columns are `Vec3A` values. It does the
/// same maths as [`Matrix3f`], but it is 16-byte aligned and 48 bytes wide,
/// so the two memory layouts are not interchangeable.
pub type Matrix3fA = Mat3A;

/// A 4x4 matrix of `f32` values.
pub type Matrix4f = Mat4;

// =============================================================================
// Directions
// =============================================================================

/// A direction along one of the coordinate axes.
///
/// Variants without `World` name a direction on the object's own axes. The
/// `World` variants name a direction on the scene's axes. Nothing in the
/// workspace uses the enum yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Local forward.
    Forward,
    /// Local backward.
    Backward,
    /// Local right.
    Right,
    /// Local left.
    Left,
    /// Local up.
    Up,
    /// Local down.
    Down,
    /// World forward.
    WorldForward,
    /// World backward.
    WorldBackward,
    /// World right.
    WorldRight,
    /// World left.
    WorldLeft,
    /// World up.
    WorldUp,
    /// World down.
    WorldDown,
}
