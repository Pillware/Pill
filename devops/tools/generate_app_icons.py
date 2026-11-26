#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+ (standard library only).
#
# DESCRIPTION: Renders the editor's application icon files from the Pill mark.
#
#   The mark is `modules/editor/pill_editor_core/assets/pill_mark.svg` - the
#   white pill the editor's menu bar draws. This tool flattens the path's
#   subpaths through the transforms the file carries, fits the path's own
#   bounding box into a disc (the SVG's viewBox clips that path, so the box is
#   measured from the points rather than trusted from the file), and fills it
#   even-odd with scanline coverage: vertical samples per pixel, four for a
#   large icon and eight for one 64 pixels and smaller, plus the exact
#   fraction each span covers of the pixel it starts and ends in, in straight
#   RGBA. It renders two colourways, each named after its disc:
#
#     _black   the white mark over a black disc - the one the editor ships.
#     _white   the inverse: the black mark over a white disc.
#
#   It writes one set per colourway into the editor executable's `res` folder:
#
#     pill_editor_<colour>.ico              Windows: 16, 24, 32, 48 and 64 as
#                                           32-bit DIB entries, 128 and 256 as
#                                           PNG entries.
#     pill_editor_<colour>.icns             macOS: PNG chunks from 16 through
#                                           1024.
#     pill_editor_<colour>.png              Linux: 256, the desktop entry's icon.
#     pill_editor_window_icon_<colour>.png  The run-time window icon: 64.
#
#   The PNG writer is this tool's own (RGBA8, filter 0, zlib level 9), so a
#   re-run is byte-identical and the devops suite can compare the committed
#   files against a fresh generation.
#
# USAGE:
#   --output <dir>     folder the icon files are written to (default: the
#                      editor's res folder)
#   --mark <path.svg>  the SVG the icons are rendered from (default: the Pill
#                      mark)
#
# EXAMPLE USAGE:
#   python devops/tools/generate_app_icons.py
#   python devops/tools/generate_app_icons.py --output <dir> --mark <path.svg>
#
# --- SCRIPT ---

import argparse
import math
import re
import struct
import sys
import zlib
from itertools import accumulate
from pathlib import Path
from xml.etree import ElementTree

REPOSITORY_ROOT = Path(__file__).resolve().parent.parent.parent
MARK_PATH = (
    REPOSITORY_ROOT
    / "modules"
    / "editor"
    / "pill_editor_core"
    / "assets"
    / "pill_mark.svg"
)
OUTPUT_DIRECTORY = REPOSITORY_ROOT / "modules" / "editor" / "pill_editor" / "res"

# The mark fills this share of the icon's side, centred. The disc's inscribed
# square is 1/sqrt(2) of its diameter, so this leaves the mark well inside the
# circle with a margin that reads as deliberate at 16 pixels.
MARK_FILL = 0.62
# Straight segments per cubic curve. The mark's curves have a radius near a
# tenth of the icon, which makes a segment's flatness error far below a
# quarter of a pixel even at 1024.
CURVE_SEGMENTS = 16
# Vertical samples per pixel: an icon this small covers proportionally more of
# the shape in one pixel, so it gets more.
SUBSAMPLES = 4
COARSE_SUBSAMPLES = 8
COARSE_SIDE = 64
# The colourways, each `(suffix, mark grey, disc grey)`: the suffix names the
# disc's colour, the part of the icon a taskbar shows most of. `black` is the
# set the editor's executable, window and install trees carry.
ICON_VARIANTS = (
    ("black", 255, 0),
    ("white", 0, 255),
)

ICO_ENTRIES = (
    (16, False),
    (24, False),
    (32, False),
    (48, False),
    (64, False),
    (128, True),
    (256, True),
)
ICNS_CHUNKS = (
    ("icp4", 16),
    ("icp5", 32),
    ("icp6", 64),
    ("ic07", 128),
    ("ic08", 256),
    ("ic09", 512),
    ("ic10", 1024),
    ("ic11", 32),
    ("ic12", 64),
    ("ic13", 256),
    ("ic14", 512),
)
LINUX_ICON_SIDE = 256
WINDOW_ICON_SIDE = 64

IDENTITY = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
TRANSFORM_CALL = re.compile(r"([a-zA-Z]+)\s*\(([^)]*)\)")
PATH_TOKEN = re.compile(r"[A-Za-z]|-?\d*\.?\d+(?:[eE][-+]?\d+)?")


# --- SVG geometry ---------------------------------------------------------


