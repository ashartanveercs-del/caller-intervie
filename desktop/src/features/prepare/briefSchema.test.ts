import { describe, expect, it } from "vitest";
import { interviewBriefSchema } from "./briefSchema";

describe("interviewBriefSchema", () => {
  it("accepts Quick Start without optional context", () => {
    const result = interviewBriefSchema.safeParse({
      schemaVersion: 1,
      mode: "interview",
      context: {},
      documents: {},
      storyIds: [],
      interviewType: "mixed",
      answerStyle: "concise",
      languages: {
        ui: "en",
        input: "auto",
        response: "ur",
        review: "en",
      },
    });

    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.languages).toEqual({
        ui: "en",
        input: "auto",
        response: "ur",
        review: "en",
      });
    }
  });
});
