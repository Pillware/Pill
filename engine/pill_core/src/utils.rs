use crate::{EngineError, define_new_pill_slotmap_key};

use anyhow::{ Context, Result, Error };
use boolinator::Boolinator;
use colored::{ColoredString, Colorize};
use std::{ any::type_name, collections::HashMap, hash::Hash, path::PathBuf };
use log::debug;

// --- Type to string utils ---

pub const DISTINCT_COLOR_PALETTE: &[(f32, f32, f32)] = &[
    (0.894, 0.102, 0.110), // Red
    (0.215, 0.494, 0.721), // Blue
    (0.302, 0.686, 0.290), // Green
    (0.596, 0.306, 0.639), // Purple
    (1.000, 0.498, 0.000), // Orange
    (1.000, 1.000, 0.200), // Yellow
    (0.651, 0.337, 0.157), // Brown
    (0.969, 0.506, 0.749), // Pink
    (0.600, 0.600, 0.600), // Gray
    (0.100, 0.100, 0.100), // Near-black

    (0.000, 0.447, 0.698), // Deep blue
    (0.800, 0.475, 0.655), // Mauve
    (0.337, 0.705, 0.913), // Sky blue
    (0.000, 0.619, 0.451), // Teal
    (0.941, 0.894, 0.259), // Lemon
    (0.800, 0.725, 0.454), // Tan
    (0.792, 0.698, 0.839), // Lavender
    (0.984, 0.603, 0.600), // Salmon
    (0.541, 0.168, 0.886), // Indigo
    (0.125, 0.694, 0.298), // Bright green
];

pub fn generate_color_palette() -> Vec<(f32, f32, f32)> {
    (0..100).map(|i| {
        let hue = i as f32 / 100.0; // Evenly spaced hues
        hsl_to_rgb(hue, 0.6, 0.5)   // Saturation and lightness fixed
    }).collect()
}

// Convert HSL to RGB
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let a = s * l.min(1.0 - l);
    let f = |n: f32| {
        let k = (n + h * 12.0) % 12.0;
        l - a * (-((k - 3.0).abs() - 1.0).max(-1.0).min(1.0))
    };
    (f(0.0), f(8.0), f(4.0))
}

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
pub struct MeshX {
    pub name: String,
    pub path: PathBuf,
}

pub trait MeshXImpl {
    fn get_name(&self) -> String;
    fn get_path(&self) -> PathBuf;
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

#[macro_export]
macro_rules! create_game {
    ($game_contructor:expr, $game_trait:path) => {
        #[no_mangle]
        pub extern "C" fn get_game() -> *mut std::ffi::c_void {
            let game: Box<dyn $game_trait> = Box::new($game_contructor);
            Box::into_raw(Box::new(game)) as *mut std::ffi::c_void
        }
    };
}
