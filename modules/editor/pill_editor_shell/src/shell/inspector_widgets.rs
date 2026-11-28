//! The Inspector's field controls.
//!
//! # Responsibilities
//!
//! - `FieldEditResponse`: what one control reports for a frame - the value and
//!   the interaction phase it belongs to.
//! - `scalar_edit_decorated`: one field's editor, drawn in the form the row
//!   model asked for. It returns the edit and leaves committing it to the
//!   caller.
//! - `Number` and `integer_edit`: the editor every whole-number kind shares,
//!   in its slider and drag-value forms.
//! - `axis_slot` and `fixed_width_field`: a vector component's axis letter and
//!   its editor. Both use a fixed width, so a long value is cropped inside its
//!   field instead of widening the row.
//! - `enum_edit`: a declared enum's variant dropdown.
//! - `scalar_edit`, `ascending` and `component_rows`: the plain editor, bounds
//!   put in ascending order, and a component's fields as plain display rows.
//! - The read-only row and the value formatters, which the inspector's rows
//!   and the asset settings rows draw.
//!
//! The two glob imports below use the package's absolute path, so a patch
//! crate resolves them too.

// Current crate
use pill_editor_core::hooks::FieldEditPhase;
#[allow(unused_imports)]
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::tab_inspector::{
    INSPECTOR_VECTOR_FIELD_HEIGHT, INSPECTOR_VECTOR_LABEL_WIDTH,
};
use pill_editor_shell::shell::*;

// =============================================================================
// Types
// =============================================================================

/// What one field control reports for a frame.
///
/// A continuous control reports [`FieldEditPhase::Interactive`] while it is
/// dragged and one [`FieldEditPhase::Committed`] on release; a one-shot control
/// reports one committed edit. The editor API's callbacks see the phase, so a
/// hook can react once when a drag ends rather than on every frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldEditResponse {
    /// The value to write.
    pub value: FieldValue,
    /// The interaction phase the write belongs to.
    pub phase: FieldEditPhase,
}

impl FieldEditResponse {
    /// One committed edit of `value`.
    pub fn committed(value: FieldValue) -> Self {
        Self {
            value,
            phase: FieldEditPhase::Committed,
        }
    }
}

/// Turns a continuous control's response into the edit it reports.
///
/// A drag reports `Interactive` while the pointer moves and one `Committed` on
/// release. A value typed into the control commits at once. A release frame
/// with no new value still reports the commit, carrying the value the control
/// shows, so a callback hears exactly one end of the interaction.
pub fn continuous_edit(response: &egui::Response, value: FieldValue) -> Option<FieldEditResponse> {
    if response.changed() {
        let phase = if response.dragged() {
            FieldEditPhase::Interactive
        } else {
            FieldEditPhase::Committed
        };
        return Some(FieldEditResponse { value, phase });
    }
    response
        .drag_stopped()
        .then_some(FieldEditResponse::committed(value))
}

/// Turns a one-shot control's response into the edit it reports.
pub fn one_shot_edit(response: &egui::Response, value: FieldValue) -> Option<FieldEditResponse> {
    response
        .changed()
        .then(|| FieldEditResponse::committed(value))
}

/// One whole number seen through `f64`, with the way back into its own
/// `FieldValue`.
///
/// Every integer kind needs the same editor behaviour, so this type lets the
/// slider and the drag value be written once instead of once per kind. Each
/// kind carries its own bounds, so a `[Range]` wider than the kind clamps to
/// the kind's limit.
///
/// Public like every item here, so a patch crate can reach it through the
/// shell's re-exports; the build script's inventory registers its functions by
/// qualified path.
pub struct Number {
    /// The value, as the editors read and write it.
    pub current: f64,
    /// The kind's own lower bound.
    pub kind_min: f64,
    /// The kind's own upper bound.
    pub kind_max: f64,
    /// The kind's `FieldValue` constructor for an edited value.
    pub rebuild: fn(f64) -> FieldValue,
}

