import { StrictMode } from "react";
import { act, render, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EventKind, type Envelope } from "../shared/protocol";
import type { PlatformApi, SessionRecord, SidecarStatus } from "../platform";
import { RuntimeProvider, useRuntime } from "./RuntimeProvider";

const sessionId = "018f0000-0000-7000-8000-000000000001";

function status(state: SidecarStatus["state"] = "ready"): SidecarStatus {
  return { state, restartCount: 0, diagnostics: [] };
}

function fakePlatform(): PlatformApi & { emit(event: Envelope): void; unlisten: ReturnType<typeof vi.fn> } {
  let listener: ((event: Envelope) => void) | undefined;
  const unlisten = vi.fn(() => {
    listener = undefined;
  });
  return {
    sidecarStatus: vi.fn().mockResolvedValue(status()),
    send: vi.fn().mockResolvedValue(undefined),
    restartSidecar: vi.fn().mockResolvedValue(status()),
    subscribe: vi.fn().mockImplementation(async (next) => {
      listener = next;
      return unlisten;
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
    unlisten,
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

describe("RuntimeProvider", () => {
  it("subscribes and unsubscribes exactly once under StrictMode", async () => {
    const platform = fakePlatform();
    const view = render(
      <StrictMode>
        <RuntimeProvider platform={platform}>
          <Consumer />
        </RuntimeProvider>
      </StrictMode>,
    );

    await waitFor(() => expect(platform.subscribe).toHaveBeenCalledTimes(1));
    view.unmount();

    expect(platform.sidecarStatus).toHaveBeenCalledTimes(1);
    expect(platform.restoreActiveSession).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(platform.unlisten).toHaveBeenCalledTimes(1));
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
