#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+, PyYAML installed, Rust toolchain (cargo) on PATH.
#               Run from anywhere; the repository root is located from this
#               script.

# DESCRIPTION: Generates the shipping bundle crate for the static (shipping)
#   posture of the engine host. The bundle is the single crate `pill_standalone`
#   links under `shipping`: it declares the project and every extension
#    selected by the project's `project_settings.yaml` as ordinary Rust
#   dependencies, and exposes the `StaticModule` / `StaticProject` registration
#   the static-link path initializes.
#
#   The project's scripting language is read from its manifest: a `Cargo.toml`
#   is a native Rust project, a `*.csproj` is a managed C# project. A managed
#   project has no cargo dependency to declare; its `project_backend()` instead
#   returns an external backend, the C# bridge's `CSharpBackend::coreclr`,
#   resolving the assemblies `dotnet build` produced against the engine
#   workspace root. With `--csharp-aot` it is `CSharpBackend::native_aot` and the
#   project subdirectory points at the `dotnet publish` NativeAOT output, which
#   the runtime loads directly (no hostfxr, no installed .NET).
#
#   Cargo resolves dependencies before build scripts run, so a `build.rs`
#   cannot pull modules in from a YAML file; this generator runs before cargo
#   instead (as a pre-step of the shipping build) and writes the bundle crate
#   under `<repository_root>/build/pill_shipping_bundle/`. The location is
#   project-agnostic (cargo needs a static path, and only one shipping binary
#   is built at a time), so `pill_standalone`'s manifest never names a specific
#   project.
#
#   A project with a `res` directory, and any linked module with asset roots,
#   gets a `build.rs` in the bundle: it packs the project's `res` and every
#   module's `res` (its shader tree included, under `shaders/`) with
#   `pill_assets::write_asset_pack_with_roots` at build time, and the bundle
#   embeds the pack as `StaticProject::asset_pack`, which the runtime mounts in
#   the engine's asset store. A shipped game then reads its assets - its own
#   and its modules' - from its own binary, wherever it runs. A browser build
#   cannot: a wasm module is downloaded whole, so `--web` packs the directories
#   beside the crate instead and the bundle names the file in
#   `StaticProject::asset_pack_url` for `pill_web` to fetch.
#   `devops/tools/build_web.py` copies the file next to the page.
#
#   With `--web` it also writes the web app crate, `build/pill_web_app`: a
#   `cdylib` whose `#[wasm_bindgen(start)]` shim runs the bundle's project
#   through `pill_web` on the page's canvas (or, for `--editor`, through the
#   browser editor frontend `pill_web_editor`). It has a `[workspace]` of its
#   own, so the engine workspace's native feature choices and `.cargo`
#   settings stay out of the wasm build. `devops/tools/build_web.py` builds
#   it. The host links it by path (like the project itself), so no workspace
#   edit is needed; the folder is gitignored, and regeneration is
#   content-based: unchanged output is not rewritten, so a stable tree shows
#   no diff.
#
#   The generated bundle is one project's build artifact, never the tree's
#   state: the committed files are a project-agnostic stub that refuses to
#   compile. `--restore-stub` writes that stub back, which is what the gate
#   runs after its shipping clippy step, and
#   `devops/tests/test_shipping_smoke.py` refuses a committed generation.
#
# USAGE: python devops/tools/generate_shipping_bundle.py [--feature <name>...]
#                                                     [project_path]
#        python devops/tools/generate_shipping_bundle.py --restore-stub
#          --feature <name>  project feature to enable on the `project`
#                            dependency in the bundle (repeatable; names the
#                            project does not declare are ignored). The
#                            shipping build forwards its requested features,
#                            so e.g. `--feature rendering` makes the static
#                            build link the project's renderer components.
#          --restore-stub    put the committed stub back instead of generating:
#                            the manifest, `src/lib.rs`, and removing the
#                            generated `build.rs`. Takes no project path.
#          --csharp-aot      emit the NativeAOT backend (`native_aot`) instead
#                            of the hostfxr one, for a managed project whose
#                            assembly was published with `dotnet publish
#                            -p:PublishAot=true`.
#          --rid <rid>       runtime identifier for the NativeAOT publish
#                            output path (default win-x64; only meaningful
#                            with --csharp-aot).
#          --web             also write the web app crate for a browser build
#                            (implies `--feature rendering`; Rust projects only).
#          [project_path]    workspace-relative path to the project directory
#                            (e.g. devops/tests/projects/native). When omitted, the
#                            PROJECT_PATH environment variable is used - the
#                            same resolution the host uses at startup.
#
# EXAMPLE USAGE:
#   python devops/tools/generate_shipping_bundle.py devops/tests/projects/native
#   python devops/tools/generate_shipping_bundle.py --feature rendering devops/tests/projects/native
#   python devops/tools/generate_shipping_bundle.py devops/tests/projects/managed
#   python devops/tools/generate_shipping_bundle.py --web devops/tests/projects/scene
#   python devops/tools/generate_shipping_bundle.py --restore-stub
#   set PROJECT_PATH=devops/tests/projects/native
#   python devops/tools/generate_shipping_bundle.py

# --- SCRIPT ---

# Standard library
import os
import sys
import tomllib
from pathlib import Path

# Third-party
import yaml

# The generated crate's name and its location relative to the REPOSITORY root.
# The location is shared by every project, so `pill_standalone`'s manifest
# path stays stable; only the contents change per project.
BUNDLE_CRATE_NAME = "pill_shipping_bundle"
BUNDLE_DIRECTORY = Path("build") / BUNDLE_CRATE_NAME

