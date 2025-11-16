use crate::{EngineError, define_new_pill_slotmap_key};

use anyhow::{ Context, Result, Error };
use boolinator::Boolinator;
use std::{ any::type_name, collections::HashMap, hash::Hash, path::PathBuf };

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
    let mut name = full_type_name.split('(').next().unwrap_or(&full_type_name);
    name = name.trim(); // in case there's space
    name.to_string()
}

// --- Path utils ---

// Check if path to asset is correct (exists and has supported format)
pub fn validate_asset_path(path: &PathBuf, allowed_formats: &'static [&'static str]) -> Result<()> {
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
