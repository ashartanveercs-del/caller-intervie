"""Data models for the RAG pipeline."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Optional


class DocumentType(Enum):
    """Supported document types for ingestion."""

    PDF = "pdf"
    DOCX = "docx"
    TXT = "txt"
    MARKDOWN = "markdown"


@dataclass
class DocumentMetadata:
    """Metadata about an ingested document."""

    source_path: str
    doc_type: DocumentType
    title: str = ""
    total_pages: Optional[int] = None
    ingested_at: float = 0.0


@dataclass
class Chunk:
    """A chunk of text from a document."""

    text: str
    chunk_index: int
    source_path: str
    page_number: Optional[int]
    start_char: int
    end_char: int
    metadata: dict = field(default_factory=dict)


@dataclass
class ChunkWithScore:
    """A chunk paired with its relevance score."""

    chunk: Chunk
    score: float


@dataclass
class RetrievalResult:
    """Result of a retrieval query."""

    query: str
    chunks: list[ChunkWithScore]
    total_searched: int