# File names the generator reads and writes.
PROJECT_SETTINGS_FILE_NAME = "project_settings.yaml"
PROJECT_MANIFEST_FILE_NAME = "Cargo.toml"
# Where a native project's sources live; the generator scans them for the
# `register` half of the project's entry point.
PROJECT_SOURCE_DIRECTORY_NAME = "src"
EXTENSION_DIRECTORY = Path("modules") / "extensions"
# Extension crates live under each extension folder's `modules/` folder: the
# runtime crate in `modules/runtime` (for the master renderer, one crate per
# child of `modules/runtime`) and the editor crate in `modules/editor`.
# Suffix of an extension's editor crate, which no shipped build ever links.
EDITOR_CRATE_SUFFIX = "_editor"
# An editor build also links the editor's own modules - the crates directly
# under `modules/editor/` tagged for its frontend (`editor_frontend` under
# `[package.metadata.pill]`), the shell among them, found the way the host's
# `resolve_editor_modules` finds them - and compiles the UI canvas's element id
# into the web app. `generated/` beside them holds the host's wrappers.
EDITOR_DIRECTORY = Path("modules") / "editor"
EDITOR_GENERATED_DIRECTORY_NAME = "generated"
EDITOR_FRONTENDS = ("egui",)
DEFAULT_EDITOR_FRONTEND = "egui"
EDITOR_UI_CANVAS_ID = "pill-editor-ui"
# The crates a bundle names: the runtime always (the static-link types), and the
# C# bridge for a managed project (its external project backend). Never the
# development host - a shipping build does not compile it at all.
RUNTIME_CRATE_DIRECTORY = Path("modules") / "hosting" / "pill_player"
CSHARP_BRIDGE_CRATE_DIRECTORY = Path("modules") / "csharp" / "pill_csharp_bridge"
# The renderer, chosen by the project's `renderer:` setting exactly as the host
# chooses it (`pill_development_host::config`): absent means the master renderer, `none`
# means no renderer. Its data crate, `<renderer>_data`, is linked as the first
# static module in every shipping build, headless included; a windowed build
# also links the GPU crate `<renderer>` and hands the host its entry points as
# `StaticRenderer`.
DEFAULT_RENDERER = "pill_master_renderer"
NO_RENDERER = "none"
RENDERER_DATA_SUFFIX = "_data"
# The requested feature that makes a shipping build windowed.
RENDERING_FEATURE = "rendering"
# The project's asset directory, packed into the binary when it exists, and the
# crate whose build-time writer packs it.
ASSETS_DIRECTORY_NAME = "res"
# The directories a module ships beside its code and declares as its asset
# roots at runtime (`declare_module_root`): its `res`, which also holds the
# shader tree its build script cooks. A packed build has no module directory
# to declare, so the pack carries the files under the same root-relative
# names.
MODULE_ROOT_DIRECTORY_NAMES = (ASSETS_DIRECTORY_NAME,)
ASSETS_CRATE_DIRECTORY = Path("modules") / "assets" / "pill_assets"
ASSET_PACK_FILE_NAME = "assets.pillpack"
# Where a web bundle's build script packs it: beside the crate, because a
# browser fetches it and must not carry it inside the module.
WEB_PACK_DIRECTORY = "pack"
BUILD_SCRIPT_FILE_NAME = "build.rs"
# The web app crate `--web` writes, the frontend it runs on, and the id of the
# canvas its page provides (`devops/tools/web/index.html`).
WEB_APP_CRATE_NAME = "pill_web_app"
WEB_APP_DIRECTORY = Path("build") / WEB_APP_CRATE_NAME
WEB_CRATE_DIRECTORY = Path("modules") / "frontends" / "pill_web"
# The browser editor frontend an `--editor` build links: the scene and the
# shell's UI on two stacked canvases, the counterpart of `pill_editor`.
WEB_EDITOR_CRATE_DIRECTORY = Path("modules") / "editor" / "pill_web_editor"
WEB_CANVAS_ID = "pill-canvas"

# Managed (C#) project constants, mirroring `pill_development_host::config` so a generated
# bundle resolves assemblies exactly where `dotnet build` produced them. The
# host derives the same four values in `csharp_from_manifest`; a shipping build
# has no project path to read, so the generator states them instead.
CSHARP_RUNTIME_ASSEMBLY_NAME = "csharp_runtime"
CSHARP_RUNTIME_OUTPUT_SUBDIRECTORY = "csharp/pill_csharp_runtime/bin/Release/net8.0"
CSHARP_TARGET_FRAMEWORK = "net8.0"
# The engine workspace root, against which the managed config's output
# subdirectories are resolved (the workspace manifest globs `modules/*`).
WORKSPACE_DIRECTORY = Path("modules")


# Folders inside an extension folder, and inside its `modules/` folder.
EXTENSION_MODULES_DIRECTORY = "modules"
EXTENSION_RUNTIME_DIRECTORY = "runtime"
EXTENSION_EDITOR_DIRECTORY = "editor"


# Returns the package name `directory/Cargo.toml` declares, or None when the
# directory holds no readable manifest or the manifest declares no package.
def declared_package_name(directory: Path):
    manifest_path = directory / PROJECT_MANIFEST_FILE_NAME
    if not manifest_path.is_file():
        return None
    try:
        with manifest_path.open("rb") as handle:
            manifest = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError):
        return None
    package_name = manifest.get("package", {}).get("name")
    return package_name if isinstance(package_name, str) else None


# Yields every extension folder under `extensions_root`, in path order: a
# directory holding a `modules/` folder is an extension folder itself, and any
# other directory is a group whose children are extension folders. Folders
# starting with `_` are workspace bookkeeping and skipped at both levels.
def extension_folders(extensions_root: Path):
    for folder in sorted(extensions_root.iterdir()):
        if not folder.is_dir() or folder.name.startswith("_"):
            continue
        if (folder / EXTENSION_MODULES_DIRECTORY).is_dir():
            yield folder
            continue
        for child in sorted(folder.iterdir()):
            if child.is_dir() and not child.name.startswith("_"):
                yield child


# Yields every extension crate directory, the way the host's index finds them:
# `modules/runtime` (or one crate per child of it, for the master renderer)
# and `modules/editor` inside every extension folder, a group folder's children
# included.
def extension_crate_directories(root: Path):
    extensions_root = root / EXTENSION_DIRECTORY
    if not extensions_root.is_dir():
        return
    for folder in extension_folders(extensions_root):
        modules_directory = folder / EXTENSION_MODULES_DIRECTORY
        runtime_directory = modules_directory / EXTENSION_RUNTIME_DIRECTORY
        if declared_package_name(runtime_directory) is not None:
            yield runtime_directory
        elif runtime_directory.is_dir():
            for crate_directory in sorted(runtime_directory.iterdir()):
                if crate_directory.is_dir() and declared_package_name(crate_directory) is not None:
                    yield crate_directory
        editor_directory = modules_directory / EXTENSION_EDITOR_DIRECTORY
        if declared_package_name(editor_directory) is not None:
            yield editor_directory


# Returns the editor frontend `directory/Cargo.toml` tags itself for
# (`editor_frontend` under `[package.metadata.pill]`), or None when the
# directory holds no readable manifest or the manifest carries no tag.
def declared_editor_frontend(directory: Path):
    manifest_path = directory / PROJECT_MANIFEST_FILE_NAME
    if not manifest_path.is_file():
        return None
    try:
        with manifest_path.open("rb") as handle:
            manifest = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError):
        return None
    pill_metadata = manifest.get("package", {}).get("metadata", {}).get("pill", {})
    editor_frontend = pill_metadata.get("editor_frontend")
    return editor_frontend if isinstance(editor_frontend, str) else None


# Yields the directory of every editor module, in directory name order: each
# crate directly under `modules/editor/` whose manifest tags `editor_frontend`
# (any frontend when it is None). The executables and the shared core carry no
# tag, and the host's `generated/` wrappers are never searched.
def editor_module_directories(root: Path, editor_frontend=None):
    editor_root = root / EDITOR_DIRECTORY
    if not editor_root.is_dir():
        return
    for directory in sorted(editor_root.iterdir()):
        if not directory.is_dir() or directory.name == EDITOR_GENERATED_DIRECTORY_NAME:
            continue
        if declared_package_name(directory) is None:
            continue
        frontend_tag = declared_editor_frontend(directory)
        if frontend_tag is None:
            continue
        if editor_frontend is None or frontend_tag == editor_frontend:
            yield directory


# Finds the directory of the module crate whose manifest declares
# `crate_name`: an extension crate, the way the host's `find_extension_crate`
# does, or one of the editor's own modules. Falls back to the conventional
# `modules/extensions/<crate>/modules/runtime` when no folder holds the crate,
# so a missing crate is reported as missing.
def module_crate_directory(root: Path, crate_name: str) -> Path:
    conventional_directory = (
        root
        / EXTENSION_DIRECTORY
        / crate_name
        / EXTENSION_MODULES_DIRECTORY
        / EXTENSION_RUNTIME_DIRECTORY
    )
    for crate_directory in extension_crate_directories(root):
        if declared_package_name(crate_directory) == crate_name:
            return crate_directory
    for crate_directory in editor_module_directories(root):
        if declared_package_name(crate_directory) == crate_name:
            return crate_directory
    return conventional_directory


