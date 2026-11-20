//! Thread-safe raw-pointer wrappers used by the query targets, the row
//! filters and the parallel system dispatcher.
//!
//! # Responsibilities
//!
//! - Provides [`SendPtr<T>`] and [`SendPtrMut<T>`] - raw-pointer wrappers
//!   that implement [`Send`] and [`Sync`] for cross-thread sharing.
//! - Lets a worker thread reach component storage through an address that was
//!   resolved once, with no second `HashMap` lookup.
//!
//! # Design
//!
//! A query resolves component storage once per archetype, then reads or
//! writes rows on worker threads through these wrappers. A wrapper holds an
//! address and nothing else: it owns no data, so moving it around is safe by
//! itself. What needs care is the dereference, and the caller is the one that
//! has to keep it sound.
//!
//! The query holds the exclusive `World` borrow, so the storage stays alive
//! and never reallocates while the pointers are in use. No two threads touch
//! the same row, because `ParQueryIter` gives each thread its own row range
//! and the scheduler only batches systems whose access
//! `SystemAccess::conflicts_with` reports as disjoint.

// =============================================================================
// SendPtr
// =============================================================================

/// Thread-safe wrapper around a `*const T` that lets worker threads read
/// storage they do not own.
///
/// The wrapper stores an address and nothing else, so copying or moving it is
/// harmless. Only dereferencing it needs `unsafe`, and the query code that
/// dereferences it guarantees the address is valid and nobody writes the same
/// row at the same time.
#[derive(Clone, Copy)]
pub struct SendPtr<T>(*const T);

impl<T> SendPtr<T> {
    /// Wraps a raw read-only pointer in a [`SendPtr`].
    pub fn new(pointer: *const T) -> Self {
        Self(pointer)
    }

    /// Returns the wrapped raw read-only pointer.
    pub fn as_ptr(&self) -> *const T {
        self.0
    }
}

// SAFETY: `SendPtr<T>` owns only a raw `*const T`. Moving it to another
// thread copies an address, so the move itself cannot cause a data race or
// an aliasing violation.
//
// The impl is bounded on `T: Send` instead of being left open to every `T`,
// even though the wrapper never dereferences the pointer itself. The address
// exists to be dereferenced on a worker thread, so an unbounded impl would
// let `SendPtr<Rc<_>>` cross threads and leave nothing but an `unsafe` block
// between that and a data race. The bound states the requirement where the
// type system can check it.
unsafe impl<T: Send> Send for SendPtr<T> {}
// SAFETY: Sharing `&SendPtr<T>` only allows copying the stored address out;
// the wrapper never dereferences the pointer, so nothing is aliased and no
// data race can happen through the wrapper. The bound is `T: Sync` because
// several threads read through the same address at once, which is exactly
// what `Sync` promises is safe.
unsafe impl<T: Sync> Sync for SendPtr<T> {}

// =============================================================================
// SendPtrMut
// =============================================================================

/// Thread-safe wrapper around a `*mut T` that lets worker threads write
/// storage they do not own.
///
/// As with [`SendPtr`], the wrapper stores an address and nothing else. The
/// `*mut` versus `*const` difference only matters where the address is
/// dereferenced, which needs `unsafe` and relies on the scheduler never
/// batching two systems that touch the same component.
#[derive(Clone, Copy)]
pub struct SendPtrMut<T>(*mut T);

impl<T> SendPtrMut<T> {
    /// Wraps a raw mutable pointer in a [`SendPtrMut`].
    pub fn new(pointer: *mut T) -> Self {
        Self(pointer)
    }

    /// Returns the wrapped raw mutable pointer.
    pub fn as_ptr(&self) -> *mut T {
        self.0
    }
}

// SAFETY: `SendPtrMut<T>` owns only a raw `*mut T`, and moving it between
// threads transfers an address; the wrapper itself never dereferences, so the
// move introduces no data race. The `T: Send` bound has the same reason as on
// `SendPtr`: the address exists to be dereferenced on another thread.
unsafe impl<T: Send> Send for SendPtrMut<T> {}
// SAFETY: Sharing `&SendPtrMut<T>` only allows copying the stored `*mut T`
// address. Dereferencing happens later inside `unsafe` blocks, which rely on
// the disjoint-access guarantee that `SystemAccess::conflicts_with`
// establishes, so no data race is reachable through the wrapper itself.
//
// The bound is `T: Send`, not `T: Sync`: the pointer grants exclusive access
// to a row, which hands that row to another thread instead of sharing it.
unsafe impl<T: Send> Sync for SendPtrMut<T> {}
