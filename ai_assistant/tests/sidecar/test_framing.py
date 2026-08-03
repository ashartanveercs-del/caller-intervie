from __future__ import annotations

from io import BytesIO
from pathlib import Path
import struct

import pytest

from ai_assistant.sidecar.framing import (
    FrameError,
    encode_frame,
    read_frame,
    write_frame,
)
from ai_assistant.sidecar.protocol import Envelope


FIXTURES = Path(__file__).resolve().parents[3] / "protocol" / "v1" / "fixtures"


def _envelope(name: str, *, sequence: int | None = None) -> Envelope:
    import json

    raw = json.loads((FIXTURES / name).read_text(encoding="utf-8"))
    if sequence is not None:
        raw["sequence"] = sequence
    return Envelope.from_dict(raw)


def test_two_frames_decode_without_bleeding():
    first = _envelope("session-start.json")
    second = _envelope("sidecar-ready.json", sequence=2)
    stream = BytesIO(encode_frame(first) + encode_frame(second))

    assert read_frame(stream) == first
    assert read_frame(stream) == second
    assert read_frame(stream) is None


def test_write_frame_writes_a_readable_envelope():
    envelope = _envelope("session-start.json")
    stream = BytesIO()

    write_frame(stream, envelope)

    stream.seek(0)
    assert read_frame(stream) == envelope


def test_truncated_header_is_rejected():
    with pytest.raises(FrameError, match="truncated.*header"):
        read_frame(BytesIO(b"\x00\x00\x00"))


def test_truncated_payload_is_rejected():
    with pytest.raises(FrameError, match="truncated.*payload"):
        read_frame(BytesIO(struct.pack(">I", 2) + b"\x81"))


def test_invalid_messagepack_is_rejected():
    with pytest.raises(FrameError, match="MessagePack"):
        read_frame(BytesIO(struct.pack(">I", 1) + b"\xc1"))


def test_oversized_frame_is_rejected():
    stream = BytesIO(struct.pack(">I", 16_777_217))

    with pytest.raises(FrameError, match="maximum"):
        read_frame(stream)


def test_encoding_an_oversized_envelope_is_rejected():
    envelope = _envelope("session-start.json")
    raw = envelope.to_dict()
    raw["payload"] = {"text": "x" * 16_777_216}

    with pytest.raises(FrameError, match="maximum"):
        encode_frame(Envelope.from_dict(raw))
