# THE QUIET PROMPTER — Final Implementation Spec v1.0

Base: Direction 2 (Quiet Prompter). Grafts: from FLIGHT DECK — answer-age telemetry counter, footer state microtext, handoff choreography, meter ballistics + armed-idle cue, calm-failure doctrine; from FROSTLINE — glance mode, single-open drawers, count badges, copy-confirm glyph swap, capture-exclusion re-apply insurance, unarmed-meter dashed affordance.

Design law: brass means "the machine is writing", sage means "the machine is listening", terracotta means "attention but not danger", one error red. No other color ever carries state. No gradients except three sanctioned painted ones (prompter-line end fades, viewport top scrim, ember alpha ramp). The answer card is the only card on screen.

---

## 1. PALETTE (exact hex; alpha noted where painted)

| Token | Value | Usage |
|---|---|---|
| bg | #191512 @ alpha 238 (drop to 210 if acrylic on) | Window backdrop, painted in OverlayWindow.paintEvent, radius 14, 1px hairline border |
| surface | #211B15 opaque | Answer card, chat input well, settings tray |
| surface-raised | #2A231B opaque | Pinned item cards, combo popup, tooltips |
| hairline | rgba(242,233,220,23) | ALL borders and dividers; never brighter |
| hairline-focus | rgba(201,162,94,140) | Focused chat input border only |
| text-primary | #F0E7D8 | Answer body, final transcript |
| text-secondary | #A5977F | Key Points support, status word, ghost buttons, placeholders' parent labels |
| text-tertiary | #6B5F4E | Timestamps, interim transcript, kbd hints, T+ counter, footer microtext, placeholder |
| label | #8C7D66 | Small-caps section labels (ANSWER / KEY POINTS / DEEP DIVE) |
| accent (brass) | #C9A25E | Streaming caret, prompter line, focus ring, send glyph, bullet glyphs, drop-invite border, count badges |
| accent-dim | #9C7F4C | Ghost-link "Deep dive ↓", quick-action hover text |
| accent-pressed | rgba(201,162,94,36) | Pressed background on ghost buttons |
| success (sage) | #8FB98B | Listening dot, active mode, interviewer-audio switch ON, copy-confirm check |
| warn (terracotta) | #D08B5B | Suggestion mode, clipping, reconnecting, click-through-on microtext |
| error | #C96A57 | Capture-exclusion failure, stream failure, error dot |
| speaker-you | #97B4C9 | You ember meter + transcript speaker name |
| speaker-interviewer | #D3A57C | Interviewer ember + speaker name + question headline tint |
| selection | rgba(201,162,94,60) | selection-background-color everywhere |
| code-bg / code-text | #14100C / #C9BFA8 | Code blocks in answers |
| MODE_COLORS | passive #6B5F4E · suggestion #D08B5B · active #8FB98B | Mode toggle word + tray dot |

---

## 2. TYPOGRAPHY (all stock Windows 11; probe families via QFontDatabase.families() at startup)

| Role | Stack | Size / LH / Weight | Color |
|---|---|---|---|
| Question headline | "Sitka Heading", Georgia, serif — ITALIC | 17px / 1.35 / 400 | #D3A57C |
| Answer body (hero) | "Sitka Text", "Sitka", Georgia, serif | 15px / 1.6 / 400, 10px paragraph spacing | #F0E7D8 |
| Key Points | same serif | 14px / 1.5, en-dash "–" bullet in #C9A25E, hanging indent 14px | #F0E7D8 |
| Section labels | "Segoe UI Variable Small", "Segoe UI" | 10px / 600, UPPERCASE via .upper(), letter-spacing +1.4px via QFont.setLetterSpacing(QFont.AbsoluteSpacing, 1.4) | #8C7D66 |
| Status word | "Segoe UI Variable Small", "Segoe UI" | 11px / 600 | #A5977F |
| Transcript body | "Segoe UI Variable Text", "Segoe UI" | 12px / 1.45 | final #F0E7D8, interim #6B5F4E |
| Speaker names | same | 11px / 600 | speaker colors |
| Chat input | same | 13px | #F0E7D8, placeholder #6B5F4E |
| Ghost buttons | same | 11px / 500 | #A5977F |
| Kbd hints | "Segoe UI" | 9.5px | #6B5F4E |
| T+ counter (GRAFT) | "Cascadia Mono", "Cascadia Code", Consolas | 9.5px | #6B5F4E — mono guarantees zero digit jitter |
| Footer microtext (GRAFT) | same mono | 9px | #6B5F4E |
| Code blocks | same mono | 12px / 1.45 | #C9BFA8 |

