"""
Editor Revision Integration Test for Pill.

PROTECTS: A module reload bumps the editor revision counter, so the editor drops its caches.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)
  - Run from workspace root or any path (script resolves paths itself)

DESCRIPTION
    Verifies the host's editor-revision counter, which the editor's snapshot
    refresh uses to drop caches after any reload/rollback/patch. The counter
    lives on the host and is bumped at each reload transaction; this suite
    proves a real module edit moves it.

    The scenario mirrors `test_hot_reload_suite.py`'s cascade: load `pill_spline`
    into `devops/tests/projects/native`, edit the module's sample-offset constant, and
    wait for the reload transaction. The assertion is the host's own log line
    `editor revision bumped`, which the editor surfaces to integration suites
    without a GUI.

USAGE
  python tests/test_editor_revision.py [--timeout-scale S]

EXAMPLE USAGE
  python tests/test_editor_revision.py
  python tests/test_editor_revision.py --timeout-scale 1.5

--- SCRIPT ---
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path
from typing import Tuple

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# Shared paths, tokens, print wrapper, OutputMonitor, process helpers.
from core import suite_common as common  # noqa: E402
from core.suite_common import *  # noqa: E402,F401,F403

# =============================================================================
# Configuration
# =============================================================================

MODULE_LIB_RS = MODULES_ROOT / "extensions" / "pill_spline" / "modules" / "runtime" / "src" / "pill_spline.rs"

# The host reads extensions from the project settings file, so the test
# backs up and installs its own minimal list, exactly like the auto-reload
# suite does.
TEST_PROJECT_SETTINGS = """\
name: "Editor Revision Test"
build_binary_name: "EditorRevisionTest"
modules:
  - "pill_spline"
