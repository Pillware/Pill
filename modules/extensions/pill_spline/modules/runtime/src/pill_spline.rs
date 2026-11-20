//! Extension providing spline paths.
//!
//! # Responsibilities
//!
//! - Defines the [`Spline`] component: an ordered set of control points, with
//!   one tangent per point for each side of it.
//! - Samples a position anywhere along that path with [`Spline::get_location_at`].
//! - Draws the curve, its points and its tangents through the engine's gizmo
//!   pass.
//! - Registers the component through the extension ABI when loaded.
//!
//! # Design
//!
//! The curve is a uniform Catmull-Rom spline while its [`TangentMode`] is
//! [`TangentMode::Auto`]. It passes exactly through every control point, which
//! is what makes a hand-placed path behave the way it looks in an editor. The
//! first and last control points also serve as the outer neighbours of the end
//! segments, so the curve starts and ends exactly on them instead of
//! overshooting past them.
//!
//! A spline whose mode is [`TangentMode::Aligned`] or [`TangentMode::Broken`]
//! samples from the tangents it stores instead, one in-tangent and one
//! out-tangent per point, as a cubic Hermite segment. `Aligned` keeps each
//! point's two tangents collinear and `Broken` lets them part; the curve maths
//! is the same for both, so a reader that only samples does not care which one
//! it is - an editor tool does, because it is the rule it keeps while dragging.
//! The stored tangents are always filled in by [`Spline::refit_tangents`], so
//! switching a spline back to `Auto` and then to a stored mode finds the same
//! curve it left.
//!
//! Control points live in a fixed-size array rather than a `Vec`. That keeps
//! the component plain data across the module boundary: one library never
//! allocates memory another has to free, and `#[repr(C)]` gives every copy of
//! the type the same field layout. A project that depends on the crate and the
//! module DLL the host loads hold separate copies of the type;
//! `shared` on [`Spline`] is what makes them one component.
//!
//! Registration lives in [`register`], a plain Rust function, so the crate can
//! also be linked statically.

// External crates
use pill_core::math::Vector3f;
use pill_core::telemetry::telemetry_target;
use pill_engine::*;
use serde::{Deserialize, Serialize};

// The build script scans this crate and emits one address entry per function
// into `function_inventory.rs`; the `include!` is what makes every function
// resolvable by qualified path with nothing in this file annotated.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of control points one spline can hold.
///
/// Fixed so the component stays plain data; see the module documentation.
pub const MAX_CONTROL_POINTS: usize = 16;

/// Number of demo splines [`register`] keeps in the world.
///
/// Used only by the registration path, which every build compiles.
const DEMO_SPLINE_COUNT: usize = 1;

/// Extra vertical offset added to every position sampled from a curve segment.
///
/// Compiled only when `test-hooks` is on. The hot-reload integration suites
/// change this one value in the source and watch the change reach a dependent
/// project's probe after a cascade reload. Nothing else in the module is
/// observable that way, because a module's component data survives a reload
/// and only its code is replaced. A default build has no such constant and
/// samples the plain curve.
#[cfg(feature = "test-hooks")]
const SAMPLE_VERTICAL_OFFSET: f32 = 0.0;

// =============================================================================
// Error Type
// =============================================================================

/// More control points than a [`Spline`] can hold.
///
/// Returned by [`Spline::try_from_points`]; it names both counts so a caller
/// can trim the path without reading [`MAX_CONTROL_POINTS`] first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyControlPoints {
    /// How many points the caller supplied.
    pub supplied: usize,
    /// How many a spline can hold ([`MAX_CONTROL_POINTS`]).
    pub capacity: usize,
}

impl std::fmt::Display for TooManyControlPoints {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} control points supplied but a spline holds at most {}",
            self.supplied, self.capacity
        )
    }
}

impl std::error::Error for TooManyControlPoints {}

// =============================================================================
// TangentMode
// =============================================================================

/// How a spline decides the tangents its segments leave and arrive with.
///
/// [`TangentMode::Auto`] derives them from the neighbouring control points on
/// every sample, which is the behaviour a spline had before it could store
/// tangents at all: a mode of `Auto` samples exactly what it always did.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TangentMode {
    /// Each tangent follows the line between its neighbours, as the
    /// Catmull-Rom curve does. Stored tangents are kept up to date but unused.
    #[default]
    Auto = 0,
    /// The stored tangents are the curve, and a point's two stay collinear.
    Aligned = 1,
    /// The stored tangents are the curve, and a point's two may part: a corner
    /// the curve does not round off.
    Broken = 2,
}

impl TangentMode {
    /// The mode a stored value names, defaulting to [`TangentMode::Auto`] for
    /// a value no mode was written as.
    pub fn from_stored(stored: u8) -> TangentMode {
        match stored {
            1 => TangentMode::Aligned,
            2 => TangentMode::Broken,
            _ => TangentMode::Auto,
        }
    }
}

// =============================================================================
// Component
// =============================================================================

