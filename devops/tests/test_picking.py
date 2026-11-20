"""
Picking and selection outline integration test for Rust-Hybrid-ECS.

PROTECTS: Picking returns the entity under the cursor, or nothing, and outlines the hit entity.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo); a GPU the windowed host can render with
  - Run from workspace root or any path (script resolves paths itself)

DESCRIPTION
    Verifies the renderer's pick buffer and selection outlines on
    `devops/tests/projects/scene`, whose models fill the middle of the window,
    through the standalone dev host's test frontend (`PILL_PICK_TEST=1`):

      1. center pick - a pick at the window's center answers with an entity;
      2. corner pick - a pick at its top-left corner answers with nothing;
      3. outline     - the entity the center pick found is outlined, and the
                       host keeps drawing afterwards without a GPU error.

    Both picks are asked in one frame and answered frames later, so this also
    covers the asynchronous readback.

USAGE
  python devops/tests/test_picking.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_picking.py
  python devops/tests/test_picking.py --timeout-scale 1.5
  python devops/tests/test_picking.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path
from typing import Tuple

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core import suite_common as common  # noqa: E402
from core.suite_common import *  # noqa: E402,F401,F403

# =============================================================================
# Configuration
# =============================================================================

PROJECT_PATH = SCENE_PROJECT_PATH

PICKS_REQUESTED_TOKEN = "[pick test] requested center"
CENTER_HIT_TOKEN = "[pick test] center: Entity("
CORNER_TOKEN = "[pick test] corner:"
CORNER_NOTHING_TOKEN = "[pick test] corner: Nothing"
OUTLINED_TOKEN = "[pick test] outlined"
FRAME_STATISTICS_TOKEN = "FPS"
GPU_ERROR_TOKEN = "GPU error"

STARTUP_TIMEOUT = 600
ANSWER_TIMEOUT = 60

# `rendering` is required, not a preference: the project links
# `pill_master_renderer`, so the project build resolves the wgpu graph, and the
# host's engine dylibs must match the resolution every spawned build gets. The
# suite picks the entity the renderer's pick buffer answers with, so it is also
# the posture that carries the renderer at all.
HOST_POSTURE = HOST_DEV_HOT_PATCH_RENDERING


# =============================================================================
# Helpers
# =============================================================================


def launch_host() -> Tuple[subprocess.Popen, OutputMonitor]:
    """Starts the windowed dev host with the picking test on."""
    environment = os.environ.copy()
    environment["PROJECT_PATH"] = PROJECT_PATH
    environment["PILL_PICK_TEST"] = "1"
    return launch_process(host_command(HOST_POSTURE), MODULES_ROOT, environment)


# =============================================================================
# Suite
# =============================================================================


def run_suite() -> bool:
    """Runs the three scenarios against one host."""
    process, monitor = launch_host()
    try:
        if not require(monitor, PICKS_REQUESTED_TOKEN, STARTUP_TIMEOUT, 0, "picks requested"):
            return False
        # Scenario 1: the center pick finds the model.
        if not require(monitor, CENTER_HIT_TOKEN, ANSWER_TIMEOUT, 0, "the center pick hit an entity"):
            return False
        # Scenario 2: the corner pick finds nothing.
        if not monitor.wait_for(CORNER_TOKEN, ANSWER_TIMEOUT, 0):
            print("  [FAIL] the corner pick was never answered")
            return False
        if CORNER_NOTHING_TOKEN not in monitor.output_since(0):
            print("  [FAIL] the corner pick hit something; expected nothing")
            return False
        print("  [OK] the corner pick hit nothing")
        # Scenario 3: the hit entity is outlined and the host keeps drawing.
        if not require(monitor, OUTLINED_TOKEN, ANSWER_TIMEOUT, 0, "the hit entity was outlined"):
            return False
        mark = monitor.line_count
        if not require(monitor, FRAME_STATISTICS_TOKEN, 30, mark, "the host kept drawing with the outline"):
            return False
        output = monitor.output_since(0)
        if GPU_ERROR_TOKEN in output:
            print("  [FAIL] The host log carries a GPU error.")
            return False
        if common.has_crash_signals(output):
            print("  [FAIL] The host log carries a crash signal.")
            return False
        return True
    finally:
        common.terminate_process(process, monitor)
        kill_stale_hosts()


def main() -> None:
    """Parses arguments and runs the suite."""
    global STARTUP_TIMEOUT, ANSWER_TIMEOUT
    parser = argparse.ArgumentParser(description="Picking and selection outline integration test")
    add_common_suite_arguments(parser)
    args = parser.parse_args()
    if args.timeout_scale <= 0:
        print("ERROR: --timeout-scale must be > 0")
        sys.exit(1)
    STARTUP_TIMEOUT = int(STARTUP_TIMEOUT * args.timeout_scale)
    ANSWER_TIMEOUT = int(ANSWER_TIMEOUT * args.timeout_scale)

    ensure_host_lock()
    print("=" * 60)
    print("  Picking And Outline Integration Test")
    print("=" * 60)
    kill_stale_hosts()
    if not prepare_host(HOST_POSTURE, args, BUILD_TIMEOUT):
        sys.exit(1)
    passed = run_suite()
    print("\n" + "=" * 60)
    print("  TEST PASSED" if passed else "  TEST FAILED")
    print("=" * 60)
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    run_suite_with_timing(main)
