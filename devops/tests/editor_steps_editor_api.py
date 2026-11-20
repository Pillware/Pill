"""The editor suite's editor API area: the proving crate's events end to end.

One session of its own on the `editor_api` test project, because the crate
under test is an extension's `_editor` part: it is registered by the editor
host, not present in the other projects every area shares. The area drives the
scripted channel the way a person drives the editor and reads the module's own
`[test] hook` lines, so a callback that never ran and a callback that ran
twice are both visible.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    DELETE_ENTITY_TOKEN,
    EDITOR_WINDOW_CREATED_TOKEN,
    EGUI_SHELL_LOADED_TOKEN,
    ENTITIES_DUMP_TOKEN,
    ENTITY_ROW_TOKEN,
    HOOK_ASSET_TOKEN,
    HOOK_COMMAND_ADD_COMPONENT_TOKEN,
    HOOK_COMMAND_SELECTION_TOKEN,
    HOOK_COMPONENT_TOKEN,
    HOOK_ENTITY_TOKEN,
    HOOK_EXECUTABLE_REGISTERED_TOKEN,
    HOOK_FIELD_AFTER_TOKEN,
    HOOK_FIELD_BEFORE_TOKEN,
    HOOK_FOLLOW_UP_TOKEN,
    HOOK_HISTORY_TOKEN,
    HOOK_PLAY_TOKEN,
    HOOK_REFUSED_TOKEN,
    HOOK_REGISTERED_TOKEN,
    HOOK_SCENE_TOKEN,
    HOOK_SELECTION_TOKEN,
    HOOK_TOOL_TOKEN,
    PROJECT_EDITOR_CASCADE_TOKEN,
    PROJECT_EDITOR_LOADED_TOKEN,
    PROJECT_EDITOR_RELOADED_TOKEN,
    PROJECT_HOOK_TOKEN,
    PROJECT_RELOAD_TOKEN,
)
from core.editor_session import EditorSession
from core.projects import REPOSITORY_ROOT
from editor_step_areas import StepArea, StepContext
from editor_step_support import print_ok

# The project this area runs, beside the other test projects.
API_PROJECT = REPOSITORY_ROOT / "devops" / "tests" / "projects" / "editor_api"
API_PROJECT_LIB = API_PROJECT / "src" / "lib.rs"

# The crate the reload step edits: an edit to it is what re-runs `register`,
# so the step proves the callbacks were rebound rather than replaced.
API_EDITOR_CRATE_SOURCE = (
    REPOSITORY_ROOT
    / "modules"
    / "extensions"
    / "test"
    / "modules"
    / "editor"
    / "src"
    / "pill_test_editor.rs"
)

# The component the proving crate's callbacks edit.
PROBE_COMPONENT = "ModuleTest"

# How many callbacks the crate registers.
CALLBACK_COUNT = 11


# The event names the executable reports for this crate's registrations.
EXECUTABLE_REGISTERED = re.compile(
    r'\[editor\] hook registered \|[^\n]*source="pill_test_editor"[^\n]*event="([a-z-]+)"'
)


def entity_handles(session: EditorSession) -> "dict":
    """The `display -> components` rows of one entity dump."""
    start = session.length()
    session.send("dump entities")
    session.wait(re.escape(ENTITIES_DUMP_TOKEN), 60, "the entity list", start)
    rows = re.findall(
        log_layout.message_then_fields_pattern(
            re.escape(ENTITY_ROW_TOKEN), r"\s+display=(\S+)\s+components=(.*)"
        ),
        session.read_since(start),
    )
    return {display.strip('"'): components.strip().strip('"') for display, components in rows}


def probe_entity(session: EditorSession) -> str:
    """The first entity carrying the probe component."""
    for display, components in entity_handles(session).items():
        if PROBE_COMPONENT in components:
            return display
    raise SystemExit("no entity carries {}: {}".format(PROBE_COMPONENT, entity_handles(session)))


def field_value(session: EditorSession, entity: str, field: str) -> str:
    """One component field's dumped value."""
    start = session.length()
    session.send("dump field {} {} {}".format(entity, PROBE_COMPONENT, field))
    session.wait(r"\[test\] field[^\n]*", 60, "the field dump", start)
    match = re.search(r"value=(\S+)", session.read_since(start))
    return match.group(1) if match else ""


