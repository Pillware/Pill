# REQUIREMENTS
#   Blender 4.x (this script is run BY Blender, not by Python). Uses only the
#   standard library plus bpy, which Blender ships.
#
# DESCRIPTION
#   Headless check of the Pill panels' layout: it imports the add-on from
#   `..\blender_addon\pill_blender_link.py` and draws each panel against a fake
#   layout that records every row, so the structure can be asserted without a
#   screen. Covers the sidebar's Connection block (its status, the port and one
#   button), the Properties editor's Pill block (the two pickers while an
#   object is unlinked, the disconnect button once it is, and the whole block
#   disabled without a connection), and the mesh a connection pushes into the
#   editor. Prints one line per check and exits non-zero on the first failure.
#
# USAGE
#   "C:\Program Files\Blender Foundation\Blender 4.5\blender.exe" -b ^
#       --factory-startup --python-exit-code 1 ^
#       --python modules/extensions/pill_blender_link/tests/blender_link_panel_probe.py
#
# EXAMPLE USAGE
#   See the command above; every check prints "[probe] ok ..." on success.
#
# --- SCRIPT ---
import importlib.util
import os

import bpy

ADDON_PATH = os.path.abspath(
    os.path.join(
        os.path.dirname(__file__),
        "..",
        "blender_addon",
        "pill_blender_link.py",
    )
)

CHECKS = []


def check(condition, description):
    if not condition:
        raise AssertionError(description)
    CHECKS.append(description)
    print(f"[probe] ok {description}")


