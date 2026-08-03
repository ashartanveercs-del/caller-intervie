"""Local embedding using SentenceTransformers."""

from __future__ import annotations

import logging

import numpy as np

from ai_assistant.rag.model_assets import MODEL_REPOSITORY, resolve_model_source

logger = logging.getLogger(__name__)


class LocalEmbedder:
    """Wrap a SentenceTransformer model for dense-vector encoding."""

    def __init__(
        self,
        model_name: str = "all-MiniLM-L6-v2",
        device: str = "cpu",
    ) -> None:
        from sentence_transformers import SentenceTransformer

        source = resolve_model_source()
        logger.info("Loading embedding model %s on %s", source.path, device)
        if source.bundled:
            self._model = SentenceTransformer(
                str(source.path), device=device, local_files_only=True
            )
            return

        # Development remains cache-first, with an explicit initial-download path.
        try:
            self._model = SentenceTransformer(
                model_name, device=device, local_files_only=True
            )
        except Exception:
            logger.info("Model not in local cache; downloading %s", MODEL_REPOSITORY)
            self._model = SentenceTransformer(MODEL_REPOSITORY, device=device)

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
        """Encode texts into a (N, dim) float32 array of unit vectors."""
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
