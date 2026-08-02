"""Application bootstrap — creates the Qt app with asyncio integration."""

from __future__ import annotations

import asyncio
import logging
import sys

from PySide6.QtWidgets import QApplication

from ai_assistant.ui.overlay_window import OverlayWindow
from ai_assistant.ui.shortcuts import ShortcutManager
from ai_assistant.ui.signals import AppSignals
from ai_assistant.ui.tray_icon import TrayIcon

logger = logging.getLogger(__name__)


class AssistantApp:
    """Creates the QApplication, overlay, tray, and shortcuts.

    Integrates the asyncio event loop via ``qasync``.
    """

    def __init__(self) -> None:
        self._app: QApplication | None = None
        self.signals = AppSignals()
        self.overlay: OverlayWindow | None = None
        self.tray: TrayIcon | None = None
        self.shortcuts: ShortcutManager | None = None

    # ------------------------------------------------------------------
    # Setup
    # ------------------------------------------------------------------

    def setup(self) -> QApplication:
        """Create all UI components. Returns the QApplication instance."""
        self._app = QApplication.instance() or QApplication(sys.argv)
        self._app.setQuitOnLastWindowClosed(False)

        # Overlay
        self.overlay = OverlayWindow(self.signals)

        # Tray icon
        self.tray = TrayIcon(self.signals)
        self.tray.show()

        # Global shortcuts (must happen after overlay has a valid HWND)
        self.overlay.show()
        self.shortcuts = ShortcutManager(self.signals, self.overlay)
        self.shortcuts.register_defaults()
        self.overlay.set_shortcut_manager(self.shortcuts)
        # Start visible so user sees it immediately; Alt+Space to toggle
        self.overlay._apply_capture_exclusion()

        # Wire overlay visibility tracking
        self.signals.toggle_overlay.connect(self._on_toggle_overlay)

        # Wire capture exclusion from tray
        self.tray._capture_action.triggered.connect(
            lambda checked: self.overlay.set_capture_excluded(not checked)
            if self.overlay
            else None
        )

        logger.info("UI components initialised")
        return self._app

    # ------------------------------------------------------------------
    # Run (with qasync event loop integration)
    # ------------------------------------------------------------------

    def run(self) -> int:
        """Set up the UI and start the combined Qt + asyncio event loop."""
        app = self.setup()

        try:
            import qasync

            loop = qasync.QEventLoop(app)
            asyncio.set_event_loop(loop)
            with loop:
                loop.run_forever()
            return 0
        except ImportError:
            logger.warning(
                "qasync not installed — falling back to plain Qt event loop. "
                "Async features will not work."
            )
            return app.exec()

    async def run_async(self) -> None:
        """Start the UI inside an existing asyncio loop (used by main.py)."""
        app = self.setup()

        try:
            import qasync

            loop = qasync.QEventLoop(app)
            asyncio.set_event_loop(loop)
            logger.info("qasync event loop active")
        except ImportError:
            logger.warning("qasync not available — UI may not update correctly")

    # ------------------------------------------------------------------
    # Handlers
    # ------------------------------------------------------------------

    def _on_toggle_overlay(self) -> None:
        if self.overlay is None:
            return
        # toggle_visibility is on the overlay itself, but we also track state
        was_visible = self.overlay.isVisible()
        self.overlay.toggle_visibility()
        self.signals.overlay_visible_changed.emit(not was_visible)

    # ------------------------------------------------------------------
    # Cleanup
    # ------------------------------------------------------------------

    def shutdown(self) -> None:
        """Unregister hotkeys and clean up."""
        if self.shortcuts:
            self.shortcuts.unregister_all()
        logger.info("UI shutdown complete")
