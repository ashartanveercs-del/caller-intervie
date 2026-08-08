import { describe, expect, it } from "vitest";
import { EventKind, type Envelope } from "../shared/protocol";
import type { StorageHealth } from "../platform";
import { createSessionStore } from "./sessionStore";

const ids = {
  session: "018f0000-0000-7000-8000-000000000001",
  questionOne: "018f0000-0000-7000-8000-000000000002",
  questionTwo: "018f0000-0000-7000-8000-000000000003",
  queryOne: "018f0000-0000-7000-8000-000000000004",
  queryTwo: "018f0000-0000-7000-8000-000000000005",
};

let counter = 10;

function event(
  kind: EventKind,
  payload: Record<string, unknown>,
  overrides: Partial<Envelope> = {},
): Envelope {
  counter += 1;
  return {
    version: 1,
    id: `018f0000-0000-7000-8000-${String(counter).padStart(12, "0")}`,
    session_id: ids.session,
    sequence: counter,
    timestamp_ms: counter,
    kind,
    payload,
    correlation_id: null,
    ...overrides,
  };
}

function question(turnId: string, text: string, sequence: number): Envelope {
  return event(
    EventKind.TRANSCRIPT_UPDATED,
    { turn_id: turnId, text, is_final: true, speech_final: true, speaker_role: "interviewer" },
    { sequence },
  );
}

function activate(store: ReturnType<typeof createSessionStore>, id = ids.session) {
  store.getState().beginSession({
    id,
    mode: "interview",
    status: "active",
    inputLanguage: "en",
    responseLanguage: "ur",
    reviewLanguage: "en",
  });
}

