import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { changeInterfaceLanguage } from "../i18n";
import type { PlatformApi, SessionRecord } from "../platform";
import { RuntimeProvider } from "./RuntimeProvider";
import { routes } from "./router";

const sessionId = "018f0000-0000-7000-8000-000000000001";

type CaptureState = "applying" | "protected" | "unavailable" | "unsupported";

function activeSession(): SessionRecord {
  return {
    id: sessionId,
    mode: "interview",
    status: "active",
    inputLanguage: "en",
    responseLanguage: "en",
    reviewLanguage: "en",
  };
}

function completedSession(): SessionRecord {
  return {
    ...activeSession(),
    status: "completed",
  };
}

function fakePlatform(
  captureState: CaptureState,
  restoredSession: SessionRecord | null = null,
): PlatformApi {
  return {
    sidecarStatus: vi.fn().mockResolvedValue({ state: "ready", restartCount: 0, diagnostics: [] }),
    storageHealth: vi.fn().mockResolvedValue({ status: "ready", recoverable: false }),
    captureProtectionStatus: vi.fn().mockResolvedValue(captureState === "unsupported"
      ? { state: "unsupported", code: "browser_preview" }
      : { state: captureState }),
    retryCaptureProtection: vi.fn().mockResolvedValue({ state: "protected" }),
    send: vi.fn().mockResolvedValue(undefined),
    restartSidecar: vi.fn().mockResolvedValue({ state: "ready", restartCount: 0, diagnostics: [] }),
    subscribe: vi.fn().mockResolvedValue(() => undefined),
    subscribeStorageHealth: vi.fn().mockResolvedValue(() => undefined),
    subscribeCaptureProtection: vi.fn().mockResolvedValue(() => undefined),
    associateRequestWithTurn: vi.fn().mockResolvedValue(undefined),
    getRequestTurnAssociations: vi.fn().mockResolvedValue([]),
    createSession: vi.fn().mockResolvedValue(activeSession()),
    saveSessionBrief: vi.fn().mockResolvedValue(undefined),
    completeSession: vi.fn().mockResolvedValue(undefined),
    listSessions: vi.fn().mockResolvedValue([]),
    getSession: vi.fn().mockResolvedValue(restoredSession),
    getTimeline: vi.fn().mockResolvedValue([]),
    restoreActiveSession: vi.fn().mockResolvedValue(restoredSession),
    deleteSession: vi.fn().mockResolvedValue(undefined),
  };
}

function renderRoute(
  path: string,
  captureState: CaptureState,
  restoredSession: SessionRecord | null = null,
) {
  const platform = fakePlatform(captureState, restoredSession);
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  render(
    <RuntimeProvider platform={platform}>
      <RouterProvider router={router} />
    </RuntimeProvider>,
  );
  return { platform, router };
}

afterEach(async () => {
  await changeInterfaceLanguage("en");
});

describe("router", () => {
  it("renders the Home product screen at the root route", async () => {
    renderRoute("/", "unsupported");

    expect(await screen.findByRole("button", { name: "Start Interview" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Start Sales Call" })).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("renders Interview preparation at its supported route", async () => {
    renderRoute("/prepare/interview", "unsupported");

    expect(await screen.findByRole("heading", { name: "Prepare for your interview" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Start interview" })).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps planned Home modes unavailable without navigating", async () => {
    const { router } = renderRoute("/", "unsupported");

    fireEvent.click(await screen.findByRole("button", { name: "Start Sales Call" }));

    expect(screen.getByRole("dialog", { name: "Sales Call is coming soon" })).toBeVisible();
    expect(router.state.location.pathname).toBe("/");
    expect(screen.queryByRole("button", { name: "Start interview" })).not.toBeInTheDocument();
  });

  it("uses pseudo-localized catalog copy across Home and Interview preparation", async () => {
    await changeInterfaceLanguage("ar-XB");
    renderRoute("/", "unsupported");

    const startInterview = await screen.findByRole("button", {
      name: "[[ S~tart I~nte~rvie~w ]]",
    });
    fireEvent.click(startInterview);

    expect(await screen.findByRole("heading", {
      name: "[[ P~repa~re f~or y~our i~nte~rvie~w ]]",
    })).toBeVisible();
    expect(screen.getByRole("button", { name: "[[ S~tart i~nte~rvie~w ]]" })).toBeVisible();
  });

  it("does not expose Interview preparation through planned mode URLs", async () => {
    const { router } = renderRoute("/prepare/sales", "unsupported");

    await waitFor(() => {
      expect(Object.values(router.state.errors ?? {})).toEqual([
        expect.objectContaining({ status: 404 }),
      ]);
    });
    expect(screen.queryByRole("button", { name: "Start interview" })).not.toBeInTheDocument();
  });

  it("blocks Live content when capture protection is unavailable", async () => {
    const { platform } = renderRoute(`/live/${sessionId}`, "unavailable", activeSession());

    const blocker = await screen.findByRole("alert");
    expect(blocker).toHaveTextContent(/Live mode is blocked/i);
    expect(screen.queryByRole("toolbar", { name: /Live session controls/i })).not.toBeInTheDocument();
    const retry = within(blocker).getByRole("button", { name: /retry capture protection/i });
    fireEvent.click(retry);

    await waitFor(() => expect(platform.retryCaptureProtection).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("toolbar", { name: /Live session controls/i })).toBeVisible();
  });

  it("renders normal Live content only after capture protection is confirmed", async () => {
    renderRoute(`/live/${sessionId}`, "protected", activeSession());

    expect(await screen.findByRole("toolbar", { name: /Live session controls/i })).toBeVisible();
    expect(screen.getAllByRole("main")).toHaveLength(1);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("blocks Live content while capture protection is applying", () => {
    renderRoute(`/live/${sessionId}`, "applying", activeSession());

    expect(screen.getByRole("alert")).toHaveTextContent(/checking capture protection/i);
    expect(screen.queryByRole("toolbar", { name: /Live session controls/i })).not.toBeInTheDocument();
  });

  it("keeps Prepare usable when browser preview capture protection is unsupported", async () => {
    renderRoute("/prepare/interview", "unsupported");

    expect(await screen.findByRole("heading", { name: /Prepare for your interview/i })).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("explains that unsupported browser preview requires the desktop app", async () => {
    renderRoute(`/live/${sessionId}`, "unsupported", activeSession());

    expect(await screen.findByRole("alert")).toHaveTextContent(/desktop app/i);
    expect(screen.queryByRole("button", { name: /retry capture protection/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("toolbar", { name: /Live session controls/i })).not.toBeInTheDocument();
  });

  it("renders the saved Review product screen", async () => {
    renderRoute(`/review/${sessionId}`, "unsupported", completedSession());

    expect(await screen.findByRole("heading", { name: "Session review" })).toBeVisible();
    expect(screen.getAllByRole("main")).toHaveLength(1);
    expect(screen.getByRole("region", { name: "Original transcript" })).toBeVisible();
    expect(screen.getByText("No final transcript was saved.")).toBeVisible();
  });
});
