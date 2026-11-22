"""
Full Hot-Reloading Integration Suite for Pill.

PROTECTS: Project and module hot reload keep data, migrate schemas, cascade and stay stable.
TIER: host
SETS: hot-reload, nightly-allowed-to-fail

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo) on PATH
  - Run from the repository root or anywhere (paths are resolved from __file__)

DESCRIPTION
    End-to-end hot-reload suite that launches the standalone host and drives
    real source edits against the live project and an extension, then
    asserts the host's behaviour from its console output:

      Session A (project = devops/tests/projects/native, module = pill_spline)
        1. project_hot_reload      - editing the project triggers a reload and
                                     the counter system keeps ticking (data
                                     survives).
        2. schema_migration        - adding a field to a persistable component
                                     runs selective migration, data survives.
        3. project_forgotten_type  - a component the project stops registering
                                     emits the orphaned-data warning and names
                                     the type.
        4. module_hot_reload       - editing an extension reloads it and
                                     its persistable data survives (existing=1).
        5. module_double_reload    - two consecutive same-config reloads stay
                                     stable: the module re-seeds nothing
                                     (existing stays 1), pinning per-artifact
                                     TypeId stability (no per-reload growth).
        6. module_forgotten_type   - a module that stops registering a type
                                     emits the orphaned-data warning and names
                                     the type. Its data survives, because the
                                     project links the same crate and still
                                     claims that shared type - the peer rule
                                     `unclaimed_type_names` documents.
        7. module_forgotten_type_drop
                                   - the same warning for a module nothing
                                     else links, where the orphaned column is
                                     then dropped: the restored generation
                                     finds no data and re-seeds from scratch
                                     (existing=0), pinning drop-at-detection
                                     end-to-end.
        8. init_failure_rollback   - an init that returns non-zero keeps the
                                     previous generation and the host alive.

      Session B (project = devops/tests/projects/native, module = pill_spline)
        9. cascade_reload          - editing a module the project links triggers
                                     the module reload AND a project reload, and
                                     the new value reaches the running project
                                     (probe midpoint changes). The project probe
                                     still sees exactly ONE spline (xxsees 1),
                                     proving the project's embedded copy and the
                                     module's own copy coexist as distinct types.

    Every file the suite touches (project_settings.yaml files, module/project
    sources) is
    backed up at startup and restored afterwards, so a normal developer
    workspace is left exactly as it was.

USAGE
  python tests/test_hot_reload_suite.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python tests/test_hot_reload_suite.py
  python tests/test_hot_reload_suite.py --timeout-scale 1.5
  python tests/test_hot_reload_suite.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import re
import sys
from pathlib import Path
from typing import Callable, List, Optional, Sequence, Tuple

# Shared paths, tokens, print wrapper, OutputMonitor, process helpers. The
# host's log tokens live in one place.
# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# Shared paths, tokens, print wrapper, OutputMonitor, process helpers.
from core.suite_common import *  # noqa: E402,F401,F403

# The shared scenario runner: one definition for every hot-reload suite.
# This suite adds only its tokens and scenario tables.
from core.scenario import (  # noqa: E402
    Scenario,
    ScenarioPhase,
    ScenarioRunner,
    run_scenario,
    settle_token_for_test_project_path,
)

# =============================================================================
# Session configuration
# =============================================================================

SPLINE_LIB_RS = MODULES_ROOT / "extensions" / "pill_spline" / "modules" / "runtime" / "src" / "pill_spline.rs"
# The component's own file: the derive, the registration helper and the demo
# sample offset live there, while `register`/`init` stay in the crate root.
SPLINE_COMPONENT_RS = MODULES_ROOT / "extensions" / "pill_spline" / "modules" / "runtime" / "src" / "component.rs"

# A module the project does not link. Its persistable component is therefore
# declared by this artifact alone, which makes it the only subject in the
# test project a forgotten type can be dropped for: `pill_spline`'s `Spline` is
# `#[pill(shared)]` and the project registers it from its own copy, so that
# column is the project's to keep (see `peer_component_names`).
PILL_TEST_RS = MODULES_ROOT / "extensions" / "pill_test" / "modules" / "runtime" / "src" / "pill_test.rs"

# Session settings: only the extension list lives in a file now; the
# project itself is selected by PROJECT_PATH per session.
#
# `engine::hot_reload` runs at debug so the scenarios can read the layer's own
# statement of what it dropped (`FORGOTTEN_DROP_TOKEN`). The drop is otherwise
# invisible whenever the scene re-seeds the witness it removed, and a count
# cannot tell those two apart.
SESSION_A_SETTINGS = """\
name: "Suite Session A"
build_binary_name: "SuiteSessionA"
renderer: "none"
modules:
  - "pill_spline"
  - "pill_test"
logging:
  targets:
    engine::hot_reload: debug
