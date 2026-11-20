"""The editor suite's area table: one entry per runnable step area.

Every `editor_steps_*.py` module beside this one declares one `AREA`, and the
suite finds them by convention and runs them in `order`. An area may run without
the areas before it only when every name it needs is provided inside the same
selection: the table is what lets `--steps` refuse an unsound subset with a
message naming the missing area, instead of failing inside a step body.
"""

import importlib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, List, Sequence, Tuple


@dataclass
class StepContext:
    """What an area runs against: the shared state and the run's session.

    The suite starts one editor session for every run, seeded with the fixtures
    the areas share; an area reads `state` exactly as it did inside the suite.
    """

    state: object
    session: object
    binary: object
    project: str


@dataclass(frozen=True)
class StepArea:
    """One runnable area: what it runs, what it reads and what it provides.

    `needs` names the `StepState` values an earlier area has to have written;
    `provides` names the ones this area writes for later areas. `sequence_only`
    is the guarantee a subset run cannot cover (the text is printed when the
    area refuses to run alone), and is empty for an area that stands alone.
    """

    name: str
    order: int
    run: Callable[[StepContext], None]
    needs: Tuple[str, ...] = ()
    provides: Tuple[str, ...] = ()
    sequence_only: str = ""


def area_modules() -> List[Path]:
    """Every `editor_steps_*.py` module beside this file, in name order."""
    return sorted(Path(__file__).parent.glob("editor_steps_*.py"))


def discover_areas() -> List[StepArea]:
    """Every area module's entries, in run order, with their needs checked.

    A module declares one `AREA` or a tuple of them (`AREAS`); the lifecycle's
    five step areas share one module because they already share their sessions.
    """
    areas: List[StepArea] = []
    for path in area_modules():
        module = importlib.import_module(path.stem)
        declared = getattr(module, "AREAS", None)
        if declared is None:
            single = getattr(module, "AREA", None)
            if single is None:
                raise RuntimeError(f"{path.name} declares no AREA")
            declared = (single,)
        areas.extend(declared)
    areas.sort(key=lambda area: area.order)
    names = [area.name for area in areas]
    if len(names) != len(set(names)):
        raise RuntimeError(f"two areas share a name: {names}")
    if len({area.order for area in areas}) != len(areas):
        raise RuntimeError("two areas share an order: " + ", ".join(names))
    provided = set()
    for area in areas:
        missing = set(area.needs) - provided
        if missing:
            raise RuntimeError(
                "{} needs {} before it, and no earlier area provides it".format(
                    area.name, ", ".join(sorted(missing))
                )
            )
        provided |= set(area.provides)
    return areas


def select_areas(areas: Sequence[StepArea], spec: str) -> List[StepArea]:
    """The areas a run covers, or `SystemExit` naming what the spec cannot run.

    `all` selects every area. A subset runs in the table's order. An area whose
    `needs` are not met pulls in the areas that provide them - a subset is
    about skipping areas, not about reproducing the suite's state by hand - and
    the additions are reported by `print_subset_banner`. An area marked
    `sequence_only` refuses the subset outright, because the guarantee it
    protects only exists across a full run.
    """
    if spec == "all":
        return list(areas)
    names = [part.strip() for part in spec.split(",") if part.strip()]
    known = {area.name: area for area in areas}
    unknown = [name for name in names if name not in known]
    if unknown:
        raise SystemExit(
            "unknown step area(s): {}; --steps list prints every name".format(
                ", ".join(unknown)
            )
        )
    chosen = {known[name].name for name in names}
    # An area whose value a later chosen area reads is pulled in, and so are
    # the areas that provide what it reads: a subset is about skipping areas,
    # not about reproducing the suite's state by hand.
    changed = True
    while changed:
        changed = False
        for area in areas:
            if area.name in chosen:
                continue
            if any(
                other.name in chosen
                and other.order > area.order
                and set(other.needs) & set(area.provides)
                for other in areas
            ):
                chosen.add(area.name)
                changed = True
    provided: set = set()
    for area in areas:
        if area.name not in chosen:
            continue
        absent = set(area.needs) - provided
        if absent:
            raise SystemExit(
                "{} needs {}, which no earlier area provides".format(
                    area.name, ", ".join(sorted(absent))
                )
            )
        provided |= set(area.provides)
    selected = [area for area in areas if area.name in chosen]
    for area in selected:
        if area.sequence_only:
            raise SystemExit(
                "{} cannot run in a subset: it protects {}, which only a run "
                "from the start of the suite can cover.".format(
                    area.name, area.sequence_only
                )
            )
    return selected


def print_subset_banner(chosen: Sequence[StepArea], areas: Sequence[StepArea]) -> None:
    """States what a subset run does and does not cover, once, before it runs."""
    skipped = [area.name for area in areas if area.name not in {a.name for a in chosen}]
    print(
        "[SUBSET] running {} of {} areas: {}".format(
            len(chosen), len(areas), ", ".join(area.name for area in chosen)
        )
    )
    print("[SUBSET] not run: {}".format(", ".join(skipped) if skipped else "none"))
    print(
        "[SUBSET] a subset run is not the suite's verdict: the guarantees the "
        "skipped areas protect are not covered"
    )
