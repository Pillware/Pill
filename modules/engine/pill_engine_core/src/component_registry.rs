//! Compile-time field layouts of components and value types.
//!
//! # Responsibilities
//!
//! - Declare [`ComponentFieldDescriptor`], the description of one component or
//!   value-type field. The derive macros emit it, the world keeps it for the
//!   editor, and the host reads it to generate the C# mirror.
//! - Declare [`FieldDecorator`], the presentation metadata a field declares
//!   for the editor's Inspector.
//! - Declare the [`ComponentLayout`] trait the derives implement, and
//!   [`ValueTypeLayoutResolver`], the function the world calls to find the
//!   layout of a value type that a component field nests.
//! - Declare [`EnumLayout`] and [`EnumLayoutResolver`], which let a field of a
//!   declared enum type become an editable enum field.
//!
//! # Design
//!
//! The registries the derives submit into (`PillComponentDescriptor` and the
//! other descriptor types) live in `pill_engine`, the facade crate every
//! project and extension depends on, not here. The facade is compiled into
//! every DLL, so each DLL's registry holds exactly what that DLL declared.
//! This crate holds only the definitions that are the same for all of them.

// Standard library
use std::borrow::Cow;

// =============================================================================
// FieldDecorator
// =============================================================================

/// Presentation metadata a field declares for the editor's Inspector.
///
/// The vocabulary is Unity's. A decorator changes only what the Inspector
/// draws and how it draws it. It never reaches storage, persistence, the
/// schema fingerprint or the C# mirror. Text payloads are [`Cow`] so a
/// deriving artifact can borrow its literals, while the C# manifest lane owns
/// the text it parsed.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldDecorator {
    /// Unity's `[HideInInspector]`: the field is not drawn. It is still stored,
    /// serialized and copied; only its row disappears.
    HideInInspector,
    /// Unity's `[Header("Text")]`: a separator line with the text is drawn
    /// above the field.
    Header(Cow<'static, str>),
    /// Unity's `[Space(pixels)]`: a vertical gap is drawn above the field.
    Space(f32),
    /// Unity's `[Tooltip("Text")]`: the text is shown on hover of the field's
    /// label.
    Tooltip(Cow<'static, str>),
    /// Unity's `[Range(min, max)]`: a numeric field is drawn as a slider over
    /// the range. Integer fields slide in whole steps.
    Range {
        /// Lowest value the slider offers.
        min: f32,
        /// Highest value the slider offers.
        max: f32,
    },
    /// Unity's `[TextArea]` / `[TextArea(minLines, maxLines)]`: a text field is
    /// drawn as a multi-line box.
    TextArea {
        /// Row count the Inspector asks the box to show.
        min_lines: usize,
        /// Declared upper row count; the Inspector sizes the box from
        /// `min_lines` alone.
        max_lines: usize,
    },
    /// Unity's `[Multiline(lines)]`: a text field is drawn as a multi-line box
    /// that asks for that many rows.
    Multiline(usize),
    /// Unity's `[Min(value)]`: a numeric field's drag value never goes below
    /// the minimum.
    Min(f32),
    /// The field draws as one colour row with a popup picker. That covers an
    /// array of three or four numbers and the leaves of a nested colour struct
    /// (`fill.r`, `fill.g`, ...), which the world flattens at registration.
    Color,
    /// Consecutive scalar leaves sharing a dotted prefix draw as one vector
    /// row, X/Y/Z/W side by side, instead of one row each.
    Vector,
}

// =============================================================================
// ComponentFieldDescriptor
// =============================================================================

/// One named field of a component or a value type: its name, its type tag and
/// where it sits in the row.
///
/// The derive macros build these at compile time and the C# back end builds
/// them at runtime from a managed declaration. The host reads them to emit a
/// typed C# mirror instead of an opaque ABI blob, and the world keeps them so
/// the editor can show and edit the field.
///
/// Every value can be written in a `const` (`offset_of!`, `size_of!`,
/// `align_of!`), so a descriptor list can live in a `static` inside the
/// declaring artifact.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentFieldDescriptor {
    /// Rust field name (snake_case); the C# codegen maps it to PascalCase.
    pub name: &'static str,
    /// The field's type tag, from a closed set: `f32`, `u32`, `bool`, ...,
    /// `array:<inner>`, `struct:<path>`, `vec:<element>`, `dynbuf:<element>`,
    /// `string`. The C# codegen maps each tag to a C# type; see
    /// `modules/hosting/pill_development_host/src/csharp/codegen.rs`.
    ///
    /// A `vec:<element>` or `string` field is a heap value that Rust owns, so
    /// it has no C# field of its own. Managed code reaches it through the
    /// accessor members the codegen emits, whose trampolines are declared with
    /// `pill_engine::component_registry::PillFieldAccessorDescriptor`.
    ///
    /// A `dynbuf:<element>` field is a native buffer the engine owns. Its
    /// `(ptr, len, cap)` handle is mirrorable as plain words, so managed code
    /// reads the buffer in place and only resizing goes through an accessor.
    pub type_tag: &'static str,
    /// Byte offset of the field within the type (`core::mem::offset_of!`).
    pub offset: usize,
    /// Byte size of the field's type.
    pub size: usize,
    /// Byte alignment of the field's type.
    pub align: usize,
    /// Number of elements for an `array:` field; zero for a field that is not
    /// an array. The derive computes it from the length expression, so a
    /// literal length and a `const` both work.
    pub element_count: usize,
    /// The presentation decorators the field declared, in declaration order.
    /// Empty when it declares none.
    ///
    /// A Rust type's own list is borrowed from the declaring artifact's static
    /// data. A list the world builds is owned instead: the leaves of a
    /// flattened struct that inherit a colour or vector grouping, and a layout
    /// a managed declaration describes at runtime.
    pub decorators: Cow<'static, [FieldDecorator]>,
}

