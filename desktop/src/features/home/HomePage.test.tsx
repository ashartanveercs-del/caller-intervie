import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { RuntimeProvider } from "../../app/RuntimeProvider";
import "../../i18n";
import type { PlatformApi } from "../../platform";
import { HomePage } from "./HomePage";

function fakePlatform(): PlatformApi {
  return {
    sidecarStatus: vi.fn().mockResolvedValue({ state: "ready", restartCount: 0, diagnostics: [] }),
    storageHealth: vi.fn().mockResolvedValue({ status: "ready", recoverable: false }),
    captureProtectionStatus: vi.fn().mockResolvedValue({ state: "protected" }),
    send: vi.fn().mockResolvedValue(undefined),
    restartSidecar: vi.fn().mockResolvedValue({ state: "ready", restartCount: 0, diagnostics: [] }),
    retryCaptureProtection: vi.fn().mockResolvedValue({ state: "protected" }),
    subscribe: vi.fn().mockResolvedValue(() => undefined),
    subscribeCaptureProtection: vi.fn().mockResolvedValue(() => undefined),
    subscribeStorageHealth: vi.fn().mockResolvedValue(() => undefined),
    associateRequestWithTurn: vi.fn().mockResolvedValue(undefined),
    getRequestTurnAssociations: vi.fn().mockResolvedValue([]),
    createSession: vi.fn(),
    saveSessionBrief: vi.fn(),
    completeSession: vi.fn(),
    listSessions: vi.fn().mockResolvedValue([]),
    getSession: vi.fn(),
    getTimeline: vi.fn(),
    restoreActiveSession: vi.fn().mockResolvedValue(null),
    deleteSession: vi.fn(),
  };
}

function renderHome(platform: PlatformApi = fakePlatform()) {
  const router = createMemoryRouter(
    [
      { path: "/", element: <HomePage /> },
      { path: "/prepare/interview", element: <h2>Interview preparation route</h2> },
      { path: "/live/:sessionId", element: <h2>Live route</h2> },
      { path: "/review/:sessionId", element: <h2>Review route</h2> },
    ],
    { initialEntries: ["/"] },
  );

  render(
    <RuntimeProvider platform={platform}>
      <RouterProvider router={router} />
    </RuntimeProvider>,
  );
}

describe("HomePage", () => {
  it("presents every professional mode with Interview first", () => {
    renderHome();

    expect(screen.getAllByRole("button", { name: /^Start / }).map((button) => button.textContent)).toEqual([
      "Start Interview",
      "Start Sales Call",
      "Start Meeting",
      "Start Presentation",
    ]);
  });

  it("opens Interview preparation from the available action", async () => {
    renderHome();

    fireEvent.click(screen.getByRole("button", { name: "Start Interview" }));

    expect(await screen.findByRole("heading", { name: "Interview preparation route" })).toBeVisible();
  });

  it("explains planned modes without leaving Home", () => {
    renderHome();

    fireEvent.click(screen.getByRole("button", { name: "Start Sales Call" }));

    const dialog = screen.getByRole("dialog", { name: "Sales Call is coming soon" });
    expect(dialog).toHaveTextContent("Interview is available now");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Interview workspace" })).toBeVisible();
  });

  it("loads recent sessions with status-appropriate actions", async () => {
    const platform = fakePlatform();
    vi.mocked(platform.listSessions).mockResolvedValue([
      {
        id: "018f0000-0000-7000-8000-000000000001",
        mode: "interview",
        status: "active",
        inputLanguage: "auto",
        responseLanguage: "en",
        reviewLanguage: "en",
      },
      {
        id: "018f0000-0000-7000-8000-000000000002",
        mode: "meeting",
        status: "completed",
        inputLanguage: "en",
        responseLanguage: "en",
        reviewLanguage: "en",
      },
    ]);
    renderHome(platform);

    expect(await screen.findByRole("button", { name: "Resume Interview session" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Review Meeting session" })).toBeVisible();
    expect(platform.listSessions).toHaveBeenCalledWith(5);

    fireEvent.click(screen.getByRole("button", { name: "Resume Interview session" }));
    expect(await screen.findByRole("heading", { name: "Live route" })).toBeVisible();
  });

  it("offers a retry when recent sessions cannot be loaded", async () => {
    const platform = fakePlatform();
    vi.mocked(platform.listSessions)
      .mockRejectedValueOnce(new Error("storage unavailable"))
      .mockResolvedValueOnce([]);
    renderHome(platform);

    expect(await screen.findByRole("alert")).toHaveTextContent("Recent sessions could not be loaded");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    await waitFor(() => expect(platform.listSessions).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
