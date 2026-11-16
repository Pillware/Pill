"""The editor suite's tool extension steps.

An extension adds to a tool another crate owns - menu rows, element kinds,
frames and keys - without replacing the tool and without removing any row the
tool itself offers. The fixture tool carries two extensions for these steps:
`fixture notes`, which adds one row for `point` elements and is attached only
to an entity carrying `TagAlphaComponent`, and `fixture marks`, which adds one
row for every kind on every entity. Both bind the chord `n`.

The steps show the three rules end to end: an extension is attached only while
its base tool is active and the entity carries the component it requires; the
merged menu keeps the base's rows first and puts the extensions after them in
`order`; and a chord two extensions bind is answered by the lower `order` and
reported once, not once per press.

The log vocabulary comes from `editor_log_tokens`; the tokens' field order is
the contract these patterns pin.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    ELEMENT_KEY_CONSUMED_TOKEN,
    ELEMENT_MENU_QUEUED_TOKEN,
    ELEMENT_MENU_RAN_TOKEN,
    ELEMENT_MENU_ROW_TOKEN,
    ELEMENT_SELECTION_TOKEN,
    ELEMENTS_DUMP_TOKEN,
    ENTITIES_DUMP_TOKEN,
    ENTITY_ROW_TOKEN,
    SELECTION_CHANGED_TOKEN,
    SELECTION_DUMP_TOKEN,
    TOOL_ACTIVATED_TOKEN,
    TOOL_DUMP_TOKEN,
    TOOL_EXTENSION_KEY_CONFLICT_TOKEN,
    TOOL_EXTENSION_ROW_TOKEN,
    TOOL_EXTENSIONS_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import print_ok, StepState

# The fixture's two extensions, as `dump tools` names them, and the component
# the first one is attached by.
NOTES_EXTENSION = "fixture notes"
MARKS_EXTENSION = "fixture marks"
NOTES_COMPONENT = "TagAlphaComponent"
SHARED_CHORD = "n"

# The fixture tool's own menu rows, and the two its extensions add.
INSERT_ROW = "insert"
RESET_ROW = "reset"
NOTE_ROW = "note"
MARK_ROW = "mark"


def message_pattern(token: str, fields: str) -> str:
    """The one-line pattern for a token followed by `fields`."""
    return log_layout.message_then_fields_pattern(re.escape(token), fields)


ENTITIES_DUMP_PATTERN = message_pattern(
    ENTITIES_DUMP_TOKEN, r"\s+count=(\d+)"
)
ENTITY_ROW_PATTERN = message_pattern(
    ENTITY_ROW_TOKEN, r'\s+display="([^"]+)"\s+components="([^"]*)"'
)
TOOLS_DUMP_PATTERN = message_pattern(
    TOOL_DUMP_TOKEN, r'\s+count=(\d+)\s+active="([^"]*)"\s+extensions=(\d+)'
)
EXTENSION_ROW_PATTERN = message_pattern(
    TOOL_EXTENSION_ROW_TOKEN,
    r'\s+name="([^"]+)"\s+base="([^"]+)"\s+order=(-?\d+)'
    r'\s+requires="([^"]*)"\s+kinds="([^"]*)"\s+keys="([^"]*)"\s+attached=(true|false)',
)
ELEMENTS_DUMP_PATTERN = message_pattern(
    ELEMENTS_DUMP_TOKEN,
    r'\s+entity=\S+\s+count=(\d+)\s+kinds=(\d+)\s+kind="([^"]*)"\s+selected=(\d+)',
)
ELEMENT_MENU_ROW_PATTERN = message_pattern(
    ELEMENT_MENU_ROW_TOKEN, r'\s+item="([^"]+)"\s+label="([^"]+)"'
)
REGISTERED_PATTERN = message_pattern(
    TOOL_EXTENSIONS_TOKEN, r"\s+enabled=(true|false)\s+extensions=(\d+)"
)
CONFLICT_PATTERN = message_pattern(
    TOOL_EXTENSION_KEY_CONFLICT_TOKEN,
    r'\s+base="([^"]+)"\s+chord="([^"]+)"\s+extensions="([^"]*)"\s+winner="([^"]*)"',
)
TOOL_ACTIVATED_PATTERN = message_pattern(TOOL_ACTIVATED_TOKEN, r'\s+tool="([^"]+)"')
ELEMENT_SELECTED_PATTERN = message_pattern(
    ELEMENT_SELECTION_TOKEN, r'\s+kind="point"\s+selected=(\d+)'
)
SELECTION_DUMP_PATTERN = message_pattern(
    SELECTION_DUMP_TOKEN,
    r'\s+count=(\d+)\s+asset="[^"]*"\s+primary="([^"]*)"\s+elements=(\d+)',
)
KEY_CONSUMED_PATTERN = message_pattern(
    ELEMENT_KEY_CONSUMED_TOKEN, r'\s+entity=\S+\s+kind="point"\s+key="n"'
)


def menu_queued_pattern(item: str) -> str:
    """The line one `element menu <item>` queues."""
    return message_pattern(
        ELEMENT_MENU_QUEUED_TOKEN, r'\s+entity=\S+\s+item="{}"'.format(re.escape(item))
    )


def menu_ran_pattern(item: str) -> str:
    """The line the drain logs once the row's own answer is known."""
    return message_pattern(
        ELEMENT_MENU_RAN_TOKEN,
        r'\s+item="{}"\s+commands=(\d+)'.format(re.escape(item)),
    )


