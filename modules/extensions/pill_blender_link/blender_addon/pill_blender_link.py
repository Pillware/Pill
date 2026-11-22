# REQUIREMENTS
#   Blender 3.6 or newer (tested API surface: 3.6 - 4.x). Python's standard
#   library only; nothing to install.
#
# DESCRIPTION
#   The Blender half of the Pill Engine live mesh link. It connects to the Pill
#   editor over a local socket, opens the meshes the editor sends (the Blender
#   button on a MeshRendererComponent's Inspector header), and streams every
#   change made to such a mesh - in Object or Edit mode, modifiers applied -
#   back to the editor, which writes it straight into the mesh asset in memory.
#   Nothing is saved to disk on either side.
#
#   Geometry crosses the link in Pill's conventions (Y up, texture V counted
#   from the top edge); this script converts to and from Blender's Z up,
#   bottom-up V on its side.
#
#   A drag that keeps the mesh's topology (the triangles, the corner to
#   vertex mapping and the UVs) sends only positions and normals
#   (`mesh_positions`), written for the vertex order of the last full update
#   the editor received; anything else sends the whole mesh (`mesh_update`).
#   The export reads the mesh through NumPy, so heavy meshes stream smoothly.
#
#   The editor holds a streamed change in memory until it is saved. An undo or
#   redo in the editor replaces the object's mesh here too. The "Pill" sidebar
#   tab carries the connection: its status, the port and one
#   connect/disconnect button.
#
#   The entity's material comes with the mesh: the editor reads the PBR
#   shader's surface and sends the node graph the object's material shows - a
#   Principled BSDF fed from its base colour, roughness, metallic, normal and
#   emission, with the material's parameters as named nodes and its textures
#   loaded from the project's files. A parameter edit sends only the values and
#   a repainted texture only the image, so the node tree survives both.
#
#   The other direction starts in the Properties editor's Object tab, in a
#   "Pill" block: an object that is not linked yet offers "Link to existing
#   Pill mesh" (a file dialog over the project's res) and "Create new Pill
#   mesh" (the same dialog, writing a new file there); a linked object offers
#   only "Disconnect from Pill mesh". The block greys out while Blender is not
#   connected to the editor, and a refused request is the only text it adds.
#
# USAGE
#   Started by the editor (the usual way):
#     blender --python pill_blender_link.py -- --pill-link-port <port>
#   Installed as an add-on (the editor installs a loader for it the first time
#   the Blender button is used, or Edit > Preferences > Add-ons > Install from
#   disk): it connects to port 47390 - or PILL_BLENDER_LINK_PORT, or the port
#   set in the "Pill" sidebar tab of the 3D viewport - and keeps retrying while
#   the editor is not running. The editor keeps one Blender linked at a time;
#   another one waits and takes over when the first closes.
#
#   --pill-link-port <port>   The port the editor listens on (default 47390).
#
# EXAMPLE USAGE
#   "C:\Program Files\Blender Foundation\Blender 4.2\blender.exe" --python pill_blender_link.py -- --pill-link-port 47390
#
# --- SCRIPT ---

bl_info = {
    "name": "Pill Engine Live Link",
    "author": "Pill Engine",
    "version": (1, 4, 0),
    "blender": (3, 6, 0),
    "location": "3D Viewport > Sidebar > Pill",
    "description": "Edit Pill Engine meshes and materials in Blender and stream every mesh change back to the editor",
    "category": "Import-Export",
}

import json
import os
import select
import socket
import struct
import sys
import time

import bpy
import numpy as np
from bpy.app.handlers import persistent

# =============================================================================
# Constants
# =============================================================================

PROTOCOL_VERSION = 1
DEFAULT_PORT = 47390
# How often the link's timer runs, in seconds: it reads the socket and sends
# the edits made since the last run, so it also caps the streaming rate.
TICK_INTERVAL_SECONDS = 1.0 / 30.0
# How often the sidebar's connection status is re-derived from the link, in
# seconds. The timer itself runs far more often; this only decides when the
# sidebar is asked to repaint.
STATUS_CHECK_INTERVAL_SECONDS = 3.0
# How long to wait between attempts to reach an editor that is not running.
RECONNECT_INTERVAL_SECONDS = 2.0
# How long to wait after the editor answered that another Blender is linked.
BUSY_RETRY_SECONDS = 5.0
# The environment variable that names the editor's port; a Blender the editor
# started inherits it.
PORT_ENVIRONMENT_VARIABLE = "PILL_BLENDER_LINK_PORT"
# The object properties that tie a Blender object to a Pill mesh asset.
LINK_ID_PROPERTY = "pill_link_id"
SESSION_PROPERTY = "pill_link_session"
# The asset name (`models/teapot.obj`, the path below the project's res) the
# last answer named, shown in the Pill block.
ASSET_NAME_PROPERTY = "pill_linked_asset"
# The object's switch that merges the geometry a Geometry Nodes modifier
# places as instances into the one mesh the link sends.
REALIZE_INSTANCES_PROPERTY = "pill_realize_instances"
# Corner normals closer than this (as a dot product) count as the same normal.
SAME_NORMAL_DOT = 0.9999
# How far apart (per component) the normals of corners merged into one vertex
# may drift before a positions-only update cannot carry them.
MERGED_NORMAL_TOLERANCE = 1e-4


# =============================================================================
# Link state
# =============================================================================

# Holds the connection and what the timer needs between runs. Module-level,
# because Blender's handlers and timers are plain functions.
class LinkState:
    def __init__(self):
        self.port = DEFAULT_PORT
        self.connection = None
        self.receive_buffer = bytearray()
        self.session = None
        # What the sidebar says about the connection, and why it says it: the
        # label is derived from the socket, the detail from how it got here.
        self.status = "Not connected"
        # The Pill editor's window title, from its `welcome` frame; the sidebar
        # shows it as the connection's status line.
        self.editor_title = None
        # The project's `res` folder, from the same frame: the two pickers of
        # the Object tab's Pill block start there, and the editor refuses a
        # create outside it.
        self.project_res_directory = None
        # When the status was last derived, and the label it produced, so the
        # timer repaints the sidebar only when the label changes.
        self.status_checked_at = 0.0
        self.shown_status = ""
        # Why the last connect or create attempt was refused, by this side or
        # by the editor. The Object tab's Pill block shows this and nothing
        # else: a link that works is visible through its button, so it says
        # nothing.
        self.connect_problem = ""
        self.last_connect_attempt = 0.0
        # The earliest time to try connecting again; the editor's `busy`
        # answer pushes it out.
        self.retry_after = 0.0
        self.dirty_object_names = set()
        self.enabled = True
        # Per object name, what the editor last received from it and how its
        # corners map onto those vertices (a `StreamCache`). A state equal to
        # it is not sent again: opening or replacing a mesh, or clicking a
        # selection in Edit mode, changes nothing the editor needs to hear
        # about. It is also the base positions-only updates are written for.
        self.stream_caches = {}
        # The id the next full update carries; a positions-only update names
        # the full update whose vertex order it uses.
        self.next_topology_id = 1
        # The material each link shows, by link id: the material's name in
        # `bpy.data.materials`. Node names carry what a values or images
        # message addresses, so nothing else has to be remembered.
        self.material_names = {}
        # The object and picked path a create request was sent for, until the
        # editor answers: the object carries no link id yet, so the answer is
        # matched to it by name.
        self.pending_create = None


link_state = LinkState()


# Encodes one frame: header length, body length, JSON header, body.
def encode_frame(header, body=b""):
    header_bytes = json.dumps(header).encode("utf-8")
    return struct.pack("<II", len(header_bytes), len(body)) + header_bytes + body


# Sends one frame, dropping the connection when the editor is gone.
def send_frame(header, body=b""):
    if link_state.connection is None:
        return
    try:
        link_state.connection.sendall(encode_frame(header, body))
    except OSError as send_error:
        disconnect(f"send failed: {send_error}")


