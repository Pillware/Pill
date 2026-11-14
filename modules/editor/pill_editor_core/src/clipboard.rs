//! The editor clipboard's structured payloads: the JSON documents a copy
//! writes and a paste reads.
//!
//! # Responsibilities
//!
//! - One versioned document that names a component by its registered type
//!   name and a field by its field name, and carries each value as a token
//!   string ([`format_value`] / [`parse_value`]). No pointer, registry id or
//!   `ComponentId` is written, so a payload still means the same thing after
//!   a reload and in another process.
//! - Encoding a payload from the snapshot's detail tier, and decoding it back
//!   into the shapes a paste needs: `CreateEntity`'s component seeds, or
//!   `SetField`s (plus an `AddComponent` when the component is missing).
//!
//! The clipboard also carries this text as plain text, which is how another
//! editor instance, or a text editor, reads it. That is why the document is
//! JSON and not a binary blob.

// Standard library
use std::fmt::Write as _;

// External crates
use pill_engine::FieldValue;
use serde_json::{json, Value};

// Current crate
use crate::editor_state::{
    ComponentSeed, EditorCommand, EditorSelection, EditorSnapshot, EntityDetail,
};

// =============================================================================
// Constants
// =============================================================================

/// The name of the clipboard format the structured payload is registered
/// under. The executable registers the name with `RegisterClipboardFormatW`
/// and writes the payload in that format beside the plain text.
pub const FORMAT_NAME: &str = "PillEditor.Object";

/// The version of the payload document. A reader refuses any other version.
pub const VERSION: u64 = 1;

// =============================================================================
// Types
// =============================================================================

/// One decoded payload document.
///
/// The variant is the document's `kind` member: a copy of an entity, of one
/// component, of one field value, or of one asset reference.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    /// One entity, as the components a fresh entity is seeded with.
    Entity {
        /// The components, each with the field values it carried.
        components: Vec<SeededComponent>,
    },
    /// One component, as the values a paste writes onto an entity.
    Component {
        /// The component's type name and its carried field values.
        component: SeededComponent,
    },
    /// One field value, carried as its token.
    Field {
        /// Registered component type name.
        component: String,
        /// Field name.
        field: String,
        /// The value's token, in the form [`format_value`] writes.
        value: String,
    },
    /// One asset reference, by project-relative path.
    Asset {
        /// Project-relative path, with forward slashes.
        path: String,
    },
}

impl Payload {
    /// Returns the document's kind tag, for messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Entity { .. } => "entity",
            Self::Component { .. } => "component",
            Self::Field { .. } => "field",
            Self::Asset { .. } => "asset",
        }
    }
}

/// One component in a payload: its type name and the fields it carried.
#[derive(Clone, Debug, PartialEq)]
pub struct SeededComponent {
    /// Registered component type name. This name, not a registry id, is the
    /// key that survives a reload.
    pub type_name: String,
    /// Each field's name and its token value, in the component's layout order.
    pub fields: Vec<(String, String)>,
}

/// The three clipboard commands the Edit menu and the shortcuts run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardAction {
    /// Put the selection on the clipboard.
    Copy,
    /// Put the selection on the clipboard and delete it.
    Cut,
    /// Read the clipboard and apply what it carries.
    Paste,
}

/// What the executable should do with the OS clipboard.
///
/// The shell cannot reach the OS clipboard. It is a reloadable DLL, and the
/// Win32 half belongs to the executable, which is the only binary that names
/// the `windows` crate. The shell therefore queues one of these, and the
/// executable carries it out at the frame boundary, the same way
/// `PanelCommand` and `PendingEditorCommands` queue their work.
#[derive(Clone, Debug, PartialEq)]
pub enum ClipboardRequest {
    /// Write this payload text to the clipboard, in the registered format and
    /// as plain text.
    Write(String),
    /// Read the clipboard and apply it as a paste.
    Read,
}

/// The shell's side of the clipboard: the request it queued and the text the
/// executable last read back.
#[derive(Default)]
pub struct EditorClipboard {
    /// The request waiting for the executable, which takes it at the frame
    /// boundary.
    pub request: Option<ClipboardRequest>,
    /// The last text the executable read from the clipboard, kept even when
    /// the text is not a payload.
    pub text: String,
}

