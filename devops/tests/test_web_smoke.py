"""
Builds a project for the browser and checks that it runs and draws there.

PROTECTS: A project built for the browser mounts its assets and draws frames with WebGPU.
TIER: build

REQUIREMENTS
  - Python 3.8+ (standard library only; Pillow, when installed, adds the
    picture check)
  - Rust toolchain with the `wasm32-unknown-unknown` target and `wasm-pack` on
    PATH (see `devops/tools/build_web.py`)
  - Chrome or Edge with WebGPU (Chrome/Edge 113+); `PILL_WEB_BROWSER` may name
    the executable

DESCRIPTION
    `devops/tools/build_web.py --dev` builds `devops/tests/projects/scene`, a
    project with a `res` directory, so the asset pack is exercised too. The
    build is served on a free local port and opened in a headless browser with
    WebGPU enabled, driven through the Chrome DevTools Protocol. Before the
    page loads, a listener is injected on every WebGPU device, so validation
    errors the engine does not see are still reported.

    Asserted:
      1. The project's asset pack was downloaded beside the page and mounted:
         the module does not carry the project's assets.
      2. The renderer presents a first frame with a camera.
      3. Frames keep running (frame statistics are reported).
      4. No engine error, WebGPU uncaptured error or page exception occurs.
      5. With Pillow installed: at least one capture of the canvas, taken once
         per second, shows a lit scene rather than an empty frame. The scene
         rotates, so not every moment shows it.
      6. Input reaches the game: a key press (sent without a click, so the
         canvas must hold focus from the start), a wheel turn and a mouse
         button, each logged by the project's control systems.

    The shipping bundle the build regenerates is restored afterwards, so the
    tree is left as it was found.

    `--project` builds another project instead; its input is not checked,
    because the logged reactions are the default project's. `--save-capture`
    keeps the last canvas capture as a PNG, for a look at what was drawn.

    `--editor` builds the editor posture instead (`build_web.py --dev --editor`)
    and drives its scripted channel: `dump play`, `command editor.play`,
    `dump play`, `command editor.stop`, `dump play`, one line per
    `EDITOR_STEP_SECONDS` once `window.pillEditor` exists. It asserts that the
    page reported edit mode, that Play switched it to playing, and that Stop
    restored the world (`restore_epoch=1`).

USAGE
  python devops/tests/test_web_smoke.py [--timeout-scale N] [--seconds N]
      [--project PATH] [--save-capture FILE] [--editor]

EXAMPLE USAGE
  python devops/tests/test_web_smoke.py
  python devops/tests/test_web_smoke.py --timeout-scale 2 --seconds 20
  python devops/tests/test_web_smoke.py --project examples/pills/pills_rust --save-capture local/temp/web.png
  python devops/tests/test_web_smoke.py --editor --project devops/tests/projects/native

--- SCRIPT ---
"""

# Standard library
import argparse
import base64
import functools
import http.server
import io
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.paths import REPOSITORY_ROOT  # noqa: E402
from core.projects import SCENE_PROJECT_ROOT, repository_relative  # noqa: E402
from core.suite_common import (  # noqa: E402
    SHIPPING_BUNDLE_PATHS,
    restore_paths,
    run_suite_with_timing,
    snapshot_paths,
)

# =============================================================================
# Constants
# =============================================================================

# The project built for the browser by default; its web build lands in the
# project's `build/web`.
PROJECT = repository_relative(SCENE_PROJECT_ROOT)
BUILD_SCRIPT = REPOSITORY_ROOT / "devops" / "tools" / "build_web.py"

# Tokens the page's console must show.
PACK_TOKEN = "mounted the fetched asset pack"
FIRST_FRAME_TOKEN = "[render] First frame: Presented; camera=true"
FRAME_TOKEN = "FPS |"
# The pack the page fetches, and the window of a `res` file the module must not
# contain: a browser build keeps its assets out of the wasm.
ASSET_PACK_FILE_NAME = "assets.pillpack"
EMBEDDED_PROBE_BYTES = 512
MODULE_NAME = "pkg/pill_web_app_bg.wasm"

