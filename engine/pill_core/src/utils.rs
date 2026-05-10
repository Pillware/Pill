use crate::{define_new_pill_slotmap_key, EngineError, Result};

use boolinator::Boolinator;
use std::{any::type_name, collections::HashMap, hash::Hash, path::Path};

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
pub fn validate_asset_path(path: &Path, allowed_formats: &'static [&'static str]) -> Result<()> {
    if !path.exists() {
        return Err(EngineError::InvalidAssetPath(path.display().to_string()).into());
    }

    match path.extension() {
        Some(v) => match allowed_formats.contains(&v.to_str().unwrap()) {
            true => Ok(()),
            false => Err(EngineError::InvalidAssetFormat(
                allowed_formats,
                v.to_str().unwrap().to_string(),
            )
            .into()),
        },
        None => Err(EngineError::InvalidAssetPath(path.display().to_string()).into()),
    }
}

// --- PillSlotMap utils ---

#[macro_export]
macro_rules! define_component_handle {
    ( $(#[$outer:meta])* $vis:vis struct $name:ident; $($rest:tt)* ) => {
        pill_core::define_new_pill_slotmap_key! {}
    };
}

// --- Other ---

#[inline]
pub fn get_game_error_message(result: Result<()>) -> Option<String> {
    result.err().map(|e| {
        use std::error::Error;
        let mut message = format!("Game error: {e}\n");
        let mut source = e.source();
        let mut i = 0usize;
        while let Some(s) = source {
            message.push_str(&format!("  {i}: {s}\n"));
            i += 1;
            source = s.source();
        }
        message
    })
}

#[macro_export]
macro_rules! create_game {
    ($game_contructor:expr, $game_trait:path) => {
        #[no_mangle]
        pub extern "C" fn get_game() -> *mut std::ffi::c_void {
            let game: Box<dyn $game_trait> = Box::new($game_contructor);
            Box::into_raw(Box::new(game)) as *mut std::ffi::c_void
        }

        pub fn create_pill_game() -> Box<dyn $game_trait> {
            Box::new($game_contructor)
        }
    };
}
