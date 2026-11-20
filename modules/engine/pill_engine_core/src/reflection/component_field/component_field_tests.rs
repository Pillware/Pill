//! Unit tests for the component field layer. They cover the field value
//! vocabulary, the scalar codec, the heap-container reads, the field reads and
//! writes, and the layout expansion done when a component registers.
//!
//! # Responsibilities
//!
//! - Cover the field entry points of [`World`] through registered test
//!   components, so a change to the codec or the row layer fails here rather
//!   than in a caller.
//! - Check the layouts the tests declare: each probe type is `repr(C)`, and
//!   the hand-written [`ComponentFieldDescriptor`] table beside it gives the
//!   same offsets, so a reordered struct fails instead of passing by accident.
//!
//! # Design
//!
//! The tests reach the module's items through `use super::*`. A probe
//! component that needs a field layout is registered with a hand-written
//! descriptor table. That table repeats the offsets of the struct's `repr(C)`
//! layout, so the tests check the reader against offsets written out in the
//! test rather than against what the derive would generate.

// Current crate
use super::codec::{decode_scalar, encode_scalar, is_scalar_tag, scalar_size};
use super::*;
use crate::component::{Component, ComponentId};
use crate::component_registry::ComponentFieldDescriptor;
use crate::entity::Entity;
use crate::world::World;

// =============================================================================
// The typed accessors
// =============================================================================

/// Each accessor answers its own variant and `None` for another, which is what
/// lets a callback read a value without matching the variant itself.
#[test]
fn the_field_value_accessors_answer_their_own_variant_only() {
    let float = FieldValue::F32(1.5);
    assert_eq!(float.as_f32(), Some(1.5));
    assert_eq!(float.as_u32(), None);
    assert_eq!(float.as_bool(), None);
    assert!(float.as_array().is_none());

    let integer = FieldValue::U32(7);
    assert_eq!(integer.as_u32(), Some(7));
    assert_eq!(integer.as_f32(), None);

    let flag = FieldValue::Bool(true);
    assert_eq!(flag.as_bool(), Some(true));
    assert_eq!(flag.as_u32(), None);

    let array = FieldValue::Array(vec![FieldValue::U32(2), FieldValue::U32(3)]);
    let elements = array.as_array().expect("an array field reads back");
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[1].as_u32(), Some(3));
    assert_eq!(array.as_f32(), None);
}

// =============================================================================
// Probe components and their declared layouts
// =============================================================================

/// `repr(C)` keeps the fields in the order they are declared. A plain
/// `repr(Rust)` struct may reorder them, and the hand-written layout below
/// would then describe offsets the struct does not have.
#[repr(C)]
#[derive(Debug, Clone, PartialEq)]
struct SampleComponent {
    a_float: f32,
    an_int: u32,
    a_bool: bool,
    an_array: [i16; 3],
}
impl Component for SampleComponent {}

/// A second registered type that the sample entity's archetype never carries,
/// so the write path's storage-missing error can be reached.
#[repr(C)]
#[derive(Debug, Clone, PartialEq)]
struct OtherComponent {
    sample_number: u32,
}
impl Component for OtherComponent {}

/// The struct that [`NestedOuter`] below carries as its `inner` field. It is
/// registered as a component too, so the layout expansion can find its fields.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NestedInner {
    a: f32,
    b: f32,
}
impl Component for NestedInner {}

/// A value type that is not registered as a component. A component reaches
/// its layout through the value-type resolver instead. The facade's
/// `register_all_components` installs a resolver over the registering DLL's
/// `#[derive(PillMirror)]` types, and the tests install their own resolver.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ProbeVector {
    x: f32,
    y: f32,
}

/// `ProbeVector`'s layout, as `#[derive(PillMirror)]` would declare it.
static PROBE_VECTOR_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "x",
        type_tag: "f32",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "y",
        type_tag: "f32",
        offset: 4,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// The test's value-type resolver. It knows `ProbeVector` and nothing else.
fn probe_value_types(type_path: &str) -> Option<&'static [ComponentFieldDescriptor]> {
    (type_path == "ProbeVector").then_some(PROBE_VECTOR_FIELDS)
}

/// Carries one nested component field and one nested value-type field.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NestedOuter {
    lead: f32,
    inner: NestedInner,
    probe: ProbeVector,
}
impl Component for NestedOuter {}

/// A hand-written field table, with the same shape `#[derive(PillComponent)]`
/// generates.
static SAMPLE_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "a_float",
        type_tag: "f32",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "an_int",
        type_tag: "u32",
        offset: 4,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "a_bool",
        type_tag: "bool",
        offset: 8,
        size: 1,
        align: 1,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    // In `#[repr(C)]` the `[i16; 3]` follows the `bool` at offset 8 and
    // only needs 2-byte alignment, so it starts at offset 10.
    ComponentFieldDescriptor {
        name: "an_array",
        type_tag: "array:i16",
        offset: 10,
        size: 6,
        align: 2,
        element_count: 3,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// A world with [`SampleComponent`] registered together with its declared
/// layout.
fn test_world() -> World {
    let mut world = World::new();
    world.register_component_with_layout::<SampleComponent>(SAMPLE_FIELDS);
    world
}

/// The sample entity's value for [`SampleComponent`].
fn sample_component() -> SampleComponent {
    SampleComponent {
        a_float: 1.5,
        an_int: 7,
        a_bool: true,
        an_array: [1, 2, 3],
    }
}

static NESTED_INNER_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "a",
        type_tag: "f32",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "b",
        type_tag: "f32",
        offset: 4,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

static NESTED_OUTER_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "lead",
        type_tag: "f32",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "inner",
        type_tag: "struct:NestedInner",
        offset: 4,
        size: 8,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "probe",
        type_tag: "struct:ProbeVector",
        offset: 12,
        size: 8,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// A component holding an array of value types, the shape a spline's control
/// points have.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PointArray {
    lead: f32,
    points: [ProbeVector; 2],
}
impl Component for PointArray {}

