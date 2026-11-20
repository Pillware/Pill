"""
Patch bookkeeping survival test for Rust-Hybrid-ECS.

PROTECTS: A failed reload keeps the live patch records, so rolling the patch back still works.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)

DESCRIPTION
    Pins the order between a project reload and the patch bookkeeping it
    invalidates: the records may only be dropped once the image they point
    into has actually been replaced.

    A reload attempt that fails - build error, load refusal, rolled-back init -
    keeps the current image running, and with it every live patch installed in
    it. Dropping the patch records on the attempt rather than on the outcome
    made the session misreport what is running: `list` stops naming the live
    generation, a rollback of the still-installed patch refuses with "has not
    been patched in this session", and the developer loses the way back
    through the history until the function is patched again.

    The scenario is three saves and one request:

      1. A body-only edit to an annotated project system, delivered by the
         per-function fast path. The host reports `[hot] ... LIVE ...` with
         the generation number.
      2. A structural edit that breaks the project build, so the reload that
         picks it up fails and keeps the current image. The host reports
         `build failed; keeping the old project module`.
      3. A rollback request for the function patched in step 1, while that
         failed reload's image is still the one running. The rollback must
         succeed (the patch is still installed and its records still name it);
         before the fix it refused because the failed attempt had already
         dropped the bookkeeping.

    The build failure is a deliberately unparseable tail appended to the same
    file, so no fast path can touch it and the reload path is the only one
    that runs.

USAGE
  python devops/tests/test_patch_bookkeeping.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_patch_bookkeeping.py
  python devops/tests/test_patch_bookkeeping.py --timeout-scale 2.0

--- SCRIPT ---
"""

import argparse
import os
import sys
import time
from pathlib import Path
from typing import Optional

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
# The edit planner is shared with the coverage suite rather than copied, so the
# two stay in step about which functions are patchable.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from core import suite_common as common  # noqa: E402,F401
from core.suite_common import *  # noqa: E402,F401,F403
from test_hot_patch_coverage import PATCH_APPLIED_PATTERN, Crate, plan_edit  # noqa: E402

# =============================================================================
# Configuration
# =============================================================================

# The rollback request file, relative to the host's working directory, which is
# `modules/` here. The host deletes it as soon as it is read.
ROLLBACK_REQUEST = MODULES_ROOT / "target" / "hot" / "rollback.request"

# Launched through cargo for the same reason the coverage suite does: the
# binary needs the toolchain's dynamic runtime on the loader path, and
# `rendering` is required because devops/tests/projects/native links the renderer.
HOST_POSTURE = HOST_DEV_HOT_PATCH_RENDERING

# Reported by the native project reload when the build produced nothing usable
# and the current image was kept. This is the outcome the bookkeeping must be
# gated on.
BUILD_FAILED_TOKEN = "build failed; keeping the old project module"

# Reported once a rollback has reinstalled a generation.
ROLLBACK_DONE_TOKEN = "patch generation rolled back"

# Reported instead when the records a rollback needs were dropped early. Either
# signature means the bookkeeping did not survive the failed reload.
ROLLBACK_REFUSED_TOKENS = (
    "has not been patched in this session",
    "no saved prologue bytes",
)

# Unparseable on purpose: no fast path can deliver it, so the edit can only
# reach the running image through a full reload - which this makes fail.
BROKEN_TAIL = b"\nfn patch_bookkeeping_probe_broken( {\n"

PATCH_TIMEOUT = 120
RELOAD_TIMEOUT = 120
ROLLBACK_TIMEOUT = 60
BUILD_TIMEOUT_SECONDS = 600


def write_rollback_request(request: str) -> None:
    """Drop one request file for the running host to pick up."""
    ROLLBACK_REQUEST.parent.mkdir(parents=True, exist_ok=True)
    ROLLBACK_REQUEST.write_text(request + "\n", encoding="utf-8")


def clear_rollback_request() -> None:
    """Remove a request file a previous run may have left behind."""
    try:
        ROLLBACK_REQUEST.unlink()
    except FileNotFoundError:
        pass


def wait_for_patch(monitor: OutputMonitor, start_index: int) -> Optional[str]:
    """Wait for the fast path to report the edit it delivered.

    Returns the qualified name the report used, which is the name the rollback
    request has to name - the bare function name is not a key the session
    bookkeeping knows.
    """
    deadline = time.time() + PATCH_TIMEOUT
    while time.time() < deadline:
        applied = PATCH_APPLIED_PATTERN.search(monitor.output_since(start_index))
        if applied:
            print(f"  [OK] Patched: {applied.group(1)} "
                  f"({applied.group(2)} ms via {applied.group(3)})")
            return applied.group(1)
        if not monitor.process_alive():
            print("  [FAIL] The host exited before reporting the patch.")
            return None
        # Poll until the patch outcome arrives.
        time.sleep(0.2)
    print(f"  [FAIL] No fast-path patch within {PATCH_TIMEOUT}s.")
    return None


def wait_for_failed_reload(monitor: OutputMonitor, start_index: int) -> bool:
    """Wait for the broken save to fail its reload and keep the current image."""
    if monitor.wait_for(BUILD_FAILED_TOKEN, RELOAD_TIMEOUT, start_index):
        print("  [OK] The broken save failed its reload; the current image was kept.")
        return True
    if not monitor.process_alive():
        print("  [FAIL] The host exited during the broken reload.")
        return False
    print(f"  [FAIL] No failed-reload report within {RELOAD_TIMEOUT}s.")
    return False


