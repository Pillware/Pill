"""The editor suite's lifecycle steps: restart, recovery, cadence, posture, panels.

Each function here is one step area of `test_editor_egui.py` that runs after
the suite's main session has closed: it opens its own sessions on the same
binary and project, and asserts on their logs exactly as the earlier steps do.
The suite's `main` keeps the parser, the preflight, the main session and the
`finally` restore, and calls `run_lifecycle_steps` once.

The step bodies are moved verbatim; their names (log patterns and tokens) come
from `core.editor_log_tokens`, the one owner of the vocabulary they share
with the steps that stay in `main`.
"""

import json
import re
import time
from pathlib import Path

from core import win_clipboard
from core import log_layout
from core.editor_log_tokens import EDITOR_WINDOW_CREATED_TOKEN
from core.editor_session import EditorSession
from editor_step_areas import StepArea
from core.paths import MODULES_ROOT, REPOSITORY_ROOT
from core.suite_common import MODULE_RELOAD_COMPLETE_TOKEN
from core.editor_log_tokens import (
    CADENCE_DUMP_TOKEN,
    CLIPBOARD_PASTED_TOKEN,
    DOCK_DUMP_PATTERN,
    DOCK_LEAF_PATTERN,
    EGUI_SHELL_LOADED_TOKEN,
    EGUI_TAB_UNDOCKED_TOKEN,
    ENTITIES_DUMP_TOKEN,
    ENTITY_ROW_TOKEN,
    FIELD_TOKEN,
    FRAME_HEARTBEAT_TOKEN,
    FRAME_IDLE_TOKEN,
    HISTORY_PUSH_TOKEN,
    HISTORY_UNDO_TOKEN,
    LAYOUT_SAVED_TOKEN,
    NO_OVERLAY_TOKEN,
    OVERLAY_POSTURE_TOKEN,
    PANELS_DUMP_TOKEN,
    PANEL_FIELD_QUEUED_TOKEN,
    PANEL_LOG_TOKEN,
    PANEL_MODAL_ANSWERED_TOKEN,
    PANEL_OVER_BUDGET_TOKEN,
    PANEL_REGISTERED_TOKEN,
    PANEL_RESTORED_TOKEN,
    PANEL_ROW_TOKEN,
    PANEL_UNLOADED_TOKEN,
    PANEL_WINDOW_OPENED_TOKEN,
    PANEL_WINDOW_REQUESTED_TOKEN,
    PAUSE_TOGGLED_TOKEN,
    PREFERENCES_DUMP_TOKEN,
    PREFERENCES_SAVED_TOKEN,
    PROBLEMS_DUMP_TOKEN,
    PROBLEM_ROW_TOKEN,
    QUIT_REQUESTED_TOKEN,
    RECOVERED_STATE_TOKEN,
    REDRAW_ON_DEMAND_TOKEN,
    SAFE_MODE_RETRY_TOKEN,
    SAFE_MODE_TOKEN,
    SCENE_NEW_TOKEN,
    SCENE_OPENED_TOKEN,
    SCENE_SAVED_TOKEN,
    SCENE_TITLE_TOKEN,
    SCENE_VIEW_RECT_TOKEN,
    SHOWCASE_COMPONENT,
    SYSTEMS_DUMP_TOKEN,
    SYSTEM_ROW_PATTERN,
    TOAST_TOKEN,
    WINDOWS_DUMP_TOKEN,
    WINDOW_ROW_TOKEN,
)


from editor_step_support import print_ok


def run_lifecycle_steps(session: EditorSession, binary: Path, project: str) -> None:
    """Runs the five step areas that follow the main session, in order."""
    step_11_layout_survives_restart(session, binary, project)
    step_11b_recovery_after_unclean_exit(binary, project)
    step_12_redraw_cadence(binary, project)
    step_13_no_overlay_posture(binary, project)
    step_14_module_panels(session, binary, project)


AREAS = (
    StepArea(
        name="layout_survives_restart",
        order=13,
        run=lambda context: step_11_layout_survives_restart(
            context.session, context.binary, context.project
        ),
        sequence_only=(
            "the dock layout and the clipboard payload the earlier session wrote"
        ),
    ),
    StepArea(
        name="recovery_after_unclean_exit",
        order=14,
        run=lambda context: step_11b_recovery_after_unclean_exit(
            context.binary, context.project
        ),
        sequence_only=(
            "the recovery document the killed session had already autosaved"
        ),
    ),
    StepArea(
        name="redraw_cadence",
        order=15,
        run=lambda context: step_12_redraw_cadence(context.binary, context.project),
    ),
    StepArea(
        name="no_overlay_posture",
        order=16,
        run=lambda context: step_13_no_overlay_posture(
            context.binary, context.project
        ),
    ),
    StepArea(
        name="module_panels",
        order=17,
        run=lambda context: step_14_module_panels(
            context.session, context.binary, context.project
        ),
    ),
)