/// A path through an ordered set of control points.
///
/// Only the first `control_point_count` entries of `control_points` are part of
/// the curve; the rest of the array is unused storage. A reload that changes
/// this component's layout migrates its rows by serializing them, so the struct
/// stays `#[repr(C)]` and every field stays serde compatible.
///
/// `shared` because a project that depends on this crate and the
/// module DLL the host loads hold two copies of the type, each with its own
/// `TypeId`. The declared name (`pill_spline::Spline`, the string
/// `std::any::type_name` reports) is what makes them one component with one
/// column; without it the engine would allocate two and keep whichever
/// registered first.
#[component(persistable, shared, gizmo = draw_spline_gizmo)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Spline {
    /// Control points the curve passes through, in order.
    pub control_points: [Vector3f; MAX_CONTROL_POINTS],
    /// How many leading entries of `control_points` are in use.
    pub control_point_count: u32,
    /// A scalar that only [`Self::default`] sets: no method here reads it, and
    /// the C# test project copies the value so a spline built in managed code
    /// matches the Rust default.
    pub elo: f32,
    /// The vector each active point arrives along, in the spline's own space.
    ///
    /// Read only while the mode is not [`TangentMode::Auto`];
    /// [`Self::refit_tangents`] keeps it filled in either way, so an edit made
    /// in `Auto` is not lost when the mode changes.
    pub in_tangents: [Vector3f; MAX_CONTROL_POINTS],
    /// The vector each active point leaves along, measured the same way.
    pub out_tangents: [Vector3f; MAX_CONTROL_POINTS],
    /// How the segments decide their tangents, as [`TangentMode`]'s value.
    ///
    /// Stored as the mode's own number rather than as the enum so the
    /// component stays a blittable row for every ABI that carries it; read it
    /// with [`Self::tangent_mode`] and write it with
    /// [`Self::set_tangent_mode`].
    pub tangent_mode: u8,
}

impl Default for Spline {
    /// An empty spline, which samples to the origin everywhere, with the
    /// tangents the neighbours give it.
    fn default() -> Self {
        Self {
            control_points: [Vector3f::ZERO; MAX_CONTROL_POINTS],
            control_point_count: 0,
            elo: 30.0,
            in_tangents: [Vector3f::ZERO; MAX_CONTROL_POINTS],
            out_tangents: [Vector3f::ZERO; MAX_CONTROL_POINTS],
            tangent_mode: TangentMode::Auto as u8,
        }
    }
}

impl Spline {
    /// Build a spline from control points, in order.
    ///
    /// Points beyond [`MAX_CONTROL_POINTS`] are ignored rather than treated as
    /// an error, so a caller assembling a path procedurally cannot fail here.
    /// Use [`Self::try_from_points`] when a truncated path would be worse than
    /// an error.
    pub fn from_points(points: &[Vector3f]) -> Self {
        let mut spline = Self::default();
        let used_control_point_count = points.len().min(MAX_CONTROL_POINTS);
        spline.control_points[..used_control_point_count]
            .copy_from_slice(&points[..used_control_point_count]);
        spline.control_point_count = used_control_point_count as u32;
        spline
    }

    /// Build a spline from control points, refusing more than the capacity.
    ///
    /// The checked counterpart of [`Self::from_points`], for callers who would
    /// rather hear about a too-long path than silently receive a truncated
    /// one. The capacity is checked before any copying, so a refused input
    /// leaves nothing half-built.
    ///
    /// # Errors
    ///
    /// Returns [`TooManyControlPoints`] naming both counts when `points` is
    /// longer than [`MAX_CONTROL_POINTS`].
    pub fn try_from_points(points: &[Vector3f]) -> Result<Self, TooManyControlPoints> {
        if points.len() > MAX_CONTROL_POINTS {
            return Err(TooManyControlPoints {
                supplied: points.len(),
                capacity: MAX_CONTROL_POINTS,
            });
        }
        Ok(Self::from_points(points))
    }

    /// The active control points, without the unused tail of the array.
    pub fn control_points(&self) -> &[Vector3f] {
        let used_control_point_count = (self.control_point_count as usize).min(MAX_CONTROL_POINTS);
        &self.control_points[..used_control_point_count]
    }

    /// Number of curve segments between consecutive control points.
    pub fn segment_count(&self) -> usize {
        self.control_points().len().saturating_sub(1)
    }

    /// Append one control point, ignoring it when the spline is already full.
    ///
    /// Returns whether the point was stored, so a caller that cares about the
    /// capacity limit can react to it.
    pub fn push_control_point(&mut self, point: Vector3f) -> bool {
        let used_control_point_count = self.control_point_count as usize;
        if used_control_point_count >= MAX_CONTROL_POINTS {
            return false;
        }
        self.control_points[used_control_point_count] = point;
        self.control_point_count += 1;
        true
    }

    /// Samples the position along the curve at `t`.
    ///
    /// `t` runs from 0.0 at the first control point to 1.0 at the last and is
    /// clamped into that range. It spreads evenly over the segments, not by arc
    /// length. Degenerate splines still answer sensibly: an empty spline samples
    /// to the origin, a single point samples to itself, and two points
    /// interpolate in a straight line.
    #[pill_hot_fn]
    pub fn get_location_at(&self, t: f32) -> Vector3f {
        let points = self.control_points();
        match points.len() {
            // An unset spline has no position to report; the origin is the
            // only answer that cannot be mistaken for real path data.
            0 => Vector3f::ZERO,
            1 => points[0],
            // Two points describe a straight line, where a Catmull-Rom segment
            // with duplicated neighbours would ease in and out instead.
            2 => points[0].lerp(points[1], t.clamp(0.0, 1.0)),
            _ => {
                // Map the global parameter onto one segment plus a local
                // parameter inside it. The final segment owns t == 1.0.
                let segment_count = points.len() - 1;
                let scaled_parameter = t.clamp(0.0, 1.0) * segment_count as f32;
                let segment_index = (scaled_parameter.floor() as usize).min(segment_count - 1);
                let local_t = scaled_parameter - segment_index as f32;

                let start = points[segment_index];
                let end = points[segment_index + 1];
                let sampled_position = match self.tangent_mode() {
                    TangentMode::Auto => {
                        // The outermost segments have no neighbour beyond the
                        // endpoint. Duplicating the endpoint makes the curve begin
                        // and end exactly on the first and last control points.
                        let before_start = if segment_index == 0 {
                            start
                        } else {
                            points[segment_index - 1]
                        };
                        let after_end = if segment_index + 2 < points.len() {
                            points[segment_index + 2]
                        } else {
                            end
                        };
                        catmull_rom(before_start, start, end, after_end, local_t)
                    }
                    TangentMode::Aligned | TangentMode::Broken => {
                        let (start_tangent, end_tangent) = self.segment_tangents(segment_index);
                        hermite(start, end, start_tangent, end_tangent, local_t)
                    }
                };
                // A `test-hooks` build shifts the sampled height so a code
                // change is observable through a cascade reload; a default
                // build computes only the curve.
                #[cfg(feature = "test-hooks")]
                let sampled_position = Vector3f::new(
                    sampled_position.x,
                    sampled_position.y + SAMPLE_VERTICAL_OFFSET,
                    sampled_position.z,
                );
                sampled_position
            }
        }
    }

