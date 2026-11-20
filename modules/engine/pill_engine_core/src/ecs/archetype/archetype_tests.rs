//! Unit tests for the byte-storage pieces an archetype is built from:
//! [`ColumnLayout`], [`FieldPlan`] and [`ComponentColumn`].
//!
//! # Responsibilities
//!
//! - Exercises column layout validation, byte-row moves and tick
//!   migration across the archetype's unsafe storage paths.
//!
//! # Design
//!
//! Kept beside the code they exercise, because the storage paths are where
//! coverage matters most. A sibling file also lets `archetype.rs` read as
//! implementation, while these tests still reach private items through
//! `super::*`.

// Current crate
use super::*;
use crate::component::Tick;

/// The layout of a row of two `u32` fields: `a` at byte 0, `b` at byte 4.
fn two_field_layout() -> ColumnLayout {
    ColumnLayout {
        size: 8,
        align: 4,
        schema_hash: 1,
        blittability: Blittability::engine_verified(),
    }
}

#[cfg(feature = "development")]
fn column_with_rows(rows: &[[u8; 8]]) -> ComponentColumn {
    let mut column = ComponentColumn::new(two_field_layout()).expect("a valid layout");
    for row in rows {
        column.push_bytes(row).expect("row matches the layout");
    }
    column
}

// =============================================================================
// Column layouts and field plans
// =============================================================================

/// A layout keeps the `Blittability` value it was given, and a column frees
/// each live row exactly once through its ops table - both in `Drop` and in
/// `swap_remove_discard`.
#[test]
fn a_layout_keeps_its_witness_and_a_column_releases_through_its_ops() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let constructed_layout =
        ColumnLayout::new(4, 4, 7, Blittability::from_manifest_fields()).expect("a plain layout");
    assert_eq!(
        constructed_layout.blittability,
        Blittability::from_manifest_fields(),
        "the constructor stores the witness it was handed"
    );

    // Freeing a row is the ops table's job, not a hook on the layout, and the
    // same table serves both call sites: `Drop` and `swap_remove_discard`. A
    // plain-data table reports `trivial_drop` and is never called; a Rust
    // type's table runs the element type's destructor. Counting the calls
    // shows that each site frees its row exactly once.
    static RELEASE_COUNT: AtomicUsize = AtomicUsize::new(0);
    unsafe fn count_releases(_row_pointer: *mut u8, row_count: usize) {
        RELEASE_COUNT.fetch_add(row_count, Ordering::SeqCst);
    }

    let layout =
        ColumnLayout::new(4, 4, 7, Blittability::from_manifest_fields()).expect("a valid layout");
    let mut column = ComponentColumn::new(layout).expect("a column");
    // Rows go in while the column still reports plain data, because the byte
    // mutators refuse a column whose rows own resources. The counting table is
    // installed afterwards. A native column gets its table when it is built,
    // from `T`'s own drop code, and is filled with `push`, which is why this
    // test re-points a descriptor column instead.
    column
        .push_zeroed()
        .expect("the first row grows the column");
    column.push_zeroed().expect("the second row fits");
    column.refresh_ops(ColumnOps {
        drop_range: count_releases,
        trivial_drop: false,
    });
    column.swap_remove_discard(0);
    assert_eq!(
        RELEASE_COUNT.load(Ordering::SeqCst),
        1,
        "swap_remove_discard releases the row it overwrites"
    );

    drop(column);
    assert_eq!(
        RELEASE_COUNT.load(Ordering::SeqCst),
        2,
        "Drop releases the one row still live"
    );
}

#[test]
fn plan_between_matches_fields_by_name_not_by_position() {
    let old = [
        LayoutField {
            name: "a",
            type_tag: "u32",
            offset: 0,
            size: 4,
        },
        LayoutField {
            name: "b",
            type_tag: "u32",
            offset: 4,
            size: 4,
        },
    ];
    // The same two fields, listed in the opposite order in the new layout.
    let new = [
        LayoutField {
            name: "b",
            type_tag: "u32",
            offset: 0,
            size: 4,
        },
        LayoutField {
            name: "a",
            type_tag: "u32",
            offset: 4,
            size: 4,
        },
    ];

    let plan = FieldPlan::between(&old, &new);

    assert_eq!(
        plan.fields(),
        &[
            PlannedField {
                offset: 0,
                bytes: 4,
                source: FieldSource::OldOffset(4),
            },
            PlannedField {
                offset: 4,
                bytes: 4,
                source: FieldSource::OldOffset(0),
            },
        ]
    );
}

