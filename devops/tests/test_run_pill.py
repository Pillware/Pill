#!/usr/bin/env python3
"""
The project launcher: the command it builds from a project's settings.

PROTECTS: The one command line a project starts with, so a renderer project, a headless project and the editor posture are decided by the settings rather than by whoever typed the cargo line.
TIER: static

REQUIREMENTS: Python 3.11+ and PyYAML (standard library otherwise); no Rust
              toolchain, because every check runs `--print-command`.

DESCRIPTION
    `devops/tools/run_pill.py` turns a project directory into the command that
    runs it. The command has to agree with what the host reads from the same
    file, and the two ways it can come apart are worth pinning:

    - The window. A project that enables a renderer draws, so the command carries
      the `rendering` feature; a project that enables none is headless, so it
      does not. `--headless` overrides both, which is how a project with a
      renderer runs on a machine with no GPU.
    - The posture. `--editor` starts the editor crate, `--shipping` delegates to
      the shipping build, and `--print-command` prints the line instead of
      running it - the last one is what makes these checks cost nothing.

    The module features are deliberately absent: the host writes each module's
    wrapper and its feature list comes from `modules.<module>.build`, so a
    command naming them would be a second, drifting source of truth.

USAGE
    python devops/tests/test_run_pill.py

EXAMPLE USAGE
    python devops/tests/test_run_pill.py
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

SUITES_DIRECTORY = Path(__file__).resolve().parent
DEVOPS_ROOT = SUITES_DIRECTORY.parent
if str(DEVOPS_ROOT) not in sys.path:
    sys.path.insert(0, str(DEVOPS_ROOT))

from core.projects import (  # noqa: E402
    OFFICIAL_EXAMPLE_PROJECTS,
    REPOSITORY_ROOT,
)

LAUNCHER = DEVOPS_ROOT / "tools" / "run_pill.py"
RENDERING_PROJECT = OFFICIAL_EXAMPLE_PROJECTS["cube_rust"]
HEADLESS_PROJECT = OFFICIAL_EXAMPLE_PROJECTS["headless_rust"]


def check(condition: bool, message: str, failures: list) -> None:
    """Records one failed expectation, printing it as it happens."""
    if condition:
        print(f"  [OK] {message}")
    else:
        print(f"  [FAIL] {message}")
        failures.append(message)


def print_command(project_root: Path, *flags: str) -> str:
    """The launcher's printed command line for `project_root`, or "" on failure."""
    completed = subprocess.run(
        [
            sys.executable,
            str(LAUNCHER),
            str(project_root),
            "--print-command",
            *flags,
        ],
        cwd=str(REPOSITORY_ROOT),
        capture_output=True,
        text=True,
        timeout=120,
    )
    if completed.returncode != 0:
        print(completed.stdout)
        print(completed.stderr)
        return ""
    return completed.stdout.strip()


def check_project_path_is_required(failures: list) -> None:
    """A launch with no project path is refused, before anything runs."""
    completed = subprocess.run(
        [sys.executable, str(LAUNCHER), "--print-command"],
        cwd=str(REPOSITORY_ROOT),
        capture_output=True,
        text=True,
        timeout=120,
    )
    check(
        completed.returncode != 0 and "one project path is required" in completed.stderr,
        "a launch with no project path is refused",
        failures,
    )


def check_window_follows_the_renderer(failures: list) -> None:
    """A project that enables a renderer carries the feature; a headless one does not."""
    drawing = print_command(RENDERING_PROJECT)
    headless = print_command(HEADLESS_PROJECT)
    check(
        "cargo run -p pill_standalone" in drawing and "--features rendering" in drawing,
        f"a project with a renderer draws: {drawing}",
        failures,
    )
    check(
        "cargo run -p pill_standalone" in headless and "--features" not in headless,
        f"a project with no module listed is headless: {headless}",
        failures,
    )
    check(
        f"PROJECT_PATH={RENDERING_PROJECT}" in drawing,
        "the command carries the project path the host reads",
        failures,
    )


def check_flags_choose_the_posture(failures: list) -> None:
    """`--headless` and `--editor` change the crate or the features, nothing else."""
    headless_override = print_command(RENDERING_PROJECT, "--headless")
    check(
        "cargo run -p pill_standalone" in headless_override
        and "--features" not in headless_override,
        f"--headless drops the window from a project that enables a renderer: {headless_override}",
        failures,
    )
    editor = print_command(RENDERING_PROJECT, "--editor")
    check(
        "cargo run -p pill_editor" in editor,
        f"--editor starts the editor crate: {editor}",
        failures,
    )
    shipping = print_command(RENDERING_PROJECT, "--shipping")
    check(
        "build_shipping.py" in shipping and "--project" in shipping,
        f"--shipping delegates to the shipping build: {shipping}",
        failures,
    )
    check(
        "--local_project_settings" not in shipping,
        "a shipping build ignores the local override unless asked",
        failures,
    )
    local_shipping = print_command(RENDERING_PROJECT, "--shipping", "--local_project_settings")
    check(
        "--local_project_settings" in local_shipping,
        f"a shipping build reads the local override when asked: {local_shipping}",
        failures,
    )


def check_module_features_stay_out_of_the_command(failures: list) -> None:
    """The command names no module feature: the host's wrappers carry those."""
    with tempfile.TemporaryDirectory() as directory:
        project = Path(directory)
        (project / "project_settings.yaml").write_text(
            "name: \"Probe\"\n"
            "build_binary_name: \"Probe\"\n"
            "modules:\n"
            "  pill_rendering:\n"
            "    enabled: true\n"
            "  pill_renderer:\n"
            "    enabled: true\n"
            "    build:\n"
            "      ray_tracing: true\n",
            encoding="utf-8",
        )
        line = print_command(project)
    check(
        "--features rendering" in line and "ray_tracing" not in line,
        f"the build option stays off the command line: {line}",
        failures,
    )


def main() -> int:
    """Runs every launcher check; returns 0 when they all pass."""
    failures: list = []
    print("Project launcher")
    check_project_path_is_required(failures)
    check_window_follows_the_renderer(failures)
    check_flags_choose_the_posture(failures)
    check_module_features_stay_out_of_the_command(failures)
    print()
    if failures:
        print(f"FAILED: {len(failures)} check(s)")
        return 1
    print("PASS: the launcher builds the command the settings ask for.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
