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
        "scripts/sidecar-artifact-transaction.psm1",
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


def create_build_receipt(
    binary: Path, target_triple: str, packaging_input_hash: str | None = None
) -> dict[str, int | str]:
    """Return a receipt bound to the source inputs observed for this build."""
    current_input_hash = packaging_input_sha256()
    if packaging_input_hash is None:
        packaging_input_hash = current_input_hash
    if packaging_input_hash != current_input_hash:
        raise ValueError("build receipt packaging inputs do not match")
    return {
        "schema_version": SCHEMA_VERSION,
        "target_triple": target_triple,
        "model_revision": MODEL_REVISION,
        "binary_sha256": sha256_file(binary),
        "packaging_input_sha256": packaging_input_hash,
    }


def write_build_receipt(
    binary: Path,
    target_triple: str,
    output: Path,
    packaging_input_hash: str | None = None,
) -> dict[str, int | str]:
    """Write canonical build receipt next to the PyInstaller output."""
    receipt = create_build_receipt(binary, target_triple, packaging_input_hash)
    output.write_text(
        json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    return receipt


def validate_build_receipt(binary: Path, target_triple: str, receipt_path: Path) -> None:
    """Reject receipt, binary, target, revision, and source-input mismatches."""
    try:
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError("sidecar build receipt is unreadable") from error

    if not isinstance(receipt, dict) or set(receipt) != REQUIRED_FIELDS:
        raise ValueError("sidecar build receipt schema is invalid")
    if receipt["schema_version"] != SCHEMA_VERSION:
        raise ValueError("sidecar build receipt schema version is invalid")
    if receipt["target_triple"] != target_triple:
        raise ValueError("sidecar build receipt target triple is invalid")
    if receipt["model_revision"] != MODEL_REVISION:
        raise ValueError("sidecar build receipt model revision is invalid")
    if receipt["binary_sha256"] != sha256_file(binary):
        raise ValueError("sidecar build receipt binary hash does not match")
    if receipt["packaging_input_sha256"] != packaging_input_sha256():
        raise ValueError("sidecar build receipt packaging inputs do not match")


# Compatibility aliases keep the smoke contract readable while publication only
# accepts receipts produced by the build flow.
create_provenance = create_build_receipt
write_provenance = write_build_receipt
validate_provenance = validate_build_receipt


def main() -> int:
    parser = argparse.ArgumentParser()
    command = parser.add_subparsers(dest="command", required=True)
    for name in ("write-receipt", "validate-receipt"):
        subparser = command.add_parser(name)
        subparser.add_argument("--binary", required=True, type=Path)
        subparser.add_argument("--target-triple", required=True)
        subparser.add_argument("--receipt", required=True, type=Path)
        if name == "write-receipt":
            subparser.add_argument("--packaging-input-sha256", required=True)
    fingerprint = command.add_parser("fingerprint")
    fingerprint.add_argument("--project-root", type=Path, default=PROJECT_ROOT)
    arguments = parser.parse_args()

    if arguments.command == "write-receipt":
        write_build_receipt(
            arguments.binary,
            arguments.target_triple,
            arguments.receipt,
            arguments.packaging_input_sha256,
        )
    elif arguments.command == "validate-receipt":
        validate_build_receipt(arguments.binary, arguments.target_triple, arguments.receipt)
    else:
        print(packaging_input_sha256(arguments.project_root))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
