//! The Inspector's editing helpers: the rows a component section draws, the
//! edits those rows queue, and the asset settings documents as editable rows.
//!
//! # Responsibilities
//!
//! - The section the tab draws: its label ([`component_label`]), its icon
//!   ([`component_icon`]) and its rows ([`rows`]) - which widget each field
//!   asks for and the rows a field's decorators declare.
//! - The grouped-row model: [`RowLeaf`], [`grouped_row`] and [`channel_run`]
//!   fold a colour's channels or a struct's flattened axes into one row.
//! - The cached row model ([`CachedRows`], [`take_component_rows`]), so a
//!   section redraws without rebuilding its rows.
//! - The edit builders and their history labels: `SetField`,
//!   `SetArrayElement`, [`EditorCommand::SetFieldAsset`],
//!   [`EditorCommand::SetFieldEntity`] and the resource field write, so the
//!   shell's widgets and the scripted channel queue identical edits.
//! - Text into a value and back: [`parse_field_value`], [`value_text`] and
//!   one element of an array value ([`element_field`]).
//! - Reference rows: the asset builders ([`asset_edit`],
//!   [`selection_asset_edit`]) with the assets a picker offers
//!   ([`picker_candidates`]), and the entity labels a reference takes
//!   ([`parse_entity_reference`]).
//! - Asset settings as editable data: key paths (`a.b[0]`), leaf reads and
//!   patches, and the [`EditorCommand::SetAssetSettings`] builder.
//!
//! # Design
//!
//! Everything here is plain data and commands: the shell draws widgets and
//! queues batches, the executable applies them at the frame boundary, and
//! the history records them - the same split the Assets pipeline follows.

// Standard library
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// External crates
use egui::Color32;
use pill_engine::component_registry::FieldDecorator;
use pill_engine::{Entity, FieldValue};
use serde_json::Value as JsonValue;

// Current crate
use crate::editor_state::{
    AssetListing, ComponentView, EditorCommand, EngineAsset, EntityDetail, EntityListEntry,
};
use crate::icons::Icon;
use crate::session::InspectorSession;

// =============================================================================
// Constants
// =============================================================================

/// How many side-by-side elements a vector row holds at most: X, Y, Z, W.
pub const VECTOR_AXES_MAX: usize = 4;

/// The channel suffixes a colour row reads, in order.
pub const COLOR_CHANNELS: [&str; 4] = ["r", "g", "b", "a"];

/// The axis suffixes a grouped vector row reads, in order.
pub const VECTOR_AXES: [&str; 4] = ["x", "y", "z", "w"];

/// How many channels a colour row needs at least: red, green and blue.
pub const COLOR_CHANNELS_MIN: usize = 3;

/// The shortest grouped vector row: two axes.
pub const VECTOR_AXES_MIN: usize = 2;

// =============================================================================
// Labels and icons
// =============================================================================

/// The name after the last `::`, for labels and history entries.
pub fn short_component_name(type_name: &str) -> &str {
    type_name.rsplit("::").next().unwrap_or(type_name)
}

/// The title a component section's header shows.
///
/// A component is named the way Unity names it: without its type suffix, so
/// the engine's `TransformComponent` reads `Transform`. A type without that
/// suffix keeps its own short name.
pub fn component_label(type_name: &str) -> &str {
    let short = short_component_name(type_name);
    short.strip_suffix("Component").unwrap_or(short)
}

/// The duotone icon a component section's header draws, from its type name.
///
/// The engine's components carry no icon metadata, so the icon is chosen from
/// the words in the type's name. A transform gets the arrows that point in all
/// directions, a collider an outlined cube, a mesh a cube, a sprite or texture
/// an image, a material a sphere, a light a power switch, and a camera a
/// monitor. Audio gets a play mark, a script, behaviour or system a code mark,
/// and physics names outward arrows. Every other name gets the property
/// sliders.
pub fn component_icon(type_name: &str) -> Icon {
    let lower = type_name.to_lowercase();
    if lower.contains("transform") {
        Icon::ArrowsOutCardinal
    } else if lower.contains("collider") || lower.contains("collision") {
        Icon::CubeTransparent
    } else if lower.contains("mesh") || lower.contains("model") {
        Icon::Cube
    } else if lower.contains("sprite") || lower.contains("texture") || lower.contains("image") {
        Icon::Image
    } else if lower.contains("material") {
        Icon::Sphere
    } else if lower.contains("light") {
        Icon::Power
    } else if lower.contains("camera") {
        Icon::MonitorPlay
    } else if lower.contains("audio") || lower.contains("sound") {
        Icon::Play
    } else if lower.contains("script")
        || lower.contains("behaviour")
        || lower.contains("behavior")
        || lower.contains("system")
    {
        Icon::Code
    } else if lower.contains("rigid") || lower.contains("physics") || lower.contains("velocity") {
        Icon::ArrowsOut
    } else {
        Icon::SlidersHorizontal
    }
}

/// The duotone icon a scene badge draws for an entity, or `None` when the
/// entity gets no mark.
///
/// A label that names a Camera component gets the camera icon, and one that
/// names a Light component gets the light icon. Any other label returns
/// `None`, which the badge draws as a plain circle. The label is the component
/// list the Hierarchy row already carries, so a new component type gains a
/// badge by naming it here and nowhere else.
pub fn badge_icon(archetype_label: &str) -> Option<Icon> {
    if archetype_label.contains("Camera") {
        Some(Icon::MonitorPlay)
    } else if archetype_label.contains("Light") {
        Some(Icon::Power)
    } else {
        None
    }
}

// =============================================================================
// Field widgets
// =============================================================================

/// The widget a value's kind asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldWidget {
    /// A boolean: a checkbox.
    Checkbox,
    /// A whole number: a drag field that scrubs and accepts typed input.
    Integer,
    /// A floating point number: a drag field with decimals.
    Float,
    /// A string: a single-line text edit.
    Text,
    /// A value the row shows as text with no editor: a heap-backed string, a
    /// long array, a list or an opaque payload.
    ReadOnly,
    /// A `[Range]` on a floating point field: a slider bounded by `min` and
    /// `max`.
    FloatSlider {
        /// Lowest value the slider offers.
        min: f32,
        /// Highest value the slider offers.
        max: f32,
    },
    /// A `[Range]` on a whole-number field: a slider in whole steps.
    IntegerSlider {
        /// Lowest value the slider offers.
        min: f32,
        /// Highest value the slider offers.
        max: f32,
    },
    /// A `[Min]` on a floating point field: a drag value with a floor.
    FloatDrag {
        /// Lowest value the drag value accepts.
        min: f32,
    },
    /// A `[Min]` on a whole-number field: a drag value with a floor.
    IntegerDrag {
        /// Lowest value the drag value accepts.
        min: f32,
    },
    /// A `[TextArea]` or `[Multiline]` on a text field: a box of that many
    /// rows.
    MultilineText {
        /// Rows the box shows.
        lines: usize,
    },
    /// A declared enum: a dropdown of its variants, which the value's own tag
    /// lists ([`pill_engine::parse_enum_tag`]).
    Enum,
}

/// The widget a field's current value asks for.
pub fn widget_for(value: &FieldValue) -> FieldWidget {
    match value {
        FieldValue::Bool(_) => FieldWidget::Checkbox,
        FieldValue::F32(_) | FieldValue::F64(_) => FieldWidget::Float,
        FieldValue::I8(_)
        | FieldValue::I16(_)
        | FieldValue::I32(_)
        | FieldValue::I64(_)
        | FieldValue::U8(_)
        | FieldValue::U16(_)
        | FieldValue::U32(_)
        | FieldValue::U64(_)
        | FieldValue::Usize(_)
        | FieldValue::Isize(_) => FieldWidget::Integer,
        FieldValue::Enum { .. } => FieldWidget::Enum,
        FieldValue::Array(_)
        | FieldValue::List { .. }
        | FieldValue::Asset { .. }
        | FieldValue::Entity { .. }
        | FieldValue::Opaque { .. } => FieldWidget::ReadOnly,
        // A string whose bytes live behind the value itself is read-only
        // through the field layer: the writer reaches its buffer through the
        // type's own accessors, so a text box here would queue an edit it
        // refuses on every keystroke.
        FieldValue::Text(_) => FieldWidget::ReadOnly,
    }
}

/// The widget a field's value asks for once its decorators are applied.
///
/// A decorator that cannot apply to the value is ignored, not refused: a
/// `[Range]` on a string or a `[TextArea]` on a number still draws its field.
/// The declaration only says how a field should look, and a wrong one must not
/// hide the field it decorates. A range wins over a minimum, because a slider
/// already bounds the value below. `[TextArea]` takes its `min_lines` as the
/// box height, and `[Multiline]` its count.
pub fn widget_for_decorated(value: &FieldValue, decorators: &[FieldDecorator]) -> FieldWidget {
    let mut widget = widget_for(value);
    let mut range: Option<(f32, f32)> = None;
    let mut minimum_value: Option<f32> = None;
    let mut line_count: Option<usize> = None;
    for decorator in decorators {
        match decorator {
            FieldDecorator::Range { min, max } => range = Some((*min, *max)),
            FieldDecorator::Min(value) => minimum_value = Some(*value),
            FieldDecorator::TextArea { min_lines, .. } => line_count = Some(*min_lines),
            FieldDecorator::Multiline(count) => line_count = Some(*count),
            FieldDecorator::HideInInspector
            | FieldDecorator::Header(_)
            | FieldDecorator::Space(_)
            | FieldDecorator::Tooltip(_)
            | FieldDecorator::Color
            | FieldDecorator::Vector => {}
        }
    }
    if let Some((min, max)) = range {
        widget = match widget {
            FieldWidget::Float => FieldWidget::FloatSlider { min, max },
            FieldWidget::Integer => FieldWidget::IntegerSlider { min, max },
            other => other,
        };
    } else if let Some(min) = minimum_value {
        widget = match widget {
            FieldWidget::Float => FieldWidget::FloatDrag { min },
            FieldWidget::Integer => FieldWidget::IntegerDrag { min },
            other => other,
        };
    }
    if let (FieldWidget::Text, Some(line_count)) = (widget, line_count) {
        widget = FieldWidget::MultilineText { lines: line_count };
    }
    widget
}

// =============================================================================
// Rows
// =============================================================================

/// One writable leaf of a grouped row.
///
/// A leaf is one field, or one element inside a field's array value. A colour
/// held as `[f32; 4]` writes `SetArrayElement` four times; one held as four
/// scalar fields writes `SetField` four times. Either way, committing the row
/// records one step.
#[derive(Clone, Debug, PartialEq)]
pub struct RowLeaf {
    /// The field's position in the component's field list (the value's, too).
    pub index: usize,
    /// The field's registered name, which the edit names.
    pub field: String,
    /// The element inside the field's array value, when the field is one.
    pub element: Option<usize>,
}

/// One row of a component section.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldRow {
    /// A separator line with the text a field's `[Header]` declared.
    Header {
        /// Text the line shows.
        text: String,
    },
    /// A vertical gap a field's `[Space]` declared.
    Space {
        /// Gap height, in pixels.
        pixels: f32,
    },
    /// One field with the widget its kind asks for.
    Single {
        /// The field's position in the component's field list (the value's,
        /// too).
        index: usize,
        /// The field's registered name.
        field: String,
        /// The widget to draw.
        widget: FieldWidget,
        /// Text the field's `[Tooltip]` declared, shown on hover of the label.
        tooltip: Option<String>,
    },
    /// A short array of scalars: one row, one widget per element, labelled
    /// with the element's axis (`X`, `Y`, `Z`, `W`) - Unity's vector row.
    Vector {
        /// The array's position in the component's field list.
        index: usize,
        /// The field's registered name.
        field: String,
        /// The element widgets' kind.
        widget: FieldWidget,
        /// How many elements the row holds (2..=`VECTOR_AXES_MAX`).
        elements: usize,
        /// Text the field's `[Tooltip]` declared, shown on hover of the label.
        tooltip: Option<String>,
    },
    /// A reference to an asset: the asset it names, a picker of the assets
    /// of its type, a clear button and a drop target for the Assets tab.
    Asset {
        /// The field's position in the component's field list.
        index: usize,
        /// The field's registered name.
        field: String,
        /// The registered name of the asset type the field references, when
        /// recorded; the picker offers only assets of this type.
        asset_type: Option<String>,
        /// Text the field's `[Tooltip]` declared, shown on hover of the label.
        tooltip: Option<String>,
    },
    /// A reference to another entity: the referenced
    /// entity's Hierarchy label, a picker over the scene's entities, a
    /// viewport pick, a clear button and a drop target for the Entities tab.
    Entity {
        /// The field's position in the component's field list.
        index: usize,
        /// The field's registered name.
        field: String,
        /// Text the field's `[Tooltip]` declared, shown on hover of the label.
        tooltip: Option<String>,
    },
    /// A colour: one swatch, and a popup that holds the picker. Its leaves are
    /// the channels, which are either one array field's elements or one scalar
    /// field each.
    Color {
        /// The channels the row writes, in R/G/B[/A] order.
        leaves: Vec<RowLeaf>,
        /// The name the row labels itself with.
        field: String,
        /// Text a channel's `[Tooltip]` declared, shown on hover of the label.
        tooltip: Option<String>,
    },
    /// A vector assembled from consecutive leaves that share a dotted prefix
    /// and carry a `#[pill(vector)]` decorator. It draws as one row of X/Y/Z
    /// numeric fields, the way a `[f32; 3]` field already draws.
    VectorGroup {
        /// The axes the row writes, in X/Y/Z[/W] order.
        leaves: Vec<RowLeaf>,
        /// The element widgets' kind.
        widget: FieldWidget,
        /// The name the row labels itself with.
        field: String,
        /// Text a leaf's `[Tooltip]` declared, shown on hover of the label.
        tooltip: Option<String>,
    },
    /// A field shown read-only (a long array, a list, an opaque value).
    ReadOnly {
        /// The field's position in the component's field list.
        index: usize,
        /// The field's registered name.
        field: String,
        /// Text the field's `[Tooltip]` declared, shown on hover of the label.
        tooltip: Option<String>,
    },
}

