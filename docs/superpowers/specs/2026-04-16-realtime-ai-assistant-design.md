# Real-Time AI Assistant — Design Spec

## Problem

Build a production-ready, low-profile AI assistant that listens to live audio (meetings, conversations), transcribes in real-time, augments with document context via RAG, and provides AI-powered responses through Claude — all in a minimal overlay that is **invisible to screen capture** (Zoom, Teams, OBS, etc.).

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| UI framework | PySide6 | Single process, native overlays, system tray, no heavy runtime |
| Embedding model | sentence-transformers (all-MiniLM-L6-v2) | Local, free, 384-dim, ~80MB, good enough for document retrieval |
| Vector store | FAISS (IndexFlatIP) | Fast, local, normalized vectors give cosine similarity via inner product |
| Audio capture | sounddevice | Lightweight C-level callbacks, cross-platform |
| Transcription | Deepgram SDK v3 (nova-2) | Low-latency streaming, diarization, smart formatting |
| LLM | Anthropic Claude (streaming) | Reasoning engine per requirements |
| Async bridge | qasync | Clean asyncio + Qt event loop integration |
| Screen-share hiding | Win32 SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) | OS-level exclusion from all capture tools |
| Deployment | Local pip install | Docker deferred to later phase |

## Architecture

```
Mic (sounddevice) --> Deepgram WebSocket --> EventBus --> Orchestrator
                                                        /     |     \
                                                    RAG     Memory    LLM (Claude)
                                                     |                  |
                                                  FAISS           Streaming Response
                                                                       |
                                                              PySide6 Overlay UI
                                                        (capture-excluded from screen share)
```

**Single-process Python app.** All modules communicate through an async `EventBus`. The UI runs on the Qt event loop with `qasync` bridging asyncio coroutines.

## Module Design

### 1. Event System (`core/events.py`)

The backbone. All inter-module communication flows through typed events.

**Event types:**
- `TRANSCRIPT_UPDATE` — new transcript chunk from Deepgram
- `MODE_CHANGE` — passive/suggestion/active mode transition
- `QUERY_TRIGGERED` — user explicitly requests a response (hotkey)
- `RESPONSE_CHUNK` — one token from LLM stream
- `RESPONSE_COMPLETE` — full response with token usage stats
- `CONNECTION_STATE` — Deepgram connection up/down
- `ERROR` — recoverable error from any module

**EventBus class:**
- `on(event_type, async_handler)` — subscribe
- `off(event_type, handler)` — unsubscribe
- `emit(event_type, data)` — fire-and-forget via `asyncio.create_task`
- Error isolation: one handler crash does not affect others

**Event dataclasses:**
- `TranscriptEvent(text, is_final, speech_final, speaker, confidence, timestamp)`
- `ModeChangeEvent(old_mode, new_mode)`
- `ResponseChunkEvent(text, request_id)`
- `ResponseCompleteEvent(full_text, request_id, input_tokens, output_tokens)`

### 2. Configuration (`config.py`)

Single `@dataclass` with `from_env()` classmethod. Loads from environment variables (via `python-dotenv`).

**Key fields:**
- Audio: `sample_rate=16000`, `channels=1`, `audio_blocksize=1024`
- Deepgram: `model="nova-2"`, `diarize=True`, `interim_results=True`, `utterance_end_ms=1000`
- Anthropic: `llm_model="claude-sonnet-4-20250514"`, `max_output_tokens=1024`, `max_context_tokens=180000`, `temperature=0.3`
- Context budget fractions: `system_prompt=0.05`, `recent_transcript=0.40`, `rag=0.30`, `history=0.25`
- Reconnect: `base_delay=1.0s`, `max_delay=30.0s`, `max_attempts=10`

### 3. Audio Pipeline (`audio/`)

#### `mic_capture.py` — MicCapture

- `sounddevice.RawInputStream` with C-level callback
- Callback pushes raw bytes to `queue.Queue(maxsize=100)` — backpressure drops frames
- Async iterator drains via `loop.run_in_executor(None, queue.get)`
- Format: 16kHz, 16-bit mono PCM, 1024-sample blocks (~64ms)

#### `deepgram_client.py` — DeepgramTranscriber

