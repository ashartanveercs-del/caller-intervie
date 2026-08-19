# Minimal Live Overlay and Simplified Interview UX Design

**Date:** 2026-08-19  
**Status:** Approved design, pending written-spec review  
**Scope:** CallerInterview native Interview Mode on Windows and macOS

## 1. Goal

CallerInterview must stop behaving like a dashboard during a live conversation. Preparation should be a short, obvious path to `Start`, and the live experience should be a small, semi-transparent teleprompter that stays out of the user's way.

The main window remains the workspace for setup, settings, history, and review. A separate native overlay becomes the only normal Live-mode surface.

## 2. Product Principles

1. **Glanceable under pressure.** The user should understand the current suggestion in one glance without scanning panels.
2. **Answer first.** Live mode shows the current answer scaffold, not a transcript, dashboard, or settings surface.
3. **Stable geometry.** Streaming text, health changes, and new questions must not resize or move the overlay.
4. **Optional depth.** Context and advanced controls remain available but do not appear in the default path.
5. **Private, not deceptive.** The overlay uses confirmed operating-system capture exclusion but does not claim to evade proctoring, process inspection, device management, cameras, or privileged capture software.
6. **Fast feedback.** Partial answer content appears as soon as it is available; the UI never waits for a complete long response before rendering useful guidance.

## 3. Approaches Considered

### 3.1 Separate native overlay window (selected)

Create a frameless, protected, always-on-top Tauri window for Live mode. Hide the main application only after the overlay has loaded, hydrated the active session, and confirmed capture protection.

This adds explicit cross-window lifecycle and state synchronization, but it produces the correct product behavior: a small teleprompter independent of the setup and review application.

### 3.2 Resize the main window

Reuse the existing renderer and shrink the current window during Live mode. This avoids cross-window state, but mixes incompatible responsibilities, complicates restoration, and makes the live surface vulnerable to existing application chrome and layout changes.

### 3.3 Full window with a compact mode

Keep the current Live page and add a compact toggle. This is the smallest code change but preserves the wrong default experience and leaves too many controls, panels, and navigation elements visible during the conversation.

## 4. Application Structure

CallerInterview has two native windows with distinct responsibilities.

### 4.1 Main window

The main window owns:

- Home and mode selection;
- interview preparation;
- provider, language, audio, account, and privacy settings;
- saved sessions and reusable context;
- post-session review.

The main window is hidden during a normal live session. It remains a normal application window in the taskbar or Dock and is restored when Live mode ends or cannot start.

### 4.2 Live overlay window

The overlay owns only:

- the current concise answer scaffold;
- a minimal listening/generation state;
- transient live actions;
- a compact blocking error when the live runtime cannot continue.

It does not render global navigation, preparation fields, previous answers, the transcript timeline, session history, promotional content, or account controls.

### 4.3 Main shell and Home

The Interview release does not show disabled Sales, Meeting, or Presentation items in primary navigation. Unavailable future modes remain in product plans, not in the repeated customer workflow.

The main shell contains only:

- CallerInterview identity;
- Home/Sessions navigation;
- Settings and account actions.

Home leads with one `Start interview` action and a compact recent-sessions list. It does not use marketing copy, a hero section, decorative mode cards, or unavailable actions. A resumable interrupted session may appear above recent history with one clear resume or review action.

### 4.4 Review

Review uses three tabs instead of showing every artifact at once:

- `Overview`: concise summary, strongest moments, and improvement priorities;
- `Transcript`: searchable conversation timeline;
- `Notes`: user notes, pins, and saved answer scaffolds.

Overview is the default. Coaching details expand in place. Export and delete remain clear commands in the Review toolbar rather than full-width sections.

## 5. Simplified Preparation Experience

Preparation becomes one compact page with one primary action.

### 5.1 Default surface

The first viewport contains:

- a back button and `Interview` title;
- target role and company on one row, both optional;
- interview type as a segmented control;
- one `Add context` disclosure;
- one dominant `Start interview` button;
- a compact readiness line shown only when action is required.

