//! Metadata files beside an asset's source, and loading an asset through one.
//!
//! # Responsibilities
//!
//! - Name the metadata file of a source asset ([`metadata_path_for`]): the
//!   source's normalized path with `.meta` appended.
//! - Define what an asset type needs to be loaded through its metadata
//!   ([`ImportedAsset`]): import settings and a decoder that takes them.
//! - Own the import pipeline on [`AssetManager`]: [`AssetManager::import`],
//!   [`AssetManager::reimport`], [`AssetManager::ensure_metadata`] and
//!   [`AssetManager::follow_move`].
//! - Hold the file-name rule every editor's create dialog shares
//!   ([`standalone_file_name`]): which base names and characters Windows
//!   refuses. It lives here so every editor builds the same name, and a
//!   standalone asset file is the usual user.
//! - Hold the path check every tool-side asset path goes through
//!   ([`checked_relative`]): relative, inside the asset directory, and not
//!   empty.
//!
//! # Design
//!
//! A source file in a project's `res` directory (`textures/helmet.jpg`) can
//! carry a metadata file beside it. It holds what the source does not say for
//! itself: the asset's stable guid and its import settings. It never holds
//! the decoded payload. The metadata name keeps the source's extension
//! instead of replacing it, so `helmet.png` and `helmet.obj` get different
//! metadata files, and a `.meta` file is never an asset itself.
//!
//! The file is YAML with a small header the engine owns and the settings the
//! asset type owns:
//!
//! ```yaml
//! format_version: 1
//! asset_type: pill_master_renderer::Texture
//! guid: 9c0e4b7a1f2d3e4c5b6a798812345678
//! settings:
//!   texture_type: Normal
//! ```
//!
//! An asset loaded this way is named by its normalized path relative to `res`
//! and keyed by the guid in its metadata. One path is one asset, which is what
//! a project reload needs when it runs its loading code again. A guid claimed
//! by a second path (a source copied together with its `.meta`) is refused
//! rather than silently moved, because the store would otherwise rebind it.
//!
//! Nothing here stores a function pointer: [`AssetManager::import`] is generic,
//! so each call is compiled into the binary that makes it and a reload has
//! nothing to re-point. Code that does not know an asset's type goes through
//! [`ImportRegistry`](crate::asset_import_registry::ImportRegistry), which does
//! store function pointers and handles the reload hazard that comes with them.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_core::telemetry::telemetry_target;
use serde::de::DeserializeOwned;
use serde::Serialize;
use trait_type_map::TraitAccessible;

// Current crate
use crate::asset::{Asset, AssetGuid, AssetLoadError, AssetLoadResult, AssetManager, Handle};
use crate::asset_store::{self, pack_key};

// =============================================================================
// Metadata files
// =============================================================================

/// The extension every metadata file carries, appended to its source's name.
pub const METADATA_EXTENSION: &str = "meta";

/// The metadata format version this engine writes, and the newest one it
/// reads.
pub const METADATA_FORMAT_VERSION: u32 = 1;

/// The metadata file for the source asset at `source_path`, relative to the
/// project's asset directory, or `None` when `source_path` cannot have one.
///
/// The path is normalized the way an asset name is (`/`-separated, `.`
/// removed, by the same rules as the asset store's pack key), and `.meta` is
/// appended to the full file name: `textures\helmet.jpg` gives
/// `textures/helmet.jpg.meta`.
///
/// Returns `None` for a path that does not normalize (an absolute path, or one
/// that climbs out with `..`), and for one that is already a metadata file.
pub fn metadata_path_for(source_path: &Path) -> Option<PathBuf> {
    let name = pack_key(source_path)?;
    if is_metadata_file(&name) {
        return None;
    }
    Some(PathBuf::from(format!("{name}.{METADATA_EXTENSION}")))
}

/// Whether `name` is itself a metadata file.
fn is_metadata_file(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(METADATA_EXTENSION))
}

// =============================================================================
// Asset file names and paths
// =============================================================================

/// Base names Windows refuses, whatever the extension.
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters Windows refuses in a file name.
const INVALID_CHARACTERS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// The file name a new asset called `name` gets with `extension`: the name
/// with `.extension` appended, unless it already ends with that extension in
/// some letter case.
///
/// It is the naming rule a "create asset" dialog checks, kept here so every
/// editor builds the same name, and it only states the rule: nothing here
/// creates or checks a file.
///
/// # Errors
///
/// A message when `name`:
///
/// - is empty;
/// - contains a path separator or a character Windows refuses, including
///   control characters;
/// - contains `..`;
/// - ends with a dot or a space;
/// - is a reserved Windows name (`CON`, `NUL`, `COM1`, ...).
pub fn standalone_file_name(name: &str, extension: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the name is empty".to_owned());
    }
    if let Some(character) = name
        .chars()
        .find(|character| INVALID_CHARACTERS.contains(character) || character.is_control())
    {
        return Err(format!("the name cannot contain `{character}`"));
    }
    if name == "." || name == ".." || name.contains("..") {
        return Err("the name cannot contain `..`".to_owned());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("the name cannot end with a dot or a space".to_owned());
    }
    let suffix = format!(".{extension}");
    let file_name = if name
        .to_ascii_lowercase()
        .ends_with(&suffix.to_ascii_lowercase())
    {
        name.to_owned()
    } else {
        format!("{name}{suffix}")
    };
    let base = file_name.split('.').next().unwrap_or_default();
    if RESERVED_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(base))
    {
        return Err(format!("`{base}` is a name Windows reserves"));
    }
    Ok(file_name)
}

/// Checks that `path` stays inside the asset directory and returns it as a
/// `PathBuf`: relative, without `..`, and not empty.
///
/// This is the one check every tool-side asset path goes through, so a path
/// that would escape the directory is refused before it reaches the file
/// system.
///
/// # Errors
///
/// A message when the path is absolute, climbs out with `..`, or is empty.
pub fn checked_relative(path: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(path.replace('\\', "/"));
    let mut normal = PathBuf::new();
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(part) => normal.push(part),
            std::path::Component::CurDir => {}
            _ => return Err(format!("`{path}` is not a path inside the asset directory")),
        }
    }
    if normal.as_os_str().is_empty() {
        return Err("the path is empty".to_owned());
    }
    Ok(normal)
}

// =============================================================================
// ImportedAsset
// =============================================================================

/// An asset built from a source file in `res`, with import settings that can
/// live in an `<asset_name>.meta` file beside it.
///
/// Implemented by the crate that owns the asset type, next to that type's own
/// constructors. [`AssetManager::import`] and [`AssetManager::reimport`] are
/// the only callers.
pub trait ImportedAsset: Asset + TraitAccessible<dyn Asset> + Sized {
    /// Everything the source bytes do not say for themselves.
    ///
    /// Every field becomes an on-disk contract, so keep the list short. Mark
    /// the struct `#[serde(default)]` so a file written before a field existed
    /// still reads, and use `BTreeMap` rather than `HashMap` for any map: a
    /// value stored in the world outlives the binary that built it, and an
    /// empty `HashMap` points into that binary.
    type ImportSettings: Serialize + DeserializeOwned + Default + Clone;

    /// The source file extensions this type is imported from, lowercase and
    /// without the dot (`["png", "jpg"]`).
    ///
    /// The [`ImportRegistry`](crate::asset_import_registry::ImportRegistry)
    /// maps a file to its asset type by these extensions, so the mapping comes
    /// from the types themselves rather than from a list in the engine. Two
    /// types may not claim one extension.
    const SOURCE_EXTENSIONS: &'static [&'static str];

    /// The stable name written to a metadata file's `asset_type`, which a
    /// file must match to be read as this type.
    ///
    /// Defaults to [`Asset::shared_name`]. A type without a shared name falls
    /// back to its stored type name, `<crate>::<Type>`
    /// ([`stable_type_name`](crate::type_names::stable_type_name)), which does
    /// not change when the type moves to another module.
    fn metadata_type_name() -> &'static str {
        Self::shared_name().unwrap_or_else(crate::type_names::stable_type_name::<Self>)
    }

    /// Decode `source_bytes` into an asset with the given settings.
    ///
    /// `name` is the asset's normalized path, for the asset's own label and
    /// for error messages.
    ///
    /// # Errors
    ///
    /// Returns an [`AssetLoadError`] (usually `Decode`) when the bytes cannot
    /// be read as this asset with these settings.
    fn import(
        name: &str,
        source_bytes: &[u8],
        settings: &Self::ImportSettings,
    ) -> AssetLoadResult<Self>;
}

// =============================================================================
// Requests and outcomes
// =============================================================================

