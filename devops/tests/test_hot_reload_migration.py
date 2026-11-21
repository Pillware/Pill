"""
Hot-Reload Migration Integration Suite for Pill.

PROTECTS: Component schema changes on reload migrate the data, or are refused and rolled back.
TIER: host
SETS: hot-reload, nightly

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)
  - Run from workspace root or any path (script resolves paths itself)

DESCRIPTION
    This script launches the standalone host and executes a table-driven migration
    suite by editing the native test project's src/lib.rs. Every scenario waits for
    hot-reload, checks crash signals, verifies expected migration logs, optionally
    validates migration entity counts and the test project's value witnesses (component
    values printed once per generation), and that the counter system still ticks.
    One scenario expects the reload to be REJECTED - a size-changing schema edit
    trips an engine guard - and asserts the rollback and continued ticking instead
    of a completed reload.

USAGE
  python tests/test_hot_reload_migration.py [--cycles N] [--timeout-scale S]

EXAMPLE USAGE
  python tests/test_hot_reload_migration.py
  python tests/test_hot_reload_migration.py --cycles 2
  python tests/test_hot_reload_migration.py --timeout-scale 1.5

--- SCRIPT ---
"""

import argparse
import os
import re
import subprocess
import sys
import time
from functools import partial
from pathlib import Path
from typing import List, Sequence, Tuple

# Shared paths, tokens, print wrapper, OutputMonitor, process helpers. The
# host's log tokens live in one place.
# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# Shared paths, tokens, print wrapper, OutputMonitor, process helpers.
from core.suite_common import *  # noqa: E402,F401,F403

# The shared scenario runner: one definition for every hot-reload suite.
# This suite adds only its tokens, its validators and the scenario table
# below.
from core.scenario import Scenario, ScenarioPhase, ScenarioRunner, run_scenario  # noqa: E402

# =============================================================================
# Configuration
# =============================================================================

# The migration scenarios drive the native test project: its persistable components
# and the witness systems that print them live in `src/lib.rs`.
TEST_PROJECT_ROOT = NATIVE_PROJECT_ROOT
PROJECT_LIB_RS = NATIVE_PROJECT_LIB_RS

# The scenarios rewrite a persistable component's shape. A schema migration
# while extension modules are loaded overflows the host's stack - the module
# artifacts bring their own component registries into the same reload - so this
# suite runs the test project module-free and restores its settings afterwards. The
# reload suites, which never change a schema, keep the usual module list.
MODULE_FREE_SETTINGS = """name: "Native Test Project"
build_binary_name: "NativeTestProject"
modules: []
"""
PROJECT_SETTINGS_PATH = TEST_PROJECT_ROOT / "project_settings.yaml"

RELOAD_COMPLETE_TOKEN = "hot reload complete"
FAST_PATH_TOKEN = "schema unchanged for all persistable component types"
SELECTIVE_START_TOKEN = MIGRATION_START_TOKEN
SELECTIVE_FINISHED_TOKEN = MIGRATION_FINISHED_TOKEN
FRAMECOUNTER_MIGRATE_LOG_TOKEN = "'native::FrameCounter' -> migrating"
SPATIAL_POSITION_MIGRATE_LOG_TOKEN = "'native::SpatialPosition' -> migrating"
LINEAR_VELOCITY_MIGRATE_LOG_TOKEN = "'native::LinearVelocity' -> migrating"
MARKER_TRAIL_MIGRATE_LOG_TOKEN = "'native::MarkerTrail' -> migrating"
DESERIALIZE_FAILED_TOKEN = "[persistence] Failed to deserialize"

# Value witnesses: the test project prints its components once per module generation
# (see the witness systems in project/src/lib.rs), so a scenario can assert that
# migration carried VALUES across, not just entity counts. Without these, a
# migration that silently reset every field to its default would still pass.
WITNESS_POSITION_2 = "[project] witness SpatialPosition(10.00,20.00)"
WITNESS_POSITION_3_A = "[project] witness SpatialPosition(10.00,20.00,0.00)"
WITNESS_POSITION_3_B = "[project] witness SpatialPosition(1.00,2.00,0.00)"
WITNESS_POSITION_3_C = "[project] witness SpatialPosition(-5.00,8.00,0.00)"
WITNESS_TRAIL_ALPHA = (
    '[project] witness MarkerTrail(points=3,sum=6.00,label="alpha",samples=2,last=1.50)'
)
WITNESS_TRAIL_BETA = (
    '[project] witness MarkerTrail(points=2,sum=9.00,label="beta",samples=1,last=2.50)'
)

STARTUP_TIMEOUT = 60
RELOAD_TIMEOUT = 45
BUILD_TIMEOUT = 120
STABILITY_SLEEP = 3
COUNTER_TICK_TIMEOUT = 10
CYCLE_PAUSE = 2

ORIGINAL_CONTENT: str = ""

