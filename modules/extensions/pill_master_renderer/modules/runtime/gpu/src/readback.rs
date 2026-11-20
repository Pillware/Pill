//! Mapping a GPU buffer back to the CPU without stalling a frame.
//!
//! # Responsibilities
//!
//! - Start a buffer's mapping once the frame that wrote it was submitted
//!   ([`MapState::start`]).
//! - Poll that mapping without waiting, hand the mapped bytes to a decoder and
//!   report one of three outcomes ([`MapState::poll`]).
//! - Own a readback buffer together with its mapping, for a caller that needs
//!   both ([`MappedReadback`]).
//!
//! # Design
//!
//! Reading a buffer back cannot block a frame, so the work is split. The copy
//! is recorded while the frame is built, and only after that frame is
//! submitted does [`MapState::start`] ask the device to map the buffer. Asking
//! earlier would make the submission fail. A later poll decodes whatever has
//! arrived by then.
//!
//! Neither a map that failed nor bytes that cannot be read may leave a caller
//! waiting, so both cases report [`Readback::Failed`], and the caller answers
//! its own request with nothing.
//!
//! The state lives behind an [`Arc`] because the map callback runs on the
//! device's own thread while the frame path polls it.

// Standard library
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

// External crates
use wgpu::Buffer;

// =============================================================================
// Mapping State Codes
// =============================================================================

/// The map callback has not reported yet.
const MAPPING_PENDING: u8 = 0;
/// The callback reported that the buffer is mapped and readable.
const MAPPING_DONE: u8 = 1;
/// The callback reported that the device refused to map the buffer.
const MAPPING_FAILED: u8 = 2;

// =============================================================================
// Readback
// =============================================================================

/// What one [`MapState::poll`] found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Readback<T> {
    /// The map has not finished yet. Poll again later.
    Pending,
    /// The map succeeded and the bytes were decoded.
    Done(T),
    /// The map or the read failed. Nothing will arrive for this one.
    Failed,
}

// =============================================================================
// Map Outcome
// =============================================================================

/// What the device's map callback reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MapOutcome {
    /// The map is still under way.
    Pending,
    /// The buffer is mapped and readable.
    Mapped,
    /// The device refused to map the buffer.
    Failed,
}

// =============================================================================
// Map State
// =============================================================================

/// One buffer's mapping, shared with the callback that reports it.
///
/// The callback stores its outcome with `Release` and the frame path reads it
/// with `Acquire`, so a thread that reads the new outcome also sees everything
/// the callback wrote before it stored that outcome.
pub(crate) struct MapState {
    outcome_code: Arc<AtomicU8>,
}

impl MapState {
    /// Asks for `buffer` to be mapped for reading.
    ///
    /// Only call this after the submission that wrote the buffer. A map asked
    /// for earlier makes that submission fail.
    pub(crate) fn start(buffer: &Buffer) -> Self {
        // Step 1: Make one code cell that this state and the callback share.
        let outcome_code = Arc::new(AtomicU8::new(MAPPING_PENDING));
        let callback_outcome_code = Arc::clone(&outcome_code);
        // Step 2: Ask for the map. The callback stores what came of it.
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let reported_code = if result.is_ok() {
                    MAPPING_DONE
                } else {
                    MAPPING_FAILED
                };
                callback_outcome_code.store(reported_code, Ordering::Release);
            });
        Self { outcome_code }
    }

    /// Whether the map has finished, whether it succeeded or failed.
    ///
    /// The frame profiler tests this for each slot of its ring before it reads
    /// the slot: a slot is read once every map it asked for has finished.
    pub(crate) fn finished(&self) -> bool {
        self.outcome() != MapOutcome::Pending
    }

    /// What the device's callback last reported.
    pub(crate) fn outcome(&self) -> MapOutcome {
        match self.outcome_code.load(Ordering::Acquire) {
            MAPPING_DONE => MapOutcome::Mapped,
            MAPPING_FAILED => MapOutcome::Failed,
            _ => MapOutcome::Pending,
        }
    }

    /// Polls the mapping without waiting.
    ///
    /// When the map is done, the bytes are handed to `read` once and the buffer
    /// is unmapped. A map the device refused answers [`Readback::Failed`] and
    /// the buffer is never read.
    pub(crate) fn poll<T>(&self, buffer: &Buffer, read: impl FnOnce(&[u8]) -> T) -> Readback<T> {
        match self.outcome() {
            MapOutcome::Pending => Readback::Pending,
            MapOutcome::Failed => Readback::Failed,
            MapOutcome::Mapped => read_mapped(buffer, read),
        }
    }

    /// A state whose callback already reported `outcome`, for the tests that
    /// drive the failure branches without a device.
    #[cfg(test)]
    pub(crate) fn at_outcome(outcome: MapOutcome) -> Self {
        let outcome_code = match outcome {
            MapOutcome::Pending => MAPPING_PENDING,
            MapOutcome::Mapped => MAPPING_DONE,
            MapOutcome::Failed => MAPPING_FAILED,
        };
        Self {
            outcome_code: Arc::new(AtomicU8::new(outcome_code)),
        }
    }
}