impl pill_engine::Resource for EditorClipboard {}

impl EditorClipboard {
    /// Takes the waiting request, if there is one.
    pub fn take_request(&mut self) -> Option<ClipboardRequest> {
        self.request.take()
    }
}

/// The recorded step one paste produces, and what the paste could not carry.
#[derive(Clone, Debug, PartialEq)]
pub struct PastePlan {
    /// The commands, in the order the batch applies them.
    pub commands: Vec<EditorCommand>,
    /// The history label the step is recorded under.
    pub label: String,
    /// What the paste skipped: a component type this build does not register,
    /// or a field whose token it cannot read, each named `type.field`. The
    /// caller reports them by name.
    pub skipped: Vec<String>,
    /// A note for the caller, shown as a notice. An asset paste sets it to the
    /// asset it moved the selection to.
    pub note: String,
}

/// A function that reads the digits in front of a type suffix and turns them
/// into the field value of that suffix's type.
type SuffixDigitsParser = fn(&str) -> Option<FieldValue>;

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the token that stands for a field value, or `None` for a value the
/// payload cannot carry.
///
/// Arrays, lists, text, enums, opaque payloads, and asset and entity
/// references have no token: an enum value only means something next to the
/// field tag that names its variants, which a token does not carry. The last two hold a slot of the world that issued them, so a
/// token would mean nothing in another process.
pub fn format_value(value: &FieldValue) -> Option<String> {
    Some(match value {
        FieldValue::F32(value) => format!("{value:?}f"),
        FieldValue::F64(value) => format!("{value:?}d"),
        FieldValue::I8(value) => format!("{value}i8"),
        FieldValue::I16(value) => format!("{value}i16"),
        FieldValue::I32(value) => format!("{value}i32"),
        FieldValue::I64(value) => format!("{value}i64"),
        FieldValue::U8(value) => format!("{value}u8"),
        FieldValue::U16(value) => format!("{value}u16"),
        FieldValue::U32(value) => format!("{value}u32"),
        FieldValue::U64(value) => format!("{value}u64"),
        FieldValue::Usize(value) => format!("{value}usize"),
        FieldValue::Isize(value) => format!("{value}isize"),
        FieldValue::Bool(value) => value.to_string(),
        FieldValue::Array(_)
        | FieldValue::List { .. }
        | FieldValue::Text(_)
        | FieldValue::Asset { .. }
        | FieldValue::Entity { .. }
        | FieldValue::Enum { .. }
        | FieldValue::Opaque { .. } => return None,
    })
}

