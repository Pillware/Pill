"""
The shared scenario runner for the host-driving suites.

REQUIREMENTS
  - Python 3.8+

DESCRIPTION
    One definition of the loop every hot-reload suite runs: edit source files,
    wait for the reload, assert what the host printed, put the files back.

    The three suites that drive the host grew their own copy of it, and the
    copies drifted. Two of them captured their wait index per phase and one per
    scenario; two waited proof-of-life tokens from an index taken after the
    reload token and one from its settle token; the crash sweep, the
    required/forbidden sweep and the restore step were each written three
    times, with the messages disagreeing. One copy makes those choices one
    decision instead of three.

    A suite supplies what is genuinely its own:

      * `Scenario` / `ScenarioPhase` - its table of edits, tokens and waits.
      * `ScenarioRunner`              - its scaled timeouts, and the token its
                                        restores settle with.
      * `validators`                  - checks over a scenario's output that
                                        only that suite knows how to make
                                        (migrated entity counts, witnesses).

USAGE
  from core.scenario import Scenario, ScenarioPhase, ScenarioRunner, run_scenario

  with `devops/` on `sys.path`; see `core.paths.ensure_devops_on_path`.

--- SCRIPT ---
"""

import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Optional, Sequence, Tuple

# `print` here is suite_common's console-color wrapper, as in every suite.
from .suite_common import (  # noqa: A004
    BACKUP,
    NATIVE_PROJECT_LIB_RS,
    RELOAD_MODULE_TOKEN,
    RELOAD_PROJECT_TOKEN,
    RELOAD_TIMEOUT,
    SETTLE_TIMEOUT,
    STABILITY_SLEEP,
    OutputMonitor,
    apply_replacements,
    has_crash_signals,
    print,
)

# =============================================================================
# Scenario model
# =============================================================================


@dataclass
class ScenarioPhase:
    """One edit-and-wait step inside a scenario."""

    edits: Sequence[Tuple[Path, Sequence[Tuple[str, str]]]]
    wait_token: str
    required_tokens: Sequence[str] = ()
    forbidden_tokens: Sequence[str] = ()
    # Tokens waited for from the start of the scenario's window, for text the
    # edit itself produces (a new spline count, a probe line): an occurrence
    # anywhere in the window after the edit satisfies it.
    wait_after: Sequence[Tuple[str, float]] = ()
    # Proof-of-life tokens, waited for from an index taken once `wait_token`
    # landed. A counter that kept printing while the reload was building
    # satisfies a window-indexed wait instantly, so that wait passes whether or
    # not the new generation ever ran; indexing after the reload outcome is the
    # only way to tell "still alive" from "was alive".
    alive_tokens: Sequence[Tuple[str, float]] = ()


@dataclass
class Scenario:
    """One reload scenario: sequential phases, expected output, cleanup."""

    name: str
    phases: Sequence[ScenarioPhase]
    # Files put back once the scenario is done, so the next one starts from the
    # originals. Each restore rewrites the file, which the watcher turns into
    # another reload; the runner waits for that reload so its output cannot
    # pollute the next scenario's window.
    restore_after: Sequence[Path] = field(default_factory=list)
    # Assertions that only the restore-driven reload can satisfy: the module
    # re-seeding fresh data after its forgotten type's column was dropped
    # logs `existing=0`, which the reload before the restore cannot produce.
    restore_required_tokens: Sequence[str] = ()
    # Checks over the scenario's window that this module cannot know about.
    # Called as `validator(scenario.name, output)`.
    validators: Sequence[Callable[[str, str], bool]] = ()


# =============================================================================
# Runner settings
# =============================================================================


def settle_token_for_test_project_path(path: Path) -> str:
    """The token a restore of `path` settles with, for the test project.

    Restoring a project file reloads the project; restoring a module file
    reloads the module, whose reload cascades into the project's own reload
    line - so the module token is the one that arrives first, and it is the one
    that means the module is back.
    """
    if path == NATIVE_PROJECT_LIB_RS:
        return RELOAD_PROJECT_TOKEN
    return RELOAD_MODULE_TOKEN


@dataclass(frozen=True)
class ScenarioRunner:
    """How a suite's scenarios run: its waits, and how a restore settles.

    Built by the suite rather than shared as one instance, because each suite
    scales these from `--timeout-scale` at startup and each has its own posture
    for a restore: the native suite's module reloads cascade into a project
    reload, while the C# suite swaps an assembly and settles on its own token.
    """

    reload_timeout: float = RELOAD_TIMEOUT
    settle_timeout: float = SETTLE_TIMEOUT
    stability_sleep: float = STABILITY_SLEEP
    restore_settle_token: Callable[[Path], str] = settle_token_for_test_project_path


# =============================================================================
# Scenario execution
# =============================================================================