# Closes the connection and records why.
def disconnect(reason):
    if link_state.connection is not None:
        try:
            link_state.connection.close()
        except OSError:
            pass
    link_state.connection = None
    link_state.receive_buffer.clear()
    link_state.session = None
    link_state.stream_caches.clear()
    link_state.material_names.clear()
    link_state.pending_create = None
    # The title and the res folder belong to the editor that answered; the
    # next `welcome` brings its own.
    link_state.editor_title = None
    link_state.project_res_directory = None
    link_state.status = f"Not connected ({reason})"
    redraw_sidebars()
    print(f"[pill_blender_link] disconnected: {reason}")


# Opens the connection to the editor and greets it. Sends block with a
# timeout so a large mesh is written whole; reads go through `select`.
def try_connect():
    link_state.last_connect_attempt = time.monotonic()
    try:
        connection = socket.create_connection(("127.0.0.1", link_state.port), timeout=0.25)
    except OSError:
        link_state.status = f"Waiting for the Pill editor on port {link_state.port}"
        return
    connection.settimeout(5.0)
    connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    link_state.connection = connection
    link_state.status = ""
    print(f"[pill_blender_link] connected to the Pill editor on port {link_state.port}")
    send_frame(
        {
            "type": "hello",
            "protocol": PROTOCOL_VERSION,
            "blender_version": bpy.app.version_string,
        }
    )


# Reads what the socket holds and returns the complete frames in it. The
# frames read before the editor closed the connection are still returned (its
# `busy` answer arrives that way), and the connection is dropped after them.
def receive_frames():
    frames = []
    connection = link_state.connection
    if connection is None:
        return frames
    # Step 1: read while the socket has bytes, without blocking.
    closed_reason = None
    while True:
        try:
            readable, _, _ = select.select([connection], [], [], 0)
        except (OSError, ValueError) as select_error:
            closed_reason = f"read failed: {select_error}"
            break
        if not readable:
            break
        try:
            chunk = connection.recv(1 << 16)
        except OSError as receive_error:
            closed_reason = f"read failed: {receive_error}"
            break
        if not chunk:
            closed_reason = "closed by the editor"
            break
        link_state.receive_buffer.extend(chunk)
    # Step 2: cut every complete frame off the buffer.
    buffer = link_state.receive_buffer
    while len(buffer) >= 8:
        header_length, body_length = struct.unpack_from("<II", buffer, 0)
        frame_length = 8 + header_length + body_length
        if len(buffer) < frame_length:
            break
        header = json.loads(bytes(buffer[8 : 8 + header_length]).decode("utf-8"))
        body = bytes(buffer[8 + header_length : frame_length])
        del buffer[:frame_length]
        frames.append((header, body))
    # Step 3: a closed connection goes once its last frames are out.
    if closed_reason is not None:
        disconnect(closed_reason)
    return frames


# =============================================================================
# Coordinate conversion
# =============================================================================

# Pill is Y up, Blender is Z up; both are right-handed, so this is a rotation
# about X and keeps the triangles' winding.
def pill_to_blender(vector):
    return (vector[0], -vector[2], vector[1])


def blender_to_pill(vector):
    return (vector[0], vector[2], -vector[1])


# =============================================================================
# Opening a mesh from the editor
# =============================================================================

# Reads a geometry body into positions, texture coordinates, normals and
# indices, in Pill's conventions.
def decode_geometry(body, vertex_count, index_count):
    float_count = vertex_count * 8
    floats = struct.unpack_from(f"<{float_count}f", body, 0)
    indices = struct.unpack_from(f"<{index_count}I", body, float_count * 4)
    positions = [floats[i * 3 : i * 3 + 3] for i in range(vertex_count)]
    texture_offset = vertex_count * 3
    texture_coordinates = [
        floats[texture_offset + i * 2 : texture_offset + i * 2 + 2] for i in range(vertex_count)
    ]
    normal_offset = vertex_count * 5
    normals = [floats[normal_offset + i * 3 : normal_offset + i * 3 + 3] for i in range(vertex_count)]
    return positions, texture_coordinates, normals, indices


def dot(first, second):
    return first[0] * second[0] + first[1] * second[1] + first[2] * second[2]


def face_normal(first, second, third):
    edge_a = (second[0] - first[0], second[1] - first[1], second[2] - first[2])
    edge_b = (third[0] - first[0], third[1] - first[1], third[2] - first[2])
    cross = (
        edge_a[1] * edge_b[2] - edge_a[2] * edge_b[1],
        edge_a[2] * edge_b[0] - edge_a[0] * edge_b[2],
        edge_a[0] * edge_b[1] - edge_a[1] * edge_b[0],
    )
    length = dot(cross, cross) ** 0.5
    if length == 0.0:
        return (0.0, 0.0, 1.0)
    return (cross[0] / length, cross[1] / length, cross[2] / length)


# Builds a Blender mesh from Pill's single-indexed vertices. Vertices that
# share a position are welded, so the mesh edits as one surface; the split
# normals Pill carried become flat faces and sharp edges, so Blender's own
# normals reproduce the asset's shading and stay right while it is edited.
def build_blender_mesh(name, positions, texture_coordinates, normals, indices):
    # Step 1: weld the vertices by position.
    welded_index_of_position = {}
    welded_positions = []
    welded_index_of_vertex = []
    for position in positions:
        key = tuple(position)
        welded_index = welded_index_of_position.get(key)
        if welded_index is None:
            welded_index = len(welded_positions)
            welded_index_of_position[key] = welded_index
            welded_positions.append(pill_to_blender(position))
        welded_index_of_vertex.append(welded_index)

    # Step 2: keep the triangles that are still triangles after welding.
    kept_triangles = []
    faces = []
    for triangle_start in range(0, len(indices), 3):
        corners = indices[triangle_start : triangle_start + 3]
        welded = [welded_index_of_vertex[corner] for corner in corners]
        if len(set(welded)) == 3:
            kept_triangles.append(corners)
            faces.append(welded)

    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(welded_positions, [], faces)

    # Step 3: texture coordinates per corner; Blender counts V from the bottom.
    uv_layer = mesh.uv_layers.new(name="UVMap")
    for loop_index, corner in enumerate(corner for triangle in kept_triangles for corner in triangle):
        u_coordinate, v_coordinate = texture_coordinates[corner]
        uv_layer.data[loop_index].uv = (u_coordinate, 1.0 - v_coordinate)

    # Step 4: a face whose corner normals all match its own normal is flat.
    smooth_flags = []
    for corners in kept_triangles:
        flat_normal = face_normal(*(positions[corner] for corner in corners))
        smooth_flags.append(any(dot(normals[corner], flat_normal) < SAME_NORMAL_DOT for corner in corners))
    mesh.polygons.foreach_set("use_smooth", smooth_flags)

    # Step 5: an edge whose two faces use different normals at either end is
    # sharp: the asset had a seam in its shading there.
    corner_normal = {}
    faces_of_edge = {}
    for face_index, corners in enumerate(kept_triangles):
        welded = [welded_index_of_vertex[corner] for corner in corners]
        for corner_position in range(3):
            corner_normal[(face_index, welded[corner_position])] = normals[corners[corner_position]]
            edge_key = tuple(sorted((welded[corner_position], welded[(corner_position + 1) % 3])))
            faces_of_edge.setdefault(edge_key, []).append(face_index)
    sharp_edge_keys = set()
    for edge_key, edge_faces in faces_of_edge.items():
        if len(edge_faces) != 2:
            continue
        first_face, second_face = edge_faces
        for vertex in edge_key:
            if dot(corner_normal[(first_face, vertex)], corner_normal[(second_face, vertex)]) < SAME_NORMAL_DOT:
                sharp_edge_keys.add(edge_key)
                break
    for edge in mesh.edges:
        if tuple(sorted(edge.vertices)) in sharp_edge_keys:
            edge.use_edge_sharp = True
    # Before 4.1 sharp edges only count while auto smooth is on.
    if hasattr(mesh, "use_auto_smooth"):
        mesh.use_auto_smooth = True
        mesh.auto_smooth_angle = 3.14159

    mesh.validate(clean_customdata=False)
    mesh.update()
    return mesh


