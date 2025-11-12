use colored::{ColoredString, Colorize};

// Functions for changing the style of output string
pub trait PillStyle {
    fn module_object_style(self) -> ColoredString;
    fn general_object_style(self) -> ColoredString;
    fn specific_object_style(self) -> ColoredString;
    fn name_style(self) -> ColoredString;
    fn error_style(self) -> ColoredString;
    fn warn_style(self) -> ColoredString;
    fn debug_style(self) -> ColoredString;
}

impl PillStyle for &str {
    // To be used with large module objects (Engine, Renderer, Window, etc) - changes color and adds bold
    #[inline]
    fn module_object_style(self) -> ColoredString {
        self.color(colored::Color::TrueColor { r: 180, g: 25, b: 100 }).bold()
    }

    // To be used with general objects (Scene, Component, System, Resource, etc) - changes color and adds bold
    #[inline]
    fn general_object_style(self) -> ColoredString {
        self.color(colored::Color::BrightCyan)
    }

    // To be used with specific objects (CameraComponent, Texture, Mesh, etc) - changes color
    #[inline]
    fn specific_object_style(self) -> ColoredString {
        self.color(colored::Color::TrueColor { r: 95, g: 210, b: 90 })
    }

    // To be used with names - changes color adds quotation marks
    #[inline]
    fn name_style(self) -> ColoredString {
        format!("\"{}\"", self).color(colored::Color::TrueColor { r: 190, g: 220, b: 160 })
    }

    // To be used with names - changes color adds bold
    #[inline]
    fn error_style(self) -> ColoredString {
        self.color(colored::Color::Red).bold()
    }

    // Warning
    #[inline]
    fn warn_style(self) -> ColoredString {
        self.color(colored::Color::Yellow).bold()
    }

    // Debug
    #[inline]
    fn debug_style(self) -> ColoredString {
        self.color(colored::Color::Blue).bold()
    }
}