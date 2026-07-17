"""Design system for THE QUIET PROMPTER — palette, fonts, and global QSS.

A warm-dark editorial teleprompter. Design law: exactly four state colors —
brass = "the machine is writing", sage = "listening", terracotta = "attention,
not danger", one error red. No other color ever carries state.
"""

from __future__ import annotations

from PySide6.QtGui import QFont, QFontDatabase

# ---------------------------------------------------------------------------
# Palette (exact hex; alpha noted where painted)
# ---------------------------------------------------------------------------

BG = "#191512"                 # window backdrop (painted, radius 14, hairline border)
BG_ALPHA = 238                 # 210 when acrylic is enabled
SURFACE = "#211B15"            # answer card, chat well, settings tray
SURFACE_RAISED = "#2A231B"     # pinned cards, combo popup, tooltips

HAIRLINE = "rgba(242,233,220,23)"        # all borders/dividers
HAIRLINE_FOCUS = "rgba(201,162,94,140)"  # focused chat input border

TEXT_PRIMARY = "#F0E7D8"       # answer body, final transcript
TEXT_SECONDARY = "#A5977F"     # support text, status word, ghost buttons
TEXT_TERTIARY = "#6B5F4E"      # timestamps, interim transcript, hints, T+ counter
LABEL = "#8C7D66"              # small-caps section labels

ACCENT = "#C9A25E"             # brass — streaming, prompter line, focus, glyphs
ACCENT_DIM = "#9C7F4C"         # ghost links, quick-action hover text
ACCENT_PRESSED = "rgba(201,162,94,36)"

SUCCESS = "#8FB98B"            # sage — listening, active mode, switch ON, copy-confirm
WARN = "#D08B5B"              # terracotta — suggestion mode, clipping, reconnecting
ERROR = "#C96A57"             # capture failure, stream failure, error dot

SPEAKER_YOU = "#97B4C9"
SPEAKER_INTERVIEWER = "#D3A57C"

SELECTION = "rgba(201,162,94,60)"
CODE_BG = "#14100C"
CODE_TEXT = "#C9BFA8"

# Legacy aliases kept so any old import still resolves
BACKGROUND_COLOR = f"rgba(25, 21, 18, {BG_ALPHA})"
TEXT_COLOR = TEXT_PRIMARY
BORDER_RADIUS = 14

MODE_COLORS: dict[str, str] = {
    "passive": TEXT_TERTIARY,
    "suggestion": WARN,
    "active": SUCCESS,
}

# High-level assistant states → (dot color, status word)
STATE_STYLES: dict[str, tuple[str, str]] = {
    "loading": (TEXT_TERTIARY, "Warming up…"),
    "listening": (SUCCESS, "Listening"),
    "composing": (ACCENT, "Composing…"),
    "paused": (TEXT_TERTIARY, "Paused — Ctrl+Shift+L"),
    "error": (ERROR, "Reconnecting"),
}

# ---------------------------------------------------------------------------
# Fonts — probed once against the running Qt font database
# ---------------------------------------------------------------------------

_SERIF_HEAD = ["Sitka Heading", "Sitka", "Georgia", "serif"]
_SERIF_BODY = ["Sitka Text", "Sitka", "Georgia", "serif"]
_UI = ["Segoe UI Variable Text", "Segoe UI", "sans-serif"]
_UI_SMALL = ["Segoe UI Variable Small", "Segoe UI", "sans-serif"]
_MONO = ["Cascadia Mono", "Cascadia Code", "Consolas", "monospace"]

_families_cache: set[str] | None = None


def _available() -> set[str]:
    global _families_cache
    if _families_cache is None:
        try:
            _families_cache = set(QFontDatabase.families())
        except Exception:
            _families_cache = set()
    return _families_cache


def _first_available(stack: list[str]) -> str:
    fams = _available()
    for name in stack:
        if name in fams:
            return name
    return stack[-1]


def serif_head() -> str:
    return _first_available(_SERIF_HEAD)


def serif_body() -> str:
    return _first_available(_SERIF_BODY)


def ui_family() -> str:
    return _first_available(_UI)


def ui_small_family() -> str:
    return _first_available(_UI_SMALL)


def mono_family() -> str:
    return _first_available(_MONO)


def _css_stack(stack: list[str]) -> str:
    return ", ".join(f'"{n}"' if " " in n else n for n in stack)


def make_font(family: str, pixel: int, weight: int = 400,
              italic: bool = False, letter_spacing: float = 0.0) -> QFont:
    f = QFont(family)
    f.setPixelSize(pixel)
    f.setWeight(QFont.Weight(weight) if weight in (100, 200, 300, 400, 500, 600, 700, 800, 900) else QFont.Weight.Normal)
    f.setItalic(italic)
    if letter_spacing:
        f.setLetterSpacing(QFont.SpacingType.AbsoluteSpacing, letter_spacing)
    return f


# ---------------------------------------------------------------------------
# Global stylesheet — the shared chrome for the overlay
# ---------------------------------------------------------------------------

OVERLAY_STYLESHEET = f"""
QWidget {{
    color: {TEXT_PRIMARY};
    font-family: {_css_stack(_UI)};
}}
QTextEdit, QPlainTextEdit {{
    background: transparent;
    color: {TEXT_PRIMARY};
    border: none;
    selection-background-color: {SELECTION};
}}
QLabel {{ background: transparent; }}
QToolTip {{
    background: {SURFACE_RAISED};
    color: {TEXT_PRIMARY};
    border: 1px solid {HAIRLINE};
    padding: 4px 8px;
}}
QScrollBar:vertical {{
    background: transparent; width: 4px; margin: 0;
}}
QScrollBar::handle:vertical {{
    background: rgba(242,233,220,34); border-radius: 2px; min-height: 24px;
}}
QScrollBar::handle:vertical:hover {{ background: rgba(201,162,94,90); }}
QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical {{ height: 0; }}
QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical {{ background: transparent; }}
"""

# Ghost text button (header toggles, quick actions)
GHOST_BUTTON = f"""
QPushButton {{
    background: transparent; color: {TEXT_SECONDARY};
    border: none; border-radius: 6px; padding: 4px 8px;
    font-family: {_css_stack(_UI)}; font-size: 11px; font-weight: 500;
}}
QPushButton:hover {{ background: rgba(242,233,220,14); color: {ACCENT}; }}
QPushButton:pressed {{ background: {ACCENT_PRESSED}; }}
QPushButton:checked {{ color: {TEXT_PRIMARY}; }}
"""

COMBO_STYLE = f"""
QComboBox {{
    background: {SURFACE_RAISED}; color: {TEXT_PRIMARY};
    border: 1px solid {HAIRLINE}; border-radius: 6px;
    padding: 3px 8px; font-size: 11px; min-width: 120px;
}}
QComboBox::drop-down {{ border: none; width: 16px; }}
QComboBox QAbstractItemView {{
    background: {SURFACE_RAISED}; color: {TEXT_PRIMARY};
    selection-background-color: {SELECTION};
    border: 1px solid {HAIRLINE}; outline: 0;
}}
"""
