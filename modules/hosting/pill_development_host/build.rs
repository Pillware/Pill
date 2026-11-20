//! Record the cargo profile and target triple this host is compiled into.
//!
//! # Responsibilities
//!
//! - Emit `PILL_HOST_PROFILE_DIRECTORY`, the profile directory the host was
//!   built into, so the project and extension builds it starts write to the
//!   same directory.
//! - Emit `PILL_HOST_TARGET_TRIPLE`, empty for a native build, so
//!   host-spawned module builds can pass the `--target` a launcher gave the
//!   host.
//!
//! # Design
//!
//! The host runs cargo for the project and for every extension, and those
//! builds must use the profile the host itself was built with. A release host
//! that loads debug-profile modules fails in `LoadLibrary` with "The
//! specified procedure could not be found" (os error 127), because the two
//! sides resolve different crate-metadata hashes. The same hashing makes a
//! natively built module unloadable by a host that cargo built with
//! `--target`.
//!
//! `cfg!(debug_assertions)` cannot answer this: `release-with-debug` inherits
//! `release`, so its assertions stay off, yet it builds into
//! `target/release-with-debug`. `OUT_DIR` always points into the real profile
//! directory: cargo shapes it as
//! `<target>/[<triple>/]<profile>/build/<package>-<hash>/out`, so the script
//! reads the profile from there.

// Standard library
use std::path::{Path, PathBuf};

// =============================================================================
// Constants
// =============================================================================

/// Profile directory assumed when `OUT_DIR` cannot be parsed.
///
/// `debug` is the safe fallback: an ordinary `cargo build` writes there, so a
/// host that falls back still matches the default build.
const FALLBACK_PROFILE_DIRECTORY: &str = "debug";

// =============================================================================
// Free Functions
// =============================================================================

/// Emits the host's profile directory and target triple as `rustc-env` values.
fn main() {
    // Step 1: Declare the inputs. The workspace manifest holds the
    // `[profile.*]` sections, and its path must name a file that exists:
    // cargo treats a missing path as changed on every build, so this script
    // would rerun each time.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../Cargo.toml");

    // Step 2: Read the layout from `OUT_DIR`, falling back to the default
    // build profile.
    let (profile_directory, target_triple) = out_directory_layout();
    let profile_directory =
        profile_directory.unwrap_or_else(|| FALLBACK_PROFILE_DIRECTORY.to_string());
    println!("cargo:rustc-env=PILL_HOST_PROFILE_DIRECTORY={profile_directory}");

    // Empty when the host was built natively (no `--target`), which is the
    // usual `cargo run` case. Set to the triple when a tool such as the
    // dioxus CLI passed `--target <host>` explicitly.
    println!("cargo:rustc-env=PILL_HOST_TARGET_TRIPLE={target_triple}");
}

/// Reads the profile directory and the target triple out of `OUT_DIR`.
///
/// `OUT_DIR` has the shape
/// `<target-root>/[<triple>/]<profile>/build/<package>-<hash>/out`, so the
/// profile directory is three folders up. The folder above it is the target
/// root in a native build and the triple in a `--target` build, and its name
/// decides which. A natively built module loaded into a `--target` host fails
/// only when it is loaded, with a `LoadLibrary` "procedure not found" error.
fn out_directory_layout() -> (Option<String>, String) {
    // Step 1: Read `OUT_DIR`; without it there is nothing to report, so
    // return no profile directory and an empty triple.
    let Some(out_directory) = std::env::var_os("OUT_DIR").map(PathBuf::from) else {
        return (None, String::new());
    };

    // Step 2: Walk up from `build/<package>-<hash>/out` to the profile
    // directory and the folder above it.
    let package_build_directory = out_directory.parent();
    let build_directory = package_build_directory.and_then(Path::parent);
    let profile_directory = build_directory.and_then(Path::parent);
    let target_root_directory = profile_directory.and_then(Path::parent);
    let profile_directory_name = profile_directory
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned());

    // Step 3: Take the folder above the profile directory as the target triple
    // only when its name contains a hyphen. Every rustc target triple has at
    // least one (`x86_64-pc-windows-msvc`, `aarch64-apple-darwin`,
    // `wasm32-unknown-unknown`), and the workspace's `target` folder has none.
    // A `CARGO_TARGET_DIR` whose own folder name contains a hyphen would be
    // mistaken for a triple; the editor and host builds here do not set it.
    let target_triple = target_root_directory
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.contains('-'))
        .unwrap_or_default();
    (profile_directory_name, target_triple)
}