def run_editor_api_steps(context: StepContext) -> None:
    """Drives the proving crate's callbacks through one editor session."""
    session = EditorSession(
        "editor_api",
        "main",
        project=str(API_PROJECT),
        test_seconds=420,
        binary=context.binary,
    )
    session.start(extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"})
    try:
        _drive_api(session)
    finally:
        session.close()


def _drive_api(session: EditorSession) -> None:
    """The steps themselves, run while the session is alive."""
    # Step 1: the crate registers one callback per event.
    session.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 180, "api startup")
    session.wait_for_match(re.escape(EGUI_SHELL_LOADED_TOKEN), 180, "api shell load")
    session.wait_for_match(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_REGISTERED_TOKEN),
            r'\s+source="?pill_test_editor"?\s+callbacks={}'.format(CALLBACK_COUNT),
        ),
        180,
        "the crate's registrations",
    )
    # The module queues its requests in `register`; the executable binds them
    # at its frame boundary, so the binding lines arrive a frame later.
    def every_event_bound(_text: str):
        found = set(EXECUTABLE_REGISTERED.findall(session.read_since(0)))
        return found if len(found) == CALLBACK_COUNT else None

    session.wait_until(
        every_event_bound,
        60,
        "the executable never bound all {} callbacks".format(CALLBACK_COUNT),
    )
    print_ok("the crate registered {} callbacks and the executable bound them".format(CALLBACK_COUNT))

    # Step 2: a selection fires the selection callback.
    entity = probe_entity(session)
    begin = session.length()
    session.send(f"select entity {entity}")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_SELECTION_TOKEN), r"\s+previous=\d+\s+current=(\d+)"
        ),
        30,
        "the selection callback",
        begin,
    )
    print_ok("the selection callback heard the pick")

    # Step 3: a committed field edit reports the phase, the primary flag and
    # the state the callback read.
    begin = session.length()
    session.send(f"edit set {entity} {PROBE_COMPONENT} accumulated_time 3.5")
    session.wait(re.escape(HOOK_FIELD_BEFORE_TOKEN), 30, "the before callback", begin)
    session.wait(re.escape(HOOK_FIELD_AFTER_TOKEN), 30, "the after callback", begin)
    before_line = session.read_since(begin)
    match = re.search(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_FIELD_BEFORE_TOKEN),
            r'[^\n]*field="?(\w+)"?[^\n]*phase="?(\w+)"?\s+primary=(\w+)',
        ),
        before_line,
    )
    assert match, "the before line carried no phase: {}".format(before_line[-400:])
    assert match.group(1) == "accumulated_time", "the callback named the field: {}".format(match.group(1))
    assert match.group(2) == "committed", "the phase the callback read: {}".format(match.group(2))
    assert match.group(3) == "true", "the edit's entity is the primary: {}".format(match.group(3))
    assert "view=0" in before_line, "the callback read the active view: {}".format(before_line[-400:])
    assert "selection=1" in before_line, "the callback read the selection: {}".format(
        before_line[-400:]
    )
    print_ok("the field callbacks report the phase, the primary flag and the state they read")

    # Step 4: the magic clamp value applies as its replacement.
    begin = session.length()
    session.send(f"edit set {entity} {PROBE_COMPONENT} accumulated_time 999")
    session.wait(re.escape(HOOK_FIELD_AFTER_TOKEN), 30, "the clamped edit", begin)
    clamped = field_value(session, entity, "accumulated_time")
    assert "1.0" in clamped, "the replacement applied: {}".format(clamped)
    print_ok("a before callback replaced the magic value")

    # Step 5: the magic refuse value is dropped with a notice.
    begin = session.length()
    session.send(f"edit set {entity} {PROBE_COMPONENT} accumulated_time -1")
    session.wait(re.escape(HOOK_REFUSED_TOKEN), 30, "the refusal", begin)
    refused = field_value(session, entity, "accumulated_time")
    assert "1.0" in refused, "a refused edit wrote nothing: {}".format(refused)
    print_ok("a before callback refused the magic value")

    # Step 6: one create, so the next field write cannot coalesce with the
    # writes above it - the shell merges same-labelled edits that arrive back
    # to back into one step, and a step of its own has to be the whole step.
    begin = session.length()
    session.send("command editor.entity_create")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_ENTITY_TOKEN), r"[^\n]*created=true"
        ),
        30,
        "the created entity",
        begin,
    )
    bare = _bare_entity(session)

    # Step 7: the follow-up write joins the step, so one undo reverts both.
    begin = session.length()
    session.send(f"edit set {entity} {PROBE_COMPONENT} accumulated_time 5")
    session.wait(re.escape(HOOK_FOLLOW_UP_TOKEN), 30, "the follow-up", begin)
    followed = field_value(session, entity, "processed_frame_count")
    assert "42" in followed, "the follow-up wrote its value: {}".format(followed)
    begin = session.length()
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_HISTORY_TOKEN), r'[^\n]*direction="?(\w+)"?'
        ),
        30,
        "the undo callback",
        begin,
    )
    undone = field_value(session, entity, "processed_frame_count")
    assert "42" not in undone, "one undo reverted the follow-up with the edit: {}".format(undone)
    print_ok("one undo reverted the edit and the follow-up its callback queued")

    # Step 8: the component builder attaches the component a selected entity
    # lacks, as one recorded step.
    begin = session.length()
    session.send(f"select entity {bare}")
    session.wait(re.escape(HOOK_COMMAND_ADD_COMPONENT_TOKEN), 30, "the add-component builder", begin)
    session.wait(re.escape(HOOK_COMPONENT_TOKEN), 30, "the component callback", begin)
    print_ok("a selection callback attached a component through the builder")

    # Step 9: removing the component reaches the callback; the entity it
    # belonged to is left bare again.
    begin = session.length()
    session.send(f"edit remove {bare} {PROBE_COMPONENT}")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_COMPONENT_TOKEN), r"[^\n]*added=false"
        ),
        30,
        "the remove callback",
        begin,
    )
    print_ok("the component callback heard the remove")

    # Step 9: an asset command reaches the asset callback.
    begin = session.length()
    session.send("scene save res/api_probe.scene")
    session.wait(r"\[editor\] scene saved[^\n]*", 60, "the scene saved", begin)
    begin = session.length()
    session.send("edit asset delete res/api_probe.scene")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_ASSET_TOKEN), r'[^\n]*kind="?deleted"?'
        ),
        30,
        "the asset callback",
        begin,
    )
    print_ok("the asset callback heard the delete")

    # Step 10: play, stop and a tool switch reach their callbacks.
    for command, token, pattern, label in (
        ("command editor.play", HOOK_PLAY_TOKEN, r"[^\n]*playing=true", "the play callback"),
        ("command editor.stop", HOOK_PLAY_TOKEN, r"[^\n]*playing=false", "the stop callback"),
        (
            "command editor.tool_rotate",
            HOOK_TOOL_TOKEN,
            r'[^\n]*current="?rotate"?',
            "the tool callback",
        ),
    ):
        begin = session.length()
        session.send(command)
        session.wait(
            log_layout.message_then_fields_pattern(re.escape(token), pattern),
            30,
            label,
            begin,
        )
    print_ok("the play, stop and tool callbacks heard their events")

    # Step 11: a project reload rebinds every callback without doubling one.
    _reload_rebinds(session)

    # Step 12: the project's own editor crate loads, hears an edit and reloads
    # with its project.
    _project_editor_crate(session)

    # Step 13: a new scene reaches the scene callback, and leaves the area
    # last because it clears the world.
    begin = session.length()
    session.send("scene new")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_SCENE_TOKEN), r"[^\n]*loaded=false"
        ),
        60,
        "the scene callback",
        begin,
    )
    print_ok("the scene callback heard the new scene")


