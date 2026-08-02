"""THE QUIET PROMPTER — frameless editorial overlay, invisible to screen capture."""

from __future__ import annotations

import ctypes
import logging
from typing import Optional

from PySide6.QtCore import (
    QEasingCurve,
    QParallelAnimationGroup,
    QPoint,
    QPropertyAnimation,
    QSize,
    Qt,
    Signal,
)
from PySide6.QtGui import (
    QColor,
    QCursor,
    QDragEnterEvent,
    QDragLeaveEvent,
    QDropEvent,
    QMouseEvent,
    QPainter,
    QPainterPath,
    QPaintEvent,
)
from PySide6.QtWidgets import (
    QComboBox,
    QGraphicsOpacityEffect,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPlainTextEdit,
    QPushButton,
    QSizeGrip,
    QVBoxLayout,
    QWidget,
    QApplication,
)

from ai_assistant.audio.mic_capture import list_mic_devices
from ai_assistant.ui import styles
from ai_assistant.ui.effects import heartbeat
from ai_assistant.ui.level_meter import LevelMeter
from ai_assistant.ui.notes_panel import NotesPanel
from ai_assistant.ui.pinned_panel import PinnedPanel
from ai_assistant.ui.response_panel import ResponsePanel
from ai_assistant.ui.shortcuts import ShortcutManager
from ai_assistant.ui.signals import AppSignals
from ai_assistant.ui.styles import MODE_COLORS, STATE_STYLES
from ai_assistant.ui.transcript_panel import TranscriptPanel

logger = logging.getLogger(__name__)

# Win32 constants
GWL_EXSTYLE = -20
WS_EX_TRANSPARENT = 0x00000020
WS_EX_LAYERED = 0x00080000
WDA_NONE = 0x00000000
WDA_EXCLUDEFROMCAPTURE = 0x00000011

_MODE_CYCLE = ["passive", "suggestion", "active"]
_MODE_LABEL = {"passive": "passive ›", "suggestion": "suggest ›", "active": "active ›"}


# ---------------------------------------------------------------------------
# Header components
# ---------------------------------------------------------------------------

class BreathingDot(QWidget):
    """8px status dot; alpha breathes with the shared heartbeat."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setFixedSize(8, 8)
        self._color = QColor(STATE_STYLES["loading"][0])
        self._ambient = True
        heartbeat().tick.connect(self._on_tick)

    def set_state(self, state: str) -> None:
        color, _ = STATE_STYLES.get(state, STATE_STYLES["listening"])
        self._color = QColor(color)
        self._ambient = state in ("loading", "listening", "composing")
        self.update()

    def _on_tick(self) -> None:
        if self._ambient:
            self.update()

    def paintEvent(self, event: QPaintEvent) -> None:  # noqa: N802
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        c = QColor(self._color)
        if self._ambient:
            c.setAlphaF(min(1.0, heartbeat().breath))
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(c)
        p.drawEllipse(0, 0, 8, 8)
        p.end()


class ShieldGlyph(QWidget):
    """14px shield — brass when capture-excluded, error red when it failed."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setFixedSize(14, 16)
        self._ok = True
        self.setToolTip("Hidden from screen capture")

    def set_ok(self, ok: bool) -> None:
        self._ok = ok
        self.setToolTip(
            "Hidden from screen capture" if ok
            else "Window may be visible to screen capture"
        )
        self.update()

    def paintEvent(self, event: QPaintEvent) -> None:  # noqa: N802
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        col = QColor(styles.ACCENT if self._ok else styles.ERROR)
        path = QPainterPath()
        w, h = self.width(), self.height()
        path.moveTo(w / 2, 1)
        path.lineTo(w - 1, 4)
        path.lineTo(w - 1, h * 0.55)
        path.quadTo(w - 1, h - 1, w / 2, h - 1)
        path.quadTo(1, h - 1, 1, h * 0.55)
        path.lineTo(1, 4)
        path.closeSubpath()
        pen = p.pen()
        pen.setColor(col)
        pen.setWidthF(1.3)
        p.setPen(pen)
        c = QColor(col); c.setAlpha(40)
        p.setBrush(c)
        p.drawPath(path)
        p.end()


