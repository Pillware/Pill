use std::any::type_name;

use colored::*;

pub fn get_type_name<T>() -> String {
    let full_type_name = type_name::<T>().to_string();
    let pure_type_name_start_index = full_type_name.rfind(':').unwrap() + 1;
    full_type_name[pure_type_name_start_index..].to_string()
}

pub fn get_value_type_name<T>(_: &T) -> String {
    let full_type_name = std::any::type_name::<T>().to_string();
    let pure_type_name_start_index = full_type_name.rfind(':').unwrap() + 1;
    full_type_name[pure_type_name_start_index..].to_string()
}

// Functions for changing the style of output string
pub trait PillStyle {
    fn gobj_style(self) -> ColoredString;
    fn sobj_style(self) -> ColoredString;
    fn name_style(self) -> ColoredString;
    fn err_style(self) -> ColoredString;
}

impl PillStyle for &str {
    // To be used with general objects (Scene, Component, System, Resource, etc) - changes color and adds bold
    fn gobj_style(self) -> ColoredString {
        self.color(colored::Color::BrightCyan)
    }

    // To be used with specific objects (CameraComponent, Texture, Mesh, etc)- changes color
    fn sobj_style(self) -> ColoredString {
        self.color(colored::Color::TrueColor { r: 95, g: 210, b: 90 }).bold()
    }

    // To be used with names - changes color adds quotation marks
    fn name_style(self) -> ColoredString {
        format!("\"{}\"", self).color(colored::Color::TrueColor { r: 190, g: 220, b: 160 })
    }

    // To be used with names - changes color adds bold
    fn err_style(self) -> ColoredString {
        self.color(colored::Color::Red).bold()
    }
}

