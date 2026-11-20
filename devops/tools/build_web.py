#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+, PyYAML (for the bundle generator), the Rust
#               toolchain with the `wasm32-unknown-unknown` target
#               (`rustup target add wasm32-unknown-unknown`), and `wasm-pack`
#               on PATH (`cargo install wasm-pack`). On its first run wasm-pack
#               downloads the `wasm-bindgen` CLI matching the crate version,
#               and in a shipping build also `wasm-opt` unless `--no-wasm-opt`.
#               cargo-about for the license notices (`cargo install cargo-about
#               --locked --features cli`; without it the build warns and
#               writes no notices).

# DESCRIPTION: Builds a Rust project for the browser. It regenerates the
#   shipping bundle and the web app crate (`generate_shipping_bundle.py
#   --web`), builds the app for `wasm32-unknown-unknown` with wasm-pack, and
#   puts the page (`devops/tools/web/index.html`) beside the generated
#   `pkg/` in `<project>/build/web/`. Serve that directory over HTTP to play.
#
#   The project's assets travel beside the page, not inside the module:
#   the bundle's build script packs them, this script copies
#   `assets.pillpack` next to `index.html`, and `pill_web` fetches and mounts
#   it before the project initializes.
#
#   The web app crate has a workspace of its own and is built from its own
#   directory, so the engine workspace's `.cargo/config.toml` (whose
#   `-C prefer-dynamic` a wasm build cannot use) does not apply. The engine
#   workspace's `Cargo.lock` is copied in first, so the browser build resolves
#   the same dependency versions as the native one. `RUSTC_WRAPPER` is cleared
#   because sccache cannot spawn `web-sys`'s very long command line on Windows.
#
#   The third-party license notices of the crates compiled into the module
#   are written into `licenses/` beside the page
#   (devops/tools/generate_third_party_licenses.py). A crate under a license
#   `modules/about.toml` does not accept fails the build.
#
#   Like a shipping build, this regenerates `build/pill_shipping_bundle`;
#   restore the committed stub afterwards with
#   `git checkout -- build/pill_shipping_bundle`.
#
# USAGE: python devops/tools/build_web.py [--dev] [--editor] [--editor-frontend egui]
#          [--no-wasm-opt] [project_path]
#          --dev           unoptimized build with debug info (faster to build)
#          --editor        build the editor posture: the shell compiled into
#                          the wasm, the two-canvas editor page beside it
#          --editor-frontend
#                          which editor shell an --editor build links:
#                          only `egui` exists
#          --no-wasm-opt   skip wasm-opt in a shipping build
#          [project_path]  workspace-relative project directory; defaults to
#                          the PROJECT_PATH environment variable
#
# EXAMPLE USAGE:
#   python devops/tools/build_web.py devops/tests/projects/scene
#   python devops/tools/build_web.py --dev devops/tests/projects/scene
#   python devops/tools/build_web.py --dev --editor devops/tests/projects/native
#   python -m http.server 8000 -d devops/tests/projects/scene/build/web

# --- SCRIPT ---

# Standard library
import os
import shutil
import subprocess
import sys
from pathlib import Path

# Locations, relative to the repository root.
GENERATOR = Path("devops") / "tools" / "generate_shipping_bundle.py"
PAGE_TEMPLATE = Path("devops") / "tools" / "web" / "index.html"
# The editor posture's page: two stacked canvases
# and the scripted channel's page hook.
EDITOR_PAGE_TEMPLATE = Path("devops") / "tools" / "web" / "editor.html"
# The editor shells an `--editor` build can link (the generator's list).
EDITOR_FRONTENDS = ("egui",)
DEFAULT_EDITOR_FRONTEND = "egui"
WEB_APP_DIRECTORY = Path("build") / "pill_web_app"
WORKSPACE_LOCK = Path("modules") / "Cargo.lock"
WEB_TARGET_DIRECTORY = Path("build") / "build_meta" / "web_target"
# The module wasm-pack writes, named after the web app crate.
WEB_MODULE_NAME = "pill_web_app_bg.wasm"
# The project's assets, packed by the bundle's build script and fetched by the
# module at startup. It sits beside the page, not inside the module.
ASSET_PACK_FILE_NAME = "assets.pillpack"
BUNDLE_DIRECTORY = Path("build") / "pill_shipping_bundle"
WEB_PACK_DIRECTORY = "pack"
# Writes the third-party license notices, and the exit code it returns when
# cargo-about is not installed.
LICENSE_GENERATOR = Path("devops") / "tools" / "generate_third_party_licenses.py"
LICENSE_GENERATOR_TOOL_MISSING = 3
WEB_TARGET = "wasm32-unknown-unknown"


def repository_root() -> Path:
    """The repository root, located from this script."""
    return Path(__file__).resolve().parent.parent.parent


def run(command: list, working_directory: Path, environment=None) -> int:
    """Runs `command`, echoing it first; returns its exit code."""
    print("$ " + " ".join(str(part) for part in command), flush=True)
    return subprocess.call(command, cwd=working_directory, env=environment)


