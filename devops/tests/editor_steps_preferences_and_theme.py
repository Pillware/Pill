"""The editor suite's preferences and theme steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re

import yaml

from core.projects import THEMED_PROJECT_FONT
from core import log_layout
from core.editor_log_tokens import (
    EDITOR_THEME_APPLIED_TOKEN,
    EDITOR_THEME_LEGACY_TOKEN,
    EDITOR_THEME_REFUSED_TOKEN,
    EDITOR_THEME_RESOLVED_TOKEN,
    EGUI_ROWS_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    GIZMO_ANCHOR_TOKEN,
    GIZMO_DRAG_ENDED_TOKEN,
    GIZMO_DRAG_STARTED_TOKEN,
    GIZMO_PREFERENCES_WRITTEN_TOKEN,
    HISTORY_TRIMMED_TOKEN,
    PREFERENCES_APPLIED_TOKEN,
    PREFERENCES_DUMP_TOKEN,
    PREFERENCES_SAVED_TOKEN,
    PREFERENCE_EDIT_QUEUED_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    assert_theme,
    dump_theme,
    occurrence_count,
    wait_applied,
    wait_for_entity_count,
    write_editor_theme,
    write_theme_file,
)


def run_preferences_and_theme_steps(state: StepState) -> None:
    """The steps through: the theme layers merge in order, live, and refuse individually."""
    session = state.session

    wait_applied(session, state.applied_before, "delete undo applied")
    wait_for_entity_count(session, state.count_before + 1, "the undo restored the entity")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), r'[^\n]*detail="(?!-)[^"]+"'),
        60,
        "the selection came back with the restore",
        state.begin,
    )
    print_ok("entity create, delete and undo round-trip")

    # Step 4d: the Preferences tab - the file's rows through
    # the scripted channel, the live halves the reader applies (the
    # history limits and the snapping steps), and the per-project gizmo
    # entry the tool choices persist.
    preferences_path = session.sandbox / "RustHybridEcs" / "preferences.yaml"

    def wait_preference_value(key: str, expected: str, label: str) -> None:
        """Waits until the preferences document's dotted `key` shows `expected`."""

        def probe(_text: str):
            if not preferences_path.is_file():
                return None
            current = yaml.safe_load(preferences_path.read_text(encoding="utf-8"))
            for segment in key.split("."):
                current = current.get(segment) if isinstance(current, dict) else None
                if current is None:
                    break
            return True if current is not None and str(current) == expected else None

        session.wait_until(
            probe,
            30,
            f"{label}: {key} is not {expected!r} in {preferences_path}\n",
            label=label,
            # Poll until the save lands on disk.
            poll=0.2,
        )

    # The seed proves the startup read: the live reader applied its cap
    # and the tab's view carries its rows.
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PREFERENCES_APPLIED_TOKEN), r"[^\n]*step_cap=400\s"),
        30,
        "the seeded step cap applied",
    )
    begin = session.length()
    session.send("dump preferences")
    session.wait(re.escape(PREFERENCES_DUMP_TOKEN), 30, "preferences dumped", begin)
    dump = session.read_since(begin)
    assert re.search(r'snapping_translation="0\.25"', dump), dump[-600:]
    assert re.search(r'history_step_cap="400"', dump), dump[-600:]
    assert re.search(r'project="scene"', dump), dump[-600:]
    print_ok("the seeded preferences reached the tab's view")

    # One row through the scripted save: the file, the tab's view and
    # the live reader all follow.
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("edit preference snapping_steps.rotation_degrees 7.5")
    session.wait(
        re.escape(PREFERENCE_EDIT_QUEUED_TOKEN), 30, "the preference edit queued", begin
    )
    session.wait(re.escape(PREFERENCES_SAVED_TOKEN), 30, "the preferences saved", begin)
    wait_applied(session, applied_before, "the preference row applied")
    wait_preference_value(
        "snapping_steps.rotation_degrees", "7.5", "the file carries the saved row"
    )
    session.wait(
        re.escape(PREFERENCES_APPLIED_TOKEN), 30, "the live reader reapplied", begin
    )
    begin = session.length()
    session.send("dump preferences")
    session.wait(re.escape(PREFERENCES_DUMP_TOKEN), 30, "preferences dumped again", begin)
    assert re.search(
        r'snapping_rotation_degrees="7\.5"', session.read_since(begin)
    ), session.read_since(begin)[-600:]
    print_ok("a saved row reaches the file, the view and the reader")

    # The project's remembered entry persists with a tool change; the
    # saved translate step then steers the gizmo, so a snapped drag
    # moves by a multiple of it.
    begin = session.length()
    session.send("command editor.tool_translate")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_PREFERENCES_WRITTEN_TOKEN), r'\s+project="scene"'),
        30,
        "the tool choice persisted",
        begin,
    )
    wait_preference_value(
        "gizmo.projects.scene.mode",
        "translate",
        "the project entry carries the tool choice",
    )
    begin = session.length()
    session.send("edit preference snapping_steps.translation 0.3")
    session.wait(re.escape(PREFERENCES_SAVED_TOKEN), 30, "the translate step saved", begin)
    wait_preference_value(
        "snapping_steps.translation", "0.3", "the file carries the translate step"
    )
    session.wait(
        re.escape(PREFERENCES_APPLIED_TOKEN), 30, "the reader reapplied the step", begin
    )
    begin = session.length()
    session.send("command editor.tool_snap_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), r"[^\n]*snapped=true"),
        60,
        "snapping switched on",
        begin,
    )
    session.wait(
        re.escape(GIZMO_PREFERENCES_WRITTEN_TOKEN), 30, "the snap toggle persisted", begin
    )
    begin = session.length()
    session.send("gizmo drag x 30 0")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_STARTED_TOKEN), r'[^\n]*mode="translate"\s+axis="x"'),
        60,
        "the snapped drag started",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_ENDED_TOKEN), r'[^\n]*mode="translate"\s+axis="x"[^\n]*value=\[([-\d.eE+]+),'),
        60,
        "the snapped drag ended",
        begin,
    )
    match = re.search(r"value=\[([-\d.eE+]+),", session.read_since(begin))
    snapped_delta = float(match.group(1))
    ratio = snapped_delta / 0.3
    assert abs(ratio - round(ratio)) < 0.01, (
        f"the snapped drag moved {snapped_delta}, not a multiple of 0.3"
    )
    print_ok(f"the snapped drag moved x by {snapped_delta} (a multiple of 0.3)")
    begin = session.length()
    session.send("command editor.tool_snap_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), r"[^\n]*snapped=false"),
        60,
        "snapping switched off",
        begin,
    )

    # The history limits are the file's other live half: a tiny cap
    # trims at once, and the restored cap stops the trimming - the
    # later steps' undo needs a working history.
    begin = session.length()
    session.send("edit preference history_step_cap 2")
    session.wait(re.escape(PREFERENCES_SAVED_TOKEN), 30, "the tiny cap saved", begin)
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PREFERENCES_APPLIED_TOKEN), r"[^\n]*step_cap=2\s"),
        30,
        "the tiny cap applied",
        begin,
    )
    session.wait(re.escape(HISTORY_TRIMMED_TOKEN), 30, "the history trimmed", begin)
    begin = session.length()
    session.send("edit preference history_step_cap 500")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PREFERENCES_APPLIED_TOKEN), r"[^\n]*step_cap=500\s"),
        30,
        "the cap restored",
        begin,
    )
    print_ok("the history limits applied and trimmed live")

    # The project section: a tool choice persists under the project's
    # name, in the file and in the tab's view.
    begin = session.length()
    session.send("command editor.tool_rotate")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_PREFERENCES_WRITTEN_TOKEN), r'\s+project="scene"'),
        30,
        "the second tool choice persisted",
        begin,
    )
    wait_preference_value(
        "gizmo.projects.scene.mode", "rotate", "the project entry followed"
    )
    wait_preference_value("gizmo.mode", "rotate", "the last-used mode followed")
    begin = session.length()
    session.send("dump preferences")
    session.wait(re.escape(PREFERENCES_DUMP_TOKEN), 30, "preferences dumped once more", begin)
    assert re.search(
        r"remembered=true[^\n]*mode=Some\(Rotate\)", session.read_since(begin)
    ), session.read_since(begin)[-600:]
    print_ok("the project's remembered tool round-trips")

    # Step 4d2: the editor's theme layers. Three layers
    # sit over the code default - the user's file, named by the `theme`
    # preference and read from the state directory, and the project's file
    # under `res/editor/`, named by the settings' `editor.theme` - and the
    # project wins where both name the same item. Every step goes through
    # the files a person would edit; `dump theme` reports the merge the
    # shell installs.

    # 1. The user's file: an accent and a panel fill over the code default.
    user_theme_path = session.sandbox / "RustHybridEcs" / "theme_user.theme"
    user_theme_path.write_text(
        yaml.safe_dump(
            {"palette": {"accent": "#ff488c", "panel": "#101820"}},
            sort_keys=False,
            default_flow_style=False,
        ),
        encoding="utf-8",
    )
    begin = session.length()
    session.send("edit preference theme theme_user.theme")
    session.wait(re.escape(PREFERENCES_SAVED_TOKEN), 30, "the theme row saved", begin)
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_THEME_RESOLVED_TOKEN), r"\s+layers=code,user\s+generation=\d+"),
        60,
        "the user layer merged",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_THEME_APPLIED_TOKEN), r"\s+layers=code,user\s+user=\S*theme_user\.theme"),
        60,
        "the shell installed the user layer",
        begin,
    )
    user_generation = assert_theme(
        dump_theme(session, "the user layer dumped"),
        "code,user",
        "#ff488c",
        "#101820",
        "the user file's accent and panel fill applied",
    )

    # 2. The project's own file names only the accent: it wins over the
    # user's accent and leaves the user's panel fill alone.
    write_theme_file(
        state.theme_root / "editor.theme",
        "000000000000000000000000000000b1",
        {"palette": {"accent": "#33d17a"}},
    )
    begin = session.length()
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_THEME_RESOLVED_TOKEN), r"\s+layers=code,user,project\s+generation=\d+"),
        60,
        "the project layer merged",
        begin,
    )
    project_generation = assert_theme(
        dump_theme(session, "the project layer dumped"),
        "code,user,project",
        "#33d17a",
        "#101820",
        "the project's accent won and the user's panel fill stayed",
    )
    assert project_generation > user_generation, (
        "the project layer did not raise the generation: {}".format(
            (user_generation, project_generation)
        )
    )

    # 3. Editing the project's file reaches the editor: the store
    # reimports it and the merge runs again.
    write_theme_file(
        state.theme_root / "editor.theme",
        "000000000000000000000000000000b1",
        {"palette": {"accent": "#0aa1ff"}},
    )
    begin = session.length()
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_THEME_RESOLVED_TOKEN), r"\s+layers=code,user,project\s+generation=\d+"),
        60,
        "the edited project layer merged",
        begin,
    )
    edited_generation = assert_theme(
        dump_theme(session, "the edited project layer dumped"),
        "code,user,project",
        "#0aa1ff",
        "#101820",
        "an edited project theme re-applied",
    )
    assert edited_generation > project_generation, (
        "the edited project file did not raise the generation: {}".format(
            (project_generation, edited_generation)
        )
    )

    # 4. The settings' `editor.theme` points the project layer at another file
    # without a restart.
    write_theme_file(
        state.theme_root / "alt.theme",
        "000000000000000000000000000000b2",
        {"palette": {"accent": "#d17a33"}},
    )
    write_editor_theme(state.project_settings_path, "editor/alt.theme")
    begin = session.length()
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_THEME_RESOLVED_TOKEN), r"\s+layers=code,user,project\s+generation=\d+"),
        60,
        "the named project layer merged",
        begin,
    )
    switched = dump_theme(session, "the named project layer dumped")
    assert "project=editor/alt.theme" in switched, switched
    switched_generation = assert_theme(
        switched,
        "code,user,project",
        "#d17a33",
        "#101820",
        "the settings file switched the project layer live",
    )
    assert switched_generation > edited_generation, (
        "the switched layer did not raise the generation: {}".format(
            (edited_generation, switched_generation)
        )
    )

    # 5. A path outside `editor/` is refused with the rule, and the
    # remaining layers still apply.
    write_editor_theme(state.project_settings_path, "ui/x.theme")
    begin = session.length()
    session.wait(
        re.escape(EDITOR_THEME_REFUSED_TOKEN)
        + r"project: [^\n]*must be under `editor/`",
        60,
        "the out-of-scope theme refused with the rule",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_THEME_RESOLVED_TOKEN), r"\s+layers=code,user\s+generation=\d+"),
        60,
        "the refused project layer skipped",
        begin,
    )
    refused = dump_theme(session, "the refused project layer dumped")
    assert "project=none" in refused, refused
    assert_theme(
        refused,
        "code,user",
        "#ff488c",
        "#101820",
        "a refused project layer left the user's layer in force",
    )

    # 6. A name an earlier build understood names no file: it is reported
    # once, and the project layer still applies.
    write_editor_theme(state.project_settings_path, None)
    begin = session.length()
    session.send("edit preference theme dark")
    session.wait(
        re.escape(EDITOR_THEME_LEGACY_TOKEN)
        + r" 'dark' is not a file; no user layer",
        60,
        "the legacy theme name reported",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_THEME_RESOLVED_TOKEN), r"\s+layers=code,project\s+generation=\d+"),
        60,
        "the project layer alone applied",
        begin,
    )
    assert_theme(
        dump_theme(session, "the legacy name dumped"),
        "code,project",
        "#0aa1ff",
        "#1a1a1d",
        "a legacy theme name left the project layer alone",
    )

    # 7. A user file the merge refuses is named with its item, and the
    # project layer still applies: one broken file costs only itself.
    broken_path = session.sandbox / "RustHybridEcs" / "broken.theme"
    broken_path.write_text(
        "palette:\n  accent: not-a-colour\n", encoding="utf-8"
    )
    begin = session.length()
    session.send("edit preference theme broken.theme")
    session.wait(
        re.escape(EDITOR_THEME_REFUSED_TOKEN)
        + r"user: [^\n]*palette\.accent",
        60,
        "the broken user layer refused with its item",
        begin,
    )
    assert_theme(
        dump_theme(session, "the broken user layer dumped"),
        "code,project",
        "#0aa1ff",
        "#1a1a1d",
        "a refused user layer left the project's layer in force",
    )
    print_ok("the theme layers merge in order, live, and refuse individually")

    # 8. The project layer's font. A `.ttf` under
    # `res/editor/` loads through the project's asset store and installs
    # ahead of the editor's own families; a path that ships nothing is
    # refused by name and leaves that family to the built-ins.
    font_directory = state.theme_root / "fonts"
    font_directory.mkdir(parents=True, exist_ok=True)
    (font_directory / "Silkscreen-Regular.ttf").write_bytes(
        THEMED_PROJECT_FONT.read_bytes()
    )

    state.applied_before = applied_before
    state.begin = begin


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_preferences_and_theme_steps(context.state)


AREA = StepArea(
    name="preferences_and_theme",
    order=8,
    run=_area_entry,
    needs=("count_before",),
    provides=(),
)
