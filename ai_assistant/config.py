"""Application configuration loaded from environment variables."""

from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass
class Config:
    # Audio
    sample_rate: int = 16000
    channels: int = 1
    audio_dtype: str = "int16"
    audio_blocksize: int = 1024  # ~64ms at 16kHz

    # Deepgram
    deepgram_api_key: str = ""
    deepgram_model: str = "nova-3"
    deepgram_language: str = "en"
    diarize: bool = True
    interim_results: bool = True
    utterance_end_ms: int = 1000
    endpointing: int = 300

    # LLM — DeepSeek V4 Flash via the Anthropic-compatible API
    # (https://api.deepseek.com/anthropic speaks the Anthropic Messages API,
    #  so the anthropic SDK works unchanged — only base_url/key/model differ).
    llm_api_key: str = ""
    llm_base_url: str = "https://api.deepseek.com/anthropic"
    llm_model: str = "deepseek-v4-flash"
    max_output_tokens: int = 4096
    max_context_tokens: int = 180_000
    temperature: float = 0.3

    # Context budget allocation (fractions of max_context_tokens - max_output_tokens)
    system_prompt_budget: float = 0.05
    recent_transcript_budget: float = 0.40
    rag_budget: float = 0.30
    history_budget: float = 0.25

    # RAG
    rag_db_path: str = "./data/rag_index"
    rag_relevance_threshold: float = 0.3
    rag_top_k: int = 5

    # Memory
    memory_window_seconds: float = 300.0

    # Reconnect
    reconnect_base_delay: float = 1.0
    reconnect_max_delay: float = 30.0
    reconnect_max_attempts: int = 10

    @classmethod
    def from_env(cls) -> Config:
        return cls(
            deepgram_api_key=os.environ.get("DEEPGRAM_API_KEY", ""),
            llm_api_key=os.environ.get("DEEPSEEK_API_KEY", ""),
            llm_base_url=os.environ.get("LLM_BASE_URL", cls.llm_base_url),
            llm_model=os.environ.get("LLM_MODEL", cls.llm_model),
            max_context_tokens=int(
                os.environ.get("MAX_CONTEXT_TOKENS", cls.max_context_tokens)
            ),
            rag_db_path=os.environ.get("RAG_DB_PATH", cls.rag_db_path),
            memory_window_seconds=float(
                os.environ.get("MEMORY_WINDOW_SECONDS", cls.memory_window_seconds)
            ),
        )