def step_11_layout_survives_restart(
    session: EditorSession, binary: Path, project: str
) -> None:
    # Step 11: the layout survives a restart. The session
    # above quit through File -> Quit, which wrote the rearranged dock - the
    # second viewport floating on its own surface, in front - into the
    # sandbox; a second session on the same sandbox must open with it.
    # The copy the restart pastes crosses a process boundary through the OS
    # clipboard, so it is checked here as well: a payload another program
    # replaced reports itself, instead of surfacing as a timeout a minute into
    # the restarted editor.
    leftover = win_clipboard.read_payload()
    assert leftover and json.loads(leftover).get("kind") == "entity", (
        "the entity copy did not survive the quit: payload={!r} text={!r}".format(
            leftover, win_clipboard.read_text()
        )
    )
    # The bytes the first editor wrote, kept for the paste below: the clipboard
    # is machine-global, and the restart takes long enough (three sessions'
    # worth of startup) that another program can replace the payload in
    # between. Restoring the copy the first editor wrote keeps the step about
    # the payload crossing a process boundary rather than about the machine
    # staying quiet.
    entity_payload = leftover
    restart = EditorSession(
        "egui",
        "restart",
        project=project,
        test_seconds=240,
        binary=binary,
        sandbox=session.sandbox,
    )
    restart.start(extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"})
    try:
        restart.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 120, "restart startup"
        )
        restart.wait_for_match(re.escape(EGUI_SHELL_LOADED_TOKEN), 120, "restart shell load"
        )
        restored = restart.request(
            "dump tabs", DOCK_DUMP_PATTERN, 60, "the restored dock"
        )
        assert re.search(r"surfaces=3\b", restored), (
            "the rearranged dock did not survive the restart: {}".format(restored)
        )
        leaves = restart.text()
        floating = [
            match.group(0)
            for match in re.finditer(DOCK_LEAF_PATTERN, leaves)
            if (match.group(1), match.group(3)) in (("1", "entities"), ("2", "scene2"))
        ]
        assert len(floating) == 2, (
            "the floating surfaces did not come back with their tabs: {}".format(
                leaves[-2000:]
            )
        )
        print_ok("the rearranged dock came back after a restart")

        # The third scene view came back too: the layout
        # carried its tab, so this process opened a window and a surface for
        # it before it drew anything.
        begin = restart.length()
        restart.send("dump scenes")
        restart.wait(
            log_layout.message_then_fields_pattern(re.escape(SCENE_VIEW_RECT_TOKEN), r"\s+view=2\b"),
            60,
            "the third view after the restart",
            begin,
        )
        assert "scene3" in restart.text(), (
            "the third scene view's tab did not survive the restart"
        )
        print_ok("the third scene view survived the restart")

        # The clipboard crossed the process boundary: the
        # session above copied an entity onto the OS clipboard, and this
        # editor - a different process - pastes it as one recorded step.
        entities_here = len(
            re.findall(
                re.escape(ENTITY_ROW_TOKEN),
                restart.request(
                    "dump entities",
                    re.escape(ENTITIES_DUMP_TOKEN),
                    60,
                    "the restarted editor's entity list",
                ),
            )
        )
        begin = restart.length()
        # The payload is put back when something else took the clipboard while
        # the restart was starting; without this the paste reports a decode
        # failure from a program the suite never asked, one step from the end.
        current = win_clipboard.read_payload()
        if current != entity_payload:
            win_clipboard.write_payload(entity_payload)
            print_ok(
                "the clipboard was restored for the restart: {!r}".format(
                    (current or "")[:80]
                )
            )
        restart.send("command editor.paste")
        restart.wait(
            log_layout.message_then_fields_pattern(re.escape(CLIPBOARD_PASTED_TOKEN), r'\s+kind="entity"'),
            60,
            "the pasted entity",
            begin,
        )
        pasted = len(
            re.findall(
                re.escape(ENTITY_ROW_TOKEN),
                restart.request(
                    "dump entities",
                    re.escape(ENTITIES_DUMP_TOKEN),
                    60,
                    "the entity list after the paste",
                ),
            )
        )
        assert pasted == entities_here + 1, (
            "the paste did not add exactly one entity: {} vs {}".format(
                pasted, entities_here
            )
        )
        begin = restart.length()
        restart.send("command editor.undo")
        restart.wait(
            re.escape(HISTORY_UNDO_TOKEN), 30, "the pasted entity's undo", begin
        )
        restored = len(
            re.findall(
                re.escape(ENTITY_ROW_TOKEN),
                restart.request(
                    "dump entities",
                    re.escape(ENTITIES_DUMP_TOKEN),
                    60,
                    "the entity list after the undo",
                ),
            )
        )
        assert restored == entities_here, (
            "the undo did not remove the pasted entity: {} vs {}".format(
                restored, entities_here
            )
        )
        print_ok("an entity copied in one editor pasted into another")

        # Leave through the scripted channel's own quit: the first click of a
        # freshly started session goes to focusing its window, and the menu's
        # quit path is the main session's check, not this one.
        begin = restart.length()
        restart.send("quit")
        restart.wait(
            re.escape(QUIT_REQUESTED_TOKEN), 30, "the restart's quit", begin
        )
        restart.wait(
            re.escape(LAYOUT_SAVED_TOKEN), 30, "the restart's layout save", begin
        )
        assert restart.wait_exit(timeout=60) == 0, "the restarted editor did not exit 0"
        print_ok("the restarted editor saved its layout and exited 0")
        # A clean exit leaves nothing to recover.
        recovery_file = restart.editor_state_directory / "editor_egui_recovery.yaml"
        assert not recovery_file.exists(), (
            "a clean exit left a recovery document behind: {}".format(recovery_file)
        )
        print_ok("a clean exit removed the recovery document")
    finally:
        restart.close()


