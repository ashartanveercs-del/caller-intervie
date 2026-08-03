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
  durability: "transient" | "durable";
  correlationId: string | null;
  turnId?: string;
  eventId: string;
  sessionId: string | null;
  sequence: number;
  timestampMs: number;
  payload: Record<string, unknown>;
};

export type UnresolvedCompletedSuggestion = Suggestion & {
  isComplete: true;
  durability: "durable";
  associationReason: "missing-request-association" | "ambiguous-replay";
};

export type SessionStoreState = {
  session: SessionRecord | null;
  sessionRevision: number;
  turns: TranscriptTurn[];
  partialTurnsById: Record<string, TranscriptTurn>;
  suggestionsByTurn: Record<string, Suggestion>;
  partialSuggestionsById: Record<string, Suggestion>;
  partialSuggestionIdByCorrelation: Record<string, string>;
  unresolvedCompletedSuggestionsById: Record<string, UnresolvedCompletedSuggestion>;
  requestToTurn: Record<string, string>;
  unresolvedSuggestionCorrelations: string[];
  health: RuntimeHealth;
  languages: SessionLanguages;
  sidecarGeneration: number;
  lastSequence: number;
  lastError: string | null;
  applyEnvelope(value: unknown): void;
  applyPersistedEnvelope(value: unknown): void;
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

type IngestionSource = "live" | "persisted";

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
    const ingest = (value: unknown, source: IngestionSource) => {
      let envelope: Envelope;
      try {
        envelope = decodeEnvelope(value);
      } catch (error) {
        get().recordError(error);
        return;
      }
      if (!acceptsEnvelope(get(), envelope) || seenEventIds.has(envelope.id)) {
        return;
      }
      if (source === "live" && envelope.kind !== EventKind.SIDECAR_READY && envelope.sequence <= get().lastSequence) {
        return;
      }

      seenEventIds.add(envelope.id);
      if (source === "live") {
        set({ lastSequence: envelope.sequence });
      }

      switch (envelope.kind) {
        case EventKind.SIDECAR_READY:
          if (source === "live") {
            set((state) => ({
              sidecarGeneration: state.sidecarGeneration + 1,
              lastSequence: envelope.sequence,
              health: { ...state.health, sidecar: { status: "ready" } },
            }));
          }
          return;
        case EventKind.TRANSCRIPT_UPDATED:
          applyTranscript(get, set, envelope);
          return;
        case EventKind.SUGGESTION_CHUNK:
          applySuggestion(get, set, envelope, false, source);
          return;
        case EventKind.SUGGESTION_COMPLETED:
          applySuggestion(get, set, envelope, true, source);
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
      sessionRevision: 0,
      turns: [],
      partialTurnsById: {},
      suggestionsByTurn: {},
      partialSuggestionsById: {},
      partialSuggestionIdByCorrelation: {},
      unresolvedCompletedSuggestionsById: {},
      requestToTurn: {},
      unresolvedSuggestionCorrelations: [],
      health: unknownHealth(),
      languages: initialLanguages,
      sidecarGeneration: 0,
      lastSequence: -1,
      lastError: null,
      applyEnvelope(value) {
        ingest(value, "live");
      },
      applyPersistedEnvelope(value) {
        ingest(value, "persisted");
      },
      associateRequestWithTurn(requestId, turnId) {
        set((state) => {
          const unresolved = Object.values(state.unresolvedCompletedSuggestionsById)
            .filter((suggestion) => suggestion.correlationId === requestId)
            .sort((left, right) => left.sequence - right.sequence);
          const unresolvedCompletedSuggestionsById = { ...state.unresolvedCompletedSuggestionsById };
          const partialSuggestionsById = { ...state.partialSuggestionsById };
          const partialSuggestionIdByCorrelation = { ...state.partialSuggestionIdByCorrelation };
          let suggestionsByTurn = state.suggestionsByTurn;
          for (const suggestion of unresolved) {
            delete unresolvedCompletedSuggestionsById[suggestion.id];
            delete partialSuggestionsById[suggestion.id];
            if (suggestion.correlationId) delete partialSuggestionIdByCorrelation[suggestion.correlationId];
            suggestionsByTurn = {
              ...suggestionsByTurn,
              [turnId]: resolvedSuggestion(suggestion, turnId),
            };
          }
          return {
            requestToTurn: { ...state.requestToTurn, [requestId]: turnId },
            partialSuggestionsById,
            partialSuggestionIdByCorrelation,
            unresolvedCompletedSuggestionsById,
            unresolvedSuggestionCorrelations: Object.values(unresolvedCompletedSuggestionsById)
              .flatMap((suggestion) => suggestion.correlationId ? [suggestion.correlationId] : []),
            suggestionsByTurn,
          };
        });
      },
      beginSession(session) {
        set((state) => resetForSession(state, session));
      },
      endSession(status) {
        set((state) => ({
          session: state.session ? { ...state.session, status } : null,
        }));
      },
      restoreSession(session) {
        set((state) => state.session?.id === session.id
          ? { session, languages: languagesFromSession(session, state.languages.ui), lastError: null }
          : resetForSession(state, session));
      },
      restoreReplay(events) {
        for (const event of [...events].sort(compareChronologically)) {
          ingest(event, "persisted");
        }
      },
      clearTransientState() {
        set((state) => ({
          partialTurnsById: {},
          partialSuggestionsById: {},
          partialSuggestionIdByCorrelation: {},
          requestToTurn: {},
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
          health: {
            ...state.health,
            sidecar: { status: sidecarHealth(status.state), message: status.diagnostics[0] },
          },
        }));
      },
      recordError(error) {
        set({ lastError: errorMessage(error) });
      },
    };
  });
}

