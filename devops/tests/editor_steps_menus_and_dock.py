"""The editor suite's menus and dock steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    EGUI_ROWS_TOKEN,
    GIZMO_PREFERENCES_WRITTEN_TOKEN,
    HISTORY_PUSH_TOKEN,
    HISTORY_UNDO_TOKEN,
    INSPECTOR_ADD_ENTRY_PATTERN,
    INSPECTOR_ADD_OPENED_TOKEN,
    INSPECTOR_SECTION_RECT_PATTERN,
    INSPECTOR_SECTION_TOKEN,
    MENU_BAR_RECT_PATTERN,
    SCENE_GRID_PATTERN,
    SCENE_GRID_STATE_TOKEN,
    SCENE_GRID_TOKEN,
    SCENE_RECT_PATTERN,
    SELECTION_CHANGED_TOKEN,
    TAB_ACTIVATED_TOKEN,
    VIEWPORT_DUMP_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    entity_count,
    inspector_add_rect,
    inspector_rows,
    inspector_section_rect,
    latest_detail_components,
    latest_detail_entity,
    menu_click,
    menu_title_rect,
    rect_center,
    toolbox_click,
    visible_scene_rect,
    wait_for_detail_components,
    wait_for_entity_count,
)


def run_menus_and_dock_steps(state: StepState) -> None:
    """The steps through: the Add Component button added {chosen} and the suite removed it."""
    session = state.session


    # Back to the first viewport's tab: the steps below read the primary
    # scene's viewport and its dock.
    begin = session.length()
    session.send("tab scene")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="scene"'),
        30,
        "the scene tab back again",
        begin,
    )

    # Step 4g: the infinite scene grid - the view-bound scene
    # viewports draw it under the world; the toolbox's grid row flips the
    # renderer's one switch, and the resolution names only the scene
    # surfaces (the game viewport never draws it).
    step_begin = session.length()
    session.send("dump grid")
    session.wait(re.escape(SCENE_GRID_TOKEN), 30, "the grid dumped", step_begin)
    on_line = session.find(SCENE_GRID_PATTERN, step_begin)
    assert on_line, session.read_since(begin)[-400:]
    assert on_line.group(1) == "true" and on_line.group(3) == "1,2,3", on_line.group(0)
    print_ok(f"the grid is on over the scene viewports ({on_line.group(3)})")
    begin = session.length()
    toolbox_click(session, 0, "grid", "the toolbox turned the grid off")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_GRID_STATE_TOKEN), r"\s+enabled=false"),
        30,
        "the grid switched off",
        begin,
    )
    begin = session.length()
    session.send("dump grid")
    session.wait(re.escape(SCENE_GRID_TOKEN), 30, "the grid dumped again", begin)
    off_line = session.find(SCENE_GRID_PATTERN, begin)
    assert off_line and off_line.group(1) == "false" and off_line.group(3) == "", (
        "the off grid still names surfaces: {}".format(
            off_line.group(0) if off_line else session.read_since(begin)[-400:]
        )
    )
    print_ok("the grid switched off and stopped naming surfaces")
    begin = session.length()
    toolbox_click(session, 0, "grid", "the toolbox turned the grid back on")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_GRID_STATE_TOKEN), r"\s+enabled=true"),
        30,
        "the grid switched back on",
        begin,
    )
    picks = session.read_since(step_begin)
    assert "scene pick" not in picks, (
        "a grid row click reached the scene: {}".format(picks[-500:])
    )
    print_ok("the grid toggles through the toolbox row")

    # Step 4h: the menu bar - the pill mark and the six titles
    # above the dock. Every item runs the action its panel runs, so the
    # assertions are the same tokens: a tab activated, a recorded create
    # and its undo, the snapping switch.
    bar = session.find(MENU_BAR_RECT_PATTERN)
    assert bar, "the menu bar never reported its rectangle"
    bar_rect = (float(bar.group(1)), float(bar.group(2)), float(bar.group(3)))
    bar_height = float(bar.group(4))
    assert bar_rect[1] < 10.0, f"the bar is not at the top of the window: {bar_rect}"
    assert bar_rect[2] > 200.0, f"the bar is too narrow to hold the menus: {bar_rect}"
    assert bar_height >= 20.0, f"the bar is too short to hold the mark: {bar_height}"
    titles = {name: menu_title_rect(session, name) for name in
              ("file", "edit", "create", "view", "window", "help")}
    ordered = sorted(titles.items(), key=lambda item: item[1][0])
    assert [name for name, _ in ordered] == ["file", "edit", "create", "view", "window", "help"], (
        "the titles are out of Unity's order: {}".format(ordered)
    )
    first_title_left = titles["file"][0]
    # The mark sits at x=10 and is 18 wide, then the gap: a title inside
    # that room would be drawn over the logo.
    assert first_title_left >= 30.0, (
        "the mark's room is missing before the first title: {}".format(first_title_left)
    )
    print_ok(f"the bar holds the mark and six titles from x={first_title_left:.0f}")

    # The Window menu brings the Systems tab forward - the same
    # activation the scripted `tab` command takes.
    begin = session.length()
    menu_click(session, "window", "Systems", "Systems")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="systems"'),
        30,
        "the Window menu activated the Systems tab",
        begin,
    )

    # Create -> New Entity records an entity; Edit -> Undo takes it back.
    count_before = entity_count(session)
    begin = session.length()
    menu_click(session, "create", "New Entity", "New Entity")
    wait_for_entity_count(session, count_before + 1, "the Create menu's new entity")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="new entity"'),
        30,
        "the Create menu's step recorded",
        begin,
    )
    begin = session.length()
    menu_click(session, "edit", "Undo", "Undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="new entity"'),
        30,
        "the Edit menu undid it",
        begin,
    )
    wait_for_entity_count(session, count_before, "the undo removed the menu's entity")
    print_ok("the Create and Edit menus round-trip a recorded step")

    # View -> Snap flips the same switch the toolbox's row does, twice, so
    # the suite leaves it where it found it.
    begin = session.length()
    menu_click(session, "view", "Snap", "Snap")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_PREFERENCES_WRITTEN_TOKEN), r"[^\n]*snap=true"),
        30,
        "the View menu turned snapping on",
        begin,
    )
    begin = session.length()
    menu_click(session, "view", "Snap", "Snap")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_PREFERENCES_WRITTEN_TOKEN), r"[^\n]*snap=false"),
        30,
        "the View menu turned snapping back off",
        begin,
    )
    picks = session.read_since(step_begin)
    assert "scene pick" not in picks, (
        "a menu item click reached the scene: {}".format(picks[-500:])
    )
    print_ok("the View menu drives the gizmo's snapping, and its menus stay off the scene")

    # Step 4i: the Unity-style Inspector - one collapsible
    # section per component (a caret, a duotone icon, the label), typed
    # rows, and the Add Component button in the tab's top-right.
    begin = session.length()
    session.send("tab inspector")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="inspector"'),
        30,
        "the Inspector tab shown",
        begin,
    )
    # A live handle: the rows heartbeat names the entity the Inspector
    # describes, and re-selecting it is the recorded path the entities
    # list takes.
    detail_entity = latest_detail_entity(session)
    begin = session.length()
    session.send(f"select entity {detail_entity}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CHANGED_TOKEN), rf"\s+entity={re.escape(detail_entity)}"),
        30,
        "an entity selected for the Inspector",
        begin,
    )
    section_begin = session.length()
    session.send("dump viewport")
    session.wait(re.escape(VIEWPORT_DUMP_TOKEN), 30, "a frame drawn", section_begin)
    # The rectangles log once per placement (and again whenever the
    # layout moves them), so a section drawn since the editor started is
    # read from the whole log, not from this window. The Inspector is a
    # narrow column that scrolls, so only its top is guaranteed to be
    # visible: the section whose logged header sits highest is the one a
    # scripted click can always reach.
    section_rects = [
        (
            match.group(1),
            float(match.group(2)),
            float(match.group(3)),
            float(match.group(4)),
            float(match.group(5)),
        )
        for match in re.finditer(INSPECTOR_SECTION_RECT_PATTERN, session.text())
    ]
    assert section_rects, "the Inspector never reported a section's rectangle"
    sections = [entry[0] for entry in section_rects]
    # Ties on the smallest y are broken by the latest log entry: the most
    # recent placement is the one the panel is showing.
    topmost = min(
        enumerate(section_rects), key=lambda entry: (entry[1][2], -entry[0])
    )[1]
    transform_section = topmost[0]
    print_ok(
        f"the Inspector draws {len(set(sections))} section(s), "
        f"toggling {transform_section}"
    )

    # A header click collapses its section; a second brings it back. The
    # click lands on the bar's upper half, which stays inside the panel's
    # visible viewport even when the header is partly scrolled out.
    def header_click_point() -> tuple:
        rect = inspector_section_rect(session, transform_section)
        return (rect[0] + 12.0, rect[1] + 6.0)

    center = header_click_point()
    begin = session.length()
    session.send(f"click {center[0]:.0f} {center[1]:.0f}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(INSPECTOR_SECTION_TOKEN), rf'\s+component="{re.escape(transform_section)}"\s+collapsed=true'),
        30,
        "the section collapsed",
        begin,
    )
    # egui answers a click from the previous frame's widget rectangles, and
    # collapsing a section reshapes everything below it. A frame with no
    # click in flight lets the header settle as the pointer's target before
    # the second click arrives.
    settle = session.length()
    session.send("dump viewport")
    session.wait(re.escape(VIEWPORT_DUMP_TOKEN), 30, "a settling frame", settle)
    begin = session.length()
    center = header_click_point()
    session.send(f"click {center[0]:.0f} {center[1]:.0f}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(INSPECTOR_SECTION_TOKEN), rf'\s+component="{re.escape(transform_section)}"\s+collapsed=false'),
        30,
        "the section expanded again",
        begin,
    )
    print_ok("a section collapses and expands from its header")

    # The Add Component button lists what the entity lacks; a pick adds
    # one recorded component, which the suite then removes again. The
    # click that opens the list can also land as a pick through the dock
    # and move the selection, so the count and the entity are read from a
    # heartbeat drawn for the opened list, not from before the click.
    add = rect_center(inspector_add_rect(session))
    begin = session.length()
    session.send(f"click {add[0]:.0f} {add[1]:.0f}")
    session.wait(re.escape(INSPECTOR_ADD_OPENED_TOKEN), 30, "the add list opened", begin)
    session.wait(
        INSPECTOR_ADD_ENTRY_PATTERN, 30, "the add list reported its entries", begin
    )
    entries = [
        match.group(1)
        for match in re.finditer(INSPECTOR_ADD_ENTRY_PATTERN, session.read_since(begin))
    ]
    assert entries, "the add list was empty"
    # The detail counts below come from the rows heartbeat, so wait for a
    # heartbeat that carries a selected detail - a bare frame advance can
    # land before the shell reports the new one.
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), r'[^\n]*detail="(?!-)[^"]+"'),
        60,
        "a heartbeat after the add list opened",
        begin,
    )
    components_before = latest_detail_components(session)
    detail_entity = latest_detail_entity(session)
    chosen = entries[0]
    entry_rect_matches = [
        match
        for match in re.finditer(INSPECTOR_ADD_ENTRY_PATTERN, session.read_since(begin))
        if match.group(1) == chosen
    ]
    entry = entry_rect_matches[-1]
    entry_center = (
        float(entry.group(2)) + float(entry.group(4)) / 2.0,
        float(entry.group(3)) + float(entry.group(5)) / 2.0,
    )
    begin = session.length()
    session.send(f"click {entry_center[0]:.0f} {entry_center[1]:.0f}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="add '),
        30,
        "the add recorded a step",
        begin,
    )
    wait_for_detail_components(
        session,
        components_before + 1,
        f"the added component reached the section of {detail_entity}",
    )
    begin = session.length()
    session.send(f"edit remove {detail_entity} {chosen}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="remove '),
        30,
        "the suite removed the added component again",
        begin,
    )
    wait_for_detail_components(
        session, components_before, "the removal took it back out"
    )
    print_ok(f"the Add Component button added {chosen} and the suite removed it")

    # Step 4j: the field decorators - a component whose fields
    # declare [Header]/[Space]/[Tooltip]/[Range]/[Min]/[HideInInspector]
    # draws the rows the model chose: a separator line, a gap, a slider
    # carrying its tooltip, a clamped drag value, and no row at all for
    # the hidden field. `scene`'s models carry the decorated
    # `ShowcaseComponent`, so the step picks one the way the scene
    # selection step does.
    begin = session.length()
    session.send("tab inspector")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="inspector"'),
        30,
        "the Inspector tab shown again",
        begin,
    )
    scenes = session.request(
        "dump scenes", SCENE_RECT_PATTERN, 30, "the scene rects dumped"
    )
    viewport = visible_scene_rect(scenes)
    assert viewport, "no visible scene view rect for the decorator step"
    showcase_rows = []
    for fraction_x, fraction_y in ((0.5, 0.5), (0.35, 0.4), (0.65, 0.4)):
        x = int(viewport[0] + viewport[2] * fraction_x)
        y = int(viewport[1] + viewport[3] * fraction_y)
        probe = session.length()
        session.send(f"click {x} {y}")
        session.send("dump viewport")
        session.wait(re.escape(VIEWPORT_DUMP_TOKEN), 30, "a frame drawn", probe)
        showcase_rows = inspector_rows(session, "ShowcaseComponent")
        if showcase_rows:
            break
    assert showcase_rows, (
        "no decorated component reached the Inspector; the scene probe never "
        "selected a model carrying ShowcaseComponent"
    )
    kinds = [kind for kind, _, _ in showcase_rows]
    assert "header" in kinds, f"no header row was drawn: {showcase_rows}"
    assert "space" in kinds, f"no gap row was drawn: {showcase_rows}"
    header = next(rest for kind, _, rest in showcase_rows if kind == "header")
    assert 'text="Frame"' in header, f"the header text did not reach the row: {header}"
    space = next(rest for kind, _, rest in showcase_rows if kind == "space")
    assert "pixels=6" in space, f"the gap height did not reach the row: {space}"
    slider = next(
        rest for kind, _, rest in showcase_rows
        if kind == "field" and 'field="lean"' in rest
    )

    state.begin = begin
    state.showcase_rows = showcase_rows
    state.slider = slider


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_menus_and_dock_steps(context.state)


AREA = StepArea(
    name="menus_and_dock",
    order=10,
    run=_area_entry,
    needs=(),
    provides=("showcase_rows", "slider",),
)
