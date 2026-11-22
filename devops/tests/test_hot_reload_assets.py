"""
Live asset reimport suite for Pill.

PROTECTS: Live asset reimport: .meta edits, broken and restored images, moved files, renderer reloads.
TIER: host

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)

DESCRIPTION
    Drives the headless dev host on `devops/tests/projects/scene`, whose sample
    map is imported through a `.meta` file, and checks that the host's asset
    watcher reimports an asset in place while the game runs:

      1. metadata edit - `reimport_sample.png.meta` changes `texture_type`
         from `Color` to `Normal`. The host must log `[assets] reimported` for
         that asset, with the guid in the file and a content version that
         moved. The line is only printed when the loaded value was replaced in
         its existing slot, which is what keeps every handle to it valid.
      2. broken source - the image is overwritten with bytes that are not an
         image. The host must log that the reimport failed and keep the loaded
         value; restoring the bytes must reimport it again.
      3. reload, then import - `pill_rendering` is rebuilt and
         reloaded (a comment edit), which also reloads the project that links
         it. A metadata edit after that must still reimport, through the
         import registry entries the reloaded images registered: the entries
         of the retired images would point into unmapped code.
      4. move - the image and its `.meta` are moved into a subfolder together
         and back. Each time the host must log `[assets] followed a move`,
         keeping the asset's handle and guid: no new import, and no warning
         that the old source was deleted.
      5. material edit - `materials/teapot.material`, a standalone asset with
         no `.meta`, changes a parameter. The host must reimport the material
         in its slot, re-resolving the shader it names by guid.

    Every file the suite touches is restored byte for byte afterwards.

USAGE
  python devops/tests/test_hot_reload_assets.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python devops/tests/test_hot_reload_assets.py
  python devops/tests/test_hot_reload_assets.py --timeout-scale 2

--- SCRIPT ---
"""

import argparse
import yaml
import os
import re
import sys
import time
from pathlib import Path
from typing import Optional

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.suite_common import *  # noqa: E402,F401,F403

# =============================================================================
# Configuration
# =============================================================================

PROJECT_ROOT = SCENE_PROJECT_ROOT
ASSET_NAME = "textures/reimport_sample.png"
SOURCE_FILE = PROJECT_ROOT / "res" / ASSET_NAME
METADATA_FILE = SOURCE_FILE.with_name(SOURCE_FILE.name + ".meta")
MATERIAL_NAME = "materials/teapot.material"
MATERIAL_FILE = PROJECT_ROOT / "res" / MATERIAL_NAME
DATA_CRATE_ROOT = (
    MODULES_ROOT / "extensions" / "rendering" / "pill_rendering_core" / "modules" / "runtime" / "data" / "src" / "pill_rendering.rs"
)

# Headless dev host: the project draws nothing here, and the asset path under
# test is the same with or without a window.
HOST_POSTURE = HOST_DEV_HOT_PATCH

# Host log lines this suite asserts on (printed by `pill_development_host/src/runtime.rs`).
ASSET_WATCH_TOKEN = "[assets] watching the project's assets for edits"
REIMPORTED_TOKEN = "[assets] reimported"
REIMPORT_FAILED_TOKEN = "[assets] reimport failed; keeping the loaded value"
DATA_RELOADED_TOKEN = "extension hot reload complete"
FOLLOWED_MOVE_TOKEN = "[assets] followed a move"
NEW_IMPORT_TOKEN = "[assets] imported a new asset"
SOURCE_DELETED_TOKEN = "[assets] source deleted"

# The structured logger appends fields with no separator after the message.
VERSIONS_PATTERN = re.compile(r"content_version_before=Some\((\d+)\) content_version=Some\((\d+)\)")
GUID_PATTERN = re.compile(r"guid=([0-9a-f]{32})")

ASSET_TIMEOUT = 60
RELOAD_TIMEOUT = 900
BUILD_TIMEOUT_SECONDS = 1800


def asset_line(output: str, token: str) -> Optional[str]:
    """The last line holding `token` for the asset under test, if any."""
    matches = [line for line in output.splitlines() if token in line and ASSET_NAME in line]
    return matches[-1] if matches else None