/// The rows a component section draws, in field order.
///
/// A scalar array of two to four elements becomes one vector row, the way
/// Unity draws a `Vector3` as three fields on one line. Anything longer draws
/// as one read-only row.
///
/// The decorators are applied here, so the shell only draws what the model
/// asks for. `[HideInInspector]` drops the field together with its header and
/// gap. `[Header]` and `[Space]` add their own rows above the field, in
/// declaration order. A `[Tooltip]` is shown on the field's row.
pub fn rows(component: &ComponentView) -> Vec<FieldRow> {
    let mut rows = Vec::new();
    let mut index = 0;
    while index < component.fields.len() {
        // Step 1: Read the field's descriptor and value, and skip a hidden
        // field together with the rows it declared.
        let Some(descriptor) = component.fields.get(index) else {
            break;
        };
        let Some(value) = component.values.get(index) else {
            break;
        };
        // A hidden field takes its decorations with it. A header drawn over
        // nothing would look worse than drawing nothing at all.
        if descriptor
            .decorators
            .iter()
            .any(|decorator| matches!(decorator, FieldDecorator::HideInInspector))
        {
            index += 1;
            continue;
        }
        // Step 2: Draw the rows the decorators declare, and keep the tooltip
        // for the field's own row.
        let mut tooltip: Option<String> = None;
        for decorator in descriptor.decorators.iter() {
            match decorator {
                FieldDecorator::Header(text) => rows.push(FieldRow::Header {
                    text: text.to_string(),
                }),
                FieldDecorator::Space(pixels) => rows.push(FieldRow::Space { pixels: *pixels }),
                FieldDecorator::Tooltip(text) => tooltip = Some(text.to_string()),
                FieldDecorator::HideInInspector
                | FieldDecorator::Range { .. }
                | FieldDecorator::TextArea { .. }
                | FieldDecorator::Multiline(_)
                | FieldDecorator::Min(_)
                | FieldDecorator::Color
                | FieldDecorator::Vector => {}
            }
        }
        // Step 3: A run of scalar leaves sharing a dotted prefix and a
        // grouping decorator draws as one row: a colour's channels or a
        // vector's axes. The run decides the row, so a nested struct the world
        // flattened at registration reads as the value it is.
        if let Some((row, consumed)) = grouped_row(component, index, tooltip.clone()) {
            rows.push(row);
            index += consumed;
            continue;
        }
        // Step 4: Draw the one field with the widget its kind and its
        // decorators ask for.
        let widget = widget_for_decorated(value, descriptor.decorators.as_ref());
        let row = match value {
            FieldValue::Array(elements)
                if (COLOR_CHANNELS_MIN..=VECTOR_AXES_MAX).contains(&elements.len())
                    && elements.iter().all(|element| {
                        matches!(
                            widget_for(element),
                            FieldWidget::Integer | FieldWidget::Float
                        )
                    })
                    && descriptor
                        .decorators
                        .iter()
                        .any(|decorator| matches!(decorator, FieldDecorator::Color)) =>
            {
                FieldRow::Color {
                    leaves: (0..elements.len())
                        .map(|element| RowLeaf {
                            index,
                            field: descriptor.name.to_string(),
                            element: Some(element),
                        })
                        .collect(),
                    field: descriptor.name.to_string(),
                    tooltip,
                }
            }
            FieldValue::Array(elements)
                if (2..=VECTOR_AXES_MAX).contains(&elements.len())
                    && elements.iter().all(|element| {
                        matches!(
                            widget_for(element),
                            FieldWidget::Integer | FieldWidget::Float
                        )
                    }) =>
            {
                FieldRow::Vector {
                    index,
                    field: descriptor.name.to_string(),
                    // A range belongs to one scalar, so a vector row keeps the
                    // plain element widget and the tooltip.
                    widget: widget_for(elements.first().expect("the array is not empty")),
                    elements: elements.len(),
                    tooltip,
                }
            }
            FieldValue::Asset { asset_type, .. } => FieldRow::Asset {
                index,
                field: descriptor.name.to_string(),
                asset_type: asset_type.clone(),
                tooltip,
            },
            FieldValue::Entity { .. } => FieldRow::Entity {
                index,
                field: descriptor.name.to_string(),
                tooltip,
            },
            FieldValue::Array(_) | FieldValue::List { .. } | FieldValue::Opaque { .. } => {
                FieldRow::ReadOnly {
                    index,
                    field: descriptor.name.to_string(),
                    tooltip,
                }
            }
            _ => FieldRow::Single {
                index,
                field: descriptor.name.to_string(),
                widget,
                tooltip,
            },
        };
        rows.push(row);
        index += 1;
    }
    rows
}

/// The grouped row that starts at `start_index`, with how many fields it
/// consumed.
///
/// A field marked as both a colour and a vector draws as a colour. The first
/// field's suffix decides which run can match at all, so only one grouping is
/// tried.
fn grouped_row(
    component: &ComponentView,
    start_index: usize,
    tooltip: Option<String>,
) -> Option<(FieldRow, usize)> {
    if let Some(leaves) = channel_run(
        component,
        start_index,
        &COLOR_CHANNELS,
        &FieldDecorator::Color,
        COLOR_CHANNELS_MIN,
    ) {
        let consumed = leaves.len();
        let field = grouped_label(component, start_index, "color");
        return Some((
            FieldRow::Color {
                leaves,
                field,
                tooltip,
            },
            consumed,
        ));
    }
    let leaves = channel_run(
        component,
        start_index,
        &VECTOR_AXES,
        &FieldDecorator::Vector,
        VECTOR_AXES_MIN,
    )?;
    let consumed = leaves.len();
    let field = grouped_label(component, start_index, "vector");
    let widget = component
        .values
        .get(start_index)
        .map(widget_for)
        .unwrap_or(FieldWidget::Float);
    Some((
        FieldRow::VectorGroup {
            leaves,
            widget,
            field,
            tooltip,
        },
        consumed,
    ))
}

/// The name a grouped row labels itself with: the dotted prefix its leaves
/// share.
///
/// When the leaves have no prefix, the row is named after its kind. The
/// `Color` component's own channels are `r`, `g`, `b` and `a`, with nothing in
/// front of them.
fn grouped_label(component: &ComponentView, start_index: usize, fallback_label: &str) -> String {
    match component
        .fields
        .get(start_index)
        .map(|field| leaf_parts(field.name.as_str()))
    {
        Some((prefix, _)) if !prefix.is_empty() => prefix.to_string(),
        _ => fallback_label.to_string(),
    }
}

/// The dotted prefix and last segment of a leaf field's name: `fill.g` is
/// (`fill`, `g`), and a bare `g` is (`""`, `g`).
pub fn leaf_parts(field_name: &str) -> (&str, &str) {
    match field_name.rfind('.') {
        Some(dot) => (&field_name[..dot], &field_name[dot + 1..]),
        None => ("", field_name),
    }
}

/// A run of consecutive scalar leaves, starting at `start_index`, that form
/// the channels of one grouped row.
///
/// Every leaf in the run must share the first leaf's dotted prefix, carry
/// `required_decorator`, hold a numeric value and use the next suffix from
/// `suffixes` in order. The run stops at the first leaf that does not match.
/// A run shorter than `minimum_leaf_count` is no row at all: a `fill.r` with
/// no `fill.g` is just a float.
fn channel_run(
    component: &ComponentView,
    start_index: usize,
    suffixes: &[&str],
    required_decorator: &FieldDecorator,
    minimum_leaf_count: usize,
) -> Option<Vec<RowLeaf>> {
    let first = component.fields.get(start_index)?;
    let (prefix, first_suffix) = leaf_parts(first.name.as_str());
    // Step 1: The run starts only at the first suffix with the grouping
    // decorator.
    if first_suffix != suffixes[0] || !first.decorators.contains(required_decorator) {
        return None;
    }
    // Step 2: Read the following leaves while the prefix, the suffix and the
    // decorator still match.
    let mut leaves = Vec::new();
    for (offset, suffix) in suffixes.iter().enumerate() {
        let Some(descriptor) = component.fields.get(start_index + offset) else {
            break;
        };
        let Some(value) = component.values.get(start_index + offset) else {
            break;
        };
        let (candidate_prefix, candidate_suffix) = leaf_parts(descriptor.name.as_str());
        if candidate_prefix != prefix
            || candidate_suffix != *suffix
            || !descriptor.decorators.contains(required_decorator)
            || !matches!(widget_for(value), FieldWidget::Integer | FieldWidget::Float)
        {
            break;
        }
        leaves.push(RowLeaf {
            index: start_index + offset,
            field: descriptor.name.to_string(),
            element: None,
        });
    }
    (leaves.len() >= minimum_leaf_count).then_some(leaves)
}

// =============================================================================
// Cached rows
// =============================================================================

/// The log lines a drawn row can emit, held as bits. There are two: the row's
/// own line, and the `mixed` line that a scalar or vector row adds when the
/// selected entities disagree on its value.
///
/// A cached row carries this, so a row that redraws every frame still logs
/// once per subject. The row's position in the cached model identifies it for
/// the session, so no key has to be built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowLogged(u8);

impl RowLogged {
    /// The row's own `[editor] inspector row kind=…` line.
    pub const ROW: u8 = 1 << 0;
    /// The `mixed` line a row adds when the selection disagrees on its value.
    pub const MIXED: u8 = 1 << 1;

    /// Whether the line `line_bit` names has already been logged for this row.
    #[must_use]
    pub fn has(self, line_bit: u8) -> bool {
        self.0 & line_bit != 0
    }

    /// Mark the line `line_bit` names as logged.
    pub fn mark(&mut self, line_bit: u8) {
        self.0 |= line_bit;
    }
}

/// One row of a component's cached model and the lines it has already logged.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedRow {
    /// The row to draw.
    pub row: FieldRow,
    /// The lines this row already logged for the current subject.
    pub logged: RowLogged,
}

/// One component's cached row model and the selection's write targets.
///
/// The rows are rebuilt only when [`row_layout`] changes, and the targets only
/// when the selection's answer for this component changes. Both live in the
/// shared session, so a shell reload keeps them.
#[derive(Default)]
pub struct CachedRows {
    /// The layout the cached rows were built from.
    pub layout: Option<RowLayout>,
    /// The rows, in field order, each with the lines it has already logged.
    pub rows: Vec<CachedRow>,
    /// The selection the cached targets were derived from.
    pub targets_stamp: Option<u64>,
    /// The entities the section's edits reach (the primary first).
    pub targets: Vec<Entity>,
}

/// The layout fingerprint of one component's row model.
///
/// It covers the field names, the decorators and the value kinds that [`rows`]
/// reads, but never the values themselves, so editing a value does not rebuild
/// the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowLayout(u64);

/// The stamp that decides when a component's cached rows are rebuilt.
pub fn row_layout(component: &ComponentView) -> RowLayout {
    let mut hasher = DefaultHasher::new();
    component.fields.len().hash(&mut hasher);
    component.values.len().hash(&mut hasher);
    for (descriptor, value) in component.fields.iter().zip(component.values.iter()) {
        descriptor.name.hash(&mut hasher);
        descriptor.decorators.len().hash(&mut hasher);
        for decorator in descriptor.decorators.iter() {
            hash_decorator(decorator, &mut hasher);
        }
        hash_value_kind(value, &mut hasher);
    }
    RowLayout(hasher.finish())
}

/// Hash what one decorator adds to the row model: its kind, plus the text or
/// value that the row draws or the widget takes from it.
fn hash_decorator(decorator: &FieldDecorator, hasher: &mut DefaultHasher) {
    match decorator {
        FieldDecorator::HideInInspector => 0u8.hash(hasher),
        FieldDecorator::Header(text) => {
            1u8.hash(hasher);
            text.hash(hasher);
        }
        FieldDecorator::Space(pixels) => {
            2u8.hash(hasher);
            pixels.to_bits().hash(hasher);
        }
        FieldDecorator::Tooltip(text) => {
            3u8.hash(hasher);
            text.hash(hasher);
        }
        FieldDecorator::Range { min, max } => {
            4u8.hash(hasher);
            min.to_bits().hash(hasher);
            max.to_bits().hash(hasher);
        }
        FieldDecorator::Min(value) => {
            5u8.hash(hasher);
            value.to_bits().hash(hasher);
        }
        FieldDecorator::TextArea { min_lines, .. } => {
            6u8.hash(hasher);
            min_lines.hash(hasher);
        }
        FieldDecorator::Multiline(count) => {
            7u8.hash(hasher);
            count.hash(hasher);
        }
        FieldDecorator::Color => 8u8.hash(hasher),
        FieldDecorator::Vector => 9u8.hash(hasher),
    }
}

