//! Engine-owned native buffer allocation.
//!
//! # Responsibilities
//!
//! - Allocate, retain and release the element blocks behind
//!   [`DynamicBuffer`](crate::DynamicBuffer) handles.
//! - Keep the number of live blocks observable, so a test can fail on a leak
//!   and a debug build fails on a double free.
//!
//! # Design
//!
//! One block is one allocation: a reference-count header first, then the
//! element area. The pointer handed out points at the elements, not at the
//! header. A handle can therefore store that pointer, and mirror it to C# as
//! plain `(ptr, len, cap)` words, while the reference count sits where no
//! handle can reach it.
//!
//! The service lives in `pill_core`, the dylib every artifact links. It is not
//! in `pill_engine`, which is an rlib compiled into each DLL. So a block's
//! lifetime never depends on which DLL allocated it, and a release run by a
//! reloaded generation reaches the same instance the allocation did.
//!
//! # Address-stability contract
//!
//! - A block's element pointer never moves while the block is alive. Growth
//!   allocates a new block instead of relocating the existing one.
//! - `retain` and `release` only change a count, so a block's address survives
//!   the handle being cloned. An archetype move copies the handle; it cannot
//!   move or copy the block.
//! - The block is freed when the last reference is released. A borrow seen
//!   through one handle stays valid for as long as any handle to the block
//!   exists.

// Standard library
use std::alloc::Layout;
use std::mem::size_of;
use std::sync::atomic::{AtomicUsize, Ordering};

// =============================================================================
// Accounting
// =============================================================================

/// Element bytes held by the live blocks, counted across the whole process.
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Number of live blocks, counted across the whole process.
static LIVE_BLOCKS: AtomicUsize = AtomicUsize::new(0);

/// Returns the total number of element bytes held by live blocks.
///
/// The header bytes of each block are not counted, so the number matches what
/// the handles' `(len, cap)` words add up to. Tests use it to check that a
/// world released everything it allocated.
pub fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Returns the number of live blocks in the process.
pub fn live_blocks() -> usize {
    LIVE_BLOCKS.load(Ordering::Relaxed)
}

// =============================================================================
// Blocks
// =============================================================================

/// Returns the distance in bytes between a block's elements and its
/// reference-count header.
///
/// The result is at least one `usize`, so the header itself is aligned. It is
/// also a multiple of `element_align` for the power-of-two alignments
/// [`allocate`] accepts, which keeps the element area aligned.
#[inline]
pub const fn header_size(element_align: usize) -> usize {
    if element_align > size_of::<usize>() {
        element_align
    } else {
        size_of::<usize>()
    }
}

/// Allocates one block of `byte_capacity` element bytes with the given element
/// alignment and returns a pointer to its uninitialised element area.
///
/// # Panics
///
/// Panics when `byte_capacity` is zero (an empty buffer has no block), when
/// `element_align` is not a power of two, or when the requested size cannot be
/// laid out. Aborts when the allocator cannot provide the memory.
///
/// # Safety
///
/// The returned pointer owns one reference. It must be handed to [`release`]
/// exactly once - through whichever handle owns it - after every [`retain`]
/// taken from it has been balanced.
pub unsafe fn allocate(byte_capacity: usize, element_align: usize) -> *mut u8 {
    assert!(byte_capacity > 0, "an empty block has no allocation");
    assert!(
        element_align.is_power_of_two(),
        "element alignment must be a power of two"
    );
    let block_alignment = element_align.max(std::mem::align_of::<usize>());
    let layout =
        Layout::from_size_align(header_size(element_align) + byte_capacity, block_alignment)
            .expect("native buffer layout is valid");
    // SAFETY: `layout` has a non-zero size (the header alone is at least one
    // word), which is the only precondition `alloc` has.
    let allocation_base = unsafe { std::alloc::alloc(layout) };
    if allocation_base.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    // SAFETY: `allocation_base` points at `header_size + byte_capacity`
    // writable bytes, aligned to at least `align_of::<usize>()` because the
    // header sits at offset zero. An `AtomicUsize` fits there.
    unsafe {
        allocation_base
            .cast::<AtomicUsize>()
            .write(AtomicUsize::new(1));
    }
    LIVE_BYTES.fetch_add(byte_capacity, Ordering::Relaxed);
    LIVE_BLOCKS.fetch_add(1, Ordering::Relaxed);

    // SAFETY: the element area starts at `header_size` inside the same
    // allocation, and `header_size` is a multiple of `element_align`, so the
    // returned pointer is aligned for the elements the caller is about to
    // write.
    unsafe { allocation_base.add(header_size(element_align)) }
}

