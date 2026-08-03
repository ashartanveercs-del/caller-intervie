"""Offline verification for the frozen Python sidecar payload."""

from __future__ import annotations

import numpy as np

from ai_assistant.rag.model_assets import MODEL_REVISION, resolve_model_source


def run() -> dict[str, int | str]:
    """Exercise bundled CPU inference and native runtime imports without hardware."""
    import faiss
    import fitz  # noqa: F401
    import docx  # noqa: F401
    import pyaudiowpatch  # noqa: F401
    from sentence_transformers import SentenceTransformer
    from transformers import AutoTokenizer

    source = resolve_model_source()
    if not source.bundled:
        raise RuntimeError("sidecar self-test requires the bundled pinned model")

    tokenizer = AutoTokenizer.from_pretrained(
        str(source.path), local_files_only=True
    )
    model = SentenceTransformer(
        str(source.path), device="cpu", local_files_only=True
    )
    embeddings = np.asarray(
        model.encode(["callerinterview frozen self test"], normalize_embeddings=True),
        dtype=np.float32,
    )
    index = faiss.IndexFlatIP(embeddings.shape[1])
    index.add(embeddings)
    _, indices = index.search(embeddings, 1)
    if indices.tolist() != [[0]]:
        raise RuntimeError("FAISS self-test did not return the indexed vector")

    tokenizer.encode("callerinterview frozen self test")
    return {
        "status": "ok",
        "model_revision": MODEL_REVISION,
        "embedding_dimension": int(embeddings.shape[1]),
        "faiss_top_index": int(indices[0, 0]),
    }