class ToggleChip(QPushButton):
    """Ghost text toggle with an optional count badge."""

    def __init__(self, text: str, parent: QWidget | None = None) -> None:
        super().__init__(text, parent)
        self.setCheckable(True)
        self.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
        self.setStyleSheet(
            styles.GHOST_BUTTON + "QPushButton { padding: 3px 6px; font-size: 10px; }"
        )
        self._badge = QLabel("", self)
        self._badge.setStyleSheet(
            f"background: rgba(201,162,94,40); color: {styles.ACCENT}; "
            f"border-radius: 7px; padding: 0 5px; font-size: 9px;"
        )
        self._badge.hide()

    _BASE = styles.GHOST_BUTTON + "QPushButton { padding: 3px 6px; font-size: 10px; }"

    def set_count(self, n: int) -> None:
        if n > 0:
            self._badge.setText(str(n))
            self._badge.adjustSize()
            # reserve right padding so the badge never sits over the label
            self.setStyleSheet(
                styles.GHOST_BUTTON
                + "QPushButton { padding: 3px 20px 3px 6px; font-size: 10px; }"
            )
            self._badge.show()
            self._reposition()
        else:
            self.setStyleSheet(self._BASE)
            self._badge.hide()

    def _reposition(self) -> None:
        self._badge.move(self.width() - self._badge.width() - 3,
                         (self.height() - self._badge.height()) // 2)

    def resizeEvent(self, event) -> None:  # noqa: N802
        super().resizeEvent(event)
        self._reposition()


class ModeToggle(QPushButton):
    """Cycles passive → suggest → active; colored by mode."""

    mode_selected = Signal(str)

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._mode = "suggestion"
        self.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
        self.setFlat(True)
        self._apply()
        self.clicked.connect(self._cycle)

    def _apply(self) -> None:
        color = MODE_COLORS.get(self._mode, styles.TEXT_TERTIARY)
        self.setText(_MODE_LABEL[self._mode])
        self.setStyleSheet(
            f"QPushButton {{ background: transparent; border: none; color: {color}; "
            f"font-family: '{styles.ui_family()}'; font-size: 11px; font-weight: 600; "
            f"padding: 0 8px; text-align: left; }}"
            f"QPushButton:hover {{ color: {color}; }}"
        )

    def _cycle(self) -> None:
        i = (_MODE_CYCLE.index(self._mode) + 1) % len(_MODE_CYCLE)
        self._mode = _MODE_CYCLE[i]
        self._apply()
        self.mode_selected.emit(self._mode)

    def set_mode(self, mode: str) -> None:
        if mode in _MODE_CYCLE:
            self._mode = mode
            self._apply()


class InterviewerSwitch(QPushButton):
    """30×16 painted toggle for arming interviewer (system) audio."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setCheckable(True)
        self.setFixedSize(30, 16)
        self.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
        self.setStyleSheet("QPushButton { background: transparent; border: none; }")
        self.toggled.connect(lambda _: self.update())

    def paintEvent(self, event: QPaintEvent) -> None:  # noqa: N802
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        on = self.isChecked()
        track = QColor(styles.SUCCESS) if on else QColor("#3A322A")
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(track)
        p.drawRoundedRect(0, 0, 30, 16, 8, 8)
        p.setBrush(QColor("#F0E7D8"))
        x = 16 if on else 2
        p.drawEllipse(x, 2, 12, 12)
        p.end()


# ---------------------------------------------------------------------------
# Overlay window
# ---------------------------------------------------------------------------

class OverlayWindow(QWidget):
    """Always-on-top frameless overlay, excluded from screen capture."""

    DEFAULT_WIDTH = 460
    DEFAULT_HEIGHT = 560
    MAX_WIDTH = 620

    mic_changed = Signal(int)
    system_audio_changed = Signal(int)

    _DRAWER_HEIGHTS = {"transcript": 128, "notes": 88, "pins": 110}

    def __init__(self, signals: AppSignals, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._signals = signals
        self._drag_pos: Optional[QPoint] = None
        self._resize_edge: Optional[str] = None
        self._resize_start_geo = None
        self._resize_start_pos = None
        self._click_through = False
        self._capture_excluded = True
        self._edge_margin = 8
        self._drop_active = False
        self._open_drawer: Optional[str] = None
        self._settings_open = False
        self._glance = False
        self._pre_glance_size: Optional[QSize] = None
        self._drawer_anims: dict[str, QParallelAnimationGroup] = {}

        self._setup_window()
        self._setup_layout()
        self._connect_signals()
        self.setAcceptDrops(True)

    # ------------------------------------------------------------------
    # Window
    # ------------------------------------------------------------------

    def _setup_window(self) -> None:
        self.setWindowFlags(
            Qt.WindowType.FramelessWindowHint
            | Qt.WindowType.WindowStaysOnTopHint
            | Qt.WindowType.Tool
        )
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setMinimumSize(380, 420)
        self.resize(self.DEFAULT_WIDTH, self.DEFAULT_HEIGHT)
        self._position_default()
        self._size_grip = QSizeGrip(self)
        self._size_grip.setStyleSheet(
            "QSizeGrip { background: transparent; width: 12px; height: 12px; }"
        )

    def _position_default(self) -> None:
        screen = QApplication.primaryScreen()
        if screen is None:
            return
        geo = screen.availableGeometry()
        x = geo.right() - self.DEFAULT_WIDTH - 24
        y = geo.bottom() - self.DEFAULT_HEIGHT - 24
        self.move(max(geo.left(), x), max(geo.top(), y))

    # ------------------------------------------------------------------
    # Layout
    # ------------------------------------------------------------------

    def _setup_layout(self) -> None:
        root = QVBoxLayout(self)
        root.setContentsMargins(16, 12, 16, 12)
        root.setSpacing(8)

        root.addLayout(self._build_header())
        root.addLayout(self._build_meters())

        self._response_panel = ResponsePanel(self._signals)
        root.addWidget(self._response_panel, stretch=1)

        # Drawers (collapsed to 0 by default)
        self.transcript_panel = TranscriptPanel(self._signals)
        self._notes_panel = NotesPanel(self._signals)
        self._pinned_panel = PinnedPanel(self._signals)
        for name, w in (
            ("transcript", self.transcript_panel),
            ("notes", self._notes_panel),
            ("pins", self._pinned_panel),
        ):
            w.setMaximumHeight(0)
            root.addWidget(w)
        self._pinned_panel.count_changed.connect(self._pins_chip.set_count)

        root.addLayout(self._build_chat())
        root.addLayout(self._build_footer())

        self._settings_tray = self._build_settings_tray()
        self._settings_tray.setMaximumHeight(0)
        root.addWidget(self._settings_tray)

        self.setStyleSheet(styles.OVERLAY_STYLESHEET)

    def _build_header(self) -> QHBoxLayout:
        row = QHBoxLayout()
        row.setSpacing(5)
        self._dot = BreathingDot()
        row.addWidget(self._dot, alignment=Qt.AlignmentFlag.AlignVCenter)

        self._status = QLabel(STATE_STYLES["loading"][1])
        self._status.setFont(styles.make_font(styles.ui_small_family(), 11, weight=600))
        self._status.setStyleSheet(f"color: {styles.TEXT_SECONDARY};")
        row.addWidget(self._status)
        row.addStretch()

        self._power_btn = QPushButton("On")
        self._power_btn.setCheckable(True)
        self._power_btn.setChecked(True)
        self._power_btn.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
        self._power_btn.setToolTip("Turn listening on/off (Ctrl+Shift+L)")
        self._style_power(True)
        self._power_btn.clicked.connect(self._signals.toggle_listening.emit)
        row.addWidget(self._power_btn)

        self._shield = ShieldGlyph()
        row.addWidget(self._shield, alignment=Qt.AlignmentFlag.AlignVCenter)
        row.addSpacing(4)

        self._transcript_chip = ToggleChip("Transcript")
        self._transcript_chip.toggled.connect(lambda c: self._on_chip("transcript", c))
        self._notes_chip = ToggleChip("Notes")
        self._notes_chip.toggled.connect(lambda c: self._on_chip("notes", c))
        self._pins_chip = ToggleChip("Pins")
        self._pins_chip.toggled.connect(lambda c: self._on_chip("pins", c))
        for chip in (self._transcript_chip, self._notes_chip, self._pins_chip):
            row.addWidget(chip)
        return row

    def _build_meters(self) -> QVBoxLayout:
        col = QVBoxLayout()
        col.setContentsMargins(2, 0, 2, 0)
        col.setSpacing(2)
        self._mic_meter = LevelMeter(color=styles.SPEAKER_YOU)
        self._sys_meter = LevelMeter(color=styles.SPEAKER_INTERVIEWER)
        self._sys_meter.set_armed(False)
        col.addWidget(self._mic_meter)
        col.addWidget(self._sys_meter)
        return col

    def _build_chat(self) -> QHBoxLayout:
        row = QHBoxLayout()
        row.setSpacing(6)
        self._mode_toggle = ModeToggle()
        self._mode_toggle.mode_selected.connect(self._signals.mode_changed.emit)
        row.addWidget(self._mode_toggle)

        wrap = QWidget()
        wrap_l = QHBoxLayout(wrap)
        wrap_l.setContentsMargins(0, 0, 0, 0)
        self._chat_input = QLineEdit()
        self._chat_input.setPlaceholderText("Ask the assistant…  (Enter)")
        self._chat_input.setStyleSheet(
            f"QLineEdit {{ background: {styles.SURFACE}; color: {styles.TEXT_PRIMARY}; "
            f"border: 1px solid {styles.HAIRLINE}; border-radius: 10px; "
            f"padding: 7px 30px 7px 12px; font-family: '{styles.ui_family()}'; font-size: 13px; }}"
            f"QLineEdit:focus {{ border: 1px solid {styles.HAIRLINE_FOCUS}; }}"
        )
        self._chat_input.returnPressed.connect(self._on_chat_send)
        self._chat_input.textChanged.connect(self._on_chat_text)
        wrap_l.addWidget(self._chat_input)

        self._send_glyph = QLabel("↵", wrap)
        self._send_glyph.setStyleSheet(
            f"color: {styles.ACCENT}; font-size: 14px; background: transparent;"
        )
        self._send_effect = QGraphicsOpacityEffect(self._send_glyph)
        self._send_glyph.setGraphicsEffect(self._send_effect)
        self._send_effect.setOpacity(0.0)
        row.addWidget(wrap, stretch=1)
        return row

    def _build_footer(self) -> QHBoxLayout:
        row = QHBoxLayout()
        row.setContentsMargins(2, 0, 2, 0)
        self._footer_micro = QLabel("")
        self._footer_micro.setFont(styles.make_font(styles.mono_family(), 9))
        self._footer_micro.setStyleSheet(f"color: {styles.TEXT_TERTIARY};")
        self._update_footer_micro()
        row.addWidget(self._footer_micro)
        row.addStretch()

        self._gear = QPushButton("···")
        self._gear.setStyleSheet(styles.GHOST_BUTTON)
        self._gear.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
        self._gear.setToolTip("Settings")
        self._gear.clicked.connect(self._toggle_settings)
        row.addWidget(self._gear)
        row.addStretch()
        # right side left open for the size grip
        row.addSpacing(12)
        return row

    def _build_settings_tray(self) -> QWidget:
        tray = QWidget()
        tray.setObjectName("settingsTray")
        # Scope to the tray itself — an unscoped stylesheet cascades the border
        # onto every child widget (boxes around all the labels).
        tray.setStyleSheet(
            f"QWidget#settingsTray {{ background: {styles.SURFACE}; "
            f"border: 1px solid {styles.HAIRLINE}; border-radius: 10px; }}"
        )
        lay = QVBoxLayout(tray)
        lay.setContentsMargins(12, 10, 12, 10)
        lay.setSpacing(8)

        # Mic row
        mic_row = QHBoxLayout()
        mic_label = QLabel("Your mic")
        mic_label.setStyleSheet(f"color: {styles.TEXT_SECONDARY}; font-size: 11px;")
        mic_row.addWidget(mic_label)
        self._mic_combo = QComboBox()
        self._mic_combo.setStyleSheet(styles.COMBO_STYLE)
        self._populate_mic_dropdown()
        self._mic_combo.currentIndexChanged.connect(self._on_mic_changed)
        mic_row.addWidget(self._mic_combo, stretch=1)
        lay.addLayout(mic_row)

        # Interviewer audio row
        sys_row = QHBoxLayout()
        sys_label = QLabel("Interviewer audio (system)")
        sys_label.setStyleSheet(f"color: {styles.TEXT_SECONDARY}; font-size: 11px;")
        sys_row.addWidget(sys_label)
        sys_row.addStretch()
        self._sys_switch = InterviewerSwitch()
        self._sys_switch.toggled.connect(self._on_sys_toggled)
        sys_row.addWidget(self._sys_switch)
        lay.addLayout(sys_row)

        # Which source is "you" (the candidate) vs the interviewer
        role_row = QHBoxLayout()
        role_label = QLabel("You are")
        role_label.setStyleSheet(f"color: {styles.TEXT_SECONDARY}; font-size: 11px;")
        role_row.addWidget(role_label)
        self._you_source_combo = QComboBox()
        self._you_source_combo.setStyleSheet(styles.COMBO_STYLE)
        # index 0 -> "mic", index 1 -> "system"
        self._you_source_combo.addItem("My mic (interviewer on system)")
        self._you_source_combo.addItem("System audio (interviewer on mic)")
        self._you_source_combo.setCurrentIndex(0)
        self._you_source_combo.currentIndexChanged.connect(self._on_you_source_changed)
        role_row.addWidget(self._you_source_combo, stretch=1)
        lay.addLayout(role_row)

        # System prompt
        sp_label = QLabel("System prompt")
        sp_label.setStyleSheet(f"color: {styles.LABEL}; font-size: 10px; font-weight: 600;")
        lay.addWidget(sp_label)
        self._system_prompt_edit = QPlainTextEdit()
        self._system_prompt_edit.setPlaceholderText(
            "e.g. Senior Python role at Google — favor system-design depth"
        )
        self._system_prompt_edit.setFixedHeight(48)
        self._system_prompt_edit.setStyleSheet(
            f"QPlainTextEdit {{ background: {styles.SURFACE_RAISED}; color: {styles.TEXT_PRIMARY}; "
            f"border: 1px solid {styles.HAIRLINE}; border-radius: 8px; padding: 6px; "
            f"font-family: '{styles.ui_family()}'; font-size: 11px; }}"
        )
        self._system_prompt_edit.textChanged.connect(self._on_system_prompt_changed)
        lay.addWidget(self._system_prompt_edit)

        # Documents
        doc_row = QHBoxLayout()
        self._doc_btn = QPushButton("Add documents")
        self._doc_btn.setStyleSheet(styles.GHOST_BUTTON)
        self._doc_btn.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
        self._doc_btn.clicked.connect(self._open_file_picker)
        doc_row.addWidget(self._doc_btn)
        self._doc_status = QLabel("Drop PDF · DOCX · TXT · MD onto the window")
        self._doc_status.setStyleSheet(f"color: {styles.TEXT_TERTIARY}; font-size: 9px;")
        doc_row.addWidget(self._doc_status, stretch=1)
        lay.addLayout(doc_row)
        return tray

    def _populate_mic_dropdown(self) -> None:
        try:
            devices = list_mic_devices()
        except Exception:
            devices = []
        self._mic_devices: list[int | None] = [None]
        self._mic_combo.blockSignals(True)
        self._mic_combo.clear()
        self._mic_combo.addItem("System default")
        for dev in devices:
            name = dev["name"]
            if len(name) > 40:
                name = name[:37] + "…"
            self._mic_combo.addItem(name)
            self._mic_devices.append(dev["index"])
        self._mic_combo.blockSignals(False)

    # ------------------------------------------------------------------
    # Header / drawer interactions
    # ------------------------------------------------------------------

    def _on_chip(self, name: str, checked: bool) -> None:
        if checked:
            # single-open: close any other drawer + uncheck its chip
            if self._open_drawer and self._open_drawer != name:
                other = self._open_drawer
                self._animate_drawer(other, False)
                self._chip_for(other).setChecked(False)
            self._open_drawer = name
            self._animate_drawer(name, True)
        else:
            if self._open_drawer == name:
                self._open_drawer = None
            self._animate_drawer(name, False)

    def _chip_for(self, name: str) -> ToggleChip:
        return {
            "transcript": self._transcript_chip,
            "notes": self._notes_chip,
            "pins": self._pins_chip,
        }[name]

    def _drawer_for(self, name: str) -> QWidget:
        return {
            "transcript": self.transcript_panel,
            "notes": self._notes_panel,
            "pins": self._pinned_panel,
        }[name]

    def _animate_drawer(self, name: str, opening: bool) -> None:
        widget = self._drawer_for(name)
        target = self._DRAWER_HEIGHTS[name] if opening else 0
        group = QParallelAnimationGroup(self)
        h_anim = QPropertyAnimation(widget, b"maximumHeight")
        h_anim.setDuration(240 if opening else 200)
        h_anim.setStartValue(widget.maximumHeight())
        h_anim.setEndValue(target)
        h_anim.setEasingCurve(QEasingCurve.Type.InOutCubic)
        group.addAnimation(h_anim)
        group.start()
        self._drawer_anims[name] = group  # keep a ref so it isn't GC'd

    def _toggle_settings(self) -> None:
        self._settings_open = not self._settings_open
        # Size to actual content — a hardcoded height squeezed rows into overlap.
        target = self._settings_tray.sizeHint().height() if self._settings_open else 0
        # Grow/shrink the window by the same delta so the tray never squeezes
        # (and overlaps) the rest of the layout.
        delta = target - self._settings_tray.maximumHeight()
        self.resize(self.width(), max(self.minimumHeight(), self.height() + delta))
        anim = QPropertyAnimation(self._settings_tray, b"maximumHeight", self)
        anim.setDuration(220)
        anim.setStartValue(self._settings_tray.maximumHeight())
        anim.setEndValue(target)
        anim.setEasingCurve(QEasingCurve.Type.InOutCubic)
        anim.start()
        self._settings_anim = anim

    # ------------------------------------------------------------------
    # Chat / system prompt / documents
    # ------------------------------------------------------------------

    def _on_chat_text(self, text: str) -> None:
        self._send_effect.setOpacity(1.0 if text.strip() else 0.0)

    def _on_chat_send(self) -> None:
        text = self._chat_input.text().strip()
        if text:
            self._signals.chat_message_sent.emit(text)
            self._chat_input.clear()

    def _on_system_prompt_changed(self) -> None:
        self._signals.system_prompt_changed.emit(self._system_prompt_edit.toPlainText())

    def _on_mic_changed(self, combo_index: int) -> None:
        if 0 <= combo_index < len(self._mic_devices):
            device_index = self._mic_devices[combo_index]
            logger.info("Mic selection changed to device: %s", device_index)
            self.mic_changed.emit(device_index if device_index is not None else -1)

    def _on_you_source_changed(self, combo_index: int) -> None:
        source = "system" if combo_index == 1 else "mic"
        logger.info("You-source changed to: %s", source)
        self._signals.you_source_changed.emit(source)

    def _on_sys_toggled(self, checked: bool) -> None:
        self._sys_meter.set_armed(checked)
        self.system_audio_changed.emit(1 if checked else -1)

    SUPPORTED_EXTENSIONS = {".pdf", ".docx", ".txt", ".md"}

    def dragEnterEvent(self, event: QDragEnterEvent) -> None:  # noqa: N802
        if event.mimeData().hasUrls():
            self._drop_active = True
            self.update()
            event.acceptProposedAction()

    def dragLeaveEvent(self, event: QDragLeaveEvent) -> None:  # noqa: N802
        self._drop_active = False
        self.update()

    def dropEvent(self, event: QDropEvent) -> None:  # noqa: N802
        self._drop_active = False
        self.update()
        paths = []
        for url in event.mimeData().urls():
            path = url.toLocalFile()
            if path and any(path.lower().endswith(ext) for ext in self.SUPPORTED_EXTENSIONS):
                paths.append(path)
        if paths:
            self._doc_status.setText(f"Ingesting {len(paths)} file(s)…")
            self._signals.files_dropped.emit(paths)
        else:
            self._doc_status.setText("Unsupported type — use PDF, DOCX, TXT, MD")

    def _open_file_picker(self) -> None:
        from PySide6.QtWidgets import QFileDialog
        files, _ = QFileDialog.getOpenFileNames(
            self, "Select Documents", "",
            "Documents (*.pdf *.docx *.txt *.md);;All Files (*)",
        )
        if files:
            self._doc_status.setText(f"Ingesting {len(files)} file(s)…")
            self._signals.files_dropped.emit(files)

    def update_doc_status(self, text: str) -> None:
        self._doc_status.setText(text)

    # ------------------------------------------------------------------
    # Signals
    # ------------------------------------------------------------------

    def _connect_signals(self) -> None:
        self._signals.toggle_overlay.connect(self.toggle_visibility)
        self._signals.mode_changed.connect(self._on_mode_changed)
        self._signals.mic_level.connect(self._mic_meter.set_level)
        self._signals.system_level.connect(self._sys_meter.set_level)
        self._signals.assistant_state.connect(self._on_assistant_state)
        self._signals.capture_status.connect(self._on_capture_status)
        self._signals.listening_changed.connect(self._on_listening_changed)

    def _style_power(self, on: bool) -> None:
        color = styles.SUCCESS if on else styles.TEXT_TERTIARY
        # Plain text — the U+23FB power glyph renders as a tofu box in Segoe UI.
        self._power_btn.setText("On" if on else "Off")
        self._power_btn.setStyleSheet(
            f"QPushButton {{ background: transparent; border: 1px solid {color}; "
            f"border-radius: 8px; color: {color}; font-size: 10px; font-weight: 600; "
            f"padding: 2px 8px; }}"
            f"QPushButton:hover {{ background: rgba(242,233,220,14); }}"
        )

    def _on_listening_changed(self, on: bool) -> None:
        self._power_btn.setChecked(on)
        self._style_power(on)

    def _on_mode_changed(self, mode: str) -> None:
        self._mode_toggle.set_mode(mode)

    def _on_assistant_state(self, state: str) -> None:
        self._dot.set_state(state)
        _, word = STATE_STYLES.get(state, STATE_STYLES["listening"])
        self._status.setText(word)
        # Quicken the room while composing
        heartbeat().set_period(1600 if state == "composing" else 2600)

    def _on_capture_status(self, ok: bool) -> None:
        self._capture_excluded_ok = ok
        self._shield.set_ok(ok)
        self._update_footer_micro()

    def _update_footer_micro(self) -> None:
        ok = getattr(self, "_capture_excluded_ok", self._capture_excluded)
        cap = "● hidden from capture" if ok else "● VISIBLE to capture"
        ct = "click-through on" if self._click_through else "click-through off"
        self._footer_micro.setText(f"{cap} · {ct}")

    # ------------------------------------------------------------------
    # Capture exclusion
    # ------------------------------------------------------------------

    def _apply_capture_exclusion(self) -> None:
        try:
            hwnd = int(self.winId())
            affinity = WDA_EXCLUDEFROMCAPTURE if self._capture_excluded else WDA_NONE
            ok = bool(ctypes.windll.user32.SetWindowDisplayAffinity(hwnd, affinity))
        except Exception:
            ok = False
        if ok:
            logger.info("Screen-capture exclusion %s",
                        "enabled" if self._capture_excluded else "disabled")
        else:
            logger.warning("SetWindowDisplayAffinity failed (Windows 10 2004+ required)")
        # Drive shield/footer/tray from the real result
        self._signals.capture_status.emit(ok if self._capture_excluded else True)

    def set_capture_excluded(self, excluded: bool) -> None:
        self._capture_excluded = excluded
        if self.isVisible():
            self._apply_capture_exclusion()

    # ------------------------------------------------------------------
    # Click-through
    # ------------------------------------------------------------------

    def set_click_through(self, enabled: bool) -> None:
        try:
            hwnd = int(self.winId())
            user32 = ctypes.windll.user32
            ex_style = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
            if enabled:
                user32.SetWindowLongW(hwnd, GWL_EXSTYLE,
                                      ex_style | WS_EX_TRANSPARENT | WS_EX_LAYERED)
            else:
                user32.SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style & ~WS_EX_TRANSPARENT)
        except Exception:
            pass
        self._click_through = enabled
        self.setWindowOpacity(0.85 if enabled else 1.0)
        self._update_footer_micro()
        # GWL_EXSTYLE changes can force recreation — re-assert exclusion
        if self._capture_excluded:
            self._apply_capture_exclusion()

    # ------------------------------------------------------------------
    # Visibility + glance mode
    # ------------------------------------------------------------------

    def toggle_visibility(self) -> None:
        if self.isVisible():
            self._fade_out()
        else:
            self.show()
            self._apply_capture_exclusion()
            self._fade_in()

    def _fade_in(self) -> None:
        self.setWindowOpacity(0.0)
        anim = QPropertyAnimation(self, b"windowOpacity", self)
        anim.setDuration(180)
        anim.setStartValue(0.0)
        anim.setEndValue(0.85 if self._click_through else 1.0)
        anim.setEasingCurve(QEasingCurve.Type.InOutQuad)
        anim.start()
        self._show_anim = anim

    def _fade_out(self) -> None:
        anim = QPropertyAnimation(self, b"windowOpacity", self)
        anim.setDuration(150)
        anim.setStartValue(self.windowOpacity())
        anim.setEndValue(0.0)
        anim.setEasingCurve(QEasingCurve.Type.InOutQuad)
        anim.finished.connect(self.hide)
        anim.start()
        self._hide_anim = anim

    def _toggle_glance(self) -> None:
        self._glance = not self._glance
        if self._glance:
            self._pre_glance_size = self.size()
            for w in (self.transcript_panel, self._notes_panel, self._pinned_panel):
                w.setMaximumHeight(0)
            self._settings_tray.setMaximumHeight(0)
            self._chat_input.parentWidget().setVisible(False)
            self._mode_toggle.setVisible(False)
            self._footer_micro.setVisible(False)
            self.resize(self.width(), 320)
        else:
            self._chat_input.parentWidget().setVisible(True)
            self._mode_toggle.setVisible(True)
            self._footer_micro.setVisible(True)
            if self._pre_glance_size:
                self.resize(self._pre_glance_size)

    # ------------------------------------------------------------------
    # Painting — backdrop + drop invite
    # ------------------------------------------------------------------

    def paintEvent(self, event: QPaintEvent) -> None:  # noqa: N802
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        bg = QColor(styles.BG)
        bg.setAlpha(styles.BG_ALPHA)
        p.setBrush(bg)
        p.setPen(Qt.PenStyle.NoPen)
        rect = self.rect().adjusted(0, 0, -1, -1)
        p.drawRoundedRect(rect, 14, 14)
        # Hairline border
        pen = p.pen()
        pen.setColor(QColor(242, 233, 220, 23))
        pen.setWidthF(1.0)
        p.setPen(pen)
        p.setBrush(Qt.BrushStyle.NoBrush)
        p.drawRoundedRect(rect, 14, 14)

        if self._drop_active:
            pen = p.pen()
            c = QColor(styles.ACCENT); c.setAlpha(120)
            pen.setColor(c)
            pen.setWidthF(1.5)
            p.setPen(pen)
            p.drawRoundedRect(rect.adjusted(6, 6, -6, -6), 12, 12)
            p.setPen(QColor(styles.ACCENT))
            p.setFont(styles.make_font(styles.ui_family(), 12, weight=600))
            p.drawText(rect, Qt.AlignmentFlag.AlignCenter, "Drop PDF / DOCX / TXT / MD")
        p.end()

    # ------------------------------------------------------------------
    # Frameless drag / resize
    # ------------------------------------------------------------------

    def _get_edge(self, pos: QPoint) -> Optional[str]:
        m = self._edge_margin
        r = self.rect()
        edges = []
        if pos.y() < m:
            edges.append("top")
        if pos.y() > r.height() - m:
            edges.append("bottom")
        if pos.x() < m:
            edges.append("left")
        if pos.x() > r.width() - m:
            edges.append("right")
        return "-".join(edges) if edges else None

    def mouseDoubleClickEvent(self, event: QMouseEvent) -> None:  # noqa: N802
        # Double-click the header strip toggles glance mode
        if event.position().y() < 34:
            self._toggle_glance()
            event.accept()

    def mousePressEvent(self, event: QMouseEvent) -> None:  # noqa: N802
        if event.button() == Qt.MouseButton.LeftButton:
            edge = self._get_edge(event.position().toPoint())
            if edge:
                self._resize_edge = edge
                self._resize_start_geo = self.geometry()
                self._resize_start_pos = event.globalPosition().toPoint()
            else:
                self._drag_pos = event.globalPosition().toPoint() - self.frameGeometry().topLeft()
            event.accept()

    def mouseMoveEvent(self, event: QMouseEvent) -> None:  # noqa: N802
        if not (event.buttons() & Qt.MouseButton.LeftButton):
            edge = self._get_edge(event.position().toPoint())
            if edge in ("left", "right"):
                self.setCursor(QCursor(Qt.CursorShape.SizeHorCursor))
            elif edge in ("top", "bottom"):
                self.setCursor(QCursor(Qt.CursorShape.SizeVerCursor))
            elif edge:
                self.setCursor(QCursor(Qt.CursorShape.SizeFDiagCursor))
            else:
                self.setCursor(QCursor(Qt.CursorShape.ArrowCursor))
            return

        if self._resize_edge and self._resize_start_geo and self._resize_start_pos:
            delta = event.globalPosition().toPoint() - self._resize_start_pos
            geo = self._resize_start_geo
            new_geo = geo.__class__(geo)
            if "right" in self._resize_edge:
                new_geo.setWidth(max(self.minimumWidth(),
                                     min(self.MAX_WIDTH, geo.width() + delta.x())))
            if "bottom" in self._resize_edge:
                new_geo.setHeight(max(self.minimumHeight(), geo.height() + delta.y()))
            if "left" in self._resize_edge:
                new_geo.setLeft(geo.left() + delta.x())
                if new_geo.width() < self.minimumWidth():
                    new_geo.setLeft(geo.right() - self.minimumWidth())
            if "top" in self._resize_edge:
                new_geo.setTop(geo.top() + delta.y())
                if new_geo.height() < self.minimumHeight():
                    new_geo.setTop(geo.bottom() - self.minimumHeight())
            self.setGeometry(new_geo)
            event.accept()
        elif self._drag_pos is not None:
            self.move(event.globalPosition().toPoint() - self._drag_pos)
            event.accept()

    def mouseReleaseEvent(self, event: QMouseEvent) -> None:  # noqa: N802
        self._drag_pos = None
        self._resize_edge = None
        self._resize_start_geo = None
        self._resize_start_pos = None

    # ------------------------------------------------------------------
    # Native events (global hotkeys)
    # ------------------------------------------------------------------

    def set_shortcut_manager(self, manager: ShortcutManager) -> None:
        self._shortcut_manager = manager

    def nativeEvent(self, event_type: bytes, message: int) -> object:  # noqa: N802
        if hasattr(self, "_shortcut_manager"):
            result = self._shortcut_manager.handle_native_event(event_type, message)
            if result:
                return (True, 0)
        return super().nativeEvent(event_type, message)
