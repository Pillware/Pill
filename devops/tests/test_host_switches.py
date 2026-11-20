#!/usr/bin/env python3
"""
The three hot-reload switches each stop exactly the watches they name.

PROTECTS: `PILL_HOT_RELOADING`, `PILL_FUNCTION_HOT_PATCHING` and `PILL_ASSETS_HOT_RELOADING` each switch off one watch, in a real host, without stopping the project.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo); the host is built once and launched four times.

DESCRIPTION
    The development host reads three environment switches once, at startup, and
    reports them in one line. This suite starts the host four times against
    `devops/tests/projects/native` - once per posture, because a value read once
    cannot be changed in a running host - and reads the log each time:

      1. defaults - the switch line reports all three on, the project's assets
         are watched, and the source watchers started for every module.
      2. `PILL_HOT_RELOADING=0` - an edit to the project's
         `src/simulation_time.rs` is never noticed: no `hot reload triggered`
         appears while the project's own systems keep printing.
      3. `PILL_FUNCTION_HOT_PATCHING=0` - a body edit in the `#[pill_hot]`
         system reloads the module (`building project module`, then
         `hot reload complete`) instead of installing a patch, and the host
         says once that patching is off.
      4. `PILL_ASSETS_HOT_RELOADING=0` - the asset watcher never starts, the
         host says so, and an edit under the project's `res` produces no
         `[assets] reimported`.

    Every absence is asserted only after the host has been seen running, so a
    host that died cannot pass by saying nothing. Every edited file is restored
    byte for byte, and the host is stopped on every path, including a failure.

USAGE
  python devops/tests/test_host_switches.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_host_switches.py
  python devops/tests/test_host_switches.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import sys
import time
from pathlib import Path

# Standalone-runnable: `devops/` holds `core`, and the suites sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.projects import NATIVE_PROJECT_PATH, NATIVE_PROJECT_ROOT  # noqa: E402
from core.suite_common import (  # noqa: E402
    HOST_DEV,
    MODULES_ROOT,
    add_common_suite_arguments,
    ensure_host_lock,
    host_command,
    launch_process,
    prepare_host,
    run_suite_with_timing,
    terminate_process,
)

# =============================================================================
# Constants
# =============================================================================

PROJECT_ROOT = NATIVE_PROJECT_ROOT

# The three variables, named once: the suite clears all of them from its own
# environment before every launch, so a leftover value in the shell cannot
# decide a scenario.
HOT_RELOADING_VARIABLE = "PILL_HOT_RELOADING"
FUNCTION_HOT_PATCHING_VARIABLE = "PILL_FUNCTION_HOT_PATCHING"
ASSETS_HOT_RELOADING_VARIABLE = "PILL_ASSETS_HOT_RELOADING"
SWITCH_VARIABLES = (
    HOT_RELOADING_VARIABLE,
    FUNCTION_HOT_PATCHING_VARIABLE,
    ASSETS_HOT_RELOADING_VARIABLE,
)

# The project's own sources: a system file whose edit the source watcher would
# reload, the `#[pill_hot]` system whose body the fast path would patch, and an
# asset under `res` whose edit the asset watcher would reimport.
SYSTEM_FILE = PROJECT_ROOT / "src" / "simulation_time.rs"
HOT_SYSTEM_FILE = PROJECT_ROOT / "src" / "physics.rs"
ASSET_FILE = PROJECT_ROOT / "res" / "shaders" / "reload_probe_fragment.hlsl"

# What the edits change. The shader's constant is the one its own suite edits,
# so the file is known to be reimportable.
HOT_SYSTEM_ORIGINAL = "/ 80.0,"
HOT_SYSTEM_EDITED = "/ 81.0,"
ASSET_APPENDED_LINE = "// host switches suite touch\n"

# Host log lines this suite asserts on.
SWITCHES_TOKEN = "the hot reload switches"
SOURCE_WATCH_TOKEN = "watching for source changes"
ASSET_WATCH_TOKEN = "[assets] watching the project's assets for edits"
RELOAD_TRIGGERED_TOKEN = "hot reload triggered"
BUILDING_TOKEN = "building project module"
RELOAD_COMPLETE_TOKEN = "hot reload complete"
PATCH_INSTALLED_TOKEN = "patch generation installed"
PATCHING_OFF_TOKEN = "per-function hot patching is switched off"
ASSETS_OFF_TOKEN = "[assets] asset hot reloading is switched off"
# Printed by the test project's own system on every frame, so it only appears once
# the project initialized and the host runs its systems.
PROJECT_ALIVE_TOKEN = "[project]"

# A cold start builds every module and the project; a warm one is seconds.
STARTUP_TIMEOUT_SECONDS = 240
# A reload has a cargo build in front of it on a warm cache.
RELOAD_TIMEOUT_SECONDS = 180
# How long an absence is watched for. Long enough for a watcher to notice an
# edit and act on it, short enough to keep four host starts in one suite.
QUIET_WINDOW_SECONDS = 25


# =============================================================================
# Free functions
# =============================================================================


def lines_with(text: str, token: str) -> list:
    """Every line in `text` that carries `token`, oldest first."""
    return [line for line in text.splitlines() if token in line]


def first_line_with(text: str, token: str) -> str:
    """The first line in `text` that carries `token`, or an empty string."""
    matches = lines_with(text, token)
    return matches[0] if matches else ""


def edited_bytes(path: Path, original: str, replacement: str) -> bytes:
    """The file's bytes with `original` replaced by `replacement`.

    Reads and writes bytes rather than text so the line endings the file has
    stay the ones it keeps, and raises when the anchor is missing: a silent
    no-op edit would turn an assertion about a reload into an assertion about
    nothing.
    """
    source_bytes = path.read_bytes()
    edited = source_bytes.replace(original.encode("utf-8"), replacement.encode("utf-8"))
    if edited == source_bytes:
        raise RuntimeError("{} does not hold {!r}".format(path, original))
    return edited


def launch(overrides: dict):
    """Starts the development host with the switch variables it is given."""
    environment = {
        key: value for key, value in os.environ.items() if key not in SWITCH_VARIABLES
    }
    environment["PROJECT_PATH"] = NATIVE_PROJECT_PATH
    environment.update(overrides)
    return launch_process(host_command(HOST_DEV), MODULES_ROOT, environment)


def wait_until_running(monitor, scale: float) -> bool:
    """Waits for the switch line and for the project's own frame traffic."""
    if not monitor.wait_for(SWITCHES_TOKEN, STARTUP_TIMEOUT_SECONDS * scale):
        return False
    return monitor.wait_for(PROJECT_ALIVE_TOKEN, STARTUP_TIMEOUT_SECONDS * scale)