"""

SESSION_B_SETTINGS = """\
name: "Suite Session B"
build_binary_name: "SuiteSessionB"
modules:
  - "pill_spline"
  - "pill_test"
"""

# --- Suite-specific output tokens --------------------------------------------

# The development host names the project it runs before its own configuration
# line, so a start log says which project it belongs to before any build
# output. The suite pins both the text and that order.
HOST_WELCOME_TOKEN = "welcome to the Pill engine"
HOST_STARTING_TOKEN = "ECS host starting"

PROJECT_FORGOTTEN_WARN_TOKEN = "no longer registered by the project"
MODULE_FORGOTTEN_WARN_TOKEN = "no longer registered by this module"
# The reload layer's own record of the columns it removed for a forgotten
# type. The forgotten-type scenarios assert on this line rather than on an
# entity count: a scene that re-seeds the dropped witness during the reload
# reports the same count either way, while only a real drop logs this.
FORGOTTEN_DROP_TOKEN = "dropped orphaned columns for component types no longer registered"

# The analytics line a module reload prints, named after the module that
# reloaded. `RELOAD_MODULE_TOKEN` is the spline's; `pill_test` reloads too, and
# its scenario has to wait for its own line rather than one the spline prints.
PILL_TEST_RELOAD_TOKEN = "| module=pill_test"

# The project probe's report line, e.g. "midpoint (390.0, 120.0)". Session B's
# cascade scenario derives the value it expects from the reports the host
# itself prints, so nothing here names a coordinate: the midpoint is the
# module's math applied to the project's spawn geometry, and a literal would
# need re-calibrating whenever either side is edited (this is the check the
# folded `test_module_project_auto_reload.py` made on its own).
PROBE_PREFIX = "midpoint ("
PROBE_MIDPOINT_PATTERN = re.compile(r"midpoint \((-?[0-9.]+), (-?[0-9.]+)\)")
# One entity's stable id, as the project re-reads it from the world on every
# generation. The handle a query yields names a recycled slot;
# this id is what must not change across a reload.
PROBE_ID_PATTERN = re.compile(r"entityid ([0-9a-f]{32})")

# Printed by the test project's one-time `init`, and by nothing else. A
# process must reach that entry point exactly once: a reload repeats the
# artifact's registration half and never its seeding, so a second occurrence
# means the world was seeded twice.
PROJECT_INIT_MARKER = "[project] init ran"


def last_probe_entity_id(output: str) -> str:
    """Returns the stable id the newest probe report names."""
    matches = PROBE_ID_PATTERN.findall(output)
    if not matches:
        raise RuntimeError("No entity stable id found in the host output")
    return matches[-1]


def last_probe_midpoint(output: str) -> Tuple[float, float]:
    """Returns the (x, y) of the newest probe report in the given output."""
    matches = PROBE_MIDPOINT_PATTERN.findall(output)
    if not matches:
        raise RuntimeError("No spline probe midpoint found in the host output")
    x_text, y_text = matches[-1]
    return float(x_text), float(y_text)

# =============================================================================
# Host session
# =============================================================================

# This suite measures the plain reload transaction, so it pins the reload-only
# posture to keep the patch fast path out of the host it drives. `rendering`
# stays ON because session B drives `devops/tests/projects/native`, which links
# `pill_rendering`: a host without it resolves `pill_core` differently
# from that project and the project DLL fails to load with "The specified
# procedure could not be found" (os error 127).
HOST_POSTURE = HOST_DEV_RENDERING


def launch_host(project_path: str):
    """Launches the standalone host exe with PROJECT_PATH pinned to the session project."""
    environment = os.environ.copy()
    environment["PROJECT_PATH"] = project_path
    # The suite measures the reload transaction, so the patch fast path stays
    # out of the host it drives: an edited body rebuilds and reloads instead of
    # installing a patch, which is what the scenarios wait for (`hot reload
    # complete`, "crates rebuilt by cargo"). `test_hot_patch_coverage.py` is
    # where the patch route is asserted.
    environment["PILL_FUNCTION_HOT_PATCHING"] = "0"
    return launch_process([str(HOST_EXE)], MODULES_ROOT, environment)

# The scenario model and runner are shared: `core/scenario.py`.

# =============================================================================
# Scenario definitions
# =============================================================================

# --- pill_spline module edits (all applied from the original source) ---------

SPLINE_REGISTER_ORIGINAL = """\
pub fn register(engine: &mut Engine) -> u32 {
    // Fill up to the target count rather than spawning a new path on every
    // rebuild, because a reload keeps the entities already created - and fills
    // back up when a dropped column took them with it.
    let existing_spline_count = {
        let mut query = Query::<&Spline>::new(engine.world_mut());
        query.iter_mut().count()
    };
    for _ in existing_spline_count..DEMO_SPLINE_COUNT {
        if engine
            .world_mut()
            .create_entity()
            .with(demo_spline())
            .build()
            .is_err()
        {
            // Report the failure so the host keeps the previous generation
            // instead of running with a half-populated world.
            return 1;
        }
    }

    // NOTE: this line is asserted on. `MODULE_REGISTERED_MESSAGE` in
    // `devops/core/suite_common.py` matches the message text against the
    // host's stdout, and scenarios in `devops/tests/test_hot_reload_suite.py`
    // additionally require the `existing=` field - which is what proves a
    // reload preserved the entities the previous generation created rather
    // than spawning a fresh set. Reword either and those suites fail with
    // "Missing required token", which reads like a reload failure and is not.
    pill_core::info!(
        target: telemetry_target::ECS,
        splines = DEMO_SPLINE_COUNT,
        existing = existing_spline_count,
        max_control_points = MAX_CONTROL_POINTS,
        "pill_spline module registered"
    );
    0
}"""

# The struct preamble of `Spline` as committed. Dropping `#[component(...)]` -
# whose expansion is the derive that registers the type - is what lets a stubbed
# module avoid auto-registering the type while the project's copy of the same
# sources keeps declaring it: `register_all_components` reads this artifact's
# own inventory, and without the attribute that inventory holds nothing for
# `Spline`. The hand-written `Component` impl gives the type the identity the
# attribute would have, so the rest of both builds compiles and every other
# type-keyed behavior is unchanged.
SPLINE_DERIVE_ORIGINAL = """\
#[component(persistable, shared, gizmo = crate::gizmo::draw_spline_gizmo)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Spline {"""

