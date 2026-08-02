"""Streaming LLM client for the answer pipeline.

Targets DeepSeek V4 Flash through its Anthropic-compatible API
(https://api.deepseek.com/anthropic). Because that endpoint speaks the
Anthropic Messages API, the ``anthropic`` SDK is used unchanged as the
transport — only the base URL, key, and model come from config.
"""

from __future__ import annotations

import asyncio
import logging
from uuid import uuid4

import anthropic

from ai_assistant.config import Config
from ai_assistant.core.events import (
    EventBus,
    EventType,
    ResponseChunkEvent,
    ResponseCompleteEvent,
)
from ai_assistant.llm.prompt_builder import BuiltPrompt

logger = logging.getLogger(__name__)


class AnthropicLLM:
    """Streams responses from DeepSeek's Anthropic-compatible Messages API."""

    def __init__(self, config: Config, event_bus: EventBus) -> None:
        self._config = config
        self._event_bus = event_bus
        self._client = anthropic.AsyncAnthropic(
            api_key=config.llm_api_key,
            base_url=config.llm_base_url,
        )
        self._active_task: asyncio.Task | None = None

    # ------------------------------------------------------------------
    # Streaming generation
    # ------------------------------------------------------------------

    async def stream_generate(self, prompt: BuiltPrompt) -> str:
        """Stream a response and emit chunk / complete events.

        Returns the full concatenated response text.
        """
        request_id = uuid4().hex[:8]
        full_text = ""

        async with self._client.messages.stream(
            model=self._config.llm_model,
            max_tokens=self._config.max_output_tokens,
            system=prompt.system,
            messages=prompt.messages,
            temperature=self._config.temperature,
        ) as stream:
            async for text in stream.text_stream:
                full_text += text
                await self._event_bus.emit(
                    EventType.RESPONSE_CHUNK,
                    ResponseChunkEvent(text=text, request_id=request_id),
                )

            final_message = await stream.get_final_message()

        input_tokens = final_message.usage.input_tokens
        output_tokens = final_message.usage.output_tokens

        await self._event_bus.emit(
            EventType.RESPONSE_COMPLETE,
            ResponseCompleteEvent(
                full_text=full_text,
                request_id=request_id,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
            ),
        )

        logger.info(
            "Request %s complete: %d input / %d output tokens",
            request_id,
            input_tokens,
            output_tokens,
        )

        return full_text

    # ------------------------------------------------------------------
    # Task management
    # ------------------------------------------------------------------

    def cancel_active(self) -> None:
        """Cancel the in-flight generation task, if any."""
        if self._active_task is not None and not self._active_task.done():
            self._active_task.cancel()
            logger.debug("Cancelled active generation task")

    async def submit(self, prompt: BuiltPrompt) -> None:
        """Cancel any running generation and start a new one."""
        self.cancel_active()
        self._active_task = asyncio.create_task(self._run(prompt))

    async def _run(self, prompt: BuiltPrompt) -> None:
        """Wrapper that handles cancellation and API errors."""
        try:
            await self.stream_generate(prompt)
        except asyncio.CancelledError:
            pass
        except anthropic.APIStatusError as exc:
            logger.error("Anthropic API error: %s", exc)
            await self._event_bus.emit(
                EventType.ERROR,
                {"error": str(exc), "source": "anthropic_client"},
            )
        except Exception:
            logger.exception("Unexpected error during generation")
            await self._event_bus.emit(
                EventType.ERROR,
                {"error": "unexpected generation error", "source": "anthropic_client"},
            )
