"""The editor's application icons: generated from the one mark, well formed, unchanged.

PROTECTS: The editor's icon files are a fresh generation of the Pill mark - so they cannot drift from the mark the menu bar draws, or be hand-edited into something Windows, macOS or Linux refuses - and each file's structure and pixels are valid.
TIER: static

The files live in `modules/editor/pill_editor/res/`, four per colourway - the
`_black` set (the white mark on a black disc, the one the editor ships) and its
inverse, the `_white` set - and are rendered by
`devops/tools/generate_app_icons.py`. The suite regenerates them into a
temporary folder and compares byte for byte (the generator's own PNG writer
keeps a re-run identical), then reads every format back: the ICO's directory,
each entry's payload, the ICNS header and its chunk chain, and the pixels of
the small window icon.

The pixel checks are what a structure check cannot see: a re-run that loses the
mark still writes a valid disc, so the suite asks for the mark's colour in the
middle, the disc's colour clear of the mark, and nothing at all in the corners.

USAGE:
  python devops/tests/test_app_icons.py
  python devops/tests/test_app_icons.py --check-exe modules/target/debug/pill_editor.exe
"""
import argparse
import struct
import sys
import tempfile
import zlib
from pathlib import Path

DEVOPS_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(DEVOPS_ROOT / "tools"))

import generate_app_icons  # noqa: E402

ICON_DIRECTORY = generate_app_icons.OUTPUT_DIRECTORY
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
ICO_HEADER_SIZE = 6
ICO_ENTRY_SIZE = 16
# The resource types the editor's build script files its icons under, by the
# ordinals `rc.exe` uses: one resource per image, and one group naming them.
RESOURCE_TYPE_ICON = 3
RESOURCE_TYPE_GROUP_ICON = 14
# The 64px window icon, sampled where its three answers differ: the mark in
# the middle, the disc clear of the mark, and outside the disc.
WINDOW_ICON_SIDE = generate_app_icons.WINDOW_ICON_SIDE
# How far a sampled grey may sit from the colour it should be: the mark's
# centre is solid, and the disc's sample is near its rim, where it blends.
MARK_GREY_TOLERANCE = 25
DISC_GREY_TOLERANCE = 40


class IconError(AssertionError):
    """One icon file is not what the generator says it is."""


def read_png(data, where):
    """`(side, rows)` of an RGBA8 PNG written with filter 0 on every row."""
    if not data.startswith(PNG_SIGNATURE):
        raise IconError("{}: not a PNG".format(where))
    header = struct.unpack(">IIBBBBB", data[16:29])
    width, height, depth, colour_type = header[0], header[1], header[2], header[3]
    if depth != 8 or colour_type != 6:
        raise IconError(
            "{}: expected 8-bit RGBA, found depth {} colour type {}".format(
                where, depth, colour_type
            )
        )
    if width != height:
        raise IconError("{}: {}x{} is not square".format(where, width, height))
    compressed = b""
    offset = 8
    while offset + 8 <= len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        payload = data[offset + 8 : offset + 8 + length]
        if kind == b"IDAT":
            compressed += payload
        offset += 12 + length
        if kind == b"IEND":
            break
    raw = zlib.decompress(compressed)
    stride = width * 4
    rows = []
    for row in range(height):
        start = row * (stride + 1)
        if raw[start] != 0:
            raise IconError("{}: row {} carries filter {}".format(where, row, raw[start]))
        rows.append(raw[start + 1 : start + 1 + stride])
    return width, rows


def pixel(rows, x, y):
    start = x * 4
    red, green, blue, alpha = rows[y][start : start + 4]
    if red != green or green != blue:
        raise IconError("pixel ({}, {}) is not grey: {}".format(x, y, (red, green, blue)))
    return red, alpha


def check_generation_matches():
    """Every committed icon equals a fresh generation."""
    with tempfile.TemporaryDirectory() as temporary:
        generated = generate_app_icons.write_icon_set(Path(temporary))
        for name in generated:
            fresh = (Path(temporary) / name).read_bytes()
            committed_path = ICON_DIRECTORY / name
            if not committed_path.is_file():
                raise IconError(
                    "{} is missing; run python devops/tools/generate_app_icons.py".format(
                        committed_path
                    )
                )
            committed = committed_path.read_bytes()
            if committed != fresh:
                raise IconError(
                    "{} differs from a fresh generation ({} committed bytes, {} generated); "
                    "run python devops/tools/generate_app_icons.py".format(
                        name, len(committed), len(fresh)
                    )
                )


