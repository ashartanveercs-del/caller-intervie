"""Deepgram live transcription via raw websocket — supports dual streams.

Runs two independent Deepgram websocket connections on a dedicated
``SelectorEventLoop`` in a daemon thread:
  - **mic** stream: transcribes your microphone (source="mic")
  - **system** stream: transcribes system audio / interviewer (source="system")

Each transcript event is tagged with its source so the UI can display
"You" vs "Interviewer" reliably without depending on diarization.
"""

from __future__ import annotations

import asyncio
import json
import logging
import threading
from typing import Any, Optional
from urllib.parse import urlencode

import websockets

from ai_assistant.config import Config
from ai_assistant.core.events import EventBus, EventType, TranscriptEvent

logger = logging.getLogger(__name__)

DEEPGRAM_WS_URL = "wss://api.deepgram.com/v1/listen"


class _DGStream:
    """A single Deepgram websocket connection for one audio source."""

    def __init__(
        self,
        config: Config,
        bus: EventBus,
        source: str,
        main_loop: asyncio.AbstractEventLoop,
        dg_loop: asyncio.AbstractEventLoop,
    ) -> None:
        self._config = config
        self._bus = bus
        self._source = source  # "mic" or "system"
        self._main_loop = main_loop
        self._dg_loop = dg_loop
        self._ws: Optional[Any] = None
        self._running = False
        self._reconnect_attempts = 0

    @property
    def connected(self) -> bool:
        """Whether this stream has an active provider websocket."""
        return self._running and self._ws is not None

    def _build_url(self) -> str:
        params = {
            "model": self._config.deepgram_model,
            "language": self._config.deepgram_language,
            "encoding": "linear16",
            "sample_rate": self._config.sample_rate,
            "channels": self._config.channels,
            "interim_results": str(self._config.interim_results).lower(),
            "smart_format": "true",
            "punctuate": "true",
            "utterance_end_ms": str(self._config.utterance_end_ms),
            "endpointing": str(self._config.endpointing),
        }
        return f"{DEEPGRAM_WS_URL}?{urlencode(params)}"

    async def connect(self) -> None:
        self._reconnect_attempts += 1
        logger.info(
            "[%s] Connecting to Deepgram (attempt %d)",
            self._source, self._reconnect_attempts,
        )
        url = self._build_url()
        headers = {"Authorization": f"Token {self._config.deepgram_api_key}"}

        try:
            self._ws = await websockets.connect(
                url, extra_headers=headers, ping_interval=20, ping_timeout=10,
            )
            self._running = True
            self._reconnect_attempts = 0
            self._last_send = asyncio.get_event_loop().time()
            logger.info("[%s] Deepgram connected", self._source)
            self._dg_loop.create_task(self._receive_loop())
            self._dg_loop.create_task(self._keepalive_loop())
        except Exception:
            logger.exception("[%s] Deepgram connection failed", self._source)
            self._ws = None
            await self._schedule_reconnect()

    async def send(self, data: bytes) -> None:
        if self._ws is not None:
            try:
                await self._ws.send(data)
                self._last_send = asyncio.get_event_loop().time()
            except Exception:
                pass

    async def close(self) -> None:
        self._running = False
        if self._ws is not None:
            try:
                await self._ws.send(json.dumps({"type": "CloseStream"}))
                await self._ws.close()
            except Exception:
                pass
            self._ws = None

    async def _keepalive_loop(self) -> None:
        """Send silence frames to prevent Deepgram from closing idle connections."""
        silence = b"\x00" * 640  # 20ms of 16kHz 16-bit silence
        while self._running and self._ws is not None:
            await asyncio.sleep(3)
            now = asyncio.get_event_loop().time()
            if now - self._last_send > 3:
                try:
                    await self._ws.send(silence)
                    self._last_send = now
                except Exception:
                    break

    async def _receive_loop(self) -> None:
        try:
            async for raw_msg in self._ws:
                try:
                    msg = json.loads(raw_msg)
                    self._handle_message(msg)
                except json.JSONDecodeError:
                    pass
        except websockets.ConnectionClosed:
            logger.warning("[%s] Deepgram websocket closed", self._source)
        except Exception:
            logger.exception("[%s] Receive loop error", self._source)
        finally:
            if self._running:
                await self._schedule_reconnect()

    def _handle_message(self, msg: dict) -> None:
        msg_type = msg.get("type", "")
        if msg_type == "Results":
            self._handle_transcript(msg)
        elif msg_type == "Metadata":
            logger.info("[%s] Deepgram session started", self._source)

    def _handle_transcript(self, msg: dict) -> None:
        try:
            alt = msg.get("channel", {}).get("alternatives", [{}])[0]
            text = alt.get("transcript", "")
            if not text:
                return

            event = TranscriptEvent(
                text=text,
                is_final=msg.get("is_final", False),
                speech_final=msg.get("speech_final", False),
                speaker=0 if self._source == "mic" else 1,
                confidence=alt.get("confidence", 0.0),
                source=self._source,
            )

            logger.debug(
                "[%s] Transcript: final=%s text=%r",
                self._source, event.is_final, text[:80],
            )

            asyncio.run_coroutine_threadsafe(
                self._bus.emit(EventType.TRANSCRIPT_UPDATE, event),
                self._main_loop,
            )
        except Exception:
            logger.exception("[%s] Transcript processing error", self._source)

    async def _schedule_reconnect(self) -> None:
        if self._reconnect_attempts >= self._config.reconnect_max_attempts:
            logger.error("[%s] Max reconnect attempts reached", self._source)
            self._running = False
            return
        delay = min(
            self._config.reconnect_base_delay * (2 ** self._reconnect_attempts),
            self._config.reconnect_max_delay,
        )
        logger.info("[%s] Reconnecting in %.1fs", self._source, delay)
        await asyncio.sleep(delay)
        try:
            await self.connect()
        except Exception:
            await self._schedule_reconnect()


