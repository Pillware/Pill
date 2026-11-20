"""The editor suite's popouts and reloads steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re

from core import log_layout
from core.editor_log_tokens import (
    DOCK_DUMP_PATTERN,
    DOCK_LEAF_PATTERN,
    DOCK_POPOUT_TOKEN,
    EDITOR_THEME_APPLIED_TOKEN,
    EGUI_SHELL_LOADED_TOKEN,
    EGUI_TAB_UNDOCKED_TOKEN,
    EXTENSION_RELOAD_TOKEN,
    FIELD_TOKEN,
    HISTORY_PUSH_TOKEN,
    HOT_PATCH_LIVE_PATTERN,
    POPOUT_CLOSED_TOKEN,
    POPOUT_MOVED_TOKEN,
    POPOUT_OPENED_TOKEN,
    POPOUT_PRESENTED_TOKEN,
    SCENE_VIEW_REBOUND_TOKEN,
    SHELL_CONTEXT_CLAIMED_TOKEN,
    TOAST_PREAMBLE,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    dump_theme,
    latest_detail_entity,
    occurrence_count,
    shell_frame,
    wait_for_frames_past,
)


def run_popouts_and_reloads_steps(state: StepState) -> None:
    """The steps through: the theme survived the shell reload without a re-install."""
    session = state.session

    assert 'widget="float-slider"' in state.slider, (
        f"the range decorator did not become a slider: {state.slider}"
    )
    assert 'tooltip="How far the shape leans towards the camera"' in state.slider, (
        f"the tooltip did not reach the row: {state.slider}"
    )
    clamped = next(
        rest for kind, _, rest in state.showcase_rows
        if kind == "field" and 'field="scale"' in rest
    )
    assert 'widget="float-drag"' in clamped, (
        f"the minimum decorator did not clamp the drag value: {clamped}"
    )
    assert not any(
        'field="internal_id"' in rest for _, _, rest in state.showcase_rows
    ), f"the hidden field drew a row: {state.showcase_rows}"
    print_ok("the Inspector drew the decorated rows and hid the hidden field")

    # A slider edit is one recorded step, like any other field write, and
    # the value it wrote answers the field dump in the *same* append: a
    # scripted line's batch is applied before the next line runs.
    slider_entity = latest_detail_entity(session)
    begin = session.length()
    session.send_many(
        [
            f"edit set {slider_entity} ShowcaseComponent lean 0.25",
            f"dump field {slider_entity} ShowcaseComponent lean",
        ]
    )
    session.wait(re.escape(FIELD_TOKEN), 30, "the edited field", begin)
    dump = session.read_since(begin)
    assert re.search(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="set ShowcaseComponent\.lean"'),
        dump,
    ), f"the slider edit recorded no step: {dump[-400:]}"
    assert "0.25" in dump, f"the edit did not reach the field: {dump[-400:]}"
    print_ok("a decorated field edit records and reads back in one append")

    # Step 5: the dock rearranges - an undocked tab becomes a second
    # dock surface - through the scripted channel.
    session.send("dump tabs")
    dump = session.wait_for_match(DOCK_DUMP_PATTERN, 30, "dock dump")
    assert re.search(r"surfaces=1\b", dump), f"the dock did not start with one surface: {dump}"
    session.send("undock entities")
    session.wait_for_match(re.escape(EGUI_TAB_UNDOCKED_TOKEN), 30, "undock")
    session.send("dump tabs")
    second = session.wait_occurrence(DOCK_DUMP_PATTERN, 2, 30, "dock dump after undock")
    assert re.search(r"surfaces=2\b", second), (
        "the undocked tab did not become a second dock surface: {}".format(second)
    )
    print_ok("dock rearranged, two surfaces")

    # Step 5b: a tab pops out into its own window. The
    # window draws the tab in its own egui context, so the tab leaves the
    # dock's tree and keeps drawing; closing it or asking for a redock
    # brings it back. The window survives a shell reload, which Step 7
    # checks by reloading the shell while one is open.
    begin = session.length()
    session.send("popout inspector")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_MOVED_TOKEN), r'\s+tab="inspector"\s+redock=false'),
        30,
        "the Inspector popped out",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_OPENED_TOKEN), r'\s+tab="inspector"'),
        60,
        "the pop-out window opened",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_PRESENTED_TOKEN), r'\s+tab="inspector"'),
        60,
        "the pop-out window presented a frame",
        begin,
    )
    popout_dump = session.request(
        "dump tabs", DOCK_DUMP_PATTERN, 30, "dock dump with a pop-out"
    )
    assert re.search(
        log_layout.message_then_fields_pattern(re.escape(DOCK_POPOUT_TOKEN), r'\s+tab="inspector"'), popout_dump
    ), "the dock did not report the popped-out tab: {}".format(popout_dump[-600:])
    held = [
        tabs
        for tabs in re.findall(DOCK_LEAF_PATTERN, popout_dump)
        if "inspector" in tabs[3].split(",")
    ]
    assert not held, "the popped-out tab stayed in the dock's tree: {}".format(held)
    print_ok("the Inspector left the dock for its own window")

    # The way back: the same dock rule, driven from the scripted channel.
    begin = session.length()
    session.send("popout inspector redock")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_MOVED_TOKEN), r'\s+tab="inspector"\s+redock=true'),
        30,
        "the Inspector redocked",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_CLOSED_TOKEN), r'\s+tab="inspector"'),
        60,
        "the pop-out window closed",
        begin,
    )
    back = session.request("dump tabs", DOCK_DUMP_PATTERN, 30, "dock dump after redock")
    assert any(
        "inspector" in tabs[3].split(",")
        for tabs in re.findall(DOCK_LEAF_PATTERN, back)
    ), "the redocked tab did not return to the dock: {}".format(back[-600:])
    print_ok("the Inspector came back to the dock")

    # A scene view popped out needs a window of its own: a child window
    # can never leave the editor's monitor, so the frontend rebuilds the
    # view on a top-level window placed at the pop-out's own rectangle.
    # Redocking gives it back its child window.
    begin = session.length()
    session.send("popout scene")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_VIEW_REBOUND_TOKEN), r"\s+surface=\d+\s+popped=true"),
        60,
        "the popped-out scene view moved to a top-level window",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_PRESENTED_TOKEN), r'\s+tab="scene"'),
        60,
        "the scene pop-out presented a frame",
        begin,
    )
    session.send("popout scene redock")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SCENE_VIEW_REBOUND_TOKEN), r"\s+surface=\d+\s+popped=false"),
        60,
        "the scene view went back to its child window",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_CLOSED_TOKEN), r'\s+tab="scene"'),
        60,
        "the scene pop-out closed",
        begin,
    )
    print_ok("the scene view followed its pop-out and came back")

    # One pop-out stays open across Step 7's shell reload: a generation
    # that does not know it must clear the window's egui data, or the new
    # shell reads a value the unloaded image created.
    begin = session.length()
    session.send("popout assets")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_MOVED_TOKEN), r'\s+tab="assets"\s+redock=false'),
        30,
        "the Assets tab popped out",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(POPOUT_PRESENTED_TOKEN), r'\s+tab="assets"'),
        60,
        "the Assets pop-out presented a frame",
        begin,
    )
    print_ok("a pop-out is open for the reload to keep")

    # Step 6: body edits patch live. A tab body calls its siblings by bare
    # name and `run_frame` reads a module constant and calls the menu bar,
    # so a patch crate only compiles because the crate root re-exports the
    # shell module: without it the host refuses the patch and reloads,
    # which is why this step demands LIVE where it used to accept either.
    highest = shell_frame(session)
    # The anchor carries the file's own line ending: the text is written
    # back byte for byte, and a translated anchor would either miss or
    # rewrite every line's ending.
    newline = "\r\n" if "\r\n" in state.original_tab_source else "\n"
    # Two lines inside the tab's row loop: the loop header and the first
    # statement of its body, so the probe lands where a statement is legal.
    tab_body = newline.join(
        (
            "        for entry in &snapshot.entities[visible_rows] {",
            "            let entity = entry.entity;",
        )
    )
    assert tab_body in state.original_tab_source, "the Entities tab body moved; update the step"
    patched_tab = state.original_tab_source.replace(
        tab_body,
        newline.join(
            (
                "        for entry in &snapshot.entities[visible_rows] {",
                "            // A patch probe: one statement, inside the tab body only.",
                "            let _hot_body_probe = 0u8;",
                "            let entity = entry.entity;",
            )
        ),
        1,
    )
    begin = session.length()
    session.edit_source(state.tab_entities_source, patched_tab)
    session.wait(
        HOT_PATCH_LIVE_PATTERN.format("entities_tab"),
        180,
        "the tab body patched live",
        begin,
    )
    highest = wait_for_frames_past(session, highest, 120, "frame counter continued")
    print_ok(f"tab body patched live, frame counter continued past {highest}")

    # The Systems tab's body names `icons` and `systems` by bare name, both
    # of which only `shell::prelude::*` publishes: a patch crate replaying
    # this file's opening compiles only because that glob is there. The
    # probe goes inside the header closure, where a statement is legal.
    systems_anchor = (
        "        let enabled_system_count = snapshot.systems.iter().filter(|row| row.enabled).count();"
    )
    assert (
        systems_anchor in state.original_systems_source
    ), "the Systems tab body moved; update the step"
    patched_systems = state.original_systems_source.replace(
        systems_anchor,
        newline.join(
            (
                "        // A patch probe: one statement, inside the tab body only.",
                "        let _hot_systems_probe = 0u8;",
                systems_anchor,
            )
        ),
        1,
    )
    begin = session.length()
    session.edit_source(state.tab_systems_source, patched_systems)
    session.wait(
        HOT_PATCH_LIVE_PATTERN.format("systems_tab"),
        180,
        "the Systems tab body patched live",
        begin,
    )
    highest = wait_for_frames_past(session, highest, 120, "frame counter continued")
    print_ok(
        "Systems tab body patched live, frame counter continued past "
        f"{highest}"
    )

    # The same for a body in `run_frame`: the heartbeat cadence, a read of
    # `SLOW_UPDATE_MICROS` and a bare call to the menu bar all sit in it.
    cadence = re.search(r"is_multiple_of\((\d+)\)", state.original_source)
    assert cadence, "the shell's heartbeat cadence was not found"
    patched_frame = state.original_source.replace(
        cadence.group(0), "is_multiple_of({})".format(int(cadence.group(1)) + 1), 1
    )
    begin = session.length()
    session.edit_source(state.shell_source, patched_frame)
    session.wait(
        HOT_PATCH_LIVE_PATTERN.format("run_frame"),
        180,
        "the frame body patched live",
        begin,
    )
    print_ok("frame body patched live")

    # The tab dispatch itself: this body lived in `impl TabViewer for
    # EditorTabViewer<'_>` until it moved to the free `tab_body`, and the
    # scanner refuses an impl header carrying a lifetime, so a patch could
    # never address it there. The probe sits in the Scene arm, the first
    # one, where a statement is legal. This anchor is in the shell's own
    # file, so it carries that file's endings rather than the tab body's:
    # the two are written by different tools and need not agree.
    shell_newline = "\r\n" if "\r\n" in state.original_source else "\n"
    dispatch_anchor = shell_newline.join(
        (
            "        EditorTab::Scene(view) => {",
            "            let view = view as usize;",
        )
    )
    assert dispatch_anchor in patched_frame, "the tab dispatch moved; update the step"
    patched_dispatch = patched_frame.replace(
        dispatch_anchor,
        shell_newline.join(
            (
                "        EditorTab::Scene(view) => {",
                "            // A patch probe: one statement, inside the dispatch only.",
                "            let _hot_dispatch_probe = 0u8;",
                "            let view = view as usize;",
            )
        ),
        1,
    )
    begin = session.length()
    session.edit_source(state.shell_source, patched_dispatch)
    session.wait(
        HOT_PATCH_LIVE_PATTERN.format("tab_body"),
        180,
        "the tab dispatch patched live",
        begin,
    )
    highest = wait_for_frames_past(session, highest, 120, "frame counter continued")
    print_ok(
        "tab dispatch patched live, frame counter continued past "
        f"{highest}"
    )

    # Step 7: a structural edit of the shell reloads it; the rearranged
    # dock (a second surface) must survive the reload. An item added
    # outside every hot body cannot be patched at all, so the host reloads
    # and the waits below count occurrences rather than assuming how many
    # happened.
    structural = patched_dispatch + (
        "\n/// A marker the egui suite adds so the module grows an item and "
        "must reload, not patch.\npub fn egui_suite_marker() -> u32 {\n    1\n}\n"
    )
    # Every byte the reload writes lands after this offset, so the toast
    # waits below cannot be satisfied by an earlier patch's notice.
    toast_start = session.length()
    # Count both tokens BEFORE the edit: the reloaded module's own loaded
    # line is logged before the reload's processed line, so counting the
    # loaded token after waiting for the reload would already include it
    # and wait for one occurrence that never comes.
    reloads = occurrence_count(session, re.escape(EXTENSION_RELOAD_TOKEN))
    loads = occurrence_count(session, re.escape(EGUI_SHELL_LOADED_TOKEN))
    claims = occurrence_count(session, re.escape(SHELL_CONTEXT_CLAIMED_TOKEN))
    # The theme the executable resolved before the reload: a reloaded
    # shell seeds its applied generation from the record, so it installs
    # nothing again.
    applies = occurrence_count(session, re.escape(EDITOR_THEME_APPLIED_TOKEN))
    session.edit_source(state.shell_source, structural)
    session.wait_occurrence(
        re.escape(EXTENSION_RELOAD_TOKEN), reloads + 1, 180, "shell reload"
    )
    session.wait_occurrence(
        re.escape(EGUI_SHELL_LOADED_TOKEN), loads + 1, 180, "shell reloaded"
    )
    # The reloaded generation claims the shared egui context before it
    # draws: whatever the previous generation left in egui's temporary
    # data would be a read through an unloaded image.
    session.wait_occurrence(
        re.escape(SHELL_CONTEXT_CLAIMED_TOKEN), claims + 1, 60, "context claim"
    )
    # The reload's own notice reaches the toast stack, with the
    # level the host assigned and the timing its build reported.
    session.wait(
        TOAST_PREAMBLE.format("info") + r'[^"]*reloaded in \d+ ms',
        60,
        "reload toast",
        toast_start,
    )
    after = session.request(
        "dump tabs", DOCK_DUMP_PATTERN, 30, "dock dump after shell reload"
    )
    assert re.search(r"surfaces=2\b", after), (
        "the shell reload reset the dock: {}".format(after)
    )
    highest = wait_for_frames_past(
        session, highest, 120, "frame counter survived the reload"
    )
    print_ok("shell reloaded, dock and counter kept")
    # The theme was not installed again: the record already held it, and
    # the reloaded generation read its generation from there.
    assert occurrence_count(session, re.escape(EDITOR_THEME_APPLIED_TOKEN)) == applies, (
        "the shell reload installed the theme again"
    )
    themed = dump_theme(session, "theme after the shell reload")
    assert f"generation={state.font_generation}" in themed, (
        "the reloaded shell lost the resolved theme: {}".format(themed)
    )
    print_ok("the theme survived the shell reload without a re-install")

    # The pop-out that was open across the reload kept its window: the
    # new generation cleared its context before drawing into it, and the
    # dock model still lists the tab as popped out. One claim line covers
    # every context the session owns, so the wait is for the line naming
    # the popped-out window, by occurrence rather than by offset: the
    # generation before the reload had already claimed and never logged a
    # window.
    session.wait_occurrence(
        log_layout.message_then_fields_pattern(re.escape(SHELL_CONTEXT_CLAIMED_TOKEN), r"\s+entries=\d+\s+windows=1\b"),
        1,
        60,
        "the reloaded shell claimed the pop-out's context",
    )
    survived = session.request(
        "dump tabs", DOCK_DUMP_PATTERN, 30, "dock dump after the reload"
    )

    state.begin = begin
    state.highest = highest
    state.survived = survived


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_popouts_and_reloads_steps(context.state)


AREA = StepArea(
    name="popouts_and_reloads",
    order=11,
    run=_area_entry,
    needs=("font_generation", "showcase_rows", "slider",),
    provides=("highest", "survived",),
)
