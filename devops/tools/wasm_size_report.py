#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+ (standard library only). `twiggy` on PATH for the
#               per-crate and per-symbol breakdown (`cargo install twiggy`);
#               without it only the section table is printed.

# DESCRIPTION: Explains what a project's web build is made of. Reads the
#   module `build_web.py` produced (`<project>/build/web/pkg/
#   pill_web_app_bg.wasm`) and the binary cargo wrote before wasm-bindgen and
#   wasm-opt processed it (`build/build_meta/web_target/wasm32-unknown-unknown/
#   <profile>/pill_web_app.wasm`), and prints:
#
#     1. Sizes: the shipped module, its gzip size (what a browser downloads
#        from a server that compresses), the pre-optimization binary and what
#        wasm-opt removed.
#     2. The shipped module's sections (code, data, ...), read from the file
#        itself, so exact.
#     3. Where the code comes from, by crate, from twiggy on the
#        pre-optimization binary: only that one still has function names.
#        Grouped as engine crates, the project, Rust's standard library,
#        third-party crate families and wasm internals. Each code group also
#        gets an estimate of its shipped size: its share of the
#        pre-optimization code applied to the shipped code section. wasm-opt
#        does not shrink every crate equally, so this is an estimate.
#     4. The embedded asset pack (`res/` packed into the module), measured
#        from the pack file the build wrote.
#     5. The largest symbols.
#
#   twiggy reports shallow sizes: code generic over a type is counted where
#   the generic is defined, so a `Vec<T>` method instantiated for an engine
#   type lands under Rust's standard library.
#
#   The pre-optimization binary lives in a target directory every project's
#   web build shares. A binary written after the shipped module belongs to a
#   later build of another project, and is skipped with a warning; rebuild the
#   project (or pass `--build`) to analyse it.
#
# USAGE: python devops/tools/wasm_size_report.py [options] [project_path]
#          [project_path]       workspace-relative project directory; defaults
#                               to the PROJECT_PATH environment variable
#          --build              run `build_web.py` (release) first; it
#                               regenerates build/pill_shipping_bundle
#          --wasm PATH          analyse this shipped module instead
#          --pre-opt PATH       use this pre-optimization binary instead
#          --top N              symbols to list (default 15)
#          --crates N           third-party families to list (default 20)
#          --max-size-kb N      fail (exit 1) when the shipped module is larger
#
# EXAMPLE USAGE:
#   python devops/tools/wasm_size_report.py examples/local/cube
#   python devops/tools/wasm_size_report.py --build devops/tests/projects/scene
#   python devops/tools/wasm_size_report.py --max-size-kb 2500 examples/local/cube

# --- SCRIPT ---

# Standard library
import argparse
import gzip
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

# Locations, relative to the repository root.
BUILD_WEB_SCRIPT = Path("devops") / "tools" / "build_web.py"
WEB_TARGET_DIRECTORY = Path("build") / "build_meta" / "web_target" / "wasm32-unknown-unknown"
# The module wasm-pack writes, and the binary cargo wrote before it.
SHIPPED_MODULE = Path("build") / "web" / "pkg" / "pill_web_app_bg.wasm"
PRE_OPTIMIZATION_NAME = "pill_web_app.wasm"
ASSET_PACK_NAME = "assets.pillpack"

# How much later than the pre-optimization binary the shipped module may be
# written by the same build (wasm-bindgen and wasm-opt run in between).
SAME_BUILD_WINDOW_SECONDS = 15 * 60

# Wasm section ids, by the specification's names.
SECTION_NAMES = {
    0: "custom",
    1: "type",
    2: "import",
    3: "function",
    4: "table",
    5: "memory",
    6: "global",
    7: "export",
    8: "start",
    9: "element",
    10: "code",
    11: "data",
    12: "datacount",
    13: "tag",
}

# Crates that are Rust itself; their code is the runtime every binary carries.
RUST_STANDARD_CRATES = {
    "core", "alloc", "std", "compiler_builtins", "rustc_demangle", "dlmalloc", "panic_abort",
    "panic_unwind", "str", "bool", "char", "u8", "u16", "u32", "u64", "u128", "i8", "i16",
    "i32", "i64", "i128", "f32", "f64", "usize", "isize", "T",
}

