//! The comment-preserving editor for a project's settings file.
//!
//! # Responsibilities
//!
//! - [`set_setting`]: put one value at a dotted path in a settings file's text,
//!   adding the groups the path names when they are not there yet.
//! - [`remove_setting`]: take one value out, and take the groups an emptied
//!   section leaves behind with it.
//!
//! # Design
//!
//! `project_settings.yaml` belongs to the person who wrote it: its comments
//! explain choices, its key order is deliberate, and a settings save must not
//! rewrite any of that. So this module never round-trips the file through a
//! YAML value: it works on the lines, finds the one line that holds the value,
//! and replaces only the bytes after its `key:`. Every other byte of the file -
//! comments, blank lines, indentation, the order of unrelated keys - is copied
//! through untouched.
//!
//! The shape this handles is the one a settings file has: mappings of scalars
//! nested two spaces per level, with lists and inline tables as leaf values.
//! A line is a mapping entry when it reads `key:` or `key: value` after its
//! indentation; a line whose value is empty opens a block, and the block runs
//! until a line indented no deeper than its own key. Comment-only and blank
//! lines inside a block belong to it.
//!
//! A value typed into a row arrives as text. It is read as YAML first, so
//! `2.5`, `true` and `[a, b]` keep their kinds, and stored as a quoted string
//! when it is not a YAML value at all.

// Standard library
use std::ops::Range;

// External crates
use serde_yaml::Value;

// =============================================================================
// Constants
// =============================================================================

/// The indentation one nesting level adds, in spaces.
const INDENT_STEP: usize = 2;

// =============================================================================
// Line kinds
// =============================================================================

/// What a line holds, once its indentation is taken off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    /// A `key:` line that opens a block.
    Group,
    /// A `key: value` line.
    Value,
    /// A comment or an empty line.
    Filler,
    /// Anything else: a list item, a continuation, a quoted key.
    Other,
}

// =============================================================================
// Public API
// =============================================================================

/// Writes `value_text` at `dotted_path` in `text`, keeping every other byte.
///
/// `dotted_path` is the full path inside the file, entry first:
/// `modules.pill_audio.audio.ambient_sinks` or `host.logging.level`.
///
/// A path whose groups are missing is created at the end of the block that
/// already exists closest to it, so a new module's settings land beside the
/// modules that are already listed.
///
/// # Errors
///
/// Returns a message naming the path when it is malformed, when it runs into a
/// group where it expects a value, or into a value where it expects a group.
pub fn set_setting(text: &str, dotted_path: &str, value_text: &str) -> Result<String, String> {
    let segments = path_segments(dotted_path)?;
    let rendered = render_value(value_text);
    let mut lines = split_lines(text);

    // Step 1: Walk the groups the path names. `parent_indent` is `None` above
    // the file's top level, and `block` is the line range the next segment is
    // looked for in.
    let mut parent_indent: Option<usize> = None;
    let mut block = 0..lines.len();
    for (depth, segment) in segments[..segments.len() - 1].iter().enumerate() {
        let Some(index) = find_entry(&lines, block.clone(), parent_indent, segment) else {
            insert_group(
                &mut lines,
                block.end,
                parent_indent,
                &segments[depth..],
                rendered.as_str(),
            );
            return Ok(join_lines(&lines));
        };
        if kind_of(&lines[index]) == LineKind::Value {
            return Err(format!(
                "`{dotted_path}` runs into a value where `{segment}` has to be a group"
            ));
        }
        parent_indent = Some(indent_of(&lines[index]));
        block = (index + 1)..block_end(&lines, index);
    }

    // Step 2: The value's own line, replaced in place or added under its
    // group.
    let key = segments[segments.len() - 1];
    match find_entry(&lines, block.clone(), parent_indent, key) {
        Some(index) => {
            if kind_of(&lines[index]) == LineKind::Group && !block_is_empty(&lines, index) {
                return Err(format!(
                    "`{dotted_path}` runs into a group where it has to be a value"
                ));
            }
            let comment = trailing_comment(&lines[index]);
            let indent = indent_of(&lines[index]);
            lines[index] = format!("{}{key}: {rendered}{comment}", " ".repeat(indent));
        }
        None => {
            let indent = child_indent(&lines, &block, parent_indent);
            let insert_at = child_insert_index(&lines, &block);
            lines.insert(
                insert_at,
                format!("{}{key}: {rendered}", " ".repeat(indent)),
            );
        }
    }
    Ok(join_lines(&lines))
}

