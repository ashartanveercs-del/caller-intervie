"""Shared animation primitives for the overlay.

A single :class:`Heartbeat` drives all ambient motion (breathing status dot,
armed-ember pulse) from one 33 ms timer with a continuous phase, so nothing
drifts out of sync and there is exactly one repaint clock.
"""

from __future__ import annotations

import math

from PySide6.QtCore import QObject, QTimer, Signal


class Heartbeat(QObject):
    """Emits ``tick`` every ~33 ms and exposes a phase-continuous breath value."""

    tick = Signal()

    def __init__(self) -> None:
        super().__init__()
        self._phase = 0.0
        self._period_ms = 2600.0
        self.breath = 0.35  # eased 0.35 .. 1.0
        self._timer = QTimer(self)
        self._timer.setInterval(33)
        self._timer.timeout.connect(self._on_tick)
        self._running = False

    def start(self) -> None:
        if not self._running:
            self._timer.start()
            self._running = True

    def set_period(self, ms: float) -> None:
        """Change tempo without a phase jump (e.g. quicken while composing)."""
        self._period_ms = max(200.0, float(ms))

    def _on_tick(self) -> None:
        self._phase += 2.0 * math.pi * 33.0 / self._period_ms
        if self._phase > 2.0 * math.pi:
            self._phase -= 2.0 * math.pi
        s = 0.5 + 0.5 * math.sin(self._phase)
        self.breath = 0.35 + 0.65 * s
        self.tick.emit()


_HEARTBEAT: Heartbeat | None = None


def heartbeat() -> Heartbeat:
    """Return the process-wide heartbeat (create + start on first use)."""
    global _HEARTBEAT
    if _HEARTBEAT is None:
        _HEARTBEAT = Heartbeat()
        _HEARTBEAT.start()
    return _HEARTBEAT


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t
