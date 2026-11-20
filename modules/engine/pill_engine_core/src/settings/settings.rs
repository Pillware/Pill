//! Manager settings: what a project configures on a subsystem-owning resource.
//!
//! # Responsibilities
//!
//! - Declares [`SettingsAssetManager`]: the standalone asset type a manager
//!   reads, its settings name, its conventional path, and the applier that moves
//!   a document into the live value.
//! - Carries each manager's settings surface as plain data
//!   ([`SettingsAssetOps`]) so it can live in the per-type table that a reload
//!   re-points at the newest image: rendering a created file and applying an
//!   edited one.
//! - Declares [`SettingsOutcome`]: what the manager decided about a document.
//!
//! # Design
//!
//! A settings asset is a standalone asset - a real file in the project's `res`
//! with a header, a guid and a document - at
//! `settings/<settings_name>.<extension>`. The manager owns the document type,
//! so an edit is checked against the type before it reaches the live value, and
//! the Inspector can draw the same fields a component shows.
//!
//! A manager is a [`Resource`] that owns a subsystem - the asset store, the
//! audio device, a physics world. The host creates the manager's file from its
//! document defaults when it is missing, applies it after a load, and
//! re-applies it when the file changes.
//!
//! The applier is a function pointer into the artifact that registered the
//! manager. It therefore has the same lifetime rule as the drop function it sits
//! beside: valid while that image is mapped, and refreshed by the world's
//! re-homing pass together with the rest of the table. A manager that has to
//! survive a reload needs a `shared_name`, because an id built from `TypeId`
//! differs in every generation.

// Standard library
use std::path::Path;

// Current crate
use crate::asset::AssetGuid;
use crate::asset_metadata::parse_header;
use crate::asset_standalone::{render_standalone, StandaloneAsset, STANDALONE_BODY_FIELD};
use crate::resource::Resource;

// =============================================================================
// SettingsOutcome
// =============================================================================

/// What a manager did with a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsOutcome {
    /// The document is in effect.
    Applied,
    /// The manager refused it, kept what it had, and logged why.
    Refused,
}

// =============================================================================
// SettingsAssetManager
// =============================================================================

/// A manager whose tuning lives in a settings asset of its own type.
///
/// A manager declares the standalone asset type that holds its settings, and
/// everything else follows from that declaration: the conventional path
/// (`settings/<settings_name>.<extension>` under the project's mounted `res`),
/// the file a host writes when the project has none, the check that an edited
/// file still fits the document, and re-applying it to the live value when its
/// bytes change.
///
/// The value is stored like any manager ([`crate::World::insert_manager_resource`]),
/// and its settings surface is registered with
/// [`crate::World::insert_settings_asset_manager`] or
/// [`crate::World::register_settings_asset_manager`]. The development host then
/// binds and applies every manager's settings asset; a game may point a manager
/// at another settings asset at runtime with
/// [`crate::World::bind_manager_settings`].
///
/// A manager with nothing to configure does not implement the trait: a settings
/// file in every project would be a file nobody can act on.
pub trait SettingsAssetManager: Resource {
    /// The standalone asset type that holds this manager's tuning.
    ///
    /// Its `Document` is the settings document the manager applies, and its
    /// `FILE_EXTENSION` names the file (`audio_settings`).
    type Settings: StandaloneAsset;

    /// The settings name: the file stem and the label a log line uses
    /// (`pill_audio`).
    ///
    /// Letters, digits and `_` only, so the name is safe as a file name. The
    /// module's own name is the convention, because a person looking at the
    /// folder knows which module a file belongs to.
    fn settings_name() -> &'static str;

    /// Where this manager's settings asset lives while the project keeps the
    /// default: `settings/<settings_name>.<extension>` under the mounted `res`
    /// directory.
    fn settings_asset_path() -> String {
        format!(
            "settings/{}.{}",
            Self::settings_name(),
            <Self::Settings as StandaloneAsset>::FILE_EXTENSION
        )
    }

