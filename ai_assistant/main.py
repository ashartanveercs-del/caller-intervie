"""Application entrypoint - wires the runtime to the legacy PySide6 UI."""

from __future__ import annotations

import asyncio
import logging
import sys
from pathlib import Path
from uuid import uuid4

from dotenv import load_dotenv

from ai_assistant.config import Config
from ai_assistant.core.events import EventType
from ai_assistant.runtime import (
    RuntimeConfigurationError,
    RuntimeStateError,
    SessionConfig,
    build_runtime,
)
from ai_assistant.ui.app import AssistantApp

logger = logging.getLogger(__name__)


def _application_shutdown_event(application) -> asyncio.Event:
    """Translate Qt's quit signal into an asyncio event from any thread."""
    loop = asyncio.get_running_loop()
    shutdown_event = asyncio.Event()

    def _request_shutdown() -> None:
        loop.call_soon_threadsafe(shutdown_event.set)

    application.aboutToQuit.connect(_request_shutdown)
    return shutdown_event


async def _run_until_application_quit(
    application,
    runtime,
    shutdown_event: asyncio.Event | None = None,
) -> None:
    """Wait for normal Qt termination and drain the runtime before returning."""
    event = shutdown_event or _application_shutdown_event(application)
    await event.wait()
    await runtime.stop_session()


def _setup_logging() -> None:
    import os

    # pythonw.exe (double-click / start.bat launch) has no console, so
    # sys.stdout and sys.stderr are None. Any write to them - including a
    # default logging StreamHandler or a third-party library - raises and can
    # kill the process. Redirect them to devnull so nothing can crash on write.
    # UTF-8 with errors ignored: a cp1252 devnull still RAISES on characters
    # like arrows in log lines, and that exception killed the app under pythonw.
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
    # Add a console handler only when a REAL console/pipe is attached -
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


