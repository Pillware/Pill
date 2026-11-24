"""The editor suite's step support: the readers and waits every area shares.

`test_editor_egui.py` keeps the fixtures, the session and the run loop; each
`editor_steps_<area>.py` holds one area's step bodies. Everything both need -
the anchored readers, the small polls and the `StepState` the areas pass along -
lives here, so an area module imports its tools instead of the suite.
"""

import re
from pathlib import Path

import yaml

from core import log_layout
from core.editor_log_tokens import (
    CONSOLE_DUMP_PATTERN,
    EDITOR_THEME_DUMP_TOKEN,
    EGUI_ROWS_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    ENTITIES_DUMP_TOKEN,
    FIELD_TOKEN,
    INSPECTOR_ADD_RECT_PATTERN,
    INSPECTOR_ROW_PATTERN,
    INSPECTOR_SECTION_RECT_PATTERN,
    MENU_ACTION_TOKEN,
    MENU_ENTRY_RECT_PATTERN,
    MENU_OPENED_TOKEN,
    MENU_TITLE_RECT_PATTERN,
    REFERENCE_ROW_TOKEN,
    SCENE_BADGE_RECT_TOKEN,
    SCENE_RECT_PATTERN,
    TOOLBOX_CLICK_TOKEN,
    TOOLBOX_ITEM_PATTERN,
    TOOLBOX_RECT_PATTERN,
    TRANSFORM_COMPONENT,
    VIEWPORT_DUMP_TOKEN,
)
from core.editor_session import EditorSession, print_step_ok


def print_ok(label: str) -> None:
    """Prints a step's `[OK]` line and the milliseconds the step took.

    The clock is the session's, so a wait's own closing line and a step's
    explicit one share one timeline: the figure is the time since the previous
    `[OK]` of either kind.
    """
    print_step_ok(label)


def write_editor_theme(project_settings_path: Path, theme) -> None:
    """Sets or removes `editor.theme` in a project's settings file.

    `theme=None` removes the key, and the `editor:` entry with it when nothing
    else is left. Every other key is kept, so a step changes one value and the
    project keeps the module list it runs with.
    """
    data = yaml.safe_load(project_settings_path.read_text(encoding="utf-8")) or {}
    editor = data.get("editor")
    if not isinstance(editor, dict):
        editor = {}
    if theme is None:
        editor.pop("theme", None)
    else:
        editor["theme"] = theme
    if editor:
        data["editor"] = editor
    else:
        data.pop("editor", None)
    project_settings_path.write_text(
        yaml.safe_dump(data, sort_keys=False), encoding="utf-8"
    )


def read_budget_exceeded(session: EditorSession) -> bool:
    """Whether this run read far more log text than the log itself holds.

    The cursor reads every appended byte once, so a healthy run's count is the
    log's own length. A reader that polls by rereading the whole file multiplies
    it, which is the regression the budget watches for. The slack absorbs a
    truncated or replaced log, which is read again from the start.
    """
    return session.log_characters_read() > session.length() * 2 + 1_000_000


def occurrence_count(session: EditorSession, pattern: str) -> int:
    """How many times `pattern` appears in the log so far."""
    return len(re.findall(pattern, session.text()))


def newest_log_match(
    session: EditorSession, pattern: str, key_filters=(), start: int = 0, label: str = ""
) -> "re.Match":
    """The newest log match of `pattern` from `start`, optionally keyed.

    `key_filters` holds `(group, value)` pairs a match must equal; the
    rectangle readers differ only in which group names their key, and a
    rectangle can be logged again as the layout settles, so the latest one
    counts.
    """
    matches = [
        match
        for match in re.finditer(pattern, session.read_since(start))
        if all(match.group(group) == value for group, value in key_filters)
    ]
    if not matches:
        raise AssertionError(label or "no match for {}".format(pattern))
    return matches[-1]


def latest_log_match(session: EditorSession, pattern: str, view: int, label: str) -> "re.Match":
    """The newest log line matching `pattern` whose first group is `view`."""
    return newest_log_match(
        session,
        pattern,
        ((1, str(view)),),
        label="no {} for view {} in the log".format(label, view),
    )


