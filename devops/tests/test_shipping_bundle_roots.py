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
        as its one root, the statically linked renderer's `res` travels beside
        it, and a module that ships nothing beside its code contributes no
        root;
      - a group folder's extension crates are found, and the group itself is
        never read as an extension;
      - the emitted build script calls `write_asset_pack_with_roots`, names
        the root relative to the bundle, cooks each module root's shaders
        before packing it, and cooks the project's own shader sources too - a
        project has no build script to cook them, and the shader import reads
        the cooked file, not the source;
      - a renderer that tags a build target is linked for that target alone:
        the bundle declares it under that target's `cfg` and hands the host a
        `StaticRenderer` arm per target, so a device build links no GPU crate
        and a desktop build no display one, and `renderer: none` links neither;
      - the project's `renderer_features:` reach cargo as a named feature on
        the renderer's dependency edge, which keeps its default features off,
        and an arm that does not declare a name is not given it.

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
RENDERER = "pill_renderer"
DATA_CRATE_NAME = "pill_rendering"
DATA_CRATE = REPOSITORY_ROOT / "modules" / "extensions" / "rendering" / "pill_rendering_core" / "modules" / "runtime" / "data"
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
        REPOSITORY_ROOT,
        generator.load_renderer(PROJECT_ROOT),
        generator.load_module_list(PROJECT_ROOT),
    )
    expect(
        "the renderer's data crate leads the module list",
        bool(modules) and modules[0] == DATA_CRATE_NAME,
    )
    expect(
        "the renderer names its data crate in its own manifest",
        generator.declared_renderer_data(
            generator.module_crate_directory(REPOSITORY_ROOT, RENDERER)
        )
        == DATA_CRATE_NAME,
    )

    expected_roots = [DATA_CRATE / "res"]
    roots = generator.module_asset_roots(REPOSITORY_ROOT, modules)
    expect(
        "a module's res, shader tree included, is its one pack root",
        roots == expected_roots,
    )
    # The renderer is linked statically rather than listed as a module, and it
    # is the crate that ships the shaders, so its own `res` has to travel too.
    linked_roots = generator.module_asset_roots(REPOSITORY_ROOT, modules + [RENDERER])
    expect(
        "the linked renderer's res, its shader tree included, is packed too",
        linked_roots
        == expected_roots
        + [generator.module_crate_directory(REPOSITORY_ROOT, RENDERER) / "res"],
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

    # A renderer that tags a target is linked for that target only: the bundle
    # names one dependency and one `StaticRenderer` arm per target, and Cargo
    # picks the arm. A device build therefore links no GPU crate and a desktop
    # build no display one, without the generator guessing which is being
    # built; `renderer: none` links neither.
    modules_root = REPOSITORY_ROOT / "modules"
    arms = generator.renderer_target_arms(REPOSITORY_ROOT, RENDERER)
    expect(
        "the display renderer serves the device target and the GPU one the targets left over",
        arms
        == [
            ('cfg(target_os = "espidf")', "pill_embedded_renderer"),
            ('cfg(not(any(target_os = "espidf")))', RENDERER),
        ],
    )
    targeted_manifest = generator.build_cargo_manifest(
        BUNDLE_DIRECTORY,
        PROJECT_ROOT,
        "native",
        modules,
        set(),
        REPOSITORY_ROOT,
        renderer=RENDERER,
        renderer_arms=arms,
    )
    plain_table = targeted_manifest.split("[target.")[0]
    expect(
        "the display renderer is a dependency of its own target only",
        '[target.\'cfg(target_os = "espidf")\'.dependencies]\npill_embedded_renderer = { path = '
        in targeted_manifest
        and "pill_embedded_renderer" not in plain_table,
    )
    expect(
        "the GPU renderer is the dependency of the targets left over",
        "[target.'cfg(not(any(target_os = \"espidf\")))'.dependencies]\n"
        f"{RENDERER} = {{ path = " in targeted_manifest,
    )
    headless_manifest = generator.build_cargo_manifest(
        BUNDLE_DIRECTORY,
        PROJECT_ROOT,
        "native",
        modules,
        set(),
        REPOSITORY_ROOT,
    )
    expect(
        "a build without a renderer declares neither renderer",
        "pill_renderer " not in headless_manifest
        and "pill_embedded_renderer" not in headless_manifest,
    )

    # `renderer_features:` reaches cargo on the renderer's own edge, because a
    # shipped build links that renderer with its default features off. A
    # target-specific arm is given only the names it declares.
    feature_arms = generator.renderer_features_for_arms(
        REPOSITORY_ROOT, arms, ["ray_tracing"]
    )
    expect(
        "the project's renderer features land on the renderer that declares them",
        feature_arms == {"pill_embedded_renderer": [], RENDERER: ["ray_tracing"]},
    )
    featured_manifest = generator.build_cargo_manifest(
        BUNDLE_DIRECTORY,
        PROJECT_ROOT,
        "native",
        modules,
        set(),
        REPOSITORY_ROOT,
        renderer=RENDERER,
        renderer_arms=arms,
        renderer_features=feature_arms,
    )
    renderer_lines = [
        line
        for line in featured_manifest.splitlines()
        if line.startswith(f"{RENDERER} = ") or line.startswith("pill_embedded_renderer = ")
    ]
    expect(
        "the shipped renderer keeps its default features off and names the one asked for",
        any(
            'default-features = false, features = ["ray_tracing"]' in line
            and line.startswith(f"{RENDERER} = ")
            for line in renderer_lines
        ),
    )
    expect(
        "a target arm that declares no such feature names none",
        any(
            line.startswith("pill_embedded_renderer = ") and "features = [" not in line
            for line in renderer_lines
        ),
    )
    unfiltered = generator.build_cargo_manifest(
        BUNDLE_DIRECTORY,
        PROJECT_ROOT,
        "native",
        modules,
        set(),
        REPOSITORY_ROOT,
        renderer=RENDERER,
        renderer_arms=arms,
    )
    expect(
        "a project that asks for no renderer feature names none",
        "features = [" not in unfiltered,
    )
    try:
        generator.renderer_features_for_arms(REPOSITORY_ROOT, arms, ["ray_tracin"])
    except ValueError as error:
        expect(
            "a feature no linked renderer declares is reported, not dropped",
            "ray_tracin" in str(error) and RENDERER in str(error),
        )
    else:
        expect("a feature no linked renderer declares is reported, not dropped", False)

    # The key is read from the project's own settings, as one name or a list.
    with tempfile.TemporaryDirectory() as temporary:
        project = Path(temporary)
        settings = project / generator.PROJECT_SETTINGS_FILE_NAME
        settings.write_text('name: "P"\nbuild_binary_name: "P"\n', encoding="utf-8")
        expect(
            "a project that names no renderer feature asks for none",
            generator.load_renderer_features(project) == [],
        )
        settings.write_text(
            'name: "P"\nbuild_binary_name: "P"\nrenderer_features: ray_tracing\n',
            encoding="utf-8",
        )
        expect(
            "one renderer feature is read as a one-name list",
            generator.load_renderer_features(project) == ["ray_tracing"],
        )
        settings.write_text(
            'name: "P"\nbuild_binary_name: "P"\nrenderer_features:\n'
            "  - ray_tracing\n  - another\n",
            encoding="utf-8",
        )
        expect(
            "a list of renderer features is read in order",
            generator.load_renderer_features(project) == ["ray_tracing", "another"],
        )

    targeted_source = generator.build_library_source(
        "bundle_package",
        "Bundle Project",
        modules,
        "native",
        "../../example",
        BUNDLE_DIRECTORY,
        modules_root,
        renderer=RENDERER,
        renderer_arms=arms,
    )
    expect(
        "the bundle hands the host one renderer per target, under matching cfgs",
        '    #[cfg(target_os = "espidf")]\n'
        "    return Some(StaticRenderer {\n"
        "        init: pill_embedded_renderer::register,"
        in targeted_source
        and '    #[cfg(not(any(target_os = "espidf")))]\n'
        "    return Some(StaticRenderer {\n"
        f"        init: {RENDERER}::register,"
        in targeted_source,
    )
    headless_source = generator.build_library_source(
        "bundle_package",
        "Bundle Project",
        modules,
        "native",
        "../../example",
        BUNDLE_DIRECTORY,
        modules_root,
    )
    expect(
        "a build without a renderer hands over none",
        "::register," not in headless_source.split("static_renderer")[1].split("}")[0]
        and "None" in headless_source.split("pub fn static_renderer")[1].split("}")[0],
    )

    # A project that names a target-specific renderer gets no renderer on the
    # targets it leaves over, rather than one it did not ask for.
    with tempfile.TemporaryDirectory() as temporary:
        tagged = Path(temporary) / "modules" / "extensions" / "rendering"
        runtime = tagged / "device" / "modules" / "runtime"
        runtime.mkdir(parents=True)
        (runtime / "Cargo.toml").write_text(
            '[package]\nname = "device_renderer"\n\n'
            "[package.metadata.pill]\n"
            'renderer_target = \'cfg(target_os = "espidf")\'\n',
            encoding="utf-8",
        )
        expect(
            "naming a target-specific renderer leaves the other targets without one",
            generator.renderer_target_arms(Path(temporary), "device_renderer")
            == [
                ('cfg(target_os = "espidf")', "device_renderer"),
                ('cfg(not(any(target_os = "espidf")))', None),
            ],
        )

    if failures:
        print(f"\n  {len(failures)} check(s) failed")
        return 1
    print("\n  all checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