    /// Applies a settings document to the live value.
    ///
    /// Return [`SettingsOutcome::Refused`] when the document cannot be used - a
    /// value out of range, a setting that cannot change while the subsystem
    /// runs - and leave the manager exactly as it was. The manager logs the
    /// reason itself; the caller logs which file it came from.
    fn apply_settings(
        &mut self,
        settings: &<Self::Settings as StandaloneAsset>::Document,
    ) -> SettingsOutcome;
}

// =============================================================================
// SettingsAssetOps
// =============================================================================

/// A settings-asset manager's surface as plain data, stored in its per-type
/// table.
///
/// The appliers belong to the artifact that registered the manager, so they
/// follow the same lifetime rule as the drop function beside them: valid while
/// that image is mapped, and replaced together with the rest of the table when
/// the world re-homes a resource.
#[derive(Clone, Copy)]
pub struct SettingsAssetOps {
    /// The file stem, from [`SettingsAssetManager::settings_name`].
    pub settings_name: &'static str,
    /// The settings asset type's file extension.
    pub file_extension: &'static str,
    /// The name a created file carries in its `asset_type` field.
    pub asset_type_name: &'static str,
    /// Renders a complete file holding the document defaults under `guid`.
    pub render_default: fn(AssetGuid) -> Result<Vec<u8>, String>,
    /// Reads a settings file's bytes and applies them to the value at the first
    /// argument.
    ///
    /// # Safety
    ///
    /// The pointer must point at a live, correctly aligned value of the type
    /// this table was built for. The path is used for error messages only.
    pub apply_bytes: unsafe fn(*mut u8, &Path, &[u8]) -> Result<SettingsOutcome, String>,
}

impl SettingsAssetOps {
    /// Builds the settings-asset surface of a manager type.
    pub fn of<T: SettingsAssetManager>() -> Self {
        Self {
            settings_name: T::settings_name(),
            file_extension: <T::Settings as StandaloneAsset>::FILE_EXTENSION,
            asset_type_name: <T::Settings as StandaloneAsset>::metadata_type_name(),
            render_default: render_settings_default_of::<T>,
            apply_bytes: apply_settings_bytes_of::<T>,
        }
    }
}

/// Renders a settings file holding `T`'s document defaults.
fn render_settings_default_of<T: SettingsAssetManager>(guid: AssetGuid) -> Result<Vec<u8>, String> {
    let document = <<T::Settings as StandaloneAsset>::Document as Default>::default();
    render_standalone::<T::Settings>(guid, &document).map_err(|error| error.to_string())
}

/// Applies a settings file's bytes to the `T` at `value_pointer`.
///
/// # Safety
///
/// `value_pointer` must point at a live, correctly aligned `T`.
unsafe fn apply_settings_bytes_of<T: SettingsAssetManager>(
    value_pointer: *mut u8,
    path: &Path,
    bytes: &[u8],
) -> Result<SettingsOutcome, String> {
    // Step 1: Read the document out of the file, naming the asset type when it
    // does not fit.
    let asset_type_name = <T::Settings as StandaloneAsset>::metadata_type_name();
    let (_guid, body) = parse_header(path, bytes, asset_type_name, STANDALONE_BODY_FIELD)
        .map_err(|error| error.to_string())?;
    let settings: <T::Settings as StandaloneAsset>::Document = match body {
        Some(body) => serde_json::from_value(body)
            .map_err(|error| format!("the document does not fit {asset_type_name}: {error}"))?,
        // A file with no body is the type's defaults, exactly as the asset
        // store reads one.
        None => Default::default(),
    };
    // Step 2: Hand it to the manager.
    // SAFETY: guaranteed by the caller; the table and the box it describes come
    // from the same registration.
    let manager = unsafe { &mut *value_pointer.cast::<T>() };
    Ok(manager.apply_settings(&settings))
}
