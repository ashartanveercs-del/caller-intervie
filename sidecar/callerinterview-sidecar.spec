# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller specification for the constrained CallerInterview sidecar."""

from pathlib import Path

from PyInstaller.utils.hooks import collect_all, copy_metadata


project_root = Path(SPECPATH).parent
datas = []
binaries = []
hiddenimports = ["docx", "fitz", "pyaudiowpatch"]

# The production factory reaches these packages through lazy imports. Collecting
# their code, extension modules, and package data keeps the packaged runtime
# independent of PyInstaller's static import discovery.
for package in ("faiss", "sentence_transformers", "tokenizers", "torch", "transformers"):
    package_datas, package_binaries, package_hiddenimports = collect_all(package)
    datas += package_datas
    binaries += package_binaries
    hiddenimports += package_hiddenimports

for distribution in ("faiss-cpu", "sentence-transformers", "tokenizers", "torch", "transformers"):
    datas += copy_metadata(distribution, recursive=True)


analysis = Analysis(
    [str(project_root / "ai_assistant" / "sidecar" / "__main__.py")],
    pathex=[str(project_root)],
    binaries=binaries,
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=["PySide6", "qasync"],
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
    upx=True,
    console=True,
)
