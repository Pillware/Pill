//! Integration tests for components whose fields own heap memory.
//!
//! Three things are pinned here. The derive records each container field with
//! a tag that says what it holds (`vec:f32`, `vec:string`, `string`). The
//! generated accessor functions reach the live value through the address of
//! its row. And a reload that changes a component's schema migrates the stored
//! values through serde, while a reload that changes nothing leaves the
//! existing allocations where they are.
//!
//! # Responsibilities
//!
//! - Cover the tags the derive registers for container fields.
//! - Exercise the generated accessor functions through row addresses.
//! - Pin reload migration of heap fields, including the path that leaves an
//!   unchanged allocation in place.

// Reload migration is a development-only engine API.
#![cfg(feature = "development")]

// Standard library
use std::collections::HashSet;

// External crates
use pill_engine::archetype::Blittability;
use pill_engine::component_registry::{register_all_components, ComponentFieldDescriptor};
use pill_engine::{component, ComponentId, World};

/// A persistable component with a `Vec` field, a `String` field and an inline
/// scalar field.
#[component(persistable)]
#[derive(Clone, Debug, Default)]
struct HeapProbe {
    /// Heap-owning sequence field.
    values: Vec<f32>,
    /// Heap-owning text field.
    label: String,
    /// Inline scalar: it keeps a plain `u32` tag beside the container fields,
    /// and its value survives migration unchanged.
    weight: u32,
}

/// The field layout the derive records for [`HeapProbe`], written out by hand
/// so a test can change one tag and watch the schema hash move.
static HEAP_PROBE_LAYOUT: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "values",
        type_tag: "vec:f32",
        offset: 0,
        size: 24,
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "label",
        type_tag: "string",
        offset: 24,
        size: 24,
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "weight",
        type_tag: "u32",
        offset: 48,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// The same [`HeapProbe`] layout with a different element tag, standing in for
/// a build where the declared shape moved. The new tag gives the registration
/// a different schema hash.
static HEAP_PROBE_ALTERED_LAYOUT: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "values",
        type_tag: "vec:f64",
        offset: 0,
        size: 24,
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "label",
        type_tag: "string",
        offset: 24,
        size: 24,
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "weight",
        type_tag: "u32",
        offset: 48,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// The case the layout-aware hash exists for: the same offsets and sizes as
/// `HeapProbe`'s own layout, but a different element type, which a default
/// value alone cannot tell apart.
static HEAP_PROBE_STRING_LAYOUT: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "values",
        type_tag: "vec:string",
        offset: 0,
        size: 24,
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "label",
        type_tag: "string",
        offset: 24,
        size: 24,
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "weight",
        type_tag: "u32",
        offset: 48,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// A persistable component holding a list of strings, each in its own
/// allocation, so the list cannot be read as one contiguous run of bytes.
#[component(persistable)]
#[derive(Clone, Debug, Default)]
struct NameList {
    /// Text list: each element is its own allocation.
    names: Vec<String>,
    /// Inline field after the container.
    weight: u32,
}

/// The field layout the derive records for [`NameList`].
static NAME_LIST_LAYOUT: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "names",
        type_tag: "vec:string",
        offset: 0,
        size: 24,
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "weight",
        type_tag: "u32",
        offset: 24,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// [`NameList`]'s layout with the element tag changed from `vec:string` to
/// `vec:f32`, standing in for a build whose declared shape moved.
static NAME_LIST_ALTERED_LAYOUT: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "names",
        type_tag: "vec:f32",
        offset: 0,
        size: 24,
        align: 8,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "weight",
        type_tag: "u32",
        offset: 24,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// Returns the schema hash of the registered `HeapProbe`, read from the
/// manifest the reload path compares.
fn schema_hash(world: &World) -> u64 {
    world
        .persist_type_manifest()
        .into_iter()
        .find(|entry| entry.type_name.ends_with("HeapProbe"))
        .expect("the component is registered")
        .schema_hash
}

