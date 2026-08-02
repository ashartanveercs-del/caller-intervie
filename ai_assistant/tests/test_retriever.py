"""Regression tests for RAG ingestion behavior."""

import numpy as np

from ai_assistant.rag.retriever import RAGRetriever
from ai_assistant.rag.vector_store import FAISSVectorStore


class _DeterministicEmbedder:
    def embed_texts(self, texts: list[str]) -> np.ndarray:
        embeddings = np.ones((len(texts), 3), dtype=np.float32)
        return embeddings / np.linalg.norm(embeddings, axis=1, keepdims=True)


def test_reingesting_same_source_replaces_existing_chunks(tmp_path):
    source = tmp_path / "resume.txt"
    source.write_text("Python systems design and distributed queues.", encoding="utf-8")
    retriever = RAGRetriever(_DeterministicEmbedder(), FAISSVectorStore(dimension=3))

    first_count = retriever.ingest_file(str(source))
    retriever.ingest_file(str(source))

    assert first_count > 0
    assert retriever.vector_store.size == first_count
