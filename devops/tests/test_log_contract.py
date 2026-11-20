"""
Guards the host log strings that the end-to-end suites match on.

PROTECTS: Every host log token a suite waits for still exists in the Rust sources.
TIER: static
SETS: hot-reload

REQUIREMENTS
  - Python 3.8+
  - No Rust toolchain, no host process, no .NET. Reads the Rust sources, so a
    run takes a few seconds.

DESCRIPTION
    Every end-to-end suite in this directory decides pass or fail by searching
    host console output for a literal token, declared as a `*_TOKEN` constant.
    Those constants make the host's log text an interface - but nothing on the
    Rust side says so, so a refactor that rewords a log line breaks the suites
    with no compiler error and no unit-test failure.

    That is not hypothetical. Unifying the project and extension reload
    paths behind one `ReloadTransaction` collapsed three distinct messages:

        "hot reload complete"                  also emitted by modules now
        "no longer registered by the project"  became "...by this module"
        "schema unchanged for all persistable component types"
                                               became "...persistable module types"

    The full workspace test suite stayed green at 412 passed. Only launching a
    host would have caught it, minutes into a run, as a confusing "no reload
    detected".

    This test closes that gap statically. It collects every `*_TOKEN` literal
    the suites define, plus every module-level string constant a
    `wait_for(...)` call passes whatever its name, and requires each one to be
    printable by a Rust string literal - in `modules/` or in the tracked
    test projects, which the host also runs: verbatim, or through the
    `{...}` placeholders a format string carries, since most host lines are
    built at runtime:

        "| module=pill_spline"  <-  "... | size={} | module={} | kind={} ..."

    A placeholder stands for one or more non-whitespace characters, and a token
    may start and stop in the middle of the literal, because a suite waits for
    the field it cares about and the host prints others around it. A match
    must carry MINIMUM_ANCHOR_CHARACTERS of static text, so that a template the
    log layer applies to every line - a formatter's `[{level}] {location}
    {message}` - cannot vacuously cover a token it has nothing to do with.

    A token no literal can print must be listed in EXTERNALLY_PRODUCED below
    with the reason it cannot be, so that exemptions are stated rather than
    assumed; a token another language prints whose shape a Rust format string
    fits by coincidence goes in SHAPE_ONLY_FITS, so that the coincidence does
    not read as coverage. The same scan rejects a log line written inline in a
    `wait_for(...)` call, where no constant makes it visible here.

USAGE
  python devops/tests/test_log_contract.py

EXAMPLE USAGE
  python devops/tests/test_log_contract.py

--- SCRIPT ---
"""

# Standard library
import io
import re
import subprocess
import sys
import tokenize
from functools import lru_cache
from pathlib import Path
from typing import Callable, Dict, Iterator, List, NamedTuple, Optional, Set, Tuple

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.suite_common import run_suite_with_timing  # noqa: E402

# =============================================================================
# Constants
# =============================================================================

# Repository root, two levels above this file (devops/tests/<this>).
REPOSITORY_ROOT = Path(__file__).resolve().parents[2]

# Directories whose Python modules declare host-output tokens.
TOKEN_SOURCE_DIRECTORIES = ("devops/tests", "devops/core")

# Matches `SOME_TOKEN = "literal"` at module level, capturing name and literal.
TOKEN_DEFINITION_RE = re.compile(
    r"^([A-Z][A-Z0-9_]*TOKEN[A-Z0-9_]*)\s*=\s*(\"|')(?P<literal>.*?)\2\s*$",
    re.MULTILINE,
)

# Any other module-level uppercase string constant. A constant with no TOKEN in
# its name still joins the contract when a `wait_for(...)` call passes it: the
# call site, not the spelling, is what makes the text an interface.
STRING_ASSIGNMENT_RE = re.compile(
    r"^([A-Z][A-Z0-9_]*)\s*=\s*(\"|')(?P<literal>[^\"'\n]*)\2\s*$",
    re.MULTILINE,
)

