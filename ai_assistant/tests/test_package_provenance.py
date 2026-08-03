"""Tests for deterministic sidecar packaging provenance."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from sidecar import package_provenance


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def _fixture_project(tmp_path: Path) -> Path:
    _write(tmp_path / "ai_assistant" / "sidecar.py", "sidecar = 1\n")
    _write(tmp_path / "requirements-dev.txt", "pyinstaller==6.16.0\n")
    _write(tmp_path / "sidecar" / "hook.py", "hook = 1\n")
    _write(tmp_path / "sidecar" / "sidecar.spec", "spec = 1\n")
    _write(tmp_path / "scripts" / "build-sidecar.ps1", "$value = 1\n")
    return tmp_path


def test_packaging_input_hash_is_stable_and_changes_with_input(tmp_path) -> None:
    project_root = _fixture_project(tmp_path)

    first = package_provenance.packaging_input_sha256(project_root)
    second = package_provenance.packaging_input_sha256(project_root)
    _write(project_root / "scripts" / "build-sidecar.ps1", "$value = 2\n")

    assert first == second
    assert first != package_provenance.packaging_input_sha256(project_root)


def test_provenance_validation_rejects_changed_packaging_input(monkeypatch, tmp_path) -> None:
    project_root = _fixture_project(tmp_path)
    binary = tmp_path / "callerinterview-sidecar.exe"
    provenance = tmp_path / "callerinterview-sidecar.exe.provenance.json"
    binary.write_bytes(b"sidecar")
    monkeypatch.setattr(package_provenance, "PROJECT_ROOT", project_root)

    package_provenance.write_provenance(binary, "x86_64-pc-windows-msvc", provenance)
    _write(project_root / "sidecar" / "hook.py", "hook = 2\n")

    with pytest.raises(ValueError, match="packaging inputs"):
        package_provenance.validate_provenance(
            binary, "x86_64-pc-windows-msvc", provenance
        )


def test_written_provenance_is_relative_and_canonical(monkeypatch, tmp_path) -> None:
    project_root = _fixture_project(tmp_path)
    binary = tmp_path / "callerinterview-sidecar.exe"
    provenance = tmp_path / "sidecar.provenance.json"
    binary.write_bytes(b"sidecar")
    monkeypatch.setattr(package_provenance, "PROJECT_ROOT", project_root)

    package_provenance.write_provenance(binary, "x86_64-pc-windows-msvc", provenance)

    manifest = json.loads(provenance.read_text(encoding="utf-8"))
    assert set(manifest) == package_provenance.REQUIRED_FIELDS
    assert str(project_root) not in provenance.read_text(encoding="utf-8")
