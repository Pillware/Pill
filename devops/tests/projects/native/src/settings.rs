//! Scene counts, appearance, and physics tuning.
//!
//! # Responsibilities
//!
//! - Hold every tuning constant the project's systems read: the physics step,
//!   gravity, bounce speeds and restitution, the scene's floor and wall
//!   extents, and the two tints the materials take.

/// The step a fresh [`PhysicsState`](crate::PhysicsState) starts with, in
/// seconds; the per-frame system replaces it with the frame's real delta.
pub const FIXED_DELTA_TIME: f32 = 1.0 / 60.0;
/// Downward acceleration applied to every ball, in world units per second
/// squared.
pub const GRAVITY: f32 = 800.0;
/// Upward launch speed of a freshly spawned ball.
pub const BOUNCE_VELOCITY_Y: f32 = -800.0;
/// Horizontal travel speed of a freshly spawned ball; the direction
/// alternates with the ball's index.
pub const BOUNCE_VELOCITY_X: f32 = 350.0;
/// Share of the impact speed a wall or floor contact gives back.
pub const RESTITUTION: f32 = 0.6;

/// Upward speed restored when a floor bounce would otherwise decay to rest.
///
/// Keeps every ball visibly bouncing for the whole lifetime of the scene
/// instead of settling on the floor after a few seconds.
pub const MINIMUM_BOUNCE_VELOCITY_Y: f32 = 500.0;

pub const FLOOR_Y: f32 = 580.0;
pub const CEILING_Y: f32 = 20.0;
pub const LEFT_WALL: f32 = 20.0;
pub const RIGHT_WALL: f32 = 780.0;

/// Number of balls in the scene, and with it the number of control points the
/// spline is driven from.
///
/// Must stay at or below `pill_spline::MAX_CONTROL_POINTS`, the length of the
/// control point array a [`pill_spline::Spline`] stores.
pub const BALL_COUNT: usize = 5;

/// Curve parameter between two neighbouring sample dots.
pub const SPLINE_SAMPLE_STEP: f32 = 0.05;

/// Number of sample dots: one every [`SPLINE_SAMPLE_STEP`], from `t = 0.0`
/// through `t = 1.0`, so both endpoints of the curve get a dot of their own.
pub const SPLINE_SAMPLE_COUNT: usize = 21;

/// Edge length of a sample dot, in pixels.
pub const SPLINE_SAMPLE_DOT_SIZE: f32 = 6.0;

/// Fill colour of the ball meshes, as the material parameter's linear RGB.
pub const BALL_COLOR: [f32; 3] = [1.0, 0.3, 0.3];

/// Fill colour of the sample dots, as the material parameter's linear RGB.
pub const SAMPLE_DOT_COLOR: [f32; 3] = [0.25, 0.85, 1.0];

/// How much of the distance to its target the camera closes each frame.
///
/// Small enough that the camera drifts rather than snapping: the follow is a
/// visible behaviour the Inspector's reference row can be pointed at.
pub const FOLLOW_WEIGHT: f32 = 0.02;