/// Takes one more reference to a block's elements.
///
/// # Safety
///
/// `element_pointer` must point at the element area of a live block allocated
/// by [`allocate`] with the same `element_align`. The caller owns the new
/// reference and must balance it with one [`release`].
pub unsafe fn retain(element_pointer: *mut u8, element_align: usize) {
    // SAFETY: the caller guarantees `element_pointer` belongs to a live block,
    // so the header just before it is a live, initialised `AtomicUsize`.
    let reference_count_header = unsafe {
        element_pointer
            .sub(header_size(element_align))
            .cast::<AtomicUsize>()
    };
    // SAFETY: the same caller guarantee makes the header live here.
    unsafe { (*reference_count_header).fetch_add(1, Ordering::Relaxed) };
}

/// Drops one reference to a block's elements, and frees the block when the
/// last reference goes away.
///
/// # Panics
///
/// Panics in debug builds when the block is released more times than it was
/// retained.
///
/// # Safety
///
/// `element_pointer` must point at the element area of a live block allocated
/// by [`allocate`] with the same `byte_capacity` and `element_align`, and this
/// call must balance exactly one earlier [`allocate`] or [`retain`]. The
/// pointer must not be used after the call unless another reference is still
/// held.
pub unsafe fn release(element_pointer: *mut u8, byte_capacity: usize, element_align: usize) {
    // SAFETY: the caller guarantees `element_pointer` belongs to a live block,
    // so the header just before it is a live, initialised `AtomicUsize`.
    let reference_count_header = unsafe {
        element_pointer
            .sub(header_size(element_align))
            .cast::<AtomicUsize>()
    };
    // SAFETY: the same caller guarantee makes the header live here. `AcqRel`
    // publishes this thread's writes on the release side, and the release that
    // takes the count to zero sees the writes of every earlier release.
    let previous_reference_count =
        unsafe { (*reference_count_header).fetch_sub(1, Ordering::AcqRel) };
    debug_assert!(
        previous_reference_count > 0,
        "a native buffer was released more times than it was retained"
    );
    if previous_reference_count != 1 {
        return;
    }

    let block_alignment = element_align.max(std::mem::align_of::<usize>());
    let layout =
        Layout::from_size_align(header_size(element_align) + byte_capacity, block_alignment)
            .expect("native buffer layout is valid");
    // SAFETY: the caller guarantees `element_pointer` belongs to a block
    // allocated with exactly this layout, and the count just reached zero, so
    // this is the last reference and no new one can be taken.
    unsafe { std::alloc::dealloc(element_pointer.sub(header_size(element_align)), layout) };
    LIVE_BYTES.fetch_sub(byte_capacity, Ordering::Relaxed);
    LIVE_BLOCKS.fetch_sub(1, Ordering::Relaxed);
}

/// Returns how many references the block behind `element_pointer` holds.
///
/// # Safety
///
/// `element_pointer` must point at the element area of a live block allocated
/// by [`allocate`] with the same `element_align`.
pub unsafe fn reference_count(element_pointer: *mut u8, element_align: usize) -> usize {
    // SAFETY: the caller guarantees `element_pointer` belongs to a live block,
    // so the header just before it is a live, initialised `AtomicUsize`.
    let reference_count_header = unsafe {
        element_pointer
            .sub(header_size(element_align))
            .cast::<AtomicUsize>()
    };
    // SAFETY: the same caller guarantee makes the header live here.
    unsafe { (*reference_count_header).load(Ordering::Acquire) }
}

