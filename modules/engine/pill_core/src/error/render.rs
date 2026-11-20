//! The semantic rendering protocol and its two renderers.
//!
//! # Responsibilities
//!
//! - Define [`SemanticRole`], the vocabulary of meanings a `#[message(...)]`
//!   definition marks the parts of its text with.
//! - Declare [`MessageRenderer`] and [`EngineMessage`], the seam between a
//!   generated error definition and whatever draws it.
//! - Provide the two renderers: plain text, and terminal text with styling.
//!
//! # Design
//!
//! A role says what a part of a message means, never how it looks, so an error
//! definition never names a colour or a glyph. Each renderer decides how a
//! role looks in its own medium, and this module is the only place a message
//! becomes styled text.

// Current crate
use super::*;

// =============================================================================
// Semantic Rendering Protocol
// =============================================================================

/// The meaning a styled part of a message carries.
///
/// A role says what the text is about, not how it looks. The renderer decides
/// the appearance for its own medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticRole {
    /// General engine objects: `Scene`, `Component`, `System`, `Resource`.
    General,
    /// Specific engine objects or types: `Texture`, `Surface`, `Handle`.
    Specific,
    /// Module-level concepts: `Engine`, `Renderer`, `Window`.
    Module,
    /// User or domain names: file paths, scene names, symbol names.
    Name,
}

/// The sink a generated message is written into, one part at a time.
///
/// Generated `EngineMessage` implementations call this protocol directly, so
/// no intermediate list or rebuilt string is ever created.
pub trait MessageRenderer {
    /// Write one piece of plain text.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying writer fails.
    fn text(&mut self, text: &str) -> fmt::Result;

    /// Write one value with its semantic role, so the renderer can style it.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying writer fails.
    fn styled(&mut self, role: SemanticRole, value: &dyn fmt::Display) -> fmt::Result;

    /// Write one value with no role and no styling.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying writer fails.
    fn value(&mut self, value: &dyn fmt::Display) -> fmt::Result;

    /// Write one value in its debug form, the way `{:?}` prints it.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying writer fails.
    fn debug_value(&mut self, value: &dyn fmt::Debug) -> fmt::Result;
}

/// The rendering interface `#[engine_error]` generates for every error enum.
///
/// The generated implementation walks the one message definition the variant
/// carries and writes it through [`MessageRenderer`], so the plain and the
/// styled output always agree.
pub trait EngineMessage {
    /// Render the variant's single message definition into `renderer`.
    ///
    /// # Errors
    ///
    /// Returns an error if `renderer` reports a write failure.
    fn render_message(&self, renderer: &mut dyn MessageRenderer) -> fmt::Result;

    /// Render the message as plain text, with no roles and no styling.
    ///
    /// This is the text `Display` shows, the text a log line carries, and the
    /// text handed to another runtime such as the C# side.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::error::{BuildError, EngineMessage};
    ///
    /// let message = BuildError::Cancelled.to_plain_message();
    /// assert_eq!(
    ///     message,
    ///     "build cancelled: sources changed again during compilation"
    /// );
    /// ```
    fn to_plain_message(&self) -> String {
        let mut message_text = String::new();
        let mut renderer = PlainMessageRenderer {
            writer: &mut message_text,
        };
        // Writing into a String cannot fail, so the error is ignored.
        let _ = self.render_message(&mut renderer);
        message_text
    }
}

// =============================================================================
// Plain and Terminal Renderers
// =============================================================================

/// A renderer that turns every role back into plain text.
///
/// Each value is written exactly as it is given, with no styling. A message
/// therefore reads the same in a log file, in a test assertion, or in a
/// terminal without colour. [`EngineMessage::to_plain_message`] is the usual
/// way to reach it.
pub struct PlainMessageRenderer<'a, W> {
    writer: &'a mut W,
}

impl<W: fmt::Write> PlainMessageRenderer<'_, W> {
    /// Wrap a writer that receives the plain text.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::error::{BuildError, EngineMessage, PlainMessageRenderer};
    ///
    /// let mut message_text = String::new();
    /// {
    ///     let mut renderer = PlainMessageRenderer::new(&mut message_text);
    ///     BuildError::Cancelled.render_message(&mut renderer).unwrap();
    /// }
    /// assert_eq!(
    ///     message_text,
    ///     "build cancelled: sources changed again during compilation"
    /// );
    /// ```
    pub fn new(writer: &mut W) -> PlainMessageRenderer<'_, W> {
        PlainMessageRenderer { writer }
    }
}

impl<W: fmt::Write> MessageRenderer for PlainMessageRenderer<'_, W> {
    fn text(&mut self, text: &str) -> fmt::Result {
        self.writer.write_str(text)
    }

    fn styled(&mut self, _role: SemanticRole, value: &dyn fmt::Display) -> fmt::Result {
        write!(self.writer, "{value}")
    }

    fn value(&mut self, value: &dyn fmt::Display) -> fmt::Result {
        write!(self.writer, "{value}")
    }

    fn debug_value(&mut self, value: &dyn fmt::Debug) -> fmt::Result {
        write!(self.writer, "{value:?}")
    }
}

/// A renderer that styles each value according to its semantic role.
///
/// The colours come from the crate's [`PillStyle`](crate::PillStyle)
/// vocabulary, one method per role, so an error definition never names a
/// colour or an ANSI code.
pub struct TerminalMessageRenderer<'a, W> {
    writer: &'a mut W,
}

impl<W: fmt::Write> TerminalMessageRenderer<'_, W> {
    /// Wrap a writer that receives the styled text.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::error::{BuildError, EngineMessage, TerminalMessageRenderer};
    ///
    /// let mut message_text = String::new();
    /// {
    ///     let mut renderer = TerminalMessageRenderer::new(&mut message_text);
    ///     BuildError::Cancelled.render_message(&mut renderer).unwrap();
    /// }
    /// assert!(message_text.contains("build cancelled"));
    /// ```
    pub fn new(writer: &mut W) -> TerminalMessageRenderer<'_, W> {
        TerminalMessageRenderer { writer }
    }
}

impl<W: fmt::Write> MessageRenderer for TerminalMessageRenderer<'_, W> {
    fn text(&mut self, text: &str) -> fmt::Result {
        self.writer.write_str(text)
    }

    fn styled(&mut self, role: SemanticRole, value: &dyn fmt::Display) -> fmt::Result {
        // Each role has one style method in the crate's vocabulary, so a
        // colour is defined once and the renderer keeps no palette of its own.
        let value_text = value.to_string();
        let styled_text = match role {
            SemanticRole::General => value_text.general_object_style(),
            SemanticRole::Specific => value_text.specific_object_style(),
            SemanticRole::Module => value_text.module_object_style(),
            SemanticRole::Name => value_text.name_style(),
        };
        self.writer.write_str(&styled_text)
    }

    fn value(&mut self, value: &dyn fmt::Display) -> fmt::Result {
        write!(self.writer, "{value}")
    }

    fn debug_value(&mut self, value: &dyn fmt::Debug) -> fmt::Result {
        write!(self.writer, "{value:?}")
    }
}
