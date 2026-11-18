#!/usr/bin/env python3
"""
Engine feature drift check: every build the development hosts run resolves the
engine dylibs' dependency trees exactly as the host does.

PROTECTS: Every build the host spawns resolves the same engine features as the host itself.
TIER: static

REQUIREMENTS: Python 3.8+, cargo on PATH (runs `cargo tree --offline`).

DESCRIPTION
    `pill_core.dll` and `pill_engine_core.dll` are loaded once per process and
    imported by the host, every module and the project. The names they export
    hash the features their dependency tree resolved with, so a module whose
    build resolved one crate in that tree with a different feature set imports
    names the host's copy does not export, and fails to load with "procedure
    could not be found".

    For each host posture - one per key of `FRONTENDS` - this resolves each
    shared dylib's subtree for the host alone,
    then for every build a host spawns for a project: each extension's module
    build - the crate selected directly with the feature set its generated
    `host_module_<name>` wrapper would enable - the project's own build, and
    the project's startup batch, each plus the engine features
    `build_runner::host_engine_features` mirrors from the running host. Any
    crate whose features differ from the host's own resolution is reported.
    A posture is measured only against the dylibs named in its row; every
    posture today loads every shared dylib.

    What is measured comes from tracked sources only - the extension crates by
    their `extensions/<folder>/<crate>` convention, the projects from their
    `project_settings.yaml` and manifests - never from the generated
    `host_module_*` / `host_project_*` crates the host leaves behind, which are
    gitignored and pruned between runs. A fresh clone and a used checkout
    therefore measure the same builds, and the count is printed so a drop is
    visible.

    Every posture receives its engine features explicitly, exactly like the
    standalone builders. An editor host instead anchors its module builds to
    its own package, because only that package's graph reproduces the renderer
    and UI feature unions onto the engine dylibs; an anchored build ignores the
    engine features.

    Extensions are found by convention (every `extensions/<folder>/<crate>`
    crate); the projects are every example with both a `project_settings.yaml`
    and a `Cargo.toml`.

USAGE
  python devops/tests/test_engine_feature_drift.py [--verbose]
    --verbose  list the builds behind each reported difference

EXAMPLE USAGE
  python devops/tests/test_engine_feature_drift.py

  Exit status: 0 when no build drifts, 1 when one does, 2 when cargo tree fails.

--- SCRIPT ---
"""

import re
import subprocess
import sys
from pathlib import Path

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.workspace_graph import Crate, example_projects  # noqa: E402

REPOSITORY = Path(__file__).resolve().parents[2]
MODULES = REPOSITORY / "modules"

# The shared engine libraries whose subtrees must resolve identically in every
# posture: the engine's, and the plain draw data the UI library and the
# renderer contract share.
#
# `pill_project_ui` is deliberately absent: projects that draw their own egui link
# it, modules and the renderer do not, so comparing it against a posture that
# never loads it reports drift nothing can act on - the same reason the
# editor's dylibs are compared per editor family below rather than here.
ENGINE_DYLIBS = (
    "pill_core",
    "pill_engine_core",
    "pill_ui_frame",
)

# The egui editor's shared dylibs: the egui boundary and its model, and the
# theme pipeline's plain data it links. Only the postures that load
# egui editor parts resolve them.
EGUI_EDITOR_DYLIBS = (
    "pill_editor_core",
    "pill_ui_theme",
)

# A posture is measured only against the dylibs it loads: the standalone
# postures resolve the engine dylibs alone, and the editor adds its own parts.
ALL_DYLIBS = ENGINE_DYLIBS + EGUI_EDITOR_DYLIBS