/// A descriptor component's field layout is checked before it is stored. A
/// field whose range overflows and a field that claims a container tag are
/// refused, and the layout accepted before them stays in place.
#[test]
fn an_overflowing_field_layout_is_refused() {
    let mut world = World::new();
    // SAFETY: the registered layout describes eight bytes of plain data, and
    // this test never creates an entity or a column from it, so no pointer or
    // owner is ever read.
    let witness = unsafe { Blittability::assume() };
    let component_id = world
        .register_component_descriptor(0x51, "Heap.LayoutProbe", 8, 4, 1, witness)
        .expect("the component registers");

    let fitting_layout = vec![ComponentFieldDescriptor {
        name: "value",
        type_tag: "u32",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    }];
    world
        .register_component_descriptor_with_layout(component_id, fitting_layout.clone())
        .expect("a fitting layout is accepted");

    let overflowing_layout = vec![ComponentFieldDescriptor {
        name: "whole",
        type_tag: "u32",
        offset: usize::MAX,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    }];
    assert!(
        world
            .register_component_descriptor_with_layout(component_id, overflowing_layout)
            .is_err(),
        "an overflowing field range is refused"
    );

    let container_tag_layout = vec![ComponentFieldDescriptor {
        name: "text",
        type_tag: "string",
        offset: 0,
        size: 8,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    }];
    assert!(
        world
            .register_component_descriptor_with_layout(component_id, container_tag_layout)
            .is_err(),
        "a container tag on a descriptor row is refused"
    );

    // Both refusals left the accepted layout in place.
    assert_eq!(
        world.component_field_layout(component_id),
        Some(fitting_layout.as_slice())
    );
}

/// The derive registers every container field with a tag that names its
/// element type, so both the C# code generator and the schema hash see the
/// real shape.
#[test]
fn derive_registers_container_tags() {
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");

    let layout = world
        .component_field_layout(ComponentId::of::<HeapProbe>())
        .expect("the derive registers a field layout");
    let tag = |name: &str| {
        layout
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.type_tag)
    };
    assert_eq!(tag("values"), Some("vec:f32"));
    assert_eq!(tag("label"), Some("string"));
    assert_eq!(tag("weight"), Some("u32"));
}

/// The generated accessor functions work on the live value. The view reports
/// the real buffer, resize grows the real `Vec`, and set replaces the real
/// `String` and refuses invalid UTF-8.
#[test]
fn accessor_trampolines_operate_on_the_live_value() {
    let mut probe = HeapProbe {
        values: vec![1.0, 2.5],
        label: "before".to_string(),
        weight: 7,
    };
    // A raw address rather than a reference, so nothing borrows `probe` while
    // the trampolines mutate it.
    let row = std::ptr::addr_of_mut!(probe).cast::<u8>();

    // SAFETY: `row` addresses the live `HeapProbe` above for the whole test,
    // and every out pointer is a valid local the trampoline may write.
    unsafe {
        let mut data: *const u8 = std::ptr::null();
        let mut length = 0usize;
        assert_eq!(
            pill_accessor_HeapProbe_values_view(row as *const u8, &mut data, &mut length),
            0
        );
        assert_eq!(length, 2);
        assert_eq!(
            std::slice::from_raw_parts(data as *const f32, length),
            &[1.0, 2.5]
        );

        // Growing fills the new elements with the element type's default.
        assert_eq!(pill_accessor_HeapProbe_values_resize(row, 4), 0);
        assert_eq!(
            pill_accessor_HeapProbe_values_view(row as *const u8, &mut data, &mut length),
            0
        );
        assert_eq!(length, 4);
        assert_eq!(
            std::slice::from_raw_parts(data as *const f32, length),
            &[1.0, 2.5, 0.0, 0.0]
        );

        // A valid replacement is stored; invalid UTF-8 is refused.
        let replacement = "after";
        assert_eq!(
            pill_accessor_HeapProbe_label_set(row, replacement.as_ptr(), replacement.len()),
            0
        );
        assert_eq!(
            pill_accessor_HeapProbe_label_view(row as *const u8, &mut data, &mut length),
            0
        );
        assert_eq!(
            std::slice::from_raw_parts(data, length),
            replacement.as_bytes()
        );
        assert_eq!(
            pill_accessor_HeapProbe_label_set(row, [0xFFu8].as_ptr(), 1),
            2
        );
    }

    assert_eq!(probe.values, vec![1.0, 2.5, 0.0, 0.0]);
    assert_eq!(probe.label, "after");
}

