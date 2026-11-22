"""
Multi-surface renderer integration test for Pill.

PROTECTS: One renderer drives two windows; a renderer reload reattaches the second surface.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo); a GPU the windowed host can render with
  - Run from workspace root or any path (script resolves paths itself)

DESCRIPTION
    Verifies that one renderer draws one scene into two windows through two
    cameras, on `devops/tests/projects/scene`, through the standalone dev
    host's test frontend (`PILL_SECOND_VIEW=1` opens a second window as
    surface 1 with a camera of its own):

      1. both surfaces - the main window and surface 1 each present a frame;
      2. reload        - editing the renderer's GPU module reloads it, and
                         surface 1 is reattached and presents again;
      3. close one     - closing the second window (`PILL_SECOND_VIEW_SECONDS`)
                         detaches only surface 1, and the main window keeps
                         drawing.

    The edited GPU module source is backed up and restored.

USAGE
  python devops/tests/test_multi_surface.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_multi_surface.py
  python devops/tests/test_multi_surface.py --timeout-scale 1.5

--- SCRIPT ---
"""

import argparse
import os
import subprocess
import sys
import time
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
GPU_MODULE_SOURCE = (
    MODULES_ROOT / "extensions" / "rendering" / "pill_renderer" / "modules" / "runtime" / "src" / "pill_renderer.rs"
)

PRIMARY_FIRST_FRAME_TOKEN = "[render] First frame"
SECOND_VIEW_OPENED_TOKEN = "[second view] opened as surface 1"
SURFACE_ATTACHED_TOKEN = "surface 1 attached"
SURFACE_PRESENTED_TOKEN = "[render] surface 1 first frame presented"
RENDERER_REATTACHED_TOKEN = "renderer reattached to the window"
SURFACE_REATTACHED_TOKEN = "surface 1 reattached to the new renderer"
SECOND_VIEW_CLOSED_TOKEN = "[second view] closed"
SURFACE_DETACHED_TOKEN = "surface 1 detached"
# The engine's frame-statistics line (`pill_player`'s heartbeat), logged about
# every three seconds while the frame loop runs. Seeing it after the second
# window closed is what shows the main window is still being driven.
FRAME_STATISTICS_TOKEN = "Heartbeat | fps="

STARTUP_TIMEOUT = 600
RELOAD_TIMEOUT = 300
# The second window closes itself after this many seconds - long enough for
# the reload in between.
SECOND_VIEW_SECONDS = 150

ORIGINAL_SOURCE = ""


# =============================================================================
# Helpers
# =============================================================================

# `rendering` is what this suite measures; the engine's `development` feature
# stays on with it because the two are one resolved graph here (the dev host
# pins it).
HOST_POSTURE = HOST_DEV_HOT_PATCH_RENDERING


def launch_host() -> Tuple[subprocess.Popen, OutputMonitor]:
    """Starts the windowed dev host with the second view open."""
    environment = os.environ.copy()
    environment["PROJECT_PATH"] = PROJECT_PATH
    environment["PILL_SECOND_VIEW"] = "1"
    environment["PILL_SECOND_VIEW_SECONDS"] = str(SECOND_VIEW_SECONDS)
    return launch_process(host_command(HOST_POSTURE), MODULES_ROOT, environment)


def restore_source() -> None:
    """Restores the GPU module source captured at startup."""
    if ORIGINAL_SOURCE and GPU_MODULE_SOURCE.read_text(encoding="utf-8") != ORIGINAL_SOURCE:
        common.atomic_write(GPU_MODULE_SOURCE, ORIGINAL_SOURCE)


# =============================================================================
# Suite
# =============================================================================


def run_suite() -> bool:
    """Runs the three scenarios against one host."""
    process, monitor = launch_host()
    try:
        # Scenario 1: both surfaces present.
        if not require(monitor, SECOND_VIEW_OPENED_TOKEN, STARTUP_TIMEOUT, 0, "second window opened"):
            return False
        if not require(monitor, SURFACE_ATTACHED_TOKEN, 30, 0, "surface 1 attached"):
            return False
        if not require(monitor, PRIMARY_FIRST_FRAME_TOKEN, 60, 0, "the main window presented"):
            return False
        if not require(monitor, SURFACE_PRESENTED_TOKEN, 60, 0, "surface 1 presented"):
            return False

        # Scenario 2: a renderer reload reattaches surface 1.
        mark = monitor.line_count
        common.atomic_write(
            GPU_MODULE_SOURCE, ORIGINAL_SOURCE + "\n// test_multi_surface.py reload\n"
        )
        if not require(monitor, RENDERER_REATTACHED_TOKEN, RELOAD_TIMEOUT, mark, "the renderer reloaded"):
            return False
        if not require(monitor, SURFACE_REATTACHED_TOKEN, 30, mark, "surface 1 reattached"):
            return False
        if not require(monitor, SURFACE_PRESENTED_TOKEN, 60, mark, "surface 1 presented after the reload"):
            return False
        restore_source()

        # Scenario 3: closing the second window detaches only surface 1.
        mark = monitor.line_count
        if not require(monitor, SECOND_VIEW_CLOSED_TOKEN, SECOND_VIEW_SECONDS + 60, mark, "the second window closed"):
            return False
        if not require(monitor, SURFACE_DETACHED_TOKEN, 30, mark, "surface 1 detached"):
            return False
        closed_mark = monitor.line_count
        if not require(monitor, FRAME_STATISTICS_TOKEN, 30, closed_mark, "the main window kept drawing"):
            return False
        if common.has_crash_signals(monitor.output_since(0)):
            print("  [FAIL] The host log carries a crash signal.")
            return False
        return True
    finally:
        restore_source()
        common.terminate_process(process, monitor)
        kill_stale_hosts()


def main() -> None:
    """Parses arguments and runs the suite."""
    global ORIGINAL_SOURCE, STARTUP_TIMEOUT, RELOAD_TIMEOUT
    parser = argparse.ArgumentParser(description="Multi-surface renderer integration test")
    add_common_suite_arguments(parser)
    args = parser.parse_args()
    if args.timeout_scale <= 0:
        print("ERROR: --timeout-scale must be > 0")
        sys.exit(1)
    STARTUP_TIMEOUT = int(STARTUP_TIMEOUT * args.timeout_scale)
    RELOAD_TIMEOUT = int(RELOAD_TIMEOUT * args.timeout_scale)

    print("=" * 60)
    print("  Multi-Surface Renderer Integration Test")
    print("=" * 60)
    ensure_host_lock()
    kill_stale_hosts()
    if not prepare_host(HOST_POSTURE, args, BUILD_TIMEOUT):
        sys.exit(1)
    ORIGINAL_SOURCE = GPU_MODULE_SOURCE.read_text(encoding="utf-8")
    passed = False
    try:
        passed = run_suite()
    finally:
        restore_source()
    print("\n" + "=" * 60)
    print("  TEST PASSED" if passed else "  TEST FAILED")
    print("=" * 60)
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    run_suite_with_timing(main)
