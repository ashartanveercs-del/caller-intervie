import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { RuntimeProvider } from "../../app/RuntimeProvider";
import "../../i18n";
import type { PlatformApi, SessionRecord } from "../../platform";
import { CommandKind, decodeEnvelope } from "../../shared/protocol";
import type { InterviewBrief } from "./briefSchema";
import { buildSessionStartCommand, InterviewPreparePage } from "./InterviewPreparePage";

const session: SessionRecord = {
  id: "018f0000-0000-7000-8000-000000000021",
  mode: "interview",
  status: "active",
  uiLanguage: "en",
  inputLanguage: "auto",
  responseLanguage: "ur",
  reviewLanguage: "en",
};

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
    createSession: vi.fn().mockResolvedValue(session),
    saveSessionBrief: vi.fn().mockResolvedValue(undefined),
    completeSession: vi.fn().mockResolvedValue(undefined),
    listSessions: vi.fn().mockResolvedValue([]),
    getSession: vi.fn(),
    getTimeline: vi.fn(),
    restoreActiveSession: vi.fn().mockResolvedValue(null),
    deleteSession: vi.fn().mockResolvedValue(undefined),
  };
}

function renderPrepare(
  platform: PlatformApi = fakePlatform(),
  storyOptions: readonly { id: string; title: string; detail?: string }[] = [],
) {
  const router = createMemoryRouter(
    [
      { path: "/prepare/interview", element: <InterviewPreparePage storyOptions={storyOptions} /> },
      { path: "/live/:sessionId", element: <h2>Live interview route</h2> },
    ],
    { initialEntries: ["/prepare/interview"] },
  );
  render(
    <RuntimeProvider platform={platform}>
      <RouterProvider router={router} />
    </RuntimeProvider>,
  );
}

describe("InterviewPreparePage", () => {
  it("assigns increasing sequence numbers to start commands", () => {
    const first = buildSessionStartCommand(session);
    const second = buildSessionStartCommand(session);

    expect(second.sequence).toBeGreaterThan(first.sequence);
  });

  it("creates and starts an Interview session with independent languages", async () => {
    const platform = fakePlatform();
    renderPrepare(platform);

    fireEvent.change(screen.getByLabelText("Spoken language"), { target: { value: "auto" } });
    fireEvent.change(screen.getByLabelText("Suggestion language"), { target: { value: "ur" } });
    fireEvent.click(screen.getByRole("button", { name: "Start interview" }));

    await waitFor(() => expect(platform.createSession).toHaveBeenCalledWith({
      mode: "interview",
      uiLanguage: "en",
      inputLanguage: "auto",
      responseLanguage: "ur",
      reviewLanguage: "en",
    }));
    expect(platform.saveSessionBrief).toHaveBeenCalledWith(expect.objectContaining({ sessionId: session.id }));

    const command = decodeEnvelope(vi.mocked(platform.send).mock.calls[0][0]);
    expect(command).toMatchObject({
      kind: CommandKind.SESSION_START,
      session_id: session.id,
      payload: {
        mode: "interview",
        input_language: "auto",
        response_language: "ur",
        review_language: "en",
        you_source: "mic",
        brief_id: session.id,
      },
    });
    expect(await screen.findByRole("heading", { name: "Live interview route" })).toBeVisible();
  });

  it("persists selected context, document metadata, and story references", async () => {
    const platform = fakePlatform();
    renderPrepare(platform, [{ id: "story-leadership", title: "Leadership launch" }]);
    const resume = new File(["resume text"], "resume.pdf", { type: "application/pdf", lastModified: 42 });
    const jobDescription = new File(["role text"], "role.txt", { type: "text/plain", lastModified: 84 });

    fireEvent.change(screen.getByLabelText("Target role"), { target: { value: "Staff engineer" } });
    fireEvent.change(screen.getByLabelText("Company"), { target: { value: "Acme" } });
    fireEvent.change(screen.getByLabelText("Interview stage"), { target: { value: "Panel" } });
    fireEvent.change(screen.getByLabelText("Resume"), { target: { files: [resume] } });
    fireEvent.change(screen.getByLabelText("Job description"), { target: { files: [jobDescription] } });
    fireEvent.click(screen.getByLabelText("Leadership launch"));
    fireEvent.change(screen.getByLabelText("Interview type"), { target: { value: "technical" } });
    fireEvent.change(screen.getByLabelText("Answer style"), { target: { value: "star" } });
    fireEvent.change(screen.getByLabelText("Custom guidance"), { target: { value: "Use measurable outcomes" } });
    fireEvent.click(screen.getByRole("button", { name: "Start interview" }));

    await waitFor(() => expect(platform.saveSessionBrief).toHaveBeenCalledTimes(1));
    const savedBrief = vi.mocked(platform.saveSessionBrief).mock.calls[0][0].brief as InterviewBrief;
    expect(savedBrief).toMatchObject({
      context: { role: "Staff engineer", company: "Acme", stage: "Panel" },
      documents: {
        resume: { name: "resume.pdf", sizeBytes: 11, mediaType: "application/pdf", lastModifiedMs: 42 },
        jobDescription: { name: "role.txt", sizeBytes: 9, mediaType: "text/plain", lastModifiedMs: 84 },
      },
      storyIds: ["story-leadership"],
      interviewType: "technical",
      answerStyle: "star",
      answerGuidance: "Use measurable outcomes",
    });
    expect(JSON.stringify(savedBrief)).not.toContain("resume text");
    expect(JSON.stringify(savedBrief)).not.toContain("role text");
  });

  it("keeps focus on Prepare when runtime startup is blocked", async () => {
    const platform = fakePlatform();
    vi.mocked(platform.send).mockRejectedValue(Object.assign(new Error("private runtime detail"), {
      code: "capture_protection_required",
    }));
    renderPrepare(platform);

    fireEvent.click(screen.getByRole("button", { name: "Start interview" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveFocus();
    expect(alert).toHaveTextContent("Confirm screen-capture protection");
    expect(alert).not.toHaveTextContent("private runtime detail");
    expect(screen.getByRole("heading", { name: "Prepare for your interview" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Live interview route" })).not.toBeInTheDocument();
    expect(platform.deleteSession).toHaveBeenCalledWith(session.id);
    await waitFor(() => expect(screen.getByRole("button", { name: "Start interview" })).toBeEnabled());
  });
});