# Third-party crates reported together, as the family a reader recognises.
CRATE_FAMILIES = {
    "image": (
        "image", "jpeg_decoder", "zune_jpeg", "zune_core", "zune_inflate", "png", "fdeflate",
        "tiff", "gif", "weezl", "miniz_oxide", "color_quant", "qoi", "exr", "image_webp",
        "simd_adler32", "crc32fast", "adler2", "adler", "bytemuck",
    ),
    "wgpu": (
        "wgpu", "wgpu_core", "wgpu_hal", "wgpu_types", "naga", "codespan_reporting", "spirv",
        "pp_rs",
    ),
    "regex": ("regex", "regex_syntax", "regex_automata", "regex_lite", "aho_corasick"),
    "tracing": (
        "tracing", "tracing_core", "tracing_subscriber", "tracing_log", "sharded_slab",
        "matchers", "nu_ansi_term", "thread_local",
    ),
    "serde": ("serde", "serde_core", "serde_json", "serde_yaml", "ryu", "itoa"),
    "wasm-bindgen / web-sys": (
        "wasm_bindgen", "wasm_bindgen_futures", "js_sys", "web_sys",
    ),
    "rayon": ("rayon", "rayon_core", "crossbeam_deque", "crossbeam_epoch", "crossbeam_utils"),
    "hashbrown": ("hashbrown", "indexmap", "foldhash", "ahash"),
    "error reporting": ("miette", "thiserror", "anyhow", "backtrace", "owo_colors"),
    "tobj": ("tobj",),
    "winit": ("winit", "raw_window_handle", "dpi", "cursor_icon", "smol_str"),
}
FAMILY_OF_CRATE = {crate: family for family, crates in CRATE_FAMILIES.items() for crate in crates}

# Item groups that are not code from any crate.
FUNCTION_NAMES = "function names (debug, not shipped)"
BINDGEN_METADATA = "wasm-bindgen metadata (not shipped)"
CONSTANT_DATA = "constant data (.rodata, .data)"
CUSTOM_SECTIONS = "other custom sections"
WASM_STRUCTURE = "wasm structure (types, imports, tables)"
BINDGEN_GLUE = "wasm-bindgen glue"
UNATTRIBUTED = "unattributed code"


# =============================================================================
# Wasm sections
# =============================================================================


def read_leb128(data: bytes, offset: int) -> tuple:
    """Reads an unsigned LEB128 number; returns (value, next offset)."""
    value = 0
    shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return value, offset
        shift += 7


def read_sections(path: Path) -> list:
    """The sections of a wasm module as (name, size in bytes, item count).

    The size counts the section's id and length header too, so the sizes add
    up to the file size minus the 8-byte preamble. A custom section is named
    after its own name; the item count is the vector length for the sections
    that hold one (functions, data segments, ...), else None.
    """
    data = path.read_bytes()
    if data[:4] != b"\0asm":
        raise ValueError(f"{path} is not a wasm module")
    sections = []
    offset = 8
    while offset < len(data):
        start = offset
        section_id = data[offset]
        payload_size, payload_start = read_leb128(data, offset + 1)
        offset = payload_start + payload_size
        name = SECTION_NAMES.get(section_id, f"unknown ({section_id})")
        count = None
        if section_id == 0:
            name_length, name_start = read_leb128(data, payload_start)
            custom_name = data[name_start : name_start + name_length].decode("utf-8", "replace")
            name = f"custom '{custom_name}'"
        elif section_id in (1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 13):
            count, _ = read_leb128(data, payload_start)
        sections.append((name, offset - start, count))
    return sections


# =============================================================================
# twiggy
# =============================================================================


