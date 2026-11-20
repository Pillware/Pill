//! Typed error system shared by every Pill workspace crate, generated from
//! one semantic message definition per leaf variant.
//!
//! # Responsibilities
//!
//! - Define the semantic rendering protocol ([`SemanticRole`],
//!   [`MessageRenderer`], [`EngineMessage`]) used by generated code.
//! - Provide the plain and terminal renderers plus the miette presentation
//!   adapter and report handler.
//! - Declare every subsystem error ([`ConfigError`], [`BuildError`],
//!   [`LibraryError`], [`ModuleError`], [`WatcherError`], [`CSharpError`]) and
//!   their transparent composition [`HostError`].
//!
//! # Design
//!
//! Every leaf variant carries one `#[message(...)]` definition; [`HostError`]
//! wraps whole subsystem errors and adds no text of its own. The
//! `#[engine_error]` macro generates the plain `Display` text, a stable
//! `namespace::variant` diagnostic code, and an [`EngineMessage`]
//! implementation that renders the same definition with semantic roles.
//! Subsystem errors compose into [`HostError`] through transparent
//! `#[from]` wrappers, so `?` works across boundaries and the leaf error
//! keeps its source chain. Error definitions never import a styling crate;
//! only the renderer turns a role into styled text.

// Standard library
use std::fmt;

// External crates
use pill_core_macros::engine_error;

// Current crate
use crate::style::PillStyle;

// The submodules bring all of this in through `use super::*;`, so a line
// removed here breaks them even where this file never names it.

// =============================================================================
// Module Layout
// =============================================================================

mod host;
mod render;
mod report;

