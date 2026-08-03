"""High-level RAG retriever that ties ingestion, chunking, embedding, and search."""

from __future__ import annotations

from functools import wraps
import logging
import os
from pathlib import Path
import threading
from typing import Optional

from ai_assistant.rag.chunking import RecursiveTextSplitter
from ai_assistant.rag.embeddings import LocalEmbedder
from ai_assistant.rag.ingestion import DocumentIngester, _SUFFIX_TO_DOCTYPE
from ai_assistant.rag.models import ChunkWithScore, RetrievalResult
from ai_assistant.rag.vector_store import FAISSVectorStore

logger = logging.getLogger(__name__)


def _with_store_lock(method):
    @wraps(method)
    def locked(self, *args, **kwargs):
        with self._store_lock:
            return method(self, *args, **kwargs)

    return locked


class RAGRetriever:
    """Orchestrates the full ingest-embed-search pipeline."""

    def __init__(
        self,
        embedder: LocalEmbedder,
        vector_store: FAISSVectorStore,
        relevance_threshold: float = 0.3,
        default_k: int = 5,
    ) -> None:
        self.embedder = embedder
        self.vector_store = vector_store
        self.relevance_threshold = relevance_threshold
        self.default_k = default_k
        self._store_lock = threading.RLock()

        self._ingester = DocumentIngester()
        self._splitter = RecursiveTextSplitter()

    # ------------------------------------------------------------------
    # Ingestion
    # ------------------------------------------------------------------

    @_with_store_lock
    def ingest_file(self, file_path: str) -> int:
        """Ingest a single file and return the number of chunks produced."""
        pages, _metadata = self._ingester.ingest(file_path)
        chunks = self._splitter.split_pages(pages, source_path=file_path)

        if not chunks:
            self.vector_store.remove_by_source(file_path)
            logger.warning("No chunks produced from %s", file_path)
            return 0

        texts = [c.text for c in chunks]
        embeddings = self.embedder.embed_texts(texts)
        self.vector_store.remove_by_source(file_path)
        self.vector_store.add(embeddings, chunks)

        logger.info("Ingested %s — %d chunks added to store", file_path, len(chunks))
        return len(chunks)

    @_with_store_lock
    def ingest_directory(
        self,
        dir_path: str,
        extensions: Optional[list[str]] = None,
    ) -> dict[str, int]:
        """Walk *dir_path* and ingest all supported files.

        Returns a mapping of file path to chunk count.
        """
        if extensions is None:
            extensions = list(_SUFFIX_TO_DOCTYPE.keys())
        else:
            extensions = [ext if ext.startswith(".") else f".{ext}" for ext in extensions]

        results: dict[str, int] = {}
        for root, _dirs, files in os.walk(dir_path):
            for fname in files:
                if Path(fname).suffix.lower() not in extensions:
                    continue
                full_path = os.path.join(root, fname)
                try:
                    count = self.ingest_file(full_path)
                    results[full_path] = count
                except Exception:
                    logger.exception("Failed to ingest %s", full_path)

        logger.info(
            "Directory ingestion complete: %d files, %d total chunks",
            len(results),
            sum(results.values()),
        )
        return results

    # ------------------------------------------------------------------
    # Query
    # ------------------------------------------------------------------

    @_with_store_lock
    def query(
        self,
        query_text: str,
        k: Optional[int] = None,
    ) -> RetrievalResult:
        """Embed *query_text*, search the store, and return filtered results."""
        k = k or self.default_k
        query_vector = self.embedder.embed_query(query_text)
        raw_results = self.vector_store.search(query_vector, k=k)

        chunks_with_scores: list[ChunkWithScore] = []
        for idx, score in raw_results:
            if score < self.relevance_threshold:
                continue
            chunk = self.vector_store.get_chunk(idx)
            chunks_with_scores.append(ChunkWithScore(chunk=chunk, score=score))

        logger.info(
            "Query '%s' — %d/%d results above threshold %.2f",
            query_text[:80],
            len(chunks_with_scores),
            len(raw_results),
            self.relevance_threshold,
        )
        return RetrievalResult(
            query=query_text,
            chunks=chunks_with_scores,
            total_searched=self.vector_store.size,
        )

    # ------------------------------------------------------------------
    # Persistence helpers
    # ------------------------------------------------------------------

    @_with_store_lock
    def save_index(self, directory: str) -> None:
        """Save the vector store to disk."""
        self.vector_store.save(directory)

    @_with_store_lock
    def load_index(self, directory: str) -> None:
        """Load a previously saved vector store."""
        self.vector_store.load(directory)

    # ------------------------------------------------------------------
    # Source management
    # ------------------------------------------------------------------

    @_with_store_lock
    def remove_source(self, file_path: str) -> None:
        """Remove all chunks for *file_path* from the store."""
        self.vector_store.remove_by_source(file_path)
