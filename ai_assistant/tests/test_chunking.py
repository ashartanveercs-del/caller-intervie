"""Tests for RecursiveTextSplitter using a fake char-based tokenizer.

The real splitter loads a HuggingFace tokenizer lazily; injecting a fake one
(1 token per character) keeps these tests fast and offline while still
exercising the split/merge/overlap logic.
"""

from ai_assistant.rag.chunking import RecursiveTextSplitter


class FakeTokenizer:
    """1 token == 1 character, so token budgets map to character counts."""

    def encode(self, text):
        return [ord(c) for c in text]

    def decode(self, ids, skip_special_tokens=True):
        return "".join(chr(i) for i in ids)


def _splitter(chunk_size=20, chunk_overlap=5):
    s = RecursiveTextSplitter(chunk_size=chunk_size, chunk_overlap=chunk_overlap)
    s._tokenizer = FakeTokenizer()  # bypass the lazy HF download
    return s


def test_short_text_single_chunk():
    s = _splitter(chunk_size=100)
    chunks = s.split_pages(["short text"], source_path="a.txt")
    assert len(chunks) == 1
    assert chunks[0].text == "short text"
    assert chunks[0].chunk_index == 0
    assert chunks[0].source_path == "a.txt"
    assert chunks[0].page_number == 0


def test_long_text_splits_into_multiple_chunks():
    page = "word " * 40  # 200 chars -> many 20-token chunks
    s = _splitter(chunk_size=20, chunk_overlap=5)
    chunks = s.split_pages([page], source_path="doc.txt")
    assert len(chunks) > 1
    # Indices are sequential and start at 0
    assert [c.chunk_index for c in chunks] == list(range(len(chunks)))
    # No chunk grossly exceeds the budget (allow overlap headroom)
    for c in chunks:
        assert len(c.text) <= 20 + 5


def test_empty_and_whitespace_pages_skipped():
    s = _splitter(chunk_size=20)
    chunks = s.split_pages(["", "   \n  ", "real content here"], source_path="d.txt")
    assert all(c.text.strip() for c in chunks)
    # page_number should reflect the page the content came from (index 2)
    assert chunks[0].page_number == 2


def test_multi_page_indices_are_global():
    s = _splitter(chunk_size=15, chunk_overlap=3)
    chunks = s.split_pages(["alpha beta gamma delta", "one two three four five"], "m.txt")
    assert [c.chunk_index for c in chunks] == list(range(len(chunks)))
    pages = {c.page_number for c in chunks}
    assert pages == {0, 1}