def toolbox_panel(session: EditorSession, view: int) -> tuple:
    """The toolbox panel's last logged rectangle: (x, y, width, height)."""
    match = latest_log_match(session, TOOLBOX_RECT_PATTERN, view, "toolbox rect")
    return (
        float(match.group(2)),
        float(match.group(3)),
        float(match.group(4)),
        float(match.group(5)),
    )


def toolbox_row(session: EditorSession, view: int, item: str) -> tuple:
    """One toolbox row's last logged rectangle, looked up by its name."""
    last = newest_log_match(
        session,
        TOOLBOX_ITEM_PATTERN,
        ((1, str(view)), (2, item)),
        label=f"no toolbox row {item!r} for view {view} in the log",
    )
    return (
        float(last.group(3)),
        float(last.group(4)),
        float(last.group(5)),
        float(last.group(6)),
    )


def toolbox_click(session: EditorSession, view: int, item: str, label: str) -> None:
    """Clicks one toolbox row at its logged center and waits for its token.

    Like a menu item, a toolbox row's click is delivered on one frame and can
    be swallowed while the panel is moving or the pointer is settling, so a
    miss is retried once with the row's rectangle read again.
    """
    pattern = log_layout.message_then_fields_pattern(re.escape(TOOLBOX_CLICK_TOKEN), rf'\s+view={view}\s+item="{item}"')
    for attempt in (1, 2):
        x, y = rect_center(toolbox_row(session, view, item))
        begin = session.length()
        session.send(f"click {x:.0f} {y:.0f}")
        try:
            session.wait(pattern, 30, label, begin)
            return
        except TimeoutError:
            if attempt == 2:
                raise
            print("  [RETRY] the toolbox row {!r} click missed; retrying once".format(item))


def menu_title_rect(session: EditorSession, name: str) -> tuple:
    """One menu title's last logged rectangle, looked up by its wire name."""
    last = newest_log_match(
        session,
        MENU_TITLE_RECT_PATTERN,
        ((1, name),),
        label=f"no menu title {name!r} in the log",
    )
    return (
        float(last.group(2)),
        float(last.group(3)),
        float(last.group(4)),
        float(last.group(5)),
    )


def menu_click(session: EditorSession, menu: str, label: str, action_label: str) -> None:
    """Opens `menu`, clicks the `label` item and waits for its action's log line.

    Every menu is opened fresh: a click on an item closes its menu, so a
    recorded rectangle from an earlier opening would address a closed popup.

    A synthetic click is delivered on one frame, and a frame that is still
    laying the popup out swallows it: the click is retried once, with the
    rectangle read again, before the absence counts as a failure.
    """
    for attempt in (1, 2):
        center = rect_center(menu_title_rect(session, menu))
        session.request(
            f"click {center[0]:.0f} {center[1]:.0f}",
            log_layout.message_then_fields_pattern(re.escape(MENU_OPENED_TOKEN), rf'\s+menu="{menu}"'),
            30,
            f"the {menu} menu opened",
        )
        found = [
            match
            for match in re.finditer(MENU_ENTRY_RECT_PATTERN, session.text())
            if match.group(1) == menu and match.group(2) == label
        ]
        assert found, f"the {menu} menu did not list {label!r}: " + session.text()[-600:]
        entry = found[-1]
        x = float(entry.group(3)) + float(entry.group(5)) / 2.0
        y = float(entry.group(4)) + float(entry.group(6)) / 2.0
        begin = session.length()
        session.send(f"click {x:.0f} {y:.0f}")
        try:
            session.wait(
                log_layout.message_then_fields_pattern(re.escape(MENU_ACTION_TOKEN), rf'\s+menu="{menu}"\s+label="{re.escape(action_label)}"'),
                30,
                f"the {menu} menu ran {action_label!r}",
                begin,
            )
            return
        except TimeoutError:
            if attempt == 2:
                raise
            print(
                "  [RETRY] the {} menu's {!r} click missed; retrying once".format(
                    menu, label
                )
            )


