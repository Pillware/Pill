//! `#[pill_mirror_impl]` must accept `#[pill_mirror_method]` written either
//! bare or as a fully qualified path.
//!
//! # Responsibilities
//!
//! - Declare one `#[derive(PillMirror)]` value type with the marker in both
//!   spellings.
//! - Pin that both methods reach the mirror-method registry this test binary
//!   builds. A skipped method is simply missing from the generated C# mirror,
//!   so the loss only shows up when C# code that uses it fails to compile.

// External crates
use pill_engine::component_registry::mirror_method_descriptors;
use pill_engine::{pill_mirror_impl, pill_mirror_method, PillMirror};

/// A value type whose two methods pin the two marker spellings.
#[repr(C)]
#[derive(Clone, Copy, PillMirror)]
struct MarkerProbe {
    /// Input the mirrored methods read.
    value: u64,
}

#[pill_mirror_impl]
impl MarkerProbe {
    /// The bare spelling, imported above.
    #[pill_mirror_method]
    fn bare(&self) -> u64 {
        self.value + 1
    }

    /// The fully qualified spelling, no import needed.
    #[pill_engine::pill_mirror_method]
    fn qualified(&self) -> u64 {
        self.value + 2
    }
}

/// Both spellings must register a descriptor against the declaring type.
#[test]
fn both_marker_spellings_register_the_method() {
    let methods = mirror_method_descriptors();
    for name in ["bare", "qualified"] {
        let descriptor = methods
            .iter()
            .find(|descriptor| descriptor.name == name)
            .unwrap_or_else(|| panic!("`{name}` was skipped by the macro"));
        assert!(
            descriptor.type_name.ends_with("::MarkerProbe"),
            "registered against the declaring type, got {}",
            descriptor.type_name
        );
        assert!(!descriptor.is_free_function);
        assert_eq!(descriptor.return_tag, "u64");
    }
}