def step_11b_recovery_after_unclean_exit(binary: Path, project: str) -> None:
    # Step 11b: recovery after an unclean exit. A killed
    # editor leaves its dock and selection in the recovery document; the next
    # run applies them and says so, and its own clean exit removes the file.
    recovered = EditorSession(
        "egui",
        "recovery",
        project=project,
        test_seconds=180,
        binary=binary,
    )
    recovered.start(extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"})
    try:
        recovered.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 120, "recovery startup"
        )
        recovered.wait_for_match(re.escape(EGUI_SHELL_LOADED_TOKEN), 120, "recovery shell load"
        )
        # The document exists from the first frames; a change the autosave
        # watches (here an undock) rewrites it, which is what the survivor must
        # see, so the step waits for that write rather than for the file.
        recovery_file = recovered.editor_state_directory / "editor_egui_recovery.yaml"
        deadline = time.time() + 30
        while time.time() < deadline and not recovery_file.exists():
            # The editor writes the document on its own cadence; poll the
            # file system until it appears or the deadline passes.
            time.sleep(0.25)
        assert recovery_file.exists(), (
            "the running editor never wrote its recovery document: {}".format(
                recovery_file
            )
        )
        written_before = recovery_file.stat().st_mtime_ns
        recovered.send("undock entities")
        recovered.wait_for_match(re.escape(EGUI_TAB_UNDOCKED_TOKEN), 60, "the recovery undock"
        )
        deadline = time.time() + 30
        while time.time() < deadline and recovery_file.stat().st_mtime_ns == written_before:
            # The autosave also fires on change; wait for a newer mtime so the
            # comparison below cannot read the previous write.
            time.sleep(0.25)
        assert recovery_file.stat().st_mtime_ns != written_before, (
            "the undock did not reach the recovery document"
        )
        print_ok("the autosave recorded the rearranged dock")

        # The unclean exit: the process is killed, so nothing removes the file.
        process = recovered.process
        assert process is not None, "the recovery session did not start"
        process.kill()
        process.wait(timeout=60)
        recovered.close()
        assert recovery_file.exists(), (
            "the killed editor's recovery document went away: {}".format(recovery_file)
        )

        survivor = EditorSession(
            "egui",
            "recovered",
            project=project,
            test_seconds=180,
            binary=binary,
            sandbox=recovered.sandbox,
        )
        survivor.start(extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"})
        try:
            survivor.wait_for_match(re.escape(RECOVERED_STATE_TOKEN),
                120,
                "the recovered state",
            )
            restored = survivor.request(
                "dump tabs", DOCK_DUMP_PATTERN, 60, "the recovered dock"
            )
            assert re.search(r"surfaces=2\b", restored), (
                "the rearranged dock did not come back from the recovery document: {}".format(
                    restored
                )
            )
            begin = survivor.length()
            survivor.send("quit")
            survivor.wait(
                re.escape(QUIT_REQUESTED_TOKEN), 30, "the survivor's quit", begin
            )
            assert survivor.wait_exit(timeout=60) == 0, (
                "the recovering editor did not exit 0"
            )
        finally:
            survivor.close()
        assert not recovery_file.exists(), (
            "the recovering editor's clean exit left the document behind"
        )
        print_ok("a killed editor's dock came back, and its clean exit cleared it")
    finally:
        recovered.close()


def step_12_redraw_cadence(binary: Path, project: str) -> None:
    # Step 12: the redraw cadence. Without the continuous
    # switch, a paused idle editor sleeps between frames: two dumps three
    # seconds apart must not advance by the display's frame rate. Every other
    # session runs with the switch, so the older steps keep their pace.
    cadence = EditorSession(
        "egui", "cadence", project=project, test_seconds=180, binary=binary
    )
    cadence.start(extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "0"})
    try:
        cadence.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 120, "cadence startup"
        )
        assert REDRAW_ON_DEMAND_TOKEN in cadence.text(), (
            "the editor never reported its redraw posture"
        )
        begin = cadence.length()
        cadence.send("command editor.pause_toggle")
        cadence.wait(
            log_layout.message_then_fields_pattern(re.escape(PAUSE_TOGGLED_TOKEN), r"\s+paused=true"),
            60,
            "the project paused for the cadence step",
            begin,
        )
        # A settled editor says so once, with the poll it settles into:
        # a running project draws at the display rate and a paused one stops
        # drawing until something asks for a frame.
        cadence.wait(
            log_layout.message_then_fields_pattern(re.escape(FRAME_IDLE_TOKEN), r"\s+poll_millis=250\b"),
            60,
            "the editor settled into its idle poll",
            begin,
        )
        cadence_frames = []
        for index in range(2):
            begin = cadence.length()
            cadence.send("dump cadence")
            cadence.wait(re.escape(CADENCE_DUMP_TOKEN), 60, "the cadence dump", begin)
            dump = re.search(
                log_layout.message_then_fields_pattern(re.escape(CADENCE_DUMP_TOKEN), r"\s+continuous=(?:true|false)\s+frames=(\d+)"),
                cadence.read_since(begin),
            )
            assert dump is not None, "the cadence dump carried no frame count"
            cadence_frames.append(int(dump.group(1)))
            if index == 0:
                # The second dump is the one an idle stretch separates from
                # the first; the suite's own sleep is that stretch.
                time.sleep(3.0)
        idle_frames = cadence_frames[1] - cadence_frames[0]
        # A continuous loop draws at the display's pace - well over a hundred
        # frames in three seconds - while the idle poll owes twelve plus
        # whatever the host's own log lines changed.
        assert idle_frames <= 45, (
            "the editor drew {} frames in three idle seconds; the loop is not "
            "waiting (frames={})".format(idle_frames, cadence_frames)
        )
        print_ok(
            "the paused editor waited ({} frames in three seconds)".format(
                idle_frames
            )
        )
    finally:
        cadence.close()


