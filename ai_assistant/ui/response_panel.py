"""The answer card — the hero surface. A calm editorial teleprompter.

The question headline is pinned above the scroll region so it never scrolls
away; the answer body streams in and a persistent scroll animation keeps the
freshest sentence near a fixed optical line so the reader's eye never hunts.
A monospace T+ counter reports how fresh the visible answer is.
"""

from __future__ import annotations

import re

from PySide6.QtCore import (
    QEasingCurve,
    QEvent,
    QPropertyAnimation,
    QTimer,
    Qt,
)
from PySide6.QtGui import (
    QColor,
    QLinearGradient,
    QPainter,
    QPaintEvent,
    QTextCursor,
)
from PySide6.QtWidgets import (
    QApplication,
    QFrame,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QTextBrowser,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from ai_assistant.ui import styles
from ai_assistant.ui.signals import AppSignals

_PROMPTER_FRAC = 0.38  # optical focal line, fraction of viewport height


# ---------------------------------------------------------------------------
# Glyph buttons (copy / pin) — painted, 22px
# ---------------------------------------------------------------------------

class _GlyphButton(QPushButton):
    def __init__(self, glyph: str, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._glyph = glyph
        self._confirm = False
        self.setFixedSize(24, 24)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        self.setStyleSheet(
            "QPushButton { background: transparent; border: none; border-radius: 6px; }"
            "QPushButton:hover { background: rgba(242,233,220,14); }"
        )

    def flash_confirm(self) -> None:
        self._confirm = True
        self.update()
        QTimer.singleShot(1200, self._revert)

    def _revert(self) -> None:
        self._confirm = False
        self.update()

    def paintEvent(self, event: QPaintEvent) -> None:  # noqa: N802
        super().paintEvent(event)
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        r = self.rect()
        cx, cy = r.center().x(), r.center().y()
        col = QColor(styles.SUCCESS) if self._confirm else QColor(styles.TEXT_SECONDARY)
        pen = p.pen()
        pen.setColor(col)
        pen.setWidthF(1.4)
        p.setPen(pen)

        if self._confirm:
            p.drawLine(cx - 4, cy, cx - 1, cy + 3)
            p.drawLine(cx - 1, cy + 3, cx + 5, cy - 4)
        elif self._glyph == "copy":
            p.drawRoundedRect(cx - 5, cy - 5, 8, 8, 2, 2)
            p.drawRoundedRect(cx - 2, cy - 2, 8, 8, 2, 2)
        elif self._glyph == "pin":
            p.drawEllipse(cx - 3, cy - 5, 6, 6)
            p.drawLine(cx, cy + 1, cx, cy + 5)
        p.end()


# ---------------------------------------------------------------------------
# Prompter overlay — the 38% rule + top scrim + streaming caret
# ---------------------------------------------------------------------------

class _PrompterOverlay(QWidget):
    def __init__(self, view: QTextEdit) -> None:
        super().__init__(view.viewport())
        self._view = view
        self.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents, True)
        self.setAttribute(Qt.WidgetAttribute.WA_NoSystemBackground, True)
        self._streaming = False

    def set_streaming(self, on: bool) -> None:
        self._streaming = on
        self.update()

    def paintEvent(self, event: QPaintEvent) -> None:  # noqa: N802
        try:
            p = QPainter(self)
            p.setRenderHint(QPainter.RenderHint.Antialiasing)
            w, h = self.width(), self.height()

            # Top scrim — history dissolves upward
            scrim = QLinearGradient(0, 0, 0, 24)
            top = QColor(styles.SURFACE)
            top.setAlpha(220)
            faded = QColor(styles.SURFACE)
            faded.setAlpha(0)
            scrim.setColorAt(0.0, top)
            scrim.setColorAt(1.0, faded)
            p.fillRect(0, 0, w, 24, scrim)

            if self._streaming:
                # Prompter rule at 38%, brass, fading at both ends
                y = int(h * _PROMPTER_FRAC)
                grad = QLinearGradient(0, 0, w, 0)
                edge = QColor(styles.ACCENT); edge.setAlpha(0)
                mid = QColor(styles.ACCENT); mid.setAlpha(110)
                grad.setColorAt(0.0, edge)
                grad.setColorAt(0.5, mid)
                grad.setColorAt(1.0, edge)
                p.fillRect(0, y, w, 1, grad)

                # Streaming caret at the end cursor
                cursor = self._view.textCursor()
                cursor.movePosition(QTextCursor.MoveOperation.End)
                cr = self._view.cursorRect(cursor)
                caret = QColor(styles.ACCENT)
                caret.setAlpha(220)
                p.fillRect(cr.right() + 1, cr.top() + 1, 2, max(14, cr.height() - 2), caret)
            p.end()
        except Exception:
            pass


# ---------------------------------------------------------------------------
# Answer card
# ---------------------------------------------------------------------------

class ResponsePanel(QFrame):
    """Streaming answer surface with pinned headline, teleprompter scroll."""

    def __init__(self, signals: AppSignals, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._signals = signals
        self._current_text = ""
        self._chunk_buffer = ""
        self._streaming = False
        self._t_start_ms = 0
        self._elapsed_ms = 0
        self._deep_dive_expanded = False
        self._setup_ui()
        self._connect_signals()

        # Streaming flush (batch text every 100ms to avoid jitter)
        self._flush_timer = QTimer(self)
        self._flush_timer.setInterval(100)
        self._flush_timer.timeout.connect(self._flush_chunks)
        self._flush_timer.start()

        # T+ freshness counter
        self._t_timer = QTimer(self)
        self._t_timer.setInterval(100)
        self._t_timer.timeout.connect(self._tick_counter)

        # One persistent scroll animation (never per-chunk)
        self._scroll_anim = QPropertyAnimation(
            self._body.verticalScrollBar(), b"value", self
        )
        self._scroll_anim.setDuration(240)
        self._scroll_anim.setEasingCurve(QEasingCurve.Type.OutCubic)

    # ------------------------------------------------------------------

    def _setup_ui(self) -> None:
        self.setObjectName("answerCard")
        self.setStyleSheet(
            f"QFrame#answerCard {{ background: {styles.SURFACE}; "
            f"border: 1px solid {styles.HAIRLINE}; border-radius: 12px; }}"
        )
        layout = QVBoxLayout(self)
        layout.setContentsMargins(20, 14, 20, 12)
        layout.setSpacing(8)

        # Headline row: pinned question + T+ counter
        head_row = QHBoxLayout()
        head_row.setSpacing(8)
        self._headline = QLabel("")
        self._headline.setWordWrap(True)
        self._headline.setFont(styles.make_font(styles.serif_head(), 17, italic=True))
        self._headline.setStyleSheet(f"color: {styles.SPEAKER_INTERVIEWER};")
        self._headline.setVisible(False)
        head_row.addWidget(self._headline, stretch=1)

        self._counter = QLabel("")
        self._counter.setFont(styles.make_font(styles.mono_family(), 10))
        self._counter.setStyleSheet(f"color: {styles.TEXT_TERTIARY};")
        self._counter.setAlignment(Qt.AlignmentFlag.AlignTop | Qt.AlignmentFlag.AlignRight)
        head_row.addWidget(self._counter, alignment=Qt.AlignmentFlag.AlignTop)
        layout.addLayout(head_row)

        # Answer body
        self._body = QTextBrowser()
        self._body.setReadOnly(True)
        self._body.setFrameShape(QFrame.Shape.NoFrame)
        self._body.setVerticalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAsNeeded)
        self._body.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self._body.setOpenLinks(False)
        self._body.setStyleSheet(
            f"QTextEdit {{ background: transparent; border: none; "
            f"selection-background-color: {styles.SELECTION}; }}"
        )
        self._body.document().setDefaultStyleSheet(
            f"p {{ line-height: 160%; margin: 0 0 10px 0; color: {styles.TEXT_PRIMARY}; }}"
        )
        self._body.setFont(styles.make_font(styles.serif_body(), 15))
        self._body.setPlaceholderText("Your answers will appear here — spoken-ready.")
        self._body.anchorClicked.connect(self._on_anchor)
        layout.addWidget(self._body, stretch=1)

        self._prompter = _PrompterOverlay(self._body)
        self._body.viewport().installEventFilter(self)

        # Bottom row: quick actions (left) + copy/pin (right)
        bottom = QHBoxLayout()
        bottom.setSpacing(2)
        for text, sig in (
            ("Summarize", "trigger_summarize"),
            ("Detail", "trigger_detail"),
            ("Suggest", "trigger_suggest"),
            ("Notes", "trigger_notes"),
        ):
            b = QPushButton(text)
            b.setStyleSheet(styles.GHOST_BUTTON)
            b.setCursor(Qt.CursorShape.PointingHandCursor)
            b.clicked.connect(getattr(self._signals, sig).emit)
            bottom.addWidget(b)
        bottom.addStretch()

        self._copy_btn = _GlyphButton("copy")
        self._copy_btn.setToolTip("Copy answer")
        self._copy_btn.clicked.connect(self.copy_to_clipboard)
        bottom.addWidget(self._copy_btn)

        self._pin_btn = _GlyphButton("pin")
        self._pin_btn.setToolTip("Pin answer")
        self._pin_btn.clicked.connect(self._pin_current)
        bottom.addWidget(self._pin_btn)
        layout.addLayout(bottom)

    def _connect_signals(self) -> None:
        self._signals.response_chunk.connect(self.append_chunk)
        self._signals.response_complete.connect(self.on_response_complete)
        self._signals.response_clear.connect(self.clear_display)

    # ------------------------------------------------------------------
    # Overlay geometry
    # ------------------------------------------------------------------

    def eventFilter(self, obj, event):  # noqa: N802
        if obj is self._body.viewport() and event.type() in (
            QEvent.Type.Resize, QEvent.Type.Show,
        ):
            self._prompter.setGeometry(self._body.viewport().rect())
        return super().eventFilter(obj, event)

    # ------------------------------------------------------------------
    # Streaming
    # ------------------------------------------------------------------

    def append_chunk(self, chunk: str) -> None:
        if not self._streaming:
            self._begin_stream()
        self._current_text += chunk
        self._chunk_buffer += chunk

    def _begin_stream(self) -> None:
        self._streaming = True
        self._headline.setVisible(False)
        self._deep_dive_expanded = False
        self._elapsed_ms = 0
        self._t_start_ms = 0
        self._prompter.set_streaming(True)
        self._t_timer.start()
        self._signals.assistant_state.emit("composing")

    def _flush_chunks(self) -> None:
        if not self._chunk_buffer:
            return
        text = self._chunk_buffer
        self._chunk_buffer = ""
        cursor = self._body.textCursor()
        cursor.movePosition(QTextCursor.MoveOperation.End)
        cursor.insertText(text)
        # Pull the question headline out as soon as it's available
        self._maybe_extract_headline()
        self._follow_scroll()
        self._prompter.update()

    def _maybe_extract_headline(self) -> None:
        if self._headline.isVisible():
            return
        m = re.search(r"Summarized question:\s*\n?(.+)", self._current_text)
        if m:
            q = m.group(1).splitlines()[0].strip()
            if q:
                self._headline.setText(q)
                self._headline.setVisible(True)

    def _follow_scroll(self) -> None:
        sb = self._body.verticalScrollBar()
        vh = self._body.viewport().height()
        target = max(0, sb.maximum() - int(vh * (1.0 - _PROMPTER_FRAC)))
        if abs(sb.value() - target) < 2:
            return
        self._scroll_anim.stop()
        self._scroll_anim.setStartValue(sb.value())
        self._scroll_anim.setEndValue(target)
        self._scroll_anim.start()

    def _tick_counter(self) -> None:
        self._elapsed_ms += 100
        self._counter.setText(f"T+{self._elapsed_ms / 1000:04.1f}s")

    def on_response_complete(self, full_text: str) -> None:
        self._current_text = full_text
        self._streaming = False
        self._t_timer.stop()
        self._prompter.set_streaming(False)
        self._render_formatted(expand_deep_dive=False)
        # Settle top-aligned for review
        self._scroll_anim.stop()
        self._scroll_anim.setStartValue(self._body.verticalScrollBar().value())
        self._scroll_anim.setEndValue(0)
        self._scroll_anim.start()
        self._signals.assistant_state.emit("listening")

    # ------------------------------------------------------------------
    # Formatting
    # ------------------------------------------------------------------

    def _render_formatted(self, expand_deep_dive: bool) -> None:
        headline, body_html = self._format_response(self._current_text, expand_deep_dive)
        if headline:
            self._headline.setText(headline)
            self._headline.setVisible(True)
        self._body.setHtml(body_html)

    def _on_anchor(self, url) -> None:
        if url.toString() == "#deepdive":
            self._deep_dive_expanded = True
            self._render_formatted(expand_deep_dive=True)

    @staticmethod
    def _format_response(text: str, expand_deep_dive: bool) -> tuple[str, str]:
        lines = text.split("\n")
        html: list[str] = []
        headline = ""
        in_code = False
        in_deep = False
        deep_buffer: list[str] = []

        def label(t: str) -> str:
            return (
                f'<div style="color:{styles.LABEL};font-family:\'{styles.ui_small_family()}\';'
                f'font-size:10px;font-weight:600;letter-spacing:1px;margin:14px 0 4px 0;">'
                f'{t.upper()}</div>'
            )

        def render_line(line: str) -> str:
            stripped = line.strip()
            if stripped.startswith("- "):
                return (
                    f'<div style="color:{styles.TEXT_PRIMARY};margin-left:14px;'
                    f'text-indent:-14px;"><span style="color:{styles.ACCENT};">– </span>'
                    f'{_esc(stripped[2:])}</div>'
                )
            if stripped:
                return f'<div style="color:{styles.TEXT_PRIMARY};margin:2px 0;">{_esc(stripped)}</div>'
            return "<div style='height:6px;'></div>"

        for line in lines:
            stripped = line.strip()
            if stripped.startswith("```"):
                in_code = not in_code
                html.append(
                    f'<div style="background:{styles.CODE_BG};color:{styles.CODE_TEXT};'
                    f'font-family:\'{styles.mono_family()}\';font-size:12px;padding:8px 10px;'
                    f'border-radius:6px;margin:6px 0;">' if in_code else "</div>"
                )
                continue
            target = deep_buffer if in_deep else html
            if in_code:
                target.append(f"{_esc(line)}<br>")
                continue
            if stripped.startswith("Summarized question:"):
                rest = stripped.split(":", 1)[1].strip()
                headline = rest
                continue
            if stripped in ("Answer:", "Key Points:"):
                html.append(label(stripped.rstrip(":")))
                continue
            if stripped == "Deep Dive:":
                in_deep = True
                deep_buffer.append(label("Deep Dive"))
                continue
            if stripped == "---":
                continue
            target.append(render_line(line))

        if deep_buffer:
            if expand_deep_dive:
                html.extend(deep_buffer)
            else:
                html.append(
                    f'<div style="margin:12px 0 2px 0;">'
                    f'<a href="#deepdive" style="color:{styles.ACCENT_DIM};'
                    f'font-style:italic;text-decoration:none;">Deep dive ↓</a></div>'
                )
        return headline, "".join(html)

    # ------------------------------------------------------------------

    def clear_display(self) -> None:
        self._body.clear()
        self._current_text = ""
        self._chunk_buffer = ""
        self._streaming = False
        self._headline.setVisible(False)
        self._counter.setText("")
        self._t_timer.stop()
        self._prompter.set_streaming(False)

    def copy_to_clipboard(self) -> None:
        if self._current_text:
            clipboard = QApplication.clipboard()
            if clipboard is not None:
                clipboard.setText(self._current_text)
                self._copy_btn.flash_confirm()

    def _pin_current(self) -> None:
        if self._current_text:
            self._signals.pin_response.emit(self._current_text)


def _esc(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
