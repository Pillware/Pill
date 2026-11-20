//! Expansion of nested component field layouts at registration time.
//!
//! # Responsibilities
//!
//! - Flatten a `struct:` or `array:struct:` field whose type has a known
//!   layout into dotted leaf rows - one per leaf, and one per array element -
//!   composing offsets as it descends.
//! - Keep the declared layout beside the flattened one, and own the resolver
//!   that tells a later registration what a nested type looks like.
//! - Retag a `struct:` field whose type is a declared enum as an `enum:`
//!   field, carrying its variants, so it reads and writes as an enum.
//!
//! # Design
//!
//! Expanded field names are interned: expansion keeps one shared copy of each
//! distinct dotted name for the whole process and never frees it. A layout
//! stores its names as `&'static str`, so a later registration of the same
//! type reuses that copy instead of making a new one. A layout with nothing to
//! expand borrows the caller's `'static` slice instead, so only a real
//! expansion allocates.

// Standard library
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

// Current crate
use crate::component::ComponentId;
use crate::component_registry::{ComponentFieldDescriptor, FieldDecorator};
use crate::world::World;

/// How many elements of an `array:struct:` field become leaf rows.
///
/// A control-point array of a few dozen entries becomes a few dozen rows,
/// which is what an editor wants to show. A fixed array of thousands would
/// only make every layout huge, so the editor view stops at this cap. The
/// declared layout still describes the whole array.
pub const MAX_EXPANDED_ELEMENTS: usize = 64;

impl World {
    /// Records a component's field layout, expanding nested struct fields into
    /// dotted leaf rows for the editor.
    ///
    /// A `struct:<path>` field whose type has a known layout - another
    /// component registered with one, or a `#[derive(PillMirror)]` value type -
    /// becomes one row per leaf, named `<field>.<leaf>`, and the row's offset
    /// is composed from the struct's own offset. Only layouts registered
    /// before this call are visible, so a nested component type must be
    /// registered first.
    ///
    /// The declared slice is kept beside the flattened view, because a
    /// generated mirror describes declarations, not leaf rows. When nothing
    /// expands, both views borrow the caller's `'static` slice; only a real
    /// expansion builds an owned editor list.
    pub(crate) fn record_component_field_layout(
        &mut self,
        component_id: ComponentId,
        fields: &'static [ComponentFieldDescriptor],
    ) {
        let layout = match self.expanded_field_layout(fields) {
            Some(editor_fields) => {
                crate::world::ComponentFieldLayout::from_expanded(fields, editor_fields)
            }
            None => crate::world::ComponentFieldLayout::from_static(fields),
        };
        self.component_field_layouts.insert(component_id, layout);
    }

    /// Expands `struct:` fields into dotted leaf rows where the type's layout
    /// is known. Returns `None` when no field needed expanding.
    fn expanded_field_layout(
        &self,
        fields: &[ComponentFieldDescriptor],
    ) -> Option<Vec<ComponentFieldDescriptor>> {
        let mut expanded_fields = Vec::new();
        let mut changed = false;
        self.expand_fields_into(fields, "", 0, &[], &mut expanded_fields, &mut changed);
        changed.then_some(expanded_fields)
    }