# The dev host pins the engine's `development` feature in every posture; this
# suite measures migration through the plain reload transaction, so it drives
# the reload-only host posture.
HOST_POSTURE = HOST_DEV

# =============================================================================
# Source edit patterns
# =============================================================================

FRAMECOUNTER_COUNT_ONLY = """struct FrameCounter {
    count: u64,
}"""

FRAMECOUNTER_WITH_BOOL = """struct FrameCounter {
    count: u64,
    migrated: bool,
}"""

SPATIAL_POSITION_BASE = """struct SpatialPosition {
    horizontal: f32,
    vertical: f32,
}"""

SPATIAL_POSITION_WITH_DEPTH = """struct SpatialPosition {
    horizontal: f32,
    vertical: f32,
    depth: f32,
}"""

LINEAR_VELOCITY_BASE = """struct LinearVelocity {
    horizontal_speed: f32,
    vertical_speed: f32,
}"""

LINEAR_VELOCITY_RENAMED_FIELD = """struct LinearVelocity {
    horizontal_speed: f32,
    upward_speed: f32,
}"""

FRAMECOUNTER_ENTITY_ONE_BASE = ".with(FrameCounter { count: 0 })"
FRAMECOUNTER_ENTITY_TWO_BASE = ".with(FrameCounter { count: 90 })"
FRAMECOUNTER_ENTITY_THREE_BASE = ".with(FrameCounter { count: 180 })"

FRAMECOUNTER_ENTITY_ONE_WITH_BOOL = ".with(FrameCounter { count: 0, migrated: false })"
FRAMECOUNTER_ENTITY_TWO_WITH_BOOL = ".with(FrameCounter { count: 90, migrated: false })"
FRAMECOUNTER_ENTITY_THREE_WITH_BOOL = ".with(FrameCounter { count: 180, migrated: false })"

SPATIAL_POSITION_ENTITY_ONE_BASE = """.with(SpatialPosition {
            horizontal: 10.0,
            vertical: 20.0,
        })"""

SPATIAL_POSITION_ENTITY_TWO_BASE = """.with(SpatialPosition {
            horizontal: 1.0,
            vertical: 2.0,
        })"""

SPATIAL_POSITION_ENTITY_THREE_BASE = """.with(SpatialPosition {
            horizontal: -5.0,
            vertical: 8.0,
        })"""

SPATIAL_POSITION_ENTITY_ONE_WITH_DEPTH = """.with(SpatialPosition {
            horizontal: 10.0,
            vertical: 20.0,
            depth: 0.0,
        })"""

SPATIAL_POSITION_ENTITY_TWO_WITH_DEPTH = """.with(SpatialPosition {
            horizontal: 1.0,
            vertical: 2.0,
            depth: 1.0,
        })"""

SPATIAL_POSITION_ENTITY_THREE_WITH_DEPTH = """.with(SpatialPosition {
            horizontal: -5.0,
            vertical: 8.0,
            depth: 2.0,
        })"""

LINEAR_VELOCITY_ENTITY_ONE_BASE = """.with(LinearVelocity {
            horizontal_speed: 1.5,
            vertical_speed: 0.25,
        })"""

LINEAR_VELOCITY_ENTITY_TWO_BASE = """.with(LinearVelocity {
            horizontal_speed: 0.5,
            vertical_speed: 0.75,
        })"""

LINEAR_VELOCITY_ENTITY_ONE_RENAMED = """.with(LinearVelocity {
            horizontal_speed: 1.5,
            upward_speed: 0.25,
        })"""

LINEAR_VELOCITY_ENTITY_TWO_RENAMED = """.with(LinearVelocity {
            horizontal_speed: 0.5,
            upward_speed: 0.75,
        })"""

# SpatialPosition after the depth field was added: reordering the fields must
# not reorder the DATA (JSON migrates by field name).
SPATIAL_POSITION_REORDERED = """struct SpatialPosition {
    depth: f32,
    vertical: f32,
    horizontal: f32,
}"""

# Widening a field changes the component's in-memory size, which the registry
# guard refuses outright: the reload is rejected and the host rolls back. This
# edit therefore runs last, once no later scenario needs a working source tree.
SPATIAL_POSITION_WIDENED = """struct SpatialPosition {
    depth: f32,
    vertical: f32,
    horizontal: f64,
}"""

WITNESS_POSITION_PRINT_BASE = """        pill_engine::tracing::info!(
            target: "project",
            "[project] witness SpatialPosition({:.2},{:.2})",
            position.horizontal, position.vertical
        );"""

WITNESS_POSITION_PRINT_WITH_DEPTH = """        pill_engine::tracing::info!(
            target: "project",
            "[project] witness SpatialPosition({:.2},{:.2},{:.2})",
            position.horizontal, position.vertical, position.depth
        );"""

