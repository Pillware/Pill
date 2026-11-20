//! Importing assets whose Rust type the caller does not know.
//!
//! # Responsibilities
//!
//! - Hold, per imported asset type, its metadata type name, the source
//!   extensions it accepts and type-erased entry points
//!   ([`ImportRegistry`]).
//! - Import, reimport and read or check the settings of a file by its
//!   extension or type name, for code that sees only paths: the `res` scan,
//!   the file watcher, an editor.
//! - Import every known source under a directory in one call
//!   ([`ImportRegistry::scan`]), reporting what it did.
//! - Hold standalone asset types too - files that are the asset, such as a
//!   `.material` - with the default document a new file of each starts from.
//! - Refuse a second type claiming an extension another type already has.
//! - Never call into a binary that is gone: entries whose registering
//!   generation was retired are skipped and pruned.
//!
//! # Design
//!
//! [`AssetManager::import`] is generic, so the type is fixed where it is called.
//! A caller holding only `textures/a.png` needs the step from an extension to a
//! type. That step is what [`World::register_imported_asset`] records: each
//! registration stores function pointers monomorphized for that type, in the
//! binary that registered it.
//!
//! Those pointers are code from the registering DLL. The DLL is unmapped some
//! time after a reload retires it, so each registration holds only a weak
//! reference to a marker resource and stays callable only while that marker
//! lives:
//!
//! - A reload's `init` registers again, which records fresh pointers into the
//!   new image and replaces the marker.
//! - A generation that stops registering a type, or a module that is unloaded,
//!   loses its claim on the marker. The host drops unclaimed resources while the
//!   image is still mapped, which ends the registration.
//!
//! Several binaries can register one type: a project calls
//! `pill_master_renderer_data::register` in its own `init`, and so does the data
//! crate's own module. Each keeps its own registration, and the newest live one
//! is used. A project that stops registering a type therefore falls back to the
//! module's registration instead of losing the type.
//!
//! [`World::register_imported_asset`]: crate::World::register_imported_asset

// Standard library
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

// External crates
use trait_type_map::TraitAccessible;

// Current crate
use crate::asset::{Asset, AssetGuid, AssetLoadError, AssetLoadResult, AssetManager, Handle};
use crate::asset_database::AssetDatabase;
use crate::asset_metadata::{
    normalize_settings_json, settings_json_for, AssetImport, AssetImportError, ImportedAsset,
    MetadataPolicy, MetadataSource, METADATA_EXTENSION,
};
use crate::asset_standalone::{
    default_standalone_document, normalize_standalone_document, standalone_document_json,
    StandaloneAsset,
};
use crate::asset_store::pack_key;
use crate::resource::Resource;

// =============================================================================
// Erased entry points
// =============================================================================

/// What an erased import or reimport did, without the typed handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasedImportOutcome {
    /// The asset type's metadata type name.
    pub type_name: String,
    /// The asset's name: its normalized path.
    pub name: String,
    /// The asset's guid, or `None` when it has none. That happens for a path
    /// that was already loaded and has no metadata file.
    pub guid: Option<AssetGuid>,
    /// The slot index of the asset's handle. Together with [`Self::generation`]
    /// it is the raw pair a typed caller turns back into a handle with
    /// [`Handle::from_raw`](crate::Handle::from_raw).
    pub index: u32,
    /// The generation of the asset's handle.
    pub generation: u32,
    /// Where the guid and settings came from.
    pub metadata: MetadataSource,
    /// What the call did with a value that was already there. A consumer that
    /// asks "did this change?" needs this, and a plain bool could not tell
    /// loading an existing value apart from replacing it.
    pub change: ImportChange,
    /// The asset's content version after the call.
    pub content_version: Option<u64>,
    /// The asset's content version before the call, when it was loaded. A
    /// reimport moves the version; an import of a path that was already loaded
    /// leaves it as it was.
    pub previous_content_version: Option<u64>,
}

impl ErasedImportOutcome {
    /// Whether the call found a value already there - either because the path
    /// was already loaded, or because it replaced one.
    pub fn previously_loaded(&self) -> bool {
        !matches!(self.change, ImportChange::Fresh)
    }

    /// The outcome of a call that imported a path, or found it loaded.
    ///
    /// The previous content version follows from the change: a path that was
    /// already loaded keeps the version it had, and a fresh import had none.
    fn from_import<T>(
        assets: &AssetManager,
        type_name: &str,
        handle: Handle<T>,
        guid: Option<AssetGuid>,
        metadata: MetadataSource,
        already_loaded: bool,
    ) -> Self
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let change = if already_loaded {
            ImportChange::AlreadyLoaded
        } else {
            ImportChange::Fresh
        };
        let content_version = assets.content_version(handle);
        Self {
            type_name: type_name.to_owned(),
            name: assets.name_of(handle).unwrap_or_default().to_owned(),
            guid,
            index: handle.index(),
            generation: handle.generation(),
            metadata,
            change,
            content_version,
            previous_content_version: content_version.filter(|_| already_loaded),
        }
    }

    /// The outcome of a call that replaced a loaded value.
    ///
    /// `previous_content_version` is read before the call, because the reimport
    /// is what moves it.
    fn from_reimport<T>(
        assets: &AssetManager,
        type_name: &str,
        handle: Handle<T>,
        guid: Option<AssetGuid>,
        metadata: MetadataSource,
        previous_content_version: Option<u64>,
    ) -> Self
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        Self {
            type_name: type_name.to_owned(),
            name: assets.name_of(handle).unwrap_or_default().to_owned(),
            guid,
            index: handle.index(),
            generation: handle.generation(),
            metadata,
            change: ImportChange::Replaced,
            content_version: assets.content_version(handle),
            previous_content_version,
        }
    }
}

/// What a call did with the value under a path, including the case where
/// nothing was there yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportChange {
    /// Nothing was there: the asset was decoded and added.
    Fresh,
    /// The path was already loaded and nothing was decoded.
    AlreadyLoaded,
    /// The loaded value was replaced in its slot.
    Replaced,
}

/// Import a path as one asset type.
type ImportFunction = fn(&mut AssetManager, &Path, MetadataPolicy) -> ErasedImportResult;
/// Decode a loaded path again, in place.
type ReimportFunction = fn(&mut AssetManager, &Path) -> ErasedImportResult;
/// The settings a path would use, as JSON.
type SettingsJsonFunction = fn(&Path) -> AssetLoadResult<serde_json::Value>;
/// Settings JSON checked against the type and normalized.
type NormalizeSettingsFunction = fn(serde_json::Value) -> AssetLoadResult<serde_json::Value>;
/// Whether an asset is loaded under a path's name.
type IsLoadedFunction = fn(&AssetManager, &Path) -> bool;
/// Drop the asset loaded under a path's name; `true` when one went.
type RemoveLoadedFunction = fn(&mut AssetManager, &Path) -> bool;
/// Follow a source that was moved together with its metadata file, and return
/// the name it had before the move, if any.
type FollowMoveFunction = fn(&mut AssetManager, &Path) -> Result<Option<String>, AssetImportError>;
/// A standalone type's default document, as JSON.
type DefaultDocumentFunction = fn() -> AssetLoadResult<serde_json::Value>;
/// Write a sourced asset's metadata file when it has none, and return the guid
/// it wrote, if any.
type EnsureMetadataFunction =
    fn(&AssetManager, &Path) -> Result<Option<AssetGuid>, AssetImportError>;
/// What a handle of the type addresses, from its index and generation.
type IdentityFunction = fn(&AssetManager, u32, u32) -> Option<AssetIdentity>;
/// The guid the store holds a path's asset under, when it is loaded.
type GuidByPathFunction = fn(&AssetManager, &Path) -> Option<AssetGuid>;
/// What the store holds for a guid: its name and the handle's raw parts.
type LoadedByGuidFunction = fn(&AssetManager, AssetGuid) -> Option<(String, u32, u32)>;
/// The handle the store holds an asset under a name, as its raw parts.
type LoadedByNameFunction = fn(&AssetManager, &str) -> Option<(u32, u32)>;
/// Every live asset of the type, as its guid and its name.
type LoadedFunction = fn(&AssetManager) -> Vec<(Option<AssetGuid>, String)>;

