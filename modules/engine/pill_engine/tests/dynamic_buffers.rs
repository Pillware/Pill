//! Integration tests for engine-owned native dynamic buffers.
//!
//! Pins the address-stability contract at three points: the tag the derive
//! registers for a buffer field, the trampolines it generates, and what happens
//! to an engine-owned block when its entity moves to another archetype (the
//! handle is copied, the block is not). It also covers migration through serde,
//! the unchanged-schema pass, and balanced ownership of the engine's native
//! memory.
//!
//! # Responsibilities
//!
//! - Exercise the derive's `dynbuf:` tag and the `DynamicBuffer` API.
//! - Pin the address-stability contract across an archetype move and the
//!   unchanged-schema migration pass.
//! - Pin balanced ownership of the shared native allocation service.

// Reload migration is a development-only engine API.
#![cfg(feature = "development")]

// Standard library
use std::collections::HashSet;
use std::sync::Mutex;

// External crates
use pill_core::native_buffer;
use pill_engine::component_registry::{register_all_components, ComponentFieldDescriptor};
use pill_engine::{component, ComponentId, DynamicBuffer, World};

// =============================================================================
// Constants
// =============================================================================

/// The layout the derive registers for [`Trail`], written out by hand so a test
/// can register the same Rust type with a different one.
static TRAIL_DERIVE_LAYOUT: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "points",
        type_tag: "dynbuf:f32",
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

/// The same Rust type with a moved declared shape: `points` claims `f64`
/// elements instead of `f32`, which changes the schema hash. It stands in for a
/// build whose declared shape moved.
static TRAIL_F64_ELEMENT_LAYOUT: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "points",
        type_tag: "dynbuf:f64",
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

/// Held by every test that checks the process-wide block counters, so those
/// tests never run at the same time.
static ACCOUNTING_LOCK: Mutex<()> = Mutex::new(());

// =============================================================================
// Fixture Components
// =============================================================================

/// A persistable component with an engine-owned buffer field and a plain `u32`
/// field, so one structural change carries both kinds of field.
#[component(persistable)]
#[derive(Clone, Debug, Default)]
struct Trail {
    points: DynamicBuffer<f32>,
    weight: u32,
}