static POINT_ARRAY_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "lead",
        type_tag: "f32",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "points",
        type_tag: "array:struct:ProbeVector",
        offset: 4,
        size: 16,
        align: 4,
        element_count: 2,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

// =============================================================================
// Field reads, writes and expanded layouts
// =============================================================================

/// An `array:struct:` field expands into one dotted row for every leaf of
/// every element, up to [`MAX_EXPANDED_ELEMENTS`] elements. The stride between
/// elements comes from the array's own size, and each leaf reads and writes
/// like a scalar field.
#[test]
fn struct_arrays_expand_into_editable_element_rows() {
    let mut world = World::new();
    world.replace_value_type_layout_resolver(Some(probe_value_types));
    world.register_component_with_layout::<PointArray>(POINT_ARRAY_FIELDS);
    world.replace_value_type_layout_resolver(None);

    let layout = world
        .component_field_layout(ComponentId::of::<PointArray>())
        .expect("PointArray registered with a layout");
    let field_names: Vec<&str> = layout.iter().map(|field| field.name).collect();
    assert_eq!(
        field_names,
        [
            "lead",
            "points[0].x",
            "points[0].y",
            "points[1].x",
            "points[1].y"
        ]
    );
    assert_eq!(layout[1].offset, 4, "first element starts after `lead`");
    assert_eq!(layout[3].offset, 12, "the second element is one stride on");

    let entity = world
        .create_entity()
        .with(PointArray {
            lead: 1.0,
            points: [
                ProbeVector { x: 2.0, y: 3.0 },
                ProbeVector { x: 4.0, y: 5.0 },
            ],
        })
        .build()
        .expect("entity builds");
    let component_name = world
        .entity_component_names(entity)
        .and_then(|names| names.into_iter().find(|name| name.ends_with("PointArray")))
        .expect("the entity carries PointArray");
    let read_value = world
        .read_component_field(entity, &component_name, "points[1].y")
        .expect("the element's leaf reads");
    assert!(matches!(read_value, FieldValue::F32(value) if value == 5.0));
    world
        .write_component_field(entity, &component_name, "points[0].x", FieldValue::F32(9.5))
        .expect("the element's leaf writes");
    let read_value = world
        .read_component_field(entity, &component_name, "points[0].x")
        .expect("the element's leaf reads back");
    assert!(matches!(read_value, FieldValue::F32(value) if value == 9.5));
}

/// A world with [`NestedInner`] and [`NestedOuter`] registered.
///
/// The value-type resolver is installed while the two register, so
/// `NestedOuter`'s `probe` field expands into leaf rows.
fn nested_world() -> World {
    let mut world = World::new();
    world.replace_value_type_layout_resolver(Some(probe_value_types));
    world.register_component_with_layout::<NestedInner>(NESTED_INNER_FIELDS);
    world.register_component_with_layout::<NestedOuter>(NESTED_OUTER_FIELDS);
    world.replace_value_type_layout_resolver(None);
    world
}

/// Nested `struct:` fields expand into dotted leaf rows at registration,
/// and the leaves read and write like any scalar field.
#[test]
fn nested_fields_expand_into_editable_rows() {
    let mut world = nested_world();

    let layout = world
        .component_field_layout(ComponentId::of::<NestedOuter>())
        .expect("NestedOuter registered with a layout");
    let field_names: Vec<&str> = layout.iter().map(|field| field.name).collect();
    assert_eq!(
        field_names,
        ["lead", "inner.a", "inner.b", "probe.x", "probe.y"]
    );
    assert_eq!(layout[1].offset, 4, "composed offsets for inner.a");
    assert_eq!(layout[2].offset, 8, "composed offsets for inner.b");
    assert_eq!(layout[3].offset, 12, "composed offsets for probe.x");

    let entity = world
        .create_entity()
        .with(NestedOuter {
            lead: 1.0,
            inner: NestedInner { a: 2.0, b: 3.0 },
            probe: ProbeVector { x: 4.0, y: 5.0 },
        })
        .build()
        .expect("entity builds");

    let component_name = crate::type_names::stable_type_name::<NestedOuter>();
    assert_eq!(
        world
            .read_component_field(entity, component_name, "inner.b")
            .unwrap(),
        FieldValue::F32(3.0)
    );
    assert_eq!(
        world
            .read_component_field(entity, component_name, "probe.y")
            .unwrap(),
        FieldValue::F32(5.0)
    );

    world
        .write_component_field(entity, component_name, "inner.b", FieldValue::F32(9.0))
        .expect("the expanded leaf is writable");
    assert_eq!(
        world
            .read_component_field(entity, component_name, "inner.b")
            .unwrap(),
        FieldValue::F32(9.0)
    );
}

