# REQUIREMENTS
#   Blender 4.x (this script is run BY Blender, not by Python). Uses only the
#   standard library plus bpy, which Blender ships.
#
# DESCRIPTION
#   Headless check of the Pill live link's create direction: it imports the
#   add-on script from `..\blender_addon\pill_blender_link.py`, fakes the
#   socket, and asserts that "Create new Pill mesh"
#     - sends `create_mesh` with the file the person picked and the object's
#       evaluated geometry (a Subdivision modifier is applied, not the base
#       cage),
#     - refuses locally an object that is already linked, and a request with no
#       picked file,
#     - on an ok answer sets the object's link properties, remembers its
#       baseline as not sent and asks for the material,
#     - on a refusal leaves the object unlinked and records the reason.
#   Prints one line per check and exits non-zero on the first failure.
#
# USAGE
#   "C:\Program Files\Blender Foundation\Blender 4.5\blender.exe" -b ^
#       --factory-startup --python-exit-code 1 ^
#       --python modules/extensions/pill_blender_link/tests/blender_link_create_probe.py
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

# The geometry body: 3 positions, 2 texture coordinates and 3 normals of f32 per
# vertex, then one u32 per triangle corner.
VERTEX_BODY_BYTES = 32


def check(condition, description):
    if not condition:
        raise AssertionError(description)
    CHECKS.append(description)
    print(f"[probe] ok {description}")


