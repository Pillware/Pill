//! Where a listener's ears sit in the world.
//!
//! # Responsibilities
//!
//! - Turns a listener's position and facing into the two ear points rodio pans
//!   a positional sound between.
//!
//! # Design
//!
//! The geometry is a free function instead of a step inside the audio system,
//! so the tests need no audio device. A change in how a listener's placement is
//! stored also touches this one function. The facing is an angle in the XY
//! plane, so the ears sit either side of that direction in the same plane and
//! keep the listener's Z coordinate.

// Current crate
use crate::audio_manager::EAR_SEPARATION;

// =============================================================================
// Ear placement
// =============================================================================

/// Returns the two points where a listener's ears sit.
///
/// `listener_translation` is the listener's world position, and
/// `facing_degrees` is the direction it faces, in degrees counter-clockwise
/// from +X. Each ear sits `EAR_SEPARATION` away from the listener, on the line
/// at right angles to that direction. The returned pair is `(left, right)`,
/// the two points rodio pans a sound between, and both keep the listener's Z
/// coordinate.
pub fn ear_positions(listener_translation: [f32; 3], facing_degrees: f32) -> ([f32; 3], [f32; 3]) {
    let facing_radians = facing_degrees.to_radians();
    // Both ears sit on the line at right angles to the facing direction. A
    // quarter turn counter-clockwise from the facing vector points towards the
    // left ear; the right ear is the same distance on the other side.
    let (sin, cos) = facing_radians.sin_cos();
    let left_ear_position = [
        listener_translation[0] - sin * EAR_SEPARATION,
        listener_translation[1] + cos * EAR_SEPARATION,
        listener_translation[2],
    ];
    let right_ear_position = [
        listener_translation[0] + sin * EAR_SEPARATION,
        listener_translation[1] - cos * EAR_SEPARATION,
        listener_translation[2],
    ];
    (left_ear_position, right_ear_position)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for comparing ear positions in the tests, in world units.
    /// The geometry is a couple of trig calls, so the rounding error is far
    /// smaller than this.
    const EPSILON: f32 = 1e-5;

    /// Facing along +X puts the left ear at +Y and the right ear at -Y.
    #[test]
    fn ears_straddle_the_facing_direction() {
        let (left, right) = ear_positions([0.0, 0.0, 0.0], 0.0);
        assert!((left[0] - 0.0).abs() < EPSILON);
        assert!((left[1] - EAR_SEPARATION).abs() < EPSILON);
        assert!((right[0] - 0.0).abs() < EPSILON);
        assert!((right[1] + EAR_SEPARATION).abs() < EPSILON);
    }

    /// A quarter turn swaps which world axis the ears lie on.
    #[test]
    fn turning_the_listener_rotates_its_ears() {
        let (left, right) = ear_positions([0.0, 0.0, 0.0], 90.0);
        assert!((left[0] + EAR_SEPARATION).abs() < EPSILON);
        assert!((left[1] - 0.0).abs() < EPSILON);
        assert!((right[0] - EAR_SEPARATION).abs() < EPSILON);
        assert!((right[1] - 0.0).abs() < EPSILON);
    }

    /// Ears are placed relative to where the listener stands, and both keep
    /// its Z coordinate.
    #[test]
    fn ears_follow_the_listener_position() {
        let (left, right) = ear_positions([10.0, -5.0, 2.5], 0.0);
        assert!((left[0] - 10.0).abs() < EPSILON);
        assert!((left[1] - (-5.0 + EAR_SEPARATION)).abs() < EPSILON);
        assert!((right[0] - 10.0).abs() < EPSILON);
        assert!((right[1] - (-5.0 - EAR_SEPARATION)).abs() < EPSILON);
        assert!(
            (left[2] - 2.5).abs() < EPSILON,
            "the left ear keeps the height"
        );
        assert!(
            (right[2] - 2.5).abs() < EPSILON,
            "the right ear keeps the height"
        );
    }

    /// The two ears stay the same distance apart at every angle. rodio divides
    /// the difference between the two ear-to-emitter distances by that
    /// spacing, so the left-right balance keeps the same scale as the listener
    /// turns.
    #[test]
    fn ear_separation_is_preserved_under_rotation() {
        for degrees in [0.0, 37.0, 90.0, 180.0, 271.5] {
            let (left, right) = ear_positions([3.0, 4.0, 0.0], degrees);
            let span = ((left[0] - right[0]).powi(2) + (left[1] - right[1]).powi(2)).sqrt();
            assert!(
                (span - 2.0 * EAR_SEPARATION).abs() < EPSILON,
                "separation drifted at {degrees} degrees"
            );
        }
    }
}