SPLINE_DERIVE_STUB_NO_REGISTRATION = """\
impl ::pill_engine::component::Component for Spline {
    fn shared_name() -> Option<&'static str> {
        Some("pill_spline::Spline")
    }

    fn shared_identity() -> Option<u128> {
        const IDENTITY: u128 =
            ::pill_engine::component::shared_component_identity("pill_spline::Spline");
        Some(IDENTITY)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Spline {"""

SPLINE_REGISTER_STUB_NO_REGISTRATION = """\
// Suite stub: the module loads WITHOUT registering `Spline`. `init` still
// calls this function and still runs `register_all_components`, so what
// removes the registration is the derive edit that goes with this stub
// (`SPLINE_DERIVE_STUB_NO_REGISTRATION`): this artifact's inventory then holds
// nothing for `Spline`. `DEMO_SPLINE_COUNT`/`demo_spline` stay referenced only
// to keep the dead-code lint quiet in this stub build.
pub fn register(engine: &mut Engine) -> u32 {
    let _ = engine;
    let _ = (DEMO_SPLINE_COUNT, demo_spline());
    pill_core::info!(
        target: telemetry_target::ECS,
        "pill_spline module registered (suite stub, Spline NOT registered)"
    );
    0
}"""

# The crate's one-type registration path calls the symbol the derive writes, so
# the stubbed generation has to stop calling it too - the function stays (other
# crates name it) and registers nothing.
SPLINE_REGISTER_COMPONENT_ORIGINAL = """\
pub fn register_component(world: &mut World) {
    __pill_register_Spline(world);
}"""

SPLINE_REGISTER_COMPONENT_STUB_NO_REGISTRATION = """\
pub fn register_component(world: &mut World) {
    let _ = world;
}"""

SPLINE_REGISTER_STUB_INIT_FAILURE = """\
// Suite stub: deliberately fail registration so the host rolls back to the
// previous generation. `init` still calls this function, so the stub only
// changes the body. `DEMO_SPLINE_COUNT`/`demo_spline` stay referenced to keep
// the dead-code lint quiet in this stub build.
pub fn register(engine: &mut Engine) -> u32 {
    let _ = engine;
    let _ = (DEMO_SPLINE_COUNT, demo_spline());
    1
}"""

# --- pill_test module edits (whole file, applied from the original source) ----

# The module declares `ModuleTest`, a persistable component no other artifact
# in the test project links, and seeds `MODULE_TEST_ENTITY_COUNT` entities carrying
# it. This stub generation declares nothing at all, which is the one way to
# make a component type vanish from the world's point of view: dropping the
# derive alone would leave the rest of the crate querying the type, and a
# query registers what it names.
PILL_TEST_STUB_NO_REGISTRATION = """\
// Suite stub: a generation that registers nothing, so the module's persistable
// component `ModuleTest` is forgotten. Replacing the whole file is what keeps
// the stub from naming the type anywhere - a query would register it again and
// the generation would no longer be one that stopped declaring it.
use pill_engine::{pill_module, Engine};

#[pill_module]
pub fn register(_engine: &mut Engine) -> u32 {
    pill_core::info!(
        target: pill_core::telemetry::telemetry_target::ECS,
        "pill_test module registered (suite stub, ModuleTest NOT registered)"
    );
    0
}"""

