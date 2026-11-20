"""The editor suite's element menu and override steps.

A tool does more than report elements: it adds rows to the element context menu,
edits its element set, and can override what a press or a key does. These steps
drive all of it end to end through the fixture tool - a menu row that inserts a
point and one that resets the selected points, Delete removing the selection
through the tool, a direct nudge, and the three press answers (the editor's own
rule, `Consume` and `Select`).

Every edit is one recorded step, so each step here undoes once and the element
count is the observable: the count the tool reports comes from `dump elements`,
and the selection from the `elements selected` line.

The log vocabulary comes from `editor_log_tokens`; the tokens' field order is
the contract these patterns pin.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    DELETE_ELEMENTS_QUEUED_TOKEN,
    ELEMENT_DUMP_ROW_TOKEN,
    ELEMENT_HANDLES_TOKEN,
    ELEMENT_KEY_COMMANDS_TOKEN,
    ELEMENT_KEY_CONSUMED_TOKEN,
    ELEMENT_MENU_QUEUED_TOKEN,
    ELEMENT_MENU_RAN_TOKEN,
    ELEMENT_MENU_ROW_TOKEN,
    ELEMENT_NUDGE_TOKEN,
    ELEMENT_PRESS_CONSUMED_TOKEN,
    ELEMENT_PRESS_SELECTED_TOKEN,
    ELEMENT_PRESS_TAKEN_TOKEN,
    ELEMENT_SELECTION_TOKEN,
    ELEMENT_TRANSFORMS_WRITTEN_TOKEN,
    ELEMENTS_DUMP_TOKEN,
    ELEMENTS_INSERTED_TOKEN,
    ELEMENTS_REMOVED_TOKEN,
    HISTORY_UNDO_TOKEN,
    TOOL_ACTIVATED_TOKEN,
    TOOL_SELECTION_CHANGED_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import print_ok, StepState

# The fixture tool's two menu rows, as the tool reports them.
INSERT_ROW = "insert"
RESET_ROW = "reset"


def message_pattern(token: str, fields: str) -> str:
    """The one-line pattern for a token followed by `fields`."""
    return log_layout.message_then_fields_pattern(re.escape(token), fields)


HANDLES_PATTERN = message_pattern(
    ELEMENT_HANDLES_TOKEN, r"\s+entity=\S+\s+kind_count=2\s+handles=5"
)
ELEMENTS_COUNT_PATTERN = message_pattern(
    ELEMENTS_DUMP_TOKEN, r"\s+entity=\S+\s+count=(\d+)\s+kinds=(\d+)"
)
ELEMENT_ROW_PATTERN = message_pattern(
    ELEMENT_DUMP_ROW_TOKEN,
    r'\s+entity=\S+\s+id=(\d+)\s+kind="(\w+)"'
    r"\s+x=(-?[\d.]+)\s+y=(-?[\d.]+)\s+z=(-?[\d.]+)\s+selected=(true|false)",
)
ELEMENT_MENU_ROW_PATTERN = message_pattern(
    ELEMENT_MENU_ROW_TOKEN, r'\s+item="([^"]+)"\s+label="([^"]+)"'
)


def read_elements(session) -> "tuple":
    """One `dump elements`: `(count, {id: (kind, x, y, z, selected)}, menu ids)`."""
    dump = session.request(
        "dump elements", ELEMENTS_COUNT_PATTERN, 30, "the element dump"
    )
    count_match = re.search(ELEMENTS_COUNT_PATTERN, dump)
    assert count_match, "the element dump carried no count: {}".format(dump[-500:])
    elements = {}
    for match in re.finditer(ELEMENT_ROW_PATTERN, dump):
        elements[int(match.group(1))] = (
            match.group(2),
            float(match.group(3)),
            float(match.group(4)),
            float(match.group(5)),
            match.group(6) == "true",
        )
    menu = [match.group(1) for match in re.finditer(ELEMENT_MENU_ROW_PATTERN, dump)]
    return int(count_match.group(1)), elements, menu


def selected_ids(elements: "dict") -> "list":
    """The ids the dump marked as selected, in id order."""
    return sorted(element for element, row in elements.items() if row[4])


def wait_selected(session, count: int, description: str, begin: int) -> None:
    """Waits for the element selection to land on `count` elements.

    `begin` is the log length captured BEFORE the action that changes the
    selection: the applied selection line is logged in the same batch as the
    edit that caused it, so a caller that waits for the edit first would miss
    it.
    """
    session.wait(
        message_pattern(
            ELEMENT_SELECTION_TOKEN, r'\s+kind="\w*"\s+selected={}'.format(count)
        ),
        60,
        description,
        begin,
    )


def undo_once(session, description: str) -> None:
    """Undoes one step and waits for the history line."""
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 60, description, begin)


def run_element_menu_steps(state: StepState) -> None:
    """The steps through: menu rows, insert, delete, nudge and the overrides."""
    session = state.session

    # Step 1: the fixture tool on the selected entity, with its three points
    # selected, and the dump that says what the tool reports and offers.
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
    wait_selected(session, 3, "the three points selected", begin)
    count, elements, menu = read_elements(session)
    assert count == 5, "the fixture reports {} elements, not five".format(count)
    assert INSERT_ROW in menu, "the insert row is missing from the menu: {}".format(menu)
    assert RESET_ROW in menu, "the reset row is missing from the menu: {}".format(menu)
    assert selected_ids(elements) == [0, 1, 2], (
        "the dump says {} is selected".format(selected_ids(elements))
    )
    print_ok("the tool reports five elements and two menu rows for three selected")

    # Step 2: a plain press on a point that is already selected is taken over
    # by the tool: it logs the press, hears the change and consumes the press,
    # so the selection does not move.
    begin = session.length()
    session.send("select element 0")
    session.wait(
        message_pattern(ELEMENT_PRESS_TAKEN_TOKEN, r"\s+element=0\s+selected=3"),
        30,
        "the tool took the press",
        begin,
    )
    session.wait(
        message_pattern(ELEMENT_PRESS_CONSUMED_TOKEN, r".*\s+element=0"),
        30,
        "the press consumed",
        begin,
    )
    count_after, elements_after, _ = read_elements(session)
    assert count_after == count, "a consumed press changed the element count"
    assert selected_ids(elements_after) == [0, 1, 2], (
        "a consumed press changed the selection to {}".format(
            selected_ids(elements_after)
        )
    )
    print_ok("a press the tool consumes leaves the selection untouched")

    # Step 3: the tool's own toggle. A Ctrl press on a selected point answers
    # `Select` with the rest of the selection, and exactly that lands.
    begin = session.length()
    session.send("select element add 1")
    session.wait(
        message_pattern(ELEMENT_PRESS_SELECTED_TOKEN, r".*\s+element=1\s+count=2"),
        30,
        "the tool answered with its own selection",
        begin,
    )
    wait_selected(session, 2, "two points left selected", begin)
    session.wait(
        message_pattern(
            TOOL_SELECTION_CHANGED_TOKEN, r".*\s+before=3\s+after=2"
        ),
        30,
        "the tool heard the selection change",
        begin,
    )
    count_after, elements_after, _ = read_elements(session)
    assert selected_ids(elements_after) == [0, 2], (
        "the tool's answer did not set exactly what it named: {}".format(
            selected_ids(elements_after)
        )
    )
    print_ok("a press the tool answers with Select sets exactly what it names")

    # Step 4: the tool's menu inserts a point, and one undo takes it away.
    count_before, _, _ = read_elements(session)
    begin = session.length()
    session.send(f"element menu {INSERT_ROW}")
    session.wait(
        message_pattern(ELEMENT_MENU_QUEUED_TOKEN, r'.*\s+item="insert"'),
        30,
        "the menu row queued",
        begin,
    )
    session.wait(
        message_pattern(ELEMENT_MENU_RAN_TOKEN, r'\s+item="insert"\s+commands=1'),
        30,
        "the tool ran its menu row",
        begin,
    )
    session.wait(
        message_pattern(ELEMENTS_INSERTED_TOKEN, r".*\s+elements=1"),
        30,
        "the point inserted",
        begin,
    )
    count_inserted, _, _ = read_elements(session)
    assert count_inserted == count_before + 1, (
        "the insert row left {} elements, not {}".format(
            count_inserted, count_before + 1
        )
    )
    undo_once(session, "the insert undone")
    count_undone, _, _ = read_elements(session)
    assert count_undone == count_before, (
        "one undo left {} elements; the insert was not one step".format(count_undone)
    )
    print_ok("a tool menu row inserts a point and undoes in one step")

    # Step 5: a direct nudge and the reset row both go through the tool's
    # write-back, which is the recorded step a gizmo drag ends with.
    nudge_id = sorted(elements)[0]
    begin = session.length()
    session.send(f"drag element {nudge_id} 1 0 0")
    session.wait(
        message_pattern(ELEMENT_NUDGE_TOKEN, r".*\s+element={}".format(nudge_id)),
        30,
        "the nudge queued",
        begin,
    )
    session.wait(
        message_pattern(ELEMENT_TRANSFORMS_WRITTEN_TOKEN, r".*\s+elements=1"),
        30,
        "the nudge written through the tool",
        begin,
    )
    _, nudged, _ = read_elements(session)
    begin = session.length()
    session.send(f"element menu {RESET_ROW}")
    session.wait(
        message_pattern(ELEMENT_MENU_RAN_TOKEN, r'\s+item="reset"\s+commands=1'),
        30,
        "the reset row ran",
        begin,
    )
    session.wait(
        message_pattern(ELEMENT_TRANSFORMS_WRITTEN_TOKEN, r".*\s+elements=2"),
        30,
        "the reset written through the tool",
        begin,
    )
    _, reset, _ = read_elements(session)
    moved = nudged[nudge_id][1]
    assert abs(reset[nudge_id][1] - moved) > 1e-3, (
        "the reset row left point {} at x={}".format(nudge_id, reset[nudge_id][1])
    )
    undo_once(session, "the reset undone")
    print_ok("a direct nudge and a menu row both go through the tool's write-back")

    # Step 6: Delete removes the selection through the tool, in one step, and
    # the undo brings the elements and the selection back.
    count_before, _, _ = read_elements(session)
    begin = session.length()
    session.send("command editor.entity_delete")
    session.wait(
        message_pattern(DELETE_ELEMENTS_QUEUED_TOKEN, r".*\s+count=2"),
        30,
        "the element delete queued",
        begin,
    )
    session.wait(
        message_pattern(ELEMENTS_REMOVED_TOKEN, r".*\s+elements=2"),
        30,
        "the tool removed the elements",
        begin,
    )
    wait_selected(session, 0, "the selection cleared with the delete", begin)
    count_deleted, _, _ = read_elements(session)
    assert count_deleted == count_before - 2, (
        "the delete left {} elements, not {}".format(count_deleted, count_before - 2)
    )
    begin = session.length()
    undo_once(session, "the delete undone")
    wait_selected(session, 2, "the selection came back with the undo", begin)
    count_restored, _, _ = read_elements(session)
    assert count_restored == count_before, (
        "one undo left {} elements; the delete was not one step".format(count_restored)
    )
    print_ok("Delete removes the selected elements through the tool, in one step")

    # Step 7: the tool hears keys before the keybindings. `k` is consumed and
    # `i` answers with the insert the menu row means.
    begin = session.length()
    session.send("key k")
    session.wait(
        message_pattern(ELEMENT_KEY_CONSUMED_TOKEN, r'.*\s+key="k"'),
        30,
        "the tool took the key",
        begin,
    )
    count_before, _, _ = read_elements(session)
    begin = session.length()
    session.send("key i")
    session.wait(
        message_pattern(ELEMENT_KEY_COMMANDS_TOKEN, r'.*\s+key="i"\s+commands=1'),
        30,
        "the tool answered the key with commands",
        begin,
    )
    session.wait(
        message_pattern(ELEMENTS_INSERTED_TOKEN, r".*\s+elements=1"),
        30,
        "the key's insert ran",
        begin,
    )
    count_inserted, _, _ = read_elements(session)
    assert count_inserted == count_before + 1, (
        "the key left {} elements, not {}".format(count_inserted, count_before + 1)
    )
    undo_once(session, "the key's insert undone")
    print_ok("a tool hears a key before the keybindings, and can answer with commands")

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
    run_element_menu_steps(context.state)


AREA = StepArea(
    name="element_menus",
    order=5.7,
    run=_area_entry,
    needs=("gizmo_entity",),
    provides=(),
)
