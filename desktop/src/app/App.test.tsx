import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("App", () => {
  it("renders the Interview-first product shell", () => {
    render(<App />);
    expect(screen.getByRole("heading", { name: "CallerInterview" })).toBeVisible();
    expect(screen.getByRole("link", { name: "Interview" })).toHaveAttribute("aria-current", "page");
  });
});