/// The declared layout keeps nested fields whole, unlike the flattened layout
/// [`World::component_field_layout`] returns. The C# mirror is generated from
/// the declared form, because the editor's leaf rows name fields no struct
/// declares.
#[test]
fn the_declared_layout_keeps_nested_fields_whole() {
    let mut world = nested_world();
    world.replace_value_type_layout_resolver(Some(probe_value_types));
    world.register_component_with_layout::<PointArray>(POINT_ARRAY_FIELDS);
    world.replace_value_type_layout_resolver(None);

    let declared_layout = world
        .component_declared_field_layout(ComponentId::of::<NestedOuter>())
        .expect("NestedOuter registered with a layout");
    let field_names: Vec<&str> = declared_layout.iter().map(|field| field.name).collect();
    assert_eq!(field_names, ["lead", "inner", "probe"]);
    assert_eq!(declared_layout[1].type_tag, "struct:NestedInner");
    assert_eq!(declared_layout[1].offset, 4);
    assert_eq!(
        declared_layout[1].size, 8,
        "the whole nested struct, not a leaf"
    );

    // An array of structs stays one field in the declared layout, so the mirror
    // declares the array it holds instead of one field per leaf.
    let declared_layout = world
        .component_declared_field_layout(ComponentId::of::<PointArray>())
        .expect("PointArray registered with a layout");
    let field_names: Vec<&str> = declared_layout.iter().map(|field| field.name).collect();
    assert_eq!(field_names, ["lead", "points"]);
    assert_eq!(declared_layout[1].type_tag, "array:struct:ProbeVector");
    assert_eq!(declared_layout[1].element_count, 2);
}

/// A `struct:` field whose type has no known layout stays one opaque row
/// instead of being guessed at.
#[test]
fn unresolvable_struct_fields_stay_opaque() {
    static OPAQUE_HOLDER_FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
        name: "mystery",
        type_tag: "struct:NoSuchTypeExists",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    }];

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct OpaqueHolder {
        mystery: u32,
    }
    impl Component for OpaqueHolder {}

    let mut world = World::new();
    world.register_component_with_layout::<OpaqueHolder>(OPAQUE_HOLDER_FIELDS);
    let layout = world
        .component_field_layout(ComponentId::of::<OpaqueHolder>())
        .expect("OpaqueHolder registered with a layout");
    assert_eq!(layout.len(), 1);
    assert_eq!(layout[0].name, "mystery");
    assert_eq!(layout[0].type_tag, "struct:NoSuchTypeExists");
}

/// A captured opaque payload round-trips. Read it from a live row, write it
/// back through a field write or into a whole component image, and the bytes
/// match. A payload whose tag or size does not match the field is refused.
#[test]
fn captured_opaque_payloads_round_trip_and_are_pinned_to_their_field() {
    static OPAQUE_HOLDER_FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
        name: "mystery",
        type_tag: "struct:NoSuchTypeExists",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    }];

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct OpaqueHolder {
        mystery: u32,
    }
    impl Component for OpaqueHolder {}

    let mut world = World::new();
    world.register_component_with_layout::<OpaqueHolder>(OPAQUE_HOLDER_FIELDS);
    let component_name = crate::component::ComponentRegistry::registered_name::<OpaqueHolder>();

    let entity = world
        .create_entity()
        .with(OpaqueHolder {
            mystery: 0xAAAA_BBBB,
        })
        .build()
        .expect("entity builds");
    let captured_payload = world
        .read_component_field(entity, &component_name, "mystery")
        .expect("the opaque field reads");
    let FieldValue::Opaque { type_tag, bytes } = captured_payload.clone() else {
        panic!("a struct-tagged field must read as opaque, got {captured_payload:?}");
    };
    assert_eq!(type_tag, "struct:NoSuchTypeExists");
    // These are the row's bytes, so they use the machine's native byte order
    // rather than a fixed one.
    assert_eq!(bytes, 0xAAAA_BBBBu32.to_ne_bytes());

    // A field write of the captured payload writes the same bytes back.
    world
        .write_component_field(entity, &component_name, "mystery", captured_payload.clone())
        .expect("the captured payload writes back");
    assert_eq!(
        world.read_component_field(entity, &component_name, "mystery"),
        Ok(captured_payload.clone())
    );

    // The same payload also builds a whole component image. That is the call a
    // scene load makes to attach a component.
    let component_image = world
        .build_component_image(
            &component_name,
            &[("mystery".to_string(), captured_payload.clone())],
        )
        .expect("the captured image builds");
    assert_eq!(component_image, 0xAAAA_BBBBu32.to_ne_bytes());

    // A payload whose size or tag does not match the field is refused.
    let short_payload = FieldValue::Opaque {
        type_tag,
        bytes: vec![0, 1],
    };
    assert_eq!(
        world.write_component_field(entity, &component_name, "mystery", short_payload.clone()),
        Err(ComponentFieldError::TypeMismatch {
            field: "mystery".to_string(),
            expected: "struct:NoSuchTypeExists",
            found: "opaque",
        })
    );
    let mistagged_payload = FieldValue::Opaque {
        type_tag: "struct:SomethingElse",
        bytes: bytes.clone(),
    };
    assert_eq!(
        world.build_component_image(
            &component_name,
            &[("mystery".to_string(), mistagged_payload)],
        ),
        Err(ComponentFieldError::TypeMismatch {
            field: "mystery".to_string(),
            expected: "struct:NoSuchTypeExists",
            found: "opaque",
        })
    );
}

