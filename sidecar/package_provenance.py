"""Create and verify deterministic sidecar packaging provenance."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys
from typing import Iterable

PROJECT_ROOT = Path(__file__).resolve().parents[1]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from ai_assistant.rag.model_assets import MODEL_REVISION


SCHEMA_VERSION = 1
REQUIRED_FIELDS = {
    "schema_version",
    "target_triple",
    "model_revision",
    "binary_sha256",
    "packaging_input_sha256",
}


def sha256_file(path: Path) -> str:
    """Return the SHA-256 digest for a file without loading it all at once."""
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _input_paths(project_root: Path) -> Iterable[Path]:
    patterns = (
        "ai_assistant/**/*.py",
        "requirements*.txt",
        "sidecar/**/*.py",
        "sidecar/**/*.spec",
        "scripts/build-sidecar.ps1",
        "scripts/publish-sidecar-artifact.ps1",
        "scripts/test-sidecar-package.ps1",
    )
    paths = {
        path
        for pattern in patterns
        for path in project_root.glob(pattern)
        if path.is_file()
    }
    return sorted(paths, key=lambda path: path.relative_to(project_root).as_posix())


def packaging_input_sha256(project_root: Path | None = None) -> str:
    """Hash relevant source inputs using stable relative paths and file hashes."""
    project_root = project_root or PROJECT_ROOT
    digest = hashlib.sha256()
    for path in _input_paths(project_root):
        relative_path = path.relative_to(project_root).as_posix()
        digest.update(relative_path.encode("utf-8"))
        digest.update(b"\0")
        digest.update(sha256_file(path).encode("ascii"))
        digest.update(b"\n")
    return digest.hexdigest()


def create_provenance(binary: Path, target_triple: str) -> dict[str, int | str]:
    """Return the sidecar manifest with no machine-specific path information."""
    return {
        "schema_version": SCHEMA_VERSION,
        "target_triple": target_triple,
        "model_revision": MODEL_REVISION,
        "binary_sha256": sha256_file(binary),
        "packaging_input_sha256": packaging_input_sha256(),
    }


def write_provenance(binary: Path, target_triple: str, output: Path) -> dict[str, int | str]:
    """Write canonical provenance next to a staged or final sidecar."""
    provenance = create_provenance(binary, target_triple)
    output.write_text(
        json.dumps(provenance, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    return provenance


def validate_provenance(binary: Path, target_triple: str, provenance_path: Path) -> None:
    """Reject binary, source-input, target, revision, and schema mismatches."""
    try:
        provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError("sidecar provenance is unreadable") from error

    if not isinstance(provenance, dict) or set(provenance) != REQUIRED_FIELDS:
        raise ValueError("sidecar provenance schema is invalid")
    if provenance["schema_version"] != SCHEMA_VERSION:
        raise ValueError("sidecar provenance schema version is invalid")
    if provenance["target_triple"] != target_triple:
        raise ValueError("sidecar provenance target triple is invalid")
    if provenance["model_revision"] != MODEL_REVISION:
        raise ValueError("sidecar provenance model revision is invalid")
    if provenance["binary_sha256"] != sha256_file(binary):
        raise ValueError("sidecar provenance binary hash does not match")
    if provenance["packaging_input_sha256"] != packaging_input_sha256():
        raise ValueError("sidecar provenance packaging inputs do not match")


def main() -> int:
    parser = argparse.ArgumentParser()
    command = parser.add_subparsers(dest="command", required=True)
    for name in ("write", "validate"):
        subparser = command.add_parser(name)
        subparser.add_argument("--binary", required=True, type=Path)
        subparser.add_argument("--target-triple", required=True)
        subparser.add_argument("--provenance", required=True, type=Path)
    fingerprint = command.add_parser("fingerprint")
    fingerprint.add_argument("--project-root", type=Path, default=PROJECT_ROOT)
    arguments = parser.parse_args()

    if arguments.command == "write":
        write_provenance(arguments.binary, arguments.target_triple, arguments.provenance)
    elif arguments.command == "validate":
        validate_provenance(arguments.binary, arguments.target_triple, arguments.provenance)
    else:
        print(packaging_input_sha256(arguments.project_root))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