# Input sent through the DevTools protocol once the first frame is up, and what
# the project logs when its systems see it (`devops/tests/projects/scene/
# src/controls.rs`). The key goes first and without a click, so it also checks
# that the canvas holds keyboard focus from the start. The wheel scrolls down,
# moving the camera back: closer would put it inside the models, and the
# picture check would see an empty frame.
INPUT_DELAY_SECONDS = 1.0
INPUT_EVENTS = (
    ("Input.dispatchKeyEvent", {"type": "keyDown", "code": "KeyW", "key": "w", "windowsVirtualKeyCode": 87}),
    ("Input.dispatchKeyEvent", {"type": "keyUp", "code": "KeyW", "key": "w", "windowsVirtualKeyCode": 87}),
    ("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": 640, "y": 360}),
    ("Input.dispatchMouseEvent", {"type": "mouseWheel", "x": 640, "y": 360, "deltaX": 0, "deltaY": 240}),
    ("Input.dispatchMouseEvent", {"type": "mousePressed", "x": 640, "y": 360, "button": "left", "clickCount": 1}),
    ("Input.dispatchMouseEvent", {"type": "mouseReleased", "x": 640, "y": 360, "button": "left", "clickCount": 1}),
)
INPUT_TOKENS = (
    ("key pressed: KeyW", "a key press did not reach the game"),
    ("scrolled; camera distance", "the mouse wheel did not reach the game"),
    ("mouse button pressed: Left", "a mouse button did not reach the game"),
)

# A blurred capture whose brightness varies this much shows a scene; an empty
# frame is the clear colour under the lens grain, which blurs flat.
SCENE_STANDARD_DEVIATION = 15.0

# The editor posture's play check: the lines pushed into the page's scripted
# channel, one per `EDITOR_STEP_SECONDS`, once `window.pillEditor` exists. The
# dumps either side of the session are what the check reads.
PLAY_DUMP_TOKEN = "[test] play"
PLAY_EDITING_TOKEN = 'mode="editing"'
PLAY_PLAYING_TOKEN = 'mode="playing"'
PLAY_RESTORED_TOKEN = "restore_epoch=1"
# The editor page's own frame report, which stands in for the game posture's
# project statistics line in the frame check.
EDITOR_FRAME_TOKEN = "[editor] web frame"
EDITOR_PLAY_LINES = (
    "dump play",
    "command editor.play",
    "dump play",
    "command editor.stop",
    "dump play",
)
EDITOR_STEP_SECONDS = 1.5
# How long an editor run watches the page: the wasm boots, then the five lines
# above have their own time.
EDITOR_MIN_SECONDS = 20.0

BUILD_TIMEOUT = 1800
BROWSER_START_TIMEOUT = 30

# Browsers tried in order when `PILL_WEB_BROWSER` is not set.
BROWSER_CANDIDATES = (
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
)

# Reports every WebGPU uncaptured error and device loss to the console; runs
# before the page's own scripts.
INJECTED_SCRIPT = r"""
(() => {
  if (!window.GPUAdapter) return;
  const requestDevice = GPUAdapter.prototype.requestDevice;
  GPUAdapter.prototype.requestDevice = async function (...args) {
    const device = await requestDevice.apply(this, args);
    device.addEventListener("uncapturederror", (event) =>
      console.error("WEBGPU-UNCAPTURED " + event.error.message));
    device.lost.then((info) => {
      if (info.reason !== "destroyed") console.error("WEBGPU-LOST " + info.message);
    });
    return device;
  };
})();
"""

# =============================================================================
# DevTools connection
# =============================================================================


