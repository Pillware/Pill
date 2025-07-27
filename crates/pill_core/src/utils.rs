use crate::{EngineError, define_new_pill_slotmap_key};

use anyhow::{ Context, Result, Error };
use boolinator::Boolinator;
use std::{ any::type_name, path::PathBuf };
use colored::*;
use log::debug;

// --- Type to string utils ---

// E.g. pill_core::get_type_name::<Resource>(); will return "Resource"
pub fn get_type_name<T>() -> String {
    let full_type_name = type_name::<T>().to_string();
    let pure_type_name_start_index = full_type_name.rfind(':').unwrap() + 1;
    full_type_name[pure_type_name_start_index..].to_string()
}

pub fn get_value_type_name<T>(_: &T) -> String {
    let full_type_name = type_name::<T>().to_string();
    let pure_type_name_start_index = full_type_name.rfind(':').unwrap() + 1;
    full_type_name[pure_type_name_start_index..].to_string()
}

// Returns true if enum variants are the same and false if not
pub fn enum_variant_eq<T>(a: &T, b: &T) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

// Returns only the name of enum variant
// E.g. pill_core::get_enum_variant_type_name(MyEnum::Hello(88)); will return "Hello"
pub fn get_enum_variant_type_name<T: core::fmt::Debug>(a: &T) -> String {
    let full_type_name = format!("{:?}", a);
    let pure_type_name_end_index = full_type_name.rfind('(');
    match pure_type_name_end_index {
        Some(v) => full_type_name[..v].to_string(),
        None => full_type_name.to_string(),
    }
}

// --- String style utils ---

// Functions for changing the style of output string
pub trait PillStyle {
    fn mobj_style(self) -> ColoredString;
    fn gobj_style(self) -> ColoredString;
    fn sobj_style(self) -> ColoredString;
    fn name_style(self) -> ColoredString;
    fn err_style(self) -> ColoredString;
}

impl PillStyle for &str {
    // To be used with large module objects (Engine, Renderer, Window, etc) - changes color and adds bold
    #[inline]
    fn mobj_style(self) -> ColoredString {
        self.color(colored::Color::TrueColor { r: 180, g: 25, b: 100 }).bold()
    }

    // To be used with general objects (Scene, Component, System, Resource, etc) - changes color and adds bold
    #[inline]
    fn gobj_style(self) -> ColoredString {
        self.color(colored::Color::BrightCyan)
    }

    // To be used with specific objects (CameraComponent, Texture, Mesh, etc) - changes color
    #[inline]
    fn sobj_style(self) -> ColoredString {
        self.color(colored::Color::TrueColor { r: 95, g: 210, b: 90 })
    }

    // To be used with names - changes color adds quotation marks
    #[inline]
    fn name_style(self) -> ColoredString {
        format!("\"{}\"", self).color(colored::Color::TrueColor { r: 190, g: 220, b: 160 })
    }

    // To be used with names - changes color adds bold
    #[inline]
    fn err_style(self) -> ColoredString {
        self.color(colored::Color::Red).bold()
    }
}

// --- Path utils ---

// Check if path to asset is correct (exists and has supported format)
pub fn validate_asset_path(path: &PathBuf, allowed_formats: &'static [&'static str]) -> Result<()> // Vec<String>
{
    path.exists().ok_or(Error::new(EngineError::InvalidAssetPath(path.display().to_string())))?;

    match path.extension() {
        Some(v) => match allowed_formats.contains(&v.to_str().unwrap()) { //} v.eq(allowed_format) {
            true => return Ok(()),
            false => return Err(Error::new(EngineError::InvalidAssetFormat(allowed_formats, v.to_str().unwrap().to_string()))),
        },
        None => return Err(Error::new(EngineError::InvalidAssetPath(path.display().to_string()))),
    }
}

// --- PillSlotMap utils ---

#[macro_export] macro_rules! define_component_handle { 
    ( $(#[$outer:meta])* $vis:vis struct $name:ident; $($rest:tt)* ) => {
        pill_core::define_new_pill_slotmap_key! { }
    }; 
}

// --- Time tracking ---

pub struct Timer {
    start: std::time::Instant,
    context_name: String,
}

impl Timer {
    pub fn new(context_name: &str) -> Self {
        Self {
            start: std::time::Instant::now(),
            context_name: context_name.to_string(),
        }
    }

    pub fn lap(&mut self, label: &str) {
        debug!("{} - Stage: {} took {:.3} ms", self.context_name, label, self.start.elapsed().as_secs_f32() * 1000.0);
        self.start = std::time::Instant::now();
    }
}

// --- Other ---

#[inline]
pub fn get_game_error_message(result: Result<()>) -> Option<String> {
    if result.is_err() { 
        let mut message = String::new();
        for (i, error) in result.err().unwrap().chain().enumerate() {
            let message_part = match i == 0 {
                true => format!("Game error: {} \n", error),
                false => format!("  {}: {} \n", i - 1, error),
            };
            message.push_str(message_part.as_str());
        }
        Some(message)
    }
    else {
        None
    }
}