/// Returns the value a token stands for, or `None` when this build cannot read
/// the token.
pub fn parse_value(token: &str) -> Option<FieldValue> {
    // Step 1: the two boolean tokens.
    match token {
        "true" => return Some(FieldValue::Bool(true)),
        "false" => return Some(FieldValue::Bool(false)),
        _ => {}
    }
    // Step 2: a number with a type suffix. The suffixes are tried longest
    // first. `usize` and `isize` end in `e`, so a shorter suffix cannot match
    // their tail by accident. A token with no digits before its suffix (`f`)
    // or with only a minus sign (`-i8`) is not a value.
    const SUFFIX_PARSERS: [(&str, SuffixDigitsParser); 12] = [
        ("usize", |digits| digits.parse().ok().map(FieldValue::Usize)),
        ("isize", |digits| digits.parse().ok().map(FieldValue::Isize)),
        ("u64", |digits| digits.parse().ok().map(FieldValue::U64)),
        ("u32", |digits| digits.parse().ok().map(FieldValue::U32)),
        ("u16", |digits| digits.parse().ok().map(FieldValue::U16)),
        ("u8", |digits| digits.parse().ok().map(FieldValue::U8)),
        ("i64", |digits| digits.parse().ok().map(FieldValue::I64)),
        ("i32", |digits| digits.parse().ok().map(FieldValue::I32)),
        ("i16", |digits| digits.parse().ok().map(FieldValue::I16)),
        ("i8", |digits| digits.parse().ok().map(FieldValue::I8)),
        ("f", |digits| digits.parse().ok().map(FieldValue::F32)),
        ("d", |digits| digits.parse().ok().map(FieldValue::F64)),
    ];
    for (suffix, parse) in SUFFIX_PARSERS {
        if let Some(digits) = token.strip_suffix(suffix) {
            if !digits.is_empty() && digits != "-" {
                if let Some(value) = parse(digits) {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// Builds the payload document for one entity's detail: every editable
/// component, with only the field values a payload can carry.
pub fn encode_entity(detail: &EntityDetail) -> String {
    let mut components: Vec<Value> = Vec::new();
    for component in &detail.components {
        if !component.editable {
            continue;
        }
        components.push(component_document(
            &component.type_name,
            &component_field_pairs(component),
        ));
    }
    let document = json!({
        "pill": VERSION,
        "kind": "entity",
        "components": components,
    });
    document.to_string()
}

/// Builds the payload document for one component's current values.
pub fn encode_component(type_name: &str, fields: &[(String, FieldValue)]) -> String {
    let document = json!({
        "pill": VERSION,
        "kind": "component",
        "component": component_document(type_name, fields),
    });
    document.to_string()
}

/// Builds the payload document for one field value, or returns `None` when the
/// value has no token.
pub fn encode_field(component: &str, field: &str, value: &FieldValue) -> Option<String> {
    let token = format_value(value)?;
    let document = json!({
        "pill": VERSION,
        "kind": "field",
        "component": component,
        "field": field,
        "value": token,
    });
    Some(document.to_string())
}

/// Builds the payload document for one asset reference.
pub fn encode_asset(path: &str) -> String {
    let document = json!({
        "pill": VERSION,
        "kind": "asset",
        "path": path,
    });
    document.to_string()
}

/// Builds the payload text a copy takes from the current selection.
///
/// An entity wins over an asset when both slots hold, because a cut deletes
/// the entity. `None` means the selection holds nothing a copy could write.
pub fn selection_payload(selection: &EditorSelection, snapshot: &EditorSnapshot) -> Option<String> {
    if let Some(detail) = snapshot.detail.as_ref() {
        return Some(encode_entity(detail));
    }
    if let Some(path) = selection.asset.as_deref() {
        return Some(encode_asset(path));
    }
    None
}

/// Reports whether the selection holds something a copy would write.
///
/// This is the boolean form of [`selection_payload`], and the Edit menu's Copy
/// and Cut rows read it: a row is offered exactly when the copy it runs would
/// carry something. An entity whose detail the snapshot has not captured yet
/// has no payload, so the row stays unavailable until the next capture.
pub fn can_copy(selection: &EditorSelection, snapshot: &EditorSnapshot) -> bool {
    selection_payload(selection, snapshot).is_some()
}

/// Queues one clipboard command from the shell.
///
/// A copy writes the selection's payload. A cut writes it and also queues the
/// delete, as the same recorded step the Entities tab's trash button queues;
/// that delete is queued even when the selection had no payload to write. A
/// paste asks the executable to read the clipboard.
///
/// Returns a one-line summary of the payload for the caller's log line, or
/// `None` when a copy or a cut found nothing to write.
pub fn queue_action(
    clipboard: &mut EditorClipboard,
    action: ClipboardAction,
    selection: &EditorSelection,
    snapshot: &EditorSnapshot,
    pending_commands: &mut crate::commands::PendingEditorCommands,
) -> Option<String> {
    // Step 1: queue the request the executable carries out.
    let summary = queue_request(clipboard, action, selection, snapshot);
    // Step 2: a cut also queues its delete, recorded as one step.
    if action == ClipboardAction::Cut {
        if let Some((commands, label)) = cut_commands(selection) {
            pending_commands.push_recorded(commands, label, None);
        }
    }
    summary
}

/// Queues the request alone: the write a copy or a cut needs, or the read a
/// paste needs.
///
/// It is split from [`queue_action`] because the caller reaches the two
/// resources behind separate borrows: the request lives on the clipboard
/// resource, and the recorded delete on the pending-command queue. Both cannot
/// be borrowed mutably at once.
///
/// Returns the payload's one-line summary for the caller's log line, `read` for
/// a paste, or `None` when a copy or a cut found nothing to write.
pub fn queue_request(
    clipboard: &mut EditorClipboard,
    action: ClipboardAction,
    selection: &EditorSelection,
    snapshot: &EditorSnapshot,
) -> Option<String> {
    match action {
        ClipboardAction::Copy | ClipboardAction::Cut => {
            let payload_text = selection_payload(selection, snapshot)?;
            let summary = decode(&payload_text)
                .map(|payload| summarize(&payload))
                .unwrap_or_else(|_| "payload".to_string());
            clipboard.request = Some(ClipboardRequest::Write(payload_text));
            Some(summary)
        }
        ClipboardAction::Paste => {
            clipboard.request = Some(ClipboardRequest::Read);
            Some("read".to_string())
        }
    }
}

/// Builds the recorded step a cut queues: the same delete the Entities tab's
/// trash button queues.
///
/// The step clears the selection and destroys the entity. Undoing it restores
/// the entity with a new handle and selects that handle. `None` means the
/// selection has no primary entity, so there is nothing to delete.
pub fn cut_commands(selection: &EditorSelection) -> Option<(Vec<EditorCommand>, String)> {
    let entity = selection.primary()?;
    Some(crate::entities::entity_delete(
        entity,
        selection.asset.clone(),
    ))
}

/// Builds the recorded step a paste applies.
///
/// An entity payload becomes one `CreateEntity`. A component payload becomes
/// an `AddComponent` (only when the entity does not carry the component yet)
/// plus one `SetField` per field. A field payload becomes one `SetField`. An
/// asset payload queues no command, because it only moves the selection.
///
/// In an entity payload, a component this build does not register is skipped
/// and named in [`PastePlan::skipped`]. A field whose token does not parse is
/// skipped the same way in every payload kind that carries fields, so a paste
/// never writes a value it could not read; the field keeps its current value,
/// or the component's default when the component is new.
///
/// # Errors
///
/// Returns a message when a component payload names a component this build
/// does not register or cannot add, when a component or field payload has no
/// selected entity, or when a field payload's value is not a token this build
/// parses.
pub fn paste_plan(
    payload: &Payload,
    selection: &EditorSelection,
    snapshot: &EditorSnapshot,
) -> Result<PastePlan, String> {
    let is_registered = |type_name: &str| {
        snapshot
            .components
            .iter()
            .any(|entry| entry.type_name == type_name && entry.addable)
    };
    let mut skipped = Vec::new();
    match payload {
        Payload::Entity { components } => {
            let mut component_seeds = Vec::with_capacity(components.len());
            for component in components {
                if !is_registered(&component.type_name) {
                    skipped.push(component.type_name.clone());
                    continue;
                }
                component_seeds.push(seeded_component(component, &mut skipped));
            }
            Ok(PastePlan {
                commands: vec![EditorCommand::CreateEntity {
                    components: component_seeds,
                }],
                label: "paste entity".to_string(),
                skipped,
                note: String::new(),
            })
        }
        Payload::Component { component } => {
            if !is_registered(&component.type_name) {
                return Err(format!(
                    "no component named `{}` is loaded",
                    component.type_name
                ));
            }
            let Some(entity) = selection.primary() else {
                return Err("paste a component onto a selected entity".to_string());
            };
            // An entity that already carries the component takes the payload's
            // values over it. `AddComponent` is only for an entity that does
            // not, because attaching a component twice fails. One failed
            // command stops the batch from being recorded, so the step could
            // not be undone.
            let already_carries_component = snapshot
                .selected_details
                .iter()
                .chain(snapshot.detail.iter())
                .find(|detail| detail.entity == entity)
                .is_some_and(|detail| {
                    detail
                        .components
                        .iter()
                        .any(|view| view.type_name == component.type_name)
                });
            let mut commands = Vec::new();
            if !already_carries_component {
                commands.push(EditorCommand::AddComponent {
                    entity,
                    component: component.type_name.clone(),
                });
            }
            for (field, token) in &component.fields {
                let Some(value) = parse_value(token) else {
                    skipped.push(format!("{}.{}", component.type_name, field));
                    continue;
                };
                commands.push(EditorCommand::SetField {
                    entity,
                    component: component.type_name.clone(),
                    field: field.clone(),
                    value,
                });
            }
            Ok(PastePlan {
                commands,
                label: format!("paste {}", component.type_name),
                skipped,
                note: String::new(),
            })
        }
        Payload::Field {
            component,
            field,
            value,
        } => {
            let Some(entity) = selection.primary() else {
                return Err("paste a value onto a selected entity".to_string());
            };
            let Some(value) = parse_value(value) else {
                return Err(format!("`{value}` is not a value this build parses"));
            };
            Ok(PastePlan {
                commands: vec![EditorCommand::SetField {
                    entity,
                    component: component.clone(),
                    field: field.clone(),
                    value,
                }],
                label: format!("paste {component}.{field}"),
                skipped,
                note: String::new(),
            })
        }
        Payload::Asset { path } => Ok(PastePlan {
            commands: Vec::new(),
            label: "paste asset".to_string(),
            skipped,
            note: format!("asset {path}"),
        }),
    }
}

/// Turns one payload component into the seed a create rebuilds.
///
/// A field whose token does not parse is left out and named in `skipped` as
/// `type.field`, the same form the component paste uses. The new entity then
/// gets the component's default for that field, and a notice names what was
/// skipped.
fn seeded_component(component: &SeededComponent, skipped: &mut Vec<String>) -> ComponentSeed {
    let mut fields = Vec::with_capacity(component.fields.len());
    for (field, token) in &component.fields {
        match parse_value(token) {
            Some(value) => fields.push((field.clone(), value)),
            None => skipped.push(format!("{}.{}", component.type_name, field)),
        }
    }
    ComponentSeed {
        type_name: component.type_name.clone(),
        fields,
    }
}

/// Decodes a payload document.
///
/// # Errors
///
/// Returns a message when the text is not JSON, when the document's version is
/// not [`VERSION`], when its `kind` member is missing or unknown, or when a
/// component entry is malformed: no type name, or a field entry that is not a
/// name and a token.
pub fn decode(document_text: &str) -> Result<Payload, String> {
    // Step 1: read the document and refuse anything that is not this version.
    let document: Value = serde_json::from_str(document_text)
        .map_err(|error| format!("payload is not JSON: {error}"))?;
    let version = document.get("pill").and_then(Value::as_u64);
    if version != Some(VERSION) {
        return Err(format!(
            "payload version {} is not {VERSION}",
            version.map_or("missing".to_string(), |value| value.to_string())
        ));
    }
    // Step 2: decode the members the kind names.
    let kind = document
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "payload has no kind".to_string())?;
    match kind {
        "entity" => {
            let components = document
                .get("components")
                .and_then(Value::as_array)
                .ok_or_else(|| "entity payload has no components".to_string())?;
            let mut seeds = Vec::with_capacity(components.len());
            for component in components {
                seeds.push(decode_component(component)?);
            }
            Ok(Payload::Entity { components: seeds })
        }
        "component" => {
            let component = document
                .get("component")
                .ok_or_else(|| "component payload has no component".to_string())?;
            Ok(Payload::Component {
                component: decode_component(component)?,
            })
        }
        "field" => {
            let component = string_field(&document, "component")?;
            let field = string_field(&document, "field")?;
            let value = string_field(&document, "value")?;
            Ok(Payload::Field {
                component,
                field,
                value,
            })
        }
        "asset" => Ok(Payload::Asset {
            path: string_field(&document, "path")?,
        }),
        other => Err(format!("unknown payload kind `{other}`")),
    }
}

/// Pairs each field of one component view with its current value.
///
/// Every value is kept, including the ones a payload cannot carry. The caller
/// drops those when it writes the document.
pub fn component_field_pairs(
    component: &crate::editor_state::ComponentView,
) -> Vec<(String, FieldValue)> {
    component
        .fields
        .iter()
        .zip(component.values.iter())
        .map(|(descriptor, value)| (descriptor.name.to_string(), value.clone()))
        .collect()
}

/// Builds the JSON object for one component: its type name and its carried
/// fields, each written as a `[name, token]` pair.
fn component_document(type_name: &str, fields: &[(String, FieldValue)]) -> Value {
    let mut carried_fields: Vec<Value> = Vec::new();
    for (name, value) in fields {
        if let Some(token) = format_value(value) {
            carried_fields.push(json!([name, token]));
        }
    }
    json!({ "type": type_name, "fields": carried_fields })
}

/// Decodes one component object into its seed.
///
/// A field entry is never dropped: losing one without a word would make the
/// paste quietly lossy. A component with no `fields` member decodes with no
/// fields.
///
/// # Errors
///
/// Returns a message when the object has no `type`, when a field entry is not
/// a `[name, token]` pair, or when the name or the token is not a string.
fn decode_component(component: &Value) -> Result<SeededComponent, String> {
    let type_name = component
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "component has no type".to_string())?
        .to_string();
    let mut fields: Vec<(String, String)> = Vec::new();
    if let Some(entries) = component.get("fields").and_then(Value::as_array) {
        for entry in entries {
            let pair = entry
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or_else(|| format!("field entry of `{type_name}` is not a pair"))?;
            let name = pair[0]
                .as_str()
                .ok_or_else(|| format!("field entry of `{type_name}` has no name"))?
                .to_string();
            let token = pair[1]
                .as_str()
                .ok_or_else(|| format!("field `{name}` of `{type_name}` has no value"))?
                .to_string();
            fields.push((name, token));
        }
    }
    Ok(SeededComponent { type_name, fields })
}

/// Reads one required string member of a document.
///
/// # Errors
///
/// Returns a message naming the member when it is missing or is not a string.
fn string_field(document: &Value, member: &str) -> Result<String, String> {
    document
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("payload has no `{member}`"))
}

/// Builds a one-line summary of a payload, for a log line.
pub fn summarize(payload: &Payload) -> String {
    let mut summary = String::new();
    match payload {
        Payload::Entity { components } => {
            let _ = write!(summary, "entity with {} component(s)", components.len());
            for component in components {
                let _ = write!(summary, ", {}", component.type_name);
            }
        }
        Payload::Component { component } => {
            let _ = write!(
                summary,
                "component {} with {} field(s)",
                component.type_name,
                component.fields.len()
            );
        }
        Payload::Field {
            component,
            field,
            value,
        } => {
            let _ = write!(summary, "field {component}.{field} = {value}");
        }
        Payload::Asset { path } => {
            let _ = write!(summary, "asset {path}");
        }
    }
    summary
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_state::{ComponentFieldView, ComponentView, EntityDetail};
    use pill_engine::Entity;

    /// One field of a component view, with no decorators.
    fn field_view(field_name: &str) -> ComponentFieldView {
        ComponentFieldView {
            name: field_name.to_string(),
            decorators: Vec::new(),
        }
    }

    /// A detail carrying one editable component and one that is not.
    ///
    /// The entity comes from a real world: the payload only carries the
    /// components, but a test that invents handles tests nothing.
    fn sample_detail() -> EntityDetail {
        let mut engine = pill_engine::Engine::new();
        let entity = engine
            .world_mut()
            .create_entity()
            .build()
            .expect("an empty entity builds");
        EntityDetail {
            entity,
            components: vec![
                ComponentView {
                    asset_labels: Vec::new(),
                    entity_labels: Vec::new(),
                    type_name: "Position".to_string(),
                    persistable: true,
                    editable: true,
                    fields: vec![field_view("x"), field_view("y")],
                    values: vec![FieldValue::F32(1.5), FieldValue::F32(-2.25)],
                },
                ComponentView {
                    asset_labels: Vec::new(),
                    entity_labels: Vec::new(),
                    type_name: "Opaque".to_string(),
                    persistable: false,
                    editable: false,
                    fields: Vec::new(),
                    values: Vec::new(),
                },
            ],
        }
    }

    /// An editable component and its values come back from the payload
    /// unchanged, and a component that is not editable is left out.
    #[test]
    fn an_entity_round_trips_through_the_payload() {
        let text = encode_entity(&sample_detail());
        match decode(&text).expect("round trip") {
            Payload::Entity { components } => {
                assert_eq!(components.len(), 1, "only editable components are carried");
                assert_eq!(components[0].type_name, "Position");
                assert_eq!(
                    components[0].fields,
                    vec![
                        ("x".to_string(), "1.5f".to_string()),
                        ("y".to_string(), "-2.25f".to_string()),
                    ]
                );
            }
            other => panic!("unexpected payload {other:?}"),
        }
    }

    /// Every scalar value survives a trip through its token, and the values a
    /// payload cannot carry get no token at all.
    #[test]
    fn every_scalar_token_round_trips() {
        let values = [
            FieldValue::F32(1.5),
            FieldValue::F64(-0.125),
            FieldValue::I8(-8),
            FieldValue::I16(-16),
            FieldValue::I32(-32),
            FieldValue::I64(-64),
            FieldValue::U8(8),
            FieldValue::U16(16),
            FieldValue::U32(32),
            FieldValue::U64(64),
            FieldValue::Usize(4),
            FieldValue::Isize(-4),
            FieldValue::Bool(true),
            FieldValue::Bool(false),
        ];
        for value in values {
            let token = format_value(&value).expect("scalar formats");
            assert_eq!(parse_value(&token), Some(value.clone()), "token {token}");
        }
        for value in [
            FieldValue::Array(Vec::new()),
            FieldValue::List {
                element_tag: "f32",
                element_count: 3,
            },
            FieldValue::Text("hi".to_string()),
        ] {
            assert_eq!(format_value(&value), None);
        }
    }

    /// Text that is not JSON, another version and an unknown kind are each
    /// refused, while a field payload decodes.
    #[test]
    fn versions_and_kinds_are_refused_by_name() {
        assert!(decode("not json").is_err());
        assert!(decode(r#"{"pill": 99, "kind": "entity", "components": []}"#).is_err());
        assert!(decode(r#"{"pill": 1, "kind": "mystery"}"#).is_err());
        let field = encode_field("Position", "x", &FieldValue::F32(2.0)).expect("carried");
        assert!(matches!(decode(&field), Ok(Payload::Field { .. })));
    }

    /// A snapshot whose registry knows `component_type_names` and nothing
    /// else.
    fn snapshot_with(component_type_names: &[&str]) -> EditorSnapshot {
        EditorSnapshot {
            components: component_type_names
                .iter()
                .map(|type_name| crate::editor_state::RegisteredComponent {
                    type_name: type_name.to_string(),
                    addable: true,
                })
                .collect(),
            ..EditorSnapshot::default()
        }
    }

    /// A selection holding exactly one entity.
    fn selection_with(entity: Entity) -> EditorSelection {
        let mut selection = EditorSelection::default();
        selection.select_only(entity);
        selection
    }

    /// Pasting an entity creates one, seeded with what the payload carried, and
    /// names the components this build does not register.
    #[test]
    fn an_entity_paste_seeds_a_create_and_names_what_it_skipped() {
        let text = concat!(
            r#"{"pill": 1, "kind": "entity", "components": ["#,
            r#"{"type": "Position", "fields": [["x", "1.5f"]]},"#,
            r#"{"type": "Gone", "fields": []}]}"#
        );
        let payload = decode(text).expect("a well-formed payload");
        let plan = paste_plan(
            &payload,
            &selection_with(Entity::from_raw(1, 0)),
            &snapshot_with(&["Position"]),
        )
        .expect("the paste plans");
        assert_eq!(plan.skipped, vec!["Gone".to_string()]);
        assert_eq!(plan.commands.len(), 1);
        match &plan.commands[0] {
            EditorCommand::CreateEntity { components } => {
                assert_eq!(components.len(), 1, "the unregistered component is skipped");
                assert_eq!(components[0].type_name, "Position");
                assert_eq!(
                    components[0].fields,
                    vec![("x".to_string(), FieldValue::F32(1.5))]
                );
            }
            other => panic!("an entity paste queued {other:?}"),
        }
    }

    /// An entity paste whose component carries a token this build cannot parse
    /// names the field it dropped, exactly as the single-component paste does:
    /// a lossy paste says what it lost.
    #[test]
    fn an_entity_paste_names_the_field_it_could_not_read() {
        let text = concat!(
            r#"{"pill": 1, "kind": "entity", "components": ["#,
            r#"{"type": "Demo", "fields": [["first", "1.5f"], ["second", "nonsense"]]}]}"#
        );
        let payload = decode(text).expect("a well-formed payload");
        let plan = paste_plan(
            &payload,
            &selection_with(Entity::from_raw(1, 0)),
            &snapshot_with(&["Demo"]),
        )
        .expect("the paste plans");
        assert_eq!(plan.skipped, vec!["Demo.second".to_string()]);
        match &plan.commands[0] {
            EditorCommand::CreateEntity { components } => {
                assert_eq!(
                    components[0].fields,
                    vec![("first".to_string(), FieldValue::F32(1.5))],
                    "the unparsable field is left out of the seed"
                );
            }
            other => panic!("an entity paste queued {other:?}"),
        }
    }

    /// A component paste is `AddComponent` plus one `SetField` per field: one
    /// recorded step, undone in one.
    #[test]
    fn a_component_paste_is_one_step() {
        let entity = Entity::from_raw(2, 0);
        let text = encode_component(
            "Position",
            &[
                ("x".to_string(), FieldValue::F32(4.0)),
                ("y".to_string(), FieldValue::F32(5.0)),
            ],
        );
        let payload = decode(&text).expect("a well-formed payload");
        let plan = paste_plan(
            &payload,
            &selection_with(entity),
            &snapshot_with(&["Position"]),
        )
        .expect("the paste plans");
        assert_eq!(plan.label, "paste Position");
        assert_eq!(plan.commands.len(), 3);
        assert!(matches!(
            plan.commands[0],
            EditorCommand::AddComponent { entity: queued, .. } if queued == entity
        ));
        let fields: Vec<&str> = plan
            .commands
            .iter()
            .skip(1)
            .filter_map(|command| match command {
                EditorCommand::SetField { field, .. } => Some(field.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(fields, vec!["x", "y"]);
    }

    /// An entity that already carries the component takes the values over it:
    /// no `AddComponent`, which would fail and cost the step its recording.
    #[test]
    fn a_component_paste_over_a_carried_component_only_writes_fields() {
        let entity = Entity::from_raw(5, 0);
        let mut snapshot = snapshot_with(&["Position"]);
        snapshot.detail = Some(EntityDetail {
            entity,
            components: vec![ComponentView {
                asset_labels: Vec::new(),
                entity_labels: Vec::new(),
                type_name: "Position".to_string(),
                persistable: true,
                editable: true,
                fields: vec![field_view("x")],
                values: vec![FieldValue::F32(1.0)],
            }],
        });
        let text = encode_component("Position", &[("x".to_string(), FieldValue::F32(9.0))]);
        let payload = decode(&text).expect("a well-formed payload");
        let plan =
            paste_plan(&payload, &selection_with(entity), &snapshot).expect("the paste plans");
        assert_eq!(plan.commands.len(), 1);
        assert!(matches!(
            plan.commands[0],
            EditorCommand::SetField { entity: queued, .. } if queued == entity
        ));
    }

    /// A component paste needs a selected entity, and says so.
    #[test]
    fn a_component_paste_without_a_selection_is_refused() {
        let text = encode_component("Position", &[("x".to_string(), FieldValue::F32(1.0))]);
        let payload = decode(&text).expect("a well-formed payload");
        let error = paste_plan(
            &payload,
            &EditorSelection::default(),
            &snapshot_with(&["Position"]),
        )
        .expect_err("an empty selection cannot take a component");
        assert!(error.contains("selected entity"), "{error}");
    }

    /// A field paste writes the primary alone; an asset paste only moves the
    /// selection, so it queues no command.
    #[test]
    fn a_field_paste_writes_one_field_and_an_asset_paste_writes_none() {
        let entity = Entity::from_raw(3, 0);
        let text = encode_field("Position", "y", &FieldValue::F32(-1.0)).expect("carried");
        let payload = decode(&text).expect("a well-formed payload");
        let plan = paste_plan(
            &payload,
            &selection_with(entity),
            &snapshot_with(&["Position"]),
        )
        .expect("the paste plans");
        assert!(matches!(
            plan.commands.as_slice(),
            [EditorCommand::SetField { entity: queued, field, .. }] if *queued == entity && field == "y"
        ));

        let asset = decode(&encode_asset("res/models/fox.gltf")).expect("a well-formed payload");
        let plan = paste_plan(&asset, &selection_with(entity), &snapshot_with(&[]))
            .expect("the paste plans");
        assert!(plan.commands.is_empty());
        assert!(plan.note.contains("res/models/fox.gltf"), "{}", plan.note);
    }
}
