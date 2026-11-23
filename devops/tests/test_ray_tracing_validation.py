#!/usr/bin/env python3
"""
The ray tracing path runs clean under the Vulkan validation layers.

PROTECTS: With `WGPU_VALIDATION=1` the Demo 1 scene traces its shadows, and the log carries no GPU error, no lost device and no crash.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo) and a display: the host is windowed.
  - The Vulkan SDK's validation layer. Without it the suite prints
    `[SKIP] Vulkan SDK not installed` and passes, because `WGPU_VALIDATION=1`
    would have no layer to load and would check nothing.
  - A Vulkan adapter that offers the ray tracing extensions. When the
    renderer reports the path off, the suite skips with the reason it printed
    rather than pretending the layers were exercised.

DESCRIPTION
    `examples/cube/cube_rt_rust` is Demo 1: a cube over a ground plane, with the
    key light's shadows traced. The suite launches the windowed development
    host on it with `PILL_RAY_TRACING=1` and `WGPU_VALIDATION=1`, and reads
    the lines the renderer writes:

        ray tracing | state="on" ...
        ray traced shadows | shadowed=<n> pixels=<m> surface=<s>

    A run passes when the path reported itself on, the shadow pass counted its
    pixels, and the whole log holds no `GPU error;` line (the renderer's
    handler for errors no scope captured), no lost device and no crash token.
    The validation layer reports a mistake in the ray tracing code as one of
    those, so their absence is the evidence the layer was clean.

    Nothing is edited; the host is terminated on every path.

USAGE
  python devops/tests/test_ray_tracing_validation.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_ray_tracing_validation.py

--- SCRIPT ---
"""

import argparse
import os
import re
import sys
from pathlib import Path

# Standalone-runnable: `devops/` holds `core`, and the suites sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.projects import OFFICIAL_EXAMPLE_PROJECTS, host_project_path  # noqa: E402
from core.suite_common import (  # noqa: E402
    HOST_DEV_RENDERING,
    MODULES_ROOT,
    add_common_suite_arguments,
    ensure_host_lock,
    has_crash_signals,
    host_command,
    launch_process,
    prepare_host,
    run_suite_with_timing,
    terminate_process,
)

# =============================================================================
# Constants
# =============================================================================

PROJECT_ROOT = OFFICIAL_EXAMPLE_PROJECTS["cube_rt_rust"]

# Why the suite cannot check anything on a machine without the layer.
SDK_SKIP_REASON = "Vulkan SDK not installed"

# The layer's manifest, beside the `vkconfig` install and in the directories a
# developer points the loader at by hand.
VALIDATION_LAYER_MANIFEST = "VkLayer_khronos_validation.json"
LAYER_PATH_VARIABLES = ("VK_LAYER_PATH", "VK_ADD_LAYER_PATH")

RAY_TRACING_TOKEN = "ray tracing |"
SHADOW_TOKEN = "ray traced shadows |"

# What the validation layer turns into a log line when the code made a mistake.
GPU_ERROR_TOKEN = "GPU error;"
DEVICE_LOST_TOKEN = "device lost"

STATE_PATTERN = re.compile(r'state="(?P<state>on|off)"')
REASON_PATTERN = re.compile(r'reason="(?P<reason>[^"]*)"')

# The host builds the project and the renderer module on its first run.
STARTUP_TIMEOUT_SECONDS = 300


# =============================================================================
# Free functions
# =============================================================================


def layer_search_directories() -> list:
    """The directories that may hold the validation layer's manifest."""
    directories = []
    for variable in LAYER_PATH_VARIABLES:
        for entry in os.environ.get(variable, "").split(os.pathsep):
            if entry:
                directories.append(Path(entry))
    sdk = os.environ.get("VULKAN_SDK", "")
    if sdk:
        directories.append(Path(sdk) / "Bin")
        directories.append(Path(sdk) / "bin")
    return directories


def validation_layer_available() -> bool:
    """Whether the Vulkan validation layer is installed on this machine."""
    return any(
        (directory / VALIDATION_LAYER_MANIFEST).is_file()
        for directory in layer_search_directories()
    )


def field(pattern: re.Pattern, text: str) -> str:
    """The captured group of `pattern` in the newest line of `text` holding it."""
    for line in reversed(text.splitlines()):
        match = pattern.search(line)
        if match:
            return match.group(1)
    return ""


def ray_tracing_lines(text: str) -> list:
    """Every `ray tracing |` line in `text`."""
    return [line for line in text.splitlines() if RAY_TRACING_TOKEN in line]


def check(text: str) -> list:
    """Returns one `(ok, description)` pair per assertion over the log."""
    checks = [
        (
            field(STATE_PATTERN, text) == "on",
            "the renderer reported the ray tracing path on",
        ),
        (
            SHADOW_TOKEN in text,
            "the shadow pass counted its pixels",
        ),
        (
            GPU_ERROR_TOKEN not in text,
            "no GPU error was logged",
        ),
        (
            DEVICE_LOST_TOKEN not in text,
            "the device was not lost",
        ),
        (
            not has_crash_signals(text),
            "the host did not panic and did not crash",
        ),
    ]
    return checks


def main() -> int:
    """Builds the windowed host, runs Demo 1 under the layers, reads the log."""
    parser = argparse.ArgumentParser(
        description="Run the ray traced shadows under the Vulkan validation layers."
    )
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()
    scale = arguments.timeout_scale

    print("\n  Ray tracing validation suite")
    print("  Project: {}".format(PROJECT_ROOT))
    if not validation_layer_available():
        print("  [SKIP] {}".format(SDK_SKIP_REASON))
        print("         `WGPU_VALIDATION=1` has no layer to load, so there is")
        print("         nothing to check on this machine.")
        return 0

    ensure_host_lock()
    if not prepare_host(HOST_DEV_RENDERING, arguments):
        return 1

    environment = dict(
        os.environ,
        PROJECT_PATH=host_project_path(PROJECT_ROOT),
        PILL_RAY_TRACING="1",
        WGPU_VALIDATION="1",
    )
    process, monitor = launch_process(
        host_command(HOST_DEV_RENDERING), MODULES_ROOT, environment
    )
    failures = 0
    try:
        if not monitor.wait_for(RAY_TRACING_TOKEN, STARTUP_TIMEOUT_SECONDS * scale):
            print("  [FAIL] the renderer reported no ray tracing decision; its output was:")
            print(monitor.output_since(0)[-1500:])
            return 1
        state = field(STATE_PATTERN, monitor.output_since(0))
        if state != "on":
            reason = field(REASON_PATTERN, monitor.output_since(0)) or "no reason given"
            print("  [SKIP] the ray tracing path is off here: {}".format(reason))
            return 0
        monitor.wait_for(SHADOW_TOKEN, 120 * scale)

        for ok, description in check(monitor.output_since(0)):
            print("  [{}] {}".format("OK" if ok else "FAIL", description))
            if not ok:
                failures += 1
        if failures:
            print("\n  Ray tracing lines seen:")
            for line in ray_tracing_lines(monitor.output_since(0)):
                print("    " + line.strip())
            print("\n  Last output:")
            print(monitor.output_since(0)[-1500:])
    finally:
        print("\n  [CLEANUP] Stopping the windowed host...")
        terminate_process(process, monitor)

    if failures:
        print("\n  [FAIL] {} check(s) failed".format(failures))
        return 1
    print("\n  [OK] the ray tracing path runs clean under the validation layers")
    return 0


if __name__ == "__main__":
    sys.exit(run_suite_with_timing(main))
