import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import "../../../i18n";
import type { InterviewLanguages } from "../briefSchema";
import { LanguageControls } from "./LanguageControls";

function LanguageHarness() {
  const [value, setValue] = useState<InterviewLanguages>({
    ui: "en",
    input: "en",
    response: "en",
    review: "en",
  });
  return <LanguageControls onChange={setValue} value={value} />;
}

describe("LanguageControls", () => {
  it("changes spoken and suggestion languages independently", () => {
    render(<LanguageHarness />);

    fireEvent.change(screen.getByLabelText("Spoken language"), { target: { value: "auto" } });
    fireEvent.change(screen.getByLabelText("Suggestion language"), { target: { value: "ur" } });

    expect(screen.getByLabelText("Spoken language")).toHaveValue("auto");
    expect(screen.getByLabelText("Suggestion language")).toHaveValue("ur");
    expect(screen.getByLabelText("Interface language")).toHaveValue("en");
    expect(screen.getByLabelText("Review language")).toHaveValue("en");
  });
});
