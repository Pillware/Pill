//! Integration tests for the systems an artifact declares with `#[system]`.
//!
//! # Responsibilities
//!
//! - Prove a declared system reaches the engine from the artifact entry point,
//!   with the display name and order its declaration asks for.
//! - Prove the post-update flag puts a system in the later stage.
//!
//! # Design
//!
//! The declarations are collected by the `pill_engine` copy linked into this
//! test binary, which is the same shape a project or a module has, so calling
//! `run_artifact_registration` here exercises the real path. One frame is run
//! to observe the order the systems execute in. Every declared system writes
//! to one resource, so they conflict and the scheduler must run them one after
//! another in registration order; independent systems would run as one
//! parallel batch and the observation would say nothing about order.
//!
//! A loaded artifact shares its declaration collection with the images already
//! mapped in the process, so the host takes a baseline before mapping it. The
//! last test walks from such a baseline and proves the declarations in front of
//! it stay unregistered, which is what keeps a reloaded generation from
//! registering the retired generation's systems a second time.

// Current crate
use pill_engine::component_registry;
use pill_engine::system;
use pill_engine::Engine;
use pill_engine::ResMut;
use pill_engine::Resource;

/// The order the declared systems ran in, shared through the world so the
/// scheduler cannot run them concurrently.
#[derive(Default)]
struct RunLog {
    /// One entry per system run, in the order the systems ran.
    entries: Vec<&'static str>,
}

impl Resource for RunLog {}

/// Appends a name to the run log.
fn record(run_log: &mut ResMut<RunLog>, name: &'static str) {
    if let Some(mut run_log) = run_log.get_mut() {
        run_log.entries.push(name);
    }
}

/// Declares a system with no arguments, so its name comes from the default.
#[system]
fn unnamed_system(mut run_log: ResMut<RunLog>) {
    record(&mut run_log, "unnamed");
}

/// Declares the system that must run after [`unnamed_system`].
#[system(name = "second", order = 20)]
fn second_system(mut run_log: ResMut<RunLog>) {
    record(&mut run_log, "second");
}

/// Declares the system that must run before [`second_system`].
#[system(name = "first", order = 10)]
fn first_system(mut run_log: ResMut<RunLog>) {
    record(&mut run_log, "first");
}

/// Declares a post-project system whose order value is lower than every
/// project system's, so the phase is the only thing that can put it last.
#[system(name = "after_flush", order = 5, run_phase = "post_project")]
fn after_flush_system(mut run_log: ResMut<RunLog>) {
    record(&mut run_log, "after_flush");
}

/// Declares a system that opts in to edit mode, so it runs while the editor
/// edits and pauses with the rest of the game's work.
#[system(name = "editing_only", order = 30, run_in_edit_mode = true)]
fn editing_only_system(mut run_log: ResMut<RunLog>) {
    record(&mut run_log, "editing_only");
}

/// Declares a pre-project system, so the phase a declaration names decides
/// where it runs, not only its order number.
#[system(name = "pre_flush", order = 40, run_phase = "pre_project")]
fn pre_flush_system(mut run_log: ResMut<RunLog>) {
    record(&mut run_log, "pre_flush");
}

/// The attribute marks the declaration, and the engine runs that system while
/// the editor is editing and the game's systems are not running.
#[test]
fn an_edit_mode_declaration_runs_while_the_editor_edits() {
    let mut engine = Engine::new();
    component_registry::run_artifact_registration(&mut engine, component_registry::no_registration);
    engine.world_mut().insert_resource(RunLog::default());

    engine.set_run_mode(pill_engine::RunMode::Editing);
    engine.process_frame().expect("the frame reports no errors");
    let entries = engine
        .world()
        .get_resource::<RunLog>()
        .expect("the systems ran against the log")
        .entries
        .clone();
    assert_eq!(
        entries,
        ["editing_only"],
        "only the opted-in system runs while the editor edits"
    );
}

/// A declared system is registered by the artifact entry point under the name
/// its attribute asks for, and the entry point's result is a success.
#[test]
fn declared_systems_are_registered_by_the_entry_point() {
    let mut engine = Engine::new();

    let status = component_registry::run_artifact_registration(
        &mut engine,
        component_registry::no_registration,
    );

    assert_eq!(status, 0, "registration reported a failure");
    let names: Vec<String> = engine
        .system_snapshots()
        .into_iter()
        .map(|snapshot| snapshot.name)
        .collect();
    // The list spans the phases: the pre-project declaration first, then the
    // project systems in (order, name) order, then the post-project one
    // whatever its order number is.
    assert_eq!(
        names,
        [
            String::from("pre_flush"),
            String::from(concat!(module_path!(), "::unnamed_system")),
            String::from("first"),
            String::from("second"),
            String::from("editing_only"),
            String::from("after_flush"),
        ],
        "declared systems register in (phase, order) order"
    );
}

/// A frame runs the declared systems in their order values, and the
/// post-update declaration last even though its order value is the smallest.
#[test]
fn declared_systems_run_in_their_declared_order() {
    let mut engine = Engine::new();
    component_registry::run_artifact_registration(&mut engine, component_registry::no_registration);
    engine.world_mut().insert_resource(RunLog::default());

    engine.process_frame().expect("the frame reports no errors");

    let entries = engine
        .world()
        .get_resource::<RunLog>()
        .expect("the systems ran against the log")
        .entries
        .clone();
    assert_eq!(
        entries,
        [
            "pre_flush",
            "unnamed",
            "first",
            "second",
            "editing_only",
            "after_flush"
        ]
    );
}

/// A registration walks from the baseline the host took before mapping the
/// image, so the declarations already in the process stay unregistered. Without
/// a baseline - a static build, a test, the first artifact of a process - the
/// same declarations register.
#[test]
fn a_load_baseline_leaves_the_declarations_taken_before_it_alone() {
    let baseline = pill_engine::declarations::DeclarationBaseline::current();
    let mut engine = Engine::new();
    pill_engine::declarations::with_declaration_baseline(baseline, || {
        component_registry::run_artifact_registration(
            &mut engine,
            component_registry::no_registration,
        );
    });
    assert!(
        engine.system_snapshots().is_empty(),
        "declarations present before the load belong to an earlier image"
    );

    let mut engine = Engine::new();
    component_registry::run_artifact_registration(&mut engine, component_registry::no_registration);
    assert_eq!(
        engine.system_snapshots().len(),
        6,
        "without a baseline the declarations register"
    );
}
