"""Tests for Config — env loading and DeepSeek defaults."""

from ai_assistant.config import Config


def test_defaults_point_at_deepseek():
    cfg = Config()
    assert cfg.llm_model == "deepseek-v4-flash"
    assert cfg.llm_base_url == "https://api.deepseek.com/anthropic"


def test_from_env_reads_deepseek_key(monkeypatch):
    monkeypatch.setenv("DEEPSEEK_API_KEY", "sk-secret")
    monkeypatch.setenv("DEEPGRAM_API_KEY", "dg-secret")
    monkeypatch.delenv("LLM_MODEL", raising=False)
    monkeypatch.delenv("LLM_BASE_URL", raising=False)

    cfg = Config.from_env()
    assert cfg.llm_api_key == "sk-secret"
    assert cfg.deepgram_api_key == "dg-secret"
    # Falls back to defaults when unset
    assert cfg.llm_model == "deepseek-v4-flash"
    assert cfg.llm_base_url == "https://api.deepseek.com/anthropic"


def test_from_env_overrides(monkeypatch):
    monkeypatch.setenv("DEEPSEEK_API_KEY", "k")
    monkeypatch.setenv("DEEPGRAM_API_KEY", "d")
    monkeypatch.setenv("LLM_MODEL", "deepseek-v4-pro")
    monkeypatch.setenv("LLM_BASE_URL", "https://example.test/anthropic")
    monkeypatch.setenv("MAX_CONTEXT_TOKENS", "64000")

    cfg = Config.from_env()
    assert cfg.llm_model == "deepseek-v4-pro"
    assert cfg.llm_base_url == "https://example.test/anthropic"
    assert cfg.max_context_tokens == 64000


def test_missing_keys_default_empty(monkeypatch):
    monkeypatch.delenv("DEEPSEEK_API_KEY", raising=False)
    monkeypatch.delenv("DEEPGRAM_API_KEY", raising=False)
    cfg = Config.from_env()
    assert cfg.llm_api_key == ""
    assert cfg.deepgram_api_key == ""
