#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+ (standard library only). `git` on PATH lists the
#               tracked sources; without it the scan walks the directory tree.

# DESCRIPTION: Ranks the Rust sources by comment debt, so a comment cleanup can
#   start where it matters instead of alphabetically. For every scanned file it
#   reports:
#
#     1. Size: lines, comment lines, doc lines and the longest doc block.
#     2. Process references in comments - work-item ids, stage numbers, plan
#        decisions and site paths. These describe the work that produced the
#        code, not the code, so they are the debt a cleanup removes first.
#     3. History wording (the HISTORY_REFERENCE_PATTERNS list), which
#        describes an older version of the file rather than the current one.
#     4. TODO markers, and doc blocks longer than their budget (item docs and
#        module headers have separate budgets).
#     5. Public surface: how many `pub` items the file exposes, because a
#        comment on a public item is read far more often than one in a private
#        helper.
#
#   The score is deliberately transparent and printed in pieces:
#
#     debt     = 3 * process refs + 2 * history refs + 2 * TODO markers
#                + 1 * excess doc lines
#     density  = debt per 100 comment lines
#     priority = debt * (1 + min(public items, 50) / 10)
#
#   Priority answers "where is the most reader-facing debt"; density answers
#   "where is the debt most concentrated". `--sort` chooses either. The weights
#   are a triage aid, not a verdict: a long `# Safety` contract is not debt,
#   which is why long-doc excess is capped per block and weighted lowest, and
#   why safety comments are reported separately as value to protect.
#
#   The scan is line-oriented: it counts full-line comments (`//`, `///`,
#   `//!`), not trailing comments. A `//` at the start of a line inside a
#   multi-line string literal would be miscounted as a comment; Rust sources
#   rarely carry one. Path claims are checked only with `--check-paths`, and
#   only against files present in the working tree, because a comment may
#   legitimately name a runtime file or a cooked build artifact. A token needs
#   a directory segment to count: a bare filename is often a module shorthand
#   or a file the build generates.
#
# USAGE: python devops/tools/comment_audit.py [options]
#          --root PATH        directory to scan, absolute or relative to the
#                             repository root (default: modules)
#          --top N            rows per table (default: 25)
#          --sort KEY         priority | debt | density | surface | path
#                             (default: priority)
#          --min-debt N       hide files whose debt is below N (default: 0)
#          --json PATH        write the full per-file record as JSON
#          --markdown PATH    write a report with the method and the tables
#          --check-paths      also list source paths named by comments that do
#                             not exist on disk (advisory)
#          --self-test        check the analyzer against embedded samples
#
# EXAMPLE USAGE:
#   python devops/tools/comment_audit.py
#   python devops/tools/comment_audit.py --root modules/editor/pill_editor_core
#   python devops/tools/comment_audit.py --sort density --top 40
#   python devops/tools/comment_audit.py --json local/temp/comment-audit.json
#   python devops/tools/comment_audit.py --markdown local/temp/comment-audit.md
#   python devops/tools/comment_audit.py --check-paths --top 60

# --- SCRIPT ---

# Standard library
import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Sequence, Tuple

# A doc block may run this many lines before its excess starts counting. Item
# docs are held to the rules' 2-4 line shape; module headers carry a summary,
# responsibilities and sometimes design notes, so they get a larger budget.
ITEM_DOC_BLOCK_BUDGET = 12
MODULE_DOC_BLOCK_BUDGET = 24
# Most excess lines one block can contribute, so one huge header cannot swamp
# the ranking on its own.
DOC_BLOCK_EXCESS_CAP = 40
# Filters for the density table, so a tiny file with one comment does not win
# on a technicality.
DENSITY_TABLE_MINIMUM_DEBT = 5
DENSITY_TABLE_LIMIT = 15
# How many missing path claims to print before stopping.
MISSING_PATH_REPORT_LIMIT = 25

# Weights used by the debt formula. Process references cost the most because
# they are always removable; history wording needs a rewrite; a TODO marker
# may be honest; excess doc lines are only a hint.
PROCESS_REFERENCE_WEIGHT = 3
HISTORY_REFERENCE_WEIGHT = 2
TODO_MARKER_WEIGHT = 2
LONG_DOC_LINE_WEIGHT = 1

