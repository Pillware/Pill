# REQUIREMENTS
#   The editor binary built (`cargo build -p pill_editor` in modules/), and the
#   repository's devops packages importable (the script adds them to sys.path).
#
# DESCRIPTION
#   Runtime check of the Blender link's create direction, without a Blender:
#   the script plays Blender's half of the protocol, sends `create_mesh` for the
#   file its picker would name, and asserts the editor
#     - logged the request accepted and the mesh written as an asset,
#     - answered `create_result` with ok and a link id,
#     - spawned one new entity that carries a mesh renderer,
#     - lists the written asset in the project's assets,
#     - refuses the same file a second time and spawns nothing more,
#     - refuses a destination outside its `res`, whose folder the greeting
#       named.
#   It deletes the asset it creates (`.obj` and its `.meta`) on the way out.
#   Prints one line per check and exits non-zero on failure.
#
# USAGE
#   python modules/extensions/pill_blender_link/tests/blender_link_create_smoke.py
#
# EXAMPLE USAGE
#   python modules/extensions/pill_blender_link/tests/blender_link_create_smoke.py
#
# --- SCRIPT ---
import re
import struct
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
from core.projects import SCENE_PROJECT_PATH, SCENE_PROJECT_ROOT

from blender_link_test_support import FakeBlender, check, entity_rows

LINK_PORT = 47902
ASSET_NAME = "blender_create_smoke"
ASSET_PATH = f"models/{ASSET_NAME}.obj"
CREATED_FILES = SCENE_PROJECT_ROOT / "res" / "models"

# The editor's own lines about a create; they are the observation point, so the
# module's log text has to match these exactly.
CREATE_ACCEPTED_TOKEN = "[blender_link] create accepted"
CREATED_ASSET_TOKEN = "[blender_link] created mesh asset"
CREATED_ENTITY_TOKEN = "[blender_link] created entity for the new mesh"


def triangle_body():
    """One triangle in the geometry body's layout: positions, UVs, normals, indices."""
    positions = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)]
    texture_coordinates = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]
    normals = [(0.0, 0.0, 1.0), (0.0, 0.0, 1.0), (0.0, 0.0, 1.0)]
    body = bytearray()
    for position in positions:
        body.extend(struct.pack("<3f", *position))
    for texture_coordinate in texture_coordinates:
        body.extend(struct.pack("<2f", *texture_coordinate))
    for normal in normals:
        body.extend(struct.pack("<3f", *normal))
    body.extend(struct.pack("<3I", 0, 1, 2))
    return bytes(body)


def wait_for_frames(fake_blender, message_type, expected_count, timeout_seconds):
    """Reads until at least `expected_count` frames of the type have arrived."""
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        frames = fake_blender.frame_of_type(message_type)
        if len(frames) >= expected_count:
            return frames
        fake_blender.read_frames(1.0)
    return fake_blender.frame_of_type(message_type)


def request_create(fake_blender, session_id, picked_path):
    """Sends one create_mesh for the triangle, as Blender's picker does."""
    body = triangle_body()
    fake_blender.send(
        {
            "type": "create_mesh",
            "session": session_id,
            "path": picked_path,
            "vertex_count": 3,
            "index_count": 3,
        },
        body,
    )


def wait_for_asset_row(session, pattern, timeout_seconds):
    """Asks for the asset listing until `pattern` appears in one of them.

    The listing is refreshed on a cadence, so the first dump after a write can
    still be the one from before it.
    """
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        begin = session.length()
        session.send("dump assets")
        session.wait_token(tokens.ASSETS_DUMP_TOKEN, 15, "the asset listing", begin)
        if re.search(pattern, session.read_since(begin)):
            return
    raise AssertionError(f"the listing never showed {pattern}")


def remove_created_files():
    """Deletes what the create wrote into the test project."""
    for path in CREATED_FILES.glob(f"{ASSET_NAME}.obj*"):
        path.unlink()