def logged_scene_rect(session: EditorSession, view: int) -> tuple:
    """One scene viewport's last logged rectangle, visible flag included."""
    match = latest_log_match(session, SCENE_RECT_PATTERN, view, "scene view rect")
    return (
        float(match.group(2)),
        float(match.group(3)),
        float(match.group(4)),
        float(match.group(5)),
        match.group(6) == "true",
    )


def visible_scene_rect(text: str, view: int = 0) -> tuple:
    """The last visible rectangle of `view` in a scene dump, or None."""
    matches = [
        match
        for match in re.finditer(SCENE_RECT_PATTERN, text)
        if match.group(1) == str(view) and match.group(6) == "true"
    ]
    if not matches:
        return None
    return tuple(int(value) for value in matches[-1].groups()[1:5])


def dump_field_values(session: EditorSession, entity: str, field: str) -> "list[float]":
    """One array field of `entity`, from a scripted `dump field`."""
    dump = session.request(
        f"dump field {entity} {TRANSFORM_COMPONENT} {field}",
        re.escape(FIELD_TOKEN),
        30,
        f"the {field} dump",
    )
    match = re.search(r"value=Array\(\[([^\]]*)\]\)", dump)
    assert match, "the {} dump carried no array: {}".format(field, dump[-500:])
    return [
        float(value) for value in re.findall(r"F32\(([-\d.eE+]+)\)", match.group(1))
    ]


def wait_applied(session: EditorSession, since: int, label: str) -> None:
    """Waits until the editor applied a command batch past `since`.

    The session's anchored occurrence wait. It stays for the sites whose next
    observation does not prove the apply (a following action, a history-token
    wait, or a click-driven line whose batch crosses a frame boundary) and
    carries the step's `[OK]` label; sites that follow with a fresh anchored
    dump could drop it, but that conversion is deferred until the channel's
    batch-order contract has one proof per undo path.
    """
    session.wait_occurrence(
        re.escape(ENGINE_COMMANDS_APPLIED_TOKEN), since + 1, 60, label
    )


def wait_meta_value(
    session: EditorSession,
    path: Path,
    key: str,
    expected,
    timeout: float,
    label: str,
) -> None:
    """Waits until a `.meta` sidecar's `settings.<key>` carries `expected`."""

    def probe(_text: str):
        try:
            document = yaml.safe_load(path.read_text(encoding="utf-8"))
        except (OSError, yaml.YAMLError):
            document = {}
        if not isinstance(document, dict):
            document = {}
        return True if document.get("settings", {}).get(key) == expected else None

    session.wait_until(
        probe,
        timeout,
        f"{label}: {path} settings.{key} is not {expected!r}\n",
        label=label,
        # Poll until the apply writes the sidecar.
        poll=0.2,
    )


def wait_for_path(
    session: EditorSession, path: Path, exists: bool, timeout: float, label: str
) -> None:
    """Waits until the path's presence matches `exists`."""

    def probe(_text: str):
        return True if path.exists() == exists else None

    session.wait_until(
        probe,
        timeout,
        f"{label}: {path} still {'absent' if exists else 'present'}\n",
        label=label,
        # The command drain writes the file from another process: poll
        # rather than busy-wait.
        poll=0.2,
    )


def entity_count(session: EditorSession) -> int:
    """The entity list's row count, from one anchored dump."""
    pattern = log_layout.message_then_fields_pattern(re.escape(ENTITIES_DUMP_TOKEN), r"\s+count=(\d+)")
    dump = session.request("dump entities", pattern, 60, "the entity list")
    match = re.search(r"count=(\d+)", dump)
    assert match, "the entity dump carried no count: {}".format(dump[-500:])
    return int(match.group(1))


def wait_for_entity_count(session: EditorSession, target: int, label: str) -> None:
    """Waits until a fresh entity dump reports `target` entities."""

    def probe(_text: str):
        return True if entity_count(session) == target else None

    session.wait_until(
        probe,
        60,
        f"{label}: the entity list never reported count={target}\n",
        label=label,
        # Each round is its own anchored dump; the poll only spaces them out.
        poll=0.2,
    )


