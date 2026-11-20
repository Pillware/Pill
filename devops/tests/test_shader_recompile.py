#!/usr/bin/env python3
"""
An edited project shader reaches the GPU while the host runs.

PROTECTS: A project's `.hlsl` edit is re-cooked, reimported and rebuilt on the GPU, and a project with no cooked output still loads its shader.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo) and a display: the rebuild only happens in a
    windowed host, which is the host that has a renderer to compile for.

DESCRIPTION
    A project's shader asset is a fragment source plus its `.hlsl.meta`, and
    the asset's import reads the `.wgsl` cooked beside the source. Nothing in
    a project cooks that file: the renderer data crate's build script cooks its
    own tree only. The development host therefore owes the project two things,
    and this suite checks both in one windowed run against
    `devops/tests/projects/native`, whose `init` imports
    `shaders/reload_probe_fragment.hlsl`:

      1. cook at startup - the suite deletes the committed `.wgsl` before the
         host starts. The host must log `[assets] cooked the project's shader
         sources` with one rebuilt output, the project must initialize (its
         own `[project]` line proves its systems run), and its shader must
         compile.
      2. recompile on an edit - the suite writes the edited fragment (`0.75`
         becomes `0.25`). The host must log `[assets] project shader
         reimported` naming the source, then `shader recompiled` naming the
         same asset. The second line is only printed when a shader object is
         replaced, so it cannot come from the boot compile, and the cooked
         `.wgsl` on disk must hold the edited value: that is what separates a
         real re-cook from a reimport of the previous text.

    The edit is written only once the host announces the project's own shader
    watch. That watch starts with the renderer, which attaches after the
    project initializes, so an edit written the moment the project is alive
    would land before anything watches the tree and would never be seen.

    Both files are restored byte for byte afterwards - the source, and the
    cooked output the host rewrites from it. The host is terminated on every
    path.

USAGE
  python devops/tests/test_shader_recompile.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_shader_recompile.py
  python devops/tests/test_shader_recompile.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import re
import sys
import time
from pathlib import Path

# Standalone-runnable: `devops/` holds `core`, and the suites sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core import log_layout  # noqa: E402
from core.projects import NATIVE_PROJECT_PATH, NATIVE_PROJECT_ROOT  # noqa: E402
from core.suite_common import (  # noqa: E402
    HOST_DEV_RENDERING,
    MODULES_ROOT,
    add_common_suite_arguments,
    ensure_host_lock,
    host_command,
    launch_process,
    prepare_host,
    run_suite_with_timing,
    terminate_process,
)

# =============================================================================
# Constants
# =============================================================================

PROJECT_ROOT = NATIVE_PROJECT_ROOT
SHADER_DIRECTORY = PROJECT_ROOT / "res" / "shaders"

# The asset's own name, which is its path under the project's `res`: both the
# import and the reimport are keyed by it.
SHADER_ASSET = "shaders/reload_probe_fragment.hlsl"
SOURCE_FILE = PROJECT_ROOT / "res" / SHADER_ASSET
COOKED_FILE = SOURCE_FILE.with_suffix(".wgsl")

# What the suite changes: the constant the fragment multiplies by, and what the
# cooked WGSL shows instead once the host has re-cooked it.
ORIGINAL_CONSTANT = "0.75;"
EDITED_CONSTANT = "0.25;"
COOKED_ORIGINAL = "0.75f"
COOKED_EDITED = "0.25f"

# Host log lines this suite asserts on.
COOKED_TOKEN = "[assets] cooked the project's shader sources"
REIMPORTED_TOKEN = "[assets] project shader reimported"
RECOMPILED_TOKEN = "shader recompiled"
# Printed by the test project's own system on every frame, so it only appears once
# the project initialized and the host runs its systems.
PROJECT_ALIVE_TOKEN = "[project]"

# The project's shader watcher is announced by the project's crate name, and
# only once it is registered with the file system. It starts with the
# renderer, which attaches after the project's own module has initialized, so
# an edit written as soon as the project is alive is written before anything
# watches the tree. The shared log-line grammar owns the join between the
# message and its fields.
PROJECT_MODULE_NAME = re.search(
    r'^name\s*=\s*"([^"]+)"',
    (PROJECT_ROOT / "Cargo.toml").read_text(encoding="utf-8"),
    re.MULTILINE,
).group(1)
PROJECT_SHADER_WATCH_MESSAGE_TOKEN = "watching shader sources for edits"
PROJECT_SHADER_WATCH_TOKEN = log_layout.message_then_fields(
    PROJECT_SHADER_WATCH_MESSAGE_TOKEN, f'tree="{PROJECT_MODULE_NAME}"'
)

# The host builds the project DLL on its first run; a cold one is slow.
STARTUP_TIMEOUT_SECONDS = 240
# A cook and a reimport are milliseconds once the host is up; this is slack.
RELOAD_TIMEOUT_SECONDS = 90


# =============================================================================
# Free functions
# =============================================================================


def lines_with(text: str, token: str) -> list:
    """Every line in `text` that carries `token`, oldest first."""
    return [line for line in text.splitlines() if token in line]


def first_line_with(text: str, token: str) -> str:
    """The first line in `text` that carries `token`, or an empty string."""
    matches = lines_with(text, token)
    return matches[0] if matches else ""


def check_startup_cook(text: str) -> list:
    """One `(ok, description)` pair per assertion about the startup cook."""
    cook_lines = lines_with(text, COOKED_TOKEN)
    rebuilt_one = [line for line in cook_lines if "rebuilt=1" in line]
    return [
        (
            bool(cook_lines),
            "the host cooked the project's shader sources at startup",
        ),
        (
            bool(rebuilt_one),
            "the cook rebuilt the missing cooked output (rebuilt=1)",
        ),
        (
            bool(lines_with(text, PROJECT_ALIVE_TOKEN)),
            "the project initialized and its systems run",
        ),
    ]


def rebuilds_after_reimport(text: str) -> list:
    """The shader's rebuild lines that follow the project's shader reimport.

    The rebuild this suite cares about is the one the cook's reimport drives. A
    source edit also reimports through the asset watcher first, carrying the
    text the cook has not replaced yet, so a rebuild that precedes the
    reimport proves only that the stale import reached the GPU.
    """
    reimported = first_line_with(text, REIMPORTED_TOKEN)
    if not reimported:
        return []
    after_reimport = text.split(reimported, 1)[1]
    return [
        line for line in lines_with(after_reimport, RECOMPILED_TOKEN) if SHADER_ASSET in line
    ]


def wait_for_rebuild_after_reimport(monitor, timeout_seconds: float) -> bool:
    """Waits until a rebuild line follows the project's shader reimport.

    A plain wait on the rebuild token would return at once on the rebuild the
    earlier asset-watcher import already logged, so this waits on the pairing
    instead of on the token.
    """
    deadline = time.monotonic() + timeout_seconds
    while True:
        if rebuilds_after_reimport(monitor.output_since(0)):
            return True
        if time.monotonic() >= deadline or not monitor.process_alive():
            return False
        # Poll the log: the rebuild is printed by the host's build, which lands
        # whenever cargo finishes rather than on a cadence this side controls.
        time.sleep(0.1)


def check_live_edit(text: str, cooked_text: str) -> list:
    """One `(ok, description)` pair per assertion about the edited shader."""
    reimported = first_line_with(text, REIMPORTED_TOKEN)
    return [
        (
            SHADER_ASSET in reimported,
            "the host reimported the edited shader asset",
        ),
        (
            bool(rebuilds_after_reimport(text)),
            "the renderer rebuilt the shader after the reimport",
        ),
        (
            COOKED_EDITED in cooked_text,
            "the cooked WGSL holds the edited constant ({})".format(COOKED_EDITED),
        ),
    ]


def report(checks: list) -> int:
    """Prints every check and returns the number that failed."""
    failures = 0
    for ok, description in checks:
        print("  [{}] {}".format("OK" if ok else "FAIL", description))
        if not ok:
            failures += 1
    return failures


def main() -> int:
    """Runs the windowed host, edits the shader, checks the log and restores."""
    parser = argparse.ArgumentParser(
        description="Check that an edited project shader reaches the GPU."
    )
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()
    scale = arguments.timeout_scale

    print("\n  Project shader recompile suite")
    print("  Project: {}".format(PROJECT_ROOT))

    # Step 1: Take the bytes of both files, then remove the cooked output so
    # the host has to cook it at startup. The anchor is read before the write
    # it guards, and both files are restored in the `finally` below.
    source_bytes = SOURCE_FILE.read_bytes()
    cooked_bytes = COOKED_FILE.read_bytes()
    edited_source = source_bytes.replace(
        ORIGINAL_CONSTANT.encode("utf-8"), EDITED_CONSTANT.encode("utf-8")
    )
    if edited_source == source_bytes:
        print("  [FAIL] the fragment does not hold {}".format(ORIGINAL_CONSTANT))
        return 1

    ensure_host_lock()
    if not prepare_host(HOST_DEV_RENDERING, arguments):
        return 1

    environment = dict(os.environ, PROJECT_PATH=NATIVE_PROJECT_PATH)
    process = None
    monitor = None
    failures = 0
    try:
        COOKED_FILE.unlink()
        process, monitor = launch_process(
            host_command(HOST_DEV_RENDERING), MODULES_ROOT, environment
        )
        if not monitor.wait_for(COOKED_TOKEN, STARTUP_TIMEOUT_SECONDS * scale):
            print("  [FAIL] the host logged no startup cook; the last output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        # The cook runs before the project module builds and initializes, so
        # the project's own lines only start once its build is done.
        if not monitor.wait_for(PROJECT_ALIVE_TOKEN, STARTUP_TIMEOUT_SECONDS * scale):
            print("  [FAIL] the project never started; the last output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        failures += report(check_startup_cook(monitor.output_since(0)))

        # Step 2: Wait for the project's own shader watch before editing. The
        # renderer attaches after the project initializes, so its watch is the
        # last part of startup this step depends on.
        if not monitor.wait_for(PROJECT_SHADER_WATCH_TOKEN, STARTUP_TIMEOUT_SECONDS * scale):
            print(
                "  [FAIL] the host never watched the project's shader sources; "
                "the last output was:"
            )
            print(monitor.output_since(0)[-1500:])
            return 1

        # Step 3: Edit the fragment while the host runs, and wait for the two
        # lines that carry the edit through the pipeline.
        SOURCE_FILE.write_bytes(edited_source)
        if not monitor.wait_for(REIMPORTED_TOKEN, RELOAD_TIMEOUT_SECONDS * scale):
            print("  [FAIL] the host reimported no project shader; last output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        # The rebuild the reimport drives is logged on the next frame whose
        # asset revision moved, so wait for one that follows that line.
        wait_for_rebuild_after_reimport(monitor, RELOAD_TIMEOUT_SECONDS * scale)
        failures += report(
            check_live_edit(monitor.output_since(0), COOKED_FILE.read_text(encoding="utf-8"))
        )
    finally:
        SOURCE_FILE.write_bytes(source_bytes)
        COOKED_FILE.write_bytes(cooked_bytes)
        if process is not None and monitor is not None:
            print("\n  [CLEANUP] Stopping the windowed host...")
            terminate_process(process, monitor)

    if failures:
        print("\n  [FAIL] {} check(s) failed".format(failures))
        return 1
    print("\n  [OK] the edited project shader reached the GPU")
    return 0


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
