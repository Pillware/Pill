"""The editor suite's assets browser and pipeline steps.

Moved verbatim from `test_editor_egui.py`; the tools it reads through come
from `editor_step_support`, the log vocabulary from `editor_log_tokens`.
"""

import re

import yaml

from core import log_layout
from core.editor_log_tokens import (
    ASSETS_DUMP_TOKEN,
    ASSET_CREATE_QUEUED_TOKEN,
    ASSET_DELETE_QUEUED_TOKEN,
    ASSET_DELETE_REFUSED_TOKEN,
    ASSET_DIALOG_CLOSED_TOKEN,
    ASSET_DIALOG_OPENED_TOKEN,
    ASSET_LISTING_TOKEN,
    ASSET_META_QUEUED_TOKEN,
    ASSET_READ_ONLY_ROW_TOKEN,
    ASSET_RENAME_QUEUED_TOKEN,
    ASSET_SELECTED_TOKEN,
    BROWSER_DUMP_TOKEN,
    BROWSER_FOLDER_TOKEN,
    BROWSER_ITEM_TOKEN,
    COMMAND_FAILED_TOKEN,
    ENGINE_COMMANDS_APPLIED_TOKEN,
    HISTORY_PUSH_TOKEN,
    HISTORY_REDO_TOKEN,
    HISTORY_UNDO_TOKEN,
    SELECTION_CLEARED_TOKEN,
    SELECTION_FOLLOWED_TOKEN,
    STANDALONE_TYPE_TOKEN,
)
from editor_step_areas import StepArea, StepContext
from editor_step_support import (
    print_ok,
    StepState,
    dump_field_values,
    occurrence_count,
    wait_applied,
    wait_for_path,
    wait_meta_value,
)


