"""The editor suite's window resize steps.

The main window has no system frame, so the shell draws the border bands that
resize it and the executable follows the pointer itself. These steps drag those
bands and read the OS window's own rectangle: the window has to follow the
pointer, the editor has to keep drawing while it does, and one size change has
to cost one scene-surface resize rather than two.

The area is also where the OS window itself is inspected, so the first step
asks Windows whether the window was created with the editor's icon.

The area opens a session of its own, because the run's shared one has already
quit its editor by the time the late areas run.
"""

import ctypes
import re
import time
from ctypes import wintypes

from core.editor_log_tokens import (
    EDITOR_WINDOW_CREATED_TOKEN,
    EGUI_ROWS_TOKEN,
    SCENE_SURFACE_RESIZED_TOKEN,
    WINDOW_RESIZE_ENDED_TOKEN,
    WINDOW_RESIZE_STARTED_TOKEN,
)
from core.editor_session import EditorSession
from editor_step_areas import StepArea, StepContext
from editor_step_support import print_ok, shell_frame

# How far inside the client area a border band is pressed, in pixels. The band
# covers the outermost four points of the window, so at any common DPI a press
# this close to the edge is inside it.
BAND_INSET = 2

# How long the window may take to reach the rectangle a drag asked for. The
# size arrives as a window event after the frame that requested it.
WINDOW_SETTLE_SECONDS = 5.0

# How far a drag's result may sit from what the pointer asked for.
PIXEL_TOLERANCE = 2

# The size changes a resize burst drives, and how many surface resizes it may
# cost. One per size change is the contract; a little slack absorbs a stale
# dock rectangle settling beside the burst.
BURST_STEPS = 30
BURST_RESIZE_ALLOWANCE = BURST_STEPS + 2

# How long the editor may take to draw its next frame during the burst.
FRAME_SETTLE_SECONDS = 2.0

user32 = ctypes.windll.user32
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
user32.SetWindowPos.argtypes = [
    wintypes.HWND,
    wintypes.HWND,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_int,
    wintypes.UINT,
]
ENUM_WINDOWS_PROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

# The icon the window was created with: `WM_GETICON` answers with its handle,
# which winit sets as the small icon. The taskbar's own icon comes from the
# executable's resource, which the icon suite reads out of the built binary.
WM_GETICON = 0x007F
ICON_SMALL = 0

user32.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.SendMessageW.restype = ctypes.c_ssize_t


def main_window_handle(process_id: int):
    """The editor's first visible top-level window, or `None`."""
    handles = []

    def collect(hwnd, _lparam):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if (
            owner.value == process_id
            and user32.IsWindowVisible(hwnd)
            and user32.GetWindowTextLengthW(hwnd) > 0
        ):
            handles.append(hwnd)
        return True

    user32.EnumWindows(ENUM_WINDOWS_PROC(collect), 0)
    return handles[0] if handles else None


def window_rect(hwnd: int) -> tuple:
    """The window's rectangle as `(x, y, width, height)` in screen pixels."""
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    return (rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top)


def window_icon_handle(hwnd: int) -> int:
    """The small icon handle the window carries, or zero when it has none."""
    return int(user32.SendMessageW(hwnd, WM_GETICON, ICON_SMALL, 0))


def wait_for_window_rect(hwnd: int, expected_width: int, expected_height: int) -> tuple:
    """Waits for the window to take a size, returning the rectangle last seen.

    The size arrives as a window event after the frame that asked for it, so a
    rectangle read the instant a drag ends can still be the old one.
    """
    deadline = time.monotonic() + WINDOW_SETTLE_SECONDS
    rect = window_rect(hwnd)
    while (rect[2], rect[3]) != (expected_width, expected_height):
        if time.monotonic() >= deadline:
            return rect
        time.sleep(0.02)
        rect = window_rect(hwnd)
    return rect


def resize_window(hwnd: int, width: int, height: int) -> None:
    """Asks the OS for one window size, the way a platform-driven drag does."""
    user32.SetWindowPos(hwnd, None, 0, 0, width, height, 0x0002 | 0x0004 | 0x0010)


def wait_for_next_frame(session: EditorSession) -> None:
    """Waits until the editor draws one more frame.

    The resize burst paces its size changes with it, so each change lands in a
    frame of its own the way a platform-driven drag arrives, however slow the
    machine is.
    """
    start = shell_frame(session)
    deadline = time.monotonic() + FRAME_SETTLE_SECONDS
    while shell_frame(session) <= start:
        assert time.monotonic() < deadline, "the editor drew no frame within {}s".format(
            FRAME_SETTLE_SECONDS
        )
        time.sleep(0.02)


def drag_band(session: EditorSession, start: tuple, end: tuple, label: str) -> None:
    """Drags a point of the client area to another and waits for the drag's end."""
    begin = session.length()
    session.send("drag {:.0f} {:.0f} {:.0f} {:.0f}".format(start[0], start[1], end[0], end[1]))
    session.wait(
        re.escape(WINDOW_RESIZE_STARTED_TOKEN),
        30,
        "{}: the resize drag started".format(label),
        begin,
    )
    session.wait(
        re.escape(WINDOW_RESIZE_ENDED_TOKEN), 30, "{}: the resize drag ended".format(label), begin
    )


def run_window_and_resize_steps(context: "StepContext") -> None:
    """The steps through: a size change costs one scene-surface resize."""
    session = EditorSession(
        "window_and_resize",
        "main",
        project=context.project,
        test_seconds=420,
        binary=context.binary,
    )
    session.start()
    try:
        session.wait_for_match(re.escape(EDITOR_WINDOW_CREATED_TOKEN), 180, "resize startup")
        session.wait(re.escape(EGUI_ROWS_TOKEN), 180, "the shell's rows heartbeat")
        _drive_resize_steps(session)
    finally:
        session.close()