/// An opaque payload cannot be written over a heap container's header.
///
/// A `dynbuf:` field is a `DynamicBuffer` header, so an opaque payload with
/// the right tag and size would copy its bytes straight over the live pointer.
/// Every container tag is refused by the generic write path instead and can
/// only be edited through the field's accessors.
#[test]
fn an_opaque_payload_cannot_overwrite_a_container_handle() {
    static DYN_HOLDER_FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
        name: "samples",
        type_tag: "dynbuf:f32",
        offset: 0,
        size: 3 * std::mem::size_of::<usize>(),
        align: std::mem::align_of::<usize>(),
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    }];

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct DynHolder {
        /// A `DynamicBuffer` header: pointer, element count, capacity.
        samples: [usize; 3],
    }
    impl Component for DynHolder {}

    let mut world = World::new();
    world.register_component_with_layout::<DynHolder>(DYN_HOLDER_FIELDS);
    let component_name = crate::component::ComponentRegistry::registered_name::<DynHolder>();
    let entity = world
        .create_entity()
        .with(DynHolder {
            samples: [0x1000, 4, 8],
        })
        .build()
        .expect("entity builds");

    // A scene document can carry this payload. The tag and size match the
    // field exactly, and the bytes are the caller's own, so the write path
    // has to refuse it by the field's tag alone.
    let forged_payload = FieldValue::Opaque {
        type_tag: "dynbuf:f32",
        bytes: 0xDEAD_BEEF_u64.to_ne_bytes().repeat(3),
    };
    assert!(
        matches!(
            world.write_component_field(entity, &component_name, "samples", forged_payload),
            Err(ComponentFieldError::UnsupportedField { .. })
        ),
        "a container field must refuse the generic write path"
    );

    // The live header is untouched.
    assert_eq!(
        world.read_component_field(entity, &component_name, "samples"),
        Ok(FieldValue::List {
            element_tag: "f32",
            element_count: 4,
        })
    );
}

/// A heap container field is read by following its real header.
///
/// The standard library does not fix the order of the three words in a
/// `String` or `Vec` header, so the codec measures the pointer and length
/// offsets at runtime. A guessed offset would read the length as an address
/// and dereference it, or report the capacity as the element count.
#[test]
fn heap_container_fields_read_through_their_real_header() {
    static HEAP_CONTAINER_PROBE_FIELDS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "label",
            type_tag: "string",
            offset: 0,
            size: std::mem::size_of::<String>(),
            align: std::mem::align_of::<String>(),
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "points",
            type_tag: "vec:u32",
            offset: std::mem::size_of::<String>(),
            size: std::mem::size_of::<Vec<u32>>(),
            align: std::mem::align_of::<Vec<u32>>(),
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];

    #[repr(C)]
    #[derive(Debug)]
    struct HeapContainerProbe {
        label: String,
        points: Vec<u32>,
    }
    impl Component for HeapContainerProbe {}

    // Three live elements in a sixteen-element allocation, so the count and
    // the capacity cannot be confused for one another.
    let mut points = Vec::with_capacity(16);
    points.extend([10_u32, 20, 30]);

    let mut world = World::new();
    world.register_component_with_layout::<HeapContainerProbe>(HEAP_CONTAINER_PROBE_FIELDS);
    let component_name =
        crate::component::ComponentRegistry::registered_name::<HeapContainerProbe>();
    let entity = world
        .create_entity()
        .with(HeapContainerProbe {
            label: "hello".to_string(),
            points,
        })
        .build()
        .expect("entity builds");

    assert_eq!(
        world.read_component_field(entity, &component_name, "label"),
        Ok(FieldValue::Text("hello".to_string()))
    );
    assert_eq!(
        world.read_component_field(entity, &component_name, "points"),
        Ok(FieldValue::List {
            element_tag: "u32",
            element_count: 3,
        })
    );
}

/// Every scalar value agrees with the tags and codecs that describe it.
///
/// A tag counts as part of the scalar vocabulary only when the tag test, the
/// size lookup and both codecs all accept it. Otherwise a scalar could be
/// sized but not decoded, or decoded but reported by no name.
#[test]
fn every_scalar_tag_sizes_encodes_decodes_and_names_itself() {
    let field_values = [
        FieldValue::F32(1.5),
        FieldValue::F64(-2.25),
        FieldValue::I8(-3),
        FieldValue::I16(-300),
        FieldValue::I32(-70_000),
        FieldValue::I64(-5_000_000_000),
        FieldValue::U8(7),
        FieldValue::U16(600),
        FieldValue::U32(70_000),
        FieldValue::U64(5_000_000_000),
        FieldValue::Bool(true),
        FieldValue::Usize(12),
        FieldValue::Isize(-12),
    ];
    for field_value in field_values {
        let type_tag = field_value.variant_tag();
        let size = scalar_size(type_tag).unwrap_or_else(|| panic!("`{type_tag}` is a scalar tag"));
        assert!(is_scalar_tag(type_tag), "`{type_tag}` is a scalar tag");

        let mut bytes = Vec::new();
        assert!(
            encode_scalar(type_tag, &field_value, &mut bytes),
            "`{type_tag}` encodes"
        );
        assert_eq!(
            bytes.len(),
            size,
            "`{type_tag}` encodes to its declared size"
        );

        let decoded = decode_scalar(&bytes, 0, type_tag).expect("the encoded value decodes");
        assert_eq!(
            decoded, field_value,
            "`{type_tag}` survives its own round trip"
        );
    }

    // A tag no helper knows, `u128` here, is refused by every one of them
    // rather than by some.
    assert_eq!(scalar_size("u128"), None);
    assert!(!is_scalar_tag("u128"));
    assert!(!encode_scalar("u128", &FieldValue::U8(1), &mut Vec::new()));
    assert_eq!(decode_scalar(&[0_u8; 16], 0, "u128"), None);
}

