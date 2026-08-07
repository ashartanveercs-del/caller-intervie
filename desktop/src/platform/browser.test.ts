import { describe, expect, it } from "vitest";
import { EventKind, type Envelope } from "../shared/protocol";
import { createBrowserPlatform } from "./browser";

const sessionId = "018f0000-0000-7000-8000-000000000001";

function transcript(sequence: number): Envelope {
  return {
    version: 1,
    id: `018f0000-0000-7000-8000-${String(sequence).padStart(12, "0")}`,
    session_id: sessionId,
    sequence,
    timestamp_ms: sequence,
    kind: EventKind.TRANSCRIPT_UPDATED,
    payload: { turn_id: "018f0000-0000-7000-8000-000000000002", text: "Hello", is_final: true },
    correlation_id: null,
  };
}

describe("browser platform", () => {
  it("persists deterministic CRUD state and publishes subscriptions", async () => {
    const platform = createBrowserPlatform();
    const received: Envelope[] = [];
    const unlisten = await platform.subscribe((item) => received.push(item));
    const session = await platform.createSession({
      mode: "interview",
      inputLanguage: "en",
      responseLanguage: "ur",
      reviewLanguage: "en",
    });

    await platform.saveSessionBrief({ sessionId: session.id, brief: { role: "Engineer" } });
    platform.emit(transcript(1));
    await platform.completeSession(session.id, "completed");

    expect(await platform.getSession(session.id)).toMatchObject({
      id: session.id,
      status: "completed",
      brief: { role: "Engineer" },
    });
    expect(await platform.getTimeline(session.id)).toEqual([transcript(1)]);
    expect(received).toEqual([transcript(1)]);
    expect(await platform.listSessions(1)).toHaveLength(1);

    unlisten();
    await platform.deleteSession(session.id);
    await expect(platform.getSession(session.id)).rejects.toThrow(/not found/i);
  });

  it("restores the newest active session when multiple sessions remain active", async () => {
    const platform = createBrowserPlatform();
    await platform.createSession({
      mode: "interview",
      inputLanguage: "en",
      responseLanguage: "en",
      reviewLanguage: "en",
    });
    const second = await platform.createSession({
      mode: "interview",
      inputLanguage: "en",
      responseLanguage: "ur",
      reviewLanguage: "en",
    });
    expect(await platform.restoreActiveSession()).toMatchObject({ id: second.id, status: "active" });
  });

  it("emulates durable associations with defensive clones and rejects conflicts", async () => {
    const platform = createBrowserPlatform();
    const session = await platform.createSession({
      mode: "interview",
      inputLanguage: "en",
      responseLanguage: "ur",
      reviewLanguage: "en",
    });
    const association = {
      sessionId: session.id,
      requestId: "018f0000-0000-7000-8000-000000000010",
      turnId: "018f0000-0000-7000-8000-000000000011",
    };

    await platform.associateRequestWithTurn(association);
    await platform.associateRequestWithTurn(association);
    const restored = await platform.getRequestTurnAssociations(session.id);
    restored[0].turnId = "018f0000-0000-7000-8000-000000000012";

    expect(await platform.getRequestTurnAssociations(session.id)).toEqual([{
      requestId: association.requestId,
      turnId: association.turnId,
    }]);
    await expect(platform.associateRequestWithTurn({ ...association, turnId: "018f0000-0000-7000-8000-000000000012" }))
      .rejects.toThrow(/conflict/i);
    await expect(platform.associateRequestWithTurn({
      ...association,
      sessionId: "018f0000-0000-7000-8000-000000000099",
    }))
      .rejects.toThrow(/not found/i);
  });

  it("scopes the same request independently and cascades associations with session deletion", async () => {
    const platform = createBrowserPlatform();
    const first = await platform.createSession({
      mode: "interview",
      inputLanguage: "en",
      responseLanguage: "en",
      reviewLanguage: "en",
    });
    const second = await platform.createSession({
      mode: "interview",
      inputLanguage: "en",
      responseLanguage: "ur",
      reviewLanguage: "en",
    });
    const requestId = "018f0000-0000-7000-8000-000000000020";
    const firstTurnId = "018f0000-0000-7000-8000-000000000021";
    const secondTurnId = "018f0000-0000-7000-8000-000000000022";

    await platform.associateRequestWithTurn({ sessionId: first.id, requestId, turnId: firstTurnId });
    await platform.associateRequestWithTurn({ sessionId: second.id, requestId, turnId: secondTurnId });

    expect(await platform.getRequestTurnAssociations(first.id)).toEqual([{ requestId, turnId: firstTurnId }]);
    expect(await platform.getRequestTurnAssociations(second.id)).toEqual([{ requestId, turnId: secondTurnId }]);

    await platform.deleteSession(first.id);

    await expect(platform.getRequestTurnAssociations(first.id)).rejects.toThrow(/not found/i);
    expect(await platform.getRequestTurnAssociations(second.id)).toEqual([{ requestId, turnId: secondTurnId }]);
  });

  it("clones initial and subscribed storage health transitions", async () => {
    const platform = createBrowserPlatform();
    const received: unknown[] = [];
    const cleanup = await platform.subscribeStorageHealth((health) => received.push(health));

    const initial = await platform.storageHealth();
    initial.status = "error";
    expect(await platform.storageHealth()).toEqual({ status: "ready", recoverable: false });
    platform.emitStorageHealth({
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    });
    (received[0] as { message?: string }).message = "mutated by listener";
    expect(await platform.storageHealth()).toEqual({
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    });
    platform.emitStorageHealth({ status: "ready", recoverable: false });
    cleanup();

    expect(await platform.storageHealth()).toEqual({ status: "ready", recoverable: false });
    expect(received).toEqual([
      { status: "degraded", code: "storage-unavailable", message: "mutated by listener", recoverable: true },
      { status: "ready", recoverable: false },
    ]);
  });
});
