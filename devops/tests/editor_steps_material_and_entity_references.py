"""The editor suite's material and entity references steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re
import time

from core import log_layout
from core.editor_log_tokens import (
    DELETE_ENTITY_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    ENTITIES_DUMP_TOKEN,
    ENTITY_REFERENCE_ROW_RECT_TOKEN,
    ENTITY_REFERENCE_ROW_TOKEN,
    ENTITY_REFERENCE_SET_TOKEN,
    ENTITY_ROW_TOKEN,
    HIERARCHY_ROW_RECT_TOKEN,
    HISTORY_PUSH_TOKEN,
    HISTORY_UNDO_TOKEN,
    INSPECTOR_SECTION_RECT_PATTERN,
    INSPECTOR_SECTION_TOKEN,
    NEW_ENTITY_TOKEN,
    REFERENCE_ROW_RECT_TOKEN,
    REFERENCE_SET_TOKEN,
    REFERENCE_TILE_RECT_TOKEN,
    SELECTION_CHANGED_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    asset_row_pattern,
    inspector_section_rect,
    last_log_match,
    logged_rect_bounds,
    logged_rect_centre,
    occurrence_count,
    rect_center,
    wait_applied,
    wait_for_frame_advance,
)


def run_material_and_entity_references_steps(state: StepState) -> None:
    """The steps through: the prior selection is back."""
    session = state.session


    # The material pick: the staged file is indexed but loaded by nothing,
    # so assigning it is what loads it.
    staged_material = "materials/s16_reference.material"
    begin = session.length()
    session.send(
        "edit set {} MeshRendererComponent material {}".format(
            state.mesh_entity, staged_material
        )
    )
    session.wait(
        re.escape(REFERENCE_SET_TOKEN)
        + r'(?=[^\n]*field="material")[^\n]*asset="'
        + re.escape(staged_material)
        + r'"',
        60,
        "the material pick",
        begin,
    )
    session.wait(
        asset_row_pattern(state.mesh_entity, "material", staged_material),
        30,
        "the row names the picked material",
        begin,
    )
    print_ok("picking the staged, unloaded material loaded and assigned it")

    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the material pick's undo", begin)
    session.wait(
        asset_row_pattern(state.mesh_entity, "material", state.original_material),
        30,
        "the row names the original material again",
        begin,
    )
    print_ok("undo restored the original material")

    # The drop: the material's tile dragged from the Assets tab onto the
    # material row. The Assets tab is brought to the front first: the grid
    # only draws - and so only logs its tiles - while it is visible.
    begin = session.length()
    session.send("tab assets")
    session.send("browser materials")
    tile_pattern = (
        re.escape(REFERENCE_TILE_RECT_TOKEN)
        + r'(?=[^\n]*path="'
        + re.escape(staged_material)
        + r'")[^\n]*'
    )
    session.wait(tile_pattern, 30, "the material tile's rectangle", begin)
    # The Inspector's body is shorter than its rows, and the mesh renderer
    # sits below others: the sections above it are collapsed, topmost
    # first, as a person collapsing a header does, until its material row's
    # rectangle - logged only while the row's centre is visible - comes on
    # screen. Which sections those are is read from the log rather than
    # listed here, so a component the project adds moves nothing.
    row_rect_pattern = (
        re.escape(REFERENCE_ROW_RECT_TOKEN)
        + r'(?=[^\n]*MeshRendererComponent\.material")[^\n]*'
    )
    visible_from = session.length()
    collapsed_sections = []
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        if re.search(row_rect_pattern, session.read_since(visible_from)):
            break
        headers = {}
        # Only the sections the panel drew for the entity it shows now:
        # the shell forgets a previous subject's rectangles, so everything
        # after the last selection change is this entity's.
        log_text = session.text()
        last_change = log_text.rfind(SELECTION_CHANGED_TOKEN)
        scoped = log_text[last_change:] if last_change >= 0 else log_text
        for match in re.finditer(INSPECTOR_SECTION_RECT_PATTERN, scoped):
            headers[match.group(1)] = match
        above = sorted(
            (
                (name, float(logged.group(3)))
                for name, logged in headers.items()
                if name != "pill_rendering::MeshRendererComponent"
                and name not in collapsed_sections
            ),
            key=lambda entry: entry[1],
        )
        assert above, (
            "every Inspector section is collapsed and the material row's "
            "rectangle never came on screen"
        )
        component = above[0][0]
        begin = session.length()
        x, y, width, height = inspector_section_rect(session, component)
        session.send("click {:.0f} {:.0f}".format(x + 12.0, y + 6.0))
        session.wait(
            re.escape(INSPECTOR_SECTION_TOKEN)
            + r'(?=[^\n]*component="'
            + re.escape(component)
            + r'")[^\n]*collapsed=true',
            30,
            "the {} section collapsed".format(component.rsplit("::", 1)[-1]),
            begin,
        )
        collapsed_sections.append(component)
    session.wait(row_rect_pattern, 30, "the material row's rectangle", visible_from)
    row_rect = logged_rect_bounds(
        last_log_match(session, row_rect_pattern, visible_from).group(0)
    )
    print_ok("the material row came on screen")
    tile_x, tile_y = logged_rect_centre(last_log_match(session, tile_pattern).group(0))
    row_x, row_y = rect_center(row_rect)
    begin = session.length()
    session.send(f"drag {tile_x:.0f} {tile_y:.0f} {row_x:.0f} {row_y:.0f}")
    session.wait(
        re.escape(REFERENCE_SET_TOKEN)
        + r'(?=[^\n]*field="material")[^\n]*asset="'
        + re.escape(staged_material)
        + r'"',
        60,
        "the dropped material",
        begin,
    )
    session.wait(
        asset_row_pattern(state.mesh_entity, "material", staged_material),
        30,
        "the row names the dropped material",
        begin,
    )
    print_ok("dropping a material tile onto the row assigned it")

    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the drop's undo", begin)
    session.wait(
        asset_row_pattern(state.mesh_entity, "material", state.original_material),
        30,
        "the original material after the undo",
        begin,
    )
    # The sections are opened again - each header is on screen when its
    # turn comes - so the steps after this see the Inspector as they did.
    for component in reversed(collapsed_sections):
        begin = session.length()
        x, y, width, height = inspector_section_rect(session, component)
        session.send("click {:.0f} {:.0f}".format(x + 12.0, y + 6.0))
        session.wait(
            re.escape(INSPECTOR_SECTION_TOKEN)
            + r'(?=[^\n]*component="'
            + re.escape(component)
            + r'")[^\n]*collapsed=false',
            30,
            "the {} section expanded".format(component.rsplit("::", 1)[-1]),
            begin,
        )
    print_ok("an asset reference picked, undid, cleared and took a drop")

    # Step 3a8c: entity reference rows. A field that names
    # another entity draws a row whose text is that entity's Hierarchy
    # label. A scripted pick - the same recorded step the row's picker
    # queues - re-points it and undoes, `none` clears it, a Hierarchy row
    # dragged onto it assigns that entity, and an armed row takes the next
    # scene press as its reference instead of the press selecting. Every
    # way in is one history step, so the row ends where it started.
    reference_dump = session.request(
        "dump entities", re.escape(ENTITIES_DUMP_TOKEN), 30, "the entity list"
    )
    reference_rows = [
        match.group(1)
        for match in re.finditer(
            log_layout.message_then_fields_pattern(re.escape(ENTITY_ROW_TOKEN), r'\s+display="([^"]+)"\s+components="([^"]*)"'),
            reference_dump,
        )
        if "ShowcaseReferenceComponent" in match.group(2)
    ]
    assert reference_rows, (
        "no entity carries a showcase reference: {}".format(reference_dump[-800:])
    )
    listed_entities = [
        match.group(1)
        for match in re.finditer(
            log_layout.message_then_fields_pattern(re.escape(ENTITY_ROW_TOKEN), r'\s+display="([^"]+)"'), reference_dump
        )
    ]
    # A carrier no earlier step selected: a row logs its text on its first
    # draw and on every change, so a freshly selected one always logs.
    holder = next(
        (
            display
            for display in reference_rows
            if display not in (state.mesh_entity, state.prior_entity, state.picked_entity)
        ),
        None,
    )
    assert holder, "every reference carrier was selected by an earlier step"

    def entity_row_capture() -> str:
        """The holder's reference row, capturing the text it names."""
        return (
            re.escape(ENTITY_REFERENCE_ROW_TOKEN)
            + r"(?=[^\n]*entity="
            + re.escape(holder)
            + r"\b)[^\n]*reference=\"([^\"]*)\""
        )

    def entity_row_pattern(reference: str) -> str:
        """The holder's reference row, naming `reference`."""
        return (
            re.escape(ENTITY_REFERENCE_ROW_TOKEN)
            + r"(?=[^\n]*entity="
            + re.escape(holder)
            + r"\b)[^\n]*reference=\""
            + re.escape(reference)
            + r'"'
        )

    begin = session.length()
    session.send("select entity {}".format(holder))
    session.wait(entity_row_capture(), 30, "the reference row", begin)
    authored_target = last_log_match(session, entity_row_capture(), begin).group(1)
    assert authored_target not in ("None", "(missing)"), (
        "the reference row starts unset: {}".format(authored_target)
    )
    print_ok("the reference row names {}".format(authored_target))

    # The picker's step: by label, recorded, and undone back.
    picked_target = next(
        display
        for display in listed_entities
        if display not in (authored_target, holder)
    )
    begin = session.length()
    session.send(
        "edit set {} ShowcaseReferenceComponent follow {}".format(
            holder, picked_target
        )
    )
    session.wait(
        re.escape(ENTITY_REFERENCE_SET_TOKEN)
        + r'(?=[^\n]*field="follow")[^\n]*reference="'
        + re.escape(picked_target)
        + r'"',
        60,
        "the reference pick",
        begin,
    )
    session.wait(
        entity_row_pattern(picked_target),
        30,
        "the row names the picked entity",
        begin,
    )
    print_ok("a scripted pick pointed the reference at {}".format(picked_target))
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the pick's undo", begin)
    session.wait(
        entity_row_pattern(authored_target),
        30,
        "the authored reference after the undo",
        begin,
    )
    print_ok("undoing the pick put the authored reference back")

    # `none` clears the field to the null handle, and undoes back.
    begin = session.length()
    session.send("edit set {} ShowcaseReferenceComponent follow none".format(holder))
    session.wait(
        re.escape(ENTITY_REFERENCE_SET_TOKEN)
        + r'(?=[^\n]*field="follow")[^\n]*reference="none"',
        60,
        "the reference cleared",
        begin,
    )
    session.wait(entity_row_pattern("None"), 30, "the row reads unset", begin)
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the clear's undo", begin)
    session.wait(
        entity_row_pattern(authored_target),
        30,
        "the authored reference after the clear's undo",
        begin,
    )
    print_ok("clearing the reference undoes back to what it named")

    # The drop: the entities tab's rows are the drag sources, so the tab is
    # opened (the Inspector stays beside it) and both ends' rectangles come
    # from the logs. The rows log their rectangles the first time they are
    # drawn, and the tab body keeps drawing while another tab is in front,
    # so the dragged row's rectangle is already there.
    session.send("tab entities")
    hierarchy_row_pattern = (
        log_layout.message_then_fields_pattern(re.escape(HIERARCHY_ROW_RECT_TOKEN), r"\s+entity=")
        + re.escape(picked_target)
        + r"\b[^\n]*"
    )
    session.wait(hierarchy_row_pattern, 60, "the hierarchy row's rectangle", 0)
    reference_row_rect_pattern = (
        re.escape(ENTITY_REFERENCE_ROW_RECT_TOKEN)
        + r'(?=[^\n]*row="'
        + re.escape(holder)
        + r"\.)"
        + r"[^\n]*"
    )
    # The reference section sits below others: they are collapsed, topmost
    # first, until its row's rectangle - logged only while the row's centre
    # is visible, which is the box a drop lands in - comes on screen. The
    # row kept drawing since the holder was first selected, so a rectangle
    # it already logged counts.
    collapsed_for_reference = []
    begin = session.length()
    session.send("select entity {}".format(holder))
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        if re.search(reference_row_rect_pattern, session.text()):
            break
        headers = {}
        # Only this entity's sections: the shell forgets a previous
        # subject's rectangles, so everything after the last selection
        # change belongs to the entity it shows now.
        log_text = session.text()
        last_change = log_text.rfind(SELECTION_CHANGED_TOKEN)
        scoped = log_text[last_change:] if last_change >= 0 else log_text
        for match in re.finditer(INSPECTOR_SECTION_RECT_PATTERN, scoped):
            headers[match.group(1)] = match
        above = sorted(
            (
                (name, float(logged.group(3)))
                for name, logged in headers.items()
                if not name.endswith("::ShowcaseReferenceComponent")
                and name not in collapsed_for_reference
            ),
            key=lambda entry: entry[1],
        )
        assert above, "every other Inspector section is collapsed and the row stayed off screen"
        component = above[0][0]
        section_begin = session.length()
        x, y, width, height = inspector_section_rect(session, component)
        session.send("click {:.0f} {:.0f}".format(x + 12.0, y + 6.0))
        session.wait(
            re.escape(INSPECTOR_SECTION_TOKEN)
            + r'(?=[^\n]*component="'
            + re.escape(component)
            + r'")[^\n]*collapsed=true',
            30,
            "the {} section collapsed".format(component.rsplit("::", 1)[-1]),
            section_begin,
        )
        collapsed_for_reference.append(component)
    session.wait(
        reference_row_rect_pattern,
        30,
        "the reference row's rectangle",
        0,
    )
    drop_from = logged_rect_centre(
        last_log_match(session, hierarchy_row_pattern).group(0)
    )
    drop_to = logged_rect_centre(
        last_log_match(session, reference_row_rect_pattern).group(0)
    )
    begin = session.length()
    session.send(f"drag {drop_from[0]:.0f} {drop_from[1]:.0f} {drop_to[0]:.0f} {drop_to[1]:.0f}")
    session.wait(
        re.escape(ENTITY_REFERENCE_SET_TOKEN)
        + r'(?=[^\n]*field="follow")[^\n]*reference="'
        + re.escape(picked_target)
        + r'"',
        60,
        "the dropped entity",
        begin,
    )
    session.wait(
        entity_row_pattern(picked_target),
        30,
        "the row names the dropped entity",
        begin,
    )
    print_ok("dropping a Hierarchy row onto the reference assigned it")
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the drop's undo", begin)
    session.wait(
        entity_row_pattern(authored_target),
        30,
        "the authored reference after the drop's undo",
        begin,
    )

    # A reference to a destroyed entity reads as missing: a throwaway
    # entity is created, pointed at, deleted and the row re-read, so the
    # scene's own entities are left alone.
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.entity_create")
    session.wait(re.escape(NEW_ENTITY_TOKEN), 30, "the throwaway entity queued", begin)
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="new entity"'),
        30,
        "the throwaway recorded",
        begin,
    )
    wait_applied(session, applied_before, "the throwaway applied")
    # The created entity is the one the list did not have: the selection
    # the create leaves behind is not read, because a later `select entity`
    # in this step would then name the wrong entity.
    after_create = session.request(
        "dump entities", re.escape(ENTITIES_DUMP_TOKEN), 30, "the entity list again"
    )
    created = [
        match.group(1)
        for match in re.finditer(
            log_layout.message_then_fields_pattern(re.escape(ENTITY_ROW_TOKEN), r'\s+display="([^"]+)"'), after_create
        )
        if match.group(1) not in listed_entities
    ]
    assert len(created) == 1, "the create added {} entities".format(len(created))
    throwaway = created[0]
    assert throwaway != holder, "the throwaway is the holder itself"
    begin = session.length()
    session.send(
        "edit set {} ShowcaseReferenceComponent follow {}".format(holder, throwaway)
    )
    session.wait(
        re.escape(ENTITY_REFERENCE_SET_TOKEN)
        + r'(?=[^\n]*field="follow")[^\n]*reference="'
        + re.escape(throwaway)
        + r'"',
        60,
        "the reference pointed at the throwaway",
        begin,
    )
    # The selection has to be on the throwaway before the delete is queued:
    # the delete command reads the selection when it is built.
    begin = session.length()
    session.send("select entity {}".format(throwaway))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), r"\s+entity=")
        + re.escape(throwaway)
        + r"\b",
        30,
        "the throwaway selected",
        begin,
    )
    begin = session.length()
    session.send("command editor.entity_delete")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(DELETE_ENTITY_TOKEN), r"\s+entity=") + re.escape(throwaway) + r"\b",
        30,
        "the throwaway deleted",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="delete entity"'),
        30,
        "the delete recorded",
        begin,
    )
    begin = session.length()
    session.send("select entity {}".format(holder))
    session.wait(
        entity_row_pattern("(missing)"),
        60,
        "the row reads missing",
        begin,
    )
    print_ok("a reference to a destroyed entity reads as missing")

    # And back to the entity the scene authored the reference with.
    begin = session.length()
    session.send(
        "edit set {} ShowcaseReferenceComponent follow {}".format(
            holder, authored_target
        )
    )
    session.wait(
        entity_row_pattern(authored_target),
        60,
        "the authored reference back",
        begin,
    )
    for component in reversed(collapsed_for_reference):
        begin = session.length()
        x, y, width, height = inspector_section_rect(session, component)
        session.send("click {:.0f} {:.0f}".format(x + 12.0, y + 6.0))
        session.wait(
            re.escape(INSPECTOR_SECTION_TOKEN)
            + r'(?=[^\n]*component="'
            + re.escape(component)
            + r'")[^\n]*collapsed=false',
            30,
            "the {} section expanded".format(component.rsplit("::", 1)[-1]),
            begin,
        )
    session.send("tab assets")
    wait_for_frame_advance(session, "a frame drawn after the tab switch")
    print_ok("an entity reference was picked, cleared, dropped and went missing")

    begin = session.length()
    session.send("select entity {}".format(state.prior_entity))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), r'\s+entity=[^ ]+\s+count=1\b'),
        30,
        "the prior selection restored",
        begin,
    )
    print_ok("the prior selection is back")

    state.applied_before = applied_before
    state.begin = begin


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_material_and_entity_references_steps(context.state)


AREA = StepArea(
    name="material_and_entity_references",
    order=4,
    run=_area_entry,
    needs=("mesh_entity", "original_material", "picked_entity", "prior_entity",),
    provides=(),
)
