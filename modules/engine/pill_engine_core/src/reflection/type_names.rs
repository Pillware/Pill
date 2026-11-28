//! Type names as the engine stores them: the crate, then the type.
//!
//! # Responsibilities
//!
//! - [`stable_type_name`](crate::type_names::stable_type_name): the type name
//!   the engine stores as data. A component or a persisted resource registers
//!   under it unless it declares a shared name of its own.
//! - [`crate_qualified_name`](crate::type_names::crate_qualified_name): the
//!   same rule applied to a name given as text.
//! - [`names_same_type`](crate::type_names::names_same_type): whether a name
//!   read back from a file (a scene's component key, an asset's `asset_type`)
//!   names a given type, including a name written before the rule existed.
//!
//! # Design
//!
//! A stored type name is `<crate>::<Type>`: the crate the type is compiled in,
//! then the type's own name, with every module in between left out. Moving a
//! type to another file therefore never renames it, and the declaring macros
//! build the same string at compile time from `CARGO_CRATE_NAME`. Type names
//! are unique within a crate, which is what keeps the short form unambiguous.
//!
//! The engine's code lives in `pill_engine_core`, while code outside the
//! engine names it through `pill_engine`, so the core's crate is written as
//! `pill_engine`: `TransformComponent`'s Rust path
//! `pill_engine_core::ecs::common_components::TransformComponent` is stored as
//! `pill_engine::TransformComponent`. A generic argument follows the same rule
//! (`alloc::Vec<pill_engine::TransformComponent>`).
//!
//! Names used to keep the module path, so files written earlier carry the
//! longer form (`pill_physics::components::RigidBodyComponent`). Every current
//! name has exactly two segments, so a stored name with more is one of those,
//! and [`names_same_type`](crate::type_names::names_same_type) reads it as its
//! first and last segment.
//!
//! A shortened name is built once per distinct type name and kept for the rest
//! of the program's life; a name that is already short is returned as it is.

// Standard library
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

// =============================================================================
// Constants
// =============================================================================

/// The crate name [`std::any::type_name`] gives the engine's own types.
const CORE_CRATE_NAME: &str = "pill_engine_core";

/// The crate name the engine's own types are stored under.
const PUBLIC_CRATE_NAME: &str = "pill_engine";

// =============================================================================
// Free Functions
// =============================================================================

/// [`std::any::type_name`] of `T` in the stored form: every path in it written
/// as its crate and its last segment, and the engine core's crate written as
/// `pill_engine`.
///
/// A name that is already in that form is returned as it is, without an
/// allocation.
pub fn stable_type_name<T: ?Sized>() -> &'static str {
    let type_name = std::any::type_name::<T>();
    if !type_name.contains("::") {
        return type_name;
    }
    interned_short_name(type_name)
}

/// `type_name` with every path in it written as `<crate>::<last segment>`,
/// and `pill_engine_core` written as `pill_engine`.
///
/// `pill_spline::component::Spline` gives `pill_spline::Spline`, and
/// `alloc::vec::Vec<pill_spline::component::Spline>` gives
/// `alloc::Vec<pill_spline::Spline>`. A path of one or two segments only has
/// its crate rewritten when it is the engine core's. Borrows when nothing
/// changes.
pub fn crate_qualified_name(type_name: &str) -> Cow<'_, str> {
    let mut shortened = String::with_capacity(type_name.len());
    let mut changed = false;
    let mut path_start: Option<usize> = None;
    let bytes = type_name.as_bytes();
    let mut index = 0;
    // Walk the name, copying everything that is not a path as it is and
    // replacing each path with its short form. A path is a run of identifier
    // characters and `::` separators.
    while index < bytes.len() {
        let is_path_byte = bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_';
        let is_separator = bytes[index] == b':' && bytes.get(index + 1) == Some(&b':');
        if is_path_byte || (is_separator && path_start.is_some()) {
            path_start.get_or_insert(index);
            index += if is_separator { 2 } else { 1 };
            continue;
        }
        if let Some(start) = path_start.take() {
            changed |= push_short_path(&type_name[start..index], &mut shortened);
        }
        shortened.push(char::from(bytes[index]));
        index += 1;
    }
    if let Some(start) = path_start {
        changed |= push_short_path(&type_name[start..], &mut shortened);
    }
    if changed {
        Cow::Owned(shortened)
    } else {
        Cow::Borrowed(type_name)
    }
}

