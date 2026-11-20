#!/usr/bin/env python3
"""
Suite lint for `devops/tests`: the header `run_all.py` reads, and the rules
that keep the shared helpers from being forked again.

PROTECTS: Suites keep their run_all header, stop re-forking the core helpers, say
          why they sleep, anchor their output before they write, name project
          directories through `core.projects`, leave the test project set inside
          its weight budget, and define each module-level name once.
TIER: static

REQUIREMENTS: Python 3.8+ (standard library only).

DESCRIPTION
    The end-to-end suites are the sharpest tools in the repository and the
    easiest to fork by accident: a private `read_log` in one file, a bare
    settle in another, and the shared helpers quietly stop being the only way
    to drive a host. This lint holds every `devops/tests/test_*.py` to eight
    rules:

      1. header   - the docstring declares the `PROTECTS:` line and a valid
                    `TIER:` that `run_all.py` reads. Checked by calling
                    run_all's own discovery, so the lint and the runner can
                    never disagree about what a header must look like.
      2. no forks - no suite defines its own `read_log`, `wait_token`,
                    `atomic_write`, `require` or `kill_*` helper. The lock,
                    the output monitor, the wait-and-report and the atomic
                    source edit live in `devops/core/`, and a second copy is
                    how the suites drifted apart.
      3. sleep    - every `time.sleep` call carries a comment above it (or on
                    it) saying why it waits. A suite that cannot say why it
                    sleeps is a suite that will one day sleep for nothing.
      4. anchor   - an output anchor (`start_index = monitor.line_count`) is
                    taken *before* the write it anchors. A host can answer a
                    body-only edit within the write's own turn, and a line
                    counted into the anchor never appears in
                    `output_since(start_index)` - the crate then reads as
                    `TIMED OUT` while its fast path is healthy.
      5. project  - a suite names a project directory through `core.projects`,
                    never as a literal. A literal is how one project move
                    turned into a list of files to edit, and a stale literal
                    fails as "no such project" only when that suite runs.
      6. budget   - the test project set under `devops/tests/projects` stays inside
                    its weight budget and ships no file twice. The seven
                    test projects this set replaced weighed 37.4 MB, almost all of
                    it the same maps and models copied three times, and
                    nothing measured it until an audit did.
      7. poll     - a sleep waits inside a poll loop, or the function that
                    owns it is a named exception. A fixed sleep that stands
                    in for a wait is a regression the moment a step cannot
                    see its own token.
      8. unique   - each module-level name is defined once. Python rebinds a
                    repeated `class` or `def` without a warning, so the first
                    copy is dead code that still looks live.

    Each rule ships with a planted-violation self-test (`--self-test`), so a
    rule that stopped matching fails here, where it is cheap to see, instead
    of letting a forked helper through into a green run.

USAGE
  python devops/tests/test_suite_standards.py
  python devops/tests/test_suite_standards.py --self-test

EXAMPLE USAGE
  python devops/tests/test_suite_standards.py

  Exit status: 0 when every suite complies, 1 on violations, 2 on a usage
  error.

--- SCRIPT ---
"""

import argparse
import ast
import hashlib
import re
import sys
import tempfile
from pathlib import Path
from typing import Dict, List, Tuple

# Standalone-runnable: `run_all` and the suites this lints sit beside this
# file, and run_all puts `devops/` on `sys.path` itself for `core`.
SUITES_DIRECTORY = Path(__file__).resolve().parent
sys.path.insert(0, str(SUITES_DIRECTORY))
sys.path.insert(0, str(SUITES_DIRECTORY.parent))

import run_all  # noqa: E402

from core.projects import TEST_PROJECTS_ROOT  # noqa: E402

# A helper under one of these names belongs to `devops/core/`: `read_log`,
# `atomic_write` and `require` live in `core/suite_common.py`, `wait_token`
# in the editor driver, and `kill_*` covers stray-host cleanup, which may
# only run under the lock.
FORKED_HELPER_PATTERN = re.compile(
    r"^\s*def (read_log|wait_token|atomic_write|require|kill_[a-z_]*)\s*\(", re.MULTILINE
)

# A real call, as code. Matched anywhere in the line, because a suite may
# guard one with `if` on the same line.
SLEEP_CALL_PATTERN = re.compile(r"time\.sleep\s*\(")

