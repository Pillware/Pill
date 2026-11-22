"""
Edit-during-build regression test for Pill.

PROTECTS: A save made during a build wins: the newer edit is the one built and loaded.
TIER: host
SETS: hot-reload

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)

DESCRIPTION
    Pins the one thing every other hot-reload suite misses: what happens to a
    save that lands WHILE a rebuild triggered by the previous save is running.

    The host watches a generation counter. A save bumps it; the frame loop
    reloads when it differs from the last processed value. `run_build_command`
    polls the same counter mid-build and aborts the moment it advances, so that
    a newer save wins instead of the process adopting code the developer has
    already replaced.

    That cancellation is only useful if the newer save is then built. The
    bookkeeping has to record the generation the reload STARTED from, not a
    fresh read taken after it finished - a fresh read includes the save that
    caused the cancellation and marks it as handled, so nothing ever builds it.
    The edit then sits on disk, never compiled, with no error printed anywhere,
    until something unrelated happens to touch the crate again.

    Every other suite makes one edit and waits, so none of them can see this.
    This one saves twice, the second inside the build window, and requires both
    halves of the story: the host cancelled the in-flight build for the second
    save, AND the newer sources were then built. A rebuild that arrives without
    the cancellation means the second save missed the window - the suite's whole
    subject never happened - so that is a miss to retry with an earlier save,
    not a pass. The retry places its save from the window the missed build
    measured, because a machine whose builds are three times slower moves it.
    Both attempts wait for the host to stop logging first: a save made while
    another build is in flight reloads late, because both builds share cargo's
    lock, and the window this suite measures would not be the one it saved into.

USAGE
  python devops/tests/test_reload_edit_during_build.py [--timeout-scale S]
      [--skip-build] [--second-edit-delay S]

  `--second-edit-delay` is an upper bound: a retry derives its delay from the
  build it measured.

EXAMPLE USAGE
  python devops/tests/test_reload_edit_during_build.py
  python devops/tests/test_reload_edit_during_build.py --second-edit-delay 0.4

--- SCRIPT ---
"""

import argparse
import os
import re
import sys
import time
from pathlib import Path
from typing import Optional, Tuple

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core import suite_common as common  # noqa: E402
from core import log_layout  # noqa: E402
from core.suite_common import *  # noqa: E402,F401,F403

# =============================================================================
# Configuration
# =============================================================================

MODULE_LIB_RS = MODULES_ROOT / "extensions" / "pill_spline" / "modules" / "runtime" / "src" / "pill_spline.rs"
MODULE_NAME = "pill_spline"

# Launched through cargo because `-C prefer-dynamic` means the binary needs the
# toolchain's `std-*.dll` on the loader path, which `cargo run` sets up.
#
# `rendering` is required, not a preference, and this suite ran without it for
# its whole history. `devops/tests/projects/native` links `pill_rendering`
# directly, which turns on features in crates `pill_core` also depends on. The
# host binary links its own `pill_core.dll`, so a host built without
# `rendering` resolves a different variant than the project does - and Windows
# keeps one module of a given name per process, so the project DLL loads
# against the host's copy and fails with "The specified procedure could not be
# found" (os error 127), naming nothing. The sibling hot-patch suites pass the
# same posture. See `test_shared_component_identity.py` for the same note.
HOST_POSTURE = HOST_DEV_HOT_PATCH_RENDERING

# The shared log-line grammar owns the join between a message and its fields.
BUILD_STARTED_MESSAGE_TOKEN = "building extension"
BUILD_STARTED = log_layout.message_then_fields(
    BUILD_STARTED_MESSAGE_TOKEN, f'module="{MODULE_NAME}"'
)
CANCELLED_TOKEN = "sources changed again during compilation"

# The fast path's success line. A second save that lands after the build
# finished is a body-only edit, so the host patches it instead of rebuilding -
# the save was never in the window, and no build is coming to wait for.
PATCH_INSTALLED_TOKEN = "patch generation installed"

# How long after the first save to make the second, at most. Must land inside the
# build window: long enough that the build has started, short enough that it has
# not finished. A module build here takes roughly a second, and the retry below
# places its save from the window it measures.
SECOND_EDIT_DELAY = 0.8

# The share of a measured build window a retry places its second save at. The
# watcher debounces a save for 60 ms before it reports it, so a save at the very
# edge of the window is only seen once the build has already finished.
SECOND_EDIT_WINDOW_SHARE = 0.25

# A save closer to the build's start than this cannot be observed inside the
# window at all: the debounce alone is 60 ms and the build polls for
# cancellation every 10 ms.
MIN_SECOND_EDIT_DELAY = 0.15

# How many times to re-save with an earlier delay when the second save misses the
# window. Every attempt starts with a structural save, so every attempt starts a
# build of its own - and every module reload costs a project rebuild too, so the
# budget stays at two.
MAX_ATTEMPTS = 2

