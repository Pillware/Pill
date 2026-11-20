//! Inline, fixed-capacity strings for plain-data structs.
//!
//! # Responsibilities
//!
//! - Provide [`FixedString64`]: a UTF-8 string stored in the value itself,
//!   never on the heap.
//!
//! # Design
//!
//! The engine copies values between DLLs, so a value it moves across must be
//! plain data: `#[repr(C)]`, no pointers, no heap allocation. A `String`
//! fails that in two ways. Its buffer belongs to the allocator of the DLL
//! that created it, so after a hot reload one library can free memory another
//! allocated. And a struct holding a `String` has no fixed C layout, so it
//! cannot be read across the module boundary or mirrored into C#. Keeping the
//! bytes in the value makes it `Copy`, gives it a fixed layout, and lets an
//! archetype move it with a memcpy like a `u32` field. The price is the length
//! ceiling the type's name states.
//!
//! [`FixedString64::new`] truncates instead of failing, and it always cuts on
//! a character boundary. That is what lets [`FixedString64::as_str`] return a
//! `&str` without a `Result`, and it also means a string written by a build
//! with a larger capacity still loads. Serde sees the string, not the backing
//! array: `serde` implements its traits only up to 32-element arrays, and a
//! quoted `"footstep"` is what a person can read in a snapshot.
//!
//! ```
//! use pill_core::FixedString64;
//!
//! let name = FixedString64::new("footstep");
//! assert_eq!(name.as_str(), "footstep");
//! ```

// External crates
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// Capacity of a [`FixedString64`], in bytes.
///
/// The capacity counts bytes, not characters. One multi-byte character uses
/// several of them. 64 is long enough for an asset name or a short path, and
/// it keeps the value cheap to copy.
pub const FIXED_STRING_64_CAPACITY: usize = 64;

// =============================================================================
// FixedString64
// =============================================================================

/// A UTF-8 string of up to [`FIXED_STRING_64_CAPACITY`] bytes, stored inline.
///
/// It is `Copy` and `#[repr(C)]`, so it fits in a component, can cross the
/// module boundary, and can be mirrored into C# with no allocation. The
/// module documentation explains why a `String` cannot do that.
///
/// Comparison and hashing use only the bytes the string uses, so two values
/// holding the same text are equal however the unused bytes differ.
#[repr(C)]
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct FixedString64 {
    /// The string's bytes, stored inline in the value.
    ///
    /// Only the first `length` bytes are in use. Every constructor zeroes the
    /// rest and nothing reads it, so the unused bytes are not part of the
    /// value - which is why `PartialEq` is written by hand below.
    storage_bytes: [u8; FIXED_STRING_64_CAPACITY],
    /// How many leading bytes of `storage_bytes` hold the string.
    ///
    /// A `u8` holds any value up to the 64-byte capacity and keeps the struct
    /// small.
    length: u8,
}

impl FixedString64 {
    /// Returns the empty string.
    pub const fn empty() -> Self {
        Self {
            storage_bytes: [0; FIXED_STRING_64_CAPACITY],
            length: 0,
        }
    }

    /// Creates a string from `text`, cutting it to
    /// [`FIXED_STRING_64_CAPACITY`] bytes if it is longer.
    ///
    /// The cut always falls on a character boundary, so the stored value is
    /// valid UTF-8. Use [`Self::try_new`] instead when the caller needs to
    /// know that the text did not fit.
    pub fn new(text: &str) -> Self {
        let stored_byte_count =
            Self::boundary_at_or_below(text, text.len().min(FIXED_STRING_64_CAPACITY));
        let mut storage_bytes = [0; FIXED_STRING_64_CAPACITY];
        storage_bytes[..stored_byte_count].copy_from_slice(&text.as_bytes()[..stored_byte_count]);
        Self {
            storage_bytes,
            length: stored_byte_count as u8,
        }
    }

    /// Creates a string from `text`, or returns `None` when it does not fit.
    ///
    /// This is the checking version of [`Self::new`]. Use it where a name
    /// must be rejected rather than shortened, for example in a content
    /// pipeline that validates its input.
    pub fn try_new(text: &str) -> Option<Self> {
        (text.len() <= FIXED_STRING_64_CAPACITY).then(|| Self::new(text))
    }

