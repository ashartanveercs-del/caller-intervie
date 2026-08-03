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


class _FragmentedReadStream:
    def __init__(self, data: bytes) -> None:
        self._data = data
        self._offset = 0
        self._chunk_sizes = iter((1, 3, 2, 1, 2, 3) * 100)

    def read(self, size: int) -> bytes:
        chunk_size = min(size, next(self._chunk_sizes))
        chunk = self._data[self._offset : self._offset + chunk_size]
        self._offset += len(chunk)
        return chunk


def test_fragmented_reads_decode_a_complete_frame():
    envelope = _envelope("session-start.json")
    stream = _FragmentedReadStream(encode_frame(envelope))

    assert read_frame(stream) == envelope
    assert read_frame(stream) is None


def test_write_frame_writes_a_readable_envelope():
    envelope = _envelope("session-start.json")
    stream = BytesIO()

    write_frame(stream, envelope)

    stream.seek(0)
    assert read_frame(stream) == envelope


class _ThreeByteWriteStream(BytesIO):
    def write(self, data: bytes) -> int:
        return super().write(data[:3])


def test_write_frame_retries_short_writes_until_the_frame_is_complete():
    envelope = _envelope("session-start.json")
    stream = _ThreeByteWriteStream()

    write_frame(stream, envelope)

    assert stream.getvalue() == encode_frame(envelope)


class _ZeroProgressWriteStream:
    def write(self, data: bytes) -> int:
        return 0


def test_write_frame_rejects_zero_progress_writes():
    with pytest.raises(FrameError, match="progress"):
        write_frame(_ZeroProgressWriteStream(), _envelope("session-start.json"))


class _NoneProgressWriteStream:
    def write(self, data: bytes) -> None:
        return None


def test_write_frame_rejects_non_blocking_no_progress_writes():
    with pytest.raises(FrameError, match="progress"):
        write_frame(_NoneProgressWriteStream(), _envelope("session-start.json"))


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