#[test]
fn round_trip_every_scalar_field() {
    let mut world = test_world();
    let entity = world
        .create_entity()
        .with(sample_component())
        .build()
        .expect("entity builds");

    let component_name = crate::type_names::stable_type_name::<SampleComponent>();
    assert_eq!(
        world.read_component_field(entity, component_name, "a_float"),
        Ok(FieldValue::F32(1.5))
    );
    assert_eq!(
        world.read_component_field(entity, component_name, "an_int"),
        Ok(FieldValue::U32(7))
    );
    assert_eq!(
        world.read_component_field(entity, component_name, "a_bool"),
        Ok(FieldValue::Bool(true))
    );
    assert_eq!(
        world.read_component_field(entity, component_name, "an_array"),
        Ok(FieldValue::Array(vec![
            FieldValue::I16(1),
            FieldValue::I16(2),
            FieldValue::I16(3),
        ]))
    );

    world
        .write_component_field(entity, component_name, "a_float", FieldValue::F32(9.25))
        .expect("write succeeds");
    world
        .write_component_array_element(entity, component_name, "an_array", 1, FieldValue::I16(-5))
        .expect("element write succeeds");

    let component = world
        .get_component::<SampleComponent>(entity)
        .expect("component still attached");
    assert_eq!(component.a_float, 9.25);
    assert_eq!(component.an_array, [1, -5, 3]);
}

#[test]
fn refusing_errors_leave_state_untouched() {
    let mut world = test_world();
    let entity = world
        .create_entity()
        .with(sample_component())
        .build()
        .expect("entity builds");
    let component_name = crate::type_names::stable_type_name::<SampleComponent>();

    assert_eq!(
        world.write_component_field(entity, component_name, "an_int", FieldValue::F32(1.0)),
        Err(ComponentFieldError::TypeMismatch {
            field: "an_int".to_string(),
            expected: "u32",
            found: "f32",
        })
    );
    assert_eq!(
        world.write_component_field(
            entity,
            component_name,
            "an_array",
            FieldValue::Array(vec![])
        ),
        Err(ComponentFieldError::ArrayLengthMismatch {
            field: "an_array".to_string(),
            expected: 3,
            found: 0,
        })
    );
    assert_eq!(
        world.write_component_array_element(
            entity,
            component_name,
            "an_array",
            5,
            FieldValue::I16(1),
        ),
        Err(ComponentFieldError::ArrayIndexOutOfRange {
            field: "an_array".to_string(),
            index: 5,
            length: 3,
        })
    );

    // Nothing above was written.
    let component = world.get_component::<SampleComponent>(entity).unwrap();
    assert_eq!(component.an_int, 7);
    assert_eq!(component.an_array, [1, 2, 3]);
}

#[test]
fn dead_entity_and_unknown_component_and_field() {
    let mut world = test_world();
    let entity = world
        .create_entity()
        .with(sample_component())
        .build()
        .expect("entity builds");
    let component_name = crate::type_names::stable_type_name::<SampleComponent>();

    let _ = world.destroy_entity(entity);
    assert_eq!(
        world.read_component_field(entity, component_name, "a_float"),
        Err(ComponentFieldError::EntityNotFound)
    );

    let entity = world
        .create_entity()
        .with(sample_component())
        .build()
        .expect("entity builds");
    assert_eq!(
        world.read_component_field(entity, "missing::Type", "a_float"),
        Err(ComponentFieldError::ComponentNotFound {
            component: "missing::Type".to_string(),
        })
    );
    assert_eq!(
        world.read_component_field(entity, component_name, "missing_field"),
        Err(ComponentFieldError::FieldNotFound {
            component: component_name.to_string(),
            field: "missing_field".to_string(),
        })
    );
}

/// The three single-field entry points miss the same way.
///
/// A read, a write and an array-element write share one resolver, so each must
/// report a dead entity, a missing component and a missing field with the same
/// variant and payload. A new entry point has to join the probes below before
/// it is covered.
#[test]
fn every_entry_point_reports_the_same_misses() {
    let mut world = test_world();
    let entity = world
        .create_entity()
        .with(sample_component())
        .build()
        .expect("entity builds");
    let component_name = crate::type_names::stable_type_name::<SampleComponent>();

    // One probe per entry point, for each kind of miss.
    for (component, field) in [
        ("missing::Type", "a_float"),
        (component_name, "missing_field"),
    ] {
        let read = world.read_component_field(entity, component, field).err();
        let write = world
            .write_component_field(entity, component, field, FieldValue::F32(0.0))
            .err();
        let element = world
            .write_component_array_element(entity, component, field, 0, FieldValue::I16(0))
            .err();
        assert_eq!(read, write, "read and write agree on `{field}`");
        assert_eq!(write, element, "the array write agrees on `{field}`");
    }
    assert_eq!(
        world.read_component_field(entity, "missing::Type", "a_float"),
        Err(ComponentFieldError::ComponentNotFound {
            component: "missing::Type".to_string(),
        })
    );
    assert_eq!(
        world.read_component_field(entity, component_name, "missing_field"),
        Err(ComponentFieldError::FieldNotFound {
            component: component_name.to_string(),
            field: "missing_field".to_string(),
        })
    );

    let _ = world.destroy_entity(entity);
    assert_eq!(
        world.read_component_field(entity, component_name, "a_float"),
        Err(ComponentFieldError::EntityNotFound)
    );
    assert_eq!(
        world.write_component_field(entity, component_name, "a_float", FieldValue::F32(0.0)),
        Err(ComponentFieldError::EntityNotFound)
    );
    assert_eq!(
        world.write_component_array_element(
            entity,
            component_name,
            "an_array",
            0,
            FieldValue::I16(0),
        ),
        Err(ComponentFieldError::EntityNotFound)
    );
}

