//! The project's settings: the declarations the host owns, the file pair it
//! reads, and the resolution every consumer asks.
//!
//! # Responsibilities
//!
//! - Declares the `host`, `engine` and `editor` entries: the switches the
//!   development host obeys, the values a shipped build starts with, and the
//!   editor's project layer.
//! - Reads `project_settings.yaml` and `project_settings.local.yaml`, in that
//!   order, into the engine's sparse settings documents.
//! - Resolves them against the declarations and the environment, logs what
//!   came from where, and reports what cannot be used - as a warning, or as a
//!   configuration error under the strict switch.
//! - Names the modules the project enables, in the order the file lists them,
//!   which is the order they load in.
//!
//! # Design
//!
//! One resolver serves the host and every module: this module only supplies
//! the declarations it owns and the two documents; the resolution order,
//! provenance and diagnostics live in `pill_engine`'s settings model. A
//! module's own runtime settings are declared in the module, so the host does
//! not know their schema - it validates the entries that are its own and lets
//! a module's registration supply the rest.

// Standard library
use std::collections::BTreeMap;
use std::path::Path;

// External crates
use serde::Deserialize;
use tracing::{info, warn};

// Current crate
use pill_core::error::ConfigError;
use pill_core::telemetry::telemetry_target;
use pill_engine::{
    parse_environment_value, resolve, ApplyTiming, ModuleSettingsSchema, ResolvedSettings,
    SettingDefault, SettingSpec, SettingsDocument, SettingsEnvironment, SettingsResolutionInput,
};

// =============================================================================
// Constants
// =============================================================================

/// The project's settings file, in the project root.
pub const PROJECT_SETTINGS_FILE: &str = "project_settings.yaml";

/// The local override beside it: the same shape, read over the project file,
/// and never committed.
pub const PROJECT_SETTINGS_LOCAL_FILE: &str = "project_settings.local.yaml";

// =============================================================================
// SettingsStamp
// =============================================================================

/// When a project's two settings files last moved, so an edit is noticed.
///
/// The shape a file had is its length and its modification time: a settings
/// file is a few hundred bytes of text, so a same-length rewrite by an editor
/// that preserves the timestamp is not a case worth a content hash on every
/// frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsStamp {
    /// The project file's shape, or `None` when it does not exist.
    project: Option<FileShape>,
    /// The local override's shape, or `None` when it does not exist.
    local: Option<FileShape>,
}

/// One file's length and modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileShape {
    /// The file's length in bytes.
    length: u64,
    /// The file's modification time, in nanoseconds since the Unix epoch.
    modified: u128,
}

impl FileShape {
    /// The shape of `path`, or `None` when it cannot be read.
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            length: metadata.len(),
            modified,
        })
    }
}

impl SettingsStamp {
    /// The current shape of a project's two settings files.
    pub fn of(project_directory: &Path) -> Self {
        Self {
            project: FileShape::of(&project_directory.join(PROJECT_SETTINGS_FILE)),
            local: FileShape::of(&project_directory.join(PROJECT_SETTINGS_LOCAL_FILE)),
        }
    }

    /// The project's settings, with their new stamp, once either file has
    /// moved since this stamp was taken.
    ///
    /// Returns `None` when the files are as they were, and also when the new
    /// text cannot be read or parsed: the caller keeps running the settings it
    /// has, and the next edit tries again.
    pub fn reread(&self, project_directory: &Path) -> Option<(Self, ProjectSettings)> {
        let stamp = Self::of(project_directory);
        if stamp == *self {
            return None;
        }
        let settings = ProjectSettings::read(project_directory).ok()?;
        Some((stamp, settings))
    }
}

