#!/usr/bin/env python3
"""
The shipping bundle packs every linked module's asset roots.

PROTECTS: A shipped build reads a module's resources from its asset pack: the generated build script names every linked module's `res` and `shader_sources` and packs them with the multi-root writer.
TIER: static

REQUIREMENTS: Python 3.8+ and PyYAML (the generator imports it). No build runs.

DESCRIPTION
    A module ships its resources beside its code (`res` and the shader tree
    its build script cooks) and declares them as asset roots at runtime. A
    shipped build has no module directory left to declare, so the pack is the
    only source: when the pack misses them the renderer's default pipeline
    fails to install and the game quietly falls back to its built-in chain.

    The pack writer's half is covered by `pill_assets`' own unit tests. This
    suite covers the generator's half - which directories it selects and what
    the build script it emits does with them - by importing
    `devops/tools/generate_shipping_bundle.py` and asserting:

      - `devops/tests/projects/native` selects the renderer data crate's `res` and
        `shader_sources`, in that order, and a module that ships nothing
        beside its code contributes no root;
      - the emitted build script calls `write_asset_pack_with_roots`, names
        both roots relative to the bundle, cooks each module's shader tree
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
from pathlib import Path

DEVOPS_ROOT = Path(__file__).resolve().parent.parent
REPOSITORY_ROOT = DEVOPS_ROOT.parent
sys.path.insert(0, str(DEVOPS_ROOT))

from core.projects import NATIVE_PROJECT_ROOT  # noqa: E402

GENERATOR_PATH = DEVOPS_ROOT / "tools" / "generate_shipping_bundle.py"
PROJECT_ROOT = NATIVE_PROJECT_ROOT
RENDERER = "pill_master_renderer"
DATA_CRATE_NAME = RENDERER + "_data"
DATA_CRATE = REPOSITORY_ROOT / "modules" / "extensions" / "master_renderer" / "modules" / "runtime" / "data"
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

    expected_roots = [DATA_CRATE / "res", DATA_CRATE / "shader_sources"]
    roots = generator.module_asset_roots(REPOSITORY_ROOT, modules)
    expect(
        "a module's res and shader tree are its pack roots, in that order",
        roots == expected_roots,
    )
    expect(
        "a module that ships nothing beside its code contributes no root",
        generator.module_asset_roots(REPOSITORY_ROOT, ["pill_spline"]) == [],
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
        "the build script cooks a shader tree before packing it",
        'if root.file_name().is_some_and(|name| name == "shader_sources")' in script
        and "pill_assets::cook_shader_tree(root)" in script,
    )
    expect(
        "the build script cooks the project's shader tree before packing it",
        "pill_assets::cook_shader_tree(&source)" in script,
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
