//! Small general-purpose helpers shared by Pill crates.
//!
//! # Responsibilities
//!
//! - Reflect type and enum-variant names for diagnostics and tooling.
//! - Define the error an asset loader reports for a refused path
//!   ([`AssetPathError`]).
//! - Construct and inspect fixed-width bitmasks.
//! - Format errors and their cause chains for reporting.
//!
//! # Design
//!
//! The helpers here keep no engine state. The single error type,
//! [`AssetPathError`], is defined here with `thiserror`, so a loader can
//! match its variants without depending on an engine crate. The bitmask
//! helpers number bits from the most significant bit (index 0) to the least.

// Standard library
use std::{
    any::type_name,
    fmt::Binary,
    ops::{Add, Not, Shl, Sub},
};

// =============================================================================
// Types
// =============================================================================

/// The reason an asset loader refused a path: it names nothing usable, or its
/// format is not one the loader accepts.
///
/// Loaders that check a path before reading it report this type;
/// `pill_audio`'s `Sound::load` is one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssetPathError {
    /// The path names nothing usable: it does not exist, or it carries no file
    /// extension.
    #[error("asset path is not usable: {path}")]
    InvalidPath {
        /// The offending path, for diagnostics.
        path: String,
    },
    /// The file extension is not among the allowed formats.
    #[error("asset format `{extension}` is not allowed (expected one of: {allowed})")]
    InvalidFormat {
        /// The file extension that was rejected.
        extension: String,
        /// Comma-separated list of allowed formats.
        allowed: String,
    },
}

// =============================================================================
// Type-name reflection
// =============================================================================

/// Returns the short name of `T`: the part of its full type name after the
/// last `:`, so `std::vec::Vec<u8>` reads as `Vec<u8>`.
///
/// # Examples
///
/// ```
/// use pill_core::utils::get_type_name;
///
/// assert_eq!(get_type_name::<Vec<u8>>(), "Vec<u8>");
/// assert_eq!(get_type_name::<u32>(), "u32");
/// ```
pub fn get_type_name<T>() -> String {
    let full_type_name = type_name::<T>();
    // Strip any leading module path so only the type's short name remains.
    full_type_name
        .rsplit_once(':')
        .map_or(full_type_name, |(_, name)| name)
        .to_string()
}

/// Returns the short name of the type of `value`: the part of its full type
/// name after the last `:`.
///
/// Only the type of `value` is used; the value itself is not read.
///
/// # Examples
///
/// ```
/// use pill_core::utils::get_value_type_name;
///
/// let value = 42u32;
/// assert_eq!(get_value_type_name(&value), "u32");
/// ```
pub fn get_value_type_name<T>(_: &T) -> String {
    let full_type_name = type_name::<T>();
    // Strip any leading module path so only the type's short name remains.
    full_type_name
        .rsplit_once(':')
        .map_or(full_type_name, |(_, name)| name)
        .to_string()
}

/// Returns whether two values are the same enum variant.
///
/// It compares [`std::mem::discriminant`], so payload values are ignored.
///
/// # Examples
///
/// ```
/// use pill_core::utils::enum_variant_eq;
///
/// assert!(enum_variant_eq(&Some(1), &Some(2)));
/// assert!(!enum_variant_eq(&Some(1), &None::<i32>));
/// ```
pub fn enum_variant_eq<T>(first: &T, second: &T) -> bool {
    std::mem::discriminant(first) == std::mem::discriminant(second)
}

/// Returns the name of the enum variant `value` holds, read from its `Debug`
/// output.
///
/// Everything from the first `(` onward is dropped, which removes a tuple
/// variant's payload. When that output has no `(`, the whole output comes
/// back: that is the case for a struct, or for a struct-shaped variant.
///
/// `value` must implement `Debug`.
///
/// # Examples
///
/// ```
/// use pill_core::utils::get_enum_variant_type_name;
///
/// assert_eq!(get_enum_variant_type_name(&Some(2.0)), "Some");
/// assert_eq!(get_enum_variant_type_name(&None::<f32>), "None");
/// ```
pub fn get_enum_variant_type_name<T: core::fmt::Debug>(value: &T) -> String {
    // The `Debug` output starts with the variant name and then, for a tuple
    // variant, a parenthesised payload that is dropped here.
    let debug_representation = format!("{value:?}");
    let name_before_payload = debug_representation
        .split('(')
        .next()
        .unwrap_or(&debug_representation);
    name_before_payload.trim().to_string()
}

// =============================================================================
// Error-chain formatting
// =============================================================================

/// Formats an error and its whole cause chain into one message.
///
/// The first line holds the top-level error, prefixed with
/// `Pill project error: `. Each cause follows on its own line, indented and
/// numbered from 0 in chain order.
///
/// # Examples
///
/// ```
/// use pill_core::utils::get_project_error_message;
///
/// let error = std::io::Error::new(std::io::ErrorKind::NotFound, "missing preset");
/// let message = get_project_error_message(Err(error)).expect("an error formats");
/// assert!(message.starts_with("Pill project error: missing preset"));
/// assert!(get_project_error_message(Ok::<(), std::io::Error>(())).is_none());
/// ```
pub fn get_project_error_message<E: std::error::Error>(result: Result<(), E>) -> Option<String> {
    result.err().map(|error| {
        let mut message = format!("Pill project error: {error}\n");
        // Walk the error's `source` chain, indenting each cause in order so
        // the report reads top-down from the outermost to the innermost error.
        let mut source = error.source();
        let mut cause_index = 0usize;
        while let Some(inner_source) = source {
            message.push_str(&format!("  {cause_index}: {inner_source}\n"));
            cause_index += 1;
            source = inner_source.source();
        }
        message
    })
}