def wait_for_asset_line(monitor: OutputMonitor, token: str, start_index: int, timeout: float) -> Optional[str]:
    """Waits until `token` is logged for the asset under test; returns that line."""
    if not monitor.wait_for(token, timeout, start_index):
        return None
    # The token can be logged for another asset first; give this one a moment.
    for _ in range(int(timeout * 10)):
        line = asset_line(monitor.output_since(start_index), token)
        if line is not None:
            return line
        if not monitor.process_alive():
            return None
        # Poll until the expected line shows up.
        time.sleep(0.1)
    return None


def set_texture_type(texture_type: str) -> str:
    """Rewrites the metadata file's `texture_type`; returns the file's guid."""
    document = yaml.safe_load(METADATA_FILE.read_text(encoding="utf-8"))
    document["settings"]["texture_type"] = texture_type
    METADATA_FILE.write_text(
        yaml.safe_dump(document, sort_keys=False, default_flow_style=False),
        encoding="utf-8",
        newline="",
    )
    return document["guid"]


def check_reimported(monitor: OutputMonitor, start_index: int, guid: str, label: str) -> bool:
    """Asserts a reimport of the asset under test with that guid and a moved version."""
    line = wait_for_asset_line(monitor, REIMPORTED_TOKEN, start_index, ASSET_TIMEOUT)
    if line is None:
        print(f"  [FAIL] {label}: no '{REIMPORTED_TOKEN}' for {ASSET_NAME} within {ASSET_TIMEOUT}s.")
        return False
    versions = VERSIONS_PATTERN.search(line)
    logged_guid = GUID_PATTERN.search(line)
    if versions is None or logged_guid is None:
        print(f"  [FAIL] {label}: the reimport line lacks its fields: {line.strip()}")
        return False
    before, after = int(versions.group(1)), int(versions.group(2))
    if after <= before:
        print(f"  [FAIL] {label}: the content version did not move ({before} -> {after}).")
        return False
    if logged_guid.group(1) != guid:
        print(f"  [FAIL] {label}: guid {logged_guid.group(1)} is not the file's {guid}.")
        return False
    print(f"  [OK] {label}: reimported in place, guid {guid}, content version {before} -> {after}.")
    return True


# =============================================================================
# Scenarios
# =============================================================================


def metadata_edit(monitor: OutputMonitor) -> bool:
    """Scenario 1: a `.meta` edit reimports the texture in its slot."""
    start_index = monitor.line_count
    guid = set_texture_type("Normal")
    print(f"  [EDIT] {METADATA_FILE.name}: texture_type Color -> Normal")
    return check_reimported(monitor, start_index, guid, "metadata edit")


def broken_source(monitor: OutputMonitor) -> bool:
    """Scenario 2: an undecodable source keeps the loaded value; a fix reimports."""
    original = SOURCE_FILE.read_bytes()
    start_index = monitor.line_count
    SOURCE_FILE.write_bytes(b"this is not an image")
    print(f"  [EDIT] {SOURCE_FILE.name}: overwritten with bytes that are not an image")
    line = wait_for_asset_line(monitor, REIMPORT_FAILED_TOKEN, start_index, ASSET_TIMEOUT)
    if line is None:
        print(f"  [FAIL] broken source: no '{REIMPORT_FAILED_TOKEN}' for {ASSET_NAME}.")
        return False
    print(f"  [OK] broken source: logged and kept: {line.strip()[-160:]}")

    start_index = monitor.line_count
    SOURCE_FILE.write_bytes(original)
    print(f"  [EDIT] {SOURCE_FILE.name}: original bytes restored")
    guid = yaml.safe_load(METADATA_FILE.read_text(encoding="utf-8"))["guid"]
    return check_reimported(monitor, start_index, guid, "restored source")