def repository_root() -> Path:
    """Returns the repository root, three levels above this script."""
    return Path(__file__).resolve().parent.parent.parent


def load_module_list(project_root: Path) -> list:
    """Loads the extension list from the project's settings file.

    Raises FileNotFoundError when the settings file is missing.
    """
    settings_path = project_root / PROJECT_SETTINGS_FILE_NAME
    if not settings_path.is_file():
        raise FileNotFoundError(
            f"no {PROJECT_SETTINGS_FILE_NAME} in project root {project_root}"
        )
    with settings_path.open(encoding="utf-8") as handle:
        data = yaml.safe_load(handle) or {}
    modules = data.get("modules") or []
    return [str(name) for name in modules]


def load_renderer(project_root: Path):
    """Returns the renderer the project selects, or None for `renderer: none`.

    Reads the settings file's `renderer:` key; absent means DEFAULT_RENDERER.
    """
    settings_path = project_root / PROJECT_SETTINGS_FILE_NAME
    with settings_path.open(encoding="utf-8") as handle:
        data = yaml.safe_load(handle) or {}
    name = str(data.get("renderer") or DEFAULT_RENDERER).strip()
    return None if name == NO_RENDERER else name


# The log levels the `logging:` section accepts, as `pill_player::LoggingSettings`
# reads them.
LOG_LEVELS = {"off", "error", "warn", "info", "debug", "trace"}

# The timestamp formats `logging: timestamp:` accepts, as
# `pill_core::telemetry::TimestampFormat` reads them.
TIMESTAMP_FORMATS = {"date_time", "time"}


def check_log_level(settings_path: Path, where: str, level) -> str:
    """Returns `level` lowercased, or raises ValueError naming `where`."""
    text = str(level).strip().lower()
    if text not in LOG_LEVELS:
        raise ValueError(
            f"{settings_path}: `logging:` {where} has level `{level}`; use "
            "off, error, warn, info, debug or trace"
        )
    return text


def load_logging(project_root: Path):
    """Loads the `logging:` section as
    (level, timestamp, source_location, [(target, level), ...]).

    `level`, `timestamp` and `source_location` are None when the section does
    not set them.

    Validated here exactly as the development host validates it, so the
    generated bundle only ever carries settings the runtime can read. Raises
    ValueError on an unknown key, level, timestamp format, or target.
    """
    settings_path = project_root / PROJECT_SETTINGS_FILE_NAME
    with settings_path.open(encoding="utf-8") as handle:
        data = yaml.safe_load(handle) or {}
    section = data.get("logging") or {}
    if not isinstance(section, dict):
        raise ValueError(f"{settings_path}: `logging:` must be a mapping")
    unknown = set(section) - {"level", "timestamp", "source_location", "targets"}
    if unknown:
        raise ValueError(f"{settings_path}: `logging:` has unknown keys {sorted(unknown)}")
    level = section.get("level")
    level = None if level is None else check_log_level(settings_path, "`level`", level)
    timestamp = section.get("timestamp")
    if timestamp is not None:
        timestamp = str(timestamp).strip().lower()
        if timestamp not in TIMESTAMP_FORMATS:
            raise ValueError(
                f"{settings_path}: `logging:` has timestamp `{section['timestamp']}`; "
                "use date_time or time"
            )
    source_location = section.get("source_location")
    if source_location is not None and not isinstance(source_location, bool):
        raise ValueError(
            f"{settings_path}: `logging:` has source_location `{source_location}`; "
            "use true or false"
        )
    targets = []
    for target, target_level in (section.get("targets") or {}).items():
        target = str(target)
        if not target or not all(c.isascii() and (c.isalnum() or c in "_-.:") for c in target):
            raise ValueError(f"{settings_path}: `logging:` target `{target}` is not a log target")
        targets.append((target, check_log_level(settings_path, f"target `{target}`", target_level)))
    # The host reads the targets into a sorted map; the bundle keeps that order.
    targets.sort()
    return level, timestamp, source_location, targets


def load_debug_draw(project_root: Path) -> bool:
    """Loads the `debug_draw:` key: whether the build starts with debug
    drawing on. Absent means off. Raises ValueError on anything but a bool.
    """
    settings_path = project_root / PROJECT_SETTINGS_FILE_NAME
    with settings_path.open(encoding="utf-8") as handle:
        data = yaml.safe_load(handle) or {}
    value = data.get("debug_draw", False)
    if value is None:
        return False
    if not isinstance(value, bool):
        raise ValueError(f"{settings_path}: `debug_draw:` is `{value}`; use true or false")
    return value


def load_startup_scene(project_root: Path) -> str:
    """Loads the `startup_scene:` key: the `.scene` the build loads before the
    project's `init` seeds anything. Absent means none. Raises ValueError when
    the value is not a relative `.scene` path below the asset directory.
    """
    settings_path = project_root / PROJECT_SETTINGS_FILE_NAME
    with settings_path.open(encoding="utf-8") as handle:
        data = yaml.safe_load(handle) or {}
    value = data.get("startup_scene")
    if value is None:
        return ""
    text = str(value)
    if not text.endswith(".scene") or text.startswith(("/", "\\")) or ".." in text:
        raise ValueError(
            f"{settings_path}: `startup_scene:` is `{value}`; use a relative path to a .scene file"
        )
    return text


def build_logging_literal(logging) -> str:
    """Renders the `StaticLogging` expression for the loaded `logging:` section."""
    level, timestamp, source_location, targets = logging
    if level is None and timestamp is None and source_location is None and not targets:
        return "StaticLogging::NONE"
    level_text = "None" if level is None else f'Some("{level}")'
    timestamp_text = "None" if timestamp is None else f'Some("{timestamp}")'
    location_text = "None" if source_location is None else f"Some({str(source_location).lower()})"
    pairs = ", ".join(f'("{target}", "{target_level}")' for target, target_level in targets)
    return (
        f"StaticLogging {{ level: {level_text}, timestamp: {timestamp_text}, "
        f"source_location: {location_text}, targets: &[{pairs}] }}"
    )


def modules_with_renderer_data(renderer, modules: list) -> list:
    """Puts the renderer's data crate first in the module list, once."""
    if renderer is None:
        return list(modules)
    data = renderer + RENDERER_DATA_SUFFIX
    return [data] + [name for name in modules if name.strip() != data]


def module_asset_roots(root: Path, modules: list) -> list:
    """Returns the asset roots every linked module ships, in load order.

    `modules` is the final static module list (the renderer's data crate first)
    plus any other linked module. Only directories that exist are roots: a
    module that ships nothing beside its code contributes none.
    """
    roots = []
    for name in modules:
        crate_directory = module_crate_directory(root, name)
        for directory_name in MODULE_ROOT_DIRECTORY_NAMES:
            candidate = crate_directory / directory_name
            if candidate.is_dir():
                roots.append(candidate)
    return roots


def load_project_name(project_root: Path) -> str:
    """Loads the required project `name` from the settings file.

    The name drives the artifact filename and the window title, so a settings
    file without one is an error, not a silent default.

    Raises FileNotFoundError when the settings file is missing, ValueError when
    it does not declare a `name`.
    """
    settings_path = project_root / PROJECT_SETTINGS_FILE_NAME
    if not settings_path.is_file():
        raise FileNotFoundError(
            f"no {PROJECT_SETTINGS_FILE_NAME} in project root {project_root}"
        )
    with settings_path.open(encoding="utf-8") as handle:
        data = yaml.safe_load(handle) or {}
    name = str(data.get("name") or "").strip()
    if not name:
        raise ValueError(
            f"{settings_path} must declare a `name` (used for the window title)"
        )
    return name