# A `{...}` placeholder in a Rust format string: `{id}`, `{:?}`, `{value:.1}`.
PLACEHOLDER_RE = re.compile(r"\{[^{}]*\}")

# The fewest non-whitespace characters of static text a match must carry. A
# template the log layer applies to every line (`[{level}] {location}
# {message}`) or to every structured field (`{} {} {}`) would otherwise
# "cover" any token that happened to fit its shape.
MINIMUM_ANCHOR_CHARACTERS = 4

# The token kinds that open an inline literal argument (`"..."`, `f"..."`): a
# wait written with one of these hides its text from `collect_declared_tokens`.
# Python before 3.12 tokenises an f-string as one STRING, with no FSTRING_START.
INLINE_LITERAL_TOKENS = frozenset(
    {tokenize.STRING, getattr(tokenize, "FSTRING_START", -1)}
)

# Escapes rustc resolves in a normal string literal, so that a token carrying a
# quote or a backslash still matches the source text that prints it.
STRING_ESCAPES: Dict[str, str] = {
    "n": "\n",
    "r": "\r",
    "t": "\t",
    "0": "\0",
    '"': '"',
    "'": "'",
    "\\": "\\",
}

# Tokens that legitimately never appear in Rust source, each with the reason.
# Anything not listed here must be printable by a Rust literal, so that a
# reworded log line fails this test instead of an end-to-end suite.
#
# The wildcard match is an approximation: a token whose shape an unrelated
# format string happens to fit counts as printable. A token that really is
# printed by another language belongs in SHAPE_ONLY_FITS below instead, where
# the coincidence is stated rather than read as coverage.
EXTERNALLY_PRODUCED: Dict[str, str] = {
    "STATUS_ACCESS_VIOLATION": "Windows prints this exit status, not the host.",
    "warning CS": "Emitted by the C# compiler.",
    "[csharp_runtime] reloaded managed.dll":
        "Emitted by the .NET bridge, whose source is C# rather than Rust.",
    "'native::FrameCounter' -> migrating":
        "Composed at runtime from the component's type name and its verdict.",
    "'native::SpatialPosition' -> migrating":
        "Composed at runtime from the component's type name and its verdict.",
    "'native::LinearVelocity' -> migrating":
        "Composed at runtime from the component's type name and its verdict.",
    "'native::MarkerTrail' -> migrating":
        "Composed at runtime from the component's type name and its verdict.",
    "[csharp_runtime] reload failed:":
        "Printed by the managed loader (pill_csharp_runtime, C#), whose source "
        "is C# rather than Rust.",
    "[probe] schedule step=1 order=480":
        "Printed by the C# ScheduleProbes.cs fixture, which composes the step "
        "counter and the declared order into the line.",
    "[probe] schedule step=2 order=520":
        "Printed by the C# ScheduleProbes.cs fixture, which composes the step "
        "counter and the declared order into the line.",
    "[probe] schedule post_project frame=2 markers=1":
        "Printed by the C# ScheduleProbes fixture, which composes the frame "
        "counter and the marker count into the line.",
    "[probe] schedule step=2 order=999":
        "Printed by the C# ScheduleProbes.cs fixture after the suite edits the "
        "declared order, with the edited value composed in.",
    "are still loaded after":
        "Emitted by the .NET bridge (ProjectHost.cs), whose source is C# "
        "rather than Rust.",
    "awaiting the host's verdict":
        "Emitted by the .NET bridge (ProjectHost.cs), whose source is C# "
        "rather than Rust.",
    'kind="component" entries=1 touched=3':
        "Composed at runtime: the retirement line's structured fields carry "
        "`kind`, `entries` and `touched`, and their values are counts.",
    'kind="resource" entries=1 touched=1':
        "Composed at runtime: the retirement line's structured fields carry "
        "`kind`, `entries` and `touched`, and their values are counts.",
    "existing=1":
        "Composed at runtime: `existing` is a structured field on "
        "pill_spline's registration line, and its value is a count.",
    "[managed] sample extra=7.5":
        "Printed by C# that test_csharp_bridge.py injects into "
        "devops/tests/projects/managed during the run, to witness a field default.",
    "omo=(12,34) sum=47 a=1212 b=1234":
        "Printed by the C# BridgeProbe fixture test_csharp_bridge.py injects "
        "into devops/tests/projects/managed during the run: the values are the "
        "ones C# reads out of the module's column.",
    "omo=(12,34) sum=47 a=1212 b=1234 alpha=1":
        "Printed by the C# BridgeProbe fixture test_csharp_bridge.py injects "
        "into devops/tests/projects/managed during the run, with the extra "
        "field the wider probe line carries.",
    "cs spline bridge: sees 2 spline(s)":
        "Printed by the C# BridgeProbe fixture test_csharp_bridge.py injects "
        "into devops/tests/projects/managed during the run.",
    "probe retirement markers seeded":
        "Printed by C# that test_csharp_bridge.py injects into "
        "devops/tests/projects/managed during the run, to witness declaration "
        "retirement.",
    "SHARED RESOURCE PROBE":
        "Printed by Rust that test_shared_component_identity.py injects into "
        "the project during the run.",
    "SHARED RESOURCE WRITER MODULE":
        "Printed by Rust that test_shared_component_identity.py injects into "
        "pill_spline during the run.",
    "SHARED RESOURCE WRITER PROJECT":
        "Printed by Rust that test_shared_component_identity.py injects into "
        "the project during the run.",
    "TIME SURVIVAL PROBE":
        "Printed by Rust that test_shared_component_identity.py injects into "
        "the project during the run.",
    'mode="editing"':
        "Composed at runtime: the `dump play` line and the play-toggle line "
        "carry `mode` as a structured field, so the quotes come from the field "
        "formatter rather than from a literal.",
    'mode="playing"':
        "Composed at runtime: the `dump play` line and the play-toggle line "
        "carry `mode` as a structured field, so the quotes come from the field "
        "formatter rather than from a literal.",
    "restore_epoch=1":
        "Composed at runtime: `restore_epoch` is a structured field on the "
        "`dump play` and `play exited` lines, and its value is a count.",
}