    /// Returns the stored text.
    pub fn as_str(&self) -> &str {
        // Every constructor cuts on a character boundary, so the bytes in use
        // are always valid UTF-8. Should a future constructor break that
        // rule, this returns "" rather than panicking.
        std::str::from_utf8(&self.storage_bytes[..self.length as usize]).unwrap_or("")
    }

    /// Returns the stored text as bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.storage_bytes[..self.length as usize]
    }

    /// Returns how many bytes the string uses.
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Returns `true` when the string holds no text.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Returns the largest character boundary of `text` at or below `limit`.
    ///
    /// Both constructors end up here, so truncation has one rule for where
    /// it may cut.
    fn boundary_at_or_below(text: &str, limit: usize) -> usize {
        let mut limit = limit;
        while limit > 0 && !text.is_char_boundary(limit) {
            limit -= 1;
        }
        limit
    }
}

impl Default for FixedString64 {
    fn default() -> Self {
        Self::empty()
    }
}

// --- Conversions ---

/// Converts the value to an owned `String`, the form serde writes.
impl From<FixedString64> for String {
    fn from(value: FixedString64) -> Self {
        value.as_str().to_owned()
    }
}

/// Converts an owned `String`, cutting it to the capacity the way
/// [`FixedString64::new`] does, so a value written by a build with a larger
/// capacity still loads.
impl From<String> for FixedString64 {
    fn from(value: String) -> Self {
        Self::new(&value)
    }
}

impl From<&str> for FixedString64 {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl std::str::FromStr for FixedString64 {
    /// Cutting the text short is not an error, so parsing cannot fail.
    type Err = std::convert::Infallible;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(text))
    }
}

impl AsRef<str> for FixedString64 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for FixedString64 {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

// --- Comparison ---

/// Compares only the bytes the string uses.
///
/// The unused bytes are not part of the value. A string built by
/// [`FixedString64::new`] and the same text loaded from a snapshot must
/// compare equal even when those bytes differ.
impl PartialEq for FixedString64 {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for FixedString64 {}

impl PartialEq<str> for FixedString64 {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for FixedString64 {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Hashes the bytes the string uses, so it agrees with the hand-written
/// `PartialEq`.
impl std::hash::Hash for FixedString64 {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl Ord for FixedString64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for FixedString64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// --- Formatting ---

impl std::fmt::Display for FixedString64 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Prints the quoted text, not the backing array. A dump of 64 mostly unused
/// bytes would only add noise to a log line.
impl std::fmt::Debug for FixedString64 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}", self.as_str())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A string within capacity is stored and read back unchanged.
    #[test]
    fn stores_and_returns_a_short_string() {
        let value = FixedString64::new("footstep");
        assert_eq!(value.as_str(), "footstep");
        assert_eq!(value.len(), 8);
        assert!(!value.is_empty());
    }

    /// The default is the empty string, and it reads as empty text rather
    /// than as the NUL bytes the buffer holds.
    #[test]
    fn empty_is_the_default() {
        let value = FixedString64::default();
        assert_eq!(value.as_str(), "");
        assert!(value.is_empty());
        assert_eq!(value.len(), 0);
        assert_eq!(FixedString64::empty(), value);
    }

    /// A string of exactly the capacity length fits whole, because the limit
    /// is inclusive.
    #[test]
    fn a_string_at_capacity_is_kept_whole() {
        let text = "a".repeat(FIXED_STRING_64_CAPACITY);
        let value = FixedString64::new(&text);
        assert_eq!(value.len(), FIXED_STRING_64_CAPACITY);
        assert_eq!(value.as_str(), text);
    }

    /// One byte past capacity is truncated rather than rejected.
    #[test]
    fn an_over_long_string_is_truncated() {
        let text = "a".repeat(FIXED_STRING_64_CAPACITY + 1);
        let value = FixedString64::new(&text);
        assert_eq!(value.len(), FIXED_STRING_64_CAPACITY);
        assert!(text.starts_with(value.as_str()));
    }

    /// This is what makes `as_str` infallible. A cut that would split a
    /// character moves back to the previous boundary, so the stored bytes
    /// stay valid UTF-8.
    #[test]
    fn truncation_cuts_on_a_character_boundary() {
        // Each character is three bytes, and 64 is not a multiple of three,
        // so a cut at 64 would split the 22nd character.
        let text = "\u{4f60}".repeat(FIXED_STRING_64_CAPACITY);
        let value = FixedString64::new(&text);

        assert!(value.len() <= FIXED_STRING_64_CAPACITY);
        // 21 whole characters take 63 bytes. The 22nd would end at byte 66.
        assert_eq!(value.len(), 63);
        assert_eq!(value.as_str().chars().count(), 21);
        assert!(text.starts_with(value.as_str()));
    }