def twiggy_items(path: Path) -> list:
    """Every item twiggy finds in `path`, as (shallow size, name), largest first.

    Returns an empty list when twiggy is missing or fails.
    """
    executable = shutil.which("twiggy")
    if executable is None:
        return []
    result = subprocess.run(
        [executable, "top", "-f", "json", str(path)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        print(f"warning: twiggy failed on {path}: {result.stderr.strip()[:300]}", file=sys.stderr)
        return []
    items = [
        (entry["shallow_size"], entry["name"])
        for entry in json.loads(result.stdout)
        if entry.get("shallow_size", 0) > 0
    ]
    items.sort(key=lambda item: item[0], reverse=True)
    return items


def leading_crate(name: str) -> str:
    """The crate an item name starts with, past `<`, `&` and `&mut `."""
    rest = name.lstrip("<")
    for prefix in ("&mut ", "&", "*const ", "*mut ", "dyn "):
        if rest.startswith(prefix):
            rest = rest[len(prefix) :]
            break
    identifier = ""
    for character in rest:
        if character.isalnum() or character == "_":
            identifier += character
        else:
            break
    return identifier


def classify(name: str, project_crate: str) -> tuple:
    """Sorts one twiggy item into (category, group).

    Categories: "engine", "project", "std", "third-party" (all code), and
    "internals" for what is not code from a crate.
    """
    # Not code: metadata, constant data and the module's own structure.
    if '"function names"' in name or "name section" in name:
        return "internals", FUNCTION_NAMES
    if "__wasm_bindgen_unstable" in name:
        return "internals", BINDGEN_METADATA
    if name.startswith("data segment") or name.startswith("data["):
        return "internals", CONSTANT_DATA
    if name.startswith("custom section") or name.startswith('"'):
        return "internals", CUSTOM_SECTIONS
    if name.startswith(("type[", "import ", "table[", "elem[", "global[", "memory[", "export ")):
        return "internals", WASM_STRUCTURE
    if "__wbg_" in name or "__wbindgen" in name or name.startswith("__externref"):
        return "internals", BINDGEN_GLUE

    # Code, by the crate its path starts with.
    crate = leading_crate(name)
    if not crate:
        return "internals", UNATTRIBUTED
    if crate == project_crate:
        return "project", crate
    if crate.startswith("pill_"):
        return "engine", crate
    # A lone capital is a generic parameter (`<F as FnOnce>::call_once`):
    # closure and trait shims the standard library defines.
    if crate in RUST_STANDARD_CRATES or (len(crate) == 1 and crate.isupper()):
        return "std", "Rust standard library"
    return "third-party", FAMILY_OF_CRATE.get(crate, crate)


# =============================================================================
# Locating the inputs
# =============================================================================


def repository_root() -> Path:
    """The repository root, located from this script."""
    return Path(__file__).resolve().parent.parent.parent


def project_crate_name(project_root: Path) -> str:
    """The project's crate name (the `[package]` name, `-` as `_`), or ""."""
    manifest = project_root / "Cargo.toml"
    if not manifest.is_file():
        return ""
    in_package = False
    for line in manifest.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_package = stripped == "[package]"
        elif in_package and stripped.startswith("name") and "=" in stripped:
            return stripped.split("=", 1)[1].strip().strip('"').replace("-", "_")
    return ""


def find_pre_optimization(root: Path, shipped: Path) -> Path:
    """The pre-optimization binary of the build that wrote `shipped`, or None.

    Release and dev builds write to different profile directories; the one
    written last before the shipped module, within the same-build window, is
    the match. A binary written after it belongs to another, later build.
    """
    shipped_time = shipped.stat().st_mtime
    best = None
    for profile in ("release", "debug"):
        candidate = root / WEB_TARGET_DIRECTORY / profile / PRE_OPTIMIZATION_NAME
        if not candidate.is_file():
            continue
        candidate_time = candidate.stat().st_mtime
        # A few seconds of slack: file times on network drives are coarse.
        if candidate_time > shipped_time + 5:
            print(
                f"warning: {candidate} was written after the shipped module, by a later "
                "web build (another project?); rebuild this project to analyse it",
                file=sys.stderr,
            )
            continue
        if shipped_time - candidate_time > SAME_BUILD_WINDOW_SECONDS:
            continue
        if best is None or candidate_time > best.stat().st_mtime:
            best = candidate
    return best


def find_asset_pack(root: Path, project_root: Path, pre_optimization: Path) -> Path:
    """The asset pack the build embedded, or None when the project has no `res`.

    The bundle's build script writes it into its `OUT_DIR`, under the same
    profile directory as the pre-optimization binary; the newest pack written
    no later than that binary is the one it embedded.
    """
    if pre_optimization is None or not (project_root / "res").is_dir():
        return None
    limit = pre_optimization.stat().st_mtime + 5
    packs = [
        pack
        for pack in (pre_optimization.parent / "build").glob(f"*/out/{ASSET_PACK_NAME}")
        if pack.stat().st_mtime <= limit
    ]
    return max(packs, key=lambda pack: pack.stat().st_mtime) if packs else None


# =============================================================================
# Printing
# =============================================================================


def size_text(size: float) -> str:
    """A byte count for people: B, KB or MB (binary units)."""
    if size >= 1024 * 1024:
        return f"{size / (1024 * 1024):.2f} MB"
    if size >= 1024:
        return f"{size / 1024:.1f} KB"
    return f"{int(size)} B"


def percent(part: float, whole: float) -> str:
    """`part` as a percentage of `whole`, zero-guarded."""
    return f"{100.0 * part / whole:5.1f}%" if whole else "  0.0%"


def shorten(text: str, length: int) -> str:
    """`text` cut to `length` characters, marking the cut."""
    return text if len(text) <= length else text[: length - 3] + "..."


def print_heading(title: str) -> None:
    """A section heading."""
    print()
    print(f"--- {title} ---")
    print()


def print_sections(sections: list, total: int) -> None:
    """The section table of the shipped module."""
    print(f"  {'section':<36} {'size':>10} {'share':>6}   items")
    for name, size, count in sorted(sections, key=lambda section: section[1], reverse=True):
        items = "" if count is None else str(count)
        print(f"    {shorten(name, 34):<34} {size_text(size):>10} {percent(size, total)}   {items}")
    print(f"    {'(preamble)':<34} {size_text(8):>10}")
    print(f"    {'---':<34} {size_text(total):>10} 100.0%")


def print_code_groups(
    title: str, groups: dict, code_total: int, shipped_code: int, limit: int = 0
) -> int:
    """One block of code groups with their pre-optimization share and the
    shipped-size estimate; returns the block's total."""
    rows = sorted(groups.items(), key=lambda row: row[1], reverse=True)
    if not rows:
        return 0
    hidden = rows[limit:] if limit else []
    rows = rows[:limit] if limit else rows
    block_total = sum(groups.values())
    print(f"  {title:<34} {'pre-opt':>10} {'share':>6} {'~shipped':>10}")
    for name, size in rows:
        estimate = shipped_code * size / code_total if code_total else 0
        print(
            f"    {shorten(name, 32):<32} {size_text(size):>10} "
            f"{percent(size, code_total)} {size_text(estimate):>10}"
        )
    if hidden:
        hidden_size = sum(size for _, size in hidden)
        estimate = shipped_code * hidden_size / code_total if code_total else 0
        print(
            f"    {f'({len(hidden)} more)':<32} {size_text(hidden_size):>10} "
            f"{percent(hidden_size, code_total)} {size_text(estimate):>10}"
        )
    estimate = shipped_code * block_total / code_total if code_total else 0
    print(
        f"    {'---':<32} {size_text(block_total):>10} "
        f"{percent(block_total, code_total)} {size_text(estimate):>10}"
    )
    print()
    return block_total


# =============================================================================
# Report
# =============================================================================


def report(
    shipped: Path,
    pre_optimization: Path,
    asset_pack: Path,
    project_crate: str,
    top: int,
    crate_limit: int,
) -> None:
    """Prints the whole report for one build."""
    shipped_bytes = shipped.read_bytes()
    shipped_size = len(shipped_bytes)
    gzip_size = len(gzip.compress(shipped_bytes, 9))
    sections = read_sections(shipped)
    shipped_code = sum(size for name, size, _ in sections if name == "code")

    # Step 1: the sizes.
    print_heading(f"Sizes ({shipped})")
    print(f"  shipped module         {size_text(shipped_size):>10}  ({shipped_size} bytes)")
    print(f"  gzip -9                {size_text(gzip_size):>10}  ({percent(gzip_size, shipped_size).strip()} of the module)")
    if pre_optimization is not None:
        pre_size = pre_optimization.stat().st_size
        print(f"  before wasm-bindgen    {size_text(pre_size):>10}  ({pre_optimization})")
        print(
            f"  removed by the tools   {size_text(pre_size - shipped_size):>10}  "
            f"({percent(pre_size - shipped_size, pre_size).strip()}: names, bindgen metadata, wasm-opt)"
        )
    if asset_pack is not None:
        pack_size = asset_pack.stat().st_size
        print(
            f"  embedded asset pack    {size_text(pack_size):>10}  "
            f"({percent(pack_size, shipped_size).strip()} of the module, inside the data section)"
        )

    # Step 2: the shipped module's sections.
    print_heading("Shipped module sections (exact)")
    print_sections(sections, shipped_size)

    # Step 3: where the code comes from.
    if pre_optimization is None:
        print()
        print("  (no pre-optimization binary from this build: no per-crate breakdown;")
        print("   rebuild the project, or pass --build)")
        return
    items = twiggy_items(pre_optimization)
    if not items:
        print()
        print("  (twiggy not found or failed: no per-crate breakdown; cargo install twiggy)")
        return

    categories = {"engine": {}, "project": {}, "std": {}, "third-party": {}, "internals": {}}
    for size, name in items:
        category, group = classify(name, project_crate)
        categories[category][group] = categories[category].get(group, 0) + size
    code_total = sum(
        sum(categories[category].values())
        for category in ("engine", "project", "std", "third-party")
    )

    print_heading(
        f"Code by crate (pre-optimization {size_text(code_total)} of code; "
        f"~shipped scales it to the shipped {size_text(shipped_code)} code section)"
    )
    engine = print_code_groups("Engine crates", categories["engine"], code_total, shipped_code)
    project_title = f"Project ({project_crate})" if project_crate else "Project"
    if categories["project"]:
        project = print_code_groups(project_title, categories["project"], code_total, shipped_code)
    else:
        project = 0
        print(f"  {project_title}: no code of its own left (inlined, or nothing but data)")
        print()
    standard = print_code_groups("Rust standard library", categories["std"], code_total, shipped_code)
    third_party = print_code_groups(
        "Third-party crates (by family)", categories["third-party"], code_total, shipped_code, crate_limit
    )
    print(f"  {'Summary':<34} {'pre-opt':>10} {'share':>6} {'~shipped':>10}")
    for title, size in (
        ("engine", engine),
        ("project", project),
        ("Rust standard library", standard),
        ("third-party", third_party),
    ):
        estimate = shipped_code * size / code_total if code_total else 0
        print(f"    {title:<32} {size_text(size):>10} {percent(size, code_total)} {size_text(estimate):>10}")

    print_heading("Not code (pre-optimization)")
    pre_size = pre_optimization.stat().st_size
    for name, size in sorted(categories["internals"].items(), key=lambda row: row[1], reverse=True):
        print(f"    {name:<44} {size_text(size):>10} {percent(size, pre_size)}")

    # Step 4: the largest symbols.
    print_heading(f"Largest code symbols (pre-optimization, top {top})")
    shown = 0
    for size, name in items:
        if classify(name, project_crate)[0] == "internals":
            continue
        print(f"  {size_text(size):>10} {percent(size, code_total)}  {shorten(name, 100)}")
        shown += 1
        if shown >= top:
            break


def main() -> int:
    """Builds (optionally) and analyses one project's web build."""
    parser = argparse.ArgumentParser(description="Explain what a web build's wasm module is made of.")
    parser.add_argument("project_path", nargs="?", default=os.environ.get("PROJECT_PATH", ""))
    parser.add_argument("--build", action="store_true", help="run build_web.py (release) first")
    parser.add_argument("--wasm", type=Path, help="shipped module to analyse")
    parser.add_argument("--pre-opt", type=Path, help="pre-optimization binary to use")
    parser.add_argument("--top", type=int, default=15, help="symbols to list")
    parser.add_argument("--crates", type=int, default=20, help="third-party families to list")
    parser.add_argument("--max-size-kb", type=int, help="fail when the module is larger")
    arguments = parser.parse_args()

    root = repository_root()
    if not arguments.project_path and arguments.wasm is None:
        print("error: no project given and PROJECT_PATH is not set", file=sys.stderr)
        return 2
    project_root = (root / arguments.project_path).resolve() if arguments.project_path else None

    # Step 1: build, when asked.
    if arguments.build:
        if project_root is None:
            print("error: --build needs a project", file=sys.stderr)
            return 2
        status = subprocess.call(
            [sys.executable, str(root / BUILD_WEB_SCRIPT), arguments.project_path], cwd=root
        )
        if status != 0:
            return status

    # Step 2: locate the inputs.
    shipped = arguments.wasm or project_root / SHIPPED_MODULE
    if not shipped.is_file():
        print(f"error: no web build at {shipped}; build it with build_web.py or pass --build", file=sys.stderr)
        return 1
    if arguments.pre_opt is not None and not arguments.pre_opt.is_file():
        print(f"error: no pre-optimization binary at {arguments.pre_opt}", file=sys.stderr)
        return 1
    pre_optimization = arguments.pre_opt or find_pre_optimization(root, shipped)
    asset_pack = find_asset_pack(root, project_root, pre_optimization) if project_root else None
    project_crate = project_crate_name(project_root) if project_root else ""

    # Step 3: report, then hold the module to its budget.
    report(shipped, pre_optimization, asset_pack, project_crate, arguments.top, arguments.crates)
    if arguments.max_size_kb is not None:
        limit = arguments.max_size_kb * 1024
        size = shipped.stat().st_size
        print()
        if size > limit:
            print(f"FAIL: the module ({size_text(size)}) exceeds the budget ({size_text(limit)})")
            return 1
        print(f"OK: the module ({size_text(size)}) is within the budget ({size_text(limit)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