def main() -> int:
    """Builds the web version of a project; returns the process exit code."""
    # Parse the flags and the optional project path.
    development = False
    wasm_opt = True
    editor = False
    editor_frontend = DEFAULT_EDITOR_FRONTEND
    positional = []
    arguments = sys.argv[1:]
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == "--dev":
            development = True
        elif argument == "--editor":
            editor = True
        elif argument == "--editor-frontend":
            index += 1
            if index >= len(arguments):
                print("error: --editor-frontend requires a value", file=sys.stderr)
                return 2
            editor_frontend = arguments[index]
        elif argument.startswith("--editor-frontend="):
            editor_frontend = argument[len("--editor-frontend=") :]
        elif argument == "--no-wasm-opt":
            wasm_opt = False
        else:
            positional.append(argument)
        index += 1
    if editor_frontend not in EDITOR_FRONTENDS:
        print(
            "error: --editor-frontend must be one of "
            f"{', '.join(EDITOR_FRONTENDS)}; got {editor_frontend}",
            file=sys.stderr,
        )
        return 2
    if len(positional) > 1:
        print(
            "usage: build_web.py [--dev] [--editor] [--editor-frontend egui] "
            "[--no-wasm-opt] [project_path]",
            file=sys.stderr,
        )
        return 2
    project_path = positional[0] if positional else os.environ.get("PROJECT_PATH", "")
    if not project_path:
        print("error: no project given and PROJECT_PATH is not set", file=sys.stderr)
        return 2
    root = repository_root()
    project_root = (root / project_path).resolve()

    # Step 1: regenerate the shipping bundle and the web app crate. The
    # editor posture links a shell into the same wasm; `--editor-frontend`
    # picks which one (the egui shell by default).
    generator_command = [sys.executable, root / GENERATOR, "--web"]
    if editor:
        generator_command += ["--editor", "--editor-frontend", editor_frontend]
    generator_command.append(project_path)
    status = run(generator_command, root)
    if status != 0:
        return status

    # Step 2: pin the dependency versions the native build uses.
    web_app_directory = root / WEB_APP_DIRECTORY
    shutil.copyfile(root / WORKSPACE_LOCK, web_app_directory / "Cargo.lock")

    # Step 3: build for the browser. The output lands straight in the web
    # build directory; the page is copied next to it.
    output_directory = project_root / "build" / "web"
    package_directory = output_directory / "pkg"
    environment = dict(
        os.environ,
        RUSTC_WRAPPER="",
        RUSTFLAGS="",
        CARGO_TARGET_DIR=str(root / WEB_TARGET_DIRECTORY),
    )
    command = [
        "wasm-pack",
        "build",
        "--target",
        "web",
        "--no-typescript",
        "--no-pack",
        "--out-dir",
        str(package_directory),
        "--dev" if development else "--release",
    ]
    if not development and not wasm_opt:
        command.append("--no-opt")
    status = run(command, web_app_directory, environment)
    if status != 0:
        return status
    page_template = EDITOR_PAGE_TEMPLATE if editor else PAGE_TEMPLATE
    shutil.copyfile(root / page_template, output_directory / "index.html")

    # The bundle's build script packed the project's `res` directory beside
    # its crate; the page fetches it from its own directory, so it lands here.
    pack_source = root / BUNDLE_DIRECTORY / WEB_PACK_DIRECTORY / ASSET_PACK_FILE_NAME
    if not pack_source.is_file():
        print(
            f"error: the bundle did not pack {pack_source}",
            file=sys.stderr,
        )
        return 2
    pack_path = output_directory / ASSET_PACK_FILE_NAME
    shutil.copyfile(pack_source, pack_path)

    # The license notices of every crate compiled into the module, beside the
    # page. A missing cargo-about only warns; a license problem fails.
    status = run(
        [
            sys.executable,
            root / LICENSE_GENERATOR,
            "--manifest-path",
            web_app_directory / "Cargo.toml",
            "--target",
            WEB_TARGET,
            "--output-directory",
            output_directory,
            "--title",
            "Pill Engine web build - third-party licenses",
        ],
        root,
    )
    if status not in (0, LICENSE_GENERATOR_TOOL_MISSING):
        return status

    # Step 4: report where it is and how to open it.
    module_path = package_directory / WEB_MODULE_NAME
    size_megabytes = module_path.stat().st_size / (1024 * 1024)
    pack_megabytes = pack_path.stat().st_size / (1024 * 1024)
    print(f"web build: {output_directory}")
    print(f"  {WEB_MODULE_NAME}: {size_megabytes:.1f} MB")
    print(f"  {ASSET_PACK_FILE_NAME}: {pack_megabytes:.1f} MB")
    print(f"  serve it with: python -m http.server 8000 -d \"{output_directory}\"")
    return 0


if __name__ == "__main__":
    sys.exit(main())
