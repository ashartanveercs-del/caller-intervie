"""Lifecycle tests for orchestrator cancellation."""

import asyncio

from ai_assistant.config import Config
from ai_assistant.core.events import EventBus
from ai_assistant.core.orchestrator import Orchestrator


class _FakeLLM:
    def __init__(self) -> None:
        self.cancel_calls = 0
        self.submit_calls = 0

    def cancel_active(self) -> None:
        self.cancel_calls += 1

    async def submit(self, _prompt) -> None:
        self.submit_calls += 1


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