async def async_main() -> None:
    # Logging + dotenv are initialised once in main() before the loop starts.
    config = Config.from_env()

    if not config.deepgram_api_key:
        logger.error("DEEPGRAM_API_KEY not set — exiting")
        sys.exit(1)
    if not config.llm_api_key:
        logger.error("DEEPSEEK_API_KEY not set — exiting")
        sys.exit(1)

    runtime = build_runtime(config)

    # Qt remains an adapter: it creates widgets and translates runtime events.
    ui_app = AssistantApp()
    qt_app = ui_app.setup()
    shutdown_event = _application_shutdown_event(qt_app)

    async def _on_transcript(event) -> None:
        if ui_app.overlay is not None:
            ui_app.overlay.transcript_panel.add_transcript(
                event.text, event.speaker, event.is_final, event.source
            )

    async def _on_response_chunk(event) -> None:
        ui_app.signals.response_chunk.emit(event.text)

    async def _on_response_complete(event) -> None:
        ui_app.signals.response_complete.emit(event.full_text)

    async def _on_mode_change(event) -> None:
        if event.old_mode == event.new_mode:
            return
        ui_app.signals.mode_changed.emit(event.new_mode)

    runtime.event_bus.on(EventType.TRANSCRIPT_UPDATE, _on_transcript)
    runtime.event_bus.on(EventType.RESPONSE_CHUNK, _on_response_chunk)
    runtime.event_bus.on(EventType.RESPONSE_COMPLETE, _on_response_complete)
    runtime.event_bus.on(EventType.MODE_CHANGE, _on_mode_change)

    async def _set_mode(mode_str: str) -> None:
        try:
            await runtime.set_mode(mode_str)
        except ValueError:
            logger.warning("Unknown mode: %s", mode_str)

    def _on_ui_mode_change(mode_str: str) -> None:
        asyncio.create_task(_set_mode(mode_str))

    def _on_ui_toggle_listening() -> None:
        new_state = runtime.snapshot().state != "listening"
        runtime.set_listening(new_state)
        ui_app.signals.listening_changed.emit(new_state)
        ui_app.signals.assistant_state.emit("listening" if new_state else "paused")

    def _on_mic_changed(device_index: int) -> None:
        device = None if device_index == -1 else device_index
        asyncio.create_task(runtime.set_audio_device("mic", device))

    def _on_sys_audio_changed(signal_value: int) -> None:
        asyncio.create_task(runtime.set_system_audio(signal_value > 0))

    ui_app.signals.mode_changed.connect(_on_ui_mode_change)
    ui_app.signals.toggle_listening.connect(_on_ui_toggle_listening)

    def _on_summarize() -> None:
        ui_app.signals.response_clear.emit()
        asyncio.create_task(runtime.trigger_query(
            "Summarize the ENTIRE conversation from the very beginning to now. "
            "Cover every topic and question discussed, in order — do not skip "
            "the earlier parts. Use as many concise bullet points as needed.",
            "summary",
        ))

    def _on_detail() -> None:
        ui_app.signals.response_clear.emit()
        asyncio.create_task(runtime.trigger_query(
            "Write a detailed word-for-word script answering the most recent question. "
            "Make it thorough with specific examples and technical depth. "
            "Just the script, ready to read out loud.",
            "detail",
        ))

    def _on_suggest() -> None:
        ui_app.signals.response_clear.emit()
        asyncio.create_task(runtime.trigger_query(
            "Write a short word-for-word script for what I should say next. "
            "One natural-sounding response, 2-3 sentences. Just the words to speak.",
            "suggestion",
        ))

    def _on_take_notes() -> None:
        asyncio.create_task(_generate_notes())

    async def _generate_notes() -> None:
        if not runtime.full_transcript():
            ui_app.signals.note_added.emit("No conversation to take notes from yet.")
            return
        try:
            for note in await runtime.generate_notes():
                ui_app.signals.note_added.emit(note)
        except Exception:
            logger.exception("Note generation failed")

    ui_app.signals.trigger_summarize.connect(_on_summarize)
    ui_app.signals.trigger_detail.connect(_on_detail)
    ui_app.signals.trigger_suggest.connect(_on_suggest)
    ui_app.signals.trigger_notes.connect(_on_take_notes)

    def _on_chat_message(text: str) -> None:
        ui_app.signals.response_clear.emit()
        asyncio.create_task(runtime.trigger_query(text, "chat"))

    def _on_system_prompt_changed(text: str) -> None:
        runtime.set_system_prompt(text)
        logger.info("System prompt updated (%d chars)", len(text))

    ui_app.signals.chat_message_sent.connect(_on_chat_message)
    ui_app.signals.system_prompt_changed.connect(_on_system_prompt_changed)

    def _on_you_source_changed(source: str) -> None:
        runtime.set_you_source(source)
        if ui_app.overlay is not None:
            ui_app.overlay.transcript_panel.set_you_source(source)
        logger.info("You-source set to %s", source)

    ui_app.signals.you_source_changed.connect(_on_you_source_changed)

    def _on_files_dropped(paths: list[str]) -> None:
        asyncio.create_task(_ingest_files(paths))

    async def _ingest_files(paths: list[str]) -> None:
        try:
            result = await runtime.ingest_documents(paths)
        except RuntimeStateError:
            if ui_app.overlay is not None:
                ui_app.overlay.update_doc_status(
                    "Knowledge base still warming up — try again shortly"
                )
            return
        if ui_app.overlay is not None:
            ui_app.overlay.update_doc_status(
                f"Loaded {result.files} file(s), {result.chunks_added} chunks · "
                f"{result.total_chunks} total"
            )

    ui_app.signals.files_dropped.connect(_on_files_dropped)

    if ui_app.overlay is not None:
        ui_app.overlay.mic_changed.connect(_on_mic_changed)
        ui_app.overlay.system_audio_changed.connect(_on_sys_audio_changed)

    def _on_audio_level(source: str, level: float) -> None:
        signal = (
            ui_app.signals.mic_level
            if source == "mic"
            else ui_app.signals.system_level
        )
        signal.emit(level)

    runtime.set_audio_level_handler(_on_audio_level)

    try:
        await runtime.start_session(
            SessionConfig(
                session_id=str(uuid4()),
                mode="interview",
                input_language=config.deepgram_language,
                response_language=config.response_language,
                review_language=config.review_language,
                you_source="mic",
                brief_id="",
            )
        )
    except RuntimeConfigurationError as error:
        logger.error("Runtime configuration missing: %s", ", ".join(error.missing))
        ui_app.shutdown()
        sys.exit(1)

    logger.info("AI Assistant running. Press Ctrl+C to exit.")
    logger.info("  Alt+Space       → toggle overlay")
    logger.info("  Ctrl+Shift+L    → toggle listening")

    ui_app.signals.assistant_state.emit("listening")
    if ui_app.overlay is not None:
        ui_app.overlay.update_doc_status("Loading knowledge base…")
        ui_app.overlay._sys_switch.setChecked(True)

    from PySide6.QtCore import QTimer

    def _check_rag() -> None:
        snapshot = runtime.snapshot()
        if snapshot.knowledge_state == "ready":
            size = snapshot.knowledge_chunks
            if ui_app.overlay is not None:
                ui_app.overlay.update_doc_status(
                    f"Knowledge base ready · {size} chunks"
                    if size else "Drop PDF · DOCX · TXT · MD onto the window"
                )
            ui_app.signals.rag_ready.emit(True)
            _rag_timer.stop()
        elif snapshot.knowledge_state == "error":
            if ui_app.overlay is not None:
                ui_app.overlay.update_doc_status("Running without documents")
            _rag_timer.stop()

    _rag_timer = QTimer()
    _rag_timer.setInterval(500)
    _rag_timer.timeout.connect(_check_rag)
    _rag_timer.start()

    try:
        await _run_until_application_quit(qt_app, runtime, shutdown_event)
    finally:
        _rag_timer.stop()
        ui_app.shutdown()


def main() -> None:
    """Entry point - sets up qasync event loop and runs."""
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
