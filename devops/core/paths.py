"""
Repository layout resolution for the devops tooling.

REQUIREMENTS: Python 3.8+.

DESCRIPTION
    Every path the devops scripts touch is derived from this module's own
    location, so any of them can be launched from any working directory.
    Windows paths are handled by `pathlib` throughout - no shell-specific path
    juggling.

    Directory roles, all under `devops/`:

      tests/       standalone pass/fail suites plus their test projects
      core/        this package: shared code every script imports
      ci_cd/       the CI gate and the shipping builds
      tools/       builders, probes and one-off repository tools
      setup/       environment setup scripts

--- SCRIPT ---
"""

import shutil
import sys
from pathlib import Path

# devops/core/paths.py -> devops
DEVOPS_ROOT = Path(__file__).resolve().parent.parent
# devops -> repository root
REPOSITORY_ROOT = DEVOPS_ROOT.parent

# The devops directories a script reaches by constant, so none has to
# hardcode a relative hop. `setup/` and `tools/` have no constant on purpose.
TESTS_ROOT = DEVOPS_ROOT / "tests"
CORE_ROOT = DEVOPS_ROOT / "core"
CI_CD_ROOT = DEVOPS_ROOT / "ci_cd"

def ensure_devops_on_path() -> None:
    """Puts `devops/` on `sys.path` so `import core` resolves.

    Every standalone script under `tests/` or `tools/` must run from a
    console, which means it cannot rely on a parent package being imported
    first. Each one calls this before importing anything from `core`.
    """
    devops_path = str(DEVOPS_ROOT)
    if devops_path not in sys.path:
        sys.path.insert(0, devops_path)

# The Cargo workspace lives in `modules/`, not at the repository root.
MODULES_ROOT = REPOSITORY_ROOT / "modules"
CARGO_TARGET_ROOT = MODULES_ROOT / "target"

def executable_name(stem: str) -> Path:
    """Returns the debug-profile binary path for a crate, per platform.

    Windows appends `.exe`; every other platform uses the bare stem. This is
    the one spelling of a built binary: a caller that hardcodes `.exe` breaks
    `--skip-build` everywhere else.
    """
    import platform

    suffix = ".exe" if platform.system() == "Windows" else ""
    return CARGO_TARGET_ROOT / "debug" / f"{stem}{suffix}"


def find_executable(name: str) -> str:
    """Resolves a tool on PATH, returning the bare name when lookup fails.

    `shutil.which` is used so a missing tool is reported by the subprocess
    call itself rather than silently resolving to something unexpected.
    """
    return shutil.which(name) or name