# How long the host must be building nothing before an attempt starts. A save
# made while another build is in flight reloads late - both builds share cargo's
# lock - and the second save then falls outside the window this suite measures.
QUIET_SECONDS = 1.5

# Every line the host logs while a build is running. The project logs several
# lines a frame, so the scenario cannot settle on "the log went silent": it
# settles on "nothing is building".
BUILD_ACTIVITY_PATTERN = re.compile(
    r"Building of |build watchdog armed|build watchdog cancelling"
    r"|building extension|building project module|building renderer module"
    r"|hot reload complete|build failed"
)

# The last line the startup batch logs: once the shader watchers are armed,
# every module is built, loaded and watched. Both trees log the same line -
# the renderer data crate's, then the project's - and either one means the
# startup batch is over.
STARTUP_COMPLETE_TOKEN = "watching shader sources for edits"

# How long to wait for the rebuild that must follow the cancellation.
REBUILD_TIMEOUT = 90
BUILD_TIMEOUT_SECONDS = 600

# A structural edit forces a real rebuild rather than a live patch, which is the
# path under test. The marker is a bare comment so it cannot change behaviour.
STRUCTURAL_MARKER = "// EDIT-DURING-BUILD PROBE"
ANCHOR = "cubic_term) * 0.5"

# The probe constant as it appears in the source, rewritten on every save so no
# attempt reuses the value an earlier one left on disk.
CONSTANT_PATTERN = re.compile(r"cubic_term\) \* [0-9.]+")


def count_occurrences(output: str, token: str) -> int:
    """How many times a token appears, for before/after comparisons."""
    return output.count(token)


def probe_value(serial: int) -> float:
    """The probe constant's value for one save, unique across the run."""
    return 0.5 + serial / 10000.0


def set_probe_constant(text: str, serial: int) -> str:
    """Returns `text` with the probe's constant set to this save's value."""
    return CONSTANT_PATTERN.sub(f"cubic_term) * {probe_value(serial):.4f}", text, count=1)


def structural_edit(text: str, serial: int) -> str:
    """Returns a structural edit of `text`: one marker line and a new constant.

    The marker line makes the file structurally different from the generation
    the host has loaded, so the save cannot be served by the fast patch path.
    """
    marked = text.replace(
        "// Free Functions", f"// Free Functions\n{STRUCTURAL_MARKER} {serial}", 1
    )
    return set_probe_constant(marked, serial)


def save_during_build(
    monitor: OutputMonitor, text: str, serial: int, delay: float
) -> Optional[Tuple[str, bool, Optional[float]]]:
    """Saves structurally, then again `delay` later, inside the build.

    Returns the source text as left on disk, whether the host cancelled the
    in-flight build for the second save, and how long that build took when it
    was seen through. None means the scenario cannot go on: the first save
    started no build, the second was answered by a patch, or the host exited.
    """
    start_index = monitor.line_count
    atomic_write(MODULE_LIB_RS, text)
    print(f"  [EDIT 1] structural change to {MODULE_LIB_RS.name}; a rebuild must start")
    if not monitor.wait_for(BUILD_STARTED, REBUILD_TIMEOUT, start_index):
        print("  [FAIL] The first save did not start a module build.")
        return None
    started = time.time()
    builds_before = count_occurrences(monitor.output_since(start_index), BUILD_STARTED)

    # The second save, inside the build window. This is the save the host must
    # not lose.
    time.sleep(delay)
    serial += 1
    second = set_probe_constant(text, serial)
    atomic_write(MODULE_LIB_RS, second)
    print(f"  [EDIT 2] second save {delay:.2f}s into the build")

    window: Optional[float] = None
    deadline = time.time() + REBUILD_TIMEOUT
    while time.time() < deadline:
        output = monitor.output_since(start_index)
        if window is None and MODULE_RELOAD_COMPLETE_TOKEN in output:
            window = time.time() - started
        cancelled = CANCELLED_TOKEN in output
        rebuilt = count_occurrences(output, BUILD_STARTED) > builds_before
        if cancelled and rebuilt:
            return second, True, window
        if rebuilt:
            print("  [MISS] The build finished before the second save was seen.")
            return second, False, window
        if PATCH_INSTALLED_TOKEN in output:
            print("  [MISS] The second save was patched, so it never entered a build.")
            return second, False, window
        if not monitor.process_alive():
            print("  [FAIL] The host exited during the scenario.")
            return None
        # Poll for the scenario's outcome lines.
        time.sleep(0.05)

    output = monitor.output_since(start_index)
    print(f"  [FAIL] Nothing was built within {REBUILD_TIMEOUT}s of the second save.")
    print(f"         module builds seen: {count_occurrences(output, BUILD_STARTED)} "
          f"(expected more than {builds_before})")
    return None