/// What [`AssetManager::import`] does when an asset has no metadata file yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataPolicy {
    /// Build the asset from the request's initial settings and write nothing.
    ReadIfPresent,
    /// The same, then write the metadata file beside the source - when the
    /// source is on a writable filesystem. A packed source (every shipping or
    /// web build) behaves as [`Self::ReadIfPresent`].
    CreateIfMissing,
}

/// One request to [`AssetManager::import`].
pub struct AssetImport<T: ImportedAsset> {
    /// The source file, relative to the project's asset directory.
    pub path: PathBuf,
    /// What to do when the source has no metadata file.
    pub policy: MetadataPolicy,
    /// The settings used when no metadata file exists. An existing file's
    /// settings always win over these.
    pub initial_settings: T::ImportSettings,
}

impl<T: ImportedAsset> AssetImport<T> {
    /// A request for `path` with the type's default initial settings.
    pub fn new(path: impl Into<PathBuf>, policy: MetadataPolicy) -> Self {
        Self {
            path: path.into(),
            policy,
            initial_settings: T::ImportSettings::default(),
        }
    }

    /// Use `settings` when the source has no metadata file yet.
    pub fn with_initial_settings(mut self, settings: T::ImportSettings) -> Self {
        self.initial_settings = settings;
        self
    }
}

/// Where an imported asset's guid and settings came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataSource {
    /// An existing metadata file, packed or on disk.
    ReadFromFile,
    /// No file existed; this import wrote one.
    CreatedOnDisk,
    /// No file existed and none was written. The policy did not ask for one,
    /// or the source is not on a writable filesystem. The guid lasts only for
    /// this run.
    InMemoryOnly,
}

/// The result of a successful [`AssetManager::import`].
pub struct ImportOutcome<T: Asset> {
    /// The asset's handle.
    pub handle: Handle<T>,
    /// The asset's guid, or `None` when it has none.
    ///
    /// The only import without a guid is a path that is already loaded and
    /// was registered under its name alone ([`AssetManager::add_named`]
    /// rather than [`AssetManager::add_named_with_guid`]) when there is also
    /// no metadata file to take one from. Every other import reads a guid or
    /// draws one.
    pub guid: Option<AssetGuid>,
    /// Where the guid and settings came from.
    pub metadata: MetadataSource,
    /// `true` when the path was already loaded and its existing handle was
    /// returned. The source is not decoded and nothing is written.
    pub already_loaded: bool,
}

// Written out by hand: a derive would require `T: Debug`, and a handle stores
// no `T`.
impl<T: Asset> std::fmt::Debug for ImportOutcome<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImportOutcome")
            .field("handle", &self.handle)
            .field("guid", &self.guid)
            .field("metadata", &self.metadata)
            .field("already_loaded", &self.already_loaded)
            .finish()
    }
}

/// The result of a successful [`AssetManager::reimport`].
pub struct ReimportOutcome<T: Asset> {
    /// The asset's handle. It is the handle the asset already had when it was
    /// loaded.
    pub handle: Handle<T>,
    /// The asset's guid, or `None` under the same conditions as
    /// [`ImportOutcome::guid`]. A path imported afresh always has one. A path
    /// already loaded without one still has none.
    pub guid: Option<AssetGuid>,
    /// Where the settings came from. It is [`MetadataSource::ReadFromFile`],
    /// or [`MetadataSource::InMemoryOnly`] when the source has no metadata
    /// file and the type's default settings were used.
    pub metadata: MetadataSource,
    /// `true` when the loaded value was replaced in its slot. `false` when the
    /// path was not loaded and was imported fresh instead.
    pub replaced: bool,
}

// Written out by hand for the same reason: a derive would need `T: Debug`,
// and a handle stores no `T`.
impl<T: Asset> std::fmt::Debug for ReimportOutcome<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReimportOutcome")
            .field("handle", &self.handle)
            .field("guid", &self.guid)
            .field("metadata", &self.metadata)
            .field("replaced", &self.replaced)
            .finish()
    }
}

/// Why [`AssetManager::import`] did not load an asset.
#[derive(Debug, thiserror::Error)]
pub enum AssetImportError {
    /// Reading or decoding the source or its metadata file failed.
    #[error(transparent)]
    Load(#[from] AssetLoadError),
    /// The path is already loaded, but not under the guid its metadata file
    /// now holds. Either the file was edited, or the name was taken by an
    /// asset that was added without metadata (`loaded` is then `None`).
    #[error("asset `{path}` is loaded under guid {}, but its metadata says {metadata}", display_guid(.loaded))]
    PathBoundToOtherGuid {
        /// The asset's normalized path.
        path: String,
        /// The guid the loaded asset has, if any.
        loaded: Option<AssetGuid>,
        /// The guid in the metadata file.
        metadata: AssetGuid,
    },
    /// The metadata file's guid already belongs to another live asset of this
    /// type. It is usually a source copied together with its `.meta`.
    #[error(
        "asset `{path}` claims guid {guid}, which `{other_path}` already has; \
         was it copied together with its .meta file?"
    )]
    DuplicateGuid {
        /// The contested guid.
        guid: AssetGuid,
        /// The path being imported.
        path: String,
        /// The path that already owns the guid.
        other_path: String,
    },
}

/// A guid for an error message, or "none".
fn display_guid(guid: &Option<AssetGuid>) -> String {
    guid.map_or_else(|| "none".to_owned(), |guid| guid.to_string())
}

// =============================================================================
// The metadata file
// =============================================================================

/// The metadata file as written, holding the engine's header and then the
/// type's settings.
#[derive(Serialize)]
struct MetadataDocument<'settings, S> {
    format_version: u32,
    asset_type: &'static str,
    guid: AssetGuid,
    settings: &'settings S,
}

/// What a metadata file says, once checked against the asset type.
struct StoredMetadata<S> {
    guid: AssetGuid,
    settings: S,
}

/// Parse a metadata file for `T`.
///
/// The header is checked before the settings are read, so the error says what
/// is actually wrong: a newer format, another asset type, or settings that do
/// not fit this type. A missing `settings` object means "all defaults".
fn parse_metadata<T: ImportedAsset>(
    path: &Path,
    bytes: &[u8],
) -> AssetLoadResult<StoredMetadata<T::ImportSettings>> {
    let (guid, settings) = parse_header(path, bytes, T::metadata_type_name(), "settings")?;
    let settings = match settings {
        None => T::ImportSettings::default(),
        Some(settings) => {
            serde_json::from_value(settings).map_err(|error| AssetLoadError::Metadata {
                path: path.to_owned(),
                detail: format!("`settings` do not fit this asset type: {error}"),
            })?
        }
    };
    Ok(StoredMetadata { guid, settings })
}

/// What a header says, before any caller checks the type it expected.
pub(crate) struct RawHeader {
    /// The `asset_type` the file declares.
    pub asset_type: String,
    /// The guid the file carries.
    pub guid: AssetGuid,
}

/// Reads the header that a `.meta` sidecar and a standalone asset share, and
/// refuses one this engine cannot read.
///
/// This is the one statement of the on-disk contract: the format version range
/// and the 32-digit guid rule. The type the caller expected is checked by
/// [`parse_header`], because the asset index reads the same header without
/// expecting one particular type.
///
/// The error carries only the detail sentence. [`parse_header`] adds the path
/// it knows.
pub(crate) fn read_header(
    bytes: &[u8],
) -> Result<(RawHeader, serde_json::Map<String, serde_json::Value>), String> {
    let document = crate::asset_document::parse(bytes)?;
    let serde_json::Value::Object(header) = document else {
        return Err("not a YAML mapping".to_owned());
    };

    let version = header
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "`format_version` is missing".to_owned())?;
    if version == 0 || version > u64::from(METADATA_FORMAT_VERSION) {
        return Err(format!(
            "format version {version} is not supported; this engine reads up to {METADATA_FORMAT_VERSION}"
        ));
    }

    let asset_type = header
        .get("asset_type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "`asset_type` is missing".to_owned())?
        .to_owned();
    let guid = header
        .get("guid")
        .and_then(serde_json::Value::as_str)
        .and_then(AssetGuid::parse)
        .ok_or_else(|| "`guid` is missing or not 32 hexadecimal digits".to_owned())?;

    Ok((RawHeader { asset_type, guid }, header))
}