# Records the object's current evaluated geometry as what the editor already
# has, so the next depsgraph update does not send it back.
# The editor built its mesh itself, in its own vertex order, so the cache is
# marked as not sent: the next change goes as a full update.
def remember_baseline(scene_object):
    bpy.context.view_layer.update()
    depsgraph = bpy.context.evaluated_depsgraph_get()
    mesh_arrays = read_object_arrays(scene_object, depsgraph)
    encoding = encode_full(mesh_arrays)
    if encoding is None:
        link_state.stream_caches.pop(scene_object.name, None)
    else:
        link_state.stream_caches[scene_object.name] = StreamCache(mesh_arrays, encoding, None, sent=False)
    link_state.dirty_object_names.discard(scene_object.name)


# The editor's mesh is no longer in the vertex order of this object's last
# full update (an undo, a re-import): send the whole mesh on the next tick,
# even when it has not changed.
def resync(header):
    scene_object = find_linked_object(header.get("link_id"))
    if scene_object is None:
        return
    stream_cache = link_state.stream_caches.get(scene_object.name)
    if stream_cache is not None:
        stream_cache.sent = False
        stream_cache.full_body = None
    link_state.dirty_object_names.add(scene_object.name)


# Replaces a linked object's mesh with the geometry the editor sent after an
# undo or redo there. An object in Edit mode leaves it for the swap and comes
# back to it, so the person keeps working where they were.
def replace_mesh(header, body):
    scene_object = find_linked_object(header["link_id"])
    if scene_object is None:
        return
    positions, texture_coordinates, normals, indices = decode_geometry(
        body, header["vertex_count"], header["index_count"]
    )
    was_editing = scene_object.mode == "EDIT"
    if was_editing:
        with bpy.context.temp_override(active_object=scene_object, object=scene_object):
            bpy.ops.object.mode_set(mode="OBJECT")
    old_mesh = scene_object.data
    new_mesh = build_blender_mesh(old_mesh.name, positions, texture_coordinates, normals, indices)
    for material in old_mesh.materials:
        new_mesh.materials.append(material)
    scene_object.data = new_mesh
    if old_mesh.users == 0:
        bpy.data.meshes.remove(old_mesh)
    if was_editing:
        with bpy.context.temp_override(active_object=scene_object, object=scene_object):
            bpy.ops.object.mode_set(mode="EDIT")
    remember_baseline(scene_object)
    print(f"[pill_blender_link] replaced '{scene_object.name}' from the editor (undo or redo)")


# Repaints every 3D viewport's sidebar, so a status change shows at once.
def redraw_sidebars():
    window_manager = getattr(bpy.context, "window_manager", None)
    if window_manager is None:
        return
    for window in window_manager.windows:
        for area in window.screen.areas:
            if area.type == "VIEW_3D":
                area.tag_redraw()


# The Connection section's status line: the editor's own name while the socket
# is up, and plainly "Not connected" otherwise.
def connection_status_text():
    if link_state.connection is None:
        return "Not connected"
    return f"Connected to: {link_state.editor_title or 'the Pill editor'}"


# The line under the status: why the link is where it is, which only says
# something while there is no connection.
def connection_detail_text():
    if link_state.connection is not None:
        return ""
    return link_state.status


# Re-derives the sidebar's connection status every
# STATUS_CHECK_INTERVAL_SECONDS and repaints when it changed. The timer reads
# the socket on every tick, so a dropped connection is noticed there; this is
# what keeps the label current without repainting the sidebar on every tick.
def check_connection_status():
    now = time.monotonic()
    if now - link_state.status_checked_at < STATUS_CHECK_INTERVAL_SECONDS:
        return
    link_state.status_checked_at = now
    status_text = connection_status_text()
    if status_text != link_state.shown_status:
        link_state.shown_status = status_text
        redraw_sidebars()


# Returns the object linked to `link_id` in this editor session, if any.
def find_linked_object(link_id):
    for scene_object in bpy.data.objects:
        if scene_object.get(LINK_ID_PROPERTY) == link_id and scene_object.get(SESSION_PROPERTY) == link_state.session:
            return scene_object
    return None


# Makes `scene_object` the only selected and the active object.
def select_only(scene_object):
    view_layer = bpy.context.view_layer
    for selected in list(bpy.context.selected_objects):
        selected.select_set(False)
    scene_object.select_set(True)
    view_layer.objects.active = scene_object


# Frames the selection in every 3D viewport, when there is one to frame in.
def frame_selection():
    window_manager = bpy.context.window_manager
    for window in window_manager.windows:
        for area in window.screen.areas:
            if area.type != "VIEW_3D":
                continue
            region = next((region for region in area.regions if region.type == "WINDOW"), None)
            if region is None:
                continue
            try:
                with bpy.context.temp_override(window=window, area=area, region=region):
                    bpy.ops.view3d.view_selected()
            except (RuntimeError, AttributeError):
                pass


# Opens a mesh the editor sent: a new object the first time, the existing one
# selected again afterwards, so Blender's edits are never overwritten.
def open_mesh(header, body):
    link_id = header["link_id"]
    existing = find_linked_object(link_id)
    if existing is not None:
        select_only(existing)
        frame_selection()
        return
    positions, texture_coordinates, normals, indices = decode_geometry(
        body, header["vertex_count"], header["index_count"]
    )
    name = header.get("name") or f"pill_mesh_{link_id}"
    # A new object can only be selected and framed from Object mode.
    if bpy.context.mode != "OBJECT":
        try:
            bpy.ops.object.mode_set(mode="OBJECT")
        except RuntimeError:
            pass
    mesh = build_blender_mesh(name, positions, texture_coordinates, normals, indices)
    scene_object = bpy.data.objects.new(name, mesh)
    scene_object[LINK_ID_PROPERTY] = link_id
    scene_object[SESSION_PROPERTY] = link_state.session
    bpy.context.scene.collection.objects.link(scene_object)
    select_only(scene_object)
    remember_baseline(scene_object)
    # A new object has no node tree: ask the editor for the entity's material.
    send_frame({"type": "material_request", "session": link_state.session, "link_id": link_id})
    frame_selection()
    print(f"[pill_blender_link] opened '{name}' (link {link_id})")


# =============================================================================
# Showing the entity's material
# =============================================================================

# The Principled BSDF input each surface field feeds, by the name Blender 4.x
# uses and the name older versions used.
PRINCIPLED_FIELD_SOCKETS = (
    ("base_color", "Base Color", None),
    ("roughness", "Roughness", None),
    ("metallic", "Metallic", None),
    ("normal", "Normal", None),
    ("emission", "Emission Color", "Emission"),
)


# Returns the Principled input named `socket_name`, or its older name when this
# Blender does not have it.
def principled_input(principled_node, socket_name, fallback_name=None):
    socket = principled_node.inputs.get(socket_name)
    if socket is None and fallback_name is not None:
        socket = principled_node.inputs.get(fallback_name)
    return socket


# The node a material parameter's value lives in. Named after the parameter, so
# a values message finds it without being told where it is.
def parameter_node_name(parameter_name):
    return f"pill_param_{parameter_name}"


# The node a texture slot's image lives in.
def texture_node_name(texture_slot):
    return f"pill_texture_{texture_slot}"


# The type of the node that splits a colour into channels: Blender 4.x renamed
# it and kept its function.
def separate_color_node_type():
    if hasattr(bpy.types, "ShaderNodeSeparateColor"):
        return "ShaderNodeSeparateColor"
    return "ShaderNodeSeparateRGB"


# The output a channel names on either colour splitter: both order their
# sockets red, green, blue.
def channel_output_index(channel):
    return {"Red": 0, "Green": 1, "Blue": 2}.get(channel, 0)


# The material a link shows, by its name; made on first use. The name comes
# from the material asset's file name, so two links to the same asset share one
# Blender material.
def link_material(link_id, asset_name):
    material_name = os.path.basename(asset_name or "").strip() or f"pill_material_{link_id}"
    if material_name.lower().endswith(".material"):
        material_name = material_name[: -len(".material")]
    previous_name = link_state.material_names.get(link_id)
    material = bpy.data.materials.get(previous_name) if previous_name else None
    if material is None:
        material = bpy.data.materials.get(material_name)
    if material is None:
        material = bpy.data.materials.new(material_name)
    link_state.material_names[link_id] = material.name
    return material


