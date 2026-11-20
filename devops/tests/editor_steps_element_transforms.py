"""The editor suite's element transform steps: gizmo drags that move elements.

A drag on a gizmo handle over an element selection belongs to the elements, not
to the entity: the editor measures the move exactly as it always does and hands
the active tool one finished world transform per element. These steps drive that
end to end through the fixture tool - three points move together in one undo
step, the space row reaches the tool's own frame and a drag along that frame's
axis produces the world displacement the frame implies, and a tool with no
frames leaves the row cycling the two settings' spaces.

The world positions come from `dump selection`, whose element rows carry the
translation the active tool reports; the log vocabulary comes from
`editor_log_tokens`.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    ELEMENT_DRAG_RECORDED_TOKEN,
    ELEMENT_HANDLES_TOKEN,
    ELEMENT_SELECTION_TOKEN,
    ELEMENT_TRANSFORMS_WRITTEN_TOKEN,
    GIZMO_ANCHOR_TOKEN,
    GIZMO_SPACE_TOKEN,
    HISTORY_UNDO_TOKEN,
    SELECTED_ELEMENT_TOKEN,
    TOOL_ACTIVATED_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import print_ok, StepState

# The three fixture points, by the ids the tool reports them under.
POINT_IDS = (0, 1, 2)

# How close two numbers have to be to count as the same one: the positions are
# read back from a `f32` log line, so the tolerance is what printing rounds off.
TOLERANCE = 1e-3

# How far a drag has to move a point before it counts as a move, in world units.
MINIMUM_DELTA = 1e-2


def message_pattern(token: str, fields: str) -> str:
    """The one-line pattern for a token followed by `fields`."""
    return log_layout.message_then_fields_pattern(re.escape(token), fields)


HANDLES_PATTERN = message_pattern(
    ELEMENT_HANDLES_TOKEN, r"\s+entity=\S+\s+kind_count=2\s+handles=5"
)
THREE_POINTS_PATTERN = message_pattern(
    ELEMENT_SELECTION_TOKEN, r'\s+kind="point"\s+selected=3'
)
TOOL_NONE_PATTERN = message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="none"')
TRANSLATE_ANCHOR_PATTERN = message_pattern(
    GIZMO_ANCHOR_TOKEN, r'.*\s+mode="?translate"?'
)
FRAME_ANCHOR_PATTERN = message_pattern(GIZMO_ANCHOR_TOKEN, r'.*\s+frame="Fixture"')
ELEMENT_ROW_PATTERN = message_pattern(
    SELECTED_ELEMENT_TOKEN,
    r'\s+entity=\S+\s+id=(\d+)\s+kind="(\w+)"'
    r"\s+x=(-?[\d.]+|-?[Nn]a[Nn])\s+y=(-?[\d.]+|-?[Nn]a[Nn])"
    r"\s+z=(-?[\d.]+|-?[Nn]a[Nn])",
)
ANCHOR_PATTERN = message_pattern(
    GIZMO_ANCHOR_TOKEN,
    r'\s+entity=\S+\s+mode="?(\w+)"?\s+space="?(\w+)"?\s+frame="?([^"\s]+)"?',
)
SPACE_ROW_PATTERN = message_pattern(GIZMO_SPACE_TOKEN, r'\s+space="?([^"\s]+)"?')
RECORDED_PATTERN = message_pattern(
    ELEMENT_DRAG_RECORDED_TOKEN, r'\s+entity=\S+\s+kind="point"\s+elements=(\d+)'
)
WRITTEN_PATTERN = message_pattern(
    ELEMENT_TRANSFORMS_WRITTEN_TOKEN, r'\s+entity=\S+\s+kind="?point"?\s+elements=(\d+)'
)


def read_element_positions(session) -> "dict":
    """One `dump selection`, as `{element id: (x, y, z)}` over the point rows."""
    dump = session.request(
        "dump selection", ELEMENT_ROW_PATTERN, 30, "the selection dump"
    )
    positions = {}
    for match in re.finditer(ELEMENT_ROW_PATTERN, dump):
        if match.group(2) != "point":
            continue
        positions[int(match.group(1))] = tuple(
            float(match.group(index)) for index in (3, 4, 5)
        )
    return positions


def read_gizmo_space(session) -> "tuple":
    """The newest gizmo anchor's `(space, frame)` from the drawn handles."""
    rows = list(re.finditer(ANCHOR_PATTERN, session.text()))
    assert rows, "no gizmo anchor was logged, so the handles were never drawn"
    newest = rows[-1]
    return newest.group(2), newest.group(3)


def cycle_to_world(session) -> None:
    """Walks the space row until its anchor reports the world space again."""
    for _ in range(4):
        space, _ = read_gizmo_space(session)
        if space == "world":
            return
        begin = session.length()
        session.send("command editor.tool_space_toggle")
        session.wait(SPACE_ROW_PATTERN, 30, "the space row walked on", begin)
    space, _ = read_gizmo_space(session)
    assert space == "world", "the space row never came back to world: {}".format(space)