The page must fit without a sticky summary sidebar. It does not show numbered sections, a readiness checklist, empty story-vault messaging, explanatory feature copy, or four simultaneous language selectors.

### 5.2 Add context disclosure

The collapsed `Add context` disclosure contains:

- resume upload;
- job-description upload;
- interview stage;
- saved stories when at least one exists;
- optional custom guidance.

Empty optional collections are omitted. A missing resume, job description, company, or story never creates a warning list.

### 5.3 Language behavior

- Interface language remains a global setting.
- Spoken language defaults to automatic detection.
- Suggestion and review languages default to the interface language.
- A single compact language control appears in Prepare and opens the detailed language settings only when changed.
- The existing separate input, response, and review language values remain in the session model.

### 5.4 Runtime readiness

Preparation must distinguish these states:

- desktop runtime ready;
- speech provider ready;
- AI provider ready;
- microphone/system audio ready;
- capture protection ready.

Healthy dependencies collapse into one quiet `Ready` status. Only blocking or degraded states expand. `Runtime connected` must not be shown when speech or AI credentials are absent. Blocking provider states expose one `Configure` action that opens Settings; provider configuration does not expand inside Prepare.

## 6. Live Overlay Design

### 6.1 Window properties

The default overlay is:

- `560 x 210` logical pixels;
- resizable between `380 x 140` and `760 x 420`;
- centered near the top of the active display on first use;
- clamped within the usable bounds of the current display;
- frameless, transparent, always on top, and draggable from a narrow top strip;
- excluded from ordinary screen capture only after native protection is applied and verified;
- remembered by display, including position, size, and opacity;
- rendered with a neutral charcoal surface at 82% opacity by default, adjustable from 55% to 95%.

The overlay uses an 8-pixel maximum corner radius, a restrained border, and no decorative cards, gradients, or nested panels.

### 6.2 Default content

At rest, the overlay shows only:

- a small listening/generating indicator;
- three to five concise teleprompter bullets;
- a subtle continuation indicator when more detail is available.

The current question is not visible by default. It appears as one truncated line when controls are revealed. Previous answers and transcript text never appear in the default overlay.

Answer generation targets a short scaffold first. Long-form reasoning may continue in the background but cannot replace or move the short scaffold. Streaming tokens update within fixed content bounds.

### 6.3 Controls

Controls are hidden until the overlay receives pointer hover, keyboard focus, or the configured reveal shortcut. The revealed toolbar contains familiar icon controls with tooltips:

- pause/resume listening;
- ask a manual question;
- pin the current answer;
- adjust opacity;
- expand/collapse within the overlay's maximum bounds;
- enable/disable click-through;
- end the session.

Click-through allows pointer input to reach the application beneath the overlay. A global shortcut must always restore interaction, because hover controls cannot be reached while click-through is enabled. Shortcut assignments are configurable in Settings and are not narrated in the live surface.

### 6.4 Expanded state

Expanded state remains an overlay, not the main application. It may show:

- the current question;
- the complete current answer;
- a manual prompt field;
- the current runtime problem, if one exists.

It still does not show navigation, transcript history, preparation, prior-answer cards, or review artifacts.

### 6.5 Accessibility and internationalization

- Text size is adjustable without changing the outer window bounds unexpectedly.
- Keyboard focus order follows the revealed toolbar, answer, and optional manual prompt.
- Status changes use polite announcements; answer streaming does not announce every token.
- Right-to-left languages mirror content alignment and toolbar order where appropriate.
- Bullet layout supports long words and CJK text without horizontal overflow.
- Opacity never drops below a readable configured minimum while text is visible.
- Reduced-motion mode disables nonessential fades and transitions.

### 6.6 Visual system

The redesign uses a neutral grayscale foundation with one restrained green state/accent color. It does not use dominant purple, blue gradients, beige, decorative glow effects, or marketing-style illustration.