def wait_for_rollback(monitor: OutputMonitor, start_index: int) -> bool:
    """Wait for the rollback to land, and report any refusal it printed."""
    deadline = time.time() + ROLLBACK_TIMEOUT
    while time.time() < deadline:
        output = monitor.output_since(start_index)
        if ROLLBACK_DONE_TOKEN in output:
            print("  [OK] The rollback succeeded; the records survived the failed reload.")
            return True
        for refused in ROLLBACK_REFUSED_TOKENS:
            if refused in output:
                print("  [FAIL] The rollback was refused after a failed reload:")
                print(f"         {refused!r}")
                print("         The failed attempt dropped the patch bookkeeping that")
                print("         the still-installed patch needs; the clear must be gated")
                print("         on the image actually changing.")
                return False
        if not monitor.process_alive():
            print("  [FAIL] The host exited during the rollback.")
            return False
        # Poll until the rollback outcome arrives.
        time.sleep(0.2)
    print(f"  [FAIL] No rollback outcome within {ROLLBACK_TIMEOUT}s.")
    return False


def run_bookkeeping_scenario(monitor: OutputMonitor, backups: BackupRegistry) -> bool:
    """Patch, break the build, and roll the patch back through the failure."""
    crate = Crate(NATIVE_PROJECT_ROOT.name, NATIVE_PROJECT_ROOT / "src", "project")
    if not plan_edit(crate):
        print(
            "  [FAIL] No patchable function body found in {}.".format(
                repository_relative(NATIVE_PROJECT_ROOT)
            )
        )
        return False

    backups.capture(crate.edit_file)
    original = crate.edit_file.read_bytes()
    edited = original.replace(crate.edit_from.encode(), crate.edit_to.encode(), 1)
    if edited == original:
        print(f"  [FAIL] Edit text {crate.edit_from!r} not found as written.")
        return False

    # Save 1: body-only, so the fast path delivers it and a live generation
    # exists for the failed reload to endanger.
    patch_index = monitor.line_count
    crate.edit_file.write_bytes(edited)
    print(f"  [EDIT 1] {crate.edit_file.name}: fn {crate.function} "
          f"{crate.edit_from} -> {crate.edit_to}")
    qualified = wait_for_patch(monitor, patch_index)
    if qualified is None:
        return False
    # Let the patch land and go quiet before the failing save.
    time.sleep(0.5)

    # Save 2: break the build, so the reload this triggers fails and the image
    # with the live patch stays current.
    reload_index = monitor.line_count
    crate.edit_file.write_bytes(crate.edit_file.read_bytes() + BROKEN_TAIL)
    print("  [EDIT 2] appended an unparseable tail; the project build must now fail")
    if not wait_for_failed_reload(monitor, reload_index):
        return False
    # Let the failed build finish unwinding before the rollback.
    time.sleep(0.5)

    # The rollback: the patch is still installed in the unreplaced image, so
    # its records must still name it.
    rollback_index = monitor.line_count
    write_rollback_request(f"{qualified}@0")
    print(f"  [REQUEST] rollback {qualified}@0")
    return wait_for_rollback(monitor, rollback_index)


def main() -> None:
    """Launch the host, run the scenario, restore the source."""
    parser = argparse.ArgumentParser(
        description="Assert patch bookkeeping survives a failed project reload"
    )
    add_common_suite_arguments(parser)
    args = parser.parse_args()

    global PATCH_TIMEOUT, RELOAD_TIMEOUT, ROLLBACK_TIMEOUT
    PATCH_TIMEOUT = int(PATCH_TIMEOUT * args.timeout_scale)
    RELOAD_TIMEOUT = int(RELOAD_TIMEOUT * args.timeout_scale)
    ROLLBACK_TIMEOUT = int(ROLLBACK_TIMEOUT * args.timeout_scale)
    startup_timeout = int(STARTUP_TIMEOUT * args.timeout_scale)

    print("=" * 70)
    print("  Patch Bookkeeping Survival Test")
    print(f"  Workspace: {WORKSPACE_ROOT}")
    print("=" * 70)

    ensure_host_lock()
    kill_stale_hosts()
    clear_rollback_request()
    if not prepare_host(HOST_POSTURE, args, BUILD_TIMEOUT_SECONDS):
        sys.exit(1)

    environment = os.environ.copy()
    environment["PROJECT_PATH"] = NATIVE_PROJECT_PATH
    process, monitor = launch_process(host_command(HOST_POSTURE), MODULES_ROOT, environment)
    backups = BACKUP
    passed = False
    try:
        if not monitor.wait_for(STARTUP_TOKEN, startup_timeout):
            print(f"  [FAIL] Host did not start within {startup_timeout}s.")
            sys.exit(1)
        print("  [OK] Host running.\n")
        passed = run_bookkeeping_scenario(monitor, backups)
    finally:
        print("\n  [CLEANUP] Restoring source and stopping the host...")
        clear_rollback_request()
        # The restored source is the only restore of these edits, and the host
        # is killed immediately after, so the artifact on disk is still the
        # broken save's failed build. Rewinding the mtime would make the next
        # host start trust a stale artifact as up to date; leave the file
        # stamped "now" so the next build regenerates the original.
        BACKUP.restore_all(reset_mtime=False)
        terminate_process(process, monitor)
        print("  [OK] Restored.")

    print("\n" + "=" * 70)
    print("  TEST PASSED" if passed else "  TEST FAILED")
    print("=" * 70)
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    run_suite_with_timing(main)
