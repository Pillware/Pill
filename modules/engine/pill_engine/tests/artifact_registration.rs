//! The two halves of a loadable artifact's entry point, kept apart.
//!
//! # Responsibilities
//!
//! - Proves that a later generation's registration call runs the artifact's
//!   `register` and never its one-time `init`.
//! - Proves that a first load still runs both, in the contract's order.
//! - Proves that a registration that fails leaves `init` unrun, so a broken
//!   image cannot seed a world.

use std::sync::Mutex;

use pill_engine::component_registry::{run_artifact_init, run_artifact_registration};
use pill_engine::Engine;

/// What the entry points called, in order, for the test that is running.
///
/// A `static` because the entry points take plain `fn` pointers. One test runs
/// at a time: [`lock_tests`] serializes them, because the log is shared.
static CALLS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Held for a whole test, so two tests never write [`CALLS`] at once.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Takes the one-test-at-a-time slot, after emptying the call log.
fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().expect("the test lock is never poisoned");
    CALLS
        .lock()
        .expect("the call log is never poisoned")
        .clear();
    guard
}

/// An artifact's registration half: what a reload repeats.
fn register(_engine: &mut Engine) -> u32 {
    CALLS
        .lock()
        .expect("the call log is never poisoned")
        .push("register");
    0
}

/// An artifact's one-time half: seeding.
fn init(_engine: &mut Engine) -> u32 {
    CALLS
        .lock()
        .expect("the call log is never poisoned")
        .push("init");
    0
}

/// A registration that reports failure.
fn register_failing(_engine: &mut Engine) -> u32 {
    CALLS
        .lock()
        .expect("the call log is never poisoned")
        .push("register");
    7
}

/// Empties the call log and takes it, so one test's calls cannot leak into the
/// next.
fn take_calls() -> Vec<&'static str> {
    let mut calls = CALLS.lock().expect("the call log is never poisoned");
    std::mem::take(&mut *calls)
}

#[test]
fn a_later_generation_registers_and_does_not_init() {
    let _serial = lock_tests();
    let mut engine = Engine::new();
    let status = run_artifact_registration(&mut engine, register);
    assert_eq!(status, 0, "registration reports its own status");
    assert_eq!(
        take_calls(),
        vec!["register"],
        "a reload repeats registration and never the one-time init"
    );
}

#[test]
fn a_first_load_registers_and_then_inits() {
    let _serial = lock_tests();
    let mut engine = Engine::new();
    let status = run_artifact_init(&mut engine, register, init);
    assert_eq!(status, 0);
    assert_eq!(
        take_calls(),
        vec!["register", "init"],
        "the registration runs before the one-time init"
    );
}

#[test]
fn a_failed_registration_skips_init() {
    let _serial = lock_tests();
    let mut engine = Engine::new();
    let status = run_artifact_init(&mut engine, register_failing, init);
    assert_eq!(status, 7, "the artifact's own status is what the host sees");
    assert_eq!(
        take_calls(),
        vec!["register"],
        "a world is not seeded on top of a failed registration"
    );
}