/// A storage miss names the component instead of blaming the entity.
///
/// The entry points resolve the component through a live entity, so a column
/// or row that disappears after that point means the registration and the
/// storage disagree. Reporting `EntityNotFound` would send callers looking for
/// a dead handle that is right there.
#[test]
fn storage_missing_names_the_component() {
    // The summary names the component and cannot be read as a dead handle.
    let summary = ComponentFieldError::ComponentStorageMissing {
        component: "demo::Settings".to_string(),
    }
    .summary();
    assert!(summary.contains("demo::Settings"), "{summary}");
    assert!(!summary.contains("entity"), "{summary}");

    // The write path reports it too. `OtherComponent` is registered, but the
    // entity's archetype holds no column for it.
    let mut world = test_world();
    world.register_component::<OtherComponent>();
    let entity = world
        .create_entity()
        .with(sample_component())
        .build()
        .expect("entity builds");
    let (archetype_id, row) = world.entity_row_location(entity).expect("entity is live");
    assert_eq!(
        world.apply_row_write(
            super::row::RowWriteTarget {
                archetype_id,
                row,
                component_id: ComponentId::of::<OtherComponent>(),
                component_name: "OtherComponent",
                field_name: "other",
            },
            0,
            &[0u8; 4],
        ),
        Err(ComponentFieldError::ComponentStorageMissing {
            component: "OtherComponent".to_string(),
        })
    );
}

#[test]
fn reads_do_not_stamp_but_writes_do() {
    let mut world = test_world();
    let entity = world
        .create_entity()
        .with(sample_component())
        .build()
        .expect("entity builds");
    let component_name = crate::type_names::stable_type_name::<SampleComponent>();
    let baseline_tick = world.change_tick();

    let component_id = ComponentId::of::<SampleComponent>();
    let row_ticks_of = |world: &World| {
        *world
            .storage
            .archetypes
            .values()
            .next()
            .unwrap()
            .component_storages
            .get(component_id)
            .unwrap()
            .row_ticks(0)
            .unwrap()
    };

    let _ = world
        .read_component_field(entity, component_name, "a_float")
        .unwrap();
    let row_ticks = row_ticks_of(&world);
    assert!(!row_ticks
        .changed
        .is_newer_than(baseline_tick, world.change_tick()));

    world
        .write_component_field(entity, component_name, "a_float", FieldValue::F32(2.0))
        .expect("write succeeds");
    let row_ticks = row_ticks_of(&world);
    assert!(row_ticks
        .changed
        .is_newer_than(baseline_tick, world.change_tick()));
}

#[test]
fn changed_filter_observes_a_field_edit() {
    use crate::query::{Changed, Query};

    let mut world = test_world();
    let entity = world
        .create_entity()
        .with(sample_component())
        .build()
        .expect("entity builds");
    let component_name = crate::type_names::stable_type_name::<SampleComponent>();

    // Step 1: set the baseline to now, so the entity's creation already counts
    // as seen and only the edit below is new.
    world.set_system_last_run(world.change_tick());

    // Step 2: edit one field through the generic API. The write must stamp the
    // row's `changed` tick.
    world
        .write_component_field(entity, component_name, "an_int", FieldValue::U32(42))
        .expect("write succeeds");

    // Step 3: a `Changed<T>` filter must yield exactly the edited entity.
    let mut query = Query::<(crate::entity::Entity,), Changed<SampleComponent>>::new(&mut world);
    let matched_entities: Vec<crate::entity::Entity> = query
        .iter_mut()
        .map(|(matched_entity,)| matched_entity)
        .collect();
    assert_eq!(matched_entities, vec![entity]);
}

// =============================================================================
// Asset handle fields
// =============================================================================

/// A component holding two asset handles, with the layout the derive gives
/// `MeshRendererComponent`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RendererProbe {
    mesh: crate::asset::Handle<ProbeMesh>,
    material: crate::asset::Handle<ProbeMaterial>,
}
impl Component for RendererProbe {}

/// The two asset types the probe references.
struct ProbeMesh;
impl crate::asset::Asset for ProbeMesh {}
struct ProbeMaterial;
impl crate::asset::Asset for ProbeMaterial {}

static RENDERER_PROBE_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "mesh",
        type_tag: "struct:Handle<ProbeMesh>",
        offset: 0,
        size: 8,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "material",
        type_tag: "struct:Handle<ProbeMaterial>",
        offset: 8,
        size: 8,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// A world with the probe registered and its two fields' asset types
/// recorded, and one entity carrying it.
///
/// The third value is the probe's registered component name, the form the
/// field entry points take.
fn renderer_probe_world() -> (World, crate::entity::Entity, String) {
    let mut world = World::new();
    world.register_component_with_layout::<RendererProbe>(RENDERER_PROBE_FIELDS);
    let component_id = ComponentId::of::<RendererProbe>();
    world.record_asset_field_type(component_id, "mesh", "probe::Mesh");
    world.record_asset_field_type(component_id, "material", "probe::Material");
    let entity = world
        .create_entity()
        .with(RendererProbe {
            mesh: crate::asset::Handle::from_raw(0, 0),
            material: crate::asset::Handle::from_raw(1, 0),
        })
        .build()
        .expect("entity builds");
    let component_name = crate::component::ComponentRegistry::registered_name::<RendererProbe>();
    (world, entity, component_name)
}

/// A handle field reads as an asset value naming its type and the handle,
/// not as opaque bytes.
#[test]
fn a_handle_field_reads_as_a_typed_asset_reference() {
    let (world, entity, component_name) = renderer_probe_world();
    let field_values = world
        .read_component_fields(entity, &component_name)
        .unwrap();
    assert_eq!(
        field_values,
        vec![
            FieldValue::Asset {
                asset_type: Some("probe::Mesh".to_owned()),
                index: 0,
                generation: 0,
            },
            FieldValue::Asset {
                asset_type: Some("probe::Material".to_owned()),
                index: 1,
                generation: 0,
            },
        ]
    );
}

