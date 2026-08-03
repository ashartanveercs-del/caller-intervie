# Task 5 Report: UI-Independent Python Runtime Service

## Status

Complete. Task 5 was implemented on `agent/tauri-interview-foundation` from base
`104a46b793fd0729fc9136dd29d26e739d01f89c`.

The production runtime is now importable and operable without importing PySide6.
The legacy PySide6 entry point remains in place as a UI adapter over the public
runtime service.

## Commits

- `06c73c6` - `refactor: extract UI-independent assistant runtime`
- Report-only follow-up commit: `docs: record task 5 verification`

## Files

Created:

- `ai_assistant/runtime/__init__.py`
- `ai_assistant/runtime/models.py`
- `ai_assistant/runtime/factory.py`
- `ai_assistant/runtime/service.py`
- `ai_assistant/tests/runtime/__init__.py`
- `ai_assistant/tests/runtime/test_service.py`

Modified:

- `ai_assistant/main.py`
- `ai_assistant/config.py`

No desktop UI component, Rust source, protocol schema/framing, or runtime log
artifact was changed.

## Implementation

- Added frozen `SessionConfig`, `RuntimeSnapshot`, and document-ingestion result
  models plus configuration/state errors.
- Added `build_runtime(Config)`, which constructs the current EventBus, dual
  capture, transcriber, LLM, prompt builder, orchestrator, and lazy RAG factory
  without starting audio or connecting providers.
- Added `RuntimeService` ownership of credential validation, session lifecycle,
  audio pumping and metering, listening/role/device/system-audio controls,
  query triggering, background RAG loading, document ingestion, snapshots,
  transcript access, notes, prompt customization, and response history.
- Added `Config` fields and environment loading for input, response, and review
  languages. Session start applies all language fields before transcription starts.
- Reduced `main.py` to Qt setup and translation between Qt signals and the public
  runtime/event-bus APIs. It no longer constructs engine dependencies or reads
  `orchestrator._transcript_buffer`.

## Observed Red State

Initial required red command:

```powershell
.\.venv\Scripts\python.exe -m pytest ai_assistant/tests/runtime/test_service.py -q
```

Observed result:

```text
ERROR ai_assistant/tests/runtime/test_service.py
ModuleNotFoundError: No module named 'ai_assistant.runtime'
1 error in 0.59s
```

Shutdown review added a partial-start regression test. Before the fix it failed
with:

```text
assert deps.transcriber.stop_calls == 1
E assert 0 == 1
1 failed in 4.70s
```

Background RAG review added a failure-path regression test. Before the fix it
timed out and exposed:

```text
NameError: cannot access free variable 'error' where it is not associated with a value in enclosing scope
1 failed in 6.23s
```

## Exact Green Verification

The worktree virtual environment was activated first so the brief's exact `py`
commands resolve to `D:\interview\.worktrees\tauri-interview-foundation\.venv\Scripts\python.exe`.

```powershell
& .\.venv\Scripts\Activate.ps1
py -m pytest ai_assistant/tests/runtime/test_service.py -q
py -m pytest ai_assistant/tests -q
```

Output:

```text
...........                                                              [100%]
11 passed in 9.03s
........................................................................ [ 91%]
.......                                                                  [100%]
79 passed in 45.52s
```

Additional verification:

```powershell
py -c "import ai_assistant.main; print('main import: ok')"
py -c "import builtins; original=builtins.__import__; builtins.__import__=lambda name,*a,**k: (_ for _ in ()).throw(AssertionError(name)) if name == 'PySide6' or name.startswith('PySide6.') else original(name,*a,**k); import ai_assistant.runtime; print('runtime PySide6 guard: ok')"
git diff --check
```

Output:

```text
main import: ok
runtime PySide6 guard: ok
```

`git diff --check` produced no errors.

## Self-Review

### `main.py` cleanup

- Confirmed `main.py` has no references to `DualMicCapture`,
  `DeepgramTranscriber`, `AnthropicLLM`, `PromptBuilder`, `RAGRetriever`,
  `Orchestrator`, or `_transcript_buffer`.
- Qt remains responsible for widget construction, signal connections, timer-based
  status display, and event-to-signal translation.
- Engine controls invoked by Qt use the same public runtime methods intended for
  the sidecar.
- Existing quick-action prompts, note behavior, status text, audio meters,
  response history, RAG readiness, and runtime log messages were preserved.
- A no-op mode-event guard prevents the existing bidirectional Qt/EventBus bridge
  from echoing an unchanged mode indefinitely.

### Idempotent shutdown

- The audio pump is cancelled and awaited before capture/transcriber shutdown.
- Capture and transcriber ownership flags are set before their start awaits, so a
  dependency that partially starts and raises is still cleaned up.
- Capture and transcriber stops are both attempted even if one fails.
- Ownership flags are cleared before stop awaits, so repeated or concurrent
  shutdown cannot stop the same dependency twice.
- Repeated `stop_session()` calls are covered and leave state `stopped` with one
  stop call per dependency.
- Background RAG completions are generation-gated after shutdown, and loaded RAG
  state is saved through the existing index path.

### Test discipline

- Tests use `asyncio.run` helpers; `pytest-asyncio` was not installed or added.
- Side effects are represented by fakes for audio, transcription, LLM, RAG, and
  orchestration boundaries.
- The focused suite covers startup, audio routing, credential validation,
  partial-start cleanup, idempotent stop, device/role/listening/system-audio
  controls, languages, queries, transcript access, ingestion, RAG failure, and
  the PySide6 import boundary.

## Concerns

- No live microphone, WASAPI loopback, Deepgram, DeepSeek, or embedding-model
  smoke session was run. Verification used side-effect fakes and the full existing
  Python suite to avoid provider calls and hardware changes.
- Invoking `py` without activating this worktree's `.venv` selects the global
  Python installation, which lacks PySide6 and FAISS. The documented green
  commands activate `.venv` first.
