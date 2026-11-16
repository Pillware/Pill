"""The editor suite's element tools steps: handles, hover, click, marquee.

The tools framework's first producer is the editor's own fixture tool, which a
scripted run registers beside the built-ins: it reports three `point` elements
and two `tangent` elements for the selected entity. These steps drive the
element half end to end through it - the published handles, the hover, a
click, an extend, a refusal across kinds and a marquee - and through the
`select element` verb, which applies the same click rule a pointer press does.

The tools it reads through come from `editor_step_support`, the log vocabulary
from `editor_log_tokens`.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    ELEMENT_CLICKED_TOKEN,
    ELEMENT_HANDLES_TOKEN,
    ELEMENT_HANDLE_TOKEN,
    ELEMENT_HOVER_NONE_TOKEN,
    ELEMENT_HOVER_TOKEN,
    ELEMENT_MARQUEE_TOKEN,
    ELEMENT_REFUSED_TOKEN,
    ELEMENT_SELECTION_TOKEN,
    HISTORY_UNDO_TOKEN,
    SCENE_RECT_PATTERN,
    SELECTED_ELEMENT_TOKEN,
    SELECTION_CHANGED_TOKEN,
    SELECTION_DUMP_TOKEN,
    TOOL_ACTIVATED_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import logged_scene_rect, print_ok, StepState


def message_pattern(token: str, fields: str) -> str:
    """The one-line pattern for a token followed by `fields`."""
    return log_layout.message_then_fields_pattern(re.escape(token), fields)


HANDLES_PATTERN = message_pattern(
    ELEMENT_HANDLES_TOKEN, r"\s+entity=\S+\s+kind_count=2\s+handles=5"
)
HANDLE_ROW_PATTERN = message_pattern(
    ELEMENT_HANDLE_TOKEN,
    r'\s+view=(\d+)\s+kind="([^"]+)"\s+element=(\d+)\s+handle=(\d+)'
    r"\s+x=(-?\d+)\s+y=(-?\d+)\s+selected=(true|false)",
)
SELECTION_DUMP_PATTERN = message_pattern(
    SELECTION_DUMP_TOKEN,
    r'\s+count=(\d+)\s+asset="[^"]*"\s+primary="[^"]*"\s+elements=(\d+)',
)


def read_element_handles(session) -> "dict":
    """The newest block of element handle rows, keyed by kind then element id.

    Each row carries what the step needs to aim at it: the view it was
    projected in, the pixels it landed on and whether the selection holds it.
    """
    rows = [
        match
        for match in re.finditer(HANDLE_ROW_PATTERN, session.text())
        if match.group(1) == "0"
    ]
    assert rows, "no element handle rows were logged for view 0"
    handles = {}
    for match in rows[-5:]:
        _, kind, element, handle, x, y, selected = match.groups()
        handles.setdefault(kind, {})[int(element)] = {
            "handle": int(handle),
            "x": int(x),
            "y": int(y),
            "selected": selected == "true",
        }
    return handles


def dump_selection(session) -> "tuple":
    """One anchored `dump selection`: the entity count, the element count, text."""
    dump = session.request(
        "dump selection", SELECTION_DUMP_PATTERN, 30, "the selection dump"
    )
    match = re.search(SELECTION_DUMP_PATTERN, dump)
    assert match, "the selection dump carried no counts: {}".format(dump[-500:])
    return int(match.group(1)), int(match.group(2)), dump


def run_element_tools_steps(state: StepState) -> None:
    """The steps through: a marquee selects three elements in one undo step."""
    session = state.session

    # Step 1: the entity the fixture reports its elements for, and the tool
    # itself. The tool is registered because a scripted channel drives this
    # run; a person's editor never lists it.
    begin = session.length()
    session.send(f"select entity {state.gizmo_entity}")
    session.wait(
        message_pattern(SELECTION_CHANGED_TOKEN, r".*count=1"),
        30,
        "the entity selected for the tool",
        begin,
    )
    begin = session.length()
    session.send("tool fixture")
    session.wait(
        message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="fixture"'),
        30,
        "the fixture tool activated",
        begin,
    )
    print_ok("the fixture tool reports elements for the selected entity")

    # Step 2: the handles land in the active view, five of them of two kinds.
    begin = session.length()
    session.wait(HANDLES_PATTERN, 30, "the element handles published", begin)
    handles = read_element_handles(session)
    points = handles.get("point", {})
    tangents = handles.get("tangent", {})
    assert sorted(points) == [0, 1, 2], "the points are not the fixture's three: {}".format(
        sorted(points)
    )
    assert sorted(tangents) == [10, 11], "the tangents are not the fixture's two: {}".format(
        sorted(tangents)
    )
    assert all(not row["selected"] for row in points.values()), (
        "an element was drawn as selected before the first click"
    )
    print_ok(
        "five handles of two kinds landed at "
        + ", ".join(
            "({},{})".format(row["x"], row["y"]) for row in points.values()
        )
    )

    # Step 3: the pointer highlights the handle under it and clears when it
    # leaves. The hover is aimed in the view's own pixels, which is where the
    # handle rows report them.
    first = points[0]
    begin = session.length()
    session.send(f"hover 0 {first['x']} {first['y']}")
    session.wait(
        message_pattern(
            ELEMENT_HOVER_TOKEN, r"\s+view=0\s+handle={}".format(first["handle"])
        ),
        30,
        "the element handle highlighted",
        begin,
    )
    begin = session.length()
    session.send("hover 0 5 5")
    session.wait(
        message_pattern(ELEMENT_HOVER_NONE_TOKEN, r"\s+view=0"),
        30,
        "the element highlight cleared",
        begin,
    )
    print_ok("the pointer highlights the element handle under it")

    # Step 4: a click on a handle selects it. A scripted click carries
    # main-window pixels, so the view's own rect converts the handle's place.
    session.send("dump scenes")
    session.wait(SCENE_RECT_PATTERN, 30, "the scene rects dumped")
    scene_x, scene_y, _, _, visible = logged_scene_rect(session, 0)
    assert visible, "scene view 0 is not visible"

    begin = session.length()
    session.send(f"click {int(scene_x) + first['x']} {int(scene_y) + first['y']}")
    session.wait(
        message_pattern(
            ELEMENT_CLICKED_TOKEN, r'\s+entity=\S+\s+kind="point"\s+element=0\s+count=1'
        ),
        30,
        "the element click applied",
        begin,
    )
    session.wait(
        message_pattern(ELEMENT_SELECTION_TOKEN, r'\s+kind="point"\s+selected=1'),
        30,
        "one point selected",
        begin,
    )
    _, selected_count, dump = dump_selection(session)
    assert selected_count == 1, "the click selected {} elements".format(selected_count)
    assert re.search(
        message_pattern(SELECTED_ELEMENT_TOKEN, r'.*id=0\s+kind="point"'), dump
    ), "the selected element line names the clicked point: {}".format(dump[-500:])
    print_ok("a click on a handle selects its element")

    # Step 5: an extend adds the second point and the third is refused with the
    # kind named - the rule a Ctrl-click across kinds hits.
    begin = session.length()
    session.send("select element add 1")
    session.wait(
        message_pattern(ELEMENT_SELECTION_TOKEN, r'\s+kind="point"\s+selected=2'),
        30,
        "the second point extended the selection",
        begin,
    )
    begin = session.length()
    session.send("select element add 10")
    session.wait(
        message_pattern(ELEMENT_REFUSED_TOKEN, r'\s+entity=\S+\s+kind="tangent"'),
        30,
        "the tangent extend refused by kind",
        begin,
    )
    _, selected_count, _ = dump_selection(session)
    assert selected_count == 2, (
        "the refused extend changed the selection to {} elements".format(selected_count)
    )
    print_ok("an extend adds inside the kind and is refused across kinds")

    # Step 6: a marquee over the three points selects all of them. The drag
    # starts beyond the points' own box, so its press lands in empty space, and
    # it is aimed from the far corner so the panel in the viewport's top-left
    # corner can never take it.
    spacing = max(row["x"] for row in points.values()) - min(
        row["x"] for row in points.values()
    )
    assert spacing > 12, "the three points are only {} px apart".format(spacing)
    left = min(row["x"] for row in points.values()) - 24
    right = max(row["x"] for row in points.values()) + 24
    top = min(row["y"] for row in points.values()) - 24
    bottom = max(row["y"] for row in points.values()) + 24
    begin = session.length()
    session.send(
        "drag {} {} {} {}".format(
            int(scene_x) + right,
            int(scene_y) + bottom,
            int(scene_x) + left,
            int(scene_y) + top,
        )
    )
    session.wait(
        message_pattern(ELEMENT_MARQUEE_TOKEN, r'.*state="started"'),
        60,
        "the marquee started",
        begin,
    )
    session.wait(
        message_pattern(ELEMENT_MARQUEE_TOKEN, r'.*count=3.*state="released"'),
        60,
        "the marquee selected three elements",
        begin,
    )
    session.wait(
        message_pattern(ELEMENT_SELECTION_TOKEN, r'\s+kind="point"\s+selected=3'),
        60,
        "three points selected",
        begin,
    )
    print_ok("a marquee over three points selects all three")

    # Step 7: the marquee is one undo step: one undo returns the two points
    # the extend had selected, not an empty or a partly restored set.
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 60, "the marquee undone", begin)
    _, selected_count, _ = dump_selection(session)
    assert selected_count == 2, (
        "one undo left {} elements; the marquee was not one step".format(
            selected_count
        )
    )
    print_ok("the marquee undoes as one step")

    # Step 8: the neutral tool, so the areas after this one keep the plain
    # press rules.
    begin = session.length()
    session.send("tool none")
    session.wait(
        message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="none"'),
        30,
        "the tool cleared",
        begin,
    )
    print_ok("the fixture tool cleared")


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_element_tools_steps(context.state)


AREA = StepArea(
    name="element_tools",
    order=5.5,
    run=_area_entry,
    needs=("gizmo_entity",),
    provides=(),
)
