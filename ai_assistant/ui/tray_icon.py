"""System tray icon with context menu for mode switching and controls."""

from __future__ import annotations

import logging

from PySide6.QtCore import Qt
from PySide6.QtGui import QAction, QActionGroup, QColor, QFont, QIcon, QPainter, QPixmap
from PySide6.QtWidgets import QMenu, QSystemTrayIcon, QWidget

from ai_assistant.ui import styles
from ai_assistant.ui.signals import AppSignals

logger = logging.getLogger(__name__)


def _make_icon(dot_color: str, size: int = 16) -> QIcon:
    """Brass quotation mark with a small status dot in the corner."""
    pixmap = QPixmap(size, size)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    # Quotation-mark glyph
    painter.setPen(QColor(styles.ACCENT))
    font = QFont("Georgia")
    font.setPixelSize(size - 1)
    font.setBold(True)
    painter.setFont(font)
    painter.drawText(pixmap.rect(), Qt.AlignmentFlag.AlignCenter, "“")
    # Status dot, bottom-right
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(QColor(dot_color))
    painter.drawEllipse(size - 6, size - 6, 5, 5)
    painter.end()
    return QIcon(pixmap)


class TrayIcon(QSystemTrayIcon):
    """System-tray icon with mode selection, listening toggle, and controls."""

    def __init__(
        self,
        signals: AppSignals,
        parent: QWidget | None = None,
    ) -> None:
        super().__init__(parent)
        self._signals = signals
        self._capture_excluded = True
        self._dot_color = styles.SUCCESS

        # Icon dot reflects capture safety (sage = hidden, red = visible)
        self.setIcon(_make_icon(self._dot_color))
        self.setToolTip("Interview copilot — hidden from capture")

        self._build_menu()
        self._connect_signals()

        # Double-click toggles overlay
        self.activated.connect(self._on_activated)

    # ------------------------------------------------------------------
    # Menu
    # ------------------------------------------------------------------

    def _build_menu(self) -> None:
        menu = QMenu()

        # Show overlay
        self._show_action = QAction("Show Overlay", menu)
        self._show_action.setCheckable(True)
        self._show_action.setChecked(False)
        self._show_action.triggered.connect(self._signals.toggle_overlay.emit)
        menu.addAction(self._show_action)

        menu.addSeparator()

        # Mode submenu
        mode_menu = menu.addMenu("Mode")
        mode_group = QActionGroup(mode_menu)
        mode_group.setExclusive(True)

        self._mode_actions: dict[str, QAction] = {}
        for mode_name in ("passive", "suggestion", "active"):
            action = QAction(mode_name.capitalize(), mode_group)
            action.setCheckable(True)
            action.setData(mode_name)
            if mode_name == "passive":
                action.setChecked(True)
            mode_menu.addAction(action)
            self._mode_actions[mode_name] = action

        mode_group.triggered.connect(self._on_mode_selected)

        menu.addSeparator()

        # Listening toggle
        self._listen_action = QAction("Listening", menu)
        self._listen_action.setCheckable(True)
        self._listen_action.setChecked(True)
        self._listen_action.triggered.connect(self._signals.toggle_listening.emit)
        menu.addAction(self._listen_action)

        menu.addSeparator()

        # Capture exclusion toggle
        self._capture_action = QAction("Visible in Screen Share", menu)
        self._capture_action.setCheckable(True)
        self._capture_action.setChecked(False)  # excluded by default
        self._capture_action.triggered.connect(self._on_capture_toggle)
        menu.addAction(self._capture_action)

        menu.addSeparator()

        # Quit
        quit_action = QAction("Quit", menu)
        quit_action.triggered.connect(self._signals.quit_requested.emit)
        menu.addAction(quit_action)

        self.setContextMenu(menu)

    # ------------------------------------------------------------------
    # Signal connections
    # ------------------------------------------------------------------

    def _connect_signals(self) -> None:
        self._signals.mode_changed.connect(self._on_mode_changed)
        self._signals.overlay_visible_changed.connect(self._show_action.setChecked)
        self._signals.listening_changed.connect(self._listen_action.setChecked)
        self._signals.status_message.connect(self.setToolTip)
        self._signals.capture_status.connect(self._on_capture_status)

    def _on_capture_status(self, ok: bool) -> None:
        self._dot_color = styles.SUCCESS if ok else styles.ERROR
        self.setIcon(_make_icon(self._dot_color))

    # ------------------------------------------------------------------
    # Handlers
    # ------------------------------------------------------------------

    def _on_activated(self, reason: QSystemTrayIcon.ActivationReason) -> None:
        if reason == QSystemTrayIcon.ActivationReason.DoubleClick:
            self._signals.toggle_overlay.emit()

    def _on_mode_selected(self, action: QAction) -> None:
        mode = action.data()
        self._signals.mode_changed.emit(mode)

    def _on_mode_changed(self, mode: str) -> None:
        self.setToolTip(f"Interview copilot — {mode.capitalize()}")
        if mode in self._mode_actions:
            self._mode_actions[mode].setChecked(True)

    def _on_capture_toggle(self, checked: bool) -> None:
        """User toggled 'Visible in Screen Share'."""
        # When checked → visible in capture → exclusion OFF
        self._capture_excluded = not checked
        self._signals.status_message.emit(
            "Overlay visible in screen share" if checked else "Overlay hidden from screen share"
        )
