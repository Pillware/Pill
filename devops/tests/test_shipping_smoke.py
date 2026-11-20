"""
Launches the statically linked shipping binary and checks that it runs.

PROTECTS: The shipping binary has no reload machinery, runs the project and never starts cargo.
TIER: build

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain on PATH. No .NET, and no project sources are edited.

DESCRIPTION
    The hot-reload net cannot cover the shipping build: by construction none of
    its eight suites apply to a binary with reloading compiled out. This suite
    builds that binary, checks it statically, then checks that it actually runs.

    Five things are asserted:

      0. The committed bundle stub is project-agnostic, so no shipping binary
         can be type-checked against a stale generation of another project.
         This runs before the bundle is generated, over the committed files.
      1. The binary contains none of the strings only the reloading path
         produces, which shows that machinery was gated out rather than merely
         unused (the same technique `test_log_contract.py` uses in the other
         direction).
      2. The binary reaches the project loop and reports frames.
      3. It prints the project's own probe line, so the statically linked
         project and its extensions really did initialize - a build that
         linked them but never called their entry points would still start.
      4. It never invokes cargo. The reloading host shells out to cargo before
         the first frame; a shipping build must not, and the surest way to know
         is to watch for a cargo child process rather than to trust a grep. The
         watch polls from launch and fails when the process query itself cannot
         be run, rather than reporting a clean bill of health it did not earn.

    Nothing is edited, so this suite is safe to run against a dirty tree and
    leaves nothing to restore. The one exception is the bundle: the build
    regenerates it and puts the stub back, so the check above sees the tree as
    it was left.

USAGE
  python devops/tests/test_shipping_smoke.py [--timeout-scale N] [--stub-only]

  --timeout-scale N  multiply every timeout, for slower machines
  --stub-only        run check 0 alone, without building anything (the gate
                     uses it, so a committed generation never reaches a build)

EXAMPLE USAGE
  python devops/tests/test_shipping_smoke.py
  python devops/tests/test_shipping_smoke.py --timeout-scale 2
  python devops/tests/test_shipping_smoke.py --stub-only

--- SCRIPT ---
"""

# Standard library
import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.paths import MODULES_ROOT, REPOSITORY_ROOT, find_executable  # noqa: E402
from core.projects import NATIVE_PROJECT_ROOT, repository_relative  # noqa: E402
from core.suite_common import (  # noqa: E402
    SHIPPING_BUNDLE_PATHS,
    restore_paths,
    run_suite_with_timing,
    snapshot_paths,
)

# =============================================================================
# Constants
# =============================================================================

# Cargo arguments that select the shipping posture: no development host,
# project and modules linked in.
SHIPPING_FEATURES = ["--no-default-features", "--features", "shipping"]

# Where the shipping build lands.
SHIPPING_BINARY = (
    MODULES_ROOT
    / "target"
    / "release"
    / ("pill_standalone.exe" if os.name == "nt" else "pill_standalone")
)

# The host prints this once it is running frames.
FRAME_TOKEN = "FPS |"

# The example project's own output, which only appears if `project::init` ran
# and registered its systems.
PROJECT_TOKEN = "[project]"

# Proof that the statically linked modules initialized too.
MODULE_TOKEN = "extension linked"

# The embedded pack indexed by guid: the example ships one standalone
# asset (`res/dwdw.render_pass`), so the index holds at least one record.
PACK_DATABASE_PATTERN = re.compile(r"database indexed from the embedded pack\s+assets=([0-9]+)")

# Anything here means the reload machinery was compiled in after all.
FORBIDDEN_TOKENS = (
    "Hot reloading of",
    "building project module",
    "watching for source changes",
    "module DLL loaded successfully",
)

# Strings only the reloading path contains; any of them in the shipping binary
# means that machinery was compiled in rather than gated out.
RELOAD_ONLY_STRINGS = (
    "watching for source changes",
    "module DLL loaded successfully",
    "building project module",
    "pill_standalone_temp",
    "cargo build",
    "--offline",
    "--timings",
)

