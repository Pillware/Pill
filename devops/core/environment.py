"""
Process queries and timestamps the devops scripts share.

REQUIREMENTS: Python 3.8+ (standard library only).

DESCRIPTION
    `windows_process_commands` answers "is this process running" for the host
    cleanup in `core/suite_common.py`; a failed query raises rather than
    reporting an empty answer, so a guard never disables itself silently.
    `filesystem_timestamp` names one suite run's private folder.

--- SCRIPT ---
"""

import shutil
import subprocess
from datetime import datetime
from typing import List


class ProcessToolUnavailable(RuntimeError):
    """The process query could not run, so its answer means nothing."""


def windows_process_commands(name: str) -> List[str]:
    """Every command line of a running process named `name` (Windows).

    A guard that swallows a failed query as "nothing is running" disables
    itself silently, so this raises [`ProcessToolUnavailable`] when the shell
    is missing, the query raises, or it exits non-zero.
    """
    shell = shutil.which("powershell") or shutil.which("pwsh")
    if not shell:
        raise ProcessToolUnavailable("neither powershell nor pwsh is on PATH")
    query = (
        f"Get-CimInstance Win32_Process -Filter \"Name='{name}'\""
        " | Select-Object -ExpandProperty CommandLine"
    )
    try:
        completed = subprocess.run(
            [shell, "-NoProfile", "-NonInteractive", "-Command", query],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise ProcessToolUnavailable(
            f"the process query could not be run: {error}"
        ) from error
    if completed.returncode != 0:
        raise ProcessToolUnavailable(
            f"the process query failed: {(completed.stderr or '').strip()[:200]}"
        )
    return [line.strip() for line in (completed.stdout or "").splitlines() if line.strip()]


def filesystem_timestamp() -> str:
    """Returns a filesystem-safe local timestamp: `YYYY-MM-DD_HH-MM-SS`."""
    return datetime.now().astimezone().strftime("%Y-%m-%d_%H-%M-%S")