/// Check the header every asset file shares and return its guid and the value
/// under `body_field` (`None` when absent or `null`).
///
/// The header is checked before the body is looked at, so the error says what
/// is actually wrong: a newer format, another asset type, or a bad guid.
pub(crate) fn parse_header(
    path: &Path,
    bytes: &[u8],
    expected_type: &str,
    body_field: &str,
) -> AssetLoadResult<(AssetGuid, Option<serde_json::Value>)> {
    let fail = |detail: String| AssetLoadError::Metadata {
        path: path.to_owned(),
        detail,
    };
    let (raw, header) = read_header(bytes).map_err(fail)?;
    // A file written while type names kept their module path still names the
    // same type (`pill_master_renderer::Mesh` for `...::Mesh`).
    if !crate::type_names::names_same_type(&raw.asset_type, expected_type) {
        return Err(fail(format!(
            "it describes a `{}`, not a `{expected_type}`",
            raw.asset_type
        )));
    }

    let body = match header.get(body_field) {
        None | Some(serde_json::Value::Null) => None,
        Some(body) => Some(body.clone()),
    };
    Ok((raw.guid, body))
}

/// Whether two settings values are the same, compared through their JSON form
/// so that `ImportSettings` need not implement `PartialEq`.
fn same_settings<T: ImportedAsset>(left: &T::ImportSettings, right: &T::ImportSettings) -> bool {
    match (serde_json::to_value(left), serde_json::to_value(right)) {
        (Ok(left), Ok(right)) => left == right,
        // Settings that cannot be compared are treated as different, which
        // costs at most one extra decode.
        _ => false,
    }
}

/// Returns the bytes of a metadata file for `T` holding `guid` and `settings`.
fn render_metadata<T: ImportedAsset>(
    path: &Path,
    guid: AssetGuid,
    settings: &T::ImportSettings,
) -> AssetLoadResult<Vec<u8>> {
    let document = MetadataDocument {
        format_version: METADATA_FORMAT_VERSION,
        asset_type: T::metadata_type_name(),
        guid,
        settings,
    };
    crate::asset_document::render(&document).map_err(|error| AssetLoadError::Metadata {
        path: path.to_owned(),
        detail: format!("the settings cannot be written as YAML: {error}"),
    })
}

// =============================================================================
// AssetManager::import
// =============================================================================

impl AssetManager {
    /// Load the source asset at `request.path` through its metadata file.
    ///
    /// The asset is named by its normalized path and keyed by the guid in its
    /// metadata. An existing metadata file's guid and settings win; otherwise
    /// the request's initial settings and a new random guid are used. With
    /// [`MetadataPolicy::CreateIfMissing`] the file is written beside the
    /// source, and only after the source decoded, so a broken source never
    /// leaves one behind. An existing file is never overwritten, even when it
    /// cannot be read.
    ///
    /// Importing a path that is already loaded returns its existing handle
    /// with [`ImportOutcome::already_loaded`] set, and decodes nothing.
    ///
    /// # Errors
    ///
    /// - [`AssetImportError::Load`] when the path does not normalize or is a
    ///   metadata file, the source cannot be read or decoded, or the metadata
    ///   file is unreadable, for another asset type, or from a newer format.
    /// - [`AssetImportError::PathBoundToOtherGuid`] when the path is loaded
    ///   under a different guid than its metadata file now holds.
    /// - [`AssetImportError::DuplicateGuid`] when another live asset of this
    ///   type already has the metadata file's guid.
    pub fn import<T: ImportedAsset>(
        &mut self,
        request: AssetImport<T>,
    ) -> Result<ImportOutcome<T>, AssetImportError> {
        let AssetImport {
            path,
            policy,
            initial_settings,
        } = request;

        // Step 1: the asset's name and its metadata file's path.
        let asset_name =
            pack_key(&path).ok_or_else(|| AssetLoadError::PathNotFound { path: path.clone() })?;
        let metadata_path =
            metadata_path_for(Path::new(&asset_name)).ok_or_else(|| AssetLoadError::Metadata {
                path: path.clone(),
                detail: "a .meta file describes an asset; it is not one".to_owned(),
            })?;

        // Step 2: what the metadata file says, when there is one.
        let stored = match asset_store::read(&metadata_path) {
            Ok(bytes) => Some(parse_metadata::<T>(&metadata_path, &bytes)?),
            Err(AssetLoadError::PathNotFound { .. }) => None,
            Err(error) => return Err(error.into()),
        };

        // Step 3: a path that is already loaded is either this same asset or a
        // conflict. Without a metadata file there is no guid to disagree
        // with, so the loaded asset's own guid stands. When it has none
        // either, there is nothing to report but the handle.
        if let Some(handle) = self.handle_by_name::<T>(&asset_name) {
            let loaded = self.guid_of(handle);
            return match (&stored, loaded) {
                (Some(stored), Some(loaded)) if stored.guid == loaded => Ok(ImportOutcome {
                    handle,
                    guid: Some(loaded),
                    metadata: MetadataSource::ReadFromFile,
                    already_loaded: true,
                }),
                (None, Some(loaded)) => Ok(ImportOutcome {
                    handle,
                    guid: Some(loaded),
                    metadata: MetadataSource::InMemoryOnly,
                    already_loaded: true,
                }),
                (None, None) => Ok(ImportOutcome {
                    handle,
                    guid: None,
                    metadata: MetadataSource::InMemoryOnly,
                    already_loaded: true,
                }),
                (Some(stored), loaded) => Err(AssetImportError::PathBoundToOtherGuid {
                    path: asset_name,
                    loaded,
                    metadata: stored.guid,
                }),
            };
        }

        // The guid and settings this import uses. A new guid is drawn only
        // here, once the asset is known to be new.
        let has_metadata_file = stored.is_some();
        let (guid, settings) = match stored {
            Some(stored) => (stored.guid, stored.settings),
            None => {
                let guid = AssetGuid::random().map_err(|error| AssetLoadError::Metadata {
                    path: metadata_path.clone(),
                    detail: format!("no random source for a new guid: {error}"),
                })?;
                (guid, initial_settings)
            }
        };

        // Step 4: refuse a guid another asset already holds - the store would
        // otherwise rebind it without a word.
        if let Some(other) = self.handle_by_guid::<T>(guid) {
            return Err(AssetImportError::DuplicateGuid {
                guid,
                path: asset_name,
                other_path: self.name_of(other).unwrap_or("<unnamed>").to_owned(),
            });
        }

        // Step 5: decode the source.
        let source_bytes = asset_store::read(Path::new(&asset_name))?;
        let mut asset = T::import(&asset_name, &source_bytes, &settings)?;
        let mut guid = guid;

        // Step 6: record the guid and settings when the policy asks for it and
        // the source allows it. A failed write only costs the persistence of
        // this run's guid, so it is reported but does not refuse an asset that
        // decoded.
        let mut metadata = if has_metadata_file {
            MetadataSource::ReadFromFile
        } else {
            MetadataSource::InMemoryOnly
        };
        if !has_metadata_file && policy == MetadataPolicy::CreateIfMissing {
            let document = render_metadata::<T>(&metadata_path, guid, &settings)?;
            match asset_store::write_beside(Path::new(&asset_name), &metadata_path, &document) {
                Ok(Some(_)) => {
                    // Another writer - a second host, a parallel test - may
                    // have created the file between step 2 and now, and
                    // `write_beside` keeps theirs. The file is the truth, so
                    // read it back and adopt it if it is not ours.
                    let written_bytes = asset_store::read(&metadata_path)?;
                    let on_disk = parse_metadata::<T>(&metadata_path, &written_bytes)?;
                    if on_disk.guid == guid {
                        metadata = MetadataSource::CreatedOnDisk;
                    } else {
                        if let Some(other) = self.handle_by_guid::<T>(on_disk.guid) {
                            return Err(AssetImportError::DuplicateGuid {
                                guid: on_disk.guid,
                                path: asset_name,
                                other_path: self.name_of(other).unwrap_or("<unnamed>").to_owned(),
                            });
                        }
                        if !same_settings::<T>(&on_disk.settings, &settings) {
                            asset = T::import(&asset_name, &source_bytes, &on_disk.settings)?;
                        }
                        guid = on_disk.guid;
                        metadata = MetadataSource::ReadFromFile;
                    }
                }
                Ok(None) => pill_core::debug!(
                    target: telemetry_target::RESOURCES,
                    asset = asset_name.as_str(),
                    "asset is not on a writable filesystem; its metadata was not written"
                ),
                Err(error) => pill_core::warn!(
                    target: telemetry_target::RESOURCES,
                    asset = asset_name.as_str(),
                    error = %error,
                    "asset loaded, but its metadata could not be written"
                ),
            }
        }

        // Step 7: steps 3 and 4 ruled out both guid conflicts, so this cannot
        // fail.
        let handle = self
            .add_named_with_guid(asset_name, guid, asset)
            .expect("the path was checked to be unloaded before decoding");
        // Step 8: the settings are this run's own rather than a file's, so
        // remember them. A later `ensure_metadata` then writes what actually
        // decoded the asset instead of the type's defaults.
        if metadata == MetadataSource::InMemoryOnly {
            self.set_import_settings(handle, settings_to_json::<T>(&metadata_path, &settings)?);
        }
        Ok(ImportOutcome {
            handle,
            guid: Some(guid),
            metadata,
            already_loaded: false,
        })
    }
}

