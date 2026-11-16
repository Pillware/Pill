#!/usr/bin/env python3
"""
Key code mirror check: the managed `KeyCode` enum against the engine's
`key_codes!` table, which the two halves only ever tied together with a comment.

PROTECTS: The C# KeyCode mirror names the same keys in the same order as the engine table.
TIER: static

REQUIREMENTS: Python 3.11+ (standard library only).

DESCRIPTION
    `pill_engine_core::input::KeyCode` is built by the `key_codes!` macro from
    one table, and its discriminant is what crosses the input ABI - the managed
    side receives a key as that number. `pill_csharp_runtime/src/KeyCode.cs`
    restates the table with explicit numbers, because C# cannot read the Rust
    macro. Until now the only coupling was a comment asking the two to be kept
    in step, so a key inserted mid-table on one side silently shifted every key
    after it: `KeyHeld(KeyCode.W)` would report another key, and nothing would
    say so.

    This check parses both tables and enforces three things:

      1. same keys   - the mirror declares exactly the engine's key names.
      2. same order  - the names appear in the table's declaration order, which
                        is what makes a discriminant mean the same key on both
                        sides.
      3. numbers     - each member's explicit number is its position, so a
                        hand-edited number is caught even when the names and
                        their order still agree.

    A fourth rule pins that the mirror is an enum over `byte`: the ABI carries
    a key in one byte, and a table that outgrew 256 keys would need the slot
    widened on both sides.

USAGE
  python devops/tests/test_keycode_mirror.py [--root <repository>]
  python devops/tests/test_keycode_mirror.py --self-test

EXAMPLE USAGE
  python devops/tests/test_keycode_mirror.py
  python devops/tests/test_keycode_mirror.py --self-test

  Exit status: 0 when every rule holds, 1 when at least one is broken, 2 on a
  usage error.

--- SCRIPT ---
"""

import argparse
import re
import sys
import tempfile
from pathlib import Path

# The engine's table and the managed mirror, relative to the repository root.
ENGINE_TABLE = "modules/engine/pill_engine_core/src/services/input.rs"
MIRROR_ENUM = "modules/csharp/pill_csharp_runtime/src/KeyCode.cs"

# `Name => "code-name"` inside the `key_codes!` block.
TABLE_ENTRY = re.compile(r'([A-Za-z0-9_]+)\s*=>\s*"')

# `Name = 12,` inside the enum body, and the body itself.
MIRROR_ENTRY = re.compile(r"^\s+([A-Za-z0-9_]+)\s*=\s*(\d+)\s*,", re.MULTILINE)
MIRROR_BODY = re.compile(r"public enum KeyCode\s*:\s*byte\s*\{(.*?)\n\}", re.DOTALL)

# One byte crosses the ABI, so the table has to fit in it.
KEY_LIMIT = 256


# The keys the engine declares, in declaration order.
def engine_keys(source):
    start = source.find("key_codes! {")
    if start < 0:
        raise ValueError(f"no `key_codes!` table in {ENGINE_TABLE}")
    end = source.find("\n}", start)
    if end < 0:
        raise ValueError(f"the `key_codes!` table in {ENGINE_TABLE} is not closed")
    return TABLE_ENTRY.findall(source[start:end])


# The mirror's members, as (name, number) pairs in declaration order.
def mirror_keys(source):
    body = MIRROR_BODY.search(source)
    if body is None:
        raise ValueError(f"no `public enum KeyCode : byte` in {MIRROR_ENUM}")
    return [(name, int(number)) for name, number in MIRROR_ENTRY.findall(body.group(1))]


