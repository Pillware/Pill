"""The editor suite's startup and hover steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re
import time

from core.editor_session import ScriptedCommandRejected
from core import log_layout
from core.editor_log_tokens import (
    CURSOR_ICON_TOKEN,
    EDITOR_WINDOW_CREATED_TOKEN,
    EGUI_SHELL_LOADED_TOKEN,
    OUTLINE_SET_TOKEN,
    PICK_ENTITY_PATTERN,
    PICK_NOTHING_PATTERN,
    PICK_PROBE_TIMEOUT,
    ROWS_PATTERN,
    SCENE_RECT_PATTERN,
    SCENE_SURFACE_TOKEN,
    SELECTION_CHANGED_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    visible_scene_rect,
)


def run_startup_and_hover_steps(state: StepState) -> None:
    """The steps through: hovering the splitter asked for the resize cursor."""
    begin = state.begin
    session = state.session

    # Step 1: startup, all the way to both tabs reporting rows.
    session.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 120, "startup")
    session.wait_for_match(log_layout.message_then_fields_pattern(re.escape(SCENE_SURFACE_TOKEN), r"\s+surface=1\s+detached=false"),
        120,
        "scene surface",
    )
    # Both scene viewports attach at startup: the Scene tab's surface
    # and the second viewport's.
    session.wait_occurrence(
        log_layout.message_then_fields_pattern(re.escape(SCENE_SURFACE_TOKEN), r"\s+surface=\d+\s+detached=false"),
        2,
        30,
        "second scene surface",
    )
    session.wait_for_match(re.escape(EGUI_SHELL_LOADED_TOKEN), 180, "shell load")
    rows = session.wait_for_match(ROWS_PATTERN, 180, "rows heartbeat")
    match = re.search(r"entities=(\d+)\s+systems=(\d+)", rows)
    assert match and int(match.group(1)) > 0, f"no entities in the rows: {rows}"
    print_ok(f"shell loaded, {rows}")

    # A misspelled verb must be refused at once: the session raises on the
    # channel's rejection line instead of the suite waiting out its
    # timeout for an answer that never comes.
    started = time.monotonic()
    refusal = None
    try:
        session.request("nonsense-verb", "an answer that never comes", 30, "a refusal")
    except ScriptedCommandRejected as error:
        refusal = error
    elapsed = time.monotonic() - started
    assert refusal is not None, "the channel accepted a verb it does not know"
    assert refusal.command == "nonsense-verb", f"refused {refusal.command!r}, not the verb"
    assert "unknown verb" in refusal.reason, f"unexpected reason: {refusal.reason}"
    assert elapsed < 15, f"the refusal took {elapsed:.1f}s"
    print_ok(f"a misspelled verb was refused in {elapsed:.1f}s: {refusal.reason}")

    # The same rule for a line whose grammar is right and whose target is
    # not: a handle no entity answers to is refused at once, so a suite that
    # mistypes an entity sees the refusal instead of a step that times out
    # looking like a slow editor.
    started = time.monotonic()
    refusal = None
    try:
        session.request(
            "select entity 9v999",
            "an answer that never comes",
            30,
            "a target refusal",
        )
    except ScriptedCommandRejected as error:
        refusal = error
    elapsed = time.monotonic() - started
    assert refusal is not None, "the channel resolved an entity that is not there"
    assert refusal.command == "select entity 9v999", f"refused {refusal.command!r}"
    assert "9v999" in refusal.reason, f"unexpected reason: {refusal.reason}"
    assert elapsed < 15, f"the refusal took {elapsed:.1f}s"
    print_ok(f"an unresolvable entity was refused in {elapsed:.1f}s: {refusal.reason}")

    # Step 2: scene selection - a click inside a scene viewport picks
    # through the renderer's pick contract, and the answer moves the
    # shared selection and publishes the outline; a corner click hits
    # nothing and clears both. The view's rectangle comes from the
    # frontend's own log, in the physical pixels the scripted clicks
    # use.
    scenes = session.request(
        "dump scenes", SCENE_RECT_PATTERN, 30, "the scene rects dumped"
    )
    scene_rect = visible_scene_rect(scenes)
    assert scene_rect, "no visible scene view rect in the log"
    scene_x, scene_y, scene_width, scene_height = scene_rect
    print_ok(
        f"scene view rect x={scene_x} y={scene_y} "
        f"w={scene_width} h={scene_height}"
    )

    # Probe outward from the viewport centre until one click answers with an
    # entity. The centre is where the middle entity's badge sits - a chip the
    # shell owns, which selects that entity and asks for no pick at all - so it
    # is the point just off the centre that reaches the mesh underneath it. A
    # cell nothing answers is the UI's, not a failure: the next one is tried.
    picked_entity = None
    for fraction_x, fraction_y in (
        (0.5, 0.5),
        (0.56, 0.5),
        (0.44, 0.5),
        (0.56, 0.55),
        (0.44, 0.55),
        (0.5, 0.56),
        (0.5, 0.44),
        (0.62, 0.5),
        (0.38, 0.5),
        (0.35, 0.4),
        (0.65, 0.6),
        (0.5, 0.25),
        (0.5, 0.75),
    ):
        click_x = scene_x + scene_width * fraction_x
        click_y = scene_y + scene_height * fraction_y
        begin = session.length()
        session.send(f"click {click_x:.0f} {click_y:.0f}")
        try:
            found, _ = session.wait_either(
                [PICK_ENTITY_PATTERN, PICK_NOTHING_PATTERN], PICK_PROBE_TIMEOUT, begin
            )
        except TimeoutError:
            # Nothing answered: the shell took the click - a badge, a chip or the
            # floating toolbox - and no scene pick was ever asked.
            print_ok(f"click at ({click_x:.0f}, {click_y:.0f}) was the UI's, next")
            continue
        if found == PICK_ENTITY_PATTERN:
            match = re.search(PICK_ENTITY_PATTERN, session.read_since(begin))
            picked_entity = match.group(1)
            print_ok(
                f"click at ({click_x:.0f}, {click_y:.0f}) picked "
                f"{picked_entity}"
            )
            break
        print_ok(f"click at ({click_x:.0f}, {click_y:.0f}) hit nothing, next")
    assert picked_entity, "no grid point hit an entity in the scene view"
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), rf"\s+entity={re.escape(picked_entity)}"),
        60,
        "the pick moved the shared selection",
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(OUTLINE_SET_TOKEN), rf"\s+entity={re.escape(picked_entity)}"),
        60,
        "the outline was published for it",
    )

    # The dock's splitter: the grab band beside a
    # panel's edge takes the drag, so a press a few pixels outside the
    # one-pixel line still moves the split - and the scene window, which
    # the shell places at the dock's own rectangle, moves with it. The
    # band is the line plus eight points of grab width on each side, so a
    # press four points above the scene's bottom edge is inside it and
    # was outside the four points the dock shipped with.
    badge_begin = session.length()
    splitter_x = scene_x + scene_width * 0.5
    splitter_y = scene_y + scene_height - 4.0
    # A hover on the band asks the dock for the resize cursor, which the
    # executable maps and logs on change - the half of the splitter's
    # story a drag cannot show.
    hover_begin = session.length()
    session.send(f"hover ui {splitter_x:.0f} {splitter_y:.0f}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(CURSOR_ICON_TOKEN), r'\s+icon="ResizeVertical"'),
        30,
        "the resize cursor over the splitter",
        hover_begin,
    )
    print_ok("hovering the splitter asked for the resize cursor")

    state.badge_begin = badge_begin
    state.begin = begin
    state.picked_entity = picked_entity
    state.scene_height = scene_height
    state.scene_rect = scene_rect
    state.scene_x = scene_x
    state.scene_y = scene_y
    state.splitter_x = splitter_x
    state.splitter_y = splitter_y


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_startup_and_hover_steps(context.state)


AREA = StepArea(
    name="startup_and_hover",
    order=1,
    run=_area_entry,
    needs=(),
    provides=("badge_begin", "picked_entity", "scene_height", "scene_rect", "scene_x", "scene_y", "splitter_x", "splitter_y",),
)
