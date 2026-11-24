//! Project settings: the declared surface, the sparse documents, and the one
//! resolution that composes them.
//!
//! # Responsibilities
//!
//! - Declares one setting ([`SettingSpec`]) and the schema of one entry
//!   ([`ModuleSettingsSchema`]): a module, the engine, the host or the editor.
//! - Holds the plain value a resolved setting is ([`SettingsValue`]) and the
//!   sparse documents a project writes ([`SettingsDocument`]).
//! - Resolves default < project file < local file < environment into
//!   [`ResolvedSettings`], recording where every value came from.
//! - Reports what cannot be used: an entry, group or key that is not declared,
//!   a value of the wrong type or outside its range, an unknown `PILL__`
//!   variable, and a top-level key that is not a setting. Strict mode turns
//!   those reports into an error.
//!
//! # Design
//!
//! A module declares its settings once, as data: the schema is a static slice
//! of [`SettingSpec`] values, so a declaration allocates nothing and a host can
//! read it before it applies anything. Resolved values are never a module type:
//! [`ResolvedSettings`] maps dotted paths to plain values, and each generation
//! reads its typed struct out of that map, so no value of a module type
//! crosses a reload.
//!
//! Nothing here reads a registry, a file or the process environment. The
//! caller passes all three in - the schemas, the documents and the environment
//! pairs - which keeps the resolution testable and keeps this crate free of
//! host policy. The walk runs over the *declared* paths rather than over the
//! document: a declared default is always present, a document only overrides
//! one, and a document path no schema declares is exactly the diagnostic a
//! mistyped key has to produce.

// Standard library
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

// Current crate
// Current crate
use crate::ecs::world::World;
use crate::resource::Resource;

// =============================================================================
// Constants
// =============================================================================

/// Prefix of every environment variable that overrides a setting.
pub const ENVIRONMENT_PREFIX: &str = "PILL__";

/// Separator between the parts of an environment variable's name.
///
/// Two underscores, because a crate name is full of single ones.
pub const ENVIRONMENT_SEPARATOR: &str = "__";

/// The key every entry declares first: whether it is on.
pub const ENABLED_KEY: &str = "enabled";

/// Top-level `project_settings.yaml` keys that describe the project instead of
/// configuring anything. Every other top-level scalar is reported as a setting
/// that is in the wrong place.
pub const PROJECT_METADATA_KEYS: [&str; 5] = [
    "name",
    "build_binary_name",
    "version",
    "author",
    "description",
];

// =============================================================================
// SettingKind
// =============================================================================

/// The kind of value a setting holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettingKind {
    /// On or off.
    Bool,
    /// A whole number.
    Integer,
    /// A number with a fractional part.
    Float,
    /// One line of text.
    Text,
    /// An ordered list of text values.
    List,
    /// F Keys mapped to values, for a setting whose children are data rather
    /// than declared settings (a log target, for instance).
    Table,
}

impl SettingKind {
    /// The kind's name in a message and in a document.
    pub const fn as_str(self) -> &'static str {
        match self {
            SettingKind::Bool => "bool",
            SettingKind::Integer => "integer",
            SettingKind::Float => "float",
            SettingKind::Text => "text",
            SettingKind::List => "list",
            SettingKind::Table => "table",
        }
    }
}

impl fmt::Display for SettingKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// =============================================================================
// SettingsValue
// =============================================================================

/// One setting's value, in the plain form a resolved setting is stored in.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsValue {
    /// On or off.
    Bool(bool),
    /// A whole number.
    Integer(i64),
    /// A number with a fractional part.
    Float(f64),
    /// One line of text.
    Text(String),
    /// An ordered list.
    List(Vec<SettingsValue>),
    /// Keys mapped to values, in key order.
    Table(BTreeMap<String, SettingsValue>),
}

impl SettingsValue {
    /// Which kind of value this is.
    pub fn kind(&self) -> SettingKind {
        match self {
            SettingsValue::Bool(_) => SettingKind::Bool,
            SettingsValue::Integer(_) => SettingKind::Integer,
            SettingsValue::Float(_) => SettingKind::Float,
            SettingsValue::Text(_) => SettingKind::Text,
            SettingsValue::List(_) => SettingKind::List,
            SettingsValue::Table(_) => SettingKind::Table,
        }
    }

    /// Reads a YAML value.
    ///
    /// # Errors
    ///
    /// Returns a message when the value is a YAML tag or carries nothing a
    /// setting can hold.
    pub fn from_yaml(value: serde_yaml::Value) -> Result<Self, String> {
        match value {
            serde_yaml::Value::Bool(flag) => Ok(SettingsValue::Bool(flag)),
            serde_yaml::Value::Number(number) => {
                if let Some(integer) = number.as_i64() {
                    Ok(SettingsValue::Integer(integer))
                } else if let Some(float) = number.as_f64() {
                    Ok(SettingsValue::Float(float))
                } else {
                    Err(format!("{number} is not a number a setting can hold"))
                }
            }
            serde_yaml::Value::String(text) => Ok(SettingsValue::Text(text)),
            serde_yaml::Value::Sequence(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(Self::from_yaml(item)?);
                }
                Ok(SettingsValue::List(values))
            }
            serde_yaml::Value::Mapping(mapping) => {
                let mut table = BTreeMap::new();
                for (key, item) in mapping {
                    let serde_yaml::Value::String(key) = key else {
                        return Err("a table key must be text".to_string());
                    };
                    table.insert(key, Self::from_yaml(item)?);
                }
                Ok(SettingsValue::Table(table))
            }
            serde_yaml::Value::Null => Err("the value is empty".to_string()),
            serde_yaml::Value::Tagged(tagged) => {
                Err(format!("the tag `{}` is not a setting", tagged.tag))
            }
        }
    }

    /// The YAML value this setting is written as.
    pub fn to_yaml(&self) -> serde_yaml::Value {
        match self {
            SettingsValue::Bool(flag) => serde_yaml::Value::Bool(*flag),
            SettingsValue::Integer(integer) => serde_yaml::Value::Number((*integer).into()),
            SettingsValue::Float(float) => {
                serde_yaml::Value::Number(serde_yaml::Number::from(*float))
            }
            SettingsValue::Text(text) => serde_yaml::Value::String(text.clone()),
            SettingsValue::List(items) => {
                serde_yaml::Value::Sequence(items.iter().map(SettingsValue::to_yaml).collect())
            }
            SettingsValue::Table(table) => serde_yaml::Value::Mapping(
                table
                    .iter()
                    .map(|(key, value)| (serde_yaml::Value::String(key.clone()), value.to_yaml()))
                    .collect(),
            ),
        }
    }

    /// The value as a flag, or `None` when it is another kind.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            SettingsValue::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    /// The value as a whole number, or `None` when it is another kind.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            SettingsValue::Integer(integer) => Some(*integer),
            _ => None,
        }
    }

    /// The value as a float, accepting a whole number, or `None` when it is
    /// another kind.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            SettingsValue::Float(float) => Some(*float),
            SettingsValue::Integer(integer) => Some(*integer as f64),
            _ => None,
        }
    }

    /// The value as text, or `None` when it is another kind.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            SettingsValue::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The value as a list, or `None` when it is another kind.
    pub fn as_list(&self) -> Option<&[SettingsValue]> {
        match self {
            SettingsValue::List(items) => Some(items),
            _ => None,
        }
    }

    /// The value as a table, or `None` when it is another kind.
    pub fn as_table(&self) -> Option<&BTreeMap<String, SettingsValue>> {
        match self {
            SettingsValue::Table(table) => Some(table),
            _ => None,
        }
    }

    /// The value as one line, for a log field or a tooltip.
    pub fn describe(&self) -> String {
        match self {
            SettingsValue::Bool(flag) => flag.to_string(),
            SettingsValue::Integer(integer) => integer.to_string(),
            SettingsValue::Float(float) => float.to_string(),
            SettingsValue::Text(text) => text.clone(),
            SettingsValue::List(items) => items
                .iter()
                .map(SettingsValue::describe)
                .collect::<Vec<_>>()
                .join(", "),
            SettingsValue::Table(table) => {
                let entries: Vec<String> = table
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.describe()))
                    .collect();
                format!("{{{}}}", entries.join(", "))
            }
        }
    }
}

