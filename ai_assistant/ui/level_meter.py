"""Ember audio meter — a 2px luminous line, no traffic-light, no peak tick.

Position + color carry speaker identity (You above, Interviewer below).
Ballistics run off the shared :class:`Heartbeat` so there is one repaint clock
regardless of how fast level signals arrive. An armed-but-silent line breathes
so a live mic always reads as armed; a disarmed line renders as a dashed
hairline.
"""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QPainter, QPaintEvent, QPen
from PySide6.QtWidgets import QWidget

from ai_assistant.ui.effects import heartbeat


class LevelMeter(QWidget):
    """Horizontal ember line displaying a smoothed audio level (0.0–1.0)."""

    def __init__(self, label: str = "", color: str = "#97B4C9",
                 parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._color = QColor(color)
        self._raw = 0.0
        self._lvl = 0.0
        self._armed = True
        self.setFixedHeight(2)
        self.setMinimumWidth(60)
        heartbeat().tick.connect(self._on_tick)

    # -- public API (unchanged contract) --------------------------------

    def set_level(self, level: float) -> None:
        self._raw = max(0.0, min(1.0, level))

    def set_armed(self, armed: bool) -> None:
        self._armed = armed
        self.update()

    # -- ballistics -----------------------------------------------------

    def _on_tick(self) -> None:
        prev = self._lvl
        # Noise gate: ignore tiny background levels so the line sits calm in silence
        raw = self._raw if self._raw > 0.04 else 0.0
        if raw > self._lvl:
            # gentler attack — less jumpy on transients
            self._lvl = self._lvl * 0.6 + raw * 0.4
        else:
            self._lvl *= 0.86  # smoother release
        self._raw *= 0.55
        if self._lvl < 0.005:
            self._lvl = 0.0
        # Only repaint when something visibly changed (or while idle-pulsing an
        # armed line) — avoids constant flicker/jitter when nothing is happening.
        if abs(self._lvl - prev) > 0.004 or (self._armed and self._lvl < 0.05):
            self.update()

    # -- painting -------------------------------------------------------

    def paintEvent(self, event: QPaintEvent) -> None:  # noqa: N802
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        w, h = self.width(), self.height()

        if not self._armed:
            pen = QPen(QColor(242, 233, 220, 20))
            pen.setWidthF(1.0)
            pen.setStyle(Qt.PenStyle.CustomDashLine)
            pen.setDashPattern([3, 3])
            p.setPen(pen)
            y = h / 2
            p.drawLine(0, int(y), w, int(y))
            p.end()
            return

        # Track
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(QColor(242, 233, 220, 12))
        p.drawRoundedRect(0, 0, w, h, 1, 1)

        # Fill
        fill_w = int(self._lvl * w)
        if fill_w > 1:
            c = QColor(self._color)
            c.setAlpha(min(255, int(60 + self._lvl * 160)))
            p.setBrush(c)
            p.drawRoundedRect(0, 0, fill_w, h, 1, 1)

        # Armed-idle pulse on the leftmost sliver, phased with the breathing dot
        if self._lvl < 0.05:
            phase = heartbeat().breath  # 0.35 .. 1.0
            a = int(18 + (40 - 18) * ((phase - 0.35) / 0.65))
            c = QColor(self._color)
            c.setAlpha(a)
            p.setBrush(c)
            p.drawRoundedRect(0, 0, 10, h, 1, 1)

        p.end()
