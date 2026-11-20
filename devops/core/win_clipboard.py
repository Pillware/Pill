"""The editor suite's side of the Windows clipboard.

REQUIREMENTS
    Windows, a 64-bit CPython, no third-party packages.

DESCRIPTION
    The editor registers its structured payloads under the clipboard format
    `PillEditor.Object` and also writes plain text. A suite that only watched
    the editor's own log could not tell whether the payload really crossed the
    process boundary, so these helpers read and write the clipboard through
    `ctypes` exactly as another application would: the registered format
    first, `CF_UNICODETEXT` second.

USAGE
    write_payload(text)        put `text` on the clipboard, both formats
    read_payload()             the registered format's text, None when absent
    read_text()                the plain text, None when absent
    clear()                    empty the clipboard

EXAMPLE USAGE
    from core.win_clipboard import read_payload, write_payload
    write_payload('{"pill": 1, "kind": "component", ...}')
    assert json.loads(read_payload())["kind"] == "component"

--- SCRIPT ---
"""

import sys

# The bindings below exist only on Windows, and `ctypes.WinDLL` is missing
# elsewhere; refuse first so the failure names the platform.
if sys.platform != "win32":
    raise RuntimeError("the editor clipboard suite needs Windows")

import ctypes
from ctypes import wintypes

# The registered format both the editor and this helper agree on.
FORMAT_NAME = "PillEditor.Object"
CF_UNICODETEXT = 13
GMEM_MOVEABLE = 0x0002

_user32 = ctypes.WinDLL("user32", use_last_error=True)
_kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

_user32.RegisterClipboardFormatW.argtypes = [wintypes.LPCWSTR]
_user32.RegisterClipboardFormatW.restype = wintypes.UINT
_user32.OpenClipboard.argtypes = [wintypes.HWND]
_user32.OpenClipboard.restype = wintypes.BOOL
_user32.CloseClipboard.argtypes = []
_user32.CloseClipboard.restype = wintypes.BOOL
_user32.EmptyClipboard.argtypes = []
_user32.EmptyClipboard.restype = wintypes.BOOL
_user32.IsClipboardFormatAvailable.argtypes = [wintypes.UINT]
_user32.IsClipboardFormatAvailable.restype = wintypes.BOOL
_user32.GetClipboardData.argtypes = [wintypes.UINT]
_user32.GetClipboardData.restype = wintypes.HANDLE
_user32.SetClipboardData.argtypes = [wintypes.UINT, wintypes.HANDLE]
_user32.SetClipboardData.restype = wintypes.HANDLE
_kernel32.GlobalAlloc.argtypes = [wintypes.UINT, ctypes.c_size_t]
_kernel32.GlobalAlloc.restype = wintypes.HGLOBAL
_kernel32.GlobalLock.argtypes = [wintypes.HGLOBAL]
_kernel32.GlobalLock.restype = ctypes.c_void_p
_kernel32.GlobalUnlock.argtypes = [wintypes.HGLOBAL]
_kernel32.GlobalUnlock.restype = wintypes.BOOL
_kernel32.GlobalFree.argtypes = [wintypes.HGLOBAL]
_kernel32.GlobalFree.restype = wintypes.HGLOBAL


def _payload_format():
    # The format id the editor registered; the name is process-independent, so
    # the id a reader gets is the same one the editor wrote under.
    return _user32.RegisterClipboardFormatW(FORMAT_NAME)


def _open():
    # A null owner asks for the clipboard on behalf of this process. The
    # clipboard is a global lock: a failure means another process holds it,
    # which the caller retries rather than treating as "empty".
    if not _user32.OpenClipboard(None):
        raise OSError(ctypes.get_last_error(), "OpenClipboard failed")


def _close():
    _user32.CloseClipboard()


def _read_format(fmt):
    """The text one clipboard format carries, or None when it is absent."""
    if not _user32.IsClipboardFormatAvailable(fmt):
        return None
    handle = _user32.GetClipboardData(fmt)
    if not handle:
        return None
    pointer = _kernel32.GlobalLock(handle)
    if not pointer:
        return None
    try:
        # The block is NUL-terminated UTF-16; its length is not reported, so it
        # is measured by walking to the terminator.
        units = ctypes.cast(pointer, ctypes.POINTER(ctypes.c_uint16))
        length = 0
        while units[length] != 0:
            length += 1
        return bytes(
            ctypes.string_at(pointer, length * 2)
        ).decode("utf-16-le", errors="replace")
    finally:
        _kernel32.GlobalUnlock(handle)


def _with_clipboard(action):
    """Run `action` with the clipboard open, retrying a busy clipboard."""
    last = None
    for _attempt in range(40):
        try:
            _open()
        except OSError as error:
            last = error
            import time
            time.sleep(0.05)
            continue
        try:
            return action()
        finally:
            _close()
    raise last if last is not None else OSError("the clipboard stayed busy")


def read_payload():
    """The registered format's text, or None when the clipboard lacks one."""
    return _with_clipboard(lambda: _read_format(_payload_format()))


def read_text():
    """The clipboard's plain text, or None when it holds none."""
    return _with_clipboard(lambda: _read_format(CF_UNICODETEXT))


def _set_format(fmt, text):
    """Put one UTF-16 copy of `text` under `fmt`, taking the allocation over."""
    data = text.encode("utf-16-le") + b"\x00\x00"
    handle = _kernel32.GlobalAlloc(GMEM_MOVEABLE, len(data))
    if not handle:
        raise OSError(ctypes.get_last_error(), "GlobalAlloc failed")
    pointer = _kernel32.GlobalLock(handle)
    if not pointer:
        _kernel32.GlobalFree(handle)
        raise OSError(ctypes.get_last_error(), "GlobalLock failed")
    try:
        ctypes.memmove(pointer, data, len(data))
    finally:
        _kernel32.GlobalUnlock(handle)
    if not _user32.SetClipboardData(fmt, handle):
        _kernel32.GlobalFree(handle)
        raise OSError(ctypes.get_last_error(), "SetClipboardData failed")


def write_payload(text):
    """Put `text` on the clipboard under the registered format and as text."""

    def action():
        if not _user32.EmptyClipboard():
            raise OSError(ctypes.get_last_error(), "EmptyClipboard failed")
        _set_format(_payload_format(), text)
        _set_format(CF_UNICODETEXT, text)

    _with_clipboard(action)


def clear():
    """Empty the clipboard."""

    def action():
        if not _user32.EmptyClipboard():
            raise OSError(ctypes.get_last_error(), "EmptyClipboard failed")

    _with_clipboard(action)