/// Removes the value at `dotted_path` from `text`, keeping every other byte.
///
/// A group whose last entry this was goes too, and so does its own parent when
/// it is emptied in turn, so a reset leaves no empty `fixture:` behind.
///
/// A path the file does not hold leaves the text exactly as it was: reset is
/// idempotent.
///
/// # Errors
///
/// Returns a message naming the path when it is malformed.
pub fn remove_setting(text: &str, dotted_path: &str) -> Result<String, String> {
    let segments = path_segments(dotted_path)?;
    let mut lines = split_lines(text);
    let mut parent_indent: Option<usize> = None;
    let mut block = 0..lines.len();
    // The header line of each group the path goes through, deepest last: an
    // emptied group takes its own header out, and its parent may follow.
    let mut headers: Vec<usize> = Vec::new();
    for segment in &segments[..segments.len() - 1] {
        let Some(index) = find_entry(&lines, block.clone(), parent_indent, segment) else {
            return Ok(join_lines(&lines));
        };
        parent_indent = Some(indent_of(&lines[index]));
        block = (index + 1)..block_end(&lines, index);
        headers.push(index);
    }
    let key = segments[segments.len() - 1];
    let Some(index) = find_entry(&lines, block, parent_indent, key) else {
        return Ok(join_lines(&lines));
    };
    let removed_indent = indent_of(&lines[index]);
    lines.remove(index);
    // The comment block that introduces the removed key explains that key, so
    // it goes with it; a blank line ends the block.
    let mut cursor = index;
    while cursor > 0 {
        let previous = &lines[cursor - 1];
        let is_comment = previous.trim_start().starts_with('#');
        if !is_comment || indent_of(previous) < removed_indent {
            break;
        }
        lines.remove(cursor - 1);
        cursor -= 1;
    }

    // The header list is deepest last, so walk it backwards: each removal can
    // empty the group above it in turn.
    for header in headers.into_iter().rev() {
        if block_is_empty(&lines, header) {
            lines.remove(header);
        } else {
            break;
        }
    }
    Ok(join_lines(&lines))
}

// =============================================================================
// Line handling
// =============================================================================

/// Splits the text into lines without their terminators.
fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

/// Joins the lines back into a document, ending it with one newline.
fn join_lines(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The number of leading spaces of one line.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// What kind of line this is.
fn kind_of(line: &str) -> LineKind {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return LineKind::Filler;
    }
    if trimmed.starts_with('-') {
        return LineKind::Other;
    }
    let Some(colon) = trimmed.find(':') else {
        return LineKind::Other;
    };
    let key = &trimmed[..colon];
    if key.is_empty() || key.contains(' ') || key.contains('\t') {
        return LineKind::Other;
    }
    if trimmed[colon + 1..].trim().is_empty() {
        LineKind::Group
    } else {
        LineKind::Value
    }
}

/// The comment at the end of a line, with the whitespace in front of it; empty
/// when the line carries none.
///
/// A `#` inside a quoted value is part of that value, so quotes are counted up
/// to the hash before it is taken as a comment.
fn trailing_comment(line: &str) -> String {
    let mut in_single = false;
    let mut in_double = false;
    for (index, character) in line.char_indices() {
        match character {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => {
                let start = line[..index].trim_end().len();
                return line[start..].to_string();
            }
            _ => {}
        }
    }
    String::new()
}

/// The key of a mapping line, without its colon.
fn key_of(line: &str) -> Option<&str> {
    match kind_of(line) {
        LineKind::Group | LineKind::Value => {
            let trimmed = line.trim_start();
            Some(&trimmed[..trimmed.find(':')?])
        }
        LineKind::Filler | LineKind::Other => None,
    }
}

/// The index of the entry named `key` inside `block`, at the indentation a
/// child of `parent_indent` takes.
fn find_entry(
    lines: &[String],
    block: Range<usize>,
    parent_indent: Option<usize>,
    key: &str,
) -> Option<usize> {
    let want = parent_indent.map_or(0, |parent| parent + INDENT_STEP);
    for index in block {
        let Some(line) = lines.get(index) else {
            break;
        };
        if indent_of(line) != want {
            continue;
        }
        if key_of(line) == Some(key) {
            return Some(index);
        }
    }
    None
}