def load_addon():
    spec = importlib.util.spec_from_file_location("pill_blender_link_panel_probe", ADDON_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class FakeConnection:
    def close(self):
        pass


# Stands in for a `UILayout`: every row is recorded with the layout's own
# `enabled` flag, which is what greys out the Pill block while there is no
# connection.
class FakeLayout:
    def __init__(self, record):
        self.record = record
        self.enabled = True

    def label(self, text="", icon="NONE"):
        self.record.append(("label", text, icon, self.enabled))

    def prop(self, data, name, text=""):
        self.record.append(("prop", name, text, self.enabled))

    def operator(self, idname, text="", icon="NONE"):
        self.record.append(("operator", idname, text, self.enabled))

    def column(self):
        return FakeLayout(self.record)

    def row(self):
        return FakeLayout(self.record)


class FakeWindowManager:
    pill_link_port = 47390


class FakeContext:
    window_manager = FakeWindowManager()
    active_object = None


class PanelHolder:
    def __init__(self, layout):
        self.layout = layout


def draw_panel(addon, panel_class):
    """Draws one panel once and returns the recorded rows."""
    record = []
    panel_class.draw(PanelHolder(FakeLayout(record)), FakeContext())
    return record


def rows_of(record, kind):
    return [row for row in record if row[0] == kind]


def texts_of(record, kind):
    return [row[1] for row in rows_of(record, kind)]


# Reads one property default off a registered operator's RNA, which is what a
# probe in background mode can see of the file browser's own settings.
def operator_default(operator_idname, property_name):
    operator_type = bpy.ops
    for name_part in operator_idname.split("."):
        operator_type = getattr(operator_type, name_part)
    return operator_type.get_rna_type().properties[property_name].default


def main():
    addon = load_addon()
    addon.register()
    # The timer stays out of the way: these checks set the link state by hand.
    addon.link_state.enabled = False
    bpy.ops.mesh.primitive_cube_add()
    mesh_object = bpy.context.active_object
    FakeContext.active_object = mesh_object

    # Step 1: the sidebar is one block, Connection, in the Pill category.
    connection_panel = addon.PILL_PT_connection_panel
    check(
        connection_panel.bl_label == "Connection",
        "the sidebar block is titled Connection",
    )
    check(
        connection_panel.bl_category == "Pill",
        "the block lives in the Pill tab",
    )
    check(
        (connection_panel.bl_space_type, connection_panel.bl_region_type) == ("VIEW_3D", "UI"),
        "the block is drawn in the 3D viewport's sidebar",
    )
    check(
        not hasattr(addon, "PILL_PT_actions_panel"),
        "the sidebar carries no second block",
    )

    # Step 2: the Connection block with no link.
    connection_rows = draw_panel(addon, connection_panel)
    check(
        texts_of(connection_rows, "label")[0] == "Not connected",
        "a closed link reads as not connected",
    )
    check(
        texts_of(connection_rows, "prop") == ["pill_link_port"],
        "the Connection block holds the port field",
    )
    connection_operators = rows_of(connection_rows, "operator")
    check(
        [row[1] for row in connection_operators] == ["pill.link_connect"],
        "a closed link offers one Connect button",
    )
    check(
        connection_operators[0][2] == "Connect",
        "the button's label says Connect",
    )
    check(
        all(row[3] is True for row in connection_rows),
        "the Connection block is enabled while the link is closed",
    )

    # Step 3: the Object tab's Pill block is a PROPERTIES block of the Object
    # tab that only a mesh object shows, and with no connection every row of it
    # is disabled, because nothing in it can reach the editor.
    object_panel = addon.PILL_PT_mesh_link_panel
    check(
        (object_panel.bl_space_type, object_panel.bl_context, object_panel.bl_label)
        == ("PROPERTIES", "object", "Pill"),
        "the Object tab carries a Pill block",
    )
    check(object_panel.poll(FakeContext()), "the Pill block shows for a mesh object")
    empty = bpy.data.objects.new("ProbeEmpty", None)
    FakeContext.active_object = empty
    check(
        not object_panel.poll(FakeContext()),
        "the Pill block hides for an object that is not a mesh",
    )
    FakeContext.active_object = mesh_object
    addon.link_state.connection = None
    disconnected_rows = draw_panel(addon, object_panel)
    check(
        texts_of(disconnected_rows, "label")[0] == "Linked Pill mesh: None",
        "an unlinked object's block names no asset",
    )
    check(
        "Not linked to any Pill mesh" in texts_of(disconnected_rows, "label"),
        "an unlinked object's block says it is not linked",
    )
    check(
        [row[1] for row in rows_of(disconnected_rows, "operator")]
        == ["pill.link_pick", "pill.link_create"],
        "an unlinked object offers link and create",
    )
    check(
        not rows_of(disconnected_rows, "prop"),
        "an object without a Geometry Nodes modifier has no realize switch",
    )
    check(
        disconnected_rows and all(row[3] is False for row in disconnected_rows),
        "every row is disabled while the editor is not connected",
    )

    # Step 4: the Connection block with a connection.
    addon.link_state.connection = FakeConnection()
    addon.link_state.editor_title = "Pill"
    addon.link_state.session = "probe-session"
    addon.link_state.project_res_directory = "D:/project/res"
    connection_rows = draw_panel(addon, connection_panel)
    check(
        texts_of(connection_rows, "label")[0] == "Connected to: Pill",
        "a connected link names the editor it is connected to",
    )
    connection_operators = rows_of(connection_rows, "operator")
    check(
        [row[1] for row in connection_operators] == ["pill.link_disconnect"],
        "a connected link offers one Disconnect button",
    )
    check(
        connection_operators[0][2] == "Disconnect",
        "the button's label says Disconnect",
    )
    connected_rows = draw_panel(addon, object_panel)
    check(
        connected_rows and all(row[3] is True for row in connected_rows),
        "every row is enabled once the editor is connected",
    )
    check(
        (addon.PILL_OT_link_pick.bl_label, addon.PILL_OT_link_create.bl_label)
        == ("Link to existing Pill mesh", "Create new Pill mesh"),
        "the two buttons read link and create",
    )
    check(
        operator_default("pill.link_pick", "filter_glob") == "*.obj"
        and operator_default("pill.link_create", "filter_glob") == "*.obj",
        "both pickers offer .obj files only",
    )
    check(
        operator_default("pill.link_create", "check_existing") is True,
        "the create picker asks before a name is taken",
    )

    # Step 5: a pick sends the file it names, and a pick with no file does not.
    picked_path = os.path.join("D:\\", "project", "res", "models", "teapot.obj")
    sent_frames = []
    addon.send_frame = lambda header, body=b"": sent_frames.append((header, body))
    problem = addon.request_connect(mesh_object, picked_path)
    check(problem is None, "a link request on a connected link is accepted")
    check(len(sent_frames) == 1, "a link request sends exactly one frame")
    link_frame = sent_frames[-1][0]
    check(link_frame["type"] == "connect_asset", "the frame is a connect_asset request")
    check(link_frame["path"] == picked_path, "the frame carries the picked file's path")
    check(link_frame["object"] == mesh_object.name, "the frame names the object")
    sent_frames.clear()
    problem = addon.request_connect(mesh_object, "  ")
    check(
        problem == "pick the file to link to first",
        "a link request with no picked file is refused with the reason",
    )
    check(sent_frames == [], "a refused link request sends nothing")

    # Step 6: a closed link says why, under the status line.
    addon.disconnect("probe finished")
    connection_rows = draw_panel(addon, connection_panel)
    connection_labels = texts_of(connection_rows, "label")
    check(
        connection_labels[0] == "Not connected",
        "a closed link reads as not connected again",
    )
    check(
        connection_labels[1:] == ["Not connected (probe finished)"],
        "the Connection block says why the link is closed",
    )
    addon.link_state.connection = FakeConnection()
    addon.link_state.editor_title = "Pill"
    addon.link_state.session = "probe-session"

    # Step 7: the answer to a link request links the object, and the mesh it
    # already holds goes to the editor at once.
    addon.handle_connect_result(
        {
            "type": "connect_result",
            "session": "probe-session",
            "object": mesh_object.name,
            "asset_name": "models/teapot.obj",
            "ok": True,
            "link_id": 5,
        }
    )
    check(
        mesh_object.get(addon.LINK_ID_PROPERTY) == 5,
        "a connected object carries the link id",
    )
    check(
        mesh_object.get(addon.SESSION_PROPERTY) == "probe-session",
        "a connected object carries the session",
    )
    check(
        mesh_object.name in addon.link_state.dirty_object_names,
        "a connected object is queued for streaming",
    )
    # The connect answer only queues: the tick's own streaming step turns the
    # queued object into one full update in the same frame.
    sent_frames.clear()
    addon.stream_dirty_objects()
    updates = [frame for frame in sent_frames if frame[0].get("type") == "mesh_update"]
    check(len(updates) == 1, "connecting sends the object's mesh straight away")
    check(
        updates and updates[0][0].get("link_id") == 5,
        "the pushed update names the new link",
    )
    check(
        updates and updates[0][0].get("vertex_count") > 0,
        "the pushed update carries the object's vertices",
    )
    object_rows = draw_panel(addon, object_panel)
    check(
        texts_of(object_rows, "label")[0] == "Linked Pill mesh: models/teapot.obj",
        "a linked object's block names the asset it streams into",
    )
    check(
        [row[1] for row in rows_of(object_rows, "operator")] == ["pill.link_detach"],
        "a linked object offers the disconnect button instead",
    )
    check(
        "Not linked to any Pill mesh" not in texts_of(object_rows, "label"),
        "a linked object's block does not claim it is unlinked",
    )

    # Step 8: a refused connect leaves the object alone and shows the reason
    # under the label an unlinked object always carries.
    fresh = bpy.data.objects.new("ProbeUnlinked", bpy.data.meshes.new("ProbeUnlinkedMesh"))
    bpy.context.scene.collection.objects.link(fresh)
    addon.handle_connect_result(
        {
            "type": "connect_result",
            "session": "probe-session",
            "object": fresh.name,
            "asset_name": "",
            "ok": False,
            "detail": "models/nope.obj is outside the project's res",
        }
    )
    check(
        addon.LINK_ID_PROPERTY not in fresh,
        "a refused connect leaves the object without a link id",
    )
    FakeContext.active_object = fresh
    refused_rows = draw_panel(addon, object_panel)
    check(
        texts_of(refused_rows, "label")
        == [
            "Linked Pill mesh: None",
            "Not linked to any Pill mesh",
            "models/nope.obj is outside the project's res",
        ],
        "a refused connect keeps the labels and adds the reason",
    )
    check(
        [row[2] for row in rows_of(refused_rows, "label")] == ["INFO", "UNLINKED", "ERROR"],
        "the reason is drawn as an error",
    )

    # Step 9: only an object with a Geometry Nodes modifier carries the switch
    # that decides whether its instances are exported.
    node_group = bpy.data.node_groups.new("ProbeGeometryNodes", "GeometryNodeTree")
    fresh.modifiers.new(name="ProbeNodes", type="NODES").node_group = node_group
    FakeContext.active_object = fresh
    nodes_rows = draw_panel(addon, object_panel)
    check(
        texts_of(nodes_rows, "prop") == ["pill_realize_instances"],
        "a Geometry Nodes object offers the realize switch",
    )
    fresh.modifiers.remove(fresh.modifiers["ProbeNodes"])
    plain_rows = draw_panel(addon, object_panel)
    check(
        not rows_of(plain_rows, "prop"),
        "without the modifier the switch is not drawn",
    )

    print(f"[probe] {len(CHECKS)} checks passed")


main()
