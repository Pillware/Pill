#!/usr/bin/env python3
"""
The shipping bundle packs every linked module's asset roots.

PROTECTS: A shipped build reads a module's resources from its asset pack: the generated build script names every linked module's `res` (its shader tree included) and packs it with the multi-root writer.
TIER: static

REQUIREMENTS: Python 3.8+ and PyYAML (the generator imports it). No build runs.

DESCRIPTION
    A module ships its resources beside its code (`res`, which holds the
    shader tree its build script cooks under `shaders/`) and declares them as
    an asset root at runtime. A shipped build has no module directory left to
    declare, so the pack is the only source: when the pack misses them the renderer's default pipeline
    fails to install and the game quietly falls back to its built-in chain.

    The pack writer's half is covered by `pill_assets`' own unit tests. This
    suite covers the generator's half - which directories it selects and what
    the build script it emits does with them - by importing
    `devops/tools/generate_shipping_bundle.py` and asserting:

      - `devops/tests/projects/native` selects the renderer data crate's `res`
        as its one root, and a module that ships nothing beside its code
        contributes no root;
      - a group folder's extension crates are found, and the group itself is
        never read as an extension;
      - the emitted build script calls `write_asset_pack_with_roots`, names
        the root relative to the bundle, cooks each module root's shaders
        before packing it, and cooks the project's own shader sources too - a
        project has no build script to cook them, and the shader import reads
        the cooked file, not the source.

USAGE
  python devops/tests/test_shipping_bundle_roots.py

EXAMPLE USAGE
  python devops/tests/test_shipping_bundle_roots.py

--- SCRIPT ---
"""

import importlib.util
import sys
import tempfile
from pathlib import Path

DEVOPS_ROOT = Path(__file__).resolve().parent.parent
REPOSITORY_ROOT = DEVOPS_ROOT.parent
sys.path.insert(0, str(DEVOPS_ROOT))

from core.projects import NATIVE_PROJECT_ROOT  # noqa: E402

GENERATOR_PATH = DEVOPS_ROOT / "tools" / "generate_shipping_bundle.py"
PROJECT_ROOT = NATIVE_PROJECT_ROOT
RENDERER = "pill_master_renderer"
DATA_CRATE_NAME = RENDERER + "_data"
DATA_CRATE = REPOSITORY_ROOT / "modules" / "extensions" / "pill_master_renderer" / "modules" / "runtime" / "data"
BUNDLE_DIRECTORY = REPOSITORY_ROOT / "build" / "pill_shipping_bundle"


def load_generator():
    """Imports the generator by path: it lives in `devops/tools`, not `core`."""
    spec = importlib.util.spec_from_file_location(
        "generate_shipping_bundle", GENERATOR_PATH
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> int:
    generator = load_generator()
    failures = []

    def expect(description: str, ok: bool) -> None:
        print(f"  [{'PASS' if ok else 'FAIL'}] {description}")
        if not ok:
            failures.append(description)

    modules = generator.modules_with_renderer_data(
        generator.load_renderer(PROJECT_ROOT), generator.load_module_list(PROJECT_ROOT)
    )
    expect(
        "the renderer's data crate leads the module list",
        bool(modules) and modules[0] == DATA_CRATE_NAME,
    )

    expected_roots = [DATA_CRATE / "res"]
    roots = generator.module_asset_roots(REPOSITORY_ROOT, modules)
    expect(
        "a module's res, shader tree included, is its one pack root",
        roots == expected_roots,
    )
    expect(
        "a module that ships nothing beside its code contributes no root",
        generator.module_asset_roots(REPOSITORY_ROOT, ["pill_spline"]) == [],
    )

    # A group folder - a directory under `extensions/` with no `modules/` folder
    # of its own - holds extension folders, and their crates are found exactly
    # as the host's index finds them. Bookkeeping folders are skipped.
    with tempfile.TemporaryDirectory() as temporary:
        extensions = Path(temporary) / "modules" / "extensions"
        for relative in (
            "audio/modules/runtime",
            "rendering/embedded/modules/runtime",
            "rendering/embedded/modules/editor",
            "rendering/_hidden/modules/runtime",
            "_anchor/group/modules/runtime",
        ):
            directory = extensions / relative
            directory.mkdir(parents=True)
            (directory / "Cargo.toml").write_text(
                '[package]\nname = "x"\n', encoding="utf-8"
            )
        found = [
            directory.relative_to(extensions).as_posix()
            for directory in generator.extension_crate_directories(Path(temporary))
        ]
        expect(
            "a group folder's extension crates are found, and the group is never read as one",
            found
            == [
                "audio/modules/runtime",
                "rendering/embedded/modules/runtime",
                "rendering/embedded/modules/editor",
            ],
        )

    script = generator.build_build_script(
        BUNDLE_DIRECTORY, PROJECT_ROOT / "res", roots, web=False
    )
    expect(
        "the build script packs with the multi-root writer",
        "pill_assets::write_asset_pack_with_roots(&source, &module_roots, &output)"
        in script,
    )
    for root in expected_roots:
        relative = generator.manifest_relative_path(BUNDLE_DIRECTORY, root)
        expect(
            f"the build script names {relative}",
            f'manifest_directory.join("{relative}")' in script,
        )
    expect(
        "the build script cooks every module root's shaders before packing it",
        "for root in &module_roots {\n        pill_assets::cook_shader_tree(root)" in script,
    )
    expect(
        "the build script collects the modules' shader include directories",
        "let module_include_directories: Vec<std::path::PathBuf>" in script
        and "layout.include_directories" in script,
    )
    expect(
        "the build script cooks the project's shader tree with those includes",
        "pill_assets::cook_shader_tree_with_include_directories(&source, &module_include_directories)"
        in script,
    )
    expect(
        "the build script reports every root as an input",
        "for root in &module_roots {\n        println!(\"cargo:rerun-if-changed={}\", root.display());"
        in script,
    )

    # A manager's settings are an ordinary file under `res/settings`, so the
    # pack carries them and the player reads them through the asset store,
    # exactly where the development host reads the project's copy.
    source = generator.build_library_source(
        "bundle_package",
        "Bundle Project",
        modules,
        "native",
        "../../example",
        BUNDLE_DIRECTORY,
        REPOSITORY_ROOT / "modules",
    )
    expect(
        "the bundle bakes no manager settings documents",
        "manager_settings" not in source and "StaticManagerSettings" not in source,
    )
    expect(
        "the settings directory travels in the project's pack",
        generator.ASSETS_DIRECTORY_NAME == "res",
    )

    if failures:
        print(f"\n  {len(failures)} check(s) failed")
        return 1
    print("\n  all checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
