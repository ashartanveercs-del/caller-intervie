"""Behavioral tests for the RuntimeService Protocol V1 adapter."""

from __future__ import annotations

import asyncio
import functools
from io import BytesIO
import json
import struct
from dataclasses import dataclass, replace
from pathlib import Path
from uuid import UUID

import msgpack
import pytest

from ai_assistant.core.events import (
    ResponseChunkEvent,
    ResponseCompleteEvent,
    TranscriptEvent,
)
from ai_assistant.runtime import DocumentIngestionResult, RuntimeSnapshot
from ai_assistant.sidecar import __main__ as sidecar_main
from ai_assistant.sidecar.adapter import RuntimeProtocolAdapter
from ai_assistant.sidecar.protocol import CommandKind, Envelope, EventKind, PROTOCOL_VERSION
from ai_assistant.sidecar.framing import read_frame
from ai_assistant.sidecar.transport import SidecarTransport


FIXTURES = Path(__file__).resolve().parents[3] / "protocol" / "v1" / "fixtures"
SESSION_ID = "018f0000-0000-7000-8000-000000000003"
COMMAND_ID = "018f0000-0000-7000-8000-000000000002"
PARENT_CORRELATION_ID = "018f0000-0000-7000-8000-000000000099"


def load_fixture(name: str) -> Envelope:
    return Envelope.from_dict(json.loads((FIXTURES / name).read_text(encoding="utf-8")))


def _async_test(function):
    @functools.wraps(function)
    def run(*args, **kwargs):
        return asyncio.run(function(*args, **kwargs))

    return run


def command(kind: CommandKind, payload: dict, *, correlation_id: str | None = None) -> Envelope:
    return Envelope(
        version=PROTOCOL_VERSION,
        id=COMMAND_ID,
        session_id=SESSION_ID,
        sequence=1,
        timestamp_ms=1,
        kind=kind,
        payload=payload,
        correlation_id=correlation_id,
    )


def framed_wire_command(*, version: int = PROTOCOL_VERSION, kind: str = "session.stop") -> bytes:
    raw = load_fixture("session-start.json").to_dict()
    raw["version"] = version
    raw["kind"] = kind
    payload = msgpack.packb(raw, use_bin_type=True)
    return struct.pack(">I", len(payload)) + payload


class CollectingOutput:
    def __init__(self) -> None:
        self.events: list[Envelope] = []

    async def write(self, event: Envelope) -> None:
        self.events.append(event)

    @property
    def last(self) -> Envelope:
        return self.events[-1]


@dataclass
class FakeEventBus:
    handlers: dict = None

    def __post_init__(self) -> None:
        self.handlers = {}

    def on(self, event_type, handler) -> None:
        self.handlers[event_type] = handler


class FakeRuntimeService:
    def __init__(self) -> None:
        self.event_bus = FakeEventBus()
        self.started_with = None
        self.listening = None
        self.you_source = None
        self.system_audio = None
        self.audio_device = None
        self.query = None
        self.paths = None
        self.stopped = False
        self.error: Exception | None = None
        self._snapshot = RuntimeSnapshot(
            state="listening",
            session_id=SESSION_ID,
            mode="interview",
            input_language="auto",
            response_language="en",
            review_language="en",
            you_source="mic",
            listening=True,
            system_audio_enabled=False,
            knowledge_state="ready",
            knowledge_chunks=3,
        )

    async def start_session(self, config) -> None:
        self._raise_if_needed()
        self.started_with = config

    async def stop_session(self) -> None:
        if self.error is not None:
            self._snapshot = replace(self._snapshot, state="error", listening=False)
            raise self.error
        self.stopped = True
        self._snapshot = replace(self._snapshot, state="stopped", listening=False)

    def set_listening(self, enabled: bool) -> None:
        self._raise_if_needed()
        self.listening = enabled

    def set_you_source(self, source: str) -> None:
        self._raise_if_needed()
        self.you_source = source

    async def set_system_audio(self, enabled: bool) -> None:
        self._raise_if_needed()
        self.system_audio = enabled

    async def set_audio_device(self, source: str, device: int | None) -> None:
        self._raise_if_needed()
        self.audio_device = (source, device)

    async def trigger_query(
        self,
        text: str,
        answer_format: str,
        correlation_id: str | None = None,
    ) -> None:
        self._raise_if_needed()
        self.query = (text, answer_format, correlation_id)

    async def ingest_documents(self, paths: list[str]) -> DocumentIngestionResult:
        self._raise_if_needed()
        self.paths = paths
        return DocumentIngestionResult(files=len(paths), chunks_added=4, total_chunks=7)

    def snapshot(self) -> RuntimeSnapshot:
        return self._snapshot

    def _raise_if_needed(self) -> None:
        if self.error is not None:
            raise self.error


