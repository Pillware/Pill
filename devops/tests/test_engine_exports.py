#!/usr/bin/env python3
"""
Engine export count check: each engine dylib stays clear of the Windows limit
of 65,535 exported names per DLL.

PROTECTS: Each engine dylib stays below the Windows limit of 65,535 exported symbols.
TIER: build

REQUIREMENTS: Python 3.8+ (standard library only). The engine dylibs built
  (`cargo build -p pill_core -p pill_engine_core` in `modules/`).

DESCRIPTION
    A Rust `dylib` exports every symbol, and the linker refuses a DLL with more
    than 65,535 exports (`LNK1189`). `pill_engine` was once 74,979 exports while
    the renderer lived inside it. This reads the export directory of
    `pill_core.dll` and `pill_engine_core.dll` in `modules/target/<profile>` and
    fails when either passes the threshold, so growth is noticed well before
    the build breaks.

USAGE
  python devops/tests/test_engine_exports.py [--profile <name>] [--threshold <count>]
  python devops/tests/test_engine_exports.py --self-test
    --profile    cargo profile directory under target/ (default: debug)
    --threshold  maximum allowed exports per DLL (default: 60000)
    --self-test  check the PE reader against a DLL from the Python install

EXAMPLE USAGE
  python devops/tests/test_engine_exports.py
  python devops/tests/test_engine_exports.py --profile development-optimized

  Exit status: 0 when every dylib is under the threshold, 1 when one is over,
  2 when a dylib is missing or unreadable.

--- SCRIPT ---
"""

import argparse
import struct
import sys
from pathlib import Path

MODULES = Path(__file__).resolve().parents[2] / "modules"

# The engine's shared libraries, by file stem.
ENGINE_DYLIBS = ("pill_core", "pill_engine_core")

# Default ceiling: the hard limit is 65,535; this leaves room to react.
DEFAULT_THRESHOLD = 60_000


class PortableExecutableError(Exception):
    """A file that is not a readable PE image."""


# Parses the PE headers; returns the section table and the data directories.
def parse_headers(data):
    if len(data) < 0x40:
        raise PortableExecutableError("too short for a DOS header")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe_offset:pe_offset + 4] != b"PE\0\0":
        raise PortableExecutableError("no PE signature")
    section_count = struct.unpack_from("<H", data, pe_offset + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    optional_offset = pe_offset + 24
    magic = struct.unpack_from("<H", data, optional_offset)[0]
    directory_offset = optional_offset + (112 if magic == 0x20B else 96)
    directories = [struct.unpack_from("<II", data, directory_offset + 8 * index) for index in range(16)]
    sections = []
    section_offset = optional_offset + optional_size
    for index in range(section_count):
        base = section_offset + 40 * index
        virtual_size, virtual_address, raw_size, raw_pointer = struct.unpack_from("<IIII", data, base + 8)
        sections.append((virtual_address, max(virtual_size, raw_size), raw_pointer))
    return sections, directories


# Converts a relative virtual address into a file offset.
def rva_to_offset(sections, rva):
    for virtual_address, size, raw_pointer in sections:
        if virtual_address <= rva < virtual_address + size:
            return rva - virtual_address + raw_pointer
    raise PortableExecutableError(f"RVA {rva:#x} is in no section")


# The number of names in a PE file's export directory (0 when it has none).
def export_count(path):
    data = path.read_bytes()
    sections, directories = parse_headers(data)
    export_rva, export_size = directories[0]
    if export_size == 0:
        return 0
    # IMAGE_EXPORT_DIRECTORY: NumberOfNames is the field at offset 24.
    return struct.unpack_from("<I", data, rva_to_offset(sections, export_rva) + 24)[0]


# Proves the reader finds a plausible export count in a DLL that ships with
# Python itself, and refuses a file that is not a PE image.
def self_test():
    candidates = sorted(Path(sys.executable).parent.glob("python3*.dll"))
    passed = True
    if candidates:
        count = export_count(candidates[0])
        outcome = "PASS" if count > 100 else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {candidates[0].name} reads as {count} exports")
    else:
        print("  SKIP  no python3*.dll beside the interpreter")
    try:
        export_count(Path(__file__))
        print("  FAIL  a non-PE file was accepted")
        passed = False
    except PortableExecutableError:
        print("  PASS  a non-PE file is refused")
    return 0 if passed else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[1])
    parser.add_argument("--profile", default="debug")
    parser.add_argument("--threshold", type=int, default=DEFAULT_THRESHOLD)
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        return self_test()
    status = 0
    for stem in ENGINE_DYLIBS:
        path = MODULES / "target" / arguments.profile / f"{stem}.dll"
        try:
            count = export_count(path)
        except (OSError, PortableExecutableError) as error:
            print(f"  FAIL  {path}: {error} (build it first)")
            status = 2
            continue
        over = count > arguments.threshold
        print(f"  {'FAIL' if over else 'PASS'}  {stem}.dll: {count} exports (threshold {arguments.threshold})")
        if over and status == 0:
            status = 1
    return status


if __name__ == "__main__":
    sys.exit(main())