"""

PROBE_PREFIX = "midpoint ("
REVISION_BUMP_TOKEN = "editor revision bumped"
PROJECT_RELOAD_TOKEN = "hot reload complete"

STARTUP_TIMEOUT = 90
PREBUILD_TIMEOUT = 300
BASELINE_PROBE_TIMEOUT = 60
MODULE_RELOAD_TIMEOUT = 90
PROBE_UPDATE_TIMEOUT = 60
REVISION_BUMP_TIMEOUT = 60

# The probe's baseline midpoint at t = 0.5 (one decimal): the project's
# reference spline spans the ball spawn points, and the middle spawn point
# sits at (390, 120). The vertical position moves with the module's sample
# offset; the horizontal one does not.
BASE_PROBE_MIDPOINT_X = 390.0
BASE_PROBE_MIDPOINT_Y = 120.0

ORIGINAL_CONTENT: str = ""


# =============================================================================
# Source helpers
# =============================================================================


def restore_original() -> None:
    """Restores the module source captured at script startup."""
    if not ORIGINAL_CONTENT:
        return
    if read_source(MODULE_LIB_RS) == ORIGINAL_CONTENT:
        return
    atomic_write(MODULE_LIB_RS, ORIGINAL_CONTENT)


def plan_value_edit(content: str) -> Tuple[str, str, str]:
    """Returns (old line, replacement line, expected new probe midpoint)."""
    edit = edit_spline_sample_offset(content)
    new_y = BASE_PROBE_MIDPOINT_Y + edit.new_offset
    return (
        edit.old_line,
        edit.new_line,
        f"midpoint ({BASE_PROBE_MIDPOINT_X:.1f}, {new_y:.1f})",
    )


# =============================================================================
# Process helpers
# =============================================================================

# `rendering` is required rather than optional: devops/tests/projects/native links
# `pill_master_renderer`, so a host without it resolves `pill_core` differently
# from the project and the project DLL fails to load with "The specified
# procedure could not be found" (os error 127), the same reason the cascade
# suite pins it. No function hot patching: this suite measures the plain reload
# path.
HOST_POSTURE = HOST_DEV_RENDERING


def launch_standalone() -> Tuple[subprocess.Popen, OutputMonitor]:
    """Starts the host and returns process + monitor."""
    process_environment = os.environ.copy()
    process_environment["PROJECT_PATH"] = NATIVE_PROJECT_PATH
    return launch_process(host_command(HOST_POSTURE), MODULES_ROOT, process_environment)


def terminate_process(process: subprocess.Popen, monitor: OutputMonitor) -> None:
    """Stops the monitor, terminates the host safely, then cleans stray hosts."""
    common.terminate_process(process, monitor)
    kill_stale_hosts()


# =============================================================================
# Suite
# =============================================================================


def run_suite(expected_midpoint: str) -> bool:
    """Runs the revision-bump scenario once and reports the outcome."""
    print("\n  [TEST] Launching standalone...")
    try:
        process, monitor = launch_standalone()
    except FileNotFoundError:
        print("  [FAIL] 'cargo' not found. Is Rust installed and on PATH?")
        return False
    except OSError as error:
        print(f"  [FAIL] Could not launch standalone: {error}")
        return False

    try:
        if not monitor.wait_for(STARTUP_TOKEN, STARTUP_TIMEOUT):
            print("  [FAIL] Standalone did not start in time.")
            return False
        print("  [OK] Standalone started.")

        if not monitor.wait_for(PROBE_PREFIX, BASELINE_PROBE_TIMEOUT):
            print("  [FAIL] No spline probe report after startup.")
            return False
        print("  [OK] Baseline spline probe observed.")

        # No reload has happened yet, so the revision counter is untouched and
        # the bump token must be absent from the startup log.
        if monitor.wait_for(REVISION_BUMP_TOKEN, 1.0):
            print("  [FAIL] Revision bumped before any edit (startup reload?).")
            return False
        print("  [OK] Baseline revision counter is quiet.")

        print("  [TEST] Editing the module sample offset...")
        old_line, new_line, _ = plan_value_edit(read_source(MODULE_LIB_RS))
        content = read_source(MODULE_LIB_RS).replace(old_line, new_line)
        atomic_write(MODULE_LIB_RS, content)

        if not monitor.wait_for(REVISION_BUMP_TOKEN, REVISION_BUMP_TIMEOUT):
            print("  [FAIL] No `editor revision bumped` after the reload.")
            return False
        print("  [OK] Editor revision bumped after the reload.")

        if not monitor.wait_for(PROJECT_RELOAD_TOKEN, MODULE_RELOAD_TIMEOUT):
            print("  [FAIL] The reload transaction did not complete in time.")
            return False
        print("  [OK] Reload transaction completed.")

        if not monitor.wait_for(expected_midpoint, PROBE_UPDATE_TIMEOUT):
            print("  [FAIL] The probe never reported the edited midpoint.")
            return False
        print("  [OK] Probe reflects the edited module.")

        return True
    finally:
        terminate_process(process, monitor)


# =============================================================================
# Entry point
# =============================================================================


def main() -> int:
    """Parses arguments, restores state, runs the scenario, reports result."""
    parser = argparse.ArgumentParser(description=__doc__)
    add_common_suite_arguments(parser)
    args = parser.parse_args()

    # Timeouts scale together so a slow machine only needs one knob.
    for name in [
        "STARTUP_TIMEOUT",
        "PREBUILD_TIMEOUT",
        "BASELINE_PROBE_TIMEOUT",
        "MODULE_RELOAD_TIMEOUT",
        "PROBE_UPDATE_TIMEOUT",
        "REVISION_BUMP_TIMEOUT",
    ]:
        globals()[name] = int(globals()[name] * args.timeout_scale)

    global ORIGINAL_CONTENT
    ORIGINAL_CONTENT = read_source(MODULE_LIB_RS)
    try:
        _, new_line, expected_midpoint = plan_value_edit(ORIGINAL_CONTENT)
        print(f"  [PLAN] New offset line: {new_line.strip()}")
    except RuntimeError as error:
        print(f"  [FAIL] {error}")
        return 1

    settings_path = project_settings_yaml(NATIVE_PROJECT_ROOT)
    ensure_host_lock()
    kill_stale_hosts()
    BACKUP.capture(MODULE_LIB_RS)
    BACKUP.capture(settings_path)
    write_project_settings(NATIVE_PROJECT_ROOT, TEST_PROJECT_SETTINGS)
    if not prepare_host(HOST_POSTURE, args, PREBUILD_TIMEOUT):
        if not BACKUP.captured(settings_path):
            settings_path.unlink(missing_ok=True)
        BACKUP.restore_all(reset_mtime=False)
        restore_original()
        return 1

    try:
        ok = run_suite(expected_midpoint)
    finally:
        BACKUP.restore_all(reset_mtime=False)
        if not BACKUP.captured(settings_path):
            settings_path.unlink(missing_ok=True)
        restore_original()
        kill_stale_hosts()

    print(f"\n{'PASS' if ok else 'FAIL'}: test_editor_revision")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
