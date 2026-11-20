//! Shared core definitions of the Pill engine: the one library every DLL links.
//!
//! # Responsibilities
//!
//! - Own the error types every workspace crate uses, built from one semantic
//!   message definition per leaf variant and rendered as plain or styled text.
//! - Own the telemetry foundation: the static log targets, the developer log
//!   macros, the terminal line formatter, the log subscriber builder and the
//!   metrics recorder.
//! - Own the platform layer ([`platform`]): clock, log output, futures and
//!   terminal styling, the only code that differs between native and the web.
//! - Own the Tracy profiling API (`profile_scope!` and the other `profile_*`
//!   macros), with its feature gates and no-op fallbacks.
//! - Own the state that must have one copy per process: the Rayon thread
//!   pool, the native buffer allocation service and the per-thread change
//!   ticks (counters that record when each system last ran).
//! - Provide the plain-data and helper types the DLLs share: inline strings,
//!   native-backed buffers, slot maps, `PillStyle`, color palettes, math
//!   aliases and small utilities.
//! - Re-export the `tracing` and `metrics` macros, so a caller imports every
//!   telemetry entry point from `pill_core` alone.
//!
//! # Design
//!
//! The crate builds both as a `dylib` and as an `rlib`. Every DLL links the
//! one shared copy, so process-global state has a single instance across the
//! DLL boundary. A monolithic release-profile build can link everything
//! statically instead.
//!
//! The error system lives in [`error`]. A subsystem declares its errors with
//! `#[engine_error]`, and they combine into [`error::HostError`] without
//! adding text of their own. A renderer turns one semantic message definition
//! into plain or styled output. The telemetry system lives in [`telemetry`]
//! and [`metrics`].

// =============================================================================
// Modules
// =============================================================================

/// Color palettes for debug visualization.
pub mod color;
/// A growable run of plain (`Copy`) elements in engine-owned native memory.
pub mod dynamic_buffer;
/// Semantic error system shared by every workspace crate.
pub mod error;
/// Inline, fixed-capacity strings for plain-data structs.
pub mod fixed_string;
/// Math type aliases over `glam` and the direction vocabulary.
pub mod math;
/// Metrics recording foundation, gated on the `metrics` feature.
#[cfg(feature = "metrics")]
pub mod metrics;
/// Engine-owned native buffer allocation, shared by every loaded DLL.
pub mod native_buffer;
/// The platform layer: clock, log output, futures and terminal styling on
/// native and on the web.
pub mod platform;
/// Tracy profiling API with feature gating and no-op fallbacks.
pub mod profiling;
/// Generational slot arena for values addressed by stable handles.
pub mod slot_map;
/// `PillStyle` string-styling vocabulary for terminal output.
pub mod style;
/// Per-thread change ticks of the system running on the current thread, shared
/// by every loaded DLL.
pub mod system_ticks;
/// Telemetry foundation: the log targets, the log filter, the line formatter
/// and the subscriber builder.
pub mod telemetry;
/// Small general-purpose helpers shared by Pill crates.
pub mod utils;

// =============================================================================
// Flat Re-exports: engine types and shared crates
// =============================================================================

/// The engine-owned dynamic buffer: a growable run of plain elements in
/// native memory.
pub use dynamic_buffer::DynamicBuffer;
/// `FixedString64` and `FIXED_STRING_64_CAPACITY`: an inline string type and
/// the capacity it allows.
pub use fixed_string::{FixedString64, FIXED_STRING_64_CAPACITY};
/// The `PillStyle` trait, which styles text for terminal output.
pub use style::PillStyle;
/// The `tracing` crate, re-exported so downstream crates import telemetry
/// macros from `pill_core` alone.
pub use tracing;

/// The `rayon` crate, re-exported so every DLL runs parallel work on the one
/// global thread pool inside `pill_core.dll`, which never unloads. Use
/// `rayon` through this path. A module must not build a pool of its own
/// (`ThreadPoolBuilder::build`): its threads would keep running in the
/// module's code, and a reload unloads that code.
pub use rayon;

// Permanent logging macros. `error` has the same name as the `pub mod error`
// module; Rust keeps macros and modules in separate namespaces, so both
// resolve.
pub use tracing::{debug, error, info, span, trace, trace_span, warn};

// Metrics macros, gated on the `metrics` feature. The leading `::` selects
// the external `metrics` crate instead of the local `pub mod metrics`.
#[cfg(feature = "metrics")]
pub use ::metrics::{counter, gauge, histogram};

// The `tracy_client` crate, re-exported so the `profile_*` macros can call it
// through `$crate::tracy_client::...` in any crate, without that crate
// declaring a dependency on it.
#[cfg(any(
    feature = "profiling",
    feature = "profiling-minimal",
    feature = "tracy"
))]
pub use tracy_client;

// =============================================================================
// Simple Developer Logging Macros
// =============================================================================

/// Writes a temporary developer log at `DEBUG` level on the `engine::dev`
/// lane ([`DEV_LOG_TARGET`]).
///
/// Only available with the `dev-logs` feature. Without it the macro expands
/// to nothing. Use these macros for diagnostics while developing, never for
/// permanent telemetry.
///
/// [`DEV_LOG_TARGET`]: telemetry::DEV_LOG_TARGET
#[cfg(feature = "dev-logs")]
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::tracing::debug!(target: $crate::telemetry::DEV_LOG_TARGET, $($arg)*)
    };
}

/// The version of [`log!`] used when the `dev-logs` feature is off. It
/// expands to nothing.
#[cfg(not(feature = "dev-logs"))]
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {};
}

/// Writes a temporary developer warning at `WARN` level on the `engine::dev`
/// lane. Only available with the `dev-logs` feature.
#[cfg(feature = "dev-logs")]
#[macro_export]
macro_rules! dev_warn {
    ($($arg:tt)*) => {
        $crate::tracing::warn!(target: $crate::telemetry::DEV_LOG_TARGET, $($arg)*)
    };
}

/// The version of [`dev_warn!`] used when the `dev-logs` feature is off. It
/// expands to nothing.
#[cfg(not(feature = "dev-logs"))]
#[macro_export]
macro_rules! dev_warn {
    ($($arg:tt)*) => {};
}

/// Writes a temporary developer error at `ERROR` level on the `engine::dev`
/// lane. Only available with the `dev-logs` feature.
#[cfg(feature = "dev-logs")]
#[macro_export]
macro_rules! dev_error {
    ($($arg:tt)*) => {
        $crate::tracing::error!(target: $crate::telemetry::DEV_LOG_TARGET, $($arg)*)
    };
}

/// The version of [`dev_error!`] used when the `dev-logs` feature is off. It
/// expands to nothing.
#[cfg(not(feature = "dev-logs"))]
#[macro_export]
macro_rules! dev_error {
    ($($arg:tt)*) => {};
}
