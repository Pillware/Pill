//! Integration tests for the scripts an artifact declares with `#[script]`.
//!
//! # Responsibilities
//!
//! - Prove a declared script reaches the engine from the artifact entry point,
//!   with the hooks and orders its declaration states.
//! - Prove the update pass runs the declared types in `#[order(n)]` order
//!   rather than in registration order.
//! - Prove a block that declares no hook at all is a legal script.
//!
//! # Design
//!
//! The declarations are collected by the `pill_engine` copy linked into this
//! test binary, which is the same shape a project or a module has, so calling
//! `run_artifact_registration` here exercises the real path. The scripts append
//! to a process-wide log instead of a component, because the order they run in
//! is the whole observation: a component would need a second entity and a
//! handle to reach it, which says nothing more about ordering.

// Standard library
use std::sync::Mutex;

// External crates
use pill_engine::component_registry;
use pill_engine::script;
use pill_engine::{Component, Engine, ScriptComponent, ScriptContext, ScriptDeclaration};

/// The order the declared scripts ran in, in the sequence they ran.
static RUN_ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Appends one script's name to [`RUN_ORDER`].
fn record_run(name: &'static str) {
    RUN_ORDER
        .lock()
        .expect("the run log is not poisoned")
        .push(name);
}

/// Declares a script whose update order puts it after [`EarlyDeclaredScript`].
#[derive(Debug)]
struct LateDeclaredScript;

impl Component for LateDeclaredScript {}

#[script]
impl ScriptComponent for LateDeclaredScript {
    #[order(20)]
    fn on_update(&mut self, _script_context: &mut ScriptContext) {
        record_run("late");
    }
}

/// Declares a script whose update order puts it before [`LateDeclaredScript`].
#[derive(Debug)]
struct EarlyDeclaredScript;

impl Component for EarlyDeclaredScript {}

#[script]
impl ScriptComponent for EarlyDeclaredScript {
    #[order(10)]
    fn on_update(&mut self, _script_context: &mut ScriptContext) {
        record_run("early");
    }
}

/// Declares a script that opts in to edit mode, so the flag reaches the
/// declaration and the update pass.
#[derive(Debug, Default)]
struct EditModeDeclaredScript {
    /// How many times the engine ran it.
    runs: u32,
}

impl Component for EditModeDeclaredScript {}

#[script(edit_mode)]
impl ScriptComponent for EditModeDeclaredScript {
    fn on_update(&mut self, _script_context: &mut ScriptContext) {
        self.runs += 1;
    }
}

/// Declares a script with no hook at all, which every default covers.
#[derive(Debug)]
struct SilentDeclaredScript;

impl Component for SilentDeclaredScript {}

#[script]
impl ScriptComponent for SilentDeclaredScript {}

/// The attribute writes the hooks, their orders and the edit-mode flag the
/// block states into the declaration the engine reads.
#[test]
fn a_declaration_records_what_the_block_states() {
    let late = <LateDeclaredScript as ScriptDeclaration>::DECLARATION;
    assert!(late.implements_on_update, "the block defines on_update");
    assert!(
        !late.implements_on_start,
        "a hook the block leaves out is not declared"
    );
    assert_eq!(
        late.on_update_order, 20,
        "the marker reaches the declaration"
    );
    assert!(!late.run_in_edit_mode, "edit mode was not asked for");

    let editor = <EditModeDeclaredScript as ScriptDeclaration>::DECLARATION;
    assert!(editor.run_in_edit_mode, "the flag reaches the declaration");
    assert!(
        editor.implements_on_update,
        "the block defines on_update, so it is declared"
    );
}

/// The artifact entry point registers every declared script, including one
/// whose block defines no hook, and reports success.
#[test]
fn declared_scripts_are_registered_by_the_entry_point() {
    let mut engine = Engine::new();

    let status = component_registry::run_artifact_registration(
        &mut engine,
        component_registry::no_registration,
    );

    assert_eq!(status, 0, "registration reported a failure");
}

/// The update pass runs the declared types in their declared order, not in the
/// order the entry point happened to register them.
#[test]
fn declared_scripts_run_in_their_declared_order() {
    let mut engine = Engine::new();
    component_registry::run_artifact_registration(&mut engine, component_registry::no_registration);
    RUN_ORDER
        .lock()
        .expect("the run log is not poisoned")
        .clear();

    let world = engine.world_mut();
    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, LateDeclaredScript)
        .expect("the late script attaches");
    world
        .add_component(entity, EarlyDeclaredScript)
        .expect("the early script attaches");

    engine.process_frame().expect("the frame reports no errors");

    let order = RUN_ORDER
        .lock()
        .expect("the run log is not poisoned")
        .clone();
    assert_eq!(
        order,
        ["early", "late"],
        "the lower declared order runs first"
    );
}

/// The edit-mode flag reaches the update pass: the script runs while the editor
/// edits, before any play session has started.
#[test]
fn an_edit_mode_declaration_runs_while_the_editor_edits() {
    let mut engine = Engine::new();
    component_registry::run_artifact_registration(&mut engine, component_registry::no_registration);

    let world = engine.world_mut();
    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, EditModeDeclaredScript::default())
        .expect("the edit-mode script attaches");

    engine.set_run_mode(pill_engine::RunMode::Editing);
    engine.process_frame().expect("the frame reports no errors");

    let runs = engine
        .world()
        .get_component::<EditModeDeclaredScript>(entity)
        .expect("the entity carries the script")
        .runs;
    assert_eq!(runs, 1, "an edit-mode script runs while the editor edits");
}