# Seconds to wait for the binary to reach the project loop, before scaling.
STARTUP_TIMEOUT = 40

# How long to let it run once started, so several frames and one probe land.
OBSERVE_SECONDS = 6

BUILD_TIMEOUT = 1800

# =============================================================================
# Free Functions
# =============================================================================


def stub_failure() -> str:
    """Checks that the shipping bundle stub in the tree is project-agnostic.

    The stub is committed only so cargo can resolve `pill_standalone`'s
    optional path dependency on a fresh clone. A stub that is a real
    generation is a bundle that compiles, so the shipping posture can be
    type-checked - and shipped - against some other project.

    A tree that holds a generated bundle (an earlier build's output) is asked
    to restore the stub rather than skipped: the file it leaves behind is
    exactly the one that must not be committed, and
    `generate_shipping_bundle.py --restore-stub` puts the stub back.
    """
    bundle = REPOSITORY_ROOT / "build" / "pill_shipping_bundle"
    try:
        source = (bundle / "src" / "lib.rs").read_text(encoding="utf-8")
    except OSError as error:
        return f"cannot read the bundle stub: {error}"
    if "Generated shipping bundle" in source:
        return (
            "the tree holds a generated bundle where the stub belongs; restore it "
            "with `python devops/tools/generate_shipping_bundle.py --restore-stub`"
        )
    for relative, required, forbidden in (
        ("Cargo.toml", ('name = "pill_shipping_bundle"',), ("[dependencies]",)),
        ("src/lib.rs", ("compile_error!", "generate_shipping_bundle.py"), ("static_project",)),
    ):
        try:
            content = (bundle / relative).read_text(encoding="utf-8")
        except OSError as error:
            return f"cannot read the bundle stub's {relative}: {error}"
        for token in required:
            if token not in content:
                return f"the bundle stub's {relative} does not contain {token!r}"
        for token in forbidden:
            if token in content:
                return (
                    f"the bundle stub's {relative} contains {token!r}: it is a real "
                    "generation, so a shipping build could compile against it"
                )
    return ""


def build_shipping_binary(timeout: int) -> str:
    """Builds the shipping binary, returning an error message or "" on success.

    RUSTFLAGS is cleared for the reason `devops/ci_cd/build_shipping.sh`
    documents: the workspace sets `-C prefer-dynamic`, which rustc refuses to
    combine with the release profile's `lto = "fat"`.
    """
    # The shipping binary links the generated bundle, which is gitignored build
    # output: regenerate it from the project's settings file first.
    generator = [
        sys.executable,
        str(REPOSITORY_ROOT / "devops" / "tools" / "generate_shipping_bundle.py"),
        repository_relative(NATIVE_PROJECT_ROOT),
    ]
    completed = subprocess.run(
        generator,
        cwd=str(REPOSITORY_ROOT),
        env=dict(os.environ, RUSTFLAGS=""),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )
    if completed.returncode != 0:
        return (completed.stdout or "")[-1500:]

    command = [
        find_executable("cargo"),
        "build",
        "--release",
        "--package",
        "pill_standalone",
        *SHIPPING_FEATURES,
        "--manifest-path",
        str(MODULES_ROOT / "Cargo.toml"),
    ]
    completed = subprocess.run(
        command,
        cwd=str(REPOSITORY_ROOT),
        env=dict(os.environ, RUSTFLAGS=""),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )
    if completed.returncode != 0:
        return (completed.stdout or "")[-1500:]
    return ""


# How often to look for a cargo/rustc child of the shipping process. A reload
# build lives for seconds, so a single sample can miss it - the check polls from
# launch instead.
CHILD_CHECK_INTERVAL = 0.5


class ProcessToolUnavailable(Exception):
    """The platform's process listing tool could not be run at all.

    A check that silently passes when its tool is missing is worse than no check:
    the suite used to report `[OK] no cargo or rustc child process` on a machine
    whose `wmic` had been removed, proving nothing. The caller turns this into a
    failure.
    """