# The material a link showed, once it has one.
def linked_material(link_id):
    material_name = link_state.material_names.get(link_id)
    if material_name is None:
        return None
    return bpy.data.materials.get(material_name)


# Loads a texture from its absolute path and marks how to read its texels. A
# file that cannot be read leaves the node without an image; a later images
# message fills it in.
def load_material_image(absolute_path, color_space):
    if not absolute_path:
        return None
    try:
        image = bpy.data.images.load(absolute_path, check_existing=True)
    except RuntimeError as load_error:
        print(f"[pill_blender_link] could not load '{absolute_path}': {load_error}")
        return None
    apply_colorspace(image, color_space)
    return image


# Marks an image as colour data or as plain data; a normal map read as colour
# would come out wrong at every edge.
def apply_colorspace(image, color_space):
    wanted = "Non-Color" if color_space == "NonColor" else "sRGB"
    try:
        image.colorspace_settings.name = wanted
    except TypeError:
        print(f"[pill_blender_link] this Blender has no '{wanted}' colour space")


# Links the input `description` names to `target_socket`, when it names a node
# of this graph. An unconnected input keeps what the node's own default is.
def connect_graph_input(node_tree, target_socket, description, node_outputs):
    if not isinstance(description, dict):
        return
    source_index = description.get("Node")
    if not isinstance(source_index, int) or not 0 <= source_index < len(node_outputs):
        return
    node_tree.links.new(node_outputs[source_index], target_socket)


# Builds the graph's nodes in their serialized order and returns the output
# socket each index names. A node's inputs always come before it, so one pass
# makes them all and a second connects them.
def build_material_nodes(node_tree, nodes):
    created_nodes = []
    for description in nodes:
        if not isinstance(description, dict) or not description:
            continue
        kind, settings = next(iter(description.items()))
        if not isinstance(settings, dict):
            continue
        inputs = []
        output_index = 0
        if kind == "Value":
            node = node_tree.nodes.new("ShaderNodeValue")
            parameter_name = settings.get("name", "")
            if parameter_name:
                node.name = parameter_node_name(parameter_name)
                node.label = parameter_name
            node.outputs[0].default_value = float(settings.get("value", 0.0))
        elif kind == "Rgb":
            node = node_tree.nodes.new("ShaderNodeRGB")
            parameter_name = settings.get("name", "")
            if parameter_name:
                node.name = parameter_node_name(parameter_name)
                node.label = parameter_name
            red, green, blue = settings.get("value", [0.0, 0.0, 0.0])
            node.outputs[0].default_value = (red, green, blue, 1.0)
        elif kind == "Image":
            node = node_tree.nodes.new("ShaderNodeTexImage")
            node.name = texture_node_name(settings.get("slot", ""))
            node.label = settings.get("slot", "")
            node.image = load_material_image(
                settings.get("absolute_path", ""), settings.get("color_space", "Srgb")
            )
        elif kind == "Component":
            node = node_tree.nodes.new(separate_color_node_type())
            output_index = channel_output_index(settings.get("channel", "Red"))
            inputs = [(node.inputs[0], settings.get("input"))]
        elif kind in ("Multiply", "Add"):
            node = node_tree.nodes.new("ShaderNodeVectorMath")
            node.operation = "MULTIPLY" if kind == "Multiply" else "ADD"
            inputs = [(node.inputs[0], settings.get("first")), (node.inputs[1], settings.get("second"))]
        elif kind == "Saturate":
            node = node_tree.nodes.new("ShaderNodeClamp")
            node.clamp_type = "MINMAX"
            inputs = [(node.inputs["Value"], settings.get("input"))]
        elif kind == "NormalMap":
            node = node_tree.nodes.new("ShaderNodeNormalMap")
            inputs = [(node.inputs["Color"], settings.get("input"))]
        else:
            print(f"[pill_blender_link] unknown material node '{kind}'")
            continue
        created_nodes.append((node, output_index, inputs))
    outputs = [node.outputs[output_index] for node, output_index, _ in created_nodes]
    for _, _, node_inputs in created_nodes:
        for target_socket, description in node_inputs:
            connect_graph_input(node_tree, target_socket, description, outputs)
    return outputs


# Applies a whole graph: the tree is rebuilt from it, so node edits made in
# Blender are discarded. That is what a full send means - the graph is what the
# shader computes, and a half-rebuilt tree would shade differently.
def apply_material(header, body):
    link_id = header["link_id"]
    scene_object = find_linked_object(link_id)
    if scene_object is None:
        print(f"[pill_blender_link] material for link {link_id} arrived with no linked object")
        return
    graph = json.loads(body.decode("utf-8")) if body else {}
    material = link_material(link_id, header.get("name", ""))
    material.use_nodes = True
    node_tree = material.node_tree
    node_tree.nodes.clear()

    # Step 1: the output and the surface shader every field feeds.
    output_node = node_tree.nodes.new("ShaderNodeOutputMaterial")
    output_node.location = (600.0, 0.0)
    principled_node = node_tree.nodes.new("ShaderNodeBsdfPrincipled")
    principled_node.location = (300.0, 0.0)
    node_tree.links.new(principled_node.outputs["BSDF"], output_node.inputs["Surface"])
    emission_strength = principled_input(principled_node, "Emission Strength")
    if emission_strength is not None:
        emission_strength.default_value = 1.0

    # Step 2: the nodes the five fields are built from.
    node_outputs = build_material_nodes(node_tree, graph.get("nodes", []))

    # Step 3: the five inputs themselves.
    surface_inputs = graph.get("principled", {})
    for field, socket_name, fallback_name in PRINCIPLED_FIELD_SOCKETS:
        target_socket = principled_input(principled_node, socket_name, fallback_name)
        if target_socket is None:
            print(f"[pill_blender_link] this Blender has no Principled '{socket_name}' input")
            continue
        connect_graph_input(node_tree, target_socket, surface_inputs.get(field), node_outputs)

    # Step 4: the object shows it in its first slot; any other slot stays.
    mesh = scene_object.data
    if len(mesh.materials) == 0:
        mesh.materials.append(material)
    else:
        mesh.materials[0] = material

    # Step 5: say what was built, and which fields the editor could not read.
    refused_fields = [refusal.get("field", "") for refusal in graph.get("refusals", [])]
    refused_note = f", refused: {', '.join(refused_fields)}" if refused_fields else ""
    print(
        f"[pill_blender_link] built material '{material.name}' on '{scene_object.name}'"
        f" ({len(node_outputs)} node(s){refused_note})"
    )


# Applies new parameter values to the nodes the graph already built. The tree
# is not touched, so a slider edit costs one value and nothing else.
def apply_material_values(header, body):
    link_id = header["link_id"]
    material = linked_material(link_id)
    if material is None:
        print(f"[pill_blender_link] values for link {link_id} arrived with no material")
        return
    parameters = json.loads(body.decode("utf-8")) if body else {}
    nodes = material.node_tree.nodes
    updated = 0
    for parameter_name, value in parameters.items():
        node = nodes.get(parameter_node_name(parameter_name))
        if node is None or not isinstance(value, dict):
            # A parameter the graph does not carry: the next full graph adds it.
            continue
        if "Scalar" in value:
            node.outputs[0].default_value = float(value["Scalar"])
            updated += 1
        elif "Color" in value:
            red, green, blue = value["Color"]
            node.outputs[0].default_value = (red, green, blue, 1.0)
            updated += 1
    print(f"[pill_blender_link] updated {updated} value(s) of material '{material.name}'")


# Reloads the listed textures in place. The tree is not touched either, so a
# repainted texture costs a reload and nothing else.
def apply_material_images(header):
    link_id = header["link_id"]
    material = linked_material(link_id)
    if material is None:
        print(f"[pill_blender_link] images for link {link_id} arrived with no material")
        return
    nodes = material.node_tree.nodes
    reloaded = 0
    for description in header.get("images", []):
        node = nodes.get(texture_node_name(description.get("slot", "")))
        if node is None:
            # The node was deleted in Blender; the next full graph puts it back.
            continue
        absolute_path = description.get("absolute_path", "")
        color_space = description.get("color_space", "Srgb")
        if not absolute_path:
            # The texture's file did not resolve: reload what Blender holds.
            if node.image is not None:
                node.image.reload()
                reloaded += 1
        elif node.image is None:
            node.image = load_material_image(absolute_path, color_space)
            reloaded += 1
        else:
            node.image.filepath = absolute_path
            node.image.reload()
            apply_colorspace(node.image, color_space)
            reloaded += 1
    print(f"[pill_blender_link] reloaded {reloaded} image(s) of material '{material.name}'")


