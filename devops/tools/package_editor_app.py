#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+ (standard library only).
#
# DESCRIPTION: Writes the tree a Linux or macOS install of the editor needs.
#
#   The editor is one executable plus the icon its desktop environment reads,
#   so a "package" is that executable and the files that tell the platform what
#   to call it and which picture to show:
#
#     --platform linux   <output>/pill_editor, beside the 256-pixel PNG under
#                        share/icons/hicolor/256x256/apps and a
#                        pill_editor.desktop entry. Wayland's compositors take
#                        a window's taskbar icon from the desktop entry alone,
#                        matched to the window's app id - which winit derives
#                        from the process name - so the entry is named after
#                        the executable.
#
#     --platform macos   Pill.app/Contents, holding the executable under
#                        MacOS, the .icns under Resources and an Info.plist
#                        naming both. A bare executable gets no Dock icon on
#                        macOS; the bundle is what carries one.
#
#   The icon files are the ones `devops/tools/generate_app_icons.py` renders
#   from the Pill mark; nothing here draws anything. Without --binary the tool
#   writes only the metadata and the icons, which is what CI checks on a
#   machine that cannot build an executable for the target.
#
# USAGE:
#   python devops/tools/package_editor_app.py --platform linux --output build/linux
#   python devops/tools/package_editor_app.py --platform macos --output build/macos \
#       --binary modules/target/release/pill_editor --version 0.1.0

import argparse
import os
import plistlib
import shutil
import sys
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parent.parent.parent
ICON_DIRECTORY = REPOSITORY_ROOT / "modules" / "editor" / "pill_editor" / "res"
EDITOR_MANIFEST = REPOSITORY_ROOT / "modules" / "editor" / "pill_editor" / "Cargo.toml"

APP_NAME = "Pill Editor"
APP_BUNDLE_NAME = "Pill"
EXECUTABLE_NAME = "pill_editor"
ICON_BASENAME = "pill_editor"
# The colourway the install trees carry: the white mark on the black disc. The
# installed files drop the suffix, because the desktop entry and the plist name
# the icon after the executable.
ICON_VARIANT = "black"
SOURCE_ICON_PNG = ICON_DIRECTORY / "{}_{}.png".format(ICON_BASENAME, ICON_VARIANT)
SOURCE_ICON_ICNS = ICON_DIRECTORY / "{}_{}.icns".format(ICON_BASENAME, ICON_VARIANT)
BUNDLE_IDENTIFIER = "com.pillware.pill-editor"
MINIMUM_MACOS = "11.0"
LINUX_ICON_SIDE = 256

# The desktop entry's fields. `Icon` is filled with the installed PNG's own
# path, because the tool writes a tree the person places themselves; a system
# install points it at the same file under /usr/share.
DESKTOP_ENTRY_ORDER = (
    ("Type", "Application"),
    ("Name", APP_NAME),
    ("GenericName", "Game Engine Editor"),
    ("Comment", "Edit and run a Pill Engine project"),
    ("Exec", "{} %F".format(EXECUTABLE_NAME)),
    ("Icon", None),
    ("Terminal", "false"),
    ("Categories", "Development;Game;"),
    ("Keywords", "pill;engine;game;editor;"),
    # X11 groups a window to its entry by this class; Wayland matches the
    # window's app id, which winit takes from the process name - the same
    # string, which is why the entry is named `pill_editor.desktop`.
    ("StartupWMClass", EXECUTABLE_NAME),
)


def editor_version():
    """The version the editor manifest declares, as a string."""
    for line in EDITOR_MANIFEST.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("version = "):
            return stripped.split("=", 1)[1].strip().strip('"')
    raise SystemExit("the editor manifest declares no version: {}".format(EDITOR_MANIFEST))


def copy_binary(binary, destination):
    """Copies the editor executable, and makes it executable on POSIX."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(binary, destination)
    os.chmod(destination, 0o755)


def desktop_entry(icon_path):
    """The `pill_editor.desktop` file's text."""
    lines = ["[Desktop Entry]"]
    for key, value in DESKTOP_ENTRY_ORDER:
        lines.append("{}={}".format(key, icon_path if value is None else value))
    return "\n".join(lines) + "\n"


def package_linux(output, binary=None):
    """Writes the Linux tree, and returns the files it wrote."""
    output = Path(output)
    icon = output / "share" / "icons" / "hicolor" / "{}x{}".format(
        LINUX_ICON_SIDE, LINUX_ICON_SIDE
    ) / "apps" / "{}.png".format(ICON_BASENAME)
    icon.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(SOURCE_ICON_PNG, icon)
    written = [icon]
    if binary is not None:
        executable = output / EXECUTABLE_NAME
        copy_binary(binary, executable)
        written.append(executable)
    entry = output / "{}.desktop".format(EXECUTABLE_NAME)
    entry.write_text(desktop_entry(icon.resolve().as_posix()), encoding="utf-8")
    os.chmod(entry, 0o755)
    written.append(entry)
    return written


def info_plist(version):
    """The bundle's `Info.plist` contents."""
    return {
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": APP_BUNDLE_NAME,
        "CFBundleDisplayName": APP_NAME,
        "CFBundleExecutable": EXECUTABLE_NAME,
        # macOS treats the `.icns` extension as implied, so the key carries the
        # name alone.
        "CFBundleIconFile": ICON_BASENAME,
        "CFBundleIdentifier": BUNDLE_IDENTIFIER,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSMinimumSystemVersion": MINIMUM_MACOS,
        "NSHighResolutionCapable": True,
    }


def package_macos(output, binary=None, version=None):
    """Writes the `.app` bundle, and returns the files it wrote."""
    version = version or editor_version()
    contents = Path(output) / "{}.app".format(APP_BUNDLE_NAME) / "Contents"
    resources = contents / "Resources"
    resources.mkdir(parents=True, exist_ok=True)
    icon = resources / "{}.icns".format(ICON_BASENAME)
    shutil.copyfile(SOURCE_ICON_ICNS, icon)
    written = [icon]
    if binary is not None:
        executable = contents / "MacOS" / EXECUTABLE_NAME
        copy_binary(binary, executable)
        written.append(executable)
    manifest = contents / "Info.plist"
    with manifest.open("wb") as handle:
        plistlib.dump(info_plist(version), handle, sort_keys=True)
    written.append(manifest)
    return written


def main():
    parser = argparse.ArgumentParser(
        description="Write the Linux or macOS install tree for the Pill editor."
    )
    parser.add_argument(
        "--platform", choices=("linux", "macos"), required=True, help="the tree to write"
    )
    parser.add_argument(
        "--output", type=Path, required=True, help="the folder the tree is written into"
    )
    parser.add_argument(
        "--binary",
        type=Path,
        help="the built editor executable to copy into the tree",
    )
    parser.add_argument(
        "--version",
        help="the version the macOS bundle declares (default: the editor manifest's)",
    )
    arguments = parser.parse_args()
    if arguments.binary is not None and not arguments.binary.is_file():
        raise SystemExit("no executable at {}".format(arguments.binary))
    if arguments.platform == "linux":
        written = package_linux(arguments.output, arguments.binary)
    else:
        written = package_macos(arguments.output, arguments.binary, arguments.version)
    for path in written:
        print("wrote {}".format(path))
    return 0


if __name__ == "__main__":
    sys.exit(main())
