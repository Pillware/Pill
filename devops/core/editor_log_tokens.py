"""The editor suite's log vocabulary: every token and pattern a wait reads.

One owner for the names `test_editor_egui.py`, `editor_steps_lifecycle.py` and
`editor_session.py` share. `test_log_contract.py` scans this module with the
rest of `devops/` and pins each `*_TOKEN` literal to the Rust line that prints
it, so a token cannot drift from the editor without the contract failing.

The tokens are the stable halves of the lines the suite asserts on; the
runtime halves (entity handles, counts, rectangles) stay in the suites, which
compose them onto these patterns at the wait.
"""

import re

from core import log_layout
from core.paths import executable_name

# The suite tokens: the stable halves of the lines this suite asserts on, so
# `test_log_contract.py` checks each against the Rust sources. Patterns that
# embed runtime values (frame counts, dock surfaces) compose them with their
# regex suffix at the wait.

# The lines every editor suite reads first: the window came up, the keymap was
# applied, a shortcut ran (or had no action yet), and a scripted line the editor
# refused.
EDITOR_WINDOW_CREATED_TOKEN = "[editor] window created"
# The command registry's report after a keybindings file lands: the preset it
# activated, how many overrides it resolved, and one line per chord two
# commands answer to.
EDITOR_KEYBINDINGS_APPLIED_TOKEN = "[editor] keybindings applied"
EDITOR_KEYBINDING_CONFLICT_TOKEN = "[editor] keybinding conflict"
# A chord the registry resolved, and the same chord when the command it named
# has no action in the executable yet.
EDITOR_SHORTCUT_RAN_TOKEN = "[editor] shortcut ran"
# The egui editor's one refusal line for a scripted line it cannot run: an
# unknown verb, an unknown target, or a missing or malformed argument. A suite
# typo raises `ScriptedCommandRejected` from the next wait instead of running
# out its timeout, and `test_log_contract.py` pins the literal against Rust.
EDITOR_SCRIPTED_REJECTED_TOKEN = "[editor] scripted command rejected"
REJECTION_PATTERN = re.compile(
    log_layout.message_then_fields_pattern(re.escape(EDITOR_SCRIPTED_REJECTED_TOKEN), r'\s+command="((?:[^"\\]|\\.)*)"\s+reason="((?:[^"\\]|\\.)*)"')
)

