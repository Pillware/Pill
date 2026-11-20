#!/usr/bin/env python3
"""
Discovery of example projects: the local checkout, the curated official set and
the extension glob.

PROTECTS: The rule that finds example projects, so an extension's example is discovered without an edit and never silently skipped.
TIER: static

REQUIREMENTS: Python 3.11+ (standard library only).

DESCRIPTION
    `devops/tests/test_examples.py` builds every project `example_project_roots()`
    returns. That makes the function the one place a project can go missing: a
    project the walk does not reach is not built, not run, and nothing fails -
    the suite simply has less to do and still exits 0.

    The extension half of the rule is the newest and the least obvious, because
    nothing named in `devops/` points at it: a module ships
    `modules/extensions/<extension>/examples/<name>/`, and the walk finds it. The
    checks below pin that rule against a temporary tree, so they hold whether or
    not an extension currently ships an example, and they pin the real tree
    against handing out a path that does not exist.

    They also pin the curated catalogue: it selects a subset of `examples/` on
    purpose, so a check that every entry exists catches a rename that would
    otherwise only surface as a suite failing to launch a project minutes later.

USAGE
    python devops/tests/test_example_discovery.py

EXAMPLE USAGE
    python devops/tests/test_example_discovery.py
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

SUITES_DIRECTORY = Path(__file__).resolve().parent
DEVOPS_ROOT = SUITES_DIRECTORY.parent
if str(DEVOPS_ROOT) not in sys.path:
    sys.path.insert(0, str(DEVOPS_ROOT))

from core.projects import (  # noqa: E402
    OFFICIAL_EXAMPLE_PROJECTS,
    PROJECT_SETTINGS_FILENAME,
    example_project_roots,
    extension_example_roots,
    project_settings_yaml,
)


def check(condition: bool, message: str, failures: list[str]) -> None:
    """Records one failed expectation, printing it as it happens."""
    if not condition:
        failures.append(message)
        print(f"  FAIL  {message}")


def build_extension_tree(root: Path) -> None:
    """Creates the temporary extension tree the discovery checks run against.

    Covers the four shapes the rule has to tell apart: a shipped example, a
    directory under `examples/` that is not a project, an extension with no
    `examples/` folder, and an extension whose `examples/` folder is empty.
    """
    (root / "audio" / "examples" / "demo").mkdir(parents=True)
    project_settings_yaml(root / "audio" / "examples" / "demo").write_text(
        'name: "Demo"\n', encoding="utf-8"
    )
    (root / "audio" / "examples" / "not_a_project").mkdir()
    (root / "spline").mkdir()
    (root / "empty" / "examples").mkdir(parents=True)


def check_extension_glob(failures: list[str]) -> None:
    """Checks the extension glob against a temporary tree."""
    with tempfile.TemporaryDirectory() as temporary_directory:
        root = Path(temporary_directory)
        build_extension_tree(root)

        discovered = extension_example_roots(root)
        check(
            discovered == [root / "audio" / "examples" / "demo"],
            f"the glob finds exactly the shipped example, got {discovered}",
            failures,
        )

        missing_root = extension_example_roots(root / "does_not_exist")
        check(missing_root == [], f"a missing root yields nothing, got {missing_root}", failures)

        (root / "spline" / "examples" / "second" ).mkdir(parents=True)
        project_settings_yaml(root / "spline" / "examples" / "second").write_text(
            'name: "Second"\n', encoding="utf-8"
        )
        discovered = extension_example_roots(root)
        check(
            [path.name for path in discovered] == ["demo", "second"],
            f"two extensions are discovered in sorted order, got {[path.name for path in discovered]}",
            failures,
        )


def check_real_tree(failures: list[str]) -> None:
    """Checks the discovery against the checkout this suite runs in."""
    discovered = example_project_roots()
    check(bool(discovered), "the real tree yields at least one example project", failures)

    missing = [path for path in discovered if not (path / PROJECT_SETTINGS_FILENAME).is_file()]
    check(
        not missing,
        f"every discovered root holds a {PROJECT_SETTINGS_FILENAME}, missing: "
        f"{[str(path) for path in missing]}",
        failures,
    )

    extension_examples = extension_example_roots()
    unchecked = [path for path in extension_examples if path not in discovered]
    check(
        not unchecked,
        f"every extension example reaches example_project_roots, missing: "
        f"{[str(path) for path in unchecked]}",
        failures,
    )


def check_catalogue(failures: list[str]) -> None:
    """Checks that every curated official entry still names a project on disk."""
    missing = [
        name
        for name, path in OFFICIAL_EXAMPLE_PROJECTS.items()
        if not (path / PROJECT_SETTINGS_FILENAME).is_file()
    ]
    check(
        not missing,
        f"every curated official example exists on disk, missing: {missing}",
        failures,
    )


def main() -> int:
    """Runs the discovery checks and returns the suite's exit code."""
    failures: list[str] = []
    check_extension_glob(failures)
    check_real_tree(failures)
    check_catalogue(failures)
    if failures:
        return 1
    print(
        "  PASS  example discovery: the extension glob finds a shipped example and "
        "ignores a bare directory, the real tree hands out no missing project, and "
        "every curated official entry exists"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