# Tokens whose text another language prints and whose shape a Rust format string
# fits by coincidence. `{} reload {} {}` fits any `[subject] reload detail`
# token, so the managed loader's `[csharp_runtime] reload failed:` line reads as
# printable by `pill_development_host`'s analytics line although no Rust literal really
# prints it. An entry here is checked for still being declared and for really
# being such a coincidence; nothing can require its presence, because the fit it
# describes is indistinguishable from coverage.
SHAPE_ONLY_FITS: Dict[str, str] = {}

# Tokens whose whole point is to distinguish one reload subject from another.
# Collapsing either pair into a single string would leave the suites unable to
# tell a project reload from a module reload, so both must stay sourced.
SUBJECT_DISTINGUISHING_TOKENS: Tuple[Tuple[str, str], ...] = (
    ("hot reload complete", "extension hot reload complete"),
    ("no longer registered by the project", "no longer registered by this module"),
)

# =============================================================================
# Free Functions
# =============================================================================


def collect_declared_tokens() -> Dict[str, List[str]]:
    """
    Scans the devops Python suites for `*_TOKEN` literals.

    Returns a mapping from the literal string to the list of `file:CONSTANT`
    locations that declare it, so a failure can name every affected suite.
    """
    tokens: Dict[str, List[str]] = {}
    for directory in TOKEN_SOURCE_DIRECTORIES:
        for module_path in sorted((REPOSITORY_ROOT / directory).glob("*.py")):
            text = module_path.read_text(encoding="utf-8")
            for match in TOKEN_DEFINITION_RE.finditer(text):
                literal = match.group("literal")
                if not literal:
                    continue
                origin = f"{directory}/{module_path.name}:{match.group(1)}"
                tokens.setdefault(literal, []).append(origin)
            waited = waited_constant_names(module_path)
            for match in STRING_ASSIGNMENT_RE.finditer(text):
                if "TOKEN" in match.group(1) or match.group(1) not in waited:
                    continue
                literal = match.group("literal")
                if not literal:
                    continue
                origin = f"{directory}/{module_path.name}:{match.group(1)}"
                tokens.setdefault(literal, []).append(origin)
    return tokens


