//! Placeholder extension with no ECS state: a deterministic pseudo-random
//! generator.
//!
//! # Responsibilities
//!
//! - Defines the [`LcgRandom`] struct with two generator methods.
//! - Exposes the [`seed_from_u32`] free function.
//! - Registers through the extension ABI when the host loads it.
//!
//! # Design
//!
//! The crate carries no ECS state. [`register`] is a no-op that only reports
//! success, and it is a plain Rust function so a build that links this crate
//! statically can call it directly. The generator is linear congruential: it
//! returns the same sequence for the same seed, so it is only useful as dummy
//! data, never for real randomness.

// External crates
// `pill_module` and `Engine` must be available in every build: `register` is
// always compiled, and the attribute is written on it in the source.
use pill_engine::pill_module;
use pill_engine::Engine;

// =============================================================================
// Struct
// =============================================================================

/// Dummy linear congruential generator producing a deterministic sequence.
#[derive(Debug, Clone, Copy)]
pub struct LcgRandom {
    /// Current value that the next number is derived from.
    pub state: u32,
}

impl LcgRandom {
    /// Advances the generator and returns the next value in its sequence.
    pub fn next_u32(&mut self) -> u32 {
        // Multiplier and increment from Numerical Recipes. They make the
        // sequence deterministic, not statistical.
        self.state = self
            .state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.state
    }

    /// Returns the next value mapped into `0.0..=1.0`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() as f32) / (u32::MAX as f32)
    }
}

// =============================================================================
// Free functions
// =============================================================================

/// Builds a generator seeded from `seed`.
pub fn seed_from_u32(seed: u32) -> LcgRandom {
    LcgRandom { state: seed }
}

// =============================================================================
// Registration
// =============================================================================

/// Registers the module against the host engine. Returns zero on success.
///
/// This is the half a reload repeats. The module adds nothing to the engine, so
/// the call does nothing; it exists so every in-repo artifact answers the same
/// contract. A second call must leave the engine in the same state.
pub fn register(_engine: &mut Engine) -> u32 {
    0
}

/// The artifact's one-time entry point.
///
/// The module keeps nothing that has to run once, so this function defers to
/// [`register`]. It exists because a statically linked build calls it directly
/// and because the generated `host_module_pill_dummy_random` wrapper carries it
/// as `pill_module_init`.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}
