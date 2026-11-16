"""The editor suite's component gizmo steps.

A component declares its own gizmo, and the engine draws it for every entity
that carries the component. These steps drive the whole contract through the
project's own witness - the `FollowTarget` component, whose gizmo draws a line
to the entity the reference names:

- the pass drew it, and the surfaces it draws on are the frame's editor views,
  never the game's own surface (`dump gizmos` names them);
- the toolbox's gizmos row switches the whole pass off and on;
- hiding the type through the preferences stops it, and the list survives a
  project reload;
- showing it again brings the drawing back.

The log vocabulary comes from `editor_log_tokens`; the tokens' field order is
the contract these patterns pin.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    GIZMO_PATTERN,
    GIZMO_TYPE_STATE_TOKEN,
    GIZMOS_PATTERN,
    GIZMOS_STATE_TOKEN,
    PROJECT_RELOAD_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    occurrence_count,
    print_ok,
    toolbox_click,
    wait_for_frame_advance,
    StepState,
)

# The witness component's type name, matched by its suffix: the fully-qualified
# name carries the project's module path, which the preference write needs
# exactly, so the dump's own answer is what the steps write back.
WITNESS_TYPE_SUFFIX = "ShowcaseReferenceComponent"


def message_pattern(token: str, fields: str) -> str:
    """The one-line pattern for a token followed by `fields`."""
    return log_layout.message_then_fields_pattern(re.escape(token), fields)


def read_gizmos(session):
    """One `dump gizmos`: the summary match and the rows keyed by type name."""
    dump = session.request("dump gizmos", GIZMOS_PATTERN, 30, "the gizmo dump")
    summary = re.search(GIZMOS_PATTERN, dump)
    assert summary, "the gizmo dump carried no summary: {}".format(dump[-500:])
    rows = {match.group(1): match for match in re.finditer(GIZMO_PATTERN, dump)}
    return summary, rows


def witness_row(rows, description):
    """The row of the project's witness gizmo, matched by its type suffix."""
    for type_name, row in rows.items():
        if type_name.endswith(WITNESS_TYPE_SUFFIX):
            return row
    raise AssertionError(
        "{}: no gizmo named ...{} in {}".format(
            description, WITNESS_TYPE_SUFFIX, sorted(rows)
        )
    )


def drawn_count(session, description):
    """The entity count the witness gizmo drew in the last frame."""
    _, rows = read_gizmos(session)
    return int(witness_row(rows, description).group(5))


def wait_for_visibility(
    session, type_name: str, visible: bool, description: str, begin: int
) -> None:
    """Waits for one gizmo type's visibility to reach `visible`."""
    session.wait(
        message_pattern(
            GIZMO_TYPE_STATE_TOKEN,
            r'\s+gizmo_type="?{}\"?\s+visible={}'.format(
                re.escape(type_name), "true" if visible else "false"
            ),
        ),
        60,
        description,
        begin,
    )


