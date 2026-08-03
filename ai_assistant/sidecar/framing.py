"""Bounded MessagePack framing for sidecar protocol envelopes."""

from __future__ import annotations

import struct
from typing import BinaryIO

import msgpack

from .protocol import Envelope


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


def read_frame(stream: BinaryIO) -> Envelope | None:
    """Read one frame, returning ``None`` only for a clean end of stream."""
    header = stream.read(_LENGTH_PREFIX_SIZE)
    if header == b"":
        return None
    if len(header) != _LENGTH_PREFIX_SIZE:
        raise FrameError("truncated frame header")

    payload_size = struct.unpack(">I", header)[0]
    if payload_size > MAX_FRAME_SIZE:
        raise FrameError(f"frame payload exceeds maximum of {MAX_FRAME_SIZE} bytes")

    payload = stream.read(payload_size)
    if len(payload) != payload_size:
        raise FrameError("truncated frame payload")

    try:
        value = msgpack.unpackb(payload, raw=False, strict_map_key=True)
    except (msgpack.ExtraData, msgpack.FormatError, msgpack.StackError, ValueError) as error:
        raise FrameError("invalid MessagePack payload") from error

    try:
        return Envelope.from_dict(value)
    except ValueError as error:
        raise FrameError(f"invalid envelope: {error}") from error


def write_frame(stream: BinaryIO, envelope: Envelope) -> None:
    """Write a complete framed envelope to a binary stream."""
    stream.write(encode_frame(envelope))
    flush = getattr(stream, "flush", None)
    if flush is not None:
        flush()
