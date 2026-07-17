"""Tests for DocumentIngester text/markdown reading and type detection."""

import pytest

from ai_assistant.rag.ingestion import DocumentIngester
from ai_assistant.rag.models import DocumentType


def test_ingest_txt(tmp_path):
    f = tmp_path / "notes.txt"
    f.write_text("hello world", encoding="utf-8")

    pages, meta = DocumentIngester().ingest(str(f))
    assert pages == ["hello world"]
    assert meta.doc_type is DocumentType.TXT
    assert meta.title == "notes"
    assert meta.total_pages == 1


def test_ingest_markdown(tmp_path):
    f = tmp_path / "readme.md"
    f.write_text("# Heading\n\nBody text.", encoding="utf-8")

    pages, meta = DocumentIngester().ingest(str(f))
    assert "# Heading" in pages[0]
    assert meta.doc_type is DocumentType.MARKDOWN


def test_latin1_fallback(tmp_path):
    f = tmp_path / "legacy.txt"
    f.write_bytes(b"caf\xe9")  # invalid utf-8, valid latin-1

    pages, _ = DocumentIngester().ingest(str(f))
    assert pages[0] == "café"


def test_unsupported_type_raises(tmp_path):
    f = tmp_path / "data.xyz"
    f.write_text("x", encoding="utf-8")
    with pytest.raises(ValueError):
        DocumentIngester().ingest(str(f))
