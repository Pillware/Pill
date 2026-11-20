"""Generate rustdoc JSON and convert it into a tree of Markdown pages.

This script is the complete Markdown documentation pipeline. It first runs
``generate_documentation.py --json`` with Cargo's target directory redirected
to an automatically cleaned temporary folder, then converts the generated JSON:

    python devops/tools/generate_documentation_markdown.py

The converter validates and renders every temporary JSON input before deleting
any existing output. It then purges its default output directory and writes a
rustdoc-style Markdown tree there: crate/module ``index.md`` files and one page
per named API item, plus the VitePress sidebar module inside the tree. All of
it defaults to the repository's gitignored ``local/docs_build/reference/``
folder; copying that folder into a website checkout is a separate, deliberate
step. Temporary Cargo artifacts and JSON files are removed automatically when
the script finishes or fails.

Rustdoc JSON is an unstable nightly interface. Unknown future type variants are
rendered conservatively instead of aborting the entire conversion.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence, Set, Tuple


# =============================================================================
# Repository locations, derived from this script's own path
# =============================================================================

# The generator and Cargo both run from the Cargo workspace directory, which
# is modules/ rather than the repository root that holds this script's folder.
REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
WORKSPACE_ROOT = REPOSITORY_ROOT / "modules"
DOCUMENTATION_GENERATOR_SCRIPT = Path(__file__).resolve().with_name(
    "generate_documentation.py"
)
# The generated tree is purged and rewritten on every run. The sidebar module
# lives inside that tree; it is safe there because its text is rendered from
# in-memory data before the purge and written after the pages, and a run that
# fails before the purge leaves the previous tree, sidebar included, untouched.
# Both paths stay under the repository's gitignored local/ folder, so a run
# never touches a website checkout that happens to live beside this repository.
OUTPUT_DIRECTORY = REPOSITORY_ROOT / "local" / "docs_build" / "reference"
SIDEBAR_DATA_FILE = OUTPUT_DIRECTORY / "reference-sidebar.ts"
CODE_REFERENCE_ROUTE = "/reference"

# Primary items appear as full entries in a crate page. Fields, variants, and
# associated items are rendered beneath their owning primary item instead.
PRIMARY_ITEM_KINDS = {
    "module",
    "struct",
    "enum",
    "union",
    "trait",
    "trait_alias",
    "function",
    "type_alias",
    "constant",
    "static",
    "macro",
    "proc_macro",
    "extern_type",
    "primitive",
}

ITEM_KIND_ORDER = [
    "module",
    "struct",
    "enum",
    "union",
    "trait",
    "trait_alias",
    "function",
    "type_alias",
    "constant",
    "static",
    "macro",
    "proc_macro",
    "extern_type",
    "primitive",
]

ITEM_KIND_TITLES = {
    "module": "Modules",
    "struct": "Structs",
    "enum": "Enums",
    "union": "Unions",
    "trait": "Traits",
    "trait_alias": "Trait aliases",
    "function": "Functions",
    "type_alias": "Type aliases",
    "constant": "Constants",
    "static": "Statics",
    "macro": "Macros",
    "proc_macro": "Procedural macros",
    "extern_type": "External types",
    "primitive": "Primitive types",
}

# Rustdoc's HTML backend prefixes item filenames with their kind, for example
# ``struct.World.html`` and ``fn.setup.html``. Reusing that convention with a
# ``.md`` extension makes the generated source tree immediately familiar.
ITEM_FILE_PREFIXES = {
    "struct": "struct",
    "enum": "enum",
    "union": "union",
    "trait": "trait",
    "trait_alias": "traitalias",
    "function": "fn",
    "type_alias": "type",
    "constant": "constant",
    "static": "static",
    "macro": "macro",
    "proc_macro": "macro",
    "extern_type": "type",
    "primitive": "primitive",
}

ITEM_KIND_LABELS = {
    "struct": "Struct",
    "enum": "Enum",
    "union": "Union",
    "trait": "Trait",
    "trait_alias": "Trait alias",
    "function": "Function",
    "type_alias": "Type alias",
    "constant": "Constant",
    "static": "Static",
    "macro": "Macro",
    "proc_macro": "Procedural macro",
    "extern_type": "External type",
    "primitive": "Primitive type",
}


# =============================================================================
# General helpers
# =============================================================================


# Rustdoc stores an item's concrete variant as the sole key under ``inner``.
def item_kind(item: Dict[str, Any]) -> str:
    """Return the single rustdoc item variant stored inside ``inner``."""
    # Missing or malformed item bodies degrade to ``unknown`` so callers can
    # render a conservative fallback instead of crashing on an unstable schema.
    inner = item.get("inner", {})
    return next(iter(inner), "unknown")


# IDs have changed representation across rustdoc JSON schema revisions.
def item_identifier(value: Any) -> str:
    """Normalize numeric or string rustdoc IDs for dictionary lookup."""
    # A single string representation makes lookups stable across both forms.
    return str(value)


# Explicit anchors prevent generated links from depending on heading slug rules.
def markdown_anchor(identifier: Any) -> str:
    """Create a deterministic anchor independent of VitePress slug rules."""
    # Prefix the raw ID because a bare numeric HTML ID is difficult to inspect
    # and could collide with author-written anchors in embedded documentation.
    return "rustdoc-item-{}".format(item_identifier(identifier))


# Generated names become real directories and filenames on Windows.
def safe_path_component(value: str) -> str:
    """Return a Windows-safe path component while preserving Rust names."""
    # Keep familiar Rust identifier punctuation while replacing separators,
    # generic syntax, and other characters that are unsafe in a path segment.
    sanitized = re.sub(r"[^A-Za-z0-9_.-]+", "_", value).strip(". ")
    # Never return an empty component, even for a fully unsupported input name.
    return sanitized or "unnamed"


# Sidebar labels should be readable prose rather than raw crate identifiers.
def display_name(value: str) -> str:
    """Turn a snake-case crate name into a compact sidebar label."""
    # Ignore repeated underscores so they do not introduce empty words.
    return " ".join(part.capitalize() for part in value.split("_") if part)


# Markdown code spans require a longer delimiter when their text has backticks.
def code_span(value: str) -> str:
    """Wrap text in a Markdown code span, safely handling embedded backticks."""
    # The common path stays compact; the doubled delimiter protects unusual
    # macro names or generated text containing a literal single backtick.
    if "`" not in value:
        return "`{}`".format(value)
    return "``{}``".format(value)


# Index tables need a short summary rather than an item's complete prose docs.
def first_documentation_line(documentation: Optional[str]) -> str:
    """Return a compact summary suitable for a contents table."""
    # Undocumented items intentionally contribute an empty summary.
    if not documentation:
        return ""
    # Skip blank lines and strip heading markers from common rustdoc openings.
    for line in documentation.splitlines():
        candidate = line.strip().lstrip("#").strip()
        if candidate:
            return candidate
    return ""


# Embedded rustdoc sections must sit below the generated page's own headings.
def demote_markdown_headings(documentation: str, offset: int) -> str:
    """Demote rustdoc headings so embedded docs fit the generated page outline.

    Headings inside fenced code blocks are deliberately left untouched.
    """
    # Track fenced blocks explicitly because example source may legitimately
    # begin with ``#`` and must never be interpreted as a Markdown heading.
    output: List[str] = []
    inside_fence = False
    for line in documentation.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            inside_fence = not inside_fence
            output.append(line)
            continue

        # Only ATX headings outside fences are adjusted. Clamp at Markdown's
        # maximum heading depth so deeply nested docs remain valid Markdown.
        match = None if inside_fence else re.match(r"^(#{1,6})(\s+.*)$", line)
        if match:
            level = min(6, len(match.group(1)) + offset)
            output.append("{}{}".format("#" * level, match.group(2)))
        else:
            output.append(line)
    return "\n".join(output)


# Vue parses raw HTML embedded in Markdown. Rust prose such as ``Res<T>`` or
# ``Box<dyn Component>`` therefore looks like a Vue component/tag unless its
# angle brackets are escaped. Preserve genuine HTML supported by Markdown and
# transform only tag-shaped fragments that are not standard HTML elements.
HTML_TAG_NAMES = {
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "caption",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "samp",
    "small",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
    "var",
}


# Protect Rust generic syntax from Vue while retaining deliberate raw HTML.
def escape_rust_generic_brackets(documentation: str) -> str:
    """Escape non-HTML ``<...>`` fragments outside code fences and code spans."""
    # Fenced examples and inline-code spans are already protected by Markdown,
    # so only ordinary prose is eligible for escaping.
    output: List[str] = []
    inside_fence = False

    # Process a prose fragment that is known not to contain inline code.
    def escape_text(text: str) -> str:
        # Decide whether one angle-bracketed fragment is HTML or Rust syntax.
        def replace_tag(match: re.Match) -> str:
            content = match.group(1)
            stripped = content.strip()
            # Autolinks use angle brackets but must remain intact for Markdown.
            if stripped.startswith(("http://", "https://", "mailto:")):
                return match.group(0)
            tag_match = re.match(r"/?\s*([A-Za-z][A-Za-z0-9-]*)", stripped)
            # Preserve only recognized HTML tags; unknown tag-shaped values are
            # treated as Rust generics so Vue does not try to resolve them.
            if tag_match and tag_match.group(1).lower() in HTML_TAG_NAMES:
                return match.group(0)
            return "&lt;{}&gt;".format(content)

        # Work from the innermost tag-shaped fragments on a single prose line.
        return re.sub(r"<([^<>\n]+)>", replace_tag, text)

    # Preserve original line structure while tracking fenced-code boundaries.
    for line in documentation.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            inside_fence = not inside_fence
            output.append(line)
            continue
        if inside_fence:
            output.append(line)
            continue

        # Markdown inline-code spans may use one or more backticks. Escape only
        # the prose between spans; Markdown itself protects the code contents.
        cursor = 0
        pieces: List[str] = []
        for code_match in re.finditer(r"(`+).*?\1", line):
            pieces.append(escape_text(line[cursor : code_match.start()]))
            pieces.append(code_match.group(0))
            cursor = code_match.end()
        pieces.append(escape_text(line[cursor:]))
        output.append("".join(pieces))
    return "\n".join(output)


# =============================================================================
# Rust type and signature rendering
# =============================================================================


# Generic arguments are tagged unions in rustdoc's JSON representation.
def render_generic_argument(argument: Dict[str, Any]) -> str:
    """Render one generic argument from rustdoc's tagged JSON representation."""
    # Test known variants in schema order and recursively render nested types.
    if "lifetime" in argument:
        return str(argument["lifetime"])
    if "type" in argument:
        return render_type(argument["type"])
    if "const" in argument:
        constant = argument["const"]
        if isinstance(constant, dict):
            return str(constant.get("expr") or constant.get("value") or "_")
        return str(constant)
    if "infer" in argument:
        return "_"
    # Unknown nightly variants become valid placeholder Rust syntax.
    return "_"


