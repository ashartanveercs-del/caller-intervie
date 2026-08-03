"""Bounded MessagePack framing for sidecar protocol envelopes."""

from __future__ import annotations

import struct
from typing import BinaryIO

import msgpack

from .protocol import Envelope, WireEnvelope


MAX_FRAME_SIZE = 16 * 1024 * 1024
_LENGTH_PREFIX_SIZE = 4


class FrameError(ValueError):
    """Raised when a framed sidecar message cannot be safely decoded."""


def encode_frame(envelope: Envelope) -> bytes:
    """Return an Envelope as a length-prefixed MessagePack frame."""
    if not isinstance(envelope, Envelope):
        raise TypeError("envelope must be an Envelope")

    payload = msgpack.packb(envelope.to_dict(), use_bin_type=True)
    if len(payload) > MAX_FRAME_SIZE:
        raise FrameError(f"frame payload exceeds maximum of {MAX_FRAME_SIZE} bytes")
    return struct.pack(">I", len(payload)) + payload


def _read_exact(stream: BinaryIO, size: int, *, section: str, allow_clean_eof: bool) -> bytes | None:
    data = bytearray()
    while len(data) < size:
        chunk = stream.read(size - len(data))
        if chunk == b"":
            if not data and allow_clean_eof:
                return None
            raise FrameError(f"truncated frame {section}")
        if chunk is None:
            raise FrameError(f"frame {section} read made no progress")
        data.extend(chunk)
    return bytes(data)


def _read_frame_value(stream: BinaryIO) -> object | None:
    header = _read_exact(
        stream, _LENGTH_PREFIX_SIZE, section="header", allow_clean_eof=True
    )
    if header is None:
        return None

    payload_size = struct.unpack(">I", header)[0]
    if payload_size > MAX_FRAME_SIZE:
        raise FrameError(f"frame payload exceeds maximum of {MAX_FRAME_SIZE} bytes")

    payload = _read_exact(stream, payload_size, section="payload", allow_clean_eof=False)
    assert payload is not None

    try:
        value = msgpack.unpackb(payload, raw=False, strict_map_key=True)
    except (msgpack.ExtraData, msgpack.FormatError, msgpack.StackError, ValueError) as error:
        raise FrameError("invalid MessagePack payload") from error

    return value


def read_frame(stream: BinaryIO) -> Envelope | None:
    """Read one strict Protocol V1 envelope, returning ``None`` at clean EOF."""
    value = _read_frame_value(stream)
    if value is None:
        return None
    try:
        return Envelope.from_dict(value)
    except ValueError as error:
        raise FrameError(f"invalid envelope: {error}") from error


def read_command(stream: BinaryIO) -> Envelope | WireEnvelope | None:
    """Read a structurally safe command while retaining version/kind mismatch metadata."""
    value = _read_frame_value(stream)
    if value is None:
        return None
    try:
        return Envelope.from_dict(value)
    except ValueError:
        try:
            return WireEnvelope.from_dict(value)
        except ValueError as error:
            raise FrameError(f"invalid envelope: {error}") from error


def write_frame(stream: BinaryIO, envelope: Envelope) -> None:
    """Write a complete framed envelope to a binary stream."""
    remaining = memoryview(encode_frame(envelope))
    while remaining:
        written = stream.write(remaining)
        if written is None or written <= 0:
            raise FrameError("frame write made no progress")
        if written > len(remaining):
            raise FrameError("frame write exceeded remaining payload")
        remaining = remaining[written:]
    flush = getattr(stream, "flush", None)
    if flush is not None:
        flush()
