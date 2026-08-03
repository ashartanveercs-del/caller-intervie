import { createStore, type StoreApi } from "zustand/vanilla";
import { decodeEnvelope, EventKind, type Envelope } from "../shared/protocol";
import type { SessionRecord, SessionStatus, SidecarStatus } from "../platform";

export type RuntimeHealthStatus = "unknown" | "pending" | "ready" | "degraded" | "error" | "offline";

export type DependencyHealth = {
  status: RuntimeHealthStatus;
  message?: string;
};

export type RuntimeHealth = {
  sidecar: DependencyHealth;
  microphone: DependencyHealth;
  systemAudio: DependencyHealth;
  speechProvider: DependencyHealth;
  modelProvider: DependencyHealth;
};

export type SessionLanguages = {
  ui: string;
  input: string;
  response: string;
  review: string;
};

export type TranscriptTurn = {
  id: string;
  text: string;
  isFinal: boolean;
  speechFinal: boolean;
  speakerRole?: string;
  language?: string;
  sequence: number;
};

export type Suggestion = {
  id: string;
  text: string;
  isComplete: boolean;
  correlationId: string | null;
  turnId?: string;
};

export type SessionStoreState = {
  session: SessionRecord | null;
  turns: TranscriptTurn[];
  partialTurnsById: Record<string, TranscriptTurn>;
  suggestionsByTurn: Record<string, Suggestion>;
  partialSuggestionsByCorrelation: Record<string, Suggestion>;
  requestToTurn: Record<string, string>;
  unresolvedSuggestionCorrelations: string[];
  health: RuntimeHealth;
  languages: SessionLanguages;
  sidecarGeneration: number;
  lastSequence: number;
  lastError: string | null;
  applyEnvelope(value: unknown): void;
  associateRequestWithTurn(requestId: string, turnId: string): void;
  beginSession(session: SessionRecord): void;
  endSession(status: "completed" | "interrupted"): void;
  restoreSession(session: SessionRecord): void;
  restoreReplay(events: readonly unknown[]): void;
  clearTransientState(): void;
  setLanguages(languages: SessionLanguages): void;
  setSidecarStatus(status: SidecarStatus): void;
  recordError(error: unknown): void;
};

const initialLanguages: SessionLanguages = { ui: "en", input: "en", response: "en", review: "en" };

function unknownHealth(): RuntimeHealth {
  return {
    sidecar: { status: "unknown" },
    microphone: { status: "unknown" },
    systemAudio: { status: "unknown" },
    speechProvider: { status: "unknown" },
    modelProvider: { status: "unknown" },
  };
}

