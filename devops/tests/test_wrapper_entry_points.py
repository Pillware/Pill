#!/usr/bin/env python3
"""
Wrapper entry points check: a wrapper DLL exports the whole loadable-artifact
entry-point set.

PROTECTS: A wrapper DLL exports the full loadable-artifact entry-point set.
TIER: build

REQUIREMENTS: Python 3.8+, cargo on PATH. Windows (the reader parses PE
  export directories).

DESCRIPTION
    `#[pill_module]` emits its loadable-artifact exports (`pill_module_init`,
    the ABI revision, the registration half, the descriptor count/copy pairs,
    the hot-patch resolvers) through `__pill_module_entry_points!`, a macro the generated
    `host_module_<name>` wrapper crate expands - the extension itself is a
    plain library and defines no symbols. The wrapper's DLL must carry the
    whole set, because the host resolves them by symbol exactly as it did
    before the wrapper existed.

    The script writes `modules/extensions/pill_test/generated/host_module_wrapper_probe` (a
    throwaway workspace member wrapping `pill_test`), builds it, reads
    the DLL's export directory and compares the `pill_*` names against the
    expected set. The probe is deleted again afterwards, unless `--keep-probe`
    is passed; it lives under the gitignored `host_module_*` name space.

USAGE
  python devops/tests/test_wrapper_entry_points.py [--keep-probe]
  python devops/tests/test_wrapper_entry_points.py --self-test
    --keep-probe  leave the generated wrapper member in place
    --self-test   check the export-name reader against a DLL from the Python
                  install, and against a non-PE file

EXAMPLE USAGE
  python devops/tests/test_wrapper_entry_points.py

  Exit status: 0 when the export sets match, 1 when they differ, 2 when a
  build or a DLL read fails.

--- SCRIPT ---
"""

import argparse
import shutil
import struct
import subprocess
import sys
from pathlib import Path

# Shared with the export-count check: the PE header and RVA readers.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_engine_exports import PortableExecutableError, parse_headers, rva_to_offset  # noqa: E402

MODULES = Path(__file__).resolve().parents[2] / "modules"
PROBE_DIRECTORY = (
    MODULES / "extensions" / "pill_test" / "generated" / "host_module_wrapper_probe"
)

# The extension both builds compile; it carries no extra features, so the
# check isolates what the macro emits.
FIXTURE_EXTENSION = "pill_test"

# Every name a wrapped extension must export; the set the host's loader and
# the patch machinery resolve by symbol. Exact equality is the check: a
# missing entry point fails the load, and an extra `pill_*` name means the
# wrapper or the macro emitted something nobody expects.
EXPECTED_ENTRY_POINTS = frozenset({
    "pill_module_init",
    "pill_module_abi_version",
    "pill_module_name",
    # Emitted whenever the module names a `register` half, in every profile:
    # the macro passes it an empty gate
    # (`module_register_export(&quote! {}, ...)`), so it is neither a
    # development-only export nor tied to the hot-patch machinery.
    "pill_module_register",
    "pill_hot_resolve",
    "pill_hot_resolve_install",
    "pill_hot_resolve_plain",
    "pill_hot_resolve_reset",
    "pill_hot_resolve_address",
    "pill_hot_resolve_extent_coverage",
    "pill_value_type_descriptor_count",
    "pill_copy_value_type_descriptors",
    "pill_mirror_method_descriptor_count",
    "pill_copy_mirror_method_descriptors",
    # No mirror trampoline is exported: each descriptor carries its
    # trampoline's address, so the host reads it from the descriptor rather
    # than resolving a symbol, and two crates mirroring same-named types
    # cannot collide in a statically linked build.
    "pill_field_accessor_descriptor_count",
    "pill_copy_field_accessor_descriptors",
})

# The throwaway wrapper member, as `devops/tests/test_wrapper_entry_points.py`
# writes it. Mirrors a host-generated `host_module_*` crate: the loadable
# artifact is the wrapper's cdylib, the extension a plain library inside it.
PROBE_MANIFEST = """[package]
name = "host_module_wrapper_probe"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true

[lib]
name = "host_module_wrapper_probe"
crate-type = ["cdylib", "rlib"]

[dependencies]
pill_engine = { path = "../../../../engine/pill_engine", features = ["development"] }
pill_test = { path = "../../modules/runtime", default-features = false }
"""

