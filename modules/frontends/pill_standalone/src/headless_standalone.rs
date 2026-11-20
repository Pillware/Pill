//! The headless run loop: no window and no renderer.
//!
//! # Responsibilities
//!
//! - Run the project one frame after another until the process exits.
//! - Report setup failures, the only failure a headless run can have.
//!
//! # Design
//!
//! The loop is written against [`FrameDriver`], so it runs whichever driver
//! [`crate::posture`] compiled. A headless frame cannot fail, so [`RunError`]
//! converts the driver's `Infallible` error, and the loop never returns.
//!
//! This file is compiled only in a headless build: `main.rs` declares it
//! instead of `windowed_standalone` while the `rendering` feature is off.

// External crates
use pill_player::FrameDriver;

// Current crate
use crate::error::RunError;
use crate::posture;

// =============================================================================
// Free Functions
// =============================================================================

/// Runs the project continuously without creating a native window.
///
/// Setup is the only step that can fail: a headless driver's frame cannot
/// fail, so once the driver exists the loop runs until the process exits.
///
/// # Errors
///
/// Returns [`RunError`] if setup fails, such as when the project cannot be
/// built, loaded or initialized.
pub fn run(project: posture::ProjectSource) -> Result<(), RunError> {
    // Step 1: set up the driver this build's posture compiled.
    let mut driver = posture::setup_driver(project)?;
    // Step 2: run a frame at a time. Only setup can fail, so the loop never
    // ends on its own.
    loop {
        driver.run_frame()?;
    }
}