def write_theme_file(path: Path, guid: str, body) -> None:
    """Writes one standalone `.theme` document, the shape the loader reads."""
    path.parent.mkdir(parents=True, exist_ok=True)
    document = {
        "format_version": 1,
        "asset_type": "pill_ui_theme::ThemeOverrides",
        "guid": guid,
        "asset": body,
    }
    path.write_text(
        yaml.safe_dump(document, sort_keys=False, default_flow_style=False),
        encoding="utf-8",
    )


def dump_theme(session: EditorSession, label: str) -> str:
    """One `dump theme` line: the merge the shell installs."""
    text = session.request(
        "dump theme", re.escape(EDITOR_THEME_DUMP_TOKEN), 30, label
    )
    return [row for row in text.splitlines() if EDITOR_THEME_DUMP_TOKEN in row][-1]


def assert_theme(line: str, layers: str, accent: str, panel: str, label: str) -> int:
    """Asserts one dump and returns its generation."""
    assert "layers={}".format(layers) in line, "{}: {}".format(label, line)
    assert "accent={}".format(accent) in line, "{}: {}".format(label, line)
    assert "panel={}".format(panel) in line, "{}: {}".format(label, line)
    print("  [OK] {}".format(label))
    return int(re.search(r"generation=(\d+)", line).group(1))


# The two row helpers below select a row with lookaheads over its fields
# rather than consuming the message-to-fields join; the lookaheads read the
# whole line, so they are not built through `log_layout`.
def asset_row_capture(entity: str, field: str) -> str:
    """`entity`'s asset row for `field`, capturing the asset it names."""
    return (
        re.escape(REFERENCE_ROW_TOKEN)
        + r"(?=[^\n]*entity="
        + re.escape(entity)
        + r"\b)"
        + r'(?=[^\n]*field="'
        + field
        + r'\")[^\n]*asset="([^"]*)"'
    )


def asset_row_pattern(entity: str, field: str, asset: str) -> str:
    """`entity`'s asset row for `field`, naming `asset`."""
    return (
        re.escape(REFERENCE_ROW_TOKEN)
        + r"(?=[^\n]*entity="
        + re.escape(entity)
        + r"\b)"
        + r'(?=[^\n]*field="'
        + field
        + r'\")[^\n]*asset="'
        + re.escape(asset)
        + r'"'
    )


def inspector_section_rect(session: EditorSession, component: str) -> tuple:
    """One Inspector section header's last logged rectangle."""
    last = newest_log_match(
        session,
        INSPECTOR_SECTION_RECT_PATTERN,
        ((1, component),),
        label=f"no Inspector section for {component!r} in the log",
    )
    return (
        float(last.group(2)),
        float(last.group(3)),
        float(last.group(4)),
        float(last.group(5)),
    )


def inspector_add_rect(session: EditorSession) -> tuple:
    """The Add Component button's last logged rectangle."""
    last = newest_log_match(
        session,
        INSPECTOR_ADD_RECT_PATTERN,
        label="the Inspector's add button never reported its rectangle",
    )
    return (
        float(last.group(1)),
        float(last.group(2)),
        float(last.group(3)),
        float(last.group(4)),
    )


def latest_detail_entity(session: EditorSession) -> str:
    """The entity the newest rows heartbeat reports as the Inspector's detail.

    The `detail` field is read only from the rows heartbeat's own line: a
    hot-reload refusal logs a `detail=` field too, and an unanchored search
    would hand back that message as if it were an entity handle.
    """
    details = re.findall(
        log_layout.message_then_fields_pattern(
            re.escape(EGUI_ROWS_TOKEN), r'[^\n]*detail="(?!-)([^"]+)"'
        ),
        session.text(),
    )
    if not details:
        raise AssertionError("no rows heartbeat reported a selected entity")
    return details[-1]


# How many times the badge step clicks a chip before it reports a failure: the
# chips follow the scene camera, which eases for a few frames after a splitter
# drag, and the newest rectangle a click can be aimed at is still a frame old.
BADGE_CLICK_ATTEMPTS = 3