# --- devops/tests/projects/native edits (applied from the original source) ----

FRAMECOUNTER_MIGRATION_EDITS = [
    (
        "struct FrameCounter {\n    count: u64,\n}",
        "struct FrameCounter {\n    count: u64,\n    migrated: bool,\n}",
    ),
    (
        ".with(FrameCounter { count: 0 })",
        ".with(FrameCounter { count: 0, migrated: false })",
    ),
    (
        ".with(FrameCounter { count: 90 })",
        ".with(FrameCounter { count: 90, migrated: false })",
    ),
    (
        ".with(FrameCounter { count: 180 })",
        ".with(FrameCounter { count: 180, migrated: false })",
    ),
]

PROJECT_FORGOTTEN_EDITS = [
    (
        "/// A two-axis velocity, paired with [`SpatialPosition`] on one archetype.\n"
        "#[component(persistable)]\n"
        "#[derive(Debug, Clone, Default)]\n"
        "struct LinearVelocity {\n"
        "    horizontal_speed: f32,\n"
        "    vertical_speed: f32,\n"
        "}\n"
        "\n",
        "",
    ),
    (
        "        .with(LinearVelocity {\n"
        "            horizontal_speed: 1.5,\n"
        "            vertical_speed: 0.25,\n"
        "        })\n",
        "",
    ),
    (
        "        .with(LinearVelocity {\n"
        "            horizontal_speed: 0.5,\n"
        "            vertical_speed: 0.75,\n"
        "        })\n",
        "",
    ),
]

# --- Session A scenarios (project = devops/tests/projects/native, module = pill_spline) ---