#[test]
fn plan_between_zeroes_the_new_fields_and_omits_the_removed_ones() {
    let old = [
        LayoutField {
            name: "kept",
            type_tag: "u32",
            offset: 0,
            size: 4,
        },
        LayoutField {
            name: "removed",
            type_tag: "u32",
            offset: 4,
            size: 4,
        },
    ];
    let new = [
        LayoutField {
            name: "kept",
            type_tag: "u32",
            offset: 0,
            size: 4,
        },
        LayoutField {
            name: "added",
            type_tag: "u32",
            offset: 4,
            size: 8,
        },
    ];

    let plan = FieldPlan::between(&old, &new);

    // The removed field leaves no instruction at all. The added field is an
    // explicit zero fill, not a missing entry.
    assert_eq!(
        plan.fields(),
        &[
            PlannedField {
                offset: 0,
                bytes: 4,
                source: FieldSource::OldOffset(0),
            },
            PlannedField {
                offset: 4,
                bytes: 8,
                source: FieldSource::ZeroFill,
            },
        ]
    );
}

#[test]
fn plan_between_copies_the_smaller_of_two_sizes() {
    let old = [LayoutField {
        name: "grew",
        type_tag: "u32",
        offset: 0,
        size: 4,
    }];
    let new = [LayoutField {
        name: "grew",
        type_tag: "u32",
        offset: 0,
        size: 8,
    }];
    assert_eq!(
        FieldPlan::between(&old, &new).fields()[0].bytes,
        4,
        "the tail of a grown field has to come from the zero fill, not the old row"
    );

    let old = [LayoutField {
        name: "shrank",
        type_tag: "u32",
        offset: 0,
        size: 8,
    }];
    let new = [LayoutField {
        name: "shrank",
        type_tag: "u32",
        offset: 0,
        size: 4,
    }];
    assert_eq!(FieldPlan::between(&old, &new).fields()[0].bytes, 4);
}

/// A field that keeps its name but changes type is reset, not reinterpreted.
#[test]
fn plan_between_resets_a_field_whose_type_changed() {
    let old = [LayoutField {
        name: "health",
        type_tag: "i32",
        offset: 0,
        size: 4,
    }];
    let new = [LayoutField {
        name: "health",
        type_tag: "f32",
        offset: 0,
        size: 4,
    }];

    let plan = FieldPlan::between(&old, &new);

    // Copying would carry the bits across: `5i32` read as `f32` is 7e-45, so
    // the same name would hold a different number.
    assert_eq!(
        plan.fields(),
        &[PlannedField {
            offset: 0,
            bytes: 4,
            source: FieldSource::ZeroFill,
        }],
        "a retyped field takes the zero fill, not the old bytes"
    );
    assert_eq!(
        plan.retyped_fields(),
        &["health".to_string()],
        "and the reset is recorded so the caller can report it"
    );
}

/// A field that keeps its name and type still carries its bytes over.
#[test]
fn plan_between_copies_a_field_whose_type_is_unchanged() {
    let old = [LayoutField {
        name: "health",
        type_tag: "i32",
        offset: 8,
        size: 4,
    }];
    let new = [LayoutField {
        name: "health",
        type_tag: "i32",
        offset: 0,
        size: 4,
    }];

    let plan = FieldPlan::between(&old, &new);

    assert_eq!(
        plan.fields(),
        &[PlannedField {
            offset: 0,
            bytes: 4,
            source: FieldSource::OldOffset(8),
        }],
        "a moved field follows its name"
    );
    assert!(
        plan.retyped_fields().is_empty(),
        "nothing was reset, so nothing is reported"
    );
}