# Rule 4's two halves: an anchor assignment, and a source write that must not
# come before it. Both are matched as code, so a docstring that merely names
# them is not a violation.
ANCHOR_ASSIGNMENT_PATTERN = re.compile(r"^\s*\w+\s*=\s*\w+\.line_count\b")
SOURCE_WRITE_PATTERN = re.compile(
    r"\b(atomic_write|restore_one|write_bytes|write_text)\s*\("
)

# Rule 5's shape: a quoted project directory, complete in one segment. A path
# into a file or a synthetic layout carries another slash and is left alone;
# so is prose, which is not a bare literal.
PROJECT_PATH_LITERAL_PATTERN = re.compile(
    r'"(?:\.\./)?examples/[A-Za-z0-9_.-]+"|"\.\./devops/tests/[A-Za-z0-9_.-]+"'
)

# Rule 7's two lists: the step-area modules the editor suite is split into
# carry sleeps too, and the one call that is deliberately not a poll - the idle
# stretch the redraw-cadence step measures across, which IS the interval.
STEP_MODULE_GLOB = "editor_steps_*.py"
VERIFIED_SLEEP_EXCEPTIONS = {
    "step_12_redraw_cadence": "the 3 s idle stretch is the interval the step measures",
}


def discover_step_files() -> List[Path]:
    """The step-area modules beside the suites: they carry the step bodies."""
    return sorted(SUITES_DIRECTORY.glob(STEP_MODULE_GLOB))


def discover_module_files() -> List[Path]:
    """Every module beside the suites, for the duplicate-definition rule."""
    return sorted(SUITES_DIRECTORY.glob("*.py"))


# Rule 6's two numbers: the whole test project set's weight, and the size below which
# a repeated file is not worth reporting. A small stub mesh legitimately ships in
# more than one test project; a map that size is how the set reached 37.4 MB.
TEST_PROJECT_BUDGET_MEGABYTES = 10.0
TEST_PROJECT_DUPLICATE_MINIMUM_BYTES = 4096

# A test project's build output, which is never counted and never committed.
TEST_PROJECT_BUILD_OUTPUT_NAMES = {"target", "build", "bin", "obj", ".git"}


# Every suite this lint covers: the same glob `run_all.py` discovers.
def discover_files() -> List[Path]:
    return sorted(SUITES_DIRECTORY.glob("test_*.py"))


# Rule 1's findings, from run_all's own discovery - the reader both sides
# share, so a header rule change cannot land in one and not the other.
def header_findings() -> List[str]:
    _suites, errors = run_all.discover_suites()
    return errors


# Rule 2: a suite may call these helpers, never define them.
def fork_findings(path: Path, text: str) -> List[str]:
    found = []
    for match in FORKED_HELPER_PATTERN.finditer(text):
        line = text.count("\n", 0, match.start()) + 1
        found.append(
            f"{path.name}:{line}: defines `{match.group(1)}`; the shared helper in "
            "devops/core is the only copy"
        )
    return found


# Rule 3: a sleep says why it waits, on the line or directly above it.
#
# Directly above, not somewhere above: the suites document a wait with one
# comment per call site. A rule that accepted a comment three lines up would
# pass a loop whose sleep has drifted away from the reason it was written for.
def sleep_findings(path: Path, text: str) -> List[str]:
    found = []
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if SLEEP_CALL_PATTERN.search(line) is None or line.lstrip().startswith("#"):
            continue
        if "#" in line.split("time.sleep", 1)[-1]:
            continue
        above = next((candidate for candidate in reversed(lines[:index]) if candidate.strip()), "")
        if not above.lstrip().startswith("#"):
            found.append(f"{path.name}:{index + 1}: a sleep with no comment above it saying why")
    return found


# Rule 7: a sleep waits for nothing unless it is a poll interval.
#
# A poll sleeps inside its `while`; a fixed sleep that stands in for a wait is
# a regression the moment a step cannot see its own token, which is why the
# editor suite has none left. The one exception is named: the cadence step's
# 3 s idle stretch is the interval it measures, not a wait for something to
# happen.
def sleep_outside_poll_findings(path: Path, text: str) -> List[str]:
    found = []

    def walk(node: ast.AST, in_loop: bool, function: str) -> None:
        for child in ast.iter_child_nodes(node):
            name = function
            if isinstance(child, ast.FunctionDef):
                name = child.name
            elif isinstance(child, ast.ClassDef):
                name = ""
            if isinstance(child, ast.While):
                walk(child, True, name)
                continue
            if isinstance(child, ast.Call):
                target = child.func
                called = (
                    target.attr
                    if isinstance(target, ast.Attribute)
                    else getattr(target, "id", "")
                )
                base = getattr(getattr(target, "value", None), "id", "")
                if called == "sleep" and base == "time" and not in_loop:
                    if name not in VERIFIED_SLEEP_EXCEPTIONS:
                        found.append(
                            "{}:{}: sleeps outside a poll loop in {}; wait for the \n"
                            "                            "
                            "token or name the exception".format(
                                path.name, child.lineno, name or "module scope"
                            )
                        )
            walk(child, in_loop, name)

    walk(ast.parse(text), False, "")
    return found


