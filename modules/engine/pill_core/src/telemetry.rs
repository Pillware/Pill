//! One `tracing` system for the engine: three lanes of log targets feeding
//! configurable output layers.
//!
//! # Responsibilities
//!
//! - Define the static tracing targets: developer logs (`engine::dev`), engine
//!   logs (`engine::*`) and the profiling lanes (`profile::*`).
//! - Turn a [`LoggingConfig`] into a strict `EnvFilter`, and hand out reload
//!   handles so a filter can be replaced while the host runs.
//! - Own the [`EngineTerminalFormatter`], which decides how a line looks:
//!   level, target, message, fields, timestamp and source location.
//! - Hold the process-wide [`TimestampFormat`] and the source-location switch
//!   ([`set_show_source_location`]), so every DLL writes lines the same way.
//! - Assemble the subscriber stack (terminal, optional file, optional capture
//!   ring, optional Tracy) through [`TelemetryBuilder`], each layer filtered on
//!   its own.
//!
//! # Design
//!
//! The engine logs through `tracing`; each output layer decides where a line
//! goes and how it looks. A lane is a group of targets, and the three lanes
//! stay apart:
//!
//! - `engine::dev` - scratch developer logs from `log!`/`dev_warn!`/`dev_error!`,
//!   feature-gated behind `dev-logs`.
//! - `engine::*` - permanent structured engine logs.
//! - `profile::*` - profiling spans. A `TracyZone` asks the Tracy layer's
//!   filter whether its target is enabled, independently of terminal verbosity.

/// Bounded in-process log capture for the editor console.
pub mod capture;

// Standard library
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

// External crates
/// Text styling (`.cyan()`, `.bold()`, ...) for [`log_block_colored`] lines,
/// re-exported so a caller needs no `colored` dependency of its own.
pub use colored::Colorize;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer as _;
use tracing_subscriber::{EnvFilter, Registry};

// Current crate
use crate::platform::log_output::{self, FileGuard, FileWriter, TerminalWriter};

// =============================================================================
// Static Telemetry Targets
// =============================================================================

/// Target of the simple developer logging macros (`log!`, `dev_warn!`, `dev_error!`).
pub const DEV_LOG_TARGET: &str = "engine::dev";

/// Static `tracing` targets used by instrumentation callsites.
///
/// A callsite names one of these constants. The strings never change, so a log
/// filter can select a target by its name.
pub mod telemetry_target {
    /// The main engine lifecycle.
    pub const ENGINE: &str = "engine::engine";
    /// Hot-reload coordination.
    pub const HOT_RELOAD: &str = "engine::hot_reload";
    /// Startup of the statically linked project and its modules, which a
    /// shipped build has in place of the dev host's reloads.
    pub const LINK: &str = "engine::link";
    /// Input handling.
    pub const INPUT: &str = "engine::input";
    /// ECS world and scheduler activity.
    pub const ECS: &str = "engine::ecs";
    /// Hardware detection and startup configuration reporting.
    pub const SYSTEM: &str = "engine::system";
    /// Renderer activity.
    pub const RENDERING: &str = "engine::rendering";
    /// The in-game UI, which mirrors textures and publishes draw lists.
    pub const UI: &str = "engine::ui";
    /// Resource loading and storage.
    pub const RESOURCES: &str = "engine::resources";
    /// The editor and its panels: the executable, its shared core and the
    /// shell module.
    pub const EDITOR: &str = "engine::editor";
    /// Lines forwarded from managed C# code through the bridge.
    pub const CSHARP: &str = "engine::csharp";

    /// The coarse profiling lane, for frame, system and pass zones.
    pub const PROFILE_COARSE: &str = "profile::coarse";
    /// The fine profiling lane, for temporary deep investigation.
    pub const PROFILE_FINE: &str = "profile::fine";
}

/// The target of the coarse profiling lane.
pub const PROFILE_COARSE_TARGET: &str = telemetry_target::PROFILE_COARSE;
/// The target of the fine investigative profiling lane.
pub const PROFILE_FINE_TARGET: &str = telemetry_target::PROFILE_FINE;

/// The text between a line's message and its fields.
///
/// A line with no fields has no separator. A block carries it on the prefix
/// line between its heading and its fields. The source location is not a
/// field and keeps its own gap.
pub const FIELD_SEPARATOR: &str = " | ";

// =============================================================================
// Logging Configuration
// =============================================================================

/// Per-target logging levels expressed as strict `EnvFilter` directives.
///
/// A configuration holds one baseline level and a list of per-target
/// overrides. [`LoggingConfig::build_env_filter`] parses both strictly: an
/// invalid directive is a configuration error, never a silent fallback to
/// `INFO`.
#[derive(Debug, Clone)]
pub struct LoggingConfig {
    /// Baseline level applied before any target-specific directive.
    baseline: tracing::level_filters::LevelFilter,
    /// Ordered `(target, level)` overrides.
    directives: Vec<(String, tracing::level_filters::LevelFilter)>,
}

impl LoggingConfig {
    /// Create an empty logging configuration (baseline `INFO`, no overrides).
    pub fn new() -> Self {
        Self {
            baseline: tracing::level_filters::LevelFilter::INFO,
            directives: Vec::new(),
        }
    }

    /// Set the baseline level applied to every target without an override.
    pub fn with_baseline(mut self, baseline: tracing::level_filters::LevelFilter) -> Self {
        self.baseline = baseline;
        self
    }

    /// Override the level of one static target (for example
    /// `telemetry_target::RENDERING`). Use `LevelFilter::OFF` to silence a
    /// target entirely.
    pub fn with_directive(
        mut self,
        target: impl Into<String>,
        level: tracing::level_filters::LevelFilter,
    ) -> Self {
        self.directives.push((target.into(), level));
        self
    }

    /// The default configuration for a game host.
    ///
    /// Engine logs are `INFO`, except resource loading, which is off. The
    /// renderer's per-shader and per-material detail stays at `DEBUG`.
    /// Developer scratch logs are enabled when the `dev-logs` feature is on,
    /// and `wgpu` and `naga` are reduced to warnings.
    pub fn default_engine() -> Self {
        use tracing::level_filters::LevelFilter;
        Self::new()
            .with_directive(DEV_LOG_TARGET, LevelFilter::DEBUG)
            .with_directive(telemetry_target::ENGINE, LevelFilter::INFO)
            .with_directive(telemetry_target::HOT_RELOAD, LevelFilter::INFO)
            .with_directive(telemetry_target::LINK, LevelFilter::INFO)
            .with_directive(telemetry_target::INPUT, LevelFilter::INFO)
            .with_directive(telemetry_target::ECS, LevelFilter::INFO)
            .with_directive(telemetry_target::RENDERING, LevelFilter::INFO)
            .with_directive(telemetry_target::RESOURCES, LevelFilter::OFF)
            .with_directive("wgpu", LevelFilter::WARN)
            .with_directive("naga", LevelFilter::WARN)
    }