def waited_constant_names(module_path: Path) -> Set[str]:
    """
    The bare names passed to a `wait_for(...)` call in one module.

    A suite can wait on a host line through a constant whose name does not say
    TOKEN; the call site is the declaration of intent, so the name is collected
    whatever it is called. Tokenising keeps a `wait_for(` mentioned in prose
    from counting.
    """
    names: Set[str] = set()
    tokens = list(
        tokenize.generate_tokens(
            io.StringIO(module_path.read_text(encoding="utf-8")).readline
        )
    )
    for index, token in enumerate(tokens):
        if token.type != tokenize.NAME or token.string != "wait_for":
            continue
        if index and tokens[index - 1].string == "def":
            continue
        call = tokens[index + 1:]
        if not call or call[0].string != "(":
            continue
        depth = 0
        for following in call:
            if following.type == tokenize.OP:
                if following.string in "([{":
                    depth += 1
                elif following.string in ")]}":
                    depth -= 1
                    if depth == 0:
                        break
            elif depth == 1 and following.type == tokenize.NAME:
                names.add(following.string)
    return names


def iter_inline_wait_lines(module_path: Path) -> Iterator[int]:
    """
    Yields the line of every `wait_for(<inline literal>, ...)` call in a module.

    Tokenising rather than matching text keeps a `wait_for(` mentioned in a
    docstring, a comment or the `def wait_for(...)` signature itself from
    counting as a call site. Any argument counts, not only the first: the suites
    carry both helper shapes - `wait_for(token, ...)` and
    `wait_for(log_path, token, ...)` - and a literal in either hides its text
    from `collect_declared_tokens`.
    """
    tokens = list(
        tokenize.generate_tokens(
            io.StringIO(module_path.read_text(encoding="utf-8")).readline
        )
    )
    for index, token in enumerate(tokens):
        if token.type != tokenize.NAME or token.string != "wait_for":
            continue
        if index and tokens[index - 1].string == "def":
            continue
        call = tokens[index + 1:]
        if not call or call[0].string != "(":
            continue
        depth = 0
        for following in call:
            if depth == 1 and following.type in INLINE_LITERAL_TOKENS:
                yield token.start[0]
                break
            if following.type != tokenize.OP:
                continue
            if following.string in "([{":
                depth += 1
            elif following.string in ")]}":
                depth -= 1
                if depth == 0:
                    break


def read_rust_sources() -> str:
    """
    Concatenates every first-party Rust source into one haystack.

    The sources are `modules/` and the tracked test projects: a suite may
    wait on a line the test project prints, and the test project is code the host runs.
    Decoding errors are replaced rather than raised so that a stray non-UTF-8
    byte in an unrelated file cannot mask a genuine contract break.
    """
    return "\n".join(text for _, text in rust_source_files())


@lru_cache(maxsize=1)
def rust_source_files() -> Tuple[Tuple[Path, str], ...]:
    """
    Every first-party Rust source under `modules/` and the test projects.

    Only git-tracked files count: the host leaves hot-patch copies under
    `modules/pill_standalone_temp/` and generated `host_module_*` crates in the
    tree, and either would keep a renamed token green after the real source
    moved. The checks below scan the same tree several times, and reading it is
    the bulk of this suite's runtime.
    """
    completed = subprocess.run(
        ["git", "ls-files", "-z", "--", "modules", "devops/tests/projects"],
        cwd=str(REPOSITORY_ROOT),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "git ls-files failed; the log contract needs a git work tree:\n"
            + completed.stderr[-800:]
        )
    sources = []
    for relative in completed.stdout.split("\0"):
        if not relative or not relative.endswith(".rs"):
            continue
        source_path = REPOSITORY_ROOT / relative
        # A file deleted in the working tree is still tracked until the deletion
        # is committed, so `git ls-files` lists it; there is no source to scan.
        if not source_path.is_file():
            continue
        sources.append(
            (source_path, source_path.read_text(encoding="utf-8", errors="replace"))
        )
    return tuple(sources)


