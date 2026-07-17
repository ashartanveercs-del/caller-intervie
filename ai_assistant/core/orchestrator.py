"""Orchestration layer — mode management, context building, event coordination."""

from __future__ import annotations

import asyncio
import logging
from enum import Enum
from typing import Optional

from ai_assistant.config import Config
from ai_assistant.core.events import (
    EventBus,
    EventType,
    ModeChangeEvent,
    TranscriptEvent,
)
from ai_assistant.llm.anthropic_client import AnthropicLLM
from ai_assistant.llm.prompt_builder import PromptBuilder, TranscriptBuffer
from ai_assistant.rag.retriever import RAGRetriever

logger = logging.getLogger(__name__)


class Mode(str, Enum):
    PASSIVE = "passive"
    SUGGESTION = "suggestion"
    ACTIVE = "active"


class Orchestrator:
    """Central coordinator: routes transcript events through RAG and LLM
    according to the current mode."""

    SUGGESTION_DEBOUNCE_SECONDS = 1.5

    def __init__(
        self,
        config: Config,
        event_bus: EventBus,
        llm: AnthropicLLM,
        prompt_builder: PromptBuilder,
        retriever: Optional[RAGRetriever] = None,
    ) -> None:
        self._config = config
        self._bus = event_bus
        self._llm = llm
        self._prompt_builder = prompt_builder
        self._retriever = retriever
        self._transcript_buffer = TranscriptBuffer(
            max_seconds=config.memory_window_seconds,
        )
        self._mode = Mode.SUGGESTION
        self._debounce_handle: asyncio.TimerHandle | None = None
        self._listening = True

        # Wire event handlers
        self._bus.on(EventType.TRANSCRIPT_UPDATE, self._on_transcript)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    @property
    def mode(self) -> Mode:
        return self._mode

    @property
    def listening(self) -> bool:
        return self._listening

    def set_listening(self, enabled: bool) -> None:
        self._listening = enabled
        logger.info("Listening %s", "enabled" if enabled else "disabled")

    def set_retriever(self, retriever: RAGRetriever) -> None:
        """Attach the RAG retriever once it has finished loading in the background."""
        self._retriever = retriever
        logger.info("RAG retriever attached to orchestrator")

    async def set_mode(self, new_mode: Mode) -> None:
        old = self._mode
        self._mode = new_mode
        if new_mode == Mode.PASSIVE:
            self._llm.cancel_active()
        await self._bus.emit(
            EventType.MODE_CHANGE,
            ModeChangeEvent(old_mode=old.value, new_mode=new_mode.value),
        )
        logger.info("Mode changed: %s -> %s", old.value, new_mode.value)

    async def trigger_query(self, explicit_query: Optional[str] = None) -> None:
        """Manually trigger an LLM response (e.g. from hotkey)."""
        await self._bus.emit(
            EventType.QUERY_TRIGGERED, {"query": explicit_query}
        )

        rag_context = self._get_rag_context()

        prompt = await self._prompt_builder.build(
            mode=Mode.ACTIVE.value,
            transcript_buffer=self._transcript_buffer,
            rag_context=rag_context,
            query=explicit_query,
        )
        await self._llm.submit(prompt)

    # ------------------------------------------------------------------
    # Event handlers
    # ------------------------------------------------------------------

    async def _on_transcript(self, event: TranscriptEvent) -> None:
        if not self._listening:
            return

        self._transcript_buffer.add(event)

        if self._mode == Mode.PASSIVE:
            return

        if not event.is_final:
            return

        if self._mode == Mode.SUGGESTION:
            # Auto-generate script when interviewer finishes a sentence
            if event.source == "system":
                self._schedule_suggestion(delay=0.8)
            # Also trigger on your own speech in case interviewer audio isn't on
            elif event.source == "mic" and event.speech_final:
                self._schedule_suggestion()
        elif self._mode == Mode.ACTIVE:
            if event.is_final:
                await self.trigger_query()

    # ------------------------------------------------------------------
    # Suggestion debounce
    # ------------------------------------------------------------------

    def _schedule_suggestion(self, delay: float | None = None) -> None:
        """Debounce: wait before generating a suggestion to avoid spam."""
        loop = asyncio.get_running_loop()
        if self._debounce_handle is not None:
            self._debounce_handle.cancel()
        wait = delay if delay is not None else self.SUGGESTION_DEBOUNCE_SECONDS
        self._debounce_handle = loop.call_later(
            wait,
            lambda: asyncio.create_task(self._generate_suggestion()),
        )

    async def _generate_suggestion(self) -> None:
        rag_context = self._get_rag_context()
        prompt = await self._prompt_builder.build(
            mode=Mode.SUGGESTION.value,
            transcript_buffer=self._transcript_buffer,
            rag_context=rag_context,
        )
        await self._llm.submit(prompt)

    # ------------------------------------------------------------------
    # RAG helper
    # ------------------------------------------------------------------

    def _get_rag_context(self) -> Optional[str]:
        """Query the RAG retriever with the most recent transcript text."""
        if self._retriever is None or self._retriever.vector_store.size == 0:
            return None

        recent = self._transcript_buffer.get_recent_text(max_chars=500)
        if not recent:
            return None

        try:
            result = self._retriever.query(recent)
            if not result.chunks:
                return None

            parts: list[str] = []
            for cws in result.chunks:
                source = cws.chunk.source_path
                page = cws.chunk.page_number
                loc = f"{source}"
                if page is not None:
                    loc += f" (p.{page})"
                parts.append(f"[Source: {loc} | score={cws.score:.2f}]\n{cws.chunk.text}")

            return "\n\n---\n\n".join(parts)
        except Exception:
            logger.exception("RAG query failed")
            return None