// =============================================================================
// SettingDefault
// =============================================================================

/// A declared default, in a form a static declaration can hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingDefault {
    /// Off unless a document says otherwise.
    Bool(bool),
    /// A whole number.
    Integer(i64),
    /// A number with a fractional part.
    Float(f64),
    /// One line of text.
    Text(&'static str),
    /// A list of text values.
    List(&'static [&'static str]),
    /// An empty table.
    EmptyTable,
}

impl SettingDefault {
    /// The kind this default declares.
    pub fn kind(&self) -> SettingKind {
        match self {
            SettingDefault::Bool(_) => SettingKind::Bool,
            SettingDefault::Integer(_) => SettingKind::Integer,
            SettingDefault::Float(_) => SettingKind::Float,
            SettingDefault::Text(_) => SettingKind::Text,
            SettingDefault::List(_) => SettingKind::List,
            SettingDefault::EmptyTable => SettingKind::Table,
        }
    }

    /// The default as a resolved value.
    pub fn value(&self) -> SettingsValue {
        match self {
            SettingDefault::Bool(flag) => SettingsValue::Bool(*flag),
            SettingDefault::Integer(integer) => SettingsValue::Integer(*integer),
            SettingDefault::Float(float) => SettingsValue::Float(*float),
            SettingDefault::Text(text) => SettingsValue::Text((*text).to_string()),
            SettingDefault::List(items) => SettingsValue::List(
                items
                    .iter()
                    .map(|item| SettingsValue::Text((*item).to_string()))
                    .collect(),
            ),
            SettingDefault::EmptyTable => SettingsValue::Table(BTreeMap::new()),
        }
    }
}

// =============================================================================
// ApplyTiming
// =============================================================================

/// When a changed value takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyTiming {
    /// The value decides a cargo feature: changing it rebuilds and restarts.
    Build,
    /// The value is read once, when the program starts.
    Startup,
    /// The value is applied again at the next update, with no restart.
    Live,
}

impl ApplyTiming {
    /// The timing's name in a message, a tooltip or the settings dump.
    pub const fn as_str(self) -> &'static str {
        match self {
            ApplyTiming::Build => "build",
            ApplyTiming::Startup => "startup",
            ApplyTiming::Live => "live",
        }
    }
}

// =============================================================================
// SettingSpec
// =============================================================================

/// One declared setting: where it lives, what it defaults to and when a change
/// takes effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingSpec {
    /// The group the setting sits in; empty when it sits directly under the
    /// entry, as `enabled` does.
    pub group: &'static str,
    /// The setting's own name.
    pub key: &'static str,
    /// What the setting is when nothing overrides it.
    pub default: SettingDefault,
    /// One line saying what the setting does, for the editor's rows and the
    /// settings dump.
    pub description: &'static str,
    /// The smallest accepted value, for a number.
    pub minimum: Option<f64>,
    /// The largest accepted value, for a number.
    pub maximum: Option<f64>,
    /// When a changed value takes effect.
    pub timing: ApplyTiming,
}

impl SettingSpec {
    /// Declares a setting that applies at startup and accepts any value of its
    /// kind.
    pub const fn new(
        group: &'static str,
        key: &'static str,
        default: SettingDefault,
        description: &'static str,
    ) -> Self {
        Self {
            group,
            key,
            default,
            description,
            minimum: None,
            maximum: None,
            timing: ApplyTiming::Startup,
        }
    }

    /// Declares the `enabled` setting every entry starts with.
    pub const fn enabled() -> Self {
        Self::new(
            "",
            ENABLED_KEY,
            SettingDefault::Bool(false),
            "Whether this entry is on. An entry nobody lists is off.",
        )
        .with_timing(ApplyTiming::Startup)
    }

    /// The same setting, taking effect at a different time.
    pub const fn with_timing(mut self, timing: ApplyTiming) -> Self {
        self.timing = timing;
        self
    }

    /// The same setting, with the range a number must fall in.
    pub const fn with_range(mut self, minimum: f64, maximum: f64) -> Self {
        self.minimum = Some(minimum);
        self.maximum = Some(maximum);
        self
    }

    /// The same setting, with only a lower bound.
    pub const fn with_minimum(mut self, minimum: f64) -> Self {
        self.minimum = Some(minimum);
        self
    }

    /// The same setting, with only an upper bound.
    pub const fn with_maximum(mut self, maximum: f64) -> Self {
        self.maximum = Some(maximum);
        self
    }

    /// What kind of value the setting holds.
    pub fn kind(&self) -> SettingKind {
        self.default.kind()
    }

    /// The setting's path inside its entry: `group.key`, or just `key` when it
    /// has no group.
    pub fn path(&self) -> String {
        if self.group.is_empty() {
            self.key.to_string()
        } else {
            format!("{}.{}", self.group, self.key)
        }
    }

    /// The setting's full dotted path, entry first.
    pub fn full_path(&self, entry: &str) -> String {
        format!("{entry}.{}", self.path())
    }

    /// The environment variable that overrides the setting:
    /// `PILL__<ENTRY>__<GROUP>__<KEY>`.
    pub fn environment_variable(&self, entry: &str) -> String {
        let group = if self.group.is_empty() {
            String::new()
        } else {
            format!(
                "{}{}",
                self.group.to_ascii_uppercase(),
                ENVIRONMENT_SEPARATOR
            )
        };
        format!(
            "{ENVIRONMENT_PREFIX}{}{}{group}{}",
            entry.to_ascii_uppercase(),
            ENVIRONMENT_SEPARATOR,
            self.key.to_ascii_uppercase()
        )
    }
}

// =============================================================================
// ModuleSettingsSchema
// =============================================================================

/// Every setting one entry declares: a module, the engine, the host or the
/// editor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModuleSettingsSchema {
    /// The entry's name: a crate name, or `engine`, `host` or `editor`.
    pub module: &'static str,
    /// The declared settings, `enabled` first.
    pub settings: &'static [SettingSpec],
}