    /// Build a strict [`EnvFilter`] from this configuration.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::InvalidFilter`] when the baseline level
    /// cannot form a filter, and [`TelemetryError::InvalidDirective`] when any
    /// configured directive fails to parse.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::telemetry::LoggingConfig;
    /// use pill_core::tracing::level_filters::LevelFilter;
    ///
    /// let logging_config = LoggingConfig::new()
    ///     .with_directive("engine::ecs", LevelFilter::DEBUG);
    /// let filter = logging_config.build_env_filter().expect("directive must parse");
    /// ```
    pub fn build_env_filter(&self) -> Result<EnvFilter, TelemetryError> {
        let mut filter = EnvFilter::try_new(self.baseline.to_string()).map_err(|source| {
            TelemetryError::InvalidFilter {
                filter: self.baseline.to_string(),
                source: Box::new(source),
            }
        })?;
        for (target, level) in &self.directives {
            let directive = format!("{target}={level}");
            let directive =
                directive
                    .parse()
                    .map_err(|source| TelemetryError::InvalidDirective {
                        directive: directive.clone(),
                        source: Box::new(source),
                    })?;
            filter = filter.add_directive(directive);
        }
        Ok(filter)
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Timestamp Format
// =============================================================================

/// How the local time at the start of every log line is written.
///
/// Chosen by the project's `logging: timestamp:` setting and applied with
/// [`set_timestamp_format`]. It is one process-wide value, held in
/// `pill_core` so every DLL's log lines agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum TimestampFormat {
    /// `[dd.mm.yyyy hh:mm:ss:mmm]`, written `date_time` in settings.
    DateTime = 1,
    /// `[hh:mm:ss:mmm]`, written `time` in settings. The default.
    #[default]
    Time = 0,
}

impl TimestampFormat {
    /// Read the settings name: `date_time` or `time`, in any case.
    ///
    /// # Errors
    ///
    /// Returns a message naming the accepted values for anything else.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::telemetry::TimestampFormat;
    ///
    /// assert_eq!(TimestampFormat::parse("time"), Ok(TimestampFormat::Time));
    /// ```
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "date_time" => Ok(Self::DateTime),
            "time" => Ok(Self::Time),
            _ => Err(format!(
                "`{text}` is not a timestamp format; use date_time or time"
            )),
        }
    }

    /// The settings name of this format, as [`Self::parse`] reads it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DateTime => "date_time",
            Self::Time => "time",
        }
    }

    /// The `chrono` pattern that writes this format (`%3f` is milliseconds).
    fn pattern(self) -> &'static str {
        match self {
            Self::DateTime => "%d.%m.%Y %H:%M:%S:%3f",
            Self::Time => "%H:%M:%S:%3f",
        }
    }
}

/// The active [`TimestampFormat`], as the discriminant it declares.
static TIMESTAMP_FORMAT: AtomicU8 = AtomicU8::new(TimestampFormat::Time as u8);

/// Decodes a stored discriminant; `None` for a byte no variant wrote.
///
/// The match is exhaustive on purpose: adding a format adds a stored value, and
/// this is the only place that must learn to read it. A `!= DateTime` test
/// would quietly report the default instead.
const fn timestamp_format_from_discriminant(discriminant: u8) -> Option<TimestampFormat> {
    match discriminant {
        0 => Some(TimestampFormat::Time),
        1 => Some(TimestampFormat::DateTime),
        _ => None,
    }
}

/// Set how every later log line writes its time.
pub fn set_timestamp_format(format: TimestampFormat) {
    TIMESTAMP_FORMAT.store(format as u8, Ordering::Relaxed);
}

/// How log lines currently write their time.
pub fn timestamp_format() -> TimestampFormat {
    timestamp_format_from_discriminant(TIMESTAMP_FORMAT.load(Ordering::Relaxed)).unwrap_or_default()
}

/// Whether log lines end with the `file:line` that emitted them. Off by
/// default; the project's `logging: source_location:` setting turns it on.
static SHOW_SOURCE_LOCATION: AtomicBool = AtomicBool::new(false);

/// Set whether every later log line ends with its source location.
pub fn set_show_source_location(show: bool) {
    SHOW_SOURCE_LOCATION.store(show, Ordering::Relaxed);
}

/// Whether log lines currently end with their source location.
pub fn show_source_location() -> bool {
    SHOW_SOURCE_LOCATION.load(Ordering::Relaxed)
}

// =============================================================================
// Terminal Formatter
// =============================================================================

/// Line formatting layer for `tracing` events, used by the terminal and file
/// layers.
///
/// Owns the line's appearance: severity colors, target styling, timestamps,
/// file/line, and the message plus its structured fields. It adds that styling
/// itself, so callsites write plain text. A message that carries its own colors
/// (see [`log_block_colored`]) keeps them only on a color-capable layer.
#[derive(Debug, Clone, Copy)]
pub struct EngineTerminalFormatter {
    timestamps_enabled: bool,
}

impl EngineTerminalFormatter {
    /// Create a formatter with the timestamp prefix enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::telemetry::EngineTerminalFormatter;
    ///
    /// let formatter = EngineTerminalFormatter::new();
    /// ```
    pub fn new() -> Self {
        Self {
            timestamps_enabled: true,
        }
    }

    /// Disable the timestamp prefix.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::telemetry::EngineTerminalFormatter;
    ///
    /// let formatter = EngineTerminalFormatter::new().without_timestamps();
    /// ```
    pub fn without_timestamps(mut self) -> Self {
        self.timestamps_enabled = false;
        self
    }
}