describe("session store", () => {
  it("ignores duplicate event ids and stale sequences within one sidecar generation", () => {
    const store = createSessionStore();
    activate(store);
    const first = question(ids.questionOne, "First", 4);

    store.getState().applyEnvelope(first);
    store.getState().applyEnvelope({ ...first, payload: { ...first.payload, text: "Duplicate" } });
    store.getState().applyEnvelope(question(ids.questionTwo, "Stale", 3));

    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["First"]);
  });

  it("accepts a new sidecar generation after a restart even when its sequence starts over", () => {
    const store = createSessionStore();
    activate(store);

    store.getState().applyEnvelope(question(ids.questionOne, "Before restart", 9));
    store.getState().clearTransientState();
    store.getState().applyEnvelope(
      event(EventKind.SIDECAR_READY, { status: "ready" }, { session_id: null, sequence: 0 }),
    );
    store.getState().applyEnvelope(question(ids.questionTwo, "After restart", 1));

    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["Before restart", "After restart"]);
  });

  it("replaces a partial transcript and commits the final transcript", () => {
    const store = createSessionStore();
    activate(store);

    store.getState().applyEnvelope(
      event(EventKind.TRANSCRIPT_UPDATED, {
        turn_id: ids.questionOne,
        text: "How would you",
        is_final: false,
        speech_final: false,
        speaker_role: "interviewer",
      }),
    );
    store.getState().applyEnvelope(
      event(EventKind.TRANSCRIPT_UPDATED, {
        turn_id: ids.questionOne,
        text: "How would you design this?",
        is_final: true,
        speech_final: true,
        speaker_role: "interviewer",
      }),
    );

    expect(store.getState().partialTurnsById[ids.questionOne]).toBeUndefined();
    expect(store.getState().turns).toMatchObject([
      { id: ids.questionOne, text: "How would you design this?", isFinal: true },
    ]);
  });

  it("binds concurrent suggestion correlations to their intended question turns", () => {
    const store = createSessionStore();
    activate(store);
    store.getState().applyEnvelope(question(ids.questionOne, "First question", 1));
    store.getState().applyEnvelope(question(ids.questionTwo, "Second question", 2));
    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionOne);
    store.getState().associateRequestWithTurn(ids.queryTwo, ids.questionTwo);

    store.getState().applyEnvelope(
      event(EventKind.SUGGESTION_COMPLETED, { suggestion_id: "suggestion-two", text: "Second answer" }, {
        correlation_id: ids.queryTwo,
        sequence: 3,
      }),
    );
    store.getState().applyEnvelope(
      event(EventKind.SUGGESTION_COMPLETED, { suggestion_id: "suggestion-one", text: "First answer" }, {
        correlation_id: ids.queryOne,
        sequence: 4,
      }),
    );

    expect(store.getState().suggestionsByTurn).toMatchObject({
      [ids.questionOne]: { text: "First answer", isComplete: true },
      [ids.questionTwo]: { text: "Second answer", isComplete: true },
    });
  });

  it("retains a completed answer when another question arrives", () => {
    const store = createSessionStore();
    activate(store);
    store.getState().applyEnvelope(question(ids.questionOne, "First question", 1));
    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionOne);
    store.getState().applyEnvelope(
      event(EventKind.SUGGESTION_COMPLETED, { suggestion_id: "suggestion-one", text: "First answer" }, {
        correlation_id: ids.queryOne,
        sequence: 2,
      }),
    );
    store.getState().applyEnvelope(question(ids.questionTwo, "Second question", 3));

    expect(store.getState().suggestionsByTurn[ids.questionOne]).toMatchObject({
      text: "First answer",
      isComplete: true,
    });
  });

  it("tracks health and keeps each language independently configurable", () => {
    const store = createSessionStore();

    store.getState().setLanguages({ ui: "ar", input: "auto", response: "ur", review: "en" });
    store.getState().applyEnvelope(event(EventKind.AUDIO_HEALTH, {
      source: "mic",
      status: "degraded",
      message: "Muted",
    }, { session_id: null }));
    store.getState().applyEnvelope(event(EventKind.PROVIDER_HEALTH, {
      provider: "model",
      status: "ready",
      message: null,
    }, { session_id: null }));

    expect(store.getState().languages).toEqual({ ui: "ar", input: "auto", response: "ur", review: "en" });
    expect(store.getState().health.microphone).toMatchObject({ status: "degraded", message: "Muted" });
    expect(store.getState().health.modelProvider).toMatchObject({ status: "ready" });
  });

  it("tracks host storage health from pending through recovery", () => {
    const store = createSessionStore();

    expect(store.getState().health.storage).toEqual({ status: "pending", recoverable: false });
    store.getState().setStorageHealth({
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    });
    expect(store.getState().health.storage).toEqual({
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    });

    store.getState().setStorageHealth({ status: "ready", recoverable: false });

    expect(store.getState().health.storage).toEqual({ status: "ready", recoverable: false });
  });

  it.each([
    ["stopped", "completed"],
    ["error", "interrupted"],
  ] as const)("maps live runtime state %s to persisted status %s and cancels restore", (runtimeState, status) => {
    const store = createSessionStore();
    activate(store);
    store.getState().restoreSession(store.getState().session!);
    expect(store.getState().isRestoringSession).toBe(true);

    store.getState().applyEnvelope(event(EventKind.SESSION_STATE, {
      state: runtimeState,
      input_language: "en",
      response_language: "ur",
      review_language: "en",
    }));

    expect(store.getState().session?.status).toBe(status);
    expect(store.getState().isRestoringSession).toBe(false);
  });

  it.each(["listening", "paused"])(
    "keeps persisted status active and restore open for nonterminal runtime state %s",
    (runtimeState) => {
      const store = createSessionStore();
      activate(store);
      store.getState().restoreSession(store.getState().session!);

      store.getState().applyEnvelope(event(EventKind.SESSION_STATE, {
        state: runtimeState,
        input_language: "en",
        response_language: "ur",
        review_language: "en",
      }));

      expect(store.getState().session?.status).toBe("active");
      expect(store.getState().isRestoringSession).toBe(true);
    },
  );

  it.each([
    { status: "ready", recoverable: false },
    {
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    },
    {
      status: "error",
      code: "storage-corrupt",
      message: "Storage requires repair",
      recoverable: false,
    },
  ] satisfies StorageHealth[])("preserves $status storage health across a sidecar restart", (storageHealth) => {
    const store = createSessionStore();
    store.getState().setStorageHealth(storageHealth);

    store.getState().clearTransientState();

    expect(store.getState().health.storage).toEqual(storageHealth);
    expect(store.getState().health.sidecar).toEqual({ status: "pending" });
  });

  it("clears only transient state after a sidecar restart", () => {
    const store = createSessionStore();
    store.getState().restoreSession({
      id: ids.session,
      mode: "interview",
      status: "active",
      inputLanguage: "en",
      responseLanguage: "ur",
      reviewLanguage: "en",
      brief: { role: "Engineer" },
    });
    store.getState().applyEnvelope(question(ids.questionOne, "Durable question", 1));
    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionOne);
    store.getState().applyEnvelope(
      event(EventKind.SUGGESTION_COMPLETED, { suggestion_id: "suggestion-one", text: "Durable answer" }, {
        correlation_id: ids.queryOne,
        sequence: 2,
      }),
    );
    store.getState().applyEnvelope(
      event(EventKind.SUGGESTION_CHUNK, { suggestion_id: "suggestion-two", text: "Transient" }, {
        correlation_id: ids.queryTwo,
        sequence: 3,
      }),
    );

    store.getState().clearTransientState();

    expect(store.getState().session?.brief).toEqual({ role: "Engineer" });
    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["Durable question"]);
    expect(store.getState().suggestionsByTurn[ids.questionOne]?.text).toBe("Durable answer");
    expect(store.getState().unresolvedSuggestionCorrelations).toEqual([]);
  });

  it("ignores duplicate replay within the same materialized session and records ambiguous associations", () => {
    const store = createSessionStore();
    activate(store);
    const first = question(ids.questionOne, "First question", 1);
    const second = question(ids.questionTwo, "Second question", 2);
    const answer = event(
      EventKind.SUGGESTION_COMPLETED,
      { suggestion_id: "suggestion-one", text: "Answer" },
      { correlation_id: ids.queryOne, sequence: 3 },
    );

    store.getState().restoreReplay([first, second, answer]);
    store.getState().restoreReplay([first, second, answer]);

    expect(store.getState().turns).toHaveLength(2);
    expect(store.getState().unresolvedSuggestionCorrelations).toEqual([ids.queryOne]);
  });

  it("keeps a persisted completed suggestion unresolved without a durable association", () => {
    const store = createSessionStore();
    activate(store);
    const turn = question(ids.questionOne, "Only question", 1);
    const answer = event(
      EventKind.SUGGESTION_COMPLETED,
      { suggestion_id: "suggestion-unmapped", text: "Unmapped answer" },
      { correlation_id: ids.queryOne, sequence: 2 },
    );

    store.getState().restoreReplay([turn, answer]);

    expect(store.getState().suggestionsByTurn).toEqual({});
    expect(store.getState().unresolvedCompletedSuggestionsById["suggestion-unmapped"]).toMatchObject({
      correlationId: ids.queryOne,
      text: "Unmapped answer",
    });
  });

  it("replays timeline envelopes in the host-provided order", () => {
    const store = createSessionStore();
    activate(store);
    const firstFromHost = question(ids.questionOne, "First from host", 2);
    const secondFromHost = question(ids.questionTwo, "Second from host", 1);

    store.getState().restoreReplay([
      { ...firstFromHost, timestamp_ms: 200 },
      { ...secondFromHost, timestamp_ms: 100 },
    ]);

    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["First from host", "Second from host"]);
  });

  it("keeps the first durable request mapping when a conflicting replay mapping appears", () => {
    const store = createSessionStore();
    activate(store);

    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionOne);
    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionOne);
    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionTwo);

    expect(store.getState().requestToTurn).toEqual({ [ids.queryOne]: ids.questionOne });
    expect(store.getState().lastError).toMatch(/conflict/i);
  });

  it("rejects malformed envelopes without corrupting state", () => {
    const store = createSessionStore();
    activate(store);
    store.getState().applyEnvelope(question(ids.questionOne, "First", 1));

    store.getState().applyEnvelope({ id: "not-an-envelope" } as unknown as Envelope);

    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["First"]);
    expect(store.getState().lastError).toMatch(/invalid/i);
  });

  it.each([
    ["before", ["ready", "persisted", "live"]],
    ["during", ["persisted", "ready", "persisted", "live"]],
    ["after", ["persisted", "ready", "live"]],
  ])("keeps the live sequence watermark isolated when ready arrives %s replay", (_position, order) => {
    const store = createSessionStore();
    activate(store);
    const persistedOne = question(ids.questionOne, "Persisted one", 40);
    const persistedTwo = question(ids.questionTwo, "Persisted two", 41);
    const ready = event(EventKind.SIDECAR_READY, { status: "ready" }, { session_id: null, sequence: 0 });
    const live = question(ids.questionTwo, "Live sequence one", 1);
    let persistedCount = 0;

    for (const step of order) {
      if (step === "ready") store.getState().applyEnvelope(ready);
      if (step === "persisted") {
        store.getState().applyPersistedEnvelope(persistedCount++ === 0 ? persistedOne : persistedTwo);
      }
      if (step === "live") store.getState().applyEnvelope(live);
    }

    expect(store.getState().turns.map((turn) => turn.text)).toContain("Live sequence one");
    expect(store.getState().lastSequence).toBe(1);
  });

  it("accumulates delta chunks and replaces them with the completed suggestion text", () => {
    const store = createSessionStore();
    activate(store);
    store.getState().applyEnvelope(question(ids.questionOne, "Question", 1));
    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionOne);

    store.getState().applyEnvelope(event(EventKind.SUGGESTION_CHUNK, {
      suggestion_id: "018f0000-0000-7000-8000-000000000021",
      text: "First ",
    }, { correlation_id: ids.queryOne, sequence: 2 }));
    store.getState().applyEnvelope(event(EventKind.SUGGESTION_CHUNK, {
      suggestion_id: "018f0000-0000-7000-8000-000000000021",
      text: "second",
    }, { correlation_id: ids.queryOne, sequence: 3 }));

    expect(store.getState().partialSuggestionsById["018f0000-0000-7000-8000-000000000021"]?.text).toBe("First second");

    store.getState().applyEnvelope(event(EventKind.SUGGESTION_COMPLETED, {
      suggestion_id: "018f0000-0000-7000-8000-000000000021",
      text: "Authoritative final answer",
    }, { correlation_id: ids.queryOne, sequence: 4 }));

    expect(store.getState().partialSuggestionsById["018f0000-0000-7000-8000-000000000021"]).toBeUndefined();
    expect(store.getState().suggestionsByTurn[ids.questionOne]).toMatchObject({
      text: "Authoritative final answer",
      durability: "durable",
    });
  });

  it("retains unresolved completed suggestions and resolves them when a request association arrives", () => {
    const store = createSessionStore();
    activate(store);
    store.getState().applyEnvelope(question(ids.questionOne, "Question", 1));
    const suggestionId = "018f0000-0000-7000-8000-000000000022";
    store.getState().applyEnvelope(event(EventKind.SUGGESTION_CHUNK, {
      suggestion_id: suggestionId,
      text: "Partial ",
    }, { correlation_id: ids.queryOne, sequence: 2 }));
    store.getState().applyEnvelope(event(EventKind.SUGGESTION_COMPLETED, {
      suggestion_id: suggestionId,
      text: "Completed answer",
    }, { correlation_id: ids.queryOne, sequence: 3 }));

    expect(store.getState().unresolvedCompletedSuggestionsById[suggestionId]).toMatchObject({
      text: "Completed answer",
      correlationId: ids.queryOne,
      payload: { suggestion_id: suggestionId, text: "Completed answer" },
    });

    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionOne);

    expect(store.getState().suggestionsByTurn[ids.questionOne]).toMatchObject({ text: "Completed answer" });
    expect(store.getState().unresolvedCompletedSuggestionsById[suggestionId]).toBeUndefined();
    expect(store.getState().partialSuggestionsById[suggestionId]).toBeUndefined();
  });

  it("clears associated incomplete suggestions while preserving only completed durable suggestions", () => {
    const store = createSessionStore();
    activate(store);
    store.getState().applyEnvelope(question(ids.questionOne, "Question", 1));
    store.getState().associateRequestWithTurn(ids.queryOne, ids.questionOne);
    store.getState().applyEnvelope(event(EventKind.SUGGESTION_CHUNK, {
      suggestion_id: "018f0000-0000-7000-8000-000000000023",
      text: "Partial",
    }, { correlation_id: ids.queryOne, sequence: 2 }));

    store.getState().clearTransientState();

    expect(store.getState().partialSuggestionsById).toEqual({});
    expect(store.getState().suggestionsByTurn).toEqual({});
  });

  it("maps canonical audio and provider health payloads to their dedicated dependencies", () => {
    const store = createSessionStore();

    store.getState().applyEnvelope(event(EventKind.AUDIO_HEALTH, {
      source: "system",
      status: "ready",
      message: null,
    }, { session_id: null, sequence: 1 }));
    store.getState().applyEnvelope(event(EventKind.PROVIDER_HEALTH, {
      provider: "speech",
      status: "degraded",
      message: "Reconnect",
    }, { session_id: null, sequence: 2 }));

    expect(store.getState().health.systemAudio).toMatchObject({ status: "ready" });
    expect(store.getState().health.speechProvider).toMatchObject({ status: "degraded", message: "Reconnect" });
  });

  it("ignores a late session A envelope while session B is active", () => {
    const store = createSessionStore();
    const sessionB = "018f0000-0000-7000-8000-000000000024";
    activate(store, ids.session);
    store.getState().applyEnvelope(question(ids.questionOne, "Session A", 1));
    activate(store, sessionB);

    store.getState().applyEnvelope(question(ids.questionTwo, "Late session A", 2));
    store.getState().applyEnvelope(event(EventKind.TRANSCRIPT_UPDATED, {
      turn_id: ids.questionTwo,
      text: "Session B",
      is_final: true,
      speech_final: true,
      speaker_role: "interviewer",
    }, { session_id: sessionB, sequence: 3 }));

    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["Session B"]);
    expect(store.getState().session?.id).toBe(sessionB);
  });

  it("replays session A again after materializing session B", () => {
    const store = createSessionStore();
    const sessionA = ids.session;
    const sessionB = "018f0000-0000-7000-8000-000000000025";
    const timelineA = [
      question(ids.questionOne, "A first turn", 40),
      question(ids.questionTwo, "A second turn", 41),
    ];

    store.getState().restoreSession({
      id: sessionA,
      mode: "interview",
      status: "active",
      inputLanguage: "en",
      responseLanguage: "ur",
      reviewLanguage: "en",
    });
    store.getState().restoreReplay(timelineA);
    activate(store, sessionB);
    store.getState().restoreSession({
      id: sessionA,
      mode: "interview",
      status: "active",
      inputLanguage: "en",
      responseLanguage: "ur",
      reviewLanguage: "en",
    });
    store.getState().restoreReplay(timelineA);

    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["A first turn", "A second turn"]);
  });

  it("keeps live ready dedupe across a session switch and resets only for a new generation", () => {
    const store = createSessionStore();
    const firstReady = event(EventKind.SIDECAR_READY, { status: "ready" }, {
      session_id: null,
      sequence: 0,
    });
    const liveSequenceTen = event(EventKind.AUDIO_HEALTH, {
      source: "mic",
      status: "ready",
      message: null,
    }, { session_id: null, sequence: 10 });

    store.getState().applyEnvelope(firstReady);
    store.getState().applyEnvelope(liveSequenceTen);
    activate(store);
    store.getState().applyEnvelope(firstReady);
    for (let sequence = 1; sequence <= 10; sequence += 1) {
      store.getState().applyEnvelope(question(ids.questionOne, `Stale ${sequence}`, sequence));
    }

    expect(store.getState().sidecarGeneration).toBe(1);
    expect(store.getState().lastSequence).toBe(10);
    expect(store.getState().turns).toEqual([]);

    store.getState().applyEnvelope(event(EventKind.SIDECAR_READY, { status: "ready" }, {
      session_id: null,
      sequence: 0,
    }));
    store.getState().applyEnvelope(question(ids.questionOne, "New generation", 1));

    expect(store.getState().sidecarGeneration).toBe(2);
    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["New generation"]);
    expect(store.getState().lastSequence).toBe(1);
  });
});