/// Another handle of the field's type is written and reads back. Writing the
/// invalid handle clears the reference.
#[test]
fn a_handle_field_writes_another_handle_and_clears() {
    let (mut world, entity, component_name) = renderer_probe_world();
    let another_mesh_handle = FieldValue::Asset {
        asset_type: Some("probe::Mesh".to_owned()),
        index: 7,
        generation: 2,
    };
    world
        .write_component_field(entity, &component_name, "mesh", another_mesh_handle.clone())
        .expect("a mesh handle writes into the mesh field");
    assert_eq!(
        world
            .read_component_field(entity, &component_name, "mesh")
            .unwrap(),
        another_mesh_handle
    );
    assert_eq!(
        world.get_component::<RendererProbe>(entity).unwrap().mesh,
        crate::asset::Handle::from_raw(7, 2)
    );

    let cleared_handle = FieldValue::Asset {
        asset_type: Some("probe::Mesh".to_owned()),
        index: u32::MAX,
        generation: u32::MAX,
    };
    world
        .write_component_field(entity, &component_name, "mesh", cleared_handle)
        .expect("the invalid handle clears the reference");
    assert_eq!(
        world.get_component::<RendererProbe>(entity).unwrap().mesh,
        crate::asset::Handle::INVALID
    );
}

/// A handle of another asset type is refused with a typed error, and a
/// non-asset value is refused as a type mismatch. Neither writes.
#[test]
fn a_handle_of_another_asset_type_is_refused() {
    let (mut world, entity, component_name) = renderer_probe_world();
    let material_reference = FieldValue::Asset {
        asset_type: Some("probe::Material".to_owned()),
        index: 3,
        generation: 0,
    };
    let refused = world.write_component_field(entity, &component_name, "mesh", material_reference);
    assert!(
        matches!(
            &refused,
            Err(ComponentFieldError::AssetTypeMismatch { expected, found, .. })
                if expected == "probe::Mesh" && found == "probe::Material"
        ),
        "{refused:?}"
    );
    let scalar_write =
        world.write_component_field(entity, &component_name, "mesh", FieldValue::U32(5));
    assert!(
        matches!(scalar_write, Err(ComponentFieldError::TypeMismatch { .. })),
        "{scalar_write:?}"
    );
    assert_eq!(
        world.get_component::<RendererProbe>(entity).unwrap().mesh,
        crate::asset::Handle::from_raw(0, 0),
        "nothing was written"
    );
}

// =============================================================================
// Entity reference fields
// =============================================================================

/// A component holding a reference to another entity, with the layout the
/// derive gives an `Entity` field.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FollowProbe {
    target: crate::entity::Entity,
    weight: f32,
}
impl Component for FollowProbe {}

static FOLLOW_PROBE_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "target",
        type_tag: "entity",
        offset: 0,
        size: size_of::<crate::entity::Entity>(),
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "weight",
        type_tag: "f32",
        offset: 16,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// An entity field reads as an entity value naming the handle it holds,
/// not as opaque bytes.
#[test]
fn an_entity_field_reads_as_an_entity_reference() {
    let mut world = World::new();
    world.register_component_with_layout::<FollowProbe>(FOLLOW_PROBE_FIELDS);
    let target = world.create_entity().build().expect("target builds");
    let follower = world
        .create_entity()
        .with(FollowProbe {
            target,
            weight: 0.5,
        })
        .build()
        .expect("follower builds");
    let component_name = crate::component::ComponentRegistry::registered_name::<FollowProbe>();

    assert_eq!(
        world
            .read_component_field(follower, &component_name, "target")
            .unwrap(),
        FieldValue::Entity {
            id: target.id(),
            generation: target.generation(),
        }
    );
}

/// Writing another reference re-points the field and leaves the neighbouring
/// scalar alone.
#[test]
fn an_entity_field_writes_another_reference() {
    let mut world = World::new();
    world.register_component_with_layout::<FollowProbe>(FOLLOW_PROBE_FIELDS);
    let first = world.create_entity().build().expect("first builds");
    let second = world.create_entity().build().expect("second builds");
    let follower = world
        .create_entity()
        .with(FollowProbe {
            target: first,
            weight: 0.5,
        })
        .build()
        .expect("follower builds");
    let component_name = crate::component::ComponentRegistry::registered_name::<FollowProbe>();

    let another_reference = FieldValue::Entity {
        id: second.id(),
        generation: second.generation(),
    };
    world
        .write_component_field(follower, &component_name, "target", another_reference)
        .expect("an entity value writes into the reference");
    assert_eq!(
        world.get_component::<FollowProbe>(follower).unwrap().target,
        second
    );
    assert_eq!(
        world.get_component::<FollowProbe>(follower).unwrap().weight,
        0.5,
        "the neighbouring field was not touched"
    );
}

/// A non-entity value is refused and nothing is written.
#[test]
fn an_entity_field_refuses_a_scalar() {
    let mut world = World::new();
    world.register_component_with_layout::<FollowProbe>(FOLLOW_PROBE_FIELDS);
    let target = world.create_entity().build().expect("target builds");
    let follower = world
        .create_entity()
        .with(FollowProbe {
            target,
            weight: 0.5,
        })
        .build()
        .expect("follower builds");
    let component_name = crate::component::ComponentRegistry::registered_name::<FollowProbe>();

    let refused =
        world.write_component_field(follower, &component_name, "target", FieldValue::U64(7));
    assert!(
        matches!(
            &refused,
            Err(ComponentFieldError::TypeMismatch { expected, found, .. })
                if *expected == "entity" && *found == "u64"
        ),
        "{refused:?}"
    );
    assert_eq!(
        world.get_component::<FollowProbe>(follower).unwrap().target,
        target,
        "nothing was written"
    );
}