class DevToolsSocket:
    """A minimal client-side WebSocket, enough for the DevTools protocol."""

    def __init__(self, url: str):
        address = url[len("ws://"):]
        host_port, path = address.split("/", 1)
        host, port = host_port.split(":")
        self.connection = socket.create_connection((host, int(port)))
        key = base64.b64encode(os.urandom(16)).decode()
        request = (
            f"GET /{path} HTTP/1.1\r\nHost: {host_port}\r\nUpgrade: websocket\r\n"
            f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.connection.sendall(request.encode())
        response = b""
        while b"\r\n\r\n" not in response:
            response += self.connection.recv(1)
        self.next_identifier = 0

    def send(self, method: str, params=None) -> int:
        """Send one command as a masked text frame; returns its id."""
        self.next_identifier += 1
        payload = json.dumps(
            {"id": self.next_identifier, "method": method, "params": params or {}}
        ).encode()
        header = bytearray([0x81])
        length = len(payload)
        if length < 126:
            header.append(0x80 | length)
        elif length < 65536:
            header.append(0x80 | 126)
            header += struct.pack(">H", length)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", length)
        mask = os.urandom(4)
        header += mask
        masked = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
        self.connection.sendall(bytes(header) + masked)
        return self.next_identifier

    def read_exactly(self, count: int) -> bytes:
        """Read `count` bytes or raise when the connection closes."""
        data = b""
        while len(data) < count:
            chunk = self.connection.recv(count - len(data))
            if not chunk:
                raise ConnectionError("the browser closed the DevTools connection")
            data += chunk
        return data

    def receive(self) -> dict:
        """Read one message, reassembling continuation frames."""
        message = b""
        while True:
            first, second = self.read_exactly(2)
            length = second & 0x7F
            if length == 126:
                length = struct.unpack(">H", self.read_exactly(2))[0]
            elif length == 127:
                length = struct.unpack(">Q", self.read_exactly(8))[0]
            message += self.read_exactly(length)
            if first & 0x80:
                return json.loads(message.decode("utf-8", errors="replace"))


# =============================================================================
# Steps
# =============================================================================


def find_browser():
    """The browser to drive: `PILL_WEB_BROWSER`, else the first one installed."""
    configured = os.environ.get("PILL_WEB_BROWSER")
    if configured:
        return configured if Path(configured).is_file() else None
    return next((path for path in BROWSER_CANDIDATES if Path(path).is_file()), None)


def serve(directory: Path) -> http.server.ThreadingHTTPServer:
    """Serve `directory` on a free local port, quietly, with the wasm MIME type."""

    class QuietHandler(http.server.SimpleHTTPRequestHandler):
        extensions_map = {
            **http.server.SimpleHTTPRequestHandler.extensions_map,
            ".wasm": "application/wasm",
        }

        def log_message(self, *arguments):
            pass

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(QuietHandler, directory=str(directory))
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def scene_variation(png_bytes: bytes):
    """How much a capture's blurred brightness varies, or None without Pillow."""
    try:
        from PIL import Image, ImageFilter, ImageStat
    except ImportError:
        return None
    image = Image.open(io.BytesIO(png_bytes)).convert("L").filter(ImageFilter.GaussianBlur(6))
    return ImageStat.Stat(image).stddev[0]


def observe_page(
    browser: str, url: str, seconds: float, send_input: bool, captures: list, editor: bool = False
) -> tuple:
    """Open `url` and collect console text, problems and capture variations.

    Sends the default project's input when `send_input`; every capture's PNG
    bytes are appended to `captures`. With `editor`, the page's scripted channel
    is driven through `EDITOR_PLAY_LINES` instead: one session, entered and left.
    """
    console_lines = []
    problems = []
    variations = []
    with tempfile.TemporaryDirectory() as profile:
        process = subprocess.Popen(
            [
                browser,
                "--headless=new",
                "--enable-unsafe-webgpu",
                f"--user-data-dir={profile}",
                "--no-first-run",
                "--window-size=1280,720",
                "--remote-debugging-port=0",
                "about:blank",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            # The browser writes the port it chose into the profile directory.
            port_file = Path(profile) / "DevToolsActivePort"
            deadline = time.monotonic() + BROWSER_START_TIMEOUT
            while not port_file.is_file() and time.monotonic() < deadline:
                # Poll until the browser writes its DevTools port file.
                time.sleep(0.2)
            if not port_file.is_file():
                return console_lines, ["the browser did not open its DevTools port"], variations
            # Let the port file finish being written before reading it.
            time.sleep(0.5)
            port = port_file.read_text().split()[0]
            targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{port}/json").read())
            page = next(target for target in targets if target["type"] == "page")
            devtools = DevToolsSocket(page["webSocketDebuggerUrl"])
            for method in ("Page.enable", "Runtime.enable"):
                devtools.send(method)
            devtools.send("Page.addScriptToEvaluateOnNewDocument", {"source": INJECTED_SCRIPT})
            devtools.send("Page.navigate", {"url": url})

            # Collect events, once a second ask for a capture of the page, and
            # send the input shortly after the first frame.
            devtools.connection.settimeout(0.5)
            end = time.monotonic() + seconds
            next_capture = time.monotonic() + 1
            capture_requests = set()
            input_due = None
            # An editor page is driven once its scripted channel exists, which
            # the probe below polls for.
            editor_ready_at = None
            editor_lines_sent = 0
            next_editor_probe = time.monotonic()
            while time.monotonic() < end:
                if time.monotonic() >= next_capture:
                    capture_requests.add(devtools.send("Page.captureScreenshot", {"format": "png"}))
                    next_capture += 1
                if input_due is not None and time.monotonic() >= input_due:
                    for method, params in INPUT_EVENTS:
                        devtools.send(method, params)
                    input_due = float("inf")
                if editor and editor_ready_at is None and time.monotonic() >= next_editor_probe:
                    devtools.send(
                        "Runtime.evaluate",
                        {"expression": "typeof window.pillEditor", "returnByValue": True},
                    )
                    next_editor_probe = time.monotonic() + 0.5
                if editor and editor_ready_at is not None:
                    due = editor_ready_at + EDITOR_STEP_SECONDS * (editor_lines_sent + 1)
                    if editor_lines_sent < len(EDITOR_PLAY_LINES) and time.monotonic() >= due:
                        line = EDITOR_PLAY_LINES[editor_lines_sent]
                        devtools.send(
                            "Runtime.evaluate",
                            {
                                "expression": "window.pillEditor.command("
                                + json.dumps(line)
                                + ")",
                                "returnByValue": True,
                            },
                        )
                        editor_lines_sent += 1
                try:
                    message = devtools.receive()
                except (socket.timeout, TimeoutError):
                    continue
                result = message.get("result")
                if editor and editor_ready_at is None and isinstance(result, dict):
                    if result.get("result", {}).get("value") == "object":
                        editor_ready_at = time.monotonic()
                if message.get("id") in capture_requests and "result" in message:
                    png_bytes = base64.b64decode(message["result"]["data"])
                    captures.append(png_bytes)
                    variation = scene_variation(png_bytes)
                    if variation is not None:
                        variations.append(variation)
                method = message.get("method")
                if method == "Runtime.consoleAPICalled":
                    level = message["params"]["type"]
                    text = " ".join(
                        str(argument.get("value", argument.get("description", "")))
                        for argument in message["params"]["args"]
                    )
                    console_lines.append(text)
                    if send_input and input_due is None and FIRST_FRAME_TOKEN in text:
                        input_due = time.monotonic() + INPUT_DELAY_SECONDS
                    if level == "error":
                        problems.append(f"console error: {text[:300]}")
                elif method == "Runtime.exceptionThrown":
                    details = message["params"]["exceptionDetails"]
                    description = details.get("exception", {}).get("description", details.get("text"))
                    problems.append(f"page exception: {str(description)[:300]}")
        finally:
            process.kill()
            process.wait()
    return console_lines, problems, variations


def res_files_in_module(module_path: Path, res_directory: Path) -> list:
    """The `res` files whose bytes the wasm module carries.

    A window from the middle of each file that is comfortably larger than the
    window, so a coincidental match is not plausible; a browser build fetches
    its pack and leaves these out.
    """
    if not module_path.is_file() or not res_directory.is_dir():
        return []
    module = module_path.read_bytes()
    embedded = []
    for source in sorted(res_directory.rglob("*")):
        if not source.is_file():
            continue
        data = source.read_bytes()
        if len(data) < EMBEDDED_PROBE_BYTES * 2:
            continue
        start = len(data) // 2
        if data[start : start + EMBEDDED_PROBE_BYTES] in module:
            embedded.append(str(source.relative_to(res_directory)))
    return embedded


def run_smoke_test(
    timeout_scale: float, seconds: float, project: str, save_capture, editor: bool = False
) -> int:
    """Build, serve and open the web build of `project`, then check what it
    did; the last capture goes to `save_capture` when given.

    With `editor`, the editor posture is built and the scripted channel's play
    verbs are driven instead of the game's input.
    """
    web_build_directory = REPOSITORY_ROOT / project / "build" / "web"
    checks_input = project == PROJECT and not editor
    if editor:
        seconds = max(seconds, EDITOR_MIN_SECONDS)
    print("=" * 70)
    print("  Web Editor Play Smoke Test" if editor else "  Web Build Smoke Test")
    print("=" * 70)

    browser = find_browser()
    if browser is None:
        print("  [FAIL] no Chrome or Edge found; set PILL_WEB_BROWSER")
        return 1

    # Step 1: build, restoring the shipping bundle the build regenerates.
    build_command = [sys.executable, str(BUILD_SCRIPT), "--dev"]
    if editor:
        build_command.append("--editor")
    build_command.append(project)
    print(f"\n  [BUILD] {' '.join(build_command[3:])}")
    snapshot = snapshot_paths(SHIPPING_BUNDLE_PATHS)
    try:
        result = subprocess.run(
            build_command,
            cwd=str(REPOSITORY_ROOT),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=BUILD_TIMEOUT * timeout_scale,
        )
    finally:
        restore_paths(SHIPPING_BUNDLE_PATHS, snapshot)
    if result.returncode != 0:
        print("  [FAIL] the web build failed:")
        print((result.stdout + result.stderr)[-3000:])
        return 1
    print("  [OK] built " + str(web_build_directory))

    # Step 2: serve it and watch it run.
    print(f"\n  [RUN] {Path(browser).name}, headless with WebGPU, for {seconds:.0f} s")
    server = serve(web_build_directory)
    captures = []
    try:
        url = f"http://127.0.0.1:{server.server_address[1]}/"
        console_lines, problems, variations = observe_page(
            browser, url, seconds * timeout_scale, checks_input, captures, editor
        )
    finally:
        server.shutdown()
    if save_capture and captures:
        Path(save_capture).write_bytes(captures[-1])
        print(f"  [SAVED] the last capture: {save_capture}")

    # Step 3: judge.
    console = "\n".join(console_lines)
    console_dump = os.environ.get("PILL_WEB_CONSOLE_OUT")
    if console_dump:
        Path(console_dump).write_text(console, encoding="utf-8")
        print(f"  [SAVED] the whole page console: {console_dump}")
    failures = list(problems)
    if editor:
        # The editor posture: one session entered and left, read from the
        # scripted channel's dumps. The dumps either side of the session are
        # what tell an entered session from a refused one.
        if console.count(PLAY_DUMP_TOKEN) < 3:
            failures.append(
                f"the editor answered {console.count(PLAY_DUMP_TOKEN)} dump(s) instead of 3"
            )
        if PLAY_EDITING_TOKEN not in console:
            failures.append("the editor never reported edit mode")
        if PLAY_PLAYING_TOKEN not in console:
            failures.append("Play never switched the run mode")
        if PLAY_RESTORED_TOKEN not in console:
            failures.append("Stop never restored the world (no restore_epoch=1)")
    pack_path = web_build_directory / ASSET_PACK_FILE_NAME
    if not pack_path.is_file():
        failures.append(f"the page has no {ASSET_PACK_FILE_NAME} beside it to fetch")
    embedded = res_files_in_module(
        web_build_directory / MODULE_NAME, REPOSITORY_ROOT / project / "res"
    )
    if embedded:
        failures.append(
            "the module carries the project's assets instead of fetching them: "
            + ", ".join(embedded[:5])
        )
    for token, meaning in (
        (PACK_TOKEN, "the fetched asset pack was not mounted"),
        (FIRST_FRAME_TOKEN, "no first frame was presented with a camera"),
        # An editor page reports its own frames: the project's statistics line
        # belongs to the game posture's project, which the editor does not run
        # the same way.
        (
            EDITOR_FRAME_TOKEN if editor else FRAME_TOKEN,
            "frames did not keep running",
        ),
        *(INPUT_TOKENS if checks_input else ()),
    ):
        if token not in console:
            failures.append(meaning)
    if not editor and variations and max(variations) < SCENE_STANDARD_DEVIATION:
        failures.append(
            f"no capture showed a scene (largest variation {max(variations):.1f})"
        )
    if failures:
        print("\n  [FAIL] " + "\n  [FAIL] ".join(failures))
        print("\n  Console tail:\n" + console[-2000:])
        return 1

    print("  [OK] the asset pack was fetched and mounted")
    print("  [OK] the module carries none of the project's assets")
    print("  [OK] the first frame was presented with a camera")
    print(f"  [OK] frames kept running ({console.count(FRAME_TOKEN)} statistics report(s))")
    if editor:
        print(
            f"  [OK] one session entered and left ({console.count(PLAY_DUMP_TOKEN)} dumps)"
        )
    elif checks_input:
        print("  [OK] keyboard, wheel and mouse button input reached the game")
    else:
        print("  [SKIP] input check: it reads the default project's logs")
    print("  [OK] no engine error, WebGPU error or page exception")
    if variations:
        print(f"  [OK] a lit scene was drawn (largest variation {max(variations):.1f})")
    else:
        print("  [SKIP] picture check: Pillow is not installed")
    print("-" * 70)
    print("  TEST PASSED")
    print("=" * 70)
    return 0


def main() -> None:
    """Parse arguments and run the smoke test."""
    parser = argparse.ArgumentParser(
        prog="test_web_smoke.py",
        description="Build a project for the browser and check it runs and draws.",
    )
    parser.add_argument(
        "--timeout-scale",
        type=float,
        default=1.0,
        help="multiply every timeout, for slower machines (default 1.0)",
    )
    parser.add_argument(
        "--seconds",
        type=float,
        default=15.0,
        help="how long to watch the running page (default 15)",
    )
    parser.add_argument(
        "--project",
        default=PROJECT,
        help=f"the project to build, relative to the repository root (default {PROJECT})",
    )
    parser.add_argument(
        "--save-capture",
        default=None,
        help="write the last canvas capture to this PNG file",
    )
    parser.add_argument(
        "--editor",
        action="store_true",
        help="build the editor posture and drive its play verbs instead of the game's input",
    )
    arguments = parser.parse_args()
    sys.exit(
        run_smoke_test(
            arguments.timeout_scale,
            arguments.seconds,
            arguments.project,
            arguments.save_capture,
            arguments.editor,
        )
    )


if __name__ == "__main__":
    run_suite_with_timing(main)