// The submodules are private and everything they define is re-exported here,
// so `pill_core::error::BuildError` and `pill_core::error::MessageRenderer`
// keep working whatever file the item lives in.
pub use host::*;
pub use render::*;
pub use report::*;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use miette::Diagnostic as _;

    /// A boxed source still reports its cause through the chain.
    ///
    /// The watcher and loader variants store their error boxed instead of
    /// naming it, because this crate is the shared dylib every posture links
    /// and may not depend on `notify` or `libloading`. The host builds its
    /// one-line reports with `format_error_chain`, so the cause it walks to
    /// has to survive.
    #[test]
    fn a_boxed_source_still_reports_its_cause() {
        let error = WatcherError::CreationFailed {
            source: Box::new(std::io::Error::other("the platform watcher refused")),
        };

        assert_eq!(
            crate::utils::format_error_chain(&error),
            "failed to create file watcher: the platform watcher refused"
        );
    }

    /// The engine-dylib mismatch names both dylibs and both ways out.
    ///
    /// Windows reports the same failure as `os error 127`, which names neither
    /// the dylib nor the feature that moved. That is why the parts a reader
    /// has to act on are worth pinning.
    #[test]
    fn engine_dylib_mismatch_names_both_dylibs_and_the_remedy() {
        let error = LibraryError::EngineDylibMismatch {
            subject: "project".to_string(),
            host_engine: "target/debug/pill_core.dll".to_string(),
            module_engine: "target/hot/pill_core.dll".to_string(),
        };
        let message = error.to_string();

        assert!(message.contains("project"), "names the subject: {message}");
        assert!(
            message.contains("target/debug/pill_core.dll")
                && message.contains("target/hot/pill_core.dll"),
            "names both dylibs: {message}"
        );
        assert!(
            message.contains("--features rendering"),
            "names the remedy: {message}"
        );
        assert!(
            !message.contains("  "),
            "no run of spaces from a wrapped message literal: {message}"
        );
    }

    /// One piece of a rendered message, kept so a test can compare it.
    #[derive(Debug, PartialEq, Eq)]
    enum RecordedPart {
        Text(String),
        Styled { role: SemanticRole, text: String },
        Value(String),
        DebugValue(String),
    }

    /// Renderer that records calls instead of writing anywhere.
    struct RecordingRenderer {
        parts: Vec<RecordedPart>,
    }

    impl MessageRenderer for RecordingRenderer {
        fn text(&mut self, text: &str) -> fmt::Result {
            self.parts.push(RecordedPart::Text(text.to_string()));
            Ok(())
        }

        fn styled(&mut self, role: SemanticRole, value: &dyn fmt::Display) -> fmt::Result {
            self.parts.push(RecordedPart::Styled {
                role,
                text: value.to_string(),
            });
            Ok(())
        }

        fn value(&mut self, value: &dyn fmt::Display) -> fmt::Result {
            self.parts.push(RecordedPart::Value(value.to_string()));
            Ok(())
        }

        fn debug_value(&mut self, value: &dyn fmt::Debug) -> fmt::Result {
            self.parts
                .push(RecordedPart::DebugValue(format!("{value:?}")));
            Ok(())
        }
    }

    /// Plain rendering drops semantic roles and preserves values.
    #[test]
    fn plain_renderer_ignores_roles_and_keeps_values() {
        let error = BuildError::TimedOut {
            name: "project-rs".to_string(),
            seconds: 120,
        };
        assert_eq!(
            error.to_plain_message(),
            "build command for project-rs timed out after 120 seconds"
        );
    }

    /// The renderer receives the text and values in the order the definition
    /// names them.
    #[test]
    fn recording_renderer_receives_text_and_values_in_order() {
        let error = CSharpError::InteropVersionMismatch {
            expected: 2,
            actual: 3,
        };
        let mut renderer = RecordingRenderer { parts: Vec::new() };
        error.render_message(&mut renderer).unwrap();
        assert_eq!(
            renderer.parts,
            vec![
                RecordedPart::Text(
                    "csharp_runtime interop version mismatch: host expects ".to_string()
                ),
                RecordedPart::Value("2".to_string()),
                RecordedPart::Text(" but the runtime reports ".to_string()),
                RecordedPart::Value("3".to_string()),
            ]
        );
    }

    /// A styled field reaches the renderer with its role.
    #[test]
    fn styled_field_carries_its_role_into_the_renderer() {
        let error = BuildError::OutputMissing {
            path: "target/debug/project.dll".to_string(),
        };
        let mut renderer = RecordingRenderer { parts: Vec::new() };
        error.render_message(&mut renderer).unwrap();
        assert_eq!(
            renderer.parts,
            vec![
                RecordedPart::Text("shared library not found at expected path: ".to_string()),
                RecordedPart::Styled {
                    role: SemanticRole::Name,
                    text: "target/debug/project.dll".to_string(),
                },
            ]
        );
    }

    /// Transparent composition preserves the plain message of the leaf error.
    #[test]
    fn transparent_composition_keeps_the_leaf_message() {
        let build = BuildError::Cancelled;
        let host_error: HostError = build.into();
        assert_eq!(
            host_error.to_string(),
            "build cancelled: sources changed again during compilation"
        );
        assert_eq!(
            host_error.to_plain_message(),
            "build cancelled: sources changed again during compilation"
        );
    }

    /// Real source errors remain stored as `#[source]`, not converted text.
    #[test]
    fn source_chain_preserves_the_underlying_error() {
        let source = std::io::Error::new(std::io::ErrorKind::NotFound, "missing program");
        let error = BuildError::SpawnFailed {
            name: "project-rs".to_string(),
            source,
        };
        assert!(std::error::Error::source(&error).is_some());
        let host_error: HostError = error.into();
        assert!(std::error::Error::source(&host_error).is_some());
    }

    /// Diagnostic codes derive from the namespace and variant name.
    #[test]
    fn diagnostic_codes_derive_from_namespace_and_variant() {
        let error = BuildError::Cancelled;
        assert_eq!(
            error.code().map(|code| code.to_string()).as_deref(),
            Some("host::build::cancelled")
        );
    }

    /// The terminal renderer quotes a name token, which a test can check
    /// without matching ANSI codes.
    #[test]
    fn terminal_renderer_quotes_name_tokens() {
        let error = WatcherError::WatchDirectoryMissing {
            path: "project_rs/src".to_string(),
        };
        let mut rendered_message = String::new();
        let mut renderer = TerminalMessageRenderer::new(&mut rendered_message);
        error.render_message(&mut renderer).unwrap();
        assert!(rendered_message.contains("\"project_rs/src\""));
    }
}