MARKER_TRAIL_BASE = """struct MarkerTrail {
    points: Vec<f32>,
    label: String,
    samples: DynamicBuffer<f32>,
}"""

MARKER_TRAIL_WITH_STAMPS = """struct MarkerTrail {
    points: Vec<f32>,
    label: String,
    samples: DynamicBuffer<f32>,
    stamps: Vec<u32>,
}"""

MARKER_TRAIL_POINTS_F64 = """struct MarkerTrail {
    points: Vec<f64>,
    label: String,
    samples: DynamicBuffer<f32>,
    stamps: Vec<u32>,
}"""

MARKER_TRAIL_RENAMED_LABEL = """struct MarkerTrail {
    points: Vec<f64>,
    name: String,
    samples: DynamicBuffer<f32>,
    stamps: Vec<u32>,
}"""

# A `Vec` is 24 bytes whatever it holds, so changing the ELEMENT type keeps the
# component's size stable - the registry guard lets it through and the failure
# only shows up when the stored numbers meet `Vec<u32>` in the JSON layer.
MARKER_TRAIL_INCOMPATIBLE = """struct MarkerTrail {
    points: Vec<u32>,
    name: String,
    samples: DynamicBuffer<f32>,
    stamps: Vec<u32>,
}"""

MARKER_TRAIL_SEED_ALPHA = """.with(MarkerTrail {
            points: vec![1.0, 2.0, 3.0],
            label: String::from("alpha"),
            samples: DynamicBuffer::from_slice(&[0.5, 1.5]),
        })"""

MARKER_TRAIL_SEED_ALPHA_WITH_STAMPS = """.with(MarkerTrail {
            points: vec![1.0, 2.0, 3.0],
            label: String::from("alpha"),
            samples: DynamicBuffer::from_slice(&[0.5, 1.5]),
            stamps: vec![10, 11],
        })"""

MARKER_TRAIL_SEED_ALPHA_NAMED = """.with(MarkerTrail {
            points: vec![1.0, 2.0, 3.0],
            name: String::from("alpha"),
            samples: DynamicBuffer::from_slice(&[0.5, 1.5]),
            stamps: vec![10, 11],
        })"""

MARKER_TRAIL_SEED_ALPHA_U32 = """.with(MarkerTrail {
            points: vec![1, 2, 3],
            name: String::from("alpha"),
            samples: DynamicBuffer::from_slice(&[0.5, 1.5]),
            stamps: vec![10, 11],
        })"""

MARKER_TRAIL_SEED_BETA = """.with(MarkerTrail {
            points: vec![4.0, 5.0],
            label: String::from("beta"),
            samples: DynamicBuffer::from_slice(&[2.5]),
        })"""

MARKER_TRAIL_SEED_BETA_WITH_STAMPS = """.with(MarkerTrail {
            points: vec![4.0, 5.0],
            label: String::from("beta"),
            samples: DynamicBuffer::from_slice(&[2.5]),
            stamps: vec![20],
        })"""

MARKER_TRAIL_SEED_BETA_NAMED = """.with(MarkerTrail {
            points: vec![4.0, 5.0],
            name: String::from("beta"),
            samples: DynamicBuffer::from_slice(&[2.5]),
            stamps: vec![20],
        })"""

MARKER_TRAIL_SEED_BETA_U32 = """.with(MarkerTrail {
            points: vec![4, 5],
            name: String::from("beta"),
            samples: DynamicBuffer::from_slice(&[2.5]),
            stamps: vec![20],
        })"""

WITNESS_TRAIL_PRINT_BASE = r"""        let sum: f32 = trail.points.iter().sum();
        let last = trail.samples.last().copied().unwrap_or(0.0);
        pill_engine::tracing::info!(
            target: "project",
            "[project] witness MarkerTrail(points={},sum={:.2},label=\"{}\",samples={},last={:.2})",
            trail.points.len(),
            sum,
            trail.label,
            trail.samples.len(),
            last,
        );"""

WITNESS_TRAIL_PRINT_WITH_STAMPS = r"""        let sum: f32 = trail.points.iter().sum();
        let last = trail.samples.last().copied().unwrap_or(0.0);
        pill_engine::tracing::info!(
            target: "project",
            "[project] witness MarkerTrail(points={},sum={:.2},label=\"{}\",samples={},last={:.2},stamps={})",
            trail.points.len(),
            sum,
            trail.label,
            trail.samples.len(),
            last,
            trail.stamps.len(),
        );"""

WITNESS_TRAIL_PRINT_WITH_STAMPS_F64 = r"""        let sum: f64 = trail.points.iter().sum();
        let last = trail.samples.last().copied().unwrap_or(0.0);
        pill_engine::tracing::info!(
            target: "project",
            "[project] witness MarkerTrail(points={},sum={:.2},label=\"{}\",samples={},last={:.2},stamps={})",
            trail.points.len(),
            sum,
            trail.label,
            trail.samples.len(),
            last,
            trail.stamps.len(),
        );"""

