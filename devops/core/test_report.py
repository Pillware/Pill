"""
Terminal result reporting for the Python test scripts.

REQUIREMENTS: Python 3.8+ (standard library only).

DESCRIPTION
    The pass/fail/skip tally, the summary block, the binary-size report and
    the cargo test-result summary, shared by the lint, build and suite
    scripts so they all print the same output shape.

    The color handling is deliberately kept here rather than imported from
    `core.suite_common`: a lint or a build check has no business pulling in
    host-process plumbing (output monitors, backup registries) just to print
    a green PASS.

--- SCRIPT ---
"""

import os
import re
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

# =============================================================================
# Terminal colors
# =============================================================================

ANSI_RESET = "\033[0m"
ANSI_BOLD = "\033[1m"
ANSI_RED = "\033[0;31m"
ANSI_GREEN = "\033[0;32m"
ANSI_YELLOW = "\033[1;33m"
ANSI_CYAN = "\033[0;36m"


def _supports_color() -> bool:
    """Reports whether stdout can render ANSI escapes.

    Honors the `NO_COLOR` convention, and on Windows enables virtual terminal
    processing so escapes render in the classic console too.
    """
    if os.environ.get("NO_COLOR"):
        return False
    if not sys.stdout.isatty():
        return False
    if os.name == "nt":
        try:
            import ctypes

            kernel = ctypes.windll.kernel32
            # 7 == STD_OUTPUT_HANDLE as a negative DWORD; 0x0004 is
            # ENABLE_VIRTUAL_TERMINAL_PROCESSING.
            kernel.SetConsoleMode(kernel.GetStdHandle(-11), 7)
        except Exception:
            return False
    return True


USE_COLOR = _supports_color()


def colorize(text: str, color: str) -> str:
    """Wraps text in a color when the terminal supports it."""
    return f"{color}{text}{ANSI_RESET}" if USE_COLOR else text


def section(title: str) -> None:
    """Prints the banner the shell scripts use between check groups."""
    print()
    print(colorize("=" * 79, ANSI_CYAN))
    print(colorize(title, ANSI_BOLD + ANSI_CYAN))


# =============================================================================
# Result tally
# =============================================================================


class ResultTally:
    """Counts pass/fail/skip results and prints the closing summary."""

    def __init__(self) -> None:
        self.passed = 0
        self.failed = 0
        self.skipped = 0
        self.entries: List[str] = []

    def report_pass(self, description: str) -> None:
        """Records and prints a passing result."""
        print(f"  {colorize('PASS', ANSI_GREEN)} {description}")
        self.passed += 1
        self.entries.append(f"PASS|{description}")

    def report_fail(self, description: str, reason: str) -> None:
        """Records and prints a failing result with its reason."""
        print(f"  {colorize('FAIL', ANSI_RED)} {description} - {reason}")
        self.failed += 1
        self.entries.append(f"FAIL|{description} - {reason}")

    def report_skip(self, description: str, reason: str) -> None:
        """Records and prints a skipped result with its reason."""
        print(f"  {colorize('SKIP', ANSI_YELLOW)} {description} - {reason}")
        self.skipped += 1
        self.entries.append(f"SKIP|{description} - {reason}")

    def print_summary(self) -> int:
        """Prints the numbered result list and totals; returns the exit code.

        Returns 1 when anything failed, so a caller can `return
        tally.print_summary()` directly.
        """
        total = self.passed + self.failed + self.skipped
        print()
        print("=" * 40)
        for index, entry in enumerate(self.entries, start=1):
            status, _, description = entry.partition("|")
            color = {
                "PASS": ANSI_GREEN,
                "FAIL": ANSI_RED,
                "SKIP": ANSI_YELLOW,
            }.get(status, "")
            print(f"  {index:>3}. {colorize(status, color)} {description}")
        print("=" * 40)
        print(
            f"  total {total}   "
            f"{colorize('passed', ANSI_GREEN)} {self.passed}   "
            f"{colorize('failed', ANSI_RED)} {self.failed}   "
            f"{colorize('skipped', ANSI_YELLOW)} {self.skipped}"
        )
        return 1 if self.failed else 0


# =============================================================================
# Binary size reporting
# =============================================================================


def format_size(byte_count: int) -> str:
    """Formats a byte count with the largest sensible binary unit."""
    if byte_count >= 1024 * 1024:
        return f"{byte_count / (1024 * 1024):.2f} MB"
    if byte_count >= 1024:
        return f"{byte_count / 1024:.2f} KB"
    return f"{byte_count} B"


def size_report(directory: Path, recursive: bool = True) -> Optional[Dict[str, Any]]:
    """Builds the per-file size report for a build output directory.

    Returns None when the directory does not exist, matching the shell
    version's "print nothing" behaviour. Sizes are megabytes so the numbers
    stay comparable with the reports the bash scripts produced.

    `recursive=False` reports only the directory's own files. Cargo drops the
    shipped artifacts at the top of `target/<profile>/` and everything else in
    `deps/`, `build/` and `incremental/`; recursing there would report the
    whole build cache (hundreds of megabytes) instead of the binary.
    """
    if not directory.is_dir():
        return None
    files: List[Dict[str, Any]] = []
    total_megabytes = 0.0
    candidates = directory.rglob("*") if recursive else directory.iterdir()
    for path in sorted(candidates):
        if not path.is_file():
            continue
        megabytes = path.stat().st_size / (1024 * 1024)
        total_megabytes += megabytes
        files.append(
            {
                "file": path.relative_to(directory).as_posix(),
                "mb": round(megabytes, 4),
            }
        )
    return {
        "total_mb": round(total_megabytes, 4),
        "file_count": len(files),
        "files": files,
    }


def print_size_report(
    directory: Path, recursive: bool = True
) -> Optional[Dict[str, Any]]:
    """Prints the size report as JSON, and returns it for further use."""
    import json

    report = size_report(directory, recursive)
    if report is None:
        return None
    print("  Binary sizes:")
    for line in json.dumps(report, indent=2).splitlines():
        print(f"  {line}")
    return report


# =============================================================================
# Cargo test output
# =============================================================================


def summarize_test_results(output: str) -> str:
    """Totals the per-binary `test result:` lines cargo prints.

    Cargo reports one line per test binary, so a run spanning a dozen crates
    scrolls the interesting number off the screen. This collapses them into the
    single figure a developer actually wants.
    """
    passed = failed = ignored = 0
    for match in re.finditer(
        r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", output
    ):
        passed += int(match.group(1))
        failed += int(match.group(2))
        ignored += int(match.group(3))
    return f"  {passed} passed, {failed} failed, {ignored} ignored"
