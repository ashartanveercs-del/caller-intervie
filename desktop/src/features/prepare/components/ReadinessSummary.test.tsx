import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import "../../../i18n";
import type { InterviewBrief } from "../briefSchema";
import { ReadinessSummary } from "./ReadinessSummary";

const quickStartBrief: InterviewBrief = {
  schemaVersion: 1,
  mode: "interview",
  context: {},
  documents: {},
  storyIds: [],
  interviewType: "mixed",
  answerStyle: "concise",
  languages: { ui: "en", input: "auto", response: "en", review: "en" },
};

describe("ReadinessSummary", () => {
  it("suggests optional context without blocking Quick Start", () => {
    render(<ReadinessSummary brief={quickStartBrief} />);

    const summary = screen.getByRole("region", { name: "Session readiness" });
    expect(summary).toHaveTextContent("You can start now");
    expect(summary).toHaveTextContent("Add a target role");
    expect(summary).toHaveTextContent("Attach a resume");
    expect(summary).toHaveTextContent("Select a story");
  });
});
