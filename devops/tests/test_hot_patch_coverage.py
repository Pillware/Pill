"""
Hot-patch coverage test for Pill.

PROTECTS: Body-only edits in every loaded crate are live-patched rather than reloaded.
TIER: host
SETS: hot-reload

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo)
  - `pill_standalone` built in its development posture, which pins
    builds it unless --skip-build is given)

DESCRIPTION
    Detects crates whose live-patch fast path silently stopped working.

    The failure this exists for leaves no trace a person would notice. When a
    patch cannot be built, the host reports it and falls back to a full reload -
    the edit still reaches the running process, just seconds later instead of
    milliseconds. Nothing breaks, no test fails, and the fast path can be dead
    for an entire crate without anyone finding out. That is exactly what
    happened: for every crate that declares cargo features, the replayed `rustc`
    command line was mis-tokenized and the patch never compiled. Two modules
    happened to be unaffected, so live patching looked healthy.

    So this test asserts the thing the console tells you and no other test
    checks: that an edit was delivered BY A PATCH, not by a reload.

    Crates are discovered rather than listed - the project and the extensions
     come from `devops/tests/projects/native/project_settings.yaml`, so a module
    added later is
    covered without touching this file. The function to edit is discovered too:
    the scanner looks for a numeric literal inside a function body, preferring
    an annotated function when the crate has one.

    Each crate is reported in one of five states:

      PATCHED      an edit was delivered by the fast path (the expected result)
      FELL BACK    the fast path refused or failed - the failure this test hunts
      TIMED OUT    the edit was made and no outcome ever came back, which is what
                   a watcher that stopped delivering looks like
      NO FAST PATH the crate has neither annotations nor a build script, so no
                   fast path exists for it (reported, not a failure)
      SKIPPED      no editable literal was found to drive the test with

    Exits non-zero when a crate with a fast path fell back or timed out, when the
    flag cache holds a split argument, or when not one crate was patched - a run
    in which no edit reached the fast path proves nothing about it.

USAGE
  python devops/tests/test_hot_patch_coverage.py [--timeout-scale S]
      [--skip-build] [--strict]
        --timeout-scale S  Multiply every timeout (slow machines)
        --skip-build       Assume pill_standalone is already built
        --strict           Also fail when a crate has NO FAST PATH at all

EXAMPLE USAGE
  python devops/tests/test_hot_patch_coverage.py
  python devops/tests/test_hot_patch_coverage.py --strict --timeout-scale 2

--- SCRIPT ---
"""

import argparse
import os
import re
import sys
import time
from pathlib import Path
from typing import Dict, List, Optional, Tuple

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core import suite_common as common  # noqa: E402
from core.suite_common import *  # noqa: E402,F401,F403
from core.workspace_graph import read_manifest  # noqa: E402

# =============================================================================
# Configuration
# =============================================================================

# The host prints one of these for every change it processes. Which one appears
# is the entire assertion: the first means the fast path delivered the edit, the
# rest mean it did not and a full reload picked the change up instead.
PATCH_APPLIED_PATTERN = re.compile(r"\[hot\] (\S+) LIVE (\d+) ms \(generation \d+ via ([\w+-]+)")
PATCH_REFUSED_TOKEN = "fast patch refused"
PATCH_FAILED_TOKEN = "patch failed"
RELOAD_TRIGGERED_TOKEN = "hot reload triggered"

# How long to wait for the host to report an outcome for one edit. Generous
# because the first patch of a session re-derives compiler flags (~1.2 s) and
# a fallback runs a real cargo build.
OUTCOME_TIMEOUT = 90
# How long a retry waits, after restoring the refused edit, for the fallback
# reload to finish before it edits the next candidate: a retry that lands
# mid-build is cancelled again, and again.
RETRY_SETTLE_TIMEOUT = 120
BUILD_TIMEOUT_SECONDS = 600

