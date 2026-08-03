# Foundation and Interview Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the production Tauri/React foundation and a persistent end-to-end Interview session shell that reuses the existing Python audio and AI engine through a supervised, typed sidecar.

**Architecture:** A Tauri 2 Rust host owns the window, secure storage, encrypted SQLCipher database, Python sidecar lifecycle, and durable event log. A React/TypeScript client renders Home, Prepare, Live, and Review and communicates only with validated Tauri commands/events. The existing Python engine is moved behind `RuntimeService` and a length-prefixed MessagePack sidecar protocol without removing the working PySide6 interface during migration.

**Tech Stack:** Tauri 2, Rust, React, TypeScript, Vite, pnpm, Zustand, React Router, i18next, Zod, Lucide, Python 3.12, msgpack, pytest, SQLCipher through rusqlite, Tauri Stronghold, Vitest, Testing Library, axe-core, Playwright, and WebdriverIO.

## Global Constraints

- Target Windows and macOS desktop; keep web-compatible React feature code separate from Tauri adapters.
- Use Tauri 2 with React and TypeScript; do not extend the PySide6 UI for new customer-facing workflows.
- Keep the existing PySide6 application runnable until its replacement passes parity checks.
- Use Node.js 22 or newer, pnpm 11 or newer, Python 3.12, and Rust 1.77.2 or newer.
- Use length-prefixed MessagePack frames over inherited standard input/output; do not open a localhost sidecar port.
- The Tauri host owns sidecar streams; React never launches or writes to the process directly.
- Store session data locally in SQLCipher and keep raw audio transient by default.
- Keep provider keys and database key material out of React, SQLite rows, process arguments, and logs.
- Use external locale resources from the first React release; original-language transcript text is canonical.
- Support independent interface, spoken-input, suggestion, and review language settings in session models.
- Use `letter-spacing: 0`; do not scale font size with viewport width.
- Use Lucide icons for familiar icon actions and tooltips for unfamiliar icon-only controls.
- Keep cards at 8px radius or less; do not nest cards or turn full page sections into floating cards.
- Keep Live view geometry stable when transcript, health, or answer content changes.
- Exclude proctoring evasion, process hiding, and undetectability claims.
- Do not stage or modify `_crash_err.txt`, `_crash_out.txt`, `_pw_err.txt`, or `_pw_out.txt`.
- Follow TDD for every behavior and commit after each independently testable task.

## Planned File Structure

```text
protocol/v1/
  envelope.schema.json        # Language-neutral protocol contract
  fixtures/                   # Cross-language conformance messages
ai_assistant/runtime/
  service.py                  # UI-independent engine lifecycle
  factory.py                  # Production dependency construction
ai_assistant/sidecar/
  protocol.py                 # Python protocol models
  framing.py                  # MessagePack frame reader/writer
  transport.py                # Command loop and event output
  adapter.py                  # Runtime events to protocol events
  __main__.py                 # Sidecar executable entry point
desktop/
  src/
    app/                      # Router, providers, global shell
    components/               # Shared accessible UI primitives
    features/home/            # Mode launch surface
    features/prepare/         # Interview session brief
    features/live/            # Live copilot and health surface
    features/review/          # Persisted timeline review
    i18n/                     # Locale initialization and resources
    platform/                 # Browser/Tauri adapter boundary
    stores/                   # Session and runtime state
    styles/                   # Tokens and global layout rules
    test/                     # Shared frontend test utilities
  src-tauri/src/
    commands.rs               # Validated React-facing commands
    protocol.rs               # Rust protocol models and framing
    sidecar.rs                # Sidecar supervision and event fanout
    state.rs                  # Tauri managed state
    storage/                  # SQLCipher key, schema, repositories
  e2e/                        # Browser visual and Tauri integration tests
scripts/
  build-sidecar.ps1           # PyInstaller build and Tauri binary naming
```

---

### Task 1: Bootstrap the Tauri and React Workspace

**Files:**
- Create: `desktop/package.json`
- Create: `desktop/pnpm-lock.yaml`
- Create: `desktop/index.html`
- Create: `desktop/tsconfig.json`
- Create: `desktop/tsconfig.node.json`
- Create: `desktop/vite.config.ts`
- Create: `desktop/vitest.config.ts`
- Create: `desktop/src/main.tsx`
- Create: `desktop/src/app/App.tsx`
- Create: `desktop/src/app/App.test.tsx`
- Create: `desktop/src/test/setup.ts`
- Create: `desktop/src-tauri/Cargo.toml`
- Create: `desktop/src-tauri/build.rs`
- Create: `desktop/src-tauri/tauri.conf.json`
- Create: `desktop/src-tauri/capabilities/default.json`
- Create: `desktop/src-tauri/src/main.rs`
- Create: `desktop/src-tauri/src/lib.rs`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: none.
- Produces: `App(): JSX.Element`, `pnpm --dir desktop test`, `pnpm --dir desktop build`, and `cargo test --manifest-path desktop/src-tauri/Cargo.toml`.

- [ ] **Step 1: Install the missing Rust toolchain and verify all build tools**

Run on Windows:

```powershell
winget install --id Rustlang.Rustup -e
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
rustup default stable
node --version
pnpm --version
py --version
rustc --version
cargo --version
```

Expected: Node 22+, pnpm 11+, Python 3.12, and Rust 1.77.2+.

- [ ] **Step 2: Scaffold the official Tauri React TypeScript template**

Run:

```powershell
pnpm create tauri-app@latest desktop --manager pnpm --template react-ts --tauri-version 2 --yes
pnpm --dir desktop install
```

Retain the generated lockfile. Set the bundle identifier to `com.callerinterview.desktop`, product name to `CallerInterview`, and the initial window to 1280x800 with minimum size 960x640.

- [ ] **Step 3: Write the failing application-shell test**

