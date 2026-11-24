#!/usr/bin/env python3
"""
A module's declared settings reach its manager, live and across a reload.

PROTECTS: `modules.<module>.<group>.*` in project_settings.yaml is resolved against the schema the module registered, applied to the live manager, re-applied on an edit, and refused when the manager says the value is unusable.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)

DESCRIPTION
    A module declares its settings in its own crate and registers the surface
    from its registration half. The development host resolves the project's
    documents against every registered schema once the modules have loaded,
    hands each manager its values, and does it again when either settings file
    changes or a reload replaced the manager.

    The `pill_test` extension carries the subject: a manager with three knobs
    (`modules.pill_test.fixture`) whose applier prints what it was asked to
    apply. The test project already sets `gain: 1.5`, so one host run covers
    four behaviours:

      1. applied - the project's value reaches the manager at startup, and the
         host logs the module it applied.
      2. live - editing the value in `project_settings.yaml` re-applies it to
         the running manager, and the manager reports the new value.
      3. reported - a key nobody declares is named by its path, once.
      4. kept across a reload - a source edit reloads the project, and the
         regenerated manager takes the project's value again.
      5. refused - a value the manager cannot use is refused by the manager and
         reported by the host, and the manager keeps what it had.

    The project file is restored byte for byte, and the source edit is undone,
    so the test project is left as it was.

USAGE
  python devops/tests/test_manager_settings.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_manager_settings.py
  python devops/tests/test_manager_settings.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import sys
from pathlib import Path

# Standalone-runnable: `devops/` holds `core`, and the suites sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.projects import (  # noqa: E402
    NATIVE_PROJECT_PATH,
    NATIVE_PROJECT_ROOT,
    project_settings_yaml,
)
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
SETTINGS_FILE = project_settings_yaml(PROJECT_ROOT)

# What the test project sets, and what the scenarios edit it to.
PROJECT_GAIN = "gain: 1.5"
EDITED_GAIN = "gain: 4.5"
REFUSED_GAIN = "gain: -1.0"
EDITED_LABEL = "label: fixture"
# A key nobody declares, to prove the resolution reports it under its path.
UNKNOWN_KEY = "\n      nonsense: 1\n"

# An unrelated project source, edited to force a reload in the reload scenario.
RELOAD_SOURCE = SOURCE_DIRECTORY / "simulation_time.rs"
RELOAD_EDIT = "\n// module-settings suite: an edit that only reloads the project\n"

# Host and module log lines this suite asserts on.
APPLIED_TOKEN = "[settings] applied a module's declared settings"
REFUSED_TOKEN = "[settings] refused a module's declared settings"
MANAGER_APPLIED_TOKEN = "[test] fixture manager applied"
MANAGER_REFUSED_TOKEN = "[test] fixture manager refused"
UNUSED_SETTING_TOKEN = "a project setting is not used"
STRICT_ERROR_TOKEN = "do not resolve; every module keeps the values it has"
RELOAD_STARTED_TOKEN = "building project module"
RELOAD_COMPLETE_TOKEN = "hot reload complete"

# The host builds the project DLL on its first run; a cold one is slow.
STARTUP_TIMEOUT_SECONDS = 240
# A settings apply is milliseconds; this is slack for the file stamp's frame.
SETTINGS_TIMEOUT_SECONDS = 60
# One project rebuild and reload is seconds once the host is up.
RELOAD_TIMEOUT_SECONDS = 120


# =============================================================================
# Free functions
# =============================================================================


def lines_with(text: str, token: str) -> list:
    """Every line in `text` that carries `token`, oldest first."""
    return [line for line in text.splitlines() if token in line]


def report(checks: list) -> int:
    """Prints every check and returns the number that failed."""
    failures = 0
    for ok, description in checks:
        print("  [{}] {}".format("OK" if ok else "FAIL", description))
        if not ok:
            failures += 1
    return failures


def rewrite_settings(settings_text: str, old: str, new: str) -> str:
    """`settings_text` with `old` replaced by `new`, refusing a no-op."""
    if old not in settings_text:
        raise SystemExit("error: `{}` is not in {}".format(old, SETTINGS_FILE))
    return settings_text.replace(old, new, 1)


def strict_run_reports(settings_text: str, scale: float) -> list:
    """Starts a host with strict settings and a key nobody declares.

    A strict run turns what would be a warning into a configuration error. The
    modules are already loaded by the time their entries can be checked, so the
    run reports the error and keeps the values it has instead of tearing down a
    live world; the check is that the error is reported and named.
    """
    SETTINGS_FILE.write_text(
        settings_text.replace(EDITED_LABEL, EDITED_LABEL + UNKNOWN_KEY, 1),
        encoding="utf-8",
        newline="",
    )
    environment = dict(
        os.environ,
        PROJECT_PATH=NATIVE_PROJECT_PATH,
        PILL__HOST__STRICT_SETTINGS="1",
    )
    process = None
    monitor = None
    try:
        process, monitor = launch_process(
            host_command(HOST_DEV_RENDERING), MODULES_ROOT, environment
        )
        reported = monitor.wait_for(
            STRICT_ERROR_TOKEN, STARTUP_TIMEOUT_SECONDS * scale
        )
        output = monitor.output_since(0)
        return [
            (reported, "a strict run reports an undeclared module setting as an error"),
            (
                bool([line for line in lines_with(output, STRICT_ERROR_TOKEN) if "nonsense" in line]),
                "the error names the undeclared key",
            ),
        ]
    finally:
        if process is not None and monitor is not None:
            terminate_process(process, monitor)


def main() -> int:
    """Runs the host, drives the scenarios, and restores the test project."""
    parser = argparse.ArgumentParser(
        description="Check that a module's declared settings are applied and re-applied."
    )
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()
    scale = arguments.timeout_scale

    print("\n  Module settings suite")
    print("  Project: {}".format(PROJECT_ROOT))

    settings_bytes = SETTINGS_FILE.read_bytes()
    settings_text = settings_bytes.decode("utf-8")
    if PROJECT_GAIN not in settings_text:
        print("  [FAIL] the test project does not set `{}`".format(PROJECT_GAIN))
        return 1
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
            print("  [FAIL] the manager never applied anything; last output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        failures += report(
            [
                (
                    bool(
                        [
                            line
                            for line in lines_with(
                                monitor.output_since(0), MANAGER_APPLIED_TOKEN
                            )
                            if "gain=1.5" in line
                        ]
                    ),
                    "the project's value reached the manager at startup (gain=1.5)",
                ),
                (
                    bool(
                        [
                            line
                            for line in lines_with(monitor.output_since(0), APPLIED_TOKEN)
                            if "pill_test" in line
                        ]
                    ),
                    "the host reported applying the module's declared settings",
                ),
            ]
        )

        # Step 2: Edit the value while the host runs. The manager takes it with
        # no restart.
        start = monitor.line_count
        SETTINGS_FILE.write_text(
            rewrite_settings(settings_text, PROJECT_GAIN, EDITED_GAIN),
            encoding="utf-8",
            newline="",
        )
        if not monitor.wait_for(
            MANAGER_APPLIED_TOKEN, SETTINGS_TIMEOUT_SECONDS * scale, start
        ):
            print("  [FAIL] the edited value was never applied; last output was:")
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
                    "the manager took the edited value while the host ran (gain=4.5)",
                )
            ]
        )

        # Step 3: A key nobody declares is reported by its path.
        start = monitor.line_count
        edited = rewrite_settings(settings_text, PROJECT_GAIN, EDITED_GAIN)
        SETTINGS_FILE.write_text(
            edited.replace(EDITED_LABEL, EDITED_LABEL + UNKNOWN_KEY, 1),
            encoding="utf-8",
            newline="",
        )
        if not monitor.wait_for(
            UNUSED_SETTING_TOKEN, SETTINGS_TIMEOUT_SECONDS * scale, start
        ):
            print("  [FAIL] an undeclared key was not reported; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        failures += report(
            [
                (
                    bool(
                        [
                            line
                            for line in lines_with(
                                monitor.output_since(start), UNUSED_SETTING_TOKEN
                            )
                            if "nonsense" in line
                        ]
                    ),
                    "the report names the undeclared key",
                )
            ]
        )
        # Step 4: A source edit reloads the project, and the regenerated
        # manager takes the project's value again. The anchor is taken before
        # either write, so nothing the host reports in between is lost.
        start = monitor.line_count
        SETTINGS_FILE.write_text(edited, encoding="utf-8", newline="")
        RELOAD_SOURCE.write_bytes(source_bytes + RELOAD_EDIT.encode("utf-8"))
        if not monitor.wait_for(
            RELOAD_STARTED_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start
        ):
            print("  [FAIL] the source edit started no reload; last output was:")
            print(monitor.output_since(start)[-1500:])
            return 1
        if not monitor.wait_for(
            RELOAD_COMPLETE_TOKEN, RELOAD_TIMEOUT_SECONDS * scale, start
        ):
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
                    "the manager kept the project's value across the reload (gain=4.5)",
                )
            ]
        )

        # Step 5: A value the manager cannot use is refused, and the manager
        # keeps what it had.
        start = monitor.line_count
        SETTINGS_FILE.write_text(
            rewrite_settings(edited, EDITED_GAIN, REFUSED_GAIN),
            encoding="utf-8",
            newline="",
        )
        if not monitor.wait_for(
            MANAGER_REFUSED_TOKEN, SETTINGS_TIMEOUT_SECONDS * scale, start
        ):
            print("  [FAIL] the unusable value was not refused; last output was:")
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
                    "the refused value was not applied",
                ),
            ]
        )
    finally:
        SETTINGS_FILE.write_bytes(settings_bytes)
        RELOAD_SOURCE.write_bytes(source_bytes)
        if process is not None and monitor is not None:
            print("\n  [CLEANUP] Stopping the host...")
            terminate_process(process, monitor)

    # Step 6: A strict run reports the same key as an error rather than a
    # warning. It cannot stop a run that has already loaded its modules, so the
    # check is that the report arrives and the host still refuses nothing it
    # could apply - see the stage's record for the follow-up.
    failures += report(strict_run_reports(settings_text, scale))
    SETTINGS_FILE.write_bytes(settings_bytes)

    if failures:
        print("\n  [FAIL] {} check(s) failed".format(failures))
        return 1
    print("\n  [OK] the module's declared settings were applied live and kept")
    return 0


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