def select_element(session, element: int, description: str) -> None:
    """Selects one element of the selected entity, through the tool's click."""
    begin = session.length()
    session.send("select element {}".format(element))
    session.wait(ELEMENT_SELECTED_PATTERN, 30, description, begin)


def select_entity(session, display: str, description: str) -> None:
    """Selects one entity, waiting for the change only when there is one.

    Selecting the entity the editor already has selected changes nothing and
    logs nothing, so the wait would outlast its timeout.
    """
    if read_primary(session) == display:
        return
    begin = session.length()
    session.send("select entity {}".format(display))
    session.wait(re.escape(SELECTION_CHANGED_TOKEN), 30, description, begin)


def read_primary(session) -> str:
    """The display handle of the entity the selection is primarily on."""
    dump = session.request(
        "dump selection", SELECTION_DUMP_PATTERN, 30, "the selection dump"
    )
    head = re.search(SELECTION_DUMP_PATTERN, dump)
    assert head, "the selection dump carried no counts: {}".format(dump[-500:])
    return head.group(2)


def read_entities(session: "tuple") -> "list":
    """One `dump entities`: every row as `(display, component label)`."""
    dump = session.request(
        "dump entities", ENTITIES_DUMP_PATTERN, 30, "the entity list"
    )
    rows = [
        (match.group(1), match.group(2))
        for match in re.finditer(ENTITY_ROW_PATTERN, dump)
    ]
    assert rows, "the entity dump carried no rows: {}".format(dump[-500:])
    return rows


def read_tools(session: "tuple") -> "tuple":
    """One `dump tools`: `(active, extension count, extension rows)`.

    Each row is `(name, base, order, requires, kinds, keys, attached)`, in the
    chain's order, which is what the order rule is read from.
    """
    dump = session.request("dump tools", TOOLS_DUMP_PATTERN, 30, "the tool dump")
    head = re.search(TOOLS_DUMP_PATTERN, dump)
    assert head, "the tool dump carried no counts: {}".format(dump[-500:])
    rows = [
        (
            match.group(1),
            match.group(2),
            int(match.group(3)),
            match.group(4),
            match.group(5),
            match.group(6),
            match.group(7) == "true",
        )
        for match in re.finditer(EXTENSION_ROW_PATTERN, dump)
    ]
    return head.group(2), int(head.group(3)), rows


def read_menu(session: "tuple") -> "tuple":
    """One `dump elements`: `(kind, selected count, menu row ids in order)`."""
    dump = session.request(
        "dump elements", ELEMENTS_DUMP_PATTERN, 30, "the element dump"
    )
    head = re.search(ELEMENTS_DUMP_PATTERN, dump)
    assert head, "the element dump carried no counts: {}".format(dump[-500:])
    menu = [match.group(1) for match in re.finditer(ELEMENT_MENU_ROW_PATTERN, dump)]
    return head.group(3), int(head.group(4)), menu