# Rule 4: an anchor is taken before the write it anchors.#
# The test is "the statement immediately before the anchor is a write", walking
# back over blanks, comments and progress prints. That is the shape the defect
# takes - `atomic_write(...)` on one line, the anchor on the next - and it
# leaves alone the legitimate neighbours: an anchor at the top of a function,
# and an anchor after a wait that read the previous write back.
def anchor_findings(path: Path, text: str) -> List[str]:
    found = []
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if ANCHOR_ASSIGNMENT_PATTERN.search(line) is None:
            continue
        for previous in reversed(lines[:index]):
            stripped = previous.strip()
            if not stripped or stripped.startswith("#") or stripped.startswith("print("):
                continue
            if SOURCE_WRITE_PATTERN.search(previous):
                found.append(
                    f"{path.name}:{index + 1}: the output anchor is taken after a source "
                    "write; take it before, or an outcome the host reports in between is lost"
                )
            break
    return found


# Rule 5: a project directory is named through `core.projects`.
#
# Only a bare literal is reported: a quoted one-segment directory under the
# examples root or the suite's test project. A path into a file carries another
# slash and prose carries spaces, so neither is flagged.
def project_findings(path: Path, text: str) -> List[str]:
    found = []
    for match in PROJECT_PATH_LITERAL_PATTERN.finditer(text):
        line = text.count("\n", 0, match.start()) + 1
        found.append(
            f"{path.name}:{line}: names {match.group(0)}; import the path from "
            "devops/core/projects.py"
        )
    return found


