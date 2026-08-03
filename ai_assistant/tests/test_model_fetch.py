"""Tests for the pinned model snapshot build helper."""

from __future__ import annotations

import importlib.util
from pathlib import Path


def _load_fetch_module():
    project_root = Path(__file__).resolve().parents[2]
    module_path = project_root / "sidecar" / "fetch_model_assets.py"
    spec = importlib.util.spec_from_file_location("fetch_model_assets", module_path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_fetch_creates_staging_directory_before_download(monkeypatch, tmp_path) -> None:
    module = _load_fetch_module()
    snapshot = tmp_path / "snapshot"
    snapshot.mkdir()
    for relative_path in module.MODEL_ASSET_ALLOWLIST:
        destination = snapshot / relative_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text("{}", encoding="utf-8")

    def fake_snapshot_download(**kwargs):
        assert list(tmp_path.glob("rag-model.staging-*"))
        assert kwargs == {
            "repo_id": module.MODEL_REPOSITORY,
            "revision": module.MODEL_REVISION,
            "allow_patterns": list(module.MODEL_ASSET_ALLOWLIST),
        }
        return str(snapshot)

    monkeypatch.setattr(module, "snapshot_download", fake_snapshot_download)

    output = module.fetch(tmp_path / "rag-model")

    assert (output / "config.json").is_file()
    assert (output / module.MODEL_MANIFEST_FILENAME).is_file()


def test_fetch_copies_only_pytorch_inference_snapshot_files(monkeypatch, tmp_path) -> None:
    module = _load_fetch_module()
    snapshot = tmp_path / "snapshot"
    snapshot.mkdir()
    for relative_path in module.MODEL_ASSET_ALLOWLIST:
        destination = snapshot / relative_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(relative_path, encoding="utf-8")
    for relative_path in (
        "onnx/model.onnx",
        "openvino/openvino_model.bin",
        "rust_model.ot",
        "flax_model.msgpack",
        "pytorch_model.bin",
        "trainer_state.json",
    ):
        destination = snapshot / relative_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text("excluded", encoding="utf-8")

    monkeypatch.setattr(module, "snapshot_download", lambda **_kwargs: str(snapshot))

    output = module.fetch(tmp_path / "rag-model")

    copied_files = {
        path.relative_to(output).as_posix()
        for path in output.rglob("*")
        if path.is_file()
    }
    assert copied_files == set(module.MODEL_ASSET_ALLOWLIST) | {
        module.MODEL_MANIFEST_FILENAME
    }