/// The live asset a handle addresses, as a tool shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetIdentity {
    /// The asset's guid, when it has one.
    pub guid: Option<AssetGuid>,
    /// The asset's name - its path relative to `res` for an imported one -
    /// when it has one.
    pub name: Option<String>,
}

/// The result of an erased import or reimport.
pub type ErasedImportResult = Result<ErasedImportOutcome, AssetImportError>;

/// One type's erased entry points, all monomorphized in the binary that
/// registered it.
#[derive(Clone, Copy)]
struct ImportFunctions {
    import: ImportFunction,
    reimport: ReimportFunction,
    settings_json: SettingsJsonFunction,
    normalize_settings: NormalizeSettingsFunction,
    follow_move: FollowMoveFunction,
    is_loaded: IsLoadedFunction,
    remove_loaded: RemoveLoadedFunction,
    identity: IdentityFunction,
    guid_by_path: GuidByPathFunction,
    loaded_by_guid: LoadedByGuidFunction,
    loaded_by_name: LoadedByNameFunction,
    loaded: LoadedFunction,
    /// `Some` for a standalone type, whose file is the asset; `None` for a
    /// sourced one.
    default_document: Option<DefaultDocumentFunction>,
    /// `Some` for a sourced type, whose metadata is a sidecar; `None` for a
    /// standalone one, which holds its guid itself.
    ensure_metadata: Option<EnsureMetadataFunction>,
}

impl ImportFunctions {
    /// The entry points for `T`, compiled into the calling binary.
    fn for_type<T: ImportedAsset>() -> Self {
        Self {
            import: import_erased::<T>,
            reimport: reimport_erased::<T>,
            settings_json: settings_json_for::<T>,
            normalize_settings: normalize_settings_json::<T>,
            follow_move: AssetManager::follow_move::<T>,
            is_loaded: is_loaded::<T>,
            remove_loaded: remove_loaded::<T>,
            identity: asset_identity_of::<T>,
            guid_by_path: guid_by_path::<T>,
            loaded_by_guid: loaded_by_guid_of::<T>,
            loaded_by_name: handle_by_name_of::<T>,
            loaded: loaded_assets_of::<T>,
            default_document: None,
            ensure_metadata: Some(AssetManager::ensure_metadata::<T>),
        }
    }

    /// The entry points for the standalone type `T`, compiled into the calling
    /// binary. For this kind the import settings are the asset's own document.
    fn for_standalone_type<T: StandaloneAsset>() -> Self {
        Self {
            import: import_standalone_erased::<T>,
            reimport: reimport_standalone_erased::<T>,
            settings_json: standalone_document_json::<T>,
            normalize_settings: normalize_standalone_document::<T>,
            follow_move: AssetManager::follow_standalone_move::<T>,
            is_loaded: is_loaded::<T>,
            remove_loaded: remove_loaded::<T>,
            identity: asset_identity_of::<T>,
            guid_by_path: guid_by_path::<T>,
            loaded_by_guid: loaded_by_guid_of::<T>,
            loaded_by_name: handle_by_name_of::<T>,
            loaded: loaded_assets_of::<T>,
            default_document: Some(default_standalone_document::<T>),
            ensure_metadata: None,
        }
    }
}

/// [`AssetManager::import_standalone`] for `T`. The metadata policy does not
/// apply here, because importing a standalone file never writes a file.
fn import_standalone_erased<T: StandaloneAsset>(
    assets: &mut AssetManager,
    path: &Path,
    _policy: MetadataPolicy,
) -> ErasedImportResult {
    let outcome = assets.import_standalone::<T>(path)?;
    Ok(ErasedImportOutcome::from_import::<T>(
        assets,
        T::metadata_type_name(),
        outcome.handle,
        outcome.guid,
        outcome.metadata,
        outcome.already_loaded,
    ))
}

/// [`AssetManager::reimport_standalone`] for `T`.
fn reimport_standalone_erased<T: StandaloneAsset>(
    assets: &mut AssetManager,
    path: &Path,
) -> ErasedImportResult {
    let previous_content_version = pack_key(path)
        .and_then(|name| assets.handle_by_name::<T>(&name))
        .and_then(|handle| assets.content_version(handle));
    let outcome = assets.reimport_standalone::<T>(path)?;
    Ok(ErasedImportOutcome::from_reimport::<T>(
        assets,
        T::metadata_type_name(),
        outcome.handle,
        outcome.guid,
        outcome.metadata,
        previous_content_version,
    ))
}

/// [`AssetManager::import`] for `T`, with the type's default initial settings.
fn import_erased<T: ImportedAsset>(
    assets: &mut AssetManager,
    path: &Path,
    policy: MetadataPolicy,
) -> ErasedImportResult {
    let outcome = assets.import(AssetImport::<T>::new(path, policy))?;
    Ok(ErasedImportOutcome::from_import::<T>(
        assets,
        T::metadata_type_name(),
        outcome.handle,
        outcome.guid,
        outcome.metadata,
        outcome.already_loaded,
    ))
}

/// Whether a `T` is loaded under `path`'s normalized name.
fn is_loaded<T>(assets: &AssetManager, path: &Path) -> bool
where
    T: crate::asset::Asset + trait_type_map::TraitAccessible<dyn crate::asset::Asset>,
{
    pack_key(path).is_some_and(|name| assets.handle_by_name::<T>(&name).is_some())
}

/// Drop the `T` loaded under `path`'s normalized name, if any.
fn remove_loaded<T>(assets: &mut AssetManager, path: &Path) -> bool
where
    T: crate::asset::Asset + trait_type_map::TraitAccessible<dyn crate::asset::Asset>,
{
    let Some(handle) = pack_key(path).and_then(|name| assets.handle_by_name::<T>(&name)) else {
        return false;
    };
    assets.remove(handle).is_some()
}

/// The live asset a `T` handle with `index` and `generation` addresses, or
/// `None` when the handle is stale.
fn asset_identity_of<T>(assets: &AssetManager, index: u32, generation: u32) -> Option<AssetIdentity>
where
    T: Asset + TraitAccessible<dyn Asset>,
{
    let handle = Handle::<T>::from_raw(index, generation);
    assets.get(handle)?;
    Some(AssetIdentity {
        guid: assets.guid_of(handle),
        name: assets.name_of(handle).map(str::to_owned),
    })
}

/// The guid a `T` is loaded under, addressed by its path's normalized name.
fn guid_by_path<T>(assets: &AssetManager, path: &Path) -> Option<AssetGuid>
where
    T: Asset + TraitAccessible<dyn Asset>,
{
    let handle = assets.handle_by_name::<T>(&pack_key(path)?)?;
    assets.guid_of(handle)
}

/// The name a `T` with `guid` is loaded under, with the handle's raw parts.
fn loaded_by_guid_of<T>(assets: &AssetManager, guid: AssetGuid) -> Option<(String, u32, u32)>
where
    T: Asset + TraitAccessible<dyn Asset>,
{
    let handle = assets.handle_by_guid::<T>(guid)?;
    Some((
        assets.name_of(handle).unwrap_or_default().to_owned(),
        handle.index(),
        handle.generation(),
    ))
}

/// The raw parts of the handle the store holds `T` under `name`, if any.
fn handle_by_name_of<T>(assets: &AssetManager, name: &str) -> Option<(u32, u32)>
where
    T: Asset + TraitAccessible<dyn Asset>,
{
    assets
        .handle_by_name::<T>(name)
        .map(|handle| (handle.index(), handle.generation()))
}