    /// The tangents segment `segment_index` leaves and arrives with.
    ///
    /// Segment `i` runs from point `i` to point `i + 1`, so its start tangent
    /// is point `i`'s out-tangent and its end tangent is point `i + 1`'s
    /// in-tangent. A segment past the last one answers zeroes rather than
    /// panicking: a caller walking the segments reads [`Self::segment_count`]
    /// first, and a zero tangent draws a straight line instead of failing.
    pub fn segment_tangents(&self, segment_index: usize) -> (Vector3f, Vector3f) {
        let start_point = segment_index;
        let end_point = segment_index + 1;
        if end_point >= self.control_points().len() {
            return (Vector3f::ZERO, Vector3f::ZERO);
        }
        (self.out_tangents[start_point], self.in_tangents[end_point])
    }

    /// Fills the stored tangents from the control points, as a Catmull-Rom
    /// curve would derive them.
    ///
    /// Every point's tangent is the line through its two neighbours, halved,
    /// which is the slope the auto mode gives the curve there. The end points
    /// have only one neighbour, so their tangent points at it. An `Aligned`
    /// spline keeps the two halves equal, which is what it means for a point's
    /// tangents to stay collinear; a `Broken` spline keeps whatever was stored
    /// for the half a caller edited, so this function is what a spline calls
    /// after its points moved and the mode is `Auto`.
    pub fn refit_tangents(&mut self) {
        let points: Vec<Vector3f> = self.control_points().to_vec();
        let point_count = points.len();
        for (index, point) in points.iter().enumerate() {
            let before = if index == 0 {
                *point
            } else {
                points[index - 1]
            };
            let after = if index + 1 == point_count {
                *point
            } else {
                points[index + 1]
            };
            let tangent = (after - before) * 0.5;
            self.in_tangents[index] = tangent;
            self.out_tangents[index] = tangent;
        }
    }

    /// The mode this spline samples with.
    ///
    /// The stored byte is [`TangentMode`]'s own value; an unknown value reads
    /// as [`TangentMode::Auto`], which is the curve a spline had before it could
    /// store tangents.
    pub fn tangent_mode(&self) -> TangentMode {
        TangentMode::from_stored(self.tangent_mode)
    }

    /// Sets the mode this spline samples with.
    pub fn set_tangent_mode(&mut self, mode: TangentMode) {
        self.tangent_mode = mode as u8;
    }

    /// Moves one control point, answering whether the index named a live point.
    ///
    /// The stored tangents are deliberately left alone: a caller that moved a
    /// point in the auto mode calls [`Self::refit_tangents`] afterwards, and a
    /// caller editing tangents by hand keeps the ones it set.
    pub fn set_control_point(&mut self, index: usize, point: Vector3f) -> bool {
        if index >= self.control_points().len() {
            return false;
        }
        self.control_points[index] = point;
        true
    }

    /// Inserts a control point at `index`, answering the index it took.
    ///
    /// `index` past the last live point appends, so a caller assembling a path
    /// never has to clamp. `None` when the spline is full: the arrays are fixed
    /// so the component stays plain data.
    pub fn insert_control_point(&mut self, index: usize, point: Vector3f) -> Option<usize> {
        let used = self.control_points().len();
        if used >= MAX_CONTROL_POINTS {
            return None;
        }
        let insert_at = index.min(used);
        self.control_points
            .copy_within(insert_at..used, insert_at + 1);
        self.control_points[insert_at] = point;
        self.control_point_count = used as u32 + 1;
        Some(insert_at)
    }

    /// Removes one control point, shifting the later ones down, and answers
    /// whether the index named a live point.
    pub fn remove_control_point(&mut self, index: usize) -> bool {
        let used = self.control_points().len();
        if index >= used {
            return false;
        }
        self.control_points.copy_within(index + 1..used, index);
        self.control_point_count = used as u32 - 1;
        true
    }

    /// A dummy alpha channel that forwards straight to `pill_dummy_color`.
    ///
    /// A `test-hooks` build offsets it so a hot-patch suite can observe the
    /// changed body; a default build returns exactly what the colour module
    /// returns.
    #[doc(hidden)]
    #[pill_hot_fn]
    pub fn get_color_a(&self) -> f32 {
        let color = pill_dummy_color::get_color_a();
        #[cfg(feature = "test-hooks")]
        let color = color + 1450.0;
        color
    }
}

