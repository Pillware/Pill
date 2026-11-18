//! `#[derive(PillMirror)]` enum fields of a derived component.
//!
//! # Responsibilities
//!
//! - Show that a component registered through `register_all_components`
//!   reads a field of a mirrored fieldless enum as an enum value, with the
//!   variants the derive declared, and writes a picked variant into the row.
//! - Show that the facade's resolver reads an enum's declaration back from
//!   the mirror rows, explicit discriminants included.

// External crates
use pill_engine::component_registry::{register_all_components, resolve_enum_layout};
use pill_engine::{component, FieldValue, PillMirror, World};

/// A mirrored enum with an explicit discriminant after the first two.
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PillMirror)]
enum ProbeKind {
    #[default]
    Still,
    Moving,
    Spinning = 7,
}

/// A derived component carrying the enum beside a scalar.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct KindCarrier {
    speed: f32,
    kind: ProbeKind,
}

/// The resolver finds the enum by the name a field spells and returns every
/// variant with its discriminant.
#[test]
fn the_resolver_reads_an_enum_declaration_back() {
    let layout = resolve_enum_layout("ProbeKind").expect("the enum is declared");
    assert_eq!(layout.representation, "u8");
    assert_eq!(
        layout.variants,
        [("Still", 0), ("Moving", 1), ("Spinning", 7)]
    );
    assert!(resolve_enum_layout("KindCarrier").is_none());
}

/// Registered through the loop, the enum field reads as an enum and takes a
/// declared variant.
#[test]
fn an_enum_field_reads_and_writes_when_registered_through_the_loop() {
    // Step 1: Register every component this test binary declares and spawn
    // one carrier.
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");
    let entity = world
        .create_entity()
        .with(KindCarrier {
            speed: 2.0,
            kind: ProbeKind::Moving,
        })
        .build()
        .expect("entity builds");
    let component_name = world
        .entity_component_names(entity)
        .and_then(|names| names.into_iter().find(|name| name.ends_with("KindCarrier")))
        .expect("the entity carries KindCarrier");

    // Step 2: The field reads as the variant it holds.
    let read_value = world
        .read_component_field(entity, &component_name, "kind")
        .expect("the enum field reads");
    assert_eq!(read_value.enum_variant_name(), Some("Moving"));
    let FieldValue::Enum { type_tag, .. } = read_value else {
        panic!("expected an enum value, got {read_value:?}");
    };

    // Step 3: A declared variant writes into the row.
    world
        .write_component_field(
            entity,
            &component_name,
            "kind",
            FieldValue::Enum {
                type_tag,
                discriminant: 7,
            },
        )
        .expect("a declared variant writes");
    let stored = world
        .get_component::<KindCarrier>(entity)
        .expect("the component is still there")
        .kind;
    assert_eq!(stored, ProbeKind::Spinning);
}
