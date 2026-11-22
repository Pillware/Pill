# REQUIREMENTS
#   Blender 4.x (this script is run BY Blender, not by Python). Uses only the
#   standard library plus bpy, which Blender ships.
#
# DESCRIPTION
#   Headless check of the Pill live link's material half: it imports the
#   add-on script from `..\blender_addon\pill_blender_link.py`, builds the
#   node graph the editor sends
#   for the shipped PBR shader, and asserts the tree, the values-only path,
#   the images-only path and the rebuild rule. Prints one line per check and
#   exits non-zero on the first failure.
#
# USAGE
#   "C:\Program Files\Blender Foundation\Blender 4.5\blender.exe" -b ^
#       --factory-startup --python-exit-code 1 ^
#       --python modules/extensions/pill_blender_link/tests/blender_material_probe.py
#
# EXAMPLE USAGE
#   See the command above; every check prints "[probe] ok ..." on success.
#
# --- SCRIPT ---
import importlib.util
import json
import os
import sys
import tempfile

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
    spec = importlib.util.spec_from_file_location("pill_blender_link_probe", ADDON_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_png(path, size, colour):
    image = bpy.data.images.new("probe_source", size, size, alpha=False)
    image.pixels = list(colour) * (size * size)
    image.filepath_raw = path
    image.file_format = "PNG"
    image.save()
    bpy.data.images.remove(image)


def make_linked_object(link_id, session):
    mesh = bpy.data.meshes.new("ProbeMesh")
    mesh.from_pydata([(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)], [], [(0, 1, 2)])
    scene_object = bpy.data.objects.new("ProbeMesh", mesh)
    scene_object["pill_link_id"] = link_id
    scene_object["pill_link_session"] = session
    bpy.context.scene.collection.objects.link(scene_object)
    return scene_object


def graph_body(texture_paths):
    """The graph translate_material emits for the shipped PBR fragment."""

    def image(slot):
        return {"slot": slot, "absolute_path": texture_paths[slot], "color_space": "NonColor" if slot == "normal" else "Srgb"}

    nodes = [
        {"Image": image("base_color")},
        {"Rgb": {"name": "pbr_base", "value": [1.0, 1.0, 1.0]}},
        {"Multiply": {"first": {"Node": 0}, "second": {"Node": 1}}},
        {"Image": image("metallic_roughness")},
        {"Component": {"input": {"Node": 3}, "channel": "Green"}},
        {"Value": {"name": "pbr_roughness", "value": 0.5}},
        {"Multiply": {"first": {"Node": 4}, "second": {"Node": 5}}},
        {"Saturate": {"input": {"Node": 6}}},
        {"Component": {"input": {"Node": 3}, "channel": "Blue"}},
        {"Value": {"name": "pbr_metallic", "value": 0.0}},
        {"Multiply": {"first": {"Node": 8}, "second": {"Node": 9}}},
        {"Saturate": {"input": {"Node": 10}}},
        {"Image": image("normal")},
        {"NormalMap": {"input": {"Node": 12}}},
        {"Image": image("emissive")},
        {"Rgb": {"name": "pbr_emissive", "value": [0.0, 0.0, 0.0]}},
        {"Multiply": {"first": {"Node": 14}, "second": {"Node": 15}}},
    ]
    graph = {
        "principled": {
            "base_color": {"Node": 2},
            "roughness": {"Node": 7},
            "metallic": {"Node": 11},
            "normal": {"Node": 13},
            "emission": {"Node": 16},
        },
        "nodes": nodes,
        "refusals": [],
    }
    return json.dumps(graph).encode("utf-8")


def linked_from(node_tree, socket):
    for link in node_tree.links:
        if link.to_socket == socket:
            return link
    return None


def link_out_of(node_tree, socket):
    for link in node_tree.links:
        if link.from_socket == socket:
            return link
    return None


def node_named(material, name):
    return material.node_tree.nodes.get(name)


def principled_of(material):
    for node in material.node_tree.nodes:
        if node.type == "BSDF_PRINCIPLED":
            return node
    raise AssertionError("the material has no Principled BSDF")


def check_graph(addon, material, scene_object, node_tree):
    check(
        scene_object.data.materials[0] == material,
        "the material fills the object's first slot",
    )
    check(material.name == "probe", f"the material is named after its asset ({material.name})")
    principled = principled_of(material)
    for link in node_tree.links:
        print(
            "[probe] link "
            f"{link.from_node.name}.{link.from_socket.name} -> {link.to_node.name}.{link.to_socket.name}"
        )
    print(f"[probe] principled outputs {[socket.name for socket in principled.outputs]}")
    print(f"[probe] principled inputs {[socket.name for socket in principled.inputs]}")
    surface = link_out_of(node_tree, principled.outputs["BSDF"])
    check(surface is not None and surface.to_node.type == "OUTPUT_MATERIAL", "the BSDF feeds the material output")
    strength = principled.inputs.get("Emission Strength")
    check(strength is not None and strength.default_value == 1.0, "emission strength is 1.0")

    base_colour = linked_from(node_tree, principled.inputs["Base Color"])
    check(base_colour is not None, "base colour is connected")
    multiply = base_colour.from_node
    check(multiply.type == "VECT_MATH" and multiply.operation == "MULTIPLY", "base colour is image times parameter")
    sources = [multiply.inputs[0].links[0].from_node, multiply.inputs[1].links[0].from_node]
    check(sources[0].name == "pill_texture_base_color", "base colour samples its image node")
    check(sources[1].name == "pill_param_pbr_base", "base colour multiplies its RGB parameter")

    roughness = linked_from(node_tree, principled.inputs["Roughness"])
    check(roughness is not None, "roughness is connected")
    clamp = roughness.from_node
    check(clamp.type == "CLAMP", "roughness is clamped")
    inner = clamp.inputs["Value"].links[0].from_node
    check(inner.type == "VECT_MATH" and inner.operation == "MULTIPLY", "roughness is channel times parameter")
    channel = inner.inputs[0].links[0].from_node
    check(
        channel.type in ("SEPARATE_COLOR", "SEPRGB") and channel.outputs[1].links[0].to_socket == inner.inputs[0],
        "roughness reads the green channel of the packing map",
    )
    check(
        channel.inputs[0].links[0].from_node.name == "pill_texture_metallic_roughness",
        "the packing map is the metallic-roughness texture",
    )
    check(inner.inputs[1].links[0].from_node.name == "pill_param_pbr_roughness", "roughness multiplies its parameter")

    metallic = linked_from(node_tree, principled.inputs["Metallic"])
    check(metallic is not None and metallic.from_node.type == "CLAMP", "metallic is connected and clamped")
    normal = linked_from(node_tree, principled.inputs["Normal"])
    check(normal is not None and normal.from_node.type == "NORMAL_MAP", "the normal goes through a Normal Map node")
    check(
        normal.from_node.inputs["Color"].links[0].from_node.name == "pill_texture_normal",
        "the normal map samples the normal texture",
    )
    emission_names = [name for name in ("Emission Color", "Emission") if principled.inputs.get(name)]
    emission_link = linked_from(node_tree, principled.inputs[emission_names[0]])
    check(emission_link is not None and emission_link.from_node.type == "VECT_MATH", "emission is connected")

    image_nodes = [node for node in node_tree.nodes if node.type == "TEX_IMAGE"]
    check(len(image_nodes) == 4, f"four image nodes ({len(image_nodes)})")
    for node in image_nodes:
        check(
            node.image is not None and node.image.size[0] > 0,
            f"image node '{node.name}' loaded '{os.path.basename(node.image.filepath) if node.image else ''}'",
        )
    check(
        node_named(material, "pill_texture_normal").image.colorspace_settings.name == "Non-Color",
        "the normal map is read as Non-Color",
    )
    check(
        node_named(material, "pill_texture_base_color").image.colorspace_settings.name == "sRGB",
        "the colour map is read as sRGB",
    )


def main():
    addon = load_addon()
    session = "probe-session"
    link_id = 7
    addon.link_state.session = session
    scene_object = make_linked_object(link_id, session)

    texture_directory = tempfile.mkdtemp(prefix="pill_probe_textures_")
    texture_paths = {}
    for slot in ("base_color", "metallic_roughness", "normal", "emissive"):
        path = os.path.join(texture_directory, f"{slot}.png").replace("\\", "/")
        write_png(path, 2, (1.0, 1.0, 1.0, 1.0))
        texture_paths[slot] = path

    header = {"link_id": link_id, "name": "materials/probe.material", "node_count": 17}
    addon.apply_material(header, graph_body(texture_paths))
    material = bpy.data.materials.get("probe")
    check(material is not None, "the add-on made the material")
    node_tree = material.node_tree
    check_graph(addon, material, scene_object, node_tree)

    # Values only: the tree must not be rebuilt.
    names_before = sorted(node.name for node in node_tree.nodes)
    images_before = [node.image for node in node_tree.nodes if node.type == "TEX_IMAGE"]
    values_body = json.dumps(
        {"pbr_roughness": {"Scalar": 0.25}, "pbr_base": {"Color": [1.0, 0.0, 0.0]}}
    ).encode("utf-8")
    addon.apply_material_values({"link_id": link_id}, values_body)
    check(
        node_named(material, "pill_param_pbr_roughness").outputs[0].default_value == 0.25,
        "a value send moves the roughness parameter",
    )
    base_parameter = node_named(material, "pill_param_pbr_base").outputs[0].default_value
    check(tuple(base_parameter)[:3] == (1.0, 0.0, 0.0), "a value send moves the base-colour parameter")
    check(
        sorted(node.name for node in node_tree.nodes) == names_before,
        "a value send keeps every node",
    )
    check(
        [node.image for node in node_tree.nodes if node.type == "TEX_IMAGE"] == images_before,
        "a value send keeps every image",
    )

    # Images only: a repainted file reloads and the tree stays.
    write_png(texture_paths["base_color"], 4, (0.0, 1.0, 0.0, 1.0))
    addon.apply_material_images(
        {
            "link_id": link_id,
            "images": [
                {
                    "slot": "base_color",
                    "absolute_path": texture_paths["base_color"],
                    "color_space": "Srgb",
                }
            ],
        }
    )
    check(
        node_named(material, "pill_texture_base_color").image.size[0] == 4,
        "a repainted texture reloads in Blender",
    )
    check(
        sorted(node.name for node in node_tree.nodes) == names_before,
        "an image send keeps every node",
    )

    # A full send rebuilds the tree and discards what was added in Blender.
    added_node = node_tree.nodes.new("ShaderNodeMath")
    added_node.name = "in_blender_edit"
    check(node_tree.nodes.get("in_blender_edit") is not None, "a node was added in Blender")
    addon.apply_material(header, graph_body(texture_paths))
    material = bpy.data.materials.get("probe")
    check(
        material.node_tree.nodes.get("in_blender_edit") is None,
        "a full send discards node edits made in Blender",
    )
    check(
        node_named(material, "pill_param_pbr_roughness").outputs[0].default_value == 0.5,
        "a full send restores the graph's values",
    )
    check_graph(addon, material, scene_object, material.node_tree)

    # The 3.x Principled name falls back to the older one.
    class FakeSocket:
        pass

    class FakePrincipled:
        def __init__(self):
            self.inputs = {"Emission": FakeSocket()}

    check(
        addon.principled_input(FakePrincipled(), "Emission Color", "Emission") is not None,
        "the older Emission input name is found",
    )

    # A fallback-shaped graph (the standard slot names, no surface): the same
    # node kinds without the clamp, and an input the graph skips left alone.
    fallback_nodes = [
        {"Image": {"slot": "base_color", "absolute_path": texture_paths["base_color"], "color_space": "Srgb"}},
        {"Rgb": {"name": "pbr_base", "value": [1.0, 1.0, 1.0]}},
        {"Multiply": {"first": {"Node": 0}, "second": {"Node": 1}}},
        {"Image": {"slot": "metallic_roughness", "absolute_path": texture_paths["metallic_roughness"], "color_space": "Srgb"}},
        {"Component": {"input": {"Node": 3}, "channel": "Green"}},
        {"Value": {"name": "pbr_roughness", "value": 0.5}},
        {"Multiply": {"first": {"Node": 4}, "second": {"Node": 5}}},
        {"Image": {"slot": "normal", "absolute_path": texture_paths["normal"], "color_space": "NonColor"}},
        {"NormalMap": {"input": {"Node": 7}}},
    ]
    fallback_graph = {
        "principled": {
            "base_color": {"Node": 2},
            "roughness": {"Node": 6},
            "metallic": "Unconnected",
            "normal": {"Node": 8},
            "emission": "Unconnected",
        },
        "nodes": fallback_nodes,
        "refusals": [{"field": "detail_map", "reason": "no standard Principled input takes this texture slot"}],
    }
    addon.apply_material(
        {"link_id": link_id, "name": "materials/probe.material"},
        json.dumps(fallback_graph).encode("utf-8"),
    )
    material = bpy.data.materials.get("probe")
    fallback_tree = material.node_tree
    fallback_principled = principled_of(material)
    roughness_link = linked_from(fallback_tree, fallback_principled.inputs["Roughness"])
    check(
        roughness_link is not None
        and roughness_link.from_node.type == "VECT_MATH"
        and roughness_link.from_node.operation == "MULTIPLY",
        "a fallback graph puts the packing channel times its parameter straight on Roughness",
    )
    check(
        not fallback_principled.inputs["Metallic"].links,
        "an unconnected fallback input keeps Blender's default",
    )
    check(
        not fallback_principled.inputs["Emission Color"].links,
        "an unconnected emission keeps Blender's default",
    )
    check(
        linked_from(fallback_tree, fallback_principled.inputs["Normal"]).from_node.type == "NORMAL_MAP",
        "a fallback graph still sends the normal through a Normal Map node",
    )

    print(f"[probe] {len(CHECKS)} checks passed")


main()
sys.exit(0)
