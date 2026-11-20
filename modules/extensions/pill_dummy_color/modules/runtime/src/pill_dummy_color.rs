//! An example extension with no ECS state. It provides a colour value type and
//! an alpha function that can be hot-patched.
//!
//! # Responsibilities
//!
//! - Defines [`Tint`] with its two colour methods and the [`grayscale`] free
//!   function.
//! - Exposes [`get_color_a`], hot-patchable and mirrored to C# as
//!   `pill_dummy_color.PillDummyColor.GetColorA()`.
//! - Declares [`Tint`] and [`TestStruct`] as value types, and mirrors
//!   `TestStruct::aaa` with a `pill_mirror_method` marker written with its full
//!   path. Together they cover the three shapes the code generator handles: a
//!   free function, a value type and a mirrored method.
//! - Registers through the extension ABI when the host loads it.
//!
//! # Design
//!
//! The crate carries no ECS state. [`register`] is a no-op that only reports
//! success, and it stays a plain Rust function so the same crate can also be
//! linked statically into a monolithic build. `pill_spline` depends on it, and
//! the managed C# test project in `devops/tests/projects/managed` loads it as a
//! module.

// External crates
// `pill_module` and `Engine` must be available in every build, because the
// attribute is written on `register` here and `register` is compiled
// everywhere.
use pill_engine::{pill_hot_fn, pill_mirror_fn, pill_module, PillMirror};
use pill_engine::{pill_mirror_impl, Engine};

// The build script scans this crate and writes one address entry per function
// into `function_inventory.rs`. Including that file is what lets the host find
// each function by its qualified path, with nothing annotated in this source.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Struct
// =============================================================================

/// A plain value type with a mirrored method.
///
/// [`PillMirror`] puts it in the managed mirror even though no component
/// exposes it, and the impl block below adds `aaa` to the module's mirrored
/// method table. That is the third shape the code generator handles, after the
/// free function and the plain value type. The method marker is spelled with
/// its full path on purpose: a qualified `pill_engine::pill_mirror_method`
/// must register exactly like the bare one.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PillMirror)]
pub struct TestStruct {
    /// The integer [`TestStruct::aaa`] converts to `u64`.
    pub value: i32,
}

#[pill_mirror_impl]
impl TestStruct {
    /// A mirrored method on a mirrored value type.
    #[pill_engine::pill_mirror_method]
    pub fn aaa(&self) -> u64 {
        self.value as u64
    }
}

/// A dummy RGB colour with two channel helpers.
///
/// It is a plain value type declared for the managed mirror. The C# code
/// generator writes it into the module's generated file even though no
/// component exposes it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PillMirror)]
pub struct Tint {
    /// Red channel, normally in the `0.0..=1.0` range.
    pub r: f32,
    /// Green channel, normally in the `0.0..=1.0` range.
    pub g: f32,
    /// Blue channel, normally in the `0.0..=1.0` range.
    pub b: f32,
}

impl Tint {
    /// Returns the colour halfway between this one and `other`, one channel at
    /// a time.
    pub fn mix(&self, other: Tint) -> Tint {
        Tint {
            r: (self.r + other.r) * 0.5,
            g: (self.g + other.g) * 0.5,
            b: (self.b + other.b) * 0.5,
        }
    }

    /// Returns the opposite colour, subtracting each channel from `1.0`.
    /// Expects channels in the `0.0..=1.0` range.
    pub fn invert(&self) -> Tint {
        Tint {
            r: 1.0 - self.r,
            g: 1.0 - self.g,
            b: 1.0 - self.b,
        }
    }
}

// =============================================================================
// Free functions
// =============================================================================

/// Returns the average of `tint`'s three channels as one grey level.
pub fn grayscale(tint: Tint) -> f32 {
    (tint.r + tint.g + tint.b) / 3.0
}

/// A dummy alpha channel. `Tint` carries no alpha, so this always reports
/// fully opaque. Other crates call it as a stand-in.
///
/// It is mirrored to C# as `pill_dummy_color.PillDummyColor.GetColorA()` and
/// is hot-patchable like any `#[pill_hot_fn]`. The mirror attribute comes
/// first so it records the original signature, which is the contract managed
/// code was compiled against. The body can still be replaced: after a patch, a
/// managed call runs the new code with no reload.
#[pill_mirror_fn]
#[pill_hot_fn]
pub fn get_color_a() -> f32 {
    1.0
}

// =============================================================================
// Registration
// =============================================================================

/// Registers the module with the host engine and returns zero on success.
///
/// This is the half a reload repeats. The module adds nothing to the engine -
/// its Mirrored declarations are compile-time registries - so the call does
/// nothing, and it exists so every in-repo artifact answers the same contract.
///
/// It must be idempotent: the host calls it on every generation, and when it
/// returns a non-zero status the host keeps the previous library.
pub fn register(_engine: &mut Engine) -> u32 {
    0
}