WITNESS_TRAIL_PRINT_NAMED = r"""        let sum: f64 = trail.points.iter().sum();
        let last = trail.samples.last().copied().unwrap_or(0.0);
        pill_engine::tracing::info!(
            target: "project",
            "[project] witness MarkerTrail(points={},sum={:.2},name=\"{}\",samples={},last={:.2},stamps={})",
            trail.points.len(),
            sum,
            trail.name,
            trail.samples.len(),
            last,
            trail.stamps.len(),
        );"""

WITNESS_TRAIL_PRINT_U32 = r"""        let sum: f64 = trail.points.iter().map(|&value| value as f64).sum();
        let last = trail.samples.last().copied().unwrap_or(0.0);
        pill_engine::tracing::info!(
            target: "project",
            "[project] witness MarkerTrail(points={},sum={:.2},name=\"{}\",samples={},last={:.2},stamps={})",
            trail.points.len(),
            sum,
            trail.name,
            trail.samples.len(),
            last,
            trail.stamps.len(),
        );"""


THRESHOLD_200 = "const THRESHOLD: u64 = 200;"
THRESHOLD_150 = "const THRESHOLD: u64 = 150;"


# =============================================================================
# Data models
# =============================================================================


# The scenario model and runner are shared: `core/scenario.py`. Every migration
# scenario has the same shape - one edit to the test project source, one reload (or
# the rollback a refused one produces), a counter that must keep ticking, and
# the migration assertions - so the table below builds them through
# `migration_scenario` instead of repeating that shape thirteen times.


def migration_scenario(
    name: str,
    replacements: Sequence[Tuple[str, str]],
    required_tokens: Sequence[str],
    forbidden_tokens: Sequence[str],
    expected_migration_entity_counts: Sequence[Tuple[str, int]] = (),
    expected_witness_tokens: Sequence[str] = (),
    expect_rejected_reload: bool = False,
) -> Scenario:
    """Builds one migration scenario on the shared runner.

    A refused reload never reaches "hot reload complete": the engine refuses the
    generation with a diagnostic and the host rolls back to the running one, so
    that scenario waits for the rollback marker instead. The counter-tick wait
    runs from an index taken after the reload outcome, which is what makes it
    proof of life for the generation the reload produced rather than a report
    from the one it replaced.
    """
    return Scenario(
        name=name,
        phases=[
            ScenarioPhase(
                edits=[(PROJECT_LIB_RS, replacements)],
                wait_token=(
                    ROLLBACK_TOKEN if expect_rejected_reload else RELOAD_COMPLETE_TOKEN
                ),
                required_tokens=required_tokens,
                forbidden_tokens=forbidden_tokens,
                alive_tokens=[(COUNTER_TICK_TOKEN, COUNTER_TICK_TIMEOUT)],
            )
        ],
        validators=[
            partial(
                validate_migration_entity_counts,
                expected_counts=expected_migration_entity_counts,
            ),
            partial(
                validate_witness_tokens,
                expected_tokens=expected_witness_tokens,
            ),
        ],
    )


def migration_runner() -> ScenarioRunner:
    """The runner settings for this suite.

    Built per cycle, and with the timeouts named explicitly rather than left to
    the runner's defaults: those are bound when `core.scenario` is imported, so
    they cannot follow the scaling `apply_timeout_scale` applies here. No
    scenario restores a file between edits - the table is one cumulative chain
    from the original source - so the settle timeout stays at its default.
    """
    return ScenarioRunner(
        reload_timeout=RELOAD_TIMEOUT,
        stability_sleep=STABILITY_SLEEP,
    )


# =============================================================================
# Source helpers
# =============================================================================


def restore_original() -> None:
    """Restores the test project source captured at script startup."""
    if not ORIGINAL_CONTENT:
        print("  [WARN] No original content captured. Skipping restore.")
        return

    if read_source(PROJECT_LIB_RS) == ORIGINAL_CONTENT:
        return

    atomic_write(PROJECT_LIB_RS, ORIGINAL_CONTENT)


# =============================================================================
# Scenario execution helpers
# =============================================================================


def validate_migration_entity_counts(
    label: str,
    output: str,
    expected_counts: Sequence[Tuple[str, int]],
) -> bool:
    """Validates per-component migrated entity counts from persistence logs."""
    if not expected_counts:
        return True

    for component_name, expected_count in expected_counts:
        escaped_component_name = re.escape(component_name)
        pattern = rf"'{escaped_component_name}' -> OK \((\d+) entities\)"
        match = re.search(pattern, output)
        if match is None:
            print(
                f"  [FAIL] Missing migration count log for {label}: '{component_name}'",
            )
            print(f"  Output tail:\n{output[-1600:]}")
            return False

        actual_count = int(match.group(1))
        if actual_count != expected_count:
            print(
                f"  [FAIL] Unexpected migration count for {label}: "
                f"'{component_name}' expected {expected_count}, got {actual_count}",
            )
            print(f"  Output tail:\n{output[-1600:]}")
            return False

    return True


