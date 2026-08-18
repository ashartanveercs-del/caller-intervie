import { RotateCw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams } from "react-router-dom";
import { useStore } from "zustand";
import { useRuntime } from "../../app/RuntimeProvider";
import { CommandKind, PROTOCOL_VERSION, type Envelope } from "../../shared/protocol";
import type { TranscriptTurn } from "../../stores/sessionStore";
import {
  briefWithReviewArtifacts,
  parseReviewArtifacts,
  type ReviewArtifact,
  type ReviewNote,
  type ReviewPin,
} from "../review/reviewArtifacts";
import { AnswerScaffold } from "./components/AnswerScaffold";
import { CurrentQuestion } from "./components/CurrentQuestion";
import { LiveToolbar, type AnswerFormat } from "./components/LiveToolbar";
import { RuntimeHealth } from "./components/RuntimeHealth";
import { TranscriptTimeline } from "./components/TranscriptTimeline";
import "./live.css";

type ManualQuestion = TranscriptTurn & {
  sourceTurnCount: number;
};

export function LivePage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { sessionId: routeSessionId } = useParams();
  const runtime = useRuntime();
  const { store } = runtime;
  const session = useStore(store, (state) => state.session);
  const turns = useStore(store, (state) => state.turns);
  const partialTurnsById = useStore(store, (state) => state.partialTurnsById);
  const suggestionsByTurn = useStore(store, (state) => state.suggestionsByTurn);
  const partialSuggestionsById = useStore(store, (state) => state.partialSuggestionsById);
  const health = useStore(store, (state) => state.health);
  const languages = useStore(store, (state) => state.languages);
  const lastError = useStore(store, (state) => state.lastError);
  const lastSequence = useStore(store, (state) => state.lastSequence);
  const [listening, setListening] = useState(true);
  const [answerFormat, setAnswerFormat] = useState<AnswerFormat>("chat");
  const [manualQuestions, setManualQuestions] = useState<ManualQuestion[]>([]);
  const [noteOpen, setNoteOpen] = useState(false);
  const [noteDraft, setNoteDraft] = useState("");
  const [reviewArtifacts, setReviewArtifacts] = useState<ReviewArtifact[]>([]);
  const [artifactBusy, setArtifactBusy] = useState(false);
  const reviewArtifactsRef = useRef<ReviewArtifact[]>([]);
  const artifactQueueRef = useRef<Promise<void>>(Promise.resolve());
  const artifactMutationRef = useRef(false);
  const [ending, setEnding] = useState(false);
  const [actionMessage, setActionMessage] = useState("");

  useEffect(() => {
    const artifacts = parseReviewArtifacts(session?.brief);
    reviewArtifactsRef.current = artifacts;
    artifactQueueRef.current = Promise.resolve();
    artifactMutationRef.current = false;
    setReviewArtifacts(artifacts);
    setArtifactBusy(false);
  }, [session?.brief, session?.id]);

  const finalQuestions = useMemo(() => turns.filter(isQuestionTurn), [turns]);
  const partialQuestions = useMemo(
    () => Object.values(partialTurnsById).filter(isQuestionTurn),
    [partialTurnsById],
  );
  const currentQuestion = useMemo(() => {
    const manual = manualQuestions[manualQuestions.length - 1];
    if (manual && finalQuestions.length <= manual.sourceTurnCount) return manual;
    const availableQuestions = [...finalQuestions, ...partialQuestions]
      .sort((left, right) => left.sequence - right.sequence);
    return availableQuestions[availableQuestions.length - 1];
  }, [finalQuestions, manualQuestions, partialQuestions]);
  const partialSuggestions = useMemo(() => Object.values(partialSuggestionsById), [partialSuggestionsById]);
  const currentAnswer = currentQuestion
    ? suggestionsByTurn[currentQuestion.id] ?? partialSuggestions.find((item) => item.turnId === currentQuestion.id)
    : undefined;
  const priorAnswers = useMemo(() => {
    const questionOrder = [...finalQuestions, ...manualQuestions]
      .filter((question) => question.id !== currentQuestion?.id)
      .sort((left, right) => left.sequence - right.sequence);
    return questionOrder.flatMap((question) => {
      const answer = suggestionsByTurn[question.id];
      return answer ? [{ question, answer }] : [];
    });
  }, [currentQuestion?.id, finalQuestions, manualQuestions, suggestionsByTurn]);
  const transcriptTurns = useMemo(
    () => [...turns, ...partialQuestions, ...manualQuestions].sort((left, right) => left.sequence - right.sequence),
    [manualQuestions, partialQuestions, turns],
  );
  const pinnedQuestions = useMemo(
    () => new Set(reviewArtifacts.flatMap((artifact) => artifact.kind === "pin" ? [artifact.turnId] : [])),
    [reviewArtifacts],
  );
  const savedNoteCount = reviewArtifacts.filter((artifact) => artifact.kind === "note").length;

  if (runtime.captureProtection.state !== "protected") {
    const unavailable = runtime.captureProtection.state === "unavailable";
    const message = unavailable
      ? t("live.blockers.capture.unavailable")
      : runtime.captureProtection.state === "unsupported"
        ? t("live.blockers.capture.unsupported")
        : t("live.blockers.capture.applying");
    return (
      <section className="live-page__blocker" role="alert">
        <h2>{t("live.blockers.capture.title")}</h2>
        <p>{message}</p>
        {unavailable ? (
          <button onClick={() => void runtime.retryCaptureProtection()} type="button">
            <RotateCw aria-hidden="true" size={17} />
            {t("live.blockers.capture.retry")}
          </button>
        ) : null}
      </section>
    );
  }

  if (!session || session.status !== "active") {
    return (
      <section className="live-page__blocker" role="alert">
        <h2>{t("live.blockers.noActive.title")}</h2>
        <p>{t("live.blockers.noActive.message")}</p>
      </section>
    );
  }

  if (routeSessionId !== session.id) {
    return (
      <section className="live-page__blocker" role="alert">
        <h2>{t("live.blockers.sessionMismatch.title")}</h2>
        <p>{t("live.blockers.sessionMismatch.message")}</p>
      </section>
    );
  }

  const setListeningState = async (enabled: boolean) => {
    setActionMessage("");
    await runtime.send(command(CommandKind.LISTENING_SET, session.id, { enabled }));
    setListening(enabled);
  };

  const sendManualPrompt = async (text: string) => {
    const turnId = crypto.randomUUID();
    const question: ManualQuestion = {
      id: turnId,
      text,
      isFinal: true,
      speechFinal: true,
      speakerRole: "manual",
      language: languages.input,
      sequence: Math.max(lastSequence + 1, 0),
      sourceTurnCount: finalQuestions.length,
    };
    setManualQuestions((items) => [...items, question]);
    setActionMessage("");
    try {
      await runtime.send(command(CommandKind.QUERY_TRIGGER, session.id, {
        text,
        answer_format: answerFormat,
      }), turnId);
    } catch (error) {
      setManualQuestions((items) => items.filter((item) => item.id !== turnId));
      throw error;
    }
  };

  const persistArtifacts = (
    update: (current: readonly ReviewArtifact[]) => ReviewArtifact[],
  ): Promise<ReviewArtifact[]> => {
    const activeSession = session;
    const operation = artifactQueueRef.current.then(async () => {
      const next = update(reviewArtifactsRef.current);
      const currentSession = store.getState().session;
      const nextBrief = briefWithReviewArtifacts(
        currentSession?.id === activeSession.id ? currentSession.brief : activeSession.brief,
        next,
      );
      await runtime.platform.saveSessionBrief({
        sessionId: activeSession.id,
        brief: nextBrief,
      });
      store.getState().updateSessionBrief(activeSession.id, nextBrief);
      reviewArtifactsRef.current = next;
      setReviewArtifacts(next);
      return next;
    });
    artifactQueueRef.current = operation.then(() => undefined, () => undefined);
    return operation;
  };

  const togglePin = async () => {
    if (!currentQuestion || artifactMutationRef.current) return;
    const alreadyPinned = pinnedQuestions.has(currentQuestion.id);
    if (!alreadyPinned && !currentAnswer?.isComplete) return;

    artifactMutationRef.current = true;
    setArtifactBusy(true);
    setActionMessage("");
    try {
      const pin: ReviewPin = {
        id: crypto.randomUUID(),
        kind: "pin",
        turnId: currentQuestion.id,
        question: currentQuestion.text,
        suggestion: currentAnswer?.text ?? "",
        createdAtMs: Date.now(),
      };
      await persistArtifacts((current) => alreadyPinned
        ? current.filter((artifact) => artifact.kind !== "pin" || artifact.turnId !== currentQuestion.id)
        : [...current, pin]);
      setActionMessage(t(alreadyPinned ? "live.status.unpinned" : "live.status.pinned"));
    } catch (error) {
      store.getState().recordError(error);
    } finally {
      artifactMutationRef.current = false;
      setArtifactBusy(false);
    }
  };

  const copyAnswer = async () => {
    if (!currentAnswer) return;
    try {
      if (!navigator.clipboard?.writeText) throw new Error(t("live.errors.clipboardUnavailable"));
      await navigator.clipboard.writeText(currentAnswer.text);
      setActionMessage(t("live.status.copied"));
    } catch (error) {
      store.getState().recordError(error);
    }
  };

  const keepNote = async () => {
    const note = noteDraft.trim();
    if (!note || artifactMutationRef.current) return;

    artifactMutationRef.current = true;
    setArtifactBusy(true);
    setActionMessage("");
    try {
      const artifact: ReviewNote = {
        id: crypto.randomUUID(),
        kind: "note",
        text: note,
        createdAtMs: Date.now(),
      };
      const next = await persistArtifacts((current) => [...current, artifact]);
      const count = next.filter((item) => item.kind === "note").length;
      setNoteDraft("");
      setNoteOpen(false);
      setActionMessage(t("live.status.savedNotes", { count }));
    } catch (error) {
      store.getState().recordError(error);
    } finally {
      artifactMutationRef.current = false;
      setArtifactBusy(false);
    }
  };

  const endSession = async () => {
    if (ending) return;
    setEnding(true);
    setActionMessage("");
    let terminalStatus: "completed" | "interrupted";
    try {
      await artifactQueueRef.current;
      if (health.sidecar.status === "error" || health.sidecar.status === "offline") {
        throw new Error(t("live.errors.sidecarUnavailable"));
      }
      await runtime.send(command(CommandKind.SESSION_STOP, session.id, {}));
      terminalStatus = await waitForTerminalSession(store, session.id, t("live.errors.stopTimeout"));
    } catch (error) {
      store.getState().recordError(error);
      store.getState().endSession("interrupted");
      terminalStatus = "interrupted";
    }
    try {
      await runtime.platform.completeSession(session.id, terminalStatus);
    } catch (persistenceError) {
      store.getState().recordError(persistenceError);
    }
    navigate(`/review/${session.id}`);
    setEnding(false);
  };

  const pinned = Boolean(currentQuestion && pinnedQuestions.has(currentQuestion.id));

  return (
    <section aria-busy={ending || artifactBusy} aria-label={t("live.title")} className="live-page">
      <div className="live-page__top">
        <RuntimeHealth health={health} onRetry={() => { void runtime.restart(); }} />
        <LiveToolbar
          answerFormat={answerFormat}
          artifactBusy={artifactBusy}
          canCopy={Boolean(currentAnswer?.text)}
          canPin={Boolean(currentAnswer?.isComplete) || pinned}
          ending={ending}
          listening={listening}
          noteOpen={noteOpen}
          onAnswerFormatChange={setAnswerFormat}
          onCopy={() => { void copyAnswer(); }}
          onEnd={() => { void endSession(); }}
          onManualPrompt={sendManualPrompt}
          onNote={() => setNoteOpen((value) => !value)}
          onPin={() => { void togglePin(); }}
          onSetListening={setListeningState}
          pinned={pinned}
        />
        {noteOpen ? (
          <section aria-label={t("live.notes.draftLabel")} className="live-page__note-composer">
            <label htmlFor="live-note">{t("live.notes.sessionNote")}</label>
            <textarea
              id="live-note"
              onChange={(event) => setNoteDraft(event.target.value)}
              rows={3}
              value={noteDraft}
            />
            <div>
              <button disabled={artifactBusy} onClick={() => setNoteOpen(false)} type="button">
                {t("live.notes.cancel")}
              </button>
              <button
                disabled={artifactBusy || !noteDraft.trim()}
                onClick={() => { void keepNote(); }}
                type="button"
              >
                {artifactBusy ? t("live.notes.saving") : t("live.notes.keep")}
              </button>
            </div>
          </section>
        ) : null}
      </div>

      <div className="live-page__workspace">
        <CurrentQuestion
          isFinal={currentQuestion?.isFinal}
          isManual={currentQuestion?.speakerRole === "manual"}
          language={currentQuestion?.language}
          text={currentQuestion?.text}
        />
        <div className="live-page__answer-track">
          <AnswerScaffold
            announce
            answer={currentAnswer}
            waitingForQuestion={!currentQuestion}
          />
          {priorAnswers.length > 0 ? (
            <section aria-labelledby="previous-answers-heading" className="live-page__previous-answers">
              <h3 id="previous-answers-heading">{t("live.answer.previous")}</h3>
              <div className="live-page__previous-list">
                {priorAnswers.map(({ question, answer }, index) => (
                  <AnswerScaffold
                    answer={answer}
                    expandLabel={t("live.answer.expandPrevious", { ordinal: ordinal(index + 1, t) })}
                    heading={question.text}
                    key={question.id}
                  />
                ))}
              </div>
            </section>
          ) : null}
        </div>
      </div>

      <TranscriptTimeline turns={transcriptTurns} />
      <div
        aria-atomic="true"
        aria-label={t("live.status.action")}
        aria-live="polite"
        className="live-page__action-status"
        role="status"
      >
        {actionMessage || (savedNoteCount > 0 ? t("live.status.savedNotes", { count: savedNoteCount }) : "")}
      </div>
      {lastError ? (
        <p aria-label={t("live.status.runtimeError")} className="live-page__last-error" role="alert">
          {t("live.status.actionFailed")}
        </p>
      ) : null}
    </section>
  );
}