def check_ico(path):
    """The ICO's directory and every entry's payload."""
    data = path.read_bytes()
    reserved, kind, count = struct.unpack("<HHH", data[:ICO_HEADER_SIZE])
    if reserved != 0 or kind != 1:
        raise IconError("{}: not an ICO (reserved {}, type {})".format(path.name, reserved, kind))
    expected = generate_app_icons.ICO_ENTRIES
    if count != len(expected):
        raise IconError("{}: {} entries, expected {}".format(path.name, count, len(expected)))
    for index, (side, as_png) in enumerate(expected):
        start = ICO_HEADER_SIZE + index * ICO_ENTRY_SIZE
        width, height, colours, entry_reserved, planes, bits, size, offset = struct.unpack(
            "<BBBBHHII", data[start : start + ICO_ENTRY_SIZE]
        )
        wanted = 0 if side >= 256 else side
        if (width, height) != (wanted, wanted):
            raise IconError(
                "{} entry {}: declared {}x{}, expected {}x{}".format(
                    path.name, index, width, height, wanted, wanted
                )
            )
        if colours != 0 or entry_reserved != 0 or planes != 1 or bits != 32:
            raise IconError("{} entry {}: not a 32-bit colour entry".format(path.name, index))
        if offset + size > len(data):
            raise IconError(
                "{} entry {}: {} bytes at {} run past the {}-byte file".format(
                    path.name, index, size, offset, len(data)
                )
            )
        payload = data[offset : offset + size]
        if as_png:
            decoded_side, _rows = read_png(payload, "{} entry {}".format(path.name, index))
            if decoded_side != side:
                raise IconError(
                    "{} entry {}: PNG is {}px, expected {}px".format(
                        path.name, index, decoded_side, side
                    )
                )
            continue
        header_size, dib_width, dib_height, dib_planes, dib_bits = struct.unpack(
            "<IiiHH", payload[:16]
        )
        if (header_size, dib_width, dib_height, dib_planes, dib_bits) != (
            40,
            side,
            side * 2,
            1,
            32,
        ):
            raise IconError(
                "{} entry {}: bitmap header {}x{}x{}bpp".format(
                    path.name, index, dib_width, dib_height // 2, dib_bits
                )
            )
        mask_stride = ((side + 31) // 32) * 4
        if len(payload) != 40 + side * side * 4 + mask_stride * side:
            raise IconError(
                "{} entry {}: {} bytes, expected the bitmap and its mask".format(
                    path.name, index, len(payload)
                )
            )


def check_icns(path):
    """The ICNS header and its chunk chain, each chunk a PNG of its size."""
    data = path.read_bytes()
    if data[:4] != b"icns":
        raise IconError("{}: not an ICNS".format(path.name))
    total = struct.unpack(">I", data[4:8])[0]
    if total != len(data):
        raise IconError(
            "{}: header says {} bytes, the file is {}".format(path.name, total, len(data))
        )
    expected = generate_app_icons.ICNS_CHUNKS
    offset = 8
    for index, (kind, side) in enumerate(expected):
        if offset + 8 > len(data):
            raise IconError("{}: stops before chunk {}".format(path.name, index))
        found = data[offset : offset + 4].decode("ascii", "replace")
        if found != kind:
            raise IconError(
                "{} chunk {}: {} where {} belongs".format(path.name, index, found, kind)
            )
        length = struct.unpack(">I", data[offset + 4 : offset + 8])[0]
        if length < 8 or offset + length > len(data):
            raise IconError(
                "{} chunk {}: length {} runs past the file".format(path.name, index, length)
            )
        decoded_side, _rows = read_png(data[offset + 8 : offset + length], "{} {}".format(path.name, kind))
        if decoded_side != side:
            raise IconError(
                "{} {}: {}px PNG, expected {}px".format(path.name, kind, decoded_side, side)
            )
        offset += length
    if offset != len(data):
        raise IconError("{}: {} bytes after the last chunk".format(path.name, len(data) - offset))


def check_window_icon(path, mark_grey, disc_grey):
    """The small window icon draws the mark inside the disc, and nothing outside."""
    side, rows = read_png(path.read_bytes(), path.name)
    if side != WINDOW_ICON_SIDE:
        raise IconError("{}: {}px, expected {}px".format(path.name, side, WINDOW_ICON_SIDE))
    middle = side // 2
    grey, alpha = pixel(rows, middle, middle)
    if alpha < 250 or abs(grey - mark_grey) > MARK_GREY_TOLERANCE:
        raise IconError(
            "{}: the mark's centre is grey {} at alpha {}, expected {}".format(
                path.name, grey, alpha, mark_grey
            )
        )
    disc_x = int(side * 0.9)
    grey, alpha = pixel(rows, disc_x, middle)
    if alpha < 250 or abs(grey - disc_grey) > DISC_GREY_TOLERANCE:
        raise IconError(
            "{}: clear of the mark the disc is grey {} at alpha {}, expected {}".format(
                path.name, grey, alpha, disc_grey
            )
        )
    grey, alpha = pixel(rows, 0, 0)
    if alpha != 0 or grey != 0:
        raise IconError(
            "{}: the corner is grey {} at alpha {}, expected nothing".format(
                path.name, grey, alpha
            )
        )


def run_icon_checks():
    check_generation_matches()
    # Every colourway's four files, each read back in its own format.
    for variant, mark_grey, disc_grey in generate_app_icons.ICON_VARIANTS:
        names = generate_app_icons.icon_file_names(variant)
        check_ico(ICON_DIRECTORY / names["ico"])
        check_icns(ICON_DIRECTORY / names["icns"])
        for name, side in (
            (names["linux"], generate_app_icons.LINUX_ICON_SIDE),
            (names["window"], WINDOW_ICON_SIDE),
        ):
            decoded_side, _rows = read_png((ICON_DIRECTORY / name).read_bytes(), name)
            if decoded_side != side:
                raise IconError("{}: {}px, expected {}px".format(name, decoded_side, side))
        check_window_icon(ICON_DIRECTORY / names["window"], mark_grey, disc_grey)


def check_executable(path):
    """Windows builds the executable's icon from the resource the build links."""
    if sys.platform != "win32":
        raise IconError("the executable check asks Windows, and this is not Windows")
    import ctypes
    from ctypes import wintypes

    path = Path(path)
    if not path.is_file():
        raise IconError("{} is not a file; build the editor first".format(path))
    shell32 = ctypes.windll.shell32
    shell32.ExtractIconExW.argtypes = [
        wintypes.LPCWSTR,
        ctypes.c_int,
        ctypes.POINTER(wintypes.HICON),
        ctypes.POINTER(wintypes.HICON),
        wintypes.UINT,
    ]
    shell32.ExtractIconExW.restype = wintypes.UINT
    # `ExtractIconEx` counts icon groups, not the images inside one.
    groups = shell32.ExtractIconExW(str(path), -1, None, None, 0)
    if groups != 1:
        raise IconError(
            "{} carries {} icon groups, expected the one the build links".format(
                path.name, groups
            )
        )
    check_resource_table(path)
    large = wintypes.HICON()
    small = wintypes.HICON()
    built = shell32.ExtractIconExW(
        str(path), 0, ctypes.byref(large), ctypes.byref(small), 1
    )
    if built < 1 or not large.value or not small.value:
        raise IconError(
            "{}: Windows built no icon out of the resource".format(path.name)
        )
    user32 = ctypes.windll.user32
    user32.DestroyIcon.argtypes = [wintypes.HICON]
    user32.DestroyIcon(large)
    user32.DestroyIcon(small)


def check_resource_table(path):
    """Every image the asset holds is its own resource, under the one group."""
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.windll.kernel32
    kernel32.LoadLibraryExW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.HANDLE,
        wintypes.DWORD,
    ]
    kernel32.LoadLibraryExW.restype = wintypes.HMODULE
    kernel32.FindResourceW.argtypes = [wintypes.HMODULE, wintypes.LPCWSTR, wintypes.LPCWSTR]
    kernel32.FindResourceW.restype = wintypes.HANDLE
    kernel32.FreeLibrary.argtypes = [wintypes.HMODULE]

    def ordinal(value):
        return ctypes.cast(ctypes.c_void_p(value), wintypes.LPCWSTR)

    # LOAD_LIBRARY_AS_DATAFILE maps the executable for reading its resources
    # only, so nothing of it runs.
    module = kernel32.LoadLibraryExW(str(path), None, 0x00000002)
    if not module:
        raise IconError("{} could not be opened to read its resources".format(path.name))
    try:
        missing = []
        if not kernel32.FindResourceW(
            module, ordinal(1), ordinal(RESOURCE_TYPE_GROUP_ICON)
        ):
            missing.append("icon group")
        for index in range(1, len(generate_app_icons.ICO_ENTRIES) + 1):
            if not kernel32.FindResourceW(
                module, ordinal(index), ordinal(RESOURCE_TYPE_ICON)
            ):
                missing.append("image {}".format(index))
        if missing:
            raise IconError(
                "{}: the resource table has no {}".format(path.name, ", ".join(missing))
            )
    finally:
        kernel32.FreeLibrary(module)


def main():
    parser = argparse.ArgumentParser(
        description="The editor's application icons, and the ones an executable carries."
    )
    parser.add_argument(
        "--check-exe",
        type=Path,
        help="also ask Windows what icons this built editor executable carries",
    )
    arguments = parser.parse_args()
    try:
        run_icon_checks()
        print("  PASS  the icon files match the mark and every format is well formed")
        if arguments.check_exe is not None:
            check_executable(arguments.check_exe)
            print(
                "  PASS  {} carries the editor's icon as its own resource".format(
                    arguments.check_exe.name
                )
            )
    except IconError as error:
        print("  FAIL  {}".format(error))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