# Rule 6: the test project set is weighed, and no file ships twice.
#
# Measured over what a test project commits: its sources and its assets, never the
# build output a run leaves behind. The duplication half only reports a file
# large enough to matter, so a stub mesh copied between test projects stays legal.
def project_set_findings(
    test_projects_root: Path = TEST_PROJECTS_ROOT,
) -> Tuple[int, List[str]]:
    found = []
    total_bytes = 0
    owners: Dict[str, List[str]] = {}
    for path in sorted(test_projects_root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(test_projects_root)
        if any(part in TEST_PROJECT_BUILD_OUTPUT_NAMES for part in relative.parts):
            continue
        size = path.stat().st_size
        total_bytes += size
        if size < TEST_PROJECT_DUPLICATE_MINIMUM_BYTES:
            continue
        digest = hashlib.sha1(path.read_bytes()).hexdigest()
        owners.setdefault(digest, []).append(str(relative))
    for paths in owners.values():
        test_projects = {path.replace("\\", "/").split("/")[0] for path in paths}
        if len(test_projects) > 1:
            found.append(
                "the same file ships in several test projects: " + ", ".join(sorted(paths))
            )
    limit_bytes = int(TEST_PROJECT_BUDGET_MEGABYTES * 1024 * 1024)
    if total_bytes > limit_bytes:
        found.append(
            "the test project set weighs {:.2f} MB, over its {} MB budget".format(
                total_bytes / (1024 * 1024), TEST_PROJECT_BUDGET_MEGABYTES
            )
        )
    return total_bytes, found


# Rule 8: one definition per module-level name.
#
# Python rebinds a repeated name at module scope silently, so the first copy
# of a pasted class or function is dead while still looking like the live one.
# Only the module's own body is read: a name assigned inside an `if` is not a
# second definition of the same thing.
def duplicate_definition_findings(path: Path, text: str) -> List[str]:
    found = []
    first_lines: Dict[str, int] = {}
    for node in ast.parse(text).body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            names = [node.name]
        elif isinstance(node, ast.Assign):
            names = [target.id for target in node.targets if isinstance(target, ast.Name)]
        elif isinstance(node, ast.AnnAssign):
            names = [node.target.id] if isinstance(node.target, ast.Name) else []
        else:
            continue
        for name in names:
            if name in first_lines:
                found.append(
                    f"{path.name}:{node.lineno}: `{name}` is defined again; the "
                    f"definition at line {first_lines[name]} is dead"
                )
            else:
                first_lines[name] = node.lineno
    return found


# Every rule's findings, in report order.
def collect() -> List[Tuple[str, List[str]]]:
    sources = [(path, path.read_text(encoding="utf-8")) for path in discover_files()]
    step_sources = [
        (path, path.read_text(encoding="utf-8")) for path in discover_step_files()
    ]
    # Rule 7 landed with the editor suite's sleep audit; it is scoped to that
    # suite and its step modules here, and the other suites adopt it as they are
    # touched (their sleeps predate the rule and are each a finding of their own).
    editor_sources = [
        (path, text) for path, text in sources if path.name == "test_editor_egui.py"
    ]
    measured_bytes, test_project_problems = project_set_findings()
    return [
        ("rule 1: every suite has the header run_all reads", header_findings()),
        (
            "rule 2: no suite forks a core helper",
            [finding for path, text in sources for finding in fork_findings(path, text)],
        ),
        (
            "rule 3: every sleep says why it waits",
            [finding for path, text in sources for finding in sleep_findings(path, text)],
        ),
        (
            "rule 4: every output anchor is taken before the write it anchors",
            [finding for path, text in sources for finding in anchor_findings(path, text)],
        ),
        (
            "rule 5: every project path comes from core.projects",
            [finding for path, text in sources for finding in project_findings(path, text)],
        ),
        (
            "rule 6: the test project set stays inside its {} MB budget "
            "(measured {:.2f} MB)".format(
                TEST_PROJECT_BUDGET_MEGABYTES, measured_bytes / (1024 * 1024)
            ),
            test_project_problems,
        ),
        (
            "rule 7: every sleep is a poll interval or a named exception",
            [
                finding
                for path, text in editor_sources + step_sources
                for finding in sleep_outside_poll_findings(path, text)
            ],
        ),
        (
            "rule 8: every module-level name is defined once",
            [
                finding
                for path in discover_module_files()
                for finding in duplicate_definition_findings(
                    path, path.read_text(encoding="utf-8")
                )
            ],
        ),
    ]


# --- Self test ---------------------------------------------------------------

# Assembled from parts so this lint does not flag its own fixture text: the
# rules above scan every suite, and this file is one of them.
BARE_SLEEP = "time" + ".sleep(0.5)"
COMMENTED_SLEEP = "# Let the watcher see the write before the next edit.\n" + BARE_SLEEP
WRITE_FIXTURE = "atomic" + "_write(path, text)"
ANCHOR_FIXTURE = "start_" + "index = monitor.line_count"
PROJECT_LITERAL_FIXTURE = '"../examples/' + 'project_rs"'


# Every rule accepts its compliant fixture and fails its planted violation.
def self_test() -> int:
    failures = 0

    def expect(description: str, ok: bool) -> None:
        nonlocal failures
        print(f"  [{'PASS' if ok else 'FAIL'}] {description}")
        if not ok:
            failures += 1

    fixture = SUITES_DIRECTORY / "test_suite_standards.py"
    expect(
        "a forked helper is caught",
        len(fork_findings(fixture, "def " + "read_log(path):\n    return []\n")) == 1,
    )
    expect(
        "a forked require is caught",
        len(fork_findings(fixture, "def " + "require(monitor, token):\n    return True\n")) == 1,
    )
    expect(
        "calling a shared helper passes",
        fork_findings(fixture, "    lines = read_log(path)\n") == [],
    )
    expect("a bare sleep is caught", len(sleep_findings(fixture, "    " + BARE_SLEEP + "\n")) == 1)
    expect(
        "a comment above the sleep passes",
        sleep_findings(fixture, "    " + COMMENTED_SLEEP + "\n") == [],
    )
    expect(
        "a trailing comment passes",
        sleep_findings(fixture, "    " + BARE_SLEEP + "  # give the host a frame\n") == [],
    )
    expect(
        "a comment two lines above does not count",
        len(sleep_findings(fixture, "    # Poll until the marker shows.\n    check()\n    " + BARE_SLEEP + "\n"))
        == 1,
    )
    expect(
        "a sleep outside a poll loop is caught",
        len(
            sleep_outside_poll_findings(
                fixture, "def step():\n    drain()\n    " + BARE_SLEEP + "\n"
            )
        )
        == 1,
    )
    expect(
        "a sleep inside a poll loop passes",
        sleep_outside_poll_findings(
            fixture,
            "def step():\n    while not done():\n        " + BARE_SLEEP + "\n",
        )
        == [],
    )
    expect(
        "a named exception passes",
        sleep_outside_poll_findings(
            fixture,
            "def step_12_redraw_cadence():\n    " + BARE_SLEEP + "\n",
        )
        == [],
    )
    expect(
        "an anchor after the write is caught",
        len(anchor_findings(fixture, "    " + WRITE_FIXTURE + "\n    " + ANCHOR_FIXTURE + "\n")) == 1,
    )
    expect(
        "an anchor before the write passes",
        anchor_findings(fixture, "    " + ANCHOR_FIXTURE + "\n    " + WRITE_FIXTURE + "\n") == [],
    )
    expect(
        "an anchor after a wait passes",
        anchor_findings(
            fixture,
            "    " + WRITE_FIXTURE + "\n    check_reimported(monitor, 0)\n    " + ANCHOR_FIXTURE + "\n",
        )
        == [],
    )
    expect(
        "a duplicated definition is caught",
        len(
            duplicate_definition_findings(
                fixture,
                "class " + "StepState:\n    pass\n\n\nclass StepState:\n    pass\n",
            )
        )
        == 1,
    )
    expect(
        "one definition per name passes",
        duplicate_definition_findings(
            fixture, "class " + "StepState:\n    pass\n\ndef helper():\n    pass\n"
        )
        == [],
    )
    expect(
        "a literal project path is caught",
        len(project_findings(fixture, "PROJECT_ROOT = " + PROJECT_LITERAL_FIXTURE + "\n")) == 1,
    )
    expect(
        "a catalog name passes",
        project_findings(fixture, "PROJECT_ROOT = NATIVE_PROJECT_ROOT\n") == [],
    )
    expect(
        "a path into a project file passes",
        project_findings(fixture, 'source = "devops/tests/projects/native/src/lib.rs"\n') == [],
    )
    expect(
        "a missing PROTECTS line is caught",
        run_all.header_field('"""\nTIER: static\n"""', "PROTECTS") == "",
    )
    expect(
        "an unknown tier is caught",
        run_all.header_field('"""\nTIER: nightly\n"""', "TIER") not in run_all.TIER_ORDER,
    )
    # Rule 6 is measured rather than text-matched, so its self test builds a
    # test project tree: the same asset in two test projects is the finding, in one is not.
    with tempfile.TemporaryDirectory() as scratch:
        tree = Path(scratch)
        for name in ("native", "scene"):
            assets = tree / name / "res"
            assets.mkdir(parents=True)
            (assets / "map.png").write_bytes(b"p" * (TEST_PROJECT_DUPLICATE_MINIMUM_BYTES * 2))
        expect(
            "the same asset in two test projects is caught",
            len(project_set_findings(tree)[1]) == 1,
        )
        (tree / "scene" / "res" / "map.png").unlink()
        expect("an asset in one test project passes", project_set_findings(tree)[1] == [])
        # The build output a run leaves behind is not the test project's weight.
        (tree / "native" / "target").mkdir()
        (tree / "native" / "target" / "big.bin").write_bytes(b"b" * 4096)
        expect(
            "build output is not counted",
            project_set_findings(tree)[0] == TEST_PROJECT_DUPLICATE_MINIMUM_BYTES * 2,
        )
    expect("the real suites pass the header rule", header_findings() == [])
    print("PASS: self test." if failures == 0 else f"FAIL: {failures} self test case(s).")
    return 0 if failures == 0 else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="Lint the devops test suites' own rules.")
    parser.add_argument("--self-test", action="store_true", help="check the checker itself")
    arguments = parser.parse_args()
    if arguments.self_test:
        return self_test()

    failures = 0
    for title, findings in collect():
        print(f"  [{'FAIL' if findings else 'PASS'}] {title}")
        for finding in findings:
            print(f"         {finding}")
        failures += len(findings)
    if failures:
        print(f"FAIL: {failures} suite-standards violation(s).")
        return 1
    print("PASS: every suite follows the suite rules.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
