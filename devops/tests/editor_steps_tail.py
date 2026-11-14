"""The editor suite's tail steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re
import json

from core import win_clipboard
from core import log_layout
from core.editor_log_tokens import (
    CLIPBOARD_WRITTEN_TOKEN,
    DOCK_DUMP_PATTERN,
    DOCK_POPOUT_TOKEN,
    EGUI_TAB_UNDOCKED_TOKEN,
    ENTITIES_DUMP_TOKEN,
    LAYOUT_SAVED_TOKEN,
    POPOUT_CLOSED_TOKEN,
    POPOUT_OPENED_TOKEN,
    POPOUT_PRESENTED_TOKEN,
    PROBLEMS_DUMP_TOKEN,
    PROBLEM_OPENED_TOKEN,
    PROBLEM_ROW_TOKEN,
    PROJECT_RELOAD_TOKEN,
    QUIT_REQUESTED_TOKEN,
    SCENE_SURFACE_RESIZED_TOKEN,
    SELECTION_CHANGED_TOKEN,
    TOAST_PREAMBLE,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    menu_click,
    occurrence_count,
    wait_for_frames_past,
    wait_for_stub_record,
)


def run_tail_steps(state: StepState) -> None:
    """The steps through: clean exit, code 0, the rearranged layout saved."""
    session = state.session

    assert re.search(
        log_layout.message_then_fields_pattern(re.escape(DOCK_POPOUT_TOKEN), r'\s+tab="assets"'), state.survived
    ), "the reload lost the popped-out tab: {}".format(state.survived[-600:])

    # A window the reloaded generation opens presents like any other: the
    # pop-out machinery is the new image's code, not the old one's.
    begin = session.length()
    session.send("popout systems")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_OPENED_TOKEN), r'\s+tab="systems"'),
        60,
        "the reloaded shell opened a pop-out window",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_PRESENTED_TOKEN), r'\s+tab="systems"'),
        60,
        "the reloaded shell's pop-out presented a frame",
        begin,
    )
    # Both pop-outs come home, so the steps after this one see the dock
    # the earlier steps built.
    session.send("popout systems redock")
    session.send("popout assets redock")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_CLOSED_TOKEN), r'\s+tab="assets"'),
        60,
        "the surviving pop-out redocked",
        begin,
    )
    print_ok("the pop-out survived the reload and redocked")

    # Step 8: a project reload keeps the editor UI and its dock.
    project_text = state.project_source.read_bytes().decode("utf-8")
    session.edit_source(
        state.project_source, project_text + "\n// egui suite: project reload marker\n"
    )
    reloads = occurrence_count(session, re.escape(PROJECT_RELOAD_TOKEN))
    session.wait_occurrence(
        re.escape(PROJECT_RELOAD_TOKEN), reloads + 1, 240, "project reload"
    )
    after = session.request(
        "dump tabs", DOCK_DUMP_PATTERN, 30, "dock dump after project reload"
    )
    assert re.search(r"surfaces=2\b", after), (
        "the project reload reset the dock: {}".format(after)
    )
    highest = wait_for_frames_past(
        session, state.highest, 120, "frame counter survived the project reload"
    )
    print_ok("project reloaded, editor UI kept")

    # Step 8b: a build failure keeps the running generation and says so -
    # an error toast now, and a Problems row with the compiler's own
    # `file:line`. The configured code editor is
    # a stub that records the argument it was opened with, so the click's
    # effect is a file this suite can read.
    session.write_stub_editor()
    failure_start = session.length()
    session.edit_source(
        state.project_source,
        project_text
        + "\n// egui suite: project reload marker\n"
        + 'compile_error!("egui suite deliberate failure");\n',
    )
    session.wait(
        TOAST_PREAMBLE.format("error") + r'[^"]*reload failed',
        240,
        "build-failed toast",
        failure_start,
    )
    # The same failure is a Problems row with the file and line the
    # compiler printed, and a click on that row runs the
    # configured code editor command - here a stub that records its
    # arguments - with `file:line` as the last argument.
    problems_begin = session.length()
    session.send("tab problems")
    session.send("dump problems")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PROBLEMS_DUMP_TOKEN), r"\s+count=([1-9][0-9]*)"),
        60,
        "the Problems rows",
        problems_begin,
    )
    row = re.search(
        log_layout.message_then_fields_pattern(re.escape(PROBLEM_ROW_TOKEN), r'\s+module="[^"]*"\s+severity="error"\s+file="([^"]+)"\s+line=([1-9][0-9]*)\s+index=([0-9]+)'),
        session.read_since(problems_begin),
    )
    assert row, "the Problems panel showed no located error row"
    assert row.group(1).endswith(state.project_source.name), (
        "the Problems row pointed at {!r}, not the edited source".format(row.group(1))
    )
    open_begin = session.length()
    session.send("problem open {}".format(row.group(3)))
    session.wait(
        re.escape(PROBLEM_OPENED_TOKEN), 60, "the row opened in the editor", open_begin
    )
    recorded = wait_for_stub_record(session)
    # The log escapes a Windows path's backslashes; the stub writes what
    # it was given, so the expectation is unescaped before the compare.
    expected = row.group(1).replace("\\\\", "\\") + ":" + row.group(2)
    assert expected in recorded, (
        "the stub editor was opened with {!r}, not {!r}".format(recorded, expected)
    )
    print_ok("the Problems panel located the error and opened it")
    # Restoring the source builds again; the project comes back and the
    # failure's rows clear with it.
    session.edit_source(
        state.project_source, project_text + "\n// egui suite: project reload marker\n"
    )
    reloads = occurrence_count(session, re.escape(PROJECT_RELOAD_TOKEN))
    session.wait_occurrence(
        re.escape(PROJECT_RELOAD_TOKEN), reloads + 1, 240, "recovery reload"
    )
    cleared_begin = session.length()
    session.send("dump problems")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PROBLEMS_DUMP_TOKEN), r"\s+count=0"),
        60,
        "the Problems rows cleared",
        cleared_begin,
    )
    print_ok("build-failed toast, then the project recovered")


    # Step 9: the second viewport's tab floats into its own egui window;
    # its own scene surface (view=1) goes live at that window's body
    # rectangle. The dock then holds three surfaces: the main one, the
    # Entities window from step 2, and the viewport's. The resize count
    # comes before the undock, because the undock's own resize (logged
    # right after the token) is the one this step waits for.
    view_one = (
        log_layout.message_then_fields_pattern(re.escape(SCENE_SURFACE_RESIZED_TOKEN), r"\s+width=(\d+)\s+height=(\d+)\s+view=1")
    )
    before = occurrence_count(session, view_one)
    session.send("undock scene2")
    session.wait_for_match(log_layout.message_then_fields_pattern(re.escape(EGUI_TAB_UNDOCKED_TOKEN), r'\s+tab="scene2"'),
        30,
        "second viewport undock",
    )
    third = session.request(
        "dump tabs",
        DOCK_DUMP_PATTERN,
        30,
        "dock dump after the second viewport's undock",
    )
    assert re.search(r"surfaces=3\b", third), (
        "the second viewport's tab did not become a third dock surface: {}".format(third)
    )
    session.wait_occurrence(view_one, before + 1, 30, "second viewport surface")
    print_ok("second viewport undocked, its surface is live")

    # Step 10: Help -> About opens the About window, and File -> Quit
    # Editor leaves through the shell's own request - the graceful quit
    # and its clean exit code.
    menu_click(session, "help", "About Pill Editor", "About Pill Editor")
    print_ok("the Help menu opened the About window")
    # The entity copy the restart session pastes is taken here, one step
    # before the quit. The clipboard is the OS's - any other program can
    # replace it - so the copy is refreshed right before the editor leaves
    # instead of two minutes earlier, and read back to prove it landed.
    # The copy follows the selection, and the asset steps above left an
    # asset selected: a copy of that writes an asset payload, not an
    # entity. The handle comes from the list the editor has now, because
    # the reloads above renumber the display handles.
    listed = session.request(
        "dump entities",
        re.escape(ENTITIES_DUMP_TOKEN),
        30,
        "the entity list for the copy",
    )
    handles = re.findall(r'display="([^"]+)"', listed)
    assert handles, "no entity to copy: {}".format(listed[-400:])
    begin = session.length()
    session.send("select entity {}".format(handles[0]))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), r'\s+entity=[^ ]+\s+count=1\b'),
        30,
        "the entity re-selected for the copy",
        begin,
    )
    win_clipboard.clear()
    begin = session.length()
    session.send("command editor.copy")
    session.wait(
        re.escape(CLIPBOARD_WRITTEN_TOKEN), 30, "the second clipboard write", begin
    )
    copied = json.loads(win_clipboard.read_payload())
    assert copied.get("kind") == "entity", (
        "the entity copy did not reach the clipboard: {}".format(copied)
    )
    print_ok("an entity copy sits on the OS clipboard for the next editor")
    begin = session.length()
    menu_click(session, "file", "Quit Editor", "Quit Editor")
    session.wait(re.escape(QUIT_REQUESTED_TOKEN), 30, "the quit reached the frontend", begin)
    # The quit is where the rearranged dock is written: the undocked tab
    # and the second viewport each float on a surface of their own.
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(LAYOUT_SAVED_TOKEN), r"\s+surfaces=3\b"),
        30,
        "the layout saved on quit",
        begin,
    )
    status = session.wait_exit(timeout=60)
    assert status == 0, f"editor exited {status} on the menu's quit"
    assert "panic" not in session.text(), "a panic was logged"
    print_ok("clean exit, code 0, the rearranged layout saved")


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_tail_steps(context.state)


AREA = StepArea(
    name="tail",
    order=12,
    run=_area_entry,
    needs=("highest", "survived",),
    provides=(),
)
