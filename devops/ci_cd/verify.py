#!/usr/bin/env python3
"""
The single list of per-commit checks, run by CI and by anyone verifying a change.

REQUIREMENTS
  - Python 3.8+ (standard library only).
  - Rust stable with rustfmt and clippy, and the `wasm32-unknown-unknown`
    target for the `wasm-clippy` step.
  - Optional: `cargo-nextest`, used by the two unit-test steps when it is
    installed, and skipped in favour of `cargo test` when it is not.

DESCRIPTION
    Steps run in the order below. Cargo steps run from `modules/`, Python steps
    from the repository root, and every Python step uses this interpreter. The
    run stops at the first failure unless `--keep-going` is given, and always
    ends with a PASS/FAIL table. A failing quick step (formatting, clippy, the
    lints) ends the run even under `--keep-going`: the slow steps after it
    cannot say anything about a change that does not yet lint, and twenty
    minutes of them after a three-second formatting failure is pure waste.

    `--changed` runs a step that declares `paths` only when a changed file
    (uncommitted or untracked) matches one of them: the engine feature drift
    check (five minutes) only looks at what the Cargo manifests resolve to, so
    a change that touches none of them skips it.

    Under agent_run.py (`AGENT_RUN_DIRECTORY` set) every finished step is
    written to `timings.json` in the run's folder, so `work checks --steps`
    shows per-step medians across runs.

    The unit-test steps run through `cargo nextest run` when nextest is
    installed: every test gets its own process and its own line with its
    duration, and `modules/.config/nextest.toml` holds the settings (no
    retries, every failure reported). nextest does not run doctests, so the
    same step runs `cargo test --doc` beside it. Without nextest the step is a
    plain `cargo test`, which runs both, and says so.

    `--quick` keeps only the inner-loop steps (formatting, clippy and the
    lints): about a minute on a warm cache, and enough to catch most mistakes
    before the full run.

    The `editor` step is the end-to-end lane. It first runs the host-tier
    `test_editor_revision.py`, which builds a narrowed `pill_standalone` and
    would re-split the shared DLLs if it ran later; then it builds
    `pill_editor` (the uplift) and runs `test_editor_egui.py` - serially,
    with its own timeout. It is the slowest step, about fifteen minutes on a
    warm cache, and it never runs in `--quick`.

    Under GitHub Actions (`GITHUB_ACTIONS=true`) each step's output is a
    collapsible log group and the summary table is also written to the job
    summary, so CI runs this script as one step and still shows every check.

USAGE
  python devops/ci_cd/verify.py [--quick] [--only NAME ...] [--skip NAME ...]
                                [--keep-going] [--list]

    --quick          run only the steps marked quick
    --only NAME ...  run only the named steps (see --list)
    --skip NAME ...  run every selected step except these
    --keep-going     run the remaining steps after a failure
    --list           print the steps and their commands, then exit

EXAMPLE USAGE
  python devops/ci_cd/verify.py
  python devops/ci_cd/verify.py --quick
  python devops/ci_cd/verify.py --only clippy test
  python devops/ci_cd/verify.py --skip wasm-clippy --keep-going

  Exit status: 0 when every selected step passed, 1 when any failed or was
  skipped after a failure, 2 on a usage error.

--- SCRIPT ---
"""

import argparse
import fnmatch
import json
import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Sequence

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPOSITORY_ROOT / "devops"))

from core.projects import NATIVE_PROJECT_ROOT, repository_relative  # noqa: E402

# Set by GitHub Actions; switches on log groups and the job summary.
RUNNING_IN_GITHUB_ACTIONS = os.environ.get("GITHUB_ACTIONS") == "true"
MODULES_ROOT = REPOSITORY_ROOT / "modules"
PYTHON = sys.executable