def validate_witness_tokens(
    label: str,
    output: str,
    expected_tokens: Sequence[str],
) -> bool:
    """Validates that migrated VALUES survived: the test project's per-generation
    witness lines must contain every expected value string."""
    for token in expected_tokens:
        if token not in output:
            print(f"  [FAIL] Missing witness value for {label}: {token!r}")
            print(f"  Output tail:\n{output[-1600:]}")
            return False
    return True


def launch_standalone() -> Tuple[subprocess.Popen, OutputMonitor]:
    """Starts the standalone host and returns process + monitor."""
    process_environment = os.environ.copy()
    process_environment["PROJECT_PATH"] = NATIVE_PROJECT_PATH
    return launch_process(host_command(HOST_POSTURE), WORKSPACE_ROOT / "modules", process_environment)


# =============================================================================
# Scenario suite definition
# =============================================================================


def build_scenarios() -> List[Scenario]:
    """Builds ordered migration scenarios from the test project's current source.

    One edit per scenario, each applied to the source as the previous scenario
    left it: the table is one cumulative chain from the original, which is why
    no scenario restores the file. Every entry goes through
    `migration_scenario`, so the thirteen share the shape - one edit, one
    reload (or the rollback a refused one produces), a counter that must keep
    ticking - and differ only in their tokens and expectations.
    """
    return [
        migration_scenario(
            name="Safe change: threshold 200 -> 150 (fast path)",
            replacements=[(THRESHOLD_200, THRESHOLD_150)],
            required_tokens=[FAST_PATH_TOKEN],
            forbidden_tokens=[SELECTIVE_START_TOKEN],
            expected_witness_tokens=[WITNESS_POSITION_2, WITNESS_TRAIL_ALPHA],
        ),
        migration_scenario(
            name="Modify FrameCounter: add migrated bool",
            replacements=[
                (FRAMECOUNTER_COUNT_ONLY, FRAMECOUNTER_WITH_BOOL),
                (FRAMECOUNTER_ENTITY_ONE_BASE, FRAMECOUNTER_ENTITY_ONE_WITH_BOOL),
                (FRAMECOUNTER_ENTITY_TWO_BASE, FRAMECOUNTER_ENTITY_TWO_WITH_BOOL),
                (FRAMECOUNTER_ENTITY_THREE_BASE, FRAMECOUNTER_ENTITY_THREE_WITH_BOOL),
            ],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
            ],
            forbidden_tokens=[
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
                LINEAR_VELOCITY_MIGRATE_LOG_TOKEN,
            ],
            expected_migration_entity_counts=[("native::FrameCounter", 3)],
            expected_witness_tokens=[WITNESS_POSITION_2, WITNESS_TRAIL_ALPHA],
        ),
        migration_scenario(
            name="Revert FrameCounter: remove migrated bool",
            replacements=[
                (FRAMECOUNTER_WITH_BOOL, FRAMECOUNTER_COUNT_ONLY),
                (FRAMECOUNTER_ENTITY_ONE_WITH_BOOL, FRAMECOUNTER_ENTITY_ONE_BASE),
                (FRAMECOUNTER_ENTITY_TWO_WITH_BOOL, FRAMECOUNTER_ENTITY_TWO_BASE),
                (FRAMECOUNTER_ENTITY_THREE_WITH_BOOL, FRAMECOUNTER_ENTITY_THREE_BASE),
            ],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
            ],
            forbidden_tokens=[
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
                LINEAR_VELOCITY_MIGRATE_LOG_TOKEN,
            ],
            expected_migration_entity_counts=[("native::FrameCounter", 3)],
            expected_witness_tokens=[WITNESS_POSITION_2, WITNESS_TRAIL_ALPHA],
        ),
        migration_scenario(
            name="Modify SpatialPosition: add depth coordinate",
            replacements=[
                (SPATIAL_POSITION_BASE, SPATIAL_POSITION_WITH_DEPTH),
                (SPATIAL_POSITION_ENTITY_ONE_BASE, SPATIAL_POSITION_ENTITY_ONE_WITH_DEPTH),
                (SPATIAL_POSITION_ENTITY_TWO_BASE, SPATIAL_POSITION_ENTITY_TWO_WITH_DEPTH),
                (SPATIAL_POSITION_ENTITY_THREE_BASE, SPATIAL_POSITION_ENTITY_THREE_WITH_DEPTH),
                (WITNESS_POSITION_PRINT_BASE, WITNESS_POSITION_PRINT_WITH_DEPTH),
            ],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
            ],
            forbidden_tokens=[
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
                LINEAR_VELOCITY_MIGRATE_LOG_TOKEN,
            ],
            expected_migration_entity_counts=[("native::SpatialPosition", 3)],
            # The added field defaults to zero on migrated entities while every
            # pre-existing value survives: the witness pins exactly that.
            expected_witness_tokens=[
                WITNESS_POSITION_3_A,
                WITNESS_POSITION_3_B,
                WITNESS_POSITION_3_C,
            ],
        ),
        migration_scenario(
            name="Modify LinearVelocity: rename vertical_speed field",
            replacements=[
                (LINEAR_VELOCITY_BASE, LINEAR_VELOCITY_RENAMED_FIELD),
                (LINEAR_VELOCITY_ENTITY_ONE_BASE, LINEAR_VELOCITY_ENTITY_ONE_RENAMED),
                (LINEAR_VELOCITY_ENTITY_TWO_BASE, LINEAR_VELOCITY_ENTITY_TWO_RENAMED),
            ],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                LINEAR_VELOCITY_MIGRATE_LOG_TOKEN,
            ],
            forbidden_tokens=[
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
            ],
            expected_migration_entity_counts=[("native::LinearVelocity", 2)],
            expected_witness_tokens=[
                WITNESS_POSITION_3_A,
                WITNESS_POSITION_3_B,
                WITNESS_TRAIL_BETA,
            ],
        ),
        migration_scenario(
            name="Remove LinearVelocity from registered/seeded components",
            replacements=[
                (
                    "#[component(persistable)]\n"
                    "#[derive(Debug, Clone, Default)]\n"
                    "struct LinearVelocity {",
                    "#[component]\n"
                    "#[derive(Debug, Clone, Default)]\n"
                    "struct LinearVelocity {",
                ),
            ],
            required_tokens=[FAST_PATH_TOKEN],
            forbidden_tokens=[
                SELECTIVE_START_TOKEN,
                LINEAR_VELOCITY_MIGRATE_LOG_TOKEN,
            ],
            expected_witness_tokens=[WITNESS_POSITION_3_C, WITNESS_TRAIL_ALPHA],
        ),
        migration_scenario(
            name="Reorder SpatialPosition fields: data follows names, not positions",
            replacements=[(SPATIAL_POSITION_WITH_DEPTH, SPATIAL_POSITION_REORDERED)],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
            ],
            forbidden_tokens=[
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
                LINEAR_VELOCITY_MIGRATE_LOG_TOKEN,
            ],
            expected_migration_entity_counts=[("native::SpatialPosition", 3)],
            expected_witness_tokens=[
                WITNESS_POSITION_3_A,
                WITNESS_POSITION_3_B,
                WITNESS_POSITION_3_C,
            ],
        ),
        migration_scenario(
            name="MarkerTrail: add a Vec<u32> field (old entities get an empty one)",
            replacements=[
                (MARKER_TRAIL_BASE, MARKER_TRAIL_WITH_STAMPS),
                (MARKER_TRAIL_SEED_ALPHA, MARKER_TRAIL_SEED_ALPHA_WITH_STAMPS),
                (MARKER_TRAIL_SEED_BETA, MARKER_TRAIL_SEED_BETA_WITH_STAMPS),
                (WITNESS_TRAIL_PRINT_BASE, WITNESS_TRAIL_PRINT_WITH_STAMPS),
            ],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                MARKER_TRAIL_MIGRATE_LOG_TOKEN,
            ],
            forbidden_tokens=[
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
            ],
            expected_migration_entity_counts=[("native::MarkerTrail", 2)],
            # Heap payloads must survive the schema change; the new vector
            # arrives empty on every row, because the seeded ones were the only
            # rows and they are migrated rather than spawned again.
            expected_witness_tokens=[
                '[project] witness MarkerTrail(points=3,sum=6.00,label="alpha",samples=2,last=1.50,stamps=0)',
                '[project] witness MarkerTrail(points=2,sum=9.00,label="beta",samples=1,last=2.50,stamps=0)',
            ],
        ),
        migration_scenario(
            name="MarkerTrail: widen the Vec element type f32 -> f64",
            replacements=[
                (MARKER_TRAIL_WITH_STAMPS, MARKER_TRAIL_POINTS_F64),
                (WITNESS_TRAIL_PRINT_WITH_STAMPS, WITNESS_TRAIL_PRINT_WITH_STAMPS_F64),
            ],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                MARKER_TRAIL_MIGRATE_LOG_TOKEN,
            ],
            # Widening a Vec ELEMENT type keeps the component's size, so the
            # entities this generation seeds land in the same archetype as the
            # ones being converted. Feeding them to the retiring generation's
            # f32 serializer reinterprets their f64 heap and prints sums like
            # 1.88 for [1.0, 2.0, 3.0]; correct migration never produces those.
            forbidden_tokens=[
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
                "MarkerTrail(points=3,sum=1.88",
                "MarkerTrail(points=2,sum=2.25",
            ],
            expected_migration_entity_counts=[("native::MarkerTrail", 2)],
            expected_witness_tokens=[
                '[project] witness MarkerTrail(points=3,sum=6.00,label="alpha",samples=2,last=1.50,stamps=0)',
                '[project] witness MarkerTrail(points=2,sum=9.00,label="beta",samples=1,last=2.50,stamps=0)',
            ],
        ),
        migration_scenario(
            name="MarkerTrail: rename label -> name (renamed text resets, rest survives)",
            replacements=[
                (MARKER_TRAIL_POINTS_F64, MARKER_TRAIL_RENAMED_LABEL),
                (MARKER_TRAIL_SEED_ALPHA_WITH_STAMPS, MARKER_TRAIL_SEED_ALPHA_NAMED),
                (MARKER_TRAIL_SEED_BETA_WITH_STAMPS, MARKER_TRAIL_SEED_BETA_NAMED),
                (WITNESS_TRAIL_PRINT_WITH_STAMPS_F64, WITNESS_TRAIL_PRINT_NAMED),
            ],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                MARKER_TRAIL_MIGRATE_LOG_TOKEN,
            ],
            forbidden_tokens=[
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
            ],
            expected_migration_entity_counts=[("native::MarkerTrail", 2)],
            expected_witness_tokens=[
                # A renamed field has no source value: it resets to Default
                # while its neighbours keep their data. Both rows are migrations
                # of the seeded entities, so both lose their name.
                '[project] witness MarkerTrail(points=3,sum=6.00,name="",samples=2,last=1.50,stamps=0)',
            ],
        ),
        migration_scenario(
            name="MarkerTrail: change the Vec element type f32 -> u32 (snapshot no longer parses)",
            replacements=[
                (MARKER_TRAIL_RENAMED_LABEL, MARKER_TRAIL_INCOMPATIBLE),
                (MARKER_TRAIL_SEED_ALPHA_NAMED, MARKER_TRAIL_SEED_ALPHA_U32),
                (MARKER_TRAIL_SEED_BETA_NAMED, MARKER_TRAIL_SEED_BETA_U32),
                (WITNESS_TRAIL_PRINT_NAMED, WITNESS_TRAIL_PRINT_U32),
            ],
            required_tokens=[
                SELECTIVE_START_TOKEN,
                SELECTIVE_FINISHED_TOKEN,
                MARKER_TRAIL_MIGRATE_LOG_TOKEN,
                DESERIALIZE_FAILED_TOKEN,
            ],
            forbidden_tokens=[
                FRAMECOUNTER_MIGRATE_LOG_TOKEN,
                SPATIAL_POSITION_MIGRATE_LOG_TOKEN,
            ],
            expected_migration_entity_counts=[("native::MarkerTrail", 2)],
            # The stored floats cannot become u32, so the snapshot fails to
            # parse and the fallback resets the WHOLE component on migrated
            # rows (name reverts too, visible as the empty string) while the
            # reload itself completes and new rows carry the new shape.
            # A reset row sums an empty Vec<u32>: f64's Sum identity is a
            # negative zero, hence the `-0.00`.
            expected_witness_tokens=[
                '[project] witness MarkerTrail(points=0,sum=-0.00,name="",samples=0,last=0.00,stamps=0)',
            ],
        ),
        migration_scenario(
            name="Revert threshold 150 -> 200 (fast path)",
            replacements=[(THRESHOLD_150, THRESHOLD_200)],
            required_tokens=[FAST_PATH_TOKEN],
            forbidden_tokens=[SELECTIVE_START_TOKEN],
        ),
        migration_scenario(
            name="Widen SpatialPosition.horizontal f32 -> f64 (refused: the old column cannot host the new alignment)",
            replacements=[(SPATIAL_POSITION_REORDERED, SPATIAL_POSITION_WIDENED)],
            expect_rejected_reload=True,
            required_tokens=[
                "was re-registered with a different size",
                ROLLBACK_TOKEN,
            ],
            forbidden_tokens=[
                SELECTIVE_START_TOKEN,
                FAST_PATH_TOKEN,
            ],
        ),
    ]