// =============================================================================
// Enum fields
// =============================================================================

/// A fieldless enum stored as a `u8`, the shape `#[derive(PillMirror)]`
/// declares.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeShape {
    // Never constructed: it is here so the declaration has the same variants
    // as the resolver below, which the reads and writes check against.
    #[allow(dead_code)]
    Ball = 0,
    Cuboid = 1,
    Capsule = 5,
}

/// A component carrying a [`ProbeShape`] beside a scalar.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ShapedProbe {
    radius: f32,
    shape: ProbeShape,
}
impl Component for ShapedProbe {}

static SHAPED_PROBE_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "radius",
        type_tag: "f32",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "shape",
        type_tag: "struct:ProbeShape",
        offset: 4,
        size: 1,
        align: 1,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// The enum declarations the probe DLL would offer: only `ProbeShape`.
fn probe_enums(path: &str) -> Option<crate::component_registry::EnumLayout> {
    (path == "ProbeShape").then(|| crate::component_registry::EnumLayout {
        representation: "u8",
        variants: vec![("Ball", 0), ("Cuboid", 1), ("Capsule", 5)],
    })
}

/// A world with [`ShapedProbe`] registered, its enum resolver installed only
/// for the registration, and one entity holding a `Cuboid`.
fn shaped_world() -> (World, Entity, String) {
    let mut world = World::new();
    world.replace_enum_layout_resolver(Some(probe_enums));
    world.register_component_with_layout::<ShapedProbe>(SHAPED_PROBE_FIELDS);
    world.replace_enum_layout_resolver(None);
    let entity = world
        .create_entity()
        .with(ShapedProbe {
            radius: 0.5,
            shape: ProbeShape::Cuboid,
        })
        .build()
        .expect("entity builds");
    let component_name = world
        .entity_component_names(entity)
        .and_then(|names| names.into_iter().find(|name| name.ends_with("ShapedProbe")))
        .expect("the entity carries ShapedProbe");
    (world, entity, component_name)
}

/// The tag a declared enum's field gets lists every variant, and reads back
/// into the same declaration.
#[test]
fn an_enum_tag_round_trips_its_variants() {
    let layout = probe_enums("ProbeShape").expect("declared");
    let tag: &'static str = Box::leak(format_enum_tag("ProbeShape", &layout).into_boxed_str());
    assert_eq!(tag, "enum:u8:ProbeShape{Ball=0,Cuboid=1,Capsule=5}");
    let parsed = parse_enum_tag(tag).expect("the tag reads back");
    assert_eq!(parsed.representation, "u8");
    assert_eq!(parsed.type_name, "ProbeShape");
    assert_eq!(parsed.variant_name(5), Some("Capsule"));
    assert_eq!(parsed.discriminant_of("Ball"), Some(0));
    assert_eq!(parsed.variant_name(2), None);
    assert!(parse_enum_tag("struct:ProbeShape").is_none());
}

/// A field of a declared enum reads as an enum value naming its variant, and
/// a write of another variant lands in the row.
#[test]
fn a_declared_enum_field_reads_and_writes_as_an_enum() {
    let (mut world, entity, component_name) = shaped_world();
    let read_value = world
        .read_component_field(entity, &component_name, "shape")
        .expect("the enum field reads");
    let FieldValue::Enum {
        type_tag,
        discriminant,
    } = read_value
    else {
        panic!("expected an enum value, got {read_value:?}");
    };
    assert_eq!(discriminant, 1);
    assert_eq!(read_value.enum_variant_name(), Some("Cuboid"));

    world
        .write_component_field(
            entity,
            &component_name,
            "shape",
            FieldValue::Enum {
                type_tag,
                discriminant: 5,
            },
        )
        .expect("a declared variant writes");
    let stored = world
        .get_component::<ShapedProbe>(entity)
        .expect("the component is still there")
        .shape;
    assert_eq!(stored, ProbeShape::Capsule);
}

/// A discriminant no variant declares, or a value of another kind, is refused
/// before anything is written: the row would otherwise hold bytes the enum
/// cannot be.
#[test]
fn an_enum_field_refuses_undeclared_discriminants_and_other_kinds() {
    let (mut world, entity, component_name) = shaped_world();
    let FieldValue::Enum { type_tag, .. } = world
        .read_component_field(entity, &component_name, "shape")
        .expect("reads")
    else {
        panic!("expected an enum value");
    };
    let undeclared = world.write_component_field(
        entity,
        &component_name,
        "shape",
        FieldValue::Enum {
            type_tag,
            discriminant: 2,
        },
    );
    assert!(matches!(
        undeclared,
        Err(ComponentFieldError::UnsupportedField { .. })
    ));
    let wrong_kind =
        world.write_component_field(entity, &component_name, "shape", FieldValue::U8(5));
    assert!(matches!(
        wrong_kind,
        Err(ComponentFieldError::TypeMismatch { .. })
    ));
    let stored = world
        .get_component::<ShapedProbe>(entity)
        .expect("the component is still there")
        .shape;
    assert_eq!(stored, ProbeShape::Cuboid, "nothing was written");
}

/// Without a declaration in reach the field stays opaque, exactly as before
/// enums were understood.
#[test]
fn an_undeclared_enum_field_stays_opaque() {
    let mut world = World::new();
    world.register_component_with_layout::<ShapedProbe>(SHAPED_PROBE_FIELDS);
    let layout = world
        .component_field_layout(ComponentId::of::<ShapedProbe>())
        .expect("registered with a layout");
    assert_eq!(layout[1].type_tag, "struct:ProbeShape");
}
