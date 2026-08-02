"""Regression tests for pinned answer card removal."""

import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QCoreApplication, QEvent, Qt
from PySide6.QtWidgets import QApplication

from ai_assistant.ui.pinned_panel import PinnedItem, PinnedPanel
from ai_assistant.ui.signals import AppSignals


def _flush_deferred_deletes() -> None:
    QCoreApplication.sendPostedEvents(None, QEvent.Type.DeferredDelete)
    QApplication.processEvents()


def test_removing_pins_out_of_order_removes_the_correct_cards():
    app = QApplication.instance() or QApplication([])
    panel = PinnedPanel(AppSignals())
    panel.add_pinned("A")
    panel.add_pinned("B")
    panel.add_pinned("C")

    panel.remove_pinned(1)
    _flush_deferred_deletes()
    panel.remove_pinned(2)
    _flush_deferred_deletes()

    cards = panel._container.findChildren(
        PinnedItem, options=Qt.FindChildOption.FindDirectChildrenOnly
    )
    assert panel.pin_count == 1
    assert [card._text for card in cards] == ["A"]
    panel.deleteLater()
    _flush_deferred_deletes()
    assert app is not None
