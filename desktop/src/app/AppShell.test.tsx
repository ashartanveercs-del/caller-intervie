import { fireEvent, render, screen } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router-dom";
import { afterEach, describe, expect, it } from "vitest";
import { AppShell } from "./AppShell";
import { changeInterfaceLanguage } from "../i18n";

function renderAppShell(initialEntry = "/") {
  const router = createMemoryRouter(
    [{ path: "*", element: <AppShell /> }],
    { initialEntries: [initialEntry] },
  );

  render(<RouterProvider router={router} />);

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
});