def _bare_entity(session: EditorSession) -> str:
    """The entity the create step made: the one without the probe component."""
    for display, components in entity_handles(session).items():
        if PROBE_COMPONENT not in components:
            return display
    raise SystemExit("the created entity carries {} already".format(PROBE_COMPONENT))


def _reload_rebinds(session: EditorSession) -> None:
    """Edits the registering crate, then proves one event fires once."""
    with API_EDITOR_CRATE_SOURCE.open("r", encoding="utf-8", newline="") as file:
        original = file.read()
    start = session.length()
    session.edit_source(
        API_EDITOR_CRATE_SOURCE, original + "\n// the editor api area's reload\n"
    )
    session.wait(re.escape(PROJECT_RELOAD_TOKEN), 240, "the module reload", start)
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(HOOK_REGISTERED_TOKEN), r"[^\n]*callbacks={}".format(CALLBACK_COUNT)
        ),
        240,
        "the reloaded registrations",
        start,
    )
    restart = session.read_since(start)
    rebinds = len(EXECUTABLE_REGISTERED.findall(restart))
    assert rebinds == CALLBACK_COUNT, (
        "the reload bound {} callbacks, not {}".format(rebinds, CALLBACK_COUNT)
    )

    # One edit after the reload fires each callback once: two lines would mean
    # the old image is still registered.
    entity = probe_entity(session)
    begin = session.length()
    session.send(f"edit set {entity} {PROBE_COMPONENT} accumulated_time 2.5")
    session.wait(re.escape(HOOK_FIELD_AFTER_TOKEN), 30, "the edit after the reload", begin)
    afters = len(re.findall(re.escape(HOOK_FIELD_AFTER_TOKEN), session.read_since(begin)))
    assert afters == 1, "the edit reached {} callbacks, not one".format(afters)
    print_ok("the reload rebound every callback and none fired twice")