- Manages Deepgram `AsyncLiveClient` WebSocket connection
- Registers handlers: `Transcript`, `Error`, `Close`, `UtteranceEnd`
- `send_audio(bytes)` wrapped in `asyncio.to_thread()` (SDK v3 send is synchronous)
- On transcript: extracts text, is_final, speech_final, speaker diarization, confidence; emits `TRANSCRIPT_UPDATE`
- Auto-reconnect on close: exponential backoff (1s, 2s, 4s, ... up to 30s, max 10 attempts)
- LiveOptions: `encoding="linear16"`, `smart_format=True`, `punctuate=True`

**Data flow:**
```
sounddevice callback (C thread) --> queue.Queue --> async iterator --> audio_pump --> DeepgramTranscriber.send_audio() --> Deepgram WS
Deepgram WS result --> _on_transcript --> EventBus.emit(TRANSCRIPT_UPDATE)
```

### 4. LLM Pipeline (`llm/`)

#### `prompt_builder.py` — PromptBuilder + TranscriptBuffer

**TranscriptBuffer:**
- Rolling buffer of `TranscriptEvent` segments, max 300 seconds
- `get_recent_text(max_chars)` returns final transcripts newest-last, with speaker labels
- Auto-prunes on add

**PromptBuilder:**
- Constructs Claude messages from: system prompt + transcript + RAG context + conversation history
- Token budgeting: fast `len(text)//4` estimate for hot path; Anthropic `count_tokens` API for precision when near limit
- System prompt template includes mode instructions (passive/suggestion/active)
- Conversation history: last 50 turns, trimmed to budget
- Message format: `<transcript>...</transcript>`, `<context>...</context>`, `<query>...</query>` sections

#### `anthropic_client.py` — AnthropicLLM

- `AsyncAnthropic` client with streaming via `messages.stream()` context manager
- `stream_generate(prompt)` emits `RESPONSE_CHUNK` per token, `RESPONSE_COMPLETE` with usage stats
- `submit(prompt)` runs generation as managed background task
- `cancel_active()` cancels in-flight generation before starting new one (prevents stale overlap)
- Error handling: catches `APIStatusError`, emits to error bus

### 5. RAG Pipeline (`rag/`)

#### `models.py` — Data classes

- `DocumentType` enum: PDF, DOCX, TXT, MARKDOWN
- `DocumentMetadata(source_path, doc_type, title, total_pages, ingested_at)`
- `Chunk(text, chunk_index, source_path, page_number, start_char, end_char, metadata)`
- `ChunkWithScore(chunk, score)`
- `RetrievalResult(query, chunks: list[ChunkWithScore], total_searched)`

#### `ingestion.py` — DocumentIngester

- `ingest(file_path) -> (list[str], DocumentMetadata)` — returns page-level text strings
- PDF: `pymupdf` (fitz) — `doc.load_page(i).get_text("text")`
- DOCX: `python-docx` — concatenated `paragraph.text`, no true pages
- TXT: utf-8 with latin-1 fallback
- Markdown: raw text read
- File type detection via `Path.suffix.lower()`

#### `chunking.py` — RecursiveTextSplitter

- Chunk size: 512 tokens, overlap: 64 tokens
- Separators tried in order: `["\n\n", "\n", ". ", " "]`
- Token counting via sentence-transformers tokenizer (matches embedding model)
- Produces `Chunk` objects with source path, page number, character offsets

#### `embeddings.py` — LocalEmbedder

- Wraps `SentenceTransformer("all-MiniLM-L6-v2")`
- `embed_texts(texts, batch_size=64) -> np.ndarray` — shape `(n, 384)`, normalized
- `embed_query(query) -> np.ndarray` — shape `(1, 384)`, normalized
- `normalize_embeddings=True` so FAISS inner product = cosine similarity

#### `vector_store.py` — FAISSVectorStore

- `faiss.IndexFlatIP(384)` — inner product on normalized vectors
- Parallel `list[Chunk]` for metadata (kept in sync with FAISS index order)
- `add(embeddings, chunks)` — append to index + metadata
- `search(query_vector, k=5) -> list[(index, score)]`
- `save(directory)` — `faiss.write_index` + `pickle.dump` metadata
- `load(directory)` — `faiss.read_index` + `pickle.load`
- `remove_by_source(path)` — rebuild index (acceptable for <100k vectors)

#### `retriever.py` — RAGRetriever

- High-level facade: `ingest_file(path) -> int` (chunk count), `query(text, k=5) -> RetrievalResult`
- Pipeline: parse → chunk → embed → store
- Query: embed → search → filter by `relevance_threshold=0.3` → return scored chunks
- `ingest_directory(path)` for bulk loading
- `save_index(dir)` / `load_index(dir)` for persistence