impl ModuleSettingsSchema {
    /// The schema of an entry.
    pub const fn new(module: &'static str, settings: &'static [SettingSpec]) -> Self {
        Self { module, settings }
    }

    /// The declared setting at `path` (`group.key` or `key`).
    pub fn spec(&self, path: &str) -> Option<&SettingSpec> {
        self.settings.iter().find(|spec| spec.path() == path)
    }

    /// Every declared path inside the entry.
    pub fn paths(&self) -> Vec<String> {
        self.settings.iter().map(SettingSpec::path).collect()
    }
}

// =============================================================================
// SettingsOutcome
// =============================================================================

/// What a manager did with the values it was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsOutcome {
    /// The values are in effect.
    Applied,
    /// The manager refused them, kept what it had, and logged why.
    Refused,
}

// =============================================================================
// DeclaredSettings
// =============================================================================

/// The typed form a module reads its settings through.
///
/// A derive implements this for the struct a module declares: the schema comes
/// from the struct's fields and decorators, and [`Self::from_resolved`] reads
/// each field out of the resolved tree, falling back to the declared default
/// when the entry is not in the tree at all.
pub trait DeclaredSettings {
    /// The entry the settings belong to.
    const MODULE: &'static str;

    /// Everything this entry declares.
    fn schema() -> ModuleSettingsSchema;

    /// Reads a typed value out of the resolved settings.
    fn from_resolved(resolved: &ResolvedSettings) -> Self;
}

// =============================================================================
// ModuleSettings
// =============================================================================

/// What a manager resource does with its settings.
///
/// The resource keeps its own validation and its own live values: the engine
/// hands it a typed settings struct and it decides whether the value is usable.
/// A manager with nothing to configure does not implement this trait.
pub trait ModuleSettings: Resource {
    /// The declared settings type, which also names the entry.
    type Values: DeclaredSettings;

    /// Applies a resolved settings value.
    ///
    /// Return [`SettingsOutcome::Refused`] when the value cannot be used - out
    /// of range for the subsystem's current state, or a value that cannot
    /// change while it runs - and leave the manager exactly as it was.
    fn apply_settings(&mut self, values: &Self::Values) -> SettingsOutcome;
}

/// Applies one manager's resolved settings, when it has a live value.
///
/// Returns `None` when the manager type is registered but no value exists,
/// which is the ordinary case before the artifact inserts its resource.
pub fn apply_module_settings<T: ModuleSettings>(
    world: &mut World,
    resolved: &ResolvedSettings,
) -> Option<SettingsOutcome> {
    let values = T::Values::from_resolved(resolved);
    world
        .get_resource_mut::<T>()
        .map(|manager| manager.apply_settings(&values))
}

/// Registers a manager's settings surface with an engine.
///
/// The artifact's registration half calls this once per generation. A module
/// that registers the same entry again - a reload, or two managers of one
/// module - replaces the earlier registration, so the applier always points
/// into the newest image.
pub fn register_settings<T: ModuleSettings>(engine: &mut crate::engine::Engine) {
    engine.register_module_settings::<T>();
}

// =============================================================================
// ModuleSettingsRegistration
// =============================================================================

/// One registered settings surface: what an entry declares and how its live
/// manager takes the values.
///
/// Both halves are function pointers into the artifact that registered them,
/// so the registration follows the same lifetime rule as a resource's drop
/// function: valid while that image is mapped, and replaced by the artifact's
/// next generation.
#[derive(Clone, Copy, Debug)]
pub struct ModuleSettingsRegistration {
    /// The entry the surface belongs to.
    pub module: &'static str,
    /// The entry's declared schema.
    pub schema: fn() -> ModuleSettingsSchema,
    /// Applies the resolved values to the live manager.
    pub apply: fn(&mut World, &ResolvedSettings) -> Option<SettingsOutcome>,
}

// =============================================================================
// SettingSource
// =============================================================================

/// Where a resolved value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettingSource {
    /// The declared default.
    Default,
    /// `project_settings.yaml`.
    Project,
    /// `project_settings.local.yaml`.
    Local,
    /// A `PILL__` environment variable.
    Environment,
}

impl SettingSource {
    /// The source's name in a log line, a tooltip or the settings dump.
    pub const fn as_str(self) -> &'static str {
        match self {
            SettingSource::Default => "default",
            SettingSource::Project => "project_settings.yaml",
            SettingSource::Local => "project_settings.local.yaml",
            SettingSource::Environment => "environment",
        }
    }
}

impl fmt::Display for SettingSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// =============================================================================
// ResolvedValue
// =============================================================================

/// One resolved setting and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedValue {
    /// The value in effect.
    pub value: SettingsValue,
    /// Where it came from.
    pub source: SettingSource,
}

// =============================================================================
// ResolvedSettings
// =============================================================================

/// Every entry's settings, resolved, with the reports the resolution made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedSettings {
    entries: BTreeMap<String, BTreeMap<String, ResolvedValue>>,
    diagnostics: Vec<SettingsDiagnostic>,
}

impl ResolvedSettings {
    /// The resolved value at `path` inside `entry`, when the entry is present.
    pub fn value(&self, entry: &str, path: &str) -> Option<&ResolvedValue> {
        self.entries.get(entry)?.get(path)
    }

    /// Whether the entry is on; an entry the resolution does not hold is off.
    pub fn is_enabled(&self, entry: &str) -> bool {
        self.boolean(entry, ENABLED_KEY, false)
    }

    /// The entry's settings, when the resolution holds them.
    pub fn entry(&self, entry: &str) -> Option<&BTreeMap<String, ResolvedValue>> {
        self.entries.get(entry)
    }