# =============================================================================
# Streaming edits back to the editor
# =============================================================================

# Sends the selected object's evaluated geometry to a file the person picked
# below the project's res, and asks the editor to import it as a new asset.
# Returns the reason it cannot be sent, or None when the request went out and
# the answer is awaited in `link_state.pending_create`.
def request_create(scene_object, picked_path):
    if link_state.connection is None:
        return "not connected to the editor"
    if LINK_ID_PROPERTY in scene_object:
        return f"'{scene_object.name}' is already linked to the editor"
    picked_path = (picked_path or "").strip()
    if not picked_path:
        return "pick the file to create first"
    # The evaluated object is what the viewport shows: modifiers applied, in
    # object space, before the object's own transform.
    bpy.context.view_layer.update()
    depsgraph = bpy.context.evaluated_depsgraph_get()
    mesh_arrays = read_object_arrays(scene_object, depsgraph)
    encoding = encode_full(mesh_arrays)
    if encoding is None:
        return f"'{scene_object.name}' has no triangles to send"
    link_state.pending_create = {"object": scene_object.name, "path": picked_path}
    link_state.connect_problem = ""
    send_frame(
        {
            "type": "create_mesh",
            "session": link_state.session,
            "path": picked_path,
            "vertex_count": len(encoding.representative_loops),
            "index_count": encoding.index_count,
        },
        encoding.body,
    )
    return None


# Takes the editor's answer to a create request. A created asset becomes the
# object's link - properties set, baseline remembered, material asked for; a
# refusal only records the reason for the Pill block.
def handle_create_result(header):
    pending = link_state.pending_create
    link_state.pending_create = None
    if not header.get("ok"):
        detail = header.get("detail") or "the editor refused it"
        link_state.connect_problem = f"Create failed: {detail}"
        print(f"[pill_blender_link] create refused: {detail}")
        redraw_sidebars()
        return
    link_id = header.get("link_id")
    scene_object = bpy.data.objects.get(pending["object"]) if pending is not None else None
    if scene_object is None:
        redraw_sidebars()
        return
    scene_object[LINK_ID_PROPERTY] = link_id
    scene_object[SESSION_PROPERTY] = link_state.session
    scene_object[ASSET_NAME_PROPERTY] = header.get("asset_name") or ""
    # The asset was written from this object, so nothing is sent back until the
    # object changes - and the first change after that goes as a full update,
    # because the editor's vertex order is its own.
    remember_baseline(scene_object)
    link_state.connect_problem = ""
    send_frame({"type": "material_request", "session": link_state.session, "link_id": link_id})
    redraw_sidebars()


# Asks the editor to link this object to the asset the person picked: a file
# the project already holds. Returns the reason it cannot be asked, or None
# when the request went out.
def request_connect(scene_object, picked_path):
    if link_state.connection is None:
        return "not connected to the editor"
    if LINK_ID_PROPERTY in scene_object:
        return f"'{scene_object.name}' is already linked to the editor"
    picked_path = (picked_path or "").strip()
    if not picked_path:
        return "pick the file to link to first"
    link_state.connect_problem = ""
    send_frame(
        {
            "type": "connect_asset",
            "session": link_state.session,
            "object": scene_object.name,
            "path": picked_path,
        }
    )
    return None


# Takes the editor's answer to a connect request: the object becomes a link, so
# its edits stream into the asset from now on. A refusal only records the
# reason, which the Object tab's Pill block shows in place of a status line.
def handle_connect_result(header):
    object_name = header.get("object", "")
    if not header.get("ok"):
        detail = header.get("detail") or "the editor refused it"
        link_state.connect_problem = detail
        print(f"[pill_blender_link] connect refused: {detail}")
        redraw_sidebars()
        return
    scene_object = bpy.data.objects.get(object_name)
    if scene_object is None:
        link_state.connect_problem = f"'{object_name}' is gone"
        redraw_sidebars()
        return
    link_id = header.get("link_id")
    scene_object[LINK_ID_PROPERTY] = link_id
    scene_object[SESSION_PROPERTY] = link_state.session
    scene_object[ASSET_NAME_PROPERTY] = header.get("asset_name") or ""
    # What the object holds is what the person connected, so it goes to the
    # editor at once: the cache is dropped and the object marked dirty, which
    # the tick's own streaming step turns into one full update in this frame -
    # no baseline, because the editor has not seen this geometry yet.
    link_state.stream_caches.pop(scene_object.name, None)
    link_state.dirty_object_names.add(scene_object.name)
    link_state.connect_problem = ""
    redraw_sidebars()


# The arrays one export reads from an evaluated mesh, all copies: the
# triangles' corners (as loop indices), each loop's vertex, the vertex
# positions, each loop's normal and each loop's UV.
class MeshArrays:
    def __init__(self, triangle_loops, loop_vertices, vertex_positions, loop_normals, loop_uvs):
        self.triangle_loops = triangle_loops
        self.loop_vertices = loop_vertices
        self.vertex_positions = vertex_positions
        self.loop_normals = loop_normals
        self.loop_uvs = loop_uvs


# One full export: the body of a `mesh_update`, the positions-and-normals part
# of it, and how corners map onto the exported vertices.
class FullEncoding:
    def __init__(self, body, positions_and_normals, representative_loops, corner_to_vertex, index_count):
        self.body = body
        self.positions_and_normals = positions_and_normals
        self.representative_loops = representative_loops
        self.corner_to_vertex = corner_to_vertex
        self.vertex_count = len(representative_loops)
        self.index_count = index_count


# What the editor last received from one object, and the mapping a
# positions-only update reuses.
class StreamCache:
    def __init__(self, mesh_arrays, encoding, topology_id, sent):
        self.triangle_loops = mesh_arrays.triangle_loops
        self.loop_vertices = mesh_arrays.loop_vertices
        self.loop_uvs = mesh_arrays.loop_uvs
        self.representative_loops = encoding.representative_loops
        self.corner_to_vertex = encoding.corner_to_vertex
        self.topology_id = topology_id
        # Whether the editor holds this export in this vertex order; only then
        # can positions-only updates be written for it.
        self.sent = sent
        self.full_body = encoding.body
        self.positions_and_normals = encoding.positions_and_normals


# Reads an evaluated mesh into arrays with `foreach_get`, which copies whole
# attributes at C speed instead of visiting each element from Python.
def read_mesh_arrays(mesh):
    mesh.calc_loop_triangles()
    triangle_loops = np.empty(len(mesh.loop_triangles) * 3, dtype=np.int32)
    mesh.loop_triangles.foreach_get("loops", triangle_loops)
    loop_vertices = np.empty(len(mesh.loops), dtype=np.int32)
    mesh.loops.foreach_get("vertex_index", loop_vertices)
    vertex_positions = np.empty(len(mesh.vertices) * 3, dtype=np.float32)
    mesh.vertices.foreach_get("co", vertex_positions)
    loop_normals = np.empty(len(mesh.loops) * 3, dtype=np.float32)
    if hasattr(mesh, "corner_normals"):
        mesh.corner_normals.foreach_get("vector", loop_normals)
    else:
        mesh.calc_normals_split()
        mesh.loops.foreach_get("normal", loop_normals)
    loop_uvs = np.zeros(len(mesh.loops) * 2, dtype=np.float32)
    uv_layer = mesh.uv_layers.active
    if uv_layer is not None:
        uv_layer.data.foreach_get("uv", loop_uvs)
    return MeshArrays(
        triangle_loops,
        loop_vertices,
        vertex_positions.reshape(-1, 3),
        loop_normals.reshape(-1, 3),
        loop_uvs.reshape(-1, 2),
    )


