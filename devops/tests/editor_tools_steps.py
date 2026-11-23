"""The editor tools suite's steps: the tools framework against real extensions.

The tools framework is only worth anything if a real tool, in a real extension,
edits a real component. These steps start two editor sessions of their own, each
on an extension example project, and drive one tool end to end:

- the spline tool on `extensions/pill_spline/examples/uber_spline`: its row
  in the floating toolbox appearing for the entity that carries a spline and
  entering the tool, its points
  and tangents reported as elements, two points selected, the `Spline` frame
  reached through the space row, a drag written back through the tool and undone
  as one step, a point inserted from the context menu and deleted with Delete,
  and the row going away for an entity that carries no spline;
- the collider tool on `extensions/pill_physics/examples/collider_edit`: six
  face handles on the cuboid, a face drag that changes `dimensions` and undoes
  in one step, and the ball's single radius handle.

They are a module of their own rather than a step area, because an area runs
beside the suite's shared editor session and starting a second editor kills the
first: one editor at a time is the harness's rule, not a limitation of these
steps. `test_editor_tools.py` is the suite that owns both sessions.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    DELETE_ELEMENTS_QUEUED_TOKEN,
    EDITOR_WINDOW_CREATED_TOKEN,
    EGUI_SHELL_LOADED_TOKEN,
    ELEMENT_DUMP_ROW_TOKEN,
    ELEMENT_MENU_RAN_TOKEN,
    ELEMENT_MENU_ROW_TOKEN,
    ELEMENT_SELECTION_TOKEN,
    ELEMENTS_DUMP_TOKEN,
    ELEMENTS_INSERTED_TOKEN,
    ELEMENTS_REMOVED_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    ENTITIES_DUMP_TOKEN,
    ENTITY_ROW_TOKEN,
    ELEMENT_TRANSFORMS_WRITTEN_TOKEN,
    FIELD_TOKEN,
    GIZMO_SPACE_TOKEN,
    HISTORY_UNDO_TOKEN,
    NEW_ENTITY_TOKEN,
    SELECTION_CHANGED_TOKEN,
    TOOL_ACTIVATED_TOKEN,
    TOOLBOX_ITEM_TOKEN,
    TOOLBOX_RECT_TOKEN,
)
from core.editor_session import EditorSession
from core.projects import EXTENSION_EXAMPLES_ROOT, host_project_path
from editor_step_support import (
    entity_count,
    occurrence_count,
    print_ok,
    toolbox_click,
    wait_applied,
    wait_for_entity_count,
)

# The two extension examples these steps drive, by the path a host resolves.
UBER_SPLINE_PROJECT = host_project_path(
    EXTENSION_EXAMPLES_ROOT / "pill_spline" / "examples" / "uber_spline"
)
COLLIDER_PROJECT = host_project_path(
    EXTENSION_EXAMPLES_ROOT / "pill_physics" / "examples" / "collider_edit"
)

# What the spline tool calls its elements, and what the collider tool calls its
# own. These are the tools' public vocabulary, not a suite convention.
SPLINE_POINT_KIND = "control point"
COLLIDER_FACE_KIND = "collider face"
COLLIDER_RADIUS_KIND = "collider radius"
SPLINE_FRAME_NAME = "Spline"
INSERT_POINT_ROW = "insert"


def message_pattern(token: str, fields: str) -> str:
    """The one-line pattern for a token followed by `fields`."""
    return log_layout.message_then_fields_pattern(re.escape(token), fields)


ENTITIES_DUMP_PATTERN = message_pattern(ENTITIES_DUMP_TOKEN, r"\s+count=(\d+)")
ENTITY_ROW_PATTERN = message_pattern(
    ENTITY_ROW_TOKEN, r'\s+display="([^"]+)"\s+components="([^"]*)"'
)
ELEMENTS_DUMP_PATTERN = message_pattern(
    ELEMENTS_DUMP_TOKEN,
    r'\s+entity=\S+\s+count=(\d+)\s+kinds=(\d+)\s+kind="([^"]*)"\s+selected=(\d+)',
)
ELEMENT_ROW_PATTERN = message_pattern(
    ELEMENT_DUMP_ROW_TOKEN,
    r'\s+entity=\S+\s+id=(\d+)\s+kind="([^"]+)"'
    r"\s+x=(-?[\d.eE+]+)\s+y=(-?[\d.eE+]+)\s+z=(-?[\d.eE+]+)\s+selected=(true|false)",
)
ELEMENT_MENU_ROW_PATTERN = message_pattern(
    ELEMENT_MENU_ROW_TOKEN, r'\s+item="([^"]+)"\s+label="([^"]+)"'
)
TOOL_ACTIVATED_PATTERN = message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="([^"]+)"')
ELEMENT_SELECTED_PATTERN = message_pattern(
    ELEMENT_SELECTION_TOKEN, r'\s+kind="([^"]*)"\s+selected=(\d+)'
)
TRANSFORMS_WRITTEN_PATTERN = message_pattern(
    ELEMENT_TRANSFORMS_WRITTEN_TOKEN, r".*\s+elements=(\d+)"
)
ELEMENTS_INSERTED_PATTERN = message_pattern(ELEMENTS_INSERTED_TOKEN, r".*\s+elements=(\d+)")
ELEMENTS_REMOVED_PATTERN = message_pattern(ELEMENTS_REMOVED_TOKEN, r".*\s+elements=(\d+)")
MENU_RAN_PATTERN = message_pattern(
    ELEMENT_MENU_RAN_TOKEN, r'\s+item="([^"]+)"\s+commands=(\d+)'
)
DIMENSIONS_PATTERN = r"value=Array\(\[([^\]]*)\]\)"


def open_session(binary, project: str, name: str) -> EditorSession:
    """Starts one editor process on `project`, its startup already waited for.

    The session keeps its own sandbox on purpose: the suite's shared editor is
    still running while these steps do, and two editors reading and writing one
    set of layout and preference files is a race neither of them needs.
    """
    session = EditorSession(
        "egui",
        name,
        project=project,
        test_seconds=240,
        binary=binary,
    )
    session.start()
    session.wait_for_match(
        re.escape(EDITOR_WINDOW_CREATED_TOKEN), 180, "the {} editor window".format(name)
    )
    session.wait_for_match(
        re.escape(EGUI_SHELL_LOADED_TOKEN), 180, "the {} shell load".format(name)
    )
    return session


def read_entities(session) -> "list":
    """One `dump entities`: every row as `(display, component label)`."""
    dump = session.request("dump entities", ENTITIES_DUMP_PATTERN, 30, "the entity list")
    rows = [
        (match.group(1), match.group(2))
        for match in re.finditer(ENTITY_ROW_PATTERN, dump)
    ]
    assert rows, "the entity dump carried no rows: {}".format(dump[-500:])
    return rows


def read_elements(session) -> "tuple":
    """One `dump elements`: `(count, {id: (kind, x)}, menu row ids)`."""
    dump = session.request(
        "dump elements", ELEMENTS_DUMP_PATTERN, 30, "the element dump"
    )
    head = re.search(ELEMENTS_DUMP_PATTERN, dump)
    assert head, "the element dump carried no counts: {}".format(dump[-500:])
    rows = {}
    for match in re.finditer(ELEMENT_ROW_PATTERN, dump):
        rows[int(match.group(1))] = (match.group(2), float(match.group(3)))
    menu = [match.group(1) for match in re.finditer(ELEMENT_MENU_ROW_PATTERN, dump)]
    return int(head.group(1)), rows, menu


def select_entity(session, display: str, description: str) -> None:
    """Selects one entity, waiting for the change only when there is one."""
    begin = session.length()
    session.send("select entity {}".format(display))
    session.wait(re.escape(SELECTION_CHANGED_TOKEN), 30, description, begin)


def select_element(session, element: int, add: bool, description: str) -> None:
    """Selects one element, or extends the selection with it."""
    begin = session.length()
    session.send(
        "select element {}{}".format("add " if add else "", element)
    )
    session.wait(ELEMENT_SELECTED_PATTERN, 30, description, begin)


def undo_once(session, description: str) -> None:
    """Undoes one step and waits for the history line."""
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 60, description, begin)


def drag_element(session, element: int, delta: "tuple", description: str) -> None:
    """Nudges one element of the active tool by a world delta."""
    begin = session.length()
    session.send(
        "drag element {} {} {} {}".format(element, delta[0], delta[1], delta[2])
    )
    moved = session.wait_for_match(
        TRANSFORMS_WRITTEN_PATTERN, 30, description, begin
    )
    assert re.search(r"elements=\d+", moved), "the drag wrote nothing: {}".format(moved)


def collider_dimensions(session, display: str) -> "list":
    """One `dump field` of the collider's dimensions, as three numbers."""
    dump = session.request(
        "dump field {} ColliderComponent dimensions".format(display),
        re.escape(FIELD_TOKEN),
        30,
        "the collider dimensions",
    )
    match = re.search(DIMENSIONS_PATTERN, dump)
    assert match, "the dimensions dump carried no array: {}".format(dump[-500:])
    return [
        float(value) for value in re.findall(r"F32\(([-\d.eE+]+)\)", match.group(1))
    ]