SESSION_A_SCENARIOS = [
    Scenario(
        name="project_hot_reload",
        phases=[
            ScenarioPhase(
                edits=[
                    (
                        NATIVE_PROJECT_LIB_RS,
                        [("const THRESHOLD: u64 = 200;", "const THRESHOLD: u64 = 150;")],
                    )
                ],
                wait_token=RELOAD_PROJECT_TOKEN,
                required_tokens=[RELOAD_PROJECT_TOKEN, "crates rebuilt by cargo"],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
                alive_tokens=[(COUNTER_TICK_TOKEN, COUNTER_TICK_TIMEOUT)],
            )
        ],
        restore_after=[NATIVE_PROJECT_LIB_RS],
    ),
    Scenario(
        name="schema_migration",
        phases=[
            ScenarioPhase(
                edits=[(NATIVE_PROJECT_LIB_RS, FRAMECOUNTER_MIGRATION_EDITS)],
                wait_token=RELOAD_PROJECT_TOKEN,
                required_tokens=[
                    MIGRATION_START_TOKEN,
                    "'native::FrameCounter' -> OK",
                ],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
                alive_tokens=[(COUNTER_TICK_TOKEN, COUNTER_TICK_TIMEOUT)],
            )
        ],
        restore_after=[NATIVE_PROJECT_LIB_RS],
    ),
    Scenario(
        name="project_forgotten_type",
        phases=[
            ScenarioPhase(
                edits=[(NATIVE_PROJECT_LIB_RS, PROJECT_FORGOTTEN_EDITS)],
                wait_token=RELOAD_PROJECT_TOKEN,
                required_tokens=[
                    PROJECT_FORGOTTEN_WARN_TOKEN,
                    "LinearVelocity",
                ],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
                alive_tokens=[(COUNTER_TICK_TOKEN, COUNTER_TICK_TIMEOUT)],
                # The warning is the whole assertion on this path. The column
                # itself survives, because this reload's own peers include the
                # project's previous generation, whose names still hold
                # `LinearVelocity` - so nothing it stops registering is ever
                # unclaimed. That is the host's behaviour today, not a property
                # of the scene; `module_forgotten_type_drop` is where a column
                # is actually removed.
            )
        ],
        restore_after=[NATIVE_PROJECT_LIB_RS],
    ),
    Scenario(
        name="module_hot_reload",
        phases=[
            ScenarioPhase(
                edits=[
                    (
                        SPLINE_LIB_RS,
                        [
                            (
                                '"pill_spline module registered"',
                                '"pill_spline module registered v2"',
                            )
                        ],
                    )
                ],
                wait_token=RELOAD_MODULE_TOKEN,
                # `existing=1` pins that the module's persistable data survives
                # a same-config reload (per-artifact TypeId stability).
                required_tokens=[
                    RELOAD_MODULE_TOKEN,
                    MODULE_RELOAD_COMPLETE_TOKEN,
                    "crates rebuilt by cargo",
                    "pill_spline module registered v2",
                    "existing=1",
                ],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
            )
        ],
        restore_after=[SPLINE_LIB_RS],
    ),
    Scenario(
        name="module_double_reload",
        phases=[
            ScenarioPhase(
                edits=[
                    (
                        SPLINE_LIB_RS,
                        [
                            (
                                '"pill_spline module registered"',
                                '"pill_spline module registered v2"',
                            )
                        ],
                    )
                ],
                wait_token=RELOAD_MODULE_TOKEN,
                # Two consecutive same-config reloads must stay stable: the
                # module re-seeds nothing (existing stays 1), which pins that
                # a same-config rebuild keeps the same TypeId - accumulation
                # is per distinct artifact, not per reload (audit 3.2/C3).
                required_tokens=["pill_spline module registered v2", "existing=1"],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
            ),
            ScenarioPhase(
                edits=[
                    (
                        SPLINE_LIB_RS,
                        [
                            (
                                '"pill_spline module registered v2"',
                                '"pill_spline module registered v3"',
                            )
                        ],
                    )
                ],
                wait_token=RELOAD_MODULE_TOKEN,
                required_tokens=["pill_spline module registered v3", "existing=1"],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
            ),
        ],
        restore_after=[SPLINE_LIB_RS],
    ),
    Scenario(
        name="module_forgotten_type",
        phases=[
            ScenarioPhase(
                edits=[
                    (
                        SPLINE_LIB_RS,
                        [
                            (
                                SPLINE_REGISTER_ORIGINAL,
                                SPLINE_REGISTER_STUB_NO_REGISTRATION,
                            ),
                        ],
                    ),
                    (
                        SPLINE_COMPONENT_RS,
                        [
                            (
                                SPLINE_DERIVE_ORIGINAL,
                                SPLINE_DERIVE_STUB_NO_REGISTRATION,
                            ),
                            (
                                SPLINE_REGISTER_COMPONENT_ORIGINAL,
                                SPLINE_REGISTER_COMPONENT_STUB_NO_REGISTRATION,
                            ),
                        ],
                    ),
                ],
                wait_token=RELOAD_MODULE_TOKEN,
                required_tokens=[MODULE_FORGOTTEN_WARN_TOKEN, "pill_spline::Spline"],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
                # No drop is asserted here. The project links this crate, so it
                # registers the same `#[pill(shared)]` type from its own copy,
                # and the reload layer keeps a column any peer still claims -
                # the restored generation therefore finds its original spline
                # instead of re-seeding one. `module_forgotten_type_drop` below
                # is where a column is actually removed.
            )
        ],
        restore_after=[SPLINE_LIB_RS],
    ),
    Scenario(
        name="module_forgotten_type_drop",
        phases=[
            ScenarioPhase(
                edits=[
                    (
                        PILL_TEST_RS,
                        [
                            (
                                read_source(PILL_TEST_RS),
                                PILL_TEST_STUB_NO_REGISTRATION,
                            )
                        ],
                    )
                ],
                wait_token=PILL_TEST_RELOAD_TOKEN,
                required_tokens=["pill_test::ModuleTest"],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
                # `ModuleTest` is declared by this artifact alone, so the
                # generation that stops registering it is also the one that
                # drops the column - the reload layer says so at debug level
                # (`SESSION_A_SETTINGS`). Nothing else can re-seed it, so this
                # is the suite's drop-at-detection witness.
                wait_after=[(FORGOTTEN_DROP_TOKEN, RELOAD_TIMEOUT)],
            )
        ],
        restore_after=[PILL_TEST_RS],
        # After the restore-driven reload the module declares `ModuleTest`
        # again and finds no data: the entities the stub generation orphaned
        # were dropped with their only column, so it seeds its full count from
        # scratch. Pins the drop end-to-end rather than only its log line.
        restore_required_tokens=["entities=4 existing=0"],
    ),
    Scenario(
        name="init_failure_rollback",
        phases=[
            ScenarioPhase(
                edits=[
                    (
                        SPLINE_LIB_RS,
                        [
                            (
                                SPLINE_REGISTER_ORIGINAL,
                                SPLINE_REGISTER_STUB_INIT_FAILURE,
                            )
                        ],
                    )
                ],
                wait_token=ROLLBACK_TOKEN,
                required_tokens=[ROLLBACK_TOKEN],
                forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
            )
        ],
        restore_after=[SPLINE_LIB_RS],
    ),
]

# --- Session B scenario (project = devops/tests/projects/native, module = pill_spline) ---

# The offset bump the cascade scenario makes, planned against the module source
# as it is now. Its `delta` is what the probe check below is derived from.
CASCADE_OFFSET_EDIT = edit_spline_sample_offset(read_source(SPLINE_COMPONENT_RS))