def wait_until_idle(monitor: OutputMonitor, quiet: float, timeout: float) -> bool:
    """Waits until the host has been building nothing for `quiet` seconds.

    The scenario needs an idle host to begin: a save that lands while another
    build is still in flight reloads late, because both builds share cargo's
    lock, and the second save then falls outside the window this suite exists
    to test. Returns False when the host never settles or has exited.
    """
    deadline = time.time() + timeout
    last_count = len(BUILD_ACTIVITY_PATTERN.findall(monitor.output_since(0)))
    settled_at = time.time()
    while time.time() < deadline:
        if not monitor.process_alive():
            return False
        count = len(BUILD_ACTIVITY_PATTERN.findall(monitor.output_since(0)))
        if count != last_count:
            last_count = count
            settled_at = time.time()
        elif time.time() - settled_at >= quiet:
            return True
        # Poll for the build activity to stop.
        time.sleep(0.1)
    return False


def run_edit_during_build_scenario(
    monitor: OutputMonitor, backups: BackupRegistry, delay: float
) -> bool:
    """Save twice inside a build window, and require the host to cancel for it."""
    backups.capture(MODULE_LIB_RS)
    original = read_source(MODULE_LIB_RS)
    if original.count(ANCHOR) != 1:
        print(f"  [FAIL] Anchor {ANCHOR!r} is not uniquely present in {MODULE_LIB_RS.name}.")
        return False

    serial = 0
    text = original
    attempt_delay = delay

    print("  [WAIT] Waiting for the startup batch to finish...")
    if not monitor.wait_for(STARTUP_COMPLETE_TOKEN, REBUILD_TIMEOUT):
        print("  [FAIL] The startup batch never finished.")
        return False

    for attempt in range(1, MAX_ATTEMPTS + 1):
        if not wait_until_idle(monitor, QUIET_SECONDS, REBUILD_TIMEOUT):
            print("  [FAIL] The host never settled between attempts.")
            return False
        print(f"  [ATTEMPT {attempt}] second save at {attempt_delay:.2f}s")
        serial += 1
        text = structural_edit(text, serial)
        outcome = save_during_build(monitor, text, serial, attempt_delay)
        if outcome is None:
            return False
        text, cancelled, window = outcome
        if cancelled:
            print("  [OK] The host cancelled the in-flight build for the second save "
                  "and then built the newer sources.")
            return True
        if attempt < MAX_ATTEMPTS:
            if window is None:
                attempt_delay = max(MIN_SECOND_EDIT_DELAY, attempt_delay / 2)
                print(f"         Retrying earlier, at {attempt_delay:.2f}s.")
            else:
                attempt_delay = max(
                    MIN_SECOND_EDIT_DELAY, window * SECOND_EDIT_WINDOW_SHARE
                )
                print(f"         The build took {window:.2f}s; retrying at {attempt_delay:.2f}s.")

    print(f"  [FAIL] The second save never cancelled a build in {MAX_ATTEMPTS} attempts.")
    print("         The cancellation is what this suite exists for: without it the")
    print("         host can adopt code the developer has already replaced.")
    return False


def main() -> None:
    """Launch the host, run the scenario, restore the source."""
    parser = argparse.ArgumentParser(
        description="Assert a save made during a rebuild is not lost"
    )
    parser.add_argument("--second-edit-delay", type=float, default=SECOND_EDIT_DELAY,
                        help="Upper bound on the wait before the second save")
    add_common_suite_arguments(parser)
    args = parser.parse_args()

    global REBUILD_TIMEOUT
    REBUILD_TIMEOUT = int(REBUILD_TIMEOUT * args.timeout_scale)
    startup_timeout = int(STARTUP_TIMEOUT * args.timeout_scale)

    print("=" * 70)
    print("  Edit-During-Build Regression Test")
    print(f"  Workspace: {WORKSPACE_ROOT}")
    print(f"  Second save at most: {args.second_edit_delay}s into the build")
    print("=" * 70)

    ensure_host_lock()
    kill_stale_hosts()
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
        passed = run_edit_during_build_scenario(monitor, backups, args.second_edit_delay)
    finally:
        print("\n  [CLEANUP] Restoring source and stopping the host...")
        # The restored source is the ONLY restore of this edit, and the host is
        # killed immediately after - so the artifact on disk is still the second
        # edit's build, not the original. Rewinding the mtime here would make the
        # next host start trust that stale artifact as up to date; leave the
        # file stamped "now" so the next build regenerates the original.
        BACKUP.restore_all(reset_mtime=False)
        terminate_process(process, monitor)
        print("  [OK] Restored.")

    print("\n" + "=" * 70)
    print("  TEST PASSED" if passed else "  TEST FAILED")
    print("=" * 70)
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    run_suite_with_timing(main)
