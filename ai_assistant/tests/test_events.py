"""Tests for the async EventBus — dispatch, isolation, unsubscribe."""

import asyncio

from ai_assistant.core.events import EventBus, EventType, TranscriptEvent


def _run(coro):
    return asyncio.run(coro)


def test_emit_delivers_to_handler():
    async def scenario():
        bus = EventBus()
        received = []

        async def handler(data):
            received.append(data)

        bus.on(EventType.TRANSCRIPT_UPDATE, handler)
        await bus.emit(EventType.TRANSCRIPT_UPDATE, "hello")
        await asyncio.sleep(0.02)  # let fire-and-forget tasks run
        return received

    assert _run(scenario()) == ["hello"]


def test_multiple_handlers_all_receive():
    async def scenario():
        bus = EventBus()
        a, b = [], []

        async def handler_a(data):
            a.append(data)

        async def handler_b(data):
            b.append(data)

        bus.on(EventType.MODE_CHANGE, handler_a)
        bus.on(EventType.MODE_CHANGE, handler_b)
        await bus.emit(EventType.MODE_CHANGE, "active")
        await asyncio.sleep(0.02)
        return a, b

    a, b = _run(scenario())
    assert a == ["active"] and b == ["active"]


def test_handler_error_is_isolated():
    async def scenario():
        bus = EventBus()
        received = []

        async def bad(_):
            raise RuntimeError("boom")

        async def good(data):
            received.append(data)

        bus.on(EventType.ERROR, bad)
        bus.on(EventType.ERROR, good)
        await bus.emit(EventType.ERROR, "x")
        await asyncio.sleep(0.02)
        return received

    # A crashing handler must not stop others from receiving the event.
    assert _run(scenario()) == ["x"]


def test_off_unsubscribes():
    async def scenario():
        bus = EventBus()
        received = []

        async def handler(data):
            received.append(data)

        bus.on(EventType.RESPONSE_CHUNK, handler)
        bus.off(EventType.RESPONSE_CHUNK, handler)
        await bus.emit(EventType.RESPONSE_CHUNK, "gone")
        await asyncio.sleep(0.02)
        return received

    assert _run(scenario()) == []


def test_transcript_event_source_default():
    ev = TranscriptEvent(text="hi", is_final=True, speech_final=True)
    assert ev.source == "mic"
    assert ev.speaker is None
