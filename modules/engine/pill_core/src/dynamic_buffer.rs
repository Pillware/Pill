//! Engine-owned native dynamic buffer.
//!
//! # Responsibilities
//!
//! - Provide [`DynamicBuffer<T>`], a growable run of plain (`Copy`) elements
//!   whose memory comes from the engine's shared allocation service
//!   ([`crate::native_buffer`]) instead of from a `Vec` heap inside whichever
//!   DLL created it.
//! - Give Rust systems and generated C# mirrors the same live view of the
//!   data: a `#[repr(C)]` handle of three pointer-sized words that managed
//!   code reads straight out of the component row.
//!
//! # Design
//!
//! A `Vec` cannot survive a reload: its buffer belongs to the allocator of
//! whichever DLL created it, and that DLL is gone after the swap. Growing a
//! `Vec` can also move its buffer, which ends any view managed code holds of
//! the old one. A `DynamicBuffer` keeps its elements in a *block* instead: one
//! allocation whose reference count sits in a header the handle cannot see,
//! with the handle pointing at the elements.
//!
//! Cloning a handle retains the block instead of copying it, so a clone shares
//! the elements at the same address. Writing through one handle copies a
//! shared block first, so two handles never write through each other. Those
//! writes are safe, but a slice borrowed from the other handle before the copy
//! does not see them, exactly as with `Cow::to_mut` or `Arc::make_mut`.
//! Operations that only shrink the buffer (`clear`, `truncate`, a shrinking
//! [`resize`](DynamicBuffer::resize)) change the handle's own count and leave
//! the block alone.
//!
//! The contract callers may rely on:
//!
//! - a block's address stays the same for as long as any handle uses it;
//! - an archetype migration copies the handle and never the block, so a
//!   component's elements stay where they were across structural changes;
//! - a call that changes the buffer may give this handle a new block: growth
//!   past the capacity always allocates one, and a write to a shared block
//!   copies it first. A raw pointer from [`DynamicBuffer::as_ptr`] or
//!   [`DynamicBuffer::as_mut_ptr`] is good only until the buffer is written
//!   to or grows, so take it, use it, then let the buffer change;
//! - nothing caches an element pointer across a reload or a frame boundary.

// Standard library
use std::mem::size_of;
use std::ops::{Deref, DerefMut};

// External crates
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// Current crate
use crate::native_buffer;

// =============================================================================
// DynamicBuffer
// =============================================================================

/// A growable run of plain elements living in engine-owned native memory.
///
/// The handle is `#[repr(C)]` and holds three pointer-sized words in this
/// order: a pointer to the elements, the element count, then the capacity. The
/// C# mirror reads those words straight out of the component row, so managed
/// code iterates the live elements without a call across the boundary.
///
/// `T: Copy` is a hard requirement, not a convenience: elements are copied
/// byte for byte between blocks, and the last release frees the block without
/// running any destructor. A type that owned anything would leak, so it cannot
/// be used in a buffer at all.
#[repr(C)]
pub struct DynamicBuffer<T: Copy> {
    /// Pointer to the block's element area; null while the buffer owns no
    /// block. An empty buffer created with a capacity still points at its
    /// block.
    element_pointer: *mut T,
    /// Number of initialised elements at `element_pointer`.
    element_count: usize,
    /// Elements the block can hold before it must grow.
    element_capacity: usize,
}

impl<T: Copy> DynamicBuffer<T> {
    /// Alignment every block holding this element type is allocated with.
    const ELEMENT_ALIGN: usize = std::mem::align_of::<T>();

    /// Creates an empty buffer that owns no block.
    pub const fn new() -> Self {
        Self {
            element_pointer: std::ptr::null_mut(),
            element_count: 0,
            element_capacity: 0,
        }
    }