EGUI_EDITOR_BINARY = executable_name("pill_editor")
EGUI_SHELL_LOADED_TOKEN = "[editor] pill_editor_shell_egui_editor loaded"
# Every shell generation clears the shared egui context's temporary data
# before its first draw, so nothing a previous generation created is read
# through an unloaded image.
SHELL_CONTEXT_CLAIMED_TOKEN = "[editor] shell context claimed"
EGUI_ROWS_TOKEN = "[editor] egui shell rows"
EGUI_DOCK_DUMP_TOKEN = "[editor] egui dock dump"
EGUI_TAB_UNDOCKED_TOKEN = "[editor] tab undocked"
SCENE_SURFACE_TOKEN = "[editor] scene surface attached"
SCENE_VIEW_ADDED_TOKEN = "[editor] scene view added"
SCENE_VIEW_OPENED_TOKEN = "[editor] scene view opened"
SCENE_SURFACE_RESIZED_TOKEN = "[editor] scene surface resized"
FRAME_HEARTBEAT_TOKEN = "[editor] frame heartbeat"
FRAME_IDLE_TOKEN = "[editor] frame idle"
CURSOR_ICON_TOKEN = "[editor] cursor icon"
SCENE_BADGE_RECT_TOKEN = "[editor] scene badge rect"
SCENE_BADGE_TOKEN = "[editor] scene badge"
PREVIEW_REQUESTED_TOKEN = "[editor] preview requested"
PREVIEW_READY_TOKEN = "[editor] preview ready"
PROJECT_RELOAD_TOKEN = "hot reload complete"
EXTENSION_RELOAD_TOKEN = "extension reload processed"
SCENE_VIEW_RECT_TOKEN = "[editor] scene view rect"
SCENE_PICK_ENTITY_TOKEN = "[editor] scene pick entity"
SCENE_PICK_NOTHING_TOKEN = "[editor] scene pick nothing"
SELECTION_CHANGED_TOKEN = "[editor] selection changed"
OUTLINE_SET_TOKEN = "[editor] outline set"
OUTLINE_CLEARED_TOKEN = "[editor] outline cleared"
SCENE_PICK_HANDLE_TOKEN = "[editor] scene pick handle"
GIZMO_ANCHOR_TOKEN = "[editor] gizmo anchor"
GIZMO_TOOL_TOKEN = "[editor] gizmo tool"
GIZMO_DRAG_PRESS_TOKEN = "[editor] gizmo drag press"
GIZMO_DRAG_STARTED_TOKEN = "[editor] gizmo drag started"
GIZMO_DRAG_ENDED_TOKEN = "[editor] gizmo drag ended"
GIZMO_DRAG_RELEASED_TOKEN = "[editor] gizmo drag ended without a move"
GIZMO_HOVER_TOKEN = "[editor] gizmo hover handle"
GIZMO_HOVER_NONE_TOKEN = "[editor] gizmo hover none"
GIZMO_GUIDES_TOKEN = "[editor] gizmo guides"
VIEWPORT_DUMP_TOKEN = "[test] viewport"
# The play session: the engine's two announcements, the editor's mode report,
# the toolbar's placement and the tab focus the shell reports.
PLAY_ENTERED_TOKEN = "[engine] play entered"
PLAY_EXITED_TOKEN = "[engine] play exited"
PLAY_REFUSED_TOKEN = "[engine] play refused"
PLAY_TOGGLED_TOKEN = "[editor] play toggled"
PLAY_TOOLBAR_RECT_TOKEN = "[editor] play toolbar rect"
PLAY_CONTROL_TOKEN = "[editor] play control"
PLAY_FOCUSED_GAME_TAB_TOKEN = "[editor] play focused the game tab"
PLAY_RESTORED_TAB_TOKEN = "[editor] play restored the tab"
PLAY_RESTORED_CAMERAS_TOKEN = "[editor] play restored the cameras"
PLAY_DUMP_TOKEN = "[test] play"

