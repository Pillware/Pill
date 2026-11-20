#!/usr/bin/env python3
"""
Pill comment and layout lint for the Rust, shader, managed and devops sources.

PROTECTS: Every .rs file follows the Pill comment and layout rules.
TIER: static

REQUIREMENTS: Python 3.8+ (standard library only).

DESCRIPTION
    Lints every `.rs` file under the repository against the Pill comment and
    layout rules (see the Pill Guide, "Contributing / Coding standards"), and
    holds shader sources and the managed and devops comment trees to the same
    comment-content ban. The checks are line-oriented heuristics, so they are
    a fast first-pass signal, not a substitute for a full per-file audit.

    Deliberate exemptions are listed beside the rule they weaken.

    The checks are the twelve `section()` titles `main()` prints, in run
    order: module headers, safety comments, public docs, import groups, tests
    placement, egui state, colour literals, comment content (Rust, managed and
    devops), shell patch openings, shader comment content, shell re-export
    blocks and log calls (the logging guide's string and long targets,
    value-only messages, trailing periods, `message` fields and prints). That
    list is the single source; this header does not repeat it.

    The two window sizes are tunable through the environment:
    `SAFETY_WINDOW_LINES` (default 30) and `DOC_WINDOW_LINES` (default 12).

USAGE
  python devops/tests/test_coding_standards.py [--root <path>] [--list]
                                              [--self-test] [--report <path>]
                                              [--exclude <path>]...
  python devops/tests/test_coding_standards.py <path/to/file.rs | path/to/dir> ...

EXAMPLE USAGE
  python devops/tests/test_coding_standards.py
  python devops/tests/test_coding_standards.py --list
  python devops/tests/test_coding_standards.py --self-test
  python devops/tests/test_coding_standards.py --root modules/engine/pill_engine/src
  python devops/tests/test_coding_standards.py modules/engine/pill_core/src/lib.rs
  python devops/tests/test_coding_standards.py modules/engine/pill_core/src/lib.rs \
      devops/tests/projects/native/src/lib.rs --report local/lint.json
  python devops/tests/test_coding_standards.py --root modules \
      --exclude modules/extensions/pill_master_renderer

  Exit status: 0 when every checked file complies, 1 when at least one
  violation is reported, 2 on a usage error.

--- SCRIPT ---
"""

import argparse
import ast
import io
import json
import os
import re
import subprocess
import sys
import tokenize
from functools import lru_cache
from pathlib import Path
from typing import Dict, List, Optional, Tuple

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.paths import REPOSITORY_ROOT  # noqa: E402
from core.projects import (  # noqa: E402
    EXAMPLES_RELATIVE_PATH,
    MANAGED_PROJECT_SRC,
    repository_relative,
)
from core.suite_common import run_suite_with_timing  # noqa: E402
from core.test_report import (  # noqa: E402
    ANSI_BOLD,
    ANSI_GREEN,
    ANSI_RED,
    ANSI_YELLOW,
    colorize,
    section,
)

# =============================================================================
# Tunable heuristic bounds
# =============================================================================

# Maximum lines between a SAFETY marker and the `unsafe` token it covers.
SAFETY_WINDOW_LINES = int(os.environ.get("SAFETY_WINDOW_LINES", "30"))
# Maximum lines between a `///` line and the item it documents.
DOC_WINDOW_LINES = int(os.environ.get("DOC_WINDOW_LINES", "12"))

# Directories whose contents are build output or vendored code, never sources.
# `build` covers cargo OUT_DIR staging under `examples/*/build/` and the
# generated shipping bundle scratch under the repo-root `build/` - with one
# exception, the tracked stub crate `build/pill_shipping_bundle` (see
# `is_linted`).
EXCLUDED_DIRECTORY_NAMES = {
    "target",
    "node_modules",
    ".git",
    "pill_standalone_temp",
    "build",
    # The host writes `host_module_*` wrappers and `host_project_*` members at
    # runtime; both are gitignored, the guard hook refuses edits to them, and
    # a git-ignored file can never reach the changed-files hook - so linting
    # them in CI would gate on files nobody can fix there.
    "generated",
    "projects_generated",
}

# The repository-root `local/` is gitignored scratch - experiment labs, notes,
# one-off probes - and not engine source. It is matched at the top level only:
# the game projects under `examples/` and the suites' own test projects under
# `devops/tests/projects/` are linted like any other project's. Excluding the
# root here rather than in one caller is what keeps a bare run and a
# `--root modules` run in agreement.
ROOT_ONLY_EXCLUDED_DIRECTORY_NAMES = {"local"}

# The repository's own `build/` holds generated scratch - and one tracked
# crate whose `.gitignore` negations keep its source in the tree: the shipping
# bundle stub the generator overwrites. A `.rs` in there is source and is
# linted; everything else under a `build/`, everywhere, is not.
BUILD_SOURCE_CRATES = ("pill_shipping_bundle",)


def repository_relative_parts(path: Path) -> Tuple[str, ...]:
    """The path's parts below the repository root, or all parts if outside."""
    resolved = path.resolve()
    try:
        return resolved.relative_to(REPOSITORY_ROOT).parts
    except ValueError:
        return resolved.parts


def is_in_excluded_directory(parts: Tuple[str, ...]) -> bool:
    """Whether repository-relative parts name a directory the lint skips."""
    for index, part in enumerate(parts):
        if part in EXCLUDED_DIRECTORY_NAMES:
            return True
        if index == 0 and part in ROOT_ONLY_EXCLUDED_DIRECTORY_NAMES:
            return True
    return False

# =============================================================================
# Patterns
#
# Word boundaries are spelled out as explicit character classes rather than
# `\b`, so a pattern cannot depend on a regex engine's boundary rules.
# =============================================================================

SAFETY_MARKER_PATTERN = re.compile(r"// *SAFETY:|# *Safety")
COMMENT_LINE_PATTERN = re.compile(r"^\s*//")
BLANK_LINE_PATTERN = re.compile(r"^\s*$")
UNSAFE_TOKEN_PATTERN = re.compile(r"(^|[^A-Za-z0-9_])unsafe([^A-Za-z0-9_]|$)")
UNSAFE_BLOCK_PATTERN = re.compile(r"unsafe\s*\{")
UNSAFE_DECLARATION_PATTERN = re.compile(r"unsafe\s+(fn|extern|const|trait)")
DOC_COMMENT_PATTERN = re.compile(r"^///")
PUBLIC_ITEM_PATTERN = re.compile(
    r"^pub(\s*\((crate|super)\))?(\s+unsafe)?\s+"
    r"(fn|struct|enum|trait|const|static|type|mod)([^A-Za-z0-9_]|$)"
)
TESTS_MODULE_PATTERN = re.compile(
    r"^\s*(?:pub\s+)?mod (?:tests|[A-Za-z0-9_]+_tests)(\s|;)"
)
TOP_LEVEL_SECTION_PATTERN = re.compile(r"^// ====")

IMPORT_GROUP_HEADERS = (
    ("// Standard library", "'// Standard library'"),
    ("// External crates", "'// External crates'"),
    ("// Current crate", "'// Current crate'"),
)

# Header text to the group it labels, for the content rule.
GROUP_NAMES = {
    "// Standard library": "standard",
    "// External crates": "external",
    "// Current crate": "crate",
}

# Which `use` root belongs to which group. `crate`, `super` and `self` are
# this crate's own modules; everything else that is not the standard library
# comes from another crate.
USE_PATH_PATTERN = re.compile(r"^use\s+([A-Za-z0-9_:]+)")
USE_STANDARD_ROOTS = frozenset({"std", "core", "alloc"})
USE_CRATE_ROOTS = frozenset({"crate", "super", "self"})

# The import-group content rule covers the renderer and the player. The rest of
# the repository still carries imports under the wrong header; widening this is
# tracked as follow-up work, and the order rule above keeps gating every
# crate in the meantime.
IMPORT_GROUP_CONTENT_SCOPE = (
    "modules/extensions/pill_master_renderer/",
    "modules/rendering/",
    "modules/hosting/pill_player/",
)

# Shader sources carry the same comment ban as Rust sources.
SHADER_SUFFIXES = (".wgsl", ".hlsl")

# A `mod tests` inside a file this short is not worth flagging.
TESTS_PLACEMENT_MINIMUM_LINES = 20

# An outline test module is named after its parent (`<parent>_tests`); the bare
# file-backed `mod tests;` is refused. Three crates still declare the bare
# form, and each rename lands with its crate's next change - a path leaves this
# tuple with its rename.
OUTLINE_TEST_LEGACY_PATHS = (
    "modules/csharp/pill_csharp_bridge/src/lib.rs",
    "modules/editor/pill_editor_shell/src/shell.rs",
    "modules/hosting/pill_development_host/src/hot_patch/mod.rs",
)

# A file-backed module declaration, and the attribute that marks a test module.
OUTLINE_MODULE_PATTERN = re.compile(r"^\s*(?:pub\s+)?mod\s+([A-Za-z0-9_]+)\s*;")
CFG_TEST_ATTRIBUTE_PATTERN = re.compile(r"^\s*#\[cfg\(test\)\]\s*$")

# The two shapes the placement rule treats as a test module: an inline
# `mod tests { … }` block, and a file-backed declaration of either name. An
# inline subject-named block (`mod round_trip_tests { … }`) is not looked for,
# and a `pub mod` declaration is left to the name rule.
INLINE_TESTS_BLOCK_PATTERN = re.compile(r"^\s*mod tests(\s|$)")
FILE_TESTS_DECLARATION_PATTERN = re.compile(
    r"^\s*(?:pub\s+)?mod (?:tests|[A-Za-z0-9_]+_tests)\s*;"
)

# Comment text that describes the work that produced the code rather than the
# code itself. Mirrors `devops/tools/comment_audit.py`; keep the two in step.
PROCESS_REFERENCE_PATTERNS = (
    ("work item", re.compile(r"\bW-\d{4}\b")),
    ("stage", re.compile(r"\bstage \d+\b", re.IGNORECASE)),
    ("plan decision", re.compile(r"\bdecision [A-Z]\d+\b")),
    ("site path", re.compile(r"\.construction_site\b|\.agents/")),
    ("audit finding", re.compile(r"\bSA-\d{4}\b")),
    ("audit opportunity", re.compile(r"\baudit opportunity \d+\b", re.IGNORECASE)),
    ("plan section", re.compile(r"\b[a-z][a-z0-9_-]* plan, section [A-Z]\b", re.IGNORECASE)),
)

# The content guard covers the engine's sources and the one tracked crate
# under `build/` (the shipping bundle stub). The managed and devops trees have
# their own scopes and carry a per-file allowance while their debt is swept; a
# path with no entry is allowed zero references.
COMMENT_CONTENT_SCOPE = (
    "modules",
    "build/pill_shipping_bundle",
)
CSHARP_COMMENT_CONTENT_ROOTS = (
    "modules/csharp",
    repository_relative(MANAGED_PROJECT_SRC),
)
# Build output inside a C# tree, never source.
CSHARP_EXCLUDED_DIRECTORY_NAMES = {"bin", "obj"}

