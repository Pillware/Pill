#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+, PyYAML, the Rust toolchain, Windows (the egui
#               editor suite needs the DirectComposition overlay).

# DESCRIPTION: The egui editor suite in the order it requires, cheapest
#   failure first:
#     1. preflight - `test_editor_egui.py --steps list` imports every step
#        area module and checks the area table (names, orders, needs). A
#        broken import or an order collision fails here in about a second,
#        before any build;
#     2. build     - `cargo build -p pill_editor --offline` from `modules/`,
#        immediately before the suite: any narrower cargo build in between
#        re-splits the shared DLLs and the editor fails to start;
#     3. suite     - `test_editor_egui.py --steps <areas>`, by default only the
#        areas named, so iterating on one area costs about a minute instead of
#        the whole suite's five.
#   Run it inside a long-command wrapper so the run is recorded and holds the
#   `cargo-target` lock that keeps other agents' builds from rewriting the
#   DLLs under it.
#
# USAGE: python devops/tools/run_editor_suite.py --steps <areas|all|failed> [--no-build] [suite args...]
#          --steps     a comma-separated list of step areas, `all` for the
#                      whole suite, or `failed` for the areas the last run
#                      failed in (`--steps list` on the suite prints the names)
#          --no-build  skip the cargo build (the binary is current)
#          any other argument is passed to test_editor_egui.py unchanged

# EXAMPLE USAGE:
#   python devops/tools/run_editor_suite.py --steps play_mode
#   python devops/tools/run_editor_suite.py --steps failed
#   python devops/tools/run_editor_suite.py --steps all

# --- SCRIPT ---

import argparse
import subprocess
import sys
import time
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
MODULES_ROOT = REPOSITORY_ROOT / "modules"
SUITE = REPOSITORY_ROOT / "devops" / "tests" / "test_editor_egui.py"


def run_phase(title: str, command: list, working_directory: Path) -> int:
    """Runs one phase, prints its time, and returns its exit code."""
    print("[run_editor_suite] {}: {}".format(title, " ".join(str(part) for part in command)), flush=True)
    started = time.monotonic()
    result = subprocess.run(command, cwd=working_directory)
    print("[run_editor_suite] {} {} after {:.1f}s".format(
        title, "passed" if result.returncode == 0 else "FAILED (exit {})".format(result.returncode),
        time.monotonic() - started), flush=True)
    return result.returncode


def main() -> int:
    """Preflight, build, then the selected areas; stops at the first failing phase."""
    parser = argparse.ArgumentParser(description="Preflight, build and run the egui editor suite.")
    parser.add_argument("--steps", required=True, help="areas, `all` or `failed`")
    parser.add_argument("--no-build", action="store_true", help="skip the cargo build")
    arguments, suite_arguments = parser.parse_known_args()

    # The preflight imports every area module; it needs no build and no editor.
    if run_phase("preflight", [sys.executable, str(SUITE), "--steps", "list"], REPOSITORY_ROOT) != 0:
        print("[run_editor_suite] the suite's own code is broken; fix it before building", flush=True)
        return 1
    if not arguments.no_build:
        if run_phase("build", ["cargo", "build", "-p", "pill_editor", "--offline"], MODULES_ROOT) != 0:
            return 1
    return run_phase("suite", [sys.executable, str(SUITE), "--steps", arguments.steps] + suite_arguments,
                     REPOSITORY_ROOT)


if __name__ == "__main__":
    sys.exit(main())