/// Hash the parts of a value the row model reads: its kind, an array's element
/// count and element kinds, a list's element count, an asset's type.
fn hash_value_kind(value: &FieldValue, hasher: &mut DefaultHasher) {
    let kind: u8 = match value {
        FieldValue::F32(_) => 1,
        FieldValue::F64(_) => 2,
        FieldValue::I8(_) => 3,
        FieldValue::I16(_) => 4,
        FieldValue::I32(_) => 5,
        FieldValue::I64(_) => 6,
        FieldValue::U8(_) => 7,
        FieldValue::U16(_) => 8,
        FieldValue::U32(_) => 9,
        FieldValue::U64(_) => 10,
        FieldValue::Bool(_) => 11,
        FieldValue::Usize(_) => 12,
        FieldValue::Isize(_) => 13,
        FieldValue::Array(_) => 14,
        FieldValue::List { .. } => 15,
        FieldValue::Text(_) => 16,
        FieldValue::Asset { .. } => 17,
        FieldValue::Entity { .. } => 18,
        FieldValue::Opaque { .. } => 19,
        FieldValue::Enum { .. } => 20,
    };
    kind.hash(hasher);
    match value {
        FieldValue::Array(elements) => {
            elements.len().hash(hasher);
            for element in elements {
                hash_value_kind(element, hasher);
            }
        }
        FieldValue::List { element_count, .. } => element_count.hash(hasher),
        FieldValue::Asset { asset_type, .. } => asset_type.hash(hasher),
        // The tag lists the variants the dropdown offers.
        FieldValue::Enum { type_tag, .. } => type_tag.hash(hasher),
        _ => {}
    }
}

/// A stamp over the two facts that change a component's write targets: which
/// entities are selected, and whether each of them carries the component.
fn selection_stamp(selected_details: &[EntityDetail], component: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    selected_details.len().hash(&mut hasher);
    for detail in selected_details {
        detail.entity.hash(&mut hasher);
        detail
            .components
            .iter()
            .any(|view| view.type_name == component)
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// Take one component's cached rows and write targets out of the session,
/// rebuilding whichever part moved.
///
/// Taking them out lets the caller draw while it still holds the same
/// `&mut InspectorSession` the model lives in, because the draw also borrows
/// the session for its pickers and rectangles. The caller hands both back with
/// [`store_component_rows`] after the section's rows.
pub fn take_component_rows(
    session: &mut InspectorSession,
    component: &ComponentView,
    selected_details: &[EntityDetail],
    primary: Entity,
) -> (Vec<CachedRow>, Vec<Entity>) {
    let type_name = component.type_name.as_str();
    let layout = row_layout(component);
    let targets_stamp = selection_stamp(selected_details, type_name);
    // Step 1: Create this component's cache entry the first time it draws.
    if !session.row_models.contains_key(type_name) {
        session
            .row_models
            .insert(type_name.to_string(), CachedRows::default());
    }
    let row_model = session
        .row_models
        .get_mut(type_name)
        .expect("the row-model entry was inserted just above");
    // Step 2: Rebuild the rows when the layout fingerprint moved.
    if row_model.layout != Some(layout) {
        row_model.layout = Some(layout);
        row_model.rows = rows(component)
            .into_iter()
            .map(|row| CachedRow {
                row,
                logged: RowLogged::default(),
            })
            .collect();
    }
    // Step 3: Rebuild the write targets when the selection moved.
    if row_model.targets_stamp != Some(targets_stamp) {
        row_model.targets_stamp = Some(targets_stamp);
        selection_targets_into(selected_details, primary, type_name, &mut row_model.targets);
    }
    // Step 4: Hand both out. The cache keeps its slots, empty, until the
    // caller stores the rows back.
    (
        std::mem::take(&mut row_model.rows),
        std::mem::take(&mut row_model.targets),
    )
}

/// Hand one component's rows and targets back after its section drew.
pub fn store_component_rows(
    session: &mut InspectorSession,
    type_name: &str,
    rows: Vec<CachedRow>,
    targets: Vec<Entity>,
) {
    let Some(row_model) = session.row_models.get_mut(type_name) else {
        return;
    };
    row_model.rows = rows;
    row_model.targets = targets;
}

// =============================================================================
// Axis labels
// =============================================================================

/// The axis label a vector row shows for element `index`: `X`, `Y`, `Z`, `W`.
pub fn axis_label(index: usize) -> &'static str {
    match index {
        0 => "X",
        1 => "Y",
        2 => "Z",
        _ => "W",
    }
}

/// The colour a vector row's axis label takes: X red, Y green, Z blue, and
/// grey for a fourth element.
pub fn axis_color(index: usize) -> Color32 {
    match index {
        0 => Color32::from_rgb(214, 96, 96),
        1 => Color32::from_rgb(112, 190, 112),
        2 => Color32::from_rgb(104, 148, 232),
        _ => Color32::GRAY,
    }
}

// =============================================================================
// Edit builders
// =============================================================================

/// The recorded component attach, labelled `add {short}`: the Component
/// menu's picker and the scripted channel share it.
pub fn component_add(entity: Entity, component: &str) -> (EditorCommand, String) {
    (
        EditorCommand::AddComponent {
            entity,
            component: component.to_string(),
        },
        format!("add {}", short_component_name(component)),
    )
}

/// The recorded component detach, labelled `remove {short}`.
pub fn component_remove(entity: Entity, component: &str) -> (EditorCommand, String) {
    (
        EditorCommand::RemoveComponent {
            entity,
            component: component.to_string(),
        },
        format!("remove {}", short_component_name(component)),
    )
}

/// The recorded whole-field edit: `SetField` labelled `set {short}.{field}`.
pub fn field_edit(
    entity: Entity,
    component: &str,
    field: &str,
    value: FieldValue,
) -> (EditorCommand, String) {
    (
        EditorCommand::SetField {
            entity,
            component: component.to_string(),
            field: field.to_string(),
            value,
        },
        format!("set {}.{}", short_component_name(component), field),
    )
}

/// The recorded asset reference edit: `SetFieldAsset` under the field edit's
/// label (`set Component.field`).
///
/// `reference` is a path relative to `res`, a guid, or the name the asset was
/// added to the store under. `None` clears the reference.
pub fn asset_edit(
    entity: Entity,
    component: &str,
    field: &str,
    reference: Option<String>,
) -> (EditorCommand, String) {
    (
        EditorCommand::SetFieldAsset {
            entity,
            component: component.to_string(),
            field: field.to_string(),
            reference,
        },
        format!("set {}.{}", short_component_name(component), field),
    )
}

/// [`asset_edit`] for every entity of `targets`, as one recorded step labelled
/// and coalesced by the primary's edit, like [`selection_field_edit`].
pub fn selection_asset_edit(
    targets: &[Entity],
    primary: Entity,
    component: &str,
    field: &str,
    reference: Option<String>,
) -> (Vec<EditorCommand>, String, String) {
    batch(targets, primary, component, field, |target| {
        asset_edit(target, component, field, reference.clone())
    })
}

/// The recorded entity reference edit: `SetFieldEntity` under the field edit's
/// label (`set Component.field`).
///
/// `reference` is the entity to point at, and `None` clears the reference.
pub fn entity_edit(
    entity: Entity,
    component: &str,
    field: &str,
    reference: Option<Entity>,
) -> (EditorCommand, String) {
    (
        EditorCommand::SetFieldEntity {
            entity,
            component: component.to_string(),
            field: field.to_string(),
            reference,
        },
        format!("set {}.{}", short_component_name(component), field),
    )
}

/// [`entity_edit`] for every entity of `targets`, as one recorded step labelled
/// and coalesced by the primary's edit, like [`selection_field_edit`].
pub fn selection_entity_edit(
    targets: &[Entity],
    primary: Entity,
    component: &str,
    field: &str,
    reference: Option<Entity>,
) -> (Vec<EditorCommand>, String, String) {
    batch(targets, primary, component, field, |target| {
        entity_edit(target, component, field, reference)
    })
}

// =============================================================================
// Reference rows
// =============================================================================

/// The entities an entity row's picker offers: every entity the Hierarchy
/// lists, in its order.
///
/// The row's own entity stays in the list. A component that references its own
/// entity is odd but harmless, and hiding it would make the picker disagree
/// with the Hierarchy for a rule the user could not see.
pub fn entity_candidates(hierarchy_entries: &[EntityListEntry]) -> Vec<&EntityListEntry> {
    hierarchy_entries.iter().collect()
}

/// The rows a picker offers for `asset_type`.
///
/// The listing's files of the type come first, then the store's own assets of
/// that type - the ones built in code, which a directory walk cannot find.
/// Both sides compare the type name's last segment, so a module's spelling of
/// a type and the renderer's agree however each was registered. A field
/// without a recorded type offers nothing, rather than every asset of every
/// type.
pub fn picker_candidates<'a>(
    listing: &'a [AssetListing],
    engine_assets: &'a [EngineAsset],
    asset_type: Option<&str>,
) -> (Vec<&'a AssetListing>, Vec<&'a EngineAsset>) {
    let Some(asset_type) = asset_type else {
        return (Vec::new(), Vec::new());
    };
    (
        listing
            .iter()
            .filter(|entry| names_type(entry.kind.text(), asset_type))
            .collect(),
        engine_assets
            .iter()
            .filter(|asset| names_type(&asset.kind, asset_type))
            .collect(),
    )
}

/// Whether a row's type name is the one a field asked for, compared on the
/// last `::` segment.
fn names_type(row_type_name: &str, field_type_name: &str) -> bool {
    row_type_name.rsplit("::").next() == field_type_name.rsplit("::").next()
}

/// The listing's files of a field's type, in listing order.
///
/// This is the file half of what an asset row's picker offers;
/// [`picker_candidates`] adds the store's own assets.
pub fn asset_candidates<'a>(
    listing: &'a [AssetListing],
    asset_type: Option<&str>,
) -> Vec<&'a AssetListing> {
    picker_candidates(listing, &[], asset_type).0
}

/// Whether a scripted or typed value names "no reference": the word that
/// clears an asset or entity reference field.
pub fn is_cleared_reference(text: &str) -> bool {
    matches!(text.trim(), "" | "none" | "None")
}

/// The entity a scripted reference names: a Hierarchy label (`3v0`), a bare
/// slot id (`3`, read as generation 0), or `None` for the word that clears the
/// reference.
///
/// # Errors
///
/// A message naming the text when it is not an entity label.
pub fn parse_entity_reference(text: &str) -> Result<Option<Entity>, String> {
    if is_cleared_reference(text) {
        return Ok(None);
    }
    let trimmed = text.trim();
    let (id_text, generation_text) = match trimmed.split_once('v') {
        Some((id, generation)) => (id, generation),
        None => (trimmed, "0"),
    };
    let id: u64 = id_text
        .parse()
        .map_err(|_| format!("`{trimmed}` is not an entity label (`<id>v<generation>`)"))?;
    let generation: u32 = generation_text
        .parse()
        .map_err(|_| format!("`{trimmed}` is not an entity label (`<id>v<generation>`)"))?;
    Ok(Some(Entity::from_raw(id, generation)))
}

// =============================================================================
// Element edits
// =============================================================================

/// The recorded array-element edit: `SetArrayElement` labelled
/// `set {short}[{index}].{field}`.
pub fn element_edit(
    entity: Entity,
    component: &str,
    field: &str,
    index: usize,
    value: FieldValue,
) -> (EditorCommand, String) {
    (
        EditorCommand::SetArrayElement {
            entity,
            component: component.to_string(),
            field: field.to_string(),
            index,
            value,
        },
        element_label(component, field, index),
    )
}

/// The step label an array element's edit records under:
/// `set {short}[{index}].{field}`.
fn element_label(component: &str, field: &str, index: usize) -> String {
    format!(
        "set {}[{}].{}",
        short_component_name(component),
        index,
        field
    )
}

/// The continuous-edit key one inspector widget's commits share, so a drag
/// or a typing run merges into one undo step.
pub fn field_coalesce_key(entity: Entity, component: &str, field: &str) -> String {
    format!("inspector.{entity}.{component}.{field}")
}

/// The selected entities one component's edit reaches: the primary first, then
/// every other selected entity that carries the component.
///
/// When a selection has entities without the component, only the ones that
/// carry it are edited. Writing to an entity without it would fail the command
/// and put an error in the Console for a row that showed nothing.
pub fn selection_targets(
    selected_details: &[EntityDetail],
    primary: Entity,
    component: &str,
) -> Vec<Entity> {
    let mut targets = Vec::new();
    selection_targets_into(selected_details, primary, component, &mut targets);
    targets
}