# One `[editor] scene badge rect` line, in the fields' rendered order.
SCENE_BADGE_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(
        re.escape(SCENE_BADGE_RECT_TOKEN),
        r"\s+view=\d+\s+entity=(\S+)\s+min_x=(-?[\d.]+)\s+min_y=(-?[\d.]+)",
    )
    + r"\s+max_x=(-?[\d.]+)\s+max_y=(-?[\d.]+)"
)


def newest_badge_rect(session: EditorSession, entity: str, start: int) -> tuple:
    """`(min_x, min_y, max_x, max_y)` of the badge's newest reported rectangle.

    A badge reports its rectangle whenever it moves, so the newest report is
    where the chip is now; the first one is where it was when the step began
    reading, which is not the same place after the camera has eased.
    """
    rects = [
        match
        for match in re.finditer(SCENE_BADGE_RECT_PATTERN, session.read_since(start))
        if match.group(1) == entity
    ]
    if not rects:
        raise AssertionError(
            "no scene badge rect for {} since the step began".format(entity)
        )
    newest = rects[-1]
    return (
        float(newest.group(2)),
        float(newest.group(3)),
        float(newest.group(4)),
        float(newest.group(5)),
    )


def inspector_rows(session: EditorSession, component_suffix: str) -> list:
    """Every drawn Inspector row of the components whose name ends that way.

    Each entry is `(kind, component, rest)`: the row's kind, the component it
    belongs to, and the rest of the line (a field's name and widget, a
    header's text, a gap's height) for the caller to read.
    """
    return [
        (match.group(1), match.group(2), match.group(3))
        for match in re.finditer(INSPECTOR_ROW_PATTERN, session.text())
        if match.group(2).endswith(component_suffix)
    ]


def latest_detail_components(session: EditorSession) -> int:
    """How many components the newest rows heartbeat sees on the detail entity."""
    counts = re.findall(
        log_layout.message_then_fields_pattern(
            re.escape(EGUI_ROWS_TOKEN), r"[^\n]*detail_components=(\d+)"
        ),
        session.text(),
    )
    if not counts:
        raise AssertionError("no rows heartbeat reported a component count")
    return int(counts[-1])


def wait_for_detail_components(session: EditorSession, target: int, label: str) -> None:
    """Waits until the newest rows heartbeat reports `target` component count."""

    def probe(text: str):
        counts = re.findall(r"detail_components=(\d+)", text)
        return True if counts and int(counts[-1]) == target else None

    session.wait_until(
        probe,
        60,
        "{}: the heartbeats never reported detail_components={}\n".format(label, target),
        label=label,
        # The rows heartbeat prints on its own cadence, so a short poll reads
        # a fresh count sooner than the shared half-second would.
        poll=0.3,
    )


def console_dump(session: EditorSession, begin: int):
    """The newest console dump written since `begin`, or None when none is."""
    matches = list(re.finditer(CONSOLE_DUMP_PATTERN, session.read_since(begin)))
    return matches[-1] if matches else None


def wait_for_stub_record(session: EditorSession, timeout: float = 30.0) -> str:
    """Waits for the stub editor to record its arguments, and returns them."""
    record = session.stub_editor_record

    def probe(_text: str):
        if record.is_file():
            text = record.read_text(encoding="utf-8").strip()
            if text:
                print("  [OK] the stub editor recorded {}".format(text))
                return text
        return None

    return session.wait_until(
        probe,
        timeout,
        "the stub code editor recorded nothing at {}\n".format(record),
        # The stub is a separate process writing a file: poll rather than
        # busy-wait, so the CPU stays free for the editor under test.
        poll=0.2,
        announce=False,
    )


def last_log_match(session: EditorSession, pattern: str, start: int = 0) -> re.Match:
    """The newest match of `pattern` in the log from `start`: a rectangle or a
    row can be logged again as the layout settles, and the latest one counts."""
    return newest_log_match(
        session, pattern, start=start, label="no match for {}".format(pattern)
    )