def multiply(left, right):
    """`left` applied after `right`, both in SVG's six-value matrix form."""
    a1, b1, c1, d1, e1, f1 = left
    a2, b2, c2, d2, e2, f2 = right
    return (
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * e2 + c1 * f2 + e1,
        b1 * e2 + d1 * f2 + f1,
    )


def apply_point(matrix, point):
    a, b, c, d, e, f = matrix
    x, y = point
    return (a * x + c * y + e, b * x + d * y + f)


def transform_matrix(name, arguments):
    if name == "matrix" and len(arguments) == 6:
        return tuple(arguments)
    if name == "translate" and len(arguments) in (1, 2):
        return (
            1.0,
            0.0,
            0.0,
            1.0,
            arguments[0],
            arguments[1] if len(arguments) == 2 else 0.0,
        )
    if name == "scale" and len(arguments) in (1, 2):
        scale_x = arguments[0]
        scale_y = arguments[1] if len(arguments) == 2 else scale_x
        return (scale_x, 0.0, 0.0, scale_y, 0.0, 0.0)
    raise SystemExit(
        "the mark uses an unsupported transform: {}({})".format(name, arguments)
    )


def parse_transform(text):
    """The matrix of one `transform` attribute; identity when absent."""
    if not text or not text.strip():
        return IDENTITY
    matrix = IDENTITY
    starts = 0
    while True:
        match = TRANSFORM_CALL.search(text, starts)
        if not match:
            break
        starts = match.end()
        arguments = [
            float(value) for value in re.split(r"[,\s]+", match.group(2).strip()) if value
        ]
        matrix = multiply(matrix, transform_matrix(match.group(1), arguments))
    return matrix


def cubic_point(start, control_one, control_two, end, t):
    inverse = 1.0 - t
    return (
        inverse ** 3 * start[0]
        + 3.0 * inverse ** 2 * t * control_one[0]
        + 3.0 * inverse * t ** 2 * control_two[0]
        + t ** 3 * end[0],
        inverse ** 3 * start[1]
        + 3.0 * inverse ** 2 * t * control_one[1]
        + 3.0 * inverse * t ** 2 * control_two[1]
        + t ** 3 * end[1],
    )


def parse_path(data):
    """One path's subpaths as point lists. Only the absolute M, C, L and Z."""
    tokens = PATH_TOKEN.findall(data)
    subpaths = []
    current = []
    position = (0.0, 0.0)
    command = None
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token.isalpha():
            command = token
            index += 1
            if command == "Z":
                if current:
                    subpaths.append(current)
                    current = []
                command = None
                continue
            if command == "M":
                position = (float(tokens[index]), float(tokens[index + 1]))
                index += 2
                if current:
                    subpaths.append(current)
                current = [position]
                # Further coordinate pairs after a move are implicit lines.
                command = "L"
                continue
        if command == "L":
            position = (float(tokens[index]), float(tokens[index + 1]))
            index += 2
            current.append(position)
        elif command == "C":
            control_one = (float(tokens[index]), float(tokens[index + 1]))
            control_two = (float(tokens[index + 2]), float(tokens[index + 3]))
            end = (float(tokens[index + 4]), float(tokens[index + 5]))
            index += 6
            for step in range(1, CURVE_SEGMENTS + 1):
                current.append(
                    cubic_point(
                        position, control_one, control_two, end, step / CURVE_SEGMENTS
                    )
                )
            position = end
        else:
            raise SystemExit(
                "the mark uses an unsupported path command: {}".format(command)
            )
    if current:
        subpaths.append(current)
    return subpaths


def collect_paths(element, matrix, paths):
    """Every path below `element` with the matrix that maps it to view space."""
    tag = element.tag.split("}")[-1]
    matrix = multiply(matrix, parse_transform(element.get("transform")))
    if tag == "path":
        paths.append((parse_path(element.get("d") or ""), matrix))
    for child in element:
        collect_paths(child, matrix, paths)


def mark_subpaths(mark_path):
    """The mark's subpaths, in view-space coordinates."""
    paths = []
    collect_paths(ElementTree.parse(str(mark_path)).getroot(), IDENTITY, paths)
    subpaths = [
        [apply_point(matrix, point) for point in subpath]
        for path_subpaths, matrix in paths
        for subpath in path_subpaths
        if subpath
    ]
    if not subpaths:
        raise SystemExit("the mark has no path: {}".format(mark_path))
    return subpaths