    /// Creates an empty buffer with room for `capacity` elements before the
    /// first growth.
    ///
    /// # Panics
    ///
    /// Panics when `capacity` is not zero and `T` is a zero-sized type, or
    /// when the allocation request cannot be laid out.
    pub fn with_capacity(capacity: usize) -> Self {
        if capacity == 0 {
            return Self::new();
        }
        assert!(
            size_of::<T>() > 0,
            "a DynamicBuffer element type must have a non-zero size"
        );
        // SAFETY: `capacity` is not zero and the element size is not zero, so
        // the block's byte capacity is not zero. The returned pointer holds
        // the block's single reference, which this handle stores.
        let block_element_pointer = unsafe {
            native_buffer::allocate(Self::element_byte_capacity(capacity), Self::ELEMENT_ALIGN)
        };
        Self {
            element_pointer: block_element_pointer.cast::<T>(),
            element_count: 0,
            element_capacity: capacity,
        }
    }

    /// Creates a buffer holding a copy of `values`.
    ///
    /// # Panics
    ///
    /// Panics when `values` is not empty and `T` is a zero-sized type, or when
    /// the allocation request cannot be laid out.
    pub fn from_slice(values: &[T]) -> Self {
        let mut buffer = Self::with_capacity(values.len());
        buffer.extend_from_slice(values);
        buffer
    }

    /// Returns the number of initialised elements.
    pub fn len(&self) -> usize {
        self.element_count
    }

    /// Returns whether the buffer holds no elements.
    pub fn is_empty(&self) -> bool {
        self.element_count == 0
    }

    /// Returns how many elements the block can hold before it must grow.
    pub fn capacity(&self) -> usize {
        self.element_capacity
    }

    /// Returns a pointer to the first element, for FFI trampolines and mirror
    /// views.
    ///
    /// The pointer is null only when the buffer owns no block (`new`, or
    /// `with_capacity(0)`). A caller that passes it to managed code must also
    /// pass [`Self::len`], and treat a zero length as "no view".
    pub fn as_ptr(&self) -> *const T {
        self.element_pointer
    }