/// [`selection_targets`], written into a caller's buffer so a redraw reuses
/// the allocation.
pub fn selection_targets_into(
    selected_details: &[EntityDetail],
    primary: Entity,
    component: &str,
    targets: &mut Vec<Entity>,
) {
    targets.clear();
    targets.push(primary);
    for detail in selected_details {
        if detail.entity == primary {
            continue;
        }
        if detail
            .components
            .iter()
            .any(|view| view.type_name == component)
        {
            targets.push(detail.entity);
        }
    }
}

/// One recorded batch that writes `value` into `field` on every target, with
/// its label and coalesce key.
///
/// The label names the primary's component and field, so a step reads the same
/// whether one entity or ten were written. The coalesce key is the primary's,
/// so one drag across a multi-selection is one undo step.
pub fn selection_field_edit(
    targets: &[Entity],
    primary: Entity,
    component: &str,
    field: &str,
    value: FieldValue,
) -> (Vec<EditorCommand>, String, String) {
    batch(targets, primary, component, field, |target| {
        field_edit(target, component, field, value.clone())
    })
}

/// One recorded batch that writes element `index` of `field` on every target.
///
/// The step is labelled and keyed on the element (`field[index]`), which is what
/// [`element_write`] does for a single entity. A drag on one element of an
/// array is one undo step, and drags on two elements are two.
pub fn selection_element_edit(
    targets: &[Entity],
    primary: Entity,
    component: &str,
    field: &str,
    index: usize,
    value: FieldValue,
) -> (Vec<EditorCommand>, String, String) {
    batch(
        targets,
        primary,
        component,
        &element_coalesce_field(field, index),
        |target| element_edit(target, component, field, index, value.clone()),
    )
}

/// One recorded field write on an editor-editable resource.
///
/// The command is `SetResourceField`, labelled `set {resource}.{field}`. A
/// resource has no selection, so there is no per-entity batch here.
pub fn resource_edit(resource: &str, field: &str, value: FieldValue) -> (EditorCommand, String) {
    (
        EditorCommand::SetResourceField {
            resource: resource.to_string(),
            field: field.to_string(),
            value,
        },
        format!("set {resource}.{field}"),
    )
}

/// The coalesce key a resource field's writes share, so a drag on one row is
/// one undo step.
pub fn resource_coalesce_key(resource: &str, field: &str) -> String {
    format!("resource.{resource}.{field}")
}

/// The field name an array element's coalesce key carries: `field[index]`.
fn element_coalesce_field(field: &str, index: usize) -> String {
    format!("{field}[{index}]")
}

/// A selection's batch of one-entity edits.
///
/// `build_target_edit` runs once per target, and the run for the primary also
/// names the step. Every selection builder has this shape: one command per
/// target, with the primary's label and coalesce key. A new reference kind is
/// therefore a new `build_target_edit`, not another copy of the loop.
fn batch(
    targets: &[Entity],
    primary: Entity,
    component: &str,
    coalesce_field: &str,
    build_target_edit: impl Fn(Entity) -> (EditorCommand, String),
) -> (Vec<EditorCommand>, String, String) {
    let mut commands = Vec::with_capacity(targets.len());
    for target in targets {
        commands.push(build_target_edit(*target).0);
    }
    let (_command, label) = build_target_edit(primary);
    (
        commands,
        label,
        field_coalesce_key(primary, component, coalesce_field),
    )
}

/// Element `index` of an array value, when it is one and the index fits.
pub fn element_field(value: &FieldValue, index: usize) -> Option<FieldValue> {
    let FieldValue::Array(elements) = value else {
        return None;
    };
    elements.get(index).cloned()
}

// =============================================================================
// Writes
// =============================================================================

/// What one value edit queues: the step's commands, the label it records
/// under, and the key that collapses consecutive writes from one drag.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedEdit {
    /// The commands, in the order the batch applies them.
    pub commands: Vec<EditorCommand>,
    /// The history label the step records under.
    pub label: String,
    /// Consecutive writes sharing this key merge into one undo step.
    pub coalesce: String,
}

impl RecordedEdit {
    /// One entity's edit, with the key its field coalesces on.
    fn from_entity_edit(edit: (EditorCommand, String), coalesce_key: String) -> Self {
        Self {
            commands: vec![edit.0],
            label: edit.1,
            coalesce: coalesce_key,
        }
    }

    /// A selection's batch, which carries its own label and key.
    fn from_selection_batch(edit: (Vec<EditorCommand>, String, String)) -> Self {
        Self {
            commands: edit.0,
            label: edit.1,
            coalesce: edit.2,
        }
    }
}

/// What a field's text names when the field is a reference rather than a value.
enum ReferenceWrite {
    /// The asset reference the text names, `None` to clear it.
    Asset(Option<String>),
    /// The entity reference the text names, `None` to clear it.
    Entity(Option<Entity>),
}

/// The reference `text` names for `current`, or `None` when the field holds a
/// plain value.
///
/// This is the one place that knows how a reference row reads its text, and
/// [`field_write`] and [`selection_write`] both use it. The two differ in how
/// many entities they write, never in what the text means.
///
/// # Errors
///
/// The message saying why the text does not name an entity.
fn reference_write(
    current: &FieldValue,
    value_text: &str,
) -> Option<Result<ReferenceWrite, String>> {
    if matches!(current, FieldValue::Asset { .. }) {
        let reference = (!is_cleared_reference(value_text)).then(|| value_text.trim().to_owned());
        return Some(Ok(ReferenceWrite::Asset(reference)));
    }
    if matches!(current, FieldValue::Entity { .. }) {
        return Some(parse_entity_reference(value_text).map(ReferenceWrite::Entity));
    }
    None
}

/// One field's write, decided in one place.
///
/// What a field's text means depends on the value it edits. An asset reference
/// takes a path, a guid or a store name. An entity reference takes a Hierarchy
/// label. Any other value takes its own text. Every surface that writes a field
/// goes through here - the Inspector, the scripted channel and a module panel -
/// so a new [`FieldValue`] family is one branch rather than five copies, and no
/// surface can be missing it.
///
/// # Errors
///
/// The message saying why the text does not name a value for this field.
pub fn field_write(
    entity: Entity,
    component: &str,
    field: &str,
    current: &FieldValue,
    value_text: &str,
) -> Result<RecordedEdit, String> {
    let coalesce_key = field_coalesce_key(entity, component, field);
    match reference_write(current, value_text) {
        Some(Ok(ReferenceWrite::Asset(reference))) => Ok(RecordedEdit::from_entity_edit(
            asset_edit(entity, component, field, reference),
            coalesce_key,
        )),
        Some(Ok(ReferenceWrite::Entity(reference))) => Ok(RecordedEdit::from_entity_edit(
            entity_edit(entity, component, field, reference),
            coalesce_key,
        )),
        Some(Err(message)) => Err(message),
        None => {
            let value = parse_field_value(value_text, current)?;
            Ok(RecordedEdit::from_entity_edit(
                field_edit(entity, component, field, value),
                coalesce_key,
            ))
        }
    }
}

/// One array element's write, keyed on the element.
///
/// The key names `field[index]`, so two drags on two elements of one array are
/// two undo steps.
///
/// # Errors
///
/// A message when the index is outside the array or the text does not parse
/// against the element's kind.
pub fn element_write(
    entity: Entity,
    component: &str,
    field: &str,
    current: &FieldValue,
    index: usize,
    value_text: &str,
) -> Result<RecordedEdit, String> {
    let element = element_field(current, index)
        .ok_or_else(|| format!("`{field}[{index}]` is outside the array"))?;
    let value = parse_field_value(value_text, &element)?;
    let coalesce_key = field_coalesce_key(entity, component, &element_coalesce_field(field, index));
    Ok(RecordedEdit::from_entity_edit(
        element_edit(entity, component, field, index, value),
        coalesce_key,
    ))
}

/// One field's write over a selection: the same three cases as
/// [`field_write`], recorded once for the whole set.
///
/// # Errors
///
/// The message saying why the text does not name a value for this field.
pub fn selection_write(
    targets: &[Entity],
    primary: Entity,
    component: &str,
    field: &str,
    current: &FieldValue,
    value_text: &str,
) -> Result<RecordedEdit, String> {
    match reference_write(current, value_text) {
        Some(Ok(ReferenceWrite::Asset(reference))) => Ok(RecordedEdit::from_selection_batch(
            selection_asset_edit(targets, primary, component, field, reference),
        )),
        Some(Ok(ReferenceWrite::Entity(reference))) => Ok(RecordedEdit::from_selection_batch(
            selection_entity_edit(targets, primary, component, field, reference),
        )),
        Some(Err(message)) => Err(message),
        None => {
            let value = parse_field_value(value_text, current)?;
            Ok(RecordedEdit::from_selection_batch(selection_field_edit(
                targets, primary, component, field, value,
            )))
        }
    }
}

/// One recorded batch that writes every leaf of a grouped row, across the whole
/// selection, as one undo step.
///
/// `values` holds one value per leaf, in the row's own order. The label and the
/// coalesce key both name the first leaf, in the form the rest of the write
/// path uses for it: `field[index]` for an array element, and `field`
/// otherwise. So a colour row's picker and its per-channel fields record the
/// same step for one element, not two names for one edit.
pub fn selection_leaf_edit(
    targets: &[Entity],
    primary: Entity,
    component: &str,
    leaves: &[RowLeaf],
    values: &[FieldValue],
) -> (Vec<EditorCommand>, String, String) {
    let mut commands = Vec::with_capacity(targets.len() * leaves.len());
    // Step 1: One command per leaf, for every target.
    for target in targets {
        for (leaf, value) in leaves.iter().zip(values) {
            let command = match leaf.element {
                Some(element) => {
                    element_edit(*target, component, &leaf.field, element, value.clone()).0
                }
                None => field_edit(*target, component, &leaf.field, value.clone()).0,
            };
            commands.push(command);
        }
    }
    // Step 2: Label and key the batch on the first leaf, in the form the
    // single-field writes use for that leaf.
    let label = match leaves.first() {
        Some(leaf) => match leaf.element {
            Some(element) => element_label(component, &leaf.field, element),
            None => format!("set {}.{}", short_component_name(component), leaf.field),
        },
        None => format!("set {}", short_component_name(component)),
    };
    let key = leaves
        .first()
        .map(|leaf| match leaf.element {
            Some(element) => field_coalesce_key(
                primary,
                component,
                &element_coalesce_field(&leaf.field, element),
            ),
            None => field_coalesce_key(primary, component, &leaf.field),
        })
        .unwrap_or_default();
    (commands, label, key)
}

// =============================================================================
// Text and values
// =============================================================================

/// Parse an edited text back into the kind `current` already holds.
///
/// The scripted channel and a text field both commit through this. A float
/// field takes a float, a text field takes the rest of the line, and a kind
/// with no editor is refused.
///
/// # Errors
///
/// A message saying the text is not a float, an integer, an unsigned number or
/// a boolean, or a variant of the enum, as the field's kind requires, or that
/// the field's kind has no editor here.
pub fn parse_field_value(text: &str, current: &FieldValue) -> Result<FieldValue, String> {
    let text = text.trim();
    let parsed = match current {
        FieldValue::F32(_) => FieldValue::F32(text.parse().map_err(|_| "not a float")?),
        FieldValue::F64(_) => FieldValue::F64(text.parse().map_err(|_| "not a float")?),
        FieldValue::I8(_) => FieldValue::I8(text.parse().map_err(|_| "not an integer")?),
        FieldValue::I16(_) => FieldValue::I16(text.parse().map_err(|_| "not an integer")?),
        FieldValue::I32(_) => FieldValue::I32(text.parse().map_err(|_| "not an integer")?),
        FieldValue::I64(_) => FieldValue::I64(text.parse().map_err(|_| "not an integer")?),
        FieldValue::U8(_) => FieldValue::U8(text.parse().map_err(|_| "not an unsigned")?),
        FieldValue::U16(_) => FieldValue::U16(text.parse().map_err(|_| "not an unsigned")?),
        FieldValue::U32(_) => FieldValue::U32(text.parse().map_err(|_| "not an unsigned")?),
        FieldValue::U64(_) => FieldValue::U64(text.parse().map_err(|_| "not an unsigned")?),
        FieldValue::Usize(_) => FieldValue::Usize(text.parse().map_err(|_| "not an unsigned")?),
        FieldValue::Isize(_) => FieldValue::Isize(text.parse().map_err(|_| "not an integer")?),
        FieldValue::Bool(_) => FieldValue::Bool(text.parse().map_err(|_| "not a boolean")?),
        FieldValue::Text(_) => FieldValue::Text(text.to_string()),
        // An enum takes a variant's name; its discriminant is accepted too.
        FieldValue::Enum { type_tag, .. } => {
            let enum_tag = pill_engine::parse_enum_tag(type_tag)
                .ok_or_else(|| format!("`{type_tag}` is not a readable enum tag"))?;
            let discriminant = enum_tag
                .discriminant_of(text)
                .or_else(|| {
                    text.parse::<i64>()
                        .ok()
                        .filter(|number| enum_tag.variant_name(*number).is_some())
                })
                .ok_or_else(|| {
                    let names: Vec<&str> =
                        enum_tag.variants.iter().map(|(name, _)| *name).collect();
                    format!(
                        "`{text}` is not a variant of `{}` ({})",
                        enum_tag.type_name,
                        names.join(", ")
                    )
                })?;
            FieldValue::Enum {
                type_tag,
                discriminant,
            }
        }
        other => return Err(format!("`{other:?}` fields are not editable here")),
    };
    Ok(parsed)
}

