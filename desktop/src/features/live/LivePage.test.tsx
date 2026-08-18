import { useEffect } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RuntimeProvider } from "../../app/RuntimeProvider";
import { changeInterfaceLanguage } from "../../i18n";
import type {
  CaptureProtectionStatus,
  PlatformApi,
  SessionRecord,
  StorageHealth,
} from "../../platform";
import { CommandKind, EventKind, type Envelope } from "../../shared/protocol";
import { LivePage } from "./LivePage";

const sessionId = "018f0000-0000-7000-8000-000000000001";
const firstTurnId = "018f0000-0000-7000-8000-000000000010";
const secondTurnId = "018f0000-0000-7000-8000-000000000011";
const firstRequestId = "018f0000-0000-7000-8000-000000000020";
const secondRequestId = "018f0000-0000-7000-8000-000000000021";

type LivePlatform = PlatformApi & {
  emit(event: Envelope): void;
  setCaptureProtection(status: CaptureProtectionStatus): void;
};

type RenderLiveOptions = {
  associations?: Array<{ requestId: string; turnId: string }>;
  captureProtection?: CaptureProtectionStatus;
  healthEvents?: Envelope[];
  platform?: LivePlatform;
  routeSessionId?: string;
  timeline?: Envelope[];
};

function activeSession(): SessionRecord {
  return {
    id: sessionId,
    mode: "interview",
    status: "active",
    inputLanguage: "en",
    responseLanguage: "en",
    reviewLanguage: "en",
  };
}

function transcript(
  id: string,
  turnId: string,
  text: string,
  sequence: number,
  speakerRole = "interviewer",
): Envelope {
  return {
    version: 1,
    id,
    session_id: sessionId,
    sequence,
    timestamp_ms: sequence,
    kind: EventKind.TRANSCRIPT_UPDATED,
    payload: {
      turn_id: turnId,
      text,
      is_final: true,
      speech_final: true,
      speaker_role: speakerRole,
      language: "en",
    },
    correlation_id: null,
  };
}

function suggestion(
  id: string,
  suggestionId: string,
  text: string,
  sequence: number,
  correlationId: string,
  complete = true,
): Envelope {
  return {
    version: 1,
    id,
    session_id: sessionId,
    sequence,
    timestamp_ms: sequence,
    kind: complete ? EventKind.SUGGESTION_COMPLETED : EventKind.SUGGESTION_CHUNK,
    payload: { suggestion_id: suggestionId, text },
    correlation_id: correlationId,
  };
}

function health(
  id: string,
  kind: EventKind.AUDIO_HEALTH | EventKind.PROVIDER_HEALTH,
  key: "mic" | "system" | "speech" | "model",
  status: string,
  message?: string,
): Envelope {
  return {
    version: 1,
    id,
    session_id: null,
    sequence: Number(id.slice(-2)),
    timestamp_ms: 1,
    kind,
    payload: {
      [kind === EventKind.AUDIO_HEALTH ? "source" : "provider"]: key,
      status,
      ...(message ? { message } : {}),
    },
    correlation_id: null,
  };
}

function runtimeError(message: string): Envelope {
  return {
    version: 1,
    id: "018f0000-0000-7000-8000-000000000046",
    session_id: sessionId,
    sequence: 46,
    timestamp_ms: 46,
    kind: EventKind.RUNTIME_ERROR,
    payload: { code: "provider_failure", message },
    correlation_id: null,
  };
}

function defaultTimeline(): Envelope[] {
  return [
    transcript("018f0000-0000-7000-8000-000000000030", firstTurnId, "First question", 1),
    suggestion(
      "018f0000-0000-7000-8000-000000000031",
      "answer-one",
      "Context first.\nAction second.\nResult third.\nExtra detail retained.",
      2,
      firstRequestId,
    ),
    transcript("018f0000-0000-7000-8000-000000000032", secondTurnId, "Second question", 3),
    suggestion(
      "018f0000-0000-7000-8000-000000000033",
      "answer-two",
      "The retained second answer",
      4,
      secondRequestId,
    ),
  ];
}