def load_build_binary_name(project_root: Path) -> str:
    """Loads the required `build_binary_name` from the settings file.

    It is the artifact file base, so it must contain only letters, digits and
    underscores (no spaces or special characters). Raises FileNotFoundError
    when the settings file is missing, ValueError when it is missing/invalid.
    """
    settings_path = project_root / PROJECT_SETTINGS_FILE_NAME
    if not settings_path.is_file():
        raise FileNotFoundError(
            f"no {PROJECT_SETTINGS_FILE_NAME} in project root {project_root}"
        )
    with settings_path.open(encoding="utf-8") as handle:
        data = yaml.safe_load(handle) or {}
    value = str(data.get("build_binary_name") or "").strip()
    if not value or not all(
        c.isascii() and (c.isalnum() or c == "_") for c in value
    ):
        raise ValueError(
            f"{settings_path} must declare a `build_binary_name` with only "
            "letters, digits and underscores (no spaces or special characters)"
        )
    return value


def load_project_package_name(project_root: Path) -> str:
    """Reads the project package name from its Cargo.toml.

    The manifest is scanned line-by-line for the `[package]` table's `name`,
    which is all the generator needs; no TOML parser is required.
    """
    manifest_path = project_root / PROJECT_MANIFEST_FILE_NAME
    if not manifest_path.is_file():
        raise FileNotFoundError(
            f"no {PROJECT_MANIFEST_FILE_NAME} in project root {project_root}"
        )
    in_package_table = False
    for line in manifest_path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_package_table = stripped == "[package]"
            continue
        if in_package_table and stripped.startswith("name"):
            value = stripped.split("=", 1)[1].strip().strip('"').strip("'")
            return value
    raise ValueError(f"no package name in {manifest_path}")


def project_declares_register(project_root: Path) -> bool:
    """Whether the project exports a `register` half of its entry point.

    `#[pill_project(register = register)]` names a `pub fn register` in the
    project's own sources; a project without one seeds everything from its
    `init`. The generated bundle points the static backend at that function
    only when it exists, and at the player's no-op otherwise. Scanned rather
    than compiled because the generator must not build the project to
    describe it.
    """
    sources = project_root / PROJECT_SOURCE_DIRECTORY_NAME
    if not sources.is_dir():
        return False
    for source in sources.rglob("*.rs"):
        if "pub fn register(" in source.read_text(encoding="utf-8", errors="replace"):
            return True
    return False


def load_project_features(project_root: Path) -> set:
    """Reads the feature names the project declares in its Cargo.toml [features].

    The bundle can enable a project feature only when the project declares it,
    so the generator filters the requested names against this table. Parsed
    line-by-line like the package name; no TOML parser is required. A managed
    project declares no cargo features, so this is only called for native ones.
    """
    manifest_path = project_root / PROJECT_MANIFEST_FILE_NAME
    if not manifest_path.is_file():
        raise FileNotFoundError(
            f"no {PROJECT_MANIFEST_FILE_NAME} in project root {project_root}"
        )
    in_features_table = False
    features = set()
    for line in manifest_path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_features_table = stripped == "[features]"
            continue
        if in_features_table and stripped and not stripped.startswith("#"):
            name = stripped.split("=", 1)[0].strip().strip('"').strip("'")
            if name:
                features.add(name)
    return features


def find_csproj_manifest(project_root: Path) -> Path:
    """Locates the single `.csproj` manifest inside a project directory.

    Raises ValueError when the directory contains no `.csproj` file.
    """
    matches = sorted(project_root.glob("*.csproj"))
    if not matches:
        raise ValueError(f"no .csproj file found in project root {project_root}")
    return matches[0]


def project_kind(project_root: Path) -> str:
    """Returns 'native' for a Cargo crate, 'managed' for a dotnet-built project.

    A project declares its scripting language by its manifest: a `Cargo.toml`
    is native, a `*.csproj` is managed. Anything else is an error, because the
    generator cannot name a project it cannot build.
    """
    if (project_root / PROJECT_MANIFEST_FILE_NAME).is_file():
        return "native"
    if find_csproj_manifest(project_root):
        return "managed"
    raise ValueError(
        f"project root {project_root} has neither {PROJECT_MANIFEST_FILE_NAME} "
        "nor a .csproj file; a shipping project must be one or the other"
    )


def manifest_relative_path(from_directory: Path, target: Path) -> str:
    """Computes a Cargo `path` dependency relative to `from_directory`, using /."""
    return os.path.relpath(target, from_directory).replace(os.sep, "/")


def build_cargo_manifest(
    bundle_directory: Path,
    project_root: Path,
    package_name: str,
    modules: list,
    project_features: set,
    root: Path,
    managed: bool = False,
    renderer=None,
    packs_assets: bool = False,
) -> str:
    """Builds the generated bundle's Cargo.toml text.

    `renderer` is the GPU crate a windowed build links, or None.
    `packs_assets` adds the asset packer the bundle's build script runs.
    """
    lines = [
        "[package]",
        f'name = "{BUNDLE_CRATE_NAME}"',
        'version = "0.0.0"',
        'edition = "2021"',
        # Without assets there is no build script. Saying so explicitly makes
        # cargo ignore a `build.rs` left behind by an earlier project's bundle
        # (it is gitignored, so restoring the stub does not remove it).
        *([] if packs_assets else ["build = false"]),
        "",
        "[dependencies]",
        # The runtime, so the table can name StaticModule / StaticProjectBackend.
        "pill_player = { path = "
        f'"{manifest_relative_path(bundle_directory, root / RUNTIME_CRATE_DIRECTORY)}" }}',
    ]
    if managed:
        # The C# bridge, whose backend starts the managed project.
        lines.append(
            "pill_csharp_bridge = { path = "
            f'"{manifest_relative_path(bundle_directory, root / CSHARP_BRIDGE_CRATE_DIRECTORY)}" }}'
        )
    if not managed:
        # The native project itself, so `project::init` is nameable. Any
        # requested feature the project actually declares is enabled so the
        # static binary matches a dev build; cargo does not propagate the
        # host's features here. `rendering` is not a project feature: the
        # renderer lives outside `pill_engine`, so a windowed ship is requested
        # on the HOST (`pill_standalone/rendering`) and the filter below drops
        # the name here. A managed project is a dotnet
        # assembly, so it has no cargo dependency to declare.
        # The feature list is built outside the f-string: nesting the same
        # quote character inside an f-string expression is only valid on
        # Python 3.12+, and this script supports 3.8+.
        feature_names = ", ".join(f'"{name}"' for name in sorted(project_features))
        feature_clause = f", features = [{feature_names}]" if project_features else ""
        # Keyed by the project's own package name: the generated source names
        # the crate as `<package_name>::init`, so a fixed `project` key only
        # ever worked for a project whose package happens to be called that.
        lines.append(
            f'{package_name} = {{ path = "{manifest_relative_path(bundle_directory, project_root)}"'
            + feature_clause
            + " }",
        )
    for module in modules:
        module_directory = module_crate_directory(root, module)
        relative_path = manifest_relative_path(bundle_directory, module_directory)
        lines.append(
            f'{module} = {{ path = "{relative_path}", default-features = false }}'
        )
    if renderer:
        # The renderer, linked statically: the bundle is the only crate that
        # depends on it for real, and hands the host its two entry points.
        # Nothing loads this binary as a module, so it carries no wrapper.
        renderer_path = manifest_relative_path(
            bundle_directory, module_crate_directory(root, renderer)
        )
        lines.append(
            f'{renderer} = {{ path = "{renderer_path}", default-features = false }}'
        )
    if packs_assets:
        # The build script packs the project's `res` directory with it.
        assets_path = manifest_relative_path(bundle_directory, root / ASSETS_CRATE_DIRECTORY)
        lines += ["", "[build-dependencies]", f'pill_assets = {{ path = "{assets_path}" }}']
    return "\n".join(lines) + "\n"


