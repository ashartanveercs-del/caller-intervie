"""FAISS-backed vector store with parallel metadata tracking."""

from __future__ import annotations

import logging
import os
import pickle
from pathlib import Path
from typing import Optional

import faiss
import numpy as np

from ai_assistant.rag.models import Chunk

logger = logging.getLogger(__name__)

_INDEX_FILENAME = "index.faiss"
_CHUNKS_FILENAME = "chunks.pkl"


class FAISSVectorStore:
    """Cosine-similarity store using FAISS ``IndexFlatIP`` on normalised vectors."""

    def __init__(
        self,
        dimension: int = 384,
        index_path: Optional[str] = None,
    ) -> None:
        self._dimension = dimension
        self._index: faiss.IndexFlatIP = faiss.IndexFlatIP(dimension)
        self._chunks: list[Chunk] = []

        if index_path is not None:
            self.load(index_path)

    # ------------------------------------------------------------------
    # Core operations
    # ------------------------------------------------------------------

    def add(self, embeddings: np.ndarray, chunks: list[Chunk]) -> None:
        """Add *embeddings* and their corresponding *chunks* to the store."""
        if len(embeddings) != len(chunks):
            raise ValueError(
                f"Embedding count ({len(embeddings)}) != chunk count ({len(chunks)})"
            )
        self._index.add(embeddings)
        self._chunks.extend(chunks)
        logger.info("Added %d vectors (total: %d)", len(chunks), self._index.ntotal)

    def search(
        self,
        query_vector: np.ndarray,
        k: int = 5,
    ) -> list[tuple[int, float]]:
        """Return up to *k* (index, score) pairs sorted by descending score."""
        effective_k = min(k, self._index.ntotal)
        if effective_k == 0:
            return []
        distances, indices = self._index.search(query_vector, effective_k)
        results: list[tuple[int, float]] = []
        for idx, score in zip(indices[0], distances[0]):
            if idx == -1:
                continue
            results.append((int(idx), float(score)))
        return results

    def get_chunk(self, index: int) -> Chunk:
        """Retrieve the chunk at *index*."""
        return self._chunks[index]

    # ------------------------------------------------------------------
    # Persistence
    # ------------------------------------------------------------------

    def save(self, directory: str) -> None:
        """Persist the FAISS index and chunk metadata to *directory*."""
        os.makedirs(directory, exist_ok=True)
        index_path = os.path.join(directory, _INDEX_FILENAME)
        chunks_path = os.path.join(directory, _CHUNKS_FILENAME)

        faiss.write_index(self._index, index_path)
        with open(chunks_path, "wb") as f:
            pickle.dump(self._chunks, f)
        logger.info("Saved index (%d vectors) to %s", self._index.ntotal, directory)

    def load(self, directory: str) -> None:
        """Load a previously saved index from *directory*."""
        index_path = os.path.join(directory, _INDEX_FILENAME)
        chunks_path = os.path.join(directory, _CHUNKS_FILENAME)

        self._index = faiss.read_index(index_path)
        with open(chunks_path, "rb") as f:
            self._chunks = pickle.load(f)
        logger.info("Loaded index (%d vectors) from %s", self._index.ntotal, directory)

    # ------------------------------------------------------------------
    # Properties
    # ------------------------------------------------------------------

    @property
    def size(self) -> int:
        """Number of vectors currently stored."""
        return self._index.ntotal

    # ------------------------------------------------------------------
    # Source management
    # ------------------------------------------------------------------

    def remove_by_source(self, source_path: str) -> None:
        """Remove all chunks originating from *source_path* and rebuild the index."""
        keep_indices = [
            i for i, c in enumerate(self._chunks) if c.source_path != source_path
        ]
        removed = len(self._chunks) - len(keep_indices)

        if removed == 0:
            logger.info("No chunks found for source %s", source_path)
            return

        # Reconstruct kept embeddings
        if keep_indices:
            kept_embeddings = np.vstack(
                [self._index.reconstruct(i) for i in keep_indices]
            ).astype(np.float32)
            kept_chunks = [self._chunks[i] for i in keep_indices]
        else:
            kept_embeddings = np.empty((0, self._dimension), dtype=np.float32)
            kept_chunks = []

        # Rebuild
        self._index = faiss.IndexFlatIP(self._dimension)
        self._chunks = kept_chunks
        if len(kept_embeddings) > 0:
            self._index.add(kept_embeddings)

        logger.info(
            "Removed %d chunks for %s (%d remaining)",
            removed,
            source_path,
            self._index.ntotal,
        )