impl Number {
    /// Returns the number inside `value` with its kind's own bounds, or `None`
    /// for a kind without a numeric editor.
    pub fn of(value: &FieldValue) -> Option<Number> {
        Some(match value {
            FieldValue::I8(current) => Number {
                current: *current as f64,
                kind_min: i8::MIN as f64,
                kind_max: i8::MAX as f64,
                rebuild: |edited| FieldValue::I8(edited as i8),
            },
            FieldValue::I16(current) => Number {
                current: *current as f64,
                kind_min: i16::MIN as f64,
                kind_max: i16::MAX as f64,
                rebuild: |edited| FieldValue::I16(edited as i16),
            },
            FieldValue::I32(current) => Number {
                current: *current as f64,
                kind_min: i32::MIN as f64,
                kind_max: i32::MAX as f64,
                rebuild: |edited| FieldValue::I32(edited as i32),
            },
            FieldValue::I64(current) => Number {
                current: *current as f64,
                kind_min: i64::MIN as f64,
                kind_max: i64::MAX as f64,
                rebuild: |edited| FieldValue::I64(edited as i64),
            },
            FieldValue::U8(current) => Number {
                current: *current as f64,
                kind_min: u8::MIN as f64,
                kind_max: u8::MAX as f64,
                rebuild: |edited| FieldValue::U8(edited as u8),
            },
            FieldValue::U16(current) => Number {
                current: *current as f64,
                kind_min: u16::MIN as f64,
                kind_max: u16::MAX as f64,
                rebuild: |edited| FieldValue::U16(edited as u16),
            },
            FieldValue::U32(current) => Number {
                current: *current as f64,
                kind_min: u32::MIN as f64,
                kind_max: u32::MAX as f64,
                rebuild: |edited| FieldValue::U32(edited as u32),
            },
            FieldValue::U64(current) => Number {
                current: *current as f64,
                kind_min: u64::MIN as f64,
                kind_max: u64::MAX as f64,
                rebuild: |edited| FieldValue::U64(edited as u64),
            },
            FieldValue::Usize(current) => Number {
                current: *current as f64,
                kind_min: usize::MIN as f64,
                kind_max: usize::MAX as f64,
                rebuild: |edited| FieldValue::Usize(edited as usize),
            },
            FieldValue::Isize(current) => Number {
                current: *current as f64,
                kind_min: isize::MIN as f64,
                kind_max: isize::MAX as f64,
                rebuild: |edited| FieldValue::Isize(edited as isize),
            },
            _ => return None,
        })
    }

    /// Returns `current` clamped into `low..=high` and rounded, ready for a
    /// whole-number widget.
    pub fn rounded(&self, low: f64, high: f64) -> f64 {
        self.current.clamp(low, high).round()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Draws the editor for one scalar field, as the widget the row model chose:
/// a slider over a `[Range]`, a drag value with a `[Min]` floor, and the plain
/// editor for every other kind.
///
/// A whole-number slider or floor runs through `f64` and is rounded back,
/// which is exact for the 8- to 32-bit kinds and precise to the float mantissa
/// above that.
pub fn scalar_edit_decorated(
    ui: &mut egui::Ui,
    value: &FieldValue,
    widget: &inspector::FieldWidget,
) -> Option<FieldEditResponse> {
    use inspector::FieldWidget as Row;
    match (value, widget) {
        (FieldValue::F32(current), Row::FloatSlider { min, max }) => {
            let mut edited = *current;
            let (low, high) = ascending(*min, *max);
            fill_slider_width(ui);
            let response = ui.add(egui::Slider::new(&mut edited, low..=high).max_decimals(4));
            continuous_edit(&response, FieldValue::F32(edited))
        }
        (FieldValue::F32(current), Row::FloatDrag { min }) => {
            let mut edited = current.max(*min);
            let response = stretched_drag_value(
                ui,
                egui::DragValue::new(&mut edited)
                    .range(*min..=f32::MAX)
                    .speed(0.05),
            );
            continuous_edit(&response, FieldValue::F32(edited))
        }
        (FieldValue::F64(current), Row::FloatSlider { min, max }) => {
            let mut edited = *current;
            let (low, high) = ascending(*min, *max);
            fill_slider_width(ui);
            let response =
                ui.add(egui::Slider::new(&mut edited, low as f64..=high as f64).max_decimals(9));
            continuous_edit(&response, FieldValue::F64(edited))
        }
        (FieldValue::F64(current), Row::FloatDrag { min }) => {
            let mut edited = current.max(*min as f64);
            let response = stretched_drag_value(
                ui,
                egui::DragValue::new(&mut edited)
                    .range(*min as f64..=f64::MAX)
                    .speed(0.05)
                    .max_decimals(9),
            );
            continuous_edit(&response, FieldValue::F64(edited))
        }
        // Every whole-number kind shares one editor. The decorator picks the
        // widget, and the kind supplies its own bounds.
        (_, Row::IntegerSlider { min, max }) => {
            integer_edit(ui, &Number::of(value)?, Some((*min, *max)), None)
        }
        (_, Row::IntegerDrag { min }) => integer_edit(ui, &Number::of(value)?, None, Some(*min)),
        _ => scalar_edit(ui, value),
    }
}

/// Draws the axis letter of one vector component in its fixed-width slot. The
/// slot keeps every component's editor the same width, whatever width its own
/// glyph takes.
pub fn axis_slot(ui: &mut egui::Ui, axis: usize) {
    ui.add_sized(
        egui::vec2(INSPECTOR_VECTOR_LABEL_WIDTH, INSPECTOR_VECTOR_FIELD_HEIGHT),
        egui::Label::new(
            egui::RichText::new(inspector::axis_label(axis))
                .color(inspector::axis_color(axis))
                .small(),
        ),
    );
}

/// Draws the editor for one vector component at the width left in its group.
/// The width is claimed exactly, and anything the editor paints past it - a
/// long value, the part of a drag value you drag - is cropped instead of
/// widening the row. Every component of a vector therefore keeps the same
/// width, whatever its value.
///
/// The crop is the child `Ui`'s clip rectangle. It also limits where the child
/// responds to the pointer, so a press past the field's edge never reaches
/// this editor. `widget` is the decorator the row model chose, or `None` for
/// the plain editor.
pub fn fixed_width_field(
    ui: &mut egui::Ui,
    value: &FieldValue,
    widget: Option<&inspector::FieldWidget>,
) -> Option<FieldEditResponse> {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), INSPECTOR_VECTOR_FIELD_HEIGHT),
        egui::Sense::hover(),
    );
    let mut field = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    field.set_clip_rect(rect.intersect(ui.clip_rect()));
    match widget {
        Some(widget) => scalar_edit_decorated(&mut field, value, widget),
        None => scalar_edit(&mut field, value),
    }
}

