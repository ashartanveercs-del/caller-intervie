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
        self._suggestion_task: asyncio.Task[None] | None = None
        self._listening = True
        # Which audio source is the candidate ("you"); the other is the
        # interviewer whose questions drive suggestions.
        self._you_source = "mic"
        # When the last final interviewer segment arrived — used to suppress
        # own-speech triggers while interviewer audio is actually flowing.
        self._last_interviewer_final = 0.0

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

    def set_you_source(self, source: str) -> None:
        """Set which audio source ('mic' or 'system') is the candidate ('you').

        The other source is treated as the interviewer, whose finished
        sentences trigger suggestions.
        """
        if source not in ("mic", "system"):
            return
        self._you_source = source
        self._transcript_buffer.you_source = source
        logger.info("You-source set to %s (interviewer is %s)",
                    source, "system" if source == "mic" else "mic")

    def cancel_pending(self) -> None:
        """Cancel debounce, suggestion preparation, and active LLM generation."""
        if self._debounce_handle is not None:
            self._debounce_handle.cancel()
            self._debounce_handle = None
        if self._suggestion_task is not None and not self._suggestion_task.done():
            self._suggestion_task.cancel()
        self._suggestion_task = None
        self._llm.cancel_active()

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

    async def trigger_query(
        self,
        explicit_query: Optional[str] = None,
        full_transcript: bool = False,
    ) -> None:
        """Manually trigger an LLM response (e.g. from hotkey).

        Set *full_transcript* for whole-conversation tasks like summarizing.
        """
        await self._bus.emit(
            EventType.QUERY_TRIGGERED, {"query": explicit_query}
        )

        rag_context = self._get_rag_context()

        prompt = await self._prompt_builder.build(
            mode=Mode.ACTIVE.value,
            transcript_buffer=self._transcript_buffer,
            rag_context=rag_context,
            query=explicit_query,
            use_full_transcript=full_transcript,
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
            interviewer_source = "system" if self._you_source == "mic" else "mic"
            # Auto-generate script when the interviewer finishes a sentence
            if event.source == interviewer_source:
                self._last_interviewer_final = event.timestamp
                self._schedule_suggestion(delay=0.8)
            # Fall back to your own speech ONLY when no interviewer audio has
            # been heard recently — otherwise every sentence the candidate
            # speaks would wipe and regenerate the answer they're reading.
            elif (
                event.source == self._you_source
                and event.speech_final
                and event.timestamp - self._last_interviewer_final > 90.0
            ):
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
            self._launch_suggestion,
        )

    def _launch_suggestion(self) -> None:
        self._debounce_handle = None
        self._suggestion_task = asyncio.create_task(self._generate_suggestion())

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
