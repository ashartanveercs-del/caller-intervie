"""Document ingestion — reads files and extracts text by type."""

from __future__ import annotations

import logging
import time
from pathlib import Path

from ai_assistant.rag.models import DocumentMetadata, DocumentType

logger = logging.getLogger(__name__)

_SUFFIX_TO_DOCTYPE = {
    ".pdf": DocumentType.PDF,
    ".docx": DocumentType.DOCX,
    ".txt": DocumentType.TXT,
    ".md": DocumentType.MARKDOWN,
}


class DocumentIngester:
    """Reads a file from disk and returns page texts plus metadata."""

    def ingest(self, file_path: str) -> tuple[list[str], DocumentMetadata]:
        """Parse *file_path* and return (pages, metadata).

        Each element in *pages* corresponds to one logical page of text.
        For non-paginated formats the list has a single element.
        """
        path = Path(file_path)
        suffix = path.suffix.lower()
        doc_type = _SUFFIX_TO_DOCTYPE.get(suffix)
        if doc_type is None:
            raise ValueError(f"Unsupported file type: {suffix}")

        logger.info("Ingesting %s (type=%s)", path, doc_type.value)

        if doc_type is DocumentType.PDF:
            pages = self._read_pdf(path)
        elif doc_type is DocumentType.DOCX:
            pages = self._read_docx(path)
        elif doc_type in (DocumentType.TXT, DocumentType.MARKDOWN):
            pages = self._read_text(path)
        else:
            raise ValueError(f"Unsupported file type: {suffix}")

        metadata = DocumentMetadata(
            source_path=str(path),
            doc_type=doc_type,
            title=path.stem,
            total_pages=len(pages),
            ingested_at=time.time(),
        )
        logger.info(
            "Ingested %s — %d page(s), %d total chars",
            path.name,
            len(pages),
            sum(len(p) for p in pages),
        )
        return pages, metadata

    # ------------------------------------------------------------------
    # Private readers
    # ------------------------------------------------------------------

    @staticmethod
    def _read_pdf(path: Path) -> list[str]:
        import fitz  # PyMuPDF

        doc = fitz.open(path)
        pages = [doc.load_page(i).get_text("text") for i in range(len(doc))]
        doc.close()
        return pages

    @staticmethod
    def _read_docx(path: Path) -> list[str]:
        from docx import Document

        doc = Document(path)
        text = "\n".join(p.text for p in doc.paragraphs)
        return [text]

    @staticmethod
    def _read_text(path: Path) -> list[str]:
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            logger.warning("UTF-8 decode failed for %s, falling back to latin-1", path)
            text = path.read_text(encoding="latin-1")
        return [text]
