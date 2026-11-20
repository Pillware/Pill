"""Generate HTML or JSON documentation for this Cargo workspace.

Without flags, the script generates classic rustdoc HTML with stable
``cargo doc`` and opens it in the default browser after a successful build.
Passing ``--json`` switches to nightly rustdoc JSON generation for downstream
tools such as ``generate_documentation_markdown.py`` and does not open a browser.

Unlike ``cargo doc --workspace``, ``cargo rustdoc`` accepts the unstable JSON
output option but operates on only one selected package target at a time. JSON
mode therefore discovers workspace members, selects each documentation target,
and invokes nightly rustdoc once per target. HTML mode documents the workspace
in one Cargo command.

Rustdoc's JSON format is unstable, so JSON mode requires both a nightly
toolchain and Cargo's ``-Z unstable-options`` switch. HTML mode uses the normal
stable toolchain and produces directly browsable rustdoc pages.

Requirements:
  - Python 3.8+
  - Rustup with the nightly toolchain installed

Usage:
  python devops/docs/generate_documentation.py
  python devops/docs/generate_documentation.py --json
  python devops/docs/generate_documentation.py --dry-run
  python devops/docs/generate_documentation.py --json --dry-run

Output:
  HTML: target/doc/index.html and crate subdirectories
  JSON: target/doc/<crate_name>.json
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List, Sequence, Tuple


# Resolve paths from this file instead of the caller's current directory. This
# lets developers invoke the script from the repository root, another folder,
# or an IDE task without changing its behavior.
REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
# The Cargo workspace lives in modules/, and Cargo discovers modules/.cargo/
# config.toml by walking up from its current directory, so every command runs
# from there rather than from the repository root above it.
WORKSPACE_ROOT = REPOSITORY_ROOT / "modules"
MANIFEST_PATH = WORKSPACE_ROOT / "Cargo.toml"

# Cargo metadata uses several target-kind labels for library-like targets. All
# of these are selected through Cargo's single ``--lib`` option, including
# procedural macro and C-compatible dynamic-library crates.
LIBRARY_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}

# Some declared features are alternatives that cannot be enabled together, and
# Cargo's ``--all-features`` would turn every one of them on. ``profiling`` and
# ``profiling-minimal`` configure incompatible instrumentation modes in
# pill_core and pill_engine, and pill_standalone refuses to compile with both
# its ``dev`` and ``shipping`` postures selected. Documentation therefore
# enables the full ``profiling`` mode and the default ``dev`` posture, and
# omits only the alternative features listed here.
EXCLUDED_DOCUMENTATION_FEATURES = {"profiling-minimal", "shipping"}

# The development host writes and prunes these launcher crates on every start
# under the gitignored ``generated/`` folders. They only re-export a project or
# extension, so documenting them would duplicate that crate's API under a shim
# name - and would race the host replacing their manifests mid-run.
GENERATED_LAUNCHER_PREFIXES = ("host_project_", "host_module_")


# =============================================================================
# Subprocess execution
# =============================================================================

# Keep external command execution in one helper so every Cargo invocation uses
# the same workspace, error behavior, encoding, and readable command logging.
def run(
    command: Sequence[str], *, capture_output: bool = False
) -> subprocess.CompletedProcess:
    """Run an external command from the Cargo workspace root.

    Args:
        command: Executable and arguments passed directly to ``subprocess``.
            A sequence is used instead of a shell string so paths and package
            names do not require platform-specific escaping.
        capture_output: Capture standard output when the caller needs to parse
            it. Standard error remains visible so Cargo warnings are not lost.

    Returns:
        The completed process. When ``capture_output`` is true, its ``stdout``
        attribute contains UTF-8 text.

    Raises:
        FileNotFoundError: The requested executable is unavailable.
        subprocess.CalledProcessError: The command exits unsuccessfully.
    """
    # Echo an exact, copyable representation before execution. Flushing keeps
    # this line ahead of Cargo output even when stdout is redirected by CI.
    print("+", subprocess.list2cmdline(command), flush=True)

    # Running from WORKSPACE_ROOT is important because Cargo discovers the
    # workspace's .cargo/config.toml by walking up from its current directory.
    return subprocess.run(
        command,
        cwd=WORKSPACE_ROOT,
        check=True,
        # Cargo metadata writes its JSON to stdout. Documentation commands keep
        # stdout attached to the terminal so build progress remains visible.
        stdout=subprocess.PIPE if capture_output else None,
        text=True,
        encoding="utf-8",
    )


# =============================================================================
# Workspace discovery
# =============================================================================

# Query Cargo rather than parsing Cargo.toml ourselves. Cargo metadata accounts
# for virtual workspaces, renamed packages, and any future manifest changes.
def workspace_packages() -> List[Dict[str, Any]]:
    """Return only this workspace's packages in Cargo metadata order.

    ``cargo metadata`` can include dependency packages in its ``packages``
    array. Although ``--no-deps`` minimizes that data, filtering by the stable
    package IDs in ``workspace_members`` makes the intent explicit and prevents
    dependencies from accidentally receiving their own rustdoc job. The
    host-generated launcher crates are dropped for the reasons given beside
    ``GENERATED_LAUNCHER_PREFIXES``.

    Returns:
        Cargo package objects belonging directly to the workspace.
    """
    # The metadata command itself does not need nightly. Stable Cargo provides
    # the package and target fields used by this script.
    result = run(
        [
            "cargo",
            "metadata",
            "--manifest-path",
            str(MANIFEST_PATH),
            "--no-deps",
            "--format-version",
            "1",
        ],
        capture_output=True,
    )

    # Parse only stdout; Cargo warnings are emitted separately on stderr and do
    # not contaminate the JSON document.
    metadata = json.loads(result.stdout)

    # Set membership avoids repeatedly scanning the workspace member list when
    # filtering larger workspaces. Host-generated launcher crates are skipped:
    # they are regenerated on every host start and expose no API of their own.
    workspace_members = set(metadata["workspace_members"])
    return [
        package
        for package in metadata["packages"]
        if package["id"] in workspace_members
        and not package["name"].startswith(GENERATED_LAUNCHER_PREFIXES)
    ]


# Translate Cargo metadata targets into the selector syntax accepted by
# ``cargo rustdoc``. One package may describe libraries, binaries, examples,
# tests, and benchmarks, but the command can document only one at a time.
def documentation_targets(package: Dict[str, Any]) -> List[Tuple[str, str]]:
    """Select Cargo's normal documentation targets for one package.

    Library-like targets take precedence, matching ``cargo doc``'s default
    behavior. A package without a library falls back to every binary target
    whose Cargo metadata permits documentation. Examples, tests, and benches
    are intentionally excluded.

    Args:
        package: One package object returned by ``cargo metadata``.

    Returns:
        ``(selector, target_name)`` pairs. The selector is ``--lib`` or
        ``--bin``; the name is retained for status messages and is also passed
        after ``--bin`` when required by Cargo.
    """
    # A manifest can opt a target out with ``doc = false``. Older metadata may
    # omit the field, in which case Cargo's default is to allow documentation.
    targets = [target for target in package["targets"] if target.get("doc", True)]

    # Cargo represents each package's library as one logical target even when
    # it emits a specialized crate type such as cdylib or proc-macro.
    library_targets = [
        target
        for target in targets
        if LIBRARY_KINDS.intersection(target["kind"])
    ]
    if library_targets:
        # ``--lib`` does not accept a following name. Keep the metadata name in
        # the tuple only so progress output identifies the generated crate.
        return [("--lib", library_targets[0]["name"])]

    # Binary-only packages need an explicit name. Returning every eligible bin
    # also handles packages that declare more than one documented executable.
    return [
        ("--bin", target["name"])
        for target in targets
        if "bin" in target["kind"]
    ]


def documentation_feature_arguments(package: Dict[str, Any]) -> List[str]:
    """Select all package features except mutually exclusive alternatives.

    Cargo metadata exposes the package's complete feature-name map. Expanding
    that map into ``--features`` arguments gives this script the coverage of an
    all-features documentation build while allowing the full ``profiling`` mode
    to win over ``profiling-minimal``.

    Packages without declared features need no feature-selection arguments.
    """
    available_features = sorted(package.get("features", {}))
    selected_features = [
        feature
        for feature in available_features
        if feature not in EXCLUDED_DOCUMENTATION_FEATURES
    ]
    if not selected_features:
        return []

    # Cargo accepts a comma-separated list as one argument. Keeping it together
    # also makes dry-run output compact and directly copyable into a terminal.
    return ["--features", ",".join(selected_features)]


# A workspace-wide Cargo command needs package-qualified feature names so
# features belonging to different members can be enabled without ambiguity.
def workspace_documentation_feature_arguments(
    packages: Sequence[Dict[str, Any]],
) -> List[str]:
    """Select compatible features across all documented workspace packages.

    Args:
        packages: Cargo metadata objects for every selected workspace member.

    Returns:
        An empty list when the workspace declares no selectable features, or a
        Cargo ``--features`` pair containing ``package/feature`` selectors.
    """
    # Qualifying every feature with its package makes the single HTML command
    # equivalent to each JSON job's package-local feature selection.
    selected_features = sorted(
        "{}/{}".format(package["name"], feature)
        for package in packages
        for feature in package.get("features", {})
        if feature not in EXCLUDED_DOCUMENTATION_FEATURES
    )
    if not selected_features:
        return []

    # Cargo accepts a comma-separated feature list as one argument. This also
    # keeps dry-run output compact enough to paste directly into a terminal.
    return ["--features", ",".join(selected_features)]


# =============================================================================
# HTML and JSON command construction
# =============================================================================


# Classic HTML supports the entire workspace directly through stable Cargo.
def generate_html_documentation(
    packages: Sequence[Dict[str, Any]], *, dry_run: bool
) -> None:
    """Generate and open classic private-item HTML for the whole workspace.

    Args:
        packages: Workspace packages used to construct compatible feature flags.
        dry_run: Print the Cargo command without executing it when true.
    """
    # ``cargo doc`` creates the usual browsable HTML tree under target/doc. The
    # stable toolchain is intentional; nightly is required only for JSON mode.
    command = [
        "cargo",
        "doc",
        "--manifest-path",
        str(MANIFEST_PATH),
        "--workspace",
        "--no-deps",
    ]

    # Enable every compatible workspace feature, retaining full profiling while
    # excluding only the mutually exclusive profiling-minimal alternative.
    command.extend(workspace_documentation_feature_arguments(packages))

    # Cargo exposes private-item documentation as a normal stable doc option.
    # ``--open`` launches the generated workspace documentation only after the
    # HTML build succeeds; JSON mode uses a separate command and remains headless.
    command.extend(["--document-private-items", "--open"])

    if dry_run:
        # Match run()'s readable Windows quoting without starting compilation.
        print("+", subprocess.list2cmdline(command), flush=True)
    else:
        run(command)


# JSON requires one nightly ``cargo rustdoc`` command per selected target.
def generate_json_package(
    package: Dict[str, Any],
    selector: str,
    target_name: str,
    *,
    dry_run: bool,
) -> None:
    """Generate private-item JSON documentation for one package target.

    Args:
        package: Cargo metadata object for the owning package.
        selector: Cargo target selector, either ``--lib`` or ``--bin``.
        target_name: Human-readable target name, and the value for ``--bin``.
        dry_run: Print the command without invoking Cargo when true.
    """
    # ``+nightly`` is command-scoped. It does not change the repository's or
    # user's default Rust toolchain for normal builds and tests.
    command = [
        "cargo",
        "+nightly",
        "rustdoc",
        # Cargo currently gates its --output-format option independently from
        # rustdoc, so enabling Cargo's unstable options is also necessary.
        "-Z",
        "unstable-options",
        "--manifest-path",
        str(MANIFEST_PATH),
        "-p",
        package["name"],
        selector,
    ]
    if selector == "--bin":
        # Unlike --lib, --bin must be followed by the exact Cargo target name.
        command.append(target_name)

    # Enable every compatible feature before adding output options. This keeps
    # full ``profiling`` enabled but excludes the mutually exclusive
    # ``profiling-minimal`` mode in packages that declare it.
    command.extend(documentation_feature_arguments(package))

    # Options before ``--`` belong to Cargo. Options after it are forwarded
    # verbatim to the nightly rustdoc process.
    command.extend(
        [
            "--output-format",
            "json",
            "--",
            # Private functions, types, fields, and modules are included in the
            # JSON graph instead of restricting output to the public API.
            "--document-private-items",
        ]
    )

    if dry_run:
        # Dry-run mode still performs Cargo metadata discovery, ensuring these
        # printed commands correspond to the workspace's current packages.
        print("+", subprocess.list2cmdline(command), flush=True)
    else:
        run(command)


# =============================================================================
# Command-line orchestration and error reporting
# =============================================================================

# Keep main's return value as an integer so the module remains easy to call from
# tests while the __main__ guard translates it into a process exit status.
def main() -> int:
    """Parse output mode, discover the workspace, and generate documentation."""
    parser = argparse.ArgumentParser(
        description=(
            "Generate classic rustdoc HTML, or nightly rustdoc JSON with --json."
        )
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="generate nightly rustdoc JSON instead of classic HTML",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print Cargo commands without running them",
    )
    args = parser.parse_args()

    try:
        # Both modes need metadata for feature selection. JSON additionally uses
        # it to expand every package into target-specific rustdoc jobs.
        packages = workspace_packages()

        if args.json:
            # Expand JSON work before compilation so progress counts are exact
            # and a workspace without documentable targets fails explicitly.
            jobs = [
                (package, selector, target_name)
                for package in packages
                for selector, target_name in documentation_targets(package)
            ]
            if not jobs:
                print("No documentable workspace targets found.", file=sys.stderr)
                return 1

            # Run sequentially because Cargo processes already parallelize their
            # own compilation and concurrent jobs would contend for target locks.
            for index, (package, selector, target_name) in enumerate(
                jobs, start=1
            ):
                print(
                    f"\n[{index}/{len(jobs)}] Generating JSON for "
                    f"{package['name']} ({target_name})",
                    flush=True,
                )
                generate_json_package(
                    package,
                    selector,
                    target_name,
                    dry_run=args.dry_run,
                )
        else:
            # HTML is the default and Cargo can generate it workspace-wide in a
            # single invocation, preserving classic rustdoc's shared resources.
            print("\nGenerating classic rustdoc HTML for the workspace", flush=True)
            generate_html_documentation(packages, dry_run=args.dry_run)
    except FileNotFoundError as error:
        # Usually indicates that Python cannot locate cargo/rustup's proxy on
        # PATH. Include the executable name to make setup failures actionable.
        print(f"Required executable not found: {error.filename}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        # Cargo already prints its detailed diagnostic. Preserve its status so
        # shells and CI jobs can reliably detect the failed documentation run.
        print(f"Command failed with exit code {error.returncode}.", file=sys.stderr)
        return error.returncode or 1
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        # Keep malformed or unexpectedly shaped metadata errors concise instead
        # of exposing an implementation traceback to normal script users.
        print(f"Could not parse Cargo workspace metadata: {error}", file=sys.stderr)
        return 1

    # Make dry runs unmistakable; otherwise users may look for files that the
    # script deliberately did not generate.
    if args.dry_run:
        print("\nDry run complete; no documentation was generated.")
    else:
        # Cargo may be redirected by the Markdown pipeline through
        # CARGO_TARGET_DIR. Report that effective destination instead of always
        # claiming output was written into the workspace's normal target.
        configured_target_directory = os.environ.get("CARGO_TARGET_DIR")
        if configured_target_directory:
            target_directory = Path(configured_target_directory)
            if not target_directory.is_absolute():
                target_directory = WORKSPACE_ROOT / target_directory
        else:
            target_directory = WORKSPACE_ROOT / "target"
        output_directory = target_directory.resolve() / "doc"
        if args.json:
            print(f"\nRustdoc JSON generated under {output_directory}")
        else:
            print(f"\nRustdoc HTML generated under {output_directory}")
    return 0


# Convert main's result into the process exit code only during direct execution;
# importing this module for tests or tooling remains side-effect free.
if __name__ == "__main__":
    raise SystemExit(main())