def run_spline_tool_steps(binary) -> None:
    """The spline tool, on the project that ships a spline."""
    session = open_session(binary, UBER_SPLINE_PROJECT, "tools_spline")
    try:
        # Step 1: the entity the spline crate's example spawned is the subject,
        # and the floating toolbox's own row is how a person enters its tool.
        rows = read_entities(session)
        spline_entity = next(
            display for display, components in rows if "Spline" in components
        )
        begin = session.length()
        select_entity(session, spline_entity, "the spline entity selected")
        session.wait(
            message_pattern(TOOLBOX_ITEM_TOKEN, r'\s+view=0\s+item="spline"'),
            30,
            "the spline row appears for the entity that carries a spline",
            begin,
        )
        begin = session.length()
        toolbox_click(session, 0, "spline", "the toolbox row entered the spline tool")
        session.wait(
            message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="spline"'),
            30,
            "the spline tool activated",
            begin,
        )
        print_ok("the toolbox row enters the spline tool for the entity that carries one")

        # Step 2: it reports the four control points and their four tangents.
        count, elements, _ = read_elements(session)
        assert count == 8, "the spline reports {} elements, not eight".format(count)
        kinds = sorted({row[0] for row in elements.values()})
        assert kinds == sorted([SPLINE_POINT_KIND, "tangent"]), (
            "the spline tool reports the kinds {}".format(kinds)
        )
        print_ok("four control points and four tangent handles are reported")

        # Step 3: two points selected, and the space row reaches the tool's own
        # frame - a drag along its X axis then follows the curve.
        select_element(session, 0, False, "the first point selected")
        select_element(session, 1, True, "the second point added to the selection")
        begin = session.length()
        session.send("command editor.tool_space_toggle")
        session.send("command editor.tool_space_toggle")
        session.wait(
            message_pattern(GIZMO_SPACE_TOKEN, r'\s+space="([^"]+)"'),
            30,
            "the space row reached the tool's frame",
            begin,
        )
        space_dump = session.read_since(begin)
        assert 'space="{}"'.format(SPLINE_FRAME_NAME) in space_dump, (
            "the space row never named the {} frame: {}".format(
                SPLINE_FRAME_NAME, space_dump[-500:]
            )
        )
        print_ok("the space row reaches the tool's own frame")

        # Step 4: a drag writes through the tool and undoes as one step.
        _, before, _ = read_elements(session)
        start_x = before[0][1]
        drag_element(session, 0, (10.0, 0.0, 0.0), "the first point dragged")
        _, moved, _ = read_elements(session)
        assert abs(moved[0][1] - (start_x + 10.0)) < 1e-3, (
            "the drag left the point at x={} instead of {}".format(
                moved[0][1], start_x + 10.0
            )
        )
        undo_once(session, "the point drag undone")
        _, restored, _ = read_elements(session)
        assert abs(restored[0][1] - start_x) < 1e-3, (
            "one undo left the point at x={}".format(restored[0][1])
        )
        print_ok("a point drag writes through the tool and undoes as one step")

        # Step 5: the insert row adds a point, through the same recorded batch
        # every tool edit takes.
        count_before, _, _ = read_elements(session)
        begin = session.length()
        session.send("element menu {}".format(INSERT_POINT_ROW))
        session.wait(
            message_pattern(
                ELEMENT_MENU_RAN_TOKEN, r'\s+item="insert"\s+commands=1'
            ),
            30,
            "the insert row ran",
            begin,
        )
        session.wait(ELEMENTS_INSERTED_PATTERN, 30, "the point inserted", begin)
        count_after, _, _ = read_elements(session)
        assert count_after == count_before + 2, (
            "the insert left {} elements, not {}".format(count_after, count_before + 2)
        )
        undo_once(session, "the insert undone")
        count_undone, _, _ = read_elements(session)
        assert count_undone == count_before, (
            "one undo left {} elements; the insert was not one step".format(
                count_undone
            )
        )
        print_ok("the context menu inserts a point in one undo step")

        # Step 6: Delete removes the selected point through the tool, and the
        # undo puts it back. The selection is re-made here, because undoing an
        # insert clears the element slot with the element it took away.
        select_element(session, 0, False, "a point selected for the delete")
        begin = session.length()
        session.send("command editor.entity_delete")
        session.wait(
            message_pattern(DELETE_ELEMENTS_QUEUED_TOKEN, r".*\s+count=1"),
            30,
            "the element delete queued",
            begin,
        )
        session.wait(ELEMENTS_REMOVED_PATTERN, 30, "the point removed", begin)
        count_deleted, _, _ = read_elements(session)
        assert count_deleted == count_before - 2, (
            "the delete left {} elements".format(count_deleted)
        )
        undo_once(session, "the delete undone")
        count_restored, _, _ = read_elements(session)
        assert count_restored == count_before, (
            "one undo left {} elements; the delete was not one step".format(
                count_restored
            )
        )
        print_ok("Delete removes the selected point and one undo restores it")

        # Step 7: an entity that carries no spline is offered no row. The tool
        # keeps running while its row is gone - that is what strict visibility
        # means - and the built-in tool rows stay there to leave it.
        count_before = entity_count(session)
        begin = session.length()
        applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
        session.send("command editor.entity_create")
        session.wait(re.escape(NEW_ENTITY_TOKEN), 30, "the new entity queued", begin)
        wait_applied(session, applied_before, "the new entity applied")
        wait_for_entity_count(session, count_before + 1, "the create added one entity")
        plain_entity = next(
            display
            for display, components in read_entities(session)
            if "Spline" not in components
        )
        begin = session.length()
        select_entity(session, plain_entity, "the entity with no spline selected")
        session.wait(
            message_pattern(TOOLBOX_RECT_TOKEN, r"\s+view=0"),
            30,
            "the panel re-laid out for the entity with no spline",
            begin,
        )
        block = session.read_since(begin)
        assert 'item="spline"' not in block, (
            "the spline row is drawn for an entity that carries none: {}".format(
                block[-500:]
            )
        )
        print_ok("an entity with no spline offers no toolbox row")

        # Step 8: back to no tool, so the session ends the way it started.
        begin = session.length()
        session.send("tool none")
        session.wait(
            message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="none"'),
            30,
            "the tool cleared",
            begin,
        )
    finally:
        session.close()