/// Changing only the element type changes the schema hash, so a reload
/// migrates the values instead of keeping the old buffer and reinterpreting
/// it under the new layout.
#[test]
fn container_element_kind_changes_the_schema_hash() {
    let mut world = World::new();
    world.register_persistable_component_with_layout::<HeapProbe>(HEAP_PROBE_LAYOUT);
    let hash_with_float_elements = schema_hash(&world);

    world.register_persistable_component_with_layout::<HeapProbe>(HEAP_PROBE_STRING_LAYOUT);
    let hash_with_string_elements = schema_hash(&world);

    assert_ne!(
        hash_with_float_elements, hash_with_string_elements,
        "a container element-kind change must change the schema hash"
    );
}

/// A changed schema migrates the stored heap values through serde. An
/// unchanged schema leaves the live allocation exactly where it was.
#[test]
fn heap_fields_migrate_with_serde_and_survive_the_fast_path() {
    let mut world = World::new();
    world.register_persistable_component_with_layout::<HeapProbe>(HEAP_PROBE_LAYOUT);
    let entity = world
        .create_entity()
        .with(HeapProbe {
            values: vec![3.5, 4.5],
            label: "kept".to_string(),
            weight: 9,
        })
        .build()
        .expect("entity is created");

    let type_name = world
        .persist_type_manifest()
        .into_iter()
        .map(|entry| entry.type_name)
        .find(|name| name.ends_with("HeapProbe"))
        .expect("the component is persistable");

    // Simulate a reload whose declared shape moved: capture the retiring
    // generation's metadata, register the new shape, then migrate.
    let previous_metadata_by_name = world.capture_persist_type_metadata();
    world.register_persistable_component_with_layout::<HeapProbe>(HEAP_PROBE_ALTERED_LAYOUT);

    let changed_type_names: HashSet<String> = HashSet::from([type_name.clone()]);
    let report = world.migrate_changed_persistable_components(
        &previous_metadata_by_name,
        &changed_type_names,
        None,
    );
    assert_eq!(report.migrated_type_count, 1);
    assert_eq!(report.migrated_entity_count, 1);

    let probe = world
        .get_component::<HeapProbe>(entity)
        .expect("data survived the migration");
    assert_eq!(probe.values, vec![3.5, 4.5]);
    assert_eq!(probe.label, "kept");
    assert_eq!(probe.weight, 9);
    let original_buffer_address = probe.values.as_ptr();

    // With no type reported as changed, the migration visits no value at all.
    // The vector keeps its buffer, which is what makes the common reload cheap
    // and keeps outstanding heap pointers valid.
    let previous_metadata_by_name = world.capture_persist_type_metadata();
    let report = world.migrate_changed_persistable_components(
        &previous_metadata_by_name,
        &HashSet::new(),
        None,
    );
    assert_eq!(report.migrated_type_count, 0);
    let probe = world
        .get_component::<HeapProbe>(entity)
        .expect("data survived the fast path");
    assert_eq!(probe.values.as_ptr(), original_buffer_address);
    assert_eq!(probe.values, vec![3.5, 4.5]);
}

/// A `Vec<String>` field registers as `vec:string`. That tag tells the C#
/// code generator the elements have no contiguous run and must be read one at
/// a time.
#[test]
fn vec_string_field_registers_the_elementwise_tag() {
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");

    let layout = world
        .component_field_layout(ComponentId::of::<NameList>())
        .expect("the derive registers a field layout");
    let names_field = layout
        .iter()
        .find(|field| field.name == "names")
        .expect("the list field is declared");
    assert_eq!(names_field.type_tag, "vec:string");
}

