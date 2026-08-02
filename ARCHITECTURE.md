# Real-Time AI Assistant Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     USER INPUT / OUTPUT                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────────────┐              ┌──────────────────────┐│
│  │   Microphone Stream  │              │   UI Overlay/Tray    ││
│  │   (System Audio)     │              │   (Response Display) ││
│  └──────────┬───────────┘              └──────────▲───────────┘│
│             │                                     │             │
└─────────────┼─────────────────────────────────────┼─────────────┘
              │                                     │
              │                                     │
┌─────────────▼─────────────────────────────────────┼─────────────┐
│                    AUDIO MODULE                   │             │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Deepgram Live Streaming Transcription Client         │   │
│  │  - Handles WebSocket connection                        │   │
│  │  - Buffers and validates transcripts                   │   │
│  │  - Speaker diarization metadata extraction            │   │
│  └────────────────────┬────────────────────────────────────┘   │
│                       │                                         │
└───────────────────────┼─────────────────────────────────────────┘
                        │
                        ▼
        ┌───────────────────────────────┐
        │   Event Bus / Transcript       │
        │   Stream (AsyncIO)            │
        └───────────┬───────────────────┘
                    │
    ┌───────────────┼───────────────┐
    │               │               │
    ▼               ▼               ▼
┌─────────┐   ┌──────────┐   ┌────────────┐
│ MEMORY  │   │   LLM    │   │    RAG     │
│ MODULE  │   │ MODULE   │   │  MODULE    │
└────┬────┘   └────┬─────┘   └────┬───────┘
     │             │              │
     └─────────────┼──────────────┘
                   │
                   ▼
    ┌──────────────────────────────┐
    │  ORCHESTRATION LAYER         │
    │  - Context Builder           │
    │  - Mode Manager              │
    │  - Event Coordinator         │
    │  - Token Accounting          │
    └──────────────┬───────────────┘
                   │
                   ▼
    ┌──────────────────────────────┐
    │   RESPONSE GENERATION        │
    │  Anthropic Claude API        │
    │  - Prompt Construction       │
    │  - Streaming Response        │
    │  - Token Optimization        │
    └──────────────┬───────────────┘
                   │
                   ▼
    ┌──────────────────────────────┐
    │   OUTPUT FORMATTER           │
    │  - Response Parsing          │
    │  - UI Message Queue          │
    └──────────────┬───────────────┘
                   │
                   └──────────┐
                              │
                              ▼
                    ┌──────────────────────┐
                    │  UI System           │
                    │  - Overlay Display   │
                    │  - Tray Integration  │
                    │  - Keyboard Hooks    │
                    └──────────────────────┘