def step_13_no_overlay_posture(binary: Path, project: str) -> None:
    # Step 13: the no-overlay posture. `PILL_EDITOR_NO_OVERLAY`
    # puts the UI on the window's own surface instead of the composition
    # overlay; the editor must still start, dock and edit, and the warning must
    # be said once rather than every frame.
    plain = EditorSession(
        "egui", "no-overlay", project=project, test_seconds=180, binary=binary
    )
    plain.start(
        extra_env={
            "PILL_EDITOR_NO_OVERLAY": "1",
            "PILL_EDITOR_CONTINUOUS_REDRAW": "1",
        }
    )
    try:
        plain.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 120, "no-overlay startup"
        )
        plain.wait_for_match(re.escape(EGUI_SHELL_LOADED_TOKEN), 120, "the shell in that posture"
        )
        log = plain.text()
        assert NO_OVERLAY_TOKEN in log, (
            "the editor never reported the no-overlay posture: {}".format(log[-800:])
        )
        assert log.count(NO_OVERLAY_TOKEN) == 1, (
            "the no-overlay warning repeated {} times".format(
                log.count(NO_OVERLAY_TOKEN)
            )
        )
        assert re.search(log_layout.message_then_fields_pattern(re.escape(OVERLAY_POSTURE_TOKEN), r"\s+window_surface=true"), log), (
            "the posture line did not name the window surface: {}".format(log[-800:])
        )
        # Docks: a tab is activated and undocked, and the dock reports the
        # second surface it became.
        plain.send("tab statistics")
        plain.send("undock statistics")
        dock = plain.request(
            "dump tabs", DOCK_DUMP_PATTERN, 60, "the dock in that posture"
        )
        surfaces = re.search(DOCK_DUMP_PATTERN, dock)
        assert surfaces and int(surfaces.group(1)) >= 2, (
            "the undocked tab did not become a surface: {}".format(dock[-600:])
        )
        assert "statistics" in dock, "the dock never named the undocked tab"
        # Edits: a preference row goes through the same queued path a click
        # takes, and reads back through the dump.
        begin = plain.length()
        plain.send("edit preference snapping_steps.rotation_degrees 9.5")
        plain.wait(
            re.escape(PREFERENCES_SAVED_TOKEN), 60, "the edit saved", begin
        )
        begin = plain.length()
        plain.send("dump preferences")
        plain.wait(re.escape(PREFERENCES_DUMP_TOKEN), 60, "the edit read back", begin)
        assert re.search(
            r'snapping_rotation_degrees="9\.5"', plain.read_since(begin)
        ), plain.read_since(begin)[-600:]
        print_ok("the no-overlay posture started, docked and edited")
    finally:
        plain.close()


