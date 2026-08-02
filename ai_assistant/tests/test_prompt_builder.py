"""Tests for TranscriptBuffer and PromptBuilder (pure logic, no network)."""

import asyncio

from ai_assistant.config import Config
from ai_assistant.core.events import TranscriptEvent
from ai_assistant.llm.prompt_builder import PromptBuilder, TranscriptBuffer


def _final(text, speaker=0, ts=0.0, is_final=True, source="mic"):
    return TranscriptEvent(
        text=text, is_final=is_final, speech_final=True, speaker=speaker,
        timestamp=ts, source=source,
    )


def test_buffer_labels_by_role_and_filters_final_only():
    buf = TranscriptBuffer(max_seconds=1000)  # default: mic = You
    buf.add(_final("hello", ts=1.0, source="mic"))
    buf.add(_final("what is a stack?", ts=1.5, source="system"))
    buf.add(_final("draft", ts=2.0, is_final=False))  # excluded
    text = buf.get_recent_text()
    assert "You: hello" in text
    assert "Interviewer: what is a stack?" in text
    assert "draft" not in text


def test_buffer_role_labels_follow_you_source():
    buf = TranscriptBuffer(max_seconds=1000)
    buf.you_source = "system"  # swap: system audio is now "You"
    buf.add(_final("my answer", ts=1.0, source="system"))
    buf.add(_final("their question", ts=1.5, source="mic"))
    text = buf.get_recent_text()
    assert "You: my answer" in text
    assert "Interviewer: their question" in text


def test_full_text_retains_beyond_prune_window():
    buf = TranscriptBuffer(max_seconds=10)
    buf.add(_final("earliest", ts=0.0, source="system"))
    buf.add(_final("latest", ts=1000.0, source="mic"))  # prunes rolling window
    # Rolling window drops the old line...
    assert "earliest" not in buf.get_recent_text()
    # ...but the full-session transcript keeps everything, role-labeled.
    full = buf.get_full_text()
    assert "Interviewer: earliest" in full
    assert "You: latest" in full


def test_buffer_prunes_old_segments():
    buf = TranscriptBuffer(max_seconds=10)
    buf.add(_final("old", ts=0.0))
    buf.add(_final("new", ts=100.0))  # prunes anything older than 90
    text = buf.get_recent_text()
    assert "new" in text
    assert "old" not in text


def test_buffer_truncates_to_max_chars_keeping_tail():
    buf = TranscriptBuffer(max_seconds=10_000)
    for i in range(50):
        buf.add(_final(f"line{i}", ts=float(i)))
    text = buf.get_recent_text(max_chars=30)
    assert len(text) <= 30
    assert "line49" in text  # newest kept
    assert "line0" not in text  # oldest dropped


def test_history_trims_to_max():
    pb = PromptBuilder(Config())
    for i in range(PromptBuilder.MAX_HISTORY + 10):
        pb.add_to_history("user", f"m{i}")
    assert len(pb._conversation_history) == PromptBuilder.MAX_HISTORY
    assert pb._conversation_history[-1]["content"] == f"m{PromptBuilder.MAX_HISTORY + 9}"


def test_estimate_tokens():
    assert PromptBuilder.estimate_tokens("a" * 40) == 10


def test_build_assembles_query_and_context():
    pb = PromptBuilder(Config())
    buf = TranscriptBuffer(max_seconds=1000)
    buf.add(_final("What is a hash map?", speaker=1, ts=1.0))

    prompt = asyncio.run(
        pb.build(
            mode="active",
            transcript_buffer=buf,
            rag_context="Reference material about maps.",
            query="Explain hash maps",
        )
    )

    assert "interview" in prompt.system.lower()
    user_msg = prompt.messages[-1]["content"]
    assert "<conversation>" in user_msg
    assert "What is a hash map?" in user_msg
    assert "<reference_docs>" in user_msg
    assert "Explain hash maps" in user_msg
    assert prompt.estimated_tokens > 0


def test_build_custom_system_prompt_appended():
    pb = PromptBuilder(Config())
    pb.custom_system_prompt = "Answer as a staff engineer."
    prompt = asyncio.run(
        pb.build(mode="active", transcript_buffer=TranscriptBuffer(), query="hi")
    )
    assert "staff engineer" in prompt.system