# The lifecycle probe the scene project seeds: one line per script hook, each
# carrying the cause it was called with and the entity it ran on.
PROBE_CREATE_TOKEN = "[probe] create"
PROBE_START_TOKEN = "[probe] start"
PROBE_UPDATE_TOKEN = "[probe] update"
PROBE_STOP_TOKEN = "[probe] stop"
PROBE_DESTROY_TOKEN = "[probe] destroy"
VIEWPORT_CAMERA_TOKEN = "[test] viewport camera"
MENU_BAR_RECT_TOKEN = "[editor] menu bar rect"
MENU_TITLE_RECT_TOKEN = "[editor] menu item rect"
MENU_ENTRY_RECT_TOKEN = "[editor] menu entry rect"
MENU_OPENED_TOKEN = "[editor] menu opened"
MENU_ACTION_TOKEN = "[editor] menu action"
PALETTE_QUERY_TOKEN = "[editor] palette query"
PALETTE_CLOSED_TOKEN = "[editor] palette closed"
COMMAND_NO_ACTION_TOKEN = "[editor] command has no action"
PALETTE_DISMISSED_TOKEN = "[editor] palette dismissed"
CONSOLE_DUMP_TOKEN = "[test] console"
PROBLEMS_DUMP_TOKEN = "[test] problems"
PROBLEM_ROW_TOKEN = "[test] problem row"
PROBLEM_OPENED_TOKEN = "[editor] problem opened"
STATS_DUMP_TOKEN = "[test] stats"
STATS_TAB_TOKEN = "[test] stats tab"
CADENCE_DUMP_TOKEN = "[test] cadence"
REDRAW_ON_DEMAND_TOKEN = "[editor] redraw on demand"
NO_OVERLAY_TOKEN = "[editor] no overlay posture"
OVERLAY_POSTURE_TOKEN = "[editor] overlay posture"
PANELS_DUMP_TOKEN = "[test] panels"
PANEL_ROW_TOKEN = "[test] panel"
PANEL_REGISTERED_TOKEN = "[editor] panel registered"
PANEL_OVER_BUDGET_TOKEN = "[editor] module panel over budget"
PANEL_UNLOADED_TOKEN = "[editor] panel source unloaded"
PANEL_RESTORED_TOKEN = "[editor] panel source restored"
PANEL_WINDOW_OPENED_TOKEN = "[editor] panel window opened"
PANEL_WINDOW_REQUESTED_TOKEN = "[editor] panel window requested"
PANEL_MODAL_ANSWERED_TOKEN = "[editor] panel modal answered"
PANEL_LOG_TOKEN = "[editor] panel log"
PANEL_FIELD_QUEUED_TOKEN = "[editor] panel field queued"
SELECTION_DUMP_TOKEN = "[test] selection"
SELECTED_ENTITY_TOKEN = "[test] selected entity"
SELECTED_ELEMENT_TOKEN = "[test] selected element"
ELEMENT_SELECTION_TOKEN = "[editor] elements selected"
ELEMENT_SELECTION_QUEUED_TOKEN = "[editor] element selection queued"
TOOL_ACTIVATED_TOKEN = "[editor] tool activated"
TOOL_DUMP_TOKEN = "[test] tools"
TOOL_ROW_TOKEN = "[test] tool"
MIXED_ROW_TOKEN = "[editor] inspector row kind=mixed"
SELECTION_EDIT_TOKEN = "[editor] selection field edit queued"
CLIPBOARD_WRITTEN_TOKEN = "[editor] clipboard written"
CLIPBOARD_PASTED_TOKEN = "[editor] clipboard pasted"
DROP_PREVIEW_TOKEN = "[editor] drop preview"
FILES_DROPPED_TOKEN = "[editor] files dropped"
DROP_REFUSED_FOLDER_TOKEN = "[editor] drop refused: folder"
DROP_REFUSED_PATH_TOKEN = "[editor] drop refused: "
DRAG_OUT_PREPARED_TOKEN = "[editor] drag out prepared"
# No pointer button is held during an automated run, so the drag has to end
# before OLE's loop: that refusal is the deterministic half of drag-out.
DRAG_OUT_NO_BUTTON_TOKEN = "[editor] drag out skipped: no button held"
RECOVERED_STATE_TOKEN = "[editor] recovered editor state after an unclean exit"
# The registered name the showcase component's payload carries: the
# paste's registry check compares full type names.
SHOWCASE_TYPE_NAME = "scene::ShowcaseComponent"
WINDOWS_DUMP_TOKEN = "[test] windows"
WINDOW_ROW_TOKEN = "[test] window"
SAFE_MODE_TOKEN = "[editor] safe mode"
SAFE_MODE_RETRY_TOKEN = "[editor] safe mode retry requested"
CONSOLE_FILTER_TOKEN = "[editor] console filter"
CONSOLE_SEARCH_TOKEN = "[editor] console search"

