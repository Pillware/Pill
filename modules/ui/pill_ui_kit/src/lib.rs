//! The UI kit: converts a theme's tokens into egui's style and fonts, and
//! holds the widget looks that are built on those same tokens.
//!
//! # Responsibilities
//!
//! - [`style`]: [`build_style`] fills in an [`egui::Style`] from a theme, and
//!   [`font_definitions`] builds the font definitions from the theme's fonts
//!   and the caller's own. [`install`] puts both on an egui context, the fonts
//!   first, and then pins the dark theme. [`FontSignature`] is the record the
//!   caller keeps, so the fonts are set again only when they changed.
//! - [`variants`]: `primary`, `secondary` and `danger`, three looks for stock
//!   egui widgets that the caller applies inside a `ui.scope`.
//! - [`widgets`]: [`ChunkyButton`], a button whose face sits on a hard shadow
//!   with no blur.
//!
//! # Design
//!
//! The kit is an `rlib` compiled into both UI dylibs (`pill_editor_core` and
//! `pill_project_ui`). It therefore runs in the same DLL as the egui `Context`
//! it writes to, and no egui value crosses a DLL boundary. Only those two
//! crates may depend on it, and each re-exports it as `kit`. Its dependencies
//! are exactly `egui` and `pill_ui_theme`. A widget that needs more belongs in
//! a separate crate, not in this one.

/// The theme's style and fonts, and how to install them on a context.
pub mod style;
/// Three looks for stock egui widgets: `primary`, `secondary` and `danger`.
pub mod variants;
/// The chunky button: a flat face on a hard shadow with no blur.
pub mod widgets;

pub use style::{
    build_style, font_definitions, install, FontSignature, ThemeFonts, DISPLAY_FAMILY,
};
pub use widgets::ChunkyButton;
pub use widgets::SHADOW_OFFSET;