    /// Every entry the resolution holds, in name order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &BTreeMap<String, ResolvedValue>)> {
        self.entries
            .iter()
            .map(|(name, values)| (name.as_str(), values))
    }

    /// A flag at `path`, or `fallback` when it is not there or another kind.
    pub fn boolean(&self, entry: &str, path: &str, fallback: bool) -> bool {
        self.value(entry, path)
            .and_then(|resolved| resolved.value.as_bool())
            .unwrap_or(fallback)
    }

    /// A whole number at `path`, or `fallback` when it is not there or another
    /// kind.
    pub fn integer(&self, entry: &str, path: &str, fallback: i64) -> i64 {
        self.value(entry, path)
            .and_then(|resolved| resolved.value.as_integer())
            .unwrap_or(fallback)
    }

    /// A float at `path`, or `fallback` when it is not there or another kind.
    pub fn float(&self, entry: &str, path: &str, fallback: f64) -> f64 {
        self.value(entry, path)
            .and_then(|resolved| resolved.value.as_float())
            .unwrap_or(fallback)
    }

    /// Text at `path`, or `fallback` when it is not there or another kind.
    pub fn text<'a>(&'a self, entry: &str, path: &str, fallback: &'a str) -> &'a str {
        self.value(entry, path)
            .and_then(|resolved| resolved.value.as_text())
            .unwrap_or(fallback)
    }

    /// A list at `path`, or nothing when it is not there or another kind.
    pub fn list(&self, entry: &str, path: &str) -> &[SettingsValue] {
        self.value(entry, path)
            .and_then(|resolved| resolved.value.as_list())
            .unwrap_or(&[])
    }

    /// A table at `path`, or nothing when it is not there or another kind.
    pub fn table(&self, entry: &str, path: &str) -> Option<&BTreeMap<String, SettingsValue>> {
        self.value(entry, path)
            .and_then(|resolved| resolved.value.as_table())
    }

    /// What the resolution reported: unknown paths, refused values and unknown
    /// variables.
    pub fn diagnostics(&self) -> &[SettingsDiagnostic] {
        &self.diagnostics
    }

    /// Every value that is not its declared default, as `entry.path`, with the
    /// value and its source, in entry then path order.
    pub fn non_default_values(&self) -> Vec<(String, &ResolvedValue)> {
        let mut values = Vec::new();
        for (entry, settings) in &self.entries {
            for (path, resolved) in settings {
                if resolved.source != SettingSource::Default {
                    values.push((format!("{entry}.{path}"), resolved));
                }
            }
        }
        values
    }

    /// Every resolved value, as `entry.path`, with the value and its source.
    pub fn all_values(&self) -> Vec<(String, &ResolvedValue)> {
        let mut values = Vec::new();
        for (entry, settings) in &self.entries {
            for (path, resolved) in settings {
                values.push((format!("{entry}.{path}"), resolved));
            }
        }
        values
    }

    fn insert(&mut self, entry: &str, path: &str, value: ResolvedValue) {
        self.entries
            .entry(entry.to_string())
            .or_default()
            .insert(path.to_string(), value);
    }

    fn report(&mut self, diagnostic: SettingsDiagnostic) {
        self.diagnostics.push(diagnostic);
    }
}

// =============================================================================
// SettingsDiagnostic
// =============================================================================

/// One report the resolution makes about a setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsDiagnostic {
    /// The dotted path the report is about, entry first.
    pub path: String,
    /// What is wrong with it.
    pub message: String,
    /// The closest declared path, when one is close enough to be a typo.
    pub suggestion: Option<String>,
}

impl SettingsDiagnostic {
    /// A report about `path`.
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            suggestion: None,
        }
    }

    /// The same report, naming the path the writer probably meant.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }
}

impl fmt::Display for SettingsDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)?;
        if let Some(suggestion) = &self.suggestion {
            write!(formatter, " (did you mean `{suggestion}`?)")?;
        }
        Ok(())
    }
}

// =============================================================================
// SettingsDocumentError
// =============================================================================

/// Why a settings file could not be read as a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsDocumentError {
    /// What is wrong with the file.
    pub message: String,
}

impl SettingsDocumentError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SettingsDocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SettingsDocumentError {}

// =============================================================================
// SettingsDocument
// =============================================================================

/// One sparse settings document: the entries a project file or its local
/// override sets.
///
/// The document keeps only what it was given. A top-level table is an entry -
/// `modules:*` is flattened into one entry per module, and `engine`, `host`
/// and `editor` are entries of their own - while a top-level scalar is project
/// metadata when it is one of [`PROJECT_METADATA_KEYS`] and a misplaced setting
/// otherwise.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsDocument {
    /// The entries the document sets, in the order the file lists them - which
    /// is the order modules load in.
    entries: Vec<(String, SettingsValue)>,
    /// The entries that came from under `modules:`, so a module is told apart
    /// from a table that belongs at the top level.
    module_entries: BTreeSet<String>,
    /// Top-level keys that are neither metadata nor an entry.
    misplaced_keys: Vec<String>,
}

impl SettingsDocument {
    /// A document with nothing in it.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A document built from entries, for a caller that already has them.
    ///
    /// The order is the order the caller gives, because a document's module
    /// order is its load order.
    pub fn from_entries(entries: Vec<(String, SettingsValue)>) -> Self {
        Self {
            entries,
            module_entries: BTreeSet::new(),
            misplaced_keys: Vec::new(),
        }
    }

    /// Reads a YAML file's text.
    ///
    /// # Errors
    ///
    /// Returns a message when the text is not valid YAML, when the top level
    /// is not a mapping, or when a `modules:` entry is not a mapping.
    pub fn from_yaml_text(text: &str) -> Result<Self, SettingsDocumentError> {
        let value: serde_yaml::Value = serde_yaml::from_str(text)
            .map_err(|error| SettingsDocumentError::new(format!("not valid YAML: {error}")))?;
        Self::from_yaml_value(value)
    }

    /// Reads a YAML value that must be a mapping.
    ///
    /// # Errors
    ///
    /// Returns a message when the value is not a mapping, when a `modules:`
    /// entry is not a mapping, or when an entry holds something a document
    /// cannot.
    pub fn from_yaml_value(value: serde_yaml::Value) -> Result<Self, SettingsDocumentError> {
        // A file holding only comments parses to nothing; it sets nothing.
        if value.is_null() {
            return Ok(Self::empty());
        }
        let serde_yaml::Value::Mapping(mapping) = value else {
            return Err(SettingsDocumentError::new(
                "the top level is not a mapping of settings",
            ));
        };
        let mut document = Self::empty();
        for (key, item) in mapping {
            let serde_yaml::Value::String(key) = key else {
                return Err(SettingsDocumentError::new(
                    "a top-level key is not text".to_string(),
                ));
            };
            if key == "modules" {
                // An empty list is the other way to say "no module": it means
                // the same as an empty mapping, so a project that lists none has
                // one spelling fewer to get wrong.
                let modules = match item {
                    serde_yaml::Value::Mapping(modules) => modules,
                    serde_yaml::Value::Sequence(entries) if entries.is_empty() => {
                        serde_yaml::Mapping::new()
                    }
                    _ => {
                        return Err(SettingsDocumentError::new(
                            "`modules` is not a mapping of module names to their settings",
                        ));
                    }
                };
                for (module, settings) in modules {
                    let serde_yaml::Value::String(module) = module else {
                        return Err(SettingsDocumentError::new(
                            "a module name is not text".to_string(),
                        ));
                    };
                    let item = drop_empty_values(settings);
                    let settings = SettingsValue::from_yaml(item).map_err(|error| {
                        SettingsDocumentError::new(format!("modules.{module}: {error}"))
                    })?;
                    document.entries.push((module.clone(), settings));
                    document.module_entries.insert(module);
                }
                continue;
            }
            if PROJECT_METADATA_KEYS.contains(&key.as_str()) {
                continue;
            }
            if matches!(item, serde_yaml::Value::Mapping(_)) {
                let item = drop_empty_values(item);
                let settings = SettingsValue::from_yaml(item)
                    .map_err(|error| SettingsDocumentError::new(format!("{key}: {error}")))?;
                document.entries.push((key, settings));
            } else {
                // A scalar here is a setting in the old flat shape, or a
                // misspelled entry. Either way it belongs nowhere in a
                // document, and the resolution says so by name.
                document.misplaced_keys.push(key);
            }
        }
        Ok(document)
    }

