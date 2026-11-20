//! The C# mirror fixtures: the value types and functions the C# bridge suite
//! calls without any component row involved.
//!
//! # Responsibilities
//!
//! - Declare a standalone mirrored value type ([`Tint`]), so the module's
//!   generated C# file carries a value struct with fixed field offsets.
//! - Declare a value type with a mirrored method ([`Counter::value_as_u64`]),
//!   which reaches C# as an instance method calling the module's trampoline.
//! - Declare a mirrored free function ([`tint_alpha`]), which reaches C# as the
//!   static `pill_test.Mirror.TintAlpha()`.
//!
//! # Design
//!
//! The suite checks the generated file's text and the value a managed call
//! returns, so the shapes and the returned value are part of its contract:
//! `Tint` keeps three `f32` fields (`R` at offset 0, `B` at offset 8), and
//! `tint_alpha` returns `1.0`, which the probe prints as `alpha=1`. The free
//! function's C# class is named after this module, so moving it to another
//! module renames the class the suite calls.

// External crates
use pill_engine::*;

/// A colour with three channels, mirrored to C# as `pill_test.Tint`.
///
/// No component holds it. It exists so the generated mirror carries a
/// standalone value type, which the managed side can construct and pass by
/// value.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PillMirror)]
pub struct Tint {
    /// The red channel, mirrored at offset 0.
    pub r: f32,
    /// The green channel, mirrored at offset 4.
    pub g: f32,
    /// The blue channel, mirrored at offset 8.
    pub b: f32,
}

/// A value type with a mirrored method, mirrored to C# as `pill_test.Counter`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PillMirror)]
pub struct Counter {
    /// The counted value; [`Counter::value_as_u64`] reads it.
    pub value: i32,
}

#[pill_mirror_impl]
impl Counter {
    /// The value widened to an unsigned 64-bit integer; mirrored to C# as
    /// `ValueAsU64()`, an instance method that borrows its receiver.
    ///
    /// A negative value wraps, which is the plain `as` conversion and all the
    /// suite needs from it.
    #[pill_mirror_method]
    pub fn value_as_u64(&self) -> u64 {
        self.value as u64
    }
}

/// The alpha channel the bridge probe reads; mirrored to C# as the static
/// `pill_test.Mirror.TintAlpha()`.
///
/// It returns a constant, so the probe's `alpha=1` proves a managed call
/// reached this code and brought its result back.
#[pill_mirror_fn]
pub fn tint_alpha() -> f32 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calls [`tint_alpha`] through its trampoline, the way the managed side
    /// does, and decodes the `f32` the trampoline writes.
    fn managed_tint_alpha() -> f32 {
        let mut return_buffer = [0u8; pill_engine::mirror::SLOT_SIZE];
        // SAFETY: the function takes no arguments, so the trampoline never
        // reads the null argument pointer, and the result buffer is one full
        // slot, which is what a `f32` result needs.
        let status =
            unsafe { pill_mirror_fn_tint_alpha(std::ptr::null(), return_buffer.as_mut_ptr()) };
        assert_eq!(status, pill_engine::mirror::STATUS_OK);
        f32::from_le_bytes([
            return_buffer[0],
            return_buffer[1],
            return_buffer[2],
            return_buffer[3],
        ])
    }

    /// The managed call returns what the Rust function returns.
    #[test]
    fn the_free_function_trampoline_returns_the_function_result() {
        assert_eq!(managed_tint_alpha(), tint_alpha());
    }

    /// The free function, the method and both value types reach the registries
    /// the host's C# code generator reads.
    #[test]
    fn the_mirrored_declarations_reach_the_registries() {
        let free_function_descriptor = pill_engine::component_registry::mirror_method_descriptors()
            .into_iter()
            .find(|candidate| candidate.name == "tint_alpha" && candidate.is_free_function)
            .expect("the mirrored free function is registered");
        assert_eq!(
            free_function_descriptor.type_name, "pill_test::mirror",
            "declared in this module, whose name becomes the C# class"
        );
        assert_eq!(free_function_descriptor.return_tag, "f32");
        assert!(
            free_function_descriptor.arg_tags.is_empty(),
            "no arguments to pass"
        );

        let mirrored_method = pill_engine::component_registry::mirror_method_descriptors()
            .into_iter()
            .find(|candidate| candidate.name == "value_as_u64")
            .expect("the mirrored method on Counter is registered");
        assert_eq!(mirrored_method.type_name, "pill_test::Counter");
        assert!(
            !mirrored_method.is_free_function,
            "a method, not a free function"
        );
        assert_eq!(mirrored_method.return_tag, "u64");
        assert_eq!(
            mirrored_method.receiver, "ref",
            "`value_as_u64` borrows its receiver"
        );

        let declared_type_names: Vec<&str> =
            pill_engine::component_registry::value_type_descriptors()
                .iter()
                .map(|value_type| value_type.type_name)
                .collect();
        for expected in ["pill_test::Tint", "pill_test::Counter"] {
            assert!(
                declared_type_names.contains(&expected),
                "{expected} is declared for the managed mirror: {declared_type_names:?}"
            );
        }
    }
}