def check_quiet(monitor, mark: int, absent_tokens: list, seconds: float) -> list:
    """Watches for `seconds` and reports whether any `absent_tokens` appeared.

    The liveness check is part of the assertion: an absence only means
    something while the host is known to be running its systems.
    """
    alive_mark = monitor.line_count
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if not monitor.process_alive():
            return [(False, "the host stayed up for the whole wait window")]
        # Polled rather than slept once: a host that died mid-window has to end
        # the wait, or a crash would read as a quiet watcher.
        time.sleep(0.25)
    after = monitor.output_since(mark)
    appeared = [token for token in absent_tokens if token in after]
    return [
        (
            not appeared,
            "nothing matched {}".format(", ".join(repr(token) for token in absent_tokens)),
        ),
        (
            PROJECT_ALIVE_TOKEN in monitor.output_since(alive_mark),
            "the project's systems kept printing while nothing else happened",
        ),
    ]


def scenario_defaults(arguments) -> list:
    """One host with no switches set: everything is watched."""
    process, monitor = launch({})
    try:
        if not wait_until_running(monitor, arguments.timeout_scale):
            return [(False, "the host started with no switches set")]
        # The asset and settings watchers start after the project loop is
        # entered, one or two lines after the liveness token the wait above
        # returns on. Wait for the watcher rather than snapshot in the window
        # between them: how much a test project prints while starting up decides
        # whether that snapshot happens to be wide enough, which is not a
        # property this check should depend on.
        monitor.wait_for(ASSET_WATCH_TOKEN, STARTUP_TIMEOUT_SECONDS * arguments.timeout_scale)
        text = monitor.output_since(0)
        switch_line = first_line_with(text, SWITCHES_TOKEN)
        return [
            (
                "hot_reloading=true" in switch_line
                and "function_hot_patching=true" in switch_line
                and "assets_hot_reloading=true" in switch_line,
                "the switch line reports hot reloading, patching and assets all on",
            ),
            (
                bool(lines_with(text, SOURCE_WATCH_TOKEN)),
                "the source watchers started for the loaded modules",
            ),
            (
                bool(lines_with(text, ASSET_WATCH_TOKEN)),
                "the project's asset directory is watched",
            ),
        ]
    finally:
        terminate_process(process, monitor)


def scenario_hot_reloading_off(arguments) -> list:
    """One host with `PILL_HOT_RELOADING=0`: an edit is not noticed."""
    original = SYSTEM_FILE.read_bytes()
    process, monitor = launch({HOT_RELOADING_VARIABLE: "0"})
    try:
        if not wait_until_running(monitor, arguments.timeout_scale):
            return [(False, "the host started with hot reloading off")]
        checks = [
            (
                "hot_reloading=false"
                in first_line_with(monitor.output_since(0), SWITCHES_TOKEN),
                "the switch line reports hot reloading off",
            )
        ]
        mark = monitor.line_count
        SYSTEM_FILE.write_bytes(
            original + b"\n// the host switches suite edited this file\n"
        )
        checks.extend(
            check_quiet(
                monitor, mark, [RELOAD_TRIGGERED_TOKEN, BUILDING_TOKEN],
                QUIET_WINDOW_SECONDS * arguments.timeout_scale,
            )
        )
        return checks
    finally:
        SYSTEM_FILE.write_bytes(original)
        terminate_process(process, monitor)


