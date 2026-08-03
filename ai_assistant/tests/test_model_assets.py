"""Tests for frozen and development RAG model resolution."""

from __future__ import annotations

import json
import sys
import types

import pytest

from ai_assistant.rag import model_assets


def test_frozen_runtime_uses_validated_bundled_model(monkeypatch, tmp_path) -> None:
    bundle = tmp_path / "rag-model"
    bundle.mkdir()
    (bundle / model_assets.MODEL_MANIFEST_FILENAME).write_text(
        json.dumps(
            {
                "repository": model_assets.MODEL_REPOSITORY,
                "revision": model_assets.MODEL_REVISION,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(sys, "_MEIPASS", str(tmp_path), raising=False)

    source = model_assets.resolve_model_source()

    assert source.path == bundle
    assert source.local_files_only is True
    assert source.bundled is True


def test_frozen_runtime_rejects_missing_or_unpinned_model(monkeypatch, tmp_path) -> None:
    (tmp_path / "rag-model").mkdir()
    monkeypatch.setattr(sys, "_MEIPASS", str(tmp_path), raising=False)

    with pytest.raises(model_assets.ModelAssetError, match="manifest"):
        model_assets.resolve_model_source()


def test_development_runtime_uses_explicit_repository_fallback(monkeypatch) -> None:
    monkeypatch.delattr(sys, "_MEIPASS", raising=False)

    source = model_assets.resolve_model_source()

    assert source.path == model_assets.MODEL_REPOSITORY
    assert source.local_files_only is False
    assert source.bundled is False


def test_frozen_embedding_and_tokenizer_use_bundled_path_offline(
    monkeypatch, tmp_path
) -> None:
    bundle = tmp_path / "rag-model"
    bundle.mkdir()
    (bundle / model_assets.MODEL_MANIFEST_FILENAME).write_text(
        json.dumps(
            {
                "repository": model_assets.MODEL_REPOSITORY,
                "revision": model_assets.MODEL_REVISION,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(sys, "_MEIPASS", str(tmp_path), raising=False)

    calls: list[tuple[str, str, object]] = []

    class FakeSentenceTransformer:
        def __init__(self, path: str, device: str, local_files_only: bool) -> None:
            calls.append(("model", path, (device, local_files_only)))

    class FakeAutoTokenizer:
        @classmethod
        def from_pretrained(cls, path: str, local_files_only: bool):
            calls.append(("tokenizer", path, local_files_only))
            return object()

    sentence_transformers = types.ModuleType("sentence_transformers")
    sentence_transformers.SentenceTransformer = FakeSentenceTransformer
    transformers = types.ModuleType("transformers")
    transformers.AutoTokenizer = FakeAutoTokenizer
    monkeypatch.setitem(sys.modules, "sentence_transformers", sentence_transformers)
    monkeypatch.setitem(sys.modules, "transformers", transformers)

    from ai_assistant.rag.chunking import RecursiveTextSplitter
    from ai_assistant.rag.embeddings import LocalEmbedder

    LocalEmbedder(device="cpu")
    RecursiveTextSplitter()._get_tokenizer()

    assert calls == [
        ("model", str(bundle), ("cpu", True)),
        ("tokenizer", str(bundle), True),
    ]
