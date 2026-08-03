"""Recursive text splitter that respects token budgets."""

from __future__ import annotations

import logging
from typing import Optional

from ai_assistant.rag.model_assets import resolve_model_source
from ai_assistant.rag.models import Chunk

logger = logging.getLogger(__name__)

DEFAULT_SEPARATORS = ["\n\n", "\n", ". ", " "]


class RecursiveTextSplitter:
    """Split text into token-bounded chunks with configurable overlap."""

    def __init__(
        self,
        chunk_size: int = 512,
        chunk_overlap: int = 64,
        separators: Optional[list[str]] = None,
    ) -> None:
        self.chunk_size = chunk_size
        self.chunk_overlap = chunk_overlap
        self.separators = separators or list(DEFAULT_SEPARATORS)
        self._tokenizer = None  # lazy-loaded

    # ------------------------------------------------------------------
    # Tokenizer helpers
    # ------------------------------------------------------------------

    def _get_tokenizer(self):
        if self._tokenizer is None:
            from transformers import AutoTokenizer

            logger.info("Loading tokenizer for token counting…")
            source = resolve_model_source()
            self._tokenizer = AutoTokenizer.from_pretrained(
                str(source.path), local_files_only=source.local_files_only
            )
        return self._tokenizer

    def _count_tokens(self, text: str) -> int:
        return len(self._get_tokenizer().encode(text))

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def split_pages(self, pages: list[str], source_path: str) -> list[Chunk]:
        """Split a list of page texts into :class:`Chunk` objects.

        *pages* is typically the output of
        :pymethod:`DocumentIngester.ingest`.
        """
        chunks: list[Chunk] = []
        global_index = 0
        global_char_offset = 0

        for page_num, page_text in enumerate(pages):
            if not page_text.strip():
                global_char_offset += len(page_text)
                continue

            segments = self._split_text(page_text, list(self.separators))
            page_chunks = self._merge_with_overlap(
                segments,
                source_path=source_path,
                page_number=page_num,
                base_char_offset=global_char_offset,
            )

            for chunk in page_chunks:
                chunk.chunk_index = global_index
                global_index += 1
                chunks.append(chunk)

            global_char_offset += len(page_text)

        logger.info(
            "Split %d page(s) of %s into %d chunks",
            len(pages),
            source_path,
            len(chunks),
        )
        return chunks

    # ------------------------------------------------------------------
    # Recursive splitting
    # ------------------------------------------------------------------

    def _split_text(self, text: str, separators: list[str]) -> list[str]:
        """Recursively split *text* until every segment fits in *chunk_size* tokens."""
        if not text:
            return []

        if self._count_tokens(text) <= self.chunk_size:
            return [text]

        if not separators:
            # Hard-split by token count as last resort
            return self._hard_split(text)

        sep = separators[0]
        remaining_seps = separators[1:]

        parts = text.split(sep)
        # Re-attach separator to each part (except last) so whitespace is preserved
        segments: list[str] = []
        for i, part in enumerate(parts):
            if not part and i < len(parts) - 1:
                continue
            piece = part + sep if i < len(parts) - 1 else part
            if self._count_tokens(piece) <= self.chunk_size:
                segments.append(piece)
            else:
                segments.extend(self._split_text(piece, remaining_seps))

        return segments

    def _hard_split(self, text: str) -> list[str]:
        """Split text into segments of at most *chunk_size* tokens."""
        tokenizer = self._get_tokenizer()
        token_ids = tokenizer.encode(text)
        segments: list[str] = []
        for start in range(0, len(token_ids), self.chunk_size):
            segment_ids = token_ids[start : start + self.chunk_size]
            segments.append(tokenizer.decode(segment_ids, skip_special_tokens=True))
        return segments

    # ------------------------------------------------------------------
    # Merge with overlap
    # ------------------------------------------------------------------

    def _merge_with_overlap(
        self,
        segments: list[str],
        source_path: str,
        page_number: int,
        base_char_offset: int,
    ) -> list[Chunk]:
        """Merge small segments up to *chunk_size* tokens and apply overlap."""
        if not segments:
            return []

        chunks: list[Chunk] = []
        current_text = ""
        current_start = base_char_offset
        char_cursor = base_char_offset

        for segment in segments:
            candidate = current_text + segment
            if current_text and self._count_tokens(candidate) > self.chunk_size:
                # Flush current chunk
                chunks.append(
                    Chunk(
                        text=current_text,
                        chunk_index=0,  # assigned later
                        source_path=source_path,
                        page_number=page_number,
                        start_char=current_start,
                        end_char=current_start + len(current_text),
                    )
                )
                # Overlap: keep tail of previous chunk
                overlap_text = self._get_overlap_text(current_text)
                current_start = current_start + len(current_text) - len(overlap_text)
                current_text = overlap_text + segment
            else:
                current_text = candidate

            char_cursor += len(segment)

        # Flush remaining text
        if current_text.strip():
            chunks.append(
                Chunk(
                    text=current_text,
                    chunk_index=0,
                    source_path=source_path,
                    page_number=page_number,
                    start_char=current_start,
                    end_char=current_start + len(current_text),
                )
            )

        return chunks

    def _get_overlap_text(self, text: str) -> str:
        """Return the trailing portion of *text* that fits in *chunk_overlap* tokens."""
        if self.chunk_overlap <= 0:
            return ""
        tokenizer = self._get_tokenizer()
        token_ids = tokenizer.encode(text)
        if len(token_ids) <= self.chunk_overlap:
            return text
        overlap_ids = token_ids[-self.chunk_overlap :]
        return tokenizer.decode(overlap_ids, skip_special_tokens=True)
