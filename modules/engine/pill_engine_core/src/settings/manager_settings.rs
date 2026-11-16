//! Settings assets that configure manager resources: the bindings, the readers
//! and the change detection.
//!
//! # Responsibilities
//!
//! - Binds a manager resource to the settings asset it reads, defaulting to the
//!   conventional `settings/<name>.<extension>` path.
//! - Reports which managers have no settings file yet, with the bytes a host
//!   writes to create one from the document defaults.
//! - Reads a settings asset's bytes, applies them through the manager's typed
//!   applier, and re-applies when the bytes change.
//! - Turns every failure into one sentence naming the manager, the path and the
//!   reason, so a host logs it once.
//!
//! # Design
//!
//! The module never writes a file: creating one needs the project's `res`
//! directory, which only a host knows, so a host asks
//! [`World::missing_manager_settings`] for the bytes and writes them. Reading
//! goes through the asset store, so a packed build answers exactly like a
//! directory does.
//!
//! Change detection is a fingerprint of the bytes last applied, not a
//! modification time and not an asset content version. A settings file may be
//! reimported without the world ever seeing a handle, and a shipped pack has no
//! modification time at all.
//!
//! # Related
//!
//! - `settings` - the manager traits and the per-type applier tables.
//! - `asset_standalone` - the standalone files a settings asset is stored as.

// Standard library
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

// Current crate
use crate::asset::AssetGuid;
use crate::asset_store;
use crate::resource::{Resource, ResourceId};
use crate::settings::{SettingsAssetOps, SettingsOutcome};
use crate::world::World;

// =============================================================================
// MissingManagerSettings
// =============================================================================

/// A manager whose settings file does not exist yet, with the bytes that create
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingManagerSettings {
    /// The manager resource the file configures.
    pub manager: ResourceId,
    /// The settings name the manager declared.
    pub settings_name: String,
    /// The asset path, relative to the mounted `res` directory.
    pub path: String,
    /// The complete file: header, guid and the document defaults.
    pub bytes: Vec<u8>,
}

// =============================================================================
// ManagerSettingsApplied
// =============================================================================

/// One manager whose settings file reached it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerSettingsApplied {
    /// The manager resource the file belongs to.
    pub manager: ResourceId,
    /// The settings name the manager declared.
    pub settings_name: String,
    /// The asset path that was read.
    pub path: String,
    /// What the manager did with the document.
    pub outcome: SettingsOutcome,
}

// =============================================================================
// ManagerSettingsFailure
// =============================================================================

/// A settings file that could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerSettingsFailure {
    /// The manager resource the file belongs to.
    pub manager: ResourceId,
    /// The settings name the manager declared.
    pub settings_name: String,
    /// The asset path that was read.
    pub path: String,
    /// Why the file was refused, or why it does not fit the manager's document.
    pub reason: String,
}

// =============================================================================
// ManagerSettingsReport
// =============================================================================

/// What one pass over every manager's settings asset did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ManagerSettingsReport {
    /// The managers whose settings reached them, in manager-id order.
    pub applied: Vec<ManagerSettingsApplied>,
    /// The files that could not be applied, in manager-id order.
    pub failures: Vec<ManagerSettingsFailure>,
}

impl ManagerSettingsReport {
    /// Whether nothing happened at all: no manager applied, nothing failed.
    pub fn is_empty(&self) -> bool {
        self.applied.is_empty() && self.failures.is_empty()
    }
}

// =============================================================================
// ManagerSettingsSurface
// =============================================================================

/// What one manager's settings asset declares, copied out of its table.
///
/// The names live in the artifact that registered the manager, so a reader that
/// keeps the answer past a reload takes this copy while the image is mapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerSettingsSurface {
    /// The settings name the manager declared.
    pub settings_name: String,
    /// The settings asset type's file extension.
    pub file_extension: String,
    /// The name a created file carries in its `asset_type` field.
    pub asset_type_name: String,
    /// The conventional path, before any binding replaces it.
    pub path: String,
}

// =============================================================================
// ManagerSettingsBindings
// =============================================================================

