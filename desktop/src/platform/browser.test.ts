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

  it("restores the newest incomplete session", async () => {
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
    await platform.completeSession(first.id, "completed");

    expect(await platform.restoreActiveSession()).toMatchObject({ id: second.id, status: "active" });
  });
});
