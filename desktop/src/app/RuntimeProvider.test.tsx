import { StrictMode } from "react";
import { act, render, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EventKind, type Envelope } from "../shared/protocol";
import { CommandKind } from "../shared/protocol";
import type { PlatformApi, SessionRecord, SidecarStatus, StorageHealth } from "../platform";
import { RuntimeProvider, useRuntime } from "./RuntimeProvider";

const sessionId = "018f0000-0000-7000-8000-000000000001";

function status(state: SidecarStatus["state"] = "ready"): SidecarStatus {
  return { state, restartCount: 0, diagnostics: [] };
}

function fakePlatform(): PlatformApi & {
  emit(event: Envelope): void;
  emitStorageHealth(health: StorageHealth): void;
  unlisten: () => void;
  storageUnlisten: () => void;
} {
  let listener: ((event: Envelope) => void) | undefined;
  let storageListener: ((health: StorageHealth) => void) | undefined;
  const unlisten: () => void = vi.fn(() => {
    listener = undefined;
  });
  const storageUnlisten: () => void = vi.fn(() => {
    storageListener = undefined;
  });
  return {
    sidecarStatus: vi.fn().mockResolvedValue(status()),
    storageHealth: vi.fn().mockResolvedValue({ status: "ready", recoverable: false }),
    send: vi.fn().mockResolvedValue(undefined),
    associateRequestWithTurn: vi.fn().mockResolvedValue(undefined),
    getRequestTurnAssociations: vi.fn().mockResolvedValue([]),
    restartSidecar: vi.fn().mockResolvedValue(status()),
    subscribe: vi.fn().mockImplementation(async (next) => {
      listener = next;
      return unlisten;
    }),
    subscribeStorageHealth: vi.fn().mockImplementation(async (next) => {
      storageListener = next;
      return storageUnlisten;
    }),
    createSession: vi.fn(),
    saveSessionBrief: vi.fn(),
    completeSession: vi.fn(),
    listSessions: vi.fn(),
    getSession: vi.fn(),
    getTimeline: vi.fn(),
    restoreActiveSession: vi.fn().mockResolvedValue(null),
    deleteSession: vi.fn(),
    emit(event) {
      listener?.(event);
    },
    emitStorageHealth(health) {
      storageListener?.(health);
    },
    unlisten,
    storageUnlisten,
  };
}

function Consumer() {
  const runtime = useRuntime();
  return <button onClick={() => void runtime.restart()}>restart</button>;
}

