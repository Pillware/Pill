#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+ (standard library only). `git` on PATH for the
#               tracked-file guard; without git the structural rules still
#               decide what is build output, but nothing is checked against
#               the index.

# DESCRIPTION: Deletes the build output under `examples/`, `devops/` and
#   `modules/`, and leaves every source file alone.
#
#   The paths it recognises as build output:
#
#     1. Cargo target directories (`target/`): Cargo's `CACHEDIR.TAG` or its
#        `debug`/`release`/`doc` children mark them.
#     2. Generated build folders (`build/`): the host's build metadata, web
#        and shipping bundles, and staging folders. The folder must hold a
#        known generated child (`build_meta`, `web`, `pill_web_app` or
#        `pill_shipping_bundle`); a source folder that happens to be called
#        `build` matches none of them and is left alone.
#     3. .NET output (`bin/`, `obj/`) beside a project file or holding NuGet
#        restore records.
#     4. Python caches (`__pycache__/`, `.pytest_cache/`, `.mypy_cache/`,
#        `.ruff_cache/`, `*.pyc`, `*.pyo`).
#     5. Cooked shaders: a `*.wgsl` beside the `*.hlsl` of the same name,
#        which the build regenerates. A handwritten `.wgsl` has no `.hlsl`
#        beside it and is kept.
#     6. The hot-reload host's staged copies, build stamps and layout
#        records (`modules/pill_standalone_temp/`).
#     7. With `--include-generated`, the host-generated workspace members:
#        `modules/projects_generated/host_project_*` and the `host_module_*`
#        crates in an extension's `generated/` folder. The host writes them
#        again on every start; they are left in place by default because the
#        workspace is built through them.
#
#   Guards, so a wipe cannot destroy source:
#
#     - The tracked files are read once from `git ls-files`, and a candidate
#       that is tracked, or that contains a tracked file, is skipped and
#       reported instead of deleted.
#     - Every candidate must sit inside one of the three trees.
#     - A directory is recognised by its shape, never by its name alone.
#     - Junctions and symlinks are removed as links, never followed.
#     - `--dry-run` lists the targets and their sizes and deletes nothing.
#     - Without `--yes` the wipe asks for the word `wipe` on stdin, and
#       refuses when stdin is not a terminal.
#
#   Runtime state and downloaded caches under `modules/local/` (the font
#   cache, for example) are not build output and stay.

# USAGE: python devops/tools/wipe_build_artifacts.py [--dry-run] [--yes]
#                                                    [--include-generated]
#          --dry-run            list the paths and their sizes; delete nothing
#          --yes                skip the interactive `wipe` confirmation
#          --include-generated  also delete the host-generated workspace
#                               member crates (see above)

# EXAMPLE USAGE:
#   python devops/tools/wipe_build_artifacts.py --dry-run
#   python devops/tools/wipe_build_artifacts.py
#   python devops/tools/wipe_build_artifacts.py --yes --include-generated
#
#   Exit status: 0 when every target was deleted (or a dry run listed them),
#   1 when a target could not be deleted or the wipe was cancelled, 2 on a
#   usage error.

# --- SCRIPT ---

# Standard library
import argparse
import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, NamedTuple, Optional, Sequence, Set, Tuple


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]

# The trees a wipe covers. Build output never exists outside them.
SCOPE_DIRECTORY_NAMES = ("examples", "devops", "modules")
SCOPE_ROOTS = tuple(REPOSITORY_ROOT / name for name in SCOPE_DIRECTORY_NAMES)
MODULES_ROOT = REPOSITORY_ROOT / "modules"
EXTENSIONS_ROOT = MODULES_ROOT / "extensions"

# Directory names recognised as build output.
CARGO_TARGET_DIRECTORY_NAME = "target"
BUILD_DIRECTORY_NAME = "build"
DOTNET_OUTPUT_DIRECTORY_NAMES = ("bin", "obj")
PYTHON_CACHE_DIRECTORY_NAMES = ("__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache")
PYTHON_CACHE_FILE_SUFFIXES = (".pyc", ".pyo")
MODULE_RUNTIME_TEMP_DIRECTORY_NAME = "pill_standalone_temp"

# What a generated `build/` folder holds. A source folder called `build`
# holds none of these and is kept.
GENERATED_BUILD_MARKERS = ("build_meta", "web", "pill_web_app", "pill_shipping_bundle")

