//! Nested `#[derive(PillMirror)]` fields of a derived component.
//!
//! # Responsibilities
//!
//! - Show that a component registered through `register_all_components`
//!   expands a mirrored value type's field into rows named by path, such as
//!   `offset.x`. While that registration loop runs, the facade lends the
//!   engine core the value types this test binary declares.
//! - Show that the value types are lent only for that loop. When the loop
//!   ends, the world gets back the value-type resolver it had before, so a
//!   component registered outside the loop sees no value types.

// External crates
use pill_engine::component_registry::{register_all_components, ComponentFieldDescriptor};
use pill_engine::{component, Component, ComponentId, PillMirror, World};

/// A mirrored value type. The engine can expand its fields only while this
/// test binary's value types are installed as the world's resolver.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PillMirror)]
struct NestedVector {
    x: f32,
    y: f32,
}

/// A derived component with one plain scalar and one field of a mirrored value
/// type.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct NestedFieldCarrier {
    lead: f32,
    offset: NestedVector,
}

/// The derive's registration, run by `register_all_components`, expands the
/// value-type field into one row per leaf, at the offsets the leaves have
/// inside the value type.
#[test]
fn a_value_type_field_expands_when_registered_through_the_loop() {
    // Step 1: Register every component this test binary declares.
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");

    // Step 2: Read the layout back as name-and-offset pairs.
    let layout = world
        .component_field_layout(ComponentId::of::<NestedFieldCarrier>())
        .expect("the derive registers a field layout");
    let rows: Vec<(&str, usize)> = layout
        .iter()
        .map(|field| (field.name, field.offset))
        .collect();

    // Step 3: The nested field expands into one row per leaf, not one row for
    // the whole value.
    assert_eq!(rows, [("lead", 0), ("offset.x", 4), ("offset.y", 8)]);
}

/// A component registered by hand after the loop returns sees no value types.
/// The loop puts back the resolver the world had before it, and this world had
/// none.
#[test]
fn the_value_types_are_handed_over_only_for_the_loop() {
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct HandRegisteredComponent {
        offset: NestedVector,
    }
    impl Component for HandRegisteredComponent {}

    // The layout the hand registration is given. It declares one field whose
    // type tag names a mirrored value type.
    static HAND_REGISTERED_FIELD_LAYOUT: &[ComponentFieldDescriptor] =
        &[ComponentFieldDescriptor {
            name: "offset",
            type_tag: "struct:NestedVector",
            offset: 0,
            size: 8,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }];

    // Step 1: Run the registration loop, which puts the world's previous
    // resolver back when it finishes.
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");

    // Step 2: Register a second component by hand, outside the loop.
    world.register_component_with_layout::<HandRegisteredComponent>(HAND_REGISTERED_FIELD_LAYOUT);

    // Step 3: Its `struct:` field stays one opaque row, because no resolver is
    // installed to expand it.
    let layout = world
        .component_field_layout(ComponentId::of::<HandRegisteredComponent>())
        .expect("registered with a layout");
    assert_eq!(
        layout.len(),
        1,
        "the field stays one opaque row: {layout:?}"
    );
    assert_eq!(layout[0].type_tag, "struct:NestedVector");
}