function session(id: string): SessionRecord {
  return {
    id,
    mode: "interview",
    status: "active",
    inputLanguage: "en",
    responseLanguage: "ur",
    reviewLanguage: "en",
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

function SessionStarter({ id, onRuntime }: { id: string; onRuntime(runtime: ReturnType<typeof useRuntime>): void }) {
  const runtime = useRuntime();
  onRuntime(runtime);
  return <button onClick={() => runtime.store.getState().beginSession(session(id))}>start session</button>;
}

function query(sessionId: string, requestId = "018f0000-0000-7000-8000-000000000035"): Envelope {
  return {
    version: 1,
    id: requestId,
    session_id: sessionId,
    sequence: 1,
    timestamp_ms: 1,
    kind: CommandKind.QUERY_TRIGGER,
    payload: {},
    correlation_id: null,
  };
}

describe("RuntimeProvider", () => {
  it("subscribes and unsubscribes exactly once under StrictMode", async () => {
    const platform = fakePlatform();
    let runtime: ReturnType<typeof useRuntime> | undefined;
    const view = render(
      <StrictMode>
        <RuntimeProvider platform={platform}>
          <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
        </RuntimeProvider>
      </StrictMode>,
    );

    await waitFor(() => expect(platform.subscribe).toHaveBeenCalledTimes(1));
    act(() => platform.emitStorageHealth({
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    }));
    expect(runtime?.store.getState().health.storage).toMatchObject({ status: "degraded", recoverable: true });
    view.unmount();

    expect(platform.sidecarStatus).toHaveBeenCalledTimes(1);
    expect(platform.storageHealth).toHaveBeenCalledTimes(1);
    expect(platform.restoreActiveSession).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(platform.unlisten).toHaveBeenCalledTimes(1));
    expect(platform.subscribeStorageHealth).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(platform.storageUnlisten).toHaveBeenCalledTimes(1));
  });

  it("loads initial storage health and applies degraded-to-ready subscription recovery", async () => {
    const platform = fakePlatform();
    let runtime: ReturnType<typeof useRuntime> | undefined;
    render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );

    await waitFor(() => expect(platform.storageHealth).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(platform.subscribeStorageHealth).toHaveBeenCalledTimes(1));
    expect(runtime?.store.getState().health.storage).toEqual({ status: "ready", recoverable: false });

    act(() => platform.emitStorageHealth({
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    }));
    expect(runtime?.store.getState().health.storage).toMatchObject({ status: "degraded", recoverable: true });

    act(() => platform.emitStorageHealth({ status: "ready", recoverable: false }));
    expect(runtime?.store.getState().health.storage).toEqual({ status: "ready", recoverable: false });
  });

  it("persists the request association before sending a query and keeps concurrent completions bound", async () => {
    const platform = fakePlatform();
    let runtime: ReturnType<typeof useRuntime> | undefined;
    render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );
    await waitFor(() => expect(platform.subscribe).toHaveBeenCalledTimes(1));
    runtime?.store.getState().beginSession(session(sessionId));
    const requestOne = query(sessionId, "018f0000-0000-7000-8000-000000000036");
    const requestTwo = query(sessionId, "018f0000-0000-7000-8000-000000000037");

    await Promise.all([
      runtime!.send(requestOne, "018f0000-0000-7000-8000-000000000038"),
      runtime!.send(requestTwo, "018f0000-0000-7000-8000-000000000039"),
    ]);
    platform.emit({
      version: 1,
      id: "018f0000-0000-7000-8000-000000000040",
      session_id: sessionId,
      sequence: 1,
      timestamp_ms: 1,
      kind: EventKind.SUGGESTION_COMPLETED,
      payload: { suggestion_id: "suggestion-two", text: "Second answer" },
      correlation_id: requestTwo.id,
    });
    platform.emit({
      version: 1,
      id: "018f0000-0000-7000-8000-000000000041",
      session_id: sessionId,
      sequence: 2,
      timestamp_ms: 2,
      kind: EventKind.SUGGESTION_COMPLETED,
      payload: { suggestion_id: "suggestion-one", text: "First answer" },
      correlation_id: requestOne.id,
    });

    expect(platform.associateRequestWithTurn).toHaveBeenNthCalledWith(1, {
      sessionId,
      requestId: requestOne.id,
      turnId: "018f0000-0000-7000-8000-000000000038",
    });
    expect(platform.send).toHaveBeenCalledTimes(2);
    for (const request of [requestOne, requestTwo]) {
      const associationIndex = vi.mocked(platform.associateRequestWithTurn).mock.calls
        .findIndex(([input]) => input.requestId === request.id);
      const sendIndex = vi.mocked(platform.send).mock.calls
        .findIndex(([command]) => command.id === request.id);
      expect(associationIndex).toBeGreaterThanOrEqual(0);
      expect(sendIndex).toBeGreaterThanOrEqual(0);
      expect(vi.mocked(platform.associateRequestWithTurn).mock.invocationCallOrder[associationIndex])
        .toBeLessThan(vi.mocked(platform.send).mock.invocationCallOrder[sendIndex]);
    }
    expect(runtime?.store.getState().suggestionsByTurn).toMatchObject({
      "018f0000-0000-7000-8000-000000000038": { text: "First answer" },
      "018f0000-0000-7000-8000-000000000039": { text: "Second answer" },
    });
  });

  it("records association failure without sending the query", async () => {
    const platform = fakePlatform();
    vi.mocked(platform.associateRequestWithTurn).mockRejectedValueOnce(new Error("storage unavailable"));
    let runtime: ReturnType<typeof useRuntime> | undefined;
    render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );
    await waitFor(() => expect(platform.subscribe).toHaveBeenCalledTimes(1));
    runtime?.store.getState().beginSession(session(sessionId));

    await expect(runtime!.send(query(sessionId), "018f0000-0000-7000-8000-000000000038"))
      .rejects.toThrow("storage unavailable");

    expect(platform.send).not.toHaveBeenCalled();
    expect(runtime?.store.getState().lastError).toBe("storage unavailable");
  });

  it("rejects an unassociated query without sending it", async () => {
    const platform = fakePlatform();
    let runtime: ReturnType<typeof useRuntime> | undefined;
    render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );
    await waitFor(() => expect(platform.subscribe).toHaveBeenCalledTimes(1));
    runtime?.store.getState().beginSession(session(sessionId));

    await expect(runtime!.send(query(sessionId))).rejects.toThrow(/question turn/i);

    expect(platform.associateRequestWithTurn).not.toHaveBeenCalled();
    expect(platform.send).not.toHaveBeenCalled();
    expect(runtime?.store.getState().lastError).toMatch(/question turn/i);
  });

  it("applies an initial storage snapshot when an event predates the query", async () => {
    const platform = fakePlatform();
    const subscription = deferred<() => void>();
    const initialHealth = deferred<StorageHealth>();
    let notify: ((health: StorageHealth) => void) | undefined;
    vi.mocked(platform.subscribeStorageHealth).mockImplementation((next) => {
      notify = next;
      return subscription.promise;
    });
    vi.mocked(platform.storageHealth).mockReturnValueOnce(initialHealth.promise);
    let runtime: ReturnType<typeof useRuntime> | undefined;
    render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );
    await waitFor(() => expect(notify).toBeTypeOf("function"));

    act(() => notify!({
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    }));
    subscription.resolve(() => platform.storageUnlisten());
    await waitFor(() => expect(platform.storageHealth).toHaveBeenCalledTimes(1));
    await act(async () => {
      initialHealth.resolve({ status: "ready", recoverable: false });
      await Promise.resolve();
    });

    expect(runtime?.store.getState().health.storage).toEqual({ status: "ready", recoverable: false });
  });

  it("does not let a storage event after the query starts overwrite the subscribed transition", async () => {
    const platform = fakePlatform();
    const initialHealth = deferred<StorageHealth>();
    vi.mocked(platform.storageHealth).mockReturnValueOnce(initialHealth.promise);
    let runtime: ReturnType<typeof useRuntime> | undefined;
    render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );
    await waitFor(() => expect(platform.subscribeStorageHealth).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(platform.storageHealth).toHaveBeenCalledTimes(1));

    act(() => platform.emitStorageHealth({
      status: "degraded",
      code: "storage-unavailable",
      message: "Storage temporarily unavailable",
      recoverable: true,
    }));
    await act(async () => {
      initialHealth.resolve({ status: "ready", recoverable: false });
      await Promise.resolve();
    });

    expect(runtime?.store.getState().health.storage).toMatchObject({
      status: "degraded",
      code: "storage-unavailable",
      recoverable: true,
    });
  });

  it("ignores sidecar and storage callbacks with zero users, including delayed subscription cleanup", async () => {
    const platform = fakePlatform();
    const initialSidecarStatus = deferred<SidecarStatus>();
    const sidecarSubscription = deferred<() => void>();
    const storageSubscription = deferred<() => void>();
    let sidecarListener: ((event: Envelope) => void) | undefined;
    let storageListener: ((health: StorageHealth) => void) | undefined;
    vi.mocked(platform.sidecarStatus).mockReturnValueOnce(initialSidecarStatus.promise);
    vi.mocked(platform.subscribe).mockImplementation((next) => {
      sidecarListener = next;
      return sidecarSubscription.promise;
    });
    vi.mocked(platform.subscribeStorageHealth).mockImplementation((next) => {
      storageListener = next;
      return storageSubscription.promise;
    });
    let runtime: ReturnType<typeof useRuntime> | undefined;
    const view = render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );
    await waitFor(() => expect(sidecarListener).toBeTypeOf("function"));
    await waitFor(() => expect(storageListener).toBeTypeOf("function"));
    view.unmount();

    const readyEvent: Envelope = {
      version: 1,
      id: "018f0000-0000-7000-8000-000000000047",
      session_id: sessionId,
      sequence: 1,
      timestamp_ms: 1,
      kind: EventKind.SIDECAR_READY,
      payload: { status: "ready" },
      correlation_id: null,
    };
    const failedHealth: StorageHealth = {
      status: "error",
      code: "storage-unavailable",
      message: "Storage unavailable",
      recoverable: false,
    };
    act(() => {
      sidecarListener!(readyEvent);
      storageListener!(failedHealth);
    });
    expect(runtime?.store.getState().health.sidecar).toEqual({ status: "unknown" });
    expect(runtime?.store.getState().health.storage).toEqual({ status: "pending", recoverable: false });

    await new Promise((resolve) => setTimeout(resolve, 1));
    act(() => {
      sidecarListener!(readyEvent);
      storageListener!(failedHealth);
    });
    expect(runtime?.store.getState().health.sidecar).toEqual({ status: "unknown" });
    expect(runtime?.store.getState().health.storage).toEqual({ status: "pending", recoverable: false });

    sidecarSubscription.resolve(() => platform.unlisten());
    storageSubscription.resolve(() => platform.storageUnlisten());
    await waitFor(() => expect(platform.unlisten).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(platform.storageUnlisten).toHaveBeenCalledTimes(1));
  });

  it("restores durable associations before replaying the host timeline", async () => {
    const platform = fakePlatform();
    vi.mocked(platform.restoreActiveSession).mockResolvedValueOnce(session(sessionId));
    vi.mocked(platform.getRequestTurnAssociations).mockResolvedValueOnce([{
      requestId: "018f0000-0000-7000-8000-000000000043",
      turnId: "018f0000-0000-7000-8000-000000000044",
    }]);
    vi.mocked(platform.getTimeline).mockResolvedValueOnce([{
      version: 1,
      id: "018f0000-0000-7000-8000-000000000045",
      session_id: sessionId,
      sequence: 1,
      timestamp_ms: 1,
      kind: EventKind.SUGGESTION_COMPLETED,
      payload: { suggestion_id: "suggestion-restored", text: "Recovered answer" },
      correlation_id: "018f0000-0000-7000-8000-000000000043",
    }]);
    let runtime: ReturnType<typeof useRuntime> | undefined;
    render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id={sessionId} onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );

    await waitFor(() => expect(platform.getTimeline).toHaveBeenCalledWith(sessionId));

    expect(vi.mocked(platform.getRequestTurnAssociations).mock.invocationCallOrder[0])
      .toBeLessThan(vi.mocked(platform.getTimeline).mock.invocationCallOrder[0]);
    expect(runtime?.store.getState().suggestionsByTurn["018f0000-0000-7000-8000-000000000044"])
      .toMatchObject({ text: "Recovered answer" });
  });

  it("records startup, restart, and status errors without throwing", async () => {
    const platform = fakePlatform();
    vi.mocked(platform.sidecarStatus).mockRejectedValueOnce(new Error("status unavailable"));
    vi.mocked(platform.restartSidecar).mockRejectedValueOnce(new Error("restart unavailable"));
    const view = render(
      <RuntimeProvider platform={platform}>
        <Consumer />
      </RuntimeProvider>,
    );

    await waitFor(() => expect(platform.subscribe).toHaveBeenCalledTimes(1));
    await act(async () => {
      view.getByRole("button", { name: "restart" }).click();
    });

    await waitFor(() => expect(platform.restartSidecar).toHaveBeenCalledTimes(1));
  });

  it("applies subscribed sidecar events", async () => {
    const platform = fakePlatform();
    render(
      <RuntimeProvider platform={platform}>
        <Consumer />
      </RuntimeProvider>,
    );
    await waitFor(() => expect(platform.subscribe).toHaveBeenCalledTimes(1));

    act(() => {
      platform.emit({
        version: 1,
        id: "018f0000-0000-7000-8000-000000000011",
        session_id: sessionId,
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind.SIDECAR_READY,
        payload: { status: "ready" },
        correlation_id: null,
      });
    });

    expect(platform.subscribe).toHaveBeenCalledTimes(1);
  });

  it("does not let a stale restore overwrite a user-started session", async () => {
    const platform = fakePlatform();
    const activeRestore = deferred<SessionRecord | null>();
    vi.mocked(platform.restoreActiveSession).mockReturnValue(activeRestore.promise);
    let runtime: ReturnType<typeof useRuntime> | undefined;
    const view = render(
      <RuntimeProvider platform={platform}>
        <SessionStarter id="018f0000-0000-7000-8000-000000000031" onRuntime={(value) => { runtime = value; }} />
      </RuntimeProvider>,
    );

    await waitFor(() => expect(platform.restoreActiveSession).toHaveBeenCalledTimes(1));
    await act(async () => {
      view.getByRole("button", { name: "start session" }).click();
    });
    await act(async () => {
      activeRestore.resolve(session("018f0000-0000-7000-8000-000000000032"));
      await Promise.resolve();
    });

    expect(runtime?.store.getState().session?.id).toBe("018f0000-0000-7000-8000-000000000031");
    expect(platform.getTimeline).not.toHaveBeenCalled();
  });

  it("invalidates a pending restore after unmount under StrictMode", async () => {
    const platform = fakePlatform();
    const activeRestore = deferred<SessionRecord | null>();
    vi.mocked(platform.restoreActiveSession).mockReturnValue(activeRestore.promise);
    let runtime: ReturnType<typeof useRuntime> | undefined;
    const view = render(
      <StrictMode>
        <RuntimeProvider platform={platform}>
          <SessionStarter id="018f0000-0000-7000-8000-000000000033" onRuntime={(value) => { runtime = value; }} />
        </RuntimeProvider>
      </StrictMode>,
    );

    await waitFor(() => expect(platform.restoreActiveSession).toHaveBeenCalledTimes(1));
    view.unmount();
    await new Promise((resolve) => setTimeout(resolve, 1));
    await act(async () => {
      activeRestore.resolve(session("018f0000-0000-7000-8000-000000000034"));
      await Promise.resolve();
    });

    expect(runtime?.store.getState().session).toBeNull();
    expect(platform.getTimeline).not.toHaveBeenCalled();
  });
});
