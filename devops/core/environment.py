"""
Process queries and timestamps the devops scripts share.

REQUIREMENTS: Python 3.8+ (standard library only).

DESCRIPTION
    `process_commands` answers "is this process running" for the host cleanup
    in `core/suite_common.py`; a failed query raises rather than reporting an
    empty answer, so a guard never disables itself silently.
    `filesystem_timestamp` names one suite run's private folder.

--- SCRIPT ---
"""

import os
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path
from typing import List


class ProcessToolUnavailable(RuntimeError):
    """The process query could not run, so its answer means nothing."""


def process_commands(name: str) -> List[str]:
    """Every command line of a running process whose executable is `name`.

    `name` carries no `.exe`; Windows adds it. On Linux each line also ends
    with the process's working directory (`  (cwd <path>)`), because a cargo
    started as `cargo build -p ...` names its workspace nowhere else.

    A guard that swallows a failed query as "nothing is running" disables
    itself silently, so this raises [`ProcessToolUnavailable`] when the query
    cannot run.
    """
    if sys.platform == "win32":
        return _windows_process_commands(f"{name}.exe")
    if Path("/proc/self/cmdline").is_file():
        return _proc_process_commands(name)
    return _ps_process_commands(name)


def _proc_process_commands(name: str) -> List[str]:
    """`process_commands` from `/proc` (Linux)."""
    commands = []
    try:
        entries = [entry for entry in os.listdir("/proc") if entry.isdigit()]
    except OSError as error:
        raise ProcessToolUnavailable(f"/proc could not be listed: {error}") from error
    for pid in entries:
        process = Path("/proc") / pid
        # A process can exit between the listing and these reads; it is simply
        # no longer running.
        try:
            arguments = (process / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if not arguments or not arguments[0]:
            continue
        if Path(arguments[0].decode("utf-8", "replace")).name != name:
            continue
        line = " ".join(
            argument.decode("utf-8", "replace") for argument in arguments if argument
        )
        try:
            line += "  (cwd {})".format(os.readlink(process / "cwd"))
        except OSError:
            pass
        commands.append(line)
    return commands


def _ps_process_commands(name: str) -> List[str]:
    """`process_commands` from `ps` (macOS and other Unix systems)."""
    try:
        completed = subprocess.run(
            ["ps", "-axww", "-o", "args="],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise ProcessToolUnavailable(f"`ps` could not be run: {error}") from error
    if completed.returncode != 0:
        raise ProcessToolUnavailable(
            f"`ps` failed: {(completed.stderr or '').strip()[:200]}"
        )
    return [
        line.strip()
        for line in (completed.stdout or "").splitlines()
        if line.strip() and Path(line.split()[0]).name == name
    ]


def _windows_process_commands(name: str) -> List[str]:
    """`process_commands` through PowerShell's `Win32_Process` query."""
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