# Reads an object's evaluated mesh (modifiers applied, the current Edit mode
# state included) into arrays. With the object's realize switch on, the
# geometry a Geometry Nodes modifier places as instances is merged in too:
# instances are not part of the evaluated mesh, so a scatter would otherwise
# reach the editor as nothing.
def read_object_arrays(scene_object, depsgraph):
    evaluated_object = scene_object.evaluated_get(depsgraph)
    mesh = evaluated_object.to_mesh()
    try:
        own_arrays = read_mesh_arrays(mesh)
    finally:
        evaluated_object.to_mesh_clear()
    if not scene_object.get(REALIZE_INSTANCES_PROPERTY, True):
        return own_arrays
    instance_arrays = read_instance_arrays(scene_object, depsgraph)
    if not instance_arrays:
        return own_arrays
    return merge_mesh_arrays([own_arrays] + instance_arrays)


# Reads every instance the object places, moved into the object's own space.
# The object's transform is not part of the mesh it sends, so each instance's
# world matrix is taken back through it; one mesh read serves every instance
# of the same source, which is what a scatter of one object repeats.
def read_instance_arrays(scene_object, depsgraph):
    host_inverse = np.array(scene_object.matrix_world.inverted(), dtype=np.float64)
    arrays_by_mesh = {}
    collected = []
    for instance in depsgraph.object_instances:
        if not instance.is_instance:
            continue
        parent = instance.parent
        if parent is None or parent.original.name != scene_object.name:
            continue
        instance_object = instance.object
        if instance_object is None or instance_object.type != "MESH":
            continue
        mesh_key = instance_object.data.as_pointer()
        if mesh_key not in arrays_by_mesh:
            arrays_by_mesh[mesh_key] = read_mesh_arrays(instance_object.data)
        placement = host_inverse @ np.array(instance.matrix_world, dtype=np.float64)
        collected.append(move_arrays(arrays_by_mesh[mesh_key], placement))
    return collected


# Moves one instance's arrays into the host's space: positions through the
# matrix, normals through its inverse transpose, then back to unit length.
def move_arrays(mesh_arrays, placement):
    linear = placement[:3, :3]
    normal_matrix = np.linalg.inv(linear).T
    positions = mesh_arrays.vertex_positions.astype(np.float64) @ linear.T + placement[:3, 3]
    normals = mesh_arrays.loop_normals.astype(np.float64) @ normal_matrix.T
    lengths = np.linalg.norm(normals, axis=1, keepdims=True)
    lengths[lengths == 0.0] = 1.0
    return MeshArrays(
        mesh_arrays.triangle_loops,
        mesh_arrays.loop_vertices,
        positions.astype(np.float32),
        (normals / lengths).astype(np.float32),
        mesh_arrays.loop_uvs,
    )


# Joins exports into one, offsetting each part's corner and vertex numbers so
# they stay their own. An empty part (a scatter on an object with no mesh of
# its own) contributes nothing.
def merge_mesh_arrays(parts):
    parts = [part for part in parts if len(part.loop_vertices) > 0]
    if not parts:
        return MeshArrays(
            np.zeros(0, dtype=np.int32),
            np.zeros(0, dtype=np.int32),
            np.zeros((0, 3), dtype=np.float32),
            np.zeros((0, 3), dtype=np.float32),
            np.zeros((0, 2), dtype=np.float32),
        )
    if len(parts) == 1:
        return parts[0]
    triangle_loops = []
    loop_vertices = []
    vertex_positions = []
    loop_normals = []
    loop_uvs = []
    loop_offset = 0
    vertex_offset = 0
    for part in parts:
        triangle_loops.append(part.triangle_loops + loop_offset)
        loop_vertices.append(part.loop_vertices + vertex_offset)
        vertex_positions.append(part.vertex_positions)
        loop_normals.append(part.loop_normals)
        loop_uvs.append(part.loop_uvs)
        loop_offset += len(part.loop_vertices)
        vertex_offset += len(part.vertex_positions)
    return MeshArrays(
        np.concatenate(triangle_loops),
        np.concatenate(loop_vertices),
        np.concatenate(vertex_positions),
        np.concatenate(loop_normals),
        np.concatenate(loop_uvs),
    )


# Converts Blender vectors (Z up) to Pill's (Y up), one row per vector.
def vectors_to_pill(vectors):
    return np.column_stack((vectors[:, 0], vectors[:, 2], -vectors[:, 1]))


# Encodes rows of floats as little-endian `f32` bytes.
def float_bytes(rows):
    return np.ascontiguousarray(rows, dtype="<f4").tobytes()


# Exports the whole mesh in Pill's conventions. Corners that share a vertex, a
# UV and a normal (to 1e-6 and 1e-5) become one Pill vertex; `None` when the
# mesh has no triangles.
def encode_full(mesh_arrays):
    triangle_loops = mesh_arrays.triangle_loops
    if len(triangle_loops) == 0:
        return None
    # Step 1: one key row per corner; equal rows are one Pill vertex. Adding
    # 0.0 turns -0.0 into 0.0, so the two do not split a vertex.
    corner_keys = np.column_stack(
        (
            mesh_arrays.loop_vertices[triangle_loops].astype(np.float64),
            np.round(mesh_arrays.loop_uvs[triangle_loops].astype(np.float64), 6),
            np.round(mesh_arrays.loop_normals[triangle_loops].astype(np.float64), 5),
        )
    ) + 0.0
    _, first_corners, corner_to_vertex = np.unique(corner_keys, axis=0, return_index=True, return_inverse=True)
    corner_to_vertex = corner_to_vertex.reshape(-1)
    # Step 2: each Pill vertex takes its values from its first corner.
    representative_loops = triangle_loops[first_corners]
    positions = vectors_to_pill(mesh_arrays.vertex_positions[mesh_arrays.loop_vertices[representative_loops]])
    representative_uvs = mesh_arrays.loop_uvs[representative_loops]
    texture_coordinates = np.column_stack((representative_uvs[:, 0], 1.0 - representative_uvs[:, 1]))
    normals = vectors_to_pill(mesh_arrays.loop_normals[representative_loops])
    positions_bytes = float_bytes(positions)
    normals_bytes = float_bytes(normals)
    body = (
        positions_bytes
        + float_bytes(texture_coordinates)
        + normals_bytes
        + np.ascontiguousarray(corner_to_vertex, dtype="<u4").tobytes()
    )
    return FullEncoding(body, positions_bytes + normals_bytes, representative_loops, corner_to_vertex, len(corner_to_vertex))


# Exports only positions and normals, in the vertex order of `stream_cache`'s
# full update. `None` when that order no longer fits: the triangulation (a
# quad can flip its diagonal mid-drag), the corner to vertex mapping or the
# UVs changed, or corners merged into one vertex now disagree on the normal.
def encode_positions(mesh_arrays, stream_cache):
    if not (
        np.array_equal(mesh_arrays.triangle_loops, stream_cache.triangle_loops)
        and np.array_equal(mesh_arrays.loop_vertices, stream_cache.loop_vertices)
        and np.array_equal(mesh_arrays.loop_uvs, stream_cache.loop_uvs)
    ):
        return None
    representative_loops = stream_cache.representative_loops
    representative_normals = mesh_arrays.loop_normals[representative_loops]
    corner_normals = mesh_arrays.loop_normals[mesh_arrays.triangle_loops]
    deviation = np.abs(corner_normals - representative_normals[stream_cache.corner_to_vertex])
    if deviation.size and deviation.max() > MERGED_NORMAL_TOLERANCE:
        return None
    positions = vectors_to_pill(mesh_arrays.vertex_positions[mesh_arrays.loop_vertices[representative_loops]])
    return float_bytes(positions) + float_bytes(vectors_to_pill(representative_normals))