    /// Whether the document sets nothing at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.misplaced_keys.is_empty()
    }

    /// The entries the document sets, in the order the file lists them.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &SettingsValue)> {
        self.entries
            .iter()
            .map(|(name, settings)| (name.as_str(), settings))
    }

    /// The modules the project lists, in the order they load.
    pub fn module_names(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(|(name, _)| self.module_entries.contains(name))
            .map(|(name, _)| name.as_str())
    }

    /// Whether an entry came from under `modules:`.
    pub fn is_module(&self, name: &str) -> bool {
        self.module_entries.contains(name)
    }

    /// One entry's settings, when the document sets any.
    pub fn entry(&self, name: &str) -> Option<&SettingsValue> {
        self.entries
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, settings)| settings)
    }

    /// The value at `path` inside `entry`, walking nested tables.
    pub fn value_at(&self, entry: &str, path: &str) -> Option<&SettingsValue> {
        let mut current = self.entry(entry)?;
        for segment in path.split('.') {
            current = current.as_table()?.get(segment)?;
        }
        Some(current)
    }

    /// The top-level keys that are neither metadata nor an entry.
    pub fn misplaced_keys(&self) -> &[String] {
        &self.misplaced_keys
    }
}

/// Removes the values a file writes with nothing after them.
///
/// `theme:` with no value reads as null, and a person writing that means "not
/// set", not "a value of no kind".
fn drop_empty_values(value: serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Mapping(mapping) => serde_yaml::Value::Mapping(
            mapping
                .into_iter()
                .filter_map(|(key, item)| match item {
                    serde_yaml::Value::Null => None,
                    other => Some((key, drop_empty_values(other))),
                })
                .collect(),
        ),
        serde_yaml::Value::Sequence(items) => serde_yaml::Value::Sequence(
            items
                .into_iter()
                .filter_map(|item| match item {
                    serde_yaml::Value::Null => None,
                    other => Some(drop_empty_values(other)),
                })
                .collect(),
        ),
        other => other,
    }
}

// =============================================================================
// SettingsEnvironment
// =============================================================================
/// The environment variables a resolution reads, captured once so a test can
/// supply its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsEnvironment {
    pairs: Vec<(String, String)>,
}

impl SettingsEnvironment {
    /// An environment with no variables.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Every `PILL__` variable of the current process.
    pub fn from_process() -> Self {
        let pairs = std::env::vars()
            .filter(|(name, _)| name.starts_with(ENVIRONMENT_PREFIX))
            .collect();
        Self { pairs }
    }

    /// An environment from pairs, keeping only the `PILL__` ones.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        let pairs = pairs
            .into_iter()
            .filter(|(name, _)| name.starts_with(ENVIRONMENT_PREFIX))
            .collect();
        Self { pairs }
    }

    /// One variable's text.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Every variable, in the order it was captured.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.pairs
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

// =============================================================================
// SettingsResolutionInput
// =============================================================================

/// Everything one resolution reads.
#[derive(Clone, Copy, Debug)]
pub struct SettingsResolutionInput<'a> {
    /// The declared schemas, in any order.
    pub schemas: &'a [ModuleSettingsSchema],
    /// The entries whose code is present: the enabled modules, plus `engine`,
    /// `host` and `editor`.
    pub loaded_entries: &'a [&'a str],
    /// The entries the project lists but turned off; their settings are not
    /// checked, which is reported once per entry.
    pub disabled_entries: &'a [&'a str],
    /// The entries whose schema is not known yet: a module declares its
    /// settings in its own crate, so the run that reads the project's files
    /// before any artifact has loaded cannot check them. Nothing is reported
    /// for such an entry now; the resolution that runs after the modules load
    /// checks it.
    pub deferred_entries: &'a [&'a str],
    /// The project's own document.
    pub project: Option<&'a SettingsDocument>,
    /// The local override, when the caller reads one.
    pub local: Option<&'a SettingsDocument>,
    /// The environment variables that override settings.
    pub environment: &'a SettingsEnvironment,
    /// Whether a report is fatal.
    pub strict: bool,
}

// =============================================================================
// SettingsErrors
// =============================================================================

/// What a strict resolution refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsErrors {
    diagnostics: Vec<SettingsDiagnostic>,
}

impl SettingsErrors {
    /// The reports that made the resolution fail.
    pub fn diagnostics(&self) -> &[SettingsDiagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for SettingsErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} setting problem{}: ",
            self.diagnostics.len(),
            if self.diagnostics.len() == 1 { "" } else { "s" }
        )?;
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{diagnostic}")?;
        }
        Ok(())
    }
}

impl Error for SettingsErrors {}

// =============================================================================
// Resolution
// =============================================================================

/// Resolves every declared setting, in the order default < project < local <
/// environment.
///
/// # Errors
///
/// With `strict`, returns every report the resolution made instead of recording
/// it on the result.
pub fn resolve(input: SettingsResolutionInput<'_>) -> Result<ResolvedSettings, SettingsErrors> {
    let mut resolved = ResolvedSettings::default();
    let schemas: BTreeMap<&str, &ModuleSettingsSchema> = input
        .schemas
        .iter()
        .map(|schema| (schema.module, schema))
        .collect();
    let loaded: BTreeSet<&str> = input.loaded_entries.iter().copied().collect();
    let disabled: BTreeSet<&str> = input.disabled_entries.iter().copied().collect();
    let deferred: BTreeSet<&str> = input.deferred_entries.iter().copied().collect();

    // Step 1: every declared setting of every present entry.
    for (module, schema) in &schemas {
        if disabled.contains(module) {
            resolved.report(SettingsDiagnostic::new(
                *module,
                "the entry is disabled; its settings are not checked",
            ));
            continue;
        }
        if !loaded.contains(module) {
            continue;
        }
        for spec in schema.settings {
            let path = spec.path();
            let (value, report) = resolve_setting(module, spec, &input);
            resolved.insert(module, &path, value);
            if let Some(report) = report {
                resolved.report(report);
            }
        }
    }

    // Step 2: what the documents set that no schema declares.
    for document in [input.project, input.local] {
        let Some(document) = document else {
            continue;
        };
        for (entry, settings) in document.entries() {
            let declared = schemas.get(entry).copied();
            if disabled.contains(entry) || (declared.is_none() && deferred.contains(entry)) {
                continue;
            }
            if declared.is_none() && !loaded.contains(entry) {
                let mut diagnostic = SettingsDiagnostic::new(
                    entry,
                    "no such entry: it is neither a listed module nor the engine, host or editor",
                );
                if let Some(suggestion) =
                    closest(entry, schemas.keys().map(|name| (*name).to_string()))
                {
                    diagnostic = diagnostic.with_suggestion(suggestion);
                }
                resolved.report(diagnostic);
                continue;
            }
            report_unknown_paths(entry, settings, declared, &mut resolved, &schemas);
        }
        for key in document.misplaced_keys() {
            resolved.report(SettingsDiagnostic::new(
                key,
                "a top-level key is not a setting; put it under a module entry, `engine:`, \
                 `host:` or `editor:`",
            ));
        }
    }

    // Step 3: environment variables no declaration accepts.
    for (name, _) in input.environment.pairs() {
        let Some((entry, path)) = split_environment_name(name) else {
            resolved.report(SettingsDiagnostic::new(
                name,
                "an environment variable does not name `PILL__<ENTRY>__<KEY>` or \
                 `PILL__<ENTRY>__<GROUP>__<KEY>`",
            ));
            continue;
        };
        if disabled.contains(entry.as_str()) {
            continue;
        }
        let known = schemas
            .get(entry.as_str())
            .and_then(|schema| schema.spec(&path))
            .is_some();
        if !known {
            let full = format!("{entry}.{path}");
            let mut diagnostic =
                SettingsDiagnostic::new(full.clone(), format!("no setting is named by `{name}`"));
            if let Some(suggestion) = closest_setting(&full, &schemas) {
                diagnostic = diagnostic.with_suggestion(suggestion);
            }
            resolved.report(diagnostic);
        }
    }

    if input.strict && !resolved.diagnostics.is_empty() {
        return Err(SettingsErrors {
            diagnostics: resolved.diagnostics,
        });
    }
    Ok(resolved)
}