    /// Returns a writable pointer to the first element, copying the block
    /// first when it is shared.
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ensure_unique();
        self.element_pointer
    }

    /// Returns the elements as a shared slice.
    pub fn as_slice(&self) -> &[T] {
        if self.element_count == 0 {
            return &[];
        }
        // SAFETY: `element_count` initialised elements follow
        // `element_pointer`, and no `&mut` to them exists while this shared
        // borrow is alive. The count never exceeds the capacity of the block
        // the handle points at.
        unsafe { std::slice::from_raw_parts(self.element_pointer, self.element_count) }
    }

    /// Returns the elements as a mutable slice.
    ///
    /// Copies the block first when another handle shares it, so a reader never
    /// sees the elements change.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.ensure_unique();
        if self.element_count == 0 {
            return &mut [];
        }
        // SAFETY: `element_count` initialised elements follow
        // `element_pointer`, `ensure_unique` left this handle the only one
        // using the block, and the exclusive borrow lasts as long as the
        // returned slice.
        unsafe { std::slice::from_raw_parts_mut(self.element_pointer, self.element_count) }
    }

    /// Appends one element, growing the buffer or copying a shared block as
    /// needed.
    ///
    /// # Panics
    ///
    /// Panics when the buffer has to grow and `T` is a zero-sized type, or
    /// when the allocation request cannot be laid out.
    pub fn push(&mut self, value: T) {
        if self.element_count == self.element_capacity {
            self.grow_to(self.element_count + 1);
        } else {
            self.ensure_unique();
        }
        // SAFETY: the capacity is now larger than `element_count`, so
        // `element_pointer + element_count` is inside the block and is not
        // null, and that slot holds no value yet, so writing one there is fine.
        unsafe { self.element_pointer.add(self.element_count).write(value) };
        self.element_count += 1;
    }

    /// Appends every element of `values`.
    ///
    /// # Panics
    ///
    /// Panics when the buffer has to grow and `T` is a zero-sized type, or
    /// when the allocation request cannot be laid out.
    pub fn extend_from_slice(&mut self, values: &[T]) {
        if values.is_empty() {
            return;
        }
        if self.element_count + values.len() > self.element_capacity {
            self.grow_to(self.element_count + values.len());
        } else {
            self.ensure_unique();
        }
        // SAFETY: the capacity covers `element_count + values.len()`, so the
        // destination range is inside the block. `values` points into a
        // different allocation, so the two ranges cannot overlap.
        unsafe {
            std::ptr::copy_nonoverlapping(
                values.as_ptr(),
                self.element_pointer.add(self.element_count),
                values.len(),
            );
        }
        self.element_count += values.len();
    }

    /// Changes the buffer to hold `new_len` elements.
    ///
    /// Growing fills the new elements with `value`. Shrinking keeps the first
    /// `new_len` elements. A growing resize past the capacity allocates a new
    /// block, so this handle ends up as its only user; within the capacity it
    /// copies a shared block first. Shrinking writes nothing and leaves a
    /// shared block where it is, so a slice taken through another handle stays
    /// valid across it.
    ///
    /// # Panics
    ///
    /// Panics when the buffer has to grow and `T` is a zero-sized type, or
    /// when the allocation request cannot be laid out.
    pub fn resize(&mut self, new_len: usize, value: T) {
        if new_len > self.element_capacity {
            self.grow_to(new_len);
        } else if new_len > self.element_count {
            self.ensure_unique();
        }
        for index in self.element_count..new_len {
            // SAFETY: the capacity covers `new_len`, so every index in
            // `element_count..new_len` is inside the block, and those slots
            // hold no value yet, so writing over them is fine. When the block
            // was shared, a write path above made this handle its only user.
            unsafe { self.element_pointer.add(index).write(value) };
        }
        self.element_count = new_len;
    }

    /// Empties the buffer, keeping the block for reuse.
    pub fn clear(&mut self) {
        self.element_count = 0;
    }

    /// Keeps only the first `new_len` elements.
    pub fn truncate(&mut self, new_len: usize) {
        if new_len < self.element_count {
            self.element_count = new_len;
        }
    }

    /// Makes room for `additional_element_count` more elements.
    ///
    /// # Panics
    ///
    /// Panics when the buffer has to grow and `T` is a zero-sized type, or
    /// when the allocation request cannot be laid out.
    pub fn reserve(&mut self, additional_element_count: usize) {
        let wanted_element_count = self.element_count.saturating_add(additional_element_count);
        if wanted_element_count > self.element_capacity {
            self.grow_to(wanted_element_count);
        }
    }

    /// Returns the byte capacity of a block that holds `element_count`
    /// elements.
    fn element_byte_capacity(element_count: usize) -> usize {
        element_count * size_of::<T>()
    }

    /// Grows the buffer to hold at least `wanted_element_count` elements,
    /// keeping the elements it already has.
    ///
    /// Always allocates a fresh block and copies into it. That is correct
    /// whether the current block is shared (the other handle keeps it) or not
    /// (the old reference is released and the block is freed), and it never
    /// moves a live block, which is what keeps the address-stability contract.
    ///
    /// # Panics
    ///
    /// Panics when `T` is a zero-sized type, or when the allocation request
    /// cannot be laid out.
    fn grow_to(&mut self, wanted_element_count: usize) {
        assert!(
            size_of::<T>() > 0,
            "a DynamicBuffer element type must have a non-zero size"
        );
        // Doubling the capacity keeps repeated pushes cheap on average. Small
        // buffers start at four elements, so the first push does not allocate
        // for a single element.
        let grown_capacity = wanted_element_count
            .max(self.element_capacity.saturating_mul(2))
            .max(4);
        let block_byte_capacity = Self::element_byte_capacity(grown_capacity);
        // SAFETY: the capacity is not zero and the element size is not zero, so
        // the allocation request is valid. The returned pointer holds the new
        // block's single reference.
        let fresh_block_pointer = unsafe {
            native_buffer::allocate(block_byte_capacity, Self::ELEMENT_ALIGN).cast::<T>()
        };

        // SAFETY: `fresh_block_pointer` holds a fresh block with room for
        // `grown_capacity` elements, which is at least the current element
        // count, with this element alignment, and no other handle uses it.
        self.swap_block(fresh_block_pointer);
        self.element_capacity = grown_capacity;
    }

    /// Replaces this handle's block with `fresh_block_pointer`, copying the
    /// live elements over and releasing the old block.
    ///
    /// This is the one place the copy, release and swap sequence lives;
    /// [`Self::grow_to`] and [`Self::ensure_unique`] differ only in how they
    /// obtain the new block. `fresh_block_pointer` must hold the new block's
    /// single reference and point at room for at least `self.element_count`
    /// elements. The old block must still be described by
    /// `self.element_capacity` elements of this type.
    fn swap_block(&mut self, fresh_block_pointer: *mut T) {
        if self.element_count > 0 {
            // SAFETY: both blocks hold `element_count` initialised elements of
            // the same type, the source block is alive (this handle holds a
            // reference to it), and the destination was just allocated, so the
            // two cannot overlap.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.element_pointer,
                    fresh_block_pointer,
                    self.element_count,
                )
            };
        }
        if !self.element_pointer.is_null() {
            // SAFETY: this handle holds one reference to the old block, and
            // its byte capacity and alignment match the stored values.
            // Releasing it leaves the block alive for any other handle that
            // still uses it.
            unsafe {
                native_buffer::release(
                    self.element_pointer.cast::<u8>(),
                    Self::element_byte_capacity(self.element_capacity),
                    Self::ELEMENT_ALIGN,
                );
            }
        }
        self.element_pointer = fresh_block_pointer;
    }

    /// Makes this handle the only user of its block, before writing through it.
    fn ensure_unique(&mut self) {
        if self.element_pointer.is_null() {
            return;
        }
        // SAFETY: the pointer addresses a live block this handle holds a
        // reference to, allocated with this element alignment.
        let block_reference_count = unsafe {
            native_buffer::reference_count(self.element_pointer.cast::<u8>(), Self::ELEMENT_ALIGN)
        };
        if block_reference_count <= 1 {
            return;
        }
        let block_byte_capacity = Self::element_byte_capacity(self.element_capacity);
        // SAFETY: the capacity is not zero (a block exists) and the element
        // size is not zero, so the allocation request is valid.
        let fresh_block_pointer = unsafe {
            native_buffer::allocate(block_byte_capacity, Self::ELEMENT_ALIGN).cast::<T>()
        };
        // SAFETY: `fresh_block_pointer` holds a fresh block with room for
        // `self.element_capacity` elements, no other handle uses it, and the
        // release inside leaves the shared block alive for the handles that
        // still use it.
        self.swap_block(fresh_block_pointer);
    }
}

