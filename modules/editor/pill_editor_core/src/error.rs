//! Failures of the persisted dock layout.
//!
//! # Responsibilities
//!
//! - Declare [`LayoutPersistenceError`] as a `#[engine_error]` enum, one
//!   variant per way the dock layout can fail.
//!
//! # Design
//!
//! Every variant carries one `#[message(...)]` definition, and
//! `#[engine_error]` turns it into the message text and the diagnostic code.
//!
//! A failed save leaves the previous document as it was, so
//! [`crate::layout::save`] logs the plain message and the editor keeps
//! running. Reading never reports here: [`crate::layout::load_or_default`]
//! falls back to the default layout for a document this build cannot use, and
//! logs one line.

// Standard library
use std::io;

// External crates
use pill_core_macros::engine_error;

// Current crate
use crate::layout::LayoutError;

// =============================================================================
// LayoutPersistenceError
// =============================================================================

/// The failures of the persisted dock layout.
///
/// A save reports [`Self::Serialization`] and [`Self::Filesystem`]. The other
/// two name the ways a saved document can be unusable: it does not parse, or
/// the layout model rejects it.
///
/// A failed save leaves the previous document as it was.
#[engine_error(namespace = editor::layout, runtime = ::pill_core::error)]
pub enum LayoutPersistenceError {
    /// The saved file does not parse as a document of the current layout
    /// schema.
    #[message("failed to read the saved dock layout from ", name_style(path))]
    Invalid {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    /// The loaded model failed a check in [`LayoutError`], such as a schema
    /// version this build does not know.
    #[message("the saved dock layout is invalid")]
    Validation {
        #[source]
        source: LayoutError,
    },

    /// The dock model could not be turned into YAML, so nothing was written.
    #[message("failed to serialize the dock layout: ", value(detail))]
    Serialization {
        /// What the YAML writer said.
        detail: String,
    },

    /// Writing the document failed: creating its folder, writing the temporary
    /// file, or replacing the old file. `path` names the one that failed.
    #[message("failed to write the dock layout to ", name_style(path))]
    Filesystem {
        path: String,
        #[source]
        source: io::Error,
    },
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use miette::Diagnostic;
    use pill_core::error::EngineMessage;
    use std::error::Error;

    /// The plain message shows the path exactly as stored, with no styling
    /// around it, and the I/O error stays reachable as the source.
    #[test]
    fn layout_persistence_messages_render_plain_paths() {
        let error = LayoutPersistenceError::Filesystem {
            path: String::from("C:\\tmp\\editor_layout.json"),
            source: io::Error::other("disk full"),
        };
        assert_eq!(
            error.to_plain_message(),
            "failed to write the dock layout to C:\\tmp\\editor_layout.json"
        );
        assert!(error.source().is_some());
    }

    /// The namespace and the variant name build the diagnostic code, which is
    /// `editor::layout::serialization` here.
    #[test]
    fn layout_persistence_codes_derive_from_namespace_and_variant() {
        let error = LayoutPersistenceError::Serialization {
            detail: String::from("the document cannot be written as YAML"),
        };
        let code = error.code().expect("generated code");
        assert_eq!(code.to_string(), "editor::layout::serialization");
    }

    /// The plain message stays the variant's own text, and the [`LayoutError`]
    /// stays reachable as the source.
    #[test]
    fn layout_validation_keeps_the_layout_error_in_the_chain() {
        let error = LayoutPersistenceError::Validation {
            source: LayoutError::UnsupportedVersion(99),
        };
        assert_eq!(error.to_plain_message(), "the saved dock layout is invalid");
        let source = error.source().expect("validation keeps its source");
        assert_eq!(
            source.to_string(),
            "invalid dock layout: UnsupportedVersion(99)"
        );
    }
}