impl Default for EngineTerminalFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, N> FormatEvent<S, N> for EngineTerminalFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // A record bridged from the `log` crate arrives with the target `log`,
        // and its real target, file and line sit in `log.*` fields. Normalizing
        // it shows the record like any other event.
        let normalized = event.normalized_metadata();
        let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
        // The file layer is built without ANSI, so styling follows the writer
        // rather than whether a terminal happens to be attached.
        let accepts_ansi = writer.has_ansi_escapes();
        if self.timestamps_enabled {
            let pattern = timestamp_format().pattern();
            let timestamp = format!("[{}]", chrono::Local::now().format(pattern));
            write!(
                writer,
                "{} ",
                paint(accepts_ansi, &timestamp, |text| text.dimmed())
            )?;
        }

        write!(writer, "{} ", styled_level(metadata.level(), accepts_ansi))?;
        write!(writer, "{}", styled_target(metadata.target(), accepts_ansi))?;

        // Message first, then its fields after `FIELD_SEPARATOR`, so a reader
        // can see where the fixed words end. A line with fields but no
        // message keeps the fields behind the target, with no separator.
        let mut visitor = StyledFieldVisitor {
            accepts_ansi,
            skip_log_fields: event.is_log(),
            message: String::new(),
            fields: String::new(),
        };
        event.record(&mut visitor);
        // A message may carry color codes (see `log_block_colored`). They reach
        // only a layer that shows colors - an interactive terminal - and are
        // stripped for the file layer and for piped output, which the suites read.
        if !(accepts_ansi && colored::control::SHOULD_COLORIZE.should_colorize()) {
            visitor.message = strip_color_codes(&visitor.message);
        }
        // A message with a line break is a block (see `log_block`): its first
        // line is the heading, which stays on the prefix line as the message,
        // and the body starts on the next line behind the `│ ` gutter. Wide
        // tables and trees then keep their full width. The block's closing line
        // breaks are dropped, so no empty line follows it.
        let (heading, body) = match visitor.message.split_once('\n') {
            Some((heading, body)) => (heading, Some(body.trim_end_matches(['\n', '\r']))),
            None => (visitor.message.as_str(), None),
        };
        if !heading.is_empty() {
            write!(writer, "  {heading}")?;
        }
        if !visitor.fields.is_empty() {
            if heading.is_empty() {
                write!(writer, "  {}", visitor.fields)?;
            } else {
                write!(writer, "{FIELD_SEPARATOR}{}", visitor.fields)?;
            }
        }

        // The source location is opt-in. It trails the line in dark gray, which
        // helps navigation without distracting a reader.
        if show_source_location() {
            if let (Some(file), Some(line)) = (metadata.file(), metadata.line()) {
                let location = format!("{file}:{line}");
                let location = paint(accepts_ansi, &location, |text| text.bright_black());
                write!(writer, "  {location}")?;
            }
        }
        // Every body line opens with a gutter in the timestamp's color, so a
        // reader sees which lines belong to the block above them.
        if let Some(body) = body.filter(|body| !body.is_empty()) {
            let gutter = paint(accepts_ansi, BLOCK_GUTTER, |text| text.dimmed());
            for body_line in body.lines() {
                write!(writer, "\n{gutter}{body_line}")?;
            }
        }
        writeln!(writer)?;
        Ok(())
    }
}

/// `tracing` visitor that collects the message and the structured fields
/// separately, so every layer can put the message first and the fields after
/// it, whatever order the callsite declared them in.
///
/// The terminal and file layers write the two parts into their line, and the
/// capture layer joins them for the editor console. Sharing the visitor is what
/// keeps the console from showing a different line than the log file for the
/// same event: the same fields appear, and a value reads the same way.
pub(crate) struct StyledFieldVisitor {
    accepts_ansi: bool,
    /// Drop the `log.*` fields of a bridged `log` record; the normalized
    /// metadata already carries what they hold.
    skip_log_fields: bool,
    message: String,
    fields: String,
}

impl StyledFieldVisitor {
    /// A visitor for one event that styles nothing and drops the bridged
    /// `log.*` fields when the record came through `log`.
    pub(crate) fn plain(skip_log_fields: bool) -> Self {
        Self {
            accepts_ansi: false,
            skip_log_fields,
            message: String::new(),
            fields: String::new(),
        }
    }

    /// The line a reader sees: the message, then its fields after
    /// [`FIELD_SEPARATOR`], with a block's fields on its heading line the way
    /// the terminal formatter puts them.
    pub(crate) fn into_line(self) -> String {
        if self.fields.is_empty() {
            return self.message;
        }
        if self.message.is_empty() {
            return self.fields;
        }
        let (heading, body) = match self.message.split_once('\n') {
            Some((heading, body)) => (heading, body.trim_end_matches(['\n', '\r'])),
            None => (self.message.as_str(), ""),
        };
        let mut line = if heading.is_empty() {
            self.fields
        } else {
            format!("{heading}{FIELD_SEPARATOR}{}", self.fields)
        };
        if !body.is_empty() {
            line.push('\n');
            line.push_str(body);
        }
        line
    }
}

impl Visit for StyledFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_debug(field, &value)
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        use fmt::Write as _;
        if field.name() == "message" {
            // The message is the primary human-readable text.
            let _ = write!(self.message, "{value:?}");
            return;
        }
        if self.skip_log_fields && field.name().starts_with("log.") {
            return;
        }
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        // Values keep their `Debug` form, with strings quoted and options shown
        // as `Some(..)`. The end-to-end suites match on that exact text.
        let field_name = paint(self.accepts_ansi, field.name(), |text| text.dimmed());
        let value = format!("{value:?}");
        let value = if MODULE_FIELD_NAMES.contains(&field.name()) {
            paint(self.accepts_ansi, &value, |text| text.cyan().bold())
        } else {
            value
        };
        let _ = write!(self.fields, "{field_name}={value}");
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_debug(field, &value)
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_debug(field, &value)
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_debug(field, &value)
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record_debug(field, &value)
    }
}

// =============================================================================
// Subscriber Builder
// =============================================================================

/// Concrete terminal layer type: the custom formatter over `Stdout`.
type TerminalLayer = tracing_subscriber::filter::Filtered<
    tracing_subscriber::fmt::Layer<
        Registry,
        tracing_subscriber::fmt::format::DefaultFields,
        EngineTerminalFormatter,
        TerminalWriter,
    >,
    reload::Layer<EnvFilter, Registry>,
    Registry,
>;

/// Registry type after the terminal layer is attached.
type TerminalStack = tracing_subscriber::layer::Layered<TerminalLayer, Registry>;

/// Concrete file layer type: the custom formatter over the platform's file writer.
type FileLayer = tracing_subscriber::filter::Filtered<
    tracing_subscriber::fmt::Layer<
        TerminalStack,
        tracing_subscriber::fmt::format::DefaultFields,
        EngineTerminalFormatter,
        FileWriter,
    >,
    reload::Layer<EnvFilter, TerminalStack>,
    TerminalStack,
>;

/// The three values the optional file layer produces: its layer, the file
/// writer's guard, and the reload handle.
type FileLayerArtifacts = (
    Option<FileLayer>,
    Option<Arc<FileGuard>>,
    Option<reload::Handle<EnvFilter, TerminalStack>>,
);

/// Configures and installs the engine's telemetry subscriber stack.
///
/// Set up the layers with the `with_*` methods, then call
/// [`TelemetryBuilder::init`] once per process. The terminal and file layers
/// are filtered independently and both are live-reloadable through
/// [`TelemetryHandles::reload_logging_config`] and
/// [`TelemetryHandles::reload_file_config`]. The Tracy layer only ever sees
/// `profile::*` targets.
#[derive(Debug, Clone, Default)]
pub struct TelemetryBuilder {
    logging: LoggingConfig,
    file_logging: Option<(LoggingConfig, PathBuf)>,
    capture: Option<capture::LogBuffer>,
    tracy_enabled: bool,
    fine_profiling_enabled: bool,
}

