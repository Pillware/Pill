use std::any::type_name;

pub fn get_type_name<T>() -> String {
    let full_type_name = type_name::<T>().to_string();
    let pure_type_name_start_index = full_type_name.rfind(':').unwrap() + 1;
    full_type_name[pure_type_name_start_index..].to_string()
}