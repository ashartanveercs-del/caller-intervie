import { render, screen, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { RuntimeProvider } from "../../app/RuntimeProvider";
import type { PlatformApi, SessionRecord } from "../../platform";
import { EventKind, type Envelope } from "../../shared/protocol";
import "../../i18n";
import { ReviewPage } from "./ReviewPage";

const sessionId = "018f0000-0000-7000-8000-000000000101";
const turnId = "018f0000-0000-7000-8000-000000000102";
const requestId = "018f0000-0000-7000-8000-000000000103";

describe("ReviewPage", () => {
  it("separates the original transcript from generated suggestions", async () => {
    renderReview(createReviewPlatform());

    expect(await screen.findByRole("heading", { name: "Session review" })).toBeVisible();
    const transcript = screen.getByRole("region", { name: "Original transcript" });
    const suggestions = screen.getByRole("region", { name: "AI suggestions" });

    expect(within(transcript).getByText("Cuéntame sobre un proyecto difícil.")).toBeVisible();
    expect(within(suggestions).getByText("Lead with the constraint, then quantify the result.")).toBeVisible();
    expect(within(suggestions).getByText("Cuéntame sobre un proyecto difícil.")).toBeVisible();
    expect(within(transcript).queryByText("Tell me about a difficult project.")).not.toBeInTheDocument();
  });

  it("labels generated translations and runtime interruptions separately", async () => {
    renderReview(createReviewPlatform({ status: "interrupted" }));

    expect(await screen.findByText("Interrupted")).toBeVisible();
    const translations = screen.getByRole("region", { name: "Generated translations" });
    const interruptions = screen.getByRole("region", { name: "Runtime interruptions" });

    expect(within(translations).getByText("Tell me about a difficult project.")).toBeVisible();
    expect(within(translations).getByText("Generated translation for review; the original remains unchanged above.")).toBeVisible();
    expect(within(interruptions).getByText("Transcription provider disconnected")).toBeVisible();
    expect(within(screen.getByRole("region", { name: "Session notes" }))
      .getByText("Follow up with the retention metric.")).toBeVisible();
    const pins = screen.getByRole("region", { name: "Pinned guidance" });
    expect(within(pins).getByText("Cuéntame sobre un proyecto difícil.")).toBeVisible();
    expect(within(pins).getByText("Lead with the constraint, then quantify the result.")).toBeVisible();
  });
});

function renderReview(platform: PlatformApi) {
  return render(
    <RuntimeProvider platform={platform}>
      <MemoryRouter initialEntries={[`/review/${sessionId}`]}>
        <Routes>
          <Route path="/review/:sessionId" element={<ReviewPage />} />
        </Routes>
      </MemoryRouter>
    </RuntimeProvider>,
  );
}

function createReviewPlatform(overrides: { status?: SessionRecord["status"] } = {}): PlatformApi {
  const session: SessionRecord = {
    id: sessionId,
    mode: "interview",
    status: overrides.status ?? "completed",
    uiLanguage: "en",
    inputLanguage: "es",
    responseLanguage: "en",
    reviewLanguage: "en",
    brief: {
      context: { role: "Product manager", company: "Acme" },
      reviewArtifacts: [
        {
          id: "018f0000-0000-7000-8000-000000000120",
          kind: "note",
          text: "Follow up with the retention metric.",
          createdAtMs: 1_700_000_000_300,
        },
        {
          id: "018f0000-0000-7000-8000-000000000121",
          kind: "pin",
          turnId,
          question: "Cuéntame sobre un proyecto difícil.",
          suggestion: "Lead with the constraint, then quantify the result.",
          createdAtMs: 1_700_000_000_400,
        },
      ],
    },
  };

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
    getRequestTurnAssociations: vi.fn().mockResolvedValue([{ requestId, turnId }]),
    createSession: vi.fn(),
    saveSessionBrief: vi.fn().mockResolvedValue(undefined),
    completeSession: vi.fn().mockResolvedValue(undefined),
    listSessions: vi.fn().mockResolvedValue([]),
    getSession: vi.fn().mockResolvedValue(session),
    getTimeline: vi.fn().mockResolvedValue(reviewTimeline()),
    restoreActiveSession: vi.fn().mockResolvedValue(null),
    deleteSession: vi.fn().mockResolvedValue(undefined),
  };
}

function reviewTimeline(): Envelope[] {
  return [
    {
      version: 1,
      id: "018f0000-0000-7000-8000-000000000110",
      session_id: sessionId,
      sequence: 1,
      timestamp_ms: 1_700_000_000_000,
      kind: EventKind.TRANSCRIPT_UPDATED,
      payload: {
        turn_id: turnId,
        text: "Cuéntame sobre un proyecto difícil.",
        is_final: true,
        speech_final: true,
        speaker_role: "interviewer",
        language: "es",
        translated_text: "Tell me about a difficult project.",
        translated_language: "en",
      },
      correlation_id: null,
    },
    {
      version: 1,
      id: "018f0000-0000-7000-8000-000000000111",
      session_id: sessionId,
      sequence: 2,
      timestamp_ms: 1_700_000_000_100,
      kind: EventKind.SUGGESTION_COMPLETED,
      payload: {
        suggestion_id: "suggestion-review-1",
        text: "Lead with the constraint, then quantify the result.",
      },
      correlation_id: requestId,
    },
    {
      version: 1,
      id: "018f0000-0000-7000-8000-000000000112",
      session_id: sessionId,
      sequence: 3,
      timestamp_ms: 1_700_000_000_200,
      kind: EventKind.RUNTIME_ERROR,
      payload: { message: "Transcription provider disconnected" },
      correlation_id: null,
    },
  ];
}