/// One field value in the text form the Inspector's widgets show and commit:
/// what a module panel displays and what it writes back through
/// `PanelCommand::SetField`.
pub fn value_text(value: &FieldValue) -> String {
    match value {
        FieldValue::F32(value) => format!("{value}"),
        FieldValue::F64(value) => format!("{value}"),
        FieldValue::I8(value) => format!("{value}"),
        FieldValue::I16(value) => format!("{value}"),
        FieldValue::I32(value) => format!("{value}"),
        FieldValue::I64(value) => format!("{value}"),
        FieldValue::U8(value) => format!("{value}"),
        FieldValue::U16(value) => format!("{value}"),
        FieldValue::U32(value) => format!("{value}"),
        FieldValue::U64(value) => format!("{value}"),
        FieldValue::Usize(value) => format!("{value}"),
        FieldValue::Isize(value) => format!("{value}"),
        FieldValue::Bool(value) => format!("{value}"),
        FieldValue::Text(value) => value.clone(),
        FieldValue::Array(elements) => format!("[{}]", elements.len()),
        FieldValue::List { element_count, .. } => format!("[{element_count}]"),
        FieldValue::Enum { discriminant, .. } => value
            .enum_variant_name()
            .map_or_else(|| discriminant.to_string(), str::to_string),
        other => format!("{other:?}"),
    }
}

// =============================================================================
// Settings documents
// =============================================================================

/// One segment of a settings key path: an object key or an array index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsKey {
    /// A key of a JSON object.
    Key(String),
    /// An index of a JSON array.
    Index(usize),
}

/// Parse a settings key path: dotted object keys with `[i]` array indexes,
/// like `waves[0].amplitude`.
///
/// # Errors
///
/// A message naming the text when a key is empty, a bracket is unbalanced, an
/// index is not a number, or the path names no key at all.
pub fn parse_settings_key_path(text: &str) -> Result<Vec<SettingsKey>, String> {
    let mut path = Vec::new();
    let mut key = String::new();
    let mut characters = text.chars().peekable();
    // Step 1: Read the path one character at a time. A dot or a bracket ends
    // the key being read; a bracket also starts an index.
    while let Some(character) = characters.next() {
        match character {
            '.' => {
                if key.is_empty() {
                    // A dot right after an index is a separator (`a[0].b`);
                    // anywhere else it has no key to end.
                    match path.last() {
                        Some(SettingsKey::Index(_)) => {}
                        _ => return Err(format!("`{text}` has an empty key")),
                    }
                } else {
                    path.push(SettingsKey::Key(std::mem::take(&mut key)));
                }
            }
            '[' => {
                if !key.is_empty() {
                    path.push(SettingsKey::Key(std::mem::take(&mut key)));
                }
                let mut index_text = String::new();
                let mut closed = false;
                for index_character in characters.by_ref() {
                    if index_character == ']' {
                        closed = true;
                        break;
                    }
                    index_text.push(index_character);
                }
                // A bracket that never closes is an unbalanced `[`, not an
                // index. Accepting it would edit a leaf the person did not
                // name.
                if !closed {
                    return Err(format!("`{text}` has an unbalanced `[`"));
                }
                let index: usize = index_text
                    .parse()
                    .map_err(|_| format!("`{text}` has a bad index `[{index_text}]`"))?;
                path.push(SettingsKey::Index(index));
            }
            ']' => return Err(format!("`{text}` has an unbalanced `]`")),
            other => key.push(other),
        }
    }
    // Step 2: Read the key the input ended on, then refuse a path that names
    // no key at all.
    if !key.is_empty() {
        path.push(SettingsKey::Key(key));
    }
    if path.is_empty() {
        return Err(format!("`{text}` names no key"));
    }
    Ok(path)
}

/// The text form of a key path: what labels and coalescing keys use.
pub fn settings_key_text(path: &[SettingsKey]) -> String {
    let mut text = String::new();
    for segment in path {
        match segment {
            SettingsKey::Key(key) => {
                if !text.is_empty() {
                    text.push('.');
                }
                text.push_str(key);
            }
            SettingsKey::Index(index) => {
                text.push('[');
                text.push_str(&index.to_string());
                text.push(']');
            }
        }
    }
    text
}

/// The settings leaf a key path names, when the document holds it.
pub fn settings_field<'a>(document: &'a JsonValue, path: &[SettingsKey]) -> Option<&'a JsonValue> {
    let mut current = document;
    for segment in path {
        current = match segment {
            SettingsKey::Key(key) => current.get(key)?,
            SettingsKey::Index(index) => current.get(index)?,
        };
    }
    Some(current)
}

/// Replace the settings leaf a key path names.
///
/// The containers along the way must already exist, because the editors only
/// edit what they displayed.
///
/// # Errors
///
/// A message naming the segment the document does not hold, or saying that the
/// path is empty.
pub fn patch_settings(
    document: &mut JsonValue,
    path: &[SettingsKey],
    value: JsonValue,
) -> Result<(), String> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| "a settings key path cannot be empty".to_string())?;
    let mut current = document;
    // Step 1: Walk to the leaf's parent, refusing a segment the document lacks.
    for segment in parents {
        current = match segment {
            SettingsKey::Key(key) => current
                .get_mut(key)
                .ok_or_else(|| format!("`{key}` is not in the settings"))?,
            SettingsKey::Index(index) => current
                .get_mut(index)
                .ok_or_else(|| format!("element [{index}] is not in the settings"))?,
        };
    }
    // Step 2: Write the leaf itself.
    let slot = match last {
        SettingsKey::Key(key) => current
            .get_mut(key)
            .ok_or_else(|| format!("`{key}` is not in the settings"))?,
        SettingsKey::Index(index) => current
            .get_mut(index)
            .ok_or_else(|| format!("element [{index}] is not in the settings"))?,
    };
    *slot = value;
    Ok(())
}

/// Replace the settings leaf a key path names, creating what is missing on the
/// way.
///
/// This follows the asset-settings rule. A slot or a reference row is drawn
/// from what the *type* declares, so the file may not hold the key yet - an
/// unbound texture slot is exactly that - and the first write is the one that
/// creates it. A missing parent object is created, and an array index one past
/// the end appends. [`patch_settings`] is the strict twin: it fails unless every
/// segment the path names is already in the document.
///
/// # Errors
///
/// A message naming the segment it could not reach: a parent that is not an
/// object or an array, an index past the end of an array, or an empty path.
pub fn patch_settings_inserting(
    document: &mut JsonValue,
    path: &[SettingsKey],
    value: JsonValue,
) -> Result<(), String> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| "a settings key path cannot be empty".to_string())?;
    let mut current = document;
    // Step 1: Walk the parents, creating a missing object on the way.
    for (position, segment) in parents.iter().enumerate() {
        current = match segment {
            SettingsKey::Key(key) => {
                if !current.is_object() {
                    return Err(format!("`{key}` is not in the settings"));
                }
                let object = current
                    .as_object_mut()
                    .ok_or_else(|| format!("`{key}` is not in the settings"))?;
                object
                    .entry(key.clone())
                    .or_insert_with(|| JsonValue::Object(serde_json::Map::new()))
            }
            SettingsKey::Index(index) => {
                // An index one past the end appends, but only when the element
                // it creates is an object: a path that would need a fresh array
                // is a gap, not an append.
                let creates_object = matches!(parents.get(position + 1), Some(SettingsKey::Key(_)))
                    || matches!(last, SettingsKey::Key(_));
                let elements = current
                    .as_array_mut()
                    .ok_or_else(|| format!("element [{index}] is not in the settings"))?;
                if *index == elements.len() && creates_object {
                    elements.push(JsonValue::Object(serde_json::Map::new()));
                }
                elements
                    .get_mut(*index)
                    .ok_or_else(|| format!("element [{index}] is not in the settings"))?
            }
        };
    }
    // Step 2: Write the last segment, appending to an array when the index is
    // one past its end.
    match last {
        SettingsKey::Key(key) => {
            let object = current
                .as_object_mut()
                .ok_or_else(|| format!("`{key}` is not in the settings"))?;
            object.insert(key.clone(), value);
        }
        SettingsKey::Index(index) => match current {
            JsonValue::Array(elements) if *index == elements.len() => elements.push(value),
            _ => {
                let slot = current
                    .get_mut(*index)
                    .ok_or_else(|| format!("element [{index}] is not in the settings"))?;
                *slot = value;
            }
        },
    }
    Ok(())
}

/// Parse an edited text back into the JSON kind `current` already holds.
///
/// # Errors
///
/// A message saying the text is not a boolean, an integer or a number of the
/// current kind, that a number is not finite, or that the document's kind has
/// no editor here.
pub fn parse_settings_value(text: &str, current: &JsonValue) -> Result<JsonValue, String> {
    let text = text.trim();
    let parsed = match current {
        JsonValue::Bool(_) => JsonValue::Bool(text.parse().map_err(|_| "not a boolean")?),
        JsonValue::Number(number) => {
            if number.is_i64() || number.is_u64() {
                let value: i64 = text.parse().map_err(|_| "not an integer")?;
                JsonValue::Number(value.into())
            } else {
                let value: f64 = text.parse().map_err(|_| "not a number")?;
                JsonValue::Number(serde_json::Number::from_f64(value).ok_or("not a finite number")?)
            }
        }
        JsonValue::String(_) => JsonValue::String(text.to_string()),
        other => return Err(format!("`{other}` settings are not editable here")),
    };
    Ok(parsed)
}

/// The recorded settings edit: `SetAssetSettings` under the label the
/// history and the suite read.
pub fn settings_edit(
    asset_path: &str,
    settings_document: JsonValue,
    key_text: &str,
) -> (EditorCommand, String) {
    (
        EditorCommand::SetAssetSettings {
            path: asset_path.to_string(),
            settings: settings_document,
        },
        format!("set asset {key_text}"),
    )
}