/// Adds a drag value that spans the whole width left in the row, the way every
/// value field in the Inspector runs to the panel's right edge.
///
/// egui centres a drag value in a larger slot rather than stretching it; its
/// smallest width is the style's interact width, so the width is raised to the
/// room left for this one widget.
pub fn stretched_drag_value(ui: &mut egui::Ui, drag_value: egui::DragValue<'_>) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.x = ui.available_width();
        ui.add(drag_value)
    })
    .inner
}

/// Sizes the next slider's track so the track and its value box together fill
/// the width left in the row.
pub fn fill_slider_width(ui: &mut egui::Ui) {
    let value_box_width = ui.spacing().interact_size.x + ui.spacing().item_spacing.x;
    ui.spacing_mut().slider_width = (ui.available_width() - value_box_width).max(40.0);
}

/// Returns the two bounds in ascending order. A decorator pair written the
/// other way round still draws a usable slider.
pub fn ascending(min: f32, max: f32) -> (f32, f32) {
    if min <= max {
        (min, max)
    } else {
        (max, min)
    }
}

/// Draws the whole-number editor for `number`: a slider over `range`, or a
/// drag value with `floor`. Both are clamped to the kind's own bounds.
pub fn integer_edit(
    ui: &mut egui::Ui,
    number: &Number,
    range: Option<(f32, f32)>,
    floor: Option<f32>,
) -> Option<FieldEditResponse> {
    if let Some((low, high)) = range {
        let low = (low as f64).clamp(number.kind_min, number.kind_max);
        let high = (high as f64).clamp(number.kind_min, number.kind_max);
        let (low, high) = if low <= high {
            (low, high)
        } else {
            (high, low)
        };
        let mut current = number.rounded(low, high);
        fill_slider_width(ui);
        let response = ui.add(
            egui::Slider::new(&mut current, low..=high)
                .step_by(1.0)
                .fixed_decimals(0),
        );
        return continuous_edit(&response, (number.rebuild)(current.round()));
    }
    let low = (floor? as f64).clamp(number.kind_min, number.kind_max);
    let mut current = number.rounded(low, number.kind_max);
    let response = stretched_drag_value(
        ui,
        egui::DragValue::new(&mut current)
            .range(low..=number.kind_max)
            .speed(1.0)
            .fixed_decimals(0),
    );
    continuous_edit(&response, (number.rebuild)(current.round()))
}

