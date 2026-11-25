"""End-to-end acceptance for the egui editor frontend.

PROTECTS: The egui editor: startup, shell patch and reload, dock and viewports, selection and gizmo, inspector and assets, systems and preferences, theme layers, game view, toolbox and grid, recovery and clean exit.
TIER: editor
SETS: editor

Builds against an already-built `pill_editor` binary - verified before use, so
a binary older than the sources it was compiled from is refused rather than
trusted - and starts it on
`scene` - three models and a camera, so the frames are fast and
the heartbeat waits are short - with the scripted input channel
(`PILL_EDITOR_TEST_COMMANDS`), and asserts on its log tokens: startup and the
two scene surfaces (the Scene and Scene 2 viewports), the egui shell module
loading, both tabs reporting rows, a scene click that picks an entity through
the renderer's pick contract, moves the shared selection and publishes the
outline, the inspector reporting the picked entity's detail and one of that
entity's rows edited through the scripted `edit array` (the history step and
the undo that puts the value back), a corner click
that clears both, scripted gizmo drags that translate, rotate and scale the
selected entity (checked through `dump field`), the Assets pipeline (the
listing, a create through its dialog, a rename that the selection follows, a
move, a delete that undoes back, and the write-meta action on a staged
sourced file), the Inspector's asset-settings rows round-tripping through
`edit asset-settings` (a boolean row and an enum row, each applied to the
asset's `.meta` sidecar and undone back to its original value), the Systems
tab's data (a scripted `dump systems`, a recorded system toggle with its
undo, and the pause and step commands reaching the engine), the context
menus (a real right-click opening the viewport's menu, and the menu
actions' command paths: the entity create/delete pair with the delete's
undo, a component detach and attach round-trip, and the gizmo tool, snap
and space switches), the Preferences tab (a seeded preferences file whose
step cap the live reader applies at startup, a scripted row whose save
reaches the file and the reader, the saved snapping step steering a
snapped drag, the history trimming at a tiny cap, and a tool choice
persisting into the project's remembered entry), the editor's theme layers
(a user file named by the `theme` preference, a project file under
`res/editor/` that the settings' `editor.theme` re-points, an out-of-scope path
refused with the rule, a legacy name reported and a broken user file
refused while the other layer applies - each checked through `dump theme`),
the Game tab's viewport
(the primary surface's child window placed at the tab's body while that tab
is the visible one, and hidden with it), the floating toolbox (its rows
switching the tool without picking the scene, and a scripted pointer drag
parking it inside the viewport while it keeps taking input), the infinite
scene grid (on over the scene viewports by default, switched off and on
through the toolbox's grid row while the game viewport never draws it), a dock
rearrangement (an undocked
tab) through the scripted channel, a live patch of the shell that keeps its
frame counter running, a structural edit that reloads the shell and keeps the
rearranged dock, a project reload that keeps the editor UI, the second
viewport's undock with its own scene surface going live at the window's
rectangle, and a clean exit.

Each run keeps its log, sandbox and command file under
`local/editor-validation/egui-<run id>/`. The sandbox belongs to that run: a
fresh run starts from the preferences the suite seeds, and only the in-run
restart reuses the same sandbox, so the dock layout and the clipboard survive
the quit. The only
sources this suite edits are the shell module's, the example project's
(its `scene.rs` and the two `.meta` sidecars whose settings rows the
Inspector step round-trips through the real apply), all restored when the
session closes; everything else it starts is its own process tree.
"""
import argparse
import json
import os
import time
import yaml
import sys
from pathlib import Path

DEVOPS_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(DEVOPS_ROOT))

from core.editor_session import EditorSession  # noqa: E402
from core.paths import MODULES_ROOT  # noqa: E402
from core.projects import (  # noqa: E402
    SCENE_PROJECT_PATH,
    SCENE_PROJECT_RES,
    SCENE_PROJECT_SCENE_RS,
)
from core.suite_common import (  # noqa: E402
    concurrent_workspace_build_refusal,
    verify_editor_binary,
)
from core.editor_log_tokens import (  # noqa: E402
    EGUI_EDITOR_BINARY,
)# Steps 11-14 (the sessions after the main one) live in their own module, one
# function per step area. The import is top level: the vocabulary they read is
# `core.editor_log_tokens`, so nothing holds a cycle together.
from editor_step_support import (  # noqa: E402
    StepState,
    read_budget_exceeded,
    write_preview_material,
)
from editor_step_areas import (  # noqa: E402
    StepContext,
    discover_areas,
    print_subset_banner,
    select_areas,
)