/// Mirrored entry points for managed code.
///
/// The mirror contract carries primitives only, so the [`Vector3f`] returned by
/// [`Spline::get_location_at`] crosses the ABI one axis at a time and a point is
/// written as an `x`/`y` pair. Managed code therefore never has to know the
/// curve's byte layout.
#[pill_mirror_impl]
impl Spline {
    /// `x` of [`Spline::get_location_at`], mirrored to C# as `GetLocationX`.
    #[pill_mirror_method]
    pub fn get_location_x(&self, t: f32) -> f32 {
        self.get_location_at(t).x
    }

    /// `y` of [`Spline::get_location_at`], mirrored to C# as `GetLocationY`.
    #[pill_mirror_method]
    pub fn get_location_y(&self, t: f32) -> f32 {
        self.get_location_at(t).y
    }

    /// Writes the `x`/`y` of one control point and zeroes its `z`, which is the
    /// plane the demo scene lives in; mirrored to C# as
    /// `SetControlPointLocation`.
    ///
    /// The write changes the component the managed call was made on, so the row
    /// is edited in place. `index` addresses the raw array, so a slot past the
    /// active points is storage the curve does not use yet. Returns `false` at
    /// or beyond [`MAX_CONTROL_POINTS`], so a caller sees the capacity limit
    /// instead of a silent no-op.
    #[pill_mirror_method]
    pub fn set_control_point_location(&mut self, index: u32, x: f32, y: f32) -> bool {
        let Some(control_point_slot) = self.control_points.get_mut(index as usize) else {
            return false;
        };
        *control_point_slot = Vector3f::new(x, y, 0.0);
        true
    }
}

// =============================================================================
// Value Types
// =============================================================================

// Compile-time layout of the vector type the control points are stored in.
//
// `Vector3f` is `glam::Vec3`, a foreign type that cannot carry
// `#[derive(PillMirror)]`. Without this declaration the managed mirror sees an
// opaque byte blob and C# has to write points by offset. The declared name
// must match the field's spelling, which tags it `array:struct:Vector3f`, so
// managed code gets typed `X`/`Y`/`Z` members. The macro reads the offsets
// from the compiler and fails the build if the declared fields no longer cover
// the type.
pill_engine::pill_value_type! {
    Vector3f {
        x: f32,
        y: f32,
        z: f32,
    }
}

/// Two coordinates and a handful of mirrored methods: the value type the C#
/// bridge suite calls through the mirror trampolines.
#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PillMirror)]
pub struct OmoMO {
    /// The first coordinate; the mirrored methods read it.
    pub x: u64,
    /// The second coordinate; the mirrored methods read it.
    pub y: u64,
}

#[pill_mirror_impl]
impl OmoMO {
    /// Sum of both coordinates; mirrored to C# as `GetSum()`.
    ///
    /// The `test-hooks` feature adds one so a suite can watch the mirrored
    /// value change; a default build returns the plain sum.
    #[doc(hidden)]
    #[pill_mirror_method]
    pub fn get_sum(&self) -> u64 {
        let sum = self.x + self.y;
        #[cfg(feature = "test-hooks")]
        let sum = sum + 1;
        sum
    }

    /// The `x` coordinate offset by 1200; mirrored to C# as `GetA()`.
    #[pill_mirror_method]
    pub fn get_a(&self) -> u64 {
        self.x + 1200
    }

    /// The `y` coordinate offset by 1200; mirrored to C# as `GetB()`.
    #[pill_mirror_method]
    pub fn get_b(&self) -> u64 {
        self.y + 1200
    }

    /// A constant; mirrored to C# as `GetC()` - a call that takes no state.
    #[pill_mirror_method]
    pub fn get_c(&self) -> u64 {
        666
    }

    /// Sum of two arguments; mirrored to C# as `GetD(int, int)` and used as
    /// the argument-conversion probe.
    #[pill_mirror_method]
    pub fn get_d(&self, alpha: i32, beta: i32) -> i32 {
        alpha + beta
    }
}

// =============================================================================
// Free Functions
// =============================================================================

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

// =============================================================================
// Gizmo
// =============================================================================

/// The colour the curve is drawn in.
const CURVE_COLOR: [f32; 4] = [0.95, 0.75, 0.2, 1.0];

/// The colour the control points are drawn in.
const POINT_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];

/// The colour the stored tangents are drawn in.
const TANGENT_COLOR: [f32; 4] = [0.35, 0.75, 0.95, 1.0];

/// How wide a control point is drawn, in pixels.
const POINT_SIZE_PIXELS: f32 = 7.0;

/// How many straight lines one curve segment is drawn with.
const SEGMENTS_PER_CURVE_SEGMENT: usize = 16;

