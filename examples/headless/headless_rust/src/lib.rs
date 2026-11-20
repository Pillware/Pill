//! A pure simulation: no renderer, no assets, no window.
//!
//! # Responsibilities
//!
//! - Fill the world with [`TICKER_COUNT`] tickers, each counting the frames it
//!   has lived through.
//! - Report the world's total once a second through the engine's log, the only
//!   output a headless run has.
//!
//! # Design
//!
//! `renderer: none` in `project_settings.yaml` keeps the renderer's GPU module
//! and its data crate out of the load order. No device is opened and no
//! surface exists, so there is nothing to draw. What is left is the engine's
//! core - world, scheduler, hot reload - which is what a server-side or test
//! build runs.
//!
//! `init` fills the world up to a target count instead of spawning fresh
//! entities, because it runs again on every hot reload, against the live
//! world.

// External crates
use pill_engine::*;
use serde::{Deserialize, Serialize};

/// How many ticker entities the world is filled up to.
pub const TICKER_COUNT: usize = 256;

/// Seconds between two reports.
///
/// The timer counts real time, not frames. A headless host has no vsync, so it
/// runs the frame loop as fast as the machine allows. A frame-count interval
/// would print thousands of lines a second.
pub const REPORT_INTERVAL_SECONDS: f32 = 1.0;

/// One entity's frame counter.
///
/// The host migrates this component across a hot reload. When the component's
/// schema (its fields and their layout) changed, the old rows are serialized
/// and read back into the new one. The type must stay `Serialize +
/// DeserializeOwned + Default`, and `#[repr(C)]` keeps its memory layout
/// fixed.
#[component(persistable)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TickCounter {
    /// How many times the tick system has advanced this entity.
    pub ticks: u64,
}

/// The report timer, stored as a resource in the world it reports on.
#[derive(Debug, Default)]
pub struct ReportSchedule {
    /// Seconds since the last report was logged.
    pub seconds_since_report: f32,
}

impl Resource for ReportSchedule {}

/// Seeds the world and the report schedule.
///
/// Returns `0` when the world is ready, or `1` if an entity could not be
/// created. The host treats any non-zero status as a failed initialization.
#[pill_project]
pub fn init(engine: &mut Engine) -> u32 {
    // Step 1: Store the report timer in the world.
    engine
        .world_mut()
        .insert_resource(ReportSchedule::default());

    // Step 2: Fill the world up to `TICKER_COUNT` tickers, leaving any that
    // are already there.
    let world = engine.world_mut();
    let existing_ticker_count = Query::<&TickCounter>::new(world).iter_mut().count();
    for _ in existing_ticker_count..TICKER_COUNT {
        if let Err(error) = world.create_entity().with(TickCounter::default()).build() {
            tracing::error!("ticker spawn failed: {error}");
            return 1;
        }
    }
    0
}

/// Advances every ticker and logs the world's total once the report timer runs
/// out.
///
/// Missing `Time` or a missing report timer only means no report is logged;
/// the tickers still advance.
#[system(name = "tick")]
fn tick_system(
    time: Res<Time>,
    mut tickers: Query<&mut TickCounter>,
    mut report_schedule: ResMut<ReportSchedule>,
) -> Result<(), SystemError> {
    // Step 1: Advance every ticker and add up their counts.
    let mut total_tick_count = 0_u64;
    for mut ticker in tickers.iter_mut() {
        ticker.ticks += 1;
        total_tick_count += ticker.ticks;
    }

    // Step 2: Add this frame's elapsed time to the report timer, and log the
    // total once the interval has passed.
    let Some(time) = time.get() else {
        return Ok(());
    };
    let Some(mut report_schedule) = report_schedule.get_mut() else {
        return Ok(());
    };
    report_schedule.seconds_since_report += time.delta_seconds();
    if report_schedule.seconds_since_report >= REPORT_INTERVAL_SECONDS {
        report_schedule.seconds_since_report = 0.0;
        tracing::info!("[headless] tickers={TICKER_COUNT} total_ticks={total_tick_count}");
    }
    Ok(())
}
