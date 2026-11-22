# REQUIREMENTS
#   Blender 3.6 or newer, run in background mode with the user's own
#   preferences (no --factory-startup).
#
# DESCRIPTION
#   Installs the Pill Engine live link as a Blender add-on and enables it, so
#   every Blender the person opens connects to the Pill editor by itself. The
#   installed add-on is a small loader that runs the link script from the
#   engine's checkout, so an update to the script needs no reinstall. The
#   preferences are saved, which is what keeps the add-on enabled. The Pill
#   editor runs this once per session, in the background, when the Blender
#   button is used; set PILL_BLENDER_INSTALL_ADDON=0 to stop it.
#
# USAGE
#   blender -b --python-exit-code 1 --python install_addon.py -- <path of pill_blender_link.py>
#
# EXAMPLE USAGE
#   blender.exe -b --python-exit-code 1 --python install_addon.py -- D:\Pill\modules\extensions\pill_blender_link\blender_addon\pill_blender_link.py
#
# --- SCRIPT ---

import os
import sys

import addon_utils
import bpy

ADDON_MODULE = "pill_blender_link"

# The loader Blender keeps in its add-ons folder. Blender reads `bl_info`
# without running the file, so it is written out as a literal.
LOADER_TEMPLATE = '''# Pill Engine Live Link - loader installed by the Pill editor.
# It runs the link script from the engine's checkout, so an update there needs
# no reinstall. Remove this folder (or disable the add-on) to stop the link.

bl_info = {
    "name": "Pill Engine Live Link",
    "author": "Pill Engine",
    "version": (1, 0, 0),
    "blender": (3, 6, 0),
    "location": "3D Viewport > Sidebar > Pill",
    "description": "Edit Pill Engine mesh assets in Blender and stream every change back to the editor",
    "category": "Import-Export",
}

LINK_SCRIPT = @SCRIPT@

_link_namespace = {"__name__": __name__, "__file__": LINK_SCRIPT}
with open(LINK_SCRIPT, encoding="utf-8") as _link_file:
    exec(compile(_link_file.read(), LINK_SCRIPT, "exec"), _link_namespace)
# The add-on module carries the script's names (its state, its timer, its
# operators), so it reads exactly like the script run on its own.
globals().update({key: value for key, value in _link_namespace.items() if not key.startswith("__")})


def register():
    _link_namespace["register"]()


def unregister():
    _link_namespace["unregister"]()
'''


def main():
    script_arguments = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    if not script_arguments:
        print("PILL_ADDON_FAILED no link script path given")
        sys.exit(1)
    link_script_path = os.path.abspath(script_arguments[0])
    if not os.path.isfile(link_script_path):
        print(f"PILL_ADDON_FAILED no link script at {link_script_path}")
        sys.exit(1)
    # Step 1: write the loader into the user's add-ons folder.
    addons_directory = bpy.utils.user_resource("SCRIPTS", path="addons", create=True)
    loader_directory = os.path.join(addons_directory, ADDON_MODULE)
    os.makedirs(loader_directory, exist_ok=True)
    loader_path = os.path.join(loader_directory, "__init__.py")
    with open(loader_path, "w", encoding="utf-8", newline="\n") as loader_file:
        loader_file.write(LOADER_TEMPLATE.replace("@SCRIPT@", repr(link_script_path)))
    # Step 2: enable it and save the preferences, so it stays enabled. A
    # folder created during this run is not on the import path until the
    # script paths are refreshed.
    bpy.utils.refresh_script_paths()
    addon_utils.modules_refresh()
    bpy.ops.preferences.addon_enable(module=ADDON_MODULE)
    bpy.ops.wm.save_userpref()
    enabled = ADDON_MODULE in bpy.context.preferences.addons
    print(f"PILL_ADDON_{'INSTALLED' if enabled else 'FAILED'} {loader_path}")
    sys.exit(0 if enabled else 1)


main()