/// Draws the editor for one scalar field: a checkbox for a boolean, a drag
/// value for a number (drag to scrub, click to type). Returns `None` when the
/// kind has no editor, and the caller then draws the value as read-only text.
pub fn scalar_edit(ui: &mut egui::Ui, value: &FieldValue) -> Option<FieldEditResponse> {
    match value {
        FieldValue::Bool(current) => {
            let mut edited = *current;
            let response = ui.checkbox(&mut edited, "");
            one_shot_edit(&response, FieldValue::Bool(edited))
        }
        FieldValue::F32(current) => {
            let mut edited = *current;
            let response = stretched_drag_value(ui, egui::DragValue::new(&mut edited).speed(0.05));
            continuous_edit(&response, FieldValue::F32(edited))
        }
        FieldValue::F64(current) => {
            let mut edited = *current;
            let response = stretched_drag_value(
                ui,
                egui::DragValue::new(&mut edited)
                    .speed(0.05)
                    .max_decimals(9),
            );
            continuous_edit(&response, FieldValue::F64(edited))
        }
        FieldValue::Enum {
            type_tag,
            discriminant,
        } => enum_edit(ui, type_tag, *discriminant).map(FieldEditResponse::committed),
        // Every whole-number kind shares one drag value here. A kind without a
        // numeric editor answers `None`.
        other => {
            let number = Number::of(other)?;
            let mut edited = number.current;
            let response = stretched_drag_value(ui, egui::DragValue::new(&mut edited));
            continuous_edit(&response, (number.rebuild)(edited))
        }
    }
}

/// Draws a declared enum's dropdown, listing the variants its field tag
/// names. Returns the picked variant's value when it differs from the current
/// one, and `None` when nothing changed or the tag cannot be read.
///
/// A row holding a discriminant no variant declares shows the number, so the
/// stored value stays visible until a variant is picked.
pub fn enum_edit(
    ui: &mut egui::Ui,
    type_tag: &'static str,
    discriminant: i64,
) -> Option<FieldValue> {
    let enum_tag = pill_engine::parse_enum_tag(type_tag)?;
    let selected_text = enum_tag
        .variant_name(discriminant)
        .map_or_else(|| discriminant.to_string(), str::to_string);
    let mut picked = discriminant;
    egui::ComboBox::from_id_salt(ui.next_auto_id())
        .selected_text(selected_text)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (variant_name, variant_discriminant) in &enum_tag.variants {
                ui.selectable_value(&mut picked, *variant_discriminant, *variant_name);
            }
        });
    (picked != discriminant).then_some(FieldValue::Enum {
        type_tag,
        discriminant: picked,
    })
}

/// Draws one read-only (name, value) row: the shared label column, then the
/// value.
///
/// The label is also its own tooltip, because the fixed label column truncates
/// a long name. A value too long for the row is truncated with its own
/// tooltip, so it never pushes the row wider.
pub fn read_only_row(ui: &mut egui::Ui, label: &str, value: &str) {
    row(ui, label, Some(label), &mut |ui| {
        kit::tooltip::on_hover_text(ui.add(egui::Label::new(value).truncate()), value);
    });
}

/// Returns a field value as display text. Kinds without their own formatting
/// fall back to their debug form.
pub fn format_field_value(value: &FieldValue) -> String {
    match value {
        FieldValue::F32(number) => format!("{number}"),
        FieldValue::F64(number) => format!("{number}"),
        FieldValue::I8(number) => format!("{number}"),
        FieldValue::I16(number) => format!("{number}"),
        FieldValue::I32(number) => format!("{number}"),
        FieldValue::I64(number) => format!("{number}"),
        FieldValue::U8(number) => format!("{number}"),
        FieldValue::U16(number) => format!("{number}"),
        FieldValue::U32(number) => format!("{number}"),
        FieldValue::U64(number) => format!("{number}"),
        FieldValue::Usize(number) => format!("{number}"),
        FieldValue::Bool(current) => format!("{current}"),
        FieldValue::Text(text) => text.clone(),
        FieldValue::Enum { discriminant, .. } => value
            .enum_variant_name()
            .map_or_else(|| discriminant.to_string(), str::to_string),
        other => format!("{other:?}"),
    }
}

/// Returns the Inspector's rows for `detail`: one (type name, field rows) pair
/// per component, and each field row a (name, formatted value) pair.
pub fn component_rows(detail: &EntityDetail) -> Vec<(String, Vec<(String, String)>)> {
    detail
        .components
        .iter()
        .map(|component| {
            let fields = component
                .fields
                .iter()
                .zip(&component.values)
                .map(|(descriptor, value)| (descriptor.name.to_string(), format_field_value(value)))
                .collect();
            (component.type_name.clone(), fields)
        })
        .collect()
}