#[test]
fn applying_a_plan_writes_every_instruction() {
    // One instruction of each kind: a copied value, a zero fill and a default.
    let mut plan = FieldPlan::new();
    plan.push(0, 4, FieldSource::OldOffset(8));
    plan.push(4, 4, FieldSource::ZeroFill);
    plan.push(8, 4, FieldSource::ZeroFill);
    plan.set_default(8, &[7, 0, 0, 0])
        .expect("the field is planned");

    let source = [1_u8; 16];
    let mut destination = [0_u8; 12];
    plan.validate(source.len(), destination.len())
        .expect("the plan fits both rows");
    plan.apply(&source, &mut destination);

    assert_eq!(&destination[..4], &[1, 1, 1, 1], "an old offset is copied");
    assert_eq!(
        &destination[4..8],
        &[0, 0, 0, 0],
        "a zero fill leaves the destination's zero"
    );
    assert_eq!(
        &destination[8..12],
        &[7, 0, 0, 0],
        "a default lands even though the source has bytes there"
    );
}

#[cfg(feature = "development")]
#[test]
fn relayout_moves_rows_through_the_plan() {
    // Two rows, `a` and `b` each holding a distinct u32.
    let mut column = column_with_rows(&[[1, 0, 0, 0, 2, 0, 0, 0], [3, 0, 0, 0, 4, 0, 0, 0]]);
    let old = [
        LayoutField {
            name: "a",
            type_tag: "u32",
            offset: 0,
            size: 4,
        },
        LayoutField {
            name: "b",
            type_tag: "u32",
            offset: 4,
            size: 4,
        },
    ];
    // `b` first, then `a`, then a new eight-byte field.
    let new = [
        LayoutField {
            name: "b",
            type_tag: "u32",
            offset: 0,
            size: 4,
        },
        LayoutField {
            name: "a",
            type_tag: "u32",
            offset: 4,
            size: 4,
        },
        LayoutField {
            name: "added",
            type_tag: "u32",
            offset: 8,
            size: 8,
        },
    ];
    let plan = FieldPlan::between(&old, &new);

    let moved_row_count = column
        .relayout(
            ColumnLayout {
                size: 16,
                align: 8,
                schema_hash: 2,
                blittability: Blittability::engine_verified(),
            },
            &plan,
        )
        .expect("the plan fits both layouts");

    assert_eq!(moved_row_count, 2);
    assert_eq!(
        column.bytes(0).unwrap(),
        [2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].as_slice()
    );
    assert_eq!(
        column.bytes(1).unwrap(),
        [4, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].as_slice()
    );
    assert_eq!(column.element_size(), 16);
    assert_eq!(column.schema_hash(), 2);
}

#[cfg(feature = "development")]
#[test]
fn relayout_of_an_equal_shape_rewrites_rows_in_place() {
    let mut column = column_with_rows(&[[1, 0, 0, 0, 2, 0, 0, 0]]);
    let pointer_before = column.as_mut_ptr();
    let plan = FieldPlan::between(
        &[
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
        ],
        &[
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
        ],
    );

    column
        .relayout(
            ColumnLayout {
                size: 8,
                align: 4,
                schema_hash: 3,
                blittability: Blittability::engine_verified(),
            },
            &plan,
        )
        .expect("an equal-shape relayout of a fitting plan");

    assert_eq!(
        column.as_mut_ptr(),
        pointer_before,
        "an unchanged shape must not reallocate: pointers into it may be live"
    );
    // The scratch copy is what makes this correct: `b` overwrites `a`'s
    // bytes before `a` has been read out of the same buffer.
    assert_eq!(
        column.bytes(0).unwrap(),
        [2, 0, 0, 0, 1, 0, 0, 0].as_slice()
    );
}

#[cfg(feature = "development")]
#[test]
fn relayout_keeps_the_row_count_and_capacity() {
    let mut column = column_with_rows(&[[1, 0, 0, 0, 2, 0, 0, 0], [3, 0, 0, 0, 4, 0, 0, 0]]);
    let plan = FieldPlan::new();

    column
        .relayout(
            ColumnLayout {
                size: 4,
                align: 4,
                schema_hash: 4,
                blittability: Blittability::engine_verified(),
            },
            &plan,
        )
        .expect("an empty plan always fits");

    assert_eq!(column.len(), 2);
    assert_eq!(column.bytes(0).unwrap(), [0, 0, 0, 0].as_slice());
    assert_eq!(column.bytes(1).unwrap(), [0, 0, 0, 0].as_slice());
    // Capacity was kept, so the next push still has its spare slot.
    column
        .push_bytes(&[9, 0, 0, 0])
        .expect("fits the kept capacity");
    assert_eq!(column.len(), 3);
}