/// The first line after the entry at `header` that is not part of its block.
fn block_end(lines: &[String], header: usize) -> usize {
    let indent = indent_of(&lines[header]);
    for (index, line) in lines.iter().enumerate().skip(header + 1) {
        if kind_of(line) == LineKind::Filler {
            continue;
        }
        if indent_of(line) <= indent {
            return index;
        }
    }
    lines.len()
}

/// Whether the entry at `header` has no entries of its own left.
fn block_is_empty(lines: &[String], header: usize) -> bool {
    let end = block_end(lines, header);
    ((header + 1)..end).all(|index| kind_of(&lines[index]) == LineKind::Filler)
}

/// The indentation a new entry under `block` takes: the one its siblings use,
/// or two spaces deeper than its parent.
fn child_indent(lines: &[String], block: &Range<usize>, parent_indent: Option<usize>) -> usize {
    for index in block.clone() {
        let Some(line) = lines.get(index) else {
            break;
        };
        if kind_of(line) == LineKind::Filler {
            continue;
        }
        return indent_of(line);
    }
    parent_indent.map_or(0, |parent| parent + INDENT_STEP)
}

/// Where a new entry goes: after the last line of the block that holds
/// something, so a new key lands below the existing ones and above a comment
/// block that closes the group.
fn child_insert_index(lines: &[String], block: &Range<usize>) -> usize {
    let mut insert_at = block.start;
    for index in block.clone() {
        let Some(line) = lines.get(index) else {
            break;
        };
        if kind_of(line) != LineKind::Filler {
            insert_at = index + 1;
        }
    }
    insert_at
}

/// Writes the missing tail of a path at `insert_at`: one header per group, the
/// last segment as a value under the deepest one.
fn insert_group(
    lines: &mut Vec<String>,
    insert_at: usize,
    parent_indent: Option<usize>,
    segments: &[&str],
    value: &str,
) {
    let (headers, key) = segments.split_at(segments.len() - 1);
    let mut indent = parent_indent.map_or(0, |parent| parent + INDENT_STEP);
    let mut written: Vec<String> = Vec::new();
    for header in headers {
        written.push(format!("{}{header}:", " ".repeat(indent)));
        indent += INDENT_STEP;
    }
    written.push(format!("{}{}: {value}", " ".repeat(indent), key[0]));
    for (offset, line) in written.into_iter().enumerate() {
        lines.insert(insert_at + offset, line);
    }
}

/// The path's segments, refusing anything a settings path cannot be.
fn path_segments(dotted_path: &str) -> Result<Vec<&str>, String> {
    let segments: Vec<&str> = dotted_path.split('.').collect();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return Err(format!(
            "`{dotted_path}` is not a setting path: it has to be an entry and a key, dotted"
        ));
    }
    Ok(segments)
}

// =============================================================================
// Values
// =============================================================================

