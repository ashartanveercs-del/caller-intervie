import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import type { PlatformApi } from "../platform";
import { RuntimeProvider } from "./RuntimeProvider";
import { routes } from "./router";

function fakePlatform(captureState: "applying" | "protected" | "unavailable" | "unsupported"): PlatformApi {
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
    associateRequestWithTurn: vi.fn().mockResolvedValue(undefined),
    getRequestTurnAssociations: vi.fn().mockResolvedValue([]),
    createSession: vi.fn(),
    saveSessionBrief: vi.fn(),
    completeSession: vi.fn(),
    listSessions: vi.fn(),
    getSession: vi.fn(),
    getTimeline: vi.fn(),
    restoreActiveSession: vi.fn().mockResolvedValue(null),
    deleteSession: vi.fn(),
  };
}

function renderRoute(path: string, captureState: "applying" | "protected" | "unavailable" | "unsupported") {
  const platform = fakePlatform(captureState);
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  render(
    <RuntimeProvider platform={platform}>
      <RouterProvider router={router} />
    </RuntimeProvider>,
  );
  return platform;
}

describe("router", () => {
  it("blocks Live content when capture protection is unavailable", async () => {
    const platform = renderRoute("/live/018f0000-0000-7000-8000-000000000001", "unavailable");

    const blocker = await screen.findByRole("alert");
    expect(blocker).toHaveTextContent(/Live mode is blocked/i);
    expect(screen.queryByRole("heading", { name: /Live session/i })).not.toBeInTheDocument();
    const retry = within(blocker).getByRole("button", { name: /retry capture protection/i });
    fireEvent.click(retry);

    await waitFor(() => expect(platform.retryCaptureProtection).toHaveBeenCalledTimes(1));
  });

  it("renders normal Live content only after capture protection is confirmed", async () => {
    renderRoute("/live/018f0000-0000-7000-8000-000000000001", "protected");

    expect(await screen.findByRole("heading", { name: /Live session/i })).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("blocks Live content while capture protection is applying", () => {
    renderRoute("/live/018f0000-0000-7000-8000-000000000001", "applying");

    expect(screen.getByRole("alert")).toHaveTextContent(/checking capture protection/i);
    expect(screen.queryByRole("heading", { name: /Live session/i })).not.toBeInTheDocument();
  });

  it("keeps Prepare usable when browser preview capture protection is unsupported", async () => {
    renderRoute("/prepare/interview", "unsupported");

    expect(await screen.findByRole("heading", { name: /Prepare Interview session/i })).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("explains that unsupported browser preview requires the desktop app", async () => {
    renderRoute("/live/018f0000-0000-7000-8000-000000000001", "unsupported");

    expect(await screen.findByRole("alert")).toHaveTextContent(/desktop app/i);
    expect(screen.queryByRole("button", { name: /retry capture protection/i })).not.toBeInTheDocument();
  });
});