/// Draws one spline: the curve, its control points and, when the spline stores
/// them, the tangents each point leaves and arrives along.
///
/// The engine's gizmo pass calls this once per entity that carries a
/// [`Spline`]; the points are in the spline's own space, so the entity's
/// transform turns them into the place they are drawn.
pub fn draw_spline_gizmo(spline: &Spline, context: &GizmoContext<'_>, draw: &mut GizmoDraw) {
    // Step 1: a spline with fewer than two points is a point, not a path.
    let point_count = spline.control_points().len();
    if point_count < 2 {
        return;
    }
    let to_world_matrix = context.to_world();
    let to_world = |point: Vector3f| {
        [
            to_world_matrix[0][0] * point.x
                + to_world_matrix[0][1] * point.y
                + to_world_matrix[0][2] * point.z
                + to_world_matrix[0][3],
            to_world_matrix[1][0] * point.x
                + to_world_matrix[1][1] * point.y
                + to_world_matrix[1][2] * point.z
                + to_world_matrix[1][3],
            to_world_matrix[2][0] * point.x
                + to_world_matrix[2][1] * point.y
                + to_world_matrix[2][2] * point.z
                + to_world_matrix[2][3],
        ]
    };

    // Step 2: the curve, sampled densely enough that a segment reads as a
    // curve rather than as a chain of straight lines.
    let sample_count = spline.segment_count() * SEGMENTS_PER_CURVE_SEGMENT;
    let curve: Vec<[f32; 3]> = (0..=sample_count)
        .map(|step| to_world(spline.get_location_at(step as f32 / sample_count as f32)))
        .collect();
    draw.polyline(&curve, CURVE_COLOR);

    // Step 3: one handle per control point, so a reader sees what the curve
    // passes through.
    for point in spline.control_points() {
        draw.point(to_world(*point), POINT_SIZE_PIXELS, POINT_COLOR);
    }

    // Step 4: the stored tangents, as the arms a segment leaves and arrives
    // along. An auto spline derives them per sample, so there is nothing stored
    // to draw.
    if spline.tangent_mode() == TangentMode::Auto {
        return;
    }
    for (index, point) in spline.control_points().iter().enumerate() {
        let arms = [
            *point - spline.in_tangents[index],
            *point + spline.out_tangents[index],
        ];
        for arm in arms {
            draw.line(to_world(*point), to_world(arm), TANGENT_COLOR);
        }
    }
}

/// A demonstration path used to populate the world on first load.
///
/// Like [`DEMO_SPLINE_COUNT`], this exists for the registration path, which
/// every build compiles.
fn demo_spline() -> Spline {
    Spline::from_points(&[
        Vector3f::new(0.0, 0.0, 0.0),
        Vector3f::new(100.0, 150.0, 0.0),
        Vector3f::new(300.0, -50.0, 0.0),
        Vector3f::new(500.0, 100.0, 0.0),
    ])
}

// =============================================================================
// Registration
// =============================================================================

/// Registers the [`Spline`] component with `world`, field layout included.
///
/// The derive writes `__pill_register_Spline` as a private item, so this is the
/// one path a crate that links the type uses to register it alone: a test
/// world, or an artifact that runs no module entry point.
pub fn register_component(world: &mut World) {
    __pill_register_Spline(world);
}

/// Registers the module's components against the host engine and fills the
/// world up to its spline count.
///
/// This is the half a reload repeats. It must be idempotent, because a first
/// load reaches it too, immediately before [`init`], and a failed generation
/// rolls back by calling it again on the older image.
///
/// The module registers no system: it contributes a component type and the math
/// to sample it, leaving movement along a path to whoever owns that behaviour.
///
/// # Errors
///
/// Returns 1 when an entity cannot be created.
pub fn register(engine: &mut Engine) -> u32 {
    // Fill up to the target count rather than spawning a new path on every
    // rebuild, because a reload keeps the entities already created - and fills
    // back up when a dropped column took them with it.
    let existing_spline_count = {
        let mut query = Query::<&Spline>::new(engine.world_mut());
        query.iter_mut().count()
    };
    for _ in existing_spline_count..DEMO_SPLINE_COUNT {
        if engine
            .world_mut()
            .create_entity()
            .with(demo_spline())
            .build()
            .is_err()
        {
            // Report the failure so the host keeps the previous generation
            // instead of running with a half-populated world.
            return 1;
        }
    }

    // NOTE: this line is asserted on. `MODULE_REGISTERED_MESSAGE` in
    // `devops/core/suite_common.py` matches the message text against the
    // host's stdout, and scenarios in `devops/tests/test_hot_reload_suite.py`
    // additionally require the `existing=` field - which is what proves a
    // reload preserved the entities the previous generation created rather
    // than spawning a fresh set. Reword either and those suites fail with
    // "Missing required token", which reads like a reload failure and is not.
    pill_core::info!(
        target: telemetry_target::ECS,
        splines = DEMO_SPLINE_COUNT,
        existing = existing_spline_count,
        max_control_points = MAX_CONTROL_POINTS,
        "pill_spline module registered"
    );
    0
}