# =============================================================================
# Suite runner
# =============================================================================


def run_suite(cycles: int) -> bool:
    """Runs all scenarios for the requested number of cycles."""
    scenarios = build_scenarios()

    for cycle_index in range(1, cycles + 1):
        print(f"\n{'=' * 60}")
        print(f"  CYCLE {cycle_index} / {cycles}")
        print(f"{'=' * 60}")

        restore_original()
        # Let the restored sources land before the next launch reads them.
        time.sleep(0.3)

        print("\n  [TEST] Launching standalone...")
        try:
            process, monitor = launch_standalone()
        except FileNotFoundError:
            print("  [FAIL] 'cargo' not found. Is Rust installed and on PATH?")
            return False
        except OSError as error:
            print(f"  [FAIL] Could not launch standalone: {error}")
            return False

        cycle_passed = True

        try:
            if not monitor.wait_for(STARTUP_TOKEN, STARTUP_TIMEOUT):
                print("  [FAIL] Standalone did not start in time.")
                return False
            print("  [OK] Standalone started.")

            if not monitor.wait_for(COUNTER_TICK_TOKEN, COUNTER_TICK_TIMEOUT):
                print("  [FAIL] Counter did not tick after startup.")
                return False

            for scenario in scenarios:
                if not run_scenario(scenario, monitor, migration_runner()):
                    cycle_passed = False
                    break

            if cycle_passed:
                print(f"\n  [PASS] Cycle {cycle_index} completed.")

        finally:
            terminate_process(process, monitor)

        if not cycle_passed:
            return False

        if cycle_index < cycles:
            # Pause between cycles so each reload starts from a settled tree.
            time.sleep(CYCLE_PAUSE)

    return True