function createLivePlatform(options: RenderLiveOptions = {}): LivePlatform {
  let listener: ((event: Envelope) => void) | undefined;
  let storageListener: ((health: StorageHealth) => void) | undefined;
  let captureProtection = options.captureProtection ?? { state: "protected" };
  const platform: LivePlatform = {
    sidecarStatus: vi.fn().mockResolvedValue({ state: "ready", restartCount: 0, diagnostics: [] }),
    storageHealth: vi.fn().mockResolvedValue({ status: "ready", recoverable: false }),
    captureProtectionStatus: vi.fn().mockImplementation(async () => ({ ...captureProtection })),
    send: vi.fn().mockResolvedValue(undefined),
    restartSidecar: vi.fn().mockResolvedValue({ state: "ready", restartCount: 1, diagnostics: [] }),
    retryCaptureProtection: vi.fn().mockImplementation(async () => ({ ...captureProtection })),
    subscribe: vi.fn().mockImplementation(async (next: (event: Envelope) => void) => {
      listener = next;
      return () => { listener = undefined; };
    }),
    subscribeStorageHealth: vi.fn().mockImplementation(async (next: (value: StorageHealth) => void) => {
      storageListener = next;
      return () => { storageListener = undefined; };
    }),
    subscribeCaptureProtection: vi.fn().mockResolvedValue(() => undefined),
    associateRequestWithTurn: vi.fn().mockResolvedValue(undefined),
    getRequestTurnAssociations: vi.fn().mockResolvedValue(options.associations ?? [
      { requestId: firstRequestId, turnId: firstTurnId },
      { requestId: secondRequestId, turnId: secondTurnId },
    ]),
    createSession: vi.fn(),
    saveSessionBrief: vi.fn(),
    completeSession: vi.fn().mockResolvedValue(undefined),
    listSessions: vi.fn(),
    getSession: vi.fn(),
    getTimeline: vi.fn().mockResolvedValue(options.timeline ?? defaultTimeline()),
    restoreActiveSession: vi.fn().mockResolvedValue(activeSession()),
    deleteSession: vi.fn(),
    emit(event) {
      listener?.(event);
    },
    setCaptureProtection(status) {
      captureProtection = status;
    },
  };
  void storageListener;
  return platform;
}

function EmitHealth({ events, platform }: { events: Envelope[]; platform: LivePlatform }) {
  useEffect(() => {
    const timer = window.setTimeout(() => {
      for (const event of events) platform.emit(event);
    }, 0);
    return () => window.clearTimeout(timer);
  }, [events, platform]);
  return null;
}

function renderLive(options: RenderLiveOptions = {}) {
  const platform = options.platform ?? createLivePlatform(options);
  const routeSessionId = options.routeSessionId ?? sessionId;
  const tree = (liveKey: string) => (
    <RuntimeProvider platform={platform}>
      <MemoryRouter initialEntries={[`/live/${routeSessionId}`]}>
        {options.healthEvents ? <EmitHealth events={options.healthEvents} platform={platform} /> : null}
        <Routes>
          <Route path="/live/:sessionId" element={<LivePage key={liveKey} />} />
          <Route path="/review/:sessionId" element={<h1>Session review</h1>} />
        </Routes>
      </MemoryRouter>
    </RuntimeProvider>
  );
  const view = render(tree("initial"));
  return {
    platform,
    remountLive() {
      view.rerender(tree("remounted"));
    },
    ...view,
  };
}

afterEach(async () => {
  await changeInterfaceLanguage("en");
});