def count_conflicts(session) -> int:
    """How often the shared chord has been reported over the whole run."""
    return session.text().count(TOOL_EXTENSION_KEY_CONFLICT_TOKEN)


def run_tool_extension_steps(state: StepState) -> None:
    """The steps through: two extensions of one tool, merged and then dropped."""
    session = state.session

    # Step 1: the two subjects. The scene project's rotating models carry
    # `TagAlphaComponent` and its camera does not, which is the difference the
    # attach rule turns on; both have a transform, so the fixture reports its
    # points for either.
    rows = read_entities(session)
    carrier = next(
        display for display, components in rows if NOTES_COMPONENT in components
    )
    plain = next(
        display for display, components in rows if NOTES_COMPONENT not in components
    )
    print_ok(
        "the carrier is {} and the entity without the component is {}".format(
            carrier, plain
        )
    )

    # Step 2: registering the extensions is one switch. The chord both bind is
    # reported once, naming the extension that answers it: the one with the
    # lower `order`, whichever crate registered first.
    begin = session.length()
    session.send("extensions on")
    session.wait(
        message_pattern(TOOL_EXTENSIONS_TOKEN, r"\s+enabled=true\s+extensions=2"),
        30,
        "the fixture's extensions registered",
        begin,
    )
    session.wait(CONFLICT_PATTERN, 30, "the shared chord reported once", begin)
    conflict = re.search(CONFLICT_PATTERN, session.read_since(begin))
    assert conflict, "the conflict line was not found after its wait"
    assert conflict.group(1) == "fixture", (
        "the conflict line names base '{}', not the tool both extend".format(
            conflict.group(1)
        )
    )
    assert conflict.group(2) == SHARED_CHORD, "the conflict names chord '{}'".format(
        conflict.group(2)
    )
    assert conflict.group(3).split(",") == [NOTES_EXTENSION, MARKS_EXTENSION], (
        "the conflict lists {}; the chain is not in order".format(conflict.group(3))
    )
    assert conflict.group(4) == NOTES_EXTENSION, (
        "the conflict names '{}' as the winner, not the lower-ordered extension".format(
            conflict.group(4)
        )
    )
    assert count_conflicts(session) == 1, (
        "the shared chord was reported {} times, not once".format(
            count_conflicts(session)
        )
    )
    print_ok("two extensions of one tool on one chord: reported once, resolved by order")

    # Step 3: both extensions attach to the carrier of the required component.
    select_entity(session, carrier, "the carrier selected")
    begin = session.length()
    session.send("tool fixture")
    session.wait(TOOL_ACTIVATED_PATTERN, 30, "the fixture tool activated", begin)
    active, extensions, extension_rows = read_tools(session)
    assert active == "fixture", "the active tool is '{}'".format(active)
    assert extensions == 2, "the registry holds {} extensions".format(extensions)
    assert [row[0] for row in extension_rows] == [NOTES_EXTENSION, MARKS_EXTENSION], (
        "the chain is {}; it is not sorted by order".format(
            [row[0] for row in extension_rows]
        )
    )
    notes = extension_rows[0]
    assert notes[1] == "fixture", "the notes extension extends '{}'".format(notes[1])
    assert notes[2] == 10, "the notes extension sits at order {}".format(notes[2])
    assert notes[3] == NOTES_COMPONENT, (
        "the notes extension requires '{}'".format(notes[3])
    )
    assert notes[4] == "point", "the notes extension reports kinds '{}'".format(notes[4])
    assert notes[6], "the notes extension is not attached to the carrier"
    marks = extension_rows[1]
    assert marks[2] == 30, "the marks extension sits at order {}".format(marks[2])
    assert marks[3] == "none", "the marks extension requires '{}'".format(marks[3])
    assert marks[6], "the marks extension is not attached to the carrier"
    assert notes[5] == SHARED_CHORD and marks[5] == SHARED_CHORD, (
        "the extensions bind {} and {}".format(notes[5], marks[5])
    )
    print_ok("both extensions are attached to the entity that carries the component")

    # Step 4: the merged menu. With one point selected the base offers both its
    # rows - the second only with a selection - and the extensions follow in
    # order, so neither of them can push a base row out.
    select_element(session, 0, "one point selected on the carrier")
    kind, selected, menu = read_menu(session)
    assert kind == "point", "the element dump reports kind '{}'".format(kind)
    assert selected == 1, "{} elements are selected, not one".format(selected)
    assert menu == [INSERT_ROW, RESET_ROW, NOTE_ROW, MARK_ROW], (
        "the merged menu is {}; the base's rows come first and the extensions "
        "follow in order".format(menu)
    )
    print_ok("the merged menu keeps the base's rows first and appends both extensions")

    # Step 5: the shared chord. Both extensions are attached here, and each
    # press is answered once - a second press must not report the clash again.
    for press in ("first", "second"):
        begin = session.length()
        session.send("key {}".format(SHARED_CHORD))
        session.wait(
            KEY_CONSUMED_PATTERN,
            30,
            "the {} press consumed by the chain".format(press),
            begin,
        )
    assert count_conflicts(session) == 1, (
        "the second press reported the clash again: {} lines".format(
            count_conflicts(session)
        )
    )
    print_ok("the shared chord is consumed on every press and reported once")

    # Step 6: an extension's row runs through the merged run, which asks the
    # base first and the extensions only when it refuses. Both rows answer with
    # no commands, so the history takes no step for either.
    for row in (NOTE_ROW, MARK_ROW):
        begin = session.length()
        session.send("element menu {}".format(row))
        session.wait(
            menu_queued_pattern(row), 30, "the {} row queued".format(row), begin
        )
        ran = session.wait_for_match(
            menu_ran_pattern(row), 30, "the {} row ran".format(row), begin
        )
        answered = re.search(r"commands=(\d+)", ran)
        assert answered and answered.group(1) == "0", (
            "the {} row answered '{}' commands, not none".format(row, ran)
        )
    print_ok("both extensions' rows reach the menu run and answer for themselves")

    # Step 7: the same tool, the same row, an entity without the component. The
    # notes extension is not attached to it, so its row is gone while the marks
    # extension - which requires nothing - stays.
    select_entity(
        session, plain, "the entity without the component selected"
    )
    select_element(session, 0, "one point selected on the entity without the component")
    active, extensions, extension_rows = read_tools(session)
    attached = {row[0]: row[6] for row in extension_rows}
    assert attached.get(NOTES_EXTENSION) is False, (
        "the notes extension is attached to an entity it requires a component of"
    )
    assert attached.get(MARKS_EXTENSION) is True, (
        "the marks extension is not attached to an entity that requires nothing"
    )
    kind, selected, menu = read_menu(session)
    assert kind == "point", "the element dump reports kind '{}'".format(kind)
    assert menu == [INSERT_ROW, RESET_ROW, MARK_ROW], (
        "the menu on the entity without the component is {}; the notes row "
        "belongs to the carriers alone".format(menu)
    )
    print_ok("an extension that requires a component is attached to that entity alone")

    # Step 8: dropping the extensions leaves the tool exactly as it was. The
    # carrier comes back so the menu is read on the entity that had both rows.
    begin = session.length()
    session.send("extensions off")
    session.wait(
        message_pattern(TOOL_EXTENSIONS_TOKEN, r"\s+enabled=false\s+extensions=0"),
        30,
        "the extensions dropped",
        begin,
    )
    select_entity(session, carrier, "the carrier selected again")
    select_element(session, 0, "one point selected on the carrier again")
    active, extensions, extension_rows = read_tools(session)
    assert active == "fixture", "the fixture tool left with its extensions"
    assert extensions == 0, "{} extensions survived the drop".format(extensions)
    assert extension_rows == [], (
        "the dump still lists {} extension rows".format(len(extension_rows))
    )
    kind, selected, menu = read_menu(session)
    assert menu == [INSERT_ROW, RESET_ROW], (
        "the tool's own menu after the drop is {}, not its two rows".format(menu)
    )
    print_ok("dropping the extensions leaves the tool's own menu untouched")

    # Step 9: the neutral tool, so the areas after this one keep the plain
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
    run_tool_extension_steps(context.state)


AREA = StepArea(
    name="tool_extensions",
    order=5.8,
    run=_area_entry,
    needs=("gizmo_entity",),
    provides=(),
)