/// Whether `stored`, a type name read back from a file, names the type whose
/// current name is `current`.
///
/// It does when the two are equal, and when either is a name written before
/// names dropped their module path (`pill_physics::components::RigidBodyComponent`
/// against `pill_physics::RigidBodyComponent`). A caller comparing two current
/// names therefore gets a plain equality, and one comparing a file's name with
/// a type's name may pass them in either order.
pub fn names_same_type(stored: &str, current: &str) -> bool {
    stored == current
        || crate_qualified_name(stored) == current
        || stored == crate_qualified_name(current)
}

/// Whether `stored`, a type name read back from a file, names the type whose
/// current name is `current` and which declares `former` as the names it was
/// stored under before.
///
/// [`names_same_type`] answers it for a name written before names dropped their
/// module path; this adds the names a type declares for itself, so a file
/// written before a rename still reads. `former` is what the type's declaration
/// says (`former = "old::Name"`), never a table the engine keeps.
pub fn names_same_type_or_former(stored: &str, current: &str, former: &[&str]) -> bool {
    names_same_type(stored, current) || former.contains(&stored)
}

/// Appends the short form of one path to `output`. Returns whether the short
/// form differs from `path`.
fn push_short_path(path: &str, output: &mut String) -> bool {
    let mut segments = path.split("::");
    let first = segments.next().unwrap_or(path);
    let mut last = first;
    let mut segment_count = 1usize;
    for segment in segments {
        last = segment;
        segment_count += 1;
    }
    let crate_name = if first == CORE_CRATE_NAME {
        PUBLIC_CRATE_NAME
    } else {
        first
    };
    output.push_str(crate_name);
    if segment_count > 1 {
        output.push_str("::");
        output.push_str(last);
    }
    segment_count > 2 || crate_name != first
}