def run_element_transform_steps(state: StepState) -> None:
    """The steps through: three points move, then the space row reaches a frame."""
    session = state.session

    # Step 1: the entity, the fixture tool and all three points selected. A
    # drag belongs to the elements only while the slot holds them and the
    # active tool writes elements back.
    begin = session.length()
    session.send(f"select entity {state.gizmo_entity}")
    session.send("tool fixture")
    session.wait(
        message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="fixture"'),
        30,
        "the fixture tool activated",
        begin,
    )
    session.wait(HANDLES_PATTERN, 30, "the element handles published", begin)
    begin = session.length()
    session.send("select element 0")
    session.send("select element add 1")
    session.send("select element add 2")
    session.wait(THREE_POINTS_PATTERN, 30, "all three points selected", begin)
    begin = session.length()
    session.send("gizmo translate")
    session.wait(
        TRANSLATE_ANCHOR_PATTERN, 30, "the translate tool active", begin
    )
    cycle_to_world(session)
    print_ok("the fixture's three points are selected under the translate tool")

    # Step 2: one drag on the Z handle moves all three points. World Z is the
    # axis the frame step below contrasts with.
    before = read_element_positions(session)
    assert sorted(before) == list(POINT_IDS), (
        "the dump did not report the three points: {}".format(sorted(before))
    )
    begin = session.length()
    session.send("gizmo drag z 0 -40")
    session.wait(RECORDED_PATTERN, 60, "the element drag recorded", begin)
    session.wait(WRITTEN_PATTERN, 60, "the tool wrote the moved elements", begin)
    after = read_element_positions(session)
    assert sorted(after) == list(POINT_IDS), (
        "the dump lost a point after the drag: {}".format(sorted(after))
    )
    deltas = {
        element: tuple(
            after[element][axis] - before[element][axis] for axis in range(3)
        )
        for element in POINT_IDS
    }
    for element, delta in deltas.items():
        assert abs(delta[0]) < TOLERANCE and abs(delta[1]) < TOLERANCE, (
            "point {} moved off the world Z axis: {}".format(element, delta)
        )
        assert abs(delta[2]) > MINIMUM_DELTA, (
            "point {} did not move along world Z: {}".format(element, delta)
        )
    first = deltas[POINT_IDS[0]]
    for element in POINT_IDS[1:]:
        delta = deltas[element]
        assert all(abs(delta[axis] - first[axis]) < TOLERANCE for axis in range(3)), (
            "the points moved by different amounts: {} against {}".format(delta, first)
        )
    print_ok(
        "one handle drag moved all three points by {:.3f} along world Z".format(first[2])
    )

    # Step 3: the drag is one undo step: one undo puts every point back.
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 60, "the element drag undone", begin)
    restored = read_element_positions(session)
    for element in POINT_IDS:
        assert element in restored, "the dump lost point {} after the undo".format(
            element
        )
        for axis in range(3):
            assert abs(restored[element][axis] - before[element][axis]) < TOLERANCE, (
                "point {} did not return to its pre-drag place on axis {}:"
                " {} against {}".format(
                    element, axis, restored[element][axis], before[element][axis]
                )
            )
    print_ok("one undo restored all three points")

    # Step 4: the space row reaches the tool's own frame, which the row names.
    # The cycle is the world space, the local one, then the fixture's frame.
    for expected in ("local", "Fixture"):
        begin = session.length()
        session.send("command editor.tool_space_toggle")
        session.wait(SPACE_ROW_PATTERN, 30, "the space row walked on", begin)
        reached = re.findall(SPACE_ROW_PATTERN, session.text())[-1]
        assert reached == expected, (
            "the space row reached {!r} instead of {!r}".format(reached, expected)
        )
    session.wait(
        FRAME_ANCHOR_PATTERN,
        30,
        "the gizmo drawn in the fixture's frame",
        session.length() - 1,
    )
    print_ok("the space row reached the tool's own frame and the gizmo draws in it")

    # Step 5: a drag on the frame's Y handle moves along the world axis the
    # frame implies - world Z - which the same handle in the world space does
    # not: there it moves along world Y.
    before = read_element_positions(session)
    begin = session.length()
    session.send("gizmo drag y 0 -40")
    session.wait(RECORDED_PATTERN, 60, "the frame drag recorded", begin)
    after = read_element_positions(session)
    for element in POINT_IDS:
        delta = tuple(
            after[element][axis] - before[element][axis] for axis in range(3)
        )
        assert abs(delta[0]) < TOLERANCE and abs(delta[1]) < TOLERANCE, (
            "a drag in the fixture's frame moved point {} off world Z: {}".format(
                element, delta
            )
        )
        assert abs(delta[2]) > MINIMUM_DELTA, (
            "a drag in the fixture's frame left point {} where it was: {}".format(
                element, delta
            )
        )
    print_ok("a drag along the frame's own axis produced the world delta it implies")

    # Step 6: the row leaves the frame for the world space, so the handles draw
    # in the world's axes again.
    cycle_to_world(session)
    _, frame = read_gizmo_space(session)
    assert frame == "-", "the gizmo still names a frame: {}".format(frame)
    print_ok("the space row left the tool frame for the world space")

    # Step 7: a tool with no frames leaves the row cycling the two settings'
    # spaces, never a frame.
    begin = session.length()
    session.send("tool none")
    session.wait(TOOL_NONE_PATTERN, 30, "the fixture tool cleared", begin)
    for expected in ("local", "world"):
        begin = session.length()
        session.send("command editor.tool_space_toggle")
        session.wait(
            message_pattern(GIZMO_SPACE_TOKEN, r'\s+space="?{}"?'.format(expected)),
            30,
            "the row walked to the {} space".format(expected),
            begin,
        )
    print_ok("a tool with no frames leaves the row cycling world and local")


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_element_transform_steps(context.state)


AREA = StepArea(
    name="element_transforms",
    order=5.6,
    run=_area_entry,
    needs=("gizmo_entity",),
    provides=(),
)