# The engine's development-posture feature, shared by the check and test steps.
# The host has no feature of its own any more: it pins `pill_engine/development`
# in its manifest, and the `PILL_*` switches decide at startup which watchers run.
DEVELOPMENT_FEATURES = "pill_engine/development"
# The default test lane must compile without `development`, so the shipping
# code paths (`cfg(not(feature = "development"))` in the engine) are built and
# tested. Cargo unifies features across one invocation, so every member that
# reaches the engine's development feature has to stay out of the lane: the two
# frontends through their host dependency, and the host itself, which pins
# `pill_engine/development` in its own manifest so the modules it builds resolve the
# graph its dylibs were built with. The development lane runs the whole workspace,
# host included. The `default-lane-features` step fails if a new member brings
# development back.
DEFAULT_LANE = [
    "--workspace",
    "--exclude",
    "pill_standalone",
    # The egui editor frontend links `pill_development_host` with its default features, so
    # leaving it in the default lane turns `pill_engine_core/development` on for
    # every crate in it - the lane exists to test the shipping path, and the
    # development lane below covers the other one.
    "--exclude",
    "pill_editor",
    # The host pins the engine's development feature, so its unit tests run in the
    # development lane only.
    "--exclude",
    "pill_development_host",
]

# The unit-test steps run through nextest when it is installed, and through
# `cargo test` when it is not, so the gate still runs on a machine that never
# installed it. `cargo-nextest` is the binary `cargo nextest` dispatches to.
NEXTEST = shutil.which("cargo-nextest")
# The nextest profile the test steps select. It is the `ci` profile of
# `modules/.config/nextest.toml`, so a local run of this gate and a CI run
# execute the tests with the same settings.
NEXTEST_PROFILE = "ci"


@dataclass
class Step:
    """One check: a name for --only, a title, and the commands it runs."""

    name: str
    title: str
    commands: List[List[str]]
    working_directory: Path = MODULES_ROOT
    quick: bool = False
    # Variables set for this step only, on top of the inherited environment.
    environment: Dict[str, str] = field(default_factory=dict)
    # Printed above the step when its commands had to adapt to this machine.
    note: str = ""
    # Commands run after the step's own, whether they passed or not: a step
    # that rewrites tracked files this way leaves the tree as it found it, so a
    # failing step cannot leave a build artifact behind for a later commit to
    # pick up.
    cleanup: List[List[str]] = field(default_factory=list)
    # With `--changed`, the step runs only when a changed path matches one of
    # these globs (fnmatch, `*` spans folders); empty means it always runs.
    paths: List[str] = field(default_factory=list)


def unit_test_step(name: str, title: str, lane: List[str]) -> Step:
    """The unit-test step for `lane`, on nextest when it is installed.

    nextest gives every test its own process and prints a duration per test,
    which `cargo test` cannot. It does not run doctests, so they keep their own
    command; without nextest a plain `cargo test` runs the unit tests and the
    doctests together.
    """
    if NEXTEST is None:
        return Step(
            name,
            title,
            [["cargo", "test", *lane]],
            note="cargo-nextest is not installed; falling back to `cargo test`",
        )
    return Step(
        name,
        title,
        [
            ["cargo", "nextest", "run", "--profile", NEXTEST_PROFILE, *lane],
            ["cargo", "test", "--doc", *lane],
        ],
    )


