"""Live transcript — pencil-to-ink settling, You vs Interviewer, hover-pause scroll."""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtGui import QTextCursor
from PySide6.QtWidgets import QTextEdit, QVBoxLayout, QWidget

from ai_assistant.ui import styles
from ai_assistant.ui.signals import AppSignals

# Styling is keyed on ROLE, not raw source, so the labels follow whichever
# audio source the user has designated as "you".
ROLE_STYLES = {
    "you": {"label": "You", "color": styles.SPEAKER_YOU},
    "interviewer": {"label": "Interviewer", "color": styles.SPEAKER_INTERVIEWER},
}

_MAX_FINALS = 60  # keep the rendered history bounded


class TranscriptPanel(QWidget):
    """Displays the live transcript, labeled by speaker, interim → final."""

    def __init__(self, signals: AppSignals, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._signals = signals
        self._finals: list[tuple[str, str]] = []
        self._interim: tuple[str, str] | None = None
        self._hovering = False
        self._you_source = "mic"
        self._setup_ui()

    def set_you_source(self, source: str) -> None:
        """Set which audio source ('mic'/'system') is labeled 'You'; re-render."""
        if source not in ("mic", "system"):
            return
        self._you_source = source
        self._render()

    def _setup_ui(self) -> None:
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)

        self._text = QTextEdit()
        self._text.setReadOnly(True)
        self._text.setVerticalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAsNeeded)
        self._text.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self._text.setStyleSheet(
            f"QTextEdit {{ background: transparent; border: none; "
            f"font-family: '{styles.ui_family()}'; font-size: 12px; padding: 2px 4px; }}"
        )
        self._text.setPlaceholderText("Transcript will appear here…")
        # Pause auto-scroll while the pointer is over the panel.
        self._text.installEventFilter(self)
        layout.addWidget(self._text)

    # ------------------------------------------------------------------

    def eventFilter(self, obj, event):  # noqa: N802
        from PySide6.QtCore import QEvent
        if obj is self._text:
            if event.type() == QEvent.Type.Enter:
                self._hovering = True
            elif event.type() == QEvent.Type.Leave:
                self._hovering = False
        return super().eventFilter(obj, event)

    def add_transcript(self, text: str, speaker: int | None, is_final: bool,
                       source: str = "mic") -> None:
        if not text.strip():
            return
        if is_final:
            self._finals.append((source, text.strip()))
            if len(self._finals) > _MAX_FINALS:
                self._finals = self._finals[-_MAX_FINALS:]
            if self._interim and self._interim[0] == source:
                self._interim = None
        else:
            self._interim = (source, text.strip())
        self._render()

    def _render(self) -> None:
        parts: list[str] = []
        last_source: str | None = None

        def speaker_header(src: str) -> str:
            role = "you" if src == self._you_source else "interviewer"
            style = ROLE_STYLES[role]
            return (
                f'<div style="margin-top:6px;"><span style="color:{style["color"]};'
                f'font-weight:600;font-size:11px;">{style["label"]}</span></div>'
            )

        for src, txt in self._finals:
            if src != last_source:
                parts.append(speaker_header(src))
                last_source = src
            parts.append(
                f'<span style="color:{styles.TEXT_PRIMARY};">{_esc(txt)} </span>'
            )

        if self._interim:
            src, txt = self._interim
            if src != last_source:
                parts.append(speaker_header(src))
            parts.append(
                f'<span style="color:{styles.TEXT_TERTIARY};">{_esc(txt)} </span>'
            )

        self._text.setHtml("".join(parts))
        if not self._hovering:
            cursor = self._text.textCursor()
            cursor.movePosition(QTextCursor.MoveOperation.End)
            self._text.setTextCursor(cursor)
            self._text.ensureCursorVisible()

    def clear(self) -> None:
        self._finals.clear()
        self._interim = None
        self._text.clear()


def _esc(text: str) -> str:
    return (
        text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    )
