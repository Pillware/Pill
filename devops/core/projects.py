"""
Project and path configuration shared by every devops suite and gate step.

REQUIREMENTS: Python 3.8+ (standard library only).

DESCRIPTION
    One module names the projects the devops scripts drive and the files inside
    them the scripts read or edit. A suite imports a name; it does not spell a
    path of its own, so relocating, adding or renaming a project is an edit here
    and nowhere else under `devops/`.

    `EXAMPLES_ROOT` and `OFFICIAL_EXAMPLE_PROJECTS` name the projects a
    developer runs: the local checkout and the tracked official examples. The
    projects the suites drive live under `devops/tests/projects/` and are named
    by `TEST_PROJECTS_ROOT` / `TEST_PROJECT_NAMES`, so a clean checkout can run
    the gate without an examples checkout. The functional test project crate derives
    from `TESTS_ROOT` as well.

    Each project is exported in two forms, because the callers need different
    ones:

      * `*_ROOT` - an absolute `Path` to the project directory, for reading and
        writing files;
      * `*_PATH` - the `PROJECT_PATH` value the host receives. The host resolves
        it against its working directory, which is the Cargo workspace
        (`modules/`), and accepts POSIX separators on every platform.

    A tool that resolves its own argument against the repository root (the
    shipping generator, the web build, a suite that writes a command line) takes
    `repository_relative(<root>)` instead of a `*_PATH` value.

USAGE
  from core.projects import NATIVE_PROJECT_ROOT, NATIVE_PROJECT_PATH

  With `devops/` on `sys.path`; see `core.paths.ensure_devops_on_path`. Suites
  that import `core.suite_common` receive every public name below through its
  star import.

--- SCRIPT ---
"""

import os
from pathlib import Path

from .paths import MODULES_ROOT, REPOSITORY_ROOT, TESTS_ROOT

# The examples directory, relative to the repository root. It is the single
# knob: the root below, every project path, and the globs and display paths of
# the guards that scan it all derive from this value.
EXAMPLES_RELATIVE_PATH = "examples/local"
EXAMPLES_ROOT = REPOSITORY_ROOT / EXAMPLES_RELATIVE_PATH

# The test projects the devops suites drive. They are test-owned rather than
# examples: a suite edits their sources, asserts their log lines and reverts
# them, and each one carries the anchors its suite needs. They live beside the
# suites so a clean checkout can run the gate without a developer's examples
# checkout.
TEST_PROJECTS_ROOT = TESTS_ROOT / "projects"
TEST_PROJECT_NAMES = (
    "native",
    "scene",
    "managed",
    "editor_api",
)

# The official examples, the tracked counterpart of the local checkout above:
# each group holds one Rust project and one C# project running the same demo,
# and both are read from disk the same way. This is a CURATED set, not the
# discovery rule: it names the projects a suite can ask for by role, and it
# deliberately omits groups that are on disk but not built in CI
# (`pills_awakens`). A new example is discovered by the walk in
# `example_project_roots`, never by an edit here.
OFFICIAL_EXAMPLES_ROOT = REPOSITORY_ROOT / "examples"
OFFICIAL_EXAMPLE_PROJECTS = {
    "cube_rust": OFFICIAL_EXAMPLES_ROOT / "cube" / "cube_rust",
    "cube_csharp": OFFICIAL_EXAMPLES_ROOT / "cube" / "cube_csharp",
    "cube_no_editor_rust": OFFICIAL_EXAMPLES_ROOT / "cube_no_editor" / "cube_no_editor_rust",
    "cube_no_editor_csharp": OFFICIAL_EXAMPLES_ROOT / "cube_no_editor" / "cube_no_editor_csharp",
    "pills_rust": OFFICIAL_EXAMPLES_ROOT / "pills" / "pills_rust",
    "pills_csharp": OFFICIAL_EXAMPLES_ROOT / "pills" / "pills_csharp",
    "headless_rust": OFFICIAL_EXAMPLES_ROOT / "headless" / "headless_rust",
    "headless_csharp": OFFICIAL_EXAMPLES_ROOT / "headless" / "headless_csharp",
}

# Every project directory is named by its manifest, so this is the file a
# discovery walk looks for.
PROJECT_SETTINGS_FILENAME = "project_settings.yaml"

# The folder inside an extension folder that holds its crates. It is also what
# tells an extension folder from a group folder.
EXTENSION_MODULES_DIRECTORY_NAME = "modules"