/// The path each manager's settings asset was bound to, and the bytes it last
/// applied.
///
/// A world resource rather than a field of the manager, because it is the
/// engine's bookkeeping and has to outlive the manager's own reload: a module
/// that comes back with a new binding store would otherwise forget the path a
/// game chose.
#[derive(Debug, Default)]
pub(crate) struct ManagerSettingsBindings {
    /// One entry per manager that has been bound or applied.
    pub(crate) bindings: BTreeMap<ResourceId, ManagerSettingsBinding>,
}

/// One manager's binding: the path it reads and the fingerprint of what reached
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagerSettingsBinding {
    /// The asset path the manager reads.
    pub(crate) path: String,
    /// The fingerprint of the bytes last applied, or `None` when nothing has
    /// been applied yet.
    pub(crate) applied_fingerprint: Option<u64>,
}

impl Resource for ManagerSettingsBindings {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }
}

// =============================================================================
// World
// =============================================================================

impl World {
    /// The managers whose settings live in a settings asset, sorted by id.
    ///
    /// Only registered *and* live managers are listed: a type that declared its
    /// surface but inserted no value has nothing to configure.
    #[must_use]
    pub fn settings_asset_manager_ids(&self) -> Vec<ResourceId> {
        let mut ids: Vec<ResourceId> = self
            .resource_factories
            .iter()
            .filter(|(id, resource_ops)| {
                resource_ops.settings_asset.is_some() && self.resources.contains_key(id)
            })
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// What the manager's settings asset declares, or `None` when the type is
    /// not a settings-asset manager.
    #[must_use]
    pub fn manager_settings_surface(&self, manager: ResourceId) -> Option<ManagerSettingsSurface> {
        let resource_ops = self.resource_factories.get(&manager)?;
        let settings_asset = resource_ops.settings_asset?;
        Some(ManagerSettingsSurface {
            settings_name: settings_asset.settings_name.to_owned(),
            file_extension: settings_asset.file_extension.to_owned(),
            asset_type_name: settings_asset.asset_type_name.to_owned(),
            path: conventional_path(settings_asset),
        })
    }

    /// The path the manager's settings asset reads: the bound path when the
    /// manager was bound, and the conventional one otherwise.
    ///
    /// Returns `None` when the type is not a settings-asset manager.
    #[must_use]
    pub fn manager_settings_path(&self, manager: ResourceId) -> Option<String> {
        let conventional = self.manager_settings_surface(manager)?.path;
        Some(
            self.settings_bindings()
                .and_then(|bindings| bindings.bindings.get(&manager))
                .map_or(conventional, |binding| binding.path.clone()),
        )
    }

    /// Binds the manager to a settings asset and applies it now.
    ///
    /// The binding is recorded even when the file cannot be applied, so a later
    /// [`Self::refresh_manager_settings`] retries the same file instead of
    /// falling back to the conventional path.
    ///
    /// Returns `None` when the type is not a settings-asset manager, the
    /// manager's outcome otherwise: a refusal is the manager's own call, while
    /// `Err` carries the reason the file itself could not be used.
    pub fn bind_manager_settings(
        &mut self,
        manager: ResourceId,
        path: impl Into<String>,
    ) -> Option<Result<SettingsOutcome, String>> {
        self.manager_settings_surface(manager)?;
        let path = path.into();
        let bindings = self.settings_bindings_mut();
        bindings.bindings.insert(
            manager,
            ManagerSettingsBinding {
                path,
                applied_fingerprint: None,
            },
        );
        Some(self.apply_bound_manager_settings(manager))
    }

    /// The managers whose settings file does not exist yet, each with the bytes
    /// that create it from the document defaults.
    ///
    /// Writing them is the host's job: it knows the project's `res` directory,
    /// and this module deliberately owns no filesystem root. The conventional
    /// path is bound as the file is reported, so
    /// [`Self::apply_manager_settings_asset`] finds the file once it is written.
    pub fn missing_manager_settings(&mut self) -> Vec<MissingManagerSettings> {
        let managers = self.settings_asset_manager_ids();
        let mut missing = Vec::new();
        for manager in managers {
            let Some(surface) = self.manager_settings_surface(manager) else {
                continue;
            };
            let Some(settings_asset) = self.settings_asset_ops(manager) else {
                continue;
            };
            let path = self
                .manager_settings_path(manager)
                .unwrap_or_else(|| surface.path.clone());
            if asset_store::read(Path::new(&path)).is_ok() {
                continue;
            }
            let guid = match AssetGuid::random() {
                Ok(guid) => guid,
                Err(_) => continue,
            };
            let Ok(bytes) = (settings_asset.render_default)(guid) else {
                continue;
            };
            self.settings_bindings_mut()
                .bindings
                .entry(manager)
                .or_insert_with(|| ManagerSettingsBinding {
                    path: path.clone(),
                    applied_fingerprint: None,
                });
            missing.push(MissingManagerSettings {
                manager,
                settings_name: surface.settings_name,
                path,
                bytes,
            });
        }
        missing
    }

    /// Applies the manager's settings asset to its live value.
    ///
    /// Returns `None` when the type is not a settings-asset manager, the
    /// manager's outcome otherwise.
    pub fn apply_manager_settings_asset(
        &mut self,
        manager: ResourceId,
    ) -> Option<Result<SettingsOutcome, String>> {
        self.manager_settings_surface(manager)?;
        if self
            .settings_bindings()
            .is_none_or(|bindings| !bindings.bindings.contains_key(&manager))
        {
            let conventional = self.manager_settings_path(manager)?;
            self.settings_bindings_mut().bindings.insert(
                manager,
                ManagerSettingsBinding {
                    path: conventional,
                    applied_fingerprint: None,
                },
            );
        }
        Some(self.apply_bound_manager_settings(manager))
    }

    /// Re-applies every bound manager whose settings bytes changed since they
    /// last reached it, and reports what happened.
    ///
    /// A missing file is neither applied nor a failure: the manager keeps the
    /// values it has, and the host decides whether to create the file. A
    /// binding whose manager left the world is dropped here, so the ids a
    /// reload replaced do not pile up.
    pub fn refresh_manager_settings(&mut self) -> ManagerSettingsReport {
        let managers = self.settings_asset_manager_ids();
        let mut report = ManagerSettingsReport::default();
        for manager in managers {
            let Some(surface) = self.manager_settings_surface(manager) else {
                continue;
            };
            let Some(path) = self.manager_settings_path(manager) else {
                continue;
            };
            let applied_fingerprint = self
                .settings_bindings()
                .and_then(|bindings| bindings.bindings.get(&manager))
                .and_then(|binding| binding.applied_fingerprint);
            let Ok(bytes) = asset_store::read(Path::new(&path)) else {
                continue;
            };
            if Some(fingerprint_of(&bytes)) == applied_fingerprint {
                continue;
            }
            match self.apply_bound_manager_settings(manager) {
                Ok(outcome) => report.applied.push(ManagerSettingsApplied {
                    manager,
                    settings_name: surface.settings_name,
                    path,
                    outcome,
                }),
                Err(reason) => report.failures.push(ManagerSettingsFailure {
                    manager,
                    settings_name: surface.settings_name,
                    path,
                    reason,
                }),
            }
        }
        // A reload gives a manager a new id, so an id the world no longer holds
        // a manager under describes nothing.
        let live = self.settings_asset_manager_ids();
        if let Some(bindings) = self.get_resource_mut::<ManagerSettingsBindings>() {
            bindings
                .bindings
                .retain(|manager, _| live.contains(manager));
        }
        report
    }

    /// Reads the manager's bound settings file and applies it, recording the
    /// bytes that reached the manager.
    fn apply_bound_manager_settings(
        &mut self,
        manager: ResourceId,
    ) -> Result<SettingsOutcome, String> {
        // Step 1: Resolve what to read and what to apply it with.
        let path = self
            .manager_settings_path(manager)
            .ok_or_else(|| "the type is not a settings-asset manager".to_owned())?;
        let settings_asset = self
            .settings_asset_ops(manager)
            .ok_or_else(|| "the type is not a settings-asset manager".to_owned())?;
        // Step 2: Read the bytes, so the fingerprint describes exactly what was
        // applied.
        let path_ref = Path::new(&path);
        let bytes = asset_store::read(path_ref).map_err(|error| error.to_string())?;
        let fingerprint = fingerprint_of(&bytes);
        // Step 3: Apply it to the live value.
        let resource = self
            .resources
            .get_mut(&manager)
            .ok_or_else(|| "nothing is stored under this manager's id".to_owned())?;
        let value_pointer = resource.value_pointer_mut();
        // SAFETY: the table was built by the type's own registration, and the
        // box under this id was created by that same registration, so the
        // pointer names a live value of the type the applier expects. The box
        // outlives the call.
        let outcome = unsafe { (settings_asset.apply_bytes)(value_pointer, path_ref, &bytes) }?;
        // Step 4: Record what reached the manager, keeping whatever path was
        // bound (a clean refresh does not re-bind).
        let bindings = self.settings_bindings_mut();
        let binding = bindings
            .bindings
            .entry(manager)
            .or_insert_with(|| ManagerSettingsBinding {
                path: path.clone(),
                applied_fingerprint: None,
            });
        binding.applied_fingerprint = Some(fingerprint);
        Ok(outcome)
    }

    /// The manager type's settings-asset surface, copied out of its table.
    fn settings_asset_ops(&self, manager: ResourceId) -> Option<SettingsAssetOps> {
        self.resource_factories.get(&manager)?.settings_asset
    }

    /// The bindings store, or `None` when no manager was bound yet.
    fn settings_bindings(&self) -> Option<&ManagerSettingsBindings> {
        self.get_resource::<ManagerSettingsBindings>()
    }

    /// The bindings store, inserted on first use.
    fn settings_bindings_mut(&mut self) -> &mut ManagerSettingsBindings {
        if self.get_resource::<ManagerSettingsBindings>().is_none() {
            self.insert_resource(ManagerSettingsBindings::default());
        }
        self.get_resource_mut::<ManagerSettingsBindings>()
            .expect("the store was inserted just above")
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The path a manager's settings asset takes when nothing binds another one.
fn conventional_path(settings_asset: SettingsAssetOps) -> String {
    format!(
        "settings/{}.{}",
        settings_asset.settings_name, settings_asset.file_extension
    )
}

/// A fingerprint of the bytes that reached a manager.
///
/// Only ever compared with another fingerprint from the same process, so it
/// needs no stability across runs.
fn fingerprint_of(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // Standard library
    use std::path::Path;

    // External crates
    use serde::{Deserialize, Serialize};
    use trait_type_map::impl_trait_accessible;

    // Current crate
    use super::*;
    use crate::asset::{Asset, AssetLoadResult, AssetManager};
    use crate::asset_standalone::StandaloneAsset;
    use crate::asset_store::{mount_directory, mounted_directory_test_lock};
    use crate::settings::{SettingsAssetManager, SettingsAssetOps, SettingsOutcome};

    /// The settings surface the fixture manager registers.
    fn fixture_ops() -> SettingsAssetOps {
        SettingsAssetOps::of::<FixtureManager>()
    }

    /// Applies `bytes` to `manager` through the surface it registered.
    fn apply_fixture_bytes(
        manager: &mut FixtureManager,
        bytes: &[u8],
    ) -> Result<SettingsOutcome, String> {
        // SAFETY: the pointer names the live fixture manager, which is the type
        // the surface was built for, and the box outlives the call.
        unsafe {
            (fixture_ops().apply_bytes)(
                std::ptr::addr_of_mut!(*manager).cast::<u8>(),
                Path::new("settings/fixture_manager.fixture_settings"),
                bytes,
            )
        }
    }

    /// The document a fixture manager reads.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default)]
    struct FixtureSettings {
        /// How many steps the fixture runs.
        steps: u32,
        /// Where the fixture logs.
        label: String,
    }

    impl Default for FixtureSettings {
        fn default() -> Self {
            Self {
                steps: 3,
                label: "fixture".to_owned(),
            }
        }
    }

    /// The standalone asset a fixture manager's settings live in.
    #[derive(Debug)]
    struct FixtureSettingsAsset {
        /// The document as loaded.
        settings: FixtureSettings,
    }

    impl Asset for FixtureSettingsAsset {}
    impl_trait_accessible!(dyn Asset; FixtureSettingsAsset);

    impl StandaloneAsset for FixtureSettingsAsset {
        type Document = FixtureSettings;
        const FILE_EXTENSION: &'static str = "fixture_settings";

        fn from_document(
            _name: &str,
            document: Self::Document,
            _assets: &AssetManager,
        ) -> AssetLoadResult<Self> {
            Ok(Self { settings: document })
        }
    }

    /// A manager the settings tests configure.
    #[derive(Debug, Default)]
    struct FixtureManager {
        /// The settings last applied.
        settings: FixtureSettings,
    }

    impl Resource for FixtureManager {}

    impl SettingsAssetManager for FixtureManager {
        type Settings = FixtureSettingsAsset;

        fn settings_name() -> &'static str {
            "fixture_manager"
        }

        fn apply_settings(&mut self, settings: &FixtureSettings) -> SettingsOutcome {
            // Zero steps is a value the fixture cannot run with, so refusing it
            // is how the tests see a refusal that keeps the old value.
            if settings.steps == 0 {
                return SettingsOutcome::Refused;
            }
            self.settings = settings.clone();
            SettingsOutcome::Applied
        }
    }

    /// The conventional path names the manager and the asset's extension.
    #[test]
    fn the_conventional_path_names_the_manager_and_the_extension() {
        assert_eq!(
            FixtureManager::settings_asset_path(),
            "settings/fixture_manager.fixture_settings"
        );
    }

    /// The bytes a host writes for a missing file are the defaults, and the
    /// applier reads them straight back.
    #[test]
    fn a_created_file_holds_the_document_defaults() {
        let bytes = (fixture_ops().render_default)(AssetGuid::new(7)).expect("the defaults render");
        let mut manager = FixtureManager::default();
        let outcome = apply_fixture_bytes(&mut manager, &bytes).expect("the file applies");

        assert_eq!(outcome, SettingsOutcome::Applied);
        assert_eq!(manager.settings, FixtureSettings::default());
        // The same defaults build the asset a load would hand the store.
        let asset = FixtureSettingsAsset::from_document(
            "settings/fixture_manager.fixture_settings",
            FixtureSettings::default(),
            &AssetManager::new(),
        )
        .expect("the defaults build an asset");
        assert_eq!(asset.settings, FixtureSettings::default());
    }

    /// A file that carries a value re-applies it, and one the manager refuses
    /// leaves the live value alone.
    #[test]
    fn an_edit_applies_and_a_refusal_keeps_the_live_value() {
        let mut manager = FixtureManager::default();

        let edited = crate::asset_standalone::render_standalone::<FixtureSettingsAsset>(
            AssetGuid::new(8),
            &FixtureSettings {
                steps: 9,
                label: "edited".to_owned(),
            },
        )
        .expect("the document renders");
        let outcome = apply_fixture_bytes(&mut manager, &edited).expect("the file applies");
        assert_eq!(outcome, SettingsOutcome::Applied);
        assert_eq!(manager.settings.steps, 9);
        assert_eq!(manager.settings.label, "edited");

        let refused = crate::asset_standalone::render_standalone::<FixtureSettingsAsset>(
            AssetGuid::new(9),
            &FixtureSettings {
                steps: 0,
                label: "refused".to_owned(),
            },
        )
        .expect("the document renders");
        let outcome = apply_fixture_bytes(&mut manager, &refused).expect("the file reads");
        assert_eq!(outcome, SettingsOutcome::Refused);
        assert_eq!(manager.settings.steps, 9);
        assert_eq!(manager.settings.label, "edited");
    }

    /// A document that does not fit the manager's type names the asset type and
    /// the field.
    #[test]
    fn a_document_that_does_not_fit_is_reported_with_the_type_name() {
        let mut manager = FixtureManager::default();
        let document = serde_json::json!({
            "format_version": crate::asset_metadata::METADATA_FORMAT_VERSION,
            "asset_type": <FixtureSettingsAsset as StandaloneAsset>::metadata_type_name(),
            "guid": "00000000000000000000000000000001",
            "asset": { "steps": "many" },
        });
        let bytes = crate::asset_document::render(&document).expect("the file renders");

        let error =
            apply_fixture_bytes(&mut manager, &bytes).expect_err("the document does not fit");

        assert!(
            error.contains(<FixtureSettingsAsset as StandaloneAsset>::metadata_type_name()),
            "the error names the asset type: {error}"
        );
        assert!(
            error.contains("expected u32"),
            "the error names what the field wanted: {error}"
        );
    }

    /// A plain declaration of the same type - what a project image runs when it
    /// links the module that owns a manager - keeps the settings surface the
    /// module recorded, so the host still finds the manager.
    #[test]
    fn a_plain_registration_keeps_the_recorded_settings_surface() {
        let mut world = World::new();
        world.insert_settings_asset_manager(FixtureManager::default());
        let manager = ResourceId::of::<FixtureManager>();

        world.register_resource::<FixtureManager>();

        assert_eq!(world.settings_asset_manager_ids(), vec![manager]);
        assert_eq!(
            world.manager_settings_path(manager).as_deref(),
            Some("settings/fixture_manager.fixture_settings")
        );
    }

    /// A plain insert of a manager value replaces the value, not the type's
    /// declaration, so the recorded surface stays too.
    #[test]
    fn a_plain_insert_keeps_the_recorded_settings_surface() {
        let mut world = World::new();
        world.insert_settings_asset_manager(FixtureManager::default());
        let manager = ResourceId::of::<FixtureManager>();

        world.insert_resource(FixtureManager::default());

        assert_eq!(world.settings_asset_manager_ids(), vec![manager]);
    }

    /// A scratch `res` directory for one test, mounted under the process-wide
    /// directory lock while it lives.
    struct ScratchRes {
        /// The directory mounted as the asset root.
        root: std::path::PathBuf,
        /// Held for the test's lifetime, so no other test mounts meanwhile.
        _mounted: std::sync::MutexGuard<'static, ()>,
    }

    impl ScratchRes {
        /// Creates an empty directory and mounts it as the asset root.
        fn new(label: &str) -> Self {
            let mounted = mounted_directory_test_lock();
            let root = std::env::temp_dir().join(format!(
                "pill-manager-settings-{label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("the scratch directory is created");
            mount_directory(&root);
            Self {
                root,
                _mounted: mounted,
            }
        }

        /// Writes `bytes` at `relative_path` under the mounted root.
        fn write(&self, relative_path: &str, bytes: &[u8]) {
            let path = self.root.join(relative_path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("the settings directory is created");
            }
            std::fs::write(path, bytes).expect("the settings file is written");
        }
    }

    impl Drop for ScratchRes {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Renders a settings file holding `settings` under `guid`.
    fn render_fixture_file(guid: u128, settings: &FixtureSettings) -> Vec<u8> {
        crate::asset_standalone::render_standalone::<FixtureSettingsAsset>(
            AssetGuid::new(guid),
            settings,
        )
        .expect("the file renders")
    }

    /// A manager with no file is reported with the bytes that create one, and
    /// that file then applies and re-applies on a later edit.
    #[test]
    fn a_missing_file_is_created_and_reapplied_on_edit() {
        let res = ScratchRes::new("create");
        let mut world = World::new();
        world.insert_settings_asset_manager(FixtureManager::default());

        // Step 1: Nothing exists yet, so the manager is reported with a file to
        // write at the conventional path.
        let missing = world.missing_manager_settings();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].path, "settings/fixture_manager.fixture_settings");
        assert_eq!(missing[0].settings_name, "fixture_manager");
        res.write(&missing[0].path, &missing[0].bytes);

        // Step 2: The manager has nothing yet, because nothing applied the file.
        assert_eq!(
            world
                .get_resource::<FixtureManager>()
                .expect("the manager is live")
                .settings,
            FixtureSettings::default()
        );
        let outcome = world
            .apply_manager_settings_asset(missing[0].manager)
            .expect("the type is a settings-asset manager")
            .expect("the created file applies");
        assert_eq!(outcome, SettingsOutcome::Applied);

        // Step 3: Nothing changed, so a refresh applies nothing.
        assert!(world.refresh_manager_settings().is_empty());
        // Step 4: An edit re-applies on the next refresh, and only then.
        res.write(
            &missing[0].path,
            &render_fixture_file(
                11,
                &FixtureSettings {
                    steps: 12,
                    label: "second".to_owned(),
                },
            ),
        );
        assert_eq!(
            world
                .get_resource::<FixtureManager>()
                .expect("the manager is live")
                .settings
                .steps,
            3
        );
        let report = world.refresh_manager_settings();
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].outcome, SettingsOutcome::Applied);
        assert_eq!(
            report.applied[0].path,
            "settings/fixture_manager.fixture_settings"
        );
        assert_eq!(
            world
                .get_resource::<FixtureManager>()
                .expect("the manager is live")
                .settings
                .steps,
            12
        );
    }

    /// A file that does not fit the document is reported once with the path and
    /// the reason, and the live value keeps what it had.
    #[test]
    fn an_unfitting_file_is_reported_and_keeps_the_live_value() {
        let res = ScratchRes::new("unfitting");
        let mut world = World::new();
        world.insert_settings_asset_manager(FixtureManager::default());
        let missing = world.missing_manager_settings();
        res.write(&missing[0].path, &missing[0].bytes);
        world
            .apply_manager_settings_asset(missing[0].manager)
            .expect("the type is a settings-asset manager")
            .expect("the created file applies");

        // A document whose field has the wrong type cannot reach the manager.
        let unfitting_document = serde_json::json!({
            "format_version": crate::asset_metadata::METADATA_FORMAT_VERSION,
            "asset_type": <FixtureSettingsAsset as StandaloneAsset>::metadata_type_name(),
            "guid": "0000000000000000000000000000000c",
            "asset": { "steps": "many" },
        });
        let unfitting =
            crate::asset_document::render(&unfitting_document).expect("the file renders");
        res.write(&missing[0].path, &unfitting);
        let failures = world.refresh_manager_settings();

        assert_eq!(failures.failures.len(), 1);
        assert_eq!(
            failures.failures[0].path,
            "settings/fixture_manager.fixture_settings"
        );
        assert_eq!(failures.failures[0].settings_name, "fixture_manager");
        assert!(
            failures.failures[0].reason.contains("expected u32"),
            "the reason says what the field wanted: {}",
            failures.failures[0].reason
        );
        assert_eq!(
            world
                .get_resource::<FixtureManager>()
                .expect("the manager is live")
                .settings,
            FixtureSettings::default()
        );
    }

    /// Binding a manager to another settings asset moves what a refresh reads.
    #[test]
    fn a_bound_manager_reads_the_file_it_was_bound_to() {
        let res = ScratchRes::new("bind");
        let mut world = World::new();
        world.insert_settings_asset_manager(FixtureManager::default());
        let manager = world.missing_manager_settings()[0].manager;

        let other = "settings/other.fixture_settings";
        res.write(
            other,
            &render_fixture_file(
                12,
                &FixtureSettings {
                    steps: 4,
                    label: "other".to_owned(),
                },
            ),
        );
        let outcome = world
            .bind_manager_settings(manager, other)
            .expect("the type is a settings-asset manager")
            .expect("the bound file applies");

        assert_eq!(outcome, SettingsOutcome::Applied);
        assert_eq!(
            world
                .get_resource::<FixtureManager>()
                .expect("the manager is live")
                .settings
                .label,
            "other"
        );
        assert_eq!(world.manager_settings_path(manager).as_deref(), Some(other));
    }
}
