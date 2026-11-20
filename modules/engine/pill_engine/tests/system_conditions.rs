//! Integration tests for the `run_if` conditions a `#[system]` declares.
//!
//! # Responsibilities
//!
//! - Prove a declared condition decides whether its system runs: every one has
//!   to hold, and a false answer skips the system entirely.
//! - Prove the skip leaves no trace a later frame could read as a run.
//!
//! # Design
//!
//! The conditions read a `Gate` resource the test flips between frames, so an
//! assertion says what a frame did without depending on timing or on the order
//! an artifact happens to register in. The declarations are collected by the
//! `pill_engine` copy linked into this test binary, the same shape a project or
//! a module has, so `run_artifact_registration` exercises the real path.

// Current crate
use pill_engine::component_registry;
use pill_engine::system;
use pill_engine::Engine;
use pill_engine::Res;
use pill_engine::ResMut;
use pill_engine::Resource;

/// The two independent switches the conditions read.
#[derive(Default)]
struct Gate {
    /// Whether the first condition holds.
    open: bool,
    /// Whether the second condition holds.
    even: bool,
}

impl Resource for Gate {}

/// How many times the gated system ran.
#[derive(Default)]
struct RunLog {
    /// One count per run.
    runs: u32,
}

impl Resource for RunLog {}

/// The first condition: it only reads the gate.
fn gate_is_open(gate: Res<Gate>) -> bool {
    gate.get().is_some_and(|gate| gate.open)
}

/// The second condition: it reads the same resource and must hold too.
fn gate_is_even(gate: Res<Gate>) -> bool {
    gate.get().is_some_and(|gate| gate.even)
}

/// Declares a system that runs only while both conditions hold.
#[system(name = "gated", run_if = gate_is_open, run_if = gate_is_even)]
fn gated_system(mut run_log: ResMut<RunLog>) {
    if let Some(mut run_log) = run_log.get_mut() {
        run_log.runs += 1;
    }
}

/// Builds an engine with the artifact's declarations, a gate and a log.
fn engine_with_gate(open: bool, even: bool) -> Engine {
    let mut engine = Engine::new();
    let status = component_registry::run_artifact_registration(
        &mut engine,
        component_registry::no_registration,
    );
    assert_eq!(status, 0, "registration reported a failure");
    engine.world_mut().insert_resource(Gate { open, even });
    engine.world_mut().insert_resource(RunLog::default());
    engine
}

/// Sets the gate between frames.
fn set_gate(engine: &mut Engine, open: bool, even: bool) {
    let gate = engine
        .world_mut()
        .get_resource_mut::<Gate>()
        .expect("the gate is a resource of the world");
    gate.open = open;
    gate.even = even;
}

/// The number of runs the log recorded.
fn run_count(engine: &Engine) -> u32 {
    engine
        .world()
        .get_resource::<RunLog>()
        .expect("the log is a resource of the world")
        .runs
}

/// Both conditions hold, so the system runs - once per frame.
#[test]
fn a_system_runs_while_every_condition_holds() {
    let mut engine = engine_with_gate(true, true);
    engine.process_frame().expect("the frame reports no errors");
    engine.process_frame().expect("the frame reports no errors");
    assert_eq!(run_count(&engine), 2);
}

/// One false condition is enough to skip the system, whichever one it is.
#[test]
fn one_false_condition_skips_the_system() {
    let mut engine = engine_with_gate(true, false);
    engine.process_frame().expect("the frame reports no errors");
    assert_eq!(run_count(&engine), 0, "the second condition was false");

    set_gate(&mut engine, false, true);
    engine.process_frame().expect("the frame reports no errors");
    assert_eq!(run_count(&engine), 0, "the first condition was false");

    set_gate(&mut engine, true, true);
    engine.process_frame().expect("the frame reports no errors");
    assert_eq!(run_count(&engine), 1, "both conditions held again");
}

/// A skipped system is not run later in the same frame: the conditions are
/// checked once per dispatch, and a skip leaves the change-detection baseline
/// alone.
#[test]
fn a_skipped_system_does_not_run_in_its_phase() {
    let mut engine = engine_with_gate(false, true);
    engine.process_frame().expect("the frame reports no errors");
    assert_eq!(run_count(&engine), 0);

    // The system is still registered: a condition never removes it.
    let names: Vec<String> = engine
        .system_snapshots()
        .into_iter()
        .map(|snapshot| snapshot.name)
        .collect();
    assert!(
        names.iter().any(|name| name == "gated"),
        "the gated system stays registered while it is skipped: {names:?}"
    );
}