STEPS: List[Step] = [
    Step("fmt", "Formatting", [["cargo", "fmt", "--check"]], quick=True),
    # Also the default posture's compile check: the workspace has no
    # `default-members`, so a bare `cargo clippy` covers every member with its
    # default features - exactly what `cargo check --workspace` would compile.
    #
    # `--all-targets` adds the test, example and bench targets. Without it a
    # warning in a `#[cfg(test)]` module went unseen: those targets are built
    # by `cargo test` and nextest, and neither denies warnings.
    Step(
        "clippy",
        "Clippy (default posture, every target)",
        [["cargo", "clippy", "--all-targets", "--", "-D", "warnings"]],
        quick=True,
    ),
    # The posture checks below run clippy rather than `cargo check` so they can
    # deny warnings: `cargo check` takes no rustc flags, and an `RUSTFLAGS`
    # variable would replace the workspace's `-C prefer-dynamic` instead of
    # adding to it (see `modules/.cargo/config.toml`). `-- -D warnings` reaches
    # rustc without touching rustflags, so a warning in a posture fails its
    # step.
    Step(
        "check-development",
        "Clippy (development posture)",
        [["cargo", "clippy", "--workspace", "--features", DEVELOPMENT_FEATURES, "--", "-D", "warnings"]],
    ),
    Step(
        "bundle-stub",
        "Shipping bundle stub (a generation is never committed)",
        # The tracked bundle files are build scratch: a generation names one
        # project and compiles, so a committed one would let the shipping
        # posture type-check - and ship - against whichever project happened
        # to be generated last. This runs before any step generates, and
        # builds nothing.
        [[PYTHON, "devops/tests/test_shipping_smoke.py", "--stub-only"]],
        working_directory=REPOSITORY_ROOT,
        quick=True,
    ),
    Step(
        "check-shipping",
        "Clippy (shipping posture)",
        # `-p pill_standalone` alone: with `--workspace`, `pill_editor` would
        # bring `pill_development_host` and the dev posture back (see the crate's manifest).
        #
        # The bundle is generated first: the committed stub is deliberately
        # project-agnostic and refuses to compile, so without this step the
        # shipping posture would either fail on the stub or type-check against
        # a stale generation of some other project.
        [
            # The generator is named by absolute path: the step runs from
            # `modules/`, where the cargo command below has to run.
            [PYTHON, str(REPOSITORY_ROOT / "devops" / "tools" / "generate_shipping_bundle.py"), repository_relative(NATIVE_PROJECT_ROOT)],
            [
                "cargo", "clippy", "-p", "pill_standalone",
                "--no-default-features", "--features", "shipping",
                "--", "-D", "warnings",
            ],
        ],
        # The generation is this step's own input, not the tree's state: put
        # the stub back, even when clippy fails, so no gate run leaves a
        # generated bundle for a later commit to sweep in.
        cleanup=[
            [PYTHON, str(REPOSITORY_ROOT / "devops" / "tools" / "generate_shipping_bundle.py"), "--restore-stub"],
        ],
    ),
    Step(
        "check-rendering",
        "Clippy (rendering posture)",
        [["cargo", "clippy", "-p", "pill_standalone", "--features", "rendering", "--", "-D", "warnings"]],
    ),
    Step(
        "default-lane-features",
        "Default test lane is free of development",
        [[PYTHON, "devops/ci_cd/verify.py", "--assert-default-lane-features"]],
        working_directory=REPOSITORY_ROOT,
    ),
    unit_test_step("test", "Unit tests (default lane, no development)", DEFAULT_LANE),
    unit_test_step("test-development", "Unit tests (development lane)", ["--workspace", "--features", DEVELOPMENT_FEATURES]),
    Step(
        "test-shipping-engine",
        "Engine refuses layout changes in the shipping posture",
        # The shipping posture has no `development` feature: the migration,
        # re-homing and relayout code is absent, and the registration refusals
        # are what stops a type being registered again with a changed layout.
        # The unit tests prove all three refusal paths in this build.
        [["cargo", "test", "-p", "pill_engine_core"]],
    ),
    Step(
        "test-metrics",
        "Unit tests (metrics posture)",
        # The metrics feature is off everywhere by default, so the per-frame
        # measurements - and the recorder tests that count them - are compiled
        # by no other step. The two crates that record are enough: the engine
        # emits from its own pass, the runtime records the frontend's frame.
        [
            [
                "cargo", "test",
                "-p", "pill_engine_core",
                "-p", "pill_player",
                "--features", "metrics",
            ],
        ],
    ),
    Step(
        "coding-standards",
        "Coding standards lint",
        # Repository-root scope, through the lint's own `is_linted`: the
        # changed-files hook imports the same predicate, so the two select the
        # same file set - every example, the test project and the shipping
        # bundle included.
        # The self test first proves the egui-state rule still fails its
        # planted violation; the run then holds every .rs file to the rules.
        [
            [PYTHON, "devops/tests/test_coding_standards.py", "--self-test"],
            [PYTHON, "devops/tests/test_coding_standards.py"],
        ],
        working_directory=REPOSITORY_ROOT,
        quick=True,
    ),
    Step(
        "suite-standards",
        "Test suite lint (headers, shared helpers, sleeps)",
        # The self test first proves each rule still fails its planted
        # violation; the run then holds every devops/tests suite to them.
        [
            [PYTHON, "devops/tests/test_suite_standards.py", "--self-test"],
            [PYTHON, "devops/tests/test_suite_standards.py"],
        ],
        working_directory=REPOSITORY_ROOT,
        quick=True,
    ),
    Step(
        "renderer-boundaries",
        "Renderer boundaries",
        # The self test first proves the checker still catches each break.
        [
            [PYTHON, "devops/tests/test_renderer_boundaries.py", "--self-test"],
            [PYTHON, "devops/tests/test_renderer_boundaries.py"],
        ],
        working_directory=REPOSITORY_ROOT,
        quick=True,
    ),
    Step(
        "engine-layout",
        "Engine layout (the core never links the facade or reads a registry)",
        # The self test first proves the checker still catches each break.
        [
            [PYTHON, "devops/tests/test_engine_layout.py", "--self-test"],
            [PYTHON, "devops/tests/test_engine_layout.py"],
        ],
        working_directory=REPOSITORY_ROOT,
        quick=True,
    ),
    Step(
        "keycode-mirror",
        "C# KeyCode mirror (names, order and numbers against the engine table)",
        # The self test first proves the checker still catches each break.
        [
            [PYTHON, "devops/tests/test_keycode_mirror.py", "--self-test"],
            [PYTHON, "devops/tests/test_keycode_mirror.py"],
        ],
        working_directory=REPOSITORY_ROOT,
        quick=True,
    ),
    Step(
        "asset-metadata",
        "Asset metadata (orphans, duplicate guids)",
        # The self test first proves the checker still catches each mistake.
        [
            [PYTHON, "devops/tests/test_asset_metadata.py", "--self-test"],
            [PYTHON, "devops/tests/test_asset_metadata.py"],
        ],
        working_directory=REPOSITORY_ROOT,
        quick=True,
    ),
    Step(
        "log-contract",
        "Host log contract (the tokens the suites wait for)",
        [[PYTHON, "devops/tests/test_log_contract.py"]],
        working_directory=REPOSITORY_ROOT,
        quick=True,
    ),
    Step(
        "engine-exports",
        "Engine dylib export counts (Windows limit 65,535)",
        # Builds the two engine dylibs, then reads their export directories.
        [
            ["cargo", "build", "-p", "pill_core", "-p", "pill_engine_core"],
            [PYTHON, "../devops/tests/test_engine_exports.py", "--self-test"],
            [PYTHON, "../devops/tests/test_engine_exports.py"],
        ],
    ),
    Step(
        "engine-feature-drift",
        "Engine dylib feature drift (host vs module and project builds)",
        [[PYTHON, "devops/tests/test_engine_feature_drift.py"]],
        working_directory=REPOSITORY_ROOT,
        paths=["Cargo.toml", "*/Cargo.toml", "Cargo.lock", "*/Cargo.lock", "modules/.cargo/*"],
    ),
    Step(
        "extension-examples",
        "Extension examples (every one builds and runs headless)",
        # Discovery is the same glob the build sweep reads, and an empty
        # discovery fails: the suite exists so an extension's example cannot
        # rot, and a rule that finds nothing is the failure it guards.
        [[PYTHON, "devops/tests/test_extension_examples.py"]],
        working_directory=REPOSITORY_ROOT,
    ),
    Step(
        "wrapper-entry-points",
        "Wrapper entry points (the wrapper exports the full entry-point set)",
        # Builds the fixture extension both ways and compares PE export names.
        [[PYTHON, "devops/tests/test_wrapper_entry_points.py"]],
        working_directory=REPOSITORY_ROOT,
    ),
    Step("doc", "Documentation", [["cargo", "doc", "--workspace", "--no-deps"]]),
    Step(
        "wasm-clippy",
        "Web target clippy",
        # What a browser build compiles must stay wasm-clean. The workspace's
        # `-C prefer-dynamic` cannot target wasm, so RUSTFLAGS is cleared, and
        # a machine-wide compiler wrapper is bypassed for the same build.
        [
            [
                "cargo", "clippy", "--target", "wasm32-unknown-unknown",
                "-p", "pill_player", "--features", "rendering",
                "-p", "pill_master_renderer",
                "--", "-D", "warnings",
            ],
            # The browser frontends, game and editor, so both stay wasm-clean.
            [
                "cargo", "clippy", "--target", "wasm32-unknown-unknown",
                "-p", "pill_web", "-p", "pill_web_editor",
                "--", "-D", "warnings",
            ],
        ],
        environment={"RUSTFLAGS": "", "RUSTC_WRAPPER": ""},
    ),
    # The theme pipeline's game half: the headless host on `pills_rust`, the
    # `.theme` override re-applying live, a refusal
    # keeping the applied theme, a project reload keeping it too, and the pack
    # writer keeping editor content out. A host-tier suite, so it runs before
    # the editor lane - that lane's build must stay the last one.
    Step(
        "theme",
        "Theme pipeline suite (the game UI's theme, end to end)",
        [[PYTHON, "devops/tests/run_all.py", "--only", "test_ui_theme.py"]],
        working_directory=REPOSITORY_ROOT,
    ),
    # The example's committed render chain: the windowed host runs
    # `devops/tests/projects/native`, whose `init` loads `res/pipelines/main.pipeline`
    # by path, and the renderer's `pass chain:` line must name the project's
    # own pass with draws. A host-tier suite, so it runs before the editor
    # lane's build.
    Step(
        "renderer-chains",
        "Render chain suite (a committed .pipeline runs the frame)",
        [[PYTHON, "devops/tests/run_all.py", "--only", "test_render_chains.py"]],
        working_directory=REPOSITORY_ROOT,
    ),
    # The project's own shader: the windowed host cooks a source that has no
    # cooked output, then re-cooks, reimports and rebuilds it after an edit.
    # Nothing else covers a project shader reaching the GPU, and the renderer's
    # rebuild line only exists for this. A host-tier suite, so it runs before
    # the editor lane's build.
    Step(
        "renderer-shaders",
        "Project shader recompile suite (an edited .hlsl reaches the GPU)",
        [[PYTHON, "devops/tests/run_all.py", "--only", "test_shader_recompile.py"]],
        working_directory=REPOSITORY_ROOT,
    ),
    # A resource that declares its layout: kept across an edit to a file it does
    # not declare, dropped through the retiring image and rebuilt when the file
    # its layout lives in changes. The test project's own drop line is what proves
    # the old value was released rather than leaked. A host-tier suite, so it
    # runs before the editor lane's build.
    Step(
        "resource-layout",
        "Resource layout fingerprint suite (kept, dropped and rebuilt across a reload)",
        [[PYTHON, "devops/tests/run_all.py", "--only", "test_resource_layout_fingerprint.py"]],
        working_directory=REPOSITORY_ROOT,
    ),
    # A manager's settings file: created from the manager's own defaults, applied
    # at startup, re-applied live when edited, and refused when the document does
    # not read. A host-tier suite, so it runs before the editor lane's build.
    Step(
        "manager-settings",
        "Manager settings suite (created, applied live, kept across a reload)",
        [[PYTHON, "devops/tests/run_all.py", "--only", "test_manager_settings.py"]],
        working_directory=REPOSITORY_ROOT,
    ),
    # The three startup switches: each one is read once, so the suite starts
    # the host four times - all on, then with reloading, patching and asset
    # reloading switched off in turn - and reads what each posture does and
    # does not notice. A host-tier suite, so it runs before the editor lane's
    # build.
    Step(
        "host-switches",
        "Host switches suite (each switch stops what it names)",
        [[PYTHON, "devops/tests/run_all.py", "--only", "test_host_switches.py"]],
        working_directory=REPOSITORY_ROOT,
    ),
    # The end-to-end editor lane: the real executable, driven through its
    # scripted channel. Order is the whole point. The host-tier revision suite
    # builds its own narrowed `pill_standalone`, which re-splits the shared
    # DLLs, so it runs first; the `pill_editor` build must then be the
    # last build before the launch, or the editor dies on 0xC0000139
    # (`check_relevant.py` encodes the same rule for the change-scoped runs).
    Step(
        "editor",
        "Editor suites (the real executable, scripted)",
        [
            [PYTHON, "../devops/tests/run_all.py", "--only", "test_editor_revision.py"],
            # The egui editor gets its own build right before its suite: the
            # host and the module builds it spawns must share one feature
            # resolution, and a narrower build in between re-splits them.
            ["cargo", "build", "-p", "pill_editor"],
            [PYTHON, "../devops/tests/run_all.py", "--only", "test_editor_egui.py"],
        ],
    ),
]


