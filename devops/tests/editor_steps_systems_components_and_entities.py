"""The editor suite's systems components and entities steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re

import yaml

from core import log_layout
from core.editor_log_tokens import (
    ASSET_SELECTED_TOKEN,
    CONTEXT_MENU_TOKEN,
    DELETE_ENTITY_TOKEN,
    EGUI_ROWS_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    GIZMO_ANCHOR_TOKEN,
    HISTORY_PUSH_TOKEN,
    HISTORY_UNDO_TOKEN,
    NEW_ENTITY_TOKEN,
    PAUSE_TOGGLED_TOKEN,
    SELECTION_CHANGED_TOKEN,
    STEP_QUEUED_TOKEN,
    SYSTEMS_DUMP_TOKEN,
    SYSTEM_EDIT_QUEUED_TOKEN,
    SYSTEM_ROW_PATTERN,
    TRANSFORM_COMPONENT,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    entity_count,
    occurrence_count,
    wait_applied,
    wait_for_entity_count,
    wait_meta_value,
)


def run_systems_components_and_entities_steps(state: StepState) -> None:
    """The steps through: component detach and attach round-trip."""
    session = state.session

    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="set asset flip_v"'),
        30,
        "the boolean row's edit undid",
        state.begin,
    )
    wait_applied(session, state.applied_before, "settings undo applied")
    wait_meta_value(session, state.cube_meta, "flip_v", state.flip_before, 30, "the undo restored the sidecar")

    texture_asset = "textures/reimport_sample.png"
    texture_meta = state.asset_root / "textures" / "reimport_sample.png.meta"
    begin = session.length()
    session.send(f"select asset {texture_asset}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_SELECTED_TOKEN), r'\s+path="') + re.escape(texture_asset) + r'"',
        30,
        "the color texture row selected",
        begin,
    )
    texture_before = yaml.safe_load(texture_meta.read_text(encoding="utf-8"))["settings"][
        "texture_type"
    ]
    texture_next = "Normal" if texture_before == "Color" else "Color"
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send(
        f"edit asset-settings {texture_asset} texture_type {texture_next}"
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="set asset texture_type"'),
        30,
        "the enum row's edit recorded",
        begin,
    )
    wait_applied(session, applied_before, "enum edit applied")
    wait_meta_value(session, 
        texture_meta, "texture_type", texture_next, 30, "the enum landed in the sidecar"
    )
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="set asset texture_type"'),
        30,
        "the enum row's edit undid",
        begin,
    )
    wait_applied(session, applied_before, "enum undo applied")
    wait_meta_value(session, 
        texture_meta, "texture_type", texture_before, 30, "the undo restored the enum"
    )
    print_ok("inspector asset settings: boolean and enum rows round-trip")
    print_ok("assets pipeline: create, rename, move, delete and meta round-trip")

    # Step 4b: the Systems tab's data and controls - the dump lists the
    # registered systems with the state the tab draws, a scripted toggle
    # is a recorded undo step, and the pause and step commands reach the
    # engine (the rows heartbeat carries the pause state the tab's
    # button reads).

    def dump_systems_state() -> "tuple[dict[int, bool], bool]":
        """One `dump systems` round-trip: index -> enabled, and the pause flag."""
        seen = occurrence_count(session, re.escape(SYSTEMS_DUMP_TOKEN))
        begin = session.length()
        session.send("dump systems")
        dumped = session.wait_occurrence(
            log_layout.message_then_fields_pattern(re.escape(SYSTEMS_DUMP_TOKEN), r"\s+count=(\d+)\s+paused=(true|false)"),
            seen + 1,
            60,
            "systems dump",
        )
        dump = session.read_since(begin)
        states = {
            int(match.group(1)): match.group(4) == "true"
            for match in re.finditer(SYSTEM_ROW_PATTERN, dump)
        }
        return states, "paused=true" in dumped

    states, paused = dump_systems_state()
    assert states, "the systems dump carried no rows"
    assert all(states.values()), f"a system started disabled: {states}"
    first_index = min(states)
    # The areas share one editor, and under some keymap presets a key pressed
    # in a viewport by an earlier area pauses the engine, so this area sets its
    # own precondition instead of assuming the session it inherits: a running
    # engine, which is what the toggle checks below measure from.
    if paused:
        begin = session.length()
        session.send("command editor.pause_toggle")
        session.wait(
            log_layout.message_then_fields_pattern(
                re.escape(PAUSE_TOGGLED_TOKEN), r"\s+paused=false"
            ),
            30,
            "the engine resumed before the pause checks",
            begin,
        )
        states, paused = dump_systems_state()
        assert not paused, "the engine stayed paused after the resume"
    print_ok(f"systems dumped: {len(states)} rows, index {first_index} first")

    # One toggle off is a recorded edit; the dump is the tab's data, so
    # seeing it flip proves the tab draws the new state. The undo puts
    # it back.
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send(f"edit system {first_index} off")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SYSTEM_EDIT_QUEUED_TOKEN), rf"\s+index={first_index}\s+enabled=false"),
        30,
        "the system toggle queued",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), rf'\s+label="system {first_index} off"'),
        30,
        "the system toggle recorded",
        begin,
    )
    wait_applied(session, applied_before, "system toggle applied")
    states, _ = dump_systems_state()
    assert states.get(first_index) is False, "the system toggle did not reach the world"
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), rf'\s+label="system {first_index} off"'),
        30,
        "the system toggle undid",
        begin,
    )
    wait_applied(session, applied_before, "system undo applied")
    states, _ = dump_systems_state()
    assert states.get(first_index) is True, "the undo did not restore the system"

    # Pause, step and resume: the commands the tab's header row queues;
    # the rows heartbeat reports the pause flag the button's label
    # reads.
    begin = session.length()
    session.send("command editor.pause_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PAUSE_TOGGLED_TOKEN), r"\s+paused=true"),
        30,
        "the pause toggle",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), r"\s+frames=\d+[^\n]*paused=true"),
        60,
        "the rows heartbeat saw the pause",
        begin,
    )
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.step")
    session.wait(re.escape(STEP_QUEUED_TOKEN), 30, "the step command", begin)
    wait_applied(session, applied_before, "step applied")
    begin = session.length()
    session.send("command editor.pause_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(PAUSE_TOGGLED_TOKEN), r"\s+paused=false"),
        30,
        "the resume toggle",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), r"\s+frames=\d+[^\n]*paused=false"),
        60,
        "the rows heartbeat saw the resume",
        begin,
    )
    print_ok("systems tab: dump, recorded toggle with undo, pause and step")

    # Step 4c: the context menus. The viewport's menu opens on a real
    # right-click; the menu actions are the same queued commands the
    # scripted entries drive here - the entity create/delete pair (one
    # recorded step each; the delete's undo restores the entity AND the
    # selection that pointed at it), a component detach and attach, and
    # the gizmo tool, snapping and space switches the viewport menu's
    # items queue. The other areas' menus (entities, Inspector, Assets,
    # dock tabs) are probe-captured.

    # Re-select an entity (step 4 left an asset selected): the scripted
    # `select entity` walks the same recorded path the Entities list's
    # row click queues - a screen-space grid cannot hit the entity the
    # gizmo drags moved.
    menu_entity = state.gizmo_entity
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send(f"select entity {menu_entity}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), rf"\s+entity={re.escape(menu_entity)}"),
        30,
        "the entity re-selected for the menu step",
        begin,
    )
    wait_applied(session, applied_before, "the selection command applied")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), rf"[^\n]*detail=\"{re.escape(menu_entity)}\""),
        60,
        "the re-picked entity reached the Inspector detail",
        begin,
    )
    match = re.search(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), rf"[^\n]*detail=\"{re.escape(menu_entity)}\"\s+detail_components=(\d+)"),
        session.read_since(begin),
    )
    components_before = int(match.group(1))
    assert components_before >= 1, "the re-picked entity carries no components to detach"

    # The viewport's menu opens on a real right-click over the scene:
    # the bottom-left of the viewport, clear of both the overlay chips
    # (top-right) and the floating toolbox (top-left), which consumes
    # whatever lands on it.
    begin = session.length()
    session.send(f"rclick {state.scene_x + 60} {state.scene_y + state.scene_height - 60}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(CONTEXT_MENU_TOKEN), r'\s+area="scene"'),
        30,
        "the viewport menu opened",
        begin,
    )

    # The tool switches: the anchor line carries the state the menu's
    # items set, so each switch is asserted through it.
    begin = session.length()
    session.send("command editor.tool_rotate")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), rf'\s+entity={re.escape(menu_entity)}\s+mode="rotate"'),
        60,
        "the tool switched to rotate",
        begin,
    )
    begin = session.length()
    session.send("command editor.tool_snap_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), r"[^\n]*snapped=true"),
        60,
        "snapping toggled on",
        begin,
    )
    begin = session.length()
    session.send("command editor.tool_snap_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), r"[^\n]*snapped=false"),
        60,
        "snapping toggled off",
        begin,
    )
    begin = session.length()
    session.send("command editor.tool_space_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), r'[^\n]*space="local"'),
        60,
        "the space switched to local",
        begin,
    )
    begin = session.length()
    session.send("command editor.tool_space_toggle")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), r'[^\n]*space="world"'),
        60,
        "the space switched back",
        begin,
    )
    print_ok("the viewport menu's tool switches reach the gizmo")

    # The component detach and attach, recorded under the native labels:
    # the heartbeat's detail_components is the count the Inspector draws.
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send(f"edit remove {menu_entity} {TRANSFORM_COMPONENT}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="remove TransformComponent"'),
        30,
        "the component detach recorded",
        begin,
    )
    wait_applied(session, applied_before, "component detach applied")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), rf"[^\n]*detail=\"{re.escape(menu_entity)}\"")
        + rf"\s+detail_components={components_before - 1}",
        60,
        "the component count dropped",
        begin,
    )
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send(f"edit add {menu_entity} {TRANSFORM_COMPONENT}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="add TransformComponent"'),
        30,
        "the component attach recorded",
        begin,
    )
    wait_applied(session, applied_before, "component attach applied")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), rf"[^\n]*detail=\"{re.escape(menu_entity)}\"")
        + rf"\s+detail_components={components_before}",
        60,
        "the component count came back",
        begin,
    )
    print_ok("component detach and attach round-trip")

    # One recorded create, then the selected entity's recorded delete
    # (the destroy and the selection clear as one step), then its undo:
    # the entity comes back under a fresh handle and the selection
    # follows the remap.
    count_before = entity_count(session)
    assert count_before >= 1, "the entity list reported an empty world"
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.entity_create")
    session.wait(re.escape(NEW_ENTITY_TOKEN), 30, "the new entity queued", begin)
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="new entity"'),
        30,
        "the new entity recorded",
        begin,
    )
    wait_applied(session, applied_before, "new entity applied")
    wait_for_entity_count(session, count_before + 1, "the create added one entity")

    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.entity_delete")
    session.wait(re.escape(DELETE_ENTITY_TOKEN), 30, "the delete queued", begin)
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="delete entity"'),
        30,
        "the delete recorded",
        begin,
    )
    wait_applied(session, applied_before, "delete applied")
    wait_for_entity_count(session, count_before, "the delete removed one entity")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), r'[^\n]*detail="-"'),
        60,
        "the selection cleared with the delete",
        begin,
    )

    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="delete entity"'),
        30,
        "the delete undid",
        begin,
    )

    state.applied_before = applied_before
    state.begin = begin
    state.count_before = count_before


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_systems_components_and_entities_steps(context.state)


AREA = StepArea(
    name="systems_components_and_entities",
    order=7,
    run=_area_entry,
    needs=("cube_meta", "flip_before", "gizmo_entity", "scene_height", "scene_x", "scene_y",),
    provides=("count_before",),
)