#[cfg(feature = "development")]
#[test]
fn relayout_refuses_a_plan_that_leaves_a_row() {
    let mut column = column_with_rows(&[[1, 0, 0, 0, 2, 0, 0, 0]]);
    let mut plan = FieldPlan::new();
    plan.push(4, 8, FieldSource::OldOffset(0));

    let relayout_result = column.relayout(
        ColumnLayout {
            size: 8,
            align: 4,
            schema_hash: 1,
            blittability: Blittability::engine_verified(),
        },
        &plan,
    );

    assert!(matches!(
        relayout_result,
        Err(WorldError::DescriptorRowInvalid)
    ));
    assert_eq!(
        column.bytes(0).unwrap(),
        [1, 0, 0, 0, 2, 0, 0, 0].as_slice(),
        "a refused plan must not touch the rows"
    );
}

#[cfg(feature = "development")]
#[test]
fn relayout_refuses_a_layout_that_cannot_be_allocated() {
    let mut column = column_with_rows(&[]);
    assert!(matches!(
        column.relayout(
            ColumnLayout {
                size: 0,
                align: 4,
                schema_hash: 1,
                blittability: Blittability::engine_verified()
            },
            &FieldPlan::new()
        ),
        Err(WorldError::DescriptorSizeZero)
    ));
    assert!(matches!(
        column.relayout(
            ColumnLayout {
                size: 4,
                align: 3,
                schema_hash: 1,
                blittability: Blittability::engine_verified()
            },
            &FieldPlan::new()
        ),
        Err(WorldError::DescriptorAlignmentInvalid)
    ));
}

// =============================================================================
// ComponentColumn: identity, ops, ticks and relayout
// =============================================================================

/// A native element type with a real destructor, so a plain-data column and a
/// typed one can be told apart by what they do, not by how they were built.
#[derive(Debug, PartialEq)]
struct DropCounted(u32);

