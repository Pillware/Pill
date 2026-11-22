# REQUIREMENTS
#   Python 3.8+ for the runner itself. `blender_material_probe.py` needs a
#   Blender 4.x install (found through PILL_BLENDER, a standard install path, or
#   `blender` on PATH). `blender_link_material_smoke.py` needs the editor binary
#   built (`cargo build -p pill_editor` in modules/).
#
# DESCRIPTION
#   Runs the Blender link's end-to-end checks in order and reports each one's
#   exit code: the headless Blender probes (the add-on's node building, its
#   create request, its panels and its Geometry Nodes export, run inside
#   Blender) and the editor smokes (a fake Blender that greets the editor and
#   reads the frames it sends). Every check prints its own lines; this runner
#   only finds the tools, runs them and exits non-zero if any fails.
#
# USAGE
#   python modules/extensions/pill_blender_link/tests/run_blender_link_tests.py
#       Runs both checks.
#   python modules/extensions/pill_blender_link/tests/run_blender_link_tests.py --probe-only
#   python modules/extensions/pill_blender_link/tests/run_blender_link_tests.py --smoke-only
#       Runs just one of them.
#   Set PILL_BLENDER to a blender executable to override the search.
#
# EXAMPLE USAGE
#   cd <repository>; python modules/extensions/pill_blender_link/tests/run_blender_link_tests.py
#
# --- SCRIPT ---
import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

TESTS_DIRECTORY = Path(__file__).resolve().parent
PROBE_SCRIPTS = [
    TESTS_DIRECTORY / "blender_material_probe.py",
    TESTS_DIRECTORY / "blender_link_create_probe.py",
    TESTS_DIRECTORY / "blender_link_panel_probe.py",
    TESTS_DIRECTORY / "blender_link_instances_probe.py",
]
SMOKE_SCRIPTS = [
    TESTS_DIRECTORY / "blender_link_material_smoke.py",
    TESTS_DIRECTORY / "blender_link_create_smoke.py",
]

# The install locations a Blender 4.x download from blender.org lands in.
CANDIDATE_BLENDER_PATHS = [
    r"C:\Program Files\Blender Foundation\Blender 4.5\blender.exe",
    r"C:\Program Files\Blender Foundation\Blender 4.4\blender.exe",
    r"C:\Program Files\Blender Foundation\Blender 4.3\blender.exe",
    r"C:\Program Files\Blender Foundation\Blender 4.2\blender.exe",
    "/Applications/Blender.app/Contents/MacOS/Blender",
]


def find_blender():
    """The blender executable to run the probe with, or None."""
    named = os.environ.get("PILL_BLENDER")
    if named:
        return named if Path(named).exists() else None
    for candidate in CANDIDATE_BLENDER_PATHS:
        if Path(candidate).exists():
            return candidate
    return shutil.which("blender")


def run(command, description):
    """Runs one check and returns True when it exited 0."""
    print(f"[run] {description}")
    print(f"[run]   {' '.join(str(part) for part in command)}")
    completed = subprocess.run(command, cwd=str(TESTS_DIRECTORY.parents[3]))
    if completed.returncode != 0:
        print(f"[run] {description} failed with exit code {completed.returncode}")
        return False
    print(f"[run] {description} passed")
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe-only", action="store_true", help="run only the Blender probes")
    parser.add_argument("--smoke-only", action="store_true", help="run only the editor smokes")
    arguments = parser.parse_args()

    results = {}
    if not arguments.smoke_only:
        blender = find_blender()
        if blender is None:
            print(
                "[run] no Blender found: set PILL_BLENDER to blender.exe, or install "
                "Blender 4.x in a standard location"
            )
            results["blender probe"] = False
        else:
            for probe_script in PROBE_SCRIPTS:
                results[f"blender probe {probe_script.stem}"] = run(
                    [
                        blender,
                        "-b",
                        "--factory-startup",
                        "--python-exit-code",
                        "1",
                        "--python",
                        str(probe_script),
                    ],
                    f"blender probe {probe_script.stem} ({blender})",
                )

    if not arguments.probe_only:
        for smoke_script in SMOKE_SCRIPTS:
            results[f"editor smoke {smoke_script.stem}"] = run(
                [sys.executable, str(smoke_script)],
                f"editor smoke {smoke_script.stem}",
            )

    for description, passed in results.items():
        print(f"[run] {description}: {'passed' if passed else 'FAILED'}")
    return 0 if all(results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