def build_parser() -> argparse.ArgumentParser:
    """Command-line options; see the module docstring."""
    parser = argparse.ArgumentParser(description="Run the per-commit checks.")
    parser.add_argument("--quick", action="store_true", help="run only the quick steps")
    parser.add_argument("--only", nargs="+", default=[], metavar="NAME", help="run only these steps")
    parser.add_argument("--skip", nargs="+", default=[], metavar="NAME", help="leave out these steps")
    parser.add_argument("--keep-going", action="store_true",
                        help="continue after a failure (a failing quick step still ends the run)")
    parser.add_argument("--changed", action="store_true",
                        help="skip steps whose paths no changed file matches")
    parser.add_argument("--list", action="store_true", help="print the steps and exit")
    # Internal: the body of the `default-lane-features` step.
    parser.add_argument("--assert-default-lane-features", action="store_true", help=argparse.SUPPRESS)
    return parser


def assert_default_lane_features() -> int:
    """0 when the default test lane's feature graph has no development feature, else 1.

    Asks cargo which features of `pill_engine_core` the lane would enable; a
    member that turns `development` on (directly or through a dependency) shows up
    here before it silently turns the shipping lane into a second development
    lane.
    """
    command = ["cargo", "tree", "--offline", "-e", "features", *DEFAULT_LANE, "-i", "pill_engine_core"]
    print(f"$ {format_command(command)}", flush=True)
    try:
        result = subprocess.run(command, cwd=MODULES_ROOT, capture_output=True, text=True)
    except FileNotFoundError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    if result.returncode != 0:
        print(result.stderr[-2000:], file=sys.stderr)
        return 1
    offenders = [line.strip() for line in result.stdout.splitlines() if "development" in line]
    if offenders:
        print("ERROR: the default test lane enables the engine's development feature through:", file=sys.stderr)
        for line in offenders:
            print(f"  {line}", file=sys.stderr)
        print("Exclude the member that turns it on in DEFAULT_LANE (verify.py).", file=sys.stderr)
        return 1
    print("default test lane: no development feature in the graph")
    return 0