QSS has no line-height/letter-spacing/text-transform: line-height goes into `document().setDefaultStyleSheet("p{line-height:160%; margin:0 0 10px 0;}")` on the answer QTextEdit; letter-spacing and uppercase in code. Never fix heights around serif text — Sitka→Georgia fallback shifts metrics; use layouts.

---

## 3. LAYOUT TREE

Window: 460×560 default, min 380×420, max width 620. Docked bottom-right, 24px screen margin. Outer radius 14. Frameless, WA_TranslucentBackground, always-on-top Tool window; paintEvent draws bg fill + 1px hairline border. Root QVBoxLayout margins 16,12,16,12, spacing 8. Spacing scale 4/8/12/16/24. QSizeGrip bottom-right, transparent; existing edge-resize logic unchanged.

1. **HEADER (30px fixed, entire strip = drag handle)** — QHBoxLayout: [BreathingDot 8px] 8 [status QLabel] stretch [shield glyph 14px] 12 [gear glyph] 12 [three ghost text toggles "Transcript" "Notes" "Pins", each with a count badge when count > 0 (GRAFT)]. No app title anywhere — the app never names itself on screen.
2. **EMBER METERS (6px total)** — two full-width 2px rounded lines, 2px gap. Top = You #97B4C9, bottom = Interviewer #D3A57C. No labels; position + color carry identity. Unarmed interviewer line renders as dashed hairline rgba(242,233,220,20), dash pattern [3,3]; clicking its right half toggles interviewer-audio arm (GRAFT). Armed-but-silent lines breathe (interaction 10) so a live mic always reads as armed (GRAFT).
3. **ANSWER CARD (stretch=1, ~65% height, minimum 200px)** — QFrame `#answerCard`, margins 20,16,20,12: [headline row: question QLabel (wordwrap) + stretch + T+ counter QLabel (GRAFT)] [QTextEdit answer body, frameless, transparent] [bottom row: ghost actions "Summarize · Detail · Suggest · Notes" left, stretch, Copy / Pin 22px glyph buttons right]. Deep Dive streams into the buffer but renders folded behind a serif-italic ghost link "Deep dive ↓" (#9C7F4C) at document end. The prompter-line overlay is a child widget over the viewport (Section 4). The card is never collapsible.
4. **DRAWERS (all collapsed to 0 by default; frameless, sit directly on backdrop — visually subordinate to the card)**: TRANSCRIPT opens to 128px, NOTES 88px, PINNED 110px (small #2A231B cards, radius 8, 8px padding, first line + unpin glyph). Hairline divider above the zone. Single-open rule: opening one closes any open sibling in the same QParallelAnimationGroup (GRAFT) — the hero never loses more than 128px.
5. **CHAT INPUT (38px)** — hairline-top divider, then: [mode text toggle left: "passive ›" / "suggest ›" / "active ›", 11px/600, colored by MODE_COLORS, click cycles] [borderless QLineEdit on #211B15 well, radius 10] [brass "↵" glyph QLabel overlaid right, fades in only when text non-empty]. No Send button; Enter submits.
6. **FOOTER (20px)** — [state microtext left, 9px Cascadia Mono #6B5F4E: "● excluded from capture · click-through off" (GRAFT — capture-exclusion state always visible calms the user)] [centered "···" ghost button → settings tray] [right: resize-grip zone]. Settings tray (animated slide-down): mic QComboBox, interviewer-audio switch row, system-prompt QPlainTextEdit (60px), Add Documents button + doc status line. All existing signals (mic_changed, system_audio_changed, files_dropped, system_prompt_changed) rewired unchanged — relocated, not rewritten.

**GLANCE MODE (GRAFT)**: double-click on header animates drawers/chat/footer maximumHeight→0 and resizes the window to 460×~300 — header + meters + answer card + ghost actions only: the interview steady-state. Double-click restores. Alt+Space show/hide unchanged. Ctrl+Shift+S/D/G/N and Ctrl+Shift+L global hotkeys unchanged and work in glance mode.

---

## 4. COMPONENT SPECS + QSS

**BreathingDot** — QWidget, paintEvent draws antialiased 8px circle. Colors/copy: listening #8FB98B "Listening" · composing #C9A25E "Composing…" · paused #6B5F4E "Paused — Ctrl+Shift+L" · error #C96A57 "Reconnecting". Status QLabel: `color:#A5977F; font:600 11px "Segoe UI Variable Small";`

**Shield glyph** — 14px QPainter path in the header: brass #C9A25E fill when SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) returned success; #C96A57 when it failed (tooltip: "Window may be visible to screen capture"). This is the only always-on trust indicator besides the footer microtext.

**Panel toggles + count badges** — ghost text buttons: `QPushButton { background:transparent; color:#A5977F; border:none; border-radius:6px; padding:4px 8px; font:500 11px "Segoe UI Variable Text"; } :hover { background:rgba(242,233,220,14); color:#C9A25E; } :checked { color:#F0E7D8; }`. Badge = child QLabel: `background:rgba(201,162,94,40); color:#C9A25E; border-radius:7px; padding:0 5px; font-size:9px;` showing pin/line counts (GRAFT).

**EmberMeter (LevelMeter rewrite)** — setFixedHeight(2). paintEvent: track full-width rounded rect QColor(242,233,220,12); fill = speaker color at alpha 60 + level×160, width = smoothed level fraction. No peak tick, no traffic-light. Ballistics on a single 33ms master QTimer: attack `lvl = max(new, lvl*0.4 + new*0.6)`, release `lvl *= 0.90` per tick (GRAFT: timer-clock ballistics, no repaint-per-signal). Armed-idle: leftmost 10px pulses alpha 18↔40 in phase with the breathing dot. Unarmed: dashed hairline as above.

**Answer card** — `QFrame#answerCard { background:#211B15; border:1px solid rgba(242,233,220,23); border-radius:12px; }`. Body: `QTextEdit { background:transparent; border:none; selection-background-color: rgba(201,162,94,60); }` with QFont("Sitka Text"), setPixelSize(15).

**_format_response HTML rules** — "Summarized question:" → strip prefix, promote text to headline QLabel (italic clay, outside the scroll region so it NEVER scrolls away). Section markers → `<div style="color:#8C7D66;font-family:'Segoe UI Variable Small';font-size:10px;font-weight:600;margin:14px 0 4px 0;">ANSWER</div>` (also KEY POINTS, DEEP DIVE). Bullets → `<div style="color:#F0E7D8;margin-left:14px;text-indent:-14px;"><span style="color:#C9A25E;">– </span>…</div>`. Code fences → `<div style="background:#14100C;color:#C9BFA8;font-family:'Cascadia Mono';font-size:12px;padding:8px 10px;border-radius:6px;">`. Deep Dive: buffered but rendered as ghost anchor `<a href="#deepdive" style="color:#9C7F4C;font-style:italic;">Deep dive ↓</a>`; setOpenLinks(False), anchorClicked re-renders document expanded.

**T+ counter (GRAFT)** — QLabel top-right in card headline row, Cascadia Mono 9.5px #6B5F4E, text "T+04.2s". QTimer(100ms) starts at first response chunk, freezes on response_complete, hides on clear. Answers "how fresh is what I'm reading" in peripheral vision.

**Copy / Pin** — 22px glyph-only ghost buttons (QPainter icons). Copy confirm: glyph swaps to a #8FB98B checkmark, QTimer.singleShot(1200) reverts. No toast — toasts steal glances (GRAFT).

**Quick actions** — shared QSS: `QPushButton { background:transparent; color:#A5977F; border:none; border-radius:6px; padding:4px 8px; font:500 11px "Segoe UI Variable Text"; } :hover { background:rgba(242,233,220,14); color:#C9A25E; } :pressed { background:rgba(201,162,94,36); }`. Verb-first text with subordinate kbd: "Summarize ⌃⇧S" (probe glyph rendering; fallback "Ctrl·Shift·S" at 9.5px). Muted "·" interpunct QLabels between.

**Transcript** — QTextEdit, transparent/frameless, HTML: speaker `<span style="color:#97B4C9;font-weight:600;font-size:11px;">You</span>` (interviewer #D3A57C); final text #F0E7D8; interim appended as `<span style="color:#6B5F4E;">` tracked by saved QTextCursor positions and deleted-and-rewritten on every interim update, promoted to #F0E7D8 on final — pencil settles into ink. Auto-scroll pauses while pointer hovers the panel.

**Chat input** — `QLineEdit { background:#211B15; color:#F0E7D8; border:1px solid rgba(242,233,220,23); border-radius:10px; padding:7px 30px 7px 12px; font:13px "Segoe UI Variable Text"; } :focus { border:1px solid rgba(201,162,94,140); }`.

**Combo + popup** — `QComboBox { background:#2A231B; color:#F0E7D8; border:1px solid rgba(242,233,220,23); border-radius:6px; padding:3px 8px; font:11px "Segoe UI Variable Text"; } QComboBox QAbstractItemView { background:#2A231B; color:#F0E7D8; selection-background-color:rgba(201,162,94,60); border:1px solid rgba(242,233,220,23); outline:0; }`.

**Interviewer-audio switch** — label #A5977F + 30×16 QPainter track/thumb: track #3A322A off / #8FB98B on. Mirrors the meter-click arm toggle.

**Scrollbars (global)** — `QScrollBar:vertical { background:transparent; width:4px; } QScrollBar::handle:vertical { background:rgba(242,233,220,34); border-radius:2px; min-height:24px; } QScrollBar::handle:vertical:hover { background:rgba(201,162,94,90); }` — zero add/sub-line.

**Tray icon** — 16px QPainter: brass „ opening quotation mark on transparent; 5px corner dot: sage listening / #6B5F4E paused / #C96A57 when capture exclusion is OFF (GRAFT: peripheral danger awareness). Default-styled tray menu — deliberately boring; the tray is the only surface others may see.

**Drop invite** — dragEnterEvent sets flag; paintEvent adds 1.5px #C9A25E inner rounded stroke at alpha 120 + centered ghost label "Drop PDF / DOCX / TXT / MD" over the hero; instant clear on dragLeave/drop (no animation — feedback must not lag the cursor).

**Prompter-line overlay** — transparent child QWidget over the answer QTextEdit viewport, setAttribute(WA_TransparentForMouseEvents) so selection still works. Paints: (a) 1px brass rule at 38% of viewport height, QLinearGradient alpha 0→110→0 across its width; (b) top scrim — QLinearGradient over the viewport's top 24px fading text toward #211B15 so history dissolves upward. Repaint only on scrollbar valueChanged and streaming state changes.

---

## 5. MICRO-INTERACTIONS (Qt mechanisms; every non-ambient anim ≤300ms except the 1200ms exhale)

1. **Breathing dot** — QVariantAnimation 0.35→1.0→0.35, 2600ms, InOutSine, loopCount(-1), driving painted alpha. Composing: retarget color to brass and shorten period to 1600ms — the room quickens. Frozen solid when paused; static #C96A57 on error (motion only when there is signal).
2. **THE HANDOFF (signature, GRAFT choreography)** — on first response chunk, one 400ms beat: status word → "Composing…", dot → brass/1600ms tempo, prompter line fades in (QVariantAnimation alpha 0→110, 300ms OutQuad), T+ counter starts, caret appears. On response_complete: prompter line exhales (1200ms InOutSine to alpha 0), T+ freezes, dot returns sage/2600ms, and a 2px brass top-edge line inside the card fades 600ms OutCubic — a peripheral "answer locked" cue readable without focusing.
3. **Teleprompter auto-scroll** — ONE persistent QPropertyAnimation(scrollbar, b"value", 240ms, OutCubic). Each 100ms flush tick: find last sentence-terminal punctuation, map its cursorRect() y, setEndValue so that baseline lands on the 38% prompter line, start() (restart retargets smoothly). Never spawn per-chunk animations. Follow-mode disengages when the user wheels >12px above target; ghost chip "↓ Following off — click to resume" fades in bottom-center (QGraphicsOpacityEffect, 180ms). When streaming ends, document settles top-aligned for review.
4. **Answer arrival** — QParallelAnimationGroup: body opacity 0→1 (QGraphicsOpacityEffect + QPropertyAnimation, 280ms OutQuad) + headline pos up 6px (QPropertyAnimation). Effect removed on finish (lingering effects break subpixel text rendering).
5. **Streaming caret** — 2×18px brass rect painted at the end-cursor rect via viewport event filter; QVariantAnimation alpha 255→70, 900ms InOutSine loop while streaming; 400ms fade-out on complete.
6. **Drawers** — QParallelAnimationGroup: maximumHeight 0→natural (240ms InOutCubic) + opacity 0→1 (180ms); collapse reverse at 200ms; the closing sibling's animation runs in the same group (single-open, GRAFT). Never setVisible() cold. Do not animate the hero's geometry while streaming.
7. **Interim→final transcript** — color snap only (#6B5F4E → #F0E7D8); QTextEdit spans can't tween color cheaply, and the snap alone reads as "settled".
8. **Window show/hide (Alt+Space)** — QParallelAnimationGroup: windowOpacity 0→1 (180ms InOutQuad) + pos rising 10px (OutCubic). Hide reverses at 150ms.
9. **Send glyph** — QGraphicsOpacityEffect 150ms fade tied to textChanged emptiness.
10. **Ember ballistics + armed breathe** — no animation objects; 33ms master QTimer does smoothing math and the idle-armed pulse (phase-shared with the dot's driver value).
11. **Click-through toggle (GRAFT)** — window opacity eases to 0.85 (QPropertyAnimation 200ms), footer microtext → "click-through on" in #D08B5B. Driven by the toggle handler, never hover — WS_EX_TRANSPARENT windows receive no hover events.
12. **Glance mode** — QParallelAnimationGroup collapsing drawers/chat/footer maximumHeight→0 + animated resize to 460×~300, 220ms OutCubic; restore reverse.
13. **Error** — no shake, no toast: status word "Reconnecting", dot static #C96A57, shield/tray handle capture failures. Calm failure is a feature for a stressed user (GRAFT doctrine).

Ambient loops permitted: breathing dot, streaming caret, armed-ember pulse. Nothing else moves unattended.

---

## 6. ENGINEERING GUARDRAILS

1. **Acrylic is optional garnish, OFF by default.** If enabled (settings toggle): ctypes SetWindowCompositionAttribute ACCENT_ENABLE_ACRYLICBLURBEHIND, tint #191512 at ~0.55; drop painted bg alpha 238→210. Disable acrylic on mousePress-drag, restore on release (known DWM drag lag). Do NOT use DWMWA_SYSTEMBACKDROP_TYPE — it fights WA_TranslucentBackground on frameless Tool windows. The painted backdrop is the designed, complete look.
2. **Capture exclusion is the product.** Re-apply SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) after every show(), after enabling acrylic, and at the end of set_click_through (GWL_EXSTYLE changes can force recreation on some drivers) (GRAFT). Verify the return value each time and drive the shield glyph + tray dot + footer microtext from it.
3. **One QGraphicsEffect per widget, ever.** No effect of any kind on the streaming QTextEdit (full-widget raster per chunk). Remove opacity effects on animation finish. All glows are painted gradients.
4. **QSS gaps** — letter-spacing via QFont.setLetterSpacing, uppercase via .upper(), line-height via document().setDefaultStyleSheet. Nothing in this spec depends on unsupported QSS.
5. **Fonts** — probe QFontDatabase.families() for "Sitka Text", "Sitka Heading", "Segoe UI Variable Small", "Cascadia Mono"; declared fallbacks: Georgia / "Segoe UI" / Consolas. No fixed heights around serif text.
6. **Streaming** — keep the existing 100ms flush buffer; one persistent retargeted scroll animation; sentence-detection regex on the flushed tail only (never rescan the whole document); if cursorRect mapping jitters during reflow, target scrollbar.maximum() minus (viewport height × 0.62) as the cheap approximation — the prompter line still reads correctly.
7. **Wiring** — all AppSignals, global hotkeys (Alt+Space, Ctrl+Shift+S/D/G/N/L), edge-resize, and drag-drop plumbing survive untouched; settings controls are relocated behind the footer tray, not rewritten.

## 7. ACCEPTANCE CHECKLIST

- 1-second glance test: from 60cm, with the window at 460px, the current answer sentence at the prompter line is readable and the composing/complete state is identifiable without reading any text.
- Silent-but-armed mic shows the ember pulse; disarmed interviewer line shows dashed.
- Capture exclusion state visible in three places (shield, tray dot, footer microtext) and re-verified after show/acrylic/click-through changes.
- No animation object is created per streaming chunk; CPU stays flat during a 60s stream with a video call running.
- Kill acrylic: the window still looks finished.
- Glance mode round-trips via double-click with no layout debris; hotkeys work while collapsed.
- Interim transcript text visibly settles from pencil (#6B5F4E) to ink (#F0E7D8) in place, with no duplicate lines.