def reload_then_import(monitor: OutputMonitor) -> bool:
    """Scenario 3: after the data crate reloads, an import still goes through."""
    start_index = monitor.line_count
    source = DATA_CRATE_ROOT.read_bytes()
    DATA_CRATE_ROOT.write_bytes(source + b"\n// ASSET-RELOAD PROBE: forces a rebuild of the data crate\n")
    print(f"  [EDIT] {DATA_CRATE_ROOT.name}: comment appended; the data crate must reload")
    if not monitor.wait_for(DATA_RELOADED_TOKEN, RELOAD_TIMEOUT, start_index):
        print(f"  [FAIL] reload then import: the data crate did not reload within {RELOAD_TIMEOUT}s.")
        return False
    # The project links the data crate, so its reload follows; wait for it so
    # the edit below lands after every swap. The analytics line names the
    # project by its package name, not as "project".
    if not monitor.wait_for(project_reload_token(), RELOAD_TIMEOUT, start_index):
        print("  [FAIL] reload then import: the project reload after the data reload did not finish.")
        return False
    print("  [OK] data crate and project reloaded")

    start_index = monitor.line_count
    guid = set_texture_type("Color")
    print(f"  [EDIT] {METADATA_FILE.name}: texture_type Normal -> Color, after the reload")
    return check_reimported(monitor, start_index, guid, "reload then import")