def load_addon():
    spec = importlib.util.spec_from_file_location("pill_blender_link_create_probe", ADDON_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# Stands in for the socket: the probe never talks to an editor, it only needs
# the link to look connected.
class FakeConnection:
    def close(self):
        pass


# Makes a cube with a Subdivision modifier, which is the case the create has to
# get right: what the viewport shows is the subdivided mesh, not the cage.
def make_subdivided_cube():
    bpy.ops.mesh.primitive_cube_add()
    scene_object = bpy.context.active_object
    modifier = scene_object.modifiers.new(name="Subdivision", type="SUBSURF")
    modifier.levels = 2
    modifier.render_levels = 2
    bpy.context.view_layer.update()
    return scene_object


def main():
    addon = load_addon()
    sent_frames = []
    addon.send_frame = lambda header, body=b"": sent_frames.append((header, body))
    addon.link_state.session = "probe-session"
    addon.link_state.connection = FakeConnection()

    # Step 0: the sidebar's Connection section follows the socket and the
    # editor's own name from the greeting.
    check(
        addon.connection_status_text() == "Connected to: the Pill editor",
        "a link still learning the editor's name says so",
    )
    addon.link_state.editor_title = "Pill"
    check(
        addon.connection_status_text() == "Connected to: Pill",
        "a connected link names the editor it is connected to",
    )
    check(
        addon.connection_detail_text() == "",
        "a connected link shows no reason line",
    )

    # Step 1: the request carries the picked file and the evaluated geometry.
    scene_object = make_subdivided_cube()
    picked_path = os.path.join("D:", "project", "res", "models", "teapot.obj")
    problem = addon.request_create(scene_object, picked_path)
    check(problem is None, "a create request on a connected link is accepted")
    check(len(sent_frames) == 1, "a create request sends exactly one frame")
    header, body = sent_frames[-1]
    check(header["type"] == "create_mesh", "the frame is a create_mesh request")
    check(header["path"] == picked_path, "the frame carries the picked file's path")
    check(header["session"] == "probe-session", "the frame carries the session")
    check(
        addon.link_state.pending_create == {"object": scene_object.name, "path": picked_path},
        "the object is remembered until the answer arrives",
    )
    # A cube is 24 corners and the subdivided one is far more; a body of
    # 32 bytes per vertex plus 4 per index is the one the editor decodes.
    check(
        header["vertex_count"] > 24,
        f"the request sends the evaluated mesh ({header['vertex_count']} vertices, not 24)",
    )
    check(
        len(body) == header["vertex_count"] * VERTEX_BODY_BYTES + header["index_count"] * 4,
        "the body is one vertex record per vertex plus one index per triangle corner",
    )

    # Step 2: an ok answer links the object, remembers its baseline as unsent
    # and asks for the material.
    addon.handle_create_result(
        {
            "type": "create_result",
            "session": "probe-session",
            "asset_name": "models/teapot.obj",
            "ok": True,
            "link_id": 7,
        }
    )
    check(
        scene_object.get(addon.LINK_ID_PROPERTY) == 7,
        "the created object carries the link id the editor answered with",
    )
    check(
        scene_object.get(addon.SESSION_PROPERTY) == "probe-session",
        "the created object carries the session",
    )
    stream_cache = addon.link_state.stream_caches.get(scene_object.name)
    check(stream_cache is not None, "the created object's baseline is remembered")
    check(
        stream_cache is not None and stream_cache.sent is False,
        "the baseline is marked unsent, so the first edit goes as a full update",
    )
    check(
        scene_object.name not in addon.link_state.dirty_object_names,
        "nothing is queued right after the create",
    )
    check(addon.link_state.pending_create is None, "the pending create is cleared by the answer")
    material_request = sent_frames[-1][0]
    check(material_request["type"] == "material_request", "a material request follows the create")
    check(material_request["link_id"] == 7, "the material request names the new link")
    check(
        addon.link_state.connect_problem == "",
        "a created link has no refusal to show",
    )

    # Step 3: an object that is already a link is refused before anything is
    # sent, which is what keeps one object from becoming two entities. A
    # request with no picked file is refused the same way.
    sent_frames.clear()
    problem = addon.request_create(scene_object, picked_path)
    check(problem is not None and "already linked" in problem, "a second create from a linked object is refused")
    check(sent_frames == [], "a refused create sends nothing")
    unlinked_object = make_subdivided_cube()
    sent_frames.clear()
    problem = addon.request_create(unlinked_object, "  ")
    check(
        problem == "pick the file to create first",
        "a create with no picked file is refused with the reason",
    )
    check(sent_frames == [], "a create with no picked file sends nothing")

    # Step 4: a refusal from the editor leaves a fresh object unlinked.
    fresh_object = make_subdivided_cube()
    sent_frames.clear()
    check(
        addon.request_create(fresh_object, picked_path) is None,
        "a create the editor will refuse still goes out",
    )
    addon.handle_create_result(
        {
            "type": "create_result",
            "session": "probe-session",
            "asset_name": "models/duplicate.obj",
            "ok": False,
            "detail": "the asset already exists",
        }
    )
    check(
        addon.LINK_ID_PROPERTY not in fresh_object,
        "a refused create leaves the object without a link id",
    )
    check(
        addon.SESSION_PROPERTY not in fresh_object,
        "a refused create leaves the object without a session",
    )
    check(
        addon.link_state.connect_problem == "Create failed: the asset already exists",
        "the Pill block shows the editor's reason",
    )
    check(
        fresh_object.name not in addon.link_state.stream_caches,
        "a refused create remembers no baseline",
    )

    # Step 5: without a connection nothing is sent, and the sidebar says so.
    addon.disconnect("probe finished")
    sent_frames.clear()
    problem = addon.request_create(fresh_object, picked_path)
    check(
        problem == "not connected to the editor",
        "a create with no connection is refused with the reason",
    )
    check(sent_frames == [], "a create with no connection sends nothing")
    check(
        addon.connection_status_text() == "Not connected",
        "a closed link reads as not connected",
    )
    check(
        addon.connection_detail_text() == "Not connected (probe finished)",
        "a closed link says why it is closed",
    )

    # Step 6: the status is re-derived on the timer's own cadence, so a
    # sidebar that was never asked to repaint still catches up.
    addon.link_state.status_checked_at = 0.0
    addon.link_state.shown_status = ""
    addon.check_connection_status()
    check(
        addon.link_state.shown_status == "Not connected",
        "the periodic check records the label it shows",
    )

    print(f"[probe] {len(CHECKS)} checks passed")


main()