# Sends the current geometry of every linked object changed since the last
# tick: positions and normals when the topology is the one the editor holds,
# the whole mesh otherwise. A state equal to what the editor has is not sent,
# and an object with no triangles is skipped: the editor refuses it, and the
# person is most likely in the middle of an edit.
def stream_dirty_objects():
    if not link_state.dirty_object_names or link_state.session is None:
        return
    depsgraph = bpy.context.evaluated_depsgraph_get()
    dirty_names = link_state.dirty_object_names
    link_state.dirty_object_names = set()
    for object_name in dirty_names:
        scene_object = bpy.data.objects.get(object_name)
        if scene_object is None or scene_object.type != "MESH":
            continue
        if scene_object.get(SESSION_PROPERTY) != link_state.session:
            continue
        mesh_arrays = read_object_arrays(scene_object, depsgraph)
        stream_cache = link_state.stream_caches.get(object_name)
        # Step 1: a drag that kept the topology sends positions and normals.
        if stream_cache is not None and stream_cache.sent:
            positions_and_normals = encode_positions(mesh_arrays, stream_cache)
            if positions_and_normals is not None:
                if positions_and_normals != stream_cache.positions_and_normals:
                    stream_cache.positions_and_normals = positions_and_normals
                    send_frame(
                        {
                            "type": "mesh_positions",
                            "session": link_state.session,
                            "link_id": scene_object[LINK_ID_PROPERTY],
                            "topology_id": stream_cache.topology_id,
                            "vertex_count": len(stream_cache.representative_loops),
                        },
                        positions_and_normals,
                    )
                continue
        # Step 2: anything else sends the whole mesh, unless the editor
        # already holds exactly this.
        encoding = encode_full(mesh_arrays)
        if encoding is None:
            continue
        if (
            stream_cache is not None
            and stream_cache.full_body == encoding.body
            and stream_cache.positions_and_normals == encoding.positions_and_normals
        ):
            continue
        topology_id = link_state.next_topology_id
        link_state.next_topology_id += 1
        link_state.stream_caches[object_name] = StreamCache(mesh_arrays, encoding, topology_id, sent=True)
        send_frame(
            {
                "type": "mesh_update",
                "session": link_state.session,
                "link_id": scene_object[LINK_ID_PROPERTY],
                "topology_id": topology_id,
                "vertex_count": encoding.vertex_count,
                "index_count": encoding.index_count,
            },
            encoding.body,
        )


# Notes every linked object whose geometry changed. The send happens on the
# timer, so a drag that changes the mesh every redraw is sent at most once per
# tick.
@persistent
def on_depsgraph_update(scene, depsgraph):
    if link_state.connection is None:
        return
    for update in depsgraph.updates:
        if not update.is_updated_geometry:
            continue
        changed = update.id.original
        if isinstance(changed, bpy.types.Object):
            if LINK_ID_PROPERTY in changed:
                link_state.dirty_object_names.add(changed.name)
        elif isinstance(changed, bpy.types.Mesh):
            for scene_object in bpy.data.objects:
                if scene_object.data == changed and LINK_ID_PROPERTY in scene_object:
                    link_state.dirty_object_names.add(scene_object.name)


# The link's heartbeat: connects when needed, handles what the editor sent and
# streams what changed.
def link_tick():
    check_connection_status()
    if not link_state.enabled:
        return TICK_INTERVAL_SECONDS
    if link_state.connection is None:
        now = time.monotonic()
        if now - link_state.last_connect_attempt >= RECONNECT_INTERVAL_SECONDS and now >= link_state.retry_after:
            try_connect()
        return TICK_INTERVAL_SECONDS
    for header, body in receive_frames():
        message_type = header.get("type")
        try:
            if message_type == "welcome":
                link_state.session = header.get("session")
                # The editor names itself in the greeting; the sidebar shows it
                # as the connection's status line. The same greeting carries the
                # project's res folder, which the two file pickers start in.
                link_state.editor_title = header.get("editor_title")
                link_state.project_res_directory = header.get("res_directory") or None
                link_state.status = ""
                redraw_sidebars()
            elif message_type == "busy":
                # Another Blender holds the editor's link; this one waits and
                # takes over once that one closes.
                disconnect("another Blender is linked to the editor")
                link_state.status = "Waiting: another Blender is linked to the editor"
                link_state.retry_after = time.monotonic() + BUSY_RETRY_SECONDS
                return TICK_INTERVAL_SECONDS
            elif message_type == "open_mesh":
                open_mesh(header, body)
            elif message_type == "replace_mesh" and header.get("session") == link_state.session:
                replace_mesh(header, body)
            elif message_type == "resync" and header.get("session") == link_state.session:
                resync(header)
            elif message_type == "material" and header.get("session") == link_state.session:
                apply_material(header, body)
            elif message_type == "material_values" and header.get("session") == link_state.session:
                apply_material_values(header, body)
            elif message_type == "material_images" and header.get("session") == link_state.session:
                apply_material_images(header)
            elif message_type == "create_result" and header.get("session") == link_state.session:
                handle_create_result(header)
            elif message_type == "connect_result" and header.get("session") == link_state.session:
                handle_connect_result(header)
        except Exception as handling_error:
            print(f"[pill_blender_link] could not handle '{message_type}': {handling_error}")
    try:
        stream_dirty_objects()
    except Exception as streaming_error:
        print(f"[pill_blender_link] could not stream an edit: {streaming_error}")
    return TICK_INTERVAL_SECONDS


# =============================================================================
# User interface
# =============================================================================

class PILL_OT_link_connect(bpy.types.Operator):
    """Connect to the Pill editor now, or reconnect"""

    bl_idname = "pill.link_connect"
    bl_label = "Connect to Pill"

    def execute(self, context):
        link_state.port = context.window_manager.pill_link_port
        link_state.enabled = True
        disconnect("reconnecting")
        try_connect()
        return {"FINISHED"}


class PILL_OT_link_disconnect(bpy.types.Operator):
    """Disconnect from the Pill editor and stop retrying"""

    bl_idname = "pill.link_disconnect"
    bl_label = "Disconnect"

    def execute(self, context):
        link_state.enabled = False
        disconnect("disconnected by you")
        return {"FINISHED"}


# The two file pickers of the Pill block. Both open Blender's file browser in
# the project's `res` folder and offer `.obj` files only; the editor checks the
# picked path again, because a stale folder could still name a file elsewhere.
class PILL_OT_link_pick(bpy.types.Operator):
    """Link this object to a Pill mesh the project already holds"""

    bl_idname = "pill.link_pick"
    bl_label = "Link to existing Pill mesh"

    filepath: bpy.props.StringProperty(subtype="FILE_PATH")
    directory: bpy.props.StringProperty(subtype="DIR_PATH", options={"HIDDEN"})
    filter_glob: bpy.props.StringProperty(default="*.obj", options={"HIDDEN"})

    @classmethod
    def poll(cls, context):
        active = context.active_object
        return (
            link_state.connection is not None
            and active is not None
            and active.type == "MESH"
            and LINK_ID_PROPERTY not in active
        )

    def invoke(self, context, event):
        self.directory = link_state.project_res_directory or ""
        context.window_manager.fileselect_add(self)
        return {"RUNNING_MODAL"}

    def execute(self, context):
        problem = request_connect(context.active_object, self.filepath)
        if problem is not None:
            link_state.connect_problem = problem
            self.report({"WARNING"}, problem)
            redraw_sidebars()
        return {"FINISHED"}


class PILL_OT_link_create(bpy.types.Operator):
    """Write this object as a new Pill mesh under the project's res"""

    bl_idname = "pill.link_create"
    bl_label = "Create new Pill mesh"

    filepath: bpy.props.StringProperty(subtype="FILE_PATH")
    directory: bpy.props.StringProperty(subtype="DIR_PATH", options={"HIDDEN"})
    filter_glob: bpy.props.StringProperty(default="*.obj", options={"HIDDEN"})
    # The dialog asks before an existing file is taken, and the editor then
    # refuses it: a create never replaces a file.
    check_existing: bpy.props.BoolProperty(default=True, options={"HIDDEN"})
    filename_ext = ".obj"

    @classmethod
    def poll(cls, context):
        active = context.active_object
        return (
            link_state.connection is not None
            and active is not None
            and active.type == "MESH"
            and LINK_ID_PROPERTY not in active
        )

    def invoke(self, context, event):
        self.directory = link_state.project_res_directory or ""
        # A name is required, so the object's own name is the first offer.
        if not self.filepath:
            self.filepath = f"{context.active_object.name}.obj"
        context.window_manager.fileselect_add(self)
        return {"RUNNING_MODAL"}

    def execute(self, context):
        problem = request_create(context.active_object, self.filepath)
        if problem is not None:
            link_state.connect_problem = problem
            self.report({"WARNING"}, problem)
            redraw_sidebars()
        return {"FINISHED"}