# =============================================================================
# Build and CLI
# =============================================================================


def build_test_project() -> bool:
    """Builds the native test project crate this suite drives.

    The host itself comes from the shared `prepare_host`; this is the one
    build that is not a host posture, so it stays local.
    """
    print("  [PREP] Building the native test project crate...")
    try:
        tests_project_result = subprocess.run(
            [
                "cargo",
                "build",
                "--manifest-path",
                str(NATIVE_PROJECT_ROOT / "Cargo.toml"),
            ],
            cwd=str(WORKSPACE_ROOT),
            capture_output=True,
            text=True,
            timeout=BUILD_TIMEOUT,
        )
    except subprocess.TimeoutExpired:
        print(f"  [FAIL] native test project build timed out after {BUILD_TIMEOUT} seconds.")
        return False
    except FileNotFoundError:
        print("  [FAIL] 'cargo' not found. Is Rust installed and on PATH?")
        return False

    if tests_project_result.returncode != 0:
        print("  [FAIL] native test project build failed:")
        print(tests_project_result.stderr[-2000:])
        return False

    print("  [OK] native test project built.")
    return True


def apply_timeout_scale(scale: float) -> None:
    """Scales timeout constants for slower machines."""
    global STARTUP_TIMEOUT, RELOAD_TIMEOUT, BUILD_TIMEOUT
    global STABILITY_SLEEP, COUNTER_TICK_TIMEOUT

    STARTUP_TIMEOUT = int(STARTUP_TIMEOUT * scale)
    RELOAD_TIMEOUT = int(RELOAD_TIMEOUT * scale)
    BUILD_TIMEOUT = int(BUILD_TIMEOUT * scale)
    STABILITY_SLEEP = max(1, int(STABILITY_SLEEP * scale))
    COUNTER_TICK_TIMEOUT = int(COUNTER_TICK_TIMEOUT * scale)


