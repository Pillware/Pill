//! The two-call handshake that copies a managed payload across the boundary.
//!
//! # Responsibilities
//!
//! - Runs the same handshake for every kind of payload: ask the length,
//!   allocate, ask for a copy. Manifests, system names, failure messages and
//!   compiler diagnostics all use it.
//! - Checks the length the managed side reports against a limit, and reserves
//!   memory in a way that can fail. Neither a buggy nor a hostile assembly can
//!   make the host abort.
//!
//! # Design
//!
//! Managed code cannot hand the host an owned buffer, so every payload crosses
//! in two calls: one reports how many bytes there are, the host allocates, and
//! the second call fills the buffer the host allocated. The protocol belongs to
//! the boundary, not to any one payload, so each caller supplies only the two
//! exported functions and the upper bound for its kind.
//!
//! The length is checked against the limit **before** anything is allocated, so
//! a corrupt length becomes a reported error instead of a multi-gigabyte
//! request. The reservation is fallible too: `Vec::try_reserve_exact` returns
//! an error where `vec![0; n]` would abort the process. A failure is returned
//! instead of logged because callers treat it differently - one returns a typed
//! load error, one logs the failure, one substitutes plain text - and that
//! difference is legitimate.

// External crates
use pill_core::error::CSharpError;

// =============================================================================
// Types
// =============================================================================

/// Why one managed payload could not be fetched.
///
/// The variants that report a length carry it, so a caller that logs the
/// failure and one that returns a typed error both have what they need without
/// asking the managed side again.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedBufferError {
    /// The managed side reported a length outside `1..=limit`.
    LengthOutOfRange {
        /// The length it reported.
        length: u32,
        /// The largest length this payload kind accepts.
        limit: u32,
    },
    /// The host could not reserve a buffer that long.
    AllocationFailed {
        /// The length the reservation was for.
        length: u32,
    },
    /// The managed side declined to fill the buffer the host offered.
    CopyFailed,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Fetches one managed payload through the two-call protocol.
///
/// `length` and `copy` are the two exported functions for this payload kind,
/// already bound to the index or receiver they need. `limit` is the largest
/// length this kind accepts. The returned buffer is exactly `length` bytes, and
/// the managed contract says a successful copy fills all of them.
///
/// The arguments are closures rather than function pointers for two reasons.
/// The exports are `extern "system"`, and that ABI does not implement the `Fn`
/// traits. Several payload kinds are also addressed by an index, which the
/// caller has to close over anyway.
///
/// # Errors
///
/// Returns [`ManagedBufferError::LengthOutOfRange`] when the reported length is
/// zero, or above `limit`. Returns [`ManagedBufferError::AllocationFailed`]
/// when a buffer that long cannot be reserved. Returns
/// [`ManagedBufferError::CopyFailed`] when managed code declines to fill it.
/// Nothing is allocated in the first case, and the partially filled buffer is
/// discarded in the last.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub fn fetch_managed_buffer(
    length: impl FnOnce() -> u32,
    copy: impl FnOnce(*mut u8, u32) -> u8,
    limit: u32,
) -> Result<Vec<u8>, ManagedBufferError> {
    // Step 1: Check the length before anything is allocated from it. Zero is
    // refused along with lengths above the limit: a zero length means the
    // managed side has no payload for this kind, and the host does not treat
    // that as an empty payload.
    let length = length();
    if length == 0 || length > limit {
        return Err(ManagedBufferError::LengthOutOfRange { length, limit });
    }

    // Step 2: Reserve the memory explicitly. A failed reservation then comes
    // back as a normal error instead of aborting the host process.
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(length as usize)
        .map_err(|_| ManagedBufferError::AllocationFailed { length })?;
    buffer.resize(length as usize, 0);

    // Step 3: Hand the buffer to the managed copy function. The managed
    // contract refuses any buffer smaller than the payload, so a successful
    // copy means the whole payload was written. A refusal is an error, and the
    // buffer is dropped.
    if copy(buffer.as_mut_ptr(), length) == 0 {
        return Err(ManagedBufferError::CopyFailed);
    }
    Ok(buffer)
}

/// Reports a failed manifest fetch as the typed error the load fails with.
///
/// The startup and AOT paths both fail the whole load on any of these, so they
/// share one mapping instead of each defining its own.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) fn manifest_fetch_error(error: ManagedBufferError) -> CSharpError {
    match error {
        ManagedBufferError::LengthOutOfRange { length, limit } => {
            CSharpError::ManifestLengthOutOfRange { length, limit }
        }
        ManagedBufferError::AllocationFailed { .. } => CSharpError::ManifestAllocationFailed,
        ManagedBufferError::CopyFailed => CSharpError::ManifestCopyFailed,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload of the reported length arrives whole.
    #[test]
    fn a_reported_payload_is_fetched_in_full() {
        let payload = b"manifest";
        let fetched = fetch_managed_buffer(
            || payload.len() as u32,
            |pointer, length| {
                // SAFETY: the helper allocated `length` bytes at `pointer`, and
                // `length` is the value the length closure returned, which is
                // `payload.len()`.
                unsafe {
                    std::ptr::copy_nonoverlapping(payload.as_ptr(), pointer, length as usize)
                };
                1
            },
            1024,
        )
        .expect("a bounded length and a successful copy");
        assert_eq!(fetched, payload);
    }

    /// A length above the bound is refused before anything is allocated.
    ///
    /// Without the check, a corrupt length would become a huge allocation
    /// attempt instead of an immediate error.
    #[test]
    fn an_oversized_length_is_refused_without_allocating() {
        let error = fetch_managed_buffer(
            || u32::MAX,
            |_, _| panic!("the copy must not be reached for a refused length"),
            64,
        )
        .expect_err("a length above the bound is refused");
        assert_eq!(
            error,
            ManagedBufferError::LengthOutOfRange {
                length: u32::MAX,
                limit: 64,
            }
        );
    }

    /// A reported length of zero is the absence of a payload, not an empty one.
    #[test]
    fn a_zero_length_is_refused() {
        let error = fetch_managed_buffer(
            || 0,
            |_, _| panic!("the copy must not be reached for a refused length"),
            64,
        )
        .expect_err("zero is outside the accepted range");
        assert_eq!(
            error,
            ManagedBufferError::LengthOutOfRange {
                length: 0,
                limit: 64,
            }
        );
    }

    /// A refused copy is a failure, not a buffer of zeroes.
    #[test]
    fn a_refused_copy_discards_the_buffer() {
        let error = fetch_managed_buffer(|| 8, |_, _| 0, 64)
            .expect_err("the managed side declined to fill the buffer");
        assert_eq!(error, ManagedBufferError::CopyFailed);
    }

    /// Each failure maps to the manifest error the load reports.
    #[test]
    fn manifest_failures_map_to_their_typed_errors() {
        assert!(matches!(
            manifest_fetch_error(ManagedBufferError::LengthOutOfRange {
                length: 9,
                limit: 4
            }),
            CSharpError::ManifestLengthOutOfRange {
                length: 9,
                limit: 4
            }
        ));
        assert!(matches!(
            manifest_fetch_error(ManagedBufferError::AllocationFailed { length: 9 }),
            CSharpError::ManifestAllocationFailed
        ));
        assert!(matches!(
            manifest_fetch_error(ManagedBufferError::CopyFailed),
            CSharpError::ManifestCopyFailed
        ));
    }
}