def fit_subpaths(subpaths, side):
    """The mark scaled and centred so its bounding box fills `MARK_FILL`."""
    xs = [x for subpath in subpaths for x, _ in subpath]
    ys = [y for subpath in subpaths for _, y in subpath]
    left, right = min(xs), max(xs)
    top, bottom = min(ys), max(ys)
    span = max(right - left, bottom - top)
    if span <= 0.0:
        raise SystemExit("the mark has no extent: {}x{}".format(right - left, bottom - top))
    scale = side * MARK_FILL / span
    middle_x = (left + right) / 2.0
    middle_y = (top + bottom) / 2.0
    centre = side / 2.0
    return [
        [((x - middle_x) * scale + centre, (y - middle_y) * scale + centre) for x, y in subpath]
        for subpath in subpaths
    ]


def polygon_edges(subpaths):
    """Non-horizontal edges as (y0, y1, x0, x1), each subpath closed."""
    edges = []
    for subpath in subpaths:
        for index in range(len(subpath)):
            x0, y0 = subpath[index]
            x1, y1 = subpath[(index + 1) % len(subpath)]
            if y0 != y1:
                edges.append((y0, y1, x0, x1))
    return edges


# --- coverage -------------------------------------------------------------


def add_span(delta, extra, side, start, end, weight):
    """Add one covered span to a row: a difference array plus partial ends."""
    start = max(0.0, min(float(side), start))
    end = max(0.0, min(float(side), end))
    if end <= start:
        return
    first = int(math.floor(start))
    last = int(math.floor(end))
    if first == last:
        extra[first] += weight * (end - start)
        return
    extra[first] += weight * (first + 1 - start)
    delta[first + 1] += weight
    delta[last] -= weight
    if last < side:
        extra[last] += weight * (end - last)


def render_icon(subpaths, side, mark_grey, disc_grey):
    """The icon at `side` pixels as straight RGBA, the mark over the disc."""
    edges = polygon_edges(subpaths)
    radius = side / 2.0
    centre = side / 2.0
    samples = COARSE_SUBSAMPLES if side <= COARSE_SIDE else SUBSAMPLES
    weight = 1.0 / samples
    rows = []
    for row in range(side):
        mark_delta = [0.0] * (side + 1)
        mark_extra = [0.0] * side
        disc_delta = [0.0] * (side + 1)
        disc_extra = [0.0] * side
        for sample in range(samples):
            y = row + (sample + 0.5) * weight
            crossings = [
                x0 + (y - y0) * (x1 - x0) / (y1 - y0)
                for y0, y1, x0, x1 in edges
                if (y0 <= y < y1) or (y1 <= y < y0)
            ]
            crossings.sort()
            for index in range(0, len(crossings) - 1, 2):
                add_span(
                    mark_delta,
                    mark_extra,
                    side,
                    crossings[index],
                    crossings[index + 1],
                    weight,
                )
            # The disc is a circle, so its crossings have a closed form.
            offset = y - centre
            if abs(offset) < radius:
                half = math.sqrt(radius * radius - offset * offset)
                add_span(disc_delta, disc_extra, side, centre - half, centre + half, weight)
        mark = [
            min(1.0, value + extra)
            for value, extra in zip(accumulate(mark_delta), mark_extra)
        ]
        disc = [
            min(1.0, value + extra)
            for value, extra in zip(accumulate(disc_delta), disc_extra)
        ]
        rows.append(compose_row(mark, disc, mark_grey, disc_grey))
    return b"".join(rows)


def compose_row(mark, disc, mark_grey, disc_grey):
    """One row of straight RGBA: the mark's grey composited over the disc's."""
    row = bytearray(len(mark) * 4)
    for index, (mark_coverage, disc_coverage) in enumerate(zip(mark, disc)):
        alpha = mark_coverage + disc_coverage * (1.0 - mark_coverage)
        if alpha <= 0.0:
            continue
        # The visible colour is each layer's share of the covered pixel, so the
        # mark's own edge blends into the disc as an in-between grey.
        disc_share = disc_coverage * (1.0 - mark_coverage)
        grey = int(round((mark_grey * mark_coverage + disc_grey * disc_share) / alpha))
        offset = index * 4
        row[offset] = grey
        row[offset + 1] = grey
        row[offset + 2] = grey
        row[offset + 3] = int(round(255.0 * alpha))
    return bytes(row)


# --- file formats ---------------------------------------------------------


def png_chunk(kind, payload):
    body = kind + payload
    return (
        struct.pack(">I", len(payload))
        + body
        + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    )


