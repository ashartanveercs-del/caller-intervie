import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { CaptureProtectionStatus } from "../platform";
import "../i18n";
import { CaptureProtectionIndicator } from "./CaptureProtectionIndicator";

function renderIndicator(captureProtection: CaptureProtectionStatus, retry = vi.fn()) {
  render(
    <CaptureProtectionIndicator
      captureProtection={captureProtection}
      onRetry={retry}
    />,
  );
  return retry;
}

describe("CaptureProtectionIndicator", () => {
  it("announces a confirmed protected capture state", () => {
    renderIndicator({ state: "protected" });

    expect(screen.getByRole("status", { name: /capture protected/i })).toBeVisible();
    expect(screen.queryByRole("button", { name: /retry capture protection/i })).not.toBeInTheDocument();
  });

  it("shows a single retry control for unavailable protection", () => {
    const retry = renderIndicator({ state: "unavailable" });

    fireEvent.click(screen.getByRole("button", { name: /retry capture protection/i }));

    expect(retry).toHaveBeenCalledTimes(1);
  });

  it("does not offer a futile retry in browser unsupported state", () => {
    renderIndicator({ state: "unsupported", code: "browser_preview" });

    expect(screen.getByRole("status", { name: /desktop app/i })).toBeVisible();
    expect(screen.queryByRole("button", { name: /retry capture protection/i })).not.toBeInTheDocument();
  });
});