def run_assets_browser_and_pipeline_steps(state: StepState) -> None:
    """The steps through: {label}."""
    session = state.session

    after_plane = dump_field_values(session, state.gizmo_entity, "translation")
    assert abs(after_plane[0] - state.before_plane[0]) > 0.05 and abs(
        after_plane[1] - state.before_plane[1]
    ) > 0.05, f"the plane drag did not move both axes: {state.before_plane} -> {after_plane}"
    assert abs(after_plane[2] - state.before_plane[2]) < 1e-4, (
        f"the plane drag left the plane on z: {state.before_plane} -> {after_plane}"
    )
    print_ok(f"the XY plane drag moved {state.before_plane} -> {after_plane}")

    # Step 4: the Assets pipeline - the listing, a create through its
    # dialog, a rename whose move the selection follows, a move into a
    # folder, a delete that undoes back, and the write-meta action. The
    # dialogs are driven through the scripted channel (the toolbar and
    # dialog commands take the same paths its widgets do) and the world
    # through the log tokens and the disk.

    def listing_count() -> int:
        """The newest asset-listing count the frontend logged."""
        counts = [
            int(value)
            for value in re.findall(
                log_layout.message_then_fields_pattern(re.escape(ASSET_LISTING_TOKEN), r"\s+count=(\d+)"), session.text()
            )
        ]
        return counts[-1] if counts else 0

    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_LISTING_TOKEN), r"\s+count=(\d+)\s+types=(\d+)"),
        120,
        "asset listing refreshed",
    )
    match = re.search(
        log_layout.message_then_fields_pattern(re.escape(ASSET_LISTING_TOKEN), r"\s+count=(\d+)\s+types=(\d+)"),
        session.text(),
    )
    listed, typed = int(match.group(1)), int(match.group(2))
    assert listed >= 1, "the asset listing is empty"
    assert typed >= 1, "no standalone asset types are loaded; the create dialog cannot act"
    print_ok(f"asset listing {listed} entries, {typed} standalone types")

    begin = session.length()
    session.send("dump assets")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSETS_DUMP_TOKEN), r"\s+count=\d+"), 30, "assets dumped", begin
    )
    dump = session.read_since(begin)
    assert re.search(r'path="?textures/?"?\s+kind="?folder', dump), (
        "the listing misses the textures folder: " + dump[-800:]
    )
    staged_row = re.search(
        r'path="?s16_stage\.png"?\s+kind="?(\S+)"?\s+standalone=(\S+)\s+has_meta=(\S+)',
        dump,
    )
    assert staged_row, "the staged meta probe is not in the listing: " + dump[-800:]
    assert staged_row.group(3) == "false", "the staging probe already has a sidecar"

    # The browser is Unity's project panel: a folder tree beside a grid of
    # the open folder's items. A scripted navigation lands the grid on
    # textures, which lists that folder's own entries and nothing that
    # lives at the root.
    begin = session.length()
    session.send("browser textures")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(BROWSER_FOLDER_TOKEN), r'\s+path="(textures)"'),
        30,
        "the browser opened the textures folder",
        begin,
    )
    begin = session.length()
    session.send("dump browser")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(BROWSER_DUMP_TOKEN), r'\s+folder="textures"\s+count=(\d+)'),
        30,
        "browser dumped",
        begin,
    )
    grid = session.read_since(begin)
    assert re.search(
        log_layout.message_then_fields_pattern(re.escape(BROWSER_ITEM_TOKEN), r'\s+path="?textures/reimport_sample\.png'),
        grid,
    ), "the grid misses the textures folder's texture: " + grid[-800:]
    for outside in ("config.ini", "s16_stage.png"):
        assert not re.search(
            log_layout.message_then_fields_pattern(re.escape(BROWSER_ITEM_TOKEN), r'\s+path="?') + re.escape(outside), grid
        ), "the grid lists an entry outside the open folder: " + grid[-800:]
    print_ok("the browser's grid lists the open folder")
    # Back to the asset root, the tree's first row, for the steps below.
    begin = session.length()
    session.send("browser root")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(BROWSER_FOLDER_TOKEN), r'\s+path=""'),
        30,
        "the browser returned to the asset root",
        begin,
    )

    # A module's resources are listed beside the project's and can never be
    # written. The listing marks the root and the rule; the tree opens that
    # root; the Inspector says its section is read-only; and a delete is
    # refused with a warning instead of being queued, so no history step
    # appears to undo.
    module_rows = [
        match.groups()
        for match in re.finditer(
            r'path="?(extensions/[^\s"]+)"?\s+kind="?([^\s"]+)"?\s+standalone=\S+'
            r'\s+has_meta=\S+\s+root="?([^\s"]+)"?\s+read_only=(true|false)',
            dump,
        )
    ]
    module_files = [
        row for row in module_rows if row[1] != "folder" and row[3] == "true" and row[2]
    ]
    assert module_files, "no module resource is listed read-only: " + dump[-800:]
    module_key, _, module_root, _ = module_files[0]
    module_folder = module_key.rsplit("/", 1)[0]
    # The browser's grid names a row by its path inside its root, so the
    # root's own prefix comes off before it is matched.
    module_relative = module_key.split(f"extensions/{module_root}/", 1)[-1]
    print_ok(f"{module_root} lists {module_key} read-only")

    begin = session.length()
    session.send(f"browser {module_folder}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(BROWSER_FOLDER_TOKEN), r'\s+path="')
        + re.escape(module_folder)
        + r'"',
        30,
        "the module's folder opened",
        begin,
    )
    begin = session.length()
    session.send("dump browser")
    session.wait(re.escape(BROWSER_DUMP_TOKEN), 30, "the module folder dumped", begin)
    module_grid = session.read_since(begin)
    assert re.search(
        log_layout.message_then_fields_pattern(re.escape(BROWSER_ITEM_TOKEN), r'\s+path="?') + re.escape(module_relative), module_grid
    ), "the grid misses the module's own file: " + module_grid[-800:]

    # Select the module's file: the Inspector draws its section read-only,
    # which is the one line the tab prints for it.
    begin = session.length()
    session.send(f"select asset {module_key}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_SELECTED_TOKEN), r'\s+path="') + re.escape(module_key) + r'"',
        30,
        "the module's file selected",
        begin,
    )
    # The Inspector draws the section, so it has to be the open tab for
    # the row line to appear at all.
    session.send("tab inspector")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_READ_ONLY_ROW_TOKEN), r'\s+asset="?') + re.escape(module_key) + r'"?',
        30,
        "the Inspector drew the module asset read-only",
        begin,
    )

    # The delete is refused where the toolbar's enablement already says it
    # would be: no queued delete, no history step, and the file is still
    # listed afterwards.
    begin = session.length()
    session.send("toolbar delete")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_DELETE_REFUSED_TOKEN), r'\s+path="') + re.escape(module_key) + r'"',
        30,
        "the module asset's delete refused",
        begin,
    )
    refused_window = session.read_since(begin)
    assert ASSET_DELETE_QUEUED_TOKEN not in refused_window, (
        "a module asset's delete was queued: " + refused_window[-800:]
    )
    assert HISTORY_PUSH_TOKEN not in refused_window, (
        "the refused delete pushed a history step: " + refused_window[-800:]
    )
    begin = session.length()
    session.send("dump assets")
    session.wait(re.escape(ASSETS_DUMP_TOKEN), 30, "the listing re-dumped", begin)
    assert re.search(
        r'path="?' + re.escape(module_key), session.read_since(begin)
    ), "the refused delete removed the module asset from the listing"
    print_ok("a module asset's delete was refused with no history step")

    # Back to the Assets tab and the project's root for the steps below.
    session.send("tab assets")
    begin = session.length()
    session.send("browser root")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(BROWSER_FOLDER_TOKEN), r'\s+path=""'),
        30,
        "the browser returned to the asset root again",
        begin,
    )

    begin = session.length()
    session.send("dump standalone-types")
    session.wait(re.escape(STANDALONE_TYPE_TOKEN), 30, "standalone types dumped", begin)
    found = session.find(
        log_layout.message_then_fields_pattern(re.escape(STANDALONE_TYPE_TOKEN), r'\s+type_name="([^"]+)"\s+display="([^"]+)"\s+extension="([^"]+)"'),
        begin,
    )
    extension = found.group(3)
    created = f"textures/s16_probe.{extension}"
    created_path = state.asset_root / "textures" / f"s16_probe.{extension}"
    print_ok(f"first standalone type is .{extension}")

    # Create through the dialog: select the textures folder (the dialog
    # seeds it), open the create dialog from the toolbar, name the file
    # and submit; the queued command is recorded and walks back through
    # undo.
    begin = session.length()
    session.send("select asset textures")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_SELECTED_TOKEN), r'\s+path="(textures)"'),
        30,
        "a folder row selected",
        begin,
    )
    begin = session.length()
    session.send("toolbar create")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_DIALOG_OPENED_TOKEN), r'\s+dialog="create"'),
        30,
        "create dialog opened",
        begin,
    )
    session.send("dialog set name s16_probe")
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("dialog submit")
    session.wait(re.escape(ASSET_CREATE_QUEUED_TOKEN), 30, "create queued", begin)
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="create asset s16_probe"'),
        30,
        "create recorded",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_DIALOG_CLOSED_TOKEN), r'\s+dialog="create"'),
        30,
        "create dialog closed",
        begin,
    )
    wait_applied(session, applied_before, "create applied")
    wait_for_path(session, created_path, True, 30, f"created {created}")
    before = listed
    session.wait_until(
        lambda _text: True if listing_count() >= before + 1 else None,
        30,
        "the listing did not pick the created file up\n",
        # The periodic walk writes the listing line; poll for it.
        poll=0.2,
    )

    # The creation undoes (the file goes) and redoes (it comes back).
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="create asset s16_probe"'),
        30,
        "create undone",
        begin,
    )
    wait_applied(session, applied_before, "undo applied")
    wait_for_path(session, created_path, False, 30, "the creation's undo removed the file")
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.redo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_REDO_TOKEN), r'\s+label="create asset s16_probe"'),
        30,
        "create redone",
        begin,
    )
    wait_applied(session, applied_before, "redo applied")
    wait_for_path(session, created_path, True, 30, "the redo restored the file")

    # Rename through the dialog: the field arrives with the stem in it,
    # the typed name keeps the extension, and the selection follows the
    # move.
    renamed = f"textures/s16_renamed.{extension}"
    renamed_path = state.asset_root / "textures" / f"s16_renamed.{extension}"
    begin = session.length()
    session.send(f"select asset {created}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_SELECTED_TOKEN), r'\s+path="')
        + re.escape(created)
        + r'"',
        30,
        "the created row selected",
        begin,
    )
    begin = session.length()
    session.send("toolbar rename")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_DIALOG_OPENED_TOKEN), r'\s+dialog="rename"'),
        30,
        "rename dialog opened",
        begin,
    )
    session.send("dialog set name s16_renamed")
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("dialog submit")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_RENAME_QUEUED_TOKEN), r'\s+from="')
        + re.escape(created)
        + r'"\s+to="'
        + re.escape(renamed)
        + r'"',
        30,
        "rename queued",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="rename asset s16_renamed\.')
        + re.escape(extension)
        + r'"',
        30,
        "rename recorded",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_FOLLOWED_TOKEN), r'\s+path="') + re.escape(renamed) + r'"',
        30,
        "the selection followed the rename",
        begin,
    )
    wait_applied(session, applied_before, "rename applied")
    wait_for_path(session, renamed_path, True, 30, f"renamed to {renamed}")
    wait_for_path(session, created_path, False, 30, "the old path is gone")

    # A cross-folder move (the scripted direct edit; no drag gesture yet)
    # carries the selection the same way.
    moved = f"models/s16_moved.{extension}"
    moved_path = state.asset_root / "models" / f"s16_moved.{extension}"
    begin = session.length()
    session.send(f"edit asset move {renamed} {moved}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_FOLLOWED_TOKEN), r'\s+path="') + re.escape(moved) + r'"',
        60,
        "the selection followed the move",
        begin,
    )
    wait_for_path(session, moved_path, True, 30, f"moved to {moved}")

    # A refused move: the destination is an existing folder, so nothing
    # moves and the selection stays where the file is. The follow reads
    # what applied rather than what was asked for, or the Inspector would
    # be pointed at a path the move never made.
    begin = session.length()
    session.send(f"edit asset move {moved} models")
    session.wait(
        re.escape(COMMAND_FAILED_TOKEN) + r'(?=[^\n]*already exists)',
        30,
        "the move onto an existing path was refused",
        begin,
    )
    refused_window = session.read_since(begin)
    assert SELECTION_FOLLOWED_TOKEN not in refused_window, (
        "the selection followed a move that never happened: "
        + refused_window[-800:]
    )
    wait_for_path(session, moved_path, True, 30, "the refused move left the file in place")
    print_ok("a refused move left the selection and the file alone")

    # The delete clears the selection and undoes back into the file.
    begin = session.length()
    session.send("toolbar delete")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_DELETE_QUEUED_TOKEN), r'\s+path="') + re.escape(moved) + r'"',
        30,
        "delete queued",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="delete asset ')
        + re.escape(moved)
        + r'"',
        30,
        "delete recorded",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(SELECTION_CLEARED_TOKEN), r'\s+path="') + re.escape(moved) + r'"',
        30,
        "the selection cleared with the delete",
        begin,
    )
    wait_for_path(session, moved_path, False, 30, "deleted from disk")
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="delete asset ') + re.escape(moved) + r'"',
        30,
        "delete undone",
        begin,
    )
    wait_applied(session, applied_before, "delete undo applied")
    wait_for_path(session, moved_path, True, 30, "the delete's undo restored the file")

    # The pipeline type is one the create dialog offers, and creating one
    # writes a valid `.pipeline`: the standard header, a guid and the
    # type's default document (an empty passes list, which is valid - a
    # chain names its passes by guid). The committed chain in
    # `devops/tests/projects/native` is what exercises a populated file.
    begin = session.length()
    session.send("dump standalone-types")
    session.wait(
        re.escape(STANDALONE_TYPE_TOKEN),
        30,
        "standalone types dumped for the pipeline",
        begin,
    )
    assert re.search(
        r'type_name="pill_rendering::RenderingPipeline"\s+'
        r'display="RenderingPipeline"\s+extension="pipeline"',
        session.read_since(begin),
    ), "the create dialog does not offer .pipeline"
    print_ok("the create dialog offers .pipeline")

    pipeline_path = state.asset_root / "textures" / "s16_pipeline.pipeline"
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send(
        "edit asset create pill_rendering::RenderingPipeline textures s16_pipeline"
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="create asset s16_pipeline"'),
        30,
        "pipeline create recorded",
        begin,
    )
    wait_applied(session, applied_before, "pipeline create applied")
    wait_for_path(session, pipeline_path, True, 30, "the pipeline was created")
    document = yaml.safe_load(pipeline_path.read_text(encoding="utf-8"))
    assert (
        document["asset_type"]
        == "pill_rendering::RenderingPipeline"
    ), document
    assert document["format_version"] == 1, document
    assert len(document["guid"]) == 32, document
    assert document["asset"]["passes"] == [], document
    print_ok("the Assets panel created a valid .pipeline")

    # The create's undo removes the file again: the tree is left as the
    # suite found it.
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    wait_applied(session, applied_before, "pipeline create undo applied")
    wait_for_path(session, pipeline_path, False, 30, "the pipeline's undo removed it")
    print_ok("the created pipeline's undo removed it")

    # The write-meta action on the staged sourced file: the sidecar is
    # written action by action and the undo removes it again.
    stage_meta = state.asset_root / "s16_stage.png.meta"
    begin = session.length()
    session.send("select asset s16_stage.png")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_SELECTED_TOKEN), r'\s+path="(s16_stage\.png)"'),
        30,
        "the staged row selected",
        begin,
    )
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("toolbar meta")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_META_QUEUED_TOKEN), r'\s+path="(s16_stage\.png)"'),
        30,
        "meta queued",
        begin,
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="write meta s16_stage\.png"'),
        30,
        "meta recorded",
        begin,
    )
    wait_applied(session, applied_before, "meta applied")
    wait_for_path(session, stage_meta, True, 30, "the sidecar was written")
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_UNDO_TOKEN), r'\s+label="write meta s16_stage\.png"'),
        30,
        "meta write undone",
        begin,
    )
    wait_applied(session, applied_before, "meta undo applied")
    wait_for_path(session, stage_meta, False, 30, "the sidecar's undo removed it")

    # The Inspector's asset rows are editable too: the
    # selected asset's import settings round-trip through the same
    # queue - a scripted `edit asset-settings` mirrors a widget edit,
    # the `.meta` sidecar on disk is the proof, and the undo restores
    # the original value. The project's cube has a boolean row
    # (`flip_v`) and its color texture has an enum one
    # (`texture_type`), so both widget shapes are covered.
    cube_asset = "models/cube.obj"
    cube_meta = state.asset_root / "models" / "cube.obj.meta"
    begin = session.length()
    session.send(f"select asset {cube_asset}")
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(ASSET_SELECTED_TOKEN), r'\s+path="') + re.escape(cube_asset) + r'"',
        30,
        "the cube row selected",
        begin,
    )
    flip_before = yaml.safe_load(cube_meta.read_text(encoding="utf-8"))["settings"]["flip_v"]
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send(
        f"edit asset-settings {cube_asset} flip_v {str(not flip_before).lower()}"
    )
    session.wait(
        log_layout.message_then_fields_pattern(re.escape(HISTORY_PUSH_TOKEN), r'\s+label="set asset flip_v"'),
        30,
        "the boolean row's edit recorded",
        begin,
    )
    wait_applied(session, applied_before, "settings edit applied")
    wait_meta_value(session, cube_meta, "flip_v", not flip_before, 30, "the sidecar carries the edit")
    begin = session.length()
    applied_before = occurrence_count(session, re.escape(ENGINE_COMMANDS_APPLIED_TOKEN))
    session.send("command editor.undo")

    state.applied_before = applied_before
    state.begin = begin
    state.cube_meta = cube_meta
    state.flip_before = flip_before


def _area_entry(context: "StepContext") -> None:
    """Runs this area against the run's shared state."""
    run_assets_browser_and_pipeline_steps(context.state)


AREA = StepArea(
    name="assets_browser_and_pipeline",
    order=6,
    run=_area_entry,
    needs=("before_plane", "gizmo_entity",),
    provides=("cube_meta", "flip_before",),
)
