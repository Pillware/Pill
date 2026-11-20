"""The editor suite's selection clipboard and gizmo steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re
import json

from core import win_clipboard
from core import log_layout
from core.paths import REPOSITORY_ROOT
from core.editor_log_tokens import (
    CLIPBOARD_PASTED_TOKEN,
    CLIPBOARD_WRITTEN_TOKEN,
    DRAG_OUT_NO_BUTTON_TOKEN,
    DRAG_OUT_PREPARED_TOKEN,
    DROP_PREVIEW_TOKEN,
    DROP_REFUSED_FOLDER_TOKEN,
    DROP_REFUSED_PATH_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    ENTITIES_DUMP_TOKEN,
    ENTITY_ROW_TOKEN,
    FIELD_TOKEN,
    FILES_DROPPED_TOKEN,
    GIZMO_DRAG_ENDED_TOKEN,
    GIZMO_DRAG_PRESS_TOKEN,
    GIZMO_DRAG_RELEASED_TOKEN,
    GIZMO_DRAG_STARTED_TOKEN,
    GIZMO_GUIDES_TOKEN,
    GIZMO_HOVER_NONE_TOKEN,
    GIZMO_HOVER_TOKEN,
    GIZMO_TOOL_TOKEN,
    HISTORY_PUSH_TOKEN,
    HISTORY_UNDO_TOKEN,
    MIXED_ROW_TOKEN,
    SELECTED_ENTITY_TOKEN,
    SELECTION_CHANGED_TOKEN,
    SELECTION_DUMP_TOKEN,
    SELECTION_EDIT_TOKEN,
    SHOWCASE_COMPONENT,
    SHOWCASE_TYPE_NAME,
    TOAST_TOKEN,
    VIEWPORT_DUMP_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    dump_field_values,
    occurrence_count,
    wait_applied,
    wait_for_frame_advance,
    wait_for_path,
)


def run_selection_clipboard_and_gizmo_steps(state: StepState) -> None:
    """The steps through: the pointer highlights the handle under it."""
    session = state.session


    # Step 3a9: the multi-selection. Ctrl adds, Shift
    # ranges, a plain click resets, and a field edit over the set is one
    # recorded step that undoes in one.
    dump = session.request(
        "dump entities", re.escape(ENTITIES_DUMP_TOKEN), 30, "the entity list"
    )
    showcase_entities = [
        match.group(1)
        for match in re.finditer(
            log_layout.message_then_fields_pattern(re.escape(ENTITY_ROW_TOKEN), r'\s+display="([^"]+)"\s+components="([^"]*)"'),
            dump,
        )
        if SHOWCASE_COMPONENT in match.group(2)
    ]
    assert len(showcase_entities) >= 3, (
        "fewer than three entities carry the showcase component: {}".format(
            showcase_entities
        )
    )
    first, second, third = showcase_entities[:3]
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
    session.send("select entity {}".format(first))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), r'\s+entity=[^ ]+\s+count=1\b'),
        30,
        "the first selection",
        begin,
    )
    begin = session.length()
    session.send("select add {}".format(second))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), r'\s+entity=[^ ]+\s+count=2\b'),
        30,
        "the Ctrl add",
        begin,
    )
    third_add_begin = session.length()
    session.send("select add {}".format(third))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), r'\s+entity=[^ ]+\s+count=3\b'),
        30,
        "the second Ctrl add",
        third_add_begin,
    )
    begin = session.length()
    session.send("dump selection")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_DUMP_TOKEN), r'\s+count=3\b'),
        30,
        "the selected set",
        begin,
    )
    selection_log = session.read_since(begin)
    assert selection_log.count(SELECTED_ENTITY_TOKEN) >= 3, (
        "the dump did not name all three entities: {}".format(selection_log[-600:])
    )
    assert re.search(
        r'primary="{}"'.format(re.escape(third)), selection_log
    ), "the last entity added is not the primary: {}".format(selection_log[-600:])
    print_ok("Ctrl extended the selection to three entities")

    # The Inspector marks the rows the selection disagrees on: the
    # showcase component's values differ per model, so the frame after the
    # third entity joined the set draws a mixed row.
    session.wait(
        re.escape(MIXED_ROW_TOKEN), 30, "the mixed inspector row", third_add_begin
    )
    print_ok("the Inspector marked a mixed row")

    # One edit over the set: three entities, one recorded step.
    originals = {}
    for entity in (first, second, third):
        log = session.request(
            "dump field {} {} scale".format(entity, SHOWCASE_COMPONENT),
            re.escape(FIELD_TOKEN),
            30,
            "the original scale",
        )
        match = re.search(r"value=(?:F32\()?(-?[0-9.]+)", log)
        assert match is not None, "a scale did not read back: {}".format(log[-400:])
        originals[entity] = float(match.group(1))
    begin = session.length()
    session.send("edit selection {} scale 2.5".format(SHOWCASE_COMPONENT))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_EDIT_TOKEN), r'\s+component="[^"]*{}"\s+field="scale"\s+entities=3\b').format(
            re.escape(SHOWCASE_COMPONENT)
        ),
        30,
        "the selection edit",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="set {}\.scale"').format(re.escape(SHOWCASE_COMPONENT)),
        30,
        "the selection edit recorded one step",
        begin,
    )
    for entity in (first, second, third):
        log = session.request(
            "dump field {} {} scale".format(entity, SHOWCASE_COMPONENT),
            re.escape(FIELD_TOKEN),
            30,
            "the edited scale",
        )
        assert "2.5" in log, (
            "the selection edit missed {}: {}".format(entity, log[-400:])
        )
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the selection undo", begin)
    for entity in (first, second, third):
        log = session.request(
            "dump field {} {} scale".format(entity, SHOWCASE_COMPONENT),
            re.escape(FIELD_TOKEN),
            30,
            "the restored scale",
        )
        match = re.search(r"value=(?:F32\()?(-?[0-9.]+)", log)
        assert match is not None and float(match.group(1)) == originals[entity], (
            "one undo did not restore {}: {}".format(entity, log[-400:])
        )
    print_ok("a shared-field edit across three entities undid in one step")

    # A plain click resets the set - and puts the selection back where the
    # step found it, so the viewport steps after this one drag the same
    # entity.
    begin = session.length()
    session.send("select entity {}".format(prior_entity))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), r'\s+entity=[^ ]+\s+count=1\b'),
        30,
        "the resetting click",
        begin,
    )
    print_ok("a plain click reset the selection")

    # Step 3a10: the clipboard. A copy leaves the
    # editor under its registered format, which another process reads back
    # here; a component payload written from outside pastes onto the
    # selection in one undo step; and a second editor instance pastes the
    # entity copy, which is what "across editor instances" means.
    win_clipboard.clear()
    begin = session.length()
    session.send("command editor.copy")
    session.wait(
        re.escape(CLIPBOARD_WRITTEN_TOKEN), 30, "the clipboard write", begin
    )
    payload = json.loads(win_clipboard.read_payload())
    assert payload.get("kind") == "entity", (
        "the registered format does not carry an entity payload: {}".format(payload)
    )
    assert payload.get("pill") == 1, "the payload's version is not 1"
    print_ok("the copy reached the OS clipboard as an entity payload")

    component_payload = {
        "pill": 1,
        "kind": "component",
        "component": {"type": SHOWCASE_TYPE_NAME, "fields": [["frame", "7.25f"]]},
    }
    win_clipboard.write_payload(json.dumps(component_payload))
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.paste")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(CLIPBOARD_PASTED_TOKEN), r'\s+kind="component"'),
        30,
        "the component paste",
        begin,
    )
    # The paste is logged when the payload is decoded; its commands cross
    # the frame boundary after that, so the dump below would otherwise race
    # the write it is meant to read.
    wait_applied(session, applied_before, "the component paste applied")
    begin = session.length()
    session.send(
        "dump field {} {} frame".format(prior_entity, SHOWCASE_COMPONENT)
    )
    session.wait(re.escape(FIELD_TOKEN), 30, "the pasted field", begin)
    assert "7.25" in session.read_since(begin), (
        "the component paste did not apply: {}".format(
            session.read_since(begin)[-600:]
        )
    )
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the paste's undo", begin)
    begin = session.length()
    session.send(
        "dump field {} {} frame".format(prior_entity, SHOWCASE_COMPONENT)
    )
    session.wait(re.escape(FIELD_TOKEN), 30, "the restored pasted field", begin)
    assert "7.25" not in session.read_since(begin), (
        "one undo did not take the whole component paste back: {}".format(
            session.read_since(begin)[-600:]
        )
    )
    print_ok("a component paste applied and undid in one step")

    # Step 3a11: an OS drop into Assets. winit's drop
    # events cannot be synthesised from the command file, so the paths go to
    # the same handler they call: the preview names the folder, the file is
    # copied into it as one recorded step, and undo takes it off the disk.
    # A folder is refused by name instead of being copied.
    drop_source = REPOSITORY_ROOT / "local" / "temp" / "s18_dropped.png"
    drop_source.parent.mkdir(parents=True, exist_ok=True)
    drop_source.write_bytes(b"\x89PNG\r\n\x1a\ndrop probe\n")
    begin = session.length()
    session.send("hover file {}".format(drop_source))
    session.wait(
        re.escape(DROP_PREVIEW_TOKEN), 30, "the drop preview", begin
    )
    begin = session.length()
    session.send("drop {}".format(drop_source))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(FILES_DROPPED_TOKEN), r"\s+count=1\b"),
        30,
        "the dropped file",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="import files"'),
        30,
        "the import step",
        begin,
    )
    # The import is a file copy the editor does on its own thread; poll
    # the tree rather than assume the copy finished with the log line.
    def copied_file(_text: str):
        return True if list(state.asset_root.rglob(drop_source.name)) else None

    session.wait_until(
        copied_file,
        30,
        "the dropped file was never copied into the project\n",
        label="the dropped file was copied",
        poll=0.2,
    )
    imported = list(state.asset_root.rglob("s18_dropped.png"))
    assert len(imported) == 1, (
        "the dropped file was not copied into the project: {}".format(imported)
    )
    begin = session.length()
    session.send("command editor.undo")
    session.wait(re.escape(HISTORY_UNDO_TOKEN), 30, "the import's undo", begin)
    # The undo deletes the copied file on its own turn; poll the tree
    # rather than give the removal a fixed moment.
    wait_for_path(session, imported[0], False, 30, "the undo removed the imported file")
    assert not imported[0].exists(), (
        "the undo left the imported file on disk: {}".format(imported[0])
    )
    print_ok("a dropped file was imported and undone")

    begin = session.length()
    session.send("drop {}".format(drop_source.parent))
    session.wait(
        re.escape(DROP_REFUSED_FOLDER_TOKEN), 30, "the folder refusal", begin
    )
    session.wait(
        re.escape(TOAST_TOKEN) + r'.*is a folder',
        30,
        "the refusal toast",
        begin,
    )
    print_ok("a dropped folder is refused with a toast")

    # A drop folder the scripted channel climbed out of: the join
    # goes through the same validator every other asset command uses, so
    # `res/../..` is refused instead of being joined onto the asset root -
    # the copy would land in `examples/`, outside the project. Both lines go
    # in one write: the channel polls once per frame, so the drop runs while
    # the browser still holds the climbed-out folder (the listing settles it
    # back to the root between frames).
    begin = session.length()
    session.send_many(["browser ../..", "drop {}".format(drop_source)])
    session.wait(
        re.escape(DROP_REFUSED_PATH_TOKEN)
        + r".*is not a path inside the asset directory",
        30,
        "the traversal refusal",
        begin,
    )
    # A queued import would copy on a later frame, so the assertions below
    # are only worth anything if that frame had time to run.
    wait_for_frame_advance(session, "the frame after the refused drop")
    escaped = state.asset_root.parent.parent / drop_source.name
    assert not escaped.exists(), (
        "a drop carried the file outside the asset directory: {}".format(escaped)
    )
    assert not list(state.asset_root.rglob(drop_source.name)), (
        "the refused drop still copied the file into the project"
    )
    print_ok("a drop folder outside the asset directory is refused")
    session.send("browser root")

    # Step 3a12: the drag-out smoke test. Dropping into
    # Explorer needs a person, so what this checks is everything up to
    # OLE: the asset's path resolves to a file and becomes a data object.
    # The drag then ends without entering OLE's loop, because no button is
    # held - the same rule the drop source applies to a released button.
    drag_source = REPOSITORY_ROOT / "local" / "temp" / "s19_drag.png"
    drag_source.write_bytes(b"\x89PNG\r\n\x1a\ndrag probe\n")
    begin = session.length()
    session.send("drag out {}".format(drag_source))
    session.wait(
        re.escape(DRAG_OUT_PREPARED_TOKEN), 60, "the drag-out hand-off", begin
    )
    session.wait(
        re.escape(DRAG_OUT_NO_BUTTON_TOKEN), 60, "the drag-out refusal", begin
    )
    print_ok("an asset reached OLE's data object, and the drag ends safely")

    # Park the tool on rotate: the next step's `gizmo translate` only logs
    # a change, and the bound chord has just taken the tool to translate.
    begin = session.length()
    session.send("gizmo rotate")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_TOOL_TOKEN), r'\s+mode="rotate"'),
        30,
        "the tool parked on rotate for the next step",
        begin,
    )

    # Step 3b: the viewport's interaction states. A drag begins
    # on the press and ends on the release; the hover highlight follows
    # the pointer; the axis guide and the displacement line appear with
    # the drag; and the translation tool's plane handles move in their
    # own plane.
    begin = session.length()
    session.send("gizmo translate")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_TOOL_TOKEN), r'\s+mode="translate"'),
        30,
        "the translate tool is active again",
        begin,
    )
    begin = session.length()
    session.send("dump viewport")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(VIEWPORT_DUMP_TOKEN), r'\s+active=0\s+interaction="idle"'),
        30,
        "the viewport is idle",
        begin,
    )
    print_ok("the viewport starts active and idle")

    # The press reports the exact handle pixel and starts the drag - no
    # release needed - and a release with no move finishes it.
    press_begin = session.length()
    session.send("gizmo press x")
    press_pattern = (
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_PRESS_TOKEN), r'\s+axis="x"\s+x=([-\d.]+)\s+y=([-\d.]+)')
    )
    session.wait(press_pattern, 30, "the X handle was pressed", press_begin)
    press = session.find(press_pattern, press_begin)
    press_x, press_y = float(press.group(1)), float(press.group(2))
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_STARTED_TOKEN), r'\s+mode="translate"\s+axis="x"'),
        60,
        "the drag started on the press",
        press_begin,
    )
    print_ok("the drag begins on the press, before any release")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_GUIDES_TOKEN), r'\s+axis="x"\s+dotted=true'),
        30,
        "the axis guide and the displacement line appeared",
        press_begin,
    )
    begin = session.length()
    session.send("dump viewport")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(VIEWPORT_DUMP_TOKEN), r'\s+active=0\s+interaction="dragging-axis"'),
        30,
        "the drag owns the viewport",
        begin,
    )
    begin = session.length()
    session.send("gizmo release")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_RELEASED_TOKEN), r'\s+mode="translate"\s+axis="x"'),
        60,
        "the release ended the drag",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_GUIDES_TOKEN), r'\s+guides=false'),
        30,
        "the guides cleared with the drag",
        begin,
    )
    print_ok("the release ends the drag and its guides")

    # The pointer over the handle highlights it, and leaving the gizmo
    # clears the highlight. The press moved nothing, so the handle still
    # sits on the pixel the press reported.
    begin = session.length()
    session.send(f"hover 0 {press_x:.0f} {press_y:.0f}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_HOVER_TOKEN), r'\s+view=0\s+handle=1'),
        30,
        "the X handle highlighted",
        begin,
    )
    begin = session.length()
    session.send("hover 0 5 5")
    session.wait(
        re.escape(GIZMO_HOVER_NONE_TOKEN),
        30,
        "the highlight cleared",
        begin,
    )
    print_ok("the pointer highlights the handle under it")

    # A plane handle drags in its own plane: X and Y follow the drag, Z
    # keeps its value.
    before_plane = dump_field_values(session, state.gizmo_entity, "translation")
    begin = session.length()
    session.send("gizmo drag xy 40 30")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_STARTED_TOKEN), r'\s+mode="translate"\s+plane="xy"'),
        60,
        "the drag started on the XY square",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_DRAG_ENDED_TOKEN), r'\s+mode="translate"\s+plane="xy"'),
        60,
        "the plane drag ended",
        begin,
    )

    state.applied_before = applied_before
    state.before_plane = before_plane
    state.begin = begin


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_selection_clipboard_and_gizmo_steps(context.state)


AREA = StepArea(
    name="selection_clipboard_and_gizmo",
    order=5,
    run=_area_entry,
    needs=("gizmo_entity",),
    provides=("before_plane",),
)
