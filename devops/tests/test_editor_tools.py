"""The editor's tools suite: a real tool, in a real extension, editing a component.

PROTECTS: The editor's tools layer end to end - the spline tool editing a `Spline` (points and tangents as elements, the tool's own coordinate frame, a drag written back and undone as one step, insert from the context menu, delete through the tool, an extension's kind and row attaching by a component) and the collider tool editing a `ColliderComponent` (six face handles, a face drag that changes `dimensions`, and a ball's single radius handle).
TIER: editor
SETS: editor

The tools framework is proven where it matters: by the extension crates that
ship a tool. Two sessions run in turn, each on an extension example project -
`extensions/pill_spline/examples/uber_spline` and
`extensions/pill_physics/examples/collider_edit` - because the editor suite's
shared project carries no extension modules, and because an editor host loads a
folder's `modules/editor` crate only when the project lists that folder.

The suite is its own file rather than an area of `test_editor_egui.py`: the
harness runs one editor at a time (starting a session clears strays, which kills
whatever editor is already up), so an area cannot open a second project beside
the shared session. The steps live in `editor_tools_steps.py`.

Builds against an already-built `pill_editor` binary. Each session keeps its log,
sandbox and command file under `local/editor-validation/egui-<run id>/`, the same
place every editor suite writes.
"""
import argparse
import sys
from pathlib import Path

DEVOPS_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(DEVOPS_ROOT))

from core.editor_log_tokens import EGUI_EDITOR_BINARY  # noqa: E402
from core.suite_common import (  # noqa: E402
    concurrent_workspace_build_refusal,
    verify_editor_binary,
)
from editor_tools_steps import run_tools_steps  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[1])
    parser.add_argument(
        "--binary",
        type=Path,
        help="the built pill_editor binary (default: the debug build)",
    )
    arguments = parser.parse_args()
    binary = arguments.binary or EGUI_EDITOR_BINARY
    # Both refusals run before any editor starts: a tree mid-build would hand
    # the editor a mixed set of shared images, which dies at process start.
    refusal = concurrent_workspace_build_refusal() or verify_editor_binary(binary)
    if refusal:
        print("  FAIL  {}".format(refusal))
        return 1
    try:
        run_tools_steps(binary)
    except Exception as error:  # noqa: BLE001 - a suite reports its own failure
        print("  FAIL  the tools suite failed: {}".format(error))
        return 1
    print("  PASS  the editor's tools layer, end to end")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