class FailedStartRuntime(FakeRuntimeService):
    async def start_session(self, config) -> None:
        self.started_with = config
        self._snapshot = replace(
            self._snapshot,
            state="error",
            session_id=config.session_id,
            mode=config.mode,
            listening=False,
        )
        raise RuntimeError("secret startup detail")


@_async_test
async def test_start_command_maps_all_language_fields_and_correlates_state() -> None:
    runtime = FakeRuntimeService()
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)
    start = load_fixture("session-start.json")

    await adapter.handle(start)

    assert runtime.started_with.session_id == SESSION_ID
    assert runtime.started_with.mode == "interview"
    assert runtime.started_with.input_language == "auto"
    assert runtime.started_with.response_language == "en"
    assert runtime.started_with.review_language == "en"
    assert runtime.started_with.you_source == "mic"
    assert runtime.started_with.brief_id == "018f0000-0000-7000-8000-000000000010"
    assert output.last.kind == EventKind.SESSION_STATE
    assert output.last.correlation_id == start.id
    assert output.last.payload["state"] == "listening"


@_async_test
async def test_start_failure_emits_error_state_before_sanitized_runtime_error() -> None:
    runtime = FailedStartRuntime()
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)
    start = load_fixture("session-start.json")

    await adapter.handle(start)

    assert [event.kind for event in output.events] == [
        EventKind.SESSION_STATE,
        EventKind.RUNTIME_ERROR,
    ]
    state, error = output.events
    assert state.session_id == SESSION_ID
    assert state.correlation_id == start.id
    assert state.payload["state"] == "error"
    assert state.payload["listening"] is False
    assert error.correlation_id == start.id
    assert error.payload == {
        "code": "runtime_error",
        "message": "Runtime command failed",
        "recoverable": True,
        "source": "runtime",
    }


@pytest.mark.parametrize(
    ("kind", "payload", "attribute", "expected"),
    [
        (CommandKind.LISTENING_SET, {"enabled": False}, "listening", False),
        (CommandKind.YOU_SOURCE_SET, {"source": "system"}, "you_source", "system"),
        (CommandKind.AUDIO_SYSTEM_SET, {"enabled": True}, "system_audio", True),
        (CommandKind.AUDIO_DEVICE_SET, {"source": "mic", "device": 7}, "audio_device", ("mic", 7)),
        (CommandKind.QUERY_TRIGGER, {"text": "Summarize", "answer_format": "summary"}, "query", ("Summarize", "summary", COMMAND_ID)),
    ],
)
@_async_test
async def test_runtime_control_commands_map_payloads(
    kind: CommandKind, payload: dict, attribute: str, expected: object
) -> None:
    runtime = FakeRuntimeService()
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)

    await adapter.handle(command(kind, payload))

    assert getattr(runtime, attribute) == expected
    assert output.last.kind == EventKind.SESSION_STATE