def project_reload_token() -> str:
    """The analytics line the host prints when this project finishes reloading."""
    manifest = (PROJECT_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    package_name = re.search(r'^name\s*=\s*"([^"]+)"', manifest, re.MULTILINE).group(1)
    return f"| module={package_name} | "


def move_pair(source: Path, target: Path) -> None:
    """Moves a source and its `.meta` together, as `pill_assets::move_asset` does."""
    target.parent.mkdir(parents=True, exist_ok=True)
    source.rename(target)
    source.with_name(source.name + ".meta").rename(target.with_name(target.name + ".meta"))


def follow_one_move(monitor: OutputMonitor, source: Path, target: Path, label: str) -> bool:
    """Moves the pair and asserts the host followed it without a new import."""
    start_index = monitor.line_count
    move_pair(source, target)
    to_name = target.relative_to(PROJECT_ROOT / "res").as_posix()
    print(f"  [MOVE] {source.name} (+ .meta) -> {to_name}")
    if not monitor.wait_for(FOLLOWED_MOVE_TOKEN, ASSET_TIMEOUT, start_index):
        print(f"  [FAIL] {label}: no '{FOLLOWED_MOVE_TOKEN}' within {ASSET_TIMEOUT}s.")
        return False
    time.sleep(1.0)  # let any stray event for the old path arrive
    output = monitor.output_since(start_index)
    followed = [line for line in output.splitlines() if FOLLOWED_MOVE_TOKEN in line]
    if not any(f'to="{to_name}"' in line for line in followed):
        print(f"  [FAIL] {label}: the followed move does not name {to_name}: {followed}")
        return False
    for token in (NEW_IMPORT_TOKEN, SOURCE_DELETED_TOKEN):
        if token in output:
            print(f"  [FAIL] {label}: the move was not followed cleanly ('{token}' logged).")
            return False
    print(f"  [OK] {label}: followed, same handle and guid, nothing imported again.")
    return True


def move(monitor: OutputMonitor) -> bool:
    """Scenario 4: moving the image with its `.meta` keeps the loaded asset."""
    moved = PROJECT_ROOT / "res" / "textures" / "moved_by_suite" / SOURCE_FILE.name
    try:
        if not follow_one_move(monitor, SOURCE_FILE, moved, "move away"):
            return False
        return follow_one_move(monitor, moved, SOURCE_FILE, "move back")
    finally:
        # Leave the pair where it started even when an assertion failed.
        if moved.exists() and not SOURCE_FILE.exists():
            move_pair(moved, SOURCE_FILE)
        for leftover in (moved.with_name(moved.name + ".meta"), moved):
            if leftover.exists():
                leftover.unlink()
        if moved.parent.exists():
            moved.parent.rmdir()


def material_edit(monitor: OutputMonitor) -> bool:
    """Scenario 5: editing the standalone material file reimports it in place."""
    start_index = monitor.line_count
    document = yaml.safe_load(MATERIAL_FILE.read_text(encoding="utf-8"))
    document["asset"]["parameters"]["pbr_roughness"] = {"Scalar": 0.5}
    MATERIAL_FILE.write_text(
        yaml.safe_dump(document, sort_keys=False, default_flow_style=False),
        encoding="utf-8",
        newline="",
    )
    print(f"  [EDIT] {MATERIAL_FILE.name}: pbr_roughness 1.0 -> 0.5")
    if not monitor.wait_for(REIMPORTED_TOKEN, ASSET_TIMEOUT, start_index):
        print(f"  [FAIL] material edit: nothing was reimported within {ASSET_TIMEOUT}s.")
        return False
    # Let the reimport finish printing both lines before reading them.
    time.sleep(1.0)
    lines = [line for line in monitor.output_since(start_index).splitlines()
             if REIMPORTED_TOKEN in line and MATERIAL_NAME in line]
    guid = document["guid"]
    if not lines or f"guid={guid}" not in lines[-1]:
        print(f"  [FAIL] material edit: no reimport of {MATERIAL_NAME} under guid {guid}: {lines}")
        return False
    versions = VERSIONS_PATTERN.search(lines[-1])
    if versions is None or int(versions.group(2)) <= int(versions.group(1)):
        print(f"  [FAIL] material edit: the content version did not move: {lines[-1].strip()}")
        return False
    print(f"  [OK] material edit: reimported in place, guid {guid}, "
          f"content version {versions.group(1)} -> {versions.group(2)}.")
    return True


def main() -> None:
    """Launch the host, run the scenarios in order, restore every file."""
    parser = argparse.ArgumentParser(description="Live asset reimport suite")
    add_common_suite_arguments(parser)
    arguments = parser.parse_args()

    global ASSET_TIMEOUT, RELOAD_TIMEOUT
    ASSET_TIMEOUT = int(ASSET_TIMEOUT * arguments.timeout_scale)
    RELOAD_TIMEOUT = int(RELOAD_TIMEOUT * arguments.timeout_scale)
    startup_timeout = int(STARTUP_TIMEOUT * arguments.timeout_scale)

    print("=" * 70)
    print("  Live Asset Reimport Suite")
    print(f"  Project: {PROJECT_ROOT}")
    print("=" * 70)

    ensure_host_lock()
    kill_stale_hosts()
    if not prepare_host(HOST_POSTURE, arguments, BUILD_TIMEOUT_SECONDS):
        sys.exit(1)

    for path in (SOURCE_FILE, METADATA_FILE, DATA_CRATE_ROOT, MATERIAL_FILE):
        BACKUP.capture(path)

    environment = os.environ.copy()
    environment["PROJECT_PATH"] = SCENE_PROJECT_PATH
    process, monitor = launch_process(host_command(HOST_POSTURE), MODULES_ROOT, environment)
    results = {}
    try:
        if not monitor.wait_for(STARTUP_TOKEN, startup_timeout):
            print(f"  [FAIL] Host did not start within {startup_timeout}s.")
            sys.exit(1)
        # The watcher starts right after the project loop does, so its line
        # can land just after the startup token; wait briefly instead of
        # reading the output once.
        if not monitor.wait_for(ASSET_WATCH_TOKEN, 10):
            print("  [FAIL] The host did not start its asset watcher.")
            sys.exit(1)
        print("  [OK] Host running, asset watcher started.\n")
        for name, scenario in (
            ("metadata edit", metadata_edit),
            ("broken source", broken_source),
            ("reload then import", reload_then_import),
            ("move", move),
            ("material edit", material_edit),
        ):
            print(f"\n--- {name} ---")
            results[name] = scenario(monitor)
            if not results[name]:
                break
        if has_crash_signals(monitor.output_since(0)):
            print("  [FAIL] The host log shows a crash.")
            results["no crash"] = False
    finally:
        print("\n  [CLEANUP] Restoring files and stopping the host...")
        # The data crate's last build was of the edited source, so its mtime
        # must stay fresh for the next host start to rebuild the original.
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