/// The artifact's one-time entry point.
///
/// The module keeps nothing that has to run once, so this function defers to
/// [`register`]. It exists because a statically linked build loads the module
/// once and calls this entry point alone, and because the generated
/// `host_module_pill_dummy_color` wrapper exposes it as `pill_module_init`, the
/// symbol the host looks up in a loaded DLL.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Calls `get_color_a` the way C# does, through its mirror trampoline, and
    /// reads the `f32` the trampoline writes into the return buffer.
    fn managed_get_color_a() -> f32 {
        let mut return_buffer = [0u8; pill_engine::mirror::SLOT_SIZE];
        // SAFETY: the function takes no arguments, so no slot is read, and the
        // return buffer holds the four bytes an `f32` result writes.
        let status = unsafe {
            crate::pill_mirror_fn_get_color_a(std::ptr::null(), return_buffer.as_mut_ptr())
        };
        assert_eq!(status, pill_engine::mirror::STATUS_OK);
        f32::from_le_bytes([
            return_buffer[0],
            return_buffer[1],
            return_buffer[2],
            return_buffer[3],
        ])
    }

    /// The function must be discoverable by its qualified path, which is how a
    /// host addresses it across the ABI.
    #[test]
    fn function_is_registered_under_its_qualified_path() {
        assert!(
            pill_engine::hot_patch::plain_function_names()
                .any(|name| name == "pill_dummy_color::get_color_a"),
            "declared functions: {:?}",
            pill_engine::hot_patch::plain_function_names().collect::<Vec<_>>()
        );
    }

    /// The mirrored declarations reach this artifact's registries. The free
    /// function is a free-function descriptor at the crate root, `Tint` and
    /// `TestStruct` are standalone value types, and `TestStruct::aaa` is a
    /// mirrored method on the latter.
    #[test]
    fn the_mirrored_declarations_reach_the_registries() {
        let free_function_descriptor = pill_engine::component_registry::mirror_method_descriptors()
            .into_iter()
            .find(|candidate| candidate.name == "get_color_a" && candidate.is_free_function)
            .expect("the mirrored free function is registered");
        assert_eq!(
            free_function_descriptor.type_name, "pill_dummy_color",
            "declared at the crate root"
        );
        assert_eq!(free_function_descriptor.return_tag, "f32");
        assert!(
            free_function_descriptor.arg_tags.is_empty(),
            "no arguments to pass"
        );

        let mirrored_method = pill_engine::component_registry::mirror_method_descriptors()
            .into_iter()
            .find(|candidate| candidate.name == "aaa")
            .expect("the mirrored method on TestStruct is registered");
        assert_eq!(mirrored_method.type_name, "pill_dummy_color::TestStruct");
        assert!(
            !mirrored_method.is_free_function,
            "a method, not a free function"
        );
        assert_eq!(mirrored_method.return_tag, "u64");
        assert_eq!(
            mirrored_method.receiver, "ref",
            "`aaa` borrows its receiver"
        );

        let declared_type_names: Vec<&str> =
            pill_engine::component_registry::value_type_descriptors()
                .iter()
                .map(|value_type| value_type.type_name)
                .collect();
        assert!(
            declared_type_names.contains(&"pill_dummy_color::Tint"),
            "Tint is declared for the managed mirror"
        );
        assert!(
            declared_type_names.contains(&"pill_dummy_color::TestStruct"),
            "TestStruct is declared for the managed mirror"
        );
    }

    /// A signature that does not match is refused, and the original
    /// implementation stays in place. Without this check, a reshaped function
    /// could be installed behind call sites compiled for the old shape.
    #[test]
    fn a_mismatched_signature_is_refused() {
        fn replacement() -> f32 {
            42.0
        }
        let install_attempt = pill_engine::hot_patch::install_plain_function(
            "pill_dummy_color::get_color_a",
            replacement as *const () as usize,
            "(some other shape)",
        );
        assert!(
            install_attempt.is_err(),
            "a changed signature must be refused"
        );
    }

    /// The dispatcher forwards to the original body. An installed replacement
    /// redirects every caller of the public name, and a reset returns the
    /// function to its own code.
    ///
    /// This is one test rather than three because the slot is a process-wide
    /// `static` and the test harness runs tests on several threads. Separate
    /// tests would see each other's installs, in whatever order the threads
    /// happened to interleave, so running the whole sequence in one body is
    /// what makes the result deterministic.
    #[test]
    fn the_slot_dispatches_installs_and_resets() {
        fn replacement() -> f32 {
            999.0
        }

        // Step 1: Read the value the function returns now.
        //
        // Read it instead of hard-coding it: the body is meant to be
        // hot-patched, so a pinned literal would fail as soon as someone
        // changes it.
        let original = get_color_a();
        assert_eq!(
            managed_get_color_a(),
            original,
            "the mirror trampoline forwards to the public name"
        );

        // Step 2: Install a replacement under the recorded signature.
        //
        // Use that text instead of writing one by hand: its spelling comes
        // from `stringify!` inside the macro.
        let signature =
            pill_engine::hot_patch::plain_function_signature("pill_dummy_color::get_color_a")
                .expect("the function must be registered");

        pill_engine::hot_patch::install_plain_function(
            "pill_dummy_color::get_color_a",
            replacement as *const () as usize,
            signature,
        )
        .expect("install with the recorded signature must be accepted");
        assert_eq!(get_color_a(), 999.0, "callers must see the replacement");
        assert_eq!(
            managed_get_color_a(),
            999.0,
            "and so must the managed call path - this is what a live patch reaches"
        );

        // Step 3: Reset the slot and check the original body answers again.
        pill_engine::hot_patch::reset_plain_function("pill_dummy_color::get_color_a")
            .expect("reset must find the registered function");
        assert_eq!(
            get_color_a(),
            original,
            "a reset must return the function to its own body"
        );
        assert_eq!(
            managed_get_color_a(),
            original,
            "the managed call path returns to the compiled body"
        );
    }
}