def select_steps(arguments: argparse.Namespace) -> Optional[List[Step]]:
    """The steps to run, in declaration order, or None after a usage error."""
    known_names = {step.name for step in STEPS}
    unknown_names = [name for name in [*arguments.only, *arguments.skip] if name not in known_names]
    if unknown_names:
        print(f"ERROR: unknown step(s): {', '.join(unknown_names)}", file=sys.stderr)
        print(f"Known steps: {', '.join(step.name for step in STEPS)}", file=sys.stderr)
        return None

    selected = STEPS
    if arguments.only:
        selected = [step for step in selected if step.name in arguments.only]
    elif arguments.quick:
        selected = [step for step in selected if step.quick]
    return [step for step in selected if step.name not in arguments.skip]


def changed_paths() -> List[str]:
    """Repository paths changed in the working tree (tracked edits and untracked files)."""
    paths: List[str] = []
    for arguments in (["diff", "--name-only", "HEAD"], ["ls-files", "--others", "--exclude-standard"]):
        result = subprocess.run(["git", *arguments], cwd=REPOSITORY_ROOT, capture_output=True, text=True)
        if result.returncode == 0:
            paths.extend(line for line in result.stdout.splitlines() if line)
    return paths


def unaffected_steps(steps: List[Step], paths: List[str]) -> List[Step]:
    """The steps with `paths` that no changed path matches."""
    return [step for step in steps
            if step.paths and not any(fnmatch.fnmatch(path, pattern) for path in paths for pattern in step.paths)]


