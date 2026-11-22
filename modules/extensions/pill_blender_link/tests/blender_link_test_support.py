# REQUIREMENTS
#   Run by the two link smokes (by Python, not Blender). The repository's
#   devops packages must be importable, which each smoke arranges before it
#   imports this module.
#
# DESCRIPTION
#   The parts both link smokes share: the frame codec, the assertion helper,
#   the fake Blender that speaks the protocol's half of the socket, and the
#   entity rows read back from the editor's `dump entities`.
#
# USAGE
#   from blender_link_test_support import FakeBlender, check, encode_frame, entity_rows
#
# EXAMPLE USAGE
#   See `blender_link_material_smoke.py` and `blender_link_create_smoke.py`.
#
# --- SCRIPT ---
import json
import re
import socket
import struct
import time

from core import editor_log_tokens as tokens
from core import log_layout


def check(condition, description):
    """Prints and records one passing assertion; raises on a failure."""
    if not condition:
        raise AssertionError(description)
    print(f"[smoke] ok {description}")


def encode_frame(header, body=b""):
    """One protocol frame: header length, body length, JSON header, body."""
    header_bytes = json.dumps(header).encode("utf-8")
    return struct.pack("<II", len(header_bytes), len(body)) + header_bytes + body


class FakeBlender:
    """Blender's end of the protocol: greets the editor and reads its frames."""

    def __init__(self, port):
        self._socket = socket.create_connection(("127.0.0.1", port), timeout=10.0)
        self._socket.settimeout(1.0)
        self._buffer = bytearray()
        self.frames = []
        self._socket.sendall(
            encode_frame(
                {"type": "hello", "protocol": 1, "blender_version": "smoke-test"}
            )
        )

    def send(self, header, body=b""):
        """Sends one frame from Blender's side."""
        self._socket.sendall(encode_frame(header, body))

    def read_frames(self, timeout_seconds):
        """Reads frames until the timeout passes with nothing new."""
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            try:
                chunk = self._socket.recv(1 << 16)
            except socket.timeout:
                continue
            if not chunk:
                break
            self._buffer.extend(chunk)
            self._cut_frames()
        return self.frames

    def _cut_frames(self):
        while len(self._buffer) >= 8:
            header_length, body_length = struct.unpack_from("<II", self._buffer, 0)
            frame_length = 8 + header_length + body_length
            if len(self._buffer) < frame_length:
                return
            header = json.loads(bytes(self._buffer[8 : 8 + header_length]).decode("utf-8"))
            body = bytes(self._buffer[8 + header_length : frame_length])
            del self._buffer[:frame_length]
            self.frames.append((header, body))

    def frame_of_type(self, message_type):
        """Every frame of one type, in arrival order."""
        return [
            (header, body)
            for header, body in self.frames
            if header.get("type") == message_type
        ]

    def close(self):
        self._socket.close()


def entity_rows(session):
    """The `dump entities` rows as (display, components) pairs, in order."""
    dump = session.request(
        "dump entities", re.escape(tokens.ENTITIES_DUMP_TOKEN), 30, "the entity list"
    )
    row_pattern = log_layout.message_then_fields_pattern(
        re.escape(tokens.ENTITY_ROW_TOKEN),
        r'\s+display="([^"]+)"\s+components="([^"]*)"',
    )
    return [(match.group(1), match.group(2)) for match in re.finditer(row_pattern, dump)]