def run_component_gizmo_steps(state: StepState) -> None:
    """The steps through: a declared gizmo draws, hides and comes back."""
    session = state.session

    # Step 1: the pass drew the witness, and the surfaces it draws on are the
    # frame's editor views - never the game's surface, whose id is zero.
    summary, rows = read_gizmos(session)
    assert int(summary.group(1)) >= 1, "no component gizmo was registered at all"
    assert summary.group(3) == "true", "the gizmo pass started switched off"
    surfaces = [text for text in summary.group(4).split(",") if text]
    assert len(surfaces) >= 2, (
        "the gizmos reach {} editor view surface(s), not the two scene views".format(
            len(surfaces)
        )
    )
    assert "0" not in surfaces, (
        "the primary (game) surface is among the gizmo surfaces: {}".format(surfaces)
    )
    witness = witness_row(rows, "the first dump")
    witness_type = witness.group(1)
    assert witness.group(3) == "always", (
        "the witness declares when={}, not the default".format(witness.group(3))
    )
    assert witness.group(4) == "true", "the witness type started hidden"
    drew_before = int(witness.group(5))
    assert drew_before >= 1, (
        "the gizmo pass drew no witness entity, so it never ran: {}".format(
            witness.group(0)
        )
    )
    print_ok(
        "a component gizmo drew {} entit(ies) on {} editor views, and the game "
        "surface is not one of them".format(drew_before, len(surfaces))
    )

    # Step 2: the toolbox row switches the whole pass off, which the pass
    # reports by drawing nothing. Each anchor is taken before the click: the
    # click's own line and the switch's land in the same few milliseconds, so
    # an anchor taken after the click has already read past the switch.
    begin = session.length()
    toolbox_click(session, 0, "gizmos", "the toolbox gizmos row")
    session.wait(
        message_pattern(GIZMOS_STATE_TOKEN, r"\s+enabled=false"),
        60,
        "the gizmo pass switched off",
        begin,
    )
    assert (
        drawn_count(session, "the switched-off dump") == 0
    ), "a switched-off pass still drew"
    begin = session.length()
    toolbox_click(session, 0, "gizmos", "the toolbox gizmos row again")
    session.wait(
        message_pattern(GIZMOS_STATE_TOKEN, r"\s+enabled=true"),
        60,
        "the gizmo pass switched on",
        begin,
    )
    assert (
        drawn_count(session, "the switched-on dump") >= 1
    ), "a switched-on pass drew nothing"
    print_ok("the toolbox row switches the whole gizmo pass, and the drawing follows")

    # Step 3: hiding the type through the preferences stops it. The scripted
    # verb writes the same key the Preferences tab's switch writes, so the file
    # itself is the record the next session starts from.
    begin = session.length()
    session.send('edit preference gizmo.hidden_types ["{}"]'.format(witness_type))
    wait_for_visibility(session, witness_type, False, "the hide applied", begin)
    preferences_path = session.sandbox / "RustHybridEcs" / "preferences.yaml"
    assert preferences_path.is_file(), (
        "the preferences file is missing at {}".format(preferences_path)
    )
    saved = preferences_path.read_text(encoding="utf-8")
    assert witness_type in saved, (
        "the hidden type is not in the preferences file: {}".format(saved)
    )
    _, rows = read_gizmos(session)
    witness = witness_row(rows, "the hidden dump")
    assert witness.group(4) == "false", "the hidden type still reports visible"
    assert int(witness.group(5)) == 0, "a hidden type still drew"
    print_ok("hiding the type in the preferences stops its gizmo and reaches the file")

    # Step 4: a project reload re-registers the gizmo and keeps the switch, so
    # the reloaded generation still draws nothing for the hidden type.
    project_text = state.project_source.read_bytes().decode("utf-8")
    reloads = occurrence_count(session, re.escape(PROJECT_RELOAD_TOKEN))
    session.edit_source(
        state.project_source, project_text + "\n// egui suite: gizmo reload marker\n"
    )
    session.wait_occurrence(
        re.escape(PROJECT_RELOAD_TOKEN), reloads + 1, 240, "the project reload"
    )
    wait_for_frame_advance(session, "a frame after the reload")
    summary, rows = read_gizmos(session)
    assert int(summary.group(1)) >= 1, "the reloaded project registered no gizmo"
    assert summary.group(2) == "1", (
        "the hidden type was forgotten across the reload: {}".format(summary.group(0))
    )
    witness = witness_row(rows, "the reloaded dump")
    assert witness.group(4) == "false", "the reloaded generation shows the hidden type"
    assert int(witness.group(5)) == 0, "the reloaded generation drew the hidden type"
    print_ok("the hide survived the project reload")

    # Step 5: showing it again brings the drawing back, so the switch is a
    # switch and not a one-way edit.
    begin = session.length()
    session.send("edit preference gizmo.hidden_types []")
    wait_for_visibility(session, witness_type, True, "the show applied", begin)
    assert (
        drawn_count(session, "the shown dump") >= 1
    ), "showing the type again drew nothing"
    print_ok("showing the type again draws it")


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_component_gizmo_steps(context.state)


AREA = StepArea(
    name="component_gizmos",
    order=14.5,
    run=_area_entry,
    needs=(),
    provides=(),
)
