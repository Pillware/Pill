"""The editor outlives four project reloads, none of which unmaps an image.

PROTECTS: The editor keeps drawing across repeated project reloads: every entity, component, gizmo declaration, system and resource a generation records still resolves after the next generation arrives, and no image is unloaded while the process runs.
TIER: editor
SETS: editor

The host used to keep a bounded graveyard and unmap the oldest retired project
image on the fourth reload. Its guard covered column tables only, and a pointer
it did not cover crashed the standalone host with an access violation, once
inside `VCRUNTIME140.dll` and once inside the released image itself
(`native.dll_unloaded`). Retired images are now never unmapped, so this suite
asserts the other half of the same contract: four reloads in a row, a live
frame after each, and a log with no unmapping line in it.

One session drives it: the scene test project reloads once per appended
comment, four times in a row. Each round waits for a heartbeat printed *after*
its reload, so a frame that ran in the new generation is what proves survival -
and the suite finishes with a scripted request, which needs a live frame pass
to answer.

The suite is its own file rather than an area of `test_editor_egui.py`: it
needs four reloads of one project and the shared session's areas already spend
their reload budget on their own steps. It builds against an already-built
`pill_editor` binary and keeps its log, sandbox and command file under
`local/editor-validation/egui-<run id>/`, like every editor suite.
"""
import argparse
import re
import sys
from pathlib import Path

DEVOPS_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(DEVOPS_ROOT))

from core.editor_log_tokens import (  # noqa: E402
    DOCK_DUMP_PATTERN,
    EGUI_EDITOR_BINARY,
    EGUI_SHELL_LOADED_TOKEN,
    EDITOR_WINDOW_CREATED_TOKEN,
    FRAME_HEARTBEAT_TOKEN,
    PROJECT_RELOAD_TOKEN,
)
from core.editor_session import EditorSession  # noqa: E402
from core.projects import SCENE_PROJECT_PATH, SCENE_PROJECT_SCENE_RS  # noqa: E402
from core.suite_common import (  # noqa: E402
    concurrent_workspace_build_refusal,
    verify_editor_binary,
)
from editor_step_support import occurrence_count, print_ok  # noqa: E402

# The line the loader prints when it releases an image. It must never appear:
# retired generations stay mapped for the life of the process, because the
# engine cannot prove that nothing it keeps still points into one.
UNMAPPING_TOKEN = "unmapping module copy"

# Four reloads: the count the bounded graveyard needed to unmap its first image,
# so the same rounds now prove the editor survives what used to crash it.
RELOAD_ROUNDS = 4
RELOAD_TIMEOUT = 240.0
HEARTBEAT_TIMEOUT = 60.0
# The executable's own deadline, a hang guard above the rounds on a slow
# machine: each round is a cargo build of the project plus a reload pass.
SESSION_SECONDS = 420


def append_reload_marker(round_index: int) -> None:
    """Appends one comment to the project source, which the watcher reloads.

    A comment changes nothing the running code reads, so the reload is the only
    effect: it retires the generation that is loaded and maps a new one.
    """
    with SCENE_PROJECT_SCENE_RS.open("a", encoding="utf-8", newline="") as project_file:
        project_file.write("\n// reload probe {}\n".format(round_index))


def wait_for_next_occurrence(session: EditorSession, token: str, timeout: float, label: str) -> None:
    """Waits for one more occurrence of a token than the log already holds."""
    seen = occurrence_count(session, re.escape(token))
    session.wait_occurrence(re.escape(token), seen + 1, timeout, label)


def run_reload_rounds(session: EditorSession) -> None:
    """Reloads the project `RELOAD_ROUNDS` times, each followed by a live frame."""
    for round_index in range(1, RELOAD_ROUNDS + 1):
        append_reload_marker(round_index)
        wait_for_next_occurrence(
            session,
            PROJECT_RELOAD_TOKEN,
            RELOAD_TIMEOUT,
            "reload round {} of {}".format(round_index, RELOAD_ROUNDS),
        )
        # The heartbeat count is taken after the reload line landed, so the
        # frame being waited for can only be one the new generation drew. It is
        # also the wait that fails first when the editor died: a process that
        # is gone raises out of the poll with its exit code.
        wait_for_next_occurrence(
            session,
            FRAME_HEARTBEAT_TOKEN,
            HEARTBEAT_TIMEOUT,
            "a frame after reload round {}".format(round_index),
        )
        print_ok("reload round {} survived".format(round_index))


def report_no_unmapping(document: str) -> None:
    """Refuses a run that released an image, and reports what the rounds did."""
    unmappings = document.count(UNMAPPING_TOKEN)
    if unmappings != 0:
        raise AssertionError(
            "the run unmapped {} retired image(s); no image may be released "
            "while the process runs".format(unmappings)
        )
    print_ok("{} reload(s), no image unmapped".format(RELOAD_ROUNDS))


def run_reload_steps(binary: Path) -> None:
    """Starts one editor on the scene project and drives the reload rounds."""
    session = EditorSession(
        "egui",
        "reloads",
        project=SCENE_PROJECT_PATH,
        test_seconds=SESSION_SECONDS,
        binary=binary,
    )
    session.start(
        extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"},
        preflight=concurrent_workspace_build_refusal,
    )
    original_source = SCENE_PROJECT_SCENE_RS.read_bytes()
    try:
        session.wait_for_match(
            re.escape(EDITOR_WINDOW_CREATED_TOKEN), 180, "the editor window"
        )
        session.wait_for_match(
            re.escape(EGUI_SHELL_LOADED_TOKEN), 180, "the shell load"
        )
        run_reload_rounds(session)
        report_no_unmapping(session.read_log())
        # The scripted request needs a live frame pass to answer, so it proves
        # the editor is not merely still resident but still drawing.
        session.request("dump tabs", DOCK_DUMP_PATTERN, 60, "the dock after the reloads")
        print_ok("the editor answered after every reload")
    finally:
        SCENE_PROJECT_SCENE_RS.write_bytes(original_source)
        session.close()


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
        run_reload_steps(binary)
    except Exception as error:  # noqa: BLE001 - a suite reports its own failure
        print("  FAIL  the editor reload suite failed: {}".format(error))
        return 1
    print("  PASS  the editor outlived every project reload, with no image unmapped")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