def main() -> None:
    """Parses arguments and runs the migration suite."""
    global ORIGINAL_CONTENT

    parser = argparse.ArgumentParser(
        description="Hot-reload migration integration suite for Pill"
    )
    parser.add_argument(
        "--cycles",
        type=int,
        default=1,
        help="Number of full scenario-suite cycles (default: 1)",
    )
    add_common_suite_arguments(parser)
    args = parser.parse_args()

    if args.cycles < 1:
        print("ERROR: --cycles must be >= 1")
        sys.exit(1)
    if args.timeout_scale <= 0:
        print("ERROR: --timeout-scale must be > 0")
        sys.exit(1)

    if not PROJECT_LIB_RS.exists():
        print(f"ERROR: Missing source file: {PROJECT_LIB_RS}")
        sys.exit(1)

    ensure_host_lock()

    apply_timeout_scale(args.timeout_scale)
    ORIGINAL_CONTENT = read_source(PROJECT_LIB_RS)
    original_settings = PROJECT_SETTINGS_PATH.read_text(encoding="utf-8")
    PROJECT_SETTINGS_PATH.write_text(MODULE_FREE_SETTINGS, encoding="utf-8", newline="")

    print("=" * 60)
    print("  Hot-Reload Migration Integration Suite")
    print(f"  Workspace:  {WORKSPACE_ROOT}")
    print(f"  Cycles:     {args.cycles}")
    print(f"  Time scale: {args.timeout_scale}x")
    print("=" * 60)

    if not prepare_host(HOST_POSTURE, args, BUILD_TIMEOUT) or not build_test_project():
        restore_original()
        sys.exit(1)

    passed = False
    try:
        passed = run_suite(args.cycles)
    finally:
        print("\n  [CLEANUP] Restoring original project source...")
        restore_original()
        PROJECT_SETTINGS_PATH.write_text(original_settings, encoding="utf-8", newline="")
        print("  [OK] Source restored.")

    print("\n" + "=" * 60)
    print("  ALL TESTS PASSED" if passed else "  SOME TESTS FAILED")
    print("=" * 60)
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    run_suite_with_timing(main)