# Where the host keeps the compiler-flag caches this test also reads
# (`flags_cache_path` in `hot_patch/compile.rs`): inside the workspace's private
# build tree, not the system temporary directory. Two checkouts share `/tmp`, so
# a cache written by one was replayed by the other - which is why the host moved
# them here, and why reading `/tmp` would inspect a foreign run's file or none.
FLAGS_CACHE_DIRECTORY = MODULES_ROOT / "target" / "hot" / "build" / "hotpatch-flags"

# The tokenizer version stamped into the first line of every cache. A cache
# written by a different tokenizer holds tokens split a different way, so it is
# reported rather than read.
FLAGS_CACHE_FORMAT_VERSION = "pill-hotpatch-flags-v3"

# Functions never worth editing: load-time plumbing, and anything a patch is
# expected to refuse for reasons that are not a defect.
UNEDITABLE_FUNCTIONS = {"register", "main"}

# A numeric literal that is safe to nudge. Deliberately narrow: an integer
# without a suffix could be an index or a capacity, while a float literal in a
# body is almost always a tunable constant.
FLOAT_LITERAL_PATTERN = re.compile(r"\b(\d+)\.(\d+)\b")

# How far left of a literal to reach when making the edit text unique. Long
# enough to clear a repeated constant, short enough to stay on one line.
MAX_SNIPPET_LENGTH = 80


# =============================================================================
# Crate discovery
# =============================================================================


class Crate:
    """One crate the host loads, and how this test drives it."""

    def __init__(self, name: str, source_root: Path, kind: str) -> None:
        self.name = name
        self.source_root = source_root
        self.kind = kind
        self.has_build_script = (source_root.parent / "build.rs").is_file()
        self.annotated = False
        self.edit_file: Optional[Path] = None
        self.edit_from = ""
        self.edit_to = ""
        self.function = ""
        # The planned edits after the current one, in preference order: the
        # driver moves on through them when the host's verdict says the
        # chosen function can never be patched (see `exercise`).
        self.candidates: List[Tuple[Path, str, str, str]] = []
        self.outcome = "SKIPPED"
        self.detail = ""
        self.route = ""
        self.milliseconds = 0

    def use_next_candidate(self) -> bool:
        """Moves to the next planned candidate; False when none remain."""
        if not self.candidates:
            return False
        path, name, edit_from, edit_to = self.candidates.pop(0)
        self.edit_file, self.function = path, name
        self.edit_from, self.edit_to = edit_from, edit_to
        return True

    @property
    def has_fast_path(self) -> bool:
        """Whether any patch mechanism can reach this crate at all.

        An annotation gives a function its own dispatch slot; a build script
        gives the crate an address inventory the prologue route resolves
        against. With neither, an edit has no fast path and a reload is the
        correct - and only - outcome.
        """
        return self.annotated or self.has_build_script


def discover_crates() -> List[Crate]:
    """Reads the example project's settings file to find the project and every
    module.

    Discovered rather than listed so a module added to `project_settings.yaml`
    is covered without editing this file - the same convention the host itself
    follows when deciding what to load. The project is the native example
    (PROJECT_PATH drives it at runtime; discovery assumes the same default).
    """
    crates: List[Crate] = []
    project_root = NATIVE_PROJECT_ROOT
    crates.append(Crate(project_root.name, project_root / "src", "project"))

    # `modules:` is a YAML list of quoted crate names, each in an extension
    # folder under `extensions/`, ending at the next top-level key or end of file.
    config_text = read_source(project_settings_yaml(NATIVE_PROJECT_ROOT))
    modules_block = re.search(
        r"^modules:\s*$(.*?)(?=^\S|\Z)", config_text, re.MULTILINE | re.DOTALL
    )
    if modules_block:
        for name in re.findall(r'^\s*-\s*"([^"]+)"', modules_block.group(1), re.MULTILINE):
            crate_directory = next(
                (
                    candidate
                    for folder in extension_folders()
                    for candidate in (
                        folder / "modules" / "runtime",
                        folder / "modules" / "runtime" / name,
                        folder / "modules" / "editor",
                    )
                    if (candidate / "Cargo.toml").is_file()
                    and read_manifest(candidate / "Cargo.toml").get("package", {}).get("name") == name
                ),
                MODULES_ROOT / "extensions" / name / "modules" / "runtime",
            )
            crates.append(Crate(name, crate_directory / "src", "module"))
    return crates