```tsx
// desktop/src/app/App.test.tsx
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("App", () => {
  it("renders the Interview-first product shell", () => {
    render(<App />);
    expect(screen.getByRole("heading", { name: "CallerInterview" })).toBeVisible();
    expect(screen.getByRole("link", { name: "Interview" })).toHaveAttribute("aria-current", "page");
  });
});
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `pnpm --dir desktop test --run src/app/App.test.tsx`

Expected: FAIL because `App` does not yet export the required shell.

- [ ] **Step 5: Implement the minimal shell and test setup**

```tsx
// desktop/src/app/App.tsx
export function App() {
  return (
    <main>
      <h1>CallerInterview</h1>
      <nav aria-label="Conversation modes">
        <a href="/" aria-current="page">Interview</a>
      </nav>
    </main>
  );
}
```

Configure Vitest with `jsdom`, Testing Library cleanup, and `@testing-library/jest-dom/vitest`. Add `node_modules/`, `desktop/dist/`, `desktop/src-tauri/target/`, and generated sidecar binaries to `.gitignore`.

- [ ] **Step 6: Run foundation checks**

Run:

```powershell
pnpm --dir desktop test --run
pnpm --dir desktop build
cargo test --manifest-path desktop/src-tauri/Cargo.toml
```

Expected: all commands exit 0.

- [ ] **Step 7: Commit**

```powershell
git add .gitignore desktop
git commit -m "build: scaffold Tauri React desktop"
```

### Task 2: Add the Design System, Application Shell, and Localization Base

**Files:**
- Create: `desktop/src/styles/tokens.css`
- Create: `desktop/src/styles/global.css`
- Create: `desktop/src/components/IconButton.tsx`
- Create: `desktop/src/components/HealthIndicator.tsx`
- Create: `desktop/src/app/AppShell.tsx`
- Create: `desktop/src/app/AppShell.test.tsx`
- Create: `desktop/src/app/router.tsx`
- Create: `desktop/src/i18n/index.ts`
- Create: `desktop/src/i18n/locales/en.json`
- Create: `desktop/src/i18n/locales/ar-XB.json`
- Modify: `desktop/src/app/App.tsx`
- Modify: `desktop/src/main.tsx`

**Interfaces:**
- Consumes: `App()` from Task 1.
- Produces: `AppShell`, `IconButton`, `HealthIndicator`, `changeInterfaceLanguage(locale: string): Promise<void>`, and routes `/`, `/prepare/:mode`, `/live/:sessionId`, `/review/:sessionId`.

- [ ] **Step 1: Write failing accessibility and bidirectional-layout tests**

```tsx
it("orders Interview first and labels icon-only controls", () => {
  render(<AppShell />);
  const modes = screen.getAllByRole("link", { name: /Interview|Sales|Meeting|Presentation/ });
  expect(modes[0]).toHaveAccessibleName("Interview");
  expect(screen.getByRole("button", { name: "Open settings" })).toBeVisible();
});

