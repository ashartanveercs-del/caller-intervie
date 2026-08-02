"""Local embedding using SentenceTransformers."""

from __future__ import annotations

import logging

import numpy as np

logger = logging.getLogger(__name__)


class LocalEmbedder:
    """Wraps a SentenceTransformer model for encoding text into dense vectors."""

    def __init__(
        self,
        model_name: str = "all-MiniLM-L6-v2",
        device: str = "cpu",
    ) -> None:
        from sentence_transformers import SentenceTransformer

        logger.info("Loading embedding model %s on %s", model_name, device)
        # Prefer the local cache: local_files_only skips the HuggingFace Hub
        # revision check, which otherwise adds ~40s of network round-trips to
        # every startup once the model is already downloaded. Fall back to a
        # network download only on first run (or if the cache is missing).
        try:
            self._model = SentenceTransformer(
                model_name, device=device, local_files_only=True
            )
        except Exception:
            logger.info("Model not in local cache — downloading %s…", model_name)
            self._model = SentenceTransformer(model_name, device=device)

    @property
    def dimension(self) -> int:
        """Dimensionality of the embedding vectors."""
        return self._model.get_sentence_embedding_dimension()

    def embed_texts(
        self,
        texts: list[str],
        batch_size: int = 64,
        show_progress: bool = False,
    ) -> np.ndarray:
        """Encode *texts* and return a (N, dim) float32 array of unit vectors."""
        embeddings = self._model.encode(
            texts,
            batch_size=batch_size,
            show_progress_bar=show_progress,
            normalize_embeddings=True,
        )
        return np.asarray(embeddings, dtype=np.float32)

    def embed_query(self, query: str) -> np.ndarray:
        """Encode a single query and return shape (1, dim)."""
        return self.embed_texts([query])