# One console dump: the filters in force, the counts and the first row the
# filters left.
CONSOLE_DUMP_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(CONSOLE_DUMP_TOKEN), r'\s+level="(?P<level>\w+)"\s+channel="(?P<channel>\w+)"')
    + r'\s+search="(?P<search>(?:[^"\\]|\\.)*)"'
    + r"\s+total=(?P<total>\d+)\s+visible=(?P<visible>\d+)"
    + r"\s+engine=(?P<engine>\d+)\s+game=(?P<game>\d+)"
)
QUIT_REQUESTED_TOKEN = "[editor] quit requested"
SCENE_NEW_TOKEN = "[editor] scene new"
SCENE_OPENED_TOKEN = "[editor] scene opened"
SCENE_SAVED_TOKEN = "[editor] scene saved"
SCENE_TITLE_TOKEN = "[editor] scene title"
LAYOUT_SAVED_TOKEN = "[editor] layout saved"
LAYOUT_LEAF_TOKEN = "[editor] egui dock leaf"
INSPECTOR_SECTION_TOKEN = "[editor] inspector section"
INSPECTOR_SECTION_RECT_TOKEN = "[editor] inspector section rect"
INSPECTOR_ADD_RECT_TOKEN = "[editor] inspector add rect"
INSPECTOR_ADD_OPENED_TOKEN = "[editor] inspector add opened"
INSPECTOR_ADD_ENTRY_TOKEN = "[editor] inspector add entry"
INSPECTOR_ROW_TOKEN = "[editor] inspector row"
ACTIVE_VIEWPORT_TOKEN = "[editor] active viewport"
CAMERA_GESTURE_TOKEN = "[editor] camera gesture applied"
CAMERA_ZOOM_TOKEN = "[editor] camera zoom"
FIELD_TOKEN = "[test] field"
ASSET_LISTING_TOKEN = "[editor] asset listing refreshed"
ASSET_SELECTED_TOKEN = "[editor] asset selected"
ASSET_DIALOG_OPENED_TOKEN = "[editor] asset dialog opened"
ASSET_DIALOG_CLOSED_TOKEN = "[editor] asset dialog closed"
ASSET_CREATE_QUEUED_TOKEN = "[editor] asset create queued"
ASSET_RENAME_QUEUED_TOKEN = "[editor] asset rename queued"
ASSET_DELETE_QUEUED_TOKEN = "[editor] asset delete queued"
ASSET_META_QUEUED_TOKEN = "[editor] asset meta queued"
ENGINE_COMMANDS_APPLIED_TOKEN = "[editor] engine commands applied"
HISTORY_PUSH_TOKEN = "[editor] history push"
HISTORY_UNDO_TOKEN = "[editor] history undo"
# An asset reference row and what it names: the Inspector logs each
# distinct row once, the applied pick or drop once, and the drop's two ends'
# rectangles once per placement.
REFERENCE_ROW_TOKEN = "[editor] inspector row kind=asset"
REFERENCE_SET_TOKEN = "[editor] asset reference set"
REFERENCE_TILE_RECT_TOKEN = "[editor] assets tile rect"
REFERENCE_ROW_RECT_TOKEN = "[editor] inspector asset row rect"
# An entity reference row and what it names: the row logs each
# distinct reference once, the applied edit once, and the scene-pick arming
# once per request.
ENTITY_REFERENCE_ROW_TOKEN = "[editor] inspector row kind=entity"
ENTITY_REFERENCE_SET_TOKEN = "[editor] entity reference set"
ENTITY_REFERENCE_ROW_RECT_TOKEN = "[editor] inspector entity row rect"
HIERARCHY_ROW_RECT_TOKEN = "[editor] hierarchy row rect"
RESOURCES_DUMP_TOKEN = "[test] resources"
RESOURCE_FIELD_TOKEN = "[test] resource field"
RESOURCE_SELECTED_TOKEN = "[editor] resource selected"
SELECTION_RESTORED_TOKEN = "[editor] selection restored"
ENTITIES_DUMP_TOKEN = "[test] entities"
ENTITY_ROW_TOKEN = "[test] entity"
SHOWCASE_COMPONENT = "ShowcaseComponent"
RESOURCE_EDIT_TOKEN = "[editor] resource edit queued"
GIZMO_RESOURCE_NAME = "pill_editor_core::gizmo::GizmoState"
HISTORY_REDO_TOKEN = "[editor] history redo"
ASSETS_DUMP_TOKEN = "[test] assets"
BROWSER_FOLDER_TOKEN = "[editor] browser folder"
BROWSER_DUMP_TOKEN = "[test] browser"
BROWSER_ITEM_TOKEN = "[test] browser item"
# A module's resources are listed and inspected, never written: the section
# says so once per selected asset, and a delete is refused instead of queued.
ASSET_READ_ONLY_ROW_TOKEN = "[editor] inspector row kind=asset-read-only"
ASSET_DELETE_REFUSED_TOKEN = "[editor] delete refused: a module's resources are read-only"
STANDALONE_TYPE_TOKEN = "[test] standalone type"
SYSTEMS_DUMP_TOKEN = "[test] systems"
SYSTEM_ROW_TOKEN = "[test] system"
PAUSE_TOGGLED_TOKEN = "[editor] pause toggled"
STEP_QUEUED_TOKEN = "[editor] step queued"
SYSTEM_EDIT_QUEUED_TOKEN = "[editor] system edit queued"
CONTEXT_MENU_TOKEN = "[editor] context menu"
NEW_ENTITY_TOKEN = "[editor] new entity queued"
DELETE_ENTITY_TOKEN = "[editor] delete entity queued"
SELECTION_FOLLOWED_TOKEN = "[editor] selection followed the move"
SELECTION_CLEARED_TOKEN = "[editor] selection cleared with the delete"
COMMAND_FAILED_TOKEN = "[editor] command failed"
PREFERENCES_DUMP_TOKEN = "[test] preferences"
PREFERENCES_APPLIED_TOKEN = "[editor] preferences applied"
PREFERENCES_SAVED_TOKEN = "[editor] preferences saved"
PREFERENCE_EDIT_QUEUED_TOKEN = "[editor] preference edit queued"
# The editor's theme layers: the executable logs each
# resolution, the shell logs each install, the dump reports what the shell
# would install, and a refused or legacy layer says so. The refusal token
# stops where a `layer=` value starts inside the message, so the patterns
# that use it append that value directly; it is message text, not a formatter
# field, and is not built through `log_layout`.
EDITOR_THEME_RESOLVED_TOKEN = "[editor] theme resolved"
EDITOR_THEME_APPLIED_TOKEN = "[editor] theme applied"
EDITOR_THEME_DUMP_TOKEN = "[test] theme"
EDITOR_THEME_REFUSED_TOKEN = "[editor] theme refused layer="
EDITOR_THEME_LEGACY_TOKEN = "[editor] theme name"
# The fonts a theme layer names: an install, a refusal.
EDITOR_FONTS_INSTALLED_TOKEN = "[editor] fonts installed"
EDITOR_FONT_REFUSED_TOKEN = "[editor] font refused"
GIZMO_PREFERENCES_WRITTEN_TOKEN = "[editor] gizmo preferences written"
HISTORY_TRIMMED_TOKEN = "[editor] history trimmed"
GAME_VIEW_RECT_TOKEN = "[editor] game view rect"
TAB_ACTIVATED_TOKEN = "[editor] tab activated"
POPOUT_MOVED_TOKEN = "[editor] popout moved"
POPOUT_OPENED_TOKEN = "[editor] popout opened"
POPOUT_CLOSED_TOKEN = "[editor] popout closed"
POPOUT_PRESENTED_TOKEN = "[editor] popout presented"
DOCK_POPOUT_TOKEN = "[editor] egui dock popout"
SCENE_VIEW_REBOUND_TOKEN = "[editor] scene view rebound"
TOOLBOX_RECT_TOKEN = "[editor] toolbox rect"
TOOLBOX_ITEM_TOKEN = "[editor] toolbox item"
TOOLBOX_CLICK_TOKEN = "[editor] toolbox click"
TOOLBOX_MOVED_TOKEN = "[editor] toolbox moved"
POINTER_DRAG_TOKEN = "[editor] pointer drag injected"
SCENE_GRID_TOKEN = "[test] scene grid"
SCENE_GRID_STATE_TOKEN = "[editor] scene grid"
HOT_PATCH_TOKEN = "[hot]"
KEY_INJECTED_TOKEN = "[editor] key injected"
TOAST_TOKEN = "[editor] toast"

