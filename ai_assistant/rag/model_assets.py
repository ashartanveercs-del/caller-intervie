"""Pinned local model resolution for development and frozen sidecars."""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import sys


MODEL_REPOSITORY = "sentence-transformers/all-MiniLM-L6-v2"
MODEL_REVISION = "1110a243fdf4706b3f48f1d95db1a4f5529b4d41"
MODEL_DIRECTORY_NAME = "rag-model"
MODEL_MANIFEST_FILENAME = ".callerinterview-model.json"


class ModelAssetError(RuntimeError):
    """Raised when a frozen sidecar lacks its verified model snapshot."""


@dataclass(frozen=True)
class ModelSource:
    path: Path | str
    local_files_only: bool
    bundled: bool


def resolve_model_source() -> ModelSource:
    """Return the pinned bundled model in a frozen app or the dev repository."""
    frozen_root = getattr(sys, "_MEIPASS", None)
    if frozen_root is None:
        return ModelSource(MODEL_REPOSITORY, local_files_only=False, bundled=False)

    model_path = Path(frozen_root) / MODEL_DIRECTORY_NAME
    manifest_path = model_path / MODEL_MANIFEST_FILENAME
    if not manifest_path.is_file():
        raise ModelAssetError("bundled RAG model manifest is missing")

    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ModelAssetError("bundled RAG model manifest is invalid") from error

    if manifest != {"repository": MODEL_REPOSITORY, "revision": MODEL_REVISION}:
        raise ModelAssetError("bundled RAG model manifest is not the pinned revision")
    return ModelSource(model_path, local_files_only=True, bundled=True)