/// The per-element accessor functions work on the live list. The view reports
/// the element count and a null data pointer, item, set and push each move one
/// string, an out-of-range index and invalid UTF-8 are refused, and growing
/// fills the new slots with empty strings.
#[test]
fn vec_string_trampolines_reach_each_element() {
    let mut list = NameList {
        names: vec!["alpha".to_string(), "beta".to_string()],
        weight: 3,
    };
    // A raw address rather than a reference, so nothing borrows `list` while
    // the trampolines mutate it.
    let row = std::ptr::addr_of_mut!(list).cast::<u8>();

    // SAFETY: `row` addresses the live `NameList` above for the whole test,
    // and every out pointer is a valid local the trampoline may write.
    unsafe {
        // The view reports the element count and writes a null data pointer:
        // each element is its own allocation, so there is no contiguous run to
        // point at. The seeded pointer is always overwritten.
        let mut data: *const u8 = 0x1234_usize as *const u8;
        let mut length = 0usize;
        assert_eq!(
            pill_accessor_NameList_names_view(row as *const u8, &mut data, &mut length),
            0
        );
        assert!(data.is_null());
        assert_eq!(length, 2);

        // An element read borrows one string for the call.
        assert_eq!(
            pill_accessor_NameList_names_item(row as *const u8, 1, &mut data, &mut length),
            0
        );
        assert_eq!(
            std::str::from_utf8(std::slice::from_raw_parts(data, length)).unwrap(),
            "beta"
        );
        assert_eq!(
            pill_accessor_NameList_names_item(row as *const u8, 2, &mut data, &mut length),
            2
        );

        // Replace one element, then the two refusals: an index out of range
        // and bytes that are not UTF-8. The slot keeps its old text in both
        // cases.
        let replacement = "gamma";
        assert_eq!(
            pill_accessor_NameList_names_set_item(row, 1, replacement.as_ptr(), replacement.len()),
            0
        );
        assert_eq!(
            pill_accessor_NameList_names_set_item(row, 5, replacement.as_ptr(), replacement.len()),
            2
        );
        assert_eq!(
            pill_accessor_NameList_names_set_item(row, 0, [0xFFu8].as_ptr(), 1),
            3
        );

        // Append one element, then grow the list. The added slots hold empty
        // strings.
        let appended = "delta";
        assert_eq!(
            pill_accessor_NameList_names_push(row, appended.as_ptr(), appended.len()),
            0
        );
        assert_eq!(pill_accessor_NameList_names_resize(row, 6), 0);
    }

    assert_eq!(list.names, vec!["alpha", "gamma", "delta", "", "", ""]);
    assert_eq!(list.weight, 3);
}

/// String lists migrate through serde when the schema changes and stay in
/// place on the fast path, like the other containers. The difference is that
/// each element's text, not one contiguous buffer, travels through the
/// serialized form.
#[test]
fn vec_string_contents_migrate_with_serde_and_survive_the_fast_path() {
    let mut world = World::new();
    world.register_persistable_component_with_layout::<NameList>(NAME_LIST_LAYOUT);
    let entity = world
        .create_entity()
        .with(NameList {
            names: vec!["one".to_string(), "two".to_string()],
            weight: 12,
        })
        .build()
        .expect("entity is created");

    let type_name = world
        .persist_type_manifest()
        .into_iter()
        .map(|entry| entry.type_name)
        .find(|name| name.ends_with("NameList"))
        .expect("the component is persistable");

    let previous_metadata_by_name = world.capture_persist_type_metadata();
    world.register_persistable_component_with_layout::<NameList>(NAME_LIST_ALTERED_LAYOUT);
    let changed_type_names: HashSet<String> = HashSet::from([type_name.clone()]);
    let report = world.migrate_changed_persistable_components(
        &previous_metadata_by_name,
        &changed_type_names,
        None,
    );
    assert_eq!(report.migrated_type_count, 1);
    assert_eq!(report.migrated_entity_count, 1);

    let list = world
        .get_component::<NameList>(entity)
        .expect("data survived the migration");
    assert_eq!(list.names, vec!["one", "two"]);
    assert_eq!(list.weight, 12);
    let original_buffer_address = list.names.as_ptr();

    let previous_metadata_by_name = world.capture_persist_type_metadata();
    let report = world.migrate_changed_persistable_components(
        &previous_metadata_by_name,
        &HashSet::new(),
        None,
    );
    assert_eq!(report.migrated_type_count, 0);
    let list = world
        .get_component::<NameList>(entity)
        .expect("data survived the fast path");
    assert_eq!(list.names.as_ptr(), original_buffer_address);
    assert_eq!(list.names, vec!["one", "two"]);
}