def build_build_script(
    bundle_directory: Path,
    assets_directory: Path,
    module_roots: list,
    web: bool = False,
) -> str:
    """Builds the bundle's build.rs, which packs the project's `res` and the
    asset roots every linked module ships.

    A native bundle packs into `OUT_DIR`, where `src/lib.rs` embeds it. A web
    bundle cannot embed it - the whole point of the split is that the
    module stays small - so it packs beside the crate instead, where
    `build_web.py` picks the file up and puts it next to the page.

    Every directory is named relative to the bundle, like every dependency
    path, so the generated file does not change with the checkout's location.
    A shader tree among the roots is cooked here before packing: the module's
    own build script cooks the same tree, and cargo does not order two crates'
    build scripts, so a clean build cannot assume the stage files are on disk.
    """
    relative_assets = manifest_relative_path(bundle_directory, assets_directory)
    root_lines = [
        f'        manifest_directory.join("{manifest_relative_path(bundle_directory, root)}"),'
        for root in module_roots
    ]
    if web:
        output_lines = [
            "    // Beside the crate, not in OUT_DIR: the browser fetches it, so it",
            "    // must not be inside the module.",
            f'    let output = manifest_directory.join("{WEB_PACK_DIRECTORY}")',
            f'        .join("{ASSET_PACK_FILE_NAME}");',
            f'    std::fs::create_dir_all(output.parent().expect("{WEB_PACK_DIRECTORY} has a parent"))',
            '        .unwrap_or_else(|error| panic!("cannot create {}: {error}", output.display()));',
        ]
        responsibilities = [
            "//! - Pack the project's `res` and the modules' roots beside this crate,",
            "//!   where `build_web.py` copies them next to the page: a browser build",
            "//!   fetches them instead of embedding them.",
        ]
    else:
        output_lines = [
            '    let output = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"))',
            f'        .join("{ASSET_PACK_FILE_NAME}");',
        ]
        responsibilities = [
            "//! - Pack the project's `res` and the modules' roots into `OUT_DIR`,",
            "//!   where `src/lib.rs` embeds them as the project's asset pack.",
        ]
    lines = [
        "//! Generated shipping bundle build script - do not edit. Regenerated by",
        "//! `devops/tools/generate_shipping_bundle.py`.",
        "//!",
        "//! # Responsibilities",
        "//!",
    ]
    lines += responsibilities
    lines += [
        "",
        "fn main() {",
        "    let manifest_directory = std::path::PathBuf::from(env!(\"CARGO_MANIFEST_DIR\"));",
        f'    let source = manifest_directory.join("{relative_assets}");',
        "    // The directories a linked module ships beside its code: its `res`,",
        "    // shader tree included. A packed build has no module directory left to",
        "    // declare, so the pack carries them under the same relative names.",
        "    let module_roots: Vec<std::path::PathBuf> = vec![",
    ]
    lines += root_lines
    lines += [
        "    ];",
    ]
    lines += output_lines
    lines += [
        "    // The module's build script cooks the same tree; cargo does not order",
        "    // two crates' build scripts, so a clean build cooks here before packing",
        "    // rather than pack a stage file that is not on disk yet.",
        "    // A root without shaders has nothing to cook.",
        "    for root in &module_roots {",
        "        pill_assets::cook_shader_tree(root).unwrap_or_else(|error| {",
        '            panic!("cannot cook shaders at {}: {error}", root.display())',
        "        });",
        "    }",
        "    // A project shader may include a header a module ships, so the",
        "    // modules' include directories join the project's include path after",
        "    // its own.",
        "    let module_include_directories: Vec<std::path::PathBuf> = module_roots",
        "        .iter()",
        "        .flat_map(|root| {",
        "            pill_assets::shader_tree_layout(root)",
        "                .map(|layout| layout.include_directories)",
        "                .unwrap_or_default()",
        "        })",
        "        .collect();",
        "    // The project's own shader sources have no build script either, and the",
        "    // shader importer reads the cooked file rather than the source: cook",
        "    // them beside themselves so the pack carries the same WGSL a",
        "    // development host cooks.",
        "    pill_assets::cook_shader_tree_with_include_directories(&source, &module_include_directories)",
        "        .unwrap_or_else(|error| {",
        '            panic!("cannot cook the project\'s shaders at {}: {error}", source.display())',
        "        });",
        "    let packed = pill_assets::write_asset_pack_with_roots(&source, &module_roots, &output)",
        "        .unwrap_or_else(|error| panic!(\"cannot pack {}: {error}\", source.display()));",
        "    // The directories themselves too, so an added or removed file repacks.",
        "    println!(\"cargo:rerun-if-changed={}\", source.display());",
        "    for root in &module_roots {",
        "        println!(\"cargo:rerun-if-changed={}\", root.display());",
        "    }",
        "    for file in packed {",
        "        println!(\"cargo:rerun-if-changed={}\", file.display());",
        "    }",
        "}",
    ]
    return "\n".join(lines) + "\n"