# Comment text that describes the work that produced the code.
PROCESS_REFERENCE_PATTERNS = (
    ("work item", re.compile(r"\bW-\d{4}\b")),
    ("stage", re.compile(r"\bstage \d+\b", re.IGNORECASE)),
    ("plan decision", re.compile(r"\bdecision [A-Z]\d+\b")),
    ("site path", re.compile(r"\.construction_site\b|\.agents/")),
    ("audit finding", re.compile(r"\bSA-\d{4}\b")),
    ("audit opportunity", re.compile(r"\baudit opportunity \d+\b", re.IGNORECASE)),
    ("plan section", re.compile(r"\b[a-z][a-z0-9_-]* plan, section [A-Z]\b", re.IGNORECASE)),
)

# Comment text that describes an older version of the code. "used to" alone is
# excluded because "used to compute" states a current purpose; only the
# history form ("used to" followed by "be") is counted.
HISTORY_REFERENCE_PATTERNS = (
    ("previously", re.compile(r"\bpreviously\b", re.IGNORECASE)),
    ("used to be", re.compile(r"\bused to be\b", re.IGNORECASE)),
    ("formerly", re.compile(r"\bformerly\b", re.IGNORECASE)),
    ("renamed from", re.compile(r"\brenamed from\b", re.IGNORECASE)),
    ("was renamed", re.compile(r"\bwas renamed\b", re.IGNORECASE)),
    ("now that", re.compile(r"\bnow that\b", re.IGNORECASE)),
    ("at the time of writing", re.compile(r"\bat the time of writing\b", re.IGNORECASE)),
    ("as of", re.compile(r"\bas of\b", re.IGNORECASE)),
)

# Words that often signal history but also have legitimate current meanings
# ("retired mirror-method tables", "the legacy path"), so they are reported
# but never scored.
ADVISORY_HISTORY_PATTERNS = (
    ("retired", re.compile(r"\bretired\b", re.IGNORECASE)),
    ("no longer", re.compile(r"\bno longer\b", re.IGNORECASE)),
    ("legacy", re.compile(r"\blegacy\b", re.IGNORECASE)),
)

TODO_MARKER_PATTERN = re.compile(r"\b(?:TODO|FIXME|HACK|XXX)\b")
SAFETY_COMMENT_PATTERN = re.compile(r"^\s*//\s*SAFETY:")

# A `pub` item without a visibility restriction: the crate's actual API.
PUBLIC_ITEM_PATTERN = re.compile(
    r"^\s*pub\s+(?:async\s+)?(?:unsafe\s+)?(?:extern\s+\"[^\"]*\"\s+)?"
    r"(?:fn|struct|enum|trait|const|static|type|mod|union)\b"
)

# Source paths a comment may name. Only code-ish extensions are checked, so a
# runtime file name (a layout `.json`, an editor `.theme`) is never counted.
PATH_CLAIM_PATTERN = re.compile(
    r"[A-Za-z0-9_][A-Za-z0-9_./-]*\.(?:rs|toml|wgsl|hlsl|cs|csproj)\b"
)

# Directories that are build output, generated, or scratch. Mirrors the
# exclusion set of `devops/tests/test_coding_standards.py`; the tracked-file
# listing usually makes this redundant, but the tree-walk fallback needs it.
EXCLUDED_DIRECTORY_NAMES = {
    "target",
    "node_modules",
    ".git",
    "pill_standalone_temp",
    "build",
    "generated",
    "projects_generated",
}
# The repository-root `local/` is gitignored scratch. Matched at the top level
# only: `examples/` holds the game projects and `devops/tests/projects/` the
# suites' test projects; both are scanned like any other
# source tree.
ROOT_ONLY_EXCLUDED_DIRECTORY_NAMES = {"local"}
# The one tracked crate under a `build/` directory that is real source.
BUILD_SOURCE_CRATES = ("pill_shipping_bundle",)
# Directories the path-claim check never walks: build output and scratch. The
# root `local/` is pruned separately, by depth.
PATH_CLAIM_SKIP_DIRECTORIES = {".git", "target", "node_modules", "pill_standalone_temp"}