def scenario_function_hot_patching_off(arguments) -> list:
    """One host with `PILL_FUNCTION_HOT_PATCHING=0`: an edit reloads."""
    original = HOT_SYSTEM_FILE.read_bytes()
    edited = edited_bytes(HOT_SYSTEM_FILE, HOT_SYSTEM_ORIGINAL, HOT_SYSTEM_EDITED)
    process, monitor = launch({FUNCTION_HOT_PATCHING_VARIABLE: "0"})
    try:
        if not wait_until_running(monitor, arguments.timeout_scale):
            return [(False, "the host started with function hot patching off")]
        # The project's own frames print before its source watcher is armed, and
        # a change made in that window is never reported. The host says patching
        # is off once it enters its project loop, after every watcher is armed,
        # so wait for that line before writing the edit.
        if not monitor.wait_for(
            PATCHING_OFF_TOKEN, STARTUP_TIMEOUT_SECONDS * arguments.timeout_scale
        ):
            return [(False, "the host said patching is switched off")]
        mark = monitor.line_count
        HOT_SYSTEM_FILE.write_bytes(edited)
        if not monitor.wait_for(
            BUILDING_TOKEN, RELOAD_TIMEOUT_SECONDS * arguments.timeout_scale, mark
        ):
            return [(False, "a body edit in a `#[pill_hot]` system rebuilt the module")]
        if not monitor.wait_for(
            RELOAD_COMPLETE_TOKEN, RELOAD_TIMEOUT_SECONDS * arguments.timeout_scale, mark
        ):
            return [(False, "the rebuilt module reloaded")]
        after = monitor.output_since(mark)
        return [
            (True, "a body edit in a `#[pill_hot]` system rebuilt the module"),
            (True, "the rebuilt module reloaded"),
            (
                PATCH_INSTALLED_TOKEN not in after,
                "no patch was installed for the edit",
            ),
            (
                # Said once, when the host first asks the fast path, so it is
                # in the log before the edit rather than after it.
                bool(lines_with(monitor.output_since(0), PATCHING_OFF_TOKEN)),
                "the host said once that patching is switched off",
            ),
        ]
    finally:
        HOT_SYSTEM_FILE.write_bytes(original)
        terminate_process(process, monitor)


def scenario_assets_off(arguments) -> list:
    """One host with `PILL_ASSETS_HOT_RELOADING=0`: no asset watch at all."""
    original = ASSET_FILE.read_bytes()
    process, monitor = launch({ASSETS_HOT_RELOADING_VARIABLE: "0"})
    try:
        if not wait_until_running(monitor, arguments.timeout_scale):
            return [(False, "the host started with asset hot reloading off")]
        # The asset watcher starts after the project module is loaded, so the
        # switch-off line lands a few lines after the liveness token the wait
        # above returns on. Wait for it rather than snapshot in that window.
        monitor.wait_for(ASSETS_OFF_TOKEN, STARTUP_TIMEOUT_SECONDS * arguments.timeout_scale)
        text = monitor.output_since(0)
        mark = monitor.line_count
        ASSET_FILE.write_bytes(original + ASSET_APPENDED_LINE.encode("utf-8"))
        checks = [
            (
                "assets_hot_reloading=false" in first_line_with(text, SWITCHES_TOKEN),
                "the switch line reports asset hot reloading off",
            ),
            (ASSETS_OFF_TOKEN in text, "the host said the asset watcher is off"),
            (
                not lines_with(text, ASSET_WATCH_TOKEN),
                "the project's asset directory is not watched",
            ),
        ]
        checks.extend(
            check_quiet(
                monitor, mark, ["[assets] reimported"],
                QUIET_WINDOW_SECONDS * arguments.timeout_scale,
            )
        )
        return checks
    finally:
        ASSET_FILE.write_bytes(original)
        terminate_process(process, monitor)


def report(name: str, checks: list) -> int:
    """Prints one scenario's checks and returns the number that failed."""
    print("\n  --- {} ---".format(name))
    failures = 0
    for ok, description in checks:
        print("  [{}] {}".format("OK" if ok else "FAIL", description))
        if not ok:
            failures += 1
    if failures:
        print("  [FAIL] {}".format(name))
    else:
        print("  [OK] {}".format(name))
    return failures


def main() -> int:
    """Runs the four postures, each in its own host, and reports every check."""
    parser = argparse.ArgumentParser(
        description="Check that each hot-reload switch stops what it names."
    )
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()

    print("\n  Host switches suite")
    print("  Project: {}".format(PROJECT_ROOT))

    ensure_host_lock()
    if not prepare_host(HOST_DEV, arguments):
        return 1

    failures = 0
    for name, scenario in (
        ("defaults", scenario_defaults),
        ("{} off".format(HOT_RELOADING_VARIABLE), scenario_hot_reloading_off),
        (
            "{} off".format(FUNCTION_HOT_PATCHING_VARIABLE),
            scenario_function_hot_patching_off,
        ),
        ("{} off".format(ASSETS_HOT_RELOADING_VARIABLE), scenario_assets_off),
    ):
        failures += report(name, scenario(arguments))

    if failures:
        print("\n  [FAIL] {} check(s) failed".format(failures))
        return 1
    print("\n  [OK] every switch stopped exactly what it names")
    return 0


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
