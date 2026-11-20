//! Registration, path scopes and the loaders.
//!
//! # Responsibilities
//!
//! - [`register()`]: declares the `.theme` standalone asset and the font asset
//!   on a world.
//! - [`ThemePathScope`]: the game/editor path rule. A game never loads editor
//!   content, and an editor layer only loads from `editor/`.
//! - [`load_overrides`]: imports a `.theme` file and returns its handle.
//! - [`load_font`]: imports the font a theme layer named and returns its
//!   handle with the store's content version, which a caller compares to
//!   notice an edited file.
//!
//! # Design
//!
//! The scope check runs before the asset store sees the path, so a path
//! outside the caller's scope is refused with a message about the rule, not as
//! a filesystem error. Both scopes go through `checked_relative`. It refuses a
//! path that is absolute, climbs out with `..`, or is empty, before the store
//! resolves it.

// Standard library
use std::path::Path;

// Current crate
use pill_engine_core::asset_metadata::checked_relative;
use pill_engine_core::{AssetManager, Handle, World};

use crate::font::FontAsset;
use crate::overrides::ThemeOverrides;

// =============================================================================
// Path scopes
// =============================================================================

/// Which part of a project's `res` a theme path may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePathScope {
    /// A game's own theme may name anything below `res` except `editor/`.
    Game,
    /// An editor layer may name only files under `res/editor/`.
    Editor,
}

impl ThemePathScope {
    /// Checks `path` against this scope's rule and returns it in normalized
    /// form.
    ///
    /// # Errors
    ///
    /// A message saying which rule the path breaks: a game naming
    /// `editor/...`, an editor layer naming something outside it, or a path
    /// that is absolute, climbs out with `..`, or is empty.
    pub fn check(self, path: &str) -> Result<String, String> {
        let normalized_path = checked_relative(path)?;
        let asset_path = normalized_path.to_string_lossy().replace('\\', "/");
        let under_editor = asset_path == "editor" || asset_path.starts_with("editor/");
        match self {
            Self::Game if under_editor => Err(format!(
                "`{path}` is editor-only content; a game theme must be outside `editor/`"
            )),
            Self::Editor if !under_editor => Err(format!(
                "`{path}` must be under `editor/` for an editor theme"
            )),
            _ => Ok(asset_path),
        }
    }
}

// =============================================================================
// Registration and loaders
// =============================================================================

/// A font a theme layer named, loaded.
pub struct LoadedFont {
    /// The font's handle in the asset store.
    pub handle: Handle<FontAsset>,
    /// The store's content version for the font. It changes whenever the
    /// store's copy changes, so a caller compares it instead of hashing the
    /// bytes.
    pub version: Option<u64>,
}

/// Declare the `.theme` standalone asset and the font asset on `world`.
///
/// Call it from the registration of the crate that owns the theme: a project's
/// `install_with_theme`, or the editor's registration. Registering the same
/// type from several binaries is supported. The newest live registration
/// serves the type's import entry points, and an older one drops out when its
/// binary is gone.
pub fn register(world: &mut World) {
    world.register_standalone_asset::<ThemeOverrides>();
    world.register_imported_asset::<FontAsset>();
}

/// Import the `.theme` file at `path` and return its handle.
///
/// `path` is below the project's asset directory; `scope` decides whether a
/// game or an editor layer may name it.
///
/// # Errors
///
/// The scope rule's message, or a message saying the world has no
/// `AssetManager`. The file itself can also fail: it could not be read, is not
/// a theme file, or does not fit the override document. A file whose guid
/// already belongs to another loaded theme is refused too.
pub fn load_overrides(
    world: &mut World,
    path: &str,
    scope: ThemePathScope,
) -> Result<Handle<ThemeOverrides>, String> {
    let path = scope.check(path)?;
    let assets = world
        .get_resource_mut::<AssetManager>()
        .ok_or_else(|| "the engine has no AssetManager".to_string())?;
    let import_outcome = assets
        .import_standalone::<ThemeOverrides>(Path::new(&path))
        .map_err(|error| format!("`{path}` did not load: {error}"))?;
    Ok(import_outcome.handle)
}

/// Import the font a theme layer named and return its handle.
///
/// `path` is below the project's asset directory and follows the same scope
/// rule as [`load_overrides`]. A font file with no `.meta` gets one when its
/// directory can be written, so a font named by the code defaults is usable
/// from the first run.
///
/// # Errors
///
/// The scope rule's message. Loading also fails when [`register`] was not
/// called, which leaves no asset types registered, or when the world has no
/// `AssetManager`. The file itself can fail: it could not be read or is empty,
/// or the path names an asset of another type. A font whose guid another
/// loaded font already owns is refused as well.
pub fn load_font(
    world: &mut World,
    path: &str,
    scope: ThemePathScope,
) -> Result<LoadedFont, String> {
    let path = scope.check(path)?;
    let handle = world
        .load_asset::<FontAsset>(&path)
        .map_err(|error| format!("font `{path}` did not load: {error}"))?;
    let version = world
        .get_resource::<AssetManager>()
        .and_then(|assets| assets.content_version(handle));
    Ok(LoadedFont { handle, version })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::ThemePathScope;

    #[test]
    fn a_game_may_not_name_editor_content() {
        assert_eq!(
            ThemePathScope::Game.check("ui/a.theme").unwrap(),
            "ui/a.theme"
        );
        let refused = ThemePathScope::Game.check("editor/a.theme").unwrap_err();
        assert!(refused.contains("editor/"), "{refused}");
        assert!(ThemePathScope::Game.check("editor").is_err());
    }

    #[test]
    fn an_editor_layer_must_name_editor_content() {
        assert_eq!(
            ThemePathScope::Editor.check("editor/a.theme").unwrap(),
            "editor/a.theme"
        );
        let refused = ThemePathScope::Editor.check("ui/a.theme").unwrap_err();
        assert!(refused.contains("editor/"), "{refused}");
    }

    #[test]
    fn both_scopes_refuse_paths_outside_the_asset_directory() {
        for scope in [ThemePathScope::Game, ThemePathScope::Editor] {
            assert!(scope.check("../a.theme").is_err());
            assert!(scope.check("C:/a.theme").is_err());
            assert!(scope.check("/a.theme").is_err());
        }
    }

    #[test]
    fn a_leading_dot_segment_is_normalized_away() {
        assert_eq!(
            ThemePathScope::Game.check("./ui/a.theme").unwrap(),
            "ui/a.theme"
        );
    }
}