function acceptsEnvelope(state: SessionStoreState, envelope: Envelope): boolean {
  if (envelope.session_id === null) {
    return envelope.kind === EventKind.SIDECAR_READY
      || envelope.kind === EventKind.AUDIO_HEALTH
      || envelope.kind === EventKind.PROVIDER_HEALTH
      || envelope.kind === EventKind.RUNTIME_ERROR;
  }
  return envelope.kind !== EventKind.SIDECAR_READY && state.session?.id === envelope.session_id;
}

function resetForSession(state: SessionStoreState, session: SessionRecord): Partial<SessionStoreState> {
  return {
    session,
    sessionRevision: state.sessionRevision + 1,
    turns: [],
    partialTurnsById: {},
    suggestionsByTurn: {},
    partialSuggestionsById: {},
    partialSuggestionIdByCorrelation: {},
    unresolvedCompletedSuggestionsById: {},
    requestToTurn: {},
    unresolvedSuggestionCorrelations: [],
    languages: languagesFromSession(session, state.languages.ui),
    lastError: null,
  };
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
    return {
      turns: position === -1
        ? [...state.turns, turn]
        : state.turns.map((item, index) => index === position ? turn : item),
      partialTurnsById,
    };
  });
}

function applySuggestion(
  get: () => SessionStoreState,
  set: StoreApi<SessionStoreState>["setState"],
  envelope: Envelope,
  isComplete: boolean,
  source: IngestionSource,
) {
  const suggestionId = stringPayload(envelope.payload, "suggestion_id");
  const delta = stringPayload(envelope.payload, "text");
  if (!suggestionId || delta === undefined) {
    get().recordError(`${envelope.kind} payload is missing suggestion_id or text`);
    return;
  }
  const state = get();
  if (!isComplete && hasCompletedSuggestion(state, suggestionId)) {
    return;
  }
  const correlationId = envelope.correlation_id;
  const associatedTurnId = correlationId ? state.requestToTurn[correlationId] : undefined;
  const replayTurnId = !associatedTurnId && source === "persisted" ? deriveReplayTurnId(state) : undefined;
  const turnId = associatedTurnId ?? replayTurnId;
  const prior = state.partialSuggestionsById[suggestionId];
  const suggestion: Suggestion = {
    id: suggestionId,
    text: isComplete ? delta : `${prior?.text ?? ""}${delta}`,
    isComplete,
    durability: isComplete ? "durable" : "transient",
    correlationId,
    turnId,
    eventId: envelope.id,
    sessionId: envelope.session_id,
    sequence: envelope.sequence,
    timestampMs: envelope.timestamp_ms,
    payload: { ...envelope.payload },
  };

  if (!isComplete) {
    set((current) => ({
      partialSuggestionsById: { ...current.partialSuggestionsById, [suggestionId]: suggestion },
      partialSuggestionIdByCorrelation: correlationId
        ? { ...current.partialSuggestionIdByCorrelation, [correlationId]: suggestionId }
        : current.partialSuggestionIdByCorrelation,
    }));
    return;
  }

  if (turnId) {
    set((current) => removePartialAndStoreCompleted(current, suggestion, turnId));
    return;
  }

  const unresolved: UnresolvedCompletedSuggestion = {
    ...suggestion,
    isComplete: true,
    durability: "durable",
    associationReason: source === "persisted" ? "ambiguous-replay" : "missing-request-association",
  };
  set((current) => {
    const partialSuggestionsById = { ...current.partialSuggestionsById };
    const partialSuggestionIdByCorrelation = { ...current.partialSuggestionIdByCorrelation };
    delete partialSuggestionsById[suggestionId];
    if (correlationId) delete partialSuggestionIdByCorrelation[correlationId];
    const unresolvedCompletedSuggestionsById = {
      ...current.unresolvedCompletedSuggestionsById,
      [suggestionId]: unresolved,
    };
    return {
      partialSuggestionsById,
      partialSuggestionIdByCorrelation,
      unresolvedCompletedSuggestionsById,
      unresolvedSuggestionCorrelations: Object.values(unresolvedCompletedSuggestionsById)
        .flatMap((item) => item.correlationId ? [item.correlationId] : []),
    };
  });
}

