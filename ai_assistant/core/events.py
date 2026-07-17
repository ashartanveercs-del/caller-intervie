"""Event bus and event types for inter-module communication."""

from __future__ import annotations

import asyncio
import logging
import time
from collections import defaultdict
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Awaitable, Callable, Optional

logger = logging.getLogger(__name__)

EventHandler = Callable[..., Awaitable[None]]


class EventType(str, Enum):
    TRANSCRIPT_UPDATE = "transcript_update"
    MODE_CHANGE = "mode_change"
    QUERY_TRIGGERED = "query_triggered"
    RESPONSE_CHUNK = "response_chunk"
    RESPONSE_COMPLETE = "response_complete"
    CONNECTION_STATE = "connection_state"
    ERROR = "error"


@dataclass
class TranscriptEvent:
    text: str
    is_final: bool
    speech_final: bool
    speaker: Optional[int] = None
    confidence: float = 0.0
    timestamp: float = field(default_factory=time.time)
    source: str = "mic"  # "mic" = you, "system" = interviewer


@dataclass
class ModeChangeEvent:
    old_mode: str
    new_mode: str


@dataclass
class ResponseChunkEvent:
    text: str
    request_id: str


@dataclass
class ResponseCompleteEvent:
    full_text: str
    request_id: str
    input_tokens: int
    output_tokens: int


class EventBus:
    """Async event bus with fire-and-forget dispatch and error isolation."""

    def __init__(self) -> None:
        self._handlers: dict[EventType, list[EventHandler]] = defaultdict(list)

    def on(self, event_type: EventType, handler: EventHandler) -> None:
        self._handlers[event_type].append(handler)

    def off(self, event_type: EventType, handler: EventHandler) -> None:
        self._handlers[event_type].remove(handler)

    async def emit(self, event_type: EventType, data: Any = None) -> None:
        for handler in self._handlers[event_type]:
            asyncio.create_task(self._safe_call(handler, data))

    async def _safe_call(self, handler: EventHandler, data: Any) -> None:
        try:
            await handler(data)
        except Exception:
            logger.exception("Event handler error")