class DeepgramTranscriber:
    """Manages two Deepgram streams: mic (you) and system (interviewer)."""

    def __init__(self, config: Config, bus: EventBus) -> None:
        self._config = config
        self._bus = bus
        self._dg_loop = asyncio.SelectorEventLoop()
        self._dg_thread: Optional[threading.Thread] = None
        self._main_loop: Optional[asyncio.AbstractEventLoop] = None
        self._mic_stream: Optional[_DGStream] = None
        self._sys_stream: Optional[_DGStream] = None

    async def start(self) -> None:
        self._main_loop = asyncio.get_running_loop()

        self._dg_thread = threading.Thread(
            target=self._dg_loop.run_forever, daemon=True
        )
        self._dg_thread.start()

        # Start mic stream
        self._mic_stream = _DGStream(
            self._config, self._bus, "mic", self._main_loop, self._dg_loop
        )
        fut = asyncio.run_coroutine_threadsafe(
            self._mic_stream.connect(), self._dg_loop
        )
        try:
            await asyncio.wait_for(asyncio.wrap_future(fut), timeout=15)
        except asyncio.TimeoutError:
            logger.error("Mic Deepgram connection timed out")
            raise
        except Exception:
            logger.exception("Mic Deepgram connection failed")
            raise
        if not self._mic_stream.connected:
            raise RuntimeError("Mic Deepgram connection failed")

    async def start_system_stream(self) -> None:
        """Start the system audio Deepgram stream (called when user selects device)."""
        if self._sys_stream is not None:
            await self.stop_system_stream()

        self._sys_stream = _DGStream(
            self._config, self._bus, "system", self._main_loop, self._dg_loop
        )
        fut = asyncio.run_coroutine_threadsafe(
            self._sys_stream.connect(), self._dg_loop
        )
        try:
            await asyncio.wait_for(asyncio.wrap_future(fut), timeout=15)
        except asyncio.TimeoutError:
            logger.error("System Deepgram connection timed out")
            raise
        except Exception:
            logger.exception("System Deepgram connection failed")
            raise
        if not self._sys_stream.connected:
            raise RuntimeError("System Deepgram connection failed")

    async def stop_system_stream(self) -> None:
        if self._sys_stream is not None:
            fut = asyncio.run_coroutine_threadsafe(
                self._sys_stream.close(), self._dg_loop
            )
            try:
                await asyncio.wait_for(asyncio.wrap_future(fut), timeout=5)
            except asyncio.TimeoutError:
                logger.warning("System Deepgram stream close timed out")
            except Exception:
                logger.exception("System Deepgram stream close failed")
            self._sys_stream = None

    async def send_mic_audio(self, data: bytes) -> None:
        if self._mic_stream is not None:
            asyncio.run_coroutine_threadsafe(
                self._mic_stream.send(data), self._dg_loop
            )

    async def send_system_audio(self, data: bytes) -> None:
        if self._sys_stream is not None:
            asyncio.run_coroutine_threadsafe(
                self._sys_stream.send(data), self._dg_loop
            )

    async def stop(self) -> None:
        for stream in (self._mic_stream, self._sys_stream):
            if stream is not None:
                fut = asyncio.run_coroutine_threadsafe(
                    stream.close(), self._dg_loop
                )
                try:
                    await asyncio.wait_for(asyncio.wrap_future(fut), timeout=5)
                except asyncio.TimeoutError:
                    logger.warning("Deepgram stream close timed out")
                except Exception:
                    logger.exception("Deepgram stream close failed")
        self._dg_loop.call_soon_threadsafe(self._dg_loop.stop)
        if self._dg_thread is not None:
            await asyncio.to_thread(self._dg_thread.join, 3)
            if self._dg_thread.is_alive():
                logger.warning("Deepgram event loop thread did not stop")
            elif not self._dg_loop.is_closed():
                self._dg_loop.close()
        logger.info("Deepgram client stopped")
