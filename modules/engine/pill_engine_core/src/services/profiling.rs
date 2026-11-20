//! Re-exports the profiling API from `pill_core`.
//!
//! # Responsibilities
//!
//! - Makes `crate::profiling::*` resolve to [`pill_core::profiling`], so
//!   engine code and downstream users name the profiling API in one place.
//!
//! # Design
//!
//! The implementation, the feature gating and every `#[macro_export]`
//! profiling macro are defined in `pill_core::profiling`. This module only
//! re-exports them. The crate root also re-exports those macros, so the many
//! internal call sites can write `crate::profile_scope!` and the other
//! `crate::profile_*` macros.

// External crates

/// Re-exports every public item of [`pill_core::profiling`] at
/// `crate::profiling`, so engine code and downstream users use one profiling
/// module.
pub use pill_core::profiling::*;