export function createSessionStore(): StoreApi<SessionStoreState> {
  const seenEventIds = new Set<string>();

  return createStore<SessionStoreState>((set, get) => {
    const applyEnvelope = (value: unknown, replay = false) => {
      let envelope: Envelope;
      try {
        envelope = decodeEnvelope(value);
      } catch (error) {
        get().recordError(error);
        return;
      }

      const current = get();
      if (seenEventIds.has(envelope.id)) {
        return;
      }
      if (!replay && envelope.kind !== EventKind.SIDECAR_READY && envelope.sequence <= current.lastSequence) {
        return;
      }

      seenEventIds.add(envelope.id);
      const nextSequence = envelope.kind === EventKind.SIDECAR_READY ? envelope.sequence : Math.max(current.lastSequence, envelope.sequence);
      set({ lastSequence: nextSequence });

      switch (envelope.kind) {
        case EventKind.SIDECAR_READY:
          set((state) => ({
            sidecarGeneration: state.sidecarGeneration + 1,
            lastSequence: envelope.sequence,
            health: { ...state.health, sidecar: { status: "ready" } },
          }));
          return;
        case EventKind.TRANSCRIPT_UPDATED:
          applyTranscript(get, set, envelope);
          return;
        case EventKind.SUGGESTION_CHUNK:
          applySuggestion(get, set, envelope, false, replay);
          return;
        case EventKind.SUGGESTION_COMPLETED:
          applySuggestion(get, set, envelope, true, replay);
          return;
        case EventKind.AUDIO_HEALTH:
        case EventKind.PROVIDER_HEALTH:
          applyHealth(set, envelope);
          return;
        case EventKind.SESSION_STATE:
          applySessionState(get, set, envelope);
          return;
        case EventKind.RUNTIME_ERROR:
          get().recordError(stringPayload(envelope.payload, "message") ?? "runtime error");
          return;
        default:
          return;
      }
    };

    return {
      session: null,
      turns: [],
      partialTurnsById: {},
      suggestionsByTurn: {},
      partialSuggestionsByCorrelation: {},
      requestToTurn: {},
      unresolvedSuggestionCorrelations: [],
      health: unknownHealth(),
      languages: initialLanguages,
      sidecarGeneration: 0,
      lastSequence: -1,
      lastError: null,
      applyEnvelope,
      associateRequestWithTurn(requestId, turnId) {
        set((state) => {
          const partial = state.partialSuggestionsByCorrelation[requestId];
          const partialSuggestionsByCorrelation = { ...state.partialSuggestionsByCorrelation };
          if (partial) {
            delete partialSuggestionsByCorrelation[requestId];
          }
          return {
            requestToTurn: { ...state.requestToTurn, [requestId]: turnId },
            partialSuggestionsByCorrelation,
            suggestionsByTurn: partial
              ? { ...state.suggestionsByTurn, [turnId]: { ...partial, turnId } }
              : state.suggestionsByTurn,
          };
        });
      },
      beginSession(session) {
        set({
          session,
          languages: languagesFromSession(session, get().languages.ui),
          lastError: null,
        });
      },
      endSession(status) {
        set((state) => ({
          session: state.session ? { ...state.session, status } : null,
        }));
      },
      restoreSession(session) {
        get().beginSession(session);
      },
      restoreReplay(events) {
        for (const event of [...events].sort(compareChronologically)) {
          applyEnvelope(event, true);
        }
      },
      clearTransientState() {
        set((state) => ({
          partialTurnsById: {},
          partialSuggestionsByCorrelation: {},
          requestToTurn: {},
          unresolvedSuggestionCorrelations: [],
          health: { ...unknownHealth(), sidecar: { status: "pending" } },
          sidecarGeneration: state.sidecarGeneration + 1,
          lastSequence: -1,
          lastError: null,
        }));
      },
      setLanguages(languages) {
        set({ languages });
      },
      setSidecarStatus(status) {
        set((state) => ({
          health: { ...state.health, sidecar: { status: sidecarHealth(status.state), message: status.diagnostics[0] } },
        }));
      },
      recordError(error) {
        set({ lastError: errorMessage(error) });
      },
    };
  });
}

function applyTranscript(
  get: () => SessionStoreState,
  set: StoreApi<SessionStoreState>["setState"],
  envelope: Envelope,
) {
  const turnId = stringPayload(envelope.payload, "turn_id");
  const text = stringPayload(envelope.payload, "text");
  if (!turnId || text === undefined) {
    get().recordError("transcript.updated payload is missing turn_id or text");
    return;
  }
  const turn: TranscriptTurn = {
    id: turnId,
    text,
    isFinal: booleanPayload(envelope.payload, "is_final"),
    speechFinal: booleanPayload(envelope.payload, "speech_final"),
    speakerRole: stringPayload(envelope.payload, "speaker_role"),
    language: stringPayload(envelope.payload, "language"),
    sequence: envelope.sequence,
  };

  set((state) => {
    if (!turn.isFinal) {
      return { partialTurnsById: { ...state.partialTurnsById, [turn.id]: turn } };
    }
    const partialTurnsById = { ...state.partialTurnsById };
    delete partialTurnsById[turn.id];
    const position = state.turns.findIndex((item) => item.id === turn.id);
    const turns = position === -1
      ? [...state.turns, turn]
      : state.turns.map((item, index) => (index === position ? turn : item));
    return { turns, partialTurnsById };
  });
}