# The Connection block: the link's state, the port, and one button that
# connects or disconnects, depending on that state.
def draw_connection_block(layout, context):
    connected = link_state.connection is not None
    layout.label(
        text=connection_status_text(),
        icon="CHECKMARK" if connected else "UNLINKED",
    )
    detail = connection_detail_text()
    if detail:
        layout.label(text=detail, icon="INFO")
    layout.prop(context.window_manager, "pill_link_port", text="Port")
    layout.operator(
        (PILL_OT_link_disconnect if connected else PILL_OT_link_connect).bl_idname,
        text="Disconnect" if connected else "Connect",
        icon="UNLINKED" if connected else "LINKED",
    )


class PILL_OT_link_detach(bpy.types.Operator):
    """Stop streaming this object into Pill"""

    bl_idname = "pill.link_detach"
    bl_label = "Disconnect from Pill mesh"

    @classmethod
    def poll(cls, context):
        active = context.active_object
        return active is not None and LINK_ID_PROPERTY in active

    def execute(self, context):
        scene_object = context.active_object
        # The editor keeps the link for the rest of its run; the object simply
        # stops sending to it.
        for property_name in (LINK_ID_PROPERTY, SESSION_PROPERTY, ASSET_NAME_PROPERTY):
            if property_name in scene_object:
                del scene_object[property_name]
        link_state.stream_caches.pop(scene_object.name, None)
        link_state.dirty_object_names.discard(scene_object.name)
        link_state.connect_problem = ""
        redraw_sidebars()
        return {"FINISHED"}


# Draws the Pill block of the Properties editor: what the object is linked to,
# the switch that decides what its Geometry Nodes result exports, and the
# buttons that change the link. The whole block is disabled while Blender is
# not connected, because nothing in it can reach the editor; the only other
# text it adds is why a request was refused.
def draw_mesh_link_block(layout, context):
    scene_object = context.active_object
    layout.enabled = link_state.connection is not None
    linked_asset = scene_object.get(ASSET_NAME_PROPERTY) or "None"
    layout.label(text=f"Linked Pill mesh: {linked_asset}", icon="INFO")
    if has_geometry_nodes_modifier(scene_object):
        layout.prop(scene_object, REALIZE_INSTANCES_PROPERTY, text="Realize geometry nodes instances")
    if scene_object.get(LINK_ID_PROPERTY) is not None:
        layout.operator(PILL_OT_link_detach.bl_idname, icon="UNLINKED")
    else:
        layout.label(text="Not linked to any Pill mesh", icon="UNLINKED")
        layout.operator(PILL_OT_link_pick.bl_idname, icon="LINKED")
        layout.operator(PILL_OT_link_create.bl_idname, icon="ADD")
    if link_state.connect_problem:
        layout.label(text=link_state.connect_problem, icon="ERROR")


# Whether the object carries a Geometry Nodes modifier, which is the only case
# in which the realize switch can change anything.
def has_geometry_nodes_modifier(scene_object):
    return any(
        modifier.type == "NODES" and modifier.node_group is not None
        for modifier in scene_object.modifiers
    )


# One block per panel: Blender draws every panel in a category as its own
# block, separated from the next by a gap - the way the Tool tab stacks Active
# Tool, Options and Workspace. A block drawn instead with `UILayout.panel` is
# the inner kind (Options' own "Transform"), which sits inside its parent's
# block with no gap, so the two live in separate panels here.
class PILL_PT_connection_panel(bpy.types.Panel):
    """The link's state, its port, and the button that connects or disconnects"""

    bl_label = "Connection"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "Pill"
    bl_order = 0

    def draw(self, context):
        draw_connection_block(self.layout, context)


class PILL_PT_mesh_link_panel(bpy.types.Panel):
    """Link this object's mesh to an asset the project already holds"""

    bl_label = "Pill"
    bl_space_type = "PROPERTIES"
    bl_region_type = "WINDOW"
    bl_context = "object"
    bl_options = {"DEFAULT_CLOSED"}

    @classmethod
    def poll(cls, context):
        active = context.active_object
        return active is not None and active.type == "MESH"

    def draw(self, context):
        draw_mesh_link_block(self.layout, context)


classes = (
    PILL_OT_link_connect,
    PILL_OT_link_disconnect,
    PILL_OT_link_pick,
    PILL_OT_link_create,
    PILL_OT_link_detach,
    PILL_PT_connection_panel,
    PILL_PT_mesh_link_panel,
)


# Returns the port named by PILL_BLENDER_LINK_PORT, when it names one.
def port_from_environment():
    port_text = os.environ.get(PORT_ENVIRONMENT_VARIABLE, "").strip()
    return int(port_text) if port_text.isdigit() else None


def register():
    # One line that says which copy of the script this Blender runs. The
    # installed add-on is a loader that reads this file when Blender enables
    # it, so an edit here shows up at the next start or re-enable, never in a
    # session that is already running.
    version_text = ".".join(str(part) for part in bl_info["version"])
    print(f"[pill_blender_link] v{version_text} loaded from {os.path.abspath(__file__)}")
    environment_port = port_from_environment()
    if environment_port is not None:
        link_state.port = environment_port
    for registered_class in classes:
        bpy.utils.register_class(registered_class)
    bpy.types.WindowManager.pill_link_port = bpy.props.IntProperty(
        name="Pill editor port", default=link_state.port, min=1, max=65535
    )
    # On by default: a Geometry Nodes result that scatters is what the person
    # sees in the viewport, so it is what the editor is sent.
    bpy.types.Object.pill_realize_instances = bpy.props.BoolProperty(
        name="Realize geometry nodes instances",
        description="Send the geometry the object places as instances (a scatter) as one mesh",
        default=True,
    )
    if on_depsgraph_update not in bpy.app.handlers.depsgraph_update_post:
        bpy.app.handlers.depsgraph_update_post.append(on_depsgraph_update)
    if not bpy.app.timers.is_registered(link_tick):
        bpy.app.timers.register(link_tick, first_interval=0.1, persistent=True)


def unregister():
    if bpy.app.timers.is_registered(link_tick):
        bpy.app.timers.unregister(link_tick)
    if on_depsgraph_update in bpy.app.handlers.depsgraph_update_post:
        bpy.app.handlers.depsgraph_update_post.remove(on_depsgraph_update)
    disconnect("add-on disabled")
    del bpy.types.WindowManager.pill_link_port
    del bpy.types.Object.pill_realize_instances
    for registered_class in reversed(classes):
        bpy.utils.unregister_class(registered_class)


# Reads `--pill-link-port <port>` from the arguments after Blender's `--`.
def port_from_command_line():
    if "--" not in sys.argv:
        return None
    script_arguments = sys.argv[sys.argv.index("--") + 1 :]
    if "--pill-link-port" in script_arguments:
        port_position = script_arguments.index("--pill-link-port") + 1
        if port_position < len(script_arguments):
            return int(script_arguments[port_position])
    return None


# Run with `--python`: the editor started this Blender, so connect at once.
if __name__ == "__main__":
    command_line_port = port_from_command_line()
    if command_line_port is not None:
        link_state.port = command_line_port
    # When the add-on is enabled too, its copy already runs the link; a second
    # copy would take the editor's one connection away from it, back and forth.
    addon_enabled = any(
        addon_name.split(".")[-1] == "pill_blender_link" for addon_name in bpy.context.preferences.addons.keys()
    )
    if addon_enabled:
        # The add-on's copy takes the editor's port and connects now.
        addon_module = next(
            (module for name, module in sys.modules.items() if name.split(".")[-1] == "pill_blender_link" and hasattr(module, "link_state")),
            None,
        )
        if addon_module is not None and command_line_port is not None:
            addon_module.link_state.port = command_line_port
            addon_module.link_state.last_connect_attempt = 0.0
            addon_module.link_state.retry_after = 0.0
        print("[pill_blender_link] the add-on is enabled; it runs the link")
    else:
        register()