/// Every live `T` in the store, as its guid and its name.
fn loaded_assets_of<T>(assets: &AssetManager) -> Vec<(Option<AssetGuid>, String)>
where
    T: Asset + TraitAccessible<dyn Asset>,
{
    assets
        .iter_handles::<T>()
        .map(|(handle, _)| {
            (
                assets.guid_of(handle),
                assets.name_of(handle).unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// [`AssetManager::reimport`] for `T`.
fn reimport_erased<T: ImportedAsset>(assets: &mut AssetManager, path: &Path) -> ErasedImportResult {
    let previous_content_version = pack_key(path)
        .and_then(|name| assets.handle_by_name::<T>(&name))
        .and_then(|handle| assets.content_version(handle));
    let outcome = assets.reimport::<T>(path)?;
    Ok(ErasedImportOutcome::from_reimport::<T>(
        assets,
        T::metadata_type_name(),
        outcome.handle,
        outcome.guid,
        outcome.metadata,
        previous_content_version,
    ))
}

// =============================================================================
// Errors
// =============================================================================

/// Why an erased import, reimport or settings call did not run.
#[derive(Debug, thiserror::Error)]
pub enum ErasedImportError {
    /// No live registration claims the path's extension.
    #[error("no imported asset type is registered for `.{extension}` (`{path}`)")]
    UnknownExtension {
        /// The path asked for.
        path: String,
        /// Its extension, lowercased.
        extension: String,
    },
    /// The path has no extension to look up.
    #[error("`{path}` has no extension, so no asset type can be chosen for it")]
    NoExtension {
        /// The path asked for.
        path: String,
    },
    /// The path is a metadata file, which is never an asset itself.
    #[error("`{path}` is a metadata file, not an asset source")]
    MetadataFile {
        /// The path asked for.
        path: String,
    },
    /// No live registration has this type name.
    #[error("no imported asset type named `{type_name}` is registered")]
    UnknownType {
        /// The type name asked for.
        type_name: String,
    },
    /// No asset of the project has this guid.
    #[error("no asset in the project has the guid {guid}")]
    UnknownGuid {
        /// The guid asked for.
        guid: AssetGuid,
    },
    /// The asset is of another type than the one asked for.
    #[error("the asset{} is a `{found}`, not a `{expected}`", guid_clause(.guid))]
    WrongType {
        /// The guid asked for, when the caller named one: a path-based load
        /// only knows it when the asset has one.
        guid: Option<AssetGuid>,
        /// The type the caller asked for.
        expected: String,
        /// The type the asset is.
        found: String,
    },
    /// The import itself failed.
    #[error(transparent)]
    Import(#[from] AssetImportError),
    /// Reading or checking settings failed.
    #[error(transparent)]
    Load(#[from] AssetLoadError),
}

/// Why [`ImportRegistry::register`] refused a type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportRegistrationError {
    /// Another live type already imports files with this extension.
    #[error(
        "`{incoming_type}` claims source extension `.{extension}`, which `{registered_type}` already imports"
    )]
    ExtensionClaimedTwice {
        /// The contested extension, lowercased.
        extension: String,
        /// The type that has it.
        registered_type: String,
        /// The type that asked for it.
        incoming_type: String,
    },
}

/// Builds the ` with guid <guid>` part of a wrong-type message. It is empty
/// when there is no guid, so the message names one only when the caller's
/// answer carries one.
fn guid_clause(guid: &Option<AssetGuid>) -> String {
    guid.map_or_else(String::new, |guid| format!(" with guid {guid}"))
}

// =============================================================================
// ImportRegistry
// =============================================================================

/// One binary's registration of a type.
#[derive(Clone)]
struct Registration {
    /// A weak reference that stays valid while the registering generation's
    /// marker resource lives.
    alive: Weak<()>,
    /// The extensions that binary's copy of the type declared.
    extensions: Vec<String>,
    /// Entry points into that binary.
    functions: ImportFunctions,
}

impl Registration {
    fn is_alive(&self) -> bool {
        self.alive.strong_count() > 0
    }
}

/// One imported asset type and every live registration of it, newest last.
#[derive(Clone)]
struct ImportedTypeEntry {
    /// The type's metadata type name, owned: a `&'static str` would point
    /// into the registering binary.
    type_name: String,
    registrations: Vec<Registration>,
}

impl ImportedTypeEntry {
    /// The newest registration still alive, which is the one to call.
    fn current(&self) -> Option<&Registration> {
        self.registrations
            .iter()
            .rev()
            .find(|registration| registration.is_alive())
    }
}

/// How one field of an asset's settings document draws in the Inspector.
///
/// Plain data on purpose: a declaration crosses the registry, which outlives
/// the crate that made it, so it holds no function pointer and no borrow. The
/// editor turns a decorator into a widget; anything that does not read it draws
/// the row exactly as it would without one.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldDecorator {
    /// The value is a number between `min` and `max`, so the row draws as a
    /// slider.
    Range {
        /// The smallest value the row offers.
        min: f64,
        /// The largest value the row offers.
        max: f64,
    },
    /// The row's label carries this explanation as hover text.
    Tooltip(String),
}

/// The imported asset types of a world, by metadata type name and source
/// extension.
///
/// Filled by [`World::register_imported_asset`](crate::World::register_imported_asset)
/// when a module or project registers its types. The module docs explain why an
/// entry cannot outlive the binary it points into, nor that binary's claim on
/// it.
///
/// Cloning is cheap: the type holds a few entries of weak references and
/// function pointers. Host code clones it to call it against the world's
/// [`AssetManager`](crate::AssetManager), which is another resource of the
/// same world. A clone keeps the same rule: it skips a registration that is
/// retired after the clone was taken.
#[derive(Default, Clone)]
pub struct ImportRegistry {
    /// One entry per imported asset type, in the order the type was first
    /// registered.
    entries: Vec<ImportedTypeEntry>,
    /// The reference fields each type declares, by owner type name. Each pair
    /// is the field key a settings document uses and the type name it points
    /// at. A key that ends in `.*` covers a whole family: `textures.*` is every
    /// texture slot the asset's shader declares.
    references: std::collections::BTreeMap<String, Vec<(String, String)>>,
    /// The field decorators each type declares, by owner type name. Each pair
    /// is the document key an edit writes and how its row draws.
    decorators: std::collections::BTreeMap<String, Vec<(String, FieldDecorator)>>,
}

impl Resource for ImportRegistry {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }
}

impl ImportRegistry {
    /// Record `T`'s entry points. They stay callable for as long as `alive`
    /// has a strong reference.
    ///
    /// Registering a type again (a reload, or a second binary) adds a newer
    /// registration that takes precedence. The older one is dropped by the next
    /// prune once it is no longer alive.
    ///
    /// # Errors
    ///
    /// [`ImportRegistrationError::ExtensionClaimedTwice`] when another live
    /// type already imports one of `T`'s extensions. Nothing is recorded then.
    pub fn register<T: ImportedAsset>(
        &mut self,
        alive: Weak<()>,
    ) -> Result<(), ImportRegistrationError> {
        self.record(
            T::metadata_type_name(),
            T::SOURCE_EXTENSIONS,
            ImportFunctions::for_type::<T>(),
            alive,
        )
    }

    /// Record the standalone type `T` under its file extension; otherwise as
    /// [`Self::register`].
    ///
    /// # Errors
    ///
    /// As [`Self::register`].
    pub fn register_standalone<T: StandaloneAsset>(
        &mut self,
        alive: Weak<()>,
    ) -> Result<(), ImportRegistrationError> {
        self.record(
            T::metadata_type_name(),
            &[T::FILE_EXTENSION],
            ImportFunctions::for_standalone_type::<T>(),
            alive,
        )
    }

    /// The registration both kinds share: the extension check, then the entry.
    fn record(
        &mut self,
        type_name: &str,
        extensions: &[&str],
        functions: ImportFunctions,
        alive: Weak<()>,
    ) -> Result<(), ImportRegistrationError> {
        self.prune();
        // Step 1: Normalize the extensions and refuse a clash with a live type.
        // Two types sharing an extension would make a file's type depend on
        // registration order, so the error names both.
        let extensions: Vec<String> = extensions
            .iter()
            .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
            .collect();
        for extension in &extensions {
            if let Some(other) = self.type_for_extension(extension) {
                if other != type_name {
                    return Err(ImportRegistrationError::ExtensionClaimedTwice {
                        extension: extension.clone(),
                        registered_type: other.to_owned(),
                        incoming_type: type_name.to_owned(),
                    });
                }
            }
        }

        // Step 2: Add the registration to the type's entry, creating the entry
        // when the type is registered for the first time.
        let registration = Registration {
            alive,
            extensions,
            functions,
        };
        match self
            .entries
            .iter_mut()
            .find(|entry| entry.type_name == type_name)
        {
            Some(entry) => entry.registrations.push(registration),
            None => self.entries.push(ImportedTypeEntry {
                type_name: type_name.to_owned(),
                registrations: vec![registration],
            }),
        }
        Ok(())
    }

    /// Drop every registration whose generation is gone, and every entry left
    /// with none.
    pub fn prune(&mut self) {
        for entry in &mut self.entries {
            entry.registrations.retain(Registration::is_alive);
        }
        self.entries.retain(|entry| !entry.registrations.is_empty());
        // A reference declaration belongs to the type that made it. When no
        // generation registers a type any more, its declarations are dropped
        // too.
        let live: std::collections::BTreeSet<String> = self
            .entries
            .iter()
            .map(|entry| entry.type_name.clone())
            .collect();
        self.references.retain(|owner, _| live.contains(owner));
        self.decorators.retain(|owner, _| live.contains(owner));
    }

    /// Record how `owner_type`'s document fields draw, as `(document key,
    /// decorator)` pairs.
    ///
    /// This is how a settings asset states that a value is a range or carries
    /// a tooltip, without the Inspector knowing the type: the manager's crate
    /// declares it where the document is defined, and the editor reads it back
    /// through [`Self::field_decorators`].
    pub fn declare_field_decorators(
        &mut self,
        owner_type: &str,
        decorators: &[(&str, FieldDecorator)],
    ) {
        let declared: Vec<(String, FieldDecorator)> = decorators
            .iter()
            .map(|(field, decorator)| ((*field).to_owned(), decorator.clone()))
            .collect();
        self.decorators.insert(owner_type.to_owned(), declared);
    }

    /// The decorators `owner_type` declares, as `(document key, decorator)`
    /// pairs in declaration order.
    pub fn field_decorators(&self, owner_type: &str) -> &[(String, FieldDecorator)] {
        self.decorators
            .get(owner_type)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The decorator declared for one field of `owner_type`, if any.
    pub fn field_decorator(&self, owner_type: &str, field: &str) -> Option<&FieldDecorator> {
        self.decorators
            .get(owner_type)?
            .iter()
            .find_map(|(key, decorator)| (key == field).then_some(decorator))
    }

    /// Record the reference fields `owner_type` declares, as `(field key,
    /// target type name)` pairs.
    ///
    /// The target is the Rust type the field holds, named by
    /// [`ImportedAsset::metadata_type_name`] or
    /// [`StandaloneAsset::metadata_type_name`]. Because the name comes from the
    /// code, a renamed type changes here with it instead of drifting away.
    /// Declaring again replaces the owner's whole list, which is what a reload
    /// does.
    pub fn declare_references(&mut self, owner_type: &str, references: &[(&str, &str)]) {
        let declared: Vec<(String, String)> = references
            .iter()
            .map(|(field, target)| ((*field).to_owned(), (*target).to_owned()))
            .collect();
        self.references.insert(owner_type.to_owned(), declared);
    }

    /// The reference fields `owner_type` declares, as `(field key, target type
    /// name)` pairs in declaration order. A key that ends in `.*` names a
    /// family; a caller turns it into the concrete keys its own document uses.
    pub fn reference_fields(&self, owner_type: &str) -> &[(String, String)] {
        self.references
            .get(owner_type)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The type name the reference field `field` of `owner_type` points at.
    ///
    /// An exact declaration wins; otherwise a family declaration (`textures.*`)
    /// covers every key below its prefix.
    pub fn reference_target(&self, owner_type: &str, field: &str) -> Option<&str> {
        let declared = self.references.get(owner_type)?;
        if let Some((_, target)) = declared.iter().find(|(key, _)| key == field) {
            return Some(target.as_str());
        }
        declared
            .iter()
            .filter_map(|(key, target)| {
                let family = key.strip_suffix(".*")?;
                (field == family || field.starts_with(&format!("{family}.")))
                    .then_some(target.as_str())
            })
            .next()
    }

    /// The type that imports `extension` (without the dot, any case), if a
    /// live registration claims it.
    pub fn type_for_extension(&self, extension: &str) -> Option<&str> {
        let extension = extension.trim_start_matches('.').to_ascii_lowercase();
        self.entries.iter().find_map(|entry| {
            let current = entry.current()?;
            current
                .extensions
                .contains(&extension)
                .then_some(entry.type_name.as_str())
        })
    }

    /// The live types and the extensions each imports, sorted by type name.
    pub fn registered_types(&self) -> Vec<(String, Vec<String>)> {
        let mut types: Vec<(String, Vec<String>)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let current = entry.current()?;
                Some((entry.type_name.clone(), current.extensions.clone()))
            })
            .collect();
        types.sort();
        types
    }

    /// The live standalone types - those whose file is the asset - with the
    /// one extension each is stored under, sorted by type name.
    pub fn standalone_types(&self) -> Vec<(String, String)> {
        let mut types: Vec<(String, String)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let current = entry.current()?;
                current.functions.default_document?;
                Some((entry.type_name.clone(), current.extensions.first()?.clone()))
            })
            .collect();
        types.sort();
        types
    }

    /// Whether files with `extension` are standalone assets of a live type.
    pub fn is_standalone_extension(&self, extension: &str) -> bool {
        let extension = extension.trim_start_matches('.').to_ascii_lowercase();
        self.entries.iter().any(|entry| {
            entry.current().is_some_and(|current| {
                current.functions.default_document.is_some()
                    && current.extensions.contains(&extension)
            })
        })
    }

    /// The document a new file of the standalone type `type_name` starts
    /// from, as JSON.
    ///
    /// # Errors
    ///
    /// [`ErasedImportError::UnknownType`] when no live standalone type has
    /// that name.
    pub fn default_document(
        &self,
        type_name: &str,
    ) -> Result<serde_json::Value, ErasedImportError> {
        let functions = self.functions_for_type(type_name)?;
        let default_document =
            functions
                .default_document
                .ok_or_else(|| ErasedImportError::UnknownType {
                    type_name: type_name.to_owned(),
                })?;
        Ok(default_document()?)
    }

    /// Whether `path` is a metadata file rather than a source.
    pub fn is_metadata_file(path: &Path) -> bool {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(METADATA_EXTENSION))
    }

    /// The entry points for the type that imports `path`'s extension.
    fn functions_for_path(&self, path: &Path) -> Result<ImportFunctions, ErasedImportError> {
        let path_text = path.display().to_string();
        if Self::is_metadata_file(path) {
            return Err(ErasedImportError::MetadataFile { path: path_text });
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| ErasedImportError::NoExtension {
                path: path_text.clone(),
            })?
            .to_ascii_lowercase();
        self.entries
            .iter()
            .filter_map(ImportedTypeEntry::current)
            .find(|registration| registration.extensions.contains(&extension))
            .map(|registration| registration.functions)
            .ok_or(ErasedImportError::UnknownExtension {
                path: path_text,
                extension,
            })
    }

    /// The entry points registered under `type_name`.
    fn functions_for_type(&self, type_name: &str) -> Result<ImportFunctions, ErasedImportError> {
        self.entries
            .iter()
            .find(|entry| entry.type_name == type_name)
            .and_then(ImportedTypeEntry::current)
            .map(|registration| registration.functions)
            .ok_or_else(|| ErasedImportError::UnknownType {
                type_name: type_name.to_owned(),
            })
    }

    /// Import `path` as the type its extension maps to, with that type's
    /// default initial settings; see [`AssetManager::import`].
    ///
    /// # Errors
    ///
    /// [`ErasedImportError::UnknownExtension`], [`ErasedImportError::NoExtension`]
    /// or [`ErasedImportError::MetadataFile`] when no type can be chosen, and
    /// [`ErasedImportError::Import`] when the import fails.
    pub fn import(
        &self,
        assets: &mut AssetManager,
        path: &Path,
        policy: MetadataPolicy,
    ) -> Result<ErasedImportOutcome, ErasedImportError> {
        let functions = self.functions_for_path(path)?;
        Ok((functions.import)(assets, path, policy)?)
    }

    /// What the handle `index`/`generation` of the asset type `type_name`
    /// addresses - its guid and name - for a tool that holds only a field
    /// value; `None` when the type is not registered or the handle is stale.
    pub fn asset_identity(
        &self,
        assets: &AssetManager,
        type_name: &str,
        index: u32,
        generation: u32,
    ) -> Option<AssetIdentity> {
        let functions = self.functions_for_type(type_name).ok()?;
        (functions.identity)(assets, index, generation)
    }

    /// The guid the store holds the asset at `path` under, when a live type of
    /// this registry imports its extension and it is loaded.
    ///
    /// A caller replacing a file asks this before writing: the identity the
    /// store already has is the one every reference names, and a file that
    /// disagrees with it cannot be imported at the same path.
    pub fn guid_by_path(&self, assets: &AssetManager, path: &Path) -> Option<AssetGuid> {
        let functions = self.functions_for_path(path).ok()?;
        (functions.guid_by_path)(assets, path)
    }

    /// The name the store holds the asset with `guid` under, with the type
    /// that owns it and the handle's raw parts; `None` when no live type has
    /// it loaded.
    ///
    /// An asset built in code - a chain's built-in pass, for instance - has no
    /// file for a panel to read, so this is what turns the guid a document
    /// stored back into a name to show and a handle to record.
    pub fn loaded_by_guid(
        &self,
        assets: &AssetManager,
        guid: AssetGuid,
    ) -> Option<(String, String, u32, u32)> {
        self.entries.iter().find_map(|entry| {
            let registration = entry.current()?;
            (registration.functions.loaded_by_guid)(assets, guid)
                .map(|(name, index, generation)| (entry.type_name.clone(), name, index, generation))
        })
    }

    /// The type the store holds an asset named `name` as, with the handle's raw
    /// parts; `None` when no live type has it loaded.
    ///
    /// An asset built in code - a chain's built-in pass - has no file for a
    /// tool to name and may have no guid, so its name in the store is the one
    /// identity it always has.
    pub fn loaded_by_name(&self, assets: &AssetManager, name: &str) -> Option<(String, u32, u32)> {
        self.entries.iter().find_map(|entry| {
            let registration = entry.current()?;
            (registration.functions.loaded_by_name)(assets, name)
                .map(|(index, generation)| (entry.type_name.clone(), index, generation))
        })
    }

    /// Every live asset every type of this registry holds, as
    /// `(type_name, guid, name)`.
    ///
    /// A directory walk sees files: a chain's built-in passes are built in
    /// code and appear in none, so this is the only place a picker learns they
    /// exist.
    pub fn loaded_assets(&self, assets: &AssetManager) -> Vec<(String, Option<AssetGuid>, String)> {
        let mut loaded = Vec::new();
        for entry in &self.entries {
            let Some(registration) = entry.current() else {
                continue;
            };
            for (guid, name) in (registration.functions.loaded)(assets) {
                loaded.push((entry.type_name.clone(), guid, name));
            }
        }
        loaded
    }

    /// Load the asset at `path`, whether or not it is loaded yet: the same
    /// call as [`Self::import`], named for callers that resolve a reference.
    ///
    /// # Errors
    ///
    /// As [`Self::import`].
    pub fn load_by_path(
        &self,
        assets: &mut AssetManager,
        path: &Path,
        policy: MetadataPolicy,
    ) -> Result<ErasedImportOutcome, ErasedImportError> {
        self.import(assets, path, policy)
    }

    /// Load the asset with `guid`, whether or not it is loaded yet: the guid's
    /// path comes from `database`, then the asset imports as [`Self::import`]
    /// would.
    ///
    /// # Errors
    ///
    /// [`ErasedImportError::UnknownGuid`] when `database` has no asset with
    /// the guid; otherwise as [`Self::import`].
    pub fn load_by_guid(
        &self,
        assets: &mut AssetManager,
        database: &AssetDatabase,
        guid: AssetGuid,
        policy: MetadataPolicy,
    ) -> Result<ErasedImportOutcome, ErasedImportError> {
        let record = database
            .by_guid(guid)
            .ok_or(ErasedImportError::UnknownGuid { guid })?;
        self.import(assets, Path::new(&record.path), policy)
    }

    /// [`Self::load_by_guid`] for a caller that knows the asset's type, which
    /// gets the typed handle back.
    ///
    /// # Errors
    ///
    /// [`ErasedImportError::WrongType`] when the guid names an asset of
    /// another type; otherwise as [`Self::load_by_guid`].
    pub fn load_handle_by_guid<T>(
        &self,
        assets: &mut AssetManager,
        database: &AssetDatabase,
        guid: AssetGuid,
        policy: MetadataPolicy,
    ) -> Result<Handle<T>, ErasedImportError>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let outcome = self.load_by_guid(assets, database, guid, policy)?;
        assets
            .handle_by_guid::<T>(guid)
            .ok_or_else(|| ErasedImportError::WrongType {
                guid: Some(guid),
                expected: std::any::type_name::<T>().to_owned(),
                found: outcome.type_name,
            })
    }

    /// [`Self::load_by_path`] for a caller that knows the asset's type, which
    /// gets the typed handle back.
    ///
    /// The game-code shape of loading an asset by name, recorded with its
    /// guid the way any import is: `world.load_asset::<Mesh>("models/x.obj")`
    /// wraps it.
    ///
    /// # Errors
    ///
    /// [`ErasedImportError::WrongType`] when the extension names an asset of
    /// another type; otherwise as [`Self::load_by_path`].
    pub fn load_handle_by_path<T>(
        &self,
        assets: &mut AssetManager,
        path: &Path,
        policy: MetadataPolicy,
    ) -> Result<Handle<T>, ErasedImportError>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let outcome = self.load_by_path(assets, path, policy)?;
        assets
            .handle_by_name::<T>(&outcome.name)
            .ok_or_else(|| ErasedImportError::WrongType {
                guid: outcome.guid,
                expected: std::any::type_name::<T>().to_owned(),
                found: outcome.type_name,
            })
    }

    /// Decode `path` again in place as the type its extension maps to; see
    /// [`AssetManager::reimport`].
    ///
    /// # Errors
    ///
    /// As [`Self::import`].
    pub fn reimport(
        &self,
        assets: &mut AssetManager,
        path: &Path,
    ) -> Result<ErasedImportOutcome, ErasedImportError> {
        let functions = self.functions_for_path(path)?;
        Ok((functions.reimport)(assets, path)?)
    }

    /// Write the metadata file of the source at `path` when it has none; see
    /// [`AssetManager::ensure_metadata`]. Returns the guid written, or `None`
    /// when nothing was written - the file exists, or `path` is a standalone
    /// asset, which has no sidecar.
    ///
    /// # Errors
    ///
    /// As [`Self::import`].
    pub fn ensure_metadata(
        &self,
        assets: &AssetManager,
        path: &Path,
    ) -> Result<Option<AssetGuid>, ErasedImportError> {
        let functions = self.functions_for_path(path)?;
        match functions.ensure_metadata {
            Some(ensure) => Ok(ensure(assets, path)?),
            None => Ok(None),
        }
    }

    /// Give every source under `root` that a registered sourced type imports
    /// a metadata file, where it has none. Nothing is decoded or loaded.
    /// Returns the files written (as their sources' paths) and the failures.
    pub fn ensure_all_metadata(
        &self,
        assets: &AssetManager,
        root: &Path,
    ) -> (Vec<String>, Vec<(String, String)>) {
        let mut written = Vec::new();
        let mut failed = Vec::new();
        for relative in source_files(root) {
            let name = relative.to_string_lossy().replace('\\', "/");
            match self.ensure_metadata(assets, &relative) {
                Ok(Some(_)) => written.push(name),
                Ok(None)
                | Err(ErasedImportError::UnknownExtension { .. })
                | Err(ErasedImportError::NoExtension { .. }) => {}
                Err(error) => failed.push((name, error.to_string())),
            }
        }
        (written, failed)
    }

    /// Whether an asset of the type `path`'s extension maps to is loaded under
    /// `path`'s name. `false` for a path no registered type imports.
    pub fn is_loaded(&self, assets: &AssetManager, path: &Path) -> bool {
        self.functions_for_path(path)
            .is_ok_and(|functions| (functions.is_loaded)(assets, path))
    }

    /// Drop the asset loaded under `path`'s name, if there is one. A delete
    /// calls this once the file is gone, so a new file at the same path does
    /// not collide with the old asset's guid. Returns `false` when no asset was
    /// loaded.
    pub fn remove_loaded(&self, assets: &mut AssetManager, path: &Path) -> bool {
        self.functions_for_path(path)
            .is_ok_and(|functions| (functions.remove_loaded)(assets, path))
    }

    /// When `path` is a loaded asset moved here with its metadata file, rename
    /// it to `path` and return its old name; see
    /// [`AssetManager::follow_move`].
    ///
    /// # Errors
    ///
    /// As [`Self::import`].
    pub fn follow_move(
        &self,
        assets: &mut AssetManager,
        path: &Path,
    ) -> Result<Option<String>, ErasedImportError> {
        let functions = self.functions_for_path(path)?;
        Ok((functions.follow_move)(assets, path)?)
    }

    /// The import settings `path` would use, as JSON: its metadata file's, or
    /// its type's defaults.
    ///
    /// # Errors
    ///
    /// When no type is chosen for the path, or its metadata file exists but
    /// does not read as that type's.
    pub fn settings_json(&self, path: &Path) -> Result<serde_json::Value, ErasedImportError> {
        let functions = self.functions_for_path(path)?;
        Ok((functions.settings_json)(path)?)
    }

    /// `settings` read as `type_name`'s import settings and written back:
    /// missing fields defaulted, unknown ones dropped.
    ///
    /// # Errors
    ///
    /// When the type is not registered, or the settings do not fit it.
    pub fn normalize_settings_json(
        &self,
        type_name: &str,
        settings: serde_json::Value,
    ) -> Result<serde_json::Value, ErasedImportError> {
        let functions = self.functions_for_type(type_name)?;
        Ok((functions.normalize_settings)(settings)?)
    }
}