### 6. Orchestrator (`core/orchestrator.py`)

**Mode state machine:**
- `PASSIVE` — transcripts buffered, no LLM calls
- `SUGGESTION` — 1.5s debounce after `speech_final`, then background generation
- `ACTIVE` — immediate generation on `speech_final`

**Key methods:**
- `set_mode(mode)` — transitions, cancels active generation if switching to passive
- `trigger_query(query?)` — manual trigger from hotkey, builds prompt with RAG context
- `_on_transcript(event)` — buffers transcript, triggers generation per mode rules

**Context building:** merges `TranscriptBuffer.get_recent_text()` + `RAGRetriever.query()` results + conversation history into a `BuiltPrompt` via `PromptBuilder`.

### 7. UI System (`ui/`)

#### Screen Capture Exclusion (Critical Feature)

```python
import ctypes
WDA_EXCLUDEFROMCAPTURE = 0x00000011

def exclude_from_capture(hwnd: int) -> bool:
    """Makes window invisible to Zoom, Teams, OBS, screenshots, etc."""
    return ctypes.windll.user32.SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)
```

- Applied on `OverlayWindow` creation, after `self.winId()` is valid
- Requires Windows 10 2004+ (build 19041) — which Windows 11 satisfies
- Toggleable via tray menu ("Visible in Screen Share") for cases where user wants to share it
- Fallback for older builds: log warning, minimize-to-tray option

#### `signals.py` — AppSignals

Centralized `QObject` signal hub shared by all widgets:
- `toggle_overlay`, `overlay_visible_changed(bool)`
- `response_chunk(str)`, `response_complete(str)`, `response_clear()`
- `mode_changed(str)`, `toggle_listening()`, `listening_changed(bool)`
- `pin_response(str)`, `unpin_response(int)`
- `status_message(str)`

#### `styles.py` — Constants + QSS

- Dark theme: `rgba(30, 30, 30, 200)` background, `#E0E0E0` text
- Mode indicator colors: green (active), yellow (suggestion), gray (passive)
- Font: Segoe UI, 13px

#### `overlay_window.py` — OverlayWindow

- Window flags: `FramelessWindowHint | WindowStaysOnTopHint | Tool` (hides from taskbar)
- `WA_TranslucentBackground` attribute
- `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on creation
- Custom `paintEvent`: rounded rectangle with semi-transparent fill
- Click-through via `WS_EX_TRANSPARENT | WS_EX_LAYERED` (toggleable by hotkey)
- Manual drag via `mousePressEvent` / `mouseMoveEvent`
- Fade animation via `QPropertyAnimation` on `windowOpacity`
- Default position: bottom-right of primary screen, 400x300

**Widget hierarchy:**
```
OverlayWindow (frameless, capture-excluded)
  +-- StatusBar (24px): mode dot + title + drag area
  +-- ResponsePanel: QTextEdit (read-only) + Copy/Pin/Clear buttons
  +-- PinnedPanel (collapsible): scrollable list of pinned items
