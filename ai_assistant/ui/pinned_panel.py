"""Pinned answers — small raised cards the overlay shows in its Pins drawer."""

from __future__ import annotations

from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor, QCursor, QPainter, QPaintEvent
from PySide6.QtWidgets import (
    QApplication,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from ai_assistant.ui import styles
from ai_assistant.ui.signals import AppSignals


class _UnpinButton(QPushButton):
    """22px ghost button drawing a small × glyph."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setFixedSize(22, 22)
        self.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
        self.setStyleSheet(
            "QPushButton { background: transparent; border: none; border-radius: 6px; }"
            "QPushButton:hover { background: rgba(242,233,220,14); }"
        )

    def paintEvent(self, event: QPaintEvent) -> None:  # noqa: N802
        super().paintEvent(event)
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        pen = p.pen()
        pen.setColor(QColor(styles.TEXT_TERTIARY))
        pen.setWidthF(1.4)
        p.setPen(pen)
        r = self.rect()
        cx, cy = r.center().x(), r.center().y()
        p.drawLine(cx - 3, cy - 3, cx + 3, cy + 3)
        p.drawLine(cx - 3, cy + 3, cx + 3, cy - 3)
        p.end()


class PinnedItem(QWidget):
    """A single pinned answer card: first line preview + copy + unpin."""

    MAX_PREVIEW_CHARS = 140

    def __init__(self, text: str, index: int, signals: AppSignals,
                 parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._text = text
        self._index = index
        self._signals = signals
        self._setup_ui()

    def _setup_ui(self) -> None:
        self.setStyleSheet(
            f"PinnedItem {{ background: {styles.SURFACE_RAISED}; border-radius: 8px; }}"
        )
        layout = QHBoxLayout(self)
        layout.setContentsMargins(10, 8, 6, 8)
        layout.setSpacing(6)

        first_line = self._text.strip().splitlines()[0] if self._text.strip() else ""
        preview = first_line[: self.MAX_PREVIEW_CHARS]
        if len(first_line) > self.MAX_PREVIEW_CHARS:
            preview += "…"

        label = QLabel(preview)
        label.setWordWrap(True)
        label.setStyleSheet(
            f"font-family:'{styles.ui_family()}'; font-size:11px; color:{styles.TEXT_SECONDARY};"
        )
        layout.addWidget(label, stretch=1)

        copy_btn = QPushButton("Copy")
        copy_btn.setStyleSheet(styles.GHOST_BUTTON)
        copy_btn.clicked.connect(self._copy)
        layout.addWidget(copy_btn)

        unpin = _UnpinButton()
        unpin.setToolTip("Unpin")
        unpin.clicked.connect(self._remove)
        layout.addWidget(unpin)

    def _copy(self) -> None:
        clipboard = QApplication.clipboard()
        if clipboard is not None:
            clipboard.setText(self._text)

    def _remove(self) -> None:
        self._signals.unpin_response.emit(self._index)


class PinnedPanel(QWidget):
    """Scrollable list of pinned answers. Visibility handled by the overlay drawer."""

    count_changed = Signal(int)

    def __init__(self, signals: AppSignals, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._signals = signals
        self._entries: list[tuple[str, PinnedItem | None]] = []
        self._setup_ui()
        self._signals.pin_response.connect(self.add_pinned)
        self._signals.unpin_response.connect(self.remove_pinned)

    def _setup_ui(self) -> None:
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        self._scroll = QScrollArea()
        self._scroll.setWidgetResizable(True)
        self._scroll.setFrameShape(QScrollArea.Shape.NoFrame)
        self._scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self._scroll.setStyleSheet("background: transparent; border: none;")

        self._container = QWidget()
        self._container_layout = QVBoxLayout(self._container)
        self._container_layout.setContentsMargins(0, 0, 0, 0)
        self._container_layout.setSpacing(6)
        self._container_layout.addStretch()
        self._scroll.setWidget(self._container)
        layout.addWidget(self._scroll)

    @property
    def pin_count(self) -> int:
        return sum(1 for _text, widget in self._entries if widget is not None)

    def add_pinned(self, text: str) -> None:
        idx = len(self._entries)
        item = PinnedItem(text, idx, self._signals)
        self._entries.append((text, item))
        self._container_layout.insertWidget(self._container_layout.count() - 1, item)
        self.count_changed.emit(self.pin_count)

    def remove_pinned(self, index: int) -> None:
        if not (0 <= index < len(self._entries)):
            return
        text, widget = self._entries[index]
        if widget is None:
            return
        self._entries[index] = (text, None)
        self._container_layout.removeWidget(widget)
        widget.deleteLater()
        self.count_changed.emit(self.pin_count)