impl TelemetryBuilder {
    /// Create a builder with the default engine logging configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::telemetry::TelemetryBuilder;
    ///
    /// let builder = TelemetryBuilder::new();
    /// ```
    pub fn new() -> Self {
        Self {
            logging: LoggingConfig::default_engine(),
            file_logging: None,
            capture: None,
            tracy_enabled: false,
            fine_profiling_enabled: false,
        }
    }

    /// Capture `info` and above into a bounded in-process ring.
    ///
    /// The editor's console panel reads the ring; the terminal and file layers
    /// are unaffected. Every line in the ring is an owned copy, so the ring
    /// never holds a pointer into the module that logged it.
    pub fn with_capture(mut self, buffer: capture::LogBuffer) -> Self {
        self.capture = Some(buffer);
        self
    }

    /// Replace the terminal logging configuration.
    pub fn with_logging_config(mut self, logging_config: LoggingConfig) -> Self {
        self.logging = logging_config;
        self
    }

    /// Add a file logging layer with its own filter. The file rotates daily.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::telemetry::{LoggingConfig, TelemetryBuilder};
    ///
    /// let builder = TelemetryBuilder::new().with_file_output(
    ///     LoggingConfig::new(),
    ///     std::env::temp_dir(),
    /// );
    /// ```
    pub fn with_file_output(
        mut self,
        file_logging_config: LoggingConfig,
        directory: impl Into<PathBuf>,
    ) -> Self {
        self.file_logging = Some((file_logging_config, directory.into()));
        self
    }

    /// Turn the Tracy layer on or off. It carries only the `profile::*`
    /// targets, and a `TracyZone` asks this filter whether the profiler wants
    /// its zone.
    pub fn with_tracy(mut self, enabled: bool) -> Self {
        self.tracy_enabled = enabled;
        self
    }

    /// Also send `profile::fine` spans to the Tracy layer, for temporary deep
    /// investigation.
    pub fn with_fine_profiling(mut self, enabled: bool) -> Self {
        self.fine_profiling_enabled = enabled;
        self
    }

    /// Build, install, and return the reload handles.
    ///
    /// Installs the subscriber stack once per process; a later call returns the
    /// handles of that first install without reinstalling. Concurrent callers
    /// are serialized so only one ever touches the global subscriber.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::InvalidFilter`] or
    /// [`TelemetryError::InvalidDirective`] when a configured filter cannot be
    /// parsed, and [`TelemetryError::FileOutput`] when the log file cannot be
    /// opened. A call made after a failed install returns
    /// [`TelemetryError::InstallFailed`], which repeats the first failure.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_core::telemetry::TelemetryBuilder;
    ///
    /// let handles = TelemetryBuilder::new()
    ///     .init()
    ///     .expect("default configuration installs cleanly");
    /// ```
    pub fn init(self) -> Result<TelemetryHandles, TelemetryError> {
        // Install once per process. `OnceLock::get_or_init` runs the installer
        // at most once and makes other callers wait for it, so no separate
        // mutex is needed. The result is stored as a message because
        // `TelemetryError` boxes its sources and cannot be cloned.
        static INSTALLED: OnceLock<Result<TelemetryHandles, String>> = OnceLock::new();
        match INSTALLED.get_or_init(|| Self::install(self).map_err(|error| error.to_string())) {
            Ok(handles) => Ok(handles.clone()),
            Err(message) => Err(TelemetryError::InstallFailed {
                message: message.clone(),
            }),
        }
    }

    /// Build the full subscriber stack (terminal, optional file, optional
    /// capture ring, optional Tracy) and install it as the process-wide default
    /// subscriber.
    ///
    /// Invoked exactly once per process by [`Self::init`], through its
    /// `OnceLock`.
    ///
    /// # Errors
    ///
    /// Returns what [`LoggingConfig::build_env_filter`] reports for an invalid
    /// filter, and [`TelemetryError::FileOutput`] when the log file cannot be
    /// opened.
    fn install(self) -> Result<TelemetryHandles, TelemetryError> {
        // Step 1: Build the terminal layer with its own reloadable filter.
        let terminal_filter = self.logging.build_env_filter()?;
        let (terminal_reload, terminal_handle) = reload::Layer::new(terminal_filter);
        let terminal_layer: TerminalLayer = tracing_subscriber::fmt::layer()
            .with_writer(log_output::terminal_writer())
            .with_ansi(true)
            .event_format(EngineTerminalFormatter::new())
            .with_filter(terminal_reload);

        // Step 2: Build the optional file layer. `Option<L>` implements
        // `Layer`, so a missing file layer is simply a no-op layer in the
        // same position.
        let (file_layer, file_guard, file_handle): FileLayerArtifacts = match self.file_logging {
            Some((file_logging_config, directory)) => {
                let file_filter = file_logging_config.build_env_filter()?;
                let (file_reload, file_handle) =
                    reload::Layer::<EnvFilter, TerminalStack>::new(file_filter);
                let (writer, file_guard) =
                    log_output::open_file_output(&directory, "engine.log")
                        .map_err(|message| TelemetryError::FileOutput { message })?;
                let layer: FileLayer = tracing_subscriber::fmt::layer::<TerminalStack>()
                    .with_writer(writer)
                    .with_ansi(false)
                    .event_format(EngineTerminalFormatter::new())
                    .with_filter(file_reload);
                (Some(layer), Some(Arc::new(file_guard)), Some(file_handle))
            }
            None => (None, None, None),
        };

        // Step 3: Build the optional Tracy layer, restricted to `profile::*`
        // targets and controlled independently of terminal verbosity.
        #[cfg(feature = "tracy")]
        let tracy_layer = if self.tracy_enabled {
            let mut tracy_filter = EnvFilter::default();
            tracy_filter = tracy_filter.add_directive(
                format!("{PROFILE_COARSE_TARGET}=trace")
                    .parse()
                    .map_err(|source| TelemetryError::InvalidDirective {
                        directive: format!("{PROFILE_COARSE_TARGET}=trace"),
                        source: Box::new(source),
                    })?,
            );
            if self.fine_profiling_enabled {
                tracy_filter = tracy_filter.add_directive(
                    format!("{PROFILE_FINE_TARGET}=trace")
                        .parse()
                        .map_err(|source| TelemetryError::InvalidDirective {
                            directive: format!("{PROFILE_FINE_TARGET}=trace"),
                            source: Box::new(source),
                        })?,
                );
            }
            Some(tracing_tracy::TracyLayer::default().with_filter(tracy_filter))
        } else {
            None
        };

        #[cfg(not(feature = "tracy"))]
        let tracy_layer: Option<tracing_subscriber::layer::Identity> = None;

        // Step 4: Build the optional capture layer, which feeds the editor
        // console.
        let capture_layer = self
            .capture
            .map(|buffer| capture::CaptureLayer::new(buffer, tracing::Level::INFO));

        // Step 5: Assemble the layers and install the stack as the
        // process-wide default subscriber.
        let registry = tracing_subscriber::registry()
            .with(terminal_layer)
            .with(file_layer)
            .with(capture_layer)
            .with(tracy_layer);
        registry.init();

        // Step 6: Bridge the `log` crate into tracing, so a dependency that
        // still logs through `log` (winit, wgpu, notify, ...) reaches the same
        // layers. A bridged record has the target `log`, so the filters above
        // cannot select it by crate name. The bridge filters instead: only
        // warnings and errors pass, because a dependency's info and debug
        // chatter (such as symphonia's format probing) is not output the engine
        // wants, and the targets listed in IGNORED_LOG_TARGETS are dropped. The
        // bridge is process-wide and installs once; a second attempt only
        // reports that it is already active.
        let _ = tracing_log::LogTracer::builder()
            .with_max_level(tracing_log::log::LevelFilter::Warn)
            .ignore_all(IGNORED_LOG_TARGETS.iter().copied())
            .init();

        Ok(TelemetryHandles {
            logging_filter: terminal_handle,
            file_filter: file_handle,
            _file_guard: file_guard,
        })
    }
}

