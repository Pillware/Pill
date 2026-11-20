"""The editor suite's resources arrays and meshes steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    ENTITIES_DUMP_TOKEN,
    ENTITY_ROW_TOKEN,
    FIELD_TOKEN,
    GIZMO_RESOURCE_NAME,
    HISTORY_PUSH_TOKEN,
    HISTORY_UNDO_TOKEN,
    REFERENCE_SET_TOKEN,
    RESOURCES_DUMP_TOKEN,
    RESOURCE_EDIT_TOKEN,
    RESOURCE_FIELD_TOKEN,
    RESOURCE_SELECTED_TOKEN,
    SELECTION_DUMP_TOKEN,
    SELECTION_RESTORED_TOKEN,
    SHOWCASE_COMPONENT,
    STATS_DUMP_TOKEN,
    STATS_TAB_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    asset_row_capture,
    asset_row_pattern,
    last_log_match,
)


def run_resources_arrays_and_meshes_steps(state: StepState) -> None:
    """The steps through: the reference cleared and came back."""
    session = state.session


    # Step 3a5: the Statistics tab. The shell times every
    # tab body and its own pass, so the tab's rows are measurements - and
    # the tabs this suite has opened already must have non-zero times.
    stats_begin = session.length()
    session.send("tab statistics")
    session.send("dump stats")
    session.wait(re.escape(STATS_DUMP_TOKEN), 30, "the statistics dump", stats_begin)
    measured = {
        match.group(1): float(match.group(2))
        for match in re.finditer(
            log_layout.message_then_fields_pattern(re.escape(STATS_TAB_TOKEN), r'\s+name="(\w+)"\s+us=([0-9.]+)\s+passes=\d+'),
            session.read_since(stats_begin),
        )
    }
    assert measured, "the Statistics tab measured no tab body"
    # The console tab was actively drawn for the whole console step, so it
    # is the one row whose non-zero time is guaranteed; a dock leaf draws
    # only its active tab, so the others depend on activation order.
    assert measured.get("console", 0.0) > 0.0, (
        "the Statistics tab showed no time for the console tab: {}".format(measured)
    )
    assert len(measured) >= 4, (
        "the Statistics tab measured too few tabs: {}".format(measured)
    )
    print_ok(
        "the Statistics tab timed {} drawn tabs".format(len(measured))
    )

    # Step 3a7: world resources. The Inspector lists the
    # editable resources, a selection of one is recorded, and a field edit
    # applies and undoes like any other edit.
    begin = session.length()
    session.send("select resource {}".format(GIZMO_RESOURCE_NAME))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(RESOURCE_SELECTED_TOKEN), r'\s+resource="{}').format(re.escape(GIZMO_RESOURCE_NAME)),
        30,
        "the resource selection",
        begin,
    )
    dump = session.request(
        "dump resources", re.escape(RESOURCES_DUMP_TOKEN), 30, "the resources"
    )
    assert re.search(r"count=1\b", dump), (
        "the editable resources did not list: {}".format(dump[-600:])
    )
    assert re.search(
        log_layout.message_then_fields_pattern(re.escape(RESOURCE_FIELD_TOKEN), r'\s+resource="[^"]+"\s+field="settings\.scale_step"\s+label="Scale step"\s+value="0\.1"'),
        dump,
    ), "the seeded scale step is not the resource's value: {}".format(dump[-800:])
    begin = session.length()
    session.send(
        "edit resource {} settings.scale_step 0.25".format(GIZMO_RESOURCE_NAME)
    )
    session.wait(
        re.escape(RESOURCE_EDIT_TOKEN), 30, "the resource edit", begin
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="set {}\.settings\.scale_step"').format(
            re.escape(GIZMO_RESOURCE_NAME)
        ),
        30,
        "the resource edit recorded a step",
        begin,
    )
    dump = session.request(
        "dump resources", re.escape(RESOURCES_DUMP_TOKEN), 30, "the edited resource"
    )
    assert re.search(
        log_layout.message_then_fields_pattern(re.escape(RESOURCE_FIELD_TOKEN), r'\s+resource="[^"]+"\s+field="settings\.scale_step"\s+label="Scale step"\s+value="0\.25"'),
        dump,
    ), "the resource edit did not apply: {}".format(dump[-800:])
    begin = session.length()
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="set {}\.settings\.scale_step"').format(
            re.escape(GIZMO_RESOURCE_NAME)
        ),
        30,
        "the resource edit's undo",
        begin,
    )
    dump = session.request(
        "dump resources", re.escape(RESOURCES_DUMP_TOKEN), 30, "the restored resource"
    )
    assert re.search(
        log_layout.message_then_fields_pattern(re.escape(RESOURCE_FIELD_TOKEN), r'\s+resource="[^"]+"\s+field="settings\.scale_step"\s+label="Scale step"\s+value="0\.1"'),
        dump,
    ), "the undo did not restore the resource field: {}".format(dump[-800:])
    print_ok("a resource field edit applied and undid")
    # The selection itself is a recorded command: undoing it restores the
    # whole selection, entity slot included, rather than leaving the
    # Inspector on nothing.
    begin = session.length()
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_RESTORED_TOKEN), r'\s+entity="(?!none)[^"]+"\s+resource="none"'),
        30,
        "the resource selection's undo",
        begin,
    )
    print_ok("a resource selection round-trips through undo")

    # Step 3a8: collection fields. An array of structs
    # expands to one row per leaf, so an element of a collection field is
    # an ordinary field - it edits, records and undoes exactly like a float.
    dump = session.request(
        "dump entities", re.escape(ENTITIES_DUMP_TOKEN), 30, "the entity list"
    )
    showcase_row = next(
        (
            match
            for match in re.finditer(
                log_layout.message_then_fields_pattern(re.escape(ENTITY_ROW_TOKEN), r'\s+display="([^"]+)"\s+components="([^"]*)"'),
                dump,
            )
            if SHOWCASE_COMPONENT in match.group(2)
        ),
        None,
    )
    assert showcase_row is not None, (
        "no entity carries the showcase component: {}".format(dump[-800:])
    )
    showcase_entity = showcase_row.group(1)
    leaf = "points[0].x"
    begin = session.length()
    session.send(
        "dump field {} {} {}".format(showcase_entity, SHOWCASE_COMPONENT, leaf)
    )
    session.wait(re.escape(FIELD_TOKEN), 30, "the collection leaf", begin)
    before = re.search(r"value=(?:F32\()?(-?[0-9.]+)", session.read_since(begin))
    assert before is not None, (
        "the element's leaf did not read back: {}".format(
            session.read_since(begin)[-600:]
        )
    )
    original = float(before.group(1))
    begin = session.length()
    session.send(
        "edit set {} {} {} 2.5".format(showcase_entity, SHOWCASE_COMPONENT, leaf)
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="set {}[^"]*"').format(re.escape(SHOWCASE_COMPONENT)),
        30,
        "the collection edit recorded a step",
        begin,
    )
    begin = session.length()
    session.send(
        "dump field {} {} {}".format(showcase_entity, SHOWCASE_COMPONENT, leaf)
    )
    session.wait(re.escape(FIELD_TOKEN), 30, "the edited leaf", begin)
    assert "2.5" in session.read_since(begin), (
        "the collection edit did not apply: {}".format(
            session.read_since(begin)[-600:]
        )
    )
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the collection undo", begin)
    begin = session.length()
    session.send(
        "dump field {} {} {}".format(showcase_entity, SHOWCASE_COMPONENT, leaf)
    )
    session.wait(re.escape(FIELD_TOKEN), 30, "the restored leaf", begin)
    restored = re.search(r"value=(?:F32\()?(-?[0-9.]+)", session.read_since(begin))
    assert restored is not None and float(restored.group(1)) == original, (
        "the undo did not restore the element: {}".format(
            session.read_since(begin)[-600:]
        )
    )
    print_ok("a struct-array element edited and undid")

    # Step 3a8b: asset reference rows. A mesh renderer's
    # mesh and material fields are asset rows, each naming the asset it
    # holds. A scripted pick points one at another asset and loads it on
    # demand - the material file the suite stages is loaded by nothing
    # before the pick - undo brings the previous asset back, `none` clears
    # the row, and a material tile dragged from the Assets tab onto the
    # material row assigns it. Both rows end on the assets they started
    # with.
    dump = session.request(
        "dump entities", re.escape(ENTITIES_DUMP_TOKEN), 30, "the entity list"
    )
    mesh_row = next(
        (
            match
            for match in re.finditer(
                log_layout.message_then_fields_pattern(re.escape(ENTITY_ROW_TOKEN), r'\s+display="([^"]+)"\s+components="([^"]*)"'),
                dump,
            )
            if "MeshRendererComponent" in match.group(2)
        ),
        None,
    )
    assert mesh_row is not None, (
        "no entity carries a mesh renderer: {}".format(dump[-800:])
    )
    mesh_entity = mesh_row.group(1)
    # What was selected before this step: the viewport steps after it drag
    # the selection's gizmo, so the step puts the selection back.
    prior = session.request(
        "dump selection", re.escape(SELECTION_DUMP_TOKEN), 30, "the prior selection"
    )
    prior_match = re.search(r'primary="([^"]+)"', prior)
    assert prior_match is not None, "the prior selection did not read back: {}".format(
        prior[-400:]
    )
    prior_entity = prior_match.group(1)

    begin = session.length()
    session.send("select entity {}".format(mesh_entity))
    session.wait(asset_row_capture(mesh_entity, "mesh"), 30, "the mesh field's asset row", begin)
    session.wait(
        asset_row_capture(mesh_entity, "material"), 30, "the material field's asset row", begin
    )
    original_mesh = last_log_match(session, asset_row_capture(mesh_entity, "mesh"), begin).group(1)
    original_material = last_log_match(
        session, asset_row_capture(mesh_entity, "material"), begin
    ).group(1)
    assert original_mesh not in ("None", "(missing)"), (
        "the mesh row starts empty: {}".format(original_mesh)
    )
    assert original_material not in ("None", "(missing)"), (
        "the material row starts empty: {}".format(original_material)
    )
    print_ok(
        "the mesh renderer's rows name {} and {}".format(
            original_mesh, original_material
        )
    )

    # The pick: by path, through the recorded step the picker queues. The
    # mesh is not loaded before this, so the step loads it - `cube.obj` is the
    # second mesh the test project ships and no entity draws.
    begin = session.length()
    session.send(
        "edit set {} MeshRendererComponent mesh models/cube.obj".format(mesh_entity)
    )
    session.wait(
        re.escape(REFERENCE_SET_TOKEN)
        + r'(?=[^\n]*field="mesh")[^\n]*asset="models/cube\.obj"',
        60,
        "the mesh pick",
        begin,
    )
    session.wait(
        asset_row_pattern(mesh_entity, "mesh", "models/cube.obj"),
        30,
        "the row names the picked mesh",
        begin,
    )
    print_ok("picking another mesh loaded and assigned it")

    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the pick's undo", begin)
    session.wait(
        asset_row_pattern(mesh_entity, "mesh", original_mesh),
        30,
        "the row names the original mesh again",
        begin,
    )
    print_ok("undo restored the original mesh")

    begin = session.length()
    session.send("edit set {} MeshRendererComponent mesh none".format(mesh_entity))
    session.wait(
        asset_row_pattern(mesh_entity, "mesh", "None"), 30, "the cleared reference", begin
    )
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the clear's undo", begin)
    session.wait(
        asset_row_pattern(mesh_entity, "mesh", original_mesh),
        30,
        "the mesh row restored after the clear",
        begin,
    )
    print_ok("the reference cleared and came back")

    state.begin = begin
    state.mesh_entity = mesh_entity
    state.original_material = original_material
    state.prior_entity = prior_entity


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_resources_arrays_and_meshes_steps(context.state)


AREA = StepArea(
    name="resources_arrays_and_meshes",
    order=3,
    run=_area_entry,
    needs=(),
    provides=("mesh_entity", "original_material", "prior_entity",),
)