// =============================================================================
// ComponentLayout
// =============================================================================

/// The compile-time field layout of a type, as the derive macros emit it.
///
/// The `#[derive(PillLayout)]`, `#[derive(PillComponent)]` and
/// `#[derive(PillMirror)]` macros all implement it. It lets a layout reach
/// [`World::register_component_with_layout`] without restating the
/// descriptors. The same list is also the type's inherent `FIELD_LAYOUT`
/// const.
///
/// [`World::register_component_with_layout`]: crate::World::register_component_with_layout
pub trait ComponentLayout {
    /// The declared field list, in declaration order.
    const FIELDS: &'static [ComponentFieldDescriptor];
}

// =============================================================================
// ValueTypeLayoutResolver
// =============================================================================

/// A function that finds the field layout of a `#[derive(PillMirror)]` value
/// type. It takes the path a `struct:<path>` field tag names and tries an
/// exact type name first, then a unique suffix match.
///
/// The value types live in a per-DLL registry, which this crate cannot read.
/// The facade's `register_all_components` lends its own DLL's resolver to the
/// world for the length of its registration loop
/// ([`World::replace_value_type_layout_resolver`]), so a component's nested
/// value-type fields expand against the value types its own DLL declares.
///
/// [`World::replace_value_type_layout_resolver`]: crate::World::replace_value_type_layout_resolver
pub type ValueTypeLayoutResolver = fn(&str) -> Option<&'static [ComponentFieldDescriptor]>;

// =============================================================================
// EnumLayout
// =============================================================================

/// The shape of a fieldless enum with an integer representation, as a
/// `#[derive(PillMirror)]` declares it.
///
/// The strings borrow the declaring DLL's static data, so a value is only read
/// while that DLL registers its components: the world copies what it needs into
/// the field's own tag before the registration ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumLayout {
    /// The integer type the enum is stored as (`u8`, `i32`, ...).
    pub representation: &'static str,
    /// Every variant's name and discriminant, in declaration order.
    pub variants: Vec<(&'static str, i64)>,
}

