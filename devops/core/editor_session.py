"""One driver for the editor end-to-end suites.

RESPONSIBILITIES
  - EditorSession: one editor run for a suite - the machine-global host lock,
    the stray-editor cleanup under it, a per-run folder (log, command file,
    LOCALAPPDATA sandbox), the launch environment and the character-anchored
    log waits the editor suite is built on.
  - The shared editor log tokens (*_TOKEN) the suite waits on, as plain
    substrings, so `test_log_contract.py` checks each against the Rust
    sources that print it.

DESIGN
  The suite anchors waits on character offsets into a growing log file: an
  assertion captures `start = session.length()` before it sends a command and
  re-scans only what arrives after. `OutputMonitor` in `suite_common` is a
  pipe-based line monitor with token searches; it cannot serve an offset
  anchor over the whole stream without changing every assertion, so
  EditorSession owns a log FILE (the child's stdout lands in it) and
  re-reads it tolerantly, exactly as the suites did. The lock, the
  stray-process cleanup and the sandbox conventions come from
  `suite_common`; per-run folders keep every run's log instead of
  overwriting a fixed one.

USAGE
  from core.editor_session import EditorSession

  session = EditorSession("commands")
  session.start()
  try:
      session.wait(EDITOR_WINDOW_CREATED_TOKEN, 300, "editor started")
      session.send("command editor.pause_toggle")
      session.wait(EDITOR_SCRIPTED_REJECTED_TOKEN, 60, "no rejection")
  finally:
      session.close()
"""

import codecs
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable, Dict, List, Optional, Tuple, TypeVar

import yaml

from core.editor_log_tokens import (  # noqa: F401 - re-exported for the suites
    EDITOR_SCRIPTED_REJECTED_TOKEN,
    REJECTION_PATTERN,
)
from core.environment import filesystem_timestamp
from core.paths import MODULES_ROOT, REPOSITORY_ROOT, executable_name
from core.projects import NATIVE_PROJECT_PATH
from core.suite_common import (
    ensure_host_lock,
    kill_stale_hosts,
    release_host_lock,
    verify_editor_binary,
)

# The editor binary every native-editor suite drives.
EDITOR_BINARY = executable_name("pill_editor")

# Every suite's run artifacts live here, one folder per suite and run.
EDITOR_VALIDATION_ROOT = REPOSITORY_ROOT / "local" / "editor-validation"

# The default poll for log waits: the cursor reads only appended bytes, so a
# poll costs one stat and, at most, one small read. The disk probes pass their
# own slower cadence because another process writes those files. 0.3 is the
# audit's conservative first step; the frame and count waits pass their own.
LOG_POLL_SECONDS = 0.3

# What a wait's probe produces; `wait_until` returns it unchanged.
T = TypeVar("T")

# When the last `[OK]` line was printed, so the next one can say how long the
# step it closes took. One clock for the whole process: the session's waits and
# a suite's own prints share the figure, which is the gap between two steps.
_LAST_STEP_OK_AT: List[float] = []


def print_step_ok(label: str, elapsed: Optional[float] = None) -> None:
    """Prints a step's `[OK]` line, then the milliseconds the step took.

    `elapsed` is the step's own measurement when the caller has one (a wait
    knows how long it waited); without it the figure is the time since the
    previous `[OK]`, which is what a reader of a slow run wants next to it.
    """
    now = time.monotonic()
    if elapsed is None:
        elapsed = now - _LAST_STEP_OK_AT[-1] if _LAST_STEP_OK_AT else 0.0
    _LAST_STEP_OK_AT.clear()
    _LAST_STEP_OK_AT.append(now)
    print("  [OK] {}".format(label))
    print("  [timing] {} {:.0f}".format(label, elapsed * 1000.0))

