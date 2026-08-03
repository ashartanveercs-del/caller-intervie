"""Public models and errors for the UI-independent runtime."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal


AudioSource = Literal["mic", "system"]
RuntimeState = Literal[
    "idle", "starting", "listening", "paused", "stopping", "stopped", "error"
]
KnowledgeState = Literal["disabled", "loading", "ready", "error"]


@dataclass(frozen=True)
class SessionConfig:
    session_id: str
    mode: str
    input_language: str
    response_language: str
    review_language: str
    you_source: AudioSource
    brief_id: str


@dataclass(frozen=True)
class RuntimeSnapshot:
    state: RuntimeState
    session_id: str | None
    mode: str | None
    input_language: str | None
    response_language: str | None
    review_language: str | None
    you_source: AudioSource
    listening: bool
    system_audio_enabled: bool
    knowledge_state: KnowledgeState
    knowledge_chunks: int


@dataclass(frozen=True)
class DocumentIngestionResult:
    files: int
    chunks_added: int
    total_chunks: int


class RuntimeConfigurationError(ValueError):
    """Raised before startup when required provider credentials are absent."""

    def __init__(self, missing: list[str]) -> None:
        self.missing = missing
        super().__init__(f"Missing runtime configuration: {', '.join(missing)}")


class RuntimeStateError(RuntimeError):
    """Raised when a runtime command is invalid for the current state."""