    /// `try_new` is the checking version. It returns `None` instead of
    /// shortening the text.
    #[test]
    fn try_new_refuses_what_does_not_fit() {
        assert!(FixedString64::try_new("footstep").is_some());
        assert!(FixedString64::try_new(&"a".repeat(FIXED_STRING_64_CAPACITY)).is_some());
        assert!(FixedString64::try_new(&"a".repeat(FIXED_STRING_64_CAPACITY + 1)).is_none());
    }

    /// Equality looks only at the bytes in use, so two values holding the same
    /// text are equal even when their unused bytes differ. This test dirties
    /// one unused byte by hand, the way a reused buffer would.
    #[test]
    fn equality_ignores_padding() {
        let original_value = FixedString64::new("footstep_sound_effect");
        let mut copy_with_dirty_padding = FixedString64::new("footstep_sound_effect");
        // Write into the unused bytes, the way a reused buffer would.
        copy_with_dirty_padding.storage_bytes[30] = b'x';

        assert_eq!(original_value, copy_with_dirty_padding);

        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let hash_of = |value: &FixedString64| {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(hash_of(&original_value), hash_of(&copy_with_dirty_padding));
    }

    /// Comparing against a plain string is the common case and needs no
    /// conversion at the call site.
    #[test]
    fn compares_against_str() {
        let value = FixedString64::new("footstep");
        assert_eq!(value, "footstep");
        assert_ne!(value, "splash");
        assert_eq!(value.as_str(), "footstep");
    }

    /// Ordering compares the text, not the raw byte array, so a sorted list
    /// comes out in the order a person expects.
    #[test]
    fn orders_lexicographically() {
        let mut values = [
            FixedString64::new("cherry"),
            FixedString64::new("apple"),
            FixedString64::new("banana"),
        ];
        values.sort();
        assert_eq!(values[0], "apple");
        assert_eq!(values[1], "banana");
        assert_eq!(values[2], "cherry");
    }

    /// Written as a plain string, so a snapshot stays readable and serde's
    /// 32-element array limit never applies.
    #[test]
    fn serializes_as_a_string() {
        let value = FixedString64::new("footstep");
        let json = serde_json::to_string(&value).expect("serialize");
        assert_eq!(json, "\"footstep\"");

        let restored: FixedString64 = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, value);
    }

    /// A string longer than the capacity, such as one written by a build with
    /// more room, still loads: it is cut to fit instead of failing the whole
    /// snapshot.
    #[test]
    fn deserializing_an_over_long_string_truncates() {
        let json = format!("\"{}\"", "a".repeat(FIXED_STRING_64_CAPACITY + 10));
        let restored: FixedString64 = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.len(), FIXED_STRING_64_CAPACITY);
    }

    /// This is the point of the type. It is plain data, with no pointer and a
    /// fixed layout.
    #[test]
    fn is_plain_data() {
        // One byte for each capacity slot plus one for the length, with no
        // padding because the alignment is 1.
        assert_eq!(
            std::mem::size_of::<FixedString64>(),
            FIXED_STRING_64_CAPACITY + 1
        );
        assert_eq!(std::mem::align_of::<FixedString64>(), 1);

        // `Copy`, so moving the value between archetypes is a memcpy.
        let value = FixedString64::new("footstep");
        let copied_value = value;
        assert_eq!(value, copied_value);
    }

    /// `Deref` to `str` makes the whole string API available without calling
    /// `as_str` at every use.
    #[test]
    fn derefs_to_str() {
        let value = FixedString64::new("footstep");
        assert!(value.starts_with("foot"));
        assert_eq!(value.to_uppercase(), "FOOTSTEP");
    }

    /// `Display` writes the text, and `Debug` writes it quoted instead of
    /// dumping the backing array.
    #[test]
    fn formats_as_its_text() {
        let value = FixedString64::new("footstep");
        assert_eq!(format!("{value}"), "footstep");
        assert_eq!(format!("{value:?}"), "\"footstep\"");
    }
}