# Paths may use standard ``<T>`` arguments or function-like ``(T) -> U`` forms.
def render_generic_arguments(arguments: Optional[Dict[str, Any]]) -> str:
    """Render angle-bracketed or parenthesized path arguments."""
    # Most paths carry no generic metadata at all.
    if not arguments:
        return ""

    # Assemble ordinary type/lifetime/const arguments and associated bindings.
    angle_bracketed = arguments.get("angle_bracketed")
    if angle_bracketed is not None:
        values = [
            render_generic_argument(argument)
            for argument in angle_bracketed.get("args", [])
        ]
        for constraint in angle_bracketed.get("constraints", []):
            name = constraint.get("name", "_")
            binding = constraint.get("binding") or constraint.get("constraint") or {}
            if "equality" in binding:
                values.append(
                    "{} = {}".format(name, render_type(binding["equality"]))
                )
            elif "bounds" in binding:
                values.append(
                    "{}: {}".format(name, render_bounds(binding["bounds"]))
                )
        return "<{}>".format(", ".join(values)) if values else ""

    # Parenthesized arguments represent callable traits such as ``Fn(A) -> B``.
    parenthesized = arguments.get("parenthesized")
    if parenthesized is not None:
        inputs = ", ".join(
            render_type(input_type)
            for input_type in parenthesized.get("inputs", [])
        )
        output = parenthesized.get("output")
        suffix = " -> {}".format(render_type(output)) if output else ""
        return "({}){}".format(inputs, suffix)

    # Future schema variants are omitted without invalidating the whole path.
    return ""


# Bounds share a tagged representation for lifetimes, traits, and captures.
def render_bound(bound: Dict[str, Any]) -> str:
    """Render a trait, lifetime, or precise-capture generic bound."""
    # Lifetime/outlives bounds are already stored in source-like form.
    if "outlives" in bound:
        return str(bound["outlives"])

    # Trait bounds may be relaxed with ``?Trait`` and may carry arguments.
    trait_bound = bound.get("trait_bound")
    if trait_bound is not None:
        trait = trait_bound.get("trait", {})
        modifier = trait_bound.get("modifier", "none")
        prefix = "?" if modifier == "maybe" else ""
        return "{}{}{}".format(
            prefix,
            trait.get("path", "_"),
            render_generic_arguments(trait.get("args")),
        )

    # Precise-capture bounds use the newer ``use<...>`` Rust syntax.
    if "use" in bound:
        captures = bound["use"]
        if isinstance(captures, list):
            return "use<{}>".format(", ".join(map(str, captures)))
    # Maintain readable output when nightly introduces an unfamiliar variant.
    return "_"


# Multiple generic bounds use Rust's standard plus-separated notation.
def render_bounds(bounds: Sequence[Dict[str, Any]]) -> str:
    """Render a plus-separated list of generic bounds."""
    # Delegate each tagged entry so nested trait arguments are handled equally.
    return " + ".join(render_bound(bound) for bound in bounds)


# Rustdoc types are recursive tagged unions that require variant-by-variant output.
def render_type(type_data: Any) -> str:
    """Render rustdoc's recursive ``Type`` enum as Rust-like source text."""
    # Normalize absent, legacy string, and malformed values before inspecting
    # the mapping variants used by current nightly schemas.
    if type_data is None:
        return "()"
    if isinstance(type_data, str):
        return type_data
    if not isinstance(type_data, dict):
        return "_"

    # Leaf variants can be emitted without any recursive traversal.
    if "primitive" in type_data:
        return str(type_data["primitive"])
    if "generic" in type_data:
        return str(type_data["generic"])
    if "infer" in type_data:
        return "_"
    if "never" in type_data:
        return "!"

    # Resolved paths include optional nested generic arguments.
    resolved_path = type_data.get("resolved_path")
    if resolved_path is not None:
        return "{}{}".format(
            resolved_path.get("path", "_"),
            render_generic_arguments(resolved_path.get("args")),
        )

    # Qualified paths render associated types such as ``<T as Trait>::Item``.
    qualified_path = type_data.get("qualified_path")
    if qualified_path is not None:
        self_type = render_type(qualified_path.get("self_type"))
        trait = qualified_path.get("trait")
        if trait:
            owner = "{} as {}{}".format(
                self_type,
                trait.get("path", "_"),
                render_generic_arguments(trait.get("args")),
            )
        else:
            owner = self_type
        return "<{}>::{}{}".format(
            owner,
            qualified_path.get("name", "_"),
            render_generic_arguments(qualified_path.get("args")),
        )

    # References preserve optional lifetimes and mutability qualifiers.
    borrowed_reference = type_data.get("borrowed_ref")
    if borrowed_reference is not None:
        lifetime = borrowed_reference.get("lifetime")
        lifetime_text = "{} ".format(lifetime) if lifetime else ""
        mutable_text = "mut " if borrowed_reference.get("is_mutable") else ""
        return "&{}{}{}".format(
            lifetime_text,
            mutable_text,
            render_type(borrowed_reference.get("type")),
        )

    # Raw pointers require an explicit ``const`` or ``mut`` qualifier.
    raw_pointer = type_data.get("raw_pointer")
    if raw_pointer is not None:
        qualifier = "mut" if raw_pointer.get("is_mutable") else "const"
        return "*{} {}".format(qualifier, render_type(raw_pointer.get("type")))

    # Container forms recursively render all member types.
    if "tuple" in type_data:
        members = [render_type(member) for member in type_data["tuple"]]
        if len(members) == 1:
            return "({},)".format(members[0])
        return "({})".format(", ".join(members))
    if "slice" in type_data:
        return "[{}]".format(render_type(type_data["slice"]))
    if "array" in type_data:
        array = type_data["array"]
        return "[{}; {}]".format(
            render_type(array.get("type")), array.get("len", "_")
        )

    # Opaque and dynamic trait objects share the bound renderer but differ in
    # their leading Rust keyword and lifetime representation.
    implementation_trait = type_data.get("impl_trait")
    if implementation_trait is not None:
        return "impl {}".format(render_bounds(implementation_trait))

    dynamic_trait = type_data.get("dyn_trait")
    if dynamic_trait is not None:
        traits = []
        for poly_trait in dynamic_trait.get("traits", []):
            trait = poly_trait.get("trait", {})
            traits.append(
                "{}{}".format(
                    trait.get("path", "_"),
                    render_generic_arguments(trait.get("args")),
                )
            )
        lifetime = dynamic_trait.get("lifetime")
        if lifetime:
            traits.append(str(lifetime))
        return "dyn {}".format(" + ".join(traits) or "_")

    # Function pointers reuse the function-signature renderer, then drop the
    # declaration semicolon that is appropriate only for item signatures.
    function_pointer = type_data.get("function_pointer")
    if function_pointer is not None:
        header = render_function_header(function_pointer.get("header", {}))
        signature = render_function_signature(
            "",
            function_pointer.get("sig", {}),
            visibility="",
            header=header,
            generic_text="",
        )
        return signature.rstrip(";")

    # The JSON schema is explicitly unstable. A readable underscore is safer
    # than embedding implementation-shaped JSON into a Rust signature.
    return "_"


