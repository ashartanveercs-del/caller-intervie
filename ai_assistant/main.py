"""Application entrypoint — wires all modules and starts the event loop."""

from __future__ import annotations

import asyncio
import logging
import sys
from pathlib import Path

from dotenv import load_dotenv

from ai_assistant.config import Config
from ai_assistant.core.events import EventBus, EventType
from ai_assistant.core.orchestrator import Mode, Orchestrator
from ai_assistant.audio.mic_capture import DualMicCapture
from ai_assistant.audio.deepgram_client import DeepgramTranscriber
from ai_assistant.llm.anthropic_client import AnthropicLLM
from ai_assistant.llm.prompt_builder import PromptBuilder
from ai_assistant.rag.embeddings import LocalEmbedder
from ai_assistant.rag.vector_store import FAISSVectorStore
from ai_assistant.rag.retriever import RAGRetriever
from ai_assistant.ui.app import AssistantApp

logger = logging.getLogger(__name__)


def _setup_logging() -> None:
    import os

    # pythonw.exe (double-click / start.bat launch) has no console, so
    # sys.stdout and sys.stderr are None. Any write to them — including a
    # default logging StreamHandler or a third-party library — raises and can
    # kill the process. Redirect them to devnull so nothing can crash on write.
    # UTF-8 with errors ignored: a cp1252 devnull still RAISES on characters
    # like "→" in log lines, and that exception killed the app under pythonw.
    if sys.stdout is None:
        sys.stdout = open(os.devnull, "w", encoding="utf-8", errors="ignore")
    if sys.stderr is None:
        sys.stderr = open(os.devnull, "w", encoding="utf-8", errors="ignore")

    # Always log to a file next to the app so there's a record even with no
    # console (and so DEBUG spam never goes to a possibly-None stream).
    log_path = Path(__file__).resolve().parent.parent / "interview.log"
    handlers: list[logging.Handler] = [
        logging.FileHandler(log_path, encoding="utf-8")
    ]
    # Add a console handler only when a REAL console/pipe is attached —
    # never route logging through the devnull stub above.
    if sys.__stderr__ is not None:
        try:
            handlers.append(logging.StreamHandler(sys.stderr))
        except Exception:
            pass

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%H:%M:%S",
        handlers=handlers,
    )
    # Quiet down noisy libraries (qasync/websockets dump raw audio bytes at DEBUG)
    logging.getLogger("urllib3").setLevel(logging.WARNING)
    logging.getLogger("httpcore").setLevel(logging.WARNING)
    logging.getLogger("httpx").setLevel(logging.WARNING)
    logging.getLogger("sentence_transformers").setLevel(logging.INFO)
    logging.getLogger("datasets").setLevel(logging.WARNING)
    logging.getLogger("filelock").setLevel(logging.WARNING)
    logging.getLogger("huggingface_hub").setLevel(logging.WARNING)
    logging.getLogger("qasync").setLevel(logging.WARNING)
    logging.getLogger("websockets").setLevel(logging.WARNING)
    logging.getLogger("asyncio").setLevel(logging.WARNING)

    # Post-mortem instrumentation: under pythonw a fatal error is invisible,
    # so route native crashes (faulthandler), uncaught exceptions, and normal
    # interpreter exit into fatal.log to make any death diagnosable.
    import atexit
    import faulthandler
    try:
        fatal = open(log_path.parent / "fatal.log", "a", buffering=1, encoding="utf-8")
        fatal.write(f"--- start pid={os.getpid()} ---\n")
        faulthandler.enable(fatal)
    except Exception:
        fatal = None

    def _excepthook(exc_type, exc, tb) -> None:
        logging.getLogger("fatal").critical(
            "Uncaught exception", exc_info=(exc_type, exc, tb)
        )

    sys.excepthook = _excepthook
    atexit.register(
        lambda: fatal and fatal.write(f"--- clean exit pid={os.getpid()} ---\n")
    )


def _compute_level(pcm_bytes: bytes) -> float:
    """Compute RMS level from 16-bit PCM audio, normalized to 0.0–1.0."""
    import struct
    n_samples = len(pcm_bytes) // 2
    if n_samples == 0:
        return 0.0
    samples = struct.unpack(f"<{n_samples}h", pcm_bytes)
    rms = (sum(s * s for s in samples) / n_samples) ** 0.5
    # Normalize: 32768 is max for int16, but speech rarely hits that
    level = min(1.0, rms / 8000.0)
    return level


