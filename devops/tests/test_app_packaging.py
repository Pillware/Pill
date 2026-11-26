"""The editor's Linux and macOS install trees: the files each platform reads for its icon.

PROTECTS: A Linux install carries a desktop entry a compositor can find the editor's icon through, and a macOS bundle carries the Info.plist and .icns that give it a Dock icon - the entry names files the tree really holds, and the pictures are the generated ones.
TIER: static

Neither tree can be run here: the editor builds and runs on Windows only, and the Linux port is parked. What is checked is what the platforms read - the entry's keys and file name, the plist's keys, and that every file they name is present and identical to the generated icon.

USAGE:
  python devops/tests/test_app_packaging.py
  python devops/tests/test_app_packaging.py --platform linux
"""
import argparse
import os
import plistlib
import sys
import tempfile
from pathlib import Path

DEVOPS_ROOT = Path(__file__).resolve().parent.parent
REPOSITORY_ROOT = DEVOPS_ROOT.parent
sys.path.insert(0, str(DEVOPS_ROOT / "tools"))

import package_editor_app  # noqa: E402


class PackagingError(AssertionError):
    """One install tree is not what its platform reads."""


def write_dummy_executable(path):
    """A stand-in for the built editor, so the tree is complete without a build."""
    path.write_bytes(b"#!stand-in for the built editor\n")
    return path


def read_desktop_entry(path):
    """The entry's `key=value` lines, as a dictionary."""
    if not path.is_file():
        raise PackagingError("{}: the desktop entry is missing".format(path))
    entries = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("[") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        entries[key] = value
    return entries


def check_linux_tree(output, binary):
    """The tree a Linux install needs, and the entry that names it."""
    entry_path = output / "pill_editor.desktop"
    entries = read_desktop_entry(entry_path)
    # A compositor matches the window's app id - the process name - to the
    # entry's file name, so the name is part of the contract.
    if entry_path.name != "{}.desktop".format(package_editor_app.EXECUTABLE_NAME):
        raise PackagingError("{}: not named for the window's app id".format(entry_path.name))
    if entries.get("Type") != "Application":
        raise PackagingError("the entry is not an Application: {}".format(entries.get("Type")))
    if entries.get("Name") != package_editor_app.APP_NAME:
        raise PackagingError("the entry is called {}".format(entries.get("Name")))
    if not entries.get("Exec", "").startswith(package_editor_app.EXECUTABLE_NAME):
        raise PackagingError("the entry runs {}".format(entries.get("Exec")))
    if entries.get("Terminal") != "false":
        raise PackagingError("the entry asks for a terminal")
    categories = entries.get("Categories", "")
    if "Development" not in categories.split(";") or not categories.endswith(";"):
        raise PackagingError("the entry's categories are {}".format(categories))
    if entries.get("StartupWMClass") != package_editor_app.EXECUTABLE_NAME:
        raise PackagingError("the entry's X11 class is {}".format(entries.get("StartupWMClass")))
    icon_value = entries.get("Icon", "")
    if not icon_value:
        raise PackagingError("the entry names no icon")
    icon = Path(icon_value)
    if not icon.is_file():
        raise PackagingError("the entry's icon {} is not in the tree".format(icon_value))
    if icon.parts[-6:] != (
        "share",
        "icons",
        "hicolor",
        "256x256",
        "apps",
        "pill_editor.png",
    ):
        raise PackagingError("the icon is not under a hicolor icon path: {}".format(icon_value))
    committed = package_editor_app.SOURCE_ICON_PNG.read_bytes()
    if icon.read_bytes() != committed:
        raise PackagingError("the installed icon is not the generated one")
    if os.name == "posix" and (entry_path.stat().st_mode & 0o111) == 0:
        raise PackagingError("the desktop entry is not executable")
    if not binary.is_file():
        raise PackagingError("the tree carries no executable")


def check_macos_tree(output, version):
    """The bundle, its plist, and the files the plist names."""
    contents = output / "Pill.app" / "Contents"
    manifest = contents / "Info.plist"
    if not manifest.is_file():
        raise PackagingError("{}: the bundle has no Info.plist".format(contents))
    with manifest.open("rb") as handle:
        plist = plistlib.load(handle)
    executable_name = plist.get("CFBundleExecutable")
    if not executable_name:
        raise PackagingError("the plist names no executable")
    executable = contents / "MacOS" / executable_name
    if not executable.is_file():
        raise PackagingError("the plist's executable {} is not in the bundle".format(executable_name))
    if os.name == "posix" and (executable.stat().st_mode & 0o111) == 0:
        raise PackagingError("the bundle's executable is not executable")
    icon_name = plist.get("CFBundleIconFile")
    if not icon_name:
        raise PackagingError("the plist names no icon")
    icon = contents / "Resources" / "{}.icns".format(icon_name)
    if not icon.is_file():
        raise PackagingError("the plist's icon {} is not in the bundle".format(icon_name))
    committed = package_editor_app.SOURCE_ICON_ICNS.read_bytes()
    if icon.read_bytes() != committed:
        raise PackagingError("the bundled icon is not the generated one")
    expected = {
        "CFBundlePackageType": "APPL",
        "CFBundleName": package_editor_app.APP_BUNDLE_NAME,
        "CFBundleDisplayName": package_editor_app.APP_NAME,
        "CFBundleIdentifier": package_editor_app.BUNDLE_IDENTIFIER,
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "CFBundleInfoDictionaryVersion": "6.0",
    }
    for key, value in expected.items():
        if plist.get(key) != value:
            raise PackagingError(
                "the plist's {} is {!r}, expected {!r}".format(key, plist.get(key), value)
            )
    if plist.get("NSHighResolutionCapable") is not True:
        raise PackagingError("the bundle does not declare retina support")


def run_linux_checks():
    with tempfile.TemporaryDirectory() as temporary:
        output = Path(temporary) / "linux"
        binary = write_dummy_executable(Path(temporary) / "pill_editor")
        package_editor_app.package_linux(output, binary)
        check_linux_tree(output, output / package_editor_app.EXECUTABLE_NAME)


def run_macos_checks():
    with tempfile.TemporaryDirectory() as temporary:
        output = Path(temporary) / "macos"
        binary = write_dummy_executable(Path(temporary) / "pill_editor")
        version = package_editor_app.editor_version()
        package_editor_app.package_macos(output, binary, version)
        check_macos_tree(output, version)


def main():
    parser = argparse.ArgumentParser(
        description="The editor's Linux desktop entry and macOS app bundle."
    )
    parser.add_argument(
        "--platform",
        choices=("linux", "macos", "all"),
        default="all",
        help="which install tree to check (default: both)",
    )
    arguments = parser.parse_args()
    checks = []
    if arguments.platform in ("linux", "all"):
        checks.append(("the Linux desktop entry", run_linux_checks))
    if arguments.platform in ("macos", "all"):
        checks.append(("the macOS app bundle", run_macos_checks))
    for label, check in checks:
        try:
            check()
        except PackagingError as error:
            print("  FAIL  {}".format(error))
            return 1
        print("  PASS  {}".format(label))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
