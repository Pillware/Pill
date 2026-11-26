"""The editor's and the UI's tooltips all open through the kit, so all open above.

PROTECTS: Every tooltip in the editor and the shared UI crates is asked for through `pill_ui_kit::tooltip`, which aims it above the widget; egui's own `Response::on_hover_text` and its siblings aim it below and cannot be reached from either tree any more.
TIER: static

egui's tooltip methods are inherent on `Response`, so no setting on the context
can redirect them - a single `response.on_hover_text(..)` would open below while
the rest of the UI opens above. This suite is the guard that keeps the adoption
complete: it reads the editor's sources and the UI crates' sources and refuses
the inherent methods, naming the line and the replacement.

The kit's own `tooltip.rs` is the one place allowed to build egui's tooltip, and
the editor tree must still call the kit - a scan that found no call at all would
otherwise pass while the feature was quietly gone.
"""

import re
import sys
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parent.parent.parent
SCAN_ROOTS = (
    REPOSITORY_ROOT / "modules" / "editor",
    REPOSITORY_ROOT / "modules" / "ui",
)
# The kit's own module builds egui's tooltip; that is the point of it.
ALLOWED_PATH = REPOSITORY_ROOT / "modules" / "ui" / "pill_ui_kit" / "src" / "tooltip.rs"
# egui's tooltip methods, which all place the tooltip below the widget.
FORBIDDEN = re.compile(r"\.(on_hover_text|on_hover_ui|on_disabled_hover_text|on_disabled_hover_ui)\s*\(")
# What a call site has to say instead.
REPLACEMENT = "kit::tooltip::on_hover_text(response, text)"
# The editor's own adoption, which must still be there.
KIT_CALL = "kit::tooltip::on_hover_text("
KIT_CALL_MINIMUM = 30


class TooltipError(AssertionError):
    """A tooltip is asked for through egui's own method."""


def rust_sources(root):
    """Every `.rs` file under `root`."""
    return sorted(path for path in root.rglob("*.rs") if "__pycache__" not in path.parts)


def check_none_bypass_the_kit():
    """No source under the scan roots calls egui's tooltip methods."""
    offenders = []
    for root in SCAN_ROOTS:
        if not root.is_dir():
            raise TooltipError("{} is not a folder; the scan roots moved".format(root))
        for path in rust_sources(root):
            if path.resolve() == ALLOWED_PATH:
                continue
            for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
                if FORBIDDEN.search(line):
                    offenders.append(
                        "{}:{}: {} - use {} instead".format(
                            path.relative_to(REPOSITORY_ROOT).as_posix(),
                            number,
                            line.strip(),
                            REPLACEMENT,
                        )
                    )
    if offenders:
        raise TooltipError(
            "egui's tooltip methods open below the widget:\n  " + "\n  ".join(offenders)
        )


def check_the_editor_still_uses_the_kit():
    """The editor's tooltips really go through the kit."""
    calls = sum(
        path.read_text(encoding="utf-8").count(KIT_CALL)
        for path in rust_sources(REPOSITORY_ROOT / "modules" / "editor")
    )
    if calls < KIT_CALL_MINIMUM:
        raise TooltipError(
            "the editor tree holds {} {} call(s), and it had {} when the kit's tooltip "
            "was adopted; the scan is looking at the wrong tree".format(
                calls, KIT_CALL, KIT_CALL_MINIMUM
            )
        )


def main():
    try:
        check_none_bypass_the_kit()
        check_the_editor_still_uses_the_kit()
    except TooltipError as error:
        print("  FAIL  {}".format(error))
        return 1
    print("  PASS  every editor and UI tooltip opens through the kit")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
