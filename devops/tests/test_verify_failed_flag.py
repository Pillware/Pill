#!/usr/bin/env python3
"""
`verify.py --failed`: the gate reruns what failed, plus the quick steps.

PROTECTS: A red gate is followed by a rerun of the failed steps, not by a second whole gate.
TIER: static

REQUIREMENTS: Python 3.8+ (standard library only).

DESCRIPTION
    A whole-gate run ends at its first quick failure, so the steps after it
    were never run, and the agent that fixes the failure needs both halves:
    the red steps and the inner loop that says whether the fix holds. The gate
    therefore records every run's step outcomes in `local/ci/verify-last-run.json`
    (written per step, so a stopped run still leaves what it saw), and
    `--failed` reads that file and selects the failed steps plus the quick
    ones - never the slow steps that never got a chance to run.

    This suite checks the three parts that make that honest:

      1. round trip  - a written record reads back with exactly its failed
                       steps (and no others: a skipped or passed step is not
                       a rerun candidate).
      2. selection   - the failed steps and every quick step come back in
                       declaration order, and a name the gate no longer has
                       is dropped instead of failing the run.
      3. the command - `--failed --list` on a synthetic record prints the
                       selection and runs nothing; no record is a usage error;
                       a clean record reruns nothing.

USAGE
  python devops/tests/test_verify_failed_flag.py [--root <repository>]

EXAMPLE USAGE
  python devops/tests/test_verify_failed_flag.py
"""

import argparse
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
from typing import List

FAILURES: List[str] = []


def load_gate(root: Path):
    """The gate module, imported by path so its helpers can be called."""
    path = root / "devops" / "ci_cd" / "verify.py"
    spec = importlib.util.spec_from_file_location("rh_ecs_verify_gate", str(path))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def check(failures: List[str], condition: bool, message: str) -> None:
    """Record one failed condition with a readable message."""
    if not condition:
        failures.append(message)


def record_of(verify, steps):
    """A last-run record naming `steps` as (name, state) pairs."""
    return {"finished": "2026-11-15T12:00:00+01:00",
            "steps": [{"name": name, "state": state} for name, state in steps]}


def check_round_trip(verify, folder: Path, failures: List[str]) -> None:
    """A written record reads back with exactly its failed steps."""
    path = folder / "verify-last-run.json"
    os.environ[verify.LAST_RUN_PATH_ENVIRONMENT] = str(path)
    try:
        verify.write_last_run([(verify.STEPS[0], "FAIL", 1.0), (verify.STEPS[1], "PASS", 2.0),
                               (verify.STEPS[2], "SKIP", 0.0)])
        check(failures, path.is_file(), "write_last_run wrote no file")
        check(failures, verify.last_failed_steps() == [verify.STEPS[0].name],
              f"last_failed_steps read {verify.last_failed_steps()}, expected [{verify.STEPS[0].name}]")
        recorded = json.loads(path.read_text(encoding="utf-8"))
        check(failures, [step["state"] for step in recorded["steps"]] == ["failed", "passed", "skipped"],
              f"the record's states are {recorded['steps']}")
        path.unlink()
        check(failures, verify.last_failed_steps() is None, "a missing record must read as None")
        path.write_text("not json", encoding="utf-8")
        check(failures, verify.last_failed_steps() is None, "an unreadable record must read as None")
    finally:
        os.environ.pop(verify.LAST_RUN_PATH_ENVIRONMENT, None)


def check_selection(verify, failures: List[str]) -> None:
    """Failed steps and every quick step, in declaration order, unknown names dropped."""
    quick_names = [step.name for step in verify.STEPS if step.quick]
    slow_step = next(step for step in verify.STEPS if not step.quick)
    selected = [step.name for step in verify.failed_selection([slow_step.name])]
    expected = [step.name for step in verify.STEPS if step.quick or step.name == slow_step.name]
    check(failures, selected == expected, f"failed_selection gave {selected}, expected {expected}")
    check(failures, all(name in selected for name in quick_names),
          f"failed_selection dropped quick step(s): {quick_names} not all in {selected}")
    check(failures, [step.name for step in verify.failed_selection(["no-such-step"])] == quick_names,
          "a name the gate no longer has must be dropped, leaving the quick steps")


def run_gate(verify, root: Path, record, extra_arguments: List[str]) -> subprocess.CompletedProcess:
    """Run the real `verify.py` with a synthetic last-run record."""
    with tempfile.TemporaryDirectory(prefix="rh-verify-failed-") as folder:
        path = Path(folder) / "verify-last-run.json"
        if record is not None:
            path.write_text(json.dumps(record), encoding="utf-8")
        environment = dict(os.environ)
        environment[verify.LAST_RUN_PATH_ENVIRONMENT] = str(path)
        return subprocess.run([sys.executable, str(root / "devops" / "ci_cd" / "verify.py"), *extra_arguments],
                              cwd=str(root), env=environment, capture_output=True, text=True, timeout=120)


