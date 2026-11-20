//! The two curve segments a spline samples from.
//!
//! # Responsibilities
//!
//! - Evaluate one Catmull-Rom segment ([`catmull_rom`]) and one cubic Hermite
//!   segment ([`hermite`]).
//!
//! Both functions are pure: they take points and a local parameter and return a
//! position, so the sampling modes in [`crate::component`] differ only in which
//! one they call.

// External crates
use pill_core::math::Vector3f;

/// Evaluate one Catmull-Rom segment between `start` and `end`.
///
/// `before_start` and `after_end` are the neighbouring control points that give
/// the segment its tangents; `local_t` runs from 0.0 at `start` to 1.0 at `end`.
pub fn catmull_rom(
    before_start: Vector3f,
    start: Vector3f,
    end: Vector3f,
    after_end: Vector3f,
    local_t: f32,
) -> Vector3f {
    // Standard uniform Catmull-Rom basis, written as a cubic in `local_t` so
    // each control point contributes one weighted term.
    let squared_t = local_t * local_t;
    let cubed_t = squared_t * local_t;

    let constant_term = start * 2.0;
    let linear_term = (end - before_start) * local_t;
    let quadratic_term = (before_start * 2.0 - start * 5.0 + end * 4.0 - after_end) * squared_t;
    let cubic_term = (start * 3.0 - before_start - end * 3.0 + after_end) * cubed_t;

    (constant_term + linear_term + quadratic_term + cubic_term) * 0.5
}

/// Evaluate one cubic Hermite segment from `start` to `end`.
///
/// `start_tangent` and `end_tangent` are the vectors the segment leaves and
/// arrives along, the same length unit as the points; `local_t` runs from 0.0
/// at `start` to 1.0 at `end`.
pub fn hermite(
    start: Vector3f,
    end: Vector3f,
    start_tangent: Vector3f,
    end_tangent: Vector3f,
    local_t: f32,
) -> Vector3f {
    // The four Hermite basis functions, written as weights on the two points
    // and the two tangents.
    let squared_t = local_t * local_t;
    let cubed_t = squared_t * local_t;
    let start_weight = 2.0 * cubed_t - 3.0 * squared_t + 1.0;
    let start_tangent_weight = cubed_t - 2.0 * squared_t + local_t;
    let end_weight = -2.0 * cubed_t + 3.0 * squared_t;
    let end_tangent_weight = cubed_t - squared_t;
    start * start_weight
        + start_tangent * start_tangent_weight
        + end * end_weight
        + end_tangent * end_tangent_weight
}
