//! Integration tests for the layout checks on a descriptor component column.
//!
//! A descriptor component is one whose type lives in another language, so the
//! column stores its rows as raw bytes. `ComponentColumn` copies rows with
//! `ptr::copy`, frees them without running element destructors, and is
//! `Send + Sync` because the `Blittability` its layout carries is evidence
//! that the rows are plain data.
//!
//! # Responsibilities
//!
//! - Pin the layout shapes the engine must refuse, as typed errors.
//! - Cover both places a layout is checked: when the column is built and when
//!   it first grows.

use pill_engine::archetype::{Blittability, ColumnLayout, ComponentColumn};
use pill_engine::world::WorldError;

/// A layout that the constructor refuses comes back as the error naming the
/// failed check, and a layout assembled without the constructor is refused by
/// the column instead.
#[test]
fn degenerate_layouts_are_typed_errors() {
    // SAFETY: this test never builds a column from these layouts. Each is
    // refused before one exists, so the plain-data claim is never relied on.
    let witness = unsafe { Blittability::assume() };
    assert!(matches!(
        ColumnLayout::new(0, 4, 1, witness),
        Err(WorldError::DescriptorSizeZero)
    ));
    assert!(matches!(
        ColumnLayout::new(4, 3, 1, witness),
        Err(WorldError::DescriptorAlignmentInvalid)
    ));
    assert!(matches!(
        ColumnLayout::new(usize::MAX, 1, 1, witness),
        Err(WorldError::DescriptorLayoutInvalid)
    ));

    // The fields are public, so a column can be handed a layout the
    // constructor would have refused. It validates again in every build
    // profile, so a release-profile build cannot accept these either.
    let zero_sized = ColumnLayout {
        size: 0,
        align: 4,
        schema_hash: 1,
        blittability: Blittability::from_manifest_fields(),
    };
    assert!(matches!(
        ComponentColumn::new(zero_sized),
        Err(WorldError::DescriptorSizeZero)
    ));

    let misaligned = ColumnLayout {
        size: 4,
        align: 3,
        schema_hash: 1,
        blittability: Blittability::from_manifest_fields(),
    };
    assert!(matches!(
        ComponentColumn::new(misaligned),
        Err(WorldError::DescriptorAlignmentInvalid)
    ));
}

/// An element whose size cannot be multiplied by the first growth's capacity
/// is reported as a layout error rather than tripping an `expect`.
#[test]
fn an_unrepresentable_growth_is_a_layout_error() {
    // One element of this size still describes a layout. Four of them overflow
    // `usize`, so `grow_to` refuses the buffer size before it allocates.
    let unallocatable_growth = ColumnLayout {
        size: usize::MAX / 4 + 8,
        align: 8,
        schema_hash: 1,
        blittability: Blittability::from_manifest_fields(),
    };
    let mut column = ComponentColumn::new(unallocatable_growth).expect("one element still fits");
    assert!(matches!(
        column.push_zeroed(),
        Err(WorldError::DescriptorLayoutInvalid)
    ));
    assert_eq!(column.len(), 0, "a refused growth leaves the rows alone");
}