/// The host's own settings: `host: {}` in the project's file.
pub(crate) const HOST_SETTINGS: ModuleSettingsSchema = ModuleSettingsSchema::new(
    "host",
    &[
        SettingSpec::new(
            "",
            "renderer",
            SettingDefault::Text(""),
            "The renderer this run draws with, or `none`. Empty follows the enabled modules.",
        ),
        SettingSpec::new(
            "",
            "hot_reloading",
            SettingDefault::Bool(true),
            "Whether an edit to the project or an extension is noticed.",
        )
        .with_timing(ApplyTiming::Live),
        SettingSpec::new(
            "",
            "function_hot_patching",
            SettingDefault::Bool(true),
            "Whether a watched edit may be compiled into the running module.",
        )
        .with_timing(ApplyTiming::Live),
        SettingSpec::new(
            "",
            "assets_hot_reloading",
            SettingDefault::Bool(true),
            "Whether the asset and shader trees are watched.",
        )
        .with_timing(ApplyTiming::Live),
        SettingSpec::new(
            "",
            "strict_settings",
            SettingDefault::Bool(false),
            "Whether a settings problem stops the run instead of warning.",
        ),
        SettingSpec::new(
            "assets",
            "scan_on_start",
            SettingDefault::Bool(false),
            "Whether every source asset is imported right after the project starts.",
        ),
        SettingSpec::new(
            "logging",
            "level",
            SettingDefault::Text(""),
            "One log level for every target.",
        ),
        SettingSpec::new(
            "logging",
            "timestamp",
            SettingDefault::Text(""),
            "How each log line writes its local time: `date_time` or `time`.",
        ),
        SettingSpec::new(
            "logging",
            "source_location",
            SettingDefault::Bool(false),
            "Whether each log line ends with its source file and line.",
        ),
        SettingSpec::new(
            "logging",
            "targets",
            SettingDefault::EmptyTable,
            "A log level per target.",
        ),
    ],
);

/// The engine's own settings: `engine: {}` in the project's file.
pub(crate) const ENGINE_SETTINGS: ModuleSettingsSchema = ModuleSettingsSchema::new(
    "engine",
    &[
        SettingSpec::new(
            "",
            "debug_draw",
            SettingDefault::Bool(false),
            "Whether a shipping or web build starts with debug drawing on.",
        ),
        SettingSpec::new(
            "",
            "startup_scene",
            SettingDefault::Text(""),
            "The `.scene` the project starts with, as a path below `res`.",
        ),
    ],
);

/// The editor's project layer: `editor: {}` in the project's file.
pub(crate) const EDITOR_SETTINGS: ModuleSettingsSchema = ModuleSettingsSchema::new(
    "editor",
    &[
        SettingSpec::new(
            "",
            "theme",
            SettingDefault::Text(""),
            "The project's editor theme, as a path below `res`.",
        ),
        SettingSpec::new(
            "",
            "focus_game_on_play",
            SettingDefault::Bool(true),
            "Whether Play brings the Game tab forward.",
        ),
    ],
);

// =============================================================================
// EditorSettings
// =============================================================================

/// The editor's project layer, as `editor:` declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorSettings {
    /// The `theme:` value: a path below the project's `res`.
    pub editor_theme: Option<String>,
    /// Whether Play brings the Game tab forward.
    pub focus_game_on_play: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            editor_theme: None,
            focus_game_on_play: true,
        }
    }
}

// =============================================================================
// ProjectMetadata
// =============================================================================

/// The project's identity, which is not a setting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct ProjectMetadataFile {
    /// The project's display name, used as the window title.
    name: Option<String>,
    /// The artifact file base.
    build_binary_name: Option<String>,
}

// =============================================================================
// ProjectSettings
// =============================================================================

/// One project's settings, read and resolved.
#[derive(Debug, Clone)]
pub struct ProjectSettings {
    /// The resolved values of the entries the host declares.
    resolved: ResolvedSettings,
    /// The project's display name.
    pub(crate) name: Option<String>,
    /// The artifact file base.
    pub(crate) build_binary_name: Option<String>,
    /// The modules the project turns on, in the order the file lists them.
    pub enabled_modules: Vec<String>,
    /// The modules the project lists but turns off. Their runtime settings
    /// are not checked, which the startup log reports once.
    pub disabled_modules: Vec<String>,
    /// The build options each enabled module's entry turns on, from
    /// `modules.<module>.build`.
    build_options: BTreeMap<String, BTreeMap<String, bool>>,
    /// `project_settings.yaml`, as read: the editor's tab shows which values
    /// come from here.
    project_document: SettingsDocument,
    /// `project_settings.local.yaml`, as read, or an empty document.
    local_document: SettingsDocument,
}

