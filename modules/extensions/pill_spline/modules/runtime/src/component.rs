//! The [`Spline`] component: its control points, its tangents and their maths.
//!
//! # Responsibilities
//!
//! - Define the [`Spline`] component and its fixed-capacity control-point
//!   arrays, so the type stays plain data across the module boundary.
//! - Sample a position anywhere along the path in
//!   [`Spline::get_location_at`].
//! - Keep the stored tangents in step with the control points
//!   ([`Spline::refit_tangents`]) and report capacity violations with
//!   [`TooManyControlPoints`].
//! - Mirror the sampling entry points managed code calls.

// External crates
use pill_core::math::Vector3f;
use pill_engine::*;
use serde::{Deserialize, Serialize};

// Current crate
use crate::math::{catmull_rom, hermite};

/// The patch-registry name of [`Spline::get_location_at`].
///
/// `pill_hot_scan` records a function under its `module_path!()`, so the name
/// follows the file the method lives in. Reading it from the compiler here is
/// what keeps a patch test from hard-coding a path that a module split moves.
pub const GET_LOCATION_AT_PATCH_NAME: &str = concat!(module_path!(), "::get_location_at");

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of control points one spline can hold.
///
/// Fixed so the component stays plain data; see the crate documentation.
pub const MAX_CONTROL_POINTS: usize = 16;

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

/// The offset a `test-hooks` build applies, for the unit test that checks it.
///
/// The constant itself stays a bare `const` line because the integration
/// suites rewrite exactly that line to bump the offset.
#[cfg(feature = "test-hooks")]
pub fn sample_vertical_offset() -> f32 {
    SAMPLE_VERTICAL_OFFSET
}

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
#[component(persistable, shared, gizmo = crate::gizmo::draw_spline_gizmo)]
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

impl std::default::Default for Spline {
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

/// Registers the [`Spline`] component with `world`, field layout included.
///
/// The derive writes `__pill_register_Spline` as a private item, so this is the
/// one path a crate that links the type uses to register it alone: a test
/// world, or an artifact that runs no module entry point.
pub fn register_component(world: &mut World) {
    __pill_register_Spline(world);
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
