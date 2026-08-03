"""Focused causality tests for streamed LLM response events."""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

from ai_assistant.config import Config
from ai_assistant.core.events import EventBus, EventType
from ai_assistant.llm import anthropic_client
from ai_assistant.llm.anthropic_client import AnthropicLLM


class _FakeStream:
    @property
    def text_stream(self):
        async def text():
            yield "First"
            yield " answer"

        return text()

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args) -> None:
        return None

    async def get_final_message(self):
        return SimpleNamespace(usage=SimpleNamespace(input_tokens=3, output_tokens=2))


class _FakeMessages:
    def stream(self, **_kwargs):
        return _FakeStream()


class _FakeClient:
    messages = _FakeMessages()


def test_streamed_events_preserve_request_correlation(monkeypatch) -> None:
    async def scenario():
        event_bus = EventBus()
        chunks = []
        completed = []

        async def on_chunk(event) -> None:
            chunks.append(event)

        async def on_complete(event) -> None:
            completed.append(event)

        event_bus.on(EventType.RESPONSE_CHUNK, on_chunk)
        event_bus.on(EventType.RESPONSE_COMPLETE, on_complete)
        monkeypatch.setattr(
            anthropic_client.anthropic,
            "AsyncAnthropic",
            lambda **_kwargs: _FakeClient(),
        )
        llm = AnthropicLLM(Config(llm_api_key="test"), event_bus)
        correlation_id = "018f0000-0000-7000-8000-000000000060"

        await llm.stream_generate(
            SimpleNamespace(system="system", messages=[]),
            correlation_id=correlation_id,
        )
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        return chunks, completed

    chunks, completed = asyncio.run(scenario())
    assert [event.correlation_id for event in chunks] == [
        "018f0000-0000-7000-8000-000000000060",
        "018f0000-0000-7000-8000-000000000060",
    ]
    assert completed[0].correlation_id == "018f0000-0000-7000-8000-000000000060"
