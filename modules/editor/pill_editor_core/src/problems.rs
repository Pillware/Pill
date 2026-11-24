//! Build diagnostics: the problem rows the editor's Problems tab shows.
//!
//! # Responsibilities
//!
//! - Turn a build's output text into structured problems: the severity, the
//!   message and the first `file:line:column` the compiler printed for it.
//! - Stay dependency-free and total: malformed or unfamiliar output gives
//!   fewer problems, never an error.
//!
//! # Contract
//!
//! The input is rustc's human-readable output, one diagnostic per block. Both
//! one-line (`error: message`) and code-carrying (`error[E0425]: message`)
//! headers are recognised, at any indentation.
//!
//! The location is the first `-->` span line within the next
//! [`SPAN_LOOKAHEAD`] lines, and a blank line ends the search. A coloured
//! header matches nothing, so coloured output gives no problems. Lines like
//! `error: could not compile \`crate\`` summarise the problems above them, so
//! they are dropped.

// =============================================================================
// Constants
// =============================================================================

/// How many lines after a header the parser looks for the span line that
/// gives the location.
const SPAN_LOOKAHEAD: usize = 6;

// =============================================================================
// Types
// =============================================================================

/// How serious one problem is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// A compiler error: the build stopped.
    Error,
    /// A compiler warning: the build finished anyway.
    Warning,
    /// A note the compiler printed as its own `note:` line.
    Note,
}