def build_library_source(
    package_name: str,
    project_name: str,
    modules: list,
    kind: str,
    project_path: str,
    bundle_directory: Path,
    workspace_root: Path,
    aot: bool = False,
    rid: str = "win-x64",
    renderer=None,
    packs_assets: bool = False,
    logging=(None, None, None, []),
    debug_draw: bool = False,
    startup_scene: str = "",
    web: bool = False,
) -> str:
    """Builds the generated bundle's src/lib.rs text.

    `renderer` is the GPU crate a windowed build links, or None.
    `packs_assets` packs the project's `res` directory with the build script.
    `logging` is the validated `logging:` section, from `load_logging`.
    `debug_draw` is the `debug_draw:` key, from `load_debug_draw`.
    `web` emits the browser posture's asset fields: a fetched pack instead of
    an embedded one.
    """
    assets_responsibility = (
        [
            "//! - Fetch the project's asset pack beside the page instead of",
            "//!   embedding it, and describe them as the `StaticProject`",
            "//!   the browser frontends run.",
        ]
        if web
        else []
    )
    lines = [
        "//! Generated shipping bundle - do not edit. Regenerated from",
        "//! the project's `project_settings.yaml` by",
        "//! `devops/tools/generate_shipping_bundle.py`.",
        "//!",
        "//! # Responsibilities",
        "//!",
        "//! - Link the project, its modules and its renderer into one binary, and",
        "//!   describe them as the `StaticProject` the shipping frontends run.",
    ]
    lines += assets_responsibility
    lines += [
        "",
        "use pill_player::{",
        "    StaticLogging, StaticModule, StaticProject, StaticProjectBackend,",
        "    StaticRenderer,",
        "};",
        "",
        "/// Every selected extension: the renderer's data crate first, then",
        "/// `project_settings.yaml` order.",
        # One entry per line regardless of count. rustfmt collapses a
        # single-element array onto one line, so without this the generated
        # file fails `cargo fmt --check` for any project selecting exactly one
        # module - a repository-wide gate failing on output nobody edits.
        "#[rustfmt::skip]",
        "pub const STATIC_MODULES: &[StaticModule] = &[",
    ]
    for module in modules:
        lines += [
            "    StaticModule {",
            f'        name: "{module}",',
            f"        init: {module}::register,",
            "    },",
        ]
    lines += [
        "];",
        "",
        "/// The project backend for this shipping project.",
    ]
    if kind == "managed":
        # The managed backend resolves its assemblies against the engine
        # workspace root, which is where `dotnet build` produced them. The
        # emitted root is that workspace expressed relative to this bundle
        # crate, so no absolute path is ever compiled in.
        workspace_relative_path = manifest_relative_path(bundle_directory, workspace_root)
        if aot:
            # The NativeAOT posture loads one published native library; the
            # project subdirectory points at the `dotnet publish` output.
            project_subdirectory = (
                f"../{project_path}/bin/Release/{CSHARP_TARGET_FRAMEWORK}/{rid}/publish"
            )
            constructor = "native_aot"
        else:
            project_subdirectory = f"../{project_path}/bin/Release/{CSHARP_TARGET_FRAMEWORK}"
            constructor = "coreclr"
        # The runtime names no C# type: the bridge's backend is handed over as
        # an external project backend, which the runtime starts after the
        # extensions and keeps alive beside the engine.
        lines += [
            "pub fn project_backend() -> StaticProjectBackend {",
            "    StaticProjectBackend::External(std::sync::Arc::new(",
            f"        pill_csharp_bridge::CSharpBackend::{constructor}(",
            "            pill_csharp_bridge::CSharpModuleConfig::new(",
            f'                "{CSHARP_RUNTIME_ASSEMBLY_NAME}",',
            f'                "{CSHARP_RUNTIME_OUTPUT_SUBDIRECTORY}",',
            f'                "{package_name}",',
            f'                "{project_subdirectory}",',
            "            ),",
            f'            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("{workspace_relative_path}"),',
            "        ),",
            "    ))",
            "}",
        ]
    else:
        # Skipped for the same reason as the module list: rustfmt joins this
        # literal onto one line when the package name is short, so the shape
        # would depend on the project's name. A project without a `register`
        # half points the static backend at the player's no-op, which is the
        # only thing a project that seeds everything from `init` can mean.
        project_root = (bundle_directory.parent.parent / project_path).resolve()
        register_expression = (
            f"{package_name}::register"
            if project_declares_register(project_root)
            else "pill_player::no_project_registration"
        )
        lines += [
            "#[rustfmt::skip]",
            "pub fn project_backend() -> StaticProjectBackend {",
            "    StaticProjectBackend::Native {",
            f"        register: {register_expression},",
            f"        init: {package_name}::init,",
            "    }",
            "}",
        ]
    lines += [
        "",
        "/// The renderer this binary links, or `None` for a headless build.",
        "pub fn static_renderer() -> Option<StaticRenderer> {",
    ]
    if renderer:
        lines += [
            "    Some(StaticRenderer {",
            f"        init: {renderer}::register,",
            f"        attach: {renderer}::attach,",
            "    })",
        ]
    else:
        lines += ["    None"]
    lines += [
        "}",
        "",
        "/// The `logging:` section of `project_settings.yaml`.",
        # Skipped for the same reason as the module list: one generated line,
        # whatever its length, rather than whatever shape rustfmt would pick.
        "#[rustfmt::skip]",
        f"const LOGGING: StaticLogging = {build_logging_literal(logging)};",
        "",
    ]
    lines += [
        "",
        "/// The complete shipping project: modules first, then the project.",
        "pub fn static_project() -> StaticProject {",
        "    StaticProject {",
        # The settings display name, not the crate name: this is what the host
        # logs and what the window title carries.
        f'        name: "{project_name}",',
        "        backend: project_backend(),",
        "        modules: STATIC_MODULES,",
        "        renderer: static_renderer(),",
        (
            "        asset_pack: None,"
            if web
            else (
                f'        asset_pack: Some(include_bytes!(concat!(env!("OUT_DIR"), "/{ASSET_PACK_FILE_NAME}"))),'
                if packs_assets
                else "        asset_pack: None,"
            )
        ),
        (
            f'        asset_pack_url: Some("{ASSET_PACK_FILE_NAME}"),'
            if web and packs_assets
            else "        asset_pack_url: None,"
        ),
        "        logging: LOGGING,",
        f"        debug_draw: {'true' if debug_draw else 'false'},",
        (
            f'        startup_scene: Some("{startup_scene}"),'
            if startup_scene
            else "        startup_scene: None,"
        ),
        "    }",
        "}",
    ]
    return "\n".join(lines) + "\n"


def build_web_app_manifest(
    web_app_directory: Path, root: Path, editor: bool = False
) -> str:
    """Builds the web app crate's Cargo.toml text.

    An editor build links the browser editor frontend (`pill_web_editor`); a
    game build links the browser frontend (`pill_web`), so a game page never
    carries editor code. The empty `[workspace]` makes the crate its own
    workspace root: the wasm build resolves features for this crate alone, and
    cargo does not look for an enclosing workspace.
    """
    frontend_directory = WEB_EDITOR_CRATE_DIRECTORY if editor else WEB_CRATE_DIRECTORY
    frontend_path = manifest_relative_path(web_app_directory, root / frontend_directory)
    bundle_path = manifest_relative_path(web_app_directory, root / BUNDLE_DIRECTORY)
    frontend_dependency = (
        f'pill_web_editor = {{ path = "{frontend_path}" }}'
        if editor
        else f'pill_web = {{ path = "{frontend_path}" }}'
    )
    lines = [
        "[package]",
        f'name = "{WEB_APP_CRATE_NAME}"',
        'version = "0.0.0"',
        'edition = "2021"',
        "",
        "[lib]",
        'crate-type = ["cdylib"]',
        "",
        "[dependencies]",
        frontend_dependency,
        f'{BUNDLE_CRATE_NAME} = {{ path = "{bundle_path}" }}',
        'wasm-bindgen = "0.2"',
        "",
        "# Its own workspace: nothing of the engine workspace's native builds takes",
        "# part in this one.",
        "[workspace]",
    ]
    return "\n".join(lines) + "\n"


def build_web_app_source(
    editor: bool = False, editor_frontend: str = DEFAULT_EDITOR_FRONTEND
) -> str:
    """Builds the web app crate's src/lib.rs: the browser entry point.

    `editor` emits the editor posture's entry point: the shell draws on a
    second canvas above the scene's. `editor_frontend` names the runner, and
    the only runner the page accepts is the egui one.
    """
    if editor:
        entry = (
            f"    pill_web_editor::run(\n"
            f"        {BUNDLE_CRATE_NAME}::static_project(),\n"
            f'        "{WEB_CANVAS_ID}",\n'
            f'        "{EDITOR_UI_CANVAS_ID}",\n'
            "    );"
        )
        responsibilities = (
            "//! - Run the shipping bundle's project as the egui editor in the\n"
            "//!   page's canvases when the browser has instantiated\n"
            "//!   this module: the scene below, the egui shell's UI above."
        )
    else:
        entry = (
            f'    pill_web::run({BUNDLE_CRATE_NAME}::static_project(), "{WEB_CANVAS_ID}");'
        )
        responsibilities = (
            "//! - Run the shipping bundle's project in the page's canvas when the\n"
            "//!   browser has instantiated this module."
        )
    lines = [
        "//! Generated web app - do not edit. Regenerated by",
        "//! `devops/tools/generate_shipping_bundle.py --web`.",
        "//!",
        "//! # Responsibilities",
        "//!",
        responsibilities,
        "",
        "use wasm_bindgen::prelude::wasm_bindgen;",
        "",
        "/// The entry point the browser calls once the module is instantiated.",
        "#[wasm_bindgen(start)]",
        "pub fn start() {",
        entry,
        "}",
    ]
    return "\n".join(lines) + "\n"