def main():
    session = EditorSession(
        "egui",
        "blender-link-create-smoke",
        project=SCENE_PROJECT_PATH,
        test_seconds=240,
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
        session_id = welcome[0][0].get("session")
        check(session_id, "the welcome names the editor's session")
        res_directory = welcome[0][0].get("res_directory") or ""
        check(
            res_directory and Path(res_directory).is_dir(),
            f"the welcome names the project's res folder ({res_directory})",
        )
        picked_path = str(Path(res_directory) / ASSET_PATH)

        # The link is an editor-only module: it registers when its own build
        # finishes, which is after the editor's first frame.
        session.wait_for_match(
            re.escape("[blender_link] pill_blender_link_editor loaded"),
            180,
            "the link module loading",
        )
        print("[smoke] ok the link module registered inside the editor")

        entities_before = entity_rows(session)
        begin = session.length()
        request_create(fake_blender, session_id, picked_path)

        results = wait_for_frames(fake_blender, "create_result", 1, 30.0)
        check(results, "the editor answered the create request")
        header = results[0][0]
        check(header.get("ok") is True, f"the create succeeded ({header.get('detail')})")
        check(header.get("asset_name") == ASSET_PATH, f"the answer names the asset ({header.get('asset_name')})")
        link_id = header.get("link_id")
        check(isinstance(link_id, int), f"the answer carries the link id ({link_id})")

        session.wait(
            log_layout.message_then_fields_pattern(
                re.escape(CREATE_ACCEPTED_TOKEN), r'\s+asset="[^"]+"'
            ),
            30,
            "the request being accepted",
            begin,
        )
        print("[smoke] ok the editor accepted the request")
        session.wait(
            log_layout.message_then_fields_pattern(
                re.escape(CREATED_ASSET_TOKEN),
                r'\s+asset="?' + re.escape(ASSET_PATH),
            ),
            30,
            "the mesh being written",
            begin,
        )
        print("[smoke] ok the editor wrote the mesh as an asset")
        session.wait(
            log_layout.message_then_fields_pattern(
                re.escape(CREATED_ENTITY_TOKEN), rf"\s+asset=\"[^\"]*\"\s+link_id={link_id}\s+entity="
            ),
            30,
            "the entity being spawned",
            begin,
        )
        print("[smoke] ok the editor spawned an entity for the new mesh")

        entities_after = entity_rows(session)
        created_rows = [row for row in entities_after if row not in entities_before]
        check(len(created_rows) == 1, f"the create spawned exactly one entity ({len(created_rows)})")
        check(
            "MeshRendererComponent" in created_rows[0][1],
            "the new entity carries a mesh renderer",
        )
        print(f"[smoke] the new entity: {created_rows[0][0]}")

        wait_for_asset_row(
            session,
            log_layout.message_then_fields_pattern(
                re.escape("[test] asset"),
                r'\s+path="?' + re.escape(ASSET_PATH) + r'"?\s+kind="[^"]*"\s+standalone=\S+\s+has_meta=true',
            ),
            60,
        )
        print("[smoke] ok the project lists the asset with its metadata")

        # The same file a second time is refused, and nothing new is spawned.
        entities_before_second = entity_rows(session)
        request_create(fake_blender, session_id, picked_path)
        refused = wait_for_frames(fake_blender, "create_result", 2, 30.0)
        check(len(refused) >= 2, "the second create was answered too")
        refusal = refused[-1][0]
        check(refusal.get("ok") is False, "the same name a second time is refused")
        check(
            "already loaded" in (refusal.get("detail") or ""),
            f"the refusal says the name is taken ({refusal.get('detail')})",
        )
        check(
            entity_rows(session) == entities_before_second,
            "a refused create spawns no entity",
        )

        # A destination outside the project's res is refused with the reason.
        request_create(
            fake_blender, session_id, str(SCENE_PROJECT_ROOT / "outside_create.obj")
        )
        outside = wait_for_frames(fake_blender, "create_result", 3, 30.0)
        check(len(outside) >= 3, "the outside create was answered too")
        outside_refusal = outside[-1][0]
        check(outside_refusal.get("ok") is False, "a destination outside res is refused")
        check(
            "outside the project's res" in (outside_refusal.get("detail") or ""),
            f"the refusal says why ({outside_refusal.get('detail')})",
        )

        # An object can also be pointed at a file the project already holds.
        begin = session.length()
        connect_path = str(SCENE_PROJECT_ROOT / "res" / "models" / "cube.obj")
        fake_blender.send(
            {
                "type": "connect_asset",
                "session": session_id,
                "object": "Cube",
                "path": connect_path,
            }
        )
        answers = wait_for_frames(fake_blender, "connect_result", 1, 30.0)
        check(answers, "the editor answered the connect request")
        answer = answers[-1][0]
        check(answer.get("ok") is True, f"the connect succeeded ({answer.get('detail')})")
        check(
            answer.get("asset_name") == "models/cube.obj",
            f"the answer names the asset below res ({answer.get('asset_name')})",
        )
        check(
            isinstance(answer.get("link_id"), int),
            f"the answer carries the link id ({answer.get('link_id')})",
        )
        check(answer.get("object") == "Cube", "the answer names the object that asked")
        session.wait(
            log_layout.message_then_fields_pattern(
                re.escape("[blender_link] asset connected"),
                r'\s+asset="?models/cube\.obj"?' + r"\s+link_id=\d+",
            ),
            30,
            "the asset being connected",
            begin,
        )
        print("[smoke] ok the editor linked an existing asset by path")

        # A file outside the project's res is refused with the reason.
        begin = session.length()
        fake_blender.send(
            {
                "type": "connect_asset",
                "session": session_id,
                "object": "Cube",
                "path": str(SCENE_PROJECT_ROOT / "Cargo.toml"),
            }
        )
        answers = wait_for_frames(fake_blender, "connect_result", 2, 30.0)
        check(len(answers) >= 2, "the second connect was answered too")
        refusal = answers[-1][0]
        check(refusal.get("ok") is False, "a file outside res is refused")
        check(
            "outside the project's res" in (refusal.get("detail") or ""),
            f"the refusal says why ({refusal.get('detail')})",
        )
        session.wait(
            log_layout.message_then_fields_pattern(
                re.escape("[blender_link] connect refused"), r"\s+object=\"[^\"]*\""
            ),
            30,
            "the refusal being logged",
            begin,
        )
        print("[smoke] ok a file outside the project's res is refused")
        print("[smoke] done")
    finally:
        if fake_blender is not None:
            fake_blender.close()
        session.close()
        remove_created_files()


main()