def png_bytes(side, rgba):
    """A PNG of `side` square, RGBA8, filter 0 on every row."""
    stride = side * 4
    raw = b"".join(
        b"\x00" + rgba[row * stride : (row + 1) * stride] for row in range(side)
    )
    header = struct.pack(">IIBBBBB", side, side, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + png_chunk(b"IHDR", header)
        + png_chunk(b"IDAT", zlib.compress(raw, 9))
        + png_chunk(b"IEND", b"")
    )


def dib_bytes(side, rgba):
    """A 32-bit icon bitmap: upside-down BGRA rows, then the AND mask."""
    stride = side * 4
    rows = []
    for row in range(side - 1, -1, -1):
        source = rgba[row * stride : (row + 1) * stride]
        converted = bytearray(stride)
        for index in range(side):
            red, green, blue, alpha = source[index * 4 : index * 4 + 4]
            converted[index * 4 : index * 4 + 4] = bytes((blue, green, red, alpha))
        rows.append(bytes(converted))
    mask_stride = ((side + 31) // 32) * 4
    mask = bytes(mask_stride * side)
    header = struct.pack("<IiiHHIIiiII", 40, side, side * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    return header + b"".join(rows) + mask


def ico_bytes(rendered, entries):
    """An ICO whose entries are `(side, is_png)` pairs."""
    images = []
    for side, as_png in entries:
        rgba = rendered[side]
        images.append(
            (side, png_bytes(side, rgba) if as_png else dib_bytes(side, rgba))
        )
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    directory = b""
    payload = b""
    for side, data in images:
        dimension = 0 if side >= 256 else side
        directory += struct.pack(
            "<BBBBHHII", dimension, dimension, 0, 0, 1, 32, len(data), offset + len(payload)
        )
        payload += data
    return header + directory + payload


def icns_bytes(rendered, chunks):
    """An ICNS whose chunks are `(type, side)` pairs, each a PNG."""
    body = b""
    for kind, side in chunks:
        data = png_bytes(side, rendered[side])
        body += kind.encode("ascii") + struct.pack(">I", len(data) + 8) + data
    return b"icns" + struct.pack(">I", len(body) + 8) + body


def icon_file_names(variant):
    """The four file names of one colourway, keyed by what each file is for."""
    return {
        "ico": "pill_editor_{}.ico".format(variant),
        "icns": "pill_editor_{}.icns".format(variant),
        "linux": "pill_editor_{}.png".format(variant),
        "window": "pill_editor_window_icon_{}.png".format(variant),
    }


def write_icon_set(output_directory, mark_path=MARK_PATH):
    """Write every colourway's icon files, and return their names."""
    subpaths = mark_subpaths(mark_path)
    sides = sorted(
        {side for side, _ in ICO_ENTRIES}
        | {side for _, side in ICNS_CHUNKS}
        | {LINUX_ICON_SIDE, WINDOW_ICON_SIDE}
    )
    # The fitted geometry depends on the side alone, so both colourways share it.
    fitted = {side: fit_subpaths(subpaths, side) for side in sides}
    files = {}
    for variant, mark_grey, disc_grey in ICON_VARIANTS:
        rendered = {
            side: render_icon(fitted[side], side, mark_grey, disc_grey) for side in sides
        }
        names = icon_file_names(variant)
        files[names["ico"]] = ico_bytes(rendered, ICO_ENTRIES)
        files[names["icns"]] = icns_bytes(rendered, ICNS_CHUNKS)
        files[names["linux"]] = png_bytes(LINUX_ICON_SIDE, rendered[LINUX_ICON_SIDE])
        files[names["window"]] = png_bytes(WINDOW_ICON_SIDE, rendered[WINDOW_ICON_SIDE])
    output_directory = Path(output_directory)
    output_directory.mkdir(parents=True, exist_ok=True)
    for name, data in files.items():
        (output_directory / name).write_bytes(data)
    return sorted(files)


def main():
    parser = argparse.ArgumentParser(
        description="Generate the editor's application icons from the Pill mark."
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=OUTPUT_DIRECTORY,
        help="folder the icon files are written to (default: the editor's res folder)",
    )
    parser.add_argument(
        "--mark",
        type=Path,
        default=MARK_PATH,
        help="the SVG the icons are generated from (default: the Pill mark)",
    )
    arguments = parser.parse_args()
    for name in write_icon_set(arguments.output, arguments.mark):
        print("wrote {}".format(arguments.output / name))
    return 0


if __name__ == "__main__":
    sys.exit(main())
