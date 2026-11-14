#!/usr/bin/env python3
"""
A manager's settings asset is created, applied and re-applied live.

PROTECTS: A project with no settings asset gets one written from the manager's own document defaults, an edit reaches the running manager with no reload, and a refused document changes nothing.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)

DESCRIPTION
    A manager is a resource that owns a subsystem. It declares a settings
    asset - a standalone asset type whose document is its own tunable shape -
    and the development host keeps `res/settings/<settings_name>.<extension>` in
    step with it: created from the document defaults when missing, applied after
    the project loads, and re-applied when the file's bytes change.

    The test project carries the subject for this
    (`devops/tests/projects/native/src/settings_probe.rs`): a manager with three
    knobs whose applier prints what it was asked to apply. One host run covers
    the four behaviours:

      1. created - the project starts with no `res/settings` directory. The host
         logs `created the manager settings asset` and writes an asset file
         holding the manager's whole default document, and the manager reports
         the defaults.
      2. live - editing a value in that file re-applies it with
         `changed=true`, the manager reports the new value, and no project
         rebuild happens: the run's reload line never follows the edit.
      3. kept across a reload - a source edit reloads the project, and the
         regenerated manager takes the document again, so the edited value is
         still in effect afterwards.
      4. refused - a document with a value the manager cannot use is refused by
         the manager and by the host, and the manager keeps what it had.

    The `res/settings` directory the suite caused is removed again, so the
    test project is left byte-identical.

USAGE
  python devops/tests/test_manager_settings.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_manager_settings.py
  python devops/tests/test_manager_settings.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import shutil
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
RES_DIRECTORY = PROJECT_ROOT / "res"
SETTINGS_DIRECTORY = RES_DIRECTORY / "settings"
SETTINGS_FILE = SETTINGS_DIRECTORY / "fixture_manager.fixture_settings"

# The declared default document, as the test project's document spells it in YAML.
DEFAULT_ENABLED = "enabled: true"
DEFAULT_GAIN = "gain: 1.0"

# The edit scenario 2 writes and scenario 3 expects to survive a reload.
EDITED_GAIN = "gain: 4.5"

# The value scenario 4 writes: a gain no sound could use.
REFUSED_GAIN = "gain: -1.0"

# An unrelated project source, edited to force a reload in scenario 3.
RELOAD_SOURCE = SOURCE_DIRECTORY / "simulation_time.rs"
RELOAD_EDIT = "\n// manager-settings suite: an edit that only reloads the project\n"

# Host and test project log lines this suite asserts on.
CREATED_TOKEN = "[settings] created the manager settings asset"
APPLIED_TOKEN = "[settings] applied manager settings"
REFUSED_TOKEN = "[settings] refused manager settings"
MANAGER_APPLIED_TOKEN = "[test] fixture manager applied"
MANAGER_REFUSED_TOKEN = "[test] fixture manager refused"
RELOAD_STARTED_TOKEN = "building project module"
RELOAD_COMPLETE_TOKEN = "hot reload complete"

# The host builds the project DLL on its first run; a cold one is slow.
STARTUP_TIMEOUT_SECONDS = 240
# A settings apply is milliseconds; this is slack for the watcher's debounce.
SETTINGS_TIMEOUT_SECONDS = 60
# One project rebuild and reload is seconds once the host is up.
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


def check_created(text: str, file_text: str) -> list:
    """One `(ok, description)` pair per assertion about the created file."""
    # Another loaded module may own a manager of its own (the audio device
    # does, on a machine with a sound card), so the check looks for this
    # manager's line rather than the first one.
    created = [
        line for line in lines_with(text, CREATED_TOKEN) if "fixture_manager" in line
    ]
    return [
        (
            bool(created),
            "the host reported creating the fixture manager's settings asset",
        ),
        (
            DEFAULT_ENABLED in file_text and DEFAULT_GAIN in file_text,
            "the created file spells out every default",
        ),
        (
            "asset_type" in file_text and "format_version" in file_text,
            "the created file is a settings asset, header included",
        ),
        (
            bool(lines_with(text, MANAGER_APPLIED_TOKEN)),
            "the manager applied its defaults",
        ),
    ]


def check_live_edit(text: str) -> list:
    """One `(ok, description)` pair per assertion about the live edit."""
    applied = [
        line for line in lines_with(text, APPLIED_TOKEN) if "changed=true" in line
    ]
    manager_applied = [
        line for line in lines_with(text, MANAGER_APPLIED_TOKEN) if "gain=4.5" in line
    ]
    return [
        (
            bool(applied),
            "the host applied the edited document as a change",
        ),
        (
            bool(manager_applied),
            "the manager took the edited value (gain=4.5)",
        ),
        (
            not lines_with(text, RELOAD_STARTED_TOKEN),
            "no project rebuild started for a settings edit",
        ),
    ]


def main() -> int:
    """Runs the host, drives the four scenarios, and restores the test project."""
    parser = argparse.ArgumentParser(
        description="Check that a manager's settings file is created and applied."
    )
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()
    scale = arguments.timeout_scale

    print("\n  Manager settings suite")
    print("  Project: {}".format(PROJECT_ROOT))

    # Step 1: The scenario assumes the project has no settings asset. If an
    # earlier run left one behind, it is removed first so the "created" path is
    # the one under test. The directory is only removed again when this run made
    # it, so nothing the project tracks is touched.
    if SETTINGS_DIRECTORY.exists():
        shutil.rmtree(SETTINGS_DIRECTORY)
    created_settings_directory = not SETTINGS_DIRECTORY.exists()
    source_bytes = RELOAD_SOURCE.read_bytes()

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
        if not monitor.wait_for(MANAGER_APPLIED_TOKEN, STARTUP_TIMEOUT_SECONDS * scale):
            print("  [FAIL] the fixture manager never applied anything; last output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        if not SETTINGS_FILE.is_file():
            print("  [FAIL] the host created no {}".format(SETTINGS_FILE.name))
            return 1
        failures += report(
            check_created(
                monitor.output_since(0),
                SETTINGS_FILE.read_text(encoding="utf-8"),
            )
        )

        # Step 2: Edit the document while the host runs. The manager takes the
        # new value with no reload, which is what the theme pipeline's change
        # detection does for a theme file.
        start = monitor.line_count
        edited_text = SETTINGS_FILE.read_text(encoding="utf-8").replace(
            DEFAULT_GAIN, EDITED_GAIN
        )
        SETTINGS_FILE.write_text(edited_text, encoding="utf-8", newline="")
        if not monitor.wait_for(APPLIED_TOKEN, SETTINGS_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the edited file was never applied; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        if not monitor.wait_for(MANAGER_APPLIED_TOKEN, SETTINGS_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the manager reported no apply after the edit; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        # A little slack so a reload queued by the edit would have shown up by
        # now: the watcher has already delivered the edit, and a rebuild would
        # have logged its first line.
        monitor.wait_for(RELOAD_STARTED_TOKEN, 2.0 * scale, start)
        failures += report(check_live_edit(monitor.output_since(start)))

        # Step 3: A source edit reloads the project. The regenerated manager
        # takes the document again, so the edited value survives.
        start = monitor.line_count
        RELOAD_SOURCE.write_bytes(source_bytes + RELOAD_EDIT.encode("utf-8"))
        if not monitor.wait_for(RELOAD_STARTED_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the source edit started no reload; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        if not monitor.wait_for(RELOAD_COMPLETE_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the reload never completed; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        failures += report(
            [
                (
                    bool(
                        [
                            line
                            for line in lines_with(
                                monitor.output_since(start), MANAGER_APPLIED_TOKEN
                            )
                            if "gain=4.5" in line
                        ]
                    ),
                    "the manager kept the edited value across the reload (gain=4.5)",
                )
            ]
        )

        # Step 4: A document the manager cannot use is refused, and the manager
        # keeps what it had.
        start = monitor.line_count
        SETTINGS_FILE.write_text(
            SETTINGS_FILE.read_text(encoding="utf-8").replace(
                EDITED_GAIN, REFUSED_GAIN
            ),
            encoding="utf-8",
            newline="",
        )
        if not monitor.wait_for(MANAGER_REFUSED_TOKEN, SETTINGS_TIMEOUT_SECONDS * scale, start):
            print("  [FAIL] the out-of-range document was not refused; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        failures += report(
            [
                (
                    bool(lines_with(monitor.output_since(start), REFUSED_TOKEN)),
                    "the host reported the refusal as well",
                ),
                (
                    not [
                        line
                        for line in lines_with(
                            monitor.output_since(start), MANAGER_APPLIED_TOKEN
                        )
                        if "gain=-1" in line
                    ],
                    "the refused document was not applied",
                ),
            ]
        )
    finally:
        RELOAD_SOURCE.write_bytes(source_bytes)
        if created_settings_directory and SETTINGS_DIRECTORY.exists():
            shutil.rmtree(SETTINGS_DIRECTORY)
        if process is not None and monitor is not None:
            print("\n  [CLEANUP] Stopping the host...")
            terminate_process(process, monitor)

    if failures:
        print("\n  [FAIL] {} check(s) failed".format(failures))
        return 1
    print("\n  [OK] the manager's settings were created, applied live and kept")
    return 0


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