# The host postures module and project builds run under: the host's own cargo
# selection, the engine features `build_runner::host_engine_features`
# mirrors from a game host (the engine-affecting features `pill_development_host`
# declares: `development`, the profiling levels, `metrics`, `dev-logs`), and the
# cargo anchor a posture appends to every spawned build
# (`build_runner::apply_cargo_host_overrides`). The editor host anchors its
# module builds to `pill_editor` with whichever of the posture features that
# package declares - today the profiling pair, when the running binary was
# built with one; an anchored build ignores the engine features.
FRONTENDS = {
    "headless": (
        ["--package", "pill_standalone"],
        ["pill_engine/development"],
        None,
        ALL_DYLIBS,
    ),
    "headless reload-only": (
        ["--package", "pill_standalone", "--no-default-features", "--features", "development"],
        ["pill_engine/development"],
        None,
        ALL_DYLIBS,
    ),
    "windowed": (
        ["--package", "pill_standalone", "--features", "pill_standalone/rendering"],
        ["pill_engine/development"],
        None,
        ALL_DYLIBS,
    ),
    "editor": (
        ["--package", "pill_editor"],
        [],
        ("pill_editor", ()),
        # The editor loads every shared dylib, including its own parts.
        ALL_DYLIBS,
    ),
    # The editor built with Tracy on, which is also the posture its module and
    # project builds inherit from the anchor: every spawned build names the
    # same feature the running binary was built with.
    "editor profiling": (
        ["--package", "pill_editor", "--features", "pill_editor/profiling"],
        [],
        ("pill_editor", ("profiling",)),
        ALL_DYLIBS,
    ),
}

# One `cargo tree --prefix depth -f "{p}|{f}"` line: depth, package, features.
TREE_LINE = re.compile(r"^(\d+)(\S+ v\S+)(?: \([^)]*\))?(?: \(\*\))?\|(.*)$")


class TreeError(Exception):
    """A `cargo tree` invocation that failed."""