@_async_test
async def test_handshake_stop_snapshot_and_knowledge_commands_emit_protocol_state() -> None:
    runtime = FakeRuntimeService()
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)

    await adapter.handle(command(CommandKind.HANDSHAKE_REQUEST, {}))
    await adapter.handle(command(CommandKind.KNOWLEDGE_INGEST, {"paths": ["resume.pdf", "role.txt"]}))
    await adapter.handle(command(CommandKind.SESSION_SNAPSHOT_REQUEST, {}))
    await adapter.handle(command(CommandKind.SESSION_STOP, {}))

    assert output.events[0].kind == EventKind.SIDECAR_READY
    assert output.events[0].payload == {"status": "ready"}
    assert output.events[0].correlation_id == COMMAND_ID
    assert runtime.paths == ["resume.pdf", "role.txt"]
    assert output.events[1].kind == EventKind.KNOWLEDGE_STATE
    assert output.events[1].payload == {"state": "ready", "chunks": 7, "files": 2, "chunks_added": 4}
    assert output.events[2].kind == EventKind.SESSION_STATE
    assert runtime.stopped is True
    assert output.events[3].kind == EventKind.SESSION_STATE
    assert output.events[3].payload["state"] == "stopped"
    assert output.events[3].correlation_id == COMMAND_ID


@_async_test
async def test_stop_failure_emits_error_state_before_sanitized_runtime_error() -> None:
    runtime = FakeRuntimeService()
    runtime.error = RuntimeError("secret shutdown detail")
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)
    failure = command(CommandKind.SESSION_STOP, {})

    await adapter.handle(failure)

    assert [event.kind for event in output.events] == [
        EventKind.SESSION_STATE,
        EventKind.RUNTIME_ERROR,
    ]
    state, error = output.events
    assert state.session_id == SESSION_ID
    assert state.correlation_id == failure.id
    assert state.payload["state"] == "error"
    assert state.payload["listening"] is False
    assert error.correlation_id == failure.id
    assert error.payload == {
        "code": "runtime_error",
        "message": "Runtime command failed",
        "recoverable": True,
        "source": "runtime",
    }


@_async_test
async def test_final_transcript_keeps_original_language_and_timing() -> None:
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(FakeRuntimeService(), output.write)

    await adapter.on_transcript(
        TranscriptEvent(
            text="Bonjour",
            is_final=True,
            speech_final=True,
            language="fr",
            started_at_ms=100,
            ended_at_ms=200,
            source="mic",
        )
    )

    assert output.last.kind == EventKind.TRANSCRIPT_UPDATED
    assert output.last.payload["text"] == "Bonjour"
    assert output.last.payload["language"] == "fr"
    assert output.last.payload["started_at_ms"] == 100
    assert output.last.payload["ended_at_ms"] == 200
    assert output.last.payload["speaker_role"] == "interviewee"


@_async_test
async def test_response_events_keep_request_as_correlation_id() -> None:
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(FakeRuntimeService(), output.write)

    await adapter.on_transcript(
        TranscriptEvent(text="Question", is_final=True, speech_final=True, source="system")
    )
    transcript = output.last
    await adapter.on_response_chunk(
        ResponseChunkEvent(
            text="First",
            request_id="request-1",
            correlation_id=transcript.id,
        )
    )
    await adapter.on_response_complete(
        ResponseCompleteEvent(
            full_text="First answer",
            request_id="request-1",
            input_tokens=12,
            output_tokens=8,
            correlation_id=transcript.id,
        )
    )

    chunk, completed = output.events[-2:]
    assert chunk.kind == EventKind.SUGGESTION_CHUNK
    assert completed.kind == EventKind.SUGGESTION_COMPLETED
    assert chunk.payload["text"] == "First"
    assert completed.payload["text"] == "First answer"
    assert chunk.payload["suggestion_id"] == completed.payload["suggestion_id"]
    assert chunk.payload["suggestion_id"] != "request-1"
    assert str(UUID(chunk.payload["suggestion_id"])) == chunk.payload["suggestion_id"]
    assert chunk.correlation_id == transcript.id
    assert completed.correlation_id == transcript.id


