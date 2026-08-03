# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller specification for the constrained CallerInterview sidecar."""

import os
from pathlib import Path

from PyInstaller.utils.hooks import (
    collect_data_files,
    collect_dynamic_libs,
    collect_submodules,
    copy_metadata,
)


project_root = Path(SPECPATH).parent
model_assets = project_root / "sidecar" / "build" / "rag-model"
if not model_assets.is_dir():
    raise RuntimeError("Pinned RAG model assets are missing; run scripts/build-sidecar.ps1")

datas = [(str(model_assets), "rag-model")]
datas += collect_data_files("sentence_transformers", excludes=["**/tests/**"])
datas += collect_data_files(
    "transformers", excludes=["**/tests/**", "**/models/**/convert_*.py"]
)
binaries = collect_dynamic_libs("faiss")
hiddenimports = [
    "docx",
    "faiss",
    "faiss.loader",
    "faiss.swigfaiss",
    "fitz",
    "pyaudiowpatch",
    "safetensors",
    "sentence_transformers.sentence_transformer.model",
    "sentence_transformers.sentence_transformer.modules.transformer",
    "sentence_transformers.sentence_transformer.modules.pooling",
    "scipy._external.array_api_compat.numpy.fft",
    "scipy._external.array_api_compat.numpy.linalg",
    "tokenizers",
    "transformers.models.bert.configuration_bert",
    "transformers.models.bert.modeling_bert",
    "transformers.models.bert.tokenization_bert",
]

for distribution in (
    "faiss-cpu",
    "sentence-transformers",
    "tokenizers",
    "torch",
    "transformers",
    "safetensors",
):
    datas += copy_metadata(distribution, recursive=True)


analysis = Analysis(
    [str(project_root / "ai_assistant" / "sidecar" / "__main__.py")],
    pathex=[str(project_root)],
    binaries=binaries,
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[str(project_root / "sidecar" / "hooks")],
    hooksconfig={},
    runtime_hooks=[],
    excludes=["PySide6", "qasync", "tensorboard", "torchaudio", "torchvision"],
    noarchive=False,
)
pyz = PYZ(analysis.pure)

exe = EXE(
    pyz,
    analysis.scripts,
    analysis.binaries,
    analysis.zipfiles,
    analysis.datas,
    [],
    name="callerinterview-sidecar",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=os.environ.get("SIDECAR_USE_UPX") == "1",
    console=True,
)
