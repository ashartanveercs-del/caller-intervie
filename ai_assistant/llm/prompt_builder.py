"""Prompt construction and transcript buffering for the LLM pipeline."""

from __future__ import annotations

import logging
from dataclasses import dataclass, field

from ai_assistant.config import Config
from ai_assistant.core.events import TranscriptEvent

logger = logging.getLogger(__name__)

_ANSWER_FORMAT = (
    "You are a real-time interview answer engine. When you see a question from the conversation, "
    "output a COMPLETE, STRUCTURED answer the candidate can reference while speaking.\n\n"
    "FORMAT YOUR ANSWER EXACTLY LIKE THIS:\n"
    "---\n"
    "Summarized question:\n"
    "[Restate the question in one clear line]\n\n"
    "Answer:\n"
    "[2-3 sentence direct answer to say out loud]\n\n"
    "Key Points:\n"
    "- [point 1]\n"
    "- [point 2]\n"
    "- [point 3]\n\n"
    "Deep Dive:\n"
    "[Detailed explanation with technical depth, examples, algorithms, "
    "trade-offs, complexity analysis — whatever is relevant. "
    "Keep growing this section with more detail. "
    "Include code snippets if it's a coding question.]\n"
    "---\n\n"
    "RULES:\n"
    "- ALWAYS start with the summarized question\n"
    "- ALWAYS give the short answer first, then expand with detail\n"
    "- If it's a coding problem, include the algorithm AND code\n"
    "- If it's behavioral, include a STAR-format example\n"
    "- If it's system design, include components, trade-offs, scaling\n"
    "- Be thorough — the candidate glances at this while talking\n"
    "- NEVER say 'I'm listening' or 'waiting for question' — always give an answer\n"
    "- If the question isn't clear, answer your best interpretation"
)

MODE_INSTRUCTIONS: dict[str, str] = {
    "passive": "Do not generate responses. Only observe.",
    "suggestion": _ANSWER_FORMAT,
    "active": _ANSWER_FORMAT,
}

SYSTEM_TEMPLATE = (
    "You are a real-time interview assistant that generates structured answers "
    "the candidate can reference while speaking. Always produce an answer. "
    "Never produce meta-commentary or waiting messages.\n\n"
    "The <conversation> transcript is labeled by role. Lines that begin with "
    "'Interviewer:' are the person interviewing the candidate — treat these as "
    "the questions to answer. Lines that begin with 'You:' are the candidate "
    "(the person you are helping) speaking. Answer as the candidate: respond to "
    "the interviewer's most recent question, and use the candidate's own 'You:' "
    "lines only as context for what they have already said.\n\n"
    "{mode_instructions}"
)


@dataclass
class BuiltPrompt:
    """Assembled prompt ready for the Anthropic API."""

    system: str
    messages: list[dict]
    estimated_tokens: int


class TranscriptBuffer:
    """Rolling window of transcript segments."""

    def __init__(self, max_seconds: float = 300.0) -> None:
        self.max_seconds = max_seconds
        self._segments: list[TranscriptEvent] = []
        # Full-session history of final segments — never pruned, so notes and
        # summaries can cover the entire interview, not just the rolling window.
        self._all_finals: list[TranscriptEvent] = []
        # Which source is the candidate ("you"); the other is the interviewer.
        self.you_source = "mic"

    def add(self, event: TranscriptEvent) -> None:
        """Append a transcript segment and prune old entries."""
        self._segments.append(event)
        if event.is_final:
            self._all_finals.append(event)
        self._prune(event.timestamp)

    def _prune(self, now: float) -> None:
        """Remove segments older than max_seconds from *now*."""
        cutoff = now - self.max_seconds
        self._segments = [s for s in self._segments if s.timestamp >= cutoff]

    def get_recent_text(self, max_chars: int = 50_000) -> str:
        """Return final transcript lines newest-last, truncated to *max_chars*
        from the most recent end."""
        lines: list[str] = []
        for seg in self._segments:
            if not seg.is_final:
                continue
            # Label by role, not raw speaker index: whichever audio source the
            # user has designated as "you" is the candidate; the other is the
            # interviewer asking the questions.
            role = "You" if seg.source == self.you_source else "Interviewer"
            lines.append(f"{role}: {seg.text}")

        full_text = "\n".join(lines)

        if len(full_text) > max_chars:
            # Keep the most recent portion
            full_text = full_text[-max_chars:]

        return full_text

    def get_full_text(self, max_chars: int = 200_000) -> str:
        """Return the ENTIRE session transcript (all final segments, unpruned),
        role-labeled, for notes and summaries. Truncated from the most recent
        end only if it exceeds *max_chars*."""
        lines: list[str] = []
        for seg in self._all_finals:
            role = "You" if seg.source == self.you_source else "Interviewer"
            lines.append(f"{role}: {seg.text}")

        full_text = "\n".join(lines)
        if len(full_text) > max_chars:
            full_text = full_text[-max_chars:]
        return full_text


