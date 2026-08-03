"""Fetch the exact model snapshot that is embedded in the sidecar build."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import sys
import uuid

from huggingface_hub import snapshot_download

PROJECT_ROOT = Path(__file__).resolve().parents[1]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from ai_assistant.rag.model_assets import (
    MODEL_MANIFEST_FILENAME,
    MODEL_REPOSITORY,
    MODEL_REVISION,
)


MODEL_ASSET_ALLOWLIST = (
    "model.safetensors",
    "config.json",
    "config_sentence_transformers.json",
    "modules.json",
    "sentence_bert_config.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
    "vocab.txt",
    "1_Pooling/config.json",
)


def fetch(output: Path) -> Path:
    staging = output.with_name(f"{output.name}.staging-{uuid.uuid4().hex}")
    try:
        staging.mkdir(parents=True, exist_ok=False)
        snapshot = Path(snapshot_download(
            repo_id=MODEL_REPOSITORY,
            revision=MODEL_REVISION,
            allow_patterns=list(MODEL_ASSET_ALLOWLIST),
        ))
        for relative_path in MODEL_ASSET_ALLOWLIST:
            source = snapshot / relative_path
            if not source.is_file():
                raise FileNotFoundError(f"Pinned model asset missing: {relative_path}")
            destination = staging / relative_path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
        (staging / MODEL_MANIFEST_FILENAME).write_text(
            json.dumps(
                {"repository": MODEL_REPOSITORY, "revision": MODEL_REVISION},
                separators=(",", ":"),
            ),
            encoding="utf-8",
        )
        if output.exists():
            shutil.rmtree(output)
        staging.replace(output)
    except Exception:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    return output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    print(fetch(arguments.output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
