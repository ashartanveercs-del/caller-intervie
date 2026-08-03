"""Regression tests for the cross-thread Deepgram lifecycle."""

import asyncio

from ai_assistant.audio.deepgram_client import DeepgramTranscriber, _DGStream
from ai_assistant.config import Config
from ai_assistant.core.events import EventBus


async def _delayed_noop(self) -> None:
    await asyncio.sleep(0.05)
    self._running = True
    self._ws = object()


async def _failed_connect(self) -> None:
    await asyncio.sleep(0)


async def _assert_loop_progresses(awaitable) -> None:
    progressed = False

    async def mark_progress() -> None:
        nonlocal progressed
        await asyncio.sleep(0.01)
        progressed = True

    marker = asyncio.create_task(mark_progress())
    await awaitable
    assert progressed
    await marker


def test_start_does_not_block_main_event_loop(monkeypatch):
    monkeypatch.setattr(_DGStream, "connect", _delayed_noop)

    async def scenario() -> None:
        transcriber = DeepgramTranscriber(Config(), EventBus())
        try:
            await _assert_loop_progresses(transcriber.start())
        finally:
            await transcriber.stop()

    asyncio.run(scenario())


def test_initial_mic_connection_failure_is_propagated(monkeypatch):
    monkeypatch.setattr(_DGStream, "connect", _failed_connect)

    async def scenario() -> None:
        transcriber = DeepgramTranscriber(Config(), EventBus())
        try:
            try:
                await transcriber.start()
            except RuntimeError as error:
                assert str(error) == "Mic Deepgram connection failed"
            else:
                raise AssertionError("initial connection failure was suppressed")
        finally:
            await transcriber.stop()

    asyncio.run(scenario())


def test_initial_system_connection_failure_is_propagated(monkeypatch):
    monkeypatch.setattr(_DGStream, "connect", _delayed_noop)

    async def scenario() -> None:
        transcriber = DeepgramTranscriber(Config(), EventBus())
        await transcriber.start()
        monkeypatch.setattr(_DGStream, "connect", _failed_connect)
        try:
            try:
                await transcriber.start_system_stream()
            except RuntimeError as error:
                assert str(error) == "System Deepgram connection failed"
            else:
                raise AssertionError("system connection failure was suppressed")
        finally:
            await transcriber.stop()

    asyncio.run(scenario())


def test_stop_system_stream_does_not_block_main_event_loop(monkeypatch):
    monkeypatch.setattr(_DGStream, "connect", _delayed_noop)
    monkeypatch.setattr(_DGStream, "close", _delayed_noop)

    async def scenario() -> None:
        transcriber = DeepgramTranscriber(Config(), EventBus())
        await transcriber.start()
        await transcriber.start_system_stream()
        try:
            await _assert_loop_progresses(transcriber.stop_system_stream())
        finally:
            await transcriber.stop()

    asyncio.run(scenario())


def test_stop_does_not_block_main_event_loop(monkeypatch):
    monkeypatch.setattr(_DGStream, "connect", _delayed_noop)
    monkeypatch.setattr(_DGStream, "close", _delayed_noop)

    async def scenario() -> None:
        transcriber = DeepgramTranscriber(Config(), EventBus())
        await transcriber.start()
        await _assert_loop_progresses(transcriber.stop())

    asyncio.run(scenario())
