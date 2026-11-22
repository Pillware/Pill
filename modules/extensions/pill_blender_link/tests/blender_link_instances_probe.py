# REQUIREMENTS
#   Blender 4.x (this script is run BY Blender, not by Python). Uses only the
#   standard library plus bpy and numpy, which Blender ships.
#
# DESCRIPTION
#   Headless check of the link's Geometry Nodes export: a grid of instances of
#   a plane, exported by the add-on's own functions. Asserts that
#     - with the object's realize switch on, the export carries every instance
#       (18 triangles for a 3x3 grid of quads) and encodes a full mesh,
#     - the merged geometry is where the viewport draws it (the bounds match a
#       Realize Instances node's own evaluation),
#     - with the switch off nothing is sent for instances only, which is what
#       the link did before the merge existed,
#     - an edit to the instanced source mesh changes the export.
#   Prints one line per check and exits non-zero on the first failure.
#
# USAGE
#   "C:\Program Files\Blender Foundation\Blender 4.5\blender.exe" -b ^
#       --factory-startup --python-exit-code 1 ^
#       --python modules/extensions/pill_blender_link/tests/blender_link_instances_probe.py
#
# EXAMPLE USAGE
#   See the command above; every check prints "[probe] ok ..." on success.
#
# --- SCRIPT ---
import importlib.util
import os

import bpy
import numpy as np

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
    spec = importlib.util.spec_from_file_location("pill_blender_link_instances_probe", ADDON_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# A grid of points, each carrying one instance of `source_object`; with
# `realize`, a Realize Instances node after them makes one mesh of it.
def build_instance_group(source_object, realize):
    group = bpy.data.node_groups.new("ProbeInstances", "GeometryNodeTree")
    group.interface.new_socket(name="Geometry", in_out="INPUT", socket_type="NodeSocketGeometry")
    group.interface.new_socket(name="Geometry", in_out="OUTPUT", socket_type="NodeSocketGeometry")
    group_output = group.nodes.new("NodeGroupOutput")
    grid = group.nodes.new("GeometryNodeMeshGrid")
    grid.inputs["Vertices X"].default_value = 3
    grid.inputs["Vertices Y"].default_value = 3
    object_info = group.nodes.new("GeometryNodeObjectInfo")
    object_info.inputs["Object"].default_value = source_object
    object_info.inputs["As Instance"].default_value = True
    instance_on_points = group.nodes.new("GeometryNodeInstanceOnPoints")
    group.links.new(grid.outputs["Mesh"], instance_on_points.inputs["Points"])
    group.links.new(object_info.outputs["Geometry"], instance_on_points.inputs["Instance"])
    last_socket = instance_on_points.outputs["Instances"]
    if realize:
        realize_node = group.nodes.new("GeometryNodeRealizeInstances")
        group.links.new(last_socket, realize_node.inputs["Geometry"])
        last_socket = realize_node.outputs["Geometry"]
    group.links.new(last_socket, group_output.inputs["Geometry"])
    return group


# The triangle count `encode_full` finds for what the object exports.
def exported_triangle_count(addon, scene_object, depsgraph):
    encoding = addon.encode_full(addon.read_object_arrays(scene_object, depsgraph))
    return 0 if encoding is None else encoding.index_count // 3


def exported_bounds(addon, scene_object, depsgraph):
    arrays = addon.read_object_arrays(scene_object, depsgraph)
    if len(arrays.vertex_positions) == 0:
        return None
    converted = addon.vectors_to_pill(arrays.vertex_positions.astype(np.float64))
    return converted.min(axis=0), converted.max(axis=0)


def main():
    addon = load_addon()
    addon.register()

    # A plane that is only ever instanced, and a host that carries the node
    # tree. The host is moved and scaled, which the export must not bake: the
    # mesh travels in the object's own space.
    bpy.ops.mesh.primitive_plane_add(size=1.0)
    source_object = bpy.context.active_object
    source_object.name = "ProbeInstanceSource"
    bpy.ops.mesh.primitive_cube_add()
    host_object = bpy.context.active_object
    host_object.name = "ProbeInstanceHost"
    host_object.location = (3.0, -2.0, 1.0)
    host_object.scale = (2.0, 2.0, 2.0)
    host_object.modifiers.new(name="Instances", type="NODES").node_group = build_instance_group(
        source_object, realize=False
    )
    bpy.context.view_layer.update()
    depsgraph = bpy.context.evaluated_depsgraph_get()

    # Step 1: the switch is on by default, so the scatter is exported.
    check(
        addon.has_geometry_nodes_modifier(host_object),
        "the object is seen as carrying a Geometry Nodes modifier",
    )
    check(
        host_object.get(addon.REALIZE_INSTANCES_PROPERTY, True) is True,
        "the realize switch is on by default",
    )
    triangles = exported_triangle_count(addon, host_object, depsgraph)
    check(
        triangles == 18,
        f"a 3x3 grid of quads exports 18 triangles ({triangles})",
    )

    # Step 2: the merged geometry is where the viewport draws it.
    merged_bounds = exported_bounds(addon, host_object, depsgraph)
    host_object.modifiers["Instances"].node_group = build_instance_group(source_object, realize=True)
    bpy.context.view_layer.update()
    depsgraph = bpy.context.evaluated_depsgraph_get()
    realized_bounds = exported_bounds(addon, host_object, depsgraph)
    check(
        merged_bounds is not None and realized_bounds is not None,
        "both exports hold vertices",
    )
    check(
        merged_bounds is not None
        and realized_bounds is not None
        and np.allclose(merged_bounds[0], realized_bounds[0], atol=1.0e-4)
        and np.allclose(merged_bounds[1], realized_bounds[1], atol=1.0e-4),
        "the merged instances sit where a Realize Instances node puts them",
    )

    # Step 3: with the switch off, instances are not exported - the state the
    # link was in before the merge existed.
    host_object.modifiers["Instances"].node_group = build_instance_group(source_object, realize=False)
    host_object[addon.REALIZE_INSTANCES_PROPERTY] = False
    bpy.context.view_layer.update()
    depsgraph = bpy.context.evaluated_depsgraph_get()
    triangles = exported_triangle_count(addon, host_object, depsgraph)
    check(triangles == 0, f"with the switch off nothing is exported ({triangles} triangles)")
    host_object[addon.REALIZE_INSTANCES_PROPERTY] = True

    # Step 4: an edit to the instanced source reaches the export.
    before = exported_bounds(addon, host_object, depsgraph)
    source_object.data.vertices[0].co[2] = 1.5
    source_object.data.update()
    bpy.context.view_layer.update()
    depsgraph = bpy.context.evaluated_depsgraph_get()
    after = exported_bounds(addon, host_object, depsgraph)
    check(
        before is not None and after is not None and not np.allclose(before[1], after[1]),
        "editing the instanced mesh moves what is exported",
    )

    addon.unregister()
    print(f"[probe] {len(CHECKS)} checks passed")


main()
