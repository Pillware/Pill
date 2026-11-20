#!/usr/bin/env python3
"""
A resource that declares its layout keeps or rebuilds its value across a reload.

PROTECTS: A declared resource survives an unrelated edit, and is dropped through the retiring image and rebuilt when the file its layout lives in changes.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)

DESCRIPTION
    A resource is not cleared on reload: the value stays and only its function
    table is re-pointed at the new image. That is sound while the two images
    agree on the type's layout, and silently wrong once they do not - the bytes
    keep their size and mean something else. A resource can therefore declare
    the files and crates its layout is made of (`Resource::layout_sources`,
    `Resource::layout_dependencies`), the host fingerprints them into a per-module
    record, and a reload drops the value whose fingerprint changed so the module
    rebuilds it.

    The test project carries the witness for exactly this
    (`devops/tests/projects/native/src/layout_witness.rs`): a resource holding a
    heap payload, which prints one line when it is inserted and one when it is
    dropped, and whose declaration names its own file as its only source. The
    suite drives one headless-or-windowed host through three edits:

      1. an edit to a file the witness does not declare (`simulation_time.rs`)
         reloads the project and keeps the live value - no drop line at all;
      2. an edit to the declared file (the witness's build value) prints
         `resource layout changed`, naming the resource and that source, then
         drops the old value (`payload=layout-witness-1`) and inserts the
         rebuilt one (`build_value=2`);
      3. a further edit to an undeclared file keeps the *rebuilt* value, which
         is what proves the record was rewritten after the rebuild: a stale
         record would drop it a second time on every later reload.

    Both edited files are restored byte for byte whatever happens, and the host
    is terminated on every path.

USAGE
  python devops/tests/test_resource_layout_fingerprint.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_resource_layout_fingerprint.py
  python devops/tests/test_resource_layout_fingerprint.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import sys
from pathlib import Path

# Standalone-runnable: `devops/` holds `core`, and the suites sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.projects import NATIVE_PROJECT_PATH, NATIVE_PROJECT_ROOT  # noqa: E402
from core.suite_common import (  # noqa: E402
    HOST_DEV_RENDERING,
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
SOURCE_DIRECTORY = PROJECT_ROOT / "src"

# The witness's declared source, and the file the suite edits for scenario 2.
WITNESS_FILE = SOURCE_DIRECTORY / "layout_witness.rs"
WITNESS_SOURCE = "src/layout_witness.rs"
ORIGINAL_BUILD_VALUE = "const WITNESS_BUILD_VALUE: u32 = 1;"
EDITED_BUILD_VALUE = "const WITNESS_BUILD_VALUE: u32 = 2;"

# A file the witness does not declare, used for the two "kept" scenarios.
UNDECLARED_FILE = SOURCE_DIRECTORY / "simulation_time.rs"
UNDECLARED_EDIT = "\n// layout-fingerprint suite: an edit the witness does not declare\n"

# Host and test project log lines this suite asserts on.
RELOAD_STARTED_TOKEN = "building project module"
RELOAD_COMPLETE_TOKEN = "hot reload complete"
# The host's own "everything is loaded and watched" line. Every edit below waits
# for it first: the file watcher takes its baseline when it arms, a moment after
# the witness enters the world, so an edit written before that baseline exists
# is never reported as a change and the scenario would wait for a reload that
# cannot come.
HOST_READY_TOKEN = "Entering project loop"
LAYOUT_CHANGED_TOKEN = "resource layout changed"
WITNESS_INSERTED_TOKEN = "[test] layout witness inserted"
WITNESS_DROPPED_TOKEN = "[test] layout witness dropped"

# What the test project prints for the value it was built with, before and after the
# edit that changes the witness's layout.
DROPPED_ORIGINAL_PAYLOAD = "payload=layout-witness-1"
INSERTED_REBUILT_VALUE = "build_value=2"

# The host builds the project DLL on its first run; a cold one is slow.
STARTUP_TIMEOUT_SECONDS = 240
# One project rebuild and reload is seconds once the host is up; this is slack.
RELOAD_TIMEOUT_SECONDS = 120


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


def report(checks: list) -> int:
    """Prints every check and returns the number that failed."""
    failures = 0
    for ok, description in checks:
        print("  [{}] {}".format("OK" if ok else "FAIL", description))
        if not ok:
            failures += 1
    return failures


def check_layout_warning(text: str) -> list:
    """One `(ok, description)` pair per assertion about the changed-layout warning."""
    warning = first_line_with(text, LAYOUT_CHANGED_TOKEN)
    return [
        (
            WITNESS_SOURCE in warning,
            "the warning names the source that changed ({})".format(WITNESS_SOURCE),
        ),
        (
            "layout_witness" in warning,
            "the warning names the resource whose layout changed",
        ),
        (
            "rebuilds it" in warning,
            "the warning says what was done with the live value",
        ),
    ]


def check_rebuilt_value(text: str) -> list:
    """One `(ok, description)` pair per assertion about the rebuild."""
    dropped = lines_with(text, WITNESS_DROPPED_TOKEN)
    inserted = lines_with(text, WITNESS_INSERTED_TOKEN)
    return [
        (
            bool(dropped) and DROPPED_ORIGINAL_PAYLOAD in dropped[0],
            "the retiring image dropped the old value ({})".format(
                DROPPED_ORIGINAL_PAYLOAD
            ),
        ),
        (
            len(inserted) == 2 and INSERTED_REBUILT_VALUE in inserted[-1],
            "the module rebuilt the value with the edited layout ({})".format(
                INSERTED_REBUILT_VALUE
            ),
        ),
    ]


def main() -> int:
    """Runs the host, edits the two files in turn, checks the log and restores."""
    parser = argparse.ArgumentParser(
        description="Check the keep-or-rebuild decision for a declared resource."
    )
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()
    scale = arguments.timeout_scale

    print("\n  Resource layout fingerprint suite")
    print("  Project: {}".format(PROJECT_ROOT))

    # Step 1: Read both files before anything is written, so the `finally`
    # below restores exactly the bytes the run started with.
    witness_bytes = WITNESS_FILE.read_bytes()
    undeclared_bytes = UNDECLARED_FILE.read_bytes()
    witness_text = witness_bytes.decode("utf-8")
    if ORIGINAL_BUILD_VALUE not in witness_text:
        print("  [FAIL] the witness does not hold {}".format(ORIGINAL_BUILD_VALUE))
        return 1
    edited_witness = witness_text.replace(ORIGINAL_BUILD_VALUE, EDITED_BUILD_VALUE)

    ensure_host_lock()
    if not prepare_host(HOST_DEV_RENDERING, arguments):
        return 1

    environment = dict(os.environ, PROJECT_PATH=NATIVE_PROJECT_PATH)
    process = None
    monitor = None
    failures = 0
    try:
        process, monitor = launch_process(
            host_command(HOST_DEV_RENDERING), MODULES_ROOT, environment
        )
        if not monitor.wait_for(WITNESS_INSERTED_TOKEN, STARTUP_TIMEOUT_SECONDS * scale):
            print("  [FAIL] the witness never entered the world; last output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        failures += report(
            [
                (
                    "layout-witness-1" in first_line_with(monitor.output_since(0), WITNESS_INSERTED_TOKEN),
                    "the first load inserted the value the file declares",
                )
            ]
        )

        # Step 2: An edit the witness does not declare. The project reloads and
        # the value has to survive: no drop line may appear at all.
        if not monitor.wait_for(HOST_READY_TOKEN, STARTUP_TIMEOUT_SECONDS * scale):
            print("  [FAIL] the host never entered its project loop; last output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        start = monitor.line_count
        UNDECLARED_FILE.write_bytes(undeclared_bytes + UNDECLARED_EDIT.encode("utf-8"))
        if not monitor.wait_for(RELOAD_STARTED_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the undeclared edit started no reload; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        # Waiting from the same start makes the completion line provably this
        # reload's: nothing after the edit can carry an earlier one.
        if not monitor.wait_for(RELOAD_COMPLETE_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the undeclared edit did not finish reloading; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        failures += report(
            [
                (
                    not lines_with(monitor.output_since(start), WITNESS_DROPPED_TOKEN),
                    "the undeclared edit kept the live value (no drop line)",
                )
            ]
        )

        # Step 3: The declared edit. The host drops the old value through the
        # retiring image, then the module rebuilds it.
        start = monitor.line_count
        WITNESS_FILE.write_bytes(edited_witness.encode("utf-8"))
        if not monitor.wait_for(LAYOUT_CHANGED_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the declared edit printed no layout warning; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        if not monitor.wait_for(WITNESS_DROPPED_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the old value was never dropped; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        monitor.wait_for(WITNESS_INSERTED_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start)
        failures += report(check_layout_warning(monitor.output_since(start)))
        failures += report(check_rebuilt_value(monitor.output_since(0)))

        # Step 4: Another undeclared edit. A record that was rewritten after the
        # rebuild keeps the rebuilt value; a stale one would drop it again.
        start = monitor.line_count
        UNDECLARED_FILE.write_bytes(
            undeclared_bytes + (UNDECLARED_EDIT * 2).encode("utf-8")
        )
        if not monitor.wait_for(RELOAD_STARTED_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the second undeclared edit started no reload; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        if not monitor.wait_for(RELOAD_COMPLETE_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the second undeclared edit did not finish reloading; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        failures += report(
            [
                (
                    not lines_with(monitor.output_since(start), WITNESS_DROPPED_TOKEN),
                    "the rebuilt value survived the next reload (the record was rewritten)",
                )
            ]
        )
    finally:
        WITNESS_FILE.write_bytes(witness_bytes)
        UNDECLARED_FILE.write_bytes(undeclared_bytes)
        if process is not None and monitor is not None:
            print("\n  [CLEANUP] Stopping the host...")
            terminate_process(process, monitor)

    if failures:
        print("\n  [FAIL] {} check(s) failed".format(failures))
        return 1
    print("\n  [OK] the declared resource was kept, rebuilt and kept again")
    return 0


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
