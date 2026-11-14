//! The Preferences tab's shared vocabulary: the preferences file's editable
//! keys, the rules that turn row text into JSON, and the helpers that read and
//! write the document.
//!
//! # Responsibilities
//!
//! - [`PREFERENCE_FIELDS`]: the editable rows the Preferences tab draws, in row
//!   order. Each row names its key in the file, the label it shows, and the
//!   text a file without that key shows.
//! - [`PreferencesView`]: the file's values as display strings, plus the parsed
//!   icon size and the project's remembered gizmo entry. The tab seeds its
//!   buffers from this view.
//! - [`parse_value`], [`set_key`], [`read_document`], [`write_document`]: the
//!   text-to-JSON rule and the dotted-key document access the `SetPreferences`
//!   command applies.
//! - [`keymap_preset`]: the navigation preset the file selects, or Blender
//!   when the file says nothing usable.
//! - [`gizmo_hit_radius`], [`gizmo_icon_size`]: one clamped number each, with
//!   the default a missing key falls back to.
//! - [`update`]: reads, edits and writes the document in one step, for a caller
//!   that updates only the keys it owns.
//!
//! # Design
//!
//! The preferences file is a YAML mapping, and the editor does not own it
//! alone. The tab's save writes the editor-wide rows, and the executable
//! writes the per-project gizmo entry. Every write therefore reads the
//! document first and changes only the keys its owner owns, so a key the other
//! writer left behind survives the save.
//!
//! A row holds the text the user typed. [`parse_value`] tries that text as
//! bare JSON first, so a number or a boolean round-trips, and stores it as a
//! string otherwise. A blank row writes nothing, so a key that was never set
//! stays absent and its reader keeps its own default.

// Standard library
use std::collections::BTreeMap;
use std::path::Path;

// External crates
use serde_json::{Map, Value};

// Current crate
use crate::commands::{GizmoProjectPreferences, KeymapPreset, PREFERENCES_FILE};

// =============================================================================
// Constants
// =============================================================================

/// The preference key the editor's user theme layer is named under.
///
/// The value is a `.theme`, `.yaml` or `.yml` file, either absolute or
/// relative to the editor's state directory.
pub const THEME_PREFERENCE_KEY: &str = "theme";

/// The preference key the keymap preset lives under.
pub const KEYMAP_PRESET_KEY: &str = "keymap_preset";

/// The preference key the gizmo's hit radius lives under.
pub const GIZMO_HIT_RADIUS_KEY: &str = "editor.gizmo_hit_radius";

/// The hit radius used when the file says nothing: about the width the overlay
/// draws a gizmo line with.
pub const GIZMO_HIT_RADIUS_DEFAULT: f32 = 0.07;

/// The range a file's hit radius is clamped into, as a multiplier on the
/// handle length: tight enough to still mean a handle, wide enough to grab
/// from.
pub const GIZMO_HIT_RADIUS_RANGE: (f32, f32) = (0.01, 0.35);

/// The preference key the scene badges' icon size lives under.
pub const GIZMO_ICON_SIZE_KEY: &str = "editor.gizmo_icon_size";

/// The icon size used when the file says nothing, in points.
///
/// It matches the Preferences row's fallback text, so a file without the key
/// shows the size the badges are drawn with.
pub const GIZMO_ICON_SIZE_DEFAULT: f32 = 20.0;

/// The range a file's icon size is clamped into, in points: small enough to
/// stay a mark over the scene, large enough to keep it clickable.
pub const GIZMO_ICON_SIZE_RANGE: (f32, f32) = (8.0, 64.0);