    /// Flattens `fields` into `expanded_fields`, recursing into every
    /// `struct:` field whose type has a known layout.
    ///
    /// `prefix` is the dotted path of the struct being flattened (empty at the
    /// top level), `base_offset` is where that struct starts in the component
    /// row, and `inherited_decorators` holds the grouping decorators (`Color`,
    /// `Vector`) declared by the struct fields above. The editor's row model
    /// groups leaves by those decorators, so they must reach the rows the
    /// leaves produce. The recursion ends because a struct cannot contain
    /// itself without a heap indirection, and heap fields are not tagged
    /// `struct:`.
    fn expand_fields_into(
        &self,
        fields: &[ComponentFieldDescriptor],
        prefix: &str,
        base_offset: usize,
        inherited_decorators: &[FieldDecorator],
        expanded_fields: &mut Vec<ComponentFieldDescriptor>,
        changed: &mut bool,
    ) {
        for field in fields {
            // An array of structs (`array:struct:<path>`) becomes one dotted
            // row per leaf per element, named like `points[0].x`, so the
            // editor's field API reaches one control point the same way it
            // reaches a plain float. The array's own element count decides how
            // many rows appear; `MAX_EXPANDED_ELEMENTS` caps a huge array so it
            // cannot flood the layout.
            if let Some(path) = field.type_tag.strip_prefix("array:struct:") {
                if let Some(nested) = self.resolve_nested_layout(path) {
                    let expanded_element_count = field.element_count.min(MAX_EXPANDED_ELEMENTS);
                    // A zero-length array has no element stride to compute.
                    // Any other element count divides the field's total size
                    // evenly.
                    let element_stride_bytes =
                        field.size.checked_div(field.element_count).unwrap_or(0);
                    if expanded_element_count > 0 {
                        *changed = true;
                        for index in 0..expanded_element_count {
                            let element_prefix = if prefix.is_empty() {
                                format!("{}[{}]", field.name, index)
                            } else {
                                format!("{}.{}[{}]", prefix, field.name, index)
                            };
                            self.expand_fields_into(
                                &nested,
                                &element_prefix,
                                base_offset + field.offset + index * element_stride_bytes,
                                &grouping_decorators(&field.decorators),
                                expanded_fields,
                                changed,
                            );
                        }
                        continue;
                    }
                }
            }
            let nested = field
                .type_tag
                .strip_prefix("struct:")
                .and_then(|path| self.resolve_nested_layout(path));
            if let Some(nested) = nested {
                *changed = true;
                let name = if prefix.is_empty() {
                    field.name.to_string()
                } else {
                    format!("{prefix}.{}", field.name)
                };
                let group = grouping_decorators(&field.decorators);
                self.expand_fields_into(
                    &nested,
                    &name,
                    base_offset + field.offset,
                    &group,
                    expanded_fields,
                    changed,
                );
            } else {
                let name = if prefix.is_empty() {
                    field.name
                } else {
                    intern_nested_field_name(format!("{prefix}.{}", field.name))
                };
                let decorators = if inherited_decorators.is_empty() {
                    field.decorators.clone()
                } else {
                    leaf_decorators(inherited_decorators, field.decorators.as_ref())
                };
                // A field of a declared enum reads and writes as an enum
                // instead of an opaque blob.
                let type_tag = match self.resolve_enum_tag(field) {
                    Some(enum_tag) => {
                        *changed = true;
                        enum_tag
                    }
                    None => field.type_tag,
                };
                expanded_fields.push(ComponentFieldDescriptor {
                    name,
                    type_tag,
                    offset: base_offset + field.offset,
                    size: field.size,
                    align: field.align,
                    element_count: field.element_count,
                    decorators,
                });
            }
        }
    }

    /// The field layout a `struct:<path>` tag names, when one is known.
    ///
    /// The tag is the path as written on the field, usually an imported short
    /// name (`Color`). A name matching the tag exactly wins; otherwise a name
    /// ending in `::<path>` is taken if it is the only one. The candidates are
    /// the world's registered component layouts and the registering DLL's
    /// `#[derive(PillMirror)]` value types, the latter through the installed
    /// [`ValueTypeLayoutResolver`](crate::component_registry::ValueTypeLayoutResolver).
    /// An ambiguous suffix resolves to `None`, which leaves the field opaque
    /// and readable; a wrong match would show the wrong rows.
    fn resolve_nested_layout(&self, path: &str) -> Option<Vec<ComponentFieldDescriptor>> {
        // Registered component layouts, matched by the name rule the value
        // types use too.
        let component_names = self
            .component_field_layouts
            .keys()
            .filter_map(|component_id| self.component_registry.get_name(component_id));
        if let Some(name) = crate::component_registry::resolve_unique_name(path, component_names) {
            for (component_id, layout) in &self.component_field_layouts {
                if self.component_registry.get_name(component_id) == Some(name) {
                    return Some(layout.fields().to_vec());
                }
            }
        }

        // The registering DLL's value types, through the resolver it installed.
        // That resolver applies the same name rule.
        let resolver = self.value_type_layout_resolver?;
        resolver(path).map(<[ComponentFieldDescriptor]>::to_vec)
    }

