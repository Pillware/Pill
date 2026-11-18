//! Field decorators on `#[component]` and `#[derive(PillMirror)]`.
//!
//! # Responsibilities
//!
//! - Check that the decorators a component declares appear in its field
//!   layout in the order the field lists them.
//! - Check that a field which declares none gets an empty decorator list, so
//!   an undecorated field allocates nothing.
//! - Check that decorators declared on a nested value type's fields survive
//!   when the world turns that component field into dotted rows.

use pill_engine::component_registry::{
    register_all_components, ComponentFieldDescriptor, FieldDecorator,
};
use pill_engine::{component, ComponentId, PillMirror, World};

// =============================================================================
// Fixture Types
// =============================================================================

/// A component with one field for each decorator the tests below check, plus
/// one undecorated field, so every declared decorator can be read back from
/// its descriptor. The `color` and `vector` grouping decorators are covered by
/// the editor's inspector row tests instead.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Default)]
struct DecoratedProbe {
    /// No decorators, so its descriptor carries an empty decorator list.
    plain: f32,
    /// The Inspector leaves this row out, but the value is still stored and
    /// copied.
    #[pill(hide_in_inspector)]
    hidden: usize,
    /// A header line and a gap on the same field, in the order declared.
    #[pill(header("Timing"), space(10.0))]
    elapsed: f32,
    /// A tooltip and a slider range on one field, as a health value would use
    /// them.
    #[pill(tooltip("Current hit points"), range(0.0, 100.0))]
    health: f32,
    /// The parameterless text area, which opens at Unity's default height of
    /// three rows.
    #[pill(text_area)]
    notes: String,
    /// A text area that grows from 3 to 10 rows.
    #[pill(text_area(3, 10))]
    description: String,
    /// A fixed-height text box of five rows, declared with `multiline` instead
    /// of `text_area`.
    #[pill(multiline(5))]
    biography: String,
    /// Its drag value never goes below zero.
    #[pill(min(0.0))]
    ammo: i32,
}

/// A value type that declares its decorators on its own fields, so the
/// component that nests it does not repeat them.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PillMirror)]
struct DecoratedRange {
    /// Lower bound of the range. Its decorator shows on the dotted row.
    #[pill(range(0.0, 1.0))]
    start: f32,
    /// Upper bound, which also declares a minimum and a tooltip.
    #[pill(min(0.0), tooltip("Where the range ends"))]
    end: f32,
}

/// A component that nests the decorated value type.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct RangedCarrier {
    /// An ordinary scalar, so the dotted rows include one that is not nested.
    lead: f32,
    /// Expands into the `range.start` and `range.end` rows.
    range: DecoratedRange,
}

// =============================================================================
// Tests
// =============================================================================

/// A component's declared decorators appear in its field layout in the order
/// the field lists them, and a field that declares none gets an empty list.
#[test]
fn field_decorators_round_trip_through_the_layout() {
    let field_layout = DecoratedProbe::FIELD_LAYOUT;
    assert_eq!(field_layout.len(), 8);

    assert_eq!(field_layout[0].name, "plain");
    assert!(
        field_layout[0].decorators.is_empty(),
        "an undecorated field keeps an empty decorator slice"
    );

    assert_eq!(
        field_layout[1].decorators.as_ref(),
        &[FieldDecorator::HideInInspector]
    );

    // Header and gap are two separate decorators, so the Inspector can draw
    // the line and the space on their own.
    assert_eq!(
        field_layout[2].decorators.as_ref(),
        &[
            FieldDecorator::Header("Timing".into()),
            FieldDecorator::Space(10.0),
        ]
    );

    assert_eq!(
        field_layout[3].decorators.as_ref(),
        &[
            FieldDecorator::Tooltip("Current hit points".into()),
            FieldDecorator::Range {
                min: 0.0,
                max: 100.0,
            },
        ]
    );

    assert_eq!(
        field_layout[4].decorators.as_ref(),
        &[FieldDecorator::TextArea {
            min_lines: 3,
            max_lines: 3,
        }]
    );

    assert_eq!(
        field_layout[5].decorators.as_ref(),
        &[FieldDecorator::TextArea {
            min_lines: 3,
            max_lines: 10,
        }]
    );

    assert_eq!(
        field_layout[6].decorators.as_ref(),
        &[FieldDecorator::Multiline(5)]
    );

    assert_eq!(
        field_layout[7].decorators.as_ref(),
        &[FieldDecorator::Min(0.0)]
    );
}

/// A nested value type's decorated fields keep their decorators when the world
/// expands the struct field into dotted rows.
#[test]
fn a_decorated_value_type_field_keeps_its_decorators_on_dotted_rows() {
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");

    let field_layout = world
        .component_field_layout(ComponentId::of::<RangedCarrier>())
        .expect("the derive registers a field layout");
    let rows: Vec<(&str, &[FieldDecorator])> = field_layout
        .iter()
        .map(|field: &ComponentFieldDescriptor| (field.name, field.decorators.as_ref()))
        .collect();
    assert_eq!(
        rows,
        [
            ("lead", &[][..]),
            (
                "range.start",
                &[FieldDecorator::Range { min: 0.0, max: 1.0 }][..]
            ),
            (
                "range.end",
                &[
                    FieldDecorator::Min(0.0),
                    FieldDecorator::Tooltip("Where the range ends".into()),
                ][..],
            ),
        ]
    );
}