def run_collider_tool_steps(binary) -> None:
    """The collider tool, on the project that ships two colliders."""
    session = open_session(binary, COLLIDER_PROJECT, "tools_collider")
    try:
        # Step 1: the example spawns the cuboid first, so the entity list reads
        # it above the ball. The order is the example's own documentation.
        rows = read_entities(session)
        colliders = [
            display for display, components in rows if "ColliderComponent" in components
        ]
        assert len(colliders) == 2, (
            "the example spawned {} colliders, not two".format(len(colliders))
        )
        cuboid, ball = colliders[0], colliders[1]
        select_entity(session, cuboid, "the cuboid selected")
        begin = session.length()
        session.send("tool collider")
        session.wait(
            message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="collider"'),
            30,
            "the collider tool activated",
            begin,
        )
        print_ok("the collider tool is offered for the entity that carries a collider")

        # Step 2: a cuboid offers six face handles.
        count, faces, _ = read_elements(session)
        assert count == 6, "the cuboid reports {} handles, not six".format(count)
        assert all(row[0] == COLLIDER_FACE_KIND for row in faces.values()), (
            "the cuboid's handles are of the kinds {}".format(
                sorted({row[0] for row in faces.values()})
            )
        )
        print_ok("a cuboid reports six face handles")

        # Step 3: dragging the `+X` face writes the half extent along that axis,
        # and the change is one undo step.
        before = collider_dimensions(session, cuboid)
        drag_element(session, 1, (1.0, 0.0, 0.0), "the +X face dragged")
        during = collider_dimensions(session, cuboid)
        assert abs(during[0] - (before[0] + 1.0)) < 1e-3, (
            "the face drag left the half extent at {} instead of {}".format(
                during[0], before[0] + 1.0
            )
        )
        assert abs(during[1] - before[1]) < 1e-3 and abs(during[2] - before[2]) < 1e-3, (
            "the face drag changed an axis it does not own: {} -> {}".format(
                before, during
            )
        )
        undo_once(session, "the face drag undone")
        after = collider_dimensions(session, cuboid)
        assert abs(after[0] - before[0]) < 1e-3, (
            "one undo left the half extent at {}".format(after[0])
        )
        print_ok("a face drag changes one half extent and undoes as one step")

        # Step 4: the ball offers its radius handle and nothing else.
        select_entity(session, ball, "the ball selected")
        count, handles, _ = read_elements(session)
        assert count == 1, "the ball reports {} handles, not one".format(count)
        only = next(iter(handles.values()))
        assert only[0] == COLLIDER_RADIUS_KIND, (
            "the ball's one handle is of the kind {}".format(only[0])
        )
        print_ok("a ball reports its radius handle alone")

        # Step 5: back to no tool, so the session ends the way it started.
        begin = session.length()
        session.send("tool none")
        session.wait(
            message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="none"'),
            30,
            "the tool cleared",
            begin,
        )
    finally:
        session.close()


def run_tools_steps(binary) -> None:
    """Both sessions: the spline tool, then the collider tool."""
    run_spline_tool_steps(binary)
    run_collider_tool_steps(binary)