def step_14_module_panels(session: EditorSession, binary: Path, project: str) -> None:
    # Step 14: module panels. The shell module registers an
    # editor panel of its own; the tab docks, the layout remembers it by panel
    # id, and a panel whose module unloaded reads "module not loaded" rather
    # than calling into an image that is gone. The deliberately slow body
    # proves the budget: a badge on the tab and a warning toast.
    panels = EditorSession(
        "egui", "panels", project=project, test_seconds=240, binary=binary
    )
    panels.start(
        extra_env={
            "PILL_EDITOR_CONTINUOUS_REDRAW": "1",
            "PILL_TEST_PANEL_SLOW": "1",
        }
    )
    try:
        panels.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 120, "panel startup"
        )
        panels.wait_for_match(log_layout.message_then_fields_pattern(re.escape(PANEL_REGISTERED_TOKEN), r'\s+id="shell\.demo"'),
            120,
            "the demo panel registration",
        )
        dump = panels.request(
            "dump panels", re.escape(PANELS_DUMP_TOKEN), 60, "the panel dump"
        )
        row = re.search(
            log_layout.message_then_fields_pattern(re.escape(PANEL_ROW_TOKEN), r'\s+slot=(\d+)\s+id="shell\.demo"\s+source="[^"]+"\s+alive=true'),
            dump,
        )
        assert row is not None, "the demo panel is not registered: {}".format(
            dump[-800:]
        )
        slot = row.group(1)
        # Docking it draws it every frame, which is what the slow body needs to
        # cross the 120-frame budget.
        begin = panels.length()
        panels.send("tab module:{}".format(slot))
        panels.wait(
            re.escape(PANEL_OVER_BUDGET_TOKEN), 180, "the over-budget panel", begin
        )
        dump = panels.request(
            "dump panels", re.escape(PANELS_DUMP_TOKEN), 60, "the armed panel dump"
        )
        assert re.search(
            log_layout.message_then_fields_pattern(re.escape(PANEL_ROW_TOKEN), r"\s+slot={}\s+.*?slow=true").format(slot),
            dump,
        ), "the panel's budget badge never armed: {}".format(dump[-800:])
        assert re.search(
            log_layout.message_then_fields_pattern(re.escape(TOAST_TOKEN), r'\s+level="warning"\s+seq=\d+\s+text="Panel .* over its frame budget'),
            panels.text(),
        ), "the over-budget panel queued no toast"
        print_ok("a slow module panel armed its badge and queued a toast")
        # The module unloads: its marker is dropped, and its panel reads so.
        begin = panels.length()
        panels.send("panel unload pill_editor_shell")
        panels.wait(re.escape(PANEL_UNLOADED_TOKEN), 60, "the unload", begin)
        dump = panels.request(
            "dump panels", re.escape(PANELS_DUMP_TOKEN), 60, "the unloaded panel dump"
        )
        assert re.search(
            log_layout.message_then_fields_pattern(re.escape(PANEL_ROW_TOKEN), r"\s+slot={}\s+.*?alive=false").format(slot),
            dump,
        ), "the unloaded panel still reads alive: {}".format(dump[-800:])
        assert "module not loaded" in dump or "Module not loaded" in panels.text(), (
            "the tab did not say the module is gone: {}".format(dump[-800:])
        )
        begin = panels.length()
        panels.send("panel restore pill_editor_shell")
        panels.wait(
            re.escape(PANEL_RESTORED_TOKEN), 60, "the restore", begin
        )
        assert re.search(
            re.escape(PANEL_RESTORED_TOKEN) + r".*?restored=[1-9][0-9]*",
            panels.read_since(begin),
        ), "the restore registered nothing: {}".format(panels.read_since(begin)[-400:])
        dump = panels.request(
            "dump panels", re.escape(PANELS_DUMP_TOKEN), 60, "the restored panel dump"
        )
        assert re.search(
            log_layout.message_then_fields_pattern(re.escape(PANEL_ROW_TOKEN), r"\s+slot={}\s+.*?alive=true").format(slot),
            dump,
        ), "the restored panel did not come back: {}".format(dump[-800:])
        print_ok("a panel outlived its module's unload and came back")

        # Stages 12 and 13: the shell module's other two panels - the Spline
        # Tool, and the windows and modals a panel asks the shell for. The
        # demo panel's confirmation is driven through `panel window`, which
        # reaches the same code its own button does, and answered with Enter.
        dump = panels.request(
            "dump panels", re.escape(PANELS_DUMP_TOKEN), 60, "the spline panel"
        )
        spline = re.search(
            log_layout.message_then_fields_pattern(re.escape(PANEL_ROW_TOKEN), r'\s+slot=(\d+)\s+id="shell\.spline_tool"'),
            dump,
        )
        assert spline is not None, (
            "the Spline Tool did not register: {}".format(dump[-800:])
        )
        spline_slot = spline.group(1)
        panels.send("tab module:{}".format(spline_slot))
        # The tab has to be drawn before its panel has a timing: the dump that
        # reads it is retried until it does, rather than raced.
        def drawn_panel(_text: str):
            dump = panels.request(
                "dump panels", re.escape(PANELS_DUMP_TOKEN), 60, "the drawn spline panel"
            )
            row = re.search(
                log_layout.message_then_fields_pattern(re.escape(PANEL_ROW_TOKEN), r"\s+slot={}\s+.*?passes=(\d+)").format(spline_slot),
                dump,
            )
            return row if row is not None and int(row.group(1)) > 0 else None

        panels.wait_until(
            drawn_panel,
            30,
            "the Spline Tool never drew\n",
            label="the Spline Tool registered and drew",
            # The panel's draw line arrives on its own frame; each attempt asks
            # for a fresh dump, so the poll is the dump's own cadence.
            poll=0.5,
        )

        # A control point is a leaf of the spline's struct array, and
        # a panel row writes it through the panel contract - a queued
        # `SetField`, applied at the frame boundary as a recorded edit. The row
        # itself is a drag value the test channel cannot press, so the write is
        # queued where the row puts it (`panel field`); the row-to-command
        # mapping is the shell crate's `spline_tool` unit tests, and what this
        # proves is the rest of the path: the queue, the recorded step and the
        # undo, on a real entity of a real project.
        dump = panels.request(
            "dump entities", re.escape(ENTITIES_DUMP_TOKEN), 60, "the entities"
        )
        showcase_row = next(
            (
                match
                for match in re.finditer(
                    log_layout.message_then_fields_pattern(re.escape(ENTITY_ROW_TOKEN), r'\s+display="([^"]+)"\s+components="([^"]*)"'),
                    dump,
                )
                if SHOWCASE_COMPONENT in match.group(2)
            ),
            None,
        )
        assert showcase_row is not None, (
            "no entity carries the showcase component: {}".format(dump[-800:])
        )
        showcase_entity = showcase_row.group(1)
        panels.send("select entity {}".format(showcase_entity))
        leaf = "points[1].y"
        before = panels.request(
            "dump field {} {} {}".format(showcase_entity, SHOWCASE_COMPONENT, leaf),
            re.escape(FIELD_TOKEN),
            60,
            "the panel element's value",
        )
        original = re.search(r"value=(?:F32\()?(-?[0-9.]+)", before)
        assert original is not None, (
            "the panel element did not read back: {}".format(before[-600:])
        )
        begin = panels.length()
        panels.send("panel field {} {} 4.5".format(SHOWCASE_COMPONENT, leaf))
        panels.wait(
            re.escape(PANEL_FIELD_QUEUED_TOKEN), 60, "the panel's queued field write", begin
        )
        queued = panels.read_since(begin)
        assert re.search(
            r'component="[^"]*{}"\s+field="{}"\s+value="4\.5"'.format(
                re.escape(SHOWCASE_COMPONENT), re.escape(leaf)
            ),
            queued,
        ), "the panel queued a different write: {}".format(queued[-600:])
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="set {}[^"]*"').format(re.escape(SHOWCASE_COMPONENT)),
            60,
            "the panel edit recorded a step",
            begin,
        )
        begin = panels.length()
        panels.send(
            "dump field {} {} {}".format(showcase_entity, SHOWCASE_COMPONENT, leaf)
        )
        panels.wait(re.escape(FIELD_TOKEN), 60, "the panel-edited leaf", begin)
        assert "4.5" in panels.read_since(begin), (
            "the panel's write did not apply: {}".format(
                panels.read_since(begin)[-600:]
            )
        )
        begin = panels.length()
        panels.send("command editor.undo")
        panels.wait(re.escape(HISTORY_UNDO_TOKEN), 60, "the panel write's undo", begin)
        begin = panels.length()
        panels.send(
            "dump field {} {} {}".format(showcase_entity, SHOWCASE_COMPONENT, leaf)
        )
        panels.wait(re.escape(FIELD_TOKEN), 60, "the restored panel leaf", begin)
        restored = re.search(r"value=(?:F32\()?(-?[0-9.]+)", panels.read_since(begin))
        assert restored is not None and float(restored.group(1)) == float(
            original.group(1)
        ), "the undo did not restore the panel's element: {}".format(
            panels.read_since(begin)[-600:]
        )
        print_ok("a panel row's control-point write applied and undid")
        # The confirmation lives on the demo panel's tab, so that tab comes to
        # the front first: `panel window` stands in for clicking the panel's
        # own button, and a button is only clickable while its tab is shown.
        panels.send("tab module:{}".format(slot))
        begin = panels.length()
        panels.send("panel window {} demo.confirm".format(slot))
        panels.wait(
            re.escape(PANEL_WINDOW_REQUESTED_TOKEN), 60, "the window request", begin
        )
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(PANEL_WINDOW_OPENED_TOKEN), r'\s+slot=\d+\s+window="demo\.confirm"'),
            60,
            "the panel window",
            begin,
        )
        dump = panels.request(
            "dump windows", re.escape(WINDOWS_DUMP_TOKEN), 60, "the panel windows"
        )
        assert re.search(r"count=1\b", dump), (
            "the panel window is not on screen: {}".format(dump[-600:])
        )
        assert re.search(
            log_layout.message_then_fields_pattern(re.escape(WINDOW_ROW_TOKEN), r'\s+id="demo\.confirm".*?modal=true'), dump
        ), "the window is not modal: {}".format(dump[-600:])
        begin = panels.length()
        panels.send("key Enter")
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(PANEL_MODAL_ANSWERED_TOKEN), r'\s+window="[^"]+"\s+accepted=true'),
            60,
            "the modal answer",
            begin,
        )
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(PANEL_LOG_TOKEN), r'\s+text="demo confirmed"'),
            60,
            "the panel's confirmation",
            begin,
        )
        dump = panels.request(
            "dump windows", re.escape(WINDOWS_DUMP_TOKEN), 60, "the closed window"
        )
        assert re.search(r"count=0\b", dump), (
            "the answered modal is still on screen: {}".format(dump[-600:])
        )
        print_ok("a modal window opened, took Enter and closed")
        # A window dies with its module: open it again, unload the source, and
        # the shell drops it rather than drawing through a dead image.
        begin = panels.length()
        panels.send("panel window {} demo.confirm".format(slot))
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(PANEL_WINDOW_OPENED_TOKEN), r'\s+slot=\d+\s+window="demo\.confirm"'),
            60,
            "the reopened window",
            begin,
        )
        begin = panels.length()
        panels.send("panel unload pill_editor_shell")
        panels.wait(re.escape(PANEL_UNLOADED_TOKEN), 60, "the second unload", begin)
        dump = panels.request(
            "dump windows", re.escape(WINDOWS_DUMP_TOKEN), 60, "the unloaded windows"
        )
        assert re.search(r"count=0\b", dump), (
            "an unloaded module's window is still on screen: {}".format(dump[-600:])
        )
        begin = panels.length()
        panels.send("panel restore pill_editor_shell")
        panels.wait(
            re.escape(PANEL_RESTORED_TOKEN),
            60,
            "the second restore",
            begin,
        )
        begin = panels.length()
        panels.send("panel window {} demo.confirm".format(slot))
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(PANEL_WINDOW_OPENED_TOKEN), r'\s+slot=\d+\s+window="demo\.confirm"'),
            60,
            "the re-registered window",
            begin,
        )
        print_ok("a module's window closes on its unload and reopens after")
        begin = panels.length()
        panels.send("key Escape")
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(PANEL_MODAL_ANSWERED_TOKEN), r'\s+window="[^"]+"\s+accepted=false'),
            60,
            "the dismiss",
            begin,
        )
        print_ok("Escape dismisses a modal")
        # A scene saves, empties and opens again. Save
        # writes the live world to a new `.scene` under the project's asset
        # directory, New replaces it with nothing, and Open brings the same
        # entities back. The window title names the file the whole time.
        scene_path = "scenes/egui_suite.scene"
        scene_file = (MODULES_ROOT / project / "res" / scene_path).resolve()
        scene_file.unlink(missing_ok=True)
        begin = panels.length()
        panels.send("scene save {}".format(scene_path))
        panels.wait(re.escape(SCENE_SAVED_TOKEN), 60, "the scene save", begin)
        saved = re.search(
            log_layout.message_then_fields_pattern(re.escape(SCENE_SAVED_TOKEN), r'\s+path="scenes/egui_suite\.scene"\s+guid=\S+\s+entities=(\d+)'),
            panels.read_since(begin),
        )
        assert saved is not None, "the save did not name the scene it wrote"
        saved_count = int(saved.group(1))
        assert saved_count > 0, "the save wrote no entities"
        assert scene_file.is_file(), "the scene file was not written: {}".format(
            scene_file
        )
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(SCENE_TITLE_TOKEN), r'\s+title="egui_suite\.scene"'),
            60,
            "the title naming the saved scene",
            begin,
        )
        begin = panels.length()
        panels.send("scene new")
        panels.wait(re.escape(SCENE_NEW_TOKEN), 60, "the new scene", begin)
        removed = re.search(
            log_layout.message_then_fields_pattern(re.escape(SCENE_NEW_TOKEN), r"\s+entities=(\d+)"),
            panels.read_since(begin),
        )
        assert removed is not None and int(removed.group(1)) == saved_count, (
            "the new scene did not empty the world: {}".format(
                (removed.group(0) if removed else "no scene new line")
            )
        )
        begin = panels.length()
        panels.send("scene open {}".format(scene_path))
        panels.wait(re.escape(SCENE_OPENED_TOKEN), 60, "the scene open", begin)
        reopened = re.search(
            log_layout.message_then_fields_pattern(re.escape(SCENE_OPENED_TOKEN), r'\s+path="scenes/egui_suite\.scene"\s+guid=\S+\s+entities=(\d+)')
            + r"\s+components=(\d+)",
            panels.read_since(begin),
        )
        assert reopened is not None, "the open did not name the scene it read"
        assert int(reopened.group(1)) == saved_count, (
            "the scene opened with {} of {} entities".format(
                reopened.group(1), saved_count
            )
        )
        assert int(reopened.group(2)) > 0, "the scene opened with no components"
        panels.wait(
            log_layout.message_then_fields_pattern(re.escape(SCENE_TITLE_TOKEN), r'\s+title="egui_suite\.scene"'),
            60,
            "the title naming the opened scene",
            begin,
        )
        print_ok("a scene saved, emptied and opened with its entities")
        # The scene is a suite artifact: it must not outlive the run that made
        # it, or the next start's asset scan finds it.
        scene_file.unlink(missing_ok=True)
        # Float the tab and leave: the layout writes the panel id beside it.
        panels.send("undock module:{}".format(slot))
        dock = panels.request(
            "dump tabs", DOCK_DUMP_PATTERN, 60, "the panel dock"
        )
        surfaces = re.search(DOCK_DUMP_PATTERN, dock)
        assert surfaces and int(surfaces.group(1)) >= 2, dock[-600:]
        assert any(
            "module" in match.group(4)
            for match in re.finditer(DOCK_LEAF_PATTERN, dock)
        ), "the panel tab never reached the dock: {}".format(dock[-600:])
        begin = panels.length()
        panels.send("quit")
        panels.wait(re.escape(QUIT_REQUESTED_TOKEN), 30, "the panel quit", begin)
        panels.wait(re.escape(LAYOUT_SAVED_TOKEN), 30, "the panel layout", begin)
        assert panels.wait_exit(timeout=60) == 0, "the panel editor did not exit 0"
    finally:
        panels.close()

    # The layout remembered the panel by id, not by slot: a second session on
    # the same sandbox opens with the module tab still there.
    panels_restart = EditorSession(
        "egui",
        "panels-restart",
        project=project,
        test_seconds=180,
        binary=binary,
        sandbox=panels.sandbox,
    )
    panels_restart.start(extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"})
    try:
        panels_restart.wait_for_match(re.escape(EGUI_SHELL_LOADED_TOKEN),
            120,
            "the panel restart",
        )
        dump = panels_restart.request(
            "dump tabs", DOCK_DUMP_PATTERN, 60, "the restored panel dock"
        )
        assert any(
            "module" in match.group(4)
            for match in re.finditer(DOCK_LEAF_PATTERN, dump)
        ), "the module tab did not survive the restart: {}".format(dump[-800:])
        restored = panels_restart.request(
            "dump panels", re.escape(PANELS_DUMP_TOKEN), 60, "the restart's panels"
        )
        assert re.search(
            log_layout.message_then_fields_pattern(re.escape(PANEL_ROW_TOKEN), r'\s+slot=\d+\s+id="shell\.demo"\s+.*?alive=true'),
            restored,
        ), "the restarted editor did not register the panel: {}".format(
            restored[-800:]
        )
        print_ok("the module tab and its panel survived a restart")
        begin = panels_restart.length()
        panels_restart.send("quit")
        panels_restart.wait(re.escape(QUIT_REQUESTED_TOKEN), 30, "the restart's quit", begin)
        assert panels_restart.wait_exit(timeout=60) == 0, "the restarted panel editor did not exit 0"
    finally:
        panels_restart.close()

    # Safe mode: the editor starts against a shell whose source does
    # not compile: it shows the build errors with their `file:line` and a
    # Retry, and once the source is fixed Retry loads the shell - all without
    # restarting the process.
    shell_source = (
        REPOSITORY_ROOT / "modules" / "editor" / "pill_editor_shell" / "src" / "shell.rs"
    )
    shell_text = shell_source.read_bytes().decode("utf-8")
    safe = EditorSession(
        "egui", "safe", project=project, test_seconds=300, binary=binary
    )
    session.edit_source(
        shell_source, shell_text + '\ncompile_error!("egui suite: broken shell");\n'
    )
    # A staged DLL from the good build would load over the broken source and
    # there would be no safe mode to test: remove it so the failed build is the
    # only generation the editor can find. The retry below rebuilds it.
    staged_shell = MODULES_ROOT / "target" / "hot" / "pill_editor_shell.dll"
    staged_shell.unlink(missing_ok=True)
    try:
        safe.start(extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"})
        try:
            safe.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 120, "safe-mode startup"
            )
            # The panel is drawn once, and the host's build failing can make it
            # appear before this suite reads `window created` - so the search
            # starts at the log's beginning rather than at a later anchor.
            safe.wait(re.escape(SAFE_MODE_TOKEN), 240, "the safe-mode panel")
            rows_begin = safe.length()
            rows = safe.request(
                "dump problems",
                re.escape(PROBLEMS_DUMP_TOKEN),
                60,
                "the safe-mode problems",
            )
            assert re.search(r"count=[1-9][0-9]*", rows), (
                "safe mode reported no build error: {}".format(rows)
            )
            located = re.search(
                log_layout.message_then_fields_pattern(re.escape(PROBLEM_ROW_TOKEN), r'\s+module="[^"]*"\s+severity="error"\s+file="([^"]+)"\s+line=([1-9][0-9]*)'),
                safe.read_since(rows_begin),
            )
            assert located and located.group(1).endswith("shell.rs"), (
                "the safe-mode panel did not name the shell's source: {}".format(located)
            )
            print_ok("safe mode showed the shell's error with its location")
            # Fix the source, then press the panel's Retry through the same
            # path the button calls.
            session.edit_source(shell_source, shell_text)
            retry_begin = safe.length()
            safe.send("safe_mode retry")
            safe.wait(
                re.escape(SAFE_MODE_RETRY_TOKEN), 60, "the safe-mode retry", retry_begin
            )
            safe.wait(
                re.escape(EGUI_SHELL_LOADED_TOKEN),
                480,
                "the shell loaded after the retry",
                retry_begin,
            )
            print_ok("Retry loaded the fixed shell without a restart")

            # The retried shell and the renderer now share one process, and the
            # renderer's `rendering` system lives under a reserved owner: a shell
            # generation that took that owner (an owner derived from the loaded
            # extension count aliased the pending shell's) would clear the
            # renderer's system when its own reload retired its generation, and
            # the scene would stop drawing with nothing in the log to say why.
            def rendering_systems() -> list:
                """The systems dump's `rendering` rows."""
                dump = safe.request(
                    "dump systems",
                    re.escape(SYSTEMS_DUMP_TOKEN),
                    60,
                    "the systems after the retry",
                )
                return [
                    match.group(0)
                    for match in re.finditer(SYSTEM_ROW_PATTERN, dump)
                    if 'name="rendering"' in match.group(0)
                ]

            after_retry = rendering_systems()
            assert len(after_retry) == 1, (
                "the renderer registers exactly one `rendering` system: {}".format(
                    after_retry
                )
            )
            assert 'owner="renderer"' in after_retry[0], (
                "the rendering system is not under the reserved renderer owner: "
                + after_retry[0]
            )
            print_ok("the retried shell and the renderer hold separate owners")

            # A shell edit retires the shell's generation. The renderer's system
            # is not that generation's to clear, so it must still be there
            # afterwards, and the editor must keep drawing frames.
            session.edit_source(
                shell_source,
                shell_text + "\n// The safe-mode lane reloads the shell once.\n",
            )
            reload_begin = safe.length()
            safe.wait(
                re.escape(MODULE_RELOAD_COMPLETE_TOKEN),
                600,
                "the shell reloaded after the edit",
                reload_begin,
            )
            after_reload = rendering_systems()
            assert len(after_reload) == 1, (
                "a shell reload cleared or duplicated the renderer's system: "
                "{}".format(after_reload)
            )
            assert 'owner="renderer"' in after_reload[0], after_reload[0]
            safe.wait(
                re.escape(FRAME_HEARTBEAT_TOKEN),
                120,
                "the frame kept running after the shell reload",
                reload_begin,
            )
            print_ok("a shell reload left the renderer's system and the frame alone")
        finally:
            safe.close()
    finally:
        # A failed step must not leave a compile error behind for the next run.
        session.edit_source(shell_source, shell_text)
