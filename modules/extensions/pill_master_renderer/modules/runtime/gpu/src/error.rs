//! Renderer failures, and how wgpu's errors are captured and turned into them.
//!
//! # Responsibilities
//!
//! - Re-export [`RendererError`], [`Result`] and [`ErrorContext`] from
//!   `pill_renderer_api`, where the host can name them without wgpu.
//! - Log the GPU errors that no error scope captured
//!   ([`report_uncaptured_errors`]), instead of letting wgpu panic on them.
//! - Latch a lost device ([`latch_device_loss`]) and pass it to the frame path
//!   ([`device_loss`]). A driver reset then ends the run with a reason, instead
//!   of leaving a window that never updates again.
//! - Capture the errors a block of GPU work provoked. While the renderer is
//!   built the capture is awaited ([`captured`]). In a native development build
//!   a capture can also be blocked on during a frame (`capturing_validation`).
//!
//! # Design
//!
//! Waiting for an error scope finishes asynchronously. A browser answers the
//! wait only when control returns to its event loop, so a frame cannot block on
//! a scope there. Blocking capture is therefore a development-only feature: it
//! lets a refused shader edit be reported and skipped. In every other build the
//! same refusal reaches the error handler, which logs it.
//!
//! A lost device reaches neither path. wgpu answers every call on a lost device
//! with success, so the only witness is the callback that
//! [`latch_device_loss`] installs. It has to be passed to the frame path rather
//! than logged and forgotten.

// Standard library
use std::sync::{Mutex, MutexGuard, PoisonError};

// External crates
use pill_core::telemetry::telemetry_target;
pub use pill_renderer_api::error::{ErrorContext, RendererError, Result};

// =============================================================================
// Constants
// =============================================================================

/// The report of the device loss this image has seen, if any.
///
/// This is module state rather than a renderer field because the loss arrives
/// on a driver thread, and the renderer that has to act on it is the one this
/// image built. A reload gives a fresh image with a fresh static, which is
/// correct: the replacement renderer creates its own device.
static DEVICE_LOSS: Mutex<Option<String>> = Mutex::new(None);

// =============================================================================
// Types
// =============================================================================

/// The errors that one block of GPU work provoked.
///
/// wgpu sorts errors into three classes. Each field holds the error for its
/// class, and stays `None` when no error of that class was reported.
#[derive(Debug, Default)]
pub(crate) struct CapturedErrors {
    /// An error wgpu raised while validating a call.
    pub(crate) validation: Option<wgpu::Error>,
    /// The device ran out of memory.
    pub(crate) out_of_memory: Option<wgpu::Error>,
    /// An internal error in the driver or wgpu.
    pub(crate) internal: Option<wgpu::Error>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Sends GPU errors that no error scope captured to the log.
///
/// wgpu panics on these by default, so one refused pipeline or lost surface
/// would take the whole process down. With this handler installed, the run
/// carries on without whatever was refused, and the log says why.
pub(crate) fn report_uncaptured_errors(device: &wgpu::Device) {
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        pill_core::error!(
            target: telemetry_target::RENDERING,
            error = %error,
            "GPU error; the refused call draws nothing"
        );
    }));
}

/// Locks the device loss latch.
///
/// A poisoned lock is still used: it holds the last value written, which is all
/// a reader needs.
fn latched_device_loss() -> MutexGuard<'static, Option<String>> {
    DEVICE_LOSS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Records a lost device, so the frame path can report it.
///
/// A lost device accepts every call and draws nothing. Writes, submits and
/// polls all return success, and no error is raised, so the frame path cannot
/// see that the picture stopped moving. The callback wgpu fires for the loss is
/// the only witness, and it runs on a driver thread. This function stores its
/// message for [`device_loss`], which the next frame reads.
///
/// Call it beside [`report_uncaptured_errors`] when the device is created. It
/// also clears a loss that an earlier device of this image recorded.
pub(crate) fn latch_device_loss(device: &wgpu::Device) {
    // Step 1: Clear any loss an earlier device of this image recorded.
    *latched_device_loss() = None;
    device.set_device_lost_callback(|reason, message| {
        // Step 2: Log the loss and store the message for the frame path.
        let detail = if message.is_empty() {
            format!("the GPU device was lost ({reason:?})")
        } else {
            format!("the GPU device was lost ({reason:?}): {message}")
        };
        pill_core::error!(
            target: telemetry_target::RENDERING,
            reason = ?reason,
            error = message.as_str(),
            "GPU device lost; nothing is drawn until the renderer is rebuilt"
        );
        *latched_device_loss() = Some(detail);
    });
}