def write_timings(results: List[tuple]) -> None:
    """The finished steps as `timings.json` in the agent_run folder, when run under it."""
    run_directory = os.environ.get("AGENT_RUN_DIRECTORY")
    if not run_directory:
        return
    steps = [{"name": step.name, "seconds": round(seconds, 1),
              "state": {"PASS": "passed", "FAIL": "failed", "SKIP": "skipped"}[outcome]}
             for step, outcome, seconds in results]
    try:
        (Path(run_directory) / "timings.json").write_text(json.dumps({"steps": steps}, indent=2), encoding="utf-8")
    except OSError as error:
        print(f"warning: timings.json was not written: {error}", file=sys.stderr)


def format_command(command: Sequence[str]) -> str:
    """A command as one readable line, with this interpreter shown as `python`."""
    return " ".join("python" if part == PYTHON else part for part in command)


def run_step(step: Step) -> bool:
    """Run every command of `step` in order; True when all of them exit 0."""
    environment = dict(os.environ, **step.environment)
    if step.note:
        print(f"note: {step.note}", flush=True)
    passed = True
    try:
        for command in step.commands:
            print(f"$ {format_command(command)}", flush=True)
            # A missing tool (no cargo on PATH) is a failed step, not a crash.
            try:
                result = subprocess.run(command, cwd=step.working_directory, env=environment)
            except FileNotFoundError as error:
                print(f"ERROR: {error}", file=sys.stderr)
                passed = False
                break
            if result.returncode != 0:
                passed = False
                break
        return passed
    finally:
        # The cleanup commands undo what the step wrote, so they run even after
        # a failure: a red step must not leave a generated file in the tree.
        for command in step.cleanup:
            print(f"$ {format_command(command)}", flush=True)
            try:
                result = subprocess.run(command, cwd=step.working_directory, env=environment)
                if result.returncode != 0:
                    print(
                        f"warning: cleanup command exited {result.returncode}: "
                        f"{format_command(command)}",
                        file=sys.stderr,
                    )
            except FileNotFoundError as error:
                print(f"warning: cleanup command not run: {error}", file=sys.stderr)