# The last run's per-area outcome, read back by `--steps failed`.
LAST_RUN_SUMMARY = Path(__file__).resolve().parents[2] / "local" / "editor-validation" / "egui-last-run.json"

# The guid the material preview step's second shader is staged under. It is the
# PBR chain's shader copied into the project, so the two materials the step
# compares are drawn through two shaders that behave the same.
PREVIEW_SHADER_GUID = "c7f1a2b3d4e5968778695a4b3c2d1e0f"


def failed_areas_of_last_run() -> list:
    """The area names the last recorded run failed in ([] when it passed or none is recorded)."""
    try:
        summary = json.loads(LAST_RUN_SUMMARY.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    return [area["name"] for area in summary.get("areas", []) if area.get("state") == "failed"]


def write_run_summary(area_results: list, steps: str) -> None:
    """Records each area's time and outcome: the last-run file and, under agent_run, timings.json.

    `--steps failed` reads the first to rerun only what broke; agent_run's
    `work checks --steps <check>` reads the second for per-area medians, so a
    slow area shows up as a trend instead of a feeling.
    """
    summary = {"finished": time.strftime("%Y-%m-%d %H:%M:%S"), "steps": steps, "areas": area_results}
    try:
        LAST_RUN_SUMMARY.parent.mkdir(parents=True, exist_ok=True)
        LAST_RUN_SUMMARY.write_text(json.dumps(summary, indent=2), encoding="utf-8")
    except OSError as error:
        print("warning: the run summary was not written: {}".format(error))
    run_directory = os.environ.get("AGENT_RUN_DIRECTORY")
    if run_directory:
        try:
            (Path(run_directory) / "timings.json").write_text(
                json.dumps({"steps": area_results}, indent=2), encoding="utf-8"
            )
        except OSError as error:
            print("warning: timings.json was not written: {}".format(error))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        help="the built pill_editor binary (default: the debug build)",
    )
    parser.add_argument(
        "--project",
        default=SCENE_PROJECT_PATH,
        help="the project, resolved the way the editor resolves PROJECT_PATH (from modules/)",
    )
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument(
        "--steps",
        default="all",
        help="all (the default), list (prints the area names in run order and "
        "checks every area module imports, without a build), failed (the areas "
        "the last run failed in), or a comma-separated subset of area names",
    )
    arguments = parser.parse_args()
    # The areas are discovered before anything else: `--steps list` must answer
    # without a build, a lock or an editor. Discovery imports every area module,
    # so a broken one fails here rather than halfway through a run.
    areas = discover_areas()
    if arguments.steps == "list":
        for area in areas:
            print(area.name)
        return 0
    # The cheap rerun after a failure: only what broke, not the whole suite.
    if arguments.steps == "failed":
        failed = failed_areas_of_last_run()
        if not failed:
            print("the last recorded run failed in no area ({}); nothing to rerun".format(LAST_RUN_SUMMARY))
            return 0
        print("[RERUN] the last run failed in: {}".format(", ".join(failed)))
        arguments.steps = ",".join(failed)
    chosen = select_areas(areas, arguments.steps)
    # The egui editor needs the DirectComposition overlay, a Windows facility.
    if not EditorSession.supported():
        print("SKIP: the egui editor suite needs Windows")
        return 0
    # A concurrent cargo build of this workspace would rewrite the shared
    # dylibs this suite's modules load against. The suite refuses here, and
    # the same check runs again under the host lock, immediately before the
    # launch: a build that starts while this process waits for another
    # suite's lock is the case that once killed a run at startup.
    refusal = concurrent_workspace_build_refusal()
    if refusal:
        raise SystemExit(refusal)
    # The default must be the egui binary: EditorSession's own default is the
    # native editor, whose binary is a different build and crashes at startup
    # against the egui editor's dylib split.
    binary = arguments.binary.resolve() if arguments.binary else EGUI_EDITOR_BINARY
    # Built by the gate, not by this suite, so it is verified rather than
    # trusted: a binary older than the sources it was compiled from would test
    # code that no longer exists and report green.
    refusal = verify_editor_binary(binary)
    if refusal:
        raise SystemExit(refusal)
    shell_source = MODULES_ROOT / "editor" / "pill_editor_shell" / "src" / "shell.rs"
    # Byte-faithful reads (`newline=""`): `EditorSession.edit_source` writes
    # the text back as given, so a translated read would rewrite every line's
    # ending and the host's patch classifier would see a whole-file change
    # instead of the one body the step edits.
    with shell_source.open("r", encoding="utf-8", newline="") as file:
        original_source = file.read()
    # The Entities tab body lives in its own file, beside the root.
    tab_entities_source = shell_source.parent / "shell" / "tab_entities.rs"
    with tab_entities_source.open("r", encoding="utf-8", newline="") as file:
        original_tab_source = file.read()
    # The Systems tab's body has a file of its own too; its patch case proves
    # the module whose opening once reached only the sibling glob.
    tab_systems_source = shell_source.parent / "shell" / "tab_systems.rs"
    with tab_systems_source.open("r", encoding="utf-8", newline="") as file:
        original_systems_source = file.read()
    project_source = SCENE_PROJECT_SCENE_RS
    # The Assets step's meta probe: a sourced file with no `.meta` sidecar
    # yet, staged before the editor starts so the res watcher never sees a
    # creation event for it - the write-meta action answers exactly this
    # state.
    asset_root = SCENE_PROJECT_RES
    stage_file = asset_root / "s16_stage.png"
    samples = sorted((asset_root / "textures").glob("*.png"))
    assert samples, "no texture png to stage the meta probe from"
    stage_file.write_bytes(samples[0].read_bytes())
    # The asset reference step needs a material file: the
    # example builds its materials in code, and the picker offers the files of
    # the field's type, so there must be one in `res` to pick and to drop. It
    # names the PBR chain's shader - installed in every build under a guid
    # derived from its name (`pbr_pipeline::SHADER_GUID`) - and binds no maps,
    # so it loads and draws in this project. Staged before the editor starts,
    # like the meta probe, so the res watcher sees it indexed at startup.
    reference_material = asset_root / "materials" / "s16_reference.material"
    reference_material.parent.mkdir(exist_ok=True)
    reference_material.write_text(
        yaml.safe_dump(
            {
                "format_version": 1,
                "asset_type": "pill_rendering::Material",
                "guid": "5f16a9c2d47b4e0f8a1b2c3d4e5f6a7b",
                "asset": {
                    "shader": "8cedb4197c9f26a86708b1cd14399eab",
                    "textures": {},
                    "parameters": {
                        "pbr_base": {"Color": [1.0, 1.0, 1.0]},
                        "pbr_emissive": {"Color": [1.0, 1.0, 1.0]},
                        "pbr_metallic": {"Scalar": 1.0},
                        "pbr_roughness": {"Scalar": 1.0},
                    },
                    "rendering_order": 255,
                },
            },
            sort_keys=False,
            default_flow_style=False,
        ),
        encoding="utf-8",
    )
    # The material preview step compares two materials that differ only in the
    # shader they name: each tile is drawn through the material's own shader on
    # the renderer's preview sphere, and the renderer names the shader it drew
    # through. One names the PBR chain's shader, the other the project's own
    # second shader, which is that stage under a guid of its own. They get a
    # folder of their own, so the materials folder other steps browse keeps the
    # files it had. Staged before the editor starts, so both are in the store
    # when the grid asks for their tiles.
    preview_materials = (
        (
            "preview_shader_a.material",
            "a1b2c3d4e5f60718293a4b5c6d7e8f90",
            "8cedb4197c9f26a86708b1cd14399eab",
        ),
        (
            "preview_shader_b.material",
            "b1c2d3e4f5061728394a5b6c7d8e9f01",
            PREVIEW_SHADER_GUID,
        ),
    )
    preview_material_folder = asset_root / "preview_materials"
    for name, guid, shader in preview_materials:
        write_preview_material(preview_material_folder / name, guid, shader)
    # The Inspector's settings step writes two `.meta` sidecars through the
    # real apply and undoes them; the engine's normalization reorders their
    # keys, so their original bytes are snapshotted and put back in the
    # finally block, the way the sources are.
    inspector_metas = [
        asset_root / "models" / "cube.obj.meta",
        asset_root / "textures" / "reimport_sample.png.meta",
    ]
    original_metas = {}
    for meta_path in inspector_metas:
        assert meta_path.is_file(), f"the settings step's sidecar is missing: {meta_path}"
        original_metas[meta_path] = meta_path.read_bytes()
    # The theme layers' files live under `res/editor/` and in
    # the project root; the example ships neither, so the step writes them and
    # the finally block takes them off the disk again.
    theme_root = asset_root / "editor"
    project_settings_path = asset_root.parent / "project_settings.yaml"
    original_project_settings = project_settings_path.read_bytes()
    # The phases wait for two row heartbeats, a scene-selection probe, a
    # gizmo drag phase, the assets pipeline and a patch compile; the
    # executable's seconds deadline is a hang guard, kept comfortably above
    # the phases even on a slow machine.
    session = EditorSession(
        "egui",
        "main",
        project=arguments.project,
        test_seconds=420,
        binary=binary,
    )
    # The Preferences step seeds the file before the editor starts: the
    # seeded cap proves the startup read of the live reader, and the
    # project's remembered entry keeps the tool at the defaults (translate,
    # world, unsnapped) so the earlier steps see the state they expect.
    session.write_appdata_yaml(
        "preferences.yaml",
        {
            "history_step_cap": 400,
            "snapping_steps": {
                "translation": 0.25,
                "rotation_degrees": 15.0,
                "scale": 0.1,
            },
            "gizmo": {
                "projects": {
                    "scene": {
                        "mode": "translate",
                        "space": "world",
                        "snap": False,
                    }
                }
            },
        },
    )
    session.start(
        extra_env={"PILL_EDITOR_CONTINUOUS_REDRAW": "1"},
        preflight=concurrent_workspace_build_refusal,
    )
    state = StepState()
    state.session = session
    state.asset_root = asset_root
    state.project_settings_path = project_settings_path
    state.theme_root = theme_root
    state.original_source = original_source
    state.original_systems_source = original_systems_source
    state.original_tab_source = original_tab_source
    state.shell_source = shell_source
    state.tab_entities_source = tab_entities_source
    state.tab_systems_source = tab_systems_source
    state.project_source = project_source
    context = StepContext(
        state=state, session=session, binary=binary, project=arguments.project
    )
    if arguments.steps != "all":
        print_subset_banner(chosen, areas)
    # Each area's time and outcome; the area still "running" when an exception
    # leaves the loop is the one that failed.
    area_results = []
    area_started = time.monotonic()
    try:
        for area in chosen:
            print("[STEP AREA] {}".format(area.name))
            area_started = time.monotonic()
            area_results.append({"name": area.name, "seconds": None, "state": "running"})
            area.run(context)
            area_results[-1].update(seconds=round(time.monotonic() - area_started, 1), state="passed")
        # The read budget: the cursor decodes each appended byte once, so a run
        # that read far more than the log holds has a poll rereading the whole
        # file instead of appending to what it already read.
        assert not read_budget_exceeded(session), (
            "the run read {} characters of a {}-character log; a poll is "
            "rereading the file".format(
                session.log_characters_read(), session.length()
            )
        )
    finally:
        if area_results and area_results[-1]["state"] == "running":
            area_results[-1].update(seconds=round(time.monotonic() - area_started, 1), state="failed")
        write_run_summary(area_results, arguments.steps)
        session.close()
        # The step's files (created, renamed, moved, restored, and their
        # sidecars) come off the project's disk; the staging probe too.
        for pattern in (
            "s16_stage.png*",
            "textures/s16_probe*",
            "textures/s16_renamed*",
            "textures/s16_pipeline*",
            "models/s16_moved*",
        ):
            for path in asset_root.glob(pattern):
                path.unlink(missing_ok=True)
        # The staged materials go, along with any `.meta` the host wrote beside
        # them; the project's own material stays.
        staged_materials = asset_root / "materials"
        for pattern in ("s16_reference*", "preview_shader*"):
            for path in staged_materials.glob(pattern):
                path.unlink(missing_ok=True)
        # The preview step's own folder goes with its files; the project ships
        # none.
        if preview_material_folder.is_dir():
            for path in preview_material_folder.iterdir():
                path.unlink(missing_ok=True)
            preview_material_folder.rmdir()
        # The settings step's sidecars return to their recorded bytes: the
        # engine's normalization reorders their keys even when the values
        # come back through undo.
        for meta_path, original in original_metas.items():
            meta_path.write_bytes(original)
        # The theme layers' files, the font they name and the project's editor
        # settings go together: the example ships none of them.
        if theme_root.is_dir():
            for theme_file in sorted(theme_root.rglob("*"), reverse=True):
                if theme_file.is_file():
                    theme_file.unlink(missing_ok=True)
                elif theme_file.is_dir():
                    theme_file.rmdir()
            theme_root.rmdir()
        project_settings_path.write_bytes(original_project_settings)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
