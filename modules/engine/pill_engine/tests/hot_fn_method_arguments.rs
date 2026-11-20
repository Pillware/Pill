//! Integration tests for `#[pill_hot_fn]` on a method with parameters.
//!
//! `#[pill_hot_fn]` keeps the method's name for a small dispatcher. That
//! dispatcher calls an installed replacement when the slot holds one, and the
//! method's own body otherwise.
//!
//! # Responsibilities
//!
//! - Pin that a method annotated `#[pill_hot_fn]` compiles when it declares
//!   parameters of its own. The dispatcher forwards each one under the name
//!   the body uses, so the body runs unchanged.
//! - Pin that such a method answers with its own body, follows an installed
//!   replacement, and returns to its own body on a reset - the same lifecycle
//!   a method without parameters has.
//! - Pin that a hand-written copy of the host's generated method patch records
//!   the same signature text as the running method. Matching text is what lets
//!   its replacement pass the signature check at install time.

// External crates
use pill_engine::hot_patch::{
    install_plain_function, plain_function_signature, reset_plain_function,
};
use pill_engine::pill_hot_fn;

/// A receiver type whose methods take arguments, so the attribute has
/// parameters to forward and not only `self`.
struct Scaler {
    /// The multiplier that [`Scaler::scale`] applies to its argument.
    factor: f32,
}

impl Scaler {
    /// Multiplies `value` by the receiver's factor.
    ///
    /// This is the plain-parameter case. The body names `value` directly, so
    /// the dispatcher must forward the argument under that same name.
    #[pill_hot_fn]
    fn scale(&self, value: f32) -> f32 {
        self.factor * value
    }

    /// Adds half the receiver's factor to `delta`, through a `mut` binding.
    ///
    /// Pins that the `mut` on the parameter survives the macro: the generated
    /// declaration keeps it, and the dispatcher forwards the name it binds.
    #[pill_hot_fn]
    fn nudge(&self, mut delta: f32) -> f32 {
        delta += self.factor * 0.5;
        delta
    }
}

/// Replacement for [`Scaler::scale`], installed by the first test.
///
/// It returns a constant that the method's own body cannot produce for the
/// values the test passes, so the two answers cannot be confused.
fn scale_replacement(_scaler: &Scaler, _value: f32) -> f32 {
    777.0
}

/// A hand-written copy of the code the host generates for a method patch.
///
/// The generated copy keeps the same declaration, is registered under a patch
/// name, and names the concrete receiver type. Both copies must record the same
/// signature text, because the install check refuses a replacement whose
/// signature differs from the running method's. The last test below checks
/// that.
mod method_patch {
    // External crates
    use pill_engine::pill_hot_fn;

    // Current crate
    use super::Scaler;

    /// The patch copy of [`super::Scaler::nudge`]. With `self_type`, the
    /// attribute moves the body into a local trait implemented for the receiver
    /// type, which is how a generated patch keeps `self` working.
    #[pill_hot_fn(name = "test_patch::nudge", self_type = Scaler)]
    fn nudge(&self, mut delta: f32) -> f32 {
        delta += self.factor * 0.5;
        delta
    }
}

// =============================================================================
// Dispatch
// =============================================================================

/// A method with a plain parameter follows the install/reset lifecycle. It
/// answers with its own body first, with the replacement once installed, and
/// with its own body again after a reset.
#[test]
fn a_method_with_a_plain_parameter_dispatches_through_its_slot() {
    let scaler = Scaler { factor: 2.0 };
    // Step 1: Before any install, the call reaches the method's own body.
    assert_eq!(scaler.scale(3.0), 6.0, "the method's own body first");

    // Step 2: Record the method's signature and install the replacement.
    let qualified_name = concat!(module_path!(), "::scale");
    let signature = plain_function_signature(qualified_name)
        .expect("the attribute registers the method under its module path");
    install_plain_function(
        qualified_name,
        scale_replacement as *const () as usize,
        signature,
    )
    .expect("a replacement of the recorded shape is accepted");

    // Step 3: The same call now reaches the replacement.
    assert_eq!(scaler.scale(3.0), 777.0, "callers see the replacement");

    // Step 4: A reset returns the method to its own body.
    reset_plain_function(qualified_name).expect("a reset finds the registered method");
    assert_eq!(scaler.scale(3.0), 6.0, "a reset returns it to its own body");
}

/// A `mut` parameter binding compiles, and the call still reaches the method's
/// own computation.
#[test]
fn a_method_with_a_mut_parameter_keeps_its_computation() {
    let scaler = Scaler { factor: 4.0 };
    assert_eq!(scaler.nudge(1.0), 3.0, "delta plus half the factor");
}

/// A generated method patch records the same signature text as the running
/// method. That is what lets its replacement pass the signature check when it
/// is installed.
#[test]
fn a_generated_method_patch_records_the_running_signature() {
    let running_signature = plain_function_signature(concat!(module_path!(), "::nudge"))
        .expect("the running method is registered");
    let patch_signature = plain_function_signature("test_patch::nudge")
        .expect("the patch copy is registered under the patch name");
    assert_eq!(
        patch_signature, running_signature,
        "both copies must derive one signature from one declaration"
    );
}
