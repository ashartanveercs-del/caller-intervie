"""UI-independent lifecycle and control service for the assistant engine."""

from __future__ import annotations

import asyncio
from collections.abc import Callable
import inspect
import logging
import struct
import threading
from typing import TYPE_CHECKING, Literal

from ai_assistant.core.events import EventType
from ai_assistant.core.orchestrator import Mode

from .models import (
    AudioSource,
    DocumentIngestionResult,
    RuntimeConfigurationError,
    RuntimeSnapshot,
    RuntimeStateError,
    SessionConfig,
)

if TYPE_CHECKING:
    from .factory import RuntimeDependencies

logger = logging.getLogger(__name__)

AudioLevelHandler = Callable[[AudioSource, float], object]


class RuntimeService:
    """Own the assistant engine lifecycle independently of any UI toolkit."""

    def __init__(self, dependencies: RuntimeDependencies) -> None:
        self._deps = dependencies
        self._state: Literal[
            "idle", "starting", "listening", "paused", "stopping", "stopped", "error"
        ] = "idle"
        self._session: SessionConfig | None = None
        self._system_audio_enabled = False
        self._capture_started = False
        self._transcriber_started = False
        self._pump_task: asyncio.Task[None] | None = None
        self._lifecycle_lock = asyncio.Lock()
        self._cleanup_complete = True
        self._audio_level_handler: AudioLevelHandler | None = None

        self._retriever = None
        self._knowledge_state: Literal["disabled", "loading", "ready", "error"] = (
            "disabled"
        )
        self._rag_generation = 0

        self.event_bus.on(EventType.RESPONSE_COMPLETE, self._remember_response)

    @property
    def event_bus(self):
        return self._deps.event_bus

    async def start_session(self, config: SessionConfig) -> None:
        """Validate configuration and start transcription plus audio capture."""
        async with self._lifecycle_lock:
            if self._state in ("starting", "listening", "paused", "stopping"):
                raise RuntimeStateError("A runtime session is already active")

            missing = [
                name
                for name, value in (
                    ("deepgram_api_key", self._deps.config.deepgram_api_key),
                    ("llm_api_key", self._deps.config.llm_api_key),
                )
                if not value
            ]
            if missing:
                self._state = "idle"
                raise RuntimeConfigurationError(missing)

            self._state = "starting"
            self._cleanup_complete = False
            self._session = config
            self._deps.config.deepgram_language = config.input_language
            self._deps.config.response_language = config.response_language
            self._deps.config.review_language = config.review_language
            self._deps.orchestrator.set_you_source(config.you_source)
            self._deps.orchestrator.set_listening(True)

            try:
                self._transcriber_started = True
                await self._deps.transcriber.start()
                self._capture_started = True
                await self._deps.capture.start()
                self._pump_task = asyncio.create_task(self._audio_pump())
                self._start_knowledge_load()
                self._state = "listening"
            except BaseException:
                self._state = "error"
                cleanup_errors: list[BaseException] = []
                self._cancel_orchestration(cleanup_errors)
                await self._stop_started_dependencies()
                self._cleanup_complete = True
                raise

    async def stop_session(self) -> None:
        """Stop owned resources once; repeated calls have no side effects."""
        async with self._lifecycle_lock:
            if self._cleanup_complete:
                return

            self._state = "stopping"
            errors: list[BaseException] = []
            self._cancel_orchestration(errors)
            self._rag_generation += 1

            errors.extend(await self._stop_started_dependencies())
            self._system_audio_enabled = False
            rag_error = self._save_rag_index()
            if rag_error is not None:
                errors.append(rag_error)
            self._cleanup_complete = True
            self._state = "error" if errors else "stopped"
            if errors:
                raise errors[0]

    def set_listening(self, enabled: bool) -> None:
        self._require_active()
        self._deps.orchestrator.set_listening(enabled)
        self._state = "listening" if enabled else "paused"

    async def set_mode(self, mode: str) -> None:
        async with self._lifecycle_lock:
            self._require_active()
            try:
                parsed = Mode(mode)
            except ValueError as error:
                raise ValueError(f"Unknown mode: {mode}") from error
            await self._deps.orchestrator.set_mode(parsed)

    def set_you_source(self, source: AudioSource) -> None:
        self._require_active()
        if source not in ("mic", "system"):
            raise ValueError(f"Unknown audio source: {source}")
        self._deps.orchestrator.set_you_source(source)
        if self._session is not None:
            self._session = SessionConfig(
                session_id=self._session.session_id,
                mode=self._session.mode,
                input_language=self._session.input_language,
                response_language=self._session.response_language,
                review_language=self._session.review_language,
                you_source=source,
                brief_id=self._session.brief_id,
            )

    async def set_audio_device(
        self,
        source: AudioSource,
        device: int | None,
    ) -> None:
        async with self._lifecycle_lock:
            self._require_active()
            if source == "mic":
                await self._deps.capture.change_mic_device(device)
            elif source == "system":
                await self._deps.capture.change_system_device(device)
            else:
                raise ValueError(f"Unknown audio source: {source}")

    async def set_system_audio(self, enabled: bool) -> None:
        async with self._lifecycle_lock:
            self._require_active()
            if enabled == self._system_audio_enabled:
                return
            if enabled:
                self._deps.capture.start_system_capture()
                try:
                    await self._deps.transcriber.start_system_stream()
                except BaseException:
                    self._deps.capture.stop_system_capture()
                    raise
            else:
                self._deps.capture.stop_system_capture()
                await self._deps.transcriber.stop_system_stream()
            self._system_audio_enabled = enabled

    async def trigger_query(self, text: str, answer_format: str) -> None:
        async with self._lifecycle_lock:
            self._require_active()
            if answer_format == "chat":
                self._deps.prompt_builder.add_to_history("user", text)
            await self._deps.orchestrator.trigger_query(
                text,
                full_transcript=answer_format == "summary",
            )

    async def ingest_documents(self, paths: list[str]) -> DocumentIngestionResult:
        async with self._lifecycle_lock:
            self._require_active()
            if self._retriever is None:
                raise RuntimeStateError("Knowledge base is not ready")

            def _ingest() -> DocumentIngestionResult:
                total_chunks = 0
                for path in paths:
                    try:
                        count = self._retriever.ingest_file(path)
                        total_chunks += count
                        logger.info("Ingested %s — %d chunks", path, count)
                    except Exception:
                        logger.exception("Failed to ingest %s", path)
                self._retriever.save_index(self._deps.config.rag_db_path)
                return DocumentIngestionResult(
                    files=len(paths),
                    chunks_added=total_chunks,
                    total_chunks=self._retriever.vector_store.size,
                )

            return await asyncio.to_thread(_ingest)

    def snapshot(self) -> RuntimeSnapshot:
        session = self._session
        return RuntimeSnapshot(
            state=self._state,
            session_id=session.session_id if session else None,
            mode=session.mode if session else None,
            input_language=session.input_language if session else None,
            response_language=session.response_language if session else None,
            review_language=session.review_language if session else None,
            you_source=session.you_source if session else "mic",
            listening=self._state == "listening",
            system_audio_enabled=self._system_audio_enabled,
            knowledge_state=self._knowledge_state,
            knowledge_chunks=(
                self._retriever.vector_store.size if self._retriever is not None else 0
            ),
        )

    def full_transcript(self) -> str:
        return self._deps.orchestrator._transcript_buffer.get_full_text()

    def set_system_prompt(self, text: str) -> None:
        self._deps.prompt_builder.custom_system_prompt = text

    def set_audio_level_handler(self, handler: AudioLevelHandler | None) -> None:
        self._audio_level_handler = handler

    async def generate_notes(self) -> list[str]:
        """Generate chronological notes from the complete transcript."""
        buffer = self._deps.orchestrator._transcript_buffer
        if not buffer.get_full_text():
            return []
        prompt = await self._deps.prompt_builder.build(
            mode="active",
            transcript_buffer=buffer,
            use_full_transcript=True,
            query=(
                "Extract the KEY POINTS from the ENTIRE conversation, from the very "
                "beginning to now. Go through it in order and do not skip the earlier "
                "parts. Focus on: questions asked, answers given, decisions made, "
                "action items, and important topics. "
                "Format: one bullet per point, in chronological order, as many "
                "bullets as it takes to cover everything. Be concise per bullet. "
                "Use plain text, no markdown."
            ),
        )
        response = await self._deps.llm.stream_generate(prompt)
        return [
            line.strip().lstrip("\u2022-* ")
            for line in response.strip().split("\n")
            if line.strip().lstrip("\u2022-* ")
        ]

    async def _remember_response(self, event) -> None:
        self._deps.prompt_builder.add_to_history("assistant", event.full_text)

    async def _audio_pump(self) -> None:
        frame_count = 0
        async for source, chunk in self._deps.capture:
            if source == "mic":
                await self._deps.transcriber.send_mic_audio(chunk)
            elif source == "system":
                await self._deps.transcriber.send_system_audio(chunk)

            frame_count += 1
            if frame_count % 4 == 0 and self._audio_level_handler is not None:
                result = self._audio_level_handler(source, self._compute_level(chunk))
                if inspect.isawaitable(result):
                    await result

    async def _stop_started_dependencies(self) -> list[BaseException]:
        errors: list[BaseException] = []
        pump_task = self._pump_task
        self._pump_task = None
        if pump_task is not None:
            pump_task.cancel()
            try:
                await pump_task
            except asyncio.CancelledError:
                pass
            except BaseException as error:
                errors.append(error)

        if self._capture_started:
            self._capture_started = False
            try:
                await self._deps.capture.stop()
            except BaseException as error:
                errors.append(error)
        if self._transcriber_started:
            self._transcriber_started = False
            try:
                await self._deps.transcriber.stop()
            except BaseException as error:
                errors.append(error)
        return errors

    def _cancel_orchestration(self, errors: list[BaseException]) -> None:
        try:
            self._deps.orchestrator.set_listening(False)
        except BaseException as error:
            errors.append(error)
        try:
            self._deps.orchestrator.cancel_pending()
        except BaseException as error:
            errors.append(error)

    def _start_knowledge_load(self) -> None:
        rag_factory = self._deps.rag_factory
        if rag_factory is None:
            self._knowledge_state = "disabled"
            return

        self._knowledge_state = "loading"
        self._rag_generation += 1
        generation = self._rag_generation
        loop = asyncio.get_running_loop()

        def _worker() -> None:
            try:
                retriever = rag_factory()
            except BaseException as error:
                callback = lambda error=error: self._finish_knowledge_error(
                    generation, error
                )
            else:
                callback = lambda: self._finish_knowledge_load(generation, retriever)
            try:
                loop.call_soon_threadsafe(callback)
            except RuntimeError:
                pass

        threading.Thread(target=_worker, daemon=True).start()

    def _finish_knowledge_load(self, generation: int, retriever) -> None:
        if generation != self._rag_generation:
            return
        self._retriever = retriever
        self._deps.orchestrator.set_retriever(retriever)
        self._knowledge_state = "ready"
        logger.info("RAG ready (%d chunks)", retriever.vector_store.size)

    def _finish_knowledge_error(self, generation: int, error: BaseException) -> None:
        if generation != self._rag_generation:
            return
        self._knowledge_state = "error"
        logger.error(
            "Background RAG load failed — running without documents",
            exc_info=(type(error), error, error.__traceback__),
        )

    def _save_rag_index(self) -> BaseException | None:
        if self._retriever is None:
            return None
        try:
            self._retriever.save_index(self._deps.config.rag_db_path)
            logger.info("Saved RAG index to %s", self._deps.config.rag_db_path)
        except BaseException as error:
            logger.warning("Failed to save RAG index")
            return error
        return None

    def _require_active(self) -> None:
        if self._state not in ("listening", "paused"):
            raise RuntimeStateError("No runtime session is active")

    @staticmethod
    def _compute_level(pcm_bytes: bytes) -> float:
        n_samples = len(pcm_bytes) // 2
        if n_samples == 0:
            return 0.0
        samples = struct.unpack(f"<{n_samples}h", pcm_bytes[: n_samples * 2])
        rms = (sum(sample * sample for sample in samples) / n_samples) ** 0.5
        return min(1.0, rms / 8000.0)
