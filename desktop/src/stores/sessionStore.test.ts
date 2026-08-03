import { describe, expect, it } from "vitest";
import { EventKind, type Envelope } from "../shared/protocol";
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

describe("session store", () => {
  it("ignores duplicate event ids and stale sequences within one sidecar generation", () => {
    const store = createSessionStore();
    const first = question(ids.questionOne, "First", 4);

    store.getState().applyEnvelope(first);
    store.getState().applyEnvelope({ ...first, payload: { ...first.payload, text: "Duplicate" } });
    store.getState().applyEnvelope(question(ids.questionTwo, "Stale", 3));

    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["First"]);
  });

  it("accepts a new sidecar generation after a restart even when its sequence starts over", () => {
    const store = createSessionStore();

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
    store.getState().applyEnvelope(
      event(EventKind.AUDIO_HEALTH, { dependency: "microphone", status: "degraded", message: "Muted" }),
    );
    store.getState().applyEnvelope(
      event(EventKind.PROVIDER_HEALTH, { dependency: "model_provider", status: "ready" }),
    );

    expect(store.getState().languages).toEqual({ ui: "ar", input: "auto", response: "ur", review: "en" });
    expect(store.getState().health.microphone).toMatchObject({ status: "degraded", message: "Muted" });
    expect(store.getState().health.modelProvider).toMatchObject({ status: "ready" });
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

  it("restores replayed events idempotently and records only ambiguous replay associations", () => {
    const store = createSessionStore();
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

  it("rejects malformed envelopes without corrupting state", () => {
    const store = createSessionStore();
    store.getState().applyEnvelope(question(ids.questionOne, "First", 1));

    store.getState().applyEnvelope({ id: "not-an-envelope" } as unknown as Envelope);

    expect(store.getState().turns.map((turn) => turn.text)).toEqual(["First"]);
    expect(store.getState().lastError).toMatch(/invalid/i);
  });
});
