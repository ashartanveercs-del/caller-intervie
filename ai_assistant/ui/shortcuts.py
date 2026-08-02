"""Global keyboard shortcuts via Win32 RegisterHotKey."""

from __future__ import annotations

import ctypes
import ctypes.wintypes
import logging
from typing import Optional

from PySide6.QtWidgets import QWidget

from ai_assistant.ui.signals import AppSignals

logger = logging.getLogger(__name__)

# Win32 constants
MOD_ALT = 0x0001
MOD_CONTROL = 0x0002
MOD_SHIFT = 0x0004
MOD_NOREPEAT = 0x4000

VK_SPACE = 0x20
VK_L = 0x4C
VK_S = 0x53
VK_D = 0x44
VK_G = 0x47
VK_N = 0x4E

WM_HOTKEY = 0x0312

# Hotkey IDs
HOTKEY_TOGGLE_OVERLAY = 1
HOTKEY_TOGGLE_LISTENING = 2
HOTKEY_SUMMARIZE = 3
HOTKEY_DETAIL = 4
HOTKEY_SUGGEST = 5
HOTKEY_NOTES = 6


class ShortcutManager:
    """Registers system-wide hotkeys using Win32 RegisterHotKey.

    The overlay window's ``nativeEvent`` must delegate to
    :meth:`handle_native_event` so that ``WM_HOTKEY`` messages are processed.
    """

    def __init__(self, signals: AppSignals, window: QWidget) -> None:
        self._signals = signals
        self._window = window
        self._registered: dict[int, tuple[int, int]] = {}

    # ------------------------------------------------------------------
    # Registration
    # ------------------------------------------------------------------

    def register_defaults(self) -> None:
        """Register the default key bindings."""
        self._register(
            HOTKEY_TOGGLE_OVERLAY,
            MOD_ALT | MOD_NOREPEAT,
            VK_SPACE,
            "Alt+Space (toggle overlay)",
        )
        self._register(
            HOTKEY_TOGGLE_LISTENING,
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
            VK_L,
            "Ctrl+Shift+L (toggle listening)",
        )
        self._register(
            HOTKEY_SUMMARIZE,
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
            VK_S,
            "Ctrl+Shift+S (summarize)",
        )
        self._register(
            HOTKEY_DETAIL,
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
            VK_D,
            "Ctrl+Shift+D (detail)",
        )
        self._register(
            HOTKEY_SUGGEST,
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
            VK_G,
            "Ctrl+Shift+G (suggest)",
        )
        self._register(
            HOTKEY_NOTES,
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
            VK_N,
            "Ctrl+Shift+N (take notes)",
        )

    def _register(
        self, hotkey_id: int, modifiers: int, vk: int, label: str
    ) -> None:
        hwnd = int(self._window.winId())
        ok = ctypes.windll.user32.RegisterHotKey(hwnd, hotkey_id, modifiers, vk)
        if ok:
            self._registered[hotkey_id] = (modifiers, vk)
            logger.info("Registered global hotkey: %s", label)
        else:
            logger.warning("Failed to register hotkey: %s (may be in use)", label)

    def unregister_all(self) -> None:
        """Unregister all hotkeys. Call on shutdown."""
        hwnd = int(self._window.winId())
        for hotkey_id in list(self._registered):
            ctypes.windll.user32.UnregisterHotKey(hwnd, hotkey_id)
            logger.debug("Unregistered hotkey id=%d", hotkey_id)
        self._registered.clear()

    # ------------------------------------------------------------------
    # Native event handler
    # ------------------------------------------------------------------

    def handle_native_event(
        self, event_type: bytes, message: int
    ) -> Optional[bool]:
        """Process WM_HOTKEY messages. Returns True if handled."""
        if event_type != b"windows_generic_MSG":
            return None

        try:
            msg = ctypes.wintypes.MSG.from_address(message)
        except Exception:
            return None

        if msg.message != WM_HOTKEY:
            return None

        hotkey_id = msg.wParam
        if hotkey_id == HOTKEY_TOGGLE_OVERLAY:
            self._signals.toggle_overlay.emit()
            return True
        elif hotkey_id == HOTKEY_TOGGLE_LISTENING:
            self._signals.toggle_listening.emit()
            return True
        elif hotkey_id == HOTKEY_SUMMARIZE:
            self._signals.trigger_summarize.emit()
            return True
        elif hotkey_id == HOTKEY_DETAIL:
            self._signals.trigger_detail.emit()
            return True
        elif hotkey_id == HOTKEY_SUGGEST:
            self._signals.trigger_suggest.emit()
            return True
        elif hotkey_id == HOTKEY_NOTES:
            self._signals.trigger_notes.emit()
            return True

        return None
