"""Protocol V1 adapter around the UI-independent runtime service."""

from __future__ import annotations

import inspect
import itertools
import time
from collections.abc import Awaitable, Callable, Mapping
from typing import Any
from uuid import uuid4

from ai_assistant.core.events import (
    EventType,
    ResponseChunkEvent,
    ResponseCompleteEvent,
    TranscriptEvent,
)
from ai_assistant.runtime import SessionConfig

from .protocol import CommandKind, Envelope, EventKind, PROTOCOL_VERSION, WireEnvelope


EventOutput = Callable[[Envelope], Awaitable[None] | None]
_sequence_numbers = itertools.count()


class _InvalidCommand(ValueError):
    pass


class RuntimeProtocolAdapter:
    """Translate Protocol V1 commands and runtime events without coupling the runtime to UI code."""

    def __init__(self, runtime, output: EventOutput) -> None:
        self._runtime = runtime
        self._output = output
        self._turn_ids: dict[str, str] = {}
        self._suggestion_ids: dict[str, str] = {}
        self._response_correlations: dict[str, str | None] = {}
        self._pending_response_correlation_id: str | None = None
        runtime.event_bus.on(EventType.TRANSCRIPT_UPDATE, self.on_transcript)
        runtime.event_bus.on(EventType.RESPONSE_CHUNK, self.on_response_chunk)
        runtime.event_bus.on(EventType.RESPONSE_COMPLETE, self.on_response_complete)
        runtime.event_bus.on(EventType.ERROR, self.on_runtime_error)

    async def handle(self, command: Envelope | WireEnvelope) -> None:
        """Execute one command and emit its protocol result directly to the transport."""
        correlation_id = command.id
        if command.version != PROTOCOL_VERSION:
            await self._error(
                "unsupported_version",
                "Unsupported protocol version",
                source="adapter",
                correlation_id=correlation_id,
            )
            return
        if not isinstance(command.kind, CommandKind):
            await self._error(
                "unknown_command",
                "Unsupported command kind",
                source="adapter",
                correlation_id=correlation_id,
            )
            return

        try:
            await self._handle_command(command)
        except _InvalidCommand as error:
            await self._error(
                "invalid_command",
                str(error),
                source="adapter",
                correlation_id=correlation_id,
            )
        except Exception:
            await self._error(
                "runtime_error",
                "Runtime command failed",
                source="runtime",
                correlation_id=correlation_id,
            )

    async def emit_ready(self, correlation_id: str | None = None) -> None:
        await self._emit(
            EventKind.SIDECAR_READY,
            {"status": "ready"},
            correlation_id=correlation_id,
        )

    async def on_transcript(self, event: TranscriptEvent) -> None:
        timestamp_ms = int(event.timestamp * 1000)
        snapshot = self._runtime.snapshot()
        turn_id = event.turn_id or self._turn_ids.get(event.source) or str(uuid4())
        self._turn_ids[event.source] = turn_id
        envelope = await self._emit(
            EventKind.TRANSCRIPT_UPDATED,
            {
                "turn_id": turn_id,
                "text": event.text,
                "is_final": event.is_final,
                "speech_final": event.speech_final,
                "speaker_role": (
                    "interviewee" if event.source == snapshot.you_source else "interviewer"
                ),
                "source": event.source,
                "language": event.language or "und",
                "confidence": event.confidence,
                "started_at_ms": event.started_at_ms if event.started_at_ms is not None else timestamp_ms,
                "ended_at_ms": event.ended_at_ms if event.ended_at_ms is not None else timestamp_ms,
            },
            session_id=snapshot.session_id,
        )
        if event.is_final:
            self._pending_response_correlation_id = envelope.id
        if event.speech_final:
            self._turn_ids.pop(event.source, None)

    async def on_response_chunk(self, event: ResponseChunkEvent) -> None:
        suggestion_id, correlation_id = self._response_identity(event.request_id)
        await self._emit(
            EventKind.SUGGESTION_CHUNK,
            {"suggestion_id": suggestion_id, "text": event.text},
            session_id=self._runtime.snapshot().session_id,
            correlation_id=correlation_id,
        )

    async def on_response_complete(self, event: ResponseCompleteEvent) -> None:
        suggestion_id, correlation_id = self._response_identity(event.request_id)
        await self._emit(
            EventKind.SUGGESTION_COMPLETED,
            {"suggestion_id": suggestion_id, "text": event.full_text},
            session_id=self._runtime.snapshot().session_id,
            correlation_id=correlation_id,
        )

    async def on_runtime_error(self, _event: object) -> None:
        await self._error(
            "runtime_error",
            "Runtime event failed",
            source="runtime",
            correlation_id=None,
        )

    async def _handle_command(self, command: Envelope) -> None:
        payload = command.payload
        correlation_id = command.id
        kind = command.kind

        if kind is CommandKind.HANDSHAKE_REQUEST:
            await self.emit_ready(correlation_id)
            return
        if kind is CommandKind.SESSION_START:
            await self._runtime.start_session(self._session_config(command))
            await self._emit_snapshot(correlation_id)
            return
        if kind is CommandKind.SESSION_STOP:
            await self._runtime.stop_session()
            await self._emit_snapshot(correlation_id)
            return
        if kind is CommandKind.LISTENING_SET:
            self._runtime.set_listening(self._bool(payload, "enabled"))
            await self._emit_snapshot(correlation_id)
            return
        if kind is CommandKind.YOU_SOURCE_SET:
            self._runtime.set_you_source(self._source(payload, "source"))
            await self._emit_snapshot(correlation_id)
            return
        if kind is CommandKind.QUERY_TRIGGER:
            await self._runtime.trigger_query(
                self._string(payload, "text"), self._string(payload, "answer_format")
            )
            self._pending_response_correlation_id = command.id
            await self._emit_snapshot(correlation_id)
            return
        if kind is CommandKind.AUDIO_SYSTEM_SET:
            await self._runtime.set_system_audio(self._bool(payload, "enabled"))
            await self._emit_snapshot(correlation_id)
            return
        if kind is CommandKind.AUDIO_DEVICE_SET:
            device = payload.get("device")
            if device is not None and (isinstance(device, bool) or not isinstance(device, int)):
                raise _InvalidCommand("device must be an integer or null")
            await self._runtime.set_audio_device(self._source(payload, "source"), device)
            await self._emit_snapshot(correlation_id)
            return
        if kind is CommandKind.KNOWLEDGE_INGEST:
            paths = payload.get("paths")
            if not isinstance(paths, (list, tuple)) or not all(isinstance(path, str) for path in paths):
                raise _InvalidCommand("paths must be an array of strings")
            result = await self._runtime.ingest_documents(list(paths))
            snapshot = self._runtime.snapshot()
            await self._emit(
                EventKind.KNOWLEDGE_STATE,
                {
                    "state": snapshot.knowledge_state,
                    "chunks": result.total_chunks,
                    "files": result.files,
                    "chunks_added": result.chunks_added,
                },
                session_id=snapshot.session_id,
                correlation_id=correlation_id,
            )
            return
        if kind is CommandKind.SESSION_SNAPSHOT_REQUEST:
            await self._emit_snapshot(correlation_id)
            return
        raise _InvalidCommand("Unsupported command kind")

    @staticmethod
    def _session_config(command: Envelope) -> SessionConfig:
        payload = command.payload
        if command.session_id is None:
            raise _InvalidCommand("session_id is required")
        return SessionConfig(
            session_id=command.session_id,
            mode=RuntimeProtocolAdapter._string(payload, "mode"),
            input_language=RuntimeProtocolAdapter._string(payload, "input_language"),
            response_language=RuntimeProtocolAdapter._string(payload, "response_language"),
            review_language=RuntimeProtocolAdapter._string(payload, "review_language"),
            you_source=RuntimeProtocolAdapter._source(payload, "you_source"),
            brief_id=RuntimeProtocolAdapter._string(payload, "brief_id"),
        )

    @staticmethod
    def _string(payload: Mapping[str, Any], key: str) -> str:
        value = payload.get(key)
        if not isinstance(value, str):
            raise _InvalidCommand(f"{key} must be a string")
        return value

    @staticmethod
    def _bool(payload: Mapping[str, Any], key: str) -> bool:
        value = payload.get(key)
        if not isinstance(value, bool):
            raise _InvalidCommand(f"{key} must be a boolean")
        return value

    @staticmethod
    def _source(payload: Mapping[str, Any], key: str) -> str:
        source = RuntimeProtocolAdapter._string(payload, key)
        if source not in ("mic", "system"):
            raise _InvalidCommand(f"{key} must be mic or system")
        return source

    async def _emit_snapshot(self, correlation_id: str | None) -> None:
        snapshot = self._runtime.snapshot()
        await self._emit(
            EventKind.SESSION_STATE,
            {
                "state": snapshot.state,
                "mode": snapshot.mode,
                "input_language": snapshot.input_language,
                "response_language": snapshot.response_language,
                "review_language": snapshot.review_language,
                "you_source": snapshot.you_source,
                "listening": snapshot.listening,
                "system_audio_enabled": snapshot.system_audio_enabled,
            },
            session_id=snapshot.session_id,
            correlation_id=correlation_id,
        )

    async def _error(
        self,
        code: str,
        message: str,
        *,
        source: str,
        correlation_id: str | None,
    ) -> None:
        await self._emit(
            EventKind.RUNTIME_ERROR,
            {"code": code, "message": message, "recoverable": True, "source": source},
            correlation_id=correlation_id,
        )

    async def _emit(
        self,
        kind: EventKind,
        payload: dict[str, Any],
        *,
        session_id: str | None = None,
        correlation_id: str | None = None,
    ) -> Envelope:
        event = Envelope(
            version=PROTOCOL_VERSION,
            id=str(uuid4()),
            session_id=session_id,
            sequence=next(_sequence_numbers),
            timestamp_ms=time.time_ns() // 1_000_000,
            kind=kind,
            payload=payload,
            correlation_id=correlation_id,
        )
        result = self._output(event)
        if inspect.isawaitable(result):
            await result
        return event

    def _response_identity(self, request_id: str) -> tuple[str, str | None]:
        suggestion_id = self._suggestion_ids.setdefault(request_id, str(uuid4()))
        correlation_id = self._response_correlations.setdefault(
            request_id, self._pending_response_correlation_id
        )
        return suggestion_id, correlation_id