/// Resolves one declared setting from the four sources, in order.
fn resolve_setting(
    module: &str,
    spec: &SettingSpec,
    input: &SettingsResolutionInput<'_>,
) -> (ResolvedValue, Option<SettingsDiagnostic>) {
    let path = spec.path();
    let default = spec.default.value();

    // Step 1: the environment wins over both files.
    let variable = spec.environment_variable(module);
    if let Some(raw) = input.environment.get(&variable) {
        return match parse_environment_value(spec, raw) {
            Ok(value) => (
                ResolvedValue {
                    value,
                    source: SettingSource::Environment,
                },
                None,
            ),
            Err(reason) => (
                ResolvedValue {
                    value: default.clone(),
                    source: SettingSource::Default,
                },
                Some(SettingsDiagnostic::new(
                    spec.full_path(module),
                    format!("`{raw}` from {variable} is not usable: {reason}"),
                )),
            ),
        };
    }

    // Step 2: the local file over the project file.
    for (document, source) in [
        (input.local, SettingSource::Local),
        (input.project, SettingSource::Project),
    ] {
        let Some(document) = document else {
            continue;
        };
        let Some(value) = document.value_at(module, &path) else {
            continue;
        };
        return match accept(spec, value) {
            Ok(value) => (ResolvedValue { value, source }, None),
            Err(reason) => (
                ResolvedValue {
                    value: default.clone(),
                    source: SettingSource::Default,
                },
                Some(SettingsDiagnostic::new(
                    spec.full_path(module),
                    format!("{} is not usable: {reason}", source.as_str()),
                )),
            ),
        };
    }

    // Step 3: nothing overrides the default.
    (
        ResolvedValue {
            value: default,
            source: SettingSource::Default,
        },
        None,
    )
}

/// Checks a document value against its declaration.
fn accept(spec: &SettingSpec, value: &SettingsValue) -> Result<SettingsValue, String> {
    let expected = spec.kind();
    let accepted = match (expected, value) {
        (SettingKind::Float, SettingsValue::Integer(integer)) => {
            SettingsValue::Float(*integer as f64)
        }
        _ if value.kind() == expected => value.clone(),
        _ => {
            return Err(format!(
                "the file holds a {}, the setting is a {}",
                value.kind(),
                expected
            ));
        }
    };
    check_range(spec, &accepted)?;
    Ok(accepted)
}

/// Checks a numeric value against the setting's declared range.
fn check_range(spec: &SettingSpec, value: &SettingsValue) -> Result<(), String> {
    let Some(number) = value.as_float() else {
        return Ok(());
    };
    if let Some(minimum) = spec.minimum {
        if number < minimum {
            return Err(format!("{number} is below the minimum {minimum}"));
        }
    }
    if let Some(maximum) = spec.maximum {
        if number > maximum {
            return Err(format!("{number} is above the maximum {maximum}"));
        }
    }
    Ok(())
}

/// Reads an environment variable's text as the setting's kind.
///
/// One parser serves every setting: a flag accepts `0`/`false`/`no`/`off` and
/// `1`/`true`/`yes`/`on`, a list splits on commas, and anything unrecognized is
/// an error rather than a silent default.
///
/// # Errors
///
/// Returns why the text cannot be the setting's kind.
pub fn parse_environment_value(spec: &SettingSpec, raw: &str) -> Result<SettingsValue, String> {
    let text = raw.trim();
    match spec.kind() {
        SettingKind::Bool => match text.to_ascii_lowercase().as_str() {
            "0" | "false" | "no" | "off" => Ok(SettingsValue::Bool(false)),
            "1" | "true" | "yes" | "on" => Ok(SettingsValue::Bool(true)),
            _ => Err(format!(
                "`{text}` is not a flag (0/false/no/off or 1/true/yes/on)"
            )),
        },
        SettingKind::Integer => text
            .parse::<i64>()
            .map(SettingsValue::Integer)
            .map_err(|_| format!("`{text}` is not a whole number")),
        SettingKind::Float => text
            .parse::<f64>()
            .map(SettingsValue::Float)
            .map_err(|_| format!("`{text}` is not a number")),
        SettingKind::Text => Ok(SettingsValue::Text(text.to_string())),
        SettingKind::List => Ok(SettingsValue::List(
            text.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| SettingsValue::Text(item.to_string()))
                .collect(),
        )),
        SettingKind::Table => Err(
            "a table cannot be set from one variable; override one of its keys instead".to_string(),
        ),
    }
}

/// Reports the paths a document sets that its entry does not declare.
fn report_unknown_paths(
    entry: &str,
    settings: &SettingsValue,
    declared: Option<&ModuleSettingsSchema>,
    resolved: &mut ResolvedSettings,
    schemas: &BTreeMap<&str, &ModuleSettingsSchema>,
) {
    let Some(settings) = settings.as_table() else {
        resolved.report(SettingsDiagnostic::new(
            entry,
            "an entry is a mapping of settings, not a value",
        ));
        return;
    };
    let Some(schema) = declared else {
        // A module may declare no runtime settings at all. `enabled` is the
        // one key every entry accepts; anything else is a setting nobody
        // declared.
        let undeclared: Vec<&String> = settings
            .keys()
            .filter(|key| key.as_str() != ENABLED_KEY)
            .collect();
        if !undeclared.is_empty() {
            resolved.report(SettingsDiagnostic::new(
                entry,
                "this entry declares no runtime settings",
            ));
        }
        return;
    };
    for (key, value) in settings {
        if key == ENABLED_KEY {
            continue;
        }
        match value {
            SettingsValue::Table(_) if schema.spec(key).is_none() => {
                // A group: its keys are the declared settings.
                let table = value.as_table().expect("matched a table");
                for child in table.keys() {
                    let path = format!("{key}.{child}");
                    if schema.spec(&path).is_none() {
                        let full = format!("{entry}.{path}");
                        let mut diagnostic =
                            SettingsDiagnostic::new(full.clone(), "no such setting");
                        if let Some(suggestion) = closest_setting(&full, schemas) {
                            diagnostic = diagnostic.with_suggestion(suggestion);
                        }
                        resolved.report(diagnostic);
                    }
                }
            }
            _ => {
                if schema.spec(key).is_none() {
                    let full = format!("{entry}.{key}");
                    let mut diagnostic = SettingsDiagnostic::new(full.clone(), "no such setting");
                    if let Some(suggestion) = closest_setting(&full, schemas) {
                        diagnostic = diagnostic.with_suggestion(suggestion);
                    }
                    resolved.report(diagnostic);
                }
            }
        }
    }
}