impl Default for ProjectSettings {
    /// A project that sets nothing: every declared default, no modules.
    ///
    /// An embedder that wraps a [`ProjectModuleConfig`](crate::config::ProjectModuleConfig)
    /// with no settings file of its own starts here.
    fn default() -> Self {
        let schemas = [HOST_SETTINGS, ENGINE_SETTINGS, EDITOR_SETTINGS];
        let resolved = resolve(SettingsResolutionInput {
            schemas: &schemas,
            loaded_entries: &["engine", "host", "editor"],
            disabled_entries: &[],
            deferred_entries: &[],
            project: None,
            local: None,
            environment: &SettingsEnvironment::empty(),
            strict: false,
        })
        .unwrap_or_default();
        Self {
            resolved,
            name: None,
            build_binary_name: None,
            enabled_modules: Vec::new(),
            disabled_modules: Vec::new(),
            build_options: BTreeMap::new(),
            project_document: SettingsDocument::empty(),
            local_document: SettingsDocument::empty(),
        }
    }
}

/// Resolves a project's settings against the host's declarations plus every
/// module schema the loaded artifacts registered.
///
/// A run resolves twice: once at startup, when no artifact has registered
/// anything yet, and again after the modules load, when their entries are
/// known. Only the second resolution can validate `modules.<module>.*`.
///
/// # Errors
///
/// Returns [`ConfigError::ProjectSettingsFileMissing`] when the project has no
/// `project_settings.yaml`, a read or parse error naming the file, and
/// [`ConfigError::InvalidSettings`] when a strict run finds a setting it cannot
/// use.
pub(crate) fn resolve_with_modules(
    project_directory: &Path,
    module_schemas: &[ModuleSettingsSchema],
) -> Result<ResolvedSettings, ConfigError> {
    let (project, local) = read_documents(project_directory)?;
    let environment = SettingsEnvironment::from_process();
    let (enabled_modules, disabled_modules) = declared_modules(&project, &local, &environment);
    let strict = declared_boolean(
        "host",
        "strict_settings",
        false,
        &project,
        &local,
        &environment,
    );
    let mut loaded_entries: Vec<&str> = enabled_modules.iter().map(String::as_str).collect();
    loaded_entries.extend(["engine", "host", "editor"]);
    let disabled_entries: Vec<&str> = disabled_modules.iter().map(String::as_str).collect();
    let mut schemas: Vec<ModuleSettingsSchema> =
        vec![HOST_SETTINGS, ENGINE_SETTINGS, EDITOR_SETTINGS];
    schemas.extend(module_schemas.iter().copied());
    resolve(SettingsResolutionInput {
        schemas: &schemas,
        loaded_entries: &loaded_entries,
        disabled_entries: &disabled_entries,
        deferred_entries: &[],
        project: Some(&project),
        local: Some(&local),
        environment: &environment,
        strict,
    })
    .map_err(|errors| ConfigError::InvalidSettings {
        path: project_directory
            .join(PROJECT_SETTINGS_FILE)
            .display()
            .to_string(),
        details: errors.to_string(),
    })
}

/// The settings entries the host declares: `host`, `engine` and `editor`.
///
/// A module declares its own entry in its crate, so a reader that wants every
/// schema - the editor's Project Settings tab, for one - takes these three and
/// adds what the loaded modules registered.
pub fn declared_settings_schemas() -> [ModuleSettingsSchema; 3] {
    [HOST_SETTINGS, ENGINE_SETTINGS, EDITOR_SETTINGS]
}

/// The project's two settings documents: its own file, and the local override
/// when it exists.
fn read_documents(
    project_directory: &Path,
) -> Result<(SettingsDocument, SettingsDocument), ConfigError> {
    let project_path = project_directory.join(PROJECT_SETTINGS_FILE);
    if !project_path.is_file() {
        return Err(ConfigError::ProjectSettingsFileMissing {
            path: project_path.display().to_string(),
        });
    }
    let project_text = read_settings_text(&project_path)?;
    let project = parse_document(&project_path, &project_text)?;
    let local_path = project_directory.join(PROJECT_SETTINGS_LOCAL_FILE);
    let local = if local_path.is_file() {
        let text = read_settings_text(&local_path)?;
        parse_document(&local_path, &text)?
    } else {
        SettingsDocument::empty()
    };
    Ok((project, local))
}