def write_if_changed(path: Path, content: str) -> bool:
    """Writes content only when it differs; returns True when written."""
    if path.is_file() and path.read_text(encoding="utf-8") == content:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8", newline="")
    return True


# The tracked bundle files between generations: a crate with no project, no
# module table and no dependencies, whose only job is to keep
# `pill_standalone`'s optional path dependency resolvable on a fresh clone, and
# to refuse to compile so nothing type-checks against it by accident.
#
# A generation is one project's build artifact, so it is never committed:
# `--restore-stub` puts these bytes back, the gate runs it after its shipping
# clippy step, and `devops/tests/test_shipping_smoke.py` refuses a committed
# generation before it builds anything.
STUB_MANIFEST = """\
# The committed face of the shipping bundle: a crate with no dependencies that
# exists so `pill_standalone`'s optional path dependency resolves on a fresh
# clone. `devops/tools/generate_shipping_bundle.py` rewrites this manifest and
# `src/lib.rs` for the project being shipped; a real generation is never
# committed, because it would let a shipping build compile against another
# project. `build = false` ignores a `build.rs` that an earlier generation left
# behind (that file is gitignored, so restoring the stub does not remove it).
[package]
name = "pill_shipping_bundle"
version = "0.0.0"
edition = "2021"
build = false
"""

STUB_LIBRARY_SOURCE = """\
//! The shipping bundle's committed stub: no project, no modules, no renderer.
//!
//! # Responsibilities
//!
//! - Keep `pill_standalone`'s optional path dependency resolvable on a fresh
//!   clone, which is the only reason this crate is committed at all.
//! - Refuse to compile, so nothing can be type-checked or shipped against the
//!   stub in place of a real bundle.
//!
//! # Design
//!
//! `devops/tools/generate_shipping_bundle.py` rewrites this file and the
//! manifest beside it for the project named on its command line: the generated
//! source exports the module table, the renderer choice and the project
//! accessor the `shipping` posture of `pill_standalone` calls, and the
//! manifest gains the dependencies those names need. The stub exports none of
//! them. `--restore-stub` writes these bytes back once a generation is done.
//!
//! A committed generation would be worse than this refusal: it compiles, so
//! `cargo clippy -p pill_standalone --features shipping` and a shipped binary
//! would both type-check against whichever project happened to be generated
//! last. The generator's output is a build artifact of one project, never the
//! tree's state; `devops/tests/test_shipping_smoke.py` asserts the committed
//! files are this stub, and the gate runs that assertion before anything
//! generates a bundle.

compile_error!(
    "this is the project-agnostic shipping bundle stub; run \\
     `python devops/tools/generate_shipping_bundle.py <project>` before building the \\
     shipping posture"
);
"""


def restore_stub(bundle_directory: Path) -> int:
    """Restores the tracked bundle files to the project-agnostic stub.

    The build script belongs to a generation (it packs that project's assets),
    so it goes too; the stub's `build = false` would ignore it, but leaving
    another project's packer in the tree is exactly the state this undoes.
    """
    wrote_manifest = write_if_changed(
        bundle_directory / PROJECT_MANIFEST_FILE_NAME, STUB_MANIFEST
    )
    wrote_source = write_if_changed(
        bundle_directory / "src" / "lib.rs", STUB_LIBRARY_SOURCE
    )
    build_script_path = bundle_directory / BUILD_SCRIPT_FILE_NAME
    removed_build_script = build_script_path.is_file()
    if removed_build_script:
        build_script_path.unlink()
    changed = wrote_manifest or wrote_source or removed_build_script
    print(f"shipping bundle stub: {bundle_directory}")
    print("  restored (changed)" if changed else "  already the stub (unchanged)")
    return 0