def settle_restored_source(
    monitor: OutputMonitor,
    path: Path,
    settle_token: str,
    timeout: float,
    stability_sleep: float,
    start_index: Optional[int] = None,
) -> bool:
    """Puts `path` back and waits for the reload the restore triggers.

    The restore rewrites the file, which the watcher turns into another reload.
    Waiting for it keeps that reload out of the next edit's window, and keeps
    two edits to one crate from being coalesced into one.

    `start_index` is where the wait begins; the default is the monitor's
    current line count. Callers restoring several files pass the index they
    took before the first restore, so a build the watcher coalesced from two
    writes satisfies both waits instead of only the first.

    Returns False when the token never arrived. A suite warns and carries on
    (its assertions are window-based, and the next scenario waits for its own
    reload anyway).
    """
    if start_index is None:
        start_index = monitor.line_count
    BACKUP.restore_one(path)
    if not monitor.wait_for(settle_token, timeout, start_index):
        return False
    time.sleep(stability_sleep)
    return True


def run_scenario(
    scenario: Scenario,
    monitor: OutputMonitor,
    runner: ScenarioRunner = ScenarioRunner(),
) -> bool:
    """Runs one scenario and asserts the host's behaviour.

    Returns False at the first failed assertion; the caller decides whether a
    failed scenario ends the session (all three suites stop there).
    """
    print(f"\n  [TEST] {scenario.name}...")
    start_index = monitor.line_count

    for phase in scenario.phases:
        # Captured per phase, not per scenario. Waiting from the scenario's
        # start index lets phase 1's reload line satisfy phase 2's wait, which
        # returns immediately and then checks phase 2's tokens while its build
        # is still running - reported as a missing token, with the reload
        # finishing moments later.
        phase_index = monitor.line_count

        for path, replacements in phase.edits:
            # Captured here rather than relying on the suite's startup list:
            # `restore_one` silently does nothing for a path it never captured,
            # so a scenario touching a file the list did not name leaves that
            # file modified, and every scenario after it runs against the wrong
            # sources. `capture` is idempotent, so the startup list still stands.
            BACKUP.capture(path)
            if not apply_replacements(path, replacements):
                return False

        if not monitor.wait_for(phase.wait_token, runner.reload_timeout, phase_index):
            output = monitor.output_since(start_index)
            if has_crash_signals(output):
                print(f"  [FAIL] Crash detected in scenario: {scenario.name}")
            else:
                print(
                    f"  [FAIL] Timeout waiting for {phase.wait_token!r} in scenario: "
                    f"{scenario.name}"
                )
            print(f"  Output tail:\n{output[-1600:]}")
            return False

        time.sleep(runner.stability_sleep)

        if not monitor.process_alive():
            print(f"  [FAIL] Process died after scenario: {scenario.name}")
            return False

        for token, timeout in phase.wait_after:
            if not monitor.wait_for(token, timeout, start_index):
                print(f"  [FAIL] Missing expected token {token!r} in scenario: {scenario.name}")
                print(f"  Output tail:\n{monitor.output_since(start_index)[-1600:]}")
                return False

        alive_index = monitor.line_count
        for token, timeout in phase.alive_tokens:
            if not monitor.wait_for(token, timeout, alive_index):
                print(
                    f"  [FAIL] {token!r} did not appear AFTER the reload outcome "
                    f"in scenario: {scenario.name}"
                )
                print("         The code loaded before the edit should still be running.")
                print(f"  Output tail:\n{monitor.output_since(start_index)[-1600:]}")
                return False

    output = monitor.output_since(start_index)
    if has_crash_signals(output):
        print(f"  [FAIL] Crash token observed in scenario: {scenario.name}")
        print(f"  Output tail:\n{output[-1600:]}")
        return False

    for phase in scenario.phases:
        for token in phase.required_tokens:
            if token not in output:
                print(f"  [FAIL] Missing required token in {scenario.name}: {token!r}")
                print(f"  Output tail:\n{output[-1600:]}")
                return False
        for token in phase.forbidden_tokens:
            if token in output:
                print(f"  [FAIL] Forbidden token found in {scenario.name}: {token!r}")
                print(f"  Output tail:\n{output[-1600:]}")
                return False

    for validator in scenario.validators:
        if not validator(scenario.name, output):
            return False

    print(f"  [OK] {scenario.name}")

    # Restore edited sources so the next scenario starts from the originals.
    settle_start = monitor.line_count
    for path in scenario.restore_after:
        settle_token = runner.restore_settle_token(path)
        if not settle_restored_source(
            monitor,
            path,
            settle_token,
            runner.settle_timeout,
            runner.stability_sleep,
            settle_start,
        ):
            print(f"  [WARN] Restore of {path.name} did not settle with {settle_token!r}")

    # Post-restore assertions (e.g. the module re-seeding fresh data after a
    # forgotten-type drop: the restore-driven reload logs existing=0).
    for token in scenario.restore_required_tokens:
        if not monitor.wait_for(token, runner.settle_timeout, settle_start):
            print(f"  [FAIL] Missing post-restore token in {scenario.name}: {token!r}")
            print(f"  Output tail:\n{monitor.output_since(settle_start)[-1600:]}")
            return False

    return True