```

## Core Modules

### 1. **Audio Module** (`/audio`)
- **StreamingClient**: Manages Deepgram WebSocket connection
- **AudioBuffer**: Handles mic input with backpressure
- **TranscriptEventBus**: Event stream of transcript chunks

**Responsibilities**:
- Real-time capture from system microphone
- Streaming to Deepgram API
- Extraction of speaker diarization data
- Buffering & validation of partial transcripts

**Key Patterns**:
- AsyncIO for non-blocking I/O
- WebSocket auto-reconnection with exponential backoff
- Event-driven architecture

### 2. **Memory Module** (`/core/memory.py`)
- **ShortTermMemory**: Sliding window of recent transcripts
- **LongTermMemory**: Document retrieval + vector embeddings
- **ContextWindow**: Token-aware context aggregation

**Design**:
- Circular buffer for short-term (last N seconds)
- Prioritized weighting: recent transcript > RAG results > history
- Token counting for context management

### 3. **LLM Module** (`/llm`)
- **AnthropicClient**: Wrapper around Claude API
- **PromptBuilder**: Constructs system/user prompts with context
- **ResponseHandler**: Streams responses and formats output

**Key Features**:
- Async streaming for real-time responses
- Token budgeting (input + output + margin)
- Flexible system prompts per mode

### 4. **RAG Pipeline** (`/rag`)
- **DocumentIngester**: Loads PDF, DOCX, TXT, Markdown
- **ChunkProcessor**: Splits into semantic chunks
- **Embedder**: Uses FAISS for vector storage
- **Retriever**: Performs semantic search

**Flow**:
```
File Upload → Parse → Chunk → Embed → Store in FAISS → Query
```

### 5. **Orchestration Layer** (`/core/orchestrator.py`)
- **ModeManager**: Passive, Suggestion, Active Response modes
- **ContextBuilder**: Merges transcript + memory + RAG results
- **EventCoordinator**: Routes events between modules
- **TokenCounter**: Ensures context stays within limits

### 6. **UI System** (`/ui`)
- **OverlayUI**: PySide6 or PyQt6 for minimal overlay window
- **TrayIcon**: System tray integration
- **KeyboardListener**: Global hotkey support
- **ResponsePanel**: Displays LLM responses

**Modes**:
- Hidden tray mode (no visible window)
- Floating overlay (semi-transparent)
- Keyboard toggle (Alt+Space or custom)

---

## Data Flow Examples

### Example 1: Active Response Mode
```
1. User speaks → Mic captures audio
2. Deepgram streams transcript
3. TranscriptEventBus emits: "new_chunk: 'hello'"
4. Orchestrator receives event
5. ContextBuilder collects last 30 seconds + RAG context (if needed)
6. Token counter validates context size
7. PromptBuilder constructs message for Claude
8. AnthropicClient.stream() sends request
9. Responses streamed to UI overlay
10. UI displays message (incremental rendering)
```

### Example 2: Document-Aware Query
```
1. User says: "What did the report say about budgets?"
2. Transcript: "What did the report say about budgets?"
3. Orchestrator triggers RAG on key terms: ["report", "budgets"]
4. Retriever queries FAISS for top-3 chunks
5. ContextBuilder merges: [recent transcript] + [RAG chunks]
6. Claude receives augmented context
7. Response includes specific document references
```

### Example 3: Context Overflow
```
1. Short-term memory has 15 min of transcript (high token count)
2. Token counter detects: "input will exceed 90k of 200k limit"
3. ContextBuilder prunes:
   - Keep last 2 minutes of transcript (high priority)
   - Keep top-2 RAG results (if relevant)
   - Discard intermediate history
4. Request sent with optimized context
```

---

## Module Dependencies

```
audio/StreamingClient
  ↓
core/EventBus (AsyncIO)
  ├→ core/memory
  ├→ llm/PromptBuilder
  ├→ rag/Retriever
  └→ core/Orchestrator
      ├→ llm/AnthropicClient
      └→ ui/OverlayUI
```

---

## Configuration & Security

**Environment Variables**:
```env
ANTHROPIC_API_KEY=sk-...
DEEPGRAM_API_KEY=<key>
RAG_DB_PATH=./data/vectors.faiss
MEMORY_WINDOW_SECONDS=300
LLM_MODEL=claude-3-5-sonnet-20241022
MAX_CONTEXT_TOKENS=180000
```

**No Hardcoded Secrets**: All sensitive data loaded from `.env` file.

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Transcript Latency | <500ms |
| LLM Response Start | <2s (after speech ends) |
| Context Lookup (RAG) | <100ms |
| UI Overlay Update | <50ms |
| Memory Footprint | <500MB |

---

## Deployment Architecture

```
Development:
  ├─ Local Python environment
  ├─ .env file with API keys
  └─ FAISS vectors in ./data

Production:
  ├─ Containerized (Docker)
  ├─ Secrets via env vars / K8s secrets
  ├─ Persistent storage for vectors
  ├─ Log aggregation
  └─ Monitoring (latency, API usage)
```

---

## Next Steps

1. **Phase 1**: Audio pipeline + transcription
2. **Phase 2**: LLM integration + basic responses
3. **Phase 3**: RAG pipeline + document ingestion
4. **Phase 4**: UI overlay + mode management
5. **Phase 5**: Optimization & deployment
