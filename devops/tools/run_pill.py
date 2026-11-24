#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+ and PyYAML, and the Rust toolchain on PATH. A
#               `--shipping` run also needs the shipping toolchain
#               (`devops/ci_cd/build_shipping.py`), and a C# project needs
#               the .NET SDK.

# DESCRIPTION: Runs a project: one command that reads the project's settings and
#   starts the host the settings ask for.
#
#   The launcher reads only what decides the command - the modules the project
#   enables, which of them is a renderer, and the build options their entries
#   set - and never rewrites a file. The host owns the generated module
#   wrappers, so a `cargo run` this script starts writes them itself before the
#   first build.
#
#   The environment variable the host reads is `PROJECT_PATH`, so the command it
#   prints sets it: copy the line and it runs the same thing this script would.
#
# USAGE: python devops/tools/run_pill.py <project_path> [--editor] [--headless]
#          [--shipping] [--local_project_settings] [--print-command]
#          <project_path>            the project directory, the one holding
#                                    `project_settings.yaml` (required)
#          --editor                  run the editor instead of the game
#          --headless                run without a window, even when the
#                                    project enables a renderer
#          --shipping                build the project's shipping bundle instead
#                                    of running a development host
#          --local_project_settings  read `project_settings.local.yaml` too; a
#                                    shipping build ignores it unless asked
#          --print-command           print the command and exit without running
#                                    it
#
# EXAMPLE USAGE:
#   python devops/tools/run_pill.py examples/pills/pills_rust
#   python devops/tools/run_pill.py examples/headless/headless_rust --print-command
#   python devops/tools/run_pill.py devops/tests/projects/native --editor
#   python devops/tools/run_pill.py examples/cube/cube_rust --shipping

# --- SCRIPT ---

# Standard library
import os
import subprocess
import sys
from pathlib import Path

# Locations, relative to the repository root.
WORKSPACE_DIRECTORY = Path("modules")
SHIPPING_SCRIPT = Path("devops") / "ci_cd" / "build_shipping.py"
GENERATOR_SCRIPT = Path("devops") / "tools" / "generate_shipping_bundle.py"
# The frontend crates a run starts.
GAME_PACKAGE = "pill_standalone"
EDITOR_PACKAGE = "pill_editor"
# The cargo feature a window needs: the loop, the input and the renderer
# attach live behind it.
RENDERING_FEATURE = "rendering"


def repository_root() -> Path:
    """The repository root, located from this script."""
    return Path(__file__).resolve().parent.parent.parent


def load_generator(root: Path):
    """Imports the bundle generator, whose settings readers this script shares.

    One reader serves the shipping build and this launcher, so a project that
    runs under one runs under the other.
    """
    sys.path.insert(0, str(root / GENERATOR_SCRIPT.parent))
    try:
        import generate_shipping_bundle  # noqa: E402 - the path is set above
    finally:
        sys.path.pop(0)
    return generate_shipping_bundle


def parse_arguments(argv):
    """Splits the command line into the project path and the flags."""
    flags = {
        "editor": False,
        "headless": False,
        "shipping": False,
        "local_project_settings": False,
        "print_command": False,
    }
    positional = []
    for argument in argv:
        if argument == "--editor":
            flags["editor"] = True
        elif argument == "--headless":
            flags["headless"] = True
        elif argument == "--shipping":
            flags["shipping"] = True
        elif argument == "--local_project_settings":
            flags["local_project_settings"] = True
        elif argument == "--print-command":
            flags["print_command"] = True
        elif argument.startswith("-"):
            raise ValueError(f"unknown option {argument}")
        else:
            positional.append(argument)
    if len(positional) != 1:
        raise ValueError(
            "one project path is required: the directory holding "
            "project_settings.yaml"
        )
    return positional[0], flags


def resolve_project(root: Path, project_path: str) -> Path:
    """The project directory, resolved against the working directory or the root."""
    candidate = (Path.cwd() / project_path).resolve()
    if candidate.is_dir():
        return candidate
    from_root = (root / project_path).resolve()
    if from_root.is_dir():
        return from_root
    raise ValueError(f"no project directory at {project_path}")


def describe_command(command, environment) -> str:
    """One copyable line: the environment it needs, then the command."""
    prefix = " ".join(f"{name}={value}" for name, value in environment.items())
    return f"{prefix} {' '.join(str(part) for part in command)}"


def build_host_command(generator, project_root: Path, flags) -> list:
    """The `cargo run` the project's settings ask for.

    A window needs the `rendering` feature and nothing else decides it: a
    project that enables a renderer gets one unless `--headless` says no, and a
    project that enables none is headless by construction. The module features
    are the host's business - its generated wrappers carry them - so they never
    appear here.
    """
    package = EDITOR_PACKAGE if flags["editor"] else GAME_PACKAGE
    draws = False
    if not flags["headless"]:
        draws = generator.load_renderer(project_root) is not None
    command = ["cargo", "run", "-p", package]
    if draws:
        command += ["--features", RENDERING_FEATURE]
    return command


def build_shipping_command(root: Path, project_root: Path, flags) -> list:
    """The shipping build the flags ask for, with its project path."""
    command = [
        sys.executable,
        str(root / SHIPPING_SCRIPT),
        "--project",
        str(project_root),
    ]
    if flags["local_project_settings"]:
        command.append("--local_project_settings")
    return command


def main() -> int:
    """Runs the project the command line names."""
    root = repository_root()
    try:
        project_path, flags = parse_arguments(sys.argv[1:])
        project_root = resolve_project(root, project_path)
        generator = load_generator(root)
        generator.INCLUDE_LOCAL_SETTINGS = flags["local_project_settings"]
        if flags["shipping"]:
            command = build_shipping_command(root, project_root, flags)
            working_directory = root
        else:
            command = build_host_command(generator, project_root, flags)
            working_directory = root / WORKSPACE_DIRECTORY
    except (ValueError, FileNotFoundError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    environment = {"PROJECT_PATH": str(project_root)}
    if flags["print_command"]:
        print(describe_command(command, environment))
        return 0
    print("$ " + describe_command(command, environment), flush=True)
    launch_environment = dict(os.environ)
    launch_environment.update(environment)
    return subprocess.call(
        [str(part) for part in command],
        cwd=str(working_directory),
        env=launch_environment,
    )


if __name__ == "__main__":
    sys.exit(main())
