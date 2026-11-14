"""
End-to-end theme suite for Rust-Hybrid-ECS.

PROTECTS: The game half of the theme pipeline: the `.theme` override, live re-apply, refusals, fonts and packing.
TIER: host
SETS: hot-reload

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)

DESCRIPTION
    Drives the headless dev host on `devops/tests/projects/scene` - whose code
    default is the brutalist recipe, whose `res/ui/accent.theme` overrides the
    accent and the heading size, and whose body font is `res/fonts/Silkscreen`
    - and checks the game half of the theme pipeline end to end:
      1. startup - the UI logs `[ui] fonts installed` with a signature, then
         `[ui] theme applied layers=code,file path=ui/accent.theme` with the
         override's accent. The code default and the file are both visible in
         the line, so a layer that silently stopped applying fails here.
      2. live edit - the accent in `res/ui/accent.theme` changes. The host's
         asset watcher reimports the file and the UI re-applies with a higher
         generation and the new accent, without a restart.
      3. refusal - the accent is set to a value that is not a colour. The UI
         logs `[ui] theme refused` with the item's dotted path and keeps the
         theme it had: the generation does not move.
      4. project reload - a comment appended to the project's own source makes
         the host rebuild and reload it. The applied theme survives (no second
         `[ui] theme applied`), and an edit after the reload still re-applies,
         which is what proves the re-apply system the reloaded generation
         registered is the live one.
      5. packing - the shipping bundle's build script packs the project's `res`
         with `pill_assets::write_asset_pack`, so the writer's own test for the
         `editor/` exclusion is run here: a pack built from a project never
         carries the editor's theme or fonts.

    Every file the suite touches is restored byte for byte afterwards.

USAGE
  python devops/tests/test_ui_theme.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_ui_theme.py
  python devops/tests/test_ui_theme.py --timeout-scale 2

--- SCRIPT ---
"""

import argparse
import yaml
import os
import re
import subprocess
import sys
import time
from pathlib import Path

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.suite_common import *  # noqa: E402,F401,F403
from core import log_layout  # noqa: E402

# =============================================================================
# Configuration
# =============================================================================

PROJECT_ROOT = THEMED_PROJECT_ROOT
THEME_FILE = THEMED_PROJECT_THEME
PROJECT_SOURCE = THEMED_PROJECT_LIB_RS

# Headless dev host: this suite reads log lines, and the UI runs in every
# posture (the game UI's own pass does not need a window).
HOST_POSTURE = HOST_DEV_HOT_PATCH

# Host log lines this suite asserts on (`pill_project_ui`, `pill_ui_kit`).
FONTS_INSTALLED_TOKEN = "[ui] fonts installed"
THEME_APPLIED_TOKEN = "[ui] theme applied"
THEME_REFUSED_TOKEN = "[ui] theme refused"

APPLIED_PATTERN = re.compile(
    log_layout.message_then_fields_pattern(
        re.escape(THEME_APPLIED_TOKEN),
        r'\s+layers=(?P<layers>\S+)\s+path=(?P<path>\S+)\s+generation=(?P<generation>\d+)'
        r"\s+accent=(?P<accent>#[0-9a-f]{6})\s+corner=(?P<corner>\d+)",
    )
)
# The accent the project's `res/ui/accent.theme` names.
OVERRIDE_ACCENT = "#33d17a"
# The accent scenario 2 writes, and the one scenario 4 leaves behind.
EDITED_ACCENT = "#0aa1ff"

APPLY_TIMEOUT = 60
RELOAD_TIMEOUT = 900
BUILD_TIMEOUT_SECONDS = 1800


def applied_lines(output: str):
    """Every parsed `[ui] theme applied` line in `output`, in order."""
    matches = []
    for line in output.splitlines():
        match = APPLIED_PATTERN.search(line)
        if match:
            matches.append(match)
    return matches


def wait_for_apply(monitor: OutputMonitor, start_index: int, predicate, label: str):
    """Waits for a `[ui] theme applied` line `predicate` accepts; returns it."""
    deadline = time.monotonic() + APPLY_TIMEOUT
    while time.monotonic() < deadline:
        for match in applied_lines(monitor.output_since(start_index)):
            if predicate(match):
                print(f"  [OK] {label}: {match.group(0)}")
                return match
        if not monitor.process_alive():
            print(f"  [FAIL] {label}: the host exited first.")
            return None
        # Poll until the re-apply lands; the watcher debounces an edit.
        time.sleep(0.2)
    print(f"  [FAIL] {label}: no matching apply within {APPLY_TIMEOUT}s.")
    return None


