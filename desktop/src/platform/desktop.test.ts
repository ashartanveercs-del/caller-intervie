import { beforeEach, describe, expect, it, vi } from "vitest";
import { CommandKind, type Envelope } from "../shared/protocol";

const { invoke, listen } = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));

import { createDesktopPlatform } from "./desktop";

const sessionId = "018f0000-0000-7000-8000-000000000041";
const command: Envelope = {
  version: 1,
  id: "018f0000-0000-7000-8000-000000000042",
  session_id: sessionId,
  sequence: 1,
  timestamp_ms: 1,
  kind: CommandKind.SESSION_STOP,
  payload: {},
  correlation_id: null,
};

const sessionDto = {
  id: sessionId,
  mode: "interview",
  status: "active",
  input_language: "en",
  response_language: "ur",
  review_language: "en",
};

describe("desktop platform", () => {
  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
  });

  it("uses the exact sidecar command names and command envelope argument", async () => {
    invoke.mockResolvedValueOnce({ state: "ready", restartCount: 2, diagnostics: [] });
    invoke.mockResolvedValueOnce(undefined);
    invoke.mockResolvedValueOnce({ state: "ready", restartCount: 3, diagnostics: [] });
    const platform = createDesktopPlatform();

    await platform.sidecarStatus();
    await platform.send(command);
    await platform.restartSidecar();

    expect(invoke).toHaveBeenNthCalledWith(1, "sidecar_status");
    expect(invoke).toHaveBeenNthCalledWith(2, "send_sidecar_command", { command });
    expect(invoke).toHaveBeenNthCalledWith(3, "restart_sidecar");
  });

  it("maps nullable capture protection DTOs and invokes the exact capture commands", async () => {
    invoke
      .mockResolvedValueOnce({
        state: "unavailable",
        code: "capture_protection_unavailable",
        message: "Screen capture protection could not be confirmed.",
        internal_diagnostics: "must-not-leak",
      })
      .mockResolvedValueOnce({
        state: "protected",
        code: null,
        message: null,
        internal_diagnostics: "must-not-leak",
      });
    const platform = createDesktopPlatform();

    expect(await platform.captureProtectionStatus()).toEqual({
      state: "unavailable",
      code: "capture_protection_unavailable",
      message: "Screen capture protection could not be confirmed.",
    });
    expect(await platform.retryCaptureProtection()).toEqual({ state: "protected" });

    expect(invoke.mock.calls).toEqual([
      ["capture_protection_status"],
      ["retry_capture_protection"],
    ]);
  });

  it("maps capture protection status events and exposes Tauri listener cleanup", async () => {
    const unlisten = vi.fn();
    let callback: ((event: { payload: unknown }) => void) | undefined;
    listen.mockImplementationOnce(async (_event, handler) => {
      callback = handler;
      return unlisten;
    });
    const platform = createDesktopPlatform();
    const received: unknown[] = [];

    const cleanup = await platform.subscribeCaptureProtection((status) => received.push(status));
    callback?.({ payload: {
      state: "unavailable",
      code: "capture_protection_unavailable",
      message: "Screen capture protection could not be confirmed.",
      internal_diagnostics: "must-not-leak",
    } });
    cleanup();

    expect(listen).toHaveBeenCalledWith("capture-protection://status", expect.any(Function));
    expect(received).toEqual([{
      state: "unavailable",
      code: "capture_protection_unavailable",
      message: "Screen capture protection could not be confirmed.",
    }]);
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("forwards valid sidecar events and exposes Tauri listener cleanup", async () => {
    const unlisten = vi.fn();
    let callback: ((event: { payload: unknown }) => void) | undefined;
    listen.mockImplementationOnce(async (_event, handler) => {
      callback = handler;
      return unlisten;
    });
    const received: Envelope[] = [];
    const platform = createDesktopPlatform();

    const cleanup = await platform.subscribe((event) => received.push(event));
    callback?.({ payload: command });
    cleanup();

    expect(listen).toHaveBeenCalledWith("sidecar://event", expect.any(Function));
    expect(received).toEqual([command]);
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("uses durable association DTOs and redacts storage health events", async () => {
    const unlisten = vi.fn();
    let callback: ((event: { payload: unknown }) => void) | undefined;
    listen.mockImplementationOnce(async (_event, handler) => {
      callback = handler;
      return unlisten;
    });
    invoke
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce([{ request_id: command.id, turn_id: sessionId }])
      .mockResolvedValueOnce({ status: "ready", recoverable: false, internal_path: "must-not-leak" });
    const platform = createDesktopPlatform();
    const received: unknown[] = [];

    await platform.associateRequestWithTurn({ sessionId, requestId: command.id, turnId: sessionId });
    expect(await platform.getRequestTurnAssociations(sessionId)).toEqual([{ requestId: command.id, turnId: sessionId }]);
    expect(await platform.storageHealth()).toEqual({ status: "ready", recoverable: false });
    const cleanup = await platform.subscribeStorageHealth((health) => received.push(health));
    callback?.({ payload: {
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
      raw_database_error: "must-not-leak",
    } });
    cleanup();

    expect(invoke.mock.calls).toEqual([
      ["associate_request_with_turn", { input: { session_id: sessionId, request_id: command.id, turn_id: sessionId } }],
      ["get_request_turn_associations", { sessionId }],
      ["storage_health"],
    ]);
    expect(listen).toHaveBeenCalledWith("storage://health", expect.any(Function));
    expect(received).toEqual([{
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    }]);
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("keeps storage DTO argument shapes localized to native invokes", async () => {
    invoke
      .mockResolvedValueOnce(sessionDto)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce([sessionDto])
      .mockResolvedValueOnce(sessionDto)
      .mockResolvedValueOnce([command])
      .mockResolvedValueOnce(sessionDto)
      .mockResolvedValueOnce(undefined);
    const platform = createDesktopPlatform();

    await platform.createSession({ mode: "interview", uiLanguage: "ar", inputLanguage: "auto", responseLanguage: "ur", reviewLanguage: "en" });
    await platform.saveSessionBrief({ sessionId, brief: { role: "Engineer" } });
    await platform.completeSession(sessionId, "completed");
    await platform.listSessions(5);
    await platform.getSession(sessionId);
    await platform.getTimeline(sessionId);
    await platform.restoreActiveSession();
    await platform.deleteSession(sessionId);

    expect(invoke.mock.calls).toEqual([
      ["create_session", { input: { mode: "interview", ui_language: "ar", input_language: "auto", response_language: "ur", review_language: "en" } }],
      ["save_session_brief", { input: { session_id: sessionId, brief: { role: "Engineer" } } }],
      ["complete_session", { sessionId, status: "completed" }],
      ["list_sessions", { limit: 5 }],
      ["get_session", { sessionId }],
      ["get_timeline", { sessionId }],
      ["restore_active_session"],
      ["delete_session", { sessionId }],
    ]);
  });
});
