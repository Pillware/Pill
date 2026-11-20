//! Placeholder extension with no ECS state.
//!
//! # Responsibilities
//!
//! - Defines the [`Adder`] struct with two arithmetic methods.
//! - Exposes the [`double`] free function.
//! - Registers through the extension ABI when the host loads it.
//!
//! # Design
//!
//! [`register`] does nothing except report success. It stays a plain Rust
//! function, so a build that links this crate statically can call it directly.

// External crates
// `pill_module` and `Engine` must resolve in every build: the attribute is
// applied to `register` in source, and `register` is compiled everywhere.
use pill_engine::pill_module;
use pill_engine::Engine;

// =============================================================================
// Struct
// =============================================================================

/// Dummy accumulator exercising basic arithmetic.
#[derive(Debug, Clone, Copy, Default)]
pub struct Adder {
    /// Sum of every amount added since the last reset.
    pub total: i32,
}

impl Adder {
    /// Adds `amount_to_add` to the running total and returns the new total.
    pub fn add(&mut self, amount_to_add: i32) -> i32 {
        self.total += amount_to_add;
        self.total
    }

    /// Resets the running total to zero.
    pub fn reset(&mut self) {
        self.total = 0;
    }
}

// =============================================================================
// Free functions
// =============================================================================

/// Doubles the given number.
pub fn double(number_to_double: i32) -> i32 {
    number_to_double * 2
}

// =============================================================================
// Registration
// =============================================================================

/// Registers the module against the host engine and returns zero.
///
/// This is the half a reload repeats. The module adds nothing to the engine, so
/// the call does nothing; it exists so every in-repo artifact answers the same
/// contract. A second run must leave the engine as the first one found it.
pub fn register(_engine: &mut Engine) -> u32 {
    0
}

/// The artifact's one-time entry point.
///
/// The module keeps nothing that has to run once, so this function defers to
/// [`register`]. It exists because a statically linked build calls it directly
/// and because the generated `host_module_pill_dummy_math` wrapper carries it
/// as `pill_module_init`.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}
