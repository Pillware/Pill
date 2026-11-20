//! The UI theme pipeline's plain data.
//!
//! # Responsibilities
//!
//! - [`theme`]: the complete base [`Theme`] and its two built-in themes,
//!   [`Theme::fallback`] (neutral dark) and [`Theme::brutalist`], plus the
//!   merge that applies an override document to one.
//! - [`overrides`]: [`ThemeOverrides`], the document a `.theme` file holds,
//!   which lists only the items it changes, and the reader that parses it.
//! - [`asset`]: the `.theme` asset type, which carries its own header and
//!   guid, so it needs no `.meta` file.
//! - [`font`]: the `.ttf`/`.otf` font a theme names, loaded as an asset.
//! - [`register`](mod@register): declares both asset types on a world, applies
//!   the game/editor path rule, and loads a theme or font, returning its
//!   handle.
//!
//! # Design
//!
//! This crate is a shared `dylib`: a project, `pill_project_ui`, the editor
//! shell and `pill_editor_core` share the `Theme`, `ThemeOverrides` and
//! `FontAsset` types and one asset column in the world's asset manager.
//!
//! The crate is egui-free. The translator that builds egui's `Style` from a
//! `Theme` lives in `pill_ui_kit`, compiled into each UI's link unit, so no
//! egui value ever crosses this DLL boundary: every UI has its own copy of
//! egui, and the copies are not interchangeable.
//!
//! A theme is always complete: a `.theme` file carries **only overrides**, and
//! the merge starts from a complete base theme, never from egui's defaults, so
//! an item the file leaves out keeps the base value.

/// The `.theme` asset type, which carries its own header and guid, so it needs
/// no `.meta` file.
pub mod asset;
/// The `.ttf`/`.otf` font a theme names, loaded as an asset.
pub mod font;
/// The document a `.theme` file holds, which lists only the items it changes,
/// and the reader that parses it.
pub mod overrides;
/// Puts the theme asset types on a world, then loads a theme or font within the
/// game or editor path rule.
pub mod register;
/// The complete theme values, and the merge that applies an override document
/// to a base theme.
pub mod theme;

pub use asset::THEME_SHARED_NAME;
pub use font::{FontAsset, FontImportSettings, FONT_SHARED_NAME};
pub use overrides::ThemeOverrides;
pub use register::{load_font, load_overrides, register, LoadedFont, ThemePathScope};
pub use theme::{
    FontFamilyName, Palette, Rgba, Shadow, Shape, Spacing, Stroke, TextStyle, Theme, ThemeError,
    ThemeFontPaths, WidgetStates, WidgetStyle,
};