# Generic declarations contain separate type, lifetime, and const variants.
def render_generic_parameters(generics: Optional[Dict[str, Any]]) -> str:
    """Render generic parameter declarations, excluding where predicates."""
    # Avoid emitting empty angle brackets for non-generic items.
    if not generics:
        return ""
    parameters: List[str] = []
    # Preserve the rustdoc-provided declaration order for familiar signatures.
    for parameter in generics.get("params", []):
        name = parameter.get("name", "_")
        kind = parameter.get("kind", {})
        type_parameter = kind.get("type")
        # Type parameters may carry both bounds and a default type.
        if type_parameter is not None:
            bounds = render_bounds(type_parameter.get("bounds", []))
            default = type_parameter.get("default")
            text = "{}: {}".format(name, bounds) if bounds else name
            if default is not None:
                text += " = {}".format(render_type(default))
            parameters.append(text)
        # Lifetime parameters can themselves outlive one or more lifetimes.
        elif "lifetime" in kind:
            outlives = kind["lifetime"].get("outlives", [])
            suffix = ": {}".format(" + ".join(outlives)) if outlives else ""
            parameters.append("{}{}".format(name, suffix))
        # Const generics include a required type and an optional default value.
        elif "const" in kind:
            constant = kind["const"]
            text = "const {}: {}".format(name, render_type(constant.get("type")))
            if constant.get("default") is not None:
                text += " = {}".format(constant["default"])
            parameters.append(text)
        else:
            # Retain the name if nightly adds a parameter kind we do not know.
            parameters.append(name)
    return "<{}>".format(", ".join(parameters)) if parameters else ""


# Where clauses use a second set of tagged predicate representations.
def render_where_predicates(generics: Optional[Dict[str, Any]]) -> str:
    """Render the most common rustdoc where-predicate forms."""
    # Items without generics cannot contribute a where clause.
    if not generics:
        return ""
    predicates: List[str] = []
    # Keep predicate order from source metadata and skip unknown nightly forms.
    for predicate in generics.get("where_predicates", []):
        bound_predicate = predicate.get("bound_predicate")
        if bound_predicate is not None:
            bounds = render_bounds(bound_predicate.get("bounds", []))
            predicates.append(
                "{}: {}".format(render_type(bound_predicate.get("type")), bounds)
            )
            continue
        # Lifetime predicates join all outlives requirements with ``+``.
        lifetime_predicate = predicate.get("lifetime_predicate")
        if lifetime_predicate is not None:
            predicates.append(
                "{}: {}".format(
                    lifetime_predicate.get("lifetime", "_"),
                    " + ".join(lifetime_predicate.get("outlives", [])),
                )
            )
            continue
        # Equality predicates connect two recursively rendered types.
        equality_predicate = predicate.get("eq_predicate")
        if equality_predicate is not None:
            predicates.append(
                "{} = {}".format(
                    render_type(equality_predicate.get("lhs")),
                    render_type(equality_predicate.get("rhs")),
                )
            )
    return " where {}".format(", ".join(predicates)) if predicates else ""


# Visibility metadata is normalized into a prefix shared by every item renderer.
def render_visibility(visibility: Any) -> str:
    """Translate rustdoc visibility metadata to a Rust source prefix."""
    # Handle common public and crate-wide shorthand variants first.
    if visibility == "public":
        return "pub "
    if visibility == "crate":
        return "pub(crate) "
    # Restricted visibility carries a module path in an object variant.
    if isinstance(visibility, dict) and "restricted" in visibility:
        path = visibility["restricted"].get("path", "crate")
        normalized = str(path).lstrip(":") or "crate"
        return "pub(in {}) ".format(normalized)
    # Private/default visibility has no source prefix.
    return ""


# ABI metadata can be a simple name or a tagged object with unwind support.
def render_abi(abi: Any) -> str:
    """Render Rust or named external ABIs."""
    # Rust is the implicit ABI and should not add noise to signatures.
    if abi in (None, "Rust"):
        return ""
    if isinstance(abi, str):
        return 'extern "{}" '.format(abi)
    # Newer schemas encode the ABI name as the object's sole key.
    if isinstance(abi, dict) and abi:
        name = next(iter(abi))
        unwind = abi[name].get("unwind") if isinstance(abi[name], dict) else False
        suffix = "-unwind" if unwind else ""
        return 'extern "{}{}" '.format(name, suffix)
    # Malformed or unknown ABI metadata is safest to omit.
    return ""


# Function qualifiers must appear in Rust's conventional source order.
def render_function_header(header: Dict[str, Any]) -> str:
    """Render qualifiers appearing before the ``fn`` keyword."""
    # Accumulate only enabled qualifiers so spacing can be handled once.
    qualifiers = []
    if header.get("is_const"):
        qualifiers.append("const")
    if header.get("is_async"):
        qualifiers.append("async")
    if header.get("is_unsafe"):
        qualifiers.append("unsafe")
    # ABI text is normalized separately because its JSON shape is versioned.
    abi = render_abi(header.get("abi"))
    if abi:
        qualifiers.append(abi.strip())
    return "{} ".format(" ".join(qualifiers)) if qualifiers else ""


# Both item functions and function-pointer types share this signature formatter.
def render_function_signature(
    name: str,
    signature: Dict[str, Any],
    *,
    visibility: str,
    header: str,
    generic_text: str,
    where_text: str = "",
) -> str:
    """Render a function or function-pointer signature."""
    # Named item parameters use ``name: Type``; function-pointer metadata can
    # omit names, in which case only the type should be shown.
    inputs = []
    for parameter_name, parameter_type in signature.get("inputs", []):
        if parameter_name:
            inputs.append("{}: {}".format(parameter_name, render_type(parameter_type)))
        else:
            inputs.append(render_type(parameter_type))
    # C-variadic functions carry an additional terminal ellipsis parameter.
    if signature.get("is_c_variadic"):
        inputs.append("...")
    # Rustdoc represents the unit/default return as an absent output value.
    output = signature.get("output")
    output_text = " -> {}".format(render_type(output)) if output else ""
    return "{}{}fn {}{}({}){}{};".format(
        visibility,
        header,
        name,
        generic_text,
        ", ".join(inputs),
        output_text,
        where_text,
    )


# =============================================================================
# Markdown renderer for one rustdoc crate
# =============================================================================