// =============================================================================
// AssetManager::reimport
// =============================================================================

impl AssetManager {
    /// Decode the asset at `path` again and replace the loaded value in its
    /// slot, after its source or its metadata file changed.
    ///
    /// The handle, name and guid stay what they were, and the slot's content
    /// version moves. Whatever holds the handle therefore sees the new value,
    /// and a renderer re-uploads only this asset. The settings come from the
    /// metadata file, or are the type's defaults when there is none. Nothing
    /// is written. A path that is not loaded is imported instead, with
    /// [`MetadataPolicy::ReadIfPresent`], and `replaced` is `false`.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::import`]. A metadata file whose guid no longer
    /// matches the loaded asset is [`AssetImportError::PathBoundToOtherGuid`].
    /// On every error the loaded value is left exactly as it was: a source
    /// that fails to decode never empties its slot.
    pub fn reimport<T: ImportedAsset>(
        &mut self,
        path: &Path,
    ) -> Result<ReimportOutcome<T>, AssetImportError> {
        // Step 1: name the path. A path nothing has loaded is imported, not
        // reimported.
        let asset_name = pack_key(path).ok_or_else(|| AssetLoadError::PathNotFound {
            path: path.to_owned(),
        })?;
        let Some(handle) = self.handle_by_name::<T>(&asset_name) else {
            let imported =
                self.import(AssetImport::<T>::new(path, MetadataPolicy::ReadIfPresent))?;
            return Ok(ReimportOutcome {
                handle: imported.handle,
                guid: imported.guid,
                metadata: imported.metadata,
                replaced: false,
            });
        };

        // Step 2: the metadata file decides the settings. A guid it changed
        // under a loaded asset is refused rather than followed, because
        // handles and references elsewhere still name the old one.
        let metadata_path =
            metadata_path_for(Path::new(&asset_name)).ok_or_else(|| AssetLoadError::Metadata {
                path: path.to_owned(),
                detail: "a .meta file describes an asset; it is not one".to_owned(),
            })?;
        let loaded = self.guid_of(handle);
        let (settings, metadata) = match asset_store::read(&metadata_path) {
            Ok(bytes) => {
                let stored = parse_metadata::<T>(&metadata_path, &bytes)?;
                if Some(stored.guid) != loaded {
                    return Err(AssetImportError::PathBoundToOtherGuid {
                        path: asset_name,
                        loaded,
                        metadata: stored.guid,
                    });
                }
                (stored.settings, MetadataSource::ReadFromFile)
            }
            Err(AssetLoadError::PathNotFound { .. }) => {
                (T::ImportSettings::default(), MetadataSource::InMemoryOnly)
            }
            Err(error) => return Err(error.into()),
        };

        // Step 3: decode fully before touching the slot, so a failure leaves
        // it as it is.
        let source_bytes = asset_store::read(Path::new(&asset_name))?;
        let asset = T::import(&asset_name, &source_bytes, &settings)?;
        *self
            .get_mut(handle)
            .expect("the handle was looked up by name just above") = asset;
        // Step 4: without a metadata file the type's defaults decoded this
        // asset, so that is what a sidecar created now has to say. An earlier
        // record would name settings the loaded value no longer has.
        if metadata == MetadataSource::InMemoryOnly {
            self.set_import_settings(handle, settings_to_json::<T>(&metadata_path, &settings)?);
        }
        Ok(ReimportOutcome {
            handle,
            guid: loaded,
            metadata,
            replaced: true,
        })
    }
}

// =============================================================================
// AssetManager::ensure_metadata
// =============================================================================

