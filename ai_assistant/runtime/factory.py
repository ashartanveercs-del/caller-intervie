"""Production dependency construction for the assistant runtime."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
import logging
from pathlib import Path

from ai_assistant.audio.deepgram_client import DeepgramTranscriber
from ai_assistant.audio.mic_capture import DualMicCapture
from ai_assistant.config import Config
from ai_assistant.core.events import EventBus
from ai_assistant.core.orchestrator import Orchestrator
from ai_assistant.llm.anthropic_client import AnthropicLLM
from ai_assistant.llm.prompt_builder import PromptBuilder
from ai_assistant.rag.embeddings import LocalEmbedder
from ai_assistant.rag.retriever import RAGRetriever
from ai_assistant.rag.vector_store import FAISSVectorStore

from .service import RuntimeService

logger = logging.getLogger(__name__)


@dataclass
class RuntimeDependencies:
    config: Config
    event_bus: EventBus
    capture: DualMicCapture
    transcriber: DeepgramTranscriber
    llm: AnthropicLLM
    prompt_builder: PromptBuilder
    orchestrator: Orchestrator
    rag_factory: Callable[[], RAGRetriever] | None


def _build_rag(config: Config) -> RAGRetriever:
    embedder = LocalEmbedder()
    vector_store = FAISSVectorStore(dimension=embedder.dimension)
    retriever = RAGRetriever(
        embedder,
        vector_store,
        relevance_threshold=config.rag_relevance_threshold,
        default_k=config.rag_top_k,
    )

    index_dir = config.rag_db_path
    index_path = Path(index_dir)
    if index_path.exists() and (index_path / "index.faiss").exists():
        try:
            retriever.load_index(index_dir)
            logger.info("Loaded RAG index from %s", index_dir)
        except Exception:
            logger.warning("Failed to load RAG index — starting fresh")

    docs_dir = Path("documents")
    if docs_dir.exists():
        results = retriever.ingest_directory(str(docs_dir))
        if results:
            logger.info("Auto-ingested %d documents from ./documents/", len(results))
            retriever.save_index(index_dir)
    return retriever


def build_runtime(config: Config) -> RuntimeService:
    """Construct runtime dependencies without starting audio or provider I/O."""
    event_bus = EventBus()
    capture = DualMicCapture(
        sample_rate=config.sample_rate,
        blocksize=config.audio_blocksize,
        dtype=config.audio_dtype,
        mic_device=None,
        system_device=None,
    )
    transcriber = DeepgramTranscriber(config, event_bus)
    prompt_builder = PromptBuilder(config)
    llm = AnthropicLLM(config, event_bus)
    orchestrator = Orchestrator(
        config=config,
        event_bus=event_bus,
        llm=llm,
        prompt_builder=prompt_builder,
        retriever=None,
    )
    dependencies = RuntimeDependencies(
        config=config,
        event_bus=event_bus,
        capture=capture,
        transcriber=transcriber,
        llm=llm,
        prompt_builder=prompt_builder,
        orchestrator=orchestrator,
        rag_factory=lambda: _build_rag(config),
    )
    return RuntimeService(dependencies)