# The devops tooling's own guard: full-line `#` comments and docstrings.
DEVOPS_COMMENT_CONTENT_SCOPE = "devops/"
PYTHON_EXCLUDED_DIRECTORY_NAMES = {
    "__pycache__",
    ".git",
    "node_modules",
    "target",
}

COMMENT_BASELINE_PATH = (
    REPOSITORY_ROOT / "devops" / "tests" / "comment_content_baseline.json"
)
COMMENT_BASELINE_FORMAT_VERSION = 1

# The editor shell's hot-patch contract. A patch crate is the edited file's
# top-level `use` lines plus the crate's own glob, so a module that names
# anything from the shared model or from a sibling needs both globs. The
# prelude defines itself and the test module imports what it uses.
SHELL_SOURCES_MARKER = "/modules/editor/pill_editor_shell/src/shell/"
SHELL_PRELUDE_USE = "use pill_editor_shell::shell::prelude::*;"
SHELL_SIBLING_USE = "use pill_editor_shell::shell::*;"
SHELL_EXEMPT_FILES = ("prelude.rs", "tests.rs")

# The editor shell's crate root, whose `pub use` wall answers "what does this
# module publish" for every patch-related edit. One block per module, in the
# order the modules are declared: a reader deciding whether a name is
# re-exported must not have to scan two regions and compare them.
SHELL_ROOT_SOURCE = "/pill_editor_shell/src/shell.rs"
PUB_MODULE_DECLARATION_PATTERN = re.compile(r"^pub mod ([A-Za-z0-9_]+);$")
PUB_MODULE_REEXPORT_PATTERN = re.compile(r"^pub use ([A-Za-z0-9_]+)::")

# The APIs that keep boxed `Any`/`Fn` values inside egui's `Context`.
# A reloadable crate that stores its own types or
# callbacks through them leaves dangling type IDs, drop code and vtables
# behind after a reload, so the egui shell keeps its long-lived state in a
# world resource instead; the contract is documented in `pill_editor_core`'s
# crate docs. The patterns are dot-prefixed so an unrelated `metadata::<`
# turbofish cannot trip the `data` rule, and the closure forms require a `|`
# after the parenthesis so a handle's `.data()` accessor stays legal.
#
# The map's own methods are listed as well as its two entry points: a write
# through a borrow taken elsewhere (`memory.data.insert_temp(..)`) never
# spells `.data_mut::<`, and those are the forms a refactor produces.
EGUI_CONTEXT_STORAGE_PATTERNS = (
    (re.compile(r"\.data_mut::<"), "stores a type in egui's Context (IdTypeMap)"),
    (re.compile(r"\.data::<"), "reads a type from egui's Context (IdTypeMap)"),
    (
        re.compile(r"\.data_mut\s*\(\s*\|"),
        "writes egui's Context data map from a closure (IdTypeMap)",
    ),
    (
        re.compile(r"\.data\s*\(\s*\|"),
        "reads egui's Context data map from a closure (IdTypeMap)",
    ),
    (re.compile(r"\.insert_temp\b"), "stores a value in egui's Context (IdTypeMap)"),
    (
        re.compile(r"\.insert_persisted\b"),
        "stores a persisted value in egui's Context (IdTypeMap)",
    ),
    (
        re.compile(r"\.get_temp(?:_mut_or)?\b"),
        "reads a value from egui's Context (IdTypeMap)",
    ),
    (
        re.compile(r"\.get_persisted(?:_mut_or)?\b"),
        "reads a persisted value from egui's Context (IdTypeMap)",
    ),
    (
        re.compile(r"\.remove_by_type\b"),
        "drops a type's values from egui's Context (IdTypeMap)",
    ),
    (re.compile(r"\.set_request_repaint_callback"), "stores a callback in egui's Context"),
    (re.compile(r"\.on_begin_pass"), "installs an egui plugin callback"),
    (re.compile(r"\.on_end_pass"), "installs an egui plugin callback"),
    (re.compile(r"PaintCallback"), "paints a callback shape carrying user data"),
    (
        re.compile(r"\bdyn Any\b"),
        "erases a value into a trait object whose drop code lives in this "
        "reloadable image",
    ),
)

# A line that deliberately touches the map says so: the marker, on the line
# itself or the one above, exempts that single line. Only a test planting a
# previous generation's value uses it - production code in a reloadable crate
# never does, and the exemption is per line so it cannot cover a module.
EGUI_STATE_MARKER_PATTERN = re.compile(r"egui-state-ok")

# The editor's look lives in theme tokens: `pill_editor_core::style`
# owns the tokens and the shell asks for a role ([`text`], [`panel`], ...). A
# `Color32::` literal in the editor's UI code is a colour that bypasses the
# theme, so it is refused everywhere except at the sites whose job IS colour
# maths - listed below with the reason each needs one.
COLOUR_LITERAL_PATTERN = re.compile(r"Color32::(from_[a-z_]+|[A-Z][A-Z_]+)")
COLOUR_LITERAL_AREAS = (
    "modules/editor/pill_editor_core/src",
    "modules/editor/pill_editor/src",
    "modules/editor/pill_editor_shell/",
    "modules/frontends/pill_web/src",
    # The kit translates and draws from the same tokens: a literal here would
    # bypass the theme for both UIs at once.
    "modules/ui/pill_ui_kit/src",
)

# (path suffix, line fragment, why the site needs its own colour).
COLOUR_LITERAL_ALLOWLIST = (
    (
        "pill_editor_core/src/style.rs",
        "Color32::from_rgba_unmultiplied",
        "the token-to-egui conversion",
    ),
    (
        "pill_editor_core/src/style.rs",
        "Color32::TRANSPARENT",
        "an unfilled tab or an outline the fill alone replaces",
    ),
    (
        "pill_editor_core/src/icons.rs",
        "Color32::from_rgba_unmultiplied",
        "the duotone layers' alpha maths",
    ),
    (
        "pill_editor_core/src/icons.rs",
        "Color32::from_rgb(200, 100, 50)",
        "an icon test's sample colour",
    ),
    (
        "pill_editor_core/src/brand.rs",
        "Color32::from_white_alpha",
        "the brand mark's coverage alpha",
    ),
    (
        "pill_editor_core/src/inspector.rs",
        "Color32::from_rgb",
        "the axis colours shared with the gizmo",
    ),
    (
        "pill_editor_core/src/inspector.rs",
        "Color32::GRAY",
        "the axis colour's fallback",
    ),
    (
        "shell/assets_tiles.rs",
        "Color32::WHITE",
        "an untinted image",
    ),
    ("shell/bar.rs", "Color32::TRANSPARENT", "clearing a fill"),
    ("shell/tests.rs", "Color32::WHITE", "a test's untinted image"),
    (
        "pill_ui_kit/src/style.rs",
        "Color32::TRANSPARENT",
        "an unfilled tab or an outline the fill alone replaces",
    ),
    (
        "pill_ui_kit/src/style.rs",
        "Color32::from_rgba_unmultiplied",
        "the token-to-egui conversion",
    ),
    (
        "pill_ui_kit/src/variants.rs",
        "Color32::from_rgba_unmultiplied",
        "the variant's token-to-egui conversion",
    ),
    (
        "pill_ui_kit/src/widgets.rs",
        "Color32::from_rgba_unmultiplied",
        "the token-to-egui conversion",
    ),
)


# =============================================================================
# Violation collection
# =============================================================================


class ViolationCollector:
    """Records violations, prints them as they are found, and totals them up."""

    def __init__(self) -> None:
        self.violation_count = 0
        self.files_with_violations: List[str] = []
        self.entries: List[Tuple[str, str, str]] = []
        self._seen_files: Dict[str, bool] = {}

    def record(self, display_path: str, check: str, detail: str) -> None:
        """Records one violation against a file."""
        self.violation_count += 1
        self.entries.append((display_path, check, detail))
        if display_path not in self._seen_files:
            self._seen_files[display_path] = True
            self.files_with_violations.append(display_path)
        print(f"  {colorize('x', ANSI_RED)} {display_path}")
        print(f"      {colorize('[' + check + ']', ANSI_YELLOW)} {detail}")


# =============================================================================
# File discovery
# =============================================================================


