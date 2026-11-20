//! The `.theme` standalone asset.
//!
//! # Responsibilities
//!
//! - [`THEME_SHARED_NAME`]: the pinned asset identity across binaries.
//! - The [`Asset`](pill_engine_core::Asset) and
//!   [`StandaloneAsset`](pill_engine_core::StandaloneAsset) implementations for
//!   [`ThemeOverrides`], so a `.theme` file loads by extension through the
//!   world's import registry like a `.material` or `.scene`.
//!
//! # Design
//!
//! A `.theme` file is a standalone asset. Its own header carries
//! `format_version`, `asset_type` and `guid`, and the override document sits
//! under `asset`, so the file needs no `.meta` sidecar. The type holds no
//! asset references, so `from_document` returns the document unchanged.
//!
//! The pinned shared name gives the type one asset identity in every binary
//! that carries it: the project, `pill_project_ui`, the editor shell and
//! `pill_editor_core`. That holds even if more than one of them ends up with
//! its own compiled copy of this crate.

// External crates
use trait_type_map::impl_trait_accessible;

// Current crate
use pill_engine_core::{Asset, AssetLoadResult, AssetManager, StandaloneAsset};

use crate::overrides::ThemeOverrides;

/// The pinned shared name of [`ThemeOverrides`], used by its `Asset`
/// implementation to keep one asset column across binaries.
///
/// The engine writes the name to the `asset_type` field of the `.theme` files
/// it creates, so the text must not change.
pub const THEME_SHARED_NAME: &str = "pill_ui_theme::ThemeOverrides";

impl Asset for ThemeOverrides {
    fn shared_name() -> Option<&'static str> {
        Some(THEME_SHARED_NAME)
    }
}

impl StandaloneAsset for ThemeOverrides {
    type Document = ThemeOverrides;

    const FILE_EXTENSION: &'static str = "theme";

    fn from_document(
        _name: &str,
        document: ThemeOverrides,
        _assets: &AssetManager,
    ) -> AssetLoadResult<Self> {
        Ok(document)
    }
}

// Makes `ThemeOverrides` storable as a `dyn Asset`, which the
// `StandaloneAsset` bound requires.
impl_trait_accessible!(dyn Asset; ThemeOverrides);

#[cfg(test)]
mod tests {
    // Standard library
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};

    // External crates
    use pill_engine_core::asset_store::mount_directory;
    use pill_engine_core::{render_standalone, Asset, AssetGuid, AssetManager, World};

    // Current crate
    use crate::overrides::ThemeOverrides;
    use crate::register::{load_overrides, ThemePathScope};
    use crate::{register, THEME_SHARED_NAME};

    /// Held by every test that mounts the process-wide asset directory, so
    /// those tests run one at a time.
    static MOUNT_LOCK: Mutex<()> = Mutex::new(());

    /// A temporary directory mounted as the project's asset directory while
    /// the lock is held.
    struct ScratchAssetDirectory {
        root_directory: PathBuf,
        _mount_lock_guard: MutexGuard<'static, ()>,
    }

    impl ScratchAssetDirectory {
        fn new(directory_label: &str) -> Self {
            let mount_lock_guard = MOUNT_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let root_directory = std::env::temp_dir().join(format!(
                "pill-ui-theme-{directory_label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root_directory);
            std::fs::create_dir_all(root_directory.join("ui")).unwrap();
            mount_directory(&root_directory);
            Self {
                root_directory,
                _mount_lock_guard: mount_lock_guard,
            }
        }
    }

    impl Drop for ScratchAssetDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root_directory);
        }
    }

    #[test]
    fn the_shared_name_is_pinned() {
        assert_eq!(ThemeOverrides::shared_name(), Some(THEME_SHARED_NAME));
        assert_eq!(THEME_SHARED_NAME, "pill_ui_theme::ThemeOverrides");
    }

    #[test]
    fn registration_makes_theme_a_standalone_extension() {
        let mut world = World::new();
        register(&mut world);
        let registry = world
            .get_resource::<pill_engine_core::ImportRegistry>()
            .unwrap();
        assert!(registry.is_standalone_extension("theme"));
    }

    #[test]
    fn a_theme_file_round_trips_through_the_asset_store() {
        let scratch_directory = ScratchAssetDirectory::new("round-trip");
        let document = ThemeOverrides::from_yaml_str(
            "palette:\n  accent: \"#33d17a\"\nfonts:\n  body: fonts/body.ttf\n",
        )
        .unwrap();
        let bytes = render_standalone::<ThemeOverrides>(
            AssetGuid::random().expect("a fresh guid"),
            &document,
        )
        .expect("the document renders");
        std::fs::write(
            scratch_directory.root_directory.join("ui/accent.theme"),
            bytes,
        )
        .unwrap();

        let mut world = World::new();
        register(&mut world);
        let theme_handle =
            load_overrides(&mut world, "ui/accent.theme", ThemePathScope::Game).unwrap();
        let assets = world.get_resource::<AssetManager>().unwrap();
        let loaded_theme = assets.get(theme_handle).expect("the theme stays loaded");

        assert_eq!(
            serde_json::to_value(loaded_theme).unwrap(),
            serde_json::to_value(&document).unwrap()
        );
    }
}