function removePartialAndStoreCompleted(
  state: SessionStoreState,
  suggestion: Suggestion,
  turnId: string,
): Partial<SessionStoreState> {
  const partialSuggestionsById = { ...state.partialSuggestionsById };
  const partialSuggestionIdByCorrelation = { ...state.partialSuggestionIdByCorrelation };
  delete partialSuggestionsById[suggestion.id];
  if (suggestion.correlationId) delete partialSuggestionIdByCorrelation[suggestion.correlationId];
  return {
    partialSuggestionsById,
    partialSuggestionIdByCorrelation,
    suggestionsByTurn: { ...state.suggestionsByTurn, [turnId]: { ...suggestion, turnId } },
  };
}

function resolvedSuggestion(suggestion: UnresolvedCompletedSuggestion, turnId: string): Suggestion {
  return {
    id: suggestion.id,
    text: suggestion.text,
    isComplete: true,
    durability: "durable",
    correlationId: suggestion.correlationId,
    turnId,
    eventId: suggestion.eventId,
    sessionId: suggestion.sessionId,
    sequence: suggestion.sequence,
    timestampMs: suggestion.timestampMs,
    payload: suggestion.payload,
  };
}

function hasCompletedSuggestion(state: SessionStoreState, suggestionId: string): boolean {
  return Object.values(state.suggestionsByTurn).some((suggestion) => suggestion.id === suggestionId)
    || suggestionId in state.unresolvedCompletedSuggestionsById;
}

function deriveReplayTurnId(state: SessionStoreState): string | undefined {
  const candidates = state.turns.filter((turn) =>
    turn.speakerRole === "interviewer" && !state.suggestionsByTurn[turn.id]);
  return candidates.length === 1 ? candidates[0].id : undefined;
}

function applyHealth(set: StoreApi<SessionStoreState>["setState"], envelope: Envelope) {
  const dependency = envelope.kind === EventKind.AUDIO_HEALTH
    ? audioHealthKey(stringPayload(envelope.payload, "source"))
    : providerHealthKey(stringPayload(envelope.payload, "provider"));
  if (!dependency) return;
  const status = healthStatus(stringPayload(envelope.payload, "status"));
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
  set({
    session: current.session && state ? { ...current.session, status: state } : current.session,
    languages: {
      ui: current.languages.ui,
      input: stringPayload(envelope.payload, "input_language") ?? current.languages.input,
      response: stringPayload(envelope.payload, "response_language") ?? current.languages.response,
      review: stringPayload(envelope.payload, "review_language") ?? current.languages.review,
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

function audioHealthKey(value: string | undefined): keyof RuntimeHealth | undefined {
  return value === "mic" ? "microphone" : value === "system" ? "systemAudio" : undefined;
}

function providerHealthKey(value: string | undefined): keyof RuntimeHealth | undefined {
  return value === "speech" ? "speechProvider" : value === "model" ? "modelProvider" : undefined;
}

function healthStatus(value: string | undefined): RuntimeHealthStatus {
  switch (value) {
    case "ready":
    case "healthy":
    case "ok": return "ready";
    case "starting":
    case "restarting":
    case "pending": return "pending";
    case "degraded": return "degraded";
    case "failed":
    case "error": return "error";
    case "stopped":
    case "offline":
    case "unavailable": return "offline";
    default: return "unknown";
  }
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
  return leftTimestamp === rightTimestamp ? sequenceOf(left) - sequenceOf(right) : leftTimestamp - rightTimestamp;
}

function timestampOf(value: unknown): number {
  return objectNumber(value, "timestamp_ms") ?? Number.MAX_SAFE_INTEGER;
}

function sequenceOf(value: unknown): number {
  return objectNumber(value, "sequence") ?? Number.MAX_SAFE_INTEGER;
}

function objectNumber(value: unknown, key: string): number | undefined {
  if (typeof value !== "object" || value === null || !(key in value)) return undefined;
  const candidate = (value as Record<string, unknown>)[key];
  return typeof candidate === "number" ? candidate : undefined;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