it("sets document direction from the active locale", async () => {
  await changeInterfaceLanguage("ar-XB");
  expect(document.documentElement.dir).toBe("rtl");
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `pnpm --dir desktop test --run src/app/AppShell.test.tsx`

Expected: FAIL because the shell and i18n module do not exist.

- [ ] **Step 3: Implement tokens and shared controls**

Use CSS variables with neutral ink, white/light-gray surfaces, green healthy state, amber degraded state, red error state, and a restrained blue action color. Set radii to `4px`, `6px`, and `8px`; set `letter-spacing: 0`; define stable `--toolbar-height`, `--live-question-height`, and `--status-height` tokens. `IconButton` accepts `label`, `tooltip`, `pressed`, `disabled`, and a Lucide icon component and always exposes an accessible name.

```tsx
export type IconButtonProps = {
  label: string;
  tooltip?: string;
  pressed?: boolean;
  disabled?: boolean;
  icon: LucideIcon;
  onClick?: () => void;
};
```

- [ ] **Step 4: Implement routing and locale resources**

Create English production strings and an exaggerated `ar-XB` pseudo-locale that wraps labels and forces RTL. `changeInterfaceLanguage` must set both `document.documentElement.lang` and `.dir`. The primary nav order is Interview, Sales, Meeting, Presentation; account and settings use Lucide icons with tooltips.

- [ ] **Step 5: Run unit, accessibility, and build checks**

Run:

```powershell
pnpm --dir desktop test --run
pnpm --dir desktop exec tsc --noEmit
pnpm --dir desktop build
```

Expected: all commands exit 0 and no accessibility assertion fails.

- [ ] **Step 6: Commit**

```powershell
git add desktop/src desktop/package.json desktop/pnpm-lock.yaml
git commit -m "feat: add accessible localized app shell"
```

### Task 3: Define Protocol V1 and Cross-Language Fixtures

**Files:**
- Create: `protocol/v1/envelope.schema.json`
- Create: `protocol/v1/fixtures/sidecar-ready.json`
- Create: `protocol/v1/fixtures/session-start.json`
- Create: `protocol/v1/fixtures/transcript-final.json`
- Create: `protocol/v1/fixtures/suggestion-complete.json`
- Create: `ai_assistant/sidecar/__init__.py`
- Create: `ai_assistant/sidecar/protocol.py`
- Create: `ai_assistant/tests/sidecar/test_protocol.py`
- Create: `desktop/src/shared/protocol.ts`
- Create: `desktop/src/shared/protocol.test.ts`
- Create: `desktop/src-tauri/src/protocol.rs`
- Modify: `desktop/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Python dataclasses, Rust serde, TypeScript Zod.
- Produces: `Envelope`, `CommandKind`, `EventKind`, `decodeEnvelope(value)`, and protocol version constant `PROTOCOL_VERSION = 1` in all three languages.

Define the envelope exactly as:

```text
version: u16
id: UUID string
session_id: UUID string or null
sequence: unsigned integer
timestamp_ms: unsigned integer
kind: namespaced string
payload: object
correlation_id: UUID string or null
```

Command kinds are `handshake.request`, `session.start`, `session.stop`, `listening.set`, `you_source.set`, `query.trigger`, `audio.system.set`, `audio.device.set`, `knowledge.ingest`, and `session.snapshot.request`. Event kinds are `sidecar.ready`, `session.state`, `transcript.updated`, `suggestion.chunk`, `suggestion.completed`, `audio.health`, `provider.health`, `knowledge.state`, and `runtime.error`.

- [ ] **Step 1: Write the JSON Schema and four canonical fixtures**

The `session.start` payload must contain:

```json
{
  "mode": "interview",
  "input_language": "auto",
  "response_language": "en",
  "review_language": "en",
  "you_source": "mic",
  "brief_id": "018f0000-0000-7000-8000-000000000010"
}
```

The final transcript fixture must include `turn_id`, `text`, `is_final`, `speech_final`, `speaker_role`, `source`, `language`, `confidence`, `started_at_ms`, and `ended_at_ms`.

- [ ] **Step 2: Write failing Python, Rust, and TypeScript fixture tests**

```python
def test_session_start_fixture_round_trips():
    raw = json.loads(FIXTURE.read_text(encoding="utf-8"))
    envelope = Envelope.from_dict(raw)
    assert envelope.version == 1
    assert envelope.kind == CommandKind.SESSION_START
    assert envelope.to_dict() == raw
```

```rust
#[test]
fn transcript_fixture_round_trips() {
    let raw = include_str!("../../../protocol/v1/fixtures/transcript-final.json");
    let value: Envelope = serde_json::from_str(raw).unwrap();
    assert_eq!(value.version, PROTOCOL_VERSION);
    assert_eq!(serde_json::to_value(value).unwrap(), serde_json::from_str::<serde_json::Value>(raw).unwrap());
}
```

```ts
it("rejects an unsupported protocol version", () => {
  expect(() => decodeEnvelope({ ...readyFixture, version: 2 })).toThrow(/protocol version/i);
});
```

- [ ] **Step 3: Run the contract tests to verify they fail**

Run:

```powershell
py -m pytest ai_assistant/tests/sidecar/test_protocol.py -q
pnpm --dir desktop test --run src/shared/protocol.test.ts
cargo test --manifest-path desktop/src-tauri/Cargo.toml protocol
```

Expected: FAIL because protocol models are absent.

- [ ] **Step 4: Implement strict protocol models**

Use frozen Python dataclasses, `serde` Rust structs/enums, and discriminated Zod schemas in TypeScript. Validate UUID syntax, nonnegative sequence/timestamps, known major version, namespaced kind, and object payload. Preserve unknown payload fields for compatible minor extensions.

- [ ] **Step 5: Run all protocol tests**

Run the three commands from Step 3.

Expected: PASS in Python, TypeScript, and Rust against the same fixtures.

- [ ] **Step 6: Commit**

```powershell
git add protocol ai_assistant/sidecar ai_assistant/tests/sidecar desktop/src/shared desktop/src-tauri/src
git commit -m "feat: define versioned sidecar protocol"
```

### Task 4: Implement MessagePack Framing and the Python Sidecar Command Loop

**Files:**
- Create: `ai_assistant/sidecar/framing.py`
- Create: `ai_assistant/sidecar/transport.py`
- Create: `ai_assistant/sidecar/__main__.py`
- Create: `ai_assistant/tests/sidecar/test_framing.py`
- Create: `ai_assistant/tests/sidecar/test_transport.py`
- Modify: `requirements.txt`

**Interfaces:**
- Consumes: Python `Envelope` from Task 3.
- Produces: `encode_frame(envelope) -> bytes`, `read_frame(stream) -> Envelope | None`, `write_frame(stream, envelope) -> None`, and `SidecarTransport.run(handler) -> None`.

- [ ] **Step 1: Write failing frame boundary and corruption tests**

```python
def test_two_frames_decode_without_bleeding():
    stream = BytesIO(encode_frame(first) + encode_frame(second))
    assert read_frame(stream) == first
    assert read_frame(stream) == second
    assert read_frame(stream) is None

def test_oversized_frame_is_rejected():
    stream = BytesIO(struct.pack(">I", 16_777_217))
    with pytest.raises(FrameError, match="maximum"):
        read_frame(stream)
```

Also test truncated headers, truncated payloads, invalid MessagePack, and that diagnostics go to stderr rather than stdout.

- [ ] **Step 2: Run tests to verify they fail**

Run: `py -m pytest ai_assistant/tests/sidecar/test_framing.py ai_assistant/tests/sidecar/test_transport.py -q`

Expected: FAIL because framing and transport do not exist.

- [ ] **Step 3: Implement bounded framing**

Use a four-byte big-endian unsigned length prefix and a 16 MiB maximum payload. Encode with `msgpack.packb(..., use_bin_type=True)` and decode with `msgpack.unpackb(..., raw=False, strict_map_key=True)`. Never print to stdout; configure logs on stderr.

- [ ] **Step 4: Implement the command loop and healthcheck**

`SidecarTransport.run` reads commands on a worker thread through `asyncio.to_thread`, validates each envelope, dispatches to an async handler, and serializes writes through one lock. `py -m ai_assistant.sidecar --healthcheck` writes `{"status":"ok","protocol_version":1}` as text and exits before binary transport starts.

- [ ] **Step 5: Run sidecar tests and the healthcheck**

Run:

```powershell
py -m pytest ai_assistant/tests/sidecar -q
py -m ai_assistant.sidecar --healthcheck
```

Expected: tests pass and healthcheck prints one JSON object.

- [ ] **Step 6: Commit**

```powershell
git add requirements.txt ai_assistant/sidecar ai_assistant/tests/sidecar
git commit -m "feat: add framed Python sidecar transport"
```

### Task 5: Extract a UI-Independent Python Runtime Service

**Files:**
- Create: `ai_assistant/runtime/__init__.py`
- Create: `ai_assistant/runtime/models.py`
- Create: `ai_assistant/runtime/factory.py`
- Create: `ai_assistant/runtime/service.py`
- Create: `ai_assistant/tests/runtime/test_service.py`
- Modify: `ai_assistant/main.py`
- Modify: `ai_assistant/config.py`

**Interfaces:**
- Consumes: `EventBus`, `DualMicCapture`, `DeepgramTranscriber`, `AnthropicLLM`, `PromptBuilder`, `RAGRetriever`, and `Orchestrator`.
- Produces: `RuntimeService.start_session(config: SessionConfig)`, `stop_session()`, `set_listening(enabled: bool)`, `set_you_source(source: Literal["mic", "system"])`, `set_system_audio(enabled: bool)`, `trigger_query(text: str, answer_format: str)`, `ingest_documents(paths: list[str])`, and `snapshot() -> RuntimeSnapshot`.

- [ ] **Step 1: Write fakes and failing lifecycle tests**

```python
@pytest.mark.asyncio
async def test_runtime_starts_without_ui_and_emits_state():
    deps = FakeRuntimeDependencies()
    service = RuntimeService(deps)
    await service.start_session(SessionConfig.fixture())
    assert service.snapshot().state == "listening"
    assert deps.capture.started is True
    assert deps.transcriber.started is True

@pytest.mark.asyncio
async def test_missing_credentials_returns_configuration_error():
    service = build_runtime(Config(deepgram_api_key="", llm_api_key=""))
    with pytest.raises(RuntimeConfigurationError) as exc:
        await service.start_session(SessionConfig.fixture())
    assert exc.value.missing == ["deepgram_api_key", "llm_api_key"]
```

Also test idempotent stop, device switching, system-audio switching, language values reaching transcription configuration, and no PySide6 import from `ai_assistant.runtime`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `py -m pytest ai_assistant/tests/runtime/test_service.py -q`

Expected: FAIL because runtime modules do not exist.

- [ ] **Step 3: Implement runtime models and dependency factory**

```python
@dataclass(frozen=True)
class SessionConfig:
    session_id: str
    mode: str
    input_language: str
    response_language: str
    review_language: str
    you_source: Literal["mic", "system"]
    brief_id: str
```

`build_runtime(config)` constructs the current production audio, transcription, LLM, prompt, RAG, and orchestrator objects. Construction itself must not connect to providers or start audio.

- [ ] **Step 4: Implement `RuntimeService` and adapt the legacy entry point**

Move lifecycle wiring from `async_main` into the service. Keep Qt signal translation in `main.py`; it subscribes to `RuntimeService.event_bus` and invokes the same public service methods used by the sidecar. Remove direct access to `orchestrator._transcript_buffer` by adding `RuntimeService.full_transcript()`.

- [ ] **Step 5: Run focused and legacy Python tests**

Run:

```powershell
py -m pytest ai_assistant/tests/runtime/test_service.py -q
py -m pytest ai_assistant/tests -q
```

Expected: all existing and new tests pass.

- [ ] **Step 6: Commit**

```powershell
git add ai_assistant/runtime ai_assistant/tests/runtime ai_assistant/main.py ai_assistant/config.py
git commit -m "refactor: extract UI-independent assistant runtime"
```

### Task 6: Bridge Runtime Commands and Events to Protocol V1

**Files:**
- Create: `ai_assistant/sidecar/adapter.py`
- Create: `ai_assistant/tests/sidecar/test_adapter.py`
- Modify: `ai_assistant/sidecar/__main__.py`
- Modify: `ai_assistant/core/events.py`

**Interfaces:**
- Consumes: `RuntimeService` from Task 5 and `SidecarTransport` from Task 4.
- Produces: `RuntimeProtocolAdapter.handle(command: Envelope) -> None` and protocol events with monotonic per-process sequence numbers.

- [ ] **Step 1: Write failing command and event mapping tests**

```python
@pytest.mark.asyncio
async def test_start_command_maps_all_language_fields():
    runtime = FakeRuntimeService()
    output = CollectingOutput()
    adapter = RuntimeProtocolAdapter(runtime, output.write)
    await adapter.handle(load_fixture("session-start.json"))
    assert runtime.started_with.input_language == "auto"
    assert runtime.started_with.response_language == "en"
    assert output.last.kind == EventKind.SESSION_STATE

@pytest.mark.asyncio
async def test_final_transcript_keeps_original_language():
    await adapter.on_transcript(TranscriptEvent(text="Bonjour", language="fr", is_final=True))
    assert output.last.payload["text"] == "Bonjour"
    assert output.last.payload["language"] == "fr"
```

Also test correlation IDs, unknown commands, runtime exceptions, response chunks, response completion, and increasing sequence values.

- [ ] **Step 2: Run tests to verify they fail**

Run: `py -m pytest ai_assistant/tests/sidecar/test_adapter.py -q`

Expected: FAIL because the adapter is absent and current transcript events lack language timing fields.

- [ ] **Step 3: Extend internal events without breaking existing callers**

Add optional `language`, `started_at_ms`, and `ended_at_ms` fields to `TranscriptEvent` with backwards-compatible defaults. Keep `text`, `is_final`, `speech_final`, `speaker`, `confidence`, `timestamp`, and `source` unchanged.

- [ ] **Step 4: Implement all command/event mappings**

Reject commands with the wrong major version before touching runtime state. Return `runtime.error` with `code`, `message`, `recoverable`, `source`, and `correlation_id`; do not serialize tracebacks, keys, prompts, or transcript history.

- [ ] **Step 5: Run sidecar and full Python tests**

Run: `py -m pytest ai_assistant/tests -q`

Expected: all tests pass.

- [ ] **Step 6: Commit**

```powershell
git add ai_assistant/sidecar ai_assistant/core/events.py ai_assistant/tests/sidecar
git commit -m "feat: bridge runtime through sidecar protocol"
```

### Task 7: Package the Python Sidecar as a Tauri External Binary

**Files:**
- Create: `sidecar/callerinterview-sidecar.spec`
- Create: `scripts/build-sidecar.ps1`
- Create: `scripts/test-sidecar-package.ps1`
- Create: `requirements-dev.txt`
- Modify: `desktop/src-tauri/tauri.conf.json`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: `py -m ai_assistant.sidecar --healthcheck` from Task 4.
- Produces: `desktop/src-tauri/binaries/callerinterview-sidecar-$TARGET_TRIPLE[.exe]` and repeatable build/smoke commands.

- [ ] **Step 1: Write the failing package smoke script**

```powershell
$binary = Get-ChildItem "$PSScriptRoot\..\desktop\src-tauri\binaries\callerinterview-sidecar-*" -File | Select-Object -First 1
if (-not $binary) { throw "sidecar binary missing" }
$result = & $binary.FullName --healthcheck | ConvertFrom-Json
if ($result.status -ne "ok" -or $result.protocol_version -ne 1) { throw "sidecar healthcheck failed" }
```

- [ ] **Step 2: Run the smoke script to verify it fails**

Run: `powershell -ExecutionPolicy Bypass -File scripts/test-sidecar-package.ps1`

Expected: FAIL with `sidecar binary missing`.

- [ ] **Step 3: Implement the PyInstaller build**

Pin PyInstaller in `requirements-dev.txt`. The spec entry point is `ai_assistant/sidecar/__main__.py`; include the sentence-transformer and FAISS runtime assets required by the production factory. `build-sidecar.ps1` derives the Rust host target with `rustc -vV`, appends `.exe` only on Windows, and copies the one-file output to Tauri's required target-triple filename.

- [ ] **Step 4: Declare the external binary and restrict shell scope**

Set `bundle.externalBin` to `binaries/callerinterview-sidecar`. In `capabilities/default.json`, allow only that sidecar and stdin writes; do not enable arbitrary `shell:allow-execute` or free-form command arguments.

- [ ] **Step 5: Build and smoke-test the package**

Run:

```powershell
py -m pip install -r requirements-dev.txt
powershell -ExecutionPolicy Bypass -File scripts/build-sidecar.ps1
powershell -ExecutionPolicy Bypass -File scripts/test-sidecar-package.ps1
```

Expected: healthcheck returns protocol version 1.

- [ ] **Step 6: Commit**

```powershell
git add .gitignore requirements-dev.txt sidecar scripts desktop/src-tauri/tauri.conf.json desktop/src-tauri/capabilities/default.json
git commit -m "build: package Python engine as Tauri sidecar"
```

### Task 8: Supervise the Sidecar from Rust and Expose Validated Tauri Commands

**Files:**
- Create: `desktop/src-tauri/src/sidecar.rs`
- Create: `desktop/src-tauri/src/commands.rs`
- Create: `desktop/src-tauri/src/state.rs`
- Modify: `desktop/src-tauri/src/protocol.rs`
- Modify: `desktop/src-tauri/src/lib.rs`
- Modify: `desktop/src-tauri/Cargo.toml`

**Interfaces:**
- Consumes: packaged sidecar and Rust `Envelope`.
- Produces: Tauri commands `sidecar_status() -> SidecarStatus`, `send_sidecar_command(command: Envelope) -> Result<(), CommandError>`, `restart_sidecar() -> Result<SidecarStatus, CommandError>`, and event channel `sidecar://event`.

- [ ] **Step 1: Write failing framing and supervisor-state tests**

```rust
#[test]
fn decoder_accepts_split_frame_chunks() {
    let bytes = encode_frame(&fixture_envelope()).unwrap();
    let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
    assert!(decoder.push(&bytes[..3]).unwrap().is_empty());
    assert_eq!(decoder.push(&bytes[3..]).unwrap(), vec![fixture_envelope()]);
}

#[tokio::test]
async fn supervisor_rejects_command_until_handshake() {
    let supervisor = SidecarSupervisor::with_port(FakeSidecarPort::default());
    let error = supervisor.send(fixture_command()).await.unwrap_err();
    assert_eq!(error.code(), "sidecar_not_ready");
}
```

Also test oversized frames, malformed output, version mismatch, single restart after unexpected exit, and no restart after user-requested shutdown.

- [ ] **Step 2: Run Rust tests to verify they fail**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml sidecar`

Expected: FAIL because supervisor types do not exist.

- [ ] **Step 3: Implement the port abstraction and supervisor**

```rust
#[async_trait]
pub trait SidecarPort: Send + Sync {
    async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError>;
    async fn kill(&self) -> Result<(), SidecarError>;
}
```

Launch through Tauri's sidecar API, read stdout only as framed MessagePack, route stderr into redacted diagnostics, require `sidecar.ready` before commands, and emit validated envelopes to React. Use one automatic restart with bounded delay; further failure enters `failed` state and requires explicit retry.

- [ ] **Step 4: Register commands and least-privilege capabilities**

React may invoke only the three named commands. Validate protocol version, allowed command kind, payload schema, and session ID before writing. Do not expose raw shell or arbitrary sidecar arguments to the web view.

- [ ] **Step 5: Run Rust and Tauri compile checks**

Run:

```powershell
cargo test --manifest-path desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
pnpm --dir desktop tauri build --debug --no-bundle
```

Expected: tests pass, clippy has no warnings, and the debug binary builds.

- [ ] **Step 6: Commit**

```powershell
git add desktop/src-tauri
git commit -m "feat: supervise sidecar from Tauri host"
```

### Task 9: Add SQLCipher Persistence and Secret Management

**Files:**
- Create: `desktop/src-tauri/src/storage/mod.rs`
- Create: `desktop/src-tauri/src/storage/key_manager.rs`
- Create: `desktop/src-tauri/src/storage/migrations.rs`
- Create: `desktop/src-tauri/src/storage/models.rs`
- Create: `desktop/src-tauri/src/storage/repository.rs`
- Modify: `desktop/src-tauri/src/commands.rs`
- Modify: `desktop/src-tauri/src/state.rs`
- Modify: `desktop/src-tauri/src/lib.rs`
- Modify: `desktop/src-tauri/Cargo.toml`

**Interfaces:**
- Consumes: validated sidecar events.
- Produces: `SessionRepository.create_session`, `save_session_brief`, `append_event`, `complete_session`, `list_sessions`, `get_session`, `get_timeline`, `restore_active_session`, and `delete_session`; `KeyManager.database_key() -> Zeroizing<Vec<u8>>`; matching validated Tauri commands for each user-facing repository operation.

- [ ] **Step 1: Write failing encryption, migration, and idempotency tests**

```rust
#[test]
fn database_cannot_be_opened_without_its_key() {
    let path = temp_db_path();
    create_repository(&path, key_a()).unwrap();
    assert!(SessionRepository::open(&path, key_b()).is_err());
}

#[test]
fn duplicate_event_id_is_idempotent() {
    let repo = memory_repository();
    repo.append_event(&fixture_event()).unwrap();
    repo.append_event(&fixture_event()).unwrap();
    assert_eq!(repo.get_timeline(SESSION_ID).unwrap().len(), 1);
}
```

Also test migration from an empty database, strict event sequence ordering, incomplete-session restore, deletion cascade, and no transcript body in usage rows.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml storage`

Expected: FAIL because storage modules do not exist.

- [ ] **Step 3: Implement key management**

Use an injectable `SecretStore` trait. Production stores a generated Stronghold unlock secret in the OS credential manager and stores the random 256-bit SQLCipher key inside the Stronghold vault. Tests use `MemorySecretStore`. Wrap key bytes in `zeroize::Zeroizing` and never format them through `Debug` or logs.

- [ ] **Step 4: Implement the schema and repository**

Use `rusqlite` with `bundled-sqlcipher-vendored-openssl`. Schema version 1 includes `sessions`, `session_briefs`, `timeline_events`, and `settings`. `timeline_events` uses event ID as primary key and `(session_id, sequence)` as a unique ordering constraint. Execute `PRAGMA key` before any schema query and verify `PRAGMA cipher_version` is non-empty.

- [ ] **Step 5: Expose validated repository commands and persist sidecar events transactionally**

Register `create_session`, `save_session_brief`, `complete_session`, `list_sessions`, `get_session`, `get_timeline`, `restore_active_session`, and `delete_session` in `commands.rs`. Each command validates UUIDs, language tags, bounded text lengths, and workspace ownership before calling the repository. The Rust supervisor persists `session.state`, final `transcript.updated`, `suggestion.completed`, notes, and explicit pins before emitting them to React. Partial transcript and suggestion chunks remain memory-only. Database failure emits a recoverable storage health error and keeps the live in-memory session usable.

- [ ] **Step 6: Run storage and Rust checks**

Run:

```powershell
cargo test --manifest-path desktop/src-tauri/Cargo.toml storage
cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
```

Expected: all tests pass and the wrong-key test proves encryption is active.

- [ ] **Step 7: Commit**

```powershell
git add desktop/src-tauri
git commit -m "feat: add encrypted local session store"
```

### Task 10: Add the React Platform Bridge and Deterministic Session Store

**Files:**
- Create: `desktop/src/platform/desktop.ts`
- Create: `desktop/src/platform/browser.ts`
- Create: `desktop/src/platform/index.ts`
- Create: `desktop/src/platform/types.ts`
- Create: `desktop/src/stores/sessionStore.ts`
- Create: `desktop/src/stores/sessionStore.test.ts`
- Create: `desktop/src/app/RuntimeProvider.tsx`
- Create: `desktop/src/app/RuntimeProvider.test.tsx`

**Interfaces:**
- Consumes: Tauri commands and `sidecar://event` from Task 8.
- Produces: `PlatformApi`, `RuntimeProvider`, `useRuntime()`, and Zustand actions `applyEnvelope`, `beginSession`, `endSession`, `restoreSession`, and `clearTransientState`.

- [ ] **Step 1: Write failing ordering and recovery tests**

```ts
it("ignores duplicate and stale sidecar events", () => {
  const store = createSessionStore();
  store.getState().applyEnvelope(transcriptEnvelope({ sequence: 4, text: "First" }));
  store.getState().applyEnvelope(transcriptEnvelope({ sequence: 4, text: "Duplicate" }));
  store.getState().applyEnvelope(transcriptEnvelope({ sequence: 3, text: "Stale" }));
  expect(store.getState().turns.map((turn) => turn.text)).toEqual(["First"]);
});

it("keeps the completed answer when a new question arrives", () => {
  const store = createSessionStore();
  store.getState().applyEnvelope(completedSuggestion("answer-1"));
  store.getState().applyEnvelope(finalQuestion("question-2"));
  expect(store.getState().suggestionsByTurn["question-1"].text).toBe("answer-1");
});
```

Also test partial replacement, final transcript commitment, health updates, language changes, and clearing only transient state on sidecar restart.

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --dir desktop test --run src/stores/sessionStore.test.ts src/app/RuntimeProvider.test.tsx`

Expected: FAIL because bridge and store do not exist.

- [ ] **Step 3: Implement the adapter boundary**

```ts
export interface PlatformApi {
  sidecarStatus(): Promise<SidecarStatus>;
  send(command: Envelope): Promise<void>;
  restartSidecar(): Promise<SidecarStatus>;
  subscribe(listener: (event: Envelope) => void): Promise<() => void>;
  createSession(input: CreateSessionInput): Promise<SessionRecord>;
  saveSessionBrief(input: SaveSessionBriefInput): Promise<void>;
  completeSession(sessionId: string, status: "completed" | "interrupted"): Promise<void>;
  listSessions(limit: number): Promise<SessionRecord[]>;
  getSession(sessionId: string): Promise<SessionRecord>;
  getTimeline(sessionId: string): Promise<Envelope[]>;
  restoreActiveSession(): Promise<SessionRecord | null>;
  deleteSession(sessionId: string): Promise<void>;
}
```

`desktop.ts` uses Tauri `invoke` and `listen`; `browser.ts` is an in-memory deterministic adapter for tests, Storybook-free visual development, and Playwright.

- [ ] **Step 4: Implement the store and provider**

Validate every incoming envelope with `decodeEnvelope`. Index final turns and completed suggestions by stable IDs, retain prior turns, track last sequence, and model health separately for sidecar, microphone, system audio, speech provider, and model provider.

- [ ] **Step 5: Run frontend checks**

Run:

```powershell
pnpm --dir desktop test --run
pnpm --dir desktop exec tsc --noEmit
pnpm --dir desktop build
```

Expected: all commands pass.

- [ ] **Step 6: Commit**

```powershell
git add desktop/src/platform desktop/src/stores desktop/src/app
git commit -m "feat: connect React to persistent runtime state"
```

### Task 11: Build Interview Home and Prepare Workflows

**Files:**
- Create: `desktop/src/features/home/modes.ts`
- Create: `desktop/src/features/home/HomePage.tsx`
- Create: `desktop/src/features/home/HomePage.test.tsx`
- Create: `desktop/src/features/prepare/InterviewPreparePage.tsx`
- Create: `desktop/src/features/prepare/InterviewPreparePage.test.tsx`
- Create: `desktop/src/features/prepare/briefSchema.ts`
- Create: `desktop/src/features/prepare/components/LanguageControls.tsx`
- Create: `desktop/src/features/prepare/components/ReadinessSummary.tsx`
- Modify: `desktop/src/app/router.tsx`
- Modify: `desktop/src/i18n/locales/en.json`
- Modify: `desktop/src/i18n/locales/ar-XB.json`

**Interfaces:**
- Consumes: `PlatformApi.createSession` and route definitions.
- Produces: `ModeDefinition`, `InterviewBrief`, `HomePage`, `InterviewPreparePage`, and a valid `session.start` command.

- [ ] **Step 1: Write failing Home and Prepare behavior tests**

```tsx
it("presents all professional modes with Interview first", () => {
  renderApp("/");
  const actions = screen.getAllByRole("button", { name: /Start/ });
  expect(actions.map((item) => item.textContent)).toEqual([
    "Start Interview", "Start Sales Call", "Start Meeting", "Start Presentation",
  ]);
});

it("creates an Interview session with independent languages", async () => {
  const platform = fakePlatform();
  renderApp("/prepare/interview", { platform });
  await userEvent.selectOptions(screen.getByLabelText("Spoken language"), "auto");
  await userEvent.selectOptions(screen.getByLabelText("Suggestion language"), "ur");
  await userEvent.click(screen.getByRole("button", { name: "Start interview" }));
  expect(platform.lastSession?.responseLanguage).toBe("ur");
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --dir desktop test --run src/features/home src/features/prepare`

Expected: FAIL because pages and models do not exist.

- [ ] **Step 3: Implement the mode registry and Home**

Each mode has `id`, `labelKey`, `startLabelKey`, `icon`, `prepareRoute`, and `releaseState`. Interview is `available`; the other three use `planned` and open an informative mode-detail sheet rather than dead buttons. All four visible actions are buttons because they execute application commands; only the available Interview action navigates after activation. The page also renders recent sessions from `PlatformApi.listSessions`; empty state is concise and actionable.

- [ ] **Step 4: Implement Interview Prepare**

Use an unframed responsive layout with sections for role/company/stage, resume and job-description metadata, story-vault selection, interview type, answer style, and four language values. Foundation stores document references and brief metadata through `PlatformApi.saveSessionBrief`; document ingestion content arrives in the Interview capability plan. Readiness identifies missing optional context without blocking Quick Start.

- [ ] **Step 5: Add schema and navigation behavior**

Validate with Zod. On submit, create the session, persist its brief, send `session.start`, and navigate to `/live/:sessionId`. On runtime configuration error, remain on Prepare and focus the actionable error summary.

- [ ] **Step 6: Run tests, type checks, and accessibility checks**

Run:

```powershell
pnpm --dir desktop test --run src/features/home src/features/prepare
pnpm --dir desktop exec tsc --noEmit
pnpm --dir desktop build
```

Expected: all commands pass.

- [ ] **Step 7: Commit**

```powershell
git add desktop/src/features desktop/src/app/router.tsx desktop/src/i18n
git commit -m "feat: add Interview preparation workflow"
```

### Task 12: Build the Stable Live Copilot Surface

**Files:**
- Create: `desktop/src/features/live/LivePage.tsx`
- Create: `desktop/src/features/live/LivePage.test.tsx`
- Create: `desktop/src/features/live/live.css`
- Create: `desktop/src/features/live/components/CurrentQuestion.tsx`
- Create: `desktop/src/features/live/components/AnswerScaffold.tsx`
- Create: `desktop/src/features/live/components/TranscriptTimeline.tsx`
- Create: `desktop/src/features/live/components/RuntimeHealth.tsx`
- Create: `desktop/src/features/live/components/LiveToolbar.tsx`
- Modify: `desktop/src/app/router.tsx`
- Modify: `desktop/src/i18n/locales/en.json`
- Modify: `desktop/src/i18n/locales/ar-XB.json`

**Interfaces:**
- Consumes: `useRuntime`, Interview session state, protocol commands.
- Produces: live listening controls, current question, answer scaffold, expandable detail, transcript timeline, notes/pins entry points, and visible runtime health.

- [ ] **Step 1: Write failing state and layout tests**

```tsx
it("retains the previous answer after the next question", () => {
  const runtime = runtimeWithTwoTurns();
  render(<LivePage />, { wrapper: runtime.wrapper });
  expect(screen.getByText("Previous answers")).toBeVisible();
  expect(screen.getByText("The retained first answer")).toBeVisible();
  expect(screen.getByRole("heading", { name: "Second question" })).toBeVisible();
});

it("shows a named health state for every live dependency", () => {
  renderLive();
  for (const label of ["Microphone", "System audio", "Transcription", "AI provider"]) {
    expect(screen.getByLabelText(new RegExp(label))).toBeVisible();
  }
});
```

Also test pause/resume, manual prompt, answer-format selection, language switching, provider error, keyboard focus order, and `aria-live` behavior that does not announce every streaming token.

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --dir desktop test --run src/features/live`

Expected: FAIL because Live components do not exist.

- [ ] **Step 3: Implement stable live layout**

Use three fixed tracks: status/toolbar, question plus answer, and collapsible transcript timeline. The question region has a minimum and maximum height; the answer scrolls internally; transcript updates never resize the answer region. The default answer view renders up to three concise scaffold points, then an explicit Expand action for full detail.

- [ ] **Step 4: Implement complete controls and health behavior**

Use icon buttons for pause/resume, pin, copy, and end; segmented controls for answer format; select menus for language; a text command for manual prompts; and visible named statuses for audio/providers. Errors state what remains usable and expose Retry only when the action is valid.

- [ ] **Step 5: Implement end-session behavior**

End sends `session.stop`, waits for durable `session.state=completed`, then navigates to `/review/:sessionId`. If the sidecar is unavailable, mark the local session interrupted, persist it, and still allow Review.

- [ ] **Step 6: Run tests and frontend checks**

Run:

```powershell
pnpm --dir desktop test --run src/features/live
pnpm --dir desktop exec tsc --noEmit
pnpm --dir desktop build
```

Expected: all commands pass.

- [ ] **Step 7: Commit**

```powershell
git add desktop/src/features/live desktop/src/app/router.tsx desktop/src/i18n
git commit -m "feat: add stable live Interview copilot"
```

### Task 13: Add Review, Crash Restore, End-to-End QA, and Packaging Gates

**Files:**
- Create: `desktop/src/features/review/ReviewPage.tsx`
- Create: `desktop/src/features/review/ReviewPage.test.tsx`
- Create: `desktop/src/features/review/reviewModels.ts`
- Create: `desktop/e2e/browser/interview-flow.spec.ts`
- Create: `desktop/e2e/browser/responsive.spec.ts`
- Create: `desktop/e2e/tauri/sidecar-recovery.e2e.ts`
- Create: `desktop/playwright.config.ts`
- Create: `desktop/wdio.conf.ts`
- Create: `scripts/verify-foundation.ps1`
- Create: `.github/workflows/desktop-foundation.yml`
- Modify: `desktop/package.json`
- Modify: `desktop/src/app/router.tsx`
- Modify: `desktop/src/i18n/locales/en.json`
- Modify: `desktop/src/i18n/locales/ar-XB.json`

**Interfaces:**
- Consumes: persisted `SessionRecord` and timeline events.
- Produces: `ReviewPage`, startup active-session restore, browser visual suite, Tauri sidecar recovery suite, CI, and a signed-build-ready debug bundle.

- [ ] **Step 1: Write failing review and restore tests**

```tsx
it("separates original transcript from generated suggestions", async () => {
  renderReview(persistedFixture());
  expect(await screen.findByRole("region", { name: "Original transcript" })).toBeVisible();
  expect(screen.getByRole("region", { name: "AI suggestions" })).toBeVisible();
});

it("offers to resume an interrupted session on startup", async () => {
  renderApp("/", { platform: platformWithInterruptedSession() });
  expect(await screen.findByRole("button", { name: "Resume interview" })).toBeVisible();
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --dir desktop test --run src/features/review`

Expected: FAIL because Review and restore behavior do not exist.

- [ ] **Step 3: Implement Review and restore**

Review renders chronological original-language transcript turns, completed suggestions, notes, pins, runtime interruptions, and session metadata. It offers translated/review-language views only when corresponding derived text exists; it never labels generated translation as original. Startup calls `PlatformApi.restoreActiveSession()` and offers Resume or End and review.

- [ ] **Step 4: Write browser and Tauri end-to-end tests**

The browser test uses `browser.ts` and completes Home -> Prepare -> Live -> Review with synthetic protocol events. Responsive tests capture 960x640, 1280x800, and 1600x1000 plus pseudo-RTL and 200 percent text. The WebdriverIO test launches the Tauri binary, kills the fake sidecar once, verifies one restart, confirms committed turns survive, and checks the second failure becomes a visible retry state.

- [ ] **Step 5: Implement the verification script and CI**

`scripts/verify-foundation.ps1` runs, in order:

```powershell
py -m pytest ai_assistant/tests -q
pnpm --dir desktop test --run
pnpm --dir desktop exec tsc --noEmit
pnpm --dir desktop build
cargo test --manifest-path desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
pnpm --dir desktop exec playwright test
pnpm --dir desktop wdio run wdio.conf.ts
pnpm --dir desktop tauri build --debug
```

CI runs Python/frontend/Rust tests on every pull request and Windows/macOS packaging plus Tauri WebdriverIO on protected-branch pushes. Upload screenshots and test reports on failure, never environment files or app data.

- [ ] **Step 6: Run the complete verification suite**

Run: `powershell -ExecutionPolicy Bypass -File scripts/verify-foundation.ps1`

Expected: every command exits 0; screenshots show no overlap or layout shift; the Tauri app restores the interrupted session.

- [ ] **Step 7: Inspect diagnostics for sensitive content**

Run:

```powershell
rg -n "DEEPGRAM_API_KEY|DEEPSEEK_API_KEY|sk-[A-Za-z0-9]|The retained first answer" desktop/test-results desktop/src-tauri/target -g '*.log' -g '*.txt'
```

Expected: no matches.

- [ ] **Step 8: Commit**

```powershell
git add desktop scripts/verify-foundation.ps1 .github/workflows/desktop-foundation.yml
git commit -m "test: gate persistent Interview foundation"
```

## Completion Criteria

This plan is complete when:

- the existing PySide6 application still passes its full Python suite;
- the Tauri app builds on Windows and macOS CI;
- React can start a prepared Interview session through the typed sidecar;
- final transcript and completed suggestions persist in encrypted SQLCipher storage;
- a crash or one sidecar restart does not lose committed session data;
- Home, Prepare, Live, and Review pass keyboard, pseudo-RTL, text-size, and responsive visual tests;
- independent input, suggestion, interface, and review languages survive create, live, persistence, restore, and review;
- no raw audio or secret appears in storage or default logs;
- all commands in `scripts/verify-foundation.ps1` pass.

## Follow-On Plans

After this foundation passes, create separate executable plans in this order:

1. `interview-context-and-story-vault`: resume/JD/company ingestion, multilingual retrieval, source provenance, and story-vault authoring.
2. `interview-realtime-intelligence`: question/intent detection, fast scaffold and detail split, provider routing, failover, and latency telemetry.
3. `interview-coding-and-specialties`: screenshot OCR, coding, system design, case, phone, panel, and one-way interview workflows.
4. `interview-review-and-practice`: coaching rubrics, improved answers, practice queues, exports, and mock interviews.
5. `cloud-account-sync-billing`: identity, passkeys, encrypted sync, usage, Stripe, BYOK, device handoff, and web workspace.
6. `professional-mode-packs`: Sales, Meeting, and Presentation schemas, live actions, reviews, and exports.