def iter_rust_string_literals(source: str) -> Iterator[Tuple[str, int]]:
    """
    Yields each `"..."` literal in one Rust source, with the line it opens on.

    Comments, character literals and lifetimes are skipped, so a quoted word in
    a comment cannot pass for a log line; a raw string (`r#"..."#`) is read to
    its own closing delimiter; and a backslash-newline continuation is joined
    the way rustc joins it, so a wrapped format string stays one literal.
    """
    index = 0
    line = 1
    length = len(source)
    while index < length:
        if source.startswith("//", index):
            newline = source.find("\n", index)
            index = length if newline < 0 else newline
        elif source.startswith("/*", index):
            closing = source.find("*/", index + 2)
            end = length if closing < 0 else closing + 2
            line += source.count("\n", index, end)
            index = end
        elif source[index] == "\n":
            line += 1
            index += 1
        elif source[index] == "'":
            index = _skip_character_literal(source, index)
        elif source[index] == '"':
            opening = index
            body, index = _read_string_body(source, index + 1)
            yield body, line
            line += source.count("\n", opening, index)
        else:
            hashes = _raw_string_hashes(source, index)
            if hashes is None:
                index += 1
                continue
            start = index
            opening = index + hashes + 2
            delimiter = '"' + "#" * hashes
            closing = source.find(delimiter, opening)
            if closing < 0:
                yield source[opening:], line
                return
            yield source[opening:closing], line
            index = closing + len(delimiter)
            line += source.count("\n", start, index)


def _skip_character_literal(source: str, index: int) -> int:
    """Steps over a character literal, or over the quote of a lifetime."""
    cursor = index + 1
    if source[cursor:cursor + 1] == "\\":
        cursor += 2
        while cursor < len(source) and source[cursor] != "'":
            cursor += 1
        return min(cursor + 1, len(source))
    if source[cursor + 1:cursor + 2] == "'":
        return cursor + 2
    return cursor


def _read_string_body(source: str, cursor: int) -> Tuple[str, int]:
    """
    Reads a normal string literal body from just past its opening quote.

    Returns the body, with escapes resolved and line continuations joined, and
    the index just past the closing quote.
    """
    pieces: List[str] = []
    while cursor < len(source):
        character = source[cursor]
        if character == "\\":
            following = source[cursor + 1:cursor + 2]
            if following in ("\n", "\r"):
                # A line continuation: rustc drops the backslash, the newline
                # and the indentation of the next line.
                cursor += 2
                if source[cursor - 1] == "\r" and source[cursor:cursor + 1] == "\n":
                    cursor += 1
                while cursor < len(source) and source[cursor] in " \t":
                    cursor += 1
                continue
            pieces.append(STRING_ESCAPES.get(following, following))
            cursor += 2
            continue
        if character == '"':
            break
        pieces.append(character)
        cursor += 1
    return "".join(pieces), cursor + 1


def _raw_string_hashes(source: str, index: int) -> Optional[int]:
    """
    Returns the `#` count when a raw string opens at `index`, else `None`.

    `index` holds the `r`; the `b` or `c` of a `br#"..."#` byte or C string is
    allowed in front, and an identifier that merely ends in `r` is not a raw
    string opener. A plain `"` is not one either - the caller reaches it as an
    ordinary string literal, with its escapes resolved.
    """
    if source[index:index + 1] != "r":
        return None
    preceding = source[index - 1:index]
    if preceding in ("b", "c"):
        preceding = source[index - 2:index - 1]
    if preceding.isalnum() or preceding == "_":
        return None
    cursor = index + 1
    hashes = 0
    while source[cursor:cursor + 1] == "#":
        hashes += 1
        cursor += 1
    return hashes if source[cursor:cursor + 1] == '"' else None