    /// The `enum:` tag a `struct:<path>` field gets when its type is an enum
    /// the registering DLL declares, or `None` to leave the field as it is.
    ///
    /// The declaration is checked against the field itself: a representation
    /// whose size is not the field's size, or a discriminant that does not fit
    /// the representation, leaves the field opaque, because reading it as an
    /// enum would read the wrong bytes. Handles are never enums.
    fn resolve_enum_tag(&self, field: &ComponentFieldDescriptor) -> Option<&'static str> {
        let path = field.type_tag.strip_prefix("struct:")?;
        if super::is_handle_tag(field.type_tag) {
            return None;
        }
        let resolver = self.enum_layout_resolver?;
        let layout = resolver(path)?;
        if super::codec::scalar_size(layout.representation) != Some(field.size) {
            return None;
        }
        let discriminants_fit = layout.variants.iter().all(|(_, discriminant)| {
            super::codec::enum_discriminant_value(layout.representation, *discriminant).is_some()
        });
        if !discriminants_fit {
            return None;
        }
        Some(intern_nested_field_name(super::format_enum_tag(
            path, &layout,
        )))
    }

    /// Replaces the resolver used to find enum declarations while component
    /// layouts are recorded. Returns the resolver installed before, so the
    /// caller can put it back.
    ///
    /// Installed and removed by the facade together with the value-type
    /// resolver. It is not meant for other callers.
    #[doc(hidden)]
    pub fn replace_enum_layout_resolver(
        &mut self,
        resolver: Option<crate::component_registry::EnumLayoutResolver>,
    ) -> Option<crate::component_registry::EnumLayoutResolver> {
        std::mem::replace(&mut self.enum_layout_resolver, resolver)
    }

    /// Replaces the resolver used to find value-type layouts while component
    /// layouts are recorded. Returns the resolver installed before, so the
    /// caller can put it back.
    ///
    /// The facade's `register_all_components` calls this around its registration
    /// loop, with a resolver compiled into the registering DLL. It is not meant
    /// for other callers.
    #[doc(hidden)]
    pub fn replace_value_type_layout_resolver(
        &mut self,
        resolver: Option<crate::component_registry::ValueTypeLayoutResolver>,
    ) -> Option<crate::component_registry::ValueTypeLayoutResolver> {
        std::mem::replace(&mut self.value_type_layout_resolver, resolver)
    }
}

/// The decorators of a struct field that its flattened leaves inherit.
///
/// `Color` and `Vector` describe the group of leaves a struct expands into, so
/// they must stay on the leaves. Every other decorator describes the single
/// value its field holds (a header above it, a tooltip on it) and is dropped
/// with the struct field that carried it.
fn grouping_decorators(decorators: &[FieldDecorator]) -> Vec<FieldDecorator> {
    decorators
        .iter()
        .filter(|decorator| matches!(decorator, FieldDecorator::Color | FieldDecorator::Vector))
        .cloned()
        .collect()
}

/// Builds the decorator list for one flattened leaf: the inherited decorators
/// first, then the leaf's own, with no decorator listed twice.
fn leaf_decorators(
    inherited_decorators: &[FieldDecorator],
    own_decorators: &[FieldDecorator],
) -> std::borrow::Cow<'static, [FieldDecorator]> {
    let mut merged = inherited_decorators.to_vec();
    for decorator in own_decorators {
        if !merged.contains(decorator) {
            merged.push(decorator.clone());
        }
    }
    std::borrow::Cow::Owned(merged)
}

/// Returns one shared `&'static str` for a synthesized nested field name such
/// as "color.r" or "inner.a.b".
///
/// A descriptor stores its name as `&'static str`, and expansion builds dotted
/// names at registration. Each distinct name is therefore copied once into
/// memory that is never freed (leaked on purpose) and reused for the rest of
/// the process. The set only grows with the distinct field paths a process
/// registers, so a hot reload of the same components adds nothing to it.
fn intern_nested_field_name(name: String) -> &'static str {
    static NESTED_FIELD_NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let names = NESTED_FIELD_NAMES.get_or_init(|| Mutex::new(HashSet::new()));
    let mut names = names
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&existing) = names.get(name.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.into_boxed_str());
    names.insert(leaked);
    leaked
}
