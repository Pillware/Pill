//! Stage the toolchain's standard-library dylib next to `pill_standalone.exe`.
//!
//! # Responsibilities
//!
//! - Stage the std dylib through the shared `frontends/build_support` staging
//!   code.
//! - Refuse a non-debug build that enables `development`.
//!
//! # Design
//!
//! A development build links with `-C prefer-dynamic`, so this executable
//! imports `std-<hash>.dll` when the process starts. That dylib sits in the
//! toolchain's `lib/rustlib/<host>/lib` directory, which Windows does not
//! search. The copy staged beside the executable is what lets the binary
//! start without the toolchain directory on `PATH`; without it, the process
//! fails with `STATUS_DLL_NOT_FOUND`. The staging does nothing off Windows.
//!
//! A release-lineage profile links everything into one binary and is always
//! the shipping mode, so this script refuses the `development` feature there.
//! The binary itself refuses a build with neither mode or with both.

// The shared staging code every executable's build script includes.
#[path = "../build_support/stage_std_dylib.rs"]
mod stage_std_dylib;

fn main() {
    // Refused here rather than in the binary, which cannot see the profile. A
    // release-lineage build is always the shipping mode, so the `development`
    // posture is refused there. Cargo passes the profile as `PROFILE` and each
    // enabled feature as `CARGO_FEATURE_*`.
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let development_enabled = std::env::var_os("CARGO_FEATURE_DEVELOPMENT").is_some();
    if profile != "debug" && development_enabled {
        panic!(
            "pill_standalone cannot be built with `development` in the `{profile}` profile: \
             a release-lineage build is always the shipping mode. Build it as \
             `--no-default-features --features shipping`."
        );
    }

    // Copy `std-*.dll` next to the built executable (no-op off Windows).
    stage_std_dylib::stage_std_dylib();
}