class ScriptedCommandRejected(RuntimeError):
    """A scripted line the editor refused, raised before any timeout can hide it."""

    def __init__(self, command: str, reason: str) -> None:
        super().__init__(
            "the editor rejected the scripted line {!r}: {}".format(command, reason)
        )
        self.command = command
        self.reason = reason

# =============================================================================
# Run directories
# =============================================================================

# One run folder per suite per process: a suite that launches several editor
# sessions (the undo suite's asset phases) shares its folder, one file set
# per session. The process id keeps two suites that start in the same second
# from sharing a folder, a log and a command file. Cached so every session of
# one process agrees on the path.
_RUN_DIRECTORIES: Dict[str, Path] = {}

def run_directory(suite: str) -> Path:
    """The run folder of `suite` for this process; computed once, then cached."""
    if suite not in _RUN_DIRECTORIES:
        _RUN_DIRECTORIES[suite] = EDITOR_VALIDATION_ROOT / "{}-{}-{}".format(
            suite, filesystem_timestamp(), os.getpid()
        )
    return _RUN_DIRECTORIES[suite]

# =============================================================================
# EditorSession
# =============================================================================

class EditorSession:
    """One editor run for a suite, with the paths and waits suites share.

    `start` takes the machine-global host lock before it kills stray
    processes, so two suites started together queue instead of killing each
    other's editor. `close` restores every source the suite edited, kills the
    editor if it is still alive, and releases the lock.
    """

    def __init__(
        self,
        suite: str,
        session: str = "main",
        project: str = NATIVE_PROJECT_PATH,
        test_seconds: int = 300,
        sandbox: Optional[Path] = None,
        binary: Optional[Path] = None,
    ) -> None:
        """Names the run: `suite` owns the folder, `session` the file set.

        `sandbox` overrides the session-local LOCALAPPDATA: a suite whose runs
        share one editor state (the native suite's layout migration) passes
        one path and every session of the suite reads and writes it. `binary`
        overrides the editor executable, for the native suite's `--binary`
        flag.
        """
        self.suite = suite
        self.session = session
        self.project = project
        self.test_seconds = test_seconds
        self._sandbox_override = Path(sandbox) if sandbox is not None else None
        self._binary = Path(binary) if binary is not None else EDITOR_BINARY
        self._process: Optional[subprocess.Popen] = None
        self._sources: Dict[Path, str] = {}
        # The log cursor: the text decoded so far and the byte position it
        # came from. The editor appends UTF-8, so a poll decodes only the new
        # tail; the character length is what every anchor uses.
        self._log_text = ""
        self._log_byte_position = 0
        self._log_decoder = codecs.getincrementaldecoder("utf-8")("replace")
        # Every character this session decoded over the whole run: the read
        # budget's figure. Appends are read once, so it tracks the log's length.
        self._log_characters_read = 0
        # Refusals found but not yet raised: `wait_until` raises the newest
        # once, and `_rejections_seen` keeps an old line from raising again.
        self._pending_rejections: List[Tuple[str, str]] = []
        # Refusals already reported to a suite, so a wait that reads the whole
        # log (the default anchor) only raises on a line new since the last
        # one: a probe that expects a refusal must not poison every later wait.
        self._rejections_seen = 0

    @property
    def binary(self) -> Path:
        """The editor executable this session launches."""
        return self._binary

    @classmethod
    def supported(cls) -> bool:
        """Whether this platform runs the native editor (Windows only)."""
        return os.name == "nt"

    @property
    def run_directory(self) -> Path:
        """This suite's run folder for the process."""
        return run_directory(self.suite)

    @property
    def log_path(self) -> Path:
        """The session's log file (the editor's merged stdout/stderr)."""
        return self.run_directory / "{}.log".format(self.session)

    @property
    def commands_path(self) -> Path:
        """The scripted command file the editor reads lines from."""
        return self.run_directory / "{}-commands.txt".format(self.session)

    @property
    def sandbox(self) -> Path:
        """The session's LOCALAPPDATA sandbox (layout, preferences, keymap)."""
        if self._sandbox_override is not None:
            return self._sandbox_override
        return self.run_directory / "{}-appdata".format(self.session)

    @property
    def editor_state_directory(self) -> Path:
        """The editor's own state folder inside the sandbox.

        The layout, the preferences and the keybindings live here; a suite
        that wants a file the editor watches writes it through
        `write_keybindings` rather than guessing the path itself.
        """
        return self.sandbox / "RustHybridEcs"

    def write_state_yaml(self, name: str, document: Dict) -> Path:
        """Writes one of the editor's live-read state files, in one format.

        The layout, the preferences and the keybindings all sit in
        `editor_state_directory`, and every one is written the same way: YAML
        in the editor's own dialect, keys sorted as the engine's own writer
        sorts them, and one trailing newline, so a suite's file reads like the
        editor's own and a byte comparison in a guard stays stable.
        Returns the path it wrote, for a suite that wants to read it back.
        """
        path = self.editor_state_directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        text = yaml.safe_dump(document, sort_keys=True, default_flow_style=False, width=1000)
        path.write_text(text, encoding="utf-8", newline="")
        return path

    def write_keybindings(self, document: Dict) -> None:
        """Writes the keybindings file the editor watches.

        The editor rereads the file every frame and applies a change without a
        restart, so a suite can rebind a shortcut mid-run and observe the
        result. The document is the `KeybindingsFile` shape: an optional
        `preset` and `overrides` mapping a command id to its chord list.
        """
        self.write_state_yaml("keybindings.yaml", document)

    def write_preferences(self, document: Dict) -> None:
        """Writes the preferences file the editor watches.

        The editor applies the keys it knows on the next frame; the rest sit
        in the document for the Preferences tab. A suite uses this to point
        `code_editor_command` at a stub before clicking a Problems row.
        """
        self.write_state_yaml("preferences.yaml", document)

    @property
    def stub_editor_record(self) -> Path:
        """The file the stub code editor appends each argument line to."""
        return self.sandbox / "stub-editor.txt"

    def write_stub_editor(self) -> Path:
        """Writes a stub code editor and points `code_editor_command` at it.

        The editor launches whatever the preference names, so the suite has to
        provide a program whose effect it can read. This one appends its
        argument - the row's `file:line` - to `stub-editor.txt` and exits.
        """
        stub = self.sandbox / "stub_editor.py"
        stub.parent.mkdir(parents=True, exist_ok=True)
        stub.write_text(
            "import pathlib\n"
            "import sys\n"
            "record = pathlib.Path(__file__).with_name('stub-editor.txt')\n"
            "record.write_text(' '.join(sys.argv[1:]) + '\\n', encoding='utf-8')\n",
            encoding="utf-8",
        )
        # A record left by an earlier phase would satisfy the next wait before
        # the editor ever ran the command.
        self.stub_editor_record.unlink(missing_ok=True)
        self.write_preferences(
            {"code_editor_command": '"{}" "{}"'.format(sys.executable, stub)}
        )
        return stub

    @property
    def process(self) -> Optional[subprocess.Popen]:
        """The editor process, for the few waits a suite runs itself."""
        return self._process

    # -------------------------------------------------------------------------
    # Lifecycle
    # -------------------------------------------------------------------------

    def start(
        self,
        project: Optional[str] = None,
        test_seconds: Optional[int] = None,
        extra_env: Optional[Dict[str, str]] = None,
        scripted: bool = True,
        preflight: Optional[Callable[[], Optional[str]]] = None,
    ) -> None:
        """Locks the host, clears strays, and launches the editor.

        `scripted=False` launches without the test channel: no command file is
        written and its environment variables are left unset, for the runs that
        only prove startup and exit.

        `preflight` runs under the host lock, after the strays are cleared and
        immediately before the launch, so a check that reads the tree cannot be
        overtaken by the wait for the lock. It returns why the launch must not
        happen, or None.

        Raises `SystemExit` with the refusal text when the binary, the images
        beside it, or the preflight say the tree cannot be launched.
        """
        if self._process is not None:
            raise RuntimeError("this session already started an editor")
        if not self._binary.is_file():
            raise SystemExit(
                "editor binary not found at {}; build it with "
                "`cargo build -p pill_editor` in modules/ first".format(self._binary)
            )
        # The lock first, then the kills: a concurrent suite's editor is only
        # ever killed by the holder, never by a queued waiter.
        ensure_host_lock()
        kill_stale_hosts()
        # The launch-time checks run under the lock, not before the wait for
        # it: a build that started while this session queued would otherwise
        # land between the suite's preflight and `Popen`, and the editor would
        # die at startup on a mixed set instead of the suite refusing. The
        # binary's own check runs at every start: a hot patch builds modules
        # into their own tree, so it cannot make the shared images newer.
        refusal = verify_editor_binary(self._binary)
        if refusal is None and preflight is not None:
            refusal = preflight()
        if refusal:
            release_host_lock()
            raise SystemExit(refusal)
        self.run_directory.mkdir(parents=True, exist_ok=True)
        self.sandbox.mkdir(parents=True, exist_ok=True)
        self._reset_log_cursor()
        self._log_characters_read = 0
        environment = dict(os.environ)
        environment["PROJECT_PATH"] = project or self.project
        environment["LOCALAPPDATA"] = str(self.sandbox)
        if scripted:
            self.commands_path.write_text(
                "# {} suite: {}\n".format(self.suite, self.session), encoding="utf-8"
            )
            environment["PILL_EDITOR_TEST_COMMANDS"] = str(self.commands_path)
            environment["PILL_EDITOR_TEST_SECONDS"] = str(test_seconds or self.test_seconds)
        environment.update(extra_env or {})
        log = self.log_path.open("w", encoding="utf-8")
        self._process = subprocess.Popen(
            [str(self._binary)],
            cwd=str(MODULES_ROOT),
            env=environment,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        log.close()

    def close(self) -> None:
        """Restores edited sources, kills the editor, releases the lock."""
        self.restore_sources()
        process = self._process
        if process is not None and process.poll() is None:
            process.kill()
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                pass
        release_host_lock()

    def __enter__(self) -> "EditorSession":
        self.start()
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def quit_and_wait(self, timeout: float = 120.0) -> int:
        """Sends `quit` and waits for the editor's exit; returns its code."""
        self.send("quit")
        assert self._process is not None, "the session has not started"
        return self._process.wait(timeout=timeout)

    def wait_exit(self, timeout: float = 120.0) -> int:
        """Waits for the editor to leave on its own (a UI quit); returns its code."""
        assert self._process is not None, "the session has not started"
        return self._process.wait(timeout=timeout)

    def exit_code(self) -> Optional[int]:
        """The editor's exit code once it has left, `None` while it runs."""
        assert self._process is not None, "the session has not started"
        return self._process.poll()

    # -------------------------------------------------------------------------
    # The log: file-backed, character-anchored waits
    # -------------------------------------------------------------------------

    def _reset_log_cursor(self) -> None:
        """Forgets the cached log: a new run's file starts empty.

        The read counter is deliberately not reset: a log that shrank is read
        again from the start, and the budget has to see that second read.
        """
        self._log_text = ""
        self._log_byte_position = 0
        self._log_decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self._pending_rejections = []
        self._rejections_seen = 0

    def _refresh_log(self) -> None:
        """Decodes the bytes appended since the last poll into the cache.

        The editor writes through a line-buffered stdout, so a read can land
        mid-line or mid-character; the incremental decoder holds a split
        character until its bytes arrive, and the refusal scan looks a short
        way back so a line split across two reads is still counted. A file
        that shrank (truncated or replaced) is read again from the start.
        """
        try:
            size = self.log_path.stat().st_size
        except OSError:
            return
        if size < self._log_byte_position:
            self._reset_log_cursor()
        if size == self._log_byte_position:
            return
        try:
            with self.log_path.open("rb") as log:
                log.seek(self._log_byte_position)
                chunk = log.read(size - self._log_byte_position)
        except OSError:
            return
        self._log_byte_position += len(chunk)
        appended = self._log_decoder.decode(chunk)
        self._log_text += appended
        self._log_characters_read += len(appended)
        # A refusal is rare, so the whole cached text is scanned only when one
        # may have landed; the short look-back catches a line split across two
        # reads, and the count keeps an earlier one from raising twice.
        if EDITOR_SCRIPTED_REJECTED_TOKEN in (appended or self._log_text[-512:]):
            rejected = REJECTION_PATTERN.findall(self._log_text)
            if len(rejected) > self._rejections_seen:
                self._pending_rejections.extend(rejected[self._rejections_seen :])
                self._rejections_seen = len(rejected)

    def text(self) -> str:
        """The whole log so far, decoding only what arrived since the last read."""
        self._refresh_log()
        return self._log_text

    def length(self) -> int:
        """The log's length in characters, the unit every anchor uses.

        This is the only anchor a caller should take - never a byte count
        from `log_path.stat()`: the log carries multi-byte characters, so a
        byte count can land inside one and re-expose earlier output to a
        wait's probe.
        """
        self._refresh_log()
        return len(self._log_text)

    def read_since(self, offset: int) -> str:
        """The log text from character `offset` on, where `offset` is the
        character index `length()` reports."""
        self._refresh_log()
        return self._log_text[offset:]

    def log_characters_read(self) -> int:
        """How many log characters this session has decoded so far.

        The cursor reads each appended byte once, so a healthy run's count stays
        at the log's own length. A reader that polls by rereading the whole file
        pushes the count far past it, which is what `read_budget_exceeded`
        watches for.
        """
        self._refresh_log()
        return self._log_characters_read

    def read_log(self) -> str:
        """The editor log so far, tolerant of a file the writer holds open."""
        return self.text()

    def wait_until(
        self,
        probe: Callable[[str], Optional[T]],
        timeout: float,
        timeout_message: str,
        label: str = "",
        exit_note: str = "",
        poll: float = LOG_POLL_SECONDS,
        announce: bool = True,
    ) -> T:
        """Polls the log until `probe` returns something other than None.

        The one deadline/poll/timeout shape the session and the suites share.
        The probe sees the log text and returns None while it is not
        satisfied, so a wait for a pattern, a count, or a frame counter past
        a threshold all read the same way. Prints `[OK] <label>` on success,
        raises `RuntimeError` when a running editor exits first and
        `TimeoutError` (with `timeout_message` and the log tail) on timeout.
        """
        deadline = time.monotonic() + timeout
        started_at = time.monotonic()
        prefix = "{}: ".format(label) if label else ""
        process = self._process
        while time.monotonic() < deadline:
            text = self.text()
            found = probe(text)
            if found is not None:
                if announce and label:
                    print_step_ok(label, time.monotonic() - started_at)
                return found
            if self._pending_rejections:
                command, reason = self._pending_rejections[-1]
                self._pending_rejections.clear()
                raise ScriptedCommandRejected(command, reason)
            if process is not None and process.poll() is not None:
                raise RuntimeError(
                    prefix
                    + "the editor exited {}{}\n".format(process.returncode, exit_note)
                    + text[-4000:]
                )
            # The log cursor reads only appended bytes, so a fast poll costs a
            # stat and, at most, one small read; the disk probes pass their own
            # slower cadence.
            time.sleep(poll)
        raise TimeoutError(timeout_message + self.read_log()[-4000:])

    def wait(self, pattern: str, timeout: float, label: str, start: int = 0) -> int:
        """Waits for `pattern` after character offset `start`; returns the new length.

        `start` is a character index into the decoded log, which is what
        `length()` and `read_since()` use - never a byte count from
        `log_path.stat()`: the log carries multi-byte characters, so a byte
        count can land inside one and re-expose earlier output to the probe.

        Same contract the suites grew up with: prints `[OK] <label>` on a
        match, raises `RuntimeError` when a running editor exits first, and
        `TimeoutError` on timeout - both with the log tail for diagnosis. A
        refusal of a scripted line raises `ScriptedCommandRejected` the moment
        it lands, so a typo never waits out the timeout; a refusal an earlier
        wait already raised on is not raised again.
        """
        expression = re.compile(pattern)

        def probe(text: str) -> Optional[int]:
            return len(text) if expression.search(text[start:]) else None

        return self.wait_until(
            probe,
            timeout,
            "{}: timeout waiting for {}\n".format(label, pattern),
            label=label,
        )

    def wait_token(self, token: str, timeout: float, label: str, start: int = 0) -> int:
        """Waits for a plain token (escaped before the regex search)."""
        return self.wait(re.escape(token), timeout, label, start)

    def wait_for_match(
        self, pattern: str, timeout: float, label: str, start: int = 0
    ) -> str:
        """Waits until `pattern` matches at or after `start`; returns the newest match.

        The wait's own match is what is returned, not the first line in the
        log: a token a run repeats would otherwise hand back an old one.
        """
        expression = re.compile(pattern)

        def probe(text: str) -> Optional[str]:
            matches = list(expression.finditer(text[start:]))
            return matches[-1].group(0) if matches else None

        return self.wait_until(
            probe,
            timeout,
            "{}: timeout waiting for {}\n".format(label, pattern),
            label=label,
        )

    # -------------------------------------------------------------------------
    # Engine run state: the pause gate the suites share
    # -------------------------------------------------------------------------

    # A running engine reports tens of frames a second in its scene frame
    # report; a paused one about one. Anything below the limit reads as
    # paused.
    RUNNING_FPS_LIMIT = 8.0

    def await_engine_frame_rate(self, timeout: float = 20.0) -> float:
        """The engine's tick rate, from the next two scene frame reports.

        The scene frame report prints about once a second in both states and
        carries the engine's own tick rate. Suites gate their pauses on it:
        an input or a leftover queued command can toggle the engine before a
        step expects it, and a blind toggle would flip it the wrong way. The
        wait is for fresh reports, not the last one in the log (a report
        trails a state change by up to a second), and the SECOND report is
        the one returned: the first can straddle the boot's own slow frames
        and read like a paused engine. Returns the last rate seen, 0.0 when
        no report lands within `timeout`.
        """
        deadline = time.monotonic() + timeout
        seen = 0
        rate = 0.0
        while True:
            start = self.length()
            # Half a report interval: a fresh report is picked up as early
            # as it lands.
            time.sleep(0.5)
            for line in self.read_since(start).splitlines():
                match = re.search(
                    r"\[editor\] scene frame report\s+fps=([0-9.]+)", line
                )
                if match:
                    seen += 1
                    rate = float(match.group(1))
            if seen >= 2 or time.monotonic() >= deadline:
                return rate

    def engine_is_running(self, timeout: float = 8.0) -> bool:
        """Whether the engine is ticking, by `await_engine_frame_rate`.

        No report yet counts as running: that is what the suites assumed
        before they had a state to read.
        """
        rate = self.await_engine_frame_rate(timeout)
        return rate == 0.0 or rate >= self.RUNNING_FPS_LIMIT

    def wait_occurrence(
        self, pattern: str, occurrence: int, timeout: float, label: str
    ) -> str:
        """Waits until `pattern` has appeared at least `occurrence` times.

        A token a run repeats - a window move, a popup dismissal - must be
        waited for by count: the earlier occurrence is already in the log and
        would satisfy a plain search before the new one has happened. Returns
        the matched text, which suites compare (a frame count, a placement).
        """
        expression = re.compile(pattern)

        def probe(text: str) -> Optional[str]:
            matches = [match.group(0) for match in expression.finditer(text)]
            return matches[occurrence - 1] if len(matches) >= occurrence else None

        return self.wait_until(
            probe,
            timeout,
            "{}: timeout waiting for occurrence {} of {}\n".format(
                label, occurrence, pattern
            ),
            label=label,
            exit_note=" before occurrence {} of {}".format(occurrence, pattern),
        )

    def wait_either(self, patterns: List[str], timeout: float, start: int = 0) -> Tuple[str, int]:
        """Waits for the first of `patterns`; returns the one seen and the length.

        Same contract as `wait`: `RuntimeError` when a running editor exits
        first, `TimeoutError` when none of the patterns appear in time.
        """
        expressions = [(pattern, re.compile(pattern)) for pattern in patterns]

        def probe(text: str) -> Optional[Tuple[str, int]]:
            window = text[start:]
            for pattern, expression in expressions:
                if expression.search(window):
                    return pattern, len(text)
            return None

        return self.wait_until(
            probe,
            timeout,
            "timeout waiting for any of {}\n".format(patterns),
            exit_note=" before any of {}".format(patterns),
            announce=False,
        )

    def find(self, pattern: str, start: int = 0) -> re.Match:
        """The first match of `pattern` in the log from `start`, now."""
        match = re.compile(pattern).search(self.read_since(start))
        if not match:
            raise RuntimeError(
                "no match for {}\n".format(pattern) + self.text()[-3000:]
            )
        return match

    def send(self, line: str) -> None:
        """Appends one scripted command, flushed, so the editor sees it whole."""
        with self.commands_path.open("a", encoding="utf-8") as file:
            file.write(line + "\n")

    def request(
        self,
        text: str,
        pattern: str,
        timeout: float = 30.0,
        label: Optional[str] = None,
    ) -> str:
        """Appends one scripted line and waits for its answer, anchored.

        The wait starts at the log's length before the line is written, so an
        answer the log already carried cannot satisfy it; a refusal of the
        line raises `ScriptedCommandRejected` at once. `pattern` is a regex
        (suites pass `re.escape(<token>)` for a literal). Returns the log text
        from the anchor through the answer, for the callers that parse it.
        """
        start = self.length()
        self.send(text)
        self.wait(pattern, timeout, label or "scripted {}".format(text), start)
        return self.read_since(start)

    def send_many(self, lines: List[str]) -> None:
        """Appends several scripted commands in one write.

        The channel polls once per frame, so a single write hands them over in
        the same frame - needed when their meaning depends on arriving together
        (a multi-file drop).
        """
        with self.commands_path.open("a", encoding="utf-8") as file:
            file.write("".join(line + "\n" for line in lines))

    # -------------------------------------------------------------------------
    # Sandbox files and edited sources
    # -------------------------------------------------------------------------

    def write_appdata_yaml(self, name: str, body: dict) -> Path:
        """Writes one of the editor's live-read files, through the one path."""
        return self.write_state_yaml(name, body)

    def edit_source(self, path: Path, text: str) -> None:
        """Writes `text` over a source file, LF preserved, restored on close.

        The original content is captured once and restored by `close`, so a
        suite never leaves an edited crate behind, whatever path it exits by.
        """
        path = Path(path)
        if path not in self._sources:
            with path.open("r", encoding="utf-8", newline="") as file:
                self._sources[path] = file.read()
        with path.open("w", encoding="utf-8", newline="") as file:
            file.write(text)

    def restore_sources(self) -> None:
        """Restores every source `edit_source` wrote, byte for byte."""
        for path, original in self._sources.items():
            with path.open("w", encoding="utf-8", newline="") as file:
                file.write(original)
        self._sources.clear()
