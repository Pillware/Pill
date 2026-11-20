//! Terminal styling for `&str` on the native and web targets.
//!
//! # Responsibilities
//!
//! - Implement [`PillStyle`](crate::style::PillStyle) for `&str`, colored and
//!   bold through the `colored` crate on native targets.
//! - Provide the same methods on `wasm32` as plain text, with a name still put
//!   in quotation marks.
//!
//! # Design
//!
//! Only one of the two implementations is compiled, so they sit here beside
//! the other modules that differ per target, not next to the trait. This layer
//! holds the crate's only `cfg(target_arch)` gates, so a reader looking for
//! per-target behaviour finds all of it here.

// =============================================================================
// PillStyle for &str (native, colored)
// =============================================================================

/// Applies ANSI styling on native targets through the `colored` crate.
///
/// Each method colors the text for its role. Module objects, errors, warnings
/// and debug messages also become bold.
///
/// The `colored` crate writes the escape codes only when it decides the output
/// is a terminal, so text redirected to a file usually comes back plain.
#[cfg(not(target_arch = "wasm32"))]
impl crate::style::PillStyle for &str {
    /// Colors a large module object (Engine, Renderer, Window, and so on) and
    /// makes it bold.
    #[inline]
    fn module_object_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::TrueColor {
            r: 180,
            g: 25,
            b: 100,
        })
        .bold()
        .to_string()
    }

    /// Colors a general object (Scene, Component, System, Resource, and so on).
    #[inline]
    fn general_object_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::BrightCyan).to_string()
    }

    /// Colors a specific object (CameraComponent, Texture, Mesh, and so on).
    #[inline]
    fn specific_object_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::TrueColor {
            r: 95,
            g: 210,
            b: 90,
        })
        .to_string()
    }

    /// Puts a name in quotation marks and colors the quoted text.
    #[inline]
    fn name_style(self) -> String {
        use colored::Colorize;
        format!("\"{}\"", self)
            .color(colored::Color::TrueColor {
                r: 190,
                g: 220,
                b: 160,
            })
            .to_string()
    }

    /// Colors an error message and makes it bold.
    #[inline]
    fn error_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::Red).bold().to_string()
    }

    /// Colors a warning message and makes it bold.
    #[inline]
    fn warn_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::Yellow).bold().to_string()
    }

    /// Colors a debug message and makes it bold.
    #[inline]
    fn debug_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::Blue).bold().to_string()
    }
}

// =============================================================================
// PillStyle for &str (wasm32, plain)
// =============================================================================

/// Plain-text styling for `wasm32`, which has no terminal.
///
/// Every method returns the text unchanged, so code that styles terminal
/// output still compiles on the web. The one exception is a name, which still
/// gains quotation marks.
#[cfg(target_arch = "wasm32")]
impl crate::style::PillStyle for &str {
    /// Returns a large module object unchanged.
    #[inline]
    fn module_object_style(self) -> String {
        self.to_string()
    }

    /// Returns a general object unchanged.
    #[inline]
    fn general_object_style(self) -> String {
        self.to_string()
    }

    /// Returns a specific object unchanged.
    #[inline]
    fn specific_object_style(self) -> String {
        self.to_string()
    }

    /// Puts a name in quotation marks.
    #[inline]
    fn name_style(self) -> String {
        format!("\"{}\"", self)
    }

    /// Returns an error message unchanged.
    #[inline]
    fn error_style(self) -> String {
        self.to_string()
    }

    /// Returns a warning message unchanged.
    #[inline]
    fn warn_style(self) -> String {
        self.to_string()
    }

    /// Returns a debug message unchanged.
    #[inline]
    fn debug_style(self) -> String {
        self.to_string()
    }
}