/// The short form of `type_name`, worked out once per distinct name.
///
/// The map keeps its own copy of every key and every result for the rest of
/// the program's life, so a result can be returned as `&'static str`.
///
/// `type_name` lives in the read-only data of the binary that asked, and a hot
/// reload unmaps a retired project image while this map lives on. A borrowed
/// key would then be compared against unmapped memory by the next lookup that
/// probes its bucket, and a borrowed result would dangle in whoever kept it.
fn interned_short_name(type_name: &'static str) -> &'static str {
    static SHORT_NAMES: OnceLock<Mutex<HashMap<Box<str>, &'static str>>> = OnceLock::new();
    let mut short_names = SHORT_NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(&short_name) = short_names.get(type_name) {
        return short_name;
    }
    let short_name: &'static str = Box::leak(
        crate_qualified_name(type_name)
            .into_owned()
            .into_boxed_str(),
    );
    short_names.insert(type_name.into(), short_name);
    short_name
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's own types are named under the public crate name, with no
    /// module path, including when one appears as a generic argument.
    #[test]
    fn engine_types_use_the_public_crate_name() {
        assert_eq!(
            stable_type_name::<crate::TransformComponent>(),
            "pill_engine::TransformComponent"
        );
        assert_eq!(
            stable_type_name::<Vec<crate::TransformComponent>>(),
            "alloc::Vec<pill_engine::TransformComponent>"
        );
        assert_eq!(
            stable_type_name::<crate::ecs::component::ComponentId>(),
            "pill_engine::ComponentId"
        );
        assert_eq!(
            stable_type_name::<Vec<crate::ecs::entity::Entity>>(),
            "alloc::Vec<pill_engine::Entity>"
        );
    }

    /// An interned name never points into the caller's string, even when the
    /// short form equals it.
    ///
    /// The caller's string is a `type_name` in the read-only data of the binary
    /// that asked, which a hot reload unmaps; the editor crashed on the reload
    /// after an eviction when the interner kept such strings.
    #[test]
    fn an_interned_name_owns_its_text() {
        let caller_name: &'static str = "some_crate::AlreadyShort";
        let interned = interned_short_name(caller_name);

        assert_eq!(interned, caller_name);
        assert!(
            !std::ptr::eq(interned.as_ptr(), caller_name.as_ptr()),
            "the interner handed back the caller's own string"
        );
        assert!(std::ptr::eq(interned, interned_short_name(caller_name)));
    }

    /// A name that is already short is returned as it is, and two calls for
    /// the same type return the same string.
    #[test]
    fn short_names_are_unchanged_and_names_are_reused() {
        assert_eq!(stable_type_name::<u32>(), "u32");
        assert!(std::ptr::eq(
            stable_type_name::<crate::TransformComponent>(),
            stable_type_name::<crate::TransformComponent>()
        ));
        assert!(matches!(
            crate_qualified_name("pill_spline::Spline"),
            Cow::Borrowed("pill_spline::Spline")
        ));
    }

    /// Every path in a name is shortened: a plain path, the paths inside a
    /// generic argument list, and the paths behind a reference.
    #[test]
    fn every_path_in_a_name_keeps_its_crate_and_its_type() {
        assert_eq!(
            crate_qualified_name("pill_spline::component::Spline"),
            "pill_spline::Spline"
        );
        assert_eq!(
            crate_qualified_name(
                "std::collections::hash::map::HashMap<alloc::string::String, pill_physics::components::RigidBodyComponent>"
            ),
            "std::HashMap<alloc::String, pill_physics::RigidBodyComponent>"
        );
        assert_eq!(
            crate_qualified_name("&'static pill_engine_core::scene::Scene"),
            "&'static pill_engine::Scene"
        );
        assert_eq!(crate_qualified_name("[f32; 3]"), "[f32; 3]");
    }

    /// A name read back from a file matches the current name exactly, through
    /// the module path it was written with before, and whichever of the two is
    /// passed first.
    #[test]
    fn a_stored_name_matches_its_type_with_or_without_the_module_path() {
        assert!(names_same_type(
            "pill_spline::Spline",
            "pill_spline::Spline"
        ));
        assert!(names_same_type(
            "pill_spline::component::Spline",
            "pill_spline::Spline"
        ));
        assert!(names_same_type(
            "pill_spline::Spline",
            "pill_spline::component::Spline"
        ));
        assert!(names_same_type(
            "pill_physics::components::RigidBodyComponent",
            "pill_physics::RigidBodyComponent"
        ));
        assert!(!names_same_type(
            "pill_spline::components::Spline",
            "pill_spline::Track"
        ));
        assert!(!names_same_type(
            "pill_spline::Spline",
            "pill_physics::Spline"
        ));
    }

    /// A type's own declaration of the names it was stored under before is
    /// what makes a file written then read as the type under its new name.
    #[test]
    fn a_former_name_is_read_as_the_type_that_declared_it() {
        let former = ["pill_renderer_legacy::Mesh"];
        assert!(names_same_type_or_former(
            "pill_renderer_legacy::Mesh",
            "pill_rendering::Mesh",
            &former
        ));
        assert!(names_same_type_or_former(
            "pill_rendering::Mesh",
            "pill_rendering::Mesh",
            &former
        ));
        assert!(!names_same_type_or_former(
            "pill_renderer_legacy::Texture",
            "pill_rendering::Mesh",
            &former
        ));
        assert!(!names_same_type_or_former(
            "pill_renderer_legacy::Mesh",
            "pill_rendering::Mesh",
            &[]
        ));
    }
}
