"""Windows native presentation, lifecycle and clean-shutdown acceptance check.
Run after building the selected frontend. Only touches the process it starts.

PROTECTS: The windowed frontend presents frames and shuts down cleanly.
TIER: host
RUN: manual - needs the windowed frontend binary as an argument

WHAT IS ASSERTED
    The first frame token, the window's size after a resize request, that the
    frame loop is still presenting after the resize/minimize/restore, that the
    window came back from the minimize, and a zero exit code. The renderer logs
    a resize it could not honour but nothing when it succeeds, so the window's
    own geometry is the only evidence a resize reached it.

"""
import argparse
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import subprocess
import sys
import time

# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core.projects import NATIVE_PROJECT_PATH  # noqa: E402


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("--project", default=NATIVE_PROJECT_PATH)
    parser.add_argument("--cwd", type=Path)
    parser.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    binary = args.binary.resolve()
    logs = root / "local" / "renderer-validation"
    logs.mkdir(parents=True, exist_ok=True)
    log_path = logs / (binary.stem + "-" + Path(args.project).name + "-smoke.log")
    environment = dict(os.environ, PROJECT_PATH=args.project)
    environment["LOCALAPPDATA"] = str(logs / "appdata")
    Path(environment["LOCALAPPDATA"]).mkdir(parents=True, exist_ok=True)
    environment["PATH"] = str(binary.parent / "deps") + os.pathsep + environment["PATH"]
    user32 = ctypes.windll.user32
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user32.ShowWindowAsync.argtypes = [wintypes.HWND, ctypes.c_int]
    user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
    user32.IsIconic.argtypes = [wintypes.HWND]

    def window_rect(window):
        rect = wintypes.RECT()
        user32.GetWindowRect(window, ctypes.byref(rect))
        return rect

    def wait_for_frame_statistic(seen: int, timeout: float) -> bool:
        """True once the frontend logs a frame-statistic line past `seen`.

        `FPS |` is printed every few seconds while the loop presents, and the
        shipping smoke test waits for it too. It is the only success-side token
        the renderer emits, so it is what shows the resize did not kill it.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if log_path.read_text(encoding="utf-8", errors="replace").count(" FPS | ") > seen:
                return True
            if process.poll() is not None:
                return False
            # Poll until another frame-statistic line appears.
            time.sleep(0.5)
        return False

    with log_path.open("w", encoding="utf-8") as log:
        process = subprocess.Popen([str(binary)], cwd=args.cwd or root / "modules", env=environment,
                                   stdout=log, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                text = log_path.read_text(encoding="utf-8", errors="replace")
                if "[render] First frame: Presented; camera=true" in text:
                    break
                if process.poll() is not None:
                    raise RuntimeError(f"frontend exited {process.returncode} before presenting\n{text[-6000:]}")
                # Poll until the frontend presents its first frame.
                time.sleep(0.5)
            else:
                raise TimeoutError(f"first-frame timeout; see {log_path}")
            windows = []
            callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
            @callback
            def collect(window, _):
                pid = wintypes.DWORD()
                user32.GetWindowThreadProcessId(window, ctypes.byref(pid))
                if pid.value == process.pid and user32.IsWindowVisible(window):
                    windows.append(window)
                return True
            user32.EnumWindows(collect, 0)
            if not windows:
                raise RuntimeError("frontend presented without a visible native window")
            def area(window):
                rect = window_rect(window)
                return (rect.right - rect.left) * (rect.bottom - rect.top)
            window = max(windows, key=area)
            before = window_rect(window)
            print("native scene presented; requesting a resize", flush=True)
            user32.SetWindowPos(window, None, 40, 40, 960, 720, 0x4004)
            # Give the window a moment to apply the requested size.
            time.sleep(1)
            resized = window_rect(window)
            width = resized.right - resized.left
            height = resized.bottom - resized.top
            if (width, height) == (before.right - before.left, before.bottom - before.top):
                raise RuntimeError("the window kept its original size: the resize never reached it")
            if abs(width - 960) > 32 or abs(height - 720) > 32:
                raise RuntimeError(f"resize requested 960x720, the window is {width}x{height}")
            print(f"resize applied: {width}x{height}", flush=True)
            user32.ShowWindowAsync(window, 6)
            # Let the minimize land before the restore is sent.
            time.sleep(1)
            user32.ShowWindowAsync(window, 9)
            # Give the restore time to finish before checking the window.
            time.sleep(2)
            if user32.IsIconic(window):
                raise RuntimeError("the window stayed minimized: the restore never reached it")
            if not user32.IsWindowVisible(window):
                raise RuntimeError("the window is not visible after the restore")
            print("minimize/restore left the window restored; checking the frame loop", flush=True)
            if not wait_for_frame_statistic(
                log_path.read_text(encoding="utf-8", errors="replace").count(" FPS | "), 30
            ):
                raise RuntimeError(f"the frame loop stopped after the window calls; see {log_path}")
            print("still presenting; checking shutdown", flush=True)
            user32.PostMessageW(window, 0x0010, 0, 0)
            result = process.wait(timeout=30)
            if result != 0:
                raise RuntimeError(f"frontend shutdown failed: {result}; see {log_path}")
            print(f"PASS: present, resize applied, still presenting, clean shutdown: {binary.name}; {log_path}")
        finally:
            if process.poll() is None:
                subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True)
                process.wait(timeout=30)


if __name__ == "__main__":
    main()