def main() -> int:
    """Generates the shipping bundle crate for the given project path.

    The project path comes from the command-line argument, or from the
    PROJECT_PATH environment variable when no argument is given - the same
    resolution the host uses at startup.
    """
    # Parse `--feature <name>` (repeatable), `--csharp-aot`, `--rid <rid>`,
    # `--web`, `--editor`, `--editor-frontend <name>`, and the optional
    # project path.
    arguments = sys.argv[1:]
    requested_features = []
    aot = False
    web = False
    editor = False
    restore = False
    editor_frontend = DEFAULT_EDITOR_FRONTEND
    rid = "win-x64"
    positional = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == "--feature":
            index += 1
            if index >= len(arguments):
                print("error: --feature requires a value", file=sys.stderr)
                return 2
            requested_features.append(arguments[index])
        elif argument.startswith("--feature="):
            requested_features.append(argument[len("--feature=") :])
        elif argument == "--restore-stub":
            # Undo a generation: the tracked files go back to the stub the gate
            # and a fresh clone expect. It names no project, so everything else
            # on the line is refused rather than silently ignored.
            restore = True
        elif argument == "--csharp-aot":
            aot = True
        elif argument == "--web":
            # A browser build always draws: it links the renderer.
            web = True
            requested_features.append(RENDERING_FEATURE)
        elif argument == "--editor":
            # The browser editor posture: the bundle also links a shell.
            editor = True
        elif argument == "--editor-frontend":
            index += 1
            if index >= len(arguments):
                print("error: --editor-frontend requires a value", file=sys.stderr)
                return 2
            editor_frontend = arguments[index]
        elif argument.startswith("--editor-frontend="):
            editor_frontend = argument[len("--editor-frontend=") :]
        elif argument == "--rid":
            index += 1
            if index >= len(arguments):
                print("error: --rid requires a value", file=sys.stderr)
                return 2
            rid = arguments[index]
        elif argument.startswith("--rid="):
            rid = argument[len("--rid=") :]
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
            "usage: generate_shipping_bundle.py [--feature <name>...] [project_path]",
            file=sys.stderr,
        )
        return 2
    # Restoring names no project, so it refuses a project path: a line that
    # looks like a generation must not quietly restore the stub instead.
    if restore:
        if positional:
            print(
                "error: --restore-stub takes no project path; it puts the "
                "committed stub back",
                file=sys.stderr,
            )
            return 2
        return restore_stub(repository_root() / BUNDLE_DIRECTORY)
    project_path = positional[0] if positional else os.environ.get("PROJECT_PATH", "")
    if not project_path:
        print(
            "error: no project path: pass it as an argument or set PROJECT_PATH "
            "(e.g. PROJECT_PATH=devops/tests/projects/native).",
            file=sys.stderr,
        )
        return 1

    root = repository_root()
    # The host resolves PROJECT_PATH against the working directory, so
    # `../devops/tests/projects/native` works from the workspace dir; the generator
    # historically resolved it against the repository root, so
    # `devops/tests/projects/native` works from anywhere. Try the working directory
    # first, then the repository root, so both spellings work.
    cwd_candidate = (Path.cwd() / project_path).resolve()
    project_root = (
        cwd_candidate if cwd_candidate.is_dir() else (root / project_path).resolve()
    )
    if not project_root.is_dir():
        print(f"error: no project directory at {project_root}", file=sys.stderr)
        return 1
    # Canonicalize to a repository-root-relative path: the bundle locates the
    # project with it, and the managed backend's output subdirectory is derived
    # from it against the engine workspace root. Forward slashes keep the path
    # valid when it is emitted inside generated Rust string literals.
    project_path = os.path.relpath(project_root, root).replace(os.sep, "/")

    # Step 1: read the project's scripting language, module selection, package
    # name, and required display name + artifact binary name. A native project
    # names its crate in Cargo.toml; a managed project names its assembly in
    # the .csproj file stem.
    try:
        kind = project_kind(project_root)
        renderer_name = load_renderer(project_root)
        modules = modules_with_renderer_data(renderer_name, load_module_list(project_root))
        project_name = load_project_name(project_root)
        build_binary_name = load_build_binary_name(project_root)
        if kind == "managed":
            package_name = find_csproj_manifest(project_root).stem
        else:
            package_name = load_project_package_name(project_root)
    except (FileNotFoundError, ValueError, yaml.YAMLError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    if web and kind == "managed":
        print(
            "error: --web supports Rust projects only; a C# project cannot run in "
            f"a browser yet, and {project_path} is a managed (`.csproj`) project",
            file=sys.stderr,
        )
        return 1
    if editor and not web:
        print(
            "error: --editor is the browser editor posture; pass it with --web",
            file=sys.stderr,
        )
        return 1
    if aot and kind != "managed":
        print(
            "error: --csharp-aot requires a managed (`.csproj`) project; "
            f"{project_path} is a native Rust project",
            file=sys.stderr,
        )
        return 1

    # Step 1b: refuse an editor crate. Editor crates (`<extension>_editor`) load
    # only in an editor host, with their extension, and the editor's own
    # modules under `modules/editor/` only in an editor session; a shipped game
    # never links either, and the dev host refuses them in `modules:` too.
    editor_module_names = {
        declared_package_name(directory) for directory in editor_module_directories(root)
    }
    editor_crates = [
        name
        for name in modules
        if name.endswith(EDITOR_CRATE_SUFFIX) or name.strip() in editor_module_names
    ]
    if editor_crates:
        print(
            "error: editor crates cannot ship; remove them from `modules:`: "
            f"{', '.join(editor_crates)}",
            file=sys.stderr,
        )
        return 1
    # Step 1c: an editor build links the editor's modules for its frontend - the
    # shell - beside the runtime modules, and never an extension's editor crate
    # (the retired `pill_ui` posture did, and those crates carried its panel
    # contract). They register last: the shell's own resources exist either way
    # (its `register` tolerates missing panels).
    if editor:
        editor_modules = [
            declared_package_name(directory)
            for directory in editor_module_directories(root, editor_frontend)
        ]
        if not editor_modules:
            print(
                f"error: no editor module under {EDITOR_DIRECTORY} is tagged for the "
                f"`{editor_frontend}` editor frontend",
                file=sys.stderr,
            )
            return 1
        modules = modules + editor_modules

    # Step 2: validate every selected module exists in an extension folder under
    # modules/extensions/, the renderer's data crate included, and the GPU
    # crate a windowed build links.
    linked_renderer = (
        renderer_name if renderer_name and RENDERING_FEATURE in requested_features else None
    )
    missing_modules = [
        name
        for name in modules + ([linked_renderer] if linked_renderer else [])
        if not (module_crate_directory(root, name) / PROJECT_MANIFEST_FILE_NAME).is_file()
    ]
    if missing_modules:
        print(
            f"error: modules not found under {EXTENSION_DIRECTORY}: "
            f"{', '.join(missing_modules)}",
            file=sys.stderr,
        )
        return 1

    # Step 3: keep only the requested project features the project declares, so
    # an unknown or host-only feature is ignored rather than erroring. This is
    # what makes `--feature rendering` still work: it is a host feature now, so
    # it is dropped here and reaches cargo through the host instead. A managed
    # project declares no cargo features, so requested names are never applied.
    declared_features = (
        load_project_features(project_root) if kind == "native" else set()
    )
    project_features = set(requested_features) & declared_features
    if requested_features and not project_features and kind == "native":
        print(
            f"note: none of {sorted(requested_features)} are declared by the "
            "project; linking it without extra features",
            file=sys.stderr,
        )

    # Step 4: render and write the bundle files plus the artifact-name record
    # (content-based, so a stable tree shows no diff). The bundle and the
    # binary-name record land in the shared `<repo>/build/` scratch location,
    # not under the project, so `pill_standalone`'s manifest path stays static.
    bundle_directory = root / BUNDLE_DIRECTORY
    # A project with a `res` ships it packed inside the binary, and every
    # linked module's roots ride the same pack: a shipped build has no module
    # directory to read them from. The pack exists when either side has files.
    assets_directory = project_root / ASSETS_DIRECTORY_NAME
    module_roots = module_asset_roots(
        root, modules + ([linked_renderer] if linked_renderer else [])
    )
    packs_assets = assets_directory.is_dir() or bool(module_roots)
    cargo_manifest = build_cargo_manifest(
        bundle_directory,
        project_root,
        package_name,
        modules,
        project_features,
        root,
        managed=(kind == "managed"),
        renderer=linked_renderer,
        packs_assets=packs_assets,
    )
    library_source = build_library_source(
        package_name,
        project_name,
        modules,
        kind,
        project_path,
        bundle_directory,
        root / WORKSPACE_DIRECTORY,
        aot=aot and kind == "managed",
        rid=rid,
        renderer=linked_renderer,
        packs_assets=packs_assets,
        logging=load_logging(project_root),
        debug_draw=load_debug_draw(project_root),
        startup_scene=load_startup_scene(project_root),
        web=web,
    )
    wrote_manifest = write_if_changed(
        bundle_directory / PROJECT_MANIFEST_FILE_NAME, cargo_manifest
    )
    wrote_source = write_if_changed(bundle_directory / "src" / "lib.rs", library_source)
    # The build script exists only while there is a `res` to pack; a stale one
    # from the previous project would pack the wrong directory.
    build_script_path = bundle_directory / BUILD_SCRIPT_FILE_NAME
    if packs_assets:
        wrote_source |= write_if_changed(
            build_script_path,
            build_build_script(bundle_directory, assets_directory, module_roots, web=web),
        )
    elif build_script_path.is_file():
        build_script_path.unlink()
        wrote_source = True
    project_name_path = root / "build" / "build_meta" / "build_binary_name.txt"
    wrote_name = write_if_changed(project_name_path, build_binary_name + "\n")

    print(f"shipping bundle: {os.path.relpath(bundle_directory, root)}")
    print("  regenerated (changed)" if wrote_manifest or wrote_source else "  unchanged")

    # Step 5: the web app crate, for a browser build.
    if web:
        web_app_directory = root / WEB_APP_DIRECTORY
        wrote_web = write_if_changed(
            web_app_directory / PROJECT_MANIFEST_FILE_NAME,
            build_web_app_manifest(web_app_directory, root, editor),
        )
        wrote_web |= write_if_changed(
            web_app_directory / "src" / "lib.rs",
            build_web_app_source(editor, editor_frontend),
        )
        print(f"web app: {os.path.relpath(web_app_directory, root)}")
        print("  regenerated (changed)" if wrote_web else "  unchanged")
    print(f"build binary name: {build_binary_name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