class CrateMarkdownRenderer:
    """Turn one rustdoc crate graph into a self-contained Markdown document."""

    # Initialize graph indexes once so page rendering can perform cheap lookups.
    def __init__(self, document: Dict[str, Any], source_file: Path) -> None:
        # Retain the raw document for crate metadata and normalize the root ID at
        # the boundary because rustdoc versions may encode IDs differently.
        self.document = document
        self.source_file = source_file
        self.index: Dict[str, Dict[str, Any]] = document["index"]
        self.root_identifier = item_identifier(document["root"])
        # These indexes connect rustdoc graph IDs to canonical names, output
        # pages, module membership, and documentation-link candidates.
        self.paths: Dict[str, str] = {}
        self.primary_identifiers: List[str] = []
        self.rendered_identifiers: Set[str] = set()
        self.page_paths: Dict[str, Path] = {}
        self.module_children: Dict[str, List[str]] = {}
        self.module_uses: Dict[str, List[str]] = {}
        self.link_name_identifiers: Dict[str, List[str]] = {}
        # Discovery must precede link indexing because associated items inherit
        # their owner's page during graph traversal.
        self._discover_items()
        self._index_documentation_link_targets()

    # Centralize ID normalization for every access to rustdoc's item index.
    def get_item(self, identifier: Any) -> Optional[Dict[str, Any]]:
        """Find an item in rustdoc's string-keyed item index."""
        # Missing references are common for external crates and remain optional.
        return self.index.get(item_identifier(identifier))

    # Build the page map and ownership relationships used by all later stages.
    def _discover_items(self) -> None:
        """Walk the module tree and establish canonical local item paths."""
        # The crate root anchors the recursive module traversal and its output
        # directory; without it, no reliable local hierarchy can be generated.
        root = self.get_item(self.root_identifier)
        if root is None:
            raise ValueError("rustdoc root item is missing from the item index")
        crate_name = root.get("name") or self.source_file.stem
        self.paths[self.root_identifier] = crate_name
        crate_directory = Path(safe_path_component(crate_name))
        self.page_paths[self.root_identifier] = crate_directory / "index.md"
        # Malformed or re-export-heavy graphs can revisit a module, so track IDs
        # to prevent recursion cycles and duplicate child entries.
        visited_modules: Set[str] = set()

        # Recursively register one local module and each direct child item.
        def walk_module(
            identifier: str, module_path: str, module_directory: Path
        ) -> None:
            # Stop cycles before reading children and initialize empty buckets so
            # modules without contents still have predictable lookup entries.
            if identifier in visited_modules:
                return
            visited_modules.add(identifier)
            self.module_children.setdefault(identifier, [])
            self.module_uses.setdefault(identifier, [])
            module_item = self.get_item(identifier)
            if module_item is None:
                return
            module_data = module_item.get("inner", {}).get("module", {})
            # Only crate-local items receive generated pages. External targets
            # may be linkable metadata but cannot be rendered from this JSON.
            for child_value in module_data.get("items", []):
                child_identifier = item_identifier(child_value)
                child = self.get_item(child_identifier)
                if child is None or child.get("crate_id") != 0:
                    continue
                kind = item_kind(child)
                name = child.get("name")
                # Re-exports remain listed on the module page rather than
                # receiving misleading standalone pages of their own.
                if kind == "use":
                    self.module_uses[identifier].append(child_identifier)
                    self.page_paths.setdefault(
                        child_identifier, module_directory / "index.md"
                    )
                    continue
                # Anonymous entries and impl containers are rendered beneath an
                # owning named item, never as primary navigation destinations.
                if not name or kind == "impl":
                    continue
                child_path = "{}::{}".format(module_path, name)
                self.paths.setdefault(child_identifier, child_path)
                # Primary items receive rustdoc-style filenames; nested modules
                # receive directories containing their own ``index.md``.
                if kind in PRIMARY_ITEM_KINDS:
                    self.primary_identifiers.append(child_identifier)
                    self.module_children[identifier].append(child_identifier)
                    if kind == "module":
                        child_directory = module_directory / safe_path_component(name)
                        child_page = child_directory / "index.md"
                    else:
                        prefix = ITEM_FILE_PREFIXES.get(kind, kind)
                        child_directory = module_directory
                        child_page = child_directory / "{}.{}.md".format(
                            prefix, safe_path_component(name)
                        )
                    self.page_paths.setdefault(child_identifier, child_page)
                # Continue traversal only after its path and output directory
                # have been established above.
                if kind == "module":
                    walk_module(child_identifier, child_path, child_directory)

        # Begin at the crate root so every discovered path is crate-qualified.
        walk_module(self.root_identifier, crate_name, crate_directory)

        # Public path summaries can supply canonical names for items referenced
        # by documentation links. Do not add those items to the primary list:
        # rustdoc also stores associated methods in ``paths``, and methods must
        # remain nested below their owning type instead of appearing twice as
        # module-level functions.
        for identifier, summary in self.document.get("paths", {}).items():
            normalized_identifier = item_identifier(identifier)
            if summary.get("crate_id") != 0:
                continue
            path = summary.get("path", [])
            if path:
                self.paths.setdefault(normalized_identifier, "::".join(path))

        # Preserve discovery order while removing duplicate graph references.
        self.primary_identifiers = list(dict.fromkeys(self.primary_identifiers))
        # Related fields, variants, and methods share their primary owner's page
        # and must participate in intra-documentation link resolution.
        self.rendered_identifiers.update(self.primary_identifiers)
        self.rendered_identifiers.add(self.root_identifier)
        for identifier in self.primary_identifiers:
            related_identifiers = self._related_item_identifiers(identifier)
            self.rendered_identifiers.update(related_identifiers)
            owner_page = self.page_paths[identifier]
            for related_identifier in related_identifiers:
                self.page_paths.setdefault(related_identifier, owner_page)
        # Use entries are rendered inline on module pages and can own link IDs.
        for use_identifiers in self.module_uses.values():
            self.rendered_identifiers.update(use_identifiers)

    # Build a reverse name index used when rustdoc omits explicit link IDs.
    def _index_documentation_link_targets(self) -> None:
        """Index rendered items by Rust name for explicit intra-doc links.

        Rustdoc normally supplies an ID map for shortcut links such as
        ``[`World`]``. Macro-generated documentation can instead contain
        explicit destinations such as ``[props](ViewportProps)`` or
        ``[`field`](ViewportProps::field)``; these are not always present in
        that map, so the converter also resolves them from the item graph.
        """
        # Only index items that will actually exist in generated output; linking
        # to an external or skipped item would create a broken VitePress URL.
        for identifier in self.rendered_identifiers:
            item = self.get_item(identifier)
            if item is None:
                continue
            name = item.get("name")
            if not name:
                continue
            # Retain every candidate because common method names are ambiguous
            # until the source page and qualified destination are considered.
            self.link_name_identifiers.setdefault(name, []).append(identifier)

    # Associated content is embedded below its owning type or trait page.
    def _related_item_identifiers(self, identifier: str) -> Set[str]:
        """Collect fields, variants, trait members, and inherent impl members."""
        # Missing graph nodes contribute no related content.
        item = self.get_item(identifier)
        if item is None:
            return set()
        kind = item_kind(item)
        data = item.get("inner", {}).get(kind, {})
        related: Set[str] = set()

        # Each aggregate kind stores children under a slightly different schema
        # shape, so normalize all of them into one identifier set.
        if kind == "struct":
            structure_kind = data.get("kind")
            if isinstance(structure_kind, dict):
                if "plain" in structure_kind:
                    related.update(
                        item_identifier(value)
                        for value in structure_kind["plain"].get("fields", [])
                    )
                elif "tuple" in structure_kind:
                    related.update(
                        item_identifier(value)
                        for value in structure_kind["tuple"]
                        if value is not None
                    )
        elif kind == "union":
            related.update(item_identifier(value) for value in data.get("fields", []))
        elif kind == "enum":
            related.update(item_identifier(value) for value in data.get("variants", []))
        elif kind == "trait":
            related.update(item_identifier(value) for value in data.get("items", []))

        # Only inherent implementations belong in the owning page's method
        # section; trait implementations are summarized separately.
        if kind in {"struct", "enum", "union"}:
            for implementation_value in data.get("impls", []):
                implementation = self.get_item(implementation_value)
                if implementation is None:
                    continue
                implementation_data = implementation.get("inner", {}).get("impl", {})
                if implementation_data.get("trait") is None:
                    related.update(
                        item_identifier(value)
                        for value in implementation_data.get("items", [])
                    )
        return related

    # Every intra-doc link is emitted relative to its current Markdown page.
    def _relative_target(
        self, current_page: Path, target_identifier: Any
    ) -> Optional[str]:
        """Return a relative Markdown URL for a rendered rustdoc item."""
        # A target without an assigned page was intentionally not rendered.
        normalized_identifier = item_identifier(target_identifier)
        target_page = self.page_paths.get(normalized_identifier)
        if target_page is None:
            return None
        # Items embedded on the same page need only a fragment link.
        anchor = markdown_anchor(normalized_identifier)
        if target_page == current_page:
            return "#{}".format(anchor)
        # VitePress URLs always use forward slashes, including on Windows.
        relative = os.path.relpath(
            str(target_page), start=str(current_page.parent)
        ).replace("\\", "/")
        return "{}#{}".format(relative, anchor)

    # Resolve explicit Rust-path destinations that lack rustdoc's ID mapping.
    def _named_documentation_target(
        self, destination: str, current_page: Path
    ) -> Optional[str]:
        """Resolve a Rust path used directly as a Markdown link destination.

        Nearby pages are preferred when a type name occurs more than once.
        Qualified associated-item links, for example ``Props::value``, are
        matched to an item that shares the owning type's generated page.
        Ambiguous matches are deliberately left unchanged rather than linking
        to an arbitrary API item.
        """
        # Remove Markdown autolink wrapping but reject already-relative URLs and
        # page fragments, which must remain authored exactly as supplied.
        candidate_destination = destination.strip().strip("<>")
        if candidate_destination.startswith(("#", "/", "./", "../")):
            return None
        # A single colon marks a URI scheme (``https:`` or ``mailto:``), while
        # Rust paths use a double colon and must continue through resolution.
        if re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:(?!:)", candidate_destination):
            return None
        if not re.fullmatch(
            r"(?:r#)?[A-Za-z_][A-Za-z0-9_]*(?:::(?:r#)?[A-Za-z_][A-Za-z0-9_]*)*",
            candidate_destination,
        ):
            return None

        # Leading Rust scope keywords do not contribute to the generated local
        # path and can be removed before name-based matching.
        segments = candidate_destination.split("::")
        while segments and segments[0] in {"crate", "self", "super"}:
            segments.pop(0)
        if not segments:
            return None

        # Raw identifier prefixes affect parsing, not the display name in the
        # rustdoc index.
        target_name = segments[-1].removeprefix("r#")
        candidates = list(self.link_name_identifiers.get(target_name, []))
        if not candidates:
            return None

        # Associated items frequently have non-unique names such as ``stats``.
        # The preceding path segment identifies their owning type, whose page
        # is also the page on which rustdoc renders the associated item.
        if len(segments) >= 2:
            owner_name = segments[-2].removeprefix("r#")
            owner_pages = {
                self.page_paths[identifier]
                for identifier in self.link_name_identifiers.get(owner_name, [])
                if identifier in self.page_paths
            }
            owned_candidates = [
                identifier
                for identifier in candidates
                if self.page_paths.get(identifier) in owner_pages
            ]
            if len(owned_candidates) == 1:
                return self._relative_target(current_page, owned_candidates[0])
            if owned_candidates:
                candidates = owned_candidates

        requested_suffix = "::".join(segment.removeprefix("r#") for segment in segments)

        # Rank remaining ambiguous candidates by locality and canonical path.
        def score(identifier: str) -> int:
            # Same-page anchors are safest, then sibling pages, exact path
            # matches, and finally standalone primary items.
            target_page = self.page_paths[identifier]
            value = 0
            if target_page == current_page:
                value += 400
            if target_page.parent == current_page.parent:
                value += 200
            canonical_path = self.paths.get(identifier, "")
            if canonical_path == requested_suffix or canonical_path.endswith(
                "::{}".format(requested_suffix)
            ):
                value += 100
            if identifier in self.primary_identifiers:
                value += 10
            return value

        # A tied top score is intentionally unresolved rather than arbitrary.
        ranked = sorted(
            ((score(identifier), identifier) for identifier in candidates),
            reverse=True,
        )
        if len(ranked) > 1 and ranked[0][0] == ranked[1][0]:
            return None
        return self._relative_target(current_page, ranked[0][1])

    # Rewrite rustdoc link syntax to point at generated Markdown destinations.
    def _resolve_documentation_links(
        self, documentation: str, item: Dict[str, Any], current_page: Path
    ) -> str:
        """Resolve shortcut and explicit Rust links between generated pages."""
        # Prefer rustdoc's authoritative label-to-ID mapping for shortcut links.
        for label, target in item.get("links", {}).items():
            target_identifier = item_identifier(target)
            if target_identifier not in self.rendered_identifiers:
                continue
            relative_target = self._relative_target(current_page, target_identifier)
            if relative_target is None:
                continue
            original = "[{}]".format(label)
            replacement = "[{}]({})".format(label, relative_target)
            documentation = documentation.replace(original, replacement)

        # Resolve one inline ``[label](Rust::Path)`` destination when unambiguous.
        def replace_inline_destination(match: re.Match) -> str:
            # Returning the original match preserves external and unknown links.
            relative_target = self._named_documentation_target(
                match.group("destination"), current_page
            )
            if relative_target is None:
                return match.group(0)
            return "{}{}{}".format(
                match.group("prefix"), relative_target, match.group("suffix")
            )

        # Limit replacement to the destination portion of inline Markdown links.
        documentation = re.sub(
            r"(?P<prefix>\]\()(?P<destination>[^\s)]+)(?P<suffix>\))",
            replace_inline_destination,
            documentation,
        )

        # Resolve one reference-style ``[label]: Rust::Path`` destination.
        def replace_reference_destination(match: re.Match) -> str:
            # Leave unresolved references byte-for-byte unchanged.
            relative_target = self._named_documentation_target(
                match.group("destination"), current_page
            )
            if relative_target is None:
                return match.group(0)
            return "{}{}".format(match.group("prefix"), relative_target)

        # Reference definitions are line-oriented, hence multiline matching.
        documentation = re.sub(
            r"(?m)^(?P<prefix>\s*\[[^\]]+\]:\s*)(?P<destination>\S+)\s*$",
            replace_reference_destination,
            documentation,
        )
        return documentation

    # Apply link repair and Markdown safety transforms to one item's prose.
    def _render_documentation(
        self, item: Dict[str, Any], heading_offset: int, current_page: Path
    ) -> str:
        # Always emit a visible placeholder so undocumented APIs are explicit.
        documentation = item.get("docs")
        if not documentation:
            return "_No documentation provided._"
        # Ordering matters: resolve Rust paths before escaping generic brackets,
        # then demote headings only after the prose is otherwise finalized.
        resolved = self._resolve_documentation_links(
            documentation, item, current_page
        )
        escaped = escape_rust_generic_brackets(resolved)
        return demote_markdown_headings(escaped, heading_offset)

    # Secondary item metadata is presented as a compact, single-line summary.
    def _render_metadata(self, item: Dict[str, Any]) -> str:
        """Render compact visibility, source, and deprecation information."""
        # Public visibility is the expected default and does not need a badge.
        values = []
        visibility = item.get("visibility")
        if visibility != "public":
            values.append("Visibility: {}".format(code_span(str(visibility))))
        # Source spans may omit a line even when a filename is available.
        span = item.get("span")
        if span:
            begin = span.get("begin", [None])
            line = begin[0] if begin else None
            location = span.get("filename", "")
            if line is not None:
                location = "{}:{}".format(location, line)
            values.append("Source: {}".format(code_span(location)))
        # The presence of metadata is enough to mark an item deprecated here;
        # detailed notes remain in the item's documentation when provided.
        if item.get("deprecation") is not None:
            values.append("**Deprecated**")
        return " · ".join(values)

    # Function items need several independent signature components assembled.
    def _render_function(self, item: Dict[str, Any]) -> str:
        # Pull generics once because declarations and where predicates use
        # separate renderers but share the same metadata object.
        data = item["inner"]["function"]
        generics = data.get("generics", {})
        return render_function_signature(
            item.get("name") or "_",
            data.get("sig", {}),
            visibility=render_visibility(item.get("visibility")),
            header=render_function_header(data.get("header", {})),
            generic_text=render_generic_parameters(generics),
            where_text=render_where_predicates(generics),
        )

    # Dispatch every supported rustdoc item kind to Rust-like declaration text.
    def _render_signature(self, item: Dict[str, Any]) -> str:
        """Render a concise Rust-like declaration for a primary or child item."""
        # Normalize common fields once before the kind-specific branches.
        kind = item_kind(item)
        data = item.get("inner", {}).get(kind, {})
        name = item.get("name") or "_"
        visibility = render_visibility(item.get("visibility"))

        # Primary declarations keep their essential qualifiers and generic
        # structure while eliding full bodies with semicolons or braces.
        if kind == "function":
            return self._render_function(item)
        if kind == "module":
            return "{}mod {};".format(visibility, name)
        if kind == "struct":
            generics = data.get("generics", {})
            return "{}struct {}{}{};".format(
                visibility,
                name,
                render_generic_parameters(generics),
                render_where_predicates(generics),
            )
        if kind == "enum":
            generics = data.get("generics", {})
            return "{}enum {}{}{} {{ ... }}".format(
                visibility,
                name,
                render_generic_parameters(generics),
                render_where_predicates(generics),
            )
        if kind == "union":
            generics = data.get("generics", {})
            return "{}union {}{}{} {{ ... }}".format(
                visibility,
                name,
                render_generic_parameters(generics),
                render_where_predicates(generics),
            )
        if kind == "trait":
            generics = data.get("generics", {})
            unsafe = "unsafe " if data.get("is_unsafe") else ""
            bounds = render_bounds(data.get("bounds", []))
            bounds_text = ": {}".format(bounds) if bounds else ""
            return "{}{}trait {}{}{}{} {{ ... }}".format(
                visibility,
                unsafe,
                name,
                render_generic_parameters(generics),
                bounds_text,
                render_where_predicates(generics),
            )
        if kind == "trait_alias":
            generics = data.get("generics", {})
            return "{}trait {}{} = {};".format(
                visibility,
                name,
                render_generic_parameters(generics),
                render_bounds(data.get("params", data.get("bounds", []))),
            )
        if kind == "type_alias":
            generics = data.get("generics", {})
            return "{}type {}{} = {}{};".format(
                visibility,
                name,
                render_generic_parameters(generics),
                render_type(data.get("type")),
                render_where_predicates(generics),
            )
        if kind == "constant":
            constant = data.get("const", {})
            value = constant.get("expr") or constant.get("value") or "_"
            return "{}const {}: {} = {};".format(
                visibility, name, render_type(data.get("type")), value
            )
        if kind == "static":
            mutable = "mut " if data.get("is_mutable") else ""
            expression = data.get("expr") or "_"
            return "{}static {}{}: {} = {};".format(
                visibility, mutable, name, render_type(data.get("type")), expression
            )
        # Associated items omit visibility because it is controlled by the
        # owning trait or implementation context.
        if kind == "assoc_const":
            value = data.get("value")
            suffix = " = {}".format(value) if value is not None else ""
            return "const {}: {}{};".format(name, render_type(data.get("type")), suffix)
        if kind == "assoc_type":
            generics = data.get("generics", {})
            bounds = render_bounds(data.get("bounds", []))
            bounds_text = ": {}".format(bounds) if bounds else ""
            assigned_type = data.get("type")
            assignment = (
                " = {}".format(render_type(assigned_type))
                if assigned_type is not None
                else ""
            )
            return "type {}{}{}{};".format(
                name,
                render_generic_parameters(generics),
                bounds_text,
                assignment,
            )
        # Child and macro variants have deliberately compact representations.
        if kind == "struct_field":
            return "{}: {}".format(name, render_type(data))
        if kind == "variant":
            return name
        if kind == "macro":
            return str(data).strip() or "macro_rules! {} {{ ... }}".format(name)
        if kind == "proc_macro":
            return "proc_macro {}".format(name)
        if kind == "extern_type":
            return "{}extern type {};".format(visibility, name)
        if kind == "primitive":
            return name
        # Unknown nightly variants retain at least their human-readable name.
        return name

    # Keep declaration fencing consistent across primary and associated items.
    def _render_code_block(self, signature: str) -> str:
        # Rust language tagging enables VitePress syntax highlighting.
        return "```rust\n{}\n```".format(signature)

    # Normalize schema-specific child storage into labelled rendering groups.
    def _child_identifiers(self, item: Dict[str, Any]) -> List[Tuple[str, List[str]]]:
        """Return labelled child groups for fields, variants, and trait members."""
        # Shared setup lets every aggregate kind append to the same group list.
        kind = item_kind(item)
        data = item.get("inner", {}).get(kind, {})
        groups: List[Tuple[str, List[str]]] = []

        # Struct fields differ for named, tuple, and unit structures.
        if kind == "struct":
            structure_kind = data.get("kind")
            fields: List[str] = []
            if isinstance(structure_kind, dict):
                if "plain" in structure_kind:
                    fields = [
                        item_identifier(value)
                        for value in structure_kind["plain"].get("fields", [])
                    ]
                elif "tuple" in structure_kind:
                    fields = [
                        item_identifier(value)
                        for value in structure_kind["tuple"]
                        if value is not None
                    ]
            if fields:
                groups.append(("Fields", fields))
        # Unions, enums, and traits expose direct identifier arrays.
        elif kind == "union":
            groups.append(
                ("Fields", [item_identifier(value) for value in data.get("fields", [])])
            )
        elif kind == "enum":
            groups.append(
                (
                    "Variants",
                    [item_identifier(value) for value in data.get("variants", [])],
                )
            )
        elif kind == "trait":
            groups.append(
                (
                    "Required and provided items",
                    [item_identifier(value) for value in data.get("items", [])],
                )
            )

        # Aggregate types also embed methods from non-trait implementations.
        if kind in {"struct", "enum", "union"}:
            inherent_items: List[str] = []
            for implementation_identifier in data.get("impls", []):
                implementation = self.get_item(implementation_identifier)
                if implementation is None:
                    continue
                implementation_data = implementation.get("inner", {}).get("impl", {})
                if implementation_data.get("trait") is None:
                    inherent_items.extend(
                        item_identifier(value)
                        for value in implementation_data.get("items", [])
                    )
            if inherent_items:
                groups.append(("Associated functions and methods", inherent_items))

        # Filter empty groups so pages never contain headings without content.
        return [(label, identifiers) for label, identifiers in groups if identifiers]

    # Render fields, variants, and associated items below their owning page.
    def _render_child_groups(self, item: Dict[str, Any], current_page: Path) -> str:
        # Track IDs across groups because rustdoc graphs can reference the same
        # associated item through more than one implementation container.
        sections: List[str] = []
        seen: Set[str] = set()
        for label, identifiers in self._child_identifiers(item):
            child_sections: List[str] = []
            # Each child gets an explicit anchor, signature, and prose block.
            for identifier in identifiers:
                if identifier in seen:
                    continue
                seen.add(identifier)
                child = self.get_item(identifier)
                if child is None:
                    continue
                name = child.get("name") or "Unnamed item"
                # A level-three item heading sits beneath its level-two group.
                child_sections.extend(
                    [
                        '<a id="{}"></a>'.format(markdown_anchor(identifier)),
                        "### {}".format(code_span(name)),
                        self._render_code_block(self._render_signature(child)),
                        self._render_documentation(
                            child, heading_offset=3, current_page=current_page
                        ),
                    ]
                )
            # Add the group heading only after at least one valid child renders.
            if child_sections:
                sections.append("## {}".format(label))
                sections.extend(child_sections)
        return "\n\n".join(sections)

    # Summarize explicit trait implementations without duplicating method bodies.
    def _render_trait_implementations(self, item: Dict[str, Any]) -> str:
        # Only aggregate types carry implementation lists in the handled schema.
        kind = item_kind(item)
        if kind not in {"struct", "enum", "union"}:
            return ""
        data = item["inner"][kind]
        # Deduplicate textual trait paths because rustdoc can expose repeated
        # implementations through generated or blanket metadata.
        implementations = []
        seen: Set[str] = set()
        for implementation_identifier in data.get("impls", []):
            implementation = self.get_item(implementation_identifier)
            if implementation is None:
                continue
            implementation_data = implementation.get("inner", {}).get("impl", {})
            trait = implementation_data.get("trait")
            # Inherent impls are rendered with child methods; synthetic impls
            # would produce noisy blanket entries and are intentionally skipped.
            if trait is None or implementation_data.get("is_synthetic"):
                continue
            trait_text = "{}{}".format(
                trait.get("path", "_"),
                render_generic_arguments(trait.get("args")),
            )
            if trait_text in seen:
                continue
            seen.add(trait_text)
            # Negative implementations retain their leading ``!`` marker.
            prefix = "!" if implementation_data.get("is_negative") else ""
            implementations.append("- {}".format(code_span(prefix + trait_text)))
        if not implementations:
            return ""
        return "## Trait implementations\n\n{}".format("\n".join(implementations))

    # Assemble one complete standalone page for a named non-module API item.
    def _render_item_page(self, identifier: str) -> str:
        """Render one non-module API item on its own rustdoc-style page."""
        # A missing item should not abort unrelated pages in the same crate.
        item = self.get_item(identifier)
        if item is None:
            return ""
        current_page = self.page_paths[identifier]
        path = self.paths.get(identifier, item.get("name") or identifier)
        label = ITEM_KIND_LABELS.get(item_kind(item), "Item")
        # Generated markers discourage manual edits that would be overwritten.
        sections = [
            "<!-- This file is generated. Do not edit it directly. -->",
            '<a id="{}"></a>'.format(markdown_anchor(identifier)),
            "# {} {}".format(label, code_span(path)),
            self._render_code_block(self._render_signature(item)),
        ]
        # Optional sections are appended only when they contain useful output.
        metadata = self._render_metadata(item)
        if metadata:
            sections.append(metadata)
        sections.append(
            self._render_documentation(
                item, heading_offset=1, current_page=current_page
            )
        )

        # Embedded children and trait summaries follow the primary prose.
        child_groups = self._render_child_groups(item, current_page)
        if child_groups:
            sections.append(child_groups)
        implementations = self._render_trait_implementations(item)
        if implementations:
            sections.append(implementations)
        return "\n\n".join(sections).rstrip() + "\n"

    # Build rustdoc-like categorized tables for one module's direct children.
    def _render_module_contents(self, identifier: str, current_page: Path) -> str:
        """Render links to a module's direct child modules and API items."""
        # Initialize in display order, while setdefault tolerates future kinds.
        grouped: Dict[str, List[str]] = {kind: [] for kind in ITEM_KIND_ORDER}
        for child_identifier in self.module_children.get(identifier, []):
            child = self.get_item(child_identifier)
            if child is not None:
                grouped.setdefault(item_kind(child), []).append(child_identifier)

        # Render only known categories to keep navigation order deterministic.
        contents: List[str] = []
        for kind in ITEM_KIND_ORDER:
            child_identifiers = grouped.get(kind, [])
            if not child_identifiers:
                continue
            contents.append("### {}".format(ITEM_KIND_TITLES[kind]))
            # Canonical Rust paths provide stable alphabetical ordering.
            for child_identifier in sorted(
                child_identifiers,
                key=lambda value: self.paths.get(value, value).lower(),
            ):
                child = self.get_item(child_identifier) or {}
                name = child.get("name") or child_identifier
                relative_target = self._relative_target(
                    current_page, child_identifier
                )
                if relative_target is None:
                    continue
                # A first-line summary gives context without expanding full docs.
                summary = first_documentation_line(child.get("docs"))
                suffix = " — {}".format(summary) if summary else ""
                contents.append(
                    "- [{}]({}){}".format(
                        code_span(name), relative_target, suffix
                    )
                )
            contents.append("")
        return "\n".join(contents).rstrip()

    # Re-export declarations remain visible even when their targets are external.
    def _render_reexports(self, identifier: str, current_page: Path) -> str:
        """Render public and private ``use`` entries listed by rustdoc."""
        # Gather entries first so an empty module does not receive a heading.
        entries: List[str] = []
        for use_identifier in self.module_uses.get(identifier, []):
            item = self.get_item(use_identifier)
            if item is None:
                continue
            use_data = item.get("inner", {}).get("use", {})
            source = use_data.get("source") or use_data.get("name") or "_"
            visibility = render_visibility(item.get("visibility"))
            signature = "{}use {};".format(visibility, source)
            # Link crate-local targets; external or unresolved uses remain code.
            target = use_data.get("id")
            relative_target = (
                self._relative_target(current_page, target)
                if target is not None
                else None
            )
            if relative_target:
                entries.append(
                    "- [{}]({})".format(code_span(signature), relative_target)
                )
            else:
                entries.append("- {}".format(code_span(signature)))
        if not entries:
            return ""
        return "## Re-exports\n\n{}".format("\n".join(entries))

    # Crate roots and nested modules share a page structure with minor metadata.
    def _render_module_page(self, identifier: str) -> str:
        """Render a crate root or nested module as an ``index.md`` page."""
        # Skip stale graph identifiers without affecting other output pages.
        item = self.get_item(identifier)
        if item is None:
            return ""
        current_page = self.page_paths[identifier]
        path = self.paths.get(identifier, item.get("name") or identifier)
        # Crate roots use a descriptive API title; modules retain their full path.
        is_crate = identifier == self.root_identifier
        title = (
            "{} API reference".format(path)
            if is_crate
            else "Module {}".format(code_span(path))
        )
        sections = [
            "<!-- This file is generated. Do not edit it directly. -->",
            '<a id="{}"></a>'.format(markdown_anchor(identifier)),
            "# {}".format(title),
        ]
        # Root metadata describes the entire JSON input, while nested modules
        # use the same source/visibility metadata as ordinary items.
        if is_crate:
            version = self.document.get("crate_version") or "unspecified"
            format_version = self.document.get("format_version", "unknown")
            sections.append(
                "> Crate version: {} · Rustdoc JSON format: {} · Private items: {}".format(
                    code_span(str(version)),
                    code_span(str(format_version)),
                    "included"
                    if self.document.get("includes_private")
                    else "excluded",
                )
            )
        else:
            metadata = self._render_metadata(item)
            if metadata:
                sections.append(metadata)

        # Prose precedes generated contents, mirroring rustdoc's reading order.
        sections.append(
            self._render_documentation(
                item, heading_offset=1, current_page=current_page
            )
        )
        # Contents and re-exports are optional for empty modules.
        contents = self._render_module_contents(identifier, current_page)
        if contents:
            sections.append("## Module contents\n\n{}".format(contents))
        reexports = self._render_reexports(identifier, current_page)
        if reexports:
            sections.append(reexports)
        return "\n\n".join(sections).rstrip() + "\n"

    # Materialize the complete page mapping only after discovery has finished.
    def render_pages(self) -> Dict[Path, str]:
        """Render every crate, module, and named item page for this crate."""
        # Seed the mapping with the crate root, which is not part of the primary
        # child list populated during recursive discovery.
        pages: Dict[Path, str] = {
            self.page_paths[self.root_identifier]: self._render_module_page(
                self.root_identifier
            )
        }
        # Each primary item becomes either another module index or a standalone
        # kind-prefixed Markdown page.
        for identifier in self.primary_identifiers:
            item = self.get_item(identifier)
            if item is None:
                continue
            page_path = self.page_paths[identifier]
            # Collisions indicate unsafe filename normalization and must fail
            # before the output directory is purged.
            if page_path in pages:
                raise ValueError(
                    "multiple rustdoc items map to {}".format(page_path)
                )
            if item_kind(item) == "module":
                pages[page_path] = self._render_module_page(identifier)
            else:
                pages[page_path] = self._render_item_page(identifier)
        return pages

    # Translate source filenames into clean public routes consumed by VitePress.
    def _sidebar_route(self, identifier: str) -> str:
        """Convert an output Markdown path to its public VitePress route."""
        # Directory indexes keep their trailing slash; item pages drop ``.md``.
        page = self.page_paths[identifier].as_posix()
        if page.endswith("/index.md"):
            relative_route = page[: -len("index.md")]
        else:
            relative_route = page[: -len(".md")]
        # Join against the configured public reference prefix exactly once.
        return "{}/{}".format(CODE_REFERENCE_ROUTE.rstrip("/"), relative_route)

    # Sidebar entries mirror module contents but nest submodules recursively.
    def _sidebar_groups(self, module_identifier: str) -> List[Dict[str, Any]]:
        """Build collapsible kind groups for one module's direct children."""
        # Categorize direct children using the same order as module pages.
        grouped: Dict[str, List[str]] = {kind: [] for kind in ITEM_KIND_ORDER}
        for child_identifier in self.module_children.get(module_identifier, []):
            child = self.get_item(child_identifier)
            if child is not None:
                grouped.setdefault(item_kind(child), []).append(child_identifier)

        # Each non-empty kind becomes an independently collapsible group.
        groups: List[Dict[str, Any]] = []
        for kind in ITEM_KIND_ORDER:
            child_identifiers = grouped.get(kind, [])
            if not child_identifiers:
                continue
            # Alphabetical labels keep large generated sidebars predictable.
            children = []
            for child_identifier in sorted(
                child_identifiers,
                key=lambda value: (self.get_item(value) or {}).get("name", "").lower(),
            ):
                child = self.get_item(child_identifier)
                if child is None:
                    continue
                # Modules recurse; leaf API items need only label and route.
                if kind == "module":
                    children.append(self._sidebar_module_item(child_identifier))
                else:
                    children.append(
                        {
                            "text": child.get("name") or "Unnamed item",
                            "link": self._sidebar_route(child_identifier),
                        }
                    )
            if children:
                groups.append(
                    {
                        "text": ITEM_KIND_TITLES[kind],
                        "collapsed": True,
                        "items": children,
                    }
                )
        return groups

    # Recursively package a module and its categorized descendants for VitePress.
    def _sidebar_module_item(self, identifier: str) -> Dict[str, Any]:
        """Build one recursively nested module entry for the sidebar."""
        # Always provide a clickable module landing page.
        item = self.get_item(identifier) or {}
        result: Dict[str, Any] = {
            "text": item.get("name") or "Unnamed module",
            "link": self._sidebar_route(identifier),
        }
        # Add foldable children only when the module is non-empty.
        groups = self._sidebar_groups(identifier)
        if groups:
            result["collapsed"] = True
            result["items"] = groups
        return result

    # Expose one top-level crate node to the generated TypeScript sidebar module.
    def render_sidebar_item(self) -> Dict[str, Any]:
        """Render this crate and all of its modules as a foldable sidebar tree."""
        # Fall back to the JSON filename if root metadata lacks a crate name.
        root = self.get_item(self.root_identifier) or {}
        crate_name = root.get("name") or self.source_file.stem
        result: Dict[str, Any] = {
            "text": display_name(crate_name),
            "link": self._sidebar_route(self.root_identifier),
        }
        # Crates with content start collapsed to keep the global menu manageable.
        groups = self._sidebar_groups(self.root_identifier)
        if groups:
            result["collapsed"] = True
            result["items"] = groups
        return result