# One toast line as the shell logs it, cut after the opening quote of the
# text: the level (a quoted string field), the notice id and the text the
# executable produced, whose own pattern each check appends inside the
# quotes.
TOAST_PREAMBLE = log_layout.message_then_fields_pattern(re.escape(TOAST_TOKEN), r'\s+level="{}"\s+seq=\d+\s+text="')

# A whole toast line, rectangle included: the level, the id, the text and
# where the row landed, so a check can click the toast itself.
TOAST_LINE_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(TOAST_TOKEN), r'\s+level="(\w+)"\s+seq=\d+\s+text="((?:[^"\\]|\\.)*)"')
    + r"\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)\s+width=([0-9.]+)\s+height=([0-9.]+)"
)

# The transform the gizmo drags edit; the field dump resolves the short
# name, like the native suite's dumps do.
TRANSFORM_COMPONENT = "TransformComponent"

# One live patch of a named shell function: the host's line names the
# function it replaced, the milliseconds it took and the route it took. The
# names are package-qualified, so the pattern only pins the module and the
# function and leaves the crate prefix free. This matches a block body line,
# not a prefix line, so its spaces belong to the message and it is not built
# through `log_layout`.
HOT_PATCH_LIVE_PATTERN = (
    # The shell is split file-per-tab, so a body's path
    # carries its module: `...::shell::tab_entities::entities_tab`.
    re.escape(HOT_PATCH_TOKEN) + r" \S*::shell::(?:\w+::)?{} LIVE \d+ ms"
)