// =============================================================================
// Mapped Readback
// =============================================================================

/// A readback buffer that remembers its own mapping.
pub(crate) struct MappedReadback {
    buffer: Buffer,
    map_state: Option<MapState>,
}

impl MappedReadback {
    /// Wraps a buffer created with `MAP_READ`, with nothing mapped yet.
    pub(crate) fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            map_state: None,
        }
    }

    /// The buffer, for the copies a frame records into it.
    pub(crate) fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Asks for the map, unless one is already under way.
    pub(crate) fn start(&mut self) {
        if self.map_state.is_none() {
            self.map_state = Some(MapState::start(&self.buffer));
        }
    }

    /// Polls the map without waiting, decoding through `read` once.
    ///
    /// While no map is under way this answers [`Readback::Pending`], and keeps
    /// answering it until [`Self::start`] asks for the next map.
    pub(crate) fn poll<T>(&mut self, read: impl FnOnce(&[u8]) -> T) -> Readback<T> {
        let Some(map_state) = self.map_state.as_ref() else {
            return Readback::Pending;
        };
        let readback = map_state.poll(&self.buffer, read);
        if !matches!(readback, Readback::Pending) {
            self.map_state = None;
        }
        readback
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Reads a mapped buffer through `read`, unmapping it either way.
///
/// A read that fails still unmaps the buffer. Leaving it mapped would stop
/// every later use of the buffer.
pub(crate) fn read_mapped<T>(buffer: &Buffer, read: impl FnOnce(&[u8]) -> T) -> Readback<T> {
    match buffer.slice(..).get_mapped_range() {
        Ok(bytes) => {
            let decoded = read(&bytes);
            drop(bytes);
            buffer.unmap();
            Readback::Done(decoded)
        }
        Err(_) => {
            buffer.unmap();
            Readback::Failed
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_map_is_finished_and_reports_failed() {
        let map_state = MapState::at_outcome(MapOutcome::Failed);

        assert!(
            map_state.finished(),
            "a failed map is finished, not pending"
        );
        assert_eq!(map_state.outcome(), MapOutcome::Failed);
    }

    #[test]
    fn a_pending_map_is_not_finished() {
        let map_state = MapState::at_outcome(MapOutcome::Pending);

        assert!(!map_state.finished());
        assert_eq!(map_state.outcome(), MapOutcome::Pending);
    }

    #[test]
    fn a_mapped_map_is_finished_and_reports_mapped() {
        let map_state = MapState::at_outcome(MapOutcome::Mapped);

        assert!(map_state.finished());
        assert_eq!(map_state.outcome(), MapOutcome::Mapped);
    }
}