# The cooked shader convention: `build.rs` writes `X.wgsl` beside `X.hlsl`.
COOKED_SHADER_SUFFIX = ".wgsl"
COOKED_SHADER_SOURCE_SUFFIX = ".hlsl"

# The host-generated workspace members, deleted only under
# `--include-generated`.
PROJECT_WRAPPER_ROOT_NAME = "projects_generated"
PROJECT_WRAPPER_PREFIX = "host_project_"
MODULE_WRAPPER_PREFIX = "host_module_"
EXTENSION_GENERATED_DIRECTORY_NAME = "generated"

# Version control data and Python environments are never entered: no build
# output of ours lives inside them.
NEVER_ENTERED_DIRECTORY_NAMES = (".git", ".hg", ".svn", ".venv", "venv")

# The artifact groups the report prints, in this order.
GROUP_CARGO_TARGET = "cargo target"
GROUP_BUILD_OUTPUT = "build output"
GROUP_DOTNET_OUTPUT = ".NET output"
GROUP_PYTHON_CACHE = "python cache"
GROUP_COOKED_SHADER = "cooked shader"
GROUP_RUNTIME_TEMP = "hot-reload temp"
GROUP_GENERATED_WORKSPACE = "generated workspace member"
GROUP_LABELS = (
    (GROUP_CARGO_TARGET, "cargo target directories"),
    (GROUP_BUILD_OUTPUT, "generated build directories"),
    (GROUP_DOTNET_OUTPUT, ".NET bin/obj directories"),
    (GROUP_PYTHON_CACHE, "Python cache entries"),
    (GROUP_COOKED_SHADER, "cooked shader files"),
    (GROUP_RUNTIME_TEMP, "hot-reload temp directories"),
    (GROUP_GENERATED_WORKSPACE, "host-generated workspace members"),
)


class Artifact(NamedTuple):
    """One path the wipe deletes, with the group that named it."""

    path: Path
    group: str
    size_bytes: int


def normalized_key(path: os.PathLike) -> str:
    """Returns the comparison key of an absolute path, case-folded on Windows."""
    return os.path.normcase(os.path.abspath(str(path)))


def display_path(path: Path) -> str:
    """Returns `path` relative to the repository root, for the report."""
    try:
        return str(path.relative_to(REPOSITORY_ROOT))
    except ValueError:
        return str(path)


def format_size(byte_count: int) -> str:
    """Formats a byte count with the largest unit that keeps it readable."""
    value = float(byte_count)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024.0 or unit == "TiB":
            if unit == "B":
                return f"{int(value)} B"
            return f"{value:.1f} {unit}"
        value /= 1024.0
    return f"{byte_count} B"


def long_path(path: Path) -> str:
    """Returns an absolute path Windows accepts beyond the 260-character limit."""
    absolute = os.path.abspath(str(path))
    if os.name != "nt" or absolute.startswith("\\\\?\\"):
        return absolute
    if absolute.startswith("\\\\"):
        return "\\\\?\\UNC\\" + absolute[2:]
    return "\\\\?\\" + absolute


def is_link_or_junction(path: Path) -> bool:
    """True for a symlink or, on Windows, any reparse point (junctions too)."""
    target = long_path(path)
    if os.path.islink(target):
        return True
    if os.name != "nt":
        return False
    try:
        attributes = os.lstat(target).st_file_attributes
    except OSError:
        return False
    return bool(attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT)


def entry_is_directory(entry: os.DirEntry) -> bool:
    """True for a real directory; links and junctions are never entered."""
    try:
        entry_stat = entry.stat(follow_symlinks=False)
    except OSError:
        return False
    if not stat.S_ISDIR(entry_stat.st_mode):
        return False
    if os.name == "nt" and entry_stat.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        return False
    return True


def scan_directory(directory: Path) -> Tuple[List[str], List[str]]:
    """Returns the real child directories and the files of one directory.

    Links and junctions are reported as neither, so a walk never leaves the
    tree. An unreadable directory yields two empty lists with a warning.
    """
    child_directories: List[str] = []
    file_names: List[str] = []
    try:
        with os.scandir(long_path(directory)) as entries:
            for entry in entries:
                if entry_is_directory(entry):
                    child_directories.append(entry.name)
                elif entry.is_file(follow_symlinks=False):
                    file_names.append(entry.name)
    except OSError as error:
        print(f"warning: cannot list {display_path(directory)}: {error}", file=sys.stderr)
    return child_directories, file_names