impl ProjectSettings {
    /// Reads and resolves one project's settings.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ProjectSettingsFileMissing`] when the project has
    /// no `project_settings.yaml`, a read or parse error naming the file it
    /// came from, and [`ConfigError::InvalidSettings`] when a strict run finds
    /// a setting it cannot use.
    pub(crate) fn read(project_directory: &Path) -> Result<Self, ConfigError> {
        // Step 1 and 2: the project's file, which every project must have, and
        // the local override, which is optional and never committed.
        let (project, local) = read_documents(project_directory)?;
        let project_path = project_directory.join(PROJECT_SETTINGS_FILE);
        let project_text = read_settings_text(&project_path)?;
        let metadata: ProjectMetadataFile = serde_yaml::from_str(&project_text).unwrap_or_default();

        // Step 3: which modules this project turns on, in its own order.
        let environment = SettingsEnvironment::from_process();
        let (enabled_modules, disabled_modules) = declared_modules(&project, &local, &environment);

        // Step 4: resolve the entries the host declares, strictly when asked.
        let strict = declared_boolean(
            "host",
            "strict_settings",
            false,
            &project,
            &local,
            &environment,
        );
        // The settings entries the host declares are here; a module's entry is
        // not, because a module declares its own settings in its crate and
        // registers the schema when it loads. Those entries are deferred to the
        // second resolution, which runs after the modules have loaded, so this
        // one neither checks nor reports them.
        let loaded_entries: Vec<&str> = vec!["engine", "host", "editor"];
        let deferred_entries: Vec<&str> = enabled_modules.iter().map(String::as_str).collect();
        let disabled_entries: Vec<&str> = disabled_modules.iter().map(String::as_str).collect();
        let schemas = [HOST_SETTINGS, ENGINE_SETTINGS, EDITOR_SETTINGS];
        let resolved = resolve(SettingsResolutionInput {
            schemas: &schemas,
            loaded_entries: &loaded_entries,
            disabled_entries: &disabled_entries,
            deferred_entries: &deferred_entries,
            project: Some(&project),
            local: Some(&local),
            environment: &environment,
            strict,
        })
        .map_err(|errors| ConfigError::InvalidSettings {
            path: project_path.display().to_string(),
            details: errors.to_string(),
        })?;

        // Step 5: say what was used, and what was ignored.
        for (path, value) in resolved.non_default_values() {
            info!(
                target: telemetry_target::ENGINE,
                setting = path.as_str(),
                value = value.value.describe().as_str(),
                source = value.source.as_str(),
                "a setting overrides its default"
            );
        }
        for module in &disabled_modules {
            info!(
                target: telemetry_target::ENGINE,
                module = module.as_str(),
                "the project disables this module; its runtime settings are not checked"
            );
        }
        for diagnostic in resolved.diagnostics() {
            warn!(
                target: telemetry_target::ENGINE,
                settings = %diagnostic,
                "a project setting is not used"
            );
        }

        Ok(Self {
            name: metadata.name,
            build_binary_name: metadata.build_binary_name,
            resolved,
            build_options: declared_build_options(&enabled_modules, &project, &local, &environment),
            enabled_modules,
            disabled_modules,
            project_document: project,
            local_document: local,
        })
    }

    /// `project_settings.yaml`, as this run read it.
    pub fn project_document(&self) -> &SettingsDocument {
        &self.project_document
    }

    /// `project_settings.local.yaml`, as this run read it; empty when the
    /// project has none.
    pub fn local_document(&self) -> &SettingsDocument {
        &self.local_document
    }

    /// The resolved values of the host, engine and editor entries, with where
    /// each came from.
    pub fn resolved(&self) -> &ResolvedSettings {
        &self.resolved
    }

    /// The editor's project layer.
    pub(crate) fn editor(&self) -> EditorSettings {
        let theme = self.resolved.text("editor", "theme", "").trim();
        EditorSettings {
            editor_theme: (!theme.is_empty()).then(|| theme.to_string()),
            focus_game_on_play: self.resolved.boolean("editor", "focus_game_on_play", true),
        }
    }