// =============================================================================
// Trait Impls
// =============================================================================

impl<T: Copy> Default for DynamicBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Cloning a buffer retains its block instead of copying it.
///
/// A duplicated component therefore duplicates the handle and never the
/// elements. An archetype migration does not even call `Clone`, because a row
/// travels bitwise. Either way, the two handles share one block until one of
/// them writes, and the writer copies first.
impl<T: Copy> Clone for DynamicBuffer<T> {
    fn clone(&self) -> Self {
        if !self.element_pointer.is_null() {
            // SAFETY: the pointer addresses a live block this handle holds a
            // reference to; the clone takes ownership of the new reference.
            unsafe {
                native_buffer::retain(self.element_pointer.cast::<u8>(), Self::ELEMENT_ALIGN)
            };
        }
        Self {
            element_pointer: self.element_pointer,
            element_count: self.element_count,
            element_capacity: self.element_capacity,
        }
    }
}

impl<T: Copy> Drop for DynamicBuffer<T> {
    fn drop(&mut self) {
        if self.element_pointer.is_null() {
            return;
        }
        // SAFETY: this handle holds exactly one reference to the block, and its
        // byte capacity and alignment match the stored values. Elements are
        // `Copy`, so no destructor runs before the block is freed.
        unsafe {
            native_buffer::release(
                self.element_pointer.cast::<u8>(),
                Self::element_byte_capacity(self.element_capacity),
                Self::ELEMENT_ALIGN,
            );
        }
    }
}

// SAFETY: the handle holds one reference to a block of plain `Copy` elements
// and keeps no state that belongs to a thread. Reference counts are atomic,
// and every write path copies a shared block first, so moving the handle to
// another thread cannot race another handle's elements.
unsafe impl<T: Copy + Send> Send for DynamicBuffer<T> {}