class PromptBuilder:
    """Assembles system + user messages within a token budget."""

    MAX_HISTORY = 50

    def __init__(self, config: Config) -> None:
        self._config = config
        self._conversation_history: list[dict] = []
        self.custom_system_prompt: str = ""

    # ------------------------------------------------------------------
    # History management
    # ------------------------------------------------------------------

    def add_to_history(self, role: str, content: str) -> None:
        """Append a turn and trim to the most recent MAX_HISTORY entries."""
        self._conversation_history.append({"role": role, "content": content})
        if len(self._conversation_history) > self.MAX_HISTORY:
            self._conversation_history = self._conversation_history[-self.MAX_HISTORY :]

    # ------------------------------------------------------------------
    # Token estimation
    # ------------------------------------------------------------------

    @staticmethod
    def estimate_tokens(text: str) -> int:
        """Fast local token estimate (~4 chars per token)."""
        return len(text) // 4

    # ------------------------------------------------------------------
    # Build
    # ------------------------------------------------------------------

    async def build(
        self,
        mode: str,
        transcript_buffer: TranscriptBuffer,
        rag_context: str | None = None,
        query: str | None = None,
        use_full_transcript: bool = False,
    ) -> BuiltPrompt:
        """Assemble a BuiltPrompt that fits within the context budget.

        When *use_full_transcript* is True, include the entire session
        transcript (for notes/summaries) instead of just the rolling window.
        """
        cfg = self._config
        budget = cfg.max_context_tokens - cfg.max_output_tokens

        # --- System prompt (5 %) ---
        system_budget_chars = int(budget * cfg.system_prompt_budget) * 4
        mode_instr = MODE_INSTRUCTIONS.get(mode, MODE_INSTRUCTIONS["active"])
        system_prompt = SYSTEM_TEMPLATE.format(mode_instructions=mode_instr)
        if self.custom_system_prompt:
            system_prompt += f"\n\nAdditional instructions:\n{self.custom_system_prompt}"
        system_prompt = system_prompt[:system_budget_chars]

        # --- Recent transcript (40 %) ---
        transcript_budget_chars = int(budget * cfg.recent_transcript_budget) * 4
        if use_full_transcript:
            transcript_text = transcript_buffer.get_full_text(
                max_chars=transcript_budget_chars
            )
        else:
            transcript_text = transcript_buffer.get_recent_text(
                max_chars=transcript_budget_chars
            )

        # --- RAG context (30 %) ---
        rag_budget_chars = int(budget * cfg.rag_budget) * 4
        rag_text = ""
        if rag_context:
            rag_text = rag_context[:rag_budget_chars]

        # --- History (25 %) ---
        history_budget_chars = int(budget * cfg.history_budget) * 4
        history_messages = self._trim_history(history_budget_chars)

        # --- Assemble user message ---
        parts: list[str] = []
        if transcript_text:
            parts.append(f"<conversation>\n{transcript_text}\n</conversation>")
        if rag_text:
            parts.append(f"<reference_docs>\n{rag_text}\n</reference_docs>")
        if query:
            parts.append(f"Specific request: {query}")
        else:
            parts.append(
                "Based on the conversation above, write the candidate's next response. "
                "Output ONLY the words to speak."
            )

        user_content = "\n\n".join(parts) if parts else ""

        messages = list(history_messages)
        if user_content:
            messages.append({"role": "user", "content": user_content})

        estimated = self.estimate_tokens(
            system_prompt + user_content + self._history_text(history_messages)
        )

        logger.debug(
            "Prompt built: ~%d tokens, %d history turns, mode=%s",
            estimated,
            len(history_messages),
            mode,
        )

        return BuiltPrompt(
            system=system_prompt,
            messages=messages,
            estimated_tokens=estimated,
        )

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _trim_history(self, budget_chars: int) -> list[dict]:
        """Return as many recent history turns as fit within *budget_chars*."""
        result: list[dict] = []
        used = 0
        for turn in reversed(self._conversation_history):
            turn_len = len(turn.get("content", ""))
            if used + turn_len > budget_chars:
                break
            result.append(turn)
            used += turn_len
        result.reverse()
        return result

    @staticmethod
    def _history_text(messages: list[dict]) -> str:
        return " ".join(m.get("content", "") for m in messages)
