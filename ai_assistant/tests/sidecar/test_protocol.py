import json
from pathlib import Path

import pytest

from ai_assistant.sidecar.protocol import (
    MAX_SAFE_INTEGER,
    CommandKind,
    Envelope,
    EventKind,
)


FIXTURES = Path(__file__).resolve().parents[3] / "protocol" / "v1" / "fixtures"
SCHEMA = FIXTURES.parent / "envelope.schema.json"


def _load(name: str) -> dict[str, object]:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def test_schema_requires_protocol_version_one():
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    assert schema["properties"]["version"]["const"] == 1
    assert schema["properties"]["version"]["minimum"] == 0
    assert schema["properties"]["version"]["maximum"] == 65535


def test_session_start_fixture_round_trips():
    raw = _load("session-start.json")
    envelope = Envelope.from_dict(raw)
    assert envelope.version == 1
    assert envelope.kind == CommandKind.SESSION_START
    assert envelope.to_dict() == raw


def test_event_fixtures_round_trip():
    for name, expected_kind in (
        ("sidecar-ready.json", EventKind.SIDECAR_READY),
        ("transcript-final.json", EventKind.TRANSCRIPT_UPDATED),
        ("suggestion-complete.json", EventKind.SUGGESTION_COMPLETED),
    ):
        raw = _load(name)
        envelope = Envelope.from_dict(raw)
        assert envelope.kind == expected_kind
        assert envelope.to_dict() == raw


def test_preserves_unknown_payload_fields():
    raw = _load("sidecar-ready.json")
    raw["payload"] = {"status": "ready", "future_extension": {"enabled": True}}
    assert Envelope.from_dict(raw).to_dict() == raw


def test_payload_state_is_deeply_immutable_and_serialization_is_detached():
    raw = _load("sidecar-ready.json")
    raw["payload"] = {
        "status": "ready",
        "future_extension": {"enabled": True},
        "items": [{"position": 0}],
    }
    envelope = Envelope.from_dict(raw)

    with pytest.raises(TypeError):
        envelope.payload["future_extension"] = {"enabled": False}
    with pytest.raises(TypeError):
        envelope.payload["future_extension"]["enabled"] = False
    with pytest.raises(AttributeError):
        envelope.payload["items"].append({"position": 1})
    with pytest.raises(TypeError):
        envelope.payload["items"][0]["position"] = 1

    serialized = envelope.to_dict()
    serialized["payload"]["future_extension"]["enabled"] = False
    serialized["payload"]["items"].append({"position": 1})

    assert envelope.to_dict() == raw


@pytest.mark.parametrize("field", ("sequence", "timestamp_ms"))
def test_accepts_safe_integer_boundary(field):
    raw = _load("sidecar-ready.json")
    raw[field] = MAX_SAFE_INTEGER
    assert Envelope.from_dict(raw).to_dict() == raw


@pytest.mark.parametrize("field", ("sequence", "timestamp_ms"))
def test_rejects_safe_integer_overflow(field):
    raw = _load("sidecar-ready.json")
    raw[field] = MAX_SAFE_INTEGER + 1
    with pytest.raises(ValueError, match="safe integer"):
        Envelope.from_dict(raw)


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("version", 2, "protocol version"),
        ("id", "not-a-uuid", "id"),
        ("sequence", -1, "sequence"),
        ("timestamp_ms", -1, "timestamp_ms"),
        ("kind", "not-namespaced", "kind"),
        ("payload", [], "payload"),
    ],
)
def test_rejects_invalid_envelope_fields(field, value, message):
    raw = _load("sidecar-ready.json")
    raw[field] = value
    with pytest.raises(ValueError, match=message):
        Envelope.from_dict(raw)