@_async_test
async def test_invalid_command_and_runtime_exception_emit_sanitized_errors() -> None:
    runtime = FakeRuntimeService()
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)
    invalid = command(CommandKind.LISTENING_SET, {"enabled": "no"})

    await adapter.handle(invalid)
    runtime.error = RuntimeError("secret prompt: do not disclose")
    failure = command(CommandKind.SESSION_STOP, {}, correlation_id=PARENT_CORRELATION_ID)
    await adapter.handle(failure)

    assert output.events[0].kind == EventKind.RUNTIME_ERROR
    assert output.events[0].payload == {
        "code": "invalid_command",
        "message": "enabled must be a boolean",
        "recoverable": True,
        "source": "adapter",
    }
    assert output.events[0].correlation_id == invalid.id
    assert output.events[1].kind == EventKind.SESSION_STATE
    assert output.events[1].payload["state"] == "error"
    assert output.events[2].kind == EventKind.RUNTIME_ERROR
    assert output.events[2].payload["code"] == "runtime_error"
    assert output.events[2].payload["recoverable"] is True
    assert output.events[2].payload["source"] == "runtime"
    assert "secret prompt" not in output.events[2].payload["message"]
    assert output.events[2].correlation_id == failure.id


@_async_test
async def test_unknown_kind_and_wrong_version_do_not_touch_runtime() -> None:
    runtime = FakeRuntimeService()
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)
    unknown = command(EventKind.SIDECAR_READY, {})
    unsupported = command(CommandKind.SESSION_START, load_fixture("session-start.json").payload)
    object.__setattr__(unsupported, "version", 2)

    await adapter.handle(unknown)
    await adapter.handle(unsupported)

    assert runtime.started_with is None
    assert [event.payload["code"] for event in output.events] == ["unknown_command", "unsupported_version"]


@_async_test
async def test_immediate_command_result_uses_command_id_not_parent_correlation() -> None:
    runtime = FakeRuntimeService()
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)
    received = command(
        CommandKind.LISTENING_SET,
        {"enabled": False},
        correlation_id=PARENT_CORRELATION_ID,
    )

    await adapter.handle(received)

    assert output.last.kind == EventKind.SESSION_STATE
    assert output.last.correlation_id == received.id


@_async_test
async def test_framed_wrong_version_reaches_adapter_as_sanitized_error_without_runtime_mutation() -> None:
    runtime = FakeRuntimeService()
    transport_output = BytesIO()
    transport = SidecarTransport(BytesIO(framed_wire_command(version=2)), transport_output)
    adapter = RuntimeProtocolAdapter(runtime, transport.send)

    await transport.run(adapter.handle)

    transport_output.seek(0)
    error = read_frame(transport_output)
    assert error is not None
    assert error.kind == EventKind.RUNTIME_ERROR
    assert error.payload["code"] == "unsupported_version"
    assert error.correlation_id == COMMAND_ID
    assert runtime.started_with is None
    assert runtime.stopped is False


@_async_test
async def test_framed_unknown_kind_reaches_adapter_as_sanitized_error_without_runtime_mutation() -> None:
    runtime = FakeRuntimeService()
    transport_output = BytesIO()
    transport = SidecarTransport(
        BytesIO(framed_wire_command(kind="future.command")), transport_output
    )
    adapter = RuntimeProtocolAdapter(runtime, transport.send)

    await transport.run(adapter.handle)

    transport_output.seek(0)
    error = read_frame(transport_output)
    assert error is not None
    assert error.kind == EventKind.RUNTIME_ERROR
    assert error.payload["code"] == "unknown_command"
    assert error.correlation_id == COMMAND_ID
    assert runtime.started_with is None
    assert runtime.stopped is False