// =============================================================================
// Tests
// =============================================================================

/// One lock shared by every test that asserts on the allocation accounting.
///
/// The counters cover the whole process, so a lock that each test module keeps
/// to itself is not enough: two modules could still run at the same time. A
/// test that measured an exact count would then pick up the other module's
/// live blocks as its baseline and later see them disappear. With one shared
/// lock, the tests that measure the counters never run at the same time.
#[cfg(test)]
pub(crate) mod accounting_guard {
    use std::sync::{Mutex, MutexGuard};

    /// Held for the body of any test that asserts on `live_bytes` or
    /// `live_blocks`.
    static ACCOUNTING_LOCK: Mutex<()> = Mutex::new(());

    /// Takes the lock.
    ///
    /// A poisoned lock means an earlier holder panicked. That panic is already
    /// reported, so the lock is recovered instead of failing every later test
    /// as well.
    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        ACCOUNTING_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Takes the crate-wide accounting lock that every test asserting on the
    /// counters must hold.
    fn guarded() -> std::sync::MutexGuard<'static, ()> {
        accounting_guard::lock()
    }

    /// The header is at least one word and always a multiple of the element
    /// alignment, which is what keeps the element area aligned.
    #[test]
    fn header_size_keeps_elements_aligned() {
        assert_eq!(header_size(1), size_of::<usize>());
        assert_eq!(header_size(4), size_of::<usize>());
        assert_eq!(header_size(8), size_of::<usize>());
        assert_eq!(header_size(16), 16);
        for align in [1usize, 2, 4, 8, 16] {
            assert_eq!(header_size(align) % align, 0);
        }
    }

    /// A block reports the alignment it was allocated with and counts as one
    /// live block until it is released.
    #[test]
    fn allocate_reports_aligned_elements_and_accounting() {
        let _guard = guarded();
        let bytes_before = live_bytes();
        let blocks_before = live_blocks();

        // SAFETY: the pointer owns the block's single reference and is
        // released exactly once below.
        let element_pointer = unsafe { allocate(64, 16) };
        assert_eq!(element_pointer as usize % 16, 0);
        // SAFETY: the block is live and we hold its reference.
        assert_eq!(unsafe { reference_count(element_pointer, 16) }, 1);
        assert_eq!(live_bytes(), bytes_before + 64);
        assert_eq!(live_blocks(), blocks_before + 1);

        // SAFETY: balances the `allocate` above.
        unsafe { release(element_pointer, 64, 16) };
        assert_eq!(live_bytes(), bytes_before);
        assert_eq!(live_blocks(), blocks_before);
    }

    /// A retained reference keeps the block alive until both references are
    /// released.
    #[test]
    fn retain_keeps_the_block_alive_until_the_last_release() {
        let _guard = guarded();
        let bytes_before = live_bytes();

        // SAFETY: the pointer owns the block's first reference.
        let element_pointer = unsafe { allocate(32, 4) };
        // SAFETY: the block is live; the retained reference is released below.
        unsafe { retain(element_pointer, 4) };

        // SAFETY: the block is live and now holds two references.
        assert_eq!(unsafe { reference_count(element_pointer, 4) }, 2);

        // SAFETY: balances the first reference; the block stays alive because
        // the retained one is still outstanding.
        unsafe { release(element_pointer, 32, 4) };
        assert_eq!(live_bytes(), bytes_before + 32);
        // SAFETY: the block is still live through the second reference.
        assert_eq!(unsafe { reference_count(element_pointer, 4) }, 1);

        // SAFETY: balances the `retain` above; this is the last reference.
        unsafe { release(element_pointer, 32, 4) };
        assert_eq!(live_bytes(), bytes_before);
    }
}