/// Splits a `PILL__` variable's name into its entry and its path inside it.
fn split_environment_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix(ENVIRONMENT_PREFIX)?;
    let segments: Vec<&str> = rest.split(ENVIRONMENT_SEPARATOR).collect();
    match segments.as_slice() {
        [entry, key] => Some((entry.to_ascii_lowercase(), key.to_ascii_lowercase())),
        [entry, group, key] => Some((
            entry.to_ascii_lowercase(),
            format!(
                "{}.{}",
                group.to_ascii_lowercase(),
                key.to_ascii_lowercase()
            ),
        )),
        _ => None,
    }
}

/// The declared full path closest to `path`, when one is close enough to be a
/// typo.
fn closest_setting(path: &str, schemas: &BTreeMap<&str, &ModuleSettingsSchema>) -> Option<String> {
    let candidates = schemas.values().flat_map(|schema| {
        schema
            .settings
            .iter()
            .map(move |spec| spec.full_path(schema.module))
    });
    closest(path, candidates)
}

/// The candidate closest to `candidate`, when one is within the edit distance
/// a typo produces.
fn closest(candidate: &str, candidates: impl Iterator<Item = String>) -> Option<String> {
    let limit = (candidate.len() / 3).max(2);
    let mut best: Option<(usize, String)> = None;
    for name in candidates {
        let distance = edit_distance(candidate, &name);
        if distance > limit {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(best_distance, _)| distance < *best_distance)
        {
            best = Some((distance, name));
        }
    }
    best.map(|(_, name)| name)
}

