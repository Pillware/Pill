#!/usr/bin/env python3
"""
Keeps the moved devops tooling's shared core copy in step with the tracked one.

REQUIREMENTS
  - Python 3.8+.

DESCRIPTION
    The measurement tooling under `local/devops` (gitignored) imports a `core`
    package beside it. Its measurement modules are its own; the six shared
    modules are generated copies of `devops/core`, so there is one source of
    truth for the paths, projects, suite plumbing, scenario runner and package
    docstring.

    `paths.py` is not copied byte for byte: the copy lives two directories
    below the repository root, so its root derivation is rewritten to walk up
    to the repository and to point `DEVOPS_ROOT` at the tracked tree. Every
    other shared module is copied unchanged.

    `--check` regenerates into memory and fails when the copy differs, so a
    stale copy is reported by the first tool that runs it instead of importing
    the wrong roots.

USAGE
  python devops/tools/sync_local_devops_core.py [--check]

EXAMPLE USAGE
  python devops/tools/sync_local_devops_core.py          # refresh the copy
  python devops/tools/sync_local_devops_core.py --check  # verify it is current

  Exit status: 0 when the copy is current, 1 when it differs or the tracked
  source changed shape, 2 on a usage error.

--- SCRIPT ---
"""

import sys
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
SOURCE_ROOT = REPOSITORY_ROOT / "devops" / "core"
TARGET_ROOT = REPOSITORY_ROOT / "local" / "devops" / "core"

# The shared modules the moved tooling imports. The measurement modules beside
# them (storage, compare, cli, criterion, cargo_timings and the measurement
# helpers) belong to the local tree and are never touched here.
SHARED_MODULES = (
    "__init__.py",
    "environment.py",
    "paths.py",
    "projects.py",
    "scenario.py",
    "suite_common.py",
)

# The root derivation `paths.py` carries in the tracked package, and the one
# the copy needs: two directories further up is the repository root.
TRACKED_ROOT_LINES = (
    "# devops/core/paths.py -> devops\n"
    "DEVOPS_ROOT = Path(__file__).resolve().parent.parent\n"
    "# devops -> repository root\n"
    "REPOSITORY_ROOT = DEVOPS_ROOT.parent\n"
)
COPIED_ROOT_LINES = (
    "# Generated copy: this file lives in local/devops/core, so the repository\n"
    "# root is two directories further up and DEVOPS_ROOT names the tracked tree.\n"
    "LOCAL_DEVOPS_ROOT = Path(__file__).resolve().parent.parent\n"
    "REPOSITORY_ROOT = LOCAL_DEVOPS_ROOT.parent.parent\n"
    'DEVOPS_ROOT = REPOSITORY_ROOT / "devops"\n'
)


def generated_text(name: str, source: str) -> str:
    """Returns the content the copy of `name` must have."""
    if name != "paths.py":
        return source
    # Match on LF text whatever the tracked file's endings are, then restore
    # the source's endings so the copy mirrors it byte for byte otherwise.
    line_ending = "\r\n" if "\r\n" in source else "\n"
    normalized = source.replace("\r\n", "\n")
    if TRACKED_ROOT_LINES not in normalized:
        raise SystemExit(
            "devops/core/paths.py no longer derives its roots with the block "
            "this tool rewrites; update sync_local_devops_core.py"
        )
    generated = normalized.replace(TRACKED_ROOT_LINES, COPIED_ROOT_LINES, 1)
    return generated.replace("\n", "\r\n") if line_ending == "\r\n" else generated


def main() -> int:
    """Refreshes the copy, or reports whether it is current under `--check`."""
    arguments = sys.argv[1:]
    check_only = "--check" in arguments
    if any(argument != "--check" for argument in arguments):
        print(f"usage: {Path(__file__).name} [--check]", file=sys.stderr)
        return 2
    stale = []
    for name in SHARED_MODULES:
        # Newline-preserving reads and writes: the copy must match the tracked
        # file byte for byte, and pathlib's defaults would translate endings.
        with (SOURCE_ROOT / name).open("r", encoding="utf-8", newline="") as handle:
            source = handle.read()
        expected = generated_text(name, source)
        target = TARGET_ROOT / name
        current = None
        if target.is_file():
            with target.open("r", encoding="utf-8", newline="") as handle:
                current = handle.read()
        if current == expected:
            continue
        if check_only:
            stale.append(name)
            continue
        with target.open("w", encoding="utf-8", newline="") as handle:
            handle.write(expected)
        print(f"updated {target.relative_to(REPOSITORY_ROOT).as_posix()}")
    if check_only:
        if stale:
            print(
                "the local core copy is stale: "
                + ", ".join(stale)
                + "; run python devops/tools/sync_local_devops_core.py",
                file=sys.stderr,
            )
            return 1
        print("local core copy is current")
        return 0
    print("local core copy refreshed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
