"""The editor suite's project settings steps.

The Project Settings tab lists every declared setting with the value in effect
and the file it came from. These steps open it, read its own data through the
scripted `dump settings`, and drive the three writes a row offers - the project
file, the local override beside it, and a reset - checking the files on disk
after each one: the value moved, the file's own comments are still there, and a
reset puts the file back.

The area runs on the scene test project, which declares no module runtime
settings (`pill_test`, `pill_audio` and `pill_physics` do, and live in other
fixtures), so the rows it reads are the `host`, `engine` and `editor` ones. A
module's rows are the same view built from the schema a loaded module
registered, which `test_manager_settings.py` drives end to end.
"""

import re
from pathlib import Path

from core import log_layout
from core.editor_log_tokens import (
    EDITOR_WINDOW_CREATED_TOKEN,
    EGUI_SHELL_LOADED_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    TAB_ACTIVATED_TOKEN,
)
from core.editor_session import EditorSession
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    StepState,
    occurrence_count,
    print_ok,
    wait_applied,
)

# The setting these steps write and reset. It is a flag the project can carry
# without changing how the rest of the suite runs: the value these steps write
# is gone again by the reset below, and nothing between the two reads it.
SETTING_PATH = "editor.focus_game_on_play"

# The values the project file and the local override are given.
PROJECT_VALUE = "false"
LOCAL_VALUE = "true"

# The settings rows the tab dumps, one line per setting and one line for the
# dump's own header.
SETTING_ROW_TOKEN = "[test] setting"
SETTINGS_DUMP_TOKEN = "[test] settings"


def dump_settings(session, label: str) -> str:
    """Sends `dump settings` and returns the text it wrote."""
    before = occurrence_count(session, re.escape(SETTINGS_DUMP_TOKEN))
    begin = session.length()
    session.send("dump settings")
    session.wait_occurrence(re.escape(SETTINGS_DUMP_TOKEN), before + 1, 30, label)
    return session.read_since(begin)


def row_of(dumped: str, path: str) -> str:
    """Returns the dumped row for `path`."""
    pattern = re.compile(
        r"^.*"
        + re.escape(SETTING_ROW_TOKEN)
        + r'.*\bpath="'
        + re.escape(path)
        + r'".*$',
        re.MULTILINE,
    )
    match = pattern.search(dumped)
    assert match is not None, "no {} row in the dump:\n{}".format(path, dumped)
    return match.group(0)


def field_of(row: str, name: str) -> str:
    """Returns one quoted field of a dumped row."""
    match = re.search(r"\b" + re.escape(name) + r'="([^"]*)"', row)
    assert match is not None, "{} is missing from the row: {}".format(name, row)
    return match.group(1)


