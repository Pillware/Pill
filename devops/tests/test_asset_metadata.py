"""
Asset metadata consistency check for Rust-Hybrid-ECS.

PROTECTS: Every .meta under each project's res directory has its asset, and no two assets share a guid.
TIER: static

REQUIREMENTS
  - Python 3.8+ (standard library only)

DESCRIPTION
    Every source asset in a project's `res` can carry a `<file>.meta` sidecar
    holding its guid. Two mistakes break that silently, and only at runtime:

      * an orphaned `.meta` - its source was moved or deleted without it, so
        the guid no longer belongs to any file and a moved source gets a new
        guid on its next import;
      * two `.meta` files with one guid - a source copied together with its
        `.meta`, which the engine refuses to import (`DuplicateGuid`).

    This check scans every project's `res` directory under the configured
    examples root and fails on either, naming the files.
    Standalone assets (`*.material`, `*.render_pass`, ...) hold their guid in
    their own header and have no `.meta`; their guids are checked for
    duplicates too, against each other and against the sidecars.

    `--self-test` first proves, on scratch directories, that the check still
    catches each mistake and accepts a clean tree.

USAGE
  python devops/tests/test_asset_metadata.py [--self-test] [--root <dir> ...]

  --self-test   run the checker against generated broken and clean trees
  --root DIR    check DIR (a `res` directory) instead of every project's

EXAMPLE USAGE
  python devops/tests/test_asset_metadata.py
  python devops/tests/test_asset_metadata.py --self-test

--- SCRIPT ---
"""

import argparse
import sys
import tempfile
from pathlib import Path
from typing import Dict, List, Optional, Sequence

import yaml

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPOSITORY_ROOT / "devops"))

from core.projects import project_roots  # noqa: E402

METADATA_SUFFIX = ".meta"


def read_guid(path: Path) -> Optional[str]:
    """The `guid` in a YAML asset header, or None when the file has none."""
    try:
        document = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, yaml.YAMLError):
        return None
    guid = document.get("guid") if isinstance(document, dict) else None
    return guid.lower() if isinstance(guid, str) else None


def is_standalone_document(path: Path) -> bool:
    """Whether `path` is a standalone asset: a YAML file with an asset header.

    Recognized by content rather than by a list of extensions, so a standalone
    type added later is covered without editing this check.
    """
    if path.name.endswith(METADATA_SUFFIX) or path.suffix.lower() in (".json", ""):
        return False
    try:
        if path.stat().st_size > 1_000_000:
            return False
        document = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, yaml.YAMLError):
        return False
    return isinstance(document, dict) and {"format_version", "asset_type", "guid"} <= document.keys()


def check_root(root: Path) -> List[str]:
    """Every problem found under one `res` directory, as readable lines."""
    problems: List[str] = []
    owners: Dict[str, List[Path]] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        if path.name.endswith(METADATA_SUFFIX):
            source = path.with_name(path.name[: -len(METADATA_SUFFIX)])
            if not source.is_file():
                problems.append(f"orphaned metadata: {path} has no source ({source.name})")
            guid = read_guid(path)
            if guid is None:
                problems.append(f"unreadable metadata: {path} holds no guid")
                continue
            owners.setdefault(guid, []).append(path)
        elif is_standalone_document(path):
            guid = read_guid(path)
            if guid is not None:
                owners.setdefault(guid, []).append(path)
    for guid, paths in sorted(owners.items()):
        if len(paths) > 1:
            listed = ", ".join(str(path) for path in paths)
            problems.append(f"duplicate guid {guid}: {listed}")
    return problems


def default_roots() -> List[Path]:
    """Every example and test project's `res` directory."""
    return sorted(
        root / "res" for root in project_roots() if (root / "res").is_dir()
    )


def check(roots: Sequence[Path]) -> int:
    """Check every root and print the result; the process exit code."""
    problems = [problem for root in roots for problem in check_root(root)]
    for problem in problems:
        print(f"  [FAIL] {problem}")
    checked = sum(1 for root in roots for _ in root.rglob(f"*{METADATA_SUFFIX}"))
    if problems:
        print(f"Asset metadata: {len(problems)} problem(s) in {len(roots)} res directories.")
        return 1
    print(f"Asset metadata: OK ({checked} .meta files in {len(roots)} res directories).")
    return 0


def self_test() -> int:
    """Prove the check catches an orphan and a duplicate, and accepts a clean tree."""
    header = {"format_version": 1, "asset_type": "test::Asset", "settings": {}}

    def write_meta(path: Path, guid: str) -> None:
        path.write_text(
            yaml.safe_dump(dict(header, guid=guid), sort_keys=False),
            encoding="utf-8",
        )

    cases = []
    with tempfile.TemporaryDirectory(prefix="pill-asset-metadata-") as scratch:
        base = Path(scratch)

        clean = base / "clean"
        (clean / "textures").mkdir(parents=True)
        (clean / "textures/a.png").write_bytes(b"x")
        write_meta(clean / "textures/a.png.meta", "a" * 32)
        (clean / "b.material").write_text(
            yaml.safe_dump(dict(header, guid="b" * 32), sort_keys=False), encoding="utf-8"
        )
        cases.append(("a clean tree passes", clean, None))

        orphan = base / "orphan"
        orphan.mkdir()
        write_meta(orphan / "moved.png.meta", "c" * 32)
        cases.append(("an orphaned .meta fails", orphan, "orphaned metadata"))

        duplicate = base / "duplicate"
        duplicate.mkdir()
        for name in ("one.png", "two.png"):
            (duplicate / name).write_bytes(b"x")
            write_meta(duplicate / f"{name}.meta", "d" * 32)
        cases.append(("two .meta files with one guid fail", duplicate, "duplicate guid"))

        standalone = base / "standalone"
        standalone.mkdir()
        (standalone / "a.png").write_bytes(b"x")
        write_meta(standalone / "a.png.meta", "e" * 32)
        (standalone / "copy.material").write_text(
            yaml.safe_dump(dict(header, guid="e" * 32), sort_keys=False), encoding="utf-8"
        )
        cases.append(("a standalone asset sharing a sidecar's guid fails", standalone, "duplicate guid"))

        failures = 0
        for label, root, expected in cases:
            problems = check_root(root)
            if expected is None:
                passed = not problems
            else:
                passed = any(expected in problem for problem in problems)
            print(f"  [{'OK' if passed else 'FAIL'}] self-test: {label}")
            if not passed:
                print(f"         problems reported: {problems}")
                failures += 1
    return 1 if failures else 0


def main() -> int:
    """Parse the arguments and run the self-test or the check."""
    parser = argparse.ArgumentParser(description="Check .meta files for orphans and duplicate guids.")
    parser.add_argument("--self-test", action="store_true", help="test the checker itself")
    parser.add_argument("--root", action="append", type=Path, default=[], help="a res directory to check")
    arguments = parser.parse_args()
    if arguments.self_test:
        return self_test()
    return check(arguments.root or default_roots())


if __name__ == "__main__":
    sys.exit(main())
