import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { AppShell } from "./AppShell";
import { changeInterfaceLanguage } from "../i18n";

describe("AppShell", () => {
  afterEach(async () => {
    await changeInterfaceLanguage("en");
  });

  it("orders Interview first and labels icon-only controls", () => {
    render(<AppShell />);

    const modes = screen.getAllByRole("link", {
      name: /Interview|Sales|Meeting|Presentation/,
    });

    expect(modes[0]).toHaveAccessibleName("Interview");
    expect(screen.getByRole("button", { name: "Open settings" })).toBeVisible();
  });

  it("sets document direction from the active locale", async () => {
    await changeInterfaceLanguage("ar-XB");

    expect(document.documentElement.dir).toBe("rtl");
  });
});