@dataclass
class FileAudit:
    """The comment measurements of one Rust source file."""

    path: str
    lines: int
    comment_lines: int
    doc_lines: int
    public_items: int
    safety_comments: int
    doc_blocks: int
    longest_doc_block: int
    item_doc_excess: int
    module_doc_excess: int
    todo_count: int
    is_test_file: bool
    process_counts: Dict[str, int] = field(default_factory=dict)
    history_counts: Dict[str, int] = field(default_factory=dict)
    advisory_history_counts: Dict[str, int] = field(default_factory=dict)
    missing_path_claims: List[str] = field(default_factory=list)

    def process_total(self) -> int:
        """Total process references in this file's comments."""
        return sum(self.process_counts.values())

    def history_total(self) -> int:
        """Total scored history references in this file's comments."""
        return sum(self.history_counts.values())

    def advisory_history_total(self) -> int:
        """Total advisory history words, which do not affect the score."""
        return sum(self.advisory_history_counts.values())

    def long_doc_excess(self) -> int:
        """Total excess doc lines, from item docs and module headers."""
        return self.item_doc_excess + self.module_doc_excess

    def debt(self) -> int:
        """Weighted count of the comment smells in this file."""
        return (
            PROCESS_REFERENCE_WEIGHT * self.process_total()
            + HISTORY_REFERENCE_WEIGHT * self.history_total()
            + TODO_MARKER_WEIGHT * self.todo_count
            + LONG_DOC_LINE_WEIGHT * self.long_doc_excess()
        )

    def density(self) -> float:
        """Debt per 100 comment lines, so file size does not decide alone."""
        if self.comment_lines == 0:
            return 0.0
        return 100.0 * self.debt() / self.comment_lines

    def surface_factor(self) -> float:
        """How much the public API multiplies the debt, from 1 to 6."""
        return 1.0 + min(self.public_items, 50) / 10.0

    def priority(self) -> float:
        """Debt weighted by the size of the file's public surface."""
        return self.debt() * self.surface_factor()


def repository_root() -> Path:
    """The repository root, located from this script."""
    return Path(__file__).resolve().parent.parent.parent


REPOSITORY_ROOT = repository_root()


def resolve_scan_root(value: str) -> Path:
    """Resolves `--root`, preferring the repository root over the caller's cwd."""
    candidate = Path(value)
    if candidate.is_absolute():
        return candidate.resolve()
    rooted = REPOSITORY_ROOT / candidate
    if rooted.exists():
        return rooted.resolve()
    return candidate.resolve()


def display_path(path: Path) -> str:
    """A repository-relative path with forward slashes, for reports."""
    try:
        relative = path.resolve().relative_to(REPOSITORY_ROOT)
    except ValueError:
        relative = path.resolve()
    return relative.as_posix()


def repository_relative_parts(path: Path) -> Tuple[str, ...]:
    """The path's parts below the repository root, or all parts if outside."""
    try:
        return path.resolve().relative_to(REPOSITORY_ROOT).parts
    except ValueError:
        return path.resolve().parts


def should_scan(path: Path) -> bool:
    """Whether the tree-walk fallback treats a path as engine source."""
    parts = repository_relative_parts(path)
    for index, part in enumerate(parts):
        if part in ROOT_ONLY_EXCLUDED_DIRECTORY_NAMES and index == 0:
            return False
        if part not in EXCLUDED_DIRECTORY_NAMES:
            continue
        if part == "build" and index + 1 < len(parts) and parts[index + 1] in BUILD_SOURCE_CRATES:
            continue
        return False
    return True


def is_under_root(path: Path, root: Path) -> bool:
    """Whether `path` lies at or below `root`."""
    resolved = path.resolve()
    return resolved == root or root in resolved.parents


def tracked_rust_files(root: Path) -> List[Path]:
    """The tracked Rust files under `root`, or a tree walk when git is absent."""
    try:
        completed = subprocess.run(
            ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "*.rs"],
            cwd=str(REPOSITORY_ROOT),
            capture_output=True,
            check=False,
        )
    except OSError:
        completed = None
    if completed is not None and completed.returncode == 0:
        paths = []
        entries = completed.stdout.decode("utf-8", "replace").split("\0")
        for entry in entries:
            if not entry:
                continue
            candidate = (REPOSITORY_ROOT / entry).resolve()
            if candidate.is_file() and is_under_root(candidate, root):
                paths.append(candidate)
        return sorted(paths)
    return sorted(
        path.resolve() for path in root.rglob("*.rs") if should_scan(path)
    )