- Application headings remain compact and proportional to their work surface.
- Body text uses a readable 14-pixel base with zero letter spacing.
- Cards are limited to repeated session items, dialogs, and genuinely framed tools.
- Page sections remain unframed and separated by spacing or quiet dividers.
- Corner radii do not exceed 8 pixels.
- Familiar commands use Lucide icons with tooltips; text buttons are reserved for clear primary actions.
- Motion is limited to short opacity and state transitions that do not move surrounding content.
- Empty states state the next available action in one sentence and do not explain product features.

## 7. Native Window Architecture

### 7.1 Window creation

The Rust host creates or reuses a window labeled `live-overlay`. It starts hidden and loads an overlay-specific route without the global `AppShell`.

Window creation is idempotent. A second start request cannot create a second overlay or a second live session.

### 7.2 Capture protection

The capture-protection controller becomes window aware. It tracks and verifies every capture-sensitive native window separately.

For the overlay:

1. create the window hidden;
2. apply the platform-native capture-exclusion policy;
3. query and verify the resulting native state;
4. hydrate the active session;
5. show the overlay only after both protection and hydration succeed.

Failure at any step leaves the main window visible, destroys or hides the unusable overlay, prevents audio/transcription/generation from starting, and returns a stable non-sensitive error.

If protection is lost during Live mode, the host immediately hides the overlay, stops active live work through the existing loss-safety path, persists the interrupted session, and restores the main window with the blocking explanation.

### 7.3 Cross-window state

React renderer memory is not shared between Tauri windows. The Rust host and durable session store therefore remain authoritative.

- Sidecar events are emitted to every relevant application window.
- The overlay requests an active-session snapshot before becoming visible.
- Event IDs, sequences, and sidecar generations retain the existing deduplication rules.
- The overlay may send commands through Tauri but never starts, owns, or restarts the sidecar process.
- Main-window hiding waits for an explicit overlay-ready acknowledgement.
- A renderer refresh restores from the durable active session and a sidecar snapshot rather than assuming in-memory Zustand state survived.

## 8. Session Lifecycle

### 8.1 Start transaction

1. The user selects `Start interview`.
2. Prepare validates only blocking runtime, provider, audio, and protection requirements.
3. The host creates a durable pending session.
4. The host creates the hidden overlay and verifies native capture protection.
5. The sidecar receives `session.start` once.
6. The overlay hydrates the active session and acknowledges readiness.
7. The host atomically marks the session active, shows the overlay, and hides the main window.

Any failure compensates in reverse order: stop partial live work, mark the pending session interrupted or remove it according to the existing persistence contract, close the overlay, and keep the main window visible with one actionable error.

### 8.2 End transaction

1. The user ends the session from the overlay.
2. The overlay disables repeated end actions.
3. The sidecar receives `session.stop` once.
4. The host waits for a bounded terminal state and persists completion.
5. The overlay closes or returns to its hidden reusable state.
6. The main window is restored directly on the session Review route.

If sidecar shutdown fails or times out, the session is marked interrupted, the main window is still restored, and Review preserves all durable turns received before failure.

## 9. Component Boundaries

The existing monolithic Live page is split by responsibility:

- `LiveSessionController`: session commands, lifecycle, notes/pins, and terminal-state handling;
- `useLiveConversation`: derives current question, short scaffold, streaming state, and runtime health from the store;
- `LiveOverlayPage`: overlay-only route and accessibility shell;
- `TeleprompterAnswer`: fixed-geometry concise answer rendering;
- `OverlayToolbar`: hover/focus controls and click-through state;
- `OverlayWindowController` in Rust: creation, geometry, visibility, protection, and main-window coordination;
- window-aware `CaptureProtectionController`: authoritative native policy per protected window.

Review continues to own transcript history, prior answers, notes, pins, and coaching. Shared derivation logic may be reused, but Review and Live must not share presentation components that force dashboard geometry into the overlay.

## 10. Error Handling

- Missing AI or speech configuration is reported before session start with a `Configure` action.
- A lost provider during Live mode keeps the existing short answer visible and shows one compact recoverable status.
- Audio loss pauses capture and exposes retry; it does not repeatedly create sessions or provider connections.
- Overlay creation, hydration, or protection failure never hides the main window.
- Window position is clamped after monitor removal, resolution changes, or DPI changes.
- Invalid persisted geometry or opacity falls back to safe defaults.
- Raw provider errors, credentials, native handles, transcripts, and session content never enter user-facing diagnostics.

