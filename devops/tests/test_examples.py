#!/usr/bin/env python3
"""
Build every example project with the release profile and report artifact sizes.

PROTECTS: Every example project builds in the release profile.
TIER: build

REQUIREMENTS: Python 3.8+, Rust toolchain (cargo) on PATH, .NET SDK for any
              C# example (a missing SDK skips those rather than failing).

DESCRIPTION
    Builds each example in the release profile, reports a per-file artifact size
    breakdown, and finishes with a pass/fail/skip summary whose exit code is
    non-zero when anything failed.

    DISCOVERY, NOT A HARDCODED LIST. The shell version enumerated eight
    example paths (`examples/cube`, `examples/city`, ...) that no longer exist
    in this repository, so it had nothing left to build. This script finds
    examples by convention instead - any directory carrying a build manifest
    under one of the two example roots - so adding an example needs no edit
    here:

      * `Cargo.toml`  -> built with `cargo build --release`
      * `*.csproj`    -> built with `dotnet build -c Release`

    The two roots have different shapes. `examples/local/` is a flat checkout of
    one directory per project; `examples/<group>/` holds the official examples,
    one Rust project and one C# project per group. Both are walked one level
    below the root they belong to, so a group folder is never mistaken for a
    project.

    A subdirectory with neither manifest is skipped with that reason, not
    silently ignored.

USAGE
  python devops/tests/test_examples.py [all | <example-path>] [--list]

EXAMPLE USAGE
  python devops/tests/test_examples.py
  python devops/tests/test_examples.py devops/tests/projects/native
  python devops/tests/test_examples.py --list

  Exit status: 0 when every example built, 1 when any failed, 2 on a usage
  error.

--- SCRIPT ---
"""

import argparse
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import List, Optional, Tuple

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.paths import REPOSITORY_ROOT, find_executable  # noqa: E402
from core.projects import EXAMPLES_ROOT, example_project_roots  # noqa: E402
from core.suite_common import run_suite_with_timing  # noqa: E402
from core.test_report import (  # noqa: E402
    ANSI_BOLD,
    ANSI_CYAN,
    ResultTally,
    colorize,
    print_size_report,
    section,
)

# A release-profile build of a cold example pulls in the whole engine graph.
BUILD_TIMEOUT_SECONDS = 3600


class Example:
    """One discovered example project and how to build it."""

    def __init__(self, path: Path, kind: str, manifest: Path) -> None:
        self.path = path
        self.kind = kind  # "rust" | "csharp"
        self.manifest = manifest

    @property
    def display(self) -> str:
        """The example's repository-relative path, for reports."""
        return self.path.relative_to(REPOSITORY_ROOT).as_posix()

    @property
    def artifact_directory(self) -> Path:
        """Where the release-profile build drops its artifacts."""
        if self.kind == "rust":
            return self.path / "target" / "release"
        return self.path / "bin" / "Release"

    @property
    def artifacts_are_recursive(self) -> bool:
        """Whether the artifact directory should be walked recursively.

        Cargo puts the shipped binary at the top of `target/release/` and its
        entire build cache in `deps/`, `build/` and `incremental/` below it,
        so a recursive walk would report hundreds of megabytes of
        intermediates as the artifact size. The .NET layout nests the output
        under a framework directory, so it does need the walk.
        """
        return self.kind != "rust"


# =============================================================================
# Discovery
# =============================================================================


def discover_examples() -> List[Tuple[Path, Optional[Example]]]:
    """Finds every example project under the configured example roots.

    Returns `(directory, Example or None)` pairs so a directory without a
    recognised manifest can be reported as skipped rather than dropped.
    """
    discovered: List[Tuple[Path, Optional[Example]]] = []
    for directory in example_project_roots():
        discovered.append((directory, classify_example(directory)))
    return discovered


def classify_example(directory: Path) -> Optional[Example]:
    """Identifies an example's build system from the manifest it carries."""
    cargo_manifest = directory / "Cargo.toml"
    if cargo_manifest.is_file():
        return Example(directory, "rust", cargo_manifest)
    project_files = sorted(directory.glob("*.csproj"))
    if project_files:
        return Example(directory, "csharp", project_files[0])
    return None


# =============================================================================
# Build helpers
# =============================================================================


def build_rust_example(example: Example, tally: ResultTally) -> None:
    """Builds a Cargo example in the release profile and reports its artifact sizes."""
    command = [
        find_executable("cargo"),
        "build",
        "--release",
        "--manifest-path",
        str(example.manifest),
    ]
    if not run_build(command, example, tally):
        return
    tally.report_pass(f"{example.display} build")
    print_size_report(example.artifact_directory, example.artifacts_are_recursive)
    tally.report_pass(f"{example.display} artifact size report")


