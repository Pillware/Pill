pub trait PillStyle {
    fn module_object_style(self) -> String;
    fn general_object_style(self) -> String;
    fn specific_object_style(self) -> String;
    fn name_style(self) -> String;
    fn error_style(self) -> String;
    fn warn_style(self) -> String;
    fn debug_style(self) -> String;
}

#[cfg(not(target_arch = "wasm32"))]
impl PillStyle for &str {
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

    #[inline]
    fn general_object_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::BrightCyan).to_string()
    }

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

    #[inline]
    fn error_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::Red).bold().to_string()
    }

    #[inline]
    fn warn_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::Yellow).bold().to_string()
    }

    #[inline]
    fn debug_style(self) -> String {
        use colored::Colorize;
        self.color(colored::Color::Blue).bold().to_string()
    }
}

#[cfg(target_arch = "wasm32")]
impl PillStyle for &str {
    #[inline]
    fn module_object_style(self) -> String {
        self.to_string()
    }
    #[inline]
    fn general_object_style(self) -> String {
        self.to_string()
    }
    #[inline]
    fn specific_object_style(self) -> String {
        self.to_string()
    }
    #[inline]
    fn name_style(self) -> String {
        format!("\"{}\"", self)
    }
    #[inline]
    fn error_style(self) -> String {
        self.to_string()
    }
    #[inline]
    fn warn_style(self) -> String {
        self.to_string()
    }
    #[inline]
    fn debug_style(self) -> String {
        self.to_string()
    }
}