@_async_test
async def test_transcript_updates_share_turn_id_until_the_utterance_completes() -> None:
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(FakeRuntimeService(), output.write)

    await adapter.on_transcript(
        TranscriptEvent(text="Hello", is_final=False, speech_final=False, source="mic")
    )
    await adapter.on_transcript(
        TranscriptEvent(text="Hello there", is_final=True, speech_final=True, source="mic")
    )
    await adapter.on_transcript(
        TranscriptEvent(text="Next", is_final=False, speech_final=False, source="mic")
    )

    interim, final, next_turn = output.events
    assert interim.payload["turn_id"] == final.payload["turn_id"]
    assert next_turn.payload["turn_id"] != final.payload["turn_id"]


@_async_test
async def test_request_correlation_is_bound_to_its_triggering_transcript() -> None:
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(FakeRuntimeService(), output.write)
    triggering_id = "018f0000-0000-7000-8000-000000000040"
    later_id = "018f0000-0000-7000-8000-000000000041"

    await adapter.on_transcript(
        TranscriptEvent(
            text="First question",
            is_final=True,
            speech_final=True,
            source="system",
            event_id=triggering_id,
        )
    )
    assert output.last.id == triggering_id
    await adapter.on_transcript(
        TranscriptEvent(
            text="Later non-trigger",
            is_final=True,
            speech_final=True,
            source="mic",
            event_id=later_id,
        )
    )
    await adapter.on_response_chunk(
        ResponseChunkEvent(
            text="Answer",
            request_id="request-1",
            correlation_id=triggering_id,
        )
    )

    assert output.events[-1].correlation_id == triggering_id


@_async_test
async def test_speech_final_advances_turn_before_a_blocked_output_write() -> None:
    class BlockingOutput(CollectingOutput):
        def __init__(self) -> None:
            super().__init__()
            self.final_write_started = asyncio.Event()
            self.release_final_write = asyncio.Event()

        async def write(self, event: Envelope) -> None:
            if event.payload.get("speech_final"):
                self.final_write_started.set()
                await self.release_final_write.wait()
            self.events.append(event)

    output = BlockingOutput()
    adapter = RuntimeProtocolAdapter(FakeRuntimeService(), output.write)

    await adapter.on_transcript(
        TranscriptEvent(text="First", is_final=False, speech_final=False, source="mic")
    )
    final_task = asyncio.create_task(
        adapter.on_transcript(
            TranscriptEvent(text="First complete", is_final=True, speech_final=True, source="mic")
        )
    )
    await output.final_write_started.wait()
    await adapter.on_transcript(
        TranscriptEvent(text="Second", is_final=False, speech_final=False, source="mic")
    )
    output.release_final_write.set()
    await final_task

    assert output.events[1].payload["turn_id"] != output.events[0].payload["turn_id"]


@_async_test
async def test_emitted_sequences_increase_for_each_adapter_event() -> None:
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(FakeRuntimeService(), output.write)

    await adapter.on_response_chunk(ResponseChunkEvent(text="one", request_id="request-1"))
    await adapter.on_response_chunk(ResponseChunkEvent(text="two", request_id="request-2"))

    assert output.events[1].sequence > output.events[0].sequence


def test_transcript_event_accepts_optional_protocol_metadata_without_changing_defaults() -> None:
    event = TranscriptEvent(text="Hello", is_final=False, speech_final=False)

    assert event.language is None
    assert event.started_at_ms is None
    assert event.ended_at_ms is None
    assert event.source == "mic"


@_async_test
async def test_sidecar_entrypoint_builds_runtime_and_routes_transport_commands(monkeypatch) -> None:
    runtime = FakeRuntimeService()
    sent: list[Envelope] = []

    class FakeTransport:
        def __init__(self, *_streams) -> None:
            pass

        async def send(self, event: Envelope) -> None:
            sent.append(event)

        async def run(self, handler) -> None:
            await handler(command(CommandKind.HANDSHAKE_REQUEST, {}))

    monkeypatch.setattr(sidecar_main, "SidecarTransport", FakeTransport)
    monkeypatch.setattr(sidecar_main, "build_runtime", lambda config: runtime)

    await sidecar_main._run_transport()

    assert [event.kind for event in sent] == [EventKind.SIDECAR_READY, EventKind.SIDECAR_READY]