def _project_editor_crate(session: EditorSession) -> None:
    """The project's own editor crate loads, hears an edit and reloads with
    its project."""
    session.wait_for_match(re.escape(PROJECT_EDITOR_LOADED_TOKEN), 60, "the project editor load")
    entity = probe_entity(session)
    begin = session.length()
    session.send(f"edit set {entity} {PROBE_COMPONENT} accumulated_time 2.5")
    session.wait(re.escape(PROJECT_HOOK_TOKEN), 30, "the project editor callback", begin)
    print_ok("the project's own editor crate hears the editor")

    # A project reload reloads the crate, and one edit then fires it once: two
    # lines would mean the previous image is still registered.
    with API_PROJECT_LIB.open("r", encoding="utf-8", newline="") as file:
        original = file.read()
    begin = session.length()
    session.edit_source(API_PROJECT_LIB, original + "\n// the editor api area's project reload\n")
    session.wait(
        log_layout.message_then_fields_pattern(
            re.escape(PROJECT_RELOAD_TOKEN), r'[^\n]*module="editor_api"'
        ),
        300,
        "the project reload",
        begin,
    )
    session.wait(re.escape(PROJECT_EDITOR_CASCADE_TOKEN), 120, "the queued cascade", begin)
    session.wait(re.escape(PROJECT_EDITOR_RELOADED_TOKEN), 240, "the crate's reload", begin)
    entity = probe_entity(session)
    begin = session.length()
    session.send(f"edit set {entity} {PROBE_COMPONENT} accumulated_time 3.5")
    session.wait(re.escape(PROJECT_HOOK_TOKEN), 30, "the callback after the project reload", begin)
    fired = len(re.findall(re.escape(PROJECT_HOOK_TOKEN), session.read_since(begin)))
    assert fired == 1, "the project editor crate fired {} times after the reload".format(fired)
    print_ok("a project reload reloaded its editor crate and none fired twice")


def _area_entry(context: StepContext) -> None:
    """Runs this area against the run's shared state."""
    run_editor_api_steps(context)


AREA = StepArea(
    name="editor_api",
    order=17.5,
    run=_area_entry,
    needs=(),
    provides=(),
)
