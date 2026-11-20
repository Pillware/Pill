//! The `.ttf`/`.otf` font a theme layer names.
//!
//! # Responsibilities
//!
//! - [`FontAsset`]: the font file's bytes as an asset, so a font a theme names
//!   is imported, versioned and re-imported when the file changes, like every
//!   other asset.
//! - [`FontAsset::bytes`]: the bytes a UI hands to egui's `set_fonts`.
//! - [`FONT_SHARED_NAME`]: the name that identifies the asset type across
//!   binaries.
//!
//! # Design
//!
//! A font file needs no import settings, so the asset is only the bytes,
//! copied out of the file. Going through the asset store is what lets a
//! changed font reach a running UI: when the file is imported again, the store
//! moves the font's content version, and both the game UI and the editor
//! install the fonts again without a restart. A project that read the file
//! itself would never notice the change.
//!
//! The bytes are an owned copy, never a borrow from the file or pack they came
//! from, so the asset stays valid after the module that loaded it is unloaded.

// External crates
use serde::{Deserialize, Serialize};
use trait_type_map::impl_trait_accessible;

// Current crate
use pill_engine_core::{Asset, AssetLoadError, AssetLoadResult, ImportedAsset};

// =============================================================================
// Constants
// =============================================================================

/// The name [`FontAsset`] is registered under, the same in every binary.
///
/// It is a literal for two reasons: the store keys one asset column across
/// binaries by it, and each font's `.meta` file records it, so it must not
/// move when the type or its module is renamed.
pub const FONT_SHARED_NAME: &str = "pill_ui_theme::FontAsset";

// =============================================================================
// FontAsset
// =============================================================================

/// A font file a theme layer named, as its bytes.
///
/// The type is an asset so the store can version it and load it again when the
/// file changes; a UI copies [`Self::bytes`] into egui when it installs the
/// fonts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontAsset {
    /// The font file's bytes, copied unchanged.
    pub bytes: Vec<u8>,
}

impl Asset for FontAsset {
    fn shared_name() -> Option<&'static str> {
        Some(FONT_SHARED_NAME)
    }
}

// Lets the asset manager keep a `FontAsset` column: it stores assets as
// `dyn Asset` trait objects, so each asset type declares this access.
impl_trait_accessible!(dyn Asset; FontAsset);

// =============================================================================
// FontImportSettings
// =============================================================================

/// The import settings a font's `.meta` file holds.
///
/// There is nothing to configure, because the bytes say everything about the
/// font, but an imported asset needs a settings type. The `.meta` file is also
/// where the font's guid lives, and that guid keeps the font's identity when
/// the file is renamed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontImportSettings {}

impl ImportedAsset for FontAsset {
    type ImportSettings = FontImportSettings;
    const SOURCE_EXTENSIONS: &'static [&'static str] = &["ttf", "otf"];

    /// Copies the file's bytes into the asset.
    ///
    /// An empty file is refused here rather than left for `set_fonts`: epaint
    /// panics while parsing empty font data, and the error returned here names
    /// the file instead.
    ///
    /// # Errors
    ///
    /// [`AssetLoadError::Decode`] when the file holds no bytes.
    fn import(
        name: &str,
        source_bytes: &[u8],
        _settings: &FontImportSettings,
    ) -> AssetLoadResult<Self> {
        if source_bytes.is_empty() {
            return Err(AssetLoadError::Decode {
                label: name.to_owned(),
                detail: "the file is empty".to_owned(),
            });
        }
        Ok(Self {
            bytes: source_bytes.to_vec(),
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_font_asset_is_the_files_bytes() {
        let font = FontAsset::import("fonts/a.ttf", b"\x00\x01\x00\x00ttf", &Default::default())
            .expect("the bytes are a font");
        assert_eq!(font.bytes, b"\x00\x01\x00\x00ttf");
        assert_eq!(FontAsset::shared_name(), Some(FONT_SHARED_NAME));
    }

    #[test]
    fn an_empty_file_is_refused_with_its_name() {
        let error = FontAsset::import("fonts/empty.ttf", b"", &Default::default())
            .expect_err("an empty font is refused");
        assert!(error.to_string().contains("fonts/empty.ttf"), "{error}");
    }
}
