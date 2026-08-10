import { fireEvent, render, screen } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppShell } from "./AppShell";
import { changeInterfaceLanguage } from "../i18n";
import { RuntimeProvider } from "./RuntimeProvider";
import type { PlatformApi } from "../platform";

function fakePlatform(): PlatformApi {
  return {
    sidecarStatus: vi.fn().mockResolvedValue({ state: "ready", restartCount: 0, diagnostics: [] }),
    storageHealth: vi.fn().mockResolvedValue({ status: "ready", recoverable: false }),
    captureProtectionStatus: vi.fn().mockResolvedValue({ state: "protected" }),
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

function renderAppShell(initialEntry = "/") {
  const router = createMemoryRouter(
    [{ path: "*", element: <AppShell /> }],
    { initialEntries: [initialEntry] },
  );

  render(
    <RuntimeProvider platform={fakePlatform()}>
      <RouterProvider router={router} />
    </RuntimeProvider>,
  );

  return router;
}

describe("AppShell", () => {
  afterEach(async () => {
    await changeInterfaceLanguage("en");
  });

  it("orders Interview first and labels icon-only controls", () => {
    renderAppShell();

    const modes = screen.getAllByRole("link", {
      name: /Interview|Sales|Meeting|Presentation/,
    });

    expect(modes[0]).toHaveAccessibleName("Interview");
    expect(screen.getByRole("button", { name: "Open settings" })).toBeVisible();
  });

  it("navigates modes through the router and updates the active mode", () => {
    const router = renderAppShell();

    fireEvent.click(screen.getByRole("link", { name: "Sales" }));

    expect(router.state.location.pathname).toBe("/prepare/sales");
    expect(screen.getByRole("link", { name: "Sales" })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  it("sets document direction from the active locale", async () => {
    await changeInterfaceLanguage("ar-XB");

    expect(document.documentElement.dir).toBe("rtl");
  });

  it("keeps capture protection visible in the persistent status bar", async () => {
    renderAppShell();

    expect(await screen.findByRole("status", { name: /capture protected/i })).toBeVisible();
  });
});