def check_command(root: Path, verify, failures: List[str]) -> None:
    """The real command: selected, not selected, and no record at all."""
    slow_step = next(step for step in verify.STEPS if not step.quick and not step.cleanup)
    quick_names = [step.name for step in verify.STEPS if step.quick]
    record = record_of(verify, [(slow_step.name, "failed"), (verify.STEPS[0].name, "passed")])

    listed = run_gate(verify, root, record, ["--failed", "--list"])
    check(failures, listed.returncode == 0, f"--failed --list exited {listed.returncode}: {listed.stderr}")
    selection_line = next((line for line in listed.stdout.splitlines() if line.startswith("--failed would run:")), "")
    check(failures, bool(selection_line), f"--failed --list printed no selection:\n{listed.stdout}")
    chosen = [name.strip() for name in selection_line.partition(":")[2].split(",") if name.strip()]
    check(failures, slow_step.name in chosen, f"{slow_step.name} (the failed step) is not in {chosen}")
    check(failures, all(name in chosen for name in quick_names),
          f"the quick steps {quick_names} are not all in {chosen}")
    check(failures, "SKIP" not in listed.stdout and "=== Summary ===" not in listed.stdout,
          "--failed --list must not run any step")

    clean = run_gate(verify, root, record_of(verify, [(verify.STEPS[0].name, "passed")]), ["--failed", "--list"])
    check(failures, clean.returncode == 0, f"a clean record exited {clean.returncode}: {clean.stderr}")
    check(failures, "nothing to rerun" in clean.stdout, f"a clean record ran something:\n{clean.stdout}")

    missing = run_gate(verify, root, None, ["--failed", "--list"])
    check(failures, missing.returncode == 2, f"no record exited {missing.returncode}, expected 2")
    check(failures, "no recorded run" in missing.stderr, f"no record printed: {missing.stderr}")


def check_selection_through_cli(verify, root: Path, failures: List[str]) -> None:
    """`select_steps` itself, so a mutation of the parser is caught too."""
    arguments = verify.build_parser().parse_args(["--failed"])
    os.environ[verify.LAST_RUN_PATH_ENVIRONMENT] = str(Path(tempfile.gettempdir()) / "rh-verify-none.json")
    try:
        steps = verify.select_steps(arguments)
        check(failures, steps is None, f"--failed without a record returned {steps} instead of a usage error")
    finally:
        os.environ.pop(verify.LAST_RUN_PATH_ENVIRONMENT, None)
    quick_name = next(step.name for step in verify.STEPS if step.quick)
    failed_step = next(step for step in verify.STEPS if not step.quick)
    arguments = verify.build_parser().parse_args(["--failed", "--skip", quick_name])
    with tempfile.TemporaryDirectory(prefix="rh-verify-failed-") as folder:
        path = Path(folder) / "verify-last-run.json"
        path.write_text(json.dumps(record_of(verify, [(failed_step.name, "failed")])), encoding="utf-8")
        os.environ[verify.LAST_RUN_PATH_ENVIRONMENT] = str(path)
        try:
            steps = [step.name for step in verify.select_steps(arguments) or []]
        finally:
            os.environ.pop(verify.LAST_RUN_PATH_ENVIRONMENT, None)
    check(failures, failed_step.name in steps, f"--failed did not select the failed step: {steps}")
    check(failures, quick_name not in steps, f"--skip {quick_name} was ignored by --failed: {steps}")
    check(failures, steps == sorted(steps, key=[step.name for step in verify.STEPS].index),
          f"--failed reordered the steps: {steps}")


def main() -> int:
    """Run the three parts; 0 when they all hold."""
    parser = argparse.ArgumentParser(description="Check verify.py --failed.")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    arguments = parser.parse_args()
    root = arguments.root.resolve()
    verify = load_gate(root)
    failures: List[str] = []
    with tempfile.TemporaryDirectory(prefix="rh-verify-failed-") as folder:
        check_round_trip(verify, Path(folder), failures)
    check_selection(verify, failures)
    check_command(root, verify, failures)
    check_selection_through_cli(verify, root, failures)
    for failure in failures:
        print(f"  FAIL  {failure}")
    if failures:
        return 1
    print("  PASS  verify.py --failed reruns the failed steps plus the quick ones, and nothing else")
    return 0


if __name__ == "__main__":
    sys.exit(main())
