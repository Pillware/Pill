//! Turns engine errors into miette reports that print their message with
//! terminal styling.
//!
//! # Responsibilities
//!
//! - Wrap one engine error so a miette report prints its message with terminal
//!   styling but still carries the error's codes, labels and sources.
//! - Install the engine's report handler as miette's process-wide hook.
//!
//! # Design
//!
//! An error's own `Display` stays plain text. [`engine_report`] adds the
//! styling through [`StyledDiagnosticProxy`], so an error type never needs to
//! know how it is printed.

// Current crate
use super::*;

// =============================================================================
// Miette Presentation
// =============================================================================

/// Prints an engine error's message with terminal styling for a report.
///
/// Only the message is styled. The error's code, help, labels, severity and
/// source pass through unchanged. This adapter is allowed to add style because
/// it is the reporting boundary, not the error type itself.
pub struct StyledDiagnosticProxy<E> {
    wrapped_error: E,
}

impl<E> StyledDiagnosticProxy<E> {
    /// Wraps an engine error so a report prints its message with terminal
    /// styling.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::error::{BuildError, StyledDiagnosticProxy};
    ///
    /// let proxy = StyledDiagnosticProxy::new(BuildError::Cancelled);
    /// assert!(matches!(proxy.into_inner(), BuildError::Cancelled));
    /// ```
    pub fn new(wrapped_error: E) -> Self {
        Self { wrapped_error }
    }

    /// Returns the engine error this proxy wraps.
    pub fn into_inner(self) -> E {
        self.wrapped_error
    }
}

impl<E> fmt::Display for StyledDiagnosticProxy<E>
where
    E: EngineMessage,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Step 1: Render the styled message into a local string. The block ends
        // the mutable borrow of that string before the next step writes it.
        let mut styled_message = String::new();
        {
            let mut renderer = TerminalMessageRenderer::new(&mut styled_message);
            self.wrapped_error.render_message(&mut renderer)?;
        }
        // Step 2: Write the finished message to the caller's formatter.
        formatter.write_str(&styled_message)
    }
}

impl<E: fmt::Debug> fmt::Debug for StyledDiagnosticProxy<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.wrapped_error, formatter)
    }
}

impl<E> std::error::Error for StyledDiagnosticProxy<E>
where
    E: EngineMessage + miette::Diagnostic,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.wrapped_error.source()
    }
}

impl<E> miette::Diagnostic for StyledDiagnosticProxy<E>
where
    E: EngineMessage + miette::Diagnostic,
{
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.wrapped_error.code()
    }

    fn severity(&self) -> Option<miette::Severity> {
        self.wrapped_error.severity()
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.wrapped_error.help()
    }

    fn url<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.wrapped_error.url()
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.wrapped_error.source_code()
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        self.wrapped_error.labels()
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn miette::Diagnostic> + 'a>> {
        self.wrapped_error.related()
    }

    fn diagnostic_source(&self) -> Option<&dyn miette::Diagnostic> {
        self.wrapped_error.diagnostic_source()
    }
}

/// Prints a report in the terminal, using miette's graphical layout over the
/// engine's diagnostic data.
///
/// The styled message is set up by [`engine_report`], which wraps the error in
/// [`StyledDiagnosticProxy`] before miette turns it into a report.
#[derive(Debug)]
pub struct EngineReportHandler {
    graphical_report_handler: miette::GraphicalReportHandler,
}

impl Default for EngineReportHandler {
    fn default() -> Self {
        Self {
            graphical_report_handler: miette::GraphicalReportHandler::new(),
        }
    }
}

impl miette::ReportHandler for EngineReportHandler {
    fn debug(
        &self,
        error: &dyn miette::Diagnostic,
        formatter: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        self.graphical_report_handler.debug(error, formatter)
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Installs the engine report handler as miette's process-wide hook.
///
/// Call it once at the top-level reporting boundary, before anything prints a
/// report.
pub fn install_engine_report_handler() {
    // miette keeps one hook per process, so a second call changes nothing and
    // the first hook keeps formatting every report.
    let _ = miette::set_hook(Box::new(
        |_error: &(dyn miette::Diagnostic + 'static)| -> Box<dyn miette::ReportHandler> {
            Box::new(EngineReportHandler::default())
        },
    ));
}

/// Turns one engine error into a miette report whose message is printed with
/// terminal styling.
///
/// The message is the one the error defines, rendered for a terminal. miette
/// still carries the error's code, help, labels and source.
pub fn engine_report<E>(error: E) -> miette::Report
where
    E: EngineMessage + miette::Diagnostic + Send + Sync + 'static,
{
    miette::Report::new(StyledDiagnosticProxy::new(error))
}