function applySuggestion(
  get: () => SessionStoreState,
  set: StoreApi<SessionStoreState>["setState"],
  envelope: Envelope,
  isComplete: boolean,
  replay: boolean,
) {
  const suggestionId = stringPayload(envelope.payload, "suggestion_id");
  const text = stringPayload(envelope.payload, "text");
  if (!suggestionId || text === undefined) {
    get().recordError(`${envelope.kind} payload is missing suggestion_id or text`);
    return;
  }
  const correlationId = envelope.correlation_id;
  const state = get();
  const turnId = correlationId ? state.requestToTurn[correlationId] : undefined;
  const replayTurnId = !turnId && replay ? deriveReplayTurnId(state) : undefined;
  const association = turnId ?? replayTurnId;
  const suggestion: Suggestion = {
    id: suggestionId,
    text,
    isComplete,
    correlationId,
    turnId: association,
  };

  if (association) {
    set((current) => {
      const prior = current.suggestionsByTurn[association];
      if (!isComplete && prior?.isComplete) {
        return {};
      }
      return {
        suggestionsByTurn: { ...current.suggestionsByTurn, [association]: suggestion },
      };
    });
    return;
  }

  if (correlationId) {
    set((current) => ({
      partialSuggestionsByCorrelation: isComplete
        ? current.partialSuggestionsByCorrelation
        : { ...current.partialSuggestionsByCorrelation, [correlationId]: suggestion },
      unresolvedSuggestionCorrelations: isComplete && !current.unresolvedSuggestionCorrelations.includes(correlationId)
        ? [...current.unresolvedSuggestionCorrelations, correlationId]
        : current.unresolvedSuggestionCorrelations,
    }));
  }
}

function deriveReplayTurnId(state: SessionStoreState): string | undefined {
  const candidates = state.turns.filter((turn) => !state.suggestionsByTurn[turn.id]);
  return candidates.length === 1 ? candidates[0].id : undefined;
}

function applyHealth(set: StoreApi<SessionStoreState>["setState"], envelope: Envelope) {
  const dependency = healthKey(stringPayload(envelope.payload, "dependency"));
  const status = healthStatus(stringPayload(envelope.payload, "status"));
  if (!dependency || !status) {
    return;
  }
  const message = stringPayload(envelope.payload, "message");
  set((state) => ({
    health: { ...state.health, [dependency]: message ? { status, message } : { status } },
  }));
}

function applySessionState(
  get: () => SessionStoreState,
  set: StoreApi<SessionStoreState>["setState"],
  envelope: Envelope,
) {
  const current = get();
  const state = sessionStatus(stringPayload(envelope.payload, "state"));
  const input = stringPayload(envelope.payload, "input_language");
  const response = stringPayload(envelope.payload, "response_language");
  const review = stringPayload(envelope.payload, "review_language");
  set({
    session: current.session && state ? { ...current.session, status: state } : current.session,
    languages: {
      ui: current.languages.ui,
      input: input ?? current.languages.input,
      response: response ?? current.languages.response,
      review: review ?? current.languages.review,
    },
  });
}

function languagesFromSession(session: SessionRecord, uiFallback: string): SessionLanguages {
  return {
    ui: session.uiLanguage ?? uiFallback,
    input: session.inputLanguage,
    response: session.responseLanguage,
    review: session.reviewLanguage,
  };
}

function stringPayload(payload: Record<string, unknown>, key: string): string | undefined {
  const value = payload[key];
  return typeof value === "string" ? value : undefined;
}

function booleanPayload(payload: Record<string, unknown>, key: string): boolean {
  return payload[key] === true;
}

function healthKey(value: string | undefined): keyof RuntimeHealth | undefined {
  switch (value) {
    case "sidecar": return "sidecar";
    case "microphone": return "microphone";
    case "system_audio": return "systemAudio";
    case "speech_provider": return "speechProvider";
    case "model_provider": return "modelProvider";
    default: return undefined;
  }
}

function healthStatus(value: string | undefined): RuntimeHealthStatus | undefined {
  return value === "unknown" || value === "pending" || value === "ready" || value === "degraded" || value === "error" || value === "offline"
    ? value
    : undefined;
}

function sessionStatus(value: string | undefined): SessionStatus | undefined {
  return value === "active" || value === "completed" || value === "interrupted" ? value : undefined;
}

function sidecarHealth(state: SidecarStatus["state"]): RuntimeHealthStatus {
  switch (state) {
    case "ready": return "ready";
    case "starting":
    case "restarting": return "pending";
    case "stopped": return "offline";
    case "failed": return "error";
  }
}

function compareChronologically(left: unknown, right: unknown): number {
  const leftTimestamp = timestampOf(left);
  const rightTimestamp = timestampOf(right);
  return leftTimestamp - rightTimestamp;
}

function timestampOf(value: unknown): number {
  return typeof value === "object" && value !== null && "timestamp_ms" in value && typeof value.timestamp_ms === "number"
    ? value.timestamp_ms
    : Number.MAX_SAFE_INTEGER;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