/// Handles returned by [`TelemetryBuilder::init`] for live configuration.
///
/// Holds the reload handles for the terminal and file layers plus the guard
/// that keeps the non-blocking file writer alive for the process lifetime.
#[derive(Debug, Clone)]
pub struct TelemetryHandles {
    /// Reload handle for the terminal logging filter.
    logging_filter: reload::Handle<EnvFilter, Registry>,
    /// Reload handle for the file logging filter, when a file layer exists.
    file_filter: Option<reload::Handle<EnvFilter, TerminalStack>>,
    /// Keeps the file writer alive for the app lifetime.
    _file_guard: Option<Arc<FileGuard>>,
}

impl TelemetryHandles {
    /// Reload the terminal logging filter from a [`LoggingConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::InvalidFilter`] when the baseline level cannot
    /// form a filter, [`TelemetryError::InvalidDirective`] when a directive of
    /// the configuration cannot be parsed, or [`TelemetryError::Reload`] when
    /// the reload itself fails. The previous filter stays active either way.
    pub fn reload_logging_config(
        &self,
        logging_config: &LoggingConfig,
    ) -> Result<(), TelemetryError> {
        reload_filter(&self.logging_filter, logging_config.build_env_filter()?)
    }

    /// Reload the file logging filter from a [`LoggingConfig`]. Does nothing
    /// when no file layer is installed.
    ///
    /// # Errors
    ///
    /// As [`Self::reload_logging_config`].
    pub fn reload_file_config(
        &self,
        file_logging_config: &LoggingConfig,
    ) -> Result<(), TelemetryError> {
        let Some(handle) = &self.file_filter else {
            return Ok(());
        };
        reload_filter(handle, file_logging_config.build_env_filter()?)
    }
}

// =============================================================================
// TelemetryError
// =============================================================================