/// The artifact's one-time entry point.
///
/// The module keeps nothing that has to run once: everything it owns is
/// registration, and [`register`] re-runs all of it on every generation. This
/// function exists because a statically linked build loads the module once and
/// calls this entry point alone, so it defers to [`register`].
///
/// Public so the generated `host_module_pill_spline` wrapper can carry it as
/// `pill_module_init` for the host to find in a loaded DLL.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for comparing sampled positions, generous enough for the
    /// accumulated error of a cubic evaluation but far below any real spacing.
    const EPSILON: f32 = 1e-4;

    /// Serializes the tests that go through the process-wide
    /// `pill_spline::get_color_a` redirect slot. One of them installs a
    /// replacement, and the harness runs tests on several threads, so a reader
    /// running at the same time would see the replacement's value.
    static COLOR_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Takes the slot lock, ignoring poisoning so one failing test does not
    /// fail every other slot test with it.
    fn lock_color_slot() -> std::sync::MutexGuard<'static, ()> {
        COLOR_SLOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// An empty spline has no path, so every sample reports the origin.
    #[test]
    fn empty_spline_samples_to_the_origin() {
        let spline = Spline::default();
        assert_eq!(spline.get_location_at(0.0), Vector3f::ZERO);
        assert_eq!(spline.get_location_at(0.5), Vector3f::ZERO);
        assert_eq!(spline.get_location_at(1.0), Vector3f::ZERO);
    }

    /// A single control point is the whole path, at every parameter value.
    #[test]
    fn single_point_spline_samples_to_that_point() {
        let point = Vector3f::new(3.0, -7.0, 11.0);
        let spline = Spline::from_points(&[point]);
        assert_eq!(spline.get_location_at(0.0), point);
        assert_eq!(spline.get_location_at(0.42), point);
        assert_eq!(spline.get_location_at(1.0), point);
    }

    /// Two control points interpolate in a straight line.
    #[test]
    fn two_point_spline_is_a_straight_line() {
        let start = Vector3f::new(0.0, 0.0, 0.0);
        let end = Vector3f::new(10.0, 20.0, -30.0);
        let spline = Spline::from_points(&[start, end]);

        assert!(spline.get_location_at(0.0).abs_diff_eq(start, EPSILON));
        assert!(spline
            .get_location_at(0.5)
            .abs_diff_eq(Vector3f::new(5.0, 10.0, -15.0), EPSILON));
        assert!(spline.get_location_at(1.0).abs_diff_eq(end, EPSILON));
    }

    /// A default build computes only the documented math: no sampled-height
    /// offset, no mirrored-sum shift, no colour passthrough constant.
    #[test]
    #[cfg(not(feature = "test-hooks"))]
    fn shipping_math_is_unshifted() {
        let _slot = lock_color_slot();
        let omo = OmoMO { x: 12, y: 34 };
        assert_eq!(omo.get_sum(), 12 + 34, "the mirrored sum is the plain sum");

        let spline = Spline::default();
        assert_eq!(
            spline.get_color_a(),
            pill_dummy_color::get_color_a(),
            "the colour passthrough adds nothing in a default build"
        );

        // The geometry the integration suites sample: five collinear points
        // 150 apart, the first at x=90. They all sit at y=120, so the midpoint
        // is the middle point.
        let points: [Vector3f; 5] =
            std::array::from_fn(|index| Vector3f::new(90.0 + 150.0 * index as f32, 120.0, 0.0));
        let midpoint = Spline::from_points(&points).get_location_at(0.5);
        assert!(
            midpoint.abs_diff_eq(Vector3f::new(390.0, 120.0, 0.0), EPSILON),
            "a default build samples the plain curve: {midpoint:?}"
        );
    }

    /// The hooks shift exactly the values the integration suites watch.
    #[test]
    #[cfg(feature = "test-hooks")]
    fn test_hooks_shift_the_observed_values() {
        let _slot = lock_color_slot();
        let omo = OmoMO { x: 12, y: 34 };
        assert_eq!(omo.get_sum(), 12 + 34 + 1);

        let spline = Spline::default();
        assert_eq!(
            spline.get_color_a(),
            pill_dummy_color::get_color_a() + 1450.0
        );

        let points: [Vector3f; 5] =
            std::array::from_fn(|index| Vector3f::new(90.0 + 150.0 * index as f32, 120.0, 0.0));
        let midpoint = Spline::from_points(&points).get_location_at(0.5);
        assert!(
            midpoint.abs_diff_eq(
                Vector3f::new(390.0, 120.0 + SAMPLE_VERTICAL_OFFSET, 0.0),
                EPSILON
            ),
            "the sampled midpoint carries the test offset: {midpoint:?}"
        );
    }

    /// A too-long path is refused by the checked constructor, with both counts.
    #[test]
    fn try_from_points_reports_overflow() {
        let point = Vector3f::new(1.0, 2.0, 3.0);

        let overflow = vec![point; MAX_CONTROL_POINTS + 1];
        let error = Spline::try_from_points(&overflow)
            .expect_err("more points than capacity must be refused");
        assert_eq!(error.supplied, MAX_CONTROL_POINTS + 1);
        assert_eq!(error.capacity, MAX_CONTROL_POINTS);
        assert!(
            error.to_string().contains("at most"),
            "the message names the capacity: {error}"
        );

        let full = vec![point; MAX_CONTROL_POINTS];
        let spline = Spline::try_from_points(&full).expect("exactly capacity fits");
        assert_eq!(spline.control_points().len(), MAX_CONTROL_POINTS);

        let partial = vec![point; 3];
        let spline = Spline::try_from_points(&partial).expect("three points fit");
        assert_eq!(spline.control_points().len(), 3);
    }

    /// The curve passes exactly through every control point, which is the
    /// property that makes a hand-placed path predictable.
    #[test]
    fn curve_passes_through_every_control_point() {
        let points = [
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(100.0, 150.0, 0.0),
            Vector3f::new(300.0, -50.0, 0.0),
            Vector3f::new(500.0, 100.0, 0.0),
        ];
        let spline = Spline::from_points(&points);

        // Four control points divide the parameter range into three segments.
        for (index, expected) in points.iter().enumerate() {
            let t = index as f32 / (points.len() - 1) as f32;
            assert!(
                spline.get_location_at(t).abs_diff_eq(*expected, EPSILON),
                "control point {index} at t={t} sampled as {:?}",
                spline.get_location_at(t)
            );
        }
    }

    /// Parameters outside the range clamp to the ends instead of extrapolating.
    #[test]
    fn parameter_is_clamped_to_the_path() {
        let points = [
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(10.0, 0.0, 0.0),
            Vector3f::new(20.0, 10.0, 0.0),
        ];
        let spline = Spline::from_points(&points);

        assert!(spline.get_location_at(-5.0).abs_diff_eq(points[0], EPSILON));
        assert!(spline.get_location_at(9.0).abs_diff_eq(points[2], EPSILON));
    }

    /// Sampling advances steadily along a straight path, so the parameter
    /// moves along the curve instead of jumping between segments.
    #[test]
    fn sampling_advances_along_the_path() {
        let spline = Spline::from_points(&[
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(10.0, 0.0, 0.0),
            Vector3f::new(20.0, 0.0, 0.0),
            Vector3f::new(30.0, 0.0, 0.0),
        ]);

        let mut previous_x = f32::NEG_INFINITY;
        for step in 0..=20 {
            let current_x = spline.get_location_at(step as f32 / 20.0).x;
            assert!(
                current_x > previous_x,
                "sample at step {step} moved backwards: {current_x} after {previous_x}"
            );
            previous_x = current_x;
        }
    }

    /// Building from more points than the capacity keeps the leading ones and
    /// drops the rest instead of failing.
    #[test]
    fn building_beyond_capacity_truncates() {
        let points: Vec<Vector3f> = (0..MAX_CONTROL_POINTS + 5)
            .map(|index| Vector3f::new(index as f32, 0.0, 0.0))
            .collect();
        let spline = Spline::from_points(&points);

        assert_eq!(spline.control_points().len(), MAX_CONTROL_POINTS);
        assert_eq!(spline.segment_count(), MAX_CONTROL_POINTS - 1);
        assert_eq!(spline.get_location_at(0.0).x, 0.0);
        assert_eq!(
            spline.get_location_at(1.0).x,
            (MAX_CONTROL_POINTS - 1) as f32
        );
    }

    /// A spline built the way a scene document saved before tangents existed
    /// describes it samples exactly the curve it did then.
    ///
    /// The four demo points are set field by field, with the tangent arrays and
    /// the mode left at the default a document without them loads into, and two
    /// mid-segment samples are compared against the Catmull-Rom values the old
    /// curve produced for them.
    #[test]
    fn a_spline_without_stored_tangents_samples_the_original_curve() {
        let mut spline = Spline::default();
        let points = [
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(100.0, 150.0, 0.0),
            Vector3f::new(300.0, -50.0, 0.0),
            Vector3f::new(500.0, 100.0, 0.0),
        ];
        for (index, point) in points.iter().enumerate() {
            spline.control_points[index] = *point;
        }
        spline.control_point_count = points.len() as u32;
        spline.elo = 30.0;

        assert_eq!(spline.tangent_mode(), TangentMode::Auto);
        assert_eq!(spline.in_tangents, [Vector3f::ZERO; MAX_CONTROL_POINTS]);
        assert_eq!(spline.out_tangents, [Vector3f::ZERO; MAX_CONTROL_POINTS]);

        // Two samples a hand-computed Catmull-Rom evaluation gives: the middle
        // of the second segment, then the middle of the third.
        let second_segment_middle = spline.get_location_at(1.5 / 3.0);
        assert!(
            second_segment_middle.abs_diff_eq(Vector3f::new(193.75, 50.0, 0.0), EPSILON),
            "the second segment's middle sampled as {second_segment_middle:?}"
        );
        let third_segment_middle = spline.get_location_at(2.5 / 3.0);
        assert!(
            third_segment_middle.abs_diff_eq(Vector3f::new(412.5, 12.5, 0.0), EPSILON),
            "the third segment's middle sampled as {third_segment_middle:?}"
        );
    }

    /// The tangents an auto spline derives are the ones the Catmull-Rom curve
    /// already has, so a stored-tangent segment starts and ends where the auto
    /// curve does.
    #[test]
    fn refitted_tangents_keep_the_curve_at_its_points() {
        let mut auto_spline = Spline::from_points(&[
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(100.0, 150.0, 0.0),
            Vector3f::new(300.0, -50.0, 0.0),
            Vector3f::new(500.0, 100.0, 0.0),
        ]);
        auto_spline.refit_tangents();
        let mut stored_spline = auto_spline;
        stored_spline.set_tangent_mode(TangentMode::Aligned);

        // A point's two tangents are the same vector, which is what collinear
        // means here.
        for index in 0..stored_spline.control_points().len() {
            assert_eq!(
                stored_spline.in_tangents[index], stored_spline.out_tangents[index],
                "point {index} arrived and left along different vectors"
            );
        }
        // Both modes pass through every control point, so no switch between
        // them can move a point the reader placed.
        for index in 0..4 {
            let t = index as f32 / 3.0;
            assert!(
                auto_spline
                    .get_location_at(t)
                    .abs_diff_eq(stored_spline.get_location_at(t), EPSILON),
                "control point {index} moved when its tangents were stored"
            );
        }
    }

    /// A stored tangent long enough to lift the curve does lift it, so a tool
    /// that writes tangents changes what the path looks like.
    #[test]
    fn a_longer_tangent_lifts_the_segment() {
        let mut spline = Spline::from_points(&[
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(100.0, 0.0, 0.0),
            Vector3f::new(200.0, 0.0, 0.0),
        ]);
        spline.refit_tangents();
        let flat = spline.get_location_at(0.25 * 0.5);
        spline.set_tangent_mode(TangentMode::Broken);
        spline.out_tangents[0] = Vector3f::new(100.0, 300.0, 0.0);
        let lifted = spline.get_location_at(0.25 * 0.5);

        assert!(
            lifted.y > flat.y + 1.0,
            "the longer tangent left the curve flat: {flat:?} became {lifted:?}"
        );
        assert!(
            lifted.abs_diff_eq(Vector3f::new(lifted.x, lifted.y, 0.0), EPSILON),
            "the curve left the plane its points are in: {lifted:?}"
        );
    }

    /// Inserting and removing control points shifts the later ones along, and
    /// both answer so a caller knows what happened.
    #[test]
    fn inserting_and_removing_control_points_shifts_the_path() {
        let mut spline = Spline::from_points(&[
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(10.0, 0.0, 0.0),
            Vector3f::new(20.0, 0.0, 0.0),
        ]);

        let inserted = spline.insert_control_point(1, Vector3f::new(5.0, 1.0, 0.0));
        assert_eq!(inserted, Some(1), "the point lands where it was asked to");
        assert_eq!(spline.control_points().len(), 4);
        assert_eq!(spline.control_points[1], Vector3f::new(5.0, 1.0, 0.0));
        assert_eq!(spline.control_points[3], Vector3f::new(20.0, 0.0, 0.0));

        assert!(spline.remove_control_point(1));
        assert_eq!(spline.control_points().len(), 3);
        assert_eq!(spline.control_points[1], Vector3f::new(10.0, 0.0, 0.0));
        assert!(
            !spline.remove_control_point(3),
            "a dead index removes nothing"
        );
    }

    /// A full spline refuses an insert instead of overwriting a point.
    #[test]
    fn inserting_into_a_full_spline_is_refused() {
        let mut spline = Spline::from_points(&[Vector3f::ZERO; MAX_CONTROL_POINTS]);
        assert_eq!(spline.insert_control_point(0, Vector3f::ONE), None);
        assert_eq!(spline.control_points().len(), MAX_CONTROL_POINTS);
    }

    /// Appending reports when the spline is full rather than silently dropping.
    #[test]
    fn pushing_reports_when_the_spline_is_full() {
        let mut spline = Spline::default();
        for index in 0..MAX_CONTROL_POINTS {
            assert!(spline.push_control_point(Vector3f::new(index as f32, 0.0, 0.0)));
        }
        assert!(!spline.push_control_point(Vector3f::ZERO));
        assert_eq!(spline.control_points().len(), MAX_CONTROL_POINTS);
    }

    /// The mirrored setter edits one point in place and pins the plane the
    /// scene works in.
    #[test]
    fn set_control_point_location_writes_x_y_and_zeroes_z() {
        let mut spline =
            Spline::from_points(&[Vector3f::new(1.0, 2.0, 3.0), Vector3f::new(4.0, 5.0, 6.0)]);

        assert!(spline.set_control_point_location(1, 10.0, 20.0));

        let point = spline.control_points[1];
        assert_eq!(point.x, 10.0);
        assert_eq!(point.y, 20.0);
        assert_eq!(point.z, 0.0);
        // Neighbouring storage stays untouched.
        assert_eq!(spline.control_points[0], Vector3f::new(1.0, 2.0, 3.0));
    }

    /// Out-of-range indices report the capacity limit instead of writing.
    #[test]
    fn set_control_point_location_rejects_out_of_range_index() {
        let mut spline = Spline::default();

        assert!(!spline.set_control_point_location(MAX_CONTROL_POINTS as u32, 1.0, 2.0));
        assert!(spline.set_control_point_location(MAX_CONTROL_POINTS as u32 - 1, 1.0, 2.0));
    }

    /// An inherent method carries a redirect slot just as a free function does,
    /// and callers of the public name reach an installed replacement.
    ///
    /// The install, the call and the reset share one test because the slot is
    /// process-wide and the harness runs tests on several threads: separate
    /// tests would see each other's installs in whatever order they ran. The
    /// slot lock keeps the other slot tests out while the replacement is
    /// installed.
    #[test]
    fn a_method_dispatches_installs_and_resets() {
        let _slot = lock_color_slot();
        // The receiver is the replacement's first argument, which is what makes
        // a method patchable through the same mechanism as a free function.
        fn replacement(_spline: &Spline) -> f32 {
            777.0
        }

        let spline = Spline::default();
        let original = spline.get_color_a();

        let signature =
            pill_engine::hot_patch::plain_function_signature("pill_spline::get_color_a")
                .expect("the method must be registered under its module path");

        pill_engine::hot_patch::install_plain_function(
            "pill_spline::get_color_a",
            replacement as *const () as usize,
            signature,
        )
        .expect("install with the recorded signature must be accepted");
        assert_eq!(
            spline.get_color_a(),
            777.0,
            "callers of the method must see the replacement"
        );

        pill_engine::hot_patch::reset_plain_function("pill_spline::get_color_a")
            .expect("reset must find the registered method");
        assert_eq!(
            spline.get_color_a(),
            original,
            "a reset must return the method to its own body"
        );
    }

    /// A reshaped method is refused, so a replacement can never be installed
    /// behind call sites compiled for the old receiver or return type.
    #[test]
    fn a_method_with_a_different_shape_is_refused() {
        fn replacement(_spline: &Spline) -> f32 {
            0.0
        }
        let result = pill_engine::hot_patch::install_plain_function(
            "pill_spline::get_color_a",
            replacement as *const () as usize,
            "(&Spline,)-> f64",
        );
        assert!(result.is_err(), "a changed signature must be refused");
    }

    /// The declaration reaches this artifact's registry, which is what the
    /// managed mirror codegen reads out of a loaded module.
    #[test]
    fn the_vector3f_declaration_reaches_the_registry() {
        let descriptor = pill_engine::component_registry::value_type_descriptors()
            .into_iter()
            .find(|descriptor| descriptor.type_name == "Vector3f")
            .expect("the Vector3f declaration must be in this artifact's registry");

        assert_eq!(descriptor.size, core::mem::size_of::<Vector3f>());
        assert_eq!(descriptor.align, core::mem::align_of::<Vector3f>());
        let fields: Vec<(&str, &str, usize)> = descriptor
            .fields
            .iter()
            .map(|field| (field.name, field.type_tag, field.offset))
            .collect();
        assert_eq!(fields, [("x", "f32", 0), ("y", "f32", 4), ("z", "f32", 8)]);
    }
}
