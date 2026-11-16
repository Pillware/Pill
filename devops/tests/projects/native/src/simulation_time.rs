//! Updates the shared frame delta.
//!
//! # Responsibilities
//!
//! - Stamp the time elapsed since the previous frame into [`SimulationTime`]
//!   every frame, taken from the engine clock.
//! - Report a missing resource as the module/host initialization mismatch it
//!   is, rather than defaulting the delta.

use crate::SimulationTime;
use pill_core::platform::Instant;
use pill_engine::*;

/// Stamps the time elapsed since the previous frame into [`SimulationTime`].
///
/// # Errors
///
/// Returns [`SystemError::MissingResource`] when the resource is absent, which
/// means the project module and host disagree about initialization.
#[system(name = "simulation_time", order = 510)]
pub(crate) fn update_time_system(mut time: ResMut<SimulationTime>) -> Result<(), SystemError> {
    let now = Instant::now();
    let Some(mut time) = time.get_mut() else {
        return Err(SystemError::MissingResource {
            name: String::from("SimulationTime"),
        });
    };
    // Clamped because a breakpoint or a slow reload can stretch a single frame
    // far enough to throw a ball straight through a wall.
    time.delta_seconds = now.duration_since(time.last_frame).as_secs_f32().min(0.1);
    time.last_frame = now;
    Ok(())
}