PROBE_SOURCE = """//! Loadable-artifact wrapper for the `pill_test` extension.
//!
//! # Responsibilities
//!
//! - Builds the cdylib the export comparison reads, from `pill_test`
//!   compiled as a plain library.
//! - Carries the extension's entry points through
//!   `__pill_module_entry_points!`, the macro `#[pill_module]` exports beside
//!   its gated items.
//!
//! The test writes this crate before the comparison and deletes it again
//! afterwards; nothing else builds it.

pill_test::__pill_module_entry_points!();
"""


class BuildError(Exception):
    """A cargo build the comparison needs failed."""


# The export names a PE file lists, flattened from its name-pointer table.
def export_names(path):
    data = path.read_bytes()
    sections, directories = parse_headers(data)
    export_rva, export_size = directories[0]
    if export_size == 0:
        return []
    directory_offset = rva_to_offset(sections, export_rva)
    # IMAGE_EXPORT_DIRECTORY: NumberOfNames at offset 24; AddressOfNames (an
    # RVA to the table of name RVAs) at offset 32.
    name_count = struct.unpack_from("<I", data, directory_offset + 24)[0]
    names_rva = struct.unpack_from("<I", data, directory_offset + 32)[0]
    names_offset = rva_to_offset(sections, names_rva)
    names = []
    for index in range(name_count):
        name_rva = struct.unpack_from("<I", data, names_offset + 4 * index)[0]
        offset = rva_to_offset(sections, name_rva)
        end = data.index(b"\0", offset)
        names.append(data[offset:end].decode("ascii", "replace"))
    return names


# Builds one cargo selection from the workspace root.
def run_cargo(arguments, description):
    completed = subprocess.run(
        ["cargo", "build", "--offline", *arguments],
        cwd=str(MODULES), capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    if completed.returncode != 0:
        raise BuildError(f"{description} failed:\n{completed.stderr[-1500:]}")


# Writes the probe member so `cargo build -p host_module_wrapper_probe` finds
# it; the workspace's `extensions/*/generated/*` glob picks it up.
def write_probe():
    (PROBE_DIRECTORY / "src").mkdir(parents=True, exist_ok=True)
    (PROBE_DIRECTORY / "Cargo.toml").write_text(PROBE_MANIFEST, encoding="utf-8")
    (PROBE_DIRECTORY / "src" / "lib.rs").write_text(PROBE_SOURCE, encoding="utf-8")


# Proves the reader finds a plausible name list in a DLL that ships with
# Python itself, and refuses a file that is not a PE image.
def self_test():
    candidates = sorted(Path(sys.executable).parent.glob("python3*.dll"))
    passed = True
    if candidates:
        names = export_names(candidates[0])
        outcome = "PASS" if len(names) > 100 else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {candidates[0].name} lists {len(names)} export names")
    else:
        print("  SKIP  no python3*.dll beside the interpreter")
    try:
        export_names(Path(__file__))
        print("  FAIL  a non-PE file was accepted")
        passed = False
    except PortableExecutableError:
        print("  PASS  a non-PE file is refused")
    return 0 if passed else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[1])
    parser.add_argument("--keep-probe", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        return self_test()
    if sys.platform != "win32":
        print("  SKIP  the export comparison reads PE files (Windows only)")
        return 0

    try:
        write_probe()
        run_cargo(["-p", "host_module_wrapper_probe"], "the wrapper probe build")
        wrapper_path = MODULES / "target" / "debug" / "host_module_wrapper_probe.dll"
        wrapper_names = {name for name in export_names(wrapper_path) if name.startswith("pill_")}
    except (BuildError, OSError, PortableExecutableError) as error:
        print(f"  FAIL  {error}")
        return 2
    finally:
        if not arguments.keep_probe:
            shutil.rmtree(PROBE_DIRECTORY, ignore_errors=True)
            # The probe's `generated/` folder too, when the probe was all it held.
            try:
                PROBE_DIRECTORY.parent.rmdir()
            except OSError:
                pass

    missing = sorted(EXPECTED_ENTRY_POINTS - wrapper_names)
    unexpected = sorted(wrapper_names - EXPECTED_ENTRY_POINTS)
    if missing or unexpected:
        print("  FAIL  the wrapper DLL does not export the expected entry-point set")
        if missing:
            print(f"        missing:    {missing}")
        if unexpected:
            print(f"        unexpected: {unexpected}")
        return 1
    print(f"  PASS  the wrapper DLL exports all {len(EXPECTED_ENTRY_POINTS)} entry points")
    return 0


if __name__ == "__main__":
    sys.exit(main())
