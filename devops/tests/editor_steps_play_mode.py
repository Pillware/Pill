"""The editor suite's play-mode steps: edit mode, Play, pause, step and Stop.

The steps read the vocabulary the engine, the editor core and the shell log:
`[engine] play entered|exited|refused`, `[editor] play toggled`, the toolbar's
rectangle, the tab focus the shell reports, and the `[test] play` dump.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    PAUSE_TOGGLED_TOKEN,
    PLAY_DUMP_TOKEN,
    PLAY_ENTERED_TOKEN,
    PLAY_EXITED_TOKEN,
    PLAY_FOCUSED_GAME_TAB_TOKEN,
    PLAY_RESTORED_TAB_TOKEN,
    PLAY_TOGGLED_TOKEN,
    PLAY_TOOLBAR_RECT_TOKEN,
    PROJECT_RELOAD_TOKEN,
    STEP_QUEUED_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    StepState,
    entity_count,
    print_ok,
    wait_for_entity_count,
)


def run_play_mode_steps(state: StepState) -> None:
    """The steps through: a session that leaves the edit world as it found it."""
    session = state.session

    # Step 1: the editor starts in edit mode, with the play toolbar drawn.
    # The toolbar reports its placement once, while the editor is still
    # starting, so this one wait reads the whole log rather than the shared
    # begin index, which the startup area moved past that line. The first start
    # of a cold tree also builds every module before the shell draws, so the
    # wait is generous.
    session.wait(
        re.escape(PLAY_TOOLBAR_RECT_TOKEN),
        120,
        "the play toolbar drew",
    )
    dump = play_dump(session, "the starting run mode")
    assert 'mode="editing"' in dump, "the editor did not start in edit mode: {}".format(dump)
    assert "restore_epoch=0" in dump, "nothing has been restored yet: {}".format(dump)
    print_ok("play mode: the editor starts in edit mode with the toolbar drawn")

    # Step 2: the world and the history before Play.
    entities_before = entity_count(session)
    undo_before = play_undo_depth(dump)

    # Step 3: Play. The engine copies the world, the shell reports the mode and
    # brings the Game tab forward.
    begin = session.length()
    session.send("command editor.play")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(PLAY_ENTERED_TOKEN), r"\s+entities=(\d+)\s+components=(\d+)"
        ),
        60,
        "the session entered",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(PLAY_TOGGLED_TOKEN), r"\s+mode=\"playing\""
        ),
        30,
        "the editor reported the playing mode",
        begin,
    )
    session.wait(
        re.escape(PLAY_FOCUSED_GAME_TAB_TOKEN),
        30,
        "the Game tab came forward",
        begin,
    )
    dump = play_dump(session, "the playing run mode")
    assert 'mode="playing"' in dump, "Play did not switch the run mode: {}".format(dump)
    print_ok("play mode: Play copies the world, switches the mode and focuses the Game tab")

    # Step 4: an edit made during the session. It must not survive Stop, and it
    # must not reach the document's history.
    session.send("command editor.entity_create")
    wait_for_entity_count(session, entities_before + 1, "the play-time entity")
    print_ok("play mode: an entity spawned during the session reaches the world")

    # Step 4b: a reload while the game runs. The reload walks both the live and
    # the held entity storage and re-homes the held copies, so the playing
    # world keeps its entities and the edit world is still there to come back
    # to. The source goes back to what it was straight after, which reloads
    # once more.
    # Read byte-faithfully: the session writes the text back as given, so a
    # translated read would rewrite every line ending.
    with state.project_source.open("r", encoding="utf-8", newline="") as project_file:
        project_text = project_file.read()
    begin = session.length()
    session.edit_source(
        state.project_source,
        project_text + "\n// The play-mode steps reload the project once.\n",
    )
    session.wait(
        re.escape(PROJECT_RELOAD_TOKEN),
        240,
        "the project reloaded during the session",
        begin,
    )
    wait_for_entity_count(
        session, entities_before + 1, "the playing world's entities after the reload"
    )
    begin = session.length()
    session.edit_source(state.project_source, project_text)
    session.wait(
        re.escape(PROJECT_RELOAD_TOKEN),
        240,
        "the project reloaded back",
        begin,
    )
    print_ok("play mode: a reload during the session leaves the playing world and the held edit world alone")

    # Step 5: Stop. The engine puts the edit world back, the shell reports the
    # mode, restores the tab and puts the cameras back.
    begin = session.length()
    session.send("command editor.stop")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(PLAY_EXITED_TOKEN), r"\s+restore_epoch=1"
        ),
        60,
        "the session exited",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(PLAY_TOGGLED_TOKEN), r"\s+mode=\"editing\""
        ),
        30,
        "the editor reported the editing mode",
        begin,
    )
    session.wait(
        re.escape(PLAY_RESTORED_TAB_TOKEN),
        30,
        "the tab that was in front came back",
        begin,
    )
    dump = play_dump(session, "the restored run mode")
    assert 'mode="editing"' in dump, "Stop did not switch the run mode back: {}".format(dump)
    assert "restore_epoch=1" in dump, "Stop did not announce its restore: {}".format(dump)
    wait_for_entity_count(session, entities_before, "the edit world's entities")
    print_ok("play mode: Stop restores the world, the epoch and the tab")

    # Step 6: the history is the document's own. A session records into an
    # empty stack and drops it, so the depth is what it was.
    undo_after = play_undo_depth(dump)
    assert undo_after == undo_before, (
        "the session changed the undo depth: {} before, {} after".format(
            undo_before, undo_after
        )
    )
    print_ok("play mode: the session leaves the undo history as it found it")

    # Step 7: pause, resume and step keep working, and none of them records.
    begin = session.length()
    session.send("command editor.pause_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(PAUSE_TOGGLED_TOKEN), r"\s+paused=true"
        ),
        30,
        "the pause toggle reached the engine",
        begin,
    )
    session.send("command editor.pause_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(PAUSE_TOGGLED_TOKEN), r"\s+paused=false"
        ),
        30,
        "the resume toggle reached the engine",
        begin,
    )
    begin = session.length()
    session.send("command editor.step")
    session.wait(
        re.escape(STEP_QUEUED_TOKEN),
        30,
        "the step reached the engine",
        begin,
    )
    print_ok("play mode: pause, resume and step reach the engine")


def play_dump(session, label: str) -> str:
    """One `dump play` round-trip: the mode, the pause flag, the epoch and the
    history's depths."""
    begin = session.length()
    session.send("dump play")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(PLAY_DUMP_TOKEN),
            r"\s+mode=\"(\w+)\"\s+paused=(\w+)\s+restore_epoch=(\d+)\s+undo=(\d+)\s+redo=(\d+)",
        ),
        30,
        label,
        begin,
    )
    return session.read_since(begin)


def play_undo_depth(dump: str) -> int:
    """The undo depth one `dump play` reported."""
    match = re.search(r"\[test\] play .*undo=(\d+)", dump)
    assert match, "the play dump carried no undo depth: {}".format(dump[-400:])
    return int(match.group(1))


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_play_mode_steps(context.state)


AREA = StepArea(
    name="play_mode",
    order=10.5,
    run=_area_entry,
    needs=(),
    provides=(),
)