# =============================================================================
# JSON generation, input validation, output preparation, and sequential writes
# =============================================================================


# Launch the sibling generator while isolating every Cargo artifact from the repo.
def generate_rustdoc_json(temporary_target_directory: Path) -> Path:
    """Run the JSON generator with all Cargo output isolated in a temp folder.

    Args:
        temporary_target_directory: Temporary replacement for the workspace's
            normal ``target`` directory. Cargo writes rustdoc JSON beneath its
            ``doc`` child directory.

    Returns:
        The temporary directory containing the generated ``*.json`` files.

    Raises:
        FileNotFoundError: The sibling JSON generator script does not exist.
        subprocess.CalledProcessError: JSON generation exits unsuccessfully.

    The generator itself does not need a special output argument. Cargo honors
    ``CARGO_TARGET_DIR`` for every command launched by that subprocess, which
    keeps both compilation artifacts and final rustdoc JSON out of the
    workspace's persistent ``target`` directory.
    """
    # Fail before spawning Python when the expected sibling script was moved or
    # omitted, producing a direct and actionable path in the error message.
    if not DOCUMENTATION_GENERATOR_SCRIPT.is_file():
        raise FileNotFoundError(
            "rustdoc JSON generator does not exist: {}".format(
                DOCUMENTATION_GENERATOR_SCRIPT
            )
        )

    # Copy the environment rather than mutating os.environ. The override then
    # applies only to the child generator and its Cargo processes, never to the
    # current shell or to commands a developer runs after this script exits.
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(temporary_target_directory.resolve())

    # Reuse this process's interpreter so virtual environments and Python
    # version selection remain consistent across both pipeline stages.
    # The shared generator defaults to classic HTML, so ``--json`` is required
    # here to produce the structured input consumed by this converter.
    command = [
        sys.executable,
        str(DOCUMENTATION_GENERATOR_SCRIPT),
        "--json",
    ]
    print(
        "Generating rustdoc JSON in temporary directory {}...".format(
            temporary_target_directory
        ),
        flush=True,
    )

    # Use the same Python executable that launched this converter. Passing an
    # argument sequence without a shell keeps Windows paths safe without manual
    # quoting, while check=True stops conversion if any Cargo job fails.
    subprocess.run(
        command,
        cwd=WORKSPACE_ROOT,
        env=environment,
        check=True,
    )

    # Cargo/rustdoc place JSON files in the target directory's ``doc`` child.
    return temporary_target_directory / "doc"