async def audio_pump(
    mic: DualMicCapture,
    transcriber: DeepgramTranscriber,
    signals: object,
) -> None:
    """Read tagged audio chunks, route to Deepgram, and emit level signals."""
    frame_count = 0
    async for source, chunk in mic:
        if source == "mic":
            await transcriber.send_mic_audio(chunk)
        elif source == "system":
            await transcriber.send_system_audio(chunk)

        # Update level meters every 4th frame (~16ms * 4 = ~64ms) to avoid UI spam
        frame_count += 1
        if frame_count % 4 == 0:
            level = _compute_level(chunk)
            if source == "mic":
                signals.mic_level.emit(level)
            else:
                signals.system_level.emit(level)


async def async_main() -> None:
    # Logging + dotenv are initialised once in main() before the loop starts.
    config = Config.from_env()

    if not config.deepgram_api_key:
        logger.error("DEEPGRAM_API_KEY not set — exiting")
        sys.exit(1)
    if not config.llm_api_key:
        logger.error("DEEPSEEK_API_KEY not set — exiting")
        sys.exit(1)

    # ---- Event bus ----
    bus = EventBus()

    # ---- Audio (dual capture: your mic + system audio for interviewer) ----
    mic = DualMicCapture(
        sample_rate=config.sample_rate,
        blocksize=config.audio_blocksize,
        dtype=config.audio_dtype,
        mic_device=None,    # default mic, changed via UI dropdown
        system_device=None, # set via UI dropdown (e.g. Stereomix)
    )
    transcriber = DeepgramTranscriber(config, bus)

    # ---- LLM ----
    prompt_builder = PromptBuilder(config)
    llm = AnthropicLLM(config, bus)

    # ---- RAG ----
    # The embedding model takes ~30s to load, so build it on a background
    # thread and show the UI immediately; RAG features light up when ready.
    index_dir = config.rag_db_path
    retriever_holder: dict[str, RAGRetriever] = {}

    def _build_rag() -> RAGRetriever:
        embedder = LocalEmbedder()
        vector_store = FAISSVectorStore(dimension=embedder.dimension)
        retriever = RAGRetriever(
            embedder,
            vector_store,
            relevance_threshold=config.rag_relevance_threshold,
            default_k=config.rag_top_k,
        )
        if Path(index_dir).exists() and (Path(index_dir) / "index.faiss").exists():
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

    # ---- Orchestrator (RAG attaches later) ----
    orchestrator = Orchestrator(
        config=config,
        event_bus=bus,
        llm=llm,
        prompt_builder=prompt_builder,
        retriever=None,
    )

    # ---- UI ----
    ui_app = AssistantApp()
    ui_app.setup()  # creates overlay, tray, shortcuts (QApplication already exists)

    # Bridge EventBus → Qt signals
    async def _on_transcript(event):
        if ui_app.overlay is not None:
            ui_app.overlay.transcript_panel.add_transcript(
                event.text, event.speaker, event.is_final, event.source
            )

    async def _on_response_chunk(event):
        ui_app.signals.response_chunk.emit(event.text)

    async def _on_response_complete(event):
        ui_app.signals.response_complete.emit(event.full_text)
        prompt_builder.add_to_history("assistant", event.full_text)

    async def _on_mode_change(event):
        ui_app.signals.mode_changed.emit(event.new_mode)

    bus.on(EventType.TRANSCRIPT_UPDATE, _on_transcript)
    bus.on(EventType.RESPONSE_CHUNK, _on_response_chunk)
    bus.on(EventType.RESPONSE_COMPLETE, _on_response_complete)
    bus.on(EventType.MODE_CHANGE, _on_mode_change)

    # Bridge Qt signals → orchestrator
    def _on_ui_mode_change(mode_str: str) -> None:
        try:
            mode = Mode(mode_str)
            asyncio.create_task(orchestrator.set_mode(mode))
        except ValueError:
            logger.warning("Unknown mode: %s", mode_str)

    def _on_ui_toggle_listening() -> None:
        new_state = not orchestrator.listening
        orchestrator.set_listening(new_state)
        ui_app.signals.listening_changed.emit(new_state)
        ui_app.signals.assistant_state.emit("listening" if new_state else "paused")

    # Mic device change
    def _on_mic_changed(device_index: int) -> None:
        dev = None if device_index == -1 else device_index
        asyncio.create_task(mic.change_mic_device(dev))

    # System audio toggle (WASAPI loopback for interviewer)
    def _on_sys_audio_changed(signal_val: int) -> None:
        if signal_val > 0:
            mic.start_system_capture()
            asyncio.create_task(transcriber.start_system_stream())
        else:
            mic.stop_system_capture()
            asyncio.create_task(transcriber.stop_system_stream())

    ui_app.signals.mode_changed.connect(_on_ui_mode_change)
    ui_app.signals.toggle_listening.connect(_on_ui_toggle_listening)
    # Quick action triggers
    def _on_summarize() -> None:
        ui_app.signals.response_clear.emit()
        asyncio.create_task(orchestrator.trigger_query(
            "Summarize the ENTIRE conversation from the very beginning to now. "
            "Cover every topic and question discussed, in order — do not skip "
            "the earlier parts. Use as many concise bullet points as needed.",
            full_transcript=True,
        ))

    def _on_detail() -> None:
        ui_app.signals.response_clear.emit()
        asyncio.create_task(orchestrator.trigger_query(
            "Write a detailed word-for-word script answering the most recent question. "
            "Make it thorough with specific examples and technical depth. "
            "Just the script, ready to read out loud."
        ))

    def _on_suggest() -> None:
        ui_app.signals.response_clear.emit()
        asyncio.create_task(orchestrator.trigger_query(
            "Write a short word-for-word script for what I should say next. "
            "One natural-sounding response, 2-3 sentences. Just the words to speak."
        ))

    def _on_take_notes() -> None:
        asyncio.create_task(_generate_notes())

    async def _generate_notes() -> None:
        """Ask AI to extract key notes from recent conversation."""
        buf = orchestrator._transcript_buffer
        full = buf.get_full_text()
        if not full:
            ui_app.signals.note_added.emit("No conversation to take notes from yet.")
            return

        note_prompt = await prompt_builder.build(
            mode="active",
            transcript_buffer=buf,
            use_full_transcript=True,
            query=(
                "Extract the KEY POINTS from the ENTIRE conversation, from the very "
                "beginning to now. Go through it in order and do not skip the earlier "
                "parts. Focus on: questions asked, answers given, decisions made, "
                "action items, and important topics. "
                "Format: one bullet per point, in chronological order, as many "
                "bullets as it takes to cover everything. Be concise per bullet. "
                "Use plain text, no markdown."
            ),
        )
        try:
            response = await llm.stream_generate(note_prompt)
            # Split into individual notes
            for line in response.strip().split("\n"):
                line = line.strip().lstrip("•-* ")
                if line:
                    ui_app.signals.note_added.emit(line)
        except Exception:
            logger.exception("Note generation failed")

    ui_app.signals.trigger_summarize.connect(_on_summarize)
    ui_app.signals.trigger_detail.connect(_on_detail)
    ui_app.signals.trigger_suggest.connect(_on_suggest)
    ui_app.signals.trigger_notes.connect(_on_take_notes)

    # Chat input — user types a message to AI
    def _on_chat_message(text: str) -> None:
        ui_app.signals.response_clear.emit()
        prompt_builder.add_to_history("user", text)
        asyncio.create_task(orchestrator.trigger_query(text))

    # System prompt — user customizes the AI behavior
    def _on_system_prompt_changed(text: str) -> None:
        prompt_builder.custom_system_prompt = text
        logger.info("System prompt updated (%d chars)", len(text))

    ui_app.signals.chat_message_sent.connect(_on_chat_message)
    ui_app.signals.system_prompt_changed.connect(_on_system_prompt_changed)

    # Role assignment — which audio source is "you" vs the interviewer
    def _on_you_source_changed(source: str) -> None:
        orchestrator.set_you_source(source)
        if ui_app.overlay is not None:
            ui_app.overlay.transcript_panel.set_you_source(source)
        logger.info("You-source set to %s", source)

    ui_app.signals.you_source_changed.connect(_on_you_source_changed)

    # File drop — ingest documents into RAG
    def _on_files_dropped(paths: list) -> None:
        retriever = retriever_holder.get("r")
        if retriever is None:
            if ui_app.overlay is not None:
                ui_app.overlay.update_doc_status("Knowledge base still warming up — try again shortly")
            return
        total_chunks = 0
        for path in paths:
            try:
                count = retriever.ingest_file(path)
                total_chunks += count
                logger.info("Ingested %s — %d chunks", path, count)
            except Exception:
                logger.exception("Failed to ingest %s", path)
        try:
            retriever.save_index(index_dir)
        except Exception:
            pass
        if ui_app.overlay is not None:
            ui_app.overlay.update_doc_status(
                f"Loaded {len(paths)} file(s), {total_chunks} chunks · {retriever.vector_store.size} total"
            )

    ui_app.signals.files_dropped.connect(_on_files_dropped)

    if ui_app.overlay is not None:
        ui_app.overlay.mic_changed.connect(_on_mic_changed)
        ui_app.overlay.system_audio_changed.connect(_on_sys_audio_changed)

    # ---- Start audio ----
    await transcriber.start()
    await mic.start()

    logger.info("AI Assistant running. Press Ctrl+C to exit.")
    logger.info("  Alt+Space       → toggle overlay")
    logger.info("  Ctrl+Shift+L    → toggle listening")

    # Audio is live now, so the assistant is already listening. The knowledge
    # base (RAG) loads separately in the background and reports via doc status.
    ui_app.signals.assistant_state.emit("listening")
    if ui_app.overlay is not None:
        ui_app.overlay.update_doc_status("Loading knowledge base…")
        # Capture the interviewer's audio too (WASAPI loopback on the default
        # output — headphones or speakers, whichever is active). Toggling the
        # switch cascades through the normal signal wiring.
        ui_app.overlay._sys_switch.setChecked(True)

    import threading

    def _load_rag_worker() -> None:
        try:
            retriever_holder["r"] = _build_rag()
        except Exception:
            logger.exception("Background RAG load failed — running without documents")
            retriever_holder["error"] = True

    threading.Thread(target=_load_rag_worker, daemon=True).start()

    # Poll from the GUI thread so retriever attach + label updates are thread-safe.
    from PySide6.QtCore import QTimer

    def _check_rag() -> None:
        if "r" in retriever_holder:
            retriever = retriever_holder["r"]
            orchestrator.set_retriever(retriever)
            size = retriever.vector_store.size
            if ui_app.overlay is not None:
                ui_app.overlay.update_doc_status(
                    f"Knowledge base ready · {size} chunks"
                    if size else "Drop PDF · DOCX · TXT · MD onto the window"
                )
            ui_app.signals.rag_ready.emit(True)
            logger.info("RAG ready (%d chunks)", size)
            _rag_timer.stop()
        elif retriever_holder.get("error"):
            if ui_app.overlay is not None:
                ui_app.overlay.update_doc_status("Running without documents")
            _rag_timer.stop()

    _rag_timer = QTimer()
    _rag_timer.setInterval(500)
    _rag_timer.timeout.connect(_check_rag)
    _rag_timer.start()

    # Start audio pump as background task
    pump_task = asyncio.create_task(audio_pump(mic, transcriber, ui_app.signals))

    # Run forever (qasync event loop is already active from main())
    try:
        await asyncio.Event().wait()
    finally:
        pump_task.cancel()
        _rag_timer.stop()
        await mic.stop()
        await transcriber.stop()
        ui_app.shutdown()

        # Save RAG index if it finished loading
        retriever = retriever_holder.get("r")
        if retriever is not None:
            try:
                retriever.save_index(index_dir)
                logger.info("Saved RAG index to %s", index_dir)
            except Exception:
                logger.warning("Failed to save RAG index")


def main() -> None:
    """Entry point — sets up qasync event loop and runs."""
    _setup_logging()
    load_dotenv()

    from PySide6.QtWidgets import QApplication

    qt_app = QApplication.instance() or QApplication(sys.argv)
    qt_app.setQuitOnLastWindowClosed(False)

    try:
        import qasync
        loop = qasync.QEventLoop(qt_app)
        asyncio.set_event_loop(loop)

        with loop:
            loop.run_until_complete(async_main())
    except ImportError:
        logger.error("qasync is required: pip install qasync")
        sys.exit(1)


if __name__ == "__main__":
    main()