/// The number of single-character insertions, deletions and substitutions that
/// turn `left` into `right`.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_character) in left.iter().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_character) in right.iter().enumerate() {
            let cost = usize::from(left_character != right_character);
            current[right_index + 1] = (previous[right_index] + cost)
                .min(previous[right_index + 1] + 1)
                .min(current[right_index] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: ModuleSettingsSchema = ModuleSettingsSchema::new(
        "pill_audio",
        &[
            SettingSpec::enabled(),
            SettingSpec::new(
                "audio",
                "ambient_sinks",
                SettingDefault::Integer(16),
                "How many ambient inputs the device opens.",
            )
            .with_range(1.0, 64.0),
            SettingSpec::new(
                "audio",
                "ear_separation",
                SettingDefault::Float(1.0),
                "How far apart the two ears are, in metres.",
            )
            .with_timing(ApplyTiming::Live),
            SettingSpec::new(
                "audio",
                "tags",
                SettingDefault::List(&["music"]),
                "The groups the mixer plays.",
            ),
            SettingSpec::new(
                "audio",
                "gains",
                SettingDefault::EmptyTable,
                "A gain per group.",
            ),
        ],
    );

    /// A resolution over the schema above, with the documents and environment
    /// a test supplies.
    fn resolve_with(
        project: Option<&SettingsDocument>,
        local: Option<&SettingsDocument>,
        environment: &SettingsEnvironment,
        strict: bool,
    ) -> Result<ResolvedSettings, SettingsErrors> {
        let schemas = [SCHEMA];
        resolve(SettingsResolutionInput {
            schemas: &schemas,
            loaded_entries: &["pill_audio"],
            disabled_entries: &[],
            deferred_entries: &[],
            project,
            local,
            environment,
            strict,
        })
    }

    /// A document from YAML text, failing the test on bad YAML.
    fn document(text: &str) -> SettingsDocument {
        SettingsDocument::from_yaml_text(text).expect("the document reads")
    }

    #[test]
    fn the_order_is_default_then_project_then_local_then_environment() {
        let project = document(
            "modules:\n  pill_audio:\n    enabled: true\n    audio:\n      ambient_sinks: 2\n",
        );
        let local = document("modules:\n  pill_audio:\n    audio:\n      ambient_sinks: 3\n");
        let environment = SettingsEnvironment::from_pairs([(
            "PILL__PILL_AUDIO__AUDIO__AMBIENT_SINKS".to_string(),
            "4".to_string(),
        )]);

        let resolved =
            resolve_with(Some(&project), Some(&local), &environment, false).expect("not strict");
        assert_eq!(
            resolved.value("pill_audio", "audio.ambient_sinks"),
            Some(&ResolvedValue {
                value: SettingsValue::Integer(4),
                source: SettingSource::Environment,
            })
        );

        let without = SettingsEnvironment::empty();
        let resolved =
            resolve_with(Some(&project), Some(&local), &without, false).expect("not strict");
        assert_eq!(
            resolved.value("pill_audio", "audio.ambient_sinks"),
            Some(&ResolvedValue {
                value: SettingsValue::Integer(3),
                source: SettingSource::Local,
            })
        );

        let resolved = resolve_with(Some(&project), None, &without, false).expect("not strict");
        assert_eq!(
            resolved.value("pill_audio", "audio.ambient_sinks"),
            Some(&ResolvedValue {
                value: SettingsValue::Integer(2),
                source: SettingSource::Project,
            })
        );

        let resolved = resolve_with(None, None, &without, false).expect("not strict");
        assert_eq!(
            resolved.value("pill_audio", "audio.ambient_sinks"),
            Some(&ResolvedValue {
                value: SettingsValue::Integer(16),
                source: SettingSource::Default,
            })
        );
    }

    #[test]
    fn a_module_nobody_lists_is_off() {
        let resolved =
            resolve_with(None, None, &SettingsEnvironment::empty(), false).expect("not strict");
        assert!(!resolved.is_enabled("pill_audio"));
    }

    #[test]
    fn a_listed_module_reads_enabled() {
        let project = document("modules:\n  pill_audio:\n    enabled: true\n");
        let resolved = resolve_with(Some(&project), None, &SettingsEnvironment::empty(), false)
            .expect("not strict");
        assert!(resolved.is_enabled("pill_audio"));
    }

    #[test]
    fn an_empty_module_list_lists_no_module() {
        let project = document("name: \"Empty\"\nmodules: []\n");
        assert!(project.entries.is_empty());
        let resolved = resolve_with(Some(&project), None, &SettingsEnvironment::empty(), false)
            .expect("not strict");
        assert!(!resolved.is_enabled("pill_audio"));
    }

    #[test]
    fn a_module_list_with_a_name_is_refused() {
        let error = SettingsDocument::from_yaml_text("modules:\n  - \"pill_audio\"\n")
            .expect_err("a bare list cannot carry a module's settings");
        assert!(error.to_string().contains("not a mapping"));
    }

    #[test]
    fn an_unknown_key_suggests_the_closest_declared_name() {
        let project = document("modules:\n  pill_audio:\n    audio:\n      ambient_sink: 2\n");
        let resolved = resolve_with(Some(&project), None, &SettingsEnvironment::empty(), false)
            .expect("not strict");
        let diagnostic = resolved
            .diagnostics()
            .first()
            .expect("the typo is reported");
        assert_eq!(diagnostic.path, "pill_audio.audio.ambient_sink");
        assert_eq!(
            diagnostic.suggestion.as_deref(),
            Some("pill_audio.audio.ambient_sinks")
        );
        assert!(diagnostic.to_string().contains("did you mean"));
    }

    #[test]
    fn strict_mode_refuses_what_warn_mode_reports() {
        let project = document("modules:\n  pill_audio:\n    audio:\n      ambient_sink: 2\n");
        let environment = SettingsEnvironment::empty();
        assert!(resolve_with(Some(&project), None, &environment, false).is_ok());
        let error = resolve_with(Some(&project), None, &environment, true)
            .expect_err("strict refuses the typo");
        assert_eq!(error.diagnostics().len(), 1);
    }

    #[test]
    fn a_wrong_type_keeps_the_default() {
        let project = document("modules:\n  pill_audio:\n    audio:\n      ambient_sinks: many\n");
        let resolved = resolve_with(Some(&project), None, &SettingsEnvironment::empty(), false)
            .expect("not strict");
        assert_eq!(
            resolved.value("pill_audio", "audio.ambient_sinks"),
            Some(&ResolvedValue {
                value: SettingsValue::Integer(16),
                source: SettingSource::Default,
            })
        );
        assert!(resolved.diagnostics()[0].message.contains("text"));
    }

    #[test]
    fn an_out_of_range_value_keeps_the_default() {
        let project = document("modules:\n  pill_audio:\n    audio:\n      ambient_sinks: 0\n");
        let resolved = resolve_with(Some(&project), None, &SettingsEnvironment::empty(), false)
            .expect("not strict");
        assert_eq!(
            resolved.integer("pill_audio", "audio.ambient_sinks", -1),
            16
        );
        assert!(resolved.diagnostics()[0].message.contains("minimum"));
    }

    #[test]
    fn a_disabled_module_is_skipped_and_reported_once() {
        let project = document("modules:\n  pill_audio:\n    audio:\n      ambient_sink: 2\n");
        let schemas = [SCHEMA];
        let resolved = resolve(SettingsResolutionInput {
            schemas: &schemas,
            loaded_entries: &[],
            disabled_entries: &["pill_audio"],
            deferred_entries: &[],
            project: Some(&project),
            local: None,
            environment: &SettingsEnvironment::empty(),
            strict: false,
        })
        .expect("not strict");

        assert_eq!(resolved.diagnostics().len(), 1);
        assert_eq!(resolved.diagnostics()[0].path, "pill_audio");
        assert!(resolved
            .value("pill_audio", "audio.ambient_sinks")
            .is_none());
    }

    #[test]
    fn an_unknown_environment_variable_is_reported() {
        let environment = SettingsEnvironment::from_pairs([(
            "PILL__PILL_AUDIO__AUDIO__AMBIENT_SINK".to_string(),
            "2".to_string(),
        )]);
        let resolved = resolve_with(None, None, &environment, false).expect("not strict");
        let diagnostic = resolved.diagnostics().first().expect("reported");
        assert_eq!(diagnostic.path, "pill_audio.audio.ambient_sink");
        assert_eq!(
            diagnostic.suggestion.as_deref(),
            Some("pill_audio.audio.ambient_sinks")
        );
    }

    #[test]
    fn every_flag_spelling_reads_the_same_way() {
        let spec = SettingSpec::enabled();
        for off in ["0", "false", "FALSE", "No", "off"] {
            assert_eq!(
                parse_environment_value(&spec, off),
                Ok(SettingsValue::Bool(false))
            );
        }
        for on in ["1", "true", "YES", "on"] {
            assert_eq!(
                parse_environment_value(&spec, on),
                Ok(SettingsValue::Bool(true))
            );
        }
        assert!(parse_environment_value(&spec, "maybe").is_err());
    }

    #[test]
    fn a_declared_table_accepts_any_child_key() {
        let project =
            document("modules:\n  pill_audio:\n    audio:\n      gains:\n        music: 0.5\n");
        let resolved = resolve_with(Some(&project), None, &SettingsEnvironment::empty(), false)
            .expect("not strict");
        assert!(resolved.diagnostics().is_empty());
        let gains = resolved
            .table("pill_audio", "audio.gains")
            .expect("a table");
        assert_eq!(gains.get("music"), Some(&SettingsValue::Float(0.5)));
    }

    #[test]
    fn a_module_that_declares_no_settings_is_named() {
        let project = document("modules:\n  pill_spline:\n    anything: 1\n");
        let schemas = [SCHEMA];
        let resolved = resolve(SettingsResolutionInput {
            schemas: &schemas,
            loaded_entries: &["pill_audio", "pill_spline"],
            disabled_entries: &[],
            deferred_entries: &[],
            project: Some(&project),
            local: None,
            environment: &SettingsEnvironment::empty(),
            strict: false,
        })
        .expect("not strict");
        assert_eq!(resolved.diagnostics().len(), 1);
        assert!(resolved.diagnostics()[0]
            .message
            .contains("declares no runtime settings"));
    }

    #[test]
    fn a_flat_top_level_setting_is_reported_by_name() {
        let project = document("renderer: none\n");
        let resolved = resolve_with(Some(&project), None, &SettingsEnvironment::empty(), false)
            .expect("not strict");
        assert_eq!(resolved.diagnostics()[0].path, "renderer");
        assert!(resolved.diagnostics()[0].message.contains("not a setting"));
    }

    #[test]
    fn settings_values_round_trip_through_yaml() {
        let values = [
            SettingsValue::Bool(true),
            SettingsValue::Integer(-4),
            SettingsValue::Float(0.5),
            SettingsValue::Text("hello".to_string()),
            SettingsValue::List(vec![SettingsValue::Text("a".to_string())]),
        ];
        for value in values {
            let yaml = value.to_yaml();
            assert_eq!(SettingsValue::from_yaml(yaml), Ok(value));
        }
    }

    #[test]
    fn an_edit_distance_finds_a_one_character_typo() {
        assert_eq!(edit_distance("ambient_sink", "ambient_sinks"), 1);
        assert_eq!(edit_distance("same", "same"), 0);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn a_document_keeps_module_order_and_tells_modules_from_entries() {
        let document = document(
            "modules:\n  pill_rendering:\n    enabled: true\n  pill_renderer:\n    enabled: true\nhost:\n  hot_reloading: false\n",
        );
        let modules: Vec<&str> = document.module_names().collect();
        assert_eq!(modules, ["pill_rendering", "pill_renderer"]);
        assert!(document.is_module("pill_renderer"));
        assert!(!document.is_module("host"));
    }
}