def load_existing_relative_paths() -> List[str]:
    """Every file present in the working tree, for the path-claim check.

    The working tree, not git's index: a cooked shader or another generated
    artifact is a real file a correct comment may name, and flagging it as
    missing would only teach a reader to distrust the report.
    """
    paths: List[str] = []
    for directory, directory_names, file_names in os.walk(str(REPOSITORY_ROOT)):
        at_repository_root = Path(directory) == REPOSITORY_ROOT
        directory_names[:] = [
            name
            for name in directory_names
            if name not in PATH_CLAIM_SKIP_DIRECTORIES
            and not (at_repository_root and name in ROOT_ONLY_EXCLUDED_DIRECTORY_NAMES)
        ]
        for file_name in file_names:
            paths.append(display_path(Path(directory) / file_name))
    return paths


def is_existing_path(token: str, existing_paths: Sequence[str]) -> bool:
    """Whether a path a comment names exists anywhere in the working tree."""
    if token in existing_paths:
        return True
    suffix = "/" + token
    return any(path.endswith(suffix) for path in existing_paths)


def comment_lines_with_numbers(source_lines: Sequence[str]) -> List[Tuple[int, str]]:
    """The line number and stripped text of every full-line comment."""
    comments: List[Tuple[int, str]] = []
    for number, line in enumerate(source_lines, start=1):
        stripped = line.strip()
        if stripped.startswith("//"):
            comments.append((number, stripped))
    return comments


def doc_block_measurements(source_lines: Sequence[str]) -> Tuple[int, int, int, int, int]:
    """Returns (doc lines, doc blocks, longest block, item excess, module excess).

    A doc block is a contiguous run of `///` or `//!` lines. Only the amount a
    block exceeds its budget counts, capped per block, so a legitimate long
    contract costs a little while an essay costs a bounded amount.
    """
    doc_lines = 0
    doc_blocks = 0
    longest = 0
    item_excess = 0
    module_excess = 0
    current = 0
    current_is_module = False
    for line in source_lines:
        stripped = line.strip()
        if stripped.startswith("///") or stripped.startswith("//!"):
            if current == 0:
                current_is_module = stripped.startswith("//!")
            doc_lines += 1
            current += 1
            continue
        if current > 0:
            doc_blocks += 1
            longest = max(longest, current)
            budget = MODULE_DOC_BLOCK_BUDGET if current_is_module else ITEM_DOC_BLOCK_BUDGET
            excess = min(max(0, current - budget), DOC_BLOCK_EXCESS_CAP)
            if current_is_module:
                module_excess += excess
            else:
                item_excess += excess
            current = 0
    if current > 0:
        doc_blocks += 1
        longest = max(longest, current)
        budget = MODULE_DOC_BLOCK_BUDGET if current_is_module else ITEM_DOC_BLOCK_BUDGET
        excess = min(max(0, current - budget), DOC_BLOCK_EXCESS_CAP)
        if current_is_module:
            module_excess += excess
        else:
            item_excess += excess
    return doc_lines, doc_blocks, longest, item_excess, module_excess


def count_patterns(
    patterns: Sequence[Tuple[str, "re.Pattern"]], texts: Sequence[str]
) -> Dict[str, int]:
    """Counts every pattern's occurrences across the comment texts."""
    counts: Dict[str, int] = {}
    for name, pattern in patterns:
        counts[name] = sum(len(pattern.findall(text)) for text in texts)
    return counts


def find_missing_path_claims(
    comments: Sequence[Tuple[int, str]], existing_paths: Optional[Sequence[str]]
) -> List[str]:
    """Paths named by comments that do not exist in the working tree.

    Findings carry the comment's line number so a person can judge them at
    once; a comment may legitimately name a file in an external project.
    """
    if existing_paths is None:
        return []
    missing: List[str] = []
    seen = set()
    for number, comment in comments:
        for token in PATH_CLAIM_PATTERN.findall(comment):
            normalized = token.replace("\\", "/").lstrip("./")
            if "/" not in normalized:
                continue
            if normalized in seen:
                continue
            seen.add(normalized)
            if not is_existing_path(normalized, existing_paths):
                missing.append(f"line {number}: {token}")
    return missing