impl AssetManager {
    /// Write the metadata file of the source at `path` when it has none, and
    /// return the guid written.
    ///
    /// Returns `None` when the file already exists, when the source is not on
    /// a writable filesystem (a packed asset), or when another writer's file
    /// carries a different guid.
    ///
    /// Nothing is decoded. A source nothing has loaded gets `T`'s default
    /// settings, while one already loaded under `path`'s name gets the
    /// settings it was decoded with. This way a `.meta` deleted while the
    /// program ran comes back describing the same value. A loaded asset keeps
    /// its own guid, and anything else gets a new random one. An existing file
    /// is never touched.
    ///
    /// # Errors
    ///
    /// When the path does not normalize, the existing metadata cannot be
    /// probed, no random guid is available, or the file cannot be written or
    /// read back.
    pub fn ensure_metadata<T: ImportedAsset>(
        &self,
        path: &Path,
    ) -> Result<Option<AssetGuid>, AssetImportError> {
        let asset_name = pack_key(path).ok_or_else(|| AssetLoadError::PathNotFound {
            path: path.to_owned(),
        })?;
        let metadata_path =
            metadata_path_for(Path::new(&asset_name)).ok_or_else(|| AssetLoadError::Metadata {
                path: path.to_owned(),
                detail: "a .meta file describes an asset; it is not one".to_owned(),
            })?;
        // Step 1: an existing metadata file is never touched.
        match asset_store::read(&metadata_path) {
            Ok(_) => return Ok(None),
            Err(AssetLoadError::PathNotFound { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        // Step 2: a loaded asset keeps its own guid; anything else gets a new
        // random one.
        let loaded_handle = self.handle_by_name::<T>(&asset_name);
        let guid = match loaded_handle.and_then(|handle| self.guid_of(handle)) {
            Some(guid) => guid,
            None => AssetGuid::random().map_err(|error| AssetLoadError::Metadata {
                path: metadata_path.clone(),
                detail: format!("no random source for a new guid: {error}"),
            })?,
        };
        // Step 3: an asset this run already decoded knows the settings it was
        // decoded with; a source nothing has loaded gets the type's defaults.
        // Writing defaults over a loaded asset's own settings would change how
        // the asset decodes on the next run - a normal map read as sRGB.
        let recorded_settings = loaded_handle.and_then(|handle| self.import_settings::<T>(handle));
        let settings = match recorded_settings {
            Some(settings) => serde_json::from_value(settings.clone()).map_err(|error| {
                AssetLoadError::Metadata {
                    path: metadata_path.clone(),
                    detail: format!("the recorded settings do not fit this type: {error}"),
                }
            })?,
            None => T::ImportSettings::default(),
        };
        // Step 4: write the file, then read it back to learn which guid it
        // ended up with.
        let document = render_metadata::<T>(&metadata_path, guid, &settings)?;
        match asset_store::write_beside(Path::new(&asset_name), &metadata_path, &document)? {
            // Another writer may have created the file in the meantime. Its guid
            // is the file's truth, and it was not written here.
            Some(written_path) => {
                pill_core::debug!(
                    target: telemetry_target::RESOURCES,
                    asset = asset_name.as_str(),
                    settings = match recorded_settings {
                        Some(_) => "loaded",
                        None => "defaults",
                    },
                    "asset metadata written"
                );
                let bytes =
                    std::fs::read(&written_path).map_err(|source| AssetLoadError::Read {
                        path: written_path.clone(),
                        source,
                    })?;
                let on_disk = parse_metadata::<T>(&metadata_path, &bytes)?;
                Ok((on_disk.guid == guid).then_some(guid))
            }
            None => Ok(None),
        }
    }
}

// =============================================================================
// AssetManager::follow_move
// =============================================================================

impl AssetManager {
    /// When the source at `path` is a loaded asset that was moved here
    /// together with its metadata file, rename that asset to `path` and
    /// return its old name.
    ///
    /// A move is recognized by the guid. The metadata file at `path` holds the
    /// guid of a loaded asset that has another name, and that asset's source
    /// file is gone from its old path. The asset keeps its handle, guid and
    /// value, and nothing is decoded, so whatever holds it is unaffected.
    ///
    /// Returns `Ok(None)` and changes nothing when `path` is already loaded,
    /// has no metadata file, holds a guid no loaded asset has, or when the
    /// other asset's source still exists. That last case is a copy, which
    /// importing reports as [`AssetImportError::DuplicateGuid`].
    ///
    /// # Errors
    ///
    /// When the path does not normalize, or its metadata file exists but does
    /// not read as `T`'s.
    pub fn follow_move<T: ImportedAsset>(
        &mut self,
        path: &Path,
    ) -> Result<Option<String>, AssetImportError> {
        // Step 1: a path that is already loaded, or has no metadata file, is
        // not a move.
        let asset_name = pack_key(path).ok_or_else(|| AssetLoadError::PathNotFound {
            path: path.to_owned(),
        })?;
        if self.handle_by_name::<T>(&asset_name).is_some() {
            return Ok(None);
        }
        let Some(metadata_path) = metadata_path_for(Path::new(&asset_name)) else {
            return Ok(None);
        };
        // Step 2: read the guid from the metadata file and let
        // `follow_move_of` decide.
        let stored = match asset_store::read(&metadata_path) {
            Ok(bytes) => parse_metadata::<T>(&metadata_path, &bytes)?,
            Err(AssetLoadError::PathNotFound { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        self.follow_move_of::<T>(asset_name, stored.guid, &metadata_path)
    }

    /// Follow a move once the path's identity is known.
    ///
    /// A sourced asset reads its guid from a `.meta` sidecar and a standalone
    /// asset reads it from its own header; both agree on everything after
    /// that. The asset must already be loaded under another name, the old
    /// source must be gone, and the loaded asset takes the new name. Keeping
    /// the decision here means a new precondition is added in one place.
    ///
    /// `identity_path` is what the error names when the rename fails: the
    /// sidecar for a sourced asset, the source itself for a standalone one.
    pub(crate) fn follow_move_of<T>(
        &mut self,
        asset_name: String,
        guid: AssetGuid,
        identity_path: &Path,
    ) -> Result<Option<String>, AssetImportError>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let Some(handle) = self.handle_by_guid::<T>(guid) else {
            return Ok(None);
        };
        let Some(old_name) = self.name_of(handle).map(str::to_owned) else {
            return Ok(None);
        };
        // The old source still being there means two files claim one guid.
        if asset_store::locate_file(Path::new(&old_name)).is_some() {
            return Ok(None);
        }
        self.rename(handle, asset_name)
            .map_err(|error| AssetLoadError::Metadata {
                path: identity_path.to_owned(),
                detail: format!("the moved asset could not take its new name: {error}"),
            })?;
        Ok(Some(old_name))
    }
}

// =============================================================================
// Settings as JSON
// =============================================================================

/// Returns the import settings `T` would use for the source at `path`, as
/// JSON: the metadata file's, or the type's defaults when there is none.
///
/// For code that edits settings without knowing `T` (an editor's inspector),
/// reached through the import registry.
///
/// # Errors
///
/// Returns an [`AssetLoadError`] when the path does not normalize, or its
/// metadata file exists but cannot be read as `T`'s.
pub fn settings_json_for<T: ImportedAsset>(path: &Path) -> AssetLoadResult<serde_json::Value> {
    let metadata_path = metadata_path_for(path).ok_or_else(|| AssetLoadError::PathNotFound {
        path: path.to_owned(),
    })?;
    let settings = match asset_store::read(&metadata_path) {
        Ok(bytes) => parse_metadata::<T>(&metadata_path, &bytes)?.settings,
        Err(AssetLoadError::PathNotFound { .. }) => T::ImportSettings::default(),
        Err(error) => return Err(error),
    };
    settings_to_json::<T>(path, &settings)
}

/// Reads `settings_document` as `T`'s import settings and writes it back,
/// which fills every missing field with its default and drops unknown ones.
///
/// # Errors
///
/// Returns [`AssetLoadError::Metadata`] when `settings_document` does not fit
/// `T`'s settings.
pub fn normalize_settings_json<T: ImportedAsset>(
    settings_document: serde_json::Value,
) -> AssetLoadResult<serde_json::Value> {
    let type_name_path = Path::new(T::metadata_type_name());
    let settings: T::ImportSettings =
        serde_json::from_value(settings_document).map_err(|error| AssetLoadError::Metadata {
            path: type_name_path.to_owned(),
            detail: format!("the settings do not fit this asset type: {error}"),
        })?;
    settings_to_json::<T>(type_name_path, &settings)
}

/// Returns `settings` as a JSON value, with `path` naming the failure.
///
/// The one serializer behind every entry point that reports a type's settings:
/// [`settings_json_for`] reads them from the metadata file or the defaults,
/// and [`normalize_settings_json`] round-trips a read-modify-write. Fields the
/// type does not declare are dropped by that round-trip, not by this
/// serializer.
fn settings_to_json<T: ImportedAsset>(
    path: &Path,
    settings: &T::ImportSettings,
) -> AssetLoadResult<serde_json::Value> {
    serde_json::to_value(settings).map_err(|error| AssetLoadError::Metadata {
        path: path.to_owned(),
        detail: format!("the settings cannot be written as JSON: {error}"),
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_store::{mount_directory, mount_pack, mounted_directory_test_lock, AssetPack};
    use std::cell::Cell;
    use trait_type_map::impl_trait_accessible;

    // -------------------------------------------------------------------------
    // Path naming
    // -------------------------------------------------------------------------

    #[test]
    fn the_metadata_file_keeps_the_source_extension() {
        assert_eq!(
            metadata_path_for(Path::new("a.jpg")),
            Some(PathBuf::from("a.jpg.meta"))
        );
        // Two sources sharing a stem get two metadata files.
        assert_ne!(
            metadata_path_for(Path::new("helmet.png")),
            metadata_path_for(Path::new("helmet.obj"))
        );
    }

    #[test]
    fn nested_and_unnormalized_paths_name_one_file() {
        let expected = Some(PathBuf::from("textures/stone/a.jpg.meta"));
        assert_eq!(
            metadata_path_for(Path::new("textures/stone/a.jpg")),
            expected
        );
        assert_eq!(
            metadata_path_for(Path::new("textures\\stone\\a.jpg")),
            expected
        );
        assert_eq!(
            metadata_path_for(Path::new("./textures/./stone/a.jpg")),
            expected
        );
    }

    #[test]
    fn a_source_without_an_extension_has_metadata_too() {
        assert_eq!(
            metadata_path_for(Path::new("data/LICENSE")),
            Some(PathBuf::from("data/LICENSE.meta"))
        );
    }

    #[test]
    fn metadata_files_and_paths_outside_res_have_none() {
        assert_eq!(metadata_path_for(Path::new("a.jpg.meta")), None);
        assert_eq!(metadata_path_for(Path::new("a.jpg.META")), None);
        assert_eq!(metadata_path_for(Path::new("../a.jpg")), None);
        assert_eq!(metadata_path_for(Path::new("")), None);
        let absolute = std::env::temp_dir().join("a.jpg");
        assert_eq!(metadata_path_for(&absolute), None);
    }

    // -------------------------------------------------------------------------
    // A dummy imported asset
    // -------------------------------------------------------------------------

    /// Two settings, with defaults that differ from any test's initial ones.
    #[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
    #[serde(default)]
    struct NoteSettings {
        volume: u32,
        label: String,
    }

    impl Default for NoteSettings {
        fn default() -> Self {
            Self {
                volume: 7,
                label: "default".to_owned(),
            }
        }
    }

    /// The decoded asset: the source text plus the settings it was read with.
    #[derive(Debug, PartialEq)]
    struct Note {
        name: String,
        text: String,
        settings: NoteSettings,
    }

    impl Asset for Note {}
    impl_trait_accessible!(dyn Asset; Note);

    thread_local! {
        /// How many times `Note::import` ran on this test's thread.
        static DECODE_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    impl ImportedAsset for Note {
        type ImportSettings = NoteSettings;
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["note"];

        fn metadata_type_name() -> &'static str {
            "pill_engine::asset_metadata::tests::Note"
        }

        fn import(
            name: &str,
            source_bytes: &[u8],
            settings: &NoteSettings,
        ) -> AssetLoadResult<Self> {
            DECODE_COUNT.with(|decode_count| decode_count.set(decode_count.get() + 1));
            let text =
                String::from_utf8(source_bytes.to_vec()).map_err(|_| AssetLoadError::Decode {
                    label: name.to_owned(),
                    detail: "not UTF-8".to_owned(),
                })?;
            if text == "broken" {
                return Err(AssetLoadError::Decode {
                    label: name.to_owned(),
                    detail: "the source says it is broken".to_owned(),
                });
            }
            Ok(Self {
                name: name.to_owned(),
                text,
                settings: settings.clone(),
            })
        }
    }

    fn decode_count() -> usize {
        DECODE_COUNT.with(Cell::get)
    }

    /// A scratch `res` directory, mounted for the test's duration under the
    /// mounted-directory lock, and removed afterwards.
    struct ScratchRes {
        root: PathBuf,
        _mounted: std::sync::MutexGuard<'static, ()>,
    }

    impl ScratchRes {
        fn new(label: &str) -> Self {
            let mounted = mounted_directory_test_lock();
            static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "pill-asset-metadata-{label}-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            mount_directory(&root);
            Self {
                root,
                _mounted: mounted,
            }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }

        fn read(&self, relative: &str) -> Option<String> {
            std::fs::read_to_string(self.root.join(relative)).ok()
        }
    }

    impl Drop for ScratchRes {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn settings(volume: u32, label: &str) -> NoteSettings {
        NoteSettings {
            volume,
            label: label.to_owned(),
        }
    }

    fn request(path: &str, policy: MetadataPolicy) -> AssetImport<Note> {
        AssetImport::new(path, policy).with_initial_settings(settings(1, "initial"))
    }

    /// One metadata file's YAML text: the shared header around the JSON-shaped
    /// `settings` fragment the tests pass.
    fn metadata_yaml(guid: &str, settings: &str) -> String {
        let settings: serde_json::Value =
            serde_json::from_str(settings).expect("the test fragment is JSON");
        let document = serde_json::json!({
            "format_version": 1,
            "asset_type": "pill_engine::asset_metadata::tests::Note",
            "guid": guid,
            "settings": settings,
        });
        let bytes = crate::asset_document::render(&document).expect("the document renders");
        String::from_utf8(bytes).expect("the document is UTF-8")
    }

    const GUID_A: &str = "0000000000000000000000000000000a";
    const GUID_B: &str = "0000000000000000000000000000000b";

    /// An asset pack holding `files`, in the layout `AssetPack::parse` reads:
    /// a magic, a version, a file count, one index entry per file, then the
    /// bytes of every file in turn.
    fn pack_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        use crate::asset_store::{ASSET_PACK_MAGIC, ASSET_PACK_VERSION};
        let index_length: usize = files.iter().map(|(path, _)| 4 + path.len() + 16).sum();
        let mut offset = (8 + 4 + 4 + index_length) as u64;
        let mut pack = Vec::new();
        pack.extend_from_slice(ASSET_PACK_MAGIC);
        pack.extend_from_slice(&ASSET_PACK_VERSION.to_le_bytes());
        pack.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for (path, bytes) in files {
            pack.extend_from_slice(&(path.len() as u32).to_le_bytes());
            pack.extend_from_slice(path.as_bytes());
            pack.extend_from_slice(&offset.to_le_bytes());
            pack.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            offset += bytes.len() as u64;
        }
        for (_, bytes) in files {
            pack.extend_from_slice(bytes);
        }
        pack
    }

    // -------------------------------------------------------------------------
    // Import
    // -------------------------------------------------------------------------

    #[test]
    fn missing_metadata_read_if_present_builds_and_writes_nothing() {
        let res = ScratchRes::new("read-if-present");
        res.write("notes/a.note", "hello");
        let mut assets = AssetManager::new();

        let outcome = assets
            .import(request("notes/a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();

        assert_eq!(outcome.metadata, MetadataSource::InMemoryOnly);
        assert!(!outcome.already_loaded);
        let note = assets.get(outcome.handle).unwrap();
        assert_eq!(note.text, "hello");
        assert_eq!(note.settings, settings(1, "initial"));
        assert_eq!(res.read("notes/a.note.meta"), None);
    }

    #[test]
    fn missing_metadata_create_if_missing_writes_what_was_used() {
        let res = ScratchRes::new("create");
        res.write("notes/a.note", "hello");
        let mut assets = AssetManager::new();

        let outcome = assets
            .import(request("notes/a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();

        assert_eq!(outcome.metadata, MetadataSource::CreatedOnDisk);
        let written = res
            .read("notes/a.note.meta")
            .expect("the metadata file was written");
        let stored =
            parse_metadata::<Note>(Path::new("notes/a.note.meta"), written.as_bytes()).unwrap();
        assert_eq!(Some(stored.guid), outcome.guid);
        assert_eq!(stored.settings, settings(1, "initial"));
    }

    #[test]
    fn an_existing_metadata_file_beats_the_initial_settings() {
        let res = ScratchRes::new("existing");
        res.write("a.note", "hello");
        res.write(
            "a.note.meta",
            &metadata_yaml(GUID_A, r#"{"volume": 3, "label": "file"}"#),
        );
        let mut assets = AssetManager::new();

        let outcome = assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();

        assert_eq!(outcome.metadata, MetadataSource::ReadFromFile);
        assert_eq!(outcome.guid, Some(AssetGuid::parse(GUID_A).unwrap()));
        assert_eq!(
            assets.get(outcome.handle).unwrap().settings,
            settings(3, "file")
        );
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_fields_default() {
        let res = ScratchRes::new("tolerant");
        res.write("a.note", "hello");
        res.write(
            "a.note.meta",
            &metadata_yaml(GUID_A, r#"{"label": "file", "added_later": true}"#),
        );
        let mut assets = AssetManager::new();

        let outcome = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();

        // `volume` was missing: the type's default, not the initial settings.
        assert_eq!(
            assets.get(outcome.handle).unwrap().settings,
            settings(7, "file")
        );
    }

    /// A file that cannot be used is reported and left exactly as it was.
    #[test]
    fn unusable_metadata_is_an_error_and_is_never_overwritten() {
        let cases = [
            (
                "wrong-type",
                r#"{"format_version": 1, "asset_type": "Texture", "guid": "0000000000000000000000000000000a"}"#,
            ),
            (
                "newer",
                r#"{"format_version": 99, "asset_type": "pill_engine::asset_metadata::tests::Note", "guid": "0000000000000000000000000000000a"}"#,
            ),
            ("broken-json", r#"{"format_version": 1, "#),
        ];
        for (label, contents) in cases {
            let res = ScratchRes::new(label);
            res.write("a.note", "hello");
            res.write("a.note.meta", contents);
            let mut assets = AssetManager::new();

            let result = assets.import(request("a.note", MetadataPolicy::CreateIfMissing));

            assert!(
                matches!(
                    result,
                    Err(AssetImportError::Load(AssetLoadError::Metadata { .. }))
                ),
                "{label}: {result:?}"
            );
            assert_eq!(
                res.read("a.note.meta").as_deref(),
                Some(contents),
                "{label}"
            );
            assert!(assets.is_empty::<Note>(), "{label}");
        }
    }

    #[test]
    fn a_source_that_fails_to_decode_leaves_no_metadata_file() {
        let res = ScratchRes::new("broken-source");
        res.write("a.note", "broken");
        let mut assets = AssetManager::new();

        let result = assets.import(request("a.note", MetadataPolicy::CreateIfMissing));

        assert!(matches!(
            result,
            Err(AssetImportError::Load(AssetLoadError::Decode { .. }))
        ));
        assert_eq!(res.read("a.note.meta"), None);
    }

    #[test]
    fn a_packed_source_is_loaded_but_nothing_is_written() {
        let res = ScratchRes::new("packed-source");
        let key = "pill-meta-test/packed-source/a.note";
        mount_pack(AssetPack::parse(pack_of(&[(key, b"from the pack")])).unwrap());
        let mut assets = AssetManager::new();

        let outcome = assets
            .import(request(key, MetadataPolicy::CreateIfMissing))
            .unwrap();

        assert_eq!(outcome.metadata, MetadataSource::InMemoryOnly);
        assert_eq!(assets.get(outcome.handle).unwrap().text, "from the pack");
        assert_eq!(
            asset_store::locate_file(Path::new(&format!("{key}.meta"))),
            None
        );
        assert_eq!(res.read(&format!("{key}.meta")), None);
    }

    /// A shipping build reads the metadata it was packed with, not a stray
    /// file at the same development path.
    #[test]
    fn packed_metadata_is_read_before_the_filesystem() {
        let res = ScratchRes::new("packed-metadata");
        let key = "pill-meta-test/packed-metadata/a.note";
        let packed = metadata_yaml(GUID_A, r#"{"volume": 2, "label": "packed"}"#);
        mount_pack(
            AssetPack::parse(pack_of(&[(&format!("{key}.meta"), packed.as_bytes())])).unwrap(),
        );
        res.write(key, "hello");
        res.write(
            &format!("{key}.meta"),
            &metadata_yaml(GUID_B, r#"{"volume": 9, "label": "on disk"}"#),
        );
        let mut assets = AssetManager::new();

        let outcome = assets
            .import(request(key, MetadataPolicy::ReadIfPresent))
            .unwrap();

        assert_eq!(outcome.guid, Some(AssetGuid::parse(GUID_A).unwrap()));
        assert_eq!(
            assets.get(outcome.handle).unwrap().settings,
            settings(2, "packed")
        );
    }

    #[test]
    fn the_asset_is_named_by_its_normalized_path_and_keyed_by_its_guid() {
        let res = ScratchRes::new("identity");
        res.write("notes/a.note", "hello");
        res.write("notes/a.note.meta", &metadata_yaml(GUID_A, "{}"));
        let mut assets = AssetManager::new();

        let outcome = assets
            .import(request("notes\\.\\a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();

        assert_eq!(assets.name_of(outcome.handle), Some("notes/a.note"));
        assert_eq!(assets.get(outcome.handle).unwrap().name, "notes/a.note");
        let guid = AssetGuid::parse(GUID_A).unwrap();
        assert_eq!(assets.guid_of(outcome.handle), Some(guid));
        assert_eq!(assets.handle_by_guid::<Note>(guid), Some(outcome.handle));
    }

    /// A project reload runs its loading code again against a store that
    /// already holds the assets.
    #[test]
    fn importing_a_loaded_path_returns_its_handle_without_decoding() {
        let res = ScratchRes::new("reimport");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        let first = assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();
        let decode_count_after_first = decode_count();

        let second = assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();

        assert!(second.already_loaded);
        assert_eq!(second.handle, first.handle);
        assert_eq!(second.guid, first.guid);
        assert_eq!(
            decode_count(),
            decode_count_after_first,
            "the source was decoded again"
        );
        assert_eq!(assets.len::<Note>(), 1);
    }

    /// The same, for a source with no metadata file. The loaded asset's guid
    /// stands, instead of a new random one being drawn and disagreeing.
    #[test]
    fn importing_a_loaded_path_without_metadata_returns_its_handle() {
        let res = ScratchRes::new("reimport-no-metadata");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        let first = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();

        let second = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();

        assert!(second.already_loaded);
        assert_eq!((second.handle, second.guid), (first.handle, first.guid));
    }

    /// An asset a project registered by name alone carries no guid, so
    /// re-importing its path has nothing to disagree with. The import returns
    /// the same asset rather than a guid conflict.
    #[test]
    fn importing_a_named_asset_that_has_no_guid_is_not_a_conflict() {
        let res = ScratchRes::new("reimport-named-no-guid");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        let handle = assets
            .add_named(
                "a.note",
                Note {
                    name: "a.note".to_owned(),
                    text: "hello".to_owned(),
                    settings: settings(1, "initial"),
                },
            )
            .unwrap();
        assert_eq!(
            assets.guid_of(handle),
            None,
            "the fixture must carry no guid"
        );

        let outcome = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .expect("a path registered without a guid re-imports as itself");

        assert!(outcome.already_loaded);
        assert_eq!(outcome.handle, handle);
    }

    #[test]
    fn new_sources_get_different_guids() {
        let res = ScratchRes::new("distinct-guids");
        res.write("a.note", "a");
        res.write("b.note", "b");
        let mut assets = AssetManager::new();

        let a = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();
        let b = assets
            .import(request("b.note", MetadataPolicy::ReadIfPresent))
            .unwrap();

        assert_ne!(a.guid, b.guid);
    }

    #[test]
    fn a_removed_asset_imports_again_under_the_same_guid() {
        let res = ScratchRes::new("remove");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        let first = assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();
        assets.remove(first.handle);

        let second = assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();

        assert!(!second.already_loaded);
        assert_ne!(second.handle, first.handle);
        assert_eq!(second.guid, first.guid);
        assert_eq!(second.metadata, MetadataSource::ReadFromFile);
    }

    #[test]
    fn a_copied_metadata_file_is_a_duplicate_guid() {
        let res = ScratchRes::new("duplicate");
        res.write("a.note", "a");
        res.write("a.note.meta", &metadata_yaml(GUID_A, "{}"));
        res.write("copy.note", "a");
        res.write("copy.note.meta", &metadata_yaml(GUID_A, "{}"));
        let mut assets = AssetManager::new();
        let original = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();

        let result = assets.import(request("copy.note", MetadataPolicy::ReadIfPresent));

        assert!(matches!(
            result,
            Err(AssetImportError::DuplicateGuid { ref path, ref other_path, .. })
                if path == "copy.note" && other_path == "a.note"
        ));
        let guid = AssetGuid::parse(GUID_A).unwrap();
        assert_eq!(assets.handle_by_guid::<Note>(guid), Some(original.handle));
        assert_eq!(assets.len::<Note>(), 1);
    }

    #[test]
    fn a_metadata_guid_edited_while_loaded_is_a_conflict() {
        let res = ScratchRes::new("edited-guid");
        res.write("a.note", "hello");
        res.write("a.note.meta", &metadata_yaml(GUID_A, "{}"));
        let mut assets = AssetManager::new();
        assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();
        res.write("a.note.meta", &metadata_yaml(GUID_B, "{}"));

        let result = assets.import(request("a.note", MetadataPolicy::ReadIfPresent));

        assert!(matches!(
            result,
            Err(AssetImportError::PathBoundToOtherGuid { loaded: Some(loaded), metadata, .. })
                if loaded == AssetGuid::parse(GUID_A).unwrap() && metadata == AssetGuid::parse(GUID_B).unwrap()
        ));
    }

    /// Writers racing to create one metadata file (two hosts, parallel tests)
    /// all end up holding the guid of the file that won. None of them keeps a
    /// private guid that the next import would report as a conflict.
    #[test]
    fn racing_first_imports_agree_on_the_written_guid() {
        let res = ScratchRes::new("race");
        res.write("a.note", "hello");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        let guids: Vec<AssetGuid> = (0..8)
            .map(|_| {
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut assets = AssetManager::new();
                    barrier.wait();
                    let outcome = assets
                        .import(request("a.note", MetadataPolicy::CreateIfMissing))
                        .unwrap();
                    // A second import must accept the asset the first one made.
                    let again = assets
                        .import(request("a.note", MetadataPolicy::CreateIfMissing))
                        .unwrap();
                    assert!(again.already_loaded);
                    outcome.guid.expect("an imported source has a guid")
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().expect("no import panicked"))
            .collect();

        let written = res.read("a.note.meta").expect("one file was written");
        let on_disk = parse_metadata::<Note>(Path::new("a.note.meta"), written.as_bytes())
            .unwrap()
            .guid;
        assert!(
            guids.iter().all(|guid| *guid == on_disk),
            "{guids:?} vs {on_disk}"
        );
    }

    #[test]
    fn metadata_files_and_unnormalized_paths_are_refused() {
        let mut assets = AssetManager::new();
        assert!(matches!(
            assets.import(request("a.note.meta", MetadataPolicy::ReadIfPresent)),
            Err(AssetImportError::Load(AssetLoadError::Metadata { .. }))
        ));
        assert!(matches!(
            assets.import(request("../a.note", MetadataPolicy::ReadIfPresent)),
            Err(AssetImportError::Load(AssetLoadError::PathNotFound { .. }))
        ));
    }

    // -------------------------------------------------------------------------
    // Reimport
    // -------------------------------------------------------------------------

    /// An edited metadata file changes the loaded value in place: same handle,
    /// same guid, and the slot's content version moves.
    #[test]
    fn reimport_replaces_the_value_in_its_slot() {
        let res = ScratchRes::new("reimport-in-place");
        res.write("a.note", "hello");
        res.write("a.note.meta", &metadata_yaml(GUID_A, r#"{"volume": 2}"#));
        let mut assets = AssetManager::new();
        let first = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();
        let version_before = assets.content_version(first.handle).unwrap();

        res.write("a.note.meta", &metadata_yaml(GUID_A, r#"{"volume": 9}"#));
        let outcome = assets.reimport::<Note>(Path::new("a.note")).unwrap();

        assert!(outcome.replaced);
        assert_eq!(outcome.handle, first.handle);
        assert_eq!(outcome.guid, first.guid);
        assert_eq!(assets.get(first.handle).unwrap().settings.volume, 9);
        assert!(assets.content_version(first.handle).unwrap() > version_before);
        assert_eq!(assets.len::<Note>(), 1);
    }

    /// A source that no longer decodes leaves the previous value in place.
    #[test]
    fn reimport_of_a_broken_source_keeps_the_previous_value() {
        let res = ScratchRes::new("reimport-broken");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        let first = assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();
        let version_before = assets.content_version(first.handle).unwrap();

        res.write("a.note", "broken");
        let error = assets.reimport::<Note>(Path::new("a.note")).unwrap_err();

        assert!(matches!(
            error,
            AssetImportError::Load(AssetLoadError::Decode { .. })
        ));
        assert_eq!(assets.get(first.handle).unwrap().text, "hello");
        assert_eq!(
            assets.content_version(first.handle).unwrap(),
            version_before
        );
    }

    /// A metadata file whose guid was edited is refused, and the loaded asset
    /// keeps its guid and value.
    #[test]
    fn reimport_refuses_a_changed_guid() {
        let res = ScratchRes::new("reimport-guid");
        res.write("a.note", "hello");
        res.write("a.note.meta", &metadata_yaml(GUID_A, "{}"));
        let mut assets = AssetManager::new();
        let first = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();

        res.write("a.note.meta", &metadata_yaml(GUID_B, "{}"));
        let error = assets.reimport::<Note>(Path::new("a.note")).unwrap_err();

        assert!(matches!(
            error,
            AssetImportError::PathBoundToOtherGuid { .. }
        ));
        assert_eq!(assets.guid_of(first.handle), first.guid);
    }

    /// Reimporting a path that is not loaded imports it, and says so.
    #[test]
    fn reimport_of_an_unloaded_path_imports_it() {
        let res = ScratchRes::new("reimport-unloaded");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();

        let outcome = assets.reimport::<Note>(Path::new("a.note")).unwrap();

        assert!(!outcome.replaced);
        assert_eq!(assets.get(outcome.handle).unwrap().text, "hello");
        // Reimport never writes, even for a fresh import.
        assert!(res.read("a.note.meta").is_none());
    }

    // -------------------------------------------------------------------------
    // Ensuring a metadata file
    // -------------------------------------------------------------------------

    /// A source without metadata gets a file with default settings and a new
    /// guid, without being decoded. An existing file is left alone.
    #[test]
    fn ensure_metadata_writes_a_missing_file_without_decoding() {
        let res = ScratchRes::new("ensure");
        res.write("a.note", "hello");
        res.write("b.note", "hello");
        res.write("b.note.meta", &metadata_yaml(GUID_A, r#"{"volume": 9}"#));
        let assets = AssetManager::new();
        let decode_count_before = decode_count();

        let written = assets.ensure_metadata::<Note>(Path::new("a.note")).unwrap();
        let kept = assets.ensure_metadata::<Note>(Path::new("b.note")).unwrap();

        let guid = written.expect("a.note had no metadata");
        let file = res.read("a.note.meta").unwrap();
        assert!(file.contains(&guid.to_string()), "{file}");
        assert!(file.contains("volume: 7"), "the type's defaults: {file}");
        assert_eq!(kept, None);
        assert!(res.read("b.note.meta").unwrap().contains(GUID_A));
        assert_eq!(decode_count(), decode_count_before, "nothing was decoded");
        assert_eq!(
            assets.ensure_metadata::<Note>(Path::new("a.note")).unwrap(),
            None
        );
    }

    /// A loaded asset whose metadata file was deleted gets it back under the
    /// guid it is loaded with, so a reimport still matches.
    #[test]
    fn ensure_metadata_keeps_a_loaded_assets_guid() {
        let res = ScratchRes::new("ensure-loaded");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        let loaded = assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();
        std::fs::remove_file(res.root.join("a.note.meta")).unwrap();

        let written = assets.ensure_metadata::<Note>(Path::new("a.note")).unwrap();
        assert_eq!(written, loaded.guid);
        assert!(assets.reimport::<Note>(Path::new("a.note")).is_ok());
    }

    /// A missing metadata file is created with the settings the asset was
    /// decoded with, not the type's defaults. The file must not change how the
    /// asset decodes on the next run.
    #[test]
    fn ensure_metadata_writes_the_settings_the_asset_was_loaded_with() {
        let res = ScratchRes::new("ensure-loaded-settings");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        let loaded = assets
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();
        assert_eq!(loaded.metadata, MetadataSource::InMemoryOnly);

        let written = assets.ensure_metadata::<Note>(Path::new("a.note")).unwrap();
        assert_eq!(written, loaded.guid);
        let file = res.read("a.note.meta").expect("the sidecar was written");
        assert!(
            file.contains("volume: 1") && file.contains("initial"),
            "the settings the asset was loaded with: {file}"
        );
        let mut next = AssetManager::new();
        let again = next
            .import(request("a.note", MetadataPolicy::ReadIfPresent))
            .unwrap();
        assert_eq!(again.metadata, MetadataSource::ReadFromFile);
        assert_eq!(
            next.get(again.handle).map(|note| note.settings.clone()),
            Some(settings(1, "initial")),
            "the next run decodes what this one did"
        );
    }

    // -------------------------------------------------------------------------
    // Following a move
    // -------------------------------------------------------------------------

    /// A source moved together with its metadata file is renamed in place:
    /// same handle, same guid, no decode.
    #[test]
    fn a_moved_source_is_followed_under_its_guid() {
        let res = ScratchRes::new("follow-move");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        let first = assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();
        let decode_count_before = decode_count();

        std::fs::create_dir_all(res.root.join("moved")).unwrap();
        std::fs::rename(res.root.join("a.note"), res.root.join("moved/b.note")).unwrap();
        std::fs::rename(
            res.root.join("a.note.meta"),
            res.root.join("moved/b.note.meta"),
        )
        .unwrap();
        let old_name = assets
            .follow_move::<Note>(Path::new("moved/b.note"))
            .unwrap();

        assert_eq!(old_name.as_deref(), Some("a.note"));
        assert_eq!(
            assets.handle_by_name::<Note>("moved/b.note"),
            Some(first.handle)
        );
        assert_eq!(
            assets.handle_by_guid::<Note>(first.guid.expect("an imported source has a guid")),
            Some(first.handle)
        );
        assert_eq!(
            decode_count(),
            decode_count_before,
            "a move decodes nothing"
        );
    }

    /// A copy - the old source is still there - is not a move.
    #[test]
    fn a_copied_source_is_not_followed() {
        let res = ScratchRes::new("follow-copy");
        res.write("a.note", "hello");
        let mut assets = AssetManager::new();
        assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();
        res.write("b.note", "hello");
        std::fs::copy(res.root.join("a.note.meta"), res.root.join("b.note.meta")).unwrap();

        assert_eq!(
            assets.follow_move::<Note>(Path::new("b.note")).unwrap(),
            None
        );
        assert!(assets.handle_by_name::<Note>("a.note").is_some());
    }

    /// A path that is already loaded, one without metadata, and an unknown guid
    /// all report no move.
    #[test]
    fn unrelated_paths_are_not_moves() {
        let res = ScratchRes::new("follow-none");
        res.write("a.note", "hello");
        res.write("b.note", "hello");
        res.write("c.note", "hello");
        res.write("c.note.meta", &metadata_yaml(GUID_B, "{}"));
        let mut assets = AssetManager::new();
        assets
            .import(request("a.note", MetadataPolicy::CreateIfMissing))
            .unwrap();

        for path in ["a.note", "b.note", "c.note"] {
            assert_eq!(
                assets.follow_move::<Note>(Path::new(path)).unwrap(),
                None,
                "{path}"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Settings as JSON
    // -------------------------------------------------------------------------

    /// The index and the importer share one header contract.
    ///
    /// Both refuse a format the engine cannot read. Because they share the
    /// reader, changing the format version is one edit, and the two cannot
    /// disagree about whether a file is an asset.
    #[test]
    fn a_newer_format_is_refused_by_the_reader_and_the_parser() {
        let bytes = format!(
            r#"{{"format_version": {}, "asset_type": "Note", "guid": "00000000000000000000000000000001"}}"#,
            METADATA_FORMAT_VERSION + 1
        );
        assert!(read_header(bytes.as_bytes()).is_err());
        assert!(matches!(
            parse_header(
                Path::new("a.note.meta"),
                bytes.as_bytes(),
                "Note",
                "settings"
            ),
            Err(AssetLoadError::Metadata { .. })
        ));
    }

    #[test]
    fn settings_json_comes_from_the_metadata_file_or_the_defaults() {
        let res = ScratchRes::new("settings-json");
        res.write("a.note", "hello");
        res.write("b.note", "hello");
        res.write("a.note.meta", &metadata_yaml(GUID_A, r#"{"volume": 3}"#));

        let from_file = settings_json_for::<Note>(Path::new("a.note")).unwrap();
        let defaults = settings_json_for::<Note>(Path::new("b.note")).unwrap();

        assert_eq!(from_file["volume"], 3);
        assert_eq!(from_file["label"], "default");
        assert_eq!(defaults["volume"], 7);
    }

    #[test]
    fn normalizing_settings_fills_defaults_and_refuses_misfits() {
        let normalized =
            normalize_settings_json::<Note>(serde_json::json!({"volume": 4, "extra": true}))
                .unwrap();
        assert_eq!(
            normalized,
            serde_json::json!({"volume": 4, "label": "default"})
        );

        let misfit = normalize_settings_json::<Note>(serde_json::json!({"volume": "loud"}));
        assert!(matches!(misfit, Err(AssetLoadError::Metadata { .. })));
    }
}
