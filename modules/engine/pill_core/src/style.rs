//! String styling helpers for terminal output.
//!
//! # Responsibilities
//!
//! - Own the [`PillStyle`] trait that maps semantic output roles (module
//!   objects, general objects, specific objects, names, and severity
//!   levels) to styled strings for terminal rendering.
//!
//! # Design
//!
//! The trait states the vocabulary. The platform layer supplies the
//! implementation for the target. The `&str` implementations live in
//! [`crate::platform::styling`], beside the other target splits, so this
//! module holds no `cfg(target_arch)` gate and owns only the trait.
//!
//! There is one implementation per target. On native it colors the text
//! through the `colored` crate; module objects and the severity levels also
//! gain bold weight. On `wasm32` it returns plain text, keeping the quotation
//! marks around names.
//!
//! The trait is re-exported at the crate root and is what [`error`]'s
//! terminal renderer styles through.
//!
//! [`error`]: crate::error

// =============================================================================
// PillStyle trait
// =============================================================================

/// Maps semantic output roles to styled strings for terminal rendering.
///
/// Every role gets its own style. The roles are module objects, general
/// objects, specific objects, names, and the severity levels (error, warning
/// and debug). Implementors decide how a role is displayed.
///
/// The crate provides one implementation for `&str` per target. On native it
/// colors the text, and some roles also gain bold weight. On `wasm32` it
/// returns plain text, with names still in quotation marks.
///
/// # Examples
///
/// ```
/// use pill_core::PillStyle;
///
/// let styled = "Engine".module_object_style();
/// assert!(!styled.is_empty());
/// ```
pub trait PillStyle {
    /// Style a large module object (Engine, Renderer, Window, and so on).
    ///
    /// Changes the color and adds bold weight.
    fn module_object_style(self) -> String;

    /// Style a general object (Scene, Component, System, Resource, and so on).
    ///
    /// Changes the color.
    fn general_object_style(self) -> String;

    /// Style a specific object (CameraComponent, Texture, Mesh, and so on).
    ///
    /// Changes the color.
    fn specific_object_style(self) -> String;

    /// Style a name, changing the color and adding quotation marks.
    fn name_style(self) -> String;

    /// Style an error message, changing the color and adding bold weight.
    fn error_style(self) -> String;

    /// Style a warning message, changing the color and adding bold weight.
    fn warn_style(self) -> String;

    /// Style a debug message, changing the color and adding bold weight.
    fn debug_style(self) -> String;
}