def analyze_source(
    relative_path: str,
    text: str,
    existing_paths: Optional[Sequence[str]] = None,
) -> FileAudit:
    """Measures one Rust source's comments; the unit the whole report uses."""
    source_lines = text.splitlines()
    comments = comment_lines_with_numbers(source_lines)
    comment_texts = [comment for _number, comment in comments]
    (
        doc_lines,
        doc_blocks,
        longest_doc_block,
        item_doc_excess,
        module_doc_excess,
    ) = doc_block_measurements(source_lines)
    public_items = sum(1 for line in source_lines if PUBLIC_ITEM_PATTERN.match(line))
    safety_comments = sum(1 for line in source_lines if SAFETY_COMMENT_PATTERN.search(line))
    path = Path(relative_path)
    is_test_file = (
        path.name == "tests.rs"
        or path.name.endswith("_tests.rs")
        or "tests" in path.parts[:-1]
    )
    return FileAudit(
        path=relative_path,
        lines=len(source_lines),
        comment_lines=len(comments),
        doc_lines=doc_lines,
        public_items=public_items,
        safety_comments=safety_comments,
        doc_blocks=doc_blocks,
        longest_doc_block=longest_doc_block,
        item_doc_excess=item_doc_excess,
        module_doc_excess=module_doc_excess,
        todo_count=sum(
            len(TODO_MARKER_PATTERN.findall(comment)) for comment in comment_texts
        ),
        is_test_file=is_test_file,
        process_counts=count_patterns(PROCESS_REFERENCE_PATTERNS, comment_texts),
        history_counts=count_patterns(HISTORY_REFERENCE_PATTERNS, comment_texts),
        advisory_history_counts=count_patterns(ADVISORY_HISTORY_PATTERNS, comment_texts),
        missing_path_claims=find_missing_path_claims(comments, existing_paths),
    )


def order_audits(audits: Sequence[FileAudit], sort_key: str) -> List[FileAudit]:
    """Sorts audits by the requested key, with the path as the tiebreaker."""
    keys = {
        "priority": lambda audit: (-audit.priority(), audit.path),
        "debt": lambda audit: (-audit.debt(), audit.path),
        "density": lambda audit: (-audit.density(), audit.path),
        "surface": lambda audit: (-audit.public_items, audit.path),
        "path": lambda audit: (audit.path,),
    }
    return sorted(audits, key=keys[sort_key])


def format_table(audits: Sequence[FileAudit], limit: int) -> List[str]:
    """The aligned table rows for the top of a ranking."""
    header = (
        f"{'rank':>4} {'priority':>8} {'debt':>5} {'density':>7} {'pub':>4}"
        f" {'proc':>5} {'hist':>5} {'todo':>4} {'longdoc':>7} {'kind':>4}  file"
    )
    rows = [header, "-" * len(header)]
    for rank, audit in enumerate(audits[:limit], start=1):
        kind = "test" if audit.is_test_file else "src"
        rows.append(
            f"{rank:>4} {audit.priority():>8.1f} {audit.debt():>5} {audit.density():>7.1f}"
            f" {audit.public_items:>4} {audit.process_total():>5} {audit.history_total():>5}"
            f" {audit.todo_count:>4} {audit.long_doc_excess():>7} {kind:>4}  {audit.path}"
        )
    return rows