/// The continuous-edit key one settings widget's commits share.
pub fn settings_coalesce_key(asset_path: &str, key_text: &str) -> String {
    format!("inspector.asset.{asset_path}.{key_text}")
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_state::AssetKind;
    use serde_json::json;

    /// A detail whose components are named by their short names, enough to
    /// answer which entities carry a component.
    fn detail(entity: Entity, component_names: &[&str]) -> EntityDetail {
        EntityDetail {
            entity,
            components: component_names
                .iter()
                .map(|name| ComponentView {
                    type_name: name.to_string(),
                    persistable: true,
                    editable: true,
                    fields: Vec::new(),
                    values: Vec::new(),
                    asset_labels: Vec::new(),
                    entity_labels: Vec::new(),
                })
                .collect(),
        }
    }

    /// A multi-selection edits the entities that carry the component, and skips
    /// the ones that do not.
    #[test]
    fn selection_targets_skip_entities_without_the_component() {
        let primary = Entity::from_raw(1, 0);
        let carrier = Entity::from_raw(2, 0);
        let other = Entity::from_raw(3, 0);
        let details = vec![
            detail(primary, &["Transform"]),
            detail(carrier, &["Transform", "Health"]),
            detail(other, &["Health"]),
        ];
        assert_eq!(
            selection_targets(&details, primary, "Transform"),
            vec![primary, carrier]
        );
        assert_eq!(
            selection_targets(&details, primary, "Health"),
            vec![primary, carrier, other]
        );
    }

    /// A component view with one scalar field of the given kind.
    fn single_field_view(value: FieldValue) -> ComponentView {
        ComponentView {
            type_name: "TransformComponent".to_string(),
            persistable: true,
            editable: true,
            fields: vec![crate::editor_state::ComponentFieldView {
                name: "speed".to_string(),
                decorators: Vec::new(),
            }],
            values: vec![value],
            asset_labels: Vec::new(),
            entity_labels: Vec::new(),
        }
    }

    /// The row model is rebuilt on a layout change, not on a value edit.
    ///
    /// The stamp reads kinds and lengths, and the cache keeps its rows - with
    /// the lines they already logged - across a frame that only changed a
    /// value.
    #[test]
    fn the_row_model_rebuilds_on_layout_not_on_values() {
        let mut view = single_field_view(FieldValue::F32(1.0));
        let layout = row_layout(&view);
        view.values[0] = FieldValue::F32(2.5);
        assert_eq!(
            row_layout(&view),
            layout,
            "a value edit must not rebuild the row model"
        );
        view.values[0] = FieldValue::Bool(true);
        assert_ne!(
            row_layout(&view),
            layout,
            "a value kind change must rebuild it"
        );

        let mut session = InspectorSession::default();
        let primary = Entity::from_raw(1, 0);
        let value_view = single_field_view(FieldValue::F32(1.0));
        let (mut rows, targets) = take_component_rows(&mut session, &value_view, &[], primary);
        assert_eq!(rows.len(), 1);
        assert_eq!(targets, vec![primary]);
        assert!(!rows[0].logged.has(RowLogged::ROW));
        rows[0].logged.mark(RowLogged::ROW);
        store_component_rows(&mut session, &value_view.type_name, rows, targets);

        let (rows, targets) = take_component_rows(&mut session, &value_view, &[], primary);
        assert!(
            rows[0].logged.has(RowLogged::ROW),
            "the cached row keeps the line it already logged"
        );
        store_component_rows(&mut session, &value_view.type_name, rows, targets);

        let mut edited = value_view.clone();
        edited.values[0] = FieldValue::F32(9.0);
        let (rows, targets) = take_component_rows(&mut session, &edited, &[], primary);
        assert!(
            rows[0].logged.has(RowLogged::ROW),
            "a value edit kept the rows"
        );
        store_component_rows(&mut session, &edited.type_name, rows, targets);

        edited.values[0] = FieldValue::Bool(false);
        let (rows, _) = take_component_rows(&mut session, &edited, &[], primary);
        assert!(
            !rows[0].logged.has(RowLogged::ROW),
            "a layout change logs the new row"
        );
    }

    /// One row's commit over a multi-selection is one recorded batch: one
    /// command per target, named and coalesced by the primary.
    #[test]
    fn a_selection_field_edit_batches_every_target() {
        let primary = Entity::from_raw(1, 0);
        let second = Entity::from_raw(2, 0);
        let (commands, label, key) = selection_field_edit(
            &[primary, second],
            primary,
            "Transform",
            "speed",
            FieldValue::F32(2.5),
        );
        assert_eq!(commands.len(), 2);
        assert_eq!(label, "set Transform.speed");
        assert_eq!(key, field_coalesce_key(primary, "Transform", "speed"));
        let entities: Vec<Entity> = commands
            .iter()
            .map(|command| match command {
                EditorCommand::SetField { entity, .. } => *entity,
                other => panic!("a selection edit queued {other:?}"),
            })
            .collect();
        assert_eq!(entities, vec![primary, second]);
    }

    #[test]
    fn labels_shorten_the_component_name() {
        let entity = Entity::from_raw(3, 0);
        let (command, label) = field_edit(
            entity,
            "pill_master_renderer::component::TransformComponent",
            "speed",
            FieldValue::F32(1.0),
        );
        assert_eq!(label, "set TransformComponent.speed");
        assert!(matches!(command, EditorCommand::SetField { .. }));
        let (_, label) = element_edit(
            entity,
            "pill_master_renderer::component::TransformComponent",
            "translation",
            2,
            FieldValue::F32(1.0),
        );
        assert_eq!(label, "set TransformComponent[2].translation");
        assert_eq!(
            field_coalesce_key(entity, "Component", "speed"),
            format!("inspector.{entity}.Component.speed")
        );
    }

    #[test]
    fn parse_follows_the_current_kind() {
        assert_eq!(
            parse_field_value(" 4.25 ", &FieldValue::F32(0.0)),
            Ok(FieldValue::F32(4.25))
        );
        assert!(parse_field_value("true", &FieldValue::F32(0.0)).is_err());
        assert_eq!(
            parse_field_value("true", &FieldValue::Bool(false)),
            Ok(FieldValue::Bool(true))
        );
        assert_eq!(
            parse_field_value("hello world", &FieldValue::Text(String::new())),
            Ok(FieldValue::Text("hello world".to_string()))
        );
        assert!(parse_field_value("1", &FieldValue::Array(Vec::new())).is_err());
        // Every kind the widget table calls an Integer parses: a drawn integer
        // field must accept its own edit.
        let integers = [
            (FieldValue::I8(0), FieldValue::I8(7)),
            (FieldValue::I16(0), FieldValue::I16(7)),
            (FieldValue::I32(0), FieldValue::I32(7)),
            (FieldValue::I64(0), FieldValue::I64(7)),
            (FieldValue::U8(0), FieldValue::U8(7)),
            (FieldValue::U16(0), FieldValue::U16(7)),
            (FieldValue::U32(0), FieldValue::U32(7)),
            (FieldValue::U64(0), FieldValue::U64(7)),
            (FieldValue::Usize(0), FieldValue::Usize(7)),
            (FieldValue::Isize(0), FieldValue::Isize(7)),
        ];
        for (sample, expected) in integers {
            assert_eq!(
                parse_field_value("7", &sample),
                Ok(expected),
                "an integer field the widget table draws must accept its own edit"
            );
        }
    }

    /// The write paths share one decision: an element write coalesces on the
    /// element, and a field write handles the reference kinds.
    ///
    /// A module panel therefore reaches them the way the Inspector does.
    #[test]
    fn every_write_path_coalesces_and_branches_the_same() {
        let entity = Entity::from_raw(1, 0);
        let array = FieldValue::Array(vec![FieldValue::F32(1.0), FieldValue::F32(2.0)]);
        let first = element_write(entity, "Demo", "path", &array, 0, "5.0").expect("writes");
        let second = element_write(entity, "Demo", "path", &array, 1, "5.0").expect("writes");
        assert_ne!(
            first.coalesce, second.coalesce,
            "two elements of one array are two undo steps"
        );
        assert!(first.coalesce.ends_with("path[0]"), "{}", first.coalesce);
        assert!(element_write(entity, "Demo", "path", &array, 9, "1.0").is_err());

        // An asset reference takes a path, an entity reference a label (or the
        // word that clears it), and a scalar its own text.
        let asset = FieldValue::Asset {
            asset_type: None,
            index: 0,
            generation: 0,
        };
        let written =
            field_write(entity, "Demo", "mesh", &asset, "textures/a.png").expect("writes");
        assert!(matches!(
            written.commands[0],
            EditorCommand::SetFieldAsset { .. }
        ));
        let reference = FieldValue::Entity {
            id: 0,
            generation: 0,
        };
        let cleared = field_write(entity, "Demo", "target", &reference, "none").expect("writes");
        assert!(matches!(
            cleared.commands[0],
            EditorCommand::SetFieldEntity {
                reference: None,
                ..
            }
        ));
        assert!(field_write(entity, "Demo", "speed", &FieldValue::F32(0.0), "fast").is_err());
    }

    #[test]
    fn element_field_reads_arrays_only() {
        let value = FieldValue::Array(vec![
            FieldValue::F32(1.0),
            FieldValue::F32(2.0),
            FieldValue::F32(3.0),
        ]);
        assert_eq!(element_field(&value, 1), Some(FieldValue::F32(2.0)));
        assert_eq!(element_field(&value, 9), None);
        assert_eq!(element_field(&FieldValue::F32(1.0), 0), None);
    }

    #[test]
    fn key_paths_parse_and_render() {
        let path = parse_settings_key_path("waves[0].amplitude").expect("parses");
        assert_eq!(
            path,
            vec![
                SettingsKey::Key("waves".to_string()),
                SettingsKey::Index(0),
                SettingsKey::Key("amplitude".to_string()),
            ]
        );
        assert_eq!(settings_key_text(&path), "waves[0].amplitude");
        assert!(parse_settings_key_path("").is_err());
        assert!(parse_settings_key_path("a..b").is_err());
        assert!(parse_settings_key_path("a[x]").is_err());
        // An unterminated index and a stray bracket are both refused.
        assert!(parse_settings_key_path("waves[0").is_err());
        assert!(parse_settings_key_path("waves[").is_err());
        assert!(parse_settings_key_path("waves]").is_err());
    }

    #[test]
    fn settings_leaves_read_and_patch() {
        let mut document = json!({
            "flip_v": false,
            "waves": [{ "amplitude": 1.0 }],
        });
        let path = parse_settings_key_path("waves[0].amplitude").expect("parses");
        assert_eq!(
            settings_field(&document, &path),
            Some(&JsonValue::from(1.0))
        );
        patch_settings(&mut document, &path, json!(2.5)).expect("patches");
        assert_eq!(
            settings_field(&document, &path),
            Some(&JsonValue::from(2.5))
        );
        patch_settings(&mut document, &path, json!(7)).expect("patches");
        assert_eq!(settings_field(&document, &path), Some(&json!(7)));
        let missing = parse_settings_key_path("nope").expect("parses");
        assert!(patch_settings(&mut document, &missing, json!(1)).is_err());
    }

    /// The inserting patch follows the asset-settings rule: it creates a slot
    /// that the type declares but the file does not hold yet, with the parent
    /// it needs, while the strict patch keeps refusing the same path.
    #[test]
    fn the_inserting_patch_creates_the_leaf_the_file_lacks() {
        let mut document = json!({ "textures": { "albedo": "aa" } });
        let slot = parse_settings_key_path("textures.base_color").expect("parses");
        assert!(patch_settings(&mut document, &slot, json!("bb")).is_err());
        patch_settings_inserting(&mut document, &slot, json!("bb")).expect("inserts");
        assert_eq!(settings_field(&document, &slot), Some(&json!("bb")));
        // The sibling the file already held is untouched.
        assert_eq!(
            settings_field(&document, &slot[..1]),
            Some(&json!({ "albedo": "aa", "base_color": "bb" }))
        );

        // A missing parent object is made, not refused.
        let mut empty = json!({});
        let deep = parse_settings_key_path("textures.normal").expect("parses");
        patch_settings_inserting(&mut empty, &deep, json!("cc")).expect("creates the parent");
        assert_eq!(settings_field(&empty, &deep), Some(&json!("cc")));

        // One past the end of an array appends; further past it is refused.
        let mut list = json!({ "passes": [{ "guid": "a" }] });
        let append = parse_settings_key_path("passes[1].guid").expect("parses");
        patch_settings_inserting(&mut list, &append, json!("b")).expect("appends");
        assert_eq!(list["passes"][1]["guid"], json!("b"));
        let gap = parse_settings_key_path("passes[5].guid").expect("parses");
        assert!(patch_settings_inserting(&mut list, &gap, json!("c")).is_err());
    }

    #[test]
    fn settings_values_parse_following_the_current_kind() {
        assert_eq!(parse_settings_value("true", &json!(false)), Ok(json!(true)));
        assert_eq!(parse_settings_value("2", &json!(1)), Ok(json!(2)));
        assert_eq!(parse_settings_value("2.5", &json!(1.0)), Ok(json!(2.5)));
        assert_eq!(
            parse_settings_value("Normal", &json!("Color")),
            Ok(json!("Normal"))
        );
        assert!(parse_settings_value("2", &json!("Color")).is_ok());
        assert!(parse_settings_value("nope", &json!(1)).is_err());
        assert!(parse_settings_value("1", &json!({"a": 1})).is_err());
    }

    #[test]
    fn settings_edits_label_and_coalesce() {
        let (command, label) = settings_edit(
            "textures/rock.png",
            json!({"texture_type": "Normal"}),
            "texture_type",
        );
        assert_eq!(label, "set asset texture_type");
        assert!(matches!(command, EditorCommand::SetAssetSettings { .. }));
        assert_eq!(
            settings_coalesce_key("textures/rock.png", "texture_type"),
            "inspector.asset.textures/rock.png.texture_type"
        );
    }

    #[test]
    fn component_add_and_remove_record_the_native_labels() {
        let entity = Entity::from_raw(3, 0);
        let (add, add_label) = component_add(
            entity,
            "pill_master_renderer::component::TransformComponent",
        );
        assert_eq!(add_label, "add TransformComponent");
        assert!(matches!(add, EditorCommand::AddComponent { .. }));
        let (remove, remove_label) =
            component_remove(entity, "italian_brainrot::TagAlphaComponent");
        assert_eq!(remove_label, "remove TagAlphaComponent");
        assert!(matches!(remove, EditorCommand::RemoveComponent { .. }));
    }

    /// A component view over a field/value pair list.
    fn component(type_name: &str, values: Vec<FieldValue>) -> ComponentView {
        /// Field names for the fixture (the descriptor's name is a `&'static
        /// str`), indexed by the value's position.
        const NAMES: [&str; 8] = ["a", "b", "c", "d", "e", "f", "g", "h"];
        ComponentView {
            asset_labels: Vec::new(),
            entity_labels: Vec::new(),
            type_name: type_name.to_string(),
            persistable: false,
            editable: true,
            fields: values
                .iter()
                .enumerate()
                .map(|(index, _)| crate::editor_state::ComponentFieldView {
                    name: NAMES[index].to_string(),
                    decorators: Vec::new(),
                })
                .collect(),
            values,
        }
    }

    #[test]
    fn the_widget_follows_the_value_kind() {
        assert_eq!(widget_for(&FieldValue::Bool(true)), FieldWidget::Checkbox);
        assert_eq!(widget_for(&FieldValue::F32(1.0)), FieldWidget::Float);
        assert_eq!(widget_for(&FieldValue::F64(1.0)), FieldWidget::Float);
        for value in [
            FieldValue::I8(1),
            FieldValue::I16(1),
            FieldValue::I32(1),
            FieldValue::I64(1),
            FieldValue::U8(1),
            FieldValue::U16(1),
            FieldValue::U32(1),
            FieldValue::U64(1),
            FieldValue::Usize(1),
            FieldValue::Isize(1),
        ] {
            assert_eq!(widget_for(&value), FieldWidget::Integer);
        }
        // A heap-backed string is read-only through the field layer, so the
        // inspector offers no box that could only refuse its keystrokes.
        assert_eq!(
            widget_for(&FieldValue::Text("x".to_string())),
            FieldWidget::ReadOnly
        );
        assert_eq!(
            widget_for(&FieldValue::Opaque {
                type_tag: "struct:x",
                bytes: vec![0; 4]
            }),
            FieldWidget::ReadOnly
        );
    }

    #[test]
    fn a_short_scalar_array_becomes_one_vector_row() {
        let transform = component(
            "TransformComponent",
            vec![
                FieldValue::Array(vec![
                    FieldValue::F32(1.0),
                    FieldValue::F32(2.0),
                    FieldValue::F32(3.0),
                ]),
                FieldValue::Bool(true),
            ],
        );
        assert_eq!(
            rows(&transform),
            vec![
                FieldRow::Vector {
                    index: 0,
                    field: "a".to_string(),
                    widget: FieldWidget::Float,
                    elements: 3,
                    tooltip: None,
                },
                FieldRow::Single {
                    index: 1,
                    field: "b".to_string(),
                    widget: FieldWidget::Checkbox,
                    tooltip: None,
                },
            ]
        );
    }

    #[test]
    fn a_long_or_non_scalar_array_stays_read_only() {
        let long = component(
            "Big",
            vec![FieldValue::Array(
                (0..8).map(|index| FieldValue::F32(index as f32)).collect(),
            )],
        );
        assert_eq!(
            rows(&long),
            vec![FieldRow::ReadOnly {
                index: 0,
                field: "a".to_string(),
                tooltip: None,
            }]
        );
        let nested = component(
            "Nested",
            vec![FieldValue::Array(vec![
                FieldValue::Array(vec![FieldValue::F32(1.0), FieldValue::F32(2.0)]),
                FieldValue::Array(vec![FieldValue::F32(3.0), FieldValue::F32(4.0)]),
            ])],
        );
        assert_eq!(
            rows(&nested),
            vec![FieldRow::ReadOnly {
                index: 0,
                field: "a".to_string(),
                tooltip: None,
            }]
        );
        let list = component(
            "List",
            vec![FieldValue::List {
                element_tag: "f32",
                element_count: 3,
            }],
        );
        assert_eq!(
            rows(&list),
            vec![FieldRow::ReadOnly {
                index: 0,
                field: "a".to_string(),
                tooltip: None,
            }]
        );
    }

    #[test]
    fn a_two_element_array_is_a_vector_too() {
        let value = component(
            "Vec2-ish",
            vec![FieldValue::Array(vec![
                FieldValue::I32(1),
                FieldValue::I32(2),
            ])],
        );
        assert_eq!(
            rows(&value),
            vec![FieldRow::Vector {
                index: 0,
                field: "a".to_string(),
                widget: FieldWidget::Integer,
                elements: 2,
                tooltip: None,
            }]
        );
    }

    /// A component whose fields carry the decorations at their position.
    fn decorated(values: Vec<FieldValue>, decorators: Vec<Vec<FieldDecorator>>) -> ComponentView {
        /// Field names for the fixture (the descriptor's name is a `&'static
        /// str`), indexed by the value's position.
        const NAMES: [&str; 8] = ["a", "b", "c", "d", "e", "f", "g", "h"];
        ComponentView {
            asset_labels: Vec::new(),
            entity_labels: Vec::new(),
            type_name: "Decorated".to_string(),
            persistable: false,
            editable: true,
            fields: values
                .iter()
                .enumerate()
                .map(|(index, _)| crate::editor_state::ComponentFieldView {
                    name: NAMES[index].to_string(),
                    decorators: decorators[index].clone(),
                })
                .collect(),
            values,
        }
    }

    #[test]
    fn header_and_space_decorators_become_rows_above_their_field() {
        let view = decorated(
            vec![FieldValue::F32(1.0), FieldValue::Bool(true)],
            vec![
                vec![
                    FieldDecorator::Header("Timing".into()),
                    FieldDecorator::Space(10.0),
                ],
                Vec::new(),
            ],
        );
        assert_eq!(
            rows(&view),
            vec![
                FieldRow::Header {
                    text: "Timing".to_string(),
                },
                FieldRow::Space { pixels: 10.0 },
                FieldRow::Single {
                    index: 0,
                    field: "a".to_string(),
                    widget: FieldWidget::Float,
                    tooltip: None,
                },
                FieldRow::Single {
                    index: 1,
                    field: "b".to_string(),
                    widget: FieldWidget::Checkbox,
                    tooltip: None,
                },
            ]
        );
    }

    #[test]
    fn a_hidden_field_takes_its_header_and_gap_with_it() {
        let view = decorated(
            vec![FieldValue::F32(1.0), FieldValue::F32(2.0)],
            vec![
                vec![
                    FieldDecorator::Header("Hidden".into()),
                    FieldDecorator::HideInInspector,
                ],
                vec![FieldDecorator::Tooltip("Kept".into())],
            ],
        );
        assert_eq!(
            rows(&view),
            vec![FieldRow::Single {
                index: 1,
                field: "b".to_string(),
                widget: FieldWidget::Float,
                tooltip: Some("Kept".to_string()),
            }]
        );
    }

    #[test]
    fn a_range_decorator_picks_the_slider_for_the_value_kind() {
        let view = decorated(
            vec![FieldValue::F32(50.0), FieldValue::I32(5)],
            vec![
                vec![FieldDecorator::Range {
                    min: 0.0,
                    max: 100.0,
                }],
                vec![FieldDecorator::Range {
                    min: 0.0,
                    max: 100.0,
                }],
            ],
        );
        assert_eq!(
            rows(&view),
            vec![
                FieldRow::Single {
                    index: 0,
                    field: "a".to_string(),
                    widget: FieldWidget::FloatSlider {
                        min: 0.0,
                        max: 100.0,
                    },
                    tooltip: None,
                },
                FieldRow::Single {
                    index: 1,
                    field: "b".to_string(),
                    widget: FieldWidget::IntegerSlider {
                        min: 0.0,
                        max: 100.0,
                    },
                    tooltip: None,
                },
            ]
        );
    }

    #[test]
    fn a_minimum_decorator_clamps_the_drag_value() {
        let view = decorated(
            vec![FieldValue::F32(1.0), FieldValue::I32(1)],
            vec![
                vec![FieldDecorator::Min(0.0)],
                vec![FieldDecorator::Min(-5.0)],
            ],
        );
        assert_eq!(
            rows(&view),
            vec![
                FieldRow::Single {
                    index: 0,
                    field: "a".to_string(),
                    widget: FieldWidget::FloatDrag { min: 0.0 },
                    tooltip: None,
                },
                FieldRow::Single {
                    index: 1,
                    field: "b".to_string(),
                    widget: FieldWidget::IntegerDrag { min: -5.0 },
                    tooltip: None,
                },
            ]
        );
    }

    /// `[TextArea]` and `[Multiline]` shape a text box, but a text row has no
    /// box to shape until heap-backed fields become writable. The row stays
    /// read-only, and the decorators are ignored instead of promising an edit
    /// the field writer refuses.
    #[test]
    fn text_decorators_wait_for_a_writable_text_row() {
        let view = decorated(
            vec![FieldValue::Text("a".into()), FieldValue::Text("b".into())],
            vec![
                vec![FieldDecorator::TextArea {
                    min_lines: 3,
                    max_lines: 10,
                }],
                vec![FieldDecorator::Multiline(5)],
            ],
        );
        assert_eq!(
            rows(&view),
            vec![
                FieldRow::Single {
                    index: 0,
                    field: "a".to_string(),
                    widget: FieldWidget::ReadOnly,
                    tooltip: None,
                },
                FieldRow::Single {
                    index: 1,
                    field: "b".to_string(),
                    widget: FieldWidget::ReadOnly,
                    tooltip: None,
                },
            ]
        );
    }

    #[test]
    fn decorators_that_cannot_apply_leave_the_widget_alone() {
        let numeric = decorated(
            vec![FieldValue::F32(1.0), FieldValue::Text("x".into())],
            vec![
                vec![FieldDecorator::TextArea {
                    min_lines: 3,
                    max_lines: 3,
                }],
                vec![FieldDecorator::Range { min: 0.0, max: 1.0 }],
            ],
        );
        assert_eq!(
            rows(&numeric),
            vec![
                FieldRow::Single {
                    index: 0,
                    field: "a".to_string(),
                    widget: FieldWidget::Float,
                    tooltip: None,
                },
                FieldRow::Single {
                    index: 1,
                    field: "b".to_string(),
                    widget: FieldWidget::ReadOnly,
                    tooltip: None,
                },
            ]
        );
        // A range belongs to one scalar: a vector row keeps the plain element
        // widget and the tooltip.
        let vector = decorated(
            vec![FieldValue::Array(vec![
                FieldValue::F32(1.0),
                FieldValue::F32(2.0),
            ])],
            vec![vec![
                FieldDecorator::Range { min: 0.0, max: 1.0 },
                FieldDecorator::Tooltip("Vec".into()),
            ]],
        );
        assert_eq!(
            rows(&vector),
            vec![FieldRow::Vector {
                index: 0,
                field: "a".to_string(),
                widget: FieldWidget::Float,
                elements: 2,
                tooltip: Some("Vec".to_string()),
            }]
        );
    }

    #[test]
    fn labels_drop_the_component_suffix_and_the_module_path() {
        assert_eq!(
            component_label("pill_master_renderer::component::TransformComponent"),
            "Transform"
        );
        assert_eq!(component_label("TransformComponent"), "Transform");
        assert_eq!(
            component_label("italian_brainrot::TagAlphaComponent"),
            "TagAlpha"
        );
        assert_eq!(component_label("pill_spline::Spline"), "Spline");
        assert_eq!(component_label("Weird"), "Weird");
    }

    #[test]
    fn icons_follow_the_component_name() {
        assert_eq!(
            component_icon("x::TransformComponent"),
            Icon::ArrowsOutCardinal
        );
        assert_eq!(component_icon("x::MeshRendererComponent"), Icon::Cube);
        assert_eq!(component_icon("x::BoxCollider"), Icon::CubeTransparent);
        assert_eq!(component_icon("x::SpriteComponent"), Icon::Image);
        assert_eq!(component_icon("x::TextureAsset"), Icon::Image);
        assert_eq!(component_icon("x::MaterialComponent"), Icon::Sphere);
        assert_eq!(component_icon("x::DirectionalLightComponent"), Icon::Power);
        assert_eq!(component_icon("x::CameraComponent"), Icon::MonitorPlay);
        assert_eq!(component_icon("x::AudioSource"), Icon::Play);
        assert_eq!(component_icon("x::HealthBehaviour"), Icon::Code);
        assert_eq!(component_icon("x::SomeScript"), Icon::Code);
        assert_eq!(component_icon("x::RigidbodyBody"), Icon::ArrowsOut);
        assert_eq!(component_icon("x::UnknownThing"), Icon::SlidersHorizontal);
    }

    #[test]
    fn vector_axes_are_named_and_coloured_like_the_gizmo_guides() {
        assert_eq!(
            (0..4).map(axis_label).collect::<Vec<_>>(),
            vec!["X", "Y", "Z", "W"]
        );
        let red = axis_color(0);
        let green = axis_color(1);
        let blue = axis_color(2);
        assert!(red.r() > red.g() && red.r() > red.b(), "X is red-ish");
        assert!(
            green.g() > green.r() && green.g() > green.b(),
            "Y is green-ish"
        );
        assert!(blue.b() > blue.r() && blue.b() > blue.g(), "Z is blue-ish");
        assert_eq!(axis_color(3), Color32::GRAY);
    }

    /// An asset value becomes an asset row carrying the field's asset type,
    /// not a read-only row.
    #[test]
    fn an_asset_value_is_an_asset_row() {
        let view = component(
            "demo::Renderer",
            vec![FieldValue::Asset {
                asset_type: Some("demo::Mesh".to_owned()),
                index: 0,
                generation: 0,
            }],
        );
        assert_eq!(
            rows(&view),
            vec![FieldRow::Asset {
                index: 0,
                field: "a".to_owned(),
                asset_type: Some("demo::Mesh".to_owned()),
                tooltip: None,
            }]
        );
    }

    /// The picker offers the listing's files of the field's type only, and
    /// nothing for a field whose type is not recorded.
    #[test]
    fn asset_candidates_are_the_files_of_the_fields_type() {
        let listing = |path: &str, kind: &str| AssetListing {
            root: String::new(),
            path: path.to_owned(),
            read_only: false,
            kind: AssetKind::parse(kind),
            standalone: false,
            has_meta: true,
            guid: None,
        };
        let assets = vec![
            listing("models", "folder"),
            listing("models/cube.obj", "demo::Mesh"),
            listing("textures/a.png", "demo::Texture"),
            listing("models/plane.obj", "demo::Mesh"),
        ];
        let paths: Vec<&str> = asset_candidates(&assets, Some("demo::Mesh"))
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths, ["models/cube.obj", "models/plane.obj"]);
        assert!(asset_candidates(&assets, None).is_empty());
    }

    /// A picker offers the project's files of the type first, then the store's
    /// own assets of it - the ones built in code, which a directory walk cannot
    /// find. A type that neither side has offers nothing.
    #[test]
    fn a_picker_offers_the_files_and_the_engines_own_assets() {
        let listing = |path: &str, kind: &str| AssetListing {
            root: String::new(),
            path: path.to_owned(),
            read_only: false,
            kind: AssetKind::parse(kind),
            standalone: true,
            has_meta: true,
            guid: Some(format!("file-{path}")),
        };
        let built = |name: &str, kind: &str| EngineAsset {
            kind: kind.to_owned(),
            name: name.to_owned(),
            guid: Some(format!("engine-{name}")),
        };
        let assets = vec![
            listing("passes/unlit.render_pass", "demo::RenderPass"),
            listing("passes/pbr.render_pass", "demo::RenderPass"),
        ];
        let engine_assets = vec![
            built(
                "pill.pbr.opaque",
                "pill_master_renderer::assets::RenderPass",
            ),
            built(
                "pill.pbr.skybox",
                "pill_master_renderer::assets::RenderPass",
            ),
            built(
                "pill.pbr.material",
                "pill_master_renderer::assets::Material",
            ),
        ];
        let (files, chosen) = picker_candidates(&assets, &engine_assets, Some("RenderPass"));
        let paths: Vec<&str> = files.iter().map(|row| row.path.as_str()).collect();
        assert_eq!(
            paths,
            ["passes/unlit.render_pass", "passes/pbr.render_pass"]
        );
        let names: Vec<&str> = chosen.iter().map(|asset| asset.name.as_str()).collect();
        assert_eq!(
            names,
            ["pill.pbr.opaque", "pill.pbr.skybox"],
            "the module's passes come after the project's files"
        );
        // A type neither side names offers nothing.
        let (files, chosen) = picker_candidates(&assets, &engine_assets, Some("demo::Scene"));
        assert!(files.is_empty() && chosen.is_empty());
    }

    /// An entity value becomes an entity row, not a read-only one.
    #[test]
    fn an_entity_value_is_an_entity_row() {
        let view = component(
            "demo::Follower",
            vec![FieldValue::Entity {
                id: 7,
                generation: 2,
            }],
        );
        assert_eq!(
            rows(&view),
            vec![FieldRow::Entity {
                index: 0,
                field: "a".to_owned(),
                tooltip: None,
            }]
        );
    }

    /// The picker offers every listed entity in the Hierarchy's order, and a
    /// typed or scripted label reads back as the handle it names.
    #[test]
    fn entity_references_round_trip_through_their_labels() {
        let listed = EntityListEntry {
            entity: Entity::from_raw(9, 3),
            display: "9v3".to_owned(),
            component_count: 1,
            archetype_label: String::new(),
        };
        let entries = vec![listed];
        assert_eq!(entity_candidates(&entries).len(), 1);
        assert_eq!(
            parse_entity_reference(&EntityListEntry::display_of(Entity::from_raw(9, 3))),
            Ok(Some(Entity::from_raw(9, 3)))
        );
        assert_eq!(
            parse_entity_reference("9"),
            Ok(Some(Entity::from_raw(9, 0)))
        );
        assert_eq!(parse_entity_reference("none"), Ok(None));
        assert!(parse_entity_reference("not-an-entity").is_err());
    }

    /// The entity edit builders queue `SetFieldEntity` under the field edit's
    /// label, once per selected entity, and the clear names no entity.
    #[test]
    fn entity_edits_queue_set_field_entity_for_the_selection() {
        let first = Entity::from_raw(1, 0);
        let second = Entity::from_raw(2, 0);
        let target = Entity::from_raw(4, 1);
        let (commands, label, key) = selection_entity_edit(
            &[first, second],
            first,
            "demo::Follower",
            "follow",
            Some(target),
        );
        assert_eq!(label, "set Follower.follow");
        assert_eq!(key, field_coalesce_key(first, "demo::Follower", "follow"));
        assert_eq!(commands.len(), 2);
        assert!(matches!(
            &commands[1],
            EditorCommand::SetFieldEntity { entity, reference: Some(entity_ref), .. }
                if *entity == second && *entity_ref == target
        ));
        let (cleared, _label) = entity_edit(first, "demo::Follower", "follow", None);
        assert!(matches!(
            cleared,
            EditorCommand::SetFieldEntity {
                reference: None,
                ..
            }
        ));
    }

    /// The asset edit builders queue `SetFieldAsset` under the field edit's
    /// label, once per selected entity, and `none` reads as a clear.
    #[test]
    fn asset_edits_queue_set_field_asset_for_the_selection() {
        let first = Entity::from_raw(1, 0);
        let second = Entity::from_raw(2, 0);
        let (commands, label, _key) = selection_asset_edit(
            &[first, second],
            first,
            "demo::MeshRendererComponent",
            "mesh",
            Some("models/cube.obj".to_owned()),
        );
        assert_eq!(label, "set MeshRendererComponent.mesh");
        assert_eq!(commands.len(), 2);
        assert!(matches!(
            &commands[1],
            EditorCommand::SetFieldAsset { entity, reference: Some(path), .. }
                if *entity == second && path == "models/cube.obj"
        ));
        assert!(is_cleared_reference(" none "));
        assert!(!is_cleared_reference("models/cube.obj"));
    }

    /// A component whose fields carry their own names and decorations, for the
    /// grouped-row tests: a nested type's leaves are dotted, so the fixture
    /// names them itself.
    fn named_fields(
        field_entries: Vec<(&'static str, FieldValue, Vec<FieldDecorator>)>,
    ) -> ComponentView {
        ComponentView {
            asset_labels: Vec::new(),
            entity_labels: Vec::new(),
            type_name: "demo::Grouped".to_string(),
            persistable: false,
            editable: true,
            fields: field_entries
                .iter()
                .map(
                    |(name, _, decorators)| crate::editor_state::ComponentFieldView {
                        name: name.to_string(),
                        decorators: decorators.clone(),
                    },
                )
                .collect(),
            values: field_entries
                .into_iter()
                .map(|(_, value, _)| value)
                .collect(),
        }
    }

    /// The `Color` component's four channels are one colour row: `r`, `g`,
    /// `b` and `a` share a (empty) prefix and carry the colour decorator.
    #[test]
    fn a_colour_components_channels_draw_as_one_colour_row() {
        let colour = |channel_value: f32| {
            vec![
                (
                    "r",
                    FieldValue::F32(channel_value),
                    vec![FieldDecorator::Color],
                ),
                (
                    "g",
                    FieldValue::F32(channel_value),
                    vec![FieldDecorator::Color],
                ),
                (
                    "b",
                    FieldValue::F32(channel_value),
                    vec![FieldDecorator::Color],
                ),
                ("a", FieldValue::F32(1.0), vec![FieldDecorator::Color]),
            ]
        };
        let view = named_fields(colour(0.5));
        assert_eq!(
            rows(&view),
            vec![FieldRow::Color {
                leaves: vec![
                    RowLeaf {
                        index: 0,
                        field: "r".to_string(),
                        element: None,
                    },
                    RowLeaf {
                        index: 1,
                        field: "g".to_string(),
                        element: None,
                    },
                    RowLeaf {
                        index: 2,
                        field: "b".to_string(),
                        element: None,
                    },
                    RowLeaf {
                        index: 3,
                        field: "a".to_string(),
                        element: None,
                    },
                ],
                field: "color".to_string(),
                tooltip: None,
            }]
        );
    }

    /// A nested struct's leaves (`fill.r`, `fill.g`, `fill.b`) collapse into
    /// one colour row, and the field after them keeps its own row.
    #[test]
    fn dotted_colour_leaves_collapse_into_one_row() {
        let view = named_fields(vec![
            ("fill.r", FieldValue::F32(1.0), vec![FieldDecorator::Color]),
            ("fill.g", FieldValue::F32(0.5), vec![FieldDecorator::Color]),
            ("fill.b", FieldValue::F32(0.25), vec![FieldDecorator::Color]),
            ("strength", FieldValue::F32(2.0), Vec::new()),
        ]);
        assert_eq!(
            rows(&view),
            vec![
                FieldRow::Color {
                    leaves: vec![
                        RowLeaf {
                            index: 0,
                            field: "fill.r".to_string(),
                            element: None,
                        },
                        RowLeaf {
                            index: 1,
                            field: "fill.g".to_string(),
                            element: None,
                        },
                        RowLeaf {
                            index: 2,
                            field: "fill.b".to_string(),
                            element: None,
                        },
                    ],
                    field: "fill".to_string(),
                    tooltip: None,
                },
                FieldRow::Single {
                    index: 3,
                    field: "strength".to_string(),
                    widget: FieldWidget::Float,
                    tooltip: None,
                },
            ]
        );
    }

    /// Dotted leaves marked as a vector draw as one vector row, and a lone
    /// channel without its partner stays a plain field.
    #[test]
    fn dotted_vector_leaves_collapse_into_one_row() {
        let view = named_fields(vec![
            (
                "point.x",
                FieldValue::F32(1.0),
                vec![FieldDecorator::Vector],
            ),
            (
                "point.y",
                FieldValue::F32(2.0),
                vec![FieldDecorator::Vector],
            ),
            (
                "point.z",
                FieldValue::F32(3.0),
                vec![FieldDecorator::Vector],
            ),
            (
                "stray.x",
                FieldValue::F32(4.0),
                vec![FieldDecorator::Vector],
            ),
        ]);
        assert_eq!(
            rows(&view),
            vec![
                FieldRow::VectorGroup {
                    leaves: vec![
                        RowLeaf {
                            index: 0,
                            field: "point.x".to_string(),
                            element: None,
                        },
                        RowLeaf {
                            index: 1,
                            field: "point.y".to_string(),
                            element: None,
                        },
                        RowLeaf {
                            index: 2,
                            field: "point.z".to_string(),
                            element: None,
                        },
                    ],
                    widget: FieldWidget::Float,
                    field: "point".to_string(),
                    tooltip: None,
                },
                FieldRow::Single {
                    index: 3,
                    field: "stray.x".to_string(),
                    widget: FieldWidget::Float,
                    tooltip: None,
                },
            ]
        );
    }

    /// A `[f32; 3]` field that declares the colour decorator draws as one
    /// colour row whose leaves are the array's own elements.
    #[test]
    fn a_colour_array_draws_one_colour_row() {
        let view = decorated(
            vec![FieldValue::Array(vec![
                FieldValue::F32(1.0),
                FieldValue::F32(0.0),
                FieldValue::F32(0.0),
            ])],
            vec![vec![FieldDecorator::Color]],
        );
        assert_eq!(
            rows(&view),
            vec![FieldRow::Color {
                leaves: vec![
                    RowLeaf {
                        index: 0,
                        field: "a".to_string(),
                        element: Some(0),
                    },
                    RowLeaf {
                        index: 0,
                        field: "a".to_string(),
                        element: Some(1),
                    },
                    RowLeaf {
                        index: 0,
                        field: "a".to_string(),
                        element: Some(2),
                    },
                ],
                field: "a".to_string(),
                tooltip: None,
            }]
        );
    }

    /// A grouped row's commit writes every channel and every selected entity,
    /// as one batch under one label.
    #[test]
    fn a_grouped_row_edit_writes_every_channel_in_one_batch() {
        let first = Entity::from_raw(1, 0);
        let second = Entity::from_raw(2, 0);
        let leaves = vec![
            RowLeaf {
                index: 0,
                field: "fill.r".to_string(),
                element: None,
            },
            RowLeaf {
                index: 1,
                field: "fill.g".to_string(),
                element: None,
            },
            RowLeaf {
                index: 2,
                field: "fill.b".to_string(),
                element: None,
            },
        ];
        let values = vec![
            FieldValue::F32(0.1),
            FieldValue::F32(0.2),
            FieldValue::F32(0.3),
        ];
        let (commands, label, key) =
            selection_leaf_edit(&[first, second], first, "demo::Grouped", &leaves, &values);
        assert_eq!(label, "set Grouped.fill.r");
        assert_eq!(key, field_coalesce_key(first, "demo::Grouped", "fill.r"));
        assert_eq!(commands.len(), 6, "three channels on two entities");
        assert!(matches!(
            &commands[5],
            EditorCommand::SetField { entity, field, .. }
                if *entity == second && field == "fill.b"
        ));
    }

    /// One array element's colour records the same step whether it is written
    /// through the row's picker or one channel at a time: an element leaf's
    /// label and coalesce key are the per-element form.
    #[test]
    fn a_colours_picker_and_its_channels_record_the_same_step() {
        let entity = Entity::from_raw(1, 0);
        let leaves: Vec<RowLeaf> = (0..3)
            .map(|element| RowLeaf {
                index: 0,
                field: "a".to_string(),
                element: Some(element),
            })
            .collect();
        let picked = vec![
            FieldValue::F32(1.0),
            FieldValue::F32(0.0),
            FieldValue::F32(0.0),
        ];
        let (picker_commands, picker_label, picker_key) =
            selection_leaf_edit(&[entity], entity, "demo::Tint", &leaves, &picked);
        let (channel_commands, channel_label, channel_key) = selection_element_edit(
            &[entity],
            entity,
            "demo::Tint",
            "a",
            0,
            FieldValue::F32(1.0),
        );
        assert_eq!(picker_label, "set Tint[0].a");
        assert_eq!(channel_label, picker_label);
        assert_eq!(channel_key, picker_key);
        assert_eq!(
            picker_commands.first(),
            channel_commands.first(),
            "one element, one command"
        );
    }
}
