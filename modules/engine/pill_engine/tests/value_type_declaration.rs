//! Declaring a foreign type's layout with `pill_value_type!`.
//!
//! # Responsibilities
//!
//! - Run the macro on two paths that name different modules but share one
//!   final type name, `probe::Vec3Probe` and `other::Vec3Probe`.
//! - Check that the two declarations are registered as separate entries, so
//!   the items the macro generates do not collide.
//!
//! The macro builds a temporary struct with the declared fields and takes
//! every offset from the compiler, so a path written with module segments has
//! to work as well as a single imported name.

// =============================================================================
// Stand-in Types
// =============================================================================

mod probe {
    /// A `repr(C)` stand-in for a foreign vector type.
    #[repr(C)]
    pub struct Vec3Probe {
        pub x: f32,
        pub y: f32,
        pub z: f32,
    }
}

mod other {
    /// A second stand-in, declared in another module but under the same type
    /// name.
    #[repr(C)]
    pub struct Vec3Probe {
        pub x: f32,
        pub y: f32,
    }
}

// =============================================================================
// Value Type Declarations
// =============================================================================

pill_engine::pill_value_type! {
    probe::Vec3Probe {
        x: f32,
        y: f32,
        z: f32,
    }
}

pill_engine::pill_value_type! {
    other::Vec3Probe {
        x: f32,
        y: f32,
    }
}

// =============================================================================
// Tests
// =============================================================================

/// Checks that each declaration is registered under the path it writes, with
/// the size, alignment and field offsets the compiler gives its struct.
#[test]
fn declared_types_are_registered_under_their_written_paths() {
    // Step 1: Construct each stand-in once so the compiler does not report the
    // structs as dead code.
    let _ = probe::Vec3Probe {
        x: 1.0,
        y: 2.0,
        z: 3.0,
    };
    let _ = other::Vec3Probe { x: 1.0, y: 2.0 };

    // Step 2: Check the first declaration's entry for the size, alignment and
    // field offsets of its struct.
    let descriptors = pill_engine::component_registry::value_type_descriptors();

    let probe_descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.type_name == "probe::Vec3Probe")
        .expect("probe::Vec3Probe must be registered");
    assert_eq!(probe_descriptor.size, 12);
    assert_eq!(probe_descriptor.align, 4);
    let probe_field_layouts: Vec<(&str, &str, usize)> = probe_descriptor
        .fields
        .iter()
        .map(|field| (field.name, field.type_tag, field.offset))
        .collect();
    assert_eq!(
        probe_field_layouts,
        [("x", "f32", 0), ("y", "f32", 4), ("z", "f32", 8)]
    );

    // Step 3: Check the second declaration has an entry of its own, under its
    // own path.
    let other_descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.type_name == "other::Vec3Probe")
        .expect("other::Vec3Probe must be registered");
    assert_eq!(other_descriptor.size, 8);
    assert_eq!(other_descriptor.align, 4);
    assert_eq!(other_descriptor.fields.len(), 2);
}
