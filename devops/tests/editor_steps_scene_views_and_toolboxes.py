"""The editor suite's scene views and toolboxes steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    ACTIVE_VIEWPORT_TOKEN,
    CAMERA_GESTURE_TOKEN,
    CAMERA_ZOOM_TOKEN,
    EDITOR_FONTS_INSTALLED_TOKEN,
    EDITOR_FONT_REFUSED_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    GAME_RECT_PATTERN,
    GIZMO_ANCHOR_TOKEN,
    POINTER_DRAG_TOKEN,
    SCENE_RECT_PATTERN,
    SCENE_SURFACE_TOKEN,
    SCENE_VIEW_ADDED_TOKEN,
    SCENE_VIEW_OPENED_TOKEN,
    SCENE_VIEW_RECT_TOKEN,
    TAB_ACTIVATED_TOKEN,
    TOOLBOX_ITEM_TOKEN,
    TOOLBOX_MOVED_TOKEN,
    TOOLBOX_RECT_TOKEN,
    VIEWPORT_CAMERA_TOKEN,
    VIEWPORT_DUMP_TOKEN,
)
from core.projects import THEMED_PROJECT_FONT
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    assert_theme,
    dump_theme,
    logged_scene_rect,
    occurrence_count,
    rect_center,
    rect_contains,
    toolbox_click,
    toolbox_panel,
    toolbox_row,
    wait_applied,
    write_theme_file,
)


def run_scene_views_and_toolboxes_steps(state: StepState) -> None:
    """The steps through: three scene views hold three independent cameras."""
    session = state.session

    # The theme file below names a project font. The Preferences area puts the
    # file under `res/editor/fonts/` in a full run, so a subset that skips that
    # area writes it here: an absent font is refused, and the step would then
    # read the built-in signature as its own baseline.
    font_directory = state.theme_root / "fonts"
    font_directory.mkdir(parents=True, exist_ok=True)
    themed_font_path = font_directory / THEMED_PROJECT_FONT.name
    if not themed_font_path.is_file():
        themed_font_path.write_bytes(THEMED_PROJECT_FONT.read_bytes())

    write_theme_file(
        state.theme_root / "font.theme",
        "000000000000000000000000000000b3",
        {
            "palette": {"accent": "#33d17a"},
            "fonts": {"body": "editor/fonts/missing.ttf"},
        },
    )
    state.editor_settings_path.write_text(
        'editor_theme: "editor/font.theme"\n', encoding="utf-8"
    )
    begin = session.length()
    session.wait(
        re.escape(EDITOR_FONT_REFUSED_TOKEN) + r"[^\n]*missing\.ttf",
        60,
        "the missing font refused by name",
        begin,
    )
    # The font that is there installs, and the signature is not the one the
    # built-in families alone produce.
    installed_before = re.findall(
        r"signature=([0-9a-f]{16})", session.text()
    )[-1]
    write_theme_file(
        state.theme_root / "font.theme",
        "000000000000000000000000000000b3",
        {
            "palette": {"accent": "#33d17a"},
            "fonts": {"body": "editor/fonts/Silkscreen-Regular.ttf"},
        },
    )
    begin = session.length()
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(EDITOR_FONTS_INSTALLED_TOKEN), r"\s+signature=([0-9a-f]{16})"),
        60,
        "the project's font installed",
        begin,
    )
    installed_after = re.findall(r"signature=([0-9a-f]{16})", session.read_since(begin))[-1]
    assert installed_after != installed_before, (
        "the installed font set did not change the signature: {}".format(
            (installed_before, installed_after)
        )
    )
    assert_theme(
        dump_theme(session, "the project font dumped"),
        "code,project",
        "#33d17a",
        "#1a1a1d",
        "the font's layer kept its own accent and the editor's panel",
    )
    print_ok("a project font loads, installs and refuses with its name")
    font_generation = int(
        re.search(r"generation=(\d+)", dump_theme(session, "the font step's last dump")).group(1)
    )

    # Step 4e: the Game tab's viewport - the primary
    # surface's child window sits at the active tab's body, so the
    # project shows through it, and hides when another tab of the
    # group is active.
    def wait_game_rect(visible: bool, label: str) -> "re.Match":
        """Dumps the scene rects until the game line reports `visible`.

        The window follows the tab on the next layout pass, so a dump may
        reach the log before the placement did; each dump is its own
        anchored request, so no sleep stands in for the answer.
        """
        wanted = "true" if visible else "false"

        def probe(_text: str):
            scenes = session.request(
                "dump scenes", SCENE_RECT_PATTERN, 30, "the scene dump"
            )
            for match in re.finditer(GAME_RECT_PATTERN, scenes):
                if match.group(5) == wanted:
                    return match
            return None

        return session.wait_until(
            probe,
            30,
            f"{label}: the game view rect never reported visible={wanted}\n",
            label=label,
            # Each round waits for its own dump; the outer poll only spaces
            # the rounds out.
            poll=0.2,
        )

    begin = session.length()
    session.send("tab game")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="game"'),
        30,
        "the game tab activated",
        begin,
    )
    game = wait_game_rect(True, "the game viewport placed and visible")
    assert int(game.group(3)) > 0 and int(game.group(4)) > 0, game.group(0)
    print_ok(f"the game viewport sits at {game.group(3)}x{game.group(4)}")
    scenes = session.request(
        "dump scenes", SCENE_RECT_PATTERN, 30, "the scene rects behind the game tab"
    )
    scene_lines = list(re.finditer(SCENE_RECT_PATTERN, scenes))
    assert scene_lines and all(match.group(6) == "false" for match in scene_lines), (
        "the scene viewports did not hide behind the game tab: {}".format(scenes[-600:])
    )
    print_ok("the scene viewports hide behind the game tab")
    begin = session.length()
    session.send("tab scene")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="scene"'),
        30,
        "the scene tab back",
        begin,
    )
    wait_game_rect(False, "the game view hidden with its tab")

    # Step 4f: the floating toolbox - the gizmo's tools, its
    # space and its snapping as a panel over the scene viewport. A row
    # click switches the tool without picking the scene, the panel is
    # draggable inside the viewport and keeps taking input where it
    # lands, and the second viewport carries its own.
    step_begin = session.length()
    session.wait(log_layout.message_then_fields_pattern(re.escape(TOOLBOX_RECT_TOKEN), r"\s+view=0"), 30, "the toolbox placed")
    session.wait(re.escape(TOOLBOX_ITEM_TOKEN), 30, "the toolbox rows reported")
    panel = toolbox_panel(session, 0)
    viewport = logged_scene_rect(session, 0)
    assert rect_contains(viewport, panel), (
        f"the toolbox {panel} is not inside the scene viewport {viewport[:4]}"
    )
    row = toolbox_row(session, 0, "translate")
    assert rect_contains(panel, row), f"the translate row {row} is outside the panel {panel}"
    print_ok(f"the toolbox floats at {panel} inside the viewport")

    # The rows act: each click queues the same `SetGizmoTool` edit the
    # viewport menu's items queue, and the anchor line carries the state
    # the edit left - the clicks read entry mode=rotate from step 4d, so
    # every switch below is a real change.
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    toolbox_click(session, 0, "translate", "the toolbox switched to translate")
    wait_applied(session, applied_before, "the toolbox tool applied")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), r'[^\n]*mode="translate"'),
        60,
        "the anchor carries the toolbox's tool",
        begin,
    )
    for item, expected, label in (
        ("snap", "snapped=true", "the toolbox turned snapping on"),
        ("snap", "snapped=false", "the toolbox turned snapping off"),
        ("space", 'space="local"', "the toolbox switched the space to local"),
        ("space", 'space="world"', "the toolbox switched the space back"),
    ):
        begin = session.length()
        toolbox_click(session, 0, item, label + " click")
        session.wait(re.escape(GIZMO_ANCHOR_TOKEN) + r"[^\n]*" + expected, 60, label, begin)
    print_ok("the toolbox rows switch the tool")

    # The handle drags the panel: the runner injects the press, the
    # move and the release one frame apart, and the shell logs where the
    # panel landed.
    handle = toolbox_row(session, 0, "handle")
    start_x, start_y = rect_center(handle)
    rect_before = toolbox_panel(session, 0)
    begin = session.length()
    session.send(f"drag {start_x:.0f} {start_y:.0f} {start_x + 120:.0f} {start_y + 80:.0f}")
    session.wait(re.escape(POINTER_DRAG_TOKEN), 30, "the drag completed", begin)
    session.wait(re.escape(TOOLBOX_MOVED_TOKEN), 30, "the toolbox moved", begin)
    panel_after = toolbox_panel(session, 0)
    moved_x = panel_after[0] - rect_before[0]
    moved_y = panel_after[1] - rect_before[1]
    assert abs(moved_x - 120) <= 2 and abs(moved_y - 80) <= 2, (
        f"the dragged toolbox moved by ({moved_x}, {moved_y}), not (120, 80)"
    )
    assert rect_contains(viewport, panel_after), (
        f"the dragged toolbox left the viewport: {panel_after}"
    )
    print_ok(f"the toolbox dragged to {panel_after}")

    # The moved panel still takes input where it landed.
    begin = session.length()
    toolbox_click(session, 0, "rotate", "the moved toolbox switched to rotate")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(GIZMO_ANCHOR_TOKEN), r'[^\n]*mode="rotate"'),
        60,
        "the anchor followed the moved toolbox",
        begin,
    )

    # The second viewport carries its own toolbox, parked at its own
    # default corner (each scene view keeps its own position).
    begin = session.length()
    session.send("tab scene2")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="scene2"'),
        30,
        "the second viewport's tab activated",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TOOLBOX_RECT_TOKEN), r"\s+view=1"),
        30,
        "the second viewport's toolbox placed",
        begin,
    )
    session.request(
        "dump scenes", SCENE_RECT_PATTERN, 30, "the second viewport's rects"
    )
    second_view = logged_scene_rect(session, 1)
    assert second_view[4], "the second viewport did not become visible"
    second_panel = toolbox_panel(session, 1)
    assert rect_contains(second_view, second_panel), (
        f"the second toolbox {second_panel} is not inside its viewport {second_view[:4]}"
    )
    assert second_panel != panel_after, "the two viewports share one toolbox position"
    begin = session.length()
    session.send("tab scene")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="scene"'),
        30,
        "the scene tab back",
        begin,
    )
    assert toolbox_panel(session, 0) == panel_after, (
        "the first toolbox did not keep the place the drag left it"
    )
    # No toolbox click picked the scene: every one landed in the panel's
    # own overlay region, whatever its position.
    picks = session.read_since(step_begin)
    assert "scene pick" not in picks, (
        "a toolbox click reached the scene: {}".format(picks[-500:])
    )
    print_ok("the second viewport's toolbox and the parked position")

    # The active viewport: a click inside a viewport makes it
    # the one camera navigation addresses, and navigation moves only
    # that view's camera.
    begin = session.length()
    session.send("tab scene2")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TAB_ACTIVATED_TOKEN), r'\s+tab="scene2"'),
        30,
        "the second viewport shown again",
        begin,
    )
    session.request(
        "dump scenes",
        SCENE_RECT_PATTERN,
        30,
        "the second viewport's rects after the tab switch",
    )
    second_rect = logged_scene_rect(session, 1)
    corner_x = second_rect[0] + 6
    corner_y = second_rect[1] + 6
    begin = session.length()
    session.send(f"click {corner_x} {corner_y}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ACTIVE_VIEWPORT_TOKEN), r"\s+view=1"),
        30,
        "the second viewport activated",
        begin,
    )
    begin = session.length()
    session.send("dump viewport")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(VIEWPORT_DUMP_TOKEN), r'\s+active=1\s+interaction="idle"'),
        30,
        "the second viewport is active",
        begin,
    )
    print_ok("a click activates the viewport it lands in")

    def viewport_cameras() -> dict:
        """The newest viewport dump's per-view camera rows."""
        begin = session.length()
        session.send("dump viewport")
        session.wait(re.escape(VIEWPORT_DUMP_TOKEN), 30, "the viewport dumped", begin)
        rows = re.findall(
            log_layout.message_then_fields_pattern(re.escape(VIEWPORT_CAMERA_TOKEN), r'\s+view=(\d+)\s+eye="([-\d.,]+)"\s+pivot="([-\d.,]+)"')
            + r"\s+distance=([-\d.eE+]+)\s+yaw=([-\d.eE+]+)\s+pitch=([-\d.eE+]+)",
            session.read_since(begin),
        )
        return {int(row[0]): row[1:] for row in rows}

    before_navigation = viewport_cameras()
    begin = session.length()
    session.send("camera orbit 60 20")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(CAMERA_GESTURE_TOKEN), r'\s+view=1\s+navigation="orbit"'),
        30,
        "the orbit applied",
        begin,
    )
    session.send("camera pan 30 10")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(CAMERA_GESTURE_TOKEN), r'\s+view=1\s+navigation="pan"'),
        30,
        "the pan applied",
        begin,
    )
    session.send("camera dolly 25")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(CAMERA_GESTURE_TOKEN), r'\s+view=1\s+navigation="dolly"'),
        30,
        "the dolly applied",
        begin,
    )
    session.send("camera zoom 3")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(CAMERA_ZOOM_TOKEN), r"\s+view=1"),
        30,
        "the wheel zoom applied",
        begin,
    )
    after_navigation = viewport_cameras()
    assert after_navigation[0] == before_navigation[0], (
        "navigation moved the inactive viewport's camera: {} -> {}".format(
            before_navigation[0], after_navigation[0]
        )
    )
    assert after_navigation[1] != before_navigation[1], (
        "navigation left the active viewport's camera where it was"
    )
    print_ok("camera navigation moved the active viewport only")

    # Step 4e2: any number of scene views. The Window
    # menu's "New Scene View" docks one more, and the frontend opens a
    # child window, a renderer surface and a camera for it - the same
    # three the two startup views get. The third view's camera then
    # moves on its own: the three cameras are independent.
    begin = session.length()
    session.send("command editor.scene_new_view")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_VIEW_ADDED_TOKEN), r'\s+tab="scene3"\s+view=2\b'),
        30,
        "the third scene view",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_VIEW_OPENED_TOKEN), r"\s+view=2\s+surface=3"),
        60,
        "the third view's child window",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_SURFACE_TOKEN), r"\s+surface=3\b"),
        60,
        "the third view's surface",
        begin,
    )
    session.send("dump scenes")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_VIEW_RECT_TOKEN), r"\s+view=2\b"),
        30,
        "the third view's rectangle",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(TOOLBOX_RECT_TOKEN), r"\s+view=2\b"),
        30,
        "the third view's toolbox",
        begin,
    )
    third_rect = logged_scene_rect(session, 2)
    assert third_rect[4], "the third viewport did not become visible"
    print_ok("a third scene view opened its own window and toolbox")

    # The third viewport becomes the active one, so navigation may move
    # it; the other two keep their cameras.
    before_third = viewport_cameras()
    begin = session.length()
    session.send(f"click {third_rect[0] + 6} {third_rect[1] + 6}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ACTIVE_VIEWPORT_TOKEN), r"\s+view=2"),
        30,
        "the third viewport activated",
        begin,
    )
    session.send("camera orbit -40 15")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(CAMERA_GESTURE_TOKEN), r'\s+view=2\s+navigation="orbit"'),
        30,
        "the third viewport's orbit",
        begin,
    )
    after_third = viewport_cameras()
    assert len(after_third) >= 3, (
        "the viewport dump does not list three views: {}".format(after_third)
    )
    assert after_third[2] != before_third[2], (
        "the third viewport's camera did not move"
    )
    assert after_third[0] == before_third[0] and after_third[1] == before_third[1], (
        "the third viewport's navigation moved another view's camera"
    )
    print_ok("three scene views hold three independent cameras")

    state.begin = begin
    state.font_generation = font_generation


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_scene_views_and_toolboxes_steps(context.state)


AREA = StepArea(
    name="scene_views_and_toolboxes",
    order=9,
    run=_area_entry,
    needs=(),
    provides=("font_generation",),
)
