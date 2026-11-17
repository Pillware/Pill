#!/usr/bin/env python3
"""
A project's own editor crate belongs to the editor posture alone.

PROTECTS: `<project>/<package>_editor/` is built and loaded only by an editor
host; a game build of the same project never builds, watches or loads it.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo); the game host is built once and launched once.

DESCRIPTION
    A project may ship an editor crate beside its manifest. Only an editor host
    resolves it (`HostConfig::from_environment_for_editor_frontend`), builds it
    with the other modules and loads it after the project; `project_settings`
    never names it, and `modules:` refuses an `_editor` name.

    This suite starts the game host (`pill_standalone`) on
    `devops/tests/projects/editor_api`, whose editor crate exists, and waits
    for the project's own ready line. It then requires the whole log to carry
    no line naming that crate and no project-editor line at all, so a game
    build that picked it up would fail here rather than ship it.

    The editor posture's half is proved by the editor suite's `editor_api`
    area, which needs a window.

USAGE
  python devops/tests/test_project_editor_crate.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_project_editor_crate.py
  python devops/tests/test_project_editor_crate.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import sys
from pathlib import Path

# Standalone-runnable: `devops/` holds `core`, and the suites sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.projects import REPOSITORY_ROOT  # noqa: E402
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

# The project that ships an editor crate, and the crate's name.
PROJECT_ROOT = REPOSITORY_ROOT / "devops" / "tests" / "projects" / "editor_api"
EDITOR_CRATE_NAME = "editor_api_editor"

# The project's own ready line, printed once its `init` ran. Every absence below
# is only meaningful after it.
PROJECT_READY_TOKEN = "[editor_api] ready"
# Anything naming the crate, in a build plan, a load line, a watcher or a
# dependency list.
EDITOR_CRATE_TOKENS = (
    EDITOR_CRATE_NAME,
    "project editor",
    "editor crate",
)

STARTUP_TIMEOUT_SECONDS = 240


def main() -> int:
    """Launches the game host once and reports every check."""
    parser = argparse.ArgumentParser(
        description="Check that a game build loads no project editor crate."
    )
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()

    print("\n  Project editor crate posture suite")
    print("  Project: {}".format(PROJECT_ROOT))

    ensure_host_lock()
    if not prepare_host(HOST_DEV, arguments):
        return 1

    environment = dict(os.environ)
    environment["PROJECT_PATH"] = str(PROJECT_ROOT)
    process, monitor = launch_process(host_command(HOST_DEV), MODULES_ROOT, environment)
    failures = 0
    try:
        ready = monitor.wait_for(
            PROJECT_READY_TOKEN, STARTUP_TIMEOUT_SECONDS * arguments.timeout_scale
        )
        print(
            "  [{}] the project started".format("OK" if ready else "FAIL")
        )
        if not ready:
            failures += 1
        text = monitor.output_since(0)
        appeared = [token for token in EDITOR_CRATE_TOKENS if token in text]
        ok = not appeared
        print(
            "  [{}] no build, watch or load line named {}".format(
                "OK" if ok else "FAIL", ", ".join(repr(token) for token in EDITOR_CRATE_TOKENS)
            )
        )
        if not ok:
            failures += 1
            for token in appeared:
                print("        matched {!r}".format(token))
    finally:
        terminate_process(process, monitor)

    if failures:
        print("\n  [FAIL] {} check(s) failed".format(failures))
        return 1
    print("\n  [OK] a game build never sees the project's editor crate")
    return 0


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