def looks_like_cargo_target(path: Path) -> bool:
    """Recognises Cargo's target directory by its marker or profile children."""
    if (path / "CACHEDIR.TAG").is_file():
        return True
    return any((path / profile).is_dir() for profile in ("debug", "release", "doc"))


def looks_like_generated_build(path: Path) -> bool:
    """Recognises the generated `build/` folders by the output they hold."""
    return any((path / marker).is_dir() for marker in GENERATED_BUILD_MARKERS)


def looks_like_dotnet_output(path: Path) -> bool:
    """Recognises the `bin`/`obj` folder of a .NET project and nothing else."""
    project_suffixes = (".csproj", ".fsproj", ".vbproj", ".sln")
    if any(any(path.glob(f"*{suffix}")) for suffix in project_suffixes):
        return True
    if (path / "project.assets.json").is_file():
        return True
    if any(path.glob("*.nuget.dgspec.json")) or any(path.glob("*.nuget.g.props")):
        return True
    if any((path / configuration).is_dir() for configuration in ("Debug", "Release", "x64", "x86")):
        return True
    return any(child.is_dir() and child.name.startswith("net") for child in path.iterdir())


def is_generated_workspace_member(path: Path) -> bool:
    """True for a host-generated `host_project_*` or `host_module_*` crate."""
    wrapper_root = MODULES_ROOT / PROJECT_WRAPPER_ROOT_NAME
    if normalized_key(path.parent) == normalized_key(wrapper_root):
        return path.name.startswith(PROJECT_WRAPPER_PREFIX)
    if path.parent.name != EXTENSION_GENERATED_DIRECTORY_NAME:
        return False
    if not path.name.startswith(MODULE_WRAPPER_PREFIX):
        return False
    extensions_key = normalized_key(EXTENSIONS_ROOT)
    return any(normalized_key(parent) == extensions_key for parent in path.parents)


def classify_directory(path: Path, include_generated: bool) -> Optional[str]:
    """Returns the artifact group a directory belongs to, or None."""
    name = path.name
    try:
        if name == CARGO_TARGET_DIRECTORY_NAME and looks_like_cargo_target(path):
            return GROUP_CARGO_TARGET
        if name == BUILD_DIRECTORY_NAME and looks_like_generated_build(path):
            return GROUP_BUILD_OUTPUT
        if name in DOTNET_OUTPUT_DIRECTORY_NAMES and looks_like_dotnet_output(path):
            return GROUP_DOTNET_OUTPUT
        if name in PYTHON_CACHE_DIRECTORY_NAMES:
            return GROUP_PYTHON_CACHE
        if (
            name == MODULE_RUNTIME_TEMP_DIRECTORY_NAME
            and normalized_key(path.parent) == normalized_key(MODULES_ROOT)
        ):
            return GROUP_RUNTIME_TEMP
        if include_generated and is_generated_workspace_member(path):
            return GROUP_GENERATED_WORKSPACE
    except OSError:
        return None
    return None


def classify_file(file_name: str, directory: Path) -> Optional[str]:
    """Returns the artifact group a file belongs to, or None."""
    if file_name.endswith(PYTHON_CACHE_FILE_SUFFIXES):
        return GROUP_PYTHON_CACHE
    if file_name.endswith(COOKED_SHADER_SUFFIX):
        stem = file_name[: -len(COOKED_SHADER_SUFFIX)]
        if (directory / f"{stem}{COOKED_SHADER_SOURCE_SUFFIX}").is_file():
            return GROUP_COOKED_SHADER
    return None