# Extension examples: an extension ships its own runnable projects under
# `modules/extensions/<extension>/examples/<name>/`, or
# `modules/extensions/<group>/<extension>/examples/<name>/` when the extension
# sits in a group folder. Nothing lists them - a directory holding a manifest
# is one - so an extension can add an example without an edit anywhere under
# `devops/`.
EXTENSION_EXAMPLES_ROOT = MODULES_ROOT / "extensions"


def extension_folders(search_root=None) -> list[Path]:
    """Returns every extension folder under `search_root`, in path order.

    The rule is structural, not a list: a directory holding a `modules/` folder
    is an extension folder, and any other directory is a group folder whose
    direct children are extension folders - `extensions/<extension>/` or
    `extensions/<group>/<extension>/`. A group never nests. Bookkeeping folders
    (`_...`) are skipped at both levels.

    `search_root` defaults to [`EXTENSION_EXAMPLES_ROOT`] and exists so a test
    can point the same rule at a temporary tree.
    """
    root = EXTENSION_EXAMPLES_ROOT if search_root is None else search_root
    folders: list[Path] = []
    if not root.is_dir():
        return folders
    for folder in sorted(root.iterdir()):
        if not folder.is_dir() or folder.name.startswith("_"):
            continue
        if (folder / EXTENSION_MODULES_DIRECTORY_NAME).is_dir():
            folders.append(folder)
            continue
        for child in sorted(folder.iterdir()):
            if child.is_dir() and not child.name.startswith("_"):
                folders.append(child)
    return folders


def extension_example_roots(search_root=None) -> list[Path]:
    """Returns every project an extension ships under its `examples/` folder.

    The rule is the path, not a list:
    `<search_root>/<extension>/examples/<name>/project_settings.yaml`, and one
    level deeper for an extension inside a group folder. `search_root`
    defaults to [`EXTENSION_EXAMPLES_ROOT`] and exists so a test can point the
    same rule at a temporary tree.

    An extension with no `examples/` folder contributes nothing, and a
    directory inside `examples/` without a manifest of its own is skipped
    rather than guessed at.
    """
    root = EXTENSION_EXAMPLES_ROOT if search_root is None else search_root
    if not root.is_dir():
        return []
    roots: list[Path] = []
    for examples in sorted((*root.glob("*/examples"), *root.glob("*/*/examples"))):
        for example in sorted(examples.glob("*")):
            if example.is_dir() and project_settings_yaml(example).is_file():
                roots.append(example)
    return roots


def host_project_path(project_root: Path) -> str:
    """Returns the `PROJECT_PATH` value for a project directory.

    The host resolves `PROJECT_PATH` against its working directory, which every
    caller in `devops/` sets to the Cargo workspace.
    """
    return os.path.relpath(project_root, MODULES_ROOT).replace(os.sep, "/")


def repository_relative(path: Path) -> str:
    """Returns a repository-root-relative POSIX path, for a tool argument."""
    return os.path.relpath(path, REPOSITORY_ROOT).replace(os.sep, "/")


def project_settings_yaml(project_root: Path) -> Path:
    """Returns a project's settings file path."""
    return project_root / PROJECT_SETTINGS_FILENAME


def example_project_roots() -> list[Path]:
    """Returns every example project directory: local, official and extension.

    The local checkout is walked, because its projects are a developer's own
    and nothing names them here; the official examples come from the curated
    catalog above, so a suite and the build sweep agree on what the official
    set is; and the extension examples are walked, so a module that ships one
    is discovered without an edit here.

    The local checkout uses the same two layouts the official examples do: a
    project directory (its `project_settings.yaml`) or a group directory
    holding one Rust and one C# project. A directory with no manifest of its
    own is read as a group.
    """
    roots: list[Path] = []
    if EXAMPLES_ROOT.is_dir():
        for path in sorted(EXAMPLES_ROOT.iterdir()):
            if not path.is_dir():
                continue
            if project_settings_yaml(path).is_file():
                roots.append(path)
                continue
            roots.extend(
                sorted(
                    child
                    for child in path.iterdir()
                    if child.is_dir() and project_settings_yaml(child).is_file()
                )
            )
    roots.extend(sorted(OFFICIAL_EXAMPLE_PROJECTS.values()))
    roots.extend(extension_example_roots())
    return roots


def test_project_roots() -> list[Path]:
    """Returns the test projects the suites drive, in name order."""
    return [TEST_PROJECTS_ROOT / name for name in TEST_PROJECT_NAMES]


def project_roots() -> list[Path]:
    """Returns every project the repository ships: examples plus test projects."""
    return example_project_roots() + test_project_roots()