/// Formats an error and its whole cause chain on a single line.
///
/// Printing an error with `Display` shows only the outermost message. The
/// cause that names the actual problem - which import could not be resolved,
/// or which file was not found - is only in the chain, not in that text. This
/// function appends every cause in order, separated by `: `.
///
/// Use [`get_project_error_message`] when a multi-line report is wanted. This
/// one is shaped for a single log line.
///
/// # Examples
///
/// ```
/// use pill_core::utils::format_error_chain;
///
/// let error = std::io::Error::new(std::io::ErrorKind::NotFound, "missing preset");
/// assert_eq!(format_error_chain(&error), "missing preset");
/// ```
pub fn format_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    // Walk the causes inwards, so the line reads in the order they happened.
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

// =============================================================================
// Bitmask helpers
// =============================================================================

/// Builds a bitmask as wide as `T` with one contiguous run of set bits.
///
/// Bits are numbered from the most significant bit (MSB, index 0) to the
/// least significant. `mask_range` gives the run to set, and both ends count:
/// `0..4` sets five bits, at indexes 0 to 4.
///
/// # Examples
///
/// ```
/// use pill_core::utils::create_bitmask_from_range;
///
/// let mask = create_bitmask_from_range::<u16>(&(0..4));
/// assert_eq!(mask, 0b1111_1000_0000_0000);
/// ```
///
/// # Panics
///
/// Panics when `mask_range.end` is at or past the bit width of `T`, or when
/// `mask_range.start` is greater than `mask_range.end`. A reversed range has
/// no run to set, and its length would underflow.
pub fn create_bitmask_from_range<T>(mask_range: &core::ops::Range<T>) -> T
where
    T: Copy
        + Default
        + Binary
        + From<u8>
        + Ord
        + Shl<Output = T>
        + Sub<Output = T>
        + Add<Output = T>
        + Not<Output = T>,
{
    // Step 1: Compute the bit width of `T` and reject a range whose end
    // reaches or exceeds it.
    let bit_width = T::from(std::mem::size_of::<T>() as u8 * 8);
    if mask_range.end >= bit_width {
        panic!("Provided mask range exceeds mask size");
    }

    // Step 2: Reject a reversed range. Its length would underflow, so it has
    // no run of bits to set.
    if mask_range.start > mask_range.end {
        panic!("Provided mask range is reversed");
    }

    // Step 3: Work out the length of the run and the shift that places it at
    // the MSB end.
    let range_length: T = mask_range.end - mask_range.start + T::from(1);
    let mask_shift = bit_width - mask_range.end - T::from(1);

    // Step 4: Build the mask, handling a full-width run separately because
    // shifting by the full bit width would overflow.
    match range_length == bit_width {
        true => !(T::from(0)) << mask_shift,
        false => !(!T::from(0) << range_length) << mask_shift,
    }
}

/// Builds a `u16` bitmask with one bit set, at `index`.
///
/// Bits are numbered from the most significant bit (index 0) to the least
/// significant (index 15). An index outside `0..=15` gives a zero mask.
///
/// # Examples
///
/// ```
/// use pill_core::utils::create_bitmask_with_one;
///
/// assert_eq!(create_bitmask_with_one(3), 0b0001_0000_0000_0000);
/// assert_eq!(create_bitmask_with_one(16), 0b0000_0000_0000_0000);
/// ```
pub fn create_bitmask_with_one(index: u16) -> u16 {
    /// Most significant bit of a `u16` mask (bit index 0).
    pub const FIRST_BIT: u16 = 0b1000_0000_0000_0000;
    let mut mask: u16 = 0b0000_0000_0000_0000;
    // Step 1: Ignore indexes outside the 16-bit mask width, leaving the
    // all-zero mask untouched.
    if (0_u16..=15_u16).contains(&index) {
        // Step 2: Set the MSB, then shift it right `index` times to reach
        // the requested bit position.
        mask |= FIRST_BIT;
        for _ in 0..index {
            mask >>= 1;
        }
    }
    mask
}

/// Returns the index of every set bit in a `u16` bitmask, counted from the
/// most significant bit (index 0).
///
/// # Examples
///
/// ```
/// use pill_core::utils::get_indices_of_set_elements;
///
/// assert_eq!(
///     get_indices_of_set_elements(0b0001_0000_0100_0001),
///     vec![3, 9, 15],
/// );
/// ```
pub fn get_indices_of_set_elements(bitmask: u16) -> Vec<usize> {
    // Walk the mask from the most significant bit (index 0) to the least
    // significant (index 15), recording the position of every set bit.
    let mut single_bit_mask: u16 = 0b1000_0000_0000_0000;
    let mut set_bit_indices = Vec::<usize>::new();
    for i in 0..=15 {
        if bitmask & single_bit_mask > 0 {
            set_bit_indices.push(i);
        }
        single_bit_mask >>= 1;
    }
    set_bit_indices
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // Current crate
    use super::*;

    /// The documented example: `0..4` sets the five most significant bits of
    /// a `u16`.
    #[test]
    fn a_range_sets_its_run_of_bits() {
        assert_eq!(
            create_bitmask_from_range::<u16>(&(0..4)),
            0b1111_1000_0000_0000
        );
    }

    /// A reversed range is a caller error, not a request for a mask: its
    /// length would underflow, so there is no run of bits to set.
    #[test]
    #[should_panic(expected = "Provided mask range is reversed")]
    fn a_reversed_range_is_rejected() {
        // Built from bindings: a reversed literal range is itself a lint.
        let start = 5_u16;
        let end = 3_u16;
        let _ = create_bitmask_from_range::<u16>(&(start..end));
    }
}
