//! A component's fields, named in code.
//!
//! # Responsibilities
//!
//! - Declare [`ComponentFieldId`]: what a component's generated `<Type>Field`
//!   enum answers about its fields.
//! - Declare [`ComponentFields`]: the link from a component type to the field
//!   enum `#[derive(PillComponent)]` generates for it.
//!
//! # Design
//!
//! The field layer identifies a field by its descriptor's name (see
//! [`component_registry`](super::component_registry)): the editor's commands,
//! scene files and the C# mirror all carry strings. The generated enum is the
//! code-side view of the same list, so a typed editor callback can name a
//! field instead of comparing strings, while [`ComponentFieldId::from_name`]
//! bridges back from a queued command. The enum is generated in the declaring
//! artifact, so no table is kept in sync by hand.

// Current crate
use crate::Component;

/// One field of a component, named in code.
///
/// `#[derive(PillComponent)]` implements this for the `<Type>Field` enum it
/// generates beside the component type.
pub trait ComponentFieldId: Copy + core::fmt::Debug + Eq + core::hash::Hash + 'static {
    /// Every field, in the order the type declares them.
    const ALL: &'static [Self];

    /// The field's Rust name, as the field layer spells it.
    fn name(self) -> &'static str;

    /// The field `name` names, or `None` when the type has no such field.
    fn from_name(name: &str) -> Option<Self>;

    /// The field's registered type tag, as its descriptor reports it.
    fn value_tag(self) -> &'static str;
}

/// A component type together with the field enum its derive generated.
pub trait ComponentFields: Component {
    /// The component's generated `<Type>Field` enum.
    type Field: ComponentFieldId;
}