def summarize(audits: Sequence[FileAudit]) -> Dict[str, int]:
    """Aggregate counts for the report header."""
    process_totals: Dict[str, int] = {}
    for name, _pattern in PROCESS_REFERENCE_PATTERNS:
        process_totals[name] = sum(audit.process_counts.get(name, 0) for audit in audits)
    advisory_totals: Dict[str, int] = {}
    for name, _pattern in ADVISORY_HISTORY_PATTERNS:
        advisory_totals[name] = sum(
            audit.advisory_history_counts.get(name, 0) for audit in audits
        )
    return {
        "files": len(audits),
        "lines": sum(audit.lines for audit in audits),
        "comment_lines": sum(audit.comment_lines for audit in audits),
        "doc_lines": sum(audit.doc_lines for audit in audits),
        "public_items": sum(audit.public_items for audit in audits),
        "safety_comments": sum(audit.safety_comments for audit in audits),
        "process_references": sum(audit.process_total() for audit in audits),
        "history_references": sum(audit.history_total() for audit in audits),
        "todo_markers": sum(audit.todo_count for audit in audits),
        "long_doc_excess": sum(audit.long_doc_excess() for audit in audits),
        "item_doc_excess": sum(audit.item_doc_excess for audit in audits),
        "module_doc_excess": sum(audit.module_doc_excess for audit in audits),
        "files_with_long_doc": sum(1 for audit in audits if audit.long_doc_excess() > 0),
        "files_with_debt": sum(1 for audit in audits if audit.debt() > 0),
        "advisory_history_total": sum(audit.advisory_history_total() for audit in audits),
        "process_work_item": process_totals["work item"],
        "process_stage": process_totals["stage"],
        "process_plan_decision": process_totals["plan decision"],
        "process_site_path": process_totals["site path"],
        "advisory_retired": advisory_totals["retired"],
        "advisory_no_longer": advisory_totals["no longer"],
        "advisory_legacy": advisory_totals["legacy"],
    }


def print_summary(root: Path, audits: Sequence[FileAudit], totals: Dict[str, int]) -> None:
    """Prints the aggregate counts and the scan's scope."""
    print(f"root: {display_path(root)}")
    print(
        f"scanned {totals['files']} Rust files, {totals['lines']} lines,"
        f" {totals['comment_lines']} comment lines"
        f" ({totals['doc_lines']} doc, {totals['comment_lines'] - totals['doc_lines']} plain)"
    )
    print(
        f"process references: {totals['process_references']}"
        f" (work item {totals['process_work_item']}, stage {totals['process_stage']},"
        f" plan decision {totals['process_plan_decision']}, site path {totals['process_site_path']})"
    )
    print(f"history references: {totals['history_references']}")
    print(
        f"advisory history wording: {totals['advisory_history_total']}"
        f" (retired {totals['advisory_retired']}, no longer {totals['advisory_no_longer']},"
        f" legacy {totals['advisory_legacy']})"
    )
    print(f"todo markers: {totals['todo_markers']}")
    print(
        f"long-doc excess: {totals['long_doc_excess']} lines"
        f" (item docs {totals['item_doc_excess']}, module headers {totals['module_doc_excess']});"
        f" {totals['files_with_long_doc']} files hold a block over budget"
    )
    print(
        f"files with debt: {totals['files_with_debt']}"
        f" of {totals['files']}; public items: {totals['public_items']}"
    )


def print_missing_paths(audits: Sequence[FileAudit]) -> None:
    """Prints the advisory list of source paths comments name that do not exist."""
    files = [audit for audit in audits if audit.missing_path_claims]
    if not files:
        print("path claims: every tracked source path named by a comment exists")
        return
    print(
        f"path claims: {len(files)} files name a source path not found in the"
        " working tree (a comment may name an external project's file)"
    )
    printed = 0
    for audit in files:
        for token in audit.missing_path_claims:
            print(f"  {audit.path}: {token}")
            printed += 1
            if printed >= MISSING_PATH_REPORT_LIMIT:
                print(f"  ... stopping after {MISSING_PATH_REPORT_LIMIT}")
                return


def markdown_table(audits: Sequence[FileAudit], limit: int) -> List[str]:
    """The ranking as a Markdown table."""
    rows = [
        "| Rank | Priority | Debt | Density | Pub | Proc | Hist | TODO | Long doc | File |",
        "|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|",
    ]
    for rank, audit in enumerate(audits[:limit], start=1):
        rows.append(
            f"| {rank} | {audit.priority():.1f} | {audit.debt()} | {audit.density():.1f}"
            f" | {audit.public_items} | {audit.process_total()} | {audit.history_total()}"
            f" | {audit.todo_count} | {audit.long_doc_excess()} | `{audit.path}` |"
        )
    return rows