class RustLiteral(NamedTuple):
    """One Rust string literal, indexed for matching a suite token against it."""

    literal: str
    parts: Tuple[str, ...]
    anchor: str
    location: str


@lru_cache(maxsize=1)
def rust_literals() -> Tuple[RustLiteral, ...]:
    """
    Every Rust string literal that can print something other than itself.

    Literals without a `{...}` placeholder are skipped: a suite token that
    appears in one verbatim is already a substring of the haystack. So are
    literals with no static segment of `MINIMUM_ANCHOR_CHARACTERS`, because no
    cut of them can anchor a match.
    """
    entries: List[RustLiteral] = []
    for source_path, text in rust_source_files():
        relative = source_path.relative_to(REPOSITORY_ROOT).as_posix()
        for literal, line in iter_rust_string_literals(text):
            parts = tuple(PLACEHOLDER_RE.split(literal))
            if len(parts) < 2:
                continue
            anchor = next(
                (
                    part
                    for part in parts
                    if anchor_length(part) >= MINIMUM_ANCHOR_CHARACTERS
                ),
                None,
            )
            if anchor is None:
                continue
            entries.append(RustLiteral(literal, parts, anchor, f"{relative}:{line}"))
    return tuple(entries)


def anchor_length(text: str) -> int:
    """Counts the non-whitespace characters that make a segment a real anchor."""
    return sum(1 for character in text if not character.isspace())


def token_is_covered(token: str) -> bool:
    """
    True when some Rust string literal can print `token`.

    Verbatim first, which is how almost every token is found, then through the
    `{...}` placeholders of a format string.
    """
    if token in read_rust_sources():
        return True
    for entry in rust_literals():
        if entry.anchor in token and literal_prints(entry.parts, token):
            return True
    # A token that starts inside the line, such as one field among several:
    # every place a static segment holds the token's opening characters is a
    # possible start, and the rest of the literal must still print the token.
    # The search for those characters keeps the scan cheap.
    head = token[:MINIMUM_ANCHOR_CHARACTERS]
    for entry in rust_literals():
        for index, segment in enumerate(entry.parts):
            start = segment.find(head)
            while start >= 0:
                suffix = (segment[start:],) + entry.parts[index + 1:]
                if literal_prints(suffix, token):
                    return True
                start = segment.find(head, start + 1)
    return False


def literal_prints(parts: Tuple[str, ...], token: str) -> bool:
    """
    True when the format string `parts` can print `token`.

    `parts` holds the static text between the `{...}` placeholders, so the
    placeholders are the gaps between consecutive entries (and one before the
    first and after the last only if the literal opens or closes with one).
    A placeholder stands for one or more non-whitespace characters, and the
    token may stop anywhere - including inside a static segment, because the
    host goes on printing fields the suite does not wait for.
    """
    # (segment, characters of the token consumed, anchored yet) has one answer,
    # and a placeholder may consume any run length, so memoise to keep a token
    # full of placeholders from branching exponentially.
    answers: Dict[Tuple[int, int, bool], bool] = {}

    def walk(index: int, consumed: int, anchored: bool) -> bool:
        segment = parts[index]
        available = token[consumed:consumed + len(segment)]
        if not segment.startswith(available):
            return False
        if len(available) < len(segment):
            # The token stops inside this segment: still a prefix of the line,
            # anchored only if it carries enough of the segment.
            return anchored or anchor_length(available) >= MINIMUM_ANCHOR_CHARACTERS
        consumed += len(segment)
        anchored = anchored or anchor_length(segment) >= MINIMUM_ANCHOR_CHARACTERS
        if consumed == len(token):
            return anchored
        if index + 1 >= len(parts):
            return False
        key = (index, consumed, anchored)
        if key in answers:
            return answers[key]
        result = False
        for end in range(consumed + 1, len(token) + 1):
            if token[end - 1].isspace():
                break
            if walk(index + 1, end, anchored):
                result = True
                break
        answers[key] = result
        return result

    return walk(0, 0, False)


