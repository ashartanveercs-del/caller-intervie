"""Versioned sidecar protocol envelope models."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import Enum
import re
from types import MappingProxyType
from typing import Any


PROTOCOL_VERSION = 1
MAX_PROTOCOL_VERSION = 65_535
MAX_SAFE_INTEGER = 9_007_199_254_740_991

_UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)
_NAMESPACED_KIND_RE = re.compile(r"^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+$")
_ENVELOPE_FIELDS = {
    "version",
    "id",
    "session_id",
    "sequence",
    "timestamp_ms",
    "kind",
    "payload",
    "correlation_id",
}


class CommandKind(str, Enum):
    HANDSHAKE_REQUEST = "handshake.request"
    SESSION_START = "session.start"
    SESSION_STOP = "session.stop"
    LISTENING_SET = "listening.set"
    YOU_SOURCE_SET = "you_source.set"
    QUERY_TRIGGER = "query.trigger"
    AUDIO_SYSTEM_SET = "audio.system.set"
    AUDIO_DEVICE_SET = "audio.device.set"
    KNOWLEDGE_INGEST = "knowledge.ingest"
    SESSION_SNAPSHOT_REQUEST = "session.snapshot.request"


class EventKind(str, Enum):
    SIDECAR_READY = "sidecar.ready"
    SESSION_STATE = "session.state"
    TRANSCRIPT_UPDATED = "transcript.updated"
    SUGGESTION_CHUNK = "suggestion.chunk"
    SUGGESTION_STARTED = "suggestion.started"
    SUGGESTION_COMPLETED = "suggestion.completed"
    SUGGESTION_ERROR = "suggestion.error"
    AUDIO_HEALTH = "audio.health"
    PROVIDER_HEALTH = "provider.health"
    KNOWLEDGE_STATE = "knowledge.state"
    RAG_STATUS = "rag.status"
    RUNTIME_ERROR = "runtime.error"


Kind = CommandKind | EventKind


def _require_uuid(value: object, field: str) -> str:
    if not isinstance(value, str) or not _UUID_RE.fullmatch(value):
        raise ValueError(f"{field} must be a UUID string")
    return value


def _require_nonnegative_integer(value: object, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError(f"{field} must be a nonnegative integer")
    if value > MAX_SAFE_INTEGER:
        raise ValueError(f"{field} must be a safe integer")
    return value


def _require_protocol_version(value: object) -> int:
    version = _require_nonnegative_integer(value, "version")
    if version > MAX_PROTOCOL_VERSION:
        raise ValueError("version must be a u16")
    return version


def _freeze_json(value: Any) -> Any:
    if isinstance(value, dict):
        return MappingProxyType({key: _freeze_json(item) for key, item in value.items()})
    if isinstance(value, list):
        return tuple(_freeze_json(item) for item in value)
    return value


def _thaw_json(value: Any) -> Any:
    if isinstance(value, Mapping):
        return {key: _thaw_json(item) for key, item in value.items()}
    if isinstance(value, tuple):
        return [_thaw_json(item) for item in value]
    return value


def _parse_kind(value: object) -> Kind:
    if not isinstance(value, str):
        raise ValueError("kind must be a known namespaced string")
    try:
        return CommandKind(value)
    except ValueError:
        try:
            return EventKind(value)
        except ValueError as error:
            raise ValueError("kind must be a known namespaced string") from error


@dataclass(frozen=True)
class WireEnvelope:
    """Structurally valid inbound envelope whose version or kind may be unsupported."""

    version: int
    id: str
    session_id: str | None
    sequence: int
    timestamp_ms: int
    kind: str
    payload: Mapping[str, Any]
    correlation_id: str | None

    @classmethod
    def from_dict(cls, value: object) -> "WireEnvelope":
        if not isinstance(value, dict):
            raise ValueError("envelope must be an object")

        fields = set(value)
        if fields != _ENVELOPE_FIELDS:
            missing = _ENVELOPE_FIELDS - fields
            unknown = fields - _ENVELOPE_FIELDS
            raise ValueError(f"invalid envelope fields: missing={missing}, unknown={unknown}")

        session_id = value["session_id"]
        if session_id is not None:
            session_id = _require_uuid(session_id, "session_id")

        correlation_id = value["correlation_id"]
        if correlation_id is not None:
            correlation_id = _require_uuid(correlation_id, "correlation_id")

        payload = value["payload"]
        if not isinstance(payload, dict):
            raise ValueError("payload must be an object")

        kind = value["kind"]
        if not isinstance(kind, str) or not _NAMESPACED_KIND_RE.fullmatch(kind):
            raise ValueError("kind must be a namespaced string")

        return cls(
            version=_require_protocol_version(value["version"]),
            id=_require_uuid(value["id"], "id"),
            session_id=session_id,
            sequence=_require_nonnegative_integer(value["sequence"], "sequence"),
            timestamp_ms=_require_nonnegative_integer(value["timestamp_ms"], "timestamp_ms"),
            kind=kind,
            payload=_freeze_json(payload),
            correlation_id=correlation_id,
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "version": self.version,
            "id": self.id,
            "session_id": self.session_id,
            "sequence": self.sequence,
            "timestamp_ms": self.timestamp_ms,
            "kind": self.kind,
            "payload": _thaw_json(self.payload),
            "correlation_id": self.correlation_id,
        }


@dataclass(frozen=True)
class Envelope:
    version: int
    id: str
    session_id: str | None
    sequence: int
    timestamp_ms: int
    kind: Kind
    payload: Mapping[str, Any]
    correlation_id: str | None

    @classmethod
    def from_dict(cls, value: object) -> "Envelope":
        wire = WireEnvelope.from_dict(value)
        if wire.version != PROTOCOL_VERSION:
            raise ValueError(f"unsupported protocol version: {wire.version}")

        return cls(
            version=wire.version,
            id=wire.id,
            session_id=wire.session_id,
            sequence=wire.sequence,
            timestamp_ms=wire.timestamp_ms,
            kind=_parse_kind(wire.kind),
            payload=wire.payload,
            correlation_id=wire.correlation_id,
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "version": self.version,
            "id": self.id,
            "session_id": self.session_id,
            "sequence": self.sequence,
            "timestamp_ms": self.timestamp_ms,
            "kind": self.kind.value,
            "payload": _thaw_json(self.payload),
            "correlation_id": self.correlation_id,
        }