// =============================================================================
// Scanning a directory
// =============================================================================

/// What [`ImportRegistry::scan`] did, file by file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanReport {
    /// Assets decoded by this scan, by name.
    pub imported: Vec<String>,
    /// Assets that were already loaded; nothing was decoded for them.
    pub already_loaded: Vec<String>,
    /// The subset of [`Self::imported`] whose metadata file this scan wrote.
    pub metadata_created: Vec<String>,
    /// Sources that failed, by name, with the error.
    pub failed: Vec<(String, String)>,
    /// Extensions no registered type imports, each listed once, sorted; an
    /// empty string stands for files with no extension.
    pub unknown_extensions: Vec<String>,
}

impl ImportRegistry {
    /// Import every source file under `root` whose extension a registered
    /// type imports.
    ///
    /// `root` is the directory asset paths are relative to - a project's `res` -
    /// and each file is imported by its path relative to it, exactly as loading
    /// code names it. Metadata files are skipped (they are read with their
    /// source), as are dot-files such as the temporary files a metadata write
    /// leaves behind briefly. A file of an unknown extension is not an error:
    /// the extension is reported once in the result, however many files carry
    /// it.
    ///
    /// A second scan of the same directory decodes and writes nothing: every
    /// path is already loaded, and every file it would write exists.
    pub fn scan(
        &self,
        assets: &mut AssetManager,
        root: &Path,
        policy: MetadataPolicy,
    ) -> ScanReport {
        let mut report = ScanReport::default();
        let mut unknown = BTreeSet::new();
        // Step 1: Split the files into sourced and standalone ones. Standalone
        // files are imported last, because they refer to other assets by guid
        // and a reference to an asset that is not loaded yet resolves to
        // nothing.
        let (standalone, sourced): (Vec<PathBuf>, Vec<PathBuf>) =
            source_files(root).into_iter().partition(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| self.is_standalone_extension(extension))
            });
        // Step 2: Import each file and record it by what happened to it:
        // imported, already loaded, or failed.
        for relative in sourced.into_iter().chain(standalone) {
            let name = relative.to_string_lossy().replace('\\', "/");
            match self.import(assets, &relative, policy) {
                Ok(outcome) if outcome.previously_loaded() => report.already_loaded.push(name),
                Ok(outcome) => {
                    if outcome.metadata == MetadataSource::CreatedOnDisk {
                        report.metadata_created.push(name.clone());
                    }
                    report.imported.push(name);
                }
                Err(ErasedImportError::UnknownExtension { extension, .. }) => {
                    unknown.insert(extension);
                }
                Err(ErasedImportError::NoExtension { .. }) => {
                    unknown.insert(String::new());
                }
                Err(error) => report.failed.push((name, error.to_string())),
            }
        }
        report.unknown_extensions = unknown.into_iter().collect();
        report
    }
}