# Enumerate only top-level crate documents produced by the generator stage.
def discover_input_files(input_directory: Path) -> List[Path]:
    """Return all generated crate JSON files without recursing into subfolders."""
    # A missing directory means generation did not produce its promised layout.
    if not input_directory.is_dir():
        raise FileNotFoundError(
            "rustdoc directory does not exist: {}".format(input_directory)
        )
    # Stable filename ordering makes validation logs and final output repeatable.
    files = sorted(
        input_directory.glob("*.json"), key=lambda path: path.name.lower()
    )
    # Treat an empty successful generation as an error rather than wiping the
    # currently published reference with an empty replacement.
    if not files:
        raise FileNotFoundError(
            "no rustdoc JSON files found under {}".format(input_directory)
        )
    return files


# Parse, validate, and render a single crate before any output is deleted.
def load_and_render(
    input_file: Path,
) -> Tuple[str, str, Dict[Path, str], Dict[str, Any]]:
    """Parse one crate JSON and return its pages plus nested sidebar entry."""
    # Rustdoc emits UTF-8 JSON; parsing errors propagate to the coordinated
    # error handler so the existing website reference remains untouched.
    with input_file.open("r", encoding="utf-8") as stream:
        document = json.load(stream)
    # Validate the minimum graph shape used by the renderer before constructing
    # it, yielding a clearer message than a deep dictionary lookup failure.
    required_keys = {"root", "index", "format_version"}
    missing_keys = required_keys.difference(document)
    if missing_keys:
        raise ValueError(
            "{} is missing rustdoc keys: {}".format(
                input_file, ", ".join(sorted(missing_keys))
            )
        )
    # Renderer construction performs discovery and catches graph-level issues.
    renderer = CrateMarkdownRenderer(document, input_file)
    root = renderer.get_item(renderer.root_identifier)
    if root is None:
        raise ValueError("{} has no root item".format(input_file))
    # The filename is a stable fallback for unusual JSON without a root name.
    crate_name = root.get("name") or input_file.stem
    summary = first_documentation_line(root.get("docs"))
    return (
        crate_name,
        summary,
        renderer.render_pages(),
        renderer.render_sidebar_item(),
    )