def main() -> int:
    """Run the selected steps and print the summary table."""
    arguments = build_parser().parse_args()
    if arguments.assert_default_lane_features:
        return assert_default_lane_features()
    steps = select_steps(arguments)
    if steps is None:
        return 2

    if arguments.list:
        for step in STEPS:
            marker = " (quick)" if step.quick else ""
            print(f"{step.name}{marker}: {step.title}")
            for command in step.commands:
                print(f"    {format_command(command)}")
            for command in step.cleanup:
                print(f"    (always) {format_command(command)}")
        return 0

    if arguments.changed:
        paths = changed_paths()
        skipped = unaffected_steps(steps, paths)
        for step in skipped:
            print(f"skip {step.name}: no changed path matches {', '.join(step.paths)}", flush=True)
        steps = [step for step in steps if step not in skipped]

    # Each result is "PASS", "FAIL" or "SKIP" (not run after a failure).
    results: List[tuple] = []
    failed = False
    quick_failed = False
    for step in steps:
        if failed and (not arguments.keep_going or quick_failed):
            results.append((step, "SKIP", 0.0))
            continue
        if RUNNING_IN_GITHUB_ACTIONS:
            print(f"::group::{step.title} [{step.name}]", flush=True)
        else:
            print(f"\n=== {step.title} [{step.name}] ===", flush=True)
        started = time.monotonic()
        passed = run_step(step)
        if RUNNING_IN_GITHUB_ACTIONS:
            print("::endgroup::", flush=True)
            if not passed:
                print(f"::error::{step.name} failed", flush=True)
        results.append((step, "PASS" if passed else "FAIL", time.monotonic() - started))
        failed = failed or not passed
        if not passed and step.quick and not quick_failed:
            quick_failed = True
            if arguments.keep_going:
                print()
                print(f"{step.name} is a quick step and failed: the run ends here even with --keep-going "
                      "(fix it, then run again)", flush=True)
        write_timings(results)

    print("\n=== Summary ===")
    for step, outcome, seconds in results:
        duration = f"{seconds:7.1f}s" if outcome != "SKIP" else "        "
        print(f"  {outcome}  {duration}  {step.name}")
    write_job_summary(results)
    return 1 if failed else 0


def write_job_summary(results: List[tuple]) -> None:
    """Append the results as a Markdown table to the GitHub job summary, if any."""
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return
    lines = ["| Step | Result | Seconds |", "| --- | --- | ---: |"]
    for step, outcome, seconds in results:
        duration = f"{seconds:.1f}" if outcome != "SKIP" else ""
        lines.append(f"| {step.name} | {outcome} | {duration} |")
    with open(summary_path, "a", encoding="utf-8") as summary_file:
        summary_file.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    sys.exit(main())