function isQuestionTurn(turn: TranscriptTurn): boolean {
  switch (turn.speakerRole?.toLowerCase()) {
    case "interviewee":
    case "candidate":
    case "you": return false;
    default: return true;
  }
}

function command(kind: CommandKind, sessionId: string, payload: Record<string, unknown>): Envelope {
  return {
    version: PROTOCOL_VERSION,
    id: crypto.randomUUID(),
    session_id: sessionId,
    sequence: 0,
    timestamp_ms: Date.now(),
    kind,
    payload,
    correlation_id: null,
  };
}

function waitForTerminalSession(
  store: ReturnType<typeof useRuntime>["store"],
  sessionId: string,
  timeoutMessage: string,
): Promise<"completed" | "interrupted"> {
  const current = store.getState().session;
  if (current?.id === sessionId && current.status !== "active") return Promise.resolve(current.status);

  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      unsubscribe();
      reject(new Error(timeoutMessage));
    }, 8_000);
    const unsubscribe = store.subscribe((state) => {
      if (state.session?.id !== sessionId || state.session.status === "active") return;
      window.clearTimeout(timeout);
      unsubscribe();
      resolve(state.session.status);
    });
  });
}

type Translate = ReturnType<typeof useTranslation>["t"];

function ordinal(value: number, t: Translate): string {
  if (value === 1) return t("live.answer.ordinals.first");
  if (value === 2) return t("live.answer.ordinals.second");
  if (value === 3) return t("live.answer.ordinals.third");
  return t("live.answer.ordinals.other", { count: value });
}