def session_b_scenarios(monitor: OutputMonitor) -> Optional[Sequence[Scenario]]:
    """Session B's scenarios, with the cascade probe value derived from the host.

    The cascade edit moves the module's sample offset, which moves the project
    probe's reported midpoint by exactly that much - so the value to wait for is
    the midpoint the host is reporting now, shifted by `CASCADE_OFFSET_EDIT`.
    Reading the baseline from the running host rather than writing the
    coordinate down is the check the folded `test_module_project_auto_reload.py`
    made on its own, folded here so one host launch proves both the cascade and
    that the new offset reached the project's view.

    The table is built after the host is up for that reason; returns None when
    the probe never reported, which fails the session.
    """
    if not monitor.wait_for(PROBE_PREFIX, PROBE_TIMEOUT):
        print("  [FAIL] The project probe reported no spline midpoint before the cascade.")
        print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
        return None

    baseline_x, baseline_y = last_probe_midpoint(monitor.output_since(0))
    baseline_id = last_probe_entity_id(monitor.output_since(0))
    expected_midpoint = (
        f"midpoint ({baseline_x:.1f}, {baseline_y + CASCADE_OFFSET_EDIT.delta:.1f})"
    )
    print(
        f"  [OK] Project probe baseline ({baseline_x:.1f}, {baseline_y:.1f}), entity id "
        f"{baseline_id}; the cascade edit must report {expected_midpoint!r}."
    )

    return [
        Scenario(
            name="cascade_reload",
            phases=[
                ScenarioPhase(
                    edits=[(SPLINE_LIB_RS, CASCADE_OFFSET_EDIT.replacements)],
                    wait_token=RELOAD_MODULE_TOKEN,
                    # `1 spline(s)` pins module<->project coexistence: `Spline` is
                    # `#[pill(shared)]`, so the module DLL's copy and the project's
                    # are one component with one column, and both artifacts top up
                    # to a single spline rather than each seeding its own. After
                    # both reloads the probe therefore still sees exactly one.
                    #
                    # The count alone carries that meaning; the surrounding probe
                    # wording in `devops/tests/projects/native/src/lib.rs` is incidental and
                    # edited freely by the demos. Widened from the previous
                    # `xxsees 1 spline(s)` so a probe rewording no longer fails this
                    # scenario. If it regresses, read the analytics lines just above
                    # the failure: two reloads and a `1 spline(s)` in the output
                    # mean the behaviour is fine and only the wording moved.
                    required_tokens=[
                        RELOAD_MODULE_TOKEN,
                        MODULE_RELOAD_COMPLETE_TOKEN,
                        CASCADE_TOKEN,
                        RELOAD_PROJECT_TOKEN,
                        "1 spline(s)",
                    ],
                    forbidden_tokens=[PANIC_TOKEN, ACCESS_VIOLATION_TOKEN],
                    # Waited from the index taken after the reload outcome, not
                    # from the scenario start: a report from before the edit
                    # satisfies a start-indexed wait instantly, so only a report
                    # that arrives after the module reload can carry the new
                    # offset. The value is derived above, from the baseline the
                    # host itself printed.
                    #
                    # The id token: the probe re-reads the
                    # spline's stable id from the world every generation, so a
                    # report carrying the baseline id after the reload says the
                    # id survived it.
                    alive_tokens=[
                        (expected_midpoint, PROBE_TIMEOUT),
                        (f"entityid {baseline_id}", PROBE_TIMEOUT),
                    ],
                )
            ],
            restore_after=[SPLINE_LIB_RS],
        ),
    ]

# =============================================================================
# Session runner
# =============================================================================


def session_settle_token(path: Path) -> str:
    """The token a restore of `path` settles with, for this suite's sessions.

    The shared rule knows the project and the module the scenarios edit most;
    session A also reloads `pill_test`, whose own analytics line names it.
    """
    if path == PILL_TEST_RS:
        return PILL_TEST_RELOAD_TOKEN
    return settle_token_for_test_project_path(path)


def session_runner() -> ScenarioRunner:
    """The runner settings for this suite.

    Built per session, and with the timeouts named explicitly rather than left
    to the runner's defaults: those are bound when `core.scenario` is imported,
    so they cannot follow the scaling `main()` applies to this module's
    timeouts from `--timeout-scale`.
    """
    return ScenarioRunner(
        reload_timeout=RELOAD_TIMEOUT,
        settle_timeout=SETTLE_TIMEOUT,
        stability_sleep=STABILITY_SLEEP,
        restore_settle_token=session_settle_token,
    )


def session_a_scenarios(_monitor: OutputMonitor) -> Sequence[Scenario]:
    """Session A's scenarios; nothing in them depends on the running host."""
    return SESSION_A_SCENARIOS


def validate_project_init_ran_once(output: str) -> bool:
    """The test project's `init` must run once per process, not per reload.

    The marker is printed by that one-time entry point itself, not by a system,
    so it counts how often the host reached the entry point: a reload repeats
    the artifact's registration half and must never seed the world again.
    """
    count = output.count(PROJECT_INIT_MARKER)
    if count != 1:
        print(
            f"  [FAIL] The project's init ran {count} time(s) in one process; "
            "a reload must not run it again."
        )
        print(f"  Output tail:\n{output[-1600:]}")
        return False
    print("  [OK] The project's init ran exactly once, across every reload.")
    return True