def write_accent(accent: str) -> None:
    """Rewrites the override's palette accent, keeping the document's header."""
    document = yaml.safe_load(THEME_FILE.read_text(encoding="utf-8"))
    document["asset"]["palette"]["accent"] = accent
    THEME_FILE.write_text(
        yaml.safe_dump(document, sort_keys=False, default_flow_style=False),
        encoding="utf-8",
        newline="",
    )


def project_reload_token() -> str:
    """The analytics line the host prints when this project finishes reloading."""
    manifest = (PROJECT_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    package_name = re.search(r'^name\s*=\s*"([^"]+)"', manifest, re.MULTILINE).group(1)
    return f"| module={package_name} | "


# =============================================================================
# Scenarios
# =============================================================================


def startup(monitor: OutputMonitor) -> int:
    """Scenario 1: the code default and the file both reach the UI.

    Returns the applied generation, or 0 when anything failed.
    """
    if not monitor.wait_for(FONTS_INSTALLED_TOKEN, APPLY_TIMEOUT):
        print(f"  [FAIL] startup: no '{FONTS_INSTALLED_TOKEN}' within {APPLY_TIMEOUT}s.")
        return 0
    fonts_line = [
        line for line in monitor.output_since(0).splitlines()
        if FONTS_INSTALLED_TOKEN in line
    ][-1]
    if not re.search(r"signature=[0-9a-f]{16}", fonts_line):
        print(f"  [FAIL] startup: the fonts line carries no signature: {fonts_line.strip()}")
        return 0
    print("  [OK] startup: the fonts installed with a signature")

    match = wait_for_apply(
        monitor,
        0,
        lambda matched: matched.group("layers") == "code,file"
        and matched.group("accent") == OVERRIDE_ACCENT,
        "the code default and the file applied",
    )
    if match is None:
        return 0
    if match.group("path") != "ui/accent.theme":
        print(f"  [FAIL] startup: the applied layer is not the project's file: {match.group('path')}")
        return 0
    return int(match.group("generation"))


def live_edit(monitor: OutputMonitor, state: dict) -> bool:
    """Scenario 2: an edited override re-applies with a higher generation."""
    start_index = monitor.line_count
    write_accent(EDITED_ACCENT)
    print(f"  [EDIT] {THEME_FILE.name}: accent -> {EDITED_ACCENT}")
    match = wait_for_apply(
        monitor,
        start_index,
        lambda matched: matched.group("accent") == EDITED_ACCENT,
        "the edited override re-applied",
    )
    if match is None:
        return False
    generation = int(match.group("generation"))
    if generation <= state["generation"]:
        print(
            "  [FAIL] live edit: the generation did not move "
            f"({state['generation']} -> {generation})."
        )
        return False
    state["generation"] = generation
    return True


def refusal(monitor: OutputMonitor, state: dict) -> bool:
    """Scenario 3: a value that is not a colour is refused and changes nothing."""
    start_index = monitor.line_count
    write_accent("not-a-colour")
    print(f"  [EDIT] {THEME_FILE.name}: accent -> not-a-colour")
    if not monitor.wait_for(THEME_REFUSED_TOKEN, APPLY_TIMEOUT, start_index):
        print(f"  [FAIL] refusal: no '{THEME_REFUSED_TOKEN}' within {APPLY_TIMEOUT}s.")
        return False
    refused = [
        line for line in monitor.output_since(start_index).splitlines()
        if THEME_REFUSED_TOKEN in line
    ][-1]
    if "palette.accent" not in refused:
        print(f"  [FAIL] refusal: the line does not name the item: {refused.strip()}")
        return False
    # The refused merge must not re-apply anything.
    applies = applied_lines(monitor.output_since(start_index))
    if applies:
        print(f"  [FAIL] refusal: the UI re-applied anyway: {applies[-1].group(0)}")
        return False
    print("  [OK] refusal: named the item and kept the theme")

    # Restore the project's own accent. It differs from the one scenario 2
    # applied, so the restore itself is one more apply - the same file with
    # the same values would rightly change nothing.
    start_index = monitor.line_count
    write_accent(OVERRIDE_ACCENT)
    print(f"  [EDIT] {THEME_FILE.name}: accent -> {OVERRIDE_ACCENT} (restored)")
    match = wait_for_apply(
        monitor,
        start_index,
        lambda matched: matched.group("accent") == OVERRIDE_ACCENT,
        "the restored override re-applied",
    )
    if match is None:
        return False
    state["generation"] = int(match.group("generation"))
    return True


def project_reload(monitor: OutputMonitor, state: dict) -> bool:
    """Scenario 4: the theme survives a project reload and still follows edits."""
    source = PROJECT_SOURCE.read_bytes()
    start_index = monitor.line_count
    PROJECT_SOURCE.write_bytes(
        source + b"\n// THEME-RELOAD PROBE: forces a rebuild of the project\n"
    )
    print(f"  [EDIT] {PROJECT_SOURCE.name}: comment appended; the project must reload")
    if not monitor.wait_for(project_reload_token(), RELOAD_TIMEOUT, start_index):
        print(f"  [FAIL] reload: the project did not reload within {RELOAD_TIMEOUT}s.")
        return False
    print("  [OK] reload: the project rebuilt and reloaded")
    applies = applied_lines(monitor.output_since(start_index))
    if applies:
        print(
            "  [FAIL] reload: the UI re-applied the theme it already had: "
            f"{applies[-1].group(0)}"
        )
        return False

    # The re-apply system the reloaded generation registered is the live one:
    # an edit after the reload still reaches the UI.
    start_index = monitor.line_count
    write_accent(EDITED_ACCENT)
    print(f"  [EDIT] {THEME_FILE.name}: accent -> {EDITED_ACCENT}, after the reload")
    match = wait_for_apply(
        monitor,
        start_index,
        lambda matched: matched.group("accent") == EDITED_ACCENT,
        "the edit after the reload re-applied",
    )
    if match is None:
        return False
    generation = int(match.group("generation"))
    if generation <= state["generation"]:
        print(
            "  [FAIL] reload: the generation did not move after the reload "
            f"({state['generation']} -> {generation})."
        )
        return False
    state["generation"] = generation
    return True


def packing() -> bool:
    """Scenario 5: a pack built by the shipping bundle's writer skips `editor/`."""
    completed = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "pill_assets",
            "--offline",
            "an_asset_pack_skips_the_editor_directory",
        ],
        cwd=MODULES_ROOT,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        print("  [FAIL] packing: the writer's editor/ test failed.")
        print(completed.stdout[-2000:])
        print(completed.stderr[-2000:])
        return False
    print("  [OK] packing: an asset pack skips res/editor/ and keeps the rest")
    return True