/// The editable rows, in the order the tab draws them.
pub const PREFERENCE_FIELDS: [PreferenceField; 12] = [
    PreferenceField {
        key: KEYMAP_PRESET_KEY,
        label: "keymap preset",
        fallback: "",
    },
    PreferenceField {
        key: crate::previews::RENDER_PREVIEWS_KEY,
        label: "render asset previews",
        fallback: "true",
    },
    PreferenceField {
        key: GIZMO_HIT_RADIUS_KEY,
        label: "gizmo hit radius",
        fallback: "0.07",
    },
    PreferenceField {
        key: GIZMO_ICON_SIZE_KEY,
        label: "gizmo icon size",
        // The fallback repeats `GIZMO_ICON_SIZE_DEFAULT`, so a file without the
        // key shows the size the badges are drawn with.
        fallback: "20",
    },
    PreferenceField {
        key: "code_editor_command",
        label: "code editor command",
        fallback: "",
    },
    PreferenceField {
        key: "autosave_interval_seconds",
        label: "autosave seconds",
        fallback: "",
    },
    PreferenceField {
        key: "snapping_steps.translation",
        label: "snap translate step",
        fallback: "0.5",
    },
    PreferenceField {
        key: "snapping_steps.rotation_degrees",
        label: "snap rotate degrees",
        fallback: "15",
    },
    PreferenceField {
        key: "snapping_steps.scale",
        label: "snap scale step",
        fallback: "0.1",
    },
    PreferenceField {
        key: "history_step_cap",
        label: "undo step cap",
        fallback: "",
    },
    PreferenceField {
        key: "history_memory_budget_mb",
        label: "undo memory MB",
        fallback: "",
    },
    PreferenceField {
        key: THEME_PREFERENCE_KEY,
        label: "theme file",
        fallback: "",
    },
];

// =============================================================================
// Types
// =============================================================================

/// The tab's unsaved field buffers: one text per editable key.
pub type PreferencesDraft = BTreeMap<String, String>;

/// One editable preference: the document key, the row's label and the text a
/// file without the key shows.
pub struct PreferenceField {
    /// The file's key; a dot nests into an object.
    pub key: &'static str,
    /// The label the row shows.
    pub label: &'static str,
    /// The text the row shows when the file has no value for the key, which is
    /// the value the key's reader falls back to.
    pub fallback: &'static str,
}

/// The file's values as the tab shows them.
///
/// It holds one display string per editable key, the parsed icon size the
/// badges are drawn with, and the project's remembered gizmo entry.
#[derive(Clone, Debug, PartialEq)]
pub struct PreferencesView {
    /// One display string per editable field, keyed like the fields.
    pub values: BTreeMap<String, String>,
    /// The scene badges' icon size, in points. The shell draws with the number
    /// rather than with the row's text, so the view parses it from the same
    /// document the rows come from.
    pub gizmo_icon_size: f32,
    /// The project key the per-project entry is remembered under.
    pub project: Option<String>,
    /// The project's remembered gizmo entry, as the file holds it. A file with
    /// no entry for this project leaves it at the defaults.
    pub project_gizmo: GizmoProjectPreferences,
    /// Whether the file carries the project's entry at all.
    pub project_remembered: bool,
}

impl Default for PreferencesView {
    fn default() -> Self {
        Self {
            values: BTreeMap::new(),
            gizmo_icon_size: GIZMO_ICON_SIZE_DEFAULT,
            project: None,
            project_gizmo: GizmoProjectPreferences::default(),
            project_remembered: false,
        }
    }
}

impl PreferencesView {
    /// Builds the view from one parsed document.
    ///
    /// `project` names the per-project entry to read from `gizmo.projects`. A
    /// name the file does not hold leaves `project_gizmo` at its defaults and
    /// `project_remembered` false.
    pub fn from_document(document: &Map<String, Value>, project: Option<&str>) -> Self {
        // Step 1: build one display text per editable row, or use the row's
        // fallback.
        let row_text_by_key = PREFERENCE_FIELDS
            .iter()
            .map(|field| {
                let text = lookup(document, field.key)
                    .map(display_value)
                    .unwrap_or_else(|| field.fallback.to_string());
                (field.key.to_string(), text)
            })
            .collect();
        // Step 2: the project's own entry, when the file holds one.
        let project_entry = project.and_then(|name| {
            let projects = lookup(document, "gizmo.projects")?.as_object()?;
            projects.get(name)
        });
        let project_gizmo = project_entry
            .and_then(|entry| serde_json::from_value(entry.clone()).ok())
            .unwrap_or_default();
        Self {
            values: row_text_by_key,
            gizmo_icon_size: gizmo_icon_size(document),
            project: project.map(str::to_string),
            project_gizmo,
            project_remembered: project_entry.is_some(),
        }
    }