def render_markdown(
    root: Path,
    audits: Sequence[FileAudit],
    totals: Dict[str, int],
    generated_at: str,
    sort_key: str,
    limit: int,
) -> str:
    """A standalone report: scope, method, rankings and what to protect."""
    ordered = order_audits(audits, sort_key)
    density_ranked = [audit for audit in audits if audit.debt() >= DENSITY_TABLE_MINIMUM_DEBT]
    density_ranked.sort(key=lambda audit: (-audit.density(), audit.path))
    protect = sorted(audits, key=lambda audit: (-audit.safety_comments, audit.path))
    protect = [audit for audit in protect if audit.safety_comments > 0][:10]

    lines = [
        f"# Comment audit - {generated_at}",
        "",
        f"Scope: `{display_path(root)}`.",
        "",
        "## Totals",
        "",
        f"- {totals['files']} Rust files, {totals['lines']} lines,"
        f" {totals['comment_lines']} comment lines"
        f" ({totals['doc_lines']} doc, {totals['comment_lines'] - totals['doc_lines']} plain)",
        f"- Process references: {totals['process_references']}"
        f" (work item {totals['process_work_item']}, stage {totals['process_stage']},"
        f" plan decision {totals['process_plan_decision']}, site path {totals['process_site_path']})",
        f"- History references: {totals['history_references']};"
        f" advisory history wording: {totals['advisory_history_total']}",
        f"- TODO markers: {totals['todo_markers']}",
        f"- Long-doc excess: {totals['long_doc_excess']} lines"
        f" (item docs {totals['item_doc_excess']}, module headers {totals['module_doc_excess']})"
        f" across {totals['files_with_long_doc']} files",
        f"- Files with debt: {totals['files_with_debt']} of {totals['files']};"
        f" public items: {totals['public_items']}",
        "",
        "## Method",
        "",
        "```text",
        "debt     = 3 * process refs + 2 * history refs + 2 * TODO markers",
        "           + 1 * excess doc lines",
        "density  = debt per 100 comment lines",
        "priority = debt * (1 + min(public items, 50) / 10)",
        "```",
        "",
        f"Item doc blocks are budgeted at {ITEM_DOC_BLOCK_BUDGET} lines and module"
        f" headers at {MODULE_DOC_BLOCK_BUDGET}, with per-block excess capped at"
        f" {DOC_BLOCK_EXCESS_CAP}. Process references are work-item ids,"
        " stage numbers, plan decisions and construction-site paths. The scan"
        " counts full-line comments only. Safety comments are counted separately"
        " as value to protect, not as debt.",
        "",
        f"## Top files by {sort_key}",
        "",
    ]
    lines.extend(markdown_table(ordered, limit))
    lines.extend(["", "## Concentrated debt (density per 100 comment lines)", ""])
    lines.extend(markdown_table(density_ranked, DENSITY_TABLE_LIMIT))
    lines.extend(
        [
            "",
            "## Value to protect (most SAFETY comments)",
            "",
            "| SAFETY comments | File |",
            "|---:|---|",
        ]
    )
    for audit in protect:
        lines.append(f"| {audit.safety_comments} | `{audit.path}` |")
    if not protect:
        lines.append("| 0 | none |")
    lines.append("")
    return "\n".join(lines)


def audit_to_dict(audit: FileAudit) -> Dict[str, object]:
    """The JSON shape of one file's measurements."""
    record = asdict(audit)
    record.update(
        {
            "process_total": audit.process_total(),
            "history_total": audit.history_total(),
            "advisory_history_total": audit.advisory_history_total(),
            "long_doc_excess": audit.long_doc_excess(),
            "debt": audit.debt(),
            "density": round(audit.density(), 3),
            "surface_factor": round(audit.surface_factor(), 3),
            "priority": round(audit.priority(), 3),
        }
    )
    return record