def verify_clean_restart(
    project_root: Path, project_path: str, settings_content: str
) -> bool:
    """Relaunch the host once and verify it comes back up cleanly.

    By this point a session's scenarios have reloaded both modules and the
    project, so the relaunch starts from built artifacts: cargo decides for
    itself whether anything needs recompiling, and either answer is fine (a
    cold build state rebuilds everything). Only a crash or a failure to reach
    the loop is a hard failure; the build count is printed, not asserted.
    """
    print("\n  [TEST] Clean restart (modules already built)...")
    write_project_settings(project_root, settings_content)
    process, monitor = launch_host(project_path)
    try:
        if not monitor.wait_for(STARTUP_TOKEN, STARTUP_TIMEOUT):
            print("  [FAIL] Clean restart did not reach the project loop.")
            print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
            return False
        startup_output = monitor.output_since(0)
        if has_crash_signals(startup_output):
            print("  [FAIL] Crash signals during clean restart.")
            print(f"  Output tail:\n{startup_output[-1600:]}")
            return False
        build_match = re.search(r"builds:\s*(\d+)", startup_output)
        build_count = build_match.group(1) if build_match else "unknown"
        print(f"  [OK] Clean restart reached the loop (builds={build_count}).")
        return True
    finally:
        terminate_process(process, monitor)


def session_project_name(settings_content: str) -> str:
    """The display name the session's `project_settings.yaml` declares.

    The host reads the name from that file and prints it in its welcome line,
    so the startup assertion derives the expected value from the same text the
    suite writes instead of repeating the literal.
    """
    match = re.search(r'^name:\s*"([^"]+)"', settings_content, re.MULTILINE)
    if match is None:
        raise RuntimeError("the session settings declare no project name")
    return match.group(1)


def run_session(
    name: str,
    project_root: Path,
    project_path: str,
    settings_content: str,
    build_scenarios: Callable[[OutputMonitor], Optional[Sequence[Scenario]]],
) -> bool:
    """Writes the session's project settings, launches the host, and runs its scenarios.

    The scenario table is built after the host reaches its loop rather than
    passed in as a constant: session B derives the probe value its cascade
    scenario waits for from what the host is printing. A builder that returns
    None fails the session.
    """
    print(f"\n{'=' * 64}")
    print(f"  SESSION {name}")
    print(f"{'=' * 64}")

    write_project_settings(project_root, settings_content)

    print("\n  [TEST] Launching standalone host...")
    process, monitor = launch_host(project_path)
    session_passed = True

    try:
        if not monitor.wait_for(STARTUP_TOKEN, STARTUP_TIMEOUT):
            print("  [FAIL] Host did not reach the project loop in time.")
            output = monitor.output_since(0)
            print(f"  Output tail:\n{output[-1600:]}")
            return False
        print("  [OK] Host started and entered the project loop.")

        # Validate startup invariants: the module loaded and the analytics
        # report printed. Whether cargo recompiled anything is NOT an
        # invariant — a cold build state rebuilds everything — so it is not
        # asserted here.
        startup_output = monitor.output_since(0)
        for token in (MODULE_LOADED_TOKEN, ANALYTICS_REPORT_TOKEN):
            if token not in startup_output:
                print(f"  [FAIL] Missing startup token: {token!r}")
                print(f"  Output tail:\n{startup_output[-1600:]}")
                return False
        if has_crash_signals(startup_output):
            print("  [FAIL] Crash signals during startup.")
            print(f"  Output tail:\n{startup_output[-1600:]}")
            return False

        # The welcome line comes before the host's own configuration line and
        # names the project the settings declare, so a start log says which
        # project it belongs to before any build output.
        welcome_line = next(
            (line for line in startup_output.splitlines() if HOST_WELCOME_TOKEN in line),
            None,
        )
        if welcome_line is None:
            print(f"  [FAIL] Missing startup line: {HOST_WELCOME_TOKEN!r}")
            print(f"  Output tail:\n{startup_output[-1600:]}")
            return False
        welcome_index = startup_output.find(HOST_WELCOME_TOKEN)
        host_starting_index = startup_output.find(HOST_STARTING_TOKEN)
        if host_starting_index < 0 or welcome_index > host_starting_index:
            print("  [FAIL] The welcome line must come before 'ECS host starting'.")
            print(f"  Output tail:\n{startup_output[-1600:]}")
            return False
        expected_fragments = (
            f'project="{session_project_name(settings_content)}"',
            "directory=",
            'backend="native"',
        )
        missing_fragments = [
            fragment for fragment in expected_fragments if fragment not in welcome_line
        ]
        if missing_fragments:
            print(f"  [FAIL] The welcome line is missing {missing_fragments}: {welcome_line!r}")
            return False
        print("  [OK] The welcome line names the project before the host starts.")

        print("  [OK] Startup complete: module loaded and analytics report printed.")

        scenarios = build_scenarios(monitor)
        if scenarios is None:
            return False

        for scenario in scenarios:
            if not run_scenario(scenario, monitor, session_runner()):
                session_passed = False
                break

        if session_passed:
            # The scenarios reloaded the project and both modules; the test project's
            # one-time entry point must still have run exactly once.
            if not validate_project_init_ran_once(monitor.output_since(0)):
                session_passed = False

        if session_passed:
            # I3: verify a clean restart on the same (fully built) project.
            # Stop this host first so the relaunch does not contend for the
            # shared DLLs.
            terminate_process(process, monitor)
            if not verify_clean_restart(project_root, project_path, settings_content):
                session_passed = False
            print(f"\n  [PASS] Session {name} completed.")
    finally:
        terminate_process(process, monitor)

    return session_passed

