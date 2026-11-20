//! Proves the field enum `#[component]` generates.
//!
//! # Responsibilities
//!
//! - Prove a derived component's `<Type>Field` enum names every field, in
//!   declaration order, and reports the same type tag its descriptor carries.
//! - Prove the `ComponentFields` association resolves through a generic
//!   function, and that a tuple struct still derives without one.
//!
//! # Design
//!
//! The enum is generated beside the component by the same walk that builds the
//! field descriptors, so the fixture reads both and compares them. The tuple
//! case pins that a type with no named fields keeps compiling.

// External crates
use pill_engine::{component, ComponentFieldId, ComponentFields};

/// A component covering the field shapes the enum has to name.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct FieldEnumProbe {
    /// A scalar field.
    speed: f32,
    /// A fixed-size array field.
    offset: [f32; 3],
    /// A whole-number field.
    count: u32,
}

/// A component with no named fields: the derive emits no enum for it.
#[component]
#[derive(Debug, Clone, Copy, Default)]
struct TupleProbe(f32);

/// Returns every field name a component's generated enum reports, through the
/// generic trait bound rather than the concrete enum.
fn field_names<T: ComponentFields>() -> Vec<&'static str> {
    T::Field::ALL.iter().map(|field| field.name()).collect()
}

/// The enum lists every field in declaration order, round-trips each name and
/// reports the descriptor's own type tag.
#[test]
fn the_field_enum_names_every_descriptor() {
    // Step 1: every enum variant corresponds to one descriptor, in order.
    let descriptor_names: Vec<&str> = FieldEnumProbe::FIELD_LAYOUT
        .iter()
        .map(|descriptor| descriptor.name)
        .collect();
    let enum_names: Vec<&str> = FieldEnumProbeField::ALL
        .iter()
        .map(|field| field.name())
        .collect();
    assert_eq!(enum_names, descriptor_names);

    // Step 2: a name maps back to the same variant.
    for field in FieldEnumProbeField::ALL {
        assert_eq!(FieldEnumProbeField::from_name(field.name()), Some(*field));
    }
    assert_eq!(FieldEnumProbeField::from_name("missing"), None);

    // Step 3: the tag beside each name is the descriptor's tag.
    for descriptor in FieldEnumProbe::FIELD_LAYOUT {
        let field = FieldEnumProbeField::from_name(descriptor.name)
            .expect("every descriptor is named by the enum");
        assert_eq!(field.value_tag(), descriptor.type_tag);
    }

    // Step 4: the generic association resolves to the same enum.
    assert_eq!(
        field_names::<FieldEnumProbe>(),
        vec!["speed", "offset", "count"]
    );
}

/// A component without named fields derives, and its layout is empty; the
/// derive generating no enum is what lets this file name no `<Type>Field` for
/// it.
#[test]
fn a_tuple_component_derives_without_a_field_enum() {
    let probe = TupleProbe(1.0);
    assert_eq!(probe.0, 1.0);
    assert!(TupleProbe::FIELD_LAYOUT.is_empty());
}
