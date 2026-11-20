//! Reports spline visibility and deterministic samples for reload diagnostics.
//!
//! # Responsibilities
//!
//! - Log, at a fixed wall-clock cadence, how many splines the project sees and
//!   a deterministic sample of one, which the reload suites compare across
//!   reloads (`[project] ... spline(s), midpoint (...)`).

// Standard library
use std::time::Duration;

// External crates
use pill_core::math::Vector3f;
use pill_core::platform::Instant;
use pill_engine::*;
use pill_spline::Spline;

// Current crate
use crate::physics::ball_spawn_state;
use crate::settings::BALL_COUNT;

/// The registered name of the spline component, as the world's registry spells
/// it.
///
/// `Spline` is `#[pill(shared)]`, so this is the name under whichever copy of
/// `pill_spline` registered it - the linked crate or the module DLL.
const SPLINE_COMPONENT_NAME: &str = "pill_spline::Spline";

/// The live entity carrying the world's spline, if there is one.
///
/// Looked up by component name rather than held as a handle. A handle names a
/// slot the world recycles and a stored one would have to be trusted across a
/// reload; the probe exists to show what a *reload* does to identity, so it
/// re-finds the entity every generation and compares ids instead (W-0048
/// Stage 5).
pub(crate) fn spline_entity(world: &World) -> Option<Entity> {
    world
        .entity_rows()
        .into_iter()
        .find(|row| {
            row.components
                .iter()
                .any(|name| name == SPLINE_COMPONENT_NAME)
        })
        .map(|row| row.entity)
}

/// Timestamps the last spline probe report so the cadence is wall-clock.
pub(crate) struct SplineProbeState {
    pub(crate) last_report: Instant,
    /// The spline entity's stable id, as hex, re-read on every generation.
    pub(crate) probe_entity_id: Option<String>,
}

impl SplineProbeState {
    /// A state that has not found its entity yet.
    pub(crate) fn new() -> Self {
        Self {
            last_report: Instant::now(),
            probe_entity_id: None,
        }
    }
}

impl Resource for SplineProbeState {}

/// How often the probe reports, on the wall clock.
///
/// The demo runs uncapped, so a per-frame report would drown the log and a
/// frame-count interval would make the cadence - and every suite waiting on a
/// report - depend on the machine's frame rate. Wall-clock time is what a
/// human reading the log observes, and what the waits can bound.
const SPLINE_REPORT_INTERVAL: Duration = Duration::from_millis(250);

/// Reports how many splines the project can see, and samples the curve.
///
/// The count is what reveals whether a separately loaded copy of `pill_spline`
/// shares the component type: `Spline` is `#[pill(shared)]`, so the module
/// DLL's copy and the project's are one column, and this line keeps saying
/// `1 spline(s)` instead of each artifact seeding a curve of its own.
///
/// # Errors
///
/// Returns [`SystemError::MissingResource`] when the resource is absent, which
/// means the project module and host disagree about initialization.
#[system(name = "spline_probe", order = 550)]
pub(crate) fn spline_probe_system(
    mut state: ResMut<SplineProbeState>,
    mut splines: Query<&mut Spline>,
) -> Result<(), SystemError> {
    let Some(mut state) = state.get_mut() else {
        return Err(SystemError::MissingResource {
            name: String::from("SplineProbeState"),
        });
    };
    if state.last_report.elapsed() < SPLINE_REPORT_INTERVAL {
        return Ok(());
    }
    state.last_report = Instant::now();

    let mut visible_spline_count = 0;
    for spline in splines.iter_mut() {
        visible_spline_count += 1;
        let _ = spline;
    }

    // The sampled point comes from a reference spline over the spawn geometry,
    // not from the live curve: the live control points are the ball centres,
    // which move every frame, and an integration suite cannot wait for a moving
    // number. The spawn geometry is fixed, so this value depends only on the
    // linked module's math - which is what the cascade suites watch change.
    let mut spawn_points = [Vector3f::ZERO; BALL_COUNT];
    for (index, point) in spawn_points.iter_mut().enumerate() {
        let ball = ball_spawn_state(index);
        *point = Vector3f::new(ball.position_x, ball.position_y, 0.0);
    }
    let reference = Spline::from_points(&spawn_points);
    let midpoint = reference.get_location_at(0.5);
    // The stable id is part of the report because it is what a reload must not
    // change: the entity handle names a slot the world recycles, while the id
    // names the entity itself (W-0048 Stage 5). `init` re-reads it from the
    // world on every generation, so a generation that lost the entity prints
    // `entityid none` rather than a stale value.
    let entity_id = state.probe_entity_id.as_deref().unwrap_or("none");
    // Logged through the engine's `tracing`, which the host shares with the
    // project through `pill_core.dll`. `[project] ...` is the text the
    // end-to-end suites wait for.
    pill_engine::tracing::info!(
        target: "project",
        "[project] 12xxsees {visible_spline_count} spline(s), midpoint ({:.1}, {:.1}), entityid {entity_id}",
        midpoint.x, midpoint.y
    );

    Ok(())
}