# The rows heartbeat's runtime values, in the field order the shell logs them.
ROWS_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(EGUI_ROWS_TOKEN), r"\s+frames=(\d+)\s+entities=(\d+)\s+systems=(\d+)")
)
DOCK_DUMP_PATTERN = log_layout.message_then_fields_pattern(re.escape(EGUI_DOCK_DUMP_TOKEN), r"\s+surfaces=(\d+)")
# One leaf of the dock dump: the surface and node that hold it, the tab in
# front and every tab it holds, in draw order.
DOCK_LEAF_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(LAYOUT_LEAF_TOKEN), r"\s+surface=(\d+)\s+node=(\d+)\s+active=([a-z0-9]*)\s+tabs=([a-z0-9,]*)")
)
# The scene view rect's runtime values, in the field order the frontend logs
# them: view, x, y, width, height, visible.
SCENE_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(SCENE_VIEW_RECT_TOKEN), r"\s+view=(\d+)\s+x=(-?\d+)\s+y=(-?\d+)\s+width=(\d+)\s+height=(\d+)")
    + r"\s+visible=(true|false)"
)
# The game view rect's runtime values, in the field order the frontend logs
# them: x, y, width, height, visible. The game window has no view index - one
# primary surface, one window.
GAME_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(GAME_VIEW_RECT_TOKEN), r"\s+x=(-?\d+)\s+y=(-?\d+)\s+width=(\d+)\s+height=(\d+)")
    + r"\s+visible=(true|false)"
)
# The floating toolbox's runtime values, in the field order the shell logs
# them: the panel's rectangle, and one row's - both in logical points, which
# are the physical pixels of the click injections while the suite runs at a
# scale factor of 1.
TOOLBOX_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(TOOLBOX_RECT_TOKEN), r"\s+view=(\d+)\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)")
    + r"\s+width=([0-9.]+)\s+height=([0-9.]+)"
)
TOOLBOX_ITEM_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(TOOLBOX_ITEM_TOKEN), r'\s+view=(\d+)\s+item="([a-z]+)"\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)')
    + r"\s+width=([0-9.]+)\s+height=([0-9.]+)"
)
# The menu bar's runtime values, in the field order the shell logs them: the
# strip's rectangle, each title's, and each open menu entry's - all in logical
# points, the physical pixels of the click injections while the suite runs at a
# scale factor of 1.
MENU_BAR_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(MENU_BAR_RECT_TOKEN), r"\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)\s+width=([0-9.]+)\s+height=([0-9.]+)")
)
MENU_TITLE_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(MENU_TITLE_RECT_TOKEN), r'\s+name="([a-z]+)"\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)')
    + r"\s+width=([0-9.]+)\s+height=([0-9.]+)"
)
MENU_ENTRY_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(MENU_ENTRY_RECT_TOKEN), r'\s+menu="([a-z]+)"\s+label="([^"]+)"\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)')
    + r"\s+width=([0-9.]+)\s+height=([0-9.]+)"
)
# The Inspector's runtime values: a section header's rectangle (keyed by the
# registered component type), the Add Component button's, and an addable
# entry's - all in logical points like the rest of the shell's geometry.
INSPECTOR_SECTION_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(INSPECTOR_SECTION_RECT_TOKEN), r'\s+component="([^"]+)"\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)')
    + r"\s+width=([0-9.]+)\s+height=([0-9.]+)"
)
INSPECTOR_ADD_RECT_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(INSPECTOR_ADD_RECT_TOKEN), r"\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)\s+width=([0-9.]+)\s+height=([0-9.]+)")
)
INSPECTOR_ADD_ENTRY_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(INSPECTOR_ADD_ENTRY_TOKEN), r'\s+component="([^"]+)"\s+x=(-?[0-9.]+)\s+y=(-?[0-9.]+)')
    + r"\s+width=([0-9.]+)\s+height=([0-9.]+)"
)
# One drawn Inspector row, in the field order the shell logs it: the row's
# kind, the component it belongs to, then whatever that kind carries (a
# field's name, widget and tooltip, a header's text, a gap's height). The
# kind logs unquoted because the shell passes it as a literal, and it sits
# inside the message before the separator; the component is the first field.
INSPECTOR_ROW_PATTERN = log_layout.message_then_fields_pattern(
    re.escape(INSPECTOR_ROW_TOKEN) + r' kind="?([a-z-]+)"?',
    r'\s+component="([^"]+)"([^\n]*)',
)
# The scene grid's state, in the field order the frontend logs it: the
# switch, the cell size, and the surfaces the renderer draws it on (empty
# while it is off).
SCENE_GRID_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(SCENE_GRID_TOKEN), r"\s+enabled=(true|false)\s+cell=([0-9.]+)\s+surfaces=(\S*)")
)
# The two answers a scripted scene click can produce, as full patterns.
PICK_ENTITY_PATTERN = log_layout.message_then_fields_pattern(re.escape(SCENE_PICK_ENTITY_TOKEN), r"\s+entity=(\S+)")
PICK_NOTHING_PATTERN = re.escape(SCENE_PICK_NOTHING_TOKEN)
# How long one probe click waits for either answer. An answer arrives within a
# frame or two, so the only way to wait this long is a click the shell took
# itself - a badge, a chip or the toolbox - which is a reason to try the next
# point, not to fail the step.
PICK_PROBE_TIMEOUT = 15
# One row of a `dump systems`: index, name, owner, state, then the markers.
SYSTEM_ROW_PATTERN = (
    log_layout.message_then_fields_pattern(re.escape(SYSTEM_ROW_TOKEN), r'\s+index=(\d+)\s+name="([^"]+)"\s+owner="([^"]+)"\s+enabled=(true|false)')
    + r"\s+hot_patchable=(?:true|false)\s+ambiguous=(?:true|false)"
)

