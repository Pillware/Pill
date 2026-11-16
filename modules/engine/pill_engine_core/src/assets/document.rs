//! The YAML document format every saved document shares.
//!
//! # Responsibilities
//!
//! - [`parse`]: one document's bytes - a `.meta` sidecar, a standalone asset
//!   file, or one of the editor's own state files - as the untyped value tree
//!   every reader works in.
//! - [`render`]: one document as the YAML text the engine writes, with a
//!   trailing newline and no document marker.
//!
//! # Design
//!
//! A `.meta` sidecar and a standalone asset file are both YAML documents with
//! the same header - `format_version`, `asset_type` and `guid` - followed by
//! the type's body under `settings` or `asset`. The editor's state files (dock
//! layout, preferences, keybindings, recovery snapshot) are plain YAML
//! mappings without that header, and read and write through the same two
//! functions. Keeping one parser and one writer here means the engine, the
//! development host and the editors cannot drift apart on the file format.
//!
//! A document is read into [`serde_json::Value`], not into a typed structure:
//! the readers above this module know the body's field names but not the
//! type's Rust shape, and each asset type converts the body through its own
//! serde implementation. YAML and this value model agree on every value an
//! asset file uses: mappings, sequences, strings, integers, floats, booleans
//! and null.

// External crates
use serde::Serialize;

/// Parses one asset document from `bytes`.
///
/// # Errors
///
/// A message saying the bytes are not a YAML document.
pub fn parse(bytes: &[u8]) -> Result<serde_json::Value, String> {
    serde_yaml::from_slice(bytes).map_err(|error| format!("not valid YAML: {error}"))
}

/// Renders `document` as the YAML text an asset file holds.
///
/// The text ends with exactly one newline and carries no `---` document
/// marker, so a file the engine writes has the same shape as one a person
/// edits.
///
/// # Errors
///
/// A message when `document` cannot be written as YAML.
pub fn render<T>(document: &T) -> Result<Vec<u8>, String>
where
    T: Serialize + ?Sized,
{
    let text = serde_yaml::to_string(document)
        .map_err(|error| format!("the document cannot be written as YAML: {error}"))?;
    let text = text.strip_prefix("---\n").unwrap_or(&text);
    let mut bytes = text.trim_end_matches('\n').as_bytes().to_vec();
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_round_trips_without_a_marker() {
        let document = serde_json::json!({
            "format_version": 1,
            "asset_type": "test::Thing",
            "guid": "9c0e4b7a1f2d3e4c5b6a798812345678",
            "asset": { "speed": 1.5, "name": "walk", "flags": [true, false] },
        });
        let bytes = render(&document).expect("the document renders");
        let text = String::from_utf8(bytes.clone()).expect("the text is UTF-8");
        assert!(!text.starts_with("---"), "no document marker: {text}");
        assert!(text.ends_with('\n'), "one trailing newline: {text}");
        assert_eq!(parse(&bytes).expect("the text parses"), document);
    }

    #[test]
    fn a_value_that_looks_like_another_type_stays_a_string() {
        let document = serde_json::json!({ "guid": "12345678901234567890123456789012" });
        let bytes = render(&document).expect("the document renders");
        assert_eq!(parse(&bytes).expect("the text parses"), document);
    }

    #[test]
    fn text_that_is_not_yaml_is_refused() {
        assert!(parse(b"{ \"unterminated\": ").is_err());
    }

    #[test]
    fn a_scalar_yaml_1_1_would_call_a_boolean_stays_a_string() {
        let document = parse(b"value: on\nother: no\n").expect("the text parses");
        assert_eq!(document["value"], serde_json::json!("on"));
        assert_eq!(document["other"], serde_json::json!("no"));
    }
}
