#!/usr/bin/env python3
"""
Extension examples: build every example an extension ships, then run it headless
and check that it announces itself.

PROTECTS: An extension's example still builds and runs, and a new example cannot be added without being run.
TIER: build

REQUIREMENTS: Rust toolchain; .NET SDK 8 on PATH for an example with a .csproj.

DESCRIPTION
    `devops/tests/test_examples.py` builds example projects and never runs one,
    so an example can compile and still be dead on start. This suite closes that
    gap for the projects an extension ships under
    `modules/extensions/<extension>/examples/<name>/`:

      1. every discovered example is built by the host (a Rust project through
         cargo, a managed one through the in-process Roslyn compiler);
      2. every one is then run headless with a timeout;
      3. the run passes when the log carries `[<example folder name>]`, which is
         the convention the examples follow - their own name in brackets, once.

    An empty discovery is a failure, not a skip: the point of the suite is that
    an example cannot rot unnoticed, and a rule that quietly finds nothing is
    the failure mode it exists to prevent.

    Discovery is `core.projects.extension_example_roots`, the same function the
    build sweep reads, so the two cannot disagree about what an example is.

USAGE
    python devops/tests/test_extension_examples.py [--skip-build] [--timeout SECONDS]

EXAMPLE USAGE
    python devops/tests/test_extension_examples.py
    python devops/tests/test_extension_examples.py --skip-build
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

SUITES_DIRECTORY = Path(__file__).resolve().parent
DEVOPS_ROOT = SUITES_DIRECTORY.parent
REPOSITORY_ROOT = DEVOPS_ROOT.parent
MODULES_ROOT = REPOSITORY_ROOT / "modules"
if str(DEVOPS_ROOT) not in sys.path:
    sys.path.insert(0, str(DEVOPS_ROOT))

from core.projects import PROJECT_SETTINGS_FILENAME, extension_example_roots  # noqa: E402

HOST_PACKAGE = "pill_standalone"
DEFAULT_RUN_TIMEOUT_SECONDS = 90
POLL_INTERVAL_SECONDS = 0.25


def host_binary() -> Path:
    """Returns the debug host executable the suite runs examples with."""
    name = "pill_standalone.exe" if os.name == "nt" else "pill_standalone"
    return MODULES_ROOT / "target" / "debug" / name


def build_host(skip_build: bool) -> bool:
    """Builds the host once, unless asked to reuse what is there."""
    if skip_build:
        return host_binary().is_file()
    print(f"  building {HOST_PACKAGE} (debug)")
    completed = subprocess.run(
        ["cargo", "build", "--package", HOST_PACKAGE, "--offline"],
        cwd=MODULES_ROOT,
    )
    return completed.returncode == 0


def describe_example(example: Path) -> str:
    """Returns the kind of project an example folder holds."""
    if (example / "Cargo.toml").is_file() and (example / "project_settings.yaml").is_file():
        return "rust"
    if next(example.glob("*.csproj"), None) is not None:
        return "csharp"
    return "unknown"


def run_example(example: Path, timeout_seconds: int) -> tuple[bool, str]:
    """Runs one example headless and reports whether its token appeared."""
    token = f"[{example.name}]"
    log_path = Path(tempfile.gettempdir()) / f"extension-example-{example.name}.log"
    environment = dict(os.environ)
    environment["PROJECT_PATH"] = str(example)

    started = time.monotonic()
    with log_path.open("w", encoding="utf-8", errors="replace") as log_file:
        process = subprocess.Popen(
            [str(host_binary())],
            cwd=str(MODULES_ROOT),
            env=environment,
            stdout=log_file,
            stderr=subprocess.STDOUT,
        )
        seen_token = False
        while time.monotonic() - started < timeout_seconds:
            if process.poll() is not None:
                break
            try:
                if token in log_path.read_text(encoding="utf-8", errors="replace"):
                    seen_token = True
                    break
            except OSError:
                pass
            # The example builds and then loads before it announces itself, so
            # the log has to be polled; one interval keeps the read off the
            # host's hot path without adding a noticeable delay to the wait.
            time.sleep(POLL_INTERVAL_SECONDS)
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()

    text = log_path.read_text(encoding="utf-8", errors="replace")
    if token in text:
        seen_token = True
    if seen_token:
        return True, str(log_path)
    reason = f"no `{token}` line within {timeout_seconds}s"
    if process.returncode not in (None, 0) and "Host setup failed" in text:
        reason = "the host failed to set the example up"
    return False, f"{reason} (log: {log_path})"


def main() -> int:
    """Runs every extension example and returns the suite's exit code."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[1])
    parser.add_argument("--skip-build", action="store_true", help="reuse the host binary")
    parser.add_argument(
        "--timeout",
        type=int,
        default=DEFAULT_RUN_TIMEOUT_SECONDS,
        help="seconds one example may take to announce itself",
    )
    arguments = parser.parse_args()

    examples = extension_example_roots()
    if not examples:
        print("  FAIL  no extension example was discovered")
        print(
            "        an example is a directory at "
            f"modules/extensions/<extension>/examples/<name>/{PROJECT_SETTINGS_FILENAME}"
        )
        return 1

    if not build_host(arguments.skip_build):
        print("  FAIL  the host did not build, so no example can run")
        return 1

    failures = 0
    for example in examples:
        display = example.relative_to(REPOSITORY_ROOT).as_posix()
        kind = describe_example(example)
        if kind == "unknown":
            failures += 1
            print(f"  FAIL  {display}: no Cargo.toml and no .csproj")
            continue
        passed, detail = run_example(example, arguments.timeout)
        if passed:
            print(f"  PASS  {display} ({kind})")
        else:
            failures += 1
            print(f"  FAIL  {display} ({kind}): {detail}")

    print()
    print(
        f"  extension examples: {len(examples)} found, "
        f"{len(examples) - failures} passed, {failures} failed"
    )
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