/// Every file under `root`, as paths relative to it, sorted; metadata files
/// and dot-files left out. An unreadable directory contributes nothing.
fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative_directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&relative_directory)) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            if file_name.to_string_lossy().starts_with('.') {
                continue;
            }
            let relative = relative_directory.join(&file_name);
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(relative),
                Ok(kind) if kind.is_file() && !ImportRegistry::is_metadata_file(&relative) => {
                    files.push(relative);
                }
                _ => {}
            }
        }
    }
    files.sort();
    files
}

// =============================================================================
// The liveness marker
// =============================================================================

/// The resource whose lifetime is one generation's registration of `T`.
///
/// Inserted by [`World::register_imported_asset`](crate::World::register_imported_asset),
/// so it is claimed by the registering generation like any resource it
/// registers. The registry holds only a weak reference to `alive`.
pub(crate) struct ImportRegistrationMarker<T> {
    alive: Arc<()>,
    _type: std::marker::PhantomData<fn() -> T>,
}

impl<T> ImportRegistrationMarker<T> {
    /// A marker with a fresh token.
    pub(crate) fn new() -> Self {
        Self {
            alive: Arc::new(()),
            _type: std::marker::PhantomData,
        }
    }

    /// The weak reference a registration holds.
    pub(crate) fn alive(&self) -> Weak<()> {
        Arc::downgrade(&self.alive)
    }
}