# =============================================================================
# The native test project: the reload, patch, migration, module and shipping anchors
# =============================================================================

# One project carries every anchor the mutation-heavy native suites need: the
# ball/spline scene, the module list, the render chain, the embedded sound and
# the migration witnesses (`src/lib.rs`).
NATIVE_PROJECT_ROOT = TEST_PROJECTS_ROOT / "native"
NATIVE_PROJECT_PATH = host_project_path(NATIVE_PROJECT_ROOT)
NATIVE_PROJECT_SRC = NATIVE_PROJECT_ROOT / "src"
NATIVE_PROJECT_LIB_RS = NATIVE_PROJECT_SRC / "lib.rs"
# The constant the hot-reload harness edits to force a project reload.
NATIVE_PROJECT_SETTINGS_RS = NATIVE_PROJECT_SRC / "settings.rs"
# The components the migration scenarios rewrite live in the crate root, next
# to the seeds and the witness systems they are read by.
NATIVE_PROJECT_RES = NATIVE_PROJECT_ROOT / "res"


# =============================================================================
# The managed test project: the C# bridge, analyzer and mirror suites
# =============================================================================

MANAGED_PROJECT_ROOT = TEST_PROJECTS_ROOT / "managed"
MANAGED_PROJECT_PATH = host_project_path(MANAGED_PROJECT_ROOT)
MANAGED_PROJECT_SRC = MANAGED_PROJECT_ROOT / "src"
MANAGED_PROJECT_CSPROJ = MANAGED_PROJECT_ROOT / "managed.csproj"
MANAGED_PROJECT_COMPONENTS_CS = MANAGED_PROJECT_SRC / "Components.cs"
MANAGED_PROJECT_SYSTEMS_CS = MANAGED_PROJECT_SRC / "Systems.cs"
# The probe file the bridge suite writes and deletes.
MANAGED_PROJECT_BRIDGE_PROBE_CS = MANAGED_PROJECT_SRC / "BridgeProbe.cs"


# =============================================================================
# The scene test project: the editor, picking, surface, web, asset and theme target
# =============================================================================

# One small, deterministic scene carries every read-mostly anchor: one mesh
# drawn three times, three rotating models with the middle one on the camera's
# axis for a scripted click to reach, a committed material, a sample map the
# asset watcher reimports, a themed UI layer, a committed render pass and an
# editor project layer. Nothing in it is reload- or patch-specific; that is the
# native test project's job.
SCENE_PROJECT_ROOT = TEST_PROJECTS_ROOT / "scene"
SCENE_PROJECT_PATH = host_project_path(SCENE_PROJECT_ROOT)
SCENE_PROJECT_SRC = SCENE_PROJECT_ROOT / "src"
SCENE_PROJECT_LIB_RS = SCENE_PROJECT_SRC / "lib.rs"
# The file the editor suite's scene edits land in.
SCENE_PROJECT_SCENE_RS = SCENE_PROJECT_SRC / "scene.rs"
SCENE_PROJECT_RES = SCENE_PROJECT_ROOT / "res"


# =============================================================================
# The themed test project: the UI theme, game UI and editor suites
# =============================================================================

# The scene test project ships the theme chain the suites assert: the code default is
# `Theme::brutalist()`, `res/ui/accent.theme` overrides the accent to `#33d17a`
# and the heading size, and the body font is the Silkscreen face - the same
# three-layer setup the old example drove, now inside the test project tree so the
# gate does not read `examples/`.
THEMED_PROJECT_ROOT = SCENE_PROJECT_ROOT
THEMED_PROJECT_PATH = SCENE_PROJECT_PATH
THEMED_PROJECT_SRC = SCENE_PROJECT_SRC
THEMED_PROJECT_LIB_RS = SCENE_PROJECT_LIB_RS
# The override the theme suite edits, twice, to prove a live re-apply.
THEMED_PROJECT_THEME = SCENE_PROJECT_RES / "ui" / "accent.theme"
# The face the editor's project theme layer installs.
THEMED_PROJECT_FONT = SCENE_PROJECT_RES / "fonts" / "Silkscreen-Regular.ttf"


# =============================================================================
# The native test project's crate root: the components the schema scenarios rewrite
# =============================================================================

# The migration scenarios run against the native test project, where the witness
# components and their seeds live, so both families of scenarios run the same
# build and there is no second project to keep in step. `NATIVE_PROJECT_LIB_RS`
# is the file the native scenarios edit and whose restore settles with a
# project reload rather than a module reload.