def matching_brace(source: str, opening: int) -> Optional[int]:
    """The offset of the `}` that closes the `{` at `opening`, or None.

    Strings and comments are skipped so a brace inside either cannot unbalance
    the count.
    """
    depth, index, length = 0, opening, len(source)
    in_string = in_line_comment = in_block_comment = False
    while index < length:
        character = source[index]
        pair = source[index : index + 2]
        if in_line_comment:
            if character == "\n":
                in_line_comment = False
        elif in_block_comment:
            if pair == "*/":
                in_block_comment = False
                index += 1
        elif in_string:
            if character == "\\":
                index += 1
            elif character == '"':
                in_string = False
        elif pair == "//":
            in_line_comment = True
            index += 1
        elif pair == "/*":
            in_block_comment = True
            index += 1
        elif character == '"':
            in_string = True
        elif character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
            if depth == 0:
                return index
        index += 1
    return None


def find_function_bodies(source: str) -> List[Tuple[str, int, int, int]]:
    """Every `fn name(...) { ... }` in the file, as (name, keyword, body start, body end).

    A brace scanner rather than a parser: it only has to find a literal inside a
    body and the declaration a candidate belongs to, and being approximate costs
    nothing here - a mis-scan yields no candidate literal and the crate reports
    SKIPPED rather than a false failure.
    """
    bodies: List[Tuple[str, int, int, int]] = []
    for match in re.finditer(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[(<]", source):
        opening = source.find("{", match.end())
        if opening < 0:
            continue
        closing = matching_brace(source, opening)
        if closing is not None:
            bodies.append((match.group(1), match.start(), opening, closing))
    return bodies


def impl_block_ranges(source: str) -> List[Tuple[int, int]]:
    """Every `impl` body in the file, as (open brace, close brace) offsets.

    Approximate like the body scan: a `impl` inside a comment or a string can
    add a range that contains nothing, which only ever costs a candidate.
    """
    ranges: List[Tuple[int, int]] = []
    for match in re.finditer(r"\bimpl\b", source):
        opening = source.find("{", match.end())
        if opening < 0:
            continue
        closing = matching_brace(source, opening)
        if closing is not None:
            ranges.append((opening, closing))
    return ranges


def is_associated_without_receiver(
    source: str, keyword_start: int, body_start: int, impl_ranges: List[Tuple[int, int]]
) -> bool:
    """Whether this function is an associated one with no `Self` to bind.

    The host refuses those by construction: a patch compiles a replacement body
    against `Self`, and an associated function inside an `impl` has none, so the
    refusal reads `associated-without-receiver`. Offering one as a candidate
    reports a healthy crate as broken, which is exactly the false failure this
    test exists to avoid.
    """
    if not any(start <= keyword_start <= end for start, end in impl_ranges):
        return False
    return "self" not in source[keyword_start:body_start]


def declaration_is_annotated(source: str, body_start: int) -> bool:
    """Whether the declaration owning this body carries a hot-patch attribute.

    `#[system]` counts as one: the engine gives every declared system a
    dispatch slot in a development build, so its body is patchable without a
    marker of its own.
    """
    window = source[max(0, body_start - 400) : body_start]
    return (
        "#[pill_hot]" in window
        or "#[pill_hot_fn]" in window
        or "#[system]" in window
        or "#[system(" in window
    )


def runtime_code_end(source: str) -> int:
    """Where this file's runtime code stops and its test module begins.

    A `#[cfg(test)]` module is not in the running artifact, so editing a literal
    there changes nothing the host is executing: the watcher fires, the patch
    finds no live function to replace, and the crate would be reported as broken
    when nothing is wrong with it.
    """
    marker = source.find("#[cfg(test)]")
    return len(source) if marker < 0 else marker


def declaration_is_a_test(source: str, body_start: int) -> bool:
    """Whether this declaration is a test rather than shipped code."""
    window = source[max(0, body_start - 400) : body_start]
    return "#[test]" in window


def unique_snippet(source: str, literal_start: int, literal: str) -> Optional[str]:
    """The shortest text containing this literal that appears once in the file.

    A bare literal is rarely unique - `2.0` occurs all over a physics module -
    and replacing the first occurrence would edit a different function than the
    one being reported, turning a healthy crate into a false failure. Growing
    the window leftwards until the text is unique keeps the edit anchored to
    the intended line.

    A literal opening or closing an array ends its line before any text tells
    it apart from its fellows, so the fallback grows rightwards across the
    following lines instead. That is what lets a body whose only literals are
    uniform (`0.0`) stay usable; without it such a function is dropped even
    though its edit is as safe as any other.

    Returns `None` when no window up to `MAX_SNIPPET_LENGTH` is unique, which
    leaves the function as an unusable candidate rather than an unsafe edit.
    """
    literal_end = literal_start + len(literal)
    for reach in range(0, MAX_SNIPPET_LENGTH, 4):
        start = literal_start - reach
        if start < 0:
            break
        snippet = source[start:literal_end]
        # A snippet spanning a newline would still be anchored correctly, but
        # keeping it on one line makes the reported edit readable.
        if "\n" in snippet:
            break
        if source.count(snippet) == 1:
            return snippet
    for reach in range(4, MAX_SNIPPET_LENGTH, 4):
        end = literal_end + reach
        if end > len(source):
            break
        snippet = source[literal_start:end]
        if source.count(snippet) == 1:
            return snippet
    return None


def plan_edit(crate: Crate) -> bool:
    """Choose one function body and a literal inside it to nudge.

    Prefers an annotated function, because that exercises a dispatch slot - the
    route with the strongest guarantee - and because a crate that has one is
    unambiguously expected to patch. Falls back to any function when the crate
    has an address inventory instead.
    """
    candidates: List[Tuple[bool, Path, str, str, str]] = []
    for path in sorted(crate.source_root.rglob("*.rs")):
        source = read_source(path)
        runtime_end = runtime_code_end(source)
        impl_ranges = impl_block_ranges(source)
        for name, keyword_start, body_start, body_end in find_function_bodies(source):
            if name in UNEDITABLE_FUNCTIONS:
                continue
            # Only code the running artifact actually contains.
            if body_start >= runtime_end or declaration_is_a_test(source, body_start):
                continue
            if is_associated_without_receiver(source, keyword_start, body_start, impl_ranges):
                continue
            annotated = declaration_is_annotated(source, body_start)
            if annotated:
                crate.annotated = True
            # Only a function the host can actually reach. An annotation gives
            # the function its own slot; without one the prologue route needs
            # the crate's address inventory, which only a build script emits.
            # Editing anything else would be reported as broken when the crate
            # is simply out of scope for patching.
            if not (annotated or crate.has_build_script):
                continue
            for literal in FLOAT_LITERAL_PATTERN.finditer(source[body_start:body_end]):
                snippet = unique_snippet(source, body_start + literal.start(), literal.group(0))
                if snippet is None:
                    continue
                changed = snippet.replace(
                    literal.group(0), f"{int(literal.group(1)) + 1}.{literal.group(2)}", 1
                )
                candidates.append((annotated, path, name, snippet, changed))
                break

    if not candidates:
        return False
    candidates.sort(key=lambda entry: (not entry[0], str(entry[1]), entry[2]))
    # A patch refusal can say the chosen function is unpatchable by
    # construction (`associated-without-receiver`); keeping a few candidates
    # lets the run move on to the next one without guessing which functions
    # the host can carry.
    crate.candidates = [
        (path, name, snippet, changed) for _, path, name, snippet, changed in candidates[:6]
    ]
    crate.use_next_candidate()
    return True


# =============================================================================
# Driving the host
# =============================================================================


# The host is launched through cargo rather than by running the executable.
# `-C prefer-dynamic` means the binary needs the toolchain's `std-*.dll` and the
# workspace's `pill_core.dll` on the loader path, and `cargo run` is what sets
# that up - executed directly, the process dies in the loader before it can
# print anything. Building through the same posture also guarantees the binary
# carries the engine's `development` feature, which is the whole subject of this
# test.
#
# `rendering` is required rather than optional: devops/tests/projects/native links
# `pill_master_renderer`, so a host without it resolves `pill_core` differently
# from the project and the project DLL fails to load with "The specified
# procedure could not be found" (os error 127), the same reason the cascade
# suite pins it.
HOST_POSTURE = HOST_DEV_HOT_PATCH_RENDERING


def classify_outcome(output: str, crate: Crate) -> Tuple[str, str, str, int]:
    """Read the host's own report for one edit.

    Returns (outcome, detail, route, milliseconds). The host says exactly what
    it did, so this reads its verdict rather than inferring one from behaviour.
    """
    applied = PATCH_APPLIED_PATTERN.search(output)
    if applied:
        return "PATCHED", applied.group(1), applied.group(3), int(applied.group(2))

    # A refusal names a stable code and a sentence; both are worth surfacing,
    # because the whole point of this test is to say WHY a crate lost its fast
    # path rather than only that it did.
    for token in (PATCH_FAILED_TOKEN, PATCH_REFUSED_TOKEN):
        if token in output:
            detail = re.search(r'detail="(.*?)"(?:\s|$)', output, re.DOTALL)
            code = re.search(r'code="([^"]+)"', output)
            reason = detail.group(1)[:400] if detail else "no detail reported"
            return "FELL BACK", f"{code.group(1) if code else 'unknown'}: {reason}", "", 0
    if RELOAD_TRIGGERED_TOKEN in output:
        return "FELL BACK", "reloaded without reporting a fast-path outcome", "", 0
    return "SKIPPED", "the host reported nothing for this edit", "", 0


def exercise(crate: Crate, monitor: OutputMonitor, backups: BackupRegistry) -> None:
    """Make body-only edits and record how the host delivered the last one.

    An attempt decides more than the picker can tell up front: a chosen
    function can be unpatchable by construction - an associated function with
    no `Self` to carry the body (`associated-without-receiver`), or a body
    reaching crate-private state that the generated patch, compiled as its own
    crate, cannot name - and only asking reveals which. A fallback therefore
    moves the run on to the crate's next candidate; the last candidate's
    verdict is the crate's.
    """
    while True:
        previous = crate.function
        print(f"\n  [EDIT] {crate.name}: {crate.edit_file.name} "
              f"fn {crate.function}  {crate.edit_from} -> {crate.edit_to}")
        backups.capture(crate.edit_file)
        # The shared helpers keep line endings as stored, so only the edited
        # line changes and the host sees a body-only edit.
        original = read_source(crate.edit_file)
        edited = original.replace(crate.edit_from, crate.edit_to, 1)
        if edited == original:
            crate.outcome = "SKIPPED"
            crate.detail = f"edit text {crate.edit_from!r} not found as written"
            print(f"  [SKIP] {crate.name}: {crate.detail}")
            return
        # The anchor is taken before the write: the host can answer a body-only
        # edit within the write's own turn, and an outcome line counted into the
        # anchor would never appear in `output_since`.
        start_index = monitor.line_count
        atomic_write(crate.edit_file, edited)

        deadline = time.time() + OUTCOME_TIMEOUT
        while time.time() < deadline:
            output = monitor.output_since(start_index)
            outcome, detail, route, milliseconds = classify_outcome(output, crate)
            if outcome != "SKIPPED":
                crate.outcome, crate.detail = outcome, detail
                crate.route, crate.milliseconds = route, milliseconds
                break
            if not monitor.process_alive():
                crate.outcome, crate.detail = "FELL BACK", "the host exited during this edit"
                break
            # Poll until the host reports this edit's outcome.
            time.sleep(0.2)
        else:
            # Not `SKIPPED`: this crate has a fast path and the edit was
            # written, so the host had a chance to answer and did not.
            # Reported separately because a run of these is what a watcher
            # that stopped delivering looks like, and it must not read as
            # "nothing to test here".
            crate.outcome = "TIMED OUT"
            crate.detail = f"no outcome within {OUTCOME_TIMEOUT}s"

        # Restoring is itself a source change: the watcher fires again and the
        # host patches or reloads its way back. Waiting for that to finish is
        # what keeps the next measurement clean - without it the following
        # edit is classified against a snapshot the host has not caught up
        # with, and a healthy crate is reported as `outside-hot-body`.
        restore_index = monitor.line_count
        backups.restore_one(crate.edit_file)
        settle_deadline = time.time() + OUTCOME_TIMEOUT
        while time.time() < settle_deadline:
            output = monitor.output_since(restore_index)
            if classify_outcome(output, crate)[0] != "SKIPPED":
                break
            if not monitor.process_alive():
                break
            # Poll until the restore's own outcome arrives.
            time.sleep(0.2)
        # Then let the watcher go quiet before the next edit.
        time.sleep(1.0)

        if crate.outcome == "FELL BACK" and monitor.process_alive() and crate.candidates:
            if "associated-without-receiver" in crate.detail:
                reason = "an associated function with no receiver, which no patch can carry"
            else:
                reason = f"not patchable as written ({crate.detail})"
            crate.use_next_candidate()
            print(
                f"  [..] {crate.name}: `{previous}` is {reason}; trying "
                f"`{crate.function}` instead"
            )
            # The refused edit's fallback reload may still be building, and the
            # watcher cancels overlapping builds: a retry that lands mid-build
            # is cancelled again and again. Wait for the restored sources to
            # settle - the module reloading, or the restore patching back -
            # before the next candidate.
            retry_settle = time.time() + RETRY_SETTLE_TIMEOUT
            while time.time() < retry_settle:
                output = monitor.output_since(restore_index)
                if "module DLL loaded successfully" in output:
                    break
                if classify_outcome(output, crate)[0] == "PATCHED":
                    break  # the restore patched instantly; nothing to rebuild
                if not monitor.process_alive():
                    break
                # Poll until the module reloads or the restore patched back.
                time.sleep(0.2)
            # Let the cancelled-build churn settle before the retry.
            time.sleep(1.0)
            continue
        break

    marker = {"PATCHED": "[OK]", "FELL BACK": "[FAIL]", "TIMED OUT": "[FAIL]"}.get(
        crate.outcome, "[SKIP]"
    )
    summary = (
        f"{crate.route}, {crate.milliseconds} ms" if crate.outcome == "PATCHED" else crate.detail
    )
    print(f"  {marker} {crate.name}: {crate.outcome} - {summary}")


def inspect_flag_caches(since: float) -> Tuple[List[str], List[str], int]:
    """Look for split arguments in the compiler-flag caches the host wrote.

    A second, narrower net than the outcome check above, and the only one that
    does not depend on cargo's mood. Cargo chooses between single and double
    quoting for the same argument from run to run, so a tokenizer that mishandles
    one style shows up as a fallback only sometimes - which is exactly how the
    original bug survived. These files are the tokenizer's output, so a split
    argument is visible in them whether or not it happened to break a patch this
    time.

    A cache holds one token per line. A token that begins with a quote, or ends
    with an unbalanced one, is a fragment of a value that should have stayed
    whole.

    Only caches this run wrote are read: an older file describes a build that
    happened before the run, and reporting its tokens as this run's would be a
    failure nobody can act on. Returns (problems, notes, caches read).
    """
    problems: List[str] = []
    notes: List[str] = []
    inspected = 0
    for cache in sorted(FLAGS_CACHE_DIRECTORY.glob("*.flags")):
        try:
            if cache.stat().st_mtime < since:
                continue
            lines = cache.read_text(encoding="utf-8").splitlines()
        except OSError as error:
            notes.append(f"{cache.name}: unreadable ({error}), not inspected")
            continue
        # Line 0 is the tokenizer version, line 1 the build command, line 2 the
        # compiler; the arguments follow.
        if not lines or lines[0] != FLAGS_CACHE_FORMAT_VERSION:
            notes.append(
                f"{cache.name}: written by a different tokenizer "
                f"({lines[0][:40] if lines else 'empty'!r}), not inspected"
            )
            continue
        inspected += 1
        for token in lines[3:]:
            if token.startswith(('"', "'")) or token.count('"') % 2 == 1:
                problems.append(
                    f"{cache.name}: token {token[:60]!r} looks like half an argument"
                )
    return problems, notes, inspected


# =============================================================================
# Reporting
# =============================================================================


def print_report(crates: List[Crate], strict: bool, run_started_at: float) -> bool:
    """Print the coverage matrix and decide the exit status."""
    print("\n" + "=" * 78)
    print("  HOT-PATCH COVERAGE")
    print("=" * 78)
    print(f"  {'crate':24s} {'kind':8s} {'fast path':11s} {'result':11s} detail")
    print("  " + "-" * 74)
    for crate in crates:
        capability = "annotated" if crate.annotated else (
            "inventory" if crate.has_build_script else "none"
        )
        detail = (
            f"{crate.route}, {crate.milliseconds} ms"
            if crate.outcome == "PATCHED"
            else crate.detail[:34]
        )
        print(f"  {crate.name:24s} {crate.kind:8s} {capability:11s} "
              f"{crate.outcome:11s} {detail}")

    fragments, cache_notes, caches_read = inspect_flag_caches(run_started_at)
    if fragments:
        print("\n  [FAIL] The compiler-flag cache holds split arguments, so the")
        print("         replayed rustc line is malformed for at least one crate:")
        for problem in fragments:
            print(f"         {problem}")

    fell_back = [crate for crate in crates if crate.outcome == "FELL BACK"]
    timed_out = [crate for crate in crates if crate.outcome == "TIMED OUT"]
    no_fast_path = [crate for crate in crates if not crate.has_fast_path]
    skipped = [crate for crate in crates if crate.outcome == "SKIPPED"]
    patched = [crate for crate in crates if crate.outcome == "PATCHED"]
    print()
    print(f"  {len(patched)} patched, {len(fell_back)} fell back, "
          f"{len(timed_out)} timed out, {len(no_fast_path)} without a fast path, "
          f"{len(skipped)} not exercised")
    print(f"  {caches_read} compiler-flag cache(s) written by this run inspected")

    print()
    if not patched:
        print("  [FAIL] No crate was patched, so this run says nothing about the")
        print("         fast path: either no edit reached the host or every edit")
        print("         was delivered some other way.")
    if fell_back:
        print(f"  [FAIL] {len(fell_back)} crate(s) lost the fast path:")
        for crate in fell_back:
            print(f"         {crate.name}: {crate.detail}")
    if timed_out:
        print(f"  [FAIL] {len(timed_out)} crate(s) with a fast path reported no")
        print("         outcome at all - the watcher is not delivering edits:")
        for crate in timed_out:
            print(f"         {crate.name}: {crate.detail}")
    if no_fast_path:
        print(f"  [NOTE] {len(no_fast_path)} crate(s) have no fast path at all "
              f"(no annotation, no build script):")
        for crate in no_fast_path:
            print(f"         {crate.name} - add a build.rs calling "
                  f"pill_hot_scan::generate_function_inventory()")
    if skipped:
        print(f"  [NOTE] {len(skipped)} crate(s) could not be exercised:")
        for crate in skipped:
            print(f"         {crate.name}: {crate.detail}")
    for note in cache_notes:
        print(f"  [NOTE] {note}")
    cache_unread = bool(patched) and caches_read == 0
    if cache_unread:
        print("  [FAIL] A patch landed but no compiler-flag cache was read:")
        print("         the narrow net saw nothing, so the replay is unproven.")

    failed = (
        bool(fell_back)
        or bool(fragments)
        or bool(timed_out)
        or not patched
        or cache_unread
        or (strict and bool(no_fast_path))
    )
    return not failed


# =============================================================================
# Main
# =============================================================================


def main() -> None:
    """Discover the crates, exercise each one, and report."""
    parser = argparse.ArgumentParser(
        description="Detect crates whose live-patch fast path stopped working"
    )
    parser.add_argument("--strict", action="store_true",
                        help="Also fail when a crate has no fast path at all")
    add_common_suite_arguments(parser)
    args = parser.parse_args()

    global OUTCOME_TIMEOUT
    OUTCOME_TIMEOUT = int(OUTCOME_TIMEOUT * args.timeout_scale)
    startup_timeout = int(STARTUP_TIMEOUT * args.timeout_scale)
    # Every flag cache this run writes is newer than this, so an older file - a
    # previous run's, or another checkout's - is not read as this run's output.
    run_started_at = time.time()

    print("=" * 78)
    print("  Hot-Patch Coverage Test")
    print(f"  Workspace: {WORKSPACE_ROOT}")
    print("=" * 78)

    ensure_host_lock()
    kill_stale_hosts()
    if not prepare_host(HOST_POSTURE, args, BUILD_TIMEOUT_SECONDS):
        sys.exit(1)

    crates = discover_crates()
    if not crates:
        print("  [FAIL] No crates discovered from project_settings.yaml.")
        sys.exit(1)
    print(f"\n  Discovered {len(crates)} crate(s): "
          f"{', '.join(crate.name for crate in crates)}")

    for crate in crates:
        if plan_edit(crate):
            continue
        # Told apart deliberately. A crate with no annotation and no build
        # script has no fast path to lose, which is a coverage gap worth naming;
        # one that has a fast path but offered nothing editable is a limitation
        # of this test, not of the engine.
        if crate.has_fast_path:
            crate.outcome = "SKIPPED"
            crate.detail = "no editable float literal in a patchable function"
        else:
            crate.outcome = "NO FAST PATH"
            crate.detail = "no annotation and no build.rs inventory"

    environment = os.environ.copy()
    environment["PROJECT_PATH"] = NATIVE_PROJECT_PATH
    process, monitor = launch_process(host_command(HOST_POSTURE), MODULES_ROOT, environment)
    backups = BACKUP
    passed = False
    try:
        if not monitor.wait_for(STARTUP_TOKEN, startup_timeout):
            print(f"  [FAIL] Host did not start within {startup_timeout}s.")
            sys.exit(1)
        print("  [OK] Host running.\n")

        # One warm-up edit before measuring anything. The first patch of a
        # session re-derives compiler flags and can restage artifacts, so its
        # outcome reflects session startup rather than the crate under test.
        warm_up = next((crate for crate in crates if crate.edit_file), None)
        if warm_up:
            print("  [WARMUP] One discarded edit, so the flags cache is populated.")
            exercise(warm_up, monitor, backups)
            warm_up.outcome, warm_up.detail = "SKIPPED", ""
            warm_up.route, warm_up.milliseconds = "", 0

        for crate in crates:
            if crate.edit_file:
                exercise(crate, monitor, backups)
        passed = print_report(crates, args.strict, run_started_at)
    finally:
        print("\n  [CLEANUP] Restoring sources and stopping the host...")
        backups.restore_all()
        terminate_process(process, monitor)
        print("  [OK] Restored.")

    print("\n" + "=" * 78)
    print("  TEST PASSED" if passed else "  TEST FAILED")
    print("=" * 78)
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    run_suite_with_timing(main)