/// The YAML text for a value typed into a row.
///
/// The text is read as YAML when it can be, so `2.5`, `true` and `[a, b]` keep
/// their kinds; anything else is stored as the string it looks like. Text that
/// reads as nothing at all writes an empty string, which is how a row that was
/// cleared reads back.
fn render_value(value_text: &str) -> String {
    let trimmed = value_text.trim();
    if trimmed.is_empty() {
        return "\"\"".to_string();
    }
    let value = match serde_yaml::from_str::<Value>(trimmed) {
        Ok(value @ (Value::Bool(_) | Value::Number(_) | Value::Sequence(_))) => value,
        // A plain string, an inline table, or a document the reader refused
        // (`a: b`, a stray bracket): store the typed text as text rather than
        // failing the save.
        _ => Value::String(trimmed.to_string()),
    };
    let rendered = serde_yaml::to_string(&value).unwrap_or_else(|_| trimmed.to_string());
    rendered.trim_end().to_string()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with a comment above the entry an edit lands in, one inside the
    /// group it adds to, and two modules in a deliberate order.
    const SETTINGS: &str = "\
name: \"Native Test Project\"
# The modules this run loads, in load order.
modules:
  pill_spline:
    enabled: true
  # The fixture's own values.
  pill_test:
    enabled: true
    fixture:
      # How loud the fixture is.
      gain: 1.5
      label: fixture
host:
  logging:
    targets: {}
";

    #[test]
    fn a_set_keeps_every_comment_and_the_key_order() {
        let written = set_setting(SETTINGS, "modules.pill_test.fixture.gain", "2.5").unwrap();
        assert!(written.contains("# The modules this run loads, in load order."));
        assert!(written.contains("# The fixture's own values."));
        assert!(written.contains("# How loud the fixture is."));
        assert!(written.contains("      gain: 2.5"));
        assert!(!written.contains("gain: 1.5"));
        let gain = written.find("gain:").unwrap();
        let label = written.find("label:").unwrap();
        assert!(gain < label, "the edited key keeps its place");
    }

    #[test]
    fn a_set_keeps_the_comment_at_the_end_of_the_line() {
        let text =
            "modules:\n  pill_audio:\n    audio:\n      ambient_sinks: 16  # device default\n";
        let written = set_setting(text, "modules.pill_audio.audio.ambient_sinks", "32").unwrap();
        assert_eq!(
            written,
            "modules:\n  pill_audio:\n    audio:\n      ambient_sinks: 32  # device default\n"
        );
    }

    #[test]
    fn a_set_adds_a_key_below_its_siblings() {
        let written = set_setting(SETTINGS, "modules.pill_test.fixture.decay", "0.25").unwrap();
        assert!(written.contains("      gain: 1.5\n      label: fixture\n      decay: 0.25\n"));
    }

    #[test]
    fn a_set_creates_the_groups_a_new_entry_needs() {
        let written = set_setting(SETTINGS, "modules.pill_audio.audio.ambient_sinks", "8").unwrap();
        assert!(
            written.contains(
                "      label: fixture\n  pill_audio:\n    audio:\n      ambient_sinks: 8\nhost:"
            ),
            "the new module lands after the last listed one:\n{written}"
        );
        assert_eq!(written.matches("\nmodules:").count(), 1);
    }

    #[test]
    fn a_set_of_text_quotes_what_needs_quoting() {
        let written = set_setting(SETTINGS, "modules.pill_test.fixture.label", "a: b").unwrap();
        assert!(written.contains("label: 'a: b'"), "{written}");
        let plain = set_setting(SETTINGS, "modules.pill_test.fixture.label", "plain").unwrap();
        assert!(plain.contains("label: plain\n"));
    }

    #[test]
    fn a_set_of_a_flag_or_a_number_keeps_its_kind() {
        let written = set_setting(SETTINGS, "modules.pill_test.enabled", "false").unwrap();
        assert!(written.contains("    enabled: false"));
        let number = set_setting(SETTINGS, "modules.pill_spline.enabled", "0").unwrap();
        assert!(number.contains("    enabled: 0"));
    }

    #[test]
    fn a_remove_takes_the_key_and_the_groups_it_empties() {
        let text =
            "modules:\n  pill_audio:\n    audio:\n      ambient_sinks: 16\nhost:\n  hot_reloading: true\n";
        let written = remove_setting(text, "modules.pill_audio.audio.ambient_sinks").unwrap();
        assert_eq!(written, "host:\n  hot_reloading: true\n");
    }

    #[test]
    fn a_remove_keeps_a_group_that_still_has_a_key() {
        let written = remove_setting(SETTINGS, "modules.pill_test.fixture.gain").unwrap();
        assert!(written.contains("    fixture:\n      label: fixture\n"));
        assert!(!written.contains("gain"));
    }

    #[test]
    fn a_remove_of_a_missing_path_changes_nothing() {
        let written = remove_setting(SETTINGS, "modules.pill_audio.audio.ambient_sinks").unwrap();
        assert_eq!(written, SETTINGS);
    }

    #[test]
    fn a_set_then_a_remove_returns_the_file_to_its_shape() {
        let added = set_setting(SETTINGS, "modules.pill_test.fixture.decay", "0.25").unwrap();
        let removed = remove_setting(&added, "modules.pill_test.fixture.decay").unwrap();
        assert_eq!(removed, SETTINGS);
    }

    #[test]
    fn a_malformed_path_is_refused() {
        assert!(set_setting(SETTINGS, "modules", "1").is_err());
        assert!(set_setting(SETTINGS, "modules..gain", "1").is_err());
        assert!(remove_setting(SETTINGS, "").is_err());
    }
}
