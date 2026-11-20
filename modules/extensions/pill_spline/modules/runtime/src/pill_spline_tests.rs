//! Unit tests for the spline component, its sampling modes and its mirrors.
//!
//! # Responsibilities
//!
//! - Pin the sampling maths: every control point, both parameter ends, and the
//!   degenerate paths an empty or single-point spline takes.
//! - Pin the two tangent modes and the shared hot-patch dispatch an inherent
//!   method answers through.
//! - Pin that the managed mirror declaration reaches this artifact's registry.

// External crates
use pill_core::math::Vector3f;

// Current crate
use crate::component::{
    OmoMO, Spline, TangentMode, GET_LOCATION_AT_PATCH_NAME, MAX_CONTROL_POINTS,
};
use crate::math::catmull_rom;

/// Tolerance for comparing sampled positions, generous enough for the
/// accumulated error of a cubic evaluation but far below any real spacing.
const EPSILON: f32 = 1e-4;

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

/// A default build computes only the documented math: no mirrored-sum shift
/// and no sampled-height offset.
#[test]
#[cfg(not(feature = "test-hooks"))]
fn shipping_math_is_unshifted() {
    let omo = OmoMO { x: 12, y: 34 };
    assert_eq!(omo.get_sum(), 12 + 34, "the mirrored sum is the plain sum");

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
    let omo = OmoMO { x: 12, y: 34 };
    assert_eq!(omo.get_sum(), 12 + 34 + 1);

    let points: [Vector3f; 5] =
        std::array::from_fn(|index| Vector3f::new(90.0 + 150.0 * index as f32, 120.0, 0.0));
    let midpoint = Spline::from_points(&points).get_location_at(0.5);
    assert!(
        midpoint.abs_diff_eq(
            Vector3f::new(
                390.0,
                120.0 + crate::component::sample_vertical_offset(),
                0.0
            ),
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
    let error =
        Spline::try_from_points(&overflow).expect_err("more points than capacity must be refused");
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
/// tests would see each other's installs in whatever order they ran.
#[test]
fn a_method_dispatches_installs_and_resets() {
    // The receiver is the replacement's first argument, which is what makes
    // a method patchable through the same mechanism as a free function.
    fn replacement(_spline: &Spline, _t: f32) -> Vector3f {
        Vector3f::new(777.0, 0.0, 0.0)
    }

    let spline = Spline::from_points(&[Vector3f::ZERO, Vector3f::new(10.0, 0.0, 0.0)]);
    let original = spline.get_location_at(0.5);

    let signature = pill_engine::hot_patch::plain_function_signature(GET_LOCATION_AT_PATCH_NAME)
        .expect("the method must be registered under its module path");

    pill_engine::hot_patch::install_plain_function(
        GET_LOCATION_AT_PATCH_NAME,
        replacement as *const () as usize,
        signature,
    )
    .expect("install with the recorded signature must be accepted");
    assert_eq!(
        spline.get_location_at(0.5),
        Vector3f::new(777.0, 0.0, 0.0),
        "callers of the method must see the replacement"
    );

    pill_engine::hot_patch::reset_plain_function(GET_LOCATION_AT_PATCH_NAME)
        .expect("reset must find the registered method");
    assert_eq!(
        spline.get_location_at(0.5),
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
        GET_LOCATION_AT_PATCH_NAME,
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

/// The curve maths is reachable on its own, which is what the editor tool
/// samples with when it previews a drag.
#[test]
fn the_segment_math_matches_a_hand_evaluated_catmull_rom() {
    let before_start = Vector3f::new(-1.0, 0.0, 0.0);
    let start = Vector3f::new(0.0, 0.0, 0.0);
    let end = Vector3f::new(1.0, 0.0, 0.0);
    let after_end = Vector3f::new(2.0, 0.0, 0.0);

    assert!(catmull_rom(before_start, start, end, after_end, 0.0).abs_diff_eq(start, EPSILON));
    assert!(catmull_rom(before_start, start, end, after_end, 1.0).abs_diff_eq(end, EPSILON));
    assert!(catmull_rom(before_start, start, end, after_end, 0.5)
        .abs_diff_eq(Vector3f::new(0.5, 0.0, 0.0), EPSILON));
}
