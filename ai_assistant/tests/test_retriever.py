"""Regression tests for RAG ingestion behavior."""

import threading

import numpy as np
import pytest

from ai_assistant.rag.models import Chunk
from ai_assistant.rag.retriever import RAGRetriever
from ai_assistant.rag.vector_store import FAISSVectorStore


class _DeterministicEmbedder:
    def embed_texts(self, texts: list[str]) -> np.ndarray:
        embeddings = np.ones((len(texts), 3), dtype=np.float32)
        return embeddings / np.linalg.norm(embeddings, axis=1, keepdims=True)

    def embed_query(self, _text: str) -> np.ndarray:
        return np.ones((1, 3), dtype=np.float32)


class _StaticIngester:
    def ingest(self, _file_path: str):
        return ["replacement text"], object()


class _StaticSplitter:
    def split_pages(self, _pages, source_path: str):
        return [
            Chunk(
                text="replacement text",
                chunk_index=0,
                source_path=source_path,
                page_number=1,
                start_char=0,
                end_char=16,
            )
        ]


class _GuardedVectorStore:
    def __init__(self) -> None:
        self.mutation_started = threading.Event()
        self.release_mutation = threading.Event()
        self.read_entered = threading.Event()
        self.mutating = False
        self.chunks = [
            Chunk("old", 0, "resume.txt", 1, 0, 3),
        ]

    @property
    def size(self) -> int:
        return len(self.chunks)

    def remove_by_source(self, _source_path: str) -> None:
        self.mutating = True
        self.mutation_started.set()
        if not self.release_mutation.wait(timeout=1):
            raise TimeoutError("test did not release mutation")
        self.chunks = []

    def add(self, _embeddings, chunks: list[Chunk]) -> None:
        self.chunks.extend(chunks)
        self.mutating = False

    def search(self, _query_vector, k: int):
        if self.mutating:
            self.read_entered.set()
        return [(0, 1.0)] if self.chunks and k else []

    def get_chunk(self, index: int) -> Chunk:
        return self.chunks[index]

    def save(self, _directory: str) -> None:
        if self.mutating:
            self.read_entered.set()

    def load(self, _directory: str) -> None:
        pass


def test_reingesting_same_source_replaces_existing_chunks(tmp_path):
    source = tmp_path / "resume.txt"
    source.write_text("Python systems design and distributed queues.", encoding="utf-8")
    retriever = RAGRetriever(_DeterministicEmbedder(), FAISSVectorStore(dimension=3))

    first_count = retriever.ingest_file(str(source))
    retriever.ingest_file(str(source))

    assert first_count > 0
    assert retriever.vector_store.size == first_count


@pytest.mark.parametrize("operation", ["query", "save"])
def test_query_and_save_cannot_overlap_ingestion_mutation(operation: str) -> None:
    store = _GuardedVectorStore()
    retriever = RAGRetriever(_DeterministicEmbedder(), store)
    retriever._ingester = _StaticIngester()
    retriever._splitter = _StaticSplitter()

    ingest_thread = threading.Thread(
        target=retriever.ingest_file,
        args=("resume.txt",),
    )
    ingest_thread.start()
    assert store.mutation_started.wait(timeout=1)

    if operation == "query":
        read_thread = threading.Thread(target=retriever.query, args=("python",))
    else:
        read_thread = threading.Thread(target=retriever.save_index, args=("index",))
    read_thread.start()

    overlapped = store.read_entered.wait(timeout=0.1)
    store.release_mutation.set()
    ingest_thread.join(timeout=1)
    read_thread.join(timeout=1)

    assert overlapped is False
    assert not ingest_thread.is_alive()
    assert not read_thread.is_alive()
    assert store.size == len(store.chunks) == 1