# Resolves `selection` together with `dylib`; returns {package: features} for
# the dylib's own subtree, the dylib included.
def dylib_tree(dylib, selection):
    # `--no-dedupe`: cargo prints a repeated subtree once and marks later
    # occurrences `(*)` with no children, which would hide part of the dylib's
    # subtree whenever another selected root printed it first.
    completed = subprocess.run(
        ["cargo", "tree", "--offline", "--no-dedupe", "-e", "normal", "--prefix", "depth", "-f", "{p}|{f}",
         "--package", dylib, *selection],
        cwd=str(MODULES), capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    if completed.returncode != 0:
        raise TreeError(f"cargo tree failed for {dylib} with {selection}:\n{completed.stderr[-800:]}")
    packages, inside = {}, False
    for line in completed.stdout.splitlines():
        match = TREE_LINE.match(line.strip())
        if not match:
            continue
        depth, package, features = int(match.group(1)), match.group(2), match.group(3)
        if depth == 0:
            inside = package.startswith(f"{dylib} v")
        if inside:
            packages.setdefault(package, set()).update(feature for feature in features.split(",") if feature)
    return packages


# Every extension crate built as a loadable module: a crate in an extension
# folder, `extensions/<folder>/<crate>/` (`extensions/rendering` holds the old
# renderers, excluded from the workspace; `_`-folders are bookkeeping). Mapped
# to the folder that holds it.
def module_folders():
    folders = {}
    for manifest in sorted(MODULES.glob("extensions/*/*/Cargo.toml")):
        folder = manifest.parent.parent.name
        name = manifest.parent.name
        if folder == "rendering" or folder.startswith("_") or name == "generated":
            continue
        folders[name] = folder
    return folders


# The names of every extension crate, in name order.
def module_names():
    return sorted(module_folders())


# The extension's manifest, read through the shared reader.
def module_crate(name):
    return Crate(MODULES / "extensions" / module_folders()[name] / name / "Cargo.toml")


# The feature names an extension declares, minus the `default` key: the set a
# generated wrapper enables on its dependency edge. The wrapper file is never
# read - only what it would carry.
def wrapper_features(name):
    return [feature for feature in module_crate(name).declared_features() if feature != "default"]


# Every Rust example project the host can run, in directory order: an example
# with a settings file and a manifest. A managed project (a `.csproj`) has the
# settings but no manifest, and no cargo build to measure.
def rust_projects():
    return [
        crate
        for crate in example_projects(REPOSITORY)
        if (crate.directory / "project_settings.yaml").is_file()
    ]


# The modules a project's settings declare, in order.
#
# The file is flat and hand-written; a YAML parser would be a dependency for
# one list of names. Both forms are read: the block one (`modules:` followed by
# `- "name"` lines) and the inline one (`modules: ["name"]`, `modules: []`).
def project_modules(crate):
    modules = []
    inside = False
    settings = crate.directory / "project_settings.yaml"
    for line in settings.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("modules:"):
            rest = stripped[len("modules:") :].strip()
            modules.extend(re.findall(r'"([^"]+)"', rest))
            inside = rest == ""
            continue
        if not inside or not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("-"):
            modules.append(stripped[1:].strip().strip('"'))
        else:
            inside = False
    return modules


# The engine-feature flags a spawned build carries.
def engine_selection(engine_features):
    return ["--features", ",".join(engine_features)] if engine_features else []


# The extra `--package`/`--features` a posture appends to every build when it
# keeps a cargo anchor (`build_runner::apply_cargo_host_overrides`), without
# its defaults and with the features the running host was built with. One
# helper, so the build shapes below cannot drift apart on it. A posture whose
# anchor tuple names no features (the unprofiled editor) emits no `--features`
# at all.
def anchor_selection(anchor):
    if not anchor:
        return []
    anchor_package, anchor_features = anchor
    selection = ["--package", anchor_package, "--no-default-features"]
    if anchor_features:
        selection += [
            "--features",
            ",".join(f"{anchor_package}/{feature}" for feature in anchor_features),
        ]
    return selection


# The module build of `name`: the crate selected directly, with the features
# its generated wrapper would enable and the engine features the host passes.
# Measured through the crate, not the wrapper: the wrapper is gitignored and
# exists only after the host's first start, the two resolve the same graph, and
# only this shape exists on every checkout.
def module_selection(name, engine_features, anchor):
    features = [f"{name}/{feature}" for feature in wrapper_features(name)]
    features += engine_features
    selection = ["--package", name]
    if features:
        selection += ["--features", ",".join(features)]
    return selection + anchor_selection(anchor)


# The project build as the host runs it: `cargo build --package
# host_project_<package>`. The member's own resolution is taken from the
# example's manifest - the crates under `modules/extensions/` it names
# directly, each with the features it asks of them - which is the same graph
# the generated member carries, minus the member package itself, and with the
# manifest's per-edge `default-features = false` not applied: no module
# declares defaults today, so the two agree; a module that gains them would
# surface here as drift, which is what this guard exists to report.
def project_selection(crate, engine_features, anchor):
    selection = []
    features = []
    for key, value in crate.manifest.get("dependencies", {}).items():
        if not isinstance(value, dict) or "modules/extensions/" not in value.get("path", ""):
            continue
        selection += ["--package", key]
        features += [f"{key}/{feature}" for feature in value.get("features", [])]
    features += engine_features
    if features:
        selection += ["--features", ",".join(features)]
    return selection + anchor_selection(anchor)


# The builds a host running `frontend` spawns, as {label: cargo selection}.
#
# Every one is derived from tracked sources: the extension crates by the
# `extensions/<folder>/<crate>` convention, the projects and their startup
# batches from the project settings. Nothing reads a generated `host_module_*`
# / `host_project_*` crate, so a fresh clone measures exactly what a used
# checkout measures.
#
# The startup batch invocation selects, in one cargo call, every module the
# host will load plus the project member (see `build_runner::build_extension_batch`),
# so its incremental content over the per-module builds is the feature
# unification across that whole selection. Measured per project, from its
# settings, as the union of that project's modules; the member's own graph is
# the project build above.
def spawned_builds(frontend):
    _, engine_features, anchor, _ = FRONTENDS[frontend]
    builds = {}
    for module in module_names():
        builds[f"module {module}"] = module_selection(module, engine_features, anchor)
    for crate in rust_projects():
        selection = project_selection(crate, engine_features, anchor)
        # A project that names no extension crate selects no package of its own:
        # it is not a workspace member, and the engine features alone are a
        # `pill_engine/...` spec cargo cannot resolve against a dylib that has no
        # dependency on the facade. Such a project spawns no module build, so
        # there is no graph to compare.
        if "--package" in selection:
            builds[f"project {crate.directory.name}"] = selection
        modules = project_modules(crate)
        if modules:
            selection = []
            features = []
            for module in modules:
                selection += ["--package", module]
                features += [f"{module}/{feature}" for feature in wrapper_features(module)]
            features += engine_features
            if features:
                selection += ["--features", ",".join(features)]
            builds[f"batch {crate.directory.name}"] = selection + anchor_selection(anchor)
    return builds


# The differences between a build's subtree and the frontend's own, as
# {package: description}.
def differences(reference, measured):
    found = {}
    for package in sorted(set(reference) | set(measured)):
        if package not in reference:
            found[package] = "joins the tree"
        elif package not in measured:
            found[package] = "leaves the tree"
        elif reference[package] != measured[package]:
            gained = sorted(measured[package] - reference[package])
            lost = sorted(reference[package] - measured[package])
            parts = ([f"+{', +'.join(gained)}"] if gained else []) + ([f"-{', -'.join(lost)}"] if lost else [])
            found[package] = " ".join(parts)
    return found


# The packages `build_runner::is_editor_anchor` treats as an editor anchor.
# Read from the Rust source, so a posture anchored to a package the host does
# not recognise fails here instead of measuring a shape the host never builds.
def editor_anchor_packages():
    source = (
        MODULES / "hosting" / "pill_development_host" / "src" / "build_runner.rs"
    ).read_text(encoding="utf-8")
    match = re.search(
        r"fn is_editor_anchor\([^)]*\)[^{]*\{(.*?)\}", source, re.DOTALL
    )
    if match is None:
        raise TreeError("is_editor_anchor is gone from build_runner.rs")
    return set(re.findall(r'"([\w-]+)"', match.group(1)))


def main():
    verbose = "--verbose" in sys.argv
    anchor_packages = editor_anchor_packages()
    for frontend, (_, _, anchor, _) in FRONTENDS.items():
        if anchor and anchor[0] not in anchor_packages:
            print(
                f"error: {frontend} anchors to {anchor[0]}, which "
                "build_runner::is_editor_anchor does not recognise",
                file=sys.stderr,
            )
            return 2
    drift = {}
    measured_count = 0
    try:
        for frontend, (selection, _, _, dylibs) in FRONTENDS.items():
            for dylib in dylibs:
                reference = dylib_tree(dylib, selection)
                for label, build in spawned_builds(frontend).items():
                    measured_count += 1
                    for package, description in differences(reference, dylib_tree(dylib, build)).items():
                        drift.setdefault((dylib, package, description), []).append(f"{label} ({frontend})")
    except TreeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(f"modules: {', '.join(module_names())}")
    print(f"projects: {', '.join(crate.directory.name for crate in rust_projects()) or 'none'}")
    print(f"builds measured: {measured_count}")
    if verbose:
        for frontend, (_, _, anchor, _) in FRONTENDS.items():
            if anchor:
                print(f"  anchor {frontend}: {' '.join(anchor_selection(anchor))}")
    for (dylib, package, description), builds in sorted(drift.items()):
        print(f"  FAIL  {dylib}: {package} {description} ({len(builds)} build(s))")
        if verbose:
            print(f"        from: {', '.join(builds)}")
    if drift:
        return 1
    print(f"  PASS  {', '.join(ALL_DYLIBS)} resolve identically in every spawned build")
    return 0


if __name__ == "__main__":
    sys.exit(main())