// SAFETY: same reasoning as the `Send` impl above. Sharing `&DynamicBuffer`
// hands out element reads only, which `T: Sync` makes safe across threads.
unsafe impl<T: Copy + Sync> Sync for DynamicBuffer<T> {}

impl<T: Copy> Deref for DynamicBuffer<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: Copy> DerefMut for DynamicBuffer<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: Copy + std::fmt::Debug> std::fmt::Debug for DynamicBuffer<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_list()
            .entries(self.as_slice().iter())
            .finish()
    }
}

impl<T: Copy + PartialEq> PartialEq for DynamicBuffer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Copy + Eq> Eq for DynamicBuffer<T> {}

/// Serializes the buffer as the sequence of its live elements.
///
/// This is the same shape a `Vec<T>` writes, so a schema change migrates the
/// elements through the ordinary persistence path.
impl<T: Copy + Serialize> Serialize for DynamicBuffer<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_slice().serialize(serializer)
    }
}

/// Deserializes a sequence into a freshly allocated block. No address is ever
/// carried across a reload.
impl<'de, T: Copy + Deserialize<'de>> Deserialize<'de> for DynamicBuffer<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values: Vec<T> = Vec::deserialize(deserializer)?;
        Ok(Self::from_slice(&values))
    }
}

// =============================================================================
// Tests
// =============================================================================

/// Unit tests for the handle's contract: the pinned layout, a clone that
/// shares its block, copy-on-write mutation, balanced block accounting and
/// serde.
///
/// The accounting counters are process-wide, so the tests that assert on them
/// take turns through one shared lock.
#[cfg(test)]
mod tests {
    use super::*;