def parse_windows_process_listing(listing: str) -> int:
    """Counts cargo/rustc process names in `Get-CimInstance` output."""
    return sum(
        1
        for line in listing.splitlines()
        if "cargo" in line.lower() or "rustc" in line.lower()
    )


def cargo_child_processes(parent_id: int) -> int:
    """Counts cargo or rustc processes started underneath `parent_id`.

    A grep of the binary proves the *command* is not compiled in; this proves
    nothing spawned one anyway, which is the behaviour the shipping build is
    actually promising.

    `Get-CimInstance Win32_Process` is the query that survives on current
    Windows: `wmic` is gone, and its absence is reported as
    [`ProcessToolUnavailable`] rather than swallowed as "zero children".
    """
    if os.name != "nt":
        try:
            listed = subprocess.run(
                ["ps", "-o", "ppid=,comm="],
                stdout=subprocess.PIPE,
                text=True,
                timeout=30,
            ).stdout
        except (OSError, subprocess.SubprocessError) as error:
            raise ProcessToolUnavailable(f"`ps` could not be run: {error}") from error
        return sum(
            1
            for line in listed.splitlines()
            if line.split()[:1] == [str(parent_id)]
            and any(name in line for name in ("cargo", "rustc"))
        )

    shell = shutil.which("powershell") or shutil.which("pwsh")
    if not shell:
        raise ProcessToolUnavailable("neither powershell nor pwsh is on PATH")
    query = (
        f"Get-CimInstance Win32_Process -Filter \"ParentProcessId={parent_id}\""
        " | Select-Object -ExpandProperty Name"
    )
    try:
        completed = subprocess.run(
            [shell, "-NoProfile", "-NonInteractive", "-Command", query],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise ProcessToolUnavailable(f"the process query could not be run: {error}") from error
    if completed.returncode != 0:
        raise ProcessToolUnavailable(
            f"the process query failed: {(completed.stderr or '').strip()[:200]}"
        )
    return parse_windows_process_listing(completed.stdout or "")


def run_and_observe(timeout_scale: float) -> int:
    """Launches the shipping binary and checks what it does. Returns an exit code."""
    print("=" * 70)
    print("  Shipping Build Smoke Test")
    print("=" * 70)

    print("\n  [STUB] the bundle stub is project-agnostic")
    failure = stub_failure()
    if failure:
        print(f"  [FAIL] {failure}")
        return 1
    print("  [OK] the stub refuses to compile and names no project")

    print("\n  [BUILD] cargo build --release -p pill_standalone " + " ".join(SHIPPING_FEATURES))
    # The build regenerates the tracked shipping bundle (and may touch the
    # lockfile); put both back once the binary exists, whatever happened.
    snapshot = snapshot_paths(SHIPPING_BUNDLE_PATHS)
    try:
        failure = build_shipping_binary(int(BUILD_TIMEOUT * timeout_scale))
    finally:
        restore_paths(SHIPPING_BUNDLE_PATHS, snapshot)
    if failure:
        print("  [FAIL] the shipping binary did not build:")
        print(failure)
        return 1
    if not SHIPPING_BINARY.is_file():
        print(f"  [FAIL] {SHIPPING_BINARY} was not produced")
        return 1
    print(f"  [OK] built, {SHIPPING_BINARY.stat().st_size:,} bytes")

    # Static check: the reload machinery must be absent from the image itself.
    image = SHIPPING_BINARY.read_bytes()
    present = [text for text in RELOAD_ONLY_STRINGS if text.encode() in image]
    if present:
        print("  [FAIL] the shipping binary contains reload-only strings: " + ", ".join(present))
        return 1
    print(f"  [OK] none of {len(RELOAD_ONLY_STRINGS)} reload-only strings in the binary")

    print("\n  [RUN] launching it")
    process = subprocess.Popen(
        [str(SHIPPING_BINARY)],
        cwd=str(MODULES_ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )

    collected = []
    spawned_cargo = 0
    child_checks = 0
    process_tool_error = ""
    next_child_check = 0.0
    deadline = time.monotonic() + STARTUP_TIMEOUT * timeout_scale
    observe_until = None
    try:
        # A blocking readline would hang past the deadline if the host went
        # quiet, so the loop is bounded by wall time and the process is killed
        # to unblock it.
        while time.monotonic() < deadline:
            if process.poll() is not None:
                break
            # Polled from launch, not sampled once when the first frame appears:
            # a build the shipping path started would be gone long before then.
            if time.monotonic() >= next_child_check:
                next_child_check = time.monotonic() + CHILD_CHECK_INTERVAL
                try:
                    spawned_cargo = max(spawned_cargo, cargo_child_processes(process.pid))
                    child_checks += 1
                except ProcessToolUnavailable as error:
                    process_tool_error = str(error)
                    break
            line = process.stdout.readline()
            if not line:
                break
            collected.append(line)
            if observe_until is None and FRAME_TOKEN in line:
                # Frames are running; watch a little longer so a probe lands.
                observe_until = time.monotonic() + OBSERVE_SECONDS * timeout_scale
            if observe_until is not None and time.monotonic() > observe_until:
                break
    finally:
        process.kill()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            pass

    output = "".join(collected)
    failures = []

    if FRAME_TOKEN not in output:
        failures.append("the binary never reached the project loop")
    if MODULE_TOKEN not in output:
        failures.append("no extension was linked and initialized")
    if PROJECT_TOKEN not in output:
        failures.append("the project's own systems never produced output")
    pack_database = PACK_DATABASE_PATTERN.search(output)
    if pack_database is None:
        failures.append("the embedded asset pack was not indexed by guid")
    elif int(pack_database.group(1)) < 1:
        failures.append("the embedded pack's guid index holds no asset")
    for token in FORBIDDEN_TOKENS:
        if token in output:
            failures.append(f"reload machinery ran: {token!r}")
    if spawned_cargo:
        failures.append(f"it spawned {spawned_cargo} cargo/rustc child process(es)")
    if process_tool_error:
        failures.append(
            f"the child-process check could not run ({process_tool_error}), so "
            f"nothing here shows that no cargo was spawned"
        )

    if failures:
        print("\n  [FAIL] " + "\n  [FAIL] ".join(failures))
        print("\n  Output tail:\n" + output[-1600:])
        return 1

    frames = output.count(FRAME_TOKEN)
    modules = output.count(MODULE_TOKEN)
    print(f"  [OK] reached the project loop, {frames} frame report(s)")
    print(f"  [OK] {modules} extension(s) linked and initialized")
    print("  [OK] the project's systems produced output")
    print(f"  [OK] no cargo or rustc child in {child_checks} process queries, "
          f"and no reload machinery ran")
    print("-" * 70)
    print("  TEST PASSED")
    print("=" * 70)
    return 0


def main() -> None:
    """Parse arguments and run the smoke test."""
    parser = argparse.ArgumentParser(
        prog="test_shipping_smoke.py",
        description="Launch the statically linked shipping binary and check it runs.",
    )
    parser.add_argument(
        "--timeout-scale",
        type=float,
        default=1.0,
        help="multiply every timeout, for slower machines (default 1.0)",
    )
    parser.add_argument(
        "--stub-only",
        action="store_true",
        help="only check that the committed bundle stub is project-agnostic",
    )
    arguments = parser.parse_args()

    # The stub check alone needs no cargo run, so the gate can reject a
    # committed generation before anything builds against it.
    if arguments.stub_only:
        failure = stub_failure()
        if failure:
            print(f"  [FAIL] {failure}")
            sys.exit(1)
        print("  [OK] the stub refuses to compile and names no project")
        sys.exit(0)

    sys.exit(run_and_observe(arguments.timeout_scale))


if __name__ == "__main__":
    run_suite_with_timing(main)