/// A function that finds the declared shape of an enum a `struct:<path>` field
/// tag names, with the same name rule as [`ValueTypeLayoutResolver`].
///
/// Like the value-type resolver it lives in the registering DLL and is lent to
/// the world only for the length of that DLL's registration loop
/// ([`World::replace_enum_layout_resolver`]).
///
/// [`World::replace_enum_layout_resolver`]: crate::World::replace_enum_layout_resolver
pub type EnumLayoutResolver = fn(&str) -> Option<EnumLayout>;

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the candidate name a `struct:<path>` field tag refers to.
///
/// The tag is the path as written at the field site, usually an imported short
/// name (`Color`). An exact match is tried first, then a unique `::<path>`
/// suffix. `None` means no single candidate matches and the field shows as
/// opaque. That is better than picking a wrong type, which would show the field
/// as another type's value.
///
/// Both candidate sources - the world's registered component names and the
/// facade's value-type names - answer through this one rule, so the two can
/// never disagree about what a tag names.
pub fn resolve_unique_name<'a>(
    path: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<&'a str> {
    let suffix = format!("::{path}");
    let mut suffix_match = None;
    let mut suffix_match_count = 0usize;
    for name in candidates {
        if name == path {
            return Some(name);
        }
        if name.ends_with(&suffix) {
            suffix_match_count += 1;
            suffix_match = Some(name);
        }
    }
    if suffix_match_count == 1 {
        suffix_match
    } else {
        None
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Component, ComponentId, World};

    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct TestPersistableComponent {
        value: u32,
    }
    impl Component for TestPersistableComponent {}

    #[derive(Clone, Debug)]
    struct TestPlainComponent;
    impl Component for TestPlainComponent {}

    /// Checks the rule both candidate sources use. An exact match wins, a
    /// unique suffix is accepted, and an ambiguous suffix resolves to nothing.
    ///
    /// Resolving an ambiguous tag to one of its candidates would show the
    /// reader another type's fields to edit, so the test insists on `None`.
    #[test]
    fn a_path_resolves_by_exact_name_then_a_unique_suffix() {
        let candidate_names = [
            "project::Color",
            "project::paint::Color",
            "project::Mesh",
            "project::Grid",
        ];
        let resolve = |path: &str| resolve_unique_name(path, candidate_names.iter().copied());

        assert_eq!(resolve("project::Mesh"), Some("project::Mesh"));
        assert_eq!(resolve("Grid"), Some("project::Grid"), "a unique suffix");
        assert_eq!(
            resolve("Color"),
            None,
            "two components end with `::Color`, so neither is the answer"
        );
        assert_eq!(resolve("Texture"), None);
    }

    /// A compile-time field layout registered with a component is retrievable,
    /// while a component registered without one reports no layout.
    #[test]
    fn field_layouts_are_stored_and_queryable() {
        static FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
            name: "value",
            type_tag: "u32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }];

        let mut with_layout = World::new();
        with_layout.register_component_with_layout::<TestPlainComponent>(FIELDS);
        assert_eq!(
            with_layout.component_field_layout(ComponentId::of::<TestPlainComponent>()),
            Some(FIELDS)
        );

        let mut without_layout = World::new();
        without_layout.register_component::<TestPlainComponent>();
        assert!(without_layout
            .component_field_layout(ComponentId::of::<TestPlainComponent>())
            .is_none());
    }

    /// The persistable with-layout variant stores the layout after the
    /// standard persistable registration.
    #[test]
    fn persistable_field_layout_is_stored() {
        static FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
            name: "value",
            type_tag: "u32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }];

        let mut world = World::new();
        world.register_persistable_component_with_layout::<TestPersistableComponent>(FIELDS);
        assert_eq!(
            world.component_field_layout(ComponentId::of::<TestPersistableComponent>()),
            Some(FIELDS)
        );
    }

    /// Registering the same type first as plain, then as persistable, stays
    /// idempotent: one registry entry, one bit.
    #[test]
    fn plain_then_persistable_registration_stays_idempotent() {
        let mut world = World::new();
        world.register_component::<TestPersistableComponent>();
        let bit_before = world
            .component_registry
            .get_bit(&ComponentId::of::<TestPersistableComponent>());
        assert!(bit_before.is_some(), "plain registration must assign a bit");

        world.register_persistable_component::<TestPersistableComponent>();
        let bit_after = world
            .component_registry
            .get_bit(&ComponentId::of::<TestPersistableComponent>());
        assert_eq!(
            bit_before, bit_after,
            "persistable re-registration must reuse the existing bit"
        );

        let entry_count = world
            .component_registry
            .registered_components()
            .filter(|(id, _, _)| *id == ComponentId::of::<TestPersistableComponent>())
            .count();
        assert_eq!(
            entry_count, 1,
            "one logical type must occupy exactly one registry entry"
        );
    }
}