/// Configuration or installation failures of the telemetry stack.
///
/// Each variant carries the input that failed, or a rendered description of
/// the failure, so a caller can show a precise message.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// A complete `RUST_LOG`-style filter string could not be parsed.
    #[error("invalid logging filter `{filter}`: {source}")]
    InvalidFilter {
        /// The rejected filter string.
        filter: String,
        /// The underlying parse error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// One `target=level` directive could not be parsed.
    #[error("invalid logging directive `{directive}`: {source}")]
    InvalidDirective {
        /// The rejected directive.
        directive: String,
        /// The underlying parse error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A live filter reload failed.
    #[error("failed to reload the logging filter: {error}")]
    Reload {
        /// Human-readable reload failure.
        error: String,
    },

    /// The file layer could not be opened.
    #[error("failed to open the log file: {message}")]
    FileOutput {
        /// Rendered description of the failure.
        message: String,
    },

    /// The one-time subscriber install failed.
    ///
    /// The install is attempted at most once per process; the failure is
    /// rendered into a message and cached so every concurrent or later caller
    /// sees the same error instead of silently re-attempting the install.
    #[error("failed to install the telemetry subscriber: {message}")]
    InstallFailed {
        /// Rendered description of the underlying install failure.
        message: String,
    },
}

// =============================================================================
// Free Functions
// =============================================================================

/// Join a heading and its lines into one multi-line log message.
///
/// The formatter prints a message that contains a line break as a block: the
/// heading stays on the first line as the message, after the time, level and
/// target and before the fields, and the remaining lines follow at the left
/// edge, each behind a `│ ` gutter in the timestamp's color. The string ends
/// with a line break, which the formatter drops, so the next entry follows
/// directly:
///
/// ```text
/// [21:51:45:003] INFO  engine::hot_reload  Modules to build: | total=1
/// │ 1. pill_spline  extension
/// ```
///
/// Pass the result to any logging macro: `info!(target: ..., "{}", block)`.
///
/// # Examples
///
/// ```
/// use pill_core::telemetry::log_block;
///
/// let block = log_block("Modules to build:", ["1. pill_spline", "2. project"]);
/// assert_eq!(block, "Modules to build:\n1. pill_spline\n2. project\n");
/// ```
pub fn log_block<I, S>(heading: &str, lines: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut block = heading.to_string();
    for line in lines {
        block.push('\n');
        block.push_str(line.as_ref());
    }
    block.push('\n');
    block
}

/// Join a heading and its lines into one multi-line log message, where any
/// part may carry color.
///
/// This works like [`log_block`] for parts that style themselves: plain
/// strings, or text styled with [`Colorize`] (re-exported here) or
/// [`PillStyle`](crate::PillStyle). The formatter strips the colors from the
/// file log and from piped output.
///
/// # Examples
///
/// ```
/// use pill_core::telemetry::{log_block_colored, Colorize};
///
/// let block = log_block_colored(
///     "Modules to build:".bold(),
///     [
///         format!("1. {}  extension", "pill_spline".cyan()),
///         format!("2. {}  project", "project".cyan()),
///     ],
/// );
/// assert!(block.starts_with(&"Modules to build:".bold().to_string()));
/// ```
pub fn log_block_colored<H, I, L>(heading: H, lines: I) -> String
where
    H: fmt::Display,
    I: IntoIterator<Item = L>,
    L: fmt::Display,
{
    log_block(
        &heading.to_string(),
        lines.into_iter().map(|line| line.to_string()),
    )
}

/// `text` without its ANSI escape sequences (`ESC [ ... letter`). A layer that
/// does not show colors therefore receives plain text.
///
/// # Examples
///
/// ```
/// use pill_core::telemetry::strip_color_codes;
///
/// assert_eq!(strip_color_codes("\u{1b}[36mpill_spline\u{1b}[0m"), "pill_spline");
/// ```
pub fn strip_color_codes(text: &str) -> String {
    if !text.contains('\u{1b}') {
        return text.to_string();
    }
    let mut plain = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' && characters.peek() == Some(&'[') {
            // Skip the parameters up to and including the final letter.
            characters.next();
            for code in characters.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

/// The prefix every body line of a block carries, drawn in the timestamp's color.
const BLOCK_GUTTER: &str = "│ ";

/// The column width of the level, padded to the widest name (`ERROR`, `DEBUG`).
const LEVEL_WIDTH: usize = 5;

/// Severity color mapping owned by the terminal formatter.
fn styled_level(level: &Level, accepts_ansi: bool) -> String {
    // Padded to the widest level so the targets line up in a column.
    let padded_level = format!("{:<LEVEL_WIDTH$}", level.as_str());
    match *level {
        Level::TRACE => paint(accepts_ansi, &padded_level, |text| text.magenta()),
        Level::DEBUG => paint(accepts_ansi, &padded_level, |text| text.blue().bold()),
        Level::INFO => paint(accepts_ansi, &padded_level, |text| text.green()),
        Level::WARN => paint(accepts_ansi, &padded_level, |text| text.yellow().bold()),
        Level::ERROR => paint(accepts_ansi, &padded_level, |text| text.red().bold()),
    }
}

/// Target styling owned by the terminal formatter.
fn styled_target(target: &str, accepts_ansi: bool) -> String {
    paint(accepts_ansi, target, |text| text.cyan())
}

/// Field names whose value names a module. Such a value is highlighted so the
/// module a lifecycle line is about stands out.
const MODULE_FIELD_NAMES: &[&str] = &["module", "extension"];

/// `log` targets the bridge drops, each with the reason it is noise.
///
/// - `wgpu_hal::vulkan::conv`: wgpu warns `Unrecognized present mode
///   1000361000` on every surface query when the driver offers
///   `VK_PRESENT_MODE_FIFO_LATEST_READY_EXT`, which it does not know yet and
///   skips. The module only logs such unknown-value notices.
const IGNORED_LOG_TARGETS: &[&str] = &["wgpu_hal::vulkan::conv"];

/// Applies `style` to `text` when the writer accepts ANSI escapes, and returns
/// the text unchanged otherwise.
fn paint(
    accepts_ansi: bool,
    text: &str,
    style: impl FnOnce(&str) -> colored::ColoredString,
) -> String {
    if accepts_ansi {
        style(text).to_string()
    } else {
        text.to_string()
    }
}

/// Replace one layer's live filter, reporting a refused reload.
///
/// Both layers reload the same way, so the error mapping is written once here
/// rather than once per layer.
///
/// # Errors
///
/// Returns [`TelemetryError::Reload`] when the subscriber refuses the new
/// filter; the old filter stays active.
fn reload_filter<S>(
    handle: &reload::Handle<EnvFilter, S>,
    filter: EnvFilter,
) -> Result<(), TelemetryError> {
    handle
        .reload(filter)
        .map_err(|error| TelemetryError::Reload {
            error: error.to_string(),
        })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The default engine configuration builds a strict, usable filter.
    #[test]
    fn default_engine_config_builds_a_strict_filter() {
        let filter = LoggingConfig::default_engine()
            .build_env_filter()
            .expect("default directives must be valid");
        let rendered = format!("{filter}");
        assert!(rendered.contains("engine::dev"));
        assert!(rendered.contains("engine::rendering"));
        assert!(rendered.contains("wgpu"));
    }

    /// Invalid directives are rejected instead of silently ignored.
    #[test]
    fn invalid_directives_are_rejected() {
        use tracing::level_filters::LevelFilter;
        // A target containing a second `=` cannot form a directive.
        let logging_config = LoggingConfig::new().with_directive("a=b=c", LevelFilter::INFO);
        assert!(logging_config.build_env_filter().is_err());
    }

    /// The terminal formatter renders level, target, and message text.
    #[test]
    fn terminal_formatter_renders_level_target_and_message() {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(String::new()));
        let writer = CapturingMakeWriter(Arc::clone(&captured));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .event_format(EngineTerminalFormatter::new().without_timestamps()),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(
                target: telemetry_target::RESOURCES,
                texture = "default_normal",
                "texture created"
            );
        });
        let output = captured.lock().unwrap().clone();
        assert!(output.contains("DEBUG"), "missing level: {output}");
        assert!(
            output.contains("engine::resources"),
            "missing target: {output}"
        );
        assert!(
            output.contains("texture created"),
            "missing message: {output}"
        );
        assert!(output.contains("texture="), "missing field: {output}");
    }

    /// Both timestamp formats read back from their settings names and write
    /// the documented shape: `dd.mm.yyyy hh:mm:ss:mmm` and `hh:mm:ss:mmm`.
    #[test]
    fn timestamp_formats_parse_and_render() {
        for format in [TimestampFormat::DateTime, TimestampFormat::Time] {
            assert_eq!(TimestampFormat::parse(format.as_str()), Ok(format));
        }
        assert_eq!(TimestampFormat::parse(" TIME "), Ok(TimestampFormat::Time));
        assert!(TimestampFormat::parse("unix").is_err());

        let moment = chrono::NaiveDate::from_ymd_opt(2026, 10, 3)
            .and_then(|date| date.and_hms_milli_opt(9, 5, 7, 42))
            .expect("valid date");
        let render = |format: TimestampFormat| moment.format(format.pattern()).to_string();
        assert_eq!(render(TimestampFormat::DateTime), "03.10.2026 09:05:07:042");
        assert_eq!(render(TimestampFormat::Time), "09:05:07:042");

        // A byte no variant wrote is not a format, and the reader falls back to
        // the default rather than guessing.
        assert_eq!(
            timestamp_format_from_discriminant(0),
            Some(TimestampFormat::Time)
        );
        assert_eq!(
            timestamp_format_from_discriminant(1),
            Some(TimestampFormat::DateTime)
        );
        assert_eq!(timestamp_format_from_discriminant(2), None);
    }

    /// Color codes in a message reach no layer that cannot show them: a plain
    /// layer gets the text alone.
    #[test]
    fn color_codes_are_stripped_for_a_plain_layer() {
        use std::sync::{Arc, Mutex};

        assert_eq!(
            strip_color_codes("\u{1b}[1;36mpill_spline\u{1b}[0m  extension"),
            "pill_spline  extension"
        );
        assert_eq!(strip_color_codes("no codes"), "no codes");

        let captured = Arc::new(Mutex::new(String::new()));
        let writer = CapturingMakeWriter(Arc::clone(&captured));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .event_format(EngineTerminalFormatter::new().without_timestamps()),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                target: telemetry_target::HOT_RELOAD,
                "{}",
                log_block_colored(
                    "\u{1b}[1mModules to build:\u{1b}[0m",
                    ["1. \u{1b}[36mpill_spline\u{1b}[0m  extension"]
                )
            );
        });
        let output = captured.lock().unwrap().clone();
        assert!(!output.contains('\u{1b}'), "{output}");
        assert!(
            output.contains("Modules to build:\n│ 1. pill_spline  extension"),
            "{output}"
        );
    }

    /// A multi-line message logs as a block: the heading on the prefix line
    /// before the fields, then the body behind its gutter, with no empty line
    /// after it.
    #[test]
    fn terminal_formatter_lays_out_a_multi_line_block() {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(String::new()));
        let writer = CapturingMakeWriter(Arc::clone(&captured));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .event_format(EngineTerminalFormatter::new().without_timestamps()),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                target: telemetry_target::HOT_RELOAD,
                total = 2,
                "{}",
                log_block("Modules to build:", ["1. pill_spline", "2. project"])
            );
        });
        let output = captured.lock().unwrap().clone();
        let lines: Vec<&str> = output.lines().collect();
        // Only the start of the prefix line: another test may switch the
        // process-wide source location on while this one runs.
        assert!(
            lines[0].starts_with("INFO  engine::hot_reload  Modules to build: | total=2"),
            "{output}"
        );
        assert_eq!(lines[1..], ["│ 1. pill_spline", "│ 2. project"]);
        assert!(output.ends_with("│ 2. project\n"), "{output:?}");
    }

    /// Render one event through the terminal formatter with no timestamps and
    /// no colors, for the exact-text block tests below.
    fn render_plain(emit: impl FnOnce()) -> String {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(String::new()));
        let writer = CapturingMakeWriter(Arc::clone(&captured));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .event_format(EngineTerminalFormatter::new().without_timestamps()),
        );
        tracing::subscriber::with_default(subscriber, emit);
        let output = captured.lock().unwrap().clone();
        output
    }

    /// A plain block renders exactly: heading inline, body below, nothing
    /// after. Source locations are opt-in and another test may switch them on,
    /// so the check reads only the start of the prefix line.
    #[test]
    fn terminal_formatter_renders_a_plain_block_exactly() {
        let output = render_plain(|| {
            tracing::info!(
                target: telemetry_target::SYSTEM,
                "{}",
                log_block("system specs", ["CPU: test", "Cores: 4"])
            );
        });
        let lines: Vec<&str> = output.lines().collect();
        assert!(
            lines[0].starts_with("INFO  engine::system  system specs"),
            "{output}"
        );
        assert_eq!(lines[1..], ["│ CPU: test", "│ Cores: 4"], "{output}");
    }

    /// A coloured heading reaches a plain layer as plain text on the prefix
    /// line.
    #[test]
    fn terminal_formatter_renders_a_colored_block_heading_inline() {
        let output = render_plain(|| {
            tracing::info!(
                target: telemetry_target::HOT_RELOAD,
                "{}",
                log_block_colored("reload analytics".cyan().bold(), ["build=1ms"])
            );
        });
        let lines: Vec<&str> = output.lines().collect();
        assert!(
            lines[0].starts_with("INFO  engine::hot_reload  reload analytics"),
            "{output}"
        );
        assert!(!output.contains('\u{1b}'), "{output:?}");
        assert_eq!(lines[1..], ["│ build=1ms"], "{output}");
    }

    /// A message that starts with a line break has no heading: the prefix
    /// line keeps the fields and the body follows.
    #[test]
    fn terminal_formatter_renders_a_headless_block() {
        let output = render_plain(|| {
            tracing::info!(target: telemetry_target::ECS, rows = 2, "\nfirst\nsecond\n");
        });
        let lines: Vec<&str> = output.lines().collect();
        assert!(
            lines[0].starts_with("INFO  engine::ecs  rows=2"),
            "{output}"
        );
        assert_eq!(lines[1..], ["│ first", "│ second"], "{output}");
    }

    /// The message is separated from its fields, the source location appears
    /// only when enabled and then trails the line, and a layer without ANSI
    /// gets no escape codes even for highlighted fields.
    ///
    /// The only test that changes the source location setting, so the toggle
    /// cannot race another test's expectation.
    #[test]
    fn terminal_formatter_separates_message_fields_and_location() {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(String::new()));
        let emit = || {
            let writer = CapturingMakeWriter(Arc::clone(&captured));
            let subscriber = tracing_subscriber::registry().with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .event_format(EngineTerminalFormatter::new().without_timestamps()),
            );
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(
                    target: telemetry_target::HOT_RELOAD,
                    module = "pill_spline",
                    owner = 2,
                    "extension loaded"
                );
            });
            let output = std::mem::take(&mut *captured.lock().unwrap());
            output.lines().next().expect("one line").to_owned()
        };

        let hidden = emit();
        assert_eq!(
            hidden, "INFO  engine::hot_reload  extension loaded | module=\"pill_spline\" owner=2",
            "the location is off by default"
        );

        set_show_source_location(true);
        let shown = emit();
        set_show_source_location(false);
        assert!(
            shown.starts_with(&format!("{hidden}  ")) && shown.contains("telemetry.rs:"),
            "missing trailing location: {shown}"
        );
        assert!(
            !shown.contains('\u{1b}'),
            "escape codes in a plain lane: {shown}"
        );
    }

    /// A line with no fields has no separator to show.
    #[test]
    fn terminal_formatter_omits_the_separator_without_fields() {
        let output = render_plain(|| {
            tracing::info!(target: telemetry_target::HOT_RELOAD, "source change detected");
        });
        assert!(
            output.starts_with("INFO  engine::hot_reload  source change detected"),
            "{output}"
        );
        assert!(!output.contains(FIELD_SEPARATOR), "{output}");
    }

    /// The separator only marks where the fields begin: a message that
    /// contains a pipe is still a message.
    #[test]
    fn terminal_formatter_leaves_a_pipe_in_the_message_alone() {
        let output = render_plain(|| {
            tracing::info!(target: telemetry_target::HOT_RELOAD, total = 1, "modules | ready");
        });
        assert!(
            output.starts_with("INFO  engine::hot_reload  modules | ready | total=1"),
            "{output}"
        );
    }

    /// A capture buffer that `tracing_subscriber`'s `MakeWriter` writes into.
    struct CapturingMakeWriter(Arc<std::sync::Mutex<String>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingMakeWriter {
        type Writer = CapturingWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CapturingWriter(Arc::clone(&self.0))
        }
    }

    /// Writes into a shared capture buffer.
    struct CapturingWriter(Arc<std::sync::Mutex<String>>);

    impl std::io::Write for CapturingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut output = self.0.lock().unwrap();
            output.push_str(&String::from_utf8_lossy(buf));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A later directive for a target replaces an earlier one, which is what
    /// lets project settings layer over the engine's defaults.
    #[test]
    fn a_later_directive_for_a_target_wins() {
        use tracing::level_filters::LevelFilter;
        let filter = LoggingConfig::default_engine()
            .with_directive("engine::rendering", LevelFilter::DEBUG)
            .build_env_filter()
            .unwrap();
        let rendered = format!("{filter}");
        assert!(rendered.contains("engine::rendering=debug"), "{rendered}");
        assert!(!rendered.contains("engine::rendering=info"), "{rendered}");
    }

    /// A fresh `LoggingConfig` parses and emits a filter string.
    #[test]
    fn logging_config_round_trips_through_env_filter() {
        use tracing::level_filters::LevelFilter;
        let logging_config = LoggingConfig::new()
            .with_directive("engine::rendering", LevelFilter::DEBUG)
            .with_directive("wgpu", LevelFilter::WARN);
        let filter = logging_config.build_env_filter().unwrap();
        let rendered = format!("{filter}");
        assert!(rendered.contains("engine::rendering"));
        assert!(rendered.contains("wgpu"));
    }

    /// The simple developer macros emit on the `engine::dev` lane.
    #[cfg(feature = "dev-logs")]
    #[test]
    fn developer_macros_emit_on_the_dev_lane() {
        use crate::{dev_error, dev_warn, log};
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(String::new()));
        let writer = CapturingMakeWriter(Arc::clone(&captured));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .event_format(EngineTerminalFormatter::new().without_timestamps()),
        );
        tracing::subscriber::with_default(subscriber, || {
            log!("frame = 42");
            dev_warn!("invalid render queue key");
            dev_error!("failed to create texture: disk full");
        });
        let output = captured.lock().unwrap().clone();
        assert!(
            output.contains("engine::dev"),
            "missing dev target: {output}"
        );
        assert!(
            output.contains("DEBUG"),
            "log! should map to DEBUG: {output}"
        );
        assert!(
            output.contains("WARN"),
            "dev_warn! should map to WARN: {output}"
        );
        assert!(
            output.contains("ERROR"),
            "dev_error! should map to ERROR: {output}"
        );
        assert!(
            output.contains("frame = 42"),
            "missing log! message: {output}"
        );
        assert!(
            output.contains("invalid render queue key"),
            "missing dev_warn! message: {output}"
        );
    }

    /// The reload handles reject invalid filters strictly and, with a file
    /// layer installed, reload the file filter too.
    ///
    /// This test and the concurrency test both install the process-wide
    /// subscriber, so both use a file layer and either may be the one that
    /// installs it.
    #[test]
    fn reload_handles_work_for_terminal_and_file_layers() {
        let directory = std::env::temp_dir().join(format!(
            "ecs-telemetry-reload-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let handles = TelemetryBuilder::new()
            .with_logging_config(LoggingConfig::new())
            .with_file_output(LoggingConfig::new(), &directory)
            .init()
            .expect("telemetry install should succeed");
        assert!(handles.file_filter.is_some());

        // A valid configuration reloads both layers; an unparsable directive is
        // refused and leaves the live filters alone.
        assert!(handles.reload_logging_config(&LoggingConfig::new()).is_ok());
        assert!(handles.reload_file_config(&LoggingConfig::new()).is_ok());
        let broken =
            LoggingConfig::new().with_directive("a=b=c", tracing::level_filters::LevelFilter::INFO);
        assert!(handles.reload_logging_config(&broken).is_err());
        assert!(handles.reload_file_config(&broken).is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    /// Concurrent `init` calls install the subscriber exactly once, and every
    /// caller receives the same installed handle set.
    ///
    /// The single `OnceLock` is what provides this. Uses a file layer like
    /// `reload_handles_work_for_terminal_and_file_layers` so the two tests
    /// agree on what "installed" means whichever one wins the process-wide race.
    #[test]
    fn concurrent_init_installs_exactly_once() {
        let directory = std::env::temp_dir().join(format!(
            "ecs-telemetry-concurrent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let handles: Vec<TelemetryHandles> = std::thread::scope(|scope| {
            let mut joiners = Vec::new();
            for _ in 0..8 {
                joiners.push(scope.spawn(|| {
                    TelemetryBuilder::new()
                        .with_logging_config(LoggingConfig::new())
                        .with_file_output(LoggingConfig::new(), &directory)
                        .init()
                        .expect("concurrent install must succeed")
                }));
            }
            joiners
                .into_iter()
                .map(|joiner| joiner.join().expect("install thread must not panic"))
                .collect()
        });

        // Every caller holds the same install: the file writer guards are the
        // same `Arc`, which is only possible if the `OnceLock` ran the installer
        // once and every other caller read that result.
        let first_file_guard = handles[0]
            ._file_guard
            .as_ref()
            .expect("the shared install must carry a file lane");
        for handle in &handles[1..] {
            let file_guard = handle
                ._file_guard
                .as_ref()
                .expect("every caller must see the same install");
            assert!(
                std::sync::Arc::ptr_eq(first_file_guard, file_guard),
                "concurrent callers must share one install, not each get their own"
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }

    /// Legacy `log`-crate records are bridged into the tracing subscriber on
    /// their own target, so dependency diagnostics reach the same layers.
    #[test]
    fn log_records_are_bridged_into_tracing() {
        use std::sync::{Arc, Mutex};

        // The global `log` -> `tracing` bridge installs once per process; a
        // repeated init only reports that it is already active, which still
        // leaves the bridge in place for this test.
        let _ = tracing_log::LogTracer::init();

        let captured = Arc::new(Mutex::new(String::new()));
        let writer = CapturingMakeWriter(Arc::clone(&captured));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .event_format(EngineTerminalFormatter::new().without_timestamps()),
        );
        tracing::subscriber::with_default(subscriber, || {
            // Warnings and errors only: the engine's bridge drops lower
            // levels, and whichever test installs it first sets that cap.
            log::warn!(target: "wgpu", "adapter selected");
            log::error!(target: "winit", "swapchain lost");
        });
        let output = captured.lock().unwrap().clone();
        assert!(output.contains("wgpu"), "missing log target: {output}");
        assert!(
            output.contains("adapter selected"),
            "missing log message: {output}"
        );
        assert!(
            output.contains("winit"),
            "missing second log target: {output}"
        );
        assert!(
            output.contains("swapchain lost"),
            "missing warn message: {output}"
        );
        // Shown under their own targets, without the bridge's `log.*` fields.
        assert!(
            output.contains("WARN  wgpu  adapter selected"),
            "bridged record not normalized: {output}"
        );
        assert!(
            !output.contains("log."),
            "bridge fields leaked into the line: {output}"
        );
    }
}