# Every rule's findings for one pair of sources.
def check(engine_source, mirror_source):
    failures = []
    expected = engine_keys(engine_source)
    declared = mirror_keys(mirror_source)
    names = [name for name, _number in declared]

    # Rule 1: the same set of keys. Reported as the difference, so a rename
    # reads as a rename rather than as a shifted list.
    missing = [name for name in expected if name not in names]
    extra = [name for name in names if name not in expected]
    for name in missing:
        failures.append(f"the mirror has no `{name}`, which the engine table declares")
    for name in extra:
        failures.append(f"the mirror declares `{name}`, which the engine table does not")

    # Rule 2: the same order. Only meaningful once the sets agree; the first
    # place they diverge is the one worth naming.
    if not missing and not extra and names != expected:
        index = next(
            position
            for position, (declared_name, expected_name) in enumerate(zip(names, expected))
            if declared_name != expected_name
        )
        failures.append(
            f"the mirror is out of order at position {index}: it declares "
            f"`{names[index]}` where the engine table has `{expected[index]}`"
        )

    # Rule 3: each number is the member's position.
    for position, (name, number) in enumerate(declared):
        if number != position:
            failures.append(
                f"`{name}` is numbered {number} but sits at position {position}; "
                "the two sides would disagree about which key that number is"
            )

    # Rule 4: one byte carries a key across the ABI.
    if len(expected) > KEY_LIMIT:
        failures.append(
            f"the engine table declares {len(expected)} keys; a key crosses the ABI "
            f"in one byte, so the slot and the mirror both have to widen first"
        )
    return failures


# Reads the pair from `root` and applies every rule.
def check_root(root):
    engine = (root / ENGINE_TABLE).read_text(encoding="utf-8")
    mirror = (root / MIRROR_ENUM).read_text(encoding="utf-8")
    return check(engine, mirror)


# --- Self test ---------------------------------------------------------------

# A table and a mirror that agree, as the fixture's baseline. Assembled from
# parts so this file does not carry a second copy of the real table.
GOOD_ENGINE = 'key_codes! {\n    Alpha => "Alpha", Beta => "Beta", Gamma => "Gamma",\n}\n'
GOOD_MIRROR = "public enum KeyCode : byte\n{\n    Alpha = 0,\n    Beta = 1,\n    Gamma = 2,\n}\n"


def self_test():
    passed = True

    def expect(description, engine, mirror, expected_failures):
        nonlocal passed
        failures = check(engine, mirror)
        outcome = "PASS" if len(failures) == expected_failures else "FAIL"
        passed &= outcome == "PASS"
        print(f"  {outcome}  {description}")
        if outcome == "FAIL":
            for failure in failures:
                print(f"          {failure}")

    expect("an agreeing pair is accepted", GOOD_ENGINE, GOOD_MIRROR, 0)
    expect(
        "a key inserted mid-table is caught",
        'key_codes! {\n    Alpha => "Alpha", Delta => "Delta", Beta => "Beta", Gamma => "Gamma",\n}\n',
        GOOD_MIRROR,
        1,
    )
    expect(
        "a key missing from the mirror is caught",
        'key_codes! {\n    Alpha => "Alpha", Beta => "Beta", Gamma => "Gamma", Delta => "Delta",\n}\n',
        GOOD_MIRROR,
        1,
    )
    expect(
        "a key the engine does not declare is caught",
        GOOD_ENGINE,
        "public enum KeyCode : byte\n{\n    Alpha = 0,\n    Beta = 1,\n    Delta = 2,\n    Gamma = 3,\n}\n",
        1,
    )
    expect(
        "a swapped pair is caught as out of order",
        GOOD_ENGINE,
        "public enum KeyCode : byte\n{\n    Beta = 0,\n    Alpha = 1,\n    Gamma = 2,\n}\n",
        1,
    )
    expect(
        "a hand-edited number is caught",
        GOOD_ENGINE,
        "public enum KeyCode : byte\n{\n    Alpha = 0,\n    Beta = 7,\n    Gamma = 2,\n}\n",
        1,
    )
    return 0 if passed else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[1])
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--self-test", action="store_true", help="check the checker itself")
    arguments = parser.parse_args()
    if arguments.self_test:
        return self_test()
    try:
        failures = check_root(arguments.root.resolve())
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    for failure in failures:
        print(f"  FAIL  {failure}")
    if failures:
        return 1
    print(
        "  PASS  key code mirror: the managed KeyCode names the engine table's keys, "
        "in its order, numbered by position"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
