"""The editor suite's tiles inspector and input steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    COMMAND_NO_ACTION_TOKEN,
    CONSOLE_DUMP_TOKEN,
    CONSOLE_FILTER_TOKEN,
    CONSOLE_SEARCH_TOKEN,
    EDITOR_KEYBINDINGS_APPLIED_TOKEN,
    EDITOR_KEYBINDING_CONFLICT_TOKEN,
    EDITOR_SHORTCUT_RAN_TOKEN,
    EGUI_ROWS_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    ENTITIES_DUMP_TOKEN,
    ENTITY_ROW_TOKEN,
    GIZMO_ANCHOR_TOKEN,
    GIZMO_DRAG_ENDED_TOKEN,
    GIZMO_DRAG_PRESS_TOKEN,
    GIZMO_DRAG_STARTED_TOKEN,
    GIZMO_TOOL_TOKEN,
    HISTORY_PUSH_TOKEN,
    HISTORY_REDO_TOKEN,
    HISTORY_UNDO_TOKEN,
    KEY_INJECTED_TOKEN,
    MENU_ACTION_TOKEN,
    OUTLINE_CLEARED_TOKEN,
    PALETTE_CLOSED_TOKEN,
    PALETTE_DISMISSED_TOKEN,
    PALETTE_QUERY_TOKEN,
    PICK_ENTITY_PATTERN,
    PICK_NOTHING_PATTERN,
    PREVIEW_READY_TOKEN,
    PREVIEW_REQUESTED_TOKEN,
    SCENE_BADGE_RECT_TOKEN,
    SCENE_BADGE_TOKEN,
    SCENE_PICK_HANDLE_TOKEN,
    SCENE_PICK_NOTHING_TOKEN,
    SCENE_RECT_PATTERN,
    SELECTION_CHANGED_TOKEN,
    TOAST_LINE_PATTERN,
    TOAST_TOKEN,
    TRANSFORM_COMPONENT,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    BADGE_CLICK_ATTEMPTS,
    console_dump,
    dump_field_values,
    newest_badge_rect,
    occurrence_count,
    visible_scene_rect,
    wait_applied,
    wait_for_frame_advance,
)


def run_tiles_inspector_and_input_steps(state: StepState) -> None:
    """The steps through: the console listed, searched and filtered the log."""
    session = state.session

    session.send(f"drag {state.splitter_x:.0f} {state.splitter_y:.0f} {state.splitter_x:.0f} {state.splitter_y + 24:.0f}")
    # The release lands on the next frame and the dock lays out after it; ask
    # the shell's own counter for that frame rather than sleep through it. The
    # dump below is the new rect's occurrence: it must differ from the one the
    # drag started on, so a drag that missed the band fails with its own
    # message instead of a log wait that would only time out.
    wait_for_frame_advance(session, "the frame after the splitter drag")
    scenes = session.request(
        "dump scenes", SCENE_RECT_PATTERN, 30, "the scene rects after the splitter drag"
    )
    moved = visible_scene_rect(scenes)
    assert moved is not None, "the scene rect vanished after the splitter drag"
    assert moved != state.scene_rect, (
        "a drag within the grab band left the split where it was: {} -> {}".format(
            state.scene_rect, moved
        )
    )
    print_ok(
        "a splitter drag {}px off the line moved the split {}".format(
            4, moved
        )
    )

    # The scene badges: the executable projects every
    # entity into the view and the shell draws a chip at each point. A
    # click on a chip selects its entity through the shell, so no scene
    # pick is asked for - the badge's own log line is the proof. The chips
    # report their rectangles only when they move, and the splitter drag
    # above moved the window, so the lines read here are the live ones.
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_BADGE_RECT_TOKEN), r"\s+view=\d+\s+entity=\S+\s+min_x=(-?[\d.]+)\s+min_y=(-?[\d.]+)")
        + r"\s+max_x=(-?[\d.]+)\s+max_y=(-?[\d.]+)",
        60,
        "a scene badge's rectangle",
        state.badge_begin,
    )
    badge = re.search(
        log_layout.message_then_fields_pattern(re.escape(SCENE_BADGE_RECT_TOKEN), r"\s+view=\d+\s+entity=(\S+)\s+min_x=(-?[\d.]+)\s+min_y=(-?[\d.]+)")
        + r"\s+max_x=(-?[\d.]+)\s+max_y=(-?[\d.]+)",
        session.read_since(state.badge_begin),
    )
    assert badge, "the badge's rectangle carried no entity"
    badge_entity = badge.group(1)
    # Select another entity first, so the badge's own selection is a
    # change the log states rather than a repeat of the same handle.
    entities = re.findall(
        log_layout.message_then_fields_pattern(re.escape(ENTITY_ROW_TOKEN), r'\s+display="([^"]+)"'),
        session.request(
            "dump entities",
            re.escape(ENTITIES_DUMP_TOKEN),
            60,
            "the entity list for the badge step",
        ),
    )
    other = next(
        (entity for entity in entities if entity != badge_entity), None
    )
    if other is not None:
        begin = session.length()
        session.send("select entity {}".format(other))
        session.wait(
            log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), rf"\s+entity={re.escape(other)}"),
            60,
            "another entity selected before the badge click",
            begin,
        )
    # The chips follow the scene camera, and the camera keeps easing for a
    # moment after the splitter drag above. A click at the rectangle read
    # first lands where the chip was a report ago: a run missed by 0.25 px,
    # which is one report's worth of drift. Each attempt clicks the newest
    # rectangle, and a chip that never answers still fails here.
    for attempt in range(1, BADGE_CLICK_ATTEMPTS + 1):
        badge_rect = newest_badge_rect(session, badge_entity, state.badge_begin)
        badge_x = (badge_rect[0] + badge_rect[2]) * 0.5
        badge_y = (badge_rect[1] + badge_rect[3]) * 0.5
        begin = session.length()
        session.send(f"click {badge_x:.0f} {badge_y:.0f}")
        try:
            session.wait(
                log_layout.message_then_fields_pattern(re.escape(SCENE_BADGE_TOKEN), rf"\s+entity={re.escape(badge_entity)}"),
                20,
                "the badge's click selected its entity",
                begin,
            )
            break
        except TimeoutError:
            if attempt == BADGE_CLICK_ATTEMPTS:
                raise
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), rf"\s+entity={re.escape(badge_entity)}"),
        60,
        "the badge's click moved the shared selection",
        begin,
    )
    print_ok(f"scene badge {badge_entity} clicked at ({badge_x:.0f}, {badge_y:.0f})")
    # The badge moved the shared selection; the steps below read the
    # entity the scene pick made, so it goes back on it here.
    restored = session.length()
    session.send(f"select entity {state.picked_entity}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), rf"\s+entity={re.escape(state.picked_entity)}"),
        60,
        "the picked entity selected again",
        restored,
    )

    # The asset previews: the preference turns them on,
    # the grid queues only the tiles it drew, and the texture's rendered
    # pixels come back as an image the tile draws. One request per frame,
    # so the assertions are the request line and then the finished image.
    session.send("edit preference editor.render_asset_previews true")
    session.send("browser textures")
    session.send("tab assets")
    preview_begin = session.length()
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PREVIEW_REQUESTED_TOKEN), r'\s+path="[^"]+\.png"'),
        120,
        "a texture tile's preview request",
        preview_begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PREVIEW_READY_TOKEN), r'\s+path="[^"]+\.png"\s+width=128\s+height=128'),
        120,
        "the texture tile's preview image",
        preview_begin,
    )
    print_ok("a texture tile's preview came back rendered")
    # A mesh tile takes the preview's own mesh pass: its own pipeline,
    # depth target and camera framing, so a mesh needs no material.
    session.send("browser models")
    mesh_begin = session.length()
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PREVIEW_REQUESTED_TOKEN), r'\s+path="[^"]+\.obj"'),
        120,
        "a mesh tile's preview request",
        mesh_begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PREVIEW_READY_TOKEN), r'\s+path="[^"]+\.obj"\s+width=128\s+height=128'),
        120,
        "the mesh tile's preview image",
        mesh_begin,
    )
    print_ok("a mesh tile's preview came back rendered")
    # Off again, so the rest of the run costs no preview work, and the
    # Inspector goes back on top of the column the grid shared, whose rows
    # the steps below read.
    session.send("edit preference editor.render_asset_previews false")
    session.send("tab inspector")

    # The Inspector's data follows the selection: the rows heartbeat
    # reports the detail tier (the selected entity and its component
    # count), which is what the Inspector tab draws.
    detail_pattern = (
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), rf'\s+frames=\d+[^\n]*detail="{re.escape(state.picked_entity)}"')
        + r"\s+detail_components=(\d+)"
    )
    begin = session.length()
    session.wait(detail_pattern, 60, "the inspector reported the detail", begin)
    match = session.find(detail_pattern, begin)
    assert int(match.group(1)) >= 1, (
        "the inspector saw no components: {}".format(match.group(0))
    )
    print_ok(f"inspector detail {state.picked_entity} components={match.group(1)}")

    # The Inspector's rows are editable: a scripted `edit
    # array` mirrors a DragValue edit on the entity's component fields.
    # The command applies at the frame boundary, pushes one history
    # step whose label names the field path, and the world dump sees
    # the new value; undo puts the old one back.
    translation_before = dump_field_values(session, state.picked_entity, "translation")
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send(
        f"edit array {state.picked_entity} {TRANSFORM_COMPONENT} translation 0 4.25"
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="set TransformComponent\[0\]\.translation"'),
        30,
        "the inspector edit pushed one history step",
        begin,
    )
    wait_applied(session, applied_before, "inspector edit applied")
    assert dump_field_values(session, state.picked_entity, "translation")[0] == 4.25, (
        "the inspector edit did not reach the world"
    )
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="set TransformComponent\[0\]\.translation"'),
        30,
        "the inspector edit undid",
        begin,
    )
    wait_applied(session, applied_before, "inspector undo applied")
    assert dump_field_values(session, state.picked_entity, "translation") == translation_before, (
        "the undo did not restore the field"
    )
    print_ok("inspector edit and undo round-trip")

    # The corner of the view holds nothing, which clears both.
    begin = session.length()
    session.send(f"click {state.scene_x + 6} {state.scene_y + 6}")
    session.wait_token(SCENE_PICK_NOTHING_TOKEN, 120, "the corner hits nothing", begin)
    session.wait_token(OUTLINE_CLEARED_TOKEN, 60, "the outline cleared with it", begin)
    print_ok("selection and outline cleared")

    # Step 3: the gizmo - the selection grows translate handles, and
    # scripted drags on each tool's own handle move, turn and scale the
    # entity through the real pick; the world is checked directly with
    # `dump field`.
    #
    # What the suite drags first is a translate handle, so the tool is set
    # rather than assumed: the steps above click the toolbox's own rows, which
    # switch the gizmo to another tool and leave it there. No token is waited
    # for on the switch itself - the editor logs a tool change only when the
    # mode actually moves, and a run that arrives already on translate prints
    # nothing - so the anchor wait below is what proves it took. The point is
    # the scene view's centre, from the rect the splitter drag left rather than
    # a fixed pixel: the dock laid the view out again when it took that drag,
    # so a hardcoded point only holds for one window size and one layout.
    begin = session.length()
    session.send("gizmo translate")
    gizmo_point_x = moved[0] + moved[2] * 0.5
    gizmo_point_y = moved[1] + moved[3] * 0.5
    session.send(f"click {gizmo_point_x:.0f} {gizmo_point_y:.0f}")
    found, _ = session.wait_either(
        [PICK_ENTITY_PATTERN, PICK_NOTHING_PATTERN], 120, begin
    )
    assert found == PICK_ENTITY_PATTERN, "the gizmo step could not re-select the entity"
    gizmo_entity = re.search(PICK_ENTITY_PATTERN, session.read_since(begin)).group(1)
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), rf'\s+entity={re.escape(gizmo_entity)}')
        + r'\s+mode="translate"\s+space="world"',
        60,
        "the translate handles are drawn for the selection",
        begin,
    )

    # Translate: drag the X arrow 120 px right; the entity's x moves.
    before_x = dump_field_values(session, gizmo_entity, "translation")[0]
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("gizmo drag x 120 0")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_PRESS_TOKEN), r'\s+axis="x"'),
        30,
        "the press ray was aimed at the X handle",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_PICK_HANDLE_TOKEN), r"\s+handle=\d+"),
        60,
        "the pick answered a handle",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_STARTED_TOKEN), rf'\s+mode="translate"\s+axis="x"\s+entity={re.escape(gizmo_entity)}'),
        60,
        "the drag started on the translate X handle",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_ENDED_TOKEN), r'\s+mode="translate"\s+axis="x"\s+entity=\S+')
        + r'\s+value=\[([-\d.eE+]+),[^\n]*snapped=false',
        60,
        "the drag ended, unsnapped, with a value",
        begin,
    )
    wait_applied(session, applied_before, "the drag's step applied")
    after_x = dump_field_values(session, gizmo_entity, "translation")[0]
    assert abs(after_x - before_x) > 0.5, (
        f"the translate drag barely moved the entity: {before_x} -> {after_x}"
    )
    print_ok(f"translate drag moved x {before_x:.3f} -> {after_x:.3f}")

    # A released drag is one recorded step: undo puts the press's
    # translation back and redo returns the drag's, so the one
    # manipulation a person is most likely to undo is undoable.
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="gizmo translate"'),
        30,
        "the drag undid as one step",
        begin,
    )
    wait_applied(session, applied_before, "the drag's undo applied")
    undone_x = dump_field_values(session, gizmo_entity, "translation")[0]
    assert abs(undone_x - before_x) < 1e-3, (
        f"the undo did not restore the press's x: {undone_x} vs {before_x}"
    )
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.redo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_REDO_TOKEN), r'\s+label="gizmo translate"'),
        30,
        "the drag redid",
        begin,
    )
    wait_applied(session, applied_before, "the drag's redo applied")
    redone_x = dump_field_values(session, gizmo_entity, "translation")[0]
    assert abs(redone_x - after_x) < 1e-3, (
        f"the redo did not return the drag's x: {redone_x} vs {after_x}"
    )
    print_ok(f"drag undo/redo round-trip x {undone_x:.3f} -> {redone_x:.3f}")

    # Rotate: the Y ring turns the entity; the ended token carries the
    # quaternion the drag wrote. The project's own system animates this
    # entity's rotation, so a later world dump is the system's value,
    # not the drag's - the written quaternion itself is asserted (a
    # unit quaternion), like the native suite's ring step.
    begin = session.length()
    session.send("gizmo rotate")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_TOOL_TOKEN), r'\s+mode="rotate"'),
        30,
        "the rotate tool is active",
        begin,
    )
    begin = session.length()
    session.send("gizmo drag y 120 0")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_STARTED_TOKEN), rf'\s+mode="rotate"\s+axis="y"\s+entity={re.escape(gizmo_entity)}'),
        60,
        "the drag started on the rotate Y ring",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_ENDED_TOKEN), r'\s+mode="rotate"\s+axis="y"\s+entity=\S+')
        + r"\s+value=\[[-\d.eE+, ]+\]",
        60,
        "the ring drag ended with the quaternion it wrote",
        begin,
    )
    match = re.search(r"value=\[([-\d.eE+, ]+)\]", session.read_since(begin))
    quaternion = [float(value) for value in match.group(1).split(",")]
    length = sum(component * component for component in quaternion) ** 0.5
    assert abs(length - 1.0) < 0.01, (
        f"the written quaternion is not unit length: {quaternion}"
    )
    print_ok(f"rotate drag wrote the quaternion {quaternion}")

    # Scale: the X cube resizes the entity along its own axis; the
    # factor the drag wrote is asserted from the ended token, and the
    # world value is printed for the record.
    begin = session.length()
    session.send("gizmo scale")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_TOOL_TOKEN), r'\s+mode="scale"'),
        30,
        "the scale tool is active",
        begin,
    )
    begin = session.length()
    session.send("gizmo drag x 120 0")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_STARTED_TOKEN), rf'\s+mode="scale"\s+axis="x"\s+entity={re.escape(gizmo_entity)}'),
        60,
        "the drag started on the scale X handle",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_ENDED_TOKEN), r'\s+mode="scale"\s+axis="x"\s+entity=\S+')
        + r"\s+value=\[([-\d.eE+]+),[-\d.eE+, ]*\]\s+snapped=false",
        60,
        "the scale drag ended with the value it wrote",
        begin,
    )
    match = re.search(r"value=\[([-\d.eE+]+),", session.read_since(begin))
    factor = float(match.group(1))
    assert abs(factor - 1.0) > 0.01, f"the scale drag wrote no change: {factor}"
    scale = dump_field_values(session, gizmo_entity, "scale")
    print_ok(f"scale drag wrote {factor:.3f} (scale now {scale})")

    # Step 3a: a keyboard chord reaches the same handler a real press does -
    # the E shortcut picks the rotate tool, which the scale tool step left
    # behind - and the injected chord is logged, so the injection itself is
    # provable. The next step's `gizmo translate` still changes the tool,
    # so it logs the way it always did.
    begin = session.length()
    session.send("key e")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(KEY_INJECTED_TOKEN), r'\s+chord="e"'),
        30,
        "the key chord injected",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_TOOL_TOKEN), r'\s+mode="rotate"'),
        30,
        "the key shortcut switched the tool",
        begin,
    )
    print_ok("a key chord reached the tool shortcut")

    # Step 3a2: the keybindings file rebinds a shortcut without a restart,
    # and a chord two commands share is reported by both of their names.
    # The file lands in the editor's state folder; the watcher applies it
    # on the next frame.
    begin = session.length()
    session.write_keybindings(
        {
            "preset": "unity",
            "overrides": {
                "editor.tool_translate": ["t"],
                "editor.tool_rotate": ["t"],
            },
        }
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_KEYBINDINGS_APPLIED_TOKEN), r"\s+preset=unity\s+overrides=2"),
        30,
        "the keybindings file applied",
        begin,
    )
    conflict = re.search(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_KEYBINDING_CONFLICT_TOKEN), r'\s+chord="t"\s+commands="([^"]+)"'),
        session.read_since(begin),
    )
    assert conflict, "the rebinding reported no conflicting chord"
    assert "editor.tool_translate" in conflict.group(1)
    assert "editor.tool_rotate" in conflict.group(1)
    # The rebound chord resolves through the registry: `t` runs translate
    # (the first command in registration order wins a shared chord).
    session.send("key t")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_SHORTCUT_RAN_TOKEN), r'\s+command="editor\.tool_translate"'),
        30,
        "the rebound chord ran its command",
        begin,
    )
    print_ok("a keybindings override rebound a shortcut live")

    # The conflict is also a warning toast, and clicking
    # that toast never reaches the scene: the row's rectangle joins the
    # frame's overlay regions.
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TOAST_TOKEN), r'\s+level="warning"[^\n]*text="Keybinding conflict:[^"]*'),
        30,
        "the conflict raised a warning toast",
        begin,
    )
    toast_rows = [
        match
        for match in re.finditer(TOAST_LINE_PATTERN, session.read_since(begin))
        if match.group(1) == "warning" and "Keybinding conflict" in match.group(2)
    ]
    assert toast_rows, "the warning toast logged no rectangle"
    row = toast_rows[-1]
    toast_center = (
        float(row.group(3)) + float(row.group(5)) / 2.0,
        float(row.group(4)) + float(row.group(6)) / 2.0,
    )
    pick_start = session.length()
    session.send(f"click {toast_center[0]:.0f} {toast_center[1]:.0f}")
    # A frame must run after the click before the absence of a pick line is
    # read as an answer; the shell's own counter tells us one did.
    wait_for_frame_advance(session, "a frame ran after the toast click")
    assert "scene pick" not in session.read_since(pick_start), (
        "a click on a toast reached the scene"
    )
    print_ok("a toast click stayed in the UI")

    # Step 3a3: the command palette. The scripted channel
    # opens it with a query, the shell filters the registry's commands, and
    # a run takes the menu bar's own action path - the same log token, with
    # `menu="palette"` naming the surface that asked.
    begin = session.length()
    session.send("palette translate")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PALETTE_QUERY_TOKEN), r'\s+query="translate"\s+matches=[1-9][0-9]*'),
        30,
        "the palette filtered the registry's commands",
        begin,
    )
    filtered = re.search(
        log_layout.message_then_fields_pattern(re.escape(PALETTE_QUERY_TOKEN), r'\s+query="translate"\s+matches=(\d+)\s+first="([^"]+)"'),
        session.read_since(begin),
    )
    assert filtered, "the palette's filter line carried no match count"
    assert filtered.group(2) == "Translate", (
        "the palette's first match was {!r}, not the translate command".format(
            filtered.group(2)
        )
    )
    session.send("palette run")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(MENU_ACTION_TOKEN), r'\s+menu="palette"\s+label="Translate"'),
        30,
        "the palette ran its selected command",
        begin,
    )
    session.wait(
        re.escape(PALETTE_CLOSED_TOKEN), 30, "the palette closed after running", begin
    )
    print_ok("the palette filtered and ran a command")
    # A second open, closed without running: the popup must not stay over
    # the viewport, where every later click would be the UI's.
    begin = session.length()
    session.send("palette scale")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PALETTE_QUERY_TOKEN), r'\s+query="scale"'),
        30,
        "the palette reopened on a second query",
        begin,
    )
    session.send("palette close")
    session.wait(
        re.escape(PALETTE_DISMISSED_TOKEN), 30, "the palette closed on request", begin
    )
    print_ok("the palette closed without running")

    # A registered command this build has no action for must not be offered:
    # the layout commands lost their menu path with the Layout menu that the
    # bar never had, so the palette names each one and lists no row. Before
    # this the row appeared, a run closed the popup, and nothing happened.
    begin = session.length()
    session.send("palette save layout")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(COMMAND_NO_ACTION_TOKEN), r'\s+command="editor\.save_layout"'),
        30,
        "the palette named the command it cannot run",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PALETTE_QUERY_TOKEN), r'\s+query="save layout"\s+matches=0'),
        30,
        "the palette listed no command without an action",
        begin,
    )
    session.send("palette close")
    session.wait(
        re.escape(PALETTE_DISMISSED_TOKEN), 30, "the palette closed on request", begin
    )
    print_ok("the palette named and dropped a command with no action")

    # Step 3a4: the Console panel. The tab reads the
    # telemetry capture ring, so the project's own log line is in the game
    # channel; the filters narrow what it draws.
    session.send("tab console")
    begin = session.length()
    session.send("dump console")
    session.wait(re.escape(CONSOLE_DUMP_TOKEN), 30, "the console dump", begin)
    dump = console_dump(session, begin)
    assert dump, "the console dump did not parse"
    assert int(dump.group("game")) >= 1, (
        "the project's own log line did not reach the game channel: "
        + dump.group(0)
    )
    assert int(dump.group("visible")) >= 1, "the console showed no rows"
    # A search narrows the rows to the ones that carry its text. The token is
    # one the project's own UI layer logs, so a hit proves the project's line
    # reached the ring; the editor's own `[editor] console search` line carries
    # the text too, which is why the step asserts the rows shrank rather than
    # that every row left is the project's.
    begin = session.length()
    session.send("console search producer=game")
    session.wait(re.escape(CONSOLE_SEARCH_TOKEN), 30, "the console search applied", begin)
    # The filter lands on the executable's next frame, so a frame has to
    # run before the dump can read it.
    wait_for_frame_advance(session, "a frame applied the search")
    begin = session.length()
    session.send("dump console")
    session.wait(re.escape(CONSOLE_DUMP_TOKEN), 30, "the searched console dump", begin)
    filtered = console_dump(session, begin)
    assert filtered, "the filtered console dump did not parse"
    assert filtered.group("search") == "producer=game", (
        "the search did not reach the console: " + filtered.group(0)
    )
    assert int(filtered.group("visible")) >= 1, (
        "the search dropped the project's own line: " + filtered.group(0)
    )
    assert int(filtered.group("visible")) < int(filtered.group("total")), (
        "the search did not narrow the rows: " + filtered.group(0)
    )
    # A level filter above the project's lines leaves none of them. `error`,
    # not `warn`: the test project's own `TagAlphaComponent` line is a warning, and
    # the step is about a level nothing in the session reaches.
    begin = session.length()
    session.send("console filter error")
    session.wait(re.escape(CONSOLE_FILTER_TOKEN), 30, "the console filter applied", begin)
    wait_for_frame_advance(session, "a frame applied the level")
    begin = session.length()
    session.send("dump console")
    session.wait(re.escape(CONSOLE_DUMP_TOKEN), 30, "the levelled console dump", begin)
    levelled = console_dump(session, begin)
    assert levelled, "the level-filtered console dump did not parse"
    assert levelled.group("level") == "error", (
        "the level filter did not reach the console: " + levelled.group(0)
    )
    assert int(levelled.group("visible")) == 0, (
        "an error filter kept rows below its level: " + levelled.group(0)
    )
    # Back to the unfiltered view, so the rest of the run reads the tab
    # the way a fresh session would.
    session.send('console search ""')
    session.send("console filter info")
    session.wait(re.escape(CONSOLE_FILTER_TOKEN), 30, "the console filter reset", begin)
    print_ok("the console listed, searched and filtered the log")

    state.applied_before = applied_before
    state.begin = begin
    state.gizmo_entity = gizmo_entity


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_tiles_inspector_and_input_steps(context.state)


AREA = StepArea(
    name="tiles_inspector_and_input",
    order=2,
    run=_area_entry,
    needs=("badge_begin", "picked_entity", "scene_rect", "scene_x", "scene_y", "splitter_x", "splitter_y",),
    provides=("gizmo_entity",),
)