def _drive_resize_steps(session: EditorSession) -> None:
    """The drags and the burst, run while the area's own session is alive."""
    frames_before = shell_frame(session)
    hwnd = main_window_handle(session.process.pid)
    assert hwnd is not None, "the editor has no visible window"
    left, top, width, height = window_rect(hwnd)

    # Step 0: the window carries the editor's icon.
    assert window_icon_handle(hwnd) != 0, (
        "the editor's main window has no icon: WM_GETICON answered zero"
    )
    print_ok("the main window carries the editor's icon")

    # Step 1: the east band, dragged in by 160 pixels. The height and the
    # window's own position must not move.
    drag_band(
        session,
        (width - BAND_INSET, height / 2),
        (width - BAND_INSET - 160, height / 2),
        "the east band",
    )
    narrowed = wait_for_window_rect(hwnd, width - 160, height)
    assert abs(narrowed[2] - (width - 160)) <= PIXEL_TOLERANCE, (
        "an east drag of -160 px left the window {} wide, not {}".format(
            narrowed[2], width - 160
        )
    )
    assert narrowed[3] == height, (
        "an east drag changed the window's height: {} -> {}".format(height, narrowed[3])
    )
    assert (narrowed[0], narrowed[1]) == (left, top), (
        "an east drag moved the window: {} -> {}".format((left, top), narrowed[:2])
    )
    print_ok("an east band drag narrowed the window {} -> {}".format(width, narrowed[2]))

    # Step 2: the south-east corner, grown on both axes.
    width, height = narrowed[2], narrowed[3]
    drag_band(
        session,
        (width - BAND_INSET, height - BAND_INSET),
        (width - BAND_INSET + 80, height - BAND_INSET + 60),
        "the south-east corner",
    )
    grown = wait_for_window_rect(hwnd, width + 80, height + 60)
    assert (
        abs(grown[2] - (width + 80)) <= PIXEL_TOLERANCE
        and abs(grown[3] - (height + 60)) <= PIXEL_TOLERANCE
    ), "a south-east drag of +80x+60 left the window {}x{}, not {}x{}".format(
        grown[2], grown[3], width + 80, height + 60
    )
    assert (grown[0], grown[1]) == (left, top), (
        "a south-east drag moved the window: {} -> {}".format((left, top), grown[:2])
    )
    print_ok("a corner drag grew the window to {}x{}".format(grown[2], grown[3]))

    # Step 3: the north-west corner, shrunk by 60x40. Both the size and the
    # position move, because the corner opposite the dragged one stays put.
    width, height = grown[2], grown[3]
    drag_band(
        session,
        (BAND_INSET, BAND_INSET),
        (BAND_INSET + 60, BAND_INSET + 40),
        "the north-west corner",
    )
    shrunk = wait_for_window_rect(hwnd, width - 60, height - 40)
    assert (
        abs(shrunk[2] - (width - 60)) <= PIXEL_TOLERANCE
        and abs(shrunk[3] - (height - 40)) <= PIXEL_TOLERANCE
    ), "a north-west drag of +60x+40 left the window {}x{}, not {}x{}".format(
        shrunk[2], shrunk[3], width - 60, height - 40
    )
    assert (
        abs(shrunk[0] - (left + 60)) <= PIXEL_TOLERANCE
        and abs(shrunk[1] - (top + 40)) <= PIXEL_TOLERANCE
    ), "a north-west drag left the window at {}, not {}".format(
        shrunk[:2], (left + 60, top + 40)
    )
    print_ok(
        "a north-west drag moved the window to {} and shrank it to {}x{}".format(
            shrunk[:2], shrunk[2], shrunk[3]
        )
    )

    # Step 4: the loop kept drawing across the three drags. A size the window
    # reports is applied by the frame that draws at it, so the counter has to
    # have moved.
    frames_after = shell_frame(session)
    assert frames_after > frames_before, (
        "the editor's frame counter did not advance across the drags: {} -> {}".format(
            frames_before, frames_after
        )
    )
    print_ok(
        "the editor kept drawing across the drags ({} frames)".format(
            frames_after - frames_before
        )
    )

    # Step 5: a burst of size changes, the way a platform-driven drag arrives.
    # One size change places the scene view once; two placements would mean the
    # default layout and the dock's own rectangle both reached the surface.
    width, height = shrunk[2], shrunk[3]
    begin = session.length()
    for step in range(BURST_STEPS):
        resize_window(hwnd, width + step * 4, height + step * 2)
        wait_for_next_frame(session)
    burst_text = session.text()[begin:]
    surface_resizes = burst_text.count(SCENE_SURFACE_RESIZED_TOKEN)
    settled = wait_for_window_rect(
        hwnd, width + (BURST_STEPS - 1) * 4, height + (BURST_STEPS - 1) * 2
    )
    assert (settled[2], settled[3]) == (
        width + (BURST_STEPS - 1) * 4,
        height + (BURST_STEPS - 1) * 2,
    ), "the burst left the window {}x{}".format(settled[2], settled[3])
    assert surface_resizes <= BURST_RESIZE_ALLOWANCE, (
        "{} size changes cost {} scene-surface resizes; one per change is the "
        "contract, so a count near twice that means the scene view is placed "
        "twice per size".format(BURST_STEPS, surface_resizes)
    )
    print_ok("{} size changes cost {} scene-surface resizes".format(BURST_STEPS, surface_resizes))
    resize_window(hwnd, width, height)


AREA = StepArea(
    name="window_and_resize",
    order=18.5,
    run=run_window_and_resize_steps,
    needs=(),
    provides=(),
)