impl Severity {
    /// The name the problems rows show, and the `severity=` value of the
    /// `[test] problem row` lines.
    pub fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// One problem parsed from a build's output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Error, warning or note.
    pub severity: Severity,
    /// The compiler's message, without its bracketed code.
    pub message: String,
    /// The file the first span names, when the problem carries one.
    pub file: Option<String>,
    /// The line of that span.
    pub line: Option<u32>,
    /// The column of that span.
    pub column: Option<u32>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Parses `output` into problems, in the order the compiler reported them.
///
/// A header whose first span line cannot be found, or is malformed, still
/// gives a problem, with no location. The row can still show the message;
/// clicking it just cannot open a file.
pub fn parse_build_output(output: &str) -> Vec<Problem> {
    let lines: Vec<&str> = output.lines().collect();
    let mut problems = Vec::new();
    let mut index = 0;
    // Step 1: Take one problem per header line, in the order they appear.
    while index < lines.len() {
        let Some((severity, message)) = parse_header(lines[index]) else {
            index += 1;
            continue;
        };
        // A `could not compile` summary repeats problems already listed.
        if message.starts_with("could not compile") {
            index += 1;
            continue;
        }
        let mut problem = Problem {
            severity,
            message,
            file: None,
            line: None,
            column: None,
        };
        // Step 2: The first span line within the lookahead window names the
        // location. A blank line means the next diagnostic has started.
        for span_line in lines.iter().skip(index + 1).take(SPAN_LOOKAHEAD) {
            if span_line.trim().is_empty() {
                break;
            }
            if let Some((file, line, column)) = parse_span(span_line) {
                problem.file = Some(file);
                problem.line = Some(line);
                problem.column = Some(column);
                break;
            }
        }
        problems.push(problem);
        index += 1;
    }
    problems
}

/// Returns the severity and the message of a rustc header line, or `None`
/// when the line is not one.
///
/// Cargo's status lines (`Compiling ...`, `Finished ...`) do not begin with a
/// severity word, so they never match.
fn parse_header(line: &str) -> Option<(Severity, String)> {
    let text = line.trim_start();
    let (severity, rest) = if let Some(rest) = text.strip_prefix("error") {
        (Severity::Error, rest)
    } else if let Some(rest) = text.strip_prefix("warning") {
        (Severity::Warning, rest)
    } else {
        let rest = text.strip_prefix("note")?;
        (Severity::Note, rest)
    };
    // `error[E0425]: msg` carries a bracketed code before the colon, and
    // `error: msg` carries none. A tail that fits neither form
    // (`error_span`, `errors`) is not a header.
    let rest = if let Some(rest) = rest.strip_prefix('[') {
        let code_end_index = rest.find(']')?;
        let code = &rest[..code_end_index];
        if code.is_empty() || !code.chars().next()?.is_ascii_uppercase() {
            return None;
        }
        if !rest[code_end_index + 1..].starts_with(':') {
            return None;
        }
        &rest[code_end_index + 1..]
    } else {
        rest
    };
    let message = rest.strip_prefix(':')?.trim();
    if message.is_empty() {
        return None;
    }
    Some((severity, message.to_string()))
}

/// Returns the `file`, `line` and `column` of a rustc span line
/// (`  --> src/x.rs:12:5`), or `None` when the line is not one.
fn parse_span(line: &str) -> Option<(String, u32, u32)> {
    let text = line.trim_start();
    let location = text.strip_prefix("-->")?.trim();
    // A path may contain colons (a Windows drive), so split from the right:
    // the last two fields are the line number and the column.
    let (path_and_line, column_text) = location.rsplit_once(':')?;
    let (path, line_text) = path_and_line.rsplit_once(':')?;
    let line: u32 = line_text.trim().parse().ok()?;
    // The last field is the column. Only its leading digits are read, so any
    // text after them is ignored.
    let column: u32 = column_text
        .trim()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    if path.is_empty() {
        return None;
    }
    Some((path.to_string(), line, column))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coded_error_carries_its_first_span() {
        let output = "\
   Compiling pill_spline v0.1.0
error[E0425]: cannot find value `speed` in this scope
  --> extensions/pill_spline/modules/runtime/src/pill_spline.rs:41:9
   |
41 |     let v = speed * dt;
   |             ^^^^^ not found in this scope

error: could not compile `pill_spline` (lib) due to 1 previous error
";
        let problems = parse_build_output(output);
        assert_eq!(problems.len(), 1, "{problems:?}");
        let problem = &problems[0];
        assert_eq!(problem.severity, Severity::Error);
        assert_eq!(problem.message, "cannot find value `speed` in this scope");
        assert_eq!(
            problem.file.as_deref(),
            Some("extensions/pill_spline/modules/runtime/src/pill_spline.rs")
        );
        assert_eq!((problem.line, problem.column), (Some(41), Some(9)));
    }

    #[test]
    fn a_warning_and_an_error_both_parse_in_order() {
        let output = "\
warning: unused variable: `x`
  --> src/lib.rs:3:9
error: expected `;`, found `}`
  --> src/lib.rs:7:1
";
        let problems = parse_build_output(output);
        assert_eq!(problems.len(), 2);
        assert_eq!(problems[0].severity, Severity::Warning);
        assert_eq!(problems[0].message, "unused variable: `x`");
        assert_eq!(problems[1].severity, Severity::Error);
        assert_eq!(problems[1].line, Some(7));
    }

    #[test]
    fn a_windows_drive_path_survives_the_colon_split() {
        let output = "error[E0308]: mismatched types\n  --> D:\\work\\crate\\src\\lib.rs:12:34\n";
        let problems = parse_build_output(output);
        assert_eq!(problems.len(), 1);
        assert_eq!(
            problems[0].file.as_deref(),
            Some("D:\\work\\crate\\src\\lib.rs")
        );
        assert_eq!((problems[0].line, problems[0].column), (Some(12), Some(34)));
    }

    #[test]
    fn a_header_without_a_span_still_reports_its_message() {
        let problems =
            parse_build_output("error: linking with `link.exe` failed: exit code: 1120\n");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].file, None);
        assert_eq!(problems[0].severity, Severity::Error);
    }

    #[test]
    fn unrelated_lines_are_ignored() {
        let output = "\
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.2s
error_span: not a header
errors happened
";
        assert!(parse_build_output(output).is_empty());
    }

    #[test]
    fn a_summary_line_is_dropped_as_a_duplicate() {
        let output = "\
error: expected one of `.`, `;`, `?`, `}`, or an operator, found `)`
 --> src/main.rs:2:5
error: could not compile `demo` (bin \"demo\") due to 1 previous error
";
        let problems = parse_build_output(output);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.starts_with("expected one of"));
    }
}
