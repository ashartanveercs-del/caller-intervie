import { describe, expect, it } from "vitest";
import { decodeEnvelope, EventKind, MAX_SAFE_INTEGER, PROTOCOL_VERSION } from "./protocol";
import envelopeSchema from "../../../protocol/v1/envelope.schema.json";
import readyFixture from "../../../protocol/v1/fixtures/sidecar-ready.json";
import sessionStartFixture from "../../../protocol/v1/fixtures/session-start.json";
import transcriptFixture from "../../../protocol/v1/fixtures/transcript-final.json";
import suggestionFixture from "../../../protocol/v1/fixtures/suggestion-complete.json";
import activeSessionStateFixture from "../../../protocol/v1/fixtures/session-state-active.json";
import idleSessionStateFixture from "../../../protocol/v1/fixtures/session-state-idle.json";

describe("Protocol V1", () => {
  it("pins the JSON Schema to protocol version one", () => {
    expect(envelopeSchema.properties.version).toMatchObject({
      const: 1,
      minimum: 0,
      maximum: 65535,
    });
  });

  it("round trips the command fixture", () => {
    expect(decodeEnvelope(sessionStartFixture)).toEqual(sessionStartFixture);
  });

  it("round trips the event fixtures", () => {
    for (const fixture of [
      readyFixture,
      transcriptFixture,
      suggestionFixture,
      idleSessionStateFixture,
      activeSessionStateFixture,
    ]) {
      expect(decodeEnvelope(fixture)).toEqual(fixture);
    }
  });

  it("exports protocol constants and kinds", () => {
    expect(PROTOCOL_VERSION).toBe(1);
    expect(decodeEnvelope(readyFixture).kind).toBe(EventKind.SIDECAR_READY);
  });

  it("rejects an unsupported protocol version", () => {
    expect(() => decodeEnvelope({ ...readyFixture, version: 2 })).toThrow(/protocol version/i);
  });

  it("preserves unknown payload fields", () => {
    const decoded = decodeEnvelope({
      ...readyFixture,
      payload: { ...readyFixture.payload, future_extension: { enabled: true } },
    });
    expect(decoded.payload).toEqual({ status: "ready", future_extension: { enabled: true } });
  });

  it("rejects malformed envelope fields", () => {
    expect(() => decodeEnvelope({ ...readyFixture, id: "not-a-uuid" })).toThrow(/uuid/i);
    expect(() => decodeEnvelope({ ...readyFixture, sequence: -1 })).toThrow(/nonnegative/i);
    expect(() => decodeEnvelope({ ...readyFixture, kind: "not-namespaced" })).toThrow(/kind/i);
    expect(() => decodeEnvelope({ ...readyFixture, payload: [] })).toThrow(/payload/i);
  });

  it("accepts the shared safe integer boundary", () => {
    expect(MAX_SAFE_INTEGER).toBe(Number.MAX_SAFE_INTEGER);
    expect(
      decodeEnvelope({
        ...readyFixture,
        sequence: MAX_SAFE_INTEGER,
        timestamp_ms: MAX_SAFE_INTEGER,
      }),
    ).toMatchObject({ sequence: MAX_SAFE_INTEGER, timestamp_ms: MAX_SAFE_INTEGER });
  });

  it("rejects values above the shared safe integer boundary", () => {
    expect(() =>
      decodeEnvelope({
        ...readyFixture,
        sequence: Number.MAX_SAFE_INTEGER + 1,
        timestamp_ms: Number.MAX_SAFE_INTEGER + 1,
      }),
    ).toThrow(/safe integer/i);
  });
});
