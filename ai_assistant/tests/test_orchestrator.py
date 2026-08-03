"""Lifecycle tests for orchestrator cancellation."""

import asyncio

from ai_assistant.config import Config
from ai_assistant.core.events import EventBus, TranscriptEvent
from ai_assistant.core.orchestrator import Orchestrator


class _FakeLLM:
    def __init__(self) -> None:
        self.cancel_calls = 0
        self.submit_calls = 0
        self.correlations: list[str | None] = []

    def cancel_active(self) -> None:
        self.cancel_calls += 1

    async def submit(self, _prompt, correlation_id: str | None = None) -> None:
        self.submit_calls += 1
        self.correlations.append(correlation_id)


class _FakePromptBuilder:
    def __init__(self) -> None:
        self.build_calls = 0

    async def build(self, **_kwargs):
        self.build_calls += 1
        return object()


def test_cancel_pending_prevents_debounced_generation_and_cancels_llm() -> None:
    async def scenario() -> tuple[_FakeLLM, _FakePromptBuilder]:
        llm = _FakeLLM()
        prompts = _FakePromptBuilder()
        orchestrator = Orchestrator(Config(), EventBus(), llm, prompts)
        orchestrator._schedule_suggestion(delay=0.01)
        orchestrator.cancel_pending()
        await asyncio.sleep(0.02)
        return llm, prompts

    llm, prompts = asyncio.run(scenario())
    assert llm.cancel_calls == 1
    assert llm.submit_calls == 0
    assert prompts.build_calls == 0


def test_trigger_query_passes_its_correlation_to_the_exact_llm_request() -> None:
    async def scenario() -> _FakeLLM:
        llm = _FakeLLM()
        prompts = _FakePromptBuilder()
        orchestrator = Orchestrator(Config(), EventBus(), llm, prompts)
        correlation_id = "018f0000-0000-7000-8000-000000000050"
        await orchestrator.trigger_query("Explain", correlation_id=correlation_id)
        return llm

    llm = asyncio.run(scenario())
    assert llm.correlations == ["018f0000-0000-7000-8000-000000000050"]


def test_later_nontriggering_transcript_cannot_replace_suggestion_correlation() -> None:
    async def scenario() -> _FakeLLM:
        llm = _FakeLLM()
        prompts = _FakePromptBuilder()
        orchestrator = Orchestrator(Config(), EventBus(), llm, prompts)
        scheduled: list[str | None] = []

        def capture_schedule(delay=None, correlation_id=None) -> None:
            scheduled.append(correlation_id)

        orchestrator._schedule_suggestion = capture_schedule
        triggering_id = "018f0000-0000-7000-8000-000000000051"
        later_id = "018f0000-0000-7000-8000-000000000052"
        await orchestrator._on_transcript(
            TranscriptEvent(
                text="Question",
                is_final=True,
                speech_final=True,
                source="system",
                event_id=triggering_id,
            )
        )
        await orchestrator._on_transcript(
            TranscriptEvent(
                text="Candidate response",
                is_final=True,
                speech_final=True,
                source="mic",
                event_id=later_id,
            )
        )
        await orchestrator._generate_suggestion(scheduled[0])
        return llm

    llm = asyncio.run(scenario())
    assert llm.correlations == ["018f0000-0000-7000-8000-000000000051"]