/// A second component that forces an archetype move.
#[component(persistable)]
#[derive(Clone, Debug, Default)]
struct Marker {
    value: u32,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Takes the lock every test that checks the process-wide block counters must
/// hold.
///
/// A poisoned lock means another test panicked while holding it. That panic is
/// already reported, so the lock is taken anyway instead of failing every later
/// test as well.
fn lock_accounting() -> std::sync::MutexGuard<'static, ()> {
    ACCOUNTING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// =============================================================================
// Tests
// =============================================================================

/// The derive registers an engine-owned buffer field with its `dynbuf:` tag,
/// and both the C# codegen and the schema hash read that tag.
#[test]
fn derive_registers_the_dynbuf_tag() {
    let _accounting_lock = lock_accounting();
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");

    let field_layout = world
        .component_field_layout(ComponentId::of::<Trail>())
        .expect("the derive registers a field layout");
    let field_type_tag = |field_name: &str| {
        field_layout
            .iter()
            .find(|field| field.name == field_name)
            .map(|field| field.type_tag)
    };
    assert_eq!(field_type_tag("points"), Some("dynbuf:f32"));
    assert_eq!(field_type_tag("weight"), Some("u32"));
}

/// The generated trampolines work on the live component: the view reports the
/// block's elements and its length, and a resize writes the new elements
/// through the handle in the component row. Growing past the capacity gives the
/// buffer a new block.
#[test]
fn accessor_trampolines_resize_the_engine_owned_buffer() {
    let _accounting_lock = lock_accounting();
    let mut trail = Trail {
        points: DynamicBuffer::from_slice(&[1.0, 2.0]),
        weight: 3,
    };
    // A raw address rather than a reference, so nothing borrows `trail` while
    // the trampolines mutate it.
    let row = std::ptr::addr_of_mut!(trail).cast::<u8>();

    // SAFETY: `row` addresses the live `Trail` above for the whole test, and
    // every out pointer is a valid local the trampoline may write. The slices
    // built from `view_data` and `view_length` read elements the view reported
    // from that live block, so they stay inside it.
    unsafe {
        let mut view_data: *const u8 = std::ptr::null();
        let mut view_length = 0usize;
        assert_eq!(
            pill_accessor_Trail_points_view(row as *const u8, &mut view_data, &mut view_length),
            0
        );
        assert_eq!(view_length, 2);
        assert_eq!(
            std::slice::from_raw_parts(view_data as *const f32, view_length),
            &[1.0, 2.0]
        );

        // Growing fills the new elements with the element type's default.
        assert_eq!(pill_accessor_Trail_points_resize(row, 4), 0);
        assert_eq!(
            pill_accessor_Trail_points_view(row as *const u8, &mut view_data, &mut view_length),
            0
        );
        assert_eq!(view_length, 4);
        assert_eq!(
            std::slice::from_raw_parts(view_data as *const f32, view_length),
            &[1.0, 2.0, 0.0, 0.0]
        );
    }

    assert_eq!(trail.points.as_slice(), &[1.0, 2.0, 0.0, 0.0]);
}

/// A structural change copies the handle and never the block, so the address an
/// element view was taken from stays valid across an archetype move. Generated
/// C# spans depend on that property.
#[test]
fn archetype_move_copies_the_handle_not_the_block() {
    let _accounting_lock = lock_accounting();
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");
    let entity = world
        .create_entity()
        .with(Trail {
            points: DynamicBuffer::from_slice(&[7.0, 8.0, 9.0]),
            weight: 1,
        })
        .build()
        .expect("entity is created");

    let trail_before_move = world
        .get_component::<Trail>(entity)
        .expect("trail is present");
    let block_address = trail_before_move.points.as_ptr();
    let baseline_live_byte_count = native_buffer::live_bytes();

    // Adding a component moves the entity into a new archetype, which copies
    // each surviving component's row into the destination column as raw bytes.
    world
        .add_component::<Marker>(entity, Marker { value: 5 })
        .expect("component is added");

    let trail_after_move = world
        .get_component::<Trail>(entity)
        .expect("trail survived the move");
    assert_eq!(
        trail_after_move.points.as_ptr(),
        block_address,
        "the block must not move when the entity does"
    );
    assert_eq!(trail_after_move.points.as_slice(), &[7.0, 8.0, 9.0]);
    assert_eq!(
        native_buffer::live_bytes(),
        baseline_live_byte_count,
        "a handle copy must not allocate or leak a block"
    );

    drop(world);
    assert_eq!(
        native_buffer::live_bytes(),
        0,
        "dropping the world must release every block"
    );
}

/// A changed schema migrates the buffer's contents through serde, and the
/// unchanged-schema pass leaves the block, and its address, where it was.
#[test]
fn buffer_contents_survive_migration_and_the_fast_path() {
    let _accounting_lock = lock_accounting();
    let mut world = World::new();
    world.register_persistable_component_with_layout::<Trail>(TRAIL_DERIVE_LAYOUT);
    let entity = world
        .create_entity()
        .with(Trail {
            points: DynamicBuffer::from_slice(&[3.5, 4.5]),
            weight: 9,
        })
        .build()
        .expect("entity is created");

    let type_name = world
        .persist_type_manifest()
        .into_iter()
        .map(|entry| entry.type_name)
        .find(|name| name.ends_with("Trail"))
        .expect("the component is persistable");

    // Simulate a reload whose declared shape moved: capture the metadata of the
    // shape being replaced, register the new shape, then migrate the values.
    let previous_metadata_by_name = world.capture_persist_type_metadata();
    world.register_persistable_component_with_layout::<Trail>(TRAIL_F64_ELEMENT_LAYOUT);
    let changed_type_names: HashSet<String> = HashSet::from([type_name]);
    let migration_report = world.migrate_changed_persistable_components(
        &previous_metadata_by_name,
        &changed_type_names,
        None,
    );
    assert_eq!(migration_report.migrated_type_count, 1);
    assert_eq!(migration_report.migrated_entity_count, 1);

    let trail = world
        .get_component::<Trail>(entity)
        .expect("data survived the migration");
    assert_eq!(trail.points.as_slice(), &[3.5, 4.5]);
    assert_eq!(trail.weight, 9);
    let block_address = trail.points.as_ptr();

    // The unchanged-schema pass has no changed type names, so it visits no
    // value at all: the block stays where it is and `block_address` still holds.
    let previous_metadata_by_name = world.capture_persist_type_metadata();
    let migration_report = world.migrate_changed_persistable_components(
        &previous_metadata_by_name,
        &HashSet::new(),
        None,
    );
    assert_eq!(migration_report.migrated_type_count, 0);
    let trail = world
        .get_component::<Trail>(entity)
        .expect("data survived the fast path");
    assert_eq!(trail.points.as_ptr(), block_address);
    assert_eq!(trail.points.as_slice(), &[3.5, 4.5]);

    drop(world);
    assert_eq!(native_buffer::live_bytes(), 0);
}