def load_tracked_paths() -> Tuple[Set[str], Set[str]]:
    """Returns the tracked files and their ancestor directories as key sets.

    The sets are empty when git cannot be asked, and a warning says so: the
    structural rules are then the only guard. With them, a candidate that
    holds a tracked file is never deleted.
    """
    try:
        completed = subprocess.run(
            ["git", "ls-files", "-z", "--", *SCOPE_DIRECTORY_NAMES],
            cwd=str(REPOSITORY_ROOT),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        print(
            f"warning: git ls-files failed ({error}); the tracked-file guard is off",
            file=sys.stderr,
        )
        return set(), set()
    tracked_files: Set[str] = set()
    tracked_directories: Set[str] = set()
    repository_key = normalized_key(REPOSITORY_ROOT)
    for entry in completed.stdout.split(b"\0"):
        if not entry:
            continue
        file_key = normalized_key(REPOSITORY_ROOT / os.fsdecode(entry))
        tracked_files.add(file_key)
        parent_key = os.path.dirname(file_key)
        while parent_key.startswith(repository_key + os.sep):
            tracked_directories.add(parent_key)
            next_parent_key = os.path.dirname(parent_key)
            if next_parent_key == parent_key:
                break
            parent_key = next_parent_key
    return tracked_files, tracked_directories


def is_protected(path: Path, tracked_files: Set[str], tracked_directories: Set[str]) -> bool:
    """True when the path itself, or anything below it, is tracked by git."""
    key = normalized_key(path)
    return key in tracked_files or key in tracked_directories


def measure_path(path: Path) -> int:
    """Returns the bytes a path holds; unreadable entries count as zero."""
    total_bytes = 0
    pending = [path]
    while pending:
        current = pending.pop()
        if is_link_or_junction(current):
            continue
        target = long_path(current)
        try:
            if os.path.isdir(target):
                with os.scandir(target) as entries:
                    for entry in entries:
                        if entry_is_directory(entry):
                            pending.append(current / entry.name)
                        else:
                            try:
                                total_bytes += entry.stat(follow_symlinks=False).st_size
                            except OSError:
                                continue
            else:
                total_bytes += os.lstat(target).st_size
        except OSError:
            continue
    return total_bytes


def discover_artifacts(
    scope_roots: Sequence[Path],
    tracked_files: Set[str],
    tracked_directories: Set[str],
    include_generated: bool,
) -> Tuple[List[Artifact], List[Tuple[Path, str]]]:
    """Collects every build-output path under the scope roots.

    A directory that is recorded is pruned, so the nested `build` folder
    inside a target directory is not listed twice. A candidate holding a
    tracked file is reported in the second list instead of listed for
    deletion; the walk then descends into it, because build output can sit
    beside tracked files.
    """
    artifacts: List[Artifact] = []
    skipped: List[Tuple[Path, str]] = []
    for scope_root in scope_roots:
        pending = [scope_root]
        while pending:
            directory = pending.pop()
            child_directories, file_names = scan_directory(directory)
            for child_name in sorted(child_directories):
                if child_name in NEVER_ENTERED_DIRECTORY_NAMES:
                    continue
                child_path = directory / child_name
                group = classify_directory(child_path, include_generated)
                if group is None:
                    pending.append(child_path)
                    continue
                if is_protected(child_path, tracked_files, tracked_directories):
                    skipped.append((child_path, "contains tracked files"))
                    pending.append(child_path)
                    continue
                artifacts.append(Artifact(child_path, group, measure_path(child_path)))
            for file_name in sorted(file_names):
                group = classify_file(file_name, directory)
                if group is None:
                    continue
                file_path = directory / file_name
                if is_protected(file_path, tracked_files, tracked_directories):
                    skipped.append((file_path, "tracked file"))
                    continue
                artifacts.append(Artifact(file_path, group, measure_path(file_path)))
    return artifacts, skipped


def path_is_inside_scope(path: Path) -> bool:
    """Guards the deleter: the path must sit below one of the scope roots."""
    path_key = normalized_key(path)
    for scope_root in SCOPE_ROOTS:
        scope_key = normalized_key(scope_root)
        if path_key == scope_key:
            return False
        if path_key.startswith(scope_key + os.sep):
            return True
    return False


def remove_link(path: Path) -> None:
    """Removes a symlink or junction itself, never what it points at."""
    target = long_path(path)
    if os.name == "nt":
        try:
            os.rmdir(target)
            return
        except FileNotFoundError:
            return
        except OSError:
            pass
    try:
        os.unlink(target)
    except FileNotFoundError:
        pass


def remove_entry(path: Path) -> None:
    """Deletes a file or a whole directory tree below `path`.

    An entry that is already gone is not an error. A directory tree goes
    through `shutil.rmtree`, which reads each child's type from the
    `scandir` cache instead of asking the filesystem again, and removes a
    junction or symlink it meets instead of entering it.
    """
    target = long_path(path)
    if is_link_or_junction(path):
        remove_link(path)
        return
    if os.path.isdir(target):
        try:
            shutil.rmtree(target)
        except FileNotFoundError:
            pass
        return
    try:
        os.unlink(target)
    except FileNotFoundError:
        pass


def remove_artifact(path: Path) -> Optional[str]:
    """Deletes one target, returning the reason it failed or None."""
    if not path_is_inside_scope(path):
        return "refused: path is outside the wipe scope"
    try:
        remove_entry(path)
    except OSError as error:
        return str(error)
    return None


def print_group_summary(artifacts: Sequence[Artifact]) -> None:
    """Prints one line per artifact group: how many paths and how many bytes."""
    counts: Dict[str, int] = {}
    sizes: Dict[str, int] = {}
    for artifact in artifacts:
        counts[artifact.group] = counts.get(artifact.group, 0) + 1
        sizes[artifact.group] = sizes.get(artifact.group, 0) + artifact.size_bytes
    for group, label in GROUP_LABELS:
        if group in counts:
            print(f"  {label:<38} {counts[group]:>4}  {format_size(sizes[group]):>10}")


def print_artifact_paths(artifacts: Sequence[Artifact]) -> None:
    """Prints one line per target: its path and its size."""
    for artifact in artifacts:
        print(f"  {display_path(artifact.path)}  ({format_size(artifact.size_bytes)})")


def report_skipped(skipped: Sequence[Tuple[Path, str]]) -> None:
    """Prints the candidates that were left in place, with the reason."""
    if not skipped:
        return
    print("\nLeft in place:")
    for path, reason in skipped:
        print(f"  {display_path(path)}  ({reason})")


def ask_for_confirmation(artifact_count: int, total_bytes: int) -> bool:
    """Asks for the word `wipe`; refuses when stdin is not a terminal."""
    if not sys.stdin.isatty():
        print("error: the wipe needs --yes when stdin is not a terminal", file=sys.stderr)
        return False
    print(
        f"\nDelete these {artifact_count} paths ({format_size(total_bytes)})? "
        "Type 'wipe' to continue: ",
        end="",
        flush=True,
    )
    if sys.stdin.readline().strip().lower() != "wipe":
        print("Wipe cancelled.")
        return False
    return True


def main(arguments: Optional[Sequence[str]] = None) -> int:
    """Runs the wipe and returns the process exit status."""
    parser = argparse.ArgumentParser(
        description="Deletes the build output under examples/, devops/ and modules/.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="list the paths and their sizes; delete nothing",
    )
    parser.add_argument(
        "--yes",
        action="store_true",
        help="skip the interactive confirmation",
    )
    parser.add_argument(
        "--include-generated",
        action="store_true",
        help="also delete the host-generated workspace member crates",
    )
    options = parser.parse_args(arguments)

    missing_scopes = [scope_root for scope_root in SCOPE_ROOTS if not scope_root.is_dir()]
    if missing_scopes:
        missing_names = ", ".join(display_path(path) for path in missing_scopes)
        print(f"error: not inside a repository checkout; missing {missing_names}", file=sys.stderr)
        return 2

    tracked_files, tracked_directories = load_tracked_paths()
    artifacts, skipped = discover_artifacts(
        SCOPE_ROOTS, tracked_files, tracked_directories, options.include_generated
    )
    artifacts.sort(key=lambda artifact: display_path(artifact.path))

    scope_label = ", ".join(scope_root.name + "/" for scope_root in SCOPE_ROOTS)
    print(f"Build output under {scope_label}:")
    if not artifacts:
        print("  none found; nothing to wipe")
        report_skipped(skipped)
        return 0

    print_group_summary(artifacts)
    total_bytes = sum(artifact.size_bytes for artifact in artifacts)
    print(f"\n{len(artifacts)} paths, {format_size(total_bytes)} in total")

    if options.dry_run:
        print("\nWould delete:")
        print_artifact_paths(artifacts)
        report_skipped(skipped)
        return 0

    if not options.yes and not ask_for_confirmation(len(artifacts), total_bytes):
        return 1

    failed_count = 0
    removed_bytes = 0
    for artifact in artifacts:
        failure = remove_artifact(artifact.path)
        if failure is None:
            removed_bytes += artifact.size_bytes
            print(f"  deleted {display_path(artifact.path)}")
        else:
            failed_count += 1
            print(f"  failed  {display_path(artifact.path)}: {failure}", file=sys.stderr)

    print(f"\nRemoved {len(artifacts) - failed_count} paths, {format_size(removed_bytes)}")
    if failed_count:
        print(f"{failed_count} paths could not be deleted (still in use?)", file=sys.stderr)
    report_skipped(skipped)
    return 0 if failed_count == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