def is_exempt(token: str) -> bool:
    """True when the token is documented as not coming from a Rust literal."""
    return token in EXTERNALLY_PRODUCED or token in SHAPE_ONLY_FITS


def check_token_source_directories_exist() -> None:
    """
    Fails when a directory the contract scans is not in the tree.

    A missing directory contributes no tokens, so without this check a moved
    tree would silently narrow the contract instead of failing it.
    """
    missing = [
        directory
        for directory in TOKEN_SOURCE_DIRECTORIES
        if not (REPOSITORY_ROOT / directory).is_dir()
    ]
    assert not missing, "no such token directory: " + ", ".join(missing)


def check_every_declared_token_exists_in_rust() -> None:
    """Fails if a suite matches on a string the host no longer prints."""
    missing = []
    for literal, origins in sorted(collect_declared_tokens().items()):
        if is_exempt(literal) or token_is_covered(literal):
            continue
        missing.append(f"{literal!r} required by {', '.join(origins)}")
    assert not missing, (
        "no Rust string literal can print these tokens, verbatim or through "
        "the `{...}` placeholders a format string carries, so the suites that "
        "match on them can only fail at runtime:\n    " + "\n    ".join(missing)
    )


def check_exemptions_are_still_needed() -> None:
    """
    Fails if an entry in EXTERNALLY_PRODUCED is stale.

    An exemption whose token a Rust literal can print is a hole: it would let a
    real regression through. An exemption no suite declares any more is dead
    weight that makes the list harder to trust. Only the second test applies to
    SHAPE_ONLY_FITS: its tokens are printed by another language, so a Rust
    literal that happens to fit their shape proves nothing.
    """
    declared = collect_declared_tokens()
    stale = [
        f"{literal!r} ({reason})"
        for literal, reason in sorted(EXTERNALLY_PRODUCED.items())
        if token_is_covered(literal)
    ]
    assert not stale, (
        "these exemptions are no longer needed - a Rust literal can print the "
        "token now, so remove the entry:\n    " + "\n    ".join(stale)
    )
    unused = [
        repr(literal)
        for literal in sorted((*EXTERNALLY_PRODUCED, *SHAPE_ONLY_FITS))
        if literal not in declared
    ]
    assert not unused, (
        "these exemptions name tokens no suite declares any more:\n    "
        + "\n    ".join(unused)
    )


def check_shape_only_fits_are_real_coincidences() -> None:
    """
    Fails if a SHAPE_ONLY_FITS entry is not a shape coincidence.

    The two tables are told apart by that property: an EXTERNALLY_PRODUCED token
    no literal can print, a SHAPE_ONLY_FITS token one can only by fitting an
    unrelated line. An entry in the wrong table would state the wrong reason.
    """
    misplaced = [
        f"{literal!r} ({reason})"
        for literal, reason in sorted(SHAPE_ONLY_FITS.items())
        if not token_is_covered(literal)
    ]
    assert not misplaced, (
        "these tokens are listed as a Rust literal fitting their shape by "
        "coincidence, but no Rust format string fits them at all; they belong "
        "in EXTERNALLY_PRODUCED:\n    " + "\n    ".join(misplaced)
    )