def logged_rect_centre(line: str) -> tuple:
    """The centre of a rectangle a log line spells as `min_x`..`max_y`."""
    values = {
        key: float(value)
        for key, value in re.findall(r"\b(min_x|min_y|max_x|max_y)=(-?[0-9.]+)", line)
    }
    return (
        (values["min_x"] + values["max_x"]) / 2.0,
        (values["min_y"] + values["max_y"]) / 2.0,
    )


def logged_rect_bounds(line: str) -> tuple:
    """A rectangle a log line spells as `min_x`..`max_y`, as (x, y, width, height)."""
    values = {
        key: float(value)
        for key, value in re.findall(r"\b(min_x|min_y|max_x|max_y)=(-?[0-9.]+)", line)
    }
    return (
        values["min_x"],
        values["min_y"],
        values["max_x"] - values["min_x"],
        values["max_y"] - values["min_y"],
    )


def rect_center(rect: tuple) -> tuple:
    """The center of an (x, y, width, height) rectangle."""
    return (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0)


def rect_contains(outer: tuple, inner: tuple) -> bool:
    """Whether `outer` holds `inner`, both (x, y, width, height), half a pixel slack."""
    return (
        inner[0] >= outer[0] - 0.5
        and inner[1] >= outer[1] - 0.5
        and inner[0] + inner[2] <= outer[0] + outer[2] + 0.5
        and inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
    )


def wait_for_frames_past(session: EditorSession, highest: int, timeout: float, label: str) -> int:
    """Waits until a viewport dump reports a frame counter past `highest`.

    The counter continuing proves the shell's session resource - the dock and
    the context - survived whatever reload or patch rewrote the module. The
    dump carries the shell's own counter, so the wait asks for the observation
    instead of waiting for the rows heartbeat's 210-frame cadence.
    """
    pattern = log_layout.message_then_fields_pattern(re.escape(VIEWPORT_DUMP_TOKEN), r"[^\n]*frame=(\d+)")

    def probe(_text: str):
        dump = session.request("dump viewport", pattern, 30, "the viewport dump")
        match = re.search(r"frame=(\d+)", dump)
        return int(match.group(1)) if match and int(match.group(1)) > highest else None

    return session.wait_until(
        probe,
        timeout,
        "{}: the viewport dump did not continue past frame {}; the shell's "
        "state was reset\n".format(label, highest),
        label=label,
        poll=0.2,
    )


def shell_frame(session: EditorSession) -> int:
    """The shell's frame counter, from one anchored viewport dump."""
    pattern = log_layout.message_then_fields_pattern(re.escape(VIEWPORT_DUMP_TOKEN), r"[^\n]*frame=(\d+)")
    dump = session.request("dump viewport", pattern, 30, "the frame counter")
    match = re.search(r"frame=(\d+)", dump)
    assert match, "the viewport dump carried no frame counter: {}".format(dump[-500:])
    return int(match.group(1))


def wait_for_frame_advance(session: EditorSession, label: str) -> int:
    """Waits until the shell's frame counter advances past its current value."""
    return wait_for_frames_past(session, shell_frame(session), 60, label)


class StepState:
    """What one step area hands to the next.

    A field exists because a step boundary crosses it: a handle the
    editor opened, an entity a later step edits, a path a restore step
    needs. A name an area both assigns and reads stays a local there, so
    the state holds only what the sequence actually shares.
    """

    def __init__(self) -> None:
        self.badge_begin = None
        self.begin = None
        self.picked_entity = None
        self.scene_height = None
        self.scene_rect = None
        self.scene_x = None
        self.scene_y = None
        self.session = None
        self.splitter_x = None
        self.splitter_y = None
        self.applied_before = None
        self.gizmo_entity = None
        self.mesh_entity = None
        self.original_material = None
        self.prior_entity = None
        self.asset_root = None
        self.before_plane = None
        self.cube_meta = None
        self.flip_before = None
        self.count_before = None
        self.project_settings_path = None
        self.theme_root = None
        self.font_generation = None
        self.showcase_rows = None
        self.slider = None
        self.highest = None
        self.original_source = None
        self.original_systems_source = None
        self.original_tab_source = None
        self.shell_source = None
        self.survived = None
        self.tab_entities_source = None
        self.tab_systems_source = None
        self.project_source = None