## 11. Performance Requirements

- Revealing, hiding, dragging, resizing, and changing opacity must not wait on network calls.
- The overlay renders its waiting state immediately after it becomes visible.
- Partial transcript and answer events render incrementally without relaying through the main renderer.
- A new question clears stale generation state without blanking the previous usable scaffold until the replacement begins.
- The short scaffold is prioritized ahead of optional detailed expansion.
- Overlay rendering must remain responsive while transcription and generation are active in the sidecar.

## 12. Testing Strategy

### 12.1 Frontend automated tests

- Prepare renders the compact default path and keeps optional context collapsed.
- Healthy readiness collapses; missing provider/audio/protection states expose only the relevant action.
- The overlay defaults to answer-only content with hidden controls.
- Hover, focus, and reveal state expose the correct controls without resizing content.
- Concise, streaming, empty, degraded, and failed answer states retain stable geometry.
- Click-through state has a reachable shortcut recovery path.
- RTL, long-word, CJK, large-text, reduced-motion, and keyboard navigation cases pass.
- The main application route never renders the old full Live dashboard in production flow.

### 12.2 Rust automated tests

- Overlay creation is idempotent and initially hidden.
- The overlay cannot show until capture protection and hydration are confirmed.
- Start compensation keeps the main window visible after every failure boundary.
- Main hide/show and Review restoration follow the session lifecycle exactly once.
- Capture loss hides the overlay and invokes the existing sidecar safety stop.
- Geometry and opacity persistence reject invalid or off-screen values.
- Direct command invocation cannot bypass overlay protection or create duplicate sessions.

### 12.3 Integration and visual tests

- Exercise Prepare to protected overlay to Review with a deterministic fake sidecar.
- Refresh or crash each renderer during an active session and verify durable restoration.
- Test provider and audio loss while partial answer content is visible.
- Verify browser-preview overlay layouts at minimum, default, and maximum bounds on standard and high-DPI scales.
- Inspect desktop and compact screenshots for overlap, clipping, unstable sizing, and unreadable transparency.

### 12.4 Native acceptance

On each supported operating system:

- start Live mode on every supported multi-monitor/DPI arrangement;
- drag, resize, change opacity, enable click-through, and recover interaction by shortcut;
- share the full display in supported conferencing applications and confirm the overlay receives the platform's protected-content treatment;
- verify the overlay remains locally visible and usable;
- remove or disconnect the active display and confirm safe repositioning;
- force capture-protection failure and confirm the overlay never appears;
- end and interrupt sessions and confirm the main window restores to the correct state.

## 13. Migration

1. Preserve the current session, protocol, storage, and review contracts.
2. Extract live derivation and command logic from `LivePage` behind testable hooks/controllers.
3. Add the hidden overlay window and overlay route.
4. Move current-answer rendering and minimal actions into the overlay.
5. Remove transcript, previous-answer, note-composer, and dashboard chrome from the production Live path.
6. Simplify Prepare after the overlay start transaction is covered.
7. Remove the obsolete full-page Live CSS and components only after Review has independent access to required artifacts.

## 14. Acceptance Criteria

1. Starting an interview produces a protected mini overlay, not the full application window.
2. The main window hides only after overlay protection, session start, and hydration succeed.
3. Default Live content is limited to a listening state and three to five concise answer bullets.
4. The question and controls remain hidden until hover, focus, or reveal shortcut.
5. Click-through, opacity, position, and size controls work without network access and persist safely.
6. The overlay does not render navigation, transcript history, previous answers, preparation, or review content.
7. Prepare presents one compact default path with optional context collapsed and one primary Start action.
8. Missing provider, audio, or protection readiness is explicit and actionable before start.
9. Capture protection remains fail closed for the overlay and is reverified at lifecycle boundaries.
10. Ending or interrupting a session restores the main window on Review without losing durable session data.
11. Automated frontend, Rust, Python, packaging, and native acceptance gates pass before release.