impl<T: 'static> Resource for ImportRegistrationMarker<T> {
    /// Infrastructure, not game state: the marker says which generation
    /// registered a type, so a play session neither copies nor restores it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Asset, AssetLoadResult};
    use crate::asset_store::{mount_directory, mounted_directory_test_lock};
    use crate::World;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use trait_type_map::impl_trait_accessible;

    /// Settings of the text test asset.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct TextSettings {
        uppercase: bool,
    }

    /// A text file as an asset.
    #[derive(Debug)]
    struct Text(String);
    impl Asset for Text {}
    impl_trait_accessible!(dyn Asset; Text);

    impl ImportedAsset for Text {
        type ImportSettings = TextSettings;
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["txt", "TEXT"];

        fn metadata_type_name() -> &'static str {
            "pill_engine::asset_import_registry::tests::Text"
        }

        fn import(_name: &str, bytes: &[u8], settings: &TextSettings) -> AssetLoadResult<Self> {
            let text = String::from_utf8_lossy(bytes).into_owned();
            Ok(Self(if settings.uppercase {
                text.to_uppercase()
            } else {
                text
            }))
        }
    }

    /// A second type, which also wants `.txt`.
    #[derive(Debug)]
    struct Script;
    impl Asset for Script {}
    impl_trait_accessible!(dyn Asset; Script);

    impl ImportedAsset for Script {
        type ImportSettings = TextSettings;
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["lua", "txt"];

        fn metadata_type_name() -> &'static str {
            "pill_engine::asset_import_registry::tests::Script"
        }

        fn import(_name: &str, _bytes: &[u8], _settings: &TextSettings) -> AssetLoadResult<Self> {
            Ok(Self)
        }
    }

    /// A scratch `res` directory mounted under the mounted-directory lock.
    struct ScratchRes {
        root: PathBuf,
        _mounted: std::sync::MutexGuard<'static, ()>,
    }

    impl ScratchRes {
        fn new(label: &str) -> Self {
            let mounted = mounted_directory_test_lock();
            let root = std::env::temp_dir().join(format!(
                "pill-import-registry-{label}-{}",
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
            std::fs::write(self.root.join(relative), contents).unwrap();
        }
    }

    impl Drop for ScratchRes {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_file_is_imported_by_its_extension_in_any_case() {
        let res = ScratchRes::new("by-extension");
        res.write("a.TXT", "hello");
        let marker = ImportRegistrationMarker::<Text>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(marker.alive()).unwrap();
        let mut assets = AssetManager::new();

        let outcome = registry
            .import(
                &mut assets,
                Path::new("a.TXT"),
                MetadataPolicy::CreateIfMissing,
            )
            .unwrap();

        assert_eq!(outcome.type_name, Text::metadata_type_name());
        assert_eq!(outcome.name, "a.TXT");
        assert_eq!(outcome.metadata, MetadataSource::CreatedOnDisk);
        assert_eq!(
            registry.type_for_extension("text"),
            Some(Text::metadata_type_name())
        );
        assert_eq!(assets.get_by_name::<Text>("a.TXT").unwrap().0, "hello");
    }

    #[test]
    fn an_extension_claimed_by_two_types_names_both() {
        let text = ImportRegistrationMarker::<Text>::new();
        let script = ImportRegistrationMarker::<Script>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(text.alive()).unwrap();

        let error = registry.register::<Script>(script.alive()).unwrap_err();

        assert_eq!(
            error,
            ImportRegistrationError::ExtensionClaimedTwice {
                extension: "txt".to_owned(),
                registered_type: Text::metadata_type_name().to_owned(),
                incoming_type: Script::metadata_type_name().to_owned(),
            }
        );
        // Nothing of the refused type was recorded.
        assert_eq!(registry.type_for_extension("lua"), None);
    }

    /// A registration whose generation was retired is never called, and its
    /// extensions are free for another type.
    #[test]
    fn a_retired_registration_is_skipped_and_pruned() {
        let marker = ImportRegistrationMarker::<Text>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(marker.alive()).unwrap();
        drop(marker);
        let mut assets = AssetManager::new();

        let error = registry
            .import(
                &mut assets,
                Path::new("a.txt"),
                MetadataPolicy::ReadIfPresent,
            )
            .unwrap_err();
        assert!(matches!(error, ErasedImportError::UnknownExtension { .. }));

        let script = ImportRegistrationMarker::<Script>::new();
        registry.register::<Script>(script.alive()).unwrap();
        assert_eq!(registry.registered_types().len(), 1);
    }

    /// Two binaries registering one type: the newer is used, and when it is
    /// retired the older one takes over.
    #[test]
    fn the_newest_live_registration_wins_and_falls_back() {
        let module = ImportRegistrationMarker::<Text>::new();
        let project = ImportRegistrationMarker::<Text>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(module.alive()).unwrap();
        registry.register::<Text>(project.alive()).unwrap();
        assert_eq!(registry.registered_types().len(), 1);

        drop(project);
        registry.prune();

        assert_eq!(
            registry.type_for_extension("txt"),
            Some(Text::metadata_type_name())
        );
    }

    #[test]
    fn reimport_and_settings_go_through_the_registry() {
        let res = ScratchRes::new("reimport");
        res.write("a.txt", "hello");
        let marker = ImportRegistrationMarker::<Text>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(marker.alive()).unwrap();
        let mut assets = AssetManager::new();
        let imported = registry
            .import(
                &mut assets,
                Path::new("a.txt"),
                MetadataPolicy::CreateIfMissing,
            )
            .unwrap();

        let meta = res.root.join("a.txt.meta");
        let edited = std::fs::read_to_string(&meta)
            .unwrap()
            .replace("uppercase: false", "uppercase: true");
        std::fs::write(&meta, edited).unwrap();
        let reimported = registry.reimport(&mut assets, Path::new("a.txt")).unwrap();

        assert!(reimported.previously_loaded());
        assert_eq!(reimported.guid, imported.guid);
        assert!(reimported.content_version > imported.content_version);
        assert_eq!(assets.get_by_name::<Text>("a.txt").unwrap().0, "HELLO");
        assert_eq!(
            registry.settings_json(Path::new("a.txt")).unwrap(),
            serde_json::json!({"uppercase": true})
        );
        assert_eq!(
            registry
                .normalize_settings_json(Text::metadata_type_name(), serde_json::json!({}))
                .unwrap(),
            serde_json::json!({"uppercase": false})
        );
    }

    #[test]
    fn metadata_files_and_paths_without_extensions_are_refused() {
        let registry = ImportRegistry::default();
        let mut assets = AssetManager::new();
        assert!(matches!(
            registry.import(
                &mut assets,
                Path::new("a.txt.meta"),
                MetadataPolicy::ReadIfPresent
            ),
            Err(ErasedImportError::MetadataFile { .. })
        ));
        assert!(matches!(
            registry.import(
                &mut assets,
                Path::new("README"),
                MetadataPolicy::ReadIfPresent
            ),
            Err(ErasedImportError::NoExtension { .. })
        ));
    }

    /// The world-level registration: the marker is a resource of the world.
    /// Removing it ends the registration, and that is what the host does to a
    /// resource a retired generation no longer claims.
    #[cfg(feature = "development")]
    #[test]
    fn the_world_registration_lives_as_long_as_its_marker_resource() {
        let mut world = World::new();
        world.register_imported_asset::<Text>();
        assert!(world.take_registration_error().is_none());
        assert_eq!(
            world
                .get_resource::<ImportRegistry>()
                .unwrap()
                .type_for_extension("txt"),
            Some(Text::metadata_type_name())
        );

        // Registering again, as a reload's `init` does, keeps exactly one
        // live registration.
        world.register_imported_asset::<Text>();
        world.get_resource_mut::<ImportRegistry>().unwrap().prune();
        assert_eq!(
            world.get_resource::<ImportRegistry>().unwrap().entries[0]
                .registrations
                .len(),
            1
        );

        world
            .remove_resource::<ImportRegistrationMarker<Text>>()
            .unwrap();
        assert_eq!(
            world
                .get_resource::<ImportRegistry>()
                .unwrap()
                .type_for_extension("txt"),
            None
        );

        // The reload's re-homing pass removes the entry that has no live
        // registration left.
        world.rehome_assets();
        assert!(world
            .get_resource::<ImportRegistry>()
            .unwrap()
            .entries
            .is_empty());
    }

    /// Known files are imported, a stray metadata file is skipped, and each
    /// unknown extension is reported once however many files carry it.
    #[test]
    fn a_scan_imports_known_files_and_reports_unknown_extensions_once() {
        let res = ScratchRes::new("scan");
        std::fs::create_dir_all(res.root.join("nested/deeper")).unwrap();
        res.write("a.txt", "a");
        res.write("nested/b.txt", "b");
        res.write("nested/deeper/c.TXT", "c");
        res.write("stray.png.meta", "{}");
        res.write("one.bin", "?");
        res.write("two.bin", "?");
        res.write("README", "?");
        res.write(".hidden.tmp", "?");
        let marker = ImportRegistrationMarker::<Text>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(marker.alive()).unwrap();
        let mut assets = AssetManager::new();

        let first = registry.scan(&mut assets, &res.root, MetadataPolicy::CreateIfMissing);

        assert_eq!(
            first.imported,
            ["a.txt", "nested/b.txt", "nested/deeper/c.TXT"]
        );
        assert_eq!(first.metadata_created, first.imported);
        assert_eq!(first.unknown_extensions, ["", "bin"]);
        assert!(first.failed.is_empty(), "{:?}", first.failed);
        assert!(first.already_loaded.is_empty());
        assert!(res.root.join("nested/deeper/c.TXT.meta").is_file());
        assert_eq!(assets.get_by_name::<Text>("nested/b.txt").unwrap().0, "b");

        // A second scan decodes and creates nothing.
        let second = registry.scan(&mut assets, &res.root, MetadataPolicy::CreateIfMissing);
        assert!(second.imported.is_empty());
        assert!(second.metadata_created.is_empty());
        assert_eq!(second.already_loaded, first.imported);
    }

    /// A source that fails is reported with its error, and the scan goes on.
    #[test]
    fn a_failing_source_is_reported_and_the_scan_continues() {
        let res = ScratchRes::new("scan-failure");
        res.write("a.txt", "a");
        res.write("b.txt", "b");
        res.write("a.txt.meta", "not json");
        let marker = ImportRegistrationMarker::<Text>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(marker.alive()).unwrap();
        let mut assets = AssetManager::new();

        let report = registry.scan(&mut assets, &res.root, MetadataPolicy::ReadIfPresent);

        assert_eq!(report.imported, ["b.txt"]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "a.txt");
    }

    /// A standalone type: a label that refers to a text asset by guid, and
    /// records whether the reference resolved when it was built.
    #[derive(Debug)]
    struct Label {
        resolved: bool,
    }
    impl Asset for Label {}
    impl_trait_accessible!(dyn Asset; Label);

    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct LabelDocument {
        text: crate::asset_reference::AssetReference<Text>,
    }

    impl crate::asset_standalone::StandaloneAsset for Label {
        type Document = LabelDocument;
        const FILE_EXTENSION: &'static str = "label";

        fn metadata_type_name() -> &'static str {
            "pill_engine::asset_import_registry::tests::Label"
        }

        fn from_document(
            _name: &str,
            document: LabelDocument,
            assets: &AssetManager,
        ) -> AssetLoadResult<Self> {
            Ok(Self {
                resolved: document.text.try_resolve(assets).is_some(),
            })
        }
    }

    /// A path's guid and a guid's name and handle come back as the store holds
    /// them, and a guid no type loaded returns nothing. The loaded list names
    /// every asset of the type, which is how a picker learns about an asset
    /// built in code.
    #[test]
    fn the_store_answers_a_paths_guid_and_a_guids_name() {
        let text = ImportRegistrationMarker::<Text>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(text.alive()).unwrap();
        let mut assets = AssetManager::new();
        let guid = AssetGuid::random().expect("a fresh guid");
        assets
            .add_named_with_guid("notes/a.txt", guid, Text("a".to_owned()))
            .unwrap();
        // An asset built in code: a name and a guid, no file behind either.
        let built = AssetGuid::random().expect("a fresh guid");
        let built_handle = assets
            .add_named_with_guid("notes/built.txt", built, Text("built".to_owned()))
            .unwrap();

        assert_eq!(
            registry.guid_by_path(&assets, Path::new("notes/a.txt")),
            Some(guid)
        );
        let (type_name, name, index, generation) = registry
            .loaded_by_guid(&assets, guid)
            .expect("the file's guid answers");
        assert_eq!(type_name, Text::metadata_type_name());
        assert_eq!(name, "notes/a.txt");
        let file_handle = assets.handle_by_name::<Text>("notes/a.txt").unwrap();
        assert_eq!(
            (index, generation),
            (file_handle.index(), file_handle.generation())
        );
        let (type_name, name, index, generation) = registry
            .loaded_by_guid(&assets, built)
            .expect("an asset built in code answers too");
        assert_eq!(type_name, Text::metadata_type_name());
        assert_eq!(name, "notes/built.txt");
        assert_eq!(
            (index, generation),
            (built_handle.index(), built_handle.generation())
        );
        assert_eq!(
            registry.guid_by_path(&assets, Path::new("notes/missing.txt")),
            None
        );
        let other = AssetGuid::random().expect("a fresh guid");
        assert_eq!(registry.loaded_by_guid(&assets, other), None);

        // A name answers with the type and the handle, which is what a picker
        // writes for an asset that no guid names.
        let (type_name, index, generation) = registry
            .loaded_by_name(&assets, "notes/built.txt")
            .expect("the name answers");
        assert_eq!(type_name, Text::metadata_type_name());
        assert_eq!(
            (index, generation),
            (built_handle.index(), built_handle.generation())
        );
        assert_eq!(registry.loaded_by_name(&assets, "notes/missing.txt"), None);

        let loaded = registry.loaded_assets(&assets);
        assert_eq!(loaded.len(), 2, "{loaded:?}");
        assert!(loaded.contains(&(
            Text::metadata_type_name().to_owned(),
            Some(guid),
            "notes/a.txt".to_owned()
        )));
        assert!(loaded.contains(&(
            Text::metadata_type_name().to_owned(),
            Some(built),
            "notes/built.txt".to_owned()
        )));
    }

    #[test]
    fn standalone_types_are_listed_with_their_extension_and_default() {
        let text = ImportRegistrationMarker::<Text>::new();
        let label = ImportRegistrationMarker::<Label>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(text.alive()).unwrap();
        registry
            .register_standalone::<Label>(label.alive())
            .unwrap();

        assert_eq!(
            registry.standalone_types(),
            [(Label::metadata_type_name().to_owned(), "label".to_owned())]
        );
        assert!(registry.is_standalone_extension("LABEL"));
        assert!(!registry.is_standalone_extension("txt"));
        assert_eq!(
            registry
                .default_document(Label::metadata_type_name())
                .unwrap(),
            serde_json::json!({"text": null})
        );
        // A sourced type has no default document.
        assert!(registry
            .default_document(Text::metadata_type_name())
            .is_err());
    }

    /// A scan imports standalone files after every sourced one, so a label
    /// whose name sorts first still finds the text it refers to.
    #[test]
    fn a_scan_imports_standalone_files_after_their_sources() {
        let res = ScratchRes::new("scan-standalone");
        res.write("z.txt", "z");
        let text = ImportRegistrationMarker::<Text>::new();
        let label = ImportRegistrationMarker::<Label>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(text.alive()).unwrap();
        registry
            .register_standalone::<Label>(label.alive())
            .unwrap();

        // The text's guid has to be known up front for the label to name it.
        let mut probe = AssetManager::new();
        let guid = registry
            .import(
                &mut probe,
                Path::new("z.txt"),
                MetadataPolicy::CreateIfMissing,
            )
            .unwrap()
            .guid
            .expect("an imported source has a guid");
        res.write(
            "a.label",
            &format!(
                r#"{{"format_version": 1, "asset_type": "{}", "guid": "{}", "asset": {{"text": "{guid}"}}}}"#,
                Label::metadata_type_name(),
                "0000000000000000000000000000001a"
            ),
        );
        let mut assets = AssetManager::new();

        let report = registry.scan(&mut assets, &res.root, MetadataPolicy::CreateIfMissing);

        assert_eq!(report.imported, ["z.txt", "a.label"]);
        assert!(assets.get_by_name::<Label>("a.label").unwrap().resolved);
        assert!(
            !res.root.join("a.label.meta").exists(),
            "a standalone file gets no sidecar"
        );
    }

    /// The sweep writes a `.meta` for each source without one, skips sources
    /// that have one, unknown extensions and standalone files, and loads
    /// nothing.
    #[test]
    fn the_sweep_pairs_every_source_with_metadata() {
        let res = ScratchRes::new("ensure-sweep");
        std::fs::create_dir_all(res.root.join("nested")).unwrap();
        res.write("a.txt", "a");
        res.write("nested/b.txt", "b");
        res.write("other.bin", "?");
        res.write("note.label", "{}");
        let text = ImportRegistrationMarker::<Text>::new();
        let label = ImportRegistrationMarker::<Label>::new();
        let mut registry = ImportRegistry::default();
        registry.register::<Text>(text.alive()).unwrap();
        registry
            .register_standalone::<Label>(label.alive())
            .unwrap();
        let assets = AssetManager::new();

        let (written, failed) = registry.ensure_all_metadata(&assets, &res.root);

        assert_eq!(written, ["a.txt", "nested/b.txt"]);
        assert!(failed.is_empty(), "{failed:?}");
        assert!(res.root.join("nested/b.txt.meta").is_file());
        assert!(
            !res.root.join("note.label.meta").exists(),
            "a standalone file has no sidecar"
        );
        assert_eq!(assets.len::<Text>(), 0, "nothing was loaded");
        let (again, _) = registry.ensure_all_metadata(&assets, &res.root);
        assert!(again.is_empty());
    }

    #[test]
    fn a_world_registration_conflict_is_a_registration_error() {
        let mut world = World::new();
        world.register_imported_asset::<Text>();
        world.register_imported_asset::<Script>();
        let error = world
            .take_registration_error()
            .expect("the conflict is recorded");
        let message = error.to_string();
        assert!(message.contains(Text::metadata_type_name()), "{message}");
        assert!(message.contains(Script::metadata_type_name()), "{message}");
    }
}
