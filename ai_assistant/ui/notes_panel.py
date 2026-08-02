"""AI notes drawer — key points extracted from the interview."""

from __future__ import annotations

import time

from PySide6.QtCore import Qt
from PySide6.QtGui import QTextCursor
from PySide6.QtWidgets import (
    QApplication,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from ai_assistant.ui import styles
from ai_assistant.ui.signals import AppSignals


class NotesPanel(QWidget):
    """Displays AI-generated notes captured during the conversation."""

    def __init__(self, signals: AppSignals, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._signals = signals
        self._notes: list[tuple[str, str]] = []  # (timestamp, text)
        self._setup_ui()
        self._signals.note_added.connect(self.add_note)

    def _setup_ui(self) -> None:
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(4)

        header = QHBoxLayout()
        header.setContentsMargins(2, 0, 2, 0)
        header.setSpacing(4)

        take = QPushButton("Take notes")
        take.setStyleSheet(styles.GHOST_BUTTON)
        take.clicked.connect(self._signals.trigger_notes.emit)
        header.addWidget(take)
        header.addStretch()

        copy_btn = QPushButton("Copy all")
        copy_btn.setStyleSheet(styles.GHOST_BUTTON)
        copy_btn.clicked.connect(self._copy_all)
        header.addWidget(copy_btn)

        clear_btn = QPushButton("Clear")
        clear_btn.setStyleSheet(styles.GHOST_BUTTON)
        clear_btn.clicked.connect(self._clear)
        header.addWidget(clear_btn)
        layout.addLayout(header)

        self._text = QTextEdit()
        self._text.setReadOnly(True)
        self._text.setVerticalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAsNeeded)
        self._text.setStyleSheet(
            f"QTextEdit {{ background: transparent; border: none; "
            f"font-family: '{styles.ui_family()}'; font-size: 11px; "
            f"color: {styles.TEXT_PRIMARY}; padding: 2px; }}"
        )
        self._text.setPlaceholderText("AI notes will appear here during the interview…")
        layout.addWidget(self._text)

    def add_note(self, note: str) -> None:
        ts = time.strftime("%H:%M")
        self._notes.append((ts, note))
        self._text.setHtml(
            "".join(
                f'<div style="margin:2px 0;">'
                f'<span style="color:{styles.TEXT_TERTIARY};">{t}</span>&nbsp;'
                f'<span style="color:{styles.ACCENT};">–</span>&nbsp;'
                f'<span style="color:{styles.TEXT_PRIMARY};">{_esc(n)}</span></div>'
                for t, n in self._notes
            )
        )
        cursor = self._text.textCursor()
        cursor.movePosition(QTextCursor.MoveOperation.End)
        self._text.setTextCursor(cursor)

    def _copy_all(self) -> None:
        if self._notes:
            clipboard = QApplication.clipboard()
            if clipboard:
                clipboard.setText("\n".join(f"[{t}] {n}" for t, n in self._notes))

    def _clear(self) -> None:
        self._notes.clear()
        self._text.clear()


def _esc(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
