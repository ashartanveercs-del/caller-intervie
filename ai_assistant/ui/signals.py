"""Centralized Qt signal hub for cross-widget communication."""

from PySide6.QtCore import QObject, Signal


class AppSignals(QObject):
    """Single instance shared across all UI components.

    Any component can emit or connect without importing other widgets.
    """

    # Overlay control
    toggle_overlay = Signal()
    overlay_visible_changed = Signal(bool)

    # Streaming response
    response_chunk = Signal(str)
    response_complete = Signal(str)
    response_clear = Signal()

    # Mode changes
    mode_changed = Signal(str)  # "passive", "suggestion", "active"

    # Listening toggle
    toggle_listening = Signal()
    listening_changed = Signal(bool)

    # Pin management
    pin_response = Signal(str)
    unpin_response = Signal(int)

    # Quick action triggers
    trigger_summarize = Signal()   # Ctrl+Shift+S — summarize conversation
    trigger_detail = Signal()      # Ctrl+Shift+D — give detailed answer
    trigger_suggest = Signal()     # Ctrl+Shift+G — suggest what to say

    # Chat input
    chat_message_sent = Signal(str)       # user typed a message
    system_prompt_changed = Signal(str)   # system prompt updated

    # AI Notes
    note_added = Signal(str)      # new auto-generated note
    trigger_notes = Signal()      # Ctrl+Shift+N — generate notes from conversation

    # Document ingestion
    files_dropped = Signal(list)  # list of file paths

    # Audio levels
    mic_level = Signal(float)      # 0.0–1.0 mic intensity
    system_level = Signal(float)   # 0.0–1.0 system audio intensity

    # Status
    status_message = Signal(str)

    # High-level assistant state driving the header dot + status word.
    # One of: "loading", "listening", "composing", "paused", "error".
    assistant_state = Signal(str)
    # Screen-capture exclusion actually active (True) or failed (False).
    capture_status = Signal(bool)
    # RAG knowledge base finished loading in the background.
    rag_ready = Signal(bool)