thread_local! {
    static DROP_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Drop for DropCounted {
    fn drop(&mut self) {
        DROP_COUNT.with(|count| count.set(count.get() + 1));
    }
}

/// A native column round-trips typed values through the byte buffer.
#[test]
fn a_native_column_pushes_and_reads_typed_rows() {
    let mut column = ComponentColumn::new_native::<u64>(7, false).expect("layout");
    column.push::<u64>(11);
    column.push::<u64>(22);

    assert_eq!(column.len(), 2);
    assert_eq!(*column.get::<u64>(0), 11);
    assert_eq!(*column.get::<u64>(1), 22);
    assert_eq!(column.as_slice::<u64>(), &[11, 22]);

    *column.get_mut::<u64>(0) = 33;
    assert_eq!(*column.get::<u64>(0), 33);
    assert_eq!(column.swap_remove::<u64>(0), 33);
    assert_eq!(column.as_slice::<u64>(), &[22]);
}

/// The element-type check refuses a `T` the column does not hold.
#[test]
#[should_panic(expected = "different component type")]
fn a_native_column_refuses_a_foreign_type() {
    let column = ComponentColumn::new_native::<u64>(7, false).expect("layout");
    let _ = column.as_slice::<u32>();
}

/// A descriptor-only column has no typed reading at all.
#[test]
#[should_panic(expected = "descriptor-only column has no Rust type")]
fn a_descriptor_column_refuses_typed_access() {
    let layout = ColumnLayout::new(8, 4, 7, Blittability::engine_verified()).expect("layout");
    let column = ComponentColumn::new(layout).expect("column");
    let _ = column.as_slice::<u64>();
}

/// A shared column checks the row layout instead of `TypeId`, because one
/// type compiled into two binaries has a different `TypeId` in each.
#[test]
fn a_shared_column_accepts_a_layout_compatible_type() {
    let mut column = ComponentColumn::new_native::<u64>(7, true).expect("layout");
    column.push::<u64>(5);
    // A different type of the same shape is what the second binary's `T`
    // looks like from here.
    assert_eq!(*column.get::<i64>(0), 5);
}

/// The same column still refuses a type whose layout disagrees.
#[test]
#[should_panic(expected = "shared column layout")]
fn a_shared_column_refuses_a_layout_mismatch() {
    let column = ComponentColumn::new_native::<u64>(7, true).expect("layout");
    let _ = column.as_slice::<u32>();
}

/// The plain-data ops table says there is nothing to drop, and the generated
/// table says what the element type really does.
#[test]
fn ops_report_the_drop_behaviour_of_their_column() {
    let layout = ColumnLayout::new(4, 4, 7, Blittability::engine_verified()).expect("layout");
    assert!(
        ComponentColumn::new(layout)
            .expect("column")
            .ops()
            .trivial_drop
    );
    assert!(
        ComponentColumn::new_native::<u64>(7, false)
            .expect("column")
            .ops()
            .trivial_drop
    );
    assert!(
        !ComponentColumn::new_native::<DropCounted>(7, false)
            .expect("column")
            .ops()
            .trivial_drop
    );
}

/// The generated table drops exactly the rows it is given, once each.
#[test]
fn generated_ops_drop_each_row_once() {
    let mut column = ComponentColumn::new_native::<DropCounted>(7, false).expect("column");
    column.push(DropCounted(1));
    column.push(DropCounted(2));
    DROP_COUNT.with(|count| count.set(0));

    let ops = column.ops();
    // SAFETY: the column holds exactly two initialized `DropCounted` rows,
    // and nothing reads them afterwards.
    unsafe { (ops.drop_range)(column.as_mut_ptr(), 2) };
    assert_eq!(DROP_COUNT.with(std::cell::Cell::get), 2);

    // The rows were already dropped above, so the column must not drop them
    // again.
    std::mem::forget(column);
}

/// A column's tick list stays in step with its rows through every operation
/// that adds or removes a row.
///
/// The ticks live in the same struct as the rows, so there is one length to
/// keep, and the mismatch a separately kept tick map could suffer cannot
/// happen here. This test checks each mutator that changes the row set.
#[test]
fn a_columns_ticks_track_its_rows_through_every_mutator() {
    let mut column = ComponentColumn::new_native::<u32>(7, false).expect("column");
    assert_eq!(column.ticks().len(), column.len());

    column.push::<u32>(1);
    column.push::<u32>(2);
    column.push::<u32>(3);
    assert_eq!(column.ticks().len(), 3);
    assert_eq!(column.ticks().len(), column.len());

    column.set_row_ticks(1, ComponentTicks::new(Tick(42)));
    assert_eq!(column.row_ticks(1).expect("row 1 exists").added, Tick(42));

    // A swap-remove moves the tail tick over the hole exactly as it moves
    // the tail row, so row 1 keeps carrying row 1's metadata.
    column.swap_remove_discard(0);
    assert_eq!(column.ticks().len(), 2);
    assert_eq!(column.ticks().len(), column.len());
    assert_eq!(column.row_ticks(1).expect("row 1 exists").added, Tick(42));

    assert_eq!(column.swap_remove::<u32>(0), 3);
    assert_eq!(column.ticks().len(), column.len());

    // A move hands the row and its metadata to the destination together.
    let mut destination = ComponentColumn::new_native::<u32>(7, false).expect("column");
    destination
        .take_row_from(&mut column, 0)
        .expect("the two columns have one shape");
    assert_eq!(column.ticks().len(), column.len());
    assert_eq!(destination.ticks().len(), destination.len());
    assert_eq!(
        destination.row_ticks(0).expect("the moved row").added,
        Tick(42),
        "a moved row keeps the ticks it was carrying"
    );

    // The byte mutators keep the two in step as well.
    let mut descriptor_column = ComponentColumn::new(two_field_layout()).expect("column");
    descriptor_column.push_zeroed().expect("push");
    descriptor_column.push_bytes(&[0; 8]).expect("push");
    assert_eq!(descriptor_column.ticks().len(), 2);
    assert_eq!(descriptor_column.ticks().len(), descriptor_column.len());
    descriptor_column.swap_remove_discard(0);
    assert_eq!(descriptor_column.ticks().len(), descriptor_column.len());
}

/// A relayout reshapes the rows without disturbing their change ticks.
///
/// This relayout reallocates the column's buffer, and the ticks have to come
/// across with it. A reshaped component has not been added or changed, so a
/// system that filters on `Changed<T>` must not see the whole world light up
/// after a hot reload that only moved a field.
#[cfg(feature = "development")]
#[test]
fn a_relayout_carries_the_column_ticks_across_the_reallocation() {
    let mut column = ComponentColumn::new(two_field_layout()).expect("column");
    column.push_bytes(&[1, 0, 0, 0, 2, 0, 0, 0]).expect("push");
    column.push_bytes(&[3, 0, 0, 0, 4, 0, 0, 0]).expect("push");
    column.set_row_ticks(0, ComponentTicks::new(Tick(11)));
    column.set_row_ticks(1, ComponentTicks::new(Tick(22)));

    // A wider, more strongly aligned shape forces a fresh allocation.
    let widened_layout =
        ColumnLayout::new(16, 8, 77, Blittability::engine_verified()).expect("a valid layout");
    column
        .relayout(widened_layout, &FieldPlan::new())
        .expect("an empty plan zeroes every row");

    assert_eq!(column.ticks().len(), column.len());
    assert_eq!(column.row_ticks(0).expect("row 0").added, Tick(11));
    assert_eq!(column.row_ticks(1).expect("row 1").added, Tick(22));
}

/// Reserving space does not change what the column contains.
#[test]
fn reserving_rows_preserves_contents() {
    let mut column = ComponentColumn::new_native::<u32>(7, false).expect("column");
    column.push::<u32>(9);
    column.reserve_rows(64).expect("reserve");
    assert!(column.capacity >= 65);
    assert_eq!(column.as_slice::<u32>(), &[9]);
    column.push::<u32>(10);
    assert_eq!(column.as_slice::<u32>(), &[9, 10]);
}

/// A descriptor column that gains alignment moves its rows into a new buffer
/// instead of reinterpreting the old one.
///
/// The relayout allocates a buffer aligned for the new shape and writes every
/// row into it at the new stride, and the test checks that the buffer really is
/// 8-aligned. A native column refuses a registration whose alignment grows,
/// because its reload reads old rows with the incoming type. A descriptor
/// column has no typed read at all: its rows are only reached as bytes at the
/// layout's stride, so the buffer's alignment is what makes those reads valid.
#[cfg(feature = "development")]
#[test]
fn a_descriptor_relayout_that_widens_alignment_reallocates() {
    // A 12-byte row at align 4: three 4-byte values.
    let narrow_layout =
        ColumnLayout::new(12, 4, 1, Blittability::engine_verified()).expect("layout");
    let mut column = ComponentColumn::new(narrow_layout).expect("column");
    column
        .push_bytes(&[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0])
        .expect("push");
    column
        .push_bytes(&[4, 0, 0, 0, 5, 0, 0, 0, 6, 0, 0, 0])
        .expect("push");

    // Widen the row to align 8. A native column refuses this change at
    // registration.
    let widened_layout =
        ColumnLayout::new(16, 8, 2, Blittability::engine_verified()).expect("layout");
    let mut plan = FieldPlan::new();
    plan.push(0, 4, FieldSource::OldOffset(0));
    plan.push(8, 4, FieldSource::OldOffset(4));
    let migrated_row_count = column.relayout(widened_layout, &plan).expect("relayout");

    assert_eq!(migrated_row_count, 2, "both rows migrate");
    assert_eq!(column.len(), 2, "row count and order survive");
    assert_eq!(column.alignment(), 8);
    assert_eq!(
        column.as_mut_ptr() as usize % 8,
        0,
        "the new buffer really is 8-aligned, which is what the native lane could not promise"
    );
    assert_eq!(&column.bytes(0).expect("row")[0..4], &[1, 0, 0, 0]);
    assert_eq!(&column.bytes(0).expect("row")[8..12], &[2, 0, 0, 0]);
    assert_eq!(&column.bytes(1).expect("row")[0..4], &[4, 0, 0, 0]);
}