    /// Takes the crate-wide accounting lock.
    ///
    /// `native_buffer`'s tests assert on the same process-wide counters, so
    /// both test modules must take one lock. A lock of this module's own would
    /// let the two interleave.
    fn guarded() -> std::sync::MutexGuard<'static, ()> {
        native_buffer::accounting_guard::lock()
    }

    /// The mirror and the C# side both depend on the handle being three
    /// pointer-sized, pointer-aligned words in this order: the element
    /// pointer, the element count, then the capacity.
    #[test]
    fn handle_layout_is_pinned() {
        use std::mem::{align_of, offset_of, size_of};
        assert_eq!(size_of::<DynamicBuffer<f32>>(), 3 * size_of::<usize>());
        assert_eq!(align_of::<DynamicBuffer<f32>>(), align_of::<usize>());
        assert_eq!(offset_of!(DynamicBuffer<f32>, element_pointer), 0);
        assert_eq!(
            offset_of!(DynamicBuffer<f32>, element_count),
            size_of::<usize>()
        );
        assert_eq!(
            offset_of!(DynamicBuffer<f32>, element_capacity),
            2 * size_of::<usize>()
        );
    }

    /// Pushes keep every element, and resizing fills new elements with the
    /// given value and shrinks to the requested length.
    #[test]
    fn push_and_resize_keep_contents() {
        let _guard = guarded();
        let bytes_before = native_buffer::live_bytes();

        let mut buffer = DynamicBuffer::<f32>::new();
        assert!(buffer.is_empty());
        for value in 0..10 {
            buffer.push(value as f32);
        }
        assert_eq!(buffer.len(), 10);
        assert!(buffer.capacity() >= 10);
        assert_eq!(
            buffer.as_slice(),
            (0..10).map(|v| v as f32).collect::<Vec<_>>().as_slice()
        );

        buffer.resize(12, -1.0);
        assert_eq!(buffer.len(), 12);
        assert_eq!(buffer.as_slice()[10], -1.0);
        buffer.resize(2, 0.0);
        assert_eq!(buffer.as_slice(), &[0.0, 1.0]);

        drop(buffer);
        assert_eq!(native_buffer::live_bytes(), bytes_before);
    }

    /// A clone shares the block, and writing through either handle copies
    /// first, so neither sees the other change. An archetype move relies on
    /// exactly that.
    #[test]
    fn clone_shares_the_block_until_a_write() {
        let _guard = guarded();
        let bytes_before = native_buffer::live_bytes();

        let original = DynamicBuffer::from_slice(&[1u32, 2, 3]);
        let mut clone = original.clone();
        // Address stability: the clone holds another reference to the same
        // block, so both handles read the same address.
        assert_eq!(original.as_ptr(), clone.as_ptr());
        assert_eq!(
            // SAFETY: the block is live and both handles hold references to it.
            unsafe { native_buffer::reference_count(original.as_ptr().cast_mut().cast::<u8>(), 4) },
            2
        );

        clone.push(4);
        assert_ne!(original.as_ptr(), clone.as_ptr());
        assert_eq!(original.as_slice(), &[1, 2, 3]);
        assert_eq!(clone.as_slice(), &[1, 2, 3, 4]);

        drop(clone);
        drop(original);
        assert_eq!(native_buffer::live_bytes(), bytes_before);
    }

    /// A truncate writes nothing, so a shared block stays shared.
    #[test]
    fn truncate_keeps_a_shared_block() {
        let _guard = guarded();

        let buffer = DynamicBuffer::from_slice(&[1i64, 2, 3, 4]);
        let mut shared = buffer.clone();
        let address = buffer.as_ptr();

        shared.truncate(2);
        assert_eq!(buffer.as_slice(), &[1, 2, 3, 4]);
        assert_eq!(shared.as_slice(), &[1, 2]);
        assert_eq!(buffer.as_ptr(), address);
        assert_eq!(shared.as_ptr(), address);

        drop(shared);
        drop(buffer);
    }

    /// A shrink-only resize writes nothing, so a shared block stays shared
    /// and a slice another handle holds survives it.
    #[test]
    fn shrinking_resize_keeps_a_shared_block() {
        let _guard = guarded();

        let buffer = DynamicBuffer::from_slice(&[1i64, 2, 3, 4]);
        let mut shared = buffer.clone();
        let address = buffer.as_ptr();

        shared.resize(2, 0);
        assert_eq!(buffer.as_slice(), &[1, 2, 3, 4]);
        assert_eq!(shared.as_slice(), &[1, 2]);
        assert_eq!(buffer.as_ptr(), address);
        assert_eq!(shared.as_ptr(), address);

        drop(shared);
        drop(buffer);
    }

    /// Elements round-trip through serde, which is what carries buffer
    /// contents across a reload whose schema changed.
    #[test]
    fn serde_round_trip_preserves_elements() {
        let _guard = guarded();
        let bytes_before = native_buffer::live_bytes();

        let buffer = DynamicBuffer::from_slice(&[2.5f64, -1.0, 42.0]);
        let serialized_json = serde_json::to_string(&buffer).expect("serialize");
        assert_eq!(serialized_json, "[2.5,-1.0,42.0]");
        let restored: DynamicBuffer<f64> =
            serde_json::from_str(&serialized_json).expect("deserialize");
        assert_eq!(restored.as_slice(), buffer.as_slice());

        let empty = DynamicBuffer::<u8>::new();
        let serialized_json = serde_json::to_string(&empty).expect("serialize empty");
        assert_eq!(serialized_json, "[]");
        let restored_empty: DynamicBuffer<u8> =
            serde_json::from_str(&serialized_json).expect("deserialize empty");
        assert!(restored_empty.is_empty());

        drop(restored);
        drop(restored_empty);
        drop(empty);
        drop(buffer);
        assert_eq!(native_buffer::live_bytes(), bytes_before);
    }

    /// The free path frees exactly once. Dropping a buffer returns its bytes,
    /// and a retained reference keeps the block alive until the last drop.
    #[test]
    fn drops_release_blocks_exactly_once() {
        let _guard = guarded();
        let bytes_before = native_buffer::live_bytes();

        let buffer = DynamicBuffer::from_slice(&[0u8; 32]);
        let shared = buffer.clone();
        let held = native_buffer::live_bytes();
        drop(shared);
        assert_eq!(
            native_buffer::live_bytes(),
            held,
            "the block must outlive the first handle"
        );
        drop(buffer);
        assert_eq!(native_buffer::live_bytes(), bytes_before);
    }
}