def run_project_settings_steps(context: "StepContext") -> None:
    """The steps through: the tab opens, a write keeps the file's comments, a
    local write lands in the override, and a reset takes the key back out.

    The area opens a session of its own: the run's shared one has already quit
    its editor by the time this area runs, and the first wait below carries this
    session's startup.
    """
    session = EditorSession(
        "project_settings",
        "main",
        project=context.project,
        test_seconds=420,
        binary=context.binary,
    )
    session.start(extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"})
    try:
        session.wait_for_match(
            re.escape(EDITOR_WINDOW_CREATED_TOKEN), 180, "settings startup"
        )
        session.wait_for_match(
            re.escape(EGUI_SHELL_LOADED_TOKEN), 180, "settings shell load"
        )
        _drive_settings(context.state, session)
    finally:
        session.close()


def _drive_settings(state: "StepState", session) -> None:
    """The tab's reads and writes, run while the area's own session is alive."""
    settings_path = Path(state.project_settings_path)
    local_path = settings_path.with_name("project_settings.local.yaml")
    original = settings_path.read_text(encoding="utf-8")
    assert SETTING_PATH not in original, (
        "the theme key has to be absent before this area writes it: {}".format(original)
    )
    try:
        # 1. The tab opens from its own name. The first wait of a standalone
        # area carries the editor's own startup, which takes about half a
        # minute on a cold build.
        begin = session.length()
        session.send("tab project_settings")
        session.wait(
            log_layout.message_then_fields_pattern(
                re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="project_settings"'
            ),
            90,
            "the Project Settings tab activated",
            begin,
        )
        print_ok("the Project Settings tab opens")

        # 2. It lists the declared settings with their source, default and
        # timing.
        row = row_of(dump_settings(session, "the first settings dump"), SETTING_PATH)
        assert field_of(row, "source") == "default", row
        assert field_of(row, "entry") == "editor", row
        assert field_of(row, "default") == field_of(row, "value"), (
            "an unset row shows its declared default: {}".format(row)
        )
        assert field_of(row, "timing") in ("live", "startup", "build"), row
        print_ok("the tab lists a setting with its default and its source")

        # 3. An edit writes the project file and keeps every comment.
        before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
        session.send(f"edit setting {SETTING_PATH} {PROJECT_VALUE}")
        wait_applied(session, before, "the settings write applied")
        written = settings_path.read_text(encoding="utf-8")
        assert "focus_game_on_play: false" in written, written
        assert written.startswith(original.rstrip("\n")), (
            "the writer rewrote bytes the file already had:\n{}".format(written)
        )
        row = row_of(dump_settings(session, "the written settings dump"), SETTING_PATH)
        assert field_of(row, "source") == "project", row
        assert field_of(row, "project") == PROJECT_VALUE, row
        print_ok("an edit lands in project_settings.yaml with its comments intact")

        # 4. A local edit writes only the override.
        before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
        session.send(f"edit setting {SETTING_PATH} {LOCAL_VALUE} local")
        wait_applied(session, before, "the local settings write applied")
        local = local_path.read_text(encoding="utf-8")
        assert "focus_game_on_play: true" in local, local
        assert "focus_game_on_play: false" in settings_path.read_text(encoding="utf-8"), (
            "a local write touched the project file"
        )
        row = row_of(dump_settings(session, "the local settings dump"), SETTING_PATH)
        assert field_of(row, "source") == "local", row
        assert field_of(row, "local") == LOCAL_VALUE, row
        print_ok("a local edit lands in project_settings.local.yaml only")

        # 5. A reset takes the key out of both files, and the groups it empties.
        before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
        session.send(f"reset setting {SETTING_PATH}")
        wait_applied(session, before, "the settings reset applied")
        cleared = settings_path.read_text(encoding="utf-8")
        assert "focus_game_on_play:" not in cleared, cleared
        assert cleared.rstrip("\n") == original.rstrip("\n"), (
            "the reset did not put the project file back:\n{}".format(cleared)
        )
        local_after = local_path.read_text(encoding="utf-8") if local_path.is_file() else ""
        assert "focus_game_on_play:" not in local_after, local_after
        row = row_of(dump_settings(session, "the reset settings dump"), SETTING_PATH)
        assert field_of(row, "source") == "default", row
        assert field_of(row, "project") == "-", row
        assert field_of(row, "local") == "-", row
        print_ok("a reset removes the key from whichever file held it")

        # 6. The view still names every entry the run declares.
        dumped = dump_settings(session, "the settled settings dump")
        for entry in ("host", "engine", "editor"):
            assert 'entry="{}"'.format(entry) in dumped, (
                "the dump lost the {} entry:\n{}".format(entry, dumped)
            )
        print_ok("the settings view names every declared entry")
    finally:
        # Leave the fixture as this area found it, whether the steps passed or
        # not: every later area runs against the same files.
        settings_path.write_text(original, encoding="utf-8")
        local_path.unlink(missing_ok=True)


AREA = StepArea(
    name="project_settings",
    order=18,
    run=run_project_settings_steps,
    needs=(),
    provides=(),
)