def write_json(path: Path, root: Path, audits: Sequence[FileAudit], generated_at: str) -> None:
    """Writes the full per-file record, ranked by priority."""
    payload = {
        "generated_at": generated_at,
        "root": display_path(root),
        "totals": summarize(audits),
        "files": [audit_to_dict(audit) for audit in order_audits(audits, "priority")],
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


SELF_TEST_SOURCE = "\n".join(
    ["//! Header (W-0001): stage 3 of the plan."]
    + [f"//! Module line {index}" for index in range(29)]
    + [""]
    + [f"/// Doc line {index}" for index in range(19)]
    + [
        "pub fn first() {}",
        "",
        "/// Previously used to be a struct.",
        "// TODO: retire",
        "pub struct Second;",
    ]
)


def run_self_test() -> int:
    """Checks the analyzer against a sample with known measurements."""
    audit = analyze_source("sample.rs", SELF_TEST_SOURCE)
    expectations = (
        ("process references", audit.process_total(), 2),
        ("history references", audit.history_total(), 2),
        ("TODO markers", audit.todo_count, 1),
        ("public items", audit.public_items, 2),
        ("longest doc block", audit.longest_doc_block, 30),
        ("item doc excess", audit.item_doc_excess, 19 - ITEM_DOC_BLOCK_BUDGET),
        ("module doc excess", audit.module_doc_excess, 30 - MODULE_DOC_BLOCK_BUDGET),
    )
    failures = 0
    for label, actual, expected in expectations:
        if actual != expected:
            print(f"self-test: {label} = {actual}, expected {expected}")
            failures += 1
    if failures:
        print(f"self-test failed: {failures} mismatch(es)")
        return 1
    print("self-test passed: process, history, TODO, public surface and doc length")
    return 0


def build_parser() -> argparse.ArgumentParser:
    """The command line: scan scope, ranking, and optional outputs."""
    parser = argparse.ArgumentParser(
        description="Rank Rust sources by comment debt and public surface."
    )
    parser.add_argument(
        "--root",
        default="modules",
        help="directory to scan, absolute or relative to the repository root",
    )
    parser.add_argument("--top", type=int, default=25, help="rows per table")
    parser.add_argument(
        "--sort",
        choices=("priority", "debt", "density", "surface", "path"),
        default="priority",
        help="ranking key for the main table",
    )
    parser.add_argument(
        "--min-debt",
        type=int,
        default=0,
        help="hide files whose debt is below this value",
    )
    parser.add_argument("--json", dest="json_path", help="write the per-file record")
    parser.add_argument("--markdown", dest="markdown_path", help="write a report")
    parser.add_argument(
        "--check-paths",
        action="store_true",
        help="also report source paths named by comments that do not exist",
    )
    parser.add_argument("--self-test", action="store_true", help="check the analyzer")
    return parser


def main(argv: Optional[Sequence[str]] = None) -> int:
    """Scans the tree, prints the ranking, and writes any requested reports."""
    arguments = build_parser().parse_args(argv)
    if arguments.self_test:
        return run_self_test()
    root = resolve_scan_root(arguments.root)
    if not root.is_dir():
        print(f"error: root is not a directory: {root}", file=sys.stderr)
        return 2

    files = tracked_rust_files(root)
    existing_paths = load_existing_relative_paths() if arguments.check_paths else None
    audits = []
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        audits.append(analyze_source(display_path(path), text, existing_paths))

    generated_at = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    totals = summarize(audits)
    print_summary(root, audits, totals)

    ranked = [
        audit
        for audit in order_audits(audits, arguments.sort)
        if audit.debt() >= arguments.min_debt
    ]
    print("")
    print(f"Top files by {arguments.sort}:")
    print("\n".join(format_table(ranked, arguments.top)))

    if arguments.sort != "density":
        density_ranked = [
            audit for audit in audits if audit.debt() >= DENSITY_TABLE_MINIMUM_DEBT
        ]
        density_ranked.sort(key=lambda audit: (-audit.density(), audit.path))
        print("")
        print("Concentrated debt (density per 100 comment lines):")
        print("\n".join(format_table(density_ranked, DENSITY_TABLE_LIMIT)))

    if arguments.check_paths:
        print("")
        print_missing_paths(audits)

    if arguments.json_path:
        output = Path(arguments.json_path)
        if not output.is_absolute():
            output = Path.cwd() / output
        write_json(output, root, audits, generated_at)
        print("")
        print(f"json: {output}")

    if arguments.markdown_path:
        output = Path(arguments.markdown_path)
        if not output.is_absolute():
            output = Path.cwd() / output
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(
            render_markdown(
                root, audits, totals, generated_at, arguments.sort, arguments.top
            ),
            encoding="utf-8",
        )
        print(f"markdown: {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