def check_wait_for_literals_are_named_constants() -> None:
    """
    Fails if a host suite waits on a log line written inline.

    A literal written into a `wait_for(...)` argument hides that text from
    `collect_declared_tokens`, so the line it pins can be reworded with nothing
    here to catch it. Naming it as a `*_TOKEN` constant puts it under the
    contract; a token Rust does not print belongs in EXTERNALLY_PRODUCED.
    """
    offenders = [
        f"{directory}/{module_path.name}:{line}"
        for directory in TOKEN_SOURCE_DIRECTORIES
        for module_path in sorted((REPOSITORY_ROOT / directory).glob("*.py"))
        for line in iter_inline_wait_lines(module_path)
    ]
    assert not offenders, (
        "these suites wait on an inline log line, which no constant and no "
        "check here can see; name it as a `*_TOKEN` constant instead:\n    "
        + "\n    ".join(offenders)
    )


def check_subject_tokens_stay_distinguishable() -> None:
    """
    Fails if a project token and its module counterpart cannot be told apart.

    Both pairs differ only by wording, and one is a substring of the other
    ("hot reload complete" inside "extension hot reload complete"). The
    requirement is that each still has its own distinct Rust literal.
    """
    rust_sources = read_rust_sources()
    for project_token, module_token in SUBJECT_DISTINGUISHING_TOKENS:
        for token in (project_token, module_token):
            assert token in rust_sources, (
                f"{token!r} is not printed by any Rust source; the suites can "
                "no longer tell a project reload from a module reload"
            )
        assert project_token != module_token, (
            f"{project_token!r} collapsed onto its counterpart"
        )


def check_migration_banners_are_printed() -> None:
    """
    Fails if a `[persistence]` migration banner a suite matches on is gone.

    `test_hot_reload_migration.py` and `suite_common.py` both key on these
    banners to decide whether a selective migration ran, so losing one turns a
    migration assertion into a silent no-op.
    """
    rust_sources = read_rust_sources()
    banners = [
        literal
        for literal in collect_declared_tokens()
        if literal.startswith("[persistence]") and "migration" in literal.lower()
    ]
    assert banners, "no [persistence] migration banners are declared any more"
    for banner in sorted(banners):
        assert banner in rust_sources, (
            f"{banner!r} is declared by a suite but printed by nothing"
        )


# =============================================================================
# Test Registration
# =============================================================================

CHECKS: List[Tuple[str, Callable[[], None]]] = [
    ("token source directories exist", check_token_source_directories_exist),
    ("every declared token exists in Rust", check_every_declared_token_exists_in_rust),
    ("exemptions are still needed", check_exemptions_are_still_needed),
    (
        "shape-only exemptions are coincidences",
        check_shape_only_fits_are_real_coincidences,
    ),
    (
        "waits use named tokens",
        check_wait_for_literals_are_named_constants,
    ),
    ("project and module tokens stay distinct", check_subject_tokens_stay_distinguishable),
    ("migration banners are printed", check_migration_banners_are_printed),
]


def main() -> None:
    """Run every check and report a tally."""
    print("=" * 70)
    print("  Host Log Contract Tests")
    print("=" * 70)
    # Report what the scan sees before checking it: the count must include the
    # native-editor suites' patterns, which the editor-session driver collects
    # with everything else.
    declared = collect_declared_tokens()
    driver_tokens = sum(
        1
        for origins in declared.values()
        if any(origin.startswith("devops/core/editor_session.py") for origin in origins)
    )
    print(
        f"  tokens: {len(declared)} declared across "
        f"{', '.join(TOKEN_SOURCE_DIRECTORIES)} ({driver_tokens} from the "
        "native-editor driver)"
    )
    failed = 0
    for name, check in CHECKS:
        try:
            check()
            print(f"  [OK]   {name}")
        except AssertionError as error:
            failed += 1
            print(f"  [FAIL] {name}")
            print(f"         {error}")
        except Exception as error:  # noqa: BLE001 - report, do not mask
            failed += 1
            print(f"  [FAIL] {name} raised {type(error).__name__}: {error}")
    print("-" * 70)
    print(f"  {len(CHECKS) - failed} passed, {failed} failed")
    print("=" * 70)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    run_suite_with_timing(main)
