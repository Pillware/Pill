#!/usr/bin/env python3
"""
A committed .pipeline runs the frame a windowed build renders.

PROTECTS: The example's .pipeline loads by path and the renderer records its pass in the frame chain with a positive draw count.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo) and a display: the chain is only built with a
    surface, so this drives the windowed development host.

DESCRIPTION
    `devops/tests/projects/native` ships `res/pipelines/main.pipeline`, which names the
    project's own `dwdw.render_pass` by guid, and its `init` imports both and
    hands the chain to `RenderingManager`. A chain is only built when a surface
    exists, so the suite launches the windowed development host on that project
    and reads the renderer's observation line:

        pass chain: <pass label>(<draws>) ...

    The line is printed once per change. The suite requires a line whose only
    entry is the project's pass with a positive draw count: that is the chain
    the file describes actually running, and it cannot come from the built-in
    default chain, whose passes carry the data crate's names.

    Nothing is edited; the host is terminated on every path. The project's DLL
    is rebuilt by the host itself when it is stale.

USAGE
  python devops/tests/test_render_chains.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_render_chains.py
  python devops/tests/test_render_chains.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import re
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

# The pass the committed chain names, as the artist named the file. The
# renderer labels a pass with its asset name, which is the path it was
# imported under.
PASS_LABEL = "dwdw.render_pass"

# A pass of the chain the renderer installs for itself. Seeing one on the same
# line as the project's pass would mean the file's chain never replaced it.
DEFAULT_CHAIN_LABEL = "pill.pbr.pass."

CHAIN_TOKEN = "pass chain:"
ENTRY_PATTERN = re.compile(r"(?P<label>[^\s()]+)\((?P<draws>\d+)\)")

# The host builds the project DLL on its first run; a cold one is slow.
CHAIN_TIMEOUT_SECONDS = 240


# =============================================================================
# Free functions
# =============================================================================


def chain_lines(text: str) -> list:
    """Every `pass chain:` line in `text`, newest last."""
    return [line for line in text.splitlines() if CHAIN_TOKEN in line]


def entries(line: str) -> list:
    """The (label, draws) pairs one chain line lists."""
    return [
        (match.group("label"), int(match.group("draws")))
        for match in ENTRY_PATTERN.finditer(line.split(CHAIN_TOKEN, 1)[1])
    ]


def check(text: str) -> list:
    """Returns one `(ok, description)` pair per assertion over the log."""
    checks = []
    lines = chain_lines(text)
    with_pass = [
        (line, entries(line)) for line in lines
    ]
    with_pass = [
        (line, pairs) for line, pairs in with_pass
        if any(label == PASS_LABEL for label, _ in pairs)
    ]
    checks.append(
        (
            bool(with_pass),
            "the renderer logged a chain naming '{}'".format(PASS_LABEL),
        )
    )
    if not with_pass:
        return checks

    line, pairs = with_pass[-1]
    project_pairs = [pair for pair in pairs if pair[0] == PASS_LABEL]
    checks.append(
        (
            len(project_pairs) == 1,
            "the chain names the project's pass once",
        )
    )
    single = project_pairs[0] if project_pairs else (PASS_LABEL, 0)
    checks.append(
        (
            single[1] > 0,
            "the project's pass was handed draws ({})".format(single[1]),
        )
    )
    checks.append(
        (
            not any(DEFAULT_CHAIN_LABEL in label for label, _ in pairs),
            "the chain is the file's, not the renderer's installed one",
        )
    )
    checks.append(
        (
            len(pairs) == 1,
            "the committed chain holds exactly its one pass",
        )
    )
    return checks


def main() -> int:
    """Builds the windowed host, runs the project, checks the chain line."""
    parser = argparse.ArgumentParser(
        description="Run the committed .pipeline through the windowed host."
    )
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()
    scale = arguments.timeout_scale

    print("\n  Render chain suite")
    print("  Project: {}".format(PROJECT_ROOT))
    ensure_host_lock()
    if not prepare_host(HOST_DEV_RENDERING, arguments):
        return 1

    environment = dict(os.environ, PROJECT_PATH=NATIVE_PROJECT_PATH)
    process, monitor = launch_process(
        host_command(HOST_DEV_RENDERING), MODULES_ROOT, environment
    )
    failures = 0
    try:
        if not monitor.wait_for(CHAIN_TOKEN, CHAIN_TIMEOUT_SECONDS * scale):
            print("  [FAIL] the renderer logged no chain; the last output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        # The chain line is printed once per change and a later reload can log
        # it again; give the frame loop a moment to settle before reading.
        monitor.wait_for(PASS_LABEL, 30 * scale)

        for ok, description in check(monitor.output_since(0)):
            print("  [{}] {}".format("OK" if ok else "FAIL", description))
            if not ok:
                failures += 1
        if failures:
            print("\n  Chain lines seen:")
            for line in chain_lines(monitor.output_since(0)):
                print("    " + line.strip())
    finally:
        print("\n  [CLEANUP] Stopping the windowed host...")
        terminate_process(process, monitor)

    if failures:
        print("\n  [FAIL] {} check(s) failed".format(failures))
        return 1
    print("\n  [OK] the committed chain runs the frame")
    return 0


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
