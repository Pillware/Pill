"""
One reader for the Cargo manifests the architecture guards check.

REQUIREMENTS
  - Python 3.11+ (`tomllib`)

DESCRIPTION
    Three guards used to carry their own manifest reader: the engine-layout
    guard resolved path dependencies by hand, the renderer-boundary guard
    expanded the workspace member globs itself, and the feature-drift guard
    read module manifests with a regex. One reader means one place that gets
    renamed packages, `[target.'cfg(...)'.*]` tables and workspace globs
    right.

    Used by:
      * `devops/tests/test_engine_layout.py`
      * `devops/tests/test_renderer_boundaries.py`
      * `devops/tests/test_engine_feature_drift.py`

    Everything here answers questions about the repository's manifests. None
    of the rule sets live here: each guard keeps its own constants and
    checks, and reads them through these primitives.

USAGE
  from core.workspace_graph import Crate, example_projects, workspace_members

  crate = Crate(MODULES / "engine" / "pill_core" / "Cargo.toml")
  for dependency in crate.linked():
      print(dependency.package, dependency.kind, dependency.directory)

--- SCRIPT ---
"""

import tomllib
from pathlib import Path
from typing import Dict, List, NamedTuple, Optional

from .paths import REPOSITORY_ROOT
from .projects import (
    EXAMPLES_RELATIVE_PATH,
    OFFICIAL_EXAMPLE_PROJECTS,
    TEST_PROJECT_NAMES,
    TEST_PROJECTS_ROOT,
)


class ManifestError(Exception):
    """A manifest that could not be read or parsed."""


# Reads one Cargo.toml into a dictionary.
def read_manifest(path: Path) -> dict:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ManifestError(f"{path}: {error}") from error


class Dependency(NamedTuple):
    """One dependency edge of a crate.

    `key` is the name the manifest writes, `package` the package it actually
    names (they differ when a renamed dependency is declared), and `kind` is
    "normal", "dev" or "build". `directory` is the resolved directory of a
    path dependency and `None` for a registry or git one.
    """

    key: str
    package: str
    kind: str
    directory: Optional[Path]


class Crate:
    """One crate manifest: its package name, directory and dependency edges.

    The name falls back to the directory name for a manifest without a
    `[package]` table, which is how a glob member that is not a crate (the
    workspace anchors) still reads as something recognisable.
    """

    def __init__(self, manifest_path: Path):
        self.manifest_path = manifest_path
        self.directory = manifest_path.parent
        self.manifest = read_manifest(manifest_path)
        self.name = self.manifest.get("package", {}).get("name", self.directory.name)

    # Every dependency edge, from `[dependencies]`, `[dev-dependencies]`,
    # `[build-dependencies]` and their `[target.<cfg>.*]` forms. Path
    # dependencies are resolved against this manifest's directory.
    def dependencies(self) -> List[Dependency]:
        tables = [self.manifest]
        for target in self.manifest.get("target", {}).values():
            tables.append(target)
        found = []
        for table in tables:
            for section, kind in (
                ("dependencies", "normal"),
                ("dev-dependencies", "dev"),
                ("build-dependencies", "build"),
            ):
                for key, value in table.get(section, {}).items():
                    if isinstance(value, dict):
                        package = value.get("package", key)
                        path = value.get("path")
                        directory = (self.directory / path).resolve() if path else None
                    else:
                        package, directory = key, None
                    found.append(Dependency(key, package, kind, directory))
        return found

    # The edges a build links into the crate itself: normal and build
    # dependencies. Dev-dependencies are left out, they build tests and never
    # the library.
    def linked(self) -> List[Dependency]:
        return [dependency for dependency in self.dependencies() if dependency.kind != "dev"]

    # The crate's own feature names, in declaration order, `default` included.
    def declared_features(self) -> List[str]:
        return list(self.manifest.get("features", {}))


# The workspace members under `modules/`, expanding glob members as cargo does
# and skipping directories under an `exclude`d path.
#
# An explicitly listed member is kept even when an `exclude` covers a folder
# above it; cargo does the same, and the master renderer's layout relies on it:
# `extensions/pill_master_renderer/modules/runtime` is excluded so the
# `extensions/*/modules/*` glob does not take it for a crate, while its `data`
# and `gpu` crates are listed as members.
def workspace_members(modules: Path) -> List[Crate]:
    workspace = read_manifest(modules / "Cargo.toml")["workspace"]
    excluded = [(modules / path).resolve() for path in workspace.get("exclude", [])]
    members = []
    seen = set()
    for member in workspace.get("members", []):
        is_glob = any(character in member for character in "*?[")
        directories = sorted(modules.glob(member)) if is_glob else [modules / member]
        for directory in directories:
            manifest = directory / "Cargo.toml"
            resolved = directory.resolve()
            if is_glob and any(resolved == path or path in resolved.parents for path in excluded):
                continue
            if not manifest.is_file() or resolved in seen:
                continue
            seen.add(resolved)
            members.append(Crate(manifest))
    return members


# The example and test projects: every `<examples>/*/Cargo.toml` in the local
# checkout, every official project the catalog names, and every test project under
# `devops/tests/projects`. A project without a manifest is a managed-language
# one and has no cargo tree to measure.
#
# The local checkout uses both layouts: a project directory, and a group
# directory holding one Rust and one C# project (as `pills_awakens` does), so
# the manifest glob reaches one level deeper than the folder itself.
def example_projects(repository: Path) -> List[Crate]:
    examples = repository / EXAMPLES_RELATIVE_PATH
    manifests = sorted(
        set(examples.glob("*/Cargo.toml")) | set(examples.glob("*/*/Cargo.toml"))
    )
    # The catalog's paths are re-rooted under `repository` and then checked
    # there, not on the real checkout: a tree that carries only what it was
    # generated with - the renderer-boundary suite's self test builds one - has
    # neither the official examples nor the test projects, and a path a tree does not
    # hold is not one of its projects.
    catalog = [
        path.relative_to(REPOSITORY_ROOT) for path in OFFICIAL_EXAMPLE_PROJECTS.values()
    ] + [TEST_PROJECTS_ROOT.relative_to(REPOSITORY_ROOT) / name for name in TEST_PROJECT_NAMES]
    manifests.extend(
        manifest
        for manifest in (repository / relative / "Cargo.toml" for relative in catalog)
        if manifest.is_file()
    )
    return [Crate(manifest) for manifest in sorted(set(manifests))]