    /// Returns a copy of the rows the tab's draft buffers start from.
    pub fn seed_buffers(&self) -> BTreeMap<String, String> {
        self.values.clone()
    }

    /// Returns one row's display text, or an empty string when the table has
    /// no such row.
    pub fn text(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the keymap preset the file selects.
///
/// A key that is absent, blank or unknown selects the default, Blender. The
/// viewport navigation chords are resolved from the result.
pub fn keymap_preset(document: &Map<String, Value>) -> KeymapPreset {
    let Some(Value::String(name)) = lookup(document, KEYMAP_PRESET_KEY) else {
        return KeymapPreset::default();
    };
    match name.trim().to_ascii_lowercase().as_str() {
        "blender" => KeymapPreset::Blender,
        "unity" => KeymapPreset::Unity,
        "godot" => KeymapPreset::Godot,
        "unreal" => KeymapPreset::Unreal,
        _ => KeymapPreset::default(),
    }
}

/// Returns the gizmo handle's hit radius the file asks for, as a multiplier on
/// the handle length.
///
/// The hover test and the press both read this one number, so a handle that
/// highlights under the pointer is a handle a press grabs. A missing, blank or
/// unparsable value keeps the default, and a value outside
/// [`GIZMO_HIT_RADIUS_RANGE`] is clamped: a zero radius would make the gizmo
/// ungrabbable.
pub fn gizmo_hit_radius(document: &Map<String, Value>) -> f32 {
    let parsed_radius = match lookup(document, GIZMO_HIT_RADIUS_KEY) {
        Some(Value::Number(number)) => number.as_f64().map(|number| number as f32),
        Some(Value::String(text)) => text.trim().parse::<f32>().ok(),
        _ => None,
    };
    parsed_radius
        .filter(|radius| radius.is_finite())
        .unwrap_or(GIZMO_HIT_RADIUS_DEFAULT)
        .clamp(GIZMO_HIT_RADIUS_RANGE.0, GIZMO_HIT_RADIUS_RANGE.1)
}

/// Returns the scene badges' icon size the file asks for, in points.
///
/// The mark and the rectangle that picks it both scale with this one number.
/// A missing, blank or unparsable value keeps the default, and a value outside
/// [`GIZMO_ICON_SIZE_RANGE`] is clamped: a zero or negative size would draw
/// nothing to click on.
pub fn gizmo_icon_size(document: &Map<String, Value>) -> f32 {
    let parsed_size = match lookup(document, GIZMO_ICON_SIZE_KEY) {
        Some(Value::Number(number)) => number.as_f64().map(|number| number as f32),
        Some(Value::String(text)) => text.trim().parse::<f32>().ok(),
        _ => None,
    };
    parsed_size
        .filter(|size| size.is_finite())
        .unwrap_or(GIZMO_ICON_SIZE_DEFAULT)
        .clamp(GIZMO_ICON_SIZE_RANGE.0, GIZMO_ICON_SIZE_RANGE.1)
}

/// Follows a dotted key through the document.
///
/// Returns `None` when a segment is absent, or when a segment before the last
/// one holds something other than a JSON object.
pub fn lookup<'a>(document: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    let mut segments = key.split('.').peekable();
    let mut current_object = document;
    while let Some(segment) = segments.next() {
        let segment_value = current_object.get(segment)?;
        if segments.peek().is_none() {
            return Some(segment_value);
        }
        current_object = segment_value.as_object()?;
    }
    None
}

/// Returns one JSON value as the tab's text: a string verbatim, and anything
/// else as compact JSON.
fn display_value(json_value: &Value) -> String {
    match json_value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Parses one row's text into JSON.
///
/// The trimmed text is tried as bare JSON first, so a number, a boolean or an
/// array round-trips. When that fails, the text is quoted and parsed as a JSON
/// string. Text that still does not parse (it holds an unescaped quote, for
/// example) is returned as plain text.
pub fn parse_value(text: &str) -> Value {
    let trimmed = text.trim();
    serde_json::from_str::<Value>(trimmed)
        .or_else(|_| serde_json::from_str::<Value>(&format!("\"{trimmed}\"")))
        .unwrap_or_else(|_| Value::String(trimmed.to_string()))
}

/// Sets one dotted key, creating intermediate objects on the way.
///
/// # Errors
///
/// Returns a message when the key is empty, or when a segment on the way is
/// present and holds something other than an object.
pub fn set_key(document: &mut Map<String, Value>, key: &str, value: Value) -> Result<(), String> {
    // Step 1: refuse a key with nothing in it.
    if key.is_empty() {
        return Err(format!("`{key}` is empty"));
    }
    let segments: Vec<&str> = key.split('.').collect();
    let Some((last, parents)) = segments.split_last() else {
        return Err(format!("`{key}` is empty"));
    };
    // Step 2: walk the parent segments, creating each object that is missing.
    let mut current = document;
    for segment in parents {
        let intermediate_object = current
            .entry(segment.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        current = intermediate_object
            .as_object_mut()
            .ok_or_else(|| format!("`{segment}` in `{key}` is not an object"))?;
    }
    // Step 3: write the value under the last segment.
    current.insert(last.to_string(), value);
    Ok(())
}

/// Reads the preferences document a write will modify.
///
/// A missing file is an empty document, so the first save creates it. A file
/// that exists and cannot be read or parsed is an error instead. Reading it as
/// empty would let the next save write a file holding only the keys it set,
/// erasing every unrelated preference.
///
/// # Errors
///
/// Returns a message naming the file when it cannot be read, is not YAML, or is
/// not a YAML mapping.
pub fn read_document(directory: &Path) -> Result<Map<String, Value>, String> {
    let path = directory.join(PREFERENCES_FILE);
    match crate::editor_files::read_yaml::<Value>(&path) {
        Ok(Some(Value::Object(document))) => Ok(document),
        Ok(Some(_)) => Err(format!("{PREFERENCES_FILE} is not a YAML mapping")),
        Ok(None) => Ok(Map::new()),
        Err(error) => Err(error.message()),
    }
}

/// Parses a preferences file's text into its document.
///
/// # Errors
///
/// Returns a message naming the file when the text is not YAML or is not a YAML
/// mapping.
pub fn parse_document(text: &str) -> Result<Map<String, Value>, String> {
    let value: Value = crate::editor_files::parse_yaml(text.as_bytes())
        .map_err(|message| format!("{PREFERENCES_FILE} is {message}"))?;
    match value {
        Value::Object(document) => Ok(document),
        _ => Err(format!("{PREFERENCES_FILE} is not a YAML mapping")),
    }
}

/// Writes the document the way the editor writes its state files. The text
/// goes into a temporary file beside the target, and a rename replaces the
/// target with it. A missing directory is created first.
///
/// # Errors
///
/// Returns a message when the document cannot be serialized or the file cannot
/// be written.
pub fn write_document(directory: &Path, document: &Map<String, Value>) -> Result<(), String> {
    let path = directory.join(PREFERENCES_FILE);
    crate::editor_files::write_yaml(&path, &Value::Object(document.clone()))
        .map_err(|error| error.message())
}

/// Reads the preferences document, applies the edit and writes it back.
///
/// Use this when a caller updates only the keys it owns. The rest of the
/// document is left alone. A document that cannot be read stops the edit
/// instead of being replaced by one holding only the caller's keys.
///
/// # Errors
///
/// Returns the read's or the write's message, or the edit's own error when it
/// refuses.
pub fn update(
    directory: &Path,
    edit: impl FnOnce(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), String> {
    let mut document = read_document(directory)?;
    edit(&mut document)?;
    write_document(directory, &document)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a scratch directory of this test's own under the system temp
    /// directory, named so parallel runs do not collide.
    fn scratch_directory(label: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "ecs-preferences-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// Every editable row's key is one the document helpers can write and read
    /// back. The table, the dotted-key path and the reader therefore agree, so
    /// a duplicated or malformed key fails here instead of becoming a row whose
    /// saved value nothing reads.
    #[test]
    fn every_preference_field_round_trips_through_its_key() {
        for field in &PREFERENCE_FIELDS {
            let mut document = Map::new();
            set_key(&mut document, field.key, serde_json::json!("probe"))
                .unwrap_or_else(|error| panic!("{} cannot be written: {error}", field.key));
            assert_eq!(
                lookup(&document, field.key),
                Some(&serde_json::json!("probe")),
                "{} did not read back what the table wrote",
                field.key
            );
            let view = PreferencesView::from_document(&document, None);
            assert_eq!(view.text(field.key), "probe", "{} has no row", field.key);
        }
    }

    #[test]
    fn text_parses_as_bare_json_first_then_a_string() {
        assert_eq!(parse_value("0.3"), serde_json::json!(0.3));
        assert_eq!(parse_value("42"), serde_json::json!(42));
        assert_eq!(parse_value("true"), serde_json::json!(true));
        assert_eq!(parse_value("code ."), serde_json::json!("code ."));
        assert_eq!(parse_value(""), serde_json::json!(""));
    }

    #[test]
    fn the_gizmo_hit_radius_reads_numbers_strings_and_clamps() {
        let mut document = Map::new();
        // With no key, the default is used.
        assert_eq!(gizmo_hit_radius(&document), GIZMO_HIT_RADIUS_DEFAULT);
        // Both a number and a numeric string are read.
        set_key(&mut document, GIZMO_HIT_RADIUS_KEY, serde_json::json!(0.12)).unwrap();
        assert_eq!(gizmo_hit_radius(&document), 0.12);
        set_key(
            &mut document,
            GIZMO_HIT_RADIUS_KEY,
            serde_json::json!("0.2"),
        )
        .unwrap();
        assert_eq!(gizmo_hit_radius(&document), 0.2);
        // A value outside the range is clamped, and text that is not a number
        // keeps the default.
        set_key(&mut document, GIZMO_HIT_RADIUS_KEY, serde_json::json!(40.0)).unwrap();
        assert_eq!(gizmo_hit_radius(&document), GIZMO_HIT_RADIUS_RANGE.1);
        set_key(
            &mut document,
            GIZMO_HIT_RADIUS_KEY,
            serde_json::json!("wide"),
        )
        .unwrap();
        assert_eq!(gizmo_hit_radius(&document), GIZMO_HIT_RADIUS_DEFAULT);
    }

    #[test]
    fn the_gizmo_icon_size_defaults_and_clamps() {
        let mut document = Map::new();
        // With no key, the default is used.
        assert_eq!(gizmo_icon_size(&document), GIZMO_ICON_SIZE_DEFAULT);
        // Both a number and a numeric string are read.
        set_key(&mut document, GIZMO_ICON_SIZE_KEY, serde_json::json!(28)).unwrap();
        assert_eq!(gizmo_icon_size(&document), 28.0);
        set_key(&mut document, GIZMO_ICON_SIZE_KEY, serde_json::json!("36")).unwrap();
        assert_eq!(gizmo_icon_size(&document), 36.0);
        // A value outside the range is clamped, and text that is not a number
        // keeps the default.
        set_key(&mut document, GIZMO_ICON_SIZE_KEY, serde_json::json!(0)).unwrap();
        assert_eq!(gizmo_icon_size(&document), GIZMO_ICON_SIZE_RANGE.0);
        set_key(&mut document, GIZMO_ICON_SIZE_KEY, serde_json::json!(900)).unwrap();
        assert_eq!(gizmo_icon_size(&document), GIZMO_ICON_SIZE_RANGE.1);
        set_key(&mut document, GIZMO_ICON_SIZE_KEY, serde_json::json!("big")).unwrap();
        assert_eq!(gizmo_icon_size(&document), GIZMO_ICON_SIZE_DEFAULT);
        // The view carries the parsed size beside the row's text.
        set_key(&mut document, GIZMO_ICON_SIZE_KEY, serde_json::json!(24)).unwrap();
        let view = PreferencesView::from_document(&document, None);
        assert_eq!(view.text(GIZMO_ICON_SIZE_KEY), "24");
        assert_eq!(view.gizmo_icon_size, 24.0);
        assert_eq!(
            PreferencesView::default().gizmo_icon_size,
            GIZMO_ICON_SIZE_DEFAULT
        );
    }

    #[test]
    fn set_key_creates_nested_objects_and_refuses_scalar_hops() {
        let mut document = Map::new();
        set_key(&mut document, "history_step_cap", serde_json::json!(42)).unwrap();
        set_key(
            &mut document,
            "snapping_steps.translation",
            serde_json::json!(0.25),
        )
        .unwrap();
        assert_eq!(document["history_step_cap"], serde_json::json!(42));
        assert_eq!(
            document["snapping_steps"]["translation"],
            serde_json::json!(0.25)
        );
        assert_eq!(
            set_key(
                &mut document,
                "history_step_cap.deeper",
                serde_json::json!(1)
            ),
            Err("`history_step_cap` in `history_step_cap.deeper` is not an object".to_string())
        );
        assert_eq!(
            set_key(&mut document, "", serde_json::json!(1)),
            Err("`` is empty".to_string())
        );
    }

    #[test]
    fn the_view_reads_values_fallbacks_and_the_project_entry() {
        let mut document = Map::new();
        set_key(&mut document, "keymap_preset", serde_json::json!("preset")).unwrap();
        set_key(
            &mut document,
            "snapping_steps.translation",
            serde_json::json!(0.25),
        )
        .unwrap();
        set_key(
            &mut document,
            "gizmo.projects.italian_brainrot.mode",
            serde_json::json!("rotate"),
        )
        .unwrap();
        let view = PreferencesView::from_document(&document, Some("italian_brainrot"));
        assert_eq!(view.text("keymap_preset"), "preset");
        assert_eq!(view.text("snapping_steps.translation"), "0.25");
        // Keys the file does not have show the row's fallback text.
        assert_eq!(view.text("snapping_steps.rotation_degrees"), "15");
        assert_eq!(view.text("history_step_cap"), "");
        assert_eq!(view.text("code_editor_command"), "");
        assert!(view.project_remembered);
        assert_eq!(
            view.project_gizmo.mode,
            Some(crate::gizmo::GizmoMode::Rotate)
        );
        // A project the file never saw reads as unremembered defaults.
        let other_project_view = PreferencesView::from_document(&document, Some("project_rs"));
        assert!(!other_project_view.project_remembered);
        assert_eq!(other_project_view.project_gizmo.mode, None);
        assert_eq!(other_project_view.project.as_deref(), Some("project_rs"));
    }

    #[test]
    fn the_document_round_trips_through_the_file() {
        let directory = scratch_directory("round-trip");
        let mut document = read_document(&directory).unwrap();
        assert!(document.is_empty());
        set_key(&mut document, "theme", serde_json::json!("dark")).unwrap();
        set_key(
            &mut document,
            "snapping_steps.scale",
            serde_json::json!(0.2),
        )
        .unwrap();
        write_document(&directory, &document).unwrap();

        let read_back = read_document(&directory).unwrap();
        assert_eq!(read_back["theme"], serde_json::json!("dark"));
        assert_eq!(read_back["snapping_steps"]["scale"], serde_json::json!(0.2));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_corrupt_document_is_refused_rather_than_read_as_empty() {
        // Reading "not YAML" as an empty document would let the next save erase
        // every key the file held, including the ones this write never touched.
        let directory = scratch_directory("corrupt");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(PREFERENCES_FILE);
        let corrupt_text = "theme: [dark\n";
        std::fs::write(&path, corrupt_text).unwrap();

        let error = read_document(&directory).expect_err("an open sequence is not a document");
        assert!(
            error.contains("not valid YAML"),
            "unexpected message: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt_text,
            "the file is left exactly as it was"
        );

        // A missing file is still an empty document, so the first save creates
        // it.
        std::fs::remove_file(&path).unwrap();
        assert!(read_document(&directory)
            .expect("missing reads empty")
            .is_empty());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_document_that_is_not_an_object_is_refused() {
        // A file holding `- 1` parses as YAML but is not a document, so a save
        // that modified it would have nowhere to put a key.
        let error = parse_document("- 1\n- 2\n").expect_err("a list is not a document");
        assert!(
            error.contains("not a YAML mapping"),
            "unexpected message: {error}"
        );
        assert!(parse_document("theme: dark\n")
            .expect("a mapping is a document")
            .contains_key("theme"));
    }

    /// Two saves in a row land: the second document replaces the first.
    #[test]
    fn a_second_save_replaces_the_first_document() {
        let directory = scratch_directory("second-save");
        let mut first_document = Map::new();
        first_document.insert("theme".to_owned(), Value::String("dark".to_owned()));
        write_document(&directory, &first_document).expect("the first save");
        let mut second_document = Map::new();
        second_document.insert("theme".to_owned(), Value::String("light".to_owned()));
        write_document(&directory, &second_document).expect("the second save");
        let stored_document = read_document(&directory).expect("the read");
        assert_eq!(
            lookup(&stored_document, THEME_PREFERENCE_KEY),
            second_document.get("theme")
        );
        assert!(!directory
            .join(PREFERENCES_FILE)
            .with_extension("tmp")
            .exists());
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// An update writes its own key and leaves every other key alone.
    #[test]
    fn an_update_keeps_the_keys_it_does_not_own() {
        let directory = scratch_directory("update");
        std::fs::write(
            directory.join(PREFERENCES_FILE),
            "theme: dark\neditor.history_step_cap: 42\n",
        )
        .expect("the seeded document");
        update(&directory, |document| {
            document.insert("gizmo".to_owned(), Value::String("written".to_owned()));
            Ok(())
        })
        .expect("the update");
        let stored_document = read_document(&directory).expect("the read");
        assert!(stored_document.contains_key("gizmo"));
        assert!(
            stored_document.contains_key("theme"),
            "the update erased `theme`"
        );
        assert!(stored_document.contains_key("editor.history_step_cap"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A document that cannot be parsed stops the update. The file still holds
    /// the caller's own text, and no partial document replaces it.
    #[test]
    fn an_update_refuses_a_document_it_cannot_read() {
        let directory = scratch_directory("update-corrupt");
        let path = directory.join(PREFERENCES_FILE);
        let broken_text = "theme: [dark\n";
        std::fs::write(&path, broken_text).expect("the broken document");
        let error = update(&directory, |document| {
            document.insert("gizmo".to_owned(), Value::String("written".to_owned()));
            Ok(())
        })
        .expect_err("a broken document stops the update");
        assert!(
            error.contains("is not valid YAML"),
            "unexpected message: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("the document"),
            broken_text,
            "the failed update rewrote the document"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