describe("LivePage", () => {
  it("retains previous answers when the next question becomes current", async () => {
    renderLive();

    expect(await screen.findByRole("heading", { name: "Second question" })).toBeVisible();
    expect(screen.getByText("Previous answers")).toBeVisible();
    expect(screen.getByText("Context first.")).toBeVisible();
    expect(screen.getByText("Action second.")).toBeVisible();
    expect(screen.getByText("Result third.")).toBeVisible();
    expect(screen.queryByText("Extra detail retained.")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Expand first answer" }));
    expect(screen.getByText("Extra detail retained.")).toBeVisible();
  });

  it("blocks a Live URL that does not match the exact active session", async () => {
    await changeInterfaceLanguage("ar-XB");
    const platform = createLivePlatform();
    renderLive({
      platform,
      routeSessionId: "018f0000-0000-7000-8000-000000000099",
    });

    const heading = await screen.findByRole("heading", {
      name: "[[ L~ive s~essi~on u~navai~lable ]]",
    });
    const blocker = heading.closest("[role='alert']");
    expect(blocker).toBeInTheDocument();
    expect(blocker).toHaveTextContent("[[ T~his L~ive l~ink d~oes n~ot m~atch t~he a~ctiv~e s~essi~on. ]]");
    expect(screen.queryByRole("toolbar")).not.toBeInTheDocument();
    expect(platform.send).not.toHaveBeenCalled();
  });

  it("shows a named health state for every live dependency", async () => {
    renderLive({
      healthEvents: [
        health("018f0000-0000-7000-8000-000000000041", EventKind.AUDIO_HEALTH, "mic", "ready"),
        health("018f0000-0000-7000-8000-000000000042", EventKind.AUDIO_HEALTH, "system", "ready"),
        health("018f0000-0000-7000-8000-000000000043", EventKind.PROVIDER_HEALTH, "speech", "ready"),
        health("018f0000-0000-7000-8000-000000000044", EventKind.PROVIDER_HEALTH, "model", "ready"),
      ],
    });

    await screen.findByRole("heading", { name: "Second question" });
    for (const label of ["Microphone", "System audio", "Transcription", "AI provider"]) {
      expect(screen.getByLabelText(new RegExp(`^${label}: Ready$`, "i"))).toBeVisible();
    }
    expect(screen.queryByRole("button", { name: "Retry runtime" })).not.toBeInTheDocument();
  });

  it("pauses and resumes listening through protocol commands", async () => {
    const { platform } = renderLive();
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "Pause listening" }));
    await waitFor(() => expect(platform.send).toHaveBeenCalledWith(expect.objectContaining({
      kind: CommandKind.LISTENING_SET,
      session_id: sessionId,
      payload: { enabled: false },
    })));
    expect(screen.getByRole("button", { name: "Resume listening" })).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Resume listening" }));
    await waitFor(() => expect(platform.send).toHaveBeenLastCalledWith(expect.objectContaining({
      kind: CommandKind.LISTENING_SET,
      payload: { enabled: true },
    })));
  });

  it("keeps listening active and exposes a visible error when pause dispatch fails", async () => {
    const platform = createLivePlatform();
    vi.mocked(platform.send).mockRejectedValueOnce(new Error("Listening control unavailable"));
    renderLive({ platform });
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "Pause listening" }));

    expect(await screen.findByRole("alert", { name: "Live action failed" }))
      .toHaveTextContent("Listening control could not be changed.");
    expect(screen.queryByText("Listening control unavailable")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause listening" })).toBeVisible();
  });

  it("sends manual prompts with the selected answer format and a durable turn association", async () => {
    const { platform } = renderLive();
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("radio", { name: "Detailed" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Ask manually" }), {
      target: { value: "Compare both approaches" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Ask" }));

    await waitFor(() => expect(platform.associateRequestWithTurn).toHaveBeenCalledTimes(1));
    expect(platform.associateRequestWithTurn).toHaveBeenCalledWith(expect.objectContaining({
      sessionId,
      turnId: expect.stringMatching(/^[0-9a-f-]{36}$/i),
      requestId: expect.stringMatching(/^[0-9a-f-]{36}$/i),
    }));
    expect(platform.send).toHaveBeenCalledWith(expect.objectContaining({
      kind: CommandKind.QUERY_TRIGGER,
      session_id: sessionId,
      payload: { text: "Compare both approaches", answer_format: "summary" },
    }));
    expect(await screen.findByRole("heading", { name: "Compare both approaches" })).toBeVisible();
  });

  it("explains provider failures and only offers a valid runtime retry", async () => {
    const platform = createLivePlatform();
    renderLive({
      platform,
      healthEvents: [health(
        "018f0000-0000-7000-8000-000000000045",
        EventKind.PROVIDER_HEALTH,
        "model",
        "error",
        "Provider unavailable",
      )],
    });

    expect(await screen.findByLabelText("AI provider: Error")).toBeVisible();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "AI provider is in error. Transcript and session controls remain available.",
    );
    expect(screen.queryByText(/Provider unavailable/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry runtime" }));
    await waitFor(() => expect(platform.restartSidecar).toHaveBeenCalledTimes(1));
  });

  it("renders runtime failures with localized stable copy instead of provider text", async () => {
    await changeInterfaceLanguage("ar-XB");
    const { platform } = renderLive();
    await screen.findByRole("toolbar", { name: "[[ L~ive s~essi~on c~ontro~ls ]]" });

    act(() => platform.emit(runtimeError("Provider secret failure")));

    expect(await screen.findByRole("alert", { name: "[[ R~unti~me e~rror ]]" }))
      .toHaveTextContent("[[ L~ive a~ctio~n f~aile~d ]]");
    expect(screen.queryByText("Provider secret failure")).not.toBeInTheDocument();
  });

  it("keeps the primary toolbar controls in a predictable keyboard order", async () => {
    renderLive();
    await screen.findByRole("heading", { name: "Second question" });
    const toolbar = screen.getByRole("toolbar", { name: "Live session controls" });
    fireEvent.change(within(toolbar).getByRole("textbox", { name: "Ask manually" }), {
      target: { value: "Draft prompt" },
    });
    const controls = [...toolbar.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled]), select:not([disabled])")];

    expect(controls.map((element) => element.getAttribute("aria-label") ?? element.textContent?.trim())).toEqual([
      "Pause listening",
      "Ask manually",
      "Ask",
      "Concise",
      "Detailed",
      "Add note",
      "Pin current question",
      "Copy current answer",
      "End session",
    ]);
    expect(within(toolbar).queryByRole("combobox", { name: "Suggestion language" })).not.toBeInTheDocument();
  });

  it("uses pseudo-localized Live copy and accessible labels", async () => {
    await changeInterfaceLanguage("ar-XB");
    renderLive();

    expect(await screen.findByRole("toolbar", {
      name: "[[ L~ive s~essi~on c~ontro~ls ]]",
    })).toBeVisible();
    expect(screen.getByText("[[ C~urre~nt q~uest~ion ]]")).toBeVisible();
    expect(screen.getByRole("button", { name: "[[ P~ause l~iste~ning ]]" })).toBeVisible();
    expect(screen.getByRole("button", { name: "[[ S~how t~rans~crip~t ]]" })).toBeVisible();
    expect(screen.getByRole("status", { name: "[[ A~nswe~r u~pdat~es ]]" }))
      .toHaveTextContent("[[ A~nswe~r r~eady ]]");
  });

  it("does not announce streaming tokens and announces only a completed answer", async () => {
    const platform = createLivePlatform({
      timeline: [transcript(
        "018f0000-0000-7000-8000-000000000050",
        firstTurnId,
        "Explain the architecture",
        1,
      )],
      associations: [{ requestId: firstRequestId, turnId: firstTurnId }],
    });
    renderLive({ platform });
    await screen.findByRole("heading", { name: "Explain the architecture" });

    act(() => platform.emit(suggestion(
      "018f0000-0000-7000-8000-000000000051",
      "streaming-answer",
      "First token",
      2,
      firstRequestId,
      false,
    )));
    expect(screen.getByText("First token")).toBeVisible();
    expect(screen.getByText("First token").closest("[aria-live]")).toHaveAttribute("aria-live", "off");
    expect(screen.queryByRole("status", { name: "Answer updates" })).toHaveTextContent("");

    act(() => platform.emit(suggestion(
      "018f0000-0000-7000-8000-000000000052",
      "streaming-answer",
      "First token complete",
      3,
      firstRequestId,
      true,
    )));
    expect(screen.getByRole("status", { name: "Answer updates" })).toHaveTextContent("Answer ready");
  });

  it("fails closed when capture protection is not confirmed", async () => {
    const platform = createLivePlatform({ captureProtection: { state: "unavailable" } });
    renderLive({ platform });

    expect(await screen.findByRole("alert")).toHaveTextContent("Live is blocked until capture protection is confirmed");
    expect(screen.queryByRole("toolbar", { name: "Live session controls" })).not.toBeInTheDocument();
    expect(platform.send).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Retry capture protection" }));
    await waitFor(() => expect(platform.retryCaptureProtection).toHaveBeenCalledTimes(1));
  });

  it("waits for completed session state before persisting and opening Review", async () => {
    const platform = createLivePlatform();
    renderLive({ platform });
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "End session" }));
    await waitFor(() => expect(platform.send).toHaveBeenCalledWith(expect.objectContaining({
      kind: CommandKind.SESSION_STOP,
      session_id: sessionId,
      payload: {},
    })));
    expect(screen.queryByRole("heading", { name: "Session review" })).not.toBeInTheDocument();

    act(() => platform.emit({
      version: 1,
      id: "018f0000-0000-7000-8000-000000000060",
      session_id: sessionId,
      sequence: 10,
      timestamp_ms: 10,
      kind: EventKind.SESSION_STATE,
      payload: { state: "completed" },
      correlation_id: null,
    }));

    expect(await screen.findByRole("heading", { name: "Session review" })).toBeVisible();
    expect(platform.completeSession).toHaveBeenCalledWith(sessionId, "completed");
  });

  it("does not rewrite a confirmed completion when the host persistence acknowledgement fails", async () => {
    const platform = createLivePlatform();
    vi.mocked(platform.completeSession).mockRejectedValueOnce(new Error("Persistence acknowledgement unavailable"));
    renderLive({ platform });
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "End session" }));
    await waitFor(() => expect(platform.send).toHaveBeenCalledWith(expect.objectContaining({
      kind: CommandKind.SESSION_STOP,
    })));
    act(() => platform.emit({
      version: 1,
      id: "018f0000-0000-7000-8000-000000000061",
      session_id: sessionId,
      sequence: 11,
      timestamp_ms: 11,
      kind: EventKind.SESSION_STATE,
      payload: { state: "completed" },
      correlation_id: null,
    }));

    expect(await screen.findByRole("heading", { name: "Session review" })).toBeVisible();
    expect(platform.completeSession).toHaveBeenCalledTimes(1);
    expect(platform.completeSession).toHaveBeenCalledWith(sessionId, "completed");
  });

  it("persists an interrupted session and still opens Review when stop dispatch fails", async () => {
    const platform = createLivePlatform();
    vi.mocked(platform.send).mockRejectedValueOnce(new Error("Sidecar unavailable"));
    renderLive({ platform });
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "End session" }));

    expect(await screen.findByRole("heading", { name: "Session review" })).toBeVisible();
    expect(platform.completeSession).toHaveBeenCalledWith(sessionId, "interrupted");
  });

  it("exposes transcript and note entry points without changing the answer track", async () => {
    renderLive();
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "Show transcript" }));
    const timeline = screen.getByRole("region", { name: "Transcript timeline" });
    expect(within(timeline).getByText("First question")).toBeVisible();
    expect(within(timeline).getByText("Second question")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Add note" }));
    expect(screen.getByRole("textbox", { name: "Session note" })).toBeVisible();
  });

  it("persists a session note before reporting that it was kept", async () => {
    const platform = createLivePlatform();
    let resolveSave: (() => void) | undefined;
    vi.mocked(platform.saveSessionBrief).mockImplementation(() => new Promise<void>((resolve) => {
      resolveSave = resolve;
    }));
    renderLive({ platform });
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "Add note" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Session note" }), {
      target: { value: "Follow up with the retention metric." },
    });
    fireEvent.click(screen.getByRole("button", { name: "Keep draft" }));

    await waitFor(() => expect(platform.saveSessionBrief).toHaveBeenCalledWith({
      sessionId,
      brief: expect.objectContaining({
        reviewArtifacts: [expect.objectContaining({
          kind: "note",
          text: "Follow up with the retention metric.",
          createdAtMs: expect.any(Number),
        })],
      }),
    }));
    expect(screen.getByRole("textbox", { name: "Session note" }))
      .toHaveValue("Follow up with the retention metric.");
    expect(screen.getByRole("status", { name: "Live action status" })).not.toHaveTextContent("saved");

    await act(async () => { resolveSave?.(); });

    await waitFor(() => expect(screen.getByRole("status", { name: "Live action status" }))
      .toHaveTextContent("1 saved note"));
  });

  it("keeps an unsaved note open when persistence fails", async () => {
    const platform = createLivePlatform();
    vi.mocked(platform.saveSessionBrief).mockRejectedValueOnce(new Error("Encrypted storage unavailable"));
    renderLive({ platform });
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "Add note" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Session note" }), {
      target: { value: "Do not lose this note." },
    });
    fireEvent.click(screen.getByRole("button", { name: "Keep draft" }));

    expect(await screen.findByRole("alert", { name: "Runtime error" }))
      .toHaveTextContent("Live action failed");
    expect(screen.queryByText("Encrypted storage unavailable")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Session note" })).toHaveValue("Do not lose this note.");
    expect(screen.getByRole("status", { name: "Live action status" })).not.toHaveTextContent("saved");
  });

  it("persists pinned guidance with its question and completed answer", async () => {
    const { platform } = renderLive();
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "Pin current question" }));

    await waitFor(() => expect(platform.saveSessionBrief).toHaveBeenCalledWith({
      sessionId,
      brief: expect.objectContaining({
        reviewArtifacts: [expect.objectContaining({
          kind: "pin",
          turnId: secondTurnId,
          question: "Second question",
          suggestion: "The retained second answer",
          createdAtMs: expect.any(Number),
        })],
      }),
    }));
    expect(screen.getByRole("button", { name: "Unpin current question" })).toBeVisible();
  });

  it("keeps durable review artifacts in the session store across a Live remount", async () => {
    const platform = createLivePlatform();
    const { remountLive } = renderLive({ platform });
    await screen.findByRole("heading", { name: "Second question" });

    fireEvent.click(screen.getByRole("button", { name: "Add note" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Session note" }), {
      target: { value: "Preserve this durable note." },
    });
    fireEvent.click(screen.getByRole("button", { name: "Keep draft" }));
    await waitFor(() => expect(platform.saveSessionBrief).toHaveBeenCalledTimes(1));
    await screen.findByText("1 saved note");

    remountLive();
    await screen.findByRole("heading", { name: "Second question" });
    fireEvent.click(screen.getByRole("button", { name: "Pin current question" }));

    await waitFor(() => expect(platform.saveSessionBrief).toHaveBeenCalledTimes(2));
    expect(vi.mocked(platform.saveSessionBrief).mock.calls[1][0].brief).toEqual(expect.objectContaining({
      reviewArtifacts: expect.arrayContaining([
        expect.objectContaining({ kind: "note", text: "Preserve this durable note." }),
        expect.objectContaining({ kind: "pin", turnId: secondTurnId }),
      ]),
    }));
  });
});