```

#### `tray_icon.py` — TrayIcon

- Context menu: Show Overlay (checkable), Mode submenu (Passive/Suggestion/Active radio), Listening (checkable), Visible in Screen Share (checkable), Quit
- Programmatic icons: 16x16 QPixmap with colored circles per mode
- Double-click: toggle overlay

#### `shortcuts.py` — ShortcutManager

- Win32 `RegisterHotKey` for global shortcuts (works when unfocused)
- Default bindings: `Alt+Space` (toggle overlay), `Ctrl+Shift+L` (toggle listening)
- `nativeEvent` override on OverlayWindow intercepts `WM_HOTKEY` (0x0312)
- Cleanup via `UnregisterHotKey` on shutdown

#### `response_panel.py` — ResponsePanel

- Read-only `QTextEdit` with streaming append
- `append_chunk(text)` — `moveCursor(End)` + `insertPlainText()`, auto-scroll
- Copy: `QApplication.clipboard().setText()`
- Pin: emits `pin_response` signal

#### `pinned_panel.py` — PinnedPanel

- Collapsible scroll area with `PinnedItem` widgets
- Each item: truncated preview + copy + remove buttons
- Header shows pin count

#### `app.py` — AssistantApp

- Creates `QApplication`, initializes all components
- `qasync.QEventLoop` integrates asyncio with Qt event loop
- Startup sequence: load config → init RAG (load persisted index) → init audio → init UI → run

### 8. Main Entrypoint (`main.py`)

```
1. Load .env via python-dotenv
2. Config.from_env()
3. Create EventBus
4. Create MicCapture, DeepgramTranscriber
5. Create PromptBuilder, AnthropicLLM
6. Create LocalEmbedder, FAISSVectorStore, RAGRetriever (load persisted index)
7. Create Orchestrator (wires everything)
8. Create AssistantApp (UI), pass signals + orchestrator
9. Start audio_pump coroutine (mic → Deepgram)
10. app.run() (Qt + asyncio event loop)
```

## Project Structure

```
ai_assistant/
+-- __init__.py
+-- config.py
+-- main.py
+-- audio/
|   +-- __init__.py
|   +-- mic_capture.py
|   +-- deepgram_client.py
+-- core/
|   +-- __init__.py
|   +-- events.py
|   +-- orchestrator.py
+-- llm/
|   +-- __init__.py
|   +-- anthropic_client.py
|   +-- prompt_builder.py
+-- rag/
|   +-- __init__.py
|   +-- models.py
|   +-- ingestion.py
|   +-- chunking.py
|   +-- embeddings.py
|   +-- vector_store.py
|   +-- retriever.py
+-- ui/
|   +-- __init__.py
|   +-- app.py
|   +-- signals.py
|   +-- styles.py
|   +-- tray_icon.py
|   +-- overlay_window.py
|   +-- response_panel.py
|   +-- pinned_panel.py
|   +-- shortcuts.py
+-- data/                    # FAISS index persistence (gitignored)
+-- tests/
+-- .env.example
+-- .gitignore
+-- requirements.txt
```

## Dependencies

```
anthropic>=0.49
deepgram-sdk>=3.7
sounddevice>=0.5
PySide6>=6.6
qasync>=0.27
sentence-transformers>=3.0
faiss-cpu>=1.8
pymupdf>=1.24
python-docx>=1.1
python-dotenv>=1.0
numpy>=1.26
```

## Implementation Phases

**Phase 1: Core skeleton** — config.py, events.py, __init__ files, .env.example, .gitignore, requirements.txt

**Phase 2: Audio pipeline** — mic_capture.py, deepgram_client.py. Testable standalone: run and see transcripts printed.

**Phase 3: LLM pipeline** — prompt_builder.py, anthropic_client.py. Testable with fake transcript data.

**Phase 4: RAG pipeline** — models, ingestion, chunking, embeddings, vector_store, retriever. Independent of audio. Testable: ingest a PDF, query it.

**Phase 5: Orchestrator** — wires audio + LLM + RAG, mode state machine.

**Phase 6: UI** — signals, styles, overlay_window, response_panel, pinned_panel, tray_icon, shortcuts, app.py.

**Phase 7: Integration** — end-to-end wiring in main.py.

Phases 2, 3, 4 are parallelizable. Phases 5-7 are sequential.

## Verification Plan

1. **Audio**: Run mic_capture + deepgram_client standalone, verify transcripts print in <500ms
2. **LLM**: Feed fake transcript to prompt_builder, send to Claude, verify streaming works
3. **RAG**: Ingest a test PDF, query with related text, verify relevant chunks returned
4. **UI**: Launch overlay, verify it's invisible in a Zoom screen share, test hotkeys
5. **End-to-end**: Speak into mic, see transcript → Claude response in overlay, test all three modes
6. **Screen capture**: Start Zoom/Teams share, confirm overlay is not visible to others

## Known Pitfalls

1. **Deepgram SDK v3 handler signature**: `on()` callbacks receive `(self_ref, result, **kwargs)` — first arg is connection reference, not event
2. **sounddevice callback thread**: Cannot do async or logging — only `queue.put_nowait()`
3. **Token estimate drift**: `len//4` can be off 20-30% for non-English; use `count_tokens` API near budget limits
4. **FAISS on Windows**: `faiss-cpu` installs via pip; fall back to conda-forge if issues
5. **sentence-transformers first load**: Downloads ~80MB model; handle gracefully with log message
6. **Click-through + interaction**: Need reliable toggle (hotkey) to switch between click-through and interactive modes
7. **SetWindowDisplayAffinity**: Requires valid HWND — call after window is shown, not in constructor