# Create the landing page that links the independently generated crate trees.
def render_reference_index(crate_summaries: Sequence[Tuple[str, str]]) -> str:
    """Render the top-level index linking all generated crate indexes."""
    # Mark generated ownership and establish a small human-readable introduction.
    lines = [
        "<!-- This file is generated. Do not edit it directly. -->",
        "",
        "# Rust API reference",
        "",
        "Generated from nightly rustdoc JSON for the Pill Engine workspace.",
        "",
        "## Crates",
        "",
    ]
    # Alphabetical crate ordering keeps both navigation and diffs deterministic.
    for crate_name, summary in sorted(
        crate_summaries, key=lambda value: value[0].lower()
    ):
        # Link source Markdown directly; VitePress resolves it to the crate route.
        target = "{}/index.md".format(safe_path_component(crate_name))
        suffix = " — {}".format(summary) if summary else ""
        lines.append("- [{}]({}){}".format(code_span(crate_name), target, suffix))
    return "\n".join(lines).rstrip() + "\n"


# Serialize Python sidebar objects into the TypeScript module imported by config.
def render_sidebar_data(sidebar_items: Sequence[Dict[str, Any]]) -> str:
    """Serialize the generated tree as an importable typed TypeScript module."""
    # Preserve Unicode crate/item names and use indentation for reviewable diffs.
    serialized = json.dumps(sidebar_items, ensure_ascii=False, indent=2)
    # Include VitePress's declared type so config changes receive type checking.
    return (
        "// This file is generated. Do not edit it directly.\n"
        "import type {{ DefaultTheme }} from 'vitepress'\n\n"
        "export const codeReferenceSidebarItems: DefaultTheme.SidebarItem[] = "
        "{}\n".format(serialized)
    )