def build_csharp_example(example: Example, tally: ResultTally) -> None:
    """Builds a .NET example in the `Release` configuration, skipping when no SDK is present."""
    if shutil.which("dotnet") is None:
        tally.report_skip(example.display, "dotnet SDK not found on PATH")
        return
    command = [
        find_executable("dotnet"),
        "build",
        str(example.manifest),
        "-c",
        "Release",
    ]
    if not run_build(command, example, tally):
        return
    tally.report_pass(f"{example.display} build")
    print_size_report(example.artifact_directory, example.artifacts_are_recursive)
    tally.report_pass(f"{example.display} artifact size report")


def run_build(command: List[str], example: Example, tally: ResultTally) -> bool:
    """Runs one build command, reporting a failure against the example.

    Output is inherited rather than captured: a release-profile build runs for
    minutes, and a silent terminal looks like a hang.
    """
    print(f"  $ {' '.join(command)}")
    started = time.monotonic()
    try:
        completed = subprocess.run(command, cwd=str(REPOSITORY_ROOT), timeout=BUILD_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        tally.report_fail(example.display, f"build timed out after {BUILD_TIMEOUT_SECONDS}s")
        return False
    except OSError as error:
        tally.report_fail(example.display, f"could not start the build: {error}")
        return False
    duration = time.monotonic() - started
    if completed.returncode != 0:
        tally.report_fail(
            example.display, f"build failed (exit {completed.returncode})"
        )
        return False
    print(f"  built in {duration:.1f}s")
    return True


def build_example(example: Example, tally: ResultTally) -> None:
    """Dispatches to the builder for the example's kind."""
    if example.kind == "rust":
        build_rust_example(example, tally)
    else:
        build_csharp_example(example, tally)


# =============================================================================
# Entry points
# =============================================================================


def build_all_examples(tally: ResultTally) -> None:
    """Builds every discovered example, numbering the progress lines."""
    discovered = discover_examples()
    section("Building all example projects (release)")
    if not discovered:
        print(f"No example directories found under {EXAMPLES_ROOT} or the official catalog")
        return

    for index, (directory, example) in enumerate(discovered, start=1):
        display = directory.relative_to(REPOSITORY_ROOT).as_posix()
        print()
        print(f"({index}/{len(discovered)}) {display}")
        if example is None:
            tally.report_skip(display, "no Cargo.toml or .csproj found")
            continue
        print(f"Building {example.kind} example - this may take a while")
        build_example(example, tally)


def build_single_example(target: str, tally: ResultTally) -> None:
    """Builds one named example, given a path relative to the repository."""
    path = Path(target)
    if not path.is_absolute():
        path = REPOSITORY_ROOT / target
    display = target.replace("\\", "/")

    section(f"Building {display} (release)")
    if not path.is_dir():
        tally.report_skip(display, "directory does not exist")
        return
    example = classify_example(path)
    if example is None:
        tally.report_skip(display, "not a valid project (no Cargo.toml or .csproj found)")
        return
    print(f"Building {example.kind} example - this may take a while")
    build_example(example, tally)


def build_parser() -> argparse.ArgumentParser:
    """Builds the command-line parser."""
    parser = argparse.ArgumentParser(
        prog="test_examples.py",
        description=(
            "Builds every example project with the release profile and reports artifact "
            "sizes. Exit 0 when all built, 1 on any failure, 2 on a usage error."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "examples:\n"
            "  test_examples.py\n"
            "  test_examples.py devops/tests/projects/native\n"
            "  test_examples.py --list\n"
        ),
    )
    parser.add_argument(
        "target",
        nargs="?",
        default="all",
        metavar="TARGET",
        help="'all' (default) or the path of a single example to build",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="Print the discovered examples and how each would be built, then exit",
    )
    return parser


def main() -> int:
    """Builds the requested examples and returns the summary exit code."""
    arguments = build_parser().parse_args()

    if arguments.list:
        discovered = discover_examples()
        for directory, example in discovered:
            display = directory.relative_to(REPOSITORY_ROOT).as_posix()
            kind = example.kind if example else "unrecognised (no manifest)"
            print(f"  {display:<40} {kind}")
        print()
        print(f"Total: {len(discovered)} example directory/ies, local and official")
        return 0

    print(colorize("Example project builds", ANSI_BOLD + ANSI_CYAN))
    tally = ResultTally()
    if arguments.target in ("all", ""):
        build_all_examples(tally)
    else:
        build_single_example(arguments.target, tally)
    return tally.print_summary()


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
