# REQUIREMENTS
#   The editor binary built (`cargo build -p pill_editor` in modules/), and the
#   repository's devops packages importable (the script adds them to sys.path).
#
# DESCRIPTION
#   Runtime check of the Blender link's material path, without a Blender: the
#   script plays Blender's half of the protocol (greets the editor, reads its
#   frames), then asks the editor to open a mesh entity's material. It asserts
#   the editor sends open_mesh and then the material graph, that the graph
#   names the five Principled inputs, and that its nodes use the shipped PBR
#   shader's slots. Prints one line per check and exits non-zero on failure.
#
# USAGE
#   python modules/extensions/pill_blender_link/tests/blender_link_material_smoke.py
#
# EXAMPLE USAGE
#   python modules/extensions/pill_blender_link/tests/blender_link_material_smoke.py
#
# --- SCRIPT ---
import json
import re
import sys
import time
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(REPOSITORY_ROOT / "devops"))
sys.path.insert(0, str(REPOSITORY_ROOT / "devops" / "tests"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from core import editor_log_tokens as tokens
from core import log_layout
from core.editor_session import EditorSession
from core.projects import SCENE_PROJECT_PATH

from blender_link_test_support import FakeBlender, check, entity_rows

LINK_PORT = 47901


def mesh_entity_name(session):
    """The first entity the dump lists with a mesh renderer."""
    for display, components in entity_rows(session):
        if "MeshRendererComponent" in components:
            return display
    raise AssertionError("no entity carries a mesh renderer")


def main():
    session = EditorSession(
        "egui",
        "blender-link-smoke",
        project=SCENE_PROJECT_PATH,
        test_seconds=180,
    )
    session.start(
        extra_env={
            "PILL_EDITOR_CONTINUOUS_REDRAW": "1",
            "PILL_BLENDER_LINK_PORT": str(LINK_PORT),
        }
    )
    fake_blender = None
    try:
        session.wait_for_match(
            re.escape(tokens.EDITOR_WINDOW_CREATED_TOKEN), 120, "editor startup"
        )
        print("[smoke] ok the editor started and is listening")

        fake_blender = FakeBlender(LINK_PORT)
        fake_blender.read_frames(5.0)
        welcome = fake_blender.frame_of_type("welcome")
        check(welcome, "the editor greeted the link with welcome")
        check(welcome[0][0].get("session"), "the welcome names the editor's session")

        entity_name = mesh_entity_name(session)
        print(f"[smoke] the entity with a mesh renderer: {entity_name}")
        # The link is an editor-only module: it registers its button when the
        # module loads, which is its own build after the editor's first frame.
        session.wait_for_match(
            re.escape("[blender_link] pill_blender_link_editor loaded"),
            180,
            "the link module loading",
        )
        print("[smoke] ok the link module registered inside the editor")
        begin = session.length()
        session.send("tab inspector")
        session.send(f"select entity {entity_name}")
        session.wait(re.escape(tokens.SELECTION_CHANGED_TOKEN), 30, "the selection", begin)

        begin = session.length()
        session.send("command pill_blender_link.open_in_blender")
        try:
            session.wait(
                log_layout.message_then_fields_pattern(
                    re.escape("[blender_link] mesh opened in blender"),
                    r'\s+mesh="[^"]+"\s+link_id=\d+',
                ),
                60,
                "the mesh opened in the link",
                begin,
            )
        except TimeoutError:
            # Prove the channel is still reading: a dump after the action.
            print("[smoke] probing whether the scripted channel still reads")
            probe = session.length()
            session.send("dump systems")
            try:
                session.wait(re.escape("[test] systems"), 20, "the probe dump", probe)
                print("[smoke] the channel still reads; the action line was dropped")
            except TimeoutError:
                print("[smoke] the scripted channel stopped reading its file")
            print("[smoke] what the editor logged since the action:")
            for line in session.read_since(begin).splitlines()[-25:]:
                print(f"  {line}")
            raise
        print("[smoke] ok the editor answered the Blender button")

        # The frames arrive on the editor's next flush; read until the graph is
        # there, then look at the order and the payload.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and not fake_blender.frame_of_type("material"):
            fake_blender.read_frames(2.0)
        open_mesh = fake_blender.frame_of_type("open_mesh")
        material = fake_blender.frame_of_type("material")
        check(open_mesh, "the editor sent the mesh with open_mesh")
        check(material, "the editor sent the material graph")
        first_open_index = fake_blender.frames.index(open_mesh[0])
        first_material_index = fake_blender.frames.index(material[0])
        check(
            first_open_index < first_material_index,
            "the mesh arrives before its material",
        )

        header, body = material[0]
        check(header.get("link_id") == open_mesh[0][0].get("link_id"), "the material names the link")
        check(header.get("name"), f"the material names its asset ({header.get('name')})")
        graph = json.loads(body.decode("utf-8"))
        principled = graph.get("principled", {})
        for field in ("base_color", "roughness", "metallic", "normal", "emission"):
            check(principled.get(field) is not None, f"the graph feeds {field}")
        node_kinds = {next(iter(node)) for node in graph.get("nodes", [])}
        check("Image" in node_kinds, f"the graph carries image nodes ({sorted(node_kinds)})")
        check(
            any(kind in node_kinds for kind in ("Value", "Rgb")),
            "the graph carries parameter nodes",
        )
        check(
            not graph.get("refusals"),
            f"the shipped PBR shader refuses no field ({graph.get('refusals')})",
        )
        print(
            f"[smoke] the graph has {len(graph.get('nodes', []))} nodes: {sorted(node_kinds)}"
        )
        print("[smoke] done")
    finally:
        if fake_blender is not None:
            fake_blender.close()
        session.close()


main()
