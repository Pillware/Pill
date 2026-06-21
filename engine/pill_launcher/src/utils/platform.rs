// This file provides OS-specific constants and the dynamic library naming helper.

/// Executable file extension (e.g. ".exe" on Windows, "" on Linux/macOS).
#[cfg(target_os = "windows")]
pub(crate) const EXECUTABLE_SUFFIX: &str = ".exe";
#[cfg(not(target_os = "windows"))]
pub(crate) const EXECUTABLE_SUFFIX: &str = ""; // Linux, macOS, etc. – no extension

#[cfg(target_os = "windows")]
pub(crate) const DYNAMIC_LIBRARY_PREFIX: &str = ""; //  pill_game.dll
#[cfg(not(target_os = "windows"))]
pub(crate) const DYNAMIC_LIBRARY_PREFIX: &str = "lib"; //  libpill_game.so / .dylib

#[cfg(target_os = "windows")]
pub(crate) const DYNAMIC_LIBRARY_SUFFIX: &str = ".dll";
#[cfg(target_os = "linux")]
pub(crate) const DYNAMIC_LIBRARY_SUFFIX: &str = ".so";
#[cfg(target_os = "macos")]
pub(crate) const DYNAMIC_LIBRARY_SUFFIX: &str = ".dylib";

pub(crate) fn dynamic_library_name(name: &str) -> String {
    format!("{DYNAMIC_LIBRARY_PREFIX}{name}{DYNAMIC_LIBRARY_SUFFIX}")
}