# =============================================================================
# Main
# =============================================================================


def main() -> None:
    """Parses arguments, runs both sessions, and reports the summary."""
    parser = argparse.ArgumentParser(
        description="Full hot-reloading integration suite for Pill"
    )
    add_common_suite_arguments(parser)
    args = parser.parse_args()

    if args.timeout_scale <= 0:
        print("ERROR: --timeout-scale must be > 0")
        sys.exit(1)

    global STARTUP_TIMEOUT, RELOAD_TIMEOUT, PROBE_TIMEOUT, COUNTER_TICK_TIMEOUT
    global SETTLE_TIMEOUT, STABILITY_SLEEP, BUILD_TIMEOUT

    scale = args.timeout_scale
    STARTUP_TIMEOUT = int(STARTUP_TIMEOUT * scale)
    RELOAD_TIMEOUT = int(RELOAD_TIMEOUT * scale)
    PROBE_TIMEOUT = int(PROBE_TIMEOUT * scale)
    COUNTER_TICK_TIMEOUT = int(COUNTER_TICK_TIMEOUT * scale)
    SETTLE_TIMEOUT = int(SETTLE_TIMEOUT * scale)
    STABILITY_SLEEP = max(1, int(STABILITY_SLEEP * scale))
    BUILD_TIMEOUT = int(BUILD_TIMEOUT * scale)

    ensure_host_lock()
    kill_stale_hosts()

    # Capture originals for every file the suite may touch, including both
    # sessions' project settings files.
    for path in (
        project_settings_yaml(NATIVE_PROJECT_ROOT),
        project_settings_yaml(NATIVE_PROJECT_ROOT),
        NATIVE_PROJECT_LIB_RS,
        SPLINE_LIB_RS,
    ):
        BACKUP.capture(path)

    results: List[Tuple[str, bool]] = []
    all_passed = False
    try:
        if not prepare_host(HOST_POSTURE, args, BUILD_TIMEOUT):
            sys.exit(1)

        results.append(
            (
                "A ({} + pill_spline)".format(NATIVE_PROJECT_ROOT.name),
                run_session(
                    "A",
                    NATIVE_PROJECT_ROOT,
                    NATIVE_PROJECT_PATH,
                    SESSION_A_SETTINGS,
                    session_a_scenarios,
                ),
            )
        )
        if results[-1][1]:
            results.append(
                (
                    "B ({} + pill_spline)".format(NATIVE_PROJECT_ROOT.name),
                    run_session(
                        "B",
                        NATIVE_PROJECT_ROOT,
                        NATIVE_PROJECT_PATH,
                        SESSION_B_SETTINGS,
                        session_b_scenarios,
                    ),
                )
            )
        all_passed = all(passed for _, passed in results)
    finally:
        # Always restore the developer's files, even on failure. Rewind the
        # mtimes only when every session finished: a failed or aborted run can
        # leave an artifact the host built from an edit, and rewinding there
        # would make the next start trust that stale artifact.
        BACKUP.restore_all(reset_mtime=all_passed)
        # Restore the project settings files last.
        for path in (
            project_settings_yaml(NATIVE_PROJECT_ROOT),
            project_settings_yaml(NATIVE_PROJECT_ROOT),
        ):
            if BACKUP.captured(path):
                BACKUP.restore_one(path)

    print(f"\n{'=' * 64}")
    print("  SUMMARY")
    print(f"{'=' * 64}")
    for session_name, passed in results:
        status = "[PASS]" if passed else "[FAIL]"
        print(f"  {status} session {session_name}")

    if all_passed:
        print("\n  [PASS] Full hot-reload suite passed.")
        sys.exit(0)
    print("\n  [FAIL] One or more sessions failed.")
    sys.exit(1)


if __name__ == "__main__":
    run_suite_with_timing(main)