/// Returns the reason to stop drawing when the device has been lost, or `None`
/// while it works.
pub(crate) fn device_loss() -> Option<String> {
    latched_device_loss().clone()
}

/// Runs `make` with wgpu's errors captured, and waits for wgpu's answers.
///
/// Use this for work done while the renderer is built. That work already runs
/// asynchronously, so both the browser and native builds can wait for the error
/// scopes.
pub(crate) async fn captured<T>(
    device: &wgpu::Device,
    make: impl FnOnce() -> T,
) -> (T, CapturedErrors) {
    // Step 1: Push one error scope for each class wgpu reports.
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let block_value = make();

    // Step 2: Pop the scopes. They form a stack, so the scope pushed last is
    // the one popped first.
    let internal = internal_scope.pop().await;
    let out_of_memory = out_of_memory_scope.pop().await;
    let validation = validation_scope.pop().await;
    let errors = CapturedErrors {
        validation,
        out_of_memory,
        internal,
    };
    (block_value, errors)
}

/// Runs `make` with wgpu's errors captured, and blocks until wgpu answers.
///
/// Only native development builds get this version. A browser frame cannot
/// block on an error scope, and only development has to survive a refused
/// shader edit.
#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
pub(crate) fn captured_now<T>(
    device: &wgpu::Device,
    make: impl FnOnce() -> T,
) -> (T, CapturedErrors) {
    pill_core::platform::futures::block_on(captured(device, make))
}

/// Runs `make` without capturing.
///
/// An error it provokes reaches the handler [`report_uncaptured_errors`]
/// installed, which logs it.
#[cfg(any(not(debug_assertions), target_arch = "wasm32"))]
pub(crate) fn captured_now<T>(
    _device: &wgpu::Device,
    make: impl FnOnce() -> T,
) -> (T, CapturedErrors) {
    (make(), CapturedErrors::default())
}

/// Runs a block of GPU work and captures any error wgpu raises for it.
///
/// wgpu reports a refused pipeline, bind group or texture to the device's
/// uncaptured-error handler. The call itself returns nothing to check. A
/// capture turns the refusal into a message the caller can attach to whatever
/// asked for the work, so the problem names the pass that is not drawn instead
/// of failing the whole frame. A build that cannot capture returns the block's
/// result, and the handler logs the refusal.
///
/// # Errors
///
/// Returns the text of the captured error: the validation error if the block
/// provoked one, otherwise the out-of-memory or internal error. Returns the
/// block's result as `Ok` when wgpu reported nothing.
pub(crate) fn capturing_validation<T>(
    device: &wgpu::Device,
    make: impl FnOnce() -> T,
) -> std::result::Result<T, String> {
    let (block_value, errors) = captured_now(device, make);
    match errors
        .validation
        .or(errors.out_of_memory)
        .or(errors.internal)
    {
        Some(error) => Err(error.to_string()),
        None => Ok(block_value),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // Current crate
    use super::{device_loss, latch_device_loss};

    /// A device with no surface, taken from the platform's fallback adapter
    /// (usually the software one).
    ///
    /// Returns `None` when the platform offers no adapter.
    async fn windowless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::from_build_config(),
            backend_options: wgpu::BackendOptions::default(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: true,
                apply_limit_buckets: false,
            })
            .await
            .ok()?;
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device loss test device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .ok()
    }

    /// A destroyed device accepts every call and reports no error, so the frame
    /// path can only learn about the loss from the latch the callback fills.
    #[test]
    fn a_lost_device_is_latched_and_reported() {
        let Some((device, _queue)) = pill_core::platform::futures::block_on(windowless_device())
        else {
            eprintln!("no GPU adapter available; skipped");
            return;
        };
        // The callback also logs, but this test checks the latch that the frame
        // path reads. It installs the callback through the same entry point the
        // renderer uses.
        latch_device_loss(&device);
        assert_eq!(device_loss(), None, "a fresh device reports no loss");

        device.destroy();
        // wgpu reports the loss only after the queue has drained.
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok();

        let reported = device_loss().expect("the destroyed device is latched as lost");
        assert!(
            reported.contains("lost"),
            "the report says what happened: {reported}"
        );
    }
}