# Remove stale reference pages only after all temporary JSON has rendered safely.
def purge_output_directory() -> None:
    """Delete every existing entry from the exact default output directory."""
    # Resolve both paths before comparison so alternate path spellings cannot
    # bypass the destructive-operation guard, which repeats the expected
    # location instead of trusting the constant it protects.
    expected = (REPOSITORY_ROOT / "local" / "docs_build" / "reference").resolve()
    resolved = OUTPUT_DIRECTORY.resolve()

    # Guard the recursive operation even though the value is fixed. Any future
    # edit that points elsewhere must fail closed instead of deleting a broader
    # directory such as ``local`` or ``docs_build`` itself.
    if resolved != expected or resolved.name != "reference":
        raise RuntimeError("refusing to purge unexpected path: {}".format(resolved))

    # Keep the reference root itself and remove only its generated contents.
    resolved.mkdir(parents=True, exist_ok=True)
    for entry in resolved.iterdir():
        # Test symlinks before directories so a directory link is unlinked rather
        # than recursively traversed outside the guarded output root.
        if entry.is_symlink() or entry.is_file():
            entry.unlink()
        elif entry.is_dir():
            shutil.rmtree(str(entry))
        else:
            raise RuntimeError("unsupported output entry: {}".format(entry))


# Write one page beneath the guarded reference root using atomic replacement.
def write_output_file(relative_path: Path, markdown: str) -> Path:
    """Write one UTF-8 Markdown file atomically into the reference directory."""
    # Reject absolute and parent-traversing paths before combining components.
    if relative_path.is_absolute() or ".." in relative_path.parts:
        raise RuntimeError("refusing to write unsafe path: {}".format(relative_path))
    # Resolve and compare the final path as a second defense against escaping
    # the output root through future path-generation changes.
    destination = (OUTPUT_DIRECTORY / relative_path).resolve()
    output_root = OUTPUT_DIRECTORY.resolve()
    if os.path.commonpath([str(destination), str(output_root)]) != str(output_root):
        raise RuntimeError("output escaped reference directory: {}".format(destination))
    # Create nested crate/module folders only after path validation succeeds.
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Write completely to a sibling temporary file before replacing the target,
    # preventing readers from observing a partially written Markdown document.
    temporary = destination.with_suffix(destination.suffix + ".tmp")
    with temporary.open("w", encoding="utf-8", newline="\n") as stream:
        stream.write(markdown)
    temporary.replace(destination)
    return destination


# Replace the single generated VitePress sidebar module with strict path checks.
def write_sidebar_data(sidebar_data: str) -> Path:
    """Atomically replace the exact generated sidebar module in its default path."""
    # The sidebar lives inside the purged reference tree, so this call must stay
    # after write_output_file's page loop in main. Guard its exact path here too
    # rather than relying on write_output_file's root check.
    expected = (
        REPOSITORY_ROOT
        / "local"
        / "docs_build"
        / "reference"
        / "reference-sidebar.ts"
    ).resolve()
    destination = SIDEBAR_DATA_FILE.resolve()
    if destination != expected or destination.name != "reference-sidebar.ts":
        raise RuntimeError(
            "refusing to write unexpected sidebar path: {}".format(destination)
        )
    # Use the same atomic sibling-file strategy as generated Markdown pages.
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + ".tmp")
    with temporary.open("w", encoding="utf-8", newline="\n") as stream:
        stream.write(sidebar_data)
    temporary.replace(destination)
    return destination


# Coordinate generation, validation, destructive replacement, and error reporting.
def main() -> int:
    """Generate JSON temporarily, validate it, then replace Markdown output."""
    # Keep expected operational failures concise instead of showing tracebacks to
    # developers or CI logs during a routine documentation run.
    try:
        # TemporaryDirectory owns the complete throwaway Cargo target tree. It
        # recursively cleans that tree on normal completion and while unwinding
        # any exception raised during generation, validation, or file writing.
        with tempfile.TemporaryDirectory(prefix="pill-rustdoc-") as temp_path:
            temporary_target_directory = Path(temp_path) / "target"
            input_directory = generate_rustdoc_json(
                temporary_target_directory
            )
            input_files = discover_input_files(input_directory)

            # Render everything before the destructive purge. A malformed or
            # unsupported JSON input therefore leaves the current reference
            # pages untouched rather than replacing them with an empty or
            # partially generated directory.
            rendered_pages: Dict[Path, str] = {}
            crate_summaries: List[Tuple[str, str]] = []
            sidebar_items: List[Dict[str, Any]] = []
            # Build all pages and sidebar nodes in memory before the first delete.
            for input_file in input_files:
                print("Validating {}...".format(input_file.name), flush=True)
                crate_name, summary, crate_pages, sidebar_item = load_and_render(
                    input_file
                )
                crate_summaries.append((crate_name, summary))
                sidebar_items.append(sidebar_item)
                # Cross-crate path collisions indicate an unsafe output mapping.
                for relative_path, markdown in crate_pages.items():
                    if relative_path in rendered_pages:
                        raise ValueError(
                            "multiple rustdoc items map to {}".format(relative_path)
                        )
                    rendered_pages[relative_path] = markdown

            # Add the shared landing page only after every crate summary exists.
            rendered_pages[Path("index.md")] = render_reference_index(
                crate_summaries
            )

            print("Purging {}...".format(OUTPUT_DIRECTORY), flush=True)
            purge_output_directory()

            # Deterministic write order keeps progress logs stable across runs.
            ordered_pages = sorted(
                rendered_pages.items(),
                key=lambda value: value[0].as_posix().lower(),
            )
            for index, (relative_path, markdown) in enumerate(
                ordered_pages, start=1
            ):
                destination = write_output_file(relative_path, markdown)
                print(
                    "[{}/{}] Wrote {}".format(
                        index, len(ordered_pages), destination
                    ),
                    flush=True,
                )

            # Match the root index's alphabetical crate order in navigation.
            sidebar_items.sort(
                key=lambda item: str(item.get("text", "")).lower()
            )
            sidebar_destination = write_sidebar_data(
                render_sidebar_data(sidebar_items)
            )
            print("Wrote {}".format(sidebar_destination), flush=True)
    except subprocess.CalledProcessError as error:
        # The child generator already prints Cargo's detailed diagnostic. Keep
        # this summary short and preserve the failing exit status for CI.
        print(
            "Rustdoc JSON generation failed with exit code {}.".format(
                error.returncode
            ),
            file=sys.stderr,
        )
        return error.returncode or 1
    # Conversion, path-safety, parsing, and filesystem failures use one readable
    # error channel and leave a non-zero status for shell scripts and CI.
    except (
        OSError,
        RuntimeError,
        ValueError,
        KeyError,
        TypeError,
        json.JSONDecodeError,
    ) as error:
        print("Conversion failed: {}".format(error), file=sys.stderr)
        return 1

    # At this point TemporaryDirectory has also removed the transient Cargo tree.
    print(
        "Converted {} rustdoc JSON file(s) into {} Markdown page(s).".format(
            len(input_files), len(rendered_pages)
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