def main() -> None:
    """Launch the host, run the scenarios in order, restore every file."""
    parser = argparse.ArgumentParser(description="End-to-end theme suite")
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()

    global APPLY_TIMEOUT, RELOAD_TIMEOUT
    APPLY_TIMEOUT = int(APPLY_TIMEOUT * arguments.timeout_scale)
    RELOAD_TIMEOUT = int(RELOAD_TIMEOUT * arguments.timeout_scale)
    startup_timeout = int(STARTUP_TIMEOUT * arguments.timeout_scale)

    print("=" * 70)
    print("  Theme Pipeline Suite")
    print(f"  Project: {PROJECT_ROOT}")
    print("=" * 70)

    ensure_host_lock()
    kill_stale_hosts()
    if not prepare_host(HOST_POSTURE, arguments, BUILD_TIMEOUT_SECONDS):
        sys.exit(1)

    for path in (THEME_FILE, PROJECT_SOURCE):
        BACKUP.capture(path)

    environment = os.environ.copy()
    environment["PROJECT_PATH"] = THEMED_PROJECT_PATH
    process, monitor = launch_process(host_command(HOST_POSTURE), MODULES_ROOT, environment)
    results = {}
    state = {"generation": 0}
    try:
        if not monitor.wait_for(STARTUP_TOKEN, startup_timeout):
            print(f"  [FAIL] Host did not start within {startup_timeout}s.")
            sys.exit(1)
        print("  [OK] Host running.\n")
        state["generation"] = startup(monitor)
        results["startup"] = state["generation"] > 0
        if results["startup"]:
            results["live edit"] = live_edit(monitor, state)
            results["refusal"] = refusal(monitor, state)
            results["project reload"] = project_reload(monitor, state)
        results["packing"] = packing()
        if has_crash_signals(monitor.output_since(0)):
            print("  [FAIL] The host log shows a crash.")
            results["no crash"] = False
    finally:
        print("\n  [CLEANUP] Restoring files and stopping the host...")
        # The project's last build was of the edited source, so its mtime must
        # stay fresh for the next host start to rebuild the original.
        BACKUP.restore_all(reset_mtime=False)
        terminate_process(process, monitor)

    passed = bool(results) and all(results.values())
    print("\n" + "=" * 70)
    for name, result in results.items():
        print(f"  {'PASS' if result else 'FAIL'}  {name}")
    print("  TEST PASSED" if passed else "  TEST FAILED")
    print("=" * 70)
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    run_suite_with_timing(main)