@lru_cache(maxsize=1)
def ignored_rust_files() -> frozenset:
    """The Rust files this checkout ignores and has not added to the index.

    The changed-files hook can never receive one of these paths, so the CI
    scan must not lint it either; otherwise a local scratch project can turn
    the gate red for a file nobody can fix through the hook. The list is
    computed once per process because `is_linted` is called for every file.
    """
    try:
        completed = subprocess.run(
            [
                "git",
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--",
                "*.rs",
            ],
            cwd=REPOSITORY_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return frozenset()
    if completed.returncode != 0:
        return frozenset()
    return frozenset(
        (REPOSITORY_ROOT / line).resolve() for line in completed.stdout.splitlines()
    )


def is_linted(path: Path) -> bool:
    """Whether the lint checks this path: a `.rs` source outside every excluded
tree and outside git's ignored set.

    One predicate for every caller. CI scans with it (through
    `find_rust_files`), the changed-files hook imports it to select what it
    passes in - and an explicitly named file is subject to it too, so a hook
    run and a CI run can never disagree about a file again. Before this was
    shared, the hook linted single files before any exclusions applied while CI
    scanned `modules/` only, so a file could pass one and fail the other.

    Gitignored files are excluded because the hook cannot receive them: a
    scratch project under `examples/` is part of the checkout only for its
    owner, and a gate that lints it fails for everyone else.
    """
    if path.suffix != ".rs":
        return False
    if path.resolve() in ignored_rust_files():
        return False
    relative_parts = repository_relative_parts(path)
    for index, part in enumerate(relative_parts):
        excluded = part in EXCLUDED_DIRECTORY_NAMES or (
            index == 0 and part in ROOT_ONLY_EXCLUDED_DIRECTORY_NAMES
        )
        if not excluded:
            continue
        # The repository's own `build/` keeps one tracked crate's sources: the
        # shipping bundle stub's `src/`. Everything else under a `build/` -
        # a stray `build.rs` in that crate included - is generated scratch.
        if (
            part == "build"
            and index == 0
            and relative_parts[1:2]
            and relative_parts[1] in BUILD_SOURCE_CRATES
            and relative_parts[2:3] == ("src",)
        ):
            continue
        return False
    return True


def find_rust_files(root: Path, excludes: Optional[List[Path]] = None) -> List[Path]:
    """Returns every `.rs` file under a root, skipping build and VCS output.

    A single `.rs` file is a valid root, mirroring `find` accepting a file
    path. Results are sorted by their POSIX string.

    `excludes` names files or directory subtrees to leave out entirely - the
    escape hatch for a tree that is being brought to the standard rather than
    expected to meet it (CI passes the in-progress renderer this way, so the
    rules still gate every other crate).
    """
    if root.is_file():
        return [root] if is_linted(root) else []

    resolved_excludes = [path.resolve() for path in (excludes or [])]
    discovered: List[Path] = []
    for path in root.rglob("*.rs"):
        if not is_linted(path):
            continue
        resolved = path.resolve()
        if any(
            resolved == exclude or exclude in resolved.parents
            for exclude in resolved_excludes
        ):
            continue
        discovered.append(path)
    return sorted(discovered, key=lambda path: path.as_posix())


def find_shader_files(root: Path, excludes: Optional[List[Path]] = None) -> List[Path]:
    """Returns every shader source under a root, skipping build and VCS output.

    Shaders are not Rust, so they are not part of the per-file checks; the
    comment-content rule reaches them because a shader carries the same
    process-reference ban as a Rust comment.
    """
    if root.is_file():
        return [root] if root.suffix in SHADER_SUFFIXES else []

    resolved_excludes = [path.resolve() for path in (excludes or [])]
    discovered: List[Path] = []
    for suffix in SHADER_SUFFIXES:
        for path in root.rglob(f"*{suffix}"):
            if is_in_excluded_directory(repository_relative_parts(path)):
                continue
            resolved = path.resolve()
            if any(
                resolved == exclude or exclude in resolved.parents
                for exclude in resolved_excludes
            ):
                continue
            discovered.append(path)
    return sorted(discovered, key=lambda path: path.as_posix())


def find_csharp_files(root: Path) -> List[Path]:
    """Every `.cs` source under a root, skipping build output and VCS data."""
    if root.is_file():
        return [root] if root.suffix == ".cs" else []
    discovered: List[Path] = []
    for path in root.rglob("*.cs"):
        parts = repository_relative_parts(path)
        if any(part in CSHARP_EXCLUDED_DIRECTORY_NAMES for part in parts):
            continue
        if is_in_excluded_directory(parts):
            continue
        discovered.append(path)
    return sorted(discovered, key=lambda path: path.as_posix())


def find_python_files(root: Path) -> List[Path]:
    """Every `.py` source under a root, skipping caches and scratch trees."""
    if root.is_file():
        return [root] if root.suffix == ".py" else []
    discovered: List[Path] = []
    for path in root.rglob("*.py"):
        parts = repository_relative_parts(path)
        if any(part in PYTHON_EXCLUDED_DIRECTORY_NAMES for part in parts):
            continue
        if is_in_excluded_directory(parts):
            continue
        discovered.append(path)
    return sorted(discovered, key=lambda path: path.as_posix())


def collect_python_files(targets: List[Path]) -> List[Path]:
    """The devops Python files the guard owns under the scan targets."""
    if len(targets) == 1 and targets[0].resolve() == REPOSITORY_ROOT:
        roots = [REPOSITORY_ROOT / "devops"]
    else:
        roots = targets
    found: List[Path] = []
    seen = set()
    for root in roots:
        if not root.exists():
            continue
        for path in find_python_files(root):
            if not display_path_for(path).startswith(DEVOPS_COMMENT_CONTENT_SCOPE):
                continue
            key = path.resolve()
            if key in seen:
                continue
            seen.add(key)
            found.append(path)
    return sorted(found, key=lambda path: path.as_posix())


def collect_csharp_files(targets: List[Path]) -> List[Path]:
    """The C# files the guard owns under the given scan targets.

    A bare run walks the three declared roots; a targeted run walks only what
    the caller named, and the scope filter still keeps example projects that
    are outside the guard out of the selection.
    """
    if len(targets) == 1 and targets[0].resolve() == REPOSITORY_ROOT:
        roots = [REPOSITORY_ROOT / name for name in CSHARP_COMMENT_CONTENT_ROOTS]
    else:
        roots = targets
    found: List[Path] = []
    seen = set()
    for root in roots:
        if not root.exists():
            continue
        for path in find_csharp_files(root):
            if not is_under_roots(display_path_for(path), CSHARP_COMMENT_CONTENT_ROOTS):
                continue
            key = path.resolve()
            if key in seen:
                continue
            seen.add(key)
            found.append(path)
    return sorted(found, key=lambda path: path.as_posix())


def write_comment_baseline() -> int:
    """Writes the C# process-reference counts as per-file allowances."""
    csharp_counts: Dict[str, int] = {}
    for root_name in CSHARP_COMMENT_CONTENT_ROOTS:
        root = REPOSITORY_ROOT / root_name
        if not root.exists():
            continue
        for path in find_csharp_files(root):
            lines, _text = read_lines(path)
            total, _counts = count_references_in_texts(csharp_comment_texts(lines))
            if total:
                csharp_counts[display_path_for(path)] = total
    devops_counts: Dict[str, int] = {}
    for path in find_python_files(REPOSITORY_ROOT / "devops"):
        _lines, text = read_lines(path)
        total, _counts = count_references_in_texts(python_comment_texts(text))
        if total:
            devops_counts[display_path_for(path)] = total
    document = {
        "format_version": COMMENT_BASELINE_FORMAT_VERSION,
        "scopes": {
            "csharp": dict(sorted(csharp_counts.items())),
            "devops": dict(sorted(devops_counts.items())),
        },
    }
    COMMENT_BASELINE_PATH.write_text(
        json.dumps(document, indent=2) + "\n", encoding="utf-8"
    )
    print(
        f"Wrote {len(csharp_counts)} C# and {len(devops_counts)} devops "
        f"allowance(s) to {display_path_for(COMMENT_BASELINE_PATH)}"
    )
    return 0


def display_path_for(path: Path) -> str:
    """Renders a path relative to the repository root when possible."""
    try:
        return path.resolve().relative_to(REPOSITORY_ROOT).as_posix()
    except ValueError:
        return path.as_posix()


def read_lines(path: Path) -> Tuple[List[str], str]:
    """Reads a source file as a line list plus its raw text.

    Undecodable bytes are replaced rather than raising: a lint should report
    on a file with a stray byte, not abort the whole run.
    """
    text = path.read_text(encoding="utf-8", errors="replace")
    return text.splitlines(), text


# =============================================================================
# Check - module header
# =============================================================================


def check_module_header(
    lines: List[str], text: str, display_path: str, collector: ViolationCollector
) -> None:
    """Requires a `//!` opening block and a `# Responsibilities` section."""
    first_content_line: Optional[str] = None
    for line in lines:
        if line.strip():
            first_content_line = line.lstrip()
            break

    if first_content_line is None:
        collector.record(display_path, "module header", "file is empty or whitespace-only")
        return
    if not first_content_line.startswith("//!"):
        collector.record(
            display_path, "module header", "first content line is not a //! module header"
        )
    if "# Responsibilities" not in text:
        collector.record(
            display_path, "module header", "missing '# Responsibilities' section"
        )


# =============================================================================
# Check - safety comments near every `unsafe` token
# =============================================================================


def first_code_match(pattern, line: str):
    """Returns the first match of `pattern` that is not inside a double-quoted string.

    The lint reads Rust as text, and `unsafe` appears inside string literals in
    this codebase - the host's codegen carries a C# keyword table that lists the
    word. A quoted token is data, not a declaration, so counting unescaped
    quotes before the `unsafe` word itself (an odd count means the word sits
    inside a string) tells the two apart without parsing Rust. The count starts
    at the word, not at the match, because the surrounding pattern deliberately
    includes the character on either side of it.
    """
    for match in pattern.finditer(line):
        word_start = match.start() + match.group(0).find("unsafe")
        prefix = line[:word_start]
        quotes = 0
        index = 0
        while index < len(prefix):
            if prefix[index] == "\\":
                index += 2
                continue
            if prefix[index] == '"':
                quotes += 1
            index += 1
        if quotes % 2 == 0:
            return match
    return None


def check_safety_comments(
    lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Requires a SAFETY justification above each `unsafe` block or impl.

    `unsafe fn` / `extern` / `const` / `trait` signature declarations are
    exempt: their contract belongs in a `/// # Safety` doc, not in a
    `// SAFETY:` comment above the signature. A token inside a string literal
    is data, not code, and is skipped entirely.
    """
    last_safety_line = 0
    for line_number, line in enumerate(lines, start=1):
        if SAFETY_MARKER_PATTERN.search(line):
            last_safety_line = line_number
            continue
        if COMMENT_LINE_PATTERN.search(line):
            continue
        if BLANK_LINE_PATTERN.search(line):
            continue
        if first_code_match(UNSAFE_TOKEN_PATTERN, line) is None:
            continue
        if first_code_match(UNSAFE_BLOCK_PATTERN, line) is None and first_code_match(
            UNSAFE_DECLARATION_PATTERN, line
        ) is not None:
            continue
        if last_safety_line == 0 or line_number - last_safety_line > SAFETY_WINDOW_LINES:
            collector.record(
                display_path,
                "safety comments",
                f"line {line_number}: 'unsafe' has no // SAFETY: (or # Safety) "
                f"within {SAFETY_WINDOW_LINES} lines above",
            )


# =============================================================================
# Check - public items carry a `///` doc comment
# =============================================================================


def check_public_item_docs(
    lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Requires a `///` doc comment above every public item.

    Lines inside a raw string are skipped: test fixtures embed Rust source
    verbatim (`const PLAIN_SOURCE: &str = r#"..."#`), and the lint must read
    that text as data rather than as items to document.
    """
    last_doc_line = 0
    in_raw_string = False
    for line_number, raw_line in enumerate(lines, start=1):
        line = raw_line.strip()
        if in_raw_string:
            if '"#' in raw_line:
                in_raw_string = False
            continue
        raw_open = raw_line.find('r#"')
        if raw_open != -1:
            if '"#' not in raw_line[raw_open + 3 :]:
                in_raw_string = True
            continue
        if DOC_COMMENT_PATTERN.search(line):
            last_doc_line = line_number
            continue
        # Blank lines and attributes sit between a doc comment and its item,
        # so they must not reset the window.
        if line == "" or line.startswith("#"):
            continue
        if not PUBLIC_ITEM_PATTERN.search(line):
            continue
        if last_doc_line == 0 or line_number - last_doc_line > DOC_WINDOW_LINES:
            collector.record(
                display_path,
                "public item docs",
                f"line {line_number}: '{line}' has no /// doc comment above",
            )


# =============================================================================
# Check - import group headers appear in order
# =============================================================================


def check_import_group_order(
    lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Requires standard library, external crates, then current crate."""
    positions: Dict[str, Optional[int]] = {}
    for header, _ in IMPORT_GROUP_HEADERS:
        positions[header] = None
        for line_number, line in enumerate(lines, start=1):
            if line.startswith(header):
                positions[header] = line_number
                break

    # Every ordered pair is compared, so a file with only two of the three
    # headers is still checked.
    for index, (earlier_header, earlier_label) in enumerate(IMPORT_GROUP_HEADERS):
        for later_header, later_label in IMPORT_GROUP_HEADERS[index + 1 :]:
            earlier_line = positions[earlier_header]
            later_line = positions[later_header]
            if earlier_line is None or later_line is None:
                continue
            if earlier_line > later_line:
                collector.record(
                    display_path,
                    "import groups",
                    f"{earlier_label} (line {earlier_line}) appears after "
                    f"{later_label} (line {later_line})",
                )


# =============================================================================
# Check - tests section sits at the bottom
# =============================================================================


def check_tests_at_bottom(
    lines: List[str], text: str, display_path: str, collector: ViolationCollector
) -> None:
    """Requires that no new top-level section follows a test module.

    A column-0 `// ====` separator after the test module means another section
    starts below it. Content inside `mod tests { }` is indented, so a large
    test module that is still last passes. Both declaration shapes count:
    an inline `mod tests { … }` block and a file-backed
    `mod <parent>_tests;`.
    """
    # Newline count, matching `wc -l` in the shell version.
    if text.count("\n") < TESTS_PLACEMENT_MINIMUM_LINES:
        return

    if display_path.replace("\\", "/") in OUTLINE_TEST_LEGACY_PATHS:
        return

    tests_line: Optional[int] = None
    for line_number, line in enumerate(lines, start=1):
        if INLINE_TESTS_BLOCK_PATTERN.search(
            line
        ) or FILE_TESTS_DECLARATION_PATTERN.search(line):
            tests_line = line_number
            break
    if tests_line is None:
        return

    for line_number, line in enumerate(lines, start=1):
        if line_number <= tests_line:
            continue
        if TOP_LEVEL_SECTION_PATTERN.search(line):
            collector.record(
                display_path,
                "tests placement",
                f"a new top-level section starts at line {line_number}, after "
                f"the tests module at line {tests_line}",
            )
            return


# Flags a file-backed `mod tests;`; see `OUTLINE_TEST_LEGACY_PATHS`.
def check_outline_test_module_name(
    lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Requires a file-backed test module to be named after its parent.

    The form is `mod <parent>_tests;`. An inline `mod tests { … }` keeps its
    name - it is not a file. The name says which module the tests belong to
    even after the subject moves to another file, and it keeps a test module
    apart from a helper module a test declares.
    """
    if display_path.replace("\\", "/") in OUTLINE_TEST_LEGACY_PATHS:
        return
    for index, line in enumerate(lines):
        if not CFG_TEST_ATTRIBUTE_PATTERN.match(line):
            continue
        # A doc comment or another attribute may sit between the marker and
        # the declaration.
        for following_index in range(index + 1, min(index + 6, len(lines))):
            following = lines[following_index]
            stripped = following.strip()
            if stripped.startswith("///") or stripped.startswith("#["):
                continue
            module = OUTLINE_MODULE_PATTERN.match(following)
            if module is not None and module.group(1) == "tests":
                collector.record(
                    display_path,
                    "tests module name",
                    f"line {following_index + 1}: a file-backed `mod tests;`; "
                    "name the module after its parent (`<parent>_tests`)",
                )
            break


# Flags a shell module that cannot compile as a patch crate; see
# `SHELL_PRELUDE_USE` and `SHELL_SIBLING_USE`.
def check_shell_patch_openings(
    lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Requires both hot-patch globs in every editor shell module.

    The rule the shell module header states: a patch crate replays the edited
    file's top-level `use` lines, so a body that names the shared model's
    vocabulary (`shell::prelude::*`) or a sibling module (`shell::*`) compiles
    only while both globs are in that opening. A module missing one still runs -
    until an edit names it, at which point the host silently reloads instead of
    patching.
    """
    if SHELL_SOURCES_MARKER not in "/" + display_path.lstrip("/"):
        return
    file_name = display_path.rsplit("/", 1)[-1]
    if file_name in SHELL_EXEMPT_FILES:
        return
    top_level_uses = [line.strip() for line in lines if line.startswith("use ")]
    missing = [
        glob
        for glob in (SHELL_PRELUDE_USE, SHELL_SIBLING_USE)
        if glob not in top_level_uses
    ]
    if not missing:
        return
    collector.record(
        display_path,
        "shell patch opening",
        "a patch crate replays this file's top-level `use` lines, so the shell "
        "module must open with both `shell::prelude::*` and `shell::*`; "
        f"missing {', '.join('`' + glob + '`' for glob in missing)}",
    )


# Flags a re-export that sits outside its own module's block; see
# `SHELL_ROOT_SOURCE`.
def check_shell_reexport_blocks(
    lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Requires one contiguous `pub use` block per module in the shell's root.

    The wall is scanned, not read top to bottom, so an export that lives in
    another module's block is found the hard way: by grepping its module name
    and getting hits in two regions. A declaration opens a block and the block
    runs until the next item; blank lines, comments and attributes stay inside
    it, which is what lets the usual blank line between a `pub mod` and its
    exports pass.
    """
    if not display_path.endswith(SHELL_ROOT_SOURCE):
        return
    open_module = None
    for line in lines:
        stripped = line.strip()
        declaration = PUB_MODULE_DECLARATION_PATTERN.match(stripped)
        if declaration:
            open_module = declaration.group(1)
            continue
        reexport = PUB_MODULE_REEXPORT_PATTERN.match(stripped)
        if reexport:
            module = reexport.group(1)
            if module != open_module:
                inside = (
                    f"inside the `{open_module}` block" if open_module else "before its `pub mod`"
                )
                collector.record(
                    display_path,
                    "shell re-export block",
                    f"`pub use {module}::...` sits {inside}; every module's "
                    "exports belong in the block that follows its own `pub mod`",
                )
            continue
        if not stripped or stripped.startswith("//") or stripped.startswith("#["):
            continue
        open_module = None


# =============================================================================
# Check - no egui context storage in reloadable crates
# =============================================================================


# Whether a display path belongs to a crate that reloads while it runs:
# extension crates and example projects do; the engine, the hosts and the
# editor frontends do not.
RELOADABLE_PATH_PARTS = ("extensions",)


def is_reloadable_source(display_path: str) -> bool:
    """Whether the egui-storage rule applies to this path."""
    parts = display_path.split("/")
    if any(part in RELOADABLE_PATH_PARTS for part in parts):
        return True
    # Every example and test project reloads, whichever root a checkout keeps
    # it under: the official groups, the local-only checkout, and the suites'
    # own projects under `devops/tests/projects`.
    if parts and parts[0] == "examples":
        return True
    return display_path.startswith("devops/tests/projects/")


# Flags the egui context-storage APIs; see `EGUI_CONTEXT_STORAGE_PATTERNS`.
def check_egui_context_storage(
    lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Flags egui context-storage APIs in crates that reload at runtime.

    The same calls are fine in shared libraries and frontends, which never
    unload; the rule is scoped to `extensions/**` and `examples/**`, where
    the stored value may outlive the image that stored it. A line carrying
    `egui-state-ok` - or sitting right below one - is exempt: a test that
    plants a previous generation's value is the only legitimate user.
    """
    if not is_reloadable_source(display_path):
        return
    for index, line in enumerate(lines):
        if COMMENT_LINE_PATTERN.match(line):
            continue
        if EGUI_STATE_MARKER_PATTERN.search(line):
            continue
        if index > 0 and EGUI_STATE_MARKER_PATTERN.search(lines[index - 1]):
            continue
        for pattern, detail in EGUI_CONTEXT_STORAGE_PATTERNS:
            if pattern.search(line):
                collector.record(
                    display_path,
                    "egui state",
                    f"line {index + 1}: {detail}; keep long-lived egui state "
                    "in a world resource (pill_editor_core's reload-safety contract)",
                )


# Flags a colour literal in the editor's UI code; see `COLOUR_LITERAL_AREAS`.
def check_colour_literals(
    lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Flags `Color32::` literals outside the theme's colour-maths allowlist.

    The editor's look is a theme: `pill_editor_core::style` translates
    the tokens and the shell asks for a role, so a literal here is a colour the
    next restyle will miss. The allowlist is per line fragment, so an exempt
    site cannot cover the file it sits in.
    """
    if not any(display_path.startswith(area) for area in COLOUR_LITERAL_AREAS):
        return
    for index, line in enumerate(lines):
        if COMMENT_LINE_PATTERN.match(line):
            continue
        match = COLOUR_LITERAL_PATTERN.search(line)
        if match is None:
            continue
        allowed = any(
            display_path.endswith(suffix) and fragment in line
            for suffix, fragment, _reason in COLOUR_LITERAL_ALLOWLIST
        )
        if allowed:
            continue
        collector.record(
            display_path,
            "colour literal",
            f"line {index + 1}: `{match.group(0)}` bypasses the theme; ask "
            "`pill_editor_core::style` for a role instead",
        )


# =============================================================================
# Check - log calls follow the logging guide
# =============================================================================

# Log calls are linted only under `modules/`: a test project or example project
# logs on its own `target: "project"`, which the guide asks of projects.
LOG_CALL_AREA = "modules/"
# A `tracing` (or `pill_core`) log macro, with or without its path. `log::`
# macros are left alone: they only stand in for an external crate in the
# bridge's tests.
LOG_MACRO_PATTERN = re.compile(
    r"(?<![A-Za-z0-9_:])((?:pill_core|pill_engine|tracing)::(?:tracing::)?)?"
    r"(info|warn|error|debug|trace)!\s*\("
)
PRINT_MACRO_PATTERN = re.compile(r"(?<![A-Za-z0-9_:])(e?println)!\s*\(")
STRING_TARGET_PATTERN = re.compile(r'target\s*:\s*"')
LONG_TARGET_PATTERN = re.compile(r"target\s*:\s*pill_core::telemetry::telemetry_target::")
MESSAGE_FIELD_PATTERN = re.compile(r"^\s*message\s*=[^=]")
PLACEHOLDER_ONLY_PATTERN = re.compile(r"\{[^{}]*\}")
STRING_LITERAL_PATTERN = re.compile(r'"((?:[^"\\]|\\.)*)"')

# A message that is only a placeholder is allowed when what fills it is a
# fixed text or a block, never a bare value:
PLACEHOLDER_MESSAGE_ALLOWLIST = (
    # A block (`log_block`/`log_block_colored`, or a report helper that
    # returns one): the heading becomes the message.
    "log_block",
    "render_startup_report(",
    "render_plan(",
    # Message-selecting helpers: each returns one fixed sentence per kind,
    # kept per kind because the suites wait for the exact text.
    "_message()",
    "forgotten_types_warning()",
    # A watcher's callback line: each watcher passes its own fixed sentence.
    "callback_error_message",
    # The collision and layout refusals: one shared constant per sentence.
    "_REFUSAL",
)
# The C# bridge forwards text written in managed code; it has no wording of
# its own to give it.
PLACEHOLDER_MESSAGE_FILES = ("modules/csharp/pill_csharp_bridge/src/abi.rs",)

# A print is allowed only where telemetry cannot be: build scripts (and the
# build-script source they include), proc macros, and a frontend's `main.rs`
# before logging starts. Test code prints to the harness, not to the engine.
PRINT_ALLOWED_PATH_FRAGMENTS = (
    "/build.rs",
    "modules/frontends/build_support/",
    "_macros/src/",
    "modules/frontends/pill_standalone/src/main.rs",
    "modules/editor/pill_editor/src/main.rs",
)


def is_test_source_path(display_path: str) -> bool:
    """Whether the path is a Rust test source.

    Three shapes count: an integration test under a `tests/` folder, a file
    named `tests.rs`, and the outline test module form `<parent>_tests.rs`.
    """
    normalized = display_path.replace("\\", "/")
    if "/tests/" in normalized:
        return True
    file_name = normalized.rsplit("/", 1)[-1]
    return file_name == "tests.rs" or file_name.endswith("_tests.rs")


def iterate_macro_calls(text: str, pattern) -> List[Tuple[int, re.Match, str]]:
    """Every macro call `pattern` opens in `text`: (line, match, argument text).

    The argument text runs to the matching `)`, skipping string contents, so a
    call that spans lines is read whole. Calls on comment lines are skipped.
    """
    calls = []
    for match in pattern.finditer(text):
        line_start = text.rfind("\n", 0, match.start()) + 1
        if text[line_start:match.start()].lstrip().startswith("//"):
            continue
        depth = 0
        index = match.end() - 1
        in_string = False
        while index < len(text):
            character = text[index]
            if in_string:
                if character == "\\":
                    index += 2
                    continue
                if character == '"':
                    in_string = False
            elif character == '"':
                in_string = True
            elif character == "(":
                depth += 1
            elif character == ")":
                depth -= 1
                if depth == 0:
                    break
            index += 1
        line_number = text.count("\n", 0, match.start()) + 1
        calls.append((line_number, match, text[match.end():index]))
    return calls


def split_top_level_arguments(arguments: str) -> List[str]:
    """`arguments` split at its top-level commas (outside brackets and strings)."""
    parts: List[str] = []
    depth = 0
    in_string = False
    current = []
    index = 0
    while index < len(arguments):
        character = arguments[index]
        if in_string:
            current.append(character)
            if character == "\\" and index + 1 < len(arguments):
                current.append(arguments[index + 1])
                index += 2
                continue
            if character == '"':
                in_string = False
        elif character == '"':
            in_string = True
            current.append(character)
        elif character in "([{":
            depth += 1
            current.append(character)
        elif character in ")]}":
            depth -= 1
            current.append(character)
        elif character == "," and depth == 0:
            parts.append("".join(current).strip())
            current = []
        else:
            current.append(character)
        index += 1
    tail = "".join(current).strip()
    if tail:
        parts.append(tail)
    return parts


def test_code_start(lines: List[str]) -> int:
    """The 1-based line where the file's test module starts, or 0.

    Both forms count: an inline `mod tests { … }` and a file-backed
    `mod <parent>_tests;`.
    """
    for index, line in enumerate(lines):
        if TESTS_MODULE_PATTERN.match(line):
            return index + 1
    return 0


# Flags log calls and prints that break the logging guide; see the patterns.
def check_log_calls(
    text: str, lines: List[str], display_path: str, collector: ViolationCollector
) -> None:
    """Flags string and long targets, value-only messages, trailing periods,
    `message =` fields and prints, in Rust code under `modules/`.

    Each rule names the guide's rule it enforces; the allowlists above carry
    the reason each exemption exists.
    """
    if not display_path.startswith(LOG_CALL_AREA):
        return
    tests_from = test_code_start(lines)
    for line_number, _match, arguments in iterate_macro_calls(text, LOG_MACRO_PATTERN):
        if STRING_TARGET_PATTERN.search(arguments):
            collector.record(
                display_path,
                "log target",
                f"line {line_number}: a string target; use a `telemetry_target` constant",
            )
        if LONG_TARGET_PATTERN.search(arguments):
            collector.record(
                display_path,
                "log target",
                f"line {line_number}: write the target as `telemetry_target::X`, "
                "with the module imported",
            )
        parts = split_top_level_arguments(arguments)
        for part in parts:
            if MESSAGE_FIELD_PATTERN.match(part):
                collector.record(
                    display_path,
                    "log field",
                    f"line {line_number}: a field named `message` replaces the "
                    "line's text; name it after its concept (`error`, ...)",
                )
        # The message is the first positional string literal; a field's value
        # always follows its name.
        literals = [
            (position, part) for position, part in enumerate(parts) if part.startswith('"')
        ]
        if not literals:
            continue
        message_position, message = literals[0]
        literal = STRING_LITERAL_PATTERN.fullmatch(message)
        if literal is None:
            continue
        message_text = literal.group(1)
        if message_text.endswith("."):
            collector.record(
                display_path,
                "log message",
                f"line {line_number}: the message ends with a period",
            )
        if PLACEHOLDER_ONLY_PATTERN.fullmatch(message_text):
            rest = ", ".join(parts[message_position + 1:])
            allowed = display_path in PLACEHOLDER_MESSAGE_FILES or any(
                fragment in rest for fragment in PLACEHOLDER_MESSAGE_ALLOWLIST
            )
            if not allowed:
                collector.record(
                    display_path,
                    "log message",
                    f"line {line_number}: the message is only a value; say what "
                    "happened and put the value in a field",
                )
    if any(fragment in display_path for fragment in PRINT_ALLOWED_PATH_FRAGMENTS):
        return
    if is_test_source_path(display_path):
        return
    for line_number, match, arguments in iterate_macro_calls(text, PRINT_MACRO_PATTERN):
        if tests_from and line_number >= tests_from:
            continue
        if arguments.lstrip().startswith('"cargo:'):
            continue
        collector.record(
            display_path,
            "print",
            f"line {line_number}: `{match.group(1)}!` bypasses the log; use "
            "`tracing` so the `logging:` settings reach the line",
        )


# =============================================================================
# Check - comments carry no development-process references
# =============================================================================


def count_process_references(lines: List[str]) -> Tuple[int, List[str]]:
    """Counts process references in full-line comments.

    Returns the total and the per-pattern breakdown, with the first line that
    carries one. A trailing comment is not measured, matching the recon tool.
    """
    counts = {name: 0 for name, _pattern in PROCESS_REFERENCE_PATTERNS}
    first_line: Optional[int] = None
    for index, line in enumerate(lines, start=1):
        stripped = line.strip()
        if not stripped.startswith("//"):
            continue
        for name, pattern in PROCESS_REFERENCE_PATTERNS:
            found = len(pattern.findall(stripped))
            if found:
                counts[name] += found
                if first_line is None:
                    first_line = index
    details = [f"{name} {count}" for name, count in counts.items() if count]
    if first_line is not None:
        details.append(f"first at line {first_line}")
    return sum(counts.values()), details


def count_references_in_texts(texts: List[str]) -> Tuple[int, Dict[str, int]]:
    """Counts process references across already-extracted comment texts.

    The managed and devops extractors differ from the Rust one, so they hand
    over the comment lines they found instead of sharing its `//` filter.
    """
    counts = {name: 0 for name, _pattern in PROCESS_REFERENCE_PATTERNS}
    for text in texts:
        for name, pattern in PROCESS_REFERENCE_PATTERNS:
            counts[name] += len(pattern.findall(text))
    return sum(counts.values()), {name: count for name, count in counts.items() if count}


def check_comment_content(
    lines: List[str],
    display_path: str,
    collector: ViolationCollector,
) -> None:
    """Refuses a process reference in an engine or shipping-stub comment.

    Applies to `modules/` and the tracked `build/pill_shipping_bundle` crate:
    example projects and generated build output are outside this guard's
    scope. A reference in code (a string literal, for instance) is not a
    comment and is not measured.
    """
    if not is_under_roots(display_path, COMMENT_CONTENT_SCOPE):
        return
    total, details = count_process_references(lines)
    if total == 0:
        return
    detail = ", ".join(details) if details else "no details"
    collector.record(
        display_path,
        "comment content",
        f"{total} process reference(s): {detail}",
    )


def csharp_comment_texts(lines: List[str]) -> List[str]:
    """The full-line comments of a C# source: `//`, `///` and `/* ... */`.

    A trailing comment after code is not measured, matching the Rust rule; a
    block comment is measured on every line it spans.
    """
    texts: List[str] = []
    in_block = False
    for line in lines:
        stripped = line.strip()
        if in_block:
            texts.append(stripped)
            if "*/" in stripped:
                in_block = False
            continue
        if stripped.startswith("//"):
            texts.append(stripped)
            continue
        if stripped.startswith("/*"):
            texts.append(stripped)
            if "*/" not in stripped:
                in_block = True
    return texts


def is_under_roots(display_path: str, roots: Tuple[str, ...]) -> bool:
    """Whether a repo-relative path sits under one of the given roots."""
    return any(
        display_path == root or display_path.startswith(root + "/") for root in roots
    )


@lru_cache(maxsize=1)
def comment_baseline() -> Dict[str, Dict[str, int]]:
    """The per-file allowances by scope; an unreadable file allows nothing."""
    try:
        document = json.loads(COMMENT_BASELINE_PATH.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    if not isinstance(document, dict):
        return {}
    if document.get("format_version") != COMMENT_BASELINE_FORMAT_VERSION:
        return {}
    scopes = document.get("scopes")
    return scopes if isinstance(scopes, dict) else {}


def check_csharp_comment_content(
    lines: List[str],
    display_path: str,
    collector: ViolationCollector,
    allowance: Optional[int] = None,
) -> None:
    """Refuses a process reference in a managed source's comment.

    The managed tree carries debt while it is swept, so each file gets the
    allowance the baseline records; the stage that sweeps a file lowers it.
    """
    if not is_under_roots(display_path, CSHARP_COMMENT_CONTENT_ROOTS):
        return
    total, counts = count_references_in_texts(csharp_comment_texts(lines))
    allowed = (
        comment_baseline().get("csharp", {}).get(display_path, 0)
        if allowance is None
        else allowance
    )
    if total <= allowed:
        return
    detail = ", ".join(f"{name} {count}" for name, count in counts.items())
    collector.record(
        display_path,
        "comment content (csharp)",
        f"{total} process reference(s), allowance {allowed}: {detail or 'no details'}",
    )


def python_comment_texts(text: str) -> List[str]:
    """The full-line `#` comments and docstrings of a Python source.

    `tokenize` supplies the comments, `ast` the module, class and function
    docstrings. A trailing comment after code is not measured, matching the
    Rust rule; an unparsable file still yields its comments.
    """
    texts: List[str] = []
    docstring_lines = set()
    try:
        tree = ast.parse(text)
    except SyntaxError:
        tree = None
    if tree is not None:
        for node in ast.walk(tree):
            if not isinstance(
                node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)
            ):
                continue
            body = getattr(node, "body", [])
            if not body or not isinstance(body[0], ast.Expr):
                continue
            value = body[0].value
            if isinstance(value, ast.Constant) and isinstance(value.value, str):
                end = getattr(body[0], "end_lineno", body[0].lineno) or body[0].lineno
                for number in range(body[0].lineno, end + 1):
                    docstring_lines.add(number)
    try:
        for token in tokenize.generate_tokens(io.StringIO(text).readline):
            if token.type != tokenize.COMMENT:
                continue
            if token.line[: token.start[1]].strip():
                continue
            texts.append(token.string)
    except (tokenize.TokenError, IndentationError):
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.startswith("#"):
                texts.append(stripped)
    lines = text.splitlines()
    for number in sorted(docstring_lines):
        if 1 <= number <= len(lines):
            texts.append(lines[number - 1].strip())
    return texts


def check_python_comment_content(
    text: str,
    display_path: str,
    collector: ViolationCollector,
    allowance: Optional[int] = None,
) -> None:
    """Refuses a process reference in a devops Python comment or docstring."""
    if not display_path.startswith(DEVOPS_COMMENT_CONTENT_SCOPE):
        return
    total, counts = count_references_in_texts(python_comment_texts(text))
    allowed = (
        comment_baseline().get("devops", {}).get(display_path, 0)
        if allowance is None
        else allowance
    )
    if total <= allowed:
        return
    detail = ", ".join(f"{name} {count}" for name, count in counts.items())
    collector.record(
        display_path,
        "comment content (devops)",
        f"{total} process reference(s), allowance {allowed}: {detail or 'no details'}",
    )


def check_shader_comment_content(
    text: str,
    display_path: str,
    collector: ViolationCollector,
) -> None:
    """Refuses a process reference anywhere in a shader source.

    A shader has no code that could plausibly name a work item, so the whole
    file is measured rather than only its comment lines: `//` and `/* */`
    comments, and the header block a shader file opens with, carry the same
    ban the Rust rule enforces.
    """
    if not is_under_roots(display_path, COMMENT_CONTENT_SCOPE):
        return
    total, details = count_process_references(text.splitlines())
    if total == 0:
        return
    detail = ", ".join(details) if details else "no details"
    collector.record(
        display_path,
        "shader comment content",
        f"{total} process reference(s): {detail}",
    )


def check_import_group_contents(
    lines: List[str],
    display_path: str,
    collector: ViolationCollector,
) -> None:
    """Refuses a `use` that sits under the wrong group header.

    The standard names three groups and what belongs in each: the standard
    library, other crates, and this crate. The order check proves the groups
    are ordered; this one proves each `use` sits under the header its path
    belongs to, so a `pill_*` or `wgpu` import cannot hide under `// Current
    crate`. A file with more than one group must label each of them.

    Only the file's import block is measured - the run from its first `use`
    to the first line that starts a declaration - so a `use` inside a re-export
    region in the module body is not misread as an import.
    """
    if not display_path.startswith(IMPORT_GROUP_CONTENT_SCOPE):
        return
    block = import_block(lines)
    if not block:
        return
    uses = [
        (number, match.group(1))
        for number, stripped in block
        if (match := USE_PATH_PATTERN.match(stripped))
    ]
    groups = {classify_use_group(path) for _, path in uses}
    labelled = {
        GROUP_NAMES[stripped] for _, stripped in block if stripped in GROUP_NAMES
    }
    if len(groups) > 1 and not labelled:
        collector.record(
            display_path,
            "import group contents",
            f"{len(uses)} imports across {sorted(groups)} carry no group header",
        )
    current: Optional[str] = None
    for number, stripped in block:
        if stripped in GROUP_NAMES:
            current = GROUP_NAMES[stripped]
            continue
        match = USE_PATH_PATTERN.match(stripped)
        if not match:
            continue
        group = classify_use_group(match.group(1))
        if current is None:
            if len(groups) > 1:
                collector.record(
                    display_path,
                    "import group contents",
                    f"line {number}: {stripped} is outside any group header",
                )
        elif current != group:
            collector.record(
                display_path,
                "import group contents",
                f"line {number}: {stripped} is {group} but sits under {current}",
            )


def import_block(lines: List[str]) -> List[Tuple[int, str]]:
    """Returns the file's import block as (line number, stripped line) pairs.

    The block runs from the header above the first top-level `use` to the
    first line that is neither a use, a group header, a blank line nor a plain
    comment. A file with no top-level use has no block.
    """
    first_use: Optional[int] = None
    for number, line in enumerate(lines):
        if line.startswith("use "):
            first_use = number
            break
    if first_use is None:
        return []
    while first_use > 0 and lines[first_use - 1].strip() in GROUP_NAMES:
        first_use -= 1
    block: List[Tuple[int, str]] = []
    for number in range(first_use, len(lines)):
        line = lines[number]
        stripped = line.strip()
        if (
            line.startswith("use ")
            or stripped in GROUP_NAMES
            or not stripped
            or stripped.startswith("//")
        ):
            block.append((number + 1, stripped))
            continue
        break
    return block


def classify_use_group(path: str) -> str:
    """Returns which of the three groups a `use` path belongs to."""
    root = path.split("::", 1)[0]
    if root in USE_STANDARD_ROOTS:
        return "standard"
    if root in USE_CRATE_ROOTS:
        return "crate"
    return "external"


# =============================================================================
# Self test
# =============================================================================

# A synthetic example path for the self-tests: built from the configured root
# so a move of the examples directory keeps it pointing at a reloadable
# project rather than silently leaving the scope.
SYNTHETIC_EXAMPLE_LIB_RS = EXAMPLES_RELATIVE_PATH + "/acme/src/lib.rs"


class SilentCollector(ViolationCollector):
    """A collector for the self test: counts violations without printing."""

    def record(self, display_path: str, check: str, detail: str) -> None:
        """Counts one violation, skipping the per-file printout."""
        self.violation_count += 1
        self.entries.append((display_path, check, detail))


# Runs the egui-storage rule against planted sources: the rule must fire on
# a reloadable crate and stay quiet everywhere else.
def self_test() -> int:
    """Proves each planted violation fires and only its plant does."""
    cases = (
        (
            "a plant in an extension crate fails",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "let stored = context.data_mut::<Widget>();\n",
            1,
        ),
        (
            "the same plant outside a reloadable crate passes",
            "modules/editor/pill_editor/src/runner.rs",
            "let stored = context.data_mut::<Widget>();\n",
            0,
        ),
        (
            "a comment naming the rule passes",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "// never call context.data_mut::<Widget>() here\n",
            0,
        ),
        (
            "a paint callback in an example fails",
            SYNTHETIC_EXAMPLE_LIB_RS,
            "let shape = egui::Shape::Callback(PaintCallback { .. });\n",
            1,
        ),
        (
            "an unrelated metadata turbofish passes",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "let value = entry.metadata::<Todo>();\n",
            0,
        ),
        (
            "a closure write to the context map fails",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "context.data_mut(|data| { let _ = data; });\n",
            1,
        ),
        (
            "a write through a borrowed map fails",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "memory.data.insert_temp(id, 1);\n",
            1,
        ),
        (
            "a read through a borrowed map fails",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "let row = memory.data.get_temp::<Row>(id);\n",
            1,
        ),
        (
            "a type-erased value in a reloadable crate fails",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "let held: Box<dyn Any> = Box::new(1u8);\n",
            1,
        ),
        (
            "a marked plant passes",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "// egui-state-ok: planting a previous generation's value\n"
            "memory.data.insert_temp(id, 1);\n",
            0,
        ),
        (
            "a handle's data() accessor passes",
            "modules/extensions/acme/acme_editor/src/lib.rs",
            "let shader = handle.data();\n",
            0,
        ),
    )
    passed = True
    for description, display_path, source, expected in cases:
        collector = SilentCollector()
        check_egui_context_storage(source.splitlines(), display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Colour literals: a colour literal in the editor's UI code, unless
    # allowlisted.
    colour_cases = (
        (
            "a colour literal in the editor shell fails",
            "modules/editor/pill_editor_shell/src/shell/views.rs",
            "let fill = egui::Color32::from_rgb(1, 2, 3);\n",
            1,
        ),
        (
            "a named colour in the editor core fails",
            "modules/editor/pill_editor_core/src/session.rs",
            "let clear = Color32::TRANSPARENT;\n",
            1,
        ),
        (
            "an allowlisted colour-maths line passes",
            "modules/editor/pill_editor_core/src/inspector.rs",
            "0 => Color32::from_rgb(214, 96, 96),\n",
            0,
        ),
        (
            "a colour literal outside the editor passes",
            "modules/ui/pill_project_ui/src/lib.rs",
            "let accent = Color32::from_rgb(1, 2, 3);\n",
            0,
        ),
        (
            "a comment naming a colour passes",
            "modules/editor/pill_editor_core/src/style.rs",
            "// Color32::from_rgb is only for the conversion helpers\n",
            0,
        ),
    )
    for description, display_path, source, expected in colour_cases:
        collector = SilentCollector()
        check_colour_literals(source.splitlines(), display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Comment content: process references in engine comments.
    comment_cases = (
        (
            "a clean comment passes",
            "modules/ui/acme/src/lib.rs",
            "// The value is cached until the next reload.\n",
            0,
        ),
        (
            "a work-item reference fails",
            "modules/ui/acme/src/lib.rs",
            "// the cache exists for this path (W-0001)\n",
            1,
        ),
        (
            "a doc comment reference fails",
            "modules/ui/acme/src/lib.rs",
            "/// Stage 3 behaviour.\n",
            1,
        ),
        (
            "a plan decision fails",
            "modules/ui/acme/src/lib.rs",
            "// decision E5\n",
            1,
        ),
        (
            "a construction-site path fails",
            "modules/ui/acme/src/lib.rs",
            "// see .construction_site/items\n",
            1,
        ),
        (
            "a reference inside code passes",
            "modules/ui/acme/src/lib.rs",
            'let id = "W-0001";\n',
            0,
        ),
        (
            "an example project is outside the guard",
            "examples/acme/src/lib.rs",
            "// W-0001\n",
            0,
        ),
        (
            "a SAFETY comment passes",
            "modules/ui/acme/src/lib.rs",
            "// SAFETY: the pointer is live for the call.\n",
            0,
        ),
    )
    for description, display_path, source, expected in comment_cases:
        collector = SilentCollector()
        check_comment_content(source.splitlines(), display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Comment content, managed scope: the per-file allowance that the sweep
    # lowers to zero.
    csharp_cases = (
        (
            "a clean C# comment passes",
            "modules/csharp/acme/src/Widget.cs",
            "// The widget caches its layout until the next frame.\n",
            0,
            None,
        ),
        (
            "a work item in a C# comment fails",
            "modules/csharp/acme/src/Widget.cs",
            "// the cache exists for this path (W-0001)\n",
            1,
            None,
        ),
        (
            "a work item in a C# block comment fails",
            "modules/csharp/acme/src/Widget.cs",
            "/*\n * Cached (W-0001).\n */\n",
            1,
            None,
        ),
        (
            "a C# path outside the scope passes",
            "modules/engine/pill_core/src/Widget.cs",
            "// W-0001\n",
            0,
            None,
        ),
        (
            "one reference within the allowance passes",
            "modules/csharp/acme/src/Widget.cs",
            "// W-0001\n",
            0,
            1,
        ),
        (
            "a reference above the allowance fails",
            "modules/csharp/acme/src/Widget.cs",
            "// W-0001 W-0002\n",
            1,
            1,
        ),
    )
    for description, display_path, source, expected, allowance in csharp_cases:
        collector = SilentCollector()
        check_csharp_comment_content(
            source.splitlines(), display_path, collector, allowance
        )
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Comment content, devops scope: `#` comments and docstrings, with the
    # same per-file allowance as the managed tree.
    python_cases = (
        (
            "a clean Python comment passes",
            "devops/tests/test_acme.py",
            "# The cache holds one frame.\n",
            0,
            None,
        ),
        (
            "a work item in a Python comment fails",
            "devops/tests/test_acme.py",
            "# the cache exists for this path (W-0001)\n",
            1,
            None,
        ),
        (
            "a work item in a docstring fails",
            "devops/tests/test_acme.py",
            '"""The cache (W-0001)."""\n',
            1,
            None,
        ),
        (
            "a trailing comment after code passes",
            "devops/tests/test_acme.py",
            "value = 1  # W-0001\n",
            0,
            None,
        ),
        (
            "a Python path outside devops/ passes",
            "modules/engine/pill_core/tool.py",
            "# W-0001\n",
            0,
            None,
        ),
        (
            "one reference within the Python allowance passes",
            "devops/tests/test_acme.py",
            "# W-0001\n",
            0,
            1,
        ),
    )
    for description, display_path, source, expected, allowance in python_cases:
        collector = SilentCollector()
        check_python_comment_content(source, display_path, collector, allowance)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Shader comment content: the same ban reaches a shader source, which has
    # no code that could name a work item.
    shader_cases = (
        (
            "a work item in a shader header fails",
            "modules/extensions/acme/acme/src/preview.wgsl",
            "// The preview (W-0049 Stage 11).\n",
            1,
        ),
        (
            "a clean shader header passes",
            "modules/extensions/acme/acme/src/preview.wgsl",
            "// The preview draws one asset into a square target.\n",
            0,
        ),
        (
            "a shader outside modules/ is outside the guard",
            "examples/acme/shaders/preview.wgsl",
            "// W-0049\n",
            0,
        ),
    )
    for description, display_path, source, expected in shader_cases:
        collector = SilentCollector()
        check_shader_comment_content(source, display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Rule 4b: a `use` under the wrong group header, and a file whose groups
    # carry no headers at all.
    import_cases = (
        (
            "a std import under the external header fails",
            "modules/rendering/acme/src/lib.rs",
            "// External crates\nuse std::path::Path;\n\n// Current crate\nuse crate::Thing;\n",
            1,
        ),
        (
            "a std import under its own header passes",
            "modules/rendering/acme/src/lib.rs",
            "// Standard library\nuse std::path::Path;\n\n// Current crate\nuse crate::Thing;\n",
            0,
        ),
        (
            "unlabelled groups fail once per import",
            "modules/rendering/acme/src/lib.rs",
            "use std::path::Path;\n\nuse crate::Thing;\n",
            3,
        ),
        (
            "a file outside the content scope passes",
            "modules/engine/acme/src/lib.rs",
            "// External crates\nuse std::path::Path;\n\n// Current crate\nuse crate::Thing;\n",
            0,
        ),
        (
            "a workspace crate under the current-crate header fails",
            "modules/rendering/acme/src/lib.rs",
            "// Current crate\nuse crate::Thing;\nuse pill_core::slot_map::SlotMap;\n",
            1,
        ),
    )
    for description, display_path, source, expected in import_cases:
        collector = SilentCollector()
        check_import_group_contents(source.splitlines(), display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Shell re-export blocks: the shell root's re-export wall keeps one block
    # per module.
    shell_root = (
        "modules/editor/pill_editor_shell/src/shell.rs"
    )
    reexport_cases = (
        (
            "one block per module passes",
            shell_root,
            "pub mod bar;\n"
            "\n"
            "pub use bar::menu_bar;\n"
            "pub use bar::menu_body;\n"
            "\n"
            "pub mod palette;\n"
            "\n"
            "pub use palette::palette_window;\n",
            0,
        ),
        (
            "an export left in the previous module's block fails",
            shell_root,
            "pub mod bar;\n"
            "\n"
            "pub use bar::menu_bar;\n"
            "\n"
            "pub mod palette;\n"
            "\n"
            "pub use bar::menu_body;\n"
            "pub use palette::palette_window;\n",
            1,
        ),
        (
            "an export before any declaration fails",
            shell_root,
            "pub use toasts::draw_toasts;\n"
            "\n"
            "pub mod toasts;\n",
            1,
        ),
        (
            "an attribute and a doc comment stay inside the block",
            shell_root,
            "pub mod toasts;\n"
            "\n"
            "pub use toasts::draw_toasts;\n"
            "\n"
            "/// The tests module.\n"
            "#[cfg(test)]\n"
            "pub mod tests;\n",
            0,
        ),
        (
            "the same wall in another crate's root is outside the guard",
            "modules/editor/pill_editor_core/src/editor_state.rs",
            "pub mod bar;\n"
            "\n"
            "pub mod palette;\n"
            "\n"
            "pub use bar::menu_body;\n",
            0,
        ),
    )
    for description, display_path, source, expected in reexport_cases:
        collector = SilentCollector()
        check_shell_reexport_blocks(source.splitlines(), display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Log calls: each guide rule fires on its plant and passes a clean line.
    log_cases = (
        (
            "a constant target with fields passes",
            "modules/hosting/acme/src/lib.rs",
            'info!(target: telemetry_target::HOT_RELOAD, module = name, "hot reload complete");\n',
            0,
        ),
        (
            "a string target fails",
            "modules/hosting/acme/src/lib.rs",
            'info!(target: "engine::hot_reload", "hot reload complete");\n',
            1,
        ),
        (
            "a test project's string target passes",
            "devops/tests/projects/native/src/lib.rs",
            'pill_engine::tracing::info!(target: "project", "counter tick");\n',
            0,
        ),
        (
            "a fully qualified target fails",
            "modules/hosting/acme/src/lib.rs",
            'pill_core::info!(\n    target: pill_core::telemetry::telemetry_target::ECS,\n    "world cleared"\n);\n',
            1,
        ),
        (
            "a value-only message fails",
            "modules/hosting/acme/src/lib.rs",
            'error!(target: telemetry_target::HOT_RELOAD, "{}", error);\n',
            1,
        ),
        (
            "a named placeholder-only message fails",
            "modules/hosting/acme/src/lib.rs",
            'error!(target: telemetry_target::HOT_RELOAD, "{message}");\n',
            1,
        ),
        (
            "a block print passes",
            "modules/hosting/acme/src/lib.rs",
            'info!(target: telemetry_target::SYSTEM, "{}", log_block("system specs", lines));\n',
            0,
        ),
        (
            "a per-kind message helper passes",
            "modules/hosting/acme/src/lib.rs",
            'warn!(target: telemetry_target::HOT_RELOAD, error = %error, "{}", kind.build_failed_message());\n',
            0,
        ),
        (
            "a refusal constant passes",
            "modules/engine/acme/src/lib.rs",
            'warn!(target: telemetry_target::ECS, "{}", COMPONENT_LAYOUT_CHANGED_REFUSAL);\n',
            0,
        ),
        (
            "the C# forwarder passes",
            "modules/csharp/pill_csharp_bridge/src/abi.rs",
            'tracing::info!(target: telemetry_target::CSHARP, managed_target = %target, "{message}");\n',
            0,
        ),
        (
            "a message ending in a period fails",
            "modules/hosting/acme/src/lib.rs",
            'warn!(target: telemetry_target::ECS, "value skipped.");\n',
            1,
        ),
        (
            "a period inside a field value passes",
            "modules/hosting/acme/src/lib.rs",
            'warn!(target: telemetry_target::ECS, path = "a.", "value skipped");\n',
            0,
        ),
        (
            "a field named message fails",
            "modules/hosting/acme/src/lib.rs",
            'warn!(target: telemetry_target::EDITOR, message = text.as_str(), "[editor] command failed");\n',
            1,
        ),
        (
            "a field whose name ends in message passes",
            "modules/hosting/acme/src/lib.rs",
            'warn!(target: telemetry_target::EDITOR, error_message = text.as_str(), "[editor] command failed");\n',
            0,
        ),
        (
            "a println in engine code fails",
            "modules/hosting/acme/src/lib.rs",
            'fn report() {\n    println!("\\n");\n}\n',
            1,
        ),
        (
            "an eprintln in engine code fails",
            "modules/hosting/acme/src/lib.rs",
            'fn report() {\n    eprintln!("failed: {error}");\n}\n',
            1,
        ),
        (
            "a cargo directive passes",
            "modules/hosting/acme/src/lib.rs",
            'fn emit() {\n    println!("cargo:rerun-if-changed={}", path.display());\n}\n',
            0,
        ),
        (
            "a build script print passes",
            "modules/hosting/acme/build.rs",
            'fn main() {\n    println!("{}", value);\n}\n',
            0,
        ),
        (
            "a frontend's early error passes",
            "modules/frontends/pill_standalone/src/main.rs",
            'fn main() {\n    eprintln!("[standalone] telemetry setup failed: {error}");\n}\n',
            0,
        ),
        (
            "a print inside the tests module passes",
            "modules/hosting/acme/src/lib.rs",
            'fn real() {}\n\n#[cfg(test)]\nmod tests {\n    fn probe() {\n        println!("skipped");\n    }\n}\n',
            0,
        ),
        (
            "a commented log line passes",
            "modules/hosting/acme/src/lib.rs",
            '// info!(target: "engine::ecs", "{}", value);\n',
            0,
        ),
    )
    for description, display_path, source, expected in log_cases:
        collector = SilentCollector()
        check_log_calls(source, source.splitlines(), display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Outline test modules: a file-backed `mod tests;` is refused, and the
    # name says which module the tests belong to.
    outline_test_cases = (
        (
            "a file-backed `mod tests;` fails",
            "modules/engine/acme/src/widget.rs",
            "/// A widget.\n#[cfg(test)]\nmod tests;\n",
            1,
        ),
        (
            "a parent-named test module passes",
            "modules/engine/acme/src/widget.rs",
            "/// A widget.\n#[cfg(test)]\nmod widget_tests;\n",
            0,
        ),
        (
            "a public parent-named test module passes",
            "modules/extensions/acme/src/lib.rs",
            "#[cfg(test)]\npub mod acme_tests;\n",
            0,
        ),
        (
            "an inline tests block keeps its name",
            "modules/engine/acme/src/widget.rs",
            "#[cfg(test)]\nmod tests {\n    fn probe() {}\n}\n",
            0,
        ),
        (
            "a doc comment between the marker and the declaration still fails",
            "modules/engine/acme/src/widget.rs",
            "#[cfg(test)]\n/// The tests.\nmod tests;\n",
            1,
        ),
        (
            "a parked crate's bare declaration passes",
            "modules/csharp/pill_csharp_bridge/src/lib.rs",
            "#[cfg(test)]\nmod tests;\n",
            0,
        ),
        (
            "a bare declaration outside the parked set fails",
            "modules/hosting/other/src/lib.rs",
            "#[cfg(test)]\nmod tests;\n",
            1,
        ),
        (
            "a helper module under cfg(test) keeps its name",
            "modules/engine/acme/src/lib.rs",
            "#[cfg(test)]\nmod test_fixtures;\n",
            0,
        ),
    )
    for description, display_path, source, expected in outline_test_cases:
        collector = SilentCollector()
        check_outline_test_module_name(source.splitlines(), display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")

    # Tests-at-bottom placement sees the `_tests` form as well.
    placement_cases = (
        (
            "a section below a parent-named test module fails",
            "modules/engine/acme/src/widget.rs",
            "fn real() {}\n" * 16
            + "\n#[cfg(test)]\nmod widget_tests;\n\n// ====\n// Section\n",
            1,
        ),
        (
            "a parent-named test module last passes",
            "modules/engine/acme/src/widget.rs",
            "// ====\n// Section\n\nfn real() {}\n" * 5
            + "\n#[cfg(test)]\nmod widget_tests;\n",
            0,
        ),
        (
            "an inline tests block below a section is placed late",
            "modules/engine/acme/src/widget.rs",
            "#[cfg(test)]\nmod tests {\n    fn probe() {}\n}\n\n// ====\n// Section\n"
            + "fn real() {}\n" * 16,
            1,
        ),
        (
            "an inline subject-named block is outside placement",
            "modules/engine/acme/src/widget.rs",
            "#[cfg(test)]\nmod round_trip_tests {\n}\n\n// ====\n// Section\n"
            + "fn real() {}\n" * 16,
            0,
        ),
        (
            "a parked crate's declaration is outside placement",
            "modules/csharp/pill_csharp_bridge/src/lib.rs",
            "#[cfg(test)]\nmod tests;\n\n// ====\n// Section\n" + "fn real() {}\n" * 16,
            0,
        ),
    )
    for description, display_path, source, expected in placement_cases:
        collector = SilentCollector()
        check_tests_at_bottom(source.splitlines(), source, display_path, collector)
        outcome = "PASS" if collector.violation_count == expected else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")
    return 0 if passed else 1


# =============================================================================
# Entry point
# =============================================================================


def build_parser() -> argparse.ArgumentParser:
    """Builds the command-line parser."""
    parser = argparse.ArgumentParser(
        prog="test_coding_standards.py",
        description=(
            "Checks .rs files against the Pill comment and layout rules. "
            "Exit 0 when every file complies, 1 on violations, 2 on a usage error."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "examples:\n"
            "  test_coding_standards.py\n"
            "  test_coding_standards.py --list\n"
            "  test_coding_standards.py --root modules/engine/pill_engine/src\n"
            "  test_coding_standards.py modules/engine/pill_core/src/lib.rs\n"
            "  test_coding_standards.py --root modules --exclude "
            "modules/extensions/pill_master_renderer\n"
        ),
    )
    parser.add_argument(
        "--root",
        default=None,
        metavar="PATH",
        help="Scan a directory instead of the repository root",
    )
    parser.add_argument(
        "--exclude",
        action="append",
        default=[],
        metavar="PATH",
        help="Skip a file or directory subtree (repeatable)",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="Print every .rs file that would be checked, then exit",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        dest="self_test",
        help="Run the planted-violation self tests, then exit",
    )
    parser.add_argument(
        "--write-comment-baseline",
        action="store_true",
        dest="write_comment_baseline",
        help="Write the per-file comment-content allowances, then exit",
    )
    parser.add_argument(
        "--report",
        metavar="PATH",
        help="Write a JSON report of the checked files and violations to PATH",
    )
    parser.add_argument(
        "target",
        nargs="*",
        metavar="PATH",
        help="Files or directories to scan, instead of --root (any number)",
    )
    return parser


def collect_files(targets: List[Path], excludes: List[Path]) -> List[Path]:
    """Every linted file under the targets, deduplicated, in path order.

    More than one target arrives from callers that already know their file
    list - the changed-files hook passes the files it may have touched - so a
    directory, a file and a mixture all work in one call.
    """
    found: List[Path] = []
    seen = set()
    for target in targets:
        for path in find_rust_files(target, excludes):
            key = path.resolve()
            if key in seen:
                continue
            seen.add(key)
            found.append(path)
    return sorted(found, key=lambda path: path.as_posix())


def write_report(path: Path, files: List[Path], collector: ViolationCollector) -> None:
    """Writes the machine-readable report `--report` promises.

    The human output stays exactly as it was - CI and `run_all.py` read it -
    and this gives callers a stable shape to attribute violations by file
    instead of scraping coloured text.
    """
    document = {
        "files_checked": [display_path_for(file) for file in files],
        "violations": [
            {"file": display_path, "check": check, "detail": detail}
            for display_path, check, detail in collector.entries
        ],
    }
    path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")


def resolve_targets(arguments: argparse.Namespace) -> List[Path]:
    """Resolves the scan targets from `--root` or the positional paths.

    Raises `SystemExit(2)` on a usage error.
    """
    if arguments.target:
        targets = [Path(entry) for entry in arguments.target]
    elif arguments.root:
        targets = [Path(arguments.root)]
    else:
        targets = [REPOSITORY_ROOT]
    for target in targets:
        if not target.exists():
            print(f"ERROR: scan target '{target}' does not exist", file=sys.stderr)
            raise SystemExit(2)
    return targets


def main() -> int:
    """Scans the targets and reports every violation found."""
    arguments = build_parser().parse_args()
    if arguments.self_test:
        print(colorize("Pill comment & layout lint - self test", ANSI_BOLD))
        return self_test()
    if arguments.write_comment_baseline:
        return write_comment_baseline()
    targets = resolve_targets(arguments)
    files = collect_files(targets, [Path(entry) for entry in arguments.exclude])

    if arguments.list:
        for path in files:
            print(display_path_for(path))
        print()
        print(f"Total: {len(files)} .rs file(s) under {display_path_for(targets[0])}" + (
            f" and {len(targets) - 1} more target(s)" if len(targets) > 1 else ""
        ))
        return 0

    under = ", ".join(display_path_for(target) for target in targets)
    print(colorize("Pill comment & layout lint", ANSI_BOLD))
    print(f"Scanning {len(files)} .rs file(s) under {under}")
    if not files:
        print("No Rust source files found.")
        return 0

    collector = ViolationCollector()
    # Every file is read once and its lines reused by all twelve checks.
    sources = [(path, display_path_for(path), *read_lines(path)) for path in files]
    csharp_files = collect_csharp_files(targets)
    csharp_sources = [
        (display_path_for(path), *read_lines(path)) for path in csharp_files
    ]
    python_files = collect_python_files(targets)
    python_sources = [
        (display_path_for(path), read_lines(path)[1]) for path in python_files
    ]

    section("(1/12) Module header")
    for _, display_path, lines, text in sources:
        check_module_header(lines, text, display_path, collector)

    section("(2/12) Safety comments")
    for _, display_path, lines, _text in sources:
        check_safety_comments(lines, display_path, collector)

    section("(3/12) Public item docs")
    for _, display_path, lines, _text in sources:
        check_public_item_docs(lines, display_path, collector)

    section("(4/12) Import groups")
    for _, display_path, lines, _text in sources:
        check_import_group_order(lines, display_path, collector)
        check_import_group_contents(lines, display_path, collector)

    section("(5/12) Tests at the bottom")
    for _, display_path, lines, text in sources:
        check_tests_at_bottom(lines, text, display_path, collector)
        check_outline_test_module_name(lines, display_path, collector)

    section("(6/12) egui state (reloadable crates)")
    for _, display_path, lines, _text in sources:
        check_egui_context_storage(lines, display_path, collector)

    section("(7/12) colour literals (editor UI)")
    for _, display_path, lines, _text in sources:
        check_colour_literals(lines, display_path, collector)

    section("(8/12) comment content (modules/, csharp, devops)")
    for _, display_path, lines, _text in sources:
        check_comment_content(lines, display_path, collector)
    for display_path, lines, _text in csharp_sources:
        check_csharp_comment_content(lines, display_path, collector)
    for display_path, text in python_sources:
        check_python_comment_content(text, display_path, collector)

    section("(9/12) shell patch openings (editor shell)")
    for _, display_path, lines, _text in sources:
        check_shell_patch_openings(lines, display_path, collector)

    shaders: List[Path] = []
    excluded = [Path(entry) for entry in arguments.exclude]
    for target in targets:
        shaders.extend(find_shader_files(target, excluded))
    section(f"(10/12) shader comment content ({len(shaders)} shader source(s))")
    for path in shaders:
        _lines, text = read_lines(path)
        check_shader_comment_content(text, display_path_for(path), collector)

    section("(11/12) shell re-export blocks (editor shell)")
    for _, display_path, lines, _text in sources:
        check_shell_reexport_blocks(lines, display_path, collector)

    section("(12/12) log calls and prints (modules/)")
    for _, display_path, lines, text in sources:
        check_log_calls(text, lines, display_path, collector)

    if arguments.report:
        write_report(Path(arguments.report), files, collector)

    print()
    if collector.violation_count == 0:
        print(
            colorize(
                f"PASS: all {len(files)} files comply with the Pill comment & "
                "layout rules.",
                ANSI_GREEN,
            )
        )
        return 0

    print(
        colorize(
            f"FAIL: {collector.violation_count} violation(s) across "
            f"{len(collector.files_with_violations)} file(s).",
            ANSI_RED,
        )
    )
    print()
    print("Files that need attention:")
    for display_path in collector.files_with_violations:
        print(f"  - {display_path}")
    return 1


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
