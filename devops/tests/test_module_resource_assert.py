"""
Missing module resource assert suite for Pill.

PROTECTS: A module's declared asset root stops the host, naming the module and the path, when a required file is gone.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)

DESCRIPTION
    A module declares the read-only asset root it ships and the files inside it
    that must be there (`asset_store::declare_module_root`), and
    `asset_store::verify_module_roots` re-checks every root afterwards, outside
    the module-init `catch_unwind`, for a resource that went missing later.

    This suite proves the failing startup end to end, on the one real module
    resource: `pill_rendering` requires
    `res/materials/pill.pbr.material`, the PBR chain's default material.

      1. The file is moved aside and the headless dev host is started on
         `devops/tests/projects/native`.
      2. The host must exit non-zero, and its output must carry
         `module 'pill_rendering' requires its resource
         'materials/pill.pbr.material', which is missing at` - the module and the
         path, which is what makes the failure diagnosable. The declaration
         asserts inside the module's init, so the loader reports the panic and
         then fails the host; `verify_module_roots` is the same assertion for a
         resource deleted after it was declared, which the engine's unit test
         `a_vanished_resource_fails_the_verification` covers.
      3. It must never reach the project loop: the asset watcher's line, printed
         once the project has loaded, must be absent.
      4. The file is restored byte for byte whatever happens, and the restore is
         verified by reading it back.

    A leftover from an interrupted run (the file still moved aside) is restored
    before anything else happens, so the suite heals its own debris.

USAGE
  python devops/tests/test_module_resource_assert.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_module_resource_assert.py
  python devops/tests/test_module_resource_assert.py --timeout-scale 2

--- SCRIPT ---
"""

import argparse
import os
import subprocess
import sys
import time
from pathlib import Path

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.suite_common import *  # noqa: E402,F401,F403

# =============================================================================
# Configuration
# =============================================================================

PROJECT_ROOT = NATIVE_PROJECT_ROOT
# Headless: the assert fires while the modules load, before any window exists.
HOST_POSTURE = HOST_DEV_HOT_PATCH

MODULE_NAME = "pill_rendering"
RESOURCE_NAME = "materials/pill.pbr.material"
MATERIAL_FILE = (
    MODULES_ROOT
    / "extensions"
    / "rendering"
    / "pill_rendering_core"
    / "modules"
    / "runtime"
    / "data"
    / "res"
    / "materials"
    / "pill.pbr.material"
)
# Where the file waits while the host is started without it; same directory, so
# the move is one rename and nothing can be lost across a volume.
PARKED_FILE = MATERIAL_FILE.with_name(MATERIAL_FILE.name + ".suite-parked")

# The assert's message, as `assert_module_root_complete` spells it: the module
# and the path are the two halves that make a failure diagnosable.
MISSING_RESOURCE_TOKEN = (
    f"module '{MODULE_NAME}' requires its resource "
    f"'{RESOURCE_NAME}', which is missing at"
)
# Printed once the project has loaded, so its absence proves the host stopped
# before the loop rather than starting and failing later.
PROJECT_LOOP_TOKEN = "[assets] watching the project's assets for edits"

EXIT_TIMEOUT = 240
BUILD_TIMEOUT_SECONDS = 1800


def heal_leftover() -> bool:
    """
    Puts a parked file back if an earlier run died before its cleanup.

    Returns True when nothing is in the way. A run killed mid-flight leaves the
    resource parked, which would break every later host start; the next run
    restores it instead of inheriting the breakage.
    """
    if not PARKED_FILE.exists():
        return True
    if MATERIAL_FILE.exists():
        print(f"  [FAIL] both {MATERIAL_FILE.name} and its parked copy exist.")
        return False
    PARKED_FILE.replace(MATERIAL_FILE)
    print(f"  [CLEANUP] restored a parked {RESOURCE_NAME} from an earlier run")
    return True


def park_resource() -> bool:
    """Moves the required resource aside so the host cannot find it."""
    if not MATERIAL_FILE.is_file():
        print(f"  [FAIL] {MATERIAL_FILE} is not there to begin with.")
        return False
    MATERIAL_FILE.replace(PARKED_FILE)
    return not MATERIAL_FILE.exists()


def restore_resource() -> bool:
    """Puts the resource back and proves it is back, byte for byte."""
    if PARKED_FILE.exists():
        PARKED_FILE.replace(MATERIAL_FILE)
    restored = MATERIAL_FILE.is_file() and not PARKED_FILE.exists()
    if restored:
        print(f"  [CLEANUP] restored {RESOURCE_NAME}")
    return restored


def main() -> int:
    """Starts the host without a declared resource and reads what it says."""
    parser = argparse.ArgumentParser(description="Missing module resource assert suite")
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()

    exit_timeout = int(EXIT_TIMEOUT * arguments.timeout_scale)

    print("=" * 70)
    print("  Missing Module Resource Assert Suite")
    print(f"  Resource: {RESOURCE_NAME} of {MODULE_NAME}")
    print("=" * 70)

    if not heal_leftover():
        return 1
    ensure_host_lock()
    kill_stale_hosts()
    if not prepare_host(HOST_POSTURE, arguments, BUILD_TIMEOUT_SECONDS):
        return 1

    if not park_resource():
        return 1
    environment = os.environ.copy()
    environment["PROJECT_PATH"] = NATIVE_PROJECT_PATH
    process, monitor = launch_process(host_command(HOST_POSTURE), MODULES_ROOT, environment)
    passed = False
    try:
        try:
            exit_code = process.wait(timeout=exit_timeout)
        except subprocess.TimeoutExpired:
            print(f"  [FAIL] The host was still running after {exit_timeout}s.")
            return 1
        # The reader thread drains the pipe until it closes, which happens as
        # the process exits; the assert's line is among the last, so wait for it
        # rather than reading once and racing the drain.
        drain_deadline = time.monotonic() + 15
        while time.monotonic() < drain_deadline:
            if MISSING_RESOURCE_TOKEN in monitor.output_since(0):
                break
            # Poll so the assert's line is seen as soon as the drain lands.
            time.sleep(0.1)
        output = monitor.output_since(0)
        if exit_code == 0:
            print("  [FAIL] The host started without a declared resource.")
        elif MISSING_RESOURCE_TOKEN not in output:
            print("  [FAIL] The host failed without naming the missing resource:")
            print("\n".join(output.splitlines()[-30:]))
        elif PROJECT_LOOP_TOKEN in output:
            print("  [FAIL] The host reached its project loop before failing.")
        else:
            print(f"  [OK] The host stopped and named the missing resource: {exit_code}")
            passed = True
    finally:
        terminate_process(process, monitor)
        if not restore_resource():
            print("  [FAIL] The resource could not be restored.")
            passed = False

    print("\n" + "=" * 70)
    print("  TEST PASSED" if passed else "  TEST FAILED")
    print("=" * 70)
    return 0 if passed else 1


if __name__ == "__main__":
    run_suite_with_timing(main)