    /// The renderer the project names outright, or `None` when the choice
    /// follows the enabled modules.
    pub(crate) fn requested_renderer(&self) -> Option<&str> {
        let name = self.resolved.text("host", "renderer", "").trim();
        (!name.is_empty()).then_some(name)
    }

    /// The build options a module's entry turns on, each being a cargo feature
    /// its manifest declares, and whether it is on.
    pub(crate) fn build_options(&self, module: &str) -> BTreeMap<String, bool> {
        self.build_options.get(module).cloned().unwrap_or_default()
    }

    /// The `logging:` section, as the file spells it.
    pub(crate) fn logging(&self) -> LoggingValues {
        let targets = self
            .resolved
            .table("host", "logging.targets")
            .map(|table| {
                table
                    .iter()
                    .filter_map(|(target, level)| {
                        level
                            .as_text()
                            .map(|level| (target.clone(), level.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        LoggingValues {
            level: self.text_or_none("host", "logging.level"),
            timestamp: self.text_or_none("host", "logging.timestamp"),
            source_location: self
                .resolved
                .boolean("host", "logging.source_location", false),
            targets,
        }
    }

    /// The switches this run obeys.
    pub(crate) fn hot_reloading(&self) -> bool {
        self.resolved.boolean("host", "hot_reloading", true)
    }

    /// Whether a watched edit may be compiled into the running module.
    pub(crate) fn function_hot_patching(&self) -> bool {
        self.resolved.boolean("host", "function_hot_patching", true)
    }

    /// Whether the asset and shader trees are watched.
    pub(crate) fn assets_hot_reloading(&self) -> bool {
        self.resolved.boolean("host", "assets_hot_reloading", true)
    }

    /// Whether the host imports every source asset after the project starts.
    pub(crate) fn scan_assets_on_start(&self) -> bool {
        self.resolved.boolean("host", "assets.scan_on_start", false)
    }

    /// Whether a shipping or web build starts with debug drawing on.
    pub(crate) fn shipping_debug_draw(&self) -> bool {
        self.resolved.boolean("engine", "debug_draw", false)
    }

    /// The startup scene, as written.
    pub(crate) fn startup_scene(&self) -> Option<&str> {
        let scene = self.resolved.text("engine", "startup_scene", "").trim();
        (!scene.is_empty()).then_some(scene)
    }

    fn text_or_none(&self, entry: &str, path: &str) -> Option<String> {
        let text = self.resolved.text(entry, path, "").trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

// =============================================================================
// LoggingValues
// =============================================================================

/// The `logging:` section, read as text before it is validated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LoggingValues {
    /// One level for every target.
    pub(crate) level: Option<String>,
    /// How each line writes its local time.
    pub(crate) timestamp: Option<String>,
    /// Whether each line ends with its source location.
    pub(crate) source_location: bool,
    /// A level per target.
    pub(crate) targets: BTreeMap<String, String>,
}

impl LoggingValues {
    /// Checks every level and target, and turns them into the runtime's form.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidLoggingSettings`] naming the first level
    /// or target that does not read.
    pub(crate) fn validate(
        &self,
        path: &Path,
    ) -> Result<pill_player::LoggingSettings, ConfigError> {
        let invalid_logging_settings = |details: String| ConfigError::InvalidLoggingSettings {
            path: path.display().to_string(),
            details,
        };
        let target_levels: Vec<(&str, &str)> = self
            .targets
            .iter()
            .map(|(target, level)| (target.as_str(), level.as_str()))
            .collect();
        // One conversion serves the development host and a shipping bundle
        // alike, so the same text rules refuse a bad level or target in both.
        pill_player::LoggingSettings::from_text(
            self.level.as_deref(),
            self.timestamp.as_deref(),
            Some(self.source_location).filter(|enabled| *enabled),
            &target_levels,
        )
        .map_err(invalid_logging_settings)
    }
}

// =============================================================================
// Reading
// =============================================================================

/// Reads the editor's project layer from a project's files.
///
/// A project with no settings file has no project layer; that is not an error,
/// because the editor opens whatever project it is pointed at.
///
/// # Errors
///
/// Returns a message naming the file when it exists but cannot be read or is
/// not valid YAML.
pub fn read_editor_settings(project_directory: &Path) -> Result<EditorSettings, String> {
    let project = read_optional_document(project_directory, PROJECT_SETTINGS_FILE)?;
    let local = read_optional_document(project_directory, PROJECT_SETTINGS_LOCAL_FILE)?;
    let environment = SettingsEnvironment::from_process();
    let schemas = [EDITOR_SETTINGS];
    let resolved = resolve(SettingsResolutionInput {
        schemas: &schemas,
        loaded_entries: &["editor"],
        disabled_entries: &[],
        deferred_entries: &[],
        project: project.as_ref(),
        local: local.as_ref(),
        environment: &environment,
        strict: false,
    })
    .map_err(|errors| errors.to_string())?;
    let theme = resolved.text("editor", "theme", "").trim().to_string();
    Ok(EditorSettings {
        editor_theme: (!theme.is_empty()).then_some(theme),
        focus_game_on_play: resolved.boolean("editor", "focus_game_on_play", true),
    })
}

/// Reads one settings file into a document, when it exists.
fn read_optional_document(
    project_directory: &Path,
    name: &str,
) -> Result<Option<SettingsDocument>, String> {
    let path = project_directory.join(name);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("`{}` could not be read: {error}", path.display()))?;
    SettingsDocument::from_yaml_text(&text)
        .map(Some)
        .map_err(|error| format!("`{}`: {error}", path.display()))
}

/// Reads a settings file's text.
fn read_settings_text(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|source| ConfigError::ProjectSettingsFileReadFailed {
        path: path.display().to_string(),
        source,
    })
}

/// Parses a settings file's text into a document.
fn parse_document(path: &Path, text: &str) -> Result<SettingsDocument, ConfigError> {
    SettingsDocument::from_yaml_text(text).map_err(|error| {
        ConfigError::ProjectSettingsFileParseFailed {
            path: path.display().to_string(),
            details: error.to_string(),
        }
    })
}

/// The modules a project turns on and off, in the order the files list them.
fn declared_modules(
    project: &SettingsDocument,
    local: &SettingsDocument,
    environment: &SettingsEnvironment,
) -> (Vec<String>, Vec<String>) {
    let mut names: Vec<String> = project.module_names().map(str::to_string).collect();
    for name in local.module_names() {
        if !names.iter().any(|listed| listed.as_str() == name) {
            names.push(name.to_string());
        }
    }
    let mut enabled = Vec::new();
    let mut disabled = Vec::new();
    for name in names {
        if declared_boolean(&name, "enabled", false, project, local, environment) {
            enabled.push(name);
        } else {
            disabled.push(name);
        }
    }
    (enabled, disabled)
}

/// One flag as the files and the environment set it, without a schema.
fn declared_boolean(
    entry: &str,
    path: &str,
    fallback: bool,
    project: &SettingsDocument,
    local: &SettingsDocument,
    environment: &SettingsEnvironment,
) -> bool {
    let variable = format!(
        "PILL__{}__{}",
        entry.to_ascii_uppercase(),
        path.replace('.', "__").to_ascii_uppercase()
    );
    let spec = SettingSpec::new(
        "",
        "flag",
        SettingDefault::Bool(fallback),
        "A flag read before its entry's schema is known.",
    );
    if let Some(raw) = environment.get(&variable) {
        if let Ok(value) = parse_environment_value(&spec, raw) {
            if let Some(flag) = value.as_bool() {
                return flag;
            }
        }
    }
    for document in [local, project] {
        if let Some(flag) = document
            .value_at(entry, path)
            .and_then(|value| value.as_bool())
        {
            return flag;
        }
    }
    fallback
}

/// The build options each enabled module's entry sets, as the files and the
/// environment spell them.
///
/// Only an option a file names can be overridden in the environment: the
/// capture has no way to list its own variables, and an option nobody asked for
/// keeps its declared default.
fn declared_build_options(
    enabled_modules: &[String],
    project: &SettingsDocument,
    local: &SettingsDocument,
    environment: &SettingsEnvironment,
) -> BTreeMap<String, BTreeMap<String, bool>> {
    let mut modules: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
    for module in enabled_modules {
        let mut options: BTreeMap<String, bool> = BTreeMap::new();
        for document in [project, local] {
            let Some(table) = document
                .value_at(module, "build")
                .and_then(|value| value.as_table())
            else {
                continue;
            };
            for (option, value) in table {
                if let Some(on) = value.as_bool() {
                    options.insert(option.clone(), on);
                }
            }
        }
        for (option, on) in options.iter_mut() {
            let variable = format!(
                "PILL__{}__BUILD__{}",
                module.to_ascii_uppercase(),
                option.replace('.', "__").to_ascii_uppercase()
            );
            if let Some(raw) = environment.get(&variable) {
                let copy = SettingSpec::new(
                    "",
                    "option",
                    SettingDefault::Bool(*on),
                    "A build option read before its entry's schema is known.",
                );
                if let Some(flag) = parse_environment_value(&copy, raw)
                    .ok()
                    .and_then(|value| value.as_bool())
                {
                    *on = flag;
                }
            }
        }
        modules.insert(module.clone(), options);
    }
    modules
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A project directory holding the given settings text.
    fn project_with(text: &str) -> crate::scratch::ScratchDirectory {
        let directory = crate::scratch::ScratchDirectory::new("project-settings");
        std::fs::write(directory.path().join(PROJECT_SETTINGS_FILE), text)
            .expect("the settings file writes");
        directory
    }

    #[test]
    fn a_project_that_lists_nothing_turns_every_module_off() {
        let directory = project_with("name: \"Probe\"\nbuild_binary_name: \"Probe\"\n");
        let settings = ProjectSettings::read(directory.path()).expect("the project reads");

        assert_eq!(settings.name.as_deref(), Some("Probe"));
        assert!(settings.enabled_modules.is_empty());
        assert!(settings.disabled_modules.is_empty());
    }

    #[test]
    fn a_module_entry_reads_enabled_and_keeps_document_order() {
        let directory = project_with(
            r#"name: "Probe"
build_binary_name: "Probe"
modules:
  pill_rendering:
    enabled: true
  pill_renderer:
    enabled: true
  pill_spline:
    enabled: false
"#,
        );
        let settings = ProjectSettings::read(directory.path()).expect("the project reads");

        assert_eq!(
            settings.enabled_modules,
            ["pill_rendering", "pill_renderer"]
        );
        assert_eq!(settings.disabled_modules, ["pill_spline"]);
    }

    #[test]
    fn the_local_file_overrides_the_project_file() {
        let directory = project_with(
            "name: \"Probe\"\nbuild_binary_name: \"Probe\"\nhost:\n  hot_reloading: true\n",
        );
        std::fs::write(
            directory.path().join(PROJECT_SETTINGS_LOCAL_FILE),
            "host:\n  hot_reloading: false\n",
        )
        .expect("the local file writes");
        let settings = ProjectSettings::read(directory.path()).expect("the project reads");

        assert!(!settings.hot_reloading());
    }

    #[test]
    fn an_unknown_key_warns_and_strict_mode_refuses_it() {
        let directory = project_with(
            "name: \"Probe\"\nbuild_binary_name: \"Probe\"\nhost:\n  hot_reload: true\n",
        );
        let settings = ProjectSettings::read(directory.path()).expect("warn mode reads");
        assert!(settings.hot_reloading(), "the default is kept");
        assert_eq!(settings.resolved().diagnostics().len(), 1);

        std::fs::write(
            directory.path().join(PROJECT_SETTINGS_LOCAL_FILE),
            "host:\n  strict_settings: true\n",
        )
        .expect("the strict switch writes");
        let error = ProjectSettings::read(directory.path()).expect_err("strict refuses it");
        assert!(matches!(error, ConfigError::InvalidSettings { .. }));
    }

    #[test]
    fn the_flat_shape_is_reported_by_key() {
        let directory =
            project_with("name: \"Probe\"\nbuild_binary_name: \"Probe\"\nrenderer: none\n");
        let settings = ProjectSettings::read(directory.path()).expect("warn mode reads");

        let diagnostics = settings.resolved().diagnostics();
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.path == "renderer"));
    }

    #[test]
    fn the_editor_entry_reads_its_two_values() {
        let directory = project_with(
            "name: \"Probe\"\nbuild_binary_name: \"Probe\"\neditor:\n  theme: \"editor/editor.theme\"\n  focus_game_on_play: false\n",
        );
        let settings = ProjectSettings::read(directory.path()).expect("the project reads");

        assert_eq!(
            settings.editor(),
            EditorSettings {
                editor_theme: Some("editor/editor.theme".to_string()),
                focus_game_on_play: false,
            }
        );
    }

    /// The stamp notices an edit to either file and nothing else.
    #[test]
    fn the_stamp_notices_an_edit_to_either_settings_file() {
        let directory = project_with("name: \"Probe\"\nbuild_binary_name: \"Probe\"\n");
        let stamp = SettingsStamp::of(directory.path());
        assert!(
            stamp.reread(directory.path()).is_none(),
            "an untouched project is the stamp it was"
        );

        std::fs::write(
            directory.path().join(PROJECT_SETTINGS_FILE),
            "name: \"Probe\"\nbuild_binary_name: \"Probe\"\nhost:\n  hot_reloading: false\n",
        )
        .expect("the project file writes");
        let (stamp, settings) = stamp.reread(directory.path()).expect("the edit is noticed");
        assert!(!settings.hot_reloading());

        std::fs::write(
            directory.path().join(PROJECT_SETTINGS_LOCAL_FILE),
            "host:\n  hot_reloading: true\n",
        )
        .expect("the local file writes");
        let (stamp, settings) = stamp
            .reread(directory.path())
            .expect("the local edit is noticed");
        assert!(
            settings.hot_reloading(),
            "the local file overrides the project one"
        );
        assert!(
            stamp.reread(directory.path()).is_none(),
            "the new stamp is the files as they now are"
        );
    }

    /// Every `project_settings.yaml` in a folder tree, sorted.
    fn settings_files_under(directory: &Path, found: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            if path.is_dir() {
                let skipped = matches!(
                    &*name,
                    "target" | ".git" | "bin" | "obj" | "node_modules" | ".construction_site"
                );
                if !skipped {
                    settings_files_under(&path, found);
                }
            } else if name == PROJECT_SETTINGS_FILE {
                found.push(path);
            }
        }
    }

    #[test]
    fn a_module_entry_reads_its_build_options() {
        let directory = project_with(
            r#"name: "Probe"
build_binary_name: "Probe"
modules:
  pill_renderer:
    enabled: true
    build:
      ray_tracing: true
      gpu_timing: false
"#,
        );
        let settings = ProjectSettings::read(directory.path()).expect("the project reads");

        let options = settings.build_options("pill_renderer");
        assert_eq!(options.get("ray_tracing"), Some(&true));
        assert_eq!(options.get("gpu_timing"), Some(&false));
        assert!(settings.build_options("pill_audio").is_empty());
    }

    /// The shape is only standardized if the projects in the tree agree with
    /// it: every file resolves without a diagnostic, and a project that
    /// enables a renderer enables the data crate it draws with.
    #[test]
    fn every_project_in_the_repository_resolves() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let extensions_root = repository.join("modules").join("extensions");
        let mut files = Vec::new();
        settings_files_under(&repository, &mut files);
        files.sort();
        assert!(
            files.len() >= 20,
            "the tree should hold the examples and the suites' projects, found {}",
            files.len()
        );

        for file in files {
            let directory = file.parent().expect("a settings file sits in a folder");
            let settings = ProjectSettings::read(directory)
                .unwrap_or_else(|error| panic!("`{}` does not resolve: {error}", file.display()));
            assert!(
                settings.resolved().diagnostics().is_empty(),
                "`{}` carries a setting nothing reads: {:?}",
                file.display(),
                settings.resolved().diagnostics()
            );

            for name in &settings.enabled_modules {
                let Some(location) = crate::config::find_extension_crate(&extensions_root, name)
                else {
                    continue;
                };
                let Some(data_crate) = crate::config::declared_pill_tag(
                    &location.directory_under(&extensions_root),
                    crate::config::RENDERER_DATA_TAG,
                ) else {
                    continue;
                };
                assert!(
                    settings.enabled_modules.contains(&data_crate),
                    "`{}` enables `{name}` without its data crate `{data_crate}`",
                    file.display()
                );
            }
        }
    }
}